//! klams-mcp — Model Context Protocol surface for klams.
//!
//! Mounts an in-process MCP server on the existing axum router served
//! by `klams-service`. The public entry point is [`router`], which
//! returns an [`axum::Router`] ready to nest under `/mcp`.
//!
//! Module map (see `sprints/007-mcp-server/`):
//! - [`auth_bridge`] — translates bearer scopes into
//!   per-tool gating decisions.
//! - [`errors`] — canonical MCP error codes & envelope helpers.
//! - [`maintenance`] — `MAINTENANCE_WINDOW_ACTIVE` short-circuit.
//! - [`metrics`] — Prometheus counters (cardinality-disciplined).
//! - [`projection`] — internal entities → `PublicMemory` (re-export;
//!   lives in `klams_core::projection` since sprint 036, #730).
//! - [`tools`] — `ServerHandler` + tool registry.
//! - [`transport`] — Streamable HTTP service construction.

pub mod auth_bridge;
pub mod errors;
pub mod maintenance;
pub mod metrics;
pub mod tools;
pub mod transport;

// Sprint 036 (#730): the projection layer moved to `klams-core` so the
// REST surface can project the same `PublicMemory` shapes without
// depending on this crate. Re-exported here so `crate::projection::`
// call sites (and any external users of the old path) keep working.
pub use klams_core::projection;

use axum::{
    body::Body,
    extract::Request,
    http::{header, Method, StatusCode},
    middleware::Next,
    response::Response,
    Router,
};

use crate::tools::McpState;

/// Build the axum sub-router that exposes the MCP endpoint.
///
/// Mount under `/mcp` *with* the same `require_bearer` layer wrapping
/// the parent application — see `klams-service::main` for the canonical
/// wiring. `allowed_hosts` is the rmcp DNS-rebinding Host-header
/// allowlist; pass an empty `Vec` to disable.
#[must_use = "the returned axum::Router must be mounted"]
pub fn router(state: McpState, allowed_hosts: Vec<String>) -> Router {
    let svc = transport::streamable_http_service(state, allowed_hosts);
    Router::new()
        .fallback_service(svc)
        .layer(axum::middleware::from_fn(delete_status_compat))
}

/// Rewrite session-termination DELETE responses from 202 Accepted to
/// 204 No Content (sprint 018, WI #305).
///
/// rmcp 1.7's `StreamableHttpService::handle_delete` hardcodes 202,
/// but the mcp python-sdk treats only 200/204 as successful
/// termination and logs `Session termination failed: 202` on every
/// session close. Only a DELETE that rmcp accepted (202) is rewritten;
/// error statuses (400 missing-session-id, 5xx) pass through.
pub async fn delete_status_compat(req: Request, next: Next) -> Response {
    let is_delete = req.method() == Method::DELETE;
    let resp = next.run(req).await;
    if !is_delete || resp.status() != StatusCode::ACCEPTED {
        return resp;
    }
    let (mut parts, _body) = resp.into_parts();
    parts.status = StatusCode::NO_CONTENT;
    parts.headers.remove(header::CONTENT_LENGTH);
    Response::from_parts(parts, Body::empty())
}
