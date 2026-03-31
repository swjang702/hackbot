// SPDX-License-Identifier: GPL-2.0

//! Agent memory ring buffer and autonomous patrol tick.
//!
//! Stores structured findings from both patrol cycles and user sessions.
//! Each finding includes: summary (extracted first sentence), tools used,
//! tool call count, and detail text. Inspired by HyperAgents (Meta 2026)
//! which showed that structured memory with metadata enables cross-session
//! learning and self-improvement.
//!
//! The memory is injected into the vLLM system prompt so the agent can
//! reference past observations when answering new queries.

use kernel::{bindings, prelude::*};
#[allow(unused_imports)]
use kernel::sync::Mutex;

use crate::config::*;
use crate::net::format_usize;

// ---------------------------------------------------------------------------
// Structured memory entry
// ---------------------------------------------------------------------------

/// Maximum length of the extracted summary (first sentence).
const SUMMARY_MAX: usize = 128;
/// Maximum length of tools-used string (e.g., "ps,mem,trace sched").
const TOOLS_MAX: usize = 64;
/// Maximum length of detail text (remainder after summary).
const DETAIL_MAX: usize = 384;

/// A structured finding in the agent memory (HyperAgents-inspired).
///
/// Instead of raw text blobs, each finding stores:
/// - `summary`: first sentence of the response (the key insight)
/// - `tools_used`: which tools produced this finding
/// - `n_tool_calls`: investigation effort
/// - `detail`: supporting evidence (rest of response, truncated)
struct MemoryEntry {
    /// Uptime in seconds when the finding was recorded.
    timestamp_secs: u64,
    /// Source tag: "patrol" or "user".
    source: [u8; 8],
    source_len: usize,
    /// First sentence of the response — the key insight.
    summary: [u8; SUMMARY_MAX],
    summary_len: usize,
    /// Tools used during this investigation (e.g., "ps,mem,trace sched").
    tools_used: [u8; TOOLS_MAX],
    tools_len: usize,
    /// Number of tool calls in this session.
    n_tool_calls: u8,
    /// Rest of the response for additional context.
    detail: [u8; DETAIL_MAX],
    detail_len: usize,
    /// Is this slot occupied?
    occupied: bool,
}

impl MemoryEntry {
    const EMPTY: Self = Self {
        timestamp_secs: 0,
        source: [0u8; 8],
        source_len: 0,
        summary: [0u8; SUMMARY_MAX],
        summary_len: 0,
        tools_used: [0u8; TOOLS_MAX],
        tools_len: 0,
        n_tool_calls: 0,
        detail: [0u8; DETAIL_MAX],
        detail_len: 0,
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
// Summary extraction
// ---------------------------------------------------------------------------

/// Extract the first sentence from text as a summary.
///
/// Looks for the first `.` followed by a space, or the first newline,
/// whichever comes first. Falls back to the first `SUMMARY_MAX` bytes.
fn extract_summary<'a>(text: &'a [u8]) -> &'a [u8] {
    let max = text.len().min(SUMMARY_MAX);
    let chunk = &text[..max];

    for (i, &b) in chunk.iter().enumerate() {
        // Stop at newline
        if b == b'\n' {
            return &chunk[..i];
        }
        // Stop at period followed by space (end of sentence)
        if b == b'.' && i + 1 < max && chunk[i + 1] == b' ' {
            return &chunk[..i + 1];
        }
    }

    // No sentence boundary found — return full chunk
    chunk
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Record a structured finding into the agent memory ring buffer.
///
/// `source`: "patrol" or "user"
/// `text`: full LLM response
/// `tools_used`: comma-separated tool names (e.g., "ps,mem,trace sched")
/// `n_tool_calls`: number of tool invocations in this session
pub(crate) fn record_finding(source: &[u8], text: &[u8], tools_used: &[u8], n_tool_calls: u8) {
    let ns = unsafe { bindings::ktime_get_boot_fast_ns() };
    let secs = ns / 1_000_000_000;

    // Extract summary (first sentence)
    let summary_slice = extract_summary(text);

    // Detail = remainder after summary, capped at DETAIL_MAX
    let detail_start = summary_slice.len().min(text.len());
    // Skip leading whitespace/newlines in detail
    let detail_start = text[detail_start..].iter()
        .position(|&b| !b.is_ascii_whitespace())
        .map_or(text.len(), |p| detail_start + p);
    let detail_slice = &text[detail_start..text.len().min(detail_start + DETAIL_MAX)];

    let mut guard = MEMORY.lock();
    let idx = guard.head;
    let entry = &mut guard.entries[idx];

    entry.timestamp_secs = secs;

    // Source
    let src_len = source.len().min(entry.source.len());
    entry.source[..src_len].copy_from_slice(&source[..src_len]);
    entry.source_len = src_len;

    // Summary
    let sum_len = summary_slice.len().min(SUMMARY_MAX);
    entry.summary[..sum_len].copy_from_slice(&summary_slice[..sum_len]);
    entry.summary_len = sum_len;

    // Tools used
    let tools_len = tools_used.len().min(TOOLS_MAX);
    entry.tools_used[..tools_len].copy_from_slice(&tools_used[..tools_len]);
    entry.tools_len = tools_len;

    // Tool call count
    entry.n_tool_calls = n_tool_calls;

    // Detail
    let det_len = detail_slice.len().min(DETAIL_MAX);
    entry.detail[..det_len].copy_from_slice(&detail_slice[..det_len]);
    entry.detail_len = det_len;

    entry.occupied = true;

    guard.head = (idx + 1) % MEMORY_MAX_ENTRIES;
    guard.total_recorded += 1;
    let total = guard.total_recorded;
    drop(guard);

    let src = core::str::from_utf8(source).unwrap_or("?");
    pr_info!("hackbot: memory: recorded finding #{} from '{}' (summary {}B, tools {}B, {} calls)\n",
             total, src, sum_len, tools_len, n_tool_calls);
}

/// Format the agent memory for injection into the vLLM system prompt.
///
/// Produces structured output like:
/// ```text
/// === AGENT MEMORY (3 findings) ===
/// [+42m] (patrol) Load average elevated. httpd dominating CPU.
///   tools: ps,loadavg,trace sched | 3 calls
///
/// [+45m] (user) 55% RAM used, no swap configured.
///   tools: mem,files 1 | 2 calls
///
/// [+120m] (patrol) System nominal.
///   tools: loadavg,trace io | 2 calls
/// ===
/// ```
pub(crate) fn format_memory_for_prompt(buf: &mut KVVec<u8>) {
    let guard = MEMORY.lock();

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

    for i in 0..MEMORY_MAX_ENTRIES {
        let idx = (guard.head + i) % MEMORY_MAX_ENTRIES;
        let entry = &guard.entries[idx];
        if !entry.occupied {
            continue;
        }

        // Timestamp
        let mins = entry.timestamp_secs / 60;
        let _ = buf.extend_from_slice(b"[+", GFP_KERNEL);
        let _ = buf.extend_from_slice(format_usize(mins as usize, &mut num), GFP_KERNEL);
        let _ = buf.extend_from_slice(b"m] (", GFP_KERNEL);

        // Source
        let _ = buf.extend_from_slice(&entry.source[..entry.source_len], GFP_KERNEL);
        let _ = buf.extend_from_slice(b") ", GFP_KERNEL);

        // Summary (first sentence)
        let _ = buf.extend_from_slice(&entry.summary[..entry.summary_len], GFP_KERNEL);
        let _ = buf.push(b'\n', GFP_KERNEL);

        // Tools metadata line
        if entry.tools_len > 0 {
            let _ = buf.extend_from_slice(b"  tools: ", GFP_KERNEL);
            let _ = buf.extend_from_slice(&entry.tools_used[..entry.tools_len], GFP_KERNEL);
            let _ = buf.extend_from_slice(b" | ", GFP_KERNEL);
            let _ = buf.extend_from_slice(format_usize(entry.n_tool_calls as usize, &mut num), GFP_KERNEL);
            let _ = buf.extend_from_slice(b" calls\n", GFP_KERNEL);
        }
    }

    let _ = buf.extend_from_slice(b"===\n\n", GFP_KERNEL);
}

// ---------------------------------------------------------------------------
// Patrol tick — called from C kthread
// ---------------------------------------------------------------------------

/// Called by the C kthread (`hackbot_patrol.c`) every PATROL_INTERVAL_SECS.
#[no_mangle]
#[allow(unreachable_pub)]
pub extern "C" fn hackbot_patrol_tick() {
    pr_info!("hackbot: patrol cycle starting\n");

    match crate::vllm::agent_loop(PATROL_PROMPT) {
        Ok(response) if !response.is_empty() => {
            // Record finding (patrol doesn't track individual tools — pass empty)
            record_finding(SOURCE_PATROL, &response, b"", 0);

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
