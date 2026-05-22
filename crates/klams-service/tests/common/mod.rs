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

use klams_api::{build_router, ApiState};
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
    pub store: Arc<TestStore>,
}

impl std::fmt::Debug for TestServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TestServer")
            .field("addr", &self.addr)
            .field("bearer_token", &"<redacted>")
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

        let postgres = PostgresStore::connect(&pg_url, 4)
            .await
            .expect("postgres connect");
        // Per-test Qdrant collection so parallel tests do not race.
        let collection = format!("knowledge_items_test_{}", uuid::Uuid::new_v4().simple());
        let qdrant = QdrantStore::connect(&qdrant_url, &collection, 384)
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
        };
        let router = build_router(state, bearer.clone());

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
            store,
        }
    }
}
