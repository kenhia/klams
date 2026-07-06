# Sprint 006 — Backup sizing fixture

Day-0 sizing data (~10k facts / ~20k knowledge chunks / ~50k events)
lives in [`crates/klams-service/tests/common/fixture.rs`](../../crates/klams-service/tests/common/fixture.rs)
as the `FixtureScale::large()` preset.

The loader is gated behind the `scale-fixture` Cargo feature so it
does not run with `cargo test --workspace`:

```bash
docker compose -f tests/docker-compose.test.yml up -d
cargo test -p klams-service --features scale-fixture \
    --test scale_loader -- --ignored --nocapture
```

`just backup-size` invokes the loader, runs one `backup::run_once`,
and prints a `kind | bytes | seconds` table (plus appends a dated
entry to [`sprints/006-maintenance-and-backups/sizing.md`](../../sprints/006-maintenance-and-backups/sizing.md)).
