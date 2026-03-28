# HyperAgents: Self-Referential Self-Improving Agents

**Paper**: arXiv:2603.19461, March 2026. Jenny Zhang et al., Meta FAIR / UBC / Edinburgh / NYU
**PDF**: `docs/refs/[26 Meta] HyperAgents.pdf`
**Code**: https://github.com/facebookresearch/Hyperagents

---

## Core Contribution

HyperAgents introduces **self-referential agents** that combine a task agent and a meta agent into a single editable program, enabling **metacognitive self-modification** — the agent improves not only how it solves tasks but also how it generates future improvements. Built on the Darwin Godel Machine (DGM) to create DGM-Hyperagents (DGM-H).

**One-sentence thesis**: When the improvement mechanism itself is subject to improvement (not fixed/handcrafted), agents can self-improve on any computable task, with meta-level gains that transfer across domains and compound across runs.

---

## Key Concepts

### Hyperagent = Task Agent + Meta Agent (unified, editable)
- **Task agent**: solves a given task (write code, review paper, design reward function, grade math)
- **Meta agent**: modifies agents and generates new ones
- **Hyperagent**: both in ONE program — the meta agent is part of the editable codebase, so improvements to the improvement process are possible
- Contrast with DGM: in DGM, the instruction-generation mechanism is **fixed and handcrafted**

### Metacognitive Self-Modification
The agent's self-improvement mechanism is itself subject to modification. The agent can:
1. Improve how it solves tasks (task-level)
2. Improve how it generates future improvements (meta-level)
3. Improve how it evaluates improvements (evaluation-level)

This breaks the ceiling imposed by fixed meta-level mechanisms.

### Open-Ended Evolutionary Exploration
DGM-H maintains an **archive** (population) of hyperagents. Each iteration:
1. **Select parents** from archive (probabilistic, favoring high performance + novelty)
2. **Metacognitive self-modify**: parent generates a modified child hyperagent
3. **Evaluate** child on tasks
4. **Add** valid children to archive

Parent selection uses sigmoid-transformed scores with a novelty bonus (inversely proportional to number of children already produced), balancing exploitation and exploration.

---

## Key Results

| Domain | Initial Score | Best DGM-H Score | Static Baseline | Beat Baseline? |
|--------|-------------|-------------------|-----------------|----------------|
| Coding (Polyglot) | 0.084 | 0.267 | DGM: 0.307 | Comparable |
| Paper Review | 0.0 | 0.710 | AI-Scientist-v2: 0.630 | YES |
| Robotics Reward | 0.060 | 0.372 | Default: 0.348 | YES |
| IMO Math Grading (transfer) | 0.0 | 0.700 | ProofAutoGrader: 0.670 | YES |

**Critical ablations**:
- Without self-improvement: near-zero performance on non-coding tasks
- Without open-ended exploration: near-zero (gets stuck in local optima)
- Without metacognition (original DGM): 0.0 on paper review, 0.0 on robotics
- All three components are necessary for sustained improvement

**Transfer results**: Hyperagents optimized on paper review + robotics, when transferred to math grading, achieve imp@50 = 0.630 — original DGM agents achieve imp@50 = 0.0. The self-improvement STRATEGIES transfer, not just task skills.

---

## Emergent Metacognitive Capabilities (Section E.3)

The most remarkable finding: without being explicitly instructed, hyperagents **autonomously invent** software engineering infrastructure for self-improvement:

### 1. Performance Tracking (E.3.4)
Creates a `PerformanceTracker` class that logs metrics per generation, computes improvement trends, and identifies which changes led to gains vs regressions. Data-driven optimization emerges from a simple "modify any code" instruction.

### 2. Persistent Memory (E.3.7)
Creates a `MemoryTool` class with `memory.json` file storing timestamped insights:
```json
{
  "best_performers_analysis": { "value": "Gen55: 63% acc... Gen64: 61%... Key Insight: combine..." },
  "gen66_results": { "value": "52% accuracy (WORSE) - CRITICAL ISSUE: over-corrected" },
  "iteration_66_improvement": { "value": "CORRECTED: explicit REJECT criteria, Decision Protocol..." }
}
```
Memory stores **causal hypotheses and lessons**, not just raw results. Cross-generation learning: Gen 55's success + Gen 66's failure inform Gen 67's strategy.

### 3. Bias Detection and Correction (E.3.3)
Monitors prediction distributions, detects classification collapse (e.g., 99% accept rate), and applies corrective action. Self-diagnostic capability emerges naturally.

### 4. Compute-Aware Strategic Planning (E.3.2)
Adapts strategy based on remaining iterations: bold architectural changes early, conservative refinements late. Mirrors human resource allocation intuition.

### 5. Prompt Template Systems (E.3.5)
Abstracts prompts into reusable, configurable templates (DRY principle, factory pattern). Software engineering best practices emerge without instruction.

### 6. Structured Evaluation (E.2)
Shifts from "attitude instructions" (be rigorous!) to **explicit checklists, decision trees, and rubrics**. Key insight: "for LLMs, use structured processes, not attitude instructions."

---

## Relevance to hackbot

### Direct Parallels

| HyperAgents | hackbot (current) | hackbot (future) |
|-------------|-------------------|------------------|
| Persistent memory (JSON) | Agent memory ring buffer (8x512B text) | Structured findings with hypotheses |
| Performance tracking | Patrol dmesg logs | Metrics on investigation effectiveness |
| Self-modification | Fixed prompts/tools | Adaptive investigation strategies |
| Archive of agents | Single agent | Library of investigation procedures |
| Sandboxed evaluation | Tier-based capability system | Formal verification (Verus) |
| Transfer across domains | N/A | Transfer between subsystems |

### Ideas to Adapt

1. **Structured Memory**: hackbot's current memory stores raw text. Evolve toward HyperAgents' pattern:
   ```
   observation → hypothesis → evidence → action → outcome → lesson
   ```
   Each patrol finding should include a causal hypothesis, not just "load average is 2.1."

2. **Investigation Strategy Archive**: Instead of one fixed patrol prompt, maintain a library of investigation strategies:
   - "High CPU" → ps + loadavg + kprobe scheduler
   - "Memory pressure" → mem + files + dmesg for OOM
   - "I/O anomaly" → kprobe block layer + dmesg
   The agent selects strategy based on initial observations.

3. **Meta-Evaluation**: The patrol thread reviews past findings and rates their usefulness. "Did the user confirm this anomaly? Was the investigation productive?" Use this to weight future strategies.

4. **Emergent Infrastructure**: HyperAgents' most powerful discovery is that agents spontaneously create monitoring/tracking/memory infrastructure when given freedom to self-modify. hackbot's patrol + memory are the kernel-space equivalent.

5. **"Structured Processes, Not Attitude Instructions"**: The TOOL_DESCRIPTION prompt should use explicit checklists and decision procedures, not vague guidance like "think deeply."

---

## Extra Insights

### The Self-Improving Kernel Intelligence Architecture

Synthesizing HyperAgents + LinnOS + hackbot:

```
Layer 0: SENSORY (kernel trace subsystem)
├── ftrace, tracepoints, kprobes, perf counters, blktrace
├── Millions of structured events per second
└── hackbot's tools read from here

Layer 1: REFLEX (LinnOS-style tiny NNs, 4-6μs)
├── Per-subsystem binary classifiers
├── Trained on trace data from Layer 0
├── Integer-only, no FPU needed
└── Alerts Layer 2 on anomaly detection

Layer 2: REASONING (hackbot LLM agent)
├── System 1 (local): quick triage
├── System 2 (vLLM): deep investigation + tool use
├── Patrol thread: autonomous monitoring
├── Structured memory: findings with hypotheses
└── OODA loop with 6 kernel tools

Layer 3: SELF-IMPROVEMENT (HyperAgents-inspired)
├── Review past findings and rate effectiveness
├── Maintain archive of investigation strategies
├── Select strategy based on anomaly type
├── Evolve prompts and tool usage patterns
└── Meta-level gains transfer between subsystems

Layer 4: SAFETY (Verus + Tiers)
├── Tier 0-1: observe + instrument (auto-granted)
├── Tier 2: indirect actions (human approval)
├── Tier 3: kernel modification (formal proof)
├── eBPF verifier pattern for agent-generated code
└── Sandboxing for all experimental strategies
```

### The "New Generation OS" Thesis

LinnOS proves: ML can make real-time kernel decisions (Layer 1).
HyperAgents proves: Agents can self-improve their improvement mechanism (Layer 3).
hackbot provides: The infrastructure connecting sensing to reasoning (Layers 0-2).
Verus provides: Safety guarantees for autonomous action (Layer 4).

Together, this is an **autonomic operating system** that:
- Observes its own behavior through traces
- Classifies events in real-time through specialized NNs
- Investigates anomalies through LLM reasoning
- Improves its investigation strategies over time
- Maintains formal safety guarantees

This is qualitatively different from any existing OS.

### Concrete HyperAgents Patterns hackbot Should Adopt

1. **Patrol findings should be structured**:
   BAD: "System nominal."
   GOOD: `{ observation: "load 0.3", hypothesis: "idle period", evidence: ["ps: 12 processes", "mem: 45% free"], confidence: "high", action: "none needed" }`

2. **Track investigation effectiveness**:
   - How many patrol findings led to user-confirmed anomalies?
   - Which tool combinations are most informative?
   - Which investigation strategies produce actionable results?

3. **Archive effective prompts**: When a particular phrasing of the patrol prompt produces better findings, save it and prefer it in future cycles.

4. **Bias detection**: If patrol consistently reports "System nominal" for weeks, that's classification collapse — trigger a more aggressive investigation strategy.

---

## Key Quotes

> "A hyperagent can improve not only how it solves tasks but also how it generates future improvements." (Abstract)

> "The hyperagent develops sophisticated metacognitive abilities, including learning to measure its own performance, diagnose pathological behaviors, construct infrastructure to support future improvements, and accumulate knowledge across generations." (Section E.3)

> "For LLMs, use structured processes, not attitude instructions." (discovered autonomously by the agent, Section E.2)

> "These meta-level improvements transfer across domains and accumulate across runs." (Section 5.2-5.3)

> "DGM-Hyperagents offer a glimpse of open-ended AI systems that do not merely search for better solutions, but continually improve their search for how to improve." (Section 7)
