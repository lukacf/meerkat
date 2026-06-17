pub mod audit_generated_headers;
pub mod bridge_classifier;
pub mod effect_authority;
pub mod machine_alphabet;
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
pub mod runtime_authority_bypass;
pub mod seam_inventory;
pub mod typed_carrier;

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};

use crate::machine_alphabet::MachineAlphabetArgs;
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
    /// Structural effect-authority audit: runtime interrupt / runtime-effect
    /// authority must stay machine-owned (syn AST port of the former
    /// `scripts/audit-effect-authority.sh` gate).
    #[command(name = "effect-authority")]
    EffectAuthority,
    /// W2-F structural bridge-classifier gate: production bridge code must
    /// not re-interpret `ResponseStatus` (syn AST port of the former
    /// `scripts/pre-push-bridge-no-responsestatus.sh` grep gate).
    #[command(name = "bridge-classifier")]
    BridgeClassifier,
    /// Structural queue-to-run pilot bypass gate: raw accepted enqueue,
    /// dequeue, and stage calls must stay behind explicit generated
    /// authority/capability wrappers.
    #[command(name = "runtime-authority-bypass")]
    RuntimeAuthorityBypass,
    #[command(name = "ownership-ledger")]
    OwnershipLedger(OwnershipLedgerArgs),
    /// Verify every `@generated` header corresponds to a codegen-emit path
    /// and every codegen-emit path carries `@generated`. Errors on mismatch.
    #[command(name = "audit-generated-headers")]
    AuditGeneratedHeaders,
    /// Emit the canonical machine alphabet (phase/input/signal variant names
    /// per canonical machine) as JSON for the machine-poster content gate.
    #[command(name = "machine-alphabet")]
    MachineAlphabet(MachineAlphabetArgs),
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
        Commands::ProtocolCodegen => {
            run_machine_authority_task(protocol_codegen::run_protocol_codegen)
        }
        // rmat-audit, effect-authority, and bridge-classifier all walk the
        // whole workspace's syn ASTs — including the large generated machine
        // kernels, whose transition dispatch nests deeply — so they need the
        // same deep stack as the other machine-authority tasks. The default
        // ~8 MiB main-thread stack overflows syn's recursive visitor once the
        // mob kernel grows (the spawn-exec ladder pushed it over the edge).
        Commands::RmatAudit(args) => {
            run_machine_authority_task(move || rmat_audit::rmat_audit(args))
        }
        Commands::EffectAuthority => {
            run_machine_authority_task(effect_authority::run_effect_authority)
        }
        Commands::BridgeClassifier => {
            run_machine_authority_task(bridge_classifier::run_bridge_classifier)
        }
        Commands::RuntimeAuthorityBypass => {
            run_machine_authority_task(runtime_authority_bypass::run_runtime_authority_bypass)
        }
        // The ownership ledger resolves anchors against the canonical machine
        // catalog; constructing the DSL schemas needs the same deep stack as
        // the other machine-authority tasks.
        Commands::OwnershipLedger(args) => {
            run_machine_authority_task(move || ownership_ledger::run_ownership_ledger(args))
        }
        Commands::AuditGeneratedHeaders => {
            run_machine_authority_task(run_audit_generated_headers_command)
        }
        Commands::MachineAlphabet(args) => {
            run_machine_authority_task(move || machine_alphabet::run_machine_alphabet(args))
        }
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
