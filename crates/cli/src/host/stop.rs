//! `lazarus host stop`: graceful drain first, force-terminate second.

use std::time::Duration;

use anyhow::{Context, Result};
use lazarus_hostd::runtime::DataPaths;

use crate::client::{local_token, post};
use crate::host::discovery;
use crate::host::start::probe_reachable;

const DRAIN_TIMEOUT: Duration = Duration::from_secs(10);
const DRAIN_POLL: Duration = Duration::from_millis(200);
const FORCE_KILL_GRACE: Duration = Duration::from_secs(5);

pub async fn run(json: bool) -> Result<()> {
    let paths = DataPaths::resolve().context("resolving the Lazarus data directory")?;
    let Some(record) = discovery::load_pid(&paths)? else {
        stop_unrecorded_instance(&paths, json).await?;
        return Ok(());
    };

    let token = local_token(&paths).ok();
    let _ = request_shutdown(&record.addr, token.as_deref()).await;

    if wait_until_gone(&record.addr, DRAIN_TIMEOUT).await {
        discovery::clear_pid(&paths)?;
        report_stopped(&format!("drained gracefully (pid {})", record.pid), json)?;
        return Ok(());
    }

    terminate(record.pid)?;
    let forced = wait_until_gone(&record.addr, FORCE_KILL_GRACE).await;
    discovery::clear_pid(&paths)?;
    if forced {
        report_stopped(&format!("force-terminated (pid {})", record.pid), json)?;
        Ok(())
    } else {
        Err(anyhow::anyhow!(
            "pid {} did not exit even after a force-terminate",
            record.pid
        ))
    }
}

/// Handles a Host that is answering without a recorded instance (started
/// outside this CLI, or the record was lost): drain it if we can
/// authenticate to it on the default address, otherwise report nothing to
/// do.
async fn stop_unrecorded_instance(paths: &DataPaths, json: bool) -> Result<()> {
    let token = local_token(paths).ok();
    let default_addr = format!("http://{}", discovery::DEFAULT_LISTEN_ADDR);
    if let Some(token) = token.as_deref()
        && probe_reachable(&default_addr, token).await
    {
        let _ = request_shutdown(&default_addr, Some(token)).await;
        if wait_until_gone(&default_addr, DRAIN_TIMEOUT).await {
            report_stopped("unrecorded instance on the default address drained", json)?;
            return Ok(());
        }
        anyhow::bail!(
            "an unrecorded Host answers {default_addr} but did not drain; inspect `lazarus host logs`"
        );
    }
    report_stopped("no recorded Host instance", json)
}

/// Asks the Host to drain through its authenticated lifecycle control.
/// Transport failures are expected once the listener closes mid-drain.
async fn request_shutdown(addr: &str, token: Option<&str>) -> Result<()> {
    let Some(token) = token else {
        anyhow::bail!("no local token available to authenticate shutdown");
    };
    let client = reqwest::Client::new();
    post(&client, addr, "/system/shutdown", token).await?;
    Ok(())
}

/// Polls until nothing answers the address anymore. Any transport-level
/// failure counts as gone; only live HTTP responses keep waiting.
async fn wait_until_gone(addr: &str, budget: Duration) -> bool {
    let deadline = std::time::Instant::now() + budget;
    loop {
        let client = reqwest::Client::new();
        let probe = client
            .get(format!("{}/system/health", addr.trim_end_matches('/')))
            .timeout(Duration::from_millis(1_000))
            .send()
            .await;
        match probe {
            // Something still answers: keep waiting for the drain.
            Ok(_) => {}
            // Refused/reset/timed out: the listener is closed.
            Err(_) => return true,
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(DRAIN_POLL).await;
    }
}

fn terminate(pid: u32) -> Result<()> {
    let output = platform_terminate(pid).with_context(|| format!("terminating pid {pid}"))?;
    if !output.status.success() {
        // A missing process means it exited between our probe and the kill:
        // that is success for this purpose.
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !stderr.contains("not found") {
            anyhow::bail!("terminate of pid {pid} failed: {}", stderr.trim());
        }
    }
    Ok(())
}

#[cfg(windows)]
fn platform_terminate(pid: u32) -> Result<std::process::Output> {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    use std::os::windows::process::CommandExt;

    std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(Into::into)
}

#[cfg(unix)]
fn platform_terminate(pid: u32) -> Result<std::process::Output> {
    std::process::Command::new("kill")
        .args(["-9", &pid.to_string()])
        .output()
        .map_err(Into::into)
}

#[cfg(not(any(windows, unix)))]
fn platform_terminate(_pid: u32) -> Result<std::process::Output> {
    anyhow::bail!("force-termination is not supported on this platform")
}

fn report_stopped(detail: &str, json: bool) -> Result<()> {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "state": "stopped",
                "detail": detail,
            }))?
        );
    } else {
        println!("Host stopped: {detail}");
    }
    Ok(())
}
