//! Identity auth middleware.
//!
//! Public paths (e.g. `/healthz`, `/metrics`) should be mounted
//! outside the protected router.
//!
//! # The credential is a declared name
//!
//! A klams bearer token was a name tag, not a lock. Under the homelab
//! threat model — single user, his agents, one tailnet, agents already
//! holding sudo everywhere — the token's only job was to say *which
//! `agent_name`*, and a declared name does that without a secret.
//!
//! So a caller sends `X-Homelab-Agent: <agent_name>` and klams looks
//! the row up in `[[auth.identities]]`. An unknown name is a 401,
//! exactly as an unknown token was. Authorship has been keyed on
//! `agent_name` rather than on token bytes since sprint 009, which is
//! why the change needed no migration: nothing about attribution moved.
//!
//! Scopes are per-identity; [`klams_types::Scope`] sets are stashed in
//! the request extensions as [`AuthenticatedScopes`] so downstream
//! `require_scope(...)` layers enforce per-route permission tiers.
//!
//! # Sprint 052 — the bearer path is gone
//!
//! Sprint 049 opened a transition window by keeping `[[auth.tokens]]`
//! alive beside the identities; sprint 050 deleted every row, which is
//! what closed it (049 D-1: the rows *were* the flag). This sprint
//! deletes `resolve_bearer`, its constant-time loop and the token
//! table. No token authenticates anything.
//!
//! `Authorization` is still **read**, and authenticates nothing
//! (sprint 052 D-2, borrowed from kaed 024 D-2): a request carrying a
//! bearer and no `X-Homelab-Agent` gets a 401 whose body names the
//! header to send and the one to drop. WI 2490 is the justification —
//! three clients sat sending a retired credential and getting 401 for a
//! full day, because from outside "sent a retired credential" and "sent
//! nothing" were indistinguishable.
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
use klams_types::{AuthenticatedAuthor, AuthenticatedPeer, AuthenticatedScopes, Scope};
use std::net::IpAddr;
use std::sync::Arc;
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

/// The auth table, swapped as a unit on SIGHUP.
///
/// Sprint 049 made this two tables (tokens + identities) so a reload
/// could install both halves atomically; sprint 052 deleted the token
/// half. The struct stays — it is what [`AuthState::replace_tables`]
/// swaps, and the name is the one both routers and the reload task
/// already use.
#[derive(Clone, Debug, Default)]
pub struct AuthTables {
    pub identities: Vec<Identity>,
}

impl AuthTables {
    #[must_use]
    pub fn new(identities: Vec<Identity>) -> Self {
        Self { identities }
    }
}

// `AuthenticatedScopes` and `AuthenticatedAuthor` moved down to
// `klams-types` in sprint 031 (#645). They are the request-extension
// types both surfaces read, and keeping them here forced `klams-mcp` to
// depend on the REST crate for two structs and nothing else. Imported
// above; `require_bearer` still stamps them.

/// Sprint 018 (WI #61) — the auth table sits behind an `RwLock` so
/// `[[auth.identities]]` edits can be hot-reloaded (SIGHUP) without a
/// service restart. All clones (REST layer, `/mcp` layer, the reload
/// task) share one table; [`AuthState::replace_tables`] swaps it
/// atomically. In-flight requests hold at most a snapshot `Arc` for
/// the duration of their own auth check.
///
/// Sprint 049 added the whois resolver, because the node cross-check
/// happens at exactly the same point as the identity lookup and reads
/// the same request.
#[derive(Clone)]
pub struct AuthState {
    tables: Arc<std::sync::RwLock<Arc<AuthTables>>>,
    /// `None` = whois disabled; every request records `unknown`.
    whois: Option<Arc<dyn crate::whois::NodeResolver>>,
    /// `[auth.whois] enforce`. Off by default (WI 2389).
    enforce_nodes: bool,
}

impl AuthState {
    /// Build from an identity list. Whois is off until
    /// [`Self::with_whois`] installs a resolver.
    #[must_use]
    pub fn with_identities(identities: Vec<Identity>) -> Self {
        Self::with_tables(AuthTables::new(identities))
    }

    /// Build from a prepared [`AuthTables`]. Whois is off until
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

    /// Atomically swap the auth table (WI #61 hot-reload). Visible to
    /// every clone of this
    /// `AuthState` — the next auth check on any route uses the new
    /// tables; requests already past their auth check are unaffected.
    pub fn replace_tables(&self, tables: AuthTables) {
        let mut w = self
            .tables
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *w = Arc::new(tables);
    }

    /// Snapshot the current tables. The snapshot is immutable and
    /// outlives a concurrent [`Self::replace_tables`].
    fn tables(&self) -> Arc<AuthTables> {
        self.tables
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
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
            .field("identity_count", &tables.identities.len())
            .field("whois", &self.whois.is_some())
            .field("enforce_nodes", &self.enforce_nodes)
            // The table itself is summarised by the count above.
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
    })
}

/// Axum middleware: authenticates a request by declared identity
/// (`X-Homelab-Agent`). On a match the caller's scope set is inserted
/// into request extensions as [`AuthenticatedScopes`], the author as
/// [`AuthenticatedAuthor`], and the resolved tailnet origin as
/// [`AuthenticatedPeer`].
///
/// The name is kept (rather than `require_auth`) because it is the
/// installed layer everywhere in both routers and in the MCP mount;
/// renaming it would be churn across the codebase for no behaviour.
///
/// # Errors
/// [`ApiError::Unauthorized`] when no name is declared or the declared
/// name is unknown, [`ApiError::BearerRetired`] when the caller sent a
/// bearer instead of a name (sprint 052 D-2 — still a 401, but one that
/// says what to change), or [`ApiError::NodeNotAllowed`] when whois
/// enforcement is on and the caller's node is not one this identity is
/// pinned to.
pub async fn require_bearer(
    State(state): State<AuthState>,
    mut req: Request<Body>,
    next: Next,
) -> Result<Response, ApiError> {
    let tables = state.tables();

    // 1. The declared identity. A name that is present but unknown is a
    //    401: the caller said who it was and was wrong, and silently
    //    authenticating it as something else would be the least honest
    //    outcome available.
    let declared = req
        .headers()
        .get(AGENT_HEADER)
        .and_then(|h| h.to_str().ok())
        .map(str::trim)
        .filter(|d| !d.is_empty());

    let Some(name) = declared else {
        // 2. No name. If the caller sent a bearer instead, say so
        //    (sprint 052 D-2): it authenticates nothing, but "you sent
        //    a retired credential" and "you sent nothing" are different
        //    facts and the caller can only act on the first.
        //    `had_bearer` is a bool and never reaches a lookup.
        let had_bearer = req.headers().contains_key(AUTHORIZATION);
        return Err(if had_bearer {
            ApiError::BearerRetired
        } else {
            ApiError::Unauthorized
        });
    };
    let resolved = resolve_identity(&tables, name).ok_or(ApiError::Unauthorized)?;

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

    let peer = AuthenticatedPeer { addr, node };

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
            auth = "identity",
            method = %req.method(),
            %path,
            "authenticated write"
        );
    } else {
        tracing::debug!(
            agent_name = %resolved.agent_name,
            tailnet_node = %peer.node_or_unknown(),
            auth = "identity",
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
                AuthState::with_identities(vec![identity(
                    "claude",
                    vec![Scope::Read, Scope::Write],
                    &[],
                )]),
                require_bearer,
            ))
            .route("/healthz", get(|| async { "ok" }))
    }

    #[tokio::test]
    async fn missing_credential_is_unauthorized() {
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
    async fn unknown_name_is_unauthorized() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/protected")
                    .header(AGENT_HEADER, "nobody")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn declared_name_passes() {
        let resp = app()
            .oneshot(
                Request::builder()
                    .uri("/protected")
                    .header(AGENT_HEADER, "claude")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = to_bytes(resp.into_body(), 1024).await.unwrap();
        assert_eq!(&body[..], b"ok");
    }

    // -----------------------------------------------------------------
    // Declared identities and the tailnet cross-check.
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
        format!("{}|{}", author.agent_name, peer.node_or_unknown())
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
        let state = AuthState::with_tables(AuthTables::new(vec![identity(
            "claude",
            vec![Scope::Read, Scope::Write],
            &[],
        )]));
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
        assert_eq!(body, "claude|unknown");
    }

    /// "An unknown name is a 401 exactly as an unknown token is today."
    #[tokio::test]
    async fn unknown_declared_identity_is_unauthorized() {
        let state = AuthState::with_tables(AuthTables::new(vec![identity(
            "claude",
            vec![Scope::Read],
            &[],
        )]));
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

    /// An unknown declared name is refused even when the caller also
    /// presents an `Authorization` header. Nothing falls through: the
    /// bearer is never looked up (there is nothing to look it up in),
    /// and the 401 is the plain one, not the retired-bearer
    /// diagnostic — the caller DID declare a name, it was just wrong.
    #[tokio::test]
    async fn a_bad_declared_name_is_refused_even_with_a_bearer() {
        let state = AuthState::with_tables(AuthTables::new(vec![identity(
            "claude",
            vec![Scope::Read],
            &[],
        )]));
        let (status, body) = send(
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
        assert!(!body.contains("bearer_retired"), "{body}");
    }

    /// Sprint 052: no token authenticates, whatever its value, and
    /// whatever the identities table holds. This is the deletion's
    /// headline property.
    #[tokio::test]
    async fn no_bearer_authenticates() {
        let tables = AuthTables::new(vec![identity("claude", vec![Scope::Read], &[])]);
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

    /// Sprint 052 D-2: a bearer with no declared name gets a 401 whose
    /// body names the header to send and the one to drop. WI 2490 is
    /// the justification — three clients sat on a bare 401 for a day
    /// because "sent a retired credential" and "sent nothing" looked
    /// identical from outside.
    #[tokio::test]
    async fn a_retired_bearer_gets_a_self_diagnosing_401() {
        let tables = AuthTables::new(vec![identity("claude", vec![Scope::Read], &[])]);
        let (status, body) = send(
            &app_with(AuthState::with_tables(tables)),
            Request::builder()
                .uri("/protected")
                .header(AUTHORIZATION, "Bearer anything-at-all")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(body.contains("bearer_retired"), "{body}");
        assert!(body.contains("X-Homelab-Agent"), "{body}");
        assert!(body.contains("Authorization"), "{body}");
    }

    /// ...and a caller sending nothing at all still gets the plain 401,
    /// so the two cases stay distinguishable. That distinction is the
    /// whole point of D-2; collapsing them would restore WI 2490.
    #[tokio::test]
    async fn no_credential_at_all_is_the_plain_401() {
        let tables = AuthTables::new(vec![identity("claude", vec![Scope::Read], &[])]);
        let (status, body) = send(
            &app_with(AuthState::with_tables(tables)),
            Request::builder()
                .uri("/protected")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(body.contains("unauthorized"), "{body}");
        assert!(!body.contains("bearer_retired"), "{body}");
    }

    /// Scopes are looked up by name and stay flat: a read-only identity
    /// cannot write (WI 2388 acceptance).
    #[tokio::test]
    async fn a_read_only_identity_cannot_write() {
        let state = AuthState::with_tables(AuthTables::new(vec![
            identity("klams-view", vec![Scope::Read], &[]),
            identity("claude", vec![Scope::Read, Scope::Write], &[]),
        ]));
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
        let state = AuthState::with_tables(AuthTables::new(vec![identity(
            "claude",
            vec![Scope::Read],
            &[],
        )]))
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
            assert_eq!(body, format!("claude|{expected}"));
        }
    }

    /// Only the first `X-Forwarded-For` entry is read — the original
    /// client in the standard's ordering.
    #[tokio::test]
    async fn the_first_forwarded_entry_is_the_client() {
        let state = AuthState::with_tables(AuthTables::new(vec![identity(
            "claude",
            vec![Scope::Read],
            &[],
        )]))
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
        assert_eq!(body, "claude|kai");
    }

    /// "With tailscaled stopped, writes still succeed and record
    /// `unknown`." An unresolvable address is data, not a refusal.
    #[tokio::test]
    async fn an_unresolvable_node_is_unknown_and_never_refuses() {
        let state = AuthState::with_tables(AuthTables::new(vec![identity(
            "claude",
            vec![Scope::Read, Scope::Write],
            &[],
        )]))
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
        assert_eq!(body, "claude|unknown");
    }

    /// Enforcement OFF (the default) records a mismatch and allows it.
    #[tokio::test]
    async fn a_mismatched_node_is_recorded_not_refused_by_default() {
        let state = AuthState::with_tables(AuthTables::new(vec![identity(
            "kmon",
            vec![Scope::Read],
            &["kubs0"],
        )]))
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
        assert_eq!(body, "kmon|kai");
    }

    /// Enforcement ON: a mismatch is a 403 that names the identity, the
    /// node it actually came from, and what was allowed.
    #[tokio::test]
    async fn enforcement_on_refuses_a_mismatched_node_and_names_both() {
        let state = AuthState::with_tables(AuthTables::new(vec![identity(
            "kmon",
            vec![Scope::Read],
            &["kubs0"],
        )]))
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
        let state = AuthState::with_tables(AuthTables::new(vec![
            identity("kmon", vec![Scope::Read], &["kubs0"]),
            identity("claude", vec![Scope::Read], &[]),
        ]))
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
        let state = AuthState::with_tables(AuthTables::new(vec![identity(
            "kmon",
            vec![Scope::Read],
            &["kubs0"],
        )]))
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
        let state = AuthState::with_tables(AuthTables::new(vec![identity(
            "claude",
            vec![Scope::Read],
            &[],
        )]))
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
        assert_eq!(body, "claude|kai");
    }

    /// SIGHUP hot-reload covers the new table too (WI 2388: "SIGHUP
    /// hot-reload keeps working for the new table").
    #[tokio::test]
    async fn replace_tables_swaps_identities_for_a_live_router() {
        let state = AuthState::with_tables(AuthTables::new(vec![identity(
            "old-agent",
            vec![Scope::Read],
            &[],
        )]));
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

        state.replace_tables(AuthTables::new(vec![identity(
            "new-agent",
            vec![Scope::Read],
            &[],
        )]));

        assert_eq!(
            probe("old-agent").await,
            StatusCode::UNAUTHORIZED,
            "a removed identity must stop authenticating after reload"
        );
        assert_eq!(probe("new-agent").await, StatusCode::OK);
    }

    /// An empty or whitespace-only header is treated as "not presented"
    /// rather than as an identity named "". Sprint 052: with no bearer
    /// path left to fall through to, that means a 401 — and, when the
    /// caller also sent an `Authorization` header, the D-2 diagnostic
    /// one, which is exactly the client this case describes.
    #[tokio::test]
    async fn an_empty_agent_header_is_not_an_identity() {
        let state = AuthState::with_tables(AuthTables::new(vec![identity(
            "claude",
            vec![Scope::Read],
            &[],
        )]));
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
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(body.contains("bearer_retired"), "{body}");
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
