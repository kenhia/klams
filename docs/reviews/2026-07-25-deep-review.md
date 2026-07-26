# klams deep review — 2026-07-25

**Reviewer:** Claude (Fable 5), full-source session on kubs0, with live store access.  
**Scope:** memory lifecycle, retrieval quality, maintainability, plus auth/ops expansions.  
**Inputs:** the full workspace at `0.1.24` (`68dbd0d`), all of `docs/`, korg WIs #333/#334/#335/#406/#420/#628/#629/#631/#632/#633/#636, handoff `korg:635`, live queries against the production Postgres/Qdrant/TEI on kubs0, and live MCP `memory_search` calls.  
**Status legend:** ✅ confirmed (code path cited or measured live) · ❓ suspected (reasoned, not fully verified).  

---

## Verdict — the three things to fix first

1. **Kill the duplicate/junk supply at the source, then reset the scanner corpus.** Measured live: **44% of the 221,982-point corpus is duplicate content** (124,034 unique hashes; 107,743 points sit in cross-host duplicate groups because kai and kubs0 both scan a synced `~/src`), and the markdown chunker is fence-unaware, so every ```` ```bash ```` block containing `#` comments sheds content-free fragments that embed *better* than real content (measured `raw_score` 0.956 for a chunk that is literally a heading plus ` ```bash `). Ten search slots currently carry ~5 distinct items, ~2 of them junk. Crucially, **the scanner corpus is derived data** — after the chunker and dedupe fixes, wiping `knowledge_items` and re-scanning is a cheap deploy step, not a data-loss event. The ~62 hand-authored knowledge points and 27 facts are trivially preserved. This is the answer to "I'm open to starting data from scratch": yes for scanner rows, automatically; no need for anything more dramatic.

2. **Close the authorization hole as measured in `korg:635`, using the scope machinery that already exists.** `memory_delete` has no ownership check (✅ `crates/klams-mcp/src/tools/memory_delete.rs:46-60` — nil-check plus author-exists-check only), and the REST surface is worse: **only one REST route enforces scopes at all** (✅ `crates/klams-api/src/router.rs:124-129`) — the read-only `viewport` token can call `POST /memory/knowledge/delete` and bulk-erase any file's chunks, or promote dissents. The good news: a three-tier scope system (`Read`/`Write`/`Admin`, hot-reloadable, token-bound `author_id`) already exists and MCP already gates on it — #633 is a wiring-and-policy sprint, not an identity build.

3. **Make retrieval quality measurable, then make provenance and lifecycle count.** Post-024 fusion is pure RRF: `score = 1/(60+rank+1)`, magnitude discarded, no hook for authorship, recency, or confidence (✅ `memory_search.rs:389-416`, `hybrid.rs:239-255`). Hand-authored knowledge is ~0.03% of the corpus competing unweighted against bulk scanner chunks, and the sprint-021 miss log has recorded **one** miss in two weeks because its threshold can't fire (see F-2.6). klams-mind's eval harness exists (4 golden queries, 4/4 baseline) but measures the wrong thing — it passes while the #628 failures happen. Expand the eval first; every ranking change after that is a measured change.

---

# Part 1 — Memory lifecycle

## The structural insight: lifecycle investment is inverted

klams has sophisticated lifecycle machinery — for the wrong kind. ✅ Measured live:

| Kind | Rows (live) | Lifecycle machinery |
|---|---|---|
| `fact` | **27** | versioned amendments, source trust ranking, dissent queue, confidence, decay weights, use-count boosts (`postgres.rs:375-522`, `decay.rs`) |
| `event` | **27** | append-only, immutable by design |
| `knowledge` (scanner) | **~221,900** | implicit: delete-before-reindex + prune — the file is the source of truth (`scanner/lib.rs:96-159`) |
| `knowledge` (agent-written) | **~62** | **nothing.** No update, no supersede, no TTL, no decay — only a delete that was unusable until the authz hole made it too usable |

The agent-written knowledge rows are exactly the ones Ken's scenario is about ("not on Tailscale yet" → false 30 minutes later), and they are the only class with *no* lifecycle story at all. Meanwhile the dissent system — the one built-in correction mechanism — has been used **once ever** (1 row, status `discarded`), because it only applies to facts, and agents don't write facts (27 rows in 2 months).

**What this means:** don't design a grand unified lifecycle. Give agent-written knowledge the three verbs it's missing, and leave the scanner's re-scan model alone (it's correct for derived data).

## F-1.1 Supersession should be the primary verb, not delete ✅ recommended

Most real corrections are "this is now wrong, here is the replacement." Proposal:

- **`memory_supersede(old_id, new_text, ...)`** — atomically: write the new memory, mark the old one superseded with `superseded_by: <new_id>` (and `supersedes: <old_id>` on the new). Implementation is cheap: the soft-delete pattern already exists end-to-end (`deleted_at` payload field + `is_empty("deleted_at")` filter in `qdrant.rs:219`); supersession is the same pattern with a pointer.
- **Retrieval:** hide superseded rows by default (same filter mechanism), but keep them restorable and inspectable (extend `memory_admin_list_deleted`'s model). Do **not** rank-demote-but-show; a superseded memory surfacing at rank 7 still gets believed. "Previously believed" surfacing can come later via an explicit flag if wanted.
- **Authorization:** same capability as delete (it *is* a delete plus a write). This keeps #633's model to one decision.
- Also add **`memory_update(id, text?, tags?)`** (re-embed on text change) for the author-fixing-own-typo case where supersession is ceremony. "Manage" in #633 should cover update, supersede, delete.

## F-1.2 Staleness vs wrongness vs superseded — what the system owes each ✅ analysis

- **(a) still accurate** — needs nothing except not being buried (Part 2's problem).
- **(b) accurate-when-written, now false** — the rpidash3 class. Cannot be detected from content; can only be *corrected when noticed* (→ supersede, F-1.1) or *pre-declared as volatile* (→ F-1.4). The system's obligation is to make the correction take 10 seconds, not a session.
- **(c) wrong when written** — same remedy as (b) (supersede/delete), plus the write-time contradiction nudge (F-1.3) to catch it at the moment the correcting agent writes the truth.
- **(d) true but replaced by something more complete** — supersede with the richer record; the link preserves the trail.

The distinction matters mostly for what retrieval does afterward: (b)/(c) should disappear from results; (d) could legitimately remain findable via the link. The first version does not need to distinguish them — hide all superseded rows — and that's fine.

## F-1.3 Contradiction detection: do the cheap version, in the write path ✅ recommended

The cheap version is nearly free and belongs in klams: on `memory_add` (knowledge kind), the embedding is already computed; run the same ANN search against **agent-authored** points, and if any scores above a high threshold (~0.85+), return them in the response as `similar_existing: [{id, text_head, author, score}]` — non-blocking, purely informational. The writing agent is the one entity with context to judge "duplicate / contradiction / distinct," and it's holding the response in-hand. This turns accidental near-duplicates into supersede calls at the only moment that's cheap.

The expensive version (periodic reconciliation over the whole store) is **klams-mind's job by standing decision** (WI-259 division of labor; korg #271 contradiction detection, #272 consolidation, both in klams-mind's queue). klams owes it the primitives: supersede (F-1.1), `author.id` on read paths, and the similar-on-write signal. Don't build a second brain inside klams.

## F-1.4 Decay: keep it off knowledge for now; add declared volatility instead ✅ recommended

Facts already decay (`decay_weight = 1/(1+λ·age)`, per-type λ — `decay.rs:80-84`). For knowledge:

- **Scanner rows: recency/decay is currently meaningless.** ✅ `created_at` is scan/ingest time, not knowledge age — measured live: identical content carries `created_at` 2026-07-11 on the kubs0 copy and 2026-07-13 on the kai copy (kai's first full scan). Decay over these timestamps would express scan history, not truth.
- **Agent rows: the "IP address ages fast / 'Ken was an 11C' never ages" distinction is real but not learnable from content at this corpus size.** Make it *declarable*: an optional write-time field (`volatility: "stable" | "volatile"` or an explicit `review_after`/`expires_at`). Volatile memories get an age-based rank demotion (Part 2) and are candidates for a periodic "stale volatile memories" report; stable ones never decay. Default: no decay — wrong-but-confident demotion of stable knowledge is worse than no decay.
- Half-life per `fact_type`/tag is plausible later, but with ~62 agent rows, per-record declaration costs nothing and is unambiguous.

## F-1.5 Provenance & confidence: store the axis that already exists, expose it ✅ finding + recommendation

klams already stores a trust axis — and it points the wrong way for retrieval. `Source::trust_rank()` (✅ `klams-types/src/entities.rs:51-58`): `User=4 > Controller=3 > Task=2 > AgentProposal=1`. The scanner writes as `Task`, agents write as `AgentProposal` — so the *stored* trust model ranks bulk scanner ingest **above** curated agent writes. That ordering was designed for fact-write conflict resolution (automation beats agent proposals), and it's currently unused in knowledge ranking — but it will ambush whoever wires "trust" into retrieval naively. Recommendation: keep `Source` for fact-conflict semantics; introduce a distinct, explicit provenance class for ranking (see F-2.3) rather than overloading it. Per-memory confidence is not worth storing for knowledge today — no writer can honestly estimate it, and it would become decoration. Usefulness signals (`memory_feedback`, roadmap "Later") are the eventual empirical replacement.

## F-1.6 Garbage collection & the 10x/100x story ✅ analysis

What breaks first is **retrieval quality, and it already broke** — at 222k points the top-10 is half duplicates; storage and embedding cost are non-problems (222k × 384-dim f32 ≈ 340 MB of vectors; a full CPU re-embed is hours with the existing batch path, and the 014 re-embed runbook exists). At 10x the same ratios hold: the corpus is ~99.9% scanner-derived, so **GC = scanner hygiene, not memory aging**:

- Cross-host dedupe (F-2.2) halves the corpus outright.
- Source lifecycle (#628 problem 1): a deprecated/archived repo should be demotable or excludable without hand-editing rows. Cheapest honest version: a config/payload-level `repo_status` (active/deprecated/dead) applied by the scanner from a small config list, filterable at query time — plus actually removing roots from scan config prunes them today (the prune loop handles vanished files; removing a whole root requires care, see the multi-root prune guard `scanner/lib.rs:139-159`).
- Agent-written memories at 100x volume (~6k rows) still need no GC — they need the lifecycle verbs and the volatile-review report (F-1.4).

## F-1.7 The `event` kind and `dissent_propose` ✅ measured + recommendation

- **Events: keep.** 27 rows, but genuinely load-bearing: klams-monitor publishes service state transitions (11 of the 27), kyac writes run reports through it. Append-only audit streams are the right shape for this and the code is small. Not a home for supersession — events are observations, not beliefs.
- **`dissent_propose`: leave as-is, do not extend, revisit after supersede ships.** One dissent ever filed (discarded). It only targets facts; agents write knowledge. Its deep wiring into `upsert_fact_v2` (trust-ranked write conflicts, `postgres.rs:420-462`) makes removal invasive for zero payoff, and the viewport resolution flow exists. But its *purpose* — "this canonical record is wrong, here's a correction, pending review" — is served for the 99.9% case by supersede + trust-tier delete. Expect it to become formally vestigial; removing it then is a breather-sprint item. Do not route supersession through it: supersession must be immediate (the next agent searches in minutes, not after a human review cycle).

---

# Part 2 — Retrieval

All three of Ken's observations verified against source and live store before building on them.

## F-2.1 The score is pure reciprocal rank; magnitude is discarded ✅ confirmed

Post-sprint-024, MCP `memory_search` partitions hits into per-kind lists and RRF-fuses them (`memory_search.rs:389-416` → `hybrid.rs:239-255`): `score = Σ 1/(k + rank + 1)`, k=60. Live output shows exactly 1/61, 1/62, … 1/70. `raw_score` (cosine or `ts_rank`) is carried for eval purposes (#332) but plays **no part in ordering**. Consequences, as Ken inferred:

- A 0.96-cosine match at rank 0 and a 0.51-cosine match at rank 0 are indistinguishable in `score`.
- There is no seam in the formula for authorship, recency, or confidence — fixing ranking means changing fusion inputs or adding a weighting stage, not multiplying `score`.
- Two additional defects found while verifying: **(a)** cross-kind rank ties (fact@0 vs knowledge@0) are broken by `HashMap` iteration order — nondeterministic across identical calls (`hybrid.rs:323-336`); **(b)** within-source order for *identical vectors* (the dup pairs) is arbitrary Qdrant tie order, so which host's copy ranks first flips.

RRF itself was the right 024 call (it fixed knowledge-structurally-beats-facts). The next step is **weighted RRF**: per-hit weight `w` in `w/(k+rank+1)`, where `w` composes provenance class (F-2.3), declared volatility age (F-1.4), and dup-collapse. Small, explainable, eval-gated.

## F-2.2 Duplicates: ~44% of the corpus, ~50% of every result page ✅ confirmed + measured

- Cause confirmed in code: sprint 023 made ingest dedupe **host-scoped** — `find_knowledge_by_content_hash(hash, file, machine)` (`knowledge.rs:59-77`) — deliberately, so per-host delete works. kai and kubs0 scan a synced `~/src`, so nearly every chunk exists twice.
- Measured live (full Qdrant scroll, 221,982 points): 124,034 unique `content_hash`; 44,103 hashes have >1 point; **36,121 hashes exist on both hosts (107,743 points)**. Reproduced Ken's search shape exactly: a fresh `memory_search` for "kpidash dashboard build commands" returned 10 hits = **5 duplicate pairs**.
- **Fix in two stages.** (1) **Query-time collapse now**: group hits by `content_hash` before fusion, keep the best-ranked point, annotate `hosts: [kai, kubs0]` — small change at one seam, halves wasted slots immediately, no migration. (2) **Ingest-time cross-host dedupe later**: one point per content hash with a `machines[]` payload; requires rewriting host-scoped delete (remove host from list; delete point when empty) — do it with eval + backup in place, ideally as part of the corpus reset (Verdict #1). `content_hash` must be added to the search projection so clients and evals can see the collapse.
- **Semantics ruled (same-day feedback, Ken):** dedupe keys on **content only** — metadata differences (host/file/repo) never keep two copies apart; the storage cost of duplicates was never the issue, the top-k slots were. The surviving result carries the collapsed set (prefer a `copies: [{id, host, file}]` list over merged metadata — lossless). If a third copy exists outside the fetch window, we don't hunt for it — though note a reverse-index-by-SHA effectively already exists (`content_hash` has a Qdrant payload index; it's what `find_knowledge_by_content_hash` queries), so completing the set is one filtered lookup if ever wanted.

## F-2.3 Content-free fragments: the chunker is fence-unaware ✅ confirmed, root cause found

Sprint 022 eliminated *bare-heading* chunks, but `markdown_blocks` (`chunk.rs:194-229`) processes lines with no fenced-code-block state. Inside a ```` ```bash ```` block, a shell comment `# like this` parses as an ATX heading (`md_heading` accepts any `#<space>`), which (a) closes the current section right after the opening fence — emitting exactly the observed `"kpidash … > Dashboard build\n\n```bash"` chunk — and (b) **corrupts the heading breadcrumb** for all subsequent content (the comment text becomes an H1). The sprint-022 golden tests never include a fenced block containing `#` lines (`chunk.rs:323-431`). Live measurement: 6,125 chunks under 100 chars in the corpus.

Worse, heading-path prepending makes these fragments *strong* matches: the observed junk chunk carries `raw_score` **0.956** for the natural query, because its text is almost exactly the query's words and nothing else. Fix: track fence state (``` and ~~~) in `markdown_blocks` so fenced content is body text; add a golden test on a real README with bash comments; consider a post-chunk floor ("body below N chars after stripping the breadcrumb never ships alone").

## F-2.4 Authorship weighting: the right axis is curated-vs-bulk, and it's already recorded ✅ analysis

Measured: agent-written knowledge ≈ **62 points vs ~221,900 scanner points (0.03%)**, competing in one undifferentiated ANN pool. Ken's proposed axis (agent-written > scanner-ingested) is the right *first approximation* and is available today three ways: `source = AgentProposal` vs `Task`, author identity, and `machine == None`. His own caveat (a carefully written design doc chunk may beat an offhand agent note) is real but second-order: it argues for the weight being *mild* (a rank nudge, not a filter) — not for waiting on a perfect "usefulness" signal that doesn't exist yet. Recommendation:

- Provenance classes: `curated` (agent/human-written via memory_add) vs `bulk` (scanner). Weight curated hits up in weighted RRF (F-2.1). Magnitude tuned against the eval (F-2.7), starting gentle (e.g. curated w=1.5–2.0).
- Do **not** reuse `Source::trust_rank` — it orders these exactly backwards (F-1.5).
- The doc-chunk-vs-offhand-note refinement, if the eval demands it: a per-path prior (docs/ and README chunks above test/fixture/spec-archive chunks). Only with eval evidence.

## F-2.5 Recency ✅ analysis

- Facts: already recency-weighted within-source (decay × use-count in the FTS score, `postgres.rs:206-209`). Adequate.
- Scanner knowledge: **recency is not measurable today** (F-1.4 — `created_at` is scan time; kai's whole corpus says 2026-07-13). After steady-state (delete-before-reindex means changed files re-stamp), `created_at` approximates "content last changed," which is a *weak* freshness signal — usable as tiebreak only.
- Agent knowledge: `created_at` is honest. Apply age demotion **only to `volatile`-declared memories** (F-1.4); use recency as tiebreak between near-equal curated hits. A blanket knowledge decay would silently bury stable truths, the worst failure mode for this store.

## F-2.6 Hybrid retrieval and the dead miss log ✅ confirmed

- Knowledge is ANN-only; facts/events get Postgres FTS (`ts_rank_cd`); RRF fuses per-kind lists. `source_rank` is the hit's 0-based rank **within its own kind's list** — not a source id. The lexical gap for knowledge is real, known, and correctly gated on eval evidence (#333 / roadmap 025). Nothing new to add beyond: the eval suite must contain the identifier-heavy set *before* 025 can decide.
- **The instrument meant to measure the gap is broken.** `classify_miss` fires on `zero_hit` (ANN over 222k points essentially never returns zero) or knowledge-top `raw_score < 0.5` — but bge-small cosine on this corpus sits ~0.75–0.96 even for junk (measured: the content-free fragment scored 0.956). Result: **one miss logged in two weeks** (live `search_miss` count = 1, a `zero_hit` from an emptied filter). The threshold needs recalibration against the observed score distribution (likely ~0.78–0.82 boundary, from #628's data), or better: log a **sample of all searches** (query, top raw scores, caller) so the distribution itself is observable. Right now klams has no idea what agents ask it.

## F-2.7 How we'd know retrieval got better — the eval ✅ exists in embryo, expand it

klams-mind already has the harness this review would otherwise propose: `evals/suites/homelab-retrieval.toml` — TOML-defined queries with `substring` / `source_cited` / `no_hallucination` checks, a runner, baselines (4/4 pass, 2026-07-06). **It measures the wrong thing today**: all four queries are satisfied by scanner chunks, so it passes while every #628-class failure happens. Expansion plan (this is the gating work item for all ranking changes):

1. **Curated-beats-bulk cases**: the #628 pair verbatim (query "deferred MCP tools misdiagnosis" must surface `019f95dc-df08…`; the literal-phrasing variant must too), plus 5–10 more mined from hand-authored memories: for each, the natural *symptom* phrasing an agent would actually type.
2. **Dedupe invariant**: no two results in a page share a `content_hash` (needs `content_hash` in the projection, F-2.2).
3. **Identifier-heavy set** (feeds #333/025): error codes, config keys, hostnames, function names with known source locations.
4. **Junk ceiling**: top-5 for the golden queries contains no chunk whose body (breadcrumb stripped) is < N chars.
5. **Regression corpus from `korg:635`**: the three rpidash3 records (`019f9a83-68bc/-96f4/-c22c`) re-merged into one — retrievable by last-paragraph terms (gates #632's chunking fix).
6. Run it in klams CI against a compose stack (the docker-gated test infra exists) or at minimum as a pre-deploy gate; a baseline regression is a bug (crossroads §2.3 already says this — it just needs to be true).

Query mining beyond that: the search-sample log (F-2.6) is the honest long-term source of eval queries — harvest monthly.

## F-2.7b Oversize-write telemetry — record what agents fail to store ✅ added on same-day feedback

Agents hitting the embedder ceiling improvise ("Payload too large — splitting into focused records") and klams never records that it happened — the coherent original is simply lost, with no data on how often, by how much, or by whom. Add an **oversize-write log** on the (post-#629, honestly-classified) size-rejection path: `submitted_chars`, limit in force, agent, timestamp, and the **full submitted text** — it was content destined for the store anyway, and it is the "what did we lose" corpus. Mirror the `search_miss` table pattern; modest retention cap; Grafana by-agent panel. Stretch: match a rejection to what the agent subsequently wrote smaller (same author within ~10 min + high similarity) to answer "did the split drop the tail." After the model upgrade (F-2.8) this becomes a rare-event log — which is exactly when individual entries are worth reading, and it's the instrument that decides whether #632's server-side chunking is ever actually needed. Tracked as WI #656, in sprint proposal korg:651.

## F-2.8 Chunk-vs-record granularity and the 512-token ceiling ✅ confirmed — load-bearing for #632

Measured from the live TEI container: **`BAAI/bge-small-en-v1.5`, `max_input_length: 512` tokens, `auto_truncate: false`, 384-dim** (`curl /info` on kubs0). Consequences:

- The #632 hypothesis is confirmed: the 413 ceiling is the model's sequence limit (~2KB English prose), not a payload size. Raising limits cannot fix it; only chunking or a longer-context model can.
- **Nothing was ever silently truncated** (`auto_truncate: false`) — over-limit inputs fail loudly (413). The flip side: every scanner chunk that passed the API's 8192-char check but exceeded 512 tokens was **silently dropped at the worker** (#420 confirmed: `worker.rs:61-67` logs and discards; the scanner's cursor advanced on 202, so it never retries).
- Recommended shape for #632: server-side chunking of long `memory_add` text into overlapping windows, **N vectors → one logical memory** (search matches any chunk, returns the whole record). This preserves the agent's one-fact-one-record model and interacts correctly with #632's acceptance ("retrievable on terms from the last paragraph"). Prerequisite: a shared token-aware size gate used by *both* the API and the scanner chunker (a conservative chars-per-token bound is acceptable; note the in-tree `cl100k_base` tokenizer is the wrong vocabulary for bge — margin required).
- The embedding model itself is the floor on retrieval quality (512 tokens, 384 dims, 2023-era). **REVISED (same-day feedback):** the upgrade moves early. Ken flagged that kubs0's 4090 Super (16GB) is dedicated to klams and idle — TEI runs the **CPU** image today — so the upgrade is a config flip plus model selection, not new infrastructure. Answer to "does doing it early help the rest of the work": **yes** — it removes the 413 ceiling at the source (demoting #632's server-side chunking to optional), lets agent memories embed whole, and gives the ranking/weighting work clean data to test against. Two sequencing constraints survive: the eval suite (F-2.7) comes first, so model finalists are judged on it and the before/after is measured; and the swap rides the corpus reset (one wipe, one re-scan, one embed pass — dims change anyway, and the GPU turns the re-embed into minutes). Candidates that trivially fit 16GB: bge-m3, nomic-embed-text-v1.5, arctic-embed-l-v2.0, the Qwen3-Embedding class (verify TEI GPU support per model). Long context ≠ giant chunks: scanner chunks stay near ~800 chars for precision; the win is safety margin + whole agent memories. kai's 5090 is a recorded last resort only. Tracked as WI #655, in sprint proposal korg:652.

---

# Part 3 — Maintainability

*(Findings below combine my direct reading with two delegated full-repo sweeps; each item cites its evidence.)*

## F-3.1 Error taxonomy: the classification doesn't exist, in either direction ✅ confirmed

The known 413 case is a symptom of a structural gap: **`StoreError` has no transient/permanent axis** (`klams-store/src/lib.rs:31-45` — `Embedding(String)` is one opaque bucket for client-build failure, dim mismatch, parse failure, connection refused, and HTTP 413). Classification is destroyed at the source and unrecoverable downstream:

- **Permanent-as-transient:** the embedder retry loop retries **every** non-2xx status ×3 (`embeddings.rs:99-125` TEI, `:230-257` OpenAI-compat) and **discards the response body** — TEI's 413 body says `inputs must have less than 512 tokens`, the one actionable string. Both MCP sites that see an embedding error (`memory_add.rs:283-289`, `memory_search.rs:130-136`) then attach `EMBEDDING_UNAVAILABLE` + `retry_after_seconds: 5` unconditionally.
- **Transient-as-permanent (the mirror bug):** `INTERNAL_ERROR` is the catch-all at **32 call sites** — a Postgres pool exhaustion (retryable) and a malformed input produce the same code with no retry hint.
- The correct pattern already exists in-tree: the scanner's publish loop retries **only 503** and fails fast on everything else (`scanner/src/publish.rs:47-68`). It never made it into `embeddings.rs`.
- Dead/overloaded codes: `INVALID_KIND` is unreachable since the 018 flat schema (still documented as live in `sprints/007-mcp-server/contracts/error-codes.md`); `SCHEMA_VALIDATION_FAILED` carries both schema violations and semantic limits at 22 sites.

**Fix point:** add the transient/permanent distinction to `StoreError`, classify at the HTTP boundary (capture status + body), stop retrying 4xx, and map to error codes honestly (`PAYLOAD_TOO_LARGE {limit, submitted}` per #632's option 3). This is one sprint's worth of leverage over the whole error surface.

## F-3.2 The silent-loss triangle around knowledge ingest ✅ confirmed (#420 mechanism, fully mapped)

1. REST accepts ≤ **8192 chars** (`knowledge.rs:30`) — ~4× the model's real ~2,000-char capacity; the two limits were never reconciled.
2. The worker embeds asynchronously with **no reply channel** for knowledge: on failure it logs and drops the job — **without even incrementing `writes_failed`** (`worker.rs:60-67`; the counter is only touched in HTTP handlers). Invisible in Grafana.
3. `/healthz` stays green throughout — TEI's `/health` answers 200 whenever the model is loaded; input rejections never touch it (`embeddings.rs:145-165`, cached 2s in `health.rs:91-101`).
4. The scanner already advanced its cursor on the 202, so the chunk is never retried. (kai logged ~30k such failures in a 2h window per #420.)
5. MCP `memory_add` has **no length cap at all** (`memory_add.rs:262-283` — empty-check, then straight to TEI), while `memory_search` in the same crate does enforce `MAX_QUERY_LEN`. Deployment half: the TEI container runs with only `--model-id` and `--port` — `--auto-truncate`, `--max-client-batch-size`, `--payload-limit` are all unlooked-at defaults (`deploy/docker-compose.yml:57-72`).

## F-3.3 Two write paths, one policy — MCP bypasses the validation/trust layer ✅ confirmed

klams-api is generic over `trait Store` with a full validation pipeline; klams-mcp holds a concrete `Arc<CompositeStore>` (`tools/mod.rs:88`) and reimplements writes without it. **42 concrete `.postgres`/`.qdrant`/`.embedder` reach-throughs across 10 of 11 tool files** (post-024, exactly three retrieval calls go through the trait). Behavioral divergence, not just duplication:

| concern | REST | MCP |
|---|---|---|
| fact validation (`ValidatorRegistry`, 1,082 lines) | enforced | **absent** |
| fact write path | `upsert_fact_v2` (trust ranks, dissent divert, versioning) | **`upsert_fact` v1** — contradicting facts land canonically, no dissent |
| knowledge length / tags / normalization / dedupe probe / queue backpressure | all enforced | **all absent** — synchronous, unbounded, never deduped |

The `upsert_fact` v1 path also means the docs' claim "writes land as AgentProposal; disagree via dissent_propose rather than overwriting" describes a policy the MCP path doesn't implement. Secondary boundary defects: window-validation copy-pasted verbatim between `memories.rs` and `event_search.rs`; `fuse_in_place` round-trips through throwaway `RankedRow`s; klams-mcp depends on klams-api just for two auth extension types (dependency direction inverted — those belong in a core crate).

**Keystone fix:** make `McpState` generic over `S: Store` and route MCP writes through the same core write layer. This also unlocks F-3.5.

## F-3.4 Authorization enforcement (siblings of #633) ✅ confirmed

- **REST scopes are enforced on exactly one route** (`/v1/memories`, `router.rs:124-129`). `POST /memory/knowledge/delete`, `/memory/knowledge/index`, `/memory/facts`, `/memory/events`, and dissent `promote`/`discard` require only *any* valid bearer — the read-only `viewport` token can bulk-delete any file's chunks on every host or promote a dissent to canonical. The scope machinery (`Scope::satisfies`, `require_scope`) exists and is simply not layered on.
- **MCP scope map** (`tools/mod.rs:34-43`): `memory_delete` needs only `Write`; `register_author` needs only `Read` — a *read* scope performs DB writes and mints the identities the delete hole consumes. The `memory_admin_*` trio is correctly `Admin`-gated — a working precedent for the "trust" tier.
- **`BEARER_AUTHOR_TOOLS` omits `memory_delete`** (`tools/mod.rs:79`) — the author-defaulting fix is a membership change + making the arg optional + the ownership check.
- `/memory/knowledge/delete` with `machine` omitted deletes the path's chunks on **all** hosts (back-compat default, `knowledge.rs:112-120`) — any writer token can cross host boundaries.
- Legacy `[auth] bearer_token` materializes with all three scopes including Admin (`main.rs:358-368`) and the provisioning script renders one by default.
- Docs trap (found by the docs audit): `klams-mcp-for-agents.md:105-106` and `usage.md:601-604` state `author_id` is optional on "the write tools" including delete — **the docs already promise the #633 behavior**; only `setup.md` gets today's reality right.

## F-3.5 Testing and CI: integration coverage exists but never runs where it matters ✅ confirmed

- 477 test functions; **138 (29%) `#[ignore]`d**. All docker-gated integration jobs in CI run **only on main** (`.github/workflows/ci.yml` — four `if: github.ref == main` guards); PR branches get fmt+clippy+hermetic only. `just gate` mirrors the PR job, so local pre-commit has the same blind spot. Integration failures are discovered post-merge.
- **13 of 17 klams-mcp test files are entirely `#[ignore]`d** — structurally, because `McpState.store` is concrete and unmockable (F-3.3). klams-api, generic over `Store`, has 8 hermetic contract test files and 3 ignored tests total.
- The 413 was catchable pre-merge with zero new infrastructure: `embeddings.rs` already has 8 hermetic wiremock tests including retry-on-5xx; the ~15-line no-retry-on-4xx counterpart was never written.
- The perf regression test is `--skip`ped even on main. The sprint-022 golden chunker test exists and is good — it just lacks the fenced-code case (F-2.3).
- Retrieval quality: no labeled queries, no recall@k/MRR anywhere in klams; see F-2.7 (the harness lives in klams-mind; the klams repo's `tests/fixtures/memories/` is an empty placeholder from sprint 008).

## F-3.6 Dead code, drift, and abandoned experiments ✅ confirmed (extends #335)

Beyond the three known items (`KlamsClient::healthz`, `clear_cache_for_tests` — whose "used by integration tests" comment is impossible (`#[cfg(test)] pub(crate)`), lint-silencer fns):

- **`Embedder::embed_batch`: zero callers repo-wide** — built in 022 for a bulk re-embed that was never written; three impls + tests maintained for nothing. (Keep-or-delete decision belongs with the re-embed/model-upgrade plan; note TEI's default `--max-client-batch-size 32` would 413 it.)
- **Knowledge-digest machinery: ~150 dead lines** (`qdrant.rs:246-275, 307-340, 572-640`) — the never-wired T038 half of summarization; `summarize/mod.rs` still promises it "follows".
- **`summarize/llm.rs` (263 lines) is dead in production**: `llm_url` defaults to Ollama on 127.0.0.1:11434, which is deployed nowhere in the repo; the probe fails every cycle, disabling fallback, forever. Also `knowledge_stale_days`/`knowledge_cluster_min`: parsed, documented in the example config, never read.
- **`tools/reattribute-system` is silently broken**: `DEFAULT_COLLECTION = "klams_knowledge"` vs production `knowledge_items`; run without the env override it *creates* the wrong collection, reports zero repairs, exits 0. (The empty `klams_knowledge` collection on kubs0 — measured live, 0 points — is probably this tool's droppings.)
- **`deploy/systemd/klams.service` is a stale duplicate** of `deploy/klams-service.service` (no `ExecReload` → loses auth hot-reload; different paths) — not installed, but also not linted (`deploy_unit_files.rs` reads non-recursively).
- **Backup-path drift, the dangerous one**: the live unit hardcodes `ReadWritePaths=/gratch/klams-backup`; every doc and the example config say `/ai/klams/backups`. Under `ProtectSystem=strict` a rebuild with the documented value = every backup dies on EROFS — the exact silent failure sprint 020 spent itself fixing.
- Misc: `banner()` ×2, `ApiError::NotImplemented`/`ClientError::NotImplemented` never constructed, `ApiError::Internal.request_id` smuggles error text to clients instead of a request id, two stacked stale module-docs in klams-client, `memory_search.rs:125-129`'s comment claims trait-routing the next 200 lines don't do, `klams-test-*` compose stack left running 2 weeks on kubs0 (its qdrant unhealthy).

## F-3.7 Docs are wrong where agents and operators act on them ✅ confirmed (docs audit)

Full table in the audit; the ones that cause action:

1. **`memory_delete` `author_id` documented as optional** (agent doc + usage.md) — actively wrong today; the trap that cost the rpidash3 session (F-3.4).
2. **The decay formula is documented as exponential in all three docs; the code is hyperbolic** (`1/(1+λ·age)`, `decay.rs:79-84`) — every half-life number in `usage.md`'s table is wrong (1e-6 → 11.6 days, not 8).
3. **`architecture.md` §2j still documents the pre-024 ranking as a known limitation** — telling readers to distrust ordering that is now correct; `ScoredMemory.raw_score` and the `[retrieval]` config are undocumented; REST `/memory/search` ignores the config and hardcodes `default_rrf()` (`search.rs:104`) — a latent knob-lies-again case.
4. Copy-paste breakage: `KLAMS_AUTH_BEARER_TOKEN` (real: `KLAMS_AUTH__BEARER_TOKEN`), `task_interval = "60s"` (real: `task_interval_seconds`, integer — string fails boot), wrong systemd unit name in usage.md, `just scanner-once` recipes that hit the wrong cursor DB as the wrong user, `just wait-for-stack` doesn't exist, purge recipes predate host-scoped delete (a hand-run delete without `machine` now wipes all hosts).
5. Nonexistent observables: `klams_mcp_scope_denied_total`, `klams_mcp_calls_total{token_label}`, `klams_tei_requests_total`, `klams_decay_config_reload_total` (name off by an `s`) — any alert copied from the docs silently matches nothing.
6. Stale endpoint: `klams-mcp-for-agents.md` still says `http://kubs0:7777/mcp`; production is loopback + tailscale-serve HTTPS (`https://kubs0.encke-wahoo.ts.net:7777/mcp`) — the same stale-URL class #628 flagged.

---

# Expansions

## F-4.1 Observability: klams cannot currently answer "is retrieval working?" ✅

- **No record of what agents ask.** The miss log is the only query capture and it's dead (F-2.6): 1 row in two weeks. There is no search-sample log, so eval queries can't be mined and score distributions can't be observed.
- **Search metrics are anonymous**: `record_search("anonymous", None)` hardcoded (`memory_search.rs:292`, `event_search.rs:142`) — the per-author Grafana search panel has one series. Writes/deletes attribute correctly.
- Scope denials counted nowhere; write-path embed latency uninstrumented (read path is); worker drops uncounted (F-3.2); `/healthz` green during total write failure.
- What works: retrieval latency histograms, per-author write/delete panels, backup metrics (post-020), queue depth. The gap is *quality* observability, not liveness.

## F-4.2 Data model ✅

- **`repo` is broken as recorded**: it's the scan-root basename — measured live: 218,404 of 221,982 points say `repo: "src"`, 3,494 say `obsidian`. Any future per-repo lifecycle/filtering needs the real repo (derivable from the path's top segment under the root). The `RetrievalFilters.repo` filter and the eval's `source_cited` checks are both undermined by this today.
- **`tags` on knowledge are unused in practice** — the scanner sends `[]` always; agents rarely tag. Fine to keep (cheap), but they're not a ranking signal worth building on yet.
- **`host` earns its place** (delete scoping, provenance) but is also the duplication driver — its unit of truth should be "which hosts have this content" (a list), not "which host wrote this point" (F-2.2 stage 2).
- **`fact`/`knowledge`/`event` partition: keep.** The kinds have genuinely different write/read semantics. The real partition problem is *within* knowledge: curated vs bulk (F-2.4) matters more than fact-vs-knowledge.
- **No relationship graph exists** — `memory_related` is pure ANN nearest-neighbors (`memory_related.rs`), no stored edges. The first stored edge should be `superseded_by` (F-1.1); a fuller graph (supports/contradicts/refines) is klams-mind territory and unjustified at current volume.
- Chunk metadata (`heading_path`, `language`, `symbols`, `chunk_index`, `content_hash`) is stored in payloads but **not exposed in the search projection** — clients can't dedupe, can't strip breadcrumbs, can't see provenance class. Cheap, high-leverage projection addition.

## F-4.3 Multi-writer behavior ✅ mostly sound, two small races

- Fact upserts use `FOR UPDATE` row locks — sound. Auth hot-reload is atomic. The multi-root prune guard (`scanner/lib.rs:144-151`) correctly prevents cross-root wipes; sprint 023's host scoping prevents cross-host prune interference.
- Race 1 (minor): the REST content-hash dedupe probe is check-then-enqueue without a uniqueness constraint — two concurrent identical publishes can both miss the probe and create two points on one host. Self-heals on next delete-before-reindex; not worth a lock.
- Race 2 (benign): delete-before-reindex creates a window where an edited file's chunks are absent from search. Accepted trade already; noting for completeness.
- `register_author` under concurrency mints unlimited duplicate rows by design (no unique constraint on `agent_name`) — covered by #636.

## F-4.4 Scanner selectivity: should it ingest less? ✅ analysis

Yes, but the mechanism should be evidence-driven. The allowlist (021) already cut lockfiles/binaries. The remaining noise classes, measured or observed live: spec-archive/tasks.md scaffolding (the kpidash `tasks.md` hits), test fixtures presenting stale URLs as authoritative (#628's `kyac/tests/test_server.py`), dead repos at full rank (multae-viae), and `.obsidian`-adjacent personal notes mixing into homelab queries. Options in cost order: per-repo status config (F-1.6) → path-class demotion at ranking (tests/, specs/NNN-*/tasks.md) → per-repo `.klamsignore` seeding. The miss/sample log should drive which of these actually lands (F-2.6). What it should *not* do is grow a semantic filter — chunk-quality and dedupe fixes come first, and the corpus reset resets the baseline.

**DECIDED (same-day feedback):** the Obsidian vault comes **out** of the corpus for now — Ken's call: the vault is largely historical notes reflecting past state, not current truth, and to a recall-first agent a confident stale hit costs more than a miss. Root removed from scanner config, its 3,494 points purged, cursor cleaned. Revisit later with *targeted, validated* scanning of specific vault paths, once the search-sample log shows demand and provenance weighting exists to keep notes below curated memories. Tracked as WI #657 (standalone runbook; also folded into sprint proposal korg:652).

## F-4.5 Security posture beyond #633 ✅

Sibling findings are in F-3.4 (REST scope gap, register_author Read-scope writes, cross-host delete default, legacy full-scope token, docs trap). Additional notes: no rate limiting on any mutating route (homelab-acceptable; register_author's unbounded minting is the one that already hurt); `/metrics` deliberately public (fine on loopback+tailnet); bearer tokens in a root-readable config file (acceptable here); constant-time token comparison is correctly implemented, including no-early-exit across grants (`auth.rs:195-233`). The MCP fact path bypassing the trust/dissent policy (F-3.3) is a policy-integrity issue as much as a DRY one: the *documented* safety property ("agents can't overwrite canonical facts") isn't held on the surface agents actually use.

---

# Relationship to handoff korg:635 — corrections and confirmations

- **Confirmed** in full: the delete-path behavior table, the register_author mint-new-row behavior, the "any authenticated caller can delete the whole store" conclusion (and it's worse: the REST route needs no scope at all, F-3.x).
- **Correction (minor):** the handoff says soft delete "records `deleted_at` but not *who*". Not quite — `deleted_by_author_id` is recorded on both stores (`soft_delete_fact(id, author.id)`, `qdrant.soft_delete_payload(id, author.id, now)`; the column exists and is surfaced in `PublicMemory`). The audit gap is only that the *token* isn't recorded when it differs from the claimed author — which the #633 fix (author from token) makes moot.
- **Confirmed:** the "capability split" decision maps cleanly onto the existing `Scope` enum — `write` → `Scope::Write` + ownership check; `trust` → a new scope between Write and Admin (or Admin itself; recommend a distinct `manage` scope so Admin keeps hard-delete/restore exclusivity).
- **No decision in the handoff needs reversing.** One sequencing note: the handoff's "ownership enforcement ships before the capability tier" is right, and both are smaller than sized there because the scope/authorship plumbing already exists (F-3.x).

# Note on "start the data from scratch"

Recommended: **yes for the scanner corpus, as a routine step after the chunker/dedupe fixes — not as a data-loss event.** The corpus is 99.9% derived data, regenerable by re-scan (kubs0 corpus was already fully re-indexed once in sprint 022, 48k→73k). Preserve: the ~62 agent-authored knowledge points (identifiable by `source=AgentProposal` / absent `machine`), 27 facts, 27 events, authors table (after #636 cleanup). Everything else regenerates cleaner than it can be repaired in place. A full greenfield (new schema/store) is not justified by anything found in this review — the bones are good (WI-259 stands).

# Work items and sprint proposals filed from this review

**Existing WIs updated** (comment with verified findings on each): #633 (authz rework — resized down, fix list concretized), #636 (author lifecycle — live census added), #632 (payload limits — model questions answered), #629 (misleading 413 — broadened to the StoreError taxonomy), #628 (recall quality — all observations verified, split into specifics, stays open as the acceptance umbrella), #420 (silent drops — aggravators added), #335 (dead code — list grew, resized S→M). **#631 archived** — absorbed into #633; evidence trail preserved in its comments.

**New WIs**: #637 REST scope enforcement gap (bug S) · #638 lifecycle verbs supersede/update/similar-on-write (feature L) · #639 fence-unaware chunker (bug S) · #640 repo = scan-root basename (bug S) · #641 query-time dedupe + projection fields (feature M) · #642 cross-host ingest dedupe + corpus reset (feature L) · #643 retrieval measurement/eval expansion (task M, **gates ranking work**) · #644 weighted fusion provenance/volatility (feature M) · #645 MCP/REST write-path unification (task L) · #646 CI on PR branches (chore M) · #647 ops drift bundle (chore M) · #648 docs truth pass (chore M). **Added on same-day feedback**: #655 GPU embedder upgrade — 4090 + longer-context model + re-embed (feature M) · #656 oversize-write log (feature S) · #657 Obsidian vault out of corpus (chore S).

**Sprint proposals** (korg:649–654, sequenced):

| # | Proposal | Covers | Sequencing |
|---|---|---|---|
| 649 | 025 Authorization — ownership + real scopes | #633 #636 #637 | independent; first |
| 650 | 026 Measure retrieval + query-time dedupe | #643 #641 | before any ranking change; parallel-safe with 025 |
| 651 | 027 Ingest correctness — the 413 family + oversize log | #632 #629 #420 #656 | before the corpus reset; #632's chunking half deferred pending #656 data |
| 652 | 028 Corpus quality — chunker, repo, **GPU model upgrade**, dedupe, re-scan | #639 #640 #642 #655 #657 | after 026 + 027; one wipe/re-scan/re-embed |
| 653 | 029 Ranking + lifecycle — weighted fusion, supersede | #644 #638 #628 | after 025 + 026 |
| 654 | Breather — write-path unification, CI, ops, docs | #645 #646 #647 #648 #334 #335 | #645 after 025; rest anytime |

Roadmap note: the pre-existing queue entries (025 lexical decision → #333, 026 graph spike, 027 capability index) are **not displaced** — #333 remains gated on the eval data this plan produces (sprint 026 here feeds it directly); the graph spike and capability index sit behind this quality work, as the crossroads doc already argued.

# Store writes made during this review

No test writes were made against the store. Two knowledge memories were added deliberately at the end of the review (current-behavior gotchas for other agents: the RRF/duplicates search behavior, and the 512-token memory_add ceiling) — both will need superseding as the fixes land; they name this review so they're findable.
