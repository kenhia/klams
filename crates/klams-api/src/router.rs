//! HTTP router skeleton.
//!
//! All write/read endpoints return `501 Not Implemented` until the
//! handler tasks (T037+) land. The router does enforce auth (T030)
//! and exposes `/healthz` + `/metrics` publicly.

use crate::auth::AuthState;
use crate::handlers;
use axum::{
    middleware,
    routing::{get, post},
    Router,
};
use axum_prometheus::PrometheusMetricLayer;
use klams_core::{MemoryQueue, ValidatorRegistry};
use klams_store::Store;
use std::sync::Arc;
use tower_http::trace::TraceLayer;

/// Shared application state injected into every handler.
pub struct ApiState<S: Store> {
    pub store: Arc<S>,
    pub queue: MemoryQueue,
    pub queue_capacity: usize,
    pub workers: usize,
    pub started_at: std::time::Instant,
    pub validators: Arc<ValidatorRegistry>,
}

impl<S: Store> Clone for ApiState<S> {
    fn clone(&self) -> Self {
        Self {
            store: Arc::clone(&self.store),
            queue: self.queue.clone(),
            queue_capacity: self.queue_capacity,
            workers: self.workers,
            started_at: self.started_at,
            validators: Arc::clone(&self.validators),
        }
    }
}

impl<S: Store> std::fmt::Debug for ApiState<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiState")
            .field("store", &"<dyn Store>")
            .field("queue", &self.queue)
            .field("queue_capacity", &self.queue_capacity)
            .field("workers", &self.workers)
            .field("started_at", &self.started_at)
            .finish_non_exhaustive()
    }
}

/// Build the protected + public routes. Does **not** install any
/// global metrics recorder (axum-prometheus uses a process-global,
/// which would break parallel tests). Use [`with_metrics`] in the
/// binary to add `/metrics` and the prometheus layer.
pub fn build_router<S: Store>(state: ApiState<S>, bearer_token: impl Into<String>) -> Router {
    let auth_state = AuthState::new(bearer_token);

    let protected = Router::new()
        .route(
            "/memory/facts",
            post(handlers::facts::upsert::<S>).get(handlers::facts::list::<S>),
        )
        .route(
            "/memory/events",
            post(handlers::events::append::<S>).get(handlers::events::list::<S>),
        )
        .route(
            "/memory/knowledge/index",
            post(handlers::knowledge::index::<S>),
        )
        .route(
            "/memory/knowledge/delete",
            post(handlers::knowledge::delete::<S>),
        )
        .route("/memory/knowledge/:id", get(handlers::knowledge::get::<S>))
        .route("/memory/search", post(handlers::search::search::<S>))
        .route("/memory/context", post(handlers::context::context::<S>))
        .route("/memory/policy", get(handlers::policy::get_policy))
        .route("/memory/dissents", get(handlers::dissents::list))
        .route("/memory/dissents/:id", get(handlers::dissents::get))
        .route(
            "/memory/dissents/:id/promote",
            post(handlers::dissents::promote),
        )
        .route(
            "/memory/dissents/:id/discard",
            post(handlers::dissents::discard),
        )
        .with_state(state.clone())
        .layer(middleware::from_fn_with_state(
            auth_state,
            crate::auth::require_bearer,
        ));

    let public = Router::new()
        .route("/healthz", get(handlers::health::healthz::<S>))
        .with_state(state);

    Router::new()
        .merge(protected)
        .merge(public)
        .layer(TraceLayer::new_for_http())
}

/// Add the axum-prometheus layer and a public `/metrics` route.
/// Call **once per process** (typically from `main`). Panics if the
/// global metrics recorder is already installed.
pub fn with_metrics(router: Router) -> Router {
    let (prometheus_layer, metric_handle) = PrometheusMetricLayer::pair();
    router
        .route(
            "/metrics",
            get(move || async move { metric_handle.render() }),
        )
        .layer(prometheus_layer)
}
