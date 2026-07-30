//! Second-stage reranker client (sprint 030, WI #685).
//!
//! Wraps TEI's `POST /rerank` — a cross-encoder scoring `(query, text)`
//! pairs — served by the `reranker` compose service. The stage is
//! best-effort by contract: callers treat any error here as "skip the
//! stage", so this client makes exactly one attempt with a short
//! timeout rather than reusing the embedder's retry loop. A search
//! that waits three backoffs on a sick reranker would blow the entire
//! latency budget to improve an ordering the caller can already serve.

use crate::{StoreError, StoreResult};
use std::time::Duration;

/// Per-request ceiling. Measured on kubs0's 4080 SUPER: 50 pairs of
/// realistic chunk size score in ~110 ms, so anything past this is the
/// backend struggling, not normal variance.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// One reranked candidate: the index into the submitted `texts` and the
/// cross-encoder's relevance score (higher is more relevant).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RerankHit {
    pub index: usize,
    pub score: f32,
}

#[derive(serde::Deserialize)]
struct RerankResponseItem {
    index: usize,
    score: f32,
}

/// TEI `/rerank` client. Deliberately not part of the [`crate::Store`]
/// trait: the reranker is an optional retrieval stage, not a storage
/// backend, and absent config means the stage simply doesn't exist.
#[derive(Debug, Clone)]
pub struct TeiReranker {
    base_url: String,
    client: reqwest::Client,
}

impl TeiReranker {
    pub fn new(base_url: impl Into<String>) -> StoreResult<Self> {
        Ok(Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            client: reqwest::Client::builder()
                .timeout(REQUEST_TIMEOUT)
                .build()
                .map_err(|e| StoreError::Backend(format!("reranker client build: {e}")))?,
        })
    }

    /// Score `texts` against `query`; returns hits sorted best-first,
    /// ties broken by submission index so identical inputs always
    /// produce identical order (the 029 determinism invariant).
    ///
    /// One attempt, no retries — see the module doc. The server must be
    /// running with `--max-client-batch-size` >= `texts.len()`; the
    /// caller's rerank window (default 50, compose serves 64) keeps
    /// that true by construction.
    ///
    /// # Errors
    /// Any transport or HTTP failure, or a response that doesn't cover
    /// every submitted text exactly once.
    pub async fn rerank(&self, query: &str, texts: &[&str]) -> StoreResult<Vec<RerankHit>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let url = format!("{}/rerank", self.base_url);
        let body = serde_json::json!({ "query": query, "texts": texts });
        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| StoreError::Backend(format!("rerank send: {e}")))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(StoreError::Backend(format!(
                "rerank HTTP {status}: {}",
                body.trim()
            )));
        }
        let items: Vec<RerankResponseItem> = resp
            .json()
            .await
            .map_err(|e| StoreError::Backend(format!("rerank parse: {e}")))?;
        // A partial or duplicated index set would silently drop or
        // duplicate candidates when the caller applies the new order —
        // refuse it rather than rerank wrongly.
        let mut seen = vec![false; texts.len()];
        for item in &items {
            if item.index >= texts.len() || seen[item.index] {
                return Err(StoreError::Backend(format!(
                    "rerank returned invalid or duplicate index {} for {} texts",
                    item.index,
                    texts.len()
                )));
            }
            seen[item.index] = true;
        }
        if items.len() != texts.len() {
            return Err(StoreError::Backend(format!(
                "rerank returned {} scores for {} texts",
                items.len(),
                texts.len()
            )));
        }
        let mut hits: Vec<RerankHit> = items
            .into_iter()
            .map(|i| RerankHit {
                index: i.index,
                score: i.score,
            })
            .collect();
        hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.index.cmp(&b.index))
        });
        Ok(hits)
    }

    /// The configured base URL (sprint 036, #731 — the health probe's
    /// cache key).
    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Cheap liveness probe (mirrors [`crate::TeiEmbedder`]'s).
    ///
    /// # Errors
    /// Transport failure or a non-2xx status.
    pub async fn health(&self) -> StoreResult<()> {
        let url = format!("{}/health", self.base_url);
        let resp = self
            .client
            .get(&url)
            .timeout(Duration::from_secs(1))
            .send()
            .await
            .map_err(|e| StoreError::Backend(format!("reranker health: {e}")))?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(StoreError::Backend(format!(
                "reranker health HTTP {}",
                resp.status()
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_partial_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn rerank_parses_and_sorts_best_first() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/rerank"))
            .and(body_partial_json(serde_json::json!({
                "query": "q", "texts": ["a", "b", "c"]
            })))
            // TEI returns hits already sorted, but the client must not
            // depend on that — serve them shuffled.
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                { "index": 1, "score": 0.2 },
                { "index": 2, "score": 0.9 },
                { "index": 0, "score": 0.5 }
            ])))
            .expect(1)
            .mount(&server)
            .await;

        let r = TeiReranker::new(server.uri()).unwrap();
        let hits = r.rerank("q", &["a", "b", "c"]).await.unwrap();
        assert_eq!(
            hits.iter().map(|h| h.index).collect::<Vec<_>>(),
            vec![2, 0, 1]
        );
    }

    #[tokio::test]
    async fn rerank_ties_break_by_submission_index() {
        // Determinism (the 029 invariant): equal scores must not leave
        // the order to serializer or float whims.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/rerank"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                { "index": 2, "score": 0.5 },
                { "index": 0, "score": 0.5 },
                { "index": 1, "score": 0.5 }
            ])))
            .mount(&server)
            .await;

        let r = TeiReranker::new(server.uri()).unwrap();
        let hits = r.rerank("q", &["a", "b", "c"]).await.unwrap();
        assert_eq!(
            hits.iter().map(|h| h.index).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
    }

    #[tokio::test]
    async fn empty_texts_short_circuit_without_a_request() {
        let server = MockServer::start().await;
        // No mock mounted: a request would 404 and fail the test through
        // the error path below.
        let r = TeiReranker::new(server.uri()).unwrap();
        assert!(r.rerank("q", &[]).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_5xx_is_an_error_after_exactly_one_attempt() {
        // Best-effort contract: the caller skips the stage on error, so
        // retrying here only adds latency. Exactly ONE request.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/rerank"))
            .respond_with(ResponseTemplate::new(503).set_body_string("model loading"))
            .expect(1)
            .mount(&server)
            .await;

        let r = TeiReranker::new(server.uri()).unwrap();
        let err = r.rerank("q", &["a"]).await.unwrap_err();
        assert!(err.to_string().contains("model loading"), "{err}");
    }

    #[tokio::test]
    async fn a_short_response_is_refused() {
        // 2 texts, 1 score: applying that order would silently drop a
        // candidate.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/rerank"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!([{ "index": 0, "score": 0.9 }])),
            )
            .mount(&server)
            .await;

        let r = TeiReranker::new(server.uri()).unwrap();
        let err = r.rerank("q", &["a", "b"]).await.unwrap_err();
        assert!(err.to_string().contains("1 scores for 2 texts"), "{err}");
    }

    #[tokio::test]
    async fn an_out_of_range_or_duplicate_index_is_refused() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/rerank"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                { "index": 0, "score": 0.9 },
                { "index": 5, "score": 0.1 }
            ])))
            .mount(&server)
            .await;

        let r = TeiReranker::new(server.uri()).unwrap();
        let err = r.rerank("q", &["a", "b"]).await.unwrap_err();
        assert!(
            err.to_string().contains("invalid or duplicate index"),
            "{err}"
        );
    }

    /// Integration test against the live reranker compose service.
    /// Set `TEST_RERANKER_URL` to enable (e.g. `http://127.0.0.1:7071`).
    #[tokio::test]
    #[ignore = "requires TEST_RERANKER_URL pointing at a running TEI reranker"]
    async fn live_rerank_prefers_the_relevant_text() {
        let Ok(url) = std::env::var("TEST_RERANKER_URL") else {
            eprintln!("skipping live_rerank_prefers_the_relevant_text: TEST_RERANKER_URL not set");
            return;
        };
        let r = TeiReranker::new(url).unwrap();
        let hits = r
            .rerank(
                "what GPU does kubs0 have?",
                &[
                    "Postgres backups run nightly over NFS.",
                    "kubs0's GPU is an RTX 4080 SUPER with 16GB of VRAM.",
                    "To make pancakes, whisk flour, eggs and milk.",
                ],
            )
            .await
            .unwrap();
        assert_eq!(hits[0].index, 1, "the GPU text must rank first: {hits:?}");
    }
}
