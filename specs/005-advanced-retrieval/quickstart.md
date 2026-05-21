# Quickstart: Advanced Retrieval and Summarization

End-to-end demo of sprint 005 deliverables. Assumes klams is already
provisioned per `docs/setup.md` (sprint 003) and ansible-k has pushed
EnvFacts (sprint 004).

## 1. Prerequisites

```bash
# klams service running under systemd, healthy
systemctl --user status klams-service.service   # Active: active (running)
curl -s http://kubs0.lan:7777/healthz | jq .

# Postgres + Qdrant + TEI up
docker ps --format '{{.Names}}' | grep klams
# klams-postgres
# klams-qdrant
# klams-tei

# Ollama on kubs0 (provisioned by ansible-k phi3-medium play; optional)
curl -s http://127.0.0.1:11434/api/tags | jq '.models[].name'
# "phi3:medium"
```

If Ollama is not installed, the LLM fallback turns off automatically and
the rest of the sprint still works.

## 2. Apply the migration

```bash
cd /home/ken/src/ai/klams
docker exec klams-postgres psql -U klams -d klams \
  -f /workspace/migrations/0004_summaries.sql
```

## 3. Update config

Edit `/ai/klams/config/klams.toml` (or copy from
`deploy/config/klams.example.toml`) to add the new blocks:

```toml
[retrieval]
fusion = "rrf"
rrf_k = 60
per_source_top_k = 100

[tokens]
mode = "tiktoken"

[summarization]
enabled = true
event_cluster_min = 50
knowledge_stale_days = 90
knowledge_cluster_min = 20
llm_fallback = true
ollama_url = "http://127.0.0.1:11434"
ollama_model = "phi3:medium"
task_interval_seconds = 3600

# Optional: tune per-type decay (already config-driven from sprint 002)
[decay.lambda]
UserFact = 1e-9
TaskFact = 1e-6
EnvFact  = 1e-9
```

Restart:

```bash
systemctl --user restart klams-service.service
journalctl --user -u klams-service.service -n 20 --no-pager
# look for: "decay config loaded: UserFact=1e-9 TaskFact=1e-6 EnvFact=1e-9 ..."
# and:      "summarization task scheduled: interval=3600s"
```

## 4. Hybrid retrieval smoke test

```bash
TOKEN=$(grep bearer_token /ai/klams/config/klams.toml | cut -d'"' -f2)

# A literal-match query — should rank an EnvFact first
curl -s -H "authorization: bearer $TOKEN" \
  -H 'content-type: application/json' \
  -X POST http://kubs0.lan:7777/memory/search \
  -d '{"query": "RTX 4080 SUPER", "top_k": 5}' | jq '.[] | {type: .type, score: .score}'

# A paraphrase — should still surface the relevant note via vector
curl -s -H "authorization: bearer $TOKEN" \
  -H 'content-type: application/json' \
  -X POST http://kubs0.lan:7777/memory/search \
  -d '{"query": "the homelab gaming GPU", "top_k": 5}' | jq '.'
```

## 5. Context bundle

```bash
curl -s -H "authorization: bearer $TOKEN" \
  -H 'content-type: application/json' \
  -X POST http://kubs0.lan:7777/memory/context \
  -d '{
    "query": "kubs0 GPU and CUDA toolkit",
    "token_budget": 4000,
    "filters": { "host": "kubs0" }
  }' | jq '{
    total: .total_spent,
    truncated,
    encoder: .token_encoder,
    facts: (.facts | length),
    knowledge: (.knowledge | length),
    events: (.events | length),
    sections: .sections
  }'
```

Expected: `total_spent <= 4000`, three sections with non-zero `count` for
facts (GPU EnvFact for kubs0), zero or more knowledge hits, recent
events. `token_encoder` reports `cl100k_base` when tiktoken loaded.

Tighten the budget:

```bash
# Drop budget to force truncation; verify the bundle still has an item
curl -s -H "authorization: bearer $TOKEN" \
  -H 'content-type: application/json' \
  -X POST http://kubs0.lan:7777/memory/context \
  -d '{"query": "kubs0 GPU", "token_budget": 200}' \
  | jq '{spent: .total_spent, truncated, sample: .facts[0]}'
```

## 6. Summarization observation

After a few hundred service-monitor events accumulate (or seed
synthetic ones), watch the task:

```bash
# Force a cycle by restarting (no SIGHUP this sprint)
systemctl --user restart klams-service.service
journalctl --user -u klams-service.service -f \
  | grep -E 'summariz|cluster|digest'
```

Inspect produced summaries:

```bash
docker exec klams-postgres psql -U klams -d klams -At -F'|' -c "
  select kind, host, category, day_bucket, source_count, mechanism
  from summaries order by generated_at desc limit 10;"
```

## 7. Viewport context preview

On the Windows workstation:

```powershell
cd C:\src\klams\viewport
pnpm install
pnpm tauri dev
```

In the running app: open the **Context Preview** pane, type a query,
slide the budget slider. The bundle should re-render within a frame or
two, with the per-section token counts updating live and the
raw/summarized toggle switching event-section content.

## 8. Metrics

```bash
curl -s http://127.0.0.1:7777/metrics | grep -E '^klams_(context|hybrid|summariz|decay)' | head -30
```

Expected new metric families:
- `klams_context_request_seconds_bucket{...}` — latency histogram
- `klams_hybrid_source_hits_total{source="vector|fts|metadata"}`
- `klams_summarization_runs_total{result="ok|skipped|failed"}`
- `klams_summarization_lag_seconds`
- `klams_decay_config_reload_total` (per-restart counter)

## 9. Acceptance check

- [ ] `/memory/context` returns coherent bundle under a token budget
      for a representative query (Phase 4 exit criterion).
- [ ] Hybrid ranking surfaces literal AND paraphrase matches (US2).
- [ ] Summary records appear after threshold reached (US3).
- [ ] Bad `[decay.lambda]` value refuses startup (US4).
- [ ] Viewport context-preview pane renders bundle (US5).
- [ ] `cargo fmt --check`, `cargo clippy --all-targets --all-features
      -- -D warnings`, `cargo test` all clean.
