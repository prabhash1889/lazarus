//! `lazarus host start`: launch the daemon detached, provision its token,
//! wait for health, and record the instance.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use lazarus_hostd::runtime::DataPaths;
use protocol_rs::generated_registry::wire;

use crate::client::{contract_headers, fetch_json};
use crate::host::discovery::{self, PidRecord};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(15);
const STARTUP_POLL: Duration = Duration::from_millis(150);

#[cfg(windows)]
use std::os::windows::process::CommandExt;

/// Detach from this console so closing the terminal cannot kill the daemon,
/// and give it its own process group so Ctrl-C on the CLI is never
/// delivered to the Host.
#[cfg(windows)]
const DETACHED_PROCESS: u32 = 0x0000_0008;
#[cfg(windows)]
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

fn normalize_addr(addr: Option<String>) -> Result<String> {
    let socket = match addr {
        None => discovery::DEFAULT_LISTEN_ADDR
            .parse()
            .expect("default addr"),
        Some(raw) => raw
            .trim()
            .parse::<SocketAddr>()
            .with_context(|| format!("invalid --addr {raw}; expected ip:port"))?,
    };
    if !socket.ip().is_loopback() {
        bail!("--addr must be a loopback address; refusing {socket}");
    }
    Ok(format!("http://{socket}"))
}

pub(crate) fn resolve_daemon_binary() -> Result<PathBuf> {
    let file_name = daemon_file_name();
    if let Some(raw) = std::env::var_os("LAZARUS_HOSTD_PATH").filter(|v| !v.is_empty()) {
        let path = PathBuf::from(raw);
        if path.exists() {
            return Ok(path);
        }
        bail!(
            "LAZARUS_HOSTD_PATH points at a missing file: {}",
            path.display()
        );
    }
    if let Ok(current) = std::env::current_exe()
        && let Some(sibling) = current.parent().map(|dir| dir.join(file_name))
        && sibling.exists()
    {
        return Ok(sibling);
    }
    // Development convenience: the workspace target directory next to this
    // crate when both binaries were built together.
    for profile in ["debug", "release"] {
        let candidate = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target")
            .join(profile)
            .join(file_name);
        if candidate.exists() {
            return Ok(candidate);
        }
    }
    bail!("cannot locate {file_name}; install it next to the CLI or set LAZARUS_HOSTD_PATH")
}

fn daemon_file_name() -> &'static str {
    #[cfg(windows)]
    {
        "lazarus-hostd.exe"
    }
    #[cfg(not(windows))]
    {
        "lazarus-hostd"
    }
}

struct SpawnedDaemon {
    record: PidRecord,
}

fn spawn_daemon(paths: &DataPaths, addr: &str, token: &str) -> Result<SpawnedDaemon> {
    let binary = resolve_daemon_binary()?;
    paths.prepare()?;
    let log_path = discovery::log_path(paths);
    let stdout = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("opening {}", log_path.display()))?;
    let stderr = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("opening {}", log_path.display()))?;

    let mut command = Command::new(&binary);
    command
        .env(protocol_rs::auth::LOCAL_TOKEN_ENV, token)
        .env("LAZARUS_HOST_ADDR", addr.trim_start_matches("http://"))
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    #[cfg(windows)]
    command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);

    let child = command
        .spawn()
        .with_context(|| format!("spawning {}", binary.display()))?;
    let record = PidRecord {
        pid: child.id(),
        addr: addr.to_owned(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
    };
    drop(child);
    discovery::store_pid(paths, &record)?;
    Ok(SpawnedDaemon { record })
}

/// Waits until the daemon answers `/system/health` with SERVING. `None`
/// means the startup budget elapsed without a healthy Host.
async fn await_serving(addr: &str, token: &str) -> Option<()> {
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    while Instant::now() < deadline {
        if serving_now(addr, token).await.unwrap_or(false) {
            return Some(());
        }
        tokio::time::sleep(STARTUP_POLL).await;
    }
    None
}

async fn serving_now(addr: &str, token: &str) -> Result<bool> {
    let client = reqwest::Client::new();
    let (body, _) = fetch_json(&client, addr, "/system/health", token).await?;
    let health = wire::decode_system_health_response(&body)
        .map_err(|error| anyhow::anyhow!("health response violates the contract: {error}"))?;
    Ok(health.status == wire::SystemHealthResponseStatus::Serving)
}

/// One authenticated GET used by lifecycle probes that only need liveness,
/// not contract verification.
pub(crate) async fn probe_reachable(addr: &str, token: &str) -> bool {
    let client = reqwest::Client::new();
    let url = format!("{}/system/health", addr.trim_end_matches('/'));
    let mut request = client.get(&url).timeout(Duration::from_millis(1_500));
    if let Ok(headers) = contract_headers(token)
        && let Some((name, value)) = headers
            .into_iter()
            .find(|(name, _)| name.as_str() == protocol_rs::auth::AUTH_METADATA_KEY)
    {
        request = request.header(name, value);
    }
    request.send().await.is_ok()
}

pub async fn run(addr: Option<String>, json: bool) -> Result<()> {
    let addr = normalize_addr(addr)?;
    let paths = DataPaths::resolve().context("resolving the Lazarus data directory")?;

    if let Some(existing) = discovery::load_pid(&paths)?
        && probe_reachable(&existing.addr, "").await
    {
        report_already_running(&existing, json)?;
        return Ok(());
    }

    let token = discovery::load_or_create_token(&paths)?;
    let spawned = spawn_daemon(&paths, &addr, &token)?;

    match await_serving(&spawned.record.addr, &token).await {
        Some(()) => report_started(&spawned.record, json),
        None => {
            bail!(
                "daemon pid {} did not become healthy within {}s; inspect `lazarus host logs`",
                spawned.record.pid,
                STARTUP_TIMEOUT.as_secs()
            )
        }
    }
}

fn render(record: &PidRecord, extra: &[(&str, String)]) -> serde_json::Value {
    let mut value = serde_json::json!({
        "pid": record.pid,
        "addr": record.addr,
        "version": record.version,
    });
    for (key, detail) in extra {
        value[key] = serde_json::Value::String(detail.clone());
    }
    value
}

fn report_started(record: &PidRecord, json: bool) -> Result<()> {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&render(record, &[("state", "started".into())]))?
        );
    } else {
        println!(
            "Host started: {} (pid {}, version {})",
            record.addr, record.pid, record.version
        );
    }
    Ok(())
}

fn report_already_running(record: &PidRecord, json: bool) -> Result<()> {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&render(record, &[("state", "already-running".into())]))?
        );
    } else {
        println!("Host already running: {} (pid {})", record.addr, record.pid);
    }
    Ok(())
}
