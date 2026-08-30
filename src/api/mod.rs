pub mod handlers;
pub mod websocket;

use axum::{
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{Json, Response},
    routing::{get, post},
    Router,
};
use std::sync::Arc;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

/// Build the CORS policy from `JUDGE_CORS_ALLOW_ORIGIN`:
///   - unset          -> permissive (any origin) + a loud startup warning
///   - `*`            -> permissive (explicit opt-in), no warning
///   - comma list     -> strict allow-list of exactly those origins
///
/// The default stays permissive so an existing browser frontend is not silently
/// broken on upgrade; production deployments should set an explicit origin list.
fn build_cors_layer() -> CorsLayer {
    use axum::http::{header, HeaderName, HeaderValue, Method};

    match std::env::var("JUDGE_CORS_ALLOW_ORIGIN") {
        Ok(v) if v.trim() == "*" => CorsLayer::permissive(),
        Ok(v) if !v.trim().is_empty() => {
            let origins: Vec<HeaderValue> = v
                .split(',')
                .filter_map(|o| o.trim().parse::<HeaderValue>().ok())
                .collect();
            tracing::info!("CORS restricted to {} configured origin(s)", origins.len());
            CorsLayer::new()
                .allow_origin(origins)
                .allow_methods([Method::GET, Method::POST])
                .allow_headers([
                    header::CONTENT_TYPE,
                    header::AUTHORIZATION,
                    HeaderName::from_static("x-judge-secret"),
                ])
        }
        _ => {
            tracing::warn!(
                "CORS is permissive (any origin can call this judge). Set JUDGE_CORS_ALLOW_ORIGIN \
                 to a comma-separated allow-list to restrict browser access."
            );
            CorsLayer::permissive()
        }
    }
}

use crate::orchestrator::JudgeWorkerPool;

pub struct ApiState {
    pub pool: Arc<JudgeWorkerPool>,
    pub secret: Option<String>,
    pub start_time: std::time::Instant,
    pub redis_url: Option<String>,
    pub enabled_languages: Option<Arc<std::collections::HashSet<crate::languages::SupportedLanguage>>>,
}

async fn auth_middleware(
    State(state): State<Arc<ApiState>>,
    headers: HeaderMap,
    request: Request,
    next: Next,
) -> Result<Response, (StatusCode, Json<serde_json::Value>)> {
    if let Some(expected_secret) = &state.secret {
        let provided = headers
            .get("x-judge-secret")
            .and_then(|v| v.to_str().ok())
            .or_else(|| {
                headers
                    .get("authorization")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.strip_prefix("Bearer "))
            });

        if provided != Some(expected_secret.as_str()) {
            return Err((
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({
                    "error": "Unauthorized: invalid or missing X-Judge-Secret or Bearer token"
                })),
            ));
        }
    }
    Ok(next.run(request).await)
}

pub fn create_router(
    pool: Arc<JudgeWorkerPool>,
    secret: Option<String>,
    redis_url: Option<String>,
    enabled_languages: Option<Arc<std::collections::HashSet<crate::languages::SupportedLanguage>>>,
) -> Router {
    let state = Arc::new(ApiState {
        pool,
        secret,
        start_time: std::time::Instant::now(),
        redis_url,
        enabled_languages,
    });

    let protected_routes = Router::new()
        .route("/api/v1/submit", post(handlers::submit))
        .route("/api/v1/ws/execute", get(websocket::handle_websocket))
        .layer(middleware::from_fn_with_state(state.clone(), auth_middleware));

    let public_routes = Router::new()
        .route("/health", get(handlers::health))
        .route("/metrics", get(handlers::metrics));

    Router::new()
        .merge(protected_routes)
        .merge(public_routes)
        .layer(axum::extract::DefaultBodyLimit::max(2 * 1024 * 1024))
        .layer(build_cors_layer())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
