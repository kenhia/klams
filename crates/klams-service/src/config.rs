//! Runtime configuration for klams-service.

use figment::providers::{Env, Format, Toml};
use figment::Figment;
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
    }
}
