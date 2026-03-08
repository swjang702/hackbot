# hackbot — Detailed File Index

> Last updated: 2026-03-08
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
