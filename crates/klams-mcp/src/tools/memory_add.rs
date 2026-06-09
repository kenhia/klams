//! `memory_add` MCP tool (sprint 007 T033/T034).
//!
//! Discriminated by `kind`:
//!   * `fact` — Postgres `upsert_fact` (dedupe on
//!     `(type, payload_hash)`).
//!   * `knowledge` — TEI embedding then Qdrant `index_knowledge`,
//!     which stamps `author_id` into the point payload atomically.
//!
//! Maintenance window short-circuits the entire path with
//! `MAINTENANCE_WINDOW_ACTIVE` + `retry_after_seconds` (R-007).

use crate::{
    errors::{self, envelope, ErrorEnvelope},
    maintenance,
    metrics::{self as mcp_metrics},
    projection,
    tools::McpState,
};
use klams_types::{FactType, IndexKnowledge, PublicAuthorRef, PublicMemory, Source, UpsertFact};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MemoryAddContent {
    Fact {
        fact_type: FactTypeArg,
        payload: serde_json::Value,
    },
    Knowledge {
        text: String,
        #[serde(default)]
        tags: Vec<String>,
        #[serde(default)]
        source_path: Option<String>,
        #[serde(default)]
        repo: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "PascalCase")]
#[allow(clippy::enum_variant_names)]
pub enum FactTypeArg {
    UserFact,
    TaskFact,
    EnvFact,
}

impl From<FactTypeArg> for FactType {
    fn from(v: FactTypeArg) -> Self {
        match v {
            FactTypeArg::UserFact => FactType::UserFact,
            FactTypeArg::TaskFact => FactType::TaskFact,
            FactTypeArg::EnvFact => FactType::EnvFact,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryAddArgs {
    #[schemars(with = "String")]
    pub author_id: Uuid,
    #[serde(flatten)]
    pub content: MemoryAddContent,
}

/// Execute `memory_add`. Returns the persisted memory in public
/// projection form on success or an MCP error envelope otherwise.
///
/// # Errors
/// Returns an [`ErrorEnvelope`] for `MAINTENANCE_WINDOW_ACTIVE`,
/// `MISSING_AUTHOR_ID`, `UNKNOWN_AUTHOR_ID`, `EMBEDDING_UNAVAILABLE`,
/// `SCHEMA_VALIDATION_FAILED`, or `INTERNAL_ERROR`.
#[allow(clippy::too_many_lines)]
pub async fn run(state: &McpState, args: MemoryAddArgs) -> Result<PublicMemory, ErrorEnvelope> {
    if let Some(env) = maintenance::check(&state.maintenance) {
        return Err(env);
    }
    if args.author_id.is_nil() {
        return Err(envelope(errors::MISSING_AUTHOR_ID, "author_id is required"));
    }
    let author = state
        .store
        .postgres
        .get_author_by_id(args.author_id)
        .await
        .map_err(|e| envelope(errors::INTERNAL_ERROR, format!("get_author_by_id: {e}")))?
        .ok_or_else(|| {
            envelope(
                errors::UNKNOWN_AUTHOR_ID,
                format!("author_id {} not found", args.author_id),
            )
        })?;

    // FR-005: touch last_seen_at on every authenticated reference.
    let _ = state
        .store
        .postgres
        .touch_author_last_seen_at(author.id)
        .await;

    let author_ref = PublicAuthorRef {
        agent_name: author.agent_name.clone(),
        model: author.model.clone(),
        repo: author.repo.clone(),
    };

    match args.content {
        MemoryAddContent::Fact { fact_type, payload } => {
            let req = UpsertFact {
                fact_type: fact_type.into(),
                payload,
                source: Source::AgentProposal,
                explicit_id: None,
                expected_version: None,
                author_id: author.id,
            };
            let fact = state
                .store
                .postgres
                .upsert_fact(req)
                .await
                .map_err(|e| envelope(errors::INTERNAL_ERROR, format!("upsert_fact: {e}")))?;
            mcp_metrics::record_write(
                &author.agent_name,
                author.model.as_deref(),
                mcp_metrics::KIND_FACT,
            );
            Ok(projection::project_fact(&fact, author_ref))
        }
        MemoryAddContent::Knowledge {
            text,
            tags,
            source_path,
            repo,
        } => {
            if text.trim().is_empty() {
                return Err(envelope(
                    errors::SCHEMA_VALIDATION_FAILED,
                    "knowledge text must be non-empty",
                ));
            }
            let hash = sha256_hex(&text);
            let req = IndexKnowledge {
                id: Uuid::now_v7(),
                text,
                content_hash: hash,
                source: Source::AgentProposal,
                tags,
                repo,
                file: source_path,
                machine: None,
                author_id: author.id,
            };
            let embedding = state.store.embedder.embed(&req.text).await.map_err(|e| {
                crate::errors::envelope_with_retry(
                    errors::EMBEDDING_UNAVAILABLE,
                    format!("TEI embedding failed: {e}"),
                    5,
                )
            })?;
            let item = state
                .store
                .qdrant
                .index_knowledge(req, embedding)
                .await
                .map_err(|e| envelope(errors::INTERNAL_ERROR, format!("qdrant: {e}")))?;
            mcp_metrics::record_write(
                &author.agent_name,
                author.model.as_deref(),
                mcp_metrics::KIND_KNOWLEDGE,
            );
            Ok(projection::project_knowledge(&item, author_ref))
        }
    }
}

fn sha256_hex(s: &str) -> String {
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    let out = h.finalize();
    let mut buf = String::with_capacity(out.len() * 2);
    for b in out {
        use std::fmt::Write;
        let _ = write!(&mut buf, "{b:02x}");
    }
    buf
}
