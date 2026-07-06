# Contract: ansible-k Handoff Document Structure

**Sprint**: 003-non-agentic-writes
**Final destination**: `/home/ken/ansible-k/specs/klams-integration/`
**Staging path**: `sprints/003-non-agentic-writes/handoff/` (inside this repo)
**FR coverage**: FR-019, FR-020, FR-021 from [spec.md](../spec.md)

This contract describes the **shape** of the handoff document the
klams sprint ships to ansible-k. The actual content is authored as
part of Phase 2 tasks (see `tasks.md` once it exists).

## Required files

```text
handoff/
|-- README.md         # one-page orientation, pinned-version header
|-- spec.md           # speckit-compatible spec for the ansible-k side
|-- api-contract.md   # endpoint table, payloads, failure modes
`-- examples/
    `-- post-userfact.sh   # minimal curl walkthrough
```

Every file is plain markdown or shell. No tooling (npm, python venv)
required to read or execute.

## File-by-file requirements

### `README.md`

MUST contain, in order:

1. **Pinned-version header** — a literal block exactly matching:

   ```text
   This document is pinned to klams sprint-003 API surface
   (sprints/003-non-agentic-writes/spec.md in the klams repo).
   If GET /healthz?contract=v1 ever returns anything other than
   200 with {"contract":"v1"}, the contract this document describes
   is no longer guaranteed.
   ```

2. **One-paragraph orientation** — what klams is, why ansible-k is
   integrating with it, the one-sentence value proposition.
3. **TL;DR table** — endpoint, auth, minimal payload, dedupe, common
   failure modes. Each cell links to the relevant section of
   `api-contract.md`.
4. **Read order** — pointer to `spec.md` for the ansible-k side spec,
   `api-contract.md` for the wire format, `examples/` for working
   code.

### `spec.md`

MUST be speckit-compatible: a speckit `/specify` cycle run against
this file MUST produce coherent output. Minimum required sections:

- `# Feature Specification: klams integration in ansible-k`
- `**Input**: ...` header line
- `## User Scenarios & Testing` with at least one prioritized
  user story (the play-side wiring).
- `## Requirements` with concrete FRs the ansible-k owner will turn
  into tasks (e.g. "the callback plugin MUST post `UserFact` rows
  for every host in the play").
- `## Success Criteria` measurable SCs.
- `## Assumptions` — explicit dependency on klams sprint-003
  being deployed on the target machine.

The spec is **not** a description of klams' internals. It is the
ansible-k-side spec for the integration; klams' side is already
covered by this sprint's `spec.md`.

### `api-contract.md`

The data sheet. MUST contain, in order:

1. **Endpoint table** — every klams endpoint the integration will
   call, with HTTP method, URL, auth requirement, and a one-sentence
   purpose.
2. **Auth model** — how the bearer token is obtained, stored, and
   rotated (out-of-band — out of scope for klams).
3. **Minimal valid payload examples** — at least one each for
   `UserFact`, `EnvFact`, `Event(category=Service)`. Each example
   MUST be a JSON literal that, when POSTed to a sprint-003 klams
   instance, returns 200 (verified by the example script in
   `examples/`).
4. **Dedupe semantics** — explicit: "rerunning a play that emits
   identical facts produces zero new versions; only `last_used_at`
   advances."
5. **Failure modes table** — at minimum these rows:

   | Status | Meaning | Caller action |
   |--------|---------|---------------|
   | `200`  | canonical write | continue |
   | `202`  | accepted but diverted to dissents (only `AgentProposal` source) | log, do not retry |
   | `409`  | optimistic-concurrency mismatch | refetch, retry |
   | `422`  | validation error | fix payload, do not retry |
   | `5xx`  | transient | retry with backoff (recommended: jittered exponential, max 3 attempts) |

6. **Integration shape recommendation** — callback plugin (preferred)
   vs post-play hook (acceptable for ad-hoc plays). Trade-offs
   explicitly listed.
7. **Drift detection** — the `GET /healthz?contract=v1` convention
   from the pinned-version header; what to do if it fails.

### `examples/post-userfact.sh`

A POSIX `sh` script (`#!/usr/bin/env sh`, no bash-isms). MUST:

- Read `KLAMS_URL` and `KLAMS_TOKEN` from environment with sane
  defaults documented in comments at the top.
- Use only `curl` and `jq` (jq is optional — script must `--exit
  --code` and print the raw response if jq is absent).
- Post a minimal `UserFact` (Ken's name + kubs0 host) and pretty-print
  the response, including the `path` field added this sprint.
- Be executable with `chmod +x` set in the file mode at staging time.

## Acceptance — contract tests in this sprint

| Test | What it asserts |
|------|-----------------|
| `handoff_directory_layout_matches_contract` | The four required paths exist under `handoff/`. (`crates/klams-service/tests/us3d_handoff_layout.rs`, file-system only — no network.) |
| `handoff_example_script_posts_userfact` | The example script, run against the local test stack, succeeds with HTTP 200 and the response carries `path: "canonical"`. |
| `handoff_pinned_version_header_present` | `handoff/README.md` contains the literal pinned-version header above. |
| `handoff_api_contract_lists_required_failure_modes` | `handoff/api-contract.md` contains at minimum the six failure-mode rows in §3 above. |

## Ship step

The final task in this sprint (see `tasks.md` when generated) is the
`cp -r` from staging to the ansible-k location. The user (Ken) owns
the commit in the ansible-k repo; this sprint owns producing the
content.
