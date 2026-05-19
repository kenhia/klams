# klams API contract (sprint-003)

For the orientation, see [README.md](README.md). For the ansible-k
side spec, see [spec.md](spec.md).

## Endpoint table

| Method | URL | Auth | Purpose |
|--------|-----|------|---------|
| `POST` | `/memory/facts` | bearer | Upsert a typed fact (`UserFact` or `EnvFact`). 200 on canonical write, 202 on dissent diversion, 409 on optimistic-concurrency mismatch, 422 on validation error. |
| `POST` | `/memory/events` | bearer | Append an immutable event (`category` discriminates: `Service`, `Execution`, etc.). 200 on accept. |
| `POST` | `/memory/knowledge/index` | bearer | Index a knowledge chunk (free-form text + source/repo/file metadata). Idempotent on content hash. |
| `POST` | `/memory/knowledge/delete?source_file=<abs>` | bearer | Delete every knowledge chunk whose `file` matches. Used by the scanner when a file vanishes. |
| `POST` | `/memory/search` | bearer | Unified vector + filter search across facts, events, knowledge. |
| `GET`  | `/memory/policy` | bearer | Read the current `MemoryPolicy` (decay, dedupe, validator config). |
| `GET`  | `/memory/dissents` | bearer | List divergent writes parked for human review. |
| `GET`  | `/healthz?contract=v1` | none | Liveness + contract-pinning probe (see [drift detection](#drift-detection)). |

## Auth model

Every endpoint except `/healthz` requires `Authorization: Bearer <token>`.
The token is a shared secret distributed out-of-band: ansible-k reads
it from a file under `/etc/ansible/klams.token` (mode 0600, owned by
the deploy user). Rotation is also out-of-band — when Ken rotates the
token, both `/etc/ansible/klams.token` and `/etc/klams/klams.toml`
are updated in the same Ansible run, then `systemctl restart
klams-service` is fired. ansible-k MUST NOT attempt to fetch, mint,
or persist tokens itself.

## Minimal valid payload examples

### `UserFact` (POST `/memory/facts`)

```json
{
  "type": "UserFact",
  "payload": {"name": "Ken", "host": "kubs0"},
  "source": "User"
}
```

Response (200):

```json
{
  "id": "019e3e02-ff6e-75d1-b6d4-c821277539dc",
  "type": "UserFact",
  "version": 1,
  "payload": {"name": "Ken", "host": "kubs0"},
  "source": "User",
  "confidence": 1.0,
  "decay_weight": 1.0,
  "use_count": 0,
  "last_used_at": null,
  "created_at": "2026-05-18T12:00:00Z",
  "updated_at": "2026-05-18T12:00:00Z",
  "path": "canonical"
}
```

Fields beyond `path` are the existing `Fact` entity — flattened into
the top level so pre-sprint-003 clients ignoring unknown fields keep
working. `path` is the only field added by this sprint.

### `EnvFact` (POST `/memory/facts`)

```json
{
  "type": "EnvFact",
  "payload": {
    "host": "kubs0",
    "kernel": "6.8.0-40-generic",
    "distro": "ubuntu-24.04"
  },
  "source": "User"
}
```

### `Event(category=Service)` (POST `/memory/events`)

```json
{
  "category": "Service",
  "payload": {
    "service": "klams-service.service",
    "host": "kubs0",
    "state": "active",
    "version": "0.1.0"
  },
  "source": "User"
}
```

Response (200):

```json
{
  "id": "019e3e02-...",
  "category": "Service",
  "path": "canonical"
}
```

## Dedupe semantics

Rerunning a play that emits identical facts produces zero new
versions; only `last_used_at` advances. Concretely:

- klams hashes the canonical-JSON of `(type, payload)` into
  `content_hash`. A second POST with the same hash is a no-op write
  whose response still returns the existing fact (with bumped
  `last_used_at`).
- `Event` rows are append-only and **not** deduped — every POST adds
  a new row. Callers SHOULD batch event emission per play, not per
  task, to avoid log noise. The monitor in this sprint already
  collapses Service-state events to "edge" transitions, so re-posting
  a steady-state event is cheap.
- `KnowledgeChunk` rows are deduped on `sha256(normalize(text))` —
  the scanner relies on this to make `--once` runs idempotent.

## Failure modes

| Status | Meaning | Caller action |
|--------|---------|---------------|
| `200`  | canonical write | continue |
| `202`  | accepted but diverted to dissents (only `AgentProposal` source) | log, do not retry |
| `409`  | optimistic-concurrency mismatch | refetch the fact, set `expected_version`, retry once |
| `422`  | validation error (bad payload, missing required field) | fix payload, do not retry |
| `5xx`  | transient (DB hiccup, Qdrant restart) | retry with jittered exponential backoff, max 3 attempts |

The `202` row only fires when `source: "AgentProposal"` is used and
the write contradicts a higher-trust row. ansible-k posts as
`source: "User"`, so `202` should never be observed in practice; if
it is, treat it as a contract bug and file an issue.

## Integration shape recommendation

**Preferred**: an Ansible callback plugin (`ansible-k` already has a
plugin loader). Pros: fires on every task, before/after hooks
available, no extra play boilerplate. Cons: failures swallowed by
default — wrap every klams call in `try/except` and emit a warning
to `display.warning()` on failure.

**Acceptable for ad-hoc plays**: a `post_tasks` block at the end of
the play that loops over `ansible_facts` and posts. Pros: simple,
visible in play output. Cons: skipped on early-failure plays, easy
to forget when authoring new plays.

Whichever shape you pick, the write path is **not** in the critical
path of any production action: a klams outage MUST NOT fail an
otherwise-successful play. Surface the error, then move on.

## Drift detection

The pinned-version header in [README.md](README.md) references
`GET /healthz?contract=v1`. The handshake:

- klams responds `200 {"contract":"v1", ...}` while it is on this
  sprint's API surface.
- Any other response (different `contract` value, 404, 5xx, missing
  field) means the API has moved and this contract no longer
  applies. ansible-k SHOULD probe `/healthz?contract=v1` once per
  play startup, log the result, and degrade to "fact-emission
  disabled" mode if the probe fails. Re-enable on the next play if
  the probe recovers.
