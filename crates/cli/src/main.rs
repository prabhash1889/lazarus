use clap::{Parser, Subcommand};

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
}

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Commands::Version => {
            println!("lazarus-cli {}", env!("CARGO_PKG_VERSION"));
            println!("Phase 0 shell: no host connection yet.");
        }
    }
}
