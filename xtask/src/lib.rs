#[cfg(feature = "machine-authority")]
pub mod machines;
#[cfg(not(feature = "machine-authority"))]
#[path = "machines_test_support.rs"]
pub mod machines;
pub mod protocol_codegen;
pub mod public_contracts;
pub mod rmat_audit;
pub mod rmat_policy;
pub mod seam_inventory;

use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::machines::{SelectionArgs, VerifyArgs};
use crate::rmat_audit::RmatAuditArgs;

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
    Codegen(SelectionArgs),
    #[command(name = "machine-verify")]
    Verify(VerifyArgs),
    #[command(name = "machine-check-drift")]
    CheckDrift(SelectionArgs),
    #[command(name = "seam-inventory")]
    SeamInventory,
    #[command(name = "protocol-codegen")]
    ProtocolCodegen,
    #[command(name = "rmat-audit")]
    RmatAudit(RmatAuditArgs),
}

pub fn run() -> Result<()> {
    match Cli::parse().command {
        Commands::Codegen(args) => machines::machine_codegen(args),
        Commands::Verify(args) => machines::machine_verify(args),
        Commands::CheckDrift(args) => machines::machine_check_drift(args),
        Commands::SeamInventory => seam_inventory::run_seam_inventory(),
        Commands::ProtocolCodegen => protocol_codegen::run_protocol_codegen(),
        Commands::RmatAudit(args) => rmat_audit::rmat_audit(args),
    }
}
