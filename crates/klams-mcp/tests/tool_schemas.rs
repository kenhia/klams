//! Sprint 018 (WI #307) — advertised tool schemas must be flat objects.
//!
//! The Anthropic API rejects any tool whose `input_schema` carries
//! `oneOf`/`allOf`/`anyOf` at the TOP level (400
//! `tools.N.custom.input_schema...`), which forced Anthropic-bound
//! agents to drop `memory_add` (its serde-tagged `MemoryAddContent`
//! enum generated a top-level `oneOf`). Property-level combinators are
//! fine; only the root is restricted. This test locks the invariant in
//! for every advertised tool so a future args refactor can't regress
//! it.

use klams_mcp::tools::all_tool_descriptors;

#[test]
fn no_tool_schema_has_top_level_combinators() {
    let tools = all_tool_descriptors();
    assert!(!tools.is_empty(), "descriptor list must not be empty");
    for tool in &tools {
        let schema = tool.schema_as_json_value();
        let obj = schema
            .as_object()
            .unwrap_or_else(|| panic!("{}: input_schema is not a JSON object", tool.name));
        for combinator in ["oneOf", "allOf", "anyOf", "$ref"] {
            assert!(
                !obj.contains_key(combinator),
                "{}: input_schema has top-level `{combinator}` — the Anthropic API rejects this:\n{}",
                tool.name,
                serde_json::to_string_pretty(&schema).unwrap()
            );
        }
        assert_eq!(
            obj.get("type").and_then(|t| t.as_str()),
            Some("object"),
            "{}: input_schema root must be type object",
            tool.name
        );
    }
}

/// WI #62 — the write tools advertise `author_id` as optional (it
/// falls back to the bearer token's bound author), while still listing
/// it as a property.
#[test]
fn write_tool_author_id_is_optional_in_schema() {
    for tool in all_tool_descriptors() {
        if !["memory_add", "memory_append_event", "dissent_propose"].contains(&tool.name.as_ref()) {
            continue;
        }
        let schema = tool.schema_as_json_value();
        let has_property = schema["properties"]
            .as_object()
            .is_some_and(|p| p.contains_key("author_id"));
        assert!(has_property, "{}: author_id property missing", tool.name);
        let required = schema["required"].as_array().cloned().unwrap_or_default();
        assert!(
            !required.iter().any(|r| r == "author_id"),
            "{}: author_id must not be required (bearer fallback, WI #62)",
            tool.name
        );
    }
}
