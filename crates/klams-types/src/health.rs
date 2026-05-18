//! Health snapshot wire format.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HealthStatus {
    Ok,
    Degraded,
    Down,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubsystemStatus {
    pub state: HealthStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueStatus {
    pub depth: usize,
    pub capacity: usize,
    pub workers: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthSnapshot {
    pub status: HealthStatus,
    pub postgres: SubsystemStatus,
    pub qdrant: SubsystemStatus,
    pub embeddings: SubsystemStatus,
    pub queue: QueueStatus,
    pub version: String,
    pub uptime_seconds: u64,
}
