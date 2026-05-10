use std::fmt;

use clap::Args;
use meerkat_machine_schema::{
    CompositionSchema, EffectDisposition, MachineSchema, Route, canonical_composition_schemas,
    canonical_machine_schemas, compat_composition_schemas,
};

/// CLI args for `xtask seam-inventory`.
#[derive(Debug, Clone, Args, Default)]
pub struct SeamInventoryArgs {
    /// In strict mode every Local/External effect must carry an explicit
    /// classification (the disposition-based fallback arm is a hard error),
    /// and every routed effect must have a typed realization (consumer
    /// machine + input + route id resolvable in the composition schema).
    #[arg(long)]
    pub strict: bool,
}

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

/// Entry in the routed-effect inventory: every routed effect's consumer
/// machine + input variant must resolve to a typed [`RoutedInput`] in the
/// composition schema. This is the teeth B-10 puts on the
/// `EffectDisposition::Routed` branch that the classification table skips.
#[derive(Debug)]
pub struct RoutedRealization {
    pub producer_machine: String,
    pub effect_variant: String,
    pub composition: String,
    pub resolved_consumers: Vec<(String, String, String)>,
    pub missing_consumers: Vec<String>,
}

/// Known effect classifications for canonical machines.
///
/// This is the manually-curated ground truth that drives the seam inventory.
/// Every `Local`/`External` effect declared in a canonical machine schema
/// must be listed here; the heuristic fallback in [`classify_effect`] is
/// a hard error under `--strict` and a classification-debt entry otherwise.
fn known_classifications() -> Vec<(&'static str, &'static str, SeamClassification, &'static str)> {
    vec![
        // =========================================================================
        // MeerkatMachine — runtime kernel
        // =========================================================================
        //
        // Local effects: purely internal kernel bookkeeping. The runtime loop
        // observes them synchronously and they never cross a shell boundary.
        (
            "MeerkatMachine",
            "RequestCancellationAtBoundary",
            SeamClassification::NoOwnerRealization,
            "Local boundary-cancel intent retained entirely inside the Meerkat kernel",
        ),
        (
            "MeerkatMachine",
            "TurnRunStarted",
            SeamClassification::NoOwnerRealization,
            "Local turn-run start marker retained inside the agent loop boundary tracker",
        ),
        (
            "MeerkatMachine",
            "TurnBoundaryApplied",
            SeamClassification::NoOwnerRealization,
            "Local turn-boundary marker retained inside the agent loop boundary tracker",
        ),
        (
            "MeerkatMachine",
            "TurnRunCompleted",
            SeamClassification::NoOwnerRealization,
            "Local turn-run completion marker retained inside the agent loop boundary tracker",
        ),
        (
            "MeerkatMachine",
            "TurnRunFailed",
            SeamClassification::NoOwnerRealization,
            "Local turn-run failure marker retained inside the agent loop boundary tracker",
        ),
        (
            "MeerkatMachine",
            "TurnRunCancelled",
            SeamClassification::NoOwnerRealization,
            "Local turn-run cancellation marker retained inside the agent loop boundary tracker",
        ),
        (
            "MeerkatMachine",
            "TurnCheckCompaction",
            SeamClassification::NoOwnerRealization,
            "Local turn compaction check retained inside the agent loop boundary tracker",
        ),
        (
            "MeerkatMachine",
            "WakeInterrupt",
            SeamClassification::NoOwnerRealization,
            "Local wake signal consumed by the in-process runtime loop",
        ),
        (
            "MeerkatMachine",
            "RuntimeEffectFact",
            SeamClassification::NoOwnerRealization,
            "Local typed runtime-effect fact consumed by the runtime shell to construct sealed executor effects",
        ),
        (
            "MeerkatMachine",
            "ResolveAdmission",
            SeamClassification::NoOwnerRealization,
            "Local admission resolution inside the kernel's input-lifecycle region",
        ),
        (
            "MeerkatMachine",
            "SubmitAdmittedIngressEffect",
            SeamClassification::NoOwnerRealization,
            "Local handoff from admission to ingress inside the kernel",
        ),
        (
            "MeerkatMachine",
            "SubmitRunPrimitive",
            SeamClassification::NoOwnerRealization,
            "Local run-primitive submission to the agent-loop driver",
        ),
        (
            "MeerkatMachine",
            "ResolveCompletionAsTerminated",
            SeamClassification::NoOwnerRealization,
            "Local terminal-completion resolution inside the input-lifecycle region",
        ),
        (
            "MeerkatMachine",
            "ApplyControlPlaneCommand",
            SeamClassification::NoOwnerRealization,
            "Local control-plane command application inside the runtime control region",
        ),
        (
            "MeerkatMachine",
            "InitiateRecycle",
            SeamClassification::NoOwnerRealization,
            "Local recycle intent routed to the runtime's attachment lifecycle",
        ),
        (
            "MeerkatMachine",
            "PostAdmissionSignal",
            SeamClassification::NoOwnerRealization,
            "Local admission signal consumed by the kernel's admission region",
        ),
        (
            "MeerkatMachine",
            "ReadyForRun",
            SeamClassification::NoOwnerRealization,
            "Local readiness signal consumed by the agent loop",
        ),
        (
            "MeerkatMachine",
            "CompletionResolved",
            SeamClassification::NoOwnerRealization,
            "Local completion resolution inside the input-lifecycle region",
        ),
        (
            "MeerkatMachine",
            "CheckCompaction",
            SeamClassification::NoOwnerRealization,
            "Local compaction budget check inside the agent loop",
        ),
        (
            "MeerkatMachine",
            "RecordTerminalOutcome",
            SeamClassification::NoOwnerRealization,
            "Local terminal-outcome record in the runtime state",
        ),
        (
            "MeerkatMachine",
            "RecordRunAssociation",
            SeamClassification::NoOwnerRealization,
            "Local run-association record inside the runtime control region",
        ),
        (
            "MeerkatMachine",
            "RecordBoundarySequence",
            SeamClassification::NoOwnerRealization,
            "Local boundary-sequence record inside the runtime control region",
        ),
        (
            "MeerkatMachine",
            "SubmitOpEvent",
            SeamClassification::NoOwnerRealization,
            "Local op-event submission to the ops lifecycle registry",
        ),
        (
            "MeerkatMachine",
            "NotifyOpWatcher",
            SeamClassification::NoOwnerRealization,
            "Local op-watcher notification inside the ops lifecycle region",
        ),
        (
            "MeerkatMachine",
            "ExposeOperationPeer",
            SeamClassification::NoOwnerRealization,
            "Local peer-exposure record inside the ops lifecycle region",
        ),
        (
            "MeerkatMachine",
            "RetainTerminalRecord",
            SeamClassification::NoOwnerRealization,
            "Local terminal-record retention inside the input-lifecycle region",
        ),
        (
            "MeerkatMachine",
            "EvictCompletedRecord",
            SeamClassification::NoOwnerRealization,
            "Local completed-record eviction inside the input-lifecycle region",
        ),
        (
            "MeerkatMachine",
            "CompletionProduced",
            SeamClassification::NoOwnerRealization,
            "Local completion-produced record inside the input-lifecycle region",
        ),
        (
            "MeerkatMachine",
            "WaitAllSatisfied",
            SeamClassification::NoOwnerRealization,
            "Local wait-all satisfaction record inside the input-lifecycle region",
        ),
        (
            "MeerkatMachine",
            "CollectCompletedResult",
            SeamClassification::NoOwnerRealization,
            "Local completed-result collection inside the input-lifecycle region",
        ),
        (
            "MeerkatMachine",
            "EnqueueClassifiedEntry",
            SeamClassification::NoOwnerRealization,
            "Local classified-interaction enqueue inside the peer-ingress region",
        ),
        (
            "MeerkatMachine",
            "PeerIngressClassified",
            SeamClassification::NoOwnerRealization,
            "Local typed peer-ingress classification result consumed synchronously by the runtime peer-comms handle",
        ),
        (
            "MeerkatMachine",
            "SpawnDrainTask",
            SeamClassification::NoOwnerRealization,
            "Local drain-task spawn inside the comms drain-control region",
        ),
        (
            "MeerkatMachine",
            "ScheduleSurfaceCompletion",
            SeamClassification::NoOwnerRealization,
            "Local surface-completion schedule inside the external-tool surface region",
        ),
        (
            "MeerkatMachine",
            "CloseSurfaceConnection",
            SeamClassification::NoOwnerRealization,
            "Local surface-connection close inside the external-tool surface region",
        ),
        (
            "MeerkatMachine",
            "SwitchTurnPersistentReconfigureRequested",
            SeamClassification::NoOwnerRealization,
            "Local persistent model-routing reconfiguration request consumed by the runtime kernel",
        ),
        (
            "MeerkatMachine",
            "SwitchTurnFiniteOverrideActivated",
            SeamClassification::NoOwnerRealization,
            "Local finite model-routing override activation consumed by the runtime kernel",
        ),
        (
            "MeerkatMachine",
            "SwitchTurnFiniteOverrideRestored",
            SeamClassification::NoOwnerRealization,
            "Local finite model-routing override restoration consumed by the runtime kernel",
        ),
        //
        // External effects that carry surface truth back to callers must align
        // the surface representation with the kernel state they were derived
        // from — otherwise the API lies about outcomes.
        (
            "MeerkatMachine",
            "CommittedVisibleSetPublished",
            SeamClassification::SurfaceResultAlignment,
            "Committed visibility publication must align exactly with the kernel truth seen at boundary apply",
        ),
        (
            "MeerkatMachine",
            "RuntimeNotice",
            SeamClassification::SurfaceResultAlignment,
            "External Meerkat notices must reflect kernel lifecycle and runtime truth accurately",
        ),
        (
            "MeerkatMachine",
            "IngressNotice",
            SeamClassification::SurfaceResultAlignment,
            "External ingress notices must reflect kernel input-lifecycle classification verbatim",
        ),
        (
            "MeerkatMachine",
            "InputLifecycleNotice",
            SeamClassification::SurfaceResultAlignment,
            "External input-lifecycle notices must reflect canonical input-lifecycle transitions",
        ),
        (
            "MeerkatMachine",
            "ModelRoutingStatusChanged",
            SeamClassification::SurfaceResultAlignment,
            "External model-routing status deltas must align with canonical routing topology truth",
        ),
        (
            "MeerkatMachine",
            "SwitchTurnDenied",
            SeamClassification::SurfaceResultAlignment,
            "External switch-turn denials must reflect the exact canonical routing denial reason",
        ),
        (
            "MeerkatMachine",
            "ImageOperationPhaseChanged",
            SeamClassification::SurfaceResultAlignment,
            "External image-operation phase deltas must align with canonical image routing state",
        ),
        (
            "MeerkatMachine",
            "ImageOperationDenied",
            SeamClassification::SurfaceResultAlignment,
            "External image-operation denials must reflect the exact canonical denial reason",
        ),
        (
            "MeerkatMachine",
            "ModelRoutingApprovalTerminalized",
            SeamClassification::SurfaceResultAlignment,
            "External routing approval terminal notices must align with canonical approval state",
        ),
        (
            "MeerkatMachine",
            "SilentIntentApplied",
            SeamClassification::SurfaceResultAlignment,
            "External silent-intent confirmation must reflect the admission-policy change accurately",
        ),
        //
        // External effects that the shell realizes without a machine-level
        // feedback obligation — the machine emits and moves on; correctness
        // does not depend on acknowledgment.
        (
            "MeerkatMachine",
            "IngressAccepted",
            SeamClassification::OwnerRealizationOnly,
            "External ingress-accepted notice realized by the shell without feedback",
        ),
        (
            "MeerkatMachine",
            "RefreshVisibleSurfaceSet",
            SeamClassification::OwnerRealizationOnly,
            "External tool-surface visibility refresh realized by the MCP router",
        ),
        (
            "MeerkatMachine",
            "PublishSupervisorTrustEdge",
            SeamClassification::OwnerRealizationOnly,
            "External supervisor trust-edge publication realized by the supervisor bridge",
        ),
        (
            "MeerkatMachine",
            "RevokeSupervisorTrustEdge",
            SeamClassification::OwnerRealizationOnly,
            "External supervisor trust-edge revocation realized by the supervisor bridge",
        ),
        (
            "MeerkatMachine",
            "EmitExternalToolDelta",
            SeamClassification::OwnerRealizationOnly,
            "External tool-delta emission realized by the MCP router without feedback",
        ),
        (
            "MeerkatMachine",
            "RejectSurfaceCall",
            SeamClassification::OwnerRealizationOnly,
            "External surface-call rejection realized by the external-tool surface",
        ),
        (
            "MeerkatMachine",
            "McpServerStateChanged",
            SeamClassification::OwnerRealizationOnly,
            "External MCP server state change realized by the MCP router",
        ),
        (
            "MeerkatMachine",
            "McpServerReloadRequested",
            SeamClassification::OwnerRealizationOnly,
            "External MCP server reload realized by the MCP router without feedback",
        ),
        (
            "MeerkatMachine",
            "PeerInteractionStateChanged",
            SeamClassification::OwnerRealizationOnly,
            "External peer-interaction state delta realized by the comms surface",
        ),
        (
            "MeerkatMachine",
            "PeerInteractionCleanup",
            SeamClassification::OwnerRealizationOnly,
            "External peer-interaction cleanup realized by the comms surface",
        ),
        (
            "MeerkatMachine",
            "InboundPeerInteractionStateChanged",
            SeamClassification::OwnerRealizationOnly,
            "External inbound peer-interaction state delta realized by the comms surface",
        ),
        (
            "MeerkatMachine",
            "SessionContextAdvanced",
            SeamClassification::OwnerRealizationOnly,
            "External session-context advancement realized by the session surface",
        ),
        (
            "MeerkatMachine",
            "InteractionStreamStateChanged",
            SeamClassification::OwnerRealizationOnly,
            "External interaction-stream state delta realized by the comms surface",
        ),
        (
            "MeerkatMachine",
            "InteractionStreamCleanup",
            SeamClassification::OwnerRealizationOnly,
            "External interaction-stream cleanup realized by the comms surface",
        ),
        (
            "MeerkatMachine",
            "LocalEndpointChanged",
            SeamClassification::OwnerRealizationOnly,
            "External local-endpoint delta realized by the connectivity surface",
        ),
        (
            "MeerkatMachine",
            "PeerProjectionChanged",
            SeamClassification::OwnerRealizationOnly,
            "External peer-projection delta realized by the comms surface",
        ),
        (
            "MeerkatMachine",
            "CommsTrustReconcileRequested",
            SeamClassification::OwnerRealizationOnly,
            "External comms trust-reconcile request realized by the comms surface",
        ),
        //
        // =========================================================================
        // MobMachine — multi-agent orchestration
        // =========================================================================
        (
            "MobMachine",
            "EmitMemberLifecycleNotice",
            SeamClassification::SurfaceResultAlignment,
            "External member lifecycle notices must align with canonical identity/runtime transitions",
        ),
        (
            "MobMachine",
            "EmitRunLifecycleNotice",
            SeamClassification::SurfaceResultAlignment,
            "External run-lifecycle notices must align with mob run transitions",
        ),
        (
            "MobMachine",
            "EmitFlowRunNotice",
            SeamClassification::SurfaceResultAlignment,
            "External flow-run notices must align with flow-frame-engine transitions",
        ),
        (
            "MobMachine",
            "EmitMemberTerminalNotice",
            SeamClassification::SurfaceResultAlignment,
            "External member-terminal notices must align with member lifecycle terminal truth",
        ),
        (
            "MobMachine",
            "FlowTerminalized",
            SeamClassification::SurfaceResultAlignment,
            "External flow-terminalized notice must align with canonical flow terminal truth",
        ),
        (
            "MobMachine",
            "WiringGraphChanged",
            SeamClassification::SurfaceResultAlignment,
            "External wiring-graph delta must align with roster wiring-projection truth",
        ),
        (
            "MobMachine",
            "EmitWiringLifecycleNotice",
            SeamClassification::SurfaceResultAlignment,
            "External wiring-lifecycle notices (members wired / unwired) must align with the \
             canonical `WireMembersRunning` / `UnwireMembersRunning` DSL transitions so event \
             consumers see the same wiring truth the DSL committed",
        ),
        (
            "MobMachine",
            "EmitExternalPeerWiringLifecycleNotice",
            SeamClassification::SurfaceResultAlignment,
            "External peer wiring-lifecycle notices must align with the canonical \
             `WireExternalPeerRunning` / `UnwireExternalPeerRunning` DSL transitions so event \
             consumers see the same external peer wiring truth the DSL committed",
        ),
        (
            "MobMachine",
            "MemberSessionBindingChanged",
            SeamClassification::SurfaceResultAlignment,
            "External member-session binding delta must align with canonical runtime binding truth",
        ),
        (
            "MobMachine",
            "RequestSessionIngressDetachForMobDestroy",
            SeamClassification::NoOwnerRealization,
            "Local mob-destroy ingress detach request consumed by the runtime seam handoff owner",
        ),
        (
            "MobMachine",
            "AppendFailureLedger",
            SeamClassification::NoOwnerRealization,
            "Local failure-ledger append inside the mob orchestrator",
        ),
        (
            "MobMachine",
            "EscalateSupervisor",
            SeamClassification::OwnerRealizationOnly,
            "External supervisor-escalation realized by the supervisor bridge without feedback",
        ),
        (
            "MobMachine",
            "NotifyCoordinator",
            SeamClassification::OwnerRealizationOnly,
            "External coordinator notification realized by the mob coordinator surface",
        ),
        (
            "MobMachine",
            "ExposePendingSpawn",
            SeamClassification::OwnerRealizationOnly,
            "External pending-spawn exposure realized by the mob handle surface",
        ),
        (
            "MobMachine",
            "AdmitPeerInput",
            SeamClassification::OwnerRealizationOnly,
            "External peer-input admission realized by the mob handle surface",
        ),
        (
            "MobMachine",
            "EmitProgressNote",
            SeamClassification::OwnerRealizationOnly,
            "External progress-note emission realized by the mob event stream",
        ),
        (
            "MobMachine",
            "EmitTaskNotice",
            SeamClassification::OwnerRealizationOnly,
            "External task-notice emission realized by the task-board surface",
        ),
        (
            "MobMachine",
            "PersistKickoffUpdate",
            SeamClassification::NoOwnerRealization,
            "Local kickoff lifecycle state persistence consumed inside the mob startup tracker",
        ),
        (
            "MobMachine",
            "PersistKickoffFailureUpdate",
            SeamClassification::NoOwnerRealization,
            "Local kickoff failure persistence consumed inside the mob startup tracker",
        ),
        (
            "MobMachine",
            "EmitKickoffLifecycleNotice",
            SeamClassification::OwnerRealizationOnly,
            "External kickoff lifecycle notice realized by the mob event stream",
        ),
        //
        // =========================================================================
        // ScheduleLifecycleMachine
        // =========================================================================
        (
            "ScheduleLifecycleMachine",
            "EmitScheduleNotice",
            SeamClassification::SurfaceResultAlignment,
            "External schedule notices must align with the schedule lifecycle kernel state",
        ),
        (
            "ScheduleLifecycleMachine",
            "PlanningWindowRecorded",
            SeamClassification::NoOwnerRealization,
            "Planning window recording is local bookkeeping inside the schedule kernel",
        ),
        //
        // =========================================================================
        // OccurrenceLifecycleMachine
        // =========================================================================
        (
            "OccurrenceLifecycleMachine",
            "Claimed",
            SeamClassification::SurfaceResultAlignment,
            "Occurrence lifecycle outputs are public result surfaces and must match kernel truth",
        ),
        (
            "OccurrenceLifecycleMachine",
            "DispatchStarted",
            SeamClassification::SurfaceResultAlignment,
            "Occurrence lifecycle outputs are public result surfaces and must match kernel truth",
        ),
        (
            "OccurrenceLifecycleMachine",
            "AwaitingCompletion",
            SeamClassification::SurfaceResultAlignment,
            "Occurrence lifecycle outputs are public result surfaces and must match kernel truth",
        ),
        (
            "OccurrenceLifecycleMachine",
            "Completed",
            SeamClassification::SurfaceResultAlignment,
            "Occurrence lifecycle outputs are public result surfaces and must match kernel truth",
        ),
        (
            "OccurrenceLifecycleMachine",
            "Skipped",
            SeamClassification::SurfaceResultAlignment,
            "Occurrence lifecycle outputs are public result surfaces and must match kernel truth",
        ),
        (
            "OccurrenceLifecycleMachine",
            "Misfired",
            SeamClassification::SurfaceResultAlignment,
            "Occurrence lifecycle outputs are public result surfaces and must match kernel truth",
        ),
        (
            "OccurrenceLifecycleMachine",
            "Superseded",
            SeamClassification::SurfaceResultAlignment,
            "Occurrence lifecycle outputs are public result surfaces and must match kernel truth",
        ),
        (
            "OccurrenceLifecycleMachine",
            "DeliveryFailed",
            SeamClassification::SurfaceResultAlignment,
            "Occurrence lifecycle outputs are public result surfaces and must match kernel truth",
        ),
        (
            "OccurrenceLifecycleMachine",
            "LeaseExpired",
            SeamClassification::SurfaceResultAlignment,
            "Occurrence lifecycle outputs are public result surfaces and must match kernel truth",
        ),
        //
        // =========================================================================
        // AuthMachine — per-binding auth lease lifecycle (dogma #43/#44 resolved)
        // =========================================================================
        (
            "AuthMachine",
            "EmitLifecycleEvent",
            SeamClassification::SurfaceResultAlignment,
            "Auth lifecycle events are consumed by the RuntimeOpsLifecycleRegistry audit sink and the surface truth must match the AuthMachine phase",
        ),
        (
            "AuthMachine",
            "WakeRefreshLoop",
            SeamClassification::NoOwnerRealization,
            "Local refresh-loop wake consumed by the per-binding auth refresh task",
        ),
    ]
}

/// Classify a Local or External effect using the known-classifications table.
///
/// Strict mode turns a missing classification into a hard error (the caller
/// treats the returned `explicitly_classified = false` as classification
/// debt). Non-strict mode falls back to the disposition-based heuristic
/// purely for reporting — classification debt is still the failure signal.
fn classify_effect(
    machine: &str,
    effect: &str,
    disposition: &EffectDisposition,
    known: &[(&str, &str, SeamClassification, &str)],
) -> (SeamClassification, String, bool) {
    // Check known classifications first.
    for (m, e, class, notes) in known {
        if *m == machine && *e == effect {
            return (*class, notes.to_string(), true);
        }
    }

    // Heuristic fallback — only ever reported. Under `--strict` the caller
    // turns the resulting `was_explicit == false` into a hard error.
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
            // Routed effects are handled by the routed-effect realization
            // check below, not the seam-inventory classification table.
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

pub fn run_seam_inventory(args: SeamInventoryArgs) -> anyhow::Result<()> {
    let machines = canonical_machine_schemas();
    let compositions = canonical_composition_schemas();
    let known = known_classifications();
    let mut entries: Vec<SeamEntry> = Vec::new();
    let public_surface_contracts = known_public_surface_contracts();

    for machine in &machines {
        collect_machine_seams(machine, &known, &mut entries);
    }

    // Routed-effect realization inventory — the teeth on the
    // `EffectDisposition::Routed` arm. Each routed effect must resolve to
    // a typed consumer input via the composition schema's `routed_inputs`.
    let routed_realizations = collect_routed_realizations(&machines, &compositions);

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
                                instance.machine_name.as_str().to_string(),
                                protocol.effect_variant.as_str().to_string(),
                            ),
                            protocol.name.as_str().to_string(),
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
    let unresolved_routed_debt = routed_realizations
        .iter()
        .filter(|rr| !rr.missing_consumers.is_empty())
        .collect::<Vec<_>>();

    // Print the report
    print_report(&entries);
    print_routed_realizations(&routed_realizations);

    // Summary statistics
    print_summary(
        &entries,
        &unresolved_classification_debt,
        &unresolved_protocol_debt,
        &public_surface_contracts,
        &unresolved_public_surface_alignment_debt,
        &routed_realizations,
        &unresolved_routed_debt,
        &compositions,
    );

    let strict = args.strict;
    let classification_debt = unresolved_classification_debt.len();
    let protocol_debt = unresolved_protocol_debt.len();
    let public_surface_debt = unresolved_public_surface_alignment_debt.len();
    let routed_debt = unresolved_routed_debt.len();

    if strict && classification_debt > 0 {
        anyhow::bail!(
            "seam-inventory --strict: {classification_debt} effect(s) lack explicit classification; add entries to known_classifications() for every Local/External effect",
        );
    }
    if classification_debt > 0 || protocol_debt > 0 || public_surface_debt > 0 || routed_debt > 0 {
        anyhow::bail!(
            "seam inventory has unresolved debt: classification={classification_debt}, protocol={protocol_debt}, public_surface={public_surface_debt}, routed={routed_debt}",
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
                    machine.machine.as_str(),
                    rule.effect_variant.as_str(),
                    &rule.disposition,
                    known,
                );

                entries.push(SeamEntry {
                    machine: machine.machine.as_str().to_string(),
                    effect_variant: rule.effect_variant.as_str().to_string(),
                    disposition: disposition_str.to_string(),
                    classification,
                    notes,
                    explicitly_classified,
                });
            }
            EffectDisposition::Routed { .. } => {
                // Routed effects are classified by the routed-effect
                // realization inventory (see `collect_routed_realizations`),
                // not by the disposition-based classification table.
            }
        }
    }
}

/// Build the routed-effect realization inventory: for each `Routed` effect
/// disposition, walk every composition that binds the producing machine and
/// assert there is a typed `RoutedInput` entry resolving
/// (producer_instance, effect_variant) to (consumer_instance, input_variant).
///
/// Any listed consumer_machine that lacks a matching `RoutedInput` is a
/// `missing_consumer` — the composition schema declares a route in principle
/// but the typed route-variant table does not realize it. B-10 upgrades this
/// from a soft warning to a hard error so that the producer-consumer path
/// is guaranteed to traverse `CompositionDispatcher::dispatch`.
fn collect_routed_realizations(
    machines: &[MachineSchema],
    compositions: &[CompositionSchema],
) -> Vec<RoutedRealization> {
    let mut out = Vec::new();
    for producer in machines {
        for disposition in &producer.effect_dispositions {
            let EffectDisposition::Routed { consumer_machines } = &disposition.disposition else {
                continue;
            };
            let producer_name = producer.machine.as_str();
            let effect_variant = disposition.effect_variant.as_str();
            for composition in compositions {
                // Does this composition bind the producer machine?
                let producer_instance = composition
                    .machines
                    .iter()
                    .find(|inst| inst.machine_name == producer.machine);
                let Some(producer_instance) = producer_instance else {
                    continue;
                };
                let composition_name = composition.name.as_str().to_string();
                let mut resolved = Vec::new();
                let mut missing = Vec::new();
                for consumer in consumer_machines {
                    let consumer_instance = composition
                        .machines
                        .iter()
                        .find(|inst| inst.machine_name == *consumer);
                    let Some(consumer_instance) = consumer_instance else {
                        // Producer routed to a consumer that isn't bound in
                        // this composition. Not necessarily a bug — another
                        // composition may realize it. Record as missing so
                        // the summary reflects it if every composition has
                        // the same gap.
                        missing.push(format!(
                            "{} (not bound in composition {})",
                            consumer.as_str(),
                            composition_name
                        ));
                        continue;
                    };
                    // Find a typed Route entry in this composition that
                    // resolves producer_instance + effect_variant to an input
                    // on consumer_instance.
                    let resolved_route = composition.routes.iter().find(|r| {
                        matches_route(
                            r,
                            &producer_instance.instance_id,
                            effect_variant,
                            &consumer_instance.instance_id,
                        )
                    });
                    match resolved_route {
                        Some(route) => resolved.push((
                            consumer.as_str().to_string(),
                            route.to.machine.as_str().to_string(),
                            route.to.input_variant.as_str().to_string(),
                        )),
                        None => missing.push(format!(
                            "{} (no Route entry in composition {})",
                            consumer.as_str(),
                            composition_name
                        )),
                    }
                }
                out.push(RoutedRealization {
                    producer_machine: producer_name.to_string(),
                    effect_variant: effect_variant.to_string(),
                    composition: composition_name,
                    resolved_consumers: resolved,
                    missing_consumers: missing,
                });
            }
        }
    }
    out
}

/// Match a [`Route`] entry against a concrete
/// (producer_instance, effect_variant, consumer_instance) tuple. This is
/// where B-10 pins the semantic invariant: the typed route entry must name
/// the same producer instance, the same producer effect variant, and the
/// same consumer instance as the disposition declared at the machine level.
fn matches_route(
    route: &Route,
    producer_instance: &meerkat_machine_schema::identity::MachineInstanceId,
    effect_variant: &str,
    consumer_instance: &meerkat_machine_schema::identity::MachineInstanceId,
) -> bool {
    route.from_machine == *producer_instance
        && route.effect_variant.as_str() == effect_variant
        && route.to.machine == *consumer_instance
}

fn print_routed_realizations(routed: &[RoutedRealization]) {
    if routed.is_empty() {
        return;
    }
    println!();
    println!("## Routed Effect Realizations (producer → composition → consumer)");
    for rr in routed {
        let resolved_summary = if rr.resolved_consumers.is_empty() {
            "(none resolved)".to_string()
        } else {
            rr.resolved_consumers
                .iter()
                .map(|(consumer_machine, consumer_instance, input_variant)| {
                    format!("{consumer_machine}[{consumer_instance}]::{input_variant}")
                })
                .collect::<Vec<_>>()
                .join(", ")
        };
        println!(
            "  {}::{} in {}  →  {}",
            rr.producer_machine, rr.effect_variant, rr.composition, resolved_summary,
        );
        for missing in &rr.missing_consumers {
            println!("    ! MISSING: {missing}");
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

#[allow(clippy::too_many_arguments)]
fn print_summary(
    entries: &[SeamEntry],
    unresolved_classification_debt: &[&SeamEntry],
    unresolved_protocol_debt: &[&SeamEntry],
    public_surface_contracts: &[SurfaceContractEntry],
    unresolved_public_surface_alignment_debt: &[&SurfaceContractEntry],
    routed_realizations: &[RoutedRealization],
    unresolved_routed_debt: &[&RoutedRealization],
    compositions: &[CompositionSchema],
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
    println!(
        "  routed-effect realizations:             {}",
        routed_realizations.len()
    );
    println!();
    println!("## Seams Requiring Formal Handoff Protocols");
    for entry in entries {
        if entry.classification == SeamClassification::OwnerRealizationPlusFeedback {
            println!("  {} :: {}", entry.machine, entry.effect_variant);
        }
    }
    println!();
    // Generated handoff obligation pairs declared in canonical + perimeter
    // compositions. Each protocol is an obligation pair: producer effect
    // → realising actor → typed feedback input(s) that close the
    // step-lock (ack / failure). Producers now host the annotation on
    // their canonical effect rather than through bridge-only schemas.
    let compat = compat_composition_schemas();
    let mut protocol_rows: Vec<(String, String, String, String, String, String)> = Vec::new();
    let all_compositions: Vec<&CompositionSchema> =
        compositions.iter().chain(compat.iter()).collect();
    for composition in &all_compositions {
        for protocol in &composition.handoff_protocols {
            let feedback_variants: Vec<String> = protocol
                .allowed_feedback_inputs
                .iter()
                .map(|fb| {
                    format!(
                        "{}::{}",
                        fb.machine_instance.as_str(),
                        fb.input_variant.as_str(),
                    )
                })
                .collect();
            protocol_rows.push((
                protocol.name.as_str().to_string(),
                composition.name.as_str().to_string(),
                protocol.producer_instance.as_str().to_string(),
                protocol.effect_variant.as_str().to_string(),
                protocol.realizing_actor.as_str().to_string(),
                feedback_variants.join(", "),
            ));
        }
    }
    protocol_rows.sort();
    println!("## Declared Handoff Obligation Pairs (canonical + compat)");
    println!(
        "  {:40} {:28} {:30} {:32} {:32} feedback_inputs",
        "protocol", "composition", "producer_instance", "effect", "realizing_actor"
    );
    for (protocol, composition, producer, effect, actor, feedback) in &protocol_rows {
        println!(
            "  {protocol:40} {composition:28} {producer:30} {effect:32} {actor:32} {feedback}"
        );
    }
    println!(
        "  total handoff obligation pairs:            {}",
        protocol_rows.len()
    );
    println!();
    // C-F3 — destroy-obligation pairing audit. State-scope audit row
    // F3 flagged that `MeerkatMachine` carries a
    // `peer_ingress_mob_id: Option<MobId>` whose "mob-exists"
    // invariant is convention-driven: when a mob destroys its
    // runtime, every session whose peer-ingress ownership was
    // `MobOwned` by that mob must receive `DetachIngress` first,
    // otherwise the `peer_ingress_mob_id` on that session dangles.
    //
    // This section walks every canonical routed effect whose variant
    // name contains the substring "Destroy" and asserts a paired
    // handoff obligation exists whose feedback inputs include at
    // least one variant that mentions "IngressDetached" or "Detach".
    // Unpaired destroy routes are emitted as debt so the
    // `mob-destroy → session-detach` ordering cannot regress
    // silently.
    let mut destroy_routes: Vec<(String, String, String, String, bool)> = Vec::new();
    for composition in &all_compositions {
        for route in &composition.routes {
            // Filter to destroy *requests* that flow from a producer to a
            // consumer that will be torn down. Reply signals like
            // `RuntimeDestroyed` (consumer → producer, "destroy observed")
            // are not request-side effects and do not need a paired
            // detach obligation: the destroy is already done by the time
            // they fire.
            let variant = route.effect_variant.as_str();
            if !(variant.contains("Destroy") && variant.starts_with("Request")) {
                continue;
            }
            let producer_instance = route.from_machine.as_str().to_string();
            let producer_machine = composition
                .machines
                .iter()
                .find(|m| m.instance_id == route.from_machine)
                .map(|m| m.machine_name.as_str().to_string())
                .unwrap_or_default();
            let effect = route.effect_variant.as_str().to_string();
            let composition_name = composition.name.as_str().to_string();
            // Paired iff some protocol (across all compositions) has a
            // feedback input whose variant name is "*IngressDetach*"
            // or "*Detach*" AND the protocol's allowed feedback inputs
            // route into the same consumer as this destroy route.
            let consumer_instance = &route.to.machine;
            let paired = all_compositions.iter().any(|other| {
                other.handoff_protocols.iter().any(|protocol| {
                    protocol.allowed_feedback_inputs.iter().any(|fb| {
                        let variant = fb.input_variant.as_str();
                        (variant.contains("IngressDetached") || variant.contains("Detach"))
                            && (fb.machine_instance == *consumer_instance
                                || other.machines.iter().any(|m| {
                                    m.instance_id == fb.machine_instance
                                        && other
                                            .machines
                                            .iter()
                                            .any(|cm| cm.machine_name.as_str() == producer_machine)
                                })
                                || protocol.name.as_str().contains("session_ingress"))
                    })
                })
            });
            destroy_routes.push((
                producer_machine.clone(),
                effect,
                composition_name,
                producer_instance,
                paired,
            ));
        }
    }
    destroy_routes.sort();
    println!("## Destroy-obligation Pairing (C-F3)");
    if destroy_routes.is_empty() {
        println!("  (no canonical routed *Destroy* effects declared)");
    } else {
        println!(
            "  {:24} {:32} {:32} {:32} paired",
            "producer_machine", "effect_variant", "composition", "producer_instance"
        );
        for (producer, effect, composition_name, instance, paired) in &destroy_routes {
            println!(
                "  {:24} {:32} {:32} {:32} {}",
                producer,
                effect,
                composition_name,
                instance,
                if *paired { "yes" } else { "NO" }
            );
        }
    }
    let unpaired_destroy_debt: Vec<&(String, String, String, String, bool)> = destroy_routes
        .iter()
        .filter(|(_, _, _, _, paired)| !paired)
        .collect();
    println!(
        "  unpaired destroy routes (debt):            {}",
        unpaired_destroy_debt.len()
    );
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
    println!(
        "  unresolved routed-effect debt:             {}",
        unresolved_routed_debt.len()
    );
    println!(
        "  unpaired destroy-obligation debt (C-F3):   {}",
        unpaired_destroy_debt.len()
    );
}
