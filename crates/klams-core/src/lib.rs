//! Bounded write queue, worker pool, and `MemoryWrite` dispatch.

pub mod metrics;
pub mod queue;
pub mod worker;

pub use queue::{EnqueueOutcome, MemoryQueue, QueueFull, WriteJob, WriteReply};
pub use worker::spawn_workers;
