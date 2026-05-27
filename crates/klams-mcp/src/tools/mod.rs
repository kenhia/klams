//! MCP tool registry (sprint 007 T023 + T032/T033).
//!
//! Houses the [`ToolRegistry`] [`rmcp::ServerHandler`] implementation
//! that backs `tools/list` and `tools/call`, plus the [`McpState`]
//! that carries shared backend handles into per-tool modules.

pub mod event_search;
pub mod memory_add;
pub mod memory_admin_hard_delete;
pub mod memory_admin_list_deleted;
pub mod memory_admin_restore;
pub mod memory_append_event;
pub mod memory_delete;
pub mod memory_related;
pub mod memory_search;
pub mod register_author;

use klams_api::auth::{AuthenticatedScopes, TokenGrant};
use klams_store::CompositeStore;
use klams_types::{MaintenanceState, Scope};
use rmcp::{
    model::{
        CallToolRequestParams, CallToolResult, Content, ListToolsResult, PaginatedRequestParams,
        ProtocolVersion, ServerCapabilities, ServerInfo, Tool,
    },
    service::RequestContext,
    ErrorData as McpError, RoleServer, ServerHandler,
};
use std::sync::Arc;

/// Minimum scope required to see/invoke each tool (FR-020). Sprint 007 T065.
#[must_use]
pub fn required_scope(tool: &str) -> Option<Scope> {
    Some(match tool {
        "register_author" | "memory_search" | "memory_related" | "event_search" => Scope::Read,
        "memory_add" | "memory_append_event" | "memory_delete" => Scope::Write,
        "memory_admin_restore" | "memory_admin_hard_delete" | "memory_admin_list_deleted" => {
            Scope::Admin
        }
        _ => return None,
    })
}

/// Pull the caller's scope set from the rmcp request context. rmcp's
/// `StreamableHttpService` injects `http::request::Parts` into the
/// context extensions; `require_bearer` previously stamped
/// `AuthenticatedScopes` onto those extensions.
fn caller_scopes<R>(ctx: &RequestContext<R>) -> Option<Arc<Vec<Scope>>>
where
    R: rmcp::service::ServiceRole,
{
    ctx.extensions
        .get::<axum::http::request::Parts>()
        .and_then(|p| p.extensions.get::<AuthenticatedScopes>())
        .map(|s| s.0.clone())
}

fn scope_satisfied(scopes: &[Scope], needed: Scope) -> bool {
    scopes.iter().any(|s| s.satisfies(needed))
}

/// Shared state injected into every tool handler.
#[derive(Clone)]
pub struct McpState {
    pub store: Arc<CompositeStore>,
    pub maintenance: Arc<MaintenanceState>,
    pub grants: Arc<Vec<TokenGrant>>,
    pub api: klams_types::ApiConfig,
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
        api: klams_types::ApiConfig,
    ) -> Self {
        Self {
            store,
            maintenance,
            grants,
            api,
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
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let all_tools = vec![
            tool_descriptor::<register_author::RegisterAuthorInput>(
                "register_author",
                "Register the calling agent and obtain an author_id (UUID v7).",
            ),
            tool_descriptor::<memory_add::MemoryAddArgs>(
                "memory_add",
                "Persist a fact or knowledge memory attributed to an author_id.",
            ),
            tool_descriptor::<memory_search::MemorySearchArgs>(
                "memory_search",
                "Search memory across facts, knowledge, and events; returns merged results ranked by relevance.",
            ),
            tool_descriptor::<memory_related::MemoryRelatedArgs>(
                "memory_related",
                "Find knowledge items semantically related to an existing memory id.",
            ),
            tool_descriptor::<memory_append_event::MemoryAppendEventArgs>(
                "memory_append_event",
                "Append an immutable event (deployment, run, signal) attributed to an author_id.",
            ),
            tool_descriptor::<event_search::EventSearchArgs>(
                "event_search",
                "Search events by author/category/window/payload with cursor pagination.",
            ),
            tool_descriptor::<memory_delete::MemoryDeleteArgs>(
                "memory_delete",
                "Soft-delete a fact or knowledge memory by id (FR-014). Idempotent. Events are append-only.",
            ),
            tool_descriptor::<memory_admin_restore::MemoryAdminRestoreArgs>(
                "memory_admin_restore",
                "Admin: restore a soft-deleted memory by id (clears deleted_at).",
            ),
            tool_descriptor::<memory_admin_hard_delete::MemoryAdminHardDeleteArgs>(
                "memory_admin_hard_delete",
                "Admin: permanently delete a memory by id. Events are not deletable.",
            ),
            tool_descriptor::<memory_admin_list_deleted::MemoryAdminListDeletedArgs>(
                "memory_admin_list_deleted",
                "Admin: paginate soft-deleted facts and knowledge for rogue-agent recovery (FR-013).",
            ),
        ];
        // FR-020: only advertise tools the caller's scope set satisfies.
        let scopes = caller_scopes(&context);
        let tools: Vec<Tool> = all_tools
            .into_iter()
            .filter(|t| match required_scope(&t.name) {
                Some(needed) => scopes
                    .as_deref()
                    .is_some_and(|s| scope_satisfied(s, needed)),
                None => false,
            })
            .collect();
        let result = ListToolsResult {
            tools,
            ..ListToolsResult::default()
        };
        Ok(result)
    }

    #[allow(clippy::too_many_lines)]
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        tracing::info!(tool = %request.name, "mcp.tool dispatch");
        let name = request.name.as_ref();
        // FR-020: reject calls the caller's scope set cannot satisfy.
        match required_scope(name) {
            Some(needed) => {
                let scopes = caller_scopes(&context);
                let allowed = scopes
                    .as_deref()
                    .is_some_and(|s| scope_satisfied(s, needed));
                if !allowed {
                    return Ok(envelope_result(&crate::errors::envelope(
                        crate::errors::INSUFFICIENT_SCOPE,
                        format!("tool {name} requires scope {needed:?}"),
                    )));
                }
            }
            None => {
                return Err(McpError::invalid_params(
                    format!("unknown tool: {name}"),
                    None,
                ));
            }
        }
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
            "memory_search" => {
                let args = match serde_json::from_value(args_value) {
                    Ok(a) => a,
                    Err(e) => {
                        return Ok(envelope_result(&crate::errors::envelope(
                            crate::errors::SCHEMA_VALIDATION_FAILED,
                            format!("invalid memory_search arguments: {e}"),
                        )))
                    }
                };
                match memory_search::run(&self.state, args).await {
                    Ok(out) => Ok(json_result(&out)),
                    Err(env) => Ok(envelope_result(&env)),
                }
            }
            "memory_related" => {
                let args = match serde_json::from_value(args_value) {
                    Ok(a) => a,
                    Err(e) => {
                        return Ok(envelope_result(&crate::errors::envelope(
                            crate::errors::SCHEMA_VALIDATION_FAILED,
                            format!("invalid memory_related arguments: {e}"),
                        )))
                    }
                };
                match memory_related::run(&self.state, args).await {
                    Ok(out) => Ok(json_result(&out)),
                    Err(env) => Ok(envelope_result(&env)),
                }
            }
            "memory_append_event" => {
                let args = match serde_json::from_value(args_value) {
                    Ok(a) => a,
                    Err(e) => {
                        return Ok(envelope_result(&crate::errors::envelope(
                            crate::errors::SCHEMA_VALIDATION_FAILED,
                            format!("invalid memory_append_event arguments: {e}"),
                        )))
                    }
                };
                match memory_append_event::run(&self.state, args).await {
                    Ok(out) => Ok(json_result(&out)),
                    Err(env) => Ok(envelope_result(&env)),
                }
            }
            "event_search" => {
                let args = match serde_json::from_value(args_value) {
                    Ok(a) => a,
                    Err(e) => {
                        return Ok(envelope_result(&crate::errors::envelope(
                            crate::errors::SCHEMA_VALIDATION_FAILED,
                            format!("invalid event_search arguments: {e}"),
                        )))
                    }
                };
                match event_search::run(&self.state, args).await {
                    Ok(out) => Ok(json_result(&out)),
                    Err(env) => Ok(envelope_result(&env)),
                }
            }
            "memory_delete" => {
                let args = match serde_json::from_value(args_value) {
                    Ok(a) => a,
                    Err(e) => {
                        return Ok(envelope_result(&crate::errors::envelope(
                            crate::errors::SCHEMA_VALIDATION_FAILED,
                            format!("invalid memory_delete arguments: {e}"),
                        )))
                    }
                };
                match memory_delete::run(&self.state, args).await {
                    Ok(out) => Ok(json_result(&out)),
                    Err(env) => Ok(envelope_result(&env)),
                }
            }
            "memory_admin_restore" => {
                let args = match serde_json::from_value(args_value) {
                    Ok(a) => a,
                    Err(e) => {
                        return Ok(envelope_result(&crate::errors::envelope(
                            crate::errors::SCHEMA_VALIDATION_FAILED,
                            format!("invalid memory_admin_restore arguments: {e}"),
                        )))
                    }
                };
                match memory_admin_restore::run(&self.state, args).await {
                    Ok(out) => Ok(json_result(&out)),
                    Err(env) => Ok(envelope_result(&env)),
                }
            }
            "memory_admin_hard_delete" => {
                let args = match serde_json::from_value(args_value) {
                    Ok(a) => a,
                    Err(e) => {
                        return Ok(envelope_result(&crate::errors::envelope(
                            crate::errors::SCHEMA_VALIDATION_FAILED,
                            format!("invalid memory_admin_hard_delete arguments: {e}"),
                        )))
                    }
                };
                match memory_admin_hard_delete::run(&self.state, args).await {
                    Ok(out) => Ok(json_result(&out)),
                    Err(env) => Ok(envelope_result(&env)),
                }
            }
            "memory_admin_list_deleted" => {
                let args = match serde_json::from_value(args_value) {
                    Ok(a) => a,
                    Err(e) => {
                        return Ok(envelope_result(&crate::errors::envelope(
                            crate::errors::SCHEMA_VALIDATION_FAILED,
                            format!("invalid memory_admin_list_deleted arguments: {e}"),
                        )))
                    }
                };
                match memory_admin_list_deleted::run(&self.state, args).await {
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
