//! `lazarus host doctor`: local environment diagnostics with a stable
//! pass/warn/fail verdict per check.

use std::net::SocketAddr;

use anyhow::{Context, Result};
use lazarus_hostd::runtime::DataPaths;
use serde::Serialize;

use crate::client::local_token;
use crate::host::discovery::{self, PidRecord};
use crate::host::start::{probe_reachable, resolve_daemon_binary};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    Pass,
    Warn,
    Fail,
}

#[derive(Debug, Clone, Serialize)]
pub struct Check {
    pub name: &'static str,
    pub status: CheckStatus,
    pub detail: String,
}

fn pass(name: &'static str, detail: impl Into<String>) -> Check {
    Check {
        name,
        status: CheckStatus::Pass,
        detail: detail.into(),
    }
}

fn warn(name: &'static str, detail: impl Into<String>) -> Check {
    Check {
        name,
        status: CheckStatus::Warn,
        detail: detail.into(),
    }
}

fn fail(name: &'static str, detail: impl Into<String>) -> Check {
    Check {
        name,
        status: CheckStatus::Fail,
        detail: detail.into(),
    }
}

pub async fn run(json: bool) -> Result<()> {
    let checks = collect().await;
    let failing = checks
        .iter()
        .filter(|check| check.status == CheckStatus::Fail)
        .count();

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "ok": failing == 0,
                "checks": checks,
            }))?
        );
    } else {
        for check in &checks {
            let status = match check.status {
                CheckStatus::Pass => "PASS",
                CheckStatus::Warn => "WARN",
                CheckStatus::Fail => "FAIL",
            };
            println!("{:<16} {status:<5} {}", check.name, check.detail);
        }
        println!("\ndoctor: {}", if failing == 0 { "PASS" } else { "FAIL" });
    }

    if failing > 0 {
        anyhow::bail!("doctor reported {failing} failing check(s)");
    }
    Ok(())
}

async fn collect() -> Vec<Check> {
    let paths = match DataPaths::resolve() {
        Ok(paths) => paths,
        Err(error) => return vec![fail("data-root", error.to_string())],
    };
    let record = discovery::load_pid(&paths).ok().flatten();
    let token = local_token(&paths).ok();
    let reachable = match record.as_ref().map(|r| r.addr.clone()) {
        Some(addr) => probe_reachable(&addr, token.as_deref().unwrap_or("")).await,
        None => false,
    };

    vec![
        data_root_check(&paths),
        daemon_binary_check(),
        token_check(&paths),
        instance_record_check(record.as_ref()),
        endpoint_check(reachable, record.as_ref(), &token),
        lock_state_check(&paths, reachable),
        database_check(&paths),
        port_check(reachable, record.as_ref()),
    ]
}

fn data_root_check(paths: &DataPaths) -> Check {
    match prepare_and_probe(paths) {
        Ok(()) => pass("data-root", format!("{}", paths.root.display())),
        Err(error) => fail("data-root", error.to_string()),
    }
}

fn prepare_and_probe(paths: &DataPaths) -> Result<()> {
    paths.prepare()?;
    let probe = paths.root.join(".doctor-write-probe");
    std::fs::write(&probe, b"probe").context("writing the write-probe")?;
    std::fs::remove_file(&probe).context("removing the write-probe")?;
    Ok(())
}

fn daemon_binary_check() -> Check {
    match resolve_daemon_binary() {
        Ok(path) => pass("daemon-binary", path.display().to_string()),
        Err(error) => fail("daemon-binary", error.to_string()),
    }
}

fn token_check(paths: &DataPaths) -> Check {
    match discovery::read_token(paths) {
        Ok(Some(token)) => {
            if token.len() >= 32 {
                pass("local-token", "present (value withheld)")
            } else {
                warn(
                    "local-token",
                    "present but unusually short (value withheld)",
                )
            }
        }
        Ok(None) => warn(
            "local-token",
            "not provisioned yet; `host start` creates one",
        ),
        Err(error) => fail("local-token", error.to_string()),
    }
}

fn instance_record_check(record: Option<&PidRecord>) -> Check {
    match record {
        Some(record) => pass(
            "instance-record",
            format!("pid {} at {}", record.pid, record.addr),
        ),
        None => warn(
            "instance-record",
            "no pid.json; the Host is not running or was started outside the CLI",
        ),
    }
}

fn endpoint_check(reachable: bool, record: Option<&PidRecord>, token: &Option<String>) -> Check {
    match (record, reachable) {
        (Some(record), true) => pass(
            "endpoint",
            format!("answering at {} (pid {})", record.addr, record.pid),
        ),
        (Some(record), false) => fail(
            "endpoint",
            format!("recorded Host at {} does not answer", record.addr),
        ),
        (None, _) if token.is_some() => warn("endpoint", "no recorded instance to probe"),
        (None, _) => warn(
            "endpoint",
            "nothing recorded and no token; run `host start` first",
        ),
    }
}

fn lock_state_check(paths: &DataPaths, reachable: bool) -> Check {
    if reachable {
        return pass("lock-state", "owned by the live Host");
    }
    let crash_marker = paths.host.join("running.json");
    let lock_file = paths.host.join("host.lock");
    if crash_marker.exists() {
        warn(
            "lock-state",
            "a previous run left its running marker behind; the next start records an unclean-shutdown recovery",
        )
    } else if lock_file.exists() {
        warn(
            "lock-state",
            "a stale host.lock file remains; it is safe and ignored",
        )
    } else {
        pass("lock-state", "clean")
    }
}

fn database_check(paths: &DataPaths) -> Check {
    let database = paths.database();
    if !database.exists() {
        return warn(
            "database",
            "not created yet; the first start initializes it",
        );
    }
    match rusqlite::Connection::open_with_flags(
        &database,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .and_then(|connection| {
        connection.query_row("PRAGMA quick_check", [], |row| row.get::<_, String>(0))
    }) {
        Ok(result) if result == "ok" => pass("database", result),
        Ok(result) => fail("database", format!("quick_check reported: {result}")),
        Err(error) => fail("database", error.to_string()),
    }
}

fn port_check(reachable: bool, record: Option<&PidRecord>) -> Check {
    if reachable || record.is_none() {
        return pass("listen-port", "skipped while nothing needs to bind");
    }
    let addr: SocketAddr = record
        .map(|r| r.addr.trim_start_matches("http://").to_owned())
        .unwrap_or_else(|| discovery::DEFAULT_LISTEN_ADDR.to_owned())
        .parse()
        .expect("recorded addr parses as ip:port");
    match std::net::TcpListener::bind(addr) {
        Ok(listener) => {
            drop(listener);
            pass("listen-port", format!("{addr} is free"))
        }
        Err(error) => fail("listen-port", format!("{addr} cannot be bound: {error}")),
    }
}
