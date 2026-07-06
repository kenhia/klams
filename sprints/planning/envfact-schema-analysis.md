# EnvFact schema analysis (May 20, 2026)

Working notes prior to deciding sprint 004 scope. Two threads:

1. The `value`-must-be-string disagreement with ansible-k.
2. Whether the current `{key, value: string}` model is the right fit for
   the actual host-facts data we want to store.

## Thread 1 — what's actually true today

`crates/klams-core/src/validate/facts.rs` (`EnvFactValidator`) enforces:

- `payload.key`: required, string, `^[A-Z][A-Z0-9_]*$`, ≤256 chars.
- `payload.value`: required, **string**, ≤4096 chars.
- `payload.task_id`: optional ansible task id when present.

Non-string `value` returns `422 validation_error
field=payload.value rule=type message="value must be a string"`. This
matches what ansible-k's T008 captured. Integration test
`crates/klams-service/tests/us3b_ansible_facts.rs` exercises only string
values (`value: "2"`).

The previous fix-forward note
(`/home/ken/ansible-k/specs/klams-integration/fixforward-source-and-encoding.md`)
asserted that the schema "accepts arbitrary JSON" — that was wrong and
unverified. ansible-k's revert to `to_json(sort_keys=True)` is correct
against the deployed contract.

The handoff API doc
`sprints/003-non-agentic-writes/handoff/api-contract.md` (lines 65–77)
shows an EnvFact example with free-form top-level keys
(`{host, kernel, distro}`) that would also fail validation. The doc has
never matched the validator.

### Path-forward items (carryover from previous turn)

- Send corrected note to ansible-k: their `to_json` workaround is the
  right adaptation; keep it. Mark the original fix-forward doc
  SUPERSEDED.
- Fix `sprints/003-non-agentic-writes/handoff/api-contract.md` to show
  the real `{key, value: "<string>"}` shape.
- Decide whether the validator should be relaxed (this is what thread 2
  is about).

## Thread 2 — does the current model fit the data?

### Raw shape (`/home/ken/ansible-k/host-facts/*.yml`)

Each host has two YAML files. Base file (`<host>.yml`) is grouped into
six top-level categories:

- `hardware` — scalars (cpu_cores, cpu_model, memory_mb, bios_*, …).
- `identity` — scalars (hostname, fqdn, system_vendor, …).
- `network` — scalars + `interfaces: []` (list of objects with
  ipv4_address, macaddress, name, type).
- `os` — scalars (distribution, kernel, python_version, …).
- `storage` — `block_devices: []` and `mounts: []` (list of objects
  with model/size, device/fstype/mount/size_gb).
- `user` — scalars (id, home, shell, gecos).

GPU file (`<host>-gpu.yml`) has `collected_at`,
`cuda_toolkit_package`, `cuda_toolkit_version`, `gpu: []` (objects with
driver_version, model, vram_mb).

So the natural unit is a tree, ~150 leaf attributes per host plus three
non-trivial arrays (interfaces, block_devices, mounts) and one optional
array (gpu). `kubs0` has 6 mounts, `kai` has 4, `kubsdb` has 5; counts
will grow.

### Likely agent query patterns

- Scalar lookup: "what's the kernel on kubs0?" → exact key.
- Numeric/categorical filter: "hosts with ≥ 64 GB RAM", "hosts on
  Ubuntu 24.04".
- Array probe: "which hosts have a GPU?", "which mounts on kubs0
  exceed 1 TB?", "hosts with btrfs filesystems".
- Cross-host group-by: "kernel versions in fleet", "GPU count by
  host".
- Free-text recall: "did any host mention SUPER somewhere?" — FTS-ish.

### How the current model holds up

`{key: STRING_ENUM, value: string}` forces one of three encodings:

1. **One fact per leaf attribute** (CPU_CORES=16, CPU_MODEL="...",
   MEM_MB=128620, MOUNT_0_DEVICE=..., MOUNT_0_FSTYPE=..., …).
   - Scalar lookup is great.
   - Arrays become positional: MOUNT_0_*, MOUNT_1_*, … which is fragile
     (reorder = churn) and clumsy to query as a set.
   - ~150 facts × N hosts = high write-amplification, large dedupe
     surface.
   - Filter queries ("mounts > 1 TB") need application-side joins.

2. **One fact per category, value=JSON-stringified blob** (current
   ansible-k approach: HARDWARE=<json>, NETWORK=<json>, ...).
   - Compact, human-readable in the row.
   - JSONB structural queries require `value::jsonb -> ...` casts
     everywhere consumers look, and consumers must remember the value
     is JSON-encoded text.
   - FTS over `value` works coarsely (substring match) but loses
     structure.
   - 4096-char cap is tight: kubs0's `network` block alone serializes
     to ~1.6 KB; `storage` (with all mounts) approaches 2 KB. Headroom
     is fine today but not generous if host counts of mounts/devices
     grow.

3. **Hybrid** — store the bag-of-scalars flat (option 1) AND a
   structured doc (option 2) for browse/snapshot. Maximizes query
   flexibility, doubles write volume.

### What the model would look like if we redesigned for this data

The leanest change is to make `value` accept any JSON. That gets us:

- Same `{key, value}` shape, same `(source, host, key)` dedupe.
- `value JSONB` natively → `value -> 'gpu' -> 0 ->> 'model'` works
  without casts.
- ansible-k drops `to_json`; posts the dict directly.
- FTS index switches from `value` text to a generated text projection
  (`jsonb_to_text` or similar) so structured payloads are still
  searchable.
- Validator changes: drop the `Some(serde_json::Value::String) =>`
  arm, replace with "any JSON, serialized size ≤ N KB" (probably
  bump the cap to 16 KB — the current full kubs0 host doc serialized
  is ~5 KB).

Backward compatibility is straightforward: a string is still valid
JSON. Existing `value: "2"` rows keep working.

### A bigger redesign worth weighing

What an agent really wants for these facts is a **typed document per
(host, category)** with structural query and partial-update semantics.
That implies a `HostProfile` or `EnvDoc` entity that's separate from
the small-key/scalar `EnvFact` shape:

- `EnvFact` stays as it is (or relaxed) for terse scalar attestations
  (GPU_COUNT=4, OS_VERSION=24.04, …).
- New `EnvDoc {host, category, payload: JSONB}` for the rich tree —
  one row per (host, category, source). Subject to the same
  source-trust ladder; same dedupe semantics on `(source, host,
  category)`.

Pros: clean separation of "named scalar attestations" (the original
EnvFact intent) from "structured snapshot" (the actual ansible-k
push). Each table can be indexed and FTS'd appropriately.

Cons: another entity to model, migrate, and document; ansible-k push
role gets two endpoints; viewport surfaces both.

### My recommendation

Sprint 004 — relax `value` to JSON (the small change). It buys ~80%
of the ergonomic win for ~10% of the work, doesn't paint us into a
corner, and is reversible. The bigger `EnvDoc` redesign should wait
until we have at least one agent integration to tell us what the
read-side queries actually look like — picking a structural model
without consumers is the kind of decision we'll regret.

Concrete sprint outline:

1. `validate/facts.rs`: drop the string-type check on `value`. Keep
   `key` rules. Replace 4096-char text cap with a serialized-bytes cap
   (16 KB) on the JSON value.
2. Migration: none required (string is valid JSON; existing rows fine).
3. FTS: adjust the generated-text projection so JSON values index as
   their stringified content (or path-flattened text).
4. Contract test: add EnvFact upsert with dict value to
   `tests/contract_facts.rs`.
5. OpenAPI / api-contract.md / handoff docs: update to "value: any
   JSON, serialized ≤ 16 KB". Include both scalar example and dict
   example (matching the hardware/network/etc. categories ansible-k
   actually pushes).
6. Backlog: `EnvDoc` entity proposal — defer until first agent
   consumer exists and we know the query shape.
7. ansible-k follow-up: drop `to_json(sort_keys=True)`; emit dict
   values directly. Their T010 unblocks.

Open question for the user before implementing: 16 KB cap reasonable,
or do we want it bigger (32–64 KB) to leave headroom for hosts with
many mounts/interfaces?
