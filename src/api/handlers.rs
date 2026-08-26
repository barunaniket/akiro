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

async fn submit_to_cluster(
    redis_url: &str,
    request: &JobRequest,
) -> Result<crate::orchestrator::JobResult, Box<dyn std::error::Error + Send + Sync>> {
    let client = redis::Client::open(redis_url)?;
    let mut con = client.get_multiplexed_tokio_connection().await?;

    let job_json = serde_json::to_string(request)?;
    let job_id = request.job_id.clone();
    let result_key = format!("judge:results:{}", job_id);

    // Publish job into Redis Stream
    let _: () = redis::cmd("XADD")
        .arg("judge:jobs")
        .arg("*")
        .arg("job")
        .arg(&job_json)
        .query_async(&mut con)
        .await?;

    // Wait for any cluster worker node to pick up and process the job (ample buffer for 200+ job stampedes)
    let max_wait_ms = (request.time_limit_ms * request.test_cases.len() as u64 * 3 + 60_000).max(600_000);
    let start = std::time::Instant::now();

    while start.elapsed().as_millis() < max_wait_ms as u128 {
        tokio::time::sleep(tokio::time::Duration::from_millis(15)).await;

        let res: Option<String> = redis::cmd("GET")
            .arg(&result_key)
            .query_async(&mut con)
            .await?;

        if let Some(json_str) = res {
            let result: crate::orchestrator::JobResult = serde_json::from_str(&json_str)?;
            // Clean up result key asynchronously
            let _: Result<(), _> = redis::cmd("DEL").arg(&result_key).query_async(&mut con).await;
            return Ok(result);
        }
    }

    Err("Job execution timed out waiting for cluster worker response".into())
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

    // If Redis cluster is configured, dispatch across all distributed worker nodes
    if let Some(ref redis_url) = state.redis_url {
        match submit_to_cluster(redis_url, &request).await {
            Ok(result) => return (StatusCode::OK, Json(serde_json::to_value(&result).unwrap_or_default())),
            Err(e) => {
                tracing::warn!("Cluster queue dispatch error: {}. Falling back to local worker pool.", e);
            }
        }
    }

    // Fallback to local in-memory worker pool
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
