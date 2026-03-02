# Investigation Report: hackbot/docs/

**Date**: 2026-03-02
**Investigator**: Claude Opus 4.6
**Scope**: All files in `/home/sunwoo/projects/hackbot/docs/`

---

## Files Analyzed

| File | Type | Description |
|------|------|-------------|
| `/home/sunwoo/projects/hackbot/docs/Research_Statement.pdf` | PDF (2 pages) | Sunwoo Jang's formal research statement outlining the hackbot vision |
| `/home/sunwoo/projects/hackbot/docs/Connecting the dots - Research Direction⚡️ __ 뿌리.pdf` | PDF (13 pages) | Blog post capturing an extended brainstorming session with AI assistants (Gemini, Deepseek) about connecting research identity |
| `/home/sunwoo/projects/hackbot/docs/mynote.jpg` | Image | Handwritten architecture diagram depicting the system's conceptual flow |
| `/home/sunwoo/projects/hackbot/docs/refs.md` | Markdown | Three visualization reference links |

---

## 1. Research Identity and Background

**Researcher**: Sunwoo Jang (swjang702)

**Current professional work**: Implementing low-level eBPF-based tracing for profiling heterogeneous accelerators (GPU/NPU) in LLM workload serving systems. This involves power consumption profiling and event tracing at the kernel level.

**Past experience**:
- **SecQuant**: Quantified security by mapping disconnected system calls between containers and hosts using ftrace. Graduate research on container isolation.
- **Genians**: Built Linux EDR (Endpoint Detection and Response) agents -- security monitoring at the kernel level.
- **Systems programming**: Deep experience with eBPF, ftrace, strace, system call tracing.

**PhD aspiration**: Working with Prof. Anton Burtsev on building **provable, secure system abstractions** -- specifically "practical, provable abstraction layers" for operating systems. The goal is formal verification of system designs, particularly for heterogeneous hardware (GPUs, TPUs, accelerators).

**Core tension the researcher is resolving**: The gap between empirical tracing (observing runtime behavior) and formal verification (mathematical proof of correctness). The researcher's insight is that tracing is not just a debugging tool but an **empirical verification instrument** -- checking system invariants before full formal verification is achieved.

---

## 2. The Hackbot Vision

### 2.1 Central Metaphor

A "hacker bot" that travels through a system's internals the way a micro bio-bot travels through a body clearing cancer cells. The bot autonomously navigates opaque layers of complex systems (kernel, containers, LLM infrastructure), discovering hidden behaviors and vulnerabilities.

### 2.2 Design Pillars (from Research Statement)

1. **Autonomous bot in the kernel**: An agent that lives inside the operating system, using tracing as its senses, making decisions about what to explore and what actions to take.

2. **Visualization**: All actions the bot performs are rendered in real-time, as if the bot were a character in a video game. Motivated by DeepMind's visualization work and the rs-sdk project (RuneScape automation). The idea is to gamify the agent's actions within the kernel.

3. **Complex Plane mapping**: The trace of the bot maps onto a complex plane. System event flows become complex-number signals. Normal execution has characteristic frequency and phase. Anomalies (information leaks, exploits) manifest as **phase shifts** in this signal. This provides a mathematical basis for anomaly detection.

4. **Mathematical Formulation**: The bot applies mathematical algorithms to improve its own capabilities over time.

### 2.3 The "Auto-Hunting" System Architecture

The blog post develops the hackbot idea into an **autonomous security exploration agent** with four components:

| Component | Function | Implementation |
|-----------|----------|----------------|
| **Sensory Input** | Detect system state via low-level tracing | eBPF probes on power, system calls, scheduling events -- the agent's "eyes and ears" |
| **Brain** | Decide next actions based on observations | LLM agent that reasons about system state (e.g., "process returned 'permission denied', should I try ptrace next?") |
| **Action** | Execute decisions on the system | Interface to make system calls, load BPF programs, send signals to processes |
| **Memory and Learning** | Store experiences and learn patterns | Record call sequences from crashes, avoid or focus on similar sequences. Reinforcement learning potential. |

### 2.4 Research Journey Framework

The blog post identifies a four-stage research cycle that maps to the handwritten diagram:

1. **Adventure (Exploration)**: Use Monitor and Tunnel concepts to discover hidden system behaviors and vulnerable paths. This is the current tracing work.
2. **Modeling**: Analyze discovered abnormal executions, formalize patterns into verifiable specifications.
3. **Modding (Redesign)**: Based on models, redesign the system with new provable abstraction layers (new container runtimes, security kernel modules).
4. **Quantifying**: Measure whether the redesigned system is safer, quantify the remaining "valorability" (tolerable vulnerability).

---

## 3. Key Research Connections Identified

### 3.1 Security Angle: Side-Channel Attacks on LLM Infrastructure

The power and event traces collected for performance analysis are simultaneously **side-channel attack vectors**. Research question: Can an attacker on a shared heterogeneous system infer sensitive information about an LLM workload (conversation topic, prompt length, specific tokens) by monitoring power, cache, and memory bandwidth via eBPF? The defense is **isolation** -- and measuring its effectiveness ties back to the PhD goal.

### 3.2 Systems Angle: Provable Abstractions for Accelerators

Current OS abstractions for GPUs/TPUs are primitive character devices hiding immense complexity. Tracing work gathers the empirical data needed to design new, cleaner, formally verifiable abstractions. This aligns with Prof. Burtsev's philosophy of "design philosophy where careful system design can make strong guarantees more practical."

### 3.3 Tracing as Function Understanding

The note "tracing: x -> function -> y. The art of figuring out the function" captures the essence: tracing is reverse-engineering a system's behavior from inputs and outputs. This connects directly to **fuzzing** (providing unexpected inputs to find where behavior deviates from specification) and **specification mining** (automatically learning invariants from traces).

### 3.4 Verified AI-System Interface

A bridging concept: using eBPF to prove where security guarantees break when accelerator resources are shared, then building verified microkernel designs to defend those boundaries. Combined with **Opaque Workload Isolation** -- building "Privacy-Preserving Abstraction" at the system level for workloads like LLMs whose internals must not be visible.

---

## 4. The Handwritten Architecture Diagram (mynote.jpg)

The diagram (also embedded in the Research Statement PDF as "Figure 1: My rough note about the architecture") shows the following flow:

- **Top level**: "mathematical model??" connected to "verifiable specification" -- the formal goal
- **Left side**: "node?? (comp or system)" with a "Monitor" component -- the system being observed
- **Center flow**: Monitor feeds into "Trace" which flows through "active Mapping" producing "Specification"
- **Right loop**: "Infer / Feedback / Learning" cycle -- the agent learns from observations
- **Bottom concerns**: "System Abstraction? / Overview? / Adversize?" leading to "For APP Vulnerability??" / "abnormal exe curation" / "path...??"
- **Korean annotations**: References to using models to systematically approach research directions, and a note about connecting concepts to build understanding

The diagram captures the tension between bottom-up empirical observation (tracing/monitoring) and top-down formal specification (mathematical models, verification), with the agent sitting in the middle performing inference and learning.

---

## 5. Visualization References (refs.md)

### 5.1 rs-sdk (https://github.com/MaxBittker/rs-sdk)

A RuneScape bot development framework designed for AI agent research. TypeScript SDK with a web-based game client, gateway server, and standalone game engine (LostCity 2004scape). Agents interact with the game world, make decisions, and their actions are visible in the game UI. **This is the primary visualization inspiration** -- the hackbot's kernel exploration should look like an agent playing an RPG, navigating the system, encountering obstacles, leveling up.

Key architectural parallel: rs-sdk has Sensory Input (game state), Brain (bot script/LLM), Action (game commands), and visual feedback -- exactly mirroring the hackbot's four components.

### 5.2 Assembly (https://assembly.louve.systems/)

A Core War-inspired programming game where "delegates" (small programs) are released into shared memory space. They interact, compete, and affect each other through memory manipulation. **Relevant parallel**: Multiple agents/processes coexisting in shared system space, competing for resources, potentially interfering with each other -- similar to the kernel environment the hackbot navigates.

### 5.3 Dockercraft (https://github.com/docker-archive-public/docker.dockercraft)

Docker container management visualized inside Minecraft. Containers appear as in-game structures; users control them with levers and buttons. A Go daemon bridges Docker API events to the game engine. **Relevant parallel**: System-level abstractions (containers) rendered as interactive game objects. Demonstrates the concept of "infrastructure as game world." Archived project but strong proof-of-concept for the visualization-of-systems idea.

---

## 6. MVP Visualization Analysis

Given the user's note that this is "a good start point for visualization from the perspective of MVP," here is what the documents suggest for a minimum viable product:

### 6.1 What Must Be Visualized

1. **System state**: Kernel internals as a navigable space -- processes, system calls, memory regions, file descriptors rendered as objects in a game-like environment
2. **Agent actions**: The bot's movements through the system -- what it probes, what it reads, what it attempts -- shown as real-time character actions
3. **Trace data**: eBPF event streams transformed into visual signals (the complex plane mapping suggests waveforms, phase diagrams, or orbital paths)
4. **Anomalies**: Abnormal execution paths highlighted visually -- phase shifts in the complex plane, unusual traversal patterns, vulnerability discoveries

### 6.2 Architectural Components for MVP

Based on the documents, an MVP needs these layers:

| Layer | MVP Scope | Inspiration |
|-------|-----------|-------------|
| **Data Collection** | eBPF probes collecting system calls, power events, scheduling | Current professional work (already exists) |
| **Agent Logic** | LLM deciding what to explore next based on trace data | rs-sdk bot scripts, but with LLM brain |
| **Visualization Engine** | Real-time rendering of agent traversing system space | rs-sdk web client, Dockercraft Minecraft view |
| **Complex Plane View** | Trace-to-signal mapping showing normal vs. anomalous patterns | Research Statement section 0.3 |
| **Memory/Learning Store** | Persistent record of discovered paths and patterns | Auto-hunting architecture "Memory and Learning" component |

### 6.3 Simplest Viable Starting Point

The documents suggest starting with **LLM workload profiling as the concrete entry point**. Specifically: visualize the power trace patterns during LLM inference, let an agent analyze those patterns to find anomalous power consumption (potential side-channel leakage), and render this exploration as a game-like experience.

The rs-sdk architecture provides the most direct template:
- Game engine = System being traced (kernel/LLM serving system)
- Web client = Visualization frontend showing the "game world" (system internals)
- Gateway = Bridge between eBPF data collection and the visualization
- Bot SDK = The LLM agent's decision-making interface

### 6.4 Key Design Decisions for MVP

1. **2D vs 3D**: rs-sdk is 2D (isometric game view), Dockercraft is 3D (Minecraft). For MVP, 2D is likely more practical.
2. **Real-time vs replay**: The vision calls for real-time, but MVP could start with trace replay for development speed.
3. **Agent autonomy level**: MVP could begin with human-guided exploration with agent suggestions, progressing to full autonomy.
4. **Complex plane integration**: Could start as a secondary dashboard view alongside the game view, showing signal analysis of the trace data.

---

## 7. Connections Between All Documents

The documents form a coherent narrative:

- The **Research Statement** is the formal declaration of intent: an autonomous kernel bot with visualization, complex plane analysis, and mathematical formulation.
- The **handwritten note** is the earliest architectural sketch, capturing the observe-model-verify loop with security concerns.
- The **blog post** ("Connecting the dots") is the most detailed exploration, working through how current professional experience (eBPF/LLM profiling) connects to the PhD vision (provable secure systems), with the hackbot/auto-hunting concept as the unifying project. It identifies the four-component agent architecture and frames the research as Adventure -> Modeling -> Modding -> Quantifying.
- The **refs.md** links are the visualization technology references -- rs-sdk for the game-like agent view, Assembly for the shared-memory interaction model, Dockercraft for the infrastructure-as-game-world concept.

The project name "hackbot" itself encodes the vision: a bot that hacks (explores, probes, discovers vulnerabilities) autonomously within systems, with its journey visualized as gameplay.

---

## 8. Summary of Key Concepts

| Concept | Description |
|---------|-------------|
| **Tracing as senses** | eBPF/ftrace/strace provide the agent's perception of system state |
| **LLM as brain** | Large language model makes strategic decisions about exploration |
| **Gamification of kernel exploration** | System internals rendered as a game world the agent navigates |
| **Complex plane mapping** | Traces converted to complex signals; anomalies appear as phase shifts |
| **Auto-hunting** | Autonomous 24/7 vulnerability discovery, like bitcoin mining but for security bugs |
| **Side-channel as research bridge** | Power/event profiling data is both performance insight and attack vector |
| **Provable abstractions** | Ultimate goal: redesign system layers with formal guarantees, informed by empirical tracing |
| **Observe-Model-Redesign-Quantify** | The research cycle from empirical data to verified system design |
| **Opaque workload isolation** | Privacy-preserving abstraction for workloads (LLMs) whose internals must stay hidden |
| **Specification mining** | Automatically learning system invariants from traces to build formal specifications |
