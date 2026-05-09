# hackbot kmod — v0.1.0

**Tag**: `kmod-v0.1.0`
**Date**: 2026-05-09
**Scope**: in-kernel LLM agent — `hackbot-kmod/` and supporting `tools/`

This is the first tagged release of the in-kernel agent. It is a research
preview, independent of the visualization stack (`viz-v0.1.0`).

## What's in this release

### Module — `hackbot-kmod/`
A Linux 6.19.8 kernel module written in Rust + C that creates `/dev/hackbot`.
Write a prompt; read the LLM response. The module hosts an OODA agent loop
that observes live kernel state, calls observation tools, runs an autonomous
patrol kthread, and (since this release) tokenizes and learns kernel-event
sequences online for anomaly detection.

#### Inference backends

| Backend | Path |
|---------|------|
| Local INT8 (Q16.16 fixed-point) | `hackbot_forward.rs` + `hackbot_math.rs` |
| Local FP16 / float32 (kernel FPU) | `hackbot_fpu.c` with `kernel_fpu_begin/end` guards, temperature + top-K sampling |
| Remote vLLM | Kernel TCP socket (`sock_create_kern` / `kernel_connect` / `kernel_sendmsg` / `kernel_recvmsg`) — auto-discovers model name via `/v1/models`; uses `/v1/chat/completions` |

Backend selection is driven by `INFERENCE_MODE` in `hackbot_config.rs`
(0 = auto, 1 = local-only, 2 = vllm-only). Default vLLM target is tuned for
`Meta-Llama-3.3-70B-Instruct-AWQ-INT4`.

#### Tools (Tier 0–1, read-only or reversibly instrumenting)

`ps`, `mem`, `loadavg`, `dmesg`, `files`, `kprobe attach|check|detach <func>`,
`trace sched|syscall|io|raw|reset|list|tokens|ngram|ngram stats|ngram alerts`.

#### Always-on sensors

Three tracepoints registered at module load via `for_each_kernel_tracepoint()`
+ `tracepoint_probe_register()`: `sched_switch`, `sys_enter`,
`block_rq_complete`. Each callback updates a 1024-event raw ring, a
LinnOS-style sliding feature window, and atomic aggregate counters in
parallel. Total overhead budget ~3 % of one core.

#### Autonomous patrol kthread

`[hackbot_patrol]` wakes every 120 s, runs the agent loop with an
anomaly-detection prompt, and records findings into structured agent memory.
Wakes early on n-gram anomaly alert. Clean shutdown via `kthread_stop()`.

#### Structured agent memory (HyperAgents-inspired)

Ring of 8 × 512 B entries. Each entry stores `summary`, `tools_used`,
`n_tool_calls`, and `detail`. Injected into the system prompt before every
call so the LLM sees not just past findings but *how* they were produced.

#### Kernel-as-language anomaly layer

- `hackbot_tokenizer.c/h` — semantic 8-field tokenizer over
  tracepoint events (~200–400 ns per token).
- `hackbot_ngram.c/h` — dual-model n-gram learner (baseline + adaptive
  with halving), Hogwild lock-free updates, ~66 KB total memory.
- Anomaly detection with NORMAL/ANOMALY/DRIFT/REGRESSION classification,
  100 ms debounce, 30 s init grace period, gated adaptive learning to
  resist adversarial normalization.

#### Safety

- `Kbuild` opts the module into KASAN / UBSAN / KCSAN — instrumented when
  the host kernel has them enabled, zero overhead otherwise.
- Compiler hardening: `-Wframe-larger-than=1024`, `-Wvla`.
- `make check` (sparse `C=2`), `make check-warn` (`W=1`), `make check-all`,
  `make check-kconfig`.
- Recent fix campaign (commit `046b4c9`) addressed four root-cause kernel
  panics from races and locking violations.

### Tools — `tools/`
Python utilities for model export and verification.

- `export_hackbot.py` — HF SmolLM2-135M-Instruct → hackbot binary v1
  (INT8 + Q16.16 scales). Output: ~143 MB.
- `export_hackbot_fp16.py` — same, format v2 (FP16 weights). Output:
  ~270 MB.
- `verify_hackbot.py`, `verify_prefill.py`, `verify_generation.py`,
  `verify_tokenizer.py`, `verify_fp16_forward.py` — correctness
  verifiers (HF reference vs hackbot binary, vs in-kernel forward pass).
- `int8_reference.py` — Python reference INT8 forward pass for diff'ing.

## Running

```bash
cd hackbot-kmod
make                                # build hackbot.ko (KDIR defaults to ~/sources/linux-6.19.8)
make load                           # insmod (sudo)
sudo chmod 666 /dev/hackbot
echo "what's consuming memory?" > /dev/hackbot
cat /dev/hackbot
make unload
```

`make test-full`, `make test-tools`, `make test-patrol` run integration
suites. `make check-all` runs sparse + extra warnings; `make check-kconfig`
verifies the host kernel has the safety options enabled.

## Model binaries

The 143 MB INT8 and ~270 MB FP16 binaries are not in the git repository
(too large). They are attached as release assets to this tag; place them
at the firmware path the module expects (`/lib/firmware/...`), or rebuild
locally with `tools/export_hackbot.py` / `tools/export_hackbot_fp16.py`.

## Implementation status

- Phase 1–2c: complete (vLLM via kernel socket, kernel context injection,
  OODA agent loop with kernel tools).
- Steps 3a–3f: complete (in-kernel INT8 inference, FP16 + FPU path, model
  export tools).
- Steps 2d–2g: complete (tools expansion, FP16 sampling, patrol kthread,
  agent memory ring, continuous tracepoint sensing).
- Steps 2h, 2i, 2k: complete (config consolidation + I/O clock fix,
  context-aware truncation, structured agent memory with tool tracking).
- Kernel-as-language Milestones 0–2: complete (semantic tokenizer, n-gram
  learning, anomaly detection).

## Not in this release

- **Step 2j** — cross-subsystem anomaly demo end-to-end. Designed but not
  shipped; will land in `kmod-v0.2`.
- **Action capability (Tier 2+)**. The agent is observe-only or
  reversibly instrumenting. Mutating kernel state is gated on Verus
  formal verification work that is not yet started.
- **GPU/NPU acceleration**. The Linux 6.19.8 accel framework exposes no
  kernel-internal compute submission API; see the GPU/NPU feasibility
  research note in `docs/IMPLEMENTATION_REPORT.md`.

## Hardware tested

- AMD Ryzen 5 PRO 4650G (Zen 2, 6c/12t, AVX2, no AVX-512).
- 14 GB DDR4. Local FP16 inference ~10 ms/token.
- Linux 6.19.8 with `CONFIG_RUST=y`.

## Known limitations

- Local Q16.16 INT8 inference accumulates precision error across 30
  transformer layers; long prompts produce repetitive output. Use FP16 or
  the vLLM backend for longer interactions. See Step 2e diagnosis in
  `docs/IMPLEMENTATION_REPORT.md`.
- Patrol kthread shutdown blocks until the current vLLM call completes
  (acceptable; no deadlock risk because the call has its own TCP timeout).
