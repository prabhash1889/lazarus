//! `lazarus host install|ensure|update|rollback`: the CLI-owned Host
//! updater. Every Host installation mutation flows through here - the
//! Desktop calls this same surface rather than implementing its own
//! downloader or promoter.

use anyhow::{Context, Result};
use clap::Args;

use crate::updater::{ApplyOutcome, Updater};
use lazarus_hostd::runtime::DataPaths;

#[derive(Args)]
pub struct InstallArgs {
    /// Path or HTTPS URL of the signed release manifest.
    #[arg(long)]
    manifest: String,
    /// Reinstall even when the installed release already matches.
    #[arg(long)]
    force: bool,
    /// Emit machine-readable JSON instead of human output.
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
pub struct EnsureArgs {
    /// Path or HTTPS URL of the signed release manifest.
    #[arg(long)]
    manifest: String,
    /// Emit machine-readable JSON instead of human output.
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
pub struct UpdateArgs {
    /// Path or HTTPS URL of the signed release manifest.
    #[arg(long)]
    manifest: String,
    /// Reinstall even when the installed release already matches.
    #[arg(long)]
    force: bool,
    /// Emit machine-readable JSON instead of human output.
    #[arg(long)]
    json: bool,
}

fn updater() -> Result<Updater> {
    let paths = DataPaths::resolve().context("resolving the Lazarus data directory")?;
    let trust_root = crate::updater::trust::release_trust_root()?;
    Ok(Updater::new(paths, trust_root))
}

pub async fn run_install(args: InstallArgs) -> Result<()> {
    report_apply(
        updater()?.apply(&args.manifest, args.force).await?,
        args.json,
    )
}

/// `host ensure` is the idempotent bootstrap the Desktop calls before
/// `host start`: installs when missing, no-ops when current.
pub async fn run_ensure(args: EnsureArgs) -> Result<()> {
    report_apply(updater()?.apply(&args.manifest, false).await?, args.json)
}

pub async fn run_update(args: UpdateArgs) -> Result<()> {
    report_apply(
        updater()?.apply(&args.manifest, args.force).await?,
        args.json,
    )
}

pub async fn run_rollback(json: bool) -> Result<()> {
    let restored = updater()?.rollback()?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "state": "rolled-back",
                "version": restored.restored_version,
            }))?
        );
    } else {
        println!("Host rolled back: version {}", restored.restored_version);
    }
    Ok(())
}

fn report_apply(outcome: ApplyOutcome, json: bool) -> Result<()> {
    let (state, version, detail) = match &outcome {
        ApplyOutcome::Installed { version } => (
            "installed",
            version.clone(),
            "the Host was not previously installed".to_owned(),
        ),
        ApplyOutcome::Updated {
            from_version,
            version,
        } => (
            "updated",
            version.clone(),
            match from_version {
                Some(previous) => format!("updated from {previous}"),
                None => "updated".to_owned(),
            },
        ),
        ApplyOutcome::AlreadyCurrent { version } => (
            "already-current",
            version.clone(),
            "the installed release already matches this manifest".to_owned(),
        ),
    };
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "state": state,
                "version": version,
                "detail": detail,
            }))?
        );
    } else {
        println!("Host {state}: version {version} ({detail})");
    }
    Ok(())
}
