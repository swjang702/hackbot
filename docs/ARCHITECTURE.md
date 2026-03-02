# hackbot Architecture

## Full System Architecture

```
 ╔══════════════════════════════════════════════════════════════════════════════════╗
 ║                                                                                ║
 ║                          TARGET SYSTEM (Kernel)                                ║
 ║                                                                                ║
 ║   ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐       ║
 ║   │Processes │  │ Syscalls │  │  Power   │  │Scheduler │  │ GPU/NPU  │       ║
 ║   └────┬─────┘  └────┬─────┘  └────┬─────┘  └────┬─────┘  └────┬─────┘       ║
 ║        │              │             │              │              │             ║
 ╚════════╪══════════════╪═════════════╪══════════════╪══════════════╪═════════════╝
          │              │             │              │              │
          ▼              ▼             ▼              ▼              ▼
 ╔══════════════════════════════════════════════════════════════════════════════════╗
 ║                                                                                ║
 ║                         eBPF PROBE LAYER                                       ║
 ║                                                                                ║
 ║   ┌──────────┐  ┌───────────┐  ┌───────────┐  ┌──────────┐  ┌──────────┐     ║
 ║   │ kprobes  │  │tracepoints│  │perf_events│  │  uprobes │  │   XDP    │     ║
 ║   └──────────┘  └───────────┘  └───────────┘  └──────────┘  └──────────┘     ║
 ║                                                                                ║
 ╚════════════════════════════════╤═══════════════════════════════════════════════╝
                                  │
                                  │  ring buffer / perf buffer
                                  │
 ╔════════════════════════════════╧═══════════════════════════════════════════════╗
 ║                                                                                ║
 ║                    EVENT STREAM  (the central data bus)                         ║
 ║                                                                                ║
 ║  ┌─────────────────────────────────────────────────────────────────────────┐   ║
 ║  │  { ts, type, pid, tid, cpu, comm, payload }   (JSON Lines / .jsonl)    │   ║
 ║  └─────────────────────────────────────────────────────────────────────────┘   ║
 ║                                                                                ║
 ╚═══╤═══════════╤═════════════╤════════════════╤═════════════════════════════════╝
     │           │             │                │
     ▼           ▼             ▼                ▼
 ┌────────┐ ┌──────────┐ ┌──────────┐  ┌────────────┐
 │GATEWAY │ │ SIGNAL   │ │ MEMORY   │  │   AGENT    │
 │SERVER  │ │PROCESSOR │ │ STORE    │  │   BRAIN    │
 │        │ │          │ │          │  │            │
 │ World  │ │ Complex  │ │ Trace    │  │ LLM API    │
 │ state  │ │ plane    │ │ archive  │  │ OODA loop  │
 │ model  │ │ mapping  │ │ Pattern  │  │ Decision   │
 │        │ │ Anomaly  │ │ library  │  │ engine     │
 │        │ │ detect   │ │ Actions  │  │            │
 └───┬────┘ └────┬─────┘ └────┬─────┘  └─────┬──────┘
     │           │             ▲                │
     │           │             │                ▼
     │           │             │          ┌──────────┐
     │           │             └──────────│ ACTION   │
     │           │                        │ DISPATCH │
     │           │                        └────┬─────┘
     │           │                             │
     │           │                             │  [load new probe,
     │           │                             │   make syscall,
     │           │                             │   adjust filters]
     │           │                             │
     │           │                             ▼
     │           │                        back to eBPF
     │           │                        probe layer
     ▼           ▼
 ╔══════════════════════════════════════════════════════════════════════════════════╗
 ║                                                                                ║
 ║                         BROWSER  (Web Client)                                  ║
 ║                                                                                ║
 ║  ┌─────────────────────────────────────────────────────────────────────────┐   ║
 ║  │                      WebSocket Connection                               │   ║
 ║  │                     (JSON messages, auto-reconnect)                      │   ║
 ║  └─────────────────────────────┬───────────────────────────────────────────┘   ║
 ║                                │                                               ║
 ║                                ▼                                               ║
 ║  ┌─────────────────────────────────────────────────────────────────────────┐   ║
 ║  │                        APP ORCHESTRATOR                                 │   ║
 ║  │               (dispatches messages to panels)                           │   ║
 ║  └──────┬──────────────┬──────────────┬──────────────┬─────────────────────┘   ║
 ║         │              │              │              │                          ║
 ║         ▼              ▼              ▼              ▼                          ║
 ║  ┌────────────┐ ┌────────────┐ ┌────────────┐ ┌────────────┐                  ║
 ║  │            │ │            │ │            │ │            │                  ║
 ║  │  GAME VIEW │ │SIGNAL VIEW │ │ EVENT LOG  │ │  CONTROLS  │                  ║
 ║  │            │ │            │ │            │ │            │                  ║
 ║  │ (Pixi.js)  │ │(Canvas 2D) │ │ (HTML/CSS) │ │ (HTML/CSS) │                  ║
 ║  │            │ │            │ │            │ │            │                  ║
 ║  └────────────┘ └────────────┘ └────────────┘ └────────────┘                  ║
 ║                                                                                ║
 ╚══════════════════════════════════════════════════════════════════════════════════╝
```

---

## Browser Panel Layout

```
 ┌──────────────────────────────────────────────────────────────────────┐
 │                                                                      │
 │   ┌──────────────────────────────┐  ┌────────────────────────────┐  │
 │   │                              │  │                            │  │
 │   │         GAME VIEW            │  │     COMPLEX PLANE VIEW     │  │
 │   │       (Pixi.js WebGL)        │  │      (Canvas 2D)           │  │
 │   │                              │  │                            │  │
 │   │  ┌─────────┐  ┌─────────┐   │  │         Im                 │  │
 │   │  │ python3 │  │ worker1 │   │  │          ▲                 │  │
 │   │  │ PID:100 │  │ PID:101 │   │  │          │    ╭──╮        │  │
 │   │  │  ░░░░░  │  │  ▓▓▓▓▓  │   │  │          │   ╱    ╲       │  │
 │   │  │  ░░░░░  │  │  ▓▓▓▓▓  │   │  │          │  │  ●→  │      │  │
 │   │  └─────────┘  └─────────┘   │  │    ──────┼──╲────╱──── Re │  │
 │   │                              │  │          │   ╰──╯         │  │
 │   │  ┌─────────┐  ┌─────────┐   │  │          │                 │  │
 │   │  │ worker2 │  │ worker3 │   │  │          │  z(t) orbit     │  │
 │   │  │ PID:102 │  │ PID:103 │   │  │                            │  │
 │   │  │  ░▓░▓░  │  │  ░░░░░  │   │  ├────────────────────────────┤  │
 │   │  └─────────┘  └─────────┘   │  │                            │  │
 │   │                              │  │     PHASE DIAGRAM          │  │
 │   │   [pan: drag | zoom: scroll] │  │      theta(t) vs time      │  │
 │   │                              │  │   ┌╌╌╌┐                   │  │
 │   │                              │  │ ~~│   │~~~  ← anomaly     │  │
 │   │                              │  │   └╌╌╌┘                   │  │
 │   └──────────────────────────────┘  └────────────────────────────┘  │
 │                                                                      │
 │   ┌──────────────────────────────┐  ┌────────────────────────────┐  │
 │   │        EVENT LOG             │  │        CONTROLS            │  │
 │   │                              │  │                            │  │
 │   │ [1.002s] 101:worker1         │  │  PIDs: [✓100] [✓101]      │  │
 │   │   write(fd=3, 4096) -> 4096  │  │        [✓102] [✓103]      │  │
 │   │ [1.003s] 101:worker1         │  │                            │  │
 │   │   gpu_submit(batch=128)      │  │  Types: [✓syscall] [✓gpu] │  │
 │   │ [1.005s] 102:worker2         │  │         [✓power] [✓sched] │  │
 │   │   read(fd=5, 1024) -> 1024   │  │                            │  │
 │   │ [1.006s] 100:python3         │  │  Anomaly threshold: 2.0σ  │  │
 │   │   sched_switch(prev=idle)    │  │                            │  │
 │   └──────────────────────────────┘  └────────────────────────────┘  │
 │                                                                      │
 │   ┌──────────────────────────────────────────────────────────────┐   │
 │   │  ◄◄  ▶  ►►   [0.5x] [1x] [2x] [5x]    ═══●═══════════    │   │
 │   │                                          1.006s / 5.000s     │   │
 │   │                         TIMELINE                             │   │
 │   └──────────────────────────────────────────────────────────────┘   │
 │                                                                      │
 └──────────────────────────────────────────────────────────────────────┘
```

---

## Data Flow Diagrams

### MVP Flow (Phases 1-2): Trace Replay

```
 ┌─────────────────┐
 │  .jsonl file     │
 │  (pre-recorded   │
 │   eBPF trace)    │
 └────────┬─────────┘
          │
          ▼
 ┌─────────────────┐     ┌─────────────────┐
 │  trace_loader   │────▶│  world_model    │
 │  Parse & sort   │     │  Process map,   │
 │  events         │     │  fd table,      │
 └────────┬─────────┘     │  connections    │
          │               └────────┬─────────┘
          ▼                        │
 ┌─────────────────┐               │         ┌─────────────────┐
 │ trace_replayer  │               │         │signal_processor │
 │ Emit at timing  │───────────────┼────────▶│ Sliding window  │
 │ Batch per 16ms  │               │         │ r(t), theta(t)  │
 │ Speed control   │               │         │ z = r*e^(iθ)    │
 └────────┬─────────┘               │         │ Anomaly detect  │
          │                        │         └────────┬─────────┘
          ▼                        ▼                  │
 ┌───────────────────────────────────────────────┐    │
 │              gateway.py                       │    │
 │                                               │◀───┘
 │  On connect:  send world_state                │
 │  During play: send events batch (every 16ms)  │
 │               send signal      (every 100ms)  │
 │               send world_state (every 500ms)  │
 │  On command:  play/pause/seek/speed/filter    │
 └───────────────────────┬───────────────────────┘
                         │
                         │  WebSocket (JSON)
                         │
 ┌───────────────────────▼───────────────────────┐
 │              connection.ts                    │
 │  Parse messages, dispatch by msg type         │
 └──┬────────────┬─────────────┬────────────┬────┘
    │            │             │            │
    ▼            ▼             ▼            ▼
 ┌────────┐  ┌──────────┐  ┌────────┐  ┌──────────┐
 │ Game   │  │ Signal   │  │ Event  │  │ Timeline │
 │ View   │  │ View     │  │ Log    │  │          │
 │        │  │          │  │        │  │          │
 │world   │  │complex   │  │events  │  │playback  │
 │state + │  │plane +   │  │batch   │  │status    │
 │events  │  │phase +   │  │        │  │          │
 │batch   │  │signal    │  │        │  │          │
 └────────┘  └──────────┘  └────────┘  └──────────┘
```

### Future Flow (Phase 3+): Live Streaming

```
 ┌─────────────────┐
 │  Kernel          │
 │  (live system)   │
 └────────┬─────────┘
          │
          │  eBPF ring buffer
          ▼
 ┌─────────────────┐
 │  eBPF collector │     (runs as root)
 │  (C + bcc/libbpf)│
 └────────┬─────────┘
          │
          │  Unix socket / TCP  (newline-delimited JSON)
          ▼
 ┌─────────────────┐
 │event_ingestion  │     (runs as user)
 │  Same pipeline  │────▶  gateway -> browser
 │  as trace_loader│
 └─────────────────┘
```

### Future Flow (Phase 5): Agent Decision Loop

```
          ┌──────────────────────────────────────────┐
          │              OODA LOOP                    │
          │                                          │
          │  ┌──────────┐    ┌──────────────────┐    │
          │  │ OBSERVE   │◀──│ world_model      │    │
          │  │ System    │   │ (process map,    │    │
          │  │ state     │   │  recent events,  │    │
          │  │ summary   │   │  anomaly flags)  │    │
          │  └─────┬─────┘   └──────────────────┘    │
          │        │                                  │
          │        ▼                                  │
          │  ┌──────────┐    ┌──────────────────┐    │
          │  │ ORIENT   │◀──│ memory_store     │    │
          │  │ Add      │   │ (previous        │    │
          │  │ context  │   │  findings,       │    │
          │  │          │   │  exploration     │    │
          │  └─────┬─────┘   │  history)        │    │
          │        │         └──────────────────┘    │
          │        ▼                                  │
          │  ┌──────────┐    ┌──────────────────┐    │
          │  │ DECIDE   │───▶│ LLM API          │    │
          │  │ Choose   │◀──│ (Claude/GPT)     │    │
          │  │ action   │   │ Returns action   │    │
          │  └─────┬─────┘   │ + reasoning      │    │
          │        │         └──────────────────┘    │
          │        ▼                                  │
          │  ┌──────────┐                             │
          │  │ ACT      │──▶ move_to(pid)            │
          │  │ Execute  │──▶ focus_on(event_type)    │
          │  │ decision │──▶ flag_anomaly(desc)      │
          │  │          │──▶ adjust_filter(params)   │
          │  └─────┬─────┘                            │
          │        │                                  │
          │        └──────────────────────────────────┘
          │               feedback (observe result)
          │
          ▼
 ┌──────────────────┐         ┌──────────────────┐
 │ gateway.py       │────────▶│ Browser          │
 │ agent_state msg  │         │ Agent panel:     │
 │ (position,       │         │  - reasoning log │
 │  attention,      │         │  - action chosen │
 │  reasoning)      │         │  - alternatives  │
 └──────────────────┘         └──────────────────┘
```

---

## Trace Event Schema

```
┌─────────────────────────────────────────────────────────────────┐
│                      TraceEvent                                 │
├─────────────────────────────────────────────────────────────────┤
│  ts:      uint64    (nanoseconds since epoch)                   │
│  type:    string    (discriminator for payload)                 │
│  pid:     uint32    (process ID)                                │
│  tid:     uint32    (thread ID)                                 │
│  cpu:     uint16    (CPU core)                                  │
│  comm:    string    (process name, max 16 chars)                │
│  payload: object    (type-specific data)                        │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  type = "syscall_enter"                                         │
│    payload: { nr: int, name: str, fd?: int, count?: int, ... }  │
│                                                                 │
│  type = "syscall_exit"                                          │
│    payload: { nr: int, name: str, ret: int }                    │
│                                                                 │
│  type = "sched_switch"                                          │
│    payload: { prev_pid: int, next_pid: int, prev_state: str }   │
│                                                                 │
│  type = "power_trace"                                           │
│    payload: { watts: float, domain: str }                       │
│                                                                 │
│  type = "process_fork"                                          │
│    payload: { parent_pid: int, child_pid: int }                 │
│                                                                 │
│  type = "process_exit"                                          │
│    payload: { exit_code: int }                                  │
│                                                                 │
│  type = "gpu_submit" / "gpu_complete"                           │
│    payload: { batch_size: int, queue: str }                     │
│                                                                 │
└─────────────────────────────────────────────────────────────────┘
```

---

## WebSocket Protocol

```
 SERVER ──────────────────────────────────────────────▶ CLIENT

 ┌─────────────────────────────────────────────────────────────┐
 │ world_state                                                 │
 │ { "msg": "world_state",                                     │
 │   "processes": [                                            │
 │     { "pid": 100, "comm": "python3", "parent": 1,          │
 │       "status": "running", "syscall_count": 42 }            │
 │   ],                                                        │
 │   "connections": [                                          │
 │     { "from_pid": 100, "to_pid": 101, "type": "pipe" }     │
 │   ] }                                                       │
 ├─────────────────────────────────────────────────────────────┤
 │ events                                                      │
 │ { "msg": "events",                                          │
 │   "batch": [ { "ts": ..., "type": ..., ... }, ... ] }       │
 ├─────────────────────────────────────────────────────────────┤
 │ signal                                                      │
 │ { "msg": "signal",                                          │
 │   "z_real": 0.5, "z_imag": 0.3, "theta": 1.2,             │
 │   "anomaly": false, "deviation": 0.05 }                     │
 ├─────────────────────────────────────────────────────────────┤
 │ playback                                                    │
 │ { "msg": "playback",                                        │
 │   "status": "playing", "speed": 1.0,                        │
 │   "position_ns": 1709384000000000000 }                      │
 └─────────────────────────────────────────────────────────────┘


 CLIENT ──────────────────────────────────────────────▶ SERVER

 ┌─────────────────────────────────────────────────────────────┐
 │ { "cmd": "load",   "file": "sample-llm-workload.jsonl" }   │
 │ { "cmd": "play" }                                           │
 │ { "cmd": "pause" }                                          │
 │ { "cmd": "seek",   "position_ns": 1709384000000000000 }    │
 │ { "cmd": "speed",  "multiplier": 2.0 }                     │
 │ { "cmd": "filter", "pids": [100,101], "types": ["syscall_enter"] } │
 └─────────────────────────────────────────────────────────────┘
```

---

## Complex Plane Signal Processing

```
 RAW EVENTS (timestamped stream)
      │
      ▼
 ┌──────────────────────────────────────────────────────┐
 │  SLIDING WINDOW (100ms width, 50ms step)             │
 │                                                      │
 │  ┌─────┐ ┌─────┐ ┌─────┐ ┌─────┐ ┌─────┐          │
 │  │ w_0 │ │ w_1 │ │ w_2 │ │ w_3 │ │ w_4 │ ...      │
 │  └──┬──┘ └──┬──┘ └──┬──┘ └──┬──┘ └──┬──┘          │
 └─────┼───────┼───────┼───────┼───────┼───────────────┘
       │       │       │       │       │
       ▼       ▼       ▼       ▼       ▼
 ┌──────────────────────────────────────────────────────┐
 │  FEATURE EXTRACTION (per window)                     │
 │                                                      │
 │  r(t)     = syscall_rate  (events/second)            │
 │             → amplitude (distance from origin)       │
 │                                                      │
 │  theta(t) = Shannon entropy of syscall type dist.    │
 │             → phase angle (behavioral diversity)     │
 │                                                      │
 │  Low entropy = one thing repeatedly (stable phase)   │
 │  High entropy = diverse activity (shifting phase)    │
 └───────────────────────┬──────────────────────────────┘
                         │
                         ▼
 ┌──────────────────────────────────────────────────────┐
 │  COMPLEX MAPPING                                     │
 │                                                      │
 │  z(t) = r(t) * e^(i * theta(t))                     │
 │       = r * cos(theta) + i * r * sin(theta)          │
 │                                                      │
 │         Im ▲                                         │
 │            │      ● z(t)                             │
 │            │     ╱                                    │
 │            │    ╱  r(t)                               │
 │            │   ╱                                      │
 │            │  ╱ theta(t)                              │
 │    ────────┼──────────▶ Re                            │
 │            │                                          │
 └───────────────────────┬──────────────────────────────┘
                         │
                         ▼
 ┌──────────────────────────────────────────────────────┐
 │  ANOMALY DETECTION                                   │
 │                                                      │
 │  EMA(z) = exponential moving average of z(t)         │
 │  σ(z)   = running standard deviation                 │
 │                                                      │
 │  deviation = |z(t) - EMA(z)| / σ(z)                 │
 │                                                      │
 │  if deviation > threshold (default 2.0):             │
 │    anomaly = true                                    │
 │                                                      │
 │  Normal LLM workload:                                │
 │    Prefill:  high r, low theta  → "prefill zone"     │
 │    Decode:   moderate r, stable theta → "decode zone" │
 │    Anomaly:  r spike + theta shift → orbit deviation  │
 └──────────────────────────────────────────────────────┘
```

---

## Mock Trace Narrative (sample-llm-workload.jsonl)

```
 Time ──────────────────────────────────────────────────────────▶

 0s          1s          2s          3s          4s      4.5s    5s
 │           │           │           │           │        │      │
 │ STARTUP   │       PREFILL PHASE       │  DECODE PHASE  │NORMAL│
 │           │                           │                │      │
 │ fork x4   │ GPU submits (large batch) │ GPU submits    │      │
 │ open/mmap │ high read/write           │ (small, regular)│      │
 │           │ power: HIGH SPIKE         │ periodic writes │      │
 │           │                           │ power: moderate │      │
 │           │                           │                │      │
 │  orbit    │ orbit: "prefill zone"     │ orbit: "decode │      │
 │  forming  │ (high r, low θ)           │  zone"         │      │
 │           │                           │(moderate r,    │      │
 │           │                           │ stable θ)      │      │
 │           │                           │                │      │
                                                ┌─────────┐
                                                │ ANOMALY │
                                                │         │
                                                │open(/proc/maps)
                                                │read(shm)│
                                                │perf_event│
                                                │         │
                                                │orbit:   │
                                                │ DEVIATION│
                                                │(r spike +│
                                                │ θ shift) │
                                                └─────────┘
```

---

## Research Vision Mapping

```
 ┌────────────────────────────────────────────────────────────────────┐
 │                    RESEARCH CYCLE                                  │
 │                                                                    │
 │  ┌──────────────┐         ┌──────────────┐                        │
 │  │ 1. ADVENTURE │────────▶│ 2. MODELING  │                        │
 │  │ (Exploration)│         │(Formalization)│                        │
 │  │              │         │              │                        │
 │  │ Phase 1: Viz │         │ Phase 2:     │                        │
 │  │ Phase 3: Live│         │ Complex Plane│                        │
 │  │ Phase 4: Agent│        │              │                        │
 │  └──────────────┘         └──────┬───────┘                        │
 │         ▲                        │                                │
 │         │                        ▼                                │
 │  ┌──────────────┐         ┌──────────────┐                        │
 │  │4. QUANTIFYING│◀────────│ 3. MODDING   │                        │
 │  │(Measurement) │         │ (Redesign)   │                        │
 │  │              │         │              │                        │
 │  │ Measure if   │         │ Redesign     │                        │
 │  │ system is    │         │ with provable│                        │
 │  │ safer        │         │ abstractions │                        │
 │  └──────────────┘         └──────────────┘                        │
 │                                                                    │
 │  MVP = Phases 1-2 = Stage 1 (Adventure) + early Stage 2 (Modeling)│
 │                                                                    │
 └────────────────────────────────────────────────────────────────────┘


 ┌────────────────────────────────────────────────────────────────────┐
 │              FOUR PILLARS → IMPLEMENTATION PHASES                   │
 │                                                                    │
 │  Pillar 0.1: Autonomous bot     ──▶  Phase 4 (Agent) + Phase 5    │
 │  Pillar 0.2: Visualization      ──▶  Phase 1 (MVP core)           │
 │  Pillar 0.3: Complex Plane      ──▶  Phase 2 (Signal View)        │
 │  Pillar 0.4: Math Formulation   ──▶  Phase 5+ (Learning/RL)       │
 │                                                                    │
 └────────────────────────────────────────────────────────────────────┘
```
