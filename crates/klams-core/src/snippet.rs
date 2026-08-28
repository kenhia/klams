//! Match-window snippets (sprint 046, WI #1178).
//!
//! Compact search responses are only compact if the snippet carries the
//! answer. khound's own retro found the failure mode: its file hits
//! snippeted the first N characters of the document, so a document that
//! was *right* still forced a follow-up read, and the follow-up read is
//! what the token model charges. Curated klams memories are long and
//! frequently open with a heading or a preamble, so head-of-text is
//! close to the worst possible choice here.
//!
//! So the window is placed over the part of the text that actually
//! matched the query, not over its beginning.

/// Snippet budget in characters, matching khound's contract.
///
/// Applied identically to every kind so no kind quietly costs more per
/// rank than another.
pub const SNIPPET_BUDGET: usize = 320;

/// Minimum length for a query token to count as a match anchor.
/// One- and two-character tokens ("a", "of", "is") match everywhere and
/// would anchor the window at position 0, which is the head-of-text
/// behaviour this module exists to avoid.
const MIN_TOKEN_LEN: usize = 3;

/// Build a snippet of at most [`SNIPPET_BUDGET`] characters, centred on
/// the densest run of query-term matches in `text`.
///
/// Falls back to the head of the text when the query has no usable
/// tokens or none of them occur — a lexical hit can rank on a stem the
/// naive matcher here does not reproduce, and a head snippet is a
/// better answer than an empty one.
///
/// Boundaries are snapped outward to whitespace where that costs less
/// than a word, and elisions are marked with a leading/trailing `…` so
/// the reader can tell a window from a whole short memory.
#[must_use]
pub fn match_window(text: &str, query: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= SNIPPET_BUDGET {
        return text.trim().to_string();
    }

    let start = best_window_start(&chars, query);
    let end = (start + SNIPPET_BUDGET).min(chars.len());
    let (start, end) = snap_to_word_boundaries(&chars, start, end);

    let mut out = String::new();
    if start > 0 {
        out.push('…');
    }
    out.extend(chars[start..end].iter());
    let out = out.trim_end().to_string();
    let mut out = out;
    if end < chars.len() {
        out.push('…');
    }
    out
}

/// Lowercased query tokens worth anchoring on.
fn anchors(query: &str) -> Vec<String> {
    query
        .split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
        .filter(|t| t.chars().count() >= MIN_TOKEN_LEN)
        .map(str::to_lowercase)
        .collect()
}

/// Character offset of the window that covers the most match positions.
///
/// Scores every match position by how many other matches fall within a
/// budget-wide window starting a little before it, then centres on the
/// winner. This favours a region where several query terms co-occur
/// over the first place any single term happens to appear.
fn best_window_start(chars: &[char], query: &str) -> usize {
    let anchors = anchors(query);
    if anchors.is_empty() {
        return 0;
    }
    let lowered: String = chars.iter().flat_map(|c| c.to_lowercase()).collect();
    // char index of each match, for every anchor
    let mut hits: Vec<usize> = Vec::new();
    for a in &anchors {
        let mut from = 0usize;
        while let Some(byte_pos) = lowered[from..].find(a.as_str()) {
            let abs = from + byte_pos;
            hits.push(lowered[..abs].chars().count());
            from = abs + a.len();
        }
    }
    if hits.is_empty() {
        return 0;
    }
    hits.sort_unstable();

    let mut best_start = 0usize;
    let mut best_score = 0usize;
    for &h in &hits {
        // Put a little context before the anchor rather than starting on it.
        let start = h.saturating_sub(SNIPPET_BUDGET / 4);
        let end = start + SNIPPET_BUDGET;
        let score = hits.iter().filter(|&&x| x >= start && x < end).count();
        if score > best_score {
            best_score = score;
            best_start = start;
        }
    }
    best_start.min(chars.len().saturating_sub(SNIPPET_BUDGET))
}

/// Nudge both ends to whitespace so the window does not start or end
/// mid-word — but only by a few characters, never by enough to lose
/// content worth keeping.
fn snap_to_word_boundaries(chars: &[char], start: usize, end: usize) -> (usize, usize) {
    const SLACK: usize = 24;

    let mut s = start;
    if s > 0 {
        let limit = (start + SLACK).min(end);
        while s < limit && !chars[s].is_whitespace() {
            s += 1;
        }
        while s < limit && chars[s].is_whitespace() {
            s += 1;
        }
        if s >= limit {
            s = start;
        }
    }

    let mut e = end;
    if e < chars.len() {
        let limit = e.saturating_sub(SLACK).max(s);
        while e > limit && !chars[e - 1].is_whitespace() {
            e -= 1;
        }
        if e <= limit {
            e = end;
        }
    }
    (s, e)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn long_text(marker: &str) -> String {
        let filler = "lorem ipsum dolor sit amet consectetur adipiscing elit ".repeat(20);
        format!("{filler} {marker} {filler}")
    }

    #[test]
    fn short_text_is_returned_whole_without_ellipsis() {
        let t = "a short memory about tailscale serve";
        assert_eq!(match_window(t, "tailscale"), t);
    }

    /// The regression this module exists for: khound snippeted the head
    /// of the document and paid for it in follow-up reads.
    #[test]
    fn window_covers_the_match_not_the_head() {
        let text = long_text("the TEI embedding endpoint is configured in klams.toml");
        let snip = match_window(&text, "TEI embedding endpoint");
        assert!(
            snip.contains("TEI embedding endpoint"),
            "snippet must carry the matched region, got: {snip}"
        );
        assert!(snip.starts_with('…'), "expected an elided head: {snip}");
    }

    #[test]
    fn respects_the_budget() {
        let text = long_text("needle");
        let snip = match_window(&text, "needle");
        // budget + up to two ellipsis chars
        assert!(
            snip.chars().count() <= SNIPPET_BUDGET + 2,
            "snippet is {} chars, budget {SNIPPET_BUDGET}",
            snip.chars().count()
        );
    }

    #[test]
    fn prefers_the_region_where_several_terms_co_occur() {
        let filler = "alpha ".repeat(200);
        // "serve" alone appears early; both terms co-occur late.
        let text = format!("{filler} serve {filler} tailscale serve port conflict {filler}");
        let snip = match_window(&text, "tailscale serve");
        assert!(
            snip.contains("tailscale serve port conflict"),
            "expected the co-occurrence window, got: {snip}"
        );
    }

    #[test]
    fn falls_back_to_head_when_nothing_matches() {
        let text = long_text("needle");
        let snip = match_window(&text, "zzzzz");
        assert!(!snip.starts_with('…'), "expected a head snippet: {snip}");
        assert!(snip.ends_with('…'), "expected a truncation mark: {snip}");
    }

    /// Short tokens match everywhere; anchoring on them collapses back
    /// to head-of-text.
    #[test]
    fn ignores_tokens_too_short_to_anchor() {
        let text = long_text("the quick brown fox jumps");
        let snip = match_window(&text, "a of is the brown fox");
        assert!(
            snip.contains("brown fox"),
            "short tokens should not drag the window to the head: {snip}"
        );
    }

    #[test]
    fn handles_multibyte_text_without_panicking() {
        let text = "日本語のテキスト ".repeat(100) + "タイルスケール設定" + &"日本語 ".repeat(100);
        let snip = match_window(&text, "タイルスケール設定");
        assert!(snip.contains("タイルスケール設定"), "got: {snip}");
        assert!(snip.chars().count() <= SNIPPET_BUDGET + 2);
    }
}
