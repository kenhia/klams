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
pub mod memory_get;
pub mod memory_related;
pub mod memory_search;
pub mod memory_supersede;
pub mod memory_update;
pub mod register_author;

use klams_core::validate::ValidatorRegistry;
use klams_store::{CompositeStore, Store};
use klams_types::{AuthenticatedAuthor, AuthenticatedScopes};
use klams_types::{MaintenanceState, Scope};
use rmcp::{
    model::{
        CacheScope, CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock,
        ListToolsResult, PaginatedRequestParams, ProtocolVersion, ServerCapabilities, ServerInfo,
        Tool,
    },
    service::RequestContext,
    ErrorData as McpError, RoleServer, ServerHandler,
};
use std::sync::Arc;

/// The MCP `instructions` block, delivered on every connection.
///
/// Sprint 046 (WI #853): this block is delivered on every
/// connection to every agent and cannot be checked by its reader,
/// so a wrong sentence here is paid for constantly. Two were:
///
///  * the old text said to call `register_author` "only to write
///    as a separate per-session identity". Sprint 025 made it
///    idempotent on `agent_name` — it dedupes and returns the
///    token-bound author, so the consequence it warned about can
///    no longer occur. One session read it as "not for this",
///    concluded its author id was undiscoverable, and filed a WI
///    saying so (the 2026-07-25 rpidash3 misdiagnosis, handoff
///    korg:635). Dropped rather than reworded: nothing in the
///    ordinary agent path needs an `author_id` any more, and
///    `memory_delete`'s own description already carries the note.
///
///  * `similar_existing` was described as something to act on
///    "instead of" writing a duplicate. It arrives on the
///    response to a write that has already happened, so that is
///    not an action the caller still has.
///
/// The rule this leaves behind: when a sprint changes a tool's
/// contract, the instructions block is a call site.
pub const SERVER_INSTRUCTIONS: &str = "klams memory server. Writes are attributed to the author \
    bound to your bearer token — call the `memory_*` tools \
    directly; you do not need to supply an `author_id`, and \
    `register_author` is idempotent on `agent_name` (it returns \
    your bound author rather than minting a second identity). \
    When a memory you can see is stale or wrong, prefer \
    `memory_supersede` (one call: replacement + hidden original \
    + trail) over delete-then-add; use `memory_update` for \
    typo-level fixes to your own records. A `similar_existing` \
    block on a `memory_add` response is after the fact — the \
    near-duplicate is already written. The remedy is \
    `memory_delete` on the record you just created, then \
    `memory_supersede` on the original.";

/// Minimum scope required to see/invoke each tool (FR-020). Sprint 007 T065.
#[must_use]
pub fn required_scope(tool: &str) -> Option<Scope> {
    Some(match tool {
        "memory_search" | "memory_get" | "memory_related" | "event_search" => Scope::Read,
        // Sprint 025 (#633): `register_author` moved Read -> Write. It
        // mints identities, which was a read-scope operation until now —
        // a read-only token could manufacture authors at will.
        // Sprint 029 (#638): the lifecycle verbs sit at Write like
        // delete — ownership (or `manage`) is enforced inside the tool.
        "register_author"
        | "memory_add"
        | "memory_append_event"
        | "memory_delete"
        | "memory_supersede"
        | "memory_update"
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
///
/// Sprint 031 (#645): generic over `S: Store` instead of holding a
/// concrete `Arc<CompositeStore>`. The concrete type is why the tools
/// could not be tested without Postgres, Qdrant and TEI all running —
/// there was no seam to substitute. `klams-service` instantiates
/// `McpState<CompositeStore>`; tests instantiate it over a mock.
#[derive(Clone)]
pub struct McpState<S: Store = CompositeStore> {
    pub store: Arc<S>,
    pub maintenance: Arc<MaintenanceState>,
    pub api: klams_types::ApiConfig,
    /// Cross-source rank-fusion strategy for `memory_search` (sprint 024
    /// #328/#330 — from the `[retrieval] fusion` config).
    pub fusion: klams_types::FusionStrategy,
    /// Sprint 027 (#420): the embedder's input ceiling. `memory_add` had
    /// no length check whatsoever before this — an over-budget write went
    /// straight to TEI and came back as a misleading transient error.
    pub embed_limit: klams_types::EmbedLimit,
    /// Sprint 030 (#685): optional second-stage cross-encoder for
    /// `memory_search`, from `[retrieval] reranker_url`. `None` = the
    /// stage is off.
    pub reranker: Option<Arc<klams_store::TeiReranker>>,
    /// Max candidates per rerank call (`[retrieval] rerank_window`).
    pub rerank_window: usize,
    /// Sprint 031 (#645): the same `ValidatorRegistry` the REST surface
    /// runs writes through. MCP had none — an agent could store a
    /// `UserFact` with no `name`, or an `EnvFact` whose key broke the
    /// documented shape, and the write landed. REST rejected the
    /// identical payload. One policy, both surfaces.
    pub validators: Arc<ValidatorRegistry>,
}

impl<S: Store> std::fmt::Debug for McpState<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpState").finish_non_exhaustive()
    }
}

impl<S: Store> McpState<S> {
    /// Canonical constructor used by `klams-service`.
    #[must_use]
    pub fn new(
        store: Arc<S>,
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
            // Defaults to the deployed model's 512 tokens;
            // `klams-service` overrides it from `[embeddings]
            // max_input_tokens` (sprint 027 #420).
            embed_limit: klams_types::EmbedLimit::default(),
            // Off unless `klams-service` wires `[retrieval]
            // reranker_url` (sprint 030 #685).
            reranker: None,
            rerank_window: 50,
            // Sprint 031 (#645): the defaults are the same set REST
            // installs, so the two surfaces agree out of the box
            // rather than only when the caller remembers to wire it.
            validators: Arc::new(ValidatorRegistry::with_defaults()),
        }
    }

    /// Override the embedder size gate from config (sprint 027).
    #[must_use]
    pub fn with_embed_limit(mut self, limit: klams_types::EmbedLimit) -> Self {
        self.embed_limit = limit;
        self
    }
}

/// The MCP revisions klams serves, oldest first (sprint 043, WI #1216).
///
/// Adding a revision here is a promise to emit its required shape, so the
/// list is hand-maintained rather than borrowed from rmcp.
const SUPPORTED_PROTOCOL_VERSIONS: &[ProtocolVersion] = &[
    ProtocolVersion::V_2024_11_05,
    ProtocolVersion::V_2025_03_26,
    ProtocolVersion::V_2025_06_18,
    ProtocolVersion::V_2025_11_25,
    ProtocolVersion::V_2026_07_28,
];

/// The newest revision klams serves: what an unknown/newer request falls
/// back to, and what `get_info` advertises.
const PROTOCOL_VERSION_CEILING: ProtocolVersion = ProtocolVersion::V_2026_07_28;

/// How long a client may cache the tool catalog (SEP-2549 `ttlMs`).
///
/// Within one scope set the catalog is static per build, so an hour is
/// honest — a klams deploy restarts the server and the session with it.
const TOOL_CATALOG_TTL_MS: u64 = 3_600_000;

/// `ServerHandler` implementation that exposes the klams MCP tools.
#[derive(Clone, Debug)]
pub struct ToolRegistry<S: Store = CompositeStore> {
    state: McpState<S>,
}

impl<S: Store> ToolRegistry<S> {
    #[must_use]
    pub fn new(state: McpState<S>) -> Self {
        Self { state }
    }
}

#[allow(clippy::needless_pass_by_value)]
impl<S: Store> ServerHandler for ToolRegistry<S> {
    /// The MCP revisions klams actually implements, newest last.
    ///
    /// Sprint 043: this override is load-bearing. rmcp's default returns
    /// `ProtocolVersion::KNOWN_VERSIONS` — every revision *the SDK* knows,
    /// which is a claim about rmcp, not about klams. Serving that default
    /// is how korg:1212 happened: a client asks for a revision, gets it
    /// echoed, validates the response against that revision's schema,
    /// fails, and registers ZERO TOOLS while still looking connected.
    /// This list may only grow when the corresponding shape is emitted
    /// below.
    fn supported_protocol_versions(&self) -> std::borrow::Cow<'static, [ProtocolVersion]> {
        std::borrow::Cow::Borrowed(SUPPORTED_PROTOCOL_VERSIONS)
    }

    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        // Sprint 043: pinned, never `ProtocolVersion::default()`/`LATEST`.
        // An SDK constant moving underneath a release is precisely how this
        // bug class arrives (kaed 015 D-3) — in rmcp 3.1.2 `LATEST` is still
        // 2025-11-25 even though `KNOWN_VERSIONS` tops out at 2026-07-28.
        info.protocol_version = PROTOCOL_VERSION_CEILING;
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info.server_info.name = "klams-mcp".into();
        info.server_info.version = env!("CARGO_PKG_VERSION").into();
        info.instructions = Some(SERVER_INSTRUCTIONS.into());
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
        // Sprint 043 (WI #1216): SEP-2549 cache metadata, required by
        // 2026-07-28 on paginated results. tools/list is the only one
        // klams serves — `CallToolResult` carries no cache metadata in
        // any revision, so this is catalog caching, not result caching.
        //
        // Emitted ONLY for peers that negotiated 2026-07-28+. A 2025-11-25
        // peer is entitled to 2025-11-25's shape, and strict clients may
        // reject unknown fields — rmcp makes the same call for the
        // neighbouring `resultType`.
        //
        // `cacheScope` is PRIVATE, diverging from kaed and korg-mcp, which
        // both emit `public`. Their catalogs are static per build; klams's
        // is not — the filter above narrows it to what the caller's bearer
        // token scopes satisfy, so a Read token and an Admin token get
        // different catalogs from the same server. `public` would invite a
        // shared cache to serve one principal's catalog to another. Note
        // rmcp's `CacheScope::default()` is `Public`, so this must stay
        // explicit.
        let negotiated = context.protocol_version();
        let cache_metadata = negotiated
            .as_ref()
            .is_some_and(|v| *v >= PROTOCOL_VERSION_CEILING);
        // Sprint 046 (WI #1230): `call_tool` logs its dispatch, `list_tools`
        // logged nothing — so the one operation the korg:1212 failure class
        // actually breaks was the one klams could not observe. That class is
        // a CLIENT-side validation drop: the server sees a successful
        // request and has nothing unusual to report, so a zero-tools bug
        // reads as silence. Recording what shape we served to which revision
        // is the difference between diagnosing it from the server in one
        // command and inferring it from span timing, which is what sprint
        // 043's live gate had to do.
        tracing::info!(
            tools = result.tools.len(),
            protocol = negotiated
                .as_ref()
                .map_or("<none>", ProtocolVersion::as_str),
            cache_metadata,
            "mcp.tools/list"
        );
        if cache_metadata {
            return Ok(result
                .with_ttl_ms(TOOL_CATALOG_TTL_MS)
                .with_cache_scope(CacheScope::Private));
        }
        Ok(result)
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        // Sprint 043: rmcp 3.x returns the MRTR envelope (`CallToolResponse`)
        // rather than a bare `CallToolResult`. Every klams tool completes
        // in-request — no elicitation, no SEP-2663 tasks — so the conversion
        // is total, and stripping `resultType` for legacy peers stays rmcp's
        // job. The dispatch body below is unchanged by the 3.x migration.
        self.dispatch_tool(request, context).await.map(Into::into)
    }
}

impl<S: Store> ToolRegistry<S> {
    #[allow(clippy::too_many_lines, clippy::needless_pass_by_value)]
    async fn dispatch_tool(
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
            "memory_get" => {
                let args = match serde_json::from_value(args_value) {
                    Ok(a) => a,
                    Err(e) => {
                        return Ok(envelope_result(&crate::errors::envelope(
                            crate::errors::SCHEMA_VALIDATION_FAILED,
                            format!("invalid memory_get arguments: {e}"),
                        )))
                    }
                };
                match memory_get::run(&self.state, args).await {
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
                let caller_agent = caller.as_ref().map(|a| a.agent_name.as_str());
                match event_search::run(&self.state, args, caller_agent).await {
                    Ok(out) => Ok(json_result(&out)),
                    Err(env) => Ok(envelope_result(&env)),
                }
            }
            "memory_supersede" => {
                let args = match serde_json::from_value(args_value) {
                    Ok(a) => a,
                    Err(e) => {
                        return Ok(envelope_result(&crate::errors::envelope(
                            crate::errors::SCHEMA_VALIDATION_FAILED,
                            format!("invalid memory_supersede arguments: {e}"),
                        )))
                    }
                };
                // Sprint 029 (#638): same caller shape as delete — the
                // verb acts as the bearer-bound author, `manage` is the
                // cross-author tier.
                let lifecycle_caller = caller.as_ref().map(|a| memory_delete::DeleteCaller {
                    author_id: a.author_id,
                    scopes: caller_scopes(&context).map_or_else(Vec::new, |s| s.as_ref().clone()),
                });
                match memory_supersede::run(&self.state, args, lifecycle_caller.as_ref()).await {
                    Ok(out) => Ok(json_result(&out)),
                    Err(env) => Ok(envelope_result(&env)),
                }
            }
            "memory_update" => {
                let args = match serde_json::from_value(args_value) {
                    Ok(a) => a,
                    Err(e) => {
                        return Ok(envelope_result(&crate::errors::envelope(
                            crate::errors::SCHEMA_VALIDATION_FAILED,
                            format!("invalid memory_update arguments: {e}"),
                        )))
                    }
                };
                let lifecycle_caller = caller.as_ref().map(|a| memory_delete::DeleteCaller {
                    author_id: a.author_id,
                    scopes: caller_scopes(&context).map_or_else(Vec::new, |s| s.as_ref().clone()),
                });
                match memory_update::run(&self.state, args, lifecycle_caller.as_ref()).await {
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
            "Search memory across facts, knowledge, and events; returns merged results ranked by relevance. Hits are COMPACT: a match-window snippet (<=320 chars) plus an `id` — call `memory_get` with that id for the full record. Pass `full: true` to get whole texts inline instead. Pass `tags` to search *within* a tagged subset rather than the whole corpus — e.g. tags:[\"gotcha\"] for the curated gotchas. Multiple tags are AND.",
        ),
        tool_descriptor::<memory_get::MemoryGetArgs>(
            "memory_get",
            "Fetch one memory in full by id — the follow-up read for a `memory_search` hit whose snippet did not carry what you needed. Pass the hit's `id`. Works for facts, knowledge and events alike.",
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
        tool_descriptor::<memory_supersede::MemorySupersedeArgs>(
            "memory_supersede",
            "Replace a stale or wrong agent-authored knowledge memory in one call: writes your replacement text as a new memory and hides the old one behind a superseded_by pointer (restorable via admin). Prefer this over memory_delete + memory_add — it keeps the correction trail. Tags/volatility inherit from the old memory unless given. You may supersede memories you wrote; another author's requires the `manage` scope.",
        ),
        tool_descriptor::<memory_update::MemoryUpdateArgs>(
            "memory_update",
            "Edit an agent-authored knowledge memory in place (text, tags, and/or volatility); the id stays stable and text changes re-embed. For typos and small amendments — use memory_supersede when the old statement was wrong and the trail matters. You may update memories you wrote; another author's requires the `manage` scope.",
        ),
        tool_descriptor::<dissent_propose::DissentProposeArgs>(
            "dissent_propose",
            "File a dissent against a live canonical fact (proposed correction + reason). Lands as a pending AgentProposal a human promotes or discards over the dissents endpoints.",
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
    CallToolResult::success(vec![ContentBlock::text(text)])
}

fn envelope_result(env: &crate::errors::ErrorEnvelope) -> CallToolResult {
    let json = serde_json::to_string(env).unwrap_or_else(|_| "{}".to_string());
    let structured = serde_json::to_value(env).unwrap_or_default();
    let mut out = CallToolResult::success(vec![ContentBlock::text(json)]);
    out.is_error = Some(true);
    out.structured_content = Some(structured);
    out
}
