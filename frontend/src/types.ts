/**
 * TypeScript types mirroring server/server/schemas.py.
 *
 * Timestamps are transmitted as strings in JSON to preserve
 * nanosecond precision (JS Number max safe int is 2^53).
 */

// ---------------------------------------------------------------------------
// Trace events
// ---------------------------------------------------------------------------

export type EventType =
  | "syscall_enter"
  | "syscall_exit"
  | "sched_switch"
  | "power_trace"
  | "process_fork"
  | "process_exit"
  | "gpu_submit"
  | "gpu_complete";

export interface TraceEvent {
  ts: string; // nanoseconds as string (BigInt-safe)
  type: EventType;
  pid: number;
  tid: number;
  cpu: number;
  comm: string;
  payload: Record<string, unknown>;
}

// ---------------------------------------------------------------------------
// World state
// ---------------------------------------------------------------------------

export type ProcessStatus = "running" | "sleeping" | "exited";

export interface ProcessInfo {
  pid: number;
  comm: string;
  parent_pid: number | null;
  status: ProcessStatus;
  syscall_count: number;
  gpu_submit_count: number;
  last_event_ts: number;
}

export interface ConnectionInfo {
  from_pid: number;
  to_pid: number;
  type: string;
  fd_from: number | null;
  fd_to: number | null;
}

// ---------------------------------------------------------------------------
// Server -> Client messages
// ---------------------------------------------------------------------------

export interface WorldStateMessage {
  msg: "world_state";
  processes: ProcessInfo[];
  connections: ConnectionInfo[];
}

export interface EventsMessage {
  msg: "events";
  batch: TraceEvent[];
}

export interface PlaybackMessage {
  msg: "playback";
  status: "playing" | "paused" | "stopped";
  speed: number;
  position_ns: string; // elapsed from trace start (relative)
  duration_ns: string;
  start_ns: string; // absolute trace start timestamp
}

export type ServerMessage = WorldStateMessage | EventsMessage | PlaybackMessage;

// ---------------------------------------------------------------------------
// Client -> Server commands
// ---------------------------------------------------------------------------

export type ClientCommand =
  | { cmd: "load"; file: string }
  | { cmd: "play" }
  | { cmd: "pause" }
  | { cmd: "seek"; position_ns: string }
  | { cmd: "speed"; multiplier: number }
  | { cmd: "filter"; pids?: number[]; types?: string[] };
