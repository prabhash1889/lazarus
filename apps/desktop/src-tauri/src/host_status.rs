//! Connection status between the desktop shell and the local Host.
//!
//! Probes the Host at [`HOST_ENDPOINT`] over the same authenticated
//! JSON/HTTP contract the CLI uses: every request carries the local token as
//! `Authorization: Bearer <token>`, this client's complete per-method
//! manifest in `x-lazarus-manifest`, and its cancellation deadline in
//! `x-lazarus-deadline`; every successful response must advertise a
//! decodable, compatible manifest of its own, and every body is decoded
//! through the generated wire bindings before use. The result is a plain
//! serializable snapshot the UI can render, including a useful error message
//! when the Host is unreachable or incompatible.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use protocol_rs::auth::{self, LOCAL_TOKEN_ENV, bearer_header};
use protocol_rs::deadline::{self, CLIENT_TIMEOUT_GRACE_MS, DEFAULT_RPC_BUDGET_MS, Deadline};
use protocol_rs::generated_registry;
use protocol_rs::generated_registry::wire;
use protocol_rs::manifest::{
    self as manifest_contract, MethodManifest, NegotiatedManifest, Resolution,
    host_manifest_encoded, negotiate_with_host,
};
use reqwest::header::{HeaderName, HeaderValue};
use serde::Serialize;

const HOST_ENDPOINT: &str = "http://127.0.0.1:50051";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Capability {
    pub name: String,
    pub enabled: bool,
}

/// One protocol method's negotiated outcome: served at a common version,
/// degraded to a fallback method, or unavailable.
#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NegotiatedMethod {
    pub name: String,
    /// `major.minor` when both sides serve the method.
    pub version: Option<String>,
    /// The substitute method serving in place of an optional method.
    pub fallback: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostStatus {
    pub connected: bool,
    pub host_version: Option<String>,
    pub serving_status: Option<String>,
    pub capabilities: Vec<Capability>,
    pub methods: Vec<NegotiatedMethod>,
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
            host_version: None,
            serving_status: None,
            capabilities: Vec::new(),
            methods: Vec::new(),
            error: Some(error),
        },
    }
}

async fn probe() -> Result<HostStatus, String> {
    let token = local_token()?;
    let client = reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .build()
        .map_err(|error| format!("cannot build HTTP client: {error}"))?;

    let (info_raw, negotiated) = fetch_json(&client, HOST_ENDPOINT, "/system/info", &token).await?;
    let (health_raw, _) = fetch_json(&client, HOST_ENDPOINT, "/system/health", &token).await?;

    // Both bodies are decoded through the generated bindings before use:
    // an off-contract payload fails here instead of misreporting status.
    let info = wire::decode_system_get_info_response(&info_raw)
        .map_err(|error| format!("host info response violates the contract: {error}"))?;
    let health = wire::decode_system_health_response(&health_raw)
        .map_err(|error| format!("host health response violates the contract: {error}"))?;
    let capabilities: Vec<Capability> = info
        .capabilities
        .into_iter()
        .collect::<BTreeMap<_, _>>()
        .into_iter()
        .map(|(name, enabled)| Capability { name, enabled })
        .collect();

    Ok(HostStatus {
        connected: true,
        host_version: Some(info.host_version),
        serving_status: Some(health.status.as_str().to_owned()),
        capabilities,
        methods: negotiated_methods(&negotiated),
        error: None,
    })
}

/// Resolves the local token: the raw environment value when present, else
/// the per-install token file `lazarus host start` provisions under the
/// data root. Fails clearly before any network activity when neither is
/// available; the token value is never echoed into an error message.
fn local_token() -> Result<String, String> {
    let from_env = std::env::var(LOCAL_TOKEN_ENV).ok();
    let from_file = data_root()
        .ok()
        .and_then(|root| std::fs::read_to_string(root.join("auth").join("local-token")).ok());
    choose_token(from_env.as_deref(), from_file.as_deref())
}

/// Pure token selection over the two provision sources, so every branch is
/// unit-testable without touching the real environment or file system.
fn choose_token(from_env: Option<&str>, from_file: Option<&str>) -> Result<String, String> {
    match from_env.map(str::trim) {
        Some("") => Err(format!("{LOCAL_TOKEN_ENV} is set but empty")),
        Some(token) => Ok(token.to_owned()),
        None => match from_file.map(str::trim) {
            None => Err("no local token available; run `lazarus host start` first".to_owned()),
            Some("") => Err(
                "the per-install token file exists but is empty; run `lazarus host start` to re-provision"
                    .to_owned(),
            ),
            Some(token) => Ok(token.to_owned()),
        },
    }
}

/// The Lazarus data root: `LAZARUS_DATA_DIR` when set, else the user's home.
fn data_root() -> Result<PathBuf, String> {
    if let Some(root) = std::env::var_os("LAZARUS_DATA_DIR").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(root));
    }
    for key in ["USERPROFILE", "HOME"] {
        if let Some(home) = std::env::var_os(key).filter(|v| !v.is_empty()) {
            return Ok(PathBuf::from(home).join(".lazarus"));
        }
    }
    Err("cannot resolve the Lazarus data root (no LAZARUS_DATA_DIR or home directory)".to_owned())
}

/// The contract headers every Host request must carry: the `Authorization`
/// Bearer token, this client's complete per-method manifest, and the
/// caller's cancellation deadline (the shared default budget).
fn contract_headers(token: &str) -> Result<Vec<(HeaderName, HeaderValue)>, String> {
    let authorization = HeaderValue::from_str(&bearer_header(token))
        .map_err(|_| "local token contains characters invalid in an HTTP header".to_string())?;
    let deadline = HeaderValue::from_str(&Deadline::header_from_budget(
        deadline::unix_now_ms(),
        DEFAULT_RPC_BUDGET_MS,
    ))
    .map_err(|_| "deadline header value is not a valid HTTP header".to_string())?;
    Ok(vec![
        (
            HeaderName::from_static(auth::AUTH_METADATA_KEY),
            authorization,
        ),
        (
            HeaderName::from_static(manifest_contract::MANIFEST_METADATA_KEY),
            HeaderValue::from_static(host_manifest_encoded()),
        ),
        (HeaderName::from_static(deadline::DEADLINE_HEADER), deadline),
    ])
}

/// The local transport timeout matching the stamped deadline plus a small
/// grace, so the Host's typed `DEADLINE_EXCEEDED` wins the race against a
/// client-side abort and callers see the canonical error.
fn client_timeout() -> Duration {
    Duration::from_millis(DEFAULT_RPC_BUDGET_MS + CLIENT_TIMEOUT_GRACE_MS)
}

/// Verifies that a successful response advertises a decodable manifest
/// compatible with this client's protocol bindings, returning the per-method
/// negotiation outcome.
fn verify_response_manifest(raw: Option<&str>) -> Result<NegotiatedManifest, String> {
    let Some(raw) = raw else {
        return Err(format!(
            "host response is missing the {} manifest header",
            manifest_contract::MANIFEST_METADATA_KEY
        ));
    };
    let peer: MethodManifest = raw
        .parse()
        .map_err(|error| format!("host advertised an undecodable method manifest: {error}"))?;
    negotiate_with_host(&peer)
        .map_err(|error| format!("host manifest is incompatible with this client: {error}"))
}

/// Renders a failed HTTP response from the generated error envelope when
/// one is present (naming retryability per the canonical classification),
/// falling back to a generic status description. Never echoes request
/// secrets.
fn describe_host_error(status: u16, body: &str) -> String {
    let decoded = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|value| wire::decode_protocol_error(&value).ok());
    match decoded.filter(|error| error.retryable == error.code.is_retryable()) {
        Some(error) => format!(
            "host rejected the request: {} [{}]{}",
            error.message,
            error.code.as_str(),
            if error.retryable { " (retryable)" } else { "" }
        ),
        None => format!("host returned an unexpected error (HTTP {status})"),
    }
}

/// Performs one authenticated contract request and verifies the response
/// manifest before handing back the decoded JSON body. The request carries
/// the caller deadline and a matching local transport timeout.
async fn fetch_json(
    client: &reqwest::Client,
    addr: &str,
    path: &str,
    token: &str,
) -> Result<(serde_json::Value, NegotiatedManifest), String> {
    let url = format!("{}{}", addr.trim_end_matches('/'), path);
    let mut request = client.get(&url).timeout(client_timeout());
    for (name, value) in contract_headers(token)? {
        request = request.header(name, value);
    }

    let response = request.send().await.map_err(|error| {
        format!("cannot reach host at {addr}: {error}. Is lazarus-hostd running?")
    })?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(describe_host_error(status.as_u16(), &body));
    }

    let negotiated = verify_response_manifest(
        response
            .headers()
            .get(manifest_contract::MANIFEST_METADATA_KEY)
            .and_then(|value| value.to_str().ok()),
    )?;

    let body = response
        .json::<serde_json::Value>()
        .await
        .map_err(|error| format!("host returned malformed JSON from {path}: {error}"))?;
    Ok((body, negotiated))
}

/// Projects the negotiation outcome onto serializable UI entries.
fn negotiated_methods(negotiated: &NegotiatedManifest) -> Vec<NegotiatedMethod> {
    negotiated
        .methods
        .iter()
        .map(|(name, resolution)| match resolution {
            Resolution::Supported { minor } => {
                let major = generated_registry::binding_by_name(name)
                    .map(|binding| binding.major)
                    .unwrap_or(0);
                NegotiatedMethod {
                    name: name.clone(),
                    version: Some(format!("{major}.{minor}")),
                    fallback: None,
                }
            }
            Resolution::Fallback { fallback } => NegotiatedMethod {
                name: name.clone(),
                version: None,
                fallback: Some((*fallback).to_string()),
            },
            Resolution::Unsupported => NegotiatedMethod {
                name: name.clone(),
                version: None,
                fallback: None,
            },
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol_rs::manifest::{MethodManifest, host_manifest};

    fn negotiated_from(peer: &MethodManifest) -> NegotiatedManifest {
        negotiate_with_host(peer).expect("compatible manifests")
    }

    #[test]
    fn token_selection_prefers_env_then_file_and_never_echoes_secrets() {
        assert_eq!(
            choose_token(None, None).unwrap_err(),
            "no local token available; run `lazarus host start` first"
        );
        assert_eq!(
            choose_token(Some("  "), None).unwrap_err(),
            format!("{LOCAL_TOKEN_ENV} is set but empty")
        );
        assert_eq!(
            choose_token(None, Some("   ")).unwrap_err(),
            "the per-install token file exists but is empty; run `lazarus host start` to re-provision"
        );

        let secret = "s3cret-token-value";
        assert_eq!(
            choose_token(Some("  s3cret-token-value  "), None).expect("env"),
            secret
        );
        assert_eq!(choose_token(None, Some(secret)).expect("file"), secret);
        // A non-empty env value wins over the file.
        assert_eq!(
            choose_token(Some("env-token"), Some("file-token")).expect("both"),
            "env-token"
        );
    }

    #[test]
    fn requests_carry_bearer_manifest_and_deadline_headers() {
        let headers = contract_headers("unit-test-token").expect("valid headers");
        assert_eq!(headers.len(), 3);

        let authorization = headers
            .iter()
            .find(|(name, _)| name.as_str() == auth::AUTH_METADATA_KEY)
            .expect("authorization header");
        assert_eq!(
            authorization.1.to_str().expect("ascii"),
            bearer_header("unit-test-token")
        );

        let manifest_header = headers
            .iter()
            .find(|(name, _)| name.as_str() == manifest_contract::MANIFEST_METADATA_KEY)
            .expect("manifest header");
        let sent: MethodManifest = manifest_header
            .1
            .to_str()
            .expect("ascii")
            .parse()
            .expect("decodable manifest");
        assert_eq!(sent, host_manifest(), "manifest must be complete");

        // The deadline is a future epoch timestamp within the shared budget,
        // and the local timeout matches it (plus the receive grace) - the
        // same contract the CLI uses.
        let deadline_header = headers
            .iter()
            .find(|(name, _)| name.as_str() == deadline::DEADLINE_HEADER)
            .expect("deadline header");
        let parsed = Deadline::parse(
            deadline_header.1.to_str().expect("ascii"),
            deadline::unix_now_ms(),
        )
        .expect("future deadline");
        let remaining = parsed.remaining_ms(deadline::unix_now_ms());
        assert!(
            remaining <= DEFAULT_RPC_BUDGET_MS && remaining > DEFAULT_RPC_BUDGET_MS / 2,
            "deadline stamps the shared budget: {remaining}"
        );
        assert_eq!(
            client_timeout(),
            Duration::from_millis(DEFAULT_RPC_BUDGET_MS + CLIENT_TIMEOUT_GRACE_MS)
        );
    }

    #[test]
    fn accepts_compatible_response_manifest_and_negotiates_per_method() {
        let identical =
            verify_response_manifest(Some(host_manifest_encoded())).expect("compatible");
        assert_eq!(identical.methods.len(), host_manifest().len());

        // Newer minors on the host side clamp down to the shared floor.
        let mut newer = MethodManifest::default();
        for (name, version) in host_manifest().iter() {
            newer
                .try_insert(name.clone(), version.major, version.minor + 4)
                .expect("unique method");
        }
        let clamped = verify_response_manifest(Some(&newer.to_string())).expect("compatible");
        for (name, resolution) in &clamped.methods {
            let minor = host_manifest().get(name).expect("host method").minor;
            assert_eq!(
                resolution,
                &Resolution::Supported { minor },
                "{name} clamps to the shared minor"
            );
        }
    }

    #[test]
    fn rejects_missing_malformed_or_incompatible_response_manifests() {
        let missing = verify_response_manifest(None).unwrap_err();
        assert!(
            missing.contains(manifest_contract::MANIFEST_METADATA_KEY),
            "{missing}"
        );

        for bad in ["garbage", "v1:not-an-entry", "v1:a.method=1.0,a.method=2.0"] {
            let error = verify_response_manifest(Some(bad)).unwrap_err();
            assert!(
                error.contains("undecodable"),
                "{bad:?} must fail to decode: {error}"
            );
        }

        let mut incompatible = MethodManifest::default();
        for (name, version) in host_manifest().iter() {
            incompatible
                .try_insert(
                    name.clone(),
                    if name == "workspace.list" {
                        2
                    } else {
                        version.major
                    },
                    version.minor,
                )
                .expect("unique method");
        }
        let error = verify_response_manifest(Some(&incompatible.to_string())).unwrap_err();
        assert!(
            error.contains("workspace.list"),
            "incompatibility names the offender: {error}"
        );
    }

    #[test]
    fn parses_info_and_health_bodies_through_the_generated_bindings() {
        let info: serde_json::Value = serde_json::from_str(
            r#"{"hostVersion":"1.2.3","capabilities":{"pty":false,"events":true}}"#,
        )
        .expect("info body");
        let health: serde_json::Value =
            serde_json::from_str(r#"{"status":"SERVING"}"#).expect("health body");

        let decoded_info = wire::decode_system_get_info_response(&info).expect("on-contract");
        let mut capabilities: Vec<(String, bool)> = decoded_info.capabilities.into_iter().collect();
        capabilities.sort_by(|left, right| left.0.cmp(&right.0));
        assert_eq!(
            capabilities,
            vec![("events".to_owned(), true), ("pty".to_owned(), false),]
        );

        let decoded_health = wire::decode_system_health_response(&health).expect("on-contract");
        assert_eq!(decoded_health.status.as_str(), "SERVING");

        // Unknown additive fields are tolerated; wrong shapes are not.
        assert!(
            wire::decode_system_health_response(&serde_json::json!({
                "status": "SERVING",
                "detail": "future field",
            }))
            .is_ok()
        );
        assert!(wire::decode_system_health_response(&serde_json::json!({})).is_err());
    }

    #[test]
    fn typed_host_errors_render_code_retryability_and_message_without_secrets() {
        let body = r#"{"code":"UNAUTHENTICATED","message":"missing or invalid local token","retryable":false}"#;
        let described = describe_host_error(401, body);
        assert!(described.contains("[UNAUTHENTICATED]"), "{described}");
        assert!(!described.contains("unit-test-token"), "{described}");
        assert!(
            !described.contains("retryable"),
            "terminal stays unlabeled: {described}"
        );

        // A retryable envelope (e.g. a busy Host) says so explicitly.
        let unavailable = describe_host_error(
            503,
            r#"{"code":"UNAVAILABLE","message":"host is starting up","retryable":true}"#,
        );
        assert!(unavailable.contains("(retryable)"), "{unavailable}");

        let fallback = describe_host_error(500, "<html>not json</html>");
        assert!(fallback.contains("HTTP 500"), "{fallback}");

        // Off-contract error bodies fall back to the generic rendering
        // instead of being trusted.
        let off_contract = describe_host_error(500, r#"{"code":"INTERNAL"}"#);
        assert!(off_contract.contains("HTTP 500"), "{off_contract}");
        let inconsistent = describe_host_error(
            499,
            r#"{"code":"CANCELLED","message":"stopped","retryable":true}"#,
        );
        assert!(inconsistent.contains("HTTP 499"), "{inconsistent}");
    }

    #[test]
    fn status_shape_is_serializable_without_a_global_negotiated_version() {
        let connected = HostStatus {
            connected: true,
            host_version: Some("1.2.3".to_owned()),
            serving_status: Some("SERVING".to_owned()),
            capabilities: vec![Capability {
                name: "events".to_owned(),
                enabled: true,
            }],
            methods: negotiated_methods(&negotiated_from(&host_manifest())),
            error: None,
        };
        let json = serde_json::to_value(&connected).expect("serialize");
        assert_eq!(json["connected"], serde_json::Value::Bool(true));
        assert_eq!(json["hostVersion"], "1.2.3");
        assert_eq!(json["servingStatus"], "SERVING");
        assert_eq!(json["capabilities"][0]["name"], "events");
        assert_eq!(json["capabilities"][0]["enabled"], true);
        let methods = json["methods"].as_array().expect("methods array");
        assert_eq!(methods.len(), host_manifest().len());
        // Method order follows the manifest map; locate by name instead of
        // assuming a stable index.
        let info = methods
            .iter()
            .find(|method| method["name"] == "system.getInfo")
            .expect("system.getInfo is reported");
        assert_eq!(info["version"], "1.1");
        assert!(json.get("negotiatedVersion").is_none());

        let failed = HostStatus {
            connected: false,
            host_version: None,
            serving_status: None,
            capabilities: Vec::new(),
            methods: Vec::new(),
            error: Some("cannot reach host".to_owned()),
        };
        let json = serde_json::to_value(&failed).expect("serialize");
        assert_eq!(json["connected"], false);
        assert_eq!(json["error"], "cannot reach host");
    }
}
