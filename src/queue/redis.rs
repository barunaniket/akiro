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

                                                let mut job_json: Option<String> = None;
                                                if let redis::Value::Bulk(kvs) = &msg_data[1] {
                                                    let mut iter = kvs.iter();
                                                    while let (Some(k), Some(v)) = (iter.next(), iter.next()) {
                                                        if let redis::Value::Data(k_bytes) = k {
                                                            if String::from_utf8_lossy(k_bytes) == "job" {
                                                                if let redis::Value::Data(v_bytes) = v {
                                                                    job_json = Some(String::from_utf8_lossy(v_bytes).to_string());
                                                                }
                                                            }
                                                        }
                                                    }
                                                }

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

                                                            let pool = self.pool.clone();
                                                            let client_clone = client.clone();
                                                            let stream_key = self.stream_key.clone();
                                                            let consumer_group = self.consumer_group.clone();

                                                            tokio::spawn(async move {
                                                                tracing::info!("Processing job {} concurrently from Redis", request.job_id);

                                                                let res_to_publish = match pool.submit(request.clone(), None).await {
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

                                                                let result_key = format!("judge:results:{}", request.job_id);
                                                                let result_json = serde_json::to_string(&res_to_publish).unwrap_or_default();

                                                                if let Ok(mut task_con) = client_clone.get_multiplexed_tokio_connection().await {
                                                                    let _: Result<(), _> = redis::pipe()
                                                                        .cmd("SET").arg(&result_key).arg(&result_json).arg("EX").arg(86400)
                                                                        .cmd("XACK").arg(&stream_key).arg(&consumer_group).arg(&msg_id)
                                                                        .query_async(&mut task_con)
                                                                        .await;
                                                                    tracing::info!("Job {} completed and result stored in Redis", request.job_id);
                                                                } else {
                                                                    tracing::error!("Failed to acquire Redis connection to publish result for job {}", request.job_id);
                                                                }

                                                                drop(permit);
                                                            });
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
}
