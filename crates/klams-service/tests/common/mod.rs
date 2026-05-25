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
    pub store: Arc<TestStore>,
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
            .finish()
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
        let qdrant = QdrantStore::connect(&qdrant_url, "knowledge_items_test", 384)
            .await
            .expect("qdrant connect");
        let embedder = TeiEmbedder::new(tei_url, 384).expect("tei client");
        let store = Arc::new(CompositeStore::new(postgres, qdrant, embedder));

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
            ];
            let auth_state = klams_api::auth::AuthState::with_grants(grants.clone());
            let mcp_grants = std::sync::Arc::new(grants);
            let mcp_state = klams_mcp::tools::McpState::new(
                Arc::clone(&store),
                std::sync::Arc::new(klams_types::MaintenanceState::default()),
                mcp_grants,
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
            store,
        }
    }
}
