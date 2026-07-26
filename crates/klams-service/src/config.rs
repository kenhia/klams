//! Runtime configuration for klams-service.

use figment::providers::{Env, Format, Toml};
use figment::Figment;
use klams_types::FactType;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("config load: {0}")]
    Load(#[from] Box<figment::Error>),
    #[error("decay lambda for type `{type_}` must be >= 0, got {value}")]
    DecayLambdaNegative { type_: String, value: f32 },
    #[error("decay lambda for type `{type_}` must be finite")]
    DecayLambdaNonFinite { type_: String },
    #[error("decay config references unknown FactType `{type_}`")]
    DecayUnknownType { type_: String },
    #[error(
        "retrieval fusion strategy `{value}` is not recognized (expected \"rrf\" or \"weighted\")"
    )]
    RetrievalFusionUnknown { value: String },
    #[error("summarization.llm_url `{value}` is not a valid URL: {source}")]
    SummarizationLlmUrlInvalid {
        value: String,
        #[source]
        source: url::ParseError,
    },
    #[error("service.limits.{key} out of range: got {value}, allowed {min}..={max}")]
    InvalidLimit {
        key: &'static str,
        value: u64,
        min: u64,
        max: u64,
    },
}

impl From<figment::Error> for ConfigError {
    fn from(e: figment::Error) -> Self {
        ConfigError::Load(Box::new(e))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub server: ServerConfig,
    pub auth: AuthConfig,
    pub postgres: PostgresConfig,
    pub qdrant: QdrantConfig,
    pub embeddings: EmbeddingsConfig,
    pub queue: QueueConfig,
    pub logging: LoggingConfig,
    #[serde(default)]
    pub decay: DecayConfig,
    #[serde(default)]
    pub retrieval: RetrievalConfig,
    #[serde(default)]
    pub tokens: TokensConfig,
    #[serde(default)]
    pub summarization: SummarizationConfig,
    /// Sprint 006 — nightly backup feature. Default `enabled=false`,
    /// so a config without a `[backup]` block is unaffected.
    #[serde(default)]
    pub backup: BackupConfig,
    /// Sprint 008 — shared API knobs (memories window cap, etc.).
    /// Default values preserve sprint-007 behavior for configs that
    /// omit `[api]`.
    #[serde(default)]
    pub api: ApiConfig,
    /// Sprint 009 — service-wide knobs (currently just connection
    /// limits). Optional; missing `[service]` block applies all
    /// defaults.
    #[serde(default)]
    pub service: ServiceConfig,
}

/// Sprint 009 — `[service]` namespace. Currently houses
/// connection-limits only; future service-wide knobs land here.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ServiceConfig {
    #[serde(default)]
    pub limits: LimitsConfig,
}

/// Sprint 009 — `[service.limits]` per
/// `sprints/009-stability-attribution/contracts/connection-limits.md`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LimitsConfig {
    #[serde(default = "default_header_read_timeout_secs")]
    pub header_read_timeout_secs: u64,
    #[serde(default = "default_keep_alive_timeout_secs")]
    pub keep_alive_timeout_secs: u64,
    #[serde(default = "default_per_peer_max_concurrent")]
    pub per_peer_max_concurrent: u32,
}

impl Default for LimitsConfig {
    fn default() -> Self {
        Self {
            header_read_timeout_secs: default_header_read_timeout_secs(),
            keep_alive_timeout_secs: default_keep_alive_timeout_secs(),
            per_peer_max_concurrent: default_per_peer_max_concurrent(),
        }
    }
}

impl LimitsConfig {
    /// Validate range bounds documented in the contract.
    ///
    /// # Errors
    /// Returns [`ConfigError::InvalidLimit`] for the first key out of range.
    pub fn validate(&self) -> Result<(), ConfigError> {
        check_range(
            "header_read_timeout_secs",
            self.header_read_timeout_secs,
            1,
            600,
        )?;
        check_range(
            "keep_alive_timeout_secs",
            self.keep_alive_timeout_secs,
            1,
            3600,
        )?;
        check_range(
            "per_peer_max_concurrent",
            u64::from(self.per_peer_max_concurrent),
            1,
            10_000,
        )?;
        Ok(())
    }
}

fn check_range(key: &'static str, value: u64, min: u64, max: u64) -> Result<(), ConfigError> {
    if value < min || value > max {
        return Err(ConfigError::InvalidLimit {
            key,
            value,
            min,
            max,
        });
    }
    Ok(())
}

fn default_header_read_timeout_secs() -> u64 {
    30
}
fn default_keep_alive_timeout_secs() -> u64 {
    75
}
fn default_per_peer_max_concurrent() -> u32 {
    64
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub listen_addr: String,
    pub port: u16,
    /// Host header allowlist for the MCP Streamable HTTP mount (DNS
    /// rebinding protection). Empty (default) disables the check —
    /// `require_bearer` still gates the surface. Set to e.g.
    /// `["localhost", "my-host:7777"]` to restrict.
    #[serde(default)]
    pub mcp_allowed_hosts: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    /// Legacy single-token form. When non-empty, behaves as a grant with
    /// all scopes set. New deployments should prefer [`Self::tokens`].
    #[serde(default)]
    pub bearer_token: String,

    /// Multi-token form. Each entry carries its own scope set.
    #[serde(default)]
    pub tokens: Vec<klams_types::TokenGrantConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostgresConfig {
    pub url: String,
    #[serde(default = "default_max_connections")]
    pub max_connections: u32,
}

fn default_max_connections() -> u32 {
    8
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QdrantConfig {
    pub grpc_url: String,
    #[serde(default = "default_collection")]
    pub collection: String,
}

fn default_collection() -> String {
    "knowledge_items".into()
}

/// Which wire dialect the embedding endpoint speaks (sprint 014).
/// `tei` is the TEI-native `POST /embed`; `openai` is the
/// OpenAI-compatible `POST {url}/embeddings` served by vLLM, TEI's
/// `/v1` route, Ollama, etc.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EmbeddingsApi {
    #[default]
    Tei,
    Openai,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingsConfig {
    /// For `api = "tei"`: the TEI base (e.g. `http://127.0.0.1:7070`).
    /// For `api = "openai"`: the OpenAI-compat base *including* the
    /// version segment (e.g. `http://127.0.0.1:7070/v1`,
    /// `http://kai:8000/v1`).
    pub url: String,
    /// Sent as the `model` field on OpenAI-compat requests; purely
    /// documentary for TEI (the container decides the model).
    pub model_id: String,
    pub vector_dim: u32,
    #[serde(default)]
    pub api: EmbeddingsApi,
    /// Optional bearer key for `api = "openai"` endpoints that
    /// require one (e.g. vLLM started with `--api-key`).
    #[serde(default)]
    pub api_key: Option<String>,
    /// The model's maximum input length in tokens (sprint 027, #420).
    ///
    /// This is the number every ingest path gates against, so a text
    /// that would be refused by the embedder is refused at the boundary
    /// with an honest error instead of being accepted and dropped later.
    /// Defaults to bge-small-en-v1.5's 512; sprint 028's longer-context
    /// model raises it here rather than in code.
    #[serde(default = "default_max_input_tokens")]
    pub max_input_tokens: usize,
    /// How long to keep oversize-write log rows (sprint 027, #656).
    ///
    /// That log stores the full rejected payload, so it is capped rather
    /// than left to the operator the way `search_miss` is. 90 days is
    /// long enough to answer "how often, by whom, how much" across a
    /// couple of sprints.
    #[serde(default = "default_oversize_log_retention_days")]
    pub oversize_log_retention_days: i32,
    /// Prefix prepended to *query* text before embedding — never to
    /// stored documents (sprint 028 #655). Modern retrieval models are
    /// asymmetric: snowflake-arctic-embed wants `"query: "`, the
    /// Qwen3-Embedding family an instruct line. Leave empty for
    /// symmetric models (bge-m3, bge-small).
    #[serde(default)]
    pub query_prefix: String,
}

fn default_max_input_tokens() -> usize {
    klams_types::DEFAULT_MAX_INPUT_TOKENS
}

fn default_oversize_log_retention_days() -> i32 {
    90
}

impl EmbeddingsConfig {
    /// The shared size gate implied by this configuration.
    #[must_use]
    pub fn limit(&self) -> klams_types::EmbedLimit {
        klams_types::EmbedLimit::new(self.max_input_tokens)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueConfig {
    pub capacity: usize,
    pub workers: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    pub format: String,
    pub level: String,
}

/// Background decay-task configuration (sprint 002). Re-export of
/// the canonical type from `klams-types` so the runtime config and
/// the decay task agree on field layout.
pub use klams_types::{ApiConfig, BackupConfig, DecayConfig};

// ---------------------------------------------------------------------------
// Sprint 005 (Phase 4) configuration blocks.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrievalConfig {
    #[serde(default = "default_fusion")]
    pub fusion: String,
    #[serde(default = "default_rrf_k")]
    pub rrf_k: u32,
    #[serde(default = "default_per_source_top_k")]
    pub per_source_top_k: u32,
    #[serde(default)]
    pub weights: Option<RetrievalWeights>,
    /// Second-stage cross-encoder (sprint 030, #685): base URL of the
    /// `reranker` compose service (e.g. `http://127.0.0.1:7071`).
    /// Absent = the stage is off — that is the rollback switch.
    #[serde(default)]
    pub reranker_url: Option<String>,
    /// Max candidates submitted per rerank call. Must not exceed the
    /// reranker's `--max-client-batch-size` (compose serves 64).
    #[serde(default = "default_rerank_window")]
    pub rerank_window: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrievalWeights {
    pub vector: f32,
    pub fts: f32,
    #[serde(default = "default_weighted_norm")]
    pub normalization: String,
}

fn default_fusion() -> String {
    "rrf".into()
}
fn default_rrf_k() -> u32 {
    60
}
fn default_per_source_top_k() -> u32 {
    100
}
fn default_weighted_norm() -> String {
    "zscore".into()
}
fn default_rerank_window() -> u32 {
    50
}

impl Default for RetrievalConfig {
    fn default() -> Self {
        Self {
            fusion: default_fusion(),
            rrf_k: default_rrf_k(),
            per_source_top_k: default_per_source_top_k(),
            weights: None,
            reranker_url: None,
            rerank_window: default_rerank_window(),
        }
    }
}

impl RetrievalConfig {
    /// Map the `[retrieval]` block to a [`klams_types::FusionStrategy`]
    /// (sprint 024 #330 — the config was parsed but never applied). Used
    /// for both the `/memory/context` builder and the MCP `memory_search`
    /// path. Unknown `fusion` strings, or `weighted` without `weights`,
    /// fall back to RRF so a typo can never silently disable ranking.
    #[must_use]
    pub fn fusion_strategy(&self) -> klams_types::FusionStrategy {
        use klams_types::{FusionStrategy, WeightedNorm};
        match (self.fusion.as_str(), &self.weights) {
            ("weighted", Some(w)) => FusionStrategy::Weighted {
                vector: w.vector,
                fts: w.fts,
                normalization: match w.normalization.as_str() {
                    "minmax" => WeightedNorm::MinMax,
                    _ => WeightedNorm::ZScore,
                },
            },
            _ => FusionStrategy::Rrf { k: self.rrf_k },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokensConfig {
    #[serde(default = "default_tokens_mode")]
    pub mode: String,
}

fn default_tokens_mode() -> String {
    "tiktoken".into()
}

impl Default for TokensConfig {
    fn default() -> Self {
        Self {
            mode: default_tokens_mode(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SummarizationConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_event_cluster_min")]
    pub event_cluster_min: u32,
    #[serde(default = "default_knowledge_stale_days")]
    pub knowledge_stale_days: u32,
    #[serde(default = "default_knowledge_cluster_min")]
    pub knowledge_cluster_min: u32,
    #[serde(default = "default_true")]
    pub llm_fallback: bool,
    /// OpenAI-compat base *including* `/v1` (sprint 014 — was the
    /// Ollama-native base). The old `ollama_url` key still parses via
    /// alias, but its value must gain the `/v1` suffix at deploy time.
    #[serde(default = "default_llm_url", alias = "ollama_url")]
    pub llm_url: String,
    #[serde(default = "default_llm_model", alias = "ollama_model")]
    pub llm_model: String,
    /// Optional bearer key for endpoints that require one.
    #[serde(default)]
    pub llm_api_key: Option<String>,
    #[serde(default = "default_summarization_interval")]
    pub task_interval_seconds: u64,
}

fn default_true() -> bool {
    true
}
fn default_event_cluster_min() -> u32 {
    50
}
fn default_knowledge_stale_days() -> u32 {
    90
}
fn default_knowledge_cluster_min() -> u32 {
    20
}
fn default_llm_url() -> String {
    // Ollama's OpenAI-compatible route on kubs0.
    "http://127.0.0.1:11434/v1".into()
}
fn default_llm_model() -> String {
    "phi3:medium".into()
}
fn default_summarization_interval() -> u64 {
    3600
}

impl Default for SummarizationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            event_cluster_min: default_event_cluster_min(),
            knowledge_stale_days: default_knowledge_stale_days(),
            knowledge_cluster_min: default_knowledge_cluster_min(),
            llm_fallback: true,
            llm_url: default_llm_url(),
            llm_model: default_llm_model(),
            llm_api_key: None,
            task_interval_seconds: default_summarization_interval(),
        }
    }
}

impl LogResolvedDecay for DecayConfig {
    fn log_resolved(&self) {
        for t in [FactType::UserFact, FactType::TaskFact, FactType::EnvFact] {
            let lambda = self.lambda_for(t);
            let overridden = self.lambda.contains_key(&t);
            tracing::info!(
                fact_type = t.as_str(),
                lambda,
                overridden,
                task_interval_seconds = self.task_interval_seconds,
                batch_size = self.batch_size,
                "decay config resolved"
            );
        }
    }
}

/// Local helper trait so `DecayConfig` (re-exported from
/// `klams-types`) can keep its observability shim in the service
/// crate without polluting the shared type.
pub trait LogResolvedDecay {
    fn log_resolved(&self);
}

impl Config {
    /// Load from a TOML file at `path`, with `KLAMS_` env overrides
    /// (double underscore separates nested keys, e.g.
    /// `KLAMS_SERVER__PORT=8000`).
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let cfg: Self = Figment::new()
            .merge(Toml::file(path.as_ref()))
            .merge(Env::prefixed("KLAMS_").split("__"))
            .extract()?;
        cfg.service.limits.validate()?;
        Ok(cfg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn fusion_strategy_maps_config_and_falls_back_to_rrf() {
        use klams_types::{FusionStrategy, WeightedNorm};
        // Default → RRF at the configured k.
        let base = RetrievalConfig {
            rrf_k: 42,
            ..Default::default()
        };
        assert_eq!(base.fusion_strategy(), FusionStrategy::Rrf { k: 42 });
        // weighted + weights → Weighted with parsed norm.
        let weighted = RetrievalConfig {
            fusion: "weighted".into(),
            weights: Some(RetrievalWeights {
                vector: 0.7,
                fts: 0.3,
                normalization: "minmax".into(),
            }),
            ..base.clone()
        };
        assert_eq!(
            weighted.fusion_strategy(),
            FusionStrategy::Weighted {
                vector: 0.7,
                fts: 0.3,
                normalization: WeightedNorm::MinMax
            }
        );
        // weighted-without-weights, or an unknown string → RRF fallback
        // (a typo can never silently disable ranking).
        let no_weights = RetrievalConfig {
            fusion: "weighted".into(),
            ..base.clone()
        };
        assert_eq!(no_weights.fusion_strategy(), FusionStrategy::Rrf { k: 42 });
        let bogus = RetrievalConfig {
            fusion: "bogus".into(),
            ..base
        };
        assert_eq!(bogus.fusion_strategy(), FusionStrategy::Rrf { k: 42 });
    }

    fn example_path() -> PathBuf {
        let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        crate_dir
            .join("../..")
            .join("deploy/config/klams.example.toml")
            .canonicalize()
            .expect("klams.example.toml must exist for round-trip test")
    }

    #[test]
    fn loads_shipped_example_toml() {
        let path = example_path();
        let cfg = Config::from_path(&path).expect("example toml should parse");
        assert_eq!(cfg.server.listen_addr, "127.0.0.1");
        assert_eq!(cfg.server.port, 7777);
        assert!(!cfg.auth.bearer_token.is_empty());
        assert!(cfg.postgres.url.starts_with("postgres://"));
        assert_eq!(cfg.qdrant.collection, "knowledge_items");
        assert_eq!(cfg.embeddings.vector_dim, 384);
        assert!(cfg.queue.capacity >= 1);
        assert!(cfg.queue.workers >= 1);
        assert_eq!(cfg.logging.format, "json");
        // Sprint 005: [decay.lambda] is now uncommented in the
        // shipped example; the per-type values match the documented
        // defaults so behavior is unchanged.
        assert_eq!(cfg.decay.task_interval_seconds, 3600);
        assert_eq!(cfg.decay.batch_size, 500);
        assert_eq!(cfg.decay.lambda.len(), 3);
        assert!((cfg.decay.lambda_for(FactType::UserFact) - 1e-9).abs() < f32::EPSILON);
        assert!((cfg.decay.lambda_for(FactType::TaskFact) - 1e-6).abs() < f32::EPSILON);
        assert!((cfg.decay.lambda_for(FactType::EnvFact) - 1e-9).abs() < f32::EPSILON);
        // Sprint 005: new [retrieval], [tokens], [summarization] blocks.
        assert_eq!(cfg.retrieval.fusion, "rrf");
        assert_eq!(cfg.retrieval.rrf_k, 60);
        assert_eq!(cfg.tokens.mode, "tiktoken");
        assert!(cfg.summarization.enabled);
        assert_eq!(cfg.summarization.llm_model, "phi3:medium");
        // Sprint 014: the shipped example speaks OpenAI-compat for the
        // chat endpoint and defaults the embedder to TEI-native.
        assert!(cfg.summarization.llm_url.ends_with("/v1"));
        assert_eq!(cfg.embeddings.api, EmbeddingsApi::Tei);
    }

    /// Sprint 014 — `[embeddings] api` selector defaults to `tei` and
    /// parses `openai`; `[summarization]` accepts the legacy
    /// `ollama_url` / `ollama_model` keys as aliases.
    #[test]
    fn serving_pivot_config_surface() {
        let base = r#"
            [server]
            listen_addr = "127.0.0.1"
            port = 7777
            [auth]
            bearer_token = "test"
            [postgres]
            url = "postgres://x/y"
            [qdrant]
            grpc_url = "http://127.0.0.1:6334"
            [queue]
            capacity = 64
            workers = 1
            [logging]
            format = "json"
            level = "info"
        "#;

        // Default api = tei; no api_key.
        let toml = format!(
            r#"{base}
            [embeddings]
            url = "http://127.0.0.1:7070"
            model_id = "BAAI/bge-small-en-v1.5"
            vector_dim = 384
        "#
        );
        let cfg: Config = Figment::new()
            .merge(Toml::string(&toml))
            .extract()
            .expect("parse");
        assert_eq!(cfg.embeddings.api, EmbeddingsApi::Tei);
        assert!(cfg.embeddings.api_key.is_none());
        assert_eq!(cfg.summarization.llm_url, "http://127.0.0.1:11434/v1");

        // api = "openai" with a key.
        let toml = format!(
            r#"{base}
            [embeddings]
            url = "http://kai:8000/v1"
            model_id = "BAAI/bge-small-en-v1.5"
            vector_dim = 384
            api = "openai"
            api_key = "sk-test"
        "#
        );
        let cfg: Config = Figment::new()
            .merge(Toml::string(&toml))
            .extract()
            .expect("parse");
        assert_eq!(cfg.embeddings.api, EmbeddingsApi::Openai);
        assert_eq!(cfg.embeddings.api_key.as_deref(), Some("sk-test"));

        // Legacy [summarization] keys parse via alias.
        let toml = format!(
            r#"{base}
            [embeddings]
            url = "http://127.0.0.1:7070"
            model_id = "m"
            vector_dim = 384
            [summarization]
            ollama_url = "http://kubs0:11434/v1"
            ollama_model = "llama3.2:latest"
        "#
        );
        let cfg: Config = Figment::new()
            .merge(Toml::string(&toml))
            .extract()
            .expect("parse");
        assert_eq!(cfg.summarization.llm_url, "http://kubs0:11434/v1");
        assert_eq!(cfg.summarization.llm_model, "llama3.2:latest");
    }

    /// T012(a): a config with no `[decay]` block loads defaults.
    #[test]
    fn decay_defaults_when_block_missing() {
        let toml = r#"
            [server]
            listen_addr = "127.0.0.1"
            port = 7777
            [auth]
            bearer_token = "test"
            [postgres]
            url = "postgres://x/y"
            [qdrant]
            grpc_url = "http://127.0.0.1:6334"
            [embeddings]
            url = "http://127.0.0.1:7070"
            model_id = "BAAI/bge-small-en-v1.5"
            vector_dim = 384
            [queue]
            capacity = 64
            workers = 1
            [logging]
            format = "json"
            level = "info"
        "#;
        let cfg: Config = Figment::new()
            .merge(Toml::string(toml))
            .extract()
            .expect("parse");
        assert_eq!(cfg.decay.task_interval_seconds, 3600);
        assert_eq!(cfg.decay.batch_size, 500);
        assert!((cfg.decay.lambda_for(FactType::TaskFact) - 1e-6).abs() < f32::EPSILON);
    }

    /// T012(b): partial overrides keep defaults for unconfigured
    /// types.
    #[test]
    fn decay_partial_override_preserves_other_defaults() {
        let toml = r#"
            [server]
            listen_addr = "127.0.0.1"
            port = 7777
            [auth]
            bearer_token = "test"
            [postgres]
            url = "postgres://x/y"
            [qdrant]
            grpc_url = "http://127.0.0.1:6334"
            [embeddings]
            url = "http://127.0.0.1:7070"
            model_id = "BAAI/bge-small-en-v1.5"
            vector_dim = 384
            [queue]
            capacity = 64
            workers = 1
            [logging]
            format = "json"
            level = "info"
            [decay]
            task_interval_seconds = 60
            [decay.lambda]
            TaskFact = 5.0e-5
        "#;
        let cfg: Config = Figment::new()
            .merge(Toml::string(toml))
            .extract()
            .expect("parse");
        assert_eq!(cfg.decay.task_interval_seconds, 60);
        // batch_size still default.
        assert_eq!(cfg.decay.batch_size, 500);
        // Overridden type uses override.
        assert!((cfg.decay.lambda_for(FactType::TaskFact) - 5.0e-5).abs() < 1e-9);
        // Unconfigured types still get defaults.
        assert!((cfg.decay.lambda_for(FactType::UserFact) - 1e-9).abs() < f32::EPSILON);
        assert!((cfg.decay.lambda_for(FactType::EnvFact) - 1e-9).abs() < f32::EPSILON);
    }

    /// T012(c): full override roundtrips through serde without
    /// dropping any per-type entry.
    #[test]
    fn decay_full_override_roundtrips() {
        let toml = r#"
            [server]
            listen_addr = "127.0.0.1"
            port = 7777
            [auth]
            bearer_token = "test"
            [postgres]
            url = "postgres://x/y"
            [qdrant]
            grpc_url = "http://127.0.0.1:6334"
            [embeddings]
            url = "http://127.0.0.1:7070"
            model_id = "BAAI/bge-small-en-v1.5"
            vector_dim = 384
            [queue]
            capacity = 64
            workers = 1
            [logging]
            format = "json"
            level = "info"
            [decay]
            task_interval_seconds = 120
            batch_size = 200
            [decay.lambda]
            UserFact = 1.0e-8
            TaskFact = 2.0e-5
            EnvFact  = 3.0e-7
        "#;
        let cfg: Config = Figment::new()
            .merge(Toml::string(toml))
            .extract()
            .expect("parse");
        assert_eq!(cfg.decay.task_interval_seconds, 120);
        assert_eq!(cfg.decay.batch_size, 200);
        assert!((cfg.decay.lambda_for(FactType::UserFact) - 1.0e-8).abs() < 1e-12);
        assert!((cfg.decay.lambda_for(FactType::TaskFact) - 2.0e-5).abs() < 1e-9);
        assert!((cfg.decay.lambda_for(FactType::EnvFact) - 3.0e-7).abs() < 1e-11);
        // Round-trip through TOML and back to confirm no entry is lost.
        let serialised = toml::to_string(&cfg).expect("serialise");
        let reparsed: Config = Figment::new()
            .merge(Toml::string(&serialised))
            .extract()
            .expect("reparse");
        assert_eq!(reparsed.decay.lambda.len(), 3);
        assert!((reparsed.decay.lambda_for(FactType::TaskFact) - 2.0e-5).abs() < 1e-9);
    }
}
