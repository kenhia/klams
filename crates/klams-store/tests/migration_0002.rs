//! Migration 0002 (sprint 002 — dissents) schema-shape test.
//!
//! Runs the full migration set against a fresh database and asserts:
//! - `dissents` table exists with the expected columns.
//! - `facts.dissent_count` exists with default 0.
//! - The four sprint-002 indexes exist (`dissents_fact_id_idx`,
//!   `dissents_status_idx`, `dissents_pending_age_idx`,
//!   `dissents_pending_dedupe_idx`).
//! - The three sprint-002 triggers are registered in `pg_trigger`
//!   (`dissents_after_insert`, `dissents_after_status_update`,
//!   `facts_before_delete_orphan_dissents`).
//!
//! Uses a per-test schema (`mig0002_<uuid>`) so the test does not
//! interfere with concurrent integration tests on the same DB. The
//! schema is created, the migration is run inside it (via
//! `search_path`), assertions run, and the schema is dropped.

use sqlx::postgres::PgPoolOptions;
use sqlx::Row;
use uuid::Uuid;

fn test_db_url() -> String {
    std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://klams:klams_test@127.0.0.1:55432/klams".into())
}

#[tokio::test]
#[ignore = "requires `docker compose -f tests/docker-compose.test.yml up -d`"]
#[allow(clippy::too_many_lines)]
async fn migration_0002_installs_dissents_shape() {
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&test_db_url())
        .await
        .expect("connect");

    let schema = format!("mig0002_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA \"{schema}\""))
        .execute(&pool)
        .await
        .expect("create schema");

    // Scope a connection to the new schema and run migrations there.
    {
        let mut conn = pool.acquire().await.expect("acquire");
        sqlx::query(&format!("SET search_path TO \"{schema}\""))
            .execute(&mut *conn)
            .await
            .expect("set search_path");
        sqlx::migrate!("../../migrations")
            .run(&mut *conn)
            .await
            .expect("migrate");

        // facts.dissent_count exists with default 0.
        let row = sqlx::query(
            "SELECT column_default, is_nullable, data_type
               FROM information_schema.columns
              WHERE table_schema = $1
                AND table_name = 'facts'
                AND column_name = 'dissent_count'",
        )
        .bind(&schema)
        .fetch_one(&mut *conn)
        .await
        .expect("facts.dissent_count exists");
        let default: Option<String> = row.try_get("column_default").unwrap();
        let is_nullable: String = row.try_get("is_nullable").unwrap();
        let data_type: String = row.try_get("data_type").unwrap();
        assert_eq!(is_nullable, "NO");
        assert_eq!(data_type, "integer");
        assert!(default.as_deref().unwrap_or("").starts_with('0'));

        // dissents table exists with expected columns.
        let cols: Vec<(String, String)> = sqlx::query(
            "SELECT column_name, data_type
               FROM information_schema.columns
              WHERE table_schema = $1 AND table_name = 'dissents'
              ORDER BY ordinal_position",
        )
        .bind(&schema)
        .fetch_all(&mut *conn)
        .await
        .expect("dissents columns")
        .into_iter()
        .map(|r| {
            (
                r.try_get::<String, _>("column_name").unwrap(),
                r.try_get::<String, _>("data_type").unwrap(),
            )
        })
        .collect();
        let names: Vec<&str> = cols.iter().map(|(n, _)| n.as_str()).collect();
        for required in [
            "id",
            "fact_id",
            "proposed_payload",
            "payload_hash",
            "source",
            "submitted_at",
            "last_seen_at",
            "submission_count",
            "status",
            "resolved_at",
            "resolved_by_source",
        ] {
            assert!(
                names.contains(&required),
                "dissents missing column {required}; have {names:?}"
            );
        }

        // Four sprint-002 indexes exist on dissents.
        let idx_rows = sqlx::query(
            "SELECT indexname FROM pg_indexes
              WHERE schemaname = $1 AND tablename = 'dissents'",
        )
        .bind(&schema)
        .fetch_all(&mut *conn)
        .await
        .expect("dissents indexes");
        let idx_names: Vec<String> = idx_rows
            .iter()
            .map(|r| r.try_get::<String, _>("indexname").unwrap())
            .collect();
        for required in [
            "dissents_fact_id_idx",
            "dissents_status_idx",
            "dissents_pending_age_idx",
            "dissents_pending_dedupe_idx",
        ] {
            assert!(
                idx_names.iter().any(|n| n == required),
                "missing index {required}; have {idx_names:?}"
            );
        }

        // Three sprint-002 triggers exist.
        let trg_rows = sqlx::query(
            "SELECT t.tgname
               FROM pg_trigger t
               JOIN pg_class c ON c.oid = t.tgrelid
               JOIN pg_namespace n ON n.oid = c.relnamespace
              WHERE n.nspname = $1 AND NOT t.tgisinternal",
        )
        .bind(&schema)
        .fetch_all(&mut *conn)
        .await
        .expect("triggers");
        let trg_names: Vec<String> = trg_rows
            .iter()
            .map(|r| r.try_get::<String, _>("tgname").unwrap())
            .collect();
        for required in [
            "dissents_after_insert",
            "dissents_after_status_update",
            "facts_before_delete_orphan_dissents",
        ] {
            assert!(
                trg_names.iter().any(|n| n == required),
                "missing trigger {required}; have {trg_names:?}"
            );
        }
    }

    sqlx::query(&format!("DROP SCHEMA \"{schema}\" CASCADE"))
        .execute(&pool)
        .await
        .expect("drop schema");
}
