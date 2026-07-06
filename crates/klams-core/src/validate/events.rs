//! Per-event-category validators (`Service`, `Execution`).
//!
//! Sprint 003 (T024) tightened these to match
//! `sprints/003-non-agentic-writes/data-model.md` §3 & §4:
//!
//! - `ServiceEventValidator` — required `service`, `event` (enum of
//!   `up|down|restart|version_changed`), `host` (hostname-shape);
//!   optional `version` (≤64 chars), `port` (1..=65535).
//! - `ExecutionTraceEventValidator` — required `task_id` (UUID or
//!   `ansible-<32-hex>` per [`super::facts::check_ansible_task_id`]),
//!   `tool` (1..=128), `phase` (`started|completed|failed`); optional
//!   `detail` (≤4096).

use super::facts::check_ansible_task_id;
use super::Validator;
use klams_types::{ErrorDetail, ValidationResult};

#[derive(Debug)]
pub struct ServiceEventValidator;

const SERVICE_EVENTS: &[&str] = &["up", "down", "restart", "version_changed"];

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
        check_required_str(obj, "service", 1, 128, &mut acc);
        check_required_str(obj, "host", 1, 64, &mut acc);
        match obj.get("event") {
            Some(serde_json::Value::String(s)) if SERVICE_EVENTS.contains(&s.as_str()) => {}
            Some(v) => acc.push(detail(
                "payload.event",
                "enum",
                &format!("event must be one of {SERVICE_EVENTS:?}"),
                Some(v.clone()),
            )),
            None => acc.push(detail(
                "payload.event",
                "required",
                "event is required",
                None,
            )),
        }
        if let Some(v) = obj.get("version") {
            check_string_max(v, "payload.version", 64, &mut acc);
        }
        if let Some(v) = obj.get("port") {
            match v.as_u64() {
                Some(n) if (1..=65535).contains(&n) => {}
                _ => acc.push(detail(
                    "payload.port",
                    "numeric_range",
                    "port must be an integer in [1, 65535]",
                    Some(v.clone()),
                )),
            }
        }
        finalize(acc)
    }
}

const EXEC_PHASES: &[&str] = &["started", "completed", "failed"];

#[derive(Debug)]
pub struct ExecutionTraceEventValidator;

impl Validator for ExecutionTraceEventValidator {
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
        if let Some(v) = obj.get("task_id") {
            check_ansible_task_id(v, &mut acc);
        } else {
            acc.push(detail(
                "payload.task_id",
                "required",
                "task_id is required",
                None,
            ));
        }
        check_required_str(obj, "tool", 1, 128, &mut acc);
        match obj.get("phase") {
            Some(serde_json::Value::String(s)) if EXEC_PHASES.contains(&s.as_str()) => {}
            Some(v) => acc.push(detail(
                "payload.phase",
                "enum",
                &format!("phase must be one of {EXEC_PHASES:?}"),
                Some(v.clone()),
            )),
            None => acc.push(detail(
                "payload.phase",
                "required",
                "phase is required",
                None,
            )),
        }
        if let Some(v) = obj.get("detail") {
            check_string_max(v, "payload.detail", 4096, &mut acc);
        }
        finalize(acc)
    }
}

fn check_required_str(
    obj: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    min: usize,
    max: usize,
    acc: &mut Vec<ErrorDetail>,
) {
    match obj.get(key) {
        Some(serde_json::Value::String(s)) => {
            if s.len() < min || s.len() > max {
                acc.push(detail(
                    &format!("payload.{key}"),
                    "length",
                    &format!("{key} must be {min}..={max} chars"),
                    None,
                ));
            }
        }
        Some(v) => acc.push(detail(
            &format!("payload.{key}"),
            "type",
            &format!("{key} must be a string"),
            Some(v.clone()),
        )),
        None => acc.push(detail(
            &format!("payload.{key}"),
            "required",
            &format!("{key} is required"),
            None,
        )),
    }
}

fn check_string_max(v: &serde_json::Value, path: &str, max: usize, acc: &mut Vec<ErrorDetail>) {
    match v.as_str() {
        Some(s) if s.len() <= max => {}
        Some(_) => acc.push(detail(
            path,
            "length",
            &format!("{path} must be <= {max} chars"),
            Some(v.clone()),
        )),
        None => acc.push(detail(
            path,
            "type",
            &format!("{path} must be a string"),
            Some(v.clone()),
        )),
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn good_service() -> serde_json::Value {
        json!({"service": "qdrant", "event": "up", "host": "kubs0"})
    }

    fn good_exec() -> serde_json::Value {
        json!({
            "task_id": "01900000-0000-7000-8000-000000000000",
            "tool": "rg",
            "phase": "started"
        })
    }

    #[test]
    fn service_event_requires_service_event_host() {
        let v = ServiceEventValidator;
        let err = v.validate(&json!({})).unwrap_err();
        let fields: Vec<&str> = err.iter().map(|d| d.field.as_str()).collect();
        for f in ["payload.service", "payload.host", "payload.event"] {
            assert!(fields.contains(&f), "missing {f} in {err:?}");
        }
    }

    #[test]
    fn service_event_unknown_event_rejected() {
        let v = ServiceEventValidator;
        let mut p = good_service();
        p["event"] = json!("flapping");
        let err = v.validate(&p).unwrap_err();
        assert!(err
            .iter()
            .any(|d| d.field == "payload.event" && d.rule == "enum"));
    }

    #[test]
    fn service_event_known_events_pass() {
        let v = ServiceEventValidator;
        for ev in ["up", "down", "restart", "version_changed"] {
            let mut p = good_service();
            p["event"] = json!(ev);
            assert!(v.validate(&p).is_ok(), "{ev} should validate");
        }
    }

    #[test]
    fn service_event_port_out_of_range_rejected() {
        let v = ServiceEventValidator;
        let mut p = good_service();
        p["port"] = json!(70000);
        let err = v.validate(&p).unwrap_err();
        assert!(err.iter().any(|d| d.field == "payload.port"));
    }

    #[test]
    fn execution_event_requires_task_id_tool_phase() {
        let v = ExecutionTraceEventValidator;
        let err = v.validate(&json!({})).unwrap_err();
        let fields: Vec<&str> = err.iter().map(|d| d.field.as_str()).collect();
        for f in ["payload.task_id", "payload.tool", "payload.phase"] {
            assert!(fields.contains(&f), "missing {f} in {err:?}");
        }
    }

    #[test]
    fn execution_event_unknown_phase_rejected() {
        let v = ExecutionTraceEventValidator;
        let mut p = good_exec();
        p["phase"] = json!("queued");
        let err = v.validate(&p).unwrap_err();
        assert!(err
            .iter()
            .any(|d| d.field == "payload.phase" && d.rule == "enum"));
    }

    #[test]
    fn execution_event_ansible_run_id_accepted() {
        let v = ExecutionTraceEventValidator;
        let mut p = good_exec();
        p["task_id"] = json!("ansible-0123456789abcdef0123456789abcdef");
        assert!(v.validate(&p).is_ok());
    }

    #[test]
    fn execution_event_oversize_detail_rejected() {
        let v = ExecutionTraceEventValidator;
        let mut p = good_exec();
        p["detail"] = json!("x".repeat(5000));
        let err = v.validate(&p).unwrap_err();
        assert!(err.iter().any(|d| d.field == "payload.detail"));
    }
}
