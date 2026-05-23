//! Backup snapshot + restore mechanics (sprint 006).
//!
//! The `klams-store` half of the backup feature owns the low-level
//! snapshot/restore primitives — invoking `pg_dump`/`pg_restore` and
//! Qdrant's snapshot REST API — plus filesystem retention pruning.
//! Orchestration (scheduling, lockfile, lifecycle, `status_hook`,
//! metrics) lives in `klams-service::backup`.

use std::path::PathBuf;

pub mod postgres;
pub mod qdrant;
pub mod retention;

/// Which on-disk artifact a [`BackupArtifact`] represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    Postgres,
    Qdrant,
}

impl ArtifactKind {
    /// Filename prefix used in `backup_dir`.
    #[must_use]
    pub const fn prefix(self) -> &'static str {
        match self {
            Self::Postgres => "postgres",
            Self::Qdrant => "qdrant",
        }
    }

    /// File extension (after the date and optional `-N` suffix).
    #[must_use]
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Postgres => "dump",
            Self::Qdrant => "snapshot",
        }
    }
}

/// One committed artifact on disk (post-rename).
#[derive(Debug, Clone, serde::Serialize)]
pub struct BackupArtifact {
    pub kind: ArtifactKind,
    pub path: PathBuf,
    pub bytes: u64,
    pub duration_ms: u64,
    pub ok: bool,
    pub error: Option<String>,
}

/// Errors raised by the store-layer backup primitives.
#[derive(Debug, thiserror::Error)]
pub enum BackupError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("pg_dump exit {status}: {stderr}")]
    PgDumpFailed { status: i32, stderr: String },
    #[error("pg_restore exit {status}: {stderr}")]
    PgRestoreFailed { status: i32, stderr: String },
    #[error("qdrant http: {0}")]
    QdrantHttp(String),
    #[error("qdrant snapshot: {0}")]
    QdrantSnapshot(String),
    #[error("invalid filename: {0}")]
    InvalidFilename(String),
}
