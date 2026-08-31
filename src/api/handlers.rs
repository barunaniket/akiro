use axum::{
    extract::{Path, State, Json},
    http::StatusCode,
};
use serde_json::json;
use std::sync::Arc;

use crate::orchestrator::JobRequest;
use super::ApiState;
use super::result_bus::EnqueueOutcome;

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
            // Check real-time heartbeat registrations (updated every 20s by live workers)
            let heartbeat_keys: redis::RedisResult<Vec<String>> = redis::cmd("KEYS")
                .arg("judge:heartbeat:*")
                .query(&mut con);

            if let Ok(keys) = heartbeat_keys {
                if !keys.is_empty() {
                    let mut total_heartbeat_workers = 0;
                    for key in &keys {
                        if let Ok(count_str) = redis::cmd("GET").arg(key).query::<String>(&mut con) {
                            if let Ok(count) = count_str.parse::<usize>() {
                                total_heartbeat_workers += count;
                            }
                        }
                    }
                    if total_heartbeat_workers > 0 {
                        return (total_heartbeat_workers, total_heartbeat_workers, 0);
                    }
                }
            }

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

pub async fn metrics(
    State(state): State<Arc<ApiState>>,
) -> impl axum::response::IntoResponse {
    let local_total = state.pool.num_workers();
    let local_idle = state.pool.idle_workers();
    let local_busy = state.pool.busy_workers();

    let (total_workers, idle_workers, busy_workers) = match &state.redis_url {
        Some(url) => get_cluster_stats(url, local_total, local_idle, local_busy),
        None => (local_total, local_idle, local_busy),
    };

    let body = crate::metrics::render_prometheus(
        total_workers,
        idle_workers,
        busy_workers,
        state.pool.queued_jobs(),
        state.start_time.elapsed().as_secs(),
    );

    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        body,
    )
}

/// Shared validation for every submit path: language support → whitelist → shape/size limits.
/// On failure returns the exact `(status, body)` to send back.
fn validate_submission(
    state: &ApiState,
    request: &JobRequest,
) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    let lang = match crate::languages::SupportedLanguage::from_str(&request.language) {
        Some(l) => l,
        None => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": format!("Unsupported language: {}", request.language),
                    "supported": crate::languages::SupportedLanguage::all_canonical_names()
                })),
            ));
        }
    };

    if let Some(whitelist) = &state.enabled_languages {
        if !whitelist.contains(&lang) {
            let mut enabled_list: Vec<&'static str> = whitelist.iter().map(|l| l.as_str()).collect();
            enabled_list.sort();
            return Err((
                StatusCode::FORBIDDEN,
                Json(json!({
                    "error": format!("Language '{}' is disabled on this judge instance", request.language),
                    "enabled_languages": enabled_list
                })),
            ));
        }
    }

    if let Err(msg) = request.validate() {
        return Err((StatusCode::BAD_REQUEST, Json(json!({ "error": msg }))));
    }
    Ok(())
}

/// Max time the gateway waits for a cluster result; also bounds the pending-marker TTL.
fn max_wait_ms(request: &JobRequest) -> u64 {
    (request.time_limit_ms * request.test_cases.len() as u64 * 3 + 60_000).max(600_000)
}

/// Run a job on the local in-memory worker pool (no cluster, or Redis unreachable).
async fn submit_local(
    state: &ApiState,
    request: JobRequest,
) -> (StatusCode, Json<serde_json::Value>) {
    match state.pool.submit(request, None).await {
        Ok(result) => (StatusCode::OK, Json(serde_json::to_value(&result).unwrap_or_default())),
        Err(e) if e == "QUEUE_FULL" => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "error": "Judge queue is full. Server is under heavy contest load. Please retry in 3 seconds.",
                "retry_after_secs": 3
            })),
        ),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e }))),
    }
}

/// Synchronous submit — **unchanged contract** (`200 { JobResult }`), but now event-driven:
/// enqueue once, then wait on the result "buzzer" (no 15 ms polling). Falls back to the local
/// pool ONLY if the enqueue itself fails (Redis down) — never on a wait-timeout, which would
/// re-execute a merely-slow job (the old double-execution bug).
pub async fn submit(
    State(state): State<Arc<ApiState>>,
    Json(mut request): Json<JobRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    if let Err(resp) = validate_submission(&state, &request) {
        return resp;
    }
    if request.job_id.is_empty() {
        request.job_id = uuid::Uuid::new_v4().to_string();
    }

    if let Some(bus) = &state.result_bus {
        let wait = max_wait_ms(&request);
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(wait);
        match bus.enqueue(&request, wait / 1000 + 60).await {
            // Queued, or a duplicate already in flight — either way, wait for THAT result (never
            // re-run). Duplicate here means a retry of the same job_id; we attach to the existing run.
            EnqueueOutcome::Queued | EnqueueOutcome::Duplicate => {
                return match bus.wait_for(&request.job_id, deadline).await {
                    Some(result) => (StatusCode::OK, Json(serde_json::to_value(&result).unwrap_or_default())),
                    None => (
                        StatusCode::GATEWAY_TIMEOUT,
                        Json(json!({
                            "error": "timed out waiting for cluster result; retry GET /api/v1/result/{job_id}",
                            "job_id": request.job_id
                        })),
                    ),
                };
            }
            EnqueueOutcome::RedisDown => {
                tracing::warn!(
                    "Cluster enqueue failed (Redis down); falling back to local pool for job {}",
                    request.job_id
                );
                // fall through to the local pool
            }
        }
    }

    submit_local(&state, request).await
}

/// Async submit — enqueue and return immediately with the `job_id`. The result is retrieved via
/// `GET /api/v1/result/:job_id` or the WebSocket push. Cluster-only.
pub async fn submit_async(
    State(state): State<Arc<ApiState>>,
    Json(mut request): Json<JobRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    if let Err(resp) = validate_submission(&state, &request) {
        return resp;
    }
    if request.job_id.is_empty() {
        request.job_id = uuid::Uuid::new_v4().to_string();
    }

    let bus = match &state.result_bus {
        Some(b) => b,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "error": "async submit requires the Redis cluster (JUDGE_REDIS not configured)" })),
            );
        }
    };

    let wait = max_wait_ms(&request);
    match bus.enqueue(&request, wait / 1000 + 60).await {
        EnqueueOutcome::Queued => (
            StatusCode::ACCEPTED,
            Json(json!({
                "job_id": request.job_id,
                "status": "queued",
                "result_url": format!("/api/v1/result/{}", request.job_id)
            })),
        ),
        EnqueueOutcome::Duplicate => (
            StatusCode::CONFLICT,
            Json(json!({ "error": "job_id already in-flight", "job_id": request.job_id })),
        ),
        EnqueueOutcome::RedisDown => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "queue unavailable (Redis unreachable)", "job_id": request.job_id })),
        ),
    }
}

/// Fetch a result by id: `200 {JobResult}` | `202 {status:"pending"}` | `404` unknown/expired.
/// Idempotent — does not delete the key (relies on the result TTL). Cluster-only.
pub async fn get_result(
    State(state): State<Arc<ApiState>>,
    Path(job_id): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    let bus = match &state.result_bus {
        Some(b) => b,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "error": "result lookup requires the Redis cluster" })),
            );
        }
    };

    if let Some(result) = bus.try_result(&job_id).await {
        return (StatusCode::OK, Json(serde_json::to_value(&result).unwrap_or_default()));
    }
    if bus.is_pending(&job_id).await {
        return (StatusCode::ACCEPTED, Json(json!({ "job_id": job_id, "status": "pending" })));
    }
    (StatusCode::NOT_FOUND, Json(json!({ "error": "unknown or expired job_id", "job_id": job_id })))
}
