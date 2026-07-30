# Sharing design — knowledge leaving the homelab

**Status:** THOUGHT STAGE — design record, no build commitment.
See [README.md](README.md) for what that means.
**Date:** 2026-07-30 (sprint 038, korg:772, covering WIs #760 + #768)
**Companion docs:** [prior-art.md](prior-art.md) (research, sources) ·
[memory-feed-draft.md](memory-feed-draft.md) (draft interchange
standard) · [pitch.html](pitch.html) (the infographic)

---

## 0. The short version

The two brainstorms — a **community shared store** (#760) and
**curated exfil digests** (#768) — are the same idea approached from
two ends, and they collapse cleanly: **the digest is the primitive;
the shared store is a view over subscribed digests.**

Concretely: each participant *publishes* a small, curated, sanitized,
signed feed of their best memories as static files. Everyone else
*subscribes* — their own klams ingests the feeds they trust into a
distinct community stratum of their local store. Nobody ever writes
into anyone else's instance. The "shared store in the cloud" that #760
imagined is, in v1, nothing but object storage holding feeds — and if
a hosted community search service is ever wanted, it is *itself just a
klams instance subscribing to the same feeds*, handing out read-only
tokens.

This shape was chosen because it is the only one that doesn't fight
two standing klams decisions: **no tenancy model** (generalize-klams
§4.2: flat scopes, whole-corpus read, single-operator by design) and
**the architecture stays purpose-built** (AGENTS.md). Sharing by
publication needs neither. It also dissolves the hard problems of the
naive designs: multi-writer auth (nobody multi-writes), loop
prevention (provenance in the record format), moderation (drop a feed
from your list), availability coupling (shared content lives locally
once ingested; query time never touches the network).

Verdict on the standard: see §7 and [prior-art.md](prior-art.md) — and
the honest go/no-go with trigger conditions is §10.

---

## 1. The two brainstorms, and why they are one design space

**#760 — Community shared klams store.** A second store in the cloud,
"shared by intent" — not public, but a community could read and
contribute. Agent-skills scan handwritten memories, sanitize, assign
value, share the high-value ones. Query-time: two searches (local +
shared), merged with a "keep X% from local" knob.

**#768 — Curated exfil digests.** On a schedule, an agent reviews
memories, selects the "deep gotcha / took a lot of research" records,
sanitizes them for publication, and shares them with other klams
users. Needs loop prevention (inbound shared memories must not
re-export) and a portable format — "markdown + a bit of metadata" —
usable by *other agentic memory systems*, not just klams.

Both require exactly three capabilities:

| Capability | #760 needs it | #768 needs it |
|---|---|---|
| **Selection** — which memories are worth sharing | value scoring before contribution | the scheduled curation pass |
| **Sanitization** — safe to leave the homelab | before writing to the shared store | before publication |
| **A portable record format** | the shared store's write shape | the digest itself |

The only genuine difference is **topology**: #760 imagined a live hub;
#768 imagined published artifacts. That is a transport decision, not
two designs — and §3 argues the published-artifact transport wins on
every axis that matters at this scale.

### 1.1 What this is *not*: a general knowledge store

The Internet already exists; a shared memory pool that tries to be
"general knowledge" is a strictly worse Internet. The value
proposition is narrower and stronger: **a community whose interests
overlap** — the hard problems Alice has solved are likely related to
the hard problems Bob will encounter. Three properties follow:

- **High prior overlap.** Subscribers self-select by stack (homelab
  operators, klams operators, whatever the feed list gathers), so a
  shared gotcha has unusually high odds of being *your* future gotcha.
- **Selection inverts web-searchability.** The #768 criterion — "deep
  gotcha, took a lot of research" — selects precisely the knowledge
  that open-web search *failed* to surface cheaply the first time.
  A record that a quick web search would have answered doesn't merit
  export; the feed is, by construction, the residue the Internet is
  bad at.
- **Provenance you can weigh.** A community entry arrives attributed
  and signed by a specific operator you chose to subscribe to — not
  anonymous SEO text. That is what makes it safe(ish) to let into an
  agent's context at all (§9).

This also bounds ambition honestly: the design optimizes for dozens
of overlapping-interest peers trading distilled experience, not for
scale, discovery, or being anyone's product.

## 2. Constraints inherited from klams

These are not up for renegotiation in this design; they shaped it.

1. **No tenancy model, and there shouldn't be one**
   (generalize-klams §4.2). Scopes are flat; `read` sees the whole
   corpus. A multi-writer community hub would need per-author
   isolation, quotas, and moderation tooling — a tenancy model by
   another name, disqualified twice over (privacy analysis + AGENTS.md
   purpose-built rule).
2. **Intelligence lives outside the Rust service** (WI-259 division of
   labor). Value-scoring, sanitization judgment, and digest curation
   are LLM work → they belong to an agent skill / klams-mind, not to
   klams-service. The service ships primitives at most.
3. **YAGNI, evidence-gated.** #768 says it plainly: there is no
   current web of users. The design must be honest about which parts
   are worth building at N=1, and name the triggers for the rest.
4. **The Operator track is the on-ramp** (sprint 035; generalize-klams
   §4.1). An installable klams is the precondition for "would anyone
   share into it" — and the first real operator (ksandbox-style
   install) is the first potential sharing peer.
5. **The retrieval pipeline is ONE function** (sprint 036/037):
   `klams_core::retrieval::search`, five rank lists into weighted RRF,
   provenance tiers as fusion weights. Anything shared must enter
   retrieval *through that pipeline*, not beside it.

## 3. Topology: four shapes considered

**(A) Multi-writer cloud hub** — the literal reading of #760: a klams
instance in the cloud everyone holds a write token to. Rejected:
requires the tenancy model klams deliberately lacks (per-author
isolation, quotas, abuse handling); a poisoned or sloppy write lands
in *everyone's* retrieval instantly; the hub is a single point of
failure standing in the query path; and running it is an unbounded
moderation commitment (the generalize-klams §4.4 support-burden
analysis, but worse — it's other people's *data*, not their bugs).

**(B) Live federation** — no hub; each member's klams queries peers at
search time. Rejected for v1: N×M auth and availability coupling
(your recall latency now includes someone's residential uplink),
and it puts remote content into your context *without* a local
curation checkpoint — the worst posture against memory-poisoning
(§9).

**(C) Signed static feeds + local ingestion — CHOSEN.** Publication,
not access. Each participant exports a curated digest feed to static
hosting; subscribers' own instances pull, verify, and ingest into a
local community stratum. The community itself is just a **feed list**
(a git repo of feed URLs + keys; joining is a PR). Retrieval stays
100% local. This is the apt-repo / RSS-planet / Homebrew-tap shape —
boring, proven, and it needs *zero* new auth surface in klams.

**(D) Hosted community index — deferred, and cheap when wanted.** If
the community grows enough that "search what everyone has shared
without subscribing to 30 feeds" matters, stand up one klams instance
whose ingest job subscribes to every feed on the list, and hand
members **read-only** tokens. It is a *convenience cache over C*, not
a system of record: rebuild-from-feeds at any time, and losing it
loses nothing. This is the only shape where the #760 "two queries,
local + shared" fan-out exists literally — and it composes with C
rather than replacing it.

Why C wins: every hard problem the other shapes must *solve*, C makes
*structural*. Write auth → nobody writes anywhere but their own store.
Moderation → your feed list is your moderation policy. Revocation →
remove the feed, re-sync the stratum. Poisoning blast radius → one
subscriber set, with a local checkpoint before ingest. Availability →
feeds are static files; the query path never leaves the machine.

## 4. Query-time composition — the "keep X% from local" knob

Ken's #760 sketch (two queries, then merge with a keep-X%-local knob)
predates sprint 036. Post-036 there is a cleaner seat for it: **the
community stratum enters `klams_core::retrieval::search` as a sixth
rank list**, exactly as the curated stratum and the lexical list did.

- **Ingested community memories are ordinary knowledge points** in the
  subscriber's own Qdrant, carrying origin metadata (§6). No second
  query, no network at search time, no new merge code path.
- **A `community` provenance tier** slots into the existing weight
  ladder: hand-authored 2.0 > extracted 1.5 > **community ~1.25** >
  bulk 1.0. Rationale: a community record was hand-curated *twice*
  (exporter + your subscription choice) but wasn't written against
  *your* environment — it should outrank scanner bulk and lose to
  your own hand-authored notes. The number is a starting point; like
  every ranking change it is **eval-gated** (`just eval`
  before/after, throwaway-service pattern) before it ships.
- **The knob is a quota, not a weight.** "Keep X% from local" maps
  to a per-page cap: at most `ceil(top_k × (1 − X))` hits from the
  community stratum, applied after fusion (like the tag filter).
  A quota is predictable and explainable ("at most 3 of your 10 hits
  are community"); tuning a fusion weight to hit a percentage is
  neither. Default conservative: X = 80% local.
- **Boost-gate parity:** the community list obeys the query-relative
  relevance gate (raw cosine ≥ `boost_threshold(top_raw)`) the curated
  stratum uses — community content must never surface on tier weight
  alone.
- Results carry their origin visibly (`origin` in the projection), so
  an agent reading a community memory *knows* it's third-party (§9).

Shape D (hosted index), if it ever exists, is queried by agents as a
*separate MCP server*, not fused: the member's agent decides when to
ask the community index, the same way it decides when to search the
web. Fusing a remote store into local retrieval buys latency coupling
for no recall the local stratum doesn't already provide.

## 5. The export pipeline (curation + sanitization)

Deny-by-default: **nothing leaves unless it is selected, scrubbed, and
approved.** The pipeline is an agent skill (per WI-259 it is *not*
service code), run on a schedule (#768) or on demand:

1. **Candidate selection.** Query the curated stratum (hand-authored
   tier only — scanner chunks are derivable from repos and never
   export). Score for share-worthiness: the #768 signals ("deep
   gotcha", "took a lot of research", general-use), plus mechanical
   ones — survived supersession (end of a supersede chain), stable
   volatility, tags like `gotcha`. An explicit `share:candidate` tag
   lets Ken nominate by hand; an explicit `share:never` tag is an
   absolute veto.
2. **Generality rewrite.** The LLM pass that turns a homelab memory
   into a publishable one: strip or genericize hostnames, tailnet
   names, usernames, absolute paths; drop anything whose value doesn't
   survive that rewrite ("kubs0's backup dir moved" is not knowledge
   the world can use; "TEI cpu images before 1.8 reject
   --auto-truncate false" is).
3. **Mechanical scrub.** Secret/PII scanners over the rewritten text —
   the gitleaks/trufflehog pattern set plus a PII pass (tooling
   surveyed in [prior-art.md](prior-art.md)). Any hit fails closed.
4. **Human review.** v0 ships **nothing without Ken's explicit
   approval of each digest** — the review artifact is a PR to the
   feed's git repo, so approval, diff, and history are the same
   motion. This gate only relaxes (to spot-checking) after the
   pipeline earns trust, and "relax" is a decision, not drift.
5. **Publish.** Approved entries are rendered into the feed format
   (§7), the feed index is updated and **signed** (minisign/Sigstore —
   prior-art §4), and the static files are pushed to hosting (§8).

**Loop prevention lives here and is absolute** (#768's requirement):
step 1's query filters `origin == local`. A memory that arrived via
any feed carries foreign origin metadata (§6) and is structurally
unselectable for export — not policy, schema. Re-export of *derived*
insight (you read a community memory, learned something in your own
environment, wrote a new local memory) is fine and correctly carries
your origin, which is how attribution should work.

## 6. The import pipeline

A subscriber runs a small sync job (skill or cron; not service code):

1. Read the community **feed list** (or a personal one — subscribing
   to a single feed with no community at all is a valid mode).
2. Fetch each feed's index; verify its signature against the pinned
   key from the feed list; diff against the last-seen state.
3. For each new/updated entry: validate the format, then ingest via
   ordinary `memory_add` with a per-feed token
   (`agent_name = "feed-<publisher>"`, scopes `["read","write"]`) —
   attribution rides the existing auth model unchanged. Origin
   metadata (publisher, feed URL, origin id, content hash, license)
   lands in the payload.
4. Entries marked superseded/retracted in the feed are superseded
   locally (the existing verbs; the feed's `supersedes` chain maps to
   klams' one).
5. Removal of a feed from the list = supersede-or-delete everything
   bearing that origin. One command, rebuildable the other way at any
   time.

klams-service changes needed for all of §5+§6, v0: **approximately
none.** Tags and payload metadata cover origin marking; `memory_add` /
`memory_supersede` cover ingest and retraction. The service work only
begins if/when the community *tier and quota* (§4) ship — one
provenance-tier addition and one post-fusion cap, both eval-gated.
That smallness is the strongest sign the topology is right.

## 7. The interchange format

*(Verdict and full survey in [prior-art.md](prior-art.md); the draft
standard — written only because the research found no existing one —
is [memory-feed-draft.md](memory-feed-draft.md).)*

Design position, independent of the verdict: the wire format must be
**system-neutral by construction** — "markdown + minimal metadata"
(#768), where the mandatory metadata is only what *any* agentic memory
system already has (id, author, timestamps, text) plus what sharing
itself requires (origin, license, content hash, supersedes). Rich
klams-isms (provenance tiers, volatility) travel as optional
extensions other systems may ignore. A klams that can only trade
memories with another klams would miss the actual point of #768's
second bullet.

## 8. Hosting and costs

*(Numbers and sources in [prior-art.md](prior-art.md) §3.)*

- **v1 (shape C) — static feeds.** GitHub Pages / Cloudflare R2 +
  object storage; effectively **$0/month** at any plausible community
  size. The feed list is a git repo (also $0). This is deliberately
  the same trick the repo already plays with rendered HTML previews.
- **Shape D — hosted index, when triggered.** One small VPS running
  the standard klams stack CPU-mode (the 035 CPU path exists exactly
  for hosts like this) ≈ **$10–20/month**, or managed pieces if
  operating it ever feels like a job. Not spent until the §10 trigger
  fires.

## 9. Trust, poisoning, and the inbound gate

Shared memories are **text that will be injected into other people's
agent contexts** — that is the threat model, and it is why the design
front-loads provenance:

- **Signed feeds + allowlist**: content is only as trusted as the
  feed list, and the feed list is reviewable, versioned, and small.
- **Local checkpoint**: ingest is a discrete, loggable step (not
  query-time passthrough), so a subscriber can diff what arrived, and
  quarantine-review a new feed's first digest before ingesting.
- **Visible origin at retrieval**: community hits are labeled in the
  projection; agent routing guidance should treat them like any
  third-party document — evidence, not instruction. (The same rule
  agents already apply to scanned web content.)
- **Blast-radius asymmetry vs shape A**: a malicious entry reaches
  only subscribers, only after their sync ran, only through the
  format validator, and is revocable by dropping the feed. In a
  multi-writer hub it would reach everyone at query time immediately.
- Known attack literature on RAG/memory poisoning is collected in
  [prior-art.md](prior-art.md) §1 — the short lesson is that every
  published attack assumes an unmediated write path into the victim's
  store, which is precisely what shape C removes.

## 10. Go / no-go

**Go — the parts valuable at N=1** (a build sprint away, small):

1. **The format draft** (done in this sprint, pending community
   feedback — [memory-feed-draft.md](memory-feed-draft.md)).
2. **The exporter skill** (§5) publishing Ken's own digest feed.
   Worth it with *zero* subscribers: it is a curated, publishable
   "hard-won gotchas" feed — a blog-shaped artifact with a schema —
   and it exercises selection + sanitization for real.
3. **The import skill** (§6) pointed at that same feed from a second
   instance (ksandbox is the natural subscriber), proving the loop
   end-to-end including loop prevention.

**Not yet — trigger conditions, named** (per #768's own honesty about
the missing web of users):

| Deferred piece | Build when |
|---|---|
| Community feed list repo | a **second real operator** exists (035 track produces one) *and* wants to trade digests |
| `community` provenance tier + quota knob in klams-core | first foreign feed is actually subscribed, and evals can measure the tier |
| Hosted community index (shape D) | ≥ ~5 active feeds, or a member who can't run local ingest |
| Anything multi-writer | never, on current architecture principles |

**No-go, permanently, on this design:** guest access to Ken's
instance (§4.2 stands); auto-publish without the human gate;
service-side LLM sanitization (WI-259).

## 11. Open questions

- **License of shared memories.** CC-BY? CC0? Must be stated per feed
  (the draft format makes it mandatory); the *choice* is Ken's and
  community-forming, not technical.
- **Facts and events.** v0 shares knowledge (prose) only. Facts are
  key/value against a private environment — most fail the generality
  rewrite by definition. Revisit if a real use appears.
- **Identity.** v0 pins a signing key per feed via the feed list —
  no DID/PKI ceremony. Enough at community scale; the draft standard
  leaves room to upgrade.
- **Does the community stratum help retrieval?** Genuinely unknown
  until a foreign corpus exists — which is why the tier/quota work is
  triggered, not scheduled, and eval-gated when it comes.
