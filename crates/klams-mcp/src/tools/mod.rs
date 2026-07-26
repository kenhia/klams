//! MCP tool registry (sprint 007 T023 + T032/T033).
//!
//! Houses the [`ToolRegistry`] [`rmcp::ServerHandler`] implementation
//! that backs `tools/list` and `tools/call`, plus the [`McpState`]
//! that carries shared backend handles into per-tool modules.

pub mod dissent_propose;
pub mod event_search;
pub mod memory_add;
pub mod memory_admin_authors;
pub mod memory_admin_hard_delete;
pub mod memory_admin_list_deleted;
pub mod memory_admin_restore;
pub mod memory_append_event;
pub mod memory_delete;
pub mod memory_related;
pub mod memory_search;
pub mod register_author;

use klams_api::auth::{AuthenticatedAuthor, AuthenticatedScopes};
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
        "memory_search" | "memory_related" | "event_search" => Scope::Read,
        // Sprint 025 (#633): `register_author` moved Read -> Write. It
        // mints identities, which was a read-scope operation until now —
        // a read-only token could manufacture authors at will.
        "register_author"
        | "memory_add"
        | "memory_append_event"
        | "memory_delete"
        | "dissent_propose" => Scope::Write,
        // Sprint 025 (#636): the author lifecycle verbs rewrite
        // attribution across the whole store — a stronger capability
        // than the cross-author curation `manage` grants, so they sit
        // with the other admin tools.
        "memory_admin_restore"
        | "memory_admin_hard_delete"
        | "memory_admin_list_deleted"
        | "memory_admin_list_authors"
        | "memory_admin_remove_author"
        | "memory_admin_merge_authors" => Scope::Admin,
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

/// Pull the author bound to the caller's bearer token (sprint 018,
/// WI #62). Same extension-forwarding path as [`caller_scopes`]:
/// `require_bearer` stamps [`AuthenticatedAuthor`] on the request and
/// rmcp copies the request parts into the tool context.
fn caller_author<R>(ctx: &RequestContext<R>) -> Option<AuthenticatedAuthor>
where
    R: rmcp::service::ServiceRole,
{
    ctx.extensions
        .get::<axum::http::request::Parts>()
        .and_then(|p| p.extensions.get::<AuthenticatedAuthor>())
        .cloned()
}

/// Write tools whose `author_id` argument may be omitted and filled
/// from the bearer token's bound author (WI #62).
const BEARER_AUTHOR_TOOLS: [&str; 3] = ["memory_add", "memory_append_event", "dissent_propose"];

/// Shared state injected into every tool handler.
///
/// Sprint 018: the `grants` copy plumbed in 007 was removed — no tool
/// ever read it, and with WI #61's hot-reloadable table a snapshot
/// here would go stale. Caller identity reaches tools via request
/// extensions ([`caller_scopes`] / [`caller_author`]).
#[derive(Clone)]
pub struct McpState {
    pub store: Arc<CompositeStore>,
    pub maintenance: Arc<MaintenanceState>,
    pub api: klams_types::ApiConfig,
    /// Cross-source rank-fusion strategy for `memory_search` (sprint 024
    /// #328/#330 — from the `[retrieval] fusion` config).
    pub fusion: klams_types::FusionStrategy,
}

impl std::fmt::Debug for McpState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpState").finish_non_exhaustive()
    }
}

impl McpState {
    /// Canonical constructor used by `klams-service`.
    #[must_use]
    pub fn new(
        store: Arc<CompositeStore>,
        maintenance: Arc<MaintenanceState>,
        api: klams_types::ApiConfig,
    ) -> Self {
        Self {
            store,
            maintenance,
            api,
            // Default RRF(k=60); `klams-service` overrides this from the
            // `[retrieval] fusion` config (sprint 024 #330).
            fusion: klams_types::FusionStrategy::default_rrf(),
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
            "klams memory server. Writes are attributed to the author bound \
             to your bearer token, so you can call `memory_*` tools \
             directly; call `register_author` only to write as a separate \
             per-session identity."
                .into(),
        );
        info
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let all_tools = all_tool_descriptors();
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
        // Caller identity from the bearer token, resolved once: reused
        // for the WI #62 author-fill below and the sprint 021 miss-log
        // attribution on `memory_search`.
        let caller = caller_author(&context);
        let mut arguments = request.arguments;
        // WI #62: an authenticated caller may omit author_id on the
        // write tools; attribute the write to the author its bearer
        // token is bound to. An explicit author_id always wins.
        if BEARER_AUTHOR_TOOLS.contains(&name) {
            let missing = arguments
                .as_ref()
                .is_none_or(|a| !a.contains_key("author_id"));
            if missing {
                if let Some(author) = &caller {
                    tracing::debug!(
                        tool = name,
                        agent_name = %author.agent_name,
                        "author_id omitted; using bearer-bound author"
                    );
                    arguments.get_or_insert_with(serde_json::Map::new).insert(
                        "author_id".to_string(),
                        serde_json::Value::String(author.author_id.to_string()),
                    );
                }
            }
        }
        let args_value = arguments.map_or(serde_json::Value::Null, serde_json::Value::Object);
        // Sprint 025: the deserialize-then-run-then-envelope shape is
        // identical for every tool that needs nothing but `state` and
        // its args. New arms use this; the older ones are left as they
        // were rather than churned in a security sprint.
        macro_rules! dispatch {
            ($run:path, $label:literal) => {{
                let args = match serde_json::from_value(args_value) {
                    Ok(a) => a,
                    Err(e) => {
                        return Ok(envelope_result(&crate::errors::envelope(
                            crate::errors::SCHEMA_VALIDATION_FAILED,
                            format!("invalid {} arguments: {e}", $label),
                        )))
                    }
                };
                match $run(&self.state, args).await {
                    Ok(out) => Ok(json_result(&out)),
                    Err(env) => Ok(envelope_result(&env)),
                }
            }};
        }
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
                let caller_agent = caller.as_ref().map(|a| a.agent_name.as_str());
                match memory_search::run(&self.state, args, caller_agent).await {
                    Ok(out) => Ok(json_result(&out)),
                    Err(env) => Ok(envelope_result(&env)),
                }
            }
            "dissent_propose" => {
                let args = match serde_json::from_value(args_value) {
                    Ok(a) => a,
                    Err(e) => {
                        return Ok(envelope_result(&crate::errors::envelope(
                            crate::errors::SCHEMA_VALIDATION_FAILED,
                            format!("invalid dissent_propose arguments: {e}"),
                        )))
                    }
                };
                match dissent_propose::run(&self.state, args).await {
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
                // Sprint 025 (#633): the delete acts as the bearer-bound
                // author and consults the caller's scopes for the
                // cross-author `manage` tier.
                let delete_caller = caller.as_ref().map(|a| memory_delete::DeleteCaller {
                    author_id: a.author_id,
                    scopes: caller_scopes(&context).map_or_else(Vec::new, |s| s.as_ref().clone()),
                });
                match memory_delete::run(&self.state, args, delete_caller.as_ref()).await {
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
            "memory_admin_list_authors" => {
                dispatch!(memory_admin_authors::list, "memory_admin_list_authors")
            }
            "memory_admin_remove_author" => {
                dispatch!(memory_admin_authors::remove, "memory_admin_remove_author")
            }
            "memory_admin_merge_authors" => {
                dispatch!(memory_admin_authors::merge, "memory_admin_merge_authors")
            }
            _ => Err(McpError::invalid_params(
                format!("unknown tool: {name}"),
                None,
            )),
        }
    }
}

/// The full, unfiltered tool descriptor list — the single source of
/// truth for what the server can advertise. `list_tools` filters this
/// by caller scope (FR-020); the schema-shape contract test
/// (`tests/tool_schemas.rs`, WI #307) walks it unfiltered.
#[must_use]
pub fn all_tool_descriptors() -> Vec<Tool> {
    vec![
        tool_descriptor::<register_author::RegisterAuthorInput>(
            "register_author",
            "Register the calling agent and obtain an author_id (UUID v7). Optional for bearer-authenticated callers: write tools default to the token's bound author. `repo` accepts an absolute path or a bare repo name.",
        ),
        tool_descriptor::<memory_add::MemoryAddArgs>(
            "memory_add",
            "Persist a fact or knowledge memory. Set kind=\"fact\" with fact_type + payload, or kind=\"knowledge\" with text (+ optional tags/source_path/repo). author_id defaults to the bearer token's bound author.",
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
            "Append an immutable event (deployment, run, signal). author_id defaults to the bearer token's bound author.",
        ),
        tool_descriptor::<event_search::EventSearchArgs>(
            "event_search",
            "Search events by author/category/window/payload with cursor pagination.",
        ),
        tool_descriptor::<memory_delete::MemoryDeleteArgs>(
            "memory_delete",
            "Soft-delete a fact or knowledge memory by id (FR-014). Idempotent. Events are append-only. Omit author_id — the delete acts as the author bound to your bearer token. You may delete memories you wrote; deleting another author's memory requires the `manage` scope.",
        ),
        tool_descriptor::<dissent_propose::DissentProposeArgs>(
            "dissent_propose",
            "File a dissent against a live canonical fact (proposed correction + reason). Lands as a pending AgentProposal; a human resolves it in the viewport.",
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
        tool_descriptor::<memory_admin_authors::ListAuthorsArgs>(
            "memory_admin_list_authors",
            "Admin: list authors with the counts that decide whether one is safe to remove (facts, events, knowledge, recorded soft-deletes), plus the agent_names held by more than one row. Set only_empty to see just the removable ones.",
        ),
        tool_descriptor::<memory_admin_authors::RemoveAuthorArgs>(
            "memory_admin_remove_author",
            "Admin: remove an author row. Refused with AUTHOR_HAS_MEMORIES while it still owns anything — merge it first. Never reassigns silently.",
        ),
        tool_descriptor::<memory_admin_authors::MergeAuthorsArgs>(
            "memory_admin_merge_authors",
            "Admin: reassign every fact, event, knowledge point and soft-delete attribution from one author to another, then remove the source. Use to collapse duplicate agent_name rows.",
        ),
    ]
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
