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

## Next: Phase 2 — Complex Plane Signal View

- `hackbot-signal` crate: sliding window, feature extraction, z(t) = r(t) * exp(i * theta(t))
- Frontend: Canvas 2D orbit plot + phase diagram panel
- Anomaly detection: EMA + standard deviation threshold
