# Re-embed runbook — changing the embedding model / dimension

Changing the embedding model (or its output dimension) invalidates
every vector in the `knowledge_items` Qdrant collection: vectors from
different models are not comparable, so a mixed collection silently
degrades search. This runbook rebuilds the collection deliberately.
Facts and events carry no embeddings and are untouched.

**Time estimate:** dominated by re-embedding throughput; the current
corpus (scanner over `~/src` + `~/obsidian`) re-indexes in one
scanner cycle.

## Inventory: what holds knowledge vectors

- **Scanner-written items** (`source_file` payload set) — re-derivable
  from disk; the scanner will regenerate them.
- **Agent-written items** (MCP `memory_add`, no `source_file`) — NOT
  re-derivable; must be exported and replayed (step 3).

Count each class first:

```sh
QDRANT=http://127.0.0.1:6333
curl -s "$QDRANT/collections/knowledge_items" | jq '.result.points_count'
# agent-written (no source_file payload key):
curl -s -X POST "$QDRANT/collections/knowledge_items/points/count" \
  -H 'Content-Type: application/json' \
  -d '{"filter":{"must":[{"is_empty":{"key":"source_file"}}]},"exact":true}' \
  | jq '.result.count'
```

## Procedure

1. **Quiesce writers.** Stop the scanner timer and hold agent writes:

   ```sh
   sudo systemctl stop klams-scanner.timer klams-scanner.service
   ```

2. **Export agent-written items** (skip if the count above was 0).
   Page the full payloads out via Qdrant scroll:

   ```sh
   curl -s -X POST "$QDRANT/collections/knowledge_items/points/scroll" \
     -H 'Content-Type: application/json' \
     -d '{"filter":{"must":[{"is_empty":{"key":"source_file"}}]},
          "limit":256,"with_payload":true,"with_vector":false}' \
     > /tmp/agent-knowledge-page1.json
   # repeat with "offset": <next_page_offset> until next_page_offset is null
   ```

3. **Update config** on kubs0 (`/ai/klams/config/klams.toml`):
   new `model_id`, new `vector_dim`, and — if switching engines —
   `api` / `url`. Deploy the matching TEI model tag in
   `compose.env` if TEI stays the engine.

4. **Drop and recreate the collection.** The service bootstrap
   recreates it with the configured dim on startup:

   ```sh
   sudo systemctl stop klams-service
   curl -s -X DELETE "$QDRANT/collections/knowledge_items"
   sudo systemctl start klams-service   # bootstrap creates @ new dim
   curl -s "$QDRANT/collections/knowledge_items" \
     | jq '.result.config.params.vectors.size'   # must equal new vector_dim
   ```

5. **Reset the scanner cursor** so every file re-embeds (the cursor
   short-circuits unchanged files by mtime/hash):

   ```sh
   rm ~/.local/state/klams/scanner.sqlite
   sudo systemctl start klams-scanner.timer
   sudo systemctl start klams-scanner.service   # or: just scanner-once
   ```

6. **Replay agent-written items** from the step-2 export, one
   `POST /memory/knowledge/index` per point (text + tags + repo/file
   metadata from payload; the service re-embeds with the new model).
   Attribution note: replaying under the original author requires the
   klams-mind/companion-token era write path — until sprint 015 lands,
   replayed items are stamped with the replaying token's author and
   the original author is recorded in the export file.

7. **Verify.**
   - Point counts match the pre-migration inventory (scanner class
     converges after one full cycle; compare step-0 counts).
   - `curl -s $KLAMS/healthz` embedder probe green.
   - A known-good query returns sensible hits:
     `just verify` (SC-003 unified search) or a manual
     `POST /memory/search`.
   - `klams_embedding_latency_seconds` shows traffic; no
     `expected dim` errors in `journalctl -u klams-service`.

## Rollback

Config is the rollback lever: restore the previous `model_id` /
`vector_dim` / `api`, delete the collection again, restart, re-run
steps 5–6. The scanner-class corpus is always recoverable from disk;
the agent-class corpus is recoverable from the step-2 export — do not
skip step 2.

## Rehearsal (no production risk)

Run the same procedure against the compose **test stack**
(`tests/docker-compose.test.yml`, TEI on `:57070`, Qdrant on
`:56334`): seed with `just bench-seed`, walk steps 3–7 with
`vector_dim` unchanged but `api = "openai"` +
`url = http://127.0.0.1:57070/v1` to prove the engine-swap path, then
once more with a different-dim model tag to prove the dim-change path.
