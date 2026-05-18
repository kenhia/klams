//! Per-event-category validators (`Service`, `Execution`).

use super::Validator;
use klams_types::{ErrorDetail, ValidationResult};

#[derive(Debug)]
pub struct ServiceEventValidator;

const SERVICE_STATES: &[&str] = &["up", "down", "degraded"];

impl Validator for ServiceEventValidator {
    fn validate(&self, payload: &serde_json::Value) -> ValidationResult {
        let mut acc = Vec::new();
        let Some(obj) = payload.as_object() else {
            return Err(vec![detail(
                "payload",
                "type",
                "Service event payload must be an object",
                None,
            )]);
        };
        for field in ["hostname", "name", "state"] {
            if !obj.contains_key(field) {
                acc.push(detail(
                    &format!("payload.{field}"),
                    "required",
                    &format!("{field} is required for Service events"),
                    None,
                ));
            }
        }
        if let Some(v) = obj.get("state") {
            match v.as_str() {
                Some(s) if SERVICE_STATES.contains(&s) => {}
                _ => acc.push(detail(
                    "payload.state",
                    "enum",
                    &format!("state must be one of {SERVICE_STATES:?}"),
                    Some(v.clone()),
                )),
            }
        }
        finalize(acc)
    }
}

#[derive(Debug)]
pub struct ExecutionEventValidator;

impl Validator for ExecutionEventValidator {
    fn validate(&self, payload: &serde_json::Value) -> ValidationResult {
        let mut acc = Vec::new();
        let Some(obj) = payload.as_object() else {
            return Err(vec![detail(
                "payload",
                "type",
                "Execution event payload must be an object",
                None,
            )]);
        };
        if !obj.contains_key("command") {
            acc.push(detail(
                "payload.command",
                "required",
                "command is required for Execution events",
                None,
            ));
        }
        match obj.get("exit_code") {
            Some(v) => match v.as_i64() {
                Some(n) if (-128..=255).contains(&n) => {}
                _ => acc.push(detail(
                    "payload.exit_code",
                    "numeric_range",
                    "exit_code must be an integer in [-128, 255]",
                    Some(v.clone()),
                )),
            },
            None => acc.push(detail(
                "payload.exit_code",
                "required",
                "exit_code is required for Execution events",
                None,
            )),
        }
        if let Some(v) = obj.get("duration_ms") {
            match v.as_i64() {
                Some(n) if n >= 0 => {}
                _ => acc.push(detail(
                    "payload.duration_ms",
                    "numeric_range",
                    "duration_ms must be an integer >= 0",
                    Some(v.clone()),
                )),
            }
        }
        finalize(acc)
    }
}

fn detail(field: &str, rule: &str, message: &str, value: Option<serde_json::Value>) -> ErrorDetail {
    ErrorDetail {
        field: field.into(),
        rule: rule.into(),
        message: message.into(),
        value,
    }
}

fn finalize(acc: Vec<ErrorDetail>) -> ValidationResult {
    if acc.is_empty() {
        Ok(())
    } else {
        Err(acc)
    }
}
