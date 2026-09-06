pub mod handlers;
pub mod websocket;
pub mod result_bus;

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
    /// Async result "buzzer" — `Some` iff a Redis cluster is configured. Powers the event-driven
    /// wait for `POST /submit`, the async endpoints, and the WebSocket result push.
    pub result_bus: Option<Arc<result_bus::ResultBus>>,
}

/// Constant-time byte comparison for the API secret.
///
/// `a != b` on `&str` short-circuits at the first differing byte, so response latency leaks a
/// prefix-match oracle that lets an attacker recover the secret byte by byte. This compares every
/// byte unconditionally. The length check is deliberately NOT constant-time: secret *length* is
/// far less useful to an attacker than its contents, and hiding it needs a hash-based compare.
fn secret_eq(provided: &[u8], expected: &[u8]) -> bool {
    if provided.len() != expected.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (a, b) in provided.iter().zip(expected.iter()) {
        diff |= a ^ b;
    }
    diff == 0
}

/// Normalize a configured API secret: an empty or whitespace-only value is treated as ABSENT.
///
/// `JUDGE_SECRET=""` is a *worse* state than no secret at all. `Some("")` still takes the auth
/// branch, so a request with no header is rejected (the judge looks protected) while a request
/// sending an empty `X-Judge-Secret` is accepted — an open judge wearing a lock. It is also the
/// state a deployment falls into by accident: `-e JUDGE_SECRET=${JUDGE_SECRET}` in a systemd unit
/// with no `Environment=`/`EnvironmentFile=` expands to empty, because systemd does not inherit
/// the invoking shell's environment.
///
/// Collapsing it to `None` makes the judge honestly unauthenticated (loud warning, and a hard
/// error under `JUDGE_REQUIRE_AUTH`) instead of silently bypassable. A secret that merely has
/// surrounding whitespace is preserved verbatim — only an entirely blank value is discarded.
pub fn normalize_secret(secret: Option<String>) -> Option<String> {
    secret.filter(|s| !s.trim().is_empty())
}

/// True when `JUDGE_REQUIRE_AUTH` is set to a truthy value. When set, starting without a
/// `JUDGE_SECRET` is a hard configuration error rather than a silently open judge.
pub fn require_auth_enabled() -> bool {
    std::env::var("JUDGE_REQUIRE_AUTH")
        .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
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

        let authorized = provided
            .map(|p| secret_eq(p.as_bytes(), expected_secret.as_bytes()))
            .unwrap_or(false);

        if !authorized {
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

pub async fn create_router(
    pool: Arc<JudgeWorkerPool>,
    secret: Option<String>,
    redis_url: Option<String>,
    enabled_languages: Option<Arc<std::collections::HashSet<crate::languages::SupportedLanguage>>>,
) -> Router {
    // Spawn the async result bus when a cluster is configured (drives the buzzer + event-driven
    // sync wait). None in local-only mode → async endpoints return 503, sync /submit uses the pool.
    let result_bus = match &redis_url {
        Some(u) => result_bus::ResultBus::spawn(u).await,
        None => None,
    };

    let secret = normalize_secret(secret);

    if secret.is_none() {
        tracing::warn!(
            "AUTHENTICATION IS DISABLED — no JUDGE_SECRET is set, so every /api/v1 endpoint is \
             open to anyone who can reach this port. This judge executes arbitrary submitted \
             code: do NOT expose it on a public interface in this state. Set JUDGE_SECRET, and \
             set JUDGE_REQUIRE_AUTH=1 to make starting without one a hard error."
        );
    }

    let state = Arc::new(ApiState {
        pool,
        secret,
        start_time: std::time::Instant::now(),
        redis_url,
        enabled_languages,
        result_bus,
    });

    let protected_routes = Router::new()
        .route("/api/v1/submit", post(handlers::submit))
        .route("/api/v1/submit/async", post(handlers::submit_async))
        .route("/api/v1/result/:job_id", get(handlers::get_result))
        .route("/api/v1/ws/execute", get(websocket::handle_websocket))
        .route("/api/v1/ws/result/:job_id", get(websocket::handle_result_ws))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_secret_discards_blank_values() {
        // The bug this guards: `Some("")` takes the auth branch, so a request with NO header is
        // rejected while a request with an EMPTY header is accepted — auth that looks enabled and
        // is trivially bypassable.
        assert_eq!(normalize_secret(None), None);
        assert_eq!(normalize_secret(Some(String::new())), None);
        assert_eq!(normalize_secret(Some("   ".into())), None);
        assert_eq!(normalize_secret(Some("\t\n".into())), None);
    }

    #[test]
    fn normalize_secret_preserves_real_secrets_verbatim() {
        assert_eq!(normalize_secret(Some("s3cr3t".into())), Some("s3cr3t".into()));
        // Surrounding whitespace is part of the secret, not something to silently trim away:
        // trimming would make a configured secret and a differently-padded one interchangeable.
        assert_eq!(normalize_secret(Some(" pad ".into())), Some(" pad ".into()));
    }

    #[test]
    fn secret_eq_matches_only_exact_bytes() {
        assert!(secret_eq(b"token", b"token"));
        assert!(!secret_eq(b"token", b"tokeN"));
        assert!(!secret_eq(b"tok", b"token"));
        assert!(!secret_eq(b"", b"token"));
        assert!(!secret_eq(b"token", b""));
        assert!(secret_eq(b"", b""));
    }
}
