# Sprint 004 — EnvFact `value` accepts JSON

**Branch**: `004-envfact-value-json`
**Started**: 2026-05-20
**Type**: Abbreviated sprint (single-PR, no full spec-kit cycle).

## Purpose

The current `EnvFactValidator` enforces `payload.value` as a string ≤
4096 chars. ansible-k's host-facts data is a tree of mixed scalars and
arrays per category (`hardware`, `network`, `storage.mounts`, …), and
the only way to push it today is to JSON-encode each category blob into
a string field — every consumer then has to `value::jsonb -> ...` to
read it back, and the serialized blob easily approaches the 4 KB cap.

This sprint relaxes `value` to accept any JSON value, capped at 16 KB
serialized. A bare string remains valid (it is valid JSON), so existing
rows and existing scalar-style writes (`{"key":"GPU_COUNT","value":"4"}`)
keep working unchanged.

A bigger redesign — a separate `EnvDoc` entity for structured
per-(host, category) documents — is **deferred** until we have at
least one agent consumer to tell us what read queries actually look
like. Background analysis: see
[specs/planning/envfact-schema-analysis.md](../planning/envfact-schema-analysis.md).

## Scope

In:

1. `crates/klams-core/src/validate/facts.rs` — drop string-only rule on
   `value`; replace 4096-char cap with a 16 KB serialized-bytes cap on
   the entire JSON value.
2. Validator unit tests for both shapes (string still passes; dict
   passes; oversize JSON fails; missing value still fails).
3. `crates/klams-api/tests/contract_facts.rs` — add a contract case
   posting an EnvFact with a dict-valued `value` and asserting 200 +
   canonical path.
4. `crates/klams-service/tests/us3b_ansible_facts.rs` — add (or extend)
   a case exercising the dict shape end-to-end.
5. Update handoff doc
   `specs/003-non-agentic-writes/handoff/api-contract.md`:
   - Replace the broken EnvFact example (free-form top-level keys)
     with the real `{key, value}` shape.
   - Show both a scalar value (`"4"`) and a structured value
     (`{"count": 4, "models": ["RTX 4080 SUPER"]}`).
   - State the 16 KB serialized cap.
6. Mark the prior fix-forward note SUPERSEDED on the ansible-k side
   (`specs/klams-integration/fixforward-source-and-encoding.md`) and
   leave a corrected note pointing at the new contract.
7. Update klams `docs/usage.md` if it shows EnvFact examples.

Out:

- `EnvDoc` entity / table / endpoint — deferred (YAGNI).
- FTS / GIN index changes — `to_tsvector('english', payload::text)`
  and `payload jsonb_path_ops` already cover JSON values without any
  migration.
- Migration — none needed; column is already JSONB; existing rows are
  valid.
- OpenAPI schema — `UpsertFactRequest.payload` is already
  `{type: object, additionalProperties: true}`; no contract change
  observable at the OpenAPI level.

## Constitution gate

- **SDD**: this README is the spec.
- **TDD**: validator tests written first, then loosen rule until green;
  contract test added before docs.
- **Code Standards**: `cargo fmt`, `cargo clippy --all-targets
  --all-features -- -D warnings`, `cargo test` clean.
- **Documentation**: handoff `api-contract.md` + this brief updated as
  part of done.
- **Quality**: validator error stays actionable —
  `field: payload.value, rule: size, message: "value must be <= 16384
  serialized bytes"`.
- **Simplicity**: smallest change that unblocks the consumer; no new
  entities.

## Done criteria

- [X] Validator accepts any JSON for `value`; rejects > 16 KB
      serialized.
- [X] All previously-green tests still pass; new tests cover both
      shapes and the size cap.
- [X] `cargo fmt --check && cargo clippy --all-targets --all-features
      -- -D warnings && cargo test` clean.
- [X] Handoff `api-contract.md` updated.
- [X] ansible-k corrected-note delivered (replaces / annotates the
      prior fix-forward).
- [ ] Branch merged via PR.

## Implementation summary (2026-05-20)

- Validator: `crates/klams-core/src/validate/facts.rs` — drop
  `Some(serde_json::Value::String(s)) =>` branch on `value`; replace
  with size-only check via `serde_json::to_string(v).len() <=
  ENV_FACT_VALUE_MAX_BYTES` (16 KiB). Constant exported.
- Tests: 7 new unit tests in `env_fact_value_tests` covering string,
  dict, array, numeric, null, missing, oversize, and at-cap. Contract
  tests in `crates/klams-api/tests/contract_facts.rs` for dict-shaped
  EnvFact (200 + path canonical + structure preserved on round-trip)
  and oversize rejection (422 size). End-to-end test in
  `crates/klams-service/tests/us3b_ansible_facts.rs` —
  `ansible_env_fact_with_dict_value_round_trips` — passing against
  the docker-compose.test.yml stack.
- Docs: handoff `api-contract.md` EnvFact section rewritten with
  scalar + structured examples and the 16 KiB cap.
- ansible-k side: original fix-forward marked SUPERSEDED;
  `specs/klams-integration/bug2-corrected.md` written with the
  actual root cause, the now-correct guidance, a probe to verify
  sprint 004 is live, and an apology.
