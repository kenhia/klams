//! Sprint 007 T027 (foundational) — scoped multi-token auth.
//!
//! Exercises `klams_api::auth::AuthState` directly: a grant list
//! dispatches scoped tokens correctly, and (sprint 034, #703) the
//! config-side rule that privileged grants must be attributable is
//! pinned where the runtime grant table is built.

use klams_api::auth::{AuthState, TokenGrant};
use klams_types::{AuthConfigError, Scope, TokenGrantConfig};
use std::sync::Arc;

/// Sprint 034 (#703): the `[auth] bearer_token` config path is retired
/// — it materialized exactly this shape: all four scopes, `system`
/// binding, no `agent_name`. The same shape expressed as a
/// `[[auth.tokens]]` grant must now be refused by validation, which is
/// what makes the retirement complete rather than cosmetic. (The
/// in-code `AuthState::new` single-token constructor remains as a test
/// convenience only; `main.rs` builds grants exclusively from
/// validated `[[auth.tokens]]` entries.)
#[test]
fn an_unattributed_full_scope_grant_is_no_longer_expressible() {
    let legacy_shape = TokenGrantConfig {
        token: "super-secret-legacy-token".into(),
        scopes: vec![Scope::Read, Scope::Write, Scope::Manage, Scope::Admin],
        label: Some("legacy".into()),
        agent_name: None,
    };
    assert!(matches!(
        legacy_shape.validate(),
        Err(AuthConfigError::PrivilegedGrantNeedsAgentName)
    ));

    // The attributable equivalent (what kubs0 actually runs, e.g. the
    // `ken-admin` grant) stays valid — the retirement removes the
    // unattributable credential, not the capability.
    let attributed = TokenGrantConfig {
        agent_name: Some("ken_admin".into()),
        ..legacy_shape
    };
    attributed.validate().expect("attributed full-scope grant");
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
