//! Decay configuration shared between `klams-service` (loads from
//! TOML) and `klams-core` (drives the background task).

use crate::entities::FactType;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;

/// Background decay-task configuration (sprint 002 US3). The
/// `[decay]` TOML block is optional; missing values fall back to the
/// well-known defaults in `Default::default()`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecayConfig {
    #[serde(default = "default_decay_interval_seconds")]
    pub task_interval_seconds: u64,
    #[serde(default = "default_decay_batch_size")]
    pub batch_size: u32,
    /// Per-`FactType` lambda. Missing keys fall back to
    /// `default_lambda_for`.
    #[serde(default)]
    pub lambda: HashMap<FactType, f32>,
}

fn default_decay_interval_seconds() -> u64 {
    3600
}

fn default_decay_batch_size() -> u32 {
    500
}

impl Default for DecayConfig {
    fn default() -> Self {
        Self {
            task_interval_seconds: default_decay_interval_seconds(),
            batch_size: default_decay_batch_size(),
            lambda: HashMap::new(),
        }
    }
}

impl DecayConfig {
    #[must_use]
    pub fn task_interval(&self) -> Duration {
        Duration::from_secs(self.task_interval_seconds)
    }

    #[must_use]
    pub fn lambda_for(&self, t: FactType) -> f32 {
        self.lambda
            .get(&t)
            .copied()
            .unwrap_or_else(|| Self::default_lambda_for(t))
    }

    #[must_use]
    pub fn default_lambda_for(t: FactType) -> f32 {
        match t {
            FactType::TaskFact => 1e-6,
            FactType::UserFact | FactType::EnvFact => 1e-9,
        }
    }
}
