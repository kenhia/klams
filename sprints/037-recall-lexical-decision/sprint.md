# Sprint 037 — Recall quality: the lexical decision

**Proposal:** korg:771 · **Covers:** #333 (L), #729 (S), #632 (L→decision) ·
**Branch:** `037-recall-lexical-decision`

The oldest open P1 (#333, "decide lexical knowledge search once 021–023
data exists") finally has its gate satisfied: the miss log exists, the
eval suite carries an identifier-heavy section built for exactly this
decision, and #729 supplies a live organic failure. This is an
**empirical decision sprint** — every outcome is legitimate, including
"the gap isn't real, decommission OpenSearch". The drift risk to guard
against is building the fun L option without the data demanding it.

## Goal

1. **#729 (S, research) — the entry point.** The standing hygiene search
   (`memory_search` for `klams gotcha`) surfaces zero curated gotchas:
   every curated candidate's raw cosine sits at 0.405–0.450, under the
   0.45 curated-stratum floor, while generic bulk chunks top out at
   0.495. Weigh the three directions in the WI — symptom-phrasing
   practice fix, tag-aware boost/filter convention, lexical signal — and
   land whichever the evidence supports. Gate: the suite's `known_open`
   case (`"klams gotcha"` → `019f95dc-df08` in top-5) promotes to `pass`.
2. **#333 (L, decision + build) — the lexical question.** Knowledge is
   ANN-only. Run the identifier-heavy eval set (suite §7 — recorded as
   measurements, not a bar; which way they fall IS the decision input),
   read the miss log, and fold in #729's live datapoint. Then either
   land the cheapest adequate lexical source behind the now-unified
   fusion (candidate order: Qdrant full-text payload index → Postgres
   FTS mirror of chunk text → BM25 on the idle OpenSearch instance), or
   conclude the gap isn't real and decommission the idle OpenSearch
   container. Trial, not swap — Qdrant stays the vector store. Decide
   before building; the OpenSearch route would grow the sprint.
3. **#632 (research → decision) — the long-payload close-out.** 027
   shipped the honest `PAYLOAD_TOO_LARGE` error, 028 raised the ceiling
   to Qwen3's 32,768 tokens, and the `oversize_write` log (#656,
   migration 0012) has been collecting since — and was empty as of the
   030 breather check. Read the log; if oversize writes remain rare,
   close the chunking half as YAGNI and document the real ceiling in the
   tool description; only if the data argues does server-side chunking
   become its own future proposal.

## Evidence base going in

- Suite §7's own finding (recorded 2026-07-26): the gap is not
  "identifiers never work" — it is "identifiers work only when they are
  already well represented in prose" (`find_knowledge_by_content_hash`
  retrieves; `LOW_SCORE_THRESHOLD` historically vanished, though 028's
  Qwen3 swap promoted it — verify current state at baseline).
- #729's live measurement (2026-07-28): the vague-query failure is
  honest embedding behavior, not a ranking bug — "gotcha" as a literal
  term in the text is what BM25 catches and ANN misses.
- 036 unified the pipeline into `klams-core::retrieval`, so a lexical
  source (if any) lands once, behind the shared weighted-RRF fusion.

## Acceptance

- Each of the three questions closes with a recorded decision backed by
  live measurements, in the WI and this doc — no "keep watching"
  leftovers.
- The `known_open` "klams gotcha" case either promotes to `pass` (fix
  landed) or its WI records why the practice-fix direction was chosen
  instead, with the suite updated to match.
- If a lexical source lands: gated on `just eval` before/after via the
  throwaway-service pattern (029/030 precedent); baseline 21/21 with
  0 regressions holds; ranking changes are deliberate and recorded.
- If the gap is ruled not real: the idle OpenSearch container is
  decommissioned and #333 closes with the evidence.
- #632: `oversize_write` log read and quoted; chunking decision
  recorded; ceiling documented in the `memory_add` tool description if
  not already adequate.
- Gate green; docs updated in-sprint (architecture.md if the pipeline
  gains a source; usage.md for any operator-visible change).

## Chronicle

(as the work happens)
