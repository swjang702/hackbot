# hackbot Implementation Plan

**Date**: 2026-03-02 (updated 2026-03-08)
**Status**: Phase 1 complete (Python), rewriting backend to Rust
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

### Phase 3: Real-time Streaming

**Goal**: Connect to a live eBPF data source instead of replaying files.

**Duration estimate**: 1-2 weeks

| Task | File(s) | Description |
|------|---------|-------------|
| 3.1 | `server/server/event_ingestion.py` (new) | TCP or Unix domain socket server that receives events from an external eBPF collector process. Protocol: newline-delimited JSON (same schema as trace files). Parse and feed into the same pipeline as trace_replayer. |
| 3.2 | `server/server/gateway.py` | Add mode switching: "replay" mode (from file) vs "live" mode (from ingestion). In live mode, events are forwarded immediately (no timing replay needed). |
| 3.3 | `server/server/world_model.py` | Ensure world model handles events arriving out of order (possible with live data from multiple CPUs). Use timestamp for ordering within a small reorder buffer (e.g., 10ms). |
| 3.4 | `server/server/signal_processor.py` | Ensure streaming computation works: sliding window advances in real time, not trace time. |
| 3.5 | `frontend/src/connection.ts` | Add UI indicator for connection mode (replay vs live). In live mode, hide scrub bar; show only pause/resume and speed controls. |
| 3.6 | `server/server/main.py` | Add CLI flag or API endpoint to switch between replay and live mode. |

---

### Phase 4: Agent Character

**Goal**: Add the hackbot as a visible character navigating the system world. Initially human-controlled.

**Duration estimate**: 1-2 weeks

| Task | File(s) | Description |
|------|---------|-------------|
| 4.1 | `frontend/src/game/agent-character.ts` | Pixi.js sprite/graphic representing the agent. A small animated icon (e.g., a glowing dot with a directional indicator) that smoothly moves between process rooms. Has a "field of view" visual (semi-transparent circle showing the agent's current observation area). |
| 4.2 | `server/server/agent_state.py` (new) | Agent state model: current_position (pid of process being examined), attention_area (list of pids being monitored), action_history (recent actions taken), capabilities (available actions). |
| 4.3 | `frontend/src/game/world.ts` | Add click handler on process rooms. Clicking a room sends `agent_move` command to server. Agent character smoothly animates to the clicked room. |
| 4.4 | `frontend/src/ui/agent-panel.ts` (new) | Side panel showing agent's current state: which process it is examining, what events it sees, suggested next actions (hardcoded heuristics for now, LLM-driven in Phase 5). |
| 4.5 | `server/server/gateway.py` | Handle `agent_move` commands. Update agent state. Send `agent_state` messages to client with position, attention area, and any observations. |

---

### Phase 5: LLM Brain Integration

**Goal**: The hackbot makes autonomous decisions about where to look and what to examine.

**Duration estimate**: 2-3 weeks

| Task | File(s) | Description |
|------|---------|-------------|
| 5.1 | `server/server/agent_brain.py` (new) | OODA loop implementation. On each decision cycle (configurable interval, e.g., every 2 seconds): (a) **Observe**: summarize current system state from world_model (process list, recent event counts, anomaly flags). (b) **Orient**: include context from memory (previous findings, exploration history). (c) **Decide**: send summary to LLM API, receive action selection. (d) **Act**: execute the chosen action (move to process, enable new probe filter, flag anomaly). |
| 5.2 | `server/server/llm_client.py` (new) | Async client for LLM API (support Anthropic Claude and OpenAI GPT). Takes system state summary string, available actions list, and returns selected action with reasoning. Configurable model, temperature, max tokens. |
| 5.3 | `server/server/agent_state.py` | Extend with action types: `move_to(pid)`, `focus_on(event_type)`, `flag_anomaly(description)`, `adjust_filter(params)`. Each action has a defined effect on the world model / gateway filters. |
| 5.4 | `frontend/src/ui/agent-panel.ts` | Extend to show LLM reasoning: display the agent's thought process as text, show the action it chose and why, show alternatives it considered. Display as a scrollable log with timestamps. |
| 5.5 | `frontend/src/game/agent-character.ts` | Add visual indicators for agent state: "thinking" animation while LLM is processing, "acting" animation when executing, "idle" when waiting for next cycle. |
| 5.6 | `server/server/gateway.py` | Add human override commands: `agent_pause` (stop autonomous decisions), `agent_resume`, `agent_redirect(pid)` (force agent to examine a specific process). |
| 5.7 | `server/server/memory_store.py` (new) | Simple JSON-file-based persistence for agent findings. Stores: discovered anomalies, explored paths, pattern library (known-normal and known-anomalous event sequences). Loaded on startup, saved periodically. |

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
### Concern 2: in-kernel LLM?

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
