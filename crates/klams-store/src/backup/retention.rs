//! Daily + weekly retention pruning over `backup_dir` (sprint 006 T023).
//!
//! Filename-as-truth date parsing: canonical format is
//! `<kind>-YYYY-MM-DD[-N].{dump,snapshot}`. Keeps the newest
//! `daily_count` distinct dates plus the newest `weekly_count`
//! Sundays per kind, deletes the rest. `.partial` files are
//! ignored (not commits).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::{Datelike, NaiveDate, Weekday};

use super::{ArtifactKind, BackupError};

/// Summary of one retention pass.
#[derive(Debug, Default, Clone)]
pub struct RetentionReport {
    pub kept: Vec<PathBuf>,
    pub deleted: Vec<PathBuf>,
}

/// Apply the daily + weekly retention policy under `backup_dir`.
///
/// # Errors
///
/// Returns [`BackupError::Io`] on filesystem errors.
pub async fn prune(
    backup_dir: &Path,
    daily_count: u32,
    weekly_count: u32,
) -> Result<RetentionReport, BackupError> {
    let mut report = RetentionReport::default();
    for kind in [ArtifactKind::Postgres, ArtifactKind::Qdrant] {
        let mut sub = prune_one_kind(backup_dir, kind, daily_count, weekly_count).await?;
        report.kept.append(&mut sub.kept);
        report.deleted.append(&mut sub.deleted);
    }
    Ok(report)
}

async fn prune_one_kind(
    backup_dir: &Path,
    kind: ArtifactKind,
    daily_count: u32,
    weekly_count: u32,
) -> Result<RetentionReport, BackupError> {
    let entries = list_committed(backup_dir, kind).await?;

    // Group by date: keep the highest-suffix file per date as the "live"
    // representative; non-representatives are eligible for deletion
    // regardless of retention.
    let mut by_date: BTreeMap<NaiveDate, Vec<ParsedEntry>> = BTreeMap::new();
    for e in entries {
        by_date.entry(e.date).or_default().push(e);
    }

    let mut keep: Vec<ParsedEntry> = Vec::new();
    let mut superseded: Vec<ParsedEntry> = Vec::new();
    for (_, mut group) in by_date {
        group.sort_by_key(|e| std::cmp::Reverse(e.suffix.unwrap_or(0)));
        let mut it = group.into_iter();
        if let Some(winner) = it.next() {
            keep.push(winner);
        }
        for loser in it {
            superseded.push(loser);
        }
    }

    // Newest dates first.
    keep.sort_by_key(|e| std::cmp::Reverse(e.date));

    let mut kept_paths: Vec<PathBuf> = Vec::new();
    let mut delete_paths: Vec<PathBuf> = Vec::new();

    let daily_keep: Vec<&ParsedEntry> = keep.iter().take(daily_count as usize).collect();
    let daily_dates: std::collections::HashSet<NaiveDate> =
        daily_keep.iter().map(|e| e.date).collect();
    for e in &daily_keep {
        kept_paths.push(e.path.clone());
    }

    let mut weekly_taken = 0u32;
    for e in &keep {
        if weekly_taken >= weekly_count {
            break;
        }
        if e.date.weekday() == Weekday::Sun && !daily_dates.contains(&e.date) {
            kept_paths.push(e.path.clone());
            weekly_taken += 1;
        }
    }

    let kept_set: std::collections::HashSet<&PathBuf> = kept_paths.iter().collect();
    for e in &keep {
        if !kept_set.contains(&e.path) {
            delete_paths.push(e.path.clone());
        }
    }
    for e in superseded {
        delete_paths.push(e.path);
    }

    for p in &delete_paths {
        if let Err(err) = tokio::fs::remove_file(p).await {
            tracing::warn!(path = %p.display(), error = %err, "retention delete failed");
        }
    }

    Ok(RetentionReport {
        kept: kept_paths,
        deleted: delete_paths,
    })
}

#[derive(Debug, Clone)]
struct ParsedEntry {
    path: PathBuf,
    date: NaiveDate,
    suffix: Option<u32>,
}

async fn list_committed(
    backup_dir: &Path,
    kind: ArtifactKind,
) -> Result<Vec<ParsedEntry>, BackupError> {
    let mut rd = match tokio::fs::read_dir(backup_dir).await {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let prefix = kind.prefix();
    let ext = kind.extension();
    let mut out = Vec::new();
    while let Some(entry) = rd.next_entry().await? {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if let Some(parsed) = parse_filename(name, prefix, ext) {
            out.push(ParsedEntry {
                path,
                date: parsed.0,
                suffix: parsed.1,
            });
        }
    }
    Ok(out)
}

/// Parse a canonical artifact filename. Returns `Some((date, suffix))`
/// for committed files; `None` for `.partial` files, non-matching
/// prefixes/extensions, or malformed dates.
fn parse_filename(name: &str, prefix: &str, ext: &str) -> Option<(NaiveDate, Option<u32>)> {
    let suffix_with_dot = format!(".{ext}");
    let stem = name.strip_suffix(&suffix_with_dot)?;
    let body = stem.strip_prefix(prefix)?.strip_prefix('-')?;
    let (date_part, suffix) = match body.rsplit_once('-') {
        Some((d, s)) if s.chars().all(|c| c.is_ascii_digit()) && d.len() == 10 => {
            (d, Some(s.parse::<u32>().ok()?))
        }
        _ => (body, None),
    };
    let date = NaiveDate::parse_from_str(date_part, "%Y-%m-%d").ok()?;
    Some((date, suffix))
}

#[cfg(test)]
mod tests {
    use super::{parse_filename, prune};
    use chrono::NaiveDate;
    use std::path::Path;
    use tempfile::tempdir;

    #[test]
    fn parse_committed_no_suffix() {
        let (d, s) = parse_filename("postgres-2026-05-23.dump", "postgres", "dump").unwrap();
        assert_eq!(d, NaiveDate::from_ymd_opt(2026, 5, 23).unwrap());
        assert_eq!(s, None);
    }

    #[test]
    fn parse_committed_with_suffix() {
        let (d, s) = parse_filename("qdrant-2026-05-23-3.snapshot", "qdrant", "snapshot").unwrap();
        assert_eq!(d, NaiveDate::from_ymd_opt(2026, 5, 23).unwrap());
        assert_eq!(s, Some(3));
    }

    #[test]
    fn parse_partial_rejected() {
        assert!(parse_filename("postgres-2026-05-23.dump.partial", "postgres", "dump").is_none());
    }

    #[test]
    fn parse_wrong_prefix_rejected() {
        assert!(parse_filename("qdrant-2026-05-23.dump", "postgres", "dump").is_none());
    }

    #[tokio::test]
    async fn prune_keeps_newest_daily_and_sunday_weekly() {
        let dir = tempdir().unwrap();
        // 2026-05-23 is a Saturday; 2026-05-24 is Sunday; 2026-05-17 is Sunday.
        let dates = [
            "2026-05-10", // Sunday
            "2026-05-17", // Sunday
            "2026-05-23", // Saturday
            "2026-05-24", // Sunday
            "2026-05-25", // Monday
        ];
        for d in dates {
            tokio::fs::write(dir.path().join(format!("postgres-{d}.dump")), b"x")
                .await
                .unwrap();
            tokio::fs::write(dir.path().join(format!("qdrant-{d}.snapshot")), b"x")
                .await
                .unwrap();
        }

        let report = prune(dir.path(), 2, 2).await.unwrap();
        let kept_names: std::collections::HashSet<String> = report
            .kept
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();

        // Daily 2 newest per kind: 2026-05-25 and 2026-05-24.
        // Weekly: 2026-05-24 is already kept as daily, so next Sundays are
        // 2026-05-17 and 2026-05-10. weekly_count=2 → both kept.
        let expected_kept = [
            "postgres-2026-05-25.dump",
            "postgres-2026-05-24.dump",
            "postgres-2026-05-17.dump",
            "postgres-2026-05-10.dump",
            "qdrant-2026-05-25.snapshot",
            "qdrant-2026-05-24.snapshot",
            "qdrant-2026-05-17.snapshot",
            "qdrant-2026-05-10.snapshot",
        ];
        for name in expected_kept {
            assert!(kept_names.contains(name), "missing kept: {name}");
        }
        // 2026-05-23 should be deleted in both kinds (not in daily-top-2,
        // not a Sunday).
        assert!(!Path::new(&dir.path().join("postgres-2026-05-23.dump")).exists());
        assert!(!Path::new(&dir.path().join("qdrant-2026-05-23.snapshot")).exists());
    }

    #[tokio::test]
    async fn prune_keeps_highest_suffix_for_same_day() {
        let dir = tempdir().unwrap();
        for f in [
            "postgres-2026-05-23.dump",
            "postgres-2026-05-23-1.dump",
            "postgres-2026-05-23-2.dump",
        ] {
            tokio::fs::write(dir.path().join(f), b"x").await.unwrap();
        }
        let report = prune(dir.path(), 14, 4).await.unwrap();
        // -2 (highest suffix) is the representative; the base and -1 are
        // superseded and pruned.
        assert!(dir.path().join("postgres-2026-05-23-2.dump").exists());
        assert!(!dir.path().join("postgres-2026-05-23.dump").exists());
        assert!(!dir.path().join("postgres-2026-05-23-1.dump").exists());
        assert_eq!(report.deleted.len(), 2);
    }

    #[tokio::test]
    async fn prune_ignores_partials() {
        let dir = tempdir().unwrap();
        tokio::fs::write(dir.path().join("postgres-2026-05-23.dump.partial"), b"x")
            .await
            .unwrap();
        let report = prune(dir.path(), 14, 4).await.unwrap();
        assert!(report.kept.is_empty());
        assert!(report.deleted.is_empty());
        assert!(dir.path().join("postgres-2026-05-23.dump.partial").exists());
    }

    #[tokio::test]
    async fn prune_missing_dir_is_ok() {
        let dir = tempdir().unwrap();
        let nope = dir.path().join("does-not-exist");
        let report = prune(&nope, 14, 4).await.unwrap();
        assert!(report.kept.is_empty());
    }
}
