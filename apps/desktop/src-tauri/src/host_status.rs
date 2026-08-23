//! Connection status between the desktop shell and the local Host.
//!
//! Probes the Host at [`HOST_ENDPOINT`] over the same authenticated
//! JSON/HTTP contract the CLI uses: every request carries the local token as
//! `Authorization: Bearer <token>` plus this client's complete per-method
//! manifest in `x-lazarus-manifest`, and every successful response must
//! advertise a decodable, compatible manifest of its own. The result is a
//! plain serializable snapshot the UI can render, including a useful error
//! message when the Host is unreachable or incompatible.

use std::time::Duration;

use protocol_rs::auth::{self, LOCAL_TOKEN_ENV, bearer_header};
use protocol_rs::generated_registry;
use protocol_rs::manifest::{
    self as manifest_contract, MethodManifest, NegotiatedManifest, Resolution,
    host_manifest_encoded, negotiate_with_host,
};
use reqwest::header::{HeaderName, HeaderValue};
use serde::Serialize;
use serde_json::Value;

const HOST_ENDPOINT: &str = "http://127.0.0.1:50051";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const RPC_TIMEOUT: Duration = Duration::from_secs(5);

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
    let token = resolve_token(std::env::var(LOCAL_TOKEN_ENV).ok().as_deref())?;
    let client = reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .build()
        .map_err(|error| format!("cannot build HTTP client: {error}"))?;

    let (info, negotiated) = fetch_json(&client, HOST_ENDPOINT, "/system/info", &token).await?;
    let (health, _) = fetch_json(&client, HOST_ENDPOINT, "/system/health", &token).await?;

    let capabilities: Vec<Capability> = parse_capabilities(&info)
        .into_iter()
        .map(|(name, enabled)| Capability { name, enabled })
        .collect();

    Ok(HostStatus {
        connected: true,
        host_version: Some(
            info["hostVersion"]
                .as_str()
                .ok_or_else(|| "host info response is missing hostVersion".to_string())?
                .to_owned(),
        ),
        serving_status: Some(
            health["status"]
                .as_str()
                .ok_or_else(|| "host health response is missing status".to_string())?
                .to_owned(),
        ),
        capabilities,
        methods: negotiated_methods(&negotiated),
        error: None,
    })
}

/// Resolves the local token from its raw environment value. Fails clearly
/// before any network activity when it is unset or empty; the token value is
/// never echoed into an error message.
fn resolve_token(raw: Option<&str>) -> Result<String, String> {
    match raw.map(str::trim) {
        None => Err(format!(
            "{LOCAL_TOKEN_ENV} is not set; start the Host with a per-install local token"
        )),
        Some("") => Err(format!("{LOCAL_TOKEN_ENV} is set but empty")),
        Some(token) => Ok(token.to_owned()),
    }
}

/// The contract headers every Host request must carry: the `Authorization`
/// Bearer token and this client's complete per-method manifest.
fn contract_headers(token: &str) -> Result<Vec<(HeaderName, HeaderValue)>, String> {
    let authorization = HeaderValue::from_str(&bearer_header(token))
        .map_err(|_| "local token contains characters invalid in an HTTP header".to_string())?;
    Ok(vec![
        (
            HeaderName::from_static(auth::AUTH_METADATA_KEY),
            authorization,
        ),
        (
            HeaderName::from_static(manifest_contract::MANIFEST_METADATA_KEY),
            HeaderValue::from_static(host_manifest_encoded()),
        ),
    ])
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

/// A typed JSON error body the Host attaches to gate rejections.
#[derive(serde::Deserialize)]
struct HostErrorBody {
    code: String,
    message: String,
}

/// Renders a failed HTTP response from the typed error body when one is
/// present, falling back to a generic status description. Never echoes
/// request secrets.
fn describe_host_error(status: u16, body: &str) -> String {
    match serde_json::from_str::<HostErrorBody>(body) {
        Ok(error) => format!(
            "host rejected the request: {} [{}]",
            error.message, error.code
        ),
        Err(_) => format!("host returned an unexpected error (HTTP {status})"),
    }
}

/// Performs one authenticated contract request and verifies the response
/// manifest before handing back the decoded JSON body.
async fn fetch_json(
    client: &reqwest::Client,
    addr: &str,
    path: &str,
    token: &str,
) -> Result<(Value, NegotiatedManifest), String> {
    let url = format!("{}{}", addr.trim_end_matches('/'), path);
    let mut request = client.get(&url).timeout(RPC_TIMEOUT);
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
        .json::<Value>()
        .await
        .map_err(|error| format!("host returned malformed JSON from {path}: {error}"))?;
    Ok((body, negotiated))
}

/// Extracts the capability map from the `/system/info` response body,
/// ignoring entries whose value is not a boolean.
fn parse_capabilities(info: &Value) -> Vec<(String, bool)> {
    let mut capabilities = info
        .get("capabilities")
        .and_then(Value::as_object)
        .map(|object| {
            object
                .iter()
                .filter_map(|(name, enabled)| {
                    enabled.as_bool().map(|enabled| (name.clone(), enabled))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    capabilities.sort_by(|left, right| left.0.cmp(&right.0));
    capabilities
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
    fn rejects_missing_or_empty_local_token_before_any_network_use() {
        assert_eq!(
            resolve_token(None).unwrap_err(),
            format!("{LOCAL_TOKEN_ENV} is not set; start the Host with a per-install local token")
        );
        assert_eq!(
            resolve_token(Some("")).unwrap_err(),
            format!("{LOCAL_TOKEN_ENV} is set but empty")
        );
        assert_eq!(
            resolve_token(Some("   ")).unwrap_err(),
            format!("{LOCAL_TOKEN_ENV} is set but empty")
        );

        let secret = "s3cret-token-value";
        let resolved = resolve_token(Some(secret)).expect("valid token");
        assert_eq!(resolved, secret);
        for error in [
            resolve_token(None).unwrap_err(),
            resolve_token(Some("")).unwrap_err(),
        ] {
            assert!(!error.contains(secret), "error must not echo the token");
        }
    }

    #[test]
    fn requests_carry_bearer_and_complete_manifest_headers() {
        let headers = contract_headers("unit-test-token").expect("valid headers");
        assert_eq!(headers.len(), 2);

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
    }

    #[test]
    fn accepts_compatible_response_manifest_and_negotiates_per_method() {
        let identical =
            verify_response_manifest(Some(host_manifest_encoded())).expect("compatible");
        assert_eq!(identical.methods.len(), 5);

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
    fn parses_info_and_health_bodies_into_status_fields() {
        let info: Value = serde_json::from_str(
            r#"{"hostVersion":"1.2.3","capabilities":{"pty":false,"events":true}}"#,
        )
        .expect("info body");
        let health: Value = serde_json::from_str(r#"{"status":"SERVING"}"#).expect("health body");

        let capabilities = parse_capabilities(&info);
        assert_eq!(
            capabilities,
            vec![("events".to_owned(), true), ("pty".to_owned(), false),]
        );

        assert_eq!(info["hostVersion"].as_str(), Some("1.2.3"));
        assert_eq!(health["status"].as_str(), Some("SERVING"));
    }

    #[test]
    fn missing_body_fields_are_in_band_errors_not_panics() {
        let empty: Value = serde_json::from_str("{}").expect("empty object");

        let probe_error = || -> Result<(), String> {
            empty["hostVersion"]
                .as_str()
                .ok_or_else(|| "host info response is missing hostVersion".to_string())?;
            Ok(())
        };
        assert_eq!(
            probe_error().unwrap_err(),
            "host info response is missing hostVersion"
        );

        let probe_health = || -> Result<(), String> {
            empty["status"]
                .as_str()
                .ok_or_else(|| "host health response is missing status".to_string())?;
            Ok(())
        };
        assert_eq!(
            probe_health().unwrap_err(),
            "host health response is missing status"
        );
    }

    #[test]
    fn typed_host_errors_render_code_and_message_without_secrets() {
        let body = r#"{"code":"UNAUTHENTICATED","message":"missing or invalid local token"}"#;
        let described = describe_host_error(401, body);
        assert!(described.contains("[UNAUTHENTICATED]"), "{described}");
        assert!(!described.contains("unit-test-token"), "{described}");

        let fallback = describe_host_error(500, "<html>not json</html>");
        assert!(fallback.contains("HTTP 500"), "{fallback}");
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
        assert_eq!(json["methods"].as_array().expect("methods array").len(), 5);
        assert_eq!(json["methods"][0]["name"], "system.getInfo");
        assert_eq!(json["methods"][0]["version"], "1.0");
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
