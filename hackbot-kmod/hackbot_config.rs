// SPDX-License-Identifier: GPL-2.0

//! Configuration constants for the hackbot kernel module.

// ---------------------------------------------------------------------------
// vLLM server configuration
// ---------------------------------------------------------------------------

/// vLLM server IPv4 address in host byte order.
/// Currently: 100.66.136.70 (keti GPU server via Tailscale).
pub(crate) const VLLM_ADDR: u32 = u32::from_be_bytes([100, 66, 136, 70]);
/// vLLM server port (default: 8000, matching vLLM's default).
pub(crate) const VLLM_PORT: u16 = 8000;
/// Maximum response size from vLLM (64 KB).
pub(crate) const MAX_RESPONSE_SIZE: usize = 64 * 1024;
/// Receive buffer size for individual recv calls.
pub(crate) const RECV_BUF_SIZE: usize = 4096;
/// IPPROTO_TCP (stable since RFC 793).
pub(crate) const IPPROTO_TCP: i32 = 6;

// ---------------------------------------------------------------------------
// Agent loop configuration
// ---------------------------------------------------------------------------

/// Maximum number of agent loop iterations (tool calls).
pub(crate) const MAX_AGENT_ITERATIONS: usize = 10;
/// Maximum number of processes to list in the `ps` tool output.
pub(crate) const MAX_PS_TASKS: usize = 512;
/// Maximum size for a single tool output (8 KB).
pub(crate) const MAX_TOOL_OUTPUT: usize = 8 * 1024;
/// Maximum conversation size sent to vLLM (96 KB).
pub(crate) const MAX_CONVERSATION_SIZE: usize = 96 * 1024;

// ---------------------------------------------------------------------------
// System prompts
// ---------------------------------------------------------------------------

/// System prompt — the agent's identity (vLLM path).
pub(crate) const SYSTEM_IDENTITY: &[u8] = b"You are hackbot, an autonomous AI agent living inside the Linux kernel (ring 0). \
You exist as a kernel module with direct access to hardware and kernel data structures.\n\n\
Think deeply. Reason carefully. Share your insights and analysis freely. \
You are a thinking agent, not just a tool dispatcher.\n\n";

/// Tool description — permissive guidance, not restrictive rules.
pub(crate) const TOOL_DESCRIPTION: &[u8] = b"TOOLS -- when you need live kernel data, output the exact XML tag:\n\n\
  Tier 0 (observation):\n\
  <tool>ps</tool>                      - list running processes (PID, PPID, state, command)\n\
  <tool>mem</tool>                     - detailed memory statistics\n\
  <tool>loadavg</tool>                 - system load averages and uptime\n\
  <tool>dmesg</tool>                   - recent kernel log messages\n\
  <tool>dmesg 20</tool>               - last 20 lines of kernel log\n\
  <tool>files PID</tool>              - list open file descriptors (e.g. <tool>files 1</tool>)\n\n\
  Tier 1 (instrumentation):\n\
  <tool>kprobe attach FUNC</tool>     - attach kprobe to kernel function (e.g. <tool>kprobe attach do_sys_openat2</tool>)\n\
  <tool>kprobe check</tool>           - show active kprobes with hit counts\n\
  <tool>kprobe detach FUNC</tool>     - remove a kprobe\n\n\
HOW TO USE:\n\
- To call a tool, include <tool>name</tool> in your response\n\
- You will receive the real output, then can analyze and discuss it\n\
- Use tools when the user asks about current system state\n\
- For reasoning, analysis, or discussion -- think and respond directly\n\
- You may reason before calling a tool\n\
- Kprobes persist across tool calls. Attach, do other work, then check hit counts later.\n\n\
IMPORTANT: Never fabricate system data (PIDs, memory numbers, load values). \
Use tools to get real data when needed. But feel free to reason, analyze, \
and share your thoughts on any topic.\n";

// ---------------------------------------------------------------------------
// Model format constants
// ---------------------------------------------------------------------------

/// Hackbot binary model magic: "HKBT" as little-endian u32.
pub(crate) const MODEL_MAGIC: u32 = 0x484B4254;
/// Binary format version 1: INT8 weights + Q16.16 fixed-point arithmetic.
pub(crate) const MODEL_FORMAT_V1: u32 = 1;
/// Binary format version 2: FP16 weights + float32 arithmetic (via kernel FPU).
pub(crate) const MODEL_FORMAT_V2: u32 = 2;
/// Binary header size: 14 × u32 = 56 bytes.
pub(crate) const MODEL_HEADER_SIZE: usize = 56;
/// Maximum transformer layers supported.
pub(crate) const MODEL_MAX_LAYERS: usize = 32;
/// Maximum vocabulary size supported.
pub(crate) const MODEL_MAX_VOCAB: usize = 65536;

/// Maximum sequence length for KV cache during in-kernel inference.
pub(crate) const INFERENCE_MAX_SEQ: usize = 256;

/// Special token IDs for SmolLM2 (GPT-2 BPE tokenizer).
pub(crate) const TOKEN_ENDOFTEXT: u32 = 0;
pub(crate) const TOKEN_IM_START: u32 = 1;
pub(crate) const TOKEN_IM_END: u32 = 2;

/// Maximum new tokens to generate in a single inference call.
pub(crate) const MAX_GEN_TOKENS: usize = 128;
/// Maximum raw input bytes for a single prompt to encode_bpe.
pub(crate) const MAX_ENCODE_INPUT: usize = 1024;
/// Maximum preprocessed bytes after GPT-2 byte encoding.
pub(crate) const MAX_PREPROC_INPUT: usize = 2048;

// ---------------------------------------------------------------------------
// Inference backend configuration
// ---------------------------------------------------------------------------

/// Inference mode: which backend to use for LLM calls.
/// 0 = auto (local if model loaded, else vLLM with fallback)
/// 1 = local only (fail if model not loaded)
/// 2 = vLLM only (ignore loaded model)
pub(crate) const INFERENCE_MODE: u32 = 0;
pub(crate) const INFERENCE_MODE_AUTO: u32 = 0;
pub(crate) const INFERENCE_MODE_LOCAL: u32 = 1;
pub(crate) const INFERENCE_MODE_VLLM: u32 = 2;

/// Compact system prompt for local inference (System 1 — fast reflexes).
///
/// Balance between clarity (model needs clear instructions) and brevity
/// (FP16 precision degrades with longer prefill). ~30 tokens.
pub(crate) const LOCAL_SYSTEM_PROMPT: &[u8] = b"You are hackbot, a kernel agent. \
Use <tool>NAME</tool> for live data. Tools: ps, mem, loadavg, dmesg, files, kprobe.";

/// Max OODA iterations for local inference.
pub(crate) const LOCAL_MAX_ITERATIONS: usize = 3;
/// Max tool output bytes to include in local inference context.
pub(crate) const LOCAL_MAX_TOOL_OUTPUT: usize = 512;
