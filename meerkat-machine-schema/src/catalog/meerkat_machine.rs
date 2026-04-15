use indexmap::IndexMap;

use crate::{
    EffectDisposition, EffectDispositionRule, EffectEmit, EnumSchema, Expr, FieldSchema, Guard,
    HelperSchema, InitSchema, InputMatch, InvariantSchema, MachineSchema, Quantifier, RustBinding,
    StateSchema, TransitionSchema, TypeRef, Update, VariantSchema, machine::TriggerKind,
};

pub fn meerkat_machine() -> MachineSchema {
    let mut trigger_variants = direct_meerkat_trigger_variants();
    trigger_variants.extend(absorbed_meerkat_input_variants());
    let (input_variants, signal_variants) =
        split_variants(trigger_variants, is_meerkat_runtime_input_variant);

    MachineSchema {
        machine: "MeerkatMachine".into(),
        version: 1,
        rust: RustBinding {
            crate_name: "meerkat-runtime".into(),
            module: "generated::meerkat_machine".into(),
        },
        state: StateSchema {
            phase: EnumSchema {
                name: "MeerkatPhase".into(),
                variants: vec![
                    variant("Initializing"),
                    variant("Idle"),
                    variant("Attached"),
                    variant("Running"),
                    // Recovering remains a compatibility-facing public
                    // RuntimeState, but the top-level MeerkatMachine does not
                    // transition into it directly.
                    variant("Retired"),
                    variant("Stopped"),
                    variant("Destroyed"),
                ],
            },
            fields: vec![
                field(
                    "session_id",
                    TypeRef::Option(Box::new(TypeRef::Named("SessionId".into()))),
                ),
                field(
                    "active_runtime_id",
                    TypeRef::Option(Box::new(TypeRef::Named("AgentRuntimeId".into()))),
                ),
                field(
                    "active_fence_token",
                    TypeRef::Option(Box::new(TypeRef::Named("FenceToken".into()))),
                ),
                field(
                    "active_generation",
                    TypeRef::Option(Box::new(TypeRef::Named("Generation".into()))),
                ),
                field("attachment_live", TypeRef::Bool),
                field(
                    "current_run_id",
                    TypeRef::Option(Box::new(TypeRef::Named("RunId".into()))),
                ),
                field("pre_run_phase", TypeRef::Option(Box::new(TypeRef::String))),
                field("peer_ingress_configured", TypeRef::Bool),
                field("drain_running", TypeRef::Bool),
                field(
                    "silent_intent_overrides",
                    TypeRef::Set(Box::new(TypeRef::String)),
                ),
                field(
                    "active_requested_deferred_names",
                    TypeRef::Set(Box::new(TypeRef::String)),
                ),
                field(
                    "staged_requested_deferred_names",
                    TypeRef::Set(Box::new(TypeRef::String)),
                ),
                field("active_visibility_revision", TypeRef::U64),
                field("staged_visibility_revision", TypeRef::U64),
            ],
            init: InitSchema {
                phase: "Initializing".into(),
                fields: vec![
                    init("session_id", Expr::None),
                    init("active_runtime_id", Expr::None),
                    init("active_fence_token", Expr::None),
                    init("active_generation", Expr::None),
                    init("attachment_live", Expr::Bool(false)),
                    init("current_run_id", Expr::None),
                    init("pre_run_phase", Expr::None),
                    init("peer_ingress_configured", Expr::Bool(false)),
                    init("drain_running", Expr::Bool(false)),
                    init("silent_intent_overrides", Expr::EmptySet),
                    init("active_requested_deferred_names", Expr::EmptySet),
                    init("staged_requested_deferred_names", Expr::EmptySet),
                    init("active_visibility_revision", Expr::U64(0)),
                    init("staged_visibility_revision", Expr::U64(0)),
                ],
            },
            terminal_phases: vec!["Destroyed".into()],
        },
        inputs: EnumSchema {
            name: "MeerkatMachineInput".into(),
            variants: input_variants,
        },
        surface_only_inputs: vec![
            "ContainsSession".into(),
            "SessionHasExecutor".into(),
            "SessionHasComms".into(),
            "OpsLifecycleRegistry".into(),
            "InputState".into(),
            "ListActiveInputs".into(),
            "RuntimeState".into(),
            "LoadBoundaryReceipt".into(),
            "Recover".into(),
        ],
        signals: EnumSchema {
            name: "MeerkatMachineSignal".into(),
            variants: signal_variants,
        },
        effects: EnumSchema {
            name: "MeerkatMachineEffect".into(),
            variants: {
                let mut variants = vec![
                    VariantSchema {
                        name: "RuntimeBound".into(),
                        fields: runtime_identity_fields(),
                    },
                    VariantSchema {
                        name: "RuntimeRetired".into(),
                        fields: runtime_identity_fields(),
                    },
                    VariantSchema {
                        name: "RuntimeDestroyed".into(),
                        fields: runtime_identity_fields(),
                    },
                    VariantSchema {
                        name: "WorkCompleted".into(),
                        fields: work_identity_fields(),
                    },
                    VariantSchema {
                        name: "WorkFailed".into(),
                        fields: work_identity_fields(),
                    },
                    VariantSchema {
                        name: "WorkCancelled".into(),
                        fields: work_identity_fields(),
                    },
                    variant("RequestCancellationAtBoundary"),
                    variant("WakeInterrupt"),
                    VariantSchema {
                        name: "CommittedVisibleSetPublished".into(),
                        fields: vec![field("revision", TypeRef::U64)],
                    },
                    VariantSchema {
                        name: "RuntimeNotice".into(),
                        fields: vec![
                            field("kind", TypeRef::String),
                            field("detail", TypeRef::String),
                        ],
                    },
                ];
                variants.extend(absorbed_meerkat_effect_variants());
                variants
            },
        },
        helpers: vec![HelperSchema {
            name: "HasPendingVisibilityPromotion".into(),
            params: vec![],
            returns: TypeRef::Bool,
            body: Expr::Gt(
                Box::new(Expr::Field("staged_visibility_revision".into())),
                Box::new(Expr::Field("active_visibility_revision".into())),
            ),
        }],
        derived: vec![],
        invariants: vec![
            // The top-level kernel no longer tracks a dedicated active work id.
            // Conversation runs enter Running via Prepare/Commit/Fail and the
            // lower control/ingress authorities own run identity.
            InvariantSchema {
                name: "fence_requires_bound_runtime".into(),
                expr: Expr::Or(vec![
                    Expr::Eq(
                        Box::new(Expr::Field("active_fence_token".into())),
                        Box::new(Expr::None),
                    ),
                    Expr::Neq(
                        Box::new(Expr::Field("active_runtime_id".into())),
                        Box::new(Expr::None),
                    ),
                ]),
            },
            InvariantSchema {
                name: "running_has_current_run".into(),
                expr: Expr::Or(vec![
                    Expr::Neq(
                        Box::new(Expr::CurrentPhase),
                        Box::new(Expr::Phase("Running".into())),
                    ),
                    Expr::Neq(
                        Box::new(Expr::Field("current_run_id".into())),
                        Box::new(Expr::None),
                    ),
                ]),
            },
            InvariantSchema {
                name: "current_run_only_while_running_or_retired".into(),
                expr: Expr::Or(vec![
                    Expr::Eq(
                        Box::new(Expr::Field("current_run_id".into())),
                        Box::new(Expr::None),
                    ),
                    Expr::Or(vec![
                        Expr::Eq(
                            Box::new(Expr::CurrentPhase),
                            Box::new(Expr::Phase("Running".into())),
                        ),
                        Expr::Eq(
                            Box::new(Expr::CurrentPhase),
                            Box::new(Expr::Phase("Retired".into())),
                        ),
                    ]),
                ]),
            },
            InvariantSchema {
                name: "drain_requires_ingress_context".into(),
                expr: Expr::Or(vec![
                    Expr::Eq(
                        Box::new(Expr::Field("drain_running".into())),
                        Box::new(Expr::Bool(false)),
                    ),
                    Expr::Eq(
                        Box::new(Expr::Field("peer_ingress_configured".into())),
                        Box::new(Expr::Bool(true)),
                    ),
                ]),
            },
            InvariantSchema {
                name: "active_requested_names_subset_of_staged".into(),
                expr: Expr::Quantified {
                    quantifier: Quantifier::All,
                    binding: "name".into(),
                    over: Box::new(Expr::Field("active_requested_deferred_names".into())),
                    body: Box::new(Expr::Contains {
                        collection: Box::new(Expr::Field("staged_requested_deferred_names".into())),
                        value: Box::new(Expr::Binding("name".into())),
                    }),
                },
            },
        ],
        transitions: vec![
            TransitionSchema {
                name: "Initialize".into(),
                from: vec!["Initializing".into()],
                on: InputMatch {
                    kind: meerkat_trigger_kind("Initialize"),
                    variant: "Initialize".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![],
                to: "Idle".into(),
                emit: vec![],
            },
            register_session_transition("RegisterSessionIdle", "Idle"),
            register_session_transition("RegisterSessionAttached", "Attached"),
            register_session_transition("RegisterSessionRunning", "Running"),
            register_session_transition("RegisterSessionRetired", "Retired"),
            register_session_transition("RegisterSessionStopped", "Stopped"),
            unregister_session_transition("UnregisterSessionIdle", "Idle"),
            unregister_session_transition("UnregisterSessionAttached", "Attached"),
            unregister_session_transition("UnregisterSessionRunning", "Running"),
            unregister_session_transition("UnregisterSessionRetired", "Retired"),
            unregister_session_transition("UnregisterSessionStopped", "Stopped"),
            TransitionSchema {
                name: "ReconfigureSessionLlmIdentityAttached".into(),
                from: vec!["Attached".into()],
                on: InputMatch {
                    kind: meerkat_trigger_kind("ReconfigureSessionLlmIdentity"),
                    variant: "ReconfigureSessionLlmIdentity".into(),
                    bindings: vec![
                        "previous_identity".into(),
                        "previous_visibility_state".into(),
                        "previous_capability_surface".into(),
                        "previous_capability_surface_status".into(),
                        "target_identity".into(),
                        "target_capability_surface".into(),
                        "next_visibility_state".into(),
                        "next_capability_base_filter".into(),
                        "next_active_visibility_revision".into(),
                        "tool_visibility_delta".into(),
                    ],
                },
                guards: vec![
                    session_registered_guard(),
                    runtime_is_bound_guard(),
                    reconfigure_visibility_revision_guard(),
                ],
                updates: vec![Update::Assign {
                    field: "active_visibility_revision".into(),
                    expr: Expr::Binding("next_active_visibility_revision".into()),
                }],
                to: "Attached".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "ReconfigureSessionLlmIdentityRunning".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    kind: meerkat_trigger_kind("ReconfigureSessionLlmIdentity"),
                    variant: "ReconfigureSessionLlmIdentity".into(),
                    bindings: vec![
                        "previous_identity".into(),
                        "previous_visibility_state".into(),
                        "previous_capability_surface".into(),
                        "previous_capability_surface_status".into(),
                        "target_identity".into(),
                        "target_capability_surface".into(),
                        "next_visibility_state".into(),
                        "next_capability_base_filter".into(),
                        "next_active_visibility_revision".into(),
                        "tool_visibility_delta".into(),
                    ],
                },
                guards: vec![
                    session_registered_guard(),
                    runtime_is_bound_guard(),
                    reconfigure_visibility_revision_guard(),
                ],
                updates: vec![Update::Assign {
                    field: "active_visibility_revision".into(),
                    expr: Expr::Binding("next_active_visibility_revision".into()),
                }],
                to: "Running".into(),
                emit: vec![],
            },
            stage_persistent_filter_transition("StagePersistentFilterIdle", "Idle"),
            stage_persistent_filter_transition("StagePersistentFilterAttached", "Attached"),
            stage_persistent_filter_transition("StagePersistentFilterRunning", "Running"),
            stage_persistent_filter_transition("StagePersistentFilterRetired", "Retired"),
            stage_persistent_filter_transition("StagePersistentFilterStopped", "Stopped"),
            request_deferred_tools_transition("RequestDeferredToolsIdle", "Idle"),
            request_deferred_tools_transition("RequestDeferredToolsAttached", "Attached"),
            request_deferred_tools_transition("RequestDeferredToolsRunning", "Running"),
            request_deferred_tools_transition("RequestDeferredToolsRetired", "Retired"),
            request_deferred_tools_transition("RequestDeferredToolsStopped", "Stopped"),
            // PrepareBindings: registration + query. A newly prepared idle
            // session becomes runtime-bound/Attached; all other accepted
            // phases preserve their current phase while recording the runtime
            // binding fields.
            prepare_bindings_transition(
                "PrepareBindingsInitializing",
                "Initializing",
                "Initializing",
            ),
            prepare_bindings_transition("PrepareBindingsIdle", "Idle", "Attached"),
            prepare_bindings_transition("PrepareBindingsAttached", "Attached", "Attached"),
            prepare_bindings_transition("PrepareBindingsRunning", "Running", "Running"),
            prepare_bindings_transition("PrepareBindingsRetired", "Retired", "Retired"),
            TransitionSchema {
                name: "PrepareBindingsStopped".into(),
                from: vec!["Stopped".into()],
                on: InputMatch {
                    kind: meerkat_trigger_kind("PrepareBindings"),
                    variant: "PrepareBindings".into(),
                    bindings: vec![
                        "agent_runtime_id".into(),
                        "fence_token".into(),
                        "generation".into(),
                    ],
                },
                guards: vec![],
                updates: vec![
                    assign_some("active_runtime_id", "agent_runtime_id"),
                    assign_some("active_fence_token", "fence_token"),
                    assign_some("active_generation", "generation"),
                ],
                to: "Stopped".into(),
                emit: vec![runtime_identity_emit("RuntimeBound")],
            },
            // SetPeerIngressContext: runtime accepts from any non-Destroyed
            // phase. Surfaces call this after register or prepare_bindings,
            // and during resume flows from various states.
            set_peer_ingress_transition("SetPeerIngressContextIdle", "Idle"),
            set_peer_ingress_transition("SetPeerIngressContextAttached", "Attached"),
            set_peer_ingress_transition("SetPeerIngressContextRunning", "Running"),
            set_peer_ingress_transition("SetPeerIngressContextRetired", "Retired"),
            set_peer_ingress_transition("SetPeerIngressContextStopped", "Stopped"),
            // NotifyDrainExited: the comms drain exits asynchronously, so the
            // notification can arrive after the session has moved to any phase.
            // Runtime rejects only Destroyed. Self-loop per accepted phase.
            TransitionSchema {
                name: "NotifyDrainExitedIdle".into(),
                from: vec!["Idle".into()],
                on: InputMatch {
                    kind: meerkat_trigger_kind("NotifyDrainExited"),
                    variant: "NotifyDrainExited".into(),
                    bindings: vec!["reason".into()],
                },
                guards: vec![session_registered_guard()],
                updates: vec![Update::Assign {
                    field: "drain_running".into(),
                    expr: Expr::Bool(false),
                }],
                to: "Idle".into(),
                emit: vec![notice_emit("drain", "drain exited")],
            },
            TransitionSchema {
                name: "NotifyDrainExitedAttached".into(),
                from: vec!["Attached".into()],
                on: InputMatch {
                    kind: meerkat_trigger_kind("NotifyDrainExited"),
                    variant: "NotifyDrainExited".into(),
                    bindings: vec!["reason".into()],
                },
                guards: vec![session_registered_guard()],
                updates: vec![Update::Assign {
                    field: "drain_running".into(),
                    expr: Expr::Bool(false),
                }],
                to: "Attached".into(),
                emit: vec![notice_emit("drain", "drain exited")],
            },
            TransitionSchema {
                name: "NotifyDrainExitedRunning".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    kind: meerkat_trigger_kind("NotifyDrainExited"),
                    variant: "NotifyDrainExited".into(),
                    bindings: vec!["reason".into()],
                },
                guards: vec![session_registered_guard()],
                updates: vec![Update::Assign {
                    field: "drain_running".into(),
                    expr: Expr::Bool(false),
                }],
                to: "Running".into(),
                emit: vec![notice_emit("drain", "drain exited")],
            },
            TransitionSchema {
                name: "NotifyDrainExitedRetired".into(),
                from: vec!["Retired".into()],
                on: InputMatch {
                    kind: meerkat_trigger_kind("NotifyDrainExited"),
                    variant: "NotifyDrainExited".into(),
                    bindings: vec!["reason".into()],
                },
                guards: vec![session_registered_guard()],
                updates: vec![Update::Assign {
                    field: "drain_running".into(),
                    expr: Expr::Bool(false),
                }],
                to: "Retired".into(),
                emit: vec![notice_emit("drain", "drain exited")],
            },
            TransitionSchema {
                name: "NotifyDrainExitedStopped".into(),
                from: vec!["Stopped".into()],
                on: InputMatch {
                    kind: meerkat_trigger_kind("NotifyDrainExited"),
                    variant: "NotifyDrainExited".into(),
                    bindings: vec!["reason".into()],
                },
                guards: vec![session_registered_guard()],
                updates: vec![Update::Assign {
                    field: "drain_running".into(),
                    expr: Expr::Bool(false),
                }],
                to: "Stopped".into(),
                emit: vec![notice_emit("drain", "drain exited")],
            },
            TransitionSchema {
                name: "InterruptCurrentRunAttached".into(),
                from: vec!["Attached".into()],
                on: InputMatch {
                    kind: meerkat_trigger_kind("InterruptCurrentRun"),
                    variant: "InterruptCurrentRun".into(),
                    bindings: vec![],
                },
                guards: vec![attachment_live_guard()],
                updates: vec![],
                to: "Attached".into(),
                emit: vec![
                    EffectEmit {
                        variant: "WakeInterrupt".into(),
                        fields: IndexMap::new(),
                    },
                    EffectEmit {
                        variant: "RequestCancellationAtBoundary".into(),
                        fields: IndexMap::new(),
                    },
                ],
            },
            TransitionSchema {
                name: "InterruptCurrentRun".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    kind: meerkat_trigger_kind("InterruptCurrentRun"),
                    variant: "InterruptCurrentRun".into(),
                    bindings: vec![],
                },
                guards: vec![attachment_live_guard()],
                updates: vec![],
                to: "Running".into(),
                emit: vec![
                    EffectEmit {
                        variant: "WakeInterrupt".into(),
                        fields: IndexMap::new(),
                    },
                    EffectEmit {
                        variant: "RequestCancellationAtBoundary".into(),
                        fields: IndexMap::new(),
                    },
                ],
            },
            TransitionSchema {
                name: "CancelAfterBoundaryAttached".into(),
                from: vec!["Attached".into()],
                on: InputMatch {
                    kind: meerkat_trigger_kind("CancelAfterBoundary"),
                    variant: "CancelAfterBoundary".into(),
                    bindings: vec![],
                },
                guards: vec![attachment_live_guard()],
                updates: vec![],
                to: "Attached".into(),
                emit: vec![EffectEmit {
                    variant: "RequestCancellationAtBoundary".into(),
                    fields: IndexMap::new(),
                }],
            },
            TransitionSchema {
                name: "CancelAfterBoundary".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    kind: meerkat_trigger_kind("CancelAfterBoundary"),
                    variant: "CancelAfterBoundary".into(),
                    bindings: vec![],
                },
                guards: vec![attachment_live_guard()],
                updates: vec![],
                to: "Running".into(),
                emit: vec![EffectEmit {
                    variant: "RequestCancellationAtBoundary".into(),
                    fields: IndexMap::new(),
                }],
            },
            TransitionSchema {
                name: "BoundaryAppliedPromote".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    kind: meerkat_trigger_kind("BoundaryApplied"),
                    variant: "BoundaryApplied".into(),
                    bindings: vec!["revision".into()],
                },
                guards: vec![
                    Guard {
                        name: "has_pending_visibility_promotion".into(),
                        expr: Expr::Call {
                            helper: "HasPendingVisibilityPromotion".into(),
                            args: vec![],
                        },
                    },
                    Guard {
                        name: "revision_matches_staged".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field("staged_visibility_revision".into())),
                            Box::new(Expr::Binding("revision".into())),
                        ),
                    },
                ],
                updates: vec![
                    Update::Assign {
                        field: "active_requested_deferred_names".into(),
                        expr: Expr::Field("staged_requested_deferred_names".into()),
                    },
                    Update::Assign {
                        field: "active_visibility_revision".into(),
                        expr: Expr::Field("staged_visibility_revision".into()),
                    },
                ],
                to: "Running".into(),
                emit: vec![EffectEmit {
                    variant: "CommittedVisibleSetPublished".into(),
                    fields: IndexMap::from([("revision".into(), Expr::Binding("revision".into()))]),
                }],
            },
            TransitionSchema {
                name: "BoundaryAppliedNoop".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    kind: meerkat_trigger_kind("BoundaryApplied"),
                    variant: "BoundaryApplied".into(),
                    bindings: vec!["revision".into()],
                },
                guards: vec![
                    Guard {
                        name: "no_pending_visibility_promotion".into(),
                        expr: Expr::Not(Box::new(Expr::Call {
                            helper: "HasPendingVisibilityPromotion".into(),
                            args: vec![],
                        })),
                    },
                    Guard {
                        name: "revision_not_ahead_of_active".into(),
                        expr: Expr::Lte(
                            Box::new(Expr::Binding("revision".into())),
                            Box::new(Expr::Field("active_visibility_revision".into())),
                        ),
                    },
                ],
                updates: vec![],
                to: "Running".into(),
                emit: vec![EffectEmit {
                    variant: "CommittedVisibleSetPublished".into(),
                    fields: IndexMap::from([("revision".into(), Expr::Binding("revision".into()))]),
                }],
            },
            publish_committed_visible_set_transition("PublishCommittedVisibleSetIdle", "Idle"),
            publish_committed_visible_set_transition(
                "PublishCommittedVisibleSetAttached",
                "Attached",
            ),
            publish_committed_visible_set_transition(
                "PublishCommittedVisibleSetRunning",
                "Running",
            ),
            publish_committed_visible_set_transition(
                "PublishCommittedVisibleSetRetired",
                "Retired",
            ),
            publish_committed_visible_set_transition(
                "PublishCommittedVisibleSetStopped",
                "Stopped",
            ),
            TransitionSchema {
                name: "RetireRequestedFromIdle".into(),
                from: vec!["Idle".into(), "Attached".into(), "Running".into()],
                on: InputMatch {
                    kind: meerkat_trigger_kind("Retire"),
                    variant: "Retire".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![],
                to: "Retired".into(),
                emit: vec![runtime_identity_emit("RuntimeRetired")],
            },
            TransitionSchema {
                name: "Reset".into(),
                from: vec![
                    "Initializing".into(),
                    "Idle".into(),
                    "Attached".into(),
                    "Retired".into(),
                ],
                on: InputMatch {
                    kind: meerkat_trigger_kind("Reset"),
                    variant: "Reset".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![
                    Update::Assign {
                        field: "current_run_id".into(),
                        expr: Expr::None,
                    },
                    Update::Assign {
                        field: "active_fence_token".into(),
                        expr: Expr::None,
                    },
                    Update::Assign {
                        field: "active_generation".into(),
                        expr: Expr::None,
                    },
                    Update::Assign {
                        field: "pre_run_phase".into(),
                        expr: Expr::None,
                    },
                    Update::Assign {
                        field: "drain_running".into(),
                        expr: Expr::Bool(false),
                    },
                    Update::Assign {
                        field: "silent_intent_overrides".into(),
                        expr: Expr::EmptySet,
                    },
                ],
                to: "Idle".into(),
                emit: vec![notice_emit("reset", "runtime reset")],
            },
            TransitionSchema {
                name: "StopRuntimeExecutorDetached".into(),
                from: vec![
                    "Initializing".into(),
                    "Idle".into(),
                    "Attached".into(),
                    "Running".into(),
                    "Retired".into(),
                ],
                on: InputMatch {
                    kind: meerkat_trigger_kind("StopRuntimeExecutor"),
                    variant: "StopRuntimeExecutor".into(),
                    bindings: vec![],
                },
                guards: vec![attachment_not_live_guard()],
                updates: vec![
                    Update::Assign {
                        field: "current_run_id".into(),
                        expr: Expr::None,
                    },
                    Update::Assign {
                        field: "pre_run_phase".into(),
                        expr: Expr::None,
                    },
                    Update::Assign {
                        field: "drain_running".into(),
                        expr: Expr::Bool(false),
                    },
                    Update::Assign {
                        field: "silent_intent_overrides".into(),
                        expr: Expr::EmptySet,
                    },
                ],
                to: "Stopped".into(),
                emit: vec![notice_emit("stop", "runtime executor stopped")],
            },
            TransitionSchema {
                name: "StopRuntimeExecutorLiveAttached".into(),
                from: vec!["Attached".into()],
                on: InputMatch {
                    kind: meerkat_trigger_kind("StopRuntimeExecutor"),
                    variant: "StopRuntimeExecutor".into(),
                    bindings: vec![],
                },
                guards: vec![attachment_live_guard()],
                updates: vec![
                    Update::Assign {
                        field: "drain_running".into(),
                        expr: Expr::Bool(false),
                    },
                    Update::Assign {
                        field: "silent_intent_overrides".into(),
                        expr: Expr::EmptySet,
                    },
                ],
                to: "Attached".into(),
                emit: vec![notice_emit("stop", "runtime executor stopped")],
            },
            TransitionSchema {
                name: "StopRuntimeExecutorLiveRunning".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    kind: meerkat_trigger_kind("StopRuntimeExecutor"),
                    variant: "StopRuntimeExecutor".into(),
                    bindings: vec![],
                },
                guards: vec![attachment_live_guard()],
                updates: vec![
                    Update::Assign {
                        field: "drain_running".into(),
                        expr: Expr::Bool(false),
                    },
                    Update::Assign {
                        field: "silent_intent_overrides".into(),
                        expr: Expr::EmptySet,
                    },
                ],
                to: "Running".into(),
                emit: vec![notice_emit("stop", "runtime executor stopped")],
            },
            TransitionSchema {
                name: "Destroy".into(),
                from: vec![
                    "Initializing".into(),
                    "Idle".into(),
                    "Attached".into(),
                    "Running".into(),
                    "Retired".into(),
                    "Stopped".into(),
                ],
                on: InputMatch {
                    kind: meerkat_trigger_kind("Destroy"),
                    variant: "Destroy".into(),
                    bindings: vec![],
                },
                guards: vec![runtime_is_bound_guard()],
                updates: vec![
                    Update::Assign {
                        field: "current_run_id".into(),
                        expr: Expr::None,
                    },
                    Update::Assign {
                        field: "pre_run_phase".into(),
                        expr: Expr::None,
                    },
                    Update::Assign {
                        field: "drain_running".into(),
                        expr: Expr::Bool(false),
                    },
                    Update::Assign {
                        field: "silent_intent_overrides".into(),
                        expr: Expr::EmptySet,
                    },
                ],
                to: "Destroyed".into(),
                emit: vec![runtime_identity_emit("RuntimeDestroyed")],
            },
        ]
        .into_iter()
        .chain(absorbed_meerkat_transitions())
        .collect(),
        ci_step_limit: Some(8),
        effect_dispositions: vec![
            routed_disposition("RuntimeBound", &["MobMachine"]),
            routed_disposition("RuntimeRetired", &["MobMachine"]),
            routed_disposition("RuntimeDestroyed", &["MobMachine"]),
            routed_disposition("WorkCompleted", &["MobMachine"]),
            routed_disposition("WorkFailed", &["MobMachine"]),
            routed_disposition("WorkCancelled", &["MobMachine"]),
            local_disposition("RequestCancellationAtBoundary"),
            local_disposition("WakeInterrupt"),
            external_disposition("CommittedVisibleSetPublished"),
            external_disposition("RuntimeNotice"),
        ]
        .into_iter()
        .chain(absorbed_meerkat_effect_dispositions())
        .collect(),
    }
}

fn reset_session_state() -> Vec<Update> {
    vec![
        Update::Assign {
            field: "session_id".into(),
            expr: Expr::None,
        },
        Update::Assign {
            field: "active_runtime_id".into(),
            expr: Expr::None,
        },
        Update::Assign {
            field: "active_fence_token".into(),
            expr: Expr::None,
        },
        Update::Assign {
            field: "active_generation".into(),
            expr: Expr::None,
        },
        Update::Assign {
            field: "attachment_live".into(),
            expr: Expr::Bool(false),
        },
        Update::Assign {
            field: "current_run_id".into(),
            expr: Expr::None,
        },
        Update::Assign {
            field: "pre_run_phase".into(),
            expr: Expr::None,
        },
        Update::Assign {
            field: "peer_ingress_configured".into(),
            expr: Expr::Bool(false),
        },
        Update::Assign {
            field: "drain_running".into(),
            expr: Expr::Bool(false),
        },
        Update::Assign {
            field: "active_requested_deferred_names".into(),
            expr: Expr::EmptySet,
        },
        Update::Assign {
            field: "staged_requested_deferred_names".into(),
            expr: Expr::EmptySet,
        },
        Update::Assign {
            field: "active_visibility_revision".into(),
            expr: Expr::U64(0),
        },
        Update::Assign {
            field: "staged_visibility_revision".into(),
            expr: Expr::U64(0),
        },
    ]
}

fn runtime_identity_fields() -> Vec<FieldSchema> {
    vec![
        field("agent_runtime_id", TypeRef::Named("AgentRuntimeId".into())),
        field("fence_token", TypeRef::Named("FenceToken".into())),
        field("generation", TypeRef::Named("Generation".into())),
    ]
}

fn work_identity_fields() -> Vec<FieldSchema> {
    let mut fields = runtime_identity_fields();
    fields.push(field("work_id", TypeRef::Named("WorkId".into())));
    fields
}

fn runtime_identity_emit(variant: &str) -> EffectEmit {
    EffectEmit {
        variant: variant.into(),
        fields: IndexMap::from([
            ("agent_runtime_id".into(), option_value("active_runtime_id")),
            ("fence_token".into(), option_value("active_fence_token")),
            ("generation".into(), option_value("active_generation")),
        ]),
    }
}

fn notice_emit(kind: &str, detail: &str) -> EffectEmit {
    EffectEmit {
        variant: "RuntimeNotice".into(),
        fields: IndexMap::from([
            ("kind".into(), Expr::String(kind.into())),
            ("detail".into(), Expr::String(detail.into())),
        ]),
    }
}

fn variant(name: &str) -> VariantSchema {
    VariantSchema {
        name: name.into(),
        fields: vec![],
    }
}

fn field(name: &str, ty: TypeRef) -> FieldSchema {
    FieldSchema {
        name: name.into(),
        ty,
    }
}

fn init(field: &str, expr: Expr) -> crate::FieldInit {
    crate::FieldInit {
        field: field.into(),
        expr,
    }
}

fn named(name: &str) -> TypeRef {
    TypeRef::Named(name.into())
}

fn register_session_transition(name: &str, phase: &str) -> TransitionSchema {
    TransitionSchema {
        name: name.into(),
        from: vec![phase.into()],
        on: InputMatch {
            kind: meerkat_trigger_kind("RegisterSession"),
            variant: "RegisterSession".into(),
            bindings: vec!["session_id".into()],
        },
        // Runtime treats registration as an idempotent binding helper for an
        // already-live session and as a create helper for an unknown session.
        guards: vec![],
        updates: vec![assign_some("session_id", "session_id")],
        to: phase.into(),
        emit: vec![],
    }
}

fn unregister_session_transition(name: &str, from_phase: &str) -> TransitionSchema {
    TransitionSchema {
        name: name.into(),
        from: vec![from_phase.into()],
        on: InputMatch {
            kind: meerkat_trigger_kind("UnregisterSession"),
            variant: "UnregisterSession".into(),
            bindings: vec!["session_id".into()],
        },
        guards: vec![session_matches_guard(
            "session_matches_current",
            "session_id",
        )],
        updates: reset_session_state(),
        to: "Idle".into(),
        emit: vec![],
    }
}

fn prepare_bindings_transition(name: &str, from_phase: &str, to_phase: &str) -> TransitionSchema {
    TransitionSchema {
        name: name.into(),
        from: vec![from_phase.into()],
        on: InputMatch {
            kind: meerkat_trigger_kind("PrepareBindings"),
            variant: "PrepareBindings".into(),
            bindings: vec![
                "agent_runtime_id".into(),
                "fence_token".into(),
                "generation".into(),
            ],
        },
        // No guard: runtime does not require prior registration —
        // it calls register_session_inner itself.
        guards: vec![],
        updates: vec![
            assign_some("active_runtime_id", "agent_runtime_id"),
            assign_some("active_fence_token", "fence_token"),
            assign_some("active_generation", "generation"),
        ],
        to: to_phase.into(),
        emit: vec![runtime_identity_emit("RuntimeBound")],
    }
}

fn stage_persistent_filter_transition(name: &str, phase: &str) -> TransitionSchema {
    TransitionSchema {
        name: name.into(),
        from: vec![phase.into()],
        on: InputMatch {
            kind: meerkat_trigger_kind("StagePersistentFilter"),
            variant: "StagePersistentFilter".into(),
            bindings: vec!["filter".into(), "witnesses".into()],
        },
        guards: vec![session_registered_guard()],
        updates: vec![Update::Assign {
            field: "staged_visibility_revision".into(),
            expr: next_staged_visibility_revision_expr(),
        }],
        to: phase.into(),
        emit: vec![],
    }
}

fn request_deferred_tools_transition(name: &str, phase: &str) -> TransitionSchema {
    TransitionSchema {
        name: name.into(),
        from: vec![phase.into()],
        on: InputMatch {
            kind: meerkat_trigger_kind("RequestDeferredTools"),
            variant: "RequestDeferredTools".into(),
            bindings: vec!["names".into(), "witnesses".into()],
        },
        guards: vec![session_registered_guard()],
        updates: vec![
            Update::ForEach {
                binding: "name".into(),
                over: Expr::Binding("names".into()),
                updates: vec![Update::SetInsert {
                    field: "staged_requested_deferred_names".into(),
                    value: Expr::Binding("name".into()),
                }],
            },
            Update::Assign {
                field: "staged_visibility_revision".into(),
                expr: next_staged_visibility_revision_expr(),
            },
        ],
        to: phase.into(),
        emit: vec![],
    }
}

fn publish_committed_visible_set_transition(name: &str, phase: &str) -> TransitionSchema {
    TransitionSchema {
        name: name.into(),
        from: vec![phase.into()],
        on: InputMatch {
            kind: meerkat_trigger_kind("PublishCommittedVisibleSet"),
            variant: "PublishCommittedVisibleSet".into(),
            bindings: vec![
                "active_filter".into(),
                "staged_filter".into(),
                "active_requested_deferred_names".into(),
                "staged_requested_deferred_names".into(),
                "active_visibility_revision".into(),
                "staged_visibility_revision".into(),
            ],
        },
        guards: vec![
            session_registered_guard(),
            Guard {
                name: "active_not_behind_staged".into(),
                expr: Expr::Gte(
                    Box::new(Expr::Binding("active_visibility_revision".into())),
                    Box::new(Expr::Binding("staged_visibility_revision".into())),
                ),
            },
            Guard {
                name: "active_requested_names_subset_of_staged_input".into(),
                expr: Expr::Quantified {
                    quantifier: Quantifier::All,
                    binding: "name".into(),
                    over: Box::new(Expr::Binding("active_requested_deferred_names".into())),
                    body: Box::new(Expr::Contains {
                        collection: Box::new(Expr::Binding(
                            "staged_requested_deferred_names".into(),
                        )),
                        value: Box::new(Expr::Binding("name".into())),
                    }),
                },
            },
            Guard {
                name: "equal_revision_requires_equal_active_and_staged_input".into(),
                expr: Expr::Or(vec![
                    Expr::Neq(
                        Box::new(Expr::Binding("active_visibility_revision".into())),
                        Box::new(Expr::Binding("staged_visibility_revision".into())),
                    ),
                    Expr::And(vec![
                        Expr::Eq(
                            Box::new(Expr::Binding("active_filter".into())),
                            Box::new(Expr::Binding("staged_filter".into())),
                        ),
                        Expr::Eq(
                            Box::new(Expr::Binding("active_requested_deferred_names".into())),
                            Box::new(Expr::Binding("staged_requested_deferred_names".into())),
                        ),
                    ]),
                ]),
            },
        ],
        updates: vec![
            Update::Assign {
                field: "active_requested_deferred_names".into(),
                expr: Expr::Binding("active_requested_deferred_names".into()),
            },
            Update::Assign {
                field: "staged_requested_deferred_names".into(),
                expr: Expr::Binding("staged_requested_deferred_names".into()),
            },
            Update::Assign {
                field: "active_visibility_revision".into(),
                expr: Expr::Binding("active_visibility_revision".into()),
            },
            Update::Assign {
                field: "staged_visibility_revision".into(),
                expr: Expr::Binding("staged_visibility_revision".into()),
            },
        ],
        to: phase.into(),
        emit: vec![EffectEmit {
            variant: "CommittedVisibleSetPublished".into(),
            fields: IndexMap::from([(
                "revision".into(),
                Expr::Binding("active_visibility_revision".into()),
            )]),
        }],
    }
}

fn next_staged_visibility_revision_expr() -> Expr {
    Expr::Add(
        Box::new(Expr::IfElse {
            condition: Box::new(Expr::Gt(
                Box::new(Expr::Field("active_visibility_revision".into())),
                Box::new(Expr::Field("staged_visibility_revision".into())),
            )),
            then_expr: Box::new(Expr::Field("active_visibility_revision".into())),
            else_expr: Box::new(Expr::Field("staged_visibility_revision".into())),
        }),
        Box::new(Expr::U64(1)),
    )
}

fn reconfigure_visibility_revision_guard() -> Guard {
    Guard {
        name: "reconfigure_visibility_revision_is_stable_or_bumped".into(),
        expr: Expr::Or(vec![
            Expr::Eq(
                Box::new(Expr::Binding("next_active_visibility_revision".into())),
                Box::new(Expr::Field("active_visibility_revision".into())),
            ),
            Expr::Eq(
                Box::new(Expr::Binding("next_active_visibility_revision".into())),
                Box::new(next_staged_visibility_revision_expr()),
            ),
        ]),
    }
}

fn set_peer_ingress_transition(name: &str, phase: &str) -> TransitionSchema {
    TransitionSchema {
        name: name.into(),
        from: vec![phase.into()],
        on: InputMatch {
            kind: meerkat_trigger_kind("SetPeerIngressContext"),
            variant: "SetPeerIngressContext".into(),
            bindings: vec!["keep_alive".into()],
        },
        guards: vec![session_registered_guard()],
        updates: vec![
            Update::Assign {
                field: "peer_ingress_configured".into(),
                expr: Expr::Bool(true),
            },
            Update::Assign {
                field: "drain_running".into(),
                expr: Expr::Binding("keep_alive".into()),
            },
        ],
        to: phase.into(),
        emit: vec![],
    }
}

fn session_registered_guard() -> Guard {
    Guard {
        name: "session_registered".into(),
        expr: Expr::Neq(
            Box::new(Expr::Field("session_id".into())),
            Box::new(Expr::None),
        ),
    }
}

fn session_matches_guard(name: &str, binding: &str) -> Guard {
    Guard {
        name: name.into(),
        expr: Expr::Eq(
            Box::new(Expr::Field("session_id".into())),
            Box::new(Expr::Some(Box::new(Expr::Binding(binding.into())))),
        ),
    }
}

fn runtime_is_bound_guard() -> Guard {
    Guard {
        name: "runtime_is_bound".into(),
        expr: Expr::Neq(
            Box::new(Expr::Field("active_runtime_id".into())),
            Box::new(Expr::None),
        ),
    }
}

fn attachment_live_guard() -> Guard {
    Guard {
        name: "attachment_live".into(),
        expr: Expr::Field("attachment_live".into()),
    }
}

fn attachment_not_live_guard() -> Guard {
    Guard {
        name: "attachment_not_live".into(),
        expr: Expr::Not(Box::new(Expr::Field("attachment_live".into()))),
    }
}

fn pre_run_phase_matches_guard(phase: &str) -> Guard {
    Guard {
        name: format!("pre_run_phase_matches_{phase}").to_lowercase(),
        expr: Expr::Eq(
            Box::new(Expr::Field("pre_run_phase".into())),
            Box::new(Expr::Some(Box::new(Expr::String(phase.to_lowercase())))),
        ),
    }
}

fn current_run_id_matches_guard(binding: &str) -> Guard {
    Guard {
        name: "current_run_id_matches_binding".into(),
        expr: Expr::Eq(
            Box::new(Expr::Field("current_run_id".into())),
            Box::new(Expr::Some(Box::new(Expr::Binding(binding.into())))),
        ),
    }
}

fn bool_binding_guard(name: &str, binding: &str, value: bool) -> Guard {
    Guard {
        name: name.into(),
        expr: Expr::Eq(
            Box::new(Expr::Binding(binding.into())),
            Box::new(Expr::Bool(value)),
        ),
    }
}

fn assign_some(field: &str, binding: &str) -> Update {
    Update::Assign {
        field: field.into(),
        expr: Expr::Some(Box::new(Expr::Binding(binding.into()))),
    }
}

fn option_value(field: &str) -> Expr {
    Expr::MapGet {
        map: Box::new(Expr::Field(field.into())),
        key: Box::new(Expr::String("value".into())),
    }
}

fn local_disposition(effect_variant: &str) -> EffectDispositionRule {
    EffectDispositionRule {
        effect_variant: effect_variant.into(),
        disposition: EffectDisposition::Local,
        handoff_protocol: None,
    }
}

fn external_disposition(effect_variant: &str) -> EffectDispositionRule {
    EffectDispositionRule {
        effect_variant: effect_variant.into(),
        disposition: EffectDisposition::External,
        handoff_protocol: None,
    }
}

fn routed_disposition(effect_variant: &str, consumers: &[&str]) -> EffectDispositionRule {
    EffectDispositionRule {
        effect_variant: effect_variant.into(),
        disposition: EffectDisposition::Routed {
            consumer_machines: consumers.iter().map(|item| (*item).into()).collect(),
        },
        handoff_protocol: None,
    }
}

fn direct_meerkat_trigger_variants() -> Vec<VariantSchema> {
    vec![
        variant("Initialize"),
        VariantSchema {
            name: "RegisterSession".into(),
            fields: vec![field("session_id", TypeRef::Named("SessionId".into()))],
        },
        VariantSchema {
            name: "UnregisterSession".into(),
            fields: vec![field("session_id", TypeRef::Named("SessionId".into()))],
        },
        VariantSchema {
            name: "ReconfigureSessionLlmIdentity".into(),
            fields: vec![
                field("previous_identity", named("SessionLlmIdentity")),
                field(
                    "previous_visibility_state",
                    named("SessionToolVisibilityState"),
                ),
                field(
                    "previous_capability_surface",
                    TypeRef::Option(Box::new(named("SessionLlmCapabilitySurface"))),
                ),
                field(
                    "previous_capability_surface_status",
                    named("SessionLlmCapabilitySurfaceStatus"),
                ),
                field("target_identity", named("SessionLlmIdentity")),
                field(
                    "target_capability_surface",
                    named("SessionLlmCapabilitySurface"),
                ),
                field("next_visibility_state", named("SessionToolVisibilityState")),
                field("next_capability_base_filter", named("ToolFilter")),
                field("next_active_visibility_revision", TypeRef::U64),
                field("tool_visibility_delta", named("SessionToolVisibilityDelta")),
            ],
        },
        VariantSchema {
            name: "PrepareBindings".into(),
            fields: vec![
                field("agent_runtime_id", TypeRef::Named("AgentRuntimeId".into())),
                field("fence_token", TypeRef::Named("FenceToken".into())),
                field("generation", TypeRef::Named("Generation".into())),
            ],
        },
        VariantSchema {
            name: "SetPeerIngressContext".into(),
            fields: vec![field("keep_alive", TypeRef::Bool)],
        },
        VariantSchema {
            name: "NotifyDrainExited".into(),
            fields: vec![field("reason", TypeRef::String)],
        },
        variant("InterruptCurrentRun"),
        variant("CancelAfterBoundary"),
        VariantSchema {
            name: "StagePersistentFilter".into(),
            fields: vec![
                field("filter", named("ToolFilter")),
                field(
                    "witnesses",
                    TypeRef::Map(
                        Box::new(TypeRef::String),
                        Box::new(named("ToolVisibilityWitness")),
                    ),
                ),
            ],
        },
        VariantSchema {
            name: "RequestDeferredTools".into(),
            fields: vec![
                field("names", TypeRef::Set(Box::new(TypeRef::String))),
                field(
                    "witnesses",
                    TypeRef::Map(
                        Box::new(TypeRef::String),
                        Box::new(named("ToolVisibilityWitness")),
                    ),
                ),
            ],
        },
        VariantSchema {
            name: "PublishCommittedVisibleSet".into(),
            fields: vec![
                field("active_filter", named("ToolFilter")),
                field("staged_filter", named("ToolFilter")),
                field(
                    "active_requested_deferred_names",
                    TypeRef::Set(Box::new(TypeRef::String)),
                ),
                field(
                    "staged_requested_deferred_names",
                    TypeRef::Set(Box::new(TypeRef::String)),
                ),
                field("active_visibility_revision", TypeRef::U64),
                field("staged_visibility_revision", TypeRef::U64),
            ],
        },
        VariantSchema {
            name: "BoundaryApplied".into(),
            fields: vec![field("revision", TypeRef::U64)],
        },
        variant("Recover"),
        variant("Retire"),
        variant("Reset"),
        variant("StopRuntimeExecutor"),
        variant("Destroy"),
    ]
}

fn split_variants(
    variants: Vec<VariantSchema>,
    keep_input: impl Fn(&str) -> bool,
) -> (Vec<VariantSchema>, Vec<VariantSchema>) {
    let mut inputs = Vec::new();
    let mut signals = Vec::new();
    for variant in variants {
        if keep_input(&variant.name) {
            inputs.push(variant);
        } else {
            signals.push(variant);
        }
    }
    (inputs, signals)
}

fn is_meerkat_runtime_input_variant(variant: &str) -> bool {
    matches!(
        variant,
        "RegisterSession"
            | "UnregisterSession"
            | "EnsureSessionWithExecutor"
            | "SetSilentIntents"
            | "InterruptCurrentRun"
            | "CancelAfterBoundary"
            | "StopRuntimeExecutor"
            | "ContainsSession"
            | "SessionHasExecutor"
            | "SessionHasComms"
            | "OpsLifecycleRegistry"
            | "ReconfigureSessionLlmIdentity"
            | "PrepareBindings"
            | "InputState"
            | "ListActiveInputs"
            | "StagePersistentFilter"
            | "RequestDeferredTools"
            | "PublishCommittedVisibleSet"
            | "Ingest"
            | "PublishEvent"
            | "Retire"
            | "Recycle"
            | "Reset"
            | "Recover"
            | "Destroy"
            | "RuntimeState"
            | "LoadBoundaryReceipt"
            | "SetPeerIngressContext"
            | "NotifyDrainExited"
            | "AcceptWithCompletion"
            | "AcceptWithoutWake"
            | "AbortAll"
            | "Abort"
            | "Wait"
            | "Prepare"
            | "Commit"
            | "Fail"
    )
}

fn absorbed_meerkat_input_variants() -> Vec<VariantSchema> {
    vec![
        VariantSchema {
            name: "EnsureSessionWithExecutor".into(),
            fields: vec![field("session_id", named("SessionId"))],
        },
        VariantSchema {
            name: "SetSilentIntents".into(),
            fields: vec![
                field("session_id", named("SessionId")),
                field("intents", TypeRef::Set(Box::new(TypeRef::String))),
            ],
        },
        VariantSchema {
            name: "ContainsSession".into(),
            fields: vec![field("session_id", named("SessionId"))],
        },
        VariantSchema {
            name: "SessionHasExecutor".into(),
            fields: vec![field("session_id", named("SessionId"))],
        },
        VariantSchema {
            name: "SessionHasComms".into(),
            fields: vec![field("session_id", named("SessionId"))],
        },
        VariantSchema {
            name: "OpsLifecycleRegistry".into(),
            fields: vec![field("session_id", named("SessionId"))],
        },
        VariantSchema {
            name: "InputState".into(),
            fields: vec![
                field("session_id", named("SessionId")),
                field("input_id", named("InputId")),
            ],
        },
        VariantSchema {
            name: "ListActiveInputs".into(),
            fields: vec![field("session_id", named("SessionId"))],
        },
        VariantSchema {
            name: "Abort".into(),
            fields: vec![field("session_id", named("SessionId"))],
        },
        variant("AbortAll"),
        VariantSchema {
            name: "Wait".into(),
            fields: vec![field("session_id", named("SessionId"))],
        },
        VariantSchema {
            name: "Ingest".into(),
            fields: vec![field("runtime_id", named("AgentRuntimeId"))],
        },
        VariantSchema {
            name: "PublishEvent".into(),
            fields: vec![field("kind", TypeRef::String)],
        },
        VariantSchema {
            name: "RuntimeState".into(),
            fields: vec![field("runtime_id", TypeRef::String)],
        },
        VariantSchema {
            name: "LoadBoundaryReceipt".into(),
            fields: vec![
                field("runtime_id", TypeRef::String),
                field("sequence", TypeRef::U64),
            ],
        },
        VariantSchema {
            name: "AcceptWithCompletion".into(),
            fields: vec![
                field("input_id", named("InputId")),
                field("request_immediate_processing", TypeRef::Bool),
                field("interrupt_yielding", TypeRef::Bool),
                field("run_id", named("RunId")),
            ],
        },
        VariantSchema {
            name: "AcceptWithoutWake".into(),
            fields: vec![field("input_id", named("InputId"))],
        },
        VariantSchema {
            name: "Prepare".into(),
            fields: vec![
                field("session_id", named("SessionId")),
                field("run_id", named("RunId")),
            ],
        },
        VariantSchema {
            name: "DrainQueuedRun".into(),
            fields: vec![field("run_id", named("RunId"))],
        },
        VariantSchema {
            name: "Commit".into(),
            fields: vec![
                field("input_id", named("InputId")),
                field("run_id", named("RunId")),
            ],
        },
        VariantSchema {
            name: "Fail".into(),
            fields: vec![field("run_id", named("RunId"))],
        },
        variant("StartConversationRun"),
        variant("StartImmediateAppend"),
        variant("StartImmediateContext"),
        variant("ClassifyExternalEnvelope"),
        variant("ClassifyPlainEvent"),
        variant("EnsureDrainRunning"),
        variant("StageAdd"),
        variant("StageRemove"),
        variant("StageReload"),
        variant("ApplySurfaceBoundary"),
        variant("PendingSucceeded"),
        variant("PendingFailed"),
        variant("CallStarted"),
        variant("CallFinished"),
        variant("FinalizeRemovalClean"),
        variant("FinalizeRemovalForced"),
        variant("SnapshotAligned"),
        variant("ShutdownSurface"),
        variant("Recycle"),
    ]
}

fn absorbed_meerkat_transitions() -> Vec<TransitionSchema> {
    let mut transitions = Vec::new();

    transitions.push(TransitionSchema {
        name: "EnsureSessionWithExecutorIdle".into(),
        from: vec!["Idle".into()],
        on: InputMatch {
            kind: meerkat_trigger_kind("EnsureSessionWithExecutor"),
            variant: "EnsureSessionWithExecutor".into(),
            bindings: vec!["session_id".into()],
        },
        guards: vec![],
        updates: vec![Update::Assign {
            field: "attachment_live".into(),
            expr: Expr::Bool(true),
        }],
        to: "Attached".into(),
        emit: vec![],
    });

    for phase in ["Attached", "Running"] {
        transitions.push(self_loop_transition_with(
            &format!("EnsureSessionWithExecutor{phase}"),
            phase,
            "EnsureSessionWithExecutor",
            vec!["session_id"],
            vec![Update::Assign {
                field: "attachment_live".into(),
                expr: Expr::Bool(true),
            }],
            vec![],
            vec![],
        ));
    }

    for phase in ["Retired", "Stopped"] {
        transitions.push(self_loop_transition_with(
            &format!("EnsureSessionWithExecutor{phase}"),
            phase,
            "EnsureSessionWithExecutor",
            vec!["session_id"],
            vec![],
            vec![],
            vec![],
        ));
    }

    for phase in ["Idle", "Attached", "Running", "Retired"] {
        transitions.push(self_loop_transition_with(
            &format!("SetSilentIntents{phase}"),
            phase,
            "SetSilentIntents",
            vec!["session_id", "intents"],
            vec![Update::Assign {
                field: "silent_intent_overrides".into(),
                expr: Expr::Binding("intents".into()),
            }],
            vec![],
            vec![session_registered_guard()],
        ));
    }
    transitions.push(self_loop_transition_with(
        "SetSilentIntentsStopped",
        "Stopped",
        "SetSilentIntents",
        vec!["session_id", "intents"],
        vec![],
        vec![],
        vec![session_registered_guard()],
    ));

    // Abort/Wait only require a registered session in the runtime; they are
    // local drain-management no-ops when no drain slot is present.
    for (variant, bindings) in [("Abort", vec!["session_id"]), ("Wait", vec!["session_id"])] {
        transitions.push(self_loop_transition_with(
            &format!("{variant}Idle"),
            "Idle",
            variant,
            bindings.clone(),
            vec![],
            vec![],
            vec![session_registered_guard()],
        ));
        transitions.push(self_loop_transition_with(
            &format!("{variant}Attached"),
            "Attached",
            variant,
            bindings.clone(),
            vec![],
            vec![],
            vec![session_registered_guard()],
        ));
        transitions.push(self_loop_transition_with(
            &format!("{variant}Running"),
            "Running",
            variant,
            bindings.clone(),
            vec![],
            vec![],
            vec![session_registered_guard()],
        ));
        transitions.push(self_loop_transition_with(
            &format!("{variant}Retired"),
            "Retired",
            variant,
            bindings.clone(),
            vec![],
            vec![],
            vec![session_registered_guard()],
        ));
        transitions.push(self_loop_transition_with(
            &format!("{variant}Stopped"),
            "Stopped",
            variant,
            bindings,
            vec![],
            vec![],
            vec![session_registered_guard()],
        ));
    }

    // AbortAll is a global drain cleanup no-op when no slots are present.
    for phase in ["Idle", "Attached", "Running", "Retired", "Stopped"] {
        transitions.push(self_loop_transition_with(
            &format!("AbortAll{phase}"),
            phase,
            "AbortAll",
            vec![],
            vec![Update::Assign {
                field: "drain_running".into(),
                expr: Expr::Bool(false),
            }],
            vec![],
            vec![],
        ));
    }

    for phase in ["Attached", "Running"] {
        transitions.push(self_loop_transition_with(
            &format!("EnsureDrainRunning{phase}"),
            phase,
            "EnsureDrainRunning",
            vec![],
            vec![Update::Assign {
                field: "drain_running".into(),
                expr: Expr::Bool(true),
            }],
            vec![simple_emit("SpawnDrainTask")],
            vec![
                session_registered_guard(),
                Guard {
                    name: "peer_ingress_configured".into(),
                    expr: Expr::Eq(
                        Box::new(Expr::Field("peer_ingress_configured".into())),
                        Box::new(Expr::Bool(true)),
                    ),
                },
            ],
        ));
    }

    // Ingest rejects only Retired/Stopped/Destroyed in runtime.
    for phase in ["Idle", "Attached", "Running"] {
        transitions.push(self_loop_transition_with(
            &format!("Ingest{phase}"),
            phase,
            "Ingest",
            vec!["runtime_id"],
            vec![],
            vec![simple_emit("ResolveAdmission")],
            vec![session_registered_guard()],
        ));
    }

    // PublishEvent is a pass-through runtime notice for any non-Destroyed
    // registered session.
    for phase in ["Idle", "Attached", "Running", "Retired", "Stopped"] {
        transitions.push(self_loop_transition_with(
            &format!("PublishEvent{phase}"),
            phase,
            "PublishEvent",
            vec!["kind"],
            vec![],
            vec![simple_emit("IngressNotice")],
            vec![session_registered_guard()],
        ));
    }

    // Input admission through the helper surface currently accepts from Idle,
    // Attached, and Running, while Retired/Stopped/Destroyed reject.
    transitions.push(self_loop_transition_with(
        "AcceptWithCompletionIdleQueued",
        "Idle",
        "AcceptWithCompletion",
        vec![
            "input_id",
            "request_immediate_processing",
            "interrupt_yielding",
            "run_id",
        ],
        vec![],
        vec![
            simple_emit("IngressAccepted"),
            post_admission_signal_emit("WakeLoop"),
        ],
        vec![
            session_registered_guard(),
            bool_binding_guard(
                "request_immediate_processing",
                "request_immediate_processing",
                false,
            ),
            bool_binding_guard("interrupt_yielding", "interrupt_yielding", false),
        ],
    ));
    transitions.push(self_loop_transition_with(
        "AcceptWithCompletionIdleImmediate",
        "Idle",
        "AcceptWithCompletion",
        vec![
            "input_id",
            "request_immediate_processing",
            "interrupt_yielding",
            "run_id",
        ],
        vec![],
        vec![
            simple_emit("IngressAccepted"),
            post_admission_signal_emit("RequestImmediateProcessing"),
        ],
        vec![
            session_registered_guard(),
            bool_binding_guard(
                "request_immediate_processing",
                "request_immediate_processing",
                true,
            ),
            bool_binding_guard("interrupt_yielding", "interrupt_yielding", false),
        ],
    ));
    transitions.push(TransitionSchema {
        name: "AcceptWithCompletionAttachedImmediate".into(),
        from: vec!["Attached".into()],
        on: InputMatch {
            kind: meerkat_trigger_kind("AcceptWithCompletion"),
            variant: "AcceptWithCompletion".into(),
            bindings: vec![
                "input_id".into(),
                "request_immediate_processing".into(),
                "interrupt_yielding".into(),
                "run_id".into(),
            ],
        },
        guards: vec![
            session_registered_guard(),
            bool_binding_guard(
                "request_immediate_processing",
                "request_immediate_processing",
                true,
            ),
            bool_binding_guard("interrupt_yielding", "interrupt_yielding", false),
        ],
        updates: vec![
            assign_some("current_run_id", "run_id"),
            Update::Assign {
                field: "pre_run_phase".into(),
                expr: Expr::Some(Box::new(Expr::String("attached".into()))),
            },
        ],
        to: "Running".into(),
        emit: vec![
            simple_emit("IngressAccepted"),
            post_admission_signal_emit("RequestImmediateProcessing"),
            simple_emit("SubmitRunPrimitive"),
        ],
    });
    transitions.push(self_loop_transition_with(
        "AcceptWithCompletionAttachedQueued",
        "Attached",
        "AcceptWithCompletion",
        vec![
            "input_id",
            "request_immediate_processing",
            "interrupt_yielding",
            "run_id",
        ],
        vec![],
        vec![
            simple_emit("IngressAccepted"),
            post_admission_signal_emit("WakeLoop"),
        ],
        vec![
            session_registered_guard(),
            bool_binding_guard(
                "request_immediate_processing",
                "request_immediate_processing",
                false,
            ),
            bool_binding_guard("interrupt_yielding", "interrupt_yielding", false),
        ],
    ));
    transitions.push(self_loop_transition_with(
        "AcceptWithCompletionRunningQueuedPassive",
        "Running",
        "AcceptWithCompletion",
        vec![
            "input_id",
            "request_immediate_processing",
            "interrupt_yielding",
            "run_id",
        ],
        vec![],
        vec![simple_emit("IngressAccepted")],
        vec![
            session_registered_guard(),
            bool_binding_guard(
                "request_immediate_processing",
                "request_immediate_processing",
                false,
            ),
            bool_binding_guard("interrupt_yielding", "interrupt_yielding", false),
        ],
    ));
    transitions.push(self_loop_transition_with(
        "AcceptWithCompletionRunningInterruptYielding",
        "Running",
        "AcceptWithCompletion",
        vec![
            "input_id",
            "request_immediate_processing",
            "interrupt_yielding",
            "run_id",
        ],
        vec![],
        vec![
            simple_emit("IngressAccepted"),
            post_admission_signal_emit("InterruptYielding"),
        ],
        vec![
            session_registered_guard(),
            bool_binding_guard(
                "request_immediate_processing",
                "request_immediate_processing",
                false,
            ),
            bool_binding_guard("interrupt_yielding", "interrupt_yielding", true),
        ],
    ));
    transitions.push(self_loop_transition_with(
        "AcceptWithCompletionRunningImmediate",
        "Running",
        "AcceptWithCompletion",
        vec![
            "input_id",
            "request_immediate_processing",
            "interrupt_yielding",
            "run_id",
        ],
        vec![],
        vec![
            simple_emit("IngressAccepted"),
            post_admission_signal_emit("RequestImmediateProcessing"),
        ],
        vec![
            session_registered_guard(),
            bool_binding_guard(
                "request_immediate_processing",
                "request_immediate_processing",
                true,
            ),
            bool_binding_guard("interrupt_yielding", "interrupt_yielding", false),
        ],
    ));

    for phase in ["Idle", "Attached", "Running"] {
        transitions.push(self_loop_transition_with(
            &format!("AcceptWithoutWake{phase}"),
            phase,
            "AcceptWithoutWake",
            vec!["input_id"],
            vec![],
            vec![simple_emit("IngressAccepted")],
            vec![session_registered_guard()],
        ));
    }

    for (variant, bindings, emit_variant) in [
        (
            "ClassifyExternalEnvelope",
            vec![],
            Some("EnqueueClassifiedEntry"),
        ),
        ("ClassifyPlainEvent", vec![], Some("EnqueueClassifiedEntry")),
    ] {
        transitions.push(self_loop_transition_with(
            &format!("{variant}Attached"),
            "Attached",
            variant,
            bindings.clone(),
            vec![],
            emit_variant.into_iter().map(simple_emit).collect(),
            vec![session_registered_guard()],
        ));
        transitions.push(self_loop_transition_with(
            &format!("{variant}Running"),
            "Running",
            variant,
            bindings,
            vec![],
            emit_variant.into_iter().map(simple_emit).collect(),
            vec![session_registered_guard()],
        ));
    }

    // Prepare starts a run: Idle|Attached -> Running (not a self-loop).
    // The checked-in machine owns the coarse run-binding truth for explicit
    // preparation requests; retire-preserved drain uses the internal
    // DrainQueuedRun signal below.
    for (from_phase, pre_run_phase) in [("Idle", "idle"), ("Attached", "attached")] {
        transitions.push(TransitionSchema {
            name: format!("Prepare{from_phase}"),
            from: vec![from_phase.into()],
            on: InputMatch {
                kind: meerkat_trigger_kind("Prepare"),
                variant: "Prepare".into(),
                bindings: vec!["session_id".into(), "run_id".into()],
            },
            guards: vec![session_registered_guard()],
            updates: vec![
                assign_some("current_run_id", "run_id"),
                Update::Assign {
                    field: "pre_run_phase".into(),
                    expr: Expr::Some(Box::new(Expr::String(pre_run_phase.into()))),
                },
            ],
            to: "Running".into(),
            emit: vec![simple_emit("SubmitRunPrimitive")],
        });
    }

    transitions.push(TransitionSchema {
        name: "DrainQueuedRunRetired".into(),
        from: vec!["Retired".into()],
        on: InputMatch {
            kind: meerkat_trigger_kind("DrainQueuedRun"),
            variant: "DrainQueuedRun".into(),
            bindings: vec!["run_id".into()],
        },
        guards: vec![],
        updates: vec![
            assign_some("current_run_id", "run_id"),
            Update::Assign {
                field: "pre_run_phase".into(),
                expr: Expr::Some(Box::new(Expr::String("retired".into()))),
            },
        ],
        to: "Running".into(),
        emit: vec![simple_emit("SubmitRunPrimitive")],
    });

    for (variant, bindings) in [
        ("StartConversationRun", vec![]),
        ("StartImmediateAppend", vec![]),
        ("StartImmediateContext", vec![]),
    ] {
        transitions.push(self_loop_transition_with(
            &format!("{variant}Attached"),
            "Attached",
            variant,
            bindings,
            vec![],
            vec![simple_emit("SubmitRunPrimitive")],
            vec![session_registered_guard()],
        ));
    }

    // Commit and Fail exit Running directly at the top-level machine
    // boundary, restoring the pre-run control phase (Idle, Attached, or Retired).
    // The lower runtime authorities still own the internal run-completion
    // details.
    for (to_phase, guard_phase) in [
        ("Idle", "Idle"),
        ("Attached", "Attached"),
        ("Retired", "Retired"),
    ] {
        transitions.push(TransitionSchema {
            name: format!("CommitRunningTo{to_phase}"),
            from: vec!["Running".into()],
            on: InputMatch {
                kind: meerkat_trigger_kind("Commit"),
                variant: "Commit".into(),
                bindings: vec!["input_id".into(), "run_id".into()],
            },
            guards: vec![
                pre_run_phase_matches_guard(guard_phase),
                current_run_id_matches_guard("run_id"),
            ],
            updates: vec![
                Update::Assign {
                    field: "current_run_id".into(),
                    expr: Expr::None,
                },
                Update::Assign {
                    field: "pre_run_phase".into(),
                    expr: Expr::None,
                },
            ],
            to: to_phase.into(),
            emit: vec![],
        });
        transitions.push(TransitionSchema {
            name: format!("FailRunningTo{to_phase}"),
            from: vec!["Running".into()],
            on: InputMatch {
                kind: meerkat_trigger_kind("Fail"),
                variant: "Fail".into(),
                bindings: vec!["run_id".into()],
            },
            guards: vec![
                pre_run_phase_matches_guard(guard_phase),
                current_run_id_matches_guard("run_id"),
            ],
            updates: vec![
                Update::Assign {
                    field: "current_run_id".into(),
                    expr: Expr::None,
                },
                Update::Assign {
                    field: "pre_run_phase".into(),
                    expr: Expr::None,
                },
            ],
            to: to_phase.into(),
            emit: vec![simple_emit("RecordTerminalOutcome")],
        });
    }

    for (variant, bindings, emit_variant) in [
        ("StageAdd", vec![], Some("EmitExternalToolDelta")),
        ("StageRemove", vec![], Some("EmitExternalToolDelta")),
        ("StageReload", vec![], Some("EmitExternalToolDelta")),
        (
            "ApplySurfaceBoundary",
            vec![],
            Some("ScheduleSurfaceCompletion"),
        ),
        ("PendingSucceeded", vec![], Some("EmitExternalToolDelta")),
        ("PendingFailed", vec![], Some("EmitExternalToolDelta")),
        ("CallStarted", vec![], None),
        ("CallFinished", vec![], None),
        (
            "FinalizeRemovalClean",
            vec![],
            Some("EmitExternalToolDelta"),
        ),
        (
            "FinalizeRemovalForced",
            vec![],
            Some("EmitExternalToolDelta"),
        ),
        ("SnapshotAligned", vec![], Some("EmitExternalToolDelta")),
        ("ShutdownSurface", vec![], Some("EmitExternalToolDelta")),
    ] {
        transitions.push(self_loop_transition_with(
            &format!("{variant}Attached"),
            "Attached",
            variant,
            bindings.clone(),
            vec![],
            emit_variant.into_iter().map(simple_emit).collect(),
            vec![session_registered_guard()],
        ));
        transitions.push(self_loop_transition_with(
            &format!("{variant}Running"),
            "Running",
            variant,
            bindings,
            vec![],
            emit_variant.into_iter().map(simple_emit).collect(),
            vec![session_registered_guard()],
        ));
    }

    // Recycle: from Idle/Retired → Idle, from Attached → Attached (dispatch re-attaches).
    let recycle_updates = vec![
        Update::Assign {
            field: "active_fence_token".into(),
            expr: Expr::None,
        },
        Update::Assign {
            field: "active_generation".into(),
            expr: Expr::None,
        },
        Update::Assign {
            field: "current_run_id".into(),
            expr: Expr::None,
        },
        Update::Assign {
            field: "peer_ingress_configured".into(),
            expr: Expr::Bool(false),
        },
        Update::Assign {
            field: "drain_running".into(),
            expr: Expr::Bool(false),
        },
    ];
    transitions.push(TransitionSchema {
        name: "RecycleFromIdleOrRetired".into(),
        from: vec!["Idle".into(), "Retired".into()],
        on: InputMatch {
            kind: meerkat_trigger_kind("Recycle"),
            variant: "Recycle".into(),
            bindings: vec![],
        },
        guards: vec![runtime_is_bound_guard()],
        updates: recycle_updates.clone(),
        to: "Idle".into(),
        emit: vec![simple_emit("InitiateRecycle")],
    });
    transitions.push(TransitionSchema {
        name: "RecycleFromAttached".into(),
        from: vec!["Attached".into()],
        on: InputMatch {
            kind: meerkat_trigger_kind("Recycle"),
            variant: "Recycle".into(),
            bindings: vec![],
        },
        guards: vec![runtime_is_bound_guard()],
        updates: recycle_updates,
        to: "Attached".into(),
        emit: vec![simple_emit("InitiateRecycle")],
    });

    transitions
}

fn self_loop_transition_with(
    name: &str,
    phase: &str,
    variant: &str,
    bindings: Vec<&str>,
    updates: Vec<Update>,
    emit: Vec<EffectEmit>,
    guards: Vec<Guard>,
) -> TransitionSchema {
    TransitionSchema {
        name: name.into(),
        from: vec![phase.into()],
        on: InputMatch {
            kind: meerkat_trigger_kind(variant),
            variant: variant.into(),
            bindings: bindings.into_iter().map(Into::into).collect(),
        },
        guards,
        updates,
        to: phase.into(),
        emit,
    }
}

fn meerkat_trigger_kind(variant: &str) -> TriggerKind {
    if is_meerkat_runtime_input_variant(variant) {
        TriggerKind::Input
    } else {
        TriggerKind::Signal
    }
}

fn simple_emit(variant: &str) -> EffectEmit {
    EffectEmit {
        variant: variant.into(),
        fields: IndexMap::new(),
    }
}

fn post_admission_signal_emit(signal: &str) -> EffectEmit {
    EffectEmit {
        variant: "PostAdmissionSignal".into(),
        fields: IndexMap::from([("signal".into(), Expr::String(signal.into()))]),
    }
}

fn absorbed_meerkat_effect_variants() -> Vec<VariantSchema> {
    vec![
        variant("ResolveAdmission"),
        variant("SubmitAdmittedIngressEffect"),
        variant("SubmitRunPrimitive"),
        variant("ResolveCompletionAsTerminated"),
        variant("ApplyControlPlaneCommand"),
        variant("InitiateRecycle"),
        variant("IngressAccepted"),
        VariantSchema {
            name: "PostAdmissionSignal".into(),
            fields: vec![field("signal", TypeRef::String)],
        },
        variant("ReadyForRun"),
        variant("InputLifecycleNotice"),
        variant("CompletionResolved"),
        variant("IngressNotice"),
        variant("SilentIntentApplied"),
        variant("CheckCompaction"),
        variant("RecordTerminalOutcome"),
        variant("RecordRunAssociation"),
        variant("RecordBoundarySequence"),
        variant("SubmitOpEvent"),
        variant("NotifyOpWatcher"),
        variant("ExposeOperationPeer"),
        variant("RetainTerminalRecord"),
        variant("EvictCompletedRecord"),
        VariantSchema {
            name: "CompletionProduced".into(),
            fields: vec![
                field("seq", TypeRef::U64),
                field("operation_id", named("OperationId")),
                field("kind", named("OperationKind")),
            ],
        },
        variant("WaitAllSatisfied"),
        variant("CollectCompletedResult"),
        variant("EnqueueClassifiedEntry"),
        variant("SpawnDrainTask"),
        variant("ScheduleSurfaceCompletion"),
        variant("RefreshVisibleSurfaceSet"),
        variant("EmitExternalToolDelta"),
        variant("CloseSurfaceConnection"),
        variant("RejectSurfaceCall"),
    ]
}

fn absorbed_meerkat_effect_dispositions() -> Vec<EffectDispositionRule> {
    vec![
        local_disposition("ResolveAdmission"),
        local_disposition("SubmitAdmittedIngressEffect"),
        local_disposition("SubmitRunPrimitive"),
        local_disposition("ResolveCompletionAsTerminated"),
        local_disposition("ApplyControlPlaneCommand"),
        local_disposition("InitiateRecycle"),
        external_disposition("IngressAccepted"),
        local_disposition("PostAdmissionSignal"),
        local_disposition("ReadyForRun"),
        external_disposition("InputLifecycleNotice"),
        local_disposition("CompletionResolved"),
        external_disposition("IngressNotice"),
        external_disposition("SilentIntentApplied"),
        local_disposition("CheckCompaction"),
        local_disposition("RecordTerminalOutcome"),
        local_disposition("RecordRunAssociation"),
        local_disposition("RecordBoundarySequence"),
        local_disposition("SubmitOpEvent"),
        local_disposition("NotifyOpWatcher"),
        local_disposition("ExposeOperationPeer"),
        local_disposition("RetainTerminalRecord"),
        local_disposition("EvictCompletedRecord"),
        local_disposition("CompletionProduced"),
        local_disposition("WaitAllSatisfied"),
        local_disposition("CollectCompletedResult"),
        // ConcurrencyLimitExceeded is an OpsLifecycleError in runtime, not an
        // effect. Removed from the effect enum; the error path handles it.
        local_disposition("EnqueueClassifiedEntry"),
        local_disposition("SpawnDrainTask"),
        local_disposition("ScheduleSurfaceCompletion"),
        external_disposition("RefreshVisibleSurfaceSet"),
        external_disposition("EmitExternalToolDelta"),
        local_disposition("CloseSurfaceConnection"),
        external_disposition("RejectSurfaceCall"),
    ]
}
