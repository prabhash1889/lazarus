use std::collections::BTreeMap;
use std::net::IpAddr;

use anyhow::{Context, Result, anyhow, bail};
use clap::{Parser, Subcommand};
use protocol_rs::auth::{self, LOCAL_TOKEN_ENV, bearer_header};
use protocol_rs::generated_registry;
use protocol_rs::manifest::{
    self as manifest_contract, MethodManifest, NegotiatedManifest, Resolution,
    host_manifest_encoded, negotiate_with_host,
};
use reqwest::header::{HeaderName, HeaderValue};

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
    /// Connect to the Host over the authenticated JSON contract and report
    /// serving status with the negotiated per-method versions.
    Status {
        /// Address of the running Host.
        #[arg(long, default_value = "http://127.0.0.1:50051")]
        addr: String,
    },
}

/// Validates that `--addr` names a loopback HTTP destination before any token
/// is resolved or network activity begins. The token is never involved here.
fn validate_loopback_addr(addr: &str) -> Result<()> {
    let url = reqwest::Url::parse(addr).map_err(|err| anyhow!("invalid --addr {addr}: {err}"))?;
    if url.scheme() != "http" {
        bail!("--addr must use http; refusing non-http destination {addr}");
    }
    if !url.username().is_empty() || url.password().is_some() {
        bail!("--addr must not embed credentials; refusing {addr}");
    }

    let loopback = match url.host_str() {
        Some(host) => match host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<IpAddr>()
        {
            Ok(ip) => ip.is_loopback(),
            Err(_) => host.eq_ignore_ascii_case("localhost"),
        },
        None => false,
    };
    if !loopback {
        bail!(
            "--addr must point at a loopback address (127.0.0.1, [::1], localhost); refusing {addr}"
        );
    }
    Ok(())
}

/// Resolves the local token from its raw environment value. Fails clearly
/// before any network activity when it is unset or empty; the token value is
/// never echoed into an error message.
fn resolve_token(raw: Option<&str>) -> Result<String> {
    match raw.map(str::trim) {
        None => {
            bail!("{LOCAL_TOKEN_ENV} is not set; start the Host with a per-install local token")
        }
        Some("") => bail!("{LOCAL_TOKEN_ENV} is set but empty"),
        Some(token) => Ok(token.to_owned()),
    }
}

fn local_token() -> Result<String> {
    resolve_token(std::env::var(LOCAL_TOKEN_ENV).ok().as_deref())
}

/// The contract headers every Host request must carry: the `Authorization`
/// Bearer token and this client's complete per-method manifest.
fn contract_headers(token: &str) -> Result<Vec<(HeaderName, HeaderValue)>> {
    let authorization = HeaderValue::from_str(&bearer_header(token))
        .map_err(|_| anyhow!("local token contains characters invalid in an HTTP header"))?;
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
fn verify_response_manifest(raw: Option<&str>) -> Result<NegotiatedManifest> {
    let Some(raw) = raw else {
        bail!(
            "host response is missing the {} manifest header",
            manifest_contract::MANIFEST_METADATA_KEY
        )
    };
    let peer: MethodManifest = raw
        .parse()
        .map_err(|err| anyhow!("host advertised an undecodable method manifest: {err}"))?;
    negotiate_with_host(&peer)
        .map_err(|err| anyhow!("host manifest is incompatible with this client: {err}"))
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
        Ok(err) => format!("host rejected the request: {} [{}]", err.message, err.code),
        Err(_) => format!("host returned an unexpected error (HTTP {status})"),
    }
}

/// Renders the post-negotiation report as plain text so it can be unit
/// tested without a live connection.
fn format_report(
    host_version: &str,
    serving_status: &str,
    capabilities: &BTreeMap<String, bool>,
    negotiated: &NegotiatedManifest,
) -> String {
    let mut lines = vec![format!("host version: {host_version}")];

    let serving = serving_status.eq_ignore_ascii_case("SERVING");
    lines.push(format!(
        "host status: {}",
        if serving { "SERVING" } else { "NOT_SERVING" }
    ));

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

    let rendered_methods = negotiated
        .methods
        .iter()
        .map(|(name, resolution)| format_method_resolution(name, resolution))
        .collect::<Vec<_>>()
        .join(", ");
    lines.push(format!("negotiated methods: {rendered_methods}"));

    lines.join("\n")
}

fn format_method_resolution(name: &str, resolution: &Resolution) -> String {
    match resolution {
        Resolution::Supported { minor } => {
            let major = generated_registry::binding_by_name(name)
                .map(|binding| binding.major)
                .unwrap_or(0);
            format!("{name}={major}.{minor}")
        }
        Resolution::Fallback { fallback } => format!("{name}=>{fallback} (fallback)"),
        Resolution::Unsupported => format!("{name}=unavailable"),
    }
}

/// Performs one authenticated contract request and verifies the response
/// manifest before handing back the decoded JSON body.
async fn fetch_json(
    client: &reqwest::Client,
    addr: &str,
    path: &str,
    token: &str,
) -> Result<(serde_json::Value, NegotiatedManifest)> {
    let url = format!("{}{}", addr.trim_end_matches('/'), path);
    let mut request = client.get(&url);
    for (name, value) in contract_headers(token)? {
        request = request.header(name, value);
    }

    let response = request
        .send()
        .await
        .with_context(|| format!("cannot reach host at {addr}; is lazarus-hostd running?"))?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        bail!("{}", describe_host_error(status.as_u16(), &body));
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
        .with_context(|| format!("host returned malformed JSON from {path}"))?;
    Ok((body, negotiated))
}

async fn run_status(addr: &str) -> Result<()> {
    validate_loopback_addr(addr)?;
    let token = local_token()?;
    let client = reqwest::Client::new();

    let (info, negotiated) = fetch_json(&client, addr, "/system/info", &token).await?;
    let (health, _) = fetch_json(&client, addr, "/system/health", &token).await?;

    let host_version = info["hostVersion"]
        .as_str()
        .context("host info response is missing hostVersion")?
        .to_owned();
    let capabilities = parse_capabilities(&info);
    let serving_status = health["status"]
        .as_str()
        .context("host health response is missing status")?
        .to_owned();

    println!(
        "{}",
        format_report(&host_version, &serving_status, &capabilities, &negotiated)
    );
    Ok(())
}

fn parse_capabilities(info: &serde_json::Value) -> BTreeMap<String, bool> {
    info.get("capabilities")
        .and_then(|value| value.as_object())
        .map(|object| {
            object
                .iter()
                .filter_map(|(name, enabled)| enabled.as_bool().map(|b| (name.clone(), b)))
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default()
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
    use protocol_rs::manifest::host_manifest;

    fn negotiated_from(peer: &MethodManifest) -> NegotiatedManifest {
        negotiate_with_host(peer).expect("compatible manifests")
    }

    #[test]
    fn report_lists_version_status_capabilities_and_per_method_versions() {
        let negotiated = negotiated_from(&host_manifest());
        assert_eq!(negotiated.methods.len(), 5);

        let report = format_report(
            "1.2.3",
            "SERVING",
            &BTreeMap::from([("events".to_string(), true), ("pty".to_string(), false)]),
            &negotiated,
        );
        assert!(report.contains("host version: 1.2.3"), "{report}");
        assert!(report.contains("host status: SERVING"), "{report}");
        // Capabilities render deterministically regardless of map order.
        assert!(
            report.contains("capabilities: events=on, pty=off"),
            "{report}"
        );
        for (name, version) in host_manifest().iter() {
            assert!(
                report.contains(&format!("{name}={}.{}", version.major, version.minor)),
                "{report}"
            );
        }
        assert!(!report.contains("negotiated protocol"), "{report}");
        assert!(!report.contains("negotiated minor"), "{report}");
    }

    #[test]
    fn report_marks_not_serving_and_empty_capabilities() {
        let report = format_report(
            "0.1.0",
            "NOT_SERVING",
            &BTreeMap::new(),
            &negotiated_from(&host_manifest()),
        );
        assert!(report.contains("host status: NOT_SERVING"), "{report}");
        assert!(report.contains("capabilities: (none)"), "{report}");
    }

    #[test]
    fn rejects_missing_or_empty_local_token_before_any_network_use() {
        assert_eq!(
            resolve_token(None).unwrap_err().to_string(),
            format!("{LOCAL_TOKEN_ENV} is not set; start the Host with a per-install local token")
        );
        assert_eq!(
            resolve_token(Some("")).unwrap_err().to_string(),
            format!("{LOCAL_TOKEN_ENV} is set but empty")
        );
        assert_eq!(
            resolve_token(Some("   ")).unwrap_err().to_string(),
            format!("{LOCAL_TOKEN_ENV} is set but empty")
        );

        let secret = "s3cret-token-value";
        let resolved = resolve_token(Some(secret)).expect("valid token");
        assert_eq!(resolved, secret);
        for err in [
            resolve_token(None).unwrap_err().to_string(),
            resolve_token(Some("")).unwrap_err().to_string(),
        ] {
            assert!(!err.contains(secret), "error must not echo the token");
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
        let identical = verify_response_manifest(Some(host_manifest_encoded()))
            .expect("identical manifest is compatible");
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
            missing
                .to_string()
                .contains(manifest_contract::MANIFEST_METADATA_KEY),
            "{missing}"
        );

        for bad in ["garbage", "v1:not-an-entry", "v1:a.method=1.0,a.method=2.0"] {
            let err = verify_response_manifest(Some(bad)).unwrap_err();
            assert!(
                err.to_string().contains("undecodable"),
                "{bad:?} must fail to decode: {err}"
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
        let err = verify_response_manifest(Some(&incompatible.to_string())).unwrap_err();
        assert!(
            err.to_string().contains("workspace.list"),
            "incompatibility names the offender: {err}"
        );
    }

    #[test]
    fn accepts_loopback_addresses_with_any_port_or_path() {
        for addr in [
            "http://127.0.0.1:50051",
            "http://127.0.0.1",
            "http://127.0.0.1:1",
            "http://[::1]:50051",
            "http://localhost:50051/",
            "http://LOCALHOST:8080",
        ] {
            validate_loopback_addr(addr).unwrap_or_else(|err| panic!("{addr} must pass: {err}"));
        }
    }

    #[test]
    fn rejects_non_http_schemes_credentials_and_non_loopback_hosts() {
        let rejected = [
            "https://127.0.0.1:50051",
            "ftp://127.0.0.1:50051",
            "http://user@127.0.0.1:50051",
            "http://user:pass@localhost",
            "http://example.com",
            "http://192.168.1.10:50051",
            "http://0.0.0.0:50051",
            "http://[fe80::1]:50051",
            "127.0.0.1:50051",
            "not a url",
            "http://",
            "",
        ];
        for addr in rejected {
            let err = validate_loopback_addr(addr).expect_err(&format!("{addr} must be refused"));
            assert!(
                !err.to_string().is_empty() && !addr.contains("token"),
                "{addr:?} rejection must stay informative: {err}"
            );
        }
    }

    #[test]
    fn typed_host_errors_render_code_and_message_without_secrets() {
        let body = r#"{"code":"INCOMPATIBLE_METHOD_MANIFEST","message":"required method \"task.list\" missing from peer manifest"}"#;
        let described = describe_host_error(412, body);
        assert!(
            described.contains("[INCOMPATIBLE_METHOD_MANIFEST]"),
            "{described}"
        );
        assert!(described.contains("task.list"), "{described}");

        let unauthenticated = describe_host_error(
            401,
            r#"{"code":"UNAUTHENTICATED","message":"missing or invalid local token"}"#,
        );
        assert!(
            unauthenticated.contains("[UNAUTHENTICATED]"),
            "{unauthenticated}"
        );

        let fallback = describe_host_error(500, "<html>not json</html>");
        assert!(fallback.contains("HTTP 500"), "{fallback}");
    }
}
