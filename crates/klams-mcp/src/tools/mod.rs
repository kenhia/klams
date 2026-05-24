//! MCP tool registry (sprint 007 T023 + T032/T033).
//!
//! Houses the [`ToolRegistry`] [`rmcp::ServerHandler`] implementation
//! that backs `tools/list` and `tools/call`, plus the [`McpState`]
//! that carries shared backend handles into per-tool modules.

pub mod memory_add;
pub mod register_author;

use klams_api::auth::TokenGrant;
use klams_store::CompositeStore;
use klams_types::MaintenanceState;
use rmcp::{
    model::{
        CallToolRequestParams, CallToolResult, Content, ListToolsResult, PaginatedRequestParams,
        ProtocolVersion, ServerCapabilities, ServerInfo, Tool,
    },
    service::RequestContext,
    ErrorData as McpError, RoleServer, ServerHandler,
};
use std::sync::Arc;

/// Shared state injected into every tool handler.
#[derive(Clone)]
pub struct McpState {
    pub store: Arc<CompositeStore>,
    pub maintenance: Arc<MaintenanceState>,
    pub grants: Arc<Vec<TokenGrant>>,
}

impl std::fmt::Debug for McpState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpState")
            .field("grants", &self.grants.len())
            .finish_non_exhaustive()
    }
}

impl McpState {
    /// Canonical constructor used by `klams-service`.
    #[must_use]
    pub fn new(
        store: Arc<CompositeStore>,
        maintenance: Arc<MaintenanceState>,
        grants: Arc<Vec<TokenGrant>>,
    ) -> Self {
        Self {
            store,
            maintenance,
            grants,
        }
    }
}

/// `ServerHandler` implementation that exposes the klams MCP tools.
#[derive(Clone, Debug)]
pub struct ToolRegistry {
    state: McpState,
}

impl ToolRegistry {
    #[must_use]
    pub fn new(state: McpState) -> Self {
        Self { state }
    }
}

#[allow(clippy::needless_pass_by_value)]
impl ServerHandler for ToolRegistry {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        info.protocol_version = ProtocolVersion::default();
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info.server_info.name = "klams-mcp".into();
        info.server_info.version = env!("CARGO_PKG_VERSION").into();
        info.instructions = Some(
            "klams memory server. Call `register_author` once per session, \
             then use `memory_*` tools."
                .into(),
        );
        info
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let tools = vec![
            tool_descriptor::<register_author::RegisterAuthorInput>(
                "register_author",
                "Register the calling agent and obtain an author_id (UUID v7).",
            ),
            tool_descriptor::<memory_add::MemoryAddArgs>(
                "memory_add",
                "Persist a fact or knowledge memory attributed to an author_id.",
            ),
        ];
        let result = ListToolsResult {
            tools,
            ..ListToolsResult::default()
        };
        Ok(result)
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        tracing::info!(tool = %request.name, "mcp.tool dispatch");
        let name = request.name.as_ref();
        let args_value = request
            .arguments
            .map_or(serde_json::Value::Null, serde_json::Value::Object);
        match name {
            "register_author" => {
                let args = match serde_json::from_value(args_value) {
                    Ok(a) => a,
                    Err(e) => {
                        return Ok(envelope_result(&crate::errors::envelope(
                            crate::errors::INVALID_AGENT_NAME,
                            format!("invalid register_author arguments: {e}"),
                        )))
                    }
                };
                match register_author::run(&self.state, args).await {
                    Ok(out) => Ok(json_result(&out)),
                    Err(env) => Ok(envelope_result(&env)),
                }
            }
            "memory_add" => {
                let args = match serde_json::from_value(args_value) {
                    Ok(a) => a,
                    Err(e) => {
                        return Ok(envelope_result(&crate::errors::envelope(
                            crate::errors::SCHEMA_VALIDATION_FAILED,
                            format!("invalid memory_add arguments: {e}"),
                        )))
                    }
                };
                match memory_add::run(&self.state, args).await {
                    Ok(out) => Ok(json_result(&out)),
                    Err(env) => Ok(envelope_result(&env)),
                }
            }
            _ => Err(McpError::invalid_params(
                format!("unknown tool: {name}"),
                None,
            )),
        }
    }
}

fn tool_descriptor<T>(name: &'static str, description: &'static str) -> Tool
where
    T: schemars::JsonSchema + 'static,
{
    let placeholder: Arc<serde_json::Map<String, serde_json::Value>> =
        Arc::new(serde_json::Map::new());
    Tool::new(name, description, placeholder).with_input_schema::<T>()
}

fn json_result<T: serde::Serialize>(value: &T) -> CallToolResult {
    let text = serde_json::to_string(value).unwrap_or_else(|_| "{}".to_string());
    CallToolResult::success(vec![Content::text(text)])
}

fn envelope_result(env: &crate::errors::ErrorEnvelope) -> CallToolResult {
    let json = serde_json::to_string(env).unwrap_or_else(|_| "{}".to_string());
    let structured = serde_json::to_value(env).unwrap_or_default();
    let mut out = CallToolResult::success(vec![Content::text(json)]);
    out.is_error = Some(true);
    out.structured_content = Some(structured);
    out
}
