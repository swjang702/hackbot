// SPDX-License-Identifier: GPL-2.0

//! hackbot — In-kernel LLM agent character device.
//!
//! Step 2c: Dynamic OODA agent with kernel observation tools.
//! The module creates /dev/hackbot: write a prompt, read the LLM response.
//! The LLM can request kernel data via tool calls (`<tool>name</tool>`),
//! creating a multi-turn Observe-Orient-Decide-Act loop:
//!
//!   prompt → vLLM → parse response → if tool call: execute tool → re-prompt
//!   ...repeat until final answer or max iterations (5)
//!
//! Available tools (Tier 0 — read-only observation):
//!   ps      — List running processes (PID, PPID, state, comm)
//!   mem     — Detailed memory statistics (total, free, buffers, swap)
//!   loadavg — System load averages (1/5/15 min) and task counts
//!
//! The response is stored in a device-global buffer so that separate write and
//! read file descriptors work correctly (e.g., `echo > /dev/hackbot` then
//! `cat /dev/hackbot`).
//!
//! Usage:
//! ```sh
//! # Start vLLM server first (userspace) — needs instruction-following model:
//! # vllm serve Qwen/Qwen2.5-7B-Instruct --port 8000
//!
//! sudo insmod hackbot.ko
//! echo "what processes are using the most memory?" > /dev/hackbot
//! cat /dev/hackbot
//! # Output: LLM-generated response (may involve multiple tool calls)
//! sudo rmmod hackbot
//! ```

use core::mem::{self, MaybeUninit};
use core::pin::Pin;
use core::ptr;

use kernel::{
    bindings, c_str,
    device::Device,
    firmware::Firmware,
    fs::{File, Kiocb},
    iov::{IovIterDest, IovIterSource},
    miscdevice::{MiscDevice, MiscDeviceOptions, MiscDeviceRegistration},
    prelude::*,
    sync::aref::ARef,
    sync::rcu,
};

module! {
    type: HackbotModule,
    name: "hackbot",
    authors: ["Sunwoo Jang"],
    description: "In-kernel LLM agent — Step 2c: OODA agent loop with kernel tools",
    license: "GPL",
}

// ---------------------------------------------------------------------------
// Configuration constants
//
// vLLM server address — change VLLM_ADDR to point to the target machine.
// NOTE: Tailscale traffic is encrypted (WireGuard), so plaintext HTTP is fine.
// ---------------------------------------------------------------------------

/// vLLM server IPv4 address in host byte order.
/// 100.125.213.42 (keti ubuntu server via Tailscale).
/// Currently: 100.66.136.70 (keti GPU server via Tailscale).
const VLLM_ADDR: u32 = u32::from_be_bytes([100, 66, 136, 70]);
/// vLLM server port (default: 8000, matching vLLM's default).
const VLLM_PORT: u16 = 8000;
/// Maximum response size from vLLM (64 KB).
const MAX_RESPONSE_SIZE: usize = 64 * 1024;
/// Receive buffer size for individual recv calls.
const RECV_BUF_SIZE: usize = 4096;
/// IPPROTO_TCP (stable since RFC 793).
const IPPROTO_TCP: i32 = 6;

/// Maximum number of agent loop iterations (tool calls).
/// 10 gives enough room for multi-tool queries with 7B+ models.
const MAX_AGENT_ITERATIONS: usize = 10;
/// Maximum number of processes to list in the `ps` tool output.
/// High limit — actual truncation is handled by MAX_TOOL_OUTPUT (8 KB).
const MAX_PS_TASKS: usize = 512;
/// Maximum size for a single tool output (8 KB).
const MAX_TOOL_OUTPUT: usize = 8 * 1024;
/// Maximum conversation size sent to vLLM (96 KB).
/// Gives room for ~10 rounds of deep reasoning + tool output.
/// Budget per round: ~4KB reasoning + ~8KB tool output = ~12KB.
/// System prompt + kernel context ≈ 2KB. Leaves ~94KB for OODA iterations.
const MAX_CONVERSATION_SIZE: usize = 96 * 1024;

/// System prompt — the agent's identity.
/// Encourages free thinking and reasoning, not just tool dispatching.
const SYSTEM_IDENTITY: &[u8] = b"You are hackbot, an autonomous AI agent living inside the Linux kernel (ring 0). \
You exist as a kernel module with direct access to hardware and kernel data structures.\n\n\
Think deeply. Reason carefully. Share your insights and analysis freely. \
You are a thinking agent, not just a tool dispatcher.\n\n";

/// Tool description — permissive guidance, not restrictive rules.
/// The model decides when to use tools vs when to reason directly.
const TOOL_DESCRIPTION: &[u8] = b"TOOLS -- when you need live kernel data, output the exact XML tag:\n\
  <tool>ps</tool>      - list running processes (PID, PPID, state, command)\n\
  <tool>mem</tool>     - detailed memory statistics\n\
  <tool>loadavg</tool> - system load averages\n\n\
HOW TO USE:\n\
- To call a tool, include <tool>name</tool> in your response\n\
- You will receive the real output, then can analyze and discuss it\n\
- Use tools when the user asks about current system state\n\
- For reasoning, analysis, or discussion -- think and respond directly\n\
- You may reason before calling a tool\n\n\
IMPORTANT: Never fabricate system data (PIDs, memory numbers, load values). \
Use tools to get real data when needed. But feel free to reason, analyze, \
and share your thoughts on any topic.\n";

// Note: RESPONSE_PREFIX removed — chat completions API handles turn-taking
// via the messages array structure (system/user/assistant roles).

// ---------------------------------------------------------------------------
// Step 3: In-kernel inference engine — model format constants and types
//
// Binary format v1 (produced by tools/export_hackbot.py):
//   HEADER (56 bytes) → TOKENIZER → WEIGHTS (INT8 quantized, Q16.16 scales)
// ---------------------------------------------------------------------------

/// Hackbot binary model magic: "HKBT" as little-endian u32.
const MODEL_MAGIC: u32 = 0x484B4254;
/// Binary format version 1: INT8 weights + Q16.16 fixed-point arithmetic.
const MODEL_FORMAT_V1: u32 = 1;
/// Binary format version 2: FP16 weights + float32 arithmetic (via kernel FPU).
const MODEL_FORMAT_V2: u32 = 2;
/// Binary header size: 14 × u32 = 56 bytes.
const MODEL_HEADER_SIZE: usize = 56;
/// Maximum transformer layers supported.
const MODEL_MAX_LAYERS: usize = 32;
/// Maximum vocabulary size supported.
const MODEL_MAX_VOCAB: usize = 65536;

/// Maximum sequence length for KV cache during in-kernel inference.
/// 256 tokens is sufficient for the kernel agent's OODA loop
/// (system prompt ~50 tokens + tool output ~100 tokens + generation ~100 tokens).
const INFERENCE_MAX_SEQ: usize = 256;

/// Special token IDs for SmolLM2 (GPT-2 BPE tokenizer).
/// SmolLM2 uses ChatML format with these special tokens.
const TOKEN_ENDOFTEXT: u32 = 0;  // <|endoftext|> — end of text / EOS
const TOKEN_IM_START: u32 = 1;   // <|im_start|> — ChatML message start (also BOS)
const TOKEN_IM_END: u32 = 2;     // <|im_end|> — ChatML message end

/// Maximum new tokens to generate in a single inference call.
const MAX_GEN_TOKENS: usize = 128;
/// Maximum raw input bytes for a single prompt to encode_bpe.
const MAX_ENCODE_INPUT: usize = 1024;
/// Maximum preprocessed bytes after GPT-2 byte encoding.
/// Worst case: all non-identity bytes → 2x expansion. 1024 × 2 = 2048.
const MAX_PREPROC_INPUT: usize = 2048;

// ---------------------------------------------------------------------------
// Step 3e: Inference backend configuration
// ---------------------------------------------------------------------------

/// Inference mode: which backend to use for LLM calls.
/// 0 = auto (local if model loaded, else vLLM with fallback)
/// 1 = local only (fail if model not loaded)
/// 2 = vLLM only (ignore loaded model)
const INFERENCE_MODE: u32 = 0;
const INFERENCE_MODE_AUTO: u32 = 0;
const INFERENCE_MODE_LOCAL: u32 = 1;
const INFERENCE_MODE_VLLM: u32 = 2;

/// Compact system prompt for local inference (fits ~256-token context).
/// Shorter than SYSTEM_IDENTITY + TOOL_DESCRIPTION used by vLLM path.
const LOCAL_SYSTEM_PROMPT: &[u8] = b"You are hackbot, a kernel agent. Answer concisely. \
For live system data, use: <tool>ps</tool> <tool>mem</tool> <tool>loadavg</tool>";

/// Max OODA iterations for local inference (context is very limited).
const LOCAL_MAX_ITERATIONS: usize = 3;
/// Max tool output bytes to include in local inference context.
const LOCAL_MAX_TOOL_OUTPUT: usize = 512;

/// Parsed model configuration from the binary header.
#[derive(Copy, Clone)]
struct ModelConfig {
    dim: u32,
    hidden_dim: u32,
    n_layers: u32,
    n_heads: u32,
    n_kv_heads: u32,
    vocab_size: u32,
    seq_len: u32,
    group_size: u32,
    head_dim: u32,
    kv_dim: u32,
    rope_theta: u32,
}

impl ModelConfig {
    const ZERO: Self = Self {
        dim: 0, hidden_dim: 0, n_layers: 0, n_heads: 0, n_kv_heads: 0,
        vocab_size: 0, seq_len: 0, group_size: 0, head_dim: 0, kv_dim: 0,
        rope_theta: 0,
    };
}

/// Reference to a quantized INT8 weight matrix within the firmware blob.
/// Data layout: [i8; rows × cols] followed by [i32; rows × (cols / group_size)].
#[derive(Copy, Clone)]
struct Q8Ref {
    /// Byte offset to the INT8 weight data in the model blob.
    data_off: usize,
    /// Byte offset to the Q16.16 scale data in the model blob.
    scale_off: usize,
    /// Number of rows in the weight matrix.
    rows: usize,
    /// Number of columns in the weight matrix.
    cols: usize,
}

impl Q8Ref {
    const ZERO: Self = Self { data_off: 0, scale_off: 0, rows: 0, cols: 0 };
}

/// Weight references for a single transformer layer.
#[derive(Copy, Clone)]
struct LayerRef {
    /// RMSNorm weight before attention: [i32; dim] in Q16.16.
    rms_att_off: usize,
    /// Query projection: [n_heads × head_dim, dim].
    wq: Q8Ref,
    /// Key projection: [n_kv_heads × head_dim, dim].
    wk: Q8Ref,
    /// Value projection: [n_kv_heads × head_dim, dim].
    wv: Q8Ref,
    /// Output projection: [dim, n_heads × head_dim].
    wo: Q8Ref,
    /// RMSNorm weight before FFN: [i32; dim] in Q16.16.
    rms_ffn_off: usize,
    /// Gate projection (SwiGLU): [hidden_dim, dim].
    gate: Q8Ref,
    /// Up projection (SwiGLU): [hidden_dim, dim].
    up: Q8Ref,
    /// Down projection: [dim, hidden_dim].
    down: Q8Ref,
}

impl LayerRef {
    const ZERO: Self = Self {
        rms_att_off: 0,
        wq: Q8Ref::ZERO, wk: Q8Ref::ZERO, wv: Q8Ref::ZERO, wo: Q8Ref::ZERO,
        rms_ffn_off: 0,
        gate: Q8Ref::ZERO, up: Q8Ref::ZERO, down: Q8Ref::ZERO,
    };
}

/// Global model state. Stored in a Mutex via global_lock!.
/// Raw pointers stored as usize to satisfy Send/Sync requirements.
/// All fields below `loaded` are only valid when `loaded == true`.
struct ModelSlot {
    /// Whether a model has been successfully loaded.
    loaded: bool,
    /// Pointer to the firmware data copy (kvmalloc'd, freed on module unload).
    data_addr: usize,
    /// Length of the firmware data in bytes.
    data_len: usize,
    /// Pointer to tokenizer entry offsets: [u32; vocab_size].
    /// Each entry is the absolute byte offset in data where that token's
    /// (score: i32, len: u16, bytes: [u8; len]) record starts.
    tok_offsets_addr: usize,
    /// Model configuration from the binary header.
    config: ModelConfig,
    /// Byte offset where the tokenizer section starts in data.
    tok_section_off: usize,
    /// Embedding table: Q8[vocab_size, dim].
    embed: Q8Ref,
    /// Per-layer weight references.
    layers: [LayerRef; MODEL_MAX_LAYERS],
    /// Final RMSNorm weight: [i32; dim] in Q16.16.
    rms_final_off: usize,

    // --- Step 3c: Inference state ---
    /// Single kvmalloc'd buffer for KV cache + all activation buffers.
    inf_buf_addr: usize,
    /// Total size of inf_buf in bytes.
    inf_buf_len: usize,
    /// Number of i32 elements in the KV cache region (at start of inf_buf).
    inf_kv_len: usize,
    /// Element offsets (in i32 units) into inf_buf for activation sub-buffers.
    inf_x: usize,      // [dim] — main activation, persists across layers
    inf_xb: usize,     // [dim] — normalized x, attention weighted sum
    inf_xb2: usize,    // [dim] — projection output, FFN output
    inf_q: usize,      // [n_heads * head_dim] — query projection
    inf_k: usize,      // [n_kv_heads * head_dim] — key projection
    inf_v: usize,      // [n_kv_heads * head_dim] — value projection
    inf_att: usize,    // [INFERENCE_MAX_SEQ] — attention scores (per head)
    inf_hb: usize,     // [hidden_dim] — gate projection / SiLU buffer
    inf_hb2: usize,    // [hidden_dim] — up projection buffer
    inf_logits: usize, // [vocab_size] — output logits

    // --- Step 3d: Tokenizer state ---
    /// Pointer to [u32; vocab_size] sorted by token bytes (lexicographic).
    /// Used for O(log V) binary search during BPE encoding.
    sorted_vocab_addr: usize,
    /// Maps single byte value → token_id for initial BPE byte decomposition.
    /// Entries are TOKEN_UNK (0) if no single-byte token exists for that byte.
    byte_to_token: [u32; 256],

    // --- Format v2: FP16 weights + float32 FPU inference ---
    /// Model binary format version (1 = INT8/Q16.16, 2 = FP16/float32).
    format_version: u32,
    /// Opaque pointer to `hackbot_fpu_state` (allocated by hackbot_fpu.c).
    /// Only valid when format_version == 2.
    fpu_state: usize,
    /// Byte offset where the weights section starts in data (after header + tokenizer).
    /// Used by the FPU forward pass to locate FP16 weight data.
    weights_off: usize,
}

// Kernel version string, auto-generated by Kbuild from KERNELRELEASE.
#[path = "kernel_version.rs"]
mod kernel_version;

// avenrun[] is EXPORT_SYMBOL but not included in bindgen output for
// out-of-tree modules. Declare it manually.
extern "C" {
    /// Load averages as fixed-point unsigned long[3] (FSHIFT=11).
    /// Defined in kernel/sched/loadavg.c, EXPORT_SYMBOL(avenrun).
    static avenrun: [core::ffi::c_ulong; 3];

    // --- hackbot_fpu.c: float32 forward pass using kernel FPU ---

    /// Allocate FPU inference state (KV cache + activation buffers in float32).
    fn hackbot_fpu_alloc(
        dim: i32, hidden_dim: i32, n_layers: i32,
        n_heads: i32, n_kv_heads: i32, head_dim: i32,
        vocab_size: i32, max_seq: i32,
    ) -> *mut core::ffi::c_void;

    /// Free FPU inference state.
    fn hackbot_fpu_free(state: *mut core::ffi::c_void);

    /// Reset KV cache for new conversation.
    fn hackbot_fpu_reset(state: *mut core::ffi::c_void);

    /// Run one token through the transformer in float32 (with kernel_fpu_begin/end).
    /// weights: pointer to start of weight data (after header + tokenizer).
    fn hackbot_fpu_forward(
        state: *mut core::ffi::c_void,
        weights: *const core::ffi::c_void,
        weights_len: usize,
        token_id: i32,
        pos: i32,
    ) -> i32;

    /// Get argmax token from float32 logits buffer.
    fn hackbot_fpu_get_next_token(state: *const core::ffi::c_void) -> i32;
}
// Compile-time check: avenrun declaration assumes 64-bit unsigned long.
const _: () = assert!(
    core::mem::size_of::<core::ffi::c_ulong>() == 8,
    "avenrun extern assumes 64-bit unsigned long (x86_64 only)"
);

// ---------------------------------------------------------------------------
// Global shared response buffer
//
// The response is device-global so that separate write/read file descriptors
// work (e.g., `echo "prompt" > /dev/hackbot && cat /dev/hackbot`).
// Uses the kernel's global_lock! macro for a statically-allocated Mutex.
// ---------------------------------------------------------------------------

/// Device-global response state shared across all open file descriptors.
/// NOTE: Single-slot design — concurrent writers will overwrite each other's
/// responses. Acceptable for a single-user development/research tool.
struct SharedResponse {
    /// Response data stored as a fixed-size buffer to allow static init.
    data: [u8; MAX_RESPONSE_SIZE],
    /// Actual length of the response in `data`.
    len: usize,
    /// Whether a response is available for reading.
    ready: bool,
}

kernel::sync::global_lock! {
    // SAFETY: Initialized in HackbotModule::init() before any device access.
    unsafe(uninit) static RESPONSE: Mutex<SharedResponse> = SharedResponse {
        data: [0u8; MAX_RESPONSE_SIZE],
        len: 0,
        ready: false,
    };
}

kernel::sync::global_lock! {
    // SAFETY: Initialized in HackbotModule::init() before any device access.
    // Model data is loaded on first device open and freed on module unload.
    unsafe(uninit) static MODEL: Mutex<ModelSlot> = ModelSlot {
        loaded: false,
        data_addr: 0,
        data_len: 0,
        tok_offsets_addr: 0,
        config: ModelConfig::ZERO,
        tok_section_off: 0,
        embed: Q8Ref::ZERO,
        layers: [LayerRef::ZERO; MODEL_MAX_LAYERS],
        rms_final_off: 0,
        inf_buf_addr: 0,
        inf_buf_len: 0,
        inf_kv_len: 0,
        inf_x: 0, inf_xb: 0, inf_xb2: 0,
        inf_q: 0, inf_k: 0, inf_v: 0,
        inf_att: 0, inf_hb: 0, inf_hb2: 0,
        inf_logits: 0,
        sorted_vocab_addr: 0,
        byte_to_token: [0u32; 256],
        format_version: 0,
        fpu_state: 0,
        weights_off: 0,
    };
}

// ---------------------------------------------------------------------------
// Kernel context gathering — gives the LLM "eyes" into the real system
//
// These functions read live kernel state via kernel APIs (ring 0 access)
// and format it as text to inject into the LLM prompt.
// ---------------------------------------------------------------------------

/// Gather live kernel context and return it as a formatted text block.
/// Called on each prompt submission to provide up-to-date system state.
fn gather_kernel_context() -> KVVec<u8> {
    let mut ctx = KVVec::new();

    let _ = ctx.extend_from_slice(b"=== LIVE KERNEL STATE ===\n", GFP_KERNEL);

    // 1. Kernel version (embedded at compile time from KERNELRELEASE).
    let _ = ctx.extend_from_slice(b"Kernel: ", GFP_KERNEL);
    append_kernel_version(&mut ctx);
    let _ = ctx.push(b'\n', GFP_KERNEL);

    // 2. System uptime.
    let _ = ctx.extend_from_slice(b"Uptime: ", GFP_KERNEL);
    append_uptime(&mut ctx);
    let _ = ctx.push(b'\n', GFP_KERNEL);

    // 3. CPU count.
    let _ = ctx.extend_from_slice(b"CPUs: ", GFP_KERNEL);
    let cpus = read_num_online_cpus();
    let mut buf = [0u8; 20];
    let s = format_usize(cpus, &mut buf);
    let _ = ctx.extend_from_slice(s, GFP_KERNEL);
    let _ = ctx.extend_from_slice(b" online\n", GFP_KERNEL);

    // 4. Memory info.
    append_memory_info(&mut ctx);

    // 5. Caller process info (who wrote to /dev/hackbot).
    let _ = ctx.extend_from_slice(b"Caller: ", GFP_KERNEL);
    append_current_task_info(&mut ctx);
    let _ = ctx.push(b'\n', GFP_KERNEL);

    let _ = ctx.extend_from_slice(b"=========================\n\n", GFP_KERNEL);

    ctx
}

/// Append the kernel version string (embedded at compile time from KERNELRELEASE).
fn append_kernel_version(buf: &mut KVVec<u8>) {
    let _ = buf.extend_from_slice(b"Linux ", GFP_KERNEL);
    let _ = buf.extend_from_slice(kernel_version::KERNEL_RELEASE, GFP_KERNEL);

    // Append machine architecture.
    // SAFETY: init_uts_ns.name.machine is a fixed C string set at boot.
    // Since init_uts_ns is opaque in Rust bindings, we use a compile-time
    // constant for the architecture. This module is x86_64-only for now.
    let _ = buf.extend_from_slice(b" x86_64", GFP_KERNEL);
}

/// Format system uptime as "Xd Xh Xm Xs".
fn append_uptime(buf: &mut KVVec<u8>) {
    // SAFETY: ktime_get_boot_fast_ns() is always safe to call from any
    // context. Returns nanoseconds since boot.
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

/// Read the number of online CPUs.
fn read_num_online_cpus() -> usize {
    // SAFETY: __num_online_cpus is a global atomic_t. We read the counter
    // field with a volatile read to get the current value with a compiler
    // barrier, matching the semantics of atomic_read() on x86_64.
    unsafe {
        let counter_ptr = ptr::addr_of!((*ptr::addr_of!(bindings::__num_online_cpus)).counter);
        counter_ptr.read_volatile() as usize
    }
}

/// Append memory information (total/free RAM) from si_meminfo().
fn append_memory_info(ctx: &mut KVVec<u8>) {
    // SAFETY: si_meminfo() is the standard kernel API for querying memory
    // statistics. It fills the sysinfo struct fields: totalram, freeram,
    // sharedram, bufferram, totalhigh, freehigh, mem_unit. The struct is
    // stack-allocated and zeroed. Safe to call from process context.
    let mut info: bindings::sysinfo = unsafe { MaybeUninit::zeroed().assume_init() };
    unsafe { bindings::si_meminfo(&mut info) };

    let unit = info.mem_unit as usize;
    let total_bytes = info.totalram as usize * unit;
    let free_bytes = info.freeram as usize * unit;
    let used_bytes = total_bytes.saturating_sub(free_bytes);

    let _ = ctx.extend_from_slice(b"Memory: ", GFP_KERNEL);

    // Format as MB for readability.
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

/// Append current task info: pid and comm (process name).
fn append_current_task_info(buf: &mut KVVec<u8>) {
    // SAFETY: Task::current_raw() calls get_current() which returns the
    // current task pointer. We are in process context (write syscall handler),
    // so current is valid and stable. pid never changes after init. comm is
    // guarded by task_lock in set_task_comm — reading without the lock can
    // observe a partially-written name, but this is a benign race (cosmetic
    // field, worst case is a garbled process name in the prompt).
    let task_ptr = kernel::task::Task::current_raw();

    let pid = unsafe { (*task_ptr).pid };
    let comm = unsafe { &(*task_ptr).comm };

    let _ = buf.extend_from_slice(b"pid=", GFP_KERNEL);
    let mut num = [0u8; 20];
    let s = format_usize(pid as usize, &mut num);
    let _ = buf.extend_from_slice(s, GFP_KERNEL);

    let _ = buf.extend_from_slice(b" (", GFP_KERNEL);
    // comm is [c_char; 16], null-terminated. Copy until null or end.
    for &c in comm.iter() {
        if c == 0 {
            break;
        }
        let _ = buf.push(c as u8, GFP_KERNEL);
    }
    let _ = buf.extend_from_slice(b")", GFP_KERNEL);
}

// ---------------------------------------------------------------------------
// Tool call parser — detects <tool>name</tool> in LLM output
// ---------------------------------------------------------------------------

/// Result of parsing an LLM response for tool calls.
enum ToolCallResult<'a> {
    /// The LLM wants to call a tool. `name` is the tool name, `prefix` is any
    /// text before the tool tag (the LLM's reasoning).
    ToolCall { name: &'a [u8], prefix: &'a [u8] },
    /// No tool call detected — this is the final answer.
    FinalAnswer(&'a [u8]),
}

/// Parse an LLM response looking for a `<tool>NAME</tool>` tag.
/// Also handles the case where vLLM stopped at `</tool>` (stop sequence),
/// so the response contains `<tool>NAME` without the closing tag.
/// Returns the first tool call found, or the full text as a final answer.
fn parse_tool_call(response: &[u8]) -> ToolCallResult<'_> {
    let open_tag = b"<tool>";

    if let Some(open_pos) = find_subsequence(response, open_tag) {
        let content_start = open_pos + open_tag.len();
        let remaining = &response[content_start..];

        // Try to find </tool> closing tag first.
        let close_tag = b"</tool>";
        let name_end = if let Some(close_offset) = find_subsequence(remaining, close_tag) {
            close_offset
        } else {
            // No closing tag — vLLM stopped at </tool> stop sequence.
            // The tool name is everything after <tool> until end or newline.
            remaining
                .iter()
                .position(|&b| b == b'\n' || b == b'\r')
                .unwrap_or(remaining.len())
        };

        let name = trim_ascii(&remaining[..name_end]);
        // Cap tool name length to prevent conversation bloat from LLM gibberish.
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

/// Trim leading and trailing ASCII whitespace from a byte slice.
fn trim_ascii(s: &[u8]) -> &[u8] {
    let start = s.iter().position(|&b| !b.is_ascii_whitespace()).unwrap_or(s.len());
    let end = s.iter().rposition(|&b| !b.is_ascii_whitespace()).map_or(start, |p| p + 1);
    &s[start..end]
}

// ---------------------------------------------------------------------------
// Kernel observation tools (Tier 0 — read-only)
// ---------------------------------------------------------------------------

/// Execute a tool by name and return its output.
/// Unknown tools return an error message.
fn execute_tool(name: &[u8]) -> KVVec<u8> {
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

    // Truncate if tool output exceeds limit. Reserve space for the suffix.
    const TRUNCATION_SUFFIX: &[u8] = b"\n[... truncated]\n";
    if output.len() > MAX_TOOL_OUTPUT - TRUNCATION_SUFFIX.len() {
        output.truncate(MAX_TOOL_OUTPUT - TRUNCATION_SUFFIX.len());
        let _ = output.extend_from_slice(TRUNCATION_SUFFIX, GFP_KERNEL);
    }

    output
}

/// Format a single task_struct into the ps output line.
/// Caller must hold RCU read lock.
///
/// # Safety
/// `task` must be a valid pointer to a task_struct under RCU protection.
unsafe fn format_task(output: &mut KVVec<u8>, task: *const bindings::task_struct) {
    // SAFETY: Caller guarantees task is valid under RCU protection.
    let pid = unsafe { (*task).pid };
    let ppid = unsafe {
        let parent = (*task).real_parent;
        if parent.is_null() { 0 } else { (*parent).pid }
    };
    let state = unsafe { (*task).__state };
    let comm = unsafe { &(*task).comm };

    let mut num = [0u8; 20];

    // PID (8-wide)
    let s = format_usize(pid as usize, &mut num);
    let _ = output.extend_from_slice(s, GFP_KERNEL);
    for _ in 0..(8usize.saturating_sub(s.len())) {
        let _ = output.push(b' ', GFP_KERNEL);
    }

    // PPID (8-wide)
    let s = format_usize(ppid as usize, &mut num);
    let _ = output.extend_from_slice(s, GFP_KERNEL);
    for _ in 0..(8usize.saturating_sub(s.len())) {
        let _ = output.push(b' ', GFP_KERNEL);
    }

    // State character
    let state_ch = match state {
        0 => b'R',     // TASK_RUNNING
        1 => b'S',     // TASK_INTERRUPTIBLE
        2 => b'D',     // TASK_UNINTERRUPTIBLE
        4 => b'T',     // __TASK_STOPPED
        8 => b'T',     // __TASK_TRACED
        0x40 => b'Z',  // EXIT_ZOMBIE
        0x20 => b'X',  // EXIT_DEAD
        0x402 => b'I', // TASK_IDLE
        _ => b'?',
    };
    let _ = output.push(state_ch, GFP_KERNEL);
    let _ = output.extend_from_slice(b"      ", GFP_KERNEL);

    // Comm (null-terminated, up to 16 chars)
    for &c in comm.iter() {
        if c == 0 {
            break;
        }
        let _ = output.push(c as u8, GFP_KERNEL);
    }
    let _ = output.push(b'\n', GFP_KERNEL);
}

/// Tool: `ps` — list running processes by walking the kernel task list.
///
/// Two-pass walk: user-space processes first (mm != NULL), then kernel threads.
/// This ensures user processes (like `sleep`, `bash`) appear before kernel threads
/// fill the output buffer, since kernel threads dominate the early task list.
fn tool_ps(output: &mut KVVec<u8>) {
    // Offset of `tasks` field within `task_struct` for container_of.
    let tasks_offset = mem::offset_of!(bindings::task_struct, tasks);

    // SAFETY: Acquire RCU read-side lock to protect against task_struct
    // being freed while we iterate. The kernel guarantees that RCU-protected
    // task_structs remain valid until after the grace period following our
    // rcu_read_unlock (via Guard drop).
    let _rcu = rcu::read_lock();

    // init_task is a global exported symbol, always valid.
    let init = ptr::addr_of!(bindings::init_task);
    // SAFETY: init_task is always valid; reading its tasks field is safe.
    let list_head = unsafe { ptr::addr_of!((*init).tasks) };

    let mut count = 0usize;

    // Pass 1: User-space processes (mm != NULL).
    let _ = output.extend_from_slice(
        b"=== User-Space Processes ===\nPID      PPID     STATE  COMM\n",
        GFP_KERNEL,
    );
    // SAFETY: init_task.tasks.next points to the first real task's `tasks`
    // list_head, or back to init_task.tasks if there are no other tasks.
    let mut current = unsafe { (*list_head).next };
    while current != list_head as *mut bindings::list_head && count < MAX_PS_TASKS {
        let task = unsafe {
            (current as *const u8).sub(tasks_offset) as *const bindings::task_struct
        };
        // SAFETY: task->mm is a pointer read; NULL means kernel thread.
        // Under RCU, the task_struct (and its mm field) is valid.
        let mm = unsafe { (*task).mm };
        if !mm.is_null() {
            // SAFETY: task is valid under RCU, format_task only reads fields.
            unsafe { format_task(output, task) };
            count += 1;
        }
        current = unsafe { (*current).next };
    }

    // Pass 2: Kernel threads (mm == NULL), if we still have room.
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
                // SAFETY: task is valid under RCU, format_task only reads fields.
                unsafe { format_task(output, task) };
                count += 1;
            }
            current = unsafe { (*current).next };
        }
    }

    if count >= MAX_PS_TASKS {
        let _ = output.extend_from_slice(b"[... truncated]\n", GFP_KERNEL);
    }

    // _rcu guard dropped here → rcu_read_unlock()
}

/// Tool: `mem` — detailed memory statistics via si_meminfo().
fn tool_mem(output: &mut KVVec<u8>) {
    // SAFETY: si_meminfo() fills a stack-allocated sysinfo struct.
    // Safe to call from process context.
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

    // RAM
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

    // Swap
    let _ = output.extend_from_slice(b"Swap Total:  ", GFP_KERNEL);
    let _ = output.extend_from_slice(format_usize(swap_total_mb, &mut num), GFP_KERNEL);
    let _ = output.extend_from_slice(b" MB\n", GFP_KERNEL);

    let _ = output.extend_from_slice(b"Swap Used:   ", GFP_KERNEL);
    let _ = output.extend_from_slice(format_usize(swap_used_mb, &mut num), GFP_KERNEL);
    let _ = output.extend_from_slice(b" MB\n", GFP_KERNEL);

    let _ = output.extend_from_slice(b"Swap Free:   ", GFP_KERNEL);
    let _ = output.extend_from_slice(format_usize(swap_free_mb, &mut num), GFP_KERNEL);
    let _ = output.extend_from_slice(b" MB\n", GFP_KERNEL);

    // Usage percentage
    if total_mb > 0 {
        let pct = (used_mb * 100) / total_mb;
        let _ = output.extend_from_slice(b"RAM Usage:   ", GFP_KERNEL);
        let _ = output.extend_from_slice(format_usize(pct, &mut num), GFP_KERNEL);
        let _ = output.extend_from_slice(b"%\n", GFP_KERNEL);
    }
}

/// Tool: `loadavg` — system load averages from the kernel's avenrun[] array.
///
/// avenrun[] stores load averages as unsigned long in fixed-point format
/// with FSHIFT=11 bits of precision (i.e., value * 2048).
fn tool_loadavg(output: &mut KVVec<u8>) {
    // FSHIFT=11, so divisor is 2048.
    const FSHIFT: usize = 11;
    const FIXED_1: usize = 1 << FSHIFT; // 2048

    // SAFETY: avenrun is a global exported symbol (unsigned long[3]).
    // Reading it is safe — the values are updated atomically by the scheduler.
    let avg1 = unsafe { ptr::read_volatile(ptr::addr_of!(avenrun[0])) } as usize;
    let avg5 = unsafe { ptr::read_volatile(ptr::addr_of!(avenrun[1])) } as usize;
    let avg15 = unsafe { ptr::read_volatile(ptr::addr_of!(avenrun[2])) } as usize;

    let mut num = [0u8; 20];

    let _ = output.extend_from_slice(b"=== Load Averages ===\n", GFP_KERNEL);

    // Format each as "X.XX"
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
        // Zero-pad fractional part to 2 digits.
        if frac < 10 {
            let _ = output.push(b'0', GFP_KERNEL);
        }
        let _ = output.extend_from_slice(format_usize(frac, &mut num), GFP_KERNEL);
        let _ = output.push(b'\n', GFP_KERNEL);
    }

    // Number of online CPUs for context.
    let cpus = read_num_online_cpus();
    let _ = output.extend_from_slice(b"Online CPUs: ", GFP_KERNEL);
    let _ = output.extend_from_slice(format_usize(cpus, &mut num), GFP_KERNEL);
    let _ = output.push(b'\n', GFP_KERNEL);

    // Uptime for additional context.
    let _ = output.extend_from_slice(b"Uptime:      ", GFP_KERNEL);
    append_uptime(output);
    let _ = output.push(b'\n', GFP_KERNEL);
}

// ---------------------------------------------------------------------------
// Step 3a: Model firmware loading
//
// Loads the hackbot binary model from /lib/firmware/hackbot-model.bin on first
// device open. Parses the header, builds tokenizer index, and computes weight
// offsets into the firmware data blob. Zero-copy for weights — they stay in
// the original (copied) firmware buffer.
// ---------------------------------------------------------------------------

/// Read a little-endian u32 from a byte slice at the given offset.
/// Returns Err if the read would go out of bounds.
fn read_u32_le(data: &[u8], off: usize) -> Result<u32> {
    if off + 4 > data.len() {
        return Err(EINVAL);
    }
    Ok(u32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]]))
}

/// Read a little-endian u16 from a byte slice at the given offset.
fn read_u16_le(data: &[u8], off: usize) -> Result<u16> {
    if off + 2 > data.len() {
        return Err(EINVAL);
    }
    Ok(u16::from_le_bytes([data[off], data[off + 1]]))
}

/// Advance cursor past a Q8 weight matrix [rows × cols] and return a Q8Ref.
/// Layout: [i8; rows*cols] data, then [i32; rows*(cols/gs)] scales.
fn q8_ref_advance(cursor: &mut usize, rows: usize, cols: usize, gs: usize, data_len: usize) -> Result<Q8Ref> {
    let data_off = *cursor;
    let data_size = rows * cols; // i8 per element
    *cursor += data_size;

    let scale_off = *cursor;
    let n_groups = cols / gs;
    let scale_size = rows * n_groups * 4; // i32 per group per row
    *cursor += scale_size;

    if *cursor > data_len {
        return Err(EINVAL);
    }

    Ok(Q8Ref { data_off, scale_off, rows, cols })
}

/// Advance cursor past a RMSNorm weight [i32; dim] and return its offset.
fn norm_ref_advance(cursor: &mut usize, dim: usize, data_len: usize) -> Result<usize> {
    let off = *cursor;
    *cursor += dim * 4; // i32 per element (Q16.16 fixed-point)
    if *cursor > data_len {
        return Err(EINVAL);
    }
    Ok(off)
}

/// Parse the binary header and populate a ModelConfig.
fn parse_model_header(data: &[u8]) -> Result<ModelConfig> {
    if data.len() < MODEL_HEADER_SIZE {
        pr_err!("hackbot: model file too small ({} bytes, need >= {})\n",
                data.len(), MODEL_HEADER_SIZE);
        return Err(EINVAL);
    }

    let magic = read_u32_le(data, 0)?;
    if magic != MODEL_MAGIC {
        pr_err!("hackbot: bad model magic: 0x{:08X} (expected 0x{:08X})\n",
                magic, MODEL_MAGIC);
        return Err(EINVAL);
    }

    let version = read_u32_le(data, 4)?;
    if version != MODEL_FORMAT_V1 && version != MODEL_FORMAT_V2 {
        pr_err!("hackbot: unsupported model version: {} (expected {} or {})\n",
                version, MODEL_FORMAT_V1, MODEL_FORMAT_V2);
        return Err(EINVAL);
    }

    let config = ModelConfig {
        dim:        read_u32_le(data, 8)?,
        hidden_dim: read_u32_le(data, 12)?,
        n_layers:   read_u32_le(data, 16)?,
        n_heads:    read_u32_le(data, 20)?,
        n_kv_heads: read_u32_le(data, 24)?,
        vocab_size: read_u32_le(data, 28)?,
        seq_len:    read_u32_le(data, 32)?,
        group_size: read_u32_le(data, 36)?,
        head_dim:   read_u32_le(data, 40)?,
        kv_dim:     read_u32_le(data, 44)?,
        rope_theta: read_u32_le(data, 48)?,
    };

    // Validate constraints
    if config.n_layers as usize > MODEL_MAX_LAYERS {
        pr_err!("hackbot: too many layers: {} (max {})\n",
                config.n_layers, MODEL_MAX_LAYERS);
        return Err(EINVAL);
    }
    if config.vocab_size as usize > MODEL_MAX_VOCAB {
        pr_err!("hackbot: vocab too large: {} (max {})\n",
                config.vocab_size, MODEL_MAX_VOCAB);
        return Err(EINVAL);
    }
    if config.dim == 0 || config.hidden_dim == 0 || config.head_dim == 0 {
        pr_err!("hackbot: invalid model dimensions\n");
        return Err(EINVAL);
    }
    // v1 (INT8) requires non-zero group_size that divides weight dimensions.
    // v2 (FP16) repurposes the group_size field as weight_type (0 = FP16).
    if version == MODEL_FORMAT_V1 {
        if config.group_size == 0 {
            pr_err!("hackbot: group_size cannot be zero\n");
            return Err(EINVAL);
        }
        let gs = config.group_size as usize;
        if config.dim as usize % gs != 0 {
            pr_err!("hackbot: dim {} not divisible by group_size {}\n", config.dim, gs);
            return Err(EINVAL);
        }
        if config.hidden_dim as usize % gs != 0 {
            pr_err!("hackbot: hidden_dim {} not divisible by group_size {}\n",
                    config.hidden_dim, gs);
            return Err(EINVAL);
        }
    }

    Ok(config)
}

/// Parse and store the model from a firmware data blob into the global MODEL slot.
/// Called with MODEL mutex held.
fn parse_and_store_model(data: &[u8], slot: &mut ModelSlot) -> Result {
    let config = parse_model_header(data)?;
    let gs = config.group_size as usize;
    let dim = config.dim as usize;
    let hidden_dim = config.hidden_dim as usize;
    let n_layers = config.n_layers as usize;
    let n_heads = config.n_heads as usize;
    let n_kv_heads = config.n_kv_heads as usize;
    let head_dim = config.head_dim as usize;
    let vocab_size = config.vocab_size as usize;

    pr_info!("hackbot: model config: dim={}, hidden_dim={}, layers={}, heads={}/{}, vocab={}\n",
             dim, hidden_dim, n_layers, n_heads, n_kv_heads, vocab_size);

    // --- Parse tokenizer section ---
    let tok_section_off = MODEL_HEADER_SIZE;
    let mut pos = tok_section_off;

    let tok_n_vocab = read_u32_le(data, pos)? as usize;
    pos += 4;
    let _tok_max_len = read_u32_le(data, pos)?;
    pos += 4;

    if tok_n_vocab != vocab_size {
        pr_err!("hackbot: tokenizer vocab mismatch: {} vs header {}\n",
                tok_n_vocab, vocab_size);
        return Err(EINVAL);
    }

    // Allocate tokenizer offset index: one u32 per token
    let tok_alloc_size = vocab_size * core::mem::size_of::<u32>();
    // SAFETY: kvrealloc_node_align_noprof with null pointer acts as kvmalloc.
    // GFP_KERNEL is safe in process context. Result may be null on OOM.
    let tok_ptr = unsafe {
        bindings::kvrealloc_node_align_noprof(
            core::ptr::null(),
            tok_alloc_size,
            core::mem::align_of::<u32>() as _,
            bindings::GFP_KERNEL,
            bindings::NUMA_NO_NODE,
        )
    } as *mut u32;

    if tok_ptr.is_null() {
        pr_err!("hackbot: failed to allocate tokenizer index ({} bytes)\n", tok_alloc_size);
        return Err(ENOMEM);
    }

    // Walk tokenizer entries, recording the absolute offset of each token
    for i in 0..vocab_size {
        if pos + 6 > data.len() {
            // SAFETY: tok_ptr was just allocated and is non-null.
            unsafe { bindings::kvfree(tok_ptr as *const core::ffi::c_void) };
            pr_err!("hackbot: tokenizer truncated at token {}\n", i);
            return Err(EINVAL);
        }
        // Store absolute offset to this token's record (score, len, bytes)
        // SAFETY: tok_ptr points to a valid [u32; vocab_size] allocation, i < vocab_size.
        unsafe { *tok_ptr.add(i) = pos as u32 };
        // Skip: i32 score
        pos += 4;
        // Read u16 len
        let token_len = read_u16_le(data, pos)? as usize;
        pos += 2;
        // Skip: [u8; token_len]
        pos += token_len;
    }

    if pos > data.len() {
        unsafe { bindings::kvfree(tok_ptr as *const core::ffi::c_void) };
        pr_err!("hackbot: tokenizer extends past end of file\n");
        return Err(EINVAL);
    }

    pr_info!("hackbot: tokenizer parsed: {} tokens, {} bytes\n",
             vocab_size, pos - tok_section_off);

    // --- Parse weight offsets ---
    let mut cursor = pos;
    let weights_start = pos; // byte offset where weights begin

    if config.group_size == 0 {
        // Format v2: FP16 weights, no group quantization.
        // Weight type field (was group_size) is 0 for FP16.
        // Just validate the total size — actual offsets are computed by C code.
        let mut expected = 0usize;

        // embed: [vocab_size, dim] fp16
        expected += vocab_size * dim * 2;
        for _l in 0..n_layers {
            expected += dim * 4; // rms_att: float32
            expected += n_heads * head_dim * dim * 2; // wq: fp16
            expected += n_kv_heads * head_dim * dim * 2; // wk: fp16
            expected += n_kv_heads * head_dim * dim * 2; // wv: fp16
            expected += dim * n_heads * head_dim * 2; // wo: fp16
            expected += dim * 4; // rms_ffn: float32
            expected += hidden_dim * dim * 2; // gate: fp16
            expected += hidden_dim * dim * 2; // up: fp16
            expected += dim * hidden_dim * 2; // down: fp16
        }
        expected += dim * 4; // rms_final: float32

        let available = data.len().saturating_sub(weights_start);
        if available < expected {
            unsafe { bindings::kvfree(tok_ptr as *const core::ffi::c_void) };
            pr_err!("hackbot: v2 weights truncated: need {} bytes, have {}\n",
                    expected, available);
            return Err(EINVAL);
        }

        cursor = weights_start + expected;
        if cursor < data.len() {
            pr_warn!("hackbot: model has {} trailing bytes\n", data.len() - cursor);
        }

        pr_info!("hackbot: v2 (FP16) weights: {} bytes ({} layers)\n",
                 expected, n_layers);

        // Store minimal info — the C FPU code handles weight layout internally
        slot.config = config;
        // Override group_size for v2: set it to a sentinel so v1 code paths
        // don't accidentally run (group_size=0 would cause division by zero).
        // The v2 path doesn't use group_size.
        slot.config.group_size = 0;
        slot.tok_section_off = tok_section_off;
        slot.tok_offsets_addr = tok_ptr as usize;
        // Embedding/layer refs are unused for v2 — C code computes its own offsets
        slot.embed = Q8Ref::ZERO;
        slot.layers = [LayerRef::ZERO; MODEL_MAX_LAYERS];
        slot.rms_final_off = 0;
        slot.format_version = MODEL_FORMAT_V2;
        slot.weights_off = weights_start;

        return Ok(());
    }

    // Format v1: INT8 weights with per-group Q16.16 scales
    let gs = config.group_size as usize;
    // Validate group_size divides all weight column dimensions
    if dim % gs != 0 {
        unsafe { bindings::kvfree(tok_ptr as *const core::ffi::c_void) };
        pr_err!("hackbot: dim {} not divisible by group_size {}\n", dim, gs);
        return Err(EINVAL);
    }
    if hidden_dim % gs != 0 {
        unsafe { bindings::kvfree(tok_ptr as *const core::ffi::c_void) };
        pr_err!("hackbot: hidden_dim {} not divisible by group_size {}\n", hidden_dim, gs);
        return Err(EINVAL);
    }

    // 1. Embedding table: Q8[vocab_size, dim]
    let embed = q8_ref_advance(&mut cursor, vocab_size, dim, gs, data.len())
        .map_err(|e| {
            unsafe { bindings::kvfree(tok_ptr as *const core::ffi::c_void) };
            pr_err!("hackbot: embedding weight offset overflow\n");
            e
        })?;

    // 2. Per-layer weights
    let mut layers = [LayerRef::ZERO; MODEL_MAX_LAYERS];
    for l in 0..n_layers {
        let rms_att_off = norm_ref_advance(&mut cursor, dim, data.len())
            .map_err(|e| {
                unsafe { bindings::kvfree(tok_ptr as *const core::ffi::c_void) };
                pr_err!("hackbot: layer {} rms_att overflow\n", l);
                e
            })?;

        macro_rules! q8_or_cleanup {
            ($rows:expr, $cols:expr, $name:expr) => {
                q8_ref_advance(&mut cursor, $rows, $cols, gs, data.len())
                    .map_err(|e| {
                        unsafe { bindings::kvfree(tok_ptr as *const core::ffi::c_void) };
                        pr_err!("hackbot: layer {} {} overflow\n", l, $name);
                        e
                    })?
            };
        }
        macro_rules! norm_or_cleanup {
            ($dim:expr, $name:expr) => {
                norm_ref_advance(&mut cursor, $dim, data.len())
                    .map_err(|e| {
                        unsafe { bindings::kvfree(tok_ptr as *const core::ffi::c_void) };
                        pr_err!("hackbot: layer {} {} overflow\n", l, $name);
                        e
                    })?
            };
        }

        let wq = q8_or_cleanup!(n_heads * head_dim, dim, "wq");
        let wk = q8_or_cleanup!(n_kv_heads * head_dim, dim, "wk");
        let wv = q8_or_cleanup!(n_kv_heads * head_dim, dim, "wv");
        let wo = q8_or_cleanup!(dim, n_heads * head_dim, "wo");
        let rms_ffn_off = norm_or_cleanup!(dim, "rms_ffn");
        let gate = q8_or_cleanup!(hidden_dim, dim, "gate");
        let up = q8_or_cleanup!(hidden_dim, dim, "up");
        let down = q8_or_cleanup!(dim, hidden_dim, "down");

        layers[l] = LayerRef {
            rms_att_off, wq, wk, wv, wo, rms_ffn_off, gate, up, down,
        };
    }

    // 3. Final RMSNorm
    let rms_final_off = norm_ref_advance(&mut cursor, dim, data.len())
        .map_err(|e| {
            unsafe { bindings::kvfree(tok_ptr as *const core::ffi::c_void) };
            pr_err!("hackbot: rms_final overflow\n");
            e
        })?;

    // Verify we consumed the entire file (no trailing garbage)
    if cursor != data.len() {
        pr_warn!("hackbot: model has {} trailing bytes (expected end at {}, file is {})\n",
                 data.len() - cursor, cursor, data.len());
    }

    pr_info!("hackbot: v1 (INT8) weights parsed: {} bytes ({} layers)\n",
             cursor - pos, n_layers);

    // --- Store everything in the slot ---
    slot.config = config;
    slot.tok_section_off = tok_section_off;
    slot.tok_offsets_addr = tok_ptr as usize;
    slot.embed = embed;
    slot.layers = layers;
    slot.rms_final_off = rms_final_off;
    slot.format_version = MODEL_FORMAT_V1;
    slot.weights_off = weights_start;

    Ok(())
}

/// Attempt to load the model firmware on first device open.
/// Best-effort: if firmware is not found or parsing fails, the module
/// continues to work with vLLM remote inference only.
fn load_model_if_needed(dev: &Device) {
    let mut slot = MODEL.lock();
    if slot.loaded {
        return; // Already loaded
    }

    // Try to load firmware (non-fatal if missing)
    let fw = match Firmware::request_nowarn(c_str!("hackbot-model.bin"), dev) {
        Ok(fw) => fw,
        Err(_) => {
            pr_info!("hackbot: no model firmware found, local inference disabled\n");
            pr_info!("hackbot: to enable: sudo cp hackbot-model.bin /lib/firmware/\n");
            return;
        }
    };

    let fw_data = fw.data();
    let fw_len = fw_data.len();
    pr_info!("hackbot: loaded firmware: {} bytes ({} MB)\n",
             fw_len, fw_len / (1024 * 1024));

    // Allocate buffer and copy firmware data (so Firmware RAII object can be dropped)
    // SAFETY: kvrealloc_node_align_noprof with null acts as kvmalloc.
    // This is process context (open syscall), GFP_KERNEL is appropriate.
    let data_ptr = unsafe {
        bindings::kvrealloc_node_align_noprof(
            core::ptr::null(),
            fw_len,
            1, // byte alignment
            bindings::GFP_KERNEL,
            bindings::NUMA_NO_NODE,
        )
    } as *mut u8;

    if data_ptr.is_null() {
        pr_err!("hackbot: failed to allocate {} bytes for model data\n", fw_len);
        return;
    }

    // SAFETY: data_ptr is valid for fw_len bytes (just allocated), fw_data.as_ptr()
    // is valid for fw_len bytes (firmware data). No overlap possible.
    unsafe {
        core::ptr::copy_nonoverlapping(fw_data.as_ptr(), data_ptr, fw_len);
    }

    // Drop the Firmware object — we've copied the data
    drop(fw);

    // Parse the model from our copy
    // SAFETY: data_ptr is valid for data_len bytes, just copied from firmware.
    let data_slice = unsafe { core::slice::from_raw_parts(data_ptr, fw_len) };

    match parse_and_store_model(data_slice, &mut slot) {
        Ok(()) => {
            slot.data_addr = data_ptr as usize;
            slot.data_len = fw_len;

            // Allocate inference state (KV cache + activation buffers)
            match alloc_inference_state(&mut slot) {
                Ok(()) => {
                    // Build sorted vocabulary index for BPE encoding
                    match build_sorted_vocab(&mut slot) {
                        Ok(()) => {
                            slot.loaded = true;
                            pr_info!("hackbot: model ready for inference ({}×{}, {} layers)\n",
                                     slot.config.dim, slot.config.hidden_dim, slot.config.n_layers);
                        }
                        Err(_) => {
                            // Free inference state and everything else
                            if slot.fpu_state != 0 {
                                // SAFETY: fpu_state was allocated by hackbot_fpu_alloc.
                                unsafe { hackbot_fpu_free(slot.fpu_state as *mut core::ffi::c_void) };
                                slot.fpu_state = 0;
                            }
                            unsafe { bindings::kvfree(slot.inf_buf_addr as *const core::ffi::c_void) };
                            slot.inf_buf_addr = 0;
                            slot.inf_buf_len = 0;
                            if slot.tok_offsets_addr != 0 {
                                unsafe { bindings::kvfree(slot.tok_offsets_addr as *const core::ffi::c_void) };
                                slot.tok_offsets_addr = 0;
                            }
                            unsafe { bindings::kvfree(data_ptr as *const core::ffi::c_void) };
                            slot.data_addr = 0;
                            slot.data_len = 0;
                            pr_err!("hackbot: sorted vocab build failed\n");
                        }
                    }
                }
                Err(_) => {
                    // Failed to allocate inference state — free everything
                    if slot.tok_offsets_addr != 0 {
                        unsafe { bindings::kvfree(slot.tok_offsets_addr as *const core::ffi::c_void) };
                        slot.tok_offsets_addr = 0;
                    }
                    unsafe { bindings::kvfree(data_ptr as *const core::ffi::c_void) };
                    slot.data_addr = 0;
                    slot.data_len = 0;
                    pr_err!("hackbot: inference state allocation failed\n");
                }
            }
        }
        Err(_) => {
            // parse_and_store_model already logged the error and freed tok_offsets
            // SAFETY: data_ptr was allocated via kvrealloc_node_align_noprof.
            unsafe { bindings::kvfree(data_ptr as *const core::ffi::c_void) };
            pr_err!("hackbot: model loading failed, local inference disabled\n");
        }
    }
}

/// Free model resources. Called during module unload.
fn free_model_resources() {
    let mut slot = MODEL.lock();
    if !slot.loaded {
        return;
    }

    // Free the firmware data copy
    if slot.data_addr != 0 {
        // SAFETY: data_addr was allocated via kvrealloc_node_align_noprof
        // during load_model_if_needed(). It has not been freed since.
        unsafe { bindings::kvfree(slot.data_addr as *const core::ffi::c_void) };
        slot.data_addr = 0;
        slot.data_len = 0;
    }

    // Free the tokenizer offset index
    if slot.tok_offsets_addr != 0 {
        // SAFETY: tok_offsets_addr was allocated via kvrealloc_node_align_noprof
        // during parse_and_store_model(). It has not been freed since.
        unsafe { bindings::kvfree(slot.tok_offsets_addr as *const core::ffi::c_void) };
        slot.tok_offsets_addr = 0;
    }

    // Free FPU inference state (v2)
    if slot.fpu_state != 0 {
        // SAFETY: fpu_state was allocated by hackbot_fpu_alloc during
        // alloc_inference_state(). It has not been freed since.
        unsafe { hackbot_fpu_free(slot.fpu_state as *mut core::ffi::c_void) };
        slot.fpu_state = 0;
    }

    // Free the inference state buffer (v1: KV cache + activations)
    if slot.inf_buf_addr != 0 {
        // SAFETY: inf_buf_addr was allocated via kvrealloc_node_align_noprof
        // during alloc_inference_state(). It has not been freed since.
        unsafe { bindings::kvfree(slot.inf_buf_addr as *const core::ffi::c_void) };
        slot.inf_buf_addr = 0;
        slot.inf_buf_len = 0;
    }

    // Free the sorted vocabulary index
    if slot.sorted_vocab_addr != 0 {
        // SAFETY: sorted_vocab_addr was allocated via kvrealloc_node_align_noprof
        // during build_sorted_vocab(). It has not been freed since.
        unsafe { bindings::kvfree(slot.sorted_vocab_addr as *const core::ffi::c_void) };
        slot.sorted_vocab_addr = 0;
    }

    slot.loaded = false;
    pr_info!("hackbot: model resources freed\n");
}

// ---------------------------------------------------------------------------
// Step 3b: Integer math primitives for in-kernel inference
//
// All operations use Q16.16 fixed-point arithmetic (i32).
// No FPU/SIMD — pure scalar integer math only.
//
// Q16.16 format: 16 bits integer + 16 bits fraction.
//   1.0 = 65536, 0.5 = 32768, -1.0 = -65536
//   Multiply: ((a as i64) * (b as i64)) >> 16
//   Add/Sub: just i32 add/sub
// ---------------------------------------------------------------------------

/// 1.0 in Q16.16.
#[allow(dead_code)]
const Q16_ONE: i32 = 1 << 16; // 65536

/// 2π in Q16.16 (= 6.28318... × 65536).
#[allow(dead_code)]
const TWO_PI_Q16: i64 = 411775;

/// exp(-k) in Q16.16 for k = 0..16. Used by `exp_q16_neg`.
#[allow(dead_code)]
const EXP_TABLE: [i32; 17] = [
    65536, 24109, 8869, 3263, 1200, 442, 162, 60, 22, 8, 3, 1, 0, 0, 0, 0, 0,
];

/// sin(2π·k/256) in Q16.16 for k = 0..255. cos(θ) = SIN_TABLE[(k+64)%256].
#[allow(dead_code)]
const SIN_TABLE: [i32; 256] = [
        0,  1608,  3216,  4821,  6424,  8022,  9616, 11204, 12785, 14359,
    15924, 17479, 19024, 20557, 22078, 23586, 25080, 26558, 28020, 29466,
    30893, 32303, 33692, 35062, 36410, 37736, 39040, 40320, 41576, 42806,
    44011, 45190, 46341, 47464, 48559, 49624, 50660, 51665, 52639, 53581,
    54491, 55368, 56212, 57022, 57798, 58538, 59244, 59914, 60547, 61145,
    61705, 62228, 62714, 63162, 63572, 63944, 64277, 64571, 64827, 65043,
    65220, 65358, 65457, 65516, 65536, 65516, 65457, 65358, 65220, 65043,
    64827, 64571, 64277, 63944, 63572, 63162, 62714, 62228, 61705, 61145,
    60547, 59914, 59244, 58538, 57798, 57022, 56212, 55368, 54491, 53581,
    52639, 51665, 50660, 49624, 48559, 47464, 46341, 45190, 44011, 42806,
    41576, 40320, 39040, 37736, 36410, 35062, 33692, 32303, 30893, 29466,
    28020, 26558, 25080, 23586, 22078, 20557, 19024, 17479, 15924, 14359,
    12785, 11204,  9616,  8022,  6424,  4821,  3216,  1608,     0, -1608,
    -3216, -4821, -6424, -8022, -9616,-11204,-12785,-14359,-15924,-17479,
   -19024,-20557,-22078,-23586,-25080,-26558,-28020,-29466,-30893,-32303,
   -33692,-35062,-36410,-37736,-39040,-40320,-41576,-42806,-44011,-45190,
   -46341,-47464,-48559,-49624,-50660,-51665,-52639,-53581,-54491,-55368,
   -56212,-57022,-57798,-58538,-59244,-59914,-60547,-61145,-61705,-62228,
   -62714,-63162,-63572,-63944,-64277,-64571,-64827,-65043,-65220,-65358,
   -65457,-65516,-65536,-65516,-65457,-65358,-65220,-65043,-64827,-64571,
   -64277,-63944,-63572,-63162,-62714,-62228,-61705,-61145,-60547,-59914,
   -59244,-58538,-57798,-57022,-56212,-55368,-54491,-53581,-52639,-51665,
   -50660,-49624,-48559,-47464,-46341,-45190,-44011,-42806,-41576,-40320,
   -39040,-37736,-36410,-35062,-33692,-32303,-30893,-29466,-28020,-26558,
   -25080,-23586,-22078,-20557,-19024,-17479,-15924,-14359,-12785,-11204,
    -9616, -8022, -6424, -4821, -3216, -1608,
];

/// RoPE frequencies: 1/10000^(2i/64) in Q16.16, for head_dim=64, theta=10000.
#[allow(dead_code)]
const ROPE_FREQS_64: [i32; 32] = [
    65536, 49145, 36854, 27636, 20724, 15541, 11654,  8739,
     6554,  4915,  3685,  2764,  2072,  1554,  1165,   874,
      655,   491,   369,   276,   207,   155,   117,    87,
       66,    49,    37,    28,    21,    16,    12,     9,
];

/// Integer square root via Newton's method.
/// Returns floor(sqrt(n)). Converges in ≤ 32 iterations for u64.
#[allow(dead_code)]
fn isqrt_u64(n: u64) -> u64 {
    if n < 2 {
        return n;
    }
    // Initial guess: 2^(ceil(bit_length/2))
    let bits = 64 - n.leading_zeros();
    let mut x = 1u64 << ((bits + 1) / 2);
    loop {
        let y = (x + n / x) / 2;
        if y >= x {
            return x;
        }
        x = y;
    }
}

/// Exponential function for non-positive arguments in Q16.16.
/// Uses table lookup for integer part + Taylor expansion for fractional part.
/// For x ≤ -16·65536, returns 0 (exp(-16) < 2^-16, unrepresentable in Q16.16).
#[allow(dead_code)]
fn exp_q16_neg(x: i32) -> i32 {
    // x is Q16.16, x ≤ 0
    if x >= 0 {
        return Q16_ONE; // exp(0) = 1.0
    }
    // Decompose x = x_int + x_frac where x_int = floor(x), x_frac ∈ [0, 1)
    // For negative Q16.16: >> 16 gives floor (arithmetic right shift rounds toward -∞)
    let x_int = x >> 16; // negative integer part (e.g., -3 for x = -2.5)
    let idx = (-x_int) as usize; // table index = |x_int|
    if idx >= EXP_TABLE.len() {
        return 0; // exp(-17) and below: effectively zero
    }
    let exp_int = EXP_TABLE[idx] as i64; // exp(x_int) in Q16.16

    // Fractional part: x_frac = x - (x_int << 16), always in [0, 65535]
    let x_frac = (x - (x_int << 16)) as i64; // Q16.16 fraction in [0, 65535]

    // Taylor expansion for exp(f) where f = x_frac/65536 ∈ [0, 1):
    // exp(f) ≈ 1 + f + f²/2 + f³/6
    // In Q16.16:
    let f = x_frac; // Q16.16
    let f2 = (f * f) >> 16; // f² in Q16.16
    let f3 = (f2 * f) >> 16; // f³ in Q16.16
    let exp_frac = Q16_ONE as i64 + f + (f2 >> 1) + f3 / 6; // Q16.16

    // Combined: exp(x) = exp(x_int) × exp(x_frac)
    ((exp_int * exp_frac) >> 16) as i32
}

/// Sigmoid function in Q16.16: σ(x) = 1/(1+exp(-x)).
/// Returns value in [0, 65536] representing [0.0, 1.0].
#[allow(dead_code)]
fn sigmoid_q16(x: i32) -> i32 {
    if x >= 0 {
        // σ(x) = 1 / (1 + exp(-x))
        let e = exp_q16_neg(-x) as i64; // exp(-x), Q16.16
        // result = 65536² / (65536 + e)
        let num = (Q16_ONE as i64) * (Q16_ONE as i64); // 2^32
        (num / (Q16_ONE as i64 + e)) as i32
    } else {
        // σ(x) = exp(x) / (1 + exp(x))
        let e = exp_q16_neg(x) as i64; // exp(x), Q16.16
        ((e * Q16_ONE as i64) / (Q16_ONE as i64 + e)) as i32
    }
}

/// SiLU (Swish) activation in Q16.16: silu(x) = x · σ(x).
#[allow(dead_code)]
fn silu_q16(x: i32) -> i32 {
    let sig = sigmoid_q16(x) as i64;
    ((x as i64 * sig) >> 16) as i32
}

/// Sine lookup with linear interpolation. Input: angle in Q16.16 radians.
/// Output: sin(angle) in Q16.16.
#[allow(dead_code)]
fn sin_q16(angle_q16: i32) -> i32 {
    // Reduce angle to [0, 2π) via modulo
    let two_pi = TWO_PI_Q16;
    let mut a = angle_q16 as i64 % two_pi;
    if a < 0 {
        a += two_pi;
    }
    // Map [0, 2π) → [0, 256) for table index
    // index = a * 256 / (2π in Q16.16) = a * 256 / 411775
    // For better precision: (a << 8) / 411775
    let idx_fixed = (a << 8) / two_pi; // Q0 index in [0, 256)
    let idx = idx_fixed as usize;
    let frac = ((a << 8) - idx_fixed * two_pi) as i32; // fractional part for interpolation

    // Linear interpolation between SIN_TABLE[idx] and SIN_TABLE[(idx+1)%256]
    let s0 = SIN_TABLE[idx % 256] as i64;
    let s1 = SIN_TABLE[(idx + 1) % 256] as i64;
    // Interpolation weight: frac / two_pi ∈ [0, 1)
    // result = s0 + (s1 - s0) * frac / two_pi
    let interp = s0 + ((s1 - s0) * frac as i64) / two_pi;
    interp as i32
}

/// Cosine via sin(angle + π/2).
#[allow(dead_code)]
fn cos_q16(angle_q16: i32) -> i32 {
    // π/2 in Q16.16 = 102944 (= 1.5707963 × 65536)
    sin_q16(angle_q16.wrapping_add(102944))
}

/// Matrix-vector multiply with INT8 quantized weights.
///
/// Computes out[r] = Σ_c weight[r,c] × input[c] for each row r.
/// Weights are INT8 with per-group Q16.16 scales.
///
/// - `out`: output vector [rows], Q16.16
/// - `input`: input vector [cols], Q16.16
/// - `w_data`: INT8 weight data [rows × cols], stored as u8
/// - `w_scales`: Q16.16 scale data, stored as little-endian bytes.
///   Layout: [rows × n_groups] i32 values, where n_groups = cols / group_size.
/// - `rows`, `cols`: weight matrix dimensions
/// - `gs`: quantization group size (weights quantized per-group along cols)
#[allow(dead_code)]
fn matmul_q8(
    out: &mut [i32],
    input: &[i32],
    w_data: &[u8],
    w_scales: &[u8],
    rows: usize,
    cols: usize,
    gs: usize,
) {
    let n_groups = cols / gs;

    for r in 0..rows {
        let mut row_acc: i64 = 0;
        let row_base = r * cols;
        let scale_row_base = r * n_groups * 4; // byte offset for this row's scales

        for g in 0..n_groups {
            // Read the Q16.16 scale for this group (little-endian i32)
            let sb = scale_row_base + g * 4;
            let scale = i32::from_le_bytes([
                w_scales[sb], w_scales[sb + 1], w_scales[sb + 2], w_scales[sb + 3],
            ]) as i64;

            // Dot product of INT8 weights with Q16.16 activations for this group
            let data_base = row_base + g * gs;
            let x_base = g * gs;
            let mut group_acc: i64 = 0;

            for j in 0..gs {
                // u8 reinterpreted as i8, then sign-extended to i64
                let w = w_data[data_base + j] as i8 as i64;
                let x = input[x_base + j] as i64;
                group_acc += w * x;
            }

            // Accumulate: y_q16 += (group_dot × scale) >> 16
            // Math: Σ(w_i8 × x_q16) × scale_q16 / 2^16 gives Q16.16 result
            row_acc += (group_acc * scale) >> 16;
        }

        out[r] = row_acc as i32;
    }
}

/// RMS normalization in Q16.16 fixed-point.
///
/// Computes: out[i] = input[i] × weight[i] / RMS(input)
/// where RMS(input) = sqrt(mean(input²) + ε)
///
/// - `out`: output [dim], Q16.16
/// - `input`: input [dim], Q16.16
/// - `weight`: RMSNorm learned weight [dim], Q16.16 (from model file)
/// - `dim`: vector dimension
#[allow(dead_code)]
fn rmsnorm_q16(out: &mut [i32], input: &[i32], weight: &[u8], dim: usize) {
    // 1. Compute sum of squares: Σ(x_q16²)
    // Since x_q16² is always positive, accumulate as u64 for maximum headroom.
    // For typical activation range (|x_float| < 100 → |x_q16| < 6.5M ≈ 2^23):
    //   x_q16² ≈ 2^46, sum of 576 ≈ 2^56. Fits in u64.
    let mut ss: u64 = 0;
    for i in 0..dim {
        let xi = input[i] as i64;
        ss += (xi * xi) as u64;
    }

    // 2. Mean of squares: divide by dim
    let mean_sq = ss / dim as u64;

    // 3. Add epsilon to prevent division by zero
    // ε = 1e-5 in Q32.32 format (since x_q16² is Q32.32):
    // 1e-5 × 2^32 ≈ 42950
    let mean_sq_eps = mean_sq + 42950;

    // 4. Integer sqrt: rms_q16 = isqrt(mean_sq_eps)
    // Since mean_sq_eps is Σ(x_q16²)/dim which is in Q32.32 format,
    // isqrt gives the result in Q16.16 (half the fractional bits). ✓
    let rms_q16 = isqrt_u64(mean_sq_eps);

    // 5. Compute reciprocal: rsqrt = 1/rms in Q16.16
    // rsqrt_q16 = 2^32 / rms_q16 (because 1.0/rms_float = 2^16/rms_q16,
    // and rsqrt_q16 = (2^16/rms_q16) × 2^16 = 2^32/rms_q16)
    if rms_q16 == 0 {
        // All inputs zero — output is zero
        for i in 0..dim {
            out[i] = 0;
        }
        return;
    }
    let rsqrt_q16 = ((1u64 << 32) / rms_q16) as i64;

    // 6. Apply: out[i] = input[i] × weight[i] × rsqrt / 2^32
    // (two Q16.16 multiplications need >> 32 total to stay in Q16.16)
    for i in 0..dim {
        // Read weight from byte blob (little-endian i32)
        let wb = i * 4;
        let w = i32::from_le_bytes([
            weight[wb], weight[wb + 1], weight[wb + 2], weight[wb + 3],
        ]) as i64;

        let x = input[i] as i64;
        // x × rsqrt → Q32.32, >> 16 → Q16.16
        let x_norm = (x * rsqrt_q16) >> 16; // normalized x in Q16.16
        // x_norm × weight → Q32.32, >> 16 → Q16.16
        out[i] = ((x_norm * w) >> 16) as i32;
    }
}

/// Softmax in Q16.16, operating in-place.
///
/// Computes: x[i] = exp(x[i] - max(x)) / Σ_j exp(x[j] - max(x))
/// Output values sum to Q16_ONE (65536 = 1.0).
///
/// For greedy decoding (argmax), softmax is unnecessary — just find max logit.
#[allow(dead_code)]
fn softmax_q16(x: &mut [i32], len: usize) {
    if len == 0 {
        return;
    }
    if len == 1 {
        x[0] = Q16_ONE;
        return;
    }

    // 1. Find max for numerical stability
    let mut max_val = x[0];
    for i in 1..len {
        if x[i] > max_val {
            max_val = x[i];
        }
    }

    // 2. Compute exp(x[i] - max) and sum
    let mut sum: i64 = 0;
    for i in 0..len {
        let e = exp_q16_neg(x[i] - max_val); // always ≤ 0
        x[i] = e;
        sum += e as i64;
    }

    // 3. Normalize: x[i] = x[i] / sum (in Q16.16)
    if sum == 0 {
        // Shouldn't happen (max entry has exp(0) = 65536), but guard against it
        x[0] = Q16_ONE;
        return;
    }
    for i in 0..len {
        // x[i] / sum, scaled to Q16.16
        x[i] = ((x[i] as i64 * Q16_ONE as i64) / sum) as i32;
    }
}

/// Apply RoPE (Rotary Positional Embedding) to a single attention head vector.
///
/// Rotates pairs (vec[2i], vec[2i+1]) by angle θ_i = pos × freq[i]:
///   vec[2i]'   = vec[2i]·cos(θ) - vec[2i+1]·sin(θ)
///   vec[2i+1]' = vec[2i]·sin(θ) + vec[2i+1]·cos(θ)
///
/// - `vec`: Q/K vector for one head [head_dim], modified in-place, Q16.16
/// - `pos`: token position (0-indexed)
/// - `head_dim`: head dimension (must be even; 64 for SmolLM2)
#[allow(dead_code)]
fn rope_apply_q16(vec: &mut [i32], pos: usize, head_dim: usize) {
    let n_pairs = head_dim / 2;
    for i in 0..n_pairs {
        // Frequency for this dimension pair
        let freq = if i < ROPE_FREQS_64.len() {
            ROPE_FREQS_64[i] as i64
        } else {
            // Fallback for head_dim > 64: approximate freq = 1/θ^(2i/d)
            // For now, use 0 (no rotation) for unsupported dimensions
            0i64
        };

        // θ = pos × freq in Q16.16
        let theta_q16 = ((pos as i64 * freq) % TWO_PI_Q16) as i32;

        let cos_val = cos_q16(theta_q16) as i64;
        let sin_val = sin_q16(theta_q16) as i64;

        let v0 = vec[2 * i] as i64;
        let v1 = vec[2 * i + 1] as i64;

        // Rotation: [cos -sin; sin cos] × [v0; v1]
        vec[2 * i]     = ((v0 * cos_val - v1 * sin_val) >> 16) as i32;
        vec[2 * i + 1] = ((v0 * sin_val + v1 * cos_val) >> 16) as i32;
    }
}

/// Element-wise multiply: out[i] = a[i] × b[i] in Q16.16.
/// Used for the SwiGLU gate: output = silu(gate) * up.
#[allow(dead_code)]
fn elementwise_mul_q16(out: &mut [i32], a: &[i32], b: &[i32], len: usize) {
    for i in 0..len {
        out[i] = ((a[i] as i64 * b[i] as i64) >> 16) as i32;
    }
}

/// Vector addition: out[i] = a[i] + b[i]. Used for residual connections.
#[allow(dead_code)]
fn vec_add_q16(out: &mut [i32], a: &[i32], b: &[i32], len: usize) {
    for i in 0..len {
        out[i] = a[i].wrapping_add(b[i]);
    }
}

/// Apply SiLU activation in-place to a vector. Used in SwiGLU FFN.
#[allow(dead_code)]
fn silu_vec_q16(vec: &mut [i32], len: usize) {
    for i in 0..len {
        vec[i] = silu_q16(vec[i]);
    }
}

/// In-place element-wise multiply: a[i] = a[i] × b[i] in Q16.16.
/// Avoids borrow-checker issues when out and a would alias.
#[allow(dead_code)]
fn elementwise_mul_inplace_q16(a: &mut [i32], b: &[i32], len: usize) {
    for i in 0..len {
        a[i] = ((a[i] as i64 * b[i] as i64) >> 16) as i32;
    }
}

/// Find the index of the maximum value in a Q16.16 vector (argmax).
/// Used for greedy decoding: pick the most likely next token.
#[allow(dead_code)]
fn argmax_q16(data: &[i32], len: usize) -> usize {
    let mut best = 0;
    for i in 1..len {
        if data[i] > data[best] {
            best = i;
        }
    }
    best
}

// ---------------------------------------------------------------------------
// Step 3c: Transformer forward pass
//
// Single-token Llama-3 forward pass with Grouped Query Attention (GQA),
// SwiGLU FFN, RoPE, and KV cache. All integer arithmetic (Q16.16).
// ---------------------------------------------------------------------------

/// Allocate inference state: KV cache and activation buffers.
/// Called once after model is successfully parsed.
fn alloc_inference_state(slot: &mut ModelSlot) -> Result {
    let dim = slot.config.dim as usize;
    let hidden_dim = slot.config.hidden_dim as usize;
    let n_layers = slot.config.n_layers as usize;
    let n_kv_heads = slot.config.n_kv_heads as usize;
    let head_dim = slot.config.head_dim as usize;
    let vocab_size = slot.config.vocab_size as usize;

    // Format v2: allocate float32 inference state via C helper (uses kernel FPU)
    if slot.format_version == MODEL_FORMAT_V2 {
        let n_heads = slot.config.n_heads as usize;
        // SAFETY: hackbot_fpu_alloc allocates and initializes the FPU state.
        // All parameters are validated during parse_model_header/parse_and_store_model.
        let ptr = unsafe {
            hackbot_fpu_alloc(
                dim as i32, hidden_dim as i32, n_layers as i32,
                n_heads as i32, n_kv_heads as i32, head_dim as i32,
                vocab_size as i32, INFERENCE_MAX_SEQ as i32,
            )
        };
        if ptr.is_null() {
            pr_err!("hackbot: failed to allocate FPU inference state\n");
            return Err(ENOMEM);
        }
        slot.fpu_state = ptr as usize;
        pr_info!("hackbot: FPU inference state allocated (v2/float32)\n");
        return Ok(());
    }

    // Format v1: INT8/Q16.16 inference state
    // KV cache: [n_layers × 2(k/v) × n_kv_heads × max_seq × head_dim]
    let kv_len = n_layers * 2 * n_kv_heads * INFERENCE_MAX_SEQ * head_dim;

    // Layout activation buffers sequentially after KV cache
    let mut c = kv_len; // cursor in i32 elements
    let x = c;       c += dim;
    let xb = c;      c += dim;
    let xb2 = c;     c += dim;
    let q = c;       c += dim; // n_heads * head_dim == dim for SmolLM2
    let k = c;       c += n_kv_heads * head_dim;
    let v = c;       c += n_kv_heads * head_dim;
    let att = c;     c += INFERENCE_MAX_SEQ;
    let hb = c;      c += hidden_dim;
    let hb2 = c;     c += hidden_dim;
    let logits = c;  c += vocab_size;

    let total_elems = c;
    let total_bytes = total_elems * core::mem::size_of::<i32>();

    // SAFETY: kvrealloc with null acts as kvmalloc. Process context, GFP_KERNEL safe.
    let ptr = unsafe {
        bindings::kvrealloc_node_align_noprof(
            core::ptr::null(),
            total_bytes,
            core::mem::align_of::<i32>() as _,
            bindings::GFP_KERNEL,
            bindings::NUMA_NO_NODE,
        )
    };
    if ptr.is_null() {
        pr_err!("hackbot: failed to allocate inference state ({} bytes)\n", total_bytes);
        return Err(ENOMEM);
    }

    // Zero-initialize (critical for KV cache — unfilled positions must be zero)
    // SAFETY: ptr is valid for total_bytes, just allocated.
    unsafe { core::ptr::write_bytes(ptr as *mut u8, 0, total_bytes) };

    slot.inf_buf_addr = ptr as usize;
    slot.inf_buf_len = total_bytes;
    slot.inf_kv_len = kv_len;
    slot.inf_x = x;
    slot.inf_xb = xb;
    slot.inf_xb2 = xb2;
    slot.inf_q = q;
    slot.inf_k = k;
    slot.inf_v = v;
    slot.inf_att = att;
    slot.inf_hb = hb;
    slot.inf_hb2 = hb2;
    slot.inf_logits = logits;

    pr_info!("hackbot: inference state: {} KB (KV cache {} KB, activations {} KB)\n",
             total_bytes / 1024,
             kv_len * 4 / 1024,
             (total_bytes - kv_len * 4) / 1024);

    Ok(())
}

/// Zero the KV cache between conversations. Call before starting a new prompt.
#[allow(dead_code)]
fn reset_kv_cache(slot: &ModelSlot) {
    // v2: delegate to C FPU helper
    if slot.format_version == MODEL_FORMAT_V2 {
        if slot.fpu_state != 0 {
            // SAFETY: fpu_state was allocated by hackbot_fpu_alloc and is valid.
            unsafe { hackbot_fpu_reset(slot.fpu_state as *mut core::ffi::c_void); }
        }
        return;
    }
    // v1: zero the Q16.16 KV cache
    if slot.inf_buf_addr == 0 || slot.inf_kv_len == 0 {
        return;
    }
    let kv_bytes = slot.inf_kv_len * core::mem::size_of::<i32>();
    // SAFETY: inf_buf_addr is valid for at least kv_bytes (KV cache is at offset 0).
    unsafe {
        core::ptr::write_bytes(slot.inf_buf_addr as *mut u8, 0, kv_bytes);
    }
}

/// Run one token through the transformer, writing logits to inf_logits buffer.
///
/// After calling this, use `argmax_q16` on the logits slice to get the
/// predicted next token for greedy decoding.
///
/// # Arguments
/// - `slot`: Model state (must be loaded, caller holds MODEL lock)
/// - `token_id`: Input token ID (0..vocab_size-1)
/// - `pos`: Position in the sequence (0-indexed, must be < INFERENCE_MAX_SEQ)
///
/// # Safety
/// Caller must ensure:
/// - `slot.loaded == true` and all pointers are valid
/// - `slot.inf_buf_addr != 0` (inference state allocated)
/// - `token_id < slot.config.vocab_size`
/// - `pos < INFERENCE_MAX_SEQ`
#[allow(dead_code)]
fn forward_token(slot: &ModelSlot, token_id: usize, pos: usize) {
    // v2: delegate to C float32 forward pass (uses kernel FPU)
    if slot.format_version == MODEL_FORMAT_V2 {
        let weights = (slot.data_addr + slot.weights_off) as *const core::ffi::c_void;
        let weights_len = slot.data_len - slot.weights_off;
        // SAFETY: fpu_state was allocated by hackbot_fpu_alloc. weights points to
        // the FP16 weight data within the model blob. token_id and pos are validated
        // by callers.
        unsafe {
            hackbot_fpu_forward(
                slot.fpu_state as *mut core::ffi::c_void,
                weights, weights_len,
                token_id as i32, pos as i32,
            );
        }
        return;
    }

    // v1: Q16.16 fixed-point forward pass
    let dim = slot.config.dim as usize;
    let hidden_dim = slot.config.hidden_dim as usize;
    let n_layers = slot.config.n_layers as usize;
    let n_heads = slot.config.n_heads as usize;
    let n_kv_heads = slot.config.n_kv_heads as usize;
    let head_dim = slot.config.head_dim as usize;
    let kv_dim = n_kv_heads * head_dim;
    let gs = slot.config.group_size as usize;
    let vocab_size = slot.config.vocab_size as usize;
    let heads_per_group = n_heads / n_kv_heads;

    // Weight data (immutable, from model blob)
    // SAFETY: data_addr is valid for data_len bytes (loaded in Step 3a).
    let data = unsafe {
        core::slice::from_raw_parts(slot.data_addr as *const u8, slot.data_len)
    };

    // Inference buffer base pointer (mutable)
    let inf = slot.inf_buf_addr as *mut i32;

    // Create non-overlapping mutable slices for each activation buffer.
    // SAFETY: All offsets were computed sequentially with no overlap in
    // alloc_inference_state(). KV cache is [0..inf_kv_len), activation
    // buffers start at inf_kv_len and are each at unique offsets.
    let x = unsafe { core::slice::from_raw_parts_mut(inf.add(slot.inf_x), dim) };
    let xb = unsafe { core::slice::from_raw_parts_mut(inf.add(slot.inf_xb), dim) };
    let xb2 = unsafe { core::slice::from_raw_parts_mut(inf.add(slot.inf_xb2), dim) };
    let q_buf = unsafe { core::slice::from_raw_parts_mut(inf.add(slot.inf_q), dim) };
    let k_buf = unsafe { core::slice::from_raw_parts_mut(inf.add(slot.inf_k), kv_dim) };
    let v_buf = unsafe { core::slice::from_raw_parts_mut(inf.add(slot.inf_v), kv_dim) };
    let att = unsafe { core::slice::from_raw_parts_mut(inf.add(slot.inf_att), INFERENCE_MAX_SEQ) };
    let hb = unsafe { core::slice::from_raw_parts_mut(inf.add(slot.inf_hb), hidden_dim) };
    let hb2 = unsafe { core::slice::from_raw_parts_mut(inf.add(slot.inf_hb2), hidden_dim) };
    let logits = unsafe { core::slice::from_raw_parts_mut(inf.add(slot.inf_logits), vocab_size) };

    // KV cache layout strides (in i32 elements)
    let kv_head_stride = INFERENCE_MAX_SEQ * head_dim;       // per KV head
    let kv_type_stride = n_kv_heads * kv_head_stride;        // between K and V sections
    let kv_layer_stride = 2 * kv_type_stride;                // per layer

    // =====================================================================
    // Step 1: Embedding lookup — dequantize embed_tokens[token_id] → x
    // =====================================================================
    let e = &slot.embed;
    let n_groups_e = dim / gs;
    let row_off = token_id * dim;
    let scale_row_off = token_id * n_groups_e * 4; // byte offset for this row's scales

    for g in 0..n_groups_e {
        let sb = e.scale_off + scale_row_off + g * 4;
        let scale = i32::from_le_bytes([data[sb], data[sb+1], data[sb+2], data[sb+3]]);
        for j in 0..gs {
            let c = g * gs + j;
            // INT8 weight × Q16.16 scale = Q16.16 activation
            let w = data[e.data_off + row_off + c] as i8 as i32;
            x[c] = w * scale;
        }
    }

    // DEBUG: Print embedding values for first forward call
    if pos == 0 {
        pr_info!("hackbot: DEBUG embed[{}]: x[0..4] = [{}, {}, {}, {}]\n",
                 token_id, x[0], x[1], x[2], x[3]);
    }

    // =====================================================================
    // Step 2: Transformer layers
    // =====================================================================
    for l in 0..n_layers {
        let layer = &slot.layers[l];

        // ----- 2a: Pre-attention RMSNorm -----
        rmsnorm_q16(xb, x, &data[layer.rms_att_off..], dim);

        // ----- 2b: QKV projections -----
        matmul_q8(q_buf, xb,
                  &data[layer.wq.data_off..], &data[layer.wq.scale_off..],
                  dim, dim, gs);
        matmul_q8(k_buf, xb,
                  &data[layer.wk.data_off..], &data[layer.wk.scale_off..],
                  kv_dim, dim, gs);
        matmul_q8(v_buf, xb,
                  &data[layer.wv.data_off..], &data[layer.wv.scale_off..],
                  kv_dim, dim, gs);

        // ----- 2c: RoPE on Q and K -----
        for h in 0..n_heads {
            rope_apply_q16(&mut q_buf[h * head_dim..(h + 1) * head_dim], pos, head_dim);
        }
        for h in 0..n_kv_heads {
            rope_apply_q16(&mut k_buf[h * head_dim..(h + 1) * head_dim], pos, head_dim);
        }

        // ----- 2d: Store K and V in cache -----
        let kv_base = l * kv_layer_stride;
        for h in 0..n_kv_heads {
            let k_dst = kv_base + h * kv_head_stride + pos * head_dim;
            let v_dst = kv_base + kv_type_stride + h * kv_head_stride + pos * head_dim;
            for d in 0..head_dim {
                // SAFETY: k_dst and v_dst are within KV cache bounds
                // (layer < n_layers, h < n_kv_heads, pos < INFERENCE_MAX_SEQ, d < head_dim).
                unsafe {
                    *inf.add(k_dst + d) = k_buf[h * head_dim + d];
                    *inf.add(v_dst + d) = v_buf[h * head_dim + d];
                }
            }
        }

        // ----- 2e: Multi-head attention with GQA -----
        for h in 0..n_heads {
            let kv_group = h / heads_per_group;
            let q_head = &q_buf[h * head_dim..(h + 1) * head_dim];

            // Compute attention scores for all cached positions
            for p in 0..=pos {
                let k_src = kv_base + kv_group * kv_head_stride + p * head_dim;
                let mut dot: i64 = 0;
                for d in 0..head_dim {
                    // SAFETY: k_src + d is within KV cache bounds.
                    let k_val = unsafe { *inf.add(k_src + d) };
                    dot += q_head[d] as i64 * k_val as i64;
                }
                // Combine Q16.16→Q16.16 conversion (>>16) with /sqrt(64)=8 (>>3) = >>19.
                // This shift is only exact for head_dim=64 (sqrt=8=2^3).
                att[p] = (dot >> 19) as i32;
            }

            // Softmax over attention scores
            softmax_q16(att, pos + 1);

            // Weighted sum of values → xb[h * head_dim .. (h+1) * head_dim]
            let v_type_base = kv_base + kv_type_stride + kv_group * kv_head_stride;
            for d in 0..head_dim {
                let mut acc: i64 = 0;
                for p in 0..=pos {
                    // SAFETY: v_type_base + p * head_dim + d is within KV cache bounds.
                    let v_val = unsafe { *inf.add(v_type_base + p * head_dim + d) };
                    acc += att[p] as i64 * v_val as i64;
                }
                xb[h * head_dim + d] = (acc >> 16) as i32;
            }
        }

        // ----- 2f: Output projection -----
        matmul_q8(xb2, xb,
                  &data[layer.wo.data_off..], &data[layer.wo.scale_off..],
                  dim, dim, gs);

        // ----- 2g: Residual connection -----
        for i in 0..dim {
            x[i] = x[i].wrapping_add(xb2[i]);
        }

        // ----- 2h: Pre-FFN RMSNorm -----
        rmsnorm_q16(xb, x, &data[layer.rms_ffn_off..], dim);

        // ----- 2i: SwiGLU FFN -----
        // gate = Wgate × xb, up = Wup × xb
        matmul_q8(hb, xb,
                  &data[layer.gate.data_off..], &data[layer.gate.scale_off..],
                  hidden_dim, dim, gs);
        matmul_q8(hb2, xb,
                  &data[layer.up.data_off..], &data[layer.up.scale_off..],
                  hidden_dim, dim, gs);

        // hb = silu(gate) * up
        silu_vec_q16(hb, hidden_dim);
        elementwise_mul_inplace_q16(hb, hb2, hidden_dim);

        // down projection: xb2 = Wdown × hb
        matmul_q8(xb2, hb,
                  &data[layer.down.data_off..], &data[layer.down.scale_off..],
                  dim, hidden_dim, gs);

        // ----- 2j: Residual connection -----
        for i in 0..dim {
            x[i] = x[i].wrapping_add(xb2[i]);
        }
    }

    // DEBUG: Print final x values (only at pos 0)
    if pos == 0 {
        pr_info!("hackbot: DEBUG after layers: x[0..4] = [{}, {}, {}, {}]\n",
                 x[0], x[1], x[2], x[3]);
    }

    // =====================================================================
    // Step 3: Final RMSNorm
    // =====================================================================
    rmsnorm_q16(xb, x, &data[slot.rms_final_off..], dim);

    // =====================================================================
    // Step 4: Logits — matmul with tied embedding weights
    // =====================================================================
    matmul_q8(logits, xb,
              &data[slot.embed.data_off..], &data[slot.embed.scale_off..],
              vocab_size, dim, gs);

    // DEBUG: Print top-1 logit (only at pos 0)
    if pos == 0 {
        let mut best_i = 0usize;
        for i in 1..vocab_size {
            if logits[i] > logits[best_i] {
                best_i = i;
            }
        }
        pr_info!("hackbot: DEBUG logits[pos=0]: top1 = token {} (logit {})\n",
                 best_i, logits[best_i]);
    }
}

// ---------------------------------------------------------------------------
// Step 3d: BPE Tokenizer + Text Generation
//
// BPE (Byte Pair Encoding) tokenizer for SmolLM2's GPT-2-based tokenizer.
// SmolLM2 uses GPT-2 byte encoding (bytes_to_unicode mapping), NOT SentencePiece.
//
// GPT-2 byte encoding: maps each raw byte to a Unicode codepoint, then stores
// tokens as UTF-8 strings of those codepoints. Printable ASCII (33-126) maps
// to itself; control chars, space, DEL, and 0x80-0xA0 map to U+0100+.
//
// Encoding: GPT-2 byte transform → BPE merge of highest-score adjacent pairs.
// Decoding: token bytes → GPT-2 reverse mapping → raw bytes.
// Generation: autoregressive forward_token loop with greedy argmax decoding.
// ---------------------------------------------------------------------------

/// GPT-2 byte→Unicode codepoint mapping table.
/// Maps each raw byte (0-255) to the codepoint used by the GPT-2 tokenizer.
/// Printable ASCII (33-126) maps to itself. Other bytes map to U+0100+.
/// Generated from OpenAI's bytes_to_unicode() function.
#[allow(dead_code)]
const GPT2_BYTE_TO_CODEPOINT: [u16; 256] = [
    256, 257, 258, 259, 260, 261, 262, 263, 264, 265, 266, 267, 268, 269, 270, 271, // 0x00-0x0F
    272, 273, 274, 275, 276, 277, 278, 279, 280, 281, 282, 283, 284, 285, 286, 287, // 0x10-0x1F
    288,  33,  34,  35,  36,  37,  38,  39,  40,  41,  42,  43,  44,  45,  46,  47, // 0x20-0x2F
     48,  49,  50,  51,  52,  53,  54,  55,  56,  57,  58,  59,  60,  61,  62,  63, // 0x30-0x3F
     64,  65,  66,  67,  68,  69,  70,  71,  72,  73,  74,  75,  76,  77,  78,  79, // 0x40-0x4F
     80,  81,  82,  83,  84,  85,  86,  87,  88,  89,  90,  91,  92,  93,  94,  95, // 0x50-0x5F
     96,  97,  98,  99, 100, 101, 102, 103, 104, 105, 106, 107, 108, 109, 110, 111, // 0x60-0x6F
    112, 113, 114, 115, 116, 117, 118, 119, 120, 121, 122, 123, 124, 125, 126, 289, // 0x70-0x7F
    290, 291, 292, 293, 294, 295, 296, 297, 298, 299, 300, 301, 302, 303, 304, 305, // 0x80-0x8F
    306, 307, 308, 309, 310, 311, 312, 313, 314, 315, 316, 317, 318, 319, 320, 321, // 0x90-0x9F
    322, 161, 162, 163, 164, 165, 166, 167, 168, 169, 170, 171, 172, 323, 174, 175, // 0xA0-0xAF
    176, 177, 178, 179, 180, 181, 182, 183, 184, 185, 186, 187, 188, 189, 190, 191, // 0xB0-0xBF
    192, 193, 194, 195, 196, 197, 198, 199, 200, 201, 202, 203, 204, 205, 206, 207, // 0xC0-0xCF
    208, 209, 210, 211, 212, 213, 214, 215, 216, 217, 218, 219, 220, 221, 222, 223, // 0xD0-0xDF
    224, 225, 226, 227, 228, 229, 230, 231, 232, 233, 234, 235, 236, 237, 238, 239, // 0xE0-0xEF
    240, 241, 242, 243, 244, 245, 246, 247, 248, 249, 250, 251, 252, 253, 254, 255, // 0xF0-0xFF
];

/// GPT-2 Unicode codepoint→raw byte reverse mapping.
/// Size = 324 (max codepoint is 323). Index = codepoint, value = raw byte.
/// Built as the inverse of GPT2_BYTE_TO_CODEPOINT.
#[allow(dead_code)]
const GPT2_CODEPOINT_TO_BYTE: [u8; 324] = {
    let mut table = [0u8; 324];
    let mut b: u16 = 0;
    while b < 256 {
        table[GPT2_BYTE_TO_CODEPOINT[b as usize] as usize] = b as u8;
        b += 1;
    }
    table
};

/// Decode a token ID to its GPT-2 encoded byte representation.
/// Returns a slice into the model data blob (zero-copy).
/// The returned bytes are GPT-2 encoded — use gpt2_decode_token for raw bytes.
///
/// # Safety
/// Caller must ensure token_id < vocab_size and slot data pointers are valid.
#[allow(dead_code)]
fn decode_token_bytes<'a>(data: &'a [u8], tok_offsets: *const u32, token_id: usize) -> &'a [u8] {
    // SAFETY: tok_offsets[token_id] was validated during parse_and_store_model.
    let off = unsafe { *tok_offsets.add(token_id) } as usize;
    // Token record: [i32 score][u16 len][u8; len]
    // Skip score (4 bytes), read len (2 bytes)
    let len = u16::from_le_bytes([data[off + 4], data[off + 5]]) as usize;
    &data[off + 6..off + 6 + len]
}

/// Get the BPE merge score for a token. Higher score = merge earlier.
/// Scores are integer merge priorities (n_merges - merge_index).
#[allow(dead_code)]
fn get_token_score(data: &[u8], tok_offsets: *const u32, token_id: usize) -> i32 {
    let off = unsafe { *tok_offsets.add(token_id) } as usize;
    i32::from_le_bytes([data[off], data[off + 1], data[off + 2], data[off + 3]])
}

/// Decode GPT-2 token bytes to raw bytes.
/// Parses UTF-8 codepoints from the GPT-2 encoded token bytes and maps each
/// back to the original raw byte value via GPT2_CODEPOINT_TO_BYTE.
/// Returns the number of raw bytes written to `out`.
#[allow(dead_code)]
fn gpt2_decode_token(token_bytes: &[u8], out: &mut [u8]) -> usize {
    let mut i = 0usize; // input cursor
    let mut o = 0usize; // output cursor
    while i < token_bytes.len() && o < out.len() {
        let b = token_bytes[i];
        if b < 0x80 {
            // 1-byte UTF-8: codepoint = b (range 0-127)
            if (b as usize) < GPT2_CODEPOINT_TO_BYTE.len() {
                out[o] = GPT2_CODEPOINT_TO_BYTE[b as usize];
            } else {
                out[o] = b'?';
            }
            o += 1;
            i += 1;
        } else if b >= 0xC0 && b < 0xE0 && i + 1 < token_bytes.len() {
            // 2-byte UTF-8: codepoint = ((b & 0x1F) << 6) | (next & 0x3F)
            let cp = ((b as u16 & 0x1F) << 6) | (token_bytes[i + 1] as u16 & 0x3F);
            if (cp as usize) < GPT2_CODEPOINT_TO_BYTE.len() {
                out[o] = GPT2_CODEPOINT_TO_BYTE[cp as usize];
            } else {
                out[o] = b'?';
            }
            o += 1;
            i += 2;
        } else {
            // 3+ byte UTF-8 or invalid: output '?' and skip
            out[o] = b'?';
            o += 1;
            i += 1;
        }
    }
    o
}

/// Lexicographic comparison of two tokens by their byte representations.
/// Used as the ordering function for heapsort and binary search.
fn tok_bytes_cmp(data: &[u8], tok_offsets: *const u32, a: u32, b: u32) -> core::cmp::Ordering {
    let bytes_a = decode_token_bytes(data, tok_offsets, a as usize);
    let bytes_b = decode_token_bytes(data, tok_offsets, b as usize);
    bytes_a.cmp(bytes_b)
}

/// Sift-down operation for max-heap used by heapsort.
/// Maintains the max-heap property: parent >= children (by token byte order).
fn heapsort_sift_down(
    arr: &mut [u32], data: &[u8], tok_offsets: *const u32,
    start: usize, end: usize,
) {
    let mut root = start;
    loop {
        let left = 2 * root + 1;
        if left >= end {
            break;
        }
        let right = left + 1;
        let mut largest = root;

        if tok_bytes_cmp(data, tok_offsets, arr[left], arr[largest])
            == core::cmp::Ordering::Greater
        {
            largest = left;
        }
        if right < end
            && tok_bytes_cmp(data, tok_offsets, arr[right], arr[largest])
                == core::cmp::Ordering::Greater
        {
            largest = right;
        }
        if largest == root {
            break;
        }
        arr.swap(root, largest);
        root = largest;
    }
}

/// In-place heapsort of token IDs by their byte representations.
/// O(V log V) time, O(1) extra space.
fn heapsort_vocab(arr: &mut [u32], data: &[u8], tok_offsets: *const u32) {
    let n = arr.len();
    if n <= 1 {
        return;
    }

    // Build max-heap (bottom-up)
    let mut i = n / 2;
    while i > 0 {
        i -= 1;
        heapsort_sift_down(arr, data, tok_offsets, i, n);
    }

    // Extract elements from heap
    let mut end = n;
    while end > 1 {
        end -= 1;
        arr.swap(0, end);
        heapsort_sift_down(arr, data, tok_offsets, 0, end);
    }
}

/// Binary search the sorted vocabulary for a token matching the given bytes.
/// Returns Some(token_id) if found, None otherwise.
/// O(log V) time where V = vocab_size.
#[allow(dead_code)]
fn find_token_by_bytes(
    data: &[u8], tok_offsets: *const u32, sorted: *const u32,
    vocab_size: usize, query: &[u8],
) -> Option<u32> {
    let mut lo = 0usize;
    let mut hi = vocab_size;
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        // SAFETY: sorted is a valid [u32; vocab_size] array, mid < vocab_size.
        let mid_id = unsafe { *sorted.add(mid) };
        let mid_bytes = decode_token_bytes(data, tok_offsets, mid_id as usize);
        match query.cmp(mid_bytes) {
            core::cmp::Ordering::Equal => return Some(mid_id),
            core::cmp::Ordering::Less => hi = mid,
            core::cmp::Ordering::Greater => lo = mid + 1,
        }
    }
    None
}

/// Build the sorted vocabulary index and byte-to-token lookup table.
/// Called once after model is loaded and inference state is allocated.
///
/// The byte_to_token table maps each raw byte (0-255) to its GPT-2 byte-level
/// token ID by computing the GPT-2 codepoint, encoding it as UTF-8, and
/// searching for the matching token in the sorted vocabulary.
fn build_sorted_vocab(slot: &mut ModelSlot) -> Result {
    let vocab_size = slot.config.vocab_size as usize;
    let data = unsafe {
        core::slice::from_raw_parts(slot.data_addr as *const u8, slot.data_len)
    };
    let tok_offsets = slot.tok_offsets_addr as *const u32;

    // Allocate sorted vocab array: [u32; vocab_size]
    let alloc_size = vocab_size * core::mem::size_of::<u32>();
    // SAFETY: kvrealloc with null acts as kvmalloc. Process context, GFP_KERNEL safe.
    let ptr = unsafe {
        bindings::kvrealloc_node_align_noprof(
            core::ptr::null(),
            alloc_size,
            core::mem::align_of::<u32>() as _,
            bindings::GFP_KERNEL,
            bindings::NUMA_NO_NODE,
        )
    } as *mut u32;

    if ptr.is_null() {
        pr_err!("hackbot: failed to allocate sorted vocab ({} bytes)\n", alloc_size);
        return Err(ENOMEM);
    }

    // Initialize: sorted[i] = i
    // SAFETY: ptr is valid for vocab_size u32 elements.
    let sorted = unsafe { core::slice::from_raw_parts_mut(ptr, vocab_size) };
    for i in 0..vocab_size {
        sorted[i] = i as u32;
    }

    // Sort by token byte content (heapsort, O(V log V))
    heapsort_vocab(sorted, data, tok_offsets);

    slot.sorted_vocab_addr = ptr as usize;

    // Build byte-to-token lookup using GPT-2 byte encoding.
    // For each raw byte b: compute GPT-2 codepoint → UTF-8 → find in sorted vocab.
    slot.byte_to_token = [TOKEN_ENDOFTEXT; 256]; // default: endoftext (should never be needed)
    let mut n_found = 0u32;

    for b in 0u16..256 {
        let cp = GPT2_BYTE_TO_CODEPOINT[b as usize];
        let mut utf8 = [0u8; 2];
        let utf8_len;
        if cp < 128 {
            utf8[0] = cp as u8;
            utf8_len = 1;
        } else {
            utf8[0] = 0xC0 | ((cp >> 6) as u8);
            utf8[1] = 0x80 | ((cp & 0x3F) as u8);
            utf8_len = 2;
        }

        if let Some(tid) = find_token_by_bytes(data, tok_offsets, ptr, vocab_size, &utf8[..utf8_len]) {
            slot.byte_to_token[b as usize] = tid;
            n_found += 1;
        }
    }

    pr_info!("hackbot: tokenizer ready: sorted vocab + {}/256 byte tokens\n", n_found);
    // Debug: print byte_to_token for key bytes
    pr_info!("hackbot: byte_to_token[0x0A](nl)={}, [0x20](sp)={}, [0x68](h)={}, [0x59](Y)={}\n",
             slot.byte_to_token[0x0A], slot.byte_to_token[0x20],
             slot.byte_to_token[0x68], slot.byte_to_token[0x59]);
    Ok(())
}

/// Preprocess raw input bytes for GPT-2 BPE encoding.
/// Converts each raw byte to its GPT-2 Unicode codepoint encoded as UTF-8.
/// Printable ASCII (33-126) stays as-is (1 byte). Other bytes expand to
/// 2-byte UTF-8 (space → [0xC4, 0xA0], newline → [0xC4, 0x8A], etc.).
/// Writes preprocessed bytes into `out`, returns the number of bytes written.
#[allow(dead_code)]
fn preprocess_gpt2(input: &[u8], out: &mut [u8]) -> usize {
    let mut pos = 0usize;
    for &b in input {
        let cp = GPT2_BYTE_TO_CODEPOINT[b as usize];
        if cp < 128 {
            if pos >= out.len() {
                break;
            }
            out[pos] = cp as u8;
            pos += 1;
        } else {
            if pos + 1 >= out.len() {
                break;
            }
            out[pos] = 0xC0 | ((cp >> 6) as u8);
            out[pos + 1] = 0x80 | ((cp & 0x3F) as u8);
            pos += 2;
        }
    }
    pos
}

/// Encode a byte string into BPE token IDs using the model's vocabulary.
///
/// Algorithm:
/// 1. Preprocess: GPT-2 byte encoding (raw bytes → GPT-2 Unicode UTF-8)
/// 2. Initialize: each preprocessed byte maps to its byte-level token
/// 3. Merge loop: repeatedly merge the adjacent pair with the highest BPE score
///
/// Returns the number of tokens written to `out_tokens`.
/// O(n^2 log V) where n = input length, V = vocab_size.
#[allow(dead_code)]
fn encode_bpe(slot: &ModelSlot, input: &[u8], out_tokens: &mut [u32]) -> usize {
    if input.is_empty() || out_tokens.is_empty() {
        return 0;
    }

    let data = unsafe {
        core::slice::from_raw_parts(slot.data_addr as *const u8, slot.data_len)
    };
    let tok_offsets = slot.tok_offsets_addr as *const u32;
    let sorted = slot.sorted_vocab_addr as *const u32;
    let vocab_size = slot.config.vocab_size as usize;

    // Preprocess: GPT-2 encode the input into a scratch buffer.
    let mut preproc_buf = [0u8; MAX_PREPROC_INPUT];
    let preproc = &mut preproc_buf[..];
    let preproc_len = preprocess_gpt2(input, preproc);
    let preproc = &preproc[..preproc_len];

    // Initialize: map each preprocessed byte to its token via byte_to_token.
    // For GPT-2 encoding, single-byte GPT-2 chars (printable ASCII) map directly
    // via byte_to_token. Multi-byte chars (2-byte UTF-8 for non-printable) need
    // lookup via find_token_by_bytes since byte_to_token only handles raw bytes.
    let mut len = 0usize;
    let mut pi = 0usize;
    while pi < preproc_len && len < out_tokens.len() {
        let b = preproc[pi];
        if b < 0x80 {
            // 1-byte UTF-8: this is a printable ASCII char, codepoint == byte value.
            // The corresponding raw byte has the same codepoint, so we can look up
            // by finding which raw byte maps to this codepoint.
            // For codepoints 33-126, the raw byte is the same value.
            // For codepoint < 33 or > 126, this shouldn't happen (they'd be 2-byte UTF-8).
            out_tokens[len] = slot.byte_to_token[b as usize];
            len += 1;
            pi += 1;
        } else if b >= 0xC0 && b < 0xE0 && pi + 1 < preproc_len {
            // 2-byte UTF-8: find this char's token via binary search
            if let Some(tid) = find_token_by_bytes(data, tok_offsets, sorted, vocab_size, &preproc[pi..pi + 2]) {
                out_tokens[len] = tid;
            } else {
                out_tokens[len] = TOKEN_ENDOFTEXT; // fallback (shouldn't happen)
            }
            len += 1;
            pi += 2;
        } else {
            // Skip invalid bytes (shouldn't happen with GPT-2 encoding)
            pi += 1;
        }
    }

    // BPE merge loop: repeatedly merge the highest-scoring adjacent pair
    let mut concat_buf = [0u8; 128]; // temporary buffer for concatenated token bytes
    loop {
        if len < 2 {
            break;
        }

        let mut best_score = i32::MIN;
        let mut best_idx = 0usize;
        let mut best_token = 0u32;
        let mut found = false;

        for i in 0..len - 1 {
            let bytes_a = decode_token_bytes(data, tok_offsets, out_tokens[i] as usize);
            let bytes_b = decode_token_bytes(data, tok_offsets, out_tokens[i + 1] as usize);

            let total = bytes_a.len() + bytes_b.len();
            if total > 128 {
                continue;
            }

            concat_buf[..bytes_a.len()].copy_from_slice(bytes_a);
            concat_buf[bytes_a.len()..total].copy_from_slice(bytes_b);

            if let Some(merged_id) =
                find_token_by_bytes(data, tok_offsets, sorted, vocab_size, &concat_buf[..total])
            {
                let score = get_token_score(data, tok_offsets, merged_id as usize);
                if score > best_score {
                    best_score = score;
                    best_idx = i;
                    best_token = merged_id;
                    found = true;
                }
            }
        }

        if !found {
            break;
        }

        // Merge: replace pair at best_idx with the merged token
        out_tokens[best_idx] = best_token;
        let mut i = best_idx + 1;
        while i < len - 1 {
            out_tokens[i] = out_tokens[i + 1];
            i += 1;
        }
        len -= 1;
    }

    len
}

/// Get the next token prediction from logits buffer (argmax).
/// Reads logits directly via raw pointer to avoid slice aliasing concerns.
#[allow(dead_code)]
fn get_next_token(slot: &ModelSlot) -> usize {
    // v2: argmax over float32 logits via C helper (uses kernel FPU)
    if slot.format_version == MODEL_FORMAT_V2 {
        // SAFETY: fpu_state was allocated by hackbot_fpu_alloc and contains
        // valid float32 logits from the last hackbot_fpu_forward call.
        let tok = unsafe {
            hackbot_fpu_get_next_token(slot.fpu_state as *const core::ffi::c_void)
        };
        return tok as usize;
    }
    // v1: argmax over Q16.16 logits
    let logits_ptr = (slot.inf_buf_addr as *const i32).wrapping_add(slot.inf_logits);
    let vocab_size = slot.config.vocab_size as usize;
    let mut best = 0usize;
    for i in 1..vocab_size {
        // SAFETY: logits_ptr + i is within the inf_buf allocation (logits region).
        if unsafe { *logits_ptr.add(i) > *logits_ptr.add(best) } {
            best = i;
        }
    }
    best
}

/// Generate text from a pre-built token array using the in-kernel model.
///
/// This is the core inference function: runs prefill on prompt_tokens,
/// then autoregressively generates new tokens until a stop condition.
/// Output tokens are decoded from GPT-2 encoding to raw bytes.
///
/// Stops on <|endoftext|>, <|im_end|>, max sequence length, or output buffer full.
/// Returns the number of raw bytes written to `output`.
#[allow(dead_code)]
fn generate_from_tokens(
    slot: &ModelSlot,
    prompt_tokens: &[u32],
    n_prompt: usize,
    output: &mut [u8],
    max_new_tokens: usize,
) -> usize {
    if n_prompt == 0 || n_prompt > INFERENCE_MAX_SEQ {
        return 0;
    }

    let data = unsafe {
        core::slice::from_raw_parts(slot.data_addr as *const u8, slot.data_len)
    };
    let tok_offsets = slot.tok_offsets_addr as *const u32;

    // Reset KV cache for new sequence
    reset_kv_cache(slot);

    // Debug: print first 10 prompt tokens
    let debug_n = if n_prompt < 10 { n_prompt } else { 10 };
    for i in 0..debug_n {
        pr_info!("hackbot: prompt[{}] = {}\n", i, prompt_tokens[i]);
    }
    if n_prompt > 10 {
        pr_info!("hackbot: ... ({} more prompt tokens)\n", n_prompt - 10);
    }

    // Prefill: forward all prompt tokens through the transformer
    for i in 0..n_prompt {
        forward_token(slot, prompt_tokens[i] as usize, i);
    }

    // Debug: print logits after prefill (top-3) — v1 only (v2 logits are in C state)
    if slot.format_version != MODEL_FORMAT_V2 && slot.inf_buf_addr != 0 {
        let logits_ptr = (slot.inf_buf_addr as *const i32).wrapping_add(slot.inf_logits);
        let vs = slot.config.vocab_size as usize;
        let mut best = [0usize; 3];
        for i in 0..vs {
            let v = unsafe { *logits_ptr.add(i) };
            for b in 0..3 {
                if v > unsafe { *logits_ptr.add(best[b]) } {
                    // shift down
                    let mut j = 2;
                    while j > b { best[j] = best[j-1]; j -= 1; }
                    best[b] = i;
                    break;
                }
            }
        }
        for b in 0..3 {
            let v = unsafe { *logits_ptr.add(best[b]) };
            pr_info!("hackbot: prefill logit top-{}: token {} logit {}\n", b+1, best[b], v);
        }
    }

    // Autoregressive generation
    let mut pos = n_prompt;
    let mut out_len = 0usize;
    let mut next_token = get_next_token(slot);
    let gen_limit = max_new_tokens.min(INFERENCE_MAX_SEQ.saturating_sub(pos));

    pr_info!("hackbot: gen start: first_token={}, gen_limit={}, pos={}\n",
             next_token, gen_limit, pos);

    // Temporary buffer for GPT-2 → raw byte decoding (per token)
    let mut decode_buf = [0u8; 64];
    let mut gen_count = 0usize;

    for _ in 0..gen_limit {
        let tok = next_token as u32;
        // Stop on end-of-text or end-of-message
        if tok == TOKEN_ENDOFTEXT || tok == TOKEN_IM_END {
            pr_info!("hackbot: gen stop: token {} (EOS/IM_END) at pos {}\n", tok, pos);
            break;
        }
        if pos >= INFERENCE_MAX_SEQ {
            pr_info!("hackbot: gen stop: max seq at pos {}\n", pos);
            break;
        }

        // Debug: print first 20 generated tokens
        if gen_count < 20 {
            pr_info!("hackbot: gen[{}]: token {} at pos {}\n", gen_count, next_token, pos);
        }
        gen_count += 1;

        // Decode token: GPT-2 bytes → raw bytes
        let tok_bytes = decode_token_bytes(data, tok_offsets, next_token);
        let raw_len = gpt2_decode_token(tok_bytes, &mut decode_buf);
        let copy_len = raw_len.min(output.len().saturating_sub(out_len));
        if copy_len == 0 {
            break; // output buffer full
        }
        output[out_len..out_len + copy_len].copy_from_slice(&decode_buf[..copy_len]);
        out_len += copy_len;

        // Forward this token to get next prediction
        forward_token(slot, next_token, pos);
        pos += 1;
        next_token = get_next_token(slot);
    }

    out_len
}

/// Generate text from a raw text prompt using the in-kernel model.
/// Encodes the prompt with BPE, then calls generate_from_tokens.
/// This is a convenience wrapper for simple text-in → text-out generation.
#[allow(dead_code)]
fn generate(
    slot: &ModelSlot, prompt: &[u8], output: &mut [u8], max_new_tokens: usize,
) -> usize {
    // Encode prompt into tokens (stack buffer, 2 KB)
    let mut prompt_tokens = [0u32; 512];
    let n_encoded = encode_bpe(slot, prompt, &mut prompt_tokens);
    let n_prompt = n_encoded.min(INFERENCE_MAX_SEQ.saturating_sub(max_new_tokens));

    if n_prompt == 0 {
        return 0;
    }

    generate_from_tokens(slot, &prompt_tokens, n_prompt, output, max_new_tokens)
}

// ---------------------------------------------------------------------------
// Step 3e: Agent integration — local inference backend
//
// Wires generate_from_tokens() into the OODA agent loop as an alternative
// to the remote vLLM inference backend. Uses ChatML format with special
// token IDs inserted directly (not via BPE).
//
// Backend selection: auto-detect (local if model loaded, else vLLM).
// ---------------------------------------------------------------------------

/// Append a ChatML message to the token array.
/// Format: <|im_start|>{role}\n{content}<|im_end|>\n
/// Returns the new position in the token array.
#[allow(dead_code)]
fn append_chat_tokens(
    slot: &ModelSlot,
    tokens: &mut [u32],
    pos: usize,
    role: &[u8],
    content: &[u8],
) -> usize {
    let mut p = pos;
    let max = tokens.len();
    let nl_token = slot.byte_to_token[0x0A]; // newline byte → GPT-2 newline token

    // <|im_start|>
    if p < max { tokens[p] = TOKEN_IM_START; p += 1; }

    // BPE-encode role (e.g., "system", "user", "assistant")
    if p < max {
        let n = encode_bpe(slot, role, &mut tokens[p..]);
        p += n;
    }

    // \n (newline token)
    if p < max { tokens[p] = nl_token; p += 1; }

    // BPE-encode content
    if p < max {
        let n = encode_bpe(slot, content, &mut tokens[p..]);
        p += n;
    }

    // <|im_end|>
    if p < max { tokens[p] = TOKEN_IM_END; p += 1; }

    // \n (newline token)
    if p < max { tokens[p] = nl_token; p += 1; }

    p
}

/// Begin an assistant turn in ChatML format.
/// Appends: <|im_start|>assistant\n
/// Returns the new position.
#[allow(dead_code)]
fn begin_assistant_turn(
    slot: &ModelSlot,
    tokens: &mut [u32],
    pos: usize,
) -> usize {
    let mut p = pos;
    let max = tokens.len();
    let nl_token = slot.byte_to_token[0x0A];

    if p < max { tokens[p] = TOKEN_IM_START; p += 1; }
    if p < max {
        let n = encode_bpe(slot, b"assistant", &mut tokens[p..]);
        p += n;
    }
    if p < max { tokens[p] = nl_token; p += 1; }

    p
}

/// Local inference OODA agent loop.
/// Uses the in-kernel 135M model with ChatML format.
/// Supports tool calls with limited context (256 tokens).
#[allow(dead_code)]
fn agent_loop_local(prompt: &[u8]) -> Result<KVVec<u8>> {
    let slot = MODEL.lock();
    if !slot.loaded {
        return Err(ENODEV);
    }

    let _data = unsafe {
        core::slice::from_raw_parts(slot.data_addr as *const u8, slot.data_len)
    };
    let _tok_offsets = slot.tok_offsets_addr as *const u32;

    // DEBUG: Quick single-token sanity test
    // Forward token 1 (<|im_start|>) at position 0 and check top prediction
    {
        reset_kv_cache(&slot);
        forward_token(&slot, 1, 0); // token 1 = <|im_start|>
        let top1 = get_next_token(&slot);
        pr_info!("hackbot: DEBUG single-token test: after token 1, top1 = {}\n", top1);
        if slot.format_version != MODEL_FORMAT_V2 && slot.inf_buf_addr != 0 {
            let logits_ptr = (slot.inf_buf_addr as *const i32).wrapping_add(slot.inf_logits);
            let top1_logit = unsafe { *logits_ptr.add(top1) };
            pr_info!("hackbot: DEBUG top1 logit = {}\n", top1_logit);
            let logit_28 = unsafe { *logits_ptr.add(28) };
            let logit_198 = unsafe { *logits_ptr.add(198) };
            pr_info!("hackbot: DEBUG logits: token28={}, token198={}\n", logit_28, logit_198);
        }
    }

    // Build initial ChatML tokens:
    // <|im_start|>system\n{LOCAL_SYSTEM_PROMPT}<|im_end|>\n
    // <|im_start|>user\n{prompt}<|im_end|>\n
    // <|im_start|>assistant\n
    let mut tokens = [0u32; INFERENCE_MAX_SEQ];
    let mut n_tokens = 0usize;

    n_tokens = append_chat_tokens(&slot, &mut tokens, n_tokens, b"system", LOCAL_SYSTEM_PROMPT);
    n_tokens = append_chat_tokens(&slot, &mut tokens, n_tokens, b"user", prompt);
    n_tokens = begin_assistant_turn(&slot, &mut tokens, n_tokens);

    pr_info!("hackbot: local inference: {} prompt tokens\n", n_tokens);

    let mut final_answer = KVVec::new();
    let mut got_final_answer = false;

    for iteration in 0..LOCAL_MAX_ITERATIONS {
        // Ensure we leave room for generation
        let gen_budget = INFERENCE_MAX_SEQ.saturating_sub(n_tokens);
        if gen_budget < 4 {
            pr_warn!("hackbot: local: context full at iteration {}\n", iteration + 1);
            break;
        }
        let gen_tokens = gen_budget.min(MAX_GEN_TOKENS);

        pr_info!("hackbot: local agent iteration {}/{} ({} tokens, {} gen budget)\n",
                 iteration + 1, LOCAL_MAX_ITERATIONS, n_tokens, gen_tokens);

        // Generate response
        let mut response_buf = [0u8; 2048];
        let resp_len = generate_from_tokens(
            &slot, &tokens, n_tokens, &mut response_buf, gen_tokens,
        );
        let response = &response_buf[..resp_len];

        if resp_len == 0 {
            pr_warn!("hackbot: local: empty generation at iteration {}\n", iteration + 1);
            break;
        }

        // Parse for tool call
        match parse_tool_call(response) {
            ToolCallResult::FinalAnswer(text) => {
                pr_info!("hackbot: local: final answer at iteration {}\n", iteration + 1);
                let _ = final_answer.extend_from_slice(text, GFP_KERNEL);
                got_final_answer = true;
                break;
            }
            ToolCallResult::ToolCall { name, prefix } => {
                pr_info!("hackbot: local: tool call '{}' at iteration {}\n",
                         core::str::from_utf8(name).unwrap_or("?"), iteration + 1);

                // Accumulate prefix text
                let _ = final_answer.extend_from_slice(prefix, GFP_KERNEL);

                // Last iteration: execute tool and return raw output
                if iteration == LOCAL_MAX_ITERATIONS - 1 {
                    let tool_output = execute_tool(name);
                    let _ = final_answer.extend_from_slice(b"\n\n", GFP_KERNEL);
                    let _ = final_answer.extend_from_slice(&tool_output, GFP_KERNEL);
                    got_final_answer = true;
                    break;
                }

                // Execute tool
                let tool_output = execute_tool(name);

                // Build the assistant's response + tool result as new messages.
                // We need to re-encode the entire conversation for the next iteration.
                // Append: {prefix}<tool>{name}</tool><|im_end|>\n
                //         <|im_start|>user\n[Tool: {name}]\n{output}\n[End Tool]<|im_end|>\n
                //         <|im_start|>assistant\n

                // Build assistant content: prefix + tool call tag
                let mut assistant_content = KVVec::new();
                let _ = assistant_content.extend_from_slice(prefix, GFP_KERNEL);
                let _ = assistant_content.extend_from_slice(b"<tool>", GFP_KERNEL);
                let _ = assistant_content.extend_from_slice(name, GFP_KERNEL);
                let _ = assistant_content.extend_from_slice(b"</tool>", GFP_KERNEL);

                // Build tool result content
                let mut tool_content = KVVec::new();
                let _ = tool_content.extend_from_slice(b"[Tool: ", GFP_KERNEL);
                let _ = tool_content.extend_from_slice(name, GFP_KERNEL);
                let _ = tool_content.extend_from_slice(b"]\n", GFP_KERNEL);
                // Truncate tool output to fit context
                let tool_slice = if tool_output.len() > LOCAL_MAX_TOOL_OUTPUT {
                    &tool_output[..LOCAL_MAX_TOOL_OUTPUT]
                } else {
                    &tool_output
                };
                let _ = tool_content.extend_from_slice(tool_slice, GFP_KERNEL);
                let _ = tool_content.extend_from_slice(b"\n[End Tool]", GFP_KERNEL);

                // Re-build the full token sequence from scratch for next iteration.
                // This is O(n) re-encoding but avoids KV cache continuation complexity.
                n_tokens = 0;
                n_tokens = append_chat_tokens(&slot, &mut tokens, n_tokens,
                                               b"system", LOCAL_SYSTEM_PROMPT);
                n_tokens = append_chat_tokens(&slot, &mut tokens, n_tokens,
                                               b"user", prompt);
                n_tokens = append_chat_tokens(&slot, &mut tokens, n_tokens,
                                               b"assistant", &assistant_content);
                n_tokens = append_chat_tokens(&slot, &mut tokens, n_tokens,
                                               b"user", &tool_content);
                n_tokens = begin_assistant_turn(&slot, &mut tokens, n_tokens);

                pr_info!("hackbot: local: rebuilt conversation: {} tokens\n", n_tokens);
            }
        }
    }

    if !got_final_answer && final_answer.is_empty() {
        let _ = final_answer.extend_from_slice(
            b"[hackbot] Local inference produced no output.\n", GFP_KERNEL,
        );
    }

    Ok(final_answer)
}

// ---------------------------------------------------------------------------
// Module registration
// ---------------------------------------------------------------------------

#[pin_data(PinnedDrop)]
struct HackbotModule {
    #[pin]
    _miscdev: MiscDeviceRegistration<HackbotDev>,
}

impl kernel::InPlaceModule for HackbotModule {
    fn init(_module: &'static ThisModule) -> impl PinInit<Self, Error> {
        // SAFETY: Called exactly once during module load, before any device
        // operations can occur.
        unsafe { RESPONSE.init() };
        // SAFETY: Called exactly once during module load, before any device access.
        unsafe { MODEL.init() };

        pr_info!("hackbot: loading module, creating /dev/hackbot\n");
        pr_info!(
            "hackbot: vLLM endpoint = {}.{}.{}.{}:{}\n",
            (VLLM_ADDR >> 24) & 0xFF,
            (VLLM_ADDR >> 16) & 0xFF,
            (VLLM_ADDR >> 8) & 0xFF,
            VLLM_ADDR & 0xFF,
            VLLM_PORT,
        );

        let options = MiscDeviceOptions {
            name: c_str!("hackbot"),
        };

        try_pin_init!(Self {
            _miscdev <- MiscDeviceRegistration::register(options),
        })
    }
}

#[pinned_drop]
impl PinnedDrop for HackbotModule {
    fn drop(self: Pin<&mut Self>) {
        free_model_resources();
        pr_info!("hackbot: unloading module\n");
    }
}

// ---------------------------------------------------------------------------
// Per-fd device state (lightweight — just holds the device reference)
// ---------------------------------------------------------------------------

/// Per-file-descriptor device state.
/// The actual response data lives in the global RESPONSE mutex.
#[pin_data(PinnedDrop)]
struct HackbotDev {
    dev: ARef<Device>,
}

// ---------------------------------------------------------------------------
// Kernel socket wrapper (FFI to C socket API)
// ---------------------------------------------------------------------------

/// IPv4 socket address — not generated by bindgen for out-of-tree modules.
#[repr(C)]
struct SockaddrIn {
    sin_family: u16,
    sin_port: u16,
    sin_addr: u32,
    __pad: [u8; 8],
}

/// RAII wrapper around a kernel socket. Calls `sock_release` on drop.
struct KernelSocket {
    sock: *mut bindings::socket,
}

impl KernelSocket {
    /// Create a TCP socket and connect to the given IPv4 address and port.
    /// Both `addr` and `port` are in host byte order — this function converts.
    fn connect_tcp(addr: u32, port: u16) -> Result<Self> {
        let mut sock: *mut bindings::socket = ptr::null_mut();

        // Create a kernel-owned TCP socket.
        // SAFETY: sock_create_kern is the standard kernel API for creating sockets
        // from kernel context. init_net is the initial network namespace, valid for
        // the entire kernel lifetime. `sock` pointer is written by the function.
        let ret = unsafe {
            bindings::sock_create_kern(
                ptr::addr_of_mut!(bindings::init_net),
                bindings::AF_INET as i32,
                bindings::sock_type_SOCK_STREAM as i32,
                IPPROTO_TCP,
                &mut sock,
            )
        };
        if ret < 0 {
            pr_err!("hackbot: sock_create_kern failed: {}\n", ret);
            return Err(Error::from_errno(ret));
        }

        let socket = Self { sock };

        // Build the IPv4 address.
        let addr_in = SockaddrIn {
            sin_family: bindings::AF_INET as u16,
            sin_port: port.to_be(),
            sin_addr: addr.to_be(),
            __pad: [0u8; 8],
        };

        // Connect to the remote address.
        // SAFETY: kernel_connect is the standard kernel API. We cast SockaddrIn
        // to sockaddr_unsized which is the generic socket address type (same
        // pattern as C's (struct sockaddr *)&addr_in). The socket is valid
        // because sock_create_kern succeeded. The address struct is on the stack
        // and valid for the duration of this call.
        let ret = unsafe {
            bindings::kernel_connect(
                socket.sock,
                &addr_in as *const SockaddrIn as *mut bindings::sockaddr_unsized,
                core::mem::size_of::<SockaddrIn>() as i32,
                0,
            )
        };
        if ret < 0 {
            // Socket is released by Drop.
            pr_err!("hackbot: kernel_connect failed: {}\n", ret);
            return Err(Error::from_errno(ret));
        }

        Ok(socket)
    }

    /// Send all bytes in `buf` through the socket.
    fn send_all(&self, buf: &[u8]) -> Result<()> {
        let mut sent = 0usize;

        while sent < buf.len() {
            let remaining = &buf[sent..];
            let mut kv = bindings::kvec {
                iov_base: remaining.as_ptr() as *mut core::ffi::c_void,
                iov_len: remaining.len(),
            };

            // SAFETY: msghdr is zero-initialized. kernel_sendmsg internally sets
            // up msg_iter from the kvec. Zero-init is correct because we don't
            // need msg_name (connected socket) or msg_control.
            let mut msg: bindings::msghdr = unsafe { MaybeUninit::zeroed().assume_init() };

            // SAFETY: kernel_sendmsg is the standard kernel send API. The socket
            // is valid (from sock_create_kern + connect). The kvec points to valid
            // kernel memory (buf is a kernel slice). The function may sleep.
            let ret = unsafe {
                bindings::kernel_sendmsg(self.sock, &mut msg, &mut kv, 1, remaining.len())
            };

            if ret < 0 {
                return Err(Error::from_errno(ret));
            }
            if ret == 0 {
                // Connection closed unexpectedly.
                return Err(EPIPE);
            }
            sent += ret as usize;
        }

        Ok(())
    }

    /// Receive up to `buf.len()` bytes from the socket.
    /// Returns the number of bytes received, or 0 if the connection was closed.
    fn recv(&self, buf: &mut [u8]) -> Result<usize> {
        let mut kv = bindings::kvec {
            iov_base: buf.as_mut_ptr() as *mut core::ffi::c_void,
            iov_len: buf.len(),
        };

        // SAFETY: Same as send_all — zero-initialized msghdr, kernel_recvmsg
        // sets up msg_iter from kvec internally. MSG_WAITALL is not set, so it
        // returns as soon as any data is available.
        let mut msg: bindings::msghdr = unsafe { MaybeUninit::zeroed().assume_init() };

        // SAFETY: kernel_recvmsg is the standard kernel recv API. Socket and
        // buffer are valid kernel memory. flags=0 means blocking recv.
        let ret = unsafe {
            bindings::kernel_recvmsg(self.sock, &mut msg, &mut kv, 1, buf.len(), 0)
        };

        if ret < 0 {
            return Err(Error::from_errno(ret));
        }
        Ok(ret as usize)
    }

    /// Read all data from the socket until the connection is closed or the
    /// maximum size is reached. Appends to `response`.
    fn recv_all(&self, response: &mut KVVec<u8>, max_size: usize) -> Result<()> {
        let mut tmp = [0u8; RECV_BUF_SIZE];

        loop {
            if response.len() >= max_size {
                pr_warn!("hackbot: response truncated at {} bytes\n", max_size);
                break;
            }

            let n = self.recv(&mut tmp)?;
            if n == 0 {
                break; // Connection closed — we have the full response.
            }

            let _ = response.extend_from_slice(&tmp[..n], GFP_KERNEL);
        }

        Ok(())
    }
}

impl Drop for KernelSocket {
    fn drop(&mut self) {
        // SAFETY: sock_release is the standard cleanup for kernel sockets.
        // Called exactly once when the KernelSocket is dropped.
        unsafe { bindings::sock_release(self.sock) };
    }
}

// ---------------------------------------------------------------------------
// HTTP helpers — minimal HTTP/1.1 client for talking to vLLM
// ---------------------------------------------------------------------------

/// Append a dotted-decimal IPv4 address to a KVVec from a host-order u32.
/// E.g., `append_ipv4(buf, 0x647DD52A)` appends "100.125.213.42".
fn append_ipv4(buf: &mut KVVec<u8>, addr: u32) {
    let octets = [
        ((addr >> 24) & 0xFF) as u8,
        ((addr >> 16) & 0xFF) as u8,
        ((addr >> 8) & 0xFF) as u8,
        (addr & 0xFF) as u8,
    ];
    for (i, &octet) in octets.iter().enumerate() {
        if i > 0 {
            let _ = buf.push(b'.', GFP_KERNEL);
        }
        let mut num_buf = [0u8; 20];
        let s = format_usize(octet as usize, &mut num_buf);
        let _ = buf.extend_from_slice(s, GFP_KERNEL);
    }
}

/// Escape a byte string for embedding in a JSON string value.
/// Handles: \ → \\, " → \", newline → \n, tab → \t, CR → \r.
fn json_escape(input: &[u8], output: &mut KVVec<u8>) {
    for &b in input {
        match b {
            b'\\' => { let _ = output.extend_from_slice(b"\\\\", GFP_KERNEL); }
            b'"'  => { let _ = output.extend_from_slice(b"\\\"", GFP_KERNEL); }
            b'\n' => { let _ = output.extend_from_slice(b"\\n", GFP_KERNEL); }
            b'\r' => { let _ = output.extend_from_slice(b"\\r", GFP_KERNEL); }
            b'\t' => { let _ = output.extend_from_slice(b"\\t", GFP_KERNEL); }
            // Drop other control characters (0x00-0x1F) except the above.
            c if c < 0x20 => {}
            _ => { let _ = output.push(b, GFP_KERNEL); }
        }
    }
}

/// Append a chat message to a JSON messages array being built incrementally.
/// The `messages` buffer holds a JSON array like `[{...},{...}]`.
/// On first call (empty buffer), starts the array with `[`.
///
/// # Invariants
/// - `role` must be a valid JSON-safe ASCII string (no quotes, backslashes, or control chars).
///   Only use with: `b"system"`, `b"user"`, `b"assistant"`.
/// - This function always leaves `messages` ending with `']'`, which is used
///   to detect subsequent calls (truncate `]`, append `,{...}]`).
fn append_message_to_json(messages: &mut KVVec<u8>, role: &[u8], content: &[u8]) {
    if messages.last() == Some(&b']') {
        // Remove trailing ']' and add comma separator.
        messages.truncate(messages.len() - 1);
        let _ = messages.extend_from_slice(b",", GFP_KERNEL);
    } else {
        // First message — start the array.
        let _ = messages.extend_from_slice(b"[", GFP_KERNEL);
    }
    let _ = messages.extend_from_slice(b"{\"role\":\"", GFP_KERNEL);
    let _ = messages.extend_from_slice(role, GFP_KERNEL);
    let _ = messages.extend_from_slice(b"\",\"content\":\"", GFP_KERNEL);
    json_escape(content, messages);
    let _ = messages.extend_from_slice(b"\"}]", GFP_KERNEL);
}

/// Query vLLM's `/v1/models` endpoint to discover the served model name.
/// Returns the model ID (e.g., `Qwen/Qwen2.5-1.5B-Instruct`) as bytes.
/// vLLM requires the exact model name in chat completion requests.
fn discover_model_name() -> Result<KVVec<u8>> {
    let mut req = KVVec::new();
    let _ = req.extend_from_slice(b"GET /v1/models HTTP/1.1\r\n", GFP_KERNEL);
    let _ = req.extend_from_slice(b"Host: ", GFP_KERNEL);
    append_ipv4(&mut req, VLLM_ADDR);
    let _ = req.extend_from_slice(b"\r\nConnection: close\r\n\r\n", GFP_KERNEL);

    let sock = KernelSocket::connect_tcp(VLLM_ADDR, VLLM_PORT)?;
    sock.send_all(&req)?;

    let mut raw_response = KVVec::new();
    sock.recv_all(&mut raw_response, MAX_RESPONSE_SIZE)?;
    drop(sock);

    let status = parse_http_status(&raw_response);
    if status != 200 {
        pr_warn!("hackbot: /v1/models returned HTTP {}\n", status);
        return Err(EIO);
    }

    let body = find_http_body(&raw_response);

    // Extract model ID from {"data":[{"id":"MODEL_NAME",...}]}
    // The first "id" field in the response is the model name.
    let pattern = b"\"id\":\"";
    let pos = find_subsequence(body, pattern).ok_or(EIO)?;
    let value_start = pos + pattern.len();
    let value_end = find_json_string_end(body, value_start).ok_or(EIO)?;

    let model_name_raw = &body[value_start..value_end];
    let mut model_name = KVVec::new();
    // Unescape in case model name has JSON escapes (unlikely but correct).
    json_unescape(model_name_raw, &mut model_name);

    pr_info!(
        "hackbot: discovered model: {}\n",
        core::str::from_utf8(&model_name).unwrap_or("?"),
    );

    Ok(model_name)
}

/// Build the HTTP POST request for vLLM's /v1/chat/completions endpoint.
/// `model_name` is the vLLM model name (from `discover_model_name()`).
/// `messages_json` is a pre-built JSON array of chat messages.
fn build_vllm_request(model_name: &[u8], messages_json: &[u8]) -> Result<KVVec<u8>> {
    let mut body = KVVec::new();
    let _ = body.extend_from_slice(b"{\"model\":\"", GFP_KERNEL);
    json_escape(model_name, &mut body);
    let _ = body.extend_from_slice(b"\",\"messages\":", GFP_KERNEL);
    let _ = body.extend_from_slice(messages_json, GFP_KERNEL);
    let _ = body.extend_from_slice(
        b",\"max_tokens\":4096,\"temperature\":0.7,\"repetition_penalty\":1.1,\"stop\":[\"</tool>\"]}",
        GFP_KERNEL,
    );

    // Build HTTP request with headers.
    let mut req = KVVec::new();
    let _ = req.extend_from_slice(b"POST /v1/chat/completions HTTP/1.1\r\n", GFP_KERNEL);
    let _ = req.extend_from_slice(b"Host: ", GFP_KERNEL);
    append_ipv4(&mut req, VLLM_ADDR);
    let _ = req.extend_from_slice(b"\r\n", GFP_KERNEL);
    let _ = req.extend_from_slice(b"Content-Type: application/json\r\n", GFP_KERNEL);
    let _ = req.extend_from_slice(b"Connection: close\r\n", GFP_KERNEL);

    // Content-Length header — format the body length as decimal.
    let _ = req.extend_from_slice(b"Content-Length: ", GFP_KERNEL);
    let mut len_buf = [0u8; 20];
    let len_str = format_usize(body.len(), &mut len_buf);
    let _ = req.extend_from_slice(len_str, GFP_KERNEL);
    let _ = req.extend_from_slice(b"\r\n", GFP_KERNEL);

    // End of headers.
    let _ = req.extend_from_slice(b"\r\n", GFP_KERNEL);

    // Append body.
    let _ = req.extend_from_slice(&body, GFP_KERNEL);

    Ok(req)
}

/// Format a usize as decimal ASCII into a fixed buffer. Returns the slice
/// containing the formatted number. No heap allocation.
fn format_usize(mut n: usize, buf: &mut [u8; 20]) -> &[u8] {
    if n == 0 {
        buf[0] = b'0';
        return &buf[..1];
    }
    let mut pos = 20;
    while n > 0 {
        pos -= 1;
        buf[pos] = b'0' + (n % 10) as u8;
        n /= 10;
    }
    &buf[pos..]
}

/// Find the HTTP response body by locating the \r\n\r\n header terminator.
/// Returns the body portion, or the entire response if no header found.
fn find_http_body(raw: &[u8]) -> &[u8] {
    // Search for \r\n\r\n
    if raw.len() < 4 {
        return raw;
    }
    for i in 0..raw.len() - 3 {
        if &raw[i..i + 4] == b"\r\n\r\n" {
            return &raw[i + 4..];
        }
    }
    raw
}

/// Extract the HTTP status code from the first line (e.g., "HTTP/1.1 200 OK").
/// Returns 0 if parsing fails.
fn parse_http_status(raw: &[u8]) -> u16 {
    // Find "HTTP/1.1 " or "HTTP/1.0 " prefix, then parse the 3-digit status.
    let prefix = b"HTTP/1.";
    if raw.len() < 12 || &raw[..7] != prefix {
        return 0;
    }
    // Skip "HTTP/1.X " — status code starts at byte 9.
    if raw[8] != b' ' {
        return 0;
    }
    let d0 = raw[9].wrapping_sub(b'0') as u16;
    let d1 = raw[10].wrapping_sub(b'0') as u16;
    let d2 = raw[11].wrapping_sub(b'0') as u16;
    if d0 > 9 || d1 > 9 || d2 > 9 {
        return 0;
    }
    d0 * 100 + d1 * 10 + d2
}

// ---------------------------------------------------------------------------
// JSON response parsing — minimal, no-alloc extraction
// ---------------------------------------------------------------------------

/// Extract the "text" field value from a vLLM completions JSON response.
/// Looks for `"text":"` or `"text": "` and extracts the string value,
/// handling escaped characters. Returns the raw bytes of the text content.
fn extract_text_from_json<'a>(json: &'a [u8]) -> Option<&'a [u8]> {
    // Chat completions: {"choices":[{"message":{"content":"..."}}]}
    // Completions (fallback): {"choices":[{"text":"..."}]}
    let patterns: &[&[u8]] = &[
        b"\"content\":\"", b"\"content\": \"",
        b"\"text\":\"", b"\"text\": \"",
    ];

    let (start_pos, pat_len) = patterns.iter().find_map(|pat| {
        find_subsequence(json, pat).map(|pos| (pos, pat.len()))
    })?;

    let value_start = start_pos + pat_len;
    let value_end = find_json_string_end(json, value_start)?;

    Some(&json[value_start..value_end])
}

/// Find the position of a subsequence in a byte slice.
fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    for i in 0..=haystack.len() - needle.len() {
        if &haystack[i..i + needle.len()] == needle {
            return Some(i);
        }
    }
    None
}

/// Find the end of a JSON string value (the closing unescaped `"`).
/// `start` is the index of the first character AFTER the opening `"`.
fn find_json_string_end(json: &[u8], start: usize) -> Option<usize> {
    let mut i = start;
    while i < json.len() {
        match json[i] {
            b'"' => return Some(i),
            b'\\' => i = i.saturating_add(2), // Skip escaped character.
            _ => i += 1,
        }
    }
    None
}

/// Unescape a JSON string value in-place into a KVVec.
/// Handles: \\→\, \"→", \n→newline, \r→CR, \t→tab.
fn json_unescape(escaped: &[u8], output: &mut KVVec<u8>) {
    let mut i = 0;
    while i < escaped.len() {
        if escaped[i] == b'\\' && i + 1 < escaped.len() {
            let c = match escaped[i + 1] {
                b'n' => b'\n',
                b'r' => b'\r',
                b't' => b'\t',
                b'\\' => b'\\',
                b'"' => b'"',
                b'/' => b'/',
                other => {
                    // Unknown escape — keep as-is.
                    let _ = output.push(b'\\', GFP_KERNEL);
                    let _ = output.push(other, GFP_KERNEL);
                    i += 2;
                    continue;
                }
            };
            let _ = output.push(c, GFP_KERNEL);
            i += 2;
        } else {
            let _ = output.push(escaped[i], GFP_KERNEL);
            i += 1;
        }
    }
}

// ---------------------------------------------------------------------------
// vLLM inference — the System 2 "brain"
// ---------------------------------------------------------------------------

/// Send a prompt to vLLM and return the response bytes.
/// On error, returns a descriptive error message instead.
/// Agent loop dispatcher: selects inference backend based on INFERENCE_MODE
/// and model availability.
///
/// Auto mode (default): tries local inference if model is loaded, falls back
/// to vLLM if local fails or produces empty output.
fn agent_loop(prompt: &[u8]) -> Result<KVVec<u8>> {
    let use_local = match INFERENCE_MODE {
        INFERENCE_MODE_LOCAL => true,
        INFERENCE_MODE_VLLM => false,
        _ => {
            // Auto mode: check if model is loaded
            let slot = MODEL.lock();
            slot.loaded
        }
    };

    if use_local {
        match agent_loop_local(prompt) {
            Ok(response) if !response.is_empty() => {
                pr_info!("hackbot: using local inference backend\n");
                return Ok(response);
            }
            Ok(_) => {
                // Empty response from local
                if INFERENCE_MODE == INFERENCE_MODE_LOCAL {
                    let mut r = KVVec::new();
                    let _ = r.extend_from_slice(
                        b"[hackbot] Local inference produced no output.\n", GFP_KERNEL,
                    );
                    return Ok(r);
                }
                pr_info!("hackbot: local inference empty, falling back to vLLM\n");
            }
            Err(e) => {
                if INFERENCE_MODE == INFERENCE_MODE_LOCAL {
                    return Err(e);
                }
                pr_info!("hackbot: local inference failed ({}), falling back to vLLM\n",
                         e.to_errno());
            }
        }
    }

    agent_loop_vllm(prompt)
}

fn process_prompt(prompt: &[u8]) -> KVVec<u8> {
    // Strip trailing newline from the prompt.
    let prompt_trimmed = if prompt.last() == Some(&b'\n') {
        &prompt[..prompt.len() - 1]
    } else {
        prompt
    };

    if prompt_trimmed.is_empty() {
        let mut r = KVVec::new();
        let _ = r.extend_from_slice(b"[hackbot] Empty prompt.\n", GFP_KERNEL);
        return r;
    }

    match agent_loop(prompt_trimmed) {
        Ok(mut text) => {
            // Ensure response ends with newline.
            if text.last() != Some(&b'\n') {
                let _ = text.push(b'\n', GFP_KERNEL);
            }
            text
        }
        Err(e) => {
            let mut r = KVVec::new();
            let _ = r.extend_from_slice(b"[hackbot] Inference error: ", GFP_KERNEL);
            let mut code_buf = [0u8; 20];
            // Error codes are negative; display the positive errno value.
            let code = -(e.to_errno() as isize) as usize;
            let code_str = format_usize(code, &mut code_buf);
            let _ = r.extend_from_slice(code_str, GFP_KERNEL);
            let _ = r.extend_from_slice(b" (errno)\n", GFP_KERNEL);

            // Add human-readable hint for common errors.
            let hint = match -(e.to_errno()) {
                19 => b"No model loaded and no vLLM available.\n" as &[u8],
                111 => b"Connection refused - is vLLM running on port 8000?\n",
                110 => b"Connection timed out - check network/firewall.\n",
                _ => b"",
            };
            if !hint.is_empty() {
                let _ = r.extend_from_slice(hint, GFP_KERNEL);
            }
            r
        }
    }
}

/// Send a single chat completion request to vLLM and return the extracted text.
/// `model_name` is the vLLM model name. `messages_json` is a pre-built JSON messages array.
fn vllm_call(model_name: &[u8], messages_json: &[u8]) -> Result<KVVec<u8>> {
    let request = build_vllm_request(model_name, messages_json)?;

    let sock = KernelSocket::connect_tcp(VLLM_ADDR, VLLM_PORT)?;
    sock.send_all(&request)?;

    let mut raw_response = KVVec::new();
    sock.recv_all(&mut raw_response, MAX_RESPONSE_SIZE)?;
    drop(sock);

    let status = parse_http_status(&raw_response);
    if status != 200 {
        pr_warn!("hackbot: vLLM returned HTTP {}\n", status);
        if status == 0 {
            return Err(EIO);
        }
        let body = find_http_body(&raw_response);
        let mut result = KVVec::new();
        let _ = result.extend_from_slice(b"[HTTP ", GFP_KERNEL);
        let mut status_buf = [0u8; 20];
        let status_str = format_usize(status as usize, &mut status_buf);
        let _ = result.extend_from_slice(status_str, GFP_KERNEL);
        let _ = result.extend_from_slice(b"] ", GFP_KERNEL);
        let _ = result.extend_from_slice(body, GFP_KERNEL);
        return Ok(result);
    }

    let body = find_http_body(&raw_response);

    match extract_text_from_json(body) {
        Some(escaped_text) => {
            let mut result = KVVec::new();
            json_unescape(escaped_text, &mut result);
            Ok(result)
        }
        None => {
            pr_warn!("hackbot: could not parse vLLM JSON response\n");
            let mut result = KVVec::new();
            let _ = result.extend_from_slice(b"[hackbot] Raw response: ", GFP_KERNEL);
            let _ = result.extend_from_slice(body, GFP_KERNEL);
            Ok(result)
        }
    }
}

/// OODA agent loop: multi-turn conversation with kernel tool calls (vLLM backend).
///
/// Builds the initial conversation (system prompt + tools description +
/// live kernel context + user prompt), then loops:
///   1. Send conversation to vLLM
///   2. Parse response for `<tool>name</tool>` tags
///   3. If tool call: execute tool, append result to conversation, continue
///   4. If final answer (no tool call): return the response
///   5. Stop after MAX_AGENT_ITERATIONS or if conversation exceeds size limit
fn agent_loop_vllm(prompt: &[u8]) -> Result<KVVec<u8>> {
    // Discover the served model name from vLLM (one extra GET per prompt).
    let model_name = discover_model_name()?;

    // Gather live kernel state.
    let kernel_ctx = gather_kernel_context();

    // Build the system message: identity → kernel context → tool description.
    // Tools placed LAST for attention salience in small models.
    let mut system_content = KVVec::new();
    let _ = system_content.extend_from_slice(SYSTEM_IDENTITY, GFP_KERNEL);
    let _ = system_content.extend_from_slice(&kernel_ctx, GFP_KERNEL);
    let _ = system_content.extend_from_slice(TOOL_DESCRIPTION, GFP_KERNEL);

    // Build messages: system + user prompt only.
    // No few-shot conversation messages — the format example is inside the
    // system prompt to avoid the model confusing example data with real data.
    let mut messages = KVVec::new();
    append_message_to_json(&mut messages, b"system", &system_content);
    append_message_to_json(&mut messages, b"user", prompt);

    let mut final_answer = KVVec::new();
    let mut got_final_answer = false;

    for iteration in 0..MAX_AGENT_ITERATIONS {
        pr_info!(
            "hackbot: agent iteration {}/{}\n",
            iteration + 1,
            MAX_AGENT_ITERATIONS,
        );

        // Safety check: don't let the messages array grow unbounded.
        if messages.len() > MAX_CONVERSATION_SIZE {
            pr_warn!("hackbot: conversation exceeded {} bytes, stopping\n", MAX_CONVERSATION_SIZE);
            let _ = final_answer.extend_from_slice(
                b"\n[hackbot] Agent stopped: conversation too large.\n",
                GFP_KERNEL,
            );
            break;
        }

        // Call vLLM with the current messages.
        let response = match vllm_call(&model_name, &messages) {
            Ok(r) => r,
            Err(e) => {
                pr_err!("hackbot: vLLM call failed at iteration {}: {}\n", iteration + 1, e.to_errno());
                if !final_answer.is_empty() {
                    let _ = final_answer.extend_from_slice(
                        b"\n[hackbot] vLLM error during agent loop.\n",
                        GFP_KERNEL,
                    );
                    break;
                }
                return Err(e);
            }
        };

        // Parse the response for tool calls.
        match parse_tool_call(&response) {
            ToolCallResult::FinalAnswer(text) => {
                pr_info!("hackbot: final answer at iteration {}\n", iteration + 1);
                let _ = final_answer.extend_from_slice(text, GFP_KERNEL);
                got_final_answer = true;
                break;
            }
            ToolCallResult::ToolCall { name, prefix } => {
                pr_info!(
                    "hackbot: tool call '{}' at iteration {}\n",
                    core::str::from_utf8(name).unwrap_or("?"),
                    iteration + 1,
                );

                // Last iteration: force output instead of looping forever.
                // Execute the tool and return raw output — better than empty response.
                if iteration == MAX_AGENT_ITERATIONS - 1 {
                    pr_info!("hackbot: last iteration, forcing final answer with raw tool output\n");
                    let tool_output = execute_tool(name);
                    final_answer.truncate(0);
                    if !prefix.is_empty() {
                        let _ = final_answer.extend_from_slice(prefix, GFP_KERNEL);
                        let _ = final_answer.extend_from_slice(b"\n\n", GFP_KERNEL);
                    }
                    let _ = final_answer.extend_from_slice(&tool_output, GFP_KERNEL);
                    got_final_answer = true;
                    break;
                }

                // Execute the tool.
                let tool_output = execute_tool(name);

                // Build the assistant's response (prefix + tool call tag).
                let mut assistant_content = KVVec::new();
                let _ = assistant_content.extend_from_slice(prefix, GFP_KERNEL);
                let _ = assistant_content.extend_from_slice(b"<tool>", GFP_KERNEL);
                let _ = assistant_content.extend_from_slice(name, GFP_KERNEL);
                let _ = assistant_content.extend_from_slice(b"</tool>", GFP_KERNEL);

                // Build tool result as a user message.
                // Permissive prompt — don't restrict the model's action space.
                let mut tool_result = KVVec::new();
                let _ = tool_result.extend_from_slice(b"[Tool: ", GFP_KERNEL);
                let _ = tool_result.extend_from_slice(name, GFP_KERNEL);
                let _ = tool_result.extend_from_slice(b"]\n", GFP_KERNEL);
                let _ = tool_result.extend_from_slice(&tool_output, GFP_KERNEL);
                let _ = tool_result.extend_from_slice(
                    b"[End Tool]\nAbove is live data from the kernel. \
Analyze it thoughtfully and respond to the user.",
                    GFP_KERNEL,
                );

                append_message_to_json(&mut messages, b"assistant", &assistant_content);
                append_message_to_json(&mut messages, b"user", &tool_result);

                // Accumulate the prefix as partial answer.
                let _ = final_answer.extend_from_slice(prefix, GFP_KERNEL);
            }
        }
    }

    // If we exhausted iterations without a clean final answer, append a note.
    if !got_final_answer && final_answer.is_empty() {
        let _ = final_answer.extend_from_slice(
            b"[hackbot] Agent completed maximum iterations without a final answer.\n",
            GFP_KERNEL,
        );
    } else if !got_final_answer {
        let _ = final_answer.extend_from_slice(
            b"\n[hackbot] Agent stopped after maximum iterations.\n",
            GFP_KERNEL,
        );
    }

    Ok(final_answer)
}

// ---------------------------------------------------------------------------
// MiscDevice trait implementation — the file_operations vtable
// ---------------------------------------------------------------------------

#[vtable]
impl MiscDevice for HackbotDev {
    type Ptr = Pin<KBox<Self>>;

    fn open(_file: &File, misc: &MiscDeviceRegistration<Self>) -> Result<Pin<KBox<Self>>> {
        let dev = ARef::from(misc.device());
        dev_info!(dev, "hackbot: device opened\n");

        // Try to load model firmware on first open (best-effort, non-fatal).
        load_model_if_needed(&dev);

        KBox::try_pin_init(
            try_pin_init! {
                HackbotDev {
                    dev: dev,
                }
            },
            GFP_KERNEL,
        )
    }

    /// Handle write() from userspace — receives a prompt, sends to vLLM.
    /// This call blocks until the vLLM response is received.
    /// The response is stored in the global RESPONSE buffer so that a
    /// subsequent `cat /dev/hackbot` (separate fd) can read it.
    fn write_iter(_kiocb: Kiocb<'_, Self::Ptr>, iov: &mut IovIterSource<'_>) -> Result<usize> {
        // Read the prompt from userspace into a local buffer.
        // No lock held during this or the vLLM call — keeps the device responsive.
        let mut prompt = KVVec::new();
        let len = iov.copy_from_iter_vec(&mut prompt, GFP_KERNEL)?;

        if len == 0 {
            return Ok(0);
        }

        // Send prompt to vLLM and get response (blocking network I/O).
        let response = process_prompt(&prompt);

        // Store the response in the global buffer.
        let mut guard = RESPONSE.lock();
        let copy_len = response.len().min(MAX_RESPONSE_SIZE);
        guard.data[..copy_len].copy_from_slice(&response[..copy_len]);
        guard.len = copy_len;
        guard.ready = true;
        drop(guard);

        Ok(len)
    }

    /// Handle read() from userspace — returns the last vLLM response.
    /// If no response is available yet, returns 0 (EOF) immediately rather
    /// than blocking, since the writer may be a different (already-closed) fd.
    fn read_iter(mut kiocb: Kiocb<'_, Self::Ptr>, iov: &mut IovIterDest<'_>) -> Result<usize> {
        let guard = RESPONSE.lock();

        if !guard.ready {
            // No response yet — return EOF so `cat` exits cleanly.
            return Ok(0);
        }

        // Copy response to userspace, respecting file position for partial reads.
        let data = &guard.data[..guard.len];
        let read = iov.simple_read_from_buffer(kiocb.ki_pos_mut(), data)?;

        Ok(read)
    }
}

#[pinned_drop]
impl PinnedDrop for HackbotDev {
    fn drop(self: Pin<&mut Self>) {
        dev_info!(self.dev, "hackbot: device closed\n");
    }
}
