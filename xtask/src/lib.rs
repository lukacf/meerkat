pub mod audit_generated_headers;
#[cfg(feature = "machine-authority")]
pub mod machines;
#[cfg(not(feature = "machine-authority"))]
#[path = "machines_test_support.rs"]
pub mod machines;
pub mod ownership_ledger;
pub mod protocol_codegen;
pub mod public_contracts;
pub mod rmat_audit;
pub mod rmat_policy;
pub mod seam_inventory;

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};

#[cfg(feature = "machine-authority")]
use crate::machines::HopcroftArgs;
use crate::machines::{SelectionArgs, VerifyArgs};
use crate::ownership_ledger::OwnershipLedgerArgs;
use crate::rmat_audit::RmatAuditArgs;
use crate::seam_inventory::SeamInventoryArgs;

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
    #[cfg(feature = "machine-authority")]
    #[command(name = "machine-hopcroft")]
    Hopcroft(HopcroftArgs),
    #[command(name = "machine-check-drift")]
    CheckDrift(SelectionArgs),
    #[command(name = "seam-inventory")]
    SeamInventory(SeamInventoryArgs),
    #[command(name = "protocol-codegen")]
    ProtocolCodegen,
    #[command(name = "rmat-audit")]
    RmatAudit(RmatAuditArgs),
    #[command(name = "ownership-ledger")]
    OwnershipLedger(OwnershipLedgerArgs),
    /// Verify every `@generated` header corresponds to a codegen-emit path
    /// and every codegen-emit path carries `@generated`. Errors on mismatch.
    #[command(name = "audit-generated-headers")]
    AuditGeneratedHeaders,
}

pub fn run() -> Result<()> {
    match Cli::parse().command {
        Commands::Codegen(args) => {
            run_machine_authority_task(move || machines::machine_codegen(args))
        }
        Commands::Verify(args) => {
            run_machine_authority_task(move || machines::machine_verify(args))
        }
        #[cfg(feature = "machine-authority")]
        Commands::Hopcroft(args) => {
            run_machine_authority_task(move || machines::machine_hopcroft(args))
        }
        Commands::CheckDrift(args) => {
            run_machine_authority_task(move || machines::machine_check_drift(args))
        }
        Commands::SeamInventory(args) => seam_inventory::run_seam_inventory(args),
        Commands::ProtocolCodegen => protocol_codegen::run_protocol_codegen(),
        Commands::RmatAudit(args) => rmat_audit::rmat_audit(args),
        Commands::OwnershipLedger(args) => ownership_ledger::run_ownership_ledger(args),
        Commands::AuditGeneratedHeaders => run_audit_generated_headers_command(),
    }
}

fn run_machine_authority_task(task: impl FnOnce() -> Result<()> + Send + 'static) -> Result<()> {
    const MACHINE_AUTHORITY_STACK_SIZE: usize = 64 * 1024 * 1024;
    let handle = std::thread::Builder::new()
        .name("machine-authority-task".to_owned())
        .stack_size(MACHINE_AUTHORITY_STACK_SIZE)
        .spawn(task)?;
    match handle.join() {
        Ok(result) => result,
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

fn run_audit_generated_headers_command() -> Result<()> {
    let findings = audit_generated_headers::run_audit_generated_headers()?;
    if findings.is_empty() {
        println!(
            "audit-generated-headers: {} path(s) checked; all `@generated` markers honest",
            audit_generated_headers::live_emit_paths().len()
        );
        return Ok(());
    }
    let rendered = audit_generated_headers::render_findings(&findings);
    eprint!("{rendered}");
    bail!(
        "audit-generated-headers: {} violation(s) detected",
        findings.len()
    );
}
