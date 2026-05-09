# hackbot viz — v0.1.0

**Tag**: `viz-v0.1.0`
**Date**: 2026-05-09
**Scope**: trace-replay visualization (Phase 1 of `docs/PLAN.md`)

This is the first tagged release of the visualization stack — the `server-rs`
backend and `frontend` browser client. It is independent of the in-kernel
agent (`kmod-v0.1.0`); either may be used without the other.

## What's in this release

### Backend — `server-rs/`
- Cargo workspace with two crates: `hackbot-types` (shared types) and
  `hackbot-server` (the binary).
- Axum + Tokio HTTP/WebSocket server on port 8000.
- Loads pre-recorded `.jsonl` trace files; replays events at original timing
  in 16 ms batches (60 fps) over WebSocket.
- World model tracks processes, file descriptors, and connections derived
  from the event stream. Supports seek via `rebuild_to(ts)` (binary search
  + replay).
- Mock trace generator (`cargo run -- --generate-mock`) produces a
  deterministic ~8.9 k-event LLM-workload narrative across 5 phases.
- `cargo check --workspace` is warning-clean.

### Frontend — `frontend/`
- TypeScript + Vite + Pixi.js v8.
- **Game view** — process rooms rendered as labeled rectangles in a
  pstree-derived spatial layout, with pan/zoom, syscall animation pool
  (200 pre-allocated graphics), and event particle bursts.
- **Event log** — filterable, color-coded, auto-scrolling.
- **Timeline controls** — play/pause, speed selection (0.5×–10×), scrub
  slider with BigInt-safe position math.
- **Filter chips** — by PID and event type (paired toggles for
  enter/exit / submit/complete / fork/exit).
- WebSocket auto-reconnect with exponential backoff.
- `pnpm exec tsc --noEmit` is clean.

### Trace data
- `traces/sample-llm-workload.jsonl` — 8.9 k-event sample.
- `traces/format.md` — JSON Lines schema spec.

## Running

```bash
cd server-rs && cargo run                     # backend on :8000
cd frontend && pnpm install && pnpm dev       # frontend on :5173
# Open http://localhost:5173
```

`cargo run -- --generate-mock` re-creates the sample trace if needed.

## Not in this release

- **Phase 2 — complex-plane signal view**. The `hackbot-signal` crate and
  the orbit-plot panel are not yet implemented. The `MVP exit criterion`
  for the complex-plane orbit view from `docs/PLAN.md` is therefore not
  yet satisfied — that will land in `viz-v0.2`.
- **Phase 3 — live eBPF streaming**. Replay-only for now.
- **Phase 4 — agent character in the world view**.
- **Phase 5 — LLM-driven narration**.

## Compatibility

- WebSocket protocol matches the legacy Python prototype byte-for-byte;
  the prototype was removed earlier in the project, but the protocol is
  documented in `docs/IMPLEMENTATION_REPORT.md` and `docs/ARCHITECTURE.md`.
- Tested on Linux 6.19.8 (Fedora) with Rust stable, Node 20+, pnpm.

## Known limitations

- Mock trace event count differs slightly from the historical Python
  prototype (8912 Rust vs 8589 Python) due to RNG implementation —
  protocol-level identical.
- The world model's `FdEntry` keeps reserved fields for richer modeling
  in later phases (`_fd_type`, `_path`); both are intentionally
  underscore-prefixed.

## Verification

```bash
cd server-rs && cargo check --workspace        # clean, zero warnings
cd frontend && pnpm exec tsc --noEmit          # clean, zero errors
```
