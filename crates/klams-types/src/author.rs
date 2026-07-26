//! Author registry types (sprint 007 — MCP memory server).
//!
//! `AuthorRecord` mirrors the `authors` Postgres table. `RegisterAuthorArgs`
//! is the validated input for the `register_author` MCP tool; validation
//! enforces the constraints from `sprints/007-mcp-server/data-model.md` §1.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Row in the `authors` table.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuthorRecord {
    pub id: Uuid,
    pub agent_name: String,
    pub model: Option<String>,
    pub session_title: Option<String>,
    pub repo: Option<String>,
    pub client_app: Option<String>,
    pub client_version: Option<String>,
    #[serde(default)]
    pub extra: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
}

/// Input for the `register_author` MCP tool.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RegisterAuthorArgs {
    pub agent_name: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub session_title: Option<String>,
    #[serde(default)]
    pub repo: Option<String>,
    #[serde(default)]
    pub client_app: Option<String>,
    #[serde(default)]
    pub client_version: Option<String>,
    #[serde(default)]
    pub extra: serde_json::Value,
}

/// Compact reference to an author included in every public memory projection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PublicAuthorRef {
    pub agent_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
}

/// Validation errors for `RegisterAuthorArgs` (matches `data-model.md` §1).
#[derive(Debug, thiserror::Error)]
pub enum RegisterAuthorError {
    #[error("agent_name must be non-empty")]
    EmptyAgentName,
    #[error("agent_name exceeds 128 characters")]
    AgentNameTooLong,
    /// Sprint 025 (#636) — `register_author` accepted any charset up to
    /// 128 chars while `[[auth.tokens]]` requires 2–64 bytes of
    /// `[a-z0-9_-]`. A name like "Claude Code" registered fine and could
    /// then never be used as a token binding, which is how the store
    /// accumulated identities no grant could ever refer to. The rules
    /// are now the same; the error carries a usable suggestion.
    #[error("agent_name is invalid ({reason}): must be 2-64 characters of [a-z0-9_-] to match the `agent_name` rule for [[auth.tokens]] grants — try `{suggestion}`")]
    AgentNameInvalid {
        reason: crate::AgentNameInvalidReason,
        suggestion: String,
    },
    #[error("repo must be a non-empty absolute path or repo name")]
    RepoEmpty,
    #[error("extra must serialize to at most 16 KiB")]
    ExtraTooLarge,
}

const EXTRA_MAX_BYTES: usize = 16 * 1024;

/// Best-effort rewrite of a rejected `agent_name` into a conforming one,
/// used only to make the validation error actionable — nothing is
/// renamed implicitly. "GitHub Copilot" -> "github-copilot".
fn suggest_agent_name(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.trim().chars() {
        match ch {
            c if c.is_ascii_alphanumeric() => out.push(c.to_ascii_lowercase()),
            '_' | '-' => out.push(ch),
            _ if out.ends_with('-') => {}
            _ => out.push('-'),
        }
    }
    let trimmed = out.trim_matches('-');
    let mut s: String = trimmed.chars().take(64).collect();
    if s.len() < 2 {
        s = "agent".to_string();
    }
    s
}

impl RegisterAuthorArgs {
    /// Apply field-level validation. Returns the args unchanged on success.
    ///
    /// # Errors
    /// Returns a [`RegisterAuthorError`] for any constraint violation listed
    /// in `data-model.md` §1.
    pub fn validate(&self) -> Result<(), RegisterAuthorError> {
        if self.agent_name.trim().is_empty() {
            return Err(RegisterAuthorError::EmptyAgentName);
        }
        if self.agent_name.len() > 128 {
            return Err(RegisterAuthorError::AgentNameTooLong);
        }
        // Sprint 025 (#636): the same rule token grants use, so every
        // registered identity is one a `[[auth.tokens]]` grant could
        // actually bind to.
        if let Err(reason) = crate::validate_agent_name(&self.agent_name) {
            return Err(RegisterAuthorError::AgentNameInvalid {
                reason,
                suggestion: suggest_agent_name(&self.agent_name),
            });
        }
        // Sprint 018 (WI #62): a bare repo name ("krag") is as valid
        // as an absolute path — agents often don't know their absolute
        // working directory. Only reject blank values.
        if let Some(repo) = &self.repo {
            if repo.trim().is_empty() {
                return Err(RegisterAuthorError::RepoEmpty);
            }
        }
        if !self.extra.is_null() {
            let bytes =
                serde_json::to_vec(&self.extra).map_err(|_| RegisterAuthorError::ExtraTooLarge)?;
            if bytes.len() > EXTRA_MAX_BYTES {
                return Err(RegisterAuthorError::ExtraTooLarge);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args() -> RegisterAuthorArgs {
        RegisterAuthorArgs {
            agent_name: "ghcp".into(),
            model: None,
            session_title: None,
            repo: None,
            client_app: None,
            client_version: None,
            extra: serde_json::Value::Null,
        }
    }

    #[test]
    fn accepts_minimal() {
        args().validate().unwrap();
    }

    #[test]
    fn rejects_empty_agent_name() {
        let mut a = args();
        a.agent_name = "   ".into();
        assert!(matches!(
            a.validate(),
            Err(RegisterAuthorError::EmptyAgentName)
        ));
    }

    #[test]
    fn accepts_absolute_repo_path() {
        let mut a = args();
        a.repo = Some("/home/ken/src/ai/krag".into());
        a.validate().unwrap();
    }

    #[test]
    fn accepts_bare_repo_name() {
        // Sprint 018 (WI #62): agents don't always know their absolute
        // working directory; a short name is enough.
        let mut a = args();
        a.repo = Some("krag".into());
        a.validate().unwrap();
    }

    #[test]
    fn rejects_blank_repo() {
        let mut a = args();
        a.repo = Some("   ".into());
        assert!(matches!(a.validate(), Err(RegisterAuthorError::RepoEmpty)));
    }

    /// Sprint 025 (#636) — the names that produced unbindable author
    /// rows before the rules were aligned.
    #[test]
    fn rejects_agent_names_no_token_grant_could_bind() {
        for bad in ["GitHub Copilot", "Claude Code", "Alice", "mv cli", "a"] {
            let mut a = args();
            a.agent_name = bad.into();
            assert!(
                matches!(
                    a.validate(),
                    Err(RegisterAuthorError::AgentNameInvalid { .. })
                ),
                "expected {bad} to be rejected"
            );
        }
    }

    #[test]
    fn rejection_suggests_a_usable_name() {
        let mut a = args();
        a.agent_name = "GitHub Copilot".into();
        let Err(RegisterAuthorError::AgentNameInvalid { suggestion, .. }) = a.validate() else {
            panic!("expected AgentNameInvalid");
        };
        assert_eq!(suggestion, "github-copilot");
        // The suggestion must itself pass, or it isn't actionable.
        crate::validate_agent_name(&suggestion).expect("suggestion must be valid");
    }

    #[test]
    fn suggestion_is_always_valid_even_for_hostile_input() {
        for raw in ["!!!", "  ", "A", "@@@ !!! @@@", &"Ω".repeat(200)] {
            let s = suggest_agent_name(raw);
            crate::validate_agent_name(&s)
                .unwrap_or_else(|e| panic!("suggestion {s:?} for {raw:?} invalid: {e:?}"));
        }
    }

    #[test]
    fn rejects_oversize_extra() {
        let mut a = args();
        let big = "x".repeat(EXTRA_MAX_BYTES + 1);
        a.extra = serde_json::json!({ "blob": big });
        assert!(matches!(
            a.validate(),
            Err(RegisterAuthorError::ExtraTooLarge)
        ));
    }
}
