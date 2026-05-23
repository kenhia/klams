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

    /// Sprint 005 (T044) — validate the loaded TOML before the
    /// service accepts traffic. Returns the first offending key
    /// (data-model.md §5).
    ///
    /// Errors on:
    /// * negative λ for any `FactType`
    /// * non-finite λ (NaN, ±∞)
    /// * `task_interval_seconds == 0`
    /// * `batch_size == 0`
    pub fn validate(&self) -> Result<(), DecayConfigError> {
        if self.task_interval_seconds == 0 {
            return Err(DecayConfigError::ZeroInterval);
        }
        if self.batch_size == 0 {
            return Err(DecayConfigError::ZeroBatch);
        }
        for (t, lambda) in &self.lambda {
            if !lambda.is_finite() {
                return Err(DecayConfigError::NonFiniteLambda {
                    fact_type: *t,
                    value: *lambda,
                });
            }
            if *lambda < 0.0 {
                return Err(DecayConfigError::NegativeLambda {
                    fact_type: *t,
                    value: *lambda,
                });
            }
        }
        Ok(())
    }
}

/// Validation failure for [`DecayConfig::validate`]. The service
/// must log the offending key and exit non-zero before binding
/// the API listener (sprint 005 FR-013).
#[derive(Debug, Clone, thiserror::Error, PartialEq)]
pub enum DecayConfigError {
    #[error("decay.task_interval_seconds must be > 0")]
    ZeroInterval,
    #[error("decay.batch_size must be > 0")]
    ZeroBatch,
    #[error("decay.lambda.{fact_type:?} = {value} is not finite")]
    NonFiniteLambda { fact_type: FactType, value: f32 },
    #[error("decay.lambda.{fact_type:?} = {value} is negative")]
    NegativeLambda { fact_type: FactType, value: f32 },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_validate_ok() {
        DecayConfig::default()
            .validate()
            .expect("defaults must validate");
    }

    #[test]
    fn negative_lambda_rejected() {
        let mut c = DecayConfig::default();
        c.lambda.insert(FactType::TaskFact, -0.1);
        let err = c.validate().expect_err("negative λ must fail");
        assert!(matches!(err, DecayConfigError::NegativeLambda { .. }));
    }

    #[test]
    fn non_finite_lambda_rejected() {
        for v in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let mut c = DecayConfig::default();
            c.lambda.insert(FactType::TaskFact, v);
            let err = c.validate().expect_err("non-finite λ must fail");
            assert!(matches!(err, DecayConfigError::NonFiniteLambda { .. }));
        }
    }

    #[test]
    fn zero_interval_rejected() {
        let c = DecayConfig {
            task_interval_seconds: 0,
            ..DecayConfig::default()
        };
        assert_eq!(c.validate(), Err(DecayConfigError::ZeroInterval));
    }

    #[test]
    fn zero_batch_rejected() {
        let c = DecayConfig {
            batch_size: 0,
            ..DecayConfig::default()
        };
        assert_eq!(c.validate(), Err(DecayConfigError::ZeroBatch));
    }

    #[test]
    fn happy_path_lambda_map() {
        let mut c = DecayConfig::default();
        c.lambda.insert(FactType::TaskFact, 1e-5);
        c.lambda.insert(FactType::UserFact, 5e-10);
        c.validate().expect("happy-path map must validate");
    }
}
