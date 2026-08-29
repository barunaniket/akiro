use axum::{
    extract::{State, ws::{WebSocket, WebSocketUpgrade}},
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
