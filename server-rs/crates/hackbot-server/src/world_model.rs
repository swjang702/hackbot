//! Maintain world state from trace events.
//!
//! Processes events to build and update the world model: a map of active
//! processes, their file descriptors, and inter-process connections.

use std::collections::HashMap;

use hackbot_types::{ConnectionInfo, EventType, ProcessInfo, ProcessStatus, TraceEvent};

pub struct WorldModel {
    processes: HashMap<u32, ProcessInfo>,
    connections: Vec<ConnectionInfo>,
    /// fd table: (pid, fd) -> type info
    fd_table: HashMap<(u32, i64), FdEntry>,
}

#[derive(Debug, Clone)]
struct FdEntry {
    fd_type: String,
    _path: Option<String>,
}

impl WorldModel {
    pub fn new() -> Self {
        Self {
            processes: HashMap::new(),
            connections: Vec::new(),
            fd_table: HashMap::new(),
        }
    }

    pub fn reset(&mut self) {
        self.processes.clear();
        self.connections.clear();
        self.fd_table.clear();
    }

    fn ensure_process(&mut self, pid: u32, comm: &str) -> &mut ProcessInfo {
        self.processes
            .entry(pid)
            .or_insert_with(|| ProcessInfo::new(pid, comm.to_string()))
    }

    pub fn process_event(&mut self, event: &TraceEvent) {
        match event.event_type {
            EventType::SyscallEnter => self.handle_syscall_enter(event),
            EventType::SyscallExit => self.handle_syscall_exit(event),
            EventType::SchedSwitch => self.handle_sched_switch(event),
            EventType::ProcessFork => self.handle_process_fork(event),
            EventType::ProcessExit => self.handle_process_exit(event),
            EventType::GpuSubmit => self.handle_gpu_submit(event),
            EventType::GpuComplete => self.handle_gpu_complete(event),
            EventType::PowerTrace => {} // Power events don't modify process state
        }
    }

    pub fn process_events(&mut self, events: &[TraceEvent]) {
        for event in events {
            self.process_event(event);
        }
    }

    pub fn get_world_state_dict(&self) -> serde_json::Value {
        let processes: Vec<serde_json::Value> = self
            .processes
            .values()
            .map(|p| serde_json::to_value(p).unwrap())
            .collect();
        let connections: Vec<serde_json::Value> = self
            .connections
            .iter()
            .map(|c| serde_json::to_value(c).unwrap())
            .collect();
        serde_json::json!({
            "msg": "world_state",
            "processes": processes,
            "connections": connections,
        })
    }

    /// Reset and rebuild state from events up to a given timestamp.
    pub fn rebuild_to(&mut self, events: &[TraceEvent], up_to_ts: u64) {
        self.reset();
        for event in events {
            if event.ts > up_to_ts {
                break;
            }
            self.process_event(event);
        }
    }

    // -----------------------------------------------------------------------
    // Event handlers
    // -----------------------------------------------------------------------

    fn handle_syscall_enter(&mut self, event: &TraceEvent) {
        let proc = self.ensure_process(event.pid, &event.comm);
        proc.syscall_count += 1;
        proc.last_event_ts = event.ts;
        proc.status = ProcessStatus::Running;
    }

    fn handle_syscall_exit(&mut self, event: &TraceEvent) {
        let proc = self.ensure_process(event.pid, &event.comm);
        proc.last_event_ts = event.ts;

        let name = event.payload.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let ret = event.payload.get("ret").and_then(|v| v.as_i64()).unwrap_or(-1);

        // Record fd from successful open
        if name == "open" && ret >= 0 {
            self.fd_table.insert(
                (event.pid, ret),
                FdEntry {
                    fd_type: "file".to_string(),
                    _path: None,
                },
            );
        }
    }

    fn handle_sched_switch(&mut self, event: &TraceEvent) {
        let prev_pid = event.payload.get("prev_pid").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
        let next_pid = event.payload.get("next_pid").and_then(|v| v.as_u64()).unwrap_or(0) as u32;

        if let Some(proc) = self.processes.get_mut(&prev_pid) {
            proc.status = ProcessStatus::Sleeping;
            proc.last_event_ts = event.ts;
        }
        if let Some(proc) = self.processes.get_mut(&next_pid) {
            proc.status = ProcessStatus::Running;
            proc.last_event_ts = event.ts;
        }
    }

    fn handle_process_fork(&mut self, event: &TraceEvent) {
        let parent_pid = event.payload.get("parent_pid").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
        let child_pid = event.payload.get("child_pid").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
        let child_comm = event
            .payload
            .get("child_comm")
            .and_then(|v| v.as_str())
            .unwrap_or(&event.comm)
            .to_string();

        self.ensure_process(parent_pid, &event.comm);

        let child = self.ensure_process(child_pid, &child_comm);
        child.parent_pid = Some(parent_pid);
        child.comm = child_comm;
        child.status = ProcessStatus::Running;

        self.connections.push(ConnectionInfo {
            from_pid: parent_pid,
            to_pid: child_pid,
            conn_type: "fork".to_string(),
            fd_from: None,
            fd_to: None,
        });
    }

    fn handle_process_exit(&mut self, event: &TraceEvent) {
        if let Some(proc) = self.processes.get_mut(&event.pid) {
            proc.status = ProcessStatus::Exited;
            proc.last_event_ts = event.ts;
        }
    }

    fn handle_gpu_submit(&mut self, event: &TraceEvent) {
        let proc = self.ensure_process(event.pid, &event.comm);
        proc.gpu_submit_count += 1;
        proc.last_event_ts = event.ts;
        proc.status = ProcessStatus::Running;
    }

    fn handle_gpu_complete(&mut self, event: &TraceEvent) {
        let proc = self.ensure_process(event.pid, &event.comm);
        proc.last_event_ts = event.ts;
    }
}
