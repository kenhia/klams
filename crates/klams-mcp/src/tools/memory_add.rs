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
use klams_store::Store as _;
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
        volatility: Option<VolatilityArg>,
    },
}

/// Declared volatility for knowledge writes (sprint 029, #638 /
/// review F-1.4). `volatile` memories are age-demoted in ranking after
/// a grace week; `stable` and undeclared memories never decay.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum VolatilityArg {
    Stable,
    Volatile,
}

impl VolatilityArg {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Volatile => "volatile",
        }
    }
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
    ///
    /// Bounded by the embedding model's input length. Oversized text is
    /// refused with `PAYLOAD_TOO_LARGE`, whose message names both the
    /// limit and what you submitted — split at that size and retry. The
    /// error is permanent for that text: it will never carry
    /// `retry_after_seconds`, and retrying it unchanged always fails.
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
    /// Optional, `knowledge` only: declare how this memory ages.
    /// `volatile` = expected to go stale (IPs, versions, "not yet on
    /// X"); ranked down as it ages. `stable` (or omitted) = never
    /// decays.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volatility: Option<VolatilityArg>,
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
            volatility: None,
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
            volatility: None,
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
                    volatility: self.volatility,
                }),
                None => Err(envelope(
                    errors::SCHEMA_VALIDATION_FAILED,
                    "kind \"knowledge\" requires text",
                )),
            },
        }
    }
}

/// Similarity floor for the similar-on-write nudge (sprint 029,
/// #638 / review F-1.3). Only near-duplicates should trigger it — a
/// topically-related memory is not a reason to supersede. Under
/// Qwen3-Embedding-0.6B genuine hits run ~0.55-0.71 (sprint 028), so
/// 0.85 sits well above "related" and flags only "you are about to
/// write this again". Re-calibrate if the model changes.
const SIMILAR_ON_WRITE_THRESHOLD: f32 = 0.85;

/// How many near-duplicates to name in the nudge.
const SIMILAR_ON_WRITE_LIMIT: usize = 5;

/// Chars of the existing memory quoted back in the nudge.
const SIMILAR_TEXT_HEAD: usize = 160;

/// A near-duplicate agent memory found at write time (sprint 029,
/// #638): returned in [`MemoryAddOutput::similar_existing`] so the
/// writer can supersede instead of duplicating — the only moment that
/// check is cheap.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimilarExisting {
    pub id: Uuid,
    /// First chars of the existing memory's text.
    pub text_head: String,
    /// Agent name of the existing memory's author.
    pub author: String,
    /// Cosine similarity to the text being written.
    pub raw_score: f32,
}

/// `memory_add` response: the persisted memory (flattened, so the wire
/// shape is unchanged from pre-029) plus the similar-on-write nudge.
#[derive(Debug, Clone, Serialize)]
pub struct MemoryAddOutput {
    #[serde(flatten)]
    pub memory: PublicMemory,
    /// Agent-authored memories whose similarity to the new text is
    /// near-duplicate level. Non-blocking and purely informational: if
    /// one of these says what you were about to write, call
    /// `memory_supersede` on it instead of adding a twin. Omitted when
    /// empty.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub similar_existing: Vec<SimilarExisting>,
}

/// The output *is* the persisted memory (its wire shape flattens to
/// one), so expose the memory's fields directly — existing callers and
/// tests read `.id`/`.content`/`.author` off the add result.
impl std::ops::Deref for MemoryAddOutput {
    type Target = PublicMemory;
    fn deref(&self) -> &PublicMemory {
        &self.memory
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
pub async fn run(state: &McpState, args: MemoryAddArgs) -> Result<MemoryAddOutput, ErrorEnvelope> {
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
            Ok(MemoryAddOutput {
                memory: projection::project_fact(&fact, author_ref),
                similar_existing: Vec::new(),
            })
        }
        MemoryAddContent::Knowledge {
            text,
            tags,
            source_path,
            repo,
            volatility,
        } => {
            if text.trim().is_empty() {
                return Err(envelope(
                    errors::SCHEMA_VALIDATION_FAILED,
                    "knowledge text must be non-empty",
                ));
            }
            // Sprint 027 (#420/#629): this path had NO length check at
            // all — `memory_search` in the same crate enforced
            // MAX_QUERY_LEN, but a write of any size went straight to
            // the embedder and came back as a transient-looking error.
            // Fail here instead, with numbers the caller can act on.
            enforce_embed_size(state, author.id, &author.agent_name, &text).await?;
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
                volatility: volatility.map(|v| v.as_str().to_string()),
                supersedes: None,
            };
            // Sprint 027 (#629): classify instead of assuming. The old
            // code attached `EMBEDDING_UNAVAILABLE` + retry_after to
            // every failure including permanent ones — the misdirection
            // that made agents give up and lose the write.
            let embedding = state
                .store
                .embedder
                .embed(&req.text)
                .await
                .map_err(|e| errors::from_store_error("embedding", &e))?;
            // Sprint 029 (#638): similar-on-write. The embedding is
            // already in hand, and the curated stratum is tiny, so the
            // near-duplicate check costs one filtered ANN call. It runs
            // BEFORE the upsert so the new point can never match
            // itself. Best-effort: the nudge is informational, and a
            // failed lookup must not fail a valid write.
            let similar_existing = match state
                .store
                .qdrant
                .search_knowledge_curated(embedding.clone(), similar_fetch_k())
                .await
            {
                Ok(hits) => {
                    let close: Vec<(klams_types::KnowledgeItem, f32)> = hits
                        .into_iter()
                        .filter(|(_, s)| *s >= SIMILAR_ON_WRITE_THRESHOLD)
                        .take(SIMILAR_ON_WRITE_LIMIT)
                        .collect();
                    similar_refs(state, close).await
                }
                Err(e) => {
                    tracing::warn!(%e, "similar-on-write lookup failed (nudge is best-effort)");
                    Vec::new()
                }
            };
            let item = state
                .store
                .qdrant
                .index_knowledge(req, embedding)
                .await
                .map_err(|e| errors::from_store_error("qdrant", &e))?;
            mcp_metrics::record_write(
                &author.agent_name,
                author.model.as_deref(),
                mcp_metrics::KIND_KNOWLEDGE,
            );
            Ok(MemoryAddOutput {
                memory: projection::project_knowledge(&item, author_ref),
                similar_existing,
            })
        }
    }
}

/// Fetch size for the similar-on-write stratum search: the nudge limit
/// plus headroom for below-threshold hits.
fn similar_fetch_k() -> u32 {
    u32::try_from(SIMILAR_ON_WRITE_LIMIT * 2).unwrap_or(u32::MAX)
}

/// Resolve author names for the similar-on-write hits and project them
/// into the nudge shape.
async fn similar_refs(
    state: &McpState,
    close: Vec<(klams_types::KnowledgeItem, f32)>,
) -> Vec<SimilarExisting> {
    if close.is_empty() {
        return Vec::new();
    }
    let ids: Vec<Uuid> = close.iter().map(|(it, _)| it.id).collect();
    let author_map = state
        .store
        .qdrant
        .knowledge_authors_by_ids(&ids)
        .await
        .unwrap_or_default();
    let mut names: std::collections::HashMap<Uuid, String> = std::collections::HashMap::new();
    for author_id in author_map.values() {
        if names.contains_key(author_id) {
            continue;
        }
        if let Ok(Some(a)) = state.store.postgres.get_author_by_id(*author_id).await {
            names.insert(*author_id, a.agent_name);
        }
    }
    close
        .into_iter()
        .map(|(it, score)| SimilarExisting {
            id: it.id,
            text_head: it.text.chars().take(SIMILAR_TEXT_HEAD).collect(),
            author: author_map
                .get(&it.id)
                .and_then(|aid| names.get(aid))
                .cloned()
                .unwrap_or_else(|| "unknown".to_string()),
            raw_score: score,
        })
        .collect()
}

/// Enforce the embedder's token ceiling on a knowledge-text write,
/// keeping the refused payload in the oversize log (sprint 027, #656:
/// without that row, the coherent original — and the fact that it ever
/// existed — is gone). Logging is best-effort: a logging failure must
/// not change the error the caller gets. Shared by `memory_add` and the
/// sprint-029 lifecycle verbs (`memory_supersede` / `memory_update`),
/// which write new text under the same ceiling.
pub(crate) async fn enforce_embed_size(
    state: &McpState,
    author_id: Uuid,
    agent_name: &str,
    text: &str,
) -> Result<(), ErrorEnvelope> {
    let oversize = match state.store.check_embed_size(text, state.embed_limit).await {
        Ok(()) => None,
        Err(klams_store::StoreError::PayloadTooLarge { oversize, .. }) => Some(oversize),
        // The tokenizer was unreachable. Fall through and let the
        // embed attempt produce the real error rather than
        // inventing one — but say so, because a size check that
        // quietly stopped working is how this class of bug hides.
        Err(e) => {
            tracing::warn!(%e, "check_embed_size failed; proceeding to embed");
            None
        }
    };
    if let Some(oversize) = oversize {
        let row =
            klams_store::OversizeWrite::from_oversize(&oversize, Some(author_id), agent_name, text);
        if let Err(e) = state.store.postgres.insert_oversize_write(&row).await {
            tracing::warn!(%e, "insert_oversize_write failed (log is best-effort)");
        }
        mcp_metrics::record_oversize_write(agent_name);
        return Err(envelope(errors::PAYLOAD_TOO_LARGE, oversize.to_string()));
    }
    Ok(())
}

pub(crate) fn sha256_hex(s: &str) -> String {
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
