# Sprint 056 — stop recommending RequiresMountsFor on a remote backup_dir

Proposal korg:4153, a slice of program korg:4162 (*Low-hanging fruit —
run 5*). Run as an overseen karc leg (`klams-97fa48`) on kubs0. It covers
one work item, **#3934**. Version `0.1.56`.

## Goal

k-homelab sprint 093 (WI 3899) moved kubs0's `backup_dir` from the NFS
path `/gratch/klams-backup` to the local `/backup/klams`. It also dropped
a hand-made `RequiresMountsFor=/gratch/klams-backup` drop-in: on
2026-10-16 that drop-in kept klams down fleet-wide, because gratch was
down across a kubs0 reboot. klams refuses to start when `backup_dir` is
missing, so a remote `backup_dir` is always a start dependency. The
repo still taught the old shape. Option (a) was already chosen in the
proposal notes: document what kubs0 runs (a local `backup_dir` plus an
external copy job) and change no behaviour. Option (c), making a missing
`backup_dir` non-fatal, is a behaviour change and stays out of scope.

## Premise check: holds

- `deploy/klams-service.service` (lines 10, 52) and
  `deploy/config/klams.example.toml` (line 308) still recommended
  `RequiresMountsFor=`.
- `deploy-kubs0` Preflight step 4 still listed `/gratch/klams-backup/`.
- Live kubs0: `/backup/klams/` holds nightly pairs through 2026-10-28.
  The k-homelab-managed drop-in `backup.conf` grants
  `ReadWritePaths=/backup/klams` and has no `RequiresMountsFor`.
  `klams-backup-push.timer` (k-homelab) copies the pairs to kai and
  gratch, with no dependency on klams-service in either direction.

## What shipped

- **Unit and example config:** the `RequiresMountsFor` advice is gone.
  Both now say to keep `backup_dir` on a local disk and copy it off-host
  with a separate job, and they cite the 2026-10-16 incident (WI 3899)
  as the reason.
- **`deploy-kubs0` skill:** the path table and Preflight step 4 now name
  `/backup/klams/`. The skill lives in this repo's `.claude/skills/` and
  reaches kubs0 through this checkout, which is the only host it runs
  on. It is not fleet-delivered.
- **docs/setup.md:** the drop-in section no longer suggests
  `RequiresMountsFor`. It gains a short "local disk, copy off-host"
  paragraph and corrects the line that said kubs0's drop-in points at
  `/gratch/klams-backup`.
- **docs/usage.md:** a pointer to that paragraph sits next to the
  drop-in example. The sample config and sample output paths now read
  `/backup/klams`.

No deploy: this sprint changes only docs, comments and the skill. The
version bump will appear on `/healthz` only after the next deploy.

## Repaired in passing

- `docs/diagrams/klams-write-path.svg` and `klams-topology.svg` still
  labelled the nightly backup destination as `/gratch/klams-backup`.
  They now say `/backup/klams`, and the gratch box is labelled as the
  copy's destination. This is a text-label edit only, and a re-grep
  confirms nothing live names the old path.

## Left alone, deliberately

- References to `/gratch/klams-backup` that describe history (the
  sprint 034 hardcoded `ReadWritePaths`, in setup/usage/architecture and
  the 2026-07-25 review). They are accurate as history.
- Test fixture strings in `mcp_lifecycle_verbs.rs` and
  `hermetic_rogue_agent_drill.rs`. They are arbitrary memory text, not
  config.
