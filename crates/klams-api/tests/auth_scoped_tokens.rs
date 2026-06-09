//! Sprint 007 T027 (foundational) — scoped multi-token auth.
//!
//! Exercises `klams_api::auth::AuthState` directly: legacy single
//! `bearer_token` config still authenticates with full scope, and a
//! grant list dispatches scoped tokens correctly. The full MCP-level
//! integration arrives in Phase 3 when tools exist; this test pins
//! the foundational invariants.

use klams_api::auth::{AuthState, TokenGrant};
use klams_types::Scope;
use std::sync::Arc;

#[test]
fn legacy_bearer_grants_full_scope_label() {
    let state = AuthState::new("super-secret");
    // Legacy path: AuthState::new must construct a grant with all
    // scopes so existing single-token deployments keep working.
    let grants = state.grants_for_test();
    assert_eq!(grants.len(), 1);
    let scopes: &[Scope] = &grants[0].scopes;
    assert!(scopes.contains(&Scope::Read));
    assert!(scopes.contains(&Scope::Write));
    assert!(scopes.contains(&Scope::Admin));
    assert_eq!(grants[0].label.as_deref(), Some("legacy"));
}

#[test]
fn with_grants_preserves_per_token_scope_set() {
    let grants = vec![
        TokenGrant {
            token_bytes: Arc::new(b"read-only-token".to_vec()),
            scopes: Arc::new(vec![Scope::Read]),
            label: Some("viewer".into()),
            author_id: klams_types::SYSTEM_AUTHOR_ID,
            agent_name: Arc::new("system".into()),
        },
        TokenGrant {
            token_bytes: Arc::new(b"writer-token".to_vec()),
            scopes: Arc::new(vec![Scope::Read, Scope::Write]),
            label: Some("agent".into()),
            author_id: klams_types::SYSTEM_AUTHOR_ID,
            agent_name: Arc::new("system".into()),
        },
    ];
    let state = AuthState::with_grants(grants);
    let installed = state.grants_for_test();
    assert_eq!(installed.len(), 2);
    assert_eq!(&*installed[0].scopes, &[Scope::Read]);
    assert_eq!(&*installed[1].scopes, &[Scope::Read, Scope::Write]);
}
