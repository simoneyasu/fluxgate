//! FluxGate's reusable application library.
//!
//! Keeping the router and configuration outside `main.rs` makes the service
//! straightforward to exercise in-process without binding a real TCP port.

pub mod api;
pub mod config;
pub mod limiter;
pub mod policies;
pub mod service;
pub mod storage;
pub mod telemetry;

use api::AppState;
use axum::{http::HeaderName, Router};
use tower_http::{
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    trace::TraceLayer,
};

/// Builds the complete HTTP application.
pub fn app(state: AppState) -> Router {
    Router::new()
        .merge(api::router(state))
        .layer(PropagateRequestIdLayer::new(HeaderName::from_static(
            "x-request-id",
        )))
        .layer(SetRequestIdLayer::new(
            HeaderName::from_static("x-request-id"),
            MakeRequestUuid,
        ))
        .layer(TraceLayer::new_for_http())
}
