# Installing klams

This is the one guide from `git clone` to your first successful
recall. It assumes you have never read a sprint doc and never will.
(The operational history of the reference deployment lives in
[setup.md](setup.md); you don't need it to install.)

What you end up with: the **klams-service** HTTP + MCP API on
`127.0.0.1:7777`, backed by Postgres, Qdrant, and a text-embeddings
server in Docker; an operator bearer token; an agent wired to the MCP
surface; and a **scanner** keeping a knowledge corpus in sync with
your files.

## 1. Prerequisites

| Requirement | Why | Negotiable? |
|---|---|---|
| Linux, x86_64 | the monitor shells out to `systemctl`; the systemd deploy path and container images assume it | the *service* has no Linux-only code, but this is the supported ground |
| Docker + Compose v2 | Postgres, Qdrant, and the embedder run as containers | no |
| Rust (via [rustup](https://rustup.rs)) | klams builds where it runs; the exact toolchain (currently 1.96.0) is pinned in `rust-toolchain.toml` and rustup picks it up automatically | no |
| [`just`](https://github.com/casey/just) (`cargo install just`) | every routine task is a recipe | you could type the commands by hand, but don't |
| ~20 GB disk | Postgres + Qdrant + model cache + a Rust target dir | more later, as corpus grows |
| 16 GB RAM | four containers plus a Rust build | 8 GB works, slowly |
| systemd | the long-running deploy mode | `just run` in a terminal is fine to start |

The desktop viewport is optional and has its own toolchain (pnpm +
Node 20 + Tauri); skip it on day one.

## 2. Pick your embedding backend (the one real decision)

klams needs an embedding endpoint. Three supported options — anything
else is unsupported, full stop:

**GPU (NVIDIA/CUDA)** — the default the config ships with:
Qwen3-Embedding-0.6B plus a cross-encoder reranker, together ~2.9 GB
VRAM (measured). Any card with 4 GB works; 6 GB is roomy. You'll set
`TEI_IMAGE_TAG` to the CUDA tag matching your card's compute
capability (e.g. Ada/8.9 → `89-1.9`; see the comments in
`deploy/compose.env.example`) and include the GPU compose override.

**CPU** — no GPU required. `TEI_IMAGE_TAG=cpu-1.7` is exactly what
this repo's CI runs, with the smaller `BAAI/bge-small-en-v1.5` model.
Embedding throughput drops hard — that matters for a bulk re-scan,
not for interactive search. The §4 checklist below has the exact
config edits.

**Any OpenAI-compatible embeddings endpoint** — the AMD / Apple
Silicon / no-GPU-at-all answer. vLLM, Ollama (`/v1`), LM Studio, or a
hosted API: set `[embeddings] api = "openai"` and point `url` at the
endpoint (the `/v1` segment included). One honest caveat: klams asks
TEI's `/tokenize` for exact token counts to gate oversized writes;
off the TEI path it falls back to a character estimate, so the ingest
ceiling gets fuzzier. Handled gracefully, worth knowing.

**The reranker is optional on every path.** It's a second-stage
reorder, best-effort at runtime — a dead reranker costs the reorder,
never the search. To skip it (e.g. on CPU), delete the
`reranker_url` line from your `klams.toml`.

## 3. Provision

```sh
git clone https://github.com/kenhia/klams.git
cd klams

# Choose where config + data live. /ai/klams is the default;
# any writable path works.
KLAMS_ROOT=/ai/klams ./scripts/provision-storage-root.sh
```

The script creates the storage root and renders four files under
`$KLAMS_ROOT/config/`, generating secrets as it goes:

- `klams.toml` — service config, with three `[[auth.tokens]]` grants:
  **operator** (read+write+manage — yours; the script prints it once),
  **scanner**, and **monitor**.
- `compose.env` — image tags + the generated Postgres password.
- `scanner.toml` / `monitor.toml` — daemon configs with url + token
  already filled. The scanner's `roots` is a placeholder you'll edit
  in §7.

It's idempotent: existing config is never overwritten. Save the
printed operator token; it's also in `klams.toml` (mode 600).

If `$KLAMS_ROOT` is not `/ai/klams`, export
`KLAMS_CONFIG=$KLAMS_ROOT/config/klams.toml` in your shell (and see
the note in §6 for systemd) — otherwise the service and the justfile
find the config on their own (resolution order: `KLAMS_CONFIG`, then
`/ai/klams/config/klams.toml`, then `~/.config/klams/klams.toml`).

## 4. Configure the embedding backend you picked

**GPU**: edit `$KLAMS_ROOT/config/compose.env` and set `TEI_IMAGE_TAG`
to your card's CUDA tag. That's it — the model defaults match.

**CPU checklist** (every line matters — the service refuses to start
on a vector-width mismatch, which beats silently querying garbage):

In `compose.env`:

```sh
TEI_IMAGE_TAG=cpu-1.7
TEI_MODEL_ID=BAAI/bge-small-en-v1.5
```

In `klams.toml`, under `[embeddings]`:

```toml
model_id = "BAAI/bge-small-en-v1.5"
vector_dim = 384
max_input_tokens = 512
query_prefix = ""     # bge-small is symmetric; Qwen3's instruct prefix would hurt it
```

…and under `[retrieval]`, delete (or comment out) `reranker_url` to
skip the reranker, or leave it if you're willing to run the reranker
container on CPU too. If you change `max_input_tokens`, keep the
scanner's `max_input_tokens` (in `scanner.toml`) in step — same
number, both files.

**OpenAI-compatible**: in `klams.toml` under `[embeddings]`, set
`api = "openai"`, point `url` at the endpoint including `/v1`, set
`model_id`/`vector_dim` to your model's values, and set `api_key` if
the endpoint requires one. Skip the `tei` container entirely.

## 5. Bring up the stack and the service

```sh
cd deploy
# CPU / OpenAI-compat:
docker compose --env-file $KLAMS_ROOT/config/compose.env up -d
# GPU — include the CUDA override:
docker compose --env-file $KLAMS_ROOT/config/compose.env \
  -f docker-compose.yml -f docker-compose.gpu.yml up -d
cd ..
```

First run pulls images and downloads the embedding model — give TEI a
minute; `docker compose ps` shows health.

Build and run klams-service (migrations run automatically at start):

```sh
cargo build --release -p klams-service
just run          # foreground; logs to stderr — fine for a first install
```

Confirm liveness from another shell:

```sh
curl -fsS http://127.0.0.1:7777/healthz
```

## 6. Or run it under systemd (when you're ready to keep it)

```sh
just install-systemd
```

This builds all three binaries, creates a `klams` system user,
installs binaries to `/usr/local/bin` and units to
`/etc/systemd/system`, and expects configs at `/etc/klams/`
(`klams.toml`, `scanner.toml`, `monitor.toml` — copy your rendered
files there, or edit the units' `Environment=KLAMS_CONFIG=` lines to
point at `$KLAMS_ROOT/config/`). Token edits take effect with
`sudo systemctl reload klams-service` — no restart.

## 7. Point the scanner at your files

Edit `$KLAMS_ROOT/config/scanner.toml` (or `/etc/klams/scanner.toml`
under systemd) and set `roots` to the **absolute** paths you want
indexed — your code, your notes:

```toml
roots = ["/home/you/src", "/home/you/notes"]
```

The placeholder path deliberately doesn't exist, and the scanner
refuses to start until it points at real directories — no silent
scanning-of-nothing. Respectful of `.gitignore`; add a `.klamsignore`
for anything else. Then either let the systemd timer drive it hourly,
or run one pass now:

```sh
KLAMS_CONFIG=$KLAMS_ROOT/config/scanner.toml \
  cargo run --release --bin klams-scanner -- --once
```

## 8. Connect an agent

klams without an agent is a REST API and an empty database. Hand your
agent [klams-mcp-for-agents.md](klams-mcp-for-agents.md) — it has the
`claude mcp add` / Copilot MCP config incantations, and, more
importantly, the **routing-policy blurb** to paste into the agent's
instructions file. Don't skip the blurb: an agent that is merely
*offered* a memory tool mostly won't use it; the same tool phrased as
a routing rule ("recall-shaped question → `memory_search` FIRST")
gets used.

Mint each agent its own token: add a `[[auth.tokens]]` grant to
`klams.toml` (scopes `["read", "write"]`, a distinct `agent_name` —
see [auth.md](auth.md)), then `sudo systemctl reload klams-service`
(or restart `just run`).

### The networking truth

klams speaks **plain HTTP only** — there is no TLS in the service.
Everything on one host, over loopback: nothing to do, you're done.
The moment an agent runs on a *different* machine than the service —
which is the normal case eventually — reachability and TLS are yours
to provide: Tailscale Serve, Caddy, or an SSH tunnel all work. The
reference deployment uses Tailscale Serve:

```sh
# On the klams host, joined to a tailnet:
tailscale serve --bg --https 7777 http://127.0.0.1:7777
# Agents then use https://<host>.<tailnet>.ts.net:7777/mcp
```

Do not bind the service itself to a public interface; put a
TLS-terminating proxy in front and keep bearer tokens off the wire in
the clear.

## 9. Prove it — the first-run smoke

```sh
KLAMS_TOKEN=<your operator token> just smoke
```

This drives the whole loop against your running service — health,
a fact write found again by search, a knowledge write embedded and
retrieved, error handling, metrics — and tells you either **"your
install works"** or which step failed and what to check. It creates
its own test data, so it's valid on a completely empty store.

## Week one: an empty klams is unimpressive, by design

Everything interesting about klams — provenance tiers, dedupe,
reranking, cross-agent recall — emerges from an accumulated corpus.
On day one you have zero facts and zero knowledge. That's expected:
point the scanner at your real code and notes, wire the routing
policy into the agents you actually work with, and work normally for
a few days. Recall value shows up with corpus, not with installation.

## Upgrading

Run `main`. The version's PATCH segment is a sprint number, not a
compatibility promise; migrations are forward-only and run at start.
Occasionally a sprint changes the corpus shape (a new embedding model
or collection) — the upgrade path for those is re-running the scanner,
which is cheap. See "Support & posture" in the [README](../README.md).
