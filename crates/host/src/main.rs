use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result};
use lazarus_hostd::{HostServices, HostState};
use protocol_rs::{SystemServiceServer, TaskServiceServer, WorkspaceServiceServer};
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
    let services = HostServices::new(Arc::new(HostState::new()));

    info!("lazarus-hostd {} starting", env!("CARGO_PKG_VERSION"));
    info!("lazarus-hostd listening on {listen_addr} (loopback only)");

    tonic::transport::Server::builder()
        .add_service(SystemServiceServer::new(services.clone()))
        .add_service(WorkspaceServiceServer::new(services.clone()))
        .add_service(TaskServiceServer::new(services))
        .serve_with_shutdown(listen_addr, async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;

    info!("lazarus-hostd shutting down");
    Ok(())
}
