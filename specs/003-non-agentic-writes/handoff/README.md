# klams ↔ ansible-k integration handoff

```text
This document is pinned to klams sprint-003 API surface
(specs/003-non-agentic-writes/spec.md in the klams repo).
If GET /healthz?contract=v1 ever returns anything other than
200 with {"contract":"v1"}, the contract this document describes
is no longer guaranteed.
```

## Why you are reading this

`klams` is Ken's personal memory service: it stores user facts,
environment facts, knowledge chunks, and infra events, then exposes
them over a small HTTP surface for agents and operators to query.
ansible-k is integrating so every play run leaves a durable,
queryable record of what was true on each host — without an LLM in
the write path. One-sentence value proposition: **after a play
finishes, "what does kubs0 think is true?" becomes a single search
call instead of a `grep` through journal logs.**

## TL;DR

| What | Where | Auth | Minimal payload | Dedupe | Common failures |
|------|-------|------|-----------------|--------|-----------------|
| Post a UserFact | `POST /memory/userfact` ([api](api-contract.md#endpoint-table)) | Bearer ([auth](api-contract.md#auth-model)) | [`UserFact` example](api-contract.md#minimal-valid-payload-examples) | content-hash on `(subject_id, predicate, value)` ([dedupe](api-contract.md#dedupe-semantics)) | `409`, `422`, `5xx` ([failure modes](api-contract.md#failure-modes)) |
| Post an EnvFact | `POST /memory/envfact` ([api](api-contract.md#endpoint-table)) | Bearer | [`EnvFact` example](api-contract.md#minimal-valid-payload-examples) | content-hash on payload | `409`, `422`, `5xx` |
| Post a Service event | `POST /memory/events` ([api](api-contract.md#endpoint-table)) | Bearer | [`Service` event example](api-contract.md#minimal-valid-payload-examples) | none (events are append-only) | `422`, `5xx` |
| Verify klams is alive and on-contract | `GET /healthz?contract=v1` ([api](api-contract.md#endpoint-table)) | none | n/a | n/a | non-200 → contract drift, see [drift detection](api-contract.md#drift-detection) |

## Read order

1. [`spec.md`](spec.md) — the ansible-k-side spec for this integration
   (user stories, FRs, SCs). Drive your speckit cycle from this file.
2. [`api-contract.md`](api-contract.md) — wire format, auth, dedupe,
   failure modes, integration-shape recommendation, drift detection.
3. [`examples/post-userfact.sh`](examples/post-userfact.sh) — runnable
   POSIX `sh` walkthrough that posts a minimal `UserFact` against a
   live klams instance.
