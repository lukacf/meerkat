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
                    variant("Recovering"),
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
                field(
                    "active_work_id",
                    TypeRef::Option(Box::new(TypeRef::Named("WorkId".into()))),
                ),
                field("wake_pending", TypeRef::Bool),
                field("process_pending", TypeRef::Bool),
                field("peer_ingress_configured", TypeRef::Bool),
                field("drain_running", TypeRef::Bool),
                field(
                    "resolved_peer_keys",
                    TypeRef::Set(Box::new(named("ReachabilityKey"))),
                ),
                field(
                    "peer_reachability",
                    TypeRef::Map(
                        Box::new(named("ReachabilityKey")),
                        Box::new(named("PeerReachability")),
                    ),
                ),
                field(
                    "peer_last_reason",
                    TypeRef::Map(
                        Box::new(named("ReachabilityKey")),
                        Box::new(TypeRef::Option(Box::new(named("PeerReachabilityReason")))),
                    ),
                ),
                field("interrupt_pending", TypeRef::Bool),
                field("shutdown_pending", TypeRef::Bool),
                field("inherited_base_filter", named("ToolFilter")),
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
                field(
                    "requested_witnesses",
                    TypeRef::Map(
                        Box::new(TypeRef::String),
                        Box::new(named("ToolVisibilityWitness")),
                    ),
                ),
                field(
                    "filter_witnesses",
                    TypeRef::Map(
                        Box::new(TypeRef::String),
                        Box::new(named("ToolVisibilityWitness")),
                    ),
                ),
                field("active_visibility_revision", TypeRef::U64),
                field("staged_visibility_revision", TypeRef::U64),
                field("committed_visibility_revision", TypeRef::U64),
            ],
            init: InitSchema {
                phase: "Initializing".into(),
                fields: vec![
                    init("session_id", Expr::None),
                    init("active_runtime_id", Expr::None),
                    init("active_fence_token", Expr::None),
                    init("active_generation", Expr::None),
                    init("active_work_id", Expr::None),
                    init("wake_pending", Expr::Bool(false)),
                    init("process_pending", Expr::Bool(false)),
                    init("peer_ingress_configured", Expr::Bool(false)),
                    init("drain_running", Expr::Bool(false)),
                    init("resolved_peer_keys", Expr::EmptySet),
                    init("peer_reachability", Expr::EmptyMap),
                    init("peer_last_reason", Expr::EmptyMap),
                    init("interrupt_pending", Expr::Bool(false)),
                    init("shutdown_pending", Expr::Bool(false)),
                    init("inherited_base_filter", tool_filter_all()),
                    init("active_filter", tool_filter_all()),
                    init("staged_filter", tool_filter_all()),
                    init("active_requested_deferred_names", Expr::EmptySet),
                    init("staged_requested_deferred_names", Expr::EmptySet),
                    init("requested_witnesses", Expr::EmptyMap),
                    init("filter_witnesses", Expr::EmptyMap),
                    init("active_visibility_revision", Expr::U64(0)),
                    init("staged_visibility_revision", Expr::U64(0)),
                    init("committed_visibility_revision", Expr::U64(0)),
                ],
            },
            terminal_phases: vec!["Destroyed".into()],
        },
        inputs: EnumSchema {
            name: "MeerkatMachineInput".into(),
            variants: input_variants,
        },
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
        helpers: vec![
            HelperSchema {
                name: "HasPendingVisibilityPromotion".into(),
                params: vec![],
                returns: TypeRef::Bool,
                body: Expr::Gt(
                    Box::new(Expr::Field("staged_visibility_revision".into())),
                    Box::new(Expr::Field("active_visibility_revision".into())),
                ),
            },
            HelperSchema {
                name: "RequestedWitnessKeys".into(),
                params: vec![],
                returns: TypeRef::Set(Box::new(TypeRef::String)),
                body: Expr::MapKeys(Box::new(Expr::Field("requested_witnesses".into()))),
            },
            HelperSchema {
                name: "FilterWitnessKeys".into(),
                params: vec![],
                returns: TypeRef::Set(Box::new(TypeRef::String)),
                body: Expr::MapKeys(Box::new(Expr::Field("filter_witnesses".into()))),
            },
        ],
        derived: vec![],
        invariants: vec![
            InvariantSchema {
                name: "running_has_active_work".into(),
                expr: Expr::Or(vec![
                    Expr::Neq(
                        Box::new(Expr::CurrentPhase),
                        Box::new(Expr::Phase("Running".into())),
                    ),
                    Expr::Neq(
                        Box::new(Expr::Field("active_work_id".into())),
                        Box::new(Expr::None),
                    ),
                ]),
            },
            InvariantSchema {
                name: "bound_runtime_has_fence".into(),
                expr: Expr::Or(vec![
                    Expr::Eq(
                        Box::new(Expr::Field("active_runtime_id".into())),
                        Box::new(Expr::None),
                    ),
                    Expr::Neq(
                        Box::new(Expr::Field("active_fence_token".into())),
                        Box::new(Expr::None),
                    ),
                ]),
            },
            InvariantSchema {
                name: "destroyed_has_no_active_work".into(),
                expr: Expr::Or(vec![
                    Expr::Neq(
                        Box::new(Expr::CurrentPhase),
                        Box::new(Expr::Phase("Destroyed".into())),
                    ),
                    Expr::Eq(
                        Box::new(Expr::Field("active_work_id".into())),
                        Box::new(Expr::None),
                    ),
                ]),
            },
            InvariantSchema {
                name: "interrupt_pending_only_while_active".into(),
                expr: Expr::Or(vec![
                    Expr::Eq(
                        Box::new(Expr::Field("interrupt_pending".into())),
                        Box::new(Expr::Bool(false)),
                    ),
                    Expr::Eq(
                        Box::new(Expr::CurrentPhase),
                        Box::new(Expr::Phase("Running".into())),
                    ),
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
                name: "peer_reachability_keys_are_resolved".into(),
                expr: Expr::Quantified {
                    quantifier: Quantifier::All,
                    binding: "key".into(),
                    over: Box::new(Expr::MapKeys(Box::new(Expr::Field(
                        "peer_reachability".into(),
                    )))),
                    body: Box::new(Expr::Contains {
                        collection: Box::new(Expr::Field("resolved_peer_keys".into())),
                        value: Box::new(Expr::Binding("key".into())),
                    }),
                },
            },
            InvariantSchema {
                name: "peer_last_reason_keys_are_resolved".into(),
                expr: Expr::Quantified {
                    quantifier: Quantifier::All,
                    binding: "key".into(),
                    over: Box::new(Expr::MapKeys(Box::new(Expr::Field(
                        "peer_last_reason".into(),
                    )))),
                    body: Box::new(Expr::Contains {
                        collection: Box::new(Expr::Field("resolved_peer_keys".into())),
                        value: Box::new(Expr::Binding("key".into())),
                    }),
                },
            },
            InvariantSchema {
                name: "active_visibility_revision_not_ahead_of_staged".into(),
                expr: Expr::Lte(
                    Box::new(Expr::Field("active_visibility_revision".into())),
                    Box::new(Expr::Field("staged_visibility_revision".into())),
                ),
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
            InvariantSchema {
                name: "equal_visibility_revision_means_equal_active_and_staged_state".into(),
                expr: Expr::Or(vec![
                    Expr::Neq(
                        Box::new(Expr::Field("active_visibility_revision".into())),
                        Box::new(Expr::Field("staged_visibility_revision".into())),
                    ),
                    Expr::And(vec![
                        Expr::Eq(
                            Box::new(Expr::Field("active_filter".into())),
                            Box::new(Expr::Field("staged_filter".into())),
                        ),
                        Expr::Eq(
                            Box::new(Expr::Field("active_requested_deferred_names".into())),
                            Box::new(Expr::Field("staged_requested_deferred_names".into())),
                        ),
                    ]),
                ]),
            },
            InvariantSchema {
                name: "committed_visibility_not_ahead_of_active".into(),
                expr: Expr::Lte(
                    Box::new(Expr::Field("committed_visibility_revision".into())),
                    Box::new(Expr::Field("active_visibility_revision".into())),
                ),
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
            TransitionSchema {
                name: "RegisterSession".into(),
                from: vec!["Idle".into(), "Stopped".into(), "Retired".into()],
                on: InputMatch {
                    kind: meerkat_trigger_kind("RegisterSession"),
                    variant: "RegisterSession".into(),
                    bindings: vec!["session_id".into()],
                },
                guards: vec![],
                updates: vec![assign_some("session_id", "session_id")],
                to: "Idle".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "UnregisterSession".into(),
                from: vec!["Idle".into(), "Stopped".into(), "Retired".into()],
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
            },
            TransitionSchema {
                name: "StagePersistentFilterAttached".into(),
                from: vec!["Attached".into()],
                on: InputMatch {
                    kind: meerkat_trigger_kind("StagePersistentFilter"),
                    variant: "StagePersistentFilter".into(),
                    bindings: vec!["filter".into(), "witnesses".into()],
                },
                guards: vec![session_registered_guard()],
                updates: vec![
                    Update::Assign {
                        field: "staged_filter".into(),
                        expr: Expr::Binding("filter".into()),
                    },
                    Update::ForEach {
                        binding: "name".into(),
                        over: Expr::MapKeys(Box::new(Expr::Binding("witnesses".into()))),
                        updates: vec![Update::MapInsert {
                            field: "filter_witnesses".into(),
                            key: Expr::Binding("name".into()),
                            value: Expr::MapGet {
                                map: Box::new(Expr::Binding("witnesses".into())),
                                key: Box::new(Expr::Binding("name".into())),
                            },
                        }],
                    },
                    Update::Increment {
                        field: "staged_visibility_revision".into(),
                        amount: 1,
                    },
                ],
                to: "Attached".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "StagePersistentFilterRunning".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    kind: meerkat_trigger_kind("StagePersistentFilter"),
                    variant: "StagePersistentFilter".into(),
                    bindings: vec!["filter".into(), "witnesses".into()],
                },
                guards: vec![session_registered_guard()],
                updates: vec![
                    Update::Assign {
                        field: "staged_filter".into(),
                        expr: Expr::Binding("filter".into()),
                    },
                    Update::ForEach {
                        binding: "name".into(),
                        over: Expr::MapKeys(Box::new(Expr::Binding("witnesses".into()))),
                        updates: vec![Update::MapInsert {
                            field: "filter_witnesses".into(),
                            key: Expr::Binding("name".into()),
                            value: Expr::MapGet {
                                map: Box::new(Expr::Binding("witnesses".into())),
                                key: Box::new(Expr::Binding("name".into())),
                            },
                        }],
                    },
                    Update::Increment {
                        field: "staged_visibility_revision".into(),
                        amount: 1,
                    },
                ],
                to: "Running".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "RequestDeferredToolsAttached".into(),
                from: vec!["Attached".into()],
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
                    Update::ForEach {
                        binding: "name".into(),
                        over: Expr::MapKeys(Box::new(Expr::Binding("witnesses".into()))),
                        updates: vec![Update::MapInsert {
                            field: "requested_witnesses".into(),
                            key: Expr::Binding("name".into()),
                            value: Expr::MapGet {
                                map: Box::new(Expr::Binding("witnesses".into())),
                                key: Box::new(Expr::Binding("name".into())),
                            },
                        }],
                    },
                    Update::Increment {
                        field: "staged_visibility_revision".into(),
                        amount: 1,
                    },
                ],
                to: "Attached".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "RequestDeferredToolsRunning".into(),
                from: vec!["Running".into()],
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
                    Update::ForEach {
                        binding: "name".into(),
                        over: Expr::MapKeys(Box::new(Expr::Binding("witnesses".into()))),
                        updates: vec![Update::MapInsert {
                            field: "requested_witnesses".into(),
                            key: Expr::Binding("name".into()),
                            value: Expr::MapGet {
                                map: Box::new(Expr::Binding("witnesses".into())),
                                key: Box::new(Expr::Binding("name".into())),
                            },
                        }],
                    },
                    Update::Increment {
                        field: "staged_visibility_revision".into(),
                        amount: 1,
                    },
                ],
                to: "Running".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "PrepareBindings".into(),
                from: vec!["Idle".into(), "Stopped".into(), "Retired".into()],
                on: InputMatch {
                    kind: meerkat_trigger_kind("PrepareBindings"),
                    variant: "PrepareBindings".into(),
                    bindings: vec![
                        "agent_runtime_id".into(),
                        "fence_token".into(),
                        "generation".into(),
                    ],
                },
                guards: vec![session_registered_guard()],
                updates: vec![
                    assign_some("active_runtime_id", "agent_runtime_id"),
                    assign_some("active_fence_token", "fence_token"),
                    assign_some("active_generation", "generation"),
                    Update::Assign {
                        field: "interrupt_pending".into(),
                        expr: Expr::Bool(false),
                    },
                    Update::Assign {
                        field: "shutdown_pending".into(),
                        expr: Expr::Bool(false),
                    },
                ],
                to: "Attached".into(),
                emit: vec![runtime_identity_emit("RuntimeBound")],
            },
            TransitionSchema {
                name: "SetPeerIngressContextAttached".into(),
                from: vec!["Attached".into()],
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
                to: "Attached".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "SetPeerIngressContextRunning".into(),
                from: vec!["Running".into()],
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
                to: "Running".into(),
                emit: vec![],
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
                name: "ReconcileResolvedDirectoryAttached".into(),
                from: vec!["Attached".into()],
                on: InputMatch {
                    kind: meerkat_trigger_kind("ReconcileResolvedDirectory"),
                    variant: "ReconcileResolvedDirectory".into(),
                    bindings: vec!["keys".into(), "reachability".into(), "last_reason".into()],
                },
                guards: vec![
                    resolved_map_keys_subset_guard(
                        "reachability_keys_subset_of_resolved",
                        "reachability",
                        "keys",
                    ),
                    resolved_map_keys_subset_guard(
                        "last_reason_keys_subset_of_resolved",
                        "last_reason",
                        "keys",
                    ),
                ],
                updates: vec![
                    Update::Assign {
                        field: "resolved_peer_keys".into(),
                        expr: Expr::Binding("keys".into()),
                    },
                    Update::Assign {
                        field: "peer_reachability".into(),
                        expr: Expr::Binding("reachability".into()),
                    },
                    Update::Assign {
                        field: "peer_last_reason".into(),
                        expr: Expr::Binding("last_reason".into()),
                    },
                ],
                to: "Attached".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "ReconcileResolvedDirectoryRunning".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    kind: meerkat_trigger_kind("ReconcileResolvedDirectory"),
                    variant: "ReconcileResolvedDirectory".into(),
                    bindings: vec!["keys".into(), "reachability".into(), "last_reason".into()],
                },
                guards: vec![
                    resolved_map_keys_subset_guard(
                        "reachability_keys_subset_of_resolved",
                        "reachability",
                        "keys",
                    ),
                    resolved_map_keys_subset_guard(
                        "last_reason_keys_subset_of_resolved",
                        "last_reason",
                        "keys",
                    ),
                ],
                updates: vec![
                    Update::Assign {
                        field: "resolved_peer_keys".into(),
                        expr: Expr::Binding("keys".into()),
                    },
                    Update::Assign {
                        field: "peer_reachability".into(),
                        expr: Expr::Binding("reachability".into()),
                    },
                    Update::Assign {
                        field: "peer_last_reason".into(),
                        expr: Expr::Binding("last_reason".into()),
                    },
                ],
                to: "Running".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "RecordSendSucceededAttached".into(),
                from: vec!["Attached".into()],
                on: InputMatch {
                    kind: meerkat_trigger_kind("RecordSendSucceeded"),
                    variant: "RecordSendSucceeded".into(),
                    bindings: vec!["key".into()],
                },
                guards: vec![peer_key_is_resolved_guard("key")],
                updates: vec![
                    Update::MapInsert {
                        field: "peer_reachability".into(),
                        key: Expr::Binding("key".into()),
                        value: peer_reachability("Reachable"),
                    },
                    Update::MapInsert {
                        field: "peer_last_reason".into(),
                        key: Expr::Binding("key".into()),
                        value: Expr::None,
                    },
                ],
                to: "Attached".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "RecordSendSucceededRunning".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    kind: meerkat_trigger_kind("RecordSendSucceeded"),
                    variant: "RecordSendSucceeded".into(),
                    bindings: vec!["key".into()],
                },
                guards: vec![peer_key_is_resolved_guard("key")],
                updates: vec![
                    Update::MapInsert {
                        field: "peer_reachability".into(),
                        key: Expr::Binding("key".into()),
                        value: peer_reachability("Reachable"),
                    },
                    Update::MapInsert {
                        field: "peer_last_reason".into(),
                        key: Expr::Binding("key".into()),
                        value: Expr::None,
                    },
                ],
                to: "Running".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "RecordSendFailedAttached".into(),
                from: vec!["Attached".into()],
                on: InputMatch {
                    kind: meerkat_trigger_kind("RecordSendFailed"),
                    variant: "RecordSendFailed".into(),
                    bindings: vec!["key".into(), "reason".into()],
                },
                guards: vec![peer_key_is_resolved_guard("key")],
                updates: vec![
                    Update::MapInsert {
                        field: "peer_reachability".into(),
                        key: Expr::Binding("key".into()),
                        value: peer_reachability("Unreachable"),
                    },
                    Update::MapInsert {
                        field: "peer_last_reason".into(),
                        key: Expr::Binding("key".into()),
                        value: Expr::Some(Box::new(Expr::Binding("reason".into()))),
                    },
                ],
                to: "Attached".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "RecordSendFailedRunning".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    kind: meerkat_trigger_kind("RecordSendFailed"),
                    variant: "RecordSendFailed".into(),
                    bindings: vec!["key".into(), "reason".into()],
                },
                guards: vec![peer_key_is_resolved_guard("key")],
                updates: vec![
                    Update::MapInsert {
                        field: "peer_reachability".into(),
                        key: Expr::Binding("key".into()),
                        value: peer_reachability("Unreachable"),
                    },
                    Update::MapInsert {
                        field: "peer_last_reason".into(),
                        key: Expr::Binding("key".into()),
                        value: Expr::Some(Box::new(Expr::Binding("reason".into()))),
                    },
                ],
                to: "Running".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "BeginRunFromIdle".into(),
                from: vec!["Attached".into()],
                on: InputMatch {
                    kind: meerkat_trigger_kind("SubmitMobWork"),
                    variant: "SubmitMobWork".into(),
                    bindings: vec![
                        "agent_runtime_id".into(),
                        "fence_token".into(),
                        "work_id".into(),
                    ],
                },
                guards: vec![runtime_is_bound_guard()],
                updates: vec![
                    assign_some("active_work_id", "work_id"),
                    Update::Assign {
                        field: "wake_pending".into(),
                        expr: Expr::Bool(true),
                    },
                    Update::Assign {
                        field: "interrupt_pending".into(),
                        expr: Expr::Bool(false),
                    },
                    Update::Assign {
                        field: "shutdown_pending".into(),
                        expr: Expr::Bool(false),
                    },
                ],
                to: "Running".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "InterruptCurrentRun".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    kind: meerkat_trigger_kind("InterruptCurrentRun"),
                    variant: "InterruptCurrentRun".into(),
                    bindings: vec![],
                },
                guards: vec![has_active_work_guard()],
                updates: vec![Update::Assign {
                    field: "interrupt_pending".into(),
                    expr: Expr::Bool(true),
                }],
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
                name: "CancelAfterBoundary".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    kind: meerkat_trigger_kind("CancelAfterBoundary"),
                    variant: "CancelAfterBoundary".into(),
                    bindings: vec![],
                },
                guards: vec![has_active_work_guard()],
                updates: vec![Update::Assign {
                    field: "shutdown_pending".into(),
                    expr: Expr::Bool(true),
                }],
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
                        field: "active_filter".into(),
                        expr: Expr::Field("staged_filter".into()),
                    },
                    Update::Assign {
                        field: "active_requested_deferred_names".into(),
                        expr: Expr::Field("staged_requested_deferred_names".into()),
                    },
                    Update::Assign {
                        field: "active_visibility_revision".into(),
                        expr: Expr::Field("staged_visibility_revision".into()),
                    },
                    Update::Assign {
                        field: "committed_visibility_revision".into(),
                        expr: Expr::Binding("revision".into()),
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
                updates: vec![Update::Assign {
                    field: "committed_visibility_revision".into(),
                    expr: Expr::Binding("revision".into()),
                }],
                to: "Running".into(),
                emit: vec![EffectEmit {
                    variant: "CommittedVisibleSetPublished".into(),
                    fields: IndexMap::from([("revision".into(), Expr::Binding("revision".into()))]),
                }],
            },
            TransitionSchema {
                name: "PublishCommittedVisibleSetAttached".into(),
                from: vec!["Attached".into()],
                on: InputMatch {
                    kind: meerkat_trigger_kind("PublishCommittedVisibleSet"),
                    variant: "PublishCommittedVisibleSet".into(),
                    bindings: vec!["revision".into()],
                },
                guards: vec![published_revision_matches_active_guard()],
                updates: vec![Update::Assign {
                    field: "committed_visibility_revision".into(),
                    expr: Expr::Binding("revision".into()),
                }],
                to: "Attached".into(),
                emit: vec![EffectEmit {
                    variant: "CommittedVisibleSetPublished".into(),
                    fields: IndexMap::from([("revision".into(), Expr::Binding("revision".into()))]),
                }],
            },
            TransitionSchema {
                name: "PublishCommittedVisibleSetRunning".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    kind: meerkat_trigger_kind("PublishCommittedVisibleSet"),
                    variant: "PublishCommittedVisibleSet".into(),
                    bindings: vec!["revision".into()],
                },
                guards: vec![published_revision_matches_active_guard()],
                updates: vec![Update::Assign {
                    field: "committed_visibility_revision".into(),
                    expr: Expr::Binding("revision".into()),
                }],
                to: "Running".into(),
                emit: vec![EffectEmit {
                    variant: "CommittedVisibleSetPublished".into(),
                    fields: IndexMap::from([("revision".into(), Expr::Binding("revision".into()))]),
                }],
            },
            TransitionSchema {
                name: "RunCompleted".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    kind: meerkat_trigger_kind("RunCompleted"),
                    variant: "RunCompleted".into(),
                    bindings: vec!["work_id".into()],
                },
                guards: vec![work_matches_active_guard("work_id")],
                updates: clear_running_state(),
                to: "Attached".into(),
                emit: vec![work_identity_emit("WorkCompleted", "work_id")],
            },
            TransitionSchema {
                name: "RunFailed".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    kind: meerkat_trigger_kind("RunFailed"),
                    variant: "RunFailed".into(),
                    bindings: vec!["work_id".into()],
                },
                guards: vec![work_matches_active_guard("work_id")],
                updates: clear_running_state(),
                to: "Attached".into(),
                emit: vec![work_identity_emit("WorkFailed", "work_id")],
            },
            TransitionSchema {
                name: "RunCancelled".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    kind: meerkat_trigger_kind("RunCancelled"),
                    variant: "RunCancelled".into(),
                    bindings: vec!["work_id".into()],
                },
                guards: vec![work_matches_active_guard("work_id")],
                updates: clear_running_state(),
                to: "Attached".into(),
                emit: vec![work_identity_emit("WorkCancelled", "work_id")],
            },
            // Recover: dispatch calls drv.recover() which only touches ingress
            // authority — the control phase does NOT change. Self-loops per phase.
            TransitionSchema {
                name: "RecoverFromIdle".into(),
                from: vec!["Idle".into()],
                on: InputMatch {
                    kind: meerkat_trigger_kind("Recover"),
                    variant: "Recover".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![],
                to: "Idle".into(),
                emit: vec![notice_emit("recover", "runtime recovering")],
            },
            TransitionSchema {
                name: "RecoverFromAttached".into(),
                from: vec!["Attached".into()],
                on: InputMatch {
                    kind: meerkat_trigger_kind("Recover"),
                    variant: "Recover".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![],
                to: "Attached".into(),
                emit: vec![notice_emit("recover", "runtime recovering")],
            },
            TransitionSchema {
                name: "RetireRequestedFromIdle".into(),
                from: vec!["Idle".into(), "Attached".into(), "Running".into()],
                on: InputMatch {
                    kind: meerkat_trigger_kind("Retire"),
                    variant: "Retire".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: clear_running_state(),
                to: "Retired".into(),
                emit: vec![runtime_identity_emit("RuntimeRetired")],
            },
            TransitionSchema {
                name: "Reset".into(),
                from: vec![
                    "Initializing".into(),
                    "Idle".into(),
                    "Attached".into(),
                    "Recovering".into(),
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
                        field: "active_work_id".into(),
                        expr: Expr::None,
                    },
                    Update::Assign {
                        field: "wake_pending".into(),
                        expr: Expr::Bool(false),
                    },
                    Update::Assign {
                        field: "process_pending".into(),
                        expr: Expr::Bool(false),
                    },
                    Update::Assign {
                        field: "drain_running".into(),
                        expr: Expr::Bool(false),
                    },
                    Update::Assign {
                        field: "interrupt_pending".into(),
                        expr: Expr::Bool(false),
                    },
                    Update::Assign {
                        field: "shutdown_pending".into(),
                        expr: Expr::Bool(false),
                    },
                ],
                to: "Idle".into(),
                emit: vec![notice_emit("reset", "runtime reset")],
            },
            TransitionSchema {
                name: "StopRuntimeExecutor".into(),
                from: vec![
                    "Initializing".into(),
                    "Idle".into(),
                    "Attached".into(),
                    "Running".into(),
                    "Recovering".into(),
                    "Retired".into(),
                ],
                on: InputMatch {
                    kind: meerkat_trigger_kind("StopRuntimeExecutor"),
                    variant: "StopRuntimeExecutor".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![
                    Update::Assign {
                        field: "active_work_id".into(),
                        expr: Expr::None,
                    },
                    Update::Assign {
                        field: "drain_running".into(),
                        expr: Expr::Bool(false),
                    },
                    Update::Assign {
                        field: "interrupt_pending".into(),
                        expr: Expr::Bool(false),
                    },
                    Update::Assign {
                        field: "shutdown_pending".into(),
                        expr: Expr::Bool(false),
                    },
                ],
                to: "Stopped".into(),
                emit: vec![notice_emit("stop", "runtime executor stopped")],
            },
            TransitionSchema {
                name: "Destroy".into(),
                from: vec![
                    "Initializing".into(),
                    "Idle".into(),
                    "Attached".into(),
                    "Running".into(),
                    "Recovering".into(),
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
                        field: "active_work_id".into(),
                        expr: Expr::None,
                    },
                    Update::Assign {
                        field: "wake_pending".into(),
                        expr: Expr::Bool(false),
                    },
                    Update::Assign {
                        field: "process_pending".into(),
                        expr: Expr::Bool(false),
                    },
                    Update::Assign {
                        field: "drain_running".into(),
                        expr: Expr::Bool(false),
                    },
                    Update::Assign {
                        field: "interrupt_pending".into(),
                        expr: Expr::Bool(false),
                    },
                    Update::Assign {
                        field: "shutdown_pending".into(),
                        expr: Expr::Bool(false),
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
            field: "active_work_id".into(),
            expr: Expr::None,
        },
        Update::Assign {
            field: "wake_pending".into(),
            expr: Expr::Bool(false),
        },
        Update::Assign {
            field: "process_pending".into(),
            expr: Expr::Bool(false),
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
            field: "resolved_peer_keys".into(),
            expr: Expr::EmptySet,
        },
        Update::Assign {
            field: "peer_reachability".into(),
            expr: Expr::EmptyMap,
        },
        Update::Assign {
            field: "peer_last_reason".into(),
            expr: Expr::EmptyMap,
        },
        Update::Assign {
            field: "interrupt_pending".into(),
            expr: Expr::Bool(false),
        },
        Update::Assign {
            field: "shutdown_pending".into(),
            expr: Expr::Bool(false),
        },
        Update::Assign {
            field: "inherited_base_filter".into(),
            expr: tool_filter_all(),
        },
        Update::Assign {
            field: "active_filter".into(),
            expr: tool_filter_all(),
        },
        Update::Assign {
            field: "staged_filter".into(),
            expr: tool_filter_all(),
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
            field: "requested_witnesses".into(),
            expr: Expr::EmptyMap,
        },
        Update::Assign {
            field: "filter_witnesses".into(),
            expr: Expr::EmptyMap,
        },
        Update::Assign {
            field: "active_visibility_revision".into(),
            expr: Expr::U64(0),
        },
        Update::Assign {
            field: "staged_visibility_revision".into(),
            expr: Expr::U64(0),
        },
        Update::Assign {
            field: "committed_visibility_revision".into(),
            expr: Expr::U64(0),
        },
    ]
}

fn clear_running_state() -> Vec<Update> {
    vec![
        Update::Assign {
            field: "active_work_id".into(),
            expr: Expr::None,
        },
        Update::Assign {
            field: "interrupt_pending".into(),
            expr: Expr::Bool(false),
        },
        Update::Assign {
            field: "shutdown_pending".into(),
            expr: Expr::Bool(false),
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

fn work_identity_emit(variant: &str, binding: &str) -> EffectEmit {
    EffectEmit {
        variant: variant.into(),
        fields: IndexMap::from([
            ("agent_runtime_id".into(), option_value("active_runtime_id")),
            ("fence_token".into(), option_value("active_fence_token")),
            ("generation".into(), option_value("active_generation")),
            ("work_id".into(), Expr::Binding(binding.into())),
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

fn tool_filter_all() -> Expr {
    Expr::NamedVariant {
        enum_name: "ToolFilter".into(),
        variant: "All".into(),
    }
}

fn peer_reachability(variant: &str) -> Expr {
    Expr::NamedVariant {
        enum_name: "PeerReachability".into(),
        variant: variant.into(),
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

fn has_active_work_guard() -> Guard {
    Guard {
        name: "has_active_work".into(),
        expr: Expr::Neq(
            Box::new(Expr::Field("active_work_id".into())),
            Box::new(Expr::None),
        ),
    }
}

fn work_matches_active_guard(binding: &str) -> Guard {
    Guard {
        name: "work_matches_active".into(),
        expr: Expr::Eq(
            Box::new(Expr::Field("active_work_id".into())),
            Box::new(Expr::Some(Box::new(Expr::Binding(binding.into())))),
        ),
    }
}

fn published_revision_matches_active_guard() -> Guard {
    Guard {
        name: "revision_matches_active".into(),
        expr: Expr::Eq(
            Box::new(Expr::Field("active_visibility_revision".into())),
            Box::new(Expr::Binding("revision".into())),
        ),
    }
}

fn resolved_map_keys_subset_guard(name: &str, map_binding: &str, keys_binding: &str) -> Guard {
    Guard {
        name: name.into(),
        expr: Expr::Quantified {
            quantifier: Quantifier::All,
            binding: "k".into(),
            over: Box::new(Expr::MapKeys(Box::new(Expr::Binding(map_binding.into())))),
            body: Box::new(Expr::Contains {
                collection: Box::new(Expr::Binding(keys_binding.into())),
                value: Box::new(Expr::Binding("k".into())),
            }),
        },
    }
}

fn peer_key_is_resolved_guard(binding: &str) -> Guard {
    Guard {
        name: "peer_key_is_resolved".into(),
        expr: Expr::Contains {
            collection: Box::new(Expr::Field("resolved_peer_keys".into())),
            value: Box::new(Expr::Binding(binding.into())),
        },
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
            name: "PrepareBindings".into(),
            fields: vec![
                field("agent_runtime_id", TypeRef::Named("AgentRuntimeId".into())),
                field("fence_token", TypeRef::Named("FenceToken".into())),
                field("generation", TypeRef::Named("Generation".into())),
            ],
        },
        VariantSchema {
            name: "SubmitMobWork".into(),
            fields: vec![
                field("agent_runtime_id", TypeRef::Named("AgentRuntimeId".into())),
                field("fence_token", TypeRef::Named("FenceToken".into())),
                field("work_id", TypeRef::Named("WorkId".into())),
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
        VariantSchema {
            name: "ReconcileResolvedDirectory".into(),
            fields: vec![
                field("keys", TypeRef::Set(Box::new(named("ReachabilityKey")))),
                field(
                    "reachability",
                    TypeRef::Map(
                        Box::new(named("ReachabilityKey")),
                        Box::new(named("PeerReachability")),
                    ),
                ),
                field(
                    "last_reason",
                    TypeRef::Map(
                        Box::new(named("ReachabilityKey")),
                        Box::new(TypeRef::Option(Box::new(named("PeerReachabilityReason")))),
                    ),
                ),
            ],
        },
        VariantSchema {
            name: "RecordSendSucceeded".into(),
            fields: vec![field("key", named("ReachabilityKey"))],
        },
        VariantSchema {
            name: "RecordSendFailed".into(),
            fields: vec![
                field("key", named("ReachabilityKey")),
                field("reason", named("PeerReachabilityReason")),
            ],
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
            fields: vec![field("revision", TypeRef::U64)],
        },
        VariantSchema {
            name: "BoundaryApplied".into(),
            fields: vec![field("revision", TypeRef::U64)],
        },
        VariantSchema {
            name: "RunCompleted".into(),
            fields: vec![field("work_id", TypeRef::Named("WorkId".into()))],
        },
        VariantSchema {
            name: "RunFailed".into(),
            fields: vec![field("work_id", TypeRef::Named("WorkId".into()))],
        },
        VariantSchema {
            name: "RunCancelled".into(),
            fields: vec![field("work_id", TypeRef::Named("WorkId".into()))],
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
            | "PrepareBindings"
            | "InputState"
            | "ListActiveInputs"
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
            fields: vec![field("runtime_id", TypeRef::String)],
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
            fields: vec![field("input_id", named("InputId"))],
        },
        VariantSchema {
            name: "AcceptWithoutWake".into(),
            fields: vec![field("input_id", named("InputId"))],
        },
        VariantSchema {
            name: "Prepare".into(),
            fields: vec![field("session_id", named("SessionId"))],
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
        variant("AdmitQueued"),
        variant("AdmitConsumedOnAccept"),
        variant("StageDrainSnapshot"),
        variant("SupersedeQueuedInput"),
        variant("CoalesceQueuedInputs"),
        variant("SetSilentIntentOverrides"),
        variant("StartConversationRun"),
        variant("StartImmediateAppend"),
        variant("StartImmediateContext"),
        variant("PrimitiveApplied"),
        variant("LlmReturnedToolCalls"),
        variant("LlmReturnedTerminal"),
        variant("RegisterPendingOps"),
        variant("ToolCallsResolved"),
        variant("OpsBarrierSatisfied"),
        variant("BoundaryContinue"),
        variant("BoundaryComplete"),
        variant("RecoverableFailure"),
        variant("FatalFailure"),
        variant("RetryRequested"),
        variant("CancelNow"),
        variant("CancellationObserved"),
        variant("AcknowledgeTerminal"),
        variant("TurnLimitReached"),
        variant("BudgetExhausted"),
        variant("TimeBudgetExceeded"),
        variant("EnterExtraction"),
        variant("ExtractionValidationPassed"),
        variant("ExtractionValidationFailed"),
        variant("ExtractionStart"),
        variant("ForceCancelNoRun"),
        variant("RegisterOperation"),
        variant("ProvisioningSucceeded"),
        variant("ProvisioningFailed"),
        variant("AbortProvisioning"),
        variant("PeerReady"),
        variant("RegisterWatcher"),
        variant("ProgressReported"),
        variant("CompleteOperation"),
        variant("FailOperation"),
        variant("CancelOperation"),
        variant("RetireRequested"),
        variant("RetireCompleted"),
        variant("CollectTerminal"),
        variant("BeginWaitAll"),
        variant("CancelWaitAll"),
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

    for (variant, bindings) in [
        ("EnsureSessionWithExecutor", vec!["session_id"]),
        ("SetSilentIntents", vec!["session_id", "intents"]),
        ("ContainsSession", vec!["session_id"]),
        ("SessionHasExecutor", vec!["session_id"]),
        ("SessionHasComms", vec!["session_id"]),
        ("OpsLifecycleRegistry", vec!["session_id"]),
        ("InputState", vec!["session_id", "input_id"]),
        ("ListActiveInputs", vec!["session_id"]),
    ] {
        transitions.push(self_loop_transition_with(
            &format!("{variant}Idle"),
            "Idle",
            variant,
            bindings,
            vec![],
            vec![],
            vec![session_registered_guard()],
        ));
    }

    for (variant, bindings) in [("Abort", vec!["session_id"]), ("Wait", vec!["session_id"])] {
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
            bindings,
            vec![],
            vec![],
            vec![session_registered_guard()],
        ));
    }

    for phase in ["Attached", "Running", "Recovering", "Retired", "Stopped"] {
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

    for (variant, bindings, emit_variant) in [
        ("Ingest", vec!["runtime_id"], Some("ResolveAdmission")),
        ("PublishEvent", vec!["kind"], Some("IngressNotice")),
        (
            "AcceptWithCompletion",
            vec!["input_id"],
            Some("IngressAccepted"),
        ),
        (
            "AcceptWithoutWake",
            vec!["input_id"],
            Some("IngressAccepted"),
        ),
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

    for (variant, bindings) in [
        ("RuntimeState", vec!["runtime_id"]),
        ("LoadBoundaryReceipt", vec!["runtime_id", "sequence"]),
    ] {
        transitions.push(self_loop_transition_with(
            &format!("{variant}Attached"),
            "Attached",
            variant,
            bindings.clone(),
            vec![],
            vec![],
            vec![runtime_is_bound_guard()],
        ));
        transitions.push(self_loop_transition_with(
            &format!("{variant}Running"),
            "Running",
            variant,
            bindings,
            vec![],
            vec![],
            vec![runtime_is_bound_guard()],
        ));
    }

    for (variant, bindings) in [
        ("Prepare", vec!["session_id"]),
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

    for (variant, bindings, emit_variant) in [
        ("Commit", vec!["input_id", "run_id"], None),
        ("Fail", vec!["run_id"], Some("RecordTerminalOutcome")),
        ("AdmitQueued", vec![], Some("ResolveAdmission")),
        ("AdmitConsumedOnAccept", vec![], Some("ResolveAdmission")),
        ("StageDrainSnapshot", vec![], None),
        ("SupersedeQueuedInput", vec![], None),
        ("CoalesceQueuedInputs", vec![], None),
        (
            "SetSilentIntentOverrides",
            vec![],
            Some("SilentIntentApplied"),
        ),
        ("PrimitiveApplied", vec![], Some("SubmitRunPrimitive")),
        ("LlmReturnedToolCalls", vec![], None),
        ("LlmReturnedTerminal", vec![], Some("RecordTerminalOutcome")),
        ("RegisterPendingOps", vec![], Some("SubmitOpEvent")),
        ("ToolCallsResolved", vec![], Some("SubmitOpEvent")),
        ("OpsBarrierSatisfied", vec![], Some("SubmitOpEvent")),
        ("BoundaryContinue", vec![], None),
        ("BoundaryComplete", vec![], Some("RecordBoundarySequence")),
        ("RecoverableFailure", vec![], Some("RecordTerminalOutcome")),
        ("FatalFailure", vec![], Some("RecordTerminalOutcome")),
        ("RetryRequested", vec![], Some("SubmitRunPrimitive")),
        ("CancelNow", vec![], Some("RequestCancellationAtBoundary")),
        (
            "CancellationObserved",
            vec![],
            Some("RecordTerminalOutcome"),
        ),
        ("AcknowledgeTerminal", vec![], None),
        ("TurnLimitReached", vec![], Some("RecordTerminalOutcome")),
        ("BudgetExhausted", vec![], Some("RecordTerminalOutcome")),
        ("TimeBudgetExceeded", vec![], Some("RecordTerminalOutcome")),
        ("EnterExtraction", vec![], None),
        ("ExtractionValidationPassed", vec![], None),
        (
            "ExtractionValidationFailed",
            vec![],
            Some("RecordTerminalOutcome"),
        ),
        ("ExtractionStart", vec![], None),
        (
            "ForceCancelNoRun",
            vec![],
            Some("RequestCancellationAtBoundary"),
        ),
        ("RegisterOperation", vec![], Some("SubmitOpEvent")),
        ("ProvisioningSucceeded", vec![], Some("NotifyOpWatcher")),
        ("ProvisioningFailed", vec![], Some("NotifyOpWatcher")),
        ("AbortProvisioning", vec![], Some("NotifyOpWatcher")),
        ("PeerReady", vec![], Some("ExposeOperationPeer")),
        ("RegisterWatcher", vec![], Some("NotifyOpWatcher")),
        ("ProgressReported", vec![], Some("NotifyOpWatcher")),
        ("CompleteOperation", vec![], Some("CompletionResolved")),
        ("FailOperation", vec![], Some("CompletionResolved")),
        ("CancelOperation", vec![], Some("CompletionResolved")),
        ("RetireRequested", vec![], Some("CheckCompaction")),
        ("RetireCompleted", vec![], Some("CheckCompaction")),
        ("CollectTerminal", vec![], Some("CollectCompletedResult")),
        ("BeginWaitAll", vec![], None),
        ("CancelWaitAll", vec![], None),
    ] {
        transitions.push(self_loop_transition_with(
            &format!("{variant}Running"),
            "Running",
            variant,
            bindings,
            vec![],
            emit_variant.into_iter().map(simple_emit).collect(),
            vec![has_active_work_guard()],
        ));
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
            field: "active_work_id".into(),
            expr: Expr::None,
        },
        Update::Assign {
            field: "wake_pending".into(),
            expr: Expr::Bool(false),
        },
        Update::Assign {
            field: "process_pending".into(),
            expr: Expr::Bool(false),
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
            field: "interrupt_pending".into(),
            expr: Expr::Bool(false),
        },
        Update::Assign {
            field: "shutdown_pending".into(),
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

fn absorbed_meerkat_effect_variants() -> Vec<VariantSchema> {
    vec![
        variant("ResolveAdmission"),
        variant("SubmitAdmittedIngressEffect"),
        variant("SubmitRunPrimitive"),
        variant("ResolveCompletionAsTerminated"),
        variant("ApplyControlPlaneCommand"),
        variant("InitiateRecycle"),
        variant("IngressAccepted"),
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
        variant("WaitAllSatisfied"),
        variant("CollectCompletedResult"),
        variant("ConcurrencyLimitExceeded"),
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
        local_disposition("WaitAllSatisfied"),
        local_disposition("CollectCompletedResult"),
        external_disposition("ConcurrencyLimitExceeded"),
        local_disposition("EnqueueClassifiedEntry"),
        local_disposition("SpawnDrainTask"),
        local_disposition("ScheduleSurfaceCompletion"),
        external_disposition("RefreshVisibleSurfaceSet"),
        external_disposition("EmitExternalToolDelta"),
        local_disposition("CloseSurfaceConnection"),
        external_disposition("RejectSurfaceCall"),
    ]
}
