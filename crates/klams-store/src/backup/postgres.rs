//! `pg_dump` / `pg_restore` shell-out (sprint 006 T021/T031).
//!
//! Writes to `<backup_dir>/postgres-<UTC-date>[-N].dump.partial`
//! and atomic-renames to `.dump` on a clean exit. On any error the
//! `.partial` file is left in place for triage; the retention pruner
//! intentionally ignores `.partial` files (they're not commits).

use std::path::{Path, PathBuf};
use std::time::Instant;

use tokio::process::Command;

use super::{ArtifactKind, BackupArtifact, BackupError};

/// Resolve `pg_dump` / `pg_restore` to an absolute path when the
/// operator pins `KLAMS_PG_BIN_DIR` (e.g. `/usr/lib/postgresql/16/bin`),
/// otherwise fall back to `$PATH`. Required because a dump produced
/// by a newer `pg_dump` may emit `SET` statements that an older
/// server cannot restore (FR-002 / FR-013). `KLAMS_PG_BIN_DIR`
/// overrides any caller-supplied directory; this lets operators
/// override without redeploying config.
fn pg_binary(name: &str, override_dir: Option<&Path>) -> PathBuf {
    if let Some(dir) = std::env::var_os("KLAMS_PG_BIN_DIR") {
        return PathBuf::from(dir).join(name);
    }
    override_dir.map_or_else(|| PathBuf::from(name), |d| d.join(name))
}

/// Run `pg_dump -Fc` against `pg_url`, writing the artifact under
/// `backup_dir`. The filename is `postgres-<date>[-N].dump` where
/// `N` is a numeric suffix used when a file for `date` already
/// exists and `same_day_strategy = Suffix`.
///
/// # Errors
///
/// Returns [`BackupError::Io`] for filesystem errors and
/// [`BackupError::PgDumpFailed`] if `pg_dump` exits non-zero.
pub async fn dump(
    backup_dir: &Path,
    pg_url: &str,
    date_str: &str,
    suffix: Option<u32>,
    pg_bin_dir: Option<&Path>,
) -> Result<BackupArtifact, BackupError> {
    let started = Instant::now();
    let (final_path, partial_path) = artifact_paths(backup_dir, date_str, suffix);

    if let Some(parent) = final_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let output = Command::new(pg_binary("pg_dump", pg_bin_dir))
        .arg("-Fc")
        .arg("--no-owner")
        .arg("--no-privileges")
        .arg("-f")
        .arg(&partial_path)
        .arg(pg_url)
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        return Err(BackupError::PgDumpFailed {
            status: output.status.code().unwrap_or(-1),
            stderr,
        });
    }

    tokio::fs::rename(&partial_path, &final_path).await?;
    let bytes = tokio::fs::metadata(&final_path).await?.len();

    Ok(BackupArtifact {
        kind: ArtifactKind::Postgres,
        path: final_path,
        bytes,
        duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        ok: true,
        error: None,
    })
}

/// Restore from a previously-captured `pg_dump -Fc` artifact into
/// `pg_url`. Uses `pg_restore --single-transaction --clean --if-exists --no-owner`.
///
/// The `--single-transaction` wrapper makes the whole restore
/// atomic: any error rolls back both the `--clean` `DROP`s and the
/// partial data load, leaving the target in its pre-call state.
///
/// # Errors
///
/// Returns [`BackupError::PgRestoreFailed`] on a non-zero exit.
pub async fn restore(
    pg_url: &str,
    dump_path: &Path,
    pg_bin_dir: Option<&Path>,
) -> Result<(), BackupError> {
    let output = Command::new(pg_binary("pg_restore", pg_bin_dir))
        .arg("--single-transaction")
        .arg("--clean")
        .arg("--if-exists")
        .arg("--no-owner")
        .arg("--no-privileges")
        .arg("--dbname")
        .arg(pg_url)
        .arg(dump_path)
        .output()
        .await?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        return Err(BackupError::PgRestoreFailed {
            status: output.status.code().unwrap_or(-1),
            stderr,
        });
    }
    Ok(())
}

/// Major version of the `pg_dump` that [`dump`] would run, parsed
/// from `pg_dump --version` (`pg_dump (PostgreSQL) 18.7 (Ubuntu …)`).
#[must_use]
pub fn parse_client_major(version_output: &str) -> Option<u32> {
    let rest = version_output.split(')').nth(1)?.trim_start();
    let token = rest.split_whitespace().next()?;
    token
        .split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()
}

/// Major version from `SHOW server_version_num` (`160014` -> 16).
#[must_use]
pub fn parse_server_major(version_num: &str) -> Option<u32> {
    version_num.trim().parse::<u32>().ok().map(|n| n / 10_000)
}

/// The warning text for a client/server major mismatch, or `None`
/// when they agree (korg WI 4588).
///
/// A newer `pg_dump` writes an archive only its own major's
/// `pg_restore` can read, and that `pg_restore` emits `SET`s (e.g.
/// `transaction_timeout`) the older server rejects — so the dump is
/// written fine and cannot be restored by `--restore-from`. A warning
/// rather than a refusal: the overseer's call for sprint 060, because
/// refusing would change behaviour for anyone running backups today.
#[must_use]
pub fn mismatch_warning(client_major: u32, server_major: u32) -> Option<String> {
    (client_major != server_major).then(|| {
        format!(
            "[backup] pg_dump is PostgreSQL {client_major} but the server is {server_major}: \
             a dump written by these tools cannot be restored by --restore-from. \
             Set [backup].pg_bin_dir to the server's major \
             (e.g. /usr/lib/postgresql/{server_major}/bin)"
        )
    })
}

/// Compare the major of the `pg_dump` that backups would run against
/// the server at `pg_url`. `Ok(Some(warning))` on a mismatch,
/// `Ok(None)` when they agree, `Err` when either side could not be
/// read — the check is advisory, so callers report that and carry on.
///
/// # Errors
///
/// A description of what could not be determined (binary missing,
/// unparseable output, server unreachable within 5s).
pub async fn check_tool_version(
    pg_url: &str,
    pg_bin_dir: Option<&Path>,
) -> Result<Option<String>, String> {
    use sqlx::Connection;

    let bin = pg_binary("pg_dump", pg_bin_dir);
    let out = Command::new(&bin)
        .arg("--version")
        .output()
        .await
        .map_err(|e| format!("running {} --version: {e}", bin.display()))?;
    let text = String::from_utf8_lossy(&out.stdout);
    let client = parse_client_major(&text)
        .ok_or_else(|| format!("unparseable `{} --version`: {}", bin.display(), text.trim()))?;

    let server = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let mut conn = sqlx::PgConnection::connect(pg_url)
            .await
            .map_err(|e| e.to_string())?;
        let (v,): (String,) = sqlx::query_as("SHOW server_version_num")
            .fetch_one(&mut conn)
            .await
            .map_err(|e| e.to_string())?;
        let _ = conn.close().await;
        Ok::<_, String>(v)
    })
    .await
    .map_err(|_| "server version: timed out after 5s".to_string())?
    .map_err(|e| format!("server version: {e}"))?;
    let server = parse_server_major(&server)
        .ok_or_else(|| format!("unparseable server_version_num: {server}"))?;

    Ok(mismatch_warning(client, server))
}

fn artifact_paths(backup_dir: &Path, date_str: &str, suffix: Option<u32>) -> (PathBuf, PathBuf) {
    let stem = match suffix {
        Some(n) => format!("postgres-{date_str}-{n}"),
        None => format!("postgres-{date_str}"),
    };
    let final_path = backup_dir.join(format!("{stem}.dump"));
    let partial_path = backup_dir.join(format!("{stem}.dump.partial"));
    (final_path, partial_path)
}

#[cfg(test)]
mod tests {
    use super::{artifact_paths, mismatch_warning, parse_client_major, parse_server_major};
    use std::path::Path;

    #[test]
    fn client_major_parses_distro_and_plain_output() {
        assert_eq!(
            parse_client_major("pg_dump (PostgreSQL) 18.7 (Ubuntu 18.7-1.pgdg24.04+5)\n"),
            Some(18)
        );
        assert_eq!(parse_client_major("pg_dump (PostgreSQL) 16.15\n"), Some(16));
        assert_eq!(parse_client_major("pg_dump (PostgreSQL) 17beta1"), Some(17));
        assert_eq!(parse_client_major("garbage"), None);
    }

    #[test]
    fn server_major_from_version_num() {
        assert_eq!(parse_server_major("160014"), Some(16));
        assert_eq!(parse_server_major(" 180001\n"), Some(18));
        assert_eq!(parse_server_major("x"), None);
    }

    #[test]
    fn mismatch_warns_only_when_majors_differ() {
        assert_eq!(mismatch_warning(16, 16), None);
        // The kubs0 case from WI 4588: PG 18 tools against a PG 16 server.
        let w = mismatch_warning(18, 16).expect("18 vs 16 must warn");
        assert!(w.contains("pg_bin_dir"), "{w}");
        assert!(w.contains("/usr/lib/postgresql/16/bin"), "{w}");
    }

    #[test]
    fn artifact_paths_no_suffix() {
        let dir = Path::new("/tmp/backups");
        let (f, p) = artifact_paths(dir, "2026-05-23", None);
        assert_eq!(f, Path::new("/tmp/backups/postgres-2026-05-23.dump"));
        assert_eq!(
            p,
            Path::new("/tmp/backups/postgres-2026-05-23.dump.partial")
        );
    }

    #[test]
    fn artifact_paths_with_suffix() {
        let dir = Path::new("/tmp/backups");
        let (f, p) = artifact_paths(dir, "2026-05-23", Some(2));
        assert_eq!(f, Path::new("/tmp/backups/postgres-2026-05-23-2.dump"));
        assert_eq!(
            p,
            Path::new("/tmp/backups/postgres-2026-05-23-2.dump.partial")
        );
    }
}
