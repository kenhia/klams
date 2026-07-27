//! Sprint 018 (WI #307) — flat `memory_add` args keep the old wire shape.
//!
//! The schema flatten must not change what callers send: the tagged
//! `kind` + per-kind fields JSON that pre-018 clients used has to keep
//! deserializing and validating identically, with per-kind requirement
//! violations surfacing as `SCHEMA_VALIDATION_FAILED` (they previously
//! failed inside serde's tagged-enum deserializer).

use klams_mcp::tools::memory_add::{MemoryAddArgs, MemoryAddContent};
use serde_json::json;

const AUTHOR: &str = "019e76c8-3bb7-7a73-985c-0697269256cf";

fn parse(v: serde_json::Value) -> MemoryAddArgs {
    serde_json::from_value(v).expect("deserialize MemoryAddArgs")
}

#[test]
fn fact_wire_shape_round_trips() {
    let args = parse(json!({
        "author_id": AUTHOR,
        "kind": "fact",
        "fact_type": "EnvFact",
        "payload": {"key": "value"},
    }));
    match args.content().expect("valid fact") {
        MemoryAddContent::Fact { payload, .. } => {
            assert_eq!(payload, json!({"key": "value"}));
        }
        MemoryAddContent::Knowledge { .. } => panic!("expected fact"),
    }
}

#[test]
fn knowledge_wire_shape_round_trips_with_defaults() {
    let args = parse(json!({
        "author_id": AUTHOR,
        "kind": "knowledge",
        "text": "TIL a thing",
    }));
    match args.content().expect("valid knowledge") {
        MemoryAddContent::Knowledge {
            text,
            tags,
            source_path,
            repo,
            volatility: _,
        } => {
            assert_eq!(text, "TIL a thing");
            assert!(tags.is_empty());
            assert!(source_path.is_none());
            assert!(repo.is_none());
        }
        MemoryAddContent::Fact { .. } => panic!("expected knowledge"),
    }
}

#[test]
fn knowledge_optional_fields_carry_through() {
    let args = parse(json!({
        "author_id": AUTHOR,
        "kind": "knowledge",
        "text": "notes",
        "tags": ["a", "b"],
        "source_path": "docs/notes.md",
        "repo": "/home/ken/src/ai/klams",
    }));
    match args.content().expect("valid knowledge") {
        MemoryAddContent::Knowledge { tags, repo, .. } => {
            assert_eq!(tags, vec!["a", "b"]);
            assert_eq!(repo.as_deref(), Some("/home/ken/src/ai/klams"));
        }
        MemoryAddContent::Fact { .. } => panic!("expected knowledge"),
    }
}

#[test]
fn fact_missing_payload_is_schema_validation_failed() {
    let args = parse(json!({
        "author_id": AUTHOR,
        "kind": "fact",
        "fact_type": "UserFact",
    }));
    let env = args.content().expect_err("payload is required for facts");
    assert_eq!(env.meta.error_code, "SCHEMA_VALIDATION_FAILED");
    assert!(env.content[0].text.contains("payload"));
}

#[test]
fn fact_missing_both_fields_names_both() {
    let args = parse(json!({"author_id": AUTHOR, "kind": "fact"}));
    let env = args.content().expect_err("fact fields required");
    assert!(env.content[0].text.contains("fact_type"));
    assert!(env.content[0].text.contains("payload"));
}

#[test]
fn knowledge_missing_text_is_schema_validation_failed() {
    let args = parse(json!({"author_id": AUTHOR, "kind": "knowledge"}));
    let env = args.content().expect_err("text is required for knowledge");
    assert_eq!(env.meta.error_code, "SCHEMA_VALIDATION_FAILED");
    assert!(env.content[0].text.contains("text"));
}

#[test]
fn unknown_kind_fails_deserialization() {
    let res: Result<MemoryAddArgs, _> = serde_json::from_value(json!({
        "author_id": AUTHOR,
        "kind": "event",
        "text": "nope",
    }));
    assert!(res.is_err(), "events go through memory_append_event");
}

#[test]
fn cross_kind_extras_are_ignored_like_pre_018() {
    // A fact carrying knowledge-only fields still lands as a fact.
    let args = parse(json!({
        "author_id": AUTHOR,
        "kind": "fact",
        "fact_type": "TaskFact",
        "payload": {},
        "text": "ignored",
        "tags": ["ignored"],
    }));
    assert!(matches!(
        args.content().expect("valid fact"),
        MemoryAddContent::Fact { .. }
    ));
}
