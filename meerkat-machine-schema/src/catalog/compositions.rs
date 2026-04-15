use crate::{
    ActorKind, ActorSchema, CompositionInvariant, CompositionInvariantKind, CompositionSchema,
    CompositionStateLimits, CompositionTransactionPlan, CompositionWitness, EntryInput,
    MachineInstance, Route, RouteBindingSource, RouteDelivery, RouteFieldBinding, RouteTarget,
    RouteTargetKind,
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
        driver: None,
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
