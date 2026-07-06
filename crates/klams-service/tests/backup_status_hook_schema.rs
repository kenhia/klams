//! Sprint 006 T041 — `backup-status-hook.schema.json` contract test.
//!
//! Validates that every shape `klams_service::backup::hook::BackupHookEvent`
//! emits conforms to the published JSON schema, and that every
//! `examples[]` entry in the schema itself validates (catches drift in
//! either direction).

use chrono::{TimeZone, Utc};
use jsonschema::JSONSchema;
use klams_service::backup::hook::{BackupHookEvent, HookArtifact, HookEventKind};

fn load_schema() -> JSONSchema {
    let raw = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../sprints/006-maintenance-and-backups/contracts/backup-status-hook.schema.json"
    ))
    .expect("read schema");
    let value: serde_json::Value = serde_json::from_str(&raw).expect("parse schema");
    JSONSchema::compile(&value).expect("compile schema")
}

fn raw_schema() -> serde_json::Value {
    let raw = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../sprints/006-maintenance-and-backups/contracts/backup-status-hook.schema.json"
    ))
    .expect("read schema");
    serde_json::from_str(&raw).expect("parse schema")
}

fn assert_valid(schema: &JSONSchema, value: &serde_json::Value, label: &str) {
    let result = schema.validate(value);
    if let Err(errors) = result {
        let msgs: Vec<String> = errors.map(|e| format!("- {e}")).collect();
        panic!(
            "{label} payload failed schema validation:\n{}\npayload: {}",
            msgs.join("\n"),
            serde_json::to_string_pretty(value).unwrap()
        );
    }
}

fn fixed_run_id() -> String {
    "01HZP4K8M3VG2X9D5Q7TYRJB6N".to_string()
}

#[test]
fn started_event_validates() {
    let schema = load_schema();
    let ev = BackupHookEvent {
        schema_version: 1,
        run_id: fixed_run_id(),
        event: HookEventKind::Started,
        started_at: Utc.with_ymd_and_hms(2026, 5, 23, 10, 0, 0).unwrap(),
        ended_at: None,
        duration_ms: None,
        artifacts: Vec::new(),
        ok: false,
        error: None,
    };
    assert_valid(&schema, &serde_json::to_value(&ev).unwrap(), "started");
}

#[test]
fn finished_event_validates() {
    let schema = load_schema();
    let ev = BackupHookEvent {
        schema_version: 1,
        run_id: fixed_run_id(),
        event: HookEventKind::Finished,
        started_at: Utc.with_ymd_and_hms(2026, 5, 23, 10, 0, 0).unwrap(),
        ended_at: Some(Utc.with_ymd_and_hms(2026, 5, 23, 10, 4, 12).unwrap()),
        duration_ms: Some(252_000),
        artifacts: vec![
            HookArtifact {
                kind: "postgres".to_string(),
                path: "/mnt/gratch/klams/postgres-2026-05-23.dump".into(),
                bytes: 41_827_392,
                duration_ms: 31_200,
                ok: true,
                error: None,
            },
            HookArtifact {
                kind: "qdrant".to_string(),
                path: "/mnt/gratch/klams/qdrant-2026-05-23.snapshot".into(),
                bytes: 187_443_200,
                duration_ms: 220_800,
                ok: true,
                error: None,
            },
        ],
        ok: true,
        error: None,
    };
    assert_valid(&schema, &serde_json::to_value(&ev).unwrap(), "finished");
}

#[test]
fn failed_event_validates() {
    let schema = load_schema();
    let ev = BackupHookEvent {
        schema_version: 1,
        run_id: fixed_run_id(),
        event: HookEventKind::Failed,
        started_at: Utc.with_ymd_and_hms(2026, 5, 23, 10, 0, 0).unwrap(),
        ended_at: Some(Utc.with_ymd_and_hms(2026, 5, 23, 10, 0, 38).unwrap()),
        duration_ms: Some(38_000),
        artifacts: vec![HookArtifact {
            kind: "qdrant".to_string(),
            path: "/mnt/gratch/klams/qdrant-2026-05-23.snapshot".into(),
            bytes: 0,
            duration_ms: 6_800,
            ok: false,
            error: Some("qdrant snapshot API returned 503".into()),
        }],
        ok: false,
        error: Some("qdrant snapshot API returned 503".into()),
    };
    assert_valid(&schema, &serde_json::to_value(&ev).unwrap(), "failed");
}

#[test]
fn schema_examples_round_trip() {
    let schema = load_schema();
    let raw = raw_schema();
    let examples = raw["examples"].as_array().expect("examples[] present");
    assert!(
        !examples.is_empty(),
        "schema must publish at least one example"
    );
    for (i, ex) in examples.iter().enumerate() {
        assert_valid(&schema, ex, &format!("examples[{i}]"));
    }
}
