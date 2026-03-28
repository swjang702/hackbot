// SPDX-License-Identifier: GPL-2.0

//! Agent memory ring buffer and autonomous patrol tick.
//!
//! Stores timestamped findings from both patrol cycles and user sessions.
//! The memory is injected into the vLLM system prompt so the agent can
//! reference past observations when answering new queries.
//!
//! The patrol tick function (`hackbot_patrol_tick`) is called from the
//! C kthread helper (`hackbot_patrol.c`) every PATROL_INTERVAL_SECS.

use kernel::{bindings, prelude::*};
#[allow(unused_imports)]
use kernel::sync::Mutex;

use crate::config::*;
use crate::net::format_usize;

// ---------------------------------------------------------------------------
// Memory entry and ring buffer
// ---------------------------------------------------------------------------

/// A single timestamped finding in the agent memory.
struct MemoryEntry {
    /// Uptime in seconds when the finding was recorded.
    timestamp_secs: u64,
    /// Source tag: "patrol" or "user".
    source: [u8; 8],
    source_len: usize,
    /// The finding text, truncated to MEMORY_MAX_ENTRY_SIZE.
    text: [u8; MEMORY_MAX_ENTRY_SIZE],
    text_len: usize,
    /// Is this slot occupied?
    occupied: bool,
}

impl MemoryEntry {
    const EMPTY: Self = Self {
        timestamp_secs: 0,
        source: [0u8; 8],
        source_len: 0,
        text: [0u8; MEMORY_MAX_ENTRY_SIZE],
        text_len: 0,
        occupied: false,
    };
}

/// Ring buffer of agent findings.
pub(crate) struct AgentMemory {
    entries: [MemoryEntry; MEMORY_MAX_ENTRIES],
    /// Index of the next slot to write (wraps around).
    head: usize,
    /// Total findings ever recorded.
    total_recorded: usize,
}

kernel::sync::global_lock! {
    // SAFETY: Initialized in HackbotModule::init() before patrol thread starts.
    pub(crate) unsafe(uninit) static MEMORY: Mutex<AgentMemory> = AgentMemory {
        entries: [MemoryEntry::EMPTY; MEMORY_MAX_ENTRIES],
        head: 0,
        total_recorded: 0,
    };
}

/// Initialize the MEMORY global lock. Must be called once during module init.
pub(crate) fn init_memory() {
    // SAFETY: Called exactly once from HackbotModule::init().
    unsafe { MEMORY.init() };
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Record a finding into the agent memory ring buffer.
///
/// `source` is a short tag like "patrol" or "user".
/// `text` is the finding content (truncated to MEMORY_MAX_ENTRY_SIZE).
pub(crate) fn record_finding(source: &[u8], text: &[u8]) {
    let ns = unsafe { bindings::ktime_get_boot_fast_ns() };
    let secs = ns / 1_000_000_000;

    let mut guard = MEMORY.lock();
    let idx = guard.head;

    let entry = &mut guard.entries[idx];
    entry.timestamp_secs = secs;

    // Copy source tag
    let src_len = source.len().min(entry.source.len());
    entry.source[..src_len].copy_from_slice(&source[..src_len]);
    entry.source_len = src_len;

    // Copy text, truncated
    let txt_len = text.len().min(MEMORY_MAX_ENTRY_SIZE);
    entry.text[..txt_len].copy_from_slice(&text[..txt_len]);
    entry.text_len = txt_len;

    entry.occupied = true;

    guard.head = (idx + 1) % MEMORY_MAX_ENTRIES;
    guard.total_recorded += 1;
    let total = guard.total_recorded;
    drop(guard);

    let src = core::str::from_utf8(source).unwrap_or("?");
    pr_info!("hackbot: memory: recorded finding #{} from '{}' ({} bytes)\n",
             total, src, txt_len);
}

/// Format the agent memory for injection into the vLLM system prompt.
///
/// Produces output like:
/// ```text
/// === AGENT MEMORY (3 findings) ===
/// [+42m] (patrol) Load average elevated. httpd dominating CPU.
/// [+45m] (user) Memory pressure noted.
/// [+51m] (patrol) System nominal.
/// ===
/// ```
///
/// Entries are listed oldest-first (chronological order).
pub(crate) fn format_memory_for_prompt(buf: &mut KVVec<u8>) {
    let guard = MEMORY.lock();

    // Count occupied entries
    let mut count = 0usize;
    for entry in &guard.entries {
        if entry.occupied {
            count += 1;
        }
    }

    if count == 0 {
        pr_info!("hackbot: memory: no findings to inject into prompt\n");
        return;
    }

    pr_info!("hackbot: memory: injecting {} findings into system prompt\n", count);

    let mut num = [0u8; 20];

    let _ = buf.extend_from_slice(b"=== AGENT MEMORY (", GFP_KERNEL);
    let _ = buf.extend_from_slice(format_usize(count, &mut num), GFP_KERNEL);
    let _ = buf.extend_from_slice(b" findings) ===\n", GFP_KERNEL);

    // Iterate oldest-first: from head (next write = oldest if full) to head-1
    for i in 0..MEMORY_MAX_ENTRIES {
        let idx = (guard.head + i) % MEMORY_MAX_ENTRIES;
        let entry = &guard.entries[idx];
        if !entry.occupied {
            continue;
        }

        // Format timestamp as relative uptime in minutes: [+42m]
        let mins = entry.timestamp_secs / 60;
        let _ = buf.extend_from_slice(b"[+", GFP_KERNEL);
        let _ = buf.extend_from_slice(format_usize(mins as usize, &mut num), GFP_KERNEL);
        let _ = buf.extend_from_slice(b"m] (", GFP_KERNEL);

        // Source tag
        let _ = buf.extend_from_slice(&entry.source[..entry.source_len], GFP_KERNEL);
        let _ = buf.extend_from_slice(b") ", GFP_KERNEL);

        // Finding text — take first line only (up to newline) for brevity
        let text = &entry.text[..entry.text_len];
        let first_line_end = text.iter().position(|&b| b == b'\n').unwrap_or(text.len());
        let line = &text[..first_line_end.min(256)]; // cap at 256 chars per line
        let _ = buf.extend_from_slice(line, GFP_KERNEL);
        let _ = buf.push(b'\n', GFP_KERNEL);
    }

    let _ = buf.extend_from_slice(b"===\n\n", GFP_KERNEL);
}

// ---------------------------------------------------------------------------
// Patrol tick — called from C kthread
// ---------------------------------------------------------------------------

/// Called by the C kthread (`hackbot_patrol.c`) every PATROL_INTERVAL_SECS.
///
/// Runs the vLLM agent loop with the patrol prompt, records findings
/// into the memory ring buffer, and logs to dmesg.
///
/// # Safety
///
/// Must be called from process context (sleepable). The C kthread
/// satisfies this requirement.
#[no_mangle]
#[allow(unreachable_pub)]
pub extern "C" fn hackbot_patrol_tick() {
    pr_info!("hackbot: patrol cycle starting\n");

    match crate::vllm::agent_loop(PATROL_PROMPT) {
        Ok(response) if !response.is_empty() => {
            // Record finding
            record_finding(SOURCE_PATROL, &response);

            // Log a summary to dmesg (first 200 bytes)
            let preview_len = response.len().min(200);
            let preview = &response[..preview_len];
            if let Ok(s) = core::str::from_utf8(preview) {
                pr_info!("hackbot: patrol finding: {}\n", s);
            } else {
                pr_info!("hackbot: patrol finding: ({} bytes, non-UTF8)\n", response.len());
            }
        }
        Ok(_) => {
            pr_info!("hackbot: patrol cycle: empty response\n");
        }
        Err(e) => {
            pr_warn!("hackbot: patrol cycle failed: error {}\n", e.to_errno());
        }
    }
}
