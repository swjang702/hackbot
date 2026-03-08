//! Load and validate .jsonl trace files into TraceEvent objects.

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use hackbot_types::{TraceEvent, validate_payload};

#[derive(Debug, thiserror::Error)]
pub enum TraceLoadError {
    #[error("trace file not found: {0}")]
    NotFound(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// Load all events from a .jsonl trace file, sorted by timestamp.
pub fn load_trace(path: &Path) -> Result<Vec<TraceEvent>, TraceLoadError> {
    if !path.exists() {
        return Err(TraceLoadError::NotFound(path.display().to_string()));
    }

    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let mut events = Vec::new();

    for (line_num, line) in reader.lines().enumerate() {
        let line = line?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let event: TraceEvent = match serde_json::from_str(line) {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!("{}:{}: invalid event: {}", path.display(), line_num + 1, e);
                continue;
            }
        };

        if !validate_payload(event.event_type, &event.payload) {
            tracing::warn!(
                "{}:{}: invalid payload for type {:?}",
                path.display(),
                line_num + 1,
                event.event_type
            );
            continue;
        }

        events.push(event);
    }

    events.sort_by_key(|e| e.ts);
    Ok(events)
}

/// Return summary information about a loaded trace.
pub fn get_trace_info(events: &[TraceEvent]) -> serde_json::Value {
    if events.is_empty() {
        return serde_json::json!({
            "event_count": 0,
            "duration_ns": "0",
            "pids": [],
            "event_types": [],
        });
    }

    let mut pids: Vec<u32> = events.iter().map(|e| e.pid).collect::<std::collections::BTreeSet<_>>().into_iter().collect();
    pids.sort();

    let mut event_types: Vec<String> = events
        .iter()
        .map(|e| serde_json::to_value(&e.event_type).unwrap().as_str().unwrap().to_string())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    event_types.sort();

    let duration_ns = events.last().unwrap().ts - events.first().unwrap().ts;

    serde_json::json!({
        "event_count": events.len(),
        "start_ns": events.first().unwrap().ts.to_string(),
        "end_ns": events.last().unwrap().ts.to_string(),
        "duration_ns": duration_ns.to_string(),
        "duration_s": duration_ns as f64 / 1e9,
        "pids": pids,
        "event_types": event_types,
    })
}
