# hackbot Kernel Module

Rust + C kernel module that creates `/dev/hackbot` — the in-kernel LLM agent interface for Linux 6.19.8.

## Overview

The module is an autonomous OODA agent living in ring 0. Write a prompt to `/dev/hackbot`, read the LLM response back. The agent reasons over live kernel state, calls observation tools, and is continuously fed by always-on tracepoint sensors — all without leaving kernel space.

```
write("/dev/hackbot", prompt)
  → agent loop (OODA): reason → call tool → observe → repeat
      tools (Tier 0–1):
        ps, mem, loadavg, dmesg, files, kprobe, trace
      sensors (always-on, registered at module load):
        sched_switch, sys_enter, block_rq_complete
      memory:
        ring buffer of past findings, injected into the system prompt
  → response written to a device-global buffer
read("/dev/hackbot")
  → returns the latest response (any fd can read)
```

## Capabilities

### Inference backends

| Backend | How | When to use |
|--------|-----|-------------|
| **Local INT8** (Q16.16 fixed-point) | `hackbot_forward.rs` + `hackbot_math.rs` | No FPU; runs on any kernel. Precision-bounded for ~135M models. |
| **Local FP16** (kernel FPU) | `hackbot_fpu.c` (compiled with `-mhard-float -msse -msse2`, bracketed by `kernel_fpu_begin/end`) | Better multi-token coherence. Temperature + top-K sampling. |
| **Remote vLLM** | Kernel TCP socket (`sock_create_kern` / `kernel_connect` / `kernel_sendmsg` / `kernel_recvmsg`) | Best quality (e.g. `Meta-Llama-3.3-70B-Instruct-AWQ-INT4`); offloads compute. |

Backend selection: `INFERENCE_MODE` constant in `hackbot_config.rs` (0=auto, 1=local-only, 2=vllm-only). Auto falls back to vLLM if the local model isn't loaded.

### Tools available to the agent

| Tool | Tier | Implementation |
|------|------|----------------|
| `ps` | 0 (read) | Two-pass walk of `init_task.tasks` under RCU |
| `mem` | 0 | `si_meminfo()` |
| `loadavg` | 0 | `avenrun[]` (FSHIFT=11 fixed-point) |
| `dmesg [N]` | 0 | 64KB ring buffer fed by a registered `struct console` |
| `files <pid>` | 0 | `find_vpid()` → `pid_task()` → `d_path()` per FD |
| `kprobe attach\|check\|detach <func>` | 1 (instrument) | `register_kprobe()`, atomic64 hit counters, max 8 slots |
| `trace sched\|syscall\|io\|raw\|reset\|list` | 0 | Live tracepoint data (raw / LinnOS-style features / aggregates) |

### Always-on sensors

Three tracepoints registered at module load via `for_each_kernel_tracepoint()` + `tracepoint_probe_register()`. Each callback updates three tiers in parallel:
1. Raw event ring buffer (last 1024 events with full context)
2. LinnOS-style sliding feature window (last 4 switch intervals / I/O latencies)
3. Atomic aggregate counters and an 8-bucket I/O latency histogram

Tracepoints: `sched_switch`, `sys_enter`, `block_rq_complete`. Total overhead budget ~3% of one core on a busy server.

### Autonomous patrol

`hackbot_patrol.c` — a kthread (`[hackbot_patrol]`) that wakes every 120s, runs the agent loop with an anomaly-detection prompt, and records findings into the agent memory ring buffer. Clean shutdown via `kthread_stop()`.

### Agent memory

`hackbot_memory.rs` — fixed-size ring (8 entries × 512 B) of timestamped findings (patrol + user). Structured tool-call tracking. Injected into the system prompt before every call. Context-aware truncation keeps long conversations within model limits.

### Kernel-as-language anomaly detection

`hackbot_ngram.c` + `hackbot_tokenizer.c` — in-kernel semantic event tokenizer and online n-gram learner. Treats kernel event sequences as a language; flags high-perplexity windows as anomalies. See `docs/KERNEL_AS_LANGUAGE_PLAN.md`.

## Safety

| Mechanism | Where |
|-----------|-------|
| KASAN / UBSAN / KCSAN opt-in | `Kbuild` (active when host kernel has `CONFIG_KASAN/UBSAN/KCSAN=y`) |
| `-Wframe-larger-than=1024` | catches large stack frames in C helpers |
| `-Wvla` | bans variable-length arrays |
| `make check` (sparse, `C=2`) | lock discipline, `__rcu`/`__user` misuse, sleeping-in-atomic |
| `make check-warn` (`W=1`) | extra kernel warnings |
| `make check-kconfig` | verifies host kernel has KASAN/UBSAN/KCSAN/LOCKDEP/PROVE_LOCKING enabled |

All tools are read-only or reversibly instrumenting. Kprobes have bounded slots (max 8) and clean shutdown on `rmmod`.

## Prerequisites

### 1. Install bindgen-cli (Fedora)

```bash
sudo dnf install bindgen-cli
```

### 2. Configure the kernel with Rust support

```bash
cd ~/sources/linux-6.19.8
cp /boot/config-$(uname -r) .config
scripts/config --enable CONFIG_RUST
scripts/config --enable CONFIG_SAMPLES
scripts/config --enable CONFIG_SAMPLES_RUST
scripts/config --enable CONFIG_SAMPLE_RUST_MISC_DEVICE
make olddefconfig
make rustavailable
```

For full safety coverage, also enable `CONFIG_KASAN`, `CONFIG_UBSAN`, `CONFIG_KCSAN`, `CONFIG_LOCKDEP`, `CONFIG_PROVE_LOCKING`, `CONFIG_DEBUG_ATOMIC_SLEEP`. Verify with `make check-kconfig`.

### 3. Prepare the kernel for module building

```bash
cd ~/sources/linux-6.19.8
make modules_prepare -j$(nproc)
```

### 4. Provide a model (optional — only needed for local inference)

Build a binary with `tools/export_hackbot.py` (v1 INT8) or `tools/export_hackbot_fp16.py` (v2 FP16). The kernel module loads it via the firmware API (`/lib/firmware/`).

### 5. Provide a vLLM server (optional — only needed for remote inference)

```bash
# On a GPU host:
vllm serve <model> --port 8000

# Point the kmod at it via VLLM_ADDR / VLLM_PORT in hackbot_config.rs.
# Or tunnel: ssh -L 8000:localhost:8000 gpu-server
```

## Build & run

```bash
cd ~/projects/hackbot/hackbot-kmod
make                        # build hackbot.ko
make load                   # insmod (requires sudo)
sudo chmod 666 /dev/hackbot
echo "what's consuming memory on this system?" > /dev/hackbot
cat /dev/hackbot
make unload
```

### Make targets

| Target | What it does |
|--------|--------------|
| `make` | Build the module |
| `make clean` | Clean build artifacts |
| `make load` / `make unload` | insmod / rmmod |
| `make test` | Smoke test: load → write → read → unload |
| `make test-full` | Full integration suite (`test.sh full`) |
| `make test-tools` | Exercise all tools via vLLM |
| `make test-patrol` | Wait ~35s and verify the patrol kthread fires |
| `make check` | Sparse static analysis (`C=2`) |
| `make check-warn` | Extra kernel warnings (`W=1`) |
| `make check-all` | sparse + warnings |
| `make check-kconfig` | Verify host kernel safety configs |

## Source layout

| File | Role |
|------|------|
| `hackbot_main.rs` | Module root — declares submodules via `#[path]` |
| `hackbot_config.rs` | Constants — vLLM address, agent limits, prompts, magic |
| `hackbot_types.rs` | Shared types + extern "C" FFI declarations |
| `hackbot_state.rs` | Global mutable state behind `global_lock!` |
| `hackbot_device.rs` | `MiscDevice` for `/dev/hackbot` |
| `hackbot_agent.rs` | Local OODA agent loop (in-kernel inference) |
| `hackbot_vllm.rs` | Remote OODA agent loop + vLLM HTTP client |
| `hackbot_forward.rs` | Transformer forward pass (INT8 path; delegates v2 to FPU) |
| `hackbot_math.rs` | Q16.16 fixed-point math (matmul, RMSNorm, RoPE, softmax, SiLU) |
| `hackbot_model.rs` | Model firmware loading and weight offset computation |
| `hackbot_tokenizer.rs` | GPT-2 BPE tokenizer for SmolLM2 |
| `hackbot_tools.rs` | Tool dispatch and Rust-side tool implementations |
| `hackbot_context.rs` | Live kernel state gathering for the system prompt |
| `hackbot_memory.rs` | Agent memory ring buffer + tool tracking |
| `hackbot_net.rs` | Kernel TCP socket wrapper |
| `hackbot_fpu.c` | FP16/float32 forward pass (kernel FPU) |
| `hackbot_console.c` | 64KB printk ring buffer for `dmesg` tool |
| `hackbot_files.c` | FD table walker for `files` tool |
| `hackbot_kprobe.c` | Kprobe attach/check/detach manager |
| `hackbot_patrol.c` | Autonomous patrol kthread |
| `hackbot_trace.c` | Three-tier tracepoint sensing layer |
| `hackbot_tokenizer.c` / `hackbot_ngram.c` | Semantic kernel-event tokenizer + n-gram learner |
| `kernel_version.rs` | Auto-generated by Kbuild (`KERNELRELEASE`) |
| `Kbuild` / `Makefile` | Build harness with sanitizer opt-ins and `check*` targets |
| `test.sh` / `TESTING.md` | Integration suite + manual testing reference |

## Troubleshooting

### "Connection refused" (errno 111)
vLLM server is not running or unreachable.
```bash
curl http://<vllm-host>:8000/v1/models
```

### "module verification failed" / "Invalid module format"
The module must be built against the same kernel version you're running. Either boot the matching kernel or rebuild against your running kernel's source tree (`make KDIR=/path/to/sources`).

### Local inference produces garbage on long prompts
Q16.16 precision is bounded; it accumulates error across 30 transformer layers × N prefill tokens. Switch to v2 FP16 or to vLLM for longer interactions. See `docs/IMPLEMENTATION_REPORT.md` Step 2e for the diagnosis.

## Status

Through Steps 2h, 2i, and 2k of the in-kernel LLM agent track plus the kernel-as-language anomaly detection layer. See `docs/PLAN.md` and `docs/IMPLEMENTATION_REPORT.md` for full history. The remaining sub-step of the Step 2 group, Step 2j (cross-subsystem anomaly demo), is the next planned milestone.
