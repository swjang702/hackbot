# hackbot Implementation Report

## Phase 1: Static Trace Replay Viewer

**Status**: Complete
**Date**: 2026-03-08

### Overview

Phase 1 implements a static trace replay viewer: a Rust backend loads pre-recorded `.jsonl` trace files, replays events with timing control over WebSocket, and a TypeScript/Pixi.js frontend renders them as a navigable 2D game world.

### Architecture

```
.jsonl file --> [Rust Backend (Axum)] --WebSocket--> [Browser (Pixi.js)]
                    port 8000                          port 5173 (dev)
```

Two-process system:
- **Backend** (`server-rs/`): Rust, Axum + Tokio, single binary
- **Frontend** (`frontend/`): TypeScript, Vite, Pixi.js v8

### Backend: Rust Rewrite (from Python/FastAPI)

The backend was initially prototyped in Python 3.12 + FastAPI (`server/`, now deprecated), then rewritten to Rust for Verus formal verification alignment and native eBPF support (Phase 3).

#### Cargo Workspace Structure

```
server-rs/
  Cargo.toml                    # workspace root
  crates/
    hackbot-types/              # shared types crate
      src/lib.rs                # TraceEvent, EventType, payloads, WorldState, WS messages
    hackbot-server/             # main binary crate
      src/
        main.rs                 # Axum router, startup, CLI flags
        gateway.rs              # WebSocket handler, broadcast, command routing
        trace_loader.rs         # .jsonl parsing, validation, sorting
        trace_replayer.rs       # Async replay with timing, play/pause/seek/speed
        world_model.rs          # Process map, fd table, connections
        mock_data.rs            # Deterministic trace generator (seed=42)
```

#### Key Dependencies

| Crate | Version | Purpose |
|---|---|---|
| axum | 0.8 | HTTP + WebSocket server |
| tokio | 1 | Async runtime |
| serde + serde_json | 1 | JSON serialization |
| tower-http | 0.6 | CORS middleware |
| rand | 0.8 | Mock data generation (StdRng, seed=42) |
| tracing | 0.1 | Structured logging |
| thiserror | 2 | Error type derivation |
| futures-util | 0.3 | Stream utilities for WebSocket |

#### Module Details

**`hackbot-types`** (shared types crate):
- `TraceEvent`: ts (u64), event_type (EventType enum), pid, tid, cpu, comm, payload (serde_json::Value)
- `EventType`: 8 variants (SyscallEnter, SyscallExit, SchedSwitch, PowerTrace, ProcessFork, ProcessExit, GpuSubmit, GpuComplete)
- Payload structs for each event type with validation via `validate_payload()`
- `ProcessInfo`, `ConnectionInfo`, `ProcessStatus` for world state
- `ServerMessage` (tagged enum for world_state/events/playback)
- `ClientCommand` (tagged enum for load/play/pause/seek/speed/filter)
- Timestamps serialized as strings in JSON for JavaScript BigInt safety

**`trace_loader`**:
- Line-by-line BufReader parsing with `serde_json::from_str`
- Payload validation against typed structs
- Sorted by timestamp after loading
- `get_trace_info()` returns summary (event_count, duration, PIDs, event types)

**`world_model`**:
- HashMap<u32, ProcessInfo> for process tracking
- Event handler dispatch via match on EventType
- Tracks syscall counts, GPU submit counts, process status transitions
- `rebuild_to(events, ts)` for seek support (reset + replay up to timestamp)

**`trace_replayer`**:
- Async state machine using `tokio::time::sleep` for timing
- `tokio::sync::Notify` for play/pause/seek interrupts
- 16ms batch window (BATCH_WINDOW_NS) for 60fps rendering
- Speed multiplier (0.1x - 100x) with wall-clock compensation
- PID and event type filtering
- `next_batch()` returns `Option<Vec<TraceEvent>>` — None when complete

**`gateway`**:
- `GatewayState` behind `Arc<Mutex<>>` for shared access
- `tokio::sync::broadcast` channel for fan-out to WebSocket clients
- Per-client read/write tasks with mpsc forwarding
- Playback loop spawned as a background tokio task
- World state broadcast every 500ms during playback
- Command handling: load, play, pause, seek (relative->absolute conversion), speed, filter

**`main`**:
- Axum router: `GET /` (info), `GET /traces` (file list), `WS /ws` (upgrade)
- CORS via `tower_http::cors::CorsLayer::permissive()`
- Auto-loads `sample-llm-workload.jsonl` on startup
- `--generate-mock` CLI flag for trace generation
- Graceful error message on port-in-use

**`mock_data`**:
- Generates ~8912 events across 5 seconds (deterministic with seed=42)
- Narrative phases: Startup (fork 4 workers) -> Prefill (large GPU batches) -> Decode (small regular submits) -> Anomaly (probe_tool reads /proc/maps) -> Recovery
- Note: event count differs from Python version (8912 vs 8589) due to different RNG implementation; WebSocket protocol is identical

### Frontend: TypeScript + Pixi.js v8

Frontend was implemented in the previous session and requires **zero changes** for the Rust backend — the WebSocket JSON protocol is identical.

#### Modules

- `connection.ts`: WebSocket client with auto-reconnect (exponential backoff)
- `app.ts`: Orchestrator wiring all panels
- `game/world.ts`: Pixi.js Application with room containers, syscall pool, particles
- `game/process-room.ts`: Labeled rectangles with activity decay animation
- `game/syscall-object.ts`: Object pool (200 pre-allocated), color-coded by syscall name
- `game/event-particle.ts`: Burst particles (100 pool, 8 per burst)
- `game/camera.ts`: Pan (drag) + zoom (scroll), clamped 0.1x-5x
- `game/spatial-mapper.ts`: Tree layout from parent-child relationships
- `ui/timeline.ts`: Play/pause, speed buttons, scrub slider
- `ui/event-log.ts`: Max 500 entries, auto-scroll, color-coded
- `ui/controls.ts`: PID filter chips, event type filter chips

### WebSocket Protocol

Identical between Python and Rust backends. Frontend sends/receives:

**Server -> Client:**
- `{"msg": "world_state", "processes": [...], "connections": [...]}`
- `{"msg": "events", "batch": [{"ts": "...", "type": "...", ...}, ...]}`
- `{"msg": "playback", "status": "...", "speed": 1.0, "position_ns": "...", "duration_ns": "...", "start_ns": "..."}`

**Client -> Server:**
- `{"cmd": "load", "file": "..."}`
- `{"cmd": "play"}`
- `{"cmd": "pause"}`
- `{"cmd": "seek", "position_ns": "..."}`
- `{"cmd": "speed", "multiplier": 2.0}`
- `{"cmd": "filter", "pids": [...], "types": [...]}`

### Build & Run

```bash
# Generate mock data
cd server-rs && cargo run -- --generate-mock

# Run backend
cd server-rs && cargo run                     # dev (port 8000)
cd server-rs && cargo build --release         # optimized binary

# Run frontend
cd frontend && pnpm install && pnpm dev       # dev server (port 5173)

# SSH tunnel (for remote access from Mac)
ssh -L 5173:localhost:5173 -L 8000:localhost:8000 fedora
```

### Decision: Why Rust over Python

1. **Verus alignment**: Research Pillar 4 requires formally verified code. Verus verifies Rust.
2. **eBPF native path**: Phase 3 uses `aya` — kernel and userspace both in Rust.
3. **Small rewrite cost**: Python backend was ~850 lines, ported in one session.
4. **Single binary**: No Python environment management on research machines.
5. **Performance headroom**: Live eBPF streaming (Phase 3) at millions of events/sec.

---

## Step 2a: vLLM Inference via Kernel Socket

**Status**: Implementation complete (builds successfully)
**Date**: 2026-03-18

### Overview

Replaced the Step 1 dummy echo with a real LLM inference path. The kernel module creates a TCP socket to a localhost vLLM server, sends the user's prompt as an HTTP POST to `/v1/completions`, and returns the completion text.

### Architecture

```
write("/dev/hackbot", prompt)
  → hackbot.ko: sock_create_kern() + kernel_connect(127.0.0.1:8000)
  → kernel_sendmsg(): HTTP POST /v1/completions {"prompt":"...","max_tokens":512}
  → kernel_recvmsg(): receive full HTTP response
  → parse HTTP status, extract JSON "text" field
  → store in response buffer, signal CondVar
read("/dev/hackbot")
  → returns the LLM-generated text
```

### Key Implementation Details

**Kernel Socket FFI** — No Rust networking bindings exist in Linux 6.19.8. Wrote thin unsafe wrappers around:
- `sock_create_kern()` — creates kernel-owned TCP socket in init_net namespace
- `kernel_connect()` — connects to vLLM server. Required defining `SockaddrIn` manually (not in bindgen output)
- `kernel_sendmsg()` / `kernel_recvmsg()` — send/receive via `kvec` + zero-initialized `msghdr`
- `sock_release()` — cleanup via RAII `Drop` impl on `KernelSocket` wrapper

**HTTP Client** — Minimal HTTP/1.1 client built from raw bytes:
- Sends `Connection: close` to ensure clean socket lifecycle
- Formats `Content-Length` without sprintf (custom `format_usize`)
- Parses response status code and locates body after `\r\n\r\n`

**JSON Handling** — No serde in kernel space:
- `json_escape()`: escapes prompt for JSON embedding (handles `\ " \n \r \t`)
- `extract_text_from_json()`: finds `"text":"` pattern and extracts string value
- `json_unescape()`: unescapes the extracted text back to raw bytes
- `find_json_string_end()`: handles escaped characters within JSON strings

**Error Handling** — Graceful degradation:
- Connection refused (errno 111): human-readable hint about vLLM not running
- Connection timeout (errno 110): hint about network issues
- Non-200 HTTP status: returns the response body as error message
- JSON parse failure: returns raw response body as fallback

**Design Decisions**:
- Blocking write: socket call runs in write_iter's process context (can sleep). Simpler than kthread for Step 2a. Future Step 2c will move to async kthread.
- Port 8000 hardcoded as const: matches vLLM default. Module parameters deferred.
- `sock_create_kern` over `sock_create`: creates kernel-owned socket not tied to calling process.
- Max 64KB response: prevents unbounded kernel memory allocation from vLLM responses.
- `Connection: close` header: forces vLLM to close connection after response, so `recv_all` terminates.

### Files Changed

- `hackbot-kmod/hackbot.rs` — Complete rewrite of inference path (100 → 460 lines)
- `hackbot-kmod/README.md` — Updated for Step 2a usage

---

## Step 2b: Kernel-Aware Inference with Live Context Injection

**Status**: Implementation complete (builds successfully)
**Date**: 2026-03-20

### Overview

Three changes in this step, each solving a real problem discovered during testing:

1. **Remote vLLM via Tailscale** — connected to keti GPU server (100.125.213.42) instead of localhost
2. **Global shared response buffer** — fixed cross-fd bug where `echo > /dev/hackbot` + `cat /dev/hackbot` lost the response
3. **Kernel context injection** — the LLM now receives live kernel state gathered from ring 0 APIs

### Architecture

```
write("/dev/hackbot", prompt)
  → hackbot.ko: gather_kernel_context()
      ├── kernel_version::KERNEL_RELEASE (compile-time from Kbuild)
      ├── ktime_get_boot_fast_ns() → uptime
      ├── __num_online_cpus → CPU count
      ├── si_meminfo() → memory stats
      └── Task::current_raw() → caller pid + comm
  → build prompt:
      [System Identity]
      [=== LIVE KERNEL STATE ===]
      [Kernel: Linux 6.19.8 x86_64]
      [Uptime: 3d 2h 15m 42s]
      [CPUs: 8 online]
      [Memory: 4217 MB used / 16384 MB total]
      [Caller: pid=1234 (bash)]
      [=========================]
      [User: <prompt>]
      [hackbot: ]
  → KernelSocket::connect_tcp(100.125.213.42:8000)
  → HTTP POST /v1/completions → vLLM on keti (Tailscale)
  → parse response → store in global RESPONSE buffer
read("/dev/hackbot")
  → read from global RESPONSE buffer (any fd can read)
```

### Key Implementation Details

**Remote vLLM via Tailscale**:
- `VLLM_ADDR` changed from `127.0.0.1` to `100.125.213.42` (Tailscale IP for keti GPU server)
- Tailscale encrypts traffic via WireGuard, so plaintext HTTP is acceptable
- IP address is single-source-of-truth: `append_ipv4()` helper generates the Host header and log messages from `VLLM_ADDR` constant — no hardcoded IP strings elsewhere
- keti runs `vllm serve facebook/opt-125m --port 8000`

**Global Shared Response Buffer** (cross-fd bug fix):
- **Root cause**: Per-fd state meant `echo > /dev/hackbot` (write fd) and `cat /dev/hackbot` (read fd) each got their own `HackbotDev` with independent buffers. The response was lost when the write fd closed.
- **Fix**: Device-global `SharedResponse` protected by kernel's `global_lock!` macro (`static RESPONSE: Mutex<SharedResponse>`)
- `write_iter`: reads prompt locally (no lock), calls vLLM (no lock held during network I/O), then briefly locks global to store result
- `read_iter`: locks global, copies data to userspace via `simple_read_from_buffer`, returns EOF if no response ready
- Per-fd `HackbotDev` is now lightweight (just holds device reference)
- Removed per-fd `Mutex<Inner>` and `CondVar` — no longer needed

**Kernel Context Injection** (giving the LLM "eyes"):
- `gather_kernel_context()` called on every prompt, returns live system state as formatted text
- Data sources (all ring 0 kernel APIs):

| Data | Kernel API | Notes |
|---|---|---|
| Kernel version | `KERNELRELEASE` | Compile-time via Kbuild-generated `kernel_version.rs` |
| Uptime | `ktime_get_boot_fast_ns()` | u64 nanoseconds → d/h/m/s format |
| CPU count | `__num_online_cpus` | Volatile read of atomic_t counter field |
| Memory | `si_meminfo()` | Fills `sysinfo` struct; totalram/freeram × mem_unit |
| Caller | `Task::current_raw()` | pid + comm from `task_struct` (benign race on comm) |

**Kbuild Integration** — `kernel_version.rs` auto-generated:
- `Kbuild` rule generates `kernel_version.rs` from `KERNELRELEASE` env var
- Contains `pub(crate) const KERNEL_RELEASE: &[u8] = b"6.19.8";`
- Ensures version string always matches the kernel the module was built for
- Added to `.gitignore` as build artifact

**System Prompt Structure**:
- `SYSTEM_IDENTITY` constant: agent identity and instructions
- `gather_kernel_context()`: live kernel state block
- `"User: "` prefix + user prompt
- `RESPONSE_PREFIX` (`"\nhackbot: "`): guides model output

### Design Decisions

- **Global vs per-fd state**: Global is correct for `echo`/`cat` workflow. Single-slot design — concurrent writers overwrite. Acceptable for single-user research tool.
- **No lock during vLLM call**: The global mutex is only held briefly for the memcpy of the response, not during the multi-second network round-trip. This keeps the device responsive.
- **read_iter returns EOF (not blocks) when no response**: Prevents `cat` from hanging forever on a fresh fd. The user runs `echo` first (which blocks during vLLM call), then `cat` reads the already-available response.
- **Architecture x86_64 hardcoded**: `init_uts_ns` is opaque in kernel Rust bindings, so machine arch can't be read at runtime. Hardcoded since module only targets x86_64.
- **compile-time kernel version**: `linux_banner` is not exported to modules. Kbuild generates the version at compile time from `KERNELRELEASE`, which is always correct (module won't load on mismatched kernel).

### Files Changed

- `hackbot-kmod/hackbot.rs` — Major rewrite: global response buffer, kernel context gathering, system prompt (460 → 700 lines)
- `hackbot-kmod/Kbuild` — Added kernel_version.rs generation rule
- `hackbot-kmod/.gitignore` — New: excludes build artifacts and generated files

---

## Step 2c: Dynamic OODA Agent Loop with Kernel Tools

**Status**: Implementation complete (builds successfully)
**Date**: 2026-03-20

### Overview

Step 2c transforms hackbot from a static single-shot responder into a dynamic agent with an OODA (Observe-Orient-Decide-Act) loop. The LLM can now request specific kernel data via tool calls, reason about it, and request more — a multi-turn investigation loop entirely within kernel space.

### Architecture

```
write("/dev/hackbot", prompt)
  → hackbot.ko: agent_loop(prompt)
      Build conversation:
        [System Identity + Tool Description]
        [=== LIVE KERNEL STATE ===]
        [User: <prompt>]
        [hackbot: ]

      OODA Loop (max 10 iterations):
        ┌─→ vllm_call(conversation) → HTTP POST to keti
        │   Parse response for <tool>name</tool>
        │   ├── Tool call detected:
        │   │   execute_tool(name) → kernel API call
        │   │   Append: reasoning + [Tool: name] output [End Tool]
        │   │   Continue loop ──────────────────────────┘
        │   └── No tool call (final answer):
        │       Return response text
        └── Max iterations → return accumulated text

read("/dev/hackbot")
  → read from global RESPONSE buffer
```

### Kernel Observation Tools (Tier 0 — Read-Only)

Three tools available to the LLM, all using ring 0 kernel APIs:

| Tool | Kernel API | Output |
|------|-----------|--------|
| `ps` | `init_task.tasks` two-pass walk under RCU | PID, PPID, state, comm — user-space first, kernel threads second (up to 512 tasks) |
| `mem` | `si_meminfo()` | Total/free/used/shared/buffers RAM + swap stats |
| `loadavg` | `avenrun[]` (EXPORT_SYMBOL) | 1/5/15 min load averages + CPU count + uptime |

**Tool Call Protocol**: The LLM outputs `<tool>name</tool>` to request data. vLLM's `stop` parameter is set to `["</tool>"]` so generation halts cleanly at the tool boundary. The parser also handles the case where the stop sequence strips the closing tag.

### Key Implementation Details

**Agent Loop** (`agent_loop()`):
- Replaces the single-shot `vllm_complete()` from Step 2b
- Uses `/v1/chat/completions` API with structured JSON messages array (system/user/assistant roles)
- vLLM applies the correct chat template automatically (ChatML for Qwen, etc.)
- Messages built incrementally via `append_message_to_json()` helper
- Loops: vLLM call → parse → tool dispatch → append assistant+user messages → re-prompt
- Bounded by `MAX_AGENT_ITERATIONS` (10) and `MAX_CONVERSATION_SIZE` (96 KB)
- Request includes `temperature: 0.6` and `repetition_penalty: 1.1` to reduce degenerate outputs
- Graceful degradation: if the model doesn't generate tool calls, returns the first response as final answer

**Tool Call Parser** (`parse_tool_call()`):
- Scans for `<tool>NAME</tool>` pattern using existing `find_subsequence()`
- Also handles `<tool>NAME` without closing tag (vLLM stop sequence removes it)
- Returns `ToolCallResult::ToolCall { name, prefix }` or `ToolCallResult::FinalAnswer(text)`
- `prefix` captures LLM reasoning before the tool call (included in conversation history)

**Process List Tool** (`tool_ps()`):
- Two-pass walk of `init_task.tasks` linked list under RCU protection:
  - **Pass 1**: User-space processes (`task->mm != NULL`) — ensures `bash`, `sleep`, etc. appear first
  - **Pass 2**: Kernel threads (`task->mm == NULL`) — `kswapd`, `ksoftirqd`, etc.
- Uses `core::mem::offset_of!(task_struct, tasks)` for container_of pattern
- Protected by `kernel::sync::rcu::Guard` (RAII RCU read-side lock)
- `format_task()` helper reads: `pid`, `real_parent->pid`, `__state`, `comm` from each `task_struct`
- State decoding: R (running), S (interruptible), D (uninterruptible), T (stopped/traced), Z (zombie), X (dead), I (idle)
- Bounded to `MAX_PS_TASKS` (512), with `MAX_TOOL_OUTPUT` (8 KB) as secondary truncation
- **Bug fix (2026-03-22)**: Original single-pass walk showed only kernel threads because `init_task.tasks` is creation-ordered (kernel threads first). With `MAX_PS_TASKS=64`, user-space processes were never reached. Two-pass walk ensures user-space processes are always visible.

**Load Average Tool** (`tool_loadavg()`):
- `avenrun[]` is EXPORT_SYMBOL but not in bindgen output for out-of-tree modules
- Declared manually as `extern "C" { static avenrun: [c_ulong; 3]; }`
- Fixed-point conversion: FSHIFT=11, so `integer = val / 2048`, `frac = (val % 2048) * 100 / 2048`
- Reads via `ptr::read_volatile()` for atomic semantics

**Conversation Format** (multi-turn via chat completions API):
```json
[
  {"role": "system", "content": "<system identity + tool description + live kernel context>"},
  {"role": "user", "content": "what's consuming memory?"},
  {"role": "assistant", "content": "Let me check the processes. <tool>ps</tool>"},
  {"role": "user", "content": "[Tool: ps]\nPID PPID STATE COMM\n1 0 S systemd\n...\n[End Tool]\nUse the tool output above to answer the user's question."},
  {"role": "assistant", "content": "Based on the process list, ..."}
]
```

**vLLM API (chat completions)**:
- Uses `/v1/chat/completions` endpoint (not `/v1/completions`) — vLLM applies chat template automatically
- `build_vllm_request()` takes pre-built JSON messages array
- `append_message_to_json()` incrementally builds the array (truncate `]`, append `,{...}]`)
- Request body: `{"model":"<auto-discovered>","messages":[...],"max_tokens":4096,"temperature":0.7,"repetition_penalty":1.1,"stop":["</tool>"]}`
- Each iteration makes a new TCP connection (Connection: close), keeping things simple
- `extract_text_from_json()` looks for `"content":"` pattern (chat completions response format)

### Design Decisions

- **XML tool tags over JSON**: `<tool>name</tool>` is simpler to parse and simpler for small models to generate than JSON function calls. No escaping issues.
- **Stop sequence `</tool>`**: Prevents the LLM from generating fake tool output after a tool call. The model generates `<tool>ps`, vLLM stops, we execute the real tool and inject its output.
- **Single tool per iteration**: Parsing only the first `<tool>` tag keeps the loop simple and predictable. Multiple tools per turn adds complexity with minimal benefit.
- **Chat completions over raw completions**: Instruct/chat models (Qwen, Llama, etc.) require proper role tokens to engage instruction-following behavior. Raw `/v1/completions` leads to degenerate text completion (hallucinated conversations, repetition loops). `/v1/chat/completions` applies the chat template automatically.
- **Structured messages**: Each iteration appends assistant + user messages to a JSON array. Bounded by MAX_CONVERSATION_SIZE (96 KB). The assistant message includes reasoning prefix to maintain chain-of-thought.
- **Graceful model degradation**: If the model never generates `<tool>` tags (e.g., OPT-125M base model), the loop exits on iteration 1 with a FinalAnswer — functionally identical to Step 2b.
- **Tool-call loop prevention**: Force final answer on last iteration — execute the tool and return raw output rather than empty "max iterations" message. Aggressive nudge prompts ("Do NOT call more tools") were tested and **removed** — they restrict the model's action space and produce worse results (placeholder output from 7B-AWQ). Permissive prompts work better: "Now analyze this data and answer the user's question."
- **Model auto-discovery**: `discover_model_name()` queries vLLM's `/v1/models` endpoint to get the actual served model name, so the module works with any model without hardcoding.
- **No few-shot conversation messages**: Few-shot examples as conversation messages caused the model to confuse example data with real historical data (referencing fake PIDs from examples). Format examples are placed inside the system prompt as instructional text instead.
- **RCU for task list**: The `kernel::sync::rcu::Guard` provides RAII RCU read-side protection. Tasks can't be freed while we're iterating.
- **No timeout (yet)**: Each vLLM call has an implicit TCP timeout. A total wall-clock timeout would require `ktime_get_boot_fast_ns()` tracking — deferred to avoid complexity.

### Files Changed

- `hackbot-kmod/hackbot.rs` — Major additions: tool infrastructure, 3 kernel tools, agent loop, two-pass ps (700 → ~1476 lines)

### Model Compatibility & Testing

| Model | Status | Notes |
|-------|--------|-------|
| `facebook/opt-125m` | Works (no tools) | Base model, no instruction-following. Degrades gracefully to Step 2b behavior. |
| `Qwen/Qwen2.5-1.5B-Instruct` | Partial | Follows tool format sometimes, but unreliable. Required switch to chat completions. |
| `Qwen/Qwen2.5-7B-Instruct-AWQ` | **Recommended** | Reliably calls tools, follows protocol. Occasional looping (handled by force-final-answer). |

```bash
# On keti:
vllm serve Qwen/Qwen2.5-7B-Instruct-AWQ --port 8000
```

### Lessons Learned (Prompt Engineering for Small Models)

1. **Don't restrict action space**: Nudge prompts like "Do NOT call any more tools" make 7B models produce placeholder output instead of analysis. Use permissive prompts.
2. **Don't use few-shot conversation messages**: Models confuse example data with real historical data. Put format examples in the system prompt as instructional text.
3. **Data quality > iteration count**: If the tool returns incomplete data, more loop iterations won't help. Fix the data source.
4. **Chat completions, not raw completions**: Instruct models require `/v1/chat/completions` with role tokens. Raw `/v1/completions` produces degenerate text completion.
5. **Tools last in system prompt**: Small models pay more attention to the end of the system prompt (recency bias). Place tool descriptions after kernel context.

---

## Step 3: In-Kernel INT8 Inference Engine (System 1)

**Status**: Planning complete, implementation not started
**Date**: 2026-03-22

### Overview

Step 3 swaps the inference substrate from remote vLLM (System 2) to a local CPU-only INT8 engine running inside the kernel module. The OODA tool interface from Step 2c is unchanged — we're replacing WHERE inference happens, not WHAT the agent can do.

**Target model**: [SmolLM2-135M-Instruct](https://huggingface.co/HuggingFaceTB/SmolLM2-135M-Instruct) — a 135M-parameter Llama-3 variant that is already instruction-tuned.

### Why SmolLM2-135M-Instruct (Not TinyStories)

| Factor | TinyStories-15M | SmolLM2-135M-Instruct |
|--------|----------------|----------------------|
| Can follow instructions | No | **Yes** |
| Can call tools | No (needs fine-tuning) | **Likely yes** |
| Testable use case | No | **Yes, from day one** |
| Binary format | llama2.c custom | HuggingFace safetensors |
| Tokenizer | GPT-Neo 10K | BPE 49K |
| Attention | MHA | **GQA** (3× smaller KV cache) |

TinyStories was considered as a simpler first step, but rejected because:
1. Different model format, tokenizer, and attention type — all throwaway work
2. Cannot test the actual use case (instruction following, tool calling)
3. Scalar integer is fast enough for 135M on the target hardware (~10-20 tok/s)

### Architecture

```
SmolLM2-135M-Instruct (Llama-3 variant):
  hidden_size:       576
  intermediate_size: 1536
  num_layers:        30
  num_attention_heads:    9  (Q heads)
  num_key_value_heads:    3  (KV heads, GQA 3:1)
  vocab_size:        49152
  max_position_embeddings: 2048
  activation:        SiLU
  normalization:     RMSNorm
  positional encoding: RoPE
```

### Hardware Target

```
AMD Ryzen 5 PRO 4650G (Zen 2), 6c/12t, 4.2 GHz
  L2: 512KB/core, L3: 8MB shared
  SIMD: SSE4.2, AVX, AVX2 (no AVX-512)
  RAM: 14GB DDR4 (~25 GB/s bandwidth)
```

### Performance Estimates

| Approach | tok/s | FPU needed? | Complexity |
|----------|-------|-------------|------------|
| **Scalar integer (Step 3)** | ~10-20 | No | Low |
| AVX2 integer SIMD (Step 3.5) | ~50-60 | Yes (`kernel_fpu_begin/end`) | Medium (C FFI) |

Scalar is fast enough for a research prototype. AVX2 is an optional optimization.

Model weights (135MB INT8) live in DRAM — memory bandwidth (~25 GB/s) is the real bottleneck, not compute. No architecture change fixes this.

### Implementation Substeps

**Step 3f: Model Export Tool** (Python, userspace)
- Convert SmolLM2-135M-Instruct from HuggingFace safetensors to hackbot binary format
- INT8 per-group quantization (group_size=32, matching runq.c scheme)
- Include BPE tokenizer vocab (49K entries) in the binary
- Output: `/lib/firmware/hackbot-model.bin` (~135MB)

**Step 3a: Weight Loading** (~200-300 lines Rust)
- `ModelConfig` struct matching SmolLM2 architecture
- `Firmware::request(c"hackbot-model.bin", &dev)` — kernel firmware API (exists in Linux 6.19.8 Rust bindings)
- Parse header, allocate weight buffers (`KVVec<i8>` for INT8, `KVVec<i32>` for fixed-point scales)
- Per-layer weight slices: rms_att, wq/wk/wv/wo (GQA dims), rms_ffn, gate/up/down projections
- Embedding table: 49152 x 576 (with embedding tying for output logits)

**Step 3b: Integer Math Primitives** (~300-400 lines Rust)
- `matmul_q8()`: INT8 x INT8 -> INT32 accumulation, fixed-point rescaling. Scalar loops, no SIMD.
- `rmsnorm_q()`: Integer sum-of-squares -> Newton's method integer sqrt -> divide
- `softmax_q()`: Fixed-point exp via 256-entry LUT, normalize
- `silu_q()`: x * sigmoid_approx(x), where sigmoid(x) ~ x / (1 + |x|)
- `rope_q()`: Precomputed fixed-point sin/cos table (allocated at module init)
- All operations in Q16.16 or Q8.24 fixed-point — zero floating point

**Step 3c: Transformer Forward Pass** (~400-500 lines Rust)
- `TransformerState`: KV cache (3 KV heads x seq_len x head_dim), activation buffers
- `forward(token, pos) -> logits`: single-token Llama-3 forward pass with GQA
- Grouped Query Attention: 9 Q heads share 3 KV heads (3:1 ratio)
- FFN: SiLU-gated — `silu(gate_proj(x)) * up_proj(x) -> down_proj`
- Residual connections, RMSNorm before attention and FFN

**Step 3d: Tokenizer + Text Generation** (~200-300 lines Rust)
- BPE tokenizer: load 49K vocab + merge scores from firmware file
- `encode()`: text -> token IDs (byte-pair merge loop)
- `decode()`: token ID -> text piece
- `generate()`: prompt -> tokenize -> forward x N -> argmax -> detokenize
- Stop on EOS token or max_tokens limit

**Step 3e: Agent Integration** (~100-150 lines Rust)
- `local_inference(prompt) -> Result<KVVec<u8>>` replacing `vllm_call()`
- Module parameter: `inference_mode` (0=vLLM/System 2, 1=local/System 1)
- Same `agent_loop()`, same tool parsing, same OODA loop
- Graceful fallback: if model firmware not found, use vLLM

### Build Order

```
3f (export model) -> 3a (load weights) -> 3b (math ops) -> 3c (forward pass) -> 3d (tokenizer) -> 3e (agent integration)
```

### Reference Implementations

- [llama2.c](https://github.com/karpathy/llm2.c) — `run.c` (float32) and `runq.c` (INT8) by Karpathy. Architecture blueprint.
- [KLLM](https://github.com/randombk/kllm) — Proof that kernel-space LLM inference works (GPT-2 124M, no SIMD, 1min/token).
- `linux-6.19.8/rust/kernel/firmware.rs` — Rust firmware loading API.

### Key Design Decisions

- **Scalar integer over AVX2**: Avoids kernel_fpu_begin/end complexity, mixed C/Rust build. Fast enough (~10-20 tok/s) for 135M on Zen 2. AVX2 deferred to Step 3.5.
- **SmolLM2-135M over TinyStories**: Instruction-tuned model that can follow tool-calling format from day one. No throwaway work on a different model format.
- **GQA from the start**: SmolLM2 uses Grouped Query Attention (9Q/3KV). Smaller KV cache, better for kernel memory. Implement this directly rather than MHA first.
- **Fixed-point non-linearities**: exp/sqrt/sigmoid via integer lookup tables and polynomial approximations. Zero floating-point operations in the hot path.
- **Firmware API for weights**: `kernel::firmware::Firmware` loads from `/lib/firmware/`. Standard kernel mechanism, handles module dependencies correctly.

### Architecture Context

See `docs/PLAN.md` Appendix B for the System 1/2 hybrid architecture analysis. Key insight (2026-03-20): inference substrate (WHERE) and agent capability (WHAT) are orthogonal axes. Build OODA tools first (Step 2c), then swap inference backend (Step 3).

**Safety**: Tiered capability system (Tier 0: observe -> Tier 3: modify kernel). Steps 2c-3 are Tier 0 only (read-only observation). Action capabilities (Tier 1+) deferred to Step 5, requiring Verus verification.

## Research: GPU/NPU Compute from Kernel Space (Linux 6.19.8)

**Date**: 2026-03-17
**Status**: Research complete. Conclusion: not feasible via existing APIs.

### Question

Can a Linux kernel module dispatch compute workloads (matrix multiply, inference) to GPU or NPU hardware?

### Findings

**The accel framework (`drivers/accel/`) is device registration plumbing only.** It creates `/dev/accel/accelN` char devices and hooks them into DRM. One exported symbol: `accel_open()`. No compute submission API.

**Every accelerator driver exports zero symbols.** Checked all six: ivpu (Intel NPU), habanalabs (Gaudi), amdxdna (AMD AI Engine), qaic (Qualcomm Cloud AI), rocket (RISC-V NPU), ethosu (Arm Ethos-U). All job submission is via driver-specific DRM ioctls, callable only from userspace.

**The DRM GPU Scheduler (`include/drm/gpu_scheduler.h`) is exported but is infrastructure for driver authors**, not a client API. Using it requires owning the hardware, implementing backend_ops, setting up GPU MMU, and managing firmware comms -- i.e., writing an entire GPU driver.

**GPU drivers export almost nothing useful for compute.** AMD exports ISP buffer helpers for camera pipeline. i915 exports GVT (virtualization) and thermal/power management hooks. Xe exports only KUnit test symbols.

**Raw DMA/MMIO is technically possible but practically insane.** Would require reimplementing the GPU firmware protocol, MMU page tables, command rings, and fighting the existing driver for device ownership.

### Architecture (by design)

```
Hardware <--> Kernel Driver (exclusive device owner) <--> Userspace (ioctls only)
```

There is no kernel-internal compute dispatch layer. This is deliberate: GPU drivers are complex state machines managing firmware, MMU, power, and recovery. No stable internal ABI exists for this.

### Implications for In-Kernel LLM

1. **CPU-only (kernel_fpu_begin/end + AVX)**: Feasible for tiny models. Must not sleep while holding FPU.
2. **Kernel module calls /dev/accel via filp_open/vfs_ioctl**: Possible but architecturally repulsive (kernel pretending to be userspace).
3. **Custom driver that unbinds existing NPU driver**: Enormous effort, architecturally honest.
4. **Userspace daemon + netlink/chardev IPC**: The correct answer for anything beyond toy-scale inference.

### Key Source Files Examined

- `drivers/accel/drm_accel.c` -- accel framework core (209 lines, 1 export)
- `include/drm/drm_accel.h` -- accel header (device registration macros only)
- `include/drm/gpu_scheduler.h` -- GPU scheduler (driver infrastructure, not client API)
- `include/drm/drm_gpuvm.h` -- GPU VM manager (driver infrastructure)
- `drivers/accel/ivpu/ivpu_job.c` -- IVPU job submission (ioctl-gated, requires drm_file)
- `drivers/accel/ivpu/ivpu_drv.h` -- IVPU device structure (all internal, no exports)
- `drivers/gpu/drm/amd/amdgpu/amdgpu_isp.c` -- only AMD exports (ISP camera buffer helpers)
- `include/linux/dma-mapping.h` -- DMA APIs (available but useless without driver cooperation)
