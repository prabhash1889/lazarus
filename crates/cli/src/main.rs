use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use protocol_rs::generated::{
    ClientHello, GetInfoRequest, HealthRequest, ProtocolVersion, ServingStatus,
    system_service_client::SystemServiceClient,
};
use protocol_rs::handshake;

#[derive(Parser)]
#[command(name = "lazarus", version, about = "Lazarus CLI")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Print toolchain and environment diagnostics.
    Version,
    /// Connect to the Host, negotiate the protocol, and report serving status.
    Status {
        /// Address of the running Host.
        #[arg(long, default_value = "http://127.0.0.1:50051")]
        addr: String,
    },
}

/// Renders the post-negotiation report as plain text so it can be unit
/// tested without a live connection.
fn format_report(
    negotiated_minor: i32,
    host_protocol: &ProtocolVersion,
    capabilities: &BTreeMap<String, bool>,
    status: ServingStatus,
) -> String {
    let mut lines = vec![format!(
        "negotiated protocol: {}.{} (host speaks {}.{}; client speaks {}.{})",
        host_protocol.major,
        negotiated_minor,
        host_protocol.major,
        host_protocol.minor,
        handshake::PROTOCOL_MAJOR,
        handshake::PROTOCOL_MINOR,
    )];

    if capabilities.is_empty() {
        lines.push("capabilities: (none)".to_string());
    } else {
        let rendered = capabilities
            .iter()
            .map(|(name, enabled)| {
                if *enabled {
                    format!("{name}=on")
                } else {
                    format!("{name}=off")
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(format!("capabilities: {rendered}"));
    }

    let serving = matches!(status, ServingStatus::Serving);
    lines.push(format!(
        "host status: {}",
        if serving { "SERVING" } else { "NOT_SERVING" }
    ));

    lines.join("\n")
}

/// Summarizes a gRPC failure, decoding the structured protocol error from the
/// status details when the host attached one.
fn describe_status(status: &tonic::Status) -> String {
    match handshake::error_from_status(status) {
        Some(err) => format!("{} [code {:?}]", err.message, err.code()),
        None => status.to_string(),
    }
}

async fn run_status(addr: &str) -> Result<()> {
    let mut system = SystemServiceClient::connect(addr.to_owned())
        .await
        .with_context(|| format!("cannot reach host at {addr}; is lazarus-hostd running?"))?;

    let hello = ClientHello {
        client: "lazarus-cli".into(),
        client_version: env!("CARGO_PKG_VERSION").into(),
        protocol: Some(handshake::CURRENT_PROTOCOL_VERSION),
        supported_features: Vec::new(),
        auth: None,
    };
    let reply = match system.negotiate(hello).await {
        Ok(response) => response.into_inner(),
        Err(status) => bail!("protocol negotiation failed: {}", describe_status(&status)),
    };
    let host_protocol = reply.protocol.unwrap_or_default();
    handshake::negotiate(&handshake::CURRENT_PROTOCOL_VERSION, &host_protocol)?;
    let negotiated_minor = reply.negotiated_minor;

    let info = system
        .get_info(GetInfoRequest {})
        .await
        .map_err(|status| anyhow::anyhow!("GetInfo failed: {}", describe_status(&status)))?
        .into_inner();
    let health = system
        .health(HealthRequest {})
        .await
        .map_err(|status| anyhow::anyhow!("Health failed: {}", describe_status(&status)))?
        .into_inner();

    let capabilities: BTreeMap<String, bool> = reply
        .capabilities
        .iter()
        .map(|(name, enabled)| (name.clone(), *enabled))
        .collect();
    println!(
        "{}",
        format_report(
            negotiated_minor,
            &host_protocol,
            &capabilities,
            health.status()
        )
    );
    println!("host version: {}", info.host_version);
    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Version => {
            println!("lazarus-cli {}", env!("CARGO_PKG_VERSION"));
        }
        Commands::Status { addr } => {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?
                .block_on(run_status(&addr))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(major: i32, minor: i32) -> ProtocolVersion {
        ProtocolVersion { major, minor }
    }

    #[test]
    fn report_lists_negotiated_version_capabilities_and_serving_status() {
        let report = format_report(
            4,
            &version(1, 6),
            &BTreeMap::from([("pty".to_string(), true), ("containers".to_string(), false)]),
            ServingStatus::Serving,
        );
        assert!(report.contains("negotiated protocol: 1.4 (host speaks 1.6"));
        // Capabilities render deterministically regardless of map order.
        assert!(report.contains("containers=off, pty=on"));
        assert!(report.contains("host status: SERVING"));
    }

    #[test]
    fn report_marks_not_serving_and_empty_capabilities() {
        let report = format_report(
            0,
            &version(1, 0),
            &BTreeMap::new(),
            ServingStatus::NotServing,
        );
        assert!(report.contains("host status: NOT_SERVING"));
        assert!(report.contains("capabilities: (none)"));
    }

    #[test]
    fn local_major_mismatch_fails_with_clear_error() {
        let err = handshake::negotiate(&version(2, 0), &version(1, 6)).unwrap_err();
        assert_eq!(
            err.to_string(),
            "unsupported protocol version: client speaks 2.x, host speaks 1.x"
        );
    }

    #[test]
    fn grpc_failure_decodes_structured_protocol_error() {
        let negotiation_err = handshake::NegotiationError::UnsupportedMajor {
            client_major: 2,
            host_major: 1,
        };
        let status = tonic::Status::from(negotiation_err);
        let described = describe_status(&status);
        assert!(described.contains("client speaks 2.x"), "{described}");
        assert!(
            described.contains("UnsupportedProtocolVersion"),
            "{described}"
        );
    }
}
