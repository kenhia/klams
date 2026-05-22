//! Postgres adapter for facts and events.
//!
//! Uses runtime-checked `sqlx::query` queries; integration tests in
//! user-story phases validate every statement against a real Postgres.

use crate::{DissentQuery, EventQuery, FactQuery, StoreError, StoreResult, TextHit};
use async_trait::async_trait;
use klams_types::{
    canonical_json_hash, AppendEvent, Dissent, DissentStatus, Event, Fact, FactType,
    FactWriteOutcome, Source, UpsertFact,
};
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};
use uuid::Uuid;

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
            .map_err(|e| StoreError::Backend(format!("connect: {e}")))?;
        sqlx::migrate!("../../migrations")
            .run(&pool)
            .await
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
            .map_err(|e| StoreError::Backend(format!("pg health: {e}")))
    }

    pub async fn upsert_fact(&self, req: UpsertFact) -> StoreResult<Fact> {
        let hash = canonical_json_hash(req.fact_type.as_str(), &req.payload);
        let id = req.explicit_id.unwrap_or_else(Uuid::now_v7);

        let row = sqlx::query(
            r"
            INSERT INTO facts (id, type, payload, payload_hash, source, version)
            VALUES ($1, $2, $3, $4, $5, 1)
            ON CONFLICT (type, payload_hash) DO UPDATE
                SET updated_at = now(),
                    version = facts.version + CASE
                        WHEN facts.payload <> EXCLUDED.payload THEN 1
                        ELSE 0
                    END,
                    payload = EXCLUDED.payload
            RETURNING
                id, type, payload, version, source,
                confidence, decay_weight, use_count,
                last_used_at, created_at, updated_at
            ",
        )
        .bind(id)
        .bind(req.fact_type.as_str())
        .bind(&req.payload)
        .bind(&hash[..])
        .bind(req.source.as_str())
        .fetch_one(&self.pool)
        .await
        .map_err(|e| StoreError::Backend(format!("upsert_fact: {e}")))?;
        row_to_fact(&row)
    }

    pub async fn append_event(&self, req: AppendEvent) -> StoreResult<Event> {
        let row = sqlx::query(
            r"
            INSERT INTO events (id, task_id, category, payload, source)
            VALUES ($1, $2, $3, $4, $5)
            RETURNING id, task_id, category, payload, source, created_at
            ",
        )
        .bind(req.id)
        .bind(req.task_id)
        .bind(&req.category)
        .bind(&req.payload)
        .bind(req.source.as_str())
        .fetch_one(&self.pool)
        .await
        .map_err(|e| StoreError::Backend(format!("append_event: {e}")))?;
        row_to_event(&row)
    }

    pub async fn list_facts(&self, q: FactQuery) -> StoreResult<(Vec<Fact>, Option<String>)> {
        let limit = i64::from(q.limit.clamp(1, 500));
        let mut qb: sqlx::QueryBuilder<sqlx::Postgres> = sqlx::QueryBuilder::new(
            "SELECT id, type, payload, version, source, confidence, decay_weight,
             use_count, last_used_at, created_at, updated_at FROM facts WHERE 1=1",
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
            .map_err(|e| StoreError::Backend(format!("list_facts: {e}")))?;
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
            .map_err(|e| StoreError::Backend(format!("list_events: {e}")))?;
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
            ORDER BY score DESC, id ASC LIMIT $2
            ",
        )
        .bind(query)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| StoreError::Backend(format!("search_text(facts): {e}")))?;

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
        .map_err(|e| StoreError::Backend(format!("search_text(events): {e}")))?;

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
    StoreError::Backend(format!("decode: {e}"))
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
            .map_err(|e| StoreError::Backend(format!("upsert_fact_v2 begin: {e}")))?;

        // 1) Same (type, payload_hash) idempotent path.
        let existing_same: Option<(Uuid, i32, String)> = sqlx::query_as(
            "SELECT id, version, source FROM facts WHERE type=$1 AND payload_hash=$2 FOR UPDATE",
        )
        .bind(req.fact_type.as_str())
        .bind(&hash[..])
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| StoreError::Backend(format!("upsert_fact_v2 select-same: {e}")))?;

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
            .map_err(|e| StoreError::Backend(format!("upsert_fact_v2 touch: {e}")))?;
            let fact = row_to_fact(&row)?;
            tx.commit()
                .await
                .map_err(|e| StoreError::Backend(format!("upsert_fact_v2 commit: {e}")))?;
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
                    .map_err(|e| StoreError::Backend(format!("upsert_fact_v2 select-id: {e}")))?;
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
                    .map_err(|e| {
                        StoreError::Backend(format!("upsert_fact_v2 dissent insert: {e}"))
                    })?;
                    let dissent_id: Uuid = row.try_get("id").map_err(map_decode)?;
                    tx.commit()
                        .await
                        .map_err(|e| StoreError::Backend(format!("upsert_fact_v2 commit: {e}")))?;
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
                .map_err(|e| StoreError::Backend(format!("upsert_fact_v2 amend: {e}")))?;
                let fact = row_to_fact(&row)?;
                tx.commit()
                    .await
                    .map_err(|e| StoreError::Backend(format!("upsert_fact_v2 commit: {e}")))?;
                return Ok(FactWriteOutcome::Persisted { fact });
            }
        }

        // 3) Brand new canonical fact.
        let id = req.explicit_id.unwrap_or_else(Uuid::now_v7);
        let row = sqlx::query(
            r"INSERT INTO facts (id, type, payload, payload_hash, source, version)
              VALUES ($1, $2, $3, $4, $5, 1)
              RETURNING id, type, payload, version, source,
                        confidence, decay_weight, use_count, dissent_count,
                        last_used_at, created_at, updated_at",
        )
        .bind(id)
        .bind(req.fact_type.as_str())
        .bind(&req.payload)
        .bind(&hash[..])
        .bind(req.source.as_str())
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| StoreError::Backend(format!("upsert_fact_v2 insert: {e}")))?;
        let fact = row_to_fact(&row)?;
        tx.commit()
            .await
            .map_err(|e| StoreError::Backend(format!("upsert_fact_v2 commit: {e}")))?;
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
                    resolved_at, resolved_by_source
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
            .map_err(|e| StoreError::Backend(format!("list_dissents: {e}")))?;
        let mut items = Vec::with_capacity(rows.len());
        for r in &rows {
            items.push(row_to_dissent(r)?);
        }
        Ok((items, q.cursor))
    }

    pub async fn get_dissent(&self, id: Uuid) -> StoreResult<Option<Dissent>> {
        let row = sqlx::query(
            "SELECT id, fact_id, proposed_payload, source, status,
                    submitted_at, last_seen_at, submission_count,
                    resolved_at, resolved_by_source
             FROM dissents WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| StoreError::Backend(format!("get_dissent: {e}")))?;
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
            .map_err(|e| StoreError::Backend(format!("promote begin: {e}")))?;

        let d_row = sqlx::query(
            "SELECT id, fact_id, proposed_payload, payload_hash, source, status
             FROM dissents WHERE id = $1 FOR UPDATE",
        )
        .bind(dissent_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| StoreError::Backend(format!("promote select dissent: {e}")))?;
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
            .map_err(|e| StoreError::Backend(format!("promote select fact: {e}")))?;
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
        .map_err(|e| StoreError::Backend(format!("promote update fact: {e}")))?;

        sqlx::query(
            "UPDATE dissents SET status='promoted', resolved_at=now(),
                                  resolved_by_source=$2
             WHERE id=$1",
        )
        .bind(dissent_id)
        .bind(caller_source.as_str())
        .execute(&mut *tx)
        .await
        .map_err(|e| StoreError::Backend(format!("promote update dissent: {e}")))?;

        let fact = row_to_fact(&row)?;
        tx.commit()
            .await
            .map_err(|e| StoreError::Backend(format!("promote commit: {e}")))?;
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
                        resolved_at, resolved_by_source",
        )
        .bind(dissent_id)
        .bind(caller_source.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| StoreError::Backend(format!("discard_dissent: {e}")))?;
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
              ORDER BY id ASC
              LIMIT $2",
        )
        .bind(after)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| StoreError::Backend(format!("select_decay_batch: {e}")))?;
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
    pub async fn apply_decay_batch(&self, updates: &[(Uuid, f32)]) -> StoreResult<u64> {
        if updates.is_empty() {
            return Ok(0);
        }
        let ids: Vec<Uuid> = updates.iter().map(|(i, _)| *i).collect();
        let weights: Vec<f32> = updates.iter().map(|(_, w)| *w).collect();
        let res = sqlx::query(
            r"UPDATE facts AS f
              SET decay_weight = u.w
              FROM UNNEST($1::uuid[], $2::real[]) AS u(id, w)
              WHERE f.id = u.id",
        )
        .bind(&ids[..])
        .bind(&weights[..])
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError::Backend(format!("apply_decay_batch: {e}")))?;
        Ok(res.rows_affected())
    }

    /// Coalesced `last_used_at` bumps. Increments `use_count` per
    /// flushed id (one increment per unique id per flush).
    pub async fn apply_last_used_bumps(&self, ids: &[Uuid]) -> StoreResult<u64> {
        if ids.is_empty() {
            return Ok(0);
        }
        let res = sqlx::query(
            r"UPDATE facts
              SET last_used_at = now(),
                  use_count = use_count + 1
              WHERE id = ANY($1::uuid[])",
        )
        .bind(ids)
        .execute(&self.pool)
        .await
        .map_err(|e| StoreError::Backend(format!("apply_last_used_bumps: {e}")))?;
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
        .map_err(|e| StoreError::Backend(format!("upsert_event_summary: {e}")))?;
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
        .map_err(|e| StoreError::Backend(format!("invalidate_event_summaries: {e}")))?;
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
        .map_err(|e| StoreError::Backend(format!("get_event_summary: {e}")))?;
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
        .map_err(|e| StoreError::Backend(format!("list_event_summaries: {e}")))?;
        rows.iter().map(row_to_event_summary).collect()
    }
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
