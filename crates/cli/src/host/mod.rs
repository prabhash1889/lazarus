//! `lazarus host`: daemon lifecycle and diagnostics.

pub mod discovery;
mod doctor;
mod logs;
mod start;
mod status;
mod stop;

use anyhow::Result;
use clap::Subcommand;

#[derive(Subcommand)]
pub enum HostCommands {
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
        HostCommands::Start { addr, json } => start::run(addr, json).await,
        HostCommands::Stop { json } => stop::run(json).await,
        HostCommands::Status { addr, json } => status::run(addr, json).await,
        HostCommands::Logs { tail } => logs::run(tail),
        HostCommands::Doctor { json } => doctor::run(json).await,
    }
}
