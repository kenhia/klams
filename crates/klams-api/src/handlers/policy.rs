//! `GET /memory/policy` — serves the source-trust policy table.
//!
//! See `contracts/memory_policy.md`. Pure read; no I/O.

use axum::Json;
use klams_core::PolicyTable;

#[allow(clippy::unused_async)]
pub async fn get_policy() -> Json<PolicyTable> {
    Json(PolicyTable::default())
}
