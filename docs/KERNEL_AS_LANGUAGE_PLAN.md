# Kernel as Language: 24/7 Perpetual Learning Plan

**Date**: 2026-04-03
**Status**: Planning
**Thesis**: Kernel event sequences have linguistic structure — vocabulary, morphology, syntax, semantics, prosody, discourse — and language modeling techniques (from n-grams to transformers) can detect anomalies by measuring surprise.

---

## 0. The Big Idea

**Anomaly detection doesn't require understanding. It requires expectation.**

Train a model to predict the next kernel event. When reality diverges from prediction, that's surprise. High surprise = anomaly. The same math powers both n-gram spell-checkers and GPT — the difference is the quality of the expectation.

The kernel already speaks a language. We're learning to listen.

---

## 1. Architecture Overview

```
KERNEL SPACE
┌──────────────────────────────────────────────────────────────┐
│                                                              │
│  Tracepoints (ftrace initially, eBPF later)                  │
│  ├─ sched/sched_switch, sched_process_fork/exit/exec         │
│  ├─ syscalls/sys_enter_{openat,close,read,write,...} (~15)   │
│  ├─ block/block_rq_issue, block_rq_complete                  │
│  └─ tcp/tcp_connect, tcp_accept, tcp_close                   │
│                                                              │
│  hackbot-kmod (future: in-kernel n-gram System 0)            │
│  ├─ N-gram lookup table (~200KB, loaded via firmware)         │
│  ├─ Per-event surprise: ~100ns                               │
│  └─ Alert to userspace when surprise > threshold             │
│                                                              │
└──────────────────────────────────────────────────────────────┘
                          │
                     ring buffer
                          │
                          ▼
USER SPACE
┌──────────────────────────────────────────────────────────────┐
│                                                              │
│  hackbot-traced (Rust daemon, 24/7)                          │
│  ├─ Event consumer (ftrace/eBPF ring buffer reader)          │
│  ├─ Semantic tokenizer (raw event → 8-field structured token)│
│  ├─ Dual-model scorer:                                       │
│  │   ├─ BASELINE model (frozen, known-good, human-approved)  │
│  │   └─ ADAPTIVE model (decaying counts, tracks current)     │
│  ├─ Multi-scale decay (short/medium/long-term memory)        │
│  ├─ Anomaly incident grouping + alerting                     │
│  ├─ Tiered storage:                                          │
│  │   ├─ Tier 1: RAM hot buffer (~1GB, last 10 min)           │
│  │   ├─ Tier 2: Disk warm storage (~50GB, last 24h)          │
│  │   ├─ Tier 3: Cold statistics (~1GB, indefinite)           │
│  │   └─ Tier 4: Anomaly archive (~10GB, labeled windows)     │
│  └─ Training scheduler:                                      │
│      ├─ Every minute: adaptive n-gram count update            │
│      ├─ Every hour: transformer fine-tune (100 steps)         │
│      ├─ Every day: full retrain + evaluate + report           │
│      └─ On demand: promote adaptive → baseline (human OK)    │
│                                                              │
│  hackbot-train (Python pipeline)                             │
│  ├─ train_ngram.py — factorial n-gram training               │
│  ├─ train_transformer.py — small custom transformer          │
│  ├─ evaluate_model.py — ROC/AUC, comparison reports          │
│  └─ export to HKBT binary format (INT8)                      │
│                                                              │
│  vLLM (System 2, existing)                                   │
│  └─ Deep analysis of flagged events via OODA loop            │
│                                                              │
└──────────────────────────────────────────────────────────────┘
```

### Cognitive Hierarchy

| Layer | Model | Latency | Catches |
|-------|-------|---------|---------|
| System 0 | Factorial n-gram (in-kernel) | ~100ns/event | Fork bombs, I/O storms, rate anomalies |
| System 1 | INT8 transformer (in-kernel or daemon) | ~5μs/event | State machine violations, unusual sequences |
| System 2 | LLM via vLLM (remote) | ~1s/query | Root cause analysis, natural language explanation |

Each layer triggers the one above when surprise exceeds its threshold.

---

## 2. Semantic Tokenizer: The Morphology of Kernel Language

Each raw kernel event is decomposed into 8 semantic fields — the "morphological features" of a kernel word:

| Field | Tokens | Examples |
|-------|--------|---------|
| **Category** (8) | SCHED, SYSCALL, BLOCK, NET, MEM, FS, IRQ, SIGNAL |
| **Action** (32) | READ, WRITE, OPEN, CLOSE, CONNECT, ACCEPT, MMAP, CLONE, EXECVE, FUTEX, EPOLL, SWITCH_IN, SWITCH_OUT, BLK_ISSUE, BLK_COMPLETE, ... |
| **Object Type** (16) | FD_REGULAR_FILE, FD_SOCKET_TCP, FD_SOCKET_UDP, FD_SOCKET_UNIX, FD_PIPE, FD_EPOLL, FD_TIMERFD, FD_DEVICE, FD_PROC, FD_NONE, ... |
| **Target Class** (16) | PATH_ETC, PATH_TMP, PATH_PROC, PATH_SYS, PATH_DEV, PATH_HOME, PATH_VAR_LOG, ADDR_LOCALHOST, ADDR_INTERNAL, ADDR_EXTERNAL, ... |
| **Size Class** (8) | SIZE_0, SIZE_TINY(1-64), SIZE_SMALL(65-512), SIZE_PAGE(513-4096), SIZE_LARGE(4097-64K), SIZE_HUGE(64K-1M), SIZE_ENORMOUS(>1M), SIZE_NA |
| **Result Class** (8) | RET_SUCCESS, RET_PARTIAL, RET_EAGAIN, RET_EPERM, RET_ENOENT, RET_EINTR, RET_OTHER_ERROR, RET_NA |
| **Duration Class** (8) | DUR_INSTANT(<1μs), DUR_FAST(1-100μs), DUR_NORMAL(100μs-1ms), DUR_SLOW(1-10ms), DUR_VERY_SLOW(10-100ms), DUR_BLOCKED(>100ms), DUR_HUNG(>1s), DUR_NA |
| **Gap Class** (8) | GAP_BURST(<1μs), GAP_RAPID(1-10μs), GAP_FAST(10-100μs), GAP_NORMAL(100μs-1ms), GAP_PAUSE(1-10ms), GAP_SLOW(10-100ms), GAP_IDLE(100ms-1s), GAP_DORMANT(>1s) |

**Total: ~104 unique sub-tokens across 8 fields.**

### Example tokenization

```
sys_read(fd=3, buf=..., count=4096) = 4096, took 500ns
→ [SYSCALL, READ, FD_REGULAR_FILE, TARGET_NONE, SIZE_PAGE, RET_SUCCESS, DUR_FAST, GAP_NORMAL]

sys_read(fd=0, buf=..., count=1) = 1, took 50ms
→ [SYSCALL, READ, FD_TERMINAL, TARGET_NONE, SIZE_TINY, RET_SUCCESS, DUR_VERY_SLOW, GAP_IDLE]

sys_connect(fd=5, {AF_INET, 185.x.x.x:443}) = 0, took 20ms
→ [SYSCALL, CONNECT, FD_SOCKET_TCP, ADDR_EXTERNAL, SIZE_NA, RET_SUCCESS, DUR_SLOW, GAP_NORMAL]

mmap(NULL, 4096, PROT_READ|PROT_WRITE|PROT_EXEC, MAP_ANON, -1, 0)
→ [SYSCALL, MMAP, FD_NONE, TARGET_NONE, SIZE_PAGE, RET_SUCCESS, DUR_FAST, GAP_RAPID]
  ← ⚠️ RWX anonymous mmap = possible shellcode injection!
```

**Bucket boundaries (SIZE_*, DUR_*, GAP_*) MUST be calibrated from empirical quantiles, not hardcoded.**

---

## 3. Perpetual Learning: The Dual-Model Architecture

### The Problem
Naive online learning lets attackers "normalize" malicious patterns over time (adversarial drift).

### The Solution: Two Models

| | Baseline Model | Adaptive Model |
|---|---|---|
| **Updates** | Only with human approval | Continuously (exponential decay) |
| **Represents** | "Known-good" behavior | "Current" behavior |
| **Role** | Constitutional reference | Reality tracker |

### Anomaly Classification Matrix

| Baseline Surprise | Adaptive Surprise | Interpretation | Action |
|---|---|---|---|
| LOW | LOW | **Normal** — both models agree | Learn |
| HIGH | LOW | **Drift** — system changed, baseline stale | Queue for baseline review |
| HIGH | HIGH | **True anomaly** — neither model expected this | HIGH priority alert |
| LOW | HIGH | **Regression** — returned to old pattern | Log |

### Multi-Scale Temporal Memory

Four n-gram tables with different decay rates:

| Scale | Decay α | Memory Horizon | Detects |
|-------|---------|----------------|---------|
| Short-term | 0.9999 | ~10K events (~0.1s) | Instantaneous bursts |
| Medium-term | 0.999999 | ~1M events (~10s) | Burst anomalies |
| Long-term | 0.99999999 | ~100M events (~17min) | Sustained anomalies |
| Baseline | No decay (daily retrain) | Indefinite | Constitution |

**Combined surprise = weighted sum across all timescales.** An event surprising at ALL timescales is genuinely anomalous.

### Gated Learning

```
if surprise(event) < learn_threshold:
    update_adaptive_model(event)          # Normal — learn from it
elif surprise(event) < alert_threshold:
    update_adaptive_model(event, w=0.1)   # Slightly unusual — learn slowly
else:
    archive_to_tier4(event)               # Anomalous — don't learn, alert
```

---

## 4. Tiered Storage

| Tier | Medium | Size | Retention | Contents |
|------|--------|------|-----------|----------|
| 0 | Kernel ring buffer | ~64MB | ~1 sec | Raw events for System 0 |
| 1 | RAM | ~1GB | ~10 min | Hot tokenized events for context |
| 2 | SSD (circular) | ~50GB | ~24 hours | Complete tokenized stream for retraining |
| 3 | Disk | ~1GB | Indefinite | Aggregated hourly/daily statistics |
| 4 | Disk | ~10GB | 30 days | Full event windows around anomalies (labeled) |

Data lifecycle: events flow Tier 0 → 1 → 2 → aggregated into 3 → deleted. Anomalous windows copied to Tier 4.

---

## 5. Implementation Milestones

> **Key Insight (2026-04-03):** hackbot-kmod ALREADY has tracepoint callbacks on
> every `sched_switch`, `sys_enter`, and `block_rq_complete`. The data is already
> flowing through the kernel module. N-gram learning is just adding array updates
> to the existing callback pipeline. **No userspace daemon needed for the core loop.**
>
> The revised plan builds IN-KERNEL FIRST, Python tools for analysis SECOND.

### Phase A: IN-KERNEL LEARNING (C, ~5-8 days)

**Milestone 0: In-Kernel Semantic Tokenizer** (~2-3 days)

Extend existing tracepoint callbacks in `hackbot_trace.c` to produce structured tokens.

| Task | Description |
|------|-------------|
| 0a | Create `hackbot_tokenizer.c` + `.h`: define `struct tokenized_event` (8 × u8 fields) |
| 0b | `tokenize_sched()`: sched_switch → SCHED + SWITCH_IN/OUT + gap_class from interval |
| 0c | `tokenize_syscall()`: sys_enter → action via `syscall_to_action[]` lookup, size_class from args |
| 0d | `tokenize_io()`: block_rq_complete → BLOCK + size_class + duration_class from latency |
| 0e | FD type classification: `fget(fd)` → `inode->i_mode` → FD_REGULAR_FILE / FD_SOCKET_TCP / ... |
| 0f | Gap class: per-CPU `prev_timestamp_ns`, compute delta, quantize into 8 buckets |
| 0g | Add `tokenize_*()` calls at end of each existing callback (after Tier 1/2/3) |
| 0h | Expose via `/proc/hackbot/tokens` for verification |
| 0i | Test: `insmod`, run workload, verify tokenization |

Key decisions:
- `fget()` for fd classification: ~50ns, safe in sys_enter (process context)
- `strncpy_from_user_nofault()` for path prefix (no fault risk)
- Per-CPU `prev_timestamp_ns` (avoid cache bouncing)
- Bucket boundaries initially hardcoded, calibrated from data later

**Milestone 1: In-Kernel N-gram Learning** (~2-3 days)

Q16.16 fixed-point factorial n-gram tables that learn from EVERY event — NO FPU needed.

| Task | Description |
|------|-------------|
| 1a | Create `hackbot_ngram.c` + `.h`: core data structures |
| 1b | `struct ngram_field_table`: `s32 count[32][32]` in Q16.16 (4KB per field) |
| 1c | `struct ngram_model`: 8 field tables + decay_alpha + total_events (32KB) |
| 1d | `struct ngram_state`: baseline (frozen) + 4 adaptive (multi-scale decay) = 160KB total |
| 1e | `ngram_process_event()`: per-field decay + increment + surprise in pure integer math |
| 1f | Concurrency: Hogwild-style (READ_ONCE/WRITE_ONCE, no locks, statistical tolerance) |
| 1g | Per-CPU `prev_event` via `DEFINE_PER_CPU()` |
| 1h | Wire into tracepoint callbacks (after tokenization) |
| 1i | `/proc/hackbot/ngram/surprise` — current per-field + total surprise |
| 1j | `/proc/hackbot/ngram/stats` — event count, alert count, top surprising bigrams |
| 1k | Test: `insmod`, observe surprise settling over 5 minutes of normal operation |

Memory: 160KB total. Cost: ~200-500ns per event (all integer arithmetic).

**Milestone 2: In-Kernel Anomaly Detection** (~1-2 days)

| Task | Description |
|------|-------------|
| 2a | Dual-model surprise: baseline_surprise (frozen) + adaptive_surprise (decaying) |
| 2b | Classification matrix: LOW/LOW=normal, HIGH/HIGH=anomaly, HIGH/LOW=drift |
| 2c | Alert ring buffer → wake waitqueue → `/proc/hackbot/ngram/alerts` |
| 2d | Gated learning: only update adaptive when surprise < learn_threshold |
| 2e | Integrate with patrol thread: report anomalies to vLLM on patrol tick |
| 2f | Checkpoint: periodically write n-gram tables to procfs binary (crash recovery) |
| 2g | Boot grace period: suppress alerts for 30s after module load |
| 2h | Test: `insmod`, wait 5 min, fork bomb → verify surprise spike + alert |

**At this point: hackbot-kmod is a SELF-LEARNING kernel anomaly detector.**
Observe → tokenize → learn → detect → alert — entirely at ring 0, ~500ns per event.

---

### Phase B: ANALYSIS TOOLS (Python, ~3-5 days)

**Milestone 3: Python Analysis Pipeline**

| Task | Description |
|------|-------------|
| 3a | `tools/ngram_export.py`: read `/proc/hackbot/ngram/` tables → Python |
| 3b | `tools/trace_vocabulary.py`: vocabulary defs matching kernel tokenizer |
| 3c | `tools/trace_collector.py`: read `/proc/hackbot/tokens` → JSONL |
| 3d | `tools/evaluate_ngram.py`: ROC/AUC, surprise timeseries plots |
| 3e | `tools/train_ngram.py`: train optimal baseline (batch, Kneser-Ney smoothing) |
| 3f | `tools/load_baseline.py`: write trained baseline → kernel module |
| 3g | Anomaly injection experiments: fork bomb, dd, stress-ng, memory pressure |
| 3h | Compare: event-type-only vs multi-field (proves arguments matter) |
| 3i | Compare accuracy by n-gram order (characterizes Markov depth) |
| 3j | Analysis report with plots |

**Success criterion: AUC > 0.9 for fork bomb and I/O storm.**

---

### Phase C: TRANSFORMER UPGRADE (PyTorch + C, ~3-5 days)

**Milestone 4: Custom Kernel-Event Transformer**

| Task | Description |
|------|-------------|
| 4a | Factored embeddings (sum of per-field), 4 layers, d=128, 4 heads, ~1M params |
| 4b | Multi-head output: predict each field independently |
| 4c | Train on collected kernel trace data (next-event prediction) |
| 4d | Compare AUC vs n-gram baseline per anomaly type |
| 4e | Export to HKBT binary format (INT8) via existing tools |
| 4f | Load via existing firmware loader, run on existing INT8 engine |
| 4g | Background training kthread: pull from ring buffer, micro-batch SGD at SCHED_IDLE |
| 4h | Training uses kernel_fpu_begin/end, runs off the hot path |

---

### Phase D: VISUALIZATION (TypeScript, ~2-3 days)

**Milestone 5: Surprise in the Game View**

| Task | Description |
|------|-------------|
| 5a | hackbot-server reads `/proc/hackbot/ngram/surprise` → WebSocket |
| 5b | Anomaly heatmap in Game View (rooms glow when surprise high) |
| 5c | Surprise timeline in Timeline panel |
| 5d | Anomaly log entries in Event Log |
| 5e | Complex plane: `z(t) = surprise(t) * exp(i * θ(t))`, θ = event category |

---

### Phase E: PAPER

**"Kernel as Language: In-Kernel Online Learning for Anomaly Detection via Next-Event Prediction"**

Structure:
1. Thesis: kernel event sequences have linguistic structure
2. **Novel contribution: in-kernel ONLINE LEARNING (not just inference — the kernel trains itself)**
3. Method: semantic tokenization + factorial n-gram + Q16.16 fixed-point + Hogwild concurrency
4. Results: anomaly detection via surprise (ROC/AUC), n-gram vs transformer comparison
5. System: dual-model perpetual learning with multi-scale decay
6. Comparison: LinnOS does in-kernel inference. hackbot does in-kernel learning + inference.
7. Connection: 5-layer autonomic OS architecture

---

## 6. File Structure

```
hackbot/
├── hackbot-kmod/                  # EXISTING — extend with:
│   ├── hackbot_tokenizer.c        # NEW: semantic event tokenizer (8 fields)
│   ├── hackbot_tokenizer.h        # NEW: tokenizer types
│   ├── hackbot_ngram.c            # NEW: Q16.16 n-gram learning + surprise
│   ├── hackbot_ngram.h            # NEW: n-gram types
│   ├── hackbot_trace.c            # MODIFY: add tokenize + ngram calls
│   ├── hackbot_patrol.c           # MODIFY: report anomalies via vLLM
│   └── ...                        # existing files unchanged
│
├── tools/
│   ├── trace_vocabulary.py        # Vocabulary defs matching kernel tokenizer
│   ├── ngram_export.py            # Export kernel n-gram tables → Python
│   ├── train_ngram.py             # Train optimal baseline (batch, Kneser-Ney)
│   ├── load_baseline.py           # Write baseline to kernel module
│   ├── evaluate_ngram.py          # ROC/AUC, surprise timeseries plots
│   ├── trace_collector.py         # Collect tokenized events → JSONL
│   ├── train_transformer.py       # Train custom transformer
│   └── ...                        # existing export tools unchanged
│
├── traces/
│   ├── normal/                    # Normal traces (from /proc/hackbot/tokens)
│   └── anomaly/                   # Anomaly traces (labeled by injection)
│
└── docs/
    └── KERNEL_AS_LANGUAGE_PLAN.md # This file
```

---

## 7. Edge Cases & Mitigations

| Edge Case | Mitigation |
|-----------|------------|
| **Event storm / buffer overflow** | eBPF circuit breaker → sampling mode + synthetic OVERFLOW token |
| **Clock skew across CPUs** | Per-CPU processing, merge with bounded reorder buffer |
| **Model file corruption during reload** | Atomic write (temp + rename), checksum verification |
| **Disk full** | Tier 2 circular buffer, Tier 3 compaction policy, Tier 4 retention policy |
| **System reboot** | Checkpoint n-gram every 5 min, boot grace period (suppress alerts for 5 min) |
| **Kernel upgrade** | Tokenizer uses syscall names not numbers, UNKNOWN_SYSCALL token for new syscalls |
| **Adversarial drift** | Dual-model architecture: baseline never updated without human approval |
| **False positive fatigue** | Adaptive thresholds, anomaly incident grouping (debounce) |
| **Daemon OOM-killed** | Cgroup limits, mlock() critical structures, OOMScoreAdjust=-900 |
| **Container/VM workloads** | Add container_id/cgroup as tokenizer fields |

---

## 8. Risks & Fallbacks

| Risk | Likelihood | Fallback |
|------|-----------|----------|
| N-gram doesn't detect anomalies | Low (for coarse) | Negative result still publishable; skip to transformer |
| Tracing overhead too high | Medium | Reduce to sched_switch only (<0.1% overhead) |
| Tokenizer misses key features | Medium | Start with event type + timing only, add features iteratively |
| Perpetual learning degrades model | Low | Disable online learning, retrain from batch data |
| **Scope creep** | **HIGH** | **Phase A alone (Milestones 0-2) is a complete paper** |

---

## 9. Connection to hackbot 5-Layer Architecture

| hackbot Layer | Kernel-as-Language Component |
|---------------|------------------------------|
| Layer 0: SENSORY | Tracepoint callbacks + semantic tokenizer |
| Layer 1: REFLEX | N-gram System 0 (~100ns) — replaces/augments LinnOS-style tiny NNs |
| Layer 2: REASONING | Transformer System 1 (~5μs) + vLLM System 2 (~1s) |
| Layer 3: SELF-IMPROVEMENT | Perpetual learning (dual-model, multi-scale, gated) |
| Layer 4: META-COGNITION | Vocabulary evolution, Markov depth analysis, model comparison |

The surprise score from n-gram/transformer is a natural signal for Phase 2's complex plane view:
`z(t) = surprise(t) * exp(i * theta(t))` where theta encodes the event category.
Anomalies create spirals outward on the complex plane.

---

## 10. First Step: Start Today

hackbot-kmod already registers callbacks on `sched_switch`, `sys_enter`, and `block_rq_complete`.
The data is ALREADY FLOWING. The first step is adding the tokenizer to these callbacks.

```
Step 1: Create hackbot_tokenizer.c (200-300 lines C)
        - struct tokenized_event { u8 fields[8]; }
        - tokenize_sched(), tokenize_syscall(), tokenize_io()
        - syscall_to_action[] lookup table

Step 2: Create hackbot_ngram.c (300-400 lines C)
        - struct ngram_state with baseline + 4 adaptive models
        - ngram_process_event(): Q16.16 decay + increment + surprise
        - All integer arithmetic, no FPU

Step 3: Wire into hackbot_trace.c callbacks (20-30 lines, modifications)
        - At end of each callback: tokenize → ngram_process_event

Step 4: Add /proc entries (100-150 lines C)
        - /proc/hackbot/ngram/surprise
        - /proc/hackbot/ngram/stats
        - /proc/hackbot/ngram/alerts

Step 5: make && sudo insmod hackbot.ko
        - The kernel starts learning its own language IMMEDIATELY
        - Watch surprise scores settle: cat /proc/hackbot/ngram/surprise
        - After 5 minutes of learning: run a fork bomb
        - Surprise should SPIKE — proof of concept
```

**~700 lines of C to make the kernel learn its own language. Entirely in ring 0.**
**No userspace daemon. No data pipeline. No serialization. ~500ns per event.**

The kernel doesn't just speak a language — it LEARNS to listen to itself.
