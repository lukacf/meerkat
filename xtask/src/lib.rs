#[cfg(feature = "machine-authority")]
pub mod machines;
#[cfg(not(feature = "machine-authority"))]
#[path = "machines_test_support.rs"]
pub mod machines;
pub mod public_contracts;

use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::machines::{SelectionArgs, VerifyArgs};

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
    MachineCodegen(SelectionArgs),
    #[command(name = "machine-verify")]
    MachineVerify(VerifyArgs),
    #[command(name = "machine-check-drift")]
    MachineCheckDrift(SelectionArgs),
}

pub fn run() -> Result<()> {
    match Cli::parse().command {
        Commands::MachineCodegen(args) => machines::machine_codegen(args),
        Commands::MachineVerify(args) => machines::machine_verify(args),
        Commands::MachineCheckDrift(args) => machines::machine_check_drift(args),
    }
}
