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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub listen_addr: String,
    pub port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    pub bearer_token: String,
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
pub use klams_types::DecayConfig;

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
        // [decay] is commented out in the shipped example, so the
        // defaults must apply.
        assert_eq!(cfg.decay.task_interval_seconds, 3600);
        assert_eq!(cfg.decay.batch_size, 500);
        assert!(cfg.decay.lambda.is_empty());
        assert!((cfg.decay.lambda_for(FactType::UserFact) - 1e-9).abs() < f32::EPSILON);
        assert!((cfg.decay.lambda_for(FactType::TaskFact) - 1e-6).abs() < f32::EPSILON);
        assert!((cfg.decay.lambda_for(FactType::EnvFact) - 1e-9).abs() < f32::EPSILON);
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
