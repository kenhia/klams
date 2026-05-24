//! MCP tool registry (sprint 007 T023).
//!
//! Houses the [`ToolRegistry`] [`rmcp::ServerHandler`] implementation
//! that backs `tools/list` and `tools/call`. At Phase 2 foundational
//! the registry is intentionally empty — the individual tool handlers
//! (`memory_add`, `memory_search`, `memory_admin_*`, …) land in
//! Phases 3-7 and are appended here once their schemas and storage
//! contracts are wired.
//!
//! Scope-gated visibility (FR-020) means `tools/list` must filter by
//! the caller's [`AuthenticatedScopes`] — that filtering happens here
//! once the registry is populated so non-admin callers never see
//! `memory_admin_*`. Today the empty registry trivially satisfies the
//! contract for every scope set.

use rmcp::{
    model::{
        CallToolRequestParams, CallToolResult, ListToolsResult, PaginatedRequestParams,
        ProtocolVersion, ServerCapabilities, ServerInfo,
    },
    service::RequestContext,
    ErrorData as McpError, RoleServer, ServerHandler,
};
use std::sync::Arc;

/// Shared state injected into every tool handler.
///
/// Holds clones of the Postgres + Qdrant stores, the maintenance
/// state, and the auth grant table. Tool modules in later sprints
/// take this struct by reference / `Arc<>` and never reach for global
/// state.
#[derive(Clone, Debug)]
pub struct McpState {
    // Populated as later sprint tasks land:
    //   pub pg: Arc<PostgresStore>,
    //   pub qd: Arc<QdrantStore>,
    //   pub maintenance: Arc<MaintenanceState>,
    //   pub grants: Arc<Vec<TokenGrant>>,
    /// Placeholder so the struct is non-empty and version-stable.
    pub(crate) _phantom: Arc<()>,
}

impl McpState {
    /// Construct an empty state shell. Replaced by real state when
    /// `klams-service` wires the router (T024).
    #[must_use]
    pub fn empty() -> Self {
        Self {
            _phantom: Arc::new(()),
        }
    }
}

/// `ServerHandler` implementation that exposes the klams MCP tools.
#[derive(Clone, Debug)]
pub struct ToolRegistry {
    #[allow(dead_code)] // wired up by handlers added in later phases
    state: McpState,
}

impl ToolRegistry {
    #[must_use]
    pub fn new(state: McpState) -> Self {
        Self { state }
    }
}

#[allow(clippy::needless_pass_by_value)] // McpState is the canonical constructor input
impl ServerHandler for ToolRegistry {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        info.protocol_version = ProtocolVersion::default();
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info.server_info.name = "klams-mcp".into();
        info.server_info.version = env!("CARGO_PKG_VERSION").into();
        info.instructions = Some(
            "klams memory server. Call `register_author` once per session, \
             then use `memory_*` tools. See contracts in spec 007."
                .into(),
        );
        info
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        // Phase 2 foundational: empty tool list. Tools are added in
        // Phases 3-7 alongside their scope-gated visibility logic.
        Ok(ListToolsResult::default())
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        // T023b: trace every dispatch (entered before scope re-check so
        // denied calls still produce a span). `author_id` / `agent_name`
        // / `model` are filled in by the per-tool handlers once they
        // exist (Phase 3+); the empty registry only sees unknown tools.
        let _span = tracing::info_span!("mcp.tool", tool = %request.name).entered();
        Err(McpError::invalid_params(
            format!("unknown tool: {}", request.name),
            None,
        ))
    }
}
