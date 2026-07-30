//! Viewport configuration: URL + refresh interval in `viewport.toml`
//! under the platform config dir; bearer token in the OS keyring.

use crate::commands::ViewportError;
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

const KEYRING_SERVICE: &str = "klams-viewport";
const KEYRING_ACCOUNT: &str = "bearer";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredConfig {
    #[serde(default = "default_url")]
    pub klams_url: String,
    #[serde(default = "default_interval")]
    pub refresh_interval_seconds: u32,
}

fn default_url() -> String {
    // Loopback, not a homelab hostname (sprint 035, #776): the first
    // thing a new operator sees must not be someone else's machine.
    "http://127.0.0.1:7777".into()
}
fn default_interval() -> u32 {
    10
}

impl Default for StoredConfig {
    fn default() -> Self {
        Self {
            klams_url: default_url(),
            refresh_interval_seconds: default_interval(),
        }
    }
}

/// Public viewport configuration surfaced to the frontend. The token
/// itself is never returned, only its presence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewportConfig {
    pub klams_url: String,
    pub has_token: bool,
    pub refresh_interval_seconds: u32,
}

fn config_path() -> Result<PathBuf, ViewportError> {
    let pd = ProjectDirs::from("", "", "klams").ok_or_else(|| ViewportError::NotConfigured {
        message: "no project dirs".into(),
    })?;
    let dir = pd.config_dir().to_path_buf();
    fs::create_dir_all(&dir).map_err(|e| ViewportError::NotConfigured {
        message: format!("create config dir: {e}"),
    })?;
    Ok(dir.join("viewport.toml"))
}

pub fn load() -> StoredConfig {
    let Ok(path) = config_path() else {
        return StoredConfig::default();
    };
    match fs::read_to_string(&path) {
        Ok(s) => toml::from_str(&s).unwrap_or_default(),
        Err(_) => StoredConfig::default(),
    }
}

pub fn save(cfg: &StoredConfig) -> Result<(), ViewportError> {
    let path = config_path()?;
    let s = toml::to_string_pretty(cfg).map_err(|e| ViewportError::NotConfigured {
        message: format!("serialize: {e}"),
    })?;
    fs::write(&path, s).map_err(|e| ViewportError::NotConfigured {
        message: format!("write config: {e}"),
    })
}

pub fn read_token() -> Option<String> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_ACCOUNT).ok()?;
    entry.get_password().ok()
}

pub fn write_token(token: &str) -> Result<(), ViewportError> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_ACCOUNT).map_err(|e| {
        ViewportError::NotConfigured {
            message: format!("keyring open: {e}"),
        }
    })?;
    entry
        .set_password(token)
        .map_err(|e| ViewportError::NotConfigured {
            message: format!("keyring write: {e}"),
        })
}

pub fn snapshot() -> ViewportConfig {
    let stored = load();
    ViewportConfig {
        klams_url: stored.klams_url,
        has_token: read_token().is_some(),
        refresh_interval_seconds: stored.refresh_interval_seconds,
    }
}
