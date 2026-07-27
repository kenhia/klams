//! Embedding clients.
//!
//! Sprint 014 — the `Embedder` trait decouples the store from any one
//! serving engine. Two implementations:
//!
//! * [`TeiEmbedder`] — Hugging Face Text Embeddings Inference native
//!   API (`POST /embed`). The production default on `kubs0`.
//! * [`OpenAiCompatEmbedder`] — the OpenAI-compatible surface
//!   (`POST {url}/embeddings`) served by vLLM, TEI's `/v1` route,
//!   Ollama, etc. Selected via `[embeddings] api = "openai"`.

use crate::{StoreError, StoreResult};
use async_trait::async_trait;
use klams_types::EmbedLimit;
use std::time::Duration;

const MAX_RETRIES: usize = 3;

/// The outcome of a non-2xx response from an embedding backend
/// (sprint 027, WI #629).
///
/// Before this existed, the retry loops treated *every* non-2xx as
/// worth another attempt and discarded the response body. A permanent
/// HTTP 413 therefore burned three round-trips proving it was still 413,
/// and the one actionable string TEI sends back — `inputs must have less
/// than 512 tokens` — was thrown away, leaving callers with a bare
/// `HTTP 413` they could not act on.
enum Failure {
    /// Stop now; another attempt cannot change the answer.
    Permanent(StoreError),
    /// Worth retrying; carries the message to report if attempts run out.
    Transient(String),
}

/// Classify a non-2xx embedder response, consuming its body.
///
/// * `413` — the input exceeds the model's sequence limit. Permanent,
///   and reported with the numbers a caller needs to split correctly.
/// * other `4xx` — the backend refused the request itself. Permanent.
/// * `5xx` and everything else — the backend is unwell, not the input.
///   Transient.
async fn classify(resp: reqwest::Response, limit: EmbedLimit, inputs: &[&str]) -> Failure {
    let status = resp.status();
    // The body is where the actionable detail lives; a failure to read
    // it must not mask the status we already have.
    let body = resp.text().await.unwrap_or_default();
    let body = body.trim();
    let detail = if body.is_empty() {
        format!("HTTP {status}")
    } else {
        format!("HTTP {status}: {body}")
    };

    // TEI ≤1.7 answers an over-limit input with 413; TEI 1.9 answers 422
    // `Input validation error: `inputs` must have less than N tokens`
    // (sprint 028, found by the calibration test). Both mean the same
    // permanent, caller-fixable thing — without the 422 arm the input
    // would be misreported as EMBEDDING_REJECTED, which the 027 error
    // contract reserves for "the backend refused the request itself",
    // and the split-and-retry guidance would be lost.
    let too_large = status == reqwest::StatusCode::PAYLOAD_TOO_LARGE
        || (status == reqwest::StatusCode::UNPROCESSABLE_ENTITY
            && body.contains("must have less than")
            && body.contains("tokens"));
    if too_large {
        // Report against the largest input — the one that provoked it.
        let worst = inputs.iter().max_by_key(|t| t.len()).copied().unwrap_or("");
        let oversize = limit.check(worst).err().unwrap_or_else(|| {
            // The backend's real ceiling is lower than ours is configured
            // to be. Say so honestly rather than inventing numbers.
            klams_types::Oversize {
                limit_tokens: limit.max_input_tokens(),
                estimated_tokens: klams_types::estimate_tokens(worst),
                submitted_chars: worst.chars().count(),
                max_chars: limit.max_chars(),
            }
        });
        return Failure::Permanent(StoreError::PayloadTooLarge { oversize, detail });
    }
    if status.is_client_error() {
        return Failure::Permanent(StoreError::EmbeddingRejected(detail));
    }
    Failure::Transient(detail)
}

/// A text-embedding backend. Object-safe so `CompositeStore` can hold
/// whichever engine the config selected.
#[async_trait]
pub trait Embedder: Send + Sync + std::fmt::Debug {
    /// Embed a single input; the returned vector length must equal
    /// [`Embedder::expected_dim`].
    async fn embed(&self, text: &str) -> StoreResult<Vec<f32>>;
    /// Embed many inputs in one request where the backend supports it —
    /// TEI `/embed` and the openai-compat `/embeddings` route both accept
    /// an array (sprint 022 #325, the batch path needed for a bulk
    /// re-embed). Returns one vector per input, in input order. The
    /// default falls back to sequential [`Embedder::embed`] so simple
    /// or mock backends need not implement it.
    async fn embed_batch(&self, texts: &[String]) -> StoreResult<Vec<Vec<f32>>> {
        let mut out = Vec::with_capacity(texts.len());
        for t in texts {
            out.push(self.embed(t).await?);
        }
        Ok(out)
    }
    /// Exact input-token counts for `texts`, if this backend can
    /// produce them (sprint 027, WI #420).
    ///
    /// The default returns [`klams_types::estimate_tokens`], a
    /// conservative character-based approximation. That default is a
    /// fallback, not the intended path: measured against the live model,
    /// no character ratio is simultaneously *safe* (never under-counts)
    /// and *useful* (leaves an 800-character prose chunk alone). Real
    /// content ranges from ~1.0 chars/token for punctuation-dense text
    /// to ~39 for base64, so any single divisor is wrong somewhere. A
    /// backend that can tokenize should say so here.
    async fn count_tokens(&self, texts: &[&str]) -> StoreResult<Vec<usize>> {
        Ok(texts
            .iter()
            .map(|t| klams_types::estimate_tokens(t))
            .collect())
    }

    /// Cheap liveness probe for the health handler (≤1s).
    async fn health(&self) -> StoreResult<()>;
    fn expected_dim(&self) -> usize;
}

fn build_http_client() -> StoreResult<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        // A client that cannot be built will never build; permanent.
        .map_err(|e| StoreError::Backend(format!("embedder client build: {e}")))
}

/// Validate a batch response: exactly `count` vectors, each of the
/// expected dimension (sprint 022 #325).
///
/// A mismatch means the configured `vector_dim` disagrees with the model
/// the backend is actually serving — a deployment fault that will repeat
/// on every call, so it is permanent (sprint 027).
fn check_dims(vectors: Vec<Vec<f32>>, count: usize, expected: usize) -> StoreResult<Vec<Vec<f32>>> {
    if vectors.len() != count {
        return Err(StoreError::Backend(format!(
            "embedder returned {} vectors, expected {count}",
            vectors.len()
        )));
    }
    for v in &vectors {
        if v.len() != expected {
            return Err(StoreError::Backend(format!(
                "embedder returned dim {}, expected dim {expected}",
                v.len()
            )));
        }
    }
    Ok(vectors)
}

/// Refuse inputs that *cannot* fit, before spending a round-trip on them
/// (sprint 027, WI #420/#629).
///
/// Uses [`EmbedLimit::certainly_exceeds`], not the estimate: a rejection
/// here is final, and the estimate over-counts token-efficient content
/// (base64 runs ~39 characters per token, where the estimate assumes 3).
/// Refusing on an estimate would turn this backstop into a *new* source
/// of lost writes — exactly the failure it exists to prevent.
///
/// Anything that slips past this bound is still safe: TEI's own 413 is
/// classified as permanent by [`classify`], so it costs one round-trip
/// and yields the same honest error.
fn preflight(inputs: &[&str], limit: EmbedLimit) -> StoreResult<()> {
    for (i, text) in inputs.iter().enumerate() {
        if let Some(oversize) = limit.certainly_exceeds(text) {
            return Err(StoreError::PayloadTooLarge {
                oversize,
                detail: format!("input {i} of {} rejected before dispatch", inputs.len()),
            });
        }
    }
    Ok(())
}

/// Build the [`Oversize`](klams_types::Oversize) describing a text whose
/// **exact** token count exceeds the ceiling (sprint 027, #420).
///
/// Used wherever a real tokenizer is available, so the numbers reported
/// to the caller are the model's own rather than a character estimate.
#[must_use]
pub fn oversize_from_exact_count(
    text: &str,
    tokens: usize,
    limit: EmbedLimit,
) -> klams_types::Oversize {
    klams_types::Oversize {
        limit_tokens: limit.max_input_tokens(),
        estimated_tokens: tokens,
        submitted_chars: text.chars().count(),
        // Scale the caller's own text to the budget: with the exact
        // count in hand, its measured density is a far better guide than
        // any global constant. Floored so the advice is never larger
        // than the conservative character bound.
        max_chars: text
            .chars()
            .count()
            .saturating_mul(limit.max_input_tokens())
            .checked_div(tokens.max(1))
            .unwrap_or(0)
            .min(limit.max_chars()),
    }
}

// ---------------------------------------------------------------------
// TEI (native API)

#[derive(Debug, Clone)]
pub struct TeiEmbedder {
    base_url: String,
    client: reqwest::Client,
    expected_dim: usize,
    limit: EmbedLimit,
}

impl TeiEmbedder {
    pub fn new(base_url: impl Into<String>, expected_dim: usize) -> StoreResult<Self> {
        Ok(Self {
            base_url: base_url.into(),
            client: build_http_client()?,
            expected_dim,
            limit: EmbedLimit::default(),
        })
    }

    /// Set the model's input ceiling (sprint 027). Defaults to
    /// [`klams_types::DEFAULT_MAX_INPUT_TOKENS`], the deployed
    /// bge-small-en-v1.5 limit; sprint 028's longer-context model sets
    /// it from config instead.
    #[must_use]
    pub fn with_limit(mut self, limit: EmbedLimit) -> Self {
        self.limit = limit;
        self
    }

    /// The configured ceiling, so ingest paths can gate against exactly
    /// what the embedder will enforce.
    pub fn limit(&self) -> EmbedLimit {
        self.limit
    }
}

impl TeiEmbedder {
    /// One `POST /embed` with N inputs → N vectors, in order. Backs both
    /// [`Embedder::embed`] and [`Embedder::embed_batch`].
    async fn request(&self, inputs: &[&str]) -> StoreResult<Vec<Vec<f32>>> {
        preflight(inputs, self.limit)?;
        let url = format!("{}/embed", self.base_url.trim_end_matches('/'));
        let body = serde_json::json!({ "inputs": inputs });

        let mut last_err = String::new();
        let mut backoff = Duration::from_millis(200);
        for attempt in 0..MAX_RETRIES {
            match self.client.post(&url).json(&body).send().await {
                Ok(resp) => {
                    if resp.status().is_success() {
                        let bytes = resp
                            .bytes()
                            .await
                            .map_err(|e| StoreError::Embedding(format!("read body: {e}")))?;
                        let vectors: Vec<Vec<f32>> = serde_json::from_slice(&bytes)
                            // A 2xx we cannot parse is a contract break,
                            // not a blip; retrying will not help.
                            .map_err(|e| StoreError::Backend(format!("TEI parse: {e}")))?;
                        return check_dims(vectors, inputs.len(), self.expected_dim);
                    }
                    // Sprint 027 (#629): stop retrying what cannot succeed.
                    match classify(resp, self.limit, inputs).await {
                        Failure::Permanent(e) => return Err(e),
                        Failure::Transient(msg) => last_err = msg,
                    }
                }
                Err(e) => last_err = format!("send: {e}"),
            }
            if attempt + 1 < MAX_RETRIES {
                tokio::time::sleep(backoff).await;
                backoff *= 2;
            }
        }
        Err(StoreError::Embedding(format!(
            "TEI request failed after {MAX_RETRIES} attempts: {last_err}"
        )))
    }
}

#[async_trait]
impl Embedder for TeiEmbedder {
    async fn embed(&self, text: &str) -> StoreResult<Vec<f32>> {
        self.request(&[text])
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| StoreError::Embedding("empty vectors response".into()))
    }

    async fn embed_batch(&self, texts: &[String]) -> StoreResult<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
        self.request(&refs).await
    }

    /// Exact token counts from TEI's `POST /tokenize` (sprint 027).
    ///
    /// This is the authoritative answer to "will the model accept this",
    /// and it is cheap — `/tokenize` runs the tokenizer only, with no
    /// model forward pass — so gating on it costs far less than the
    /// failed embed call it replaces. The returned array includes the
    /// `[CLS]`/`[SEP]` special tokens, which count against
    /// `max_input_length`, so the counts are directly comparable to it.
    ///
    /// Falls back to the character estimate if the route is unavailable
    /// (older TEI builds): a gate that degrades to approximate is much
    /// better than one that fails the write.
    async fn count_tokens(&self, texts: &[&str]) -> StoreResult<Vec<usize>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let url = format!("{}/tokenize", self.base_url.trim_end_matches('/'));
        let body = serde_json::json!({ "inputs": texts });
        let resp = match self.client.post(&url).json(&body).send().await {
            Ok(r) if r.status().is_success() => r,
            other => {
                tracing::debug!(
                    status = ?other.map(|r| r.status()),
                    "TEI /tokenize unavailable; falling back to the character estimate"
                );
                return Ok(texts
                    .iter()
                    .map(|t| klams_types::estimate_tokens(t))
                    .collect());
            }
        };
        let parsed: Vec<Vec<serde_json::Value>> = resp
            .json()
            .await
            .map_err(|e| StoreError::Backend(format!("TEI tokenize parse: {e}")))?;
        if parsed.len() != texts.len() {
            return Err(StoreError::Backend(format!(
                "TEI tokenize returned {} results, expected {}",
                parsed.len(),
                texts.len()
            )));
        }
        Ok(parsed.into_iter().map(|toks| toks.len()).collect())
    }

    /// Hits the TEI `/health` endpoint with a 1s timeout so the health
    /// handler never blocks for long.
    async fn health(&self) -> StoreResult<()> {
        let url = format!("{}/health", self.base_url.trim_end_matches('/'));
        let resp = self
            .client
            .get(&url)
            .timeout(Duration::from_secs(1))
            .send()
            .await
            .map_err(|e| StoreError::Embedding(format!("tei health: {e}")))?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(StoreError::Embedding(format!(
                "tei health HTTP {}",
                resp.status()
            )))
        }
    }

    fn expected_dim(&self) -> usize {
        self.expected_dim
    }
}

// ---------------------------------------------------------------------
// OpenAI-compatible (vLLM, TEI `/v1`, Ollama `/v1`, …)

/// `base_url` is the OpenAI-compat base **including** the version
/// segment, e.g. `http://127.0.0.1:7070/v1` (TEI) or
/// `http://kai:8000/v1` (vLLM). Endpoints used: `POST {base}/embeddings`,
/// `GET {base}/models`.
#[derive(Debug, Clone)]
pub struct OpenAiCompatEmbedder {
    base_url: String,
    model: String,
    client: reqwest::Client,
    expected_dim: usize,
    api_key: Option<String>,
    limit: EmbedLimit,
}

#[derive(serde::Deserialize)]
struct EmbeddingsResponse {
    data: Vec<EmbeddingsDatum>,
}

#[derive(serde::Deserialize)]
struct EmbeddingsDatum {
    embedding: Vec<f32>,
}

impl OpenAiCompatEmbedder {
    pub fn new(
        base_url: impl Into<String>,
        model: impl Into<String>,
        expected_dim: usize,
        api_key: Option<String>,
    ) -> StoreResult<Self> {
        Ok(Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            model: model.into(),
            client: build_http_client()?,
            expected_dim,
            api_key,
            limit: EmbedLimit::default(),
        })
    }

    /// Set the model's input ceiling — see [`TeiEmbedder::with_limit`].
    #[must_use]
    pub fn with_limit(mut self, limit: EmbedLimit) -> Self {
        self.limit = limit;
        self
    }

    /// The configured ceiling, so ingest paths can gate against exactly
    /// what the embedder will enforce.
    pub fn limit(&self) -> EmbedLimit {
        self.limit
    }

    fn authed(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.api_key {
            Some(k) => req.bearer_auth(k),
            None => req,
        }
    }
}

impl OpenAiCompatEmbedder {
    /// One `POST {base}/embeddings` with N inputs → N vectors, in the
    /// `data` array order. Backs both [`Embedder::embed`] and
    /// [`Embedder::embed_batch`].
    async fn request(&self, inputs: &[&str]) -> StoreResult<Vec<Vec<f32>>> {
        preflight(inputs, self.limit)?;
        let url = format!("{}/embeddings", self.base_url);
        let body = serde_json::json!({ "model": self.model, "input": inputs });

        let mut last_err = String::new();
        let mut backoff = Duration::from_millis(200);
        for attempt in 0..MAX_RETRIES {
            match self.authed(self.client.post(&url).json(&body)).send().await {
                Ok(resp) => {
                    if resp.status().is_success() {
                        let bytes = resp
                            .bytes()
                            .await
                            .map_err(|e| StoreError::Embedding(format!("read body: {e}")))?;
                        let parsed: EmbeddingsResponse = serde_json::from_slice(&bytes)
                            // See the TEI path: an unparseable 2xx is a
                            // contract break, not a transient blip.
                            .map_err(|e| StoreError::Backend(format!("embeddings parse: {e}")))?;
                        let vectors: Vec<Vec<f32>> =
                            parsed.data.into_iter().map(|d| d.embedding).collect();
                        return check_dims(vectors, inputs.len(), self.expected_dim);
                    }
                    // Sprint 027 (#629): the OpenAI-compat path had the
                    // identical retry-everything bug, and it becomes the
                    // primary path if 028 serves the new model via vLLM.
                    match classify(resp, self.limit, inputs).await {
                        Failure::Permanent(e) => return Err(e),
                        Failure::Transient(msg) => last_err = msg,
                    }
                }
                Err(e) => last_err = format!("send: {e}"),
            }
            if attempt + 1 < MAX_RETRIES {
                tokio::time::sleep(backoff).await;
                backoff *= 2;
            }
        }
        Err(StoreError::Embedding(format!(
            "embeddings request failed after {MAX_RETRIES} attempts: {last_err}"
        )))
    }
}

#[async_trait]
impl Embedder for OpenAiCompatEmbedder {
    async fn embed(&self, text: &str) -> StoreResult<Vec<f32>> {
        self.request(&[text])
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| StoreError::Embedding("empty data response".into()))
    }

    async fn embed_batch(&self, texts: &[String]) -> StoreResult<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
        self.request(&refs).await
    }

    /// Probes `GET {base}/models` with a 1s timeout. Any 2xx counts as
    /// alive; model presence is not asserted here (some engines gate
    /// the list behind auth scopes the embed path doesn't need).
    async fn health(&self) -> StoreResult<()> {
        let url = format!("{}/models", self.base_url);
        let resp = self
            .authed(self.client.get(&url).timeout(Duration::from_secs(1)))
            .send()
            .await
            .map_err(|e| StoreError::Embedding(format!("openai-compat health: {e}")))?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(StoreError::Embedding(format!(
                "openai-compat health HTTP {}",
                resp.status()
            )))
        }
    }

    fn expected_dim(&self) -> usize {
        self.expected_dim
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_partial_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Integration test against the live TEI compose service.
    /// Set `TEST_TEI_URL` to enable; defaults are off.
    #[tokio::test]
    #[ignore = "requires TEST_TEI_URL pointing at a running TEI server"]
    async fn tei_embed_returns_configured_dim_vector() {
        // Self-skip when the endpoint isn't configured. `cargo test
        // --ignored` (main CI) runs every ignored test regardless of
        // which env vars are present, so a bare `.expect()` here would
        // panic the whole run when the var is absent (sprint 022 CI fix).
        let Ok(url) = std::env::var("TEST_TEI_URL") else {
            eprintln!("skipping tei_embed_returns_configured_dim_vector: TEST_TEI_URL not set");
            return;
        };
        // Sprint 028: probe the served model's dim instead of pinning
        // 384 — the test is about the client, not about which model the
        // operator has loaded today.
        let dim = probe_tei_dim(&url).await;
        let embedder = TeiEmbedder::new(url, dim).unwrap();
        let v = embedder.embed("hello world").await.unwrap();
        assert_eq!(v.len(), dim);
        // Batch path (#325): N inputs → N vectors of the right dim.
        let batch = embedder
            .embed_batch(&["hello world".to_string(), "second input".to_string()])
            .await
            .unwrap();
        assert_eq!(batch.len(), 2);
        assert!(batch.iter().all(|v| v.len() == dim));
    }

    /// Live-TEI helpers for the ignored integration tests: the served
    /// model's embedding dim and its `max_input_length` from `/info`.
    async fn probe_tei_dim(url: &str) -> usize {
        let resp: Vec<Vec<f32>> = reqwest::Client::new()
            .post(format!("{}/embed", url.trim_end_matches('/')))
            .json(&serde_json::json!({"inputs": ["probe"]}))
            .send()
            .await
            .expect("TEI /embed probe")
            .json()
            .await
            .expect("TEI /embed probe body");
        resp[0].len()
    }

    async fn probe_tei_max_input_length(url: &str) -> usize {
        let info: serde_json::Value = reqwest::get(format!("{}/info", url.trim_end_matches('/')))
            .await
            .expect("TEI /info")
            .json()
            .await
            .expect("TEI /info body");
        usize::try_from(info["max_input_length"].as_u64().expect("max_input_length")).unwrap()
    }

    /// Sprint 027 (#420) — the calibration test.
    ///
    /// The gate is only worth anything if it agrees with the model about
    /// what the model will accept. This asserts exactly that, across the
    /// content shapes whose token densities differ by more than 30×: for
    /// each, the token count we gate on must predict TEI's real
    /// accept/reject decision.
    ///
    /// This is the test that found the original design wrong. A
    /// character-ratio estimate passed unit tests happily while
    /// disagreeing with the live model on URLs, minified JSON, and
    /// punctuation-dense text — the exact "dense content" #420 named.
    #[tokio::test]
    #[ignore = "requires TEST_TEI_URL pointing at a running TEI server"]
    async fn token_counts_predict_what_tei_accepts() {
        let Ok(url) = std::env::var("TEST_TEI_URL") else {
            eprintln!("skipping token_counts_predict_what_tei_accepts: TEST_TEI_URL not set");
            return;
        };
        // Sprint 028: model-agnostic for real. The 027 version pinned
        // bge-small's dim (384) and ceiling (512), so the fixtures all
        // straddled 512 tokens — under a 32k-ceiling model every one of
        // them trivially fits and the test asserts nothing. Probe the
        // served model's dim and ceiling, then SCALE each shape so one
        // variant sits under the ceiling and one over it, and assert the
        // gate's verdict matches TEI's real accept/reject on both.
        let dim = probe_tei_dim(&url).await;
        let ceiling = probe_tei_max_input_length(&url).await;
        let limit = EmbedLimit::new(ceiling);
        // `.with_limit` matters: without it the embedder's own preflight
        // keeps the 512-token default and rejects before dispatch, and
        // the test measures the wrong gate.
        let e = TeiEmbedder::new(url, dim).unwrap().with_limit(limit);

        let units: Vec<(&str, &str)> = vec![
            ("punctuation", "!@#$%^&*(){}[]<>?/|~`+=_-;:'\",. "),
            ("minified json", r#"{"key":"value","n":123,"ok":true},"#),
            ("urls", "https://kubs0.encke-wahoo.ts.net:7777/mcp?x=1&y=2 "),
            ("markdown table", "| col_a | col_b | 12.5 | yes |\n"),
            ("rust code", "let x = compute(alpha, beta); // note\n"),
            (
                "english prose",
                "the homelab runs klams on kubs0 behind tailscale. ",
            ),
            ("cjk", "日本語のテキストです"),
            ("base64", "YWJjZGVmZ2hpamtsbW5vcHFyc3R1dnd4eXoxMjM0NTY3ODkw"),
        ];

        for (name, unit) in units {
            // Amortized tokens-per-16-units (special tokens spread out).
            let sample = unit.repeat(16);
            let per16 = e.count_tokens(&[sample.as_str()]).await.unwrap()[0].max(1);

            // One variant aimed under the ceiling, one aimed over; the
            // aim only picks the fixtures — the ASSERTION below uses the
            // real counted number, so an off-target aim loses coverage,
            // never correctness. Integer math: reps ≈ ratio·ceiling·16/per16.
            let reps_under = (ceiling * 9 / 10 * 16 / per16).max(1);
            let mut reps_over = ceiling * 11 / 10 * 16 / per16 + 1;
            let mut over = unit.repeat(reps_over);
            // Guarantee the over-variant is genuinely over.
            while e.count_tokens(&[over.as_str()]).await.unwrap()[0] <= ceiling {
                reps_over *= 2;
                over = unit.repeat(reps_over);
            }

            for text in [unit.repeat(reps_under), over] {
                let counted = e.count_tokens(&[text.as_str()]).await.unwrap()[0];
                let gate_says_fits = counted <= limit.max_input_tokens();
                let tei_accepts = match e.embed(&text).await {
                    Ok(_) => true,
                    Err(StoreError::PayloadTooLarge { .. }) => false,
                    Err(other) => panic!("{name}: unexpected error {other:?}"),
                };
                assert_eq!(
                    gate_says_fits,
                    tei_accepts,
                    "{name}: gate said fits={gate_says_fits} (counted {counted} tokens against \
                     a {}-token limit) but TEI accepts={tei_accepts} for {} chars",
                    limit.max_input_tokens(),
                    text.chars().count(),
                );
            }
        }
    }

    /// Integration test against a live OpenAI-compatible endpoint
    /// (e.g. TEI's `/v1` route: `TEST_OPENAI_EMBED_URL=http://127.0.0.1:7070/v1`).
    #[tokio::test]
    #[ignore = "requires TEST_OPENAI_EMBED_URL pointing at an OpenAI-compat /v1 base"]
    async fn openai_embed_returns_configured_dim_vector() {
        // Self-skip when unset — see the note on the TEI test above.
        let Ok(url) = std::env::var("TEST_OPENAI_EMBED_URL") else {
            eprintln!(
                "skipping openai_embed_returns_configured_dim_vector: TEST_OPENAI_EMBED_URL not set"
            );
            return;
        };
        let model = std::env::var("TEST_OPENAI_EMBED_MODEL")
            .unwrap_or_else(|_| "BAAI/bge-small-en-v1.5".into());
        let embedder = OpenAiCompatEmbedder::new(url, model, 384, None).unwrap();
        let v = embedder.embed("hello world").await.unwrap();
        assert_eq!(v.len(), 384);
    }

    #[tokio::test]
    async fn openai_embed_parses_response_and_sends_model() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/embeddings"))
            .and(body_partial_json(
                serde_json::json!({ "model": "test-model", "input": ["hi"] }),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "object": "list",
                "data": [ { "object": "embedding", "embedding": [0.1, 0.2, 0.3] } ]
            })))
            .expect(1)
            .mount(&server)
            .await;

        let e = OpenAiCompatEmbedder::new(format!("{}/v1", server.uri()), "test-model", 3, None)
            .unwrap();
        let v = e.embed("hi").await.unwrap();
        assert_eq!(v, vec![0.1, 0.2, 0.3]);
    }

    #[tokio::test]
    async fn openai_embed_batch_sends_all_inputs_and_parses_in_order() {
        // Sprint 022 (#325): one request carries every input; the `data`
        // array maps back to inputs in order.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/embeddings"))
            .and(body_partial_json(
                serde_json::json!({ "model": "m", "input": ["a", "b"] }),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "object": "list",
                "data": [
                    { "object": "embedding", "embedding": [1.0, 0.0] },
                    { "object": "embedding", "embedding": [0.0, 1.0] }
                ]
            })))
            .expect(1)
            .mount(&server)
            .await;

        let e = OpenAiCompatEmbedder::new(format!("{}/v1", server.uri()), "m", 2, None).unwrap();
        let out = e
            .embed_batch(&["a".to_string(), "b".to_string()])
            .await
            .unwrap();
        assert_eq!(out, vec![vec![1.0, 0.0], vec![0.0, 1.0]]);
    }

    #[tokio::test]
    async fn embed_batch_empty_is_empty() {
        let server = MockServer::start().await;
        // No mount: an empty batch must short-circuit without an HTTP call.
        let e = OpenAiCompatEmbedder::new(format!("{}/v1", server.uri()), "m", 2, None).unwrap();
        assert!(e.embed_batch(&[]).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn openai_embed_rejects_dim_mismatch() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/embeddings"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [ { "embedding": [0.1, 0.2] } ]
            })))
            .mount(&server)
            .await;

        let e = OpenAiCompatEmbedder::new(format!("{}/v1", server.uri()), "m", 3, None).unwrap();
        let err = e.embed("hi").await.unwrap_err();
        assert!(err.to_string().contains("expected dim 3"), "{err}");
    }

    #[tokio::test]
    async fn openai_embed_sends_bearer_when_key_configured() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/embeddings"))
            .and(header("authorization", "Bearer sk-test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [ { "embedding": [1.0] } ]
            })))
            .expect(1)
            .mount(&server)
            .await;

        let e = OpenAiCompatEmbedder::new(
            format!("{}/v1", server.uri()),
            "m",
            1,
            Some("sk-test".into()),
        )
        .unwrap();
        assert_eq!(e.embed("hi").await.unwrap(), vec![1.0]);
    }

    #[tokio::test]
    async fn openai_embed_retries_5xx_then_succeeds() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/embeddings"))
            .respond_with(ResponseTemplate::new(500))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/embeddings"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [ { "embedding": [1.0] } ]
            })))
            .mount(&server)
            .await;

        let e = OpenAiCompatEmbedder::new(format!("{}/v1", server.uri()), "m", 1, None).unwrap();
        assert_eq!(e.embed("hi").await.unwrap(), vec![1.0]);
    }

    // -----------------------------------------------------------------
    // Sprint 027 (WI #629) — the retry/classification regression tests.
    //
    // Per the review: "this exact test would have caught the bug
    // pre-merge." A 413 is permanent; retrying it three times is pure
    // latency and produced the ~90k wasted TEI round-trips behind #420.
    //
    // Sprint 031 (#646) asked for a hermetic `embed_does_not_retry_4xx`
    // to sit beside the 5xx one. It is already here and has been since
    // 027 — `does_not_retry_other_4xx` (generic 400 → EmbeddingRejected)
    // and `openai_does_not_retry_413` / `tei_does_not_retry_413` (the
    // one 4xx that means "split and retry"), each pinned to exactly ONE
    // request. The WI text predates them; writing a third near-identical
    // copy would have been the duplication this sprint is removing.

    #[tokio::test]
    async fn tei_does_not_retry_413() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/embed"))
            .respond_with(
                ResponseTemplate::new(413)
                    .set_body_string("inputs must have less than 512 tokens. Given: 733"),
            )
            // The assertion that matters: exactly ONE request, not three.
            .expect(1)
            .mount(&server)
            .await;

        let e = TeiEmbedder::new(server.uri(), 384).unwrap();
        let err = e
            .embed("short enough to clear the local gate")
            .await
            .unwrap_err();

        assert!(
            matches!(err, StoreError::PayloadTooLarge { .. }),
            "expected PayloadTooLarge, got {err:?}"
        );
        assert!(!err.is_transient(), "a 413 must never look retryable");
        // The response body carries the only actionable string TEI sends;
        // discarding it was half the bug.
        assert!(
            err.to_string().contains("must have less than 512 tokens"),
            "TEI's response body was discarded: {err}"
        );
    }

    /// Sprint 028: TEI 1.9 reports an over-limit input as **422** with an
    /// `Input validation error` body, where ≤1.7 used 413. It must still
    /// classify as `PayloadTooLarge` — misfiling it as
    /// `EmbeddingRejected` loses the split-and-retry guidance the 027
    /// error contract promises.
    #[tokio::test]
    async fn tei_19_422_token_validation_is_payload_too_large() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/embed"))
            .respond_with(ResponseTemplate::new(422).set_body_string(
                r#"{"error":"Input validation error: `inputs` must have less than 32768 tokens. Given: 35849","error_type":"Validation"}"#,
            ))
            .expect(1)
            .mount(&server)
            .await;

        let e = TeiEmbedder::new(server.uri(), 384).unwrap();
        let err = e.embed("short input").await.unwrap_err();
        assert!(
            matches!(err, StoreError::PayloadTooLarge { .. }),
            "TEI 1.9's 422 must classify as PayloadTooLarge, got {err:?}"
        );
        assert!(!err.is_transient());
    }

    /// A 422 that is NOT the token-limit shape stays `EmbeddingRejected`
    /// — only the over-limit validation error gets the 413 treatment.
    #[tokio::test]
    async fn tei_other_422_stays_embedding_rejected() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/embed"))
            .respond_with(
                ResponseTemplate::new(422)
                    .set_body_string(r#"{"error":"Input validation error: `inputs` cannot be empty","error_type":"Validation"}"#),
            )
            .expect(1)
            .mount(&server)
            .await;

        let e = TeiEmbedder::new(server.uri(), 384).unwrap();
        let err = e.embed("x").await.unwrap_err();
        assert!(
            matches!(err, StoreError::EmbeddingRejected(_)),
            "non-token 422 must stay EmbeddingRejected, got {err:?}"
        );
    }

    #[tokio::test]
    async fn openai_does_not_retry_413() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/embeddings"))
            .respond_with(ResponseTemplate::new(413).set_body_string("input too long"))
            .expect(1)
            .mount(&server)
            .await;

        let e = OpenAiCompatEmbedder::new(format!("{}/v1", server.uri()), "m", 1, None).unwrap();
        let err = e.embed("hi").await.unwrap_err();
        assert!(matches!(err, StoreError::PayloadTooLarge { .. }), "{err:?}");
        assert!(!err.is_transient());
    }

    #[tokio::test]
    async fn does_not_retry_other_4xx() {
        // A 400 is the backend refusing the request itself: also
        // permanent, but not a size problem, so it must not masquerade
        // as one.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/embeddings"))
            .respond_with(ResponseTemplate::new(400).set_body_string("malformed input"))
            .expect(1)
            .mount(&server)
            .await;

        let e = OpenAiCompatEmbedder::new(format!("{}/v1", server.uri()), "m", 1, None).unwrap();
        let err = e.embed("hi").await.unwrap_err();
        assert!(matches!(err, StoreError::EmbeddingRejected(_)), "{err:?}");
        assert!(!err.is_transient());
        assert!(err.to_string().contains("malformed input"), "{err}");
    }

    #[tokio::test]
    async fn still_retries_5xx_and_reports_it_as_transient() {
        // The other direction of the taxonomy: a 503 IS worth retrying,
        // and when attempts run out it must still read as transient so
        // callers get an honest retry hint.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/embeddings"))
            .respond_with(ResponseTemplate::new(503).set_body_string("model loading"))
            .expect(MAX_RETRIES as u64)
            .mount(&server)
            .await;

        let e = OpenAiCompatEmbedder::new(format!("{}/v1", server.uri()), "m", 1, None).unwrap();
        let err = e.embed("hi").await.unwrap_err();
        assert!(err.is_transient(), "a 503 must stay retryable: {err:?}");
        assert!(err.to_string().contains("model loading"), "{err}");
    }

    #[tokio::test]
    async fn oversized_input_never_reaches_the_wire() {
        // The preflight gate (#420): a text we already know exceeds the
        // model's ceiling costs zero round-trips, and the error names
        // the limit and the submitted size so the caller can split on
        // the first retry instead of bisecting.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/embed"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0) // no request at all
            .mount(&server)
            .await;

        let e = TeiEmbedder::new(server.uri(), 384).unwrap();
        // Many words: the provable floor (one token per whitespace-
        // separated word) already exceeds 512, so no request is needed
        // to know this cannot fit.
        let huge = "word ".repeat(2_000);
        let err = e.embed(&huge).await.unwrap_err();

        let StoreError::PayloadTooLarge { oversize, .. } = &err else {
            panic!("expected PayloadTooLarge, got {err:?}");
        };
        assert_eq!(oversize.submitted_chars, huge.chars().count());
        assert_eq!(oversize.limit_tokens, klams_types::DEFAULT_MAX_INPUT_TOKENS);
    }

    #[tokio::test]
    async fn preflight_never_rejects_token_efficient_text() {
        // The regression guarding the fix above: `"a".repeat(10_000)` is
        // one enormous word that the real model tokenizes to a few
        // hundred tokens and accepts. The character *estimate* refuses it
        // outright, so a preflight built on the estimate would invent a
        // brand-new class of lost writes — the very thing this sprint is
        // closing. Preflight must reject only what provably cannot fit.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/embed"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([[1.0]])))
            .expect(1)
            .mount(&server)
            .await;

        let e = TeiEmbedder::new(server.uri(), 1).unwrap();
        let one_long_word = "a".repeat(10_000);
        assert!(
            !EmbedLimit::default().fits(&one_long_word),
            "fixture must be one the estimate rejects"
        );
        assert_eq!(e.embed(&one_long_word).await.unwrap(), vec![1.0]);
    }

    #[tokio::test]
    async fn configured_limit_overrides_the_default() {
        // Sprint 028 swaps in a longer-context model; the ceiling has to
        // be a config change, not a code change.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/embed"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([[1.0]])))
            .expect(1)
            .mount(&server)
            .await;

        let e = TeiEmbedder::new(server.uri(), 1)
            .unwrap()
            .with_limit(EmbedLimit::new(8192));
        // Comfortably over the 512-token default, comfortably under 8192.
        let text = "word ".repeat(500);
        assert_eq!(e.embed(&text).await.unwrap(), vec![1.0]);
    }

    #[tokio::test]
    async fn openai_health_probes_models_endpoint() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "object": "list", "data": []
            })))
            .expect(1)
            .mount(&server)
            .await;

        let e = OpenAiCompatEmbedder::new(format!("{}/v1", server.uri()), "m", 1, None).unwrap();
        e.health().await.unwrap();
    }
}
