// Scoped clippy allows: the helpers below (comp_id, mach_id, act_id, …) use
// `expect()` on hand-authored DSL slugs that parse at construction time. A
// failure here is a DSL-slug authoring bug, never reachable from wire input.
// Inlining `parse(...).expect(...)` at every catalog entry would drown the
// composition definitions in boilerplate. Scope is the whole file because
// every composition builder uses these helpers.
#![allow(clippy::expect_used)]

use std::collections::BTreeMap;

use crate::{
    ActorKind, ActorSchema, ClosurePolicy, CommsTrustAuthorityOperation,
    CommsTrustAuthorityProtocol, CommsTrustAuthoritySourceKind, CompositionDriver,
    CompositionDriverRustBinding, CompositionInvariant, CompositionInvariantKind,
    CompositionSchema, CompositionStateLimits, CompositionTransactionPlan, CompositionWitness,
    CompositionWitnessField, CompositionWitnessInput, CompositionWitnessTransition,
    CompositionWitnessTransitionOrder, DriverDispatchRoute, DriverRefusalClosure,
    DriverRefusalFieldBinding, DriverRefusalFieldSource, DurableMarkerFieldBinding,
    DurableMarkerProtocol, DurableMarkerRelationProtocol, EffectHandoffProtocol,
    EffectTeardownClass, EntryInput, Expr, FeedbackFieldBinding, FeedbackFieldSource,
    FeedbackInputRef, HandleBridgeFeedbackBinding, MachineInstance, ProtocolGenerationMode,
    ProtocolHelperReturnShape, ProtocolRustBinding, Route, RouteBindingSource, RouteDelivery,
    RouteFieldBinding, RouteTarget, RouteTargetKind, RouteVariantId, TeardownObligationClass,
    WatchedEffect,
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
    ActorId, CompositionDriverId, CompositionId, CompositionWitnessId, EffectVariantId,
    EntryInputId, EnumTypeId, EnumVariantId, FieldId, InputVariantId, MachineId, MachineInstanceId,
    PhaseId, ProtocolId, RouteId, StorePrimitiveId, TransactionPlanId, TransactionTriggerId,
    TransitionId,
};

fn comp_id(s: &str) -> CompositionId {
    CompositionId::parse(s).expect("valid composition slug")
}
fn driver_id(s: &str) -> CompositionDriverId {
    CompositionDriverId::parse(s).expect("valid composition-driver slug")
}
fn tx_plan_id(s: &str) -> TransactionPlanId {
    TransactionPlanId::parse(s).expect("valid transaction-plan slug")
}
fn tx_trigger_id(s: &str) -> TransactionTriggerId {
    TransactionTriggerId::parse(s).expect("valid transaction-trigger slug")
}
fn store_primitive_id(s: &str) -> StorePrimitiveId {
    StorePrimitiveId::parse(s).expect("valid store-primitive slug")
}
fn witness_id(s: &str) -> CompositionWitnessId {
    CompositionWitnessId::parse(s).expect("valid witness slug")
}
fn entry_input_id(s: &str) -> EntryInputId {
    EntryInputId::parse(s).expect("valid entry-input slug")
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
fn enum_type_id(s: &str) -> EnumTypeId {
    EnumTypeId::parse(s).expect("valid enum-type slug")
}
fn enum_variant_id(s: &str) -> EnumVariantId {
    EnumVariantId::parse(s).expect("valid enum-variant slug")
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
                &[
                    bind("superseded_by_revision", "superseding_revision"),
                    bind("at_utc_ms", "at_utc_ms"),
                ],
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
                &[],
            ),
            transaction_plan(
                "revision_supersede_and_replan",
                "update_schedule_revision",
                "revision-affecting schedule updates supersede pending future occurrences before replanning",
                "ScheduleStore::commit_schedule_mutation",
                &[],
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
            revision_supersede_route_witness(),
            occurrence_supersede_ack_route_witness(),
            witness("pause_resume_without_revision", &[]),
        ],
        deep_domain_cardinality: 3,
        deep_domain_overrides: std::collections::BTreeMap::new(),
        witness_domain_cardinality: 2,
        ci_limits: Some(default_ci_limits()),
        closed_world: true,
    }
}

/// Durable two-store delivery composition for detached-job outbox entries.
///
/// The outbound route is enqueued because job terminalization and the runtime
/// inbox cannot share a database transaction. The runtime machine mints or
/// reuses the durable sequence; only its committed/reused acknowledgement is
/// routed back to `MarkDeliveryApplied`.
pub fn job_runtime_delivery_composition() -> CompositionSchema {
    CompositionSchema {
        name: comp_id("job_runtime_delivery"),
        machines: vec![
            MachineInstance {
                instance_id: mi_id("job"),
                machine_name: mach_id("DetachedJobMachine"),
                actor: act_id("job_authority"),
            },
            MachineInstance {
                instance_id: mi_id("runtime_delivery"),
                machine_name: mach_id("RuntimeDeliveryMachine"),
                actor: act_id("runtime_delivery_authority"),
            },
        ],
        actors: vec![
            machine_actor("job_authority"),
            machine_actor("runtime_delivery_authority"),
        ],
        handoff_protocols: vec![],
        entry_inputs: vec![],
        routes: vec![
            Route {
                name: route_id("job_terminal_enters_runtime_inbox"),
                from_machine: mi_id("job"),
                effect_variant: ev_id("TerminalCommitted"),
                to: RouteTarget::new(
                    mi_id("runtime_delivery"),
                    rv(RouteTargetKind::Input, "CommitDelivery"),
                ),
                bindings: vec![
                    bind("delivery_id", "job_id"),
                    bind("source_sequence", "delivery_sequence"),
                ],
                delivery: RouteDelivery::Enqueue,
                teardown: None,
            },
            Route {
                name: route_id("job_notification_enters_runtime_inbox"),
                from_machine: mi_id("job"),
                effect_variant: ev_id("NotificationCommitted"),
                to: RouteTarget::new(
                    mi_id("runtime_delivery"),
                    rv(RouteTargetKind::Input, "CommitDelivery"),
                ),
                bindings: vec![
                    bind("delivery_id", "runtime_delivery_id"),
                    bind("source_sequence", "delivery_sequence"),
                ],
                delivery: RouteDelivery::Enqueue,
                teardown: None,
            },
            Route {
                name: route_id("runtime_delivery_commit_acknowledges_job_outbox"),
                from_machine: mi_id("runtime_delivery"),
                effect_variant: ev_id("DeliveryCommitted"),
                to: RouteTarget::new(
                    mi_id("job"),
                    rv(RouteTargetKind::Input, "MarkDeliveryApplied"),
                ),
                bindings: vec![
                    bind("delivery_id", "delivery_id"),
                    bind("delivery_sequence", "source_sequence"),
                ],
                delivery: RouteDelivery::Enqueue,
                teardown: None,
            },
            Route {
                name: route_id("runtime_delivery_reuse_acknowledges_job_outbox"),
                from_machine: mi_id("runtime_delivery"),
                effect_variant: ev_id("DeliveryReused"),
                to: RouteTarget::new(
                    mi_id("job"),
                    rv(RouteTargetKind::Input, "MarkDeliveryApplied"),
                ),
                bindings: vec![
                    bind("delivery_id", "delivery_id"),
                    bind("delivery_sequence", "source_sequence"),
                ],
                delivery: RouteDelivery::Enqueue,
                teardown: None,
            },
        ],
        route_target_selectors: vec![],
        driver: None,
        transaction_plans: vec![
            transaction_plan(
                "job_result_and_outbox",
                "commit_job_delivery",
                "job delivery payload and pending outbox row commit atomically under generated job authority",
                "DetachedJobStore::compare_and_swap",
                &[],
            ),
            transaction_plan(
                "runtime_delivery_identity_and_sequence",
                "commit_runtime_delivery",
                "runtime delivery identity, generated sequence authority, and inbox row commit atomically",
                "RuntimeStore::compare_and_swap_runtime_delivery_authority",
                &[
                    "job_terminal_enters_runtime_inbox",
                    "job_notification_enters_runtime_inbox",
                ],
            ),
            transaction_plan(
                "job_outbox_acknowledgement",
                "acknowledge_runtime_delivery",
                "job outbox acknowledgement commits only after the runtime insert or exact replay succeeds",
                "DetachedJobStore::compare_and_swap",
                &[
                    "runtime_delivery_commit_acknowledges_job_outbox",
                    "runtime_delivery_reuse_acknowledges_job_outbox",
                ],
            ),
        ],
        actor_priorities: vec![],
        scheduler_rules: vec![],
        invariants: vec![
            CompositionInvariant {
                name: "job_terminal_reaches_runtime_delivery_authority".into(),
                kind: CompositionInvariantKind::RoutePresent {
                    from_machine: mi_id("job"),
                    effect_variant: ev_id("TerminalCommitted"),
                    to_machine: mi_id("runtime_delivery"),
                    input_variant: rv(RouteTargetKind::Input, "CommitDelivery"),
                },
                statement: "job terminal outbox entries enter generated runtime delivery authority before any acknowledgement".into(),
                references_machines: vec![mi_id("job"), mi_id("runtime_delivery")],
                references_actors: vec![
                    act_id("job_authority"),
                    act_id("runtime_delivery_authority"),
                ],
            },
            CompositionInvariant {
                name: "job_notification_reaches_runtime_delivery_authority".into(),
                kind: CompositionInvariantKind::RoutePresent {
                    from_machine: mi_id("job"),
                    effect_variant: ev_id("NotificationCommitted"),
                    to_machine: mi_id("runtime_delivery"),
                    input_variant: rv(RouteTargetKind::Input, "CommitDelivery"),
                },
                statement: "job notification outbox entries enter generated runtime delivery authority before any acknowledgement".into(),
                references_machines: vec![mi_id("job"), mi_id("runtime_delivery")],
                references_actors: vec![
                    act_id("job_authority"),
                    act_id("runtime_delivery_authority"),
                ],
            },
            CompositionInvariant {
                name: "runtime_commit_ack_is_the_job_delivery_ack_source".into(),
                kind: CompositionInvariantKind::ObservedRouteInputOriginatesFromEffect {
                    route_name: route_id(
                        "runtime_delivery_commit_acknowledges_job_outbox",
                    ),
                    to_machine: mi_id("job"),
                    input_variant: rv(RouteTargetKind::Input, "MarkDeliveryApplied"),
                    from_machine: mi_id("runtime_delivery"),
                    effect_variant: ev_id("DeliveryCommitted"),
                },
                statement: "a fresh job outbox acknowledgement originates from the generated runtime delivery commit effect".into(),
                references_machines: vec![mi_id("job"), mi_id("runtime_delivery")],
                references_actors: vec![
                    act_id("job_authority"),
                    act_id("runtime_delivery_authority"),
                ],
            },
            CompositionInvariant {
                name: "runtime_reuse_ack_is_the_retry_ack_source".into(),
                kind: CompositionInvariantKind::ObservedRouteInputOriginatesFromEffect {
                    route_name: route_id(
                        "runtime_delivery_reuse_acknowledges_job_outbox",
                    ),
                    to_machine: mi_id("job"),
                    input_variant: rv(RouteTargetKind::Input, "MarkDeliveryApplied"),
                    from_machine: mi_id("runtime_delivery"),
                    effect_variant: ev_id("DeliveryReused"),
                },
                statement: "a replayed projector acknowledgement originates from exact generated runtime delivery reuse".into(),
                references_machines: vec![mi_id("job"), mi_id("runtime_delivery")],
                references_actors: vec![
                    act_id("job_authority"),
                    act_id("runtime_delivery_authority"),
                ],
            },
        ],
        witnesses: vec![
            runtime_delivery_first_commit_witness(),
            runtime_delivery_notification_commit_witness(),
            runtime_delivery_crash_retry_reuse_witness(),
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
                &[
                    bind("superseded_by_revision", "superseding_revision"),
                    bind("at_utc_ms", "at_utc_ms"),
                ],
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
            &[],
        )],
        actor_priorities: vec![],
        scheduler_rules: vec![],
        invariants: vec![],
        witnesses: vec![
            witness("runtime_delivery_feedback", &[]),
            witness("runtime_lease_expiry", &[]),
            revision_supersede_route_witness(),
            occurrence_supersede_ack_route_witness(),
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
                &[
                    bind("superseded_by_revision", "superseding_revision"),
                    bind("at_utc_ms", "at_utc_ms"),
                ],
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
            &[],
        )],
        actor_priorities: vec![],
        scheduler_rules: vec![],
        invariants: vec![],
        witnesses: vec![
            witness("mob_delivery_feedback", &[]),
            witness("materialization_failure_classification", &[]),
            revision_supersede_route_witness(),
            occurrence_supersede_ack_route_witness(),
        ],
        deep_domain_cardinality: 3,
        deep_domain_overrides: std::collections::BTreeMap::new(),
        witness_domain_cardinality: 2,
        ci_limits: Some(default_ci_limits()),
        closed_world: true,
    }
}

pub fn meerkat_mob_seam_composition() -> CompositionSchema {
    let mut handoff_protocols = Vec::new();
    handoff_protocols.extend(mob_bundle_composition().handoff_protocols);
    handoff_protocols.extend(external_tool_bundle_composition().handoff_protocols);
    handoff_protocols.extend(comms_trust_bundle_composition().handoff_protocols);
    handoff_protocols.extend(supervisor_trust_bundle_composition().handoff_protocols);
    handoff_protocols.extend(mob_destroy_session_ingress_bundle_composition().handoff_protocols);

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
        actors: vec![
            machine_actor("meerkat_kernel"),
            machine_actor("mob_kernel"),
            owner_actor("ops_lifecycle_owner"),
            owner_actor("surface_host_owner"),
            owner_actor("comms_trust_reconcile_owner"),
            owner_actor("mob_comms_trust_owner"),
            owner_actor("supervisor_bridge_owner"),
            owner_actor("mob_destroy_session_ingress_owner"),
        ],
        handoff_protocols,
        entry_inputs: vec![
            EntryInput {
                name: entry_input_id("spawn_member"),
                machine: mi_id("mob"),
                // Spawn is now a machine-driven sub-lifecycle: `BeginSpawnExec`
                // opens the per-identity window (the shell then executes each
                // obligation step and `CommitSpawnMembership`/`CommitSpawnActivation`
                // commit the guarded phases). The external entry is the opener.
                input_variant: iv_id("BeginSpawnExec"),
            },
            EntryInput {
                name: entry_input_id("submit_work"),
                machine: mi_id("mob"),
                input_variant: iv_id("SubmitWork"),
            },
            EntryInput {
                name: entry_input_id("retire_member"),
                machine: mi_id("mob"),
                input_variant: iv_id("Retire"),
            },
            EntryInput {
                name: entry_input_id("destroy_mob"),
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
                    bind("fence_token", "fence_token"),
                    bind("generation", "generation"),
                    bind("session_id", "session_id"),
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
            // Typed C-F3 declaration: this is a request-side teardown route.
            // The named `detach_obligation` protocol must declare
            // `TeardownObligationClass::DetachBeforeDestroy` and ack the
            // session-ingress detach before the destroy request is routed.
            Route {
                teardown: Some(EffectTeardownClass::DestroyRequest {
                    detach_obligation: protocol_id("mob_destroying_session_ingress"),
                }),
                ..route(
                    "destroy_request_reaches_meerkat",
                    "mob",
                    "RequestRuntimeDestroy",
                    "meerkat",
                    RouteTargetKind::Input,
                    "Destroy",
                    &[bind("session_id", "session_id")],
                )
            },
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
            // Reply-signal teardown observation: the destroy already
            // happened by the time this fires, so no paired detach
            // obligation is required — declared typed instead of being
            // excluded by a `starts_with("Request")` name filter.
            Route {
                teardown: Some(EffectTeardownClass::TeardownObservation),
                ..route(
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
                )
            },
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
        // emitted through the generated `route_to_signal` surface.
        driver: Some(CompositionDriver {
            name: driver_id("meerkat_mob_seam_driver"),
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
            refusal_closures: vec![
                DriverRefusalClosure {
                    dispatch_route: route_id("binding_request_reaches_meerkat"),
                    feedback_instance: mi_id("mob"),
                    feedback_input: iv_id("ResolveRuntimeBindingRefusal"),
                    field_bindings: vec![
                        refusal_effect_bind("agent_identity", "agent_identity"),
                        refusal_effect_bind("agent_runtime_id", "agent_runtime_id"),
                        refusal_effect_bind("session_id", "session_id"),
                        refusal_code_bind("refusal_code"),
                        refusal_message_bind("reason"),
                    ],
                },
                DriverRefusalClosure {
                    dispatch_route: route_id("work_request_reaches_meerkat"),
                    feedback_instance: mi_id("mob"),
                    feedback_input: iv_id("ResolveRuntimeIngressRefusal"),
                    field_bindings: vec![
                        refusal_effect_bind("agent_runtime_id", "agent_runtime_id"),
                        refusal_effect_bind("fence_token", "fence_token"),
                        refusal_effect_bind("session_id", "session_id"),
                        refusal_effect_bind("work_id", "work_id"),
                        refusal_effect_bind("origin", "origin"),
                        refusal_code_bind("refusal_code"),
                        refusal_message_bind("reason"),
                    ],
                },
                DriverRefusalClosure {
                    dispatch_route: route_id("retire_request_reaches_meerkat"),
                    feedback_instance: mi_id("mob"),
                    feedback_input: iv_id("ResolveRuntimeRetireRefusal"),
                    field_bindings: vec![
                        refusal_effect_bind("agent_identity", "agent_identity"),
                        refusal_effect_bind("agent_runtime_id", "agent_runtime_id"),
                        refusal_effect_bind("session_id", "session_id"),
                        refusal_code_bind("refusal_code"),
                        refusal_message_bind("reason"),
                    ],
                },
            ],
        }),
        transaction_plans: vec![],
        actor_priorities: vec![],
        scheduler_rules: vec![],
        invariants: vec![],
        witnesses: vec![
            basic_round_trip_witness(),
            retire_runtime_path_witness(),
            destroy_runtime_path_witness(),
        ],
        deep_domain_cardinality: 3,
        deep_domain_overrides: std::collections::BTreeMap::new(),
        witness_domain_cardinality: 2,
        ci_limits: Some(default_ci_limits()),
        closed_world: true,
    }
}

pub fn workgraph_attention_bundle_composition() -> CompositionSchema {
    CompositionSchema {
        name: comp_id("workgraph_attention_bundle"),
        machines: vec![
            MachineInstance {
                instance_id: mi_id("workgraph"),
                machine_name: mach_id("WorkGraphLifecycleMachine"),
                actor: act_id("workgraph_authority"),
            },
            MachineInstance {
                instance_id: mi_id("attention"),
                machine_name: mach_id("WorkAttentionLifecycleMachine"),
                actor: act_id("attention_authority"),
            },
        ],
        actors: vec![
            machine_actor("workgraph_authority"),
            machine_actor("attention_authority"),
        ],
        handoff_protocols: vec![],
        entry_inputs: vec![],
        routes: vec![Route {
            name: route_id("work_item_close_stops_attention"),
            from_machine: mi_id("workgraph"),
            effect_variant: ev_id("Closed"),
            to: RouteTarget::new(
                mi_id("attention"),
                rv(RouteTargetKind::Input, "Stop"),
            ),
            bindings: vec![
                bind("at_utc_ms", "at_utc_ms"),
                owner_bind("expected_revision"),
            ],
            delivery: RouteDelivery::Immediate,
            teardown: None,
        }],
        route_target_selectors: vec![],
        driver: None,
        transaction_plans: vec![transaction_plan(
            "transactional_close_stops_attention",
            "close_work_item",
            "terminal work item close atomically stops one co-resident live attention binding; production fan-out applies this transaction per binding",
            "WorkGraphStore::update_item_and_attention_cas",
            &["work_item_close_stops_attention"],
        )],
        actor_priorities: vec![],
        scheduler_rules: vec![],
        invariants: vec![
            CompositionInvariant {
                name: "closed_work_item_routes_to_attention_stop".into(),
                kind: CompositionInvariantKind::RoutePresent {
                    from_machine: mi_id("workgraph"),
                    effect_variant: ev_id("Closed"),
                    to_machine: mi_id("attention"),
                    input_variant: rv(RouteTargetKind::Input, "Stop"),
                },
                statement: "terminal WorkGraph item closure stops co-resident attention bindings through the canonical WorkGraph-to-attention route".into(),
                references_machines: vec![mi_id("workgraph"), mi_id("attention")],
                references_actors: vec![act_id("workgraph_authority"), act_id("attention_authority")],
            },
            CompositionInvariant {
                name: "attention_stop_originates_from_work_item_close".into(),
                kind: CompositionInvariantKind::ObservedRouteInputOriginatesFromEffect {
                    route_name: route_id("work_item_close_stops_attention"),
                    to_machine: mi_id("attention"),
                    input_variant: rv(RouteTargetKind::Input, "Stop"),
                    from_machine: mi_id("workgraph"),
                    effect_variant: ev_id("Closed"),
                },
                statement: "attention stop on terminal item closure is not ad hoc service-only mutation; it originates from the WorkGraph Closed effect route".into(),
                references_machines: vec![mi_id("workgraph"), mi_id("attention")],
                references_actors: vec![act_id("workgraph_authority"), act_id("attention_authority")],
            },
        ],
        witnesses: vec![CompositionWitness {
            name: witness_id("close_stops_attention_route"),
            preload_inputs: vec![
                witness_input(
                    "workgraph",
                    "CreateOpen",
                    vec![
                        witness_field("due_at_utc_ms", Expr::None),
                        witness_field("not_before_utc_ms", Expr::None),
                        witness_field("snoozed_until_utc_ms", Expr::None),
                        witness_field(
                            "completion_policy",
                            named_variant("WorkCompletionPolicy", "SelfAttest"),
                        ),
                        witness_field("completion_supervisor_owner_key", Expr::None),
                        witness_field("completion_reviewer_quorum_threshold", Expr::None),
                        witness_field("unresolved_blocker_count", Expr::U64(0)),
                    ],
                ),
                witness_input(
                    "workgraph",
                    "CloseCompleted",
                    vec![
                        witness_field("expected_revision", Expr::U64(1)),
                        witness_field("at_utc_ms", Expr::U64(1)),
                    ],
                ),
            ],
            expected_routes: vec![route_id("work_item_close_stops_attention")],
            expected_scheduler_rules: vec![],
            expected_states: vec![],
            expected_transitions: vec![
                witness_transition("workgraph", "CreateOpen"),
                witness_transition("workgraph", "CloseOpenCompleted"),
                witness_transition("attention", "StopActive"),
            ],
            expected_transition_order: vec![CompositionWitnessTransitionOrder {
                earlier: witness_transition("workgraph", "CloseOpenCompleted"),
                later: witness_transition("attention", "StopActive"),
            }],
            state_limits: CompositionStateLimits {
                step_limit: 8,
                pending_input_limit: 8,
                pending_route_limit: 8,
                delivered_route_limit: 1,
                emitted_effect_limit: 3,
                seq_limit: 0,
                set_limit: 0,
                map_limit: 0,
            },
        }],
        deep_domain_cardinality: 3,
        deep_domain_overrides: std::collections::BTreeMap::new(),
        witness_domain_cardinality: 2,
        ci_limits: Some(default_ci_limits()),
        closed_world: true,
    }
}

pub fn adaptive_mob_bundle_composition() -> CompositionSchema {
    let mut protocol_templates = comms_trust_bundle_composition()
        .handoff_protocols
        .into_iter()
        .filter(|protocol| protocol.producer_instance == mi_id("mob"))
        .collect::<Vec<_>>();
    protocol_templates.push(
        mob_destroy_session_ingress_bundle_composition()
            .handoff_protocols
            .into_iter()
            .next()
            .expect("mob destroy composition declares destroy-ingress protocol"),
    );
    let mut handoff_protocols = Vec::new();
    for (producer, owner) in [
        ("control_mob", "control_session_ingress_owner"),
        ("layer_mob", "layer_session_ingress_owner"),
    ] {
        for template in &protocol_templates {
            let mut protocol = template.clone();
            protocol.producer_instance = mi_id(producer);
            protocol.rust.module_path = format!(
                "meerkat-mob/src/generated/protocol_adaptive_{producer}_{}.rs",
                template.name.as_str()
            )
            .into();
            if protocol.realizing_actor == act_id("mob_comms_trust_owner") {
                protocol.realizing_actor = act_id("adaptive_mob_comms_trust_owner");
            } else if protocol.realizing_actor == act_id("mob_destroy_session_ingress_owner") {
                protocol.realizing_actor = act_id(owner);
            }
            for feedback in &mut protocol.allowed_feedback_inputs {
                if feedback.machine_instance == mi_id("mob") {
                    feedback.machine_instance = mi_id(producer);
                }
            }
            handoff_protocols.push(protocol);
        }
    }

    CompositionSchema {
        name: comp_id("adaptive_mob_bundle"),
        machines: vec![
            MachineInstance {
                instance_id: mi_id("control_mob"),
                machine_name: mach_id("MobMachine"),
                actor: act_id("control_mob_authority"),
            },
            MachineInstance {
                instance_id: mi_id("layer_mob"),
                machine_name: mach_id("MobMachine"),
                actor: act_id("layer_mob_authority"),
            },
        ],
        actors: vec![
            machine_actor("control_mob_authority"),
            machine_actor("layer_mob_authority"),
            owner_actor("adaptive_mob_comms_trust_owner"),
            owner_actor("control_session_ingress_owner"),
            owner_actor("layer_session_ingress_owner"),
        ],
        handoff_protocols,
        entry_inputs: vec![],
        routes: vec![route(
            "layer_terminal_reaches_adaptive_kernel",
            "layer_mob",
            "FlowRunPublicResultClassified",
            "control_mob",
            RouteTargetKind::Input,
            "IngestLayerTerminal",
            &[
                owner_bind("adaptive_run_id"),
                owner_bind("layer_id"),
                owner_bind("attempt"),
                bind("result_class", "result"),
                owner_bind("actual_tokens"),
                owner_bind("actual_tool_calls"),
            ],
        )],
        route_target_selectors: vec![],
        driver: Some(CompositionDriver {
            name: driver_id("adaptive_mob_bundle_driver"),
            rust: CompositionDriverRustBinding {
                module_path: "meerkat-mob/src/generated/adaptive_mob_bundle.rs".into(),
                driver_type: "AdaptiveMobBundleDriver".into(),
                store_plan_type: "AdaptiveMobBundleStorePlan".into(),
                work_type: "AdaptiveMobBundleWork".into(),
                decision_type: "AdaptiveMobBundleDecision".into(),
                required_imports: vec![],
            },
            watched_effects: vec![WatchedEffect {
                producer_instance: mi_id("layer_mob"),
                effect_variant: ev_id("FlowRunPublicResultClassified"),
            }],
            dispatch_routes: vec![DriverDispatchRoute {
                name: route_id("layer_terminal_reaches_adaptive_kernel"),
                target_instance: mi_id("control_mob"),
                target_kind: RouteTargetKind::Input,
                input_variant: RouteVariantId::Input(iv_id("IngestLayerTerminal")),
            }],
            refusal_closures: vec![],
        }),
        transaction_plans: vec![transaction_plan(
            "adaptive_layer_terminal_feedback",
            "ingest_layer_terminal",
            "adaptive bundle driver enriches a terminal layer-mob result with the stored adaptive run/layer context before feeding the control mob kernel",
            "AdaptiveMobBundleDriver::ingest_layer_terminal",
            &["layer_terminal_reaches_adaptive_kernel"],
        )],
        actor_priorities: vec![],
        scheduler_rules: vec![],
        invariants: vec![
            CompositionInvariant {
                name: "layer_terminal_feedback_route_present".into(),
                kind: CompositionInvariantKind::RoutePresent {
                    from_machine: mi_id("layer_mob"),
                    effect_variant: ev_id("FlowRunPublicResultClassified"),
                    to_machine: mi_id("control_mob"),
                    input_variant: rv(RouteTargetKind::Input, "IngestLayerTerminal"),
                },
                statement: "terminal layer-mob public result classification feeds the adaptive control mob through the canonical generated route".into(),
                references_machines: vec![mi_id("layer_mob"), mi_id("control_mob")],
                references_actors: vec![
                    act_id("layer_mob_authority"),
                    act_id("control_mob_authority"),
                ],
            },
            CompositionInvariant {
                name: "control_mob_destroying_session_ingress_protocol_covered".into(),
                kind: CompositionInvariantKind::HandoffProtocolCovered {
                    producer_instance: mi_id("control_mob"),
                    effect_variant: ev_id("RequestSessionIngressDetachForMobDestroy"),
                    protocol_name: protocol_id("mob_destroying_session_ingress"),
                },
                statement: "control mob destroy keeps the canonical detach-before-destroy session ingress handoff explicit inside the adaptive bundle".into(),
                references_machines: vec![mi_id("control_mob")],
                references_actors: vec![
                    act_id("control_mob_authority"),
                    act_id("control_session_ingress_owner"),
                ],
            },
            CompositionInvariant {
                name: "layer_mob_destroying_session_ingress_protocol_covered".into(),
                kind: CompositionInvariantKind::HandoffProtocolCovered {
                    producer_instance: mi_id("layer_mob"),
                    effect_variant: ev_id("RequestSessionIngressDetachForMobDestroy"),
                    protocol_name: protocol_id("mob_destroying_session_ingress"),
                },
                statement: "layer mob destroy keeps the canonical detach-before-destroy session ingress handoff explicit inside the adaptive bundle".into(),
                references_machines: vec![mi_id("layer_mob")],
                references_actors: vec![
                    act_id("layer_mob_authority"),
                    act_id("layer_session_ingress_owner"),
                ],
            },
        ],
        witnesses: vec![CompositionWitness {
            name: witness_id("layer_terminal_feedback"),
            preload_inputs: vec![witness_input(
                "layer_mob",
                "ClassifyFlowRunPublicResult",
                vec![
                    witness_field("run_id", Expr::String("runid_1".into())),
                    witness_field("status", named_variant("FlowRunStatus", "Completed")),
                ],
            )],
            expected_routes: vec![route_id("layer_terminal_reaches_adaptive_kernel")],
            expected_scheduler_rules: vec![],
            expected_states: vec![],
            expected_transitions: vec![witness_transition(
                "layer_mob",
                "ClassifyFlowRunPublicResultSuccessRunning",
            )],
            expected_transition_order: vec![],
            state_limits: adaptive_mob_ci_limits(),
        }],
        deep_domain_cardinality: 3,
        deep_domain_overrides: std::collections::BTreeMap::new(),
        witness_domain_cardinality: 1,
        ci_limits: Some(adaptive_mob_ci_limits()),
        // Adaptive bundles are dynamic: the driver provisions concrete
        // sessions/layer mobs through runtime surfaces, so the composition
        // declares the adaptive MobMachine handoff protocols and driver
        // feedback path without claiming a closed set of session routes.
        closed_world: false,
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
        teardown: None,
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

fn owner_bind(to_field: &str) -> RouteFieldBinding {
    RouteFieldBinding {
        to_field: fld_id(to_field),
        source: RouteBindingSource::OwnerProvided,
    }
}

fn refusal_effect_bind(input_field: &str, effect_field: &str) -> DriverRefusalFieldBinding {
    DriverRefusalFieldBinding {
        input_field: fld_id(input_field),
        source: DriverRefusalFieldSource::EffectField(fld_id(effect_field)),
    }
}

fn refusal_code_bind(input_field: &str) -> DriverRefusalFieldBinding {
    DriverRefusalFieldBinding {
        input_field: fld_id(input_field),
        source: DriverRefusalFieldSource::ConsumerErrorCode,
    }
}

fn refusal_message_bind(input_field: &str) -> DriverRefusalFieldBinding {
    DriverRefusalFieldBinding {
        input_field: fld_id(input_field),
        source: DriverRefusalFieldSource::ConsumerErrorMessage,
    }
}

fn witness_input(
    machine: &str,
    input_variant: &str,
    fields: Vec<CompositionWitnessField>,
) -> CompositionWitnessInput {
    CompositionWitnessInput {
        machine: mi_id(machine),
        input_variant: iv_id(input_variant),
        fields,
    }
}

fn witness_field(field: &str, expr: Expr) -> CompositionWitnessField {
    CompositionWitnessField {
        field: fld_id(field),
        expr,
    }
}

fn witness_transition(machine: &str, transition: &str) -> CompositionWitnessTransition {
    CompositionWitnessTransition {
        machine: mi_id(machine),
        transition: transition_id(transition),
    }
}

fn named_variant(enum_name: &str, variant: &str) -> Expr {
    Expr::NamedVariant {
        enum_name: enum_type_id(enum_name),
        variant: enum_variant_id(variant),
    }
}

fn some_string(value: &str) -> Expr {
    Expr::Some(Box::new(Expr::String(value.into())))
}

fn seam_runtime_session_registration_input() -> CompositionWitnessInput {
    witness_input(
        "meerkat",
        "RegisterSession",
        vec![witness_field(
            "session_id",
            Expr::String("sessionid_1".into()),
        )],
    )
}

fn seam_authorize_spawn_profile_input() -> CompositionWitnessInput {
    witness_input(
        "mob",
        "AuthorizeSpawnProfile",
        vec![
            witness_field("agent_identity", Expr::String("agentidentity_1".into())),
            witness_field("profile_name", Expr::String("profile_1".into())),
            witness_field("model", Expr::String("model_1".into())),
            witness_field("profile_material_digest", Expr::String("digest_1".into())),
            witness_field("tool_config_digest", Expr::String("toolconfig_1".into())),
            witness_field("skills_digest", Expr::String("skills_1".into())),
            witness_field("provider_params_digest", Expr::None),
            witness_field("output_schema_digest", Expr::None),
            witness_field("external_addressable", Expr::Bool(true)),
            witness_field("resolved_spec_digest", Expr::None),
        ],
    )
}

// Spawn is now a machine-driven phase ladder (BeginSpawnExec opener ->
// CommitSpawnMembership -> CommitSpawnActivation). The `RequestRuntimeBinding`
// effect that the cross-machine routes depend on is emitted by
// `CommitSpawnMembershipFresh` (the renamed `SpawnRunningFresh`), NOT by the
// opener — so the seam witnesses DRIVE the ladder to MembershipCommitted via two
// preload inputs (BeginSpawnExec, then CommitSpawnMembership). The ladder tracks
// no per-step obligations, so no obligation-close input is needed in between.
fn seam_begin_spawn_exec_input() -> CompositionWitnessInput {
    witness_input(
        "mob",
        "BeginSpawnExec",
        vec![
            witness_field("agent_identity", Expr::String("agentidentity_1".into())),
            witness_field("agent_runtime_id", Expr::String("agentruntimeid_1".into())),
            witness_field("fence_token", Expr::U64(1)),
            witness_field("generation", Expr::U64(1)),
            witness_field("profile_material_digest", Expr::String("digest_1".into())),
            witness_field("external_addressable", Expr::Bool(true)),
            witness_field(
                "runtime_mode",
                named_variant("SpawnPolicyRuntimeMode", "AutonomousHost"),
            ),
            witness_field("bridge_session_id", some_string("sessionid_1")),
            witness_field("replacing", Expr::None),
            witness_field("placement", Expr::None),
            witness_field("workgraph_required", Expr::Bool(false)),
            witness_field("rust_bundles_present", Expr::Bool(false)),
            witness_field("per_spawn_external_tools_present", Expr::Bool(false)),
            witness_field("mob_default_external_tools_present", Expr::Bool(false)),
            witness_field("default_llm_client_override_present", Expr::Bool(false)),
            witness_field("host_surface_mcp_allowlist_present", Expr::Bool(false)),
            witness_field("inherited_tool_filter_present", Expr::Bool(false)),
            witness_field("shell_env_present", Expr::Bool(false)),
            witness_field("mcp_stdio_env_present", Expr::Bool(false)),
            witness_field("mcp_http_headers_present", Expr::Bool(false)),
            witness_field("memory_required", Expr::Bool(false)),
            witness_field("mcp_required", Expr::Bool(false)),
            witness_field("resume_session_id", Expr::None),
            witness_field("placed_spawn_id", Expr::None),
            witness_field("placed_provision_operation_id", Expr::None),
            witness_field("placed_operation_owner_session_id", Expr::None),
            witness_field("effective_profile_override_present", Expr::Bool(false)),
            witness_field("effective_model_override_present", Expr::Bool(false)),
        ],
    )
}

fn seam_commit_spawn_membership_input() -> CompositionWitnessInput {
    witness_input(
        "mob",
        "CommitSpawnMembership",
        vec![
            witness_field("agent_identity", Expr::String("agentidentity_1".into())),
            witness_field("agent_runtime_id", Expr::String("agentruntimeid_1".into())),
            witness_field("fence_token", Expr::U64(1)),
            witness_field("generation", Expr::U64(1)),
            witness_field("profile_material_digest", Expr::String("digest_1".into())),
            witness_field("external_addressable", Expr::Bool(true)),
            witness_field(
                "runtime_mode",
                named_variant("SpawnPolicyRuntimeMode", "AutonomousHost"),
            ),
            witness_field("bridge_session_id", some_string("sessionid_1")),
            witness_field("replacing", Expr::None),
            witness_field("member_peer_endpoint", Expr::None),
            witness_field("spec_digest_echo", Expr::None),
            witness_field("ack_engine_version", Expr::None),
            witness_field("placed_spawn_id", Expr::None),
            witness_field("provision_operation_id", Expr::None),
        ],
    )
}

fn seam_submit_work_input() -> CompositionWitnessInput {
    witness_input(
        "mob",
        "SubmitWork",
        vec![
            witness_field("agent_identity", Expr::String("agentidentity_1".into())),
            witness_field("agent_runtime_id", Expr::String("agentruntimeid_1".into())),
            witness_field("fence_token", Expr::U64(1)),
            witness_field("work_id", Expr::String("workid_1".into())),
            witness_field("origin", named_variant("WorkOrigin", "External")),
        ],
    )
}

fn seam_retire_input() -> CompositionWitnessInput {
    witness_input(
        "mob",
        "Retire",
        vec![
            witness_field("mob_id", Expr::String("mobid_1".into())),
            witness_field("agent_runtime_id", Expr::String("agentruntimeid_1".into())),
            witness_field("agent_identity", Expr::String("agentidentity_1".into())),
            witness_field("generation", Expr::U64(1)),
            witness_field("releasing", some_string("sessionid_1")),
            witness_field("session_id", some_string("sessionid_1")),
        ],
    )
}

fn seam_destroy_mob_signal() -> CompositionWitnessInput {
    witness_input(
        "mob",
        "DestroyMob",
        vec![witness_field(
            "session_id",
            Expr::String("sessionid_1".into()),
        )],
    )
}

fn basic_round_trip_witness() -> CompositionWitness {
    CompositionWitness {
        name: witness_id("basic_round_trip"),
        preload_inputs: vec![
            seam_runtime_session_registration_input(),
            seam_authorize_spawn_profile_input(),
            seam_begin_spawn_exec_input(),
            seam_commit_spawn_membership_input(),
            seam_submit_work_input(),
        ],
        expected_routes: vec![
            route_id("binding_request_reaches_meerkat"),
            route_id("work_request_reaches_meerkat"),
            route_id("runtime_bound_reaches_mob"),
        ],
        expected_scheduler_rules: vec![],
        expected_states: vec![],
        expected_transitions: vec![
            witness_transition("meerkat", "RegisterSessionIdle"),
            witness_transition("mob", "AuthorizeSpawnProfileRunning"),
            // Machine-driven spawn phase ladder: opener -> commit. The membership
            // commit (renamed SpawnRunningFresh) is what emits RequestRuntimeBinding,
            // so both must be takeable in Next. No obligation-close step in between.
            witness_transition("mob", "BeginSpawnExecFresh"),
            witness_transition("mob", "CommitSpawnMembershipFresh"),
            witness_transition("meerkat", "PrepareBindingsIdle"),
            witness_transition("mob", "ObserveRuntimeReady"),
            witness_transition("mob", "SubmitWorkRunningExternal"),
            witness_transition("meerkat", "IngestAttached"),
        ],
        expected_transition_order: vec![],
        // Shorter ladder: 4 preload inputs (-1 event-close) and 8 expected
        // transitions (-1 closer). CommitSpawnMembershipFresh now emits only the
        // 4 verbatim membership effects (the 2 post-commit Request* nudges are
        // gone), and BeginSpawnExecFresh emits nothing. Path effect total = 7
        // (Authorize 1 + Commit 4 + PrepareBindings 1 + SubmitWork 1); steps ~15
        // (4 injects + 8 transitions + 3 route deliveries).
        state_limits: meerkat_mob_seam_witness_limits(16, 3, 9),
    }
}

fn retire_runtime_path_witness() -> CompositionWitness {
    CompositionWitness {
        name: witness_id("retire_runtime_path"),
        preload_inputs: vec![
            seam_runtime_session_registration_input(),
            seam_authorize_spawn_profile_input(),
            seam_begin_spawn_exec_input(),
            seam_commit_spawn_membership_input(),
            seam_retire_input(),
        ],
        expected_routes: vec![
            route_id("retire_request_reaches_meerkat"),
            route_id("runtime_retired_reaches_mob"),
        ],
        expected_scheduler_rules: vec![],
        expected_states: vec![],
        expected_transitions: vec![
            witness_transition("meerkat", "RegisterSessionIdle"),
            witness_transition("mob", "AuthorizeSpawnProfileRunning"),
            // Machine-driven spawn phase ladder (see basic_round_trip_witness).
            witness_transition("mob", "BeginSpawnExecFresh"),
            witness_transition("mob", "CommitSpawnMembershipFresh"),
            witness_transition("meerkat", "PrepareBindingsIdle"),
            witness_transition("mob", "ObserveRuntimeReady"),
            witness_transition("mob", "RetireRunningReleasing"),
            witness_transition("meerkat", "RetireRequestedFromIdle"),
            witness_transition("mob", "ObserveRuntimeRetired"),
        ],
        expected_transition_order: vec![],
        // Shorter ladder (-1 preload, -1 closer transition); fewer spawn effects.
        // Path effect total = 13 (Authorize 1 + Commit 4 + PrepareBindings 1 +
        // RetireRunningReleasing 5 + RetireRequested 1 + ObserveRetired 1); steps
        // ~17 (4 injects + 9 transitions + 4 route deliveries).
        state_limits: meerkat_mob_seam_witness_limits(18, 4, 14),
    }
}

fn destroy_runtime_path_witness() -> CompositionWitness {
    CompositionWitness {
        name: witness_id("destroy_runtime_path"),
        preload_inputs: vec![
            seam_runtime_session_registration_input(),
            seam_authorize_spawn_profile_input(),
            seam_begin_spawn_exec_input(),
            seam_commit_spawn_membership_input(),
            seam_destroy_mob_signal(),
        ],
        expected_routes: vec![
            route_id("destroy_request_reaches_meerkat"),
            route_id("runtime_destroyed_reaches_mob"),
        ],
        expected_scheduler_rules: vec![],
        expected_states: vec![],
        expected_transitions: vec![
            witness_transition("meerkat", "RegisterSessionIdle"),
            witness_transition("mob", "AuthorizeSpawnProfileRunning"),
            // Machine-driven spawn phase ladder (see basic_round_trip_witness).
            witness_transition("mob", "BeginSpawnExecFresh"),
            witness_transition("mob", "CommitSpawnMembershipFresh"),
            witness_transition("meerkat", "PrepareBindingsIdle"),
            witness_transition("mob", "ObserveRuntimeReady"),
            witness_transition("mob", "DestroyMob"),
            witness_transition("meerkat", "Destroy"),
        ],
        expected_transition_order: vec![],
        // Shorter ladder (-1 preload, -1 closer transition); fewer spawn effects.
        // Path effect total = 8 (Authorize 1 + Commit 4 + PrepareBindings 1 +
        // DestroyMob 1 + Destroy 1); steps ~16 (4 injects + 8 transitions + 4
        // route deliveries).
        state_limits: meerkat_mob_seam_witness_limits(17, 4, 9),
    }
}

fn effect_extractor_rust_binding(
    module_path: &str,
    required_imports: &[&str],
    effect_enum_path: &str,
    transition_type_path: &str,
) -> ProtocolRustBinding {
    ProtocolRustBinding {
        module_path: module_path.into(),
        generation_mode: ProtocolGenerationMode::EffectExtractor,
        required_imports: required_imports
            .iter()
            .map(|import| (*import).into())
            .collect(),
        authority_type_path: None,
        mutator_trait_path: None,
        input_enum_path: None,
        effect_enum_path: Some(effect_enum_path.into()),
        transition_type_path: Some(transition_type_path.into()),
        error_type_path: None,
        executor_trigger_input_variant: None,
        bridge_source_type_path: None,
        helper_return_shape: ProtocolHelperReturnShape::Obligations,
        handle_trait_path: None,
        handle_feedback_bindings: vec![],
        input_payload_module_path: None,
        additional_modes: vec![],
    }
}

struct TrustHandoffProtocolSpec<'a> {
    name: &'a str,
    producer_instance: &'a str,
    effect_variant: &'a str,
    realizing_actor: &'a str,
    source_kind: CommsTrustAuthoritySourceKind,
    row_owner_kind: Option<CommsTrustAuthoritySourceKind>,
    allowed_operations: &'a [CommsTrustAuthorityOperation],
    obligation_fields: &'a [&'a str],
    module_path: &'a str,
    required_imports: &'a [&'a str],
    effect_enum_path: &'a str,
    transition_type_path: &'a str,
}

fn trust_handoff_protocol(spec: TrustHandoffProtocolSpec<'_>) -> EffectHandoffProtocol {
    EffectHandoffProtocol {
        name: protocol_id(spec.name),
        producer_instance: mi_id(spec.producer_instance),
        effect_variant: ev_id(spec.effect_variant),
        realizing_actor: act_id(spec.realizing_actor),
        correlation_fields: spec
            .obligation_fields
            .iter()
            .map(|field| fld_id(field))
            .collect(),
        obligation_fields: spec
            .obligation_fields
            .iter()
            .map(|field| fld_id(field))
            .collect(),
        allowed_feedback_inputs: vec![],
        closure_policy: ClosurePolicy::PublicationOnly,
        liveness_annotation: Some(
            "generated authority publication is consumed by the owning runtime; no source-machine feedback is declared"
                .into(),
        ),
        comms_trust_authority: Some(CommsTrustAuthorityProtocol {
            source_kind: spec.source_kind,
            row_owner_kind: spec.row_owner_kind,
            allowed_operations: spec.allowed_operations.to_vec(),
        }),
        durable_marker: None,
        teardown: None,
        rust: effect_extractor_rust_binding(
            spec.module_path,
            spec.required_imports,
            spec.effect_enum_path,
            spec.transition_type_path,
        ),
    }
}

fn transaction_plan(
    name: &str,
    trigger: &str,
    description: &str,
    store_primitive: &str,
    route_names: &[&str],
) -> CompositionTransactionPlan {
    CompositionTransactionPlan {
        name: tx_plan_id(name),
        trigger: tx_trigger_id(trigger),
        description: description.into(),
        store_primitive: store_primitive_id(store_primitive),
        // Routes this plan's store primitive atomically realizes. An
        // owner-provided route binding (which carries no producer-side type) is
        // anchored by declaring its route here, satisfying the composition's
        // OwnerProvided fail-closed check via an explicit synchronous-transaction
        // contract rather than silent unconstrained owner trust.
        route_names: route_names.iter().map(|r| route_id(r)).collect(),
        protocol_names: vec![],
    }
}

fn witness(name: &str, expected_routes: &[&str]) -> CompositionWitness {
    CompositionWitness {
        name: witness_id(name),
        preload_inputs: vec![],
        expected_routes: expected_routes.iter().map(|r| route_id(r)).collect(),
        expected_scheduler_rules: vec![],
        expected_states: vec![],
        expected_transitions: vec![],
        expected_transition_order: vec![],
        state_limits: default_ci_limits(),
    }
}

fn revision_supersede_route_witness() -> CompositionWitness {
    CompositionWitness {
        name: witness_id("revision_supersede_route"),
        preload_inputs: vec![witness_input(
            "schedule",
            "Revise",
            vec![
                witness_field("trigger_key", Expr::String("triggerkey_1".into())),
                witness_field(
                    "target_binding_key",
                    Expr::String("targetbindingid_1".into()),
                ),
                witness_field("misfire_policy", named_variant("MisfirePolicy", "Skip")),
                witness_field(
                    "overlap_policy",
                    named_variant("OverlapPolicy", "SkipIfRunning"),
                ),
                witness_field(
                    "missing_target_policy",
                    named_variant("MissingTargetPolicy", "MarkMisfired"),
                ),
                witness_field("planning_horizon_days", Expr::U64(1)),
                witness_field("planning_horizon_occurrences", Expr::U64(2)),
                witness_field("at_utc_ms", Expr::U64(1)),
            ],
        )],
        expected_routes: vec![route_id("revision_supersede_enters_occurrence_authority")],
        expected_scheduler_rules: vec![],
        expected_states: vec![],
        expected_transitions: vec![witness_transition("schedule", "ReviseActive")],
        expected_transition_order: vec![],
        state_limits: schedule_supersede_route_witness_limits(),
    }
}

fn occurrence_supersede_ack_route_witness() -> CompositionWitness {
    CompositionWitness {
        name: witness_id("occurrence_supersede_ack_route"),
        preload_inputs: vec![witness_input(
            "occurrence",
            "Supersede",
            vec![
                witness_field("superseded_by_revision", Expr::U64(2)),
                witness_field("at_utc_ms", Expr::U64(1)),
            ],
        )],
        expected_routes: vec![route_id("occurrence_supersede_ack_returns_to_schedule")],
        expected_scheduler_rules: vec![],
        expected_states: vec![],
        expected_transitions: vec![witness_transition("occurrence", "SupersedePendingOrLive")],
        expected_transition_order: vec![],
        state_limits: schedule_supersede_route_witness_limits(),
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

fn job_terminal_witness_inputs() -> Vec<CompositionWitnessInput> {
    vec![
        witness_input(
            "job",
            "Submit",
            vec![
                witness_field("job_id", Expr::String("job_1".into())),
                witness_field(
                    "restart_class",
                    named_variant("DetachedJobRestartClass", "Adoptable"),
                ),
            ],
        ),
        witness_input(
            "job",
            "ClaimAttempt",
            vec![
                witness_field("attempt_id", Expr::String("attempt_1".into())),
                witness_field("worker_id", Expr::String("worker_1".into())),
                witness_field("claimed_at_ms", Expr::U64(1)),
                witness_field("lease_expires_at_ms", Expr::U64(2)),
                witness_field("runner_handle", Expr::String("runner_1".into())),
            ],
        ),
        witness_input(
            "job",
            "CompleteAttempt",
            vec![
                witness_field("attempt_id", Expr::String("attempt_1".into())),
                witness_field("fence", Expr::U64(1)),
                witness_field("completed_at_ms", Expr::U64(2)),
            ],
        ),
    ]
}

fn runtime_delivery_witness_limits() -> CompositionStateLimits {
    CompositionStateLimits {
        step_limit: 12,
        pending_input_limit: 8,
        pending_route_limit: 4,
        delivered_route_limit: 3,
        emitted_effect_limit: 6,
        seq_limit: 0,
        set_limit: 2,
        map_limit: 2,
    }
}

fn runtime_delivery_first_commit_witness() -> CompositionWitness {
    CompositionWitness {
        name: witness_id("runtime_delivery_first_commit"),
        preload_inputs: job_terminal_witness_inputs(),
        expected_routes: vec![
            route_id("job_terminal_enters_runtime_inbox"),
            route_id("runtime_delivery_commit_acknowledges_job_outbox"),
        ],
        expected_scheduler_rules: vec![],
        expected_states: vec![],
        expected_transitions: vec![
            witness_transition("job", "SubmitQueued"),
            witness_transition("job", "ClaimQueued"),
            witness_transition("job", "CompleteRunningAttempt"),
            witness_transition("runtime_delivery", "CommitNewDelivery"),
            witness_transition("job", "ApplySucceededDelivery"),
        ],
        expected_transition_order: vec![],
        state_limits: runtime_delivery_witness_limits(),
    }
}

fn runtime_delivery_notification_commit_witness() -> CompositionWitness {
    CompositionWitness {
        name: witness_id("runtime_delivery_notification_commit"),
        preload_inputs: vec![
            witness_input(
                "job",
                "Submit",
                vec![
                    witness_field("job_id", Expr::String("job_1".into())),
                    witness_field(
                        "restart_class",
                        named_variant("DetachedJobRestartClass", "CheckpointResumable"),
                    ),
                ],
            ),
            witness_input(
                "job",
                "ClaimAttempt",
                vec![
                    witness_field("attempt_id", Expr::String("attempt_1".into())),
                    witness_field("worker_id", Expr::String("worker_1".into())),
                    witness_field("claimed_at_ms", Expr::U64(1)),
                    witness_field("lease_expires_at_ms", Expr::U64(2)),
                    witness_field("runner_handle", Expr::String("runner_1".into())),
                ],
            ),
            witness_input(
                "job",
                "EmitNotification",
                vec![
                    witness_field("attempt_id", Expr::String("attempt_1".into())),
                    witness_field("fence", Expr::U64(1)),
                    witness_field("notification_id", Expr::String("notification_1".into())),
                    witness_field("idempotency_key", Expr::String("key_1".into())),
                    witness_field(
                        "runtime_delivery_id",
                        Expr::String("job_1:notification:notification_1".into()),
                    ),
                    witness_field("observed_at_ms", Expr::U64(2)),
                ],
            ),
        ],
        expected_routes: vec![
            route_id("job_notification_enters_runtime_inbox"),
            route_id("runtime_delivery_commit_acknowledges_job_outbox"),
        ],
        expected_scheduler_rules: vec![],
        expected_states: vec![],
        expected_transitions: vec![
            witness_transition("job", "SubmitQueued"),
            witness_transition("job", "ClaimQueued"),
            witness_transition("job", "EmitRunningNotification"),
            witness_transition("runtime_delivery", "CommitNewDelivery"),
            witness_transition("job", "ApplyRunningNotificationDelivery"),
        ],
        expected_transition_order: vec![],
        state_limits: runtime_delivery_witness_limits(),
    }
}

fn runtime_delivery_crash_retry_reuse_witness() -> CompositionWitness {
    let mut preload_inputs = job_terminal_witness_inputs();
    preload_inputs.push(witness_input(
        "runtime_delivery",
        "CommitDelivery",
        vec![
            witness_field("delivery_id", Expr::String("job_1".into())),
            witness_field("source_sequence", Expr::U64(1)),
        ],
    ));
    CompositionWitness {
        name: witness_id("runtime_delivery_crash_retry_reuse"),
        preload_inputs,
        expected_routes: vec![
            route_id("job_terminal_enters_runtime_inbox"),
            route_id("runtime_delivery_reuse_acknowledges_job_outbox"),
        ],
        expected_scheduler_rules: vec![],
        expected_states: vec![],
        expected_transitions: vec![
            witness_transition("runtime_delivery", "CommitNewDelivery"),
            witness_transition("job", "SubmitQueued"),
            witness_transition("job", "ClaimQueued"),
            witness_transition("job", "CompleteRunningAttempt"),
            witness_transition("runtime_delivery", "ReuseCommittedDelivery"),
            witness_transition("job", "ApplySucceededDelivery"),
        ],
        expected_transition_order: vec![],
        state_limits: runtime_delivery_witness_limits(),
    }
}

fn meerkat_mob_seam_witness_limits(
    step_limit: u32,
    delivered_route_limit: u32,
    emitted_effect_limit: u32,
) -> CompositionStateLimits {
    CompositionStateLimits {
        // step_limit is the per-witness model_step_count cap (injects +
        // transition firings + route deliveries). The machine-driven spawn phase
        // ladder adds 1 preload inject (BeginSpawnExec, before CommitSpawnMembership)
        // and 1 transition firing (BeginSpawnExecFresh) over the old single Spawn.
        step_limit,
        pending_input_limit: 2,
        pending_route_limit: 1,
        delivered_route_limit,
        emitted_effect_limit,
        seq_limit: 1,
        set_limit: 8,
        map_limit: 2,
    }
}

fn schedule_supersede_route_witness_limits() -> CompositionStateLimits {
    CompositionStateLimits {
        step_limit: 3,
        pending_input_limit: 2,
        pending_route_limit: 1,
        delivered_route_limit: 1,
        emitted_effect_limit: 2,
        seq_limit: 0,
        set_limit: 0,
        map_limit: 0,
    }
}

fn adaptive_mob_ci_limits() -> CompositionStateLimits {
    CompositionStateLimits {
        step_limit: 1,
        pending_input_limit: 2,
        pending_route_limit: 1,
        delivered_route_limit: 1,
        emitted_effect_limit: 1,
        seq_limit: 0,
        set_limit: 0,
        map_limit: 0,
    }
}

/// Host composition for the `ops_barrier_satisfaction` handoff protocol.
///
/// The producer is the canonical `MeerkatMachine` which declares
/// `WaitAllSatisfied { wait_request_id, run_id, operation_ids }` as a
/// local-disposition effect. The realizing owner — the runtime's ops
/// lifecycle shell — observes the barrier closure and feeds back
/// through the `OpsBarrierSatisfied { run_id, operation_ids }` input; the
/// `HandleBridge` mode routes the same payload through
/// `TurnStateHandle::ops_barrier_satisfied` for runtime-backed sessions.
///
/// Modes: primary `ShellBridge` (accept + authority.apply submitters),
/// secondary `HandleBridge` (handle-driven submitter suffixed `_handle`).
fn mob_bundle_composition() -> CompositionSchema {
    // Handle method takes `run_id: RunId` and
    // `operation_ids: BTreeSet<OperationId>` — the obligation field is
    // `Set<OperationId>` which renders as `Vec<OperationId>`. The accessor
    // consumes the obligation vector into the handle's typed set.
    let mut handle_accessors = BTreeMap::new();
    handle_accessors.insert(fld_id("operation_ids"), ".into_iter().collect()".into());

    CompositionSchema {
        name: comp_id("mob_bundle"),
        machines: vec![MachineInstance {
            instance_id: mi_id("meerkat"),
            machine_name: mach_id("MeerkatMachine"),
            actor: act_id("meerkat_kernel"),
        }],
        actors: vec![machine_actor("meerkat_kernel"), owner_actor("ops_lifecycle_owner")],
        handoff_protocols: vec![EffectHandoffProtocol {
            name: protocol_id("ops_barrier_satisfaction"),
            producer_instance: mi_id("meerkat"),
                effect_variant: ev_id("WaitAllSatisfied"),
                realizing_actor: act_id("ops_lifecycle_owner"),
                correlation_fields: vec![fld_id("run_id"), fld_id("operation_ids")],
            obligation_fields: vec![fld_id("run_id"), fld_id("operation_ids")],
                allowed_feedback_inputs: vec![FeedbackInputRef {
                    machine_instance: mi_id("meerkat"),
                    input_variant: iv_id("OpsBarrierSatisfied"),
                    field_bindings: vec![
                        FeedbackFieldBinding {
                            input_field: fld_id("run_id"),
                            source: FeedbackFieldSource::ObligationField(fld_id("run_id")),
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
            comms_trust_authority: None,
            durable_marker: None,
            teardown: None,
            rust: ProtocolRustBinding {
                module_path: "meerkat-core/src/generated/protocol_ops_barrier_satisfaction.rs"
                    .into(),
                // Primary mode: HandleBridge. The obligation is built
                // from the shell-owned `WaitAllSatisfied` struct via the
                // shared `accept_<effect>` emission; feedback flows
                // through the `TurnStateHandle::ops_barrier_satisfied`
                // trait method. The canonical `MeerkatMachine`
                // declares the handoff protocol directly; the shell
                // struct is the source projection consumed by the
                // generated helper.
                generation_mode: ProtocolGenerationMode::HandleBridge,
                required_imports: vec![
                    "use crate::handles::{DslTransitionError, TurnStateHandle};".into(),
                    "use crate::{OperationId, RunId};".into(),
                ],
                authority_type_path: None,
                mutator_trait_path: None,
                input_enum_path: None,
                effect_enum_path: None,
                transition_type_path: None,
                error_type_path: None,
                executor_trigger_input_variant: None,
                bridge_source_type_path: None,
                helper_return_shape: ProtocolHelperReturnShape::Obligations,
                handle_trait_path: Some("meerkat_core::handles::TurnStateHandle".into()),
                handle_feedback_bindings: vec![HandleBridgeFeedbackBinding {
                    input_variant: iv_id("OpsBarrierSatisfied"),
                    method_name: "ops_barrier_satisfied".into(),
                    arg_accessors: handle_accessors,
                    forwarded_fields: Some(vec![fld_id("run_id"), fld_id("operation_ids")]),
                }],
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
                producer_instance: mi_id("meerkat"),
                effect_variant: ev_id("WaitAllSatisfied"),
                protocol_name: protocol_id("ops_barrier_satisfaction"),
            },
            statement: "wait-all barrier satisfaction crosses from the ops lifecycle owner back into turn-state authority only through the explicit `ops_barrier_satisfaction` protocol".into(),
            references_machines: vec![mi_id("meerkat")],
            references_actors: vec![act_id("meerkat_kernel"), act_id("ops_lifecycle_owner")],
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
/// Both protocols' producer-side effects are emitted from the canonical
/// `MeerkatMachine` surface-boundary effect set and consumed through the
/// runtime's hand-written `ExternalToolSurfaceAuthority` effect enum.
/// The schema keeps those field names and typed atoms in parity so the
/// generated helper remains a checked boundary rather than an ad-hoc
/// shell convention.
///
/// - `surface_completion` — EffectExtractor (scans `ExternalToolSurfaceEffect`
///   for `ScheduleSurfaceCompletion` variants) + HandleBridge
///   (`mark_pending_succeeded` / `mark_pending_failed` on
///   `ExternalToolSurfaceHandle`). No authority submitter is emitted —
///   feedback flows through the handle.
/// - `surface_snapshot_alignment` — EffectExtractor + HandleBridge
///   (`snapshot_aligned`). Same shape, single field.
fn external_tool_bundle_composition() -> CompositionSchema {
    CompositionSchema {
        name: comp_id("external_tool_bundle"),
        machines: vec![MachineInstance {
            instance_id: mi_id("meerkat"),
            machine_name: mach_id("MeerkatMachine"),
            actor: act_id("meerkat_kernel"),
        }],
        actors: vec![machine_actor("meerkat_kernel"), owner_actor("surface_host_owner")],
        handoff_protocols: vec![
            // Protocol 1: surface_completion — dual-mode emission
            // (EffectExtractor + HandleBridge).
            EffectHandoffProtocol {
                name: protocol_id("surface_completion"),
                producer_instance: mi_id("meerkat"),
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
                        machine_instance: mi_id("meerkat"),
                        input_variant: iv_id("SurfaceMarkPendingSucceeded"),
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
                        machine_instance: mi_id("meerkat"),
                        input_variant: iv_id("SurfaceMarkPendingFailed"),
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
                            FeedbackFieldBinding {
                                input_field: fld_id("cause"),
                                source: FeedbackFieldSource::OwnerContext("cause".into()),
                            },
                        ],
                    },
                ],
                closure_policy: ClosurePolicy::AckRequired,
                liveness_annotation: Some(
                    "eventual feedback under surface connection liveness".into(),
                ),
                comms_trust_authority: None,
                durable_marker: None,
                teardown: None,
                rust: ProtocolRustBinding {
                    module_path: "meerkat-mcp/src/generated/protocol_surface_completion.rs".into(),
                    generation_mode: ProtocolGenerationMode::EffectExtractor,
                    required_imports: vec![
                        "use meerkat_core::handles::{DslTransitionError, ExternalToolSurfaceEffect, ExternalToolSurfaceHandle};".into(),
                        "use meerkat_core::tool_scope::{ExternalToolSurfaceDeltaOperation, ExternalToolSurfaceFailureCause};".into(),
                    ],
                    // No authority submitters; feedback flows through the
                    // handle only. EffectExtractor emits `extract_obligations`
                    // only (no authority.apply submitter).
                    authority_type_path: None,
                    mutator_trait_path: None,
                    input_enum_path: None,
                    effect_enum_path: Some(
                        "meerkat_core::handles::ExternalToolSurfaceEffect".into(),
                    ),
                    transition_type_path: None,
                    error_type_path: None,
                    executor_trigger_input_variant: None,
                    bridge_source_type_path: None,
                    helper_return_shape: ProtocolHelperReturnShape::Obligations,
                    handle_trait_path: Some(
                        "meerkat_core::handles::ExternalToolSurfaceHandle".into(),
                    ),
                    handle_feedback_bindings: vec![
                        HandleBridgeFeedbackBinding {
                            input_variant: iv_id("SurfaceMarkPendingSucceeded"),
                            method_name: "mark_pending_succeeded".into(),
                            arg_accessors: BTreeMap::new(),
                            forwarded_fields: Some(vec![
                                fld_id("surface_id"),
                                fld_id("pending_task_sequence"),
                                fld_id("staged_intent_sequence"),
                            ]),
                        },
                        HandleBridgeFeedbackBinding {
                            input_variant: iv_id("SurfaceMarkPendingFailed"),
                            method_name: "mark_pending_failed".into(),
                            arg_accessors: BTreeMap::new(),
                            forwarded_fields: Some(vec![
                                fld_id("surface_id"),
                                fld_id("pending_task_sequence"),
                                fld_id("staged_intent_sequence"),
                                fld_id("cause"),
                            ]),
                        },
                    ],
                    input_payload_module_path: None,
                    additional_modes: vec![ProtocolGenerationMode::HandleBridge],
                },
            },
            // Protocol 2: surface_snapshot_alignment — dual-mode.
            EffectHandoffProtocol {
                name: protocol_id("surface_snapshot_alignment"),
                producer_instance: mi_id("meerkat"),
                effect_variant: ev_id("RefreshVisibleSurfaceSet"),
                realizing_actor: act_id("surface_host_owner"),
                correlation_fields: vec![fld_id("snapshot_epoch")],
                obligation_fields: vec![fld_id("snapshot_epoch")],
                allowed_feedback_inputs: vec![FeedbackInputRef {
                    machine_instance: mi_id("meerkat"),
                    input_variant: iv_id("SurfaceSnapshotAligned"),
                    field_bindings: vec![FeedbackFieldBinding {
                        input_field: fld_id("epoch"),
                        source: FeedbackFieldSource::ObligationField(fld_id("snapshot_epoch")),
                    }],
                }],
                closure_policy: ClosurePolicy::AckRequired,
                liveness_annotation: Some(
                    "eventual snapshot acknowledgement under surface host liveness".into(),
                ),
                comms_trust_authority: None,
                durable_marker: None,
                teardown: None,
                rust: ProtocolRustBinding {
                    module_path:
                        "meerkat-mcp/src/generated/protocol_surface_snapshot_alignment.rs".into(),
                    generation_mode: ProtocolGenerationMode::EffectExtractor,
                    required_imports: vec![
                        "use meerkat_core::handles::{DslTransitionError, ExternalToolSurfaceEffect, ExternalToolSurfaceHandle};".into(),
                    ],
                    authority_type_path: None,
                    mutator_trait_path: None,
                    input_enum_path: None,
                    effect_enum_path: Some(
                        "meerkat_core::handles::ExternalToolSurfaceEffect".into(),
                    ),
                    transition_type_path: None,
                    error_type_path: None,
                    executor_trigger_input_variant: None,
                    bridge_source_type_path: None,
                    helper_return_shape: ProtocolHelperReturnShape::Obligations,
                    handle_trait_path: Some(
                        "meerkat_core::handles::ExternalToolSurfaceHandle".into(),
                    ),
                    handle_feedback_bindings: vec![HandleBridgeFeedbackBinding {
                        input_variant: iv_id("SurfaceSnapshotAligned"),
                        method_name: "snapshot_aligned".into(),
                        arg_accessors: BTreeMap::new(),
                        forwarded_fields: Some(vec![fld_id("snapshot_epoch")]),
                    }],
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
                    producer_instance: mi_id("meerkat"),
                    effect_variant: ev_id("ScheduleSurfaceCompletion"),
                    protocol_name: protocol_id("surface_completion"),
                },
                statement:
                    "pending-op completion on a tool surface is returned to the authority only through the explicit `surface_completion` protocol"
                        .into(),
                references_machines: vec![mi_id("meerkat")],
                references_actors: vec![act_id("meerkat_kernel"), act_id("surface_host_owner")],
            },
            CompositionInvariant {
                name: "surface_snapshot_alignment_protocol_covered".into(),
                kind: CompositionInvariantKind::HandoffProtocolCovered {
                    producer_instance: mi_id("meerkat"),
                    effect_variant: ev_id("RefreshVisibleSurfaceSet"),
                    protocol_name: protocol_id("surface_snapshot_alignment"),
                },
                statement:
                    "visible-set refresh acknowledgement crosses back through the explicit `surface_snapshot_alignment` protocol rather than ad-hoc polling"
                        .into(),
                references_machines: vec![mi_id("meerkat")],
                references_actors: vec![act_id("meerkat_kernel"), act_id("surface_host_owner")],
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

/// Host composition for comms trust projection mutation obligations.
///
/// The MeerkatMachine and MobMachine own the semantic peer/trust facts. These
/// protocols turn their emitted effects into typed obligations consumed by the
/// runtime/mob comms owners; the owners may only mutate the comms projection by
/// carrying one of these obligations through generated protocol helpers.
fn comms_trust_bundle_composition() -> CompositionSchema {
    let member_wiring_imports = [
        "use crate::machines::mob_machine::{AgentIdentity, MemberPeerEndpoint, MobMachineAuthority, MobMachineEffect, MobMachineTransition, MobPhase, PeerId, WiringEdge};",
    ];
    let member_imports = [
        "use crate::machines::mob_machine::{MemberPeerEndpoint, MobMachineEffect, MobMachineTransition, PeerId, WiringEdge};",
    ];
    let member_overlay_imports = [
        "use crate::machines::mob_machine::{AgentIdentity, MemberPeerEndpoint, MobMachineEffect, MobMachineTransition, PeerId};",
    ];
    let external_imports = [
        "use crate::machines::mob_machine::{ExternalPeerEdge, MobMachineEffect, MobMachineTransition, PeerId};",
    ];
    let reciprocal_imports = [
        "use crate::machines::mob_machine::{ExternalPeerEdge, ExternalPeerKey, MemberPeerEndpoint, MobMachineEffect, MobMachineTransition, PeerId};",
    ];
    CompositionSchema {
        name: comp_id("comms_trust_bundle"),
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
        actors: vec![
            machine_actor("meerkat_kernel"),
            machine_actor("mob_kernel"),
            owner_actor("comms_trust_reconcile_owner"),
            owner_actor("mob_comms_trust_owner"),
        ],
        handoff_protocols: vec![
            trust_handoff_protocol(TrustHandoffProtocolSpec {
                name: "comms_trust_reconcile",
                producer_instance: "meerkat",
                effect_variant: "CommsTrustReconcileRequested",
                realizing_actor: "comms_trust_reconcile_owner",
                source_kind: CommsTrustAuthoritySourceKind::MeerkatMachinePeerProjection,
                row_owner_kind: None,
                allowed_operations: &[
                    CommsTrustAuthorityOperation::PublicAdd,
                    CommsTrustAuthorityOperation::PublicRemove,
                ],
                obligation_fields: &[
                    "local_endpoint",
                    "peer_projection_epoch",
                    "direct_peer_endpoints",
                    "mob_overlay_peer_endpoints",
                ],
                module_path: "meerkat-runtime/src/generated/protocol_comms_trust_reconcile.rs",
                required_imports: &[
                    "use crate::meerkat_machine::dsl::{MeerkatMachineEffect, MeerkatMachineTransition, PeerEndpoint};",
                ],
                effect_enum_path: "crate::meerkat_machine::dsl::MeerkatMachineEffect",
                transition_type_path: "crate::meerkat_machine::dsl::MeerkatMachineTransition",
            }),
            trust_handoff_protocol(TrustHandoffProtocolSpec {
                name: "mob_member_trust_wiring",
                producer_instance: "mob",
                effect_variant: "MemberTrustWiringRequested",
                realizing_actor: "mob_comms_trust_owner",
                source_kind: CommsTrustAuthoritySourceKind::MobMachineMemberTrustWiring,
                row_owner_kind: None,
                allowed_operations: &[CommsTrustAuthorityOperation::PublicAdd],
                obligation_fields: &[
                    "edge",
                    "a_peer_id",
                    "b_peer_id",
                    "a_endpoint",
                    "b_endpoint",
                    "epoch",
                ],
                module_path: "meerkat-mob/src/generated/protocol_mob_member_trust_wiring.rs",
                required_imports: &member_wiring_imports,
                effect_enum_path: "crate::machines::mob_machine::MobMachineEffect",
                transition_type_path: "crate::machines::mob_machine::MobMachineTransition",
            }),
            trust_handoff_protocol(TrustHandoffProtocolSpec {
                name: "mob_member_trust_unwiring",
                producer_instance: "mob",
                effect_variant: "MemberTrustUnwiringRequested",
                realizing_actor: "mob_comms_trust_owner",
                source_kind: CommsTrustAuthoritySourceKind::MobMachineMemberTrustUnwiring,
                row_owner_kind: Some(CommsTrustAuthoritySourceKind::MobMachineMemberTrustWiring),
                allowed_operations: &[CommsTrustAuthorityOperation::PublicRemove],
                obligation_fields: &["edge", "a_peer_id", "b_peer_id", "epoch"],
                module_path: "meerkat-mob/src/generated/protocol_mob_member_trust_unwiring.rs",
                required_imports: &member_imports,
                effect_enum_path: "crate::machines::mob_machine::MobMachineEffect",
                transition_type_path: "crate::machines::mob_machine::MobMachineTransition",
            }),
            EffectHandoffProtocol {
                name: protocol_id("mob_member_peer_overlay"),
                producer_instance: mi_id("mob"),
                effect_variant: ev_id("MemberPeerOverlayAuthorized"),
                realizing_actor: act_id("mob_comms_trust_owner"),
                correlation_fields: vec![
                    fld_id("agent_identity"),
                    fld_id("peer_id"),
                    fld_id("epoch"),
                ],
                obligation_fields: vec![
                    fld_id("agent_identity"),
                    fld_id("peer_id"),
                    fld_id("peer_overlay_endpoints"),
                    fld_id("epoch"),
                ],
                allowed_feedback_inputs: vec![],
                closure_policy: ClosurePolicy::PublicationOnly,
                liveness_annotation: Some(
                    "generated MobMachine peer overlay publication is consumed by the bridge owner before MeerkatMachine peer projection reconciliation"
                        .into(),
                ),
                comms_trust_authority: None,
                durable_marker: None,
                teardown: None,
                rust: effect_extractor_rust_binding(
                    "meerkat-mob/src/generated/protocol_mob_member_peer_overlay.rs",
                    &member_overlay_imports,
                    "crate::machines::mob_machine::MobMachineEffect",
                    "crate::machines::mob_machine::MobMachineTransition",
                ),
            },
            trust_handoff_protocol(TrustHandoffProtocolSpec {
                name: "mob_external_peer_trust_wiring",
                producer_instance: "mob",
                effect_variant: "ExternalPeerTrustWiringRequested",
                realizing_actor: "mob_comms_trust_owner",
                source_kind: CommsTrustAuthoritySourceKind::MobMachineExternalPeerTrustWiring,
                row_owner_kind: None,
                allowed_operations: &[CommsTrustAuthorityOperation::PublicAdd],
                obligation_fields: &["edge", "local_peer_id", "peer_id", "epoch"],
                module_path: "meerkat-mob/src/generated/protocol_mob_external_peer_trust_wiring.rs",
                required_imports: &external_imports,
                effect_enum_path: "crate::machines::mob_machine::MobMachineEffect",
                transition_type_path: "crate::machines::mob_machine::MobMachineTransition",
            }),
            trust_handoff_protocol(TrustHandoffProtocolSpec {
                name: "mob_external_peer_trust_unwiring",
                producer_instance: "mob",
                effect_variant: "ExternalPeerTrustUnwiringRequested",
                realizing_actor: "mob_comms_trust_owner",
                source_kind: CommsTrustAuthoritySourceKind::MobMachineExternalPeerTrustUnwiring,
                row_owner_kind: Some(
                    CommsTrustAuthoritySourceKind::MobMachineExternalPeerTrustWiring,
                ),
                allowed_operations: &[CommsTrustAuthorityOperation::PublicRemove],
                obligation_fields: &["edge", "local_peer_id", "peer_id", "epoch"],
                module_path: "meerkat-mob/src/generated/protocol_mob_external_peer_trust_unwiring.rs",
                required_imports: &external_imports,
                effect_enum_path: "crate::machines::mob_machine::MobMachineEffect",
                transition_type_path: "crate::machines::mob_machine::MobMachineTransition",
            }),
            trust_handoff_protocol(TrustHandoffProtocolSpec {
                name: "mob_external_peer_trust_repair",
                producer_instance: "mob",
                effect_variant: "ExternalPeerTrustRepairRequested",
                realizing_actor: "mob_comms_trust_owner",
                source_kind: CommsTrustAuthoritySourceKind::MobMachineExternalPeerTrustRepair,
                row_owner_kind: Some(
                    CommsTrustAuthoritySourceKind::MobMachineExternalPeerTrustWiring,
                ),
                allowed_operations: &[CommsTrustAuthorityOperation::PublicAdd],
                obligation_fields: &["edge", "local_peer_id", "peer_id", "epoch"],
                module_path: "meerkat-mob/src/generated/protocol_mob_external_peer_trust_repair.rs",
                required_imports: &external_imports,
                effect_enum_path: "crate::machines::mob_machine::MobMachineEffect",
                transition_type_path: "crate::machines::mob_machine::MobMachineTransition",
            }),
            trust_handoff_protocol(TrustHandoffProtocolSpec {
                name: "mob_external_peer_reciprocal_trust",
                producer_instance: "mob",
                effect_variant: "ExternalPeerReciprocalTrustRequested",
                realizing_actor: "mob_comms_trust_owner",
                source_kind: CommsTrustAuthoritySourceKind::MobMachineExternalPeerReciprocalTrust,
                row_owner_kind: Some(
                    CommsTrustAuthoritySourceKind::MobMachineExternalPeerTrustWiring,
                ),
                allowed_operations: &[CommsTrustAuthorityOperation::PublicAdd],
                obligation_fields: &["key", "edge", "peer_id", "peer_endpoint", "epoch"],
                module_path: "meerkat-mob/src/generated/protocol_mob_external_peer_reciprocal_trust.rs",
                required_imports: &reciprocal_imports,
                effect_enum_path: "crate::machines::mob_machine::MobMachineEffect",
                transition_type_path: "crate::machines::mob_machine::MobMachineTransition",
            }),
        ],
        entry_inputs: vec![],
        routes: vec![],
        route_target_selectors: vec![],
        driver: None,
        transaction_plans: vec![],
        actor_priorities: vec![],
        scheduler_rules: vec![],
        invariants: vec![CompositionInvariant {
            name: "mob_member_peer_overlay_protocol_covered".into(),
            kind: CompositionInvariantKind::HandoffProtocolCovered {
                producer_instance: mi_id("mob"),
                effect_variant: ev_id("MemberPeerOverlayAuthorized"),
                protocol_name: protocol_id("mob_member_peer_overlay"),
            },
            statement: "peer-only bridge trust overlay facts cross from MobMachine into bridge reconciliation only through the explicit `mob_member_peer_overlay` generated handoff protocol".into(),
            references_machines: vec![mi_id("mob"), mi_id("meerkat")],
            references_actors: vec![act_id("mob_kernel"), act_id("mob_comms_trust_owner")],
        }],
        witnesses: vec![witness("comms_trust_projection_round_trip", &[])],
        deep_domain_cardinality: 2,
        deep_domain_overrides: std::collections::BTreeMap::new(),
        witness_domain_cardinality: 2,
        ci_limits: Some(default_ci_limits()),
        closed_world: true,
    }
}

/// Host composition for the durable supervisor-binding trust protocols and
/// the structurally separate one-shot response-route protocol — C-F2 step-lock
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
/// C-F2 formalises the step-lock as a generated obligation publication
/// so the companion trust-edge mutation crosses from the
/// supervisor-binding authority through minted generated trust-mutation
/// authority, not via a raw shell call that the machine forgets about. The canonical
/// `MeerkatMachine` effects host the `handoff_protocol` annotations;
/// the realising actor `supervisor_bridge_owner` corresponds to
/// `meerkat-runtime::comms_drain`, which calls
/// `meerkat-comms::Router::{add,remove}_trusted_peer(...)` only with a
/// generated `CommsTrustMutationAuthority`.
///
/// Two protocols:
///
/// * `supervisor_trust_publish` — publish trust edge (add trusted
///   peer). Emitted alongside `BindSupervisor` and
///   `AuthorizeSupervisor`. The generated obligation authorizes the
///   private trust add/removal cleanup authority for the declared peer
///   and epoch.
/// * `supervisor_trust_revoke` — revoke trust edge (remove trusted
///   peer). Emitted alongside `RevokeSupervisor` and during the
///   previous-supervisor cleanup half of `AuthorizeSupervisor`. The
///   generated obligation authorizes only the matching private trust
///   removal.
///
/// `closure_policy` is `PublicationOnly` for both: the generated
/// obligation mints the only trust-mutation authority the owning shell
/// can spend, and there is no feedback input back into `MeerkatMachine`.
/// `liveness_annotation` documents that publication is consumed under
/// the comms transport's liveness guarantee.
///
/// Mode: EffectExtractor. The owner (`comms_drain`) consumes the
/// obligation via the generated extractor, calls `router.add_trusted_peer`
/// / `remove_trusted_peer` with the generated trust-mutation authority.
/// The generated surface owns obligation extraction and authority minting;
/// the hand-written owner can only spend that authority.
fn supervisor_trust_bundle_composition() -> CompositionSchema {
    CompositionSchema {
        name: comp_id("supervisor_trust_bundle"),
        machines: vec![MachineInstance {
            instance_id: mi_id("meerkat"),
            machine_name: mach_id("MeerkatMachine"),
            actor: act_id("meerkat_kernel"),
        }],
        actors: vec![machine_actor("meerkat_kernel"), owner_actor("supervisor_bridge_owner")],
        handoff_protocols: vec![
            EffectHandoffProtocol {
                name: protocol_id("supervisor_trust_publish"),
                producer_instance: mi_id("meerkat"),
                effect_variant: ev_id("PublishSupervisorTrustEdge"),
                realizing_actor: act_id("supervisor_bridge_owner"),
                // Correlation on `peer_id + epoch` — the same tuple
                // `RevokeSupervisor` guards on (`peer_id_matches_current`
                // + `epoch_matches_current`) so the obligation ack
                // cannot be confused across a rotation.
                correlation_fields: vec![fld_id("peer_id"), fld_id("epoch")],
                obligation_fields: vec![
                    fld_id("local_endpoint"),
                    fld_id("peer_id"),
                    fld_id("name"),
                    fld_id("address"),
                    fld_id("signing_public_key"),
                    fld_id("epoch"),
                ],
                allowed_feedback_inputs: vec![],
                closure_policy: ClosurePolicy::PublicationOnly,
                liveness_annotation: Some(
                    "generated supervisor trust authority publication is consumed under comms transport liveness"
                        .into(),
                ),
                comms_trust_authority: Some(CommsTrustAuthorityProtocol {
                    source_kind: CommsTrustAuthoritySourceKind::MeerkatMachineSupervisorPublish,
                    row_owner_kind: None,
                    allowed_operations: vec![
                        CommsTrustAuthorityOperation::PrivateAdd,
                        CommsTrustAuthorityOperation::PrivateRemove,
                    ],
                }),
                durable_marker: None,
                teardown: None,
                rust: ProtocolRustBinding {
                    module_path:
                        "meerkat-runtime/src/generated/protocol_supervisor_trust_publish.rs".into(),
                    generation_mode: ProtocolGenerationMode::EffectExtractor,
                    required_imports: vec![
                        "use crate::meerkat_machine::dsl::{MeerkatMachineEffect, MeerkatMachineTransition, PeerEndpoint};".into(),
                    ],
                    authority_type_path: Some(
                        "crate::meerkat_machine::dsl::MeerkatMachineAuthority".into(),
                    ),
                    mutator_trait_path: Some(
                        "crate::meerkat_machine::dsl::MeerkatMachineMutator".into(),
                    ),
                    input_enum_path: Some(
                        "crate::meerkat_machine::dsl::MeerkatMachineInput".into(),
                    ),
                    effect_enum_path: Some("crate::meerkat_machine::dsl::MeerkatMachineEffect".into()),
                    transition_type_path: Some(
                        "crate::meerkat_machine::dsl::MeerkatMachineTransition".into(),
                    ),
                    error_type_path: Some(
                        "crate::meerkat_machine::dsl::MeerkatMachineTransitionError".into(),
                    ),
                    executor_trigger_input_variant: None,
                    bridge_source_type_path: Some(
                        "crate::meerkat_machine::dsl::MeerkatMachineEffect".into(),
                    ),
                    helper_return_shape: ProtocolHelperReturnShape::Obligations,
                    handle_trait_path: None,
                    handle_feedback_bindings: vec![],
                    input_payload_module_path: None,
                    additional_modes: vec![],
                },
            },
            EffectHandoffProtocol {
                name: protocol_id("supervisor_trust_revoke"),
                producer_instance: mi_id("meerkat"),
                effect_variant: ev_id("RevokeSupervisorTrustEdge"),
                realizing_actor: act_id("supervisor_bridge_owner"),
                correlation_fields: vec![fld_id("peer_id"), fld_id("epoch")],
                obligation_fields: vec![
                    fld_id("local_endpoint"),
                    fld_id("peer_id"),
                    fld_id("epoch"),
                ],
                allowed_feedback_inputs: vec![],
                closure_policy: ClosurePolicy::PublicationOnly,
                liveness_annotation: Some(
                    "generated supervisor trust authority publication is consumed under comms transport liveness"
                        .into(),
                ),
                comms_trust_authority: Some(CommsTrustAuthorityProtocol {
                    source_kind: CommsTrustAuthoritySourceKind::MeerkatMachineSupervisorRevoke,
                    row_owner_kind: Some(
                        CommsTrustAuthoritySourceKind::MeerkatMachineSupervisorPublish,
                    ),
                    allowed_operations: vec![CommsTrustAuthorityOperation::PrivateRemove],
                }),
                durable_marker: None,
                teardown: None,
                rust: ProtocolRustBinding {
                    module_path:
                        "meerkat-runtime/src/generated/protocol_supervisor_trust_revoke.rs".into(),
                    generation_mode: ProtocolGenerationMode::EffectExtractor,
                    required_imports: vec![
                        "use crate::meerkat_machine::dsl::{MeerkatMachineEffect, MeerkatMachineTransition, PeerEndpoint};".into(),
                    ],
                    authority_type_path: Some(
                        "crate::meerkat_machine::dsl::MeerkatMachineAuthority".into(),
                    ),
                    mutator_trait_path: Some(
                        "crate::meerkat_machine::dsl::MeerkatMachineMutator".into(),
                    ),
                    input_enum_path: Some(
                        "crate::meerkat_machine::dsl::MeerkatMachineInput".into(),
                    ),
                    effect_enum_path: Some("crate::meerkat_machine::dsl::MeerkatMachineEffect".into()),
                    transition_type_path: Some(
                        "crate::meerkat_machine::dsl::MeerkatMachineTransition".into(),
                    ),
                    error_type_path: Some(
                        "crate::meerkat_machine::dsl::MeerkatMachineTransitionError".into(),
                    ),
                    executor_trigger_input_variant: None,
                    bridge_source_type_path: Some(
                        "crate::meerkat_machine::dsl::MeerkatMachineEffect".into(),
                    ),
                    helper_return_shape: ProtocolHelperReturnShape::Obligations,
                    handle_trait_path: None,
                    handle_feedback_bindings: vec![],
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
                    producer_instance: mi_id("meerkat"),
                    effect_variant: ev_id("PublishSupervisorTrustEdge"),
                    protocol_name: protocol_id("supervisor_trust_publish"),
                },
                statement: "supervisor trust-edge publication crosses from the supervisor-binding authority into runtime trust mutation only through the explicit `supervisor_trust_publish` protocol and its generated trust authority".into(),
                references_machines: vec![mi_id("meerkat")],
                references_actors: vec![act_id("meerkat_kernel"), act_id("supervisor_bridge_owner")],
            },
            CompositionInvariant {
                name: "supervisor_trust_revoke_protocol_covered".into(),
                kind: CompositionInvariantKind::HandoffProtocolCovered {
                    producer_instance: mi_id("meerkat"),
                    effect_variant: ev_id("RevokeSupervisorTrustEdge"),
                    protocol_name: protocol_id("supervisor_trust_revoke"),
                },
                statement: "supervisor trust-edge revocation crosses from the supervisor-binding authority into runtime trust mutation only through the explicit `supervisor_trust_revoke` protocol and its generated trust authority".into(),
                references_machines: vec![mi_id("meerkat")],
                references_actors: vec![act_id("meerkat_kernel"), act_id("supervisor_bridge_owner")],
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
/// * producer: canonical `MobMachine` effect
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
            instance_id: mi_id("mob"),
            machine_name: mach_id("MobMachine"),
            actor: act_id("mob_kernel"),
        }],
        actors: vec![machine_actor("mob_kernel"), owner_actor("mob_destroy_session_ingress_owner")],
        handoff_protocols: vec![EffectHandoffProtocol {
            name: protocol_id("mob_destroying_session_ingress"),
            producer_instance: mi_id("mob"),
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
                    machine_instance: mi_id("mob"),
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
                    machine_instance: mi_id("mob"),
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
            comms_trust_authority: None,
            durable_marker: None,
            // C-F3: this protocol IS the detach-before-destroy obligation the
            // `destroy_request_reaches_meerkat` route names as its paired
            // `detach_obligation` — declared typed, not inferred from names.
            teardown: Some(TeardownObligationClass::DetachBeforeDestroy),
            rust: ProtocolRustBinding {
                module_path:
                    "meerkat-mob/src/generated/protocol_mob_destroying_session_ingress.rs"
                        .into(),
                generation_mode: ProtocolGenerationMode::EffectExtractor,
                required_imports: vec![
                    "use crate::machines::mob_machine::{AgentRuntimeId, MobId, MobMachineAuthority, MobMachineEffect, MobMachineInput, MobMachineMutator, MobMachineTransition, MobMachineTransitionError};".into(),
                ],
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
                    "crate::machines::mob_machine::dsl::MobMachineEffect".into(),
                ),
                transition_type_path: Some(
                    "crate::machines::mob_machine::dsl::MobMachineTransition".into(),
                ),
                error_type_path: Some(
                    "crate::machines::mob_machine::dsl::MobMachineTransitionError".into(),
                ),
                executor_trigger_input_variant: None,
                bridge_source_type_path: None,
                helper_return_shape: ProtocolHelperReturnShape::Obligations,
                handle_trait_path: None,
                handle_feedback_bindings: vec![],
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
                producer_instance: mi_id("mob"),
                effect_variant: ev_id("RequestSessionIngressDetachForMobDestroy"),
                protocol_name: protocol_id("mob_destroying_session_ingress"),
            },
            statement: "mob destroying its runtime crosses from the mob authority back into meerkat acknowledgement only through the explicit `mob_destroying_session_ingress` protocol: DetachIngress is fired against the target session and acknowledged before RequestRuntimeDestroy is routed".into(),
            references_machines: vec![mi_id("mob")],
            references_actors: vec![act_id("mob_kernel"), act_id("mob_destroy_session_ingress_owner")],
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
/// The canonical `AuthMachine` now hosts the
/// `auth_lease_lifecycle_publication` handoff annotation directly on
/// `EmitLifecycleEvent`; no bridge machine or mirrored route is
/// allowed to stand between the lifecycle owner and the runtime
/// auth-lease owner.
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
/// - `auth_lease_owner` (Owner) — realising actor; corresponds to
///   `meerkat-runtime::handles::auth_lease::RuntimeAuthLeaseHandle`.
pub fn auth_lease_bundle_composition() -> CompositionSchema {
    CompositionSchema {
        name: comp_id("auth_lease_bundle"),
        machines: vec![MachineInstance {
            instance_id: mi_id("auth_machine"),
            machine_name: mach_id("AuthMachine"),
            actor: act_id("auth_machine_authority"),
        }],
        actors: vec![
            machine_actor("auth_machine_authority"),
            owner_actor("auth_lease_owner"),
        ],
        handoff_protocols: vec![EffectHandoffProtocol {
            // 0.7.2 D1 drain obligation: release teardown must terminally
            // cancel in-flight OAuth flows before `Release` commits (the
            // machine-side completion witness is Release's
            // `oauth_release_drained` guard). Same-composition owner drain —
            // not a cross-machine DestroyRequest route — so no C-F3
            // `teardown` class (a DetachBeforeDestroy declaration with no
            // pairing route is a dangling-teardown audit error).
            name: protocol_id("auth_release_oauth_flow_drain"),
            producer_instance: mi_id("auth_machine"),
            effect_variant: ev_id("CancelOAuthFlowsForRelease"),
            realizing_actor: act_id("auth_lease_owner"),
            correlation_fields: vec![],
            obligation_fields: vec![
                fld_id("browser_flow_ids"),
                fld_id("device_flow_ids"),
            ],
            allowed_feedback_inputs: vec![
                FeedbackInputRef {
                    machine_instance: mi_id("auth_machine"),
                    input_variant: iv_id("ExpireOAuthBrowserFlow"),
                    field_bindings: vec![FeedbackFieldBinding {
                        input_field: fld_id("flow_id"),
                        // Distinct owner-context labels per feedback input: the
                        // codegen derives the TLA bound-variable name from this
                        // string, so two `flow_id` labels in the same handoff
                        // collide as duplicate `\E owner_ctx_flow_id` binders.
                        source: FeedbackFieldSource::OwnerContext("browser_flow_id".into()),
                    }],
                },
                FeedbackInputRef {
                    machine_instance: mi_id("auth_machine"),
                    input_variant: iv_id("ExpireOAuthDeviceFlow"),
                    field_bindings: vec![FeedbackFieldBinding {
                        input_field: fld_id("flow_id"),
                        source: FeedbackFieldSource::OwnerContext("device_flow_id".into()),
                    }],
                },
            ],
            closure_policy: ClosurePolicy::AckRequired,
            liveness_annotation: Some(
                "eventual feedback: release_lease discharges the drain by firing \
                 a terminal Expire* feedback per drained flow id before committing \
                 `Release`; the Release guard `oauth_release_drained` is the \
                 machine-side completion witness"
                    .into(),
            ),
            comms_trust_authority: None,
            durable_marker: None,
            teardown: None,
            rust: ProtocolRustBinding {
                module_path:
                    "meerkat-runtime/src/generated/protocol_auth_release_oauth_flow_drain.rs"
                        .into(),
                generation_mode: ProtocolGenerationMode::EffectExtractor,
                required_imports: vec![
                    "use crate::auth_machine::dsl::{AuthMachineAuthority, AuthMachineEffect, AuthMachineInput, AuthMachineMutator, AuthMachineTransition, AuthMachineTransitionError};".into(),
                ],
                authority_type_path: Some("crate::auth_machine::dsl::AuthMachineAuthority".into()),
                mutator_trait_path: Some("crate::auth_machine::dsl::AuthMachineMutator".into()),
                input_enum_path: Some("crate::auth_machine::dsl::AuthMachineInput".into()),
                effect_enum_path: Some("crate::auth_machine::dsl::AuthMachineEffect".into()),
                transition_type_path: Some("crate::auth_machine::dsl::AuthMachineTransition".into()),
                error_type_path: Some(
                    "crate::auth_machine::dsl::AuthMachineTransitionError".into(),
                ),
                executor_trigger_input_variant: None,
                bridge_source_type_path: None,
                helper_return_shape: ProtocolHelperReturnShape::Obligations,
                handle_trait_path: None,
                handle_feedback_bindings: vec![],
                input_payload_module_path: None,
                additional_modes: vec![],
            },
        },
        EffectHandoffProtocol {
            name: protocol_id("auth_lease_lifecycle_publication"),
            producer_instance: mi_id("auth_machine"),
            effect_variant: ev_id("EmitLifecycleEvent"),
            realizing_actor: act_id("auth_lease_owner"),
            correlation_fields: vec![fld_id("new_state")],
            obligation_fields: vec![
                fld_id("new_state"),
                fld_id("expires_at"),
                fld_id("credential_generation"),
                fld_id("credential_published_at_millis"),
            ],
            allowed_feedback_inputs: vec![],
            closure_policy: ClosurePolicy::PublicationOnly,
            liveness_annotation: Some(
                "informative publication: AuthMachine's own transitions carry the \
                 authoritative phase fact; runtime owner refreshes the lease-state \
                 projection under task-scheduling fairness"
                    .into(),
            ),
            comms_trust_authority: None,
            durable_marker: Some(DurableMarkerProtocol {
                metadata_key: "meerkat_auth_lifecycle".into(),
                previous_metadata_key: "meerkat_previous_metadata".into(),
                published_field: "published".into(),
                version_field: "version".into(),
                authority_field: "authority".into(),
                protocol_field: "protocol".into(),
                realm_field: "realm".into(),
                binding_field: "binding".into(),
                profile_field: "profile".into(),
                schema_version: 4,
                phase: DurableMarkerFieldBinding {
                    marker_field: "phase".into(),
                    obligation_field: fld_id("new_state"),
                },
                expires_at: DurableMarkerFieldBinding {
                    marker_field: "expires_at".into(),
                    obligation_field: fld_id("expires_at"),
                },
                generation: DurableMarkerFieldBinding {
                    marker_field: "generation".into(),
                    obligation_field: fld_id("credential_generation"),
                },
                credential_published_at_millis: DurableMarkerFieldBinding {
                    marker_field: "credential_published_at_millis".into(),
                    obligation_field: fld_id("credential_published_at_millis"),
                },
                relation: DurableMarkerRelationProtocol::AuthLeaseCredentialPublication,
            }),
            teardown: None,
            rust: ProtocolRustBinding {
                module_path:
                    "meerkat-runtime/src/generated/protocol_auth_lease_lifecycle_publication.rs"
                        .into(),
                generation_mode: ProtocolGenerationMode::EffectExtractor,
                required_imports: vec![
                    "use crate::auth_machine::dsl::{AuthLifecyclePhase, AuthMachineEffect, AuthMachineTransition};".into(),
                ],
                authority_type_path: None,
                mutator_trait_path: None,
                input_enum_path: None,
                effect_enum_path: Some("crate::auth_machine::dsl::AuthMachineEffect".into()),
                transition_type_path: Some("crate::auth_machine::dsl::AuthMachineTransition".into()),
                error_type_path: None,
                executor_trigger_input_variant: None,
                bridge_source_type_path: None,
                helper_return_shape: ProtocolHelperReturnShape::Obligations,
                handle_trait_path: None,
                handle_feedback_bindings: vec![],
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
            name: "auth_release_oauth_flow_drain_protocol_covered".into(),
            kind: CompositionInvariantKind::HandoffProtocolCovered {
                producer_instance: mi_id("auth_machine"),
                effect_variant: ev_id("CancelOAuthFlowsForRelease"),
                protocol_name: protocol_id("auth_release_oauth_flow_drain"),
            },
            statement: "every release-drain obligation AuthMachine mints for \
                    in-flight OAuth flows crosses to the runtime auth-lease owner \
                    through the explicit `auth_release_oauth_flow_drain` protocol \
                    and is closed by terminal Expire* feedback before `Release` \
                    commits, rather than by ad-hoc shell reads of flow membership"
                .into(),
            references_machines: vec![mi_id("auth_machine")],
            references_actors: vec![act_id("auth_machine_authority"), act_id("auth_lease_owner")],
        },
        CompositionInvariant {
            name: "auth_lease_lifecycle_publication_protocol_covered".into(),
            kind: CompositionInvariantKind::HandoffProtocolCovered {
                producer_instance: mi_id("auth_machine"),
                effect_variant: ev_id("EmitLifecycleEvent"),
                protocol_name: protocol_id("auth_lease_lifecycle_publication"),
            },
            statement: "every AuthMachine lifecycle-phase transition's external \
                    publication crosses into the runtime auth-lease owner through the \
                    explicit `auth_lease_lifecycle_publication` protocol rather than \
                    ad-hoc shell observation"
                .into(),
            references_machines: vec![mi_id("auth_machine")],
            references_actors: vec![act_id("auth_machine_authority"), act_id("auth_lease_owner")],
        }],
        witnesses: vec![
            witness("auth_lease_lifecycle_publication_round_trip", &[]),
            witness("auth_release_oauth_flow_drain_round_trip", &[]),
        ],
        deep_domain_cardinality: 2,
        deep_domain_overrides: std::collections::BTreeMap::new(),
        witness_domain_cardinality: 2,
        ci_limits: Some(default_ci_limits()),
        closed_world: true,
    }
}
