use std::collections::BTreeMap;

use crate::{
    ActorKind, ActorSchema, ClosurePolicy, CompositionDriver, CompositionDriverRustBinding,
    CompositionInvariant, CompositionInvariantKind, CompositionSchema, CompositionStateLimits,
    CompositionTransactionPlan, CompositionWitness, DriverDispatchRoute, EffectHandoffProtocol,
    EntryInput, FeedbackFieldBinding, FeedbackFieldSource, FeedbackInputRef, MachineInstance,
    ProtocolGenerationMode, ProtocolHelperReturnShape, ProtocolRustBinding, Route,
    RouteBindingSource, RouteDelivery, RouteFieldBinding, RouteTarget, RouteTargetKind,
    WatchedEffect,
};

pub fn schedule_bundle_composition() -> CompositionSchema {
    CompositionSchema {
        name: "schedule_bundle".into(),
        machines: vec![
            MachineInstance {
                instance_id: "schedule".into(),
                machine_name: "ScheduleLifecycleMachine".into(),
                actor: "schedule_authority".into(),
            },
            MachineInstance {
                instance_id: "occurrence".into(),
                machine_name: "OccurrenceLifecycleMachine".into(),
                actor: "occurrence_authority".into(),
            },
        ],
        actors: vec![
            machine_actor("schedule_authority"),
            machine_actor("occurrence_authority"),
        ],
        handoff_protocols: vec![],
        entry_inputs: vec![],
        routes: vec![route(
            "revision_supersede_enters_occurrence_authority",
            "schedule",
            "SupersedePendingOccurrences",
            "occurrence",
            RouteTargetKind::Input,
            "Supersede",
            &[bind("superseded_by_revision", "superseding_revision")],
        )],
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
                    from_machine: "schedule".into(),
                    effect_variant: "SupersedePendingOccurrences".into(),
                    to_machine: "occurrence".into(),
                    input_variant: "Supersede".into(),
                },
                statement: "revision-affecting schedule edits enter occurrence authority through the explicit supersede route".into(),
                references_machines: vec!["schedule".into(), "occurrence".into()],
                references_actors: vec!["schedule_authority".into(), "occurrence_authority".into()],
            },
            CompositionInvariant {
                name: "superseded_occurrence_originates_from_schedule_revision".into(),
                kind: CompositionInvariantKind::ObservedRouteInputOriginatesFromEffect {
                    route_name: "revision_supersede_enters_occurrence_authority".into(),
                    to_machine: "occurrence".into(),
                    input_variant: "Supersede".into(),
                    from_machine: "schedule".into(),
                    effect_variant: "SupersedePendingOccurrences".into(),
                },
                statement: "pending future occurrences are superseded only by the schedule revision route rather than by ad hoc shell mutation".into(),
                references_machines: vec!["schedule".into(), "occurrence".into()],
                references_actors: vec!["schedule_authority".into(), "occurrence_authority".into()],
            },
        ],
        witnesses: vec![
            witness(
                "revision_supersede_route",
                &["revision_supersede_enters_occurrence_authority"],
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
        name: "schedule_runtime_bundle".into(),
        machines: vec![MachineInstance {
            instance_id: "occurrence".into(),
            machine_name: "OccurrenceLifecycleMachine".into(),
            actor: "occurrence_authority".into(),
        }],
        actors: vec![machine_actor("occurrence_authority")],
        handoff_protocols: vec![],
        entry_inputs: vec![],
        routes: vec![],
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
        name: "schedule_mob_bundle".into(),
        machines: vec![MachineInstance {
            instance_id: "occurrence".into(),
            machine_name: "OccurrenceLifecycleMachine".into(),
            actor: "occurrence_authority".into(),
        }],
        actors: vec![machine_actor("occurrence_authority")],
        handoff_protocols: vec![],
        entry_inputs: vec![],
        routes: vec![],
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
        name: "meerkat_mob_seam".into(),
        machines: vec![
            MachineInstance {
                instance_id: "meerkat".into(),
                machine_name: "MeerkatMachine".into(),
                actor: "meerkat_kernel".into(),
            },
            MachineInstance {
                instance_id: "mob".into(),
                machine_name: "MobMachine".into(),
                actor: "mob_kernel".into(),
            },
        ],
        actors: vec![machine_actor("meerkat_kernel"), machine_actor("mob_kernel")],
        handoff_protocols: vec![],
        entry_inputs: vec![
            EntryInput {
                name: "spawn_member".into(),
                machine: "mob".into(),
                input_variant: "Spawn".into(),
            },
            EntryInput {
                name: "submit_work".into(),
                machine: "mob".into(),
                input_variant: "SubmitWork".into(),
            },
            EntryInput {
                name: "retire_member".into(),
                machine: "mob".into(),
                input_variant: "Retire".into(),
            },
            EntryInput {
                name: "destroy_mob".into(),
                machine: "mob".into(),
                input_variant: "Destroy".into(),
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
                &[],
            ),
            route(
                "destroy_request_reaches_meerkat",
                "mob",
                "RequestRuntimeDestroy",
                "meerkat",
                RouteTargetKind::Input,
                "Destroy",
                &[],
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
        driver: Some(CompositionDriver {
            // Track-B (R5) Commit 4: the first real consumer of the
            // composition-driver execution framework (Commit 1). The
            // driver watches MobMachine's identity-level wiring +
            // session-binding effects plus MeerkatMachine's local-
            // endpoint changes, and dispatches `ApplyMobPeerOverlay`
            // to each bound session so the identity-level wiring
            // graph is projected onto endpoint-level peer sets.
            name: "RecomputeMobPeerOverlay".into(),
            rust: CompositionDriverRustBinding {
                module_path: "meerkat-runtime/src/generated/recompute_mob_peer_overlay_driver.rs"
                    .into(),
                driver_type: "RecomputeMobPeerOverlayDriver".into(),
                store_plan_type: "RecomputeMobPeerOverlayStorePlan".into(),
                work_type: "RecomputeMobPeerOverlayWork".into(),
                decision_type: "RecomputeMobPeerOverlayDecision".into(),
                required_imports: vec!["use meerkat_runtime::composition_dispatch::*;".into()],
            },
            watched_effects: vec![
                WatchedEffect {
                    producer_instance: "mob".into(),
                    effect_variant: "WiringGraphChanged".into(),
                },
                WatchedEffect {
                    producer_instance: "mob".into(),
                    effect_variant: "MemberSessionBindingChanged".into(),
                },
                WatchedEffect {
                    producer_instance: "meerkat".into(),
                    effect_variant: "LocalEndpointChanged".into(),
                },
            ],
            dispatch_routes: vec![DriverDispatchRoute {
                name: "apply_mob_peer_overlay".into(),
                target_instance: "meerkat".into(),
                target_kind: RouteTargetKind::Input,
                input_variant: "ApplyMobPeerOverlay".into(),
            }],
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
        name: name.into(),
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
        name: name.into(),
        from_machine: from_machine.into(),
        effect_variant: effect_variant.into(),
        to: RouteTarget {
            machine: to_machine.into(),
            kind: target_kind,
            input_variant: input_variant.into(),
        },
        bindings: bindings.to_vec(),
        delivery: RouteDelivery::Immediate,
    }
}

fn bind(to_field: &str, from_field: &str) -> RouteFieldBinding {
    RouteFieldBinding {
        to_field: to_field.into(),
        source: RouteBindingSource::Field {
            from_field: from_field.into(),
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
        expected_routes: expected_routes
            .iter()
            .map(|route| (*route).into())
            .collect(),
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
        name: "flow_frame_loop".into(),
        machines: vec![MachineInstance {
            instance_id: "loop_iteration".into(),
            machine_name: "LoopIterationMachine".into(),
            actor: "loop_iteration_authority".into(),
        }],
        actors: vec![
            machine_actor("loop_iteration_authority"),
            owner_actor("loop_runtime_owner"),
        ],
        handoff_protocols: vec![EffectHandoffProtocol {
            name: "flow_loop_until_evaluation".into(),
            producer_instance: "loop_iteration".into(),
            effect_variant: "EvaluateUntilCondition".into(),
            realizing_actor: "loop_runtime_owner".into(),
            correlation_fields: vec!["loop_instance_id".into(), "iteration".into()],
            obligation_fields: vec![
                "loop_instance_id".into(),
                "iteration".into(),
                "parent_frame_id".into(),
                "parent_node_id".into(),
                "loop_id".into(),
            ],
            allowed_feedback_inputs: vec![
                FeedbackInputRef {
                    machine_instance: "loop_iteration".into(),
                    input_variant: "UntilConditionMet".into(),
                    field_bindings: vec![
                        FeedbackFieldBinding {
                            input_field: "loop_instance_id".into(),
                            source: FeedbackFieldSource::ObligationField(
                                "loop_instance_id".into(),
                            ),
                        },
                        FeedbackFieldBinding {
                            input_field: "iteration".into(),
                            source: FeedbackFieldSource::ObligationField("iteration".into()),
                        },
                    ],
                },
                FeedbackInputRef {
                    machine_instance: "loop_iteration".into(),
                    input_variant: "UntilConditionFailed".into(),
                    field_bindings: vec![
                        FeedbackFieldBinding {
                            input_field: "loop_instance_id".into(),
                            source: FeedbackFieldSource::ObligationField(
                                "loop_instance_id".into(),
                            ),
                        },
                        FeedbackFieldBinding {
                            input_field: "iteration".into(),
                            source: FeedbackFieldSource::ObligationField("iteration".into()),
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
                producer_instance: "loop_iteration".into(),
                effect_variant: "EvaluateUntilCondition".into(),
                protocol_name: "flow_loop_until_evaluation".into(),
            },
            statement: "loop-iteration authority's UntilCondition evaluation effect is handed off through the explicit `flow_loop_until_evaluation` protocol rather than ad-hoc shell mutation".into(),
            references_machines: vec!["loop_iteration".into()],
            references_actors: vec![
                "loop_iteration_authority".into(),
                "loop_runtime_owner".into(),
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
        name: "mob_bundle".into(),
        // The producer is the compat `OpsBarrierBridgeMachine` which hosts
        // the handoff-annotated `WaitAllSatisfied` effect declaration.
        // Its shape mirrors the runtime-owned effect; the canonical
        // `MeerkatMachine` also declares `WaitAllSatisfied` (without the
        // handoff annotation the DSL macro cannot emit) so the runtime
        // shell still observes the effect through its own reducer.
        machines: vec![MachineInstance {
            instance_id: "ops_barrier_bridge".into(),
            machine_name: "OpsBarrierBridgeMachine".into(),
            actor: "ops_barrier_bridge_authority".into(),
        }],
        actors: vec![
            machine_actor("ops_barrier_bridge_authority"),
            owner_actor("ops_lifecycle_owner"),
        ],
        handoff_protocols: vec![EffectHandoffProtocol {
            name: "ops_barrier_satisfaction".into(),
            producer_instance: "ops_barrier_bridge".into(),
            effect_variant: "WaitAllSatisfied".into(),
            realizing_actor: "ops_lifecycle_owner".into(),
            correlation_fields: vec!["wait_request_id".into()],
            obligation_fields: vec!["wait_request_id".into(), "operation_ids".into()],
            allowed_feedback_inputs: vec![FeedbackInputRef {
                machine_instance: "ops_barrier_bridge".into(),
                input_variant: "OpsBarrierSatisfied".into(),
                field_bindings: vec![
                    FeedbackFieldBinding {
                        input_field: "wait_request_id".into(),
                        source: FeedbackFieldSource::ObligationField(
                            "wait_request_id".into(),
                        ),
                    },
                    FeedbackFieldBinding {
                        input_field: "operation_ids".into(),
                        source: FeedbackFieldSource::ObligationField("operation_ids".into()),
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
                producer_instance: "ops_barrier_bridge".into(),
                effect_variant: "WaitAllSatisfied".into(),
                protocol_name: "ops_barrier_satisfaction".into(),
            },
            statement: "wait-all barrier satisfaction crosses from the ops lifecycle owner back into turn-state authority only through the explicit `ops_barrier_satisfaction` protocol".into(),
            references_machines: vec!["ops_barrier_bridge".into()],
            references_actors: vec![
                "ops_barrier_bridge_authority".into(),
                "ops_lifecycle_owner".into(),
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
        name: "external_tool_bundle".into(),
        machines: vec![MachineInstance {
            instance_id: "external_tool_surface".into(),
            machine_name: "ExternalToolSurfaceBridgeMachine".into(),
            actor: "external_tool_surface_authority".into(),
        }],
        actors: vec![
            machine_actor("external_tool_surface_authority"),
            owner_actor("surface_host_owner"),
        ],
        handoff_protocols: vec![
            // Protocol 1: surface_completion — dual-mode emission
            // (EffectExtractor + HandleBridge).
            EffectHandoffProtocol {
                name: "surface_completion".into(),
                producer_instance: "external_tool_surface".into(),
                effect_variant: "ScheduleSurfaceCompletion".into(),
                realizing_actor: "surface_host_owner".into(),
                correlation_fields: vec![
                    "surface_id".into(),
                    "pending_task_sequence".into(),
                ],
                obligation_fields: vec![
                    "surface_id".into(),
                    "operation".into(),
                    "pending_task_sequence".into(),
                    "staged_intent_sequence".into(),
                    "applied_at_turn".into(),
                ],
                allowed_feedback_inputs: vec![
                    FeedbackInputRef {
                        machine_instance: "external_tool_surface".into(),
                        input_variant: "PendingSucceeded".into(),
                        field_bindings: vec![
                            FeedbackFieldBinding {
                                input_field: "surface_id".into(),
                                source: FeedbackFieldSource::ObligationField(
                                    "surface_id".into(),
                                ),
                            },
                            FeedbackFieldBinding {
                                input_field: "pending_task_sequence".into(),
                                source: FeedbackFieldSource::ObligationField(
                                    "pending_task_sequence".into(),
                                ),
                            },
                            FeedbackFieldBinding {
                                input_field: "staged_intent_sequence".into(),
                                source: FeedbackFieldSource::ObligationField(
                                    "staged_intent_sequence".into(),
                                ),
                            },
                        ],
                    },
                    FeedbackInputRef {
                        machine_instance: "external_tool_surface".into(),
                        input_variant: "PendingFailed".into(),
                        field_bindings: vec![
                            FeedbackFieldBinding {
                                input_field: "surface_id".into(),
                                source: FeedbackFieldSource::ObligationField(
                                    "surface_id".into(),
                                ),
                            },
                            FeedbackFieldBinding {
                                input_field: "pending_task_sequence".into(),
                                source: FeedbackFieldSource::ObligationField(
                                    "pending_task_sequence".into(),
                                ),
                            },
                            FeedbackFieldBinding {
                                input_field: "reason".into(),
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
                name: "surface_snapshot_alignment".into(),
                producer_instance: "external_tool_surface".into(),
                effect_variant: "RefreshVisibleSurfaceSet".into(),
                realizing_actor: "surface_host_owner".into(),
                correlation_fields: vec!["snapshot_epoch".into()],
                obligation_fields: vec!["snapshot_epoch".into()],
                allowed_feedback_inputs: vec![FeedbackInputRef {
                    machine_instance: "external_tool_surface".into(),
                    input_variant: "SnapshotAligned".into(),
                    field_bindings: vec![FeedbackFieldBinding {
                        input_field: "snapshot_epoch".into(),
                        source: FeedbackFieldSource::ObligationField("snapshot_epoch".into()),
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
                    producer_instance: "external_tool_surface".into(),
                    effect_variant: "ScheduleSurfaceCompletion".into(),
                    protocol_name: "surface_completion".into(),
                },
                statement:
                    "pending-op completion on a tool surface is returned to the authority only through the explicit `surface_completion` protocol"
                        .into(),
                references_machines: vec!["external_tool_surface".into()],
                references_actors: vec![
                    "external_tool_surface_authority".into(),
                    "surface_host_owner".into(),
                ],
            },
            CompositionInvariant {
                name: "surface_snapshot_alignment_protocol_covered".into(),
                kind: CompositionInvariantKind::HandoffProtocolCovered {
                    producer_instance: "external_tool_surface".into(),
                    effect_variant: "RefreshVisibleSurfaceSet".into(),
                    protocol_name: "surface_snapshot_alignment".into(),
                },
                statement:
                    "visible-set refresh acknowledgement crosses back through the explicit `surface_snapshot_alignment` protocol rather than ad-hoc polling"
                        .into(),
                references_machines: vec!["external_tool_surface".into()],
                references_actors: vec![
                    "external_tool_surface_authority".into(),
                    "surface_host_owner".into(),
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
        name: name.into(),
        kind: ActorKind::Owner,
    }
}
