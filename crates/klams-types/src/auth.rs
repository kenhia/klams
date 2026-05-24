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
}

impl TokenGrantConfig {
    /// Apply per-grant validation (length + non-empty scope set).
    ///
    /// # Errors
    /// Returns [`AuthConfigError::TokenTooShort`] if the token is under
    /// 16 characters, or [`AuthConfigError::EmptyScopes`] if `scopes` is
    /// empty.
    pub fn validate(&self) -> Result<(), AuthConfigError> {
        if self.token.len() < 16 {
            return Err(AuthConfigError::TokenTooShort);
        }
        if self.scopes.is_empty() {
            return Err(AuthConfigError::EmptyScopes);
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
        };
        assert!(matches!(g.validate(), Err(AuthConfigError::TokenTooShort)));
    }

    #[test]
    fn token_grant_requires_scopes() {
        let g = TokenGrantConfig {
            token: "abcdefghijklmnop".into(),
            scopes: vec![],
            label: None,
        };
        assert!(matches!(g.validate(), Err(AuthConfigError::EmptyScopes)));
    }

    #[test]
    fn token_grant_accepts_valid() {
        let g = TokenGrantConfig {
            token: "abcdefghijklmnop".into(),
            scopes: vec![Scope::Read, Scope::Write],
            label: Some("ghcp".into()),
        };
        g.validate().unwrap();
    }
}
