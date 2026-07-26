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

use crate::ApiError;
use axum::{
    body::Body,
    extract::{Request as AxumRequest, State},
    http::{header::AUTHORIZATION, Request},
    middleware::Next,
    response::Response,
};
use klams_types::Scope;
use std::sync::Arc;
use subtle::ConstantTimeEq;
use uuid::Uuid;

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

/// Set of scopes attached to the currently authenticated request.
/// Inserted into request extensions by [`require_bearer`]; read by
/// [`require_scope`] when gating individual routes.
#[derive(Clone, Debug)]
pub struct AuthenticatedScopes(pub Arc<Vec<Scope>>);

/// Sprint 009 — author bound to the request's bearer token. Inserted
/// into request extensions by [`require_bearer`]; consumed by REST
/// write handlers to stamp `author_id` on enqueued jobs.
#[derive(Clone, Debug)]
pub struct AuthenticatedAuthor {
    pub author_id: Uuid,
    pub agent_name: Arc<String>,
}

/// Sprint 018 (WI #61) — the grant table sits behind an `RwLock` so
/// `[[auth.tokens]]` edits can be hot-reloaded (SIGHUP) without a
/// service restart. All clones (REST layer, `/mcp` layer, the reload
/// task) share one table; [`AuthState::replace_grants`] swaps it
/// atomically. In-flight requests hold at most a snapshot `Arc` for
/// the duration of their own auth check.
#[derive(Clone)]
pub struct AuthState {
    grants: Arc<std::sync::RwLock<Arc<Vec<TokenGrant>>>>,
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
        Self {
            grants: Arc::new(std::sync::RwLock::new(Arc::new(grants))),
        }
    }

    /// Atomically swap the grant table (WI #61 hot-reload). Visible to
    /// every clone of this `AuthState` — the next auth check on any
    /// route uses the new table; requests already past their auth
    /// check are unaffected.
    pub fn replace_grants(&self, grants: Vec<TokenGrant>) {
        let mut w = self
            .grants
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *w = Arc::new(grants);
    }

    /// Snapshot the current grant table. The snapshot is immutable and
    /// outlives a concurrent [`Self::replace_grants`].
    fn grants(&self) -> Arc<Vec<TokenGrant>> {
        self.grants
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Test-only accessor for the installed grant list. Exposed via a
    /// public method (rather than `pub` field) so the storage shape
    /// stays free to evolve.
    #[doc(hidden)]
    #[must_use]
    pub fn grants_for_test(&self) -> Arc<Vec<TokenGrant>> {
        self.grants()
    }
}

impl std::fmt::Debug for AuthState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthState")
            .field("grant_count", &self.grants().len())
            .finish()
    }
}

/// Axum middleware: requires `Authorization: Bearer <token>` matching
/// one of the configured grants via constant-time comparison. On match
/// the grant's scope set is inserted into request extensions as
/// [`AuthenticatedScopes`].
pub async fn require_bearer(
    State(state): State<AuthState>,
    mut req: Request<Body>,
    next: Next,
) -> Result<Response, ApiError> {
    let header = req
        .headers()
        .get(AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .ok_or(ApiError::Unauthorized)?;
    let token = header
        .strip_prefix("Bearer ")
        .ok_or(ApiError::Unauthorized)?
        .trim();
    let provided = token.as_bytes();

    // Constant-time loop: compare against every grant unconditionally.
    // Accumulate match flag via subtle's choice; never early-exit on
    // length or content mismatch so observable timing is grant-count
    // dependent only, not token-shape dependent.
    let mut matched: Option<Arc<Vec<Scope>>> = None;
    let mut matched_author: Option<(Uuid, Arc<String>)> = None;
    let mut any_match: u8 = 0;
    let grants = state.grants();
    for g in grants.iter() {
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
            matched = Some(g.scopes.clone());
            matched_author = Some((g.author_id, g.agent_name.clone()));
        }
    }

    if any_match == 0 {
        return Err(ApiError::Unauthorized);
    }

    if let Some(scopes) = matched {
        req.extensions_mut().insert(AuthenticatedScopes(scopes));
    }
    if let Some((author_id, agent_name)) = matched_author {
        req.extensions_mut().insert(AuthenticatedAuthor {
            author_id,
            agent_name,
        });
    }
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
