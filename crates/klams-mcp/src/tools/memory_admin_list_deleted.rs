//! `memory_admin_list_deleted` MCP tool (sprint 007 T049, US4).
//!
//! Paginated listing of soft-deleted facts and knowledge points for
//! the rogue-agent recovery flow (FR-013, SC-008). Events are
//! excluded automatically because they are append-only.
//!
//! ## Cursor format
//!
//! Cursors are opaque strings with one of three shapes:
//!
//! - `f:<rfc3339>:<uuid>` — resume fact pagination after the given
//!   `(deleted_at, id)` tuple (DESC order).
//! - `k` — switch from facts to knowledge; start at the head of the
//!   knowledge scroll.
//! - `k:<uuid>` — resume knowledge pagination from the given Qdrant
//!   scroll offset.
//!
//! Callers MUST treat cursors as opaque and pass them back verbatim.

use crate::{
    errors::{self, envelope, ErrorEnvelope},
    tools::McpState,
};
use chrono::{DateTime, Utc};
use klams_types::{PublicAuthorRef, PublicMemory, PublicMemoryContent};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

const DEFAULT_LIMIT: u32 = 100;
const MAX_LIMIT: u32 = 500;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum DeletableKindFilter {
    Fact,
    Knowledge,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryAdminListDeletedArgs {
    #[serde(default)]
    pub kinds: Option<Vec<DeletableKindFilter>>,
    #[serde(default)]
    pub since: Option<DateTime<Utc>>,
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub author_id: Option<Uuid>,
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeletedMemory {
    #[serde(flatten)]
    pub memory: PublicMemory,
    pub deleted_at: DateTime<Utc>,
    pub deleted_by: PublicAuthorRef,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemoryAdminListDeletedOutput {
    pub results: Vec<DeletedMemory>,
    pub next_cursor: Option<String>,
}

enum Section {
    Facts(Option<(time::OffsetDateTime, Uuid)>),
    Knowledge(Option<Uuid>),
}

fn parse_cursor(raw: &str) -> Result<Section, ErrorEnvelope> {
    if raw == "k" {
        return Ok(Section::Knowledge(None));
    }
    if let Some(rest) = raw.strip_prefix("k:") {
        let id = Uuid::parse_str(rest)
            .map_err(|_| envelope(errors::SCHEMA_VALIDATION_FAILED, "invalid cursor"))?;
        return Ok(Section::Knowledge(Some(id)));
    }
    if let Some(rest) = raw.strip_prefix("f:") {
        let (ts, id) = rest
            .rsplit_once(':')
            .ok_or_else(|| envelope(errors::SCHEMA_VALIDATION_FAILED, "invalid cursor"))?;
        let ts = time::OffsetDateTime::parse(ts, &Rfc3339)
            .map_err(|_| envelope(errors::SCHEMA_VALIDATION_FAILED, "invalid cursor"))?;
        let id = Uuid::parse_str(id)
            .map_err(|_| envelope(errors::SCHEMA_VALIDATION_FAILED, "invalid cursor"))?;
        return Ok(Section::Facts(Some((ts, id))));
    }
    Err(envelope(errors::SCHEMA_VALIDATION_FAILED, "invalid cursor"))
}

fn offset_to_chrono(ts: time::OffsetDateTime) -> DateTime<Utc> {
    DateTime::<Utc>::from_timestamp_nanos(i64::try_from(ts.unix_timestamp_nanos()).unwrap_or(0))
}

/// Execute `memory_admin_list_deleted`.
///
/// # Errors
/// Returns an [`ErrorEnvelope`] for `INVALID_LIMIT`,
/// `SCHEMA_VALIDATION_FAILED`, or `INTERNAL_ERROR`.
#[allow(clippy::too_many_lines)]
pub async fn run(
    state: &McpState,
    args: MemoryAdminListDeletedArgs,
) -> Result<MemoryAdminListDeletedOutput, ErrorEnvelope> {
    let limit = args.limit.unwrap_or(DEFAULT_LIMIT);
    if !(1..=MAX_LIMIT).contains(&limit) {
        return Err(envelope(
            errors::INVALID_LIMIT,
            format!("limit must be between 1 and {MAX_LIMIT}"),
        ));
    }

    let kinds: Vec<DeletableKindFilter> = args
        .kinds
        .unwrap_or_else(|| vec![DeletableKindFilter::Fact, DeletableKindFilter::Knowledge]);
    let want_facts = kinds.iter().any(|k| matches!(k, DeletableKindFilter::Fact));
    let want_knowledge = kinds
        .iter()
        .any(|k| matches!(k, DeletableKindFilter::Knowledge));

    let section = match args.cursor.as_deref() {
        None => {
            if want_facts {
                Section::Facts(None)
            } else if want_knowledge {
                Section::Knowledge(None)
            } else {
                return Ok(MemoryAdminListDeletedOutput {
                    results: Vec::new(),
                    next_cursor: None,
                });
            }
        }
        Some(s) => parse_cursor(s)?,
    };

    let since = args.since.map(|t| {
        time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(
            t.timestamp_nanos_opt().unwrap_or(0),
        ))
        .unwrap_or_else(|_| time::OffsetDateTime::now_utc())
    });

    match section {
        Section::Facts(cursor) if want_facts => {
            let rows = state
                .store
                .postgres
                .list_deleted_facts_filtered(limit, since, args.author_id, cursor)
                .await
                .map_err(|e| {
                    envelope(
                        errors::INTERNAL_ERROR,
                        format!("list_deleted_facts_filtered: {e}"),
                    )
                })?;
            let len = rows.len();
            let last = rows.last().map(|(r, _)| (r.deleted_at, r.fact.id));
            let mut author_ids: Vec<Uuid> = Vec::with_capacity(rows.len());
            for (r, _) in &rows {
                if let Some(d) = r.deleted_by_author_id {
                    author_ids.push(d);
                }
            }
            let deleters = fetch_authors(state, &author_ids).await;
            let results = rows
                .into_iter()
                .map(|(r, a)| {
                    let author = PublicAuthorRef {
                        agent_name: a.agent_name,
                        model: a.model,
                        repo: a.repo,
                    };
                    let deleter = r
                        .deleted_by_author_id
                        .and_then(|d| deleters.get(&d).cloned())
                        .unwrap_or_else(unknown_author_ref);
                    let mem = PublicMemory {
                        id: r.fact.id,
                        content: PublicMemoryContent::Fact {
                            fact_type: r.fact.fact_type.as_str().to_string(),
                            payload: r.fact.payload.clone(),
                        },
                        tags: Vec::new(),
                        author,
                        created_at: offset_to_chrono(r.fact.created_at),
                        updated_at: offset_to_chrono(r.fact.updated_at),
                        deleted_at: None,
                        deleted_by_author_id: None,
                    };
                    DeletedMemory {
                        memory: mem,
                        deleted_at: offset_to_chrono(r.deleted_at),
                        deleted_by: deleter,
                    }
                })
                .collect::<Vec<_>>();
            let next_cursor = if u32::try_from(len).is_ok_and(|n| n == limit) {
                last.map(|(ts, id)| format_facts_cursor(ts, id))
            } else if want_knowledge {
                Some("k".to_string())
            } else {
                None
            };
            Ok(MemoryAdminListDeletedOutput {
                results,
                next_cursor,
            })
        }
        Section::Facts(_) => {
            // Cursor points at facts but caller filtered them out — jump to knowledge.
            if want_knowledge {
                list_knowledge(state, limit, args.author_id, None).await
            } else {
                Ok(MemoryAdminListDeletedOutput {
                    results: Vec::new(),
                    next_cursor: None,
                })
            }
        }
        Section::Knowledge(offset) if want_knowledge => {
            list_knowledge(state, limit, args.author_id, offset).await
        }
        Section::Knowledge(_) => Ok(MemoryAdminListDeletedOutput {
            results: Vec::new(),
            next_cursor: None,
        }),
    }
}

async fn list_knowledge(
    state: &McpState,
    limit: u32,
    author_id: Option<Uuid>,
    offset: Option<Uuid>,
) -> Result<MemoryAdminListDeletedOutput, ErrorEnvelope> {
    let (rows, next) = state
        .store
        .qdrant
        .list_deleted_knowledge(limit, author_id, offset)
        .await
        .map_err(|e| {
            envelope(
                errors::INTERNAL_ERROR,
                format!("list_deleted_knowledge: {e}"),
            )
        })?;
    let mut author_ids: Vec<Uuid> = Vec::with_capacity(rows.len() * 2);
    for (_, _, deleter, author) in &rows {
        if let Some(d) = deleter {
            author_ids.push(*d);
        }
        if let Some(a) = author {
            author_ids.push(*a);
        }
    }
    let authors = fetch_authors(state, &author_ids).await;
    let results = rows
        .into_iter()
        .map(|(item, deleted_at, deleter, author_id)| {
            let author = author_id
                .and_then(|a| authors.get(&a).cloned())
                .unwrap_or_else(unknown_author_ref);
            let deleter_ref = deleter
                .and_then(|d| authors.get(&d).cloned())
                .unwrap_or_else(unknown_author_ref);
            let mem = PublicMemory {
                id: item.id,
                content: PublicMemoryContent::Knowledge {
                    text: item.text.clone(),
                    source_path: item.file.clone(),
                    repo: item.repo.clone(),
                },
                tags: item.tags.clone(),
                author,
                created_at: offset_to_chrono(item.created_at),
                updated_at: offset_to_chrono(item.updated_at),
                deleted_at: None,
                deleted_by_author_id: None,
            };
            DeletedMemory {
                memory: mem,
                deleted_at: offset_to_chrono(deleted_at),
                deleted_by: deleter_ref,
            }
        })
        .collect::<Vec<_>>();
    let next_cursor = next.map(|id| format!("k:{id}"));
    Ok(MemoryAdminListDeletedOutput {
        results,
        next_cursor,
    })
}

fn format_facts_cursor(ts: time::OffsetDateTime, id: Uuid) -> String {
    let ts_str = ts.format(&Rfc3339).unwrap_or_default();
    format!("f:{ts_str}:{id}")
}

async fn fetch_authors(state: &McpState, ids: &[Uuid]) -> HashMap<Uuid, PublicAuthorRef> {
    let mut out: HashMap<Uuid, PublicAuthorRef> = HashMap::new();
    for id in ids {
        if out.contains_key(id) {
            continue;
        }
        if let Ok(Some(a)) = state.store.postgres.get_author_by_id(*id).await {
            out.insert(
                *id,
                PublicAuthorRef {
                    agent_name: a.agent_name,
                    model: a.model,
                    repo: a.repo,
                },
            );
        }
    }
    out
}

fn unknown_author_ref() -> PublicAuthorRef {
    PublicAuthorRef {
        agent_name: "unknown".into(),
        model: None,
        repo: None,
    }
}
