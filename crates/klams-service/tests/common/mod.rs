//! Shared integration-test harness.
//!
//! Boots klams-service in-process against the local docker-compose
//! test stack, returning a configured `klams_client::Client` that
//! integration tests can drive.
//!
//! Activate by running `docker compose -f tests/docker-compose.test.yml up -d`
//! and setting environment variables:
//!   `TEST_DATABASE_URL`  (default: <postgres://klams:klams_test@127.0.0.1:55432/klams>)
//!   `TEST_QDRANT_URL`    (default: <http://127.0.0.1:56334>)
//!   `TEST_TEI_URL`       (default: <http://127.0.0.1:57070>)
//!
//! Tests that depend on this harness should be marked `#[ignore]`
//! by default and run explicitly via `cargo test -- --ignored`.

#![allow(dead_code)]

use klams_api::{build_router_with_auth, ApiState};
use klams_client::Client;
use klams_core::{spawn_workers, MemoryQueue};
use klams_store::{CompositeStore, PostgresStore, QdrantStore, TeiEmbedder};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use uuid::Uuid;

pub mod fixture;
pub mod seed;

pub type TestStore = CompositeStore;

pub struct TestServer {
    pub client: Client,
    pub addr: SocketAddr,
    pub bearer_token: String,
    /// Read-only scope token (sprint 007 T066).
    pub read_token: String,
    /// Write+Read scope token (sprint 007 T066).
    pub write_token: String,
    /// Token bound to a named author (sprint 018 / WI #62) — writes
    /// through it may omit `author_id`.
    pub author_token: String,
    /// The `agent_name` the author-bound token attributes writes to.
    pub author_agent_name: String,
    /// The author id `author_token` is bound to.
    pub bound_author_id: Uuid,
    pub store: Arc<TestStore>,
    /// gRPC Qdrant URL — retained so `cleanup()` can drop the
    /// per-test collection on teardown (sprint 009 T039 / FR-021).
    qdrant_url: String,
    /// Qdrant collection name this server bound to. For
    /// `spawn_isolated` this is `klams_test_{uuid}`; for the shared
    /// helpers it is `knowledge_items_test`.
    qdrant_collection: String,
}

impl std::fmt::Debug for TestServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TestServer")
            .field("addr", &self.addr)
            .field("bearer_token", &"<redacted>")
            .field("read_token", &"<redacted>")
            .field("write_token", &"<redacted>")
            .field("client", &"<klams_client>")
            .field("store", &"<CompositeStore>")
            .finish_non_exhaustive()
    }
}

impl TestServer {
    pub async fn spawn() -> Self {
        Self::spawn_with_summary_store(false).await
    }

    /// Like `spawn`, but wires `SummaryStore` into `ContextBuilder`
    /// so the events section can substitute raw rows for summaries
    /// (sprint 005 T039 + T033).
    pub async fn spawn_with_summary_store(with_summary_store: bool) -> Self {
        Self::spawn_inner(
            with_summary_store,
            "knowledge_items_test".to_string(),
            false,
        )
        .await
    }

    /// Sprint 009 T039 (FR-021 / SC-008) — per-test isolation.
    /// Creates an ephemeral Qdrant collection `klams_test_{uuid}`
    /// (dropped via [`Self::cleanup`]) and TRUNCATEs the shared
    /// Postgres test tables so concurrent tests cannot observe
    /// each other's facts/events/summaries/dissents/authors.
    /// Seeded `system` + `lost-author` authors are preserved.
    pub async fn spawn_isolated() -> Self {
        let collection = format!("klams_test_{}", Uuid::new_v4().simple());
        Self::spawn_inner(false, collection, true).await
    }

    #[allow(clippy::too_many_lines)]
    async fn spawn_inner(
        with_summary_store: bool,
        qdrant_collection: String,
        truncate_postgres: bool,
    ) -> Self {
        let pg_url = std::env::var("TEST_DATABASE_URL")
            .unwrap_or_else(|_| "postgres://klams:klams_test@127.0.0.1:55432/klams".into());
        let qdrant_url =
            std::env::var("TEST_QDRANT_URL").unwrap_or_else(|_| "http://127.0.0.1:56334".into());
        let tei_url =
            std::env::var("TEST_TEI_URL").unwrap_or_else(|_| "http://127.0.0.1:57070".into());
        let bearer = "test-token-do-not-use-in-prod".to_string();
        let read_token = "test-token-read-only".to_string();
        let write_token = "test-token-write".to_string();

        let postgres = PostgresStore::connect(&pg_url, 4)
            .await
            .expect("postgres connect");
        if truncate_postgres {
            // Wipe per-test mutable state. `authors` keeps the two
            // seeded identities ('system', 'lost-author') so the
            // re-attribution invariants still hold.
            sqlx::query(
                "TRUNCATE TABLE facts, events, summaries, dissents RESTART IDENTITY CASCADE",
            )
            .execute(postgres.pool())
            .await
            .expect("truncate postgres");
            sqlx::query("DELETE FROM authors WHERE agent_name NOT IN ('system', 'lost-author')")
                .execute(postgres.pool())
                .await
                .expect("prune authors");
        }
        let qdrant = QdrantStore::connect(&qdrant_url, &qdrant_collection, 384)
            .await
            .expect("qdrant connect");
        let embedder = Arc::new(TeiEmbedder::new(tei_url, 384).expect("tei client"));
        let store = Arc::new(CompositeStore::new(postgres, qdrant, embedder));

        // Sprint 018 (WI #62) — mirror main.rs's resolve_token_author:
        // bind a test token to a named author so bearer-fallback
        // attribution is exercisable.
        let author_token = "test-token-author-bound".to_string();
        let author_agent_name = "bearer-bound-test-agent".to_string();
        let existing = store
            .postgres
            .get_author_by_agent_name(&author_agent_name)
            .await
            .expect("lookup bound author");
        let bound_author_id = if let Some(a) = existing {
            a.id
        } else {
            let args = klams_types::RegisterAuthorArgs {
                agent_name: author_agent_name.clone(),
                model: None,
                session_title: Some("test harness".into()),
                repo: None,
                client_app: Some("klams-service-tests".into()),
                client_version: None,
                extra: serde_json::json!({}),
            };
            store
                .postgres
                .insert_author(args, None)
                .await
                .expect("insert bound author")
                .id
        };

        let (queue, rx) = MemoryQueue::new(256);
        let _workers = spawn_workers(2, rx, Arc::clone(&store));

        let mut builder = klams_core::context::ContextBuilder::new(
            klams_core::tokens::TokenCounter::new(klams_core::tokens::TokenMode::CharsDiv4),
            100,
        );
        if with_summary_store {
            builder = builder
                .with_summary_store(Arc::clone(&store) as Arc<dyn klams_store::SummaryStore>);
        }

        let state = ApiState {
            store: Arc::clone(&store),
            api: klams_types::ApiConfig::default(),
            queue,
            queue_capacity: 256,
            workers: 2,
            started_at: std::time::Instant::now(),
            validators: Arc::new(klams_core::ValidatorRegistry::with_defaults()),
            context_builder: Arc::new(builder),
            maintenance: klams_types::MaintenanceState::default(),
        };
        let router = {
            // Mirror main.rs's wiring: single AuthState gates both REST
            // and the nested /mcp router. Sprint 007 T064.
            let grants = vec![
                klams_api::auth::TokenGrant::new(
                    bearer.clone(),
                    vec![
                        klams_types::Scope::Read,
                        klams_types::Scope::Write,
                        klams_types::Scope::Admin,
                    ],
                    Some("legacy".into()),
                ),
                klams_api::auth::TokenGrant::new(
                    read_token.clone(),
                    vec![klams_types::Scope::Read],
                    Some("read-only".into()),
                ),
                klams_api::auth::TokenGrant::new(
                    write_token.clone(),
                    vec![klams_types::Scope::Read, klams_types::Scope::Write],
                    Some("write".into()),
                ),
                klams_api::auth::TokenGrant::new_with_author(
                    author_token.clone(),
                    vec![klams_types::Scope::Read, klams_types::Scope::Write],
                    Some("author-bound".into()),
                    bound_author_id,
                    author_agent_name.clone(),
                ),
            ];
            let auth_state = klams_api::auth::AuthState::with_grants(grants);
            let mcp_state = klams_mcp::tools::McpState::new(
                Arc::clone(&store),
                std::sync::Arc::new(klams_types::MaintenanceState::default()),
                klams_types::ApiConfig::default(),
            );
            let mcp_router = klams_mcp::router(mcp_state, Vec::new()).layer(
                axum::middleware::from_fn_with_state(auth_state.clone(), klams_api::require_bearer),
            );
            build_router_with_auth(state, auth_state).nest("/mcp", mcp_router)
        };

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local_addr");
        tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });

        let client = Client::new(&format!("http://{addr}"), &bearer).expect("client");
        Self {
            client,
            addr,
            bearer_token: bearer,
            read_token,
            write_token,
            author_token,
            author_agent_name,
            bound_author_id,
            store,
            qdrant_url,
            qdrant_collection,
        }
    }

    /// Sprint 009 T039 — drop the per-test Qdrant collection.
    /// Safe to call on a `spawn()`-built server too; it will skip
    /// the shared `knowledge_items_test` collection.
    pub async fn cleanup(self) {
        if self.qdrant_collection == "knowledge_items_test" {
            return;
        }
        let client = qdrant_client::Qdrant::from_url(&self.qdrant_url)
            .build()
            .expect("qdrant client for cleanup");
        let _ = client.delete_collection(self.qdrant_collection).await;
    }
}
