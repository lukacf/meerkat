mod machines;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "xtask")]
#[command(about = "Meerkat 0.5 machine authority tooling")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    #[command(name = "machine-codegen")]
    MachineCodegen(machines::SelectionArgs),
    #[command(name = "machine-verify")]
    MachineVerify(machines::VerifyArgs),
    #[command(name = "machine-check-drift")]
    MachineCheckDrift(machines::SelectionArgs),
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Commands::MachineCodegen(args) => machines::machine_codegen(args),
        Commands::MachineVerify(args) => machines::machine_verify(args),
        Commands::MachineCheckDrift(args) => machines::machine_check_drift(args),
    }
}
