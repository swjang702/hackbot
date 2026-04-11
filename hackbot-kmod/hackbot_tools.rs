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

/// Split a raw tool invocation into (name, args).
/// e.g. `b"files 1234"` → `(b"files", b"1234")`, `b"ps"` → `(b"ps", b"")`.
fn split_tool_args(raw: &[u8]) -> (&[u8], &[u8]) {
    match raw.iter().position(|&b| b == b' ') {
        Some(pos) => (&raw[..pos], trim_ascii(&raw[pos + 1..])),
        None => (raw, &[] as &[u8]),
    }
}

// ---------------------------------------------------------------------------
// Tool execution
// ---------------------------------------------------------------------------

/// Execute a tool by name (with optional arguments) and return its output.
/// `raw` is the full content between `<tool>` and `</tool>` tags,
/// e.g. `b"ps"`, `b"files 1234"`, or `b"kprobe attach do_sys_openat2"`.
pub(crate) fn execute_tool(raw: &[u8]) -> KVVec<u8> {
    let (name, args) = split_tool_args(raw);
    let mut output = KVVec::new();

    match name {
        b"ps" => tool_ps(&mut output),
        b"mem" => tool_mem(&mut output),
        b"loadavg" => tool_loadavg(&mut output),
        b"dmesg" => tool_dmesg(&mut output, args),
        b"files" => tool_files(&mut output, args),
        b"kprobe" => tool_kprobe(&mut output, args),
        b"trace" => tool_trace(&mut output, args),
        _ => {
            let _ = output.extend_from_slice(b"[Error: unknown tool '", GFP_KERNEL);
            let _ = output.extend_from_slice(name, GFP_KERNEL);
            let _ = output.extend_from_slice(
                b"'. Available tools: ps, mem, loadavg, dmesg, files, kprobe, trace]\n",
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

// ---------------------------------------------------------------------------
// Tier 0: dmesg — kernel log ring buffer
// ---------------------------------------------------------------------------

/// Parse a decimal integer from ASCII bytes. Returns 0 on empty/invalid input.
fn parse_usize(s: &[u8]) -> usize {
    let mut n: usize = 0;
    for &b in s {
        if b.is_ascii_digit() {
            n = n.saturating_mul(10).saturating_add((b - b'0') as usize);
        } else {
            break;
        }
    }
    n
}

/// Tool: `dmesg [N]` — read recent kernel log messages from our console ring buffer.
/// Optional argument N limits output to the last N lines.
fn tool_dmesg(output: &mut KVVec<u8>, args: &[u8]) {
    // Read from console ring buffer (C helper).
    let max_read: usize = MAX_TOOL_OUTPUT - 256; // leave room for header
    let mut buf = KVVec::new();
    if buf.resize(max_read, 0, GFP_KERNEL).is_err() {
        let _ = output.extend_from_slice(b"[Error: failed to allocate dmesg buffer]\n", GFP_KERNEL);
        return;
    }

    let n = unsafe {
        crate::types::hackbot_console_read(buf.as_mut_ptr(), max_read as i32)
    };

    if n <= 0 {
        let _ = output.extend_from_slice(b"[No kernel messages captured yet]\n", GFP_KERNEL);
        return;
    }

    let data = &buf[..n as usize];

    // If a line count was given, extract last N lines.
    let max_lines = if !args.is_empty() { parse_usize(args) } else { 0 };

    let display = if max_lines > 0 {
        // Scan backward for newlines to find the start of the last N lines.
        let mut newline_count = 0usize;
        let mut start = data.len();
        for i in (0..data.len()).rev() {
            if data[i] == b'\n' {
                newline_count += 1;
                if newline_count > max_lines {
                    start = i + 1;
                    break;
                }
            }
        }
        &data[start..]
    } else {
        data
    };

    let _ = output.extend_from_slice(b"=== Kernel Log", GFP_KERNEL);
    if max_lines > 0 {
        let mut num = [0u8; 20];
        let _ = output.extend_from_slice(b" (last ", GFP_KERNEL);
        let _ = output.extend_from_slice(format_usize(max_lines, &mut num), GFP_KERNEL);
        let _ = output.extend_from_slice(b" lines)", GFP_KERNEL);
    }
    let _ = output.extend_from_slice(b" ===\n", GFP_KERNEL);
    let _ = output.extend_from_slice(display, GFP_KERNEL);
}

// ---------------------------------------------------------------------------
// Tier 0: files — list open file descriptors for a process
// ---------------------------------------------------------------------------

/// Tool: `files <pid>` — list open file descriptors for a process.
fn tool_files(output: &mut KVVec<u8>, args: &[u8]) {
    if args.is_empty() || !args[0].is_ascii_digit() {
        let _ = output.extend_from_slice(
            b"Usage: <tool>files PID</tool>\nExample: <tool>files 1</tool>\n\
              Lists all open file descriptors for the given process.\n",
            GFP_KERNEL,
        );
        return;
    }

    let pid = parse_usize(args) as i32;

    let buf_size: usize = MAX_TOOL_OUTPUT - 256;
    let mut buf = KVVec::new();
    if buf.resize(buf_size, 0, GFP_KERNEL).is_err() {
        let _ = output.extend_from_slice(b"[Error: failed to allocate files buffer]\n", GFP_KERNEL);
        return;
    }

    let n = unsafe {
        crate::types::hackbot_list_fds(pid, buf.as_mut_ptr(), buf_size as i32)
    };

    if n < 0 {
        let _ = output.extend_from_slice(b"[Error: ", GFP_KERNEL);
        match n {
            -3 => { // ESRCH
                let _ = output.extend_from_slice(b"no process with PID ", GFP_KERNEL);
                let mut num = [0u8; 20];
                let _ = output.extend_from_slice(
                    format_usize(pid as usize, &mut num), GFP_KERNEL,
                );
            }
            _ => {
                let _ = output.extend_from_slice(b"errno ", GFP_KERNEL);
                let mut num = [0u8; 20];
                let _ = output.extend_from_slice(
                    format_usize((-n) as usize, &mut num), GFP_KERNEL,
                );
            }
        }
        let _ = output.extend_from_slice(b"]\n", GFP_KERNEL);
        return;
    }

    let _ = output.extend_from_slice(b"=== Open Files for PID ", GFP_KERNEL);
    let mut num = [0u8; 20];
    let _ = output.extend_from_slice(format_usize(pid as usize, &mut num), GFP_KERNEL);
    let _ = output.extend_from_slice(b" ===\n", GFP_KERNEL);
    let _ = output.extend_from_slice(&buf[..n as usize], GFP_KERNEL);
}

// ---------------------------------------------------------------------------
// Tier 1: kprobe — kernel function instrumentation
// ---------------------------------------------------------------------------

/// Tool: `kprobe <subcommand> [args]`
///   - `kprobe attach <func>` — attach a kprobe to a kernel function
///   - `kprobe check`         — show all active kprobes with hit counts
///   - `kprobe detach <func>` — remove a kprobe from a function
fn tool_kprobe(output: &mut KVVec<u8>, args: &[u8]) {
    let (subcmd, subargs) = split_tool_args(args);

    match subcmd {
        b"attach" => {
            if subargs.is_empty() {
                let _ = output.extend_from_slice(
                    b"Usage: <tool>kprobe attach FUNCTION</tool>\n\
                      Example: <tool>kprobe attach do_sys_openat2</tool>\n",
                    GFP_KERNEL,
                );
                return;
            }
            let ret = unsafe {
                crate::types::hackbot_kprobe_attach(subargs.as_ptr(), subargs.len() as i32)
            };
            if ret < 0 {
                let _ = output.extend_from_slice(b"[Error: failed to attach kprobe '", GFP_KERNEL);
                let _ = output.extend_from_slice(subargs, GFP_KERNEL);
                let _ = output.extend_from_slice(b"': ", GFP_KERNEL);
                match ret {
                    -28 => { let _ = output.extend_from_slice(b"all kprobe slots full (max 8), detach one first", GFP_KERNEL); } // ENOSPC
                    -17 => { let _ = output.extend_from_slice(b"kprobe already attached to this function", GFP_KERNEL); } // EEXIST
                    -2  => { let _ = output.extend_from_slice(b"function not found in kernel", GFP_KERNEL); } // ENOENT
                    -22 => { let _ = output.extend_from_slice(b"function cannot be probed (blacklisted)", GFP_KERNEL); } // EINVAL
                    _ => {
                        let _ = output.extend_from_slice(b"errno ", GFP_KERNEL);
                        let mut num = [0u8; 20];
                        let _ = output.extend_from_slice(
                            format_usize((-ret) as usize, &mut num), GFP_KERNEL,
                        );
                    }
                }
                let _ = output.extend_from_slice(b"]\n", GFP_KERNEL);
            } else {
                let _ = output.extend_from_slice(b"Kprobe attached to '", GFP_KERNEL);
                let _ = output.extend_from_slice(subargs, GFP_KERNEL);
                let _ = output.extend_from_slice(b"' successfully. Use <tool>kprobe check</tool> to see hit counts.\n", GFP_KERNEL);
            }
        }
        b"check" => {
            let buf_size: usize = MAX_TOOL_OUTPUT - 256;
            let mut buf = KVVec::new();
            if buf.resize(buf_size, 0, GFP_KERNEL).is_err() {
                let _ = output.extend_from_slice(b"[Error: alloc failed]\n", GFP_KERNEL);
                return;
            }
            let n = unsafe {
                crate::types::hackbot_kprobe_check(buf.as_mut_ptr(), buf_size as i32)
            };
            let _ = output.extend_from_slice(b"=== Active Kprobes ===\n", GFP_KERNEL);
            if n <= 0 {
                let _ = output.extend_from_slice(b"[No active kprobes]\n", GFP_KERNEL);
            } else {
                let _ = output.extend_from_slice(&buf[..n as usize], GFP_KERNEL);
            }
        }
        b"detach" => {
            if subargs.is_empty() {
                let _ = output.extend_from_slice(
                    b"Usage: <tool>kprobe detach FUNCTION</tool>\n\
                      Example: <tool>kprobe detach do_sys_openat2</tool>\n",
                    GFP_KERNEL,
                );
                return;
            }
            let ret = unsafe {
                crate::types::hackbot_kprobe_detach(subargs.as_ptr(), subargs.len() as i32)
            };
            if ret < 0 {
                let _ = output.extend_from_slice(b"[Error: no active kprobe on '", GFP_KERNEL);
                let _ = output.extend_from_slice(subargs, GFP_KERNEL);
                let _ = output.extend_from_slice(b"']\n", GFP_KERNEL);
            } else {
                let _ = output.extend_from_slice(b"Kprobe detached from '", GFP_KERNEL);
                let _ = output.extend_from_slice(subargs, GFP_KERNEL);
                let _ = output.extend_from_slice(b"'.\n", GFP_KERNEL);
            }
        }
        _ => {
            let _ = output.extend_from_slice(
                b"Usage: kprobe <subcommand>\n\
                  Subcommands:\n\
                  \x20 <tool>kprobe attach FUNC</tool>  - attach kprobe to kernel function\n\
                  \x20 <tool>kprobe check</tool>         - show active kprobes and hit counts\n\
                  \x20 <tool>kprobe detach FUNC</tool>  - remove kprobe from function\n\n\
                  Example: <tool>kprobe attach do_sys_openat2</tool>\n",
                GFP_KERNEL,
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Tier 0: trace — continuous kernel tracepoint sensing
// ---------------------------------------------------------------------------

/// Tool: `trace <subsystem> [raw N]` — read from always-on tracepoint sensors.
///
/// Subsystems: `sched`, `syscall`, `io`
/// Options:
///   - `trace sched`        — aggregate summary + features + notable events
///   - `trace sched raw 20` — last 20 raw sched_switch events
///   - `trace syscall`      — syscall aggregate + features
///   - `trace io`           — I/O stats + latency histogram + LinnOS features
///   - `trace reset`        — zero "since last reset" counters
///   - `trace list`         — show active tracepoints
fn tool_trace(output: &mut KVVec<u8>, args: &[u8]) {
    let (subcmd, subargs) = split_tool_args(args);

    let buf_size: usize = MAX_TOOL_OUTPUT - 256;
    let mut buf = KVVec::new();
    if buf.resize(buf_size, 0, GFP_KERNEL).is_err() {
        let _ = output.extend_from_slice(b"[Error: alloc failed]\n", GFP_KERNEL);
        return;
    }

    match subcmd {
        b"sched" => {
            let (maybe_raw, count_str) = split_tool_args(subargs);
            let n = if maybe_raw == b"raw" {
                let count = if !count_str.is_empty() { parse_usize(count_str) } else { 20 };
                let n = unsafe {
                    crate::types::hackbot_trace_read_sched_raw(
                        buf.as_mut_ptr(), buf_size as i32, count as i32)
                };
                n
            } else {
                unsafe { crate::types::hackbot_trace_read_sched(buf.as_mut_ptr(), buf_size as i32) }
            };
            if n > 0 {
                let _ = output.extend_from_slice(&buf[..n as usize], GFP_KERNEL);
            } else {
                let _ = output.extend_from_slice(b"[No scheduler trace data]\n", GFP_KERNEL);
            }
        }
        b"syscall" => {
            let (maybe_raw, count_str) = split_tool_args(subargs);
            let n = if maybe_raw == b"raw" {
                let count = if !count_str.is_empty() { parse_usize(count_str) } else { 20 };
                unsafe {
                    crate::types::hackbot_trace_read_syscall_raw(
                        buf.as_mut_ptr(), buf_size as i32, count as i32)
                }
            } else {
                unsafe { crate::types::hackbot_trace_read_syscall(buf.as_mut_ptr(), buf_size as i32) }
            };
            if n > 0 {
                let _ = output.extend_from_slice(&buf[..n as usize], GFP_KERNEL);
            } else {
                let _ = output.extend_from_slice(b"[No syscall trace data]\n", GFP_KERNEL);
            }
        }
        b"io" => {
            let (maybe_raw, count_str) = split_tool_args(subargs);
            let n = if maybe_raw == b"raw" {
                let count = if !count_str.is_empty() { parse_usize(count_str) } else { 20 };
                unsafe {
                    crate::types::hackbot_trace_read_io_raw(
                        buf.as_mut_ptr(), buf_size as i32, count as i32)
                }
            } else {
                unsafe { crate::types::hackbot_trace_read_io(buf.as_mut_ptr(), buf_size as i32) }
            };
            if n > 0 {
                let _ = output.extend_from_slice(&buf[..n as usize], GFP_KERNEL);
            } else {
                let _ = output.extend_from_slice(b"[No I/O trace data]\n", GFP_KERNEL);
            }
        }
        b"tokens" => {
            let (count_str, _) = split_tool_args(subargs);
            let count = if !count_str.is_empty() { parse_usize(count_str) } else { 20 };
            let n = unsafe {
                crate::types::hackbot_trace_read_tokens(
                    buf.as_mut_ptr(), buf_size as i32, count as i32)
            };
            if n > 0 {
                let _ = output.extend_from_slice(&buf[..n as usize], GFP_KERNEL);
            } else {
                let _ = output.extend_from_slice(b"[No token data]\n", GFP_KERNEL);
            }
        }
        b"ngram" => {
            let (subcmd2, count_str) = split_tool_args(subargs);
            let n = if subcmd2 == b"stats" {
                unsafe {
                    crate::types::hackbot_trace_read_ngram_stats(
                        buf.as_mut_ptr(), buf_size as i32)
                }
            } else if subcmd2 == b"alerts" {
                let count = if !count_str.is_empty() { parse_usize(count_str) } else { 10 };
                unsafe {
                    crate::types::hackbot_trace_read_ngram_alerts(
                        buf.as_mut_ptr(), buf_size as i32, count as i32)
                }
            } else {
                /* Default: show surprise scores */
                unsafe {
                    crate::types::hackbot_trace_read_ngram_surprise(
                        buf.as_mut_ptr(), buf_size as i32)
                }
            };
            if n > 0 {
                let _ = output.extend_from_slice(&buf[..n as usize], GFP_KERNEL);
            } else {
                let _ = output.extend_from_slice(b"[No n-gram data]\n", GFP_KERNEL);
            }
        }
        b"reset" => {
            unsafe { crate::types::hackbot_trace_reset() };
            let _ = output.extend_from_slice(
                b"Trace counters reset. Tracepoints still active.\n", GFP_KERNEL);
        }
        b"list" => {
            let _ = output.extend_from_slice(
                b"Active tracepoints: sched_switch, sys_enter, block_rq_complete\n\
                  All registered at module load. Always-on continuous sensing.\n",
                GFP_KERNEL);
        }
        _ => {
            let _ = output.extend_from_slice(
                b"Usage: trace <subsystem> [raw N]\n\
                  Subsystems:\n\
                  \x20 <tool>trace sched</tool>        - scheduler context switches\n\
                  \x20 <tool>trace sched raw 20</tool> - last 20 raw events\n\
                  \x20 <tool>trace syscall</tool>      - syscall patterns\n\
                  \x20 <tool>trace io</tool>           - I/O latency + histogram\n\
                  \x20 <tool>trace tokens 20</tool>    - last 20 semantic tokens\n\
                  \x20 <tool>trace ngram</tool>        - n-gram surprise scores\n\
                  \x20 <tool>trace ngram stats</tool>  - n-gram model statistics\n\
                  \x20 <tool>trace ngram alerts</tool> - recent anomaly alerts\n\
                  \x20 <tool>trace reset</tool>        - zero counters\n\
                  \x20 <tool>trace list</tool>         - active tracepoints\n",
                GFP_KERNEL,
            );
        }
    }
}
