//! Replay trace events with original timing, supporting playback controls.
//!
//! The replayer yields batches of events at their original relative timing
//! (adjusted by speed multiplier). Events within 16ms of each other are
//! batched together for 60fps rendering.

use std::collections::HashSet;
use std::sync::Arc;

use hackbot_types::TraceEvent;
use tokio::sync::Notify;

/// 16ms batch window = one frame at 60fps
const BATCH_WINDOW_NS: u64 = 16_000_000;

pub struct TraceReplayer {
    events: Arc<Vec<TraceEvent>>,
    position: usize,
    speed: f64,
    playing: bool,
    start_ts: u64,
    end_ts: u64,

    /// Notified when play() is called
    play_notify: Arc<Notify>,
    /// Notified when seek() is called to interrupt sleep
    seek_notify: Arc<Notify>,

    // Filters
    filter_pids: Option<HashSet<u32>>,
    filter_types: Option<HashSet<String>>,
}

impl TraceReplayer {
    pub fn new(events: Arc<Vec<TraceEvent>>) -> Self {
        let (start_ts, end_ts) = if events.is_empty() {
            (0, 0)
        } else {
            (events.first().unwrap().ts, events.last().unwrap().ts)
        };

        Self {
            events,
            position: 0,
            speed: 1.0,
            playing: false,
            start_ts,
            end_ts,
            play_notify: Arc::new(Notify::new()),
            seek_notify: Arc::new(Notify::new()),
            filter_pids: None,
            filter_types: None,
        }
    }

    pub fn start_ns(&self) -> u64 {
        self.start_ts
    }

    pub fn position_ns(&self) -> u64 {
        if self.events.is_empty() || self.position >= self.events.len() {
            self.end_ts
        } else {
            self.events[self.position].ts
        }
    }

    pub fn elapsed_ns(&self) -> u64 {
        self.position_ns().saturating_sub(self.start_ts)
    }

    pub fn duration_ns(&self) -> u64 {
        self.end_ts.saturating_sub(self.start_ts)
    }

    pub fn speed(&self) -> f64 {
        self.speed
    }

    pub fn is_playing(&self) -> bool {
        self.playing
    }

    pub fn status(&self) -> &str {
        if self.position >= self.events.len() {
            "stopped"
        } else if self.playing {
            "playing"
        } else {
            "paused"
        }
    }

    pub fn play(&mut self) {
        self.playing = true;
        self.play_notify.notify_one();
    }

    pub fn pause(&mut self) {
        self.playing = false;
    }

    pub fn set_speed(&mut self, multiplier: f64) {
        self.speed = multiplier.clamp(0.1, 100.0);
    }

    /// Seek to an absolute timestamp position.
    pub fn seek(&mut self, position_ns: u64) {
        let target_ts = position_ns.clamp(self.start_ts, self.end_ts);

        // Binary search for the position
        let pos = self.events.partition_point(|e| e.ts < target_ts);
        self.position = pos;
        self.seek_notify.notify_one();
    }

    pub fn set_filter(&mut self, pids: Option<Vec<u32>>, types: Option<Vec<String>>) {
        self.filter_pids = pids.map(|p| p.into_iter().collect());
        self.filter_types = types.map(|t| t.into_iter().collect());
    }

    pub fn reset(&mut self) {
        self.position = 0;
        self.playing = false;
    }

    fn passes_filter(&self, event: &TraceEvent) -> bool {
        if let Some(ref pids) = self.filter_pids {
            if !pids.contains(&event.pid) {
                return false;
            }
        }
        if let Some(ref types) = self.filter_types {
            let type_str = serde_json::to_value(&event.event_type)
                .ok()
                .and_then(|v| v.as_str().map(String::from))
                .unwrap_or_default();
            if !types.contains(&type_str) {
                return false;
            }
        }
        true
    }

    /// Yield the next batch of events, respecting timing and filters.
    ///
    /// Returns `None` when playback is complete.
    /// Blocks (async) when paused, waiting for play().
    pub async fn next_batch(&mut self) -> Option<Vec<TraceEvent>> {
        loop {
            if self.position >= self.events.len() {
                return None;
            }

            // Wait for play state
            if !self.playing {
                self.play_notify.notified().await;
                // Re-check after waking
                if self.position >= self.events.len() {
                    return None;
                }
            }

            // Collect a batch of events within BATCH_WINDOW_NS
            let batch_start_ts = self.events[self.position].ts;
            let wall_start = tokio::time::Instant::now();
            let mut batch = Vec::new();

            while self.position < self.events.len() {
                let event = &self.events[self.position];
                if event.ts - batch_start_ts > BATCH_WINDOW_NS {
                    break;
                }
                self.position += 1;
                if self.passes_filter(event) {
                    batch.push(event.clone());
                }
            }

            // Calculate delay until next batch
            if self.position < self.events.len() {
                let next_ts = self.events[self.position].ts;
                let delay_ns = next_ts.saturating_sub(batch_start_ts).saturating_sub(BATCH_WINDOW_NS);
                if delay_ns > 0 {
                    let delay_s = (delay_ns as f64 / 1e9) / self.speed;
                    let elapsed = wall_start.elapsed().as_secs_f64();
                    let actual_delay = (delay_s - elapsed).max(0.0);
                    if actual_delay > 0.0 {
                        let sleep = tokio::time::sleep(tokio::time::Duration::from_secs_f64(actual_delay));
                        let seek = self.seek_notify.notified();
                        tokio::select! {
                            _ = sleep => {}
                            _ = seek => {} // Seek interrupted the sleep
                        }

                        // Check if we were paused during sleep
                        if !self.playing {
                            if !batch.is_empty() {
                                return Some(batch);
                            }
                            continue;
                        }
                    }
                }
            }

            if !batch.is_empty() {
                return Some(batch);
            }
        }
    }
}
