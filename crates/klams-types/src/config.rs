//! Backup feature configuration (sprint 006).
//!
//! Lives in `klams-types` (not `klams-service`) so other crates can
//! depend on the types without depending on the service binary, the
//! same way `DecayConfig` already does.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;

/// `[backup]` block in `klams.toml`. See
/// `specs/006-maintenance-and-backups/data-model.md`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupConfig {
    /// Master switch. Default `false` — zero runtime cost when off.
    #[serde(default)]
    pub enabled: bool,

    /// Directory the backup task writes artifacts into. Must be set
    /// and writable when `enabled = true`. Typically a NAS mount;
    /// klams does not care about the underlying device.
    #[serde(default)]
    pub backup_dir: Option<PathBuf>,

    /// 24h UTC, `"HH:MM"`. UTC-only to dodge DST drift.
    pub window_start_utc: WindowStartUtc,

    /// Newest N distinct dates retained per artifact kind.
    #[serde(default = "default_daily_count")]
    pub daily_count: u32,

    /// Newest N Sundays retained per artifact kind.
    #[serde(default = "default_weekly_count")]
    pub weekly_count: u32,

    /// How to disambiguate multiple runs on the same date.
    #[serde(default)]
    pub same_day_strategy: SameDayStrategy,

    /// Optional executable invoked at lifecycle transitions.
    #[serde(default)]
    pub status_hook: Option<PathBuf>,

    /// Bounded timeout for `status_hook` invocations. Default `10s`.
    #[serde(default = "default_hook_timeout", with = "humantime_serde")]
    pub status_hook_timeout: Duration,

    /// Optional directory containing version-matched `pg_dump` /
    /// `pg_restore` binaries (e.g. `/usr/lib/postgresql/16/bin`).
    /// When `None`, the system PATH is used. Required when the host
    /// runs newer Postgres client tools than the target server, since
    /// `pg_dump` emits server-version-specific `SET` statements
    /// (sprint 006 R-001 / FR-013). `KLAMS_PG_BIN_DIR` env overrides.
    #[serde(default)]
    pub pg_bin_dir: Option<PathBuf>,
}

const fn default_daily_count() -> u32 {
    14
}
const fn default_weekly_count() -> u32 {
    4
}
const fn default_hook_timeout() -> Duration {
    Duration::from_secs(10)
}

/// `"HH:MM"` UTC time-of-day. Stored decomposed so consumers don't
/// re-parse on every scheduler tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowStartUtc {
    pub hour: u8,
    pub minute: u8,
}

impl std::fmt::Display for WindowStartUtc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:02}:{:02}", self.hour, self.minute)
    }
}

impl FromStr for WindowStartUtc {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (h, m) = s
            .split_once(':')
            .ok_or_else(|| format!("expected HH:MM, got {s:?}"))?;
        let hour: u8 = h
            .parse()
            .map_err(|e| format!("hour parse: {e} (in {s:?})"))?;
        let minute: u8 = m
            .parse()
            .map_err(|e| format!("minute parse: {e} (in {s:?})"))?;
        if hour > 23 {
            return Err(format!("hour must be 0..=23, got {hour}"));
        }
        if minute > 59 {
            return Err(format!("minute must be 0..=59, got {minute}"));
        }
        Ok(Self { hour, minute })
    }
}

impl<'de> Deserialize<'de> for WindowStartUtc {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let s = String::deserialize(de)?;
        Self::from_str(&s).map_err(serde::de::Error::custom)
    }
}

impl Serialize for WindowStartUtc {
    fn serialize<S: Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        ser.serialize_str(&self.to_string())
    }
}

/// What to do when a backup is requested on a date that already has
/// a committed artifact of the same kind.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SameDayStrategy {
    /// Append `-N` suffix; retention treats suffixed copies as the
    /// same date and keeps the highest-N copy. (Default.)
    #[default]
    Suffix,
    /// Atomic-rename overwrites the previous same-date file.
    Overwrite,
}

/// Errors surfaced by [`BackupConfig::validate`].
#[derive(Debug, thiserror::Error)]
pub enum BackupConfigError {
    #[error("[backup] enabled=true but backup_dir is unset")]
    MissingBackupDir,
    #[error("[backup] backup_dir does not exist: {0}")]
    BackupDirMissing(PathBuf),
    #[error("[backup] backup_dir is not a directory: {0}")]
    BackupDirNotADir(PathBuf),
    #[error("[backup] backup_dir is not writable: {0}")]
    BackupDirNotWritable(PathBuf),
    #[error("[backup] status_hook does not exist: {0}")]
    HookMissing(PathBuf),
    #[error("[backup] status_hook is not executable: {0}")]
    HookNotExecutable(PathBuf),
}

impl BackupConfig {
    /// Service-start validation per spec FR-012.
    pub fn validate(&self) -> Result<(), BackupConfigError> {
        if !self.enabled {
            return Ok(());
        }

        let dir = self
            .backup_dir
            .as_ref()
            .ok_or(BackupConfigError::MissingBackupDir)?;

        let meta =
            std::fs::metadata(dir).map_err(|_| BackupConfigError::BackupDirMissing(dir.clone()))?;
        if !meta.is_dir() {
            return Err(BackupConfigError::BackupDirNotADir(dir.clone()));
        }
        if meta.permissions().readonly() {
            return Err(BackupConfigError::BackupDirNotWritable(dir.clone()));
        }

        if let Some(hook) = &self.status_hook {
            let hmeta = std::fs::metadata(hook)
                .map_err(|_| BackupConfigError::HookMissing(hook.clone()))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if hmeta.permissions().mode() & 0o111 == 0 {
                    return Err(BackupConfigError::HookNotExecutable(hook.clone()));
                }
            }
            #[cfg(not(unix))]
            {
                let _ = hmeta;
            }
        }

        Ok(())
    }
}

impl Default for BackupConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            backup_dir: None,
            window_start_utc: WindowStartUtc { hour: 7, minute: 0 },
            daily_count: default_daily_count(),
            weekly_count: default_weekly_count(),
            same_day_strategy: SameDayStrategy::Suffix,
            status_hook: None,
            status_hook_timeout: default_hook_timeout(),
            pg_bin_dir: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Result<BackupConfig, toml::de::Error> {
        toml::from_str(s)
    }

    #[test]
    fn defaults_are_correct() {
        let cfg: BackupConfig = parse(
            r#"
            window_start_utc = "07:00"
        "#,
        )
        .unwrap();
        assert!(!cfg.enabled);
        assert_eq!(cfg.daily_count, 14);
        assert_eq!(cfg.weekly_count, 4);
        assert!(matches!(cfg.same_day_strategy, SameDayStrategy::Suffix));
        assert_eq!(cfg.status_hook_timeout, Duration::from_secs(10));
        assert!(cfg.status_hook.is_none());
    }

    #[test]
    fn valid_toml_roundtrips() {
        let cfg: BackupConfig = parse(
            r#"
            enabled = true
            backup_dir = "/var/backups/klams"
            window_start_utc = "02:30"
            daily_count = 7
            weekly_count = 8
            same_day_strategy = "overwrite"
            status_hook = "/usr/local/bin/hook"
            status_hook_timeout = "30s"
        "#,
        )
        .unwrap();
        assert!(cfg.enabled);
        assert_eq!(
            cfg.backup_dir.as_deref(),
            Some(std::path::Path::new("/var/backups/klams"))
        );
        assert_eq!(
            cfg.window_start_utc,
            WindowStartUtc {
                hour: 2,
                minute: 30
            }
        );
        assert_eq!(cfg.daily_count, 7);
        assert_eq!(cfg.weekly_count, 8);
        assert!(matches!(cfg.same_day_strategy, SameDayStrategy::Overwrite));
        assert_eq!(cfg.status_hook_timeout, Duration::from_secs(30));
    }

    #[test]
    fn invalid_window_start_utc_25_00() {
        let err = parse(r#"window_start_utc = "25:00""#).unwrap_err();
        assert!(err.to_string().contains("hour must be 0..=23"), "got {err}");
    }

    #[test]
    fn invalid_window_start_utc_no_colon() {
        let err = parse(r#"window_start_utc = "10""#).unwrap_err();
        assert!(err.to_string().contains("expected HH:MM"), "got {err}");
    }

    #[test]
    fn invalid_window_start_utc_empty() {
        let err = parse(r#"window_start_utc = """#).unwrap_err();
        assert!(err.to_string().contains("expected HH:MM"), "got {err}");
    }

    #[test]
    fn invalid_window_start_utc_minute_oob() {
        let err = parse(r#"window_start_utc = "10:75""#).unwrap_err();
        assert!(
            err.to_string().contains("minute must be 0..=59"),
            "got {err}"
        );
    }

    #[test]
    fn validate_rejects_enabled_without_backup_dir() {
        let cfg = BackupConfig {
            enabled: true,
            ..BackupConfig::default()
        };
        assert!(matches!(
            cfg.validate(),
            Err(BackupConfigError::MissingBackupDir)
        ));
    }

    #[test]
    fn validate_rejects_missing_backup_dir() {
        let cfg = BackupConfig {
            enabled: true,
            backup_dir: Some(PathBuf::from("/nonexistent/klams/backups/__test__")),
            ..BackupConfig::default()
        };
        assert!(matches!(
            cfg.validate(),
            Err(BackupConfigError::BackupDirMissing(_))
        ));
    }

    #[test]
    fn validate_accepts_disabled_with_no_dir() {
        let cfg = BackupConfig::default();
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn validate_accepts_valid_writable_dir() {
        let tmp = std::env::temp_dir();
        let cfg = BackupConfig {
            enabled: true,
            backup_dir: Some(tmp),
            ..BackupConfig::default()
        };
        assert!(cfg.validate().is_ok(), "{:?}", cfg.validate());
    }
}
