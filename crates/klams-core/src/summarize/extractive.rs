//! Rule-based extractive summarization: event headlines and chunk excerpts.
//!
//! Sprint 005 (Phase 4) — T034. Pure functions, no I/O. Two
//! mechanisms:
//!
//! * [`event_headline`] — top-K category counts + time-bracket
//!   phrasing for an `(host, category, day_bucket)` cluster.
//! * [`knowledge_excerpt`] — longest-representative chunk
//!   selection across a `(repo, file_prefix)` cluster.
//!
//! See research.md D-005 and D-006.

use std::collections::HashMap;

/// Pre-aggregated cluster of events for one `(host, category, day_bucket)`.
#[derive(Debug, Clone)]
pub struct EventCluster<'a> {
    pub host: &'a str,
    pub category: &'a str,
    /// (sub-category → count) — top-3 are reported in the headline.
    pub sub_counts: HashMap<&'a str, u32>,
    pub total: u32,
    pub earliest_iso: Option<&'a str>,
    pub latest_iso: Option<&'a str>,
}

/// Build a one-line event headline. Stable across runs.
#[must_use]
pub fn event_headline(cluster: &EventCluster<'_>) -> String {
    let mut entries: Vec<(&&str, &u32)> = cluster.sub_counts.iter().collect();
    entries.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
    let top: Vec<String> = entries
        .into_iter()
        .take(3)
        .map(|(name, count)| format!("{count}× {name}"))
        .collect();
    let bracket = time_bracket(cluster.earliest_iso, cluster.latest_iso);
    let tail = if top.is_empty() {
        String::new()
    } else {
        format!(": {}", top.join(", "))
    };
    format!(
        "{host}/{category}{bracket} — {total} events{tail}",
        host = cluster.host,
        category = cluster.category,
        total = cluster.total,
    )
}

fn time_bracket(earliest: Option<&str>, latest: Option<&str>) -> String {
    match (earliest, latest) {
        (Some(e), Some(l)) => format!(" [{}–{}]", hhmm(e), hhmm(l)),
        _ => String::new(),
    }
}

fn hhmm(iso: &str) -> String {
    iso.split('T')
        .nth(1)
        .and_then(|t| t.get(0..5))
        .unwrap_or("--:--")
        .to_string()
}

/// Pre-aggregated cluster of stale knowledge chunks.
#[derive(Debug, Clone)]
pub struct KnowledgeCluster<'a> {
    pub repo: &'a str,
    pub file_prefix: &'a str,
    pub chunks: Vec<&'a str>,
}

/// Pick the longest representative chunk, capped to ~600 chars.
#[must_use]
pub fn knowledge_excerpt(cluster: &KnowledgeCluster<'_>) -> String {
    let Some(best) = cluster.chunks.iter().max_by_key(|c| c.len()).copied() else {
        return String::new();
    };
    let trimmed: String = best.chars().take(600).collect();
    format!(
        "[{repo}/{prefix}*] {trimmed}",
        repo = cluster.repo,
        prefix = cluster.file_prefix,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headline_reports_top_three_sub_categories_by_count() {
        let mut sub = HashMap::new();
        sub.insert("oom", 10u32);
        sub.insert("restart", 5u32);
        sub.insert("evict", 7u32);
        sub.insert("crash", 1u32);
        let c = EventCluster {
            host: "kubs0",
            category: "pod",
            sub_counts: sub,
            total: 23,
            earliest_iso: Some("2025-01-01T08:30:00Z"),
            latest_iso: Some("2025-01-01T17:15:00Z"),
        };
        let h = event_headline(&c);
        assert!(h.contains("10× oom"), "{h}");
        assert!(h.contains("7× evict"), "{h}");
        assert!(h.contains("5× restart"), "{h}");
        assert!(!h.contains("1× crash"), "{h}");
        assert!(h.contains("kubs0/pod"));
        assert!(h.contains("[08:30–17:15]"));
        assert!(h.contains("23 events"));
    }

    #[test]
    fn headline_handles_no_subcategories() {
        let c = EventCluster {
            host: "h",
            category: "c",
            sub_counts: HashMap::new(),
            total: 0,
            earliest_iso: None,
            latest_iso: None,
        };
        let h = event_headline(&c);
        assert!(h.starts_with("h/c"));
        assert!(h.contains("0 events"));
    }

    #[test]
    fn excerpt_picks_longest_chunk_and_caps_at_600_chars() {
        let long = "x".repeat(900);
        let chunks = vec!["short", "medium one", long.as_str()];
        let c = KnowledgeCluster {
            repo: "myrepo",
            file_prefix: "doc/",
            chunks,
        };
        let out = knowledge_excerpt(&c);
        assert!(out.starts_with("[myrepo/doc/*] "));
        assert_eq!(out.chars().count(), 600 + "[myrepo/doc/*] ".len());
    }

    #[test]
    fn excerpt_empty_cluster_returns_empty_string() {
        let c = KnowledgeCluster {
            repo: "r",
            file_prefix: "p",
            chunks: vec![],
        };
        assert!(knowledge_excerpt(&c).is_empty());
    }

    #[test]
    fn headline_is_stable_across_runs() {
        let mut sub = HashMap::new();
        sub.insert("a", 5u32);
        sub.insert("b", 5u32);
        let c = EventCluster {
            host: "h",
            category: "c",
            sub_counts: sub,
            total: 10,
            earliest_iso: None,
            latest_iso: None,
        };
        let h1 = event_headline(&c);
        let h2 = event_headline(&c);
        assert_eq!(h1, h2);
        let idx_a = h1.find("5× a").unwrap();
        let idx_b = h1.find("5× b").unwrap();
        assert!(idx_a < idx_b);
    }
}
