//! Bearer-token auth model with per-token scopes (sprint 007).
//!
//! [`Scope`] enumerates the three permission levels exposed by both the
//! legacy REST surface and the new MCP server. [`TokenGrantConfig`] is the
//! TOML-side shape (see `data-model.md` §5); [`TokenGrant`] is the
//! materialized runtime form with the token bytes wrapped for constant-time
//! comparison upstream.

use serde::{Deserialize, Serialize};

/// Permission tier attached to a bearer token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    Read,
    Write,
    /// Sprint 025 (#633) — cross-author curation: manage a memory whose
    /// author is somebody else. Self-management (deleting a memory you
    /// wrote) needs only [`Scope::Write`]; this tier is what a trusted
    /// agent needs to retract *another* author's stale record so the
    /// next session isn't misled by it. Deliberately **not** implied by
    /// [`Scope::Admin`], which keeps its own exclusivity over
    /// hard-delete / restore / list-deleted.
    Manage,
    Admin,
}

impl Scope {
    /// Returns true if a token holding `self` satisfies a route that
    /// requires `needed`. Scopes are independent (not hierarchical) — a
    /// "write" token does not automatically grant "read" unless the
    /// configured grant explicitly lists both. This holds for
    /// [`Scope::Manage`] too: it is a peer of the other three, so a
    /// grant that should curate cross-author must list `manage`
    /// alongside `read`/`write`.
    #[must_use]
    pub fn satisfies(self, needed: Scope) -> bool {
        self == needed
    }

    /// The scope's wire/TOML spelling — the same lowercase token used in
    /// `scopes = [...]` and in `scope_insufficient` error messages.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Manage => "manage",
            Self::Admin => "admin",
        }
    }
}

impl std::fmt::Display for Scope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// TOML-facing token grant entry (`[[auth.tokens]]`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenGrantConfig {
    pub token: String,
    pub scopes: Vec<Scope>,
    #[serde(default)]
    pub label: Option<String>,
    /// Sprint 009: agent identity bound to this token. Resolved to
    /// an `Author` at service startup; every REST write
    /// authenticated by this token is attributed to that author
    /// instead of `system`. `None` falls back to the seeded
    /// `system` author (back-compat for tokens issued before
    /// sprint 009).
    #[serde(default)]
    pub agent_name: Option<String>,
}

/// Validation errors for a bearer-token configuration.
#[derive(Debug, thiserror::Error)]
pub enum AuthConfigError {
    #[error("auth: at least one `[[auth.tokens]]` grant must be set")]
    NoTokens,
    #[error("auth: token must be at least 16 characters")]
    TokenTooShort,
    #[error("auth: token grant must declare at least one scope")]
    EmptyScopes,
    #[error("auth: token grant `agent_name` is invalid ({reason})")]
    InvalidAgentName { reason: AgentNameInvalidReason },
    /// Sprint 034 (#703): every privileged action must be attributable
    /// — the property sprint 025 was built around, closed here.
    #[error(
        "auth: a grant holding `manage` or `admin` must declare `agent_name` \
         so privileged actions are attributable"
    )]
    PrivilegedGrantNeedsAgentName,
    /// Sprint 034 (#703): the legacy single-token form is retired — it
    /// materialized a full-scope grant that could not declare an
    /// `agent_name`, which the rule above now forbids.
    #[error(
        "auth: `bearer_token` is retired (sprint 034); replace it with a \
         `[[auth.tokens]]` grant carrying `agent_name` — see docs/auth.md \
         for the migration note"
    )]
    LegacyBearerTokenRetired,
}

/// Reason an `agent_name` failed validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentNameInvalidReason {
    /// Empty after trim.
    Empty,
    /// Outside the 2..=64 byte length window.
    Length,
    /// Contains a character outside `[a-z0-9_-]`.
    Charset,
}

impl std::fmt::Display for AgentNameInvalidReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => f.write_str("empty"),
            Self::Length => f.write_str("length"),
            Self::Charset => f.write_str("charset"),
        }
    }
}

/// Validate a bearer-token `agent_name` per
/// `sprints/009-stability-attribution/contracts/token-grant-config.md`:
/// non-empty after trim, 2..=64 bytes, charset `[a-z0-9_-]`.
///
/// # Errors
/// Returns [`AgentNameInvalidReason`] describing the first failing
/// rule. Callers wrap this into [`AuthConfigError::InvalidAgentName`].
pub fn validate_agent_name(name: &str) -> Result<(), AgentNameInvalidReason> {
    if name.is_empty() {
        return Err(AgentNameInvalidReason::Empty);
    }
    let len = name.len();
    if !(2..=64).contains(&len) {
        return Err(AgentNameInvalidReason::Length);
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
    {
        return Err(AgentNameInvalidReason::Charset);
    }
    Ok(())
}

impl TokenGrantConfig {
    /// Apply per-grant validation (length + non-empty scope set,
    /// `agent_name` charset/length when present, and — sprint 034
    /// #703 — `agent_name` *required* on grants holding `manage` or
    /// `admin`, so every privileged action is attributable).
    ///
    /// # Errors
    /// Returns [`AuthConfigError::TokenTooShort`] if the token is under
    /// 16 characters, [`AuthConfigError::EmptyScopes`] if `scopes` is
    /// empty, [`AuthConfigError::InvalidAgentName`] if a non-None
    /// `agent_name` fails the rules in
    /// `sprints/009-stability-attribution/contracts/token-grant-config.md`,
    /// or [`AuthConfigError::PrivilegedGrantNeedsAgentName`] if a
    /// `manage`/`admin` grant declares none.
    pub fn validate(&self) -> Result<(), AuthConfigError> {
        if self.token.len() < 16 {
            return Err(AuthConfigError::TokenTooShort);
        }
        if self.scopes.is_empty() {
            return Err(AuthConfigError::EmptyScopes);
        }
        match &self.agent_name {
            Some(name) => {
                if let Err(reason) = validate_agent_name(name) {
                    return Err(AuthConfigError::InvalidAgentName { reason });
                }
            }
            None => {
                if self
                    .scopes
                    .iter()
                    .any(|s| matches!(s, Scope::Manage | Scope::Admin))
                {
                    return Err(AuthConfigError::PrivilegedGrantNeedsAgentName);
                }
            }
        }
        Ok(())
    }
}

/// The `[auth]` block of `klams.toml`.
///
/// Sprint 045 (#265): this lived in `klams-service::config` until
/// `klams-token` needed it. That CLI edits the very grants this struct
/// describes, and a config editor whose idea of the schema can drift
/// from the service's is the class of bug it exists to prevent — so the
/// shape moved down here, where both can share exactly one definition.
/// `klams_service::config::AuthConfig` is now a re-export of this type.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AuthConfig {
    /// RETIRED legacy single-token form (sprint 034, #703). It
    /// materialized a full-scope grant that could not declare an
    /// `agent_name`, which privileged grants now require. The field
    /// still parses — deliberately: a config that carries one refuses
    /// to start with the migration note instead of silently ignoring a
    /// credential the operator believes is live.
    #[serde(default)]
    pub bearer_token: String,

    /// Token grants (`[[auth.tokens]]`). Each entry carries its own
    /// scope set; grants holding `manage`/`admin` must declare an
    /// `agent_name` (#703).
    #[serde(default)]
    pub tokens: Vec<TokenGrantConfig>,
}

impl AuthConfig {
    /// Every reason this `[auth]` block would be refused, as operator-
    /// facing strings — *all* of them, not just the first, because an
    /// operator fixing a config wants the whole list in one pass.
    ///
    /// This is the single definition of "is this `[auth]` block
    /// startable": `klams-service --validate-config` reports it, and
    /// `klams-token` gates every write on it (sprint 045, #265). An
    /// empty vec means the block would boot.
    #[must_use]
    pub fn errors(&self) -> Vec<String> {
        let mut errors = Vec::new();
        if !self.bearer_token.is_empty() {
            errors.push(format!(
                "[auth]: {}",
                AuthConfigError::LegacyBearerTokenRetired
            ));
        }
        if self.tokens.is_empty() {
            errors.push(format!("[auth]: {}", AuthConfigError::NoTokens));
        }
        for (i, g) in self.tokens.iter().enumerate() {
            if let Err(e) = g.validate() {
                errors.push(format!(
                    "[auth.tokens[{i}]] ({label}): {e}",
                    label = g.label.as_deref().unwrap_or("<no label>")
                ));
            }
        }
        errors
    }

    /// Non-fatal observations about this `[auth]` block. A grant with
    /// no `label` still boots, but its log and metric attribution is
    /// empty, which is worth saying out loud.
    #[must_use]
    pub fn warnings(&self) -> Vec<String> {
        self.tokens
            .iter()
            .enumerate()
            .filter(|(_, g)| g.label.is_none())
            .map(|(i, _)| {
                format!("[auth.tokens[{i}]]: no `label` set; log/metric attribution will be empty")
            })
            .collect()
    }
}

/// Scope set of the caller, resolved from the presented bearer token.
///
/// Stamped onto request extensions by the REST auth middleware and read
/// back by both surfaces — REST route guards directly, MCP tools via the
/// `http::request::Parts` that rmcp copies into the tool context.
///
/// Sprint 031 (#645): this and [`AuthenticatedAuthor`] used to live in
/// `klams-api`, which made `klams-mcp` depend on the REST crate for two
/// plain data types and nothing else — an inverted dependency that also
/// meant the MCP server could not be built or tested without the REST
/// layer. They are pure data, so they belong at the bottom with
/// [`Scope`].
#[derive(Clone, Debug)]
pub struct AuthenticatedScopes(pub std::sync::Arc<Vec<Scope>>);

/// Author bound to the request's bearer token (sprint 009). Write paths
/// stamp `author_id` from this when the caller omits one.
#[derive(Clone, Debug)]
pub struct AuthenticatedAuthor {
    pub author_id: uuid::Uuid,
    pub agent_name: std::sync::Arc<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_satisfies_is_exact() {
        assert!(Scope::Read.satisfies(Scope::Read));
        assert!(!Scope::Write.satisfies(Scope::Read));
        assert!(!Scope::Admin.satisfies(Scope::Write));
    }

    /// Sprint 025 (#633) — `manage` is a peer of the other three, not a
    /// super-scope. Granting `admin` must NOT confer cross-author
    /// curation, and holding `manage` must not confer hard-delete.
    #[test]
    fn manage_scope_is_flat_like_the_others() {
        assert!(Scope::Manage.satisfies(Scope::Manage));
        assert!(!Scope::Admin.satisfies(Scope::Manage));
        assert!(!Scope::Manage.satisfies(Scope::Admin));
        assert!(!Scope::Write.satisfies(Scope::Manage));
        assert!(!Scope::Manage.satisfies(Scope::Write));
    }

    #[test]
    fn manage_scope_round_trips_through_toml_lowercase() {
        let grant: TokenGrantConfig = toml::from_str(
            r#"
            token      = "abcdefghijklmnop"
            scopes     = ["read", "write", "manage"]
            label      = "claude"
            agent_name = "claude"
            "#,
        )
        .expect("manage must parse as a scope in [[auth.tokens]]");
        assert_eq!(grant.scopes, vec![Scope::Read, Scope::Write, Scope::Manage]);
        grant.validate().unwrap();
    }

    /// Sprint 034 (#703): a grant holding `manage` or `admin` without
    /// an `agent_name` is a config error — privileged actions must be
    /// attributable (the property sprint 025 built and #670 Q4 asked
    /// to close).
    #[test]
    fn privileged_grant_without_agent_name_is_rejected() {
        for privileged in [Scope::Manage, Scope::Admin] {
            let g = TokenGrantConfig {
                token: "abcdefghijklmnop".into(),
                scopes: vec![Scope::Read, Scope::Write, privileged],
                label: Some("unattributed".into()),
                agent_name: None,
            };
            assert!(
                matches!(
                    g.validate(),
                    Err(AuthConfigError::PrivilegedGrantNeedsAgentName)
                ),
                "{privileged} without agent_name must be rejected"
            );
        }
    }

    /// The counterpart boundaries: read/write-only grants stay valid
    /// without an `agent_name` (back-compat for tokens issued before
    /// sprint 009), and a privileged grant WITH one is accepted.
    #[test]
    fn privileged_grant_rule_boundaries() {
        let unprivileged = TokenGrantConfig {
            token: "abcdefghijklmnop".into(),
            scopes: vec![Scope::Read, Scope::Write],
            label: None,
            agent_name: None,
        };
        unprivileged.validate().unwrap();

        let attributed = TokenGrantConfig {
            token: "abcdefghijklmnop".into(),
            scopes: vec![Scope::Read, Scope::Write, Scope::Manage, Scope::Admin],
            label: Some("ken-admin".into()),
            agent_name: Some("ken_admin".into()),
        };
        attributed.validate().unwrap();
    }

    #[test]
    fn token_grant_validates_length() {
        let g = TokenGrantConfig {
            token: "short".into(),
            scopes: vec![Scope::Read],
            label: None,
            agent_name: None,
        };
        assert!(matches!(g.validate(), Err(AuthConfigError::TokenTooShort)));
    }

    #[test]
    fn token_grant_requires_scopes() {
        let g = TokenGrantConfig {
            token: "abcdefghijklmnop".into(),
            scopes: vec![],
            label: None,
            agent_name: None,
        };
        assert!(matches!(g.validate(), Err(AuthConfigError::EmptyScopes)));
    }

    #[test]
    fn token_grant_accepts_valid() {
        let g = TokenGrantConfig {
            token: "abcdefghijklmnop".into(),
            scopes: vec![Scope::Read, Scope::Write],
            label: Some("ghcp".into()),
            agent_name: Some("alice".into()),
        };
        g.validate().unwrap();
    }

    #[test]
    fn agent_name_accepts_valid_shapes() {
        for ok in [
            "alice",
            "klams-bench",
            "agent_42",
            "ab",
            "a-b-c-d",
            "ansible-k",
        ] {
            assert!(validate_agent_name(ok).is_ok(), "expected {ok} to validate");
        }
    }

    #[test]
    fn agent_name_rejects_empty() {
        assert_eq!(validate_agent_name(""), Err(AgentNameInvalidReason::Empty));
    }

    #[test]
    fn agent_name_rejects_charset() {
        for bad in ["Alice", "alice!", "alice space", "Aa"] {
            assert_eq!(
                validate_agent_name(bad),
                Err(AgentNameInvalidReason::Charset),
                "expected {bad} to be rejected for charset"
            );
        }
    }

    #[test]
    fn agent_name_rejects_length() {
        assert_eq!(
            validate_agent_name("a"),
            Err(AgentNameInvalidReason::Length)
        );
        let long = "a".repeat(65);
        assert_eq!(
            validate_agent_name(&long),
            Err(AgentNameInvalidReason::Length)
        );
    }

    #[test]
    fn token_grant_rejects_invalid_agent_name() {
        let g = TokenGrantConfig {
            token: "abcdefghijklmnop".into(),
            scopes: vec![Scope::Read],
            label: None,
            agent_name: Some("Alice".into()),
        };
        let err = g.validate().unwrap_err();
        match err {
            AuthConfigError::InvalidAgentName { reason } => {
                assert_eq!(reason, AgentNameInvalidReason::Charset);
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }
}
