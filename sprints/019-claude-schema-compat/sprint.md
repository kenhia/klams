# Sprint 019 — schema compat for Claude Code (no boolean subschemas)

**Branch:** `019-claude-schema-compat`
**korg:** proposal 310 — WI #309
**Type:** hotfix follow-up to sprint 018 / WI 307. That fix removed the
top-level `oneOf` the Anthropic *API* rejects; live wiring then showed
Claude *Code* (v2.1.205) additionally validates every property
subschema and rejects JSON-Schema **boolean** subschemas.

## Goal

Claude Code connects to klams (`Connected`) but reports
`tools fetch failed` and loads zero tools, because
`memory_append_event`'s `payload: serde_json::Value` renders in
schemars as the boolean any-value schema (`"payload": true`) and
Claude discards the whole tool list on the first invalid tool. GHCP
is lenient and unaffected. Fix the schema so Claude Code loads all 8
tools on cleo, kai, and kubs0.

## Scope

1. Annotate `memory_append_event.payload` to render an object schema
   (`#[schemars(with = "serde_json::Map<String, serde_json::Value>")]`),
   matching the pattern `event_search.payload_match` already uses.
   Wire shape unchanged — the field still deserializes any JSON object
   it did before (payloads have always been JSON objects in practice;
   the handler-level contract is unchanged).
2. Extend the 018 schema contract test (`tests/tool_schemas.rs`) to
   walk every advertised tool's schema **recursively** and fail on any
   boolean subschema under `properties` — so a future bare
   `serde_json::Value` field can't silently re-break Claude.
   `additionalProperties: true` is allowed (ubiquitous, and Claude
   accepts it — WI 309 diagnostics).
3. Deploy to kubs0; verify live per WI 309: `claude mcp get klams`
   shows Connected with all 8 tools (no `tools fetch failed`), a
   headless `memory_search` tool call returns results, and
   `event_search` (tool 5, never reached before) validates too.

## Out of scope

- Changing `memory_add.payload` (`Option<Value>` renders acceptably —
  verified by the same recursive test).
- Any behavioral change to event payload validation.

## Acceptance

- Contract test fails on the pre-fix schema, passes post-fix; no
  advertised tool schema contains a boolean property subschema.
- `just gate` green; existing `mcp_phase5` (append_event round-trip)
  passes unchanged.
- Live on kubs0: `claude mcp get klams` → 8 tools, and a real Claude
  Code `memory_search` call succeeds.

## Chronicle

- (2026-07-09) Opened from korg proposal 310 (WI 309, found while
  wiring Claude Code up via the new `docs/klams-mcp-for-agents.md`).
  Diagnostics in the WI: raw `tools/list` is healthy, Claude aborts
  after negotiating protocol 2025-11-25 because client-side schema
  validation trips on tool 4 (`memory_append_event`); the only other
  boolean subschema in the live surface is
  `event_search.payload_match.additionalProperties: true`, which is
  standard and accepted.
- (2026-07-09) Shipped the fix TDD-style: the new recursive
  `no_boolean_property_subschemas_anywhere` contract test failed on
  the pre-fix schema exactly at `memory_append_event/payload`, passed
  after the `#[schemars(with = ...)]` annotation. Gate green;
  `mcp_phase5` live round-trip unchanged. Deployed `0.1.19` to kubs0.
  Live acceptance all green: `claude mcp get klams` → `✔ Connected`
  (no `tools fetch failed`), and a headless Claude Code
  `memory_search` call returned 3 scored hits — since Claude rejects
  the tool list all-or-nothing, this also confirms `event_search`'s
  `additionalProperties: true` validates fine.
