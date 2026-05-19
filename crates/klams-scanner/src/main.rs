//! `klams-scanner` binary scaffold (sprint 003 T002).
//!
//! Real walk/chunk/publish wiring lands in T034–T039. For now the
//! binary prints its banner via structured logging and exits 0 so
//! the workspace build is green.

use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    tracing::info!("{}", klams_scanner::banner());
    Ok(())
}
