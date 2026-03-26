// SPDX-License-Identifier: GPL-2.0

//! Kernel observation tools (Tier 0 — read-only) and tool call parser.

use core::mem;
use core::mem::MaybeUninit;
use core::ptr;

use kernel::{bindings, prelude::*, sync::rcu};

use crate::config::*;
use crate::context::{append_uptime, read_num_online_cpus};
use crate::net::format_usize;
use crate::types::avenrun;

// ---------------------------------------------------------------------------
// Tool call parser
// ---------------------------------------------------------------------------

/// Result of parsing an LLM response for tool calls.
pub(crate) enum ToolCallResult<'a> {
    ToolCall { name: &'a [u8], prefix: &'a [u8] },
    FinalAnswer(&'a [u8]),
}

/// Parse an LLM response looking for a `<tool>NAME</tool>` tag.
pub(crate) fn parse_tool_call(response: &[u8]) -> ToolCallResult<'_> {
    let open_tag = b"<tool>";

    if let Some(open_pos) = crate::net::find_subsequence(response, open_tag) {
        let content_start = open_pos + open_tag.len();
        let remaining = &response[content_start..];

        let close_tag = b"</tool>";
        let name_end = if let Some(close_offset) = crate::net::find_subsequence(remaining, close_tag) {
            close_offset
        } else {
            remaining
                .iter()
                .position(|&b| b == b'\n' || b == b'\r')
                .unwrap_or(remaining.len())
        };

        let name = trim_ascii(&remaining[..name_end]);
        let name = if name.len() > 32 { &name[..32] } else { name };
        if !name.is_empty() {
            return ToolCallResult::ToolCall {
                name,
                prefix: &response[..open_pos],
            };
        }
    }
    ToolCallResult::FinalAnswer(response)
}

fn trim_ascii(s: &[u8]) -> &[u8] {
    let start = s.iter().position(|&b| !b.is_ascii_whitespace()).unwrap_or(s.len());
    let end = s.iter().rposition(|&b| !b.is_ascii_whitespace()).map_or(start, |p| p + 1);
    &s[start..end]
}

// ---------------------------------------------------------------------------
// Tool execution
// ---------------------------------------------------------------------------

/// Execute a tool by name and return its output.
pub(crate) fn execute_tool(name: &[u8]) -> KVVec<u8> {
    let mut output = KVVec::new();

    match name {
        b"ps" => tool_ps(&mut output),
        b"mem" => tool_mem(&mut output),
        b"loadavg" => tool_loadavg(&mut output),
        _ => {
            let _ = output.extend_from_slice(b"[Error: unknown tool '", GFP_KERNEL);
            let _ = output.extend_from_slice(name, GFP_KERNEL);
            let _ = output.extend_from_slice(
                b"'. Available tools: ps, mem, loadavg]\n",
                GFP_KERNEL,
            );
        }
    }

    if output.is_empty() {
        let _ = output.extend_from_slice(b"[No output]\n", GFP_KERNEL);
    }

    const TRUNCATION_SUFFIX: &[u8] = b"\n[... truncated]\n";
    if output.len() > MAX_TOOL_OUTPUT - TRUNCATION_SUFFIX.len() {
        output.truncate(MAX_TOOL_OUTPUT - TRUNCATION_SUFFIX.len());
        let _ = output.extend_from_slice(TRUNCATION_SUFFIX, GFP_KERNEL);
    }

    output
}

/// Format a single task_struct into the ps output line.
///
/// # Safety
/// `task` must be a valid pointer to a task_struct under RCU protection.
unsafe fn format_task(output: &mut KVVec<u8>, task: *const bindings::task_struct) {
    let pid = unsafe { (*task).pid };
    let ppid = unsafe {
        let parent = (*task).real_parent;
        if parent.is_null() { 0 } else { (*parent).pid }
    };
    let state = unsafe { (*task).__state };
    let comm = unsafe { &(*task).comm };

    let mut num = [0u8; 20];

    let s = format_usize(pid as usize, &mut num);
    let _ = output.extend_from_slice(s, GFP_KERNEL);
    for _ in 0..(8usize.saturating_sub(s.len())) {
        let _ = output.push(b' ', GFP_KERNEL);
    }

    let s = format_usize(ppid as usize, &mut num);
    let _ = output.extend_from_slice(s, GFP_KERNEL);
    for _ in 0..(8usize.saturating_sub(s.len())) {
        let _ = output.push(b' ', GFP_KERNEL);
    }

    let state_ch = match state {
        0 => b'R',
        1 => b'S',
        2 => b'D',
        4 => b'T',
        8 => b'T',
        0x40 => b'Z',
        0x20 => b'X',
        0x402 => b'I',
        _ => b'?',
    };
    let _ = output.push(state_ch, GFP_KERNEL);
    let _ = output.extend_from_slice(b"      ", GFP_KERNEL);

    for &c in comm.iter() {
        if c == 0 {
            break;
        }
        let _ = output.push(c as u8, GFP_KERNEL);
    }
    let _ = output.push(b'\n', GFP_KERNEL);
}

/// Tool: `ps` — list running processes by walking the kernel task list.
fn tool_ps(output: &mut KVVec<u8>) {
    let tasks_offset = mem::offset_of!(bindings::task_struct, tasks);

    let _rcu = rcu::read_lock();

    let init = ptr::addr_of!(bindings::init_task);
    let list_head = unsafe { ptr::addr_of!((*init).tasks) };

    let mut count = 0usize;

    // Pass 1: User-space processes (mm != NULL).
    let _ = output.extend_from_slice(
        b"=== User-Space Processes ===\nPID      PPID     STATE  COMM\n",
        GFP_KERNEL,
    );
    let mut current = unsafe { (*list_head).next };
    while current != list_head as *mut bindings::list_head && count < MAX_PS_TASKS {
        let task = unsafe {
            (current as *const u8).sub(tasks_offset) as *const bindings::task_struct
        };
        let mm = unsafe { (*task).mm };
        if !mm.is_null() {
            unsafe { format_task(output, task) };
            count += 1;
        }
        current = unsafe { (*current).next };
    }

    // Pass 2: Kernel threads (mm == NULL).
    if count < MAX_PS_TASKS {
        let _ = output.extend_from_slice(
            b"\n=== Kernel Threads ===\nPID      PPID     STATE  COMM\n",
            GFP_KERNEL,
        );
        current = unsafe { (*list_head).next };
        while current != list_head as *mut bindings::list_head && count < MAX_PS_TASKS {
            let task = unsafe {
                (current as *const u8).sub(tasks_offset) as *const bindings::task_struct
            };
            let mm = unsafe { (*task).mm };
            if mm.is_null() {
                unsafe { format_task(output, task) };
                count += 1;
            }
            current = unsafe { (*current).next };
        }
    }

    if count >= MAX_PS_TASKS {
        let _ = output.extend_from_slice(b"[... truncated]\n", GFP_KERNEL);
    }
}

/// Tool: `mem` — detailed memory statistics via si_meminfo().
fn tool_mem(output: &mut KVVec<u8>) {
    let mut info: bindings::sysinfo = unsafe { MaybeUninit::zeroed().assume_init() };
    unsafe { bindings::si_meminfo(&mut info) };

    let unit = info.mem_unit as usize;
    let total_mb = (info.totalram as usize * unit) / (1024 * 1024);
    let free_mb = (info.freeram as usize * unit) / (1024 * 1024);
    let used_mb = total_mb.saturating_sub(free_mb);
    let shared_mb = (info.sharedram as usize * unit) / (1024 * 1024);
    let buffers_mb = (info.bufferram as usize * unit) / (1024 * 1024);
    let swap_total_mb = (info.totalswap as usize * unit) / (1024 * 1024);
    let swap_free_mb = (info.freeswap as usize * unit) / (1024 * 1024);
    let swap_used_mb = swap_total_mb.saturating_sub(swap_free_mb);

    let mut num = [0u8; 20];

    let _ = output.extend_from_slice(b"=== Memory Statistics ===\n", GFP_KERNEL);

    let _ = output.extend_from_slice(b"Total RAM:   ", GFP_KERNEL);
    let _ = output.extend_from_slice(format_usize(total_mb, &mut num), GFP_KERNEL);
    let _ = output.extend_from_slice(b" MB\n", GFP_KERNEL);

    let _ = output.extend_from_slice(b"Used RAM:    ", GFP_KERNEL);
    let _ = output.extend_from_slice(format_usize(used_mb, &mut num), GFP_KERNEL);
    let _ = output.extend_from_slice(b" MB\n", GFP_KERNEL);

    let _ = output.extend_from_slice(b"Free RAM:    ", GFP_KERNEL);
    let _ = output.extend_from_slice(format_usize(free_mb, &mut num), GFP_KERNEL);
    let _ = output.extend_from_slice(b" MB\n", GFP_KERNEL);

    let _ = output.extend_from_slice(b"Shared:      ", GFP_KERNEL);
    let _ = output.extend_from_slice(format_usize(shared_mb, &mut num), GFP_KERNEL);
    let _ = output.extend_from_slice(b" MB\n", GFP_KERNEL);

    let _ = output.extend_from_slice(b"Buffers:     ", GFP_KERNEL);
    let _ = output.extend_from_slice(format_usize(buffers_mb, &mut num), GFP_KERNEL);
    let _ = output.extend_from_slice(b" MB\n", GFP_KERNEL);

    let _ = output.extend_from_slice(b"Swap Total:  ", GFP_KERNEL);
    let _ = output.extend_from_slice(format_usize(swap_total_mb, &mut num), GFP_KERNEL);
    let _ = output.extend_from_slice(b" MB\n", GFP_KERNEL);

    let _ = output.extend_from_slice(b"Swap Used:   ", GFP_KERNEL);
    let _ = output.extend_from_slice(format_usize(swap_used_mb, &mut num), GFP_KERNEL);
    let _ = output.extend_from_slice(b" MB\n", GFP_KERNEL);

    let _ = output.extend_from_slice(b"Swap Free:   ", GFP_KERNEL);
    let _ = output.extend_from_slice(format_usize(swap_free_mb, &mut num), GFP_KERNEL);
    let _ = output.extend_from_slice(b" MB\n", GFP_KERNEL);

    if total_mb > 0 {
        let pct = (used_mb * 100) / total_mb;
        let _ = output.extend_from_slice(b"RAM Usage:   ", GFP_KERNEL);
        let _ = output.extend_from_slice(format_usize(pct, &mut num), GFP_KERNEL);
        let _ = output.extend_from_slice(b"%\n", GFP_KERNEL);
    }
}

/// Tool: `loadavg` — system load averages from the kernel's avenrun[] array.
fn tool_loadavg(output: &mut KVVec<u8>) {
    const FSHIFT: usize = 11;
    const FIXED_1: usize = 1 << FSHIFT;

    // SAFETY: avenrun is a global exported symbol.
    let avg1 = unsafe { ptr::read_volatile(ptr::addr_of!(avenrun[0])) } as usize;
    let avg5 = unsafe { ptr::read_volatile(ptr::addr_of!(avenrun[1])) } as usize;
    let avg15 = unsafe { ptr::read_volatile(ptr::addr_of!(avenrun[2])) } as usize;

    let mut num = [0u8; 20];

    let _ = output.extend_from_slice(b"=== Load Averages ===\n", GFP_KERNEL);

    for (label, val) in [
        (b"1 min:   " as &[u8], avg1),
        (b"5 min:   ", avg5),
        (b"15 min:  ", avg15),
    ] {
        let _ = output.extend_from_slice(label, GFP_KERNEL);
        let integer = val / FIXED_1;
        let frac = (val % FIXED_1) * 100 / FIXED_1;
        let _ = output.extend_from_slice(format_usize(integer, &mut num), GFP_KERNEL);
        let _ = output.push(b'.', GFP_KERNEL);
        if frac < 10 {
            let _ = output.push(b'0', GFP_KERNEL);
        }
        let _ = output.extend_from_slice(format_usize(frac, &mut num), GFP_KERNEL);
        let _ = output.push(b'\n', GFP_KERNEL);
    }

    let cpus = read_num_online_cpus();
    let _ = output.extend_from_slice(b"Online CPUs: ", GFP_KERNEL);
    let _ = output.extend_from_slice(format_usize(cpus, &mut num), GFP_KERNEL);
    let _ = output.push(b'\n', GFP_KERNEL);

    let _ = output.extend_from_slice(b"Uptime:      ", GFP_KERNEL);
    append_uptime(output);
    let _ = output.push(b'\n', GFP_KERNEL);
}
