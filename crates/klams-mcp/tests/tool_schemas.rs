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

/// Sprint 019 (WI #309) — no boolean subschemas under `properties`,
/// anywhere in any advertised schema.
///
/// schemars renders a bare `serde_json::Value` field as the JSON-Schema
/// boolean any-value schema (`"field": true`). That is legal JSON
/// Schema, but Claude Code (2.1.205) rejects boolean *property*
/// subschemas and discards the ENTIRE tool list on the first invalid
/// tool — `memory_append_event.payload` alone took all 8 klams tools
/// away from every Claude session. `additionalProperties: true` is
/// fine (ubiquitous; verified accepted).
#[test]
fn no_boolean_property_subschemas_anywhere() {
    fn walk(tool: &str, path: &str, schema: &serde_json::Value) {
        let Some(obj) = schema.as_object() else {
            return;
        };
        for (key, value) in obj {
            let child_path = format!("{path}/{key}");
            if key == "properties" {
                let props = value
                    .as_object()
                    .unwrap_or_else(|| panic!("{tool}: {child_path} is not an object"));
                for (prop, prop_schema) in props {
                    assert!(
                        !prop_schema.is_boolean(),
                        "{tool}: property {child_path}/{prop} is a boolean schema \
                         ({prop_schema}) — Claude Code rejects these and drops the \
                         whole tool list; give the field a real schema \
                         (e.g. #[schemars(with = \"serde_json::Map<String, serde_json::Value>\")])",
                    );
                    walk(tool, &format!("{child_path}/{prop}"), prop_schema);
                }
            } else {
                walk(tool, &child_path, value);
            }
        }
    }
    for tool in all_tool_descriptors() {
        walk(&tool.name, "", &tool.schema_as_json_value());
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

/// Sprint 046 (WI #850) — every advertised property must constrain its
/// shape. No `type`, no `enum`, no combinator = an untyped property.
///
/// This is #309's rule widened to the class it should have covered.
/// #309 fixed `memory_append_event.payload` (a bare
/// `serde_json::Value`, which schemars renders as the boolean schema
/// `true`) and guarded it with
/// `no_boolean_property_subschemas_anywhere`. But
/// `Option<serde_json::Value>` renders as `{"description": …}` — an
/// object schema with no `type`, not a boolean — so `memory_add.payload`
/// and `dissent_propose.proposed_payload` walked under that guard and
/// stayed uncallable from every MCP client for months (#850). Clients
/// do not send structured data for a field whose schema never says it
/// takes any, so the `is_object()` guard inside the tool refuses the
/// call before it reaches the store.
///
/// The narrow test stays — a boolean subschema is a distinct client
/// failure (Claude Code drops the whole tool list on one). This is the
/// general rule beneath it.
#[test]
fn no_unconstrained_property_subschemas_anywhere() {
    const CONSTRAINTS: [&str; 7] = ["type", "enum", "const", "oneOf", "anyOf", "allOf", "$ref"];

    fn walk(tool: &str, path: &str, schema: &serde_json::Value) {
        let Some(obj) = schema.as_object() else {
            return;
        };
        for (key, value) in obj {
            let child_path = format!("{path}/{key}");
            if key == "properties" {
                let props = value
                    .as_object()
                    .unwrap_or_else(|| panic!("{tool}: {child_path} is not an object"));
                for (prop, prop_schema) in props {
                    let prop_path = format!("{child_path}/{prop}");
                    let constrained = prop_schema
                        .as_object()
                        .is_some_and(|s| CONSTRAINTS.iter().any(|c| s.contains_key(*c)));
                    assert!(
                        constrained,
                        "{tool}: property {prop_path} declares no shape ({prop_schema}) — \
                         a client will not send structured data for an untyped field, so a \
                         tool guarding on it (e.g. `payload.is_object()`) refuses every call. \
                         Give it a real schema \
                         (e.g. #[schemars(with = \"serde_json::Map<String, serde_json::Value>\")])",
                    );
                    walk(tool, &prop_path, prop_schema);
                }
            } else {
                walk(tool, &child_path, value);
            }
        }
    }
    for tool in all_tool_descriptors() {
        walk(&tool.name, "", &tool.schema_as_json_value());
    }
}

/// Sprint 046 (WI #850) — the three free-form payload fields, named.
///
/// The general rule above is the guard; this is the regression pin for
/// the specific fields the bug was reported against, so a failure names
/// the tool an agent could not call rather than a JSON pointer.
#[test]
fn free_form_payload_fields_advertise_object_type() {
    const FIELDS: [(&str, &str); 4] = [
        ("memory_add", "payload"),
        ("dissent_propose", "proposed_payload"),
        ("register_author", "extra"),
        ("memory_append_event", "payload"),
    ];
    for (tool_name, field) in FIELDS {
        let tool = all_tool_descriptors()
            .into_iter()
            .find(|t| t.name.as_ref() == tool_name)
            .unwrap_or_else(|| panic!("{tool_name}: not advertised"));
        let schema = tool.schema_as_json_value();
        let prop = schema["properties"]
            .get(field)
            .unwrap_or_else(|| panic!("{tool_name}.{field}: property missing"));
        // `object`, or `["object", "null"]` for an optional field.
        let admits_object = match &prop["type"] {
            serde_json::Value::String(s) => s == "object",
            serde_json::Value::Array(v) => v.iter().any(|t| t == "object"),
            _ => false,
        };
        assert!(
            admits_object,
            "{tool_name}.{field}: schema does not admit an object ({prop}) — \
             clients will not send one, and the tool's `is_object()` guard \
             then refuses every call (#850)",
        );
        // A default must not contradict the type it defaults for.
        if let Some(default) = prop.get("default") {
            assert!(
                default.is_object() || default.is_null() && prop["type"].is_array(),
                "{tool_name}.{field}: default {default} contradicts type {}",
                prop["type"],
            );
        }
    }
}
