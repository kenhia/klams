//! Postgres adapter for facts and events.
//!
//! Uses runtime-checked `sqlx::query` queries; integration tests in
//! user-story phases validate every statement against a real Postgres.

use crate::{DissentQuery, EventQuery, FactQuery, StoreError, StoreResult, TextHit};
use async_trait::async_trait;
use klams_types::{
    canonical_json_hash, AppendEvent, AuthorRecord, Dissent, DissentStatus, Event, Fact, FactType,
    FactWriteOutcome, RegisterAuthorArgs, Source, UpsertFact,
};
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};
use uuid::Uuid;

/// Convert a `chrono::DateTime<Utc>` to a `time::OffsetDateTime` so it
/// can be bound through sqlx's `time` integration. Used by sprint 008
/// cross-author page queries.
fn chrono_to_offset(ts: chrono::DateTime<chrono::Utc>) -> time::OffsetDateTime {
    time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(
        ts.timestamp_nanos_opt().unwrap_or(0),
    ))
    .unwrap_or(time::OffsetDateTime::UNIX_EPOCH)
}

#[derive(Debug, Clone)]
pub struct PostgresStore {
    pool: PgPool,
}

impl PostgresStore {
    /// Connect to Postgres and run pending migrations.
    pub async fn connect(url: &str, max_connections: u32) -> StoreResult<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(max_connections)
            .connect(url)
            .await
            .map_err(|e| StoreError::from_sqlx("connect", &e))?;
        sqlx::migrate!("../../migrations")
            .run(&pool)
            .await
            // A failed migration is a permanent, operator-visible fault;
            // it is not sqlx::Error and never worth a retry hint.
            .map_err(|e| StoreError::Backend(format!("migrate: {e}")))?;
        Ok(Self { pool })
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Cheap liveness probe.
    pub async fn health(&self) -> StoreResult<()> {
        sqlx::query("SELECT 1")
            .execute(&self.pool)
            .await
            .map(|_| ())
            .map_err(|e| StoreError::from_sqlx("pg health", &e))
    }

    pub async fn append_event(&self, req: AppendEvent) -> StoreResult<Event> {
        let row = sqlx::query(
            r"
            INSERT INTO events (id, task_id, category, payload, source, author_id)
            VALUES ($1, $2, $3, $4, $5, $6)
            RETURNING id, task_id, category, payload, source, created_at
            ",
        )
        .bind(req.id)
        .bind(req.task_id)
        .bind(&req.category)
        .bind(&req.payload)
        .bind(req.source.as_str())
        .bind(req.author_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| StoreError::from_sqlx("append_event", &e))?;
        row_to_event(&row)
    }

    pub async fn list_facts(&self, q: FactQuery) -> StoreResult<(Vec<Fact>, Option<String>)> {
        let limit = i64::from(q.limit.clamp(1, 500));
        let mut qb: sqlx::QueryBuilder<sqlx::Postgres> = sqlx::QueryBuilder::new(
            "SELECT id, type, payload, version, source, confidence, decay_weight,
             use_count, last_used_at, created_at, updated_at FROM facts WHERE deleted_at IS NULL",
        );
        if let Some(ft) = q.fact_type {
            qb.push(" AND type = ").push_bind(ft.as_str().to_string());
        }
        if let Some(s) = q.source {
            qb.push(" AND source = ").push_bind(s.as_str().to_string());
        }
        if let Some(t) = q.created_after {
            qb.push(" AND created_at > ").push_bind(t);
        }
        if let Some(t) = q.created_before {
            qb.push(" AND created_at < ").push_bind(t);
        }
        qb.push(" ORDER BY created_at DESC, id DESC LIMIT ")
            .push_bind(limit);

        let rows = qb
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| StoreError::from_sqlx("list_facts", &e))?;
        let mut items = Vec::with_capacity(rows.len());
        for r in &rows {
            items.push(row_to_fact(r)?);
        }
        Ok((items, q.cursor))
    }

    pub async fn list_events(&self, q: EventQuery) -> StoreResult<(Vec<Event>, Option<String>)> {
        let limit = i64::from(q.limit.clamp(1, 500));
        let mut qb: sqlx::QueryBuilder<sqlx::Postgres> = sqlx::QueryBuilder::new(
            "SELECT id, task_id, category, payload, source, created_at
             FROM events WHERE 1=1",
        );
        if let Some(task_id) = q.task_id {
            if let Some(s) = &q.payload_task_id {
                qb.push(" AND (task_id = ")
                    .push_bind(task_id)
                    .push(" OR payload->>'task_id' = ")
                    .push_bind(s.clone())
                    .push(")");
            } else {
                qb.push(" AND task_id = ").push_bind(task_id);
            }
        } else if let Some(s) = q.payload_task_id {
            qb.push(" AND payload->>'task_id' = ").push_bind(s);
        }
        if let Some(c) = q.category {
            qb.push(" AND category = ").push_bind(c);
        }
        if let Some(s) = q.service {
            qb.push(" AND payload->>'service' = ").push_bind(s);
        }
        if let Some(t) = q.created_after {
            qb.push(" AND created_at > ").push_bind(t);
        }
        if let Some(t) = q.created_before {
            qb.push(" AND created_at < ").push_bind(t);
        }
        qb.push(" ORDER BY created_at ASC, id ASC LIMIT ")
            .push_bind(limit);

        let rows = qb
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| StoreError::from_sqlx("list_events", &e))?;
        let mut items = Vec::with_capacity(rows.len());
        for r in &rows {
            items.push(row_to_event(r)?);
        }
        Ok((items, q.cursor))
    }

    pub async fn search_text(
        &self,
        query: &str,
        top_k: u32,
    ) -> StoreResult<(Vec<TextHit>, Vec<TextHit>)> {
        if query.trim().is_empty() {
            return Err(StoreError::Other(
                "search_text: query must be non-empty".into(),
            ));
        }
        let limit = i64::from(top_k.max(1));

        let fact_rows = sqlx::query(
            r"
            SELECT id, payload,
                   (ts_rank_cd(tsv, plainto_tsquery('english', $1))
                    * decay_weight
                    * confidence
                    * (1.0 + ln(1.0 + use_count)))::float4 AS score
            FROM facts
            WHERE tsv @@ plainto_tsquery('english', $1)
              AND deleted_at IS NULL
            ORDER BY score DESC, id ASC LIMIT $2
            ",
        )
        .bind(query)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| StoreError::from_sqlx("search_text(facts)", &e))?;

        let mut facts = Vec::with_capacity(fact_rows.len());
        for r in &fact_rows {
            facts.push(TextHit {
                id: r.try_get("id").map_err(map_decode)?,
                score: r.try_get("score").map_err(map_decode)?,
                payload: r.try_get("payload").map_err(map_decode)?,
            });
        }

        let event_rows = sqlx::query(
            r"
            SELECT id, payload,
                   ts_rank_cd(tsv, plainto_tsquery('english', $1)) AS score
            FROM events
            WHERE tsv @@ plainto_tsquery('english', $1)
            ORDER BY score DESC, id ASC LIMIT $2
            ",
        )
        .bind(query)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| StoreError::from_sqlx("search_text(events)", &e))?;

        let mut events = Vec::with_capacity(event_rows.len());
        for r in &event_rows {
            events.push(TextHit {
                id: r.try_get("id").map_err(map_decode)?,
                score: r.try_get("score").map_err(map_decode)?,
                payload: r.try_get("payload").map_err(map_decode)?,
            });
        }
        Ok((facts, events))
    }
}

#[allow(clippy::needless_pass_by_value)] // matches signature sqlx::Result expects in map_err
fn map_decode(e: sqlx::Error) -> StoreError {
    StoreError::from_sqlx("decode", &e)
}

fn parse_fact_type(s: &str) -> StoreResult<FactType> {
    Ok(match s {
        "UserFact" => FactType::UserFact,
        "TaskFact" => FactType::TaskFact,
        "EnvFact" => FactType::EnvFact,
        other => return Err(StoreError::Other(format!("unknown FactType `{other}`"))),
    })
}

fn parse_source(s: &str) -> StoreResult<Source> {
    Ok(match s {
        "User" => Source::User,
        "Controller" => Source::Controller,
        "Task" => Source::Task,
        "AgentProposal" => Source::AgentProposal,
        other => return Err(StoreError::Other(format!("unknown Source `{other}`"))),
    })
}

fn row_to_fact(row: &sqlx::postgres::PgRow) -> StoreResult<Fact> {
    let type_str: String = row.try_get("type").map_err(map_decode)?;
    let source_str: String = row.try_get("source").map_err(map_decode)?;
    Ok(Fact {
        id: row.try_get("id").map_err(map_decode)?,
        fact_type: parse_fact_type(&type_str)?,
        payload: row.try_get("payload").map_err(map_decode)?,
        version: row.try_get("version").map_err(map_decode)?,
        source: parse_source(&source_str)?,
        confidence: row.try_get("confidence").map_err(map_decode)?,
        decay_weight: row.try_get("decay_weight").map_err(map_decode)?,
        use_count: row.try_get("use_count").map_err(map_decode)?,
        dissent_count: row.try_get::<i32, _>("dissent_count").unwrap_or(0),
        last_used_at: row.try_get("last_used_at").map_err(map_decode)?,
        created_at: row.try_get("created_at").map_err(map_decode)?,
        updated_at: row.try_get("updated_at").map_err(map_decode)?,
    })
}

fn row_to_event(row: &sqlx::postgres::PgRow) -> StoreResult<Event> {
    let source_str: String = row.try_get("source").map_err(map_decode)?;
    Ok(Event {
        id: row.try_get("id").map_err(map_decode)?,
        task_id: row.try_get("task_id").map_err(map_decode)?,
        category: row.try_get("category").map_err(map_decode)?,
        payload: row.try_get("payload").map_err(map_decode)?,
        source: parse_source(&source_str)?,
        created_at: row.try_get("created_at").map_err(map_decode)?,
    })
}

fn parse_dissent_status(s: &str) -> StoreResult<DissentStatus> {
    Ok(match s {
        "pending" => DissentStatus::Pending,
        "promoted" => DissentStatus::Promoted,
        "discarded" => DissentStatus::Discarded,
        "orphaned" => DissentStatus::Orphaned,
        other => {
            return Err(StoreError::Other(format!(
                "unknown DissentStatus `{other}`"
            )))
        }
    })
}

fn row_to_dissent(row: &sqlx::postgres::PgRow) -> StoreResult<Dissent> {
    let status_str: String = row.try_get("status").map_err(map_decode)?;
    let source_str: String = row.try_get("source").map_err(map_decode)?;
    let resolved_by_source: Option<String> =
        row.try_get("resolved_by_source").map_err(map_decode)?;
    let resolved_by_source = match resolved_by_source {
        Some(s) => Some(parse_source(&s)?),
        None => None,
    };
    Ok(Dissent {
        id: row.try_get("id").map_err(map_decode)?,
        fact_id: row.try_get("fact_id").map_err(map_decode)?,
        proposed_payload: row.try_get("proposed_payload").map_err(map_decode)?,
        source: parse_source(&source_str)?,
        status: parse_dissent_status(&status_str)?,
        submitted_at: row.try_get("submitted_at").map_err(map_decode)?,
        last_seen_at: row.try_get("last_seen_at").map_err(map_decode)?,
        submission_count: row.try_get("submission_count").map_err(map_decode)?,
        resolved_at: row.try_get("resolved_at").map_err(map_decode)?,
        resolved_by_source,
        reason: row.try_get("reason").map_err(map_decode)?,
        contradicting_memory_id: row.try_get("contradicting_memory_id").map_err(map_decode)?,
        author_id: row.try_get("author_id").map_err(map_decode)?,
    })
}

/// Trust ordering used by US2 routing. Higher is more authoritative.
/// Delegates to [`Source::trust_rank`] (klams-types) so the
/// dispatcher and the `GET /memory/policy` endpoint cannot drift.
fn trust_rank(s: Source) -> i32 {
    i32::from(s.trust_rank())
}

impl PostgresStore {
    /// Sprint-002 canonical write path. Returns a tagged outcome so
    /// the handler maps directly to HTTP 200 / 202 / 409.
    ///
    /// Semantics:
    /// - If `(type, payload_hash)` already exists (same payload), the
    ///   write is idempotent: `expected_version` must match the
    ///   stored version (else `VersionConflict`); otherwise it bumps
    ///   `updated_at` and returns `Persisted`.
    /// - If `explicit_id` targets an existing canonical fact from a
    ///   strictly higher-trust `source`, the contradicting payload
    ///   lands in `dissents` (deduped per FR-013) and returns
    ///   `Dissented`.
    /// - Otherwise the write is a brand-new canonical row.
    #[allow(clippy::too_many_lines)]
    pub async fn upsert_fact_v2(&self, req: UpsertFact) -> StoreResult<FactWriteOutcome> {
        let hash = canonical_json_hash(req.fact_type.as_str(), &req.payload);
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| StoreError::from_sqlx("upsert_fact_v2 begin", &e))?;

        // 1) Same (type, payload_hash) idempotent path.
        let existing_same: Option<(Uuid, i32, String)> = sqlx::query_as(
            "SELECT id, version, source FROM facts WHERE type=$1 AND payload_hash=$2 FOR UPDATE",
        )
        .bind(req.fact_type.as_str())
        .bind(&hash[..])
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| StoreError::from_sqlx("upsert_fact_v2 select-same", &e))?;

        if let Some((id, version, _source)) = existing_same {
            if let Some(ev) = req.expected_version {
                if ev != version {
                    return Ok(FactWriteOutcome::VersionConflict {
                        current_version: version,
                        fact_id: id,
                    });
                }
            }
            // Touch updated_at, no payload change since hashes match.
            let row = sqlx::query(
                r"UPDATE facts SET updated_at = now() WHERE id = $1
                  RETURNING id, type, payload, version, source,
                            confidence, decay_weight, use_count, dissent_count,
                            last_used_at, created_at, updated_at",
            )
            .bind(id)
            .fetch_one(&mut *tx)
            .await
            .map_err(|e| StoreError::from_sqlx("upsert_fact_v2 touch", &e))?;
            let fact = row_to_fact(&row)?;
            tx.commit()
                .await
                .map_err(|e| StoreError::from_sqlx("upsert_fact_v2 commit", &e))?;
            return Ok(FactWriteOutcome::Persisted { fact });
        }

        // 2) explicit_id targets an existing canonical fact: dissent
        //    or version-bumped amendment depending on trust.
        if let Some(id) = req.explicit_id {
            let existing: Option<(i32, String)> =
                sqlx::query_as("SELECT version, source FROM facts WHERE id=$1 FOR UPDATE")
                    .bind(id)
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(|e| StoreError::from_sqlx("upsert_fact_v2 select-id", &e))?;
            if let Some((version, source_str)) = existing {
                let canonical_source = parse_source(&source_str)?;
                if trust_rank(req.source) < trust_rank(canonical_source) {
                    // Dissent path.
                    let dissent_hash = canonical_json_hash(req.fact_type.as_str(), &req.payload);
                    let row = sqlx::query(
                        r"INSERT INTO dissents
                            (id, fact_id, proposed_payload, payload_hash, source)
                          VALUES ($1, $2, $3, $4, $5)
                          ON CONFLICT (fact_id, payload_hash) WHERE status='pending'
                          DO UPDATE SET
                            submission_count = dissents.submission_count + 1,
                            last_seen_at = now()
                          RETURNING id",
                    )
                    .bind(Uuid::now_v7())
                    .bind(id)
                    .bind(&req.payload)
                    .bind(&dissent_hash[..])
                    .bind(req.source.as_str())
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(|e| StoreError::from_sqlx("upsert_fact_v2 dissent insert", &e))?;
                    let dissent_id: Uuid = row.try_get("id").map_err(map_decode)?;
                    tx.commit()
                        .await
                        .map_err(|e| StoreError::from_sqlx("upsert_fact_v2 commit", &e))?;
                    return Ok(FactWriteOutcome::Dissented {
                        dissent_id,
                        fact_id: id,
                    });
                }
                // Same / higher trust amendment: optimistic version check.
                if let Some(ev) = req.expected_version {
                    if ev != version {
                        return Ok(FactWriteOutcome::VersionConflict {
                            current_version: version,
                            fact_id: id,
                        });
                    }
                }
                let row = sqlx::query(
                    r"UPDATE facts SET
                        payload = $2,
                        payload_hash = $3,
                        source = $4,
                        version = version + 1,
                        updated_at = now()
                      WHERE id = $1
                      RETURNING id, type, payload, version, source,
                                confidence, decay_weight, use_count, dissent_count,
                                last_used_at, created_at, updated_at",
                )
                .bind(id)
                .bind(&req.payload)
                .bind(&hash[..])
                .bind(req.source.as_str())
                .fetch_one(&mut *tx)
                .await
                .map_err(|e| StoreError::from_sqlx("upsert_fact_v2 amend", &e))?;
                let fact = row_to_fact(&row)?;
                tx.commit()
                    .await
                    .map_err(|e| StoreError::from_sqlx("upsert_fact_v2 commit", &e))?;
                return Ok(FactWriteOutcome::Persisted { fact });
            }
        }

        // 3) Brand new canonical fact.
        let id = req.explicit_id.unwrap_or_else(Uuid::now_v7);
        let row = sqlx::query(
            r"INSERT INTO facts (id, type, payload, payload_hash, source, version, author_id)
              VALUES ($1, $2, $3, $4, $5, 1, $6)
              RETURNING id, type, payload, version, source,
                        confidence, decay_weight, use_count, dissent_count,
                        last_used_at, created_at, updated_at",
        )
        .bind(id)
        .bind(req.fact_type.as_str())
        .bind(&req.payload)
        .bind(&hash[..])
        .bind(req.source.as_str())
        .bind(req.author_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| StoreError::from_sqlx("upsert_fact_v2 insert", &e))?;
        let fact = row_to_fact(&row)?;
        tx.commit()
            .await
            .map_err(|e| StoreError::from_sqlx("upsert_fact_v2 commit", &e))?;
        Ok(FactWriteOutcome::Persisted { fact })
    }

    pub async fn list_dissents(
        &self,
        q: DissentQuery,
    ) -> StoreResult<(Vec<Dissent>, Option<String>)> {
        let limit = i64::from(q.limit.clamp(1, 500));
        let mut qb: sqlx::QueryBuilder<sqlx::Postgres> = sqlx::QueryBuilder::new(
            "SELECT id, fact_id, proposed_payload, source, status,
                    submitted_at, last_seen_at, submission_count,
                    resolved_at, resolved_by_source,
                    reason, contradicting_memory_id, author_id
             FROM dissents WHERE 1=1",
        );
        if let Some(fid) = q.fact_id {
            qb.push(" AND fact_id = ").push_bind(fid);
        }
        if let Some(st) = q.status {
            qb.push(" AND status = ").push_bind(st.as_str().to_string());
        }
        if let Some(s) = q.source {
            qb.push(" AND source = ").push_bind(s.as_str().to_string());
        }
        if let Some(t) = q.created_after {
            qb.push(" AND submitted_at > ").push_bind(t);
        }
        if let Some(t) = q.created_before {
            qb.push(" AND submitted_at < ").push_bind(t);
        }
        qb.push(" ORDER BY submitted_at DESC, id DESC LIMIT ")
            .push_bind(limit);
        let rows = qb
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| StoreError::from_sqlx("list_dissents", &e))?;
        let mut items = Vec::with_capacity(rows.len());
        for r in &rows {
            items.push(row_to_dissent(r)?);
        }
        Ok((items, q.cursor))
    }

    /// Sprint 015 — file a dissent directly against a live canonical
    /// fact (MCP `dissent_propose`). Reuses the pending dedupe index:
    /// an identical `(fact_id, payload)` proposal bumps
    /// `submission_count` / `last_seen_at` and keeps the original
    /// reason/author. Returns `Ok(None)` when the fact does not exist
    /// or is soft-deleted; otherwise `(dissent_id, deduped)`.
    pub async fn propose_dissent(
        &self,
        fact_id: Uuid,
        proposed_payload: &serde_json::Value,
        author_id: Uuid,
        reason: &str,
        contradicting_memory_id: Option<Uuid>,
    ) -> StoreResult<Option<(Uuid, bool)>> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| StoreError::from_sqlx("propose begin", &e))?;

        let fact: Option<(String,)> = sqlx::query_as(
            "SELECT type FROM facts WHERE id = $1 AND deleted_at IS NULL FOR UPDATE",
        )
        .bind(fact_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| StoreError::from_sqlx("propose select fact", &e))?;
        let Some((fact_type,)) = fact else {
            return Ok(None);
        };

        let hash = canonical_json_hash(&fact_type, proposed_payload);
        let row = sqlx::query(
            r"INSERT INTO dissents
                (id, fact_id, proposed_payload, payload_hash, source,
                 reason, contradicting_memory_id, author_id)
              VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
              ON CONFLICT (fact_id, payload_hash) WHERE status='pending'
              DO UPDATE SET
                submission_count = dissents.submission_count + 1,
                last_seen_at = now()
              RETURNING id, submission_count",
        )
        .bind(Uuid::now_v7())
        .bind(fact_id)
        .bind(proposed_payload)
        .bind(&hash[..])
        .bind(Source::AgentProposal.as_str())
        .bind(reason)
        .bind(contradicting_memory_id)
        .bind(author_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| StoreError::from_sqlx("propose insert", &e))?;
        let dissent_id: Uuid = row.try_get("id").map_err(map_decode)?;
        let submission_count: i32 = row.try_get("submission_count").map_err(map_decode)?;
        tx.commit()
            .await
            .map_err(|e| StoreError::from_sqlx("propose commit", &e))?;
        Ok(Some((dissent_id, submission_count > 1)))
    }

    pub async fn get_dissent(&self, id: Uuid) -> StoreResult<Option<Dissent>> {
        let row = sqlx::query(
            "SELECT id, fact_id, proposed_payload, source, status,
                    submitted_at, last_seen_at, submission_count,
                    resolved_at, resolved_by_source,
                    reason, contradicting_memory_id, author_id
             FROM dissents WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| StoreError::from_sqlx("get_dissent", &e))?;
        match row {
            Some(r) => Ok(Some(row_to_dissent(&r)?)),
            None => Ok(None),
        }
    }

    /// Promote a pending dissent. Atomically (a) re-asserts the
    /// dissent is still pending (else `Gone`), (b) checks the
    /// canonical fact's `version` against `expected_version` (else
    /// `VersionConflict`), (c) overwrites canonical payload/source,
    /// (d) marks the dissent resolved.
    pub async fn promote_dissent(
        &self,
        dissent_id: Uuid,
        caller_source: Source,
        expected_version: i32,
    ) -> StoreResult<Fact> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| StoreError::from_sqlx("promote begin", &e))?;

        let d_row = sqlx::query(
            "SELECT id, fact_id, proposed_payload, payload_hash, source, status
             FROM dissents WHERE id = $1 FOR UPDATE",
        )
        .bind(dissent_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| StoreError::from_sqlx("promote select dissent", &e))?;
        let d_row =
            d_row.ok_or_else(|| StoreError::Other(format!("dissent {dissent_id} not found")))?;
        let status: String = d_row.try_get("status").map_err(map_decode)?;
        if status != "pending" {
            return Err(StoreError::Gone(format!(
                "dissent {dissent_id} already resolved (status={status})"
            )));
        }
        let fact_id: Uuid = d_row.try_get("fact_id").map_err(map_decode)?;
        let proposed_payload: serde_json::Value =
            d_row.try_get("proposed_payload").map_err(map_decode)?;
        let payload_hash: Vec<u8> = d_row.try_get("payload_hash").map_err(map_decode)?;

        let f_row = sqlx::query("SELECT version FROM facts WHERE id = $1 FOR UPDATE")
            .bind(fact_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|e| StoreError::from_sqlx("promote select fact", &e))?;
        let f_row = f_row
            .ok_or_else(|| StoreError::Other(format!("canonical fact {fact_id} not found")))?;
        let current_version: i32 = f_row.try_get("version").map_err(map_decode)?;
        if current_version != expected_version {
            return Err(StoreError::VersionConflict { current_version });
        }

        let row = sqlx::query(
            r"UPDATE facts SET
                payload = $2,
                payload_hash = $3,
                source = $4,
                version = version + 1,
                updated_at = now()
              WHERE id = $1
              RETURNING id, type, payload, version, source,
                        confidence, decay_weight, use_count, dissent_count,
                        last_used_at, created_at, updated_at",
        )
        .bind(fact_id)
        .bind(&proposed_payload)
        .bind(&payload_hash[..])
        .bind(caller_source.as_str())
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| StoreError::from_sqlx("promote update fact", &e))?;

        sqlx::query(
            "UPDATE dissents SET status='promoted', resolved_at=now(),
                                  resolved_by_source=$2
             WHERE id=$1",
        )
        .bind(dissent_id)
        .bind(caller_source.as_str())
        .execute(&mut *tx)
        .await
        .map_err(|e| StoreError::from_sqlx("promote update dissent", &e))?;

        let fact = row_to_fact(&row)?;
        tx.commit()
            .await
            .map_err(|e| StoreError::from_sqlx("promote commit", &e))?;
        Ok(fact)
    }

    /// Discard a pending dissent. Returns the resolved row.
    pub async fn discard_dissent(
        &self,
        dissent_id: Uuid,
        caller_source: Source,
    ) -> StoreResult<Dissent> {
        let row = sqlx::query(
            r"UPDATE dissents SET status='discarded', resolved_at=now(),
                                  resolved_by_source=$2
              WHERE id=$1 AND status='pending'
              RETURNING id, fact_id, proposed_payload, source, status,
                        submitted_at, last_seen_at, submission_count,
                        resolved_at, resolved_by_source,
                        reason, contradicting_memory_id, author_id",
        )
        .bind(dissent_id)
        .bind(caller_source.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| StoreError::from_sqlx("discard_dissent", &e))?;
        match row {
            Some(r) => row_to_dissent(&r),
            None => Err(StoreError::Gone(format!(
                "dissent {dissent_id} not pending"
            ))),
        }
    }

    /// Select up to `limit` facts past `after_id` ordered by `id ASC`,
    /// projecting (id, type, `age_seconds`) for the decay task to
    /// compute new weights against.
    pub async fn select_decay_batch(
        &self,
        after_id: Option<Uuid>,
        limit: u32,
    ) -> StoreResult<Vec<crate::DecayRow>> {
        let limit = i64::from(limit.clamp(1, 5_000));
        let after = after_id.unwrap_or(Uuid::nil());
        let rows = sqlx::query(
            r"SELECT id, type,
                     EXTRACT(EPOCH FROM (now() - COALESCE(last_used_at, created_at)))::float4
                         AS age_seconds
              FROM facts
              WHERE id > $1
                AND deleted_at IS NULL
              ORDER BY id ASC
              LIMIT $2",
        )
        .bind(after)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| StoreError::from_sqlx("select_decay_batch", &e))?;
        let mut out = Vec::with_capacity(rows.len());
        for r in &rows {
            let ft_str: String = r.try_get("type").map_err(map_decode)?;
            out.push(crate::DecayRow {
                id: r.try_get("id").map_err(map_decode)?,
                fact_type: parse_fact_type(&ft_str)?,
                age_seconds: r.try_get("age_seconds").map_err(map_decode)?,
            });
        }
        Ok(out)
    }

    /// Apply a batch of `(id, decay_weight)` updates in one round
    /// trip via `UPDATE … FROM UNNEST(...)`.
    ///
    /// Sprint 040 (#811): the `locked` CTE is load-bearing, not
    /// decoration. This and [`Self::apply_last_used_bumps`] are the two
    /// statements that lock many `facts` rows at once, and they run
    /// concurrently by design — the decay task fires hourly while the
    /// read path flushes `last_used_at` bumps for whatever searches
    /// returned. Neither statement pinned its lock order (that was the
    /// planner's choice, and a `FROM UNNEST` join and an
    /// `id = ANY(...)` scan do not share a plan shape), so an
    /// overlapping row set could be locked in opposite orders and
    /// deadlock — `40P01`, surfacing as a failed decay tick in
    /// production and as a flaky integration test in CI.
    ///
    /// `ORDER BY f.id … FOR UPDATE` takes every row lock up front in one
    /// agreed order. Both batch writers use the same discipline, so the
    /// cycle cannot form. Single-row writers are not part of this: one
    /// statement taking one lock can never be the party that holds A and
    /// waits for B.
    pub async fn apply_decay_batch(&self, updates: &[(Uuid, f32)]) -> StoreResult<u64> {
        if updates.is_empty() {
            return Ok(0);
        }
        let ids: Vec<Uuid> = updates.iter().map(|(i, _)| *i).collect();
        let weights: Vec<f32> = updates.iter().map(|(_, w)| *w).collect();
        let res = sqlx::query(
            r"WITH target AS (
                  SELECT u.id, u.w
                  FROM UNNEST($1::uuid[], $2::real[]) AS u(id, w)
              ),
              locked AS (
                  SELECT f.id
                  FROM facts f
                  JOIN target t ON t.id = f.id
                  ORDER BY f.id
                  FOR UPDATE
              )
              UPDATE facts AS f
              SET decay_weight = t.w
              FROM target t
              WHERE f.id = t.id
                AND f.id IN (SELECT id FROM locked)",
        )
        .bind(&ids[..])
        .bind(&weights[..])
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError::from_sqlx("apply_decay_batch", &e))?;
        Ok(res.rows_affected())
    }

    /// Coalesced `last_used_at` bumps. Increments `use_count` per
    /// flushed id (one increment per unique id per flush).
    ///
    /// Locks in `id` order — see [`Self::apply_decay_batch`] for why
    /// both batch writers must agree on one order (sprint 040, #811).
    pub async fn apply_last_used_bumps(&self, ids: &[Uuid]) -> StoreResult<u64> {
        if ids.is_empty() {
            return Ok(0);
        }
        let res = sqlx::query(
            r"WITH locked AS (
                  SELECT f.id
                  FROM facts f
                  WHERE f.id = ANY($1::uuid[])
                  ORDER BY f.id
                  FOR UPDATE
              )
              UPDATE facts
              SET last_used_at = now(),
                  use_count = use_count + 1
              WHERE id IN (SELECT id FROM locked)",
        )
        .bind(ids)
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError::from_sqlx("apply_last_used_bumps", &e))?;
        Ok(res.rows_affected())
    }
}

#[async_trait]
impl crate::DecayStore for PostgresStore {
    async fn select_decay_batch(
        &self,
        after_id: Option<Uuid>,
        limit: u32,
    ) -> StoreResult<Vec<crate::DecayRow>> {
        PostgresStore::select_decay_batch(self, after_id, limit).await
    }
    async fn apply_decay_batch(&self, updates: &[(Uuid, f32)]) -> StoreResult<u64> {
        PostgresStore::apply_decay_batch(self, updates).await
    }
    async fn apply_last_used_bumps(&self, ids: &[Uuid]) -> StoreResult<u64> {
        PostgresStore::apply_last_used_bumps(self, ids).await
    }
}

// ---------------------------------------------------------------------------
// Sprint 005 (T037) — SummaryStore impl.
// ---------------------------------------------------------------------------

#[async_trait]
impl crate::SummaryStore for PostgresStore {
    async fn upsert_event_summary(&self, summary: &klams_types::EventSummary) -> StoreResult<()> {
        sqlx::query(
            r"
            INSERT INTO summaries
              (id, kind, host, category, day_bucket, source_count,
               source_ids, summary_text, mechanism, generated_at,
               invalidated_at)
            VALUES ($1, 'event', $2, $3, $4, $5, $6, $7, $8, $9, $10)
            ON CONFLICT (kind, host, category, day_bucket) DO UPDATE SET
              source_count   = EXCLUDED.source_count,
              source_ids     = EXCLUDED.source_ids,
              summary_text   = EXCLUDED.summary_text,
              mechanism      = EXCLUDED.mechanism,
              generated_at   = EXCLUDED.generated_at,
              invalidated_at = NULL
            ",
        )
        .bind(summary.id)
        .bind(&summary.host)
        .bind(&summary.category)
        .bind(summary.day_bucket)
        .bind(i32::try_from(summary.source_count).unwrap_or(i32::MAX))
        .bind(&summary.source_ids)
        .bind(&summary.summary_text)
        .bind(summary.mechanism.as_str())
        .bind(summary.generated_at)
        .bind(summary.invalidated_at)
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError::from_sqlx("upsert_event_summary", &e))?;
        Ok(())
    }

    async fn invalidate_event_summaries(
        &self,
        host: &str,
        category: &str,
        day_bucket: time::Date,
    ) -> StoreResult<u64> {
        let res = sqlx::query(
            r"
            UPDATE summaries SET invalidated_at = now()
            WHERE kind = 'event' AND host = $1 AND category = $2
              AND day_bucket = $3 AND invalidated_at IS NULL
            ",
        )
        .bind(host)
        .bind(category)
        .bind(day_bucket)
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError::from_sqlx("invalidate_event_summaries", &e))?;
        Ok(res.rows_affected())
    }

    async fn get_event_summary(
        &self,
        host: &str,
        category: &str,
        day_bucket: time::Date,
    ) -> StoreResult<Option<klams_types::EventSummary>> {
        let row = sqlx::query(
            r"
            SELECT id, host, category, day_bucket, source_count,
                   source_ids, summary_text, mechanism,
                   generated_at, invalidated_at
            FROM summaries
            WHERE kind = 'event' AND host = $1 AND category = $2
              AND day_bucket = $3 AND invalidated_at IS NULL
            ",
        )
        .bind(host)
        .bind(category)
        .bind(day_bucket)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| StoreError::from_sqlx("get_event_summary", &e))?;
        row.as_ref().map(row_to_event_summary).transpose()
    }

    async fn list_event_summaries(
        &self,
        filters: &klams_types::RetrievalFilters,
        limit: u32,
    ) -> StoreResult<Vec<klams_types::EventSummary>> {
        let lim = i64::from(limit.max(1));
        let rows = sqlx::query(
            r"
            SELECT id, host, category, day_bucket, source_count,
                   source_ids, summary_text, mechanism,
                   generated_at, invalidated_at
            FROM summaries
            WHERE kind = 'event' AND invalidated_at IS NULL
              AND ($1::text IS NULL OR host = $1)
              AND ($2::timestamptz IS NULL OR generated_at >= $2)
              AND ($3::timestamptz IS NULL OR generated_at <= $3)
            ORDER BY day_bucket DESC, generated_at DESC
            LIMIT $4
            ",
        )
        .bind(filters.host.as_deref())
        .bind(filters.since)
        .bind(filters.until)
        .bind(lim)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| StoreError::from_sqlx("list_event_summaries", &e))?;
        rows.iter().map(row_to_event_summary).collect()
    }
}

// ---------- sprint 007: author registry + soft-delete ----------

/// Postgres-side result row for `list_authors_with_counts`.
#[derive(Debug, Clone)]
pub struct AuthorWithCounts {
    pub author: AuthorRecord,
    pub fact_count: i64,
    pub event_count: i64,
    /// Number of items this author has soft-deleted (rolls up
    /// `facts.deleted_by_author_id = author.id`). Knowledge points
    /// are not included in v1.
    pub soft_deletes_authored: i64,
    /// Number of times this author's items have been restored.
    /// Always `0` in v1 — restores are not yet audit-logged.
    pub restores_received: i64,
}

/// Postgres-side projection of a soft-deleted fact for the admin
/// `memory_admin_list_deleted` MCP tool. Carries the deletion bookkeeping
/// columns omitted from the public projection.
#[derive(Debug, Clone)]
pub struct DeletedFactRow {
    pub fact: Fact,
    pub deleted_at: time::OffsetDateTime,
    pub deleted_by_author_id: Option<Uuid>,
}

impl PostgresStore {
    /// Insert (or upsert by id) an author row. The id is generated as
    /// UUID v7 if not provided by the caller. On INSERT both
    /// `created_at` and `last_seen_at` are set to `now()`; on conflict
    /// the row is left untouched and the existing record is returned.
    pub async fn insert_author(
        &self,
        args: RegisterAuthorArgs,
        explicit_id: Option<Uuid>,
    ) -> StoreResult<AuthorRecord> {
        args.validate()
            .map_err(|e| StoreError::Other(format!("register_author: {e}")))?;
        let id = explicit_id.unwrap_or_else(Uuid::now_v7);
        let extra = if args.extra.is_null() {
            serde_json::json!({})
        } else {
            args.extra.clone()
        };
        let row = sqlx::query(
            r"
            INSERT INTO authors (
                id, agent_name, model, session_title, repo,
                client_app, client_version, extra
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            ON CONFLICT (id) DO UPDATE
                SET last_seen_at = now()
            RETURNING id, agent_name, model, session_title, repo,
                      client_app, client_version, extra,
                      created_at, last_seen_at
            ",
        )
        .bind(id)
        .bind(&args.agent_name)
        .bind(args.model.as_deref())
        .bind(args.session_title.as_deref())
        .bind(args.repo.as_deref())
        .bind(args.client_app.as_deref())
        .bind(args.client_version.as_deref())
        .bind(&extra)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| StoreError::from_sqlx("insert_author", &e))?;
        row_to_author(&row)
    }

    /// Look up a single author by id.
    pub async fn get_author_by_id(&self, id: Uuid) -> StoreResult<Option<AuthorRecord>> {
        let row = sqlx::query(
            r"SELECT id, agent_name, model, session_title, repo,
                     client_app, client_version, extra,
                     created_at, last_seen_at
              FROM authors WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| StoreError::from_sqlx("get_author_by_id", &e))?;
        row.map(|r| row_to_author(&r)).transpose()
    }

    /// Sprint 025 (#636) — every author row, oldest first. Unpaginated
    /// on purpose: the registry is small (44 rows at the time of
    /// writing) and the lifecycle tools need the whole set to spot
    /// duplicate `agent_name`s.
    pub async fn list_all_authors(&self) -> StoreResult<Vec<AuthorRecord>> {
        let rows = sqlx::query(
            r"SELECT id, agent_name, model, session_title, repo,
                     client_app, client_version, extra,
                     created_at, last_seen_at
              FROM authors ORDER BY created_at, id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| StoreError::from_sqlx("list_all_authors", &e))?;
        rows.iter().map(row_to_author).collect()
    }

    /// Sprint 009 — look up an author by `agent_name`. If multiple
    /// rows share the name (legacy data), returns the most recently
    /// touched one. Used by the REST bearer-binding resolver at
    /// service startup.
    pub async fn get_author_by_agent_name(
        &self,
        agent_name: &str,
    ) -> StoreResult<Option<AuthorRecord>> {
        let row = sqlx::query(
            r"SELECT id, agent_name, model, session_title, repo,
                     client_app, client_version, extra,
                     created_at, last_seen_at
              FROM authors
              WHERE agent_name = $1
              ORDER BY last_seen_at DESC
              LIMIT 1",
        )
        .bind(agent_name)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| StoreError::from_sqlx("get_author_by_agent_name", &e))?;
        row.map(|r| row_to_author(&r)).transpose()
    }

    /// Bulk-fetch facts joined with their authoring `AuthorRecord` by id.
    /// Soft-deleted facts are excluded. Returned order is unspecified;
    /// callers re-sort by their own score.
    pub async fn fetch_facts_with_authors(
        &self,
        ids: &[Uuid],
    ) -> StoreResult<Vec<(Fact, AuthorRecord)>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let rows = sqlx::query(
            r"SELECT f.id, f.type, f.payload, f.version, f.source, f.confidence,
                     f.decay_weight, f.use_count, f.last_used_at, f.created_at, f.updated_at,
                     a.id AS author_id, a.agent_name, a.model, a.session_title, a.repo,
                     a.client_app, a.client_version, a.extra,
                     a.created_at AS author_created_at, a.last_seen_at AS author_last_seen_at
              FROM facts f JOIN authors a ON a.id = f.author_id
              WHERE f.id = ANY($1) AND f.deleted_at IS NULL",
        )
        .bind(ids)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| StoreError::from_sqlx("fetch_facts_with_authors", &e))?;
        let mut out = Vec::with_capacity(rows.len());
        for r in &rows {
            let fact = row_to_fact(r)?;
            let author = row_to_author_prefixed(r)?;
            out.push((fact, author));
        }
        Ok(out)
    }

    /// Bulk-fetch events joined with their authoring `AuthorRecord` by id.
    pub async fn fetch_events_with_authors(
        &self,
        ids: &[Uuid],
    ) -> StoreResult<Vec<(Event, AuthorRecord)>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let rows = sqlx::query(
            r"SELECT e.id, e.task_id, e.category, e.payload, e.source, e.created_at,
                     a.id AS author_id, a.agent_name, a.model, a.session_title, a.repo,
                     a.client_app, a.client_version, a.extra,
                     a.created_at AS author_created_at, a.last_seen_at AS author_last_seen_at
              FROM events e JOIN authors a ON a.id = e.author_id
              WHERE e.id = ANY($1)",
        )
        .bind(ids)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| StoreError::from_sqlx("fetch_events_with_authors", &e))?;
        let mut out = Vec::with_capacity(rows.len());
        for r in &rows {
            let event = row_to_event(r)?;
            let author = row_to_author_prefixed(r)?;
            out.push((event, author));
        }
        Ok(out)
    }

    /// Bump `last_seen_at` on every authenticated MCP call that
    /// references this author (FR-005). Returns the number of rows
    /// updated (0 if the author has been hard-deleted out from under us).
    pub async fn touch_author_last_seen_at(&self, id: Uuid) -> StoreResult<u64> {
        let res = sqlx::query("UPDATE authors SET last_seen_at = now() WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| StoreError::from_sqlx("touch_author", &e))?;
        Ok(res.rows_affected())
    }

    /// Append a miss-log row (sprint 021, #317). Called fire-and-forget
    /// off the MCP search path, so callers ignore the result — a failed
    /// insert must never affect a live search.
    pub async fn insert_search_miss(&self, miss: &crate::SearchMiss) -> StoreResult<()> {
        sqlx::query(
            "INSERT INTO search_miss (query, caller, reason, top_score, hit_count, kinds) \
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(&miss.query)
        .bind(&miss.caller)
        .bind(&miss.reason)
        .bind(miss.top_score)
        .bind(miss.hit_count)
        .bind(&miss.kinds)
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError::from_sqlx("insert_search_miss", &e))?;
        Ok(())
    }

    /// Append an oversize-write row (sprint 027, #656). Called
    /// fire-and-forget off the knowledge write path, exactly like
    /// [`Self::insert_search_miss`] — a failed insert must never change
    /// the error the caller already earned.
    pub async fn insert_oversize_write(&self, w: &crate::OversizeWrite) -> StoreResult<()> {
        sqlx::query(
            "INSERT INTO oversize_write \
             (author_id, agent_name, submitted_chars, estimated_tokens, \
              limit_tokens, max_chars, text) \
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(w.author_id)
        .bind(&w.agent_name)
        .bind(w.submitted_chars)
        .bind(w.estimated_tokens)
        .bind(w.limit_tokens)
        .bind(w.max_chars)
        .bind(&w.text)
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError::from_sqlx("insert_oversize_write", &e))?;
        Ok(())
    }

    /// Drop oversize-write rows older than `max_age_days`, returning how
    /// many went (sprint 027, #656).
    ///
    /// This table is the one place klams retains rejected payloads in
    /// full, so unlike the miss log it does not leave retention purely to
    /// the operator — an unbounded log of whole documents is a liability
    /// rather than an instrument.
    pub async fn prune_oversize_writes(&self, max_age_days: i32) -> StoreResult<u64> {
        let res = sqlx::query(
            "DELETE FROM oversize_write \
             WHERE created_at < now() - make_interval(days => $1)",
        )
        .bind(max_age_days)
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError::from_sqlx("prune_oversize_writes", &e))?;
        Ok(res.rows_affected())
    }

    /// Append a search-sample row (sprint 026, #643). Called
    /// fire-and-forget off the MCP search path, exactly like
    /// [`Self::insert_search_miss`] — a failed insert must never affect a
    /// live search.
    pub async fn insert_search_sample(&self, s: &crate::SearchSample) -> StoreResult<()> {
        sqlx::query(
            "INSERT INTO search_sample \
             (query, caller, top_raw_score, top_kind, hit_count, kinds, duplicates_collapsed) \
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(&s.query)
        .bind(&s.caller)
        .bind(s.top_raw_score)
        .bind(&s.top_kind)
        .bind(s.hit_count)
        .bind(&s.kinds)
        .bind(s.duplicates_collapsed)
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError::from_sqlx("insert_search_sample", &e))?;
        Ok(())
    }

    /// List authors with per-author live-fact and live-event counts.
    /// Soft-deleted facts are excluded from `fact_count`; events have no
    /// soft-delete state.
    pub async fn list_authors_with_counts(&self, limit: u32) -> StoreResult<Vec<AuthorWithCounts>> {
        self.list_authors_with_counts_filtered(limit, None, None, None)
            .await
            .map(|(rows, _)| rows)
    }

    /// Filtered, paginated variant used by the REST `/v1/authors` route.
    /// Pagination orders by `(last_seen_at DESC, id DESC)`; pass the last
    /// row's `(last_seen_at, id)` as `cursor` to fetch the next page.
    pub async fn list_authors_with_counts_filtered(
        &self,
        limit: u32,
        since: Option<chrono::DateTime<chrono::Utc>>,
        agent_name: Option<&str>,
        cursor: Option<(chrono::DateTime<chrono::Utc>, Uuid)>,
    ) -> StoreResult<(
        Vec<AuthorWithCounts>,
        Option<(chrono::DateTime<chrono::Utc>, Uuid)>,
    )> {
        let limit = i64::from(limit.clamp(1, 1_000));
        let mut qb: sqlx::QueryBuilder<sqlx::Postgres> = sqlx::QueryBuilder::new(
            r"
            SELECT a.id, a.agent_name, a.model, a.session_title, a.repo,
                   a.client_app, a.client_version, a.extra,
                   a.created_at, a.last_seen_at,
                   COALESCE(fc.cnt, 0) AS fact_count,
                   COALESCE(ec.cnt, 0) AS event_count,
                   COALESCE(sd.cnt, 0) AS soft_deletes_authored
              FROM authors a
              LEFT JOIN (
                  SELECT author_id, COUNT(*)::bigint AS cnt
                    FROM facts WHERE deleted_at IS NULL
                   GROUP BY author_id
              ) fc ON fc.author_id = a.id
              LEFT JOIN (
                  SELECT author_id, COUNT(*)::bigint AS cnt
                    FROM events GROUP BY author_id
              ) ec ON ec.author_id = a.id
              LEFT JOIN (
                  SELECT deleted_by_author_id AS author_id, COUNT(*)::bigint AS cnt
                    FROM facts
                   WHERE deleted_at IS NOT NULL AND deleted_by_author_id IS NOT NULL
                   GROUP BY deleted_by_author_id
              ) sd ON sd.author_id = a.id
              WHERE 1 = 1
            ",
        );
        if let Some(t) = since {
            qb.push(" AND a.last_seen_at >= ").push_bind(t);
        }
        if let Some(name) = agent_name {
            qb.push(" AND a.agent_name = ").push_bind(name);
        }
        if let Some((ts, id)) = cursor {
            qb.push(" AND (a.last_seen_at, a.id) < (")
                .push_bind(ts)
                .push(", ")
                .push_bind(id)
                .push(")");
        }
        qb.push(" ORDER BY a.last_seen_at DESC, a.id DESC LIMIT ")
            .push_bind(limit);
        let rows = qb
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| StoreError::from_sqlx("list_authors_with_counts_filtered", &e))?;
        let mut out = Vec::with_capacity(rows.len());
        for r in &rows {
            out.push(AuthorWithCounts {
                author: row_to_author(r)?,
                fact_count: r.try_get("fact_count").map_err(map_decode)?,
                event_count: r.try_get("event_count").map_err(map_decode)?,
                soft_deletes_authored: r.try_get("soft_deletes_authored").map_err(map_decode)?,
                restores_received: 0,
            });
        }
        let next = if i64::try_from(out.len()).is_ok_and(|n| n == limit) {
            out.last().map(|a| (a.author.last_seen_at, a.author.id))
        } else {
            None
        };
        Ok((out, next))
    }

    /// Fetch a single author with the rolled-up counts used by
    /// `GET /v1/authors/{id}`. Returns `None` when the author id is
    /// unknown.
    pub async fn get_author_with_counts(&self, id: Uuid) -> StoreResult<Option<AuthorWithCounts>> {
        let row = sqlx::query(
            r"
            SELECT a.id, a.agent_name, a.model, a.session_title, a.repo,
                   a.client_app, a.client_version, a.extra,
                   a.created_at, a.last_seen_at,
                   COALESCE(fc.cnt, 0) AS fact_count,
                   COALESCE(ec.cnt, 0) AS event_count,
                   COALESCE(sd.cnt, 0) AS soft_deletes_authored
              FROM authors a
              LEFT JOIN (
                  SELECT author_id, COUNT(*)::bigint AS cnt
                    FROM facts WHERE deleted_at IS NULL
                   GROUP BY author_id
              ) fc ON fc.author_id = a.id
              LEFT JOIN (
                  SELECT author_id, COUNT(*)::bigint AS cnt
                    FROM events GROUP BY author_id
              ) ec ON ec.author_id = a.id
              LEFT JOIN (
                  SELECT deleted_by_author_id AS author_id, COUNT(*)::bigint AS cnt
                    FROM facts
                   WHERE deleted_at IS NOT NULL AND deleted_by_author_id IS NOT NULL
                   GROUP BY deleted_by_author_id
              ) sd ON sd.author_id = a.id
              WHERE a.id = $1
            ",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| StoreError::from_sqlx("get_author_with_counts", &e))?;
        let Some(r) = row else { return Ok(None) };
        Ok(Some(AuthorWithCounts {
            author: row_to_author(&r)?,
            fact_count: r.try_get("fact_count").map_err(map_decode)?,
            event_count: r.try_get("event_count").map_err(map_decode)?,
            soft_deletes_authored: r.try_get("soft_deletes_authored").map_err(map_decode)?,
            restores_received: 0,
        }))
    }

    /// List facts authored by `author_id` ordered by `(created_at DESC, id DESC)`.
    /// `state` selects live (default), deleted, or all. Pagination via
    /// `cursor = (created_at, id)`.
    pub async fn list_facts_by_author(
        &self,
        author_id: Uuid,
        state: AuthorMemoryState,
        limit: u32,
        cursor: Option<(time::OffsetDateTime, Uuid)>,
    ) -> StoreResult<(
        Vec<(Fact, Option<time::OffsetDateTime>, Option<Uuid>)>,
        Option<(time::OffsetDateTime, Uuid)>,
    )> {
        let limit = i64::from(limit.clamp(1, 500));
        let mut qb: sqlx::QueryBuilder<sqlx::Postgres> = sqlx::QueryBuilder::new(
            r"SELECT id, type, payload, version, source,
                     confidence, decay_weight, use_count,
                     last_used_at, created_at, updated_at,
                     deleted_at, deleted_by_author_id
              FROM facts WHERE author_id = ",
        );
        qb.push_bind(author_id);
        match state {
            AuthorMemoryState::Live => qb.push(" AND deleted_at IS NULL"),
            AuthorMemoryState::Deleted => qb.push(" AND deleted_at IS NOT NULL"),
            AuthorMemoryState::All => qb.push(""),
        };
        if let Some((ts, id)) = cursor {
            qb.push(" AND (created_at, id) < (")
                .push_bind(ts)
                .push(", ")
                .push_bind(id)
                .push(")");
        }
        qb.push(" ORDER BY created_at DESC, id DESC LIMIT ")
            .push_bind(limit);
        let rows = qb
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| StoreError::from_sqlx("list_facts_by_author", &e))?;
        let mut out = Vec::with_capacity(rows.len());
        for r in &rows {
            let fact = row_to_fact(r)?;
            let deleted_at: Option<time::OffsetDateTime> =
                r.try_get("deleted_at").map_err(map_decode)?;
            let deleted_by: Option<Uuid> = r.try_get("deleted_by_author_id").map_err(map_decode)?;
            out.push((fact, deleted_at, deleted_by));
        }
        let next = if i64::try_from(out.len()).is_ok_and(|n| n == limit) {
            out.last().map(|(f, _, _)| (f.created_at, f.id))
        } else {
            None
        };
        Ok((out, next))
    }

    /// List events authored by `author_id` ordered by `(created_at DESC, id DESC)`.
    pub async fn list_events_by_author(
        &self,
        author_id: Uuid,
        limit: u32,
        cursor: Option<(time::OffsetDateTime, Uuid)>,
    ) -> StoreResult<(Vec<Event>, Option<(time::OffsetDateTime, Uuid)>)> {
        let limit = i64::from(limit.clamp(1, 500));
        let mut qb: sqlx::QueryBuilder<sqlx::Postgres> = sqlx::QueryBuilder::new(
            r"SELECT id, task_id, category, payload, source, created_at
              FROM events WHERE author_id = ",
        );
        qb.push_bind(author_id);
        if let Some((ts, id)) = cursor {
            qb.push(" AND (created_at, id) < (")
                .push_bind(ts)
                .push(", ")
                .push_bind(id)
                .push(")");
        }
        qb.push(" ORDER BY created_at DESC, id DESC LIMIT ")
            .push_bind(limit);
        let rows = qb
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| StoreError::from_sqlx("list_events_by_author", &e))?;
        let mut out = Vec::with_capacity(rows.len());
        for r in &rows {
            out.push(row_to_event(r)?);
        }
        let next = if i64::try_from(out.len()).is_ok_and(|n| n == limit) {
            out.last().map(|e| (e.created_at, e.id))
        } else {
            None
        };
        Ok((out, next))
    }

    // Sprint 008 — cross-author paging for `GET /v1/memories` and
    // `event_search`. Authors empty ⇒ no author filter; window is
    // inclusive-exclusive on `created_at` (UTC).

    /// Page of facts across `authors` (empty ⇒ all) within
    /// `[since, until)`. Returns `(rows, next_cursor)` where each row
    /// carries the author UUID and soft-delete metadata so the composite
    /// layer can project it to the public wire shape.
    #[allow(clippy::too_many_arguments)]
    pub async fn list_memories_facts_page(
        &self,
        authors: &[Uuid],
        state: AuthorMemoryState,
        since: chrono::DateTime<chrono::Utc>,
        until: chrono::DateTime<chrono::Utc>,
        limit: u32,
        cursor: Option<(time::OffsetDateTime, Uuid)>,
    ) -> StoreResult<(
        Vec<(Fact, Uuid, Option<time::OffsetDateTime>, Option<Uuid>)>,
        Option<(time::OffsetDateTime, Uuid)>,
    )> {
        let limit = i64::from(limit.clamp(1, 500));
        let since_off = chrono_to_offset(since);
        let until_off = chrono_to_offset(until);
        let mut qb: sqlx::QueryBuilder<sqlx::Postgres> = sqlx::QueryBuilder::new(
            r"SELECT id, type, payload, version, source,
                     confidence, decay_weight, use_count,
                     last_used_at, created_at, updated_at,
                     deleted_at, deleted_by_author_id, author_id
              FROM facts WHERE created_at >= ",
        );
        qb.push_bind(since_off)
            .push(" AND created_at < ")
            .push_bind(until_off);
        if !authors.is_empty() {
            qb.push(" AND author_id = ANY(")
                .push_bind(authors.to_vec())
                .push(")");
        }
        match state {
            AuthorMemoryState::Live => qb.push(" AND deleted_at IS NULL"),
            AuthorMemoryState::Deleted => qb.push(" AND deleted_at IS NOT NULL"),
            AuthorMemoryState::All => qb.push(""),
        };
        if let Some((ts, id)) = cursor {
            qb.push(" AND (created_at, id) < (")
                .push_bind(ts)
                .push(", ")
                .push_bind(id)
                .push(")");
        }
        qb.push(" ORDER BY created_at DESC, id DESC LIMIT ")
            .push_bind(limit);
        let rows = qb
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| StoreError::from_sqlx("list_memories_facts_page", &e))?;
        let mut out = Vec::with_capacity(rows.len());
        for r in &rows {
            let fact = row_to_fact(r)?;
            let author_id: Uuid = r.try_get("author_id").map_err(map_decode)?;
            let deleted_at: Option<time::OffsetDateTime> =
                r.try_get("deleted_at").map_err(map_decode)?;
            let deleted_by: Option<Uuid> = r.try_get("deleted_by_author_id").map_err(map_decode)?;
            out.push((fact, author_id, deleted_at, deleted_by));
        }
        let next = if i64::try_from(out.len()).is_ok_and(|n| n == limit) {
            out.last().map(|(f, _, _, _)| (f.created_at, f.id))
        } else {
            None
        };
        Ok((out, next))
    }

    /// Page of events across `authors` (empty ⇒ all) within
    /// `[since, until)`. Events are never soft-deleted so no state
    /// parameter is exposed. Each row carries the author UUID so the
    /// composite layer can resolve `PublicAuthorRef` in bulk.
    pub async fn list_memories_events_page(
        &self,
        authors: &[Uuid],
        since: chrono::DateTime<chrono::Utc>,
        until: chrono::DateTime<chrono::Utc>,
        limit: u32,
        cursor: Option<(time::OffsetDateTime, Uuid)>,
    ) -> StoreResult<(Vec<(Event, Uuid)>, Option<(time::OffsetDateTime, Uuid)>)> {
        let limit = i64::from(limit.clamp(1, 500));
        let since_off = chrono_to_offset(since);
        let until_off = chrono_to_offset(until);
        let mut qb: sqlx::QueryBuilder<sqlx::Postgres> = sqlx::QueryBuilder::new(
            r"SELECT id, task_id, category, payload, source, created_at, author_id
              FROM events WHERE created_at >= ",
        );
        qb.push_bind(since_off)
            .push(" AND created_at < ")
            .push_bind(until_off);
        if !authors.is_empty() {
            qb.push(" AND author_id = ANY(")
                .push_bind(authors.to_vec())
                .push(")");
        }
        if let Some((ts, id)) = cursor {
            qb.push(" AND (created_at, id) < (")
                .push_bind(ts)
                .push(", ")
                .push_bind(id)
                .push(")");
        }
        qb.push(" ORDER BY created_at DESC, id DESC LIMIT ")
            .push_bind(limit);
        let rows = qb
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| StoreError::from_sqlx("list_memories_events_page", &e))?;
        let mut out = Vec::with_capacity(rows.len());
        for r in &rows {
            let event = row_to_event(r)?;
            let author_id: Uuid = r.try_get("author_id").map_err(map_decode)?;
            out.push((event, author_id));
        }
        let next = if i64::try_from(out.len()).is_ok_and(|n| n == limit) {
            out.last().map(|(e, _)| (e.created_at, e.id))
        } else {
            None
        };
        Ok((out, next))
    }

    /// Page of events for the `event_search` MCP tool. Supports
    /// category filtering, JSONB containment via `payload @>
    /// payload_match`, and ascending or descending order on
    /// `(created_at, id)`. Each row carries the author UUID for bulk
    /// `PublicAuthorRef` resolution.
    #[allow(clippy::too_many_arguments)]
    pub async fn event_search_page(
        &self,
        authors: &[Uuid],
        categories: &[String],
        since: Option<chrono::DateTime<chrono::Utc>>,
        until: Option<chrono::DateTime<chrono::Utc>>,
        payload_match: Option<&serde_json::Value>,
        limit: u32,
        ascending: bool,
        cursor: Option<(time::OffsetDateTime, Uuid)>,
    ) -> StoreResult<(Vec<(Event, Uuid)>, Option<(time::OffsetDateTime, Uuid)>)> {
        let limit = i64::from(limit.clamp(1, 500));
        let mut qb: sqlx::QueryBuilder<sqlx::Postgres> = sqlx::QueryBuilder::new(
            r"SELECT id, task_id, category, payload, source, created_at, author_id
              FROM events WHERE 1=1",
        );
        if let Some(ts) = since {
            qb.push(" AND created_at >= ")
                .push_bind(chrono_to_offset(ts));
        }
        if let Some(ts) = until {
            qb.push(" AND created_at < ")
                .push_bind(chrono_to_offset(ts));
        }
        if !authors.is_empty() {
            qb.push(" AND author_id = ANY(")
                .push_bind(authors.to_vec())
                .push(")");
        }
        if !categories.is_empty() {
            qb.push(" AND category = ANY(")
                .push_bind(categories.to_vec())
                .push(")");
        }
        if let Some(pm) = payload_match {
            qb.push(" AND payload @> ").push_bind(pm.clone());
        }
        if let Some((ts, id)) = cursor {
            let op = if ascending { " > " } else { " < " };
            qb.push(" AND (created_at, id)")
                .push(op)
                .push("(")
                .push_bind(ts)
                .push(", ")
                .push_bind(id)
                .push(")");
        }
        if ascending {
            qb.push(" ORDER BY created_at ASC, id ASC LIMIT ");
        } else {
            qb.push(" ORDER BY created_at DESC, id DESC LIMIT ");
        }
        qb.push_bind(limit);
        let rows = qb
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| StoreError::from_sqlx("event_search_page", &e))?;
        let mut out = Vec::with_capacity(rows.len());
        for r in &rows {
            let event = row_to_event(r)?;
            let author_id: Uuid = r.try_get("author_id").map_err(map_decode)?;
            out.push((event, author_id));
        }
        let next = if i64::try_from(out.len()).is_ok_and(|n| n == limit) {
            out.last().map(|(e, _)| (e.created_at, e.id))
        } else {
            None
        };
        Ok((out, next))
    }
}

/// Filter for `list_facts_by_author` / `list_knowledge_by_author`.
#[derive(Debug, Clone, Copy)]
pub enum AuthorMemoryState {
    Live,
    Deleted,
    All,
}

impl PostgresStore {
    /// Returns `true` if a fact row with `id` exists, regardless of
    /// its soft-delete state. Used by `memory_delete` to distinguish
    /// `NOT_FOUND` from "already soft-deleted" (FR-014).
    pub async fn fact_exists_any(&self, id: Uuid) -> StoreResult<bool> {
        let row = sqlx::query("SELECT 1 AS one FROM facts WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| StoreError::from_sqlx("fact_exists_any", &e))?;
        Ok(row.is_some())
    }

    /// Sprint 025 (#636) — how many Postgres rows attribute to
    /// `author_id`: `(facts, events, soft_deletes_authored)`. Facts and
    /// events count in **any** state; `soft_deletes_authored` counts
    /// rows this author deleted (someone else's memory included), which
    /// blocks removal too — dropping the row would erase the audit
    /// trail the delete recorded.
    pub async fn count_author_rows(&self, author_id: Uuid) -> StoreResult<(i64, i64, i64)> {
        let row = sqlx::query_as::<_, (i64, i64, i64)>(
            r"SELECT
                (SELECT count(*) FROM facts  WHERE author_id = $1),
                (SELECT count(*) FROM events WHERE author_id = $1),
                (SELECT count(*) FROM facts  WHERE deleted_by_author_id = $1)",
        )
        .bind(author_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| StoreError::from_sqlx("count_author_rows", &e))?;
        Ok(row)
    }

    /// Sprint 025 (#636) — delete an author row outright. Callers must
    /// have established that it owns nothing (see
    /// [`Self::count_author_rows`] and the Qdrant-side count); this is
    /// the unguarded primitive. Returns `false` if no such row existed.
    pub async fn delete_author(&self, author_id: Uuid) -> StoreResult<bool> {
        let res = sqlx::query("DELETE FROM authors WHERE id = $1")
            .bind(author_id)
            .execute(&self.pool)
            .await
            .map_err(|e| StoreError::from_sqlx("delete_author", &e))?;
        Ok(res.rows_affected() == 1)
    }

    /// Sprint 025 (#636) — repoint every Postgres row attributing to
    /// `from` at `into`, in one transaction, and drop the `from` row.
    /// Returns `(facts, events, soft_deletes)` moved.
    ///
    /// The Qdrant half of a merge has no transaction to join, so the
    /// caller runs it first: a failure there leaves this untouched and
    /// the whole merge re-runnable.
    pub async fn merge_author_rows(&self, from: Uuid, into: Uuid) -> StoreResult<(u64, u64, u64)> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| StoreError::from_sqlx("merge_author_rows begin", &e))?;
        let facts = sqlx::query("UPDATE facts SET author_id = $2 WHERE author_id = $1")
            .bind(from)
            .bind(into)
            .execute(&mut *tx)
            .await
            .map_err(|e| StoreError::from_sqlx("merge facts", &e))?
            .rows_affected();
        let events = sqlx::query("UPDATE events SET author_id = $2 WHERE author_id = $1")
            .bind(from)
            .bind(into)
            .execute(&mut *tx)
            .await
            .map_err(|e| StoreError::from_sqlx("merge events", &e))?
            .rows_affected();
        let deletes = sqlx::query(
            "UPDATE facts SET deleted_by_author_id = $2 WHERE deleted_by_author_id = $1",
        )
        .bind(from)
        .bind(into)
        .execute(&mut *tx)
        .await
        .map_err(|e| StoreError::from_sqlx("merge soft-deletes", &e))?
        .rows_affected();
        sqlx::query("DELETE FROM authors WHERE id = $1")
            .bind(from)
            .execute(&mut *tx)
            .await
            .map_err(|e| StoreError::from_sqlx("merge drop source author", &e))?;
        tx.commit()
            .await
            .map_err(|e| StoreError::from_sqlx("merge_author_rows commit", &e))?;
        Ok((facts, events, deletes))
    }

    /// Sprint 025 (#633) — the `author_id` that owns fact `id`, if the
    /// row exists at all (soft-deleted or not). `Ok(None)` means no such
    /// fact; it does **not** mean "unowned", since `facts.author_id` is
    /// NOT NULL. Used by `memory_delete` to enforce ownership.
    pub async fn fact_owner(&self, id: Uuid) -> StoreResult<Option<Uuid>> {
        let row = sqlx::query_scalar::<_, Uuid>("SELECT author_id FROM facts WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| StoreError::from_sqlx("fact_owner", &e))?;
        Ok(row)
    }

    /// Returns `true` if an event row with `id` exists.
    pub async fn event_exists(&self, id: Uuid) -> StoreResult<bool> {
        let row = sqlx::query("SELECT 1 AS one FROM events WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| StoreError::from_sqlx("event_exists", &e))?;
        Ok(row.is_some())
    }

    /// Soft-delete a single fact. Returns `false` if the fact was not
    /// found or was already soft-deleted.
    pub async fn soft_delete_fact(&self, id: Uuid, by_author_id: Uuid) -> StoreResult<bool> {
        let res = sqlx::query(
            r"UPDATE facts
                 SET deleted_at = now(),
                     deleted_by_author_id = $2
               WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(id)
        .bind(by_author_id)
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError::from_sqlx("soft_delete_fact", &e))?;
        Ok(res.rows_affected() == 1)
    }

    /// Restore a soft-deleted fact. The original `deleted_by_author_id`
    /// is cleared. Returns `false` if the fact was not soft-deleted.
    pub async fn restore_fact(&self, id: Uuid) -> StoreResult<bool> {
        let res = sqlx::query(
            r"UPDATE facts
                 SET deleted_at = NULL,
                     deleted_by_author_id = NULL
               WHERE id = $1 AND deleted_at IS NOT NULL",
        )
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError::from_sqlx("restore_fact", &e))?;
        Ok(res.rows_affected() == 1)
    }

    /// Permanently remove a fact row. Returns `false` if not found.
    pub async fn hard_delete_fact(&self, id: Uuid) -> StoreResult<bool> {
        let res = sqlx::query("DELETE FROM facts WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| StoreError::from_sqlx("hard_delete_fact", &e))?;
        Ok(res.rows_affected() == 1)
    }

    /// List soft-deleted facts (admin only) with deletion bookkeeping.
    pub async fn list_deleted_facts(&self, limit: u32) -> StoreResult<Vec<DeletedFactRow>> {
        let limit = i64::from(limit.clamp(1, 500));
        let rows = sqlx::query(
            r"SELECT id, type, payload, version, source,
                     confidence, decay_weight, use_count,
                     last_used_at, created_at, updated_at,
                     deleted_at, deleted_by_author_id
              FROM facts
              WHERE deleted_at IS NOT NULL
              ORDER BY deleted_at DESC, id
              LIMIT $1",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| StoreError::from_sqlx("list_deleted_facts", &e))?;
        let mut out = Vec::with_capacity(rows.len());
        for r in &rows {
            out.push(DeletedFactRow {
                fact: row_to_fact(r)?,
                deleted_at: r.try_get("deleted_at").map_err(map_decode)?,
                deleted_by_author_id: r.try_get("deleted_by_author_id").map_err(map_decode)?,
            });
        }
        Ok(out)
    }

    /// Filtered pagination over soft-deleted facts. Returns rows
    /// ordered by `(deleted_at DESC, id DESC)`; pass the last row's
    /// `(deleted_at, id)` as `cursor` to fetch the next page. Used by
    /// `memory_admin_list_deleted` (FR-013).
    pub async fn list_deleted_facts_filtered(
        &self,
        limit: u32,
        since: Option<time::OffsetDateTime>,
        author_id: Option<Uuid>,
        cursor: Option<(time::OffsetDateTime, Uuid)>,
    ) -> StoreResult<Vec<(DeletedFactRow, AuthorRecord)>> {
        let limit = i64::from(limit.clamp(1, 500));
        let mut qb: sqlx::QueryBuilder<sqlx::Postgres> = sqlx::QueryBuilder::new(
            "SELECT f.id, f.type, f.payload, f.version, f.source,
                    f.confidence, f.decay_weight, f.use_count,
                    f.last_used_at, f.created_at, f.updated_at,
                    f.deleted_at, f.deleted_by_author_id,
                    a.id AS author_id, a.agent_name, a.model, a.session_title, a.repo,
                    a.client_app, a.client_version, a.extra,
                    a.created_at AS author_created_at, a.last_seen_at AS author_last_seen_at
             FROM facts f JOIN authors a ON a.id = f.author_id
             WHERE f.deleted_at IS NOT NULL",
        );
        if let Some(t) = since {
            qb.push(" AND f.deleted_at >= ").push_bind(t);
        }
        if let Some(a) = author_id {
            qb.push(" AND f.deleted_by_author_id = ").push_bind(a);
        }
        if let Some((ts, id)) = cursor {
            qb.push(" AND (f.deleted_at, f.id) < (")
                .push_bind(ts)
                .push(", ")
                .push_bind(id)
                .push(")");
        }
        qb.push(" ORDER BY f.deleted_at DESC, f.id DESC LIMIT ")
            .push_bind(limit);
        let rows = qb
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| StoreError::from_sqlx("list_deleted_facts_filtered", &e))?;
        let mut out = Vec::with_capacity(rows.len());
        for r in &rows {
            let row = DeletedFactRow {
                fact: row_to_fact(r)?,
                deleted_at: r.try_get("deleted_at").map_err(map_decode)?,
                deleted_by_author_id: r.try_get("deleted_by_author_id").map_err(map_decode)?,
            };
            let author = row_to_author_prefixed(r)?;
            out.push((row, author));
        }
        Ok(out)
    }
}

fn row_to_author(row: &sqlx::postgres::PgRow) -> StoreResult<AuthorRecord> {
    use chrono::{DateTime, Utc};
    let created_at: DateTime<Utc> = row.try_get("created_at").map_err(map_decode)?;
    let last_seen_at: DateTime<Utc> = row.try_get("last_seen_at").map_err(map_decode)?;
    Ok(AuthorRecord {
        id: row.try_get("id").map_err(map_decode)?,
        agent_name: row.try_get("agent_name").map_err(map_decode)?,
        model: row.try_get("model").map_err(map_decode)?,
        session_title: row.try_get("session_title").map_err(map_decode)?,
        repo: row.try_get("repo").map_err(map_decode)?,
        client_app: row.try_get("client_app").map_err(map_decode)?,
        client_version: row.try_get("client_version").map_err(map_decode)?,
        extra: row.try_get("extra").map_err(map_decode)?,
        created_at,
        last_seen_at,
    })
}

/// Like `row_to_author` but reads the `a.*` columns under
/// `author_<col>` / `author_id` aliases used by the bulk-fetch joins.
fn row_to_author_prefixed(row: &sqlx::postgres::PgRow) -> StoreResult<AuthorRecord> {
    use chrono::{DateTime, Utc};
    let created_at: DateTime<Utc> = row.try_get("author_created_at").map_err(map_decode)?;
    let last_seen_at: DateTime<Utc> = row.try_get("author_last_seen_at").map_err(map_decode)?;
    Ok(AuthorRecord {
        id: row.try_get("author_id").map_err(map_decode)?,
        agent_name: row.try_get("agent_name").map_err(map_decode)?,
        model: row.try_get("model").map_err(map_decode)?,
        session_title: row.try_get("session_title").map_err(map_decode)?,
        repo: row.try_get("repo").map_err(map_decode)?,
        client_app: row.try_get("client_app").map_err(map_decode)?,
        client_version: row.try_get("client_version").map_err(map_decode)?,
        extra: row.try_get("extra").map_err(map_decode)?,
        created_at,
        last_seen_at,
    })
}

fn row_to_event_summary(r: &sqlx::postgres::PgRow) -> StoreResult<klams_types::EventSummary> {
    use klams_types::SummaryMechanism;
    let mechanism_s: String = r.try_get("mechanism").map_err(map_decode)?;
    let mechanism = match mechanism_s.as_str() {
        "extractive" => SummaryMechanism::Extractive,
        "llm" => SummaryMechanism::Llm,
        other => return Err(StoreError::Backend(format!("unknown mechanism: {other}"))),
    };
    let source_count_i32: i32 = r.try_get("source_count").map_err(map_decode)?;
    Ok(klams_types::EventSummary {
        id: r.try_get("id").map_err(map_decode)?,
        host: r.try_get("host").map_err(map_decode)?,
        category: r.try_get("category").map_err(map_decode)?,
        day_bucket: r.try_get("day_bucket").map_err(map_decode)?,
        source_count: u32::try_from(source_count_i32.max(0)).unwrap_or(0),
        source_ids: r.try_get("source_ids").map_err(map_decode)?,
        summary_text: r.try_get("summary_text").map_err(map_decode)?,
        mechanism,
        generated_at: r.try_get("generated_at").map_err(map_decode)?,
        invalidated_at: r.try_get("invalidated_at").map_err(map_decode)?,
    })
}

#[cfg(test)]
mod fts_tests {
    use super::*;
    use klams_types::Source;
    use serde_json::json;

    fn pg_url() -> Option<String> {
        std::env::var("TEST_DATABASE_URL").ok()
    }

    #[tokio::test]
    async fn empty_query_returns_error() {
        // Pure logic; no DB needed.
        let store = match pg_url() {
            Some(u) => PostgresStore::connect(&u, 2).await.expect("pg"),
            None => return, // skip silently if no db; covered by docker stack runs
        };
        let err = store.search_text("   ", 5).await.unwrap_err();
        assert!(matches!(err, StoreError::Other(_)));
    }

    #[tokio::test]
    #[ignore = "requires docker-compose.test.yml"]
    async fn fts_tie_breaks_deterministically_by_id() {
        let Some(url) = pg_url() else { return };
        let store = PostgresStore::connect(&url, 2).await.expect("pg");

        // Events have no payload-hash dedupe, so two distinct rows with
        // overlapping english lexemes will tie on ts_rank_cd and exercise
        // the deterministic ORDER BY ... id ASC tie-break.
        let task = uuid::Uuid::now_v7();
        for n in 0..3 {
            store
                .append_event(klams_types::AppendEvent {
                    id: uuid::Uuid::now_v7(),
                    task_id: Some(task),
                    category: "fts-tie-test".into(),
                    payload: json!({"note": "deterministic ordering matters", "n": n}),
                    source: Source::Controller,
                    author_id: klams_types::SYSTEM_AUTHOR_ID,
                })
                .await
                .expect("event");
        }

        let (_, a) = store
            .search_text("deterministic ordering", 50)
            .await
            .expect("search a");
        let (_, b) = store
            .search_text("deterministic ordering", 50)
            .await
            .expect("search b");
        let ids_a: Vec<_> = a.iter().map(|h| h.id).collect();
        let ids_b: Vec<_> = b.iter().map(|h| h.id).collect();
        assert_eq!(ids_a, ids_b, "FTS order must be deterministic across calls");
        assert!(
            ids_a.len() >= 3,
            "expected at least 3 tied hits, got {}",
            ids_a.len()
        );
        for w in a.windows(2) {
            if (w[0].score - w[1].score).abs() < f32::EPSILON {
                assert!(w[0].id <= w[1].id, "tied rows must be ordered by id ASC");
            }
        }
    }
}
