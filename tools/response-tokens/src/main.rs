//! Measure the compact response contract against full text (sprint 046, WI #1178).
//!
//! khound's evidence for the contract was a measurement, not an
//! argument, and the WI asks for the same here: "a compact response
//! that forces extra reads shows up as a regression, not a win". So
//! this charges a follow-up read whenever the snippet did not carry
//! what the suite's answer key wanted — otherwise "compact" would just
//! mean "truncated", and truncation always wins a token comparison
//! while losing the thing the tokens were for.
//!
//! Reads the frozen suite, queries the live klams REST search (which
//! returns whole records), and projects each response into both wire
//! shapes using the SAME snippet code the MCP tool ships.
//!
//! ```text
//! KLAMS_TOKEN=<read-scope token> cargo run -p klams-response-tokens
//! ```
//!
//! `KLAMS_URL` defaults to the loopback service, `KHOUND_EVAL_SUITE` to
//! `~/khound-eval/suite-002.toml`.

use anyhow::{Context, Result};
use klams_core::snippet::match_window;
use klams_core::tokens::{TokenCounter, TokenMode};
use serde::Deserialize;

const TOP_K: usize = 10;

#[derive(Debug, Deserialize)]
struct Suite {
    #[serde(default, rename = "query")]
    queries: Vec<Query>,
}

#[derive(Debug, Deserialize)]
struct Query {
    id: String,
    text: String,
    #[serde(default)]
    class: Option<String>,
    #[serde(default)]
    answers: Vec<Answer>,
}

/// One accepted answer. A hit satisfies it when every stated condition
/// holds — `locator_contains` against the hit's locator, `contains`
/// against its text.
#[derive(Debug, Deserialize, Default)]
struct Answer {
    #[serde(default)]
    locator_contains: Option<String>,
    #[serde(default)]
    contains: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct SearchResponse {
    #[serde(default)]
    results: Vec<Hit>,
}

#[derive(Debug, Deserialize, Clone)]
struct Hit {
    id: String,
    #[serde(default)]
    score: f32,
    #[serde(default)]
    raw_score: Option<f32>,
    #[serde(default)]
    source_rank: Option<u32>,
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    payload: serde_json::Value,
}

impl Hit {
    /// The body an agent would read. Knowledge carries prose; facts and
    /// events carry a JSON payload.
    fn text(&self) -> String {
        self.payload
            .get("text")
            .and_then(|v| v.as_str())
            .map_or_else(|| self.payload.to_string(), str::to_string)
    }

    /// Where the hit points. Scanner chunks name a file; curated
    /// memories name only themselves.
    fn locator(&self) -> String {
        self.payload
            .get("file")
            .and_then(|v| v.as_str())
            .map_or_else(|| self.id.clone(), str::to_string)
    }

    /// The full-text wire shape: the whole record, as MCP
    /// `memory_search` served it before this sprint.
    fn full_json(&self) -> serde_json::Value {
        serde_json::json!({
            "score": self.score,
            "raw_score": self.raw_score,
            "source_rank": self.source_rank,
            "memory": {
                "id": self.id,
                "kind": self.kind,
                "payload": self.payload,
            }
        })
    }

    /// The compact wire shape, built with the shipped snippet code.
    fn compact_json(&self, query: &str) -> serde_json::Value {
        let mut o = serde_json::json!({
            "id": self.id,
            "kind": self.kind,
            "snippet": match_window(&self.text(), query),
            "score": self.score,
            "source_rank": self.source_rank,
        });
        if let Some(r) = self.raw_score {
            o["raw_score"] = serde_json::json!(r);
        }
        // Typed metadata, omitted rather than faked — same rule as the
        // shipped contract.
        for field in ["file", "repo", "host", "heading_path"] {
            if let Some(v) = self.payload.get(field) {
                if !v.is_null() {
                    o[field] = v.clone();
                }
            }
        }
        o
    }
}

/// Does this hit satisfy the answer key, reading `body`?
///
/// Passing the snippet as `body` asks the question the token model
/// needs: *would the agent have had to fetch?*
fn satisfies(hit: &Hit, body: &str, answers: &[Answer]) -> bool {
    let locator = hit.locator().to_lowercase();
    let body = body.to_lowercase();
    answers.iter().any(|a| {
        let locator_ok = a
            .locator_contains
            .as_ref()
            .is_none_or(|l| locator.contains(&l.to_lowercase()));
        let contains_ok = a.contains.iter().all(|c| body.contains(&c.to_lowercase()));
        // An answer with neither condition matches nothing; treat it as
        // unsatisfiable rather than as a free pass.
        (a.locator_contains.is_some() || !a.contains.is_empty()) && locator_ok && contains_ok
    })
}

#[tokio::main]
async fn main() -> Result<()> {
    let token = std::env::var("KLAMS_TOKEN")
        .context("KLAMS_TOKEN is not set — a read-scope klams token is required")?;
    let base = std::env::var("KLAMS_URL").unwrap_or_else(|_| "http://127.0.0.1:7777".into());
    let suite_path = std::env::var("KHOUND_EVAL_SUITE").unwrap_or_else(|_| {
        format!(
            "{}/khound-eval/suite-002.toml",
            std::env::var("HOME").unwrap_or_default()
        )
    });

    let suite: Suite = toml::from_str(
        &std::fs::read_to_string(&suite_path)
            .with_context(|| format!("reading suite {suite_path}"))?,
    )
    .context("parsing suite")?;

    let counter = TokenCounter::new(TokenMode::Tiktoken);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;

    let (mut full_tokens, mut compact_tokens) = (0u64, 0u64);
    let (mut answered, mut followups, mut errors) = (0u32, 0u32, 0u32);
    let mut rows: Vec<(String, u32, u32, bool, bool)> = Vec::new();

    for q in &suite.queries {
        let resp = client
            .post(format!("{base}/memory/search"))
            .bearer_auth(&token)
            .json(&serde_json::json!({ "query": q.text, "top_k": TOP_K }))
            .send()
            .await;
        let hits = match resp {
            Ok(r) if r.status().is_success() => match r.json::<SearchResponse>().await {
                Ok(p) => p.results,
                Err(e) => {
                    eprintln!("{}: parse: {e}", q.id);
                    errors += 1;
                    continue;
                }
            },
            Ok(r) => {
                eprintln!("{}: HTTP {}", q.id, r.status());
                errors += 1;
                continue;
            }
            Err(e) => {
                eprintln!("{}: {e}", q.id);
                errors += 1;
                continue;
            }
        };

        // Was the query answered at all? Judged against the FULL body,
        // because that is what the corpus can offer.
        let answering: Vec<&Hit> = hits
            .iter()
            .filter(|h| satisfies(h, &h.text(), &q.answers))
            .collect();
        let is_answered = !answering.is_empty();

        let full: u64 = hits
            .iter()
            .map(|h| u64::from(counter.count_json(&h.full_json())))
            .sum();

        let mut compact: u64 = hits
            .iter()
            .map(|h| u64::from(counter.count_json(&h.compact_json(&q.text))))
            .sum();

        // Charge the follow-up: if the query was answerable but NO
        // answering hit's snippet carried the answer, the agent pays
        // one `memory_get` for the best answering record.
        let snippet_carried = answering.iter().any(|h| {
            let snip = match_window(&h.text(), &q.text);
            satisfies(h, &snip, &q.answers)
        });
        if is_answered && !snippet_carried {
            if let Some(best) = answering.first() {
                compact += u64::from(counter.count_json(&best.full_json()));
                followups += 1;
            }
        }

        if is_answered {
            answered += 1;
        }
        full_tokens += full;
        compact_tokens += compact;
        rows.push((
            q.id.clone(),
            u32::try_from(full).unwrap_or(u32::MAX),
            u32::try_from(compact).unwrap_or(u32::MAX),
            is_answered,
            snippet_carried,
        ));
    }

    println!("suite: {suite_path}");
    println!("queries: {} · errors: {errors}", suite.queries.len());
    println!("answered: {answered} · follow-up reads charged: {followups}\n");
    println!(
        "{:<28} {:>10} {:>10} {:>9} {:>8}",
        "query", "full", "compact", "answered", "snippet"
    );
    for (id, f, c, a, s) in &rows {
        println!(
            "{:<28} {f:>10} {c:>10} {:>9} {:>8}",
            id,
            if *a { "yes" } else { "-" },
            if *a && *s {
                "carried"
            } else if *a {
                "fetch"
            } else {
                "-"
            },
        );
    }

    if answered == 0 {
        println!("\nno query was answered — nothing to normalise against");
        return Ok(());
    }
    let full_per = full_tokens / u64::from(answered);
    let compact_per = compact_tokens / u64::from(answered);
    println!("\ntokens per answered query");
    println!("  full text : {full_per}");
    println!("  compact   : {compact_per}");
    #[allow(clippy::cast_precision_loss)]
    let ratio = full_per as f64 / compact_per.max(1) as f64;
    println!("  reduction : {ratio:.2}x");
    // The class the WI cares about: conceptual recall, where snippets
    // are most likely to fall short.
    let conceptual = suite
        .queries
        .iter()
        .filter(|q| q.class.as_deref() == Some("conceptual"))
        .count();
    println!("\n(suite carries {conceptual} conceptual queries)");
    Ok(())
}
