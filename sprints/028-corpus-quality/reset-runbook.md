# Sprint 028 — corpus reset runbook (as executed)

The 014 re-embed runbook is the template; this documents what sprint 028
actually runs, in order, with the 028-specific deltas: the model changes
(dim 384 → 1024), the chunker/repo/dedupe fixes must already be deployed,
and the Obsidian root is gone (#657).

## Preconditions (all verified before the wipe)

- [x] 0.1.28 code deployed (fence chunker #639, real repo #640,
      machines[] dedupe #642) — the re-scan must not recreate old junk.
- [x] Backup path verified live: `/gratch/klams-backup` (NFS mount up,
      `RequiresMountsFor` present, snapshots current). This was #647's
      drift — docs said otherwise; the systemd unit and
      `/etc/klams/klams.toml` agree on `/gratch/klams-backup`.
- [x] 026 eval baseline captured (`evals/baselines/homelab-retrieval.md`,
      0.1.26, 15/21) **plus** a same-day same-corpus incumbent run
      (bge-small on today's corpus: 14/21) — the honest before-number.
- [x] Obsidian: root removed from kubs0 `scanner.toml`, 361 cursor rows
      deleted. Its 3,494 points fall with the collection wipe.
- [x] Agent-authored inventory: 99 `source=AgentProposal` points (74
      with no `file`) — exported before the wipe, replayed after.
- [x] Scanner timers on BOTH hosts stopped during the wipe (kai too —
      its scanner would otherwise re-publish into the old-dim collection
      and get dim-mismatch failures).

## Order of operations

1. Fresh backup: `klams-service --run-backup-now` (or wait for the 08:01
   window), confirm today's `qdrant-*.snapshot` in `/gratch/klams-backup`.
2. Stop writers: `klams-scanner.timer` (kubs0 + kai), keep MCP up (agent
   writes are rare and the export in step 3 is re-run just before the
   wipe if any land).
3. Export agent-authored points (`source=AgentProposal`, full payloads).
4. Update `/etc/klams/klams.toml`: TEI url stays, `[embeddings]`
   model_id/vector_dim/max_input_tokens/query_prefix per the eval
   winner; `[qdrant]` collection stays `knowledge_items`.
5. Swap TEI to the GPU image + new model (compose: base + gpu override).
6. Stop `klams-service`, `DELETE /collections/knowledge_items`, start
   the service — bootstrap recreates the collection at the new dim.
7. Replay the agent export via `POST /memory/knowledge/index` (service
   re-embeds with the new model).
8. Reset scanner cursors on both hosts (delete scanner.sqlite) and
   start timers — full re-scan repopulates scanner content through the
   0.1.28 chunker with real repo names and machines[] dedupe.
9. Acceptance: `token_counts_predict_what_tei_accepts` against the new
   TEI; eval suite ≥ the same-corpus baseline; `chunk_too_large` = 0;
   "Dropped queued writes" panel quiet; corpus point count ≈
   unique-content count.
