//! Sprint 059 (korg WI 4612): every record the publisher can build must
//! validate against kdashdata's `kdash-klams-status` schema.
//!
//! The fixture is a byte copy of kdashdata
//! `contracts/schemas/kdash-klams-status.schema.json` at `d57e1dc`
//! (md5 `9e374cb9933f68f278105936621dbdb4`). A contract change there is a
//! new key (`kdash:klams:v2:status`), so this copy should never need a
//! silent refresh — replace it only alongside a key change.

use std::collections::HashMap;

use jsonschema::JSONSchema;
use klams_monitor::kdash_status::{build_record, KdashStatusConfig, Observed, ScannerConfig};
use klams_types::{HealthSnapshot, PublicAuthor};
use serde_json::{json, Value};

fn schema() -> (Value, JSONSchema) {
    let raw = include_str!("fixtures/kdash-klams-status.schema.json");
    let value: Value = serde_json::from_str(raw).expect("schema JSON");
    let compiled = JSONSchema::compile(&value).expect("compile schema");
    (value, compiled)
}

fn assert_valid(compiled: &JSONSchema, record: &Value) {
    if let Err(errors) = compiled.validate(record) {
        let msgs: Vec<String> = errors
            .map(|e| format!("{} at {}", e, e.instance_path))
            .collect();
        panic!("record failed the schema: {msgs:?}\n{record:#}");
    }
}

fn cfg() -> KdashStatusConfig {
    KdashStatusConfig {
        interval_secs: 60,
        kdash_pub: "kdash-pub".into(),
        scanners: vec![
            ScannerConfig {
                name: "kai".into(),
                author: "kai-scanner".into(),
                unit: None,
                expected_interval_s: 3600,
            },
            ScannerConfig {
                name: "kubs0".into(),
                author: "klams-scanner".into(),
                unit: Some("klams-scanner.service".into()),
                expected_interval_s: 3600,
            },
        ],
    }
}

fn health(status: &str, rerank: Option<&str>) -> HealthSnapshot {
    let mut h = json!({
        "status": status,
        "postgres": {"state": "Ok"},
        "qdrant": {"state": "Ok"},
        "embeddings": {"state": "Ok"},
        "queue": {"depth": 3, "capacity": 1024, "workers": 4},
        "version": "0.1.59",
        "uptime_seconds": 72357,
    });
    if let Some(r) = rerank {
        h["reranker"] = json!({"state": r});
    }
    serde_json::from_value(h).expect("HealthSnapshot")
}

fn author(name: &str, last_seen: &str, knowledge: i64, writes: i64, events: i64) -> PublicAuthor {
    serde_json::from_value(json!({
        "id": uuid::Uuid::now_v7(),
        "agent_name": name,
        "created_at": "2026-06-01T00:00:00Z",
        "last_seen_at": last_seen,
        "counts": {
            "writes": writes, "knowledge": knowledge, "events": events,
            "soft_deletes": 0, "restores_received": 0,
        },
    }))
    .expect("PublicAuthor")
}

fn observed() -> Observed {
    Observed {
        ts: 1_794_618_700.25,
        health: Some(health("Ok", Some("Ok"))),
        authors: Some(vec![
            author("kai-scanner", "2026-11-13T21:19:17.982202Z", 123_565, 0, 0),
            author(
                "klams-scanner",
                "2026-11-10T06:09:06.047238Z",
                104_933,
                0,
                0,
            ),
            author("claude", "2026-11-13T05:04:57Z", 364, 8, 6),
        ]),
        backup_at: Some(1_794_556_864),
        last_ran: HashMap::from([("kubs0".to_string(), 1_794_618_657)]),
    }
}

#[test]
fn validator_accepts_the_contract_examples_and_refuses_its_counterexamples() {
    // Guards the guard: a validator that passes everything proves nothing.
    let (value, compiled) = schema();
    for ex in value["examples"].as_array().expect("examples") {
        assert_valid(&compiled, ex);
    }
    for cx in value["x-counterexamples"]
        .as_array()
        .expect("counterexamples")
    {
        assert!(
            !compiled.is_valid(&cx["record"]),
            "counterexample validated: {}",
            cx["why"]
        );
    }
}

#[test]
fn healthy_record_is_valid_and_copies_klams() {
    let (_, compiled) = schema();
    let r = build_record(&cfg(), &observed());
    assert_valid(&compiled, &r);

    assert_eq!(r["health"], "ok");
    assert_eq!(r["version"], "0.1.59");
    assert_eq!(r["uptime_s"], 72357);
    assert_eq!(r["expected_refresh_s"], 60);
    // Summed exactly as klams-view's /api/overview: facts are `writes`.
    assert_eq!(r["knowledge"], 123_565 + 104_933 + 364);
    assert_eq!(r["facts"], 8);
    assert_eq!(r["events"], 6);
    assert_eq!(r["queue_depth"], 3);
    assert_eq!(r["queue_capacity"], 1024);
    assert_eq!(r["backup_at"], 1_794_556_864);
    assert_eq!(r["new_per_day"], json!([]));
    let names: Vec<&str> = r["subsystems"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["postgres", "qdrant", "embed", "rerank"]);
}

#[test]
fn a_scanner_that_ran_without_writing_carries_both_stamps() {
    // The kubs0 case behind WI 4612: wrote on 11-10, ran minutes ago.
    let r = build_record(&cfg(), &observed());
    let s = &r["scanners"];
    assert_eq!(s[0]["name"], "kai");
    assert_eq!(s[0]["last_wrote_at"], 1_794_604_757);
    // No unit on this host for kai: present-and-null, never absent.
    assert!(s[0].as_object().unwrap().contains_key("last_ran_at"));
    assert_eq!(s[0]["last_ran_at"], Value::Null);
    assert_eq!(s[1]["name"], "kubs0");
    assert_eq!(s[1]["last_wrote_at"], 1_794_290_946);
    assert_eq!(s[1]["last_ran_at"], 1_794_618_657);
    assert_eq!(s[1]["expected_interval_s"], 3600);
}

#[test]
fn degraded_record_is_valid() {
    let (_, compiled) = schema();
    let mut obs = observed();
    obs.health = Some(health("Degraded", Some("Down")));
    obs.backup_at = None;
    obs.last_ran.clear();
    let r = build_record(&cfg(), &obs);
    assert_valid(&compiled, &r);
    assert_eq!(r["health"], "degraded");
    assert_eq!(r["subsystems"][3], json!({"name": "rerank", "ok": false}));
    assert_eq!(r["backup_at"], Value::Null);
}

#[test]
fn down_record_is_valid_and_all_null() {
    let (_, compiled) = schema();
    let obs = Observed {
        ts: 1_794_618_700.0,
        ..Observed::default()
    };
    let r = build_record(&cfg(), &obs);
    assert_valid(&compiled, &r);
    assert_eq!(r["health"], "down");
    for k in [
        "version",
        "uptime_s",
        "knowledge",
        "facts",
        "events",
        "queue_depth",
        "queue_capacity",
        "backup_at",
    ] {
        assert_eq!(r[k], Value::Null, "{k} should be null");
    }
    assert_eq!(r["scanners"], json!([]));
    assert_eq!(r["subsystems"], json!([]));
}

#[test]
fn author_read_failure_nulls_counts_and_drops_scanners() {
    // klams answered /healthz but /v1/authors failed (e.g. an identity
    // klams no longer accepts): counts unknown, and a scanner row with
    // `last_wrote_at: null` would wrongly claim "never wrote".
    let (_, compiled) = schema();
    let mut obs = observed();
    obs.authors = None;
    let r = build_record(&cfg(), &obs);
    assert_valid(&compiled, &r);
    assert_eq!(r["health"], "ok");
    assert_eq!(r["knowledge"], Value::Null);
    assert_eq!(r["scanners"], json!([]));
}

#[test]
fn missing_author_and_no_reranker_are_still_valid() {
    let (_, compiled) = schema();
    let mut obs = observed();
    obs.health = Some(health("Ok", None));
    obs.authors = Some(vec![author("claude", "2026-11-13T05:04:57Z", 1, 1, 1)]);
    let r = build_record(&cfg(), &obs);
    assert_valid(&compiled, &r);
    assert_eq!(r["scanners"][0]["last_wrote_at"], Value::Null);
    assert_eq!(r["subsystems"].as_array().unwrap().len(), 3);
}
