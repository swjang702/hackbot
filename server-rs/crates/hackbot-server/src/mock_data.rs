//! Generate realistic mock eBPF trace data for an LLM inference workload.
//!
//! The generated trace tells a story across ~5 seconds:
//!
//!   0.0–1.0s  STARTUP    — Parent python3 forks 4 workers, open/mmap syscalls
//!   1.0–3.0s  PREFILL    — Large GPU batches, high read/write, power spikes
//!   3.0–4.0s  DECODE     — Small regular GPU submits, periodic writes, moderate power
//!   4.0–4.5s  ANOMALY    — Suspicious process probes /proc/maps, reads shared memory
//!   4.5–5.0s  RECOVERY   — Normal operation resumes

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use serde_json::json;
use std::fs;
use std::path::Path;

/// Base timestamp: 2024-03-02 12:00:00.000000000 UTC
const BASE_TS: u64 = 1_709_380_800_000_000_000;

const PARENT_PID: u32 = 100;
const WORKER_PIDS: [u32; 4] = [101, 102, 103, 104];
const ANOMALY_PID: u32 = 200;
const NUM_CPUS: u16 = 8;

fn ns(seconds: f64) -> u64 {
    BASE_TS + (seconds * 1_000_000_000.0) as u64
}

fn jitter(rng: &mut StdRng, base_ns: u64, max_us: u64) -> u64 {
    base_ns + rng.gen_range(0..max_us * 1000)
}

fn make_event(
    rng: &mut StdRng,
    ts: u64,
    event_type: &str,
    pid: u32,
    comm: &str,
    payload: serde_json::Value,
    cpu: Option<u16>,
) -> serde_json::Value {
    json!({
        "ts": ts,
        "type": event_type,
        "pid": pid,
        "tid": pid,
        "cpu": cpu.unwrap_or_else(|| rng.gen_range(0..NUM_CPUS)),
        "comm": comm,
        "payload": payload,
    })
}

fn syscall_pair(
    rng: &mut StdRng,
    ts: u64,
    pid: u32,
    comm: &str,
    name: &str,
    nr: i64,
    ret: i64,
    duration_us: u64,
    extra: serde_json::Value,
) -> Vec<serde_json::Value> {
    let cpu = rng.gen_range(0..NUM_CPUS);
    let mut enter_payload = json!({"nr": nr, "name": name});
    if let serde_json::Value::Object(map) = extra {
        for (k, v) in map {
            enter_payload[&k] = v;
        }
    }
    let exit_ts = ts + duration_us * 1000;
    vec![
        json!({
            "ts": ts, "type": "syscall_enter", "pid": pid, "tid": pid,
            "cpu": cpu, "comm": comm, "payload": enter_payload,
        }),
        json!({
            "ts": exit_ts, "type": "syscall_exit", "pid": pid, "tid": pid,
            "cpu": cpu, "comm": comm, "payload": {"nr": nr, "name": name, "ret": ret},
        }),
    ]
}

fn generate_startup(rng: &mut StdRng, events: &mut Vec<serde_json::Value>) {
    let t = 0.0;

    // Parent opens config files
    let files = ["/etc/llm/config.yaml", "/dev/nvidia0", "/dev/shm/model_weights"];
    for (i, fname) in files.iter().enumerate() {
        events.extend(syscall_pair(
            rng, ns(t + 0.01 * i as f64), PARENT_PID, "python3",
            "open", 2, 3 + i as i64, 50,
            json!({"path": fname, "flags": 0}),
        ));
    }

    // mmap model weights
    for i in 0..6 {
        events.extend(syscall_pair(
            rng, ns(t + 0.05 + 0.01 * i as f64), PARENT_PID, "python3",
            "mmap", 9, 0x7f0000000000_i64 + i * 0x10000000, 200,
            json!({"count": 256 * 1024 * 1024}),
        ));
    }

    // Fork 4 workers
    for (i, &wpid) in WORKER_PIDS.iter().enumerate() {
        let fork_ts = ns(t + 0.2 + 0.1 * i as f64);
        events.push(make_event(
            rng, fork_ts, "process_fork", PARENT_PID, "python3",
            json!({"parent_pid": PARENT_PID, "child_pid": wpid, "child_comm": format!("worker{i}")}),
            None,
        ));
        // Worker initial setup
        let worker_files = ["/dev/nvidia0", "/dev/shm/model_weights"];
        for (j, fname) in worker_files.iter().enumerate() {
            let setup_ts = jitter(rng, fork_ts + 5_000_000 + j as u64 * 2_000_000, 100);
            events.extend(syscall_pair(
                rng, setup_ts, wpid, &format!("worker{i}"),
                "open", 2, 3 + j as i64, 30,
                json!({"path": fname}),
            ));
        }
    }

    // Scheduling events during startup
    for i in 0..20u32 {
        let t_sched = ns(t + 0.05 * i as f64);
        let available = std::cmp::min(i as usize / 4 + 1, 4);
        let mut candidates = vec![PARENT_PID];
        candidates.extend_from_slice(&WORKER_PIDS[..available.min(WORKER_PIDS.len())]);
        let pid_from = candidates[rng.gen_range(0..candidates.len())];
        let pid_to = candidates[rng.gen_range(0..candidates.len())];
        if pid_from != pid_to {
            let comm = if pid_from == PARENT_PID {
                "python3".to_string()
            } else {
                format!("worker{}", WORKER_PIDS.iter().position(|&p| p == pid_from).unwrap_or(0))
            };
            events.push(make_event(
                rng, t_sched, "sched_switch", pid_from, &comm,
                json!({"prev_pid": pid_from, "next_pid": pid_to, "prev_state": "S"}),
                None,
            ));
        }
    }

    // Low baseline power
    for i in 0..10 {
        let watts = 45.0 + rng.gen_range(-2.0..2.0);
        events.push(make_event(
            rng, ns(t + 0.1 * i as f64), "power_trace", 0, "kernel",
            json!({"watts": watts, "domain": "package-0"}),
            Some(0),
        ));
    }
}

fn generate_prefill(rng: &mut StdRng, events: &mut Vec<serde_json::Value>) {
    let mut t = 1.0;
    while t < 3.0 {
        for (i, &wpid) in WORKER_PIDS.iter().enumerate() {
            let comm = format!("worker{i}");
            let gpu_ts = ns(t + 0.005 * i as f64);
            let batch_size: u32 = rng.gen_range(512..=2048);

            events.push(make_event(
                rng, gpu_ts, "gpu_submit", wpid, &comm,
                json!({"batch_size": batch_size, "queue": format!("compute-{}", i % 2)}),
                None,
            ));

            let complete_offset: u64 = rng.gen_range(5_000_000..=15_000_000);
            let complete_ts = gpu_ts + complete_offset;
            events.push(make_event(
                rng, complete_ts, "gpu_complete", wpid, &comm,
                json!({"batch_size": batch_size, "queue": format!("compute-{}", i % 2), "duration_ns": complete_offset}),
                None,
            ));

            let num_rw: u32 = rng.gen_range(8..=16);
            for j in 0..num_rw {
                let rw_ts = jitter(rng, gpu_ts + j as u64 * 500_000, 100);
                let (name, nr) = if rng.gen_bool(0.5) { ("read", 0) } else { ("write", 1) };
                let fds = [3_i64, 4, 5];
                let fd = fds[rng.gen_range(0..fds.len())];
                let count: i64 = rng.gen_range(4096..=65536);
                let dur: u64 = rng.gen_range(5..=50);
                events.extend(syscall_pair(
                    rng, rw_ts, wpid, &comm, name, nr, count, dur,
                    json!({"fd": fd, "count": count}),
                ));
            }
        }

        // High power
        let watts = 280.0 + rng.gen_range(-20.0..40.0);
        events.push(make_event(
            rng, ns(t), "power_trace", 0, "kernel",
            json!({"watts": watts, "domain": "package-0"}),
            Some(0),
        ));

        // Scheduling
        let num_sched: u32 = rng.gen_range(4..=8);
        for _ in 0..num_sched {
            let s_ts = jitter(rng, ns(t), 20000);
            let mut pool = vec![PARENT_PID];
            pool.extend_from_slice(&WORKER_PIDS);
            let idx1 = rng.gen_range(0..pool.len());
            let mut idx2 = rng.gen_range(0..pool.len());
            while idx2 == idx1 {
                idx2 = rng.gen_range(0..pool.len());
            }
            let pid_from = pool[idx1];
            let pid_to = pool[idx2];
            let comm = if pid_from == PARENT_PID {
                "python3".to_string()
            } else {
                format!("worker{}", WORKER_PIDS.iter().position(|&p| p == pid_from).unwrap_or(0))
            };
            events.push(make_event(
                rng, s_ts, "sched_switch", pid_from, &comm,
                json!({"prev_pid": pid_from, "next_pid": pid_to, "prev_state": "R"}),
                None,
            ));
        }

        t += rng.gen_range(0.02..0.04);
    }
}

fn generate_decode(rng: &mut StdRng, events: &mut Vec<serde_json::Value>) {
    let mut t = 3.0;
    let mut token_id = 0u32;
    while t < 4.0 {
        let worker_idx = (token_id % WORKER_PIDS.len() as u32) as usize;
        let wpid = WORKER_PIDS[worker_idx];
        let comm = format!("worker{worker_idx}");

        let gpu_ts = ns(t);
        let batch_size: u32 = rng.gen_range(1..=4);
        events.push(make_event(
            rng, gpu_ts, "gpu_submit", wpid, &comm,
            json!({"batch_size": batch_size, "queue": "compute-0"}),
            None,
        ));
        let complete_offset: u64 = rng.gen_range(1_000_000..=3_000_000);
        let complete_ts = gpu_ts + complete_offset;
        events.push(make_event(
            rng, complete_ts, "gpu_complete", wpid, &comm,
            json!({"batch_size": batch_size, "queue": "compute-0", "duration_ns": complete_offset}),
            None,
        ));

        let write_ts = jitter(rng, gpu_ts + 1_000_000, 100);
        events.extend(syscall_pair(
            rng, write_ts, wpid, &comm,
            "write", 1, 64, 5,
            json!({"fd": 6, "count": 64}),
        ));

        if rng.gen_bool(0.3) {
            let read_ts = jitter(rng, gpu_ts + 2_000_000, 100);
            events.extend(syscall_pair(
                rng, read_ts, wpid, &comm,
                "read", 0, 4096, 8,
                json!({"fd": 5, "count": 4096}),
            ));
        }

        if token_id % 5 == 0 {
            let watts = 150.0 + rng.gen_range(-10.0..15.0);
            events.push(make_event(
                rng, ns(t), "power_trace", 0, "kernel",
                json!({"watts": watts, "domain": "package-0"}),
                Some(0),
            ));
        }

        if rng.gen_bool(0.4) {
            let mut pool = vec![PARENT_PID];
            pool.extend_from_slice(&WORKER_PIDS);
            let idx1 = rng.gen_range(0..pool.len());
            let mut idx2 = rng.gen_range(0..pool.len());
            while idx2 == idx1 {
                idx2 = rng.gen_range(0..pool.len());
            }
            let pid_from = pool[idx1];
            let pid_to = pool[idx2];
            let comm_s = if pid_from == PARENT_PID {
                "python3".to_string()
            } else {
                format!("worker{}", WORKER_PIDS.iter().position(|&p| p == pid_from).unwrap_or(0))
            };
            let sched_ts = jitter(rng, ns(t), 5000);
            events.push(make_event(
                rng, sched_ts, "sched_switch", pid_from, &comm_s,
                json!({"prev_pid": pid_from, "next_pid": pid_to, "prev_state": "S"}),
                None,
            ));
        }

        t += rng.gen_range(0.008..0.015);
        token_id += 1;
    }
}

fn generate_anomaly(rng: &mut StdRng, events: &mut Vec<serde_json::Value>) {
    let t_start = 4.0;

    // Anomaly process appears
    events.push(make_event(
        rng, ns(t_start), "process_fork", 1, "init",
        json!({"parent_pid": 1, "child_pid": ANOMALY_PID, "child_comm": "probe_tool"}),
        None,
    ));

    // Probe /proc/[worker_pid]/maps
    let mut t = t_start + 0.01;
    for (i, &wpid) in WORKER_PIDS.iter().enumerate() {
        let probe_ts = ns(t + 0.02 * i as f64);
        events.extend(syscall_pair(
            rng, probe_ts, ANOMALY_PID, "probe_tool",
            "open", 2, 10 + i as i64, 20,
            json!({"path": format!("/proc/{wpid}/maps")}),
        ));
        let num_reads: u32 = rng.gen_range(5..=15);
        for j in 0..num_reads {
            let read_ts = jitter(rng, probe_ts + (j as u64 + 1) * 500_000, 100);
            let dur: u64 = rng.gen_range(3..=15);
            events.extend(syscall_pair(
                rng, read_ts, ANOMALY_PID, "probe_tool",
                "read", 0, 4096, dur,
                json!({"fd": 10 + i as i64, "count": 4096}),
            ));
        }
    }

    // High-frequency reads on shared memory
    t = t_start + 0.15;
    for i in 0..80 {
        let shm_ts = ns(t + 0.002 * i as f64);
        let dur: u64 = rng.gen_range(2..=8);
        events.extend(syscall_pair(
            rng, shm_ts, ANOMALY_PID, "probe_tool",
            "read", 0, 4096, dur,
            json!({"fd": 20, "count": 4096}),
        ));
    }

    // perf_event_open calls
    t = t_start + 0.35;
    for i in 0..5 {
        let perf_ts = ns(t + 0.01 * i as f64);
        events.extend(syscall_pair(
            rng, perf_ts, ANOMALY_PID, "probe_tool",
            "perf_event_open", 298, 30 + i, 100,
            json!({}),
        ));
    }

    // Power spikes during anomaly
    for i in 0..10 {
        let watts = 200.0 + rng.gen_range(50.0..100.0);
        events.push(make_event(
            rng, ns(t_start + 0.05 * i as f64), "power_trace", 0, "kernel",
            json!({"watts": watts, "domain": "package-0"}),
            Some(0),
        ));
    }

    // Normal workers still running during anomaly
    let t = t_start;
    for tick in 0..15 {
        let tick_t = t + 0.03 * tick as f64;
        if tick_t >= 4.5 {
            break;
        }
        let idx = tick % WORKER_PIDS.len();
        let wpid = WORKER_PIDS[idx];
        let comm = format!("worker{idx}");
        let batch_size: u32 = rng.gen_range(1..=4);
        events.push(make_event(
            rng, ns(tick_t), "gpu_submit", wpid, &comm,
            json!({"batch_size": batch_size, "queue": "compute-0"}),
            None,
        ));
        let write_ts = jitter(rng, ns(tick_t + 0.005), 100);
        events.extend(syscall_pair(
            rng, write_ts, wpid, &comm,
            "write", 1, 64, 5,
            json!({"fd": 6, "count": 64}),
        ));
    }
}

fn generate_recovery(rng: &mut StdRng, events: &mut Vec<serde_json::Value>) {
    let t_start = 4.5;

    events.push(make_event(
        rng, ns(t_start), "process_exit", ANOMALY_PID, "probe_tool",
        json!({"exit_code": 0}),
        None,
    ));

    let mut t = t_start + 0.01;
    let mut token_id = 0u32;
    while t < 5.0 {
        let worker_idx = (token_id % WORKER_PIDS.len() as u32) as usize;
        let wpid = WORKER_PIDS[worker_idx];
        let comm = format!("worker{worker_idx}");

        let gpu_ts = ns(t);
        let batch_size: u32 = rng.gen_range(1..=4);
        events.push(make_event(
            rng, gpu_ts, "gpu_submit", wpid, &comm,
            json!({"batch_size": batch_size, "queue": "compute-0"}),
            None,
        ));
        let complete_offset: u64 = rng.gen_range(1_000_000..=3_000_000);
        let complete_ts = gpu_ts + complete_offset;
        events.push(make_event(
            rng, complete_ts, "gpu_complete", wpid, &comm,
            json!({"batch_size": batch_size, "queue": "compute-0", "duration_ns": complete_offset}),
            None,
        ));

        let write_ts2 = jitter(rng, gpu_ts + 1_000_000, 100);
        events.extend(syscall_pair(
            rng, write_ts2, wpid, &comm,
            "write", 1, 64, 5,
            json!({"fd": 6, "count": 64}),
        ));

        if token_id % 5 == 0 {
            let watts = 140.0 + rng.gen_range(-10.0..10.0);
            events.push(make_event(
                rng, ns(t), "power_trace", 0, "kernel",
                json!({"watts": watts, "domain": "package-0"}),
                Some(0),
            ));
        }

        t += rng.gen_range(0.008..0.015);
        token_id += 1;
    }
}

pub fn generate_trace() -> Vec<serde_json::Value> {
    let mut rng = StdRng::seed_from_u64(42);
    let mut events = Vec::new();

    generate_startup(&mut rng, &mut events);
    generate_prefill(&mut rng, &mut events);
    generate_decode(&mut rng, &mut events);
    generate_anomaly(&mut rng, &mut events);
    generate_recovery(&mut rng, &mut events);

    events.sort_by_key(|e| e["ts"].as_u64().unwrap_or(0));
    events
}

/// Write mock trace data to disk.
pub fn write_mock_trace(traces_dir: &Path) -> std::io::Result<usize> {
    fs::create_dir_all(traces_dir)?;
    let output_path = traces_dir.join("sample-llm-workload.jsonl");

    let events = generate_trace();
    let mut output = String::new();
    for event in &events {
        output.push_str(&serde_json::to_string(event).unwrap());
        output.push('\n');
    }
    fs::write(&output_path, output)?;

    tracing::info!("Generated {} events -> {}", events.len(), output_path.display());
    Ok(events.len())
}

