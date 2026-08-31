use axum::{
    extract::{Path, State, ws::{Message, WebSocket, WebSocketUpgrade}},
    http::StatusCode,
};
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::orchestrator::JobRequest;
use super::ApiState;

pub async fn handle_websocket(
    State(state): State<Arc<ApiState>>,
    ws: WebSocketUpgrade,
) -> Result<impl axum::response::IntoResponse, StatusCode> {
    Ok(ws.on_upgrade(|socket| handle_socket(socket, state)))
}

async fn handle_socket(socket: WebSocket, state: Arc<ApiState>) {
    let (mut sender, mut receiver) = socket.split();

    // Wait for client to send JobRequest
    while let Some(msg) = receiver.next().await {
        match msg {
            Ok(axum::extract::ws::Message::Text(text)) => {
                // Parse JobRequest from JSON
                match serde_json::from_str::<JobRequest>(&text) {
                    Ok(request) => {
                        // Validate language
                        let lang = match crate::languages::SupportedLanguage::from_str(&request.language) {
                            Some(l) => l,
                            None => {
                                let error_msg = serde_json::json!({
                                    "error": format!("Unsupported language: {}", request.language),
                                    "supported": crate::languages::SupportedLanguage::all_canonical_names()
                                }).to_string();
                                let _ = sender.send(axum::extract::ws::Message::Text(error_msg)).await;
                                break;
                            }
                        };

                        // Check language whitelist if configured
                        if let Some(whitelist) = &state.enabled_languages {
                            if !whitelist.contains(&lang) {
                                let mut enabled_list: Vec<&'static str> = whitelist.iter().map(|l| l.as_str()).collect();
                                enabled_list.sort();
                                let error_msg = serde_json::json!({
                                    "error": format!("Language '{}' is disabled on this judge instance", request.language),
                                    "enabled_languages": enabled_list
                                }).to_string();
                                let _ = sender.send(axum::extract::ws::Message::Text(error_msg)).await;
                                break;
                            }
                        }

                        // Validate submission shape & size limits (shared with the REST path)
                        if let Err(msg) = request.validate() {
                            let error_msg = serde_json::json!({ "error": msg }).to_string();
                            let _ = sender.send(axum::extract::ws::Message::Text(error_msg)).await;
                            break;
                        }

                        // Create progress channel
                        let (progress_tx, mut progress_rx) = mpsc::unbounded_channel();

                        // Spawn task to execute job
                        let pool = state.pool.clone();
                        tokio::spawn(async move {
                            let _ = pool.submit(request, Some(progress_tx)).await;
                        });

                        // Stream progress events to client
                        while let Some(event) = progress_rx.recv().await {
                            if let Ok(json) = serde_json::to_string(&event) {
                                if sender.send(axum::extract::ws::Message::Text(json)).await.is_err() {
                                    break;
                                }
                            }
                        }

                        break;
                    }
                    Err(e) => {
                        let error_msg = serde_json::json!({
                            "error": format!("Failed to parse request: {}", e)
                        }).to_string();
                        let _ = sender.send(axum::extract::ws::Message::Text(error_msg)).await;
                        break;
                    }
                }
            }
            Ok(axum::extract::ws::Message::Close(_)) => break,
            Err(_) => break,
            _ => {}
        }
    }
}

/// WebSocket "buzzer": push the FINAL `JobResult` for `job_id` the instant it's ready, then
/// close. Cluster-only (needs the ResultBus). Separate from `handle_websocket`, which streams
/// live progress from the local pool and is unchanged.
pub async fn handle_result_ws(
    State(state): State<Arc<ApiState>>,
    Path(job_id): Path<String>,
    ws: WebSocketUpgrade,
) -> Result<impl axum::response::IntoResponse, StatusCode> {
    Ok(ws.on_upgrade(move |socket| handle_result_socket(socket, state, job_id)))
}

async fn handle_result_socket(socket: WebSocket, state: Arc<ApiState>, job_id: String) {
    let (mut sender, mut receiver) = socket.split();

    let bus = match &state.result_bus {
        Some(b) => b.clone(),
        None => {
            let _ = sender
                .send(Message::Text(
                    serde_json::json!({ "error": "result push requires the Redis cluster" }).to_string(),
                ))
                .await;
            let _ = sender.send(Message::Close(None)).await;
            return;
        }
    };

    // Fast paths: already done, or unknown id (never hang the socket on a non-existent job).
    if let Some(result) = bus.try_result(&job_id).await {
        if let Ok(json) = serde_json::to_string(&result) {
            let _ = sender.send(Message::Text(json)).await;
        }
        let _ = sender.send(Message::Close(None)).await;
        return;
    }
    if !bus.is_pending(&job_id).await {
        let _ = sender
            .send(Message::Text(
                serde_json::json!({ "error": "unknown or expired job_id", "job_id": job_id }).to_string(),
            ))
            .await;
        let _ = sender.send(Message::Close(None)).await;
        return;
    }

    // In flight — wait for the buzzer, but bail if the client disconnects (cancels the wait,
    // dropping its registry guard). Bounded by a deadline.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(600);
    tokio::select! {
        res = bus.wait_for(&job_id, deadline) => {
            match res {
                Some(result) => {
                    if let Ok(json) = serde_json::to_string(&result) {
                        let _ = sender.send(Message::Text(json)).await;
                    }
                }
                None => {
                    let _ = sender.send(Message::Text(
                        serde_json::json!({ "error": "timed out waiting for result", "job_id": job_id }).to_string(),
                    )).await;
                }
            }
        }
        _ = async {
            while let Some(msg) = receiver.next().await {
                if matches!(msg, Ok(Message::Close(_)) | Err(_)) {
                    break;
                }
            }
        } => {}
    }
    let _ = sender.send(Message::Close(None)).await;
}
