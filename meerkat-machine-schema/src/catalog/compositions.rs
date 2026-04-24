// Scoped clippy allows: the helpers below (comp_id, mach_id, act_id, …) use
// `expect()` on hand-authored DSL slugs that parse at construction time. A
// failure here is a DSL-slug authoring bug, never reachable from wire input.
// Inlining `parse(...).expect(...)` at every catalog entry would drown the
// composition definitions in boilerplate. Scope is the whole file because
// every composition builder uses these helpers.
#![allow(clippy::expect_used)]

use std::collections::BTreeMap;

use crate::{
    ActorKind, ActorSchema, ClosurePolicy, CompositionDriver, CompositionDriverRustBinding,
    CompositionInvariant, CompositionInvariantKind, CompositionSchema, CompositionStateLimits,
    CompositionTransactionPlan, CompositionWitness, DriverDispatchRoute, EffectHandoffProtocol,
    EntryInput, FeedbackFieldBinding, FeedbackFieldSource, FeedbackInputRef, MachineInstance,
    ProtocolGenerationMode, ProtocolHelperReturnShape, ProtocolRustBinding, Route,
    RouteBindingSource, RouteDelivery, RouteFieldBinding, RouteTarget, RouteTargetKind,
    RouteVariantId, WatchedEffect,
};

// Short-named typed-identity constructors used throughout this module.
//
// Every kernel-level identity (`MachineId`, `MachineInstanceId`, `ActorId`, …)
// is validated via `parse()`. The catalog entries below are compile-time-known
// slugs — authoring them as `identity::Thing::parse(...).expect(...)` inline
// would drown the composition definitions in boilerplate, so we route every
// construction site through these one-line helpers. A panic here is a
// hand-authored DSL-slug bug, never reachable from wire input.
use crate::identity::{
    ActorId, CompositionId, EffectVariantId, FieldId, InputVariantId, MachineId, MachineInstanceId,
    PhaseId, ProtocolId, RouteId, TransitionId,
};

fn comp_id(s: &str) -> CompositionId {
    CompositionId::parse(s).expect("valid composition slug")
}
fn mach_id(s: &str) -> MachineId {
    MachineId::parse(s).expect("valid machine slug")
}
fn mi_id(s: &str) -> MachineInstanceId {
    MachineInstanceId::parse(s).expect("valid machine-instance slug")
}
fn act_id(s: &str) -> ActorId {
    ActorId::parse(s).expect("valid actor slug")
}
fn iv_id(s: &str) -> InputVariantId {
    InputVariantId::parse(s).expect("valid input-variant slug")
}
fn ev_id(s: &str) -> EffectVariantId {
    EffectVariantId::parse(s).expect("valid effect-variant slug")
}
fn fld_id(s: &str) -> FieldId {
    FieldId::parse(s).expect("valid field slug")
}
fn route_id(s: &str) -> RouteId {
    RouteId::parse(s).expect("valid route slug")
}
fn protocol_id(s: &str) -> ProtocolId {
    ProtocolId::parse(s).expect("valid protocol slug")
}
#[allow(dead_code)]
fn phase_id(s: &str) -> PhaseId {
    PhaseId::parse(s).expect("valid phase slug")
}
#[allow(dead_code)]
fn transition_id(s: &str) -> TransitionId {
    TransitionId::parse(s).expect("valid transition slug")
}

/// Typed route-variant constructor that pairs a [`RouteTargetKind`] with
/// the matching [`InputVariantId`] or [`SignalVariantId`]. Every catalog
/// author expresses an input/signal target slug through this helper.
fn rv(kind: RouteTargetKind, slug: &str) -> crate::RouteVariantId {
    match kind {
        RouteTargetKind::Input => crate::RouteVariantId::Input(
            InputVariantId::parse(slug).expect("valid input-variant slug"),
        ),
        RouteTargetKind::Signal => crate::RouteVariantId::Signal(
            crate::identity::SignalVariantId::parse(slug).expect("valid signal-variant slug"),
        ),
    }
}

pub fn schedule_bundle_composition() -> CompositionSchema {
    CompositionSchema {
        name: comp_id("schedule_bundle"),
        machines: vec![
            MachineInstance {
                instance_id: mi_id("schedule"),
                machine_name: mach_id("ScheduleLifecycleMachine"),
                actor: act_id("schedule_authority"),
            },
            MachineInstance {
                instance_id: mi_id("occurrence"),
                machine_name: mach_id("OccurrenceLifecycleMachine"),
                actor: act_id("occurrence_authority"),
            },
        ],
        actors: vec![
            machine_actor("schedule_authority"),
            machine_actor("occurrence_authority"),
        ],
        handoff_protocols: vec![],
        entry_inputs: vec![],
        routes: vec![
            route(
                "revision_supersede_enters_occurrence_authority",
                "schedule",
                "SupersedePendingOccurrences",
                "occurrence",
                RouteTargetKind::Input,
                "Supersede",
                &[bind("superseded_by_revision", "superseding_revision")],
            ),
            // Reciprocal ack (wave-d D-f): once an occurrence absorbs
            // Supersede it emits OccurrencesSuperseded, routed back to
            // the schedule as ConfirmOccurrencesSuperseded so the
            // schedule authority observes which occurrences actually
            // superseded rather than inferring it from the outbound
            // route alone.
            route(
                "occurrence_supersede_ack_returns_to_schedule",
                "occurrence",
                "OccurrencesSuperseded",
                "schedule",
                RouteTargetKind::Input,
                "ConfirmOccurrencesSuperseded",
                &[
                    bind("occurrence_id", "occurrence_id"),
                    bind("superseding_revision", "superseding_revision"),
                ],
            ),
        ],
        route_target_selectors: vec![],
        driver: None,
        transaction_plans: vec![
            transaction_plan(
                "transactional_claim",
                "claim_due_occurrences",
                "store-backed claim uses authoritative store time plus durable lease state",
                "ScheduleStore::claim_due_occurrences",
            ),
            transaction_plan(
                "revision_supersede_and_replan",
                "update_schedule_revision",
                "revision-affecting schedule updates supersede pending future occurrences before replanning",
                "ScheduleStore::commit_schedule_mutation",
            ),
        ],
        actor_priorities: vec![],
        scheduler_rules: vec![],
        invariants: vec![
            CompositionInvariant {
                name: "schedule_revision_supersede_route_present".into(),
                kind: CompositionInvariantKind::RoutePresent {
                    from_machine: mi_id("schedule"),
                    effect_variant: ev_id("SupersedePendingOccurrences"),
                    to_machine: mi_id("occurrence"),
                    input_variant: rv(RouteTargetKind::Input, "Supersede"),
                },
                statement: "revision-affecting schedule edits enter occurrence authority through the explicit supersede route".into(),
                references_machines: vec![mi_id("schedule"), mi_id("occurrence")],
                references_actors: vec![act_id("schedule_authority"), act_id("occurrence_authority")],
            },
            CompositionInvariant {
                name: "superseded_occurrence_originates_from_schedule_revision".into(),
                kind: CompositionInvariantKind::ObservedRouteInputOriginatesFromEffect {
                    route_name: route_id("revision_supersede_enters_occurrence_authority"),
                    to_machine: mi_id("occurrence"),
                    input_variant: rv(RouteTargetKind::Input, "Supersede"),
                    from_machine: mi_id("schedule"),
                    effect_variant: ev_id("SupersedePendingOccurrences"),
                },
                statement: "pending future occurrences are superseded only by the schedule revision route rather than by ad hoc shell mutation".into(),
                references_machines: vec![mi_id("schedule"), mi_id("occurrence")],
                references_actors: vec![act_id("schedule_authority"), act_id("occurrence_authority")],
            },
            // Wave-d D-f: reciprocal ack closes the supersede loop.
            CompositionInvariant {
                name: "occurrence_supersede_ack_route_present".into(),
                kind: CompositionInvariantKind::RoutePresent {
                    from_machine: mi_id("occurrence"),
                    effect_variant: ev_id("OccurrencesSuperseded"),
                    to_machine: mi_id("schedule"),
                    input_variant: rv(RouteTargetKind::Input, "ConfirmOccurrencesSuperseded"),
                },
                statement: "the occurrence authority's supersede-consumption ack returns to the schedule authority through the reciprocal route so the schedule observes completion".into(),
                references_machines: vec![mi_id("schedule"), mi_id("occurrence")],
                references_actors: vec![act_id("schedule_authority"), act_id("occurrence_authority")],
            },
        ],
        witnesses: vec![
            witness(
                "revision_supersede_route",
                &["revision_supersede_enters_occurrence_authority"],
            ),
            witness(
                "occurrence_supersede_ack_route",
                &["occurrence_supersede_ack_returns_to_schedule"],
            ),
            witness("pause_resume_without_revision", &[]),
        ],
        deep_domain_cardinality: 3,
        deep_domain_overrides: std::collections::BTreeMap::new(),
        witness_domain_cardinality: 2,
        ci_limits: Some(default_ci_limits()),
        closed_world: true,
    }
}

pub fn schedule_runtime_bundle_composition() -> CompositionSchema {
    CompositionSchema {
        name: comp_id("schedule_runtime_bundle"),
        machines: vec![
            MachineInstance {
                instance_id: mi_id("occurrence"),
                machine_name: mach_id("OccurrenceLifecycleMachine"),
                actor: act_id("occurrence_authority"),
            },
            // Bind the schedule authority so
            // `OccurrenceLifecycleMachine::OccurrencesSuperseded` has a
            // declared consumer in this composition. Runtime-delivery
            // compositions still own the claim→delivery seam, but the
            // machine-level `=> routed [ScheduleLifecycleMachine]`
            // disposition is unconditional: if OccurrencesSuperseded
            // fires from this bundle's occurrence authority, it must
            // have somewhere to land. Mirrors `schedule_bundle`'s
            // `occurrence_supersede_ack_returns_to_schedule` route.
            MachineInstance {
                instance_id: mi_id("schedule"),
                machine_name: mach_id("ScheduleLifecycleMachine"),
                actor: act_id("schedule_authority"),
            },
        ],
        actors: vec![
            machine_actor("occurrence_authority"),
            machine_actor("schedule_authority"),
        ],
        handoff_protocols: vec![],
        entry_inputs: vec![],
        routes: vec![
            // Outbound: schedule revisions supersede pending occurrences.
            // Mirrors `schedule_bundle::revision_supersede_enters_occurrence_authority`.
            // The runtime bundle rarely originates schedule edits, but
            // binding ScheduleLifecycleMachine here means its routed
            // effects must have declared consumers; the reverse ack
            // route below closes the pair.
            route(
                "revision_supersede_enters_occurrence_authority",
                "schedule",
                "SupersedePendingOccurrences",
                "occurrence",
                RouteTargetKind::Input,
                "Supersede",
                &[bind("superseded_by_revision", "superseding_revision")],
            ),
            route(
                "occurrence_supersede_ack_returns_to_schedule",
                "occurrence",
                "OccurrencesSuperseded",
                "schedule",
                RouteTargetKind::Input,
                "ConfirmOccurrencesSuperseded",
                &[
                    bind("occurrence_id", "occurrence_id"),
                    bind("superseding_revision", "superseding_revision"),
                ],
            ),
        ],
        route_target_selectors: vec![],
        driver: None,
        transaction_plans: vec![transaction_plan(
            "transactional_runtime_claim",
            "claim_and_runtime_handoff",
            "transactional claim establishes the durable lease before runtime delivery begins",
            "ScheduleStore::claim_due_occurrences",
        )],
        actor_priorities: vec![],
        scheduler_rules: vec![],
        invariants: vec![],
        witnesses: vec![
            witness("runtime_delivery_feedback", &[]),
            witness("runtime_lease_expiry", &[]),
            witness(
                "revision_supersede_route",
                &["revision_supersede_enters_occurrence_authority"],
            ),
            witness(
                "occurrence_supersede_ack_route",
                &["occurrence_supersede_ack_returns_to_schedule"],
            ),
        ],
        deep_domain_cardinality: 3,
        deep_domain_overrides: std::collections::BTreeMap::new(),
        witness_domain_cardinality: 2,
        ci_limits: Some(default_ci_limits()),
        closed_world: true,
    }
}

pub fn schedule_mob_bundle_composition() -> CompositionSchema {
    CompositionSchema {
        name: comp_id("schedule_mob_bundle"),
        machines: vec![
            MachineInstance {
                instance_id: mi_id("occurrence"),
                machine_name: mach_id("OccurrenceLifecycleMachine"),
                actor: act_id("occurrence_authority"),
            },
            // Bind the schedule authority so
            // `OccurrenceLifecycleMachine::OccurrencesSuperseded` has a
            // declared consumer in this composition. Mob-delivery
            // compositions still own the claim→delivery seam, but the
            // machine-level `=> routed [ScheduleLifecycleMachine]`
            // disposition is unconditional: if OccurrencesSuperseded
            // fires from this bundle's occurrence authority, it must
            // have somewhere to land. Mirrors `schedule_bundle` and
            // `schedule_runtime_bundle`.
            MachineInstance {
                instance_id: mi_id("schedule"),
                machine_name: mach_id("ScheduleLifecycleMachine"),
                actor: act_id("schedule_authority"),
            },
        ],
        actors: vec![
            machine_actor("occurrence_authority"),
            machine_actor("schedule_authority"),
        ],
        handoff_protocols: vec![],
        entry_inputs: vec![],
        routes: vec![
            // Outbound: schedule revisions supersede pending occurrences.
            // Mirrors `schedule_bundle::revision_supersede_enters_occurrence_authority`.
            // The mob bundle rarely originates schedule edits, but
            // binding ScheduleLifecycleMachine here means its routed
            // effects must have declared consumers; the reverse ack
            // route below closes the pair.
            route(
                "revision_supersede_enters_occurrence_authority",
                "schedule",
                "SupersedePendingOccurrences",
                "occurrence",
                RouteTargetKind::Input,
                "Supersede",
                &[bind("superseded_by_revision", "superseding_revision")],
            ),
            route(
                "occurrence_supersede_ack_returns_to_schedule",
                "occurrence",
                "OccurrencesSuperseded",
                "schedule",
                RouteTargetKind::Input,
                "ConfirmOccurrencesSuperseded",
                &[
                    bind("occurrence_id", "occurrence_id"),
                    bind("superseding_revision", "superseding_revision"),
                ],
            ),
        ],
        route_target_selectors: vec![],
        driver: None,
        transaction_plans: vec![transaction_plan(
            "transactional_mob_claim",
            "claim_and_mob_handoff",
            "transactional claim establishes the durable lease before mob delivery begins",
            "ScheduleStore::claim_due_occurrences",
        )],
        actor_priorities: vec![],
        scheduler_rules: vec![],
        invariants: vec![],
        witnesses: vec![
            witness("mob_delivery_feedback", &[]),
            witness("materialization_failure_classification", &[]),
            witness(
                "revision_supersede_route",
                &["revision_supersede_enters_occurrence_authority"],
            ),
            witness(
                "occurrence_supersede_ack_route",
                &["occurrence_supersede_ack_returns_to_schedule"],
            ),
        ],
        deep_domain_cardinality: 3,
        deep_domain_overrides: std::collections::BTreeMap::new(),
        witness_domain_cardinality: 2,
        ci_limits: Some(default_ci_limits()),
        closed_world: true,
    }
}

pub fn meerkat_mob_seam_composition() -> CompositionSchema {
    CompositionSchema {
        name: comp_id("meerkat_mob_seam"),
        machines: vec![
            MachineInstance {
                instance_id: mi_id("meerkat"),
                machine_name: mach_id("MeerkatMachine"),
                actor: act_id("meerkat_kernel"),
            },
            MachineInstance {
                instance_id: mi_id("mob"),
                machine_name: mach_id("MobMachine"),
                actor: act_id("mob_kernel"),
            },
        ],
        actors: vec![machine_actor("meerkat_kernel"), machine_actor("mob_kernel")],
        handoff_protocols: vec![],
        entry_inputs: vec![
            EntryInput {
                name: "spawn_member".into(),
                machine: mi_id("mob"),
                input_variant: iv_id("Spawn"),
            },
            EntryInput {
                name: "submit_work".into(),
                machine: mi_id("mob"),
                input_variant: iv_id("SubmitWork"),
            },
            EntryInput {
                name: "retire_member".into(),
                machine: mi_id("mob"),
                input_variant: iv_id("Retire"),
            },
            EntryInput {
                name: "destroy_mob".into(),
                machine: mi_id("mob"),
                input_variant: iv_id("Destroy"),
            },
        ],
        routes: vec![
            route(
                "binding_request_reaches_meerkat",
                "mob",
                "RequestRuntimeBinding",
                "meerkat",
                RouteTargetKind::Input,
                "PrepareBindings",
                &[
                    bind("agent_runtime_id", "agent_runtime_id"),
                    bind("fence_token", "fence_token"),
                    bind("generation", "generation"),
                    bind("session_id", "session_id"),
                ],
            ),
            route(
                "work_request_reaches_meerkat",
                "mob",
                "RequestRuntimeIngress",
                "meerkat",
                RouteTargetKind::Input,
                "Ingest",
                &[
                    bind("runtime_id", "agent_runtime_id"),
                    bind("work_id", "work_id"),
                    bind("origin", "origin"),
                ],
            ),
            route(
                "retire_request_reaches_meerkat",
                "mob",
                "RequestRuntimeRetire",
                "meerkat",
                RouteTargetKind::Input,
                "Retire",
                &[bind("session_id", "session_id")],
            ),
            route(
                "destroy_request_reaches_meerkat",
                "mob",
                "RequestRuntimeDestroy",
                "meerkat",
                RouteTargetKind::Input,
                "Destroy",
                &[bind("session_id", "session_id")],
            ),
            route(
                "runtime_bound_reaches_mob",
                "meerkat",
                "RuntimeBound",
                "mob",
                RouteTargetKind::Signal,
                "ObserveRuntimeReady",
                &[
                    bind("agent_runtime_id", "agent_runtime_id"),
                    bind("fence_token", "fence_token"),
                ],
            ),
            route(
                "runtime_retired_reaches_mob",
                "meerkat",
                "RuntimeRetired",
                "mob",
                RouteTargetKind::Signal,
                "ObserveRuntimeRetired",
                &[
                    bind("agent_runtime_id", "agent_runtime_id"),
                    bind("fence_token", "fence_token"),
                ],
            ),
            route(
                "runtime_destroyed_reaches_mob",
                "meerkat",
                "RuntimeDestroyed",
                "mob",
                RouteTargetKind::Signal,
                "ObserveRuntimeDestroyed",
                &[
                    bind("agent_runtime_id", "agent_runtime_id"),
                    bind("fence_token", "fence_token"),
                ],
            ),
        ],
        route_target_selectors: vec![],
        // Wave-c C-6p: the mob→meerkat seam now declares a typed composition
        // driver. Its sole role is to opt the composition into codegen
        // emission — `render_composition_driver` gates on `driver: Some(...)`
        // and generates the `MeerkatMobSeamEffect` enum + `route_to_input`
        // function that the runtime dispatcher (B-5
        // `CatalogCompositionDispatcher`) consumes. The four `watched_effects`
        // and `dispatch_routes` below mirror the four Input-kind `routes`
        // above (producer=mob, consumer=meerkat); Signal-kind routes are
        // excluded by `render_composition_driver` and handled by the signal
        // surface.
        driver: Some(CompositionDriver {
            name: "meerkat_mob_seam_driver".into(),
            rust: CompositionDriverRustBinding {
                module_path: "meerkat-runtime/src/generated/meerkat_mob_seam.rs".into(),
                driver_type: "MeerkatMobSeamDriver".into(),
                store_plan_type: "MeerkatMobSeamStorePlan".into(),
                work_type: "MeerkatMobSeamWork".into(),
                decision_type: "MeerkatMobSeamDecision".into(),
                required_imports: vec![],
            },
            watched_effects: vec![
                WatchedEffect {
                    producer_instance: mi_id("mob"),
                    effect_variant: ev_id("RequestRuntimeBinding"),
                },
                WatchedEffect {
                    producer_instance: mi_id("mob"),
                    effect_variant: ev_id("RequestRuntimeIngress"),
                },
                WatchedEffect {
                    producer_instance: mi_id("mob"),
                    effect_variant: ev_id("RequestRuntimeRetire"),
                },
                WatchedEffect {
                    producer_instance: mi_id("mob"),
                    effect_variant: ev_id("RequestRuntimeDestroy"),
                },
            ],
            dispatch_routes: vec![
                DriverDispatchRoute {
                    name: route_id("binding_request_reaches_meerkat"),
                    target_instance: mi_id("meerkat"),
                    target_kind: RouteTargetKind::Input,
                    input_variant: RouteVariantId::Input(iv_id("PrepareBindings")),
                },
                DriverDispatchRoute {
                    name: route_id("work_request_reaches_meerkat"),
                    target_instance: mi_id("meerkat"),
                    target_kind: RouteTargetKind::Input,
                    input_variant: RouteVariantId::Input(iv_id("Ingest")),
                },
                DriverDispatchRoute {
                    name: route_id("retire_request_reaches_meerkat"),
                    target_instance: mi_id("meerkat"),
                    target_kind: RouteTargetKind::Input,
                    input_variant: RouteVariantId::Input(iv_id("Retire")),
                },
                DriverDispatchRoute {
                    name: route_id("destroy_request_reaches_meerkat"),
                    target_instance: mi_id("meerkat"),
                    target_kind: RouteTargetKind::Input,
                    input_variant: RouteVariantId::Input(iv_id("Destroy")),
                },
            ],
        }),
        transaction_plans: vec![],
        actor_priorities: vec![],
        scheduler_rules: vec![],
        invariants: vec![],
        witnesses: vec![
            witness(
                "basic_round_trip",
                &[
                    "binding_request_reaches_meerkat",
                    "work_request_reaches_meerkat",
                    "runtime_bound_reaches_mob",
                ],
            ),
            witness(
                "retire_runtime_path",
                &[
                    "retire_request_reaches_meerkat",
                    "runtime_retired_reaches_mob",
                ],
            ),
            witness(
                "destroy_runtime_path",
                &[
                    "destroy_request_reaches_meerkat",
                    "runtime_destroyed_reaches_mob",
                ],
            ),
        ],
        deep_domain_cardinality: 3,
        deep_domain_overrides: std::collections::BTreeMap::new(),
        witness_domain_cardinality: 2,
        ci_limits: Some(default_ci_limits()),
        closed_world: true,
    }
}

fn machine_actor(name: &str) -> ActorSchema {
    ActorSchema {
        name: act_id(name),
        kind: ActorKind::Machine,
    }
}

fn route(
    name: &str,
    from_machine: &str,
    effect_variant: &str,
    to_machine: &str,
    target_kind: RouteTargetKind,
    input_variant: &str,
    bindings: &[RouteFieldBinding],
) -> Route {
    Route {
        name: route_id(name),
        from_machine: mi_id(from_machine),
        effect_variant: ev_id(effect_variant),
        to: RouteTarget::new(mi_id(to_machine), rv(target_kind, input_variant)),
        bindings: bindings.to_vec(),
        delivery: RouteDelivery::Immediate,
    }
}

fn bind(to_field: &str, from_field: &str) -> RouteFieldBinding {
    RouteFieldBinding {
        to_field: fld_id(to_field),
        source: RouteBindingSource::Field {
            from_field: fld_id(from_field),
            allow_named_alias: false,
        },
    }
}

fn transaction_plan(
    name: &str,
    trigger: &str,
    description: &str,
    store_primitive: &str,
) -> CompositionTransactionPlan {
    CompositionTransactionPlan {
        name: name.into(),
        trigger: trigger.into(),
        description: description.into(),
        store_primitive: store_primitive.into(),
        route_names: vec![],
        protocol_names: vec![],
    }
}

fn witness(name: &str, expected_routes: &[&str]) -> CompositionWitness {
    CompositionWitness {
        name: name.into(),
        preload_inputs: vec![],
        expected_routes: expected_routes.iter().map(|r| route_id(r)).collect(),
        expected_scheduler_rules: vec![],
        expected_states: vec![],
        expected_transitions: vec![],
        expected_transition_order: vec![],
        state_limits: default_ci_limits(),
    }
}

fn default_ci_limits() -> CompositionStateLimits {
    CompositionStateLimits {
        step_limit: 8,
        pending_input_limit: 8,
        pending_route_limit: 8,
        delivered_route_limit: 0,
        emitted_effect_limit: 0,
        seq_limit: 0,
        set_limit: 0,
        map_limit: 0,
    }
}

/// Compositions declared to host cross-machine handoff protocols whose
/// producer side is backed either by a compat machine (flow/loop) or by
/// the canonical `MeerkatMachine` with an absorbed handoff effect that
/// the runtime authority fills in. They sit alongside
/// `canonical_composition_schemas()` in the codegen iteration.
///
/// Populated as each handoff protocol's producer side is wired up.
pub fn compat_composition_schemas() -> Vec<CompositionSchema> {
    vec![
        mob_bundle_composition(),
        external_tool_bundle_composition(),
        flow_frame_loop_composition(),
        supervisor_trust_bundle_composition(),
        mob_destroy_session_ingress_bundle_composition(),
        auth_lease_bundle_composition(),
    ]
}

/// Host composition for the `flow_loop_until_evaluation` handoff
/// protocol. The producer is the compat `LoopIterationMachine` whose
/// `EvaluateUntilCondition` effect is wrapped into an obligation by
/// the generated `accept_evaluate_until_condition` helper.
///
/// Mode: ShellBridge. The `meerkat_mob::runtime::loop_iteration_authority`
/// module hosts a thin wrapper over the generated `loop_iteration`
/// kernel so the protocol's `submit_*` helpers can call
/// `authority.apply(...)` — the same idiom every other `ShellBridge`
/// protocol uses in this workspace.
fn flow_frame_loop_composition() -> CompositionSchema {
    CompositionSchema {
        name: comp_id("flow_frame_loop"),
        machines: vec![MachineInstance {
            instance_id: mi_id("loop_iteration"),
            machine_name: mach_id("LoopIterationMachine"),
            actor: act_id("loop_iteration_authority"),
        }],
        actors: vec![
            machine_actor("loop_iteration_authority"),
            owner_actor("loop_runtime_owner"),
        ],
        handoff_protocols: vec![EffectHandoffProtocol {
            name: protocol_id("flow_loop_until_evaluation"),
            producer_instance: mi_id("loop_iteration"),
            effect_variant: ev_id("EvaluateUntilCondition"),
            realizing_actor: act_id("loop_runtime_owner"),
            correlation_fields: vec![fld_id("loop_instance_id"), fld_id("iteration")],
            obligation_fields: vec![
                fld_id("loop_instance_id"),
                fld_id("iteration"),
                fld_id("parent_frame_id"),
                fld_id("parent_node_id"),
                fld_id("loop_id"),
            ],
            allowed_feedback_inputs: vec![
                FeedbackInputRef {
                    machine_instance: mi_id("loop_iteration"),
                    input_variant: iv_id("UntilConditionMet"),
                    field_bindings: vec![
                        FeedbackFieldBinding {
                            input_field: fld_id("loop_instance_id"),
                            source: FeedbackFieldSource::ObligationField(fld_id("loop_instance_id")),
                        },
                        FeedbackFieldBinding {
                            input_field: fld_id("iteration"),
                            source: FeedbackFieldSource::ObligationField(fld_id("iteration")),
                        },
                    ],
                },
                FeedbackInputRef {
                    machine_instance: mi_id("loop_iteration"),
                    input_variant: iv_id("UntilConditionFailed"),
                    field_bindings: vec![
                        FeedbackFieldBinding {
                            input_field: fld_id("loop_instance_id"),
                            source: FeedbackFieldSource::ObligationField(fld_id("loop_instance_id")),
                        },
                        FeedbackFieldBinding {
                            input_field: fld_id("iteration"),
                            source: FeedbackFieldSource::ObligationField(fld_id("iteration")),
                        },
                    ],
                },
            ],
            closure_policy: ClosurePolicy::AckRequired,
            liveness_annotation: Some(
                "eventual feedback under task-scheduling fairness".into(),
            ),
            rust: ProtocolRustBinding {
                module_path:
                    "meerkat-mob/src/generated/protocol_flow_loop_until_evaluation.rs".into(),
                generation_mode: ProtocolGenerationMode::ShellBridge,
                required_imports: vec![
                    "use crate::error::MobError;".into(),
                    "use crate::ids::{FlowNodeId, FrameId, LoopId, LoopInstanceId};".into(),
                    "use crate::runtime::loop_iteration_authority::{LoopIterationAuthority, LoopIterationInput, LoopIterationMutator, LoopIterationTransition, LoopUntilEvaluationRequested, inputs};".into(),
                ],
                authority_type_path: Some(
                    "crate::runtime::loop_iteration_authority::LoopIterationAuthority".into(),
                ),
                mutator_trait_path: Some(
                    "crate::runtime::loop_iteration_authority::LoopIterationMutator".into(),
                ),
                input_enum_path: Some(
                    "crate::runtime::loop_iteration_authority::LoopIterationInput".into(),
                ),
                effect_enum_path: None,
                transition_type_path: Some(
                    "crate::runtime::loop_iteration_authority::LoopIterationTransition".into(),
                ),
                error_type_path: Some("crate::error::MobError".into()),
                executor_trigger_input_variant: None,
                bridge_source_type_path: Some(
                    "crate::runtime::loop_iteration_authority::LoopUntilEvaluationRequested".into(),
                ),
                helper_return_shape: ProtocolHelperReturnShape::Transition,
                handle_trait_path: None,
                handle_method_names: BTreeMap::new(),
                handle_arg_accessors: BTreeMap::new(),
                handle_method_forwarded_fields: BTreeMap::new(),
                // Kernel-codegen input enum uses tuple-wrapping
                // variants: `LoopIterationInput::UntilConditionMet(
                // inputs::UntilConditionMet { ... })`.
                input_payload_module_path: Some("inputs".into()),
                additional_modes: vec![],
            },
        }],
        entry_inputs: vec![],
        routes: vec![],
        route_target_selectors: vec![],
        driver: None,
        transaction_plans: vec![],
        actor_priorities: vec![],
        scheduler_rules: vec![],
        invariants: vec![CompositionInvariant {
            name: "flow_loop_until_evaluation_protocol_covered".into(),
            kind: CompositionInvariantKind::HandoffProtocolCovered {
                producer_instance: mi_id("loop_iteration"),
                effect_variant: ev_id("EvaluateUntilCondition"),
                protocol_name: protocol_id("flow_loop_until_evaluation"),
            },
            statement: "loop-iteration authority's UntilCondition evaluation effect is handed off through the explicit `flow_loop_until_evaluation` protocol rather than ad-hoc shell mutation".into(),
            references_machines: vec![mi_id("loop_iteration")],
            references_actors: vec![
                act_id("loop_iteration_authority"),
                act_id("loop_runtime_owner"),
            ],
        }],
        witnesses: vec![witness("flow_loop_eval_round_trip", &[])],
        deep_domain_cardinality: 3,
        deep_domain_overrides: std::collections::BTreeMap::new(),
        witness_domain_cardinality: 2,
        ci_limits: Some(default_ci_limits()),
        closed_world: true,
    }
}

/// Host composition for the `ops_barrier_satisfaction` handoff protocol.
///
/// The producer is the canonical `MeerkatMachine` which declares
/// `WaitAllSatisfied { wait_request_id, operation_ids }` as a
/// local-disposition effect. The realizing owner — the runtime's ops
/// lifecycle shell — observes the barrier closure and feeds back
/// through the `OpsBarrierSatisfied { operation_ids }` input; the
/// `HandleBridge` mode routes the same payload through
/// `TurnStateHandle::ops_barrier_satisfied` for runtime-backed sessions.
///
/// Modes: primary `ShellBridge` (accept + authority.apply submitters),
/// secondary `HandleBridge` (handle-driven submitter suffixed `_handle`).
fn mob_bundle_composition() -> CompositionSchema {
    let mut handle_methods = BTreeMap::new();
    handle_methods.insert("OpsBarrierSatisfied".into(), "ops_barrier_satisfied".into());
    // Handle method takes `operation_ids: BTreeSet<String>` — the obligation
    // field is `Set<OperationId>` which renders as `Vec<OperationId>`. The
    // accessor rewrites the reference to stringify each operation id.
    let mut handle_accessors = BTreeMap::new();
    handle_accessors.insert(
        "OpsBarrierSatisfied.operation_ids".into(),
        ".iter().map(ToString::to_string).collect()".into(),
    );
    // Handle method takes only `operation_ids`; the obligation carries
    // a `wait_request_id` correlation token that the turn-state handle
    // never consumes (the ops-lifecycle owner matches on it internally,
    // not through the handle).
    let mut handle_forwarded_fields = BTreeMap::new();
    handle_forwarded_fields.insert("OpsBarrierSatisfied".into(), vec!["operation_ids".into()]);

    CompositionSchema {
        name: comp_id("mob_bundle"),
        // The producer is the compat `OpsBarrierBridgeMachine` which hosts
        // the handoff-annotated `WaitAllSatisfied` effect declaration.
        // Its shape mirrors the runtime-owned effect; the canonical
        // `MeerkatMachine` also declares `WaitAllSatisfied` (without the
        // handoff annotation the DSL macro cannot emit) so the runtime
        // shell still observes the effect through its own reducer.
        machines: vec![MachineInstance {
            instance_id: mi_id("ops_barrier_bridge"),
            machine_name: mach_id("OpsBarrierBridgeMachine"),
            actor: act_id("ops_barrier_bridge_authority"),
        }],
        actors: vec![
            machine_actor("ops_barrier_bridge_authority"),
            owner_actor("ops_lifecycle_owner"),
        ],
        handoff_protocols: vec![EffectHandoffProtocol {
            name: protocol_id("ops_barrier_satisfaction"),
            producer_instance: mi_id("ops_barrier_bridge"),
            effect_variant: ev_id("WaitAllSatisfied"),
            realizing_actor: act_id("ops_lifecycle_owner"),
            correlation_fields: vec![fld_id("wait_request_id")],
            obligation_fields: vec![fld_id("wait_request_id"), fld_id("operation_ids")],
            allowed_feedback_inputs: vec![FeedbackInputRef {
                machine_instance: mi_id("ops_barrier_bridge"),
                input_variant: iv_id("OpsBarrierSatisfied"),
                field_bindings: vec![
                    FeedbackFieldBinding {
                        input_field: fld_id("wait_request_id"),
                        source: FeedbackFieldSource::ObligationField(fld_id("wait_request_id")),
                    },
                    FeedbackFieldBinding {
                        input_field: fld_id("operation_ids"),
                        source: FeedbackFieldSource::ObligationField(fld_id("operation_ids")),
                    },
                ],
            }],
            closure_policy: ClosurePolicy::AckRequired,
            liveness_annotation: Some(
                "eventual feedback under task-scheduling fairness".into(),
            ),
            rust: ProtocolRustBinding {
                module_path: "meerkat-core/src/generated/protocol_ops_barrier_satisfaction.rs"
                    .into(),
                // Primary mode: HandleBridge. The obligation is built
                // from the shell-owned `WaitAllSatisfied` struct via the
                // shared `accept_<effect>` emission; feedback flows
                // through the `TurnStateHandle::ops_barrier_satisfied`
                // trait method. The compat `OpsBarrierBridgeMachine`
                // hosts the handoff annotation purely so the protocol
                // passes composition validation — it has no runtime
                // authority of its own, so stacking ShellBridge with an
                // `authority.apply` submitter would point at nothing.
                generation_mode: ProtocolGenerationMode::HandleBridge,
                required_imports: vec![
                    "use crate::handles::{DslTransitionError, TurnStateHandle};".into(),
                    "use crate::lifecycle::identifiers::WaitRequestId;".into(),
                    "use crate::ops::OperationId;".into(),
                    "use crate::ops_lifecycle::WaitAllSatisfied;".into(),
                ],
                authority_type_path: None,
                mutator_trait_path: None,
                input_enum_path: None,
                effect_enum_path: None,
                transition_type_path: None,
                error_type_path: None,
                executor_trigger_input_variant: None,
                bridge_source_type_path: Some("crate::ops_lifecycle::WaitAllSatisfied".into()),
                helper_return_shape: ProtocolHelperReturnShape::Obligations,
                handle_trait_path: Some("meerkat_core::handles::TurnStateHandle".into()),
                handle_method_names: handle_methods,
                handle_arg_accessors: handle_accessors,
                handle_method_forwarded_fields: handle_forwarded_fields,
                input_payload_module_path: None,
                additional_modes: vec![],
            },
        }],
        entry_inputs: vec![],
        routes: vec![],
        route_target_selectors: vec![],
        driver: None,
        transaction_plans: vec![],
        actor_priorities: vec![],
        scheduler_rules: vec![],
        invariants: vec![CompositionInvariant {
            name: "ops_barrier_satisfaction_protocol_covered".into(),
            kind: CompositionInvariantKind::HandoffProtocolCovered {
                producer_instance: mi_id("ops_barrier_bridge"),
                effect_variant: ev_id("WaitAllSatisfied"),
                protocol_name: protocol_id("ops_barrier_satisfaction"),
            },
            statement: "wait-all barrier satisfaction crosses from the ops lifecycle owner back into turn-state authority only through the explicit `ops_barrier_satisfaction` protocol".into(),
            references_machines: vec![mi_id("ops_barrier_bridge")],
            references_actors: vec![
                act_id("ops_barrier_bridge_authority"),
                act_id("ops_lifecycle_owner"),
            ],
        }],
        witnesses: vec![witness("ops_barrier_close_round_trip", &[])],
        deep_domain_cardinality: 3,
        deep_domain_overrides: std::collections::BTreeMap::new(),
        witness_domain_cardinality: 2,
        ci_limits: Some(default_ci_limits()),
        closed_world: true,
    }
}

/// Host composition for the `surface_completion` and
/// `surface_snapshot_alignment` handoff protocols.
///
/// Both protocols' producer-side effects are emitted by the runtime's
/// hand-written `ExternalToolSurfaceAuthority`. The compat
/// `ExternalToolSurfaceBridgeMachine` mirrors each effect's shape and
/// hosts the `handoff_protocol` annotation that the canonical DSL
/// macro cannot express.
///
/// - `surface_completion` — EffectExtractor (scans `ExternalToolSurfaceEffect`
///   for `ScheduleSurfaceCompletion` variants) + HandleBridge
///   (`mark_pending_succeeded` / `mark_pending_failed` on
///   `ExternalToolSurfaceHandle`). No authority submitter is emitted —
///   feedback flows through the handle.
/// - `surface_snapshot_alignment` — EffectExtractor + HandleBridge
///   (`snapshot_aligned`). Same shape, single field.
fn external_tool_bundle_composition() -> CompositionSchema {
    let mut completion_methods = BTreeMap::new();
    completion_methods.insert("PendingSucceeded".into(), "mark_pending_succeeded".into());
    completion_methods.insert("PendingFailed".into(), "mark_pending_failed".into());
    let mut completion_accessors = BTreeMap::new();
    // Handle takes `String` surface_id; obligation carries typed SurfaceId.
    completion_accessors.insert("PendingSucceeded.surface_id".into(), ".0".into());
    completion_accessors.insert("PendingFailed.surface_id".into(), ".0".into());
    let mut completion_forwarded = BTreeMap::new();
    // `mark_pending_succeeded(surface_id, pending_task_sequence, staged_intent_sequence)`.
    completion_forwarded.insert(
        "PendingSucceeded".into(),
        vec![
            "surface_id".into(),
            "pending_task_sequence".into(),
            "staged_intent_sequence".into(),
        ],
    );
    // `mark_pending_failed(surface_id, reason)` — `reason` is owner-context.
    completion_forwarded.insert(
        "PendingFailed".into(),
        vec!["surface_id".into(), "reason".into()],
    );

    let mut snapshot_methods = BTreeMap::new();
    snapshot_methods.insert("SnapshotAligned".into(), "snapshot_aligned".into());
    let mut snapshot_forwarded = BTreeMap::new();
    snapshot_forwarded.insert("SnapshotAligned".into(), vec!["snapshot_epoch".into()]);

    CompositionSchema {
        name: comp_id("external_tool_bundle"),
        machines: vec![MachineInstance {
            instance_id: mi_id("external_tool_surface"),
            machine_name: mach_id("ExternalToolSurfaceBridgeMachine"),
            actor: act_id("external_tool_surface_authority"),
        }],
        actors: vec![
            machine_actor("external_tool_surface_authority"),
            owner_actor("surface_host_owner"),
        ],
        handoff_protocols: vec![
            // Protocol 1: surface_completion — dual-mode emission
            // (EffectExtractor + HandleBridge).
            EffectHandoffProtocol {
                name: protocol_id("surface_completion"),
                producer_instance: mi_id("external_tool_surface"),
                effect_variant: ev_id("ScheduleSurfaceCompletion"),
                realizing_actor: act_id("surface_host_owner"),
                correlation_fields: vec![
                    fld_id("surface_id"),
                    fld_id("pending_task_sequence"),
                ],
                obligation_fields: vec![
                    fld_id("surface_id"),
                    fld_id("operation"),
                    fld_id("pending_task_sequence"),
                    fld_id("staged_intent_sequence"),
                    fld_id("applied_at_turn"),
                ],
                allowed_feedback_inputs: vec![
                    FeedbackInputRef {
                        machine_instance: mi_id("external_tool_surface"),
                        input_variant: iv_id("PendingSucceeded"),
                        field_bindings: vec![
                            FeedbackFieldBinding {
                                input_field: fld_id("surface_id"),
                                source: FeedbackFieldSource::ObligationField(fld_id("surface_id")),
                            },
                            FeedbackFieldBinding {
                                input_field: fld_id("pending_task_sequence"),
                                source: FeedbackFieldSource::ObligationField(fld_id("pending_task_sequence")),
                            },
                            FeedbackFieldBinding {
                                input_field: fld_id("staged_intent_sequence"),
                                source: FeedbackFieldSource::ObligationField(fld_id("staged_intent_sequence")),
                            },
                        ],
                    },
                    FeedbackInputRef {
                        machine_instance: mi_id("external_tool_surface"),
                        input_variant: iv_id("PendingFailed"),
                        field_bindings: vec![
                            FeedbackFieldBinding {
                                input_field: fld_id("surface_id"),
                                source: FeedbackFieldSource::ObligationField(fld_id("surface_id")),
                            },
                            FeedbackFieldBinding {
                                input_field: fld_id("pending_task_sequence"),
                                source: FeedbackFieldSource::ObligationField(fld_id("pending_task_sequence")),
                            },
                            FeedbackFieldBinding {
                                input_field: fld_id("reason"),
                                source: FeedbackFieldSource::OwnerContext("reason".into()),
                            },
                        ],
                    },
                ],
                closure_policy: ClosurePolicy::AckRequired,
                liveness_annotation: Some(
                    "eventual feedback under surface connection liveness".into(),
                ),
                rust: ProtocolRustBinding {
                    module_path: "meerkat-mcp/src/generated/protocol_surface_completion.rs".into(),
                    generation_mode: ProtocolGenerationMode::EffectExtractor,
                    required_imports: vec![
                        "use crate::external_tool_surface_authority::{ExternalToolSurfaceEffect, SurfaceDeltaOperation, SurfaceId, TurnNumber};".into(),
                        "use meerkat_core::handles::{DslTransitionError, ExternalToolSurfaceHandle};".into(),
                    ],
                    // No authority submitters; feedback flows through the
                    // handle only. EffectExtractor emits `extract_obligations`
                    // only (no authority.apply submitter).
                    authority_type_path: None,
                    mutator_trait_path: None,
                    input_enum_path: None,
                    effect_enum_path: Some(
                        "crate::external_tool_surface_authority::ExternalToolSurfaceEffect".into(),
                    ),
                    transition_type_path: None,
                    error_type_path: None,
                    executor_trigger_input_variant: None,
                    bridge_source_type_path: None,
                    helper_return_shape: ProtocolHelperReturnShape::Obligations,
                    handle_trait_path: Some(
                        "meerkat_core::handles::ExternalToolSurfaceHandle".into(),
                    ),
                    handle_method_names: completion_methods,
                    handle_arg_accessors: completion_accessors,
                    handle_method_forwarded_fields: completion_forwarded,
                    input_payload_module_path: None,
                    additional_modes: vec![ProtocolGenerationMode::HandleBridge],
                },
            },
            // Protocol 2: surface_snapshot_alignment — dual-mode.
            EffectHandoffProtocol {
                name: protocol_id("surface_snapshot_alignment"),
                producer_instance: mi_id("external_tool_surface"),
                effect_variant: ev_id("RefreshVisibleSurfaceSet"),
                realizing_actor: act_id("surface_host_owner"),
                correlation_fields: vec![fld_id("snapshot_epoch")],
                obligation_fields: vec![fld_id("snapshot_epoch")],
                allowed_feedback_inputs: vec![FeedbackInputRef {
                    machine_instance: mi_id("external_tool_surface"),
                    input_variant: iv_id("SnapshotAligned"),
                    field_bindings: vec![FeedbackFieldBinding {
                        input_field: fld_id("snapshot_epoch"),
                        source: FeedbackFieldSource::ObligationField(fld_id("snapshot_epoch")),
                    }],
                }],
                closure_policy: ClosurePolicy::AckRequired,
                liveness_annotation: Some(
                    "eventual snapshot acknowledgement under surface host liveness".into(),
                ),
                rust: ProtocolRustBinding {
                    module_path:
                        "meerkat-mcp/src/generated/protocol_surface_snapshot_alignment.rs".into(),
                    generation_mode: ProtocolGenerationMode::EffectExtractor,
                    required_imports: vec![
                        "use crate::external_tool_surface_authority::ExternalToolSurfaceEffect;".into(),
                        "use meerkat_core::handles::{DslTransitionError, ExternalToolSurfaceHandle};".into(),
                    ],
                    authority_type_path: None,
                    mutator_trait_path: None,
                    input_enum_path: None,
                    effect_enum_path: Some(
                        "crate::external_tool_surface_authority::ExternalToolSurfaceEffect".into(),
                    ),
                    transition_type_path: None,
                    error_type_path: None,
                    executor_trigger_input_variant: None,
                    bridge_source_type_path: None,
                    helper_return_shape: ProtocolHelperReturnShape::Obligations,
                    handle_trait_path: Some(
                        "meerkat_core::handles::ExternalToolSurfaceHandle".into(),
                    ),
                    handle_method_names: snapshot_methods,
                    handle_arg_accessors: BTreeMap::new(),
                    handle_method_forwarded_fields: snapshot_forwarded,
                    input_payload_module_path: None,
                    additional_modes: vec![ProtocolGenerationMode::HandleBridge],
                },
            },
        ],
        entry_inputs: vec![],
        routes: vec![],
        route_target_selectors: vec![],
        driver: None,
        transaction_plans: vec![],
        actor_priorities: vec![],
        scheduler_rules: vec![],
        invariants: vec![
            CompositionInvariant {
                name: "surface_completion_protocol_covered".into(),
                kind: CompositionInvariantKind::HandoffProtocolCovered {
                    producer_instance: mi_id("external_tool_surface"),
                    effect_variant: ev_id("ScheduleSurfaceCompletion"),
                    protocol_name: protocol_id("surface_completion"),
                },
                statement:
                    "pending-op completion on a tool surface is returned to the authority only through the explicit `surface_completion` protocol"
                        .into(),
                references_machines: vec![mi_id("external_tool_surface")],
                references_actors: vec![
                    act_id("external_tool_surface_authority"),
                    act_id("surface_host_owner"),
                ],
            },
            CompositionInvariant {
                name: "surface_snapshot_alignment_protocol_covered".into(),
                kind: CompositionInvariantKind::HandoffProtocolCovered {
                    producer_instance: mi_id("external_tool_surface"),
                    effect_variant: ev_id("RefreshVisibleSurfaceSet"),
                    protocol_name: protocol_id("surface_snapshot_alignment"),
                },
                statement:
                    "visible-set refresh acknowledgement crosses back through the explicit `surface_snapshot_alignment` protocol rather than ad-hoc polling"
                        .into(),
                references_machines: vec![mi_id("external_tool_surface")],
                references_actors: vec![
                    act_id("external_tool_surface_authority"),
                    act_id("surface_host_owner"),
                ],
            },
        ],
        witnesses: vec![
            witness("surface_completion_round_trip", &[]),
            witness("surface_snapshot_alignment_round_trip", &[]),
        ],
        deep_domain_cardinality: 3,
        deep_domain_overrides: std::collections::BTreeMap::new(),
        witness_domain_cardinality: 2,
        ci_limits: Some(default_ci_limits()),
        closed_world: true,
    }
}

fn owner_actor(name: &str) -> ActorSchema {
    ActorSchema {
        name: act_id(name),
        kind: ActorKind::Owner,
    }
}

/// Host composition for the `supervisor_trust_publish` and
/// `supervisor_trust_revoke` handoff protocols — C-F2 step-lock
/// formalisation for the trust-edge mutation that runs alongside
/// `BindSupervisor` / `AuthorizeSupervisor` / `RevokeSupervisor`
/// transitions on `MeerkatMachine`.
///
/// The canonical `MeerkatMachine` already owns the authoritative
/// supervisor-binding fact (`supervisor_binding_kind` +
/// `supervisor_bound_*`), but the companion trust edge in
/// `meerkat-comms::Router` was mutated by the shell as a separate step
/// today (see `meerkat-runtime/src/meerkat_machine/dsl.rs` DSL comment
/// `:114-134` and `state-scope-audit.md` §3 row F2).
///
/// C-F2 formalises the step-lock as a generated obligation pair so the
/// companion trust-edge mutation crosses from the supervisor-binding
/// authority back through acknowledged owner feedback, not via a raw
/// shell call that the machine forgets about. The compat
/// `SupervisorTrustBridgeMachine` hosts the `handoff_protocol`
/// annotations; the realising actor `supervisor_bridge_owner`
/// corresponds to `meerkat-runtime::comms_drain`, which calls
/// `meerkat-comms::Router::{add,remove}_trusted_peer(...)` and emits
/// the typed feedback ack through the generated protocol helper.
///
/// Two protocols:
///
/// * `supervisor_trust_publish` — publish trust edge (add trusted
///   peer). Emitted alongside `BindSupervisor` and
///   `AuthorizeSupervisor`. Feedback: `SupervisorTrustEdgePublished`
///   or `SupervisorTrustEdgePublishFailed` (the latter triggers a
///   shell-side rollback of the supervisor-binding DSL commit —
///   preserving the invariant that "trust edge is published iff
///   `supervisor_binding_kind == Bound`").
/// * `supervisor_trust_revoke` — revoke trust edge (remove trusted
///   peer). Emitted alongside `RevokeSupervisor` and during the
///   previous-supervisor cleanup half of `AuthorizeSupervisor`.
///   Feedback: `SupervisorTrustEdgeRevoked` or
///   `SupervisorTrustEdgeRevokeFailed`.
///
/// `closure_policy` is `AckRequired` for both: the shell must feed
/// back success or failure. `liveness_annotation` documents that
/// feedback is eventual under the comms transport's liveness guarantee
/// (the existing `send_bridge_response` path already surfaces typed
/// outcomes).
///
/// Mode: EffectExtractor. The owner (`comms_drain`) consumes the
/// obligation via the generated extractor, calls `router.add_trusted_peer`
/// / `remove_trusted_peer`, and submits feedback through the runtime's
/// existing supervisor trust staging methods. Until those staging methods
/// are lifted behind a sync handle bridge, the generated surface owns the
/// obligation extraction and the hand-written owner owns the async ack.
fn supervisor_trust_bundle_composition() -> CompositionSchema {
    CompositionSchema {
        name: comp_id("supervisor_trust_bundle"),
        machines: vec![MachineInstance {
            instance_id: mi_id("supervisor_trust_bridge"),
            machine_name: mach_id("SupervisorTrustBridgeMachine"),
            actor: act_id("supervisor_trust_bridge_authority"),
        }],
        actors: vec![
            machine_actor("supervisor_trust_bridge_authority"),
            owner_actor("supervisor_bridge_owner"),
        ],
        handoff_protocols: vec![
            EffectHandoffProtocol {
                name: protocol_id("supervisor_trust_publish"),
                producer_instance: mi_id("supervisor_trust_bridge"),
                effect_variant: ev_id("PublishSupervisorTrustEdge"),
                realizing_actor: act_id("supervisor_bridge_owner"),
                // Correlation on `peer_id + epoch` — the same tuple
                // `RevokeSupervisor` guards on (`peer_id_matches_current`
                // + `epoch_matches_current`) so the obligation ack
                // cannot be confused across a rotation.
                correlation_fields: vec![fld_id("peer_id"), fld_id("epoch")],
                obligation_fields: vec![
                    fld_id("peer_id"),
                    fld_id("name"),
                    fld_id("address"),
                    fld_id("epoch"),
                ],
                allowed_feedback_inputs: vec![],
                closure_policy: ClosurePolicy::AckRequired,
                liveness_annotation: Some(
                    "eventual feedback under comms transport liveness — \
                     `send_bridge_response` surfaces the typed outcome"
                        .into(),
                ),
                rust: ProtocolRustBinding {
                    module_path:
                        "meerkat-runtime/src/generated/protocol_supervisor_trust_publish.rs".into(),
                    generation_mode: ProtocolGenerationMode::EffectExtractor,
                    required_imports: vec![
                        "use crate::comms_drain::SupervisorTrustBridgeEffect;".into(),
                    ],
                    // Compat bridge — `SupervisorTrustBridgeMachine` is
                    // intentionally excluded from the canonical TLC state
                    // space (see `compat/supervisor_trust_bridge.rs`). Its
                    // authority surface is realised directly on
                    // `MeerkatMachine`'s DSL (the `SupervisorTrustEdge*`
                    // inputs at `meerkat-runtime dsl.rs:2598-2615`). These
                    // paths point to that authority so the schema validator
                    // accepts the ShellBridge shape; codegen skips this
                    // protocol because `SupervisorTrustBridgeMachine` is
                    // not in the canonical codegen machine list
                    // (`xtask::protocol_codegen` machine registry).
                    authority_type_path: Some(
                        "crate::meerkat_machine::dsl::MeerkatMachineAuthority".into(),
                    ),
                    mutator_trait_path: Some(
                        "crate::meerkat_machine::dsl::MeerkatMachineMutator".into(),
                    ),
                    input_enum_path: Some(
                        "crate::meerkat_machine::dsl::MeerkatMachineInput".into(),
                    ),
                    effect_enum_path: Some(
                        "crate::comms_drain::SupervisorTrustBridgeEffect".into(),
                    ),
                    transition_type_path: Some(
                        "crate::meerkat_machine::dsl::MeerkatMachineTransition".into(),
                    ),
                    error_type_path: Some(
                        "crate::meerkat_machine::dsl::MeerkatMachineTransitionError".into(),
                    ),
                    executor_trigger_input_variant: None,
                    bridge_source_type_path: Some(
                        "crate::comms_drain::SupervisorTrustBridgeEffect".into(),
                    ),
                    helper_return_shape: ProtocolHelperReturnShape::Obligations,
                    handle_trait_path: None,
                    handle_method_names: BTreeMap::new(),
                    handle_arg_accessors: BTreeMap::new(),
                    handle_method_forwarded_fields: BTreeMap::new(),
                    input_payload_module_path: None,
                    additional_modes: vec![],
                },
            },
            EffectHandoffProtocol {
                name: protocol_id("supervisor_trust_revoke"),
                producer_instance: mi_id("supervisor_trust_bridge"),
                effect_variant: ev_id("RevokeSupervisorTrustEdge"),
                realizing_actor: act_id("supervisor_bridge_owner"),
                correlation_fields: vec![fld_id("peer_id"), fld_id("epoch")],
                obligation_fields: vec![fld_id("peer_id"), fld_id("epoch")],
                allowed_feedback_inputs: vec![],
                closure_policy: ClosurePolicy::AckRequired,
                liveness_annotation: Some(
                    "eventual feedback under comms transport liveness — \
                     `send_bridge_response` surfaces the typed outcome"
                        .into(),
                ),
                rust: ProtocolRustBinding {
                    module_path:
                        "meerkat-runtime/src/generated/protocol_supervisor_trust_revoke.rs".into(),
                    generation_mode: ProtocolGenerationMode::EffectExtractor,
                    required_imports: vec![
                        "use crate::comms_drain::SupervisorTrustBridgeEffect;".into(),
                    ],
                    // See `supervisor_trust_publish` above for the
                    // compat-bridge rationale for these authority paths.
                    authority_type_path: Some(
                        "crate::meerkat_machine::dsl::MeerkatMachineAuthority".into(),
                    ),
                    mutator_trait_path: Some(
                        "crate::meerkat_machine::dsl::MeerkatMachineMutator".into(),
                    ),
                    input_enum_path: Some(
                        "crate::meerkat_machine::dsl::MeerkatMachineInput".into(),
                    ),
                    effect_enum_path: Some(
                        "crate::comms_drain::SupervisorTrustBridgeEffect".into(),
                    ),
                    transition_type_path: Some(
                        "crate::meerkat_machine::dsl::MeerkatMachineTransition".into(),
                    ),
                    error_type_path: Some(
                        "crate::meerkat_machine::dsl::MeerkatMachineTransitionError".into(),
                    ),
                    executor_trigger_input_variant: None,
                    bridge_source_type_path: Some(
                        "crate::comms_drain::SupervisorTrustBridgeEffect".into(),
                    ),
                    helper_return_shape: ProtocolHelperReturnShape::Obligations,
                    handle_trait_path: None,
                    handle_method_names: BTreeMap::new(),
                    handle_arg_accessors: BTreeMap::new(),
                    handle_method_forwarded_fields: BTreeMap::new(),
                    input_payload_module_path: None,
                    additional_modes: vec![],
                },
            },
        ],
        entry_inputs: vec![],
        routes: vec![],
        route_target_selectors: vec![],
        driver: None,
        transaction_plans: vec![],
        actor_priorities: vec![],
        scheduler_rules: vec![],
        invariants: vec![
            CompositionInvariant {
                name: "supervisor_trust_publish_protocol_covered".into(),
                kind: CompositionInvariantKind::HandoffProtocolCovered {
                    producer_instance: mi_id("supervisor_trust_bridge"),
                    effect_variant: ev_id("PublishSupervisorTrustEdge"),
                    protocol_name: protocol_id("supervisor_trust_publish"),
                },
                statement: "supervisor trust-edge publication crosses from the supervisor-binding authority back into runtime acknowledgement only through the explicit `supervisor_trust_publish` protocol".into(),
                references_machines: vec![mi_id("supervisor_trust_bridge")],
                references_actors: vec![
                    act_id("supervisor_trust_bridge_authority"),
                    act_id("supervisor_bridge_owner"),
                ],
            },
            CompositionInvariant {
                name: "supervisor_trust_revoke_protocol_covered".into(),
                kind: CompositionInvariantKind::HandoffProtocolCovered {
                    producer_instance: mi_id("supervisor_trust_bridge"),
                    effect_variant: ev_id("RevokeSupervisorTrustEdge"),
                    protocol_name: protocol_id("supervisor_trust_revoke"),
                },
                statement: "supervisor trust-edge revocation crosses from the supervisor-binding authority back into runtime acknowledgement only through the explicit `supervisor_trust_revoke` protocol".into(),
                references_machines: vec![mi_id("supervisor_trust_bridge")],
                references_actors: vec![
                    act_id("supervisor_trust_bridge_authority"),
                    act_id("supervisor_bridge_owner"),
                ],
            },
        ],
        witnesses: vec![
            witness("supervisor_trust_publish_round_trip", &[]),
            witness("supervisor_trust_revoke_round_trip", &[]),
        ],
        deep_domain_cardinality: 2,
        deep_domain_overrides: std::collections::BTreeMap::new(),
        witness_domain_cardinality: 2,
        ci_limits: Some(default_ci_limits()),
        closed_world: true,
    }
}

/// Host composition for the `mob_destroying_session_ingress` handoff
/// obligation — C-F3 step-lock formalisation for the mob-destroy →
/// session-ingress-detach seam.
///
/// State-scope-audit row F3 flagged that `MeerkatMachine` carries a
/// `peer_ingress_mob_id: Option<MobId>` with no structural
/// "mob-exists" invariant (`meerkat-machine-schema/src/catalog/dsl/
/// meerkat_machine.rs:112`). When a mob is destroyed, every session
/// whose peer-ingress ownership was `MobOwned` by that mob must
/// receive `DetachIngress` first — otherwise the session is left
/// holding a dangling `peer_ingress_mob_id` on a now-retired mob. The
/// canonical `meerkat_mob_seam` route `destroy_request_reaches_meerkat`
/// (`RequestRuntimeDestroy` → `Destroy` input) carries no such
/// ordering guarantee; the shell (`meerkat-mob::runtime::actor::
/// handle_destroy`) retires each member before the `Destroy` input
/// lands, but that ordering is convention, not contract.
///
/// C-F3 closes that seam with a generated obligation pair. The
/// `mob_destroying_session_ingress` protocol declares:
///
/// * producer: compat `MobDestroySessionIngressBridgeMachine` effect
///   `RequestSessionIngressDetachForMobDestroy { mob_id,
///   agent_runtime_id }`, emitted from the mob's destroy path before
///   any `RequestRuntimeDestroy` is routed;
/// * realising actor: `mob_destroy_session_ingress_owner`, which calls
///   `DetachIngress` on the target session's `MeerkatMachine`;
/// * feedback: `SessionIngressDetachedForMobDestroy { mob_id,
///   agent_runtime_id }` on success, or
///   `SessionIngressDetachFailedForMobDestroy { mob_id,
///   agent_runtime_id, reason }` on failure (the mob's destroy report
///   surfaces the typed reason and holds off the `RequestRuntimeDestroy`).
///
/// `closure_policy` is `AckRequired`. Correlation on `mob_id +
/// agent_runtime_id` — the same tuple the runtime shell needs to
/// route the detach to the correct session's DSL authority.
///
/// The `xtask seam-inventory` destroy-obligation-pairing check
/// (`## Destroy-obligation Pairing`) asserts that every canonical
/// routed effect whose name contains "Destroy" has a paired handoff
/// protocol that threads the ingress-detach ack before destroy, and
/// this composition is the declarative witness that pair exists for
/// the `meerkat_mob_seam` route.
///
/// Mode: EffectExtractor. The mob runtime's `handle_destroy` path is the
/// effect producer today; when the canonical DSL macro grows
/// `handoff_protocol` syntax the bridge can retire and the effect
/// emission can move to `MobMachine::DestroyMob`.
fn mob_destroy_session_ingress_bundle_composition() -> CompositionSchema {
    CompositionSchema {
        name: comp_id("mob_destroy_session_ingress_bundle"),
        machines: vec![MachineInstance {
            instance_id: mi_id("mob_destroy_session_ingress_bridge"),
            machine_name: mach_id("MobDestroySessionIngressBridgeMachine"),
            actor: act_id("mob_destroy_session_ingress_bridge_authority"),
        }],
        actors: vec![
            machine_actor("mob_destroy_session_ingress_bridge_authority"),
            owner_actor("mob_destroy_session_ingress_owner"),
        ],
        handoff_protocols: vec![EffectHandoffProtocol {
            name: protocol_id("mob_destroying_session_ingress"),
            producer_instance: mi_id("mob_destroy_session_ingress_bridge"),
            effect_variant: ev_id("RequestSessionIngressDetachForMobDestroy"),
            realizing_actor: act_id("mob_destroy_session_ingress_owner"),
            correlation_fields: vec![fld_id("mob_id"), fld_id("agent_runtime_id")],
            obligation_fields: vec![fld_id("mob_id"), fld_id("agent_runtime_id")],
            // C-F3 destroy-obligation pairing: the bridge machine's
            // surface-only inputs already declare the two typed acks
            // (`SessionIngressDetachedForMobDestroy` on success,
            // `SessionIngressDetachFailedForMobDestroy` on failure).
            // Declaring them here wires the schema so the seam-inventory
            // destroy-obligation audit sees the paired detach ack for
            // `MobMachine::RequestRuntimeDestroy` and the `mob-destroy →
            // session-detach` ordering cannot regress silently.
            allowed_feedback_inputs: vec![
                FeedbackInputRef {
                    machine_instance: mi_id("mob_destroy_session_ingress_bridge"),
                    input_variant: iv_id("SessionIngressDetachedForMobDestroy"),
                    field_bindings: vec![
                        FeedbackFieldBinding {
                            input_field: fld_id("mob_id"),
                            source: FeedbackFieldSource::ObligationField(fld_id("mob_id")),
                        },
                        FeedbackFieldBinding {
                            input_field: fld_id("agent_runtime_id"),
                            source: FeedbackFieldSource::ObligationField(fld_id(
                                "agent_runtime_id",
                            )),
                        },
                    ],
                },
                FeedbackInputRef {
                    machine_instance: mi_id("mob_destroy_session_ingress_bridge"),
                    input_variant: iv_id("SessionIngressDetachFailedForMobDestroy"),
                    field_bindings: vec![
                        FeedbackFieldBinding {
                            input_field: fld_id("mob_id"),
                            source: FeedbackFieldSource::ObligationField(fld_id("mob_id")),
                        },
                        FeedbackFieldBinding {
                            input_field: fld_id("agent_runtime_id"),
                            source: FeedbackFieldSource::ObligationField(fld_id(
                                "agent_runtime_id",
                            )),
                        },
                        FeedbackFieldBinding {
                            input_field: fld_id("reason"),
                            source: FeedbackFieldSource::OwnerContext("reason".into()),
                        },
                    ],
                },
            ],
            closure_policy: ClosurePolicy::AckRequired,
            liveness_annotation: Some(
                "eventual feedback: the mob destroy path awaits each session's DetachIngress ack before requesting runtime destroy"
                    .into(),
            ),
            rust: ProtocolRustBinding {
                module_path:
                    "meerkat-mob/src/generated/protocol_mob_destroying_session_ingress.rs"
                        .into(),
                generation_mode: ProtocolGenerationMode::EffectExtractor,
                required_imports: vec![
                    "use crate::runtime::actor::MobDestroySessionIngressBridgeEffect;".into(),
                    "use crate::machines::mob_machine::{AgentRuntimeId, MobId, MobMachineAuthority, MobMachineInput, MobMachineMutator, MobMachineTransition, MobMachineTransitionError};".into(),
                ],
                // Compat bridge — the `MobDestroySessionIngressBridgeMachine`
                // is declarative-only (codegen skips it because its
                // producer machine is not in the canonical codegen
                // machine registry). The DetachIngress ack flows through
                // `MobMachine`'s DSL; these paths point to that authority
                // so the schema validator accepts the ShellBridge shape.
                authority_type_path: Some(
                    "crate::machines::mob_machine::dsl::MobMachineAuthority".into(),
                ),
                mutator_trait_path: Some(
                    "crate::machines::mob_machine::dsl::MobMachineMutator".into(),
                ),
                input_enum_path: Some(
                    "crate::machines::mob_machine::dsl::MobMachineInput".into(),
                ),
                effect_enum_path: Some(
                    "crate::runtime::actor::MobDestroySessionIngressBridgeEffect".into(),
                ),
                transition_type_path: Some(
                    "crate::machines::mob_machine::dsl::MobMachineTransition".into(),
                ),
                error_type_path: Some(
                    "crate::machines::mob_machine::dsl::MobMachineTransitionError".into(),
                ),
                executor_trigger_input_variant: None,
                bridge_source_type_path: Some(
                    "crate::runtime::actor::MobDestroySessionIngressBridgeEffect".into(),
                ),
                helper_return_shape: ProtocolHelperReturnShape::Obligations,
                handle_trait_path: None,
                handle_method_names: BTreeMap::new(),
                handle_arg_accessors: BTreeMap::new(),
                handle_method_forwarded_fields: BTreeMap::new(),
                input_payload_module_path: None,
                additional_modes: vec![],
            },
        }],
        entry_inputs: vec![],
        routes: vec![],
        route_target_selectors: vec![],
        driver: None,
        transaction_plans: vec![],
        actor_priorities: vec![],
        scheduler_rules: vec![],
        invariants: vec![CompositionInvariant {
            name: "mob_destroying_session_ingress_protocol_covered".into(),
            kind: CompositionInvariantKind::HandoffProtocolCovered {
                producer_instance: mi_id("mob_destroy_session_ingress_bridge"),
                effect_variant: ev_id("RequestSessionIngressDetachForMobDestroy"),
                protocol_name: protocol_id("mob_destroying_session_ingress"),
            },
            statement: "mob destroying its runtime crosses from the mob authority back into meerkat acknowledgement only through the explicit `mob_destroying_session_ingress` protocol: DetachIngress is fired against the target session and acknowledged before RequestRuntimeDestroy is routed".into(),
            references_machines: vec![mi_id("mob_destroy_session_ingress_bridge")],
            references_actors: vec![
                act_id("mob_destroy_session_ingress_bridge_authority"),
                act_id("mob_destroy_session_ingress_owner"),
            ],
        }],
        witnesses: vec![witness("mob_destroying_session_ingress_round_trip", &[])],
        deep_domain_cardinality: 2,
        deep_domain_overrides: std::collections::BTreeMap::new(),
        witness_domain_cardinality: 2,
        ci_limits: Some(default_ci_limits()),
        closed_world: true,
    }
}

/// Host composition for the `auth_lease_lifecycle_publication` handoff
/// protocol — wave-d D-c closes the AuthMachine composition-registry
/// orphan. Until this composition landed, the canonical per-binding
/// `AuthMachine` (`catalog::dsl::auth_machine`) was declared in
/// `canonical_machine_schemas()` but had zero references in the
/// composition registry: its `EmitLifecycleEvent { new_state }`
/// External-disposition effect crossed to the runtime's
/// `RuntimeAuthLeaseHandle` owner as an undeclared seam.
///
/// The composition links the canonical `AuthMachine` to the compat
/// `AuthLeaseBridgeMachine` via a route that mirrors the lifecycle
/// event into the bridge's input; the bridge then hosts the
/// `handoff_protocol = Some("auth_lease_lifecycle_publication")`
/// disposition annotation that the canonical DSL macro cannot express
/// today. The pattern mirrors `supervisor_trust_bundle_composition`
/// and `mob_bundle_composition`: a minimal compat bridge hosts the
/// protocol annotation, the canonical machine owns the authoritative
/// state, and a composition Route wires the producer effect into the
/// bridge's mirror input so the seam is discoverable in the registry
/// and `xtask seam-inventory`.
///
/// Mode: `EffectExtractor` with zero declared feedback inputs.
/// `AuthMachine`'s own transitions already carry the authoritative
/// phase progression (`Valid` -> `Expiring` -> `Refreshing` ->
/// {`Valid`, `ReauthRequired`} -> `Released`); the runtime owner
/// consumes the publication purely to refresh the projection consumed
/// by provider callbacks and the refresh/reauth policy surface. There
/// is no feedback input on `AuthMachine` for this obligation — the
/// DSL's state is the source of truth, and no ack is required. Per
/// validator contract
/// (`composition::validate_generation_mode_binding`), an
/// `EffectExtractor` protocol with neither authority plumbing nor any
/// feedback inputs is accepted without requiring a stacked
/// `HandleBridge` mode.
///
/// Actors:
/// - `auth_machine_authority` (Machine) — canonical `AuthMachine`.
/// - `auth_lease_bridge_authority` (Machine) — compat bridge.
/// - `auth_lease_owner` (Owner) — realising actor; corresponds to
///   `meerkat-runtime::handles::auth_lease::RuntimeAuthLeaseHandle`.
fn auth_lease_bundle_composition() -> CompositionSchema {
    CompositionSchema {
        name: comp_id("auth_lease_bundle"),
        machines: vec![
            MachineInstance {
                instance_id: mi_id("auth_machine"),
                machine_name: mach_id("AuthMachine"),
                actor: act_id("auth_machine_authority"),
            },
            MachineInstance {
                instance_id: mi_id("auth_lease_bridge"),
                machine_name: mach_id("AuthLeaseBridgeMachine"),
                actor: act_id("auth_lease_bridge_authority"),
            },
        ],
        actors: vec![
            machine_actor("auth_machine_authority"),
            machine_actor("auth_lease_bridge_authority"),
            owner_actor("auth_lease_owner"),
        ],
        handoff_protocols: vec![EffectHandoffProtocol {
            name: protocol_id("auth_lease_lifecycle_publication"),
            producer_instance: mi_id("auth_lease_bridge"),
            effect_variant: ev_id("PublishLifecycleEvent"),
            realizing_actor: act_id("auth_lease_owner"),
            correlation_fields: vec![fld_id("new_state")],
            obligation_fields: vec![fld_id("new_state")],
            allowed_feedback_inputs: vec![],
            closure_policy: ClosurePolicy::AckRequired,
            liveness_annotation: Some(
                "informative publication: AuthMachine's own transitions carry the \
                 authoritative phase fact; runtime owner refreshes the lease-state \
                 projection under task-scheduling fairness"
                    .into(),
            ),
            rust: ProtocolRustBinding {
                module_path:
                    "meerkat-runtime/src/generated/protocol_auth_lease_lifecycle_publication.rs"
                        .into(),
                generation_mode: ProtocolGenerationMode::EffectExtractor,
                required_imports: vec![
                    "use crate::handles::auth_lease::AuthLeaseBridgeEffect;".into(),
                ],
                authority_type_path: None,
                mutator_trait_path: None,
                input_enum_path: None,
                effect_enum_path: Some("crate::handles::auth_lease::AuthLeaseBridgeEffect".into()),
                transition_type_path: None,
                error_type_path: None,
                executor_trigger_input_variant: None,
                bridge_source_type_path: None,
                helper_return_shape: ProtocolHelperReturnShape::Obligations,
                handle_trait_path: None,
                handle_method_names: BTreeMap::new(),
                handle_arg_accessors: BTreeMap::new(),
                handle_method_forwarded_fields: BTreeMap::new(),
                input_payload_module_path: None,
                additional_modes: vec![],
            },
        }],
        entry_inputs: vec![],
        routes: vec![route(
            "auth_lifecycle_event_crosses_to_bridge",
            "auth_machine",
            "EmitLifecycleEvent",
            "auth_lease_bridge",
            RouteTargetKind::Input,
            "MirrorLifecycleEvent",
            &[bind("new_state", "new_state")],
        )],
        route_target_selectors: vec![],
        driver: None,
        transaction_plans: vec![],
        actor_priorities: vec![],
        scheduler_rules: vec![],
        invariants: vec![
            CompositionInvariant {
                name: "auth_lease_lifecycle_publication_protocol_covered".into(),
                kind: CompositionInvariantKind::HandoffProtocolCovered {
                    producer_instance: mi_id("auth_lease_bridge"),
                    effect_variant: ev_id("PublishLifecycleEvent"),
                    protocol_name: protocol_id("auth_lease_lifecycle_publication"),
                },
                statement: "every AuthMachine lifecycle-phase transition's external \
                    publication crosses into the runtime auth-lease owner through the \
                    explicit `auth_lease_lifecycle_publication` protocol rather than \
                    ad-hoc shell observation"
                    .into(),
                references_machines: vec![mi_id("auth_machine"), mi_id("auth_lease_bridge")],
                references_actors: vec![
                    act_id("auth_machine_authority"),
                    act_id("auth_lease_bridge_authority"),
                    act_id("auth_lease_owner"),
                ],
            },
            CompositionInvariant {
                name: "auth_lifecycle_event_bridge_route_present".into(),
                kind: CompositionInvariantKind::RoutePresent {
                    from_machine: mi_id("auth_machine"),
                    effect_variant: ev_id("EmitLifecycleEvent"),
                    to_machine: mi_id("auth_lease_bridge"),
                    input_variant: rv(RouteTargetKind::Input, "MirrorLifecycleEvent"),
                },
                statement: "canonical AuthMachine lifecycle events reach the \
                    publication-bridge input through the explicit composition route"
                    .into(),
                references_machines: vec![mi_id("auth_machine"), mi_id("auth_lease_bridge")],
                references_actors: vec![
                    act_id("auth_machine_authority"),
                    act_id("auth_lease_bridge_authority"),
                ],
            },
        ],
        witnesses: vec![witness(
            "auth_lease_lifecycle_publication_round_trip",
            &["auth_lifecycle_event_crosses_to_bridge"],
        )],
        deep_domain_cardinality: 2,
        deep_domain_overrides: std::collections::BTreeMap::new(),
        witness_domain_cardinality: 2,
        ci_limits: Some(default_ci_limits()),
        closed_world: true,
    }
}
