//! Backup snapshot + restore mechanics (sprint 006).
//!
//! The `klams-store` half of the backup feature owns the low-level
//! snapshot/restore primitives — invoking `pg_dump`/`pg_restore` and
//! Qdrant's snapshot REST API — plus filesystem retention pruning.
//! Orchestration (scheduling, lockfile, lifecycle, `status_hook`,
//! metrics) lives in `klams-service::backup`.

pub mod postgres;
pub mod qdrant;
pub mod retention;
