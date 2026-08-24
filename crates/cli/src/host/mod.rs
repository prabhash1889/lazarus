//! `lazarus host`: daemon lifecycle, installation, and diagnostics.

pub mod discovery;
mod doctor;
mod logs;
pub(crate) mod start;
mod status;
mod stop;
mod update;

use anyhow::Result;
use clap::Subcommand;

#[derive(Subcommand)]
pub enum HostCommands {
    /// Install the Host from a signed release manifest (first-time
    /// bootstrap or explicit reinstall).
    Install {
        #[command(flatten)]
        args: update::InstallArgs,
    },
    /// Install only when missing or out of date; no-op when current. The
    /// idempotent bootstrap the Desktop calls before `host start`.
    Ensure {
        #[command(flatten)]
        args: update::EnsureArgs,
    },
    /// Update the installed Host from a signed release manifest,
    /// retaining the previous installation for rollback.
    Update {
        #[command(flatten)]
        args: update::UpdateArgs,
    },
    /// Swap the retained previous installation back in.
    Rollback {
        /// Emit machine-readable JSON instead of human output.
        #[arg(long)]
        json: bool,
    },
    /// Start lazarus-hostd detached, provision the local token, wait for
    /// health, and record the instance under the data root.
    Start {
        /// Listen address (ip:port, loopback only). Defaults to
        /// 127.0.0.1:50051.
        #[arg(long)]
        addr: Option<String>,
        /// Emit machine-readable JSON instead of human output.
        #[arg(long)]
        json: bool,
    },
    /// Ask the running Host to shut down gracefully; force-terminate it if
    /// it does not drain in time.
    Stop {
        /// Emit machine-readable JSON instead of human output.
        #[arg(long)]
        json: bool,
    },
    /// Report whether a Host is reachable and its negotiated contract
    /// status.
    Status {
        /// Probe this address instead of the recorded or default one.
        #[arg(long)]
        addr: Option<String>,
        /// Emit machine-readable JSON instead of human output.
        #[arg(long)]
        json: bool,
    },
    /// Print the tail of the daemon's structured log.
    Logs {
        /// How many trailing lines to print.
        #[arg(long, default_value_t = 200)]
        tail: usize,
    },
    /// Run local environment diagnostics and report pass/warn/fail per
    /// check.
    Doctor {
        /// Emit machine-readable JSON instead of human output.
        #[arg(long)]
        json: bool,
    },
}

pub async fn dispatch(command: HostCommands) -> Result<()> {
    match command {
        HostCommands::Install { args } => update::run_install(args).await,
        HostCommands::Ensure { args } => update::run_ensure(args).await,
        HostCommands::Update { args } => update::run_update(args).await,
        HostCommands::Rollback { json } => update::run_rollback(json).await,
        HostCommands::Start { addr, json } => start::run(addr, json).await,
        HostCommands::Stop { json } => stop::run(json).await,
        HostCommands::Status { addr, json } => status::run(addr, json).await,
        HostCommands::Logs { tail } => logs::run(tail),
        HostCommands::Doctor { json } => doctor::run(json).await,
    }
}
