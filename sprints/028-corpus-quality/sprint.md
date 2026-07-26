# Sprint 028 — Corpus quality: fence-aware chunker, real repo names, GPU embedder upgrade, cross-host dedupe, clean re-scan

**korg:** proposal 652 · WIs #639, #640, #642, #655, #657
**Branch:** `028-corpus-quality` · **Version:** 0.1.28
**Run mode:** Auto (Fable 5, per Ken — sole use of kubs0's 4090 Super granted;
embedder choice is carte blanche, including switching the serving stack if
warranted).

## Goal

Fix the supply of junk and duplicates at the source, upgrade the embedder onto
the idle GPU, then reset the derived corpus — one wipe, one re-scan, one embed
pass. The corpus comes back ~50% smaller, junk-free, correctly attributed, and
embedded by a modern long-context model.

## Scope

1. **#639 (S, bug)** — fence-aware markdown chunker. `markdown_blocks` treats
   `# comment` inside a ``` fence as an ATX heading, emitting content-free
   `"<breadcrumb>\n\n```bash"` chunks (0.956 raw cosine on heading-echo
   queries) and corrupting the breadcrumb stack for the rest of the file.
   Track fence state (``` and ~~~, info strings, nesting rules); golden test
   on a real README shape. 6,125 sub-100-char chunks live today.
2. **#640 (S, bug)** — record the real repo. `scan_root` stamps
   `repo = root.file_name()` so 218k/222k points say `repo="src"`. Derive
   repo = nearest ancestor with `.git` under the root, falling back to first
   path segment. Backfill rides the corpus reset (no separate Qdrant
   set_payload pass needed).
3. **#655 (M, feature)** — TEI onto the 4090 (16GB) with an eval-selected
   longer-context model; re-embed rides the reset. Ceiling moves ~512 → 8k+
   tokens. Critical 027 constraint: the exact token gate only works with
   `api = "tei"` (`POST /tokenize`); if serving moves to vLLM/OpenAI-compat,
   `count_tokens` must be implemented for that backend first.
4. **#657 (S, chore)** — Obsidian vault out: root removed from kubs0
   scanner.toml, 3,494 points purged, cursor rows cleaned, decision noted in
   docs/setup.md.
5. **#642 (L, feature)** — cross-host dedupe: ONE point per content hash with
   a `machines: []` payload, replacing one-point-per-(host,file,content).
   Host-scoped delete/prune becomes payload bookkeeping: removing a host pops
   it from `machines`, deleting the point only when the list empties. Then
   the corpus reset: backup (verify path first — #647 drift:
   `/gratch/klams-backup` live vs docs), capture 026 eval baseline, stop
   scanner timers, wipe scanner-authored points, preserve agent-authored
   records + facts + events, re-scan both hosts into the new-model
   collection.

## Acceptance

- Golden chunker tests pass; a README with `#` comments in fences produces no
  content-free chunks and correct breadcrumbs throughout.
- New chunks carry the real repo; a `repo` filter returns only that repo.
- TEI serves from the GPU; eval-chosen model live; a ~4KB memory_add embeds
  in one piece; `token_counts_predict_what_tei_accepts` passes against the
  new model; 026 eval equal-or-better than the bge-small baseline.
- Corpus point count ≈ unique-content count; a file edited on one host
  updates the shared point without disturbing the other host's `machines`
  entry; hosts removed from scanning disappear from `machines`.
- Obsidian: no `repo="obsidian"` points remain; root absent from config;
  cursor clean.
- `klams_writes_failed_total` / "Dropped queued writes" watched during
  re-scan; oversize-writes panel ~0 after the ceiling raise.

## Model selection (#655) — decided 2026-07-26

Hardware truth: the GPU is an **RTX 4080 SUPER** (16 GB), not a "4090";
Ada, compute cap 8.9 → TEI image `89-1.9`. It was idle (1 MiB used).
`nvidia-container-toolkit` 1.19.1 was installed + CDI spec generated
(recorded as k-homelab WI #683); on Docker 29 the legacy `--gpus` flag
does not work — CDI device syntax only.

Method: dumped the live corpus (221,327 points, payloads only), embedded
one test collection per candidate on the GPU (TEI `89-1.9`), booted a
throwaway 0.1.28 klams-service per collection, ran the 026 eval suite
against each. A fourth arm ran the incumbent bge-small against the live
collection — the honest same-corpus baseline, which matters because the
corpus has drifted since the 0.1.26 baseline was captured (sprint 027's
own docs/code now bury the 413-ceiling memory — #628's prediction,
verbatim; even the incumbent fails that query today).

Results (identical corpus, identical suite):

| arm | passed | newly-fixed known-open | real pass-losses |
|---|---|---|---|
| bge-small-en-v1.5 (incumbent) | 14/21 | 0 | — (1 drift) |
| BAAI/bge-m3 | 14/21 | 2 | 2 |
| snowflake-arctic-embed-l-v2.0 (`query: ` prefix) | 16/21 | 3 | 1 hard miss |
| **Qwen/Qwen3-Embedding-0.6B (instruct prefix)** | **16/21** | **4** | 2 rank-inversions |

**Winner: Qwen3-Embedding-0.6B.** It fixes 4 of the 6 known-open cases —
including *deferred MCP tools misdiagnosis*, the #628 headline — and its
two losses are rank-1/2 inversions (the target memory retrieved but
outranked by bulk), exactly the class 029's provenance weighting
addresses; arctic's loss is a hard top-5 miss. 16k input length is the
most ceiling headroom (512 → 16,384 tokens), dims 1024.

Deploy facts: TEI ≥1.8 flipped `--auto-truncate` default to **true**;
compose now passes an explicit `false`. Query asymmetry ships as
`[embeddings] query_prefix` (new key, this sprint). Full-corpus GPU
re-embed measured at ~5–7 minutes (vs a multi-hour CPU slog).

## Chronicle

- Started 2026-07-26. Split the 027→028 handoff notes out of
  `.scratch/sprint-run-notes.md` into `.scratch/sprint-027-notes-for-028.md`.
- #639/#640 landed with golden + unit tests; #642 landed with 3
  docker-gated contract tests rewritten from the old (host,file)-identity
  semantics (all green against the real stack).
- #657 executed live: root removed from kubs0 scanner.toml, 361 cursor
  rows cleared; the 3,494 points fall with the reset wipe.
- Backup path verified live (`/gratch/klams-backup`, NFS mounted,
  snapshots current) — #647's drift is docs-only.
