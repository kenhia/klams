//! Postgres adapter for facts and events.
//!
//! Uses runtime-checked `sqlx::query` queries; integration tests in
//! user-story phases validate every statement against a real Postgres.

use crate::{EventQuery, FactQuery, StoreError, StoreResult, TextHit};
use klams_types::{canonical_json_hash, AppendEvent, Event, Fact, FactType, Source, UpsertFact};
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
            qb.push(" AND task_id = ").push_bind(task_id);
        }
        if let Some(c) = q.category {
            qb.push(" AND category = ").push_bind(c);
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
                   ts_rank_cd(tsv, plainto_tsquery('english', $1)) AS score
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
