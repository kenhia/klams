//! Sprint 031 (#645) — the tool handlers must go through `trait Store`,
//! never through `CompositeStore`'s concrete `.postgres` / `.qdrant` /
//! `.embedder` fields.
//!
//! This is the WI's stated acceptance ("enforceable by a grep test"),
//! and it is worth enforcing mechanically rather than by review: a
//! single reach-through re-pins `McpState` to the concrete store for
//! every handler in the crate, which is how the MCP surface drifted
//! away from REST in the first place — 76 of them across 13 files by
//! the time anyone counted.
//!
//! Concretely, a reach-through means:
//!   - the tool cannot be exercised without Postgres, Qdrant and TEI
//!     all live, so its test gets `#[ignore]`d and rots; and
//!   - the write goes wherever the concrete method goes, bypassing
//!     whatever policy the shared path enforces.
//!
//! Hermetic: reads source text, touches no backend.

use std::path::Path;

/// Field accesses that only exist on `CompositeStore`.
const CONCRETE_FIELDS: [&str; 3] = [".postgres", ".qdrant", ".embedder"];

/// True if `line` reaches through to a concrete backend. Comments are
/// stripped first — the tool sources discuss these fields by name.
fn is_reachthrough(line: &str) -> bool {
    let code = line.split("//").next().unwrap_or(line);
    CONCRETE_FIELDS
        .iter()
        // `.postgres.method(...)`, or a bare `.postgres` line inside a
        // multi-line call chain. Both are reach-throughs.
        .any(|f| code.contains(&format!("{f}.")) || code.trim() == *f)
}

/// A detector that has quietly stopped detecting is worse than none —
/// it reports "clean" forever. Pin both directions.
#[test]
fn detector_recognises_a_reachthrough() {
    assert!(is_reachthrough(
        "    state.store.postgres.get_author_by_id(id)"
    ));
    assert!(is_reachthrough("                .qdrant"));
    assert!(is_reachthrough(
        "    let v = self.embedder.embed(text).await;"
    ));

    assert!(!is_reachthrough("    state.store.get_author_by_id(id)"));
    assert!(!is_reachthrough(
        "// reaches .postgres directly — it used to"
    ));
    assert!(!is_reachthrough("    /// See `.qdrant` in CompositeStore."));
}

#[test]
fn tool_handlers_hold_no_concrete_store_reachthroughs() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/tools");
    let mut offenders = Vec::new();

    for entry in std::fs::read_dir(&dir).expect("read src/tools") {
        let path = entry.expect("dir entry").path();
        if path.extension().is_none_or(|e| e != "rs") {
            continue;
        }
        let src = std::fs::read_to_string(&path).expect("read tool source");
        for (n, line) in src.lines().enumerate() {
            if is_reachthrough(line) {
                offenders.push(format!(
                    "{}:{}: {}",
                    path.file_name().unwrap_or_default().to_string_lossy(),
                    n + 1,
                    line.trim()
                ));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "tool handlers must reach the store through `trait Store`, not \
         `CompositeStore`'s concrete fields. Add the operation to the \
         trait (with a defaulted impl) and delegate from `CompositeStore` \
         — see sprint 031 / WI #645. Offenders:\n  {}",
        offenders.join("\n  ")
    );
}
