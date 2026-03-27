# hackbot — Detailed File Index

> Last updated: 2026-03-27
> Lists all public types, functions, and their signatures.

---

## `server-rs/crates/hackbot-types/src/lib.rs`

Shared types crate. All WebSocket-serializable data structures.

### Enums

- **`EventType`** — 8 variants: `SyscallEnter`, `SyscallExit`, `SchedSwitch`, `PowerTrace`, `ProcessFork`, `ProcessExit`, `GpuSubmit`, `GpuComplete`. Serializes to `snake_case` strings.
- **`ProcessStatus`** — `Running`, `Sleeping`, `Exited`. Serializes to `snake_case`.
- **`ServerMessage`** — Tagged enum (`#[serde(tag = "msg")]`) with variants:
  - `WorldState { processes: Vec<Value>, connections: Vec<Value> }`
  - `Events { batch: Vec<Value> }`
  - `Playback { status: String, speed: f64, position_ns: String, duration_ns: String, start_ns: String }`
- **`ClientCommand`** — Tagged enum (`#[serde(tag = "cmd")]`) with variants:
  - `Load { file: String }`
  - `Play`
  - `Pause`
  - `Seek { position_ns: String }`
  - `Speed { multiplier: f64 }`
  - `Filter { pids: Option<Vec<u32>>, types: Option<Vec<String>> }`

### Structs

- **`TraceEvent`** — `{ ts: u64, event_type: EventType, pid: u32, tid: u32, cpu: u16, comm: String, payload: serde_json::Value }`
  - `fn to_ws_value(&self) -> Value` — Serializes with `ts` as string for JS BigInt safety.
- **`SyscallEnterPayload`** — `{ nr: i64, name: String, fd?: i64, count?: i64, path?: String, flags?: i64 }`
- **`SyscallExitPayload`** — `{ nr: i64, name: String, ret: i64 }`
- **`SchedSwitchPayload`** — `{ prev_pid: u32, next_pid: u32, prev_state: String }`
- **`PowerTracePayload`** — `{ watts: f64, domain: String }`
- **`ProcessForkPayload`** — `{ parent_pid: u32, child_pid: u32, child_comm: String }`
- **`ProcessExitPayload`** — `{ exit_code: i32 }`
- **`GpuSubmitPayload`** — `{ batch_size: u32, queue: String }`
- **`GpuCompletePayload`** — `{ batch_size: u32, queue: String, duration_ns: u64 }`
- **`ProcessInfo`** — `{ pid: u32, comm: String, parent_pid: Option<u32>, status: ProcessStatus, syscall_count: u64, gpu_submit_count: u64, last_event_ts: u64 }`
  - `fn new(pid: u32, comm: String) -> Self`
- **`ConnectionInfo`** — `{ from_pid: u32, to_pid: u32, conn_type: String, fd_from: Option<i32>, fd_to: Option<i32> }`

### Functions

- **`validate_payload(event_type: EventType, payload: &Value) -> bool`** — Validates a JSON payload against the expected typed struct for the given event type.

---

## `server-rs/crates/hackbot-server/src/main.rs`

Axum HTTP/WebSocket server entry point.

### Functions

- **`main()`** — `#[tokio::main]` entry. Initializes tracing, creates gateway state, auto-loads default trace, starts Axum server on `0.0.0.0:8000`. Handles `--generate-mock` CLI flag.
- **`root_handler() -> Json<Value>`** — `GET /` — Returns `{ name, version, description }`.
- **`traces_handler(State) -> Json<Value>`** — `GET /traces` — Returns `{ traces: [filename, ...] }`.
- **`ws_handler(WebSocketUpgrade, State) -> Response`** — `WS /ws` — Upgrades to WebSocket, delegates to `gateway::handle_ws`.

---

## `server-rs/crates/hackbot-server/src/gateway.rs`

WebSocket gateway — manages connections and routes messages.

### Structs

- **`GatewayState`** — Shared state behind `Arc<Mutex<>>`.
  - Fields: `traces_dir: PathBuf`, `events: Vec<TraceEvent>`, `replayer: Option<TraceReplayer>`, `world_model: WorldModel`, `broadcast_tx: broadcast::Sender<String>`.
  - `fn new(traces_dir: PathBuf) -> Self`
  - `fn trace_files(&self) -> Vec<String>` — Lists `.jsonl` files in traces directory.
  - `fn load_trace(&mut self, filename: &str) -> Result<Value, String>` — Loads trace, resets world model, creates replayer.
  - `fn playback_status_json(&self) -> Option<String>` — Current playback status as JSON.
  - `fn world_state_json(&self) -> String` — Current world state as JSON.

### Type Aliases

- **`SharedGateway`** = `Arc<Mutex<GatewayState>>`

### Functions

- **`handle_ws(socket: WebSocket, gateway: SharedGateway)`** — Per-client WebSocket handler. Sends initial world state, subscribes to broadcast, spawns read/write tasks.
- **`handle_command(raw: &str, gateway: &SharedGateway)`** — Parses JSON command, dispatches to load/play/pause/seek/speed/filter handlers.
- **`start_playback(gateway: SharedGateway)`** — Spawns playback loop + periodic world state broadcast as background tokio tasks.

---

## `server-rs/crates/hackbot-server/src/trace_loader.rs`

Loads and validates `.jsonl` trace files.

### Enums

- **`TraceLoadError`** — `NotFound(String)`, `Io(std::io::Error)`. Derives `thiserror::Error`.

### Functions

- **`load_trace(path: &Path) -> Result<Vec<TraceEvent>, TraceLoadError>`** — Reads `.jsonl` line-by-line, validates each event and payload, sorts by timestamp.
- **`get_trace_info(events: &[TraceEvent]) -> Value`** — Returns summary: `{ event_count, start_ns, end_ns, duration_ns, duration_s, pids, event_types }`.

---

## `server-rs/crates/hackbot-server/src/trace_replayer.rs`

Async trace replay engine with playback controls.

### Constants

- **`BATCH_WINDOW_NS: u64 = 16_000_000`** — 16ms batch window for 60fps rendering.

### Structs

- **`TraceReplayer`** — Async state machine for timed event replay.
  - `fn new(events: Arc<Vec<TraceEvent>>) -> Self`
  - `fn start_ns(&self) -> u64` — Trace start timestamp (absolute nanoseconds).
  - `fn position_ns(&self) -> u64` — Current position (absolute nanoseconds).
  - `fn elapsed_ns(&self) -> u64` — Elapsed from trace start.
  - `fn duration_ns(&self) -> u64` — Total trace duration.
  - `fn speed(&self) -> f64` — Current speed multiplier.
  - `fn is_playing(&self) -> bool`
  - `fn status(&self) -> &str` — `"playing"`, `"paused"`, or `"stopped"`.
  - `fn play(&mut self)` — Resume playback, notify waiting tasks.
  - `fn pause(&mut self)` — Pause playback.
  - `fn set_speed(&mut self, multiplier: f64)` — Clamp to 0.1..100.0.
  - `fn seek(&mut self, position_ns: u64)` — Binary search to absolute timestamp, notify to interrupt sleep.
  - `fn set_filter(&mut self, pids: Option<Vec<u32>>, types: Option<Vec<String>>)` — Set PID/type filters.
  - `fn reset(&mut self)` — Reset to beginning.
  - `async fn next_batch(&mut self) -> Option<Vec<TraceEvent>>` — Yields next event batch. Blocks when paused. Returns None when complete.

---

## `server-rs/crates/hackbot-server/src/world_model.rs`

Maintains world state from trace events.

### Structs

- **`WorldModel`** — Process map + fd table + connections.
  - `fn new() -> Self`
  - `fn reset(&mut self)` — Clear all state.
  - `fn process_event(&mut self, event: &TraceEvent)` — Update state from single event (dispatches to type-specific handler).
  - `fn process_events(&mut self, events: &[TraceEvent])` — Process a batch.
  - `fn get_world_state_dict(&self) -> Value` — Serialized world state with `msg: "world_state"`.
  - `fn rebuild_to(&mut self, events: &[TraceEvent], up_to_ts: u64)` — Reset and replay events up to timestamp (used for seek).

---

## `server-rs/crates/hackbot-server/src/mock_data.rs`

Mock trace data generator.

### Constants

- `BASE_TS: u64 = 1_709_380_800_000_000_000` — 2024-03-02 12:00:00 UTC in nanoseconds.
- `PARENT_PID: u32 = 100`, `WORKER_PIDS: [u32; 4] = [101, 102, 103, 104]`, `ANOMALY_PID: u32 = 200`
- `NUM_CPUS: u16 = 8`

### Functions

- **`generate_trace() -> Vec<Value>`** — Generates complete mock trace (~8912 events, deterministic with seed=42). Calls `generate_startup`, `generate_prefill`, `generate_decode`, `generate_anomaly`, `generate_recovery`.
- **`write_mock_trace(traces_dir: &Path) -> io::Result<usize>`** — Writes generated trace to `sample-llm-workload.jsonl`. Returns event count.
- `fn ns(seconds: f64) -> u64` — Converts seconds offset to absolute nanosecond timestamp.
- `fn jitter(rng, base_ns, max_us) -> u64` — Adds random jitter up to `max_us` microseconds.
- `fn make_event(rng, ts, event_type, pid, comm, payload, cpu) -> Value` — Creates a single event JSON value.
- `fn syscall_pair(rng, ts, pid, comm, name, nr, ret, duration_us, extra) -> Vec<Value>` — Creates matched `syscall_enter` + `syscall_exit` pair.
- `fn generate_startup(rng, events)` — Phase 0.0–1.0s: fork 4 workers, open/mmap.
- `fn generate_prefill(rng, events)` — Phase 1.0–3.0s: large GPU batches, high I/O, power spikes.
- `fn generate_decode(rng, events)` — Phase 3.0–4.0s: small regular GPU submits, periodic writes.
- `fn generate_anomaly(rng, events)` — Phase 4.0–4.5s: probe_tool reads `/proc/maps`, shared memory, perf_event_open.
- `fn generate_recovery(rng, events)` — Phase 4.5–5.0s: anomaly exits, normal operation resumes.

---

## `frontend/src/types.ts`

TypeScript type definitions mirroring `hackbot-types` Rust crate.

### Types

- **`EventType`** — String union: `"syscall_enter" | "syscall_exit" | "sched_switch" | "power_trace" | "process_fork" | "process_exit" | "gpu_submit" | "gpu_complete"`.
- **`ProcessStatus`** — `"running" | "sleeping" | "exited"`.
- **`TraceEvent`** — `{ ts: string, type: EventType, pid: number, tid: number, cpu: number, comm: string, payload: Record<string, unknown> }`.
- **`ProcessInfo`** — `{ pid, comm, parent_pid, status, syscall_count, gpu_submit_count, last_event_ts }`.
- **`ConnectionInfo`** — `{ from_pid, to_pid, type, fd_from, fd_to }`.
- **`WorldStateMessage`** — `{ msg: "world_state", processes: ProcessInfo[], connections: ConnectionInfo[] }`.
- **`EventsMessage`** — `{ msg: "events", batch: TraceEvent[] }`.
- **`PlaybackMessage`** — `{ msg: "playback", status, speed, position_ns, duration_ns, start_ns }`.
- **`ServerMessage`** — `WorldStateMessage | EventsMessage | PlaybackMessage`.
- **`ClientCommand`** — Discriminated union on `cmd` field: `load`, `play`, `pause`, `seek`, `speed`, `filter`.

---

## `frontend/src/connection.ts`

WebSocket client with auto-reconnect.

### Classes

- **`Connection`**
  - `constructor(url: string)`
  - `connect(): void` — Start connection with reconnect enabled.
  - `disconnect(): void` — Close and disable reconnect.
  - `send(cmd: ClientCommand): void` — Send JSON command if connected.
  - `get connected(): boolean` — WebSocket readyState === OPEN.
  - `onWorldState(handler): void` — Register world state handler.
  - `onEvents(handler): void` — Register events handler.
  - `onPlayback(handler): void` — Register playback handler.
  - Private: `_connect()` — Creates WebSocket, sets up reconnect on close (exponential backoff 1s→30s max), dispatches messages via `_dispatch()`.

---

## `frontend/src/app.ts`

App orchestrator.

### Classes

- **`App`**
  - `constructor()` — Creates Connection, GameWorld, Timeline, EventLog, Controls. Wires message handlers. Sets up connection status polling (1s interval).
  - `async init(): Promise<void>` — Initializes Pixi.js, connects WebSocket.

---

## `frontend/src/game/world.ts`

Pixi.js game world managing process rooms and event animations.

### Classes

- **`GameWorld`**
  - `constructor()` — Creates Pixi Application, world container, syscall pool, particle system.
  - `get zoom(): number` — Current camera zoom level.
  - `async init(container: HTMLDivElement): Promise<void>` — Initialize Pixi renderer, add camera, start animation ticker, observe resize.
  - `handleWorldState(msg: WorldStateMessage): void` — Creates/updates/removes ProcessRoom instances based on process list. Uses `computeLayout()` for positioning.
  - `handleEvents(msg: EventsMessage): void` — Spawns syscall objects in rooms, triggers particle bursts when event count per room >= `BURST_THRESHOLD` (5).

---

## `frontend/src/game/process-room.ts`

Process room — labeled rectangle with activity indicator.

### Classes

- **`ProcessRoom`**
  - `constructor(info: ProcessInfo, x, y, width, height)` — Creates container with background, border, label text, stats text.
  - `tick(dt: number): void` — Decays activity, updates border color via `lerpColor()`.
  - `addEvent(): void` — Spikes activity by 0.15 (clamped to 1.0).
  - `updateInfo(info: ProcessInfo): void` — Updates label and stats display.
  - `updateStatus(status: string): void` — Tints background by status color (green/gray/dim).
  - `getEventArea(): { x, y, w, h }` — Returns inner area for syscall object placement.

### Functions

- **`lerpColor(a: number, b: number, t: number): number`** — Linear interpolation between two RGB hex colors.

---

## `frontend/src/game/syscall-object.ts`

Syscall animation pool with 200 pre-allocated objects.

### Classes

- **`SyscallObjectPool`**
  - `constructor()` — Pre-creates 200 Graphics circles.
  - `spawn(event: TraceEvent, area: { x, y, w, h }): void` — Activates a pooled object at random position in area, colored by event type.
  - `tick(dt: number): void` — Updates animations: 0-20% scale up, 20-100% fade out. Returns completed objects to pool.

### Constants

- `POOL_SIZE = 200`, `ANIMATION_DURATION = 0.5s`, `OBJECT_RADIUS = 5`
- `EVENT_COLORS` — Maps syscall names and event types to hex colors (read=blue, write=green, open=yellow, etc.)

---

## `frontend/src/game/event-particle.ts`

Particle burst system for high-activity processes.

### Classes

- **`EventParticleSystem`**
  - `constructor()` — Pre-creates 100 particles.
  - `burst(x, y, color): void` — Spawns 8 particles at random angles from (x,y).
  - `tick(dt: number): void` — Updates particle positions and fades alpha over 0.6s lifetime.

### Constants

- `MAX_PARTICLES = 100`, `BURST_SIZE = 8`, `PARTICLE_LIFETIME = 0.6s`, `PARTICLE_SPEED = 60px/s`

---

## `frontend/src/game/camera.ts`

Pan and zoom camera controls.

### Classes

- **`Camera`**
  - `constructor(worldContainer: Container, canvas: HTMLCanvasElement)` — Binds pointer and wheel events.
  - `get zoom(): number`
  - `centerOn(worldX, worldY): void` — Centers view on world coordinate.
  - `reset(): void` — Reset zoom to 1.0 and position to origin.
  - Private: `_bindEvents()` — Pointer drag for pan, wheel for zoom (centered on cursor). Clamp 0.1x–5.0x.

### Constants

- `MIN_ZOOM = 0.1`, `MAX_ZOOM = 5.0`, `ZOOM_FACTOR = 0.1`

---

## `frontend/src/game/spatial-mapper.ts`

Tree layout algorithm for process rooms.

### Interfaces

- **`ProcessLayout`** — `{ x, y, width, height }`

### Functions

- **`computeLayout(processes: ProcessInfo[]): Map<number, ProcessLayout>`** — Builds tree from parent-child relationships, computes subtree widths, assigns (x,y) positions with parents centered over children. Filters out PID 0.

### Constants

- `ROOM_WIDTH = 160`, `ROOM_HEIGHT = 100`, `H_GAP = 24`, `V_GAP = 40`, `PADDING = 40`

---

## `frontend/src/ui/timeline.ts`

Playback timeline controls.

### Classes

- **`Timeline`**
  - `constructor(conn: Connection, refs: PanelRefs)` — Binds play/pause button, speed buttons, scrub slider events.
  - `handlePlayback(msg: PlaybackMessage): void` — Updates UI: play button text, active speed button, slider position (unless scrubbing), position text.

---

## `frontend/src/ui/event-log.ts`

Scrollable event log panel.

### Classes

- **`EventLog`**
  - `constructor(container: HTMLDivElement)` — Sets up scroll tracking for auto-scroll.
  - `setStartTimestamp(ns: string): void` — Sets trace start time for relative timestamp display.
  - `handleEvents(msg: EventsMessage): void` — Appends formatted entries (max 500, trims oldest). Auto-scrolls if user was near bottom.
  - `clear(): void` — Removes all entries.

### Functions

- `eventClass(event): string` — Returns CSS class by event type category.
- `formatEvent(event, startNs): string` — Formats event as human-readable string with relative timestamp.
- `formatSyscallArgs(payload): string` — Formats fd/count/path arguments.

### Constants

- `MAX_ENTRIES = 500`, `AUTO_SCROLL_THRESHOLD = 50px`

---

## `frontend/src/ui/controls.ts`

PID and event type filter controls.

### Classes

- **`Controls`**
  - `constructor(conn: Connection, refs: PanelRefs)` — Initializes type filter chips.
  - `handleWorldState(msg: WorldStateMessage): void` — Rebuilds PID filter chips when process list changes.
  - Private: `_rebuildPidFilters()` — Creates clickable chip per PID.
  - Private: `_initTypeFilters()` — Creates chips for event type categories. Paired toggles: syscall_enter/exit, gpu_submit/complete, process_fork/exit.
  - Private: `_sendFilter()` — Sends filter command. Sends null when all active (no filter).

---

## `frontend/src/ui/layout.ts`

DOM reference helper.

### Interfaces

- **`PanelRefs`** — `{ gameContainer, eventLog, pidFilters, typeFilters, playBtn, speedBtns, scrubSlider, timelinePosition, connectionStatus }`

### Functions

- **`getPanelRefs(): PanelRefs`** — Returns typed references to all UI DOM elements by ID.

---

## `frontend/index.html`

Single-page HTML with embedded CSS. CSS Grid layout:
- Left: game panel (Pixi.js canvas)
- Right sidebar: event log (scrollable) + filter controls (PID chips, type chips)
- Bottom full-width: timeline bar (play/pause, speed buttons 0.5x–10x, range slider, position text)

Dark theme using CSS variables (`--bg-primary: #0d1117`, etc.). Monospace font stack: JetBrains Mono → Fira Code → Cascadia Code.

---

## `frontend/vite.config.ts`

Vite dev server configuration. Port 5173. Proxies:
- `/ws` → `ws://localhost:8000` (WebSocket)
- `/traces` → `http://localhost:8000` (HTTP)

---

## `traces/format.md`

Trace format specification documenting the JSON Lines schema, all 8 event types with payload examples, and notes on timestamp handling (nanosecond u64, string serialization for JS safety).

---

# hackbot-kmod — Kernel Module

---

## `hackbot-kmod/hackbot_main.rs`

Root module file. Declares the hackbot kernel module and includes all submodules.

### Module Declaration

- **`module!`** — Metadata: name="hackbot", license="GPL", description="hackbot autonomous kernel agent with in-kernel LLM".

### Submodules

`config`, `types`, `state`, `context`, `net`, `tools`, `math`, `model`, `forward`, `tokenizer`, `agent`, `vllm`, `device` — all linked via `#[path]` attributes.

---

## `hackbot-kmod/hackbot_config.rs`

Configuration constants for the entire kernel module.

### Constants — vLLM

- **`VLLM_ADDR: u32`** — `[100, 66, 136, 70]` (remote vLLM server IP).
- **`VLLM_PORT: u16 = 8000`**
- **`MAX_RESPONSE_SIZE: usize = 65536`** — 64 KB max response.
- **`RECV_BUF_SIZE: usize = 4096`**
- **`IPPROTO_TCP: i32 = 6`**

### Constants — Agent Loop

- **`MAX_AGENT_ITERATIONS: usize = 10`**
- **`MAX_PS_TASKS: usize = 512`**
- **`MAX_TOOL_OUTPUT: usize = 8192`**
- **`MAX_CONVERSATION_SIZE: usize = 98304`**

### Constants — System Prompts

- **`SYSTEM_IDENTITY: &[u8]`** — Agent identity prompt.
- **`TOOL_DESCRIPTION: &[u8]`** — Tool usage guidance.
- **`LOCAL_SYSTEM_PROMPT: &[u8]`** — Compact prompt for local inference.
- **`LOCAL_MAX_ITERATIONS: usize = 3`**
- **`LOCAL_MAX_TOOL_OUTPUT: usize = 512`**

### Constants — Model Format

- **`MODEL_MAGIC: u32 = 0x484B4254`** — "HKBT" magic.
- **`MODEL_FORMAT_V1: u32 = 1`** — INT8 + Q16.16 fixed-point.
- **`MODEL_FORMAT_V2: u32 = 2`** — FP16 + float32 via FPU.
- **`MODEL_HEADER_SIZE: usize = 56`**
- **`MODEL_MAX_LAYERS: usize = 32`**, **`MODEL_MAX_VOCAB: usize = 65536`**
- **`INFERENCE_MAX_SEQ: usize = 256`**

### Constants — Token IDs

- **`TOKEN_ENDOFTEXT: u32 = 0`**, **`TOKEN_IM_START: u32 = 1`**, **`TOKEN_IM_END: u32 = 2`**

### Constants — Generation

- **`MAX_GEN_TOKENS: usize = 128`**, **`MAX_ENCODE_INPUT: usize = 1024`**, **`MAX_PREPROC_INPUT: usize = 2048`**

### Constants — Inference Modes

- **`INFERENCE_MODE: u32 = 0`** (auto), **`INFERENCE_MODE_LOCAL: u32 = 1`**, **`INFERENCE_MODE_VLLM: u32 = 2`**

---

## `hackbot-kmod/hackbot_types.rs`

Type definitions for model configuration, weight references, model state, and extern C FFI.

### Structs

- **`ModelConfig`** — `{ dim, hidden_dim, n_layers, n_heads, n_kv_heads, vocab_size, seq_len, group_size, head_dim, kv_dim, rope_theta }`.
- **`Q8Ref`** — INT8 quantized weight matrix reference: `{ data_off, scale_off, rows, cols }`.
- **`LayerRef`** — Transformer layer weight offsets: `{ rms_att_off, wq, wk, wv, wo, rms_ffn_off, gate, up, down }`.
- **`ModelSlot`** — Global model state: loaded flag, data pointer/length, config, layer refs, tokenizer offsets, inference buffers (x, xb, xb2, hb, hb2, q, att, logits, key_cache, value_cache), vocab index, FPU state pointer.
- **`SharedResponse`** — Device-global response buffer: `{ data: KVVec<u8>, offset: usize }`.

### Extern "C" Declarations

- **`avenrun: [c_ulong; 3]`** — Kernel load averages.
- **`hackbot_fpu_alloc(dim, hidden_dim, n_layers, n_heads, n_kv_heads, head_dim, vocab_size, max_seq) -> *mut c_void`**
- **`hackbot_fpu_free(state: *mut c_void)`**
- **`hackbot_fpu_reset(state: *mut c_void)`**
- **`hackbot_fpu_forward(state, weights, weights_len, token_id, pos) -> i32`**
- **`hackbot_fpu_get_next_token(state: *mut c_void) -> i32`**
- **`hackbot_console_init() -> i32`** — Register console driver for dmesg ring buffer.
- **`hackbot_console_exit()`** — Unregister console driver.
- **`hackbot_console_read(out: *mut u8, maxlen: i32) -> i32`** — Copy last N bytes from ring buffer.
- **`hackbot_list_fds(pid: i32, out: *mut u8, maxlen: i32) -> i32`** — List open FDs for a process.
- **`hackbot_kprobe_attach(symbol: *const u8, len: i32) -> i32`** — Attach kprobe to kernel function.
- **`hackbot_kprobe_check(out: *mut u8, maxlen: i32) -> i32`** — List active kprobes with hit counts.
- **`hackbot_kprobe_detach(symbol: *const u8, len: i32) -> i32`** — Remove a kprobe.
- **`hackbot_kprobe_cleanup()`** — Unregister all kprobes (called on rmmod).

---

## `hackbot-kmod/hackbot_state.rs`

Global mutable state shared across the module.

### Globals

- **`RESPONSE: Mutex<SharedResponse>`** — Device-global response buffer.
- **`MODEL: Mutex<ModelSlot>`** — Model firmware state.

---

## `hackbot-kmod/hackbot_device.rs`

MiscDevice implementation for `/dev/hackbot`.

### Structs

- **`HackbotModule`** — Pinned module registration container with `MiscDeviceRegistration`.
- **`HackbotDev`** — Per-fd device state holding `Device` reference.

### Trait Implementations

- **`InPlaceModule for HackbotModule`**
  - `init()` — Initialize globals, log vLLM endpoint.
- **`MiscDevice for HackbotDev`**
  - `open()` — Load model if needed.
  - `write_iter()` — Accept prompt bytes, call `process_prompt()`, store response.
  - `read_iter()` — Return buffered response data.

---

## `hackbot-kmod/hackbot_agent.rs`

Local OODA agent loop with ChatML format.

### Functions

- **`agent_loop_local(prompt: &[u8]) -> Result<KVVec<u8>>`** — Main agent loop: gathers kernel context, builds ChatML conversation, iteratively calls tools based on `<tool>` tags in model output, returns final answer.

### Private Functions

- `append_chat_tokens()` — Append ChatML-formatted message to token array.
- `begin_assistant_turn()` — Begin assistant turn in ChatML format.

---

## `hackbot-kmod/hackbot_vllm.rs`

Remote vLLM inference backend with agent loop dispatcher.

### Functions

- **`agent_loop(prompt: &[u8]) -> Result<KVVec<u8>>`** — Dispatcher: selects local or vLLM based on `INFERENCE_MODE` and model availability.
- **`process_prompt(prompt: &[u8]) -> KVVec<u8>`** — Process prompt through agent loop and format result.

### Private Functions

- `vllm_call(model_name, messages_json) -> Result<KVVec<u8>>` — Send request to vLLM `/v1/chat/completions`.
- `discover_model_name() -> Result<KVVec<u8>>` — Query `/v1/models` endpoint.
- `build_vllm_request(model_name, messages_json) -> Result<KVVec<u8>>` — Build HTTP POST request.
- `agent_loop_vllm(prompt) -> Result<KVVec<u8>>` — OODA loop with vLLM backend.

---

## `hackbot-kmod/hackbot_forward.rs`

Transformer forward pass with KV cache management.

### Functions

- **`alloc_inference_state(slot: &mut ModelSlot) -> Result`** — Allocate KV cache and activation buffers for both v1 (Q16.16) and v2 (FPU) formats.
- **`reset_kv_cache(slot: &ModelSlot)`** — Zero KV cache between conversations.
- **`forward_token(slot: &ModelSlot, token_id: usize, pos: usize)`** — Run one token through transformer. Dispatches to Q16.16 path (v1) or C FPU path (v2).

---

## `hackbot-kmod/hackbot_math.rs`

Q16.16 fixed-point math primitives (pure scalar integer, no FPU/SIMD).

### Constants

- **`Q16_ONE: i32 = 65536`**, **`TWO_PI_Q16: i64 = 411775`**
- **`EXP_TABLE: [i32; 17]`** — exp(-k) for k=0..16.
- **`SIN_TABLE: [i32; 256]`** — sin(2π·k/256).
- **`ROPE_FREQS_64: [i32; 32]`** — RoPE frequencies for head_dim=64.

### Functions

- **`isqrt_u64(n: u64) -> u64`** — Integer square root via Newton's method.
- **`exp_q16_neg(x: i32) -> i32`** — exp(-x) for non-positive x.
- **`sigmoid_q16(x: i32) -> i32`**, **`silu_q16(x: i32) -> i32`**
- **`sin_q16(angle_q16: i32) -> i32`**, **`cos_q16(angle_q16: i32) -> i32`** — Trig via table lookup + interpolation.
- **`matmul_q8(out, input, w_data, w_scales, rows, cols, gs)`** — INT8 × Q16.16 matrix-vector multiply.
- **`rmsnorm_q16(out, input, weight, dim)`** — RMS normalization.
- **`softmax_q16(x, len)`** — Softmax in-place.
- **`rope_apply_q16(vec, pos, head_dim)`** — Rotary position encoding.
- **`elementwise_mul_q16(out, a, b, len)`**, **`vec_add_q16(out, a, b, len)`**, **`silu_vec_q16(vec, len)`**, **`elementwise_mul_inplace_q16(a, b, len)`**
- **`argmax_q16(data, len) -> usize`** — Index of maximum value.

---

## `hackbot-kmod/hackbot_model.rs`

Model firmware loading and binary parsing.

### Functions

- **`read_u32_le(data: &[u8], off: usize) -> Result<u32>`**, **`read_u16_le(data: &[u8], off: usize) -> Result<u16>`** — Little-endian reads.
- **`q8_ref_advance(cursor, rows, cols, gs, data_len) -> Result<Q8Ref>`** — Advance cursor past Q8 weight matrix.
- **`norm_ref_advance(cursor, dim, data_len) -> Result<usize>`** — Advance cursor past RMSNorm weight.
- **`load_model_if_needed(dev: &Device)`** — Load firmware on first device open.
- **`free_model_resources()`** — Free all model allocations on module unload.

### Private Functions

- `parse_model_header(data: &[u8]) -> Result<ModelConfig>` — Parse binary header (magic, version, dimensions).
- `parse_and_store_model(data, slot) -> Result` — Parse tokenizer + weights, compute layer offsets.

---

## `hackbot-kmod/hackbot_tokenizer.rs`

GPT-2 BPE tokenizer for in-kernel use.

### Constants

- **`GPT2_BYTE_TO_CODEPOINT: [u16; 256]`** — Byte to Unicode codepoint mapping.
- **`GPT2_CODEPOINT_TO_BYTE: [u8; 324]`** — Reverse mapping.

### Functions

- **`decode_token_bytes(data, tok_offsets, token_id) -> &[u8]`** — Decode token ID to BPE bytes.
- **`get_token_score(data, tok_offsets, token_id) -> i32`** — Get BPE merge score.
- **`gpt2_decode_token(token_bytes, out) -> usize`** — Decode GPT-2 encoded bytes to raw bytes.
- **`find_token_by_bytes(data, tok_offsets, sorted, vocab_size, query) -> Option<u32>`** — Binary search sorted vocab.
- **`build_sorted_vocab(slot) -> Result`** — Build sorted vocab index + byte-to-token lookup.
- **`preprocess_gpt2(input, out) -> usize`** — GPT-2 byte preprocessing.
- **`encode_bpe(slot, input, out_tokens) -> usize`** — Encode bytes to BPE token IDs.
- **`get_next_token(slot) -> usize`** — Argmax from logits buffer.
- **`generate_from_tokens(slot, prompt_tokens, n_prompt, output, max_new_tokens) -> usize`** — Autoregressive generation from token array.
- **`generate(slot, prompt, output, max_new_tokens) -> usize`** — Generate from raw text prompt.

---

## `hackbot-kmod/hackbot_tools.rs`

Kernel tools (Tier 0 observation + Tier 1 instrumentation). 6 tools total.

### Enums

- **`ToolCallResult<'a>`** — `ToolCall { name, prefix }` | `FinalAnswer(text)`.

### Functions

- **`parse_tool_call(response: &[u8]) -> ToolCallResult`** — Parse `<tool>NAME args</tool>` tags from LLM output.
- **`execute_tool(raw: &[u8]) -> KVVec<u8>`** — Split name/args via `split_tool_args()`, dispatch to tool function.
- `split_tool_args(raw) -> (&[u8], &[u8])` — Split `"files 1234"` → `("files", "1234")`.
- `parse_usize(s: &[u8]) -> usize` — Parse decimal integer from ASCII bytes.

### Tool Implementations (Tier 0 — observation)

- `tool_ps(output)` — List processes via `for_each_process` RCU walk. Formats PID, PPID, state, comm.
- `tool_mem(output)` — Memory statistics via `si_meminfo()`: total, free, available, buffers, cached, swap.
- `tool_loadavg(output)` — Load averages from `avenrun[]` + uptime.
- `tool_dmesg(output, args)` — Read kernel log from console ring buffer. Optional line count arg.
- `tool_files(output, args)` — List open FDs for a PID via C helper `hackbot_list_fds()`.

### Tool Implementations (Tier 1 — instrumentation)

- `tool_kprobe(output, args)` — Dispatch kprobe subcommands: `attach <func>`, `check`, `detach <func>`.

### Private Helpers

- `format_task(output, task)` — Format single `task_struct`.

---

## `hackbot-kmod/hackbot_context.rs`

Kernel context gathering — provides system state to the LLM.

### Functions

- **`gather_kernel_context() -> KVVec<u8>`** — Gather and format live kernel state (version, uptime, CPUs, memory, current task).
- **`append_uptime(buf: &mut KVVec<u8>)`** — Format uptime as d/h/m/s.
- **`read_num_online_cpus() -> usize`** — Read online CPU count.

---

## `hackbot-kmod/hackbot_net.rs`

Kernel socket wrapper and HTTP/JSON utilities.

### Structs

- **`SockaddrIn`** — IPv4 socket address struct.
- **`KernelSocket`** — RAII wrapper around kernel socket with `Drop` impl.
  - `connect_tcp(addr: u32, port: u16) -> Result<Self>` — Create and connect TCP socket.
  - `send_all(&self, buf: &[u8]) -> Result<()>` — Send all bytes.
  - `recv(&self, buf: &mut [u8]) -> Result<usize>` — Receive into buffer.
  - `recv_all(&self, response, max_size) -> Result<()>` — Receive until EOF or max_size.

### Functions

- **`append_ipv4(buf, addr)`** — Format dotted-decimal IPv4.
- **`json_escape(input, output)`** — Escape special chars for JSON.
- **`append_message_to_json(messages, role, content)`** — Append chat message to JSON array.
- **`find_http_body(raw) -> &[u8]`** — Find HTTP response body.
- **`parse_http_status(raw) -> u16`** — Extract HTTP status code.
- **`extract_text_from_json(json) -> Option<&[u8]>`** — Extract `"text"`/`"content"` field.
- **`find_subsequence(haystack, needle) -> Option<usize>`**, **`find_json_string_end(json, start) -> Option<usize>`**
- **`json_unescape(escaped, output)`** — Unescape JSON string.
- **`format_usize(n, buf) -> &[u8]`** — Format usize as decimal ASCII (no heap).

---

## `hackbot-kmod/hackbot_fpu.c`

Float32 FPU forward pass (C). SmolLM2-135M with FP16 weights, float32 activations.

### Math Functions

- **`fp16_to_f32(h: u16) -> float`** — Software FP16 to float32 conversion.
- **`sqrtf_approx(x) -> float`** — Quake-style fast inverse square root.
- **`expf_approx(x) -> float`**, **`sinf_approx(x) -> float`**, **`cosf_approx(x) -> float`** — Fast math approximations.
- **`matmul_fp16(out, x, w_fp16, rows, cols)`** — Matrix-vector multiply with FP16 weights.
- **`rmsnorm_f32(out, x, weight_data, dim)`**, **`rope_f32(vec, pos, head_dim)`**, **`softmax_f32(x, len)`**, **`silu_f32(x)`**

### Public API (called from Rust via FFI)

- **`hackbot_fpu_alloc(...) -> void*`** — Allocate inference state (KV cache + activation buffers).
- **`hackbot_fpu_free(state)`** — Free all allocations.
- **`hackbot_fpu_reset(state)`** — Zero KV cache.
- **`hackbot_fpu_forward(state, weights, weights_len, token_id, pos) -> int`** — Forward pass with `kernel_fpu_begin/end` guards.
- **`hackbot_fpu_get_next_token(state) -> int`** — Temperature + top-k sampling (T=0.70, K=40) with kernel CSPRNG. Set `HACKBOT_TEMPERATURE=0` for greedy argmax.

### Configuration Constants

- `HACKBOT_TEMPERATURE` (70 = 0.70) — sampling temperature, 0 = greedy
- `HACKBOT_TOP_K` (40) — number of top candidates to consider

### Private

- `forward_token_impl(st, weights, token_id, pos)` — Core transformer (embedding → layers → logits). Debug logging at pos==0.

---

## `hackbot-kmod/hackbot_fpu.h`

C header for FPU inference engine. Declares `hackbot_fpu_alloc`, `hackbot_fpu_free`, `hackbot_fpu_reset`, `hackbot_fpu_forward`, `hackbot_fpu_get_next_token`.

---

## `hackbot-kmod/hackbot_console.c`

Console ring buffer — captures kernel log messages for the `dmesg` tool.

### Functions

- **`hackbot_console_init() -> int`** — Register console driver. Call on module init.
- **`hackbot_console_exit()`** — Unregister console driver. Call on module exit.
- **`hackbot_console_read(out, maxlen) -> int`** — Copy last `maxlen` bytes from 64KB ring buffer.

### Private

- `hackbot_console_write(con, s, count)` — Console write callback. Runs in ANY context (IRQ-safe via raw_spinlock).

---

## `hackbot-kmod/hackbot_files.c`

FD listing — walks process file descriptor table for the `files` tool.

### Functions

- **`hackbot_list_fds(pid, out, maxlen) -> int`** — List open FDs for process. Returns bytes written or `-ESRCH`/`-ENOMEM`.

### Private

- `append_num(out, pos, maxlen, val) -> int` — Append decimal number to buffer.
- `append_str(out, pos, maxlen, s, slen) -> int` — Append string to buffer.

---

## `hackbot-kmod/hackbot_kprobe.c`

Kprobe manager — attach/check/detach kernel function probes for the `kprobe` tool.

### Functions

- **`hackbot_kprobe_attach(symbol, len) -> int`** — Register kprobe. Returns 0 or `-ENOSPC`/`-EEXIST`/`-ENOENT`.
- **`hackbot_kprobe_check(out, maxlen) -> int`** — List active kprobes with hit counts.
- **`hackbot_kprobe_detach(symbol, len) -> int`** — Unregister kprobe. Returns 0 or `-ENOENT`.
- **`hackbot_kprobe_cleanup()`** — Unregister ALL kprobes. Called on module unload.

### Private

- `hackbot_kprobe_pre_handler(p, regs) -> int` — Kprobe hit handler: `atomic64_inc(&count)`.
- `struct hackbot_kprobe_slot` — `{ active, symbol[64], count: atomic64_t, kp: struct kprobe }`.

---

## `hackbot-kmod/kernel_version.rs`

Auto-generated by Kbuild. Contains `pub(crate) const KERNEL_RELEASE: &[u8] = b"6.19.8"`.

---

# tools/ — Python Utilities

---

## `tools/export_hackbot.py`

Exports HuggingFace SmolLM2-135M-Instruct to hackbot binary format v1 (INT8 + Q16.16).

### Constants

- `MAGIC = 0x484B4254`, `FORMAT_VERSION = 1`, `Q16_SHIFT = 16`

### Functions

- **`quantize_tensor_q8(tensor: np.ndarray, group_size: int) -> tuple[np.ndarray, np.ndarray]`** — Quantize float tensor to INT8 with per-group Q16.16 scales.
- **`float_to_q16(arr: np.ndarray) -> np.ndarray`** — Convert float32 array to Q16.16 int32.
- **`export_tokenizer(tokenizer, vocab_size: int) -> bytes`** — Export vocabulary with BPE merge scores to binary.
- **`export_model(model_name: str, output_path: str, group_size: int = 64)`** — Main export: load HF model, quantize weights, write binary file.
- **`main()`** — CLI entry point.

---

## `tools/export_hackbot_fp16.py`

Exports SmolLM2-135M to hackbot binary format v2 (FP16 weights, no quantization).

### Constants

- `MAGIC = 0x484B4254`, `FORMAT_VERSION = 2`

### Functions

- **`export_tokenizer(tokenizer, vocab_size: int) -> bytes`** — Same as v1.
- **`export_model_fp16(model_name: str, output_path: str)`** — Export FP16 weights directly (no quantization).
- **`main()`** — CLI entry point.

---

## `tools/int8_reference.py`

Python reference implementation of hackbot INT8 forward pass.

### Constants

- `Q16_SHIFT`, `Q16_ONE`, `MODEL_MAGIC`, `INFERENCE_MAX_SEQ`, `ROPE_FREQS_64`, `TWO_PI_Q16`, `EXP_TABLE`, `SIN_TABLE`

### Functions

- **`sin_q16(theta)`**, **`cos_q16(theta)`**, **`isqrt_u64(n)`** — Q16.16 math matching kernel code.

### Classes

- **`HackbotModel`**
  - `__init__(self, path)` — Load and parse hackbot binary file.
  - `matmul_q8(self, x_q16, w_offset, rows, cols)` — INT8 × Q16.16 matmul matching kernel exactly.
  - `rmsnorm_q16(self, x_q16, weight_offset, dim)` — RMSNorm in Q16.16.
  - `rope_apply(self, vec, pos, head_dim)` — RoPE application.
  - `softmax_q16(self, x, length)` — Softmax in Q16.16.
  - `silu_q16(self, x)` — SiLU activation.
  - `forward_token(self, token_id, pos)` — Full forward pass for one token.

---

## `tools/verify_hackbot.py`

Comprehensive INT8 inference verification.

### Functions

- **`load_hackbot_bin(path: str)`** — Parse binary, return (data, config, tokens, offset).
- **`dequantize_q8(data, offset, rows, cols, group_size)`** — Dequantize INT8 weights to float32.
- **`compare_weights(bin_path: str, model_name: str)`** — Compare all weights + forward pass against HF reference.
- **`bytes_to_unicode()`** — GPT-2 bytes_to_unicode mapping.
- **`main()`** — CLI entry point.

---

## `tools/verify_prefill.py`

Multi-token prefill verification with numpy-vectorized INT8.

### Classes

- **`FastHackbotModel`**
  - `__init__(self, path)` — Load model and pre-load weights as numpy arrays.
  - `matmul_q8_fast(self, x_q16, w_i8, scales, rows, cols)` — Vectorized INT8 matmul.
  - `rmsnorm_fast(self, x_q16, weights)`, `rope_apply()`, `softmax_q16()`, `silu_q16_vec()`
  - `forward_token(self, token_id, pos, verbose=False)` — Full forward pass with numpy.
- **`main()`** — Single-token and full prefill tests with float32 comparison.

---

## `tools/verify_tokenizer.py`

BPE tokenizer verification against HuggingFace reference.

### Constants

- `GPT2_BYTE_TO_CODEPOINT[256]` — Matching kernel's byte-to-codepoint mapping.

### Classes

- **`HackbotTokenizer`**
  - `__init__(self, bin_path)` — Load tokenizer from binary, build sorted vocab.
  - `preprocess_gpt2(self, input_bytes)` — GPT-2 byte preprocessing matching kernel.
  - `encode_bpe(self, input_bytes)` — BPE encoding matching kernel's `encode_bpe` exactly.
  - `decode_tokens(self, token_ids)` — Decode token IDs to bytes.
- **`main()`** — Test encoding/decoding, compare with HuggingFace tokenizer.

---

## `tools/verify_generation.py`

Quick ChatML generation verification. Script-style (no function definitions). Loads HF model, applies ChatML template, generates greedy text, outputs token-by-token predictions.
