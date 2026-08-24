use anyhow::Result;
use clap::{Parser, Subcommand};
use lazarus_cli::host;

#[derive(Parser)]
#[command(name = "lazarus", version, about = "Lazarus CLI")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Print the CLI version.
    Version,
    /// Manage the local lazarus-hostd daemon lifecycle.
    Host {
        #[command(subcommand)]
        command: host::HostCommands,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Version => {
            println!("lazarus-cli {}", env!("CARGO_PKG_VERSION"));
        }
        Commands::Host { command } => {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?
                .block_on(host::dispatch(command))?;
        }
    }
    Ok(())
}
