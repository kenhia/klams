//! klams-mcp — Model Context Protocol surface for klams.
//!
//! Mounts an in-process MCP server on the existing axum router served
//! by `klams-service`. The public entry point is [`router`], which
//! returns an [`axum::Router`] ready to nest under `/mcp`.
//!
//! Module map (see `specs/007-mcp-server/`):
//! - [`auth_bridge`] — translates `klams_api` bearer scopes into
//!   per-tool gating decisions.
//! - [`errors`] — canonical MCP error codes & envelope helpers.
//! - [`maintenance`] — `MAINTENANCE_WINDOW_ACTIVE` short-circuit.
//! - [`metrics`] — Prometheus counters (cardinality-disciplined).
//! - [`projection`] — internal entities → `PublicMemory`.
//! - [`tools`] — `ServerHandler` + tool registry.
//! - [`transport`] — Streamable HTTP service construction.

pub mod auth_bridge;
pub mod errors;
pub mod maintenance;
pub mod metrics;
pub mod projection;
pub mod tools;
pub mod transport;

use axum::Router;

use crate::tools::McpState;

/// Build the axum sub-router that exposes the MCP endpoint.
///
/// Mount with `Router::new().nest_service("/mcp", klams_mcp::router(state))`.
/// All authentication is enforced by the parent router's
/// `klams_api::auth::require_bearer` middleware; this router assumes
/// requests reaching it have already had their bearer validated.
#[must_use = "the returned axum::Router must be mounted"]
pub fn router(state: McpState) -> Router {
    let svc = transport::streamable_http_service(state);
    Router::new().fallback_service(svc)
}
