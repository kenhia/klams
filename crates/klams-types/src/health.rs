//! Health snapshot wire format.

use crate::maintenance::MaintenanceSnapshot;
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
    /// Sprint 003 T010a: when the client requests `?contract=v1`, the
    /// handler echoes the contract version back so callers can pin
    /// the response shape they depend on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contract: Option<String>,
    /// Sprint 006 FR-018: backup-window state. Omitted on responses
    /// from binaries that don't run the backup orchestrator.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maintenance: Option<MaintenanceSnapshot>,
    /// Sprint 036 (#731): the second-stage reranker. Omitted when the
    /// stage is not configured. **Visible but never fatal**: the stage
    /// is best-effort by contract (a sick reranker degrades ranking
    /// quality, not availability), so this field never contributes to
    /// the aggregate `status` — it exists precisely so "rerank silently
    /// off for a week" can't happen again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reranker: Option<SubsystemStatus>,
}
