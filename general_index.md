# hackbot — General File Index

> Last updated: 2026-03-27

## Root

| File | Description |
|------|-------------|
| `CLAUDE.md` | Project instructions for Claude Code — architecture overview, build commands, implementation phases, research context. |
| `README.md` | Project README (currently mirrors CLAUDE.md content). |
| `.gitignore` | Git ignore rules (excludes `.claude/`, `.mcp.json`, `CLAUDE.md`). |

## `docs/` — Documentation

| File | Description |
|------|-------------|
| `docs/PLAN.md` | Full implementation plan — 5 phases, tech stack rationale, task breakdowns, design decisions. |
| `docs/ARCHITECTURE.md` | ASCII architecture diagrams — system overview, panel layout, data flow, WebSocket protocol, complex plane pipeline, mock trace narrative. |
| `docs/IMPLEMENTATION_REPORT.md` | Implementation summary of completed work — Phase 1 backend/frontend details, module descriptions, dependency table. |
| `docs/INVESTIGATION_REPORT.md` | Research investigation report from initial codebase analysis session. |
| `docs/FLOW_REPORT.md` | Code flow analysis report tracing execution paths through the system. |
| `docs/refs/refs.md` | Reference links and notes for research papers. |

## `server-rs/` — Rust Backend (Axum + Tokio)

| File | Description |
|------|-------------|
| `server-rs/Cargo.toml` | Cargo workspace root — defines workspace members (`hackbot-types`, `hackbot-server`) and shared dependencies. |
| `server-rs/crates/hackbot-types/Cargo.toml` | Package manifest for the shared types crate. Dependencies: serde, serde_json. |
| `server-rs/crates/hackbot-types/src/lib.rs` | All shared types — `TraceEvent`, `EventType` enum (8 variants), payload structs, `ProcessInfo`, `ConnectionInfo`, `ServerMessage`/`ClientCommand` WebSocket enums. Handles u64 timestamps serialized as strings for JS BigInt safety. |
| `server-rs/crates/hackbot-server/Cargo.toml` | Package manifest for the server binary. Dependencies: axum, tokio, tower-http, rand, tracing, futures-util, thiserror. |
| `server-rs/crates/hackbot-server/src/main.rs` | Axum HTTP/WebSocket server — routes (`GET /`, `GET /traces`, `WS /ws`), CORS, auto-loads default trace on startup, `--generate-mock` CLI flag. |
| `server-rs/crates/hackbot-server/src/gateway.rs` | WebSocket gateway — manages client connections via `broadcast` channel, handles play/pause/seek/speed/filter commands, runs playback loop as background tokio task, periodic world state broadcasts. |
| `server-rs/crates/hackbot-server/src/trace_loader.rs` | Loads `.jsonl` trace files — line-by-line parsing with serde, payload validation, sorts by timestamp. Returns trace summary info. |
| `server-rs/crates/hackbot-server/src/trace_replayer.rs` | Async trace replay engine — yields event batches at original timing (adjusted by speed), 16ms batch window for 60fps, play/pause via `tokio::sync::Notify`, seek via binary search, PID/type filtering. |
| `server-rs/crates/hackbot-server/src/world_model.rs` | World state model — HashMap-based process map, fd table, connection graph. Event handler dispatch via match on EventType. Supports `rebuild_to()` for seek. |
| `server-rs/crates/hackbot-server/src/mock_data.rs` | Mock trace generator — creates ~8912 deterministic events (seed=42) across 5 phases: startup, prefill, decode, anomaly, recovery. Writes `.jsonl` output. |

## `frontend/` — TypeScript + Vite + Pixi.js v8

| File | Description |
|------|-------------|
| `frontend/package.json` | NPM package config — deps: pixi.js ^8.6.6, devDeps: typescript ^5.7.0, vite ^6.2.0. |
| `frontend/tsconfig.json` | TypeScript config — ES2022 target, strict mode, bundler module resolution. |
| `frontend/vite.config.ts` | Vite config — dev server on port 5173, proxies `/ws` and `/traces` to `localhost:8000`. |
| `frontend/index.html` | Single-page HTML — CSS Grid layout with 3 panels (game view, sidebar with event log + filters, timeline bar), dark theme with CSS variables, inline styles. |
| `frontend/src/main.ts` | Entry point — creates and initializes the `App` instance. |
| `frontend/src/app.ts` | App orchestrator — wires `Connection`, `GameWorld`, `Timeline`, `EventLog`, and `Controls` together. Dispatches WebSocket messages to appropriate handlers. |
| `frontend/src/connection.ts` | WebSocket client — auto-reconnect with exponential backoff (1s→30s), typed message dispatch (`onWorldState`, `onEvents`, `onPlayback`), JSON command sending. |
| `frontend/src/types.ts` | TypeScript type definitions — mirrors `hackbot-types` Rust crate. `TraceEvent`, `ProcessInfo`, `ConnectionInfo`, `ServerMessage` union, `ClientCommand` union. |
| `frontend/src/game/world.ts` | Pixi.js game world — Application lifecycle, creates/updates process rooms from world state, spawns syscall animations, triggers particle bursts for high-activity processes. |
| `frontend/src/game/process-room.ts` | Process room renderer — labeled rectangle with activity-based border color (lerp dim↔bright), status-tinted background, stats text (syscall/GPU counts). |
| `frontend/src/game/syscall-object.ts` | Syscall animation pool — 200 pre-allocated Graphics circles, color-coded by event type, scale-up then fade-out animation (500ms). |
| `frontend/src/game/event-particle.ts` | Particle burst system — 100-particle pool, bursts of 8 at random angles, 0.6s lifetime, 60px/s speed, linear fade. |
| `frontend/src/game/camera.ts` | Camera controls — pointer-drag pan, scroll-wheel zoom (centered on cursor), clamped 0.1x–5x. |
| `frontend/src/game/spatial-mapper.ts` | Tree layout algorithm — builds process tree from parent-child relationships, computes subtree widths, centers parents over children. Constants: 160x100 rooms, 24px H-gap, 40px V-gap. |
| `frontend/src/ui/timeline.ts` | Timeline controls — play/pause toggle, speed button selection (0.5x–10x), scrub slider with BigInt position math, position text display. |
| `frontend/src/ui/event-log.ts` | Event log panel — max 500 entries, auto-scroll when near bottom, color-coded by type (CSS classes), relative timestamp formatting. |
| `frontend/src/ui/controls.ts` | Filter controls — dynamically generated PID filter chips from world state, event type filter chips with paired toggle (syscall_enter/exit, gpu_submit/complete, process_fork/exit). |
| `frontend/src/ui/layout.ts` | DOM reference helper — `getPanelRefs()` returns typed references to all UI elements. |

## `traces/` — Trace Data

| File | Description |
|------|-------------|
| `traces/sample-llm-workload.jsonl` | Mock trace data — ~8912 events across 5s, LLM inference workload narrative (generated by `cargo run -- --generate-mock`). |
| `traces/format.md` | Trace format specification — JSON Lines schema, event type definitions with payload examples, notes on timestamp handling. |

## `hackbot-kmod/` — Linux Kernel Module (Rust + C)

In-kernel LLM agent module for Linux 6.19.8. Implements SmolLM2-135M inference at ring 0 with an OODA agent loop and kernel observation tools. Exposes `/dev/hackbot` character device.

| File | Description |
|------|-------------|
| `hackbot-kmod/hackbot_main.rs` | Root module file — declares kernel module and includes all submodules via `#[path]` attributes. |
| `hackbot-kmod/hackbot_config.rs` | Configuration constants — vLLM server address/port, agent loop limits, system prompts, model format magic/versions, token IDs, inference mode flags. |
| `hackbot-kmod/hackbot_types.rs` | Type definitions — `ModelConfig`, `Q8Ref` (INT8 weight reference), `LayerRef` (transformer layer offsets), `ModelSlot` (global model state), `SharedResponse`, extern C FFI declarations for FPU functions. |
| `hackbot-kmod/hackbot_state.rs` | Global mutable state — `MODEL` and `RESPONSE` mutexes shared across the module via `global_lock!` macro. |
| `hackbot-kmod/hackbot_device.rs` | MiscDevice `/dev/hackbot` — `open()` loads model, `write_iter()` accepts prompt and runs agent loop, `read_iter()` returns response. |
| `hackbot-kmod/hackbot_agent.rs` | Local OODA agent loop — ChatML-formatted iterative tool calling with in-kernel SmolLM2-135M inference. |
| `hackbot-kmod/hackbot_vllm.rs` | Remote vLLM inference backend — agent loop dispatcher (local vs remote), HTTP request building, `/v1/chat/completions` and `/v1/models` API calls via TCP socket. |
| `hackbot-kmod/hackbot_forward.rs` | Transformer forward pass — KV cache allocation, inference state management, supports both INT8/Q16.16 (v1) and float32/FPU (v2) paths. |
| `hackbot-kmod/hackbot_math.rs` | Q16.16 fixed-point math — pure scalar integer arithmetic for matmul, RMSNorm, RoPE, softmax, SiLU, sigmoid. No FPU/SIMD. |
| `hackbot-kmod/hackbot_model.rs` | Model firmware loading — binary header parsing, weight offset computation, tokenizer extraction, resource cleanup. |
| `hackbot-kmod/hackbot_tokenizer.rs` | GPT-2 BPE tokenizer — encoding, decoding, vocabulary binary search, sorted index construction, autoregressive text generation. |
| `hackbot-kmod/hackbot_tools.rs` | Kernel tools (Tier 0-1) — 6 tools: `ps`, `mem`, `loadavg`, `dmesg`, `files`, `kprobe`. Parses `<tool>NAME args</tool>` tags, dispatches with `split_tool_args()`. |
| `hackbot-kmod/hackbot_context.rs` | Kernel context gathering — formats kernel version, uptime, CPU count, memory info, current task for the LLM's system context. |
| `hackbot-kmod/hackbot_net.rs` | Kernel socket wrapper — TCP connect/send/recv, HTTP helpers, JSON escape/unescape, IPv4 formatting, usize-to-ASCII conversion. |
| `hackbot-kmod/hackbot_fpu.c` | Float32 FPU forward pass (C) — SmolLM2-135M transformer with FP16 weights, float32 activations, `kernel_fpu_begin/end` guards, temperature+top-k sampling in `get_next_token`. |
| `hackbot-kmod/hackbot_fpu.h` | C header for FPU inference — declares `hackbot_fpu_alloc/free/reset/forward/get_next_token` called from Rust via FFI. |
| `hackbot-kmod/hackbot_console.c` | Console ring buffer (C) — registers `struct console` to capture all printk into 64KB circular buffer for the `dmesg` tool. |
| `hackbot-kmod/hackbot_console.h` | C header for console ring buffer — `hackbot_console_init/exit/read`. |
| `hackbot-kmod/hackbot_files.c` | FD listing (C) — walks `task->files->fdtable` via `find_vpid()` + `d_path()` for the `files` tool. Max 256 FDs. |
| `hackbot-kmod/hackbot_files.h` | C header for FD listing — `hackbot_list_fds(pid, out, maxlen)`. |
| `hackbot-kmod/hackbot_kprobe.c` | Kprobe manager (C) — attach/check/detach kprobes with atomic64 hit counters, max 8 slots, cleanup on rmmod. |
| `hackbot-kmod/hackbot_kprobe.h` | C header for kprobe manager — `hackbot_kprobe_attach/check/detach/cleanup`. |
| `hackbot-kmod/kernel_version.rs` | Auto-generated — kernel release string constant (`"6.19.8"`), generated by Kbuild. |
| `hackbot-kmod/Kbuild` | Kernel build config — mixed C+Rust compilation, enables `-mhard-float -msse -msse2` for C FPU code, auto-generates `kernel_version.rs`. |
| `hackbot-kmod/Makefile` | Build harness — targets: `default` (build), `clean`, `load` (insmod), `unload` (rmmod), `test` (smoke test). |

## `tools/` — Python Utilities

Model export and verification scripts for the in-kernel LLM.

| File | Description |
|------|-------------|
| `tools/export_hackbot.py` | Exports HuggingFace SmolLM2-135M to hackbot binary format v1 (INT8 quantized weights with Q16.16 fixed-point scales). |
| `tools/export_hackbot_fp16.py` | Exports SmolLM2-135M to hackbot binary format v2 (FP16 weights, no quantization) for better precision on longer sequences. |
| `tools/int8_reference.py` | Python reference implementation of INT8 forward pass using Q16.16 arithmetic — verifies correctness against float32. |
| `tools/verify_hackbot.py` | Comprehensive verification of hackbot INT8 inference — compares all weights and forward pass output against HuggingFace reference model. |
| `tools/verify_prefill.py` | Multi-token prefill verification using numpy-vectorized INT8 reference — diagnoses divergence in longer sequences. |
| `tools/verify_generation.py` | Quick ChatML generation verification using float32 HuggingFace model as baseline. |
| `tools/verify_tokenizer.py` | Verifies hackbot's BPE tokenizer against HuggingFace reference by replicating the kernel's `encode_bpe` exactly. |
| `tools/verify_fp16_forward.py` | FP16 (v2) forward pass verification — reads binary model, reimplements C forward pass in Python, compares vs HuggingFace at every step. Supports `--prompt` for multi-token tests. |

## `server/` — [DEPRECATED] Python Backend

| File | Description |
|------|-------------|
| `server/pyproject.toml` | Python project config (uv). Deps: fastapi, uvicorn, websockets, pydantic, numpy. |
| `server/server/schemas.py` | Pydantic models — `TraceEvent`, `EventType` enum, 8 payload types, `WorldState`, WebSocket message models. |
| `server/server/mock_data.py` | Python mock trace generator — same narrative as Rust version, ~8589 events (different RNG). |
| `server/server/trace_loader.py` | Python trace loader — `.jsonl` parsing, validation, sorting. |
| `server/server/world_model.py` | Python world model — process map, fd table, event handler dispatch. |
| `server/server/trace_replayer.py` | Python async replayer — asyncio-based play/pause/seek, 16ms batching. |
| `server/server/gateway.py` | Python WebSocket gateway — FastAPI WebSocket handler, broadcast, command routing. |
| `server/server/main.py` | Python FastAPI app — HTTP routes, WebSocket endpoint, startup trace loading. |
