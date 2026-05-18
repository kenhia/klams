//! Per-`FactType` validators.

use super::Validator;
use klams_types::{ErrorDetail, ValidationResult};
use uuid::Uuid;

#[derive(Debug)]
pub struct UserFactValidator;

impl Validator for UserFactValidator {
    fn validate(&self, payload: &serde_json::Value) -> ValidationResult {
        let mut acc = Vec::new();
        let Some(obj) = payload.as_object() else {
            return required(vec!["name".into()], None);
        };
        required_str(obj, "name", 1, 256, &mut acc);
        if let Some(v) = obj.get("email") {
            check_email(v, "payload.email", &mut acc);
        }
        if let Some(v) = obj.get("birthdate") {
            check_rfc3339(v, "payload.birthdate", &mut acc);
        }
        finalize(acc)
    }
}

#[derive(Debug)]
pub struct TaskFactValidator;

const TASK_STATUS_ENUM: &[&str] = &["planned", "in_progress", "blocked", "done", "cancelled"];

impl Validator for TaskFactValidator {
    fn validate(&self, payload: &serde_json::Value) -> ValidationResult {
        let mut acc = Vec::new();
        let Some(obj) = payload.as_object() else {
            return required(vec!["task_id".into(), "status".into()], None);
        };
        if let Some(v) = obj.get("task_id") {
            if let Some(s) = v.as_str() {
                if Uuid::parse_str(s).is_err() {
                    acc.push(detail(
                        "payload.task_id",
                        "uuid",
                        "task_id must be a UUID",
                        Some(v.clone()),
                    ));
                }
            } else {
                acc.push(detail(
                    "payload.task_id",
                    "uuid",
                    "task_id must be a UUID string",
                    Some(v.clone()),
                ));
            }
        } else {
            acc.push(detail(
                "payload.task_id",
                "required",
                "task_id is required",
                None,
            ));
        }
        match obj.get("status") {
            Some(serde_json::Value::String(s)) if TASK_STATUS_ENUM.contains(&s.as_str()) => {}
            Some(v) => acc.push(detail(
                "payload.status",
                "enum",
                &format!("status must be one of {TASK_STATUS_ENUM:?}"),
                Some(v.clone()),
            )),
            None => acc.push(detail(
                "payload.status",
                "required",
                "status is required",
                None,
            )),
        }
        finalize(acc)
    }
}

#[derive(Debug)]
pub struct EnvFactValidator;

impl Validator for EnvFactValidator {
    fn validate(&self, payload: &serde_json::Value) -> ValidationResult {
        let mut acc = Vec::new();
        let Some(obj) = payload.as_object() else {
            return required(vec!["key".into(), "value".into()], None);
        };
        match obj.get("key") {
            Some(serde_json::Value::String(s)) => {
                if s.is_empty() || s.len() > 256 {
                    acc.push(detail(
                        "payload.key",
                        "length",
                        "key must be 1..=256 chars",
                        None,
                    ));
                }
                if !is_env_key(s) {
                    acc.push(detail(
                        "payload.key",
                        "shape",
                        "key must match ^[A-Z][A-Z0-9_]*$",
                        Some(serde_json::Value::String(s.clone())),
                    ));
                }
            }
            Some(v) => acc.push(detail(
                "payload.key",
                "type",
                "key must be a string",
                Some(v.clone()),
            )),
            None => acc.push(detail("payload.key", "required", "key is required", None)),
        }
        match obj.get("value") {
            Some(serde_json::Value::String(s)) => {
                if s.len() > 4096 {
                    acc.push(detail(
                        "payload.value",
                        "length",
                        "value must be <= 4096 chars",
                        None,
                    ));
                }
            }
            Some(v) => acc.push(detail(
                "payload.value",
                "type",
                "value must be a string",
                Some(v.clone()),
            )),
            None => acc.push(detail(
                "payload.value",
                "required",
                "value is required",
                None,
            )),
        }
        finalize(acc)
    }
}

fn is_env_key(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.is_empty() {
        return false;
    }
    if !bytes[0].is_ascii_uppercase() {
        return false;
    }
    bytes
        .iter()
        .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || *b == b'_')
}

fn required(fields: Vec<String>, _payload: Option<&serde_json::Value>) -> ValidationResult {
    let details: Vec<ErrorDetail> = fields
        .into_iter()
        .map(|f| ErrorDetail {
            field: format!("payload.{f}"),
            rule: "required".into(),
            message: format!("{f} is required"),
            value: None,
        })
        .collect();
    Err(details)
}

fn required_str(
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

fn check_email(v: &serde_json::Value, path: &str, acc: &mut Vec<ErrorDetail>) {
    let Some(s) = v.as_str() else {
        acc.push(detail(
            path,
            "type",
            "email must be a string",
            Some(v.clone()),
        ));
        return;
    };
    // Minimal mailbox sanity: exactly one `@`, non-empty local and
    // domain, domain has a dot. Not RFC-perfect; safe shape gate only.
    let parts: Vec<&str> = s.splitn(2, '@').collect();
    let ok =
        parts.len() == 2 && !parts[0].is_empty() && !parts[1].is_empty() && parts[1].contains('.');
    if !ok {
        acc.push(detail(
            path,
            "email_shape",
            "email must be a `local@domain.tld` string",
            Some(v.clone()),
        ));
    }
}

fn check_rfc3339(v: &serde_json::Value, path: &str, acc: &mut Vec<ErrorDetail>) {
    let Some(s) = v.as_str() else {
        acc.push(detail(
            path,
            "type",
            "value must be an RFC3339 string",
            Some(v.clone()),
        ));
        return;
    };
    if time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).is_err()
        && time::Date::parse(s, time::macros::format_description!("[year]-[month]-[day]")).is_err()
    {
        acc.push(detail(
            path,
            "rfc3339",
            "value is not a valid RFC3339 timestamp or date",
            Some(v.clone()),
        ));
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
