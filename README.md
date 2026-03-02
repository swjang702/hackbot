## Project Overview

hackbot is an autonomous kernel exploration agent with a game-like visualization of system internals. It renders eBPF trace data as a navigable 2D world where processes are rooms, system calls are animated events, and an AI agent is a character exploring the kernel. The project is pre-implementation — see `docs/PLAN.md` for the full implementation plan and `docs/ARCHITECTURE.md` for system diagrams.

## Architecture (Planned)

Two-process system connected via WebSocket:

- **`server/`** — Python 3.12 + FastAPI backend. Loads `.jsonl` trace files, replays events with timing control, maintains world state (process map, fd table, connections), computes complex plane signal analysis (numpy/scipy). Serves a WebSocket endpoint at `/ws`.
- **`frontend/`** — TypeScript + Vite + Pixi.js v8 frontend. Four panels: Game View (Pixi.js WebGL — process rooms, syscall animations, pan/zoom camera), Signal View (Canvas 2D — complex plane orbit plot, phase diagram), Event Log (filterable scrollable list), Timeline (play/pause/speed/scrub).

Data flows: `.jsonl` trace file → `trace_loader` → `trace_replayer` (16ms batching for 60fps) → WebSocket → browser panels. Signal processor runs a 100ms sliding window computing `z(t) = r(t) * exp(i * theta(t))` where r = syscall rate and theta = entropy of syscall type distribution.

## Key Technical Decisions

- Pixi.js v8 for game view (not a full game engine), raw Canvas 2D for signal charts
- Replay-first (pre-recorded traces), live eBPF streaming deferred to Phase 3
- Python backend (not all-TypeScript) to access numpy/scipy and eBPF ecosystem
- Tree-based spatial layout for processes (deterministic, matches pstree)
- Object pooling for syscall sprites (~200 max) to avoid browser crashes
- Nanosecond timestamps as BigInt in frontend (JS Number unsafe beyond 2^53)
- JSON Lines for trace format; binary encoding (MessagePack) deferred

## Trace Event Schema

Every event: `{ ts, type, pid, tid, cpu, comm, payload }`. Types: `syscall_enter`, `syscall_exit`, `sched_switch`, `power_trace`, `process_fork`, `process_exit`, `gpu_submit`, `gpu_complete`. Payload is type-specific (discriminated union).

## Build Commands (once scaffolded)

```bash
# Backend
cd server && uv sync                    # install deps
cd server && uv run uvicorn server.main:app --reload  # dev server

# Frontend
cd frontend && pnpm install             # install deps
cd frontend && pnpm dev                 # vite dev server

# Generate sample trace data
cd server && uv run python -m server.mock_data
```

## Implementation Phases

1. **Phase 1**: Static trace replay viewer (backend core + frontend game view + controls)
2. **Phase 2**: Complex plane signal view (signal processor + orbit plot + anomaly detection)
3. **Phase 3**: Real-time eBPF streaming
4. **Phase 4**: Agent character (visible bot navigating process rooms)
5. **Phase 5**: LLM brain (OODA loop with autonomous exploration)

Build strictly in order. Phase 1A (backend) before 1B (frontend). Within 1A: schemas → mock_data → trace_loader → world_model → trace_replayer → gateway → main.

## MCP Servers Available

context7 (library docs), puppeteer (browser testing), sequential-thinking (reasoning), deepwiki (repo docs), pdf-reader (research PDFs in `docs/refs/`), chroma (vector DB at `/home/sunwoo/projects/claude_setup/chroma/`).

## Research Context

Read `docs/refs/Research_Statement.pdf` and `docs/refs/Connecting the dots...pdf` for the research vision. The four pillars: (1) autonomous kernel bot, (2) visualization as gameplay, (3) complex plane anomaly detection, (4) mathematical self-improvement. The MVP covers pillars 1-2 (visualization) with early pillar 3 (complex plane).
