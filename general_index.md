# hackbot — General File Index

> Last updated: 2026-03-08

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
