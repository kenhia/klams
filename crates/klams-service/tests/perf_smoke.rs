//! SC-003 performance smoke test.
//!
//! Seeds the MVP corpus (10k facts / 50k events / 10k knowledge
//! items) and asserts that unified search p95 stays under 500 ms.
//!
//! `#[ignore]` because it (a) requires the docker-compose.test.yml
//! stack, (b) takes minutes to seed, and (c) is a single-run
//! measurement, not a CI gate.
//!
//! Run with:
//!   cargo test -p klams-service --release --test `perf_smoke` -- \
//!     --ignored --nocapture --test-threads=1

mod common;

use common::TestServer;
use klams_types::{
    AppendEventRequest, FactType, IndexKnowledgeRequest, SearchRequest, Source, UpsertFactRequest,
};
use serde_json::json;
use std::time::{Duration, Instant};
use uuid::Uuid;

const N_FACTS: usize = 10_000;
const N_EVENTS: usize = 50_000;
const N_KNOWLEDGE: usize = 10_000;
const N_QUERIES: usize = 100;
const P95_BUDGET_MS: u128 = 500;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "perf: requires docker-compose.test.yml + minutes to seed"]
async fn search_p95_under_500ms_at_mvp_corpus() {
    let server = TestServer::spawn().await;
    let run_id = Uuid::now_v7().simple().to_string();
    println!("perf_smoke run_id={run_id}");

    println!("seeding {N_FACTS} facts...");
    let t0 = Instant::now();
    for i in 0..N_FACTS {
        server
            .client
            .upsert_fact(&UpsertFactRequest {
                fact_type: FactType::UserFact,
                payload: json!({"note": format!("perf {run_id} fact {i} content")}),
                source: Source::Controller,
                explicit_id: None,
                expected_version: None,
            })
            .await
            .expect("fact upsert");
    }
    println!("  facts seeded in {:?}", t0.elapsed());

    println!("seeding {N_EVENTS} events...");
    let t0 = Instant::now();
    let task_id = Uuid::now_v7();
    for i in 0..N_EVENTS {
        server
            .client
            .append_event(&AppendEventRequest {
                task_id: Some(task_id),
                category: "perf".into(),
                payload: json!({"summary": format!("perf {run_id} event {i}")}),
                source: Source::Controller,
            })
            .await
            .expect("event append");
    }
    println!("  events seeded in {:?}", t0.elapsed());

    println!("seeding {N_KNOWLEDGE} knowledge items...");
    let t0 = Instant::now();
    for i in 0..N_KNOWLEDGE {
        server
            .client
            .index_knowledge(&IndexKnowledgeRequest {
                text: format!("perf {run_id} knowledge {i} about distributed memory subsystems"),
                source: Source::Controller,
                tags: vec![],
                repo: None,
                file: None,
                machine: None,
            })
            .await
            .expect("knowledge index");
    }
    println!("  knowledge seeded in {:?}", t0.elapsed());

    // Let async workers drain.
    println!("waiting for workers to drain...");
    tokio::time::sleep(Duration::from_secs(30)).await;

    // Measure unified search latency.
    println!("running {N_QUERIES} unified-search queries...");
    let mut latencies: Vec<Duration> = Vec::with_capacity(N_QUERIES);
    for i in 0..N_QUERIES {
        let query = format!("perf {run_id} {} content", i % 100);
        let t = Instant::now();
        let _ = server
            .client
            .search(&SearchRequest {
                query,
                types: None,
                filters: None,
                top_k: 10,
            })
            .await
            .expect("search");
        latencies.push(t.elapsed());
    }
    latencies.sort();
    let p50 = latencies[latencies.len() / 2];
    let p95 = latencies[latencies.len() * 95 / 100];
    let p99 = latencies[latencies.len() * 99 / 100];
    println!("latency p50={p50:?} p95={p95:?} p99={p99:?}");

    assert!(
        p95.as_millis() < P95_BUDGET_MS,
        "search p95 {p95:?} exceeded budget {P95_BUDGET_MS}ms"
    );
}
