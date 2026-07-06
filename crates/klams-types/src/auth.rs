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
    Admin,
}

impl Scope {
    /// Returns true if a token holding `self` satisfies a route that
    /// requires `needed`. Scopes are independent (not hierarchical) — a
    /// "write" token does not automatically grant "read" unless the
    /// configured grant explicitly lists both.
    #[must_use]
    pub fn satisfies(self, needed: Scope) -> bool {
        self == needed
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
    #[error("auth: at least one of `bearer_token` or `tokens` must be set")]
    NoTokens,
    #[error("auth: token must be at least 16 characters")]
    TokenTooShort,
    #[error("auth: token grant must declare at least one scope")]
    EmptyScopes,
    #[error("auth: token grant `agent_name` is invalid ({reason})")]
    InvalidAgentName { reason: AgentNameInvalidReason },
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
    /// plus optional `agent_name` charset/length).
    ///
    /// # Errors
    /// Returns [`AuthConfigError::TokenTooShort`] if the token is under
    /// 16 characters, [`AuthConfigError::EmptyScopes`] if `scopes` is
    /// empty, or [`AuthConfigError::InvalidAgentName`] if a non-None
    /// `agent_name` fails the rules in
    /// `sprints/009-stability-attribution/contracts/token-grant-config.md`.
    pub fn validate(&self) -> Result<(), AuthConfigError> {
        if self.token.len() < 16 {
            return Err(AuthConfigError::TokenTooShort);
        }
        if self.scopes.is_empty() {
            return Err(AuthConfigError::EmptyScopes);
        }
        if let Some(name) = &self.agent_name {
            if let Err(reason) = validate_agent_name(name) {
                return Err(AuthConfigError::InvalidAgentName { reason });
            }
        }
        Ok(())
    }
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
