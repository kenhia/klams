//! Streamable HTTP transport for the klams MCP server (sprint 007 T017).
//!
//! Returns a [`StreamableHttpService`] configured for in-process use
//! by `klams-service`. The HTTP+SSE fallback contemplated in R-001 is
//! served by the same Streamable HTTP mount point — rmcp 1.7's
//! `StreamableHttpService` handles both transport modes natively, so
//! no second mount or feature flag is required.

use crate::tools::{McpState, ToolRegistry};
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
};
use std::sync::Arc;

/// Build the Streamable HTTP service that the axum router mounts.
///
/// `allowed_hosts` is the Host-header allowlist for rmcp's DNS-rebinding
/// protection. Pass an empty `Vec` to disable the check entirely
/// (relying on `require_bearer` for access control); pass a list to
/// restrict.
///
/// The `service_factory` closure is invoked once per session by rmcp,
/// so it must be cheap to clone the underlying state.
#[must_use]
pub fn streamable_http_service(
    state: McpState,
    allowed_hosts: Vec<String>,
) -> StreamableHttpService<ToolRegistry, LocalSessionManager> {
    let mut config = StreamableHttpServerConfig::default();
    config.stateful_mode = true;
    config.json_response = false;
    config.allowed_hosts = allowed_hosts;
    let factory_state = state;
    StreamableHttpService::new(
        move || Ok(ToolRegistry::new(factory_state.clone())),
        Arc::new(LocalSessionManager::default()),
        config,
    )
}
