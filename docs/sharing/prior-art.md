# Prior art — research findings behind the sharing design

**Status:** THOUGHT STAGE (see [README.md](README.md)).
**Researched:** 2026-07-30 (web survey; two parallel research passes).
Claims sourced from third-party aggregators rather than vendor pages
are flagged. Read [design.md](design.md) first; this document is the
evidence it stands on.

## 0. The two verdicts up front

1. **No usable, adopted cross-system agentic-memory interchange
   standard exists as of July 2026.** What exists is a sudden crowd of
   *candidates* — all dated May–July 2026, none with multi-vendor
   adoption, with colliding acronyms (two "PAM"s, two "AMP"s) that are
   themselves evidence of a pre-standardization land-grab moment.
   Hence [memory-feed-draft.md](memory-feed-draft.md). Details in §5.
2. **Nobody has shipped a cross-operator community memory pool among
   mutually semi-trusting strangers.** Every shipping multi-party
   memory product scopes sharing to one user's tools or one team under
   one admin. The ground design.md targets appears genuinely
   unoccupied (uncertainty: a niche product could have been missed).

## 1. Community/shared agent memory, and the poisoning literature

### Shipping products — all stop short of cross-operator sharing

- **Mem0 / OpenMemory MCP** — the closest widely-adopted analog, but
  scoped to *one user across many tools*. OpenMemory MCP is
  local-first shared memory any MCP client can attach to; mem0 cloud
  has per-user/agent/org multi-tenant scoping. ~48k GitHub stars.
  ([mem0.ai/blog/introducing-openmemory-mcp](https://mem0.ai/blog/introducing-openmemory-mcp),
  [mem0.ai/openmemory](https://mem0.ai/openmemory))
- **Zep** — temporal knowledge graph (Graphiti); multi-user model is
  *isolation-first* ("one user can never reach another user's
  memory"). ([blog.getzep.com](https://blog.getzep.com/unified-agent-memory-in-any-mcp-client/))
- **Letta** — April 2026 Conversations API shares memory across
  parallel sessions of *the same user's* agent.
  ([agentmarketcap.ai landscape](https://agentmarketcap.ai/blog/2026/04/10/agent-memory-vendor-landscape-2026-letta-zep-mem0-langmem))
- **ActiveLoop Hivemind** — nearest thing to #760 found: "agent traces
  become team skills. Solved once, shared everywhere." But scope is a
  *team under one admin*; trust model is thin — workspace isolation +
  RBAC + revoke, **no documented moderation workflow, human review, or
  formal provenance for shared skills**.
  ([deeplake.ai/hivemind](https://deeplake.ai/hivemind),
  [github.com/activeloopai/hivemind](https://github.com/activeloopai/hivemind))
- Smaller: **memU** ("shared LLM wiki" — [github.com/NevaMind-AI/memU](https://github.com/NevaMind-AI/memU));
  an Awesome-list entry describes git-based swarm memory syncing
  lessons via GitHub Issues — someone already using git as the shared
  substrate ([TsinghuaC3I/Awesome-Memory-for-Agents](https://github.com/TsinghuaC3I/Awesome-Memory-for-Agents)).
  A governed-memory write-up describes "explicit commits and manual
  steward approvals, knowledge certified before LLM access" — the
  curator pattern design.md §5 adopts
  ([promptowl.ai](https://promptowl.ai/resources/persistent-memory-ai-agents/)).
- Awesome lists for the space:
  [AgentMemoryWorld/Awesome-Agent-Memory](https://github.com/AgentMemoryWorld/Awesome-Agent-Memory),
  [TeleAI-UAGI/Awesome-Agent-Memory](https://github.com/TeleAI-UAGI/Awesome-Agent-Memory).

### Memory poisoning — the load-bearing threat literature

- **AgentPoison** (NeurIPS 2024): backdoors RAG-based agent memory;
  ≥80% attack success at <0.1% poison rate.
  ([neurips.cc](https://neurips.cc/virtual/2024/poster/94715))
- **MINJA**: query-only memory injection, >95% success via bridging
  steps. ([emergentmind.com](https://www.emergentmind.com/topics/persistent-memory-poisoning))
- **MemoryGraft** (Dec 2025): implants fake "successful experiences";
  agents replicate retrieved patterns.
  ([arxiv 2512.16962](https://arxiv.org/html/2512.16962v1))
- **"From Untrusted Input to Trusted Memory"** (June 2026): the
  tension is inherent — the write/retrieval aggressiveness that helps
  long-horizon performance *is* the attack surface.
  ([arxiv 2606.04329](https://arxiv.org/pdf/2606.04329)); certified
  defense: SMSR ([arxiv 2606.12703](https://arxiv.org/pdf/2606.12703)).
- **OWASP Agent Memory Guard** — dedicated project (memory poisoning
  is ASI06 in the OWASP agentic threat taxonomy): integrity baselines,
  injection detection, and a defense-in-depth list — *partitioning,
  provenance tracking, context isolation, temporal decay* — that maps
  almost one-to-one onto design.md's structure.
  ([owasp.org](https://owasp.org/www-project-agent-memory-guard/),
  [humansecurity.com](https://www.humansecurity.com/learn/blog/agentic-ai-security-owasp-threats/))
- ChatGPT's memory feature has documented exfiltration attacks via
  injected content ([arxiv 2406.00199](https://arxiv.org/pdf/2406.00199));
  Simon Willison's dual-LLM pattern (privileged LLM never reads
  untrusted content) is the canonical architectural defense
  ([simonwillison.net](https://simonwillison.net/tags/prompt-injection/)).

**Takeaway:** every published attack assumes an unmediated write path
into the victim's store. Treat inbound shared memories as untrusted
input: per-record signed provenance, a curation gate before the pool,
partition shared from local at retrieval, and never let origin blend
away. #768's loop prevention is literally OWASP's "provenance
tracking + memory partitioning."

## 2. Sanitization / safe-to-publish tooling

- **Microsoft Presidio** — default open-source PII pass: NER + regex +
  checksum recognizers, pluggable anonymizers; actively developed
  through 2026. Practitioner caveat: needs real build-out before it
  satisfies audits.
  ([github.com/microsoft/presidio](https://github.com/microsoft/presidio),
  [predictionguard.com](https://predictionguard.com/blog/pii-detection-redaction-llm-pipelines-regulated-industries))
- **LLM Guard** wraps Presidio for LLM pipelines
  ([protectai.github.io/llm-guard](https://protectai.github.io/llm-guard/input_scanners/anonymize/));
  commercial DLP (Nightfall etc.) and a 2026 API comparison:
  [grepture.com](https://grepture.com/compare/best-pii-redaction-apis-for-llms).
- **Secrets in prose**: TruffleHog (hundreds of detectors, **live
  credential verification**) and Gitleaks (regex + entropy, fast) both
  scan arbitrary text. Consensus: Gitleaks-fast at write time,
  TruffleHog-depth in the publish pipeline.
  ([trufflesecurity.com](https://trufflesecurity.com/trufflehog),
  [jit.io comparison](https://www.jit.io/resources/appsec-tools/trufflehog-vs-gitleaks-a-detailed-comparison-of-secret-scanning-tools))
- **LLM-based redaction** is mainstream as a *layer*, not a
  replacement ([Fabric AI functions example](https://community.fabric.microsoft.com/t5/Data-Engineering-Community-Blog/PII-Detection-and-Redaction-with-Fabric-AI-Functions/ba-p/4731952)).

**Takeaway:** there is **no off-the-shelf "safe-to-publish"
certifier**; everyone composes layers. Crucially, recall on
*contextual* homelab-shaped leaks is poor everywhere — Presidio does
not know `kubs0` is sensitive. The credible pipeline is: secret
detectors → Presidio-class PII pass → LLM contextual review seeded
with an **operator-maintained denylist** (hostnames, usernames,
tailnet names) → **human approval of each digest**. Automated-only
publishing is not defensible with current tooling — which is why
design.md §5 makes the human gate non-negotiable.

## 3. Hosting shapes and costs (July 2026 prices)

| Shape | What | Rough cost |
|---|---|---|
| (a) Tiny VPS, whole Rust stack | Hetzner CX33 4 vCPU/8 GB **€6.49/mo**, CPX32 **€13.99/mo**; DO 2 vCPU/4 GB ~$24; Fly shared-4x/8 GB ~$23.66 | **€7–25/mo** |
| (b) Managed components | Qdrant Cloud free tier (1 GB RAM/4 GB disk) + Neon/Supabase free-to-$25 + embedding API (<$1/mo at this scale) | **$0–25/mo**, cliff to $60–250 past free tiers |
| (c) Object-storage digests | Cloudflare R2: 10 GB + 10M reads free, $0 egress at any volume | **~$0/mo, realistically forever** |

Sources: [hetzner.com](https://www.hetzner.com/pressroom/new-cx-plans/)
(note: Hetzner raised prices ~37% in April 2026),
[digitalocean.com/pricing](https://www.digitalocean.com/pricing/droplets),
[fly.io/docs/about/pricing](https://fly.io/docs/about/pricing/) (free
tier is gone), [mecanik.dev on R2](https://mecanik.dev/en/posts/cloudflare-r2-pricing-explained-real-costs-vs-s3-and-backblaze/).
Qdrant/Neon numbers are from third-party aggregators
([ranksquire.com](https://ranksquire.com/2026/04/19/qdrant-cloud-pricing-2026/),
[costbench.com](https://costbench.com/software/vector-databases/qdrant/free-plan/),
[vela.simplyblock.io](https://vela.simplyblock.io/articles/neon-serverless-postgres-pricing-2026/))
— **verify against vendor pages before committing**.

Two non-cost notes that matter more than the dollars: (1) CPU-only TEI
on a shared-vCPU box is the tight fit for shape (a) — budget the
CPX32 class if embedding latency matters; (2) an API embedder on a
hosted store breaks vector-space compatibility with local TEI models,
but shared-store vectors are computed server-side either way, so this
constrains less than it looks. **Cost does not differentiate the
shapes at this scale; operations and architecture do** — which is the
design.md §3 argument from the other direction.

## 4. Syndication and signing precedents

- **Feed formats**: JSON Feed (2017) is the JSON-native RSS/Atom
  successor, trivially extensible with custom per-item fields — a
  memory digest is naturally a feed with a memory-payload extension.
  ([jsonfeed.org](https://www.jsonfeed.org/),
  [lighthouseapp.io](https://lighthouseapp.io/blog/what-is-json-feed))
- **Planet aggregators** — static merge of N members' feeds into one
  combined view, no live service; people still build these
  ([dasroot.net, Dec 2025](https://dasroot.net/posts/2025/12/building-technical-blog-aggregator-planet-style/)).
  Direct precedent for the deferred community index.
- **apt / SecureApt** — the signed-index pattern: one signed manifest
  carries hashes of everything else; clients pin trusted keys.
  ([wiki.debian.org/SecureApt](https://wiki.debian.org/SecureApt))
  Notably, feed signing never took hold in the RSS world — the signing
  story comes from the package-repo world.
- **TUF** — the hardened ceiling (threshold roles, rollback
  protection; used by PyPI and Sigstore's own root). Overkill at 5–50
  people; cited as the upgrade path, not the floor.
  ([theupdateframework.github.io](https://theupdateframework.github.io/specification/latest/))
- **Signing floor**: **minisign** — Ed25519, one binary; its *trusted
  comment* is signed alongside the file, explicitly recommended for
  embedding filenames/versions against downgrade attacks — a perfect
  fit for "digest #47 from X, supersedes #46"
  ([jedisct1.github.io/minisign](https://jedisct1.github.io/minisign/)).
  **Sigstore cosign sign-blob** for keyless signing bound to an OIDC
  identity with a transparency-log proof — verification asserts
  "signed by this GitHub identity" with no key distribution
  ([docs.sigstore.dev](https://docs.sigstore.dev/cosign/signing/signing_with_blobs/)).

**Takeaway:** the composable stack for the chosen topology — JSON-Feed
shape → planet-style static merge → apt-style signed manifest
(minisign floor, cosign-keyless or TUF as upgrades) → R2/S3 hosting —
is boring, proven, and serverless at every layer.

## 5. Interchange-standard candidates, surveyed

All candidates are May–July 2026, none adopted beyond their author.
In decreasing relevance to klams:

- **agentmemoryprotocol.io "AMP"** (YouTale AI, Apache 2.0, v0.1
  draft) — the only **markdown-first** candidate: a directory store,
  `amp.yaml` manifest, nodes as markdown files with frontmatter (`id,
  type, created, modified, author, confidence, status, tags`),
  wiki-links, defined import paths from ChatGPT/Claude/Mem0/etc.
  Closest to the klams shape; a months-old single-startup draft; no
  supersession field evident. *(Frontmatter list from search
  summaries; the site's `/spec` 404s — verify against
  [the repo](https://github.com/agentmemoryprotocol/agentmemoryprotocol)
  before citing field lists as fact.)*
- **PAMSPEC / draft-infantado-agent-memory-architecture-00** (IETF
  individual I-D, July 2026, informational, zero adoption) — the most
  carefully thought-out *record schema* found: `object_id` +
  `version_id` versioned identity (a clean supersession model),
  PROV-style provenance, four temporal fields
  (`observed_at`/`asserted_at`/`committed_at`/`recorded_at`),
  separated lifecycle/validation states.
  ([datatracker](https://datatracker.ietf.org/doc/html/draft-infantado-agent-memory-architecture-00))
- **Portable Agent Memory** (arXiv 2605.11032, one author, May 2026) —
  memory *transfer* protocol: content-addressed entries, Merkle-DAG
  provenance, capability-based disclosure. JSON/CBOR + crypto
  machinery — heavier than needed, but hash-linked supersede chains
  are directly borrowable.
  ([arxiv.org/abs/2605.11032](https://arxiv.org/abs/2605.11032))
- **memorywire** (arXiv 2606.01138, one author; renamed from "AMP" to
  dodge the collision above) — standardizes the **API verbs**
  (remember/recall/forget/merge/expire), not the portable record.
  Notable for the strongest cross-system code found: five backend
  adapters (sqlite-vec, mem0, Letta, Cognee, pgvector). CC-BY-4.0.
  ([arxiv.org/abs/2606.01138](https://arxiv.org/abs/2606.01138),
  [pypi](https://pypi.org/project/memorywire/0.4.0/))
- **W3C AI Agent Memory Interoperability Community Group** — proposed
  2026-05-18, chartered 2026-06-19, 17 participants, **nothing
  published**. The only community-shaped venue; heavy crypto-identity
  scope (post-quantum binding, GDPR erasure). A place to *take* a
  draft, not a spec to adopt.
  ([w3.org/community/ai-agent-memory-interop](https://www.w3.org/community/ai-agent-memory-interop/))
- **Letta Agent File (.af)** (~1.2k stars — best-known artifact) —
  **wrong granularity**: whole-agent checkpoints (prompt, tools,
  message history), and archival memory — the part klams cares about —
  is explicitly unsupported. Only Letta implements it. Prior art for
  "open format, one implementer."
  ([github.com/letta-ai/agent-file](https://github.com/letta-ai/agent-file))
- **cognee COGX** — a real "portable memory archive" with shipping
  Mem0/Zep/Letta importers, but one vendor's format, graph-shaped, no
  public spec found.
  ([cognee blog](https://www.cognee.ai/blog/deep-dives/inside-cognee-1-0))
- **Portable AI Memory** (portable-ai-memory.org, one author) —
  "vCard for AI memory": three JSON schemas (memory store,
  conversation, embeddings), content hashes, explicit "file format,
  not a platform" scoping. No adopters.
  ([portable-ai-memory.org](https://portable-ai-memory.org/))
- **The big vendors' actual practice**: mem0 exports via user-defined
  Pydantic schemas ([docs.mem0.ai](https://docs.mem0.ai/cookbooks/essentials/exporting-memories));
  Zep publishes a pairwise mem0→Zep migration guide
  ([help.getzep.com](https://help.getzep.com/mem0-to-zep)). Today's
  "interchange" is bespoke exports and pairwise migration scripts.
- **MCP** standardizes tool *access* to memory servers (everyone above
  ships one) but defines **no record schema** — the record format is
  precisely the gap.

### Stable building blocks worth borrowing from outside the space

**W3C PROV** for provenance vocabulary
([w3.org/TR/prov-o](https://www.w3.org/TR/prov-o/)), **Dublin Core**
for descriptive metadata
([dublincore.org](https://www.dublincore.org/specifications/dublin-core/dcmi-terms/)),
**JSON Feed / Atom** for the minimal-envelope discipline, and the
vCard/iCalendar adoption story both "PAM" projects invoke: a tiny
required core with everything else optional is the property that got
those formats adopted — and the property every 2026 candidate except
AMP lacks.

### What [memory-feed-draft.md](memory-feed-draft.md) borrows, explicitly

1. PAMSPEC's field vocabulary: versioned identity as the supersession
   model, and the observed-vs-recorded temporal split.
2. AMP's file shape: markdown body + YAML frontmatter, directory =
   store, git-friendly.
3. Portable Agent Memory's integrity idea: content hashes and
   hash-linked supersede chains, *without* the Merkle/crypto
   apparatus.
4. JSON Feed's envelope discipline: tiny required core, namespaced
   optional extensions.
5. MCP as the assumed access layer: the draft specifies records and
   archives only — no verbs (memorywire's turf), no identity
   ceremony (the W3C CG's turf). Those are where heavyweight drafts
   go to stall.
