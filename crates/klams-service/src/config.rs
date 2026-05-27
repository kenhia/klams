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
    #[error("summarization.ollama_url `{value}` is not a valid URL: {source}")]
    SummarizationOllamaUrlInvalid {
        value: String,
        #[source]
        source: url::ParseError,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingsConfig {
    pub url: String,
    pub model_id: String,
    pub vector_dim: u32,
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

impl Default for RetrievalConfig {
    fn default() -> Self {
        Self {
            fusion: default_fusion(),
            rrf_k: default_rrf_k(),
            per_source_top_k: default_per_source_top_k(),
            weights: None,
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
    #[serde(default = "default_ollama_url")]
    pub ollama_url: String,
    #[serde(default = "default_ollama_model")]
    pub ollama_model: String,
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
fn default_ollama_url() -> String {
    "http://127.0.0.1:11434".into()
}
fn default_ollama_model() -> String {
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
            ollama_url: default_ollama_url(),
            ollama_model: default_ollama_model(),
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
        let cfg = Figment::new()
            .merge(Toml::file(path.as_ref()))
            .merge(Env::prefixed("KLAMS_").split("__"))
            .extract()?;
        Ok(cfg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

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
        assert_eq!(cfg.summarization.ollama_model, "phi3:medium");
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
