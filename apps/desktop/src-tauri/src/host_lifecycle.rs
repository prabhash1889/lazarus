//! Desktop delegation of Host lifecycle mutations to the bundled CLI.
//!
//! The desktop never spawns, stops, or updates the Host itself: per the
//! product contract there is exactly one lifecycle owner (`lazarus host`),
//! and this shell invokes it. Every command parses the CLI's `--json`
//! output defensively - unknown additive fields are ignored, and every
//! failure is returned in-band so the UI always renders instead of
//! rejecting a promise.

use std::path::PathBuf;
use std::time::Duration;

use serde::Serialize;
use tokio::process::Command;

const START_TIMEOUT: Duration = Duration::from_secs(30);
const STOP_TIMEOUT: Duration = Duration::from_secs(30);
const DOCTOR_TIMEOUT: Duration = Duration::from_secs(30);
/// Installs and updates download release artifacts, which can be large;
/// the CLI owns resumability so a timeout here is never data loss.
const UPDATE_TIMEOUT: Duration = Duration::from_secs(600);

/// Keep the helper console invisible on Windows; the CLI output is captured,
/// never shown in a terminal of its own.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ActionResult {
    pub ok: bool,
    pub detail: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DoctorResult {
    pub ok: bool,
    /// The full parsed doctor report (shape owned by the CLI); `None` when
    /// the doctor could not be run at all.
    pub report: Option<serde_json::Value>,
    pub error: Option<String>,
}

fn cli_file_name() -> &'static str {
    #[cfg(windows)]
    {
        "lazarus-cli.exe"
    }
    #[cfg(not(windows))]
    {
        "lazarus-cli"
    }
}

/// Resolution order for the bundled CLI: explicit override, a sibling of
/// this executable (the bundled layout), the workspace target directory
/// during development, then whatever is on PATH.
pub(crate) fn resolve_cli_path() -> Result<PathBuf, String> {
    if let Some(raw) = std::env::var_os("LAZARUS_CLI_PATH").filter(|v| !v.is_empty()) {
        let path = PathBuf::from(raw);
        if path.exists() {
            return Ok(path);
        }
        return Err(format!(
            "LAZARUS_CLI_PATH points at a missing file: {}",
            path.display()
        ));
    }
    if let Ok(current) = std::env::current_exe()
        && let Some(sibling) = current.parent().map(|dir| dir.join(cli_file_name()))
        && sibling.exists()
    {
        return Ok(sibling);
    }
    for profile in ["debug", "release"] {
        let candidate = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../target")
            .join(profile)
            .join(cli_file_name());
        if candidate.exists() {
            return Ok(candidate);
        }
    }
    let on_path = cli_file_name().trim_end_matches(".exe");
    Ok(PathBuf::from(on_path))
}

struct CliOutput {
    success: bool,
    stdout: String,
    stderr: String,
}

async fn run_cli(args: &[&str], timeout: Duration) -> Result<CliOutput, String> {
    let path = resolve_cli_path()?;
    let mut command = Command::new(&path);
    command.args(args);
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);

    let rendered = format!("lazarus {}", args.join(" "));
    let output = tokio::time::timeout(timeout, command.output())
        .await
        .map_err(|_| format!("`{rendered}` timed out after {}s", timeout.as_secs()))?
        .map_err(|error| format!("cannot execute {}: {error}", path.display()))?;

    Ok(CliOutput {
        success: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    })
}

fn failure_of(output: &CliOutput, command: &str) -> Option<String> {
    if output.success {
        return None;
    }
    let stderr = output.stderr.trim();
    if stderr.is_empty() {
        Some(format!("`{command}` failed"))
    } else {
        Some(stderr.to_string())
    }
}

fn parse_json(output: &CliOutput, command: &str) -> Result<serde_json::Value, String> {
    serde_json::from_str(output.stdout.trim()).map_err(|error| {
        format!(
            "`{command}` printed unparseable JSON ({error}); run it from a terminal for details"
        )
    })
}

#[tauri::command]
pub async fn host_start() -> ActionResult {
    const COMMAND: &str = "host start --json";
    match run_cli(&["host", "start", "--json"], START_TIMEOUT).await {
        Ok(output) => match parse_json(&output, COMMAND) {
            Ok(value) => action_from_output(&value),
            Err(error) => ActionResult {
                ok: false,
                detail: None,
                error: Some(error),
            },
        },
        Err(error) => ActionResult {
            ok: false,
            detail: None,
            error: Some(error),
        },
    }
}

#[tauri::command]
pub async fn host_stop() -> ActionResult {
    const COMMAND: &str = "host stop --json";
    match run_cli(&["host", "stop", "--json"], STOP_TIMEOUT).await {
        Ok(output) => {
            if let Some(error) = failure_of(&output, COMMAND) {
                return ActionResult {
                    ok: false,
                    detail: None,
                    error: Some(error),
                };
            }
            match parse_json(&output, COMMAND) {
                Ok(value) => action_from_output(&value),
                Err(error) => ActionResult {
                    ok: false,
                    detail: None,
                    error: Some(error),
                },
            }
        }
        Err(error) => ActionResult {
            ok: false,
            detail: None,
            error: Some(error),
        },
    }
}

#[tauri::command]
pub async fn host_doctor() -> DoctorResult {
    const COMMAND: &str = "host doctor --json";
    match run_cli(&["host", "doctor", "--json"], DOCTOR_TIMEOUT).await {
        Ok(output) => {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(output.stdout.trim()) {
                doctor_from_output(&value)
            } else if let Some(error) = failure_of(&output, COMMAND) {
                DoctorResult {
                    ok: false,
                    report: None,
                    error: Some(error),
                }
            } else {
                DoctorResult {
                    ok: false,
                    report: None,
                    error: Some(format!("`{COMMAND}` printed unparseable JSON")),
                }
            }
        }
        Err(error) => DoctorResult {
            ok: false,
            report: None,
            error: Some(error),
        },
    }
}

/// Shared shape for install/update/rollback: run the CLI, surface failures
/// in-band, and project the JSON state onto an action result.
async fn run_action(args: &[&str], timeout: Duration, command: &str) -> ActionResult {
    match run_cli(args, timeout).await {
        Ok(output) => {
            if let Some(error) = failure_of(&output, command) {
                return ActionResult {
                    ok: false,
                    detail: None,
                    error: Some(error),
                };
            }
            match parse_json(&output, command) {
                Ok(value) => action_from_output(&value),
                Err(error) => ActionResult {
                    ok: false,
                    detail: None,
                    error: Some(error),
                },
            }
        }
        Err(error) => ActionResult {
            ok: false,
            detail: None,
            error: Some(error),
        },
    }
}

/// Bootstraps (or repairs) the Host installation through the bundled CLI.
/// `ensure` is deliberately idempotent: it installs only when the
/// installed release is missing or differs from the manifest.
#[tauri::command]
pub async fn host_ensure(manifest: String) -> ActionResult {
    run_action(
        &["host", "ensure", "--manifest", &manifest, "--json"],
        UPDATE_TIMEOUT,
        "host ensure --json",
    )
    .await
}

/// Updates the Host installation through the bundled CLI. The CLI refuses
/// to touch an install while its Host is running and reports that in-band.
#[tauri::command]
pub async fn host_update(manifest: String) -> ActionResult {
    run_action(
        &["host", "update", "--manifest", &manifest, "--json"],
        UPDATE_TIMEOUT,
        "host update --json",
    )
    .await
}

/// Rolls the Host installation back to the retained previous release via
/// the bundled CLI.
#[tauri::command]
pub async fn host_rollback() -> ActionResult {
    run_action(
        &["host", "rollback", "--json"],
        STOP_TIMEOUT,
        "host rollback --json",
    )
    .await
}

/// Projects a lifecycle action's JSON onto the UI result. The CLI's stable
/// fields are read leniently so additive changes cannot break the shell.
fn action_from_output(value: &serde_json::Value) -> ActionResult {
    let state = value["state"].as_str().map(str::to_owned);
    let detail = value["detail"].as_str().map(str::to_owned);
    ActionResult {
        ok: true,
        detail: state.or(detail),
        error: None,
    }
}

/// The doctor exits nonzero when any check fails but still prints its full
/// JSON report; both halves must survive to the UI.
fn doctor_from_output(value: &serde_json::Value) -> DoctorResult {
    let ok = value["ok"].as_bool().unwrap_or(false);
    DoctorResult {
        ok,
        report: Some(value.clone()),
        error: if ok {
            None
        } else {
            Some("doctor reported failing checks".to_owned())
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_results_surface_state_or_detail_and_stay_ok() {
        let started = action_from_output(&serde_json::json!({
            "pid": 42,
            "addr": "http://127.0.0.1:50051",
            "state": "started",
        }));
        assert_eq!(
            started,
            ActionResult {
                ok: true,
                detail: Some("started".to_owned()),
                error: None,
            }
        );

        // Additive future fields and missing state/detail stay non-fatal.
        let additive = action_from_output(&serde_json::json!({
            "futureField": {"nested": true},
        }));
        assert_eq!(additive.detail, None);
        assert!(additive.ok);

        let with_detail = action_from_output(&serde_json::json!({
            "state": "stopped",
            "detail": "drained gracefully (pid 7)",
        }));
        assert_eq!(with_detail.detail, Some("stopped".to_owned()));
    }

    #[test]
    fn doctor_results_carry_the_report_even_on_failure() {
        let healthy = doctor_from_output(&serde_json::json!({
            "ok": true,
            "checks": [{ "name": "data-root", "status": "pass", "detail": "x" }],
        }));
        assert!(healthy.ok);
        assert_eq!(
            healthy.report.expect("report")["checks"]
                .as_array()
                .expect("checks")
                .len(),
            1
        );
        assert!(healthy.error.is_none());

        let unhealthy = doctor_from_output(&serde_json::json!({
            "ok": false,
            "checks": [],
        }));
        assert!(!unhealthy.ok);
        assert!(unhealthy.error.as_deref().unwrap_or("").contains("failing"));
    }

    #[test]
    fn unparseable_output_becomes_an_actionable_error_not_a_crash() {
        let output = CliOutput {
            success: true,
            stdout: "<html>not json</html>".to_owned(),
            stderr: String::new(),
        };
        let error = parse_json(&output, "host stop --json").unwrap_err();
        assert!(error.contains("unparseable"), "{error}");

        let failed = CliOutput {
            success: false,
            stdout: String::new(),
            stderr: "daemon did not become healthy\n".to_owned(),
        };
        assert_eq!(
            failure_of(&failed, "host start --json").as_deref(),
            Some("daemon did not become healthy")
        );

        let silent = CliOutput {
            success: false,
            stdout: String::new(),
            stderr: String::new(),
        };
        assert_eq!(
            failure_of(&silent, "host start --json").as_deref(),
            Some("`host start --json` failed")
        );
        assert!(
            failure_of(
                &CliOutput {
                    success: true,
                    stdout: String::new(),
                    stderr: String::new(),
                },
                "host stop --json"
            )
            .is_none()
        );
    }

    #[test]
    fn cli_resolution_falls_back_to_a_usable_candidate_without_an_override() {
        let resolved = resolve_cli_path().expect("a fallback candidate");
        let name = resolved.file_name().expect("file name").to_string_lossy();
        #[cfg(windows)]
        assert_eq!(name, "lazarus-cli.exe");
        #[cfg(not(windows))]
        assert_eq!(name, "lazarus-cli");
    }
}
