// SPDX-License-Identifier: GPL-2.0

//! Kernel context gathering — gives the LLM "eyes" into the real system.

use core::mem::MaybeUninit;
use core::ptr;

use kernel::{bindings, prelude::*};

use crate::net::format_usize;
use crate::types::kernel_version;

/// Gather live kernel context and return it as a formatted text block.
pub(crate) fn gather_kernel_context() -> KVVec<u8> {
    let mut ctx = KVVec::new();

    let _ = ctx.extend_from_slice(b"=== LIVE KERNEL STATE ===\n", GFP_KERNEL);

    let _ = ctx.extend_from_slice(b"Kernel: ", GFP_KERNEL);
    append_kernel_version(&mut ctx);
    let _ = ctx.push(b'\n', GFP_KERNEL);

    let _ = ctx.extend_from_slice(b"Uptime: ", GFP_KERNEL);
    append_uptime(&mut ctx);
    let _ = ctx.push(b'\n', GFP_KERNEL);

    let _ = ctx.extend_from_slice(b"CPUs: ", GFP_KERNEL);
    let cpus = read_num_online_cpus();
    let mut buf = [0u8; 20];
    let s = format_usize(cpus, &mut buf);
    let _ = ctx.extend_from_slice(s, GFP_KERNEL);
    let _ = ctx.extend_from_slice(b" online\n", GFP_KERNEL);

    append_memory_info(&mut ctx);

    let _ = ctx.extend_from_slice(b"Caller: ", GFP_KERNEL);
    append_current_task_info(&mut ctx);
    let _ = ctx.push(b'\n', GFP_KERNEL);

    let _ = ctx.extend_from_slice(b"=========================\n\n", GFP_KERNEL);

    ctx
}

fn append_kernel_version(buf: &mut KVVec<u8>) {
    let _ = buf.extend_from_slice(b"Linux ", GFP_KERNEL);
    let _ = buf.extend_from_slice(kernel_version::KERNEL_RELEASE, GFP_KERNEL);
    let _ = buf.extend_from_slice(b" x86_64", GFP_KERNEL);
}

pub(crate) fn append_uptime(buf: &mut KVVec<u8>) {
    // SAFETY: ktime_get_boot_fast_ns() is always safe to call.
    let ns = unsafe { bindings::ktime_get_boot_fast_ns() };
    let total_secs = (ns / 1_000_000_000) as usize;

    let days = total_secs / 86400;
    let hours = (total_secs % 86400) / 3600;
    let mins = (total_secs % 3600) / 60;
    let secs = total_secs % 60;

    let mut num = [0u8; 20];

    if days > 0 {
        let s = format_usize(days, &mut num);
        let _ = buf.extend_from_slice(s, GFP_KERNEL);
        let _ = buf.extend_from_slice(b"d ", GFP_KERNEL);
    }
    let s = format_usize(hours, &mut num);
    let _ = buf.extend_from_slice(s, GFP_KERNEL);
    let _ = buf.extend_from_slice(b"h ", GFP_KERNEL);

    let s = format_usize(mins, &mut num);
    let _ = buf.extend_from_slice(s, GFP_KERNEL);
    let _ = buf.extend_from_slice(b"m ", GFP_KERNEL);

    let s = format_usize(secs, &mut num);
    let _ = buf.extend_from_slice(s, GFP_KERNEL);
    let _ = buf.extend_from_slice(b"s", GFP_KERNEL);
}

pub(crate) fn read_num_online_cpus() -> usize {
    // SAFETY: __num_online_cpus is a global atomic_t.
    unsafe {
        let counter_ptr = ptr::addr_of!((*ptr::addr_of!(bindings::__num_online_cpus)).counter);
        counter_ptr.read_volatile() as usize
    }
}

fn append_memory_info(ctx: &mut KVVec<u8>) {
    // SAFETY: si_meminfo() fills a stack-allocated sysinfo struct.
    let mut info: bindings::sysinfo = unsafe { MaybeUninit::zeroed().assume_init() };
    unsafe { bindings::si_meminfo(&mut info) };

    let unit = info.mem_unit as usize;
    let total_bytes = info.totalram as usize * unit;
    let free_bytes = info.freeram as usize * unit;
    let used_bytes = total_bytes.saturating_sub(free_bytes);

    let _ = ctx.extend_from_slice(b"Memory: ", GFP_KERNEL);

    let used_mb = used_bytes / (1024 * 1024);
    let total_mb = total_bytes / (1024 * 1024);

    let mut num = [0u8; 20];
    let s = format_usize(used_mb, &mut num);
    let _ = ctx.extend_from_slice(s, GFP_KERNEL);
    let _ = ctx.extend_from_slice(b" MB used / ", GFP_KERNEL);

    let s = format_usize(total_mb, &mut num);
    let _ = ctx.extend_from_slice(s, GFP_KERNEL);
    let _ = ctx.extend_from_slice(b" MB total\n", GFP_KERNEL);
}

fn append_current_task_info(buf: &mut KVVec<u8>) {
    // SAFETY: Task::current_raw() returns the current task pointer.
    let task_ptr = kernel::task::Task::current_raw();

    let pid = unsafe { (*task_ptr).pid };
    let comm = unsafe { &(*task_ptr).comm };

    let _ = buf.extend_from_slice(b"pid=", GFP_KERNEL);
    let mut num = [0u8; 20];
    let s = format_usize(pid as usize, &mut num);
    let _ = buf.extend_from_slice(s, GFP_KERNEL);

    let _ = buf.extend_from_slice(b" (", GFP_KERNEL);
    for &c in comm.iter() {
        if c == 0 {
            break;
        }
        let _ = buf.push(c as u8, GFP_KERNEL);
    }
    let _ = buf.extend_from_slice(b")", GFP_KERNEL);
}
