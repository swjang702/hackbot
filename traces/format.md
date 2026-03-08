# Trace Format Specification

hackbot trace files use **JSON Lines** (`.jsonl`) format: one JSON object per line, each representing a single eBPF trace event.

## Event Structure

Every event has these fields:

| Field | Type | Description |
|-------|------|-------------|
| `ts` | integer | Timestamp in nanoseconds since epoch |
| `type` | string | Event type (discriminates the payload) |
| `pid` | integer | Process ID |
| `tid` | integer | Thread ID |
| `cpu` | integer | CPU core number |
| `comm` | string | Process name (max 16 chars, matching kernel `task_struct.comm`) |
| `payload` | object | Type-specific data (see below) |

## Event Types

### `syscall_enter`

Fired when a process enters a system call.

```json
{ "nr": 1, "name": "write", "fd": 3, "count": 4096 }
```

Optional payload fields: `fd`, `count`, `path`, `flags`.

### `syscall_exit`

Fired when a system call returns.

```json
{ "nr": 1, "name": "write", "ret": 4096 }
```

### `sched_switch`

Context switch between processes.

```json
{ "prev_pid": 100, "next_pid": 101, "prev_state": "S" }
```

`prev_state`: `R` (running), `S` (sleeping), `D` (uninterruptible), `T` (stopped).

### `power_trace`

Power consumption reading from hardware counters.

```json
{ "watts": 280.5, "domain": "package-0" }
```

### `process_fork`

New process created.

```json
{ "parent_pid": 100, "child_pid": 101, "child_comm": "worker0" }
```

### `process_exit`

Process exits.

```json
{ "exit_code": 0 }
```

### `gpu_submit`

GPU work submitted.

```json
{ "batch_size": 512, "queue": "compute-0" }
```

### `gpu_complete`

GPU work completed.

```json
{ "batch_size": 512, "queue": "compute-0", "duration_ns": 10000000 }
```

## Notes

- Timestamps are nanoseconds since epoch (uint64). In JSON they are plain integers. The WebSocket protocol transmits them as strings to avoid JavaScript Number precision loss.
- Events in a file are not guaranteed to be sorted by timestamp. The trace loader sorts them after loading.
- The format is extensible: new event types can be added without breaking existing consumers.
