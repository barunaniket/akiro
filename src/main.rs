use clap::{Parser, ValueEnum};
use akiro::{api, queue::redis::RedisConsumer, JobEnvelope, JudgeWorkerPool};
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

#[cfg(not(target_env = "msvc"))]
use tikv_jemallocator::Jemalloc;

#[cfg(not(target_env = "msvc"))]
#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;

#[derive(Debug, Clone, ValueEnum)]
enum RunMode {
    /// Run only the HTTP/WebSocket REST gateway
    Server,
    /// Run only the Redis Streams worker daemon
    Worker,
    /// Run both API server and local worker pool
    All,
}

#[derive(Parser, Debug)]
#[command(name = "Akiro")]
#[command(about = "High-performance online judge executor with Rust")]
struct Args {
    /// Runtime mode: server, worker, or all
    #[arg(long, env = "JUDGE_MODE", value_enum, default_value = "all")]
    mode: RunMode,

    /// HTTP server port
    #[arg(long, env = "JUDGE_PORT", default_value = "8080")]
    port: u16,

    /// Optional API secret key for securing HTTP endpoints
    #[arg(long, env = "JUDGE_SECRET")]
    secret: Option<String>,

    /// Optional cluster token for securing Redis broker / worker connections
    #[arg(long, env = "CLUSTER_TOKEN")]
    token: Option<String>,

    /// Optional comma-separated list of enabled languages (e.g. "cpp,python,java,rust")
    #[arg(long, env = "ENABLED_LANGUAGES")]
    languages: Option<String>,

    /// Number of worker threads (e.g. 8, auto)
    #[arg(long, env = "JUDGE_WORKERS")]
    workers: Option<String>,

    /// Redis connection string for queue consumer
    #[arg(long, env = "JUDGE_REDIS", default_value = "redis://127.0.0.1:6379")]
    redis: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    #[cfg(target_os = "linux")]
    unsafe {
        libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0);
    }

    // Single background reaper: the sole wait4 caller. It reaps every terminated child —
    // routing supervised children's exit status to their ProcessSupervisor (accurate
    // cpu/memory + MLE/TLE/RE) and discarding orphaned grandchildren (no zombies).
    #[cfg(target_os = "linux")]
    tokio::spawn(async {
        if let Ok(mut sigchld) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::child()) {
            while sigchld.recv().await.is_some() {
                akiro::sandbox::reaper::reap_all();
            }
        }
    });

    let enabled_languages = args.languages.as_deref().map(|l| {
        let set = akiro::languages::SupportedLanguage::parse_whitelist(l);
        let names: Vec<&str> = set.iter().map(|s| s.as_str()).collect();
        tracing::info!("Language whitelist enabled ({} languages): {:?}", set.len(), names);
        Arc::new(set)
    });

    let num_workers = match args.workers.as_deref() {
        Some("auto") | Some("") | None => None,
        Some(s) => match s.parse::<usize>() {
            Ok(n) => Some(n),
            Err(_) => {
                tracing::warn!("Invalid worker count '{}', falling back to auto-detection", s);
                None
            }
        },
    };

    let (pool, receiver) = JudgeWorkerPool::new(num_workers);
    let pool = Arc::new(pool);

    match args.mode {
        RunMode::Server => {
            tracing::info!("Starting Akiro in SERVER mode on port {}", args.port);
            run_server(pool, receiver, args.port, args.secret, enabled_languages).await?;
        }
        RunMode::Worker => {
            tracing::info!("Starting Akiro in WORKER mode (Redis consumer)");
            run_worker(pool, receiver, &args.redis).await?;
        }
        RunMode::All => {
            tracing::info!("Starting Akiro in ALL mode (server + worker pool)");
            run_all(pool, receiver, args.port, args.secret, &args.redis, enabled_languages).await?;
        }
    }

    Ok(())
}

async fn run_server(
    pool: Arc<JudgeWorkerPool>,
    receiver: tokio::sync::mpsc::UnboundedReceiver<JobEnvelope>,
    port: u16,
    secret: Option<String>,
    enabled_languages: Option<Arc<std::collections::HashSet<akiro::languages::SupportedLanguage>>>,
) -> Result<(), Box<dyn std::error::Error>> {
    // Spawn worker tasks
    let pool_workers = pool.clone();
    tokio::spawn(async move {
        pool_workers.run_workers(receiver).await;
    });

    // Start HTTP server
    let router = api::create_router(pool, secret, None, enabled_languages).await;
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port)).await?;

    tracing::info!("HTTP server listening on port {}", port);
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

async fn run_worker(
    pool: Arc<JudgeWorkerPool>,
    receiver: tokio::sync::mpsc::UnboundedReceiver<JobEnvelope>,
    redis_url: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    // Spawn worker tasks for local processing
    let pool_workers = pool.clone();
    tokio::spawn(async move {
        pool_workers.run_workers(receiver).await;
    });

    // Start Redis consumer
    let consumer = RedisConsumer::new(redis_url.to_string(), pool, None, None);
    consumer.run().await?;

    Ok(())
}

async fn run_all(
    pool: Arc<JudgeWorkerPool>,
    receiver: tokio::sync::mpsc::UnboundedReceiver<JobEnvelope>,
    port: u16,
    secret: Option<String>,
    redis_url: &str,
    enabled_languages: Option<Arc<std::collections::HashSet<akiro::languages::SupportedLanguage>>>,
) -> Result<(), Box<dyn std::error::Error>> {
    // Spawn worker tasks
    let pool_workers = pool.clone();
    tokio::spawn(async move {
        pool_workers.run_workers(receiver).await;
    });

    // Start HTTP server
    let pool_http = pool.clone();
    let redis_url_clone = redis_url.to_string();
    tokio::spawn(async move {
        let router = api::create_router(pool_http, secret, Some(redis_url_clone), enabled_languages).await;
        let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port))
            .await
            .expect("Failed to bind HTTP listener");
        tracing::info!("HTTP server listening on port {}", port);
        let _ = axum::serve(listener, router)
            .with_graceful_shutdown(shutdown_signal())
            .await;
    });

    // Start Redis consumer
    let consumer = RedisConsumer::new(redis_url.to_string(), pool, None, None);
    consumer.run().await?;

    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("Failed to install CTRL+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("Failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {
            tracing::info!("Received CTRL+C, shutting down...");
        }
        _ = terminate => {
            tracing::info!("Received SIGTERM, shutting down...");
        }
    }
}
