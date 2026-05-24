//! Sprint 006 T018 (US1) — filesystem retention pruning. Pure
//! filesystem; no external stack required.

use klams_store::backup::retention;
use tempfile::tempdir;
use tokio::fs;

#[tokio::test]
async fn prune_keeps_daily_and_weekly_drops_rest_per_kind() {
    let dir = tempdir().unwrap();
    let dates = [
        "2026-05-03", // Sunday
        "2026-05-10", // Sunday
        "2026-05-17", // Sunday
        "2026-05-21", // Thursday
        "2026-05-23", // Saturday
        "2026-05-24", // Sunday
        "2026-05-25", // Monday
        "2026-05-26", // Tuesday
    ];
    for d in dates {
        fs::write(dir.path().join(format!("postgres-{d}.dump")), b"x")
            .await
            .unwrap();
        fs::write(dir.path().join(format!("qdrant-{d}.snapshot")), b"x")
            .await
            .unwrap();
    }

    let report = retention::prune(dir.path(), 3, 2).await.unwrap();

    let kept: std::collections::HashSet<String> = report
        .kept
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();

    // Daily-newest-3 per kind: 2026-05-26, 2026-05-25, 2026-05-24.
    // Weekly Sundays (next 2 after excluding daily Sundays already kept,
    // i.e. 2026-05-24): 2026-05-17 and 2026-05-10.
    for kind in ["postgres", "qdrant"] {
        let ext = if kind == "postgres" {
            "dump"
        } else {
            "snapshot"
        };
        for d in ["2026-05-26", "2026-05-25", "2026-05-24"] {
            assert!(
                kept.contains(&format!("{kind}-{d}.{ext}")),
                "missing daily {kind}-{d}"
            );
        }
        for d in ["2026-05-17", "2026-05-10"] {
            assert!(
                kept.contains(&format!("{kind}-{d}.{ext}")),
                "missing weekly {kind}-{d}"
            );
        }
        // Filer days: dropped from disk.
        for d in ["2026-05-21", "2026-05-23", "2026-05-03"] {
            assert!(
                !dir.path().join(format!("{kind}-{d}.{ext}")).exists(),
                "{kind}-{d}.{ext} should have been pruned"
            );
        }
    }
}

#[tokio::test]
async fn same_day_suffix_treated_as_single_date_keep_highest_suffix() {
    let dir = tempdir().unwrap();
    // Three same-day files; -2 wins as the representative.
    for f in [
        "postgres-2026-05-26.dump",
        "postgres-2026-05-26-1.dump",
        "postgres-2026-05-26-2.dump",
    ] {
        fs::write(dir.path().join(f), b"x").await.unwrap();
    }
    let report = retention::prune(dir.path(), 14, 4).await.unwrap();
    assert!(dir.path().join("postgres-2026-05-26-2.dump").exists());
    assert!(!dir.path().join("postgres-2026-05-26.dump").exists());
    assert!(!dir.path().join("postgres-2026-05-26-1.dump").exists());
    assert_eq!(report.deleted.len(), 2);
}

#[tokio::test]
async fn retention_ignores_partials() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("postgres-2026-05-23.dump.partial"), b"x")
        .await
        .unwrap();
    let report = retention::prune(dir.path(), 14, 4).await.unwrap();
    assert!(report.kept.is_empty());
    assert!(dir.path().join("postgres-2026-05-23.dump.partial").exists());
}
