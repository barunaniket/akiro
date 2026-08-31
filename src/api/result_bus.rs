//! ResultBus — the gateway side of the async "buzzer".
//!
//! Instead of the gateway holding each HTTP connection open and polling
//! `GET judge:results:{id}` every 15 ms for the whole job, a single always-on Redis pub/sub
//! connection `PSUBSCRIBE`s `judge:done:*`. When a worker finishes it `PUBLISH`es
//! `judge:done:{id}` (the "bell"); the listener routes that to in-process waiters via a
//! registry, which then re-`GET` the result key (the source of truth). One Redis connection
//! serves any number of concurrent waiters — no per-request pub/sub connection, no polling
//! storm. A short safety poll inside `wait_for` covers the (rare) dropped-PUBLISH case, so
//! liveness never depends on the best-effort bell.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use redis::aio::ConnectionManager;
use tokio::sync::broadcast;

use crate::orchestrator::{JobRequest, JobResult};

type Registry = Arc<Mutex<HashMap<String, broadcast::Sender<()>>>>;

/// Outcome of trying to enqueue a job.
pub enum EnqueueOutcome {
    Queued,
    /// A job with this `job_id` is already in flight (pending marker present).
    Duplicate,
    /// Redis was unreachable — caller may fall back to the local pool.
    RedisDown,
}

pub struct ResultBus {
    /// Shared, cloneable, auto-reconnecting connection for GET/enqueue (NOT pub/sub).
    conn: ConnectionManager,
    registry: Registry,
}

/// RAII: removes the job's registry entry when the last waiter is dropped (client
/// disconnect / cancellation), so the map never leaks entries.
pub struct WaiterGuard {
    job_id: String,
    registry: Registry,
}

impl Drop for WaiterGuard {
    fn drop(&mut self) {
        let mut map = self.registry.lock().expect("result_bus registry poisoned");
        if let Some(tx) = map.get(&self.job_id) {
            // Only reap once no receivers remain (some other waiter may still hold one).
            if tx.receiver_count() == 0 {
                map.remove(&self.job_id);
            }
        }
    }
}

impl ResultBus {
    /// Build the bus (shared conn) and spawn the always-on `PSUBSCRIBE judge:done:*` listener.
    /// Returns `None` if Redis can't be reached at startup.
    pub async fn spawn(redis_url: &str) -> Option<Arc<ResultBus>> {
        let client = match redis::Client::open(redis_url) {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("ResultBus: invalid redis url {}: {}", redis_url, e);
                return None;
            }
        };
        let conn = match ConnectionManager::new(client.clone()).await {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("ResultBus: could not connect to Redis: {}", e);
                return None;
            }
        };
        let registry: Registry = Arc::new(Mutex::new(HashMap::new()));
        tokio::spawn(run_listener(client, registry.clone()));
        Some(Arc::new(ResultBus { conn, registry }))
    }

    /// Register a waiter for `job_id` BEFORE the first GET, so a bell arriving in the gap
    /// can't be missed. Returns a receiver plus a drop-guard.
    fn register(&self, job_id: &str) -> (broadcast::Receiver<()>, WaiterGuard) {
        let mut map = self.registry.lock().expect("result_bus registry poisoned");
        let tx = map
            .entry(job_id.to_string())
            .or_insert_with(|| broadcast::channel::<()>(1).0)
            .clone();
        let rx = tx.subscribe();
        let guard = WaiterGuard {
            job_id: job_id.to_string(),
            registry: self.registry.clone(),
        };
        (rx, guard)
    }

    /// Single non-blocking read of the result key (used by `GET /result` and internally).
    pub async fn try_result(&self, job_id: &str) -> Option<JobResult> {
        let mut con = self.conn.clone();
        let key = format!("judge:results:{}", job_id);
        let json: Option<String> = redis::cmd("GET")
            .arg(&key)
            .query_async(&mut con)
            .await
            .ok()?;
        serde_json::from_str::<JobResult>(&json?).ok()
    }

    /// Whether a pending marker exists (distinguishes `202 pending` from `404 unknown`).
    pub async fn is_pending(&self, job_id: &str) -> bool {
        let mut con = self.conn.clone();
        let key = format!("judge:pending:{}", job_id);
        redis::cmd("EXISTS")
            .arg(&key)
            .query_async::<_, bool>(&mut con)
            .await
            .unwrap_or(false)
    }

    /// Atomic-ish enqueue: clear any stale result, claim the pending marker (`NX` detects a
    /// live duplicate), then `XADD` the job. `pending_ttl_secs` should outlast the job's
    /// max wait so a crashed job's marker eventually expires (→ `GET` returns 404).
    pub async fn enqueue(&self, request: &JobRequest, pending_ttl_secs: u64) -> EnqueueOutcome {
        let mut con = self.conn.clone();
        let job_json = match serde_json::to_string(request) {
            Ok(j) => j,
            Err(_) => return EnqueueOutcome::RedisDown,
        };
        let result_key = format!("judge:results:{}", request.job_id);
        let pending_key = format!("judge:pending:{}", request.job_id);

        // DEL stale result + claim pending (NX). Read BOTH replies as a tuple (a pipeline
        // response is always a bulk array — `.ignore()` does NOT unwrap it to a scalar).
        // `set_reply` is None iff the marker already existed → a job with this id is in flight.
        let (_del_count, set_reply): (i64, Option<String>) = match redis::pipe()
            .cmd("DEL")
            .arg(&result_key)
            .cmd("SET")
            .arg(&pending_key)
            .arg("1")
            .arg("EX")
            .arg(pending_ttl_secs)
            .arg("NX")
            .query_async(&mut con)
            .await
        {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("enqueue DEL/SET failed: {}", e);
                return EnqueueOutcome::RedisDown;
            }
        };
        if set_reply.is_none() {
            return EnqueueOutcome::Duplicate;
        }

        match redis::cmd("XADD")
            .arg("judge:jobs")
            .arg("*")
            .arg("job")
            .arg(&job_json)
            .query_async::<_, String>(&mut con)
            .await
        {
            Ok(_) => EnqueueOutcome::Queued,
            Err(e) => {
                tracing::warn!("enqueue XADD failed: {}", e);
                // Roll back the pending marker so a retry isn't rejected as a duplicate.
                let _: Result<(), _> = redis::cmd("DEL")
                    .arg(&pending_key)
                    .query_async(&mut con)
                    .await;
                EnqueueOutcome::RedisDown
            }
        }
    }

    /// Wait for the result up to `deadline`: register → GET once → await bell / safety poll /
    /// deadline. Returns `None` on timeout (caller returns 504 / WS timeout frame).
    pub async fn wait_for(&self, job_id: &str, deadline: Instant) -> Option<JobResult> {
        // 1. Register FIRST so the bell can't slip through before we're listening.
        let (mut rx, _guard) = self.register(job_id);
        // 2. GET once — covers "result landed before we registered".
        if let Some(r) = self.try_result(job_id).await {
            return Some(r);
        }
        // 3. Await wake / safety poll / deadline.
        loop {
            let now = Instant::now();
            if now >= deadline {
                return None;
            }
            let poll = std::cmp::min(Duration::from_secs(2), deadline - now);
            tokio::select! {
                recv = rx.recv() => {
                    if let Some(r) = self.try_result(job_id).await {
                        return Some(r);
                    }
                    if matches!(recv, Err(broadcast::error::RecvError::Closed)) {
                        // Sender gone (shouldn't happen while we hold a receiver). Degrade to
                        // pure polling for the remainder rather than risk a busy loop.
                        return self.poll_until(job_id, deadline).await;
                    }
                }
                _ = tokio::time::sleep(poll) => {
                    if let Some(r) = self.try_result(job_id).await {
                        return Some(r);
                    }
                }
            }
        }
    }

    /// Fallback: poll every 2s until the result appears or the deadline passes.
    async fn poll_until(&self, job_id: &str, deadline: Instant) -> Option<JobResult> {
        loop {
            if let Some(r) = self.try_result(job_id).await {
                return Some(r);
            }
            let now = Instant::now();
            if now >= deadline {
                return None;
            }
            tokio::time::sleep(std::cmp::min(Duration::from_secs(2), deadline - now)).await;
        }
    }
}

/// Always-on listener: one dedicated pub/sub connection, `PSUBSCRIBE judge:done:*`, routing
/// each message to the waiter registry by its concrete channel name. Reconnects with backoff.
async fn run_listener(client: redis::Client, registry: Registry) {
    loop {
        match client.get_async_pubsub().await {
            Ok(mut pubsub) => {
                if let Err(e) = pubsub.psubscribe("judge:done:*").await {
                    tracing::warn!("ResultBus: psubscribe failed: {}; retrying", e);
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    continue;
                }
                tracing::info!("ResultBus: subscribed to judge:done:* (async result buzzer active)");
                let mut stream = pubsub.on_message();
                while let Some(msg) = stream.next().await {
                    let channel = msg.get_channel_name();
                    if let Some(job_id) = channel.strip_prefix("judge:done:") {
                        let tx = {
                            let map = registry.lock().expect("result_bus registry poisoned");
                            map.get(job_id).cloned()
                        };
                        if let Some(tx) = tx {
                            let _ = tx.send(()); // wake all waiters; Err just means none registered
                        }
                    }
                }
                tracing::warn!("ResultBus: pub/sub stream ended; reconnecting");
            }
            Err(e) => {
                tracing::warn!("ResultBus: pub/sub connect failed: {}; retrying", e);
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}
