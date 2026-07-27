//! Code-aware chunking via tree-sitter (sprint 022 #323).
//!
//! Splits Rust and Python source at top-level item boundaries instead
//! of blindly by blank line, so a chunk is a coherent unit (a function,
//! struct, impl, class…) rather than an arbitrary window. Each chunk
//! also carries the symbol names it defines, which the graph layer
//! (sprint 025) consumes. Any parse failure or unsupported language
//! returns `None` and the caller falls back to the plain splitter.

use crate::chunk::{Lang, TARGET_CHARS};
use tree_sitter::{Language, Node, Parser};

/// Split normalized `src` into `(symbols, text)` blocks at top-level
/// item boundaries. Returns `None` for unsupported languages, a parser
/// error, or a source with no top-level items (caller falls back).
#[must_use]
pub fn code_blocks(src: &str, lang: Lang) -> Option<Vec<(Vec<String>, String)>> {
    let language: Language = match lang {
        Lang::Rust => tree_sitter_rust::LANGUAGE.into(),
        Lang::Python => tree_sitter_python::LANGUAGE.into(),
        _ => return None,
    };
    let mut parser = Parser::new();
    parser.set_language(&language).ok()?;
    let tree = parser.parse(src, None)?;
    let root = tree.root_node();

    // Top-level named items, in source order. Comments/attributes that
    // sit between items are *not* named children here, so we cover the
    // whole file by spanning each block from the end of the previous
    // item to the end of the current one — leading doc-comments and
    // attributes ride along with the item they document.
    let mut cursor = root.walk();
    let items: Vec<Node> = root.named_children(&mut cursor).collect();
    if items.is_empty() {
        return None;
    }

    let mut out: Vec<(Vec<String>, String)> = Vec::new();
    let mut start = 0usize;
    let mut syms: Vec<String> = Vec::new();
    for (i, node) in items.iter().enumerate() {
        if let Some(s) = node_symbol(*node, src, lang) {
            syms.push(s);
        }
        let end = node.end_byte();
        let is_last = i + 1 == items.len();
        // Flush when the accumulated span reaches the target size, or at
        // the end of the file.
        if end - start >= TARGET_CHARS || is_last {
            let text = src.get(start..end).unwrap_or("").trim().to_owned();
            if !text.is_empty() {
                for w in slide(&text) {
                    out.push((syms.clone(), w));
                }
            }
            start = end;
            syms.clear();
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// The identifier a top-level node defines, if it defines one.
fn node_symbol(node: Node, src: &str, lang: Lang) -> Option<String> {
    let field = match (lang, node.kind()) {
        (Lang::Rust, "impl_item") => "type",
        (
            Lang::Rust,
            "function_item" | "struct_item" | "enum_item" | "trait_item" | "mod_item"
            | "const_item" | "static_item" | "type_item" | "union_item" | "macro_definition",
        )
        | (Lang::Python, "function_definition" | "class_definition") => "name",
        _ => return None,
    };
    node.child_by_field_name(field)
        .and_then(|n| n.utf8_text(src.as_bytes()).ok())
        .map(str::to_owned)
}

/// Slide a `TARGET_CHARS` window (with overlap) over an oversized item,
/// mirroring the text chunker so a giant function still yields bounded
/// chunks. Reuses the chunk module's constants.
fn slide(s: &str) -> Vec<String> {
    use crate::chunk::OVERLAP_CHARS;
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= TARGET_CHARS {
        return vec![s.to_owned()];
    }
    let step = TARGET_CHARS - OVERLAP_CHARS;
    let mut out = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let end = (start + TARGET_CHARS).min(chars.len());
        out.push(chars[start..end].iter().collect());
        if end == chars.len() {
            break;
        }
        start += step;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_splits_by_item_and_extracts_symbols() {
        let src = "//! module doc\n\nuse std::io;\n\nfn alpha() -> u32 {\n    1\n}\n\nstruct Beta {\n    x: u32,\n}\n\nimpl Beta {\n    fn go(&self) {}\n}";
        let blocks = code_blocks(src, Lang::Rust).expect("rust parses");
        let all_syms: Vec<String> = blocks.iter().flat_map(|(s, _)| s.clone()).collect();
        assert!(all_syms.contains(&"alpha".to_string()), "syms={all_syms:?}");
        assert!(all_syms.contains(&"Beta".to_string()), "syms={all_syms:?}");
        // The `//! module doc` and `use` ride along, not lost.
        assert!(blocks.iter().any(|(_, t)| t.contains("module doc")));
        assert!(blocks.iter().any(|(_, t)| t.contains("fn alpha")));
    }

    #[test]
    fn python_extracts_def_and_class_names() {
        let src = "# a comment\nimport os\n\ndef go():\n    return 1\n\nclass Thing:\n    pass";
        let blocks = code_blocks(src, Lang::Python).expect("python parses");
        let all_syms: Vec<String> = blocks.iter().flat_map(|(s, _)| s.clone()).collect();
        assert!(all_syms.contains(&"go".to_string()), "syms={all_syms:?}");
        assert!(all_syms.contains(&"Thing".to_string()), "syms={all_syms:?}");
    }

    #[test]
    fn unsupported_language_returns_none() {
        assert!(code_blocks("x = 1", Lang::Toml).is_none());
    }

    #[test]
    fn empty_source_returns_none() {
        assert!(code_blocks("", Lang::Rust).is_none());
    }
}
