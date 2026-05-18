//! klams-service binary entrypoint.
//!
//! Loads runtime config, initializes logging, connects to Postgres /
//! Qdrant / TEI, spawns the worker pool, and serves the HTTP API.

pub mod config;
pub mod logging;

use anyhow::{Context, Result};
use klams_api::{build_router, with_metrics, ApiState};
use klams_core::{spawn_workers, MemoryQueue};
use klams_store::{CompositeStore, PostgresStore, QdrantStore, TeiEmbedder};
use std::sync::Arc;
use tokio::signal;
use tracing::info;

#[tokio::main]
async fn main() -> Result<()> {
    let config_path =
        std::env::var("KLAMS_CONFIG").unwrap_or_else(|_| "/ai/klams/config/klams.toml".to_string());
    let cfg = config::Config::from_path(&config_path)
        .with_context(|| format!("loading config from {config_path}"))?;

    logging::init(&cfg.logging.format, &cfg.logging.level);
    info!(config = %config_path, "klams-service starting");

    let postgres = PostgresStore::connect(&cfg.postgres.url, cfg.postgres.max_connections)
        .await
        .context("connecting to postgres")?;
    let qdrant = QdrantStore::connect(
        &cfg.qdrant.grpc_url,
        &cfg.qdrant.collection,
        u64::from(cfg.embeddings.vector_dim),
    )
    .await
    .context("connecting to qdrant")?;
    let embedder = TeiEmbedder::new(
        cfg.embeddings.url.clone(),
        cfg.embeddings.vector_dim as usize,
    )
    .context("building TEI client")?;
    let store = Arc::new(CompositeStore::new(postgres, qdrant, embedder));

    let (queue, rx) = MemoryQueue::new(cfg.queue.capacity);
    let _workers = spawn_workers(cfg.queue.workers, rx, Arc::clone(&store));

    let state = ApiState {
        store: Arc::clone(&store),
        queue,
        queue_capacity: cfg.queue.capacity,
        workers: cfg.queue.workers,
        started_at: std::time::Instant::now(),
    };
    let router = with_metrics(build_router(state, cfg.auth.bearer_token.clone()));
    klams_core::metrics::describe();

    let addr = format!("{}:{}", cfg.server.listen_addr, cfg.server.port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("bind {addr}"))?;
    info!(%addr, "listening");

    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("axum serve")?;

    info!("klams-service stopped");
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut sig) = signal::unix::signal(signal::unix::SignalKind::terminate()) {
            sig.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {}
        () = terminate => {}
    }
}
