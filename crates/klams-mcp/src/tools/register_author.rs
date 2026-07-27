//! `register_author` MCP tool (sprint 007 T032).

use crate::errors::{envelope, ErrorEnvelope};
use crate::tools::McpState;
use klams_store::Store;
use klams_types::{AuthorRecord, RegisterAuthorArgs, RegisterAuthorError};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// MCP-facing input shape (mirrors `klams_types::RegisterAuthorArgs`
/// but carries a `JsonSchema` derive for the tool descriptor).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RegisterAuthorInput {
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

impl From<RegisterAuthorInput> for RegisterAuthorArgs {
    fn from(i: RegisterAuthorInput) -> Self {
        Self {
            agent_name: i.agent_name,
            model: i.model,
            session_title: i.session_title,
            repo: i.repo,
            client_app: i.client_app,
            client_version: i.client_version,
            extra: i.extra,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct RegisterAuthorOutput {
    pub author_id: Uuid,
    pub agent_name: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl From<AuthorRecord> for RegisterAuthorOutput {
    fn from(a: AuthorRecord) -> Self {
        Self {
            author_id: a.id,
            agent_name: a.agent_name,
            created_at: a.created_at,
        }
    }
}

/// Execute `register_author`.
///
/// # Errors
/// Returns an [`ErrorEnvelope`] for `INVALID_AGENT_NAME`,
/// `INVALID_REPO_PATH`, `EXTRA_TOO_LARGE`, or `INTERNAL_ERROR`.
pub async fn run<S: Store>(
    state: &McpState<S>,
    input: RegisterAuthorInput,
) -> Result<RegisterAuthorOutput, ErrorEnvelope> {
    let args: RegisterAuthorArgs = input.into();
    if let Err(e) = args.validate() {
        return Err(map_validation_error(&e));
    }
    // Sprint 025 (#636): dedupe on `agent_name`. Every call used to mint
    // a fresh UUIDv7 row — `insert_author` upserts on `id` conflict, and
    // there is no uniqueness on `agent_name` — so a store with 8
    // `klams-mind` and 6 `kyac` rows was the steady state, and an
    // `agent_name` in a response mapped to no single identity. Returning
    // the existing row makes registration idempotent per name.
    let existing = state
        .store
        .get_author_by_agent_name(&args.agent_name)
        .await
        .map_err(|e| {
            envelope(
                crate::errors::INTERNAL_ERROR,
                format!("get_author_by_agent_name: {e}"),
            )
        })?;
    if let Some(record) = existing {
        let _ = state.store.touch_author_last_seen_at(record.id).await;
        return Ok(record.into());
    }
    match state.store.insert_author(args, None).await {
        Ok(record) => Ok(record.into()),
        Err(e) => Err(envelope(
            crate::errors::INTERNAL_ERROR,
            format!("register_author failed: {e}"),
        )),
    }
}

fn map_validation_error(e: &RegisterAuthorError) -> ErrorEnvelope {
    use crate::errors as ec;
    match e {
        RegisterAuthorError::EmptyAgentName
        | RegisterAuthorError::AgentNameTooLong
        | RegisterAuthorError::AgentNameInvalid { .. } => {
            envelope(ec::INVALID_AGENT_NAME, e.to_string())
        }
        RegisterAuthorError::RepoEmpty => envelope(ec::INVALID_REPO_PATH, e.to_string()),
        RegisterAuthorError::ExtraTooLarge => envelope(ec::EXTRA_TOO_LARGE, e.to_string()),
    }
}
