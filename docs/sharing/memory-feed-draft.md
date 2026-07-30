# Memory Feed — a minimal interchange format for agentic memory

**Version:** 0.1-draft · **Status: DRAFT FOR DISCUSSION** — written
inside the klams project (sprint 038) because the July 2026 survey
([prior-art.md](prior-art.md) §5) found no adopted cross-system
standard; written deliberately so that **no part of it requires
klams**. If an existing standard gains real multi-vendor adoption,
this draft should yield to it — that outcome is a success, not a
failure.

The key words MUST, MUST NOT, SHOULD, SHOULD NOT, and MAY are to be
interpreted as in RFC 2119.

---

## 1. Purpose and non-goals

Memory Feed is a **publication format**: a way for one agentic memory
system to publish a curated set of memory records so that a different
system — any system — can subscribe, verify, and ingest them.

It is:

- **a record format** — one memory = one markdown document with YAML
  frontmatter;
- **an archive format** — one feed = one directory (or tarball, or
  git repo) with a signed manifest;
- **nothing else.**

Non-goals, explicitly out of scope (each has a home elsewhere and is
where comparable drafts stall — see prior-art §5): API verbs and
sync protocols (MCP is the assumed access layer), identity/PKI
ceremony beyond one pinned signing key per feed, embeddings (vector
spaces don't travel across systems), retrieval semantics, and
whole-agent serialization (Letta `.af`'s turf).

Design discipline, borrowed from the formats that actually got
adopted (JSON Feed, vCard, RSS): **a tiny required core; everything
else is an optional, namespaced extension** a consumer MAY ignore.

## 2. Terminology

- **Publisher** — the operator (human + their agents) exporting
  records.
- **Feed** — a publisher's directory of entries plus a signed
  manifest, at a stable URL.
- **Entry** — one memory record.
- **Origin** — the (feed, entry id) pair identifying where an entry
  was first published. An entry's origin never changes, no matter how
  many systems it passes through.
- **Subscriber** — a system that fetches a feed and ingests entries
  into its own store.

## 3. The entry

An entry is a UTF-8 markdown file: YAML frontmatter between `---`
fences, then the memory body as markdown prose. The body is the
memory. Systems that store memories as anything other than prose MUST
render a prose representation to publish — if it can't be said in
markdown, it doesn't belong in a feed.

```markdown
---
id: 019fb1c9-7c16-7513-9ad4-f067611afbf1
created: 2026-07-14T09:30:00Z
updated: 2026-07-30T06:49:59Z
author: claude@kens-homelab
content_hash: sha256:918c6261bf8a1dd3c32d703757737720d1af7bf2...
tags: [tei, docker, gotcha]
supersedes: 019fa04a-ceac-7253-9420-ea3a39cd0ef2
x_klams:
  volatility: stable
---
TEI CPU images older than 1.8 reject `--auto-truncate false` at
startup. If you pass that flag (you should — silent truncation
corrupts embeddings), use a `cpu-1.8`-or-newer tag.
```

### 3.1 Required frontmatter

| Field | Type | Meaning |
|---|---|---|
| `id` | string | Unique within the feed, stable across updates. Any opaque string ≤ 128 chars; UUIDv7 RECOMMENDED. Global identity is the origin pair (`feed_url`, `id`) — publishers never need coordination. |
| `created` | RFC 3339 UTC | When the memory was first recorded at the origin. |
| `updated` | RFC 3339 UTC | Last modification. `updated` > `created` signals a revision; subscribers re-ingest. |
| `author` | string | Who recorded it: `agent@operator` RECOMMENDED (`claude@kens-homelab`). Attribution, not authentication — authentication is the feed signature (§4). |
| `content_hash` | string | `sha256:<hex>` of the body bytes (after frontmatter, exclusive of the closing fence newline handling: hash the body exactly as the file carries it, trailing newline included). Integrity check and cross-feed dedupe key. |

### 3.2 Optional frontmatter (core)

| Field | Type | Meaning |
|---|---|---|
| `title` | string | Display title. Absent ⇒ first line of body. |
| `tags` | [string] | Lowercase, publisher-defined vocabulary. |
| `supersedes` | string | `id` of an earlier entry in **this same feed** that this entry replaces. Consumers SHOULD hide or demote the superseded entry. Chains form history, PAMSPEC-style versioned identity flattened to one pointer. |
| `retracted` | bool | `true` ⇒ the publisher withdraws this entry (kept in the feed as a tombstone so subscribers *learn about* the withdrawal). Consumers SHOULD remove or suppress it. |
| `observed_at` | RFC 3339 | When the described fact was true in the world, if distinct from `created` (the PAMSPEC observed/recorded split — "the outage was Tuesday; I wrote this Friday"). |
| `confidence` | string | `high` \| `medium` \| `low`. Publisher's own assessment; consumers MAY map it onto their ranking. |
| `source` | string | How the memory came to be: `hand-authored` \| `extracted` \| `derived`. Consumers MAY weight by it. |
| `license` | string | SPDX identifier overriding the feed default (§4) for this entry. |
| `lang` | string | BCP 47; default `en`. |

### 3.3 Origin fields — required exactly when republishing

`origin_feed` (URL) and `origin_id` (string): set by a subscriber
that ingested an entry and — through explicit human action (§6) —
republishes it. They point at the *first* publication and MUST be
copied verbatim on any further hop. A publisher's own original
entries MUST NOT carry them.

### 3.4 Extensions

Any field named `x_<token>` (e.g. `x_klams`, `x_mem0`) is an
extension: a map of system-specific metadata. Consumers MUST ignore
extensions they don't understand and SHOULD preserve them on
re-export. Extensions MUST NOT change the meaning of core fields.

## 4. The feed

A feed is a directory:

```text
feed/
├── memfeed.json            # the manifest
├── memfeed.json.minisig    # detached signature over the manifest
└── entries/
    ├── 019fb1c9-….md
    └── …
```

`memfeed.json`:

```json
{
  "memory_feed_version": "https://<canonical-spec-url>/0.1",
  "title": "Ken's homelab gotchas",
  "feed_url": "https://example.com/memfeed/",
  "description": "Deep gotchas from a Rust/Postgres/Qdrant homelab.",
  "publisher": { "name": "kenhia", "url": "https://github.com/kenhia" },
  "license": "CC-BY-4.0",
  "updated": "2026-07-30T12:00:00Z",
  "entries": [
    { "id": "019fb1c9-…", "path": "entries/019fb1c9-….md",
      "sha256": "<hex of the whole file>", "updated": "2026-07-30T06:49:59Z" }
  ]
}
```

Required: `memory_feed_version`, `title`, `feed_url`, `publisher.name`,
`license` (SPDX; a feed without a license is invalid — "unlicensed
shared knowledge" is how communities end up in disputes), `updated`,
`entries[]` with `id`, `path`, `sha256`, `updated` per row.

**Signing.** The manifest MUST be signed; entries are covered
transitively by their `sha256` in the manifest (the SecureApt
pattern). v0.1 profile: **minisign**, detached signature, the feed's
public key distributed out-of-band (typically pinned in a community
feed list — §7). The minisign trusted comment SHOULD carry
`<feed_url> <updated>` to resist rollback. Keyless signing (Sigstore
cosign) and TUF are anticipated upgrade profiles, not v0.1
requirements.

**Transport is boring on purpose**: HTTPS GET of static files. A git
repository whose checkout is the feed directory is a fully conforming
transport (and gives history for free). Conditional GET /
content-addressing are quality-of-implementation details.

## 5. Subscriber requirements

A conforming subscriber, per sync:

1. Fetch `memfeed.json` + signature; **verify against the pinned
   key**. Verification failure ⇒ abort the whole feed, keep last-known
   state, surface loudly.
2. Verify each new/changed entry file against its manifest `sha256`,
   and the body against `content_hash`. Mismatch ⇒ skip that entry,
   surface it.
3. Ingest new entries; re-ingest entries whose `updated` advanced;
   apply `supersedes` / `retracted` using the subscriber's native
   lifecycle (a system with no supersession MAY delete-and-replace).
4. Record origin durably: every ingested record MUST retain
   (`feed_url`, `id`, `content_hash`, publisher name) in whatever
   metadata the local store supports. **A subscriber that cannot
   record origin MUST NOT ingest** — origin is what makes §6 and
   honest retrieval labeling possible.
5. SHOULD dedupe across feeds by `content_hash` (planet-style
   aggregation makes duplicates normal, not exceptional).

## 6. Loop prevention (normative)

The rule that keeps a community of feeds from becoming an echo
chamber, and exported stores from leaking each other's content:

> A system MUST NOT automatically export any record whose origin is
> not itself. Republishing a foreign-origin entry is permitted only
> by **explicit human action**, and MUST carry the original
> `origin_feed`/`origin_id` (§3.3) unchanged.

Corollaries: export pipelines select on `origin == local` at the
schema level, not as policy; new local records *derived from* a
foreign memory (you read it, applied it, learned something in your
own environment) are legitimately local-origin — that is attribution
working, not a loop.

## 7. Communities (informative)

A community is just a **feed list**: a versioned document (naturally
a git repo) enumerating member feeds and their pinned public keys.
Joining is adding a row (a PR); moderation is the review of that PR;
revocation is deleting the row and re-syncing. An optional
planet-style aggregator or hosted search index over the listed feeds
adds convenience without adding trust — the feed list remains the
sole trust root. Nothing in this section requires protocol support;
that is the point.

## 8. Security considerations

- **Entries are untrusted third-party text destined for agent
  contexts.** The poisoning literature (prior-art §1) applies in
  full. Subscribers SHOULD partition foreign-origin records from
  local ones, label origin visibly at retrieval, and treat entry
  content as evidence, never instruction. Consumers MAY quarantine a
  feed's first digest for human review.
- The signature authenticates the **publisher**, not the truth or
  safety of the content. Sanitization is the publisher's duty
  (publish-side gates, prior-art §2); skepticism is the subscriber's.
- Markdown bodies MUST be treated as inert data: no HTML execution,
  no link fetching during ingest.
- `retracted` is advisory; a subscriber cannot be forced to forget.
  Publishers should assume publication is permanent.

## 9. Compatibility mapping (informative)

| Memory Feed | klams | agentmemoryprotocol "AMP" | PAMSPEC (I-D) | mem0 / Letta |
|---|---|---|---|---|
| `id` | knowledge point UUID | `id` | `object_id` | memory id |
| `created`/`updated` | `created_at`/`updated_at` | `created`/`modified` | `committed_at`/`recorded_at` | created/updated |
| `author` | author (`agent_name`) + operator | `author` | provenance actor | `user_id`/agent |
| `content_hash` | `content_hash` | — | content addressing (PAM arXiv) | — |
| `supersedes` | `supersedes` chain | — (`status` only) | `version_id` succession | — |
| `retracted` | soft-delete tombstone | `status` | lifecycle state | delete |
| `observed_at` | — (klams gap, noted) | — | `observed_at` | — |
| `source` | provenance tier | `type` | generating activity | inference metadata |
| `x_*` | volatility, repo, host… | — | profile extensions | metadata |

A klams exporter/importer needs no schema change to conform: every
required field already exists, and klams-specific payload rides in
`x_klams`.

## 10. Open questions for v0.2

- Entry-level signing (needed only if entries travel detached from
  their feed).
- A recommended tag vocabulary (or deliberate silence).
- Binary/asset attachments — currently excluded on purpose.
- Whether `confidence`/`source` enums are the right shape, or
  free-text with recommended values serves adoption better.
- Where to take this for feedback once it has one real
  implementation: the W3C AI Agent Memory Interop CG is the only
  community venue that exists (prior-art §5).
