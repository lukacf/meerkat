use std::collections::BTreeMap;

use meerkat_machine_schema::canonical_machine_schemas;

#[derive(Debug, Clone)]
pub struct AuditPolicy {
    pub projection_types: Vec<ProjectionTypeRule>,
    pub authority_modules: Vec<AuthorityModuleRule>,
    pub protected_fields: Vec<ProtectedFieldRule>,
    pub routed_effect_realizations: Vec<RoutedEffectRealizationRule>,
    pub required_live_symbols: Vec<RequiredLiveSymbolRule>,
    pub handoff_protocol_coverage: Vec<HandoffProtocolCoverageRule>,
    pub protocol_realization_sites: Vec<ProtocolRealizationSiteRule>,
    pub protocol_feedback_constraints: Vec<ProtocolFeedbackConstraintRule>,
    pub terminal_mapping_constraints: Vec<TerminalMappingConstraintRule>,
}

impl AuditPolicy {
    pub fn load() -> Self {
        Self {
            projection_types: vec![
                ProjectionTypeRule::new(
                    "LoopState",
                    &["transition", "can_transition_to", "force_transition"],
                ),
                ProjectionTypeRule::new("RuntimeState", &["transition", "can_transition_to"]),
                ProjectionTypeRule::new(
                    "RuntimeStateMachine",
                    &[
                        "start_run",
                        "complete_run",
                        "attach",
                        "detach",
                        "reset_to_idle",
                    ],
                ),
            ],
            authority_modules: vec![
                AuthorityModuleRule::new("meerkat-mob/src/runtime/mob_lifecycle_authority.rs"),
                AuthorityModuleRule::new("meerkat-mob/src/runtime/mob_orchestrator_authority.rs"),
                AuthorityModuleRule::new("meerkat-mob/src/runtime/flow_run_kernel.rs"),
                AuthorityModuleRule::new("meerkat-runtime/src/runtime_ingress_authority.rs"),
                AuthorityModuleRule::new("meerkat-runtime/src/ops_lifecycle_authority.rs"),
                AuthorityModuleRule::new("meerkat-core/src/turn_execution_authority.rs"),
                AuthorityModuleRule::new("meerkat-mcp/src/external_tool_surface_authority.rs"),
                AuthorityModuleRule::new("meerkat-runtime/src/runtime_control_authority.rs"),
                AuthorityModuleRule::new("meerkat-comms/src/peer_comms_authority.rs"),
            ],
            protected_fields: vec![
                ProtectedFieldRule::new(
                    "wake_requested",
                    &[
                        "process_ingress_effects",
                        "abandon_pending_inputs",
                        "forget_input",
                        "reset",
                    ],
                ),
                ProtectedFieldRule::new(
                    "process_requested",
                    &[
                        "process_ingress_effects",
                        "abandon_pending_inputs",
                        "forget_input",
                        "reset",
                    ],
                ),
                ProtectedFieldRule::new("comms_drain_active", &["set_comms_drain_active"]),
            ],
            routed_effect_realizations: default_routed_effect_realizations(),
            required_live_symbols: vec![],
            handoff_protocol_coverage: default_handoff_protocol_coverage(),
            protocol_realization_sites: default_protocol_realization_sites(),
            protocol_feedback_constraints: default_protocol_feedback_constraints(),
            terminal_mapping_constraints: default_terminal_mapping_constraints(),
        }
    }

    pub fn required_route_keys(
        &self,
    ) -> BTreeMap<(String, String), Vec<&RoutedEffectRealizationRule>> {
        let mut map: BTreeMap<(String, String), Vec<&RoutedEffectRealizationRule>> =
            BTreeMap::new();
        for rule in &self.routed_effect_realizations {
            map.entry((rule.producer_machine.clone(), rule.effect_variant.clone()))
                .or_default()
                .push(rule);
        }
        map
    }
}

#[derive(Debug, Clone)]
pub struct ProjectionTypeRule {
    pub type_name: &'static str,
    pub forbidden_methods: Vec<&'static str>,
}

impl ProjectionTypeRule {
    fn new(type_name: &'static str, forbidden_methods: &[&'static str]) -> Self {
        Self {
            type_name,
            forbidden_methods: forbidden_methods.to_vec(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AuthorityModuleRule {
    pub path_suffix: &'static str,
}

impl AuthorityModuleRule {
    fn new(path_suffix: &'static str) -> Self {
        Self { path_suffix }
    }
}

#[derive(Debug, Clone)]
pub struct ProtectedFieldRule {
    pub field_name: &'static str,
    pub allowed_writers: Vec<&'static str>,
}

impl ProtectedFieldRule {
    fn new(field_name: &'static str, allowed_writers: &[&'static str]) -> Self {
        Self {
            field_name,
            allowed_writers: allowed_writers.to_vec(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct RoutedEffectRealizationRule {
    pub producer_machine: String,
    pub effect_variant: String,
    pub consumer_machine: String,
    pub consumer_input: String,
    pub allowed_paths: Vec<&'static str>,
}

impl RoutedEffectRealizationRule {
    fn new(
        producer_machine: impl Into<String>,
        effect_variant: impl Into<String>,
        consumer_machine: impl Into<String>,
        consumer_input: impl Into<String>,
        allowed_paths: &[&'static str],
    ) -> Self {
        Self {
            producer_machine: producer_machine.into(),
            effect_variant: effect_variant.into(),
            consumer_machine: consumer_machine.into(),
            consumer_input: consumer_input.into(),
            allowed_paths: allowed_paths.to_vec(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct RequiredLiveSymbolRule {
    pub symbol: &'static str,
}

fn default_routed_effect_realizations() -> Vec<RoutedEffectRealizationRule> {
    let mut rules = Vec::new();
    for schema in canonical_machine_schemas() {
        for disposition in &schema.effect_dispositions {
            let meerkat_machine_schema::EffectDisposition::Routed { consumer_machines } =
                &disposition.disposition
            else {
                continue;
            };
            for consumer_machine in consumer_machines {
                let consumer_input = default_consumer_input(
                    &schema.machine,
                    &disposition.effect_variant,
                    consumer_machine,
                );
                rules.push(RoutedEffectRealizationRule::new(
                    schema.machine.clone(),
                    disposition.effect_variant.clone(),
                    consumer_machine.clone(),
                    consumer_input,
                    &default_allowed_paths(&schema.machine, consumer_machine),
                ));
            }
        }
    }
    rules
}

fn default_consumer_input(producer: &str, effect_variant: &str, consumer: &str) -> String {
    match (producer, effect_variant, consumer) {
        ("OpsLifecycleMachine", "ExposeOperationPeer", "PeerCommsMachine") => {
            "TrustPeer".to_string()
        }
        ("FlowRunMachine", "AdmitStepWork", "RuntimeControlMachine") => {
            "SubmitIngressEffect".to_string()
        }
        ("FlowRunMachine", "EscalateSupervisor", "MobOrchestratorMachine") => {
            "EscalateSupervisor".to_string()
        }
        ("FlowRunMachine", "FlowTerminalized", "MobOrchestratorMachine") => {
            "MarkCompleted".to_string()
        }
        ("MobLifecycleMachine", "RequestCleanup", "MobOrchestratorMachine") => "Stop".to_string(),
        ("MobOrchestratorMachine", "ActivateSupervisor", "MobLifecycleMachine") => {
            "Start".to_string()
        }
        ("MobOrchestratorMachine", "DeactivateSupervisor", "MobLifecycleMachine") => {
            "Stop".to_string()
        }
        ("MobOrchestratorMachine", "FlowActivated", "FlowRunMachine") => "StartRun".to_string(),
        ("MobOrchestratorMachine", "FlowActivated", "MobLifecycleMachine") => {
            "StartRun".to_string()
        }
        ("MobOrchestratorMachine", "FlowDeactivated", "MobLifecycleMachine") => {
            "FinishRun".to_string()
        }
        ("MobOrchestratorMachine", "MemberForceCancelled", "RuntimeControlMachine") => {
            "CancelRequested".to_string()
        }
        ("PeerCommsMachine", "SubmitPeerInputCandidate", "RuntimeControlMachine") => {
            "SubmitIngressEffect".to_string()
        }
        ("RuntimeControlMachine", "SubmitAdmittedIngressEffect", "RuntimeIngressMachine") => {
            "AdmitRecovered".to_string()
        }
        ("RuntimeControlMachine", "SubmitRunPrimitive", "TurnExecutionMachine") => {
            "BeginTurn".to_string()
        }
        ("RuntimeIngressMachine", "ReadyForRun", "RuntimeControlMachine") => {
            "BeginRunFromIngress".to_string()
        }
        ("TurnExecutionMachine", "BoundaryApplied", "RuntimeIngressMachine") => {
            "BoundaryApplied".to_string()
        }
        ("TurnExecutionMachine", "RunCompleted", "RuntimeIngressMachine") => {
            "RunCompleted".to_string()
        }
        ("TurnExecutionMachine", "RunCompleted", "RuntimeControlMachine") => {
            "RunCompleted".to_string()
        }
        ("TurnExecutionMachine", "RunFailed", "RuntimeIngressMachine") => "RunFailed".to_string(),
        ("TurnExecutionMachine", "RunFailed", "RuntimeControlMachine") => "RunFailed".to_string(),
        ("TurnExecutionMachine", "RunCancelled", "RuntimeIngressMachine") => {
            "RunCancelled".to_string()
        }
        ("TurnExecutionMachine", "RunCancelled", "RuntimeControlMachine") => {
            "RunCancelled".to_string()
        }
        _ => format!("{producer}:{effect_variant}->{consumer}"),
    }
}

/// Rule: every effect with `handoff_protocol: Some(name)` must have a matching
/// `EffectHandoffProtocol` in every composition that includes the producing machine.
/// This is enforced at the composition validation level; the audit rule checks that
/// the handoff annotation exists on the machine schema side.
#[derive(Debug, Clone)]
pub struct HandoffProtocolCoverageRule {
    pub machine: String,
    pub effect_variant: String,
    pub protocol_name: String,
}

/// Rule: every declared `EffectHandoffProtocol` must have a generated protocol
/// helper file at the expected path.
#[derive(Debug, Clone)]
pub struct ProtocolRealizationSiteRule {
    pub protocol_name: String,
    /// Candidate paths where the generated helper may live (checked in order).
    /// The helper must exist at at least one of these paths.
    pub candidate_paths: Vec<String>,
}

/// Rule: feedback inputs declared in a protocol must only be submitted through
/// generated helper code (files with `// @generated` marker).
#[derive(Debug, Clone)]
pub struct ProtocolFeedbackConstraintRule {
    pub protocol_name: String,
    pub feedback_machine_instance: String,
    pub feedback_input_variant: String,
}

/// Rule: terminal outcome classification must use generated classification helpers,
/// not handwritten match arms on terminal phase strings.
#[derive(Debug, Clone)]
pub struct TerminalMappingConstraintRule {
    pub protocol_name: String,
    pub producer_machine: String,
    pub helper_path: &'static str,
}

/// Build handoff protocol coverage rules from canonical machine schemas.
/// Every effect with `handoff_protocol: Some(name)` generates a rule.
fn default_handoff_protocol_coverage() -> Vec<HandoffProtocolCoverageRule> {
    let mut rules = Vec::new();
    for schema in canonical_machine_schemas() {
        for disposition in &schema.effect_dispositions {
            if let Some(protocol_name) = &disposition.handoff_protocol {
                rules.push(HandoffProtocolCoverageRule {
                    machine: schema.machine.clone(),
                    effect_variant: disposition.effect_variant.clone(),
                    protocol_name: protocol_name.clone(),
                });
            }
        }
    }
    rules
}

/// Build protocol realization site rules from canonical compositions.
/// Every declared `EffectHandoffProtocol` must have a generated helper file.
/// The helper may live in any crate that participates in the protocol — we
/// check both the producer crate, the feedback machine crate, and meerkat-core
/// as a fallback (cross-machine protocols often land there).
fn default_protocol_realization_sites() -> Vec<ProtocolRealizationSiteRule> {
    use meerkat_machine_schema::{canonical_composition_schemas, canonical_machine_schemas};
    let machines = canonical_machine_schemas();
    let mut rules = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for composition in canonical_composition_schemas() {
        for protocol in &composition.handoff_protocols {
            if seen.insert(protocol.name.clone()) {
                // Collect candidate crates: producer machine, feedback machines, and meerkat-core.
                let mut candidate_crates: Vec<&str> = vec!["meerkat-core"];
                if let Some(inst) = composition
                    .machines
                    .iter()
                    .find(|m| m.instance_id == protocol.producer_instance)
                {
                    if let Some(ms) = machines.iter().find(|m| m.machine == inst.machine_name) {
                        candidate_crates.push(&ms.rust.crate_name);
                    }
                }
                for feedback in &protocol.allowed_feedback_inputs {
                    if let Some(inst) = composition
                        .machines
                        .iter()
                        .find(|m| m.instance_id == feedback.machine_instance)
                    {
                        if let Some(ms) = machines.iter().find(|m| m.machine == inst.machine_name) {
                            candidate_crates.push(&ms.rust.crate_name);
                        }
                    }
                }
                candidate_crates.dedup();
                rules.push(ProtocolRealizationSiteRule {
                    protocol_name: protocol.name.clone(),
                    candidate_paths: candidate_crates
                        .into_iter()
                        .map(|c| format!("{}/src/generated/protocol_{}.rs", c, protocol.name))
                        .collect(),
                });
            }
        }
    }
    rules
}

/// Build protocol feedback constraint rules from canonical compositions.
/// Every feedback input in a declared protocol generates a constraint.
fn default_protocol_feedback_constraints() -> Vec<ProtocolFeedbackConstraintRule> {
    use meerkat_machine_schema::canonical_composition_schemas;
    let mut rules = Vec::new();
    for composition in canonical_composition_schemas() {
        for protocol in &composition.handoff_protocols {
            for feedback in &protocol.allowed_feedback_inputs {
                rules.push(ProtocolFeedbackConstraintRule {
                    protocol_name: protocol.name.clone(),
                    feedback_machine_instance: feedback.machine_instance.clone(),
                    feedback_input_variant: feedback.input_variant.clone(),
                });
            }
        }
    }
    rules
}

/// Build terminal mapping constraint rules from canonical compositions.
/// Only protocols with `TerminalClosure` policy whose producer has terminal
/// phases generate a constraint — other closure policies (AckRequired,
/// AckOrAbort) don't need terminal classification helpers.
fn default_terminal_mapping_constraints() -> Vec<TerminalMappingConstraintRule> {
    use meerkat_machine_schema::{
        ClosurePolicy, canonical_composition_schemas, canonical_machine_schemas,
    };
    let machines = canonical_machine_schemas();
    let mut rules = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for composition in canonical_composition_schemas() {
        for protocol in &composition.handoff_protocols {
            if protocol.closure_policy != ClosurePolicy::TerminalClosure {
                continue;
            }
            let producer_machine_name = composition
                .machines
                .iter()
                .find(|m| m.instance_id == protocol.producer_instance)
                .map(|m| m.machine_name.as_str());
            if let Some(machine_name) = producer_machine_name {
                if machine_name != "TurnExecutionMachine" {
                    continue;
                }
                let has_terminals = machines
                    .iter()
                    .find(|m| m.machine == machine_name)
                    .is_some_and(|m| !m.state.terminal_phases.is_empty());
                if has_terminals && seen.insert(protocol.name.clone()) {
                    rules.push(TerminalMappingConstraintRule {
                        protocol_name: protocol.name.clone(),
                        producer_machine: machine_name.to_string(),
                        helper_path: "meerkat-core/src/generated/terminal_surface_mapping.rs",
                    });
                }
            }
        }
    }
    rules
}

fn default_allowed_paths(producer: &str, consumer: &str) -> Vec<&'static str> {
    match (producer, consumer) {
        ("RuntimeControlMachine" | "TurnExecutionMachine", "RuntimeIngressMachine")
        | ("RuntimeControlMachine", "TurnExecutionMachine")
        | (
            "RuntimeIngressMachine"
            | "TurnExecutionMachine"
            | "PeerCommsMachine"
            | "OpsLifecycleMachine",
            "RuntimeControlMachine",
        ) => {
            vec![
                "meerkat-runtime/src/runtime_loop.rs",
                "meerkat-runtime/src/session_adapter.rs",
            ]
        }
        ("OpsLifecycleMachine", "PeerCommsMachine") => {
            vec![
                "meerkat-machine-schema/src/catalog/compositions.rs",
                "meerkat-runtime/src/ops_lifecycle.rs",
            ]
        }
        ("FlowRunMachine" | "MobOrchestratorMachine", "RuntimeControlMachine")
        | ("FlowRunMachine" | "MobLifecycleMachine", "MobOrchestratorMachine")
        | ("MobOrchestratorMachine", "MobLifecycleMachine" | "FlowRunMachine") => {
            vec![
                "meerkat-mob/src/runtime/actor.rs",
                "meerkat-mob/src/runtime/flow.rs",
            ]
        }
        _ => vec![],
    }
}
