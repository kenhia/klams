# Generalizing klams — what it would take to let someone else run it

**Date:** 2026-07-28  
**Status:** Planning note. No work items created (see §7).  
**Prompted by:** an ATV-community colleague asking to be a tester.  
**Basis:** workspace at `0.1.33` (`b3c4a17`), read directly — the
compose/deploy tree, the provision + install scripts, `justfile`, CI
workflow, config examples, and the crate sources. Claims that could be
checked against the repo were.  

---

## 0. The short version

**The code is far more portable than the README's disclaimer implies.**
The disclaimer is accurate about *intent* and about the **docs**, and
close to wrong about the **binaries**. There is no `kubs0` in a code
path anywhere — the hostname appears only in comments, test fixtures,
and one viewport default string. The data model (`FactType`,
`Source`, authors, scopes) has no Ken-shaped concepts baked in. Every
host-specific value is already a config key or an env var.

What actually blocks an outsider is, in order of severity:

1. **One real first-run bug.** `scripts/provision-storage-root.sh`
   renders a `klams.toml` with *no token in it*, so a freshly
   provisioned service refuses to start. Confirmed below (§2.1). This
   is a ~10-line fix and it is the single highest-value thing on this
   page.
2. **The docs are a chronicle, not a manual.** `README.md` routes a
   newcomer to sprint-001/002/003 quickstarts. `docs/setup.md` is 765
   lines organized by sprint number. There is no "install klams"
   document; there is a historical record of how klams came to be
   installed here.
3. **No corpus.** klams' value is retrieval over accumulated homelab
   knowledge. A tester gets an empty store and a scanner pointed at
   nothing. "It works" and "it's useful" are separated by weeks of
   ingest they have to bootstrap themselves.
4. **The unbounded support commitment**, which is the real cost and is
   discussed in §4.

GPU: your guess is right on both counts, and the requirement is much
smaller than you'd think — **~2.9 GB VRAM total** for the embedder and
reranker combined (measured, `docs/setup.md:85`). CUDA is what the
compose file and image tags are written for. But **a GPU is not
required at all**: CI runs the entire integration suite against TEI's
`cpu-1.7` image on a GitHub runner
(`tests/docker-compose.test.yml:56`), and sprint 014 added an
OpenAI-compatible embedder dialect so an Ollama/vLLM endpoint works
too. Details in §1.2.

---

## 1. Prerequisites for running klams elsewhere

### 1.1 Host

| Requirement | Why | Hard? |
|---|---|---|
| Linux, x86_64 | `klams-monitor` shells out to `systemctl` (`crates/klams-monitor/src/poll.rs:20`); `deploy/install-systemd.sh` targets systemd; container images are amd64 | Hard for the deploy path; the *service* itself has no Linux-only code |
| Docker + Compose v2 | Postgres, Qdrant, TEI ×2 all run as containers | Hard |
| systemd | The three units + timer are the supported run mode | Soft — `just run` works fine for a tester; systemd is a convenience |
| ~20 GB disk to start | Postgres + Qdrant + TEI model cache + cargo target dir | Soft |
| 16 GB RAM comfortable | Four containers plus a Rust build | Soft |

Reference point for storage: this deployment's Qdrant holds **180,553
points** (2026-07-28 census, `docs/reviews/2026-07-28-retrospective.md:5`)
after ~8 months. A tester's first month is a rounding error against
that.

### 1.2 GPU — the answer to your specific question

**Yes, a smaller NVIDIA card suffices. Comfortably.**

The two GPU consumers are both TEI containers:

- `tei` — `Qwen/Qwen3-Embedding-0.6B`, 1024-dim
- `reranker` — `BAAI/bge-reranker-v2-m3` (cross-encoder, sprint 030)

Together they occupy **~2.9 GB VRAM** on the 4080 SUPER. Anything with
6 GB is roomy; 4 GB works. The card generation matters only for
picking the image tag — TEI publishes per-compute-capability CUDA tags
(`89-1.9` here for Ada 8.9; Ampere/Turing/Hopper have their own), and
`deploy/compose.env.example:16-19` already documents the choice.

**NVIDIA is the assumption, but it is not a wall**, and there are three
escape hatches already in the codebase:

1. **CPU.** `TEI_IMAGE_TAG=cpu-1.7` with a small model. This is not
   hypothetical — it is exactly what CI does, integration suite and
   all. Embedding throughput drops hard (matters for a bulk re-scan,
   not for interactive search), and you'd want a smaller model than
   Qwen3-0.6B.
2. **Any OpenAI-compatible embeddings endpoint.** Sprint 014 shipped
   `[embeddings] api = "openai"`
   (`crates/klams-store/src/embeddings.rs:373`), so Ollama, vLLM, LM
   Studio, or a hosted API all work by changing two config lines. This
   is the AMD / Apple Silicon / no-GPU answer.
3. **The reranker is optional.** Deleting `[retrieval] reranker_url`
   turns the stage off, and it is best-effort at runtime anyway — a
   dead reranker costs the reorder, never the search
   (`crates/klams-service/tests/mcp_rerank.rs:69`).

One caveat worth writing down for a tester: the exact-token size gate
asks **TEI's `/tokenize`** for a real count. Tokenizer-less backends
(the OpenAI dialect against Ollama, say) fall back to a character
estimate — handled gracefully (`crates/klams-store/src/lib.rs:377-391`),
but it means the ingest ceiling gets fuzzier off the TEI path.

### 1.3 Build toolchain

- Rust **1.96.0** — pinned exactly in `rust-toolchain.toml` (sprint 032
  made this a real pin, not `stable`)
- `just` (`cargo install just`)
- For the viewport only: pnpm + Node 20, plus either `cargo-xwin`
  (Windows cross-build) or `libwebkit2gtk-4.1-dev` &c. (native Linux)
- For `just test-integration`: the docker test stack

### 1.4 Service versions

Pinned in `compose.env`: Postgres 16, Qdrant v1.18.0, TEI 1.9,
optionally Prometheus v2.55 + Grafana 11.2.2 behind the
`observability` profile. All pinned deliberately; a tester on
different versions is untested ground, particularly Qdrant.

---

## 2. Codebase changes needed

Tiered by whether a tester is *blocked*, *annoyed*, or *unserved*.

### 2.1 Tier 0 — actual blockers (half a sprint, mostly one bug)

**B-1. `provision-storage-root.sh` produces a config the service
refuses to start on.** This is a real, current bug, not a portability
gripe.

The script generates a bearer token and then `sed`s for the pattern
`changeme-rendered-by-provision-script` in `klams.toml`
(`scripts/provision-storage-root.sh:67`). That string exists only in
`compose.env.example:35` (the Postgres password) — **it is not in
`klams.example.toml` at all**. So the substitution is a silent no-op
and the generated token is discarded.

Sprint 032 (#670) is the proximate cause: it commented out the leading
`bearer_token` line so fresh installs wouldn't start life holding a
full-admin credential. Correct decision — but the provision script
wasn't updated, and now *every* token form in the rendered
`klams.toml` is commented out. The service then hits
`crates/klams-service/src/main.rs:501` and bails with
`AuthConfigError::NoTokens`.

Net effect: **run the documented provisioning path on a clean host and
the service will not boot.** You wouldn't have noticed — kubs0's
config predates the change and the script skips rendering when config
already exists (`:58`).

Fix: have the script emit a scoped `[[auth.tokens]]` grant (read +
write + manage, `agent_name = "operator"`) with the generated token,
and print it. That is strictly better than restoring the legacy
admin-everything token and matches 032's intent.

**B-2. `deploy/klams-service.service` hardcodes `/gratch/klams-backup`
in `ReadWritePaths=`.** On a host without that path, systemd's
`ProtectSystem=strict` unit fails to start. Fix: drop the line from the
shipped unit and document it as a drop-in for anyone enabling
`[backup]` (the comment above it already explains the mechanism).

**B-3. `KLAMS_CONFIG` defaults to `/ai/klams/config/klams.toml`**
(`crates/klams-service/src/main.rs:26`, plus five `justfile` recipes).
Not fatal — the env var overrides — but it means every default path
assumes your storage-root layout. An XDG-ish fallback
(`$XDG_CONFIG_HOME/klams/klams.toml`) would cost ten lines.

### 2.2 Tier 1 — papercuts a tester will hit within an hour (~1 sprint)

- **`viewport/src-tauri/src/config.rs:22` defaults to
  `http://kubs0:7777`**, and the placeholder in
  `viewport/src/routes/+layout.svelte:94` says the same. Trivial, but
  it is the first thing a tester sees.
- **`justfile` defaults to Ken's machines** — `viewport_host` is
  `kenhi@cleo`, `klams_mind` points at `../klams-mind`. Both are
  `env_var_or_default`, so overridable; both should probably have no
  default and fail loudly, the way `KLAMS_TOKEN` was fixed in sprint
  031 (#682) for exactly this reason.
- **`scanner.example.toml` ships `roots = ["/home/ken/src"]`.** Should
  be an obvious placeholder that errors rather than silently scanning
  nothing.
- **Neither `scanner.toml` nor `monitor.toml` is rendered by the
  provision script** — a tester copies them by hand and discovers the
  `token`/`url` fields the hard way.
- **The provision script's closing instructions omit `reranker`** from
  the `docker compose up` line while `klams.example.toml` now has
  `reranker_url` enabled. Degrades gracefully, but produces a
  confusing warning on a first run.
- **`monitor.example.toml` defaults `units` to klams' own units and
  carries a `[kpidash]` block referencing `rpi53`.** Commented and
  inert (the `kpidash` cargo feature is on by default but does nothing
  without the config section), so this is a documentation problem, not
  a code one.
- **`[backup] pg_bin_dir` is a real config key** (read at
  `crates/klams-service/src/main.rs:708`, used by `just
  backup-verify`) that appears **nowhere** in
  `klams.example.toml`. Anyone whose `pg_dump` isn't on `PATH` at the
  right version is stuck.

### 2.3 Tier 2 — things that are only worth doing if this becomes real (2–3 sprints)

- **A single `docs/install.md`** written forward, for someone who has
  never read a sprint doc. This is the largest item on the page by
  effort and the one with the highest payoff. `docs/setup.md` is 765
  lines of accreted sprint history — excellent as a record, hostile as
  an onboarding path. Same for the README, which sends newcomers to
  three different sprint quickstarts.
- **A first-run smoke path that proves the whole stack**, from empty
  Postgres to one `memory_add` + `memory_search` round-trip.
  `scripts/verify-mvp.sh` is close to this already, but it is framed as
  SC-001..SC-009 acceptance criteria from sprint 001.
- **Compose-mode klams-service.** The block exists, commented out, in
  `docker-compose.yml`. Uncommenting it and shipping a Dockerfile would
  let a tester run everything with one command and skip the Rust
  toolchain entirely. This is probably the highest-leverage *code*
  change for testers specifically.
- **A `cpu` compose override** to sit alongside `docker-compose.gpu.yml`,
  with a small-model default. Right now the CPU path is documented in a
  comment and exercised only by CI.
- **Sample corpus / seed data.** See §4.3 — this is the difference
  between a tester who says "it runs" and one who says "this is good."

### 2.4 What does *not* need changing

Worth stating explicitly, because it's the surprising part:

- No hostname, IP, or user is in any code path. Every hit for `kubs0`,
  `kai`, `/home/ken` is in a comment, a test fixture, or a doc.
- The type system is generic — `UserFact`/`TaskFact`/`EnvFact`,
  `User`/`Controller`/`Task`/`AgentProposal`, authors, four flat
  scopes. Nothing needs renaming.
- Migrations seed only a `system` author and a `lost` author. No
  Ken-specific rows.
- Auth, decay, retrieval, backup, summarization are all fully
  config-driven.
- The scanner's ignore rules are generic (`.gitignore` + `.klamsignore`
  + an extension allowlist).
- `klams-mind` coupling is one hardcoded string —
  `EXTRACTOR_AGENTS = ["klams-mind"]` in
  `crates/klams-core/src/provenance.rs:26` — plus the `just eval`
  recipe. Neither is required to run klams.

---

## 3. External to the project

This is where the real dependencies live, and it's a longer list than
the code changes.

### 3.1 Required

- **An MCP-capable agent.** klams is a memory *service*; without an
  agent wired to it, it is a REST API and a Svelte app. The tester needs
  Claude Code, Copilot, or another MCP client — and needs to actually
  work in a way that generates memory worth recalling.
- **Agent instructions.** This is the piece that is easiest to
  underestimate. Your global `CLAUDE.md` carries a ~40-line routing
  policy — *"recall-shaped question → `memory_search` FIRST"*, the
  deferred-vs-disconnected diagnostic table, the write-back rule.
  **Without something like that, agents mostly don't call klams.** The
  repo ships `docs/klams-mcp-for-agents.md` (good, but wired to your
  tailnet URL) and `.github/memories/klams-usage.md` for Copilot. A
  tester needs a portable version of the routing policy, not just the
  connection instructions.
- **Network reachability + TLS.** The service speaks **plain HTTP
  only** — there is no TLS anywhere in `klams-service` or
  `klams-api`. Your `https://kubs0.encke-wahoo.ts.net:7777/mcp` is
  Tailscale Serve terminating TLS in front, and that's **entirely
  outside this repo**. A tester either runs everything on one host
  (loopback, fine), or has to solve TLS + auth exposure themselves. If
  their agent runs on a different machine than the service — which is
  the normal case — this is a required, undocumented step.
- **A bearer token they mint themselves**, per agent, with an
  `agent_name` so writes are attributable (§B-1 above makes this
  harder than it should be).

### 3.2 Optional — currently entangled, cleanly separable

- **`klams-mind`** — separate repo (private?). Owns the retrieval eval
  suite and the session-extraction pipeline. `just eval` and `just
  eval-report` need a checkout. Not required to run klams; **is**
  required to reproduce any retrieval-quality claim.
- **korg** — your work-item system. Referenced heavily in sprint docs
  and code comments (`korg #628`, `korg:695`). Read-only context for a
  tester; zero runtime coupling.
- **kpidash / Redis on rpi53** — behind a config section that is inert
  when absent. Fine as-is.
- **Grafana + Prometheus** — behind the `observability` compose
  profile, with a dashboard at `deploy/grafana/klams.json`. Works
  anywhere; nothing host-specific.
- **The viewport** — a whole second toolchain (pnpm, Node, Tauri) and
  a Windows binary you cross-compile and `scp`. For a tester, I'd
  treat this as strictly optional and point them at
  `just viewport-build-linux` if they want it.

---

## 4. What else you should be asking

The four questions above are the tractable ones. These are the ones
that actually decide whether this is a good idea.

### 4.1 What does "tester" mean here, concretely?

There are at least four different asks hiding in that word, with very
different costs:

| Interpretation | What you provide | Cost to you |
|---|---|---|
| **Reader** — wants to understand the design | Nothing new; point at `docs/architecture.md` + the pitch page | ~zero |
| **Guest** — an account on *your* instance | A scoped token, an `agent_name`, tailnet access | Low technically, **high** on privacy (§4.2) |
| **Operator** — runs their own instance | Tier 0 + Tier 1 fixes, an install doc | ~1.5 sprints + support |
| **Contributor** — files issues, sends PRs | All of the above + CONTRIBUTING, issue templates, a stated support posture | Ongoing, unbounded |

**Ask them which one they mean before doing any work.** My read of
"be a tester" is *Operator*, but it's frequently *Reader* wearing
different words, and Reader costs you nothing.

### 4.2 If it's "Guest" — the privacy answer is no

Your klams holds 180k points scanned from `/home/ken/src`, plus every
memory your agents have written about your machines, your tokens'
whereabouts, your infrastructure. Scopes are flat and coarse
(`read` grants read of *everything*); there is **no per-author or
per-tenant read isolation** anywhere in the store. A `read` token sees
the whole corpus. That is the right design for a single-operator
system and disqualifying for a guest.

Don't offer this option. If asked, the honest framing is "there's no
tenancy model, and there shouldn't be — it's a single-operator
system."

### 4.3 An empty klams is unimpressive, and that's a real risk

This is the thing I'd worry about most. klams' value is *emergent from
accumulated corpus*. A tester who stands it up on Saturday has:

- zero facts
- zero knowledge chunks until they point the scanner somewhere
- zero events until the monitor has run for a while
- no agent habits pushing recall through it

They will conclude it's a thin wrapper over Qdrant. Everything
interesting in the retrospectives — provenance tiering, dedupe,
reranking, the search-miss log — only *manifests* at corpus scale.

Mitigations, in ascending cost: a documented "point the scanner at
your own `~/src` and wait 24h" expectation; a seed corpus of public
docs shipped as a fixture; or `just` recipes that pre-load the
`scale-fixture` (already exists for perf testing —
`--features scale-fixture`) so the surfaces at least have something in
them. Whatever you do, **set the expectation in writing** that week one
is the boring week.

### 4.4 The support burden is the actual cost

Tier 0 + Tier 1 is maybe 1.5 sprints of your time. The support tail is
unbounded and lands on your evenings. Things to decide *before*
answering:

- What's your response-time posture? "Best effort, no SLA, issues
  welcome, PRs better" is a legitimate and complete answer.
- Are you willing to accept PRs? Every "for portability" change
  directly contradicts AGENTS.md's standing rule ("do not generalize
  paths, hostnames, or assumptions"). If you take contributions, that
  rule needs an explicit carve-out or it becomes a source of friction
  with every contributor and every agent working in the repo.
- Will you support non-NVIDIA / CPU-only setups? Saying "CUDA or the
  OpenAI-compat endpoint, nothing else" up front is much cheaper than
  discovering an AMD user mid-thread.
- Does generalization slow *your* velocity? Your sprint cadence is
  fast partly because you never have to ask "does this break someone
  else's setup." That property is worth money.

### 4.5 Secrets, licensing, and what's already public

- **License is MIT** and the repo is on GitHub as `kenhia/klams`. So
  they can already read, fork, and run it today. Worth noticing: **the
  ask may already be satisfied** — what they actually want is probably
  your *attention*, not your permission.
- **Nothing secret is committed** — tokens live in `/etc/klams/`,
  examples carry placeholders, `.env` files are gitignored. Verified,
  no action needed.
- Your infrastructure *shape* is thoroughly documented in public
  (hostnames, topology, GPU, tailnet naming, backup paths). That's a
  choice you've already made; a tester doesn't change it.

### 4.6 Versioning and the upgrade contract

The PATCH segment tracks the sprint number and MAJOR/MINOR are
hand-managed, which means **the version number says nothing about
compatibility**. Migrations are forward-only sqlx. Sprint 028 rebuilt
the entire Qdrant collection at a different vector width; sprint 032
dropped the old one. If a tester is a sprint or two behind and you ship
another corpus-shape change, they have no path forward that isn't
"wipe and re-scan."

If you want testers, you need one paragraph of policy: *are `main`
snapshots supported at all, or do you cut tags?* Right now the honest
answer is "run `main`, expect to re-scan occasionally," and that's
fine — but it should be written down.

### 4.7 What you'd actually learn from a tester

Worth being concrete about the upside, since the costs are all above:

- **Fresh-install correctness.** You already have evidence this is the
  weak spot — §B-1 is a live bug that only a fresh install can find,
  and sprint 032 (#647) was largely about example configs drifting
  from the live host. A second install is a permanent regression test
  for a class of bug you're demonstrably prone to.
- **Whether the agent-routing policy transfers.** Does klams work for
  someone whose `CLAUDE.md` you didn't write? That's a genuinely open
  and interesting question.
- **Retrieval quality on a corpus you didn't shape.** Your eval suite
  is 21 queries over your homelab. Someone else's corpus is a real
  test of the ranking work.

---

## 5. Recommended path

Ordered so each step is independently worth doing **even if the tester
never materializes**:

1. **Fix B-1 (broken provisioning) and B-2 (`/gratch` in the unit)
   regardless.** These are bugs in your own recovery story: if kubs0
   ever needs a rebuild from scratch, you hit both. ~half a sprint.
   This is the recommendation I'd act on today.
2. **Ask the colleague which of the four §4.1 roles they mean.** One
   message. Possibly ends the whole thread — if the answer is
   "Reader," you're done; the repo is already public and MIT.
3. **If Operator:** do Tier 1 (§2.2) plus a single forward-written
   `docs/install.md`, and state a support posture in the README. ~1
   sprint. Explicitly do *not* refactor for generality — fix the
   defaults and the docs, leave the architecture alone.
4. **Only if it goes well:** the Compose-mode service + Dockerfile
   (§2.3). This is what turns a two-hour setup into a ten-minute one,
   and it's the difference between one tester and several.

**Do not** attempt a general "make klams portable" sprint. The code is
already portable; the problem is defaults, docs, and one bug. Treating
it as an architecture problem would cost 5× and buy nothing.

---

## 6. Effort summary

| Scope | Effort | Blocks a tester? |
|---|---|---|
| Tier 0 — provisioning bug, `/gratch`, config default | ~0.5 sprint | **Yes** |
| Tier 1 — defaults, example configs, missing config docs | ~1 sprint | Painful without |
| `docs/install.md` written forward | ~0.5 sprint | Painful without |
| Tier 2 — Compose service, CPU override, seed corpus | ~2 sprints | No |
| Support posture, versioning policy, CONTRIBUTING | ~0.25 sprint | No, but decide early |

**Minimum viable tester track: ~2 sprints.** Of which ~0.5 you should
do anyway.

---

## 7. Work items

None created, per instruction. If this becomes real, create a
**`generalize`** area under the `klams` project and file everything in
§2 there — the Tier 0 items belong in a normal sprint regardless of
whether generalization proceeds, so consider filing those under the
existing area instead and keeping `generalize` for Tier 1/2.
