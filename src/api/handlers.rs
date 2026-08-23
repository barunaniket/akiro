use axum::{
    extract::{State, Json},
    http::StatusCode,
};
use serde_json::json;
use std::sync::Arc;

use crate::orchestrator::JobRequest;
use super::ApiState;

#[derive(serde::Serialize)]
pub struct HealthResponse {
    pub idle_workers: usize,
    pub busy_workers: usize,
    pub queued_jobs: usize,
    pub total_workers: usize,
    pub uptime_secs: u64,
}

fn get_cluster_stats(
    redis_url: &str,
    default_workers: usize,
    default_idle: usize,
    default_busy: usize,
) -> (usize, usize, usize) {
    if let Ok(client) = redis::Client::open(redis_url) {
        if let Ok(mut con) = client.get_connection() {
            let res: redis::RedisResult<redis::Value> = redis::cmd("XINFO")
                .arg("CONSUMERS")
                .arg("judge:jobs")
                .arg("judge_workers")
                .query(&mut con);

            if let Ok(redis::Value::Bulk(consumers)) = res {
                let mut total = 0;
                let mut total_pending = 0;

                for item in consumers {
                    if let redis::Value::Bulk(fields) = item {
                        let mut name = String::new();
                        let mut idle_ms: i64 = 0;
                        let mut pending: usize = 0;

                        let mut iter = fields.into_iter();
                        while let (Some(k), Some(v)) = (iter.next(), iter.next()) {
                            if let redis::Value::Data(k_bytes) = k {
                                let k_str = String::from_utf8_lossy(&k_bytes);
                                match k_str.as_ref() {
                                    "name" => {
                                        if let redis::Value::Data(v_bytes) = v {
                                            name = String::from_utf8_lossy(&v_bytes).to_string();
                                        }
                                    }
                                    "idle" => {
                                        if let redis::Value::Int(v_int) = v {
                                            idle_ms = v_int;
                                        }
                                    }
                                    "pending" => {
                                        if let redis::Value::Int(v_int) = v {
                                            pending = v_int.max(0) as usize;
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }

                        // Only consider consumers that were active within the last 2 minutes
                        if idle_ms < 120_000 {
                            let worker_count = if let Some(idx) = name.find("-w") {
                                let rest = &name[idx + 2..];
                                rest.split('-').next().and_then(|s| s.parse::<usize>().ok()).unwrap_or(2)
                            } else {
                                2
                            };
                            total += worker_count;
                            total_pending += pending;
                        }
                    }
                }

                if total > 0 {
                    let busy = total_pending.min(total);
                    let idle = total.saturating_sub(busy);
                    return (total, idle, busy);
                }
            }
        }
    }
    (default_workers, default_idle, default_busy)
}

pub async fn health(
    State(state): State<Arc<ApiState>>,
) -> Json<HealthResponse> {
    let local_total = state.pool.num_workers();
    let local_idle = state.pool.idle_workers();
    let local_busy = state.pool.busy_workers();

    let (total_workers, idle_workers, busy_workers) = match &state.redis_url {
        Some(url) => get_cluster_stats(url, local_total, local_idle, local_busy),
        None => (local_total, local_idle, local_busy),
    };

    Json(HealthResponse {
        idle_workers,
        busy_workers,
        queued_jobs: state.pool.queued_jobs(),
        total_workers,
        uptime_secs: state.start_time.elapsed().as_secs(),
    })
}

pub async fn submit(
    State(state): State<Arc<ApiState>>,
    Json(request): Json<JobRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    // Validate language
    if crate::languages::SupportedLanguage::from_str(&request.language).is_none() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": format!("Unsupported language: {}", request.language),
                "supported": ["c", "cpp", "python", "javascript", "typescript", "sql", "java"]
            })),
        );
    }

    // Validate test cases exist
    if request.test_cases.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "At least one test case is required"})),
        );
    }

    // Submit to worker pool with backpressure protection
    match state.pool.submit(request, None).await {
        Ok(result) => (StatusCode::OK, Json(serde_json::to_value(&result).unwrap())),
        Err(e) if e == "QUEUE_FULL" => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "error": "Judge queue is full. Server is under heavy contest load. Please retry in 3 seconds.",
                "retry_after_secs": 3
            })),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e })),
        ),
    }
}
