//! Query-time duplicate collapse (sprint 026, WI #641).
//!
//! Sprint 023 deliberately made **ingest** dedupe host-scoped —
//! `find_knowledge_by_content_hash(hash, file, machine)` — so that
//! per-host delete works. kai and kubs0 scan a synced `~/src`, so nearly
//! every chunk is stored twice. Measured on the live corpus: 221,982
//! points, 124,034 unique `content_hash`, 36,121 hashes present on both
//! hosts. A 10-result page was reliably 5 duplicate pairs: half of every
//! result page wasted.
//!
//! The fix here is **query-time**, needs no migration, and is applied
//! before rank fusion so freed ranks compact and the released slots fill
//! with new content instead of leaving holes.
//!
//! Semantics are Ken's ruling on WI #641, not this module's invention:
//!
//! - **Key on content only.** Metadata differences (host / file / repo)
//!   never keep two copies apart. Duplicate *storage* was never the
//!   problem; duplicate *top-k slots* were.
//! - **The survivor carries the collapsed set** as a list of point ids
//!   with per-copy metadata, rather than merged metadata — lossless, and
//!   a caller can reach any copy.
//! - **Top-k scope is fine.** If a third copy sits outside the fetch
//!   window we do not hunt for it. (A reverse index by SHA effectively
//!   exists — `content_hash` carries a Qdrant payload index — so
//!   completing the set would be one filtered lookup. Not in v1.)

use klams_types::KnowledgeCopy;

/// Collapse duplicates in a **best-first** ranked list.
///
/// `key` extracts the content key to group by; entries with no key
/// (facts and events carry no `content_hash`) are never collapsed and
/// always pass through. `as_copy` describes an entry as a
/// [`KnowledgeCopy`] for the survivor's `copies` list.
///
/// Returns each survivor paired with the duplicates it absorbed, in the
/// input's order. The **first** entry seen for a key survives, which is
/// the best-ranked one precisely because the input is best-first — so
/// collapse never promotes a worse match over a better one.
pub fn collapse_duplicates<T>(
    items: Vec<T>,
    key: impl Fn(&T) -> Option<String>,
    as_copy: impl Fn(&T) -> KnowledgeCopy,
) -> Vec<(T, Vec<KnowledgeCopy>)> {
    // Survivor position within `out`, by content key.
    let mut survivor_of: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    let mut out: Vec<(T, Vec<KnowledgeCopy>)> = Vec::with_capacity(items.len());
    for item in items {
        let Some(k) = key(&item) else {
            out.push((item, Vec::new()));
            continue;
        };
        if let Some(&pos) = survivor_of.get(&k) {
            out[pos].1.push(as_copy(&item));
        } else {
            survivor_of.insert(k, out.len());
            out.push((item, Vec::new()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    /// Test double for a ranked hit: a content key plus the identity a
    /// collapsed copy would carry.
    #[derive(Debug, Clone, PartialEq)]
    struct Hit {
        id: Uuid,
        hash: Option<&'static str>,
        host: &'static str,
    }

    fn hit(hash: Option<&'static str>, host: &'static str) -> Hit {
        Hit {
            id: Uuid::now_v7(),
            hash,
            host,
        }
    }

    fn collapse(items: Vec<Hit>) -> Vec<(Hit, Vec<KnowledgeCopy>)> {
        collapse_duplicates(
            items,
            |h| h.hash.map(str::to_string),
            |h| KnowledgeCopy {
                id: h.id,
                host: Some(h.host.to_string()),
                file: None,
            },
        )
    }

    #[test]
    fn cross_host_duplicate_pair_collapses_to_one_result() {
        // The live failure shape: the same chunk stored once per host.
        let kai = hit(Some("aaa"), "kai");
        let kubs0 = Hit {
            id: Uuid::now_v7(),
            ..kai.clone()
        };
        let kubs0 = Hit {
            host: "kubs0",
            ..kubs0
        };
        let out = collapse(vec![kai.clone(), kubs0.clone()]);
        assert_eq!(out.len(), 1, "a duplicate pair must occupy one slot");
        assert_eq!(out[0].0, kai, "the best-ranked copy survives");
        assert_eq!(out[0].1.len(), 1, "the survivor carries the collapsed copy");
        assert_eq!(out[0].1[0].id, kubs0.id);
        assert_eq!(out[0].1[0].host.as_deref(), Some("kubs0"));
    }

    #[test]
    fn metadata_never_keeps_two_copies_apart() {
        // Ken's ruling: key on content ONLY. Same hash, different host
        // AND different file — still one result.
        let a = hit(Some("aaa"), "kai");
        let b = hit(Some("aaa"), "kubs0");
        let out = collapse_duplicates(
            vec![a, b],
            |h| h.hash.map(str::to_string),
            |h| KnowledgeCopy {
                id: h.id,
                host: Some(h.host.to_string()),
                file: Some(format!("/{}/x.md", h.host)),
            },
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].1[0].file.as_deref(), Some("/kubs0/x.md"));
    }

    #[test]
    fn the_best_ranked_copy_survives_not_a_later_one() {
        // Input is best-first, so first-seen == best-ranked. If this
        // inverted, collapse would demote a strong match to a weak one.
        let best = hit(Some("aaa"), "kai");
        let worse = hit(Some("aaa"), "kubs0");
        let out = collapse(vec![best.clone(), worse]);
        assert_eq!(out[0].0.id, best.id);
    }

    #[test]
    fn distinct_content_is_untouched_and_keeps_its_order() {
        let a = hit(Some("aaa"), "kai");
        let b = hit(Some("bbb"), "kai");
        let c = hit(Some("ccc"), "kai");
        let out = collapse(vec![a.clone(), b.clone(), c.clone()]);
        assert_eq!(
            out.iter().map(|(h, _)| h.id).collect::<Vec<_>>(),
            vec![a.id, b.id, c.id]
        );
        assert!(out.iter().all(|(_, copies)| copies.is_empty()));
    }

    #[test]
    fn keyless_entries_are_never_collapsed_together() {
        // Facts and events carry no content_hash. Two of them must not
        // fuse into one just because both keys are None.
        let f1 = hit(None, "-");
        let f2 = hit(None, "-");
        let out = collapse(vec![f1, f2]);
        assert_eq!(out.len(), 2, "keyless entries always pass through");
        assert!(out.iter().all(|(_, copies)| copies.is_empty()));
    }

    #[test]
    fn three_copies_collapse_into_one_survivor_with_two_copies() {
        let a = hit(Some("aaa"), "kai");
        let b = hit(Some("aaa"), "kubs0");
        let c = hit(Some("aaa"), "cleo");
        let out = collapse(vec![a.clone(), b.clone(), c.clone()]);
        assert_eq!(out.len(), 1);
        assert_eq!(
            out[0].1.iter().map(|c| c.id).collect::<Vec<_>>(),
            vec![b.id, c.id],
            "copies are recorded in rank order"
        );
    }

    #[test]
    fn interleaved_duplicates_collapse_without_disturbing_the_rest() {
        // The live page shape: dup, other, dup, other.
        let a1 = hit(Some("aaa"), "kai");
        let b = hit(Some("bbb"), "kai");
        let a2 = hit(Some("aaa"), "kubs0");
        let c = hit(Some("ccc"), "kai");
        let out = collapse(vec![a1.clone(), b.clone(), a2.clone(), c.clone()]);
        assert_eq!(
            out.iter().map(|(h, _)| h.id).collect::<Vec<_>>(),
            vec![a1.id, b.id, c.id],
            "the surviving order is the input order minus the duplicate"
        );
        assert_eq!(out[0].1[0].id, a2.id, "the duplicate attaches to a1");
    }

    #[test]
    fn empty_input_is_empty_output() {
        assert!(collapse(Vec::new()).is_empty());
    }
}
