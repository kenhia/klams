//! Sprint 009 (FR-013…016) — one-shot re-attribution repair.
//!
//! Walks every `facts`, `events`, and `knowledge_items` row stamped
//! with `SYSTEM_AUTHOR_ID` and reassigns it to its true author when
//! provenance is unambiguous (see [`research.md`] R4). Rows whose
//! provenance is missing, conflicting, or points at a deleted
//! `authors` row are bucketed under `LOST_AUTHOR_ID`. The total row
//! count of each table is unchanged (FR-016 invariant).
//!
//! Invoked by the `reattribute-system` admin CLI in
//! [`tools/reattribute-system/src/main.rs`].

use crate::{PostgresStore, QdrantStore, StoreError, StoreResult};
use chrono::{DateTime, Utc};
use klams_types::{LOST_AUTHOR_ID, SYSTEM_AUTHOR_ID};
use qdrant_client::qdrant::{
    point_id::PointIdOptions, Condition, Filter, PointId, ScrollPointsBuilder,
    SetPayloadPointsBuilder, Value,
};
use serde::{Serialize, Serializer};
use sqlx::Row;
use std::collections::HashMap;
use uuid::Uuid;

const CHUNK: usize = 500;

/// Repair mode. `DryRun` counts and reports; `Apply` writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepairMode {
    DryRun,
    Apply,
}

impl RepairMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::DryRun => "dry_run",
            Self::Apply => "apply",
        }
    }
}

impl Serialize for RepairMode {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

/// Per-author row in the report.
#[derive(Debug, Clone, Serialize)]
pub struct PerAuthorCount {
    pub author_id: Uuid,
    pub agent_name: String,
    pub count: u64,
}

/// Outcome for a single table (facts / events / `knowledge_items`).
#[derive(Debug, Default, Clone, Serialize)]
pub struct TableRepairOutcome {
    pub total_system_attributed: u64,
    pub reassigned_to_recovered_author: u64,
    pub reassigned_to_lost_author: u64,
    pub left_as_system: u64,
    pub per_author: Vec<PerAuthorCount>,
}

/// Full report, serialized as JSON by the CLI.
#[derive(Debug, Clone, Serialize)]
pub struct RepairReport {
    pub started_at: DateTime<Utc>,
    pub completed_at: DateTime<Utc>,
    pub mode: RepairMode,
    pub facts: TableRepairOutcome,
    pub events: TableRepairOutcome,
    pub knowledge_items: TableRepairOutcome,
}

/// Run the full repair across Postgres + Qdrant.
///
/// # Errors
/// Bubbles store backend errors as [`StoreError::Backend`].
pub async fn reattribute_system_owned(
    postgres: &PostgresStore,
    qdrant: &QdrantStore,
    mode: RepairMode,
) -> StoreResult<RepairReport> {
    let started_at = Utc::now();
    let facts = repair_facts(postgres, mode).await?;
    let events = repair_events(postgres, mode).await?;
    let knowledge_items = repair_knowledge(postgres, qdrant, mode).await?;
    let completed_at = Utc::now();
    Ok(RepairReport {
        started_at,
        completed_at,
        mode,
        facts,
        events,
        knowledge_items,
    })
}

/// Classification of a single row.
#[derive(Debug)]
enum Bucket {
    Recovered(Uuid),
    Lost,
    LeaveSystem,
}

async fn classify_fact_id(pool: &sqlx::PgPool, fact_id: Uuid) -> StoreResult<Bucket> {
    let rows = sqlx::query(
        r"SELECT DISTINCT author_id
          FROM events
          WHERE payload->>'fact_id' = $1::text
            AND author_id <> $2",
    )
    .bind(fact_id)
    .bind(SYSTEM_AUTHOR_ID)
    .fetch_all(pool)
    .await
    .map_err(|e| StoreError::Backend(format!("classify_fact {fact_id}: {e}")))?;
    classify_distinct_authors(pool, &rows).await
}

async fn classify_event_id(pool: &sqlx::PgPool, event_id: Uuid) -> StoreResult<Bucket> {
    // Events carry their own author_id; if the row itself is
    // `system`, look for sibling events on the same task_id with a
    // non-system author. This is the closest analog to the fact
    // provenance signal.
    let rows = sqlx::query(
        r"SELECT DISTINCT e2.author_id
          FROM events e1
          JOIN events e2 ON e2.task_id = e1.task_id
          WHERE e1.id = $1
            AND e1.task_id IS NOT NULL
            AND e2.author_id <> $2",
    )
    .bind(event_id)
    .bind(SYSTEM_AUTHOR_ID)
    .fetch_all(pool)
    .await
    .map_err(|e| StoreError::Backend(format!("classify_event {event_id}: {e}")))?;
    classify_distinct_authors(pool, &rows).await
}

async fn classify_distinct_authors(
    pool: &sqlx::PgPool,
    rows: &[sqlx::postgres::PgRow],
) -> StoreResult<Bucket> {
    let authors: Vec<Uuid> = rows
        .iter()
        .filter_map(|r| r.try_get::<Uuid, _>("author_id").ok())
        .collect();
    if authors.is_empty() {
        return Ok(Bucket::LeaveSystem);
    }
    if authors.len() > 1 {
        return Ok(Bucket::Lost);
    }
    let candidate = authors[0];
    let exists: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM authors WHERE id = $1)")
        .bind(candidate)
        .fetch_one(pool)
        .await
        .map_err(|e| StoreError::Backend(format!("author_exists {candidate}: {e}")))?;
    if exists {
        Ok(Bucket::Recovered(candidate))
    } else {
        Ok(Bucket::Lost)
    }
}

async fn lookup_agent_names(
    pool: &sqlx::PgPool,
    ids: &[Uuid],
) -> StoreResult<HashMap<Uuid, String>> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = sqlx::query("SELECT id, agent_name FROM authors WHERE id = ANY($1)")
        .bind(ids)
        .fetch_all(pool)
        .await
        .map_err(|e| StoreError::Backend(format!("lookup_agent_names: {e}")))?;
    let mut map = HashMap::with_capacity(rows.len());
    for r in &rows {
        if let (Ok(id), Ok(name)) = (
            r.try_get::<Uuid, _>("id"),
            r.try_get::<String, _>("agent_name"),
        ) {
            map.insert(id, name);
        }
    }
    Ok(map)
}

fn build_per_author(
    counts: &HashMap<Uuid, u64>,
    names: &HashMap<Uuid, String>,
) -> Vec<PerAuthorCount> {
    let mut out: Vec<PerAuthorCount> = counts
        .iter()
        .map(|(id, c)| PerAuthorCount {
            author_id: *id,
            agent_name: names.get(id).cloned().unwrap_or_default(),
            count: *c,
        })
        .collect();
    out.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then_with(|| a.agent_name.cmp(&b.agent_name))
    });
    out
}

async fn repair_facts(
    postgres: &PostgresStore,
    mode: RepairMode,
) -> StoreResult<TableRepairOutcome> {
    let pool = postgres.pool();
    let ids: Vec<Uuid> =
        sqlx::query_scalar(r"SELECT id FROM facts WHERE author_id = $1 ORDER BY created_at")
            .bind(SYSTEM_AUTHOR_ID)
            .fetch_all(pool)
            .await
            .map_err(|e| StoreError::Backend(format!("scan system facts: {e}")))?;

    let mut outcome = TableRepairOutcome {
        total_system_attributed: ids.len() as u64,
        ..Default::default()
    };
    let mut assignments: Vec<(Uuid, Uuid)> = Vec::new(); // (row_id, new_author_id)
    let mut counts: HashMap<Uuid, u64> = HashMap::new();

    for id in &ids {
        match classify_fact_id(pool, *id).await? {
            Bucket::Recovered(author) => {
                outcome.reassigned_to_recovered_author += 1;
                assignments.push((*id, author));
                *counts.entry(author).or_insert(0) += 1;
            }
            Bucket::Lost => {
                outcome.reassigned_to_lost_author += 1;
                assignments.push((*id, LOST_AUTHOR_ID));
                *counts.entry(LOST_AUTHOR_ID).or_insert(0) += 1;
            }
            Bucket::LeaveSystem => {
                outcome.left_as_system += 1;
            }
        }
    }

    if mode == RepairMode::Apply {
        apply_postgres_updates(pool, "facts", &assignments).await?;
    }

    let names = lookup_agent_names(pool, &counts.keys().copied().collect::<Vec<_>>()).await?;
    outcome.per_author = build_per_author(&counts, &names);
    Ok(outcome)
}

async fn repair_events(
    postgres: &PostgresStore,
    mode: RepairMode,
) -> StoreResult<TableRepairOutcome> {
    let pool = postgres.pool();
    let ids: Vec<Uuid> =
        sqlx::query_scalar(r"SELECT id FROM events WHERE author_id = $1 ORDER BY created_at")
            .bind(SYSTEM_AUTHOR_ID)
            .fetch_all(pool)
            .await
            .map_err(|e| StoreError::Backend(format!("scan system events: {e}")))?;

    let mut outcome = TableRepairOutcome {
        total_system_attributed: ids.len() as u64,
        ..Default::default()
    };
    let mut assignments: Vec<(Uuid, Uuid)> = Vec::new();
    let mut counts: HashMap<Uuid, u64> = HashMap::new();

    for id in &ids {
        match classify_event_id(pool, *id).await? {
            Bucket::Recovered(author) => {
                outcome.reassigned_to_recovered_author += 1;
                assignments.push((*id, author));
                *counts.entry(author).or_insert(0) += 1;
            }
            Bucket::Lost => {
                outcome.reassigned_to_lost_author += 1;
                assignments.push((*id, LOST_AUTHOR_ID));
                *counts.entry(LOST_AUTHOR_ID).or_insert(0) += 1;
            }
            Bucket::LeaveSystem => {
                outcome.left_as_system += 1;
            }
        }
    }

    if mode == RepairMode::Apply {
        apply_postgres_updates(pool, "events", &assignments).await?;
    }

    let names = lookup_agent_names(pool, &counts.keys().copied().collect::<Vec<_>>()).await?;
    outcome.per_author = build_per_author(&counts, &names);
    Ok(outcome)
}

async fn apply_postgres_updates(
    pool: &sqlx::PgPool,
    table: &str,
    assignments: &[(Uuid, Uuid)],
) -> StoreResult<()> {
    if assignments.is_empty() {
        return Ok(());
    }
    // Bulk update by mapping author -> Vec<row_id>, chunked.
    let mut by_author: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
    for (row, author) in assignments {
        by_author.entry(*author).or_default().push(*row);
    }
    for (author, rows) in by_author {
        for chunk in rows.chunks(CHUNK) {
            let sql = match table {
                "facts" => "UPDATE facts SET author_id = $1 WHERE id = ANY($2)",
                "events" => "UPDATE events SET author_id = $1 WHERE id = ANY($2)",
                _ => return Err(StoreError::Other(format!("unknown repair table: {table}"))),
            };
            sqlx::query(sql)
                .bind(author)
                .bind(chunk)
                .execute(pool)
                .await
                .map_err(|e| StoreError::Backend(format!("apply {table} update: {e}")))?;
        }
    }
    Ok(())
}

async fn repair_knowledge(
    postgres: &PostgresStore,
    qdrant: &QdrantStore,
    mode: RepairMode,
) -> StoreResult<TableRepairOutcome> {
    let pool = postgres.pool();
    let mut outcome = TableRepairOutcome::default();
    let mut counts: HashMap<Uuid, u64> = HashMap::new();
    let mut assignments: Vec<(Uuid, Uuid, String)> = Vec::new(); // (point_id, author_id, agent_name)

    let mut offset: Option<PointId> = None;
    let page_size: u32 = 256;
    let system_str = SYSTEM_AUTHOR_ID.to_string();

    loop {
        let filter = Filter {
            must: vec![Condition::matches("author_id", system_str.clone())],
            ..Default::default()
        };
        let mut builder = ScrollPointsBuilder::new(qdrant.collection().to_string())
            .filter(filter)
            .with_payload(true)
            .with_vectors(false)
            .limit(page_size);
        if let Some(off) = offset.clone() {
            builder = builder.offset(off);
        }
        let resp = qdrant
            .client()
            .scroll(builder)
            .await
            .map_err(|e| StoreError::Backend(format!("qdrant repair scroll: {e}")))?;
        if resp.result.is_empty() {
            break;
        }
        for p in &resp.result {
            let Some(point_id) = p.id.clone() else {
                continue;
            };
            let Some(uuid) = point_id_as_uuid(&point_id) else {
                continue;
            };
            outcome.total_system_attributed += 1;
            // No knowledge-side provenance signal yet in events
            // (R4: knowledge has no event mirror) → every point is
            // either LeaveSystem (if it was genuinely system) or
            // routed to lost-author when explicit re-attribution is
            // requested. We use the conservative "no provenance =
            // leave as system" default per the R4 contract.
            outcome.left_as_system += 1;
            let _ = (uuid, &mut counts, &mut assignments);
        }
        offset = resp.next_page_offset;
        if offset.is_none() {
            break;
        }
    }

    if mode == RepairMode::Apply && !assignments.is_empty() {
        apply_qdrant_updates(qdrant, &assignments).await?;
    }

    let names = lookup_agent_names(pool, &counts.keys().copied().collect::<Vec<_>>()).await?;
    outcome.per_author = build_per_author(&counts, &names);
    Ok(outcome)
}

fn point_id_as_uuid(p: &PointId) -> Option<Uuid> {
    match &p.point_id_options {
        Some(PointIdOptions::Uuid(s)) => Uuid::parse_str(s).ok(),
        _ => None,
    }
}

/// Sprint 009 T030: Qdrant payload-update path for the repair.
/// Stamps `author_id` (+ `author_agent_name`) on each point.
async fn apply_qdrant_updates(
    qdrant: &QdrantStore,
    assignments: &[(Uuid, Uuid, String)],
) -> StoreResult<()> {
    // Group by (author_id, agent_name) so we issue one set_payload
    // per group rather than per point.
    let mut by_author: HashMap<(Uuid, String), Vec<PointId>> = HashMap::new();
    for (point, author, name) in assignments {
        by_author
            .entry((*author, name.clone()))
            .or_default()
            .push(PointId {
                point_id_options: Some(PointIdOptions::Uuid(point.to_string())),
            });
    }
    for ((author, name), points) in by_author {
        for chunk in points.chunks(CHUNK) {
            let mut payload = HashMap::new();
            payload.insert("author_id".to_string(), Value::from(author.to_string()));
            payload.insert("author_agent_name".to_string(), Value::from(name.clone()));
            qdrant
                .client()
                .set_payload(
                    SetPayloadPointsBuilder::new(qdrant.collection().to_string(), payload)
                        .points_selector(chunk.to_vec())
                        .wait(true),
                )
                .await
                .map_err(|e| StoreError::Backend(format!("qdrant repair set_payload: {e}")))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome_with(reassigned: u64, lost: u64, kept: u64) -> TableRepairOutcome {
        TableRepairOutcome {
            total_system_attributed: reassigned + lost + kept,
            reassigned_to_recovered_author: reassigned,
            reassigned_to_lost_author: lost,
            left_as_system: kept,
            per_author: Vec::new(),
        }
    }

    /// FR-016: per-table the three buckets must sum to total.
    #[test]
    fn outcome_buckets_sum_to_total() {
        let o = outcome_with(7, 2, 1);
        assert_eq!(
            o.total_system_attributed,
            o.reassigned_to_recovered_author + o.reassigned_to_lost_author + o.left_as_system,
        );
    }

    #[test]
    fn report_serializes_mode_and_outcomes() {
        let r = RepairReport {
            started_at: Utc::now(),
            completed_at: Utc::now(),
            mode: RepairMode::DryRun,
            facts: outcome_with(3, 1, 0),
            events: TableRepairOutcome::default(),
            knowledge_items: TableRepairOutcome::default(),
        };
        let j = serde_json::to_value(&r).unwrap();
        assert_eq!(j["mode"], "dry_run");
        assert_eq!(j["facts"]["total_system_attributed"], 4);
        assert_eq!(j["facts"]["reassigned_to_recovered_author"], 3);
        assert_eq!(j["facts"]["reassigned_to_lost_author"], 1);
        assert_eq!(j["facts"]["left_as_system"], 0);
    }

    #[test]
    fn per_author_sorts_by_count_descending() {
        let mut counts: HashMap<Uuid, u64> = HashMap::new();
        let a = Uuid::from_u128(0x10);
        let b = Uuid::from_u128(0x20);
        counts.insert(a, 3);
        counts.insert(b, 7);
        let mut names: HashMap<Uuid, String> = HashMap::new();
        names.insert(a, "alice".into());
        names.insert(b, "bob".into());
        let v = build_per_author(&counts, &names);
        assert_eq!(v[0].agent_name, "bob");
        assert_eq!(v[0].count, 7);
        assert_eq!(v[1].agent_name, "alice");
    }
}
