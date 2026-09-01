use std::sync::Arc;
use std::error::Error;
use crate::orchestrator::{JobRequest, JudgeWorkerPool};

pub struct RedisConsumer {
    redis_url: String,
    pool: Arc<JudgeWorkerPool>,
    stream_key: String,
    consumer_group: String,
    consumer_name: String,
}

impl RedisConsumer {
    pub fn new(
        redis_url: String,
        pool: Arc<JudgeWorkerPool>,
        stream_key: Option<String>,
        consumer_group: Option<String>,
    ) -> Self {
        let workers_count = pool.num_workers();
        Self {
            redis_url,
            pool,
            stream_key: stream_key.unwrap_or_else(|| "judge:jobs".to_string()),
            consumer_group: consumer_group.unwrap_or_else(|| "judge_workers".to_string()),
            consumer_name: format!("worker-w{}-{}", workers_count, uuid::Uuid::new_v4()),
        }
    }

    pub async fn run(&self) -> Result<(), Box<dyn Error>> {
        let client = redis::Client::open(self.redis_url.as_str())?;

        let mut async_con = {
            let mut retries = 0;
            loop {
                match client.get_multiplexed_tokio_connection().await {
                    Ok(c) => break c,
                    Err(e) => {
                        retries += 1;
                        if retries > 10 {
                            return Err(Box::new(e));
                        }
                        tracing::warn!("Waiting for Redis at {} (attempt {}/10): {}", self.redis_url, retries, e);
                        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                    }
                }
            }
        };

        // Create consumer group if it doesn't exist
        let _: Result<redis::Value, _> = redis::cmd("XGROUP")
            .arg("CREATE")
            .arg(&self.stream_key)
            .arg(&self.consumer_group)
            .arg("$")
            .arg("MKSTREAM")
            .query_async(&mut async_con)
            .await;

        // Register consumer explicitly in consumer group
        let _: Result<redis::Value, _> = redis::cmd("XGROUP")
            .arg("CREATECONSUMER")
            .arg(&self.stream_key)
            .arg(&self.consumer_group)
            .arg(&self.consumer_name)
            .query_async(&mut async_con)
            .await;

        let num_workers = self.pool.num_workers();
        // Allow up to 2x num_workers inflight jobs to keep all CPU cores 100% saturated
        let semaphore = Arc::new(tokio::sync::Semaphore::new((num_workers * 2).max(4)));

        tracing::info!(
            "Redis concurrent consumer started: {} on group {} as {} (capacity: {} workers)",
            self.stream_key,
            self.consumer_group,
            self.consumer_name,
            num_workers
        );

        // Result retention in Redis. Shorter than the old hard-coded 24h because results embed
        // stdout/stderr and pile up under load; a result only needs to live long enough for the
        // client to poll/receive it (and re-fetch after a gateway restart). Env-tunable.
        let result_ttl: u64 = std::env::var("JUDGE_RESULT_TTL_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(1800);

        // Background orphan-reclaim sweep (XAUTOCLAIM). If a worker claims a job via XREADGROUP
        // but then dies or loses its Redis connection before it can ACK, that entry sits stranded
        // in the consumer-group PEL forever (no auto-reclaim) → the client waits until its own
        // timeout. `publish_result`'s retry (below) prevents the common transient-blip case; this
        // sweep is the safety net for true worker death. Both min-idle and interval are env-tunable
        // so short-job contests can lower min-idle for faster recovery.
        //
        // min-idle default 300s is deliberately > the worst-case *legitimate* job runtime: an entry
        // that's still being processed by a live worker keeps aging in the PEL, so a too-low min-idle
        // would reclaim + re-run healthy in-progress jobs (idempotent, but wasted compute on 2 cores).
        {
            let reclaim_min_idle_ms: u64 = std::env::var("JUDGE_RECLAIM_MIN_IDLE_MS")
                .ok().and_then(|s| s.parse().ok()).unwrap_or(300_000);
            let reclaim_interval_ms: u64 = std::env::var("JUDGE_RECLAIM_INTERVAL_MS")
                .ok().and_then(|s| s.parse().ok()).unwrap_or(15_000);
            tokio::spawn(Self::run_reclaim(
                client.clone(),
                self.pool.clone(),
                self.stream_key.clone(),
                self.consumer_group.clone(),
                self.consumer_name.clone(),
                semaphore.clone(),
                result_ttl,
                reclaim_min_idle_ms,
                tokio::time::Duration::from_millis(reclaim_interval_ms),
            ));
        }

        loop {
            // Heartbeat: register active worker node with 20-second TTL
            let heartbeat_key = format!("judge:heartbeat:{}", self.consumer_name);
            let _: Result<redis::Value, _> = redis::cmd("SET")
                .arg(&heartbeat_key)
                .arg(num_workers)
                .arg("EX")
                .arg(20)
                .query_async(&mut async_con)
                .await;

            let _: Result<redis::Value, _> = redis::cmd("XGROUP")
                .arg("CREATECONSUMER")
                .arg(&self.stream_key)
                .arg(&self.consumer_group)
                .arg(&self.consumer_name)
                .query_async(&mut async_con)
                .await;

            // Determine how many jobs we can fetch without overwhelming the worker pool
            let available = semaphore.available_permits().max(1);
            let batch_size = available.min(num_workers.max(4));

            let read_cmd = redis::cmd("XREADGROUP")
                .arg("GROUP")
                .arg(&self.consumer_group)
                .arg(&self.consumer_name)
                .arg("BLOCK")
                .arg(2000)
                .arg("COUNT")
                .arg(batch_size)
                .arg("STREAMS")
                .arg(&self.stream_key)
                .arg(">")
                .query_async(&mut async_con)
                .await;

            match read_cmd {
                Ok(redis::Value::Bulk(keys)) => {
                    for key_item in keys {
                        if let redis::Value::Bulk(key_fields) = key_item {
                            if key_fields.len() >= 2 {
                                if let redis::Value::Bulk(messages) = &key_fields[1] {
                                    for msg in messages {
                                        if let redis::Value::Bulk(msg_data) = msg {
                                            if msg_data.len() >= 2 {
                                                let msg_id = match &msg_data[0] {
                                                    redis::Value::Data(id_bytes) => String::from_utf8_lossy(id_bytes).to_string(),
                                                    _ => continue,
                                                };

                                                let job_json = Self::extract_job_field(&msg_data[1]);

                                                if let Some(json_str) = job_json {
                                                    match serde_json::from_str::<JobRequest>(&json_str) {
                                                        Ok(request) => {
                                                            let permit = match semaphore.clone().try_acquire_owned() {
                                                                Ok(p) => p,
                                                                Err(_) => match semaphore.clone().acquire_owned().await {
                                                                    Ok(p) => p,
                                                                    Err(_) => continue,
                                                                },
                                                            };

                                                            Self::spawn_process(
                                                                self.pool.clone(),
                                                                self.stream_key.clone(),
                                                                self.consumer_group.clone(),
                                                                client.clone(),
                                                                msg_id,
                                                                request,
                                                                permit,
                                                                result_ttl,
                                                            );
                                                        }
                                                        Err(e) => {
                                                            tracing::error!("Failed to parse job JSON from Redis: {}", e);
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                Ok(redis::Value::Nil) => {
                    // Timeout with no messages, normal loop
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!("Redis stream read warning: {}. Attempting to reconnect...", e);
                    tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;

                    match client.get_multiplexed_tokio_connection().await {
                        Ok(new_con) => {
                            async_con = new_con;
                            // Ensure consumer group and stream exist on reconnect or NOGROUP
                            let _: Result<redis::Value, _> = redis::cmd("XGROUP")
                                .arg("CREATE")
                                .arg(&self.stream_key)
                                .arg(&self.consumer_group)
                                .arg("$")
                                .arg("MKSTREAM")
                                .query_async(&mut async_con)
                                .await;
                            tracing::info!("Reconnected to Redis and verified consumer group ✓");
                        }
                        Err(reconn_err) => {
                            tracing::debug!("Redis reconnection attempt failed: {}", reconn_err);
                        }
                    }
                }
            }
        }
    }

    /// Pull the `job` field's value out of an XREADGROUP/XAUTOCLAIM message's field-value bulk.
    fn extract_job_field(fields: &redis::Value) -> Option<String> {
        if let redis::Value::Bulk(kvs) = fields {
            let mut iter = kvs.iter();
            while let (Some(k), Some(v)) = (iter.next(), iter.next()) {
                if let (redis::Value::Data(k_bytes), redis::Value::Data(v_bytes)) = (k, v) {
                    if String::from_utf8_lossy(k_bytes) == "job" {
                        return Some(String::from_utf8_lossy(v_bytes).to_string());
                    }
                }
            }
        }
        None
    }

    /// Run + report one job. Shared by the main XREADGROUP loop and the XAUTOCLAIM reclaim sweep so
    /// both paths complete a job identically (run → store result → clear pending → ACK → ring bell).
    /// Takes owned clones (no `&self`) so it can be spawned freely from either caller.
    #[allow(clippy::too_many_arguments)]
    fn spawn_process(
        pool: Arc<JudgeWorkerPool>,
        stream_key: String,
        consumer_group: String,
        client: redis::Client,
        msg_id: String,
        request: JobRequest,
        permit: tokio::sync::OwnedSemaphorePermit,
        result_ttl: u64,
    ) {
        tokio::spawn(async move {
            tracing::info!("Processing job {} concurrently from Redis", request.job_id);

            let result = match pool.submit(request.clone(), None).await {
                Ok(result) => result,
                Err(e) => {
                    tracing::error!("Job {} failed: {}", request.job_id, e);
                    crate::orchestrator::JobResult {
                        job_id: request.job_id.clone(),
                        verdict: crate::orchestrator::JudgeVerdict::RuntimeError,
                        total_cpu_time_ms: 0,
                        peak_memory_kb: 0,
                        compile_output: None,
                        test_results: vec![],
                    }
                }
            };

            Self::publish_result(&client, &stream_key, &consumer_group, &msg_id, &result, result_ttl).await;
            drop(permit);
        });
    }

    /// Store the result (source of truth), clear the pending marker, ACK the stream, then RING THE
    /// BELL (PUBLISH). SET precedes PUBLISH so any waiter woken by the bell is guaranteed to read
    /// the result. Payload is just the job_id — results are large, so waiters re-GET the key.
    ///
    /// The job already ran; losing this report would strand the entry in the PEL until the reclaim
    /// sweep re-runs it (≥ min-idle + a full re-run). So retry hard on transient connection failure
    /// (~20s total) — a home-network blip to a remote Redis routinely lasts 10–30s. Holding the
    /// semaphore permit across these retries is intentional backpressure: if Redis is unreachable we
    /// couldn't ACK new work anyway.
    async fn publish_result(
        client: &redis::Client,
        stream_key: &str,
        consumer_group: &str,
        msg_id: &str,
        result: &crate::orchestrator::JobResult,
        result_ttl: u64,
    ) {
        let result_key = format!("judge:results:{}", result.job_id);
        let pending_key = format!("judge:pending:{}", result.job_id);
        let done_channel = format!("judge:done:{}", result.job_id);
        let result_json = serde_json::to_string(result).unwrap_or_default();

        let ok = Self::run_pipe_with_retry(
            client,
            &format!("publish result for job {}", result.job_id),
            || {
                let mut p = redis::pipe();
                p.cmd("SET").arg(&result_key).arg(&result_json).arg("EX").arg(result_ttl)
                    .cmd("DEL").arg(&pending_key)
                    .cmd("XACK").arg(stream_key).arg(consumer_group).arg(msg_id)
                    .cmd("PUBLISH").arg(&done_channel).arg(&result.job_id);
                p
            },
        ).await;

        if ok {
            tracing::info!("Job {} completed and result stored in Redis", result.job_id);
        } else {
            tracing::error!(
                "Failed to publish result for job {} after retries; entry left unacked — the reclaim sweep (XAUTOCLAIM) will recover it",
                result.job_id
            );
        }
    }

    /// A pending entry we reclaimed already had its result stored (the original worker landed SET
    /// but not XACK, or its retry won a beat before this sweep). Don't re-run — just clear the
    /// pending marker, ACK to drain the PEL, and re-ring the bell so any live waiter wakes.
    async fn ack_existing(
        client: &redis::Client,
        stream_key: &str,
        consumer_group: &str,
        msg_id: &str,
        job_id: &str,
    ) {
        let pending_key = format!("judge:pending:{}", job_id);
        let done_channel = format!("judge:done:{}", job_id);
        let ok = Self::run_pipe_with_retry(
            client,
            &format!("re-ack existing result for job {}", job_id),
            || {
                let mut p = redis::pipe();
                p.cmd("DEL").arg(&pending_key)
                    .cmd("XACK").arg(stream_key).arg(consumer_group).arg(msg_id)
                    .cmd("PUBLISH").arg(&done_channel).arg(job_id);
                p
            },
        ).await;
        if ok {
            tracing::info!("Reclaimed job {} already had a stored result — re-acked and re-rang the bell (no re-run)", job_id);
        }
    }

    /// Execute a freshly-built pipe with reconnect-retry (fresh connection each attempt). The
    /// builder is called once per attempt because a `Pipeline` is consumed by `query_async`.
    /// Returns true on success. ~12 attempts, backoff 200ms·n capped at 2s ≈ 20s total.
    async fn run_pipe_with_retry(
        client: &redis::Client,
        describe: &str,
        build: impl Fn() -> redis::Pipeline,
    ) -> bool {
        const MAX_ATTEMPTS: u32 = 12;
        for attempt in 1..=MAX_ATTEMPTS {
            let res: redis::RedisResult<()> = async {
                let mut con = client.get_multiplexed_tokio_connection().await?;
                build().query_async(&mut con).await
            }.await;

            match res {
                Ok(()) => return true,
                Err(e) => {
                    if attempt == MAX_ATTEMPTS {
                        tracing::error!("{} failed after {} attempts: {}", describe, attempt, e);
                        return false;
                    }
                    let backoff_ms = std::cmp::min(2000, 200 * attempt as u64);
                    tracing::warn!(
                        "{} failed (attempt {}/{}): {}. Retrying in {}ms",
                        describe, attempt, MAX_ATTEMPTS, e, backoff_ms
                    );
                    tokio::time::sleep(tokio::time::Duration::from_millis(backoff_ms)).await;
                }
            }
        }
        false
    }

    /// Periodic XAUTOCLAIM sweep: reclaim consumer-group PEL entries idle longer than `min_idle_ms`
    /// (their original worker died/stalled before ACKing) and drive them to completion. For each
    /// reclaimed entry we GET the result first — if it's already stored (mid-pipe drop or a retry
    /// that just won), we only re-ACK + re-ring; otherwise we re-run it. Parse-failures are ACKed to
    /// drop them from the PEL. Runs on its own dedicated connection.
    ///
    /// Known bounded waste (documented, not fixed): a *legitimate* job running longer than
    /// `min_idle_ms` will be reclaimed and re-run in parallel once per sweep window (each XAUTOCLAIM
    /// resets the entry's idle clock, so copies can stack for pathologically long jobs). Results are
    /// idempotent so this is only wasted compute, never wrong; keep `min_idle_ms` above the worst-case
    /// legit runtime. A dead-letter after N deliveries would cap it — left as future work.
    #[allow(clippy::too_many_arguments)]
    async fn run_reclaim(
        client: redis::Client,
        pool: Arc<JudgeWorkerPool>,
        stream_key: String,
        consumer_group: String,
        consumer_name: String,
        semaphore: Arc<tokio::sync::Semaphore>,
        result_ttl: u64,
        min_idle_ms: u64,
        interval: tokio::time::Duration,
    ) {
        // Dedicated connection for the sweep so it never contends with the main read loop.
        let mut con = loop {
            match client.get_multiplexed_tokio_connection().await {
                Ok(c) => break c,
                Err(e) => {
                    tracing::debug!("Reclaim sweep waiting for Redis: {}", e);
                    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
                }
            }
        };

        tracing::info!(
            "Orphan-reclaim sweep started (min_idle={}ms, interval={:?}) as {}",
            min_idle_ms, interval, consumer_name
        );

        // XAUTOCLAIM cursor: "0-0" scans the PEL from the start; the reply hands back the cursor to
        // continue a large scan, returning to "0-0" when a full pass completes.
        let mut cursor = "0-0".to_string();

        loop {
            tokio::time::sleep(interval).await;

            let reply: redis::RedisResult<redis::Value> = redis::cmd("XAUTOCLAIM")
                .arg(&stream_key)
                .arg(&consumer_group)
                .arg(&consumer_name)
                .arg(min_idle_ms)
                .arg(&cursor)
                .arg("COUNT")
                .arg(32)
                .query_async(&mut con)
                .await;

            let items = match reply {
                Ok(redis::Value::Bulk(items)) if items.len() >= 2 => items,
                Ok(_) => { cursor = "0-0".to_string(); continue; }
                Err(e) => {
                    tracing::warn!("XAUTOCLAIM sweep failed: {}. Reconnecting...", e);
                    cursor = "0-0".to_string();
                    if let Ok(c) = client.get_multiplexed_tokio_connection().await {
                        con = c;
                    }
                    continue;
                }
            };

            // items[0] = next cursor; items[1] = claimed messages [[id, [k,v,...]], ...].
            // (items[2], if present, is IDs already deleted from the stream — XAUTOCLAIM has
            //  removed those from the PEL for us, so no action needed.)
            cursor = match &items[0] {
                redis::Value::Data(b) => String::from_utf8_lossy(b).to_string(),
                _ => "0-0".to_string(),
            };

            let messages = match &items[1] {
                redis::Value::Bulk(m) => m,
                _ => continue,
            };

            let mut reclaimed = 0usize;
            for msg in messages {
                let msg_data = match msg {
                    redis::Value::Bulk(d) if d.len() >= 2 => d,
                    _ => continue,
                };
                let msg_id = match &msg_data[0] {
                    redis::Value::Data(b) => String::from_utf8_lossy(b).to_string(),
                    _ => continue,
                };

                let request = match Self::extract_job_field(&msg_data[1])
                    .and_then(|j| serde_json::from_str::<JobRequest>(&j).ok())
                {
                    Some(r) => r,
                    None => {
                        // Unrecoverable (deleted/garbled payload): ACK to drop it from the PEL so it
                        // isn't reclaimed on every sweep forever.
                        tracing::warn!("Reclaimed message {} has no valid job payload; ACKing to discard", msg_id);
                        let _: redis::RedisResult<redis::Value> = redis::cmd("XACK")
                            .arg(&stream_key).arg(&consumer_group).arg(&msg_id)
                            .query_async(&mut con).await;
                        continue;
                    }
                };

                // Cheap GET-before-rerun: the work may already be done (SET landed, XACK didn't).
                let result_key = format!("judge:results:{}", request.job_id);
                let existing: Option<String> = redis::cmd("GET")
                    .arg(&result_key)
                    .query_async(&mut con)
                    .await
                    .ok()
                    .flatten();

                if existing.is_some() {
                    Self::ack_existing(&client, &stream_key, &consumer_group, &msg_id, &request.job_id).await;
                    reclaimed += 1;
                    continue;
                }

                // Truly lost → re-run under the same concurrency cap as fresh jobs.
                let permit = match semaphore.clone().acquire_owned().await {
                    Ok(p) => p,
                    Err(_) => break, // semaphore closed → shutting down
                };
                tracing::warn!("Reclaiming orphaned job {} (msg {}) via XAUTOCLAIM — re-running", request.job_id, msg_id);
                Self::spawn_process(
                    pool.clone(),
                    stream_key.clone(),
                    consumer_group.clone(),
                    client.clone(),
                    msg_id,
                    request,
                    permit,
                    result_ttl,
                );
                reclaimed += 1;
            }

            if reclaimed > 0 {
                tracing::info!("Reclaim sweep recovered {} orphaned job(s)", reclaimed);
            }
        }
    }
}
