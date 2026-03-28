# LinnOS: Predictability on Unpredictable Flash Storage with a Light Neural Network

**Paper**: OSDI 2020, Mingzhe Hao et al., University of Chicago
**PDF**: `docs/refs/[20 OSDI] LinnOS- Predictability on Unpredictable Flash Storage with a Light Neural Network.pdf`

---

## Core Contribution

LinnOS is the **first operating system that runs a neural network inside the kernel** (Linux block layer) to make per-I/O latency predictions in real-time. It classifies every incoming I/O as "fast" or "slow" with 87-97% accuracy and only **4-6 microsecond inference overhead**, enabling intelligent admission control for flash storage arrays.

**One-sentence thesis**: A tiny, integer-quantized, 3-layer neural network running in the kernel block layer can learn device behavior from trace data and predict I/O latency better than any heuristic, enabling the OS to make real-time scheduling decisions at per-I/O granularity.

---

## Key Technical Ideas

### 1. Binary Classification over Regression
Instead of predicting exact latency (hard), LinnOS converts the problem to binary: "will this I/O be fast or slow?" This works because SSD latencies follow a **Pareto distribution** — 90%+ are stable, but a long tail causes unpredictability. Users only need to avoid the tail, not predict exact values.

### 2. The Inflection Point Algorithm
LinnOS automatically finds the optimal fast/slow threshold for each (workload, device) pair by:
1. Collecting a busy-hour trace via `blktrace`
2. Simulating LinnOS admission control at different threshold percentiles
3. Picking the threshold that maximizes the "boost area" (gap between original and optimized latency CDFs)

This is per-device, per-workload — no manual tuning. Typical inflection points range from p72 to p98.

### 3. Surprisingly Simple Features
After extensive feature engineering, only **two types of input** matter:
- **(a)** Latencies of the **last 4 completed I/Os** (encoded as 4 decimal digits each = 16 neurons)
- **(b)** Number of **pending I/Os** when each of those 4 completed (3 digits each = 12 neurons)
- Plus 3 neurons for current queue length

**Total: 31 input features.** Block offsets, read/write flags, and long I/O history do NOT help.

**Why**: Internal SSD contention (GC, buffer flush, channel contention) manifests in recent completion latencies and queue depths. The NN learns to detect these internal states from external observations.

### 4. Ultra-Light Architecture
```
Input Layer:  31 neurons (decimal-digit encoded features)
Hidden Layer: 256 neurons (ReLU activation)
Output Layer: 2 neurons (fast/slow, argmax → binary decision)
```
- **Integer-only quantized weights** — no floating point in kernel
- 3 decimal digit precision (weights converted from float to integer)
- 8,706 weights total, 68 KB memory footprint
- **4-6 microsecond inference** on CPU (no GPU needed)
- Optional: 2-threaded matmul on extra CPU core → 36% speedup to ~4μs

### 5. Training Pipeline (LinnApp)
Userspace companion tool handles the ML lifecycle:
1. **Trace collection**: `blktrace` during busy hours (300 MB/hour, 0.5% CPU)
2. **Labeling**: Inflection point algorithm → fast/slow labels (automatic, no human input)
3. **Training**: TensorFlow, with biased loss function (penalizes false submits more)
4. **Upload**: Quantized integer weights pushed to kernel module
5. **Recalibration**: Every few hours, checks if inflection point shifted >5 percentiles → retrain

### 6. Admission Control Pattern
When a storage application issues a latency-critical I/O:
1. LinnOS **infers** fast/slow via the NN (4-6μs)
2. If **fast**: submit to device normally
3. If **slow**: **revoke** the I/O (don't send to device), return "slow" error to application
4. Application **fails over** to another replica (RAID/cluster)
5. Combined with high-percentile hedging for residual inaccuracy → **LinnOS+HL**

---

## Evaluation Results

| Metric | Value |
|--------|-------|
| Accuracy | 87-97% (varies by device/workload) |
| Inference overhead | 4-6 μs per I/O |
| CPU overhead | 0.3-0.7% per device |
| Average latency reduction | 9.6-79.6% vs hedging methods |
| Tail latency (p99.99) | Stable and predictable |
| Implementation | 2,170 LOC in Linux block layer (C) |
| Model memory | 68 KB (integer weights) |
| Training data | Millions of I/Os from production traces (Microsoft Azure, Bing, Cosmos) |
| Devices tested | 10 SSD models (consumer + enterprise) |

**Key result**: LinnOS+HL consistently outperforms ALL other methods (hedging, heuristics, cloning) across ALL workloads and platforms. Even at p99.99, latencies remain stable.

---

## Relevance to hackbot

### Direct Parallels

| LinnOS | hackbot |
|--------|---------|
| 3-layer NN in block layer | SmolLM2-135M in kernel module |
| Integer-quantized weights | INT8/Q16.16 and FP16 weights |
| blktrace as training data | kprobe/tracepoint as observation data |
| Per-I/O binary inference | Per-event anomaly classification |
| LinnApp (userspace trainer) | tools/export_hackbot.py (model exporter) |
| 4-6μs inference | ~10ms per token (1000x slower, but different task) |
| Admission control (submit/revoke) | Tier 2 actions (requires approval) |

### Ideas to Adapt

1. **System 1 as Binary Classifier**: Instead of generating text, hackbot's System 1 could do LinnOS-style binary classification: "is this pattern anomalous?" Fast (4-6μs), no text generation, no FP16 precision issues.

2. **Trace Subsystem as Senses**: LinnOS uses blktrace for storage. hackbot could use ftrace, tracepoints, kprobes, and perf counters as "five senses" — each providing a different view of kernel behavior.

3. **Per-Subsystem Specialized Models**: Instead of one LLM for everything, train tiny specialized NNs for specific subsystems (storage latency, scheduler behavior, network patterns), with the LLM as coordinator.

4. **Data Lake from Kernel Traces**: The kernel generates massive amounts of structured trace data. This is a natural training dataset for specialized models — exactly as LinnOS uses blktrace data.

5. **Biased Training for Safety**: LinnOS's insight about asymmetric error costs (false submits > false revokes) maps to hackbot's safety model: false negatives (missed anomalies) are worse than false positives (false alerts).

6. **The 4-6μs Performance Bar**: For per-event decisions in the kernel, inference must be <10μs. The LLM (~10ms) is 1000x too slow for this. A hybrid architecture (tiny NNs for reflexes + LLM for reasoning) bridges the gap.

---

## Extra Insights

### The Biological Nervous System Analogy (extended by LinnOS)

```
SENSORY LAYER (trace subsystem — observation)
├── ftrace callbacks → function call streams
├── tracepoint callbacks → structured kernel events
├── kprobes → specific function monitoring
├── perf counters → hardware performance metrics
└── blktrace → storage I/O patterns

REFLEX LAYER (LinnOS-style tiny NNs — instant, <10μs)
├── Storage: latency prediction (LinnOS architecture, proven)
├── Scheduler: anomaly detection
├── Network: traffic classification
├── Memory: pressure prediction
└── Each trained on subsystem-specific trace data

REASONING LAYER (hackbot LLM — deep analysis)
├── System 1 (local): quick triage, anomaly flagging
└── System 2 (vLLM): complex investigation, tool-using agent
    └── Interprets reflex layer alerts
    └── Requests more data via tools
    └── Can orchestrate retraining of reflex models

ACTION LAYER (tiered capabilities, formal verification)
├── Tier 0: observation only (current hackbot)
├── Tier 1: instrumentation (kprobes, tracepoints)
├── Tier 2: indirect actions (requires human approval)
└── Tier 3: kernel modification (requires Verus proof)
```

### The "New Generation of OS" Possibility

The user noted: "만약 이게 된다?? 그럼 os의 새로운 generation 아닐까?!" (If this works, wouldn't it be a new generation of OS?!)

LinnOS proves the PRINCIPLE: ML in the kernel can outperform hand-tuned heuristics. If generalized from storage to all subsystems (scheduler, memory manager, network stack, security), this is indeed a new OS paradigm — an **autonomic operating system** that learns and adapts its own behavior from runtime observations, with formal verification ensuring safety.

hackbot is building toward this vision: the LLM provides the reasoning capability that LinnOS lacks (LinnOS can only classify, not investigate), while LinnOS-style tiny NNs provide the microsecond-scale reflexes that the LLM is too slow for.

---

## Key Quotes

> "We show that it is plausible to incorporate machine learning inside operating systems for real-time decision-making." (Abstract)

> "The key to our approach is learning. Can we learn the behavior of the underlying device in a black-box way and use the results of the learning to increase predictability?" (Section 1)

> "We surprisingly found that important-looking features such as block offsets, read/write flags, or long history of writes do not significantly improve accuracy." (Section 4.3)

> "LinnOS+HL consistently outperforms all other methods across different workloads and platforms." (Section 5.3)

> "Can advanced accelerators help accelerate OS kernel operations? Can near-storage/data processing help?" (Section 6 — future work, directly relevant to hackbot)
