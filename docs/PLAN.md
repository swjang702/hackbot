# hackbot Implementation Plan

**Date**: 2026-03-02 (updated 2026-03-20)
**Status**: Phase 1 complete, in-kernel LLM through Step 2c (OODA agent loop with kernel tools)
**Guiding Principle**: Visualization-first MVP, Rust backend for Verus alignment

---

## 1. Executive Summary

hackbot is an autonomous kernel exploration agent whose journey through system internals is rendered as a game-like visual experience. The researcher (Sunwoo Jang) brings deep eBPF tracing expertise from professional work on LLM workload profiling and prior security research (SecQuant container isolation, Genians EDR). The long-term vision spans four pillars from the Research Statement: (1) autonomous bot in the kernel, (2) real-time visualization as gameplay, (3) complex plane signal mapping for anomaly detection, and (4) mathematical formulation for self-improvement.

The MVP focuses exclusively on **visualization** because:
- Data collection capability already exists (Sunwoo's professional eBPF work)
- Visualization makes every other component testable and debuggable
- The game-like rendering is the project's unique differentiator among tracing tools
- It aligns with the rs-sdk inspiration pattern: build the visual client first, then add agent intelligence

The MVP will transform eBPF trace data into a navigable 2D game world where processes are rooms, system calls are visible events, and (eventually) an AI agent is a character exploring the system.

---

## 2. MVP Scope

### What the MVP IS

A browser-based visualization tool that:
1. Loads pre-recorded eBPF trace files (system calls, power events, scheduling)
2. Renders a 2D game-like world where processes are spatial rooms and events are animated objects
3. Provides a complex plane view that maps trace signals to orbits, highlighting anomalies as phase shifts
4. Supports timeline playback with scrubbing, pause, and speed control
5. Includes an event log with filtering by PID and event type

### What the MVP is NOT

- Not a real-time monitoring dashboard (replay-first; real-time comes in Phase 3)
- Not an autonomous agent (the bot character comes in Phase 4; LLM brain in Phase 5)
- Not a production security tool (it is a research visualization instrument)
- Not a replacement for existing tracing tools (it adds a visual layer on top of existing trace data)

### MVP Exit Criteria

The MVP is complete when a user can:
1. Load a trace file containing LLM workload events
2. See processes rendered as labeled rooms in a 2D world with pan/zoom
3. Watch system call events animate within those rooms during playback
4. View the same data as a complex plane orbit plot with anomaly highlighting
5. Scrub the timeline and filter events by PID or event type

---

## 3. Technical Architecture

### 3.1 System Overview

```
+================================================================+
|                    BROWSER (Web Client)                         |
|                                                                 |
|  +---------------------------+  +----------------------------+  |
|  | GAME VIEW (Pixi.js)       |  | SIGNAL VIEW (Canvas 2D)    |  |
|  | 2D world with process     |  | Complex plane orbit plot   |  |
|  | rooms, syscall objects,   |  | Phase diagram over time    |  |
|  | event particles            |  | Anomaly highlights         |  |
|  +---------------------------+  +----------------------------+  |
|                                                                 |
|  +---------------------------+  +----------------------------+  |
|  | EVENT LOG                 |  | CONTROLS                    |  |
|  | Scrollable, filterable    |  | Play/Pause/Speed/Scrub      |  |
|  | event stream              |  | PID and event type filters  |  |
|  +---------------------------+  +----------------------------+  |
+================================================================+
                              |
                              | WebSocket (JSON messages)
                              |
+================================================================+
|                    GATEWAY SERVER (FastAPI + Python)             |
|                                                                 |
|  trace_loader.py      - Parse trace files                       |
|  trace_replayer.py    - Emit events at correct timing           |
|  world_model.py       - Build process map and world state       |
|  signal_processor.py  - Complex plane computation               |
|  schemas.py           - Event and message type definitions       |
|  mock_data.py         - Generate sample traces for development  |
+================================================================+
                              |
                              | Reads from file (MVP) or
                              | eBPF ring buffer (Phase 3)
                              |
+================================================================+
|              TRACE DATA (JSON Lines files)                       |
|              or TARGET SYSTEM (Phase 3+)                        |
+================================================================+
```

### 3.2 Data Flow

**MVP (Phases 1-2): Replay mode**
```
Trace file (.jsonl) -> trace_loader -> trace_replayer -> WebSocket -> Browser
                                    -> world_model (state)
                                    -> signal_processor (complex plane)
```

**Phase 3+: Live mode**
```
Kernel -> eBPF probes -> ring buffer -> event ingestion -> WebSocket -> Browser
                                     -> world_model
                                     -> signal_processor
```

### 3.3 Key Interface: Trace Event Schema

Every event in the system follows this JSON structure (one event per line in .jsonl files):

```json
{
  "ts": 1709384000000000000,
  "type": "syscall_enter",
  "pid": 1234,
  "tid": 1234,
  "cpu": 0,
  "comm": "python3",
  "payload": {
    "nr": 1,
    "name": "write",
    "fd": 1,
    "count": 4096
  }
}
```

Supported event types for MVP:
- `syscall_enter` / `syscall_exit` - system call entry and return
- `sched_switch` - process scheduling event
- `power_trace` - power consumption reading (from hardware counters)
- `process_fork` / `process_exit` - process lifecycle
- `gpu_submit` / `gpu_complete` - GPU work submission and completion

The schema is extensible: new event types can be added without breaking existing consumers, because each type carries a type-specific payload.

### 3.4 Key Interface: WebSocket Messages (Server -> Client)

```json
{"msg": "world_state", "processes": [...], "connections": [...]}
{"msg": "events", "batch": [{"ts": ..., "type": ..., ...}, ...]}
{"msg": "signal", "z_real": 0.5, "z_imag": 0.3, "theta": 1.2, "anomaly": false, "deviation": 0.05}
{"msg": "playback", "status": "playing", "speed": 1.0, "position_ns": 1709384000000000000}
```

### 3.5 Key Interface: WebSocket Messages (Client -> Server)

```json
{"cmd": "load", "file": "sample-llm-workload.jsonl"}
{"cmd": "play"}
{"cmd": "pause"}
{"cmd": "seek", "position_ns": 1709384000000000000}
{"cmd": "speed", "multiplier": 2.0}
{"cmd": "filter", "pids": [1234, 5678], "types": ["syscall_enter", "power_trace"]}
```

---

## 4. Technology Stack

| Component | Technology | Justification |
|-----------|-----------|---------------|
| **Frontend framework** | TypeScript + Vite | Type safety for complex visualization code. Vite for fast dev iteration. |
| **2D rendering** | Pixi.js v8 | WebGL-accelerated sprite-based 2D renderer. Ideal for game-like visuals (process rooms, event particles, agent character). |
| **Signal view rendering** | HTML5 Canvas 2D API | The complex plane plot and phase diagram are mathematical charts, not game objects. Raw Canvas is simpler. |
| **Backend framework** | Rust + Axum + Tokio | High-performance async WebSocket server. Aligns with Verus formal verification goals (Pillar 4). Single binary deployment. Native eBPF support via `aya` (Phase 3). |
| **Signal processing** | ndarray + rustfft | Rust-native numerical computing for complex plane computation. Research prototyping done in Python/Jupyter notebooks, production code in Rust. |
| **Data serialization** | serde + serde_json, JSON Lines (trace files) | Zero-copy deserialization. Human-readable for debugging. |
| **Package management** | Cargo (Rust), pnpm (TypeScript) | Cargo workspace for multi-crate backend. pnpm for frontend. |
| **Dev tooling** | clippy + rustfmt (Rust), ESLint + Prettier (TypeScript) | Standard tooling for each language. |

### Why Rust over Python (Decision Record, 2026-03-08)

The initial Phase 1 prototype used Python/FastAPI. The decision to rewrite in Rust is driven by:

1. **Verus alignment**: Research Pillar 4 targets formally verified system abstractions. Verus verifies Rust code. Writing in Python means rewriting for verification later under deadline pressure.
2. **eBPF native path**: Phase 3 uses `aya` — kernel-side eBPF programs and userspace loader both in Rust. No C/Python boundary.
3. **Small rewrite cost**: The Python backend is ~850 lines. Porting now costs 1-2 days. At Phase 3 it would be 3000+ lines.
4. **Single binary deployment**: `cargo build --release` produces one static binary. No Python environment management on research machines.
5. **Performance headroom**: Live eBPF streaming (Phase 3) at millions of events/sec would stress Python. Rust handles this natively.

The frontend remains TypeScript — Rust/WASM for UI has no advantage over the TypeScript ecosystem.

### Technologies Explicitly NOT Chosen

| Rejected | Reason |
|----------|--------|
| Python/FastAPI | Initial prototype language. Replaced by Rust for Verus alignment and eBPF native support. |
| Three.js / WebGL direct | Overkill for 2D visualization. Pixi.js abstracts WebGL for 2D use cases. |
| Phaser / Godot (web) | Full game engines. Too heavyweight for a visualization tool. |
| Grafana / existing dashboards | Chart-based, not spatial/world-based. Cannot render the game-like experience. |
| Go backend | Good for eBPF (cilium/ebpf) but lacks Verus-equivalent formal verification tooling. |
| React/Vue/Svelte | The visualization is Canvas/WebGL-based, not DOM-based. Simple HTML + TypeScript modules suffice. |

---

## 5. Directory Structure

```
hackbot/
  docs/                              # [EXISTS] Research documents
    PLAN.md                          # This file
    ARCHITECTURE.md                  # System architecture diagrams
    refs/
      Research_Statement.pdf
      Connecting the dots...pdf
      mynote.jpg
      refs.md

  server/                            # [DEPRECATED] Python backend (kept for reference)

  server-rs/                         # Rust backend (Axum + Tokio)
    Cargo.toml                       # Workspace root
    crates/
      hackbot-types/                 # Shared types (TraceEvent, WorldState, WS messages)
        Cargo.toml
        src/lib.rs
      hackbot-server/                # Main binary (axum, gateway, replayer, loader)
        Cargo.toml
        src/
          main.rs                    # Axum app, routes, startup
          gateway.rs                 # WebSocket handler, message routing
          trace_loader.rs            # Parse .jsonl trace files, validate schema
          trace_replayer.rs          # Async replay with tokio timing + speed control
          world_model.rs             # Maintain process map, fd table, connections
          mock_data.rs               # Generate realistic mock trace data
      hackbot-signal/                # [Phase 2] Signal processor (complex plane)
        Cargo.toml
        src/lib.rs
      hackbot-ebpf/                  # [Phase 3] eBPF programs + loader (aya)
        Cargo.toml
        src/lib.rs

  frontend/                          # TypeScript + Vite + Pixi.js [UNCHANGED]
    package.json
    tsconfig.json
    vite.config.ts
    index.html                      # Single page with canvas containers
    src/
      main.ts                       # Entry point, initialize app
      app.ts                        # Orchestrator: connects panels, manages state
      connection.ts                 # WebSocket client with reconnect logic
      types.ts                      # TypeScript types (mirrors hackbot-types)
      game/
        world.ts                    # Pixi.js Application + stage management
        process-room.ts             # Render process as labeled rectangle
        syscall-object.ts           # Animated event within a process room
        event-particle.ts           # Particle effect for high-frequency bursts
        camera.ts                   # Pan (drag) + zoom (scroll) controls
        spatial-mapper.ts           # Tree layout for processes
      signal/                       # [Phase 2]
        complex-plane.ts
        phase-diagram.ts
        anomaly-marker.ts
      ui/
        timeline.ts                 # Playback bar with scrub slider
        event-log.ts                # Scrollable event list
        controls.ts                 # PID and type filter chips
        layout.ts                   # CSS Grid panel layout

  traces/                            # Sample trace data for development
    sample-llm-workload.jsonl       # Mock LLM inference trace
    format.md                       # Trace format specification
```

---

## 6. Implementation Phases

### Phase 1: Static Trace Replay Viewer

**Goal**: Prove that eBPF trace data can be rendered as a navigable 2D game-like world.

**Duration estimate**: 2-3 weeks

#### Phase 1A: Backend Foundation (Rust)

| Task | File(s) | Description |
|------|---------|-------------|
| 1A.1 | `server-rs/Cargo.toml`, `crates/*/Cargo.toml` | Initialize Cargo workspace. Dependencies: axum, tokio, serde, serde_json, tower-http (CORS), rand, tracing. |
| 1A.2 | `crates/hackbot-types/src/lib.rs` | Define types with serde: `TraceEvent` (with `#[serde(tag = "type")]` for payload discrimination), `EventType` enum, payload structs, `WorldState`, `ProcessInfo`, WS message enums. Timestamps as `u64` internally, serialized as strings for JS BigInt safety. |
| 1A.3 | `crates/hackbot-server/src/mock_data.rs` | Port mock trace generator. Same narrative (startup→prefill→decode→anomaly→recovery), same seed (42), same event counts. Output compatible .jsonl. |
| 1A.4 | `crates/hackbot-server/src/trace_loader.rs` | Load .jsonl file line-by-line with `serde_json::from_str`. Validate payloads, sort by timestamp. Stream with iterator for large files. |
| 1A.5 | `crates/hackbot-server/src/world_model.rs` | HashMap-based process map, fd table, connections. Event handler dispatch via match on EventType. `rebuild_to()` for seek. |
| 1A.6 | `crates/hackbot-server/src/trace_replayer.rs` | Async state machine with `tokio::time::sleep`. Play/pause via `tokio::sync::Notify`, seek via atomic position update. 16ms batch window. Speed multiplier. Filter by PID/type. |
| 1A.7 | `crates/hackbot-server/src/gateway.rs` | Axum WebSocket handler. `tokio::sync::broadcast` for fan-out to clients. Command parsing from JSON. Playback loop as spawned task. World state updates every 500ms. |
| 1A.8 | `crates/hackbot-server/src/main.rs` | Axum router: `GET /` info, `GET /traces` list files, `WS /ws` upgrade. CORS via tower-http. Auto-load default trace on startup. Serve on port 8000. |

#### Phase 1B: Frontend Foundation

| Task | File(s) | Description |
|------|---------|-------------|
| 1B.1 | `frontend/package.json`, `frontend/vite.config.ts`, `frontend/tsconfig.json` | Initialize TypeScript project with Vite. Dependencies: pixi.js (v8), typescript. |
| 1B.2 | `frontend/src/types.ts` | Define TypeScript interfaces that mirror server schemas: `TraceEvent`, `WorldState`, `ProcessInfo`, `ServerMessage`, `ClientCommand`. |
| 1B.3 | `frontend/src/connection.ts` | WebSocket client class. Methods: `connect(url)`, `send(cmd)`, `onMessage(callback)`. Auto-reconnect with exponential backoff. Parse incoming JSON messages and dispatch by `msg` type. |
| 1B.4 | `frontend/index.html` | Single HTML page with CSS Grid layout: large game-view area (left), event-log area (right), timeline bar (bottom). Dark theme CSS. Canvas containers for Pixi.js and signal view. |
| 1B.5 | `frontend/src/ui/layout.ts` | Manage the panel layout. Expose DOM references for each panel area. Handle resize events to update canvas dimensions. |
| 1B.6 | `frontend/src/game/world.ts` | Initialize Pixi.js Application attached to the game-view canvas. Manage the stage container. On receiving `world_state`: create/update process rooms. On receiving `events` batch: trigger animations. |
| 1B.7 | `frontend/src/game/spatial-mapper.ts` | Given a list of processes with parent-child relationships, compute 2D positions. Algorithm: tree layout where root process is top-center, children are arranged in rows below. Each process gets a rectangle with width proportional to its syscall activity. Return `Map<pid, {x, y, width, height}>`. |
| 1B.8 | `frontend/src/game/process-room.ts` | Pixi.js Container subclass representing one process. Draws: bordered rectangle (room walls), label (PID + comm name), activity indicator (color shifts from dim to bright based on event frequency). Methods: `addEvent(event)` triggers a flash animation. |
| 1B.9 | `frontend/src/game/syscall-object.ts` | Pixi.js Graphics that appears inside a process room when a syscall occurs. Small colored circle (color by syscall type: read=blue, write=green, open=yellow, etc.). Animates: scale up, hold briefly, fade out over 500ms. |
| 1B.10 | `frontend/src/game/camera.ts` | Pan and zoom for the Pixi.js stage. Mouse drag to pan (translate stage position). Scroll wheel to zoom (scale stage around cursor position). Clamp zoom between 0.1x and 5x. |
| 1B.11 | `frontend/src/ui/timeline.ts` | HTML-based playback controls below the canvas. Play/Pause button, speed selector (0.5x/1x/2x/5x/10x), range slider for scrubbing. Displays current timestamp. Sends `play`/`pause`/`seek`/`speed` commands to server via WebSocket. |
| 1B.12 | `frontend/src/ui/event-log.ts` | Scrollable div showing recent events as text lines. Format: `[timestamp] PID:COMM syscall_name(args) -> result`. Color-coded by event type. Auto-scroll to bottom during playback. Max 500 visible entries (virtualized or truncated). |
| 1B.13 | `frontend/src/ui/controls.ts` | Filter panel: multi-select for PIDs (populated from world_state), checkboxes for event types. Sends `filter` command to server on change. |
| 1B.14 | `frontend/src/app.ts` | Orchestrator that wires everything together: creates Connection, World, Timeline, EventLog, Controls. Dispatches incoming messages to the correct component. |
| 1B.15 | `frontend/src/main.ts` | Entry point. Creates App instance, calls `app.init()`. |

#### Phase 1C: Integration and Polish

| Task | File(s) | Description |
|------|---------|-------------|
| 1C.1 | `traces/sample-llm-workload.jsonl` | Run mock_data.py to generate the sample trace file. |
| 1C.2 | `traces/format.md` | Document the trace format: field descriptions, supported event types, payload schemas for each type. |
| 1C.3 | Multiple | End-to-end test: start server, open browser, load trace, verify playback renders process rooms and syscall animations with correct timing. |
| 1C.4 | `frontend/src/game/event-particle.ts` | Add particle burst effect when many events hit the same process room in a short window (visual indicator of high activity). |

---

### Phase 2: Complex Plane Signal View

**Goal**: Add the mathematical visualization that bridges empirical tracing and formal analysis. This is the research differentiator.

**Duration estimate**: 1-2 weeks

| Task | File(s) | Description |
|------|---------|-------------|
| 2.1 | `server/server/signal_processor.py` | Implement the signal processing pipeline. (a) **Windowing**: sliding window of configurable width (default 100ms) over the event stream. (b) **Feature extraction per window**: syscall_rate (events/sec as amplitude r), dominant_syscall_distribution entropy (as phase angle theta -- high entropy = diverse activity, low entropy = repetitive pattern), power_mean (average power reading). (c) **Complex mapping**: `z(t) = r(t) * exp(i * theta(t))` computed as `z_real = r * cos(theta)`, `z_imag = r * sin(theta)`. (d) **Anomaly detection**: maintain exponential moving average of z; if current z deviates beyond 2 standard deviations from the moving average, flag as anomaly. |
| 2.2 | `server/server/gateway.py` | Extend to send `signal` messages alongside `events` messages. One signal message per window (every 100ms of trace time). |
| 2.3 | `frontend/src/signal/complex-plane.ts` | Canvas 2D renderer for the complex plane. Draw axes (real/imaginary), unit circle reference. Plot z(t) as a moving point with a fading trail (last 200 points). Color trail by time (older = dimmer). When anomaly=true, draw a red pulse ring around the point. Respond to resize events. |
| 2.4 | `frontend/src/signal/phase-diagram.ts` | Canvas 2D renderer for theta(t) over time. Scrolling line chart (time on x-axis, theta on y-axis). Highlight anomalous windows with red background strips. |
| 2.5 | `frontend/src/signal/anomaly-marker.ts` | When the signal processor flags an anomaly, also highlight the corresponding process room(s) in the game view (red border flash) and scroll the event log to the anomalous events. |
| 2.6 | `frontend/src/ui/layout.ts` | Update layout to include signal view panel. Default layout: game view (top-left), signal view split into complex plane (top-right top) and phase diagram (top-right bottom), event log (bottom-right), timeline (bottom full-width). User can toggle signal view on/off. |

---

### Phase 3: Real-time eBPF Streaming

**Goal**: Connect to a live eBPF data source instead of replaying files. Uses `aya` for native Rust eBPF.

**Duration estimate**: 1-2 weeks

| Task | File(s) | Description |
|------|---------|-------------|
| 3.1 | `crates/hackbot-ebpf/src/lib.rs` (new crate) | eBPF programs using `aya`: kprobes for syscall entry/exit, tracepoints for sched_switch, perf events for power. Compiles to BPF bytecode via `aya-bpf`. Userspace loader in the same crate reads from BPF ring buffer and emits `TraceEvent`s. |
| 3.2 | `crates/hackbot-server/src/event_ingestion.rs` (new) | Receives events from `hackbot-ebpf` ring buffer (in-process) or from an external collector via Unix domain socket (newline-delimited JSON). Feeds into the same pipeline as trace_replayer. |
| 3.3 | `crates/hackbot-server/src/gateway.rs` | Add mode switching: "replay" mode (from file) vs "live" mode (from ingestion). In live mode, events are forwarded immediately (no timing replay needed). |
| 3.4 | `crates/hackbot-server/src/world_model.rs` | Ensure world model handles events arriving out of order (possible with live data from multiple CPUs). Use timestamp for ordering within a small reorder buffer (e.g., 10ms). |
| 3.5 | `crates/hackbot-signal/src/lib.rs` | Ensure streaming computation works: sliding window advances in real time, not trace time. |
| 3.6 | `frontend/src/connection.ts` | Add UI indicator for connection mode (replay vs live). In live mode, hide scrub bar; show only pause/resume and speed controls. |
| 3.7 | `crates/hackbot-server/src/main.rs` | Add CLI flag `--live` to switch between replay and live mode. |

---

### Phase 4: Agent Character

**Goal**: Add the hackbot as a visible character navigating the system world. Initially human-controlled.

**Duration estimate**: 1-2 weeks

| Task | File(s) | Description |
|------|---------|-------------|
| 4.1 | `frontend/src/game/agent-character.ts` (new) | Pixi.js sprite/graphic representing the agent. A small animated icon (e.g., a glowing dot with a directional indicator) that smoothly moves between process rooms. Has a "field of view" visual (semi-transparent circle showing the agent's current observation area). |
| 4.2 | `crates/hackbot-server/src/agent_state.rs` (new) | Agent state model: current_position (pid of process being examined), attention_area (list of pids being monitored), action_history (recent actions taken), capabilities (available actions). Shared types go in `hackbot-types`. |
| 4.3 | `frontend/src/game/world.ts` | Add click handler on process rooms. Clicking a room sends `agent_move` command to server. Agent character smoothly animates to the clicked room. |
| 4.4 | `frontend/src/ui/agent-panel.ts` (new) | Side panel showing agent's current state: which process it is examining, what events it sees, suggested next actions (hardcoded heuristics for now, LLM-driven in Phase 5). |
| 4.5 | `crates/hackbot-server/src/gateway.rs` | Handle `agent_move` commands. Update agent state. Send `agent_state` messages to client with position, attention area, and any observations. |

---

### Phase 5: LLM Brain Integration

**Goal**: The hackbot makes autonomous decisions about where to look and what to examine.

**Duration estimate**: 2-3 weeks

| Task | File(s) | Description |
|------|---------|-------------|
| 5.1 | `crates/hackbot-server/src/agent_brain.rs` (new) | OODA loop implementation. On each decision cycle (configurable interval, e.g., every 2 seconds): (a) **Observe**: summarize current system state from world_model (process list, recent event counts, anomaly flags). (b) **Orient**: include context from memory (previous findings, exploration history). (c) **Decide**: send summary to LLM API, receive action selection. (d) **Act**: execute the chosen action (move to process, enable new probe filter, flag anomaly). |
| 5.2 | `crates/hackbot-server/src/llm_client.rs` (new) | Async client for LLM API (support Anthropic Claude and OpenAI GPT). Uses `reqwest` for HTTP. Takes system state summary string, available actions list, and returns selected action with reasoning. Configurable model, temperature, max tokens. |
| 5.3 | `crates/hackbot-server/src/agent_state.rs` | Extend with action types: `move_to(pid)`, `focus_on(event_type)`, `flag_anomaly(description)`, `adjust_filter(params)`. Each action has a defined effect on the world model / gateway filters. |
| 5.4 | `frontend/src/ui/agent-panel.ts` | Extend to show LLM reasoning: display the agent's thought process as text, show the action it chose and why, show alternatives it considered. Display as a scrollable log with timestamps. |
| 5.5 | `frontend/src/game/agent-character.ts` | Add visual indicators for agent state: "thinking" animation while LLM is processing, "acting" animation when executing, "idle" when waiting for next cycle. |
| 5.6 | `crates/hackbot-server/src/gateway.rs` | Add human override commands: `agent_pause` (stop autonomous decisions), `agent_resume`, `agent_redirect(pid)` (force agent to examine a specific process). |
| 5.7 | `crates/hackbot-server/src/memory_store.rs` (new) | Simple JSON-file-based persistence for agent findings using `serde_json`. Stores: discovered anomalies, explored paths, pattern library (known-normal and known-anomalous event sequences). Loaded on startup, saved periodically. |

---

## 7. Key Design Decisions and Trade-offs

### Decision 1: 2D Isometric vs 3D

**Choice**: 2D top-down/isometric.

**Rationale**: rs-sdk demonstrates that 2D is sufficient for agent-world interaction visualization. Dockercraft's 3D (Minecraft) is visually compelling but requires a 3D engine or game client dependency. 2D is faster to build, easier to iterate on, and runs in any browser without performance concerns. The complex plane view is inherently 2D. If 3D is desired later, Three.js can be added as an alternative view mode without rewriting the backend.

**Trade-off**: Less immersive than a 3D game world. Acceptable because the research value is in the data visualization, not visual fidelity.

### Decision 2: Replay-First

**Choice**: Start with trace file replay, add live streaming later (Phase 3).

**Rationale**: Replay is deterministic (same file produces same visualization every time), which makes development and debugging far easier. It eliminates the need for root access, a running kernel, or eBPF setup during frontend development. Traces from Sunwoo's professional work can be used directly. Replay also enables sharing interesting traces with collaborators.

**Trade-off**: Delays the real-time "living system" experience. Acceptable because the visualization concepts (spatial mapping, complex plane) can be fully validated with replay data.

### Decision 3: Rust Backend (not Python or all-TypeScript)

**Choice**: Rust (Axum + Tokio) for the server, TypeScript for the frontend.

**Rationale**: (a) Verus formal verification (Pillar 4) requires Rust code — writing Python now means rewriting for verification later. (b) Phase 3 eBPF uses `aya` — kernel and userspace both in Rust, no C/Python boundary. (c) Single static binary deployment simplifies running on research machines. (d) Performance headroom for live eBPF streaming at millions of events/sec (Phase 3). (e) End-to-end memory safety from kernel probe to API server.

**Trade-off**: Slower initial development vs Python. Mitigated by: (1) the Python prototype already validated the architecture, (2) Rust's serde provides equivalent serialization ergonomics to Pydantic, (3) signal processing research prototyping done in Jupyter notebooks, production code ported to ndarray.

### Decision 4: Pixi.js (not raw Canvas, not full game engine)

**Choice**: Pixi.js v8 for the game view.

**Rationale**: Pixi.js provides WebGL-accelerated 2D rendering with a sprite/container abstraction that maps naturally to the process-room/syscall-object hierarchy. It handles the render loop, batching, and GPU texture management. Raw Canvas would require reimplementing all of this. A full game engine (Phaser, Godot) would impose opinions about physics, scenes, and game loops that do not apply to a data visualization tool.

**Trade-off**: A dependency (~200KB). Acceptable for the rendering performance and API quality it provides.

### Decision 5: Complex Plane Feature Mapping

**Choice**: Amplitude r(t) = syscall event rate (events/second). Phase theta(t) = Shannon entropy of syscall type distribution within the window.

**Rationale**: Event rate is the most intuitive measure of "how active" the system is -- it maps naturally to amplitude (distance from origin). Entropy of syscall distribution captures "what kind of activity" -- low entropy means the system is doing one thing repeatedly (stable phase), high entropy means diverse activity (shifting phase). An LLM workload has characteristic patterns: prefill phase has high GPU submit rate + low syscall diversity (stable orbit), decode phase has regular small reads + low power (different stable orbit), anomalies (side-channel probing, resource contention) would disrupt both metrics simultaneously (orbit deviation).

**Trade-off**: This mapping is a hypothesis, not a proven technique. The values r(t) and theta(t) may need tuning based on real trace data. The architecture supports swapping the mapping function without changing the visualization code, because the signal_processor outputs (z_real, z_imag, theta, anomaly, deviation) are the interface, not the computation internals.

### Decision 6: Spatial Layout Algorithm

**Choice**: Tree-based grid layout for MVP. Process hierarchy (parent-child) determines position. Root process at top, children below, grandchildren below that.

**Rationale**: Simple, deterministic, and aligns with how users mentally model process hierarchies (similar to `pstree` output). Force-directed layouts look prettier but are non-deterministic (different runs produce different layouts) and expensive to compute for large process trees.

**Trade-off**: May not reveal inter-process communication patterns as clearly as a force-directed layout where communicating processes are pulled closer together. Can be upgraded to force-directed layout as an option in a later phase.

### Concern 1: Verus for formal verification and What about leveraging WASM?

Verus can formally verify Rust code, aligning with Pillar 4 (mathematical self-improvement). WASM runtimes exist in kernel space (kernel-wasm/Wasmer, Wasmjit, Camblet/wasm3) — could provide sandboxed execution for dynamic kernel-space code without native module risks. Worth investigating as an execution sandbox for the agent's action layer.

See **Appendix D: WASM in Kernel — Future Exploration** for analysis of potential WASM use cases (inference sandbox, action sandbox, plugin system). Decision deferred — attractive but adds complexity. Revisit after System 1/2 hybrid is working.

### Concern 2: In-Kernel LLM (Research Direction)

See **Appendix B: In-Kernel LLM Feasibility Analysis** for a detailed technical assessment and **Appendix C: Hybrid eBPF + Kernel Module Architecture** for the refined approach.

**Key insight**: eBPF and kernel modules are NOT competing alternatives — they serve complementary roles. eBPF = eyes/ears (safe, verified observation). Kernel module = brain (LLM inference). Communication via BPF maps.

**Action safety breakthrough**: The LLM can generate eBPF programs as its "actions" — the kernel's own BPF verifier proves they are safe before execution. No external formal verification needed for the action layer. The verifier rejects any unsafe program. This makes the eBPF verifier the formal verification layer FOR the LLM's actions.

### Concern 3: Kernel as Game World (Vision)

The long-term vision: render kernel internals as an explorable 3D game world where the in-kernel LLM agent is a visible character. Two modes:

1. **Exploration Mode**: Educational kernel tourism. LLM is your guide. Ask questions, follow the agent, learn how the kernel works by watching it live.
2. **Security Mode**: Autonomous anomaly hunting. CTF-style challenges. Red/blue team kernel security research.

The novelty is the combination: in-kernel AI + game visualization + real-time conversational interface. Each piece has precedent (KLLM, Dockercraft, chatbots) but the combination is unique. No existing tool offers a spatial, conversational, AI-guided experience of kernel internals.

---

## 8. Prioritized Build Order

This is the strict ordering. Do not skip ahead.

### First: Mock Data Generator (Task 1A.3)
Before anything else, create realistic sample data. Every other component depends on having trace data to consume. The mock generator should produce traces that exhibit the patterns we want to visualize: normal LLM workload with periodic GPU bursts, scheduling events, and injected anomalies (unusual syscall sequences, power spikes).

### Second: Backend Core (Tasks 1A.1, 1A.2, 1A.4, 1A.5, 1A.6, 1A.7, 1A.8)
Schemas first (1A.2), then loader (1A.4), then world model (1A.5), then replayer (1A.6), then gateway (1A.7), then main app (1A.8). Each can be tested independently with the mock data.

### Third: Frontend Game View (Tasks 1B.1 through 1B.10)
Project setup (1B.1-1B.3), then HTML layout (1B.4-1B.5), then the game world (1B.6-1B.10). The game view is the project's centerpiece -- the "kernel as game world" concept. Get this working and visually compelling before anything else on the frontend.

### Fourth: Frontend Controls (Tasks 1B.11 through 1B.15)
Timeline, event log, filters, and the main orchestrator. These are essential for usability but secondary to the core visualization.

### Fifth: Complex Plane (Phase 2, all tasks)
This is the research differentiator. Once the game view works, add the signal processing pipeline and the complex plane visualization. This transforms the project from "a pretty trace viewer" into "a novel anomaly detection visualization."

### Sixth: Everything Else (Phases 3-5)
Real-time streaming, agent character, LLM brain. These are important for the full vision but each builds on a working visualization foundation.

---

## 9. Edge Cases and Considerations

### Performance

- **Event volume**: An LLM workload can generate millions of events per second. The server MUST aggregate events into batches before sending to the client. The `trace_replayer.py` batches events within 16ms windows (one frame at 60fps). For live mode, server-side filtering is critical.
- **Pixi.js object count**: Creating a new Pixi.js Graphics object per syscall event and letting them accumulate will crash the browser. Syscall objects must be pooled (create a fixed pool of ~200 objects, reuse them with object pooling) or use a particle system for high-frequency events.
- **WebSocket bandwidth**: Sending every event as a separate JSON message is wasteful. Batch events into arrays. Consider binary encoding (MessagePack) if JSON bandwidth becomes a bottleneck.
- **Complex plane computation**: Sliding window over millions of events requires efficient data structures. Use a circular buffer for the window, not a list with slicing.

### Complex Plane Mapping

- The amplitude/phase mapping (event rate -> r, entropy -> theta) is an initial hypothesis. The interface between `signal_processor.py` and the visualization is deliberately abstract (`z_real`, `z_imag`, `anomaly`) so the mapping function can be swapped without changing frontend code.
- "Normal orbit" definition requires baseline data. The MVP should allow recording a baseline (e.g., first 30 seconds of a trace) and then measuring deviations from that baseline.
- The anomaly threshold (2 standard deviations) is arbitrary. Make it configurable via the UI.

### Trace File Format

- JSON Lines (.jsonl) is human-readable but verbose. A 5-second trace with 10,000 events is ~2-5MB in JSON. This is fine for MVP. If files grow to gigabytes, consider a binary format (e.g., FlatBuffers or a custom binary format matching perf.data conventions).
- Timestamps must be in nanoseconds (uint64). JavaScript cannot represent nanoseconds precisely in a Number (max safe integer is 2^53). Use BigInt or string representation for timestamps in the frontend, or represent as milliseconds (losing nanosecond precision, which is acceptable for visualization).

### Security

- The MVP runs locally (localhost only). No authentication, no HTTPS.
- When live eBPF mode is added (Phase 3), the data collector process needs root privileges for eBPF. The gateway server should NOT run as root. The collector sends events to the gateway over a local socket.
- Never expose the WebSocket endpoint to a network without authentication. The agent can potentially execute system calls (Phase 5) -- unauthorized access would be a serious risk.

### Browser Compatibility

- Pixi.js v8 requires WebGL2. All modern browsers (Chrome, Firefox, Safari 15+, Edge) support WebGL2. No IE support needed.
- WebSocket is universally supported.
- BigInt (for nanosecond timestamps) is supported in all modern browsers.

### Development Without Root/Kernel Access

- The replay-first approach means the frontend can be developed entirely without root access or a Linux kernel. Mock data is sufficient.
- The signal processing pipeline can be developed and tested with mock data.
- Only Phase 3 (live streaming) requires actual eBPF capabilities.

---

## 10. Mapping to Research Vision

This section connects each implementation phase to the broader research concepts from the source documents.

| Phase | Research Concept | Source Document |
|-------|-----------------|----------------|
| Phase 1 (Trace Viewer) | "Tracing is not merely a debugging tool -- it is an empirical verification instrument" | Blog post (Gemini section) |
| Phase 2 (Complex Plane) | "The trace of the bot maps to a complex plane" (Pillar 0.3) | Research Statement page 2 |
| Phase 3 (Real-time) | "Monitor -> Trace" flow from the handwritten diagram | mynote.jpg |
| Phase 4 (Agent Character) | "All actions that the bot is doing are shown in real time as if it is the character in video games" (Pillar 0.2) | Research Statement page 2 |
| Phase 5 (LLM Brain) | "Auto-hunting" system with Sensory Input / Brain / Action / Memory | Blog post (auto-hunting section) |

The MVP (Phases 1-2) lives within **Stage 1 (Adventure)** of the research cycle. The complex plane view provides early **Stage 2 (Modeling)** capability. Stages 3 (Modding) and 4 (Quantifying) are long-term research goals that the visualization will eventually support but do not need implementation now.

---

## Appendix A: Sample Mock Trace Structure

The mock trace generator (`server/server/mock_data.py`) should produce a trace that tells a story:

```
Time 0-1s:    LLM server starts up. Parent process forks 4 workers.
              Events: process_fork x4, many open/mmap syscalls.
              Power: low baseline.
              Signal: orbit establishing its normal position.

Time 1-3s:    Inference request arrives. Prefill phase.
              Events: GPU submits (large batches), high read/write activity.
              Power: spikes during GPU computation.
              Signal: orbit moves to "prefill zone" (high amplitude, low entropy).

Time 3-4s:    Decode phase. Token-by-token generation.
              Events: regular small GPU submits, periodic write syscalls.
              Power: moderate, rhythmic pattern.
              Signal: orbit moves to "decode zone" (moderate amplitude, stable phase).

Time 4-4.5s:  ANOMALY INJECTION. Simulate a suspicious process doing:
              - Unusual open() calls to /proc/[worker_pid]/maps
              - High-frequency read() on shared memory regions
              - Power monitoring via perf_event_open()
              Signal: orbit deviates sharply (phase shift + amplitude spike).

Time 4.5-5s:  Normal operation resumes.
              Signal: orbit returns toward normal zone.
```

This narrative structure serves multiple purposes:
1. Tests all event types in the visualization
2. Demonstrates the complex plane's ability to detect the injected anomaly
3. Tells a security-relevant story (side-channel probing of an LLM workload)
4. Aligns with the research vision (detecting information leakage through trace analysis)

---

## Appendix B: In-Kernel LLM Feasibility Analysis

**Date**: 2026-03-14
**Status**: Research exploration

### The Vision

A mini LLM running as a permanent kernel thread (`kthread`) that:
- **Never dies** — runs indefinitely like `kswapd` or `ksoftirqd`
- **Observes kernel state directly** — reads `task_struct`, VFS, network stack, scheduler state without syscall overhead
- **Responds to user prompts** — via `/dev/hackbot` or `/proc/hackbot` character device
- **Wanders the kernel** — autonomously explores data structures, registers tracepoints/kprobes, follows interesting activity

### Prior Art

1. **KLLM** ([github.com/randombk/kllm](https://github.com/randombk/kllm)): Ports `llm.c` (GPT-2 124M) to a Linux kernel module. Proof-of-concept that barely works — 1+ minute per token, system freezes during inference, single-core CPU only. Accessed via `/dev/llm0`. Demonstrates feasibility but not practicality.

2. **eBPF + ML research**: Papers show quantized neural networks (decision trees, MLPs) running in eBPF with 84% inference latency reduction vs userspace. Uses fixed-point arithmetic to avoid FPU. Limited to lightweight models by eBPF verifier constraints (instruction count, no loops, no FP).

3. **WASM in kernel**: Production-quality runtimes exist — kernel-wasm (Wasmer), Wasmjit, Camblet (Cisco/wasm3). Eliminates syscall/context-switch overhead. Could host a WASM-compiled inference engine with sandboxing guarantees.

### Hard Technical Constraints (from Linux 6.16 source analysis)

#### 1. Floating Point: The Architectural Constraint

`kernel_fpu_begin()` (`arch/x86/kernel/fpu/core.c:442`) disables preemption for the entire duration of FPU usage. Between `kernel_fpu_begin()` and `kernel_fpu_end()`, you **cannot sleep, cannot be preempted, and cannot take softirqs**.

The crypto subsystem (e.g., `aesni-intel_glue.c`) shows the idiomatic pattern: grab FPU, do ONE micro-operation (single AES block), release FPU. Never held for more than microseconds.

**Implication**: LLM inference must be decomposed into micro-operations (~1ms each), each wrapped in `kernel_fpu_begin/end`. A single matrix-vector multiply per FPU window. This is architecturally ugly but the only way to avoid destroying system latency.

**Alternative**: INT8/INT4 quantized inference using only integer arithmetic — avoids FPU entirely. eBPF ML research validates this approach, but accuracy trade-offs for language models are severe at low bit widths without careful quantization-aware training.

#### 2. Memory: Solvable

- `vmalloc()` can allocate up to `totalram_pages()` — no arbitrary cap (`mm/vmalloc.c:3829`)
- phi4-mini (3.8B params) in Q4 quantization: ~4.5GB. Fits in vmalloc on a 16GB+ machine
- Firmware loader API (`rust/kernel/firmware.rs`) can load weights from `/lib/firmware/`
- kthreads can use `kthread_use_mm()` to access userspace address space if needed

#### 3. Kernel Threads: Perfect Fit

The `kthread` API (`include/linux/kthread.h`) is exactly designed for permanent kernel threads:
- `kthread_run()` creates and wakes a thread
- `kthread_should_stop()` / `kthread_stop()` for clean shutdown
- `kthread_park/unpark()` for suspend/resume (e.g., during system suspend)
- `wait_event_interruptible()` for sleeping until work arrives
- `kthread_worker` pattern for queuing inference requests

A kthread using FPU is slightly more efficient than user-process FPU usage — no user FPU state to save (`core.c:453` checks `PF_KTHREAD`).

#### 4. User Interface: Character Device or procfs

Two options for prompt/response interaction:
- **`/dev/hackbot`** (miscdevice): Rust kernel has `miscdevice` abstractions. Clean open/read/write/poll/ioctl. Best for Rust modules.
- **`/proc/hackbot`** (procfs): `proc_ops` with `proc_read/proc_write/proc_poll`. Pattern: `/proc/kmsg` (`fs/proc/kmsg.c`) — blocking read, poll support. No Rust wrapper exists.

Both support `poll()`/`epoll()` for non-blocking userspace clients.

#### 5. No Math Libraries: Must Build from Scratch

The kernel has **zero** ML/tensor/matrix code. You must implement:
- Matrix-vector multiply (GEMM)
- Softmax (or approximation)
- Layer normalization
- Embedding lookup
- Attention mechanism
- Activation functions (SiLU/GELU)

All without libc, libm, BLAS, or any numerical library. This is the largest engineering effort.

### Possible Architectures

| Architecture | Description | Pros | Cons |
|-------------|-------------|------|------|
| **A: Pure in-kernel (Rust module)** | kthread + vmalloc'd weights + chunked FPU inference + /dev/hackbot | True in-kernel, maximum research novelty, direct kernel state access | Must write GEMM from scratch, CPU-only (no GPU kernel API exists), limited model size |
| **B: Kernel socket → vLLM/ollama** | Kernel module uses kernel socket API to call localhost LLM server | Full model quality, GPU-accelerated, ~200 lines of code, works in days | LLM brain not truly in-kernel, requires userspace server running |
| **C: WASM runtime in kernel** | Load kernel-wasm, compile quantized INT8 engine to WASM, run in kernel | Sandboxed execution, portable | WASM overhead, integer-only (no FPU in WASM), complex toolchain |
| **D: Hybrid System 1/2** | Small model in-kernel (A) + large model via socket (B) | Best of both: instant reflexes + deep reasoning | Most complex architecture, two inference paths |

#### Why Not GPU from Kernel?

Investigated in Linux 6.19.8: **zero kernel-internal APIs exist for GPU/NPU compute dispatch.** All compute (CUDA, ROCm, OpenVINO) is gated behind userspace ioctls. The `drivers/accel/` framework is registration plumbing, not a compute API. No accelerator driver exports any compute symbols for external module use. Pure in-kernel inference is CPU-only by design.

### Recommended Path: System 1/System 2 Hybrid (Architecture D)

**The Biological Analogy**: Like the human nervous system — the spinal cord produces instant reflexes (knee-jerk reaction when danger is detected), while nerve signals travel to the brain for conscious analysis. The body reacts BEFORE the brain understands why.

```
[hackbot.ko kthread — THE AGENT'S BODY]
│
├── System 1: SPINAL CORD (in-kernel, instant)
│   ├── Tiny INT8 model (~1-33M params) in vmalloc
│   ├── Pattern matching: "is this syscall pattern anomalous?"
│   ├── Instant reflexes: alert, flag, highlight
│   ├── No network, no latency, no external dependency
│   └── Tier 0-1 actions only (observe + instrument)
│
└── System 2: BRAIN (vLLM via kernel socket, deep)
    ├── Large model (phi4-mini/Llama 3) on GPU
    ├── Complex reasoning: "what IS this anomaly? what should we do?"
    ├── kernel_connect(127.0.0.1) → HTTP POST → ollama/vLLM
    ├── Response arrives 50-500ms later
    └── Can REQUEST Tier 2+ actions (requires human approval)
```

**Two Independent Axes** (key insight, 2026-03-20):

The in-kernel LLM has two orthogonal design dimensions:

1. **Inference substrate** — WHERE the brain runs (in-kernel CPU vs remote GPU)
2. **Agent capability** — WHAT the brain can DO (static Q&A vs dynamic OODA tool use)

```
                        WHERE the brain runs
                        ────────────────────
                        In-kernel INT8          Remote vLLM
                        (System 1)              (System 2)
                        │                       │
 WHAT it          ──────┼───────────────────────┼──────────────
 can DO                 │                       │
                        │                       │
 Static                 │  Brain in a jar:      │  Step 2b (DONE):
 (fixed context)        │  fast pattern match   │  smart model +
                        │  but can't look       │  kernel context
                        │  around               │  but static
                        │                       │
 Dynamic                │  THE DREAM:           │  Step 2c (DONE):
 (OODA + tools)         │  autonomous agent,    │  smart + can
                        │  instant + capable    │  investigate, but
                        │                       │  100ms+ per step
```

Build **OODA tools first** because they define the interface BOTH systems use. Then swap the inference backend from remote to local. Building INT8 without tools gives you a fast brain that can't do anything.

**Implementation Steps**:

| Step | What | Architecture | Status |
|------|------|-------------|--------|
| **1** | Kernel module skeleton: `/dev/hackbot` + prompt/response | Infrastructure | **DONE** |
| **2a** | Kernel socket → vLLM (System 2 brain) | B | **DONE** |
| **2b** | Kernel context injection (live system state in prompt) | B | **DONE** |
| **2c** | Dynamic agent loop + kernel tools (OODA) | B | **DONE** |
| **3** | In-kernel INT8 inference engine (System 1 reflex) | A | **NEXT** |
| **4** | Hybrid System 1/2 merge | D | After 2c + 3 |
| **5** | Action capabilities with Verus-verified safety | All | Research |
| **6** | 3D game rendering of agent behavior | Frontend | After 4 |

**Step 2c details** — the OODA agent loop (DONE):
- Three kernel observation tools: `ps` (two-pass walk, user-space first), `mem` (si_meminfo), `loadavg` (avenrun[])
- LLM generates `<tool>name</tool>` tags to request kernel data
- Agent loop in `agent_loop()`: prompt → vLLM → parse → if tool call, execute + re-prompt
- Bounded iterations (max 10) + conversation size limit (96 KB)
- Read-only (Tier 0) tools — no action capability
- vLLM stop sequence `["</tool>"]` ensures clean tool call boundaries
- Graceful degradation: base models (OPT-125M) work like Step 2b (no tool calls detected)
- **Tested with**: Qwen/Qwen2.5-7B-Instruct-AWQ on keti GPU server via Tailscale

**Step 3 details** — in-kernel INT8 inference (System 1):
- **Model**: SmolLM2-135M-Instruct (Llama-3 architecture, instruction-tuned, GQA)
  - 30 layers, dim=576, 9 Q heads / 3 KV heads, vocab=49152
  - INT8 quantized: ~135MB in `/lib/firmware/hackbot-model.bin`
- **Inference**: Pure scalar integer arithmetic — no FPU, no kernel_fpu_begin/end
  - INT8 matmul with INT32 accumulation
  - Fixed-point (Q16.16) for softmax/RMSNorm/SiLU via lookup tables
  - Estimated ~10-20 tok/s on Ryzen 5 PRO 4650G (memory-bandwidth limited)
- **Why SmolLM2 over TinyStories**: Already instruction-tuned (can follow tool-calling format), same Llama architecture, no throwaway work on different model format/tokenizer
- **Substeps**: 3f (Python model exporter) → 3a (firmware weight loading) → 3b (integer math primitives) → 3c (Llama-3 forward pass with GQA) → 3d (BPE tokenizer) → 3e (wire into agent_loop)
- Uses the SAME tool interface as Step 2c — substrate swap only
- Optional Step 3.5: AVX2 SIMD kernels (C FFI + kernel_fpu_begin/end) for ~3-5x speedup

**Why OODA before INT8**: The tool interface (Step 2c) is foundational — it defines what the agent CAN DO regardless of where inference runs. INT8 (Step 3) is a performance optimization of where inference runs. We validated the tool architecture with remote vLLM first (easier debugging, smarter model), then swap in the local INT8 engine.

### Action Safety: Tiered Capability System

> "관찰만 하면 그래도 괜찮은데 action이(손발) 주어지면 엄청 위험하지?! 그래서 formal verification??"

An in-kernel LLM agent needs a **capability-based security model** — like microkernel capability tokens. The agent can only perform actions it has been explicitly granted.

#### Action Tiers

| Tier | Category | Examples | Risk | Default |
|------|----------|----------|------|---------|
| **0** | Pure Observation | Read task_struct, read /proc-equivalent, read scheduler queues, read VFS state | None — read-only, cannot corrupt kernel | **GRANTED** |
| **1** | Instrumentation | Attach kprobes, register tracepoints, set eBPF filters, read perf counters | Low — reversible, well-defined APIs. Risk: performance impact from too many probes | **GRANTED** |
| **2** | Indirect Actions | Adjust nice values, send SIGTERM/SIGSTOP, modify cgroup params, adjust net QoS | Medium — affects system behavior through standard APIs. Could disrupt services | **REQUIRES HUMAN APPROVAL** |
| **3** | Direct Kernel Modification | Write kernel memory, modify function pointers, patch running code, change security policies | **EXTREME** — can crash kernel, create security holes, corrupt state | **NEVER** (unless Verus-verified) |

#### Safety Architecture (Defense in Depth)

```
Layer 3: Verus Formal Verification (compile-time)
  └── Proves the capability system itself is correctly implemented
  └── Proves Tier 0-1 actions cannot corrupt kernel state
  └── Future: proves specific Tier 2 actions are safe

Layer 2: eBPF Verifier (load-time)
  └── LLM-generated eBPF programs verified before execution
  └── Guarantees: termination, memory bounds, no corruption
  └── "The LLM proposes. The verifier disposes."

Layer 1: Capability Boundary (runtime)
  └── Agent can only call functions it has capability tokens for
  └── System 1 (reflex): limited to Tier 0-1 (observe + instrument)
  └── System 2 (brain): can REQUEST Tier 2, user must approve
  └── Tier 3: structurally impossible without Verus-verified code

Layer 0: Rust Type System (compile-time)
  └── Memory safety, no data races, no null derefs
  └── Unsafe FFI only for kernel_fpu_begin/end
```

#### The Kill Switch

The user can instantly revoke all capabilities via `/dev/hackbot`:
- `echo "STOP" > /dev/hackbot` — reverts agent to Tier 0 observation-only
- `echo "PARK" > /dev/hackbot` — parks the kthread entirely (kthread_park)
- `rmmod hackbot` — removes the agent completely

All actions are logged to a ring buffer and streamed to the visualization frontend.

#### System 1 vs System 2 Safety Mapping

| | System 1 (Reflex) | System 2 (Brain) |
|---|---|---|
| **Allowed tiers** | 0-1 (observe + instrument) | 0-2 (can REQUEST actions) |
| **Human approval needed** | Never | Yes, for Tier 2 |
| **Speed** | Instant (<1ms) | 50-500ms |
| **When** | Always running, continuous monitoring | On-demand or periodic deep analysis |
| **Example** | "Anomaly detected in PID 1234!" (instant alert) | "PID 1234 is performing a side-channel attack on the LLM workload via /proc/maps reads. Recommend: adjust cgroup isolation." |

### 3D Game Rendering of Agent Behavior

The in-kernel agent's behavior maps naturally to the game visualization:

#### Visual Mapping

| Agent Behavior | Game World Representation |
|---------------|--------------------------|
| Agent observing a process | Character standing in a process room, "looking" at events |
| System 1 reflex (anomaly detected) | **Instant red flash** — room borders glow red, particles burst, alert sound |
| System 2 thinking | Character shows "thinking" animation (pulsing glow), chat panel shows "Analyzing..." |
| System 2 response arrives | Speech bubble appears above character, detailed text in chat panel |
| Agent moving to new process | Character smoothly walks through corridors between rooms |
| kprobe attached | "Sensor" icon appears on the target function (like placing a trap in a game) |
| Agent idle / patrolling | Character slowly wanders, looking around (ambient animation) |
| Kill switch activated | Character freezes, turns grey, "PAUSED" overlay |

#### Two-Speed Visual Feedback

The biological System 1/2 split creates a compelling visual rhythm:

1. **Fast pulse** (System 1): The game world constantly pulses with real-time kernel activity. Syscalls flash, processes glow based on load, the agent reacts instantly to anomalies. This is the "heartbeat" of the visualization.

2. **Slow narrative** (System 2): Periodically, the agent pauses to "think." After 1-2 seconds, a detailed analysis appears. The user can read the agent's reasoning while the real-time visualization continues in the background.

This two-speed feedback is more engaging than a single-speed agent. Users can SEE the difference between instinct and reasoning — like watching a predator's ears perk up (reflex) before it makes a deliberate decision to pursue (reasoning).

#### 3D World Structure

```
The Kernel World (3D game environment)
│
├── Process Buildings — each process is a 3D structure
│   ├── Room interior — syscalls visualized as events inside
│   ├── Size proportional to resource usage
│   └── Color indicates state (running=green, sleeping=blue, zombie=red)
│
├── Memory Terrain — heap/stack visualized as landscape
│   ├── Height = memory pressure
│   └── Hot regions glow
│
├── Network Corridors — connections between processes
│   ├── Width = bandwidth
│   └── Packets visualized as particles flowing through
│
├── The Agent Character — hackbot
│   ├── Visible in the world, navigable by user or autonomous
│   ├── Field of view circle shows observation range
│   └── Sensor icons show where kprobes are attached
│
└── The Anomaly Zone — areas flagged by System 1
    ├── Red fog/glow around suspicious processes
    ├── Pulsing alert indicators
    └── System 2 analysis text floating nearby
```

### Model Size Reality Check

| Model | Params | Q4 Size | System | Tokens/sec | Use |
|-------|--------|---------|--------|-----------|-----|
| Custom tiny | 1M | ~2MB | 1 (in-kernel) | 50-100 | Pattern matching, anomaly flagging |
| TinyStories | 33M | ~20MB | 1 (in-kernel) | 5-10 | Simple observation summaries |
| **SmolLM2-135M-Instruct** | **135M** | **~135MB** | **1 (in-kernel)** | **10-20** | **Instruction-following, tool calling (TARGET)** |
| GPT-2 small | 124M | ~70MB | 1 (in-kernel) | 1-3 | Basic reasoning (ambitious) |
| Qwen2.5-7B-Instruct-AWQ | 7B | ~4GB | 2 (vLLM/GPU) | 20-50 | Full reasoning, analysis, planning |
| Llama 3 | 8B | ~5GB | 2 (vLLM/GPU) | 10-30 | State-of-art reasoning |

---

## Appendix C: Hybrid eBPF + Kernel Module Architecture

**Date**: 2026-03-14

### Why Not Either/Or

eBPF and kernel modules serve fundamentally different roles. They are **complementary**, not competing:

| Role | eBPF | Kernel Module |
|------|------|---------------|
| **Observation** (tracing syscalls, scheduling, power) | Excellent — purpose-built, verifier-guaranteed safe | Possible but must implement manually |
| **LLM Inference** (matrix ops, attention, generation) | Impossible — no FP, 512B stack, bounded loops, verifier rejects | Feasible — kthread + FPU + vmalloc (KLLM proved it) |
| **Safety guarantee** | BPF verifier proves program safety before loading | No verifier — bugs can panic kernel |
| **Dynamic loading** | Load/unload at runtime, no reboot | Module load/unload |
| **Communication** | BPF maps, ring buffers, perf buffers | Any kernel API |

### The Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                       KERNEL SPACE                           │
│                                                              │
│  ┌────────────────────┐        ┌──────────────────────────┐ │
│  │  eBPF Programs      │        │  Rust Kernel Module      │ │
│  │  (THE EYES & EARS)  │        │  (THE BRAIN)             │ │
│  │                     │  BPF   │                          │ │
│  │  • kprobes/syscalls │──maps──│  • LLM inference kthread │ │
│  │  • tracepoints      │───────>│  • reads observations    │ │
│  │  • sched events     │        │  • generates responses   │ │
│  │  • power/perf       │<───────│  • proposes eBPF actions │ │
│  │  • network probes   │        │  • /dev/hackbot iface    │ │
│  │                     │        │                          │ │
│  │  ★ REUSE EXISTING   │        │  ★ NEW IMPLEMENTATION    │ │
│  │    eBPF TRACER!     │        │                          │ │
│  └─────────┬──────────┘        └──────────┬───────────────┘ │
│            │ ring buffer                   │ /dev/hackbot     │
└────────────┼───────────────────────────────┼─────────────────┘
             │                               │
             ▼                               ▼
┌─────────────────────────────────────────────────────────────┐
│                       USER SPACE                             │
│                                                              │
│  ┌──────────────────────────────────────────────────────┐   │
│  │  hackbot-server (Rust / Axum + Tokio)                 │   │
│  │  • reads ring buffer → trace events for visualization │   │
│  │  • reads /dev/hackbot → LLM state, responses          │   │
│  │  • writes /dev/hackbot → user prompts to LLM          │   │
│  │  • WebSocket gateway to frontend                      │   │
│  └─────────────────────────┬────────────────────────────┘   │
│                             │ WebSocket                      │
│  ┌─────────────────────────┴────────────────────────────┐   │
│  │  Frontend (TypeScript + Pixi.js → Three.js)           │   │
│  │                                                       │   │
│  │  ┌─────────────┐ ┌─────────────┐ ┌────────────────┐  │   │
│  │  │ Game World   │ │ Chat Panel  │ │ Signal View    │  │   │
│  │  │ (3D kernel)  │ │ (LLM chat)  │ │ (complex plane)│  │   │
│  │  │              │ │             │ │                │  │   │
│  │  │ • Processes  │ │ • Ask agent │ │ • Orbit plot   │  │   │
│  │  │   as rooms   │ │ • See its   │ │ • Anomaly      │  │   │
│  │  │ • Syscalls   │ │   thoughts  │ │   detection    │  │   │
│  │  │   as events  │ │ • Direct it │ │                │  │   │
│  │  │ • Agent as   │ │   anywhere  │ │                │  │   │
│  │  │   character  │ │             │ │                │  │   │
│  │  └─────────────┘ └─────────────┘ └────────────────┘  │   │
│  └───────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────┘
```

### eBPF Verifier as Formal Verification for LLM Actions

The breakthrough insight: the LLM's **action safety problem** can be solved by the kernel's own BPF verifier.

Instead of needing Verus to formally verify every possible action the LLM might take:

1. The LLM **generates eBPF programs** as its "actions" (e.g., "attach a kprobe to function X", "filter packets matching pattern Y", "trace process Z's syscalls")
2. The eBPF programs are submitted to the **kernel's BPF verifier**
3. The verifier **proves safety** (termination, memory bounds, no kernel corruption) or **rejects** the program
4. Only verified programs are loaded and executed

This is elegant because:
- The BPF verifier is battle-tested (millions of production deployments)
- It guarantees: no infinite loops, no out-of-bounds access, no kernel state corruption
- The LLM can be creative/exploratory — the verifier catches any unsafe proposals
- No external formal verification toolchain needed for the action layer
- The action space is naturally bounded by what eBPF helpers permit

**The LLM proposes. The verifier disposes.**

### Reusing the Existing eBPF Tracer

The user's existing eBPF tracer project can be directly integrated:

1. **As-is for Phase 3**: The tracer feeds trace events through a ring buffer to hackbot-server for visualization. Zero changes to the tracer needed.

2. **Extended for LLM observation**: Add BPF map outputs alongside ring buffer. The in-kernel LLM module reads these maps directly (no userspace roundtrip). The tracer becomes the LLM's sensory input.

3. **As a template for LLM-generated probes**: The existing tracer's eBPF programs serve as templates/examples for the LLM to generate new probes. The LLM learns the patterns and can propose variations.

### Two Game Modes (Future Vision)

**Mode 1: Kernel Explorer** — "Tourism"
- Walk through the kernel as a 3D world
- Processes are buildings, memory regions are terrain, syscalls are visible events
- The LLM agent is your guide — follows you, explains what you see
- "What is this process doing?" → Agent inspects and narrates
- "Show me the hottest code path" → Agent leads you there
- Educational tool for OS courses, onboarding kernel developers

**Mode 2: Kernel Hacker** — "Security"
- The LLM autonomously hunts for anomalies
- Visualize attack surfaces, side-channel leaks, resource contention
- CTF-style challenges: "Find the covert channel in this workload"
- Red team: Agent attempts to find vulnerabilities
- Blue team: Agent monitors and alerts on suspicious patterns
- Research tool for kernel security analysis

Both modes share the same infrastructure. The difference is the LLM's objective and the visualization emphasis.

---

## Appendix D: WASM in Kernel — Future Exploration

**Date**: 2026-03-18
**Status**: Deferred. Attractive but adds architectural complexity. Revisit after System 1/2 hybrid is working.

### Existing Kernel WASM Runtimes

| Runtime | Origin | Interpreter | Status |
|---------|--------|-------------|--------|
| **Camblet (wasm3)** | Cisco | wasm3 (fast interpreter) | Production-proven, security-focused |
| **kernel-wasm** | Community | Wasmer | Research-stage |
| **Wasmjit** | Community | Custom JIT | Older, less maintained |

### Potential Use Cases for hackbot

**1. Inference Sandbox (System 1)**
Compile the INT8 inference engine to WASM. The WASM linear memory model gives memory safety by construction — a buggy inference kernel can't corrupt kernel data structures. Import list = capability boundary (only what the host explicitly provides).

**2. Action Sandbox**
More expressive than eBPF for complex agent actions. WASM programs can loop, allocate, and call imported functions — while still being sandboxed. Import validation at load time: if `kill_process` isn't in the import list, WASM code physically cannot call it. Complementary to eBPF verifier (eBPF for observation probes, WASM for complex actions).

**3. Plugin System**
Hot-swappable analysis modules without reloading the kernel module. Users could write custom anomaly detectors compiled to WASM and load them at runtime.

**4. Defense in Depth Stack**
```
Rust type system → WASM sandbox → WASM import validation → eBPF verifier → Verus proofs
```
Each layer catches different classes of bugs. WASM adds a runtime isolation layer that Rust's compile-time checks can't provide for dynamically loaded code.

### Why Deferred

- Adds a third execution model (native Rust + eBPF + WASM) — increases cognitive and build complexity
- System 1/2 hybrid is already complex enough as the next step
- WASM integer-only limitation (no native FPU) may require special handling for inference
- The value becomes clearer once we have a working agent with real action requirements
- Better to prove the architecture works first, then add sandboxing layers

### When to Revisit

- After Step 2a (kernel socket → vLLM) is working
- When the action layer needs more expressiveness than eBPF allows
- If System 1 in-kernel inference needs stronger isolation guarantees
