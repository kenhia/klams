//! Worker pool draining the `MemoryQueue` into a `Store`.

use crate::metrics as m;
use crate::queue::{WriteJob, WriteReply};
use klams_store::{Store, StoreError};
use klams_types::MemoryWrite;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tokio::task::JoinHandle;
use tracing::{error, instrument};

/// Spawn `n` worker tasks pulling from `rx`. Returns their join
/// handles so the binary can await graceful shutdown.
#[must_use]
#[allow(clippy::needless_pass_by_value)] // matches builder ergonomics; store is cloned per worker
pub fn spawn_workers<S: Store>(
    n: usize,
    rx: mpsc::Receiver<WriteJob>,
    store: Arc<S>,
) -> Vec<JoinHandle<()>> {
    let rx = Arc::new(Mutex::new(rx));
    (0..n.max(1))
        .map(|i| {
            let rx = Arc::clone(&rx);
            let store = Arc::clone(&store);
            tokio::spawn(async move { worker_loop(i, rx, store).await })
        })
        .collect()
}

#[instrument(skip(rx, store), fields(worker_id = id))]
async fn worker_loop<S: Store>(id: usize, rx: Arc<Mutex<mpsc::Receiver<WriteJob>>>, store: Arc<S>) {
    loop {
        let job = {
            let mut guard = rx.lock().await;
            guard.recv().await
        };
        let Some(job) = job else { break };
        process(job, store.as_ref()).await;
    }
}

/// A single label for why a queued write died, so Grafana can break the
/// failure counter down by cause (sprint 027, #420).
fn failure_reason(e: &StoreError) -> &'static str {
    match e {
        StoreError::PayloadTooLarge { .. } => "too_large",
        StoreError::EmbeddingRejected(_) => "embedding_rejected",
        StoreError::Embedding(_) => "embedding_unavailable",
        StoreError::BackendUnavailable(_) => "backend_unavailable",
        StoreError::Conflict(_) | StoreError::VersionConflict { .. } => "conflict",
        StoreError::Gone(_) => "gone",
        StoreError::Backend(_) | StoreError::Other(_) => "backend",
    }
}

/// Record a queued write that failed with nobody listening.
///
/// Sprint 027 (#420): these are the writes that vanish. A job with no
/// reply channel has already been answered with `202 Accepted`, and the
/// scanner has already advanced its cursor — so a failure here is
/// permanent data loss, not a retryable error. It used to be logged and
/// nothing more: `writes_failed` was only ever touched by HTTP handlers,
/// so ~30k dropped chunks on kai never moved a single counter and
/// `/healthz` stayed green throughout.
fn record_drop(kind: &'static str, e: &StoreError, what: &str) {
    m::incr_writes_failed(kind, failure_reason(e));
    error!(
        error = %e,
        kind,
        reason = failure_reason(e),
        permanent = !e.is_transient(),
        "{what} — write dropped, no reply channel; this data is lost"
    );
}

async fn process<S: Store>(job: WriteJob, store: &S) {
    let WriteJob { write, reply } = job;
    match write {
        MemoryWrite::UpsertFact(req) => {
            let result = store.upsert_fact_v2(req).await;
            if let Some(tx) = reply {
                let _ = tx.send(WriteReply::Fact(result));
            } else if let Err(e) = result {
                record_drop("fact", &e, "fact upsert failed");
            }
        }
        MemoryWrite::AppendEvent(req) => {
            let result = store.append_event(req).await;
            if let Some(tx) = reply {
                let _ = tx.send(WriteReply::Event(result));
            } else if let Err(e) = result {
                record_drop("event", &e, "event append failed");
            }
        }
        MemoryWrite::IndexKnowledge(req) => {
            let result = store.index_knowledge(req).await;
            if let Some(tx) = reply {
                let _ = tx.send(WriteReply::Knowledge(Box::new(result)));
            } else if let Err(e) = result {
                record_drop("knowledge", &e, "knowledge index failed");
            }
        }
    }
}
