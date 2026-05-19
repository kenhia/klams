//! Pre-enqueue validator registry for write traffic (sprint 002 US1).
//!
//! Layers: per-type validators (one per `FactType` or event category)
//! plus a sanity stack that runs for every write. Each validator
//! returns `ValidationResult`; errors accumulate into a single
//! `Vec<ErrorDetail>` so the HTTP layer can emit one 422 response
//! with all violations at once.

pub mod events;
pub mod facts;
pub mod sanity;

use klams_types::{ErrorDetail, FactType, MemoryWrite, ValidationResult};
use std::collections::HashMap;

/// A single validation rule. Pure: takes a JSON payload, returns
/// the violations (if any). Implementations are stateless and
/// `Send + Sync` so the registry can be shared across worker tasks.
pub trait Validator: Send + Sync + std::fmt::Debug {
    fn validate(&self, payload: &serde_json::Value) -> ValidationResult;
}

#[derive(Debug, Default)]
pub struct ValidatorRegistry {
    per_type: HashMap<FactType, Vec<Box<dyn Validator>>>,
    per_event_category: HashMap<String, Vec<Box<dyn Validator>>>,
    sanity: Vec<Box<dyn Validator>>,
}

impl ValidatorRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Wire up every default validator shipped this sprint.
    #[must_use]
    pub fn with_defaults() -> Self {
        let mut r = Self::new();
        r.register_default();
        r
    }

    pub fn register_default(&mut self) {
        // Sanity layer.
        self.sanity.push(Box::new(sanity::TimestampRangeRule));
        self.sanity.push(Box::new(sanity::HostnameShapeRule));
        self.sanity.push(Box::new(sanity::NumericRangeRule));
        // Per-type fact validators.
        self.per_type
            .insert(FactType::UserFact, vec![Box::new(facts::UserFactValidator)]);
        self.per_type
            .insert(FactType::TaskFact, vec![Box::new(facts::TaskFactValidator)]);
        self.per_type
            .insert(FactType::EnvFact, vec![Box::new(facts::EnvFactValidator)]);
        // Per-category event validators.
        self.per_event_category.insert(
            "service".into(),
            vec![Box::new(events::ServiceEventValidator)],
        );
        self.per_event_category.insert(
            "execution".into(),
            vec![Box::new(events::ExecutionTraceEventValidator)],
        );
    }

    /// Validate a pre-enqueue `MemoryWrite`. Returns `Ok(())` when
    /// every rule passes, `Err(details)` otherwise.
    pub fn validate_write(&self, write: &MemoryWrite) -> ValidationResult {
        match write {
            MemoryWrite::UpsertFact(req) => {
                let mut acc = self.run_sanity(&req.payload);
                if let Some(vs) = self.per_type.get(&req.fact_type) {
                    for v in vs {
                        if let Err(mut e) = v.validate(&req.payload) {
                            acc.append(&mut e);
                        }
                    }
                }
                finalize(acc)
            }
            MemoryWrite::AppendEvent(req) => {
                let mut acc = self.run_sanity(&req.payload);
                let key = req.category.to_lowercase();
                if let Some(vs) = self.per_event_category.get(&key) {
                    for v in vs {
                        if let Err(mut e) = v.validate(&req.payload) {
                            acc.append(&mut e);
                        }
                    }
                }
                finalize(acc)
            }
            MemoryWrite::IndexKnowledge(_) => Ok(()),
        }
    }

    fn run_sanity(&self, payload: &serde_json::Value) -> Vec<ErrorDetail> {
        let mut acc = Vec::new();
        for v in &self.sanity {
            if let Err(mut e) = v.validate(payload) {
                acc.append(&mut e);
            }
        }
        acc
    }
}

fn finalize(acc: Vec<ErrorDetail>) -> ValidationResult {
    if acc.is_empty() {
        Ok(())
    } else {
        Err(acc)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use klams_types::{FactType, Source, UpsertFact};
    use serde_json::json;

    fn upsert(fact_type: FactType, payload: serde_json::Value) -> MemoryWrite {
        MemoryWrite::UpsertFact(UpsertFact {
            fact_type,
            payload,
            source: Source::User,
            explicit_id: None,
            expected_version: Some(0),
        })
    }

    #[test]
    fn user_fact_requires_name() {
        let r = ValidatorRegistry::with_defaults();
        let err = r
            .validate_write(&upsert(FactType::UserFact, json!({})))
            .unwrap_err();
        assert!(err
            .iter()
            .any(|d| d.rule == "required" && d.field == "payload.name"));
    }

    #[test]
    fn user_fact_ok() {
        let r = ValidatorRegistry::with_defaults();
        assert!(r
            .validate_write(&upsert(FactType::UserFact, json!({"name": "Ken"})))
            .is_ok());
    }

    #[test]
    fn task_fact_requires_status_enum() {
        let r = ValidatorRegistry::with_defaults();
        let err = r
            .validate_write(&upsert(
                FactType::TaskFact,
                json!({
                    "task_id": "00000000-0000-7000-8000-000000000000",
                    "status": "weird"
                }),
            ))
            .unwrap_err();
        assert!(err
            .iter()
            .any(|d| d.rule == "enum" && d.field == "payload.status"));
    }

    #[test]
    fn hostname_shape_rejected_anywhere() {
        let r = ValidatorRegistry::with_defaults();
        let err = r
            .validate_write(&upsert(
                FactType::TaskFact,
                json!({
                    "task_id": "00000000-0000-7000-8000-000000000000",
                    "status": "planned",
                    "hostname": "WHAT_ever"
                }),
            ))
            .unwrap_err();
        assert!(err.iter().any(|d| d.rule == "hostname_shape"));
    }

    #[test]
    fn far_future_timestamp_rejected() {
        let r = ValidatorRegistry::with_defaults();
        let err = r
            .validate_write(&upsert(
                FactType::UserFact,
                json!({"name": "x", "seen_at": "9999-01-01T00:00:00Z"}),
            ))
            .unwrap_err();
        assert!(err.iter().any(|d| d.rule == "timestamp_range"));
    }
}
