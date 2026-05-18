//! Worker pool draining the `MemoryQueue` into a `Store`.

use crate::queue::{WriteJob, WriteReply};
use klams_store::Store;
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

async fn process<S: Store>(job: WriteJob, store: &S) {
    let WriteJob { write, reply } = job;
    match write {
        MemoryWrite::UpsertFact(req) => {
            let result = store.upsert_fact(req).await;
            if let Some(tx) = reply {
                let _ = tx.send(WriteReply::Fact(result));
            } else if let Err(e) = result {
                error!(error = %e, "fact upsert failed (no reply channel)");
            }
        }
        MemoryWrite::AppendEvent(req) => {
            let result = store.append_event(req).await;
            if let Some(tx) = reply {
                let _ = tx.send(WriteReply::Event(result));
            } else if let Err(e) = result {
                error!(error = %e, "event append failed");
            }
        }
        MemoryWrite::IndexKnowledge(req) => {
            let result = store.index_knowledge(req).await;
            if let Some(tx) = reply {
                let _ = tx.send(WriteReply::Knowledge(result));
            } else if let Err(e) = result {
                error!(error = %e, "knowledge index failed");
            }
        }
    }
}
