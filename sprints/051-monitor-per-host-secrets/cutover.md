# Cutover and live check — kubs0, 2026-09-12

Everything below ran **from kubs0**, which is the host klams-monitor runs on.

## The control comes first, because the two values are identical

`/etc/klams/monitor.env` and `/etc/khomelab/secrets.env` carry the same
`REDISCLI_AUTH` — both fingerprint `76cde5f57955`, against an empty-string
control of `e3b0c44298fc`. A monitor that keeps publishing after the change
therefore proves nothing on its own (convention 3). Two controls make it mean
something:

1. **rpi53's Redis genuinely requires the password.** `REDISCLI_AUTH=<wrong>
   redis-cli -h rpi53 ping` → `AUTH failed: WRONGPASS`; the real value → `PONG`.
   So a successful publish is evidence the password worked.
2. **`monitor.env` was moved aside *before* the restart**, so during the check
   the per-host file was the only possible source of the value on disk.

## Sequence

1. Installed the repo unit and the new repo-owned drop-in
   (`10-khomelab-secrets.conf`).
2. **Removed the hand-made `10-kpidash.conf`.** Not optional: `EnvironmentFile=`
   directives accumulate across drop-ins, so leaving it would have kept a second
   reference to a file about to be deleted — and with `ignore_errors=no` that is
   a unit that fails to start.
3. Moved `monitor.env` aside, `daemon-reload`, `restart`.

## Results

| check | result |
|---|---|
| `systemctl show -p EnvironmentFiles` | `/etc/khomelab/secrets.env (ignore_errors=no)` — only the per-host file |
| `DropInPaths` | only `10-khomelab-secrets.conf` |
| unit state | `active (running)`, `NRestarts=0` |
| **fresh card on rpi53** | published **31s after** the restart, with `monitor.env` off disk |
| running process `REDISCLI_AUTH` | fingerprint `76cde5f57955` = the per-host file's; control `e3b0c44298fc` |
| keys the file injects | exactly `REDISCLI_AUTH` and `HF_TOKEN`; the latter unused by klams |

The process-environment fingerprint used `sudo cat /proc/<pid>/environ | tr`,
not `sudo tr … < /proc/<pid>/environ` — the redirect runs as the calling user
and fingerprints the empty string (convention 4). The trap did bite once on an
unrelated `wc -l`, which is how it was confirmed live rather than remembered.

## Deletions, after the check passed

- `/etc/klams/monitor.env` — the private copy this slice exists to remove.
- `/etc/klams/monitor.env.bak-20260813-202310` — its plaintext backup
  (see "Repaired in passing").

Monitor re-checked after both deletions: still `active`, `NRestarts=0`, card
fresh. `/etc/klams/` now holds only `klams.toml`, its 050 backup, `monitor.toml`
and `scanner.toml`.

## Cross-repo: the group membership, asserted separately

k-homelab `45d5297` (direct to `main`, mirroring the kstudiodash precedent
`c6021d4`) adds `klams` to kubs0's `secrets_group_members`. `bin/audit` named
exactly the one change, 166 tests green, `bin/apply kubs0 khomelab-secrets`
applied it, re-audit `ok`.

Asserted independently of the monitor, because a publishing monitor is no
evidence the membership took:

```
khomelab:x:1003:ken,klams
sudo -u klams  → reads the file      (OK)
sudo -u nobody → denied              (control)
```

klams-monitor still has `SupplementaryGroups=` empty and `NRestarts=0`: it
never needed the membership, exactly as convention 1 predicts.
