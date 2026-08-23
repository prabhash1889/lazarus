use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result};
use lazarus_hostd::{
    HostServices, HostState, build_router, local_token_from_env, validate_loopback_addr,
};
use tracing::info;

const DEFAULT_LISTEN_ADDR: &str = "127.0.0.1:50051";

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let listen_addr: SocketAddr = std::env::var("LAZARUS_HOST_ADDR")
        .unwrap_or_else(|_| DEFAULT_LISTEN_ADDR.to_owned())
        .parse()
        .context("invalid LAZARUS_HOST_ADDR")?;
    validate_loopback_addr(listen_addr)?;
    let token = local_token_from_env()?;

    let services = HostServices::new(Arc::new(HostState::new()), token);
    let app = build_router(services);

    info!("lazarus-hostd {} starting", env!("CARGO_PKG_VERSION"));
    let listener = tokio::net::TcpListener::bind(listen_addr)
        .await
        .with_context(|| format!("binding {listen_addr}"))?;
    info!("lazarus-hostd listening on {listen_addr} (loopback only)");

    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;

    info!("lazarus-hostd shutting down");
    Ok(())
}
