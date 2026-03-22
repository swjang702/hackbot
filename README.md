# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Custom Instructions
- For every main/important implementation, write the summary into the `docs/IMPLEMENTATION_REPORT.md`.
- I have provided you with two files:
    - The file \@general_index.md contains a list of all the files in the codebase along with a simple description of what it does.
    - The file \@detailed_index.md contains the names of all the functions in the file along with its explanation/docstring.
    This index may or may not be up to date.
- You can take a quick look the vllm/ and the linux-6.19.8/ directories.

## Project Overview

hackbot is an autonomous kernel exploration agent with a game-like visualization of system internals. It renders eBPF trace data as a navigable 2D world where processes are rooms, system calls are animated events, and an AI agent is a character exploring the kernel. See `docs/PLAN.md` for the full implementation plan and `docs/ARCHITECTURE.md` for system diagrams.

## Architecture

Two-process system connected via WebSocket:

- **`server-rs/`** — Rust (Axum + Tokio) backend. Cargo workspace with crates: `hackbot-types` (shared types), `hackbot-server` (main binary), `hackbot-signal` (Phase 2), `hackbot-ebpf` (Phase 3). Loads `.jsonl` trace files, replays events with timing control, maintains world state, serves WebSocket at `/ws`.
- **`server/`** — [DEPRECATED] Original Python 3.12 + FastAPI prototype. Kept for reference during migration.
- **`frontend/`** — TypeScript + Vite + Pixi.js v8 frontend. Four panels: Game View (Pixi.js WebGL — process rooms, syscall animations, pan/zoom camera), Signal View (Canvas 2D — complex plane, Phase 2), Event Log (filterable scrollable list), Timeline (play/pause/speed/scrub).

Data flows: `.jsonl` trace file → `trace_loader` → `trace_replayer` (16ms batching for 60fps) → WebSocket → browser panels. Signal processor (Phase 2) runs a 100ms sliding window computing `z(t) = r(t) * exp(i * theta(t))`.

## Key Technical Decisions

- Rust backend for Verus formal verification alignment (Pillar 4) and native eBPF via `aya` (Phase 3)
- Pixi.js v8 for game view (not a full game engine), raw Canvas 2D for signal charts
- Replay-first (pre-recorded traces), live eBPF streaming deferred to Phase 3
- Tree-based spatial layout for processes (deterministic, matches pstree)
- Object pooling for syscall sprites (~200 max) to avoid browser crashes
- Nanosecond timestamps: `u64` in Rust, serialized as strings in JSON for JS BigInt safety
- JSON Lines for trace format; binary encoding deferred

## Trace Event Schema

Every event: `{ ts, type, pid, tid, cpu, comm, payload }`. Types: `syscall_enter`, `syscall_exit`, `sched_switch`, `power_trace`, `process_fork`, `process_exit`, `gpu_submit`, `gpu_complete`. Payload is type-specific (discriminated union via serde `#[serde(tag = "type")]`).

## Build Commands

```bash
# Backend (Rust)
cd server-rs && cargo build                    # build
cd server-rs && cargo run                      # dev server (port 8000)
cd server-rs && cargo run -- --generate-mock   # generate sample trace data

# Frontend
cd frontend && pnpm install                    # install deps
cd frontend && pnpm dev                        # vite dev server (port 5173)

# Run both (from project root)
# Terminal 1: cd server-rs && cargo run
# Terminal 2: cd frontend && pnpm dev
# Open http://localhost:5173 (or SSH tunnel: ssh -L 5173:localhost:5173 -L 8000:localhost:8000 fedora)
```

## Implementation Phases

1. **Phase 1**: Static trace replay viewer (Rust backend + frontend game view + controls) ✅
2. **Phase 2**: Complex plane signal view (`hackbot-signal` crate + orbit plot + anomaly detection)
3. **Phase 3**: Real-time eBPF streaming (`hackbot-ebpf` crate with `aya`)
4. **Phase 4**: Agent character (visible bot navigating process rooms)
5. **Phase 5**: LLM brain (OODA loop with autonomous exploration)

Build strictly in order. Within backend: types → mock_data → trace_loader → world_model → trace_replayer → gateway → main.

## MCP Servers Available

context7 (library docs), puppeteer (browser testing), sequential-thinking (reasoning), deepwiki (repo docs), pdf-reader (research PDFs in `docs/refs/`), chroma (vector DB at `/home/sunwoo/projects/claude_setup/chroma/`).

## Research Context

Read `docs/refs/Research_Statement.pdf` and `docs/refs/Connecting the dots...pdf` for the research vision. The four pillars: (1) autonomous kernel bot, (2) visualization as gameplay, (3) complex plane anomaly detection, (4) mathematical self-improvement. The MVP covers pillars 1-2 (visualization) with early pillar 3 (complex plane).

## Research Ideas
- Verus for formal verification
- in-kernel LLM
- What about leveraging WASM?
