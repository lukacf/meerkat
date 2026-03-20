use std::collections::BTreeMap;

use meerkat_machine_schema::canonical_machine_schemas;

#[derive(Debug, Clone)]
pub struct AuditPolicy {
    pub projection_types: Vec<ProjectionTypeRule>,
    pub authority_modules: Vec<AuthorityModuleRule>,
    pub protected_fields: Vec<ProtectedFieldRule>,
    pub routed_effect_realizations: Vec<RoutedEffectRealizationRule>,
    pub required_live_symbols: Vec<RequiredLiveSymbolRule>,
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
                ProtectedFieldRule::new(
                    "comms_drain_active",
                    &["set_comms_drain_active", "seal_comms_drain_active"],
                ),
            ],
            routed_effect_realizations: default_routed_effect_realizations(),
            required_live_symbols: vec![],
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
        ("MobOrchestratorMachine", "MemberRespawnInitiated", "RuntimeControlMachine") => {
            "RecycleRequested".to_string()
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
