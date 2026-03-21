use std::fmt;

use meerkat_machine_schema::{
    EffectDisposition, MachineSchema, canonical_composition_schemas, canonical_machine_schemas,
};

/// Classification of an effect's ownership boundary characteristics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeamClassification {
    /// Effect is fully internal to the machine — no owner realization needed.
    /// Examples: state projection, local bookkeeping.
    NoOwnerRealization,
    /// Effect requires owner/shell to realize it, but no feedback is expected.
    /// The machine emits and moves on; correctness does not depend on acknowledgment.
    OwnerRealizationOnly,
    /// Effect requires owner realization AND the owner must feed back into the
    /// machine (or another composed machine) for the lifecycle to close.
    /// This is the seam that needs a formal handoff protocol.
    OwnerRealizationPlusFeedback,
    /// Effect is a terminal/result signal whose surface representation must align
    /// with machine truth. Divergence here means the API lies about outcomes.
    SurfaceResultAlignment,
}

impl fmt::Display for SeamClassification {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoOwnerRealization => write!(f, "no-owner-realization"),
            Self::OwnerRealizationOnly => write!(f, "owner-realization-only"),
            Self::OwnerRealizationPlusFeedback => write!(f, "owner-realization-plus-feedback"),
            Self::SurfaceResultAlignment => write!(f, "surface-result-alignment"),
        }
    }
}

#[derive(Debug)]
pub struct SeamEntry {
    pub machine: String,
    pub effect_variant: String,
    pub disposition: String,
    pub classification: SeamClassification,
    pub notes: String,
    pub explicitly_classified: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContractStatus {
    Closed,
    Open,
}

#[derive(Debug)]
pub struct SurfaceContractEntry {
    pub name: &'static str,
    pub notes: &'static str,
    pub status: ContractStatus,
}

/// Known effect classifications for canonical machines.
/// This is the manually-curated ground truth that drives the seam inventory.
/// Effects not listed here get a default classification based on disposition type.
fn known_classifications() -> Vec<(&'static str, &'static str, SeamClassification, &'static str)> {
    vec![
        // === CommsDrainLifecycleMachine ===
        (
            "CommsDrainLifecycleMachine",
            "SpawnDrainTask",
            SeamClassification::OwnerRealizationPlusFeedback,
            "Shell must spawn task AND feed back TaskSpawned/TaskExited",
        ),
        (
            "CommsDrainLifecycleMachine",
            "AbortDrainTask",
            SeamClassification::OwnerRealizationPlusFeedback,
            "Shell must abort task AND feed back AbortObserved",
        ),
        (
            "CommsDrainLifecycleMachine",
            "SetTurnBoundaryDrainSuppressed",
            SeamClassification::NoOwnerRealization,
            "Local projection — sets atomic flag observable by turn loop",
        ),
        // === TurnExecutionMachine ===
        (
            "TurnExecutionMachine",
            "RunStarted",
            SeamClassification::SurfaceResultAlignment,
            "External signal: surface must emit run-started event matching machine truth",
        ),
        (
            "TurnExecutionMachine",
            "RunCompleted",
            SeamClassification::SurfaceResultAlignment,
            "Terminal outcome routed to ingress/control — surface result must match",
        ),
        (
            "TurnExecutionMachine",
            "RunFailed",
            SeamClassification::SurfaceResultAlignment,
            "Terminal outcome routed to ingress/control — surface result must match",
        ),
        (
            "TurnExecutionMachine",
            "RunCancelled",
            SeamClassification::SurfaceResultAlignment,
            "Terminal outcome routed to ingress/control — surface result must match",
        ),
        (
            "TurnExecutionMachine",
            "DrainCommsInbox",
            SeamClassification::OwnerRealizationOnly,
            "Shell executes comms drain mechanics; no feedback expected by this machine",
        ),
        (
            "TurnExecutionMachine",
            "CheckCompaction",
            SeamClassification::OwnerRealizationOnly,
            "Shell checks compaction threshold; no feedback expected by this machine",
        ),
        // === OpsLifecycleMachine ===
        (
            "OpsLifecycleMachine",
            "NotifyOpWatcher",
            SeamClassification::NoOwnerRealization,
            "Local channel notification — no ownership boundary crossed",
        ),
        (
            "OpsLifecycleMachine",
            "RetainTerminalRecord",
            SeamClassification::NoOwnerRealization,
            "Local bookkeeping — retain completed op record",
        ),
        (
            "OpsLifecycleMachine",
            "EvictCompletedRecord",
            SeamClassification::NoOwnerRealization,
            "Local bookkeeping — evict completed op record",
        ),
        (
            "OpsLifecycleMachine",
            "WaitAllSatisfied",
            SeamClassification::OwnerRealizationPlusFeedback,
            "Barrier satisfaction signal — must feed into TurnExecution barrier",
        ),
        (
            "OpsLifecycleMachine",
            "CollectCompletedResult",
            SeamClassification::NoOwnerRealization,
            "Local result collection — no ownership boundary",
        ),
        (
            "OpsLifecycleMachine",
            "ConcurrencyLimitExceeded",
            SeamClassification::SurfaceResultAlignment,
            "External signal — surface must represent concurrency rejection accurately",
        ),
        // === RuntimeControlMachine ===
        (
            "RuntimeControlMachine",
            "ResolveAdmission",
            SeamClassification::NoOwnerRealization,
            "Local admission resolution mechanics",
        ),
        (
            "RuntimeControlMachine",
            "SignalWake",
            SeamClassification::NoOwnerRealization,
            "Local wake signal — no ownership boundary",
        ),
        (
            "RuntimeControlMachine",
            "SignalImmediateProcess",
            SeamClassification::NoOwnerRealization,
            "Local processing signal — no ownership boundary",
        ),
        (
            "RuntimeControlMachine",
            "EmitRuntimeNotice",
            SeamClassification::SurfaceResultAlignment,
            "External notice — surface must emit event matching machine state",
        ),
        (
            "RuntimeControlMachine",
            "ResolveCompletionAsTerminated",
            SeamClassification::SurfaceResultAlignment,
            "External terminal — surface must represent termination accurately",
        ),
        (
            "RuntimeControlMachine",
            "ApplyControlPlaneCommand",
            SeamClassification::OwnerRealizationOnly,
            "External command execution — shell applies but no feedback to machine",
        ),
        (
            "RuntimeControlMachine",
            "InitiateRecycle",
            SeamClassification::OwnerRealizationOnly,
            "Local recycle — shell executes recycle mechanics",
        ),
        // === RuntimeIngressMachine ===
        (
            "RuntimeIngressMachine",
            "IngressAccepted",
            SeamClassification::SurfaceResultAlignment,
            "External signal — surface must confirm ingress acceptance",
        ),
        (
            "RuntimeIngressMachine",
            "InputLifecycleNotice",
            SeamClassification::NoOwnerRealization,
            "Local lifecycle projection — drives InputLifecycleMachine locally",
        ),
        (
            "RuntimeIngressMachine",
            "WakeRuntime",
            SeamClassification::NoOwnerRealization,
            "Local wake signal — no ownership boundary",
        ),
        (
            "RuntimeIngressMachine",
            "RequestImmediateProcessing",
            SeamClassification::NoOwnerRealization,
            "Local processing request — no ownership boundary",
        ),
        (
            "RuntimeIngressMachine",
            "CompletionResolved",
            SeamClassification::SurfaceResultAlignment,
            "External completion — surface must represent resolution accurately",
        ),
        (
            "RuntimeIngressMachine",
            "IngressNotice",
            SeamClassification::SurfaceResultAlignment,
            "External notice — surface must emit matching event",
        ),
        (
            "RuntimeIngressMachine",
            "SilentIntentApplied",
            SeamClassification::SurfaceResultAlignment,
            "External signal — surface must represent silent intent accurately",
        ),
        // === InputLifecycleMachine ===
        (
            "InputLifecycleMachine",
            "InputLifecycleNotice",
            SeamClassification::NoOwnerRealization,
            "Local projection — no ownership boundary",
        ),
        (
            "InputLifecycleMachine",
            "RecordTerminalOutcome",
            SeamClassification::NoOwnerRealization,
            "Local projection — records terminal outcome locally",
        ),
        (
            "InputLifecycleMachine",
            "RecordRunAssociation",
            SeamClassification::NoOwnerRealization,
            "Local projection — records run association locally",
        ),
        (
            "InputLifecycleMachine",
            "RecordBoundarySequence",
            SeamClassification::NoOwnerRealization,
            "Local projection — records boundary sequence locally",
        ),
        // === ExternalToolSurfaceMachine ===
        (
            "ExternalToolSurfaceMachine",
            "ScheduleSurfaceCompletion",
            SeamClassification::OwnerRealizationPlusFeedback,
            "Shell must schedule completion AND feed back result via SurfaceCallCompleted",
        ),
        (
            "ExternalToolSurfaceMachine",
            "RefreshVisibleSurfaceSet",
            SeamClassification::NoOwnerRealization,
            "Local projection — refreshes cached tool set",
        ),
        (
            "ExternalToolSurfaceMachine",
            "EmitExternalToolDelta",
            SeamClassification::SurfaceResultAlignment,
            "External delta — surface must stream tool output matching machine state",
        ),
        (
            "ExternalToolSurfaceMachine",
            "CloseSurfaceConnection",
            SeamClassification::OwnerRealizationOnly,
            "Shell closes connection — no feedback expected by machine",
        ),
        (
            "ExternalToolSurfaceMachine",
            "RejectSurfaceCall",
            SeamClassification::SurfaceResultAlignment,
            "External rejection — surface must represent rejection accurately",
        ),
        // === PeerCommsMachine ===
        // (Routed — not classified here, handled by composition routes)
        // === MobLifecycleMachine ===
        (
            "MobLifecycleMachine",
            "EmitLifecycleNotice",
            SeamClassification::SurfaceResultAlignment,
            "External notice — surface must represent mob lifecycle accurately",
        ),
        (
            "MobMemberLifecycleAnchorMachine",
            "MemberLifecycleSnapshotUpdated",
            SeamClassification::NoOwnerRealization,
            "Local projection anchor — publishes derived member lifecycle snapshot only",
        ),
        (
            "MobRuntimeBridgeAnchorMachine",
            "RuntimeBridgeSnapshotUpdated",
            SeamClassification::NoOwnerRealization,
            "Local projection anchor — publishes derived runtime bridge snapshot only",
        ),
        (
            "MobWiringAnchorMachine",
            "WiringSnapshotUpdated",
            SeamClassification::NoOwnerRealization,
            "Local projection anchor — publishes derived wiring snapshot only",
        ),
        (
            "MobHelperResultAnchorMachine",
            "HelperResultSnapshotUpdated",
            SeamClassification::NoOwnerRealization,
            "Local projection anchor — publishes derived helper result snapshot only",
        ),
        // (RequestCleanup is Routed — not classified here)
        // === MobOrchestratorMachine ===
        (
            "MobOrchestratorMachine",
            "EmitOrchestratorNotice",
            SeamClassification::SurfaceResultAlignment,
            "External notice — surface must represent orchestrator state accurately",
        ),
        // (All other mob orchestrator effects are Routed)
        // === FlowRunMachine ===
        (
            "FlowRunMachine",
            "EmitFlowRunNotice",
            SeamClassification::SurfaceResultAlignment,
            "External notice — surface must represent flow run state accurately",
        ),
        (
            "FlowRunMachine",
            "EmitStepNotice",
            SeamClassification::SurfaceResultAlignment,
            "External notice — surface must represent step state accurately",
        ),
        (
            "FlowRunMachine",
            "AppendFailureLedger",
            SeamClassification::NoOwnerRealization,
            "Local persistence — no ownership boundary",
        ),
        (
            "FlowRunMachine",
            "PersistStepOutput",
            SeamClassification::NoOwnerRealization,
            "Local persistence — no ownership boundary",
        ),
        (
            "FlowRunMachine",
            "ProjectTargetSuccess",
            SeamClassification::SurfaceResultAlignment,
            "External projection — surface must represent target success accurately",
        ),
        (
            "FlowRunMachine",
            "ProjectTargetFailure",
            SeamClassification::SurfaceResultAlignment,
            "External projection — surface must represent target failure accurately",
        ),
        (
            "FlowRunMachine",
            "ProjectTargetCanceled",
            SeamClassification::SurfaceResultAlignment,
            "External projection — surface must represent target cancellation accurately",
        ),
        // (AdmitStepWork, FlowTerminalized, EscalateSupervisor are Routed)
    ]
}

/// Classify a Local or External effect. Falls back to disposition-based heuristic
/// if not in the known classifications table.
fn classify_effect(
    machine: &str,
    effect: &str,
    disposition: &EffectDisposition,
    known: &[(&str, &str, SeamClassification, &str)],
) -> (SeamClassification, String, bool) {
    // Check known classifications first
    for (m, e, class, notes) in known {
        if *m == machine && *e == effect {
            return (*class, notes.to_string(), true);
        }
    }

    // Heuristic fallback
    match disposition {
        EffectDisposition::Local => (
            SeamClassification::NoOwnerRealization,
            "Default: Local effect with no known owner feedback requirement".into(),
            false,
        ),
        EffectDisposition::External => (
            SeamClassification::OwnerRealizationOnly,
            "Default: External effect assumed to need shell realization without feedback".into(),
            false,
        ),
        EffectDisposition::Routed { .. } => {
            // Routed effects are handled by composition routes, not the seam inventory.
            // This branch should not be reached since we filter to Local/External only.
            (
                SeamClassification::NoOwnerRealization,
                "Routed — handled by composition routes, not seam inventory".into(),
                false,
            )
        }
    }
}

fn known_public_surface_contracts() -> Vec<SurfaceContractEntry> {
    vec![
        SurfaceContractEntry {
            name: "mob::spawn_helper",
            notes: "Contract-tested helper wrapper; return surface derives from canonical member/session terminal truth",
            status: ContractStatus::Closed,
        },
        SurfaceContractEntry {
            name: "mob::fork_helper",
            notes: "Contract-tested helper wrapper; return surface derives from canonical member/session terminal truth",
            status: ContractStatus::Closed,
        },
        SurfaceContractEntry {
            name: "mob::respawn",
            notes: "Contract-tested helper wrapper; receipt aligns with retired and replacement session truth",
            status: ContractStatus::Closed,
        },
        SurfaceContractEntry {
            name: "mob::wait_one",
            notes: "Helper wrapper polls canonical member/session state rather than inventing terminal classification",
            status: ContractStatus::Closed,
        },
        SurfaceContractEntry {
            name: "mob::wait_all",
            notes: "Helper wrapper composes wait_one over canonical member/session state",
            status: ContractStatus::Closed,
        },
    ]
}

pub fn run_seam_inventory() -> anyhow::Result<()> {
    let machines = canonical_machine_schemas();
    let compositions = canonical_composition_schemas();
    let known = known_classifications();
    let mut entries: Vec<SeamEntry> = Vec::new();
    let public_surface_contracts = known_public_surface_contracts();

    for machine in &machines {
        collect_machine_seams(machine, &known, &mut entries);
    }

    let protocol_index = compositions
        .iter()
        .flat_map(|composition| {
            composition.handoff_protocols.iter().filter_map(|protocol| {
                composition
                    .machines
                    .iter()
                    .find(|instance| instance.instance_id == protocol.producer_instance)
                    .map(|instance| {
                        (
                            (
                                instance.machine_name.clone(),
                                protocol.effect_variant.clone(),
                            ),
                            protocol.name.clone(),
                        )
                    })
            })
        })
        .fold(
            std::collections::BTreeMap::<(String, String), Vec<String>>::new(),
            |mut acc, (key, protocol_name)| {
                acc.entry(key).or_default().push(protocol_name);
                acc
            },
        );

    let unresolved_classification_debt = entries
        .iter()
        .filter(|entry| !entry.explicitly_classified)
        .collect::<Vec<_>>();
    let unresolved_protocol_debt = entries
        .iter()
        .filter(|entry| entry.classification == SeamClassification::OwnerRealizationPlusFeedback)
        .filter(|entry| {
            !protocol_index.contains_key(&(entry.machine.clone(), entry.effect_variant.clone()))
        })
        .collect::<Vec<_>>();
    let unresolved_public_surface_alignment_debt = public_surface_contracts
        .iter()
        .filter(|entry| entry.status != ContractStatus::Closed)
        .collect::<Vec<_>>();

    // Print the report
    print_report(&entries);

    // Summary statistics
    print_summary(
        &entries,
        &unresolved_classification_debt,
        &unresolved_protocol_debt,
        &public_surface_contracts,
        &unresolved_public_surface_alignment_debt,
    );

    if !unresolved_classification_debt.is_empty()
        || !unresolved_protocol_debt.is_empty()
        || !unresolved_public_surface_alignment_debt.is_empty()
    {
        anyhow::bail!(
            "seam inventory has unresolved debt: classification={}, protocol={}, public_surface={}",
            unresolved_classification_debt.len(),
            unresolved_protocol_debt.len(),
            unresolved_public_surface_alignment_debt.len()
        );
    }

    Ok(())
}

fn collect_machine_seams(
    machine: &MachineSchema,
    known: &[(&str, &str, SeamClassification, &str)],
    entries: &mut Vec<SeamEntry>,
) {
    for rule in &machine.effect_dispositions {
        match &rule.disposition {
            EffectDisposition::Local | EffectDisposition::External => {
                let disposition_str = match &rule.disposition {
                    EffectDisposition::Local => "Local",
                    EffectDisposition::External => "External",
                    EffectDisposition::Routed { .. } => unreachable!(),
                };

                let (classification, notes, explicitly_classified) = classify_effect(
                    &machine.machine,
                    &rule.effect_variant,
                    &rule.disposition,
                    known,
                );

                entries.push(SeamEntry {
                    machine: machine.machine.clone(),
                    effect_variant: rule.effect_variant.clone(),
                    disposition: disposition_str.to_string(),
                    classification,
                    notes,
                    explicitly_classified,
                });
            }
            EffectDisposition::Routed { .. } => {
                // Routed effects are handled by composition routes — skip in seam inventory
            }
        }
    }
}

fn print_report(entries: &[SeamEntry]) {
    println!("# Seam Inventory Report");
    println!("# Generated by `xtask seam-inventory`");
    println!();

    let mut current_machine = "";
    for entry in entries {
        if entry.machine != current_machine {
            if !current_machine.is_empty() {
                println!();
            }
            println!("## {}", entry.machine);
            current_machine = &entry.machine;
        }

        println!(
            "  {:40} {:10} {:40} {}",
            entry.effect_variant, entry.disposition, entry.classification, entry.notes,
        );
    }
}

fn print_summary(
    entries: &[SeamEntry],
    unresolved_classification_debt: &[&SeamEntry],
    unresolved_protocol_debt: &[&SeamEntry],
    public_surface_contracts: &[SurfaceContractEntry],
    unresolved_public_surface_alignment_debt: &[&SurfaceContractEntry],
) {
    let total = entries.len();
    let no_owner = entries
        .iter()
        .filter(|e| e.classification == SeamClassification::NoOwnerRealization)
        .count();
    let realization_only = entries
        .iter()
        .filter(|e| e.classification == SeamClassification::OwnerRealizationOnly)
        .count();
    let realization_feedback = entries
        .iter()
        .filter(|e| e.classification == SeamClassification::OwnerRealizationPlusFeedback)
        .count();
    let surface_alignment = entries
        .iter()
        .filter(|e| e.classification == SeamClassification::SurfaceResultAlignment)
        .count();

    println!();
    println!("## Summary");
    println!("  Total Local/External effects:           {total}");
    println!("  no-owner-realization:                   {no_owner}");
    println!("  owner-realization-only:                 {realization_only}");
    println!("  owner-realization-plus-feedback:        {realization_feedback}");
    println!("  surface-result-alignment:               {surface_alignment}");
    println!();
    println!("## Seams Requiring Formal Handoff Protocols");
    for entry in entries {
        if entry.classification == SeamClassification::OwnerRealizationPlusFeedback {
            println!("  {} :: {}", entry.machine, entry.effect_variant);
        }
    }
    println!();
    println!("## Public Surface Contracts");
    for entry in public_surface_contracts {
        println!(
            "  {:28} {:6} {}",
            entry.name,
            match entry.status {
                ContractStatus::Closed => "closed",
                ContractStatus::Open => "open",
            },
            entry.notes
        );
    }
    println!();
    println!("## Debt");
    println!(
        "  unresolved classification debt:            {}",
        unresolved_classification_debt.len()
    );
    println!(
        "  unresolved protocol debt:                  {}",
        unresolved_protocol_debt.len()
    );
    println!(
        "  unresolved public-surface alignment debt:  {}",
        unresolved_public_surface_alignment_debt.len()
    );
}
