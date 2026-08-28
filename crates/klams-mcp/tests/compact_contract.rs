//! Sprint 046 (WI #1178) — the compact response contract.
//!
//! Ported from khound, whose fair eval measured klams' full-text
//! `memory_search` at 4,491 tokens per answered query against
//! 1,024-1,476 for this shape, answers preserved. The properties below
//! are the ones that make that trade honest — a compact response that
//! drops the locator, or fakes metadata it does not have, saves tokens
//! by removing the answer.

use klams_mcp::contract::CompactSearchResponse;
use klams_types::{MemoryKind, PublicAuthorRef, PublicMemory, PublicMemoryContent, ScoredMemory};
use uuid::Uuid;

fn author() -> PublicAuthorRef {
    PublicAuthorRef {
        id: Some(Uuid::nil()),
        agent_name: "claude-kubs0".into(),
        model: None,
        repo: None,
    }
}

fn knowledge_hit(text: &str) -> ScoredMemory {
    ScoredMemory {
        score: 0.0164,
        source_rank: 0,
        raw_score: Some(0.87),
        memory: PublicMemory {
            id: Uuid::from_u128(1),
            content: PublicMemoryContent::Knowledge {
                text: text.into(),
                source_path: Some("docs/setup.md".into()),
                repo: Some("klams".into()),
                host: Some("kubs0".into()),
                content_hash: None,
                heading_path: Some("Setup > TEI".into()),
                language: None,
                chunk_index: None,
                copies: Vec::new(),
                volatility: None,
                supersedes: None,
                superseded_by: None,
            },
            tags: vec!["gotcha".into()],
            author: author(),
            created_at: chrono::Utc::now() - chrono::Duration::days(3),
            updated_at: chrono::Utc::now(),
            deleted_at: None,
            deleted_by_author_id: None,
        },
    }
}

fn fact_hit() -> ScoredMemory {
    ScoredMemory {
        score: 0.01,
        source_rank: 1,
        raw_score: None,
        memory: PublicMemory {
            id: Uuid::from_u128(2),
            content: PublicMemoryContent::Fact {
                fact_type: "EnvFact".into(),
                payload: serde_json::json!({"key": "tei_endpoint", "host": "kubs0"}),
            },
            tags: Vec::new(),
            author: author(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            deleted_at: None,
            deleted_by_author_id: None,
        },
    }
}

/// The locator is the whole trade: a compact hit the agent cannot turn
/// back into full text is just a lossy hit.
#[test]
fn every_hit_carries_a_locator_and_the_fetch_op_that_uses_it() {
    let resp = CompactSearchResponse::build(&[knowledge_hit("short text"), fact_hit()], "tei");
    assert_eq!(resp.hits.len(), 2);
    for h in &resp.hits {
        assert!(!h.id.is_nil(), "hit has no id to fetch by");
    }
    assert_eq!(resp.more.fetch, "memory_get");
}

/// khound's contract: `more` is present on EVERY response, not only
/// truncated ones. A field that appears only sometimes is a field
/// nobody learns to read.
#[test]
fn more_is_present_even_when_nothing_was_elided() {
    let resp = CompactSearchResponse::build(&[knowledge_hit("short")], "short");
    assert!(!resp.more.truncated);
    let json = serde_json::to_value(&resp).unwrap();
    assert!(
        json.get("more").is_some(),
        "`more` must be unconditional: {json}"
    );
}

#[test]
fn truncated_is_true_when_a_snippet_was_elided() {
    let long = "padding ".repeat(200) + "the answer is here " + &"padding ".repeat(200);
    let resp = CompactSearchResponse::build(&[knowledge_hit(&long)], "the answer");
    assert!(
        resp.more.truncated,
        "a windowed snippet must report truncation"
    );
    assert!(
        resp.hits[0].snippet.contains("the answer is here"),
        "snippet missed the match: {}",
        resp.hits[0].snippet
    );
}

/// "Fields that do not apply are omitted, not faked."
#[test]
fn typed_metadata_is_omitted_rather_than_faked() {
    let resp = CompactSearchResponse::build(&[fact_hit()], "tei");
    let json = serde_json::to_value(&resp).unwrap();
    let hit = &json["hits"][0];
    assert_eq!(hit["type"], "EnvFact", "fact must carry its type");
    for absent in ["source_path", "repo", "host", "heading_path", "category"] {
        assert!(
            hit.get(absent).is_none(),
            "{absent} does not apply to a fact and must be omitted, got {hit}"
        );
    }

    let resp = CompactSearchResponse::build(&[knowledge_hit("x")], "x");
    let json = serde_json::to_value(&resp).unwrap();
    let hit = &json["hits"][0];
    assert_eq!(hit["source_path"], "docs/setup.md");
    assert!(
        hit.get("type").is_none(),
        "knowledge has no fact type, got {hit}"
    );
}

/// The eval signal must survive the compaction: a hit still has to say
/// how it ranked and how well it matched, or ranking work goes blind.
#[test]
fn ranking_signal_survives_compaction() {
    let resp = CompactSearchResponse::build(&[knowledge_hit("x")], "x");
    let h = &resp.hits[0];
    assert!((h.score - 0.0164).abs() < f32::EPSILON);
    assert_eq!(h.raw_score, Some(0.87));
    assert_eq!(h.source_rank, 0);
    assert_eq!(h.kind, MemoryKind::Knowledge);
}

#[test]
fn age_is_reported_in_seconds_so_freshness_needs_no_arithmetic() {
    let resp = CompactSearchResponse::build(&[knowledge_hit("x")], "x");
    let age = resp.hits[0].age_seconds;
    let three_days = 3 * 24 * 3600;
    assert!(
        (age - three_days).abs() < 60,
        "expected ~{three_days}s, got {age}"
    );
}

/// The saving is only real if the compact payload is actually smaller.
#[test]
fn compact_is_substantially_smaller_than_full_text() {
    let long = "lorem ipsum dolor sit amet ".repeat(400);
    let hit = knowledge_hit(&long);
    let full = serde_json::to_string(&vec![hit.clone()]).unwrap();
    let compact = serde_json::to_string(&CompactSearchResponse::build(&[hit], "lorem")).unwrap();
    assert!(
        compact.len() * 4 < full.len(),
        "compact {} bytes vs full {} bytes — not a saving worth a contract change",
        compact.len(),
        full.len()
    );
}
