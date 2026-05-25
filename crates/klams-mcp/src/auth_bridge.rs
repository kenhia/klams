//! Auth bridge for the MCP server (sprint 007 T018).
//!
//! Reuses the same `Arc<Vec<TokenGrant>>` that backs the REST surface
//! in `klams-api::auth`. The MCP server is mounted under the same axum
//! router stack, so by the time a request reaches a tool handler the
//! [`klams_api::auth::AuthenticatedScopes`] extension is already set by
//! the shared `require_bearer` middleware.
//!
//! [`scopes_from`] is the helper tools use to pull the scope set out of
//! the request extensions and decide whether to advertise / dispatch a
//! given tool.

use axum::extract::Request;
use klams_api::auth::AuthenticatedScopes;
use klams_types::Scope;
use std::sync::Arc;

/// Extract the caller's [`Scope`] set from the request extensions, or
/// `None` if the request slipped through without authentication
/// (should never happen on the protected `/mcp` mount; treated as
/// unauthorized by the caller).
#[must_use]
pub fn scopes_from(req: &Request) -> Option<Arc<Vec<Scope>>> {
    req.extensions()
        .get::<AuthenticatedScopes>()
        .map(|s| s.0.clone())
}

/// True iff the caller holds `needed`.
#[must_use]
pub fn has_scope(scopes: &[Scope], needed: Scope) -> bool {
    scopes.iter().any(|s| s.satisfies(needed))
}
