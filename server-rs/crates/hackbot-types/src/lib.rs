//! Shared types for hackbot — trace events, world state, and WebSocket messages.
//!
//! All timestamps are nanoseconds since epoch (u64). They are serialized
//! as strings in JSON to avoid JavaScript Number precision loss (max safe
//! integer is 2^53, while nanosecond timestamps are ~1.7e18).

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Event types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventType {
    SyscallEnter,
    SyscallExit,
    SchedSwitch,
    PowerTrace,
    ProcessFork,
    ProcessExit,
    GpuSubmit,
    GpuComplete,
}

// ---------------------------------------------------------------------------
// Trace event (core data type)
// ---------------------------------------------------------------------------

/// A single trace event from an eBPF probe.
///
/// The `payload` field is a flat JSON object whose keys depend on `event_type`.
/// We keep it as `serde_json::Value` for flexibility; validation happens at load time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceEvent {
    pub ts: u64,
    #[serde(rename = "type")]
    pub event_type: EventType,
    pub pid: u32,
    pub tid: u32,
    pub cpu: u16,
    pub comm: String,
    pub payload: serde_json::Value,
}

impl TraceEvent {
    /// Serialize for WebSocket, with `ts` as a string for JS BigInt safety.
    pub fn to_ws_value(&self) -> serde_json::Value {
        serde_json::json!({
            "ts": self.ts.to_string(),
            "type": self.event_type,
            "pid": self.pid,
            "tid": self.tid,
            "cpu": self.cpu,
            "comm": self.comm,
            "payload": self.payload,
        })
    }
}

// ---------------------------------------------------------------------------
// Payload types (for validation)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyscallEnterPayload {
    pub nr: i64,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fd: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flags: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyscallExitPayload {
    pub nr: i64,
    pub name: String,
    pub ret: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchedSwitchPayload {
    pub prev_pid: u32,
    pub next_pid: u32,
    pub prev_state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PowerTracePayload {
    pub watts: f64,
    pub domain: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessForkPayload {
    pub parent_pid: u32,
    pub child_pid: u32,
    #[serde(default)]
    pub child_comm: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessExitPayload {
    pub exit_code: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuSubmitPayload {
    pub batch_size: u32,
    #[serde(default = "default_queue")]
    pub queue: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuCompletePayload {
    pub batch_size: u32,
    #[serde(default = "default_queue")]
    pub queue: String,
    #[serde(default)]
    pub duration_ns: u64,
}

fn default_queue() -> String {
    "default".to_string()
}

/// Validate that a payload matches the expected schema for its event type.
pub fn validate_payload(event_type: EventType, payload: &serde_json::Value) -> bool {
    match event_type {
        EventType::SyscallEnter => serde_json::from_value::<SyscallEnterPayload>(payload.clone()).is_ok(),
        EventType::SyscallExit => serde_json::from_value::<SyscallExitPayload>(payload.clone()).is_ok(),
        EventType::SchedSwitch => serde_json::from_value::<SchedSwitchPayload>(payload.clone()).is_ok(),
        EventType::PowerTrace => serde_json::from_value::<PowerTracePayload>(payload.clone()).is_ok(),
        EventType::ProcessFork => serde_json::from_value::<ProcessForkPayload>(payload.clone()).is_ok(),
        EventType::ProcessExit => serde_json::from_value::<ProcessExitPayload>(payload.clone()).is_ok(),
        EventType::GpuSubmit => serde_json::from_value::<GpuSubmitPayload>(payload.clone()).is_ok(),
        EventType::GpuComplete => serde_json::from_value::<GpuCompletePayload>(payload.clone()).is_ok(),
    }
}

// ---------------------------------------------------------------------------
// World state models
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessStatus {
    Running,
    Sleeping,
    Exited,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessInfo {
    pub pid: u32,
    pub comm: String,
    pub parent_pid: Option<u32>,
    pub status: ProcessStatus,
    pub syscall_count: u64,
    pub gpu_submit_count: u64,
    pub last_event_ts: u64,
}

impl ProcessInfo {
    pub fn new(pid: u32, comm: String) -> Self {
        Self {
            pid,
            comm,
            parent_pid: None,
            status: ProcessStatus::Running,
            syscall_count: 0,
            gpu_submit_count: 0,
            last_event_ts: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionInfo {
    pub from_pid: u32,
    pub to_pid: u32,
    #[serde(rename = "type")]
    pub conn_type: String,
    pub fd_from: Option<i32>,
    pub fd_to: Option<i32>,
}

// ---------------------------------------------------------------------------
// WebSocket messages: Server -> Client
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "msg", rename_all = "snake_case")]
pub enum ServerMessage {
    WorldState {
        processes: Vec<serde_json::Value>,
        connections: Vec<serde_json::Value>,
    },
    Events {
        batch: Vec<serde_json::Value>,
    },
    Playback {
        status: String,
        speed: f64,
        position_ns: String,
        duration_ns: String,
        start_ns: String,
    },
}

// ---------------------------------------------------------------------------
// WebSocket messages: Client -> Server
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum ClientCommand {
    Load { file: String },
    Play,
    Pause,
    Seek { position_ns: String },
    Speed { multiplier: f64 },
    Filter {
        pids: Option<Vec<u32>>,
        types: Option<Vec<String>>,
    },
}
