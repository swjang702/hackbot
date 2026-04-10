# hackbot

**An autonomous kernel agent that observes, reasons about, and learns from Linux internals -- from ring 0.**

---

## Thesis

Existing kernel observability tools (bpftrace, bcc, perf) work *from outside* the kernel: userspace programs attach eBPF probes, pull data through ring buffers, and analyze it in a separate process. The observation-reasoning-action loop crosses privilege boundaries at every step.

hackbot inverts this. It is a **Rust kernel module** that lives in ring 0. It registers tracepoint callbacks directly, reasons about what it observes via an LLM (currently offloaded to vLLM over a kernel TCP socket), and maintains structured memory across investigation sessions -- all without leaving kernel space for observation. The entire sense-think-act loop stays inside the kernel.

A second thesis drives the anomaly detection layer: **kernel event sequences have linguistic structure**. System calls, context switches, and I/O operations form a vocabulary with syntax and semantics. A language model trained on these sequences can detect anomalies by measuring *surprise* -- the same math that powers n-gram spell-checkers and GPT, applied to kernel behavior.

## Key Ideas

- **Ring-0 observation.** hackbot registers kernel tracepoint callbacks directly (`register_trace_sched_switch`, `register_trace_sys_enter`, etc.) instead of going through eBPF from userspace. Zero context switches for observation. The kernel watches itself.

- **Kernel as language.** Treat event sequences as sentences in a kernel language. Train n-gram and transformer models on normal behavior. Anomaly = high perplexity. See [`docs/KERNEL_AS_LANGUAGE_PLAN.md`](docs/KERNEL_AS_LANGUAGE_PLAN.md).

- **Five-layer autonomic architecture.** Sensory (direct trace callbacks, <1us) → Reflex (tiny NNs for fast classification, <10us) → Reasoning (LLM agent via OODA loop) → Self-improvement (strategy archive, online retraining) → Safety (BPF verifier + Verus proofs as formal gates). See [`docs/refs/hackbot_vision_synthesis.md`](docs/refs/hackbot_vision_synthesis.md).

- **eBPF as safety gate, not observation tool.** For *observation* (tiers 0-1), hackbot uses direct kernel callbacks -- no eBPF overhead. For *action* (tier 2+), hackbot *generates* BPF programs that the verifier checks before execution. eBPF becomes a sandboxed execution layer, like an MMU for agent actions.

- **Visualization as gameplay.** A browser-based 2D world where processes are rooms, system calls are animated events, and (eventually) the agent is a visible character exploring the kernel. Built with Pixi.js v8 over WebSocket.

## Architecture

```
hackbot/
├── hackbot-kmod/    Rust kernel module: /dev/hackbot, LLM agent, 7 tools, 3 sensors
├── server-rs/       Rust backend (Axum): trace replay, world state, WebSocket gateway
├── frontend/        TypeScript + Pixi.js v8: game view, signal view, event log, timeline
├── tools/           Model export and verification scripts
├── traces/          Sample trace data (.jsonl)
└── docs/            Design documents, architecture diagrams, research references
```

**In-kernel agent path:**
```
tracepoint callbacks ──→ hackbot-kmod ──→ kernel TCP socket ──→ vLLM (gemma-4-31B)
                              │                                        │
                              └── structured memory ←── tool calls ←───┘
```

**Visualization path:**
```
.jsonl trace ──→ trace_loader ──→ trace_replayer (16ms batches, 60fps) ──→ WebSocket ──→ browser
```

## Status

| Component | State |
|-----------|-------|
| Trace replay viewer (Phase 1) | **Complete** |
| In-kernel LLM agent | Through Step 2k: 7 tools, 3 always-on sensors, patrol kthread, structured agent memory, context-aware truncation |
| Complex plane signal view (Phase 2) | Planned |
| Real-time eBPF streaming (Phase 3) | Planned |
| Agent character in game (Phase 4) | Planned |
| LLM brain with OODA loop (Phase 5) | Planned |
| Kernel-as-language anomaly detection | Design complete, implementation planned |

## Getting Started

### Visualization server

```bash
cd server-rs && cargo build && cargo run          # backend on :8000
cd frontend && pnpm install && pnpm dev           # frontend on :5173
```

Open `http://localhost:5173`. Use `cargo run -- --generate-mock` to create sample trace data.

### Kernel module

Requires a kernel with `CONFIG_RUST=y` and a reachable vLLM server (default `127.0.0.1:8000`).

```bash
cd hackbot-kmod
make                        # build hackbot.ko
make load                   # insmod
sudo chmod 666 /dev/hackbot
echo "What's happening on this system?" > /dev/hackbot
cat /dev/hackbot
make unload                 # rmmod
```

Run `make test-full` for the integration suite. See [`hackbot-kmod/`](hackbot-kmod/) for details.

## Documentation

| Document | Description |
|----------|-------------|
| [`docs/PLAN.md`](docs/PLAN.md) | Full implementation plan with phase breakdown |
| [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) | System diagrams and data flow |
| [`docs/KERNEL_AS_LANGUAGE_PLAN.md`](docs/KERNEL_AS_LANGUAGE_PLAN.md) | N-gram/transformer anomaly detection design |
| [`docs/refs/hackbot_vision_synthesis.md`](docs/refs/hackbot_vision_synthesis.md) | Five-layer autonomic OS architecture vision |

## Related Work

- **LinnOS** (OSDI 2020) -- Learned predictions for I/O scheduling in the Linux kernel. hackbot generalizes this to multiple subsystems and adds an LLM reasoning layer.
- **HyperAgents** (Meta, 2026) -- Self-improving agent architectures with strategy archives. hackbot's Layer 3 draws on this for meta-evaluation and investigation refinement.
- **eBPF/bcc/bpftrace** -- The standard kernel observability stack. hackbot uses direct tracepoint registration instead for observation, and repurposes eBPF as a safety gate for agent actions.
- **Verus** -- Formal verification for Rust. The long-term safety layer (Layer 4) for proving kernel module correctness.

## License

Apache 2.0
