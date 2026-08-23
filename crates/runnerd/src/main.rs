use anyhow::Result;
use tracing::info;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    info!("lazarus-runnerd {} starting", env!("CARGO_PKG_VERSION"));
    info!("lazarus-runnerd idle (no supervision wired yet; Phase 0 shell)");

    tokio::signal::ctrl_c().await?;
    info!("lazarus-runnerd shutting down");
    Ok(())
}
