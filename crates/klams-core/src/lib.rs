//! Bounded write queue, worker pool, and `MemoryWrite` dispatch.

pub mod context;
pub mod decay;
pub mod dedupe;
pub mod fetch;
pub mod hybrid;
pub mod knowledge_write;
pub mod metrics;
pub mod policy;
pub mod projection;
pub mod provenance;
pub mod queue;
pub mod retrieval;
pub mod snippet;
pub mod summarize;
pub mod tokens;
pub mod validate;
pub mod worker;

pub use decay::{DecayConfig, DecayTask, LastUsedBumper};
pub use knowledge_write::{prepare as prepare_knowledge, PreparedKnowledge};
pub use policy::{PolicyEntry, PolicyTable};
pub use queue::{EnqueueOutcome, MemoryQueue, QueueFull, WriteJob, WriteReply};
pub use validate::ValidatorRegistry;
pub use worker::spawn_workers;
