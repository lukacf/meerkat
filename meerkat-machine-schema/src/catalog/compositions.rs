use crate::{
    ActorPriority, CompositionInvariant, CompositionInvariantKind, CompositionSchema,
    CompositionStateLimits, CompositionWitness, CompositionWitnessField, CompositionWitnessInput,
    CompositionWitnessState, CompositionWitnessTransition, CompositionWitnessTransitionOrder,
    EntryInput, Expr, MachineInstance, Route, RouteBindingSource, RouteDelivery, RouteFieldBinding,
    RouteTarget, SchedulerRule,
};
use std::collections::BTreeMap;

pub fn runtime_pipeline_composition() -> CompositionSchema {
    CompositionSchema {
        name: "runtime_pipeline".into(),
        machines: vec![
            MachineInstance {
                instance_id: "runtime_control".into(),
                machine_name: "RuntimeControlMachine".into(),
                actor: "control_plane".into(),
            },
            MachineInstance {
                instance_id: "runtime_ingress".into(),
                machine_name: "RuntimeIngressMachine".into(),
                actor: "ordinary_ingress".into(),
            },
            MachineInstance {
                instance_id: "turn_execution".into(),
                machine_name: "TurnExecutionMachine".into(),
                actor: "turn_executor".into(),
            },
        ],
        entry_inputs: vec![
            EntryInput {
                name: "control_initialize".into(),
                machine: "runtime_control".into(),
                input_variant: "Initialize".into(),
            },
            EntryInput {
                name: "runtime_admission_accepted".into(),
                machine: "runtime_control".into(),
                input_variant: "AdmissionAccepted".into(),
            },
            EntryInput {
                name: "admit_queued_work".into(),
                machine: "runtime_ingress".into(),
                input_variant: "AdmitQueued".into(),
            },
            EntryInput {
                name: "admit_consumed_on_accept".into(),
                machine: "runtime_ingress".into(),
                input_variant: "AdmitConsumedOnAccept".into(),
            },
            EntryInput {
                name: "stage_drain_snapshot".into(),
                machine: "runtime_ingress".into(),
                input_variant: "StageDrainSnapshot".into(),
            },
            EntryInput {
                name: "control_recover_requested".into(),
                machine: "runtime_control".into(),
                input_variant: "RecoverRequested".into(),
            },
            EntryInput {
                name: "control_retire_requested".into(),
                machine: "runtime_control".into(),
                input_variant: "RetireRequested".into(),
            },
            EntryInput {
                name: "control_reset_requested".into(),
                machine: "runtime_control".into(),
                input_variant: "ResetRequested".into(),
            },
            EntryInput {
                name: "control_stop_requested".into(),
                machine: "runtime_control".into(),
                input_variant: "StopRequested".into(),
            },
            EntryInput {
                name: "control_destroy_requested".into(),
                machine: "runtime_control".into(),
                input_variant: "DestroyRequested".into(),
            },
            EntryInput {
                name: "primitive_applied".into(),
                machine: "turn_execution".into(),
                input_variant: "PrimitiveApplied".into(),
            },
            EntryInput {
                name: "llm_returned_tool_calls".into(),
                machine: "turn_execution".into(),
                input_variant: "LlmReturnedToolCalls".into(),
            },
            EntryInput {
                name: "llm_returned_terminal".into(),
                machine: "turn_execution".into(),
                input_variant: "LlmReturnedTerminal".into(),
            },
            EntryInput {
                name: "tool_calls_resolved".into(),
                machine: "turn_execution".into(),
                input_variant: "ToolCallsResolved".into(),
            },
            EntryInput {
                name: "boundary_continue".into(),
                machine: "turn_execution".into(),
                input_variant: "BoundaryContinue".into(),
            },
            EntryInput {
                name: "boundary_complete".into(),
                machine: "turn_execution".into(),
                input_variant: "BoundaryComplete".into(),
            },
            EntryInput {
                name: "recoverable_failure".into(),
                machine: "turn_execution".into(),
                input_variant: "RecoverableFailure".into(),
            },
            EntryInput {
                name: "fatal_failure".into(),
                machine: "turn_execution".into(),
                input_variant: "FatalFailure".into(),
            },
            EntryInput {
                name: "retry_requested".into(),
                machine: "turn_execution".into(),
                input_variant: "RetryRequested".into(),
            },
            EntryInput {
                name: "cancel_now".into(),
                machine: "turn_execution".into(),
                input_variant: "CancelNow".into(),
            },
            EntryInput {
                name: "cancel_after_boundary".into(),
                machine: "turn_execution".into(),
                input_variant: "CancelAfterBoundary".into(),
            },
            EntryInput {
                name: "cancellation_observed".into(),
                machine: "turn_execution".into(),
                input_variant: "CancellationObserved".into(),
            },
            EntryInput {
                name: "acknowledge_terminal".into(),
                machine: "turn_execution".into(),
                input_variant: "AcknowledgeTerminal".into(),
            },
        ],
        routes: vec![
            Route {
                name: "admitted_work_enters_ingress".into(),
                from_machine: "runtime_control".into(),
                effect_variant: "SubmitAdmittedIngressEffect".into(),
                to: RouteTarget {
                    machine: "runtime_ingress".into(),
                    input_variant: "AdmitQueued".into(),
                },
                bindings: vec![
                    RouteFieldBinding {
                        to_field: "work_id".into(),
                        source: RouteBindingSource::Field {
                            from_field: "work_id".into(),
                            allow_named_alias: true,
                        },
                    },
                    RouteFieldBinding {
                        to_field: "content_shape".into(),
                        source: RouteBindingSource::Field {
                            from_field: "content_shape".into(),
                            allow_named_alias: false,
                        },
                    },
                    RouteFieldBinding {
                        to_field: "request_id".into(),
                        source: RouteBindingSource::Field {
                            from_field: "request_id".into(),
                            allow_named_alias: false,
                        },
                    },
                    RouteFieldBinding {
                        to_field: "reservation_key".into(),
                        source: RouteBindingSource::Field {
                            from_field: "reservation_key".into(),
                            allow_named_alias: false,
                        },
                    },
                    RouteFieldBinding {
                        to_field: "handling_mode".into(),
                        source: RouteBindingSource::Field {
                            from_field: "handling_mode".into(),
                            allow_named_alias: false,
                        },
                    },
                    RouteFieldBinding {
                        to_field: "policy".into(),
                        source: RouteBindingSource::Literal(Expr::String("PromptAdmission".into())),
                    },
                ],
                delivery: RouteDelivery::Immediate,
            },
            Route {
                name: "staged_run_notifies_control".into(),
                from_machine: "runtime_ingress".into(),
                effect_variant: "ReadyForRun".into(),
                to: RouteTarget {
                    machine: "runtime_control".into(),
                    input_variant: "BeginRun".into(),
                },
                bindings: vec![RouteFieldBinding {
                    to_field: "run_id".into(),
                    source: RouteBindingSource::Field {
                        from_field: "run_id".into(),
                        allow_named_alias: false,
                    },
                }],
                delivery: RouteDelivery::Immediate,
            },
            Route {
                name: "control_starts_execution".into(),
                from_machine: "runtime_control".into(),
                effect_variant: "SubmitRunPrimitive".into(),
                to: RouteTarget {
                    machine: "turn_execution".into(),
                    input_variant: "StartConversationRun".into(),
                },
                bindings: vec![RouteFieldBinding {
                    to_field: "run_id".into(),
                    source: RouteBindingSource::Field {
                        from_field: "run_id".into(),
                        allow_named_alias: false,
                    },
                }],
                delivery: RouteDelivery::Immediate,
            },
            Route {
                name: "execution_boundary_updates_ingress".into(),
                from_machine: "turn_execution".into(),
                effect_variant: "BoundaryApplied".into(),
                to: RouteTarget {
                    machine: "runtime_ingress".into(),
                    input_variant: "BoundaryApplied".into(),
                },
                bindings: vec![
                    RouteFieldBinding {
                        to_field: "run_id".into(),
                        source: RouteBindingSource::Field {
                            from_field: "run_id".into(),
                            allow_named_alias: false,
                        },
                    },
                    RouteFieldBinding {
                        to_field: "boundary_sequence".into(),
                        source: RouteBindingSource::Field {
                            from_field: "boundary_sequence".into(),
                            allow_named_alias: false,
                        },
                    },
                ],
                delivery: RouteDelivery::Immediate,
            },
            Route {
                name: "execution_completion_updates_ingress".into(),
                from_machine: "turn_execution".into(),
                effect_variant: "RunCompleted".into(),
                to: RouteTarget {
                    machine: "runtime_ingress".into(),
                    input_variant: "RunCompleted".into(),
                },
                bindings: vec![RouteFieldBinding {
                    to_field: "run_id".into(),
                    source: RouteBindingSource::Field {
                        from_field: "run_id".into(),
                        allow_named_alias: false,
                    },
                }],
                delivery: RouteDelivery::Immediate,
            },
            Route {
                name: "execution_completion_notifies_control".into(),
                from_machine: "turn_execution".into(),
                effect_variant: "RunCompleted".into(),
                to: RouteTarget {
                    machine: "runtime_control".into(),
                    input_variant: "RunCompleted".into(),
                },
                bindings: vec![RouteFieldBinding {
                    to_field: "run_id".into(),
                    source: RouteBindingSource::Field {
                        from_field: "run_id".into(),
                        allow_named_alias: false,
                    },
                }],
                delivery: RouteDelivery::Immediate,
            },
            Route {
                name: "execution_failure_updates_ingress".into(),
                from_machine: "turn_execution".into(),
                effect_variant: "RunFailed".into(),
                to: RouteTarget {
                    machine: "runtime_ingress".into(),
                    input_variant: "RunFailed".into(),
                },
                bindings: vec![RouteFieldBinding {
                    to_field: "run_id".into(),
                    source: RouteBindingSource::Field {
                        from_field: "run_id".into(),
                        allow_named_alias: false,
                    },
                }],
                delivery: RouteDelivery::Immediate,
            },
            Route {
                name: "execution_failure_notifies_control".into(),
                from_machine: "turn_execution".into(),
                effect_variant: "RunFailed".into(),
                to: RouteTarget {
                    machine: "runtime_control".into(),
                    input_variant: "RunFailed".into(),
                },
                bindings: vec![RouteFieldBinding {
                    to_field: "run_id".into(),
                    source: RouteBindingSource::Field {
                        from_field: "run_id".into(),
                        allow_named_alias: false,
                    },
                }],
                delivery: RouteDelivery::Immediate,
            },
            Route {
                name: "execution_cancel_updates_ingress".into(),
                from_machine: "turn_execution".into(),
                effect_variant: "RunCancelled".into(),
                to: RouteTarget {
                    machine: "runtime_ingress".into(),
                    input_variant: "RunCancelled".into(),
                },
                bindings: vec![RouteFieldBinding {
                    to_field: "run_id".into(),
                    source: RouteBindingSource::Field {
                        from_field: "run_id".into(),
                        allow_named_alias: false,
                    },
                }],
                delivery: RouteDelivery::Immediate,
            },
            Route {
                name: "execution_cancel_notifies_control".into(),
                from_machine: "turn_execution".into(),
                effect_variant: "RunCancelled".into(),
                to: RouteTarget {
                    machine: "runtime_control".into(),
                    input_variant: "RunCancelled".into(),
                },
                bindings: vec![RouteFieldBinding {
                    to_field: "run_id".into(),
                    source: RouteBindingSource::Field {
                        from_field: "run_id".into(),
                        allow_named_alias: false,
                    },
                }],
                delivery: RouteDelivery::Immediate,
            },
        ],
        actor_priorities: vec![ActorPriority {
            higher: "control_plane".into(),
            lower: "ordinary_ingress".into(),
            reason: "control commands preempt ordinary admission".into(),
        }],
        scheduler_rules: vec![SchedulerRule::PreemptWhenReady {
            higher: "control_plane".into(),
            lower: "ordinary_ingress".into(),
        }],
        invariants: vec![
            CompositionInvariant {
                name: "admitted_work_reaches_ingress_through_runtime_control".into(),
                kind: CompositionInvariantKind::ObservedRouteInputOriginatesFromEffect {
                    route_name: "admitted_work_enters_ingress".into(),
                    to_machine: "runtime_ingress".into(),
                    input_variant: "AdmitQueued".into(),
                    from_machine: "runtime_control".into(),
                    effect_variant: "SubmitAdmittedIngressEffect".into(),
                },
                statement:
                    "runtime-admitted ordinary work reaches ingress only through the runtime-control admission handoff".into(),
                references_machines: vec!["runtime_control".into(), "runtime_ingress".into()],
                references_actors: vec!["control_plane".into(), "ordinary_ingress".into()],
            },
            CompositionInvariant {
                name: "control_preempts_ordinary_work".into(),
                kind: CompositionInvariantKind::SchedulerRulePresent {
                    rule: SchedulerRule::PreemptWhenReady {
                        higher: "control_plane".into(),
                        lower: "ordinary_ingress".into(),
                    },
                },
                statement: "control_plane outranks ordinary_ingress whenever both are ready".into(),
                references_machines: vec!["runtime_control".into(), "runtime_ingress".into()],
                references_actors: vec!["control_plane".into(), "ordinary_ingress".into()],
            },
            CompositionInvariant {
                name: "begin_run_requires_staged_drain".into(),
                kind: CompositionInvariantKind::ObservedInputOriginatesFromEffect {
                    to_machine: "runtime_control".into(),
                    input_variant: "BeginRun".into(),
                    from_machine: "runtime_ingress".into(),
                    effect_variant: "ReadyForRun".into(),
                },
                statement: "runtime control may begin a run only from ingress-owned staged work"
                    .into(),
                references_machines: vec!["runtime_control".into(), "runtime_ingress".into()],
                references_actors: vec!["control_plane".into(), "ordinary_ingress".into()],
            },
            CompositionInvariant {
                name: "begun_run_must_start_execution".into(),
                kind: CompositionInvariantKind::ObservedInputOriginatesFromEffect {
                    to_machine: "turn_execution".into(),
                    input_variant: "StartConversationRun".into(),
                    from_machine: "runtime_control".into(),
                    effect_variant: "SubmitRunPrimitive".into(),
                },
                statement:
                    "a begun run must flow into turn execution through the canonical primitive handoff"
                        .into(),
                references_machines: vec!["runtime_control".into(), "turn_execution".into()],
                references_actors: vec!["control_plane".into(), "turn_executor".into()],
            },
            CompositionInvariant {
                name: "execution_failure_is_handled".into(),
                kind: CompositionInvariantKind::OutcomeHandled {
                    from_machine: "turn_execution".into(),
                    effect_variant: "RunFailed".into(),
                    required_targets: vec![
                        RouteTarget {
                            machine: "runtime_ingress".into(),
                            input_variant: "RunFailed".into(),
                        },
                        RouteTarget {
                            machine: "runtime_control".into(),
                            input_variant: "RunFailed".into(),
                        },
                    ],
                },
                statement:
                    "turn-execution failure must route into ingress rollback and runtime lifecycle handling"
                        .into(),
                references_machines: vec![
                    "turn_execution".into(),
                    "runtime_ingress".into(),
                    "runtime_control".into(),
                ],
                references_actors: vec![
                    "turn_executor".into(),
                    "ordinary_ingress".into(),
                    "control_plane".into(),
                ],
            },
            CompositionInvariant {
                name: "execution_cancel_is_handled".into(),
                kind: CompositionInvariantKind::OutcomeHandled {
                    from_machine: "turn_execution".into(),
                    effect_variant: "RunCancelled".into(),
                    required_targets: vec![
                        RouteTarget {
                            machine: "runtime_ingress".into(),
                            input_variant: "RunCancelled".into(),
                        },
                        RouteTarget {
                            machine: "runtime_control".into(),
                            input_variant: "RunCancelled".into(),
                        },
                    ],
                },
                statement:
                    "turn-execution cancellation must route into ingress rollback and runtime lifecycle handling"
                        .into(),
                references_machines: vec![
                    "turn_execution".into(),
                    "runtime_ingress".into(),
                    "runtime_control".into(),
                ],
                references_actors: vec![
                    "turn_executor".into(),
                    "ordinary_ingress".into(),
                    "control_plane".into(),
                ],
            },
            CompositionInvariant {
                name: "execution_completion_is_handled".into(),
                kind: CompositionInvariantKind::OutcomeHandled {
                    from_machine: "turn_execution".into(),
                    effect_variant: "RunCompleted".into(),
                    required_targets: vec![
                        RouteTarget {
                            machine: "runtime_ingress".into(),
                            input_variant: "RunCompleted".into(),
                        },
                        RouteTarget {
                            machine: "runtime_control".into(),
                            input_variant: "RunCompleted".into(),
                        },
                    ],
                },
                statement:
                    "turn-execution completion must route into ingress consumption and runtime lifecycle handling"
                        .into(),
                references_machines: vec![
                    "turn_execution".into(),
                    "runtime_ingress".into(),
                    "runtime_control".into(),
                ],
                references_actors: vec![
                    "turn_executor".into(),
                    "ordinary_ingress".into(),
                    "control_plane".into(),
                ],
            },
        ],
        witnesses: vec![
            CompositionWitness {
                name: "prompt_queue_idle".into(),
                preload_inputs: vec![
                    CompositionWitnessInput {
                        machine: "runtime_control".into(),
                        input_variant: "Initialize".into(),
                        fields: vec![],
                    },
                    CompositionWitnessInput {
                        machine: "runtime_control".into(),
                        input_variant: "AdmissionAccepted".into(),
                        fields: admission_accepted_fields(
                            "prompt_1",
                            "WorkInput",
                            "TextOnly",
                            true,
                            false,
                        ),
                    },
                ],
                expected_routes: vec!["admitted_work_enters_ingress".into()],
                expected_scheduler_rules: vec![],
                expected_states: vec![
                    witness_state(
                        "runtime_control",
                        Some("Idle"),
                        vec![
                            CompositionWitnessField {
                                field: "wake_pending".into(),
                                expr: Expr::Bool(true),
                            },
                            CompositionWitnessField {
                                field: "process_pending".into(),
                                expr: Expr::Bool(false),
                            },
                        ],
                    ),
                    witness_state(
                        "runtime_ingress",
                        Some("Active"),
                        vec![
                            CompositionWitnessField {
                                field: "queue".into(),
                                expr: Expr::SeqLiteral(vec![Expr::String("prompt_1".into())]),
                            },
                            CompositionWitnessField {
                                field: "wake_requested".into(),
                                expr: Expr::Bool(true),
                            },
                            CompositionWitnessField {
                                field: "process_requested".into(),
                                expr: Expr::Bool(false),
                            },
                        ],
                    ),
                ],
                expected_transitions: vec![
                    witness_transition("runtime_control", "Initialize"),
                    witness_transition("runtime_control", "AdmissionAcceptedIdleQueue"),
                    witness_transition("runtime_ingress", "AdmitQueuedQueue"),
                ],
                expected_transition_order: vec![
                    witness_transition_order(
                        "runtime_control",
                        "Initialize",
                        "runtime_control",
                        "AdmissionAcceptedIdleQueue",
                    ),
                    witness_transition_order(
                        "runtime_control",
                        "AdmissionAcceptedIdleQueue",
                        "runtime_ingress",
                        "AdmitQueuedQueue",
                    ),
                ],
                state_limits: CompositionStateLimits {
                    step_limit: 3,
                    pending_input_limit: 2,
                    pending_route_limit: 1,
                    delivered_route_limit: 1,
                    emitted_effect_limit: 2,
                    seq_limit: 2,
                    set_limit: 2,
                    map_limit: 2,
                },
            },
            CompositionWitness {
                name: "prompt_steer_idle".into(),
                preload_inputs: vec![
                    CompositionWitnessInput {
                        machine: "runtime_control".into(),
                        input_variant: "Initialize".into(),
                        fields: vec![],
                    },
                    CompositionWitnessInput {
                        machine: "runtime_control".into(),
                        input_variant: "AdmissionAccepted".into(),
                        fields: admission_accepted_fields(
                            "prompt_1",
                            "WorkInput",
                            "InlineImage",
                            true,
                            true,
                        ),
                    },
                ],
                expected_routes: vec!["admitted_work_enters_ingress".into()],
                expected_scheduler_rules: vec![],
                expected_states: vec![
                    witness_state(
                        "runtime_control",
                        Some("Idle"),
                        vec![
                            CompositionWitnessField {
                                field: "wake_pending".into(),
                                expr: Expr::Bool(true),
                            },
                            CompositionWitnessField {
                                field: "process_pending".into(),
                                expr: Expr::Bool(true),
                            },
                        ],
                    ),
                    witness_state(
                        "runtime_ingress",
                        Some("Active"),
                        vec![
                            CompositionWitnessField {
                                field: "queue".into(),
                                expr: Expr::SeqLiteral(vec![Expr::String("prompt_1".into())]),
                            },
                            CompositionWitnessField {
                                field: "wake_requested".into(),
                                expr: Expr::Bool(true),
                            },
                            CompositionWitnessField {
                                field: "process_requested".into(),
                                expr: Expr::Bool(true),
                            },
                        ],
                    ),
                ],
                expected_transitions: vec![
                    witness_transition("runtime_control", "Initialize"),
                    witness_transition("runtime_control", "AdmissionAcceptedIdleSteer"),
                    witness_transition("runtime_ingress", "AdmitQueuedSteer"),
                ],
                expected_transition_order: vec![
                    witness_transition_order(
                        "runtime_control",
                        "Initialize",
                        "runtime_control",
                        "AdmissionAcceptedIdleSteer",
                    ),
                    witness_transition_order(
                        "runtime_control",
                        "AdmissionAcceptedIdleSteer",
                        "runtime_ingress",
                        "AdmitQueuedSteer",
                    ),
                ],
                state_limits: CompositionStateLimits {
                    step_limit: 3,
                    pending_input_limit: 2,
                    pending_route_limit: 1,
                    delivered_route_limit: 1,
                    emitted_effect_limit: 3,
                    seq_limit: 2,
                    set_limit: 2,
                    map_limit: 2,
                },
            },
            CompositionWitness {
                name: "prompt_queue_running".into(),
                preload_inputs: vec![
                    CompositionWitnessInput {
                        machine: "runtime_control".into(),
                        input_variant: "Initialize".into(),
                        fields: vec![],
                    },
                    CompositionWitnessInput {
                        machine: "runtime_control".into(),
                        input_variant: "AdmissionAccepted".into(),
                        fields: admission_accepted_fields(
                            "seed_1",
                            "WorkInput",
                            "TextOnly",
                            true,
                            false,
                        ),
                    },
                    CompositionWitnessInput {
                        machine: "runtime_ingress".into(),
                        input_variant: "StageDrainSnapshot".into(),
                        fields: vec![
                            CompositionWitnessField {
                                field: "run_id".into(),
                                expr: Expr::String("runid_1".into()),
                            },
                            CompositionWitnessField {
                                field: "contributing_work_ids".into(),
                                expr: Expr::SeqLiteral(vec![Expr::String("seed_1".into())]),
                            },
                        ],
                    },
                    CompositionWitnessInput {
                        machine: "runtime_control".into(),
                        input_variant: "AdmissionAccepted".into(),
                        fields: admission_accepted_fields(
                            "prompt_1",
                            "WorkInput",
                            "TextOnly",
                            false,
                            false,
                        ),
                    },
                ],
                expected_routes: vec![
                    "staged_run_notifies_control".into(),
                    "control_starts_execution".into(),
                    "admitted_work_enters_ingress".into(),
                ],
                expected_scheduler_rules: vec![],
                expected_states: vec![
                    witness_state(
                        "runtime_control",
                        Some("Running"),
                        vec![
                            CompositionWitnessField {
                                field: "wake_pending".into(),
                                expr: Expr::Bool(false),
                            },
                            CompositionWitnessField {
                                field: "process_pending".into(),
                                expr: Expr::Bool(false),
                            },
                        ],
                    ),
                    witness_state(
                        "runtime_ingress",
                        Some("Active"),
                        vec![
                            CompositionWitnessField {
                                field: "queue".into(),
                                expr: Expr::SeqLiteral(vec![Expr::String("prompt_1".into())]),
                            },
                            CompositionWitnessField {
                                field: "wake_requested".into(),
                                expr: Expr::Bool(false),
                            },
                            CompositionWitnessField {
                                field: "process_requested".into(),
                                expr: Expr::Bool(false),
                            },
                        ],
                    ),
                    witness_state("turn_execution", Some("ApplyingPrimitive"), vec![]),
                ],
                expected_transitions: vec![
                    witness_transition("runtime_control", "Initialize"),
                    witness_transition("runtime_control", "AdmissionAcceptedIdleQueue"),
                    witness_transition("runtime_ingress", "StageDrainSnapshot"),
                    witness_transition("runtime_control", "BeginRunFromIdle"),
                    witness_transition("turn_execution", "StartConversationRun"),
                    witness_transition("runtime_control", "AdmissionAcceptedRunningQueue"),
                    witness_transition("runtime_ingress", "AdmitQueuedQueue"),
                ],
                expected_transition_order: vec![
                    witness_transition_order(
                        "runtime_control",
                        "AdmissionAcceptedIdleQueue",
                        "runtime_ingress",
                        "StageDrainSnapshot",
                    ),
                    witness_transition_order(
                        "runtime_ingress",
                        "StageDrainSnapshot",
                        "runtime_control",
                        "BeginRunFromIdle",
                    ),
                    witness_transition_order(
                        "runtime_control",
                        "BeginRunFromIdle",
                        "turn_execution",
                        "StartConversationRun",
                    ),
                    witness_transition_order(
                        "runtime_control",
                        "BeginRunFromIdle",
                        "runtime_control",
                        "AdmissionAcceptedRunningQueue",
                    ),
                    witness_transition_order(
                        "runtime_control",
                        "AdmissionAcceptedRunningQueue",
                        "runtime_ingress",
                        "AdmitQueuedQueue",
                    ),
                ],
                state_limits: CompositionStateLimits {
                    step_limit: 8,
                    pending_input_limit: 4,
                    pending_route_limit: 1,
                    delivered_route_limit: 3,
                    emitted_effect_limit: 5,
                    seq_limit: 3,
                    set_limit: 3,
                    map_limit: 3,
                },
            },
            CompositionWitness {
                name: "prompt_steer_running".into(),
                preload_inputs: vec![
                    CompositionWitnessInput {
                        machine: "runtime_control".into(),
                        input_variant: "Initialize".into(),
                        fields: vec![],
                    },
                    CompositionWitnessInput {
                        machine: "runtime_control".into(),
                        input_variant: "AdmissionAccepted".into(),
                        fields: admission_accepted_fields(
                            "seed_1",
                            "WorkInput",
                            "TextOnly",
                            true,
                            false,
                        ),
                    },
                    CompositionWitnessInput {
                        machine: "runtime_ingress".into(),
                        input_variant: "StageDrainSnapshot".into(),
                        fields: vec![
                            CompositionWitnessField {
                                field: "run_id".into(),
                                expr: Expr::String("runid_1".into()),
                            },
                            CompositionWitnessField {
                                field: "contributing_work_ids".into(),
                                expr: Expr::SeqLiteral(vec![Expr::String("seed_1".into())]),
                            },
                        ],
                    },
                    CompositionWitnessInput {
                        machine: "runtime_control".into(),
                        input_variant: "AdmissionAccepted".into(),
                        fields: admission_accepted_fields(
                            "prompt_1",
                            "WorkInput",
                            "InlineImage",
                            true,
                            true,
                        ),
                    },
                ],
                expected_routes: vec![
                    "staged_run_notifies_control".into(),
                    "control_starts_execution".into(),
                    "admitted_work_enters_ingress".into(),
                ],
                expected_scheduler_rules: vec![],
                expected_states: vec![
                    witness_state(
                        "runtime_control",
                        Some("Running"),
                        vec![
                            CompositionWitnessField {
                                field: "wake_pending".into(),
                                expr: Expr::Bool(true),
                            },
                            CompositionWitnessField {
                                field: "process_pending".into(),
                                expr: Expr::Bool(true),
                            },
                        ],
                    ),
                    witness_state(
                        "runtime_ingress",
                        Some("Active"),
                        vec![
                            CompositionWitnessField {
                                field: "queue".into(),
                                expr: Expr::SeqLiteral(vec![Expr::String("prompt_1".into())]),
                            },
                            CompositionWitnessField {
                                field: "wake_requested".into(),
                                expr: Expr::Bool(true),
                            },
                            CompositionWitnessField {
                                field: "process_requested".into(),
                                expr: Expr::Bool(true),
                            },
                        ],
                    ),
                    witness_state("turn_execution", Some("ApplyingPrimitive"), vec![]),
                ],
                expected_transitions: vec![
                    witness_transition("runtime_control", "Initialize"),
                    witness_transition("runtime_control", "AdmissionAcceptedIdleQueue"),
                    witness_transition("runtime_ingress", "StageDrainSnapshot"),
                    witness_transition("runtime_control", "BeginRunFromIdle"),
                    witness_transition("turn_execution", "StartConversationRun"),
                    witness_transition("runtime_control", "AdmissionAcceptedRunningSteer"),
                    witness_transition("runtime_ingress", "AdmitQueuedSteer"),
                ],
                expected_transition_order: vec![
                    witness_transition_order(
                        "runtime_control",
                        "AdmissionAcceptedIdleQueue",
                        "runtime_ingress",
                        "StageDrainSnapshot",
                    ),
                    witness_transition_order(
                        "runtime_ingress",
                        "StageDrainSnapshot",
                        "runtime_control",
                        "BeginRunFromIdle",
                    ),
                    witness_transition_order(
                        "runtime_control",
                        "BeginRunFromIdle",
                        "turn_execution",
                        "StartConversationRun",
                    ),
                    witness_transition_order(
                        "runtime_control",
                        "BeginRunFromIdle",
                        "runtime_control",
                        "AdmissionAcceptedRunningSteer",
                    ),
                    witness_transition_order(
                        "runtime_control",
                        "AdmissionAcceptedRunningSteer",
                        "runtime_ingress",
                        "AdmitQueuedSteer",
                    ),
                ],
                state_limits: CompositionStateLimits {
                    step_limit: 8,
                    pending_input_limit: 4,
                    pending_route_limit: 1,
                    delivered_route_limit: 3,
                    emitted_effect_limit: 6,
                    seq_limit: 3,
                    set_limit: 3,
                    map_limit: 3,
                },
            },
            CompositionWitness {
                name: "steer_batch_running".into(),
                preload_inputs: vec![
                    CompositionWitnessInput {
                        machine: "runtime_control".into(),
                        input_variant: "Initialize".into(),
                        fields: vec![],
                    },
                    CompositionWitnessInput {
                        machine: "runtime_control".into(),
                        input_variant: "AdmissionAccepted".into(),
                        fields: admission_accepted_fields(
                            "seed_1",
                            "WorkInput",
                            "TextOnly",
                            true,
                            false,
                        ),
                    },
                    CompositionWitnessInput {
                        machine: "runtime_control".into(),
                        input_variant: "AdmissionAccepted".into(),
                        fields: admission_accepted_fields(
                            "prompt_1",
                            "WorkInput",
                            "TextOnly",
                            false,
                            true,
                        ),
                    },
                    CompositionWitnessInput {
                        machine: "runtime_control".into(),
                        input_variant: "AdmissionAccepted".into(),
                        fields: admission_accepted_fields(
                            "prompt_2",
                            "WorkInput",
                            "InlineImage",
                            false,
                            true,
                        ),
                    },
                    CompositionWitnessInput {
                        machine: "runtime_ingress".into(),
                        input_variant: "StageDrainSnapshot".into(),
                        fields: vec![
                            CompositionWitnessField {
                                field: "run_id".into(),
                                expr: Expr::String("runid_1".into()),
                            },
                            CompositionWitnessField {
                                field: "contributing_work_ids".into(),
                                expr: Expr::SeqLiteral(vec![Expr::String("seed_1".into())]),
                            },
                        ],
                    },
                    CompositionWitnessInput {
                        machine: "runtime_ingress".into(),
                        input_variant: "StageDrainSnapshot".into(),
                        fields: vec![
                            CompositionWitnessField {
                                field: "run_id".into(),
                                expr: Expr::String("runid_2".into()),
                            },
                            CompositionWitnessField {
                                field: "contributing_work_ids".into(),
                                expr: Expr::SeqLiteral(vec![
                                    Expr::String("prompt_1".into()),
                                    Expr::String("prompt_2".into()),
                                ]),
                            },
                        ],
                    },
                ],
                expected_routes: vec![
                    "control_starts_execution".into(),
                    "admitted_work_enters_ingress".into(),
                    "staged_run_notifies_control".into(),
                ],
                expected_scheduler_rules: vec![],
                expected_states: vec![
                    witness_state(
                        "runtime_ingress",
                        Some("Active"),
                        vec![
                            CompositionWitnessField {
                                field: "current_run".into(),
                                expr: Expr::Some(Box::new(Expr::String("runid_2".into()))),
                            },
                            CompositionWitnessField {
                                field: "current_run_contributors".into(),
                                expr: Expr::SeqLiteral(vec![
                                    Expr::String("prompt_1".into()),
                                    Expr::String("prompt_2".into()),
                                ]),
                            },
                            CompositionWitnessField {
                                field: "queue".into(),
                                expr: Expr::SeqLiteral(vec![]),
                            },
                        ],
                    ),
                ],
                expected_transitions: vec![
                    witness_transition("runtime_control", "Initialize"),
                    witness_transition("runtime_control", "AdmissionAcceptedIdleQueue"),
                    witness_transition("runtime_control", "BeginRunFromIdle"),
                    witness_transition("runtime_control", "AdmissionAcceptedRunningSteer"),
                    witness_transition("runtime_ingress", "AdmitQueuedSteer"),
                    witness_transition("runtime_ingress", "StageDrainSnapshot"),
                ],
                expected_transition_order: vec![
                    witness_transition_order(
                        "runtime_control",
                        "AdmissionAcceptedIdleQueue",
                        "runtime_control",
                        "BeginRunFromIdle",
                    ),
                    witness_transition_order(
                        "runtime_control",
                        "AdmissionAcceptedRunningSteer",
                        "runtime_ingress",
                        "AdmitQueuedSteer",
                    ),
                    witness_transition_order(
                        "runtime_ingress",
                        "AdmitQueuedSteer",
                        "runtime_ingress",
                        "StageDrainSnapshot",
                    ),
                ],
                state_limits: CompositionStateLimits {
                    step_limit: 10,
                    pending_input_limit: 6,
                    pending_route_limit: 2,
                    delivered_route_limit: 5,
                    emitted_effect_limit: 8,
                    seq_limit: 4,
                    set_limit: 4,
                    map_limit: 4,
                },
            },
            CompositionWitness {
                name: "success_path".into(),
                preload_inputs: vec![
                    CompositionWitnessInput {
                        machine: "runtime_control".into(),
                        input_variant: "Initialize".into(),
                        fields: vec![],
                    },
                    CompositionWitnessInput {
                        machine: "runtime_ingress".into(),
                        input_variant: "AdmitQueued".into(),
                        fields: admit_queued_fields(
                            "inputid_1",
                            "WorkInput",
                            "TextOnly",
                            "policydecision_1",
                            false,
                            false,
                        ),
                    },
                    CompositionWitnessInput {
                        machine: "runtime_ingress".into(),
                        input_variant: "StageDrainSnapshot".into(),
                        fields: vec![
                            CompositionWitnessField { field: "run_id".into(), expr: Expr::String("runid_1".into()) },
                            CompositionWitnessField { field: "contributing_work_ids".into(), expr: Expr::SeqLiteral(vec![Expr::String("inputid_1".into())]) },
                        ],
                    },
                    CompositionWitnessInput {
                        machine: "turn_execution".into(),
                        input_variant: "PrimitiveApplied".into(),
                        fields: primitive_applied_fields(
                            "runid_1",
                            "TextOnly",
                            false,
                            false,
                        ),
                    },
                    CompositionWitnessInput {
                        machine: "turn_execution".into(),
                        input_variant: "LlmReturnedTerminal".into(),
                        fields: vec![CompositionWitnessField { field: "run_id".into(), expr: Expr::String("runid_1".into()) }],
                    },
                    CompositionWitnessInput {
                        machine: "turn_execution".into(),
                        input_variant: "BoundaryComplete".into(),
                        fields: vec![CompositionWitnessField { field: "run_id".into(), expr: Expr::String("runid_1".into()) }],
                    },
                ],
                expected_routes: vec![
                    "staged_run_notifies_control".into(),
                    "control_starts_execution".into(),
                    "execution_boundary_updates_ingress".into(),
                    "execution_completion_updates_ingress".into(),
                    "execution_completion_notifies_control".into(),
                ],
                expected_scheduler_rules: vec![],
                expected_states: vec![
                    witness_state("runtime_control", Some("Idle"), vec![CompositionWitnessField {
                        field: "current_run_id".into(),
                        expr: Expr::None,
                    }]),
                    witness_state("runtime_ingress", Some("Active"), vec![CompositionWitnessField {
                        field: "current_run".into(),
                        expr: Expr::None,
                    }]),
                    witness_state("turn_execution", Some("Completed"), vec![]),
                ],
                expected_transitions: vec![
                    witness_transition("runtime_ingress", "StageDrainSnapshot"),
                    witness_transition("runtime_control", "BeginRunFromIdle"),
                    witness_transition("turn_execution", "StartConversationRun"),
                    witness_transition("turn_execution", "BoundaryComplete"),
                    witness_transition("runtime_ingress", "RunCompleted"),
                    witness_transition("runtime_control", "RunCompleted"),
                ],
                expected_transition_order: vec![
                    witness_transition_order(
                        "runtime_ingress",
                        "StageDrainSnapshot",
                        "runtime_control",
                        "BeginRunFromIdle",
                    ),
                    witness_transition_order(
                        "runtime_control",
                        "BeginRunFromIdle",
                        "turn_execution",
                        "StartConversationRun",
                    ),
                    witness_transition_order(
                        "turn_execution",
                        "BoundaryComplete",
                        "runtime_ingress",
                        "RunCompleted",
                    ),
                    witness_transition_order(
                        "turn_execution",
                        "BoundaryComplete",
                        "runtime_control",
                        "RunCompleted",
                    ),
                ],
                state_limits: CompositionStateLimits {
                    step_limit: 9,
                    pending_input_limit: 6,
                    pending_route_limit: 2,
                    delivered_route_limit: 6,
                    emitted_effect_limit: 6,
                    seq_limit: 6,
                    set_limit: 6,
                    map_limit: 2,
                },
            },
            CompositionWitness {
                name: "failure_path".into(),
                preload_inputs: vec![
                    CompositionWitnessInput {
                        machine: "runtime_control".into(),
                        input_variant: "Initialize".into(),
                        fields: vec![],
                    },
                    CompositionWitnessInput {
                        machine: "runtime_ingress".into(),
                        input_variant: "AdmitQueued".into(),
                        fields: admit_queued_fields(
                            "inputid_1",
                            "WorkInput",
                            "TextOnly",
                            "policydecision_1",
                            false,
                            false,
                        ),
                    },
                    CompositionWitnessInput {
                        machine: "runtime_ingress".into(),
                        input_variant: "StageDrainSnapshot".into(),
                        fields: vec![
                            CompositionWitnessField { field: "run_id".into(), expr: Expr::String("runid_1".into()) },
                            CompositionWitnessField { field: "contributing_work_ids".into(), expr: Expr::SeqLiteral(vec![Expr::String("inputid_1".into())]) },
                        ],
                    },
                    CompositionWitnessInput {
                        machine: "turn_execution".into(),
                        input_variant: "FatalFailure".into(),
                        fields: vec![CompositionWitnessField { field: "run_id".into(), expr: Expr::String("runid_1".into()) }],
                    },
                ],
                expected_routes: vec![
                    "staged_run_notifies_control".into(),
                    "control_starts_execution".into(),
                    "execution_failure_updates_ingress".into(),
                    "execution_failure_notifies_control".into(),
                ],
                expected_scheduler_rules: vec![],
                expected_states: vec![
                    witness_state("runtime_control", Some("Idle"), vec![CompositionWitnessField {
                        field: "current_run_id".into(),
                        expr: Expr::None,
                    }]),
                    witness_state("runtime_ingress", Some("Active"), vec![CompositionWitnessField {
                        field: "current_run".into(),
                        expr: Expr::None,
                    }]),
                    witness_state("turn_execution", Some("Failed"), vec![]),
                ],
                expected_transitions: vec![
                    witness_transition("runtime_ingress", "StageDrainSnapshot"),
                    witness_transition("runtime_control", "BeginRunFromIdle"),
                    witness_transition("turn_execution", "StartConversationRun"),
                    witness_transition("turn_execution", "FatalFailureFromApplyingPrimitive"),
                    witness_transition("runtime_ingress", "RunFailed"),
                    witness_transition("runtime_control", "RunFailed"),
                ],
                expected_transition_order: vec![
                    witness_transition_order(
                        "runtime_ingress",
                        "StageDrainSnapshot",
                        "runtime_control",
                        "BeginRunFromIdle",
                    ),
                    witness_transition_order(
                        "runtime_control",
                        "BeginRunFromIdle",
                        "turn_execution",
                        "StartConversationRun",
                    ),
                    witness_transition_order(
                        "turn_execution",
                        "FatalFailureFromApplyingPrimitive",
                        "runtime_ingress",
                        "RunFailed",
                    ),
                    witness_transition_order(
                        "turn_execution",
                        "FatalFailureFromApplyingPrimitive",
                        "runtime_control",
                        "RunFailed",
                    ),
                ],
                state_limits: CompositionStateLimits {
                    step_limit: 7,
                    pending_input_limit: 4,
                    pending_route_limit: 2,
                    delivered_route_limit: 5,
                    emitted_effect_limit: 5,
                    seq_limit: 4,
                    set_limit: 5,
                    map_limit: 2,
                },
            },
            CompositionWitness {
                name: "cancel_path".into(),
                preload_inputs: vec![
                    CompositionWitnessInput {
                        machine: "runtime_control".into(),
                        input_variant: "Initialize".into(),
                        fields: vec![],
                    },
                    CompositionWitnessInput {
                        machine: "runtime_ingress".into(),
                        input_variant: "AdmitQueued".into(),
                        fields: admit_queued_fields(
                            "inputid_1",
                            "WorkInput",
                            "TextOnly",
                            "policydecision_1",
                            false,
                            false,
                        ),
                    },
                    CompositionWitnessInput {
                        machine: "runtime_ingress".into(),
                        input_variant: "StageDrainSnapshot".into(),
                        fields: vec![
                            CompositionWitnessField { field: "run_id".into(), expr: Expr::String("runid_1".into()) },
                            CompositionWitnessField { field: "contributing_work_ids".into(), expr: Expr::SeqLiteral(vec![Expr::String("inputid_1".into())]) },
                        ],
                    },
                    CompositionWitnessInput {
                        machine: "turn_execution".into(),
                        input_variant: "CancelNow".into(),
                        fields: vec![CompositionWitnessField { field: "run_id".into(), expr: Expr::String("runid_1".into()) }],
                    },
                    CompositionWitnessInput {
                        machine: "turn_execution".into(),
                        input_variant: "CancellationObserved".into(),
                        fields: vec![CompositionWitnessField { field: "run_id".into(), expr: Expr::String("runid_1".into()) }],
                    },
                ],
                expected_routes: vec![
                    "staged_run_notifies_control".into(),
                    "control_starts_execution".into(),
                    "execution_cancel_updates_ingress".into(),
                    "execution_cancel_notifies_control".into(),
                ],
                expected_scheduler_rules: vec![],
                expected_states: vec![
                    witness_state("runtime_control", Some("Idle"), vec![CompositionWitnessField {
                        field: "current_run_id".into(),
                        expr: Expr::None,
                    }]),
                    witness_state("runtime_ingress", Some("Active"), vec![CompositionWitnessField {
                        field: "current_run".into(),
                        expr: Expr::None,
                    }]),
                    witness_state("turn_execution", Some("Cancelled"), vec![]),
                ],
                expected_transitions: vec![
                    witness_transition("runtime_ingress", "StageDrainSnapshot"),
                    witness_transition("runtime_control", "BeginRunFromIdle"),
                    witness_transition("turn_execution", "StartConversationRun"),
                    witness_transition("turn_execution", "CancelNowFromApplyingPrimitive"),
                    witness_transition("turn_execution", "CancellationObserved"),
                    witness_transition("runtime_ingress", "RunCancelled"),
                    witness_transition("runtime_control", "RunCancelled"),
                ],
                expected_transition_order: vec![
                    witness_transition_order(
                        "runtime_ingress",
                        "StageDrainSnapshot",
                        "runtime_control",
                        "BeginRunFromIdle",
                    ),
                    witness_transition_order(
                        "runtime_control",
                        "BeginRunFromIdle",
                        "turn_execution",
                        "StartConversationRun",
                    ),
                    witness_transition_order(
                        "turn_execution",
                        "CancellationObserved",
                        "runtime_ingress",
                        "RunCancelled",
                    ),
                    witness_transition_order(
                        "turn_execution",
                        "CancellationObserved",
                        "runtime_control",
                        "RunCancelled",
                    ),
                ],
                state_limits: CompositionStateLimits {
                    step_limit: 8,
                    pending_input_limit: 5,
                    pending_route_limit: 2,
                    delivered_route_limit: 5,
                    emitted_effect_limit: 5,
                    seq_limit: 5,
                    set_limit: 5,
                    map_limit: 2,
                },
            },
            CompositionWitness {
                name: "cancel_after_boundary_path".into(),
                preload_inputs: vec![
                    CompositionWitnessInput {
                        machine: "runtime_control".into(),
                        input_variant: "Initialize".into(),
                        fields: vec![],
                    },
                    CompositionWitnessInput {
                        machine: "runtime_ingress".into(),
                        input_variant: "AdmitQueued".into(),
                        fields: admit_queued_fields(
                            "inputid_1",
                            "WorkInput",
                            "TextOnly",
                            "policydecision_1",
                            false,
                            false,
                        ),
                    },
                    CompositionWitnessInput {
                        machine: "runtime_ingress".into(),
                        input_variant: "StageDrainSnapshot".into(),
                        fields: vec![
                            CompositionWitnessField {
                                field: "run_id".into(),
                                expr: Expr::String("runid_1".into()),
                            },
                            CompositionWitnessField {
                                field: "contributing_work_ids".into(),
                                expr: Expr::SeqLiteral(vec![Expr::String("inputid_1".into())]),
                            },
                        ],
                    },
                    CompositionWitnessInput {
                        machine: "turn_execution".into(),
                        input_variant: "PrimitiveApplied".into(),
                        fields: primitive_applied_fields("runid_1", "TextOnly", false, false),
                    },
                    CompositionWitnessInput {
                        machine: "turn_execution".into(),
                        input_variant: "CancelAfterBoundary".into(),
                        fields: vec![CompositionWitnessField {
                            field: "run_id".into(),
                            expr: Expr::String("runid_1".into()),
                        }],
                    },
                    CompositionWitnessInput {
                        machine: "turn_execution".into(),
                        input_variant: "LlmReturnedTerminal".into(),
                        fields: vec![CompositionWitnessField {
                            field: "run_id".into(),
                            expr: Expr::String("runid_1".into()),
                        }],
                    },
                    CompositionWitnessInput {
                        machine: "turn_execution".into(),
                        input_variant: "BoundaryComplete".into(),
                        fields: vec![CompositionWitnessField {
                            field: "run_id".into(),
                            expr: Expr::String("runid_1".into()),
                        }],
                    },
                ],
                expected_routes: vec![
                    "staged_run_notifies_control".into(),
                    "control_starts_execution".into(),
                    "execution_cancel_updates_ingress".into(),
                    "execution_cancel_notifies_control".into(),
                ],
                expected_scheduler_rules: vec![],
                expected_states: vec![
                    witness_state("runtime_control", Some("Idle"), vec![]),
                    witness_state("runtime_ingress", Some("Active"), vec![CompositionWitnessField {
                        field: "current_run".into(),
                        expr: Expr::None,
                    }]),
                    witness_state("turn_execution", Some("Cancelled"), vec![]),
                ],
                expected_transitions: vec![
                    witness_transition("runtime_ingress", "StageDrainSnapshot"),
                    witness_transition("runtime_control", "BeginRunFromIdle"),
                    witness_transition("turn_execution", "StartConversationRun"),
                    witness_transition("turn_execution", "PrimitiveAppliedConversationTurn"),
                    witness_transition("turn_execution", "CancelAfterBoundaryFromCallingLlm"),
                    witness_transition("turn_execution", "LlmReturnedTerminal"),
                    witness_transition("turn_execution", "BoundaryCompleteCancelsAfterBoundary"),
                    witness_transition("runtime_ingress", "RunCancelled"),
                    witness_transition("runtime_control", "RunCancelled"),
                ],
                expected_transition_order: vec![
                    witness_transition_order(
                        "runtime_ingress",
                        "StageDrainSnapshot",
                        "runtime_control",
                        "BeginRunFromIdle",
                    ),
                    witness_transition_order(
                        "runtime_control",
                        "BeginRunFromIdle",
                        "turn_execution",
                        "StartConversationRun",
                    ),
                    witness_transition_order(
                        "turn_execution",
                        "PrimitiveAppliedConversationTurn",
                        "turn_execution",
                        "CancelAfterBoundaryFromCallingLlm",
                    ),
                    witness_transition_order(
                        "turn_execution",
                        "CancelAfterBoundaryFromCallingLlm",
                        "turn_execution",
                        "BoundaryCompleteCancelsAfterBoundary",
                    ),
                    witness_transition_order(
                        "turn_execution",
                        "BoundaryCompleteCancelsAfterBoundary",
                        "runtime_ingress",
                        "RunCancelled",
                    ),
                    witness_transition_order(
                        "turn_execution",
                        "BoundaryCompleteCancelsAfterBoundary",
                        "runtime_control",
                        "RunCancelled",
                    ),
                ],
                state_limits: CompositionStateLimits {
                    step_limit: 10,
                    pending_input_limit: 7,
                    pending_route_limit: 2,
                    delivered_route_limit: 6,
                    emitted_effect_limit: 6,
                    seq_limit: 5,
                    set_limit: 5,
                    map_limit: 3,
                },
            },
            CompositionWitness {
                name: "control_preemption".into(),
                preload_inputs: vec![
                    CompositionWitnessInput {
                        machine: "runtime_control".into(),
                        input_variant: "Initialize".into(),
                        fields: vec![],
                    },
                    CompositionWitnessInput {
                        machine: "runtime_ingress".into(),
                        input_variant: "AdmitQueued".into(),
                        fields: admit_queued_fields(
                            "inputid_1",
                            "WorkInput",
                            "TextOnly",
                            "policydecision_1",
                            true,
                            true,
                        ),
                    },
                ],
                expected_routes: vec![],
                expected_scheduler_rules: vec![SchedulerRule::PreemptWhenReady {
                    higher: "control_plane".into(),
                    lower: "ordinary_ingress".into(),
                }],
                expected_states: vec![witness_state("runtime_control", Some("Idle"), vec![])],
                expected_transitions: vec![
                    witness_transition("runtime_control", "Initialize"),
                    witness_transition("runtime_ingress", "AdmitQueuedSteer"),
                ],
                expected_transition_order: vec![witness_transition_order(
                    "runtime_control",
                    "Initialize",
                    "runtime_ingress",
                        "AdmitQueuedSteer",
                )],
                state_limits: CompositionStateLimits {
                    step_limit: 2,
                    pending_input_limit: 2,
                    pending_route_limit: 1,
                    delivered_route_limit: 1,
                    emitted_effect_limit: 1,
                    seq_limit: 2,
                    set_limit: 2,
                    map_limit: 1,
                },
            },
        ],
        deep_domain_cardinality: 2,
        deep_domain_overrides: BTreeMap::from([
            ("ContentShapeValues".into(), 1),
            ("HandlingModeValues".into(), 2),
            ("PolicyDecisionValues".into(), 1),
            ("RequestIdValues".into(), 1),
            ("ReservationKeyValues".into(), 1),
            ("RunIdValues".into(), 1),
            ("StringValues".into(), 1),
            ("WorkIdValues".into(), 2),
        ]),
        witness_domain_cardinality: 1,
    }
}

pub fn external_tool_bundle_composition() -> CompositionSchema {
    CompositionSchema {
        name: "external_tool_bundle".into(),
        machines: vec![
            MachineInstance {
                instance_id: "external_tool_surface".into(),
                machine_name: "ExternalToolSurfaceMachine".into(),
                actor: "surface_boundary".into(),
            },
            MachineInstance {
                instance_id: "runtime_control".into(),
                machine_name: "RuntimeControlMachine".into(),
                actor: "control_plane".into(),
            },
            MachineInstance {
                instance_id: "turn_execution".into(),
                machine_name: "TurnExecutionMachine".into(),
                actor: "turn_executor".into(),
            },
        ],
        entry_inputs: vec![
            EntryInput {
                name: "control_initialize".into(),
                machine: "runtime_control".into(),
                input_variant: "Initialize".into(),
            },
            EntryInput {
                name: "stage_add".into(),
                machine: "external_tool_surface".into(),
                input_variant: "StageAdd".into(),
            },
            EntryInput {
                name: "stage_remove".into(),
                machine: "external_tool_surface".into(),
                input_variant: "StageRemove".into(),
            },
            EntryInput {
                name: "stage_reload".into(),
                machine: "external_tool_surface".into(),
                input_variant: "StageReload".into(),
            },
            EntryInput {
                name: "pending_succeeded".into(),
                machine: "external_tool_surface".into(),
                input_variant: "PendingSucceeded".into(),
            },
            EntryInput {
                name: "pending_failed".into(),
                machine: "external_tool_surface".into(),
                input_variant: "PendingFailed".into(),
            },
            EntryInput {
                name: "call_started".into(),
                machine: "external_tool_surface".into(),
                input_variant: "CallStarted".into(),
            },
            EntryInput {
                name: "call_finished".into(),
                machine: "external_tool_surface".into(),
                input_variant: "CallFinished".into(),
            },
            EntryInput {
                name: "finalize_removal_clean".into(),
                machine: "external_tool_surface".into(),
                input_variant: "FinalizeRemovalClean".into(),
            },
            EntryInput {
                name: "finalize_removal_forced".into(),
                machine: "external_tool_surface".into(),
                input_variant: "FinalizeRemovalForced".into(),
            },
            EntryInput {
                name: "turn_start_conversation".into(),
                machine: "turn_execution".into(),
                input_variant: "StartConversationRun".into(),
            },
            EntryInput {
                name: "turn_primitive_applied".into(),
                machine: "turn_execution".into(),
                input_variant: "PrimitiveApplied".into(),
            },
            EntryInput {
                name: "turn_llm_returned_terminal".into(),
                machine: "turn_execution".into(),
                input_variant: "LlmReturnedTerminal".into(),
            },
            EntryInput {
                name: "turn_boundary_complete".into(),
                machine: "turn_execution".into(),
                input_variant: "BoundaryComplete".into(),
            },
        ],
        routes: vec![
            Route {
                name: "surface_delta_notifies_runtime_control".into(),
                from_machine: "external_tool_surface".into(),
                effect_variant: "EmitExternalToolDelta".into(),
                to: RouteTarget {
                    machine: "runtime_control".into(),
                    input_variant: "ExternalToolDeltaReceived".into(),
                },
                bindings: vec![],
                delivery: RouteDelivery::Immediate,
            },
            Route {
                name: "turn_boundary_applies_surface_changes".into(),
                from_machine: "turn_execution".into(),
                effect_variant: "BoundaryApplied".into(),
                to: RouteTarget {
                    machine: "external_tool_surface".into(),
                    input_variant: "ApplyBoundary".into(),
                },
                bindings: vec![
                    RouteFieldBinding {
                        to_field: "surface_id".into(),
                        source: RouteBindingSource::Literal(Expr::String("default_surface".into())),
                    },
                    RouteFieldBinding {
                        to_field: "applied_at_turn".into(),
                        source: RouteBindingSource::Literal(Expr::String("turn_1".into())),
                    },
                ],
                delivery: RouteDelivery::Immediate,
            },
        ],
        actor_priorities: vec![ActorPriority {
            higher: "control_plane".into(),
            lower: "surface_boundary".into(),
            reason: "runtime control decisions outrank pending surface churn".into(),
        }],
        scheduler_rules: vec![SchedulerRule::PreemptWhenReady {
            higher: "control_plane".into(),
            lower: "surface_boundary".into(),
        }],
        invariants: vec![
            CompositionInvariant {
                name: "external_tool_delta_enters_runtime_control".into(),
                kind: CompositionInvariantKind::ObservedInputOriginatesFromEffect {
                    to_machine: "runtime_control".into(),
                    input_variant: "ExternalToolDeltaReceived".into(),
                    from_machine: "external_tool_surface".into(),
                    effect_variant: "EmitExternalToolDelta".into(),
                },
                statement:
                    "canonical external-tool deltas enter runtime through the runtime-control boundary"
                        .into(),
                references_machines: vec!["external_tool_surface".into(), "runtime_control".into()],
                references_actors: vec!["surface_boundary".into(), "control_plane".into()],
            },
            CompositionInvariant {
                name: "boundary_application_reaches_surface_authority".into(),
                kind: CompositionInvariantKind::ObservedInputOriginatesFromEffect {
                    to_machine: "external_tool_surface".into(),
                    input_variant: "ApplyBoundary".into(),
                    from_machine: "turn_execution".into(),
                    effect_variant: "BoundaryApplied".into(),
                },
                statement:
                    "turn-execution boundary application is reflected in the external-tool surface boundary"
                        .into(),
                references_machines: vec!["turn_execution".into(), "external_tool_surface".into()],
                references_actors: vec!["turn_executor".into(), "surface_boundary".into()],
            },
            CompositionInvariant {
                name: "control_preempts_surface_boundary".into(),
                kind: CompositionInvariantKind::SchedulerRulePresent {
                    rule: SchedulerRule::PreemptWhenReady {
                        higher: "control_plane".into(),
                        lower: "surface_boundary".into(),
                    },
                },
                statement: "runtime control outranks surface-boundary work when both are ready".into(),
                references_machines: vec!["runtime_control".into(), "external_tool_surface".into()],
                references_actors: vec!["control_plane".into(), "surface_boundary".into()],
            },
        ],
        witnesses: vec![
            CompositionWitness {
                name: "surface_add_notifies_control".into(),
                preload_inputs: vec![
                    CompositionWitnessInput {
                        machine: "runtime_control".into(),
                        input_variant: "Initialize".into(),
                        fields: vec![],
                    },
                    CompositionWitnessInput {
                        machine: "external_tool_surface".into(),
                        input_variant: "StageAdd".into(),
                        fields: vec![CompositionWitnessField {
                            field: "surface_id".into(),
                            expr: Expr::String("default_surface".into()),
                        }],
                    },
                    CompositionWitnessInput {
                        machine: "turn_execution".into(),
                        input_variant: "StartConversationRun".into(),
                        fields: vec![CompositionWitnessField {
                            field: "run_id".into(),
                            expr: Expr::String("runid_1".into()),
                        }],
                    },
                    CompositionWitnessInput {
                        machine: "turn_execution".into(),
                        input_variant: "PrimitiveApplied".into(),
                        fields: primitive_applied_fields(
                            "runid_1",
                            "TextOnly",
                            false,
                            false,
                        ),
                    },
                    CompositionWitnessInput {
                        machine: "turn_execution".into(),
                        input_variant: "LlmReturnedTerminal".into(),
                        fields: vec![CompositionWitnessField {
                            field: "run_id".into(),
                            expr: Expr::String("runid_1".into()),
                        }],
                    },
                    CompositionWitnessInput {
                        machine: "turn_execution".into(),
                        input_variant: "BoundaryComplete".into(),
                        fields: vec![CompositionWitnessField {
                            field: "run_id".into(),
                            expr: Expr::String("runid_1".into()),
                        }],
                    },
                ],
                expected_routes: vec![
                    "turn_boundary_applies_surface_changes".into(),
                    "surface_delta_notifies_runtime_control".into(),
                ],
                expected_scheduler_rules: vec![],
                expected_states: vec![
                    witness_state("external_tool_surface", Some("Operating"), vec![]),
                    witness_state("runtime_control", Some("Idle"), vec![]),
                    witness_state("turn_execution", Some("Completed"), vec![]),
                ],
                expected_transitions: vec![
                    witness_transition("runtime_control", "Initialize"),
                    witness_transition("external_tool_surface", "StageAdd"),
                    witness_transition("turn_execution", "StartConversationRun"),
                    witness_transition("turn_execution", "PrimitiveAppliedConversationTurn"),
                    witness_transition("turn_execution", "LlmReturnedTerminal"),
                    witness_transition("turn_execution", "BoundaryComplete"),
                    witness_transition("external_tool_surface", "ApplyBoundaryAdd"),
                    witness_transition("runtime_control", "ExternalToolDeltaReceivedIdle"),
                ],
                expected_transition_order: vec![
                    witness_transition_order(
                        "turn_execution",
                        "LlmReturnedTerminal",
                        "external_tool_surface",
                        "ApplyBoundaryAdd",
                    ),
                    witness_transition_order(
                        "external_tool_surface",
                        "ApplyBoundaryAdd",
                        "turn_execution",
                        "BoundaryComplete",
                    ),
                    witness_transition_order(
                        "external_tool_surface",
                        "ApplyBoundaryAdd",
                        "runtime_control",
                        "ExternalToolDeltaReceivedIdle",
                    ),
                ],
                state_limits: CompositionStateLimits {
                    step_limit: 8,
                    pending_input_limit: 6,
                    pending_route_limit: 2,
                    delivered_route_limit: 4,
                    emitted_effect_limit: 4,
                    seq_limit: 6,
                    set_limit: 6,
                    map_limit: 3,
                },
            },
            CompositionWitness {
                name: "turn_boundary_reaches_surface".into(),
                preload_inputs: vec![
                    CompositionWitnessInput {
                        machine: "runtime_control".into(),
                        input_variant: "Initialize".into(),
                        fields: vec![],
                    },
                    CompositionWitnessInput {
                        machine: "external_tool_surface".into(),
                        input_variant: "StageAdd".into(),
                        fields: vec![CompositionWitnessField {
                            field: "surface_id".into(),
                            expr: Expr::String("default_surface".into()),
                        }],
                    },
                    CompositionWitnessInput {
                        machine: "turn_execution".into(),
                        input_variant: "StartConversationRun".into(),
                        fields: vec![CompositionWitnessField {
                            field: "run_id".into(),
                            expr: Expr::String("runid_1".into()),
                        }],
                    },
                    CompositionWitnessInput {
                        machine: "turn_execution".into(),
                        input_variant: "PrimitiveApplied".into(),
                        fields: primitive_applied_fields(
                            "runid_1",
                            "TextOnly",
                            false,
                            false,
                        ),
                    },
                    CompositionWitnessInput {
                        machine: "turn_execution".into(),
                        input_variant: "LlmReturnedTerminal".into(),
                        fields: vec![CompositionWitnessField {
                            field: "run_id".into(),
                            expr: Expr::String("runid_1".into()),
                        }],
                    },
                    CompositionWitnessInput {
                        machine: "turn_execution".into(),
                        input_variant: "BoundaryComplete".into(),
                        fields: vec![CompositionWitnessField {
                            field: "run_id".into(),
                            expr: Expr::String("runid_1".into()),
                        }],
                    },
                ],
                expected_routes: vec!["turn_boundary_applies_surface_changes".into()],
                expected_scheduler_rules: vec![],
                expected_states: vec![
                    witness_state("runtime_control", Some("Idle"), vec![]),
                    witness_state("turn_execution", Some("Completed"), vec![]),
                    witness_state("external_tool_surface", Some("Operating"), vec![]),
                ],
                expected_transitions: vec![
                    witness_transition("runtime_control", "Initialize"),
                    witness_transition("external_tool_surface", "StageAdd"),
                    witness_transition("turn_execution", "StartConversationRun"),
                    witness_transition("turn_execution", "PrimitiveAppliedConversationTurn"),
                    witness_transition("turn_execution", "LlmReturnedTerminal"),
                    witness_transition("runtime_control", "ExternalToolDeltaReceivedIdle"),
                    witness_transition("turn_execution", "BoundaryComplete"),
                    witness_transition("external_tool_surface", "ApplyBoundaryAdd"),
                ],
                expected_transition_order: vec![
                    witness_transition_order(
                        "turn_execution",
                        "LlmReturnedTerminal",
                        "external_tool_surface",
                        "ApplyBoundaryAdd",
                    ),
                    witness_transition_order(
                        "external_tool_surface",
                        "ApplyBoundaryAdd",
                        "runtime_control",
                        "ExternalToolDeltaReceivedIdle",
                    ),
                    witness_transition_order(
                        "runtime_control",
                        "ExternalToolDeltaReceivedIdle",
                        "turn_execution",
                        "BoundaryComplete",
                    ),
                ],
                state_limits: CompositionStateLimits {
                    step_limit: 14,
                    pending_input_limit: 6,
                    pending_route_limit: 2,
                    delivered_route_limit: 4,
                    emitted_effect_limit: 8,
                    seq_limit: 6,
                    set_limit: 6,
                    map_limit: 3,
                },
            },
            CompositionWitness {
                name: "control_preempts_surface".into(),
                preload_inputs: vec![
                    CompositionWitnessInput {
                        machine: "runtime_control".into(),
                        input_variant: "Initialize".into(),
                        fields: vec![],
                    },
                    CompositionWitnessInput {
                        machine: "external_tool_surface".into(),
                        input_variant: "StageAdd".into(),
                        fields: vec![CompositionWitnessField {
                            field: "surface_id".into(),
                            expr: Expr::String("surface_1".into()),
                        }],
                    },
                ],
                expected_routes: vec![],
                expected_scheduler_rules: vec![SchedulerRule::PreemptWhenReady {
                    higher: "control_plane".into(),
                    lower: "surface_boundary".into(),
                }],
                expected_states: vec![witness_state("runtime_control", Some("Idle"), vec![])],
                expected_transitions: vec![
                    witness_transition("runtime_control", "Initialize"),
                    witness_transition("external_tool_surface", "StageAdd"),
                ],
                expected_transition_order: vec![witness_transition_order(
                    "runtime_control",
                    "Initialize",
                    "external_tool_surface",
                    "StageAdd",
                )],
                state_limits: CompositionStateLimits {
                    step_limit: 2,
                    pending_input_limit: 2,
                    pending_route_limit: 1,
                    delivered_route_limit: 1,
                    emitted_effect_limit: 1,
                    seq_limit: 2,
                    set_limit: 2,
                    map_limit: 2,
                },
            },
        ],
        deep_domain_cardinality: 2,
        deep_domain_overrides: BTreeMap::new(),
        witness_domain_cardinality: 1,
    }
}

pub fn peer_runtime_bundle_composition() -> CompositionSchema {
    CompositionSchema {
        name: "peer_runtime_bundle".into(),
        machines: vec![
            MachineInstance {
                instance_id: "peer_comms".into(),
                machine_name: "PeerCommsMachine".into(),
                actor: "peer_plane".into(),
            },
            MachineInstance {
                instance_id: "runtime_control".into(),
                machine_name: "RuntimeControlMachine".into(),
                actor: "control_plane".into(),
            },
            MachineInstance {
                instance_id: "runtime_ingress".into(),
                machine_name: "RuntimeIngressMachine".into(),
                actor: "ordinary_ingress".into(),
            },
        ],
        entry_inputs: vec![
            EntryInput {
                name: "control_initialize".into(),
                machine: "runtime_control".into(),
                input_variant: "Initialize".into(),
            },
            EntryInput {
                name: "trust_peer".into(),
                machine: "peer_comms".into(),
                input_variant: "TrustPeer".into(),
            },
            EntryInput {
                name: "receive_peer_envelope".into(),
                machine: "peer_comms".into(),
                input_variant: "ReceivePeerEnvelope".into(),
            },
            EntryInput {
                name: "submit_typed_peer_input".into(),
                machine: "peer_comms".into(),
                input_variant: "SubmitTypedPeerInput".into(),
            },
            EntryInput {
                name: "runtime_admission_accepted".into(),
                machine: "runtime_control".into(),
                input_variant: "AdmissionAccepted".into(),
            },
            EntryInput {
                name: "ingress_stage_drain_snapshot".into(),
                machine: "runtime_ingress".into(),
                input_variant: "StageDrainSnapshot".into(),
            },
        ],
        routes: vec![
            Route {
                name: "peer_candidate_enters_runtime_admission".into(),
                from_machine: "peer_comms".into(),
                effect_variant: "SubmitPeerInputCandidate".into(),
                to: RouteTarget {
                    machine: "runtime_control".into(),
                    input_variant: "SubmitWork".into(),
                },
                bindings: vec![
                    RouteFieldBinding {
                        to_field: "work_id".into(),
                        source: RouteBindingSource::Field {
                            from_field: "raw_item_id".into(),
                            allow_named_alias: true,
                        },
                    },
                    RouteFieldBinding {
                        to_field: "content_shape".into(),
                        source: RouteBindingSource::Field {
                            from_field: "content_shape".into(),
                            allow_named_alias: false,
                        },
                    },
                    RouteFieldBinding {
                        to_field: "request_id".into(),
                        source: RouteBindingSource::Field {
                            from_field: "request_id".into(),
                            allow_named_alias: false,
                        },
                    },
                    RouteFieldBinding {
                        to_field: "reservation_key".into(),
                        source: RouteBindingSource::Field {
                            from_field: "reservation_key".into(),
                            allow_named_alias: false,
                        },
                    },
                    RouteFieldBinding {
                        to_field: "handling_mode".into(),
                        source: RouteBindingSource::Literal(Expr::String("Steer".into())),
                    },
                ],
                delivery: RouteDelivery::Immediate,
            },
            Route {
                name: "admitted_peer_work_enters_ingress".into(),
                from_machine: "runtime_control".into(),
                effect_variant: "SubmitAdmittedIngressEffect".into(),
                to: RouteTarget {
                    machine: "runtime_ingress".into(),
                    input_variant: "AdmitQueued".into(),
                },
                bindings: vec![
                    RouteFieldBinding {
                        to_field: "work_id".into(),
                        source: RouteBindingSource::Field {
                            from_field: "work_id".into(),
                            allow_named_alias: true,
                        },
                    },
                    RouteFieldBinding {
                        to_field: "content_shape".into(),
                        source: RouteBindingSource::Field {
                            from_field: "content_shape".into(),
                            allow_named_alias: false,
                        },
                    },
                    RouteFieldBinding {
                        to_field: "request_id".into(),
                        source: RouteBindingSource::Field {
                            from_field: "request_id".into(),
                            allow_named_alias: false,
                        },
                    },
                    RouteFieldBinding {
                        to_field: "reservation_key".into(),
                        source: RouteBindingSource::Field {
                            from_field: "reservation_key".into(),
                            allow_named_alias: false,
                        },
                    },
                    RouteFieldBinding {
                        to_field: "handling_mode".into(),
                        source: RouteBindingSource::Field {
                            from_field: "handling_mode".into(),
                            allow_named_alias: false,
                        },
                    },
                    RouteFieldBinding {
                        to_field: "policy".into(),
                        source: RouteBindingSource::Literal(Expr::String("PeerQueued".into())),
                    },
                ],
                delivery: RouteDelivery::Immediate,
            },
        ],
        actor_priorities: vec![ActorPriority {
            higher: "control_plane".into(),
            lower: "peer_plane".into(),
            reason: "runtime admission/control outranks peer delivery once both are ready".into(),
        }],
        scheduler_rules: vec![SchedulerRule::PreemptWhenReady {
            higher: "control_plane".into(),
            lower: "peer_plane".into(),
        }],
        invariants: vec![
            CompositionInvariant {
                name: "peer_work_enters_runtime_via_canonical_admission".into(),
                kind: CompositionInvariantKind::ObservedInputOriginatesFromEffect {
                    to_machine: "runtime_control".into(),
                    input_variant: "SubmitWork".into(),
                    from_machine: "peer_comms".into(),
                    effect_variant: "SubmitPeerInputCandidate".into(),
                },
                statement:
                    "peer-classified work enters the runtime through the runtime-control admission surface"
                        .into(),
                references_machines: vec!["peer_comms".into(), "runtime_control".into()],
                references_actors: vec!["peer_plane".into(), "control_plane".into()],
            },
            CompositionInvariant {
                name: "peer_admission_flows_into_ingress".into(),
                kind: CompositionInvariantKind::ObservedInputOriginatesFromEffect {
                    to_machine: "runtime_ingress".into(),
                    input_variant: "AdmitQueued".into(),
                    from_machine: "runtime_control".into(),
                    effect_variant: "SubmitAdmittedIngressEffect".into(),
                },
                statement:
                    "peer-admitted work is handed from runtime control into canonical ingress ownership"
                        .into(),
                references_machines: vec![
                    "peer_comms".into(),
                    "runtime_control".into(),
                    "runtime_ingress".into(),
                ],
                references_actors: vec![
                    "peer_plane".into(),
                    "control_plane".into(),
                    "ordinary_ingress".into(),
                ],
            },
            CompositionInvariant {
                name: "control_preempts_peer_delivery".into(),
                kind: CompositionInvariantKind::SchedulerRulePresent {
                    rule: SchedulerRule::PreemptWhenReady {
                        higher: "control_plane".into(),
                        lower: "peer_plane".into(),
                    },
                },
                statement: "runtime control outranks peer delivery once both planes are ready".into(),
                references_machines: vec!["peer_comms".into(), "runtime_control".into()],
                references_actors: vec!["peer_plane".into(), "control_plane".into()],
            },
        ],
        witnesses: vec![
            CompositionWitness {
                name: "trusted_peer_enters_runtime".into(),
                preload_inputs: vec![
                    CompositionWitnessInput {
                        machine: "runtime_control".into(),
                        input_variant: "Initialize".into(),
                        fields: vec![],
                    },
                    CompositionWitnessInput {
                        machine: "peer_comms".into(),
                        input_variant: "TrustPeer".into(),
                        fields: vec![CompositionWitnessField {
                            field: "peer_id".into(),
                            expr: Expr::String("peer_1".into()),
                        }],
                    },
                    CompositionWitnessInput {
                        machine: "peer_comms".into(),
                        input_variant: "ReceivePeerEnvelope".into(),
                        fields: peer_envelope_fields(
                            "raw_1",
                            "peer_1",
                            "Message",
                            "see attached diagram",
                            "InlineImage",
                            None,
                            None,
                        ),
                    },
                    CompositionWitnessInput {
                        machine: "peer_comms".into(),
                        input_variant: "SubmitTypedPeerInput".into(),
                        fields: vec![CompositionWitnessField {
                            field: "raw_item_id".into(),
                            expr: Expr::String("raw_1".into()),
                        }],
                    },
                ],
                expected_routes: vec!["peer_candidate_enters_runtime_admission".into()],
                expected_scheduler_rules: vec![],
                expected_states: vec![
                    witness_state("peer_comms", Some("Delivered"), vec![]),
                    witness_state("runtime_control", Some("Idle"), vec![]),
                ],
                expected_transitions: vec![
                    witness_transition("runtime_control", "Initialize"),
                    witness_transition("peer_comms", "TrustPeer"),
                    witness_transition("peer_comms", "ReceiveTrustedPeerEnvelope"),
                    witness_transition("peer_comms", "SubmitTypedPeerInputDelivered"),
                ],
                expected_transition_order: vec![witness_transition_order(
                    "peer_comms",
                    "ReceiveTrustedPeerEnvelope",
                    "peer_comms",
                    "SubmitTypedPeerInputDelivered",
                )],
                state_limits: CompositionStateLimits {
                    step_limit: 5,
                    pending_input_limit: 4,
                    pending_route_limit: 2,
                    delivered_route_limit: 2,
                    emitted_effect_limit: 2,
                    seq_limit: 4,
                    set_limit: 4,
                    map_limit: 4,
                },
            },
            CompositionWitness {
                name: "admitted_peer_work_enters_ingress".into(),
                preload_inputs: vec![
                    CompositionWitnessInput {
                        machine: "runtime_control".into(),
                        input_variant: "Initialize".into(),
                        fields: vec![],
                    },
                    CompositionWitnessInput {
                        machine: "peer_comms".into(),
                        input_variant: "TrustPeer".into(),
                        fields: vec![CompositionWitnessField {
                            field: "peer_id".into(),
                            expr: Expr::String("peer_1".into()),
                        }],
                    },
                    CompositionWitnessInput {
                        machine: "peer_comms".into(),
                        input_variant: "ReceivePeerEnvelope".into(),
                        fields: peer_envelope_fields(
                            "raw_1",
                            "peer_1",
                            "request",
                            "inspect this inline image",
                            "InlineImage",
                            Some("req_1"),
                            Some("resv_1"),
                        ),
                    },
                    CompositionWitnessInput {
                        machine: "peer_comms".into(),
                        input_variant: "SubmitTypedPeerInput".into(),
                        fields: vec![CompositionWitnessField {
                            field: "raw_item_id".into(),
                            expr: Expr::String("raw_1".into()),
                        }],
                    },
                    CompositionWitnessInput {
                        machine: "runtime_control".into(),
                        input_variant: "AdmissionAccepted".into(),
                        fields: admission_accepted_fields(
                            "raw_1",
                            "PeerInput",
                            "InlineImage",
                            true,
                            true,
                        ),
                    },
                ],
                expected_routes: vec![
                    "peer_candidate_enters_runtime_admission".into(),
                    "admitted_peer_work_enters_ingress".into(),
                ],
                expected_scheduler_rules: vec![],
                expected_states: vec![
                    witness_state("runtime_control", Some("Idle"), vec![]),
                    witness_state("runtime_ingress", Some("Active"), vec![]),
                ],
                expected_transitions: vec![
                    witness_transition("peer_comms", "SubmitTypedPeerInputDelivered"),
                    witness_transition("runtime_control", "AdmissionAcceptedIdleSteer"),
                    witness_transition("runtime_ingress", "AdmitQueuedSteer"),
                ],
                expected_transition_order: vec![
                    witness_transition_order(
                        "peer_comms",
                        "SubmitTypedPeerInputDelivered",
                        "runtime_control",
                        "AdmissionAcceptedIdleSteer",
                    ),
                    witness_transition_order(
                        "runtime_control",
                        "AdmissionAcceptedIdleSteer",
                        "runtime_ingress",
                        "AdmitQueuedSteer",
                    ),
                ],
                state_limits: CompositionStateLimits {
                    step_limit: 6,
                    pending_input_limit: 5,
                    pending_route_limit: 2,
                    delivered_route_limit: 3,
                    emitted_effect_limit: 3,
                    seq_limit: 5,
                    set_limit: 5,
                    map_limit: 5,
                },
            },
            CompositionWitness {
                name: "control_preempts_peer_delivery".into(),
                preload_inputs: vec![
                    CompositionWitnessInput {
                        machine: "runtime_control".into(),
                        input_variant: "Initialize".into(),
                        fields: vec![],
                    },
                    CompositionWitnessInput {
                        machine: "peer_comms".into(),
                        input_variant: "TrustPeer".into(),
                        fields: vec![CompositionWitnessField {
                            field: "peer_id".into(),
                            expr: Expr::String("peer_1".into()),
                        }],
                    },
                    CompositionWitnessInput {
                        machine: "peer_comms".into(),
                        input_variant: "ReceivePeerEnvelope".into(),
                        fields: peer_envelope_fields(
                            "raw_1",
                            "peer_1",
                            "Message",
                            "priority handoff",
                            "TextOnly",
                            None,
                            None,
                        ),
                    },
                ],
                expected_routes: vec![],
                expected_scheduler_rules: vec![SchedulerRule::PreemptWhenReady {
                    higher: "control_plane".into(),
                    lower: "peer_plane".into(),
                }],
                expected_states: vec![witness_state("runtime_control", Some("Idle"), vec![])],
                expected_transitions: vec![
                    witness_transition("runtime_control", "Initialize"),
                    witness_transition("peer_comms", "TrustPeer"),
                ],
                expected_transition_order: vec![witness_transition_order(
                    "runtime_control",
                    "Initialize",
                    "peer_comms",
                    "TrustPeer",
                )],
                state_limits: CompositionStateLimits {
                    step_limit: 3,
                    pending_input_limit: 3,
                    pending_route_limit: 1,
                    delivered_route_limit: 1,
                    emitted_effect_limit: 1,
                    seq_limit: 3,
                    set_limit: 3,
                    map_limit: 3,
                },
            },
        ],
        deep_domain_cardinality: 2,
        deep_domain_overrides: BTreeMap::new(),
        witness_domain_cardinality: 1,
    }
}

pub fn ops_runtime_bundle_composition() -> CompositionSchema {
    CompositionSchema {
        name: "ops_runtime_bundle".into(),
        machines: vec![
            MachineInstance {
                instance_id: "ops_lifecycle".into(),
                machine_name: "OpsLifecycleMachine".into(),
                actor: "ops_plane".into(),
            },
            MachineInstance {
                instance_id: "runtime_control".into(),
                machine_name: "RuntimeControlMachine".into(),
                actor: "control_plane".into(),
            },
            MachineInstance {
                instance_id: "runtime_ingress".into(),
                machine_name: "RuntimeIngressMachine".into(),
                actor: "ordinary_ingress".into(),
            },
            MachineInstance {
                instance_id: "turn_execution".into(),
                machine_name: "TurnExecutionMachine".into(),
                actor: "turn_executor".into(),
            },
        ],
        entry_inputs: vec![
            EntryInput {
                name: "control_initialize".into(),
                machine: "runtime_control".into(),
                input_variant: "Initialize".into(),
            },
            EntryInput {
                name: "register_operation".into(),
                machine: "ops_lifecycle".into(),
                input_variant: "RegisterOperation".into(),
            },
            EntryInput {
                name: "provisioning_succeeded".into(),
                machine: "ops_lifecycle".into(),
                input_variant: "ProvisioningSucceeded".into(),
            },
            EntryInput {
                name: "provisioning_failed".into(),
                machine: "ops_lifecycle".into(),
                input_variant: "ProvisioningFailed".into(),
            },
            EntryInput {
                name: "peer_ready".into(),
                machine: "ops_lifecycle".into(),
                input_variant: "PeerReady".into(),
            },
            EntryInput {
                name: "register_watcher".into(),
                machine: "ops_lifecycle".into(),
                input_variant: "RegisterWatcher".into(),
            },
            EntryInput {
                name: "progress_reported".into(),
                machine: "ops_lifecycle".into(),
                input_variant: "ProgressReported".into(),
            },
            EntryInput {
                name: "complete_operation".into(),
                machine: "ops_lifecycle".into(),
                input_variant: "CompleteOperation".into(),
            },
            EntryInput {
                name: "fail_operation".into(),
                machine: "ops_lifecycle".into(),
                input_variant: "FailOperation".into(),
            },
            EntryInput {
                name: "cancel_operation".into(),
                machine: "ops_lifecycle".into(),
                input_variant: "CancelOperation".into(),
            },
            EntryInput {
                name: "retire_requested".into(),
                machine: "ops_lifecycle".into(),
                input_variant: "RetireRequested".into(),
            },
            EntryInput {
                name: "retire_completed".into(),
                machine: "ops_lifecycle".into(),
                input_variant: "RetireCompleted".into(),
            },
            EntryInput {
                name: "collect_terminal".into(),
                machine: "ops_lifecycle".into(),
                input_variant: "CollectTerminal".into(),
            },
            EntryInput {
                name: "runtime_admission_accepted".into(),
                machine: "runtime_control".into(),
                input_variant: "AdmissionAccepted".into(),
            },
            EntryInput {
                name: "ingress_stage_drain_snapshot".into(),
                machine: "runtime_ingress".into(),
                input_variant: "StageDrainSnapshot".into(),
            },
            EntryInput {
                name: "turn_primitive_applied".into(),
                machine: "turn_execution".into(),
                input_variant: "PrimitiveApplied".into(),
            },
            EntryInput {
                name: "turn_llm_returned_terminal".into(),
                machine: "turn_execution".into(),
                input_variant: "LlmReturnedTerminal".into(),
            },
            EntryInput {
                name: "turn_boundary_complete".into(),
                machine: "turn_execution".into(),
                input_variant: "BoundaryComplete".into(),
            },
            EntryInput {
                name: "turn_fatal_failure".into(),
                machine: "turn_execution".into(),
                input_variant: "FatalFailure".into(),
            },
            EntryInput {
                name: "turn_cancel_now".into(),
                machine: "turn_execution".into(),
                input_variant: "CancelNow".into(),
            },
            EntryInput {
                name: "turn_cancel_after_boundary".into(),
                machine: "turn_execution".into(),
                input_variant: "CancelAfterBoundary".into(),
            },
            EntryInput {
                name: "turn_cancellation_observed".into(),
                machine: "turn_execution".into(),
                input_variant: "CancellationObserved".into(),
            },
        ],
        routes: vec![
            Route {
                name: "op_event_enters_runtime_admission".into(),
                from_machine: "ops_lifecycle".into(),
                effect_variant: "SubmitOpEvent".into(),
                to: RouteTarget {
                    machine: "runtime_control".into(),
                    input_variant: "SubmitWork".into(),
                },
                bindings: vec![
                    RouteFieldBinding {
                        to_field: "work_id".into(),
                        source: RouteBindingSource::Field {
                            from_field: "operation_id".into(),
                            allow_named_alias: true,
                        },
                    },
                    RouteFieldBinding {
                        to_field: "handling_mode".into(),
                        source: RouteBindingSource::Literal(Expr::String("Steer".into())),
                    },
                ],
                delivery: RouteDelivery::Immediate,
            },
            Route {
                name: "admitted_op_work_enters_ingress".into(),
                from_machine: "runtime_control".into(),
                effect_variant: "SubmitAdmittedIngressEffect".into(),
                to: RouteTarget {
                    machine: "runtime_ingress".into(),
                    input_variant: "AdmitQueued".into(),
                },
                bindings: vec![
                    RouteFieldBinding {
                        to_field: "work_id".into(),
                        source: RouteBindingSource::Field {
                            from_field: "work_id".into(),
                            allow_named_alias: true,
                        },
                    },
                    RouteFieldBinding {
                        to_field: "handling_mode".into(),
                        source: RouteBindingSource::Field {
                            from_field: "handling_mode".into(),
                            allow_named_alias: false,
                        },
                    },
                    RouteFieldBinding {
                        to_field: "policy".into(),
                        source: RouteBindingSource::Literal(Expr::String("OperationQueued".into())),
                    },
                ],
                delivery: RouteDelivery::Immediate,
            },
            Route {
                name: "ingress_ready_starts_runtime_control".into(),
                from_machine: "runtime_ingress".into(),
                effect_variant: "ReadyForRun".into(),
                to: RouteTarget {
                    machine: "runtime_control".into(),
                    input_variant: "BeginRun".into(),
                },
                bindings: vec![RouteFieldBinding {
                    to_field: "run_id".into(),
                    source: RouteBindingSource::Field {
                        from_field: "run_id".into(),
                        allow_named_alias: false,
                    },
                }],
                delivery: RouteDelivery::Immediate,
            },
            Route {
                name: "runtime_control_starts_execution".into(),
                from_machine: "runtime_control".into(),
                effect_variant: "SubmitRunPrimitive".into(),
                to: RouteTarget {
                    machine: "turn_execution".into(),
                    input_variant: "StartConversationRun".into(),
                },
                bindings: vec![RouteFieldBinding {
                    to_field: "run_id".into(),
                    source: RouteBindingSource::Field {
                        from_field: "run_id".into(),
                        allow_named_alias: false,
                    },
                }],
                delivery: RouteDelivery::Immediate,
            },
            Route {
                name: "execution_boundary_updates_ingress".into(),
                from_machine: "turn_execution".into(),
                effect_variant: "BoundaryApplied".into(),
                to: RouteTarget {
                    machine: "runtime_ingress".into(),
                    input_variant: "BoundaryApplied".into(),
                },
                bindings: vec![
                    RouteFieldBinding {
                        to_field: "run_id".into(),
                        source: RouteBindingSource::Field {
                            from_field: "run_id".into(),
                            allow_named_alias: false,
                        },
                    },
                    RouteFieldBinding {
                        to_field: "boundary_sequence".into(),
                        source: RouteBindingSource::Field {
                            from_field: "boundary_sequence".into(),
                            allow_named_alias: false,
                        },
                    },
                ],
                delivery: RouteDelivery::Immediate,
            },
            Route {
                name: "execution_completion_updates_ingress".into(),
                from_machine: "turn_execution".into(),
                effect_variant: "RunCompleted".into(),
                to: RouteTarget {
                    machine: "runtime_ingress".into(),
                    input_variant: "RunCompleted".into(),
                },
                bindings: vec![RouteFieldBinding {
                    to_field: "run_id".into(),
                    source: RouteBindingSource::Field {
                        from_field: "run_id".into(),
                        allow_named_alias: false,
                    },
                }],
                delivery: RouteDelivery::Immediate,
            },
            Route {
                name: "execution_completion_notifies_control".into(),
                from_machine: "turn_execution".into(),
                effect_variant: "RunCompleted".into(),
                to: RouteTarget {
                    machine: "runtime_control".into(),
                    input_variant: "RunCompleted".into(),
                },
                bindings: vec![RouteFieldBinding {
                    to_field: "run_id".into(),
                    source: RouteBindingSource::Field {
                        from_field: "run_id".into(),
                        allow_named_alias: false,
                    },
                }],
                delivery: RouteDelivery::Immediate,
            },
            Route {
                name: "execution_failure_updates_ingress".into(),
                from_machine: "turn_execution".into(),
                effect_variant: "RunFailed".into(),
                to: RouteTarget {
                    machine: "runtime_ingress".into(),
                    input_variant: "RunFailed".into(),
                },
                bindings: vec![RouteFieldBinding {
                    to_field: "run_id".into(),
                    source: RouteBindingSource::Field {
                        from_field: "run_id".into(),
                        allow_named_alias: false,
                    },
                }],
                delivery: RouteDelivery::Immediate,
            },
            Route {
                name: "execution_failure_notifies_control".into(),
                from_machine: "turn_execution".into(),
                effect_variant: "RunFailed".into(),
                to: RouteTarget {
                    machine: "runtime_control".into(),
                    input_variant: "RunFailed".into(),
                },
                bindings: vec![RouteFieldBinding {
                    to_field: "run_id".into(),
                    source: RouteBindingSource::Field {
                        from_field: "run_id".into(),
                        allow_named_alias: false,
                    },
                }],
                delivery: RouteDelivery::Immediate,
            },
            Route {
                name: "execution_cancel_updates_ingress".into(),
                from_machine: "turn_execution".into(),
                effect_variant: "RunCancelled".into(),
                to: RouteTarget {
                    machine: "runtime_ingress".into(),
                    input_variant: "RunCancelled".into(),
                },
                bindings: vec![RouteFieldBinding {
                    to_field: "run_id".into(),
                    source: RouteBindingSource::Field {
                        from_field: "run_id".into(),
                        allow_named_alias: false,
                    },
                }],
                delivery: RouteDelivery::Immediate,
            },
            Route {
                name: "execution_cancel_notifies_control".into(),
                from_machine: "turn_execution".into(),
                effect_variant: "RunCancelled".into(),
                to: RouteTarget {
                    machine: "runtime_control".into(),
                    input_variant: "RunCancelled".into(),
                },
                bindings: vec![RouteFieldBinding {
                    to_field: "run_id".into(),
                    source: RouteBindingSource::Field {
                        from_field: "run_id".into(),
                        allow_named_alias: false,
                    },
                }],
                delivery: RouteDelivery::Immediate,
            },
        ],
        actor_priorities: vec![ActorPriority {
            higher: "control_plane".into(),
            lower: "ordinary_ingress".into(),
            reason: "runtime control preempts ordinary ingress while async-op work is being routed".into(),
        }],
        scheduler_rules: vec![SchedulerRule::PreemptWhenReady {
            higher: "control_plane".into(),
            lower: "ordinary_ingress".into(),
        }],
        invariants: vec![
            CompositionInvariant {
                name: "async_op_events_reenter_runtime_via_operation_input".into(),
                kind: CompositionInvariantKind::ObservedInputOriginatesFromEffect {
                    to_machine: "runtime_control".into(),
                    input_variant: "SubmitWork".into(),
                    from_machine: "ops_lifecycle".into(),
                    effect_variant: "SubmitOpEvent".into(),
                },
                statement:
                    "async-operation lifecycle events re-enter runtime through the canonical operation-input admission surface"
                        .into(),
                references_machines: vec!["ops_lifecycle".into(), "runtime_control".into()],
                references_actors: vec!["ops_plane".into(), "control_plane".into()],
            },
            CompositionInvariant {
                name: "admitted_op_work_flows_into_ingress".into(),
                kind: CompositionInvariantKind::ObservedInputOriginatesFromEffect {
                    to_machine: "runtime_ingress".into(),
                    input_variant: "AdmitQueued".into(),
                    from_machine: "runtime_control".into(),
                    effect_variant: "SubmitAdmittedIngressEffect".into(),
                },
                statement:
                    "runtime-admitted operation work is handed into canonical ingress ownership"
                        .into(),
                references_machines: vec![
                    "ops_lifecycle".into(),
                    "runtime_control".into(),
                    "runtime_ingress".into(),
                ],
                references_actors: vec![
                    "ops_plane".into(),
                    "control_plane".into(),
                    "ordinary_ingress".into(),
                ],
            },
            CompositionInvariant {
                name: "op_execution_failure_is_handled".into(),
                kind: CompositionInvariantKind::OutcomeHandled {
                    from_machine: "turn_execution".into(),
                    effect_variant: "RunFailed".into(),
                    required_targets: vec![
                        RouteTarget {
                            machine: "runtime_ingress".into(),
                            input_variant: "RunFailed".into(),
                        },
                        RouteTarget {
                            machine: "runtime_control".into(),
                            input_variant: "RunFailed".into(),
                        },
                    ],
                },
                statement:
                    "turn-execution failure after operation admission is handled by both ingress and runtime control"
                        .into(),
                references_machines: vec![
                    "turn_execution".into(),
                    "runtime_ingress".into(),
                    "runtime_control".into(),
                ],
                references_actors: vec![
                    "turn_executor".into(),
                    "ordinary_ingress".into(),
                    "control_plane".into(),
                ],
            },
            CompositionInvariant {
                name: "op_execution_cancel_is_handled".into(),
                kind: CompositionInvariantKind::OutcomeHandled {
                    from_machine: "turn_execution".into(),
                    effect_variant: "RunCancelled".into(),
                    required_targets: vec![
                        RouteTarget {
                            machine: "runtime_ingress".into(),
                            input_variant: "RunCancelled".into(),
                        },
                        RouteTarget {
                            machine: "runtime_control".into(),
                            input_variant: "RunCancelled".into(),
                        },
                    ],
                },
                statement:
                    "turn-execution cancellation after operation admission is handled by both ingress and runtime control"
                        .into(),
                references_machines: vec![
                    "turn_execution".into(),
                    "runtime_ingress".into(),
                    "runtime_control".into(),
                ],
                references_actors: vec![
                    "turn_executor".into(),
                    "ordinary_ingress".into(),
                    "control_plane".into(),
                ],
            },
        ],
        witnesses: vec![
            CompositionWitness {
                name: "op_success_path".into(),
                preload_inputs: vec![
                    CompositionWitnessInput {
                        machine: "runtime_control".into(),
                        input_variant: "Initialize".into(),
                        fields: vec![],
                    },
                    CompositionWitnessInput {
                        machine: "ops_lifecycle".into(),
                        input_variant: "RegisterOperation".into(),
                        fields: vec![
                            CompositionWitnessField {
                                field: "operation_id".into(),
                                expr: Expr::String("op_1".into()),
                            },
                            CompositionWitnessField {
                                field: "operation_kind".into(),
                                expr: Expr::String("BackgroundToolOp".into()),
                            },
                        ],
                    },
                    CompositionWitnessInput {
                        machine: "ops_lifecycle".into(),
                        input_variant: "ProvisioningSucceeded".into(),
                        fields: vec![CompositionWitnessField {
                            field: "operation_id".into(),
                            expr: Expr::String("op_1".into()),
                        }],
                    },
                    CompositionWitnessInput {
                        machine: "runtime_control".into(),
                        input_variant: "AdmissionAccepted".into(),
                        fields: admission_accepted_fields(
                            "op_1",
                            "OperationInput",
                            "TextOnly",
                            true,
                            true,
                        ),
                    },
                    CompositionWitnessInput {
                        machine: "runtime_ingress".into(),
                        input_variant: "StageDrainSnapshot".into(),
                        fields: vec![
                            CompositionWitnessField {
                                field: "run_id".into(),
                                expr: Expr::String("runid_1".into()),
                            },
                            CompositionWitnessField {
                                field: "contributing_work_ids".into(),
                                expr: Expr::SeqLiteral(vec![Expr::String("op_1".into())]),
                            },
                        ],
                    },
                    CompositionWitnessInput {
                        machine: "turn_execution".into(),
                        input_variant: "PrimitiveApplied".into(),
                        fields: primitive_applied_fields(
                            "runid_1",
                            "TextOnly",
                            false,
                            false,
                        ),
                    },
                    CompositionWitnessInput {
                        machine: "turn_execution".into(),
                        input_variant: "LlmReturnedTerminal".into(),
                        fields: vec![CompositionWitnessField {
                            field: "run_id".into(),
                            expr: Expr::String("runid_1".into()),
                        }],
                    },
                    CompositionWitnessInput {
                        machine: "turn_execution".into(),
                        input_variant: "BoundaryComplete".into(),
                        fields: vec![CompositionWitnessField {
                            field: "run_id".into(),
                            expr: Expr::String("runid_1".into()),
                        }],
                    },
                ],
                expected_routes: vec![
                    "op_event_enters_runtime_admission".into(),
                    "admitted_op_work_enters_ingress".into(),
                    "ingress_ready_starts_runtime_control".into(),
                    "runtime_control_starts_execution".into(),
                    "execution_boundary_updates_ingress".into(),
                    "execution_completion_updates_ingress".into(),
                    "execution_completion_notifies_control".into(),
                ],
                expected_scheduler_rules: vec![],
                expected_states: vec![
                    witness_state("ops_lifecycle", Some("Active"), vec![]),
                    witness_state("runtime_control", Some("Idle"), vec![]),
                    witness_state("runtime_ingress", Some("Active"), vec![]),
                    witness_state("turn_execution", Some("Completed"), vec![]),
                ],
                expected_transitions: vec![
                    witness_transition("ops_lifecycle", "RegisterOperation"),
                    witness_transition("ops_lifecycle", "ProvisioningSucceeded"),
                    witness_transition("runtime_control", "AdmissionAcceptedIdleSteer"),
                    witness_transition("runtime_ingress", "AdmitQueuedSteer"),
                    witness_transition("runtime_ingress", "StageDrainSnapshot"),
                    witness_transition("runtime_control", "BeginRunFromIdle"),
                    witness_transition("turn_execution", "StartConversationRun"),
                    witness_transition("turn_execution", "BoundaryComplete"),
                    witness_transition("runtime_ingress", "RunCompleted"),
                    witness_transition("runtime_control", "RunCompleted"),
                ],
                expected_transition_order: vec![
                    witness_transition_order(
                        "ops_lifecycle",
                        "ProvisioningSucceeded",
                        "runtime_control",
                        "AdmissionAcceptedIdleSteer",
                    ),
                    witness_transition_order(
                        "runtime_control",
                        "AdmissionAcceptedIdleSteer",
                        "runtime_ingress",
                        "AdmitQueuedSteer",
                    ),
                    witness_transition_order(
                        "runtime_ingress",
                        "StageDrainSnapshot",
                        "runtime_control",
                        "BeginRunFromIdle",
                    ),
                    witness_transition_order(
                        "runtime_control",
                        "BeginRunFromIdle",
                        "turn_execution",
                        "StartConversationRun",
                    ),
                ],
                state_limits: CompositionStateLimits {
                    step_limit: 10,
                    pending_input_limit: 7,
                    pending_route_limit: 2,
                    delivered_route_limit: 7,
                    emitted_effect_limit: 7,
                    seq_limit: 7,
                    set_limit: 7,
                    map_limit: 3,
                },
            },
            CompositionWitness {
                name: "op_failure_path".into(),
                preload_inputs: vec![
                    CompositionWitnessInput {
                        machine: "runtime_control".into(),
                        input_variant: "Initialize".into(),
                        fields: vec![],
                    },
                    CompositionWitnessInput {
                        machine: "ops_lifecycle".into(),
                        input_variant: "RegisterOperation".into(),
                        fields: vec![
                            CompositionWitnessField {
                                field: "operation_id".into(),
                                expr: Expr::String("op_1".into()),
                            },
                            CompositionWitnessField {
                                field: "operation_kind".into(),
                                expr: Expr::String("BackgroundToolOp".into()),
                            },
                        ],
                    },
                    CompositionWitnessInput {
                        machine: "ops_lifecycle".into(),
                        input_variant: "ProvisioningSucceeded".into(),
                        fields: vec![CompositionWitnessField {
                            field: "operation_id".into(),
                            expr: Expr::String("op_1".into()),
                        }],
                    },
                    CompositionWitnessInput {
                        machine: "runtime_control".into(),
                        input_variant: "AdmissionAccepted".into(),
                        fields: admission_accepted_fields(
                            "op_1",
                            "OperationInput",
                            "TextOnly",
                            true,
                            true,
                        ),
                    },
                    CompositionWitnessInput {
                        machine: "runtime_ingress".into(),
                        input_variant: "StageDrainSnapshot".into(),
                        fields: vec![
                            CompositionWitnessField {
                                field: "run_id".into(),
                                expr: Expr::String("runid_1".into()),
                            },
                            CompositionWitnessField {
                                field: "contributing_work_ids".into(),
                                expr: Expr::SeqLiteral(vec![Expr::String("op_1".into())]),
                            },
                        ],
                    },
                    CompositionWitnessInput {
                        machine: "turn_execution".into(),
                        input_variant: "FatalFailure".into(),
                        fields: vec![CompositionWitnessField {
                            field: "run_id".into(),
                            expr: Expr::String("runid_1".into()),
                        }],
                    },
                ],
                expected_routes: vec![
                    "op_event_enters_runtime_admission".into(),
                    "admitted_op_work_enters_ingress".into(),
                    "ingress_ready_starts_runtime_control".into(),
                    "runtime_control_starts_execution".into(),
                    "execution_failure_updates_ingress".into(),
                    "execution_failure_notifies_control".into(),
                ],
                expected_scheduler_rules: vec![],
                expected_states: vec![
                    witness_state("runtime_control", Some("Idle"), vec![]),
                    witness_state("runtime_ingress", Some("Active"), vec![]),
                    witness_state("turn_execution", Some("Failed"), vec![]),
                ],
                expected_transitions: vec![
                    witness_transition("ops_lifecycle", "ProvisioningSucceeded"),
                    witness_transition("runtime_control", "AdmissionAcceptedIdleSteer"),
                    witness_transition("runtime_ingress", "AdmitQueuedSteer"),
                    witness_transition("runtime_ingress", "StageDrainSnapshot"),
                    witness_transition("runtime_control", "BeginRunFromIdle"),
                    witness_transition("turn_execution", "StartConversationRun"),
                    witness_transition("turn_execution", "FatalFailureFromApplyingPrimitive"),
                    witness_transition("runtime_ingress", "RunFailed"),
                    witness_transition("runtime_control", "RunFailed"),
                ],
                expected_transition_order: vec![
                    witness_transition_order(
                        "runtime_control",
                        "BeginRunFromIdle",
                        "turn_execution",
                        "StartConversationRun",
                    ),
                    witness_transition_order(
                        "turn_execution",
                        "FatalFailureFromApplyingPrimitive",
                        "runtime_ingress",
                        "RunFailed",
                    ),
                    witness_transition_order(
                        "turn_execution",
                        "FatalFailureFromApplyingPrimitive",
                        "runtime_control",
                        "RunFailed",
                    ),
                ],
                state_limits: CompositionStateLimits {
                    step_limit: 8,
                    pending_input_limit: 6,
                    pending_route_limit: 2,
                    delivered_route_limit: 6,
                    emitted_effect_limit: 6,
                    seq_limit: 6,
                    set_limit: 6,
                    map_limit: 3,
                },
            },
            CompositionWitness {
                name: "op_cancel_path".into(),
                preload_inputs: vec![
                    CompositionWitnessInput {
                        machine: "runtime_control".into(),
                        input_variant: "Initialize".into(),
                        fields: vec![],
                    },
                    CompositionWitnessInput {
                        machine: "ops_lifecycle".into(),
                        input_variant: "RegisterOperation".into(),
                        fields: vec![
                            CompositionWitnessField {
                                field: "operation_id".into(),
                                expr: Expr::String("op_1".into()),
                            },
                            CompositionWitnessField {
                                field: "operation_kind".into(),
                                expr: Expr::String("BackgroundToolOp".into()),
                            },
                        ],
                    },
                    CompositionWitnessInput {
                        machine: "ops_lifecycle".into(),
                        input_variant: "ProvisioningSucceeded".into(),
                        fields: vec![CompositionWitnessField {
                            field: "operation_id".into(),
                            expr: Expr::String("op_1".into()),
                        }],
                    },
                    CompositionWitnessInput {
                        machine: "runtime_control".into(),
                        input_variant: "AdmissionAccepted".into(),
                        fields: admission_accepted_fields(
                            "op_1",
                            "OperationInput",
                            "TextOnly",
                            true,
                            true,
                        ),
                    },
                    CompositionWitnessInput {
                        machine: "runtime_ingress".into(),
                        input_variant: "StageDrainSnapshot".into(),
                        fields: vec![
                            CompositionWitnessField {
                                field: "run_id".into(),
                                expr: Expr::String("runid_1".into()),
                            },
                            CompositionWitnessField {
                                field: "contributing_work_ids".into(),
                                expr: Expr::SeqLiteral(vec![Expr::String("op_1".into())]),
                            },
                        ],
                    },
                    CompositionWitnessInput {
                        machine: "turn_execution".into(),
                        input_variant: "CancelNow".into(),
                        fields: vec![CompositionWitnessField {
                            field: "run_id".into(),
                            expr: Expr::String("runid_1".into()),
                        }],
                    },
                    CompositionWitnessInput {
                        machine: "turn_execution".into(),
                        input_variant: "CancellationObserved".into(),
                        fields: vec![CompositionWitnessField {
                            field: "run_id".into(),
                            expr: Expr::String("runid_1".into()),
                        }],
                    },
                ],
                expected_routes: vec![
                    "op_event_enters_runtime_admission".into(),
                    "admitted_op_work_enters_ingress".into(),
                    "ingress_ready_starts_runtime_control".into(),
                    "runtime_control_starts_execution".into(),
                    "execution_cancel_updates_ingress".into(),
                    "execution_cancel_notifies_control".into(),
                ],
                expected_scheduler_rules: vec![],
                expected_states: vec![
                    witness_state("runtime_control", Some("Idle"), vec![]),
                    witness_state("runtime_ingress", Some("Active"), vec![]),
                    witness_state("turn_execution", Some("Cancelled"), vec![]),
                ],
                expected_transitions: vec![
                    witness_transition("ops_lifecycle", "ProvisioningSucceeded"),
                    witness_transition("runtime_control", "AdmissionAcceptedIdleSteer"),
                    witness_transition("runtime_ingress", "AdmitQueuedSteer"),
                    witness_transition("runtime_ingress", "StageDrainSnapshot"),
                    witness_transition("runtime_control", "BeginRunFromIdle"),
                    witness_transition("turn_execution", "StartConversationRun"),
                    witness_transition("turn_execution", "CancelNowFromApplyingPrimitive"),
                    witness_transition("turn_execution", "CancellationObserved"),
                    witness_transition("runtime_ingress", "RunCancelled"),
                    witness_transition("runtime_control", "RunCancelled"),
                ],
                expected_transition_order: vec![
                    witness_transition_order(
                        "runtime_control",
                        "BeginRunFromIdle",
                        "turn_execution",
                        "StartConversationRun",
                    ),
                    witness_transition_order(
                        "turn_execution",
                        "CancellationObserved",
                        "runtime_ingress",
                        "RunCancelled",
                    ),
                    witness_transition_order(
                        "turn_execution",
                        "CancellationObserved",
                        "runtime_control",
                        "RunCancelled",
                    ),
                ],
                state_limits: CompositionStateLimits {
                    step_limit: 9,
                    pending_input_limit: 6,
                    pending_route_limit: 2,
                    delivered_route_limit: 6,
                    emitted_effect_limit: 6,
                    seq_limit: 6,
                    set_limit: 6,
                    map_limit: 3,
                },
            },
            CompositionWitness {
                name: "control_preempts_op_ingress".into(),
                preload_inputs: vec![
                    CompositionWitnessInput {
                        machine: "runtime_control".into(),
                        input_variant: "Initialize".into(),
                        fields: vec![],
                    },
                    CompositionWitnessInput {
                        machine: "runtime_control".into(),
                        input_variant: "AdmissionAccepted".into(),
                        fields: admission_accepted_fields(
                            "op_1",
                            "OperationInput",
                            "TextOnly",
                            true,
                            true,
                        ),
                    },
                    CompositionWitnessInput {
                        machine: "runtime_control".into(),
                        input_variant: "AdmissionAccepted".into(),
                        fields: admission_accepted_fields(
                            "op_2",
                            "OperationInput",
                            "TextOnly",
                            true,
                            true,
                        ),
                    },
                ],
                expected_routes: vec!["admitted_op_work_enters_ingress".into()],
                expected_scheduler_rules: vec![SchedulerRule::PreemptWhenReady {
                    higher: "control_plane".into(),
                    lower: "ordinary_ingress".into(),
                }],
                expected_states: vec![witness_state("runtime_control", Some("Idle"), vec![])],
                expected_transitions: vec![
                    witness_transition("runtime_control", "Initialize"),
                    witness_transition("runtime_control", "AdmissionAcceptedIdleSteer"),
                    witness_transition("runtime_ingress", "AdmitQueuedSteer"),
                ],
                expected_transition_order: vec![
                    witness_transition_order(
                        "runtime_control",
                        "Initialize",
                        "runtime_control",
                        "AdmissionAcceptedIdleSteer",
                    ),
                    witness_transition_order(
                        "runtime_control",
                        "AdmissionAcceptedIdleSteer",
                        "runtime_ingress",
                        "AdmitQueuedSteer",
                    ),
                ],
                state_limits: CompositionStateLimits {
                    step_limit: 6,
                    pending_input_limit: 3,
                    pending_route_limit: 1,
                    delivered_route_limit: 2,
                    emitted_effect_limit: 5,
                    seq_limit: 4,
                    set_limit: 5,
                    map_limit: 2,
                },
            },
        ],
        deep_domain_cardinality: 2,
        deep_domain_overrides: BTreeMap::new(),
        witness_domain_cardinality: 1,
    }
}

pub fn surface_event_runtime_bundle_composition() -> CompositionSchema {
    CompositionSchema {
        name: "surface_event_runtime_bundle".into(),
        machines: vec![
            MachineInstance {
                instance_id: "runtime_control".into(),
                machine_name: "RuntimeControlMachine".into(),
                actor: "control_plane".into(),
            },
            MachineInstance {
                instance_id: "runtime_ingress".into(),
                machine_name: "RuntimeIngressMachine".into(),
                actor: "ordinary_ingress".into(),
            },
            MachineInstance {
                instance_id: "turn_execution".into(),
                machine_name: "TurnExecutionMachine".into(),
                actor: "turn_executor".into(),
            },
        ],
        entry_inputs: vec![
            EntryInput {
                name: "control_initialize".into(),
                machine: "runtime_control".into(),
                input_variant: "Initialize".into(),
            },
            EntryInput {
                name: "surface_submit_candidate".into(),
                machine: "runtime_control".into(),
                input_variant: "SubmitWork".into(),
            },
            EntryInput {
                name: "runtime_admission_accepted".into(),
                machine: "runtime_control".into(),
                input_variant: "AdmissionAccepted".into(),
            },
            EntryInput {
                name: "ingress_stage_drain_snapshot".into(),
                machine: "runtime_ingress".into(),
                input_variant: "StageDrainSnapshot".into(),
            },
            EntryInput {
                name: "turn_primitive_applied".into(),
                machine: "turn_execution".into(),
                input_variant: "PrimitiveApplied".into(),
            },
            EntryInput {
                name: "turn_llm_returned_terminal".into(),
                machine: "turn_execution".into(),
                input_variant: "LlmReturnedTerminal".into(),
            },
            EntryInput {
                name: "turn_boundary_complete".into(),
                machine: "turn_execution".into(),
                input_variant: "BoundaryComplete".into(),
            },
            EntryInput {
                name: "turn_fatal_failure".into(),
                machine: "turn_execution".into(),
                input_variant: "FatalFailure".into(),
            },
        ],
        routes: vec![
            Route {
                name: "surface_event_enters_ingress".into(),
                from_machine: "runtime_control".into(),
                effect_variant: "SubmitAdmittedIngressEffect".into(),
                to: RouteTarget {
                    machine: "runtime_ingress".into(),
                    input_variant: "AdmitQueued".into(),
                },
                bindings: vec![
                    RouteFieldBinding {
                        to_field: "work_id".into(),
                        source: RouteBindingSource::Field {
                            from_field: "work_id".into(),
                            allow_named_alias: true,
                        },
                    },
                    RouteFieldBinding {
                        to_field: "content_shape".into(),
                        source: RouteBindingSource::Field {
                            from_field: "content_shape".into(),
                            allow_named_alias: false,
                        },
                    },
                    RouteFieldBinding {
                        to_field: "handling_mode".into(),
                        source: RouteBindingSource::Field {
                            from_field: "handling_mode".into(),
                            allow_named_alias: false,
                        },
                    },
                    RouteFieldBinding {
                        to_field: "request_id".into(),
                        source: RouteBindingSource::Field {
                            from_field: "request_id".into(),
                            allow_named_alias: false,
                        },
                    },
                    RouteFieldBinding {
                        to_field: "reservation_key".into(),
                        source: RouteBindingSource::Field {
                            from_field: "reservation_key".into(),
                            allow_named_alias: false,
                        },
                    },
                    RouteFieldBinding {
                        to_field: "policy".into(),
                        source: RouteBindingSource::Literal(Expr::String(
                            "ExternalEventQueued".into(),
                        )),
                    },
                ],
                delivery: RouteDelivery::Immediate,
            },
            Route {
                name: "surface_ingress_ready_starts_runtime_control".into(),
                from_machine: "runtime_ingress".into(),
                effect_variant: "ReadyForRun".into(),
                to: RouteTarget {
                    machine: "runtime_control".into(),
                    input_variant: "BeginRun".into(),
                },
                bindings: vec![RouteFieldBinding {
                    to_field: "run_id".into(),
                    source: RouteBindingSource::Field {
                        from_field: "run_id".into(),
                        allow_named_alias: false,
                    },
                }],
                delivery: RouteDelivery::Immediate,
            },
            Route {
                name: "surface_runtime_control_starts_execution".into(),
                from_machine: "runtime_control".into(),
                effect_variant: "SubmitRunPrimitive".into(),
                to: RouteTarget {
                    machine: "turn_execution".into(),
                    input_variant: "StartConversationRun".into(),
                },
                bindings: vec![RouteFieldBinding {
                    to_field: "run_id".into(),
                    source: RouteBindingSource::Field {
                        from_field: "run_id".into(),
                        allow_named_alias: false,
                    },
                }],
                delivery: RouteDelivery::Immediate,
            },
            Route {
                name: "surface_execution_boundary_updates_ingress".into(),
                from_machine: "turn_execution".into(),
                effect_variant: "BoundaryApplied".into(),
                to: RouteTarget {
                    machine: "runtime_ingress".into(),
                    input_variant: "BoundaryApplied".into(),
                },
                bindings: vec![
                    RouteFieldBinding {
                        to_field: "run_id".into(),
                        source: RouteBindingSource::Field {
                            from_field: "run_id".into(),
                            allow_named_alias: false,
                        },
                    },
                    RouteFieldBinding {
                        to_field: "boundary_sequence".into(),
                        source: RouteBindingSource::Field {
                            from_field: "boundary_sequence".into(),
                            allow_named_alias: false,
                        },
                    },
                ],
                delivery: RouteDelivery::Immediate,
            },
            Route {
                name: "surface_execution_completion_updates_ingress".into(),
                from_machine: "turn_execution".into(),
                effect_variant: "RunCompleted".into(),
                to: RouteTarget {
                    machine: "runtime_ingress".into(),
                    input_variant: "RunCompleted".into(),
                },
                bindings: vec![RouteFieldBinding {
                    to_field: "run_id".into(),
                    source: RouteBindingSource::Field {
                        from_field: "run_id".into(),
                        allow_named_alias: false,
                    },
                }],
                delivery: RouteDelivery::Immediate,
            },
            Route {
                name: "surface_execution_completion_notifies_control".into(),
                from_machine: "turn_execution".into(),
                effect_variant: "RunCompleted".into(),
                to: RouteTarget {
                    machine: "runtime_control".into(),
                    input_variant: "RunCompleted".into(),
                },
                bindings: vec![RouteFieldBinding {
                    to_field: "run_id".into(),
                    source: RouteBindingSource::Field {
                        from_field: "run_id".into(),
                        allow_named_alias: false,
                    },
                }],
                delivery: RouteDelivery::Immediate,
            },
            Route {
                name: "surface_execution_failure_updates_ingress".into(),
                from_machine: "turn_execution".into(),
                effect_variant: "RunFailed".into(),
                to: RouteTarget {
                    machine: "runtime_ingress".into(),
                    input_variant: "RunFailed".into(),
                },
                bindings: vec![RouteFieldBinding {
                    to_field: "run_id".into(),
                    source: RouteBindingSource::Field {
                        from_field: "run_id".into(),
                        allow_named_alias: false,
                    },
                }],
                delivery: RouteDelivery::Immediate,
            },
            Route {
                name: "surface_execution_failure_notifies_control".into(),
                from_machine: "turn_execution".into(),
                effect_variant: "RunFailed".into(),
                to: RouteTarget {
                    machine: "runtime_control".into(),
                    input_variant: "RunFailed".into(),
                },
                bindings: vec![RouteFieldBinding {
                    to_field: "run_id".into(),
                    source: RouteBindingSource::Field {
                        from_field: "run_id".into(),
                        allow_named_alias: false,
                    },
                }],
                delivery: RouteDelivery::Immediate,
            },
        ],
        actor_priorities: vec![ActorPriority {
            higher: "control_plane".into(),
            lower: "ordinary_ingress".into(),
            reason: "runtime control preempts surface event ingress work".into(),
        }],
        scheduler_rules: vec![SchedulerRule::PreemptWhenReady {
            higher: "control_plane".into(),
            lower: "ordinary_ingress".into(),
        }],
        invariants: vec![
            CompositionInvariant {
                name: "surface_event_uses_canonical_runtime_admission".into(),
                kind: CompositionInvariantKind::ObservedInputOriginatesFromEffect {
                    to_machine: "runtime_ingress".into(),
                    input_variant: "AdmitQueued".into(),
                    from_machine: "runtime_control".into(),
                    effect_variant: "SubmitAdmittedIngressEffect".into(),
                },
                statement:
                    "surface external events enter runtime ingress only through runtime-control admission".into(),
                references_machines: vec!["runtime_control".into(), "runtime_ingress".into()],
                references_actors: vec!["control_plane".into(), "ordinary_ingress".into()],
            },
            CompositionInvariant {
                name: "surface_event_begin_run_requires_staged_drain".into(),
                kind: CompositionInvariantKind::ObservedInputOriginatesFromEffect {
                    to_machine: "runtime_control".into(),
                    input_variant: "BeginRun".into(),
                    from_machine: "runtime_ingress".into(),
                    effect_variant: "ReadyForRun".into(),
                },
                statement:
                    "surface-originated work begins a run only after ingress-owned staging".into(),
                references_machines: vec!["runtime_control".into(), "runtime_ingress".into()],
                references_actors: vec!["control_plane".into(), "ordinary_ingress".into()],
            },
            CompositionInvariant {
                name: "surface_event_execution_completion_is_handled".into(),
                kind: CompositionInvariantKind::OutcomeHandled {
                    from_machine: "turn_execution".into(),
                    effect_variant: "RunCompleted".into(),
                    required_targets: vec![
                        RouteTarget {
                            machine: "runtime_ingress".into(),
                            input_variant: "RunCompleted".into(),
                        },
                        RouteTarget {
                            machine: "runtime_control".into(),
                            input_variant: "RunCompleted".into(),
                        },
                    ],
                },
                statement:
                    "surface-originated execution completion updates both ingress and runtime control".into(),
                references_machines: vec![
                    "turn_execution".into(),
                    "runtime_ingress".into(),
                    "runtime_control".into(),
                ],
                references_actors: vec![
                    "turn_executor".into(),
                    "ordinary_ingress".into(),
                    "control_plane".into(),
                ],
            },
            CompositionInvariant {
                name: "surface_event_execution_failure_is_handled".into(),
                kind: CompositionInvariantKind::OutcomeHandled {
                    from_machine: "turn_execution".into(),
                    effect_variant: "RunFailed".into(),
                    required_targets: vec![
                        RouteTarget {
                            machine: "runtime_ingress".into(),
                            input_variant: "RunFailed".into(),
                        },
                        RouteTarget {
                            machine: "runtime_control".into(),
                            input_variant: "RunFailed".into(),
                        },
                    ],
                },
                statement:
                    "surface-originated execution failure updates both ingress and runtime control".into(),
                references_machines: vec![
                    "turn_execution".into(),
                    "runtime_ingress".into(),
                    "runtime_control".into(),
                ],
                references_actors: vec![
                    "turn_executor".into(),
                    "ordinary_ingress".into(),
                    "control_plane".into(),
                ],
            },
            CompositionInvariant {
                name: "control_preempts_surface_event_ingress".into(),
                kind: CompositionInvariantKind::SchedulerRulePresent {
                    rule: SchedulerRule::PreemptWhenReady {
                        higher: "control_plane".into(),
                        lower: "ordinary_ingress".into(),
                    },
                },
                statement: "runtime control outranks surface event ingress when both are ready".into(),
                references_machines: vec!["runtime_control".into(), "runtime_ingress".into()],
                references_actors: vec!["control_plane".into(), "ordinary_ingress".into()],
            },
        ],
        witnesses: vec![
            CompositionWitness {
                name: "surface_event_success_path".into(),
                preload_inputs: vec![
                    CompositionWitnessInput {
                        machine: "runtime_control".into(),
                        input_variant: "Initialize".into(),
                        fields: vec![],
                    },
                    CompositionWitnessInput {
                        machine: "runtime_control".into(),
                        input_variant: "SubmitWork".into(),
                        fields: submit_candidate_fields(
                            "external_evt_1",
                            "WorkInput",
                            "TextOnly",
                        ),
                    },
                    CompositionWitnessInput {
                        machine: "runtime_control".into(),
                        input_variant: "AdmissionAccepted".into(),
                        fields: admission_accepted_fields(
                            "external_evt_1",
                            "WorkInput",
                            "TextOnly",
                            true,
                            false,
                        ),
                    },
                    CompositionWitnessInput {
                        machine: "runtime_ingress".into(),
                        input_variant: "StageDrainSnapshot".into(),
                        fields: vec![
                            CompositionWitnessField {
                                field: "run_id".into(),
                                expr: Expr::String("runid_surface_1".into()),
                            },
                            CompositionWitnessField {
                                field: "contributing_work_ids".into(),
                                expr: Expr::SeqLiteral(vec![Expr::String("external_evt_1".into())]),
                            },
                        ],
                    },
                    CompositionWitnessInput {
                        machine: "turn_execution".into(),
                        input_variant: "PrimitiveApplied".into(),
                        fields: primitive_applied_fields(
                            "runid_surface_1",
                            "TextOnly",
                            false,
                            false,
                        ),
                    },
                    CompositionWitnessInput {
                        machine: "turn_execution".into(),
                        input_variant: "LlmReturnedTerminal".into(),
                        fields: vec![CompositionWitnessField {
                            field: "run_id".into(),
                            expr: Expr::String("runid_surface_1".into()),
                        }],
                    },
                    CompositionWitnessInput {
                        machine: "turn_execution".into(),
                        input_variant: "BoundaryComplete".into(),
                        fields: vec![CompositionWitnessField {
                            field: "run_id".into(),
                            expr: Expr::String("runid_surface_1".into()),
                        }],
                    },
                ],
                expected_routes: vec![
                    "surface_event_enters_ingress".into(),
                    "surface_ingress_ready_starts_runtime_control".into(),
                    "surface_runtime_control_starts_execution".into(),
                    "surface_execution_boundary_updates_ingress".into(),
                    "surface_execution_completion_updates_ingress".into(),
                    "surface_execution_completion_notifies_control".into(),
                ],
                expected_scheduler_rules: vec![],
                expected_states: vec![
                    witness_state(
                        "runtime_control",
                        Some("Idle"),
                        vec![CompositionWitnessField {
                            field: "current_run_id".into(),
                            expr: Expr::None,
                        }],
                    ),
                    witness_state(
                        "runtime_ingress",
                        Some("Active"),
                        vec![CompositionWitnessField {
                            field: "current_run".into(),
                            expr: Expr::None,
                        }],
                    ),
                    witness_state("turn_execution", Some("Completed"), vec![]),
                ],
                expected_transitions: vec![
                    witness_transition("runtime_control", "Initialize"),
                    witness_transition("runtime_control", "AdmissionAcceptedIdleQueue"),
                    witness_transition("runtime_ingress", "AdmitQueuedQueue"),
                    witness_transition("runtime_ingress", "StageDrainSnapshot"),
                    witness_transition("runtime_control", "BeginRunFromIdle"),
                    witness_transition("turn_execution", "StartConversationRun"),
                    witness_transition("turn_execution", "BoundaryComplete"),
                    witness_transition("runtime_ingress", "RunCompleted"),
                    witness_transition("runtime_control", "RunCompleted"),
                ],
                expected_transition_order: vec![
                    witness_transition_order(
                        "runtime_control",
                        "AdmissionAcceptedIdleQueue",
                        "runtime_ingress",
                        "AdmitQueuedQueue",
                    ),
                    witness_transition_order(
                        "runtime_ingress",
                        "StageDrainSnapshot",
                        "runtime_control",
                        "BeginRunFromIdle",
                    ),
                    witness_transition_order(
                        "runtime_control",
                        "BeginRunFromIdle",
                        "turn_execution",
                        "StartConversationRun",
                    ),
                    witness_transition_order(
                        "turn_execution",
                        "BoundaryComplete",
                        "runtime_ingress",
                        "RunCompleted",
                    ),
                    witness_transition_order(
                        "turn_execution",
                        "BoundaryComplete",
                        "runtime_control",
                        "RunCompleted",
                    ),
                ],
                state_limits: CompositionStateLimits {
                    step_limit: 10,
                    pending_input_limit: 7,
                    pending_route_limit: 2,
                    delivered_route_limit: 7,
                    emitted_effect_limit: 7,
                    seq_limit: 7,
                    set_limit: 7,
                    map_limit: 4,
                },
            },
            CompositionWitness {
                name: "surface_event_inline_image_success_path".into(),
                preload_inputs: vec![
                    CompositionWitnessInput {
                        machine: "runtime_control".into(),
                        input_variant: "Initialize".into(),
                        fields: vec![],
                    },
                    CompositionWitnessInput {
                        machine: "runtime_control".into(),
                        input_variant: "SubmitWork".into(),
                        fields: submit_candidate_fields(
                            "external_evt_1",
                            "WorkInput",
                            "InlineImage",
                        ),
                    },
                    CompositionWitnessInput {
                        machine: "runtime_control".into(),
                        input_variant: "AdmissionAccepted".into(),
                        fields: admission_accepted_fields(
                            "external_evt_1",
                            "WorkInput",
                            "InlineImage",
                            true,
                            false,
                        ),
                    },
                    CompositionWitnessInput {
                        machine: "runtime_ingress".into(),
                        input_variant: "StageDrainSnapshot".into(),
                        fields: vec![
                            CompositionWitnessField {
                                field: "run_id".into(),
                                expr: Expr::String("runid_surface_1".into()),
                            },
                            CompositionWitnessField {
                                field: "contributing_work_ids".into(),
                                expr: Expr::SeqLiteral(vec![Expr::String("external_evt_1".into())]),
                            },
                        ],
                    },
                    CompositionWitnessInput {
                        machine: "turn_execution".into(),
                        input_variant: "PrimitiveApplied".into(),
                        fields: primitive_applied_fields(
                            "runid_surface_1",
                            "InlineImage",
                            false,
                            false,
                        ),
                    },
                    CompositionWitnessInput {
                        machine: "turn_execution".into(),
                        input_variant: "LlmReturnedTerminal".into(),
                        fields: vec![CompositionWitnessField {
                            field: "run_id".into(),
                            expr: Expr::String("runid_surface_1".into()),
                        }],
                    },
                    CompositionWitnessInput {
                        machine: "turn_execution".into(),
                        input_variant: "BoundaryComplete".into(),
                        fields: vec![CompositionWitnessField {
                            field: "run_id".into(),
                            expr: Expr::String("runid_surface_1".into()),
                        }],
                    },
                ],
                expected_routes: vec![
                    "surface_event_enters_ingress".into(),
                    "surface_ingress_ready_starts_runtime_control".into(),
                    "surface_runtime_control_starts_execution".into(),
                    "surface_execution_boundary_updates_ingress".into(),
                    "surface_execution_completion_updates_ingress".into(),
                    "surface_execution_completion_notifies_control".into(),
                ],
                expected_scheduler_rules: vec![],
                expected_states: vec![
                    witness_state(
                        "runtime_control",
                        Some("Idle"),
                        vec![CompositionWitnessField {
                            field: "current_run_id".into(),
                            expr: Expr::None,
                        }],
                    ),
                    witness_state(
                        "runtime_ingress",
                        Some("Active"),
                        vec![CompositionWitnessField {
                            field: "current_run".into(),
                            expr: Expr::None,
                        }],
                    ),
                    witness_state("turn_execution", Some("Completed"), vec![]),
                ],
                expected_transitions: vec![
                    witness_transition("runtime_control", "Initialize"),
                    witness_transition("runtime_control", "AdmissionAcceptedIdleQueue"),
                    witness_transition("runtime_ingress", "AdmitQueuedQueue"),
                    witness_transition("runtime_ingress", "StageDrainSnapshot"),
                    witness_transition("runtime_control", "BeginRunFromIdle"),
                    witness_transition("turn_execution", "StartConversationRun"),
                    witness_transition("turn_execution", "BoundaryComplete"),
                    witness_transition("runtime_ingress", "RunCompleted"),
                    witness_transition("runtime_control", "RunCompleted"),
                ],
                expected_transition_order: vec![
                    witness_transition_order(
                        "runtime_control",
                        "AdmissionAcceptedIdleQueue",
                        "runtime_ingress",
                        "AdmitQueuedQueue",
                    ),
                    witness_transition_order(
                        "runtime_ingress",
                        "StageDrainSnapshot",
                        "runtime_control",
                        "BeginRunFromIdle",
                    ),
                    witness_transition_order(
                        "runtime_control",
                        "BeginRunFromIdle",
                        "turn_execution",
                        "StartConversationRun",
                    ),
                    witness_transition_order(
                        "turn_execution",
                        "BoundaryComplete",
                        "runtime_ingress",
                        "RunCompleted",
                    ),
                    witness_transition_order(
                        "turn_execution",
                        "BoundaryComplete",
                        "runtime_control",
                        "RunCompleted",
                    ),
                ],
                state_limits: CompositionStateLimits {
                    step_limit: 10,
                    pending_input_limit: 7,
                    pending_route_limit: 2,
                    delivered_route_limit: 7,
                    emitted_effect_limit: 7,
                    seq_limit: 7,
                    set_limit: 7,
                    map_limit: 4,
                },
            },
            CompositionWitness {
                name: "surface_event_failure_path".into(),
                preload_inputs: vec![
                    CompositionWitnessInput {
                        machine: "runtime_control".into(),
                        input_variant: "Initialize".into(),
                        fields: vec![],
                    },
                    CompositionWitnessInput {
                        machine: "runtime_control".into(),
                        input_variant: "SubmitWork".into(),
                        fields: submit_candidate_fields(
                            "external_evt_1",
                            "WorkInput",
                            "TextOnly",
                        ),
                    },
                    CompositionWitnessInput {
                        machine: "runtime_control".into(),
                        input_variant: "AdmissionAccepted".into(),
                        fields: admission_accepted_fields(
                            "external_evt_1",
                            "WorkInput",
                            "TextOnly",
                            true,
                            false,
                        ),
                    },
                    CompositionWitnessInput {
                        machine: "runtime_ingress".into(),
                        input_variant: "StageDrainSnapshot".into(),
                        fields: vec![
                            CompositionWitnessField {
                                field: "run_id".into(),
                                expr: Expr::String("runid_surface_1".into()),
                            },
                            CompositionWitnessField {
                                field: "contributing_work_ids".into(),
                                expr: Expr::SeqLiteral(vec![Expr::String("external_evt_1".into())]),
                            },
                        ],
                    },
                    CompositionWitnessInput {
                        machine: "turn_execution".into(),
                        input_variant: "FatalFailure".into(),
                        fields: vec![CompositionWitnessField {
                            field: "run_id".into(),
                            expr: Expr::String("runid_surface_1".into()),
                        }],
                    },
                ],
                expected_routes: vec![
                    "surface_event_enters_ingress".into(),
                    "surface_ingress_ready_starts_runtime_control".into(),
                    "surface_runtime_control_starts_execution".into(),
                    "surface_execution_failure_updates_ingress".into(),
                    "surface_execution_failure_notifies_control".into(),
                ],
                expected_scheduler_rules: vec![],
                expected_states: vec![
                    witness_state("runtime_control", Some("Idle"), vec![]),
                    witness_state(
                        "runtime_ingress",
                        Some("Active"),
                        vec![CompositionWitnessField {
                            field: "current_run".into(),
                            expr: Expr::None,
                        }],
                    ),
                    witness_state("turn_execution", Some("Failed"), vec![]),
                ],
                expected_transitions: vec![
                    witness_transition("runtime_control", "AdmissionAcceptedIdleQueue"),
                    witness_transition("runtime_ingress", "AdmitQueuedQueue"),
                    witness_transition("runtime_ingress", "StageDrainSnapshot"),
                    witness_transition("runtime_control", "BeginRunFromIdle"),
                    witness_transition("turn_execution", "StartConversationRun"),
                    witness_transition("turn_execution", "FatalFailureFromApplyingPrimitive"),
                    witness_transition("runtime_ingress", "RunFailed"),
                    witness_transition("runtime_control", "RunFailed"),
                ],
                expected_transition_order: vec![
                    witness_transition_order(
                        "runtime_control",
                        "AdmissionAcceptedIdleQueue",
                        "runtime_ingress",
                        "AdmitQueuedQueue",
                    ),
                    witness_transition_order(
                        "runtime_ingress",
                        "StageDrainSnapshot",
                        "runtime_control",
                        "BeginRunFromIdle",
                    ),
                    witness_transition_order(
                        "runtime_control",
                        "BeginRunFromIdle",
                        "turn_execution",
                        "StartConversationRun",
                    ),
                    witness_transition_order(
                        "turn_execution",
                        "FatalFailureFromApplyingPrimitive",
                        "runtime_ingress",
                        "RunFailed",
                    ),
                    witness_transition_order(
                        "turn_execution",
                        "FatalFailureFromApplyingPrimitive",
                        "runtime_control",
                        "RunFailed",
                    ),
                ],
                state_limits: CompositionStateLimits {
                    step_limit: 8,
                    pending_input_limit: 6,
                    pending_route_limit: 2,
                    delivered_route_limit: 6,
                    emitted_effect_limit: 6,
                    seq_limit: 6,
                    set_limit: 6,
                    map_limit: 4,
                },
            },
            CompositionWitness {
                name: "surface_event_control_preemption".into(),
                preload_inputs: vec![
                    CompositionWitnessInput {
                        machine: "runtime_control".into(),
                        input_variant: "Initialize".into(),
                        fields: vec![],
                    },
                    CompositionWitnessInput {
                        machine: "runtime_control".into(),
                        input_variant: "SubmitWork".into(),
                        fields: submit_candidate_fields(
                            "external_evt_1",
                            "WorkInput",
                            "TextOnly",
                        ),
                    },
                    CompositionWitnessInput {
                        machine: "runtime_control".into(),
                        input_variant: "AdmissionAccepted".into(),
                        fields: admission_accepted_fields(
                            "external_evt_1",
                            "WorkInput",
                            "TextOnly",
                            true,
                            true,
                        ),
                    },
                ],
                expected_routes: vec!["surface_event_enters_ingress".into()],
                expected_scheduler_rules: vec![SchedulerRule::PreemptWhenReady {
                    higher: "control_plane".into(),
                    lower: "ordinary_ingress".into(),
                }],
                expected_states: vec![witness_state("runtime_control", Some("Idle"), vec![])],
                expected_transitions: vec![
                    witness_transition("runtime_control", "Initialize"),
                    witness_transition("runtime_control", "AdmissionAcceptedIdleSteer"),
                    witness_transition("runtime_ingress", "AdmitQueuedSteer"),
                ],
                expected_transition_order: vec![
                    witness_transition_order(
                        "runtime_control",
                        "Initialize",
                        "runtime_control",
                        "AdmissionAcceptedIdleSteer",
                    ),
                    witness_transition_order(
                        "runtime_control",
                        "AdmissionAcceptedIdleSteer",
                        "runtime_ingress",
                        "AdmitQueuedSteer",
                    ),
                ],
                state_limits: CompositionStateLimits {
                    step_limit: 5,
                    pending_input_limit: 4,
                    pending_route_limit: 2,
                    delivered_route_limit: 3,
                    emitted_effect_limit: 3,
                    seq_limit: 4,
                    set_limit: 4,
                    map_limit: 3,
                },
            },
        ],
        deep_domain_cardinality: 2,
        deep_domain_overrides: BTreeMap::new(),
        witness_domain_cardinality: 1,
    }
}

pub fn continuation_runtime_bundle_composition() -> CompositionSchema {
    CompositionSchema {
        name: "continuation_runtime_bundle".into(),
        machines: vec![
            MachineInstance {
                instance_id: "runtime_control".into(),
                machine_name: "RuntimeControlMachine".into(),
                actor: "control_plane".into(),
            },
            MachineInstance {
                instance_id: "runtime_ingress".into(),
                machine_name: "RuntimeIngressMachine".into(),
                actor: "ordinary_ingress".into(),
            },
            MachineInstance {
                instance_id: "turn_execution".into(),
                machine_name: "TurnExecutionMachine".into(),
                actor: "turn_executor".into(),
            },
        ],
        entry_inputs: vec![
            EntryInput {
                name: "control_initialize".into(),
                machine: "runtime_control".into(),
                input_variant: "Initialize".into(),
            },
            EntryInput {
                name: "continuation_submit_candidate".into(),
                machine: "runtime_control".into(),
                input_variant: "SubmitWork".into(),
            },
            EntryInput {
                name: "runtime_admission_accepted".into(),
                machine: "runtime_control".into(),
                input_variant: "AdmissionAccepted".into(),
            },
            EntryInput {
                name: "ingress_stage_drain_snapshot".into(),
                machine: "runtime_ingress".into(),
                input_variant: "StageDrainSnapshot".into(),
            },
            EntryInput {
                name: "turn_primitive_applied".into(),
                machine: "turn_execution".into(),
                input_variant: "PrimitiveApplied".into(),
            },
            EntryInput {
                name: "turn_llm_returned_terminal".into(),
                machine: "turn_execution".into(),
                input_variant: "LlmReturnedTerminal".into(),
            },
            EntryInput {
                name: "turn_boundary_complete".into(),
                machine: "turn_execution".into(),
                input_variant: "BoundaryComplete".into(),
            },
        ],
        routes: vec![
            Route {
                name: "continuation_enters_ingress".into(),
                from_machine: "runtime_control".into(),
                effect_variant: "SubmitAdmittedIngressEffect".into(),
                to: RouteTarget {
                    machine: "runtime_ingress".into(),
                    input_variant: "AdmitQueued".into(),
                },
                bindings: vec![
                    RouteFieldBinding {
                        to_field: "work_id".into(),
                        source: RouteBindingSource::Field {
                            from_field: "work_id".into(),
                            allow_named_alias: true,
                        },
                    },
                    RouteFieldBinding {
                        to_field: "handling_mode".into(),
                        source: RouteBindingSource::Literal(Expr::String("Steer".into())),
                    },
                    RouteFieldBinding {
                        to_field: "policy".into(),
                        source: RouteBindingSource::Literal(Expr::String(
                            "ContinuationQueued".into(),
                        )),
                    },
                ],
                delivery: RouteDelivery::Immediate,
            },
            Route {
                name: "continuation_ingress_ready_starts_runtime_control".into(),
                from_machine: "runtime_ingress".into(),
                effect_variant: "ReadyForRun".into(),
                to: RouteTarget {
                    machine: "runtime_control".into(),
                    input_variant: "BeginRun".into(),
                },
                bindings: vec![RouteFieldBinding {
                    to_field: "run_id".into(),
                    source: RouteBindingSource::Field {
                        from_field: "run_id".into(),
                        allow_named_alias: false,
                    },
                }],
                delivery: RouteDelivery::Immediate,
            },
            Route {
                name: "continuation_runtime_control_starts_execution".into(),
                from_machine: "runtime_control".into(),
                effect_variant: "SubmitRunPrimitive".into(),
                to: RouteTarget {
                    machine: "turn_execution".into(),
                    input_variant: "StartConversationRun".into(),
                },
                bindings: vec![RouteFieldBinding {
                    to_field: "run_id".into(),
                    source: RouteBindingSource::Field {
                        from_field: "run_id".into(),
                        allow_named_alias: false,
                    },
                }],
                delivery: RouteDelivery::Immediate,
            },
            Route {
                name: "continuation_execution_completion_updates_ingress".into(),
                from_machine: "turn_execution".into(),
                effect_variant: "RunCompleted".into(),
                to: RouteTarget {
                    machine: "runtime_ingress".into(),
                    input_variant: "RunCompleted".into(),
                },
                bindings: vec![RouteFieldBinding {
                    to_field: "run_id".into(),
                    source: RouteBindingSource::Field {
                        from_field: "run_id".into(),
                        allow_named_alias: false,
                    },
                }],
                delivery: RouteDelivery::Immediate,
            },
            Route {
                name: "continuation_execution_completion_notifies_control".into(),
                from_machine: "turn_execution".into(),
                effect_variant: "RunCompleted".into(),
                to: RouteTarget {
                    machine: "runtime_control".into(),
                    input_variant: "RunCompleted".into(),
                },
                bindings: vec![RouteFieldBinding {
                    to_field: "run_id".into(),
                    source: RouteBindingSource::Field {
                        from_field: "run_id".into(),
                        allow_named_alias: false,
                    },
                }],
                delivery: RouteDelivery::Immediate,
            },
        ],
        actor_priorities: vec![ActorPriority {
            higher: "control_plane".into(),
            lower: "ordinary_ingress".into(),
            reason: "runtime control preempts continuation ingress work".into(),
        }],
        scheduler_rules: vec![SchedulerRule::PreemptWhenReady {
            higher: "control_plane".into(),
            lower: "ordinary_ingress".into(),
        }],
        invariants: vec![
            CompositionInvariant {
                name: "continuation_uses_canonical_runtime_admission".into(),
                kind: CompositionInvariantKind::ObservedInputOriginatesFromEffect {
                    to_machine: "runtime_ingress".into(),
                    input_variant: "AdmitQueued".into(),
                    from_machine: "runtime_control".into(),
                    effect_variant: "SubmitAdmittedIngressEffect".into(),
                },
                statement:
                    "runtime-owned continuation work enters ingress only through runtime-control admission".into(),
                references_machines: vec!["runtime_control".into(), "runtime_ingress".into()],
                references_actors: vec!["control_plane".into(), "ordinary_ingress".into()],
            },
            CompositionInvariant {
                name: "continuation_begin_run_requires_staged_drain".into(),
                kind: CompositionInvariantKind::ObservedInputOriginatesFromEffect {
                    to_machine: "runtime_control".into(),
                    input_variant: "BeginRun".into(),
                    from_machine: "runtime_ingress".into(),
                    effect_variant: "ReadyForRun".into(),
                },
                statement:
                    "continuation work begins a run only after ingress-owned staging".into(),
                references_machines: vec!["runtime_control".into(), "runtime_ingress".into()],
                references_actors: vec!["control_plane".into(), "ordinary_ingress".into()],
            },
            CompositionInvariant {
                name: "continuation_execution_completion_is_handled".into(),
                kind: CompositionInvariantKind::OutcomeHandled {
                    from_machine: "turn_execution".into(),
                    effect_variant: "RunCompleted".into(),
                    required_targets: vec![
                        RouteTarget {
                            machine: "runtime_ingress".into(),
                            input_variant: "RunCompleted".into(),
                        },
                        RouteTarget {
                            machine: "runtime_control".into(),
                            input_variant: "RunCompleted".into(),
                        },
                    ],
                },
                statement:
                    "continuation execution completion updates both ingress and runtime control".into(),
                references_machines: vec![
                    "turn_execution".into(),
                    "runtime_ingress".into(),
                    "runtime_control".into(),
                ],
                references_actors: vec![
                    "turn_executor".into(),
                    "ordinary_ingress".into(),
                    "control_plane".into(),
                ],
            },
            CompositionInvariant {
                name: "control_preempts_continuation_ingress".into(),
                kind: CompositionInvariantKind::SchedulerRulePresent {
                    rule: SchedulerRule::PreemptWhenReady {
                        higher: "control_plane".into(),
                        lower: "ordinary_ingress".into(),
                    },
                },
                statement: "runtime control outranks continuation ingress when both are ready".into(),
                references_machines: vec!["runtime_control".into(), "runtime_ingress".into()],
                references_actors: vec!["control_plane".into(), "ordinary_ingress".into()],
            },
        ],
        witnesses: vec![
            CompositionWitness {
                name: "terminal_response_continuation_success".into(),
                preload_inputs: vec![
                    CompositionWitnessInput {
                        machine: "runtime_control".into(),
                        input_variant: "Initialize".into(),
                        fields: vec![],
                    },
                    CompositionWitnessInput {
                        machine: "runtime_control".into(),
                        input_variant: "SubmitWork".into(),
                        fields: submit_candidate_fields(
                            "continuation_1",
                            "ContinuationInput",
                            "TextOnly",
                        ),
                    },
                    CompositionWitnessInput {
                        machine: "runtime_control".into(),
                        input_variant: "AdmissionAccepted".into(),
                        fields: admission_accepted_fields(
                            "continuation_1",
                            "ContinuationInput",
                            "TextOnly",
                            true,
                            true,
                        ),
                    },
                    CompositionWitnessInput {
                        machine: "runtime_ingress".into(),
                        input_variant: "StageDrainSnapshot".into(),
                        fields: vec![
                            CompositionWitnessField {
                                field: "run_id".into(),
                                expr: Expr::String("runid_cont_1".into()),
                            },
                            CompositionWitnessField {
                                field: "contributing_work_ids".into(),
                                expr: Expr::SeqLiteral(vec![Expr::String("continuation_1".into())]),
                            },
                        ],
                    },
                    CompositionWitnessInput {
                        machine: "turn_execution".into(),
                        input_variant: "PrimitiveApplied".into(),
                        fields: primitive_applied_fields(
                            "runid_cont_1",
                            "TextOnly",
                            false,
                            false,
                        ),
                    },
                    CompositionWitnessInput {
                        machine: "turn_execution".into(),
                        input_variant: "LlmReturnedTerminal".into(),
                        fields: vec![CompositionWitnessField {
                            field: "run_id".into(),
                            expr: Expr::String("runid_cont_1".into()),
                        }],
                    },
                    CompositionWitnessInput {
                        machine: "turn_execution".into(),
                        input_variant: "BoundaryComplete".into(),
                        fields: vec![CompositionWitnessField {
                            field: "run_id".into(),
                            expr: Expr::String("runid_cont_1".into()),
                        }],
                    },
                ],
                expected_routes: vec![
                    "continuation_enters_ingress".into(),
                    "continuation_ingress_ready_starts_runtime_control".into(),
                    "continuation_runtime_control_starts_execution".into(),
                    "continuation_execution_completion_updates_ingress".into(),
                    "continuation_execution_completion_notifies_control".into(),
                ],
                expected_scheduler_rules: vec![],
                expected_states: vec![
                    witness_state("runtime_control", Some("Idle"), vec![]),
                    witness_state(
                        "runtime_ingress",
                        Some("Active"),
                        vec![CompositionWitnessField {
                            field: "current_run".into(),
                            expr: Expr::None,
                        }],
                    ),
                    witness_state("turn_execution", Some("Completed"), vec![]),
                ],
                expected_transitions: vec![
                    witness_transition("runtime_control", "AdmissionAcceptedIdleSteer"),
                    witness_transition("runtime_ingress", "AdmitQueuedSteer"),
                    witness_transition("runtime_ingress", "StageDrainSnapshot"),
                    witness_transition("runtime_control", "BeginRunFromIdle"),
                    witness_transition("turn_execution", "StartConversationRun"),
                    witness_transition("turn_execution", "BoundaryComplete"),
                    witness_transition("runtime_ingress", "RunCompleted"),
                    witness_transition("runtime_control", "RunCompleted"),
                ],
                expected_transition_order: vec![
                    witness_transition_order(
                        "runtime_control",
                        "AdmissionAcceptedIdleSteer",
                        "runtime_ingress",
                        "AdmitQueuedSteer",
                    ),
                    witness_transition_order(
                        "runtime_ingress",
                        "StageDrainSnapshot",
                        "runtime_control",
                        "BeginRunFromIdle",
                    ),
                    witness_transition_order(
                        "runtime_control",
                        "BeginRunFromIdle",
                        "turn_execution",
                        "StartConversationRun",
                    ),
                    witness_transition_order(
                        "turn_execution",
                        "BoundaryComplete",
                        "runtime_ingress",
                        "RunCompleted",
                    ),
                    witness_transition_order(
                        "turn_execution",
                        "BoundaryComplete",
                        "runtime_control",
                        "RunCompleted",
                    ),
                ],
                state_limits: CompositionStateLimits {
                    step_limit: 9,
                    pending_input_limit: 6,
                    pending_route_limit: 2,
                    delivered_route_limit: 6,
                    emitted_effect_limit: 6,
                    seq_limit: 6,
                    set_limit: 6,
                    map_limit: 4,
                },
            },
            CompositionWitness {
                name: "host_mode_continuation_success".into(),
                preload_inputs: vec![
                    CompositionWitnessInput {
                        machine: "runtime_control".into(),
                        input_variant: "Initialize".into(),
                        fields: vec![],
                    },
                    CompositionWitnessInput {
                        machine: "runtime_control".into(),
                        input_variant: "SubmitWork".into(),
                        fields: submit_candidate_fields(
                            "host_continuation_1",
                            "ContinuationInput",
                            "TextOnly",
                        ),
                    },
                    CompositionWitnessInput {
                        machine: "runtime_control".into(),
                        input_variant: "AdmissionAccepted".into(),
                        fields: admission_accepted_fields(
                            "host_continuation_1",
                            "ContinuationInput",
                            "TextOnly",
                            true,
                            true,
                        ),
                    },
                    CompositionWitnessInput {
                        machine: "runtime_ingress".into(),
                        input_variant: "StageDrainSnapshot".into(),
                        fields: vec![
                            CompositionWitnessField {
                                field: "run_id".into(),
                                expr: Expr::String("runid_host_cont_1".into()),
                            },
                            CompositionWitnessField {
                                field: "contributing_work_ids".into(),
                                expr: Expr::SeqLiteral(vec![Expr::String("host_continuation_1".into())]),
                            },
                        ],
                    },
                    CompositionWitnessInput {
                        machine: "turn_execution".into(),
                        input_variant: "PrimitiveApplied".into(),
                        fields: primitive_applied_fields(
                            "runid_host_cont_1",
                            "TextOnly",
                            false,
                            false,
                        ),
                    },
                    CompositionWitnessInput {
                        machine: "turn_execution".into(),
                        input_variant: "LlmReturnedTerminal".into(),
                        fields: vec![CompositionWitnessField {
                            field: "run_id".into(),
                            expr: Expr::String("runid_host_cont_1".into()),
                        }],
                    },
                    CompositionWitnessInput {
                        machine: "turn_execution".into(),
                        input_variant: "BoundaryComplete".into(),
                        fields: vec![CompositionWitnessField {
                            field: "run_id".into(),
                            expr: Expr::String("runid_host_cont_1".into()),
                        }],
                    },
                ],
                expected_routes: vec![
                    "continuation_enters_ingress".into(),
                    "continuation_ingress_ready_starts_runtime_control".into(),
                    "continuation_runtime_control_starts_execution".into(),
                    "continuation_execution_completion_updates_ingress".into(),
                    "continuation_execution_completion_notifies_control".into(),
                ],
                expected_scheduler_rules: vec![],
                expected_states: vec![
                    witness_state("runtime_control", Some("Idle"), vec![]),
                    witness_state(
                        "runtime_ingress",
                        Some("Active"),
                        vec![CompositionWitnessField {
                            field: "current_run".into(),
                            expr: Expr::None,
                        }],
                    ),
                    witness_state("turn_execution", Some("Completed"), vec![]),
                ],
                expected_transitions: vec![
                    witness_transition("runtime_control", "AdmissionAcceptedIdleSteer"),
                    witness_transition("runtime_ingress", "AdmitQueuedSteer"),
                    witness_transition("runtime_ingress", "StageDrainSnapshot"),
                    witness_transition("runtime_control", "BeginRunFromIdle"),
                    witness_transition("turn_execution", "StartConversationRun"),
                    witness_transition("turn_execution", "BoundaryComplete"),
                    witness_transition("runtime_ingress", "RunCompleted"),
                    witness_transition("runtime_control", "RunCompleted"),
                ],
                expected_transition_order: vec![
                    witness_transition_order(
                        "runtime_control",
                        "AdmissionAcceptedIdleSteer",
                        "runtime_ingress",
                        "AdmitQueuedSteer",
                    ),
                    witness_transition_order(
                        "runtime_ingress",
                        "StageDrainSnapshot",
                        "runtime_control",
                        "BeginRunFromIdle",
                    ),
                    witness_transition_order(
                        "runtime_control",
                        "BeginRunFromIdle",
                        "turn_execution",
                        "StartConversationRun",
                    ),
                    witness_transition_order(
                        "turn_execution",
                        "BoundaryComplete",
                        "runtime_ingress",
                        "RunCompleted",
                    ),
                    witness_transition_order(
                        "turn_execution",
                        "BoundaryComplete",
                        "runtime_control",
                        "RunCompleted",
                    ),
                ],
                state_limits: CompositionStateLimits {
                    step_limit: 9,
                    pending_input_limit: 6,
                    pending_route_limit: 2,
                    delivered_route_limit: 6,
                    emitted_effect_limit: 6,
                    seq_limit: 6,
                    set_limit: 6,
                    map_limit: 4,
                },
            },
            CompositionWitness {
                name: "continuation_control_preemption".into(),
                preload_inputs: vec![
                    CompositionWitnessInput {
                        machine: "runtime_control".into(),
                        input_variant: "Initialize".into(),
                        fields: vec![],
                    },
                    CompositionWitnessInput {
                        machine: "runtime_control".into(),
                        input_variant: "SubmitWork".into(),
                        fields: submit_candidate_fields(
                            "continuation_1",
                            "ContinuationInput",
                            "TextOnly",
                        ),
                    },
                    CompositionWitnessInput {
                        machine: "runtime_control".into(),
                        input_variant: "AdmissionAccepted".into(),
                        fields: admission_accepted_fields(
                            "continuation_1",
                            "ContinuationInput",
                            "TextOnly",
                            true,
                            true,
                        ),
                    },
                ],
                expected_routes: vec!["continuation_enters_ingress".into()],
                expected_scheduler_rules: vec![SchedulerRule::PreemptWhenReady {
                    higher: "control_plane".into(),
                    lower: "ordinary_ingress".into(),
                }],
                expected_states: vec![witness_state("runtime_control", Some("Idle"), vec![])],
                expected_transitions: vec![
                    witness_transition("runtime_control", "Initialize"),
                    witness_transition("runtime_control", "AdmissionAcceptedIdleSteer"),
                    witness_transition("runtime_ingress", "AdmitQueuedSteer"),
                ],
                expected_transition_order: vec![
                    witness_transition_order(
                        "runtime_control",
                        "Initialize",
                        "runtime_control",
                        "AdmissionAcceptedIdleSteer",
                    ),
                    witness_transition_order(
                        "runtime_control",
                        "AdmissionAcceptedIdleSteer",
                        "runtime_ingress",
                        "AdmitQueuedSteer",
                    ),
                ],
                state_limits: CompositionStateLimits {
                    step_limit: 5,
                    pending_input_limit: 4,
                    pending_route_limit: 2,
                    delivered_route_limit: 3,
                    emitted_effect_limit: 3,
                    seq_limit: 4,
                    set_limit: 4,
                    map_limit: 3,
                },
            },
        ],
        deep_domain_cardinality: 2,
        deep_domain_overrides: BTreeMap::new(),
        witness_domain_cardinality: 1,
    }
}

pub fn mob_bundle_composition() -> CompositionSchema {
    CompositionSchema {
        name: "mob_bundle".into(),
        machines: vec![
            MachineInstance {
                instance_id: "mob_lifecycle".into(),
                machine_name: "MobLifecycleMachine".into(),
                actor: "mob_lifecycle_actor".into(),
            },
            MachineInstance {
                instance_id: "mob_orchestrator".into(),
                machine_name: "MobOrchestratorMachine".into(),
                actor: "mob_orchestrator_actor".into(),
            },
            MachineInstance {
                instance_id: "flow_run".into(),
                machine_name: "FlowRunMachine".into(),
                actor: "flow_engine".into(),
            },
            MachineInstance {
                instance_id: "ops_lifecycle".into(),
                machine_name: "OpsLifecycleMachine".into(),
                actor: "ops_plane".into(),
            },
            MachineInstance {
                instance_id: "peer_comms".into(),
                machine_name: "PeerCommsMachine".into(),
                actor: "peer_plane".into(),
            },
            MachineInstance {
                instance_id: "runtime_control".into(),
                machine_name: "RuntimeControlMachine".into(),
                actor: "control_plane".into(),
            },
            MachineInstance {
                instance_id: "runtime_ingress".into(),
                machine_name: "RuntimeIngressMachine".into(),
                actor: "ordinary_ingress".into(),
            },
            MachineInstance {
                instance_id: "turn_execution".into(),
                machine_name: "TurnExecutionMachine".into(),
                actor: "turn_executor".into(),
            },
        ],
        entry_inputs: vec![
            EntryInput { name: "control_initialize".into(), machine: "runtime_control".into(), input_variant: "Initialize".into() },
            EntryInput { name: "mob_initialize".into(), machine: "mob_orchestrator".into(), input_variant: "InitializeOrchestrator".into() },
            EntryInput { name: "mob_bind_coordinator".into(), machine: "mob_orchestrator".into(), input_variant: "BindCoordinator".into() },
            EntryInput { name: "mob_unbind_coordinator".into(), machine: "mob_orchestrator".into(), input_variant: "UnbindCoordinator".into() },
            EntryInput { name: "mob_stage_spawn".into(), machine: "mob_orchestrator".into(), input_variant: "StageSpawn".into() },
            EntryInput { name: "mob_complete_spawn".into(), machine: "mob_orchestrator".into(), input_variant: "CompleteSpawn".into() },
            EntryInput { name: "mob_start_flow".into(), machine: "mob_orchestrator".into(), input_variant: "StartFlow".into() },
            EntryInput { name: "mob_complete_flow".into(), machine: "mob_orchestrator".into(), input_variant: "CompleteFlow".into() },
            EntryInput { name: "flow_create_run".into(), machine: "flow_run".into(), input_variant: "CreateRun".into() },
            EntryInput { name: "flow_start_run".into(), machine: "flow_run".into(), input_variant: "StartRun".into() },
            EntryInput { name: "flow_dispatch_step".into(), machine: "flow_run".into(), input_variant: "DispatchStep".into() },
            EntryInput { name: "flow_complete_step".into(), machine: "flow_run".into(), input_variant: "CompleteStep".into() },
            EntryInput { name: "flow_record_step_output".into(), machine: "flow_run".into(), input_variant: "RecordStepOutput".into() },
            EntryInput { name: "flow_fail_step".into(), machine: "flow_run".into(), input_variant: "FailStep".into() },
            EntryInput { name: "flow_skip_step".into(), machine: "flow_run".into(), input_variant: "SkipStep".into() },
            EntryInput { name: "flow_cancel_step".into(), machine: "flow_run".into(), input_variant: "CancelStep".into() },
            EntryInput { name: "ops_register_operation".into(), machine: "ops_lifecycle".into(), input_variant: "RegisterOperation".into() },
            EntryInput { name: "ops_provisioning_succeeded".into(), machine: "ops_lifecycle".into(), input_variant: "ProvisioningSucceeded".into() },
            EntryInput { name: "ops_progress_reported".into(), machine: "ops_lifecycle".into(), input_variant: "ProgressReported".into() },
            EntryInput { name: "ops_complete_operation".into(), machine: "ops_lifecycle".into(), input_variant: "CompleteOperation".into() },
            EntryInput { name: "ops_fail_operation".into(), machine: "ops_lifecycle".into(), input_variant: "FailOperation".into() },
            EntryInput { name: "ops_cancel_operation".into(), machine: "ops_lifecycle".into(), input_variant: "CancelOperation".into() },
            EntryInput { name: "peer_trust".into(), machine: "peer_comms".into(), input_variant: "TrustPeer".into() },
            EntryInput { name: "peer_receive".into(), machine: "peer_comms".into(), input_variant: "ReceivePeerEnvelope".into() },
            EntryInput { name: "peer_submit".into(), machine: "peer_comms".into(), input_variant: "SubmitTypedPeerInput".into() },
            EntryInput { name: "runtime_admission_accepted".into(), machine: "runtime_control".into(), input_variant: "AdmissionAccepted".into() },
            EntryInput { name: "ingress_stage_drain_snapshot".into(), machine: "runtime_ingress".into(), input_variant: "StageDrainSnapshot".into() },
            EntryInput { name: "runtime_begin_run".into(), machine: "runtime_control".into(), input_variant: "BeginRun".into() },
            EntryInput { name: "turn_start_conversation".into(), machine: "turn_execution".into(), input_variant: "StartConversationRun".into() },
            EntryInput { name: "turn_boundary_complete".into(), machine: "turn_execution".into(), input_variant: "BoundaryComplete".into() },
            EntryInput { name: "turn_primitive_applied".into(), machine: "turn_execution".into(), input_variant: "PrimitiveApplied".into() },
            EntryInput { name: "turn_llm_returned_terminal".into(), machine: "turn_execution".into(), input_variant: "LlmReturnedTerminal".into() },
            EntryInput { name: "turn_fatal_failure".into(), machine: "turn_execution".into(), input_variant: "FatalFailure".into() },
            EntryInput { name: "turn_cancel_now".into(), machine: "turn_execution".into(), input_variant: "CancelNow".into() },
            EntryInput { name: "turn_cancel_after_boundary".into(), machine: "turn_execution".into(), input_variant: "CancelAfterBoundary".into() },
            EntryInput { name: "turn_cancellation_observed".into(), machine: "turn_execution".into(), input_variant: "CancellationObserved".into() },
        ],
        routes: vec![
            Route {
                name: "mob_supervisor_activation_starts_lifecycle".into(),
                from_machine: "mob_orchestrator".into(),
                effect_variant: "ActivateSupervisor".into(),
                to: RouteTarget {
                    machine: "mob_lifecycle".into(),
                    input_variant: "Start".into(),
                },
                bindings: vec![],
                delivery: RouteDelivery::Immediate,
            },
            Route {
                name: "flow_step_dispatch_enters_runtime_admission".into(),
                from_machine: "flow_run".into(),
                effect_variant: "AdmitStepWork".into(),
                to: RouteTarget { machine: "runtime_control".into(), input_variant: "SubmitWork".into() },
                bindings: vec![
                    RouteFieldBinding { to_field: "work_id".into(), source: RouteBindingSource::Field { from_field: "step_id".into(), allow_named_alias: true } },
                    RouteFieldBinding { to_field: "handling_mode".into(), source: RouteBindingSource::Literal(Expr::String("Queue".into())) },
                ],
                delivery: RouteDelivery::Immediate,
            },
            Route {
                name: "mob_flow_activation_starts_flow_run".into(),
                from_machine: "mob_orchestrator".into(),
                effect_variant: "FlowActivated".into(),
                to: RouteTarget {
                    machine: "flow_run".into(),
                    input_variant: "StartRun".into(),
                },
                bindings: vec![],
                delivery: RouteDelivery::Immediate,
            },
            Route {
                name: "mob_flow_activation_marks_lifecycle_run".into(),
                from_machine: "mob_orchestrator".into(),
                effect_variant: "FlowActivated".into(),
                to: RouteTarget {
                    machine: "mob_lifecycle".into(),
                    input_variant: "StartRun".into(),
                },
                bindings: vec![],
                delivery: RouteDelivery::Immediate,
            },
            Route {
                name: "mob_async_op_event_enters_runtime_admission".into(),
                from_machine: "ops_lifecycle".into(),
                effect_variant: "SubmitOpEvent".into(),
                to: RouteTarget { machine: "runtime_control".into(), input_variant: "SubmitWork".into() },
                bindings: vec![
                    RouteFieldBinding { to_field: "work_id".into(), source: RouteBindingSource::Field { from_field: "operation_id".into(), allow_named_alias: true } },
                    RouteFieldBinding { to_field: "handling_mode".into(), source: RouteBindingSource::Literal(Expr::String("Steer".into())) },
                ],
                delivery: RouteDelivery::Immediate,
            },
            Route {
                name: "mob_peer_candidate_enters_runtime_admission".into(),
                from_machine: "peer_comms".into(),
                effect_variant: "SubmitPeerInputCandidate".into(),
                to: RouteTarget { machine: "runtime_control".into(), input_variant: "SubmitWork".into() },
                bindings: vec![
                    RouteFieldBinding { to_field: "work_id".into(), source: RouteBindingSource::Field { from_field: "raw_item_id".into(), allow_named_alias: true } },
                    RouteFieldBinding { to_field: "handling_mode".into(), source: RouteBindingSource::Literal(Expr::String("Steer".into())) },
                ],
                delivery: RouteDelivery::Immediate,
            },
            Route {
                name: "mob_admitted_work_enters_ingress".into(),
                from_machine: "runtime_control".into(),
                effect_variant: "SubmitAdmittedIngressEffect".into(),
                to: RouteTarget { machine: "runtime_ingress".into(), input_variant: "AdmitQueued".into() },
                bindings: vec![
                    RouteFieldBinding { to_field: "work_id".into(), source: RouteBindingSource::Field { from_field: "work_id".into(), allow_named_alias: true } },
                    RouteFieldBinding { to_field: "handling_mode".into(), source: RouteBindingSource::Field { from_field: "handling_mode".into(), allow_named_alias: false } },
                    RouteFieldBinding { to_field: "policy".into(), source: RouteBindingSource::Literal(Expr::String("MobQueued".into())) },
                ],
                delivery: RouteDelivery::Immediate,
            },
            Route {
                name: "mob_ingress_ready_starts_runtime_control".into(),
                from_machine: "runtime_ingress".into(),
                effect_variant: "ReadyForRun".into(),
                to: RouteTarget { machine: "runtime_control".into(), input_variant: "BeginRun".into() },
                bindings: vec![RouteFieldBinding { to_field: "run_id".into(), source: RouteBindingSource::Field { from_field: "run_id".into(), allow_named_alias: false } }],
                delivery: RouteDelivery::Immediate,
            },
            Route {
                name: "mob_runtime_control_starts_execution".into(),
                from_machine: "runtime_control".into(),
                effect_variant: "SubmitRunPrimitive".into(),
                to: RouteTarget { machine: "turn_execution".into(), input_variant: "StartConversationRun".into() },
                bindings: vec![RouteFieldBinding { to_field: "run_id".into(), source: RouteBindingSource::Field { from_field: "run_id".into(), allow_named_alias: false } }],
                delivery: RouteDelivery::Immediate,
            },
            Route {
                name: "mob_flow_terminalization_completes_orchestrator".into(),
                from_machine: "flow_run".into(),
                effect_variant: "FlowTerminalized".into(),
                to: RouteTarget {
                    machine: "mob_orchestrator".into(),
                    input_variant: "CompleteFlow".into(),
                },
                bindings: vec![],
                delivery: RouteDelivery::Immediate,
            },
            Route {
                name: "mob_flow_deactivation_finishes_lifecycle_run".into(),
                from_machine: "mob_orchestrator".into(),
                effect_variant: "FlowDeactivated".into(),
                to: RouteTarget {
                    machine: "mob_lifecycle".into(),
                    input_variant: "FinishRun".into(),
                },
                bindings: vec![],
                delivery: RouteDelivery::Immediate,
            },
            Route {
                name: "mob_execution_boundary_updates_ingress".into(),
                from_machine: "turn_execution".into(),
                effect_variant: "BoundaryApplied".into(),
                to: RouteTarget { machine: "runtime_ingress".into(), input_variant: "BoundaryApplied".into() },
                bindings: vec![
                    RouteFieldBinding { to_field: "run_id".into(), source: RouteBindingSource::Field { from_field: "run_id".into(), allow_named_alias: false } },
                    RouteFieldBinding { to_field: "boundary_sequence".into(), source: RouteBindingSource::Field { from_field: "boundary_sequence".into(), allow_named_alias: false } },
                ],
                delivery: RouteDelivery::Immediate,
            },
            Route {
                name: "mob_execution_completion_updates_ingress".into(),
                from_machine: "turn_execution".into(),
                effect_variant: "RunCompleted".into(),
                to: RouteTarget { machine: "runtime_ingress".into(), input_variant: "RunCompleted".into() },
                bindings: vec![RouteFieldBinding { to_field: "run_id".into(), source: RouteBindingSource::Field { from_field: "run_id".into(), allow_named_alias: false } }],
                delivery: RouteDelivery::Immediate,
            },
            Route {
                name: "mob_execution_completion_notifies_control".into(),
                from_machine: "turn_execution".into(),
                effect_variant: "RunCompleted".into(),
                to: RouteTarget { machine: "runtime_control".into(), input_variant: "RunCompleted".into() },
                bindings: vec![RouteFieldBinding { to_field: "run_id".into(), source: RouteBindingSource::Field { from_field: "run_id".into(), allow_named_alias: false } }],
                delivery: RouteDelivery::Immediate,
            },
            Route {
                name: "mob_execution_failure_updates_ingress".into(),
                from_machine: "turn_execution".into(),
                effect_variant: "RunFailed".into(),
                to: RouteTarget { machine: "runtime_ingress".into(), input_variant: "RunFailed".into() },
                bindings: vec![RouteFieldBinding { to_field: "run_id".into(), source: RouteBindingSource::Field { from_field: "run_id".into(), allow_named_alias: false } }],
                delivery: RouteDelivery::Immediate,
            },
            Route {
                name: "mob_execution_failure_notifies_control".into(),
                from_machine: "turn_execution".into(),
                effect_variant: "RunFailed".into(),
                to: RouteTarget { machine: "runtime_control".into(), input_variant: "RunFailed".into() },
                bindings: vec![RouteFieldBinding { to_field: "run_id".into(), source: RouteBindingSource::Field { from_field: "run_id".into(), allow_named_alias: false } }],
                delivery: RouteDelivery::Immediate,
            },
            Route {
                name: "mob_execution_cancel_updates_ingress".into(),
                from_machine: "turn_execution".into(),
                effect_variant: "RunCancelled".into(),
                to: RouteTarget { machine: "runtime_ingress".into(), input_variant: "RunCancelled".into() },
                bindings: vec![RouteFieldBinding { to_field: "run_id".into(), source: RouteBindingSource::Field { from_field: "run_id".into(), allow_named_alias: false } }],
                delivery: RouteDelivery::Immediate,
            },
            Route {
                name: "mob_execution_cancel_notifies_control".into(),
                from_machine: "turn_execution".into(),
                effect_variant: "RunCancelled".into(),
                to: RouteTarget { machine: "runtime_control".into(), input_variant: "RunCancelled".into() },
                bindings: vec![RouteFieldBinding { to_field: "run_id".into(), source: RouteBindingSource::Field { from_field: "run_id".into(), allow_named_alias: false } }],
                delivery: RouteDelivery::Immediate,
            },
        ],
        actor_priorities: vec![ActorPriority {
            higher: "control_plane".into(),
            lower: "ordinary_ingress".into(),
            reason: "runtime control preempts mob-side ingress work when both are ready".into(),
        }],
        scheduler_rules: vec![SchedulerRule::PreemptWhenReady {
            higher: "control_plane".into(),
            lower: "ordinary_ingress".into(),
        }],
        invariants: vec![
            CompositionInvariant {
                name: "mob_supervisor_activation_starts_lifecycle".into(),
                kind: CompositionInvariantKind::ObservedRouteInputOriginatesFromEffect {
                    route_name: "mob_supervisor_activation_starts_lifecycle".into(),
                    to_machine: "mob_lifecycle".into(),
                    input_variant: "Start".into(),
                    from_machine: "mob_orchestrator".into(),
                    effect_variant: "ActivateSupervisor".into(),
                },
                statement:
                    "mob-orchestrator supervisor activation starts the lifecycle substrate through an explicit route".into(),
                references_machines: vec!["mob_orchestrator".into(), "mob_lifecycle".into()],
                references_actors: vec!["mob_orchestrator_actor".into(), "mob_lifecycle_actor".into()],
            },
            CompositionInvariant {
                name: "mob_flow_activation_starts_flow_run".into(),
                kind: CompositionInvariantKind::ObservedRouteInputOriginatesFromEffect {
                    route_name: "mob_flow_activation_starts_flow_run".into(),
                    to_machine: "flow_run".into(),
                    input_variant: "StartRun".into(),
                    from_machine: "mob_orchestrator".into(),
                    effect_variant: "FlowActivated".into(),
                },
                statement:
                    "mob-orchestrator flow activation starts the flow-run machine through an explicit route".into(),
                references_machines: vec!["mob_orchestrator".into(), "flow_run".into()],
                references_actors: vec!["mob_orchestrator_actor".into(), "flow_engine".into()],
            },
            CompositionInvariant {
                name: "mob_flow_activation_marks_lifecycle_run".into(),
                kind: CompositionInvariantKind::ObservedRouteInputOriginatesFromEffect {
                    route_name: "mob_flow_activation_marks_lifecycle_run".into(),
                    to_machine: "mob_lifecycle".into(),
                    input_variant: "StartRun".into(),
                    from_machine: "mob_orchestrator".into(),
                    effect_variant: "FlowActivated".into(),
                },
                statement:
                    "mob-orchestrator flow activation increments lifecycle run ownership through an explicit route".into(),
                references_machines: vec!["mob_orchestrator".into(), "mob_lifecycle".into()],
                references_actors: vec!["mob_orchestrator_actor".into(), "mob_lifecycle_actor".into()],
            },
            CompositionInvariant {
                name: "flow_dispatch_uses_canonical_runtime_admission".into(),
                kind: CompositionInvariantKind::ObservedRouteInputOriginatesFromEffect {
                    route_name: "flow_step_dispatch_enters_runtime_admission".into(),
                    to_machine: "runtime_control".into(),
                    input_variant: "SubmitWork".into(),
                    from_machine: "flow_run".into(),
                    effect_variant: "AdmitStepWork".into(),
                },
                statement:
                    "flow-run step dispatch reaches runtime only through the runtime-control admission surface".into(),
                references_machines: vec!["flow_run".into(), "runtime_control".into()],
                references_actors: vec!["flow_engine".into(), "control_plane".into()],
            },
            CompositionInvariant {
                name: "mob_async_lifecycle_events_use_operation_input".into(),
                kind: CompositionInvariantKind::ObservedRouteInputOriginatesFromEffect {
                    route_name: "mob_async_op_event_enters_runtime_admission".into(),
                    to_machine: "runtime_control".into(),
                    input_variant: "SubmitWork".into(),
                    from_machine: "ops_lifecycle".into(),
                    effect_variant: "SubmitOpEvent".into(),
                },
                statement:
                    "mob-backed async lifecycle events re-enter runtime through the operation-input admission path".into(),
                references_machines: vec!["ops_lifecycle".into(), "runtime_control".into()],
                references_actors: vec!["ops_plane".into(), "control_plane".into()],
            },
            CompositionInvariant {
                name: "mob_peer_work_uses_canonical_runtime_admission".into(),
                kind: CompositionInvariantKind::ObservedRouteInputOriginatesFromEffect {
                    route_name: "mob_peer_candidate_enters_runtime_admission".into(),
                    to_machine: "runtime_control".into(),
                    input_variant: "SubmitWork".into(),
                    from_machine: "peer_comms".into(),
                    effect_variant: "SubmitPeerInputCandidate".into(),
                },
                statement:
                    "member peer communication enters runtime only through canonical admission".into(),
                references_machines: vec!["peer_comms".into(), "runtime_control".into()],
                references_actors: vec!["peer_plane".into(), "control_plane".into()],
            },
            CompositionInvariant {
                name: "mob_runtime_work_flows_into_ingress".into(),
                kind: CompositionInvariantKind::ObservedRouteInputOriginatesFromEffect {
                    route_name: "mob_admitted_work_enters_ingress".into(),
                    to_machine: "runtime_ingress".into(),
                    input_variant: "AdmitQueued".into(),
                    from_machine: "runtime_control".into(),
                    effect_variant: "SubmitAdmittedIngressEffect".into(),
                },
                statement:
                    "mob-originated admitted work is handed into canonical ingress ownership".into(),
                references_machines: vec![
                    "runtime_control".into(),
                    "runtime_ingress".into(),
                    "flow_run".into(),
                    "ops_lifecycle".into(),
                    "peer_comms".into(),
                ],
                references_actors: vec![
                    "control_plane".into(),
                    "ordinary_ingress".into(),
                    "flow_engine".into(),
                    "ops_plane".into(),
                    "peer_plane".into(),
                ],
            },
            CompositionInvariant {
                name: "mob_flow_terminalization_completes_orchestrator".into(),
                kind: CompositionInvariantKind::ObservedRouteInputOriginatesFromEffect {
                    route_name: "mob_flow_terminalization_completes_orchestrator".into(),
                    to_machine: "mob_orchestrator".into(),
                    input_variant: "CompleteFlow".into(),
                    from_machine: "flow_run".into(),
                    effect_variant: "FlowTerminalized".into(),
                },
                statement:
                    "flow terminalization closes the orchestrator-side active flow through an explicit route".into(),
                references_machines: vec!["flow_run".into(), "mob_orchestrator".into()],
                references_actors: vec!["flow_engine".into(), "mob_orchestrator_actor".into()],
            },
            CompositionInvariant {
                name: "mob_flow_deactivation_finishes_lifecycle_run".into(),
                kind: CompositionInvariantKind::ObservedRouteInputOriginatesFromEffect {
                    route_name: "mob_flow_deactivation_finishes_lifecycle_run".into(),
                    to_machine: "mob_lifecycle".into(),
                    input_variant: "FinishRun".into(),
                    from_machine: "mob_orchestrator".into(),
                    effect_variant: "FlowDeactivated".into(),
                },
                statement:
                    "orchestrator flow deactivation closes the lifecycle run count through an explicit route".into(),
                references_machines: vec!["mob_orchestrator".into(), "mob_lifecycle".into()],
                references_actors: vec!["mob_orchestrator_actor".into(), "mob_lifecycle_actor".into()],
            },
            CompositionInvariant {
                name: "mob_execution_failure_is_handled".into(),
                kind: CompositionInvariantKind::OutcomeHandled {
                    from_machine: "turn_execution".into(),
                    effect_variant: "RunFailed".into(),
                    required_targets: vec![
                        RouteTarget { machine: "runtime_ingress".into(), input_variant: "RunFailed".into() },
                        RouteTarget { machine: "runtime_control".into(), input_variant: "RunFailed".into() },
                    ],
                },
                statement:
                    "mob turn-execution failure is handled by both ingress and runtime control".into(),
                references_machines: vec![
                    "turn_execution".into(),
                    "runtime_ingress".into(),
                    "runtime_control".into(),
                ],
                references_actors: vec![
                    "turn_executor".into(),
                    "ordinary_ingress".into(),
                    "control_plane".into(),
                ],
            },
            CompositionInvariant {
                name: "mob_execution_cancel_is_handled".into(),
                kind: CompositionInvariantKind::OutcomeHandled {
                    from_machine: "turn_execution".into(),
                    effect_variant: "RunCancelled".into(),
                    required_targets: vec![
                        RouteTarget { machine: "runtime_ingress".into(), input_variant: "RunCancelled".into() },
                        RouteTarget { machine: "runtime_control".into(), input_variant: "RunCancelled".into() },
                    ],
                },
                statement:
                    "mob turn-execution cancellation is handled by both ingress and runtime control".into(),
                references_machines: vec![
                    "turn_execution".into(),
                    "runtime_ingress".into(),
                    "runtime_control".into(),
                ],
                references_actors: vec![
                    "turn_executor".into(),
                    "ordinary_ingress".into(),
                    "control_plane".into(),
                ],
            },
            CompositionInvariant {
                name: "control_preempts_mob_ingress".into(),
                kind: CompositionInvariantKind::SchedulerRulePresent {
                    rule: SchedulerRule::PreemptWhenReady {
                        higher: "control_plane".into(),
                        lower: "ordinary_ingress".into(),
                    },
                },
                statement: "runtime control outranks mob-side ingress work when both are ready".into(),
                references_machines: vec!["runtime_control".into(), "runtime_ingress".into()],
                references_actors: vec!["control_plane".into(), "ordinary_ingress".into()],
            },
        ],
        witnesses: vec![
            CompositionWitness {
                name: "mob_flow_success_path".into(),
                preload_inputs: vec![
                    CompositionWitnessInput {
                        machine: "runtime_control".into(),
                        input_variant: "Initialize".into(),
                        fields: vec![],
                    },
                    CompositionWitnessInput {
                        machine: "flow_run".into(),
                        input_variant: "CreateRun".into(),
                        fields: vec![CompositionWitnessField {
                            field: "step_ids".into(),
                            expr: Expr::SeqLiteral(vec![Expr::String("step_1".into())]),
                        }],
                    },
                    CompositionWitnessInput {
                        machine: "mob_orchestrator".into(),
                        input_variant: "InitializeOrchestrator".into(),
                        fields: vec![],
                    },
                    CompositionWitnessInput {
                        machine: "mob_orchestrator".into(),
                        input_variant: "BindCoordinator".into(),
                        fields: vec![],
                    },
                    CompositionWitnessInput {
                        machine: "mob_orchestrator".into(),
                        input_variant: "StartFlow".into(),
                        fields: vec![],
                    },
                    CompositionWitnessInput {
                        machine: "flow_run".into(),
                        input_variant: "DispatchStep".into(),
                        fields: vec![CompositionWitnessField {
                            field: "step_id".into(),
                            expr: Expr::String("step_1".into()),
                        }],
                    },
                    CompositionWitnessInput {
                        machine: "runtime_control".into(),
                        input_variant: "AdmissionAccepted".into(),
                        fields: admission_accepted_fields(
                            "step_1",
                            "WorkInput",
                            "TextOnly",
                            true,
                            true,
                        ),
                    },
                    CompositionWitnessInput {
                        machine: "runtime_ingress".into(),
                        input_variant: "StageDrainSnapshot".into(),
                        fields: vec![
                            CompositionWitnessField {
                                field: "run_id".into(),
                                expr: Expr::String("runid_1".into()),
                            },
                            CompositionWitnessField {
                                field: "contributing_work_ids".into(),
                                expr: Expr::SeqLiteral(vec![Expr::String("step_1".into())]),
                            },
                        ],
                    },
                    CompositionWitnessInput {
                        machine: "turn_execution".into(),
                        input_variant: "PrimitiveApplied".into(),
                        fields: primitive_applied_fields(
                            "runid_1",
                            "TextOnly",
                            false,
                            false,
                        ),
                    },
                    CompositionWitnessInput {
                        machine: "turn_execution".into(),
                        input_variant: "LlmReturnedTerminal".into(),
                        fields: vec![CompositionWitnessField {
                            field: "run_id".into(),
                            expr: Expr::String("runid_1".into()),
                        }],
                    },
                    CompositionWitnessInput {
                        machine: "turn_execution".into(),
                        input_variant: "BoundaryComplete".into(),
                        fields: vec![CompositionWitnessField {
                            field: "run_id".into(),
                            expr: Expr::String("runid_1".into()),
                        }],
                    },
                    CompositionWitnessInput {
                        machine: "flow_run".into(),
                        input_variant: "CompleteStep".into(),
                        fields: vec![CompositionWitnessField {
                            field: "step_id".into(),
                            expr: Expr::String("step_1".into()),
                        }],
                    },
                    CompositionWitnessInput {
                        machine: "flow_run".into(),
                        input_variant: "RecordStepOutput".into(),
                        fields: vec![CompositionWitnessField {
                            field: "step_id".into(),
                            expr: Expr::String("step_1".into()),
                        }],
                    },
                    CompositionWitnessInput {
                        machine: "flow_run".into(),
                        input_variant: "TerminalizeCompleted".into(),
                        fields: vec![],
                    },
                ],
                expected_routes: vec![
                    "mob_supervisor_activation_starts_lifecycle".into(),
                    "mob_flow_activation_starts_flow_run".into(),
                    "mob_flow_activation_marks_lifecycle_run".into(),
                    "flow_step_dispatch_enters_runtime_admission".into(),
                    "mob_admitted_work_enters_ingress".into(),
                    "mob_ingress_ready_starts_runtime_control".into(),
                    "mob_runtime_control_starts_execution".into(),
                    "mob_execution_boundary_updates_ingress".into(),
                    "mob_execution_completion_updates_ingress".into(),
                    "mob_execution_completion_notifies_control".into(),
                    "mob_flow_terminalization_completes_orchestrator".into(),
                    "mob_flow_deactivation_finishes_lifecycle_run".into(),
                ],
                expected_scheduler_rules: vec![],
                expected_states: vec![
                    witness_state(
                        "mob_orchestrator",
                        Some("Running"),
                        vec![
                            CompositionWitnessField {
                                field: "coordinator_bound".into(),
                                expr: Expr::Bool(true),
                            },
                            CompositionWitnessField {
                                field: "active_flow_count".into(),
                                expr: Expr::U64(0),
                            },
                        ],
                    ),
                    witness_state(
                        "mob_lifecycle",
                        Some("Running"),
                        vec![CompositionWitnessField {
                            field: "active_run_count".into(),
                            expr: Expr::U64(0),
                        }],
                    ),
                    witness_state("flow_run", Some("Completed"), vec![]),
                    witness_state(
                        "runtime_control",
                        Some("Idle"),
                        vec![CompositionWitnessField {
                            field: "current_run_id".into(),
                            expr: Expr::None,
                        }],
                    ),
                    witness_state(
                        "runtime_ingress",
                        Some("Active"),
                        vec![CompositionWitnessField {
                            field: "current_run".into(),
                            expr: Expr::None,
                        }],
                    ),
                    witness_state("turn_execution", Some("Completed"), vec![]),
                ],
                expected_transitions: vec![
                    witness_transition("mob_orchestrator", "InitializeOrchestrator"),
                    witness_transition("mob_lifecycle", "Start"),
                    witness_transition("mob_orchestrator", "BindCoordinator"),
                    witness_transition("mob_orchestrator", "StartFlow"),
                    witness_transition("flow_run", "CreateRun"),
                    witness_transition("flow_run", "StartRun"),
                    witness_transition("mob_lifecycle", "StartRun"),
                    witness_transition("flow_run", "DispatchStep"),
                    witness_transition("runtime_control", "AdmissionAcceptedIdleSteer"),
                    witness_transition("runtime_ingress", "AdmitQueuedSteer"),
                    witness_transition("runtime_ingress", "StageDrainSnapshot"),
                    witness_transition("runtime_control", "BeginRunFromIdle"),
                    witness_transition("turn_execution", "StartConversationRun"),
                    witness_transition("turn_execution", "BoundaryComplete"),
                    witness_transition("runtime_ingress", "RunCompleted"),
                    witness_transition("runtime_control", "RunCompleted"),
                    witness_transition("flow_run", "CompleteStep"),
                    witness_transition("flow_run", "RecordStepOutput"),
                    witness_transition("flow_run", "TerminalizeCompleted"),
                    witness_transition("mob_orchestrator", "CompleteFlow"),
                    witness_transition("mob_lifecycle", "FinishRun"),
                ],
                expected_transition_order: vec![
                    witness_transition_order(
                        "mob_orchestrator",
                        "InitializeOrchestrator",
                        "mob_lifecycle",
                        "Start",
                    ),
                    witness_transition_order(
                        "mob_lifecycle",
                        "Start",
                        "mob_orchestrator",
                        "BindCoordinator",
                    ),
                    witness_transition_order(
                        "mob_orchestrator",
                        "BindCoordinator",
                        "mob_orchestrator",
                        "StartFlow",
                    ),
                    witness_transition_order(
                        "mob_orchestrator",
                        "StartFlow",
                        "flow_run",
                        "StartRun",
                    ),
                    witness_transition_order(
                        "mob_orchestrator",
                        "StartFlow",
                        "mob_lifecycle",
                        "StartRun",
                    ),
                    witness_transition_order(
                        "flow_run",
                        "DispatchStep",
                        "runtime_control",
                        "AdmissionAcceptedIdleSteer",
                    ),
                    witness_transition_order(
                        "runtime_control",
                        "AdmissionAcceptedIdleSteer",
                        "runtime_ingress",
                        "AdmitQueuedSteer",
                    ),
                    witness_transition_order(
                        "runtime_ingress",
                        "StageDrainSnapshot",
                        "runtime_control",
                        "BeginRunFromIdle",
                    ),
                    witness_transition_order(
                        "runtime_control",
                        "BeginRunFromIdle",
                        "turn_execution",
                        "StartConversationRun",
                    ),
                    witness_transition_order(
                        "turn_execution",
                        "BoundaryComplete",
                        "runtime_ingress",
                        "RunCompleted",
                    ),
                    witness_transition_order(
                        "turn_execution",
                        "BoundaryComplete",
                        "runtime_control",
                        "RunCompleted",
                    ),
                    witness_transition_order(
                        "flow_run",
                        "TerminalizeCompleted",
                        "mob_orchestrator",
                        "CompleteFlow",
                    ),
                    witness_transition_order(
                        "mob_orchestrator",
                        "CompleteFlow",
                        "mob_lifecycle",
                        "FinishRun",
                    ),
                ],
                state_limits: CompositionStateLimits {
                    step_limit: 18,
                    pending_input_limit: 10,
                    pending_route_limit: 3,
                    delivered_route_limit: 14,
                    emitted_effect_limit: 14,
                    seq_limit: 8,
                    set_limit: 8,
                    map_limit: 4,
                },
            },
            CompositionWitness {
                name: "mob_async_op_admission".into(),
                preload_inputs: vec![
                    CompositionWitnessInput {
                        machine: "runtime_control".into(),
                        input_variant: "Initialize".into(),
                        fields: vec![],
                    },
                    CompositionWitnessInput {
                        machine: "ops_lifecycle".into(),
                        input_variant: "RegisterOperation".into(),
                        fields: vec![
                            CompositionWitnessField {
                                field: "operation_id".into(),
                                expr: Expr::String("op_1".into()),
                            },
                            CompositionWitnessField {
                                field: "operation_kind".into(),
                                expr: Expr::String("MobMemberChild".into()),
                            },
                        ],
                    },
                    CompositionWitnessInput {
                        machine: "ops_lifecycle".into(),
                        input_variant: "ProvisioningSucceeded".into(),
                        fields: vec![CompositionWitnessField {
                            field: "operation_id".into(),
                            expr: Expr::String("op_1".into()),
                        }],
                    },
                    CompositionWitnessInput {
                        machine: "runtime_control".into(),
                        input_variant: "AdmissionAccepted".into(),
                        fields: admission_accepted_fields(
                            "op_1",
                            "OperationInput",
                            "TextOnly",
                            true,
                            true,
                        ),
                    },
                ],
                expected_routes: vec![
                    "mob_async_op_event_enters_runtime_admission".into(),
                    "mob_admitted_work_enters_ingress".into(),
                ],
                expected_scheduler_rules: vec![],
                expected_states: vec![
                    witness_state("ops_lifecycle", Some("Active"), vec![]),
                    witness_state("runtime_control", Some("Idle"), vec![]),
                    witness_state("runtime_ingress", Some("Active"), vec![]),
                ],
                expected_transitions: vec![
                    witness_transition("ops_lifecycle", "RegisterOperation"),
                    witness_transition("ops_lifecycle", "ProvisioningSucceeded"),
                    witness_transition("runtime_control", "AdmissionAcceptedIdleSteer"),
                    witness_transition("runtime_ingress", "AdmitQueuedSteer"),
                ],
                expected_transition_order: vec![
                    witness_transition_order(
                        "ops_lifecycle",
                        "ProvisioningSucceeded",
                        "runtime_control",
                        "AdmissionAcceptedIdleSteer",
                    ),
                    witness_transition_order(
                        "runtime_control",
                        "AdmissionAcceptedIdleSteer",
                        "runtime_ingress",
                        "AdmitQueuedSteer",
                    ),
                ],
                state_limits: CompositionStateLimits {
                    step_limit: 6,
                    pending_input_limit: 5,
                    pending_route_limit: 2,
                    delivered_route_limit: 4,
                    emitted_effect_limit: 4,
                    seq_limit: 4,
                    set_limit: 4,
                    map_limit: 4,
                },
            },
            CompositionWitness {
                name: "mob_peer_admission".into(),
                preload_inputs: vec![
                    CompositionWitnessInput {
                        machine: "runtime_control".into(),
                        input_variant: "Initialize".into(),
                        fields: vec![],
                    },
                    CompositionWitnessInput {
                        machine: "peer_comms".into(),
                        input_variant: "TrustPeer".into(),
                        fields: vec![CompositionWitnessField {
                            field: "peer_id".into(),
                            expr: Expr::String("peer_1".into()),
                        }],
                    },
                    CompositionWitnessInput {
                        machine: "peer_comms".into(),
                        input_variant: "ReceivePeerEnvelope".into(),
                        fields: peer_envelope_fields(
                            "raw_1",
                            "peer_1",
                            "Message",
                            "mob peer handoff",
                            "InlineImage",
                            None,
                            None,
                        ),
                    },
                    CompositionWitnessInput {
                        machine: "peer_comms".into(),
                        input_variant: "SubmitTypedPeerInput".into(),
                        fields: vec![CompositionWitnessField {
                            field: "raw_item_id".into(),
                            expr: Expr::String("raw_1".into()),
                        }],
                    },
                    CompositionWitnessInput {
                        machine: "runtime_control".into(),
                        input_variant: "AdmissionAccepted".into(),
                        fields: admission_accepted_fields(
                            "raw_1",
                            "PeerInput",
                            "InlineImage",
                            true,
                            true,
                        ),
                    },
                ],
                expected_routes: vec![
                    "mob_peer_candidate_enters_runtime_admission".into(),
                    "mob_admitted_work_enters_ingress".into(),
                ],
                expected_scheduler_rules: vec![],
                expected_states: vec![
                    witness_state("peer_comms", Some("Delivered"), vec![]),
                    witness_state("runtime_control", Some("Idle"), vec![]),
                    witness_state("runtime_ingress", Some("Active"), vec![]),
                ],
                expected_transitions: vec![
                    witness_transition("peer_comms", "TrustPeer"),
                    witness_transition("peer_comms", "ReceiveTrustedPeerEnvelope"),
                    witness_transition("peer_comms", "SubmitTypedPeerInputDelivered"),
                    witness_transition("runtime_control", "AdmissionAcceptedIdleSteer"),
                    witness_transition("runtime_ingress", "AdmitQueuedSteer"),
                ],
                expected_transition_order: vec![
                    witness_transition_order(
                        "peer_comms",
                        "ReceiveTrustedPeerEnvelope",
                        "peer_comms",
                        "SubmitTypedPeerInputDelivered",
                    ),
                    witness_transition_order(
                        "peer_comms",
                        "SubmitTypedPeerInputDelivered",
                        "runtime_control",
                        "AdmissionAcceptedIdleSteer",
                    ),
                    witness_transition_order(
                        "runtime_control",
                        "AdmissionAcceptedIdleSteer",
                        "runtime_ingress",
                        "AdmitQueuedSteer",
                    ),
                ],
                state_limits: CompositionStateLimits {
                    step_limit: 7,
                    pending_input_limit: 5,
                    pending_route_limit: 2,
                    delivered_route_limit: 4,
                    emitted_effect_limit: 4,
                    seq_limit: 4,
                    set_limit: 4,
                    map_limit: 4,
                },
            },
            CompositionWitness {
                name: "mob_failure_path".into(),
                preload_inputs: vec![
                    CompositionWitnessInput {
                        machine: "runtime_control".into(),
                        input_variant: "Initialize".into(),
                        fields: vec![],
                    },
                    CompositionWitnessInput {
                        machine: "flow_run".into(),
                        input_variant: "CreateRun".into(),
                        fields: vec![CompositionWitnessField {
                            field: "step_ids".into(),
                            expr: Expr::SeqLiteral(vec![Expr::String("step_1".into())]),
                        }],
                    },
                    CompositionWitnessInput {
                        machine: "flow_run".into(),
                        input_variant: "StartRun".into(),
                        fields: vec![],
                    },
                    CompositionWitnessInput {
                        machine: "flow_run".into(),
                        input_variant: "DispatchStep".into(),
                        fields: vec![CompositionWitnessField {
                            field: "step_id".into(),
                            expr: Expr::String("step_1".into()),
                        }],
                    },
                    CompositionWitnessInput {
                        machine: "runtime_control".into(),
                        input_variant: "AdmissionAccepted".into(),
                        fields: admission_accepted_fields(
                            "step_1",
                            "WorkInput",
                            "TextOnly",
                            true,
                            true,
                        ),
                    },
                    CompositionWitnessInput {
                        machine: "runtime_ingress".into(),
                        input_variant: "StageDrainSnapshot".into(),
                        fields: vec![
                            CompositionWitnessField {
                                field: "run_id".into(),
                                expr: Expr::String("runid_1".into()),
                            },
                            CompositionWitnessField {
                                field: "contributing_work_ids".into(),
                                expr: Expr::SeqLiteral(vec![Expr::String("step_1".into())]),
                            },
                        ],
                    },
                    CompositionWitnessInput {
                        machine: "turn_execution".into(),
                        input_variant: "FatalFailure".into(),
                        fields: vec![CompositionWitnessField {
                            field: "run_id".into(),
                            expr: Expr::String("runid_1".into()),
                        }],
                    },
                ],
                expected_routes: vec![
                    "flow_step_dispatch_enters_runtime_admission".into(),
                    "mob_admitted_work_enters_ingress".into(),
                    "mob_ingress_ready_starts_runtime_control".into(),
                    "mob_runtime_control_starts_execution".into(),
                    "mob_execution_failure_updates_ingress".into(),
                    "mob_execution_failure_notifies_control".into(),
                ],
                expected_scheduler_rules: vec![],
                expected_states: vec![
                    witness_state("runtime_control", Some("Idle"), vec![]),
                    witness_state(
                        "runtime_ingress",
                        Some("Active"),
                        vec![CompositionWitnessField {
                            field: "current_run".into(),
                            expr: Expr::None,
                        }],
                    ),
                    witness_state("turn_execution", Some("Failed"), vec![]),
                ],
                expected_transitions: vec![
                    witness_transition("flow_run", "DispatchStep"),
                    witness_transition("runtime_control", "AdmissionAcceptedIdleSteer"),
                    witness_transition("runtime_ingress", "AdmitQueuedSteer"),
                    witness_transition("runtime_ingress", "StageDrainSnapshot"),
                    witness_transition("runtime_control", "BeginRunFromIdle"),
                    witness_transition("turn_execution", "StartConversationRun"),
                    witness_transition("turn_execution", "FatalFailureFromApplyingPrimitive"),
                    witness_transition("runtime_ingress", "RunFailed"),
                    witness_transition("runtime_control", "RunFailed"),
                ],
                expected_transition_order: vec![
                    witness_transition_order(
                        "flow_run",
                        "DispatchStep",
                        "runtime_control",
                        "AdmissionAcceptedIdleSteer",
                    ),
                    witness_transition_order(
                        "runtime_control",
                        "AdmissionAcceptedIdleSteer",
                        "runtime_ingress",
                        "AdmitQueuedSteer",
                    ),
                    witness_transition_order(
                        "runtime_ingress",
                        "StageDrainSnapshot",
                        "runtime_control",
                        "BeginRunFromIdle",
                    ),
                    witness_transition_order(
                        "runtime_control",
                        "BeginRunFromIdle",
                        "turn_execution",
                        "StartConversationRun",
                    ),
                    witness_transition_order(
                        "turn_execution",
                        "FatalFailureFromApplyingPrimitive",
                        "runtime_ingress",
                        "RunFailed",
                    ),
                    witness_transition_order(
                        "turn_execution",
                        "FatalFailureFromApplyingPrimitive",
                        "runtime_control",
                        "RunFailed",
                    ),
                ],
                state_limits: CompositionStateLimits {
                    step_limit: 9,
                    pending_input_limit: 7,
                    pending_route_limit: 2,
                    delivered_route_limit: 7,
                    emitted_effect_limit: 7,
                    seq_limit: 7,
                    set_limit: 7,
                    map_limit: 4,
                },
            },
            CompositionWitness {
                name: "mob_cancel_path".into(),
                preload_inputs: vec![
                    CompositionWitnessInput {
                        machine: "runtime_control".into(),
                        input_variant: "Initialize".into(),
                        fields: vec![],
                    },
                    CompositionWitnessInput {
                        machine: "flow_run".into(),
                        input_variant: "CreateRun".into(),
                        fields: vec![CompositionWitnessField {
                            field: "step_ids".into(),
                            expr: Expr::SeqLiteral(vec![Expr::String("step_1".into())]),
                        }],
                    },
                    CompositionWitnessInput {
                        machine: "flow_run".into(),
                        input_variant: "StartRun".into(),
                        fields: vec![],
                    },
                    CompositionWitnessInput {
                        machine: "flow_run".into(),
                        input_variant: "DispatchStep".into(),
                        fields: vec![CompositionWitnessField {
                            field: "step_id".into(),
                            expr: Expr::String("step_1".into()),
                        }],
                    },
                    CompositionWitnessInput {
                        machine: "runtime_control".into(),
                        input_variant: "AdmissionAccepted".into(),
                        fields: admission_accepted_fields(
                            "step_1",
                            "WorkInput",
                            "TextOnly",
                            true,
                            true,
                        ),
                    },
                    CompositionWitnessInput {
                        machine: "runtime_ingress".into(),
                        input_variant: "StageDrainSnapshot".into(),
                        fields: vec![
                            CompositionWitnessField {
                                field: "run_id".into(),
                                expr: Expr::String("runid_1".into()),
                            },
                            CompositionWitnessField {
                                field: "contributing_work_ids".into(),
                                expr: Expr::SeqLiteral(vec![Expr::String("step_1".into())]),
                            },
                        ],
                    },
                    CompositionWitnessInput {
                        machine: "turn_execution".into(),
                        input_variant: "CancelNow".into(),
                        fields: vec![CompositionWitnessField {
                            field: "run_id".into(),
                            expr: Expr::String("runid_1".into()),
                        }],
                    },
                    CompositionWitnessInput {
                        machine: "turn_execution".into(),
                        input_variant: "CancellationObserved".into(),
                        fields: vec![CompositionWitnessField {
                            field: "run_id".into(),
                            expr: Expr::String("runid_1".into()),
                        }],
                    },
                ],
                expected_routes: vec![
                    "flow_step_dispatch_enters_runtime_admission".into(),
                    "mob_admitted_work_enters_ingress".into(),
                    "mob_ingress_ready_starts_runtime_control".into(),
                    "mob_runtime_control_starts_execution".into(),
                    "mob_execution_cancel_updates_ingress".into(),
                    "mob_execution_cancel_notifies_control".into(),
                ],
                expected_scheduler_rules: vec![],
                expected_states: vec![
                    witness_state("runtime_control", Some("Idle"), vec![]),
                    witness_state(
                        "runtime_ingress",
                        Some("Active"),
                        vec![CompositionWitnessField {
                            field: "current_run".into(),
                            expr: Expr::None,
                        }],
                    ),
                    witness_state("turn_execution", Some("Cancelled"), vec![]),
                ],
                expected_transitions: vec![
                    witness_transition("flow_run", "DispatchStep"),
                    witness_transition("runtime_control", "AdmissionAcceptedIdleSteer"),
                    witness_transition("runtime_ingress", "AdmitQueuedSteer"),
                    witness_transition("runtime_ingress", "StageDrainSnapshot"),
                    witness_transition("runtime_control", "BeginRunFromIdle"),
                    witness_transition("turn_execution", "StartConversationRun"),
                    witness_transition("turn_execution", "CancelNowFromApplyingPrimitive"),
                    witness_transition("turn_execution", "CancellationObserved"),
                    witness_transition("runtime_ingress", "RunCancelled"),
                    witness_transition("runtime_control", "RunCancelled"),
                ],
                expected_transition_order: vec![
                    witness_transition_order(
                        "flow_run",
                        "DispatchStep",
                        "runtime_control",
                        "AdmissionAcceptedIdleSteer",
                    ),
                    witness_transition_order(
                        "runtime_control",
                        "AdmissionAcceptedIdleSteer",
                        "runtime_ingress",
                        "AdmitQueuedSteer",
                    ),
                    witness_transition_order(
                        "runtime_ingress",
                        "StageDrainSnapshot",
                        "runtime_control",
                        "BeginRunFromIdle",
                    ),
                    witness_transition_order(
                        "runtime_control",
                        "BeginRunFromIdle",
                        "turn_execution",
                        "StartConversationRun",
                    ),
                    witness_transition_order(
                        "turn_execution",
                        "CancelNowFromApplyingPrimitive",
                        "turn_execution",
                        "CancellationObserved",
                    ),
                    witness_transition_order(
                        "turn_execution",
                        "CancellationObserved",
                        "runtime_ingress",
                        "RunCancelled",
                    ),
                    witness_transition_order(
                        "turn_execution",
                        "CancellationObserved",
                        "runtime_control",
                        "RunCancelled",
                    ),
                ],
                state_limits: CompositionStateLimits {
                    step_limit: 10,
                    pending_input_limit: 7,
                    pending_route_limit: 2,
                    delivered_route_limit: 7,
                    emitted_effect_limit: 7,
                    seq_limit: 7,
                    set_limit: 7,
                    map_limit: 4,
                },
            },
            CompositionWitness {
                name: "mob_control_preemption".into(),
                preload_inputs: vec![
                    CompositionWitnessInput {
                        machine: "runtime_control".into(),
                        input_variant: "Initialize".into(),
                        fields: vec![],
                    },
                    CompositionWitnessInput {
                        machine: "runtime_ingress".into(),
                        input_variant: "AdmitQueued".into(),
                        fields: admit_queued_fields(
                            "step_1",
                            "WorkInput",
                            "TextOnly",
                            "MobQueued",
                            true,
                            true,
                        ),
                    },
                ],
                expected_routes: vec![],
                expected_scheduler_rules: vec![SchedulerRule::PreemptWhenReady {
                    higher: "control_plane".into(),
                    lower: "ordinary_ingress".into(),
                }],
                expected_states: vec![witness_state("runtime_control", Some("Idle"), vec![])],
                expected_transitions: vec![
                    witness_transition("runtime_control", "Initialize"),
                    witness_transition("runtime_ingress", "AdmitQueuedSteer"),
                ],
                expected_transition_order: vec![witness_transition_order(
                    "runtime_control",
                    "Initialize",
                    "runtime_ingress",
                        "AdmitQueuedSteer",
                )],
                state_limits: CompositionStateLimits {
                    step_limit: 3,
                    pending_input_limit: 2,
                    pending_route_limit: 1,
                    delivered_route_limit: 1,
                    emitted_effect_limit: 1,
                    seq_limit: 2,
                    set_limit: 2,
                    map_limit: 2,
                },
            },
        ],
        deep_domain_cardinality: 1,
        deep_domain_overrides: BTreeMap::new(),
        witness_domain_cardinality: 1,
    }
}

fn witness_state(
    machine: &str,
    phase: Option<&str>,
    fields: Vec<CompositionWitnessField>,
) -> CompositionWitnessState {
    CompositionWitnessState {
        machine: machine.into(),
        phase: phase.map(|phase| phase.into()),
        fields,
    }
}

fn witness_transition(machine: &str, transition: &str) -> CompositionWitnessTransition {
    CompositionWitnessTransition {
        machine: machine.into(),
        transition: transition.into(),
    }
}

fn witness_transition_order(
    earlier_machine: &str,
    earlier_transition: &str,
    later_machine: &str,
    later_transition: &str,
) -> CompositionWitnessTransitionOrder {
    CompositionWitnessTransitionOrder {
        earlier: witness_transition(earlier_machine, earlier_transition),
        later: witness_transition(later_machine, later_transition),
    }
}

fn submit_candidate_fields(
    work_id: &str,
    _work_kind: &str,
    content_shape: &str,
) -> Vec<CompositionWitnessField> {
    vec![
        CompositionWitnessField {
            field: "work_id".into(),
            expr: Expr::String(work_id.into()),
        },
        CompositionWitnessField {
            field: "content_shape".into(),
            expr: Expr::String(content_shape.into()),
        },
        CompositionWitnessField {
            field: "handling_mode".into(),
            expr: Expr::String("Queue".into()),
        },
        CompositionWitnessField {
            field: "request_id".into(),
            expr: Expr::None,
        },
        CompositionWitnessField {
            field: "reservation_key".into(),
            expr: Expr::None,
        },
    ]
}

fn admission_accepted_fields(
    work_id: &str,
    _work_kind: &str,
    content_shape: &str,
    _wake: bool,
    process: bool,
) -> Vec<CompositionWitnessField> {
    let handling_mode = if process { "Steer" } else { "Queue" };
    vec![
        CompositionWitnessField {
            field: "work_id".into(),
            expr: Expr::String(work_id.into()),
        },
        CompositionWitnessField {
            field: "content_shape".into(),
            expr: Expr::String(content_shape.into()),
        },
        CompositionWitnessField {
            field: "handling_mode".into(),
            expr: Expr::String(handling_mode.into()),
        },
        CompositionWitnessField {
            field: "request_id".into(),
            expr: Expr::None,
        },
        CompositionWitnessField {
            field: "reservation_key".into(),
            expr: Expr::None,
        },
        CompositionWitnessField {
            field: "admission_effect".into(),
            expr: Expr::String("SubmitAdmittedIngressEffect".into()),
        },
    ]
}

fn admit_queued_fields(
    work_id: &str,
    _work_kind: &str,
    content_shape: &str,
    policy: &str,
    _wake: bool,
    process: bool,
) -> Vec<CompositionWitnessField> {
    let handling_mode = if process { "Steer" } else { "Queue" };
    vec![
        CompositionWitnessField {
            field: "work_id".into(),
            expr: Expr::String(work_id.into()),
        },
        CompositionWitnessField {
            field: "content_shape".into(),
            expr: Expr::String(content_shape.into()),
        },
        CompositionWitnessField {
            field: "handling_mode".into(),
            expr: Expr::String(handling_mode.into()),
        },
        CompositionWitnessField {
            field: "request_id".into(),
            expr: Expr::None,
        },
        CompositionWitnessField {
            field: "reservation_key".into(),
            expr: Expr::None,
        },
        CompositionWitnessField {
            field: "policy".into(),
            expr: Expr::String(policy.into()),
        },
    ]
}

fn primitive_applied_fields(
    run_id: &str,
    admitted_content_shape: &str,
    vision_enabled: bool,
    image_tool_results_enabled: bool,
) -> Vec<CompositionWitnessField> {
    vec![
        CompositionWitnessField {
            field: "run_id".into(),
            expr: Expr::String(run_id.into()),
        },
        CompositionWitnessField {
            field: "admitted_content_shape".into(),
            expr: Expr::String(admitted_content_shape.into()),
        },
        CompositionWitnessField {
            field: "vision_enabled".into(),
            expr: Expr::Bool(vision_enabled),
        },
        CompositionWitnessField {
            field: "image_tool_results_enabled".into(),
            expr: Expr::Bool(image_tool_results_enabled),
        },
    ]
}

fn peer_envelope_fields(
    raw_item_id: &str,
    peer_id: &str,
    raw_kind: &str,
    text_projection: &str,
    content_shape: &str,
    request_id: Option<&str>,
    reservation_key: Option<&str>,
) -> Vec<CompositionWitnessField> {
    vec![
        CompositionWitnessField {
            field: "raw_item_id".into(),
            expr: Expr::String(raw_item_id.into()),
        },
        CompositionWitnessField {
            field: "peer_id".into(),
            expr: Expr::String(peer_id.into()),
        },
        CompositionWitnessField {
            field: "raw_kind".into(),
            expr: Expr::String(raw_kind.into()),
        },
        CompositionWitnessField {
            field: "text_projection".into(),
            expr: Expr::String(text_projection.into()),
        },
        CompositionWitnessField {
            field: "content_shape".into(),
            expr: Expr::String(content_shape.into()),
        },
        CompositionWitnessField {
            field: "request_id".into(),
            expr: request_id
                .map(|value| Expr::Some(Box::new(Expr::String(value.into()))))
                .unwrap_or(Expr::None),
        },
        CompositionWitnessField {
            field: "reservation_key".into(),
            expr: reservation_key
                .map(|value| Expr::Some(Box::new(Expr::String(value.into()))))
                .unwrap_or(Expr::None),
        },
    ]
}
