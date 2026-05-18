//! Bounded mpsc job queue feeding the worker pool.
//!
//! Producers (HTTP handlers) call `try_enqueue`. For writes that must
//! return canonical persisted state to the client (e.g. fact upserts),
//! the producer attaches a `oneshot::Sender` and awaits the reply
//! sent by a worker.

use klams_store::StoreError;
use klams_types::{
    AppendEvent, Event, FactWriteOutcome, IndexKnowledge, KnowledgeItem, MemoryWrite, UpsertFact,
};
use std::fmt;
use tokio::sync::{mpsc, oneshot};

/// Returned when the bounded queue is at capacity. The HTTP layer
/// maps this to 503 + Retry-After.
#[derive(Debug, Clone, Copy)]
pub struct QueueFull;

impl fmt::Display for QueueFull {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("memory write queue at capacity")
    }
}

impl std::error::Error for QueueFull {}

/// Outcome of a successful `try_enqueue` (the producer's future may
/// then await a oneshot reply for synchronous-style writes).
#[derive(Debug)]
pub struct EnqueueOutcome {
    pub depth_after: usize,
}

/// Reply value sent back to the producer on completion. Tied to the
/// variant the producer enqueued.
#[derive(Debug)]
pub enum WriteReply {
    Fact(Result<FactWriteOutcome, StoreError>),
    Event(Result<Event, StoreError>),
    Knowledge(Result<KnowledgeItem, StoreError>),
}

/// A `MemoryWrite` plus an optional reply channel.
#[derive(Debug)]
pub struct WriteJob {
    pub write: MemoryWrite,
    pub reply: Option<oneshot::Sender<WriteReply>>,
}

impl WriteJob {
    pub fn upsert_fact_with_reply(req: UpsertFact) -> (Self, oneshot::Receiver<WriteReply>) {
        let (tx, rx) = oneshot::channel();
        (
            Self {
                write: MemoryWrite::UpsertFact(req),
                reply: Some(tx),
            },
            rx,
        )
    }

    pub fn append_event(req: AppendEvent) -> Self {
        Self {
            write: MemoryWrite::AppendEvent(req),
            reply: None,
        }
    }

    pub fn index_knowledge(req: IndexKnowledge) -> Self {
        Self {
            write: MemoryWrite::IndexKnowledge(req),
            reply: None,
        }
    }
}

/// Bounded queue handle. Cheap to clone; all clones share the same
/// underlying channel.
#[derive(Clone)]
pub struct MemoryQueue {
    sender: mpsc::Sender<WriteJob>,
    capacity: usize,
}

impl fmt::Debug for MemoryQueue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MemoryQueue")
            .field("capacity", &self.capacity)
            .field("depth", &self.depth())
            .field("sender_closed", &self.sender.is_closed())
            .finish()
    }
}

impl MemoryQueue {
    /// Construct a queue and return the matching receiver for the
    /// worker pool. Capacity is the maximum number of in-flight jobs
    /// before `try_enqueue` starts rejecting.
    #[must_use]
    pub fn new(capacity: usize) -> (Self, mpsc::Receiver<WriteJob>) {
        let cap = capacity.max(1);
        let (tx, rx) = mpsc::channel(cap);
        (
            Self {
                sender: tx,
                capacity: cap,
            },
            rx,
        )
    }

    pub fn try_enqueue(&self, job: WriteJob) -> Result<EnqueueOutcome, QueueFull> {
        match self.sender.try_send(job) {
            Ok(()) => Ok(EnqueueOutcome {
                depth_after: self.depth(),
            }),
            Err(mpsc::error::TrySendError::Full(_) | mpsc::error::TrySendError::Closed(_)) => {
                Err(QueueFull)
            }
        }
    }

    #[must_use]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Approximate number of jobs sitting in the channel.
    #[must_use]
    pub fn depth(&self) -> usize {
        self.capacity - self.sender.capacity()
    }
}
