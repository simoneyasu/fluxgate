use crate::{
    policies::PolicyRegistry,
    service::{RateLimiter, RateLimiterError},
};
use axum::{
    extract::{rejection::JsonRejection, Path, State},
    http::{header::RETRY_AFTER, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Extension, Json, Router,
};
use serde::{Deserialize, Serialize};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tower_http::request_id::RequestId;

#[derive(Clone)]
pub struct AppState {
    pub limiter: RateLimiter,
    pub policies: PolicyRegistry,
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    status: &'static str,
}

#[derive(Debug, Deserialize)]
struct CheckRequest {
    key: String,
    #[serde(default = "default_cost")]
    cost: u64,
}

#[derive(Debug, Deserialize, Serialize)]
struct CheckResponse {
    allowed: bool,
    limit: u64,
    remaining: u64,
    reset_after_ms: u64,
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    code: &'static str,
    message: String,
}

enum ApiError {
    InvalidJson,
    UnknownPolicy(String),
    InvalidRequest(String),
    Unavailable,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/v1/check", post(check_default))
        .route("/v1/check/{policy}", post(check_named))
        .with_state(state)
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

async fn check_default(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    payload: Result<Json<CheckRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    check_policy(state, request_id, "default".to_owned(), payload).await
}

async fn check_named(
    State(state): State<AppState>,
    Extension(request_id): Extension<RequestId>,
    Path(policy): Path<String>,
    payload: Result<Json<CheckRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    check_policy(state, request_id, policy, payload).await
}

async fn check_policy(
    state: AppState,
    request_id: RequestId,
    policy_name: String,
    payload: Result<Json<CheckRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let Json(request) = payload.map_err(|_| ApiError::InvalidJson)?;
    let policy = state
        .policies
        .get(&policy_name)
        .ok_or_else(|| ApiError::UnknownPolicy(policy_name.clone()))?;
    let now_ms = epoch_millis().map_err(|_| ApiError::Unavailable)?;
    let started = Instant::now();
    let request_id = request_id
        .header_value()
        .to_str()
        .unwrap_or("non-utf8-request-id");
    let decision = state
        .limiter
        .check(&policy_name, &request.key, &policy, request.cost, now_ms)
        .await
        .map_err(|error| {
            tracing::error!(
                request_id,
                policy = policy_name,
                error = %error,
                "rate limit evaluation failed"
            );
            ApiError::from(error)
        })?;

    tracing::info!(
        request_id,
        policy = policy_name,
        allowed = decision.allowed(),
        remaining = decision.remaining(),
        cost = request.cost,
        latency_micros = started.elapsed().as_micros(),
        "rate limit evaluated"
    );

    let mut response = Json(CheckResponse {
        allowed: decision.allowed(),
        limit: decision.limit(),
        remaining: decision.remaining(),
        reset_after_ms: decision.reset_after_ms(),
    })
    .into_response();
    insert_number(
        response.headers_mut(),
        "x-ratelimit-limit",
        decision.limit(),
    );
    insert_number(
        response.headers_mut(),
        "x-ratelimit-remaining",
        decision.remaining(),
    );
    insert_number(
        response.headers_mut(),
        "x-ratelimit-reset-ms",
        decision.reset_after_ms(),
    );
    if let Some(retry_ms) = decision.retry_after_ms() {
        let retry_seconds = retry_ms.div_ceil(1_000).max(1);
        if let Ok(value) = HeaderValue::from_str(&retry_seconds.to_string()) {
            response.headers_mut().insert(RETRY_AFTER, value);
        }
    }
    Ok(response)
}

fn default_cost() -> u64 {
    1
}

fn epoch_millis() -> Result<u64, ()> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| ())?
        .as_millis();
    u64::try_from(millis).map_err(|_| ())
}

fn insert_number(headers: &mut HeaderMap, name: &'static str, value: u64) {
    if let Ok(value) = HeaderValue::from_str(&value.to_string()) {
        headers.insert(name, value);
    }
}

impl From<RateLimiterError> for ApiError {
    fn from(error: RateLimiterError) -> Self {
        match error {
            RateLimiterError::InvalidBucketId(error) => Self::InvalidRequest(error.to_string()),
            RateLimiterError::InvalidRequest(error) => Self::InvalidRequest(error.to_string()),
            RateLimiterError::Repository(_)
            | RateLimiterError::ExpiryOverflow
            | RateLimiterError::ContentionExhausted { .. } => Self::Unavailable,
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            Self::InvalidJson => (
                StatusCode::BAD_REQUEST,
                "invalid_json",
                "request body must be valid JSON with a string key and optional integer cost"
                    .to_owned(),
            ),
            Self::UnknownPolicy(policy) => (
                StatusCode::NOT_FOUND,
                "unknown_policy",
                format!("rate-limit policy {policy:?} does not exist"),
            ),
            Self::InvalidRequest(message) => (StatusCode::BAD_REQUEST, "invalid_request", message),
            Self::Unavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "service_unavailable",
                "rate limiter is temporarily unavailable".to_owned(),
            ),
        };
        (status, Json(ErrorBody { code, message })).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{app, storage::MemoryRepository};
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use std::sync::Arc;
    use tower::ServiceExt;

    fn test_app() -> Router {
        let limiter = RateLimiter::new(Arc::new(MemoryRepository::default()), 8, 86_400);
        app(AppState {
            limiter,
            policies: PolicyRegistry::built_in().expect("valid test policies"),
        })
    }

    fn json_request(uri: &str, body: &str) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(body.to_owned()))
            .expect("valid test request")
    }

    #[tokio::test]
    async fn health_returns_ok() {
        let response = test_app()
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn check_returns_decision_and_headers() {
        let response = test_app()
            .oneshot(json_request("/v1/check", r#"{"key":"user-1","cost":5}"#))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["x-ratelimit-limit"], "100");
        assert_eq!(response.headers()["x-ratelimit-remaining"], "95");
        assert!(response.headers().contains_key("x-request-id"));
    }

    #[tokio::test]
    async fn named_policy_uses_its_own_limit() {
        let response = test_app()
            .oneshot(json_request("/v1/check/login", r#"{"key":"user-1"}"#))
            .await
            .unwrap();
        assert_eq!(response.headers()["x-ratelimit-limit"], "10");
    }

    #[tokio::test]
    async fn denied_response_includes_retry_after() {
        let app = test_app();
        let first = app
            .clone()
            .oneshot(json_request(
                "/v1/check/login",
                r#"{"key":"empty","cost":10}"#,
            ))
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::OK);

        let response = app
            .oneshot(json_request("/v1/check/login", r#"{"key":"empty"}"#))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().contains_key(RETRY_AFTER));
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let body: CheckResponse = serde_json::from_slice(&bytes).unwrap();
        assert!(!body.allowed);
    }

    #[tokio::test]
    async fn invalid_and_unknown_requests_are_clean_errors() {
        let malformed = test_app()
            .oneshot(json_request("/v1/check", "not-json"))
            .await
            .unwrap();
        assert_eq!(malformed.status(), StatusCode::BAD_REQUEST);

        let unknown = test_app()
            .oneshot(json_request("/v1/check/missing", r#"{"key":"user"}"#))
            .await
            .unwrap();
        assert_eq!(unknown.status(), StatusCode::NOT_FOUND);

        let invalid_cost = test_app()
            .oneshot(json_request("/v1/check", r#"{"key":"user","cost":0}"#))
            .await
            .unwrap();
        assert_eq!(invalid_cost.status(), StatusCode::BAD_REQUEST);
    }
}
