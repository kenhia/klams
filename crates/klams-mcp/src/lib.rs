//! klams-mcp — Model Context Protocol surface for klams.
//!
//! This crate hosts the in-process MCP server mounted by `klams-service`.
//! The public entry point is [`router`], which returns an [`axum::Router`]
//! ready to nest under `/mcp` on the existing service router.
//!
//! The crate is a thin façade over `rmcp`'s `StreamableHttpService`. Tool
//! handlers, scoped auth, projection, soft-delete, and metrics each live in
//! their own module under `src/` (added in subsequent foundational tasks).

use axum::Router;

/// Build the axum sub-router that exposes the MCP endpoint.
///
/// At this scaffold stage the router is an empty placeholder; foundational
/// tasks T016–T024 replace it with the real `StreamableHttpService` mount
/// plus the scope-gated tool registry.
#[must_use]
pub fn router() -> Router {
    Router::new()
}
