//! Connection status between the desktop shell and the local Host.
//!
//! Probes the Host at [`HOST_ENDPOINT`]: negotiates the wire protocol
//! (plan section 9.2), then checks health and serving info. The result is
//! a plain serializable snapshot the UI can render, including a useful
//! error message when the Host is unreachable or incompatible.

use std::future::Future;
use std::time::Duration;

use protocol_rs::generated::ServingStatus;
use protocol_rs::handshake::{self, ClientHello};
use protocol_rs::{GetInfoRequest, HealthRequest, SystemServiceClient};
use serde::Serialize;

const HOST_ENDPOINT: &str = "http://127.0.0.1:50051";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const RPC_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Capability {
    pub name: String,
    pub enabled: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostStatus {
    pub connected: bool,
    pub negotiated_version: Option<String>,
    pub host_version: Option<String>,
    pub serving_status: Option<String>,
    pub capabilities: Vec<Capability>,
    pub error: Option<String>,
}

/// Tauri command returning the current Host connection snapshot. Failures
/// are reported in-band (`connected: false` plus `error`) so the UI always
/// gets a renderable status instead of a rejected promise.
#[tauri::command]
pub async fn host_status() -> HostStatus {
    match probe().await {
        Ok(status) => status,
        Err(error) => HostStatus {
            connected: false,
            negotiated_version: None,
            host_version: None,
            serving_status: None,
            capabilities: Vec::new(),
            error: Some(error),
        },
    }
}

async fn probe() -> Result<HostStatus, String> {
    let endpoint = tonic::transport::Endpoint::from_shared(HOST_ENDPOINT.to_string())
        .map_err(|error| format!("invalid host address {HOST_ENDPOINT}: {error}"))?
        .connect_timeout(CONNECT_TIMEOUT);
    let channel = endpoint.connect().await.map_err(|error| {
        format!("cannot reach host at {HOST_ENDPOINT}: {error}. Is lazarus-hostd running?")
    })?;
    let mut client = SystemServiceClient::new(channel);

    let reply = rpc("protocol negotiation", client.negotiate(client_hello())).await?;
    let host_protocol = reply.protocol.ok_or_else(|| {
        "protocol negotiation failed: host did not report a protocol version".to_string()
    })?;
    handshake::negotiate(&handshake::CURRENT_PROTOCOL_VERSION, &host_protocol)
        .map_err(|error| format!("host rejected the connection: {error}"))?;
    let negotiated_minor = reply.negotiated_minor;

    let info = rpc("fetching host info", client.get_info(GetInfoRequest {})).await?;
    let health = rpc("health check", client.health(HealthRequest {})).await?;

    let mut capabilities: Vec<Capability> = reply
        .capabilities
        .into_iter()
        .map(|(name, enabled)| Capability { name, enabled })
        .collect();
    capabilities.sort_by(|left, right| left.name.cmp(&right.name));

    Ok(HostStatus {
        connected: true,
        negotiated_version: Some(format!("{}.{}", host_protocol.major, negotiated_minor)),
        host_version: Some(info.host_version),
        serving_status: Some(serving_status_label(health.status)),
        capabilities,
        error: None,
    })
}

fn client_hello() -> ClientHello {
    ClientHello {
        client: "lazarus-desktop".to_string(),
        client_version: env!("CARGO_PKG_VERSION").to_string(),
        protocol: Some(handshake::CURRENT_PROTOCOL_VERSION),
        supported_features: Vec::new(),
        auth: None,
    }
}

async fn rpc<T>(
    context: &str,
    call: impl Future<Output = Result<tonic::Response<T>, tonic::Status>>,
) -> Result<T, String> {
    match tokio::time::timeout(RPC_TIMEOUT, call).await {
        Err(_) => Err(format!(
            "{context} timed out after {}s",
            RPC_TIMEOUT.as_secs()
        )),
        Ok(Err(status)) => Err(match handshake::error_from_status(&status) {
            Some(detail) if !detail.message.is_empty() => {
                format!("{context} failed: {}", detail.message)
            }
            Some(detail) => format!("{context} failed: {}", detail.code().as_str_name()),
            None => format!("{context} failed: {}", status.code()),
        }),
        Ok(Ok(response)) => Ok(response.into_inner()),
    }
}

fn serving_status_label(status: i32) -> String {
    match status {
        status if status == ServingStatus::Serving as i32 => "serving".to_string(),
        status if status == ServingStatus::NotServing as i32 => "not serving".to_string(),
        _ => "unspecified".to_string(),
    }
}
