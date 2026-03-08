//! WebSocket gateway — manages connections and routes messages.
//!
//! On connect: sends the current world state.
//! During playback: sends event batches and periodic world state updates.
//! Handles incoming commands (play/pause/seek/speed/filter/load).

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::{broadcast, Mutex};

use hackbot_types::{ClientCommand, ServerMessage, TraceEvent};

use crate::trace_loader;
use crate::trace_replayer::TraceReplayer;
use crate::world_model::WorldModel;

/// How often to send world_state updates during playback (ms)
const WORLD_STATE_INTERVAL_MS: u64 = 500;

/// Shared gateway state behind Arc<Mutex<...>>
pub struct GatewayState {
    pub traces_dir: PathBuf,
    pub events: Vec<TraceEvent>,
    pub replayer: Option<TraceReplayer>,
    pub world_model: WorldModel,
    pub broadcast_tx: broadcast::Sender<String>,
}

impl GatewayState {
    pub fn new(traces_dir: PathBuf) -> Self {
        let (broadcast_tx, _) = broadcast::channel(256);
        Self {
            traces_dir,
            events: Vec::new(),
            replayer: None,
            world_model: WorldModel::new(),
            broadcast_tx,
        }
    }

    pub fn trace_files(&self) -> Vec<String> {
        if !self.traces_dir.exists() {
            return Vec::new();
        }
        let mut files: Vec<String> = std::fs::read_dir(&self.traces_dir)
            .into_iter()
            .flatten()
            .filter_map(|entry| {
                let entry = entry.ok()?;
                let name = entry.file_name().to_string_lossy().to_string();
                if name.ends_with(".jsonl") { Some(name) } else { None }
            })
            .collect();
        files.sort();
        files
    }

    pub fn load_trace(&mut self, filename: &str) -> Result<serde_json::Value, String> {
        let path = self.traces_dir.join(filename);
        let events = trace_loader::load_trace(&path).map_err(|e| e.to_string())?;

        self.world_model.reset();
        self.world_model.process_events(&events);

        let info = trace_loader::get_trace_info(&events);

        let events = Arc::new(events);
        self.events = (*events).clone();
        self.replayer = Some(TraceReplayer::new(events));

        Ok(info)
    }

    fn playback_status_json(&self) -> Option<String> {
        let replayer = self.replayer.as_ref()?;
        let msg = ServerMessage::Playback {
            status: replayer.status().to_string(),
            speed: replayer.speed(),
            position_ns: replayer.elapsed_ns().to_string(),
            duration_ns: replayer.duration_ns().to_string(),
            start_ns: replayer.start_ns().to_string(),
        };
        serde_json::to_string(&msg).ok()
    }

    fn world_state_json(&self) -> String {
        serde_json::to_string(&self.world_model.get_world_state_dict()).unwrap()
    }
}

pub type SharedGateway = Arc<Mutex<GatewayState>>;

/// Handle a single WebSocket connection.
pub async fn handle_ws(socket: WebSocket, gateway: SharedGateway) {
    let (mut ws_tx, mut ws_rx) = socket.split();

    // Send initial world state
    {
        let state = gateway.lock().await;
        let ws_msg = state.world_state_json();
        let _ = ws_tx.send(Message::Text(ws_msg.into())).await;

        if let Some(playback) = state.playback_status_json() {
            let _ = ws_tx.send(Message::Text(playback.into())).await;
        }
    }

    // Subscribe to broadcasts
    let mut broadcast_rx = {
        let state = gateway.lock().await;
        state.broadcast_tx.subscribe()
    };

    // Spawn a task to forward broadcasts to this client
    let (client_tx, mut client_rx) = tokio::sync::mpsc::channel::<String>(64);
    let broadcast_forward = tokio::spawn(async move {
        while let Ok(msg) = broadcast_rx.recv().await {
            if client_tx.send(msg).await.is_err() {
                break;
            }
        }
    });

    // Two tasks: read commands from client, write broadcasts to client
    let write_task = tokio::spawn(async move {
        while let Some(msg) = client_rx.recv().await {
            if ws_tx.send(Message::Text(msg.into())).await.is_err() {
                break;
            }
        }
    });

    let read_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = ws_rx.next().await {
            match msg {
                Message::Text(text) => {
                    handle_command(&text, &gateway).await;
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
    });

    // Wait for either task to finish
    tokio::select! {
        _ = write_task => {}
        _ = read_task => {}
    }
    broadcast_forward.abort();

    tracing::info!("Client disconnected");
}

async fn handle_command(raw: &str, gateway: &SharedGateway) {
    let cmd: ClientCommand = match serde_json::from_str(raw) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("Invalid command: {}: {}", &raw[..raw.len().min(200)], e);
            return;
        }
    };

    match cmd {
        ClientCommand::Load { file } => {
            let mut state = gateway.lock().await;
            match state.load_trace(&file) {
                Ok(info) => {
                    state.world_model.reset();
                    let ws = state.world_state_json();
                    let _ = state.broadcast_tx.send(ws);
                    if let Some(pb) = state.playback_status_json() {
                        let _ = state.broadcast_tx.send(pb);
                    }
                    tracing::info!("Loaded trace: {} ({} events)", file, info["event_count"]);
                }
                Err(e) => tracing::error!("Failed to load trace {}: {}", file, e),
            }
        }

        ClientCommand::Play => {
            let should_start = {
                let mut state = gateway.lock().await;
                if let Some(ref mut replayer) = state.replayer {
                    replayer.play();
                    if let Some(pb) = state.playback_status_json() {
                        let _ = state.broadcast_tx.send(pb);
                    }
                    true
                } else {
                    false
                }
            };
            if should_start {
                start_playback(gateway.clone()).await;
            }
        }

        ClientCommand::Pause => {
            let mut state = gateway.lock().await;
            if let Some(ref mut replayer) = state.replayer {
                replayer.pause();
                if let Some(pb) = state.playback_status_json() {
                    let _ = state.broadcast_tx.send(pb);
                }
            }
        }

        ClientCommand::Seek { position_ns } => {
            let mut state = gateway.lock().await;
            if let Some(ref mut replayer) = state.replayer {
                let relative_ns: u64 = position_ns.parse().unwrap_or(0);
                let absolute_ns = replayer.start_ns() + relative_ns;
                replayer.seek(absolute_ns);
                // Clone events to avoid double borrow
                let events = state.events.clone();
                state.world_model.rebuild_to(&events, absolute_ns);
                let ws = state.world_state_json();
                let _ = state.broadcast_tx.send(ws);
                if let Some(pb) = state.playback_status_json() {
                    let _ = state.broadcast_tx.send(pb);
                }
            }
        }

        ClientCommand::Speed { multiplier } => {
            let mut state = gateway.lock().await;
            if let Some(ref mut replayer) = state.replayer {
                replayer.set_speed(multiplier);
                if let Some(pb) = state.playback_status_json() {
                    let _ = state.broadcast_tx.send(pb);
                }
            }
        }

        ClientCommand::Filter { pids, types } => {
            let mut state = gateway.lock().await;
            if let Some(ref mut replayer) = state.replayer {
                replayer.set_filter(pids, types);
            }
        }
    }
}

/// Start the playback loop as a background task.
async fn start_playback(gateway: SharedGateway) {
    // Spawn playback loop
    tokio::spawn(async move {
        let world_state_gateway = gateway.clone();

        // Spawn world state update loop
        let ws_loop = tokio::spawn(async move {
            loop {
                tokio::time::sleep(tokio::time::Duration::from_millis(WORLD_STATE_INTERVAL_MS)).await;
                let state = world_state_gateway.lock().await;
                if let Some(ref replayer) = state.replayer {
                    if replayer.is_playing() {
                        let ws = state.world_state_json();
                        let _ = state.broadcast_tx.send(ws);
                        if let Some(pb) = state.playback_status_json() {
                            let _ = state.broadcast_tx.send(pb);
                        }
                    } else {
                        break;
                    }
                } else {
                    break;
                }
            }
        });

        // Main playback loop
        loop {
            // Check if we should continue
            {
                let state = gateway.lock().await;
                if let Some(ref replayer) = state.replayer {
                    if !replayer.is_playing() {
                        break;
                    }
                } else {
                    break;
                }
            }

            // Get next batch (this may sleep for timing)
            let batch: Option<Vec<TraceEvent>> = {
                let mut state = gateway.lock().await;
                if let Some(ref mut replayer) = state.replayer {
                    replayer.next_batch().await
                } else {
                    None
                }
            };

            match batch {
                Some(events) => {
                    let mut state = gateway.lock().await;
                    state.world_model.process_events(&events);

                    let ws_batch: Vec<serde_json::Value> =
                        events.iter().map(|e| e.to_ws_value()).collect();
                    let msg = serde_json::json!({"msg": "events", "batch": ws_batch}).to_string();
                    let _ = state.broadcast_tx.send(msg);
                }
                None => {
                    // Playback finished
                    let mut state = gateway.lock().await;
                    if let Some(ref mut replayer) = state.replayer {
                        replayer.pause();
                    }
                    if let Some(pb) = state.playback_status_json() {
                        let _ = state.broadcast_tx.send(pb);
                    }
                    tracing::info!("Playback finished");
                    break;
                }
            }
        }

        ws_loop.abort();
    });
}
