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
    #[error("repo must be a non-empty absolute path or repo name")]
    RepoEmpty,
    #[error("extra must serialize to at most 16 KiB")]
    ExtraTooLarge,
}

const EXTRA_MAX_BYTES: usize = 16 * 1024;

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
            agent_name: "GHCP".into(),
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
