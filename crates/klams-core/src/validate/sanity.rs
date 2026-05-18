//! Universal sanity rules (run for every write regardless of type).

use super::Validator;
use klams_types::{ErrorDetail, ValidationResult};
use time::OffsetDateTime;

/// Allow ±10 years from the wall clock per data-model.md.
const TIMESTAMP_RANGE_SECS: i64 = 10 * 365 * 24 * 60 * 60;

/// Field-name suffixes we treat as timestamps.
const TIMESTAMP_SUFFIXES: &[&str] = &["_at", "_time", "_ts"];
const TIMESTAMP_EXACT: &[&str] = &["at", "time", "ts", "timestamp"];

#[derive(Debug)]
pub struct TimestampRangeRule;

impl Validator for TimestampRangeRule {
    fn validate(&self, payload: &serde_json::Value) -> ValidationResult {
        let mut acc = Vec::new();
        walk(payload, "payload", &mut |path, key, value| {
            if !is_timestamp_field(key) {
                return;
            }
            let Some(s) = value.as_str() else { return };
            let ts = OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339);
            match ts {
                Ok(t) => {
                    let now = OffsetDateTime::now_utc();
                    let delta = (t.unix_timestamp() - now.unix_timestamp()).abs();
                    if delta > TIMESTAMP_RANGE_SECS {
                        acc.push(ErrorDetail {
                            field: path.clone(),
                            rule: "timestamp_range".into(),
                            message: format!(
                                "{path}: {s} is more than 10 years from server wall clock"
                            ),
                            value: Some(value.clone()),
                        });
                    }
                }
                Err(_) => acc.push(ErrorDetail {
                    field: path.clone(),
                    rule: "timestamp_range".into(),
                    message: format!("{path}: value `{s}` is not a valid RFC3339 timestamp"),
                    value: Some(value.clone()),
                }),
            }
        });
        finalize(acc)
    }
}

#[derive(Debug)]
pub struct HostnameShapeRule;

/// Conservative LDH+dots check: each label is 1–63 chars, starts
/// and ends with `[a-z0-9]`, may contain hyphens; labels joined by
/// dots. Case-folded before matching per data-model.md.
fn is_valid_hostname(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    s.split('.').all(|label| {
        let n = label.len();
        if n == 0 || n > 63 {
            return false;
        }
        let bytes = label.as_bytes();
        let head_ok = bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit();
        let tail_ok = bytes[n - 1].is_ascii_lowercase() || bytes[n - 1].is_ascii_digit();
        if !head_ok || !tail_ok {
            return false;
        }
        bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
    })
}

impl Validator for HostnameShapeRule {
    fn validate(&self, payload: &serde_json::Value) -> ValidationResult {
        let mut acc = Vec::new();
        walk(payload, "payload", &mut |path, key, value| {
            if !matches!(key, "hostname" | "host") {
                return;
            }
            let Some(s) = value.as_str() else {
                acc.push(ErrorDetail {
                    field: path.clone(),
                    rule: "hostname_shape".into(),
                    message: format!("{path}: must be a string"),
                    value: Some(value.clone()),
                });
                return;
            };
            let lower = s.to_ascii_lowercase();
            if !is_valid_hostname(&lower) {
                acc.push(ErrorDetail {
                    field: path.clone(),
                    rule: "hostname_shape".into(),
                    message: format!("{path}: `{s}` is not a valid LDH hostname"),
                    value: Some(value.clone()),
                });
            }
        });
        finalize(acc)
    }
}

/// Per-field numeric range bounds. Currently advisory — bounds are
/// declared by per-type validators by emitting their own errors. The
/// generic rule rejects values that are not finite when they appear
/// in a numeric position.
#[derive(Debug)]
pub struct NumericRangeRule;

impl Validator for NumericRangeRule {
    fn validate(&self, payload: &serde_json::Value) -> ValidationResult {
        let mut acc = Vec::new();
        walk(payload, "payload", &mut |path, _key, value| {
            if let Some(n) = value.as_f64() {
                if !n.is_finite() {
                    acc.push(ErrorDetail {
                        field: path.clone(),
                        rule: "numeric_range".into(),
                        message: format!("{path}: numeric value is not finite"),
                        value: Some(value.clone()),
                    });
                }
            }
        });
        finalize(acc)
    }
}

fn is_timestamp_field(key: &str) -> bool {
    if TIMESTAMP_EXACT.contains(&key) {
        return true;
    }
    TIMESTAMP_SUFFIXES.iter().any(|s| key.ends_with(s))
}

fn finalize(acc: Vec<ErrorDetail>) -> ValidationResult {
    if acc.is_empty() {
        Ok(())
    } else {
        Err(acc)
    }
}

/// Walk a JSON value depth-first, calling `f(path, key, value)` for
/// every leaf-or-named field. `path` is the dotted root-relative
/// path; `key` is the leaf field name (used for shape-keyed rules).
fn walk<F: FnMut(&String, &str, &serde_json::Value)>(v: &serde_json::Value, path: &str, f: &mut F) {
    match v {
        serde_json::Value::Object(map) => {
            for (k, child) in map {
                let p = format!("{path}.{k}");
                f(&p, k, child);
                walk(child, &p, f);
            }
        }
        serde_json::Value::Array(items) => {
            for (i, child) in items.iter().enumerate() {
                let p = format!("{path}[{i}]");
                walk(child, &p, f);
            }
        }
        _ => {
            // leaves of object fields are reported via the parent loop;
            // top-level scalars have no field-name context.
        }
    }
}
