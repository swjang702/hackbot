# Flow Report: hackbot Architectural and Conceptual Flow Analysis

**Date**: 2026-03-02
**Analyst**: Claude Opus 4.6
**Source Documents**: Investigation Report, Research Statement PDF, "Connecting the dots" blog post PDF, handwritten architecture diagram (mynote.jpg), refs.md
**Focus**: Mapping research concepts to executable system architecture, with visualization as MVP entry point

---

## Table of Contents

1. [Conceptual Framework Overview](#1-conceptual-framework-overview)
2. [Flow Path 1: Data Collection (Sensory Input)](#2-flow-path-1-data-collection-sensory-input)
3. [Flow Path 2: Visualization Pipeline (MVP Core)](#3-flow-path-2-visualization-pipeline-mvp-core)
4. [Flow Path 3: Agent Decision Loop (Brain)](#4-flow-path-3-agent-decision-loop-brain)
5. [Flow Path 4: Complex Plane Signal Analysis](#5-flow-path-4-complex-plane-signal-analysis)
6. [Flow Path 5: Action Execution](#6-flow-path-5-action-execution)
7. [Flow Path 6: Memory and Learning](#7-flow-path-6-memory-and-learning)
8. [Flow Path 7: Research Cycle (Adventure -> Quantifying)](#8-flow-path-7-research-cycle)
9. [MVP Architecture: Visualization-First Approach](#9-mvp-architecture-visualization-first-approach)
10. [Component Interconnection Map](#10-component-interconnection-map)
11. [Key Design Decisions and Trade-offs](#11-key-design-decisions-and-trade-offs)
12. [File and Concept Cross-Reference](#12-file-and-concept-cross-reference)

---

## 1. Conceptual Framework Overview

The hackbot project sits at the intersection of three domains, each documented across the source materials:

```
                    EMPIRICAL TRACING
                   (current professional work)
                          |
                          |
    FORMAL VERIFICATION --+-- AUTONOMOUS AGENTS
    (PhD aspiration)           (hackbot vision)
```

**The core insight** from the handwritten diagram (`/home/sunwoo/projects/hackbot/docs/mynote.jpg`) and the Research Statement (`/home/sunwoo/projects/hackbot/docs/Research_Statement.pdf`) is that tracing is not merely a debugging tool -- it is an **empirical verification instrument**. The hackbot is the system that operationalizes this insight: an autonomous agent that uses tracing as its senses, navigates opaque system layers, and bridges the gap between runtime observation and formal specification.

### Three Aligned Frameworks

The documents describe three parallel four-part frameworks that map onto each other:

| Research Statement Pillars | Auto-Hunting Components | Research Cycle |
|---------------------------|------------------------|----------------|
| 0.1 Autonomous bot in kernel | Sensory Input + Brain + Action | Adventure (Exploration) |
| 0.2 Visualization | (Cross-cutting: renders all components) | (Cross-cutting: makes all stages visible) |
| 0.3 Complex Plane | Memory & Learning (pattern analysis) | Modeling (formalize patterns) |
| 0.4 Mathematical Formulation | (Emergent from learning) | Modding + Quantifying |

### The Handwritten Diagram Flow

From `/home/sunwoo/projects/hackbot/docs/mynote.jpg`, the architecture flows as:

```
mathematical model ??  ------>  verifiable specification
       ^                              ^
       |                              |
       |    +-- active Mapping ---> Specification
       |    |
Node (comp/system) --> Monitor --> Trace --+
       |                                    |
       |    Infer / Feedback / Learning <---+
       |
       +-- System Abstraction? / Overview?
       |
       +-- For APP Vulnerability??
       |
       +-- abnormal exe curation / path...??
```

This diagram captures the tension between bottom-up empirical observation (left side: node, monitor, trace) and top-down formal goals (top: mathematical model, verifiable specification), with the agent (center: infer/feedback/learning) mediating between them.

---

## 2. Flow Path 1: Data Collection (Sensory Input)

**Source references**:
- Research Statement section 0.1 ("Autonomous bot in the kernel")
- Blog post: Sensory Input component table
- Blog post: "eBPF tracing is the agent's eyes and ears"
- Investigation Report section 2.1 (Design Pillars)

### Data Flow

```
+---------------------------+
| TARGET SYSTEM             |
| (Kernel / GPU / NPU /     |
|  LLM Serving Runtime)    |
+---------------------------+
         |
         | [kernel tracepoints, kprobes, uprobes, perf events]
         v
+---------------------------+
| eBPF PROBES               |
| - Power consumption       |
| - System calls (entry/exit)|
| - Scheduling events       |
| - Memory access patterns  |
| - Network events          |
| - File descriptor ops     |
| - GPU/NPU work submission |
+---------------------------+
         |
         | [ring buffer / perf buffer]
         v
+---------------------------+
| USERSPACE EVENT STREAM    |
| (structured trace events) |
|                           |
| Schema per event:         |
| - timestamp (ns)          |
| - event_type              |
| - pid/tid                 |
| - cpu_id                  |
| - payload (type-specific) |
| - stack_trace (optional)  |
+---------------------------+
         |
         +--------+--------+--------+
         |        |        |        |
         v        v        v        v
    Viz Engine  Analysis  Storage  Agent
    (Flow 2)   (Flow 4)  (Flow 6) (Flow 3)
```

### Critical Interface: Event Stream Schema

The event stream is the foundational data contract. Every downstream component -- visualization, analysis, agent reasoning, storage -- consumes from this stream. The schema must be:
- **Extensible**: New probe types added without breaking consumers
- **Timestamped**: Nanosecond precision for signal analysis
- **Typed**: Each event type carries a known payload structure
- **Contextual**: PID, TID, CPU, cgroup for scoping

### MVP Scope for Data Collection

For the MVP, the concrete entry point (from the blog post discussion) is **LLM workload profiling**:
- Power traces during LLM inference (prefill vs. decode phases)
- System calls from the LLM runtime process
- GPU/NPU scheduling events
- Memory bandwidth usage

This is work Sunwoo already performs professionally, so the data collection layer is the most mature component. The MVP challenge is not collecting data but **rendering it meaningfully**.

---

## 3. Flow Path 2: Visualization Pipeline (MVP Core)

**Source references**:
- Research Statement section 0.2 ("Visualization")
- `/home/sunwoo/projects/hackbot/docs/refs.md` (three visualization references)
- Blog post: rs-sdk discussion, Dockercraft reference
- Research Statement: "All actions that the bot is doing are shown in real time as if it is the character in video games"

### The Visualization Vision

The Research Statement (page 2) states explicitly:

> "All actions that the bot is doing are shown in real time as if it is the character in video games. [Motivation: deepmind visualization, Canon Variations visualization was deeply impressive, rs-sdk game (gamification of agent actions?)]"

This is not a conventional monitoring dashboard. It is a **game-like rendering** where:
- The kernel is a navigable world
- The agent is a character moving through that world
- System events are visible phenomena in the world
- Anomalies are dramatic visual events

### Visualization Architecture

```
+---------------------------+
| EVENT STREAM (from Flow 1)|
+---------------------------+
         |
         v
+---------------------------+
| EVENT ROUTER / GATEWAY    |
| - Filters by relevance    |
| - Batches for frame rate  |
| - Adds spatial mapping    |
|   (event -> world coords) |
+---------------------------+
         |
         v
+---------------------------+        +---------------------------+
| WORLD STATE MODEL         |        | AGENT STATE               |
| - Process map (rooms/zones)|       | - Current position        |
| - Syscall objects          |        | - Current action          |
| - Memory regions           |        | - Decision history        |
| - File descriptors         |        | - "Level" / capability    |
| - Network connections      |        +---------------------------+
+---------------------------+                   |
         |                                      |
         +------------------+-------------------+
                            |
                            v
+--------------------------------------------------+
| RENDERING ENGINE (Web-based)                      |
|                                                   |
| +------------------+  +------------------------+ |
| | GAME VIEW        |  | SIGNAL VIEW            | |
| | (2D isometric)   |  | (Complex Plane)        | |
| |                  |  |                        | |
| | Agent character  |  | Waveforms              | |
| | Process rooms    |  | Phase diagrams         | |
| | Syscall objects  |  | Anomaly highlights     | |
| | Event particles  |  | Frequency spectrum     | |
| +------------------+  +------------------------+ |
|                                                   |
| +------------------+  +------------------------+ |
| | CONTROL PANEL    |  | TIMELINE               | |
| | Agent commands   |  | Event history          | |
| | Probe config     |  | Replay controls        | |
| | Filter controls  |  | Bookmark anomalies     | |
| +------------------+  +------------------------+ |
+--------------------------------------------------+
```

### Reference Architecture Mapping

Each visualization reference from `/home/sunwoo/projects/hackbot/docs/refs.md` maps to a specific aspect:

| Reference | URL | Maps To | Architectural Lesson |
|-----------|-----|---------|---------------------|
| **rs-sdk** | `https://github.com/MaxBittker/rs-sdk` | Game View + Agent rendering | Web client, gateway server, bot SDK pattern. Agent actions visible in game UI. TypeScript. |
| **Assembly** | `https://assembly.louve.systems/` | Multi-agent/process interaction model | Programs competing in shared memory space -- like processes in kernel competing for resources. |
| **Dockercraft** | `https://github.com/docker-archive-public/docker.dockercraft` | Infrastructure-as-game-world concept | Go daemon bridges system API to game engine. Proof that system abstractions can be game objects. |

### rs-sdk as Primary Template

The rs-sdk architecture provides the most direct structural parallel:

```
rs-sdk                          hackbot equivalent
------                          ------------------
Game Engine (2004scape)    -->  Target System (kernel + LLM runtime)
Web Client (browser)       -->  Visualization Frontend (browser)
Gateway Server             -->  Event Router / Gateway
Bot SDK (TypeScript)       -->  Agent Decision Interface
Bot scripts / LLM brain    -->  LLM Brain (hackbot agent logic)
```

The key architectural insight from rs-sdk is the **separation between the world being observed and the visualization of that world**. In rs-sdk, the game engine runs independently; the web client renders a view; the bot interacts through a defined SDK. In hackbot:
- _The kernel runs independently (it is the "game engine")_
- _The visualization renders a view of kernel state (derived from eBPF data)_
- _The agent interacts through system call and eBPF interfaces_

### MVP Visualization: What to Build First

Based on the documents, the simplest viable visualization has these layers (ordered by implementation priority):

**Layer 1 - Trace Replay Viewer (Static)**:
- Load a pre-recorded eBPF trace file
- Render system calls as a timeline
- Show process hierarchy as spatial layout
- No agent, no real-time -- just make traces visible

**Layer 2 - Real-time Event Stream**:
- Connect to live eBPF data source
- Update visualization in real-time
- Add basic filtering (by PID, syscall type, etc.)

**Layer 3 - Agent Overlay**:
- Add the "bot character" to the visualization
- Show agent's current focus / position in system space
- Render agent decisions as visible actions

**Layer 4 - Complex Plane View**:
- Add secondary panel showing trace-to-signal mapping
- Normal patterns as stable orbits
- Anomalies as phase shifts (see Flow Path 4)

---

## 4. Flow Path 3: Agent Decision Loop (Brain)

**Source references**:
- Blog post: Brain component ("LLM decides next actions based on observations")
- Blog post: "Auto-hunting" system concept
- Research Statement section 0.1 ("Autonomous bot in the kernel")
- Handwritten diagram: "Infer / Feedback / Learning" loop

### Decision Loop Architecture

```
                    +---> OBSERVE (Sensory Input)
                    |         |
                    |         | [filtered trace events]
                    |         v
                    |    ORIENT (Context Building)
                    |         |
                    |         | [current system state model]
                    |         | [historical patterns from Memory]
                    |         | [current exploration goals]
                    |         v
                    |    DECIDE (LLM Reasoning)
                    |         |
                    |         | [next action selection]
                    |         | [reasoning trace for viz]
                    |         v
                    |    ACT (Action Execution)
                    |         |
                    |         | [system call / BPF load / signal]
                    |         v
                    +----FEEDBACK (Result observation)
```

This is an OODA loop (Observe-Orient-Decide-Act) where the LLM serves as the "Orient" and "Decide" phases. The blog post gives a concrete example:

> "For instance: the current process called 'open', system call returned 'permission denied'. Should I try attaching to it with 'ptrace' next, or flag it with another approach?"

### Agent Autonomy Spectrum (MVP Progression)

The blog post discusses varying levels of autonomy. For the MVP, a progression:

| Level | Description | Visualization Role |
|-------|-------------|-------------------|
| **L0: Replay** | No agent. Human watches trace replay. | Viz shows raw trace data |
| **L1: Suggested** | Agent suggests what to look at, human decides | Viz highlights agent suggestions |
| **L2: Semi-auto** | Agent acts, human can override/redirect | Viz shows agent actions, human has controls |
| **L3: Autonomous** | Agent acts independently, human monitors | Viz is the monitoring interface |
| **L4: Auto-hunting** | Agent runs 24/7, generates reports | Viz is the report viewer + occasional live view |

The blog post's "auto-hunting" vision is L4, but the MVP should start at L0 or L1 and make the visualization compelling at those levels before adding agent autonomy.

### LLM Integration Points

The agent's "brain" needs specific interfaces:

```
LLM INPUT:
  - Current system state summary (from trace events)
  - Recent event history (windowed)
  - Known patterns (from memory store)
  - Current exploration objective
  - Available actions list

LLM OUTPUT:
  - Selected action (with parameters)
  - Reasoning explanation (for visualization)
  - Confidence level
  - Alternative actions considered
```

---

## 5. Flow Path 4: Complex Plane Signal Analysis

**Source references**:
- Research Statement section 0.3 ("Complex Plane")
- Blog post: "Tracing as Signal Processing" and "Anomaly as Phase Shift"
- Handwritten diagram: "mathematical model??" -> "verifiable specification"
- Blog post (Gemini section): Euler's formula reinterpretation

### The Complex Plane Concept

From the Research Statement: "The trace of the bot maps to a complex plane."

From the blog post (Gemini's analysis):
- **Tracing as Signal Processing**: System event flows become complex-number signals. Normal execution has characteristic frequency and phase.
- **Anomaly as Phase Shift**: When information leakage or exploitation occurs, the signal's phase shifts.
- **Goal**: Mathematically defined "normal orbital" that anomalies visibly deviate from.

### Signal Processing Pipeline

```
+---------------------------+
| RAW TRACE EVENTS          |
| (timestamped stream)      |
+---------------------------+
         |
         v
+---------------------------+
| WINDOWING                 |
| - Fixed time windows      |
| - Sliding windows         |
| - Per-process windows     |
+---------------------------+
         |
         v
+---------------------------+
| FEATURE EXTRACTION        |
| - Syscall frequency       |
| - Power consumption rate  |
| - Memory access patterns  |
| - Inter-event timing      |
| - Call sequence n-grams   |
+---------------------------+
         |
         v
+---------------------------+
| COMPLEX MAPPING           |
| z(t) = r(t) * e^(i*theta(t)) |
|                           |
| Where:                    |
| - r(t) = amplitude        |
|   (event intensity/rate)  |
| - theta(t) = phase        |
|   (behavioral state)      |
|                           |
| Normal execution:         |
|   stable orbit in z-plane |
| Anomaly:                  |
|   phase shift / orbit     |
|   deviation               |
+---------------------------+
         |
         v
+---------------------------+
| ANOMALY DETECTION         |
| - Phase shift detection   |
| - Orbit deviation measure |
| - Frequency anomaly       |
| - Statistical divergence  |
+---------------------------+
         |
         +--------+---------+
         |        |         |
         v        v         v
    Viz Signal  Agent     Memory
    Panel      Alert     Store
    (Flow 2)   (Flow 3)  (Flow 6)
```

### Visualization of Complex Plane

This maps directly to the "Signal View" panel in the visualization:
- **Orbit plot**: z(t) traced on the complex plane, showing the agent's/system's behavioral trajectory
- **Phase diagram**: theta(t) over time, highlighting sudden shifts
- **Frequency spectrum**: FFT of the signal showing dominant patterns
- **Anomaly markers**: Visual highlights when deviation exceeds threshold

### Connection to the Research Goal

The complex plane mapping is where empirical tracing meets mathematical formalization. The blog post frames this as:
- Current empirical work (tracing) provides the raw data
- Complex plane mapping provides the mathematical framework
- Deviations from "normal orbits" become formally detectable anomalies
- This bridges toward the PhD goal of "verifiable specification" (top of handwritten diagram)

---

## 6. Flow Path 5: Action Execution

**Source references**:
- Blog post: Action component ("Execute decisions on the system")
- Blog post: Sensory Input / Brain / Action / Memory table

### Action Interface

```
+---------------------------+
| AGENT DECISION (Flow 3)   |
| "What to do next"         |
+---------------------------+
         |
         v
+---------------------------+
| ACTION DISPATCHER         |
| - Validate action safety  |
| - Check permissions       |
| - Log action for viz      |
+---------------------------+
         |
         +--------+--------+--------+
         |        |        |        |
         v        v        v        v
   Load new    Make      Send     Modify
   eBPF probe  syscall   signal   probe
   (expand     (interact (to      config
   sensing)    w/system) process) (refine
                                  sensing)
```

### Action Categories

| Category | Examples | MVP Priority |
|----------|----------|-------------|
| **Observe** (expand sensing) | Attach new eBPF probe, enable tracepoint | High (MVP) |
| **Interact** (poke system) | Make system calls, open files, read /proc | Medium |
| **Signal** (affect processes) | Send signals, trigger events | Low (post-MVP) |
| **Configure** (tune self) | Adjust probe filters, change sampling rate | High (MVP) |

For the MVP, actions are primarily about **steering observation** -- deciding what to look at next -- rather than actively modifying system state. This is safer and aligns with the "Adventure" phase of the research cycle.

---

## 7. Flow Path 6: Memory and Learning

**Source references**:
- Blog post: Memory & Learning component
- Blog post: "Record call sequences from crashes, avoid or focus on similar sequences"
- Handwritten diagram: "Infer / Feedback / Learning" loop
- Blog post: Reinforcement learning potential

### Memory Architecture

```
+---------------------------+
| EXPERIENCE STORE          |
|                           |
| +---------------------+  |
| | Trace Archive        |  |  Raw trace data for replay
| +---------------------+  |
|                           |
| +---------------------+  |
| | Pattern Library      |  |  Learned normal/anomalous patterns
| | - Normal orbits      |  |
| | - Known anomalies    |  |
| | - Syscall sequences  |  |
| +---------------------+  |
|                           |
| +---------------------+  |
| | Action History       |  |  What the agent did and outcomes
| | - Action taken       |  |
| | - Resulting state    |  |
| | - Reward/outcome     |  |
| +---------------------+  |
|                           |
| +---------------------+  |
| | Discovery Log        |  |  Found vulnerabilities, anomalies
| | - Anomaly reports    |  |
| | - Crash sequences    |  |
| | - Exploit paths      |  |
| +---------------------+  |
+---------------------------+
```

### Learning Feedback Loops

Two learning loops emerge from the documents:

**Loop 1: Pattern Refinement** (within a session)
```
Observe traces -> Detect anomaly -> Record pattern -> Use pattern to detect similar -> Refine
```

**Loop 2: Strategy Evolution** (across sessions)
```
Run exploration -> Record action-outcome pairs -> Train/update strategy -> Run better exploration
```

The blog post connects this to **reinforcement learning**: the agent starts with random/guided fuzzing, but over time learns which syscall sequences tend to produce crashes, which patterns indicate vulnerabilities, and which exploration paths are most productive.

---

## 8. Flow Path 7: Research Cycle (Adventure -> Quantifying)

**Source references**:
- Blog post: Four-stage research cycle
- Handwritten diagram: overall flow from monitor to specification
- Blog post (Gemini section): "From system explorer to designer" narrative

### The Four-Stage Cycle

This is the macro-level flow that encompasses the entire hackbot project lifecycle:

```
+------------------+         +------------------+
| 1. ADVENTURE     |-------->| 2. MODELING      |
| (Exploration)    |         | (Formalization)  |
|                  |         |                  |
| Monitor + Tunnel |         | Analyze abnormal |
| Discover hidden  |         | executions,      |
| behaviors,       |         | formalize into   |
| vulnerable paths |         | verifiable specs |
+------------------+         +------------------+
        ^                            |
        |                            v
+------------------+         +------------------+
| 4. QUANTIFYING   |<--------| 3. MODDING       |
| (Measurement)    |         | (Redesign)       |
|                  |         |                  |
| Measure if       |         | Redesign system  |
| redesigned system|         | with provable    |
| is safer,        |         | abstraction      |
| quantify         |         | layers           |
| "valorability"   |         |                  |
+------------------+         +------------------+
```

### How Each Stage Maps to hackbot Components

| Stage | hackbot Components Used | Visualization Role |
|-------|------------------------|-------------------|
| **Adventure** | Sensory Input + Brain + Action | Game View: agent exploring system, discovering paths |
| **Modeling** | Complex Plane + Memory | Signal View: patterns formalized as orbits, specifications extracted |
| **Modding** | Action (at system design level) | Game View: testing new abstractions, visualizing changed system |
| **Quantifying** | Complex Plane + Memory | Signal View: comparing before/after, measuring remaining vulnerability |

### MVP Alignment

The MVP lives entirely within **Stage 1 (Adventure)**. The visualization makes the exploration visible. The complex plane view provides early "Modeling" capability. Stages 3 and 4 are long-term research goals that inform the architecture but do not need to be implemented for the MVP.

---

## 9. MVP Architecture: Visualization-First Approach

### Design Philosophy

The user identified visualization as the MVP starting point. This is strategically correct because:

1. **Data collection already exists** (Sunwoo's professional eBPF work)
2. **Visualization makes everything else testable** -- you cannot debug an agent you cannot see
3. **The game-like rendering is the project's unique differentiator** -- many tools do tracing, but none render it as gameplay
4. **It aligns with the rs-sdk inspiration** -- start with the visual client, then add agent intelligence

### MVP Component Stack

```
+================================================================+
|                    BROWSER (Web Client)                          |
|                                                                  |
|  +---------------------------+  +----------------------------+  |
|  | GAME VIEW (Canvas/WebGL)  |  | SIGNAL VIEW (Canvas/D3)   |  |
|  |                           |  |                            |  |
|  | 2D isometric view of      |  | Complex plane plot         |  |
|  | system internals:         |  | showing trace signal       |  |
|  | - Process "rooms"         |  | as orbiting point(s).      |  |
|  | - Syscall "objects"       |  | Phase shifts highlighted.  |  |
|  | - Agent "character"       |  |                            |  |
|  | - Event "particles"       |  | + Frequency spectrum       |  |
|  +---------------------------+  +----------------------------+  |
|                                                                  |
|  +---------------------------+  +----------------------------+  |
|  | EVENT LOG / TIMELINE      |  | CONTROLS                   |  |
|  | Scrollable event stream   |  | Play/Pause/Speed           |  |
|  | with filtering            |  | Filter by PID/type         |  |
|  +---------------------------+  +----------------------------+  |
|                                                                  |
+================================================================+
                              |
                              | WebSocket
                              |
+================================================================+
|                    GATEWAY SERVER                                |
|                                                                  |
|  - Receives eBPF events from collector                          |
|  - Maintains world state model                                   |
|  - Sends frame updates to browser                               |
|  - Accepts agent commands (future)                              |
|  - Performs complex plane computation                            |
|                                                                  |
+================================================================+
                              |
                              | IPC / shared memory / socket
                              |
+================================================================+
|                    DATA COLLECTOR                                |
|                                                                  |
|  - eBPF programs (C)                                            |
|  - Userspace loader + event reader                              |
|  - Event serialization                                          |
|  - Can replay from trace files                                  |
|                                                                  |
+================================================================+
                              |
                              | eBPF ring buffer
                              |
+================================================================+
|                    TARGET SYSTEM (Kernel)                        |
+================================================================+
```

### Technology Mapping

| Component | Likely Technology | Rationale |
|-----------|-----------------|-----------|
| **Web Client** | TypeScript + Canvas/WebGL or Pixi.js | rs-sdk uses TypeScript web client; 2D rendering for MVP |
| **Gateway Server** | TypeScript (Node.js) or Go | rs-sdk uses Node.js gateway; Go for Dockercraft-style daemon |
| **Data Collector** | C (eBPF programs) + Python/Go (userspace) | Standard eBPF toolchain; libbpf or bcc |
| **Complex Plane** | Python (numpy/scipy) or TypeScript | Signal processing; could run server-side or client-side |
| **Agent Brain** | Python (LLM API client) | LLM integration; connects to OpenAI/Anthropic/local model |
| **Storage** | SQLite or filesystem | Simple persistence for MVP |

### MVP Build Phases

**Phase 1: Static Trace Viewer**
- Input: Pre-recorded trace file (from existing eBPF work)
- Output: Web page showing timeline of events, basic spatial layout of processes
- No real-time, no agent, no complex plane
- Goal: Prove that trace data can be rendered as a navigable visual space

**Phase 2: Real-time Stream + Game World**
- Input: Live eBPF event stream via gateway
- Output: 2D game-like view with process "rooms", syscall "objects", event "particles"
- Add WebSocket connection for real-time updates
- Goal: The kernel feels like a living, explorable world

**Phase 3: Complex Plane Overlay**
- Add signal processing pipeline
- Render trace-to-complex-plane mapping alongside game view
- Show normal orbits and deviations
- Goal: Mathematical analysis becomes visually intuitive

**Phase 4: Agent Character**
- Add the "bot" as a visible character in the game world
- Initially human-controlled (click to direct attention)
- Show agent's "field of view" (which probes are active)
- Goal: The system feels like a game with a player character

**Phase 5: LLM Brain Integration**
- Connect LLM to agent decision loop
- Agent moves autonomously based on LLM decisions
- Visualization shows agent reasoning (thought bubbles, decision trees)
- Goal: The auto-hunting vision becomes real

---

## 10. Component Interconnection Map

### Full System Data Flow

```
+------------------------------------------------------------------------+
|                                                                        |
|   TARGET SYSTEM (Kernel + LLM Runtime + Hardware)                      |
|                                                                        |
|   [processes] [syscalls] [power] [scheduling] [memory] [network]       |
|                                                                        |
+------|------|---------|-----------|---------|----------|----------------+
       |      |         |           |         |          |
       v      v         v           v         v          v
+------------------------------------------------------------------------+
|                                                                        |
|   eBPF PROBE LAYER                                                     |
|                                                                        |
|   [kprobes] [tracepoints] [perf_events] [uprobes] [XDP]              |
|                                                                        |
+--------------------------------------|----------------------------------+
                                       |
                                       | ring buffer
                                       v
+------------------------------------------------------------------------+
|                                                                        |
|   EVENT STREAM (the central data bus)                                  |
|                                                                        |
+-----+-----------+-----------+-----------+------------------------------+
      |           |           |           |
      v           v           v           v
+-----------+ +-----------+ +---------+ +-----------+
| GATEWAY   | | SIGNAL    | | MEMORY  | | AGENT     |
| SERVER    | | PROCESSOR | | STORE   | | BRAIN     |
|           | |           | |         | |           |
| World     | | Complex   | | Trace   | | LLM API   |
| state     | | plane     | | archive | | Decision  |
| model     | | mapping   | | Pattern | | engine    |
|           | | Anomaly   | | library | |           |
|           | | detection | | Action  | |           |
+-----------+ +-----------+ | history | +-----------+
      |           |         +---------+       |
      |           |              ^            |
      v           v              |            v
+-----------+ +-----------+     |      +-----------+
| GAME VIEW | | SIGNAL    |     |      | ACTION    |
| (browser) | | VIEW      |     +------| DISPATCH  |
|           | | (browser) |            |           |
+-----------+ +-----------+            +-----------+
                                             |
                                             v
                                       [back to eBPF
                                        probe layer:
                                        load new probes,
                                        adjust filters]
```

### Key Interaction Flows

**Flow A: Passive Observation** (MVP Phase 1-2)
```
Kernel -> eBPF -> Event Stream -> Gateway -> Browser (Game View)
```

**Flow B: Signal Analysis** (MVP Phase 3)
```
Event Stream -> Signal Processor -> Complex Plane Data -> Browser (Signal View)
                                                      -> Anomaly Alert -> Agent
```

**Flow C: Agent-Directed Exploration** (MVP Phase 4-5)
```
Browser (user click) or LLM -> Agent Brain -> Action Dispatch -> eBPF (new probe)
                                                              -> Kernel (syscall)
Result -> Event Stream -> Gateway -> Browser (shows result)
                       -> Memory Store (records outcome)
```

**Flow D: Learning Loop** (Post-MVP)
```
Memory Store (accumulated experience) -> Pattern Analysis -> Updated Strategy
Updated Strategy -> Agent Brain (better decisions next time)
```

---

## 11. Key Design Decisions and Trade-offs

### Decision 1: 2D vs 3D Visualization

**Choice**: 2D isometric (like rs-sdk) for MVP.

**Rationale**:
- rs-sdk demonstrates 2D is sufficient for complex agent-world interaction
- Dockercraft's 3D (Minecraft) is compelling but requires a 3D engine or game client dependency
- 2D is faster to develop, easier to iterate, runs in any browser
- Can evolve to 3D later if needed

### Decision 2: Real-time vs Replay

**Choice**: Start with replay, add real-time incrementally.

**Rationale**:
- Replay is deterministic, debuggable, shareable
- LLM workload traces already exist from professional work
- Real-time adds WebSocket complexity and performance constraints
- Replay-first means the visualization can be developed without a live kernel

### Decision 3: Agent Autonomy Level for MVP

**Choice**: Start at L0 (no agent, human views traces) and L1 (agent suggests).

**Rationale**:
- The visualization must be compelling before adding agent complexity
- L0 validates the "kernel as game world" concept
- L1 validates the LLM integration without risky autonomous actions
- The blog post's "auto-hunting" (L4) is the end goal, not the starting point

### Decision 4: Complex Plane Computation Location

**Choice**: Server-side computation, client-side rendering.

**Rationale**:
- Signal processing (FFT, phase extraction) is compute-intensive
- Server can use numpy/scipy or Rust for performance
- Client receives pre-computed complex coordinates for rendering
- Keeps the browser lightweight

### Decision 5: Spatial Mapping Strategy

**Choice**: Process hierarchy as spatial layout (rooms/zones).

**Rationale**:
- Processes are natural spatial units (each process is a "room")
- System calls are actions within/between rooms
- File descriptors are doors/passages between rooms
- Network connections link to external spaces
- This metaphor is intuitive and aligns with the "RPG exploration" vision

---

## 12. File and Concept Cross-Reference

### Source Document to Concept Mapping

| Concept | Research Statement | Blog Post | Handwritten Note | refs.md |
|---------|-------------------|-----------|-----------------|---------|
| Autonomous kernel bot | Section 0.1 (primary) | Auto-hunting architecture | "node/Monitor" | -- |
| Visualization as gameplay | Section 0.2 (primary) | rs-sdk discussion | -- | rs-sdk, Dockercraft |
| Complex plane mapping | Section 0.3 (primary) | "Tracing as Signal Processing" | "mathematical model -> specification" | -- |
| Mathematical formulation | Section 0.4 (primary) | Euler's formula discussion | "verifiable specification" | -- |
| eBPF as senses | Section 0.1 (implied) | Sensory Input component | "Monitor" -> "Trace" | -- |
| LLM as brain | Section 0.1 (implied) | Brain component | "Infer / Feedback / Learning" | rs-sdk (bot scripts) |
| Side-channel attacks | -- | Section 1 (detailed) | -- | -- |
| Provable abstractions | -- | Section 2 (detailed) | "verifiable specification" | -- |
| Research cycle (4 stages) | -- | Adventure/Modeling/Modding/Quantifying | Full diagram flow | -- |
| Multi-agent interaction | -- | -- | -- | Assembly |
| Infrastructure as game world | Section 0.2 | -- | -- | Dockercraft |
| Auto-hunting (24/7 agent) | -- | Detailed section | -- | -- |
| Specification mining | -- | Section on formal methods | "active Mapping -> Specification" | -- |
| Valorability (tolerable vuln) | -- | Stage 4 discussion | "Vulnerability??" / "abnormal exe" | -- |

### Source Files

| File | Path | Role in Architecture |
|------|------|---------------------|
| Research Statement | `/home/sunwoo/projects/hackbot/docs/Research_Statement.pdf` | Formal declaration of the four design pillars. Defines the vision at the highest level. |
| Blog Post | `/home/sunwoo/projects/hackbot/docs/Connecting the dots - Research Direction⚡️ __ 뿌리.pdf` | Most detailed exploration. Develops the auto-hunting architecture, four-component agent model, research cycle, and connections to PhD goals. Contains brainstorming with Gemini and Deepseek. |
| Handwritten Note | `/home/sunwoo/projects/hackbot/docs/mynote.jpg` | Earliest architectural sketch. Captures the observe-model-verify loop. Shows the tension between empirical observation and formal specification. |
| Visualization Refs | `/home/sunwoo/projects/hackbot/docs/refs.md` | Three links providing visualization technology references and inspiration. |
| MCP Config | `/home/sunwoo/projects/hackbot/.mcp.json` | Development environment configuration (context7, puppeteer, sequential-thinking, deepwiki, pdf-reader, chroma). |
| Investigation Report | `/home/sunwoo/projects/hackbot/claude-code-storage/claude-instance-1/INVESTIGATION_REPORT.md` | Detailed analysis of all source documents, synthesizing key concepts and MVP recommendations. |

### Current Project State

As of this analysis, the `/home/sunwoo/projects/hackbot/` repository contains **no source code** -- only documentation in `docs/` and development tooling configuration in `.mcp.json` and `.claude/`. The project is at the conceptual/architectural stage, with the research vision fully articulated across the documents but no implementation begun.

---

## Appendix A: Glossary of hackbot-Specific Terms

| Term | Meaning | Source |
|------|---------|--------|
| **hackbot** | The autonomous kernel exploration agent -- a "hacker bot" that navigates system internals | Project name, Research Statement |
| **Auto-hunting** | 24/7 autonomous vulnerability discovery, analogous to bitcoin mining | Blog post |
| **Valorability** | Tolerable/acceptable level of vulnerability; "how much risk can we accept?" | Blog post (Stage 4) |
| **Tunnel** | Information side-channel -- data leaking through unintended physical properties (power, timing, sound) | Blog post, handwritten note |
| **Monitor** | The eBPF-based observation layer -- the agent's senses | Handwritten note |
| **Modding** | Redesigning system abstractions based on discovered patterns (like modding a game's rules) | Blog post (Stage 3) |
| **Orbit** | Normal execution pattern when mapped to complex plane -- a stable trajectory | Blog post (Gemini section) |
| **Phase shift** | Deviation from normal orbit indicating anomaly (information leak, exploit, bug) | Blog post, Research Statement |
| **Adventure** | The exploration/discovery phase of the research cycle | Blog post (Stage 1) |

---

## Appendix B: Open Questions for Implementation

1. **Event stream format**: What serialization format for eBPF events? Protobuf, FlatBuffers, JSON, or custom binary?
2. **Spatial mapping algorithm**: How exactly to map process/syscall topology to 2D game coordinates? Force-directed graph? Manual layout? Hierarchical?
3. **Complex plane mapping function**: What specific features map to amplitude vs. phase? How to define "normal orbit" for a given workload?
4. **LLM context window management**: How to summarize system state for the LLM within token limits?
5. **Safety boundary for autonomous actions**: What actions can the agent take without human approval?
6. **Performance budget**: How many events/second can the visualization handle before frame rate degrades?
7. **Replay format**: What trace file format to use for the MVP replay viewer? perf.data? Custom?
