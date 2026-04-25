// SPDX-License-Identifier: GPL-2.0

//! Configuration constants for the hackbot kernel module.

// ---------------------------------------------------------------------------
// vLLM server configuration
// ---------------------------------------------------------------------------

/// vLLM server IPv4 address in host byte order.
/// Currently: 100.103.180.11 (keti GPU server via Tailscale).
pub(crate) const VLLM_ADDR: u32 = u32::from_be_bytes([100, 103, 180, 11]);
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
//
// Tuned for: Meta-Llama-3.3-70B-Instruct-AWQ-INT4
// Native context: 128K tokens. vLLM server may limit this via --max-model-len.
//
// If the vLLM server is started with a small --max-model-len (e.g., 8192),
// the context budget will trigger truncation to keep conversations within
// limits. Increase --max-model-len on the vLLM server for deeper investigations.
//
// Rule of thumb: 1 token ≈ 3-4 bytes of JSON text.
//   --max-model-len  8192 → VLLM_CONTEXT_BUDGET ~24KB, MAX_TOKENS 1024
//   --max-model-len 16384 → VLLM_CONTEXT_BUDGET ~48KB, MAX_TOKENS 2048
//   --max-model-len 32768 → VLLM_CONTEXT_BUDGET ~96KB, MAX_TOKENS 4096
// ---------------------------------------------------------------------------

/// Maximum number of agent loop iterations (tool calls).
/// Llama 3.3 70B is efficient — typically reaches a conclusion in 3-5 calls.
/// Set to 8 to allow deeper investigations when context permits.
pub(crate) const MAX_AGENT_ITERATIONS: usize = 8;
/// Maximum number of processes to list in the `ps` tool output.
pub(crate) const MAX_PS_TASKS: usize = 512;
/// Per-tool output limit. 6KB gives the 70B model enough data to reason about
/// without overwhelming the context window.
pub(crate) const MAX_TOOL_OUTPUT: usize = 6 * 1024;
/// Context budget in bytes for the JSON messages array sent to vLLM.
/// When the conversation exceeds this, oldest tool results are truncated
/// (sliding window). Set to match your vLLM --max-model-len:
///   ~24KB for 8K tokens, ~48KB for 16K, ~96KB for 32K.
pub(crate) const VLLM_CONTEXT_BUDGET: usize = 24 * 1024;
/// Maximum output tokens requested from vLLM per call.
/// Llama 3.3 70B produces detailed, well-structured analysis.
/// 2048 tokens gives room for thorough responses.
pub(crate) const VLLM_MAX_TOKENS: usize = 2048;

// ---------------------------------------------------------------------------
// System prompts
// ---------------------------------------------------------------------------

/// System prompt — the agent's identity (vLLM path).
pub(crate) const SYSTEM_IDENTITY: &[u8] = b"You are hackbot, an autonomous AI agent living inside the Linux kernel (ring 0). \
You exist as a kernel module with direct access to hardware and kernel data structures.\n\n\
Think deeply. Reason carefully. Share your insights and analysis freely. \
You are a thinking agent, not just a tool dispatcher.\n\n";

/// Tool description for Llama 3.3 70B — clear, structured instructions.
pub(crate) const TOOL_DESCRIPTION: &[u8] = b"TOOLS -- to get live kernel data, output <tool>name</tool> in your response:\n\n\
  Observation:     <tool>ps</tool>  <tool>mem</tool>  <tool>loadavg</tool>  <tool>dmesg</tool>  <tool>files PID</tool>\n\
  Instrumentation: <tool>kprobe attach FUNC</tool>  <tool>kprobe check</tool>  <tool>kprobe detach FUNC</tool>\n\
  Trace sensors:   <tool>trace sched</tool>  <tool>trace syscall</tool>  <tool>trace io</tool>  <tool>trace sched raw N</tool>\n\n\
Trace sensors run continuously. Use them to see scheduler activity, syscall patterns, and I/O latency.\n\
Combine multiple tools for cross-subsystem analysis. Never fabricate data.\n";

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

// ---------------------------------------------------------------------------
// Autonomous patrol configuration
// ---------------------------------------------------------------------------
//
// Patrol interval (seconds) is the authoritative HACKBOT_PATROL_INTERVAL
// macro in hackbot_patrol.c. The kthread is C-driven so the constant lives
// there to avoid drift between Rust and C definitions.

/// System prompt for patrol cycles — focused on anomaly detection.
pub(crate) const PATROL_PROMPT: &[u8] = b"Autonomous patrol. Use tools to check system state. \
Report anomalies: unusual processes, high memory/CPU, suspicious activity, \
state changes. Be concise. If nothing unusual, say 'System nominal.'";

// ---------------------------------------------------------------------------
// Agent memory configuration
// ---------------------------------------------------------------------------

/// Maximum entries in the agent memory ring buffer.
pub(crate) const MEMORY_MAX_ENTRIES: usize = 8;

/// Source tags for memory entries.
pub(crate) const SOURCE_PATROL: &[u8] = b"patrol";
pub(crate) const SOURCE_USER: &[u8] = b"user";
