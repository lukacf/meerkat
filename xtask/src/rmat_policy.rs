use std::collections::BTreeMap;

use meerkat_machine_schema::canonical_machine_schemas;

#[derive(Debug, Clone)]
pub struct AuditPolicy {
    pub projection_types: Vec<ProjectionTypeRule>,
    pub authority_modules: Vec<AuthorityModuleRule>,
    pub protected_fields: Vec<ProtectedFieldRule>,
    pub forbidden_shell_reads: Vec<ForbiddenShellReadRule>,
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
                AuthorityModuleRule::new("meerkat-runtime/src/meerkat_machine.rs"),
                AuthorityModuleRule::new("meerkat-mob/src/runtime/actor.rs"),
                AuthorityModuleRule::new("meerkat-schedule/src/lifecycle.rs"),
                // AuthMachine (dogma #43 resolved): the per-binding auth-lease
                // lifecycle authority lives in meerkat-runtime. Every
                // AuthMachine transition must route through the DSL kernel
                // (`auth_machine::dsl::AuthMachineState::transition`), not
                // handwritten reducers. Dead-code in this module signals that
                // the handle trait is wired but the DSL kernel is not.
                AuthorityModuleRule::new("meerkat-runtime/src/handles/auth_lease.rs"),
                AuthorityModuleRule::new("meerkat-runtime/src/auth_machine/mod.rs"),
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
            ],
            forbidden_shell_reads: default_forbidden_shell_reads(),
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
                let producer_str = schema.machine.as_str();
                let effect_str = disposition.effect_variant.as_str();
                let consumer_str = consumer_machine.as_str();
                let consumer_input = default_consumer_input(producer_str, effect_str, consumer_str);
                rules.push(RoutedEffectRealizationRule::new(
                    producer_str.to_string(),
                    effect_str.to_string(),
                    consumer_str.to_string(),
                    consumer_input,
                    &default_allowed_paths(producer_str, consumer_str),
                ));
            }
        }
    }
    rules
}

fn default_consumer_input(producer: &str, effect_variant: &str, consumer: &str) -> String {
    // Kept in lock-step with the typed `Route::to.input_variant` entries in
    // the canonical composition schemas (see
    // `meerkat-machine-schema/src/catalog/compositions.rs`). The RMAT audit
    // `CompositionRouteSemanticCoverage` rule fails the build if this map
    // and the typed routes disagree about which consumer input a given
    // routed effect realizes.
    match (producer, effect_variant, consumer) {
        ("MobMachine", "RequestRuntimeBinding", "MeerkatMachine") => "PrepareBindings".to_string(),
        ("MobMachine", "RequestRuntimeIngress", "MeerkatMachine") => "Ingest".to_string(),
        ("MobMachine", "RequestRuntimeRetire", "MeerkatMachine") => "Retire".to_string(),
        ("MobMachine", "RequestRuntimeDestroy", "MeerkatMachine") => "Destroy".to_string(),
        ("MeerkatMachine", "RuntimeBound", "MobMachine") => "ObserveRuntimeReady".to_string(),
        ("MeerkatMachine", "RuntimeRetired", "MobMachine") => "ObserveRuntimeRetired".to_string(),
        ("MeerkatMachine", "RuntimeDestroyed", "MobMachine") => {
            "ObserveRuntimeDestroyed".to_string()
        }
        (
            "ScheduleLifecycleMachine",
            "SupersedePendingOccurrences",
            "OccurrenceLifecycleMachine",
        ) => "Supersede".to_string(),
        ("OccurrenceLifecycleMachine", "OccurrencesSuperseded", "ScheduleLifecycleMachine") => {
            "ConfirmOccurrencesSuperseded".to_string()
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
                    machine: schema.machine.as_str().to_string(),
                    effect_variant: disposition.effect_variant.as_str().to_string(),
                    protocol_name: protocol_name.as_str().to_string(),
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
                    && let Some(ms) = machines.iter().find(|m| m.machine == inst.machine_name)
                {
                    candidate_crates.push(ms.rust.crate_name.as_str());
                }
                for feedback in &protocol.allowed_feedback_inputs {
                    if let Some(inst) = composition
                        .machines
                        .iter()
                        .find(|m| m.instance_id == feedback.machine_instance)
                        && let Some(ms) = machines.iter().find(|m| m.machine == inst.machine_name)
                    {
                        candidate_crates.push(ms.rust.crate_name.as_str());
                    }
                }
                // Order-preserving dedup (Vec::dedup only removes adjacent duplicates)
                let mut seen_crates = std::collections::BTreeSet::new();
                candidate_crates.retain(|c| seen_crates.insert(*c));
                let protocol_name = protocol.name.as_str().to_string();
                let mut candidate_paths = vec![protocol.rust.module_path.clone()];
                candidate_paths.extend(candidate_crates.into_iter().map(|crate_name| {
                    format!("{crate_name}/src/generated/protocol_{protocol_name}.rs")
                }));
                candidate_paths.sort();
                candidate_paths.dedup();
                rules.push(ProtocolRealizationSiteRule {
                    protocol_name,
                    candidate_paths,
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
                    protocol_name: protocol.name.as_str().to_string(),
                    feedback_machine_instance: feedback.machine_instance.as_str().to_string(),
                    feedback_input_variant: feedback.input_variant.as_str().to_string(),
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
            if let Some(machine_name) = producer_machine_name
                && machine_name == "MeerkatMachine"
            {
                let has_terminals = machines
                    .iter()
                    .find(|m| m.machine.as_str() == machine_name)
                    .is_some_and(|m| !m.state.terminal_phases.is_empty());
                if has_terminals && seen.insert(protocol.name.clone()) {
                    rules.push(TerminalMappingConstraintRule {
                        protocol_name: protocol.name.as_str().to_string(),
                        producer_machine: machine_name.to_string(),
                        helper_path: "meerkat-core/src/generated/terminal_surface_mapping.rs",
                    });
                }
            }
        }
    }
    rules
}

/// Rule: detect shell-side authority state reads and shadow-state declarations
/// that short-circuit the machine-owned seam.
///
/// Each rule binds a file (by suffix-match on the repo-relative path) to an AST
/// pattern. Production violations are hard errors; `RMAT-ALLOW` is honored
/// only under explicit test-fixture paths so canaries can exercise the
/// suppression machinery without masking live shell/authority read seams.
#[derive(Debug, Clone)]
pub struct ForbiddenShellReadRule {
    pub path_suffix: &'static str,
    pub kind: ForbiddenShellReadKind,
    pub hint: &'static str,
}

#[derive(Debug, Clone)]
pub enum ForbiddenShellReadKind {
    /// Flag `<receiver>.<method>()` calls whose receiver tail ident is not in
    /// `allowed_receivers`. Used for "no `orch.phase()` in the mob actor shell
    /// except `lifecycle_authority.phase()`".
    MethodCall {
        method: &'static str,
        allowed_receivers: &'static [&'static str],
    },
    /// Flag any struct field declaration whose name matches `field_name`.
    /// Used for "the mob actor struct must not declare shadow counters named
    /// `tracked_flows`/`pending_spawn_ids`".
    FieldDeclared { field_name: &'static str },
    /// Flag any `<base>.<field>` field access whose `base` is a path ending in
    /// `base_ident` and whose field name is `field_name`. Used for "no
    /// `policy.apply_mode` reads in the ephemeral driver".
    FieldAccess {
        base_ident: &'static str,
        field_name: &'static str,
    },
}

/// Default forbidden-read rules. These encode what `scripts/rmat-read-seam-lint.sh`
/// used to enforce via regex, but as AST-precise policy entries. Production
/// findings remain unsuppressed; fixture-only canaries may still exercise
/// `RMAT-ALLOW` parsing.
fn default_forbidden_shell_reads() -> Vec<ForbiddenShellReadRule> {
    vec![
        // Mob actor shell: any `.phase()` read off something other than
        // `lifecycle_authority` means the shell is inspecting canonical
        // orchestrator state to decide whether to call apply().
        ForbiddenShellReadRule {
            path_suffix: "meerkat-mob/src/runtime/actor.rs",
            kind: ForbiddenShellReadKind::MethodCall {
                method: "phase",
                allowed_receivers: &["lifecycle_authority"],
            },
            hint: "remove the phase pre-check and let authority.apply() reject illegal transitions",
        },
        // Ephemeral driver shell: no `ingress.phase()` gate before apply().
        ForbiddenShellReadRule {
            path_suffix: "meerkat-runtime/src/driver/ephemeral.rs",
            kind: ForbiddenShellReadKind::MethodCall {
                method: "phase",
                allowed_receivers: &[],
            },
            hint: "remove the phase pre-check and let authority.apply() reject illegal transitions",
        },
        // Ephemeral driver shell: policy branching fields must flow through
        // authority.admit(); the shell must not classify inputs itself.
        ForbiddenShellReadRule {
            path_suffix: "meerkat-runtime/src/driver/ephemeral.rs",
            kind: ForbiddenShellReadKind::FieldAccess {
                base_ident: "policy",
                field_name: "apply_mode",
            },
            hint: "use authority.admit(); policy fields are authority-owned classification input",
        },
        ForbiddenShellReadRule {
            path_suffix: "meerkat-runtime/src/driver/ephemeral.rs",
            kind: ForbiddenShellReadKind::FieldAccess {
                base_ident: "policy",
                field_name: "queue_mode",
            },
            hint: "use authority.admit(); policy fields are authority-owned classification input",
        },
        ForbiddenShellReadRule {
            path_suffix: "meerkat-runtime/src/driver/ephemeral.rs",
            kind: ForbiddenShellReadKind::FieldAccess {
                base_ident: "policy",
                field_name: "consume_point",
            },
            hint: "use authority.admit(); policy fields are authority-owned classification input",
        },
        // MCP router: removal timing lives in ExternalToolSurfaceAuthority, not
        // a shell-owned HashMap.
        ForbiddenShellReadRule {
            path_suffix: "meerkat-mcp/src/router.rs",
            kind: ForbiddenShellReadKind::FieldDeclared {
                field_name: "removal_timeouts",
            },
            hint: "read timeout info from authority state via authority.removal_timing()",
        },
        // Mob actor: shadow counters must not be re-declared on the actor struct.
        // The snapshot type `MobFlowTrackerSnapshot` (in runtime/mod.rs) can
        // still populate these via struct-literal projection.
        ForbiddenShellReadRule {
            path_suffix: "meerkat-mob/src/runtime/actor.rs",
            kind: ForbiddenShellReadKind::FieldDeclared {
                field_name: "tracked_flows",
            },
            hint: "read counts from authority snapshots (orchestrator.snapshot().active_flow_count)",
        },
        ForbiddenShellReadRule {
            path_suffix: "meerkat-mob/src/runtime/actor.rs",
            kind: ForbiddenShellReadKind::FieldDeclared {
                field_name: "pending_spawn_ids",
            },
            hint: "read counts from authority snapshots (orchestrator.snapshot().pending_spawn_count)",
        },
    ]
}

fn default_allowed_paths(producer: &str, consumer: &str) -> Vec<&'static str> {
    match (producer, consumer) {
        ("MobMachine", "MeerkatMachine") | ("MeerkatMachine", "MobMachine") => {
            // Post-wave-c `meerkat_machine.rs` was decomposed into the
            // `meerkat_machine/` module directory; point at `mod.rs` so
            // the realization-path existence check resolves.
            vec![
                "meerkat-runtime/src/meerkat_machine/mod.rs",
                "meerkat-mob/src/runtime/actor.rs",
                "meerkat-mob/src/runtime/handle.rs",
            ]
        }
        ("ScheduleLifecycleMachine", "OccurrenceLifecycleMachine")
        | ("OccurrenceLifecycleMachine", "ScheduleLifecycleMachine") => {
            // Both directions of the schedule_bundle seam are realised
            // via the same service driver path.
            vec!["meerkat-schedule/src/service.rs"]
        }
        _ => vec![],
    }
}
