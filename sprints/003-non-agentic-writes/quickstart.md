# Quickstart: Sprint 003 Walkthrough

**Sprint**: 003-non-agentic-writes
**Audience**: an operator (Ken or future Ken) bringing sprint 003 up on `kubs0`.
**Prereq**: sprint 002 is deployed on `kubs0` and `/healthz` returns 200.

This is the script you follow once the sprint is implemented. It
mirrors the structure of [001's quickstart](../001-initial-mvp/quickstart.md)
and [002's quickstart](../002-safety-and-write-ops/quickstart.md). Each
section maps to a user story in [spec.md](spec.md).

> Conventions used below:
>
> - `kubs0$` — shell on `kubs0`.
> - `kai$` — shell on a dev box.
> - `K_URL=http://127.0.0.1:7777` — set in the operator's shell.
> - `K_TOK=<bearer-from-/etc/klams/klams.toml>` — set in the operator's shell.

---

## §1 — Build and deploy the sprint-003 binaries

Apply the schema delta first, then build all three binaries
(`klams-service`, `klams-scanner`, `klams-monitor`), rotate the live
service binary, install/refresh the systemd units, restart the
service.

```bash
kai$  cd ~/src/ai/klams
kai$  just gate
kai$  cargo build --workspace --release --bins
kai$  rsync -av target/release/{klams-service,klams-scanner,klams-monitor} \
        kubs0:/tmp/klams-build-003/
kai$  rsync -av deploy/ kubs0:/tmp/klams-deploy-003/
kai$  rsync -av migrations/ kubs0:/tmp/klams-migrations-003/

kubs0$ sudo -u klams psql klams -f /tmp/klams-migrations-003/0003_events_task_idx.sql
kubs0$ sudo /tmp/klams-deploy-003/install-systemd.sh \
         --binaries /tmp/klams-build-003 \
         --units    /tmp/klams-deploy-003
kubs0$ systemctl status klams-service klams-scanner.timer klams-monitor
```

Expected: all three units `active`. The install script:

1. Creates the `klams` system user idempotently.
2. Rotates `klams-service` → `klams-service.prev` (atomic rename),
   then installs the new `klams-service`.
3. Installs `klams-scanner` and `klams-monitor` to
   `/usr/local/lib/klams/`.
4. Writes the three unit files + the scanner timer to
   `/etc/systemd/system/`.
5. `systemctl daemon-reload && enable --now klams-service
   klams-scanner.timer klams-monitor`.

Smoke: `curl -s "$K_URL/healthz"` returns `{"status":"ok"}`.
SC-004 gate.

---

## §2 — `GET /memory/policy` returns the source-trust table (US5)

```bash
kubs0$ curl -sH "Authorization: Bearer $K_TOK" "$K_URL/memory/policy" | jq .
```

Expected:

```json
{
  "User":          { "rank": 4, "description": "..." },
  "Controller":    { "rank": 3, "description": "..." },
  "Task":          { "rank": 2, "description": "..." },
  "AgentProposal": { "rank": 1, "description": "..." }
}
```

SC-005 gate (the unit test in `klams-core` already verified the JSON
projection equals the dispatcher's struct; this curl confirms the
endpoint is wired into the router and the bearer middleware lets it
through).

---

## §3 — A `Task`-source write returns `path: "canonical"` (US1 prerequisite)

```bash
kubs0$ curl -sH "Authorization: Bearer $K_TOK" -H "Content-Type: application/json" \
         -d '{
               "type": "EnvFact",
               "source": "Task",
               "payload": {
                 "key": "GPU_MODEL",
                 "value": "RTX 4090",
                 "host": "kubs0",
                 "task_id": "ansible-gather-gpu-2026-05-18-001"
               }
             }' \
         "$K_URL/memory/facts" | jq .
```

Expected (first run, ≈):

```json
{ "id": "...", "version": 1, "path": "canonical" }
```

Re-run the same `curl` immediately. Expected:

```json
{ "id": "<same-id>", "version": 1, "path": "canonical" }
```

`version` did **not** advance — canonical-hash dedupe held end-to-end
(SC-001).

A contradicting `AgentProposal` write against the same key:

```bash
kubs0$ curl -sH "Authorization: Bearer $K_TOK" -H "Content-Type: application/json" \
         -d '{
               "type": "EnvFact",
               "source": "AgentProposal",
               "payload": {
                 "key": "GPU_MODEL",
                 "value": "GTX 1080",
                 "host": "kubs0"
               }
             }' \
         "$K_URL/memory/facts" | jq .
```

Expected:

```json
{ "path": "dissent", "dissent_id": "..." }
```

US5 acceptance scenarios pass.

---

## §4 — Scanner indexes a fresh note within one cycle (US2)

```bash
kubs0$ NONCE="qs-003-$(date +%s)"
kubs0$ echo "# qs note\n\nUnique nonce: $NONCE\n" > ~klams/obsidian/qs-$NONCE.md
kubs0$ systemctl start klams-scanner   # one-shot, not the timer
kubs0$ journalctl -u klams-scanner -n 50 --no-pager
kubs0$ curl -sH "Authorization: Bearer $K_TOK" -H "Content-Type: application/json" \
         -d "{\"query\": \"$NONCE\", \"types\": [\"Knowledge\"], \"top_k\": 5}" \
         "$K_URL/memory/search" | jq '.results[0]'
```

Expected: top result is the chunk from the new note; `source_file`
ends in `/qs-$NONCE.md`. SC-002 gate.

Edit the note (`echo "additional content $NONCE-2" >>
~klams/obsidian/qs-$NONCE.md`), start the scanner again, search for
`$NONCE-2`: expect one hit. Delete the file, start the scanner
again, search for `$NONCE`: expect zero results.

---

## §5 — Monitor emits service.* events on restart (US3)

```bash
kubs0$ systemctl restart qdrant
kubs0$ sleep 30
kubs0$ curl -sH "Authorization: Bearer $K_TOK" \
         "$K_URL/memory/events?category=Service&service=qdrant&since=-2m" | jq '.events[] | {event, created_at}'
```

Expected: at least one `service.down` followed by one `service.up`.
SC-003 gate.

---

## §6 — Reboot resilience (US4)

```bash
kubs0$ sudo reboot
# wait for kubs0 to come back...
kai$   ssh kubs0 'systemctl is-active klams-service'
```

Expected: `active`, with `/healthz` returning 200 within 30s of
postgres+qdrant being ready. SC-004 gate.

---

## §7 — Sprint 002 walkthrough still passes (FR-023)

```bash
kai$  cd ~/src/ai/klams
kai$  TEST_DATABASE_URL=postgres://klams:klams_test@127.0.0.1:55432/klams \
      TEST_QDRANT_URL=http://127.0.0.1:56334 \
      TEST_TEI_URL=http://127.0.0.1:57070 \
      cargo test -p klams-service --tests --all-features \
        -- --include-ignored --skip search_p95 --test-threads=1
```

Expected: zero failures across `us1_*`, `us2_*`, `us3_*`, `us4_*`,
`us5_*` and the new `us3a/b/c/d` rows. SC-007 gate.

---

## §8 — Ship the handoff to ansible-k

After all the gates above are green:

```bash
kai$  cp -r sprints/003-non-agentic-writes/handoff/ \
            /home/ken/ansible-k/specs/klams-integration/
kai$  cd /home/ken/ansible-k && git add sprints/klams-integration && \
      git commit -m "specs: import klams integration handoff (klams 003)"
```

Then, in the ansible-k repo, open
`sprints/klams-integration/spec.md` and run the normal speckit cycle
to generate that project's `plan.md` and `tasks.md`. SC-006 gate.

---

## Phase 3 walkthrough table

The implementation phase fills in this table the same way sprint 002
did (see `sprints/002-safety-and-write-ops/spec.md` § "Phase 2
walkthrough"). Each row maps to one §N above plus a SC-NNN.

| Step | Evidence | Result |
|------|----------|--------|
| §1 build + deploy | install-systemd output + `systemctl status` | _to be filled_ |
| §2 GET /memory/policy | curl above + `contract_policy::*` tests | _to be filled_ |
| §3 path field | curl above + `contract_facts::*_path_*` tests | _to be filled_ |
| §4 scanner | curl above + `us3b_scanner_e2e` | _to be filled_ |
| §5 monitor | curl above + `us3c_monitor` | _to be filled_ |
| §6 reboot | `systemctl is-active` after reboot | _to be filled_ |
| §7 sprint-002 regression | full `--include-ignored` test run | _to be filled_ |
| §8 handoff shipped | `ls /home/ken/ansible-k/specs/klams-integration/` | _to be filled_ |
