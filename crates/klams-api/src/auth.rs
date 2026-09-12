//! Bearer-token auth middleware.
//!
//! Uses constant-time comparison (`subtle`) to resist timing oracles.
//! Public paths (e.g. `/healthz`, `/metrics`) should be mounted
//! outside the protected router.
//!
//! Sprint 007 — multi-token + scoped tokens:
//! [`AuthState`] now holds a slice of [`TokenGrant`]s. On every request
//! the middleware compares the presented bearer against every grant
//! using a constant-time loop with **no early exit** so timing leaks
//! cannot reveal which (or how many) grants match. The matched grant's
//! [`klams_types::Scope`] set is stashed in the request extensions as
//! [`AuthenticatedScopes`] so downstream `require_scope(...)` layers
//! can enforce per-route permission tiers.
//!
//! # Sprint 049 — identities replace bearer tokens
//!
//! A klams bearer token was a name tag, not a lock. Under the homelab
//! threat model — single user, his agents, one tailnet, agents already
//! holding sudo everywhere — the token's only job was to say *which
//! `agent_name`*, and a declared name does that without a secret.
//!
//! So [`AuthState`] now holds **two** tables and this middleware checks
//! them in a fixed order:
//!
//! 1. `X-Homelab-Agent: <agent_name>` against `[[auth.identities]]`.
//!    An unknown name is a 401, exactly as an unknown token is.
//! 2. Failing that, `Authorization: Bearer` against `[[auth.tokens]]`
//!    — the legacy path, unchanged, constant-time.
//!
//! Both resolve to the same author, because authorship has been keyed
//! on `agent_name` rather than on the token bytes since sprint 009.
//! That is why this change needs no migration: nothing about
//! attribution moves.
//!
//! **The transition window is open exactly while `[[auth.tokens]]` has
//! rows.** There is no flag; deleting the rows is what closes it
//! (sprint 049 D-1), and korg:2450 does that after every client is on
//! the header.
//!
//! The caller's tailnet node is resolved alongside (see [`crate::whois`])
//! and recorded. It is record-only unless `[auth.whois] enforce` is on,
//! which it is not by default.

use crate::ApiError;
use axum::{
    body::Body,
    extract::{Request as AxumRequest, State},
    http::{header::AUTHORIZATION, HeaderMap, Request},
    middleware::Next,
    response::Response,
};
use klams_types::{AuthMethod, AuthenticatedAuthor, AuthenticatedPeer, AuthenticatedScopes, Scope};
use std::net::IpAddr;
use std::sync::Arc;
use subtle::ConstantTimeEq;
use uuid::Uuid;

/// The header a caller declares its identity in (sprint 049).
pub const AGENT_HEADER: &str = "x-homelab-agent";

/// The socket peer of the connection a request arrived on, stamped by
/// the accept loop (`klams_service::limits`).
///
/// Only the fallback: behind `tailscale serve` — which is how klams
/// actually runs — this is always loopback and `X-Forwarded-For` is the
/// informative one. See [`crate::whois`] for the measurement.
#[derive(Clone, Copy, Debug)]
pub struct PeerAddr(pub std::net::SocketAddr);

/// Materialized form of a `TokenGrantConfig`. The token bytes are
/// retained as `Vec<u8>` (not zeroized) so that constant-time compare
/// has a stable buffer to compare against.
#[derive(Clone)]
pub struct TokenGrant {
    pub token_bytes: Arc<Vec<u8>>,
    pub scopes: Arc<Vec<Scope>>,
    pub label: Option<String>,
    /// Author that writes via this grant are attributed to. Defaults
    /// to `SYSTEM_AUTHOR_ID` ("system") for grants without an explicit
    /// `agent_name` binding.
    pub author_id: Uuid,
    pub agent_name: Arc<String>,
}

impl TokenGrant {
    /// Back-compat constructor: binds the grant to the system author.
    #[must_use]
    pub fn new(token: impl Into<String>, scopes: Vec<Scope>, label: Option<String>) -> Self {
        Self::new_with_author(
            token,
            scopes,
            label,
            klams_types::SYSTEM_AUTHOR_ID,
            "system",
        )
    }

    /// Sprint 009: bind the grant to a specific author.
    #[must_use]
    pub fn new_with_author(
        token: impl Into<String>,
        scopes: Vec<Scope>,
        label: Option<String>,
        author_id: Uuid,
        agent_name: impl Into<String>,
    ) -> Self {
        Self {
            token_bytes: Arc::new(token.into().into_bytes()),
            scopes: Arc::new(scopes),
            label,
            author_id,
            agent_name: Arc::new(agent_name.into()),
        }
    }
}

impl std::fmt::Debug for TokenGrant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenGrant")
            .field("token_len", &self.token_bytes.len())
            .field("scopes", &*self.scopes)
            .field("label", &self.label)
            .field("author_id", &self.author_id)
            .field("agent_name", &*self.agent_name)
            .finish()
    }
}

/// Materialized form of a `klams_types::IdentityConfig` (sprint 049):
/// an `agent_name` the caller declares, its scopes, and its resolved
/// author.
///
/// No constant-time anything here, deliberately. A declared name is not
/// a secret — that is the entire point of the change — so comparing it
/// with `==` leaks nothing a caller could not read off the config it is
/// allowed to be in.
#[derive(Clone, Debug)]
pub struct Identity {
    pub agent_name: Arc<String>,
    pub scopes: Arc<Vec<Scope>>,
    pub label: Option<String>,
    pub author_id: Uuid,
    /// Tailnet nodes this identity may arrive from. Empty = unpinned.
    /// Consulted only when enforcement is on.
    pub nodes: Arc<Vec<String>>,
}

/// Both auth tables, swapped together (sprint 049).
///
/// One struct rather than two locks because SIGHUP re-reads one file
/// and must install both halves atomically — a reload that published
/// new identities against old tokens would be a state the config never
/// described.
#[derive(Clone, Debug, Default)]
pub struct AuthTables {
    pub tokens: Vec<TokenGrant>,
    pub identities: Vec<Identity>,
}

impl AuthTables {
    #[must_use]
    pub fn new(tokens: Vec<TokenGrant>, identities: Vec<Identity>) -> Self {
        Self { tokens, identities }
    }

    /// Is the sprint-049 transition window open? True while any legacy
    /// `[[auth.tokens]]` grant survives (D-1: the rows *are* the flag).
    #[must_use]
    pub fn legacy_window_open(&self) -> bool {
        !self.tokens.is_empty()
    }
}

// `AuthenticatedScopes` and `AuthenticatedAuthor` moved down to
// `klams-types` in sprint 031 (#645). They are the request-extension
// types both surfaces read, and keeping them here forced `klams-mcp` to
// depend on the REST crate for two structs and nothing else. Imported
// above; `require_bearer` still stamps them.

/// Sprint 018 (WI #61) — the grant table sits behind an `RwLock` so
/// `[[auth.tokens]]` edits can be hot-reloaded (SIGHUP) without a
/// service restart. All clones (REST layer, `/mcp` layer, the reload
/// task) share one table; [`AuthState::replace_grants`] swaps it
/// atomically. In-flight requests hold at most a snapshot `Arc` for
/// the duration of their own auth check.
///
/// Sprint 049: the table became two (tokens + identities), and the
/// state grew the whois resolver, because the node cross-check happens
/// at exactly the same point as the identity lookup and reads the same
/// request.
#[derive(Clone)]
pub struct AuthState {
    tables: Arc<std::sync::RwLock<Arc<AuthTables>>>,
    /// `None` = whois disabled; every request records `unknown`.
    whois: Option<Arc<dyn crate::whois::NodeResolver>>,
    /// `[auth.whois] enforce`. Off by default (WI 2389).
    enforce_nodes: bool,
}

impl AuthState {
    /// Legacy single-token constructor. Materializes one grant carrying
    /// **all** scopes — preserves pre-sprint-007 behaviour for callers
    /// that have not yet migrated to scoped tokens.
    ///
    /// Sprint 025: `Manage` is included. Scopes are flat, so omitting it
    /// here would have *removed* capability from the one token some
    /// deployments have — this grant is the "everything" token by
    /// construction, and must stay that way across the upgrade.
    pub fn new(bearer_token: impl Into<String>) -> Self {
        let grant = TokenGrant::new(
            bearer_token,
            vec![Scope::Read, Scope::Write, Scope::Manage, Scope::Admin],
            Some("legacy".into()),
        );
        Self::with_grants(vec![grant])
    }

    /// New multi-token constructor. Order of grants is irrelevant; the
    /// auth check compares against every grant with no early exit.
    #[must_use]
    pub fn with_grants(grants: Vec<TokenGrant>) -> Self {
        Self::with_tables(AuthTables::new(grants, Vec::new()))
    }

    /// Sprint 049 — both tables. Whois is off until
    /// [`Self::with_whois`] installs a resolver.
    #[must_use]
    pub fn with_tables(tables: AuthTables) -> Self {
        Self {
            tables: Arc::new(std::sync::RwLock::new(Arc::new(tables))),
            whois: None,
            enforce_nodes: false,
        }
    }

    /// Install the tailnet resolver and the enforcement setting.
    ///
    /// Enforcement without a resolver would refuse every pinned
    /// identity (nothing can ever resolve), so it is deliberately not
    /// expressible: the two travel together.
    #[must_use]
    pub fn with_whois(
        mut self,
        resolver: Arc<dyn crate::whois::NodeResolver>,
        enforce: bool,
    ) -> Self {
        self.whois = Some(resolver);
        self.enforce_nodes = enforce;
        self
    }

    /// Atomically swap both auth tables (WI #61 hot-reload, extended to
    /// identities in sprint 049). Visible to every clone of this
    /// `AuthState` — the next auth check on any route uses the new
    /// tables; requests already past their auth check are unaffected.
    pub fn replace_tables(&self, tables: AuthTables) {
        let mut w = self
            .tables
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *w = Arc::new(tables);
    }

    /// Swap the token table only, leaving identities in place.
    pub fn replace_grants(&self, grants: Vec<TokenGrant>) {
        let identities = self.tables().identities.clone();
        self.replace_tables(AuthTables::new(grants, identities));
    }

    /// Snapshot the current tables. The snapshot is immutable and
    /// outlives a concurrent [`Self::replace_tables`].
    fn tables(&self) -> Arc<AuthTables> {
        self.tables
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Test-only accessor for the installed grant list. Exposed via a
    /// public method (rather than `pub` field) so the storage shape
    /// stays free to evolve.
    #[doc(hidden)]
    #[must_use]
    pub fn grants_for_test(&self) -> Vec<TokenGrant> {
        self.tables().tokens.clone()
    }

    /// Test-only accessor for the installed identity list.
    #[doc(hidden)]
    #[must_use]
    pub fn identities_for_test(&self) -> Vec<Identity> {
        self.tables().identities.clone()
    }
}

impl std::fmt::Debug for AuthState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let tables = self.tables();
        f.debug_struct("AuthState")
            .field("grant_count", &tables.tokens.len())
            .field("identity_count", &tables.identities.len())
            .field("whois", &self.whois.is_some())
            .field("enforce_nodes", &self.enforce_nodes)
            // The table itself is summarised by the two counts above;
            // dumping every grant would put live token bytes in a log
            // line, which is the one thing this type must never do.
            .finish_non_exhaustive()
    }
}

/// What a successful auth check resolved to, before it is stamped onto
/// the request.
struct Resolved {
    scopes: Arc<Vec<Scope>>,
    author_id: Uuid,
    agent_name: Arc<String>,
    /// Tailnet nodes this caller is pinned to; empty = unpinned.
    nodes: Arc<Vec<String>>,
    method: AuthMethod,
}

/// The caller's tailnet address.
///
/// `X-Forwarded-For` first — klams runs behind `tailscale serve`, which
/// sets it to the caller's tailnet IP while the socket peer stays
/// loopback (measured; see [`crate::whois`]). The socket peer is the
/// fallback for a direct deployment.
///
/// Only the **first** entry of `X-Forwarded-For` is read: that is the
/// original client in the standard's ordering, and klams sits behind at
/// most one proxy.
fn caller_addr(headers: &HeaderMap, peer: Option<PeerAddr>) -> Option<IpAddr> {
    let forwarded = headers
        .get("x-forwarded-for")
        .and_then(|h| h.to_str().ok())
        .and_then(|v| v.split(',').next())
        .and_then(|v| v.trim().parse::<IpAddr>().ok());
    forwarded.or_else(|| peer.map(|p| p.0.ip()))
}

/// Match the declared `X-Homelab-Agent` name against the identities
/// table. Plain equality: the name is not a secret.
fn resolve_identity(tables: &AuthTables, declared: &str) -> Option<Resolved> {
    let id = tables
        .identities
        .iter()
        .find(|i| i.agent_name.as_str() == declared)?;
    Some(Resolved {
        scopes: id.scopes.clone(),
        author_id: id.author_id,
        agent_name: id.agent_name.clone(),
        nodes: id.nodes.clone(),
        method: AuthMethod::Identity,
    })
}

/// Match a presented bearer against every token grant in constant time.
///
/// Unchanged from sprint 007 and deliberately so: while the transition
/// window is open these are still live credentials, and the timing
/// property they were given is not something to drop on the way past.
fn resolve_bearer(tables: &AuthTables, provided: &[u8]) -> Option<Resolved> {
    // Constant-time loop: compare against every grant unconditionally.
    // Accumulate match flag via subtle's choice; never early-exit on
    // length or content mismatch so observable timing is grant-count
    // dependent only, not token-shape dependent.
    let mut matched: Option<Resolved> = None;
    let mut any_match: u8 = 0;
    for g in &tables.tokens {
        let len_eq = u8::from(provided.len() == g.token_bytes.len());
        // ct_eq panics on length mismatch; gate behind the length check
        // but still iterate every grant so the loop bound is constant.
        let ct = if provided.len() == g.token_bytes.len() {
            u8::from(bool::from(provided.ct_eq(&g.token_bytes)))
        } else {
            0
        };
        let m = len_eq & ct;
        any_match |= m;
        if m == 1 && matched.is_none() {
            matched = Some(Resolved {
                scopes: g.scopes.clone(),
                author_id: g.author_id,
                agent_name: g.agent_name.clone(),
                nodes: Arc::new(Vec::new()),
                method: AuthMethod::Bearer,
            });
        }
    }
    if any_match == 0 {
        return None;
    }
    matched
}

/// Axum middleware: authenticates a request by declared identity
/// (`X-Homelab-Agent`, sprint 049) or, while the transition window is
/// open, by `Authorization: Bearer` against the legacy grants. On a
/// match the caller's scope set is inserted into request extensions as
/// [`AuthenticatedScopes`], the author as [`AuthenticatedAuthor`], and
/// the resolved tailnet origin as [`AuthenticatedPeer`].
///
/// The name is kept (rather than `require_auth`) because it is the
/// installed layer everywhere in both routers and in the MCP mount;
/// renaming it would be churn across the codebase for no behaviour.
///
/// # Errors
/// [`ApiError::Unauthorized`] when neither credential matches, or
/// [`ApiError::NodeNotAllowed`] when whois enforcement is on and the
/// caller's node is not one this identity is pinned to.
pub async fn require_bearer(
    State(state): State<AuthState>,
    mut req: Request<Body>,
    next: Next,
) -> Result<Response, ApiError> {
    let tables = state.tables();

    // 1. The declared identity, when one is presented. A header that is
    //    present but unknown is a 401 and does NOT fall through to the
    //    bearer path: the caller said who it was and was wrong, and
    //    silently authenticating it as something else would be the
    //    least honest outcome available.
    let declared = req
        .headers()
        .get(AGENT_HEADER)
        .and_then(|h| h.to_str().ok())
        .map(str::trim)
        .filter(|d| !d.is_empty());

    let resolved = if let Some(name) = declared {
        resolve_identity(&tables, name).ok_or(ApiError::Unauthorized)?
    } else {
        // 2. Legacy bearer — live only while the window is open.
        let token = req
            .headers()
            .get(AUTHORIZATION)
            .and_then(|h| h.to_str().ok())
            .and_then(|h| h.strip_prefix("Bearer "))
            .map(str::trim)
            .ok_or(ApiError::Unauthorized)?;
        resolve_bearer(&tables, token.as_bytes()).ok_or(ApiError::Unauthorized)?
    };

    // 3. The tailnet cross-check. Record-only unless enforcement is on,
    //    and a failure to resolve is `unknown`, never a refusal.
    let addr = caller_addr(req.headers(), req.extensions().get::<PeerAddr>().copied());
    let node = match (&state.whois, addr) {
        (Some(whois), Some(ip)) => whois.resolve(ip).await,
        _ => None,
    };

    if state.enforce_nodes && !resolved.nodes.is_empty() {
        let allowed = node
            .as_deref()
            .is_some_and(|n| resolved.nodes.iter().any(|a| a == n));
        if !allowed {
            // Names both sides: which identity, and which node it
            // actually came from. A 403 that says only "denied" makes
            // the operator go and find this out by hand.
            return Err(ApiError::NodeNotAllowed {
                agent_name: resolved.agent_name.to_string(),
                node: node.clone().unwrap_or_else(|| "unknown".into()),
                allowed: resolved.nodes.join(", "),
            });
        }
    }

    let peer = AuthenticatedPeer {
        addr,
        node,
        method: resolved.method,
    };

    // The request log (WI 2389). Writes at info, reads at debug: a
    // write is the thing the record exists to explain after the fact,
    // and logging every read at info would bury it under klams-view's
    // polling.
    let is_write = !matches!(
        *req.method(),
        axum::http::Method::GET | axum::http::Method::HEAD | axum::http::Method::OPTIONS
    );
    let path = req.uri().path();
    if is_write {
        tracing::info!(
            agent_name = %resolved.agent_name,
            tailnet_node = %peer.node_or_unknown(),
            auth = %resolved.method,
            method = %req.method(),
            %path,
            "authenticated write"
        );
    } else {
        tracing::debug!(
            agent_name = %resolved.agent_name,
            tailnet_node = %peer.node_or_unknown(),
            auth = %resolved.method,
            method = %req.method(),
            %path,
            "authenticated read"
        );
    }

    req.extensions_mut()
        .insert(AuthenticatedScopes(resolved.scopes));
    req.extensions_mut().insert(AuthenticatedAuthor {
        author_id: resolved.author_id,
        agent_name: resolved.agent_name,
    });
    req.extensions_mut().insert(peer);
    Ok(next.run(req).await)
}

/// Axum middleware factory that gates a route on a required [`Scope`].
/// Must be layered *after* [`require_bearer`] so the
/// [`AuthenticatedScopes`] extension is populated.
#[must_use = "the returned middleware closure must be installed on a route"]
pub fn require_scope(
    needed: Scope,
) -> impl Fn(
    AxumRequest,
    Next,
)
    -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Response, ApiError>> + Send>>
       + Clone {
    move |req: AxumRequest, next: Next| {
        let needed = needed;
        Box::pin(async move {
            let granted = req
                .extensions()
                .get::<AuthenticatedScopes>()
                .ok_or(ApiError::Unauthorized)?
                .0
                .clone();
            if granted.iter().any(|s| s.satisfies(needed)) {
                Ok(next.run(req).await)
            } else {
                Err(ApiError::ScopeInsufficient { needed })
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::to_bytes,
        http::{Request, StatusCode},
        middleware,
        routing::get,
        Router,
    };
    use tower::ServiceExt;

    fn app() -> Router {
        Router::new()
            .route("/protected", get(|| async { "ok" }))
            .layer(middleware::from_fn_with_state(
                AuthState::new("super-secret"),
                require_bearer,
            ))
            .route("/healthz", get(|| async { "ok" }))
    }

    #[tokio::test]
    async fn missing_header_is_unauthorized() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/protected")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn wrong_token_is_unauthorized() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/protected")
                    .header(AUTHORIZATION, "Bearer wrong-token-x")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn correct_token_passes() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/protected")
                    .header(AUTHORIZATION, "Bearer super-secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = to_bytes(resp.into_body(), 1024).await.unwrap();
        assert_eq!(&body[..], b"ok");
    }

    async fn probe(router: &Router, token: &str) -> StatusCode {
        router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/protected")
                    .header(AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
            .status()
    }

    /// Sprint 018 (WI #61) — grants are hot-swappable: the router keeps
    /// its middleware (holding a CLONE of `AuthState`), and a
    /// `replace_grants` on the original handle must be visible to it —
    /// added tokens authenticate, removed tokens stop authenticating.
    #[tokio::test]
    async fn replace_grants_swaps_token_table_for_live_router() {
        let state = AuthState::new("old-token");
        let router = Router::new()
            .route("/protected", get(|| async { "ok" }))
            .layer(middleware::from_fn_with_state(
                state.clone(),
                require_bearer,
            ));

        assert_eq!(probe(&router, "old-token").await, StatusCode::OK);
        assert_eq!(probe(&router, "new-token").await, StatusCode::UNAUTHORIZED);

        state.replace_grants(vec![TokenGrant::new(
            "new-token",
            vec![Scope::Read],
            Some("rotated".into()),
        )]);

        assert_eq!(
            probe(&router, "old-token").await,
            StatusCode::UNAUTHORIZED,
            "removed token must stop authenticating after reload"
        );
        assert_eq!(
            probe(&router, "new-token").await,
            StatusCode::OK,
            "added token must authenticate after reload"
        );
    }

    // -----------------------------------------------------------------
    // Sprint 049 — declared identities, the transition window, and the
    // tailnet cross-check.
    // -----------------------------------------------------------------

    /// A resolver that answers from a fixed map, so the middleware can
    /// be exercised without a tailnet.
    #[derive(Debug)]
    struct FakeWhois(Vec<(&'static str, &'static str)>);

    #[async_trait::async_trait]
    impl crate::whois::NodeResolver for FakeWhois {
        async fn resolve(&self, addr: std::net::IpAddr) -> Option<String> {
            let addr = addr.to_string();
            self.0
                .iter()
                .find(|(a, _)| *a == addr)
                .map(|(_, n)| (*n).to_string())
        }
    }

    fn identity(name: &str, scopes: Vec<Scope>, nodes: &[&str]) -> Identity {
        Identity {
            agent_name: Arc::new(name.to_string()),
            scopes: Arc::new(scopes),
            label: Some(name.to_string()),
            author_id: Uuid::from_u128(u128::from(name.len() as u64) + 1),
            nodes: Arc::new(nodes.iter().map(ToString::to_string).collect()),
        }
    }

    /// Echoes back what the middleware stamped, so a test can assert on
    /// attribution rather than merely on the status code.
    async fn echo_identity(
        axum::Extension(author): axum::Extension<AuthenticatedAuthor>,
        axum::Extension(peer): axum::Extension<AuthenticatedPeer>,
    ) -> String {
        format!(
            "{}|{}|{}",
            author.agent_name,
            peer.node_or_unknown(),
            peer.method
        )
    }

    fn app_with(state: AuthState) -> Router {
        Router::new()
            .route("/protected", get(echo_identity).post(echo_identity))
            .layer(middleware::from_fn_with_state(state, require_bearer))
    }

    async fn send(router: &Router, req: Request<Body>) -> (StatusCode, String) {
        let resp = router.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let body = to_bytes(resp.into_body(), 4096).await.unwrap();
        (status, String::from_utf8_lossy(&body).into_owned())
    }

    /// The core of WI 2388: a declared name authenticates, carries its
    /// configured scopes, and binds to its author — no secret involved.
    #[tokio::test]
    async fn declared_identity_authenticates_and_attributes() {
        let state = AuthState::with_tables(AuthTables::new(
            vec![],
            vec![identity("claude", vec![Scope::Read, Scope::Write], &[])],
        ));
        let (status, body) = send(
            &app_with(state),
            Request::builder()
                .uri("/protected")
                .header(AGENT_HEADER, "claude")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "claude|unknown|identity");
    }

    /// "An unknown name is a 401 exactly as an unknown token is today."
    #[tokio::test]
    async fn unknown_declared_identity_is_unauthorized() {
        let state = AuthState::with_tables(AuthTables::new(
            vec![],
            vec![identity("claude", vec![Scope::Read], &[])],
        ));
        let (status, _) = send(
            &app_with(state),
            Request::builder()
                .uri("/protected")
                .header(AGENT_HEADER, "mallory")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    /// A caller that declares an unknown name while ALSO holding a
    /// valid bearer is refused rather than quietly authenticated as the
    /// token's identity. Falling through would attribute its writes to
    /// an agent it did not claim to be — the one outcome worse than a
    /// 401.
    #[tokio::test]
    async fn a_bad_declared_name_does_not_fall_back_to_the_bearer() {
        let state = AuthState::with_tables(AuthTables::new(
            vec![TokenGrant::new_with_author(
                "token-that-is-long-enough",
                vec![Scope::Read],
                Some("legacy".into()),
                Uuid::from_u128(7),
                "kmon",
            )],
            vec![identity("claude", vec![Scope::Read], &[])],
        ));
        let (status, _) = send(
            &app_with(state),
            Request::builder()
                .uri("/protected")
                .header(AGENT_HEADER, "not-a-configured-name")
                .header(AUTHORIZATION, "Bearer token-that-is-long-enough")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    /// The transition window (proposal guardrail): while
    /// `[[auth.tokens]]` rows exist, both credentials work and resolve
    /// to their own authors. This is the property that means no client
    /// breaks during the sprint.
    #[tokio::test]
    async fn both_credentials_work_while_the_window_is_open() {
        let tables = AuthTables::new(
            vec![TokenGrant::new_with_author(
                "token-that-is-long-enough",
                vec![Scope::Read],
                Some("kmon".into()),
                Uuid::from_u128(7),
                "kmon",
            )],
            vec![identity("claude", vec![Scope::Read], &[])],
        );
        assert!(tables.legacy_window_open());
        let app = app_with(AuthState::with_tables(tables));

        let (status, body) = send(
            &app,
            Request::builder()
                .uri("/protected")
                .header(AUTHORIZATION, "Bearer token-that-is-long-enough")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "kmon|unknown|bearer");

        let (status, body) = send(
            &app,
            Request::builder()
                .uri("/protected")
                .header(AGENT_HEADER, "claude")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "claude|unknown|identity");
    }

    /// What korg:2450 leaves behind: no token rows, so the window is
    /// closed and a bearer — any bearer — no longer authenticates.
    #[tokio::test]
    async fn closing_the_window_is_deleting_the_token_rows() {
        let tables = AuthTables::new(vec![], vec![identity("claude", vec![Scope::Read], &[])]);
        assert!(!tables.legacy_window_open());
        let (status, _) = send(
            &app_with(AuthState::with_tables(tables)),
            Request::builder()
                .uri("/protected")
                .header(AUTHORIZATION, "Bearer token-that-is-long-enough")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    /// Scopes are looked up by name and stay flat: a read-only identity
    /// cannot write (WI 2388 acceptance).
    #[tokio::test]
    async fn a_read_only_identity_cannot_write() {
        let state = AuthState::with_tables(AuthTables::new(
            vec![],
            vec![
                identity("klams-view", vec![Scope::Read], &[]),
                identity("claude", vec![Scope::Read, Scope::Write], &[]),
            ],
        ));
        let app = Router::new()
            .route("/w", axum::routing::post(|| async { "ok" }))
            .route_layer(middleware::from_fn(require_scope(Scope::Write)))
            .layer(middleware::from_fn_with_state(state, require_bearer));

        let (status, _) = send(
            &app,
            Request::builder()
                .method("POST")
                .uri("/w")
                .header(AGENT_HEADER, "klams-view")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);

        let (status, _) = send(
            &app,
            Request::builder()
                .method("POST")
                .uri("/w")
                .header(AGENT_HEADER, "claude")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    /// WI 2389 acceptance: a request from kai shows node `kai`. The
    /// address comes from `X-Forwarded-For`, which is what `tailscale
    /// serve` sets (measured — see `crate::whois`).
    #[tokio::test]
    async fn the_tailnet_node_is_recorded_from_the_forwarded_address() {
        let state = AuthState::with_tables(AuthTables::new(
            vec![],
            vec![identity("claude", vec![Scope::Read], &[])],
        ))
        .with_whois(
            Arc::new(FakeWhois(vec![
                ("100.97.109.60", "kai"),
                ("100.91.170.122", "kubs0"),
            ])),
            false,
        );
        let app = app_with(state);

        for (addr, expected) in [("100.97.109.60", "kai"), ("100.91.170.122", "kubs0")] {
            let (status, body) = send(
                &app,
                Request::builder()
                    .uri("/protected")
                    .header(AGENT_HEADER, "claude")
                    .header("x-forwarded-for", addr)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(body, format!("claude|{expected}|identity"));
        }
    }

    /// Only the first `X-Forwarded-For` entry is read — the original
    /// client in the standard's ordering.
    #[tokio::test]
    async fn the_first_forwarded_entry_is_the_client() {
        let state = AuthState::with_tables(AuthTables::new(
            vec![],
            vec![identity("claude", vec![Scope::Read], &[])],
        ))
        .with_whois(Arc::new(FakeWhois(vec![("100.97.109.60", "kai")])), false);
        let (_, body) = send(
            &app_with(state),
            Request::builder()
                .uri("/protected")
                .header(AGENT_HEADER, "claude")
                .header("x-forwarded-for", "100.97.109.60, 100.64.0.1")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(body, "claude|kai|identity");
    }

    /// "With tailscaled stopped, writes still succeed and record
    /// `unknown`." An unresolvable address is data, not a refusal.
    #[tokio::test]
    async fn an_unresolvable_node_is_unknown_and_never_refuses() {
        let state = AuthState::with_tables(AuthTables::new(
            vec![],
            vec![identity("claude", vec![Scope::Read, Scope::Write], &[])],
        ))
        .with_whois(Arc::new(FakeWhois(vec![])), false);
        let (status, body) = send(
            &app_with(state),
            Request::builder()
                .method("POST")
                .uri("/protected")
                .header(AGENT_HEADER, "claude")
                .header("x-forwarded-for", "100.64.0.99")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "a whois miss must never refuse");
        assert_eq!(body, "claude|unknown|identity");
    }

    /// Enforcement OFF (the default) records a mismatch and allows it.
    #[tokio::test]
    async fn a_mismatched_node_is_recorded_not_refused_by_default() {
        let state = AuthState::with_tables(AuthTables::new(
            vec![],
            vec![identity("kmon", vec![Scope::Read], &["kubs0"])],
        ))
        .with_whois(Arc::new(FakeWhois(vec![("100.97.109.60", "kai")])), false);
        let (status, body) = send(
            &app_with(state),
            Request::builder()
                .uri("/protected")
                .header(AGENT_HEADER, "kmon")
                .header("x-forwarded-for", "100.97.109.60")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "kmon|kai|identity");
    }

    /// Enforcement ON: a mismatch is a 403 that names the identity, the
    /// node it actually came from, and what was allowed.
    #[tokio::test]
    async fn enforcement_on_refuses_a_mismatched_node_and_names_both() {
        let state = AuthState::with_tables(AuthTables::new(
            vec![],
            vec![identity("kmon", vec![Scope::Read], &["kubs0"])],
        ))
        .with_whois(Arc::new(FakeWhois(vec![("100.97.109.60", "kai")])), true);
        let (status, body) = send(
            &app_with(state),
            Request::builder()
                .uri("/protected")
                .header(AGENT_HEADER, "kmon")
                .header("x-forwarded-for", "100.97.109.60")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(body.contains("kmon"), "{body}");
        assert!(body.contains("kai"), "{body}");
        assert!(body.contains("kubs0"), "{body}");
    }

    /// Enforcement is per-identity opt-in: an identity that declares no
    /// `nodes` is unconstrained even with the toggle on, so turning it
    /// on cannot lock every caller out at once.
    #[tokio::test]
    async fn enforcement_leaves_unpinned_identities_alone() {
        let state = AuthState::with_tables(AuthTables::new(
            vec![],
            vec![
                identity("kmon", vec![Scope::Read], &["kubs0"]),
                identity("claude", vec![Scope::Read], &[]),
            ],
        ))
        .with_whois(Arc::new(FakeWhois(vec![("100.97.109.60", "kai")])), true);
        let app = app_with(state);

        let (status, _) = send(
            &app,
            Request::builder()
                .uri("/protected")
                .header(AGENT_HEADER, "claude")
                .header("x-forwarded-for", "100.97.109.60")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "an unpinned identity is unconstrained"
        );

        // And the pinned one still passes from the node it is pinned to.
        let (status, _) = send(
            &app,
            Request::builder()
                .uri("/protected")
                .header(AGENT_HEADER, "kmon")
                .header("x-forwarded-for", "100.97.109.60")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    /// Enforcement with an UNRESOLVABLE node refuses a pinned identity.
    /// This is the one place a whois failure changes an outcome, and it
    /// is behind the default-off toggle: an operator who turns
    /// enforcement on is asking for "prove where you came from", and
    /// "could not tell" is not a proof.
    #[tokio::test]
    async fn enforcement_refuses_a_pinned_identity_it_cannot_place() {
        let state = AuthState::with_tables(AuthTables::new(
            vec![],
            vec![identity("kmon", vec![Scope::Read], &["kubs0"])],
        ))
        .with_whois(Arc::new(FakeWhois(vec![])), true);
        let (status, body) = send(
            &app_with(state),
            Request::builder()
                .uri("/protected")
                .header(AGENT_HEADER, "kmon")
                .header("x-forwarded-for", "100.64.0.99")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(body.contains("unknown"), "{body}");
    }

    /// The socket peer is the fallback when nothing forwarded an
    /// address — the direct (non-`tailscale serve`) deployment.
    #[tokio::test]
    async fn the_socket_peer_is_used_when_nothing_was_forwarded() {
        let state = AuthState::with_tables(AuthTables::new(
            vec![],
            vec![identity("claude", vec![Scope::Read], &[])],
        ))
        .with_whois(Arc::new(FakeWhois(vec![("100.97.109.60", "kai")])), false);
        let mut req = Request::builder()
            .uri("/protected")
            .header(AGENT_HEADER, "claude")
            .body(Body::empty())
            .unwrap();
        req.extensions_mut()
            .insert(PeerAddr("100.97.109.60:44321".parse().unwrap()));
        let (status, body) = send(&app_with(state), req).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "claude|kai|identity");
    }

    /// SIGHUP hot-reload covers the new table too (WI 2388: "SIGHUP
    /// hot-reload keeps working for the new table").
    #[tokio::test]
    async fn replace_tables_swaps_identities_for_a_live_router() {
        let state = AuthState::with_tables(AuthTables::new(
            vec![],
            vec![identity("old-agent", vec![Scope::Read], &[])],
        ));
        let app = app_with(state.clone());

        let probe = |name: &'static str| {
            let app = app.clone();
            async move {
                send(
                    &app,
                    Request::builder()
                        .uri("/protected")
                        .header(AGENT_HEADER, name)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .0
            }
        };

        assert_eq!(probe("old-agent").await, StatusCode::OK);
        assert_eq!(probe("new-agent").await, StatusCode::UNAUTHORIZED);

        state.replace_tables(AuthTables::new(
            vec![],
            vec![identity("new-agent", vec![Scope::Read], &[])],
        ));

        assert_eq!(
            probe("old-agent").await,
            StatusCode::UNAUTHORIZED,
            "a removed identity must stop authenticating after reload"
        );
        assert_eq!(probe("new-agent").await, StatusCode::OK);
    }

    /// An empty or whitespace-only header is treated as "not presented"
    /// rather than as an identity named "", so a client that sets the
    /// header to nothing falls back to its bearer instead of being
    /// refused with a confusing 401.
    #[tokio::test]
    async fn an_empty_agent_header_falls_through_to_the_bearer() {
        let state = AuthState::with_tables(AuthTables::new(
            vec![TokenGrant::new_with_author(
                "token-that-is-long-enough",
                vec![Scope::Read],
                Some("kmon".into()),
                Uuid::from_u128(7),
                "kmon",
            )],
            vec![identity("claude", vec![Scope::Read], &[])],
        ));
        let (status, body) = send(
            &app_with(state),
            Request::builder()
                .uri("/protected")
                .header(AGENT_HEADER, "   ")
                .header(AUTHORIZATION, "Bearer token-that-is-long-enough")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "kmon|unknown|bearer");
    }

    #[tokio::test]
    async fn healthz_is_public() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
