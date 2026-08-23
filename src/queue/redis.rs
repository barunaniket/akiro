use redis::{streams::StreamReadOptions, Commands};
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
        
        let mut con = {
            let mut retries = 0;
            loop {
                match client.get_connection() {
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
        let _: Result<(), _> = con.xgroup_create_mkstream(&self.stream_key, &self.consumer_group, "$");
        // Ignore BUSYGROUP errors

        tracing::info!(
            "Redis consumer started: {} on group {} as {}",
            self.stream_key,
            self.consumer_group,
            self.consumer_name
        );

        loop {
            // Read messages from consumer group
            let opts = StreamReadOptions::default()
                .group(&self.consumer_group, &self.consumer_name)
                .count(10)
                .block(2000);

            let response: Result<redis::streams::StreamReadReply, _> =
                con.xread_options(&[&self.stream_key], &[">"], &opts);

            match response {
                Ok(reply) => {
                    for stream_key in reply.keys {
                        for stream_id in stream_key.ids {
                            let msg_id = stream_id.id;
                            
                            // Check for "job" field in message map
                            let job_json = match stream_id.map.get("job") {
                                Some(redis::Value::Data(bytes)) => std::str::from_utf8(bytes).ok().map(|s| s.to_string()),
                                Some(redis::Value::Status(s)) => Some(s.clone()),
                                _ => None,
                            };

                            if let Some(json_str) = job_json {
                                match serde_json::from_str::<JobRequest>(&json_str) {
                                    Ok(request) => {
                                        tracing::info!("Processing job {} from Redis", request.job_id);

                                        // Execute job through worker pool
                                        match self.pool.submit(request.clone(), None).await {
                                            Ok(result) => {
                                                // Write result back to Redis
                                                let result_key = format!("judge:results:{}", request.job_id);
                                                let result_json = serde_json::to_string(&result).unwrap_or_default();
                                                let _: Result<(), _> =
                                                    con.set_ex(&result_key, result_json, 86400); // 24h TTL

                                                // Acknowledge message
                                                let _: Result<(), _> =
                                                    con.xack(&self.stream_key, &self.consumer_group, &[&msg_id]);

                                                tracing::info!("Job {} completed and result stored", request.job_id);
                                            }
                                            Err(e) => {
                                                tracing::error!("Job {} failed: {}", request.job_id, e);
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        tracing::error!("Failed to parse job JSON from Redis: {}", e);
                                    }
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("Redis stream read warning: {}. Attempting to reconnect...", e);
                    tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;

                    // Automatically re-establish connection on broken pipe or disconnect
                    match client.get_connection() {
                        Ok(new_con) => {
                            con = new_con;
                            tracing::info!("Reconnected to Redis successfully ✓");
                            // Ensure consumer group exists
                            let _: Result<(), _> = con.xgroup_create_mkstream(&self.stream_key, &self.consumer_group, "$");
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
