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

## Chronicle

(Recorded as the sprint progresses.)

- Started 2026-07-26. Split the 027→028 handoff notes out of
  `.scratch/sprint-run-notes.md` into `.scratch/sprint-027-notes-for-028.md`.
