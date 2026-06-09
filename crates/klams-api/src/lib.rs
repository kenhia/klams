//! axum router, auth middleware, request validation, error mapping.
//!
//! Builds the `/memory/*`, `/healthz`, and `/metrics` routes consumed
//! by the klams-service binary.

pub mod auth;
pub mod error;
pub mod handlers;
pub mod middleware;
pub mod router;

pub use auth::{require_bearer, AuthState, AuthenticatedAuthor, AuthenticatedScopes, TokenGrant};
pub use error::ApiError;
pub use router::{build_router, build_router_with_auth, with_metrics, ApiState};
