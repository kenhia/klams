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

/// Internal, validated form of the `memory_add` arguments. Not part of
/// the wire schema — [`MemoryAddArgs::content`] produces it after
/// enforcing the per-kind field requirements.
#[derive(Debug, Clone)]
pub enum MemoryAddContent {
    Fact {
        fact_type: FactTypeArg,
        payload: serde_json::Value,
    },
    Knowledge {
        text: String,
        tags: Vec<String>,
        source_path: Option<String>,
        repo: Option<String>,
    },
}

/// Memory kind discriminator for the flat `memory_add` schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MemoryAddKind {
    Fact,
    Knowledge,
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

/// Wire arguments for `memory_add` (sprint 018, WI #307).
///
/// Deliberately a FLAT object schema: the previous serde-tagged enum
/// (`#[serde(flatten)] MemoryAddContent`) generated a top-level
/// `oneOf`, which the Anthropic API rejects for tool `input_schema`s
/// (400), forcing Anthropic-bound agents to drop this tool. The JSON
/// wire shape is unchanged — `kind` plus the same per-kind fields —
/// only the advertised schema and where the per-kind requirements are
/// enforced (now [`MemoryAddArgs::content`]) differ.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryAddArgs {
    /// Optional: defaults to the author bound to the caller's bearer
    /// token (WI #62). Pass explicitly to write as a different
    /// registered author.
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub author_id: Uuid,
    /// Discriminator: decides which of the remaining fields apply.
    pub kind: MemoryAddKind,
    /// Required when `kind` is `fact`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fact_type: Option<FactTypeArg>,
    /// Required when `kind` is `fact`: arbitrary JSON payload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
    /// Required when `kind` is `knowledge`: the text to embed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Optional, `knowledge` only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Optional, `knowledge` only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
    /// Optional, `knowledge` only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
}

impl MemoryAddArgs {
    /// Construct fact-kind args.
    #[must_use]
    pub fn fact(author_id: Uuid, fact_type: FactTypeArg, payload: serde_json::Value) -> Self {
        Self {
            author_id,
            kind: MemoryAddKind::Fact,
            fact_type: Some(fact_type),
            payload: Some(payload),
            text: None,
            tags: Vec::new(),
            source_path: None,
            repo: None,
        }
    }

    /// Construct knowledge-kind args with no tags/source/repo; set
    /// those fields directly (or via struct update) when needed.
    #[must_use]
    pub fn knowledge(author_id: Uuid, text: impl Into<String>) -> Self {
        Self {
            author_id,
            kind: MemoryAddKind::Knowledge,
            fact_type: None,
            payload: None,
            text: Some(text.into()),
            tags: Vec::new(),
            source_path: None,
            repo: None,
        }
    }

    /// Enforce the per-kind field requirements the schema can no
    /// longer express and produce the validated content.
    ///
    /// # Errors
    /// Returns a `SCHEMA_VALIDATION_FAILED` envelope naming the
    /// missing field(s) for the given `kind`. Fields belonging to the
    /// other kind are ignored, matching the pre-018 tagged-enum
    /// behaviour.
    pub fn content(self) -> Result<MemoryAddContent, ErrorEnvelope> {
        match self.kind {
            MemoryAddKind::Fact => match (self.fact_type, self.payload) {
                (Some(fact_type), Some(payload)) => {
                    Ok(MemoryAddContent::Fact { fact_type, payload })
                }
                (fact_type, payload) => {
                    let mut missing = Vec::new();
                    if fact_type.is_none() {
                        missing.push("fact_type");
                    }
                    if payload.is_none() {
                        missing.push("payload");
                    }
                    Err(envelope(
                        errors::SCHEMA_VALIDATION_FAILED,
                        format!("kind \"fact\" requires {}", missing.join(" and ")),
                    ))
                }
            },
            MemoryAddKind::Knowledge => match self.text {
                Some(text) => Ok(MemoryAddContent::Knowledge {
                    text,
                    tags: self.tags,
                    source_path: self.source_path,
                    repo: self.repo,
                }),
                None => Err(envelope(
                    errors::SCHEMA_VALIDATION_FAILED,
                    "kind \"knowledge\" requires text",
                )),
            },
        }
    }
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
    // Per-kind requirements checked up front so a malformed request
    // fails on schema before author resolution, as it did when the
    // tagged-enum deserializer did this job (pre-018).
    let author_id = args.author_id;
    let content = args.content()?;
    let author = state
        .store
        .postgres
        .get_author_by_id(author_id)
        .await
        .map_err(|e| envelope(errors::INTERNAL_ERROR, format!("get_author_by_id: {e}")))?
        .ok_or_else(|| {
            envelope(
                errors::UNKNOWN_AUTHOR_ID,
                format!("author_id {author_id} not found"),
            )
        })?;

    // FR-005: touch last_seen_at on every authenticated reference.
    let _ = state
        .store
        .postgres
        .touch_author_last_seen_at(author.id)
        .await;

    let author_ref = PublicAuthorRef::from_record(&author);

    match content {
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
                // Agent-submitted knowledge isn't chunked by the scanner,
                // so it carries no chunk-structure metadata (sprint 022 #322).
                chunk_index: None,
                language: None,
                heading_path: None,
                symbols: Vec::new(),
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
