//! hackbot gateway server — HTTP endpoints and WebSocket gateway.

mod gateway;
mod mock_data;
mod trace_loader;
mod trace_replayer;
mod world_model;

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::ws::WebSocketUpgrade;
use axum::extract::State;
use axum::response::Json;
use axum::routing::get;
use axum::Router;
use tokio::sync::Mutex;
use tower_http::cors::CorsLayer;
use tracing_subscriber::EnvFilter;

use gateway::{GatewayState, SharedGateway};

/// Resolve project root (two levels up from the binary's source dir)
fn project_root() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // server-rs/crates/hackbot-server -> server-rs -> hackbot
    manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.parent())
        .unwrap_or(&manifest_dir)
        .to_path_buf()
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let root = project_root();
    let traces_dir = root.join("traces");

    // Check for --generate-mock flag
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--generate-mock") {
        match mock_data::write_mock_trace(&traces_dir) {
            Ok(count) => {
                println!("Generated {count} events -> {}/sample-llm-workload.jsonl", traces_dir.display());
            }
            Err(e) => {
                eprintln!("Error generating mock data: {e}");
                std::process::exit(1);
            }
        }
        return;
    }

    let gateway_state = Arc::new(Mutex::new(GatewayState::new(traces_dir)));

    // Load default trace on startup
    {
        let mut state = gateway_state.lock().await;
        let default_trace = "sample-llm-workload.jsonl";
        if state.trace_files().contains(&default_trace.to_string()) {
            match state.load_trace(default_trace) {
                Ok(info) => {
                    tracing::info!(
                        "Loaded default trace: {} ({} events)",
                        default_trace,
                        info["event_count"]
                    );
                }
                Err(e) => tracing::error!("Failed to load default trace: {}", e),
            }
        } else {
            tracing::warn!("Default trace not found. Run with --generate-mock to create sample data.");
        }
    }

    let app = Router::new()
        .route("/", get(root_handler))
        .route("/traces", get(traces_handler))
        .route("/ws", get(ws_handler))
        .layer(CorsLayer::permissive())
        .with_state(gateway_state);

    let addr = "0.0.0.0:8000";
    tracing::info!("hackbot server listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap_or_else(|e| {
        eprintln!("ERROR: Cannot bind to {addr}: {e}");
        eprintln!("       Is another hackbot-server already running? Try: kill $(lsof -t -i :8000)");
        std::process::exit(1);
    });
    axum::serve(listener, app).await.unwrap();
}

async fn root_handler() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "name": "hackbot",
        "version": "0.1.0",
        "description": "eBPF trace replay and visualization gateway",
    }))
}

async fn traces_handler(State(gateway): State<SharedGateway>) -> Json<serde_json::Value> {
    let state = gateway.lock().await;
    Json(serde_json::json!({
        "traces": state.trace_files(),
    }))
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    State(gateway): State<SharedGateway>,
) -> axum::response::Response {
    tracing::info!("WebSocket upgrade request");
    ws.on_upgrade(move |socket| gateway::handle_ws(socket, gateway))
}
