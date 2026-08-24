//! `lazarus host status`: reachability plus the negotiated contract report.

use std::collections::BTreeMap;

use anyhow::Result;
use lazarus_hostd::runtime::DataPaths;
use protocol_rs::generated_registry::wire;

use crate::client::{fetch_json, format_method_resolution, local_token};
use crate::host::discovery::{self};

#[allow(clippy::too_many_arguments)]
fn status_report(
    connected: bool,
    addr: &str,
    pid: Option<u32>,
    host_version: Option<&str>,
    serving_status: Option<&str>,
    capabilities: &BTreeMap<String, bool>,
    methods: &[String],
    error: Option<&str>,
) -> serde_json::Value {
    serde_json::json!({
        "connected": connected,
        "addr": addr,
        "pid": pid,
        "hostVersion": host_version,
        "servingStatus": serving_status,
        "capabilities": capabilities,
        "methods": methods,
        "error": error,
    })
}

pub async fn run(addr: Option<String>, json: bool) -> Result<()> {
    let paths = DataPaths::resolve()?;
    let record = discovery::load_pid(&paths)?;
    let target = match addr {
        Some(raw) => {
            let raw = raw.trim().to_owned();
            let target = if raw.starts_with("http://") {
                raw
            } else {
                format!("http://{}", raw.trim_start_matches("http://"))
            };
            crate::client::validate_loopback_addr(&target)?;
            target
        }
        None => record
            .as_ref()
            .map(|record| record.addr.clone())
            .unwrap_or_else(|| format!("http://{}", discovery::DEFAULT_LISTEN_ADDR)),
    };
    let token = local_token(&paths)?;

    let client = reqwest::Client::new();
    match collect(&client, &target, &token).await {
        Ok((info, health, negotiated)) => {
            let host_version = info.host_version.clone();
            let capabilities: BTreeMap<String, bool> = info
                .capabilities
                .iter()
                .map(|(k, v)| (k.clone(), *v))
                .collect();
            if json {
                let methods = negotiated
                    .methods
                    .iter()
                    .map(|(name, resolution)| format_method_resolution(name, resolution))
                    .collect::<Vec<_>>();
                println!(
                    "{}",
                    serde_json::to_string_pretty(&status_report(
                        true,
                        &target,
                        record.as_ref().map(|r| r.pid),
                        Some(&host_version),
                        Some(health.status.as_str()),
                        &capabilities,
                        &methods,
                        None,
                    ))?
                );
            } else {
                println!(
                    "{}",
                    crate::client::format_report(
                        &host_version,
                        health.status.as_str(),
                        &capabilities,
                        &negotiated,
                    )
                );
            }
        }
        Err(error) => {
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&status_report(
                        false,
                        &target,
                        record.as_ref().map(|r| r.pid),
                        None,
                        None,
                        &BTreeMap::new(),
                        &[],
                        Some(&error.to_string()),
                    ))?
                );
            } else {
                println!("Host not reachable at {target}: {error}");
            }
        }
    }
    Ok(())
}

type Collected = (
    wire::SystemGetInfoResponse,
    wire::SystemHealthResponse,
    protocol_rs::manifest::NegotiatedManifest,
);

async fn collect(client: &reqwest::Client, target: &str, token: &str) -> anyhow::Result<Collected> {
    let (info_raw, negotiated) = fetch_json(client, target, "/system/info", token).await?;
    let (health_raw, _) = fetch_json(client, target, "/system/health", token).await?;
    // Both bodies are decoded through the generated bindings before use:
    // an off-contract payload fails here instead of misreporting status.
    let info = wire::decode_system_get_info_response(&info_raw)
        .map_err(|error| anyhow::anyhow!("host info response violates the contract: {error}"))?;
    let health = wire::decode_system_health_response(&health_raw)
        .map_err(|error| anyhow::anyhow!("health response violates the contract: {error}"))?;
    Ok((info, health, negotiated))
}
