use indexmap::IndexMap;

use crate::{
    EffectDisposition, EffectDispositionRule, EffectEmit, EnumSchema, Expr, FieldSchema, Guard,
    InitSchema, InputMatch, InvariantSchema, MachineSchema, RustBinding, StateSchema,
    TransitionSchema, TypeRef, Update, VariantSchema, machine::TriggerKind,
};

pub fn mob_machine() -> MachineSchema {
    let mut trigger_variants = direct_mob_trigger_variants();
    trigger_variants.extend(absorbed_mob_input_variants());
    let (input_variants, signal_variants) = split_mob_variants(trigger_variants);

    MachineSchema {
        machine: "MobMachine".into(),
        version: 1,
        rust: RustBinding {
            crate_name: "meerkat-mob".into(),
            module: "generated::mob_machine".into(),
        },
        state: StateSchema {
            phase: EnumSchema {
                name: "MobPhase".into(),
                variants: vec![
                    variant("Creating"),
                    variant("Running"),
                    variant("Stopped"),
                    variant("Completed"),
                    variant("Destroyed"),
                ],
            },
            fields: vec![
                field(
                    "active_identity",
                    TypeRef::Option(Box::new(TypeRef::Named("AgentIdentity".into()))),
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
                    "current_generation",
                    TypeRef::Option(Box::new(TypeRef::Named("Generation".into()))),
                ),
                field(
                    "inflight_work_id",
                    TypeRef::Option(Box::new(TypeRef::Named("WorkId".into()))),
                ),
                field("active_member_count", TypeRef::U32),
                field("active_run_count", TypeRef::U32),
                field("pending_spawn_count", TypeRef::U32),
                field("retiring_member_count", TypeRef::U32),
                field("wiring_edge_count", TypeRef::U32),
                field("task_count", TypeRef::U32),
                field("event_subscription_count", TypeRef::U32),
                field("active_frame_count", TypeRef::U32),
                field("active_loop_count", TypeRef::U32),
                field("coordinator_bound", TypeRef::Bool),
                field("kickoff_pending", TypeRef::Bool),
            ],
            init: InitSchema {
                phase: "Creating".into(),
                fields: vec![
                    init("active_identity", Expr::None),
                    init("active_runtime_id", Expr::None),
                    init("active_fence_token", Expr::None),
                    init("current_generation", Expr::None),
                    init("inflight_work_id", Expr::None),
                    init("active_member_count", Expr::U64(0)),
                    init("active_run_count", Expr::U64(0)),
                    init("pending_spawn_count", Expr::U64(0)),
                    init("retiring_member_count", Expr::U64(0)),
                    init("wiring_edge_count", Expr::U64(0)),
                    init("task_count", Expr::U64(0)),
                    init("event_subscription_count", Expr::U64(0)),
                    init("active_frame_count", Expr::U64(0)),
                    init("active_loop_count", Expr::U64(0)),
                    init("coordinator_bound", Expr::Bool(false)),
                    init("kickoff_pending", Expr::Bool(false)),
                ],
            },
            terminal_phases: vec!["Destroyed".into()],
        },
        inputs: EnumSchema {
            name: "MobMachineInput".into(),
            variants: input_variants,
        },
        signals: EnumSchema {
            name: "MobMachineSignal".into(),
            variants: signal_variants,
        },
        effects: EnumSchema {
            name: "MobMachineEffect".into(),
            variants: {
                let mut variants = vec![
                    VariantSchema {
                        name: "RequestRuntimeBinding".into(),
                        fields: runtime_binding_request_fields(),
                    },
                    VariantSchema {
                        name: "SubmitMemberWork".into(),
                        fields: work_submission_fields(),
                    },
                    VariantSchema {
                        name: "RequestRuntimeRetire".into(),
                        fields: runtime_observation_fields(),
                    },
                    VariantSchema {
                        name: "RequestRuntimeDestroy".into(),
                        fields: runtime_observation_fields(),
                    },
                    VariantSchema {
                        name: "EmitMemberLifecycleNotice".into(),
                        fields: vec![
                            field("agent_identity", TypeRef::Named("AgentIdentity".into())),
                            field("kind", TypeRef::String),
                        ],
                    },
                ];
                variants.extend(absorbed_mob_effect_variants());
                variants
            },
        },
        helpers: vec![],
        derived: vec![],
        invariants: vec![
            InvariantSchema {
                name: "active_work_requires_runtime".into(),
                expr: Expr::Or(vec![
                    Expr::Eq(
                        Box::new(Expr::Field("inflight_work_id".into())),
                        Box::new(Expr::None),
                    ),
                    Expr::Neq(
                        Box::new(Expr::Field("active_runtime_id".into())),
                        Box::new(Expr::None),
                    ),
                ]),
            },
            InvariantSchema {
                name: "destroyed_has_no_active_runtime".into(),
                expr: Expr::Or(vec![
                    Expr::Neq(
                        Box::new(Expr::CurrentPhase),
                        Box::new(Expr::Phase("Destroyed".into())),
                    ),
                    Expr::Eq(
                        Box::new(Expr::Field("active_runtime_id".into())),
                        Box::new(Expr::None),
                    ),
                ]),
            },
            InvariantSchema {
                name: "active_runtime_has_identity".into(),
                expr: Expr::Or(vec![
                    Expr::Eq(
                        Box::new(Expr::Field("active_runtime_id".into())),
                        Box::new(Expr::None),
                    ),
                    Expr::Neq(
                        Box::new(Expr::Field("active_identity".into())),
                        Box::new(Expr::None),
                    ),
                ]),
            },
            InvariantSchema {
                name: "active_frames_require_runs".into(),
                expr: Expr::Or(vec![
                    Expr::Eq(
                        Box::new(Expr::Field("active_frame_count".into())),
                        Box::new(Expr::U64(0)),
                    ),
                    Expr::Gt(
                        Box::new(Expr::Field("active_run_count".into())),
                        Box::new(Expr::U64(0)),
                    ),
                ]),
            },
            InvariantSchema {
                name: "active_loops_require_frames".into(),
                expr: Expr::Or(vec![
                    Expr::Eq(
                        Box::new(Expr::Field("active_loop_count".into())),
                        Box::new(Expr::U64(0)),
                    ),
                    Expr::Gt(
                        Box::new(Expr::Field("active_frame_count".into())),
                        Box::new(Expr::U64(0)),
                    ),
                ]),
            },
            InvariantSchema {
                name: "retiring_members_do_not_exceed_active_members".into(),
                expr: Expr::Lte(
                    Box::new(Expr::Field("retiring_member_count".into())),
                    Box::new(Expr::Field("active_member_count".into())),
                ),
            },
            InvariantSchema {
                name: "kickoff_pending_requires_members".into(),
                expr: Expr::Or(vec![
                    Expr::Eq(
                        Box::new(Expr::Field("kickoff_pending".into())),
                        Box::new(Expr::Bool(false)),
                    ),
                    Expr::Gt(
                        Box::new(Expr::Field("active_member_count".into())),
                        Box::new(Expr::U64(0)),
                    ),
                ]),
            },
        ],
        transitions: vec![
            TransitionSchema {
                name: "Start".into(),
                from: vec!["Creating".into(), "Stopped".into()],
                on: InputMatch {
                    kind: mob_trigger_kind("Start"),
                    variant: "Start".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![],
                to: "Running".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "Spawn".into(),
                from: vec!["Creating".into(), "Running".into()],
                on: InputMatch {
                    kind: mob_trigger_kind("Spawn"),
                    variant: "Spawn".into(),
                    bindings: vec![
                        "agent_identity".into(),
                        "agent_runtime_id".into(),
                        "fence_token".into(),
                        "generation".into(),
                    ],
                },
                guards: vec![],
                updates: {
                    let mut updates = reset_member_runtime_updates();
                    updates.extend(vec![
                        assign_some("active_identity", "agent_identity"),
                        assign_some("active_runtime_id", "agent_runtime_id"),
                        assign_some("active_fence_token", "fence_token"),
                        assign_some("current_generation", "generation"),
                        Update::Assign {
                            field: "active_member_count".into(),
                            expr: Expr::U64(1),
                        },
                    ]);
                    updates
                },
                to: "Running".into(),
                emit: vec![
                    runtime_binding_emit("RequestRuntimeBinding"),
                    lifecycle_notice_emit("spawned"),
                ],
            },
            TransitionSchema {
                name: "ObserveRuntimeReady".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    kind: mob_trigger_kind("ObserveRuntimeReady"),
                    variant: "ObserveRuntimeReady".into(),
                    bindings: vec!["agent_runtime_id".into(), "fence_token".into()],
                },
                guards: vec![],
                updates: vec![],
                to: "Running".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "SubmitWork".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    kind: mob_trigger_kind("SubmitWork"),
                    variant: "SubmitWork".into(),
                    bindings: vec![
                        "agent_runtime_id".into(),
                        "fence_token".into(),
                        "work_id".into(),
                    ],
                },
                guards: vec![Guard {
                    name: "runtime_is_bound".into(),
                    expr: Expr::Neq(
                        Box::new(Expr::Field("active_runtime_id".into())),
                        Box::new(Expr::None),
                    ),
                }],
                updates: vec![
                    assign_some("inflight_work_id", "work_id"),
                    Update::Increment {
                        field: "active_run_count".into(),
                        amount: 1,
                    },
                ],
                to: "Running".into(),
                emit: vec![work_submission_emit("SubmitMemberWork", "work_id")],
            },
            TransitionSchema {
                name: "ObserveWorkCompleted".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    kind: mob_trigger_kind("ObserveWorkCompleted"),
                    variant: "ObserveWorkCompleted".into(),
                    bindings: vec![
                        "agent_runtime_id".into(),
                        "fence_token".into(),
                        "work_id".into(),
                    ],
                },
                guards: vec![],
                updates: clear_work_updates(),
                to: "Running".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "ObserveWorkFailed".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    kind: mob_trigger_kind("ObserveWorkFailed"),
                    variant: "ObserveWorkFailed".into(),
                    bindings: vec![
                        "agent_runtime_id".into(),
                        "fence_token".into(),
                        "work_id".into(),
                    ],
                },
                guards: vec![],
                updates: clear_work_updates(),
                to: "Running".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "ObserveWorkCancelled".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    kind: mob_trigger_kind("ObserveWorkCancelled"),
                    variant: "ObserveWorkCancelled".into(),
                    bindings: vec![
                        "agent_runtime_id".into(),
                        "fence_token".into(),
                        "work_id".into(),
                    ],
                },
                guards: vec![],
                updates: clear_work_updates(),
                to: "Running".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "RetireMember".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    kind: mob_trigger_kind("RetireMember"),
                    variant: "RetireMember".into(),
                    bindings: vec!["agent_runtime_id".into(), "fence_token".into()],
                },
                guards: vec![],
                updates: vec![],
                to: "Running".into(),
                emit: vec![runtime_observation_emit("RequestRuntimeRetire")],
            },
            TransitionSchema {
                name: "ObserveRuntimeRetired".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    kind: mob_trigger_kind("ObserveRuntimeRetired"),
                    variant: "ObserveRuntimeRetired".into(),
                    bindings: vec!["agent_runtime_id".into(), "fence_token".into()],
                },
                guards: vec![],
                updates: {
                    let mut updates = clear_runtime_projection_updates();
                    updates.splice(
                        0..0,
                        [
                            Update::Assign {
                                field: "active_runtime_id".into(),
                                expr: Expr::None,
                            },
                            Update::Assign {
                                field: "active_fence_token".into(),
                                expr: Expr::None,
                            },
                        ],
                    );
                    updates
                },
                to: "Stopped".into(),
                emit: vec![lifecycle_notice_emit("retired")],
            },
            TransitionSchema {
                name: "ResetMember".into(),
                from: vec!["Running".into(), "Stopped".into()],
                on: InputMatch {
                    kind: mob_trigger_kind("ResetMember"),
                    variant: "ResetMember".into(),
                    bindings: vec![
                        "agent_identity".into(),
                        "agent_runtime_id".into(),
                        "fence_token".into(),
                        "generation".into(),
                    ],
                },
                guards: vec![],
                updates: {
                    let mut updates = reset_member_runtime_updates();
                    updates.extend(vec![
                        assign_some("active_identity", "agent_identity"),
                        assign_some("active_runtime_id", "agent_runtime_id"),
                        assign_some("active_fence_token", "fence_token"),
                        assign_some("current_generation", "generation"),
                        Update::Assign {
                            field: "active_member_count".into(),
                            expr: Expr::U64(1),
                        },
                    ]);
                    updates
                },
                to: "Running".into(),
                emit: vec![
                    runtime_binding_emit("RequestRuntimeBinding"),
                    lifecycle_notice_emit("reset"),
                ],
            },
            TransitionSchema {
                name: "RespawnMember".into(),
                from: vec!["Creating".into(), "Running".into()],
                on: InputMatch {
                    kind: mob_trigger_kind("RespawnMember"),
                    variant: "RespawnMember".into(),
                    bindings: vec![
                        "agent_identity".into(),
                        "agent_runtime_id".into(),
                        "fence_token".into(),
                        "generation".into(),
                    ],
                },
                guards: vec![],
                updates: {
                    let mut updates = reset_member_runtime_updates();
                    updates.extend(vec![
                        assign_some("active_identity", "agent_identity"),
                        assign_some("active_runtime_id", "agent_runtime_id"),
                        assign_some("active_fence_token", "fence_token"),
                        assign_some("current_generation", "generation"),
                        Update::Assign {
                            field: "active_member_count".into(),
                            expr: Expr::U64(1),
                        },
                    ]);
                    updates
                },
                to: "Running".into(),
                emit: vec![
                    runtime_binding_emit("RequestRuntimeBinding"),
                    lifecycle_notice_emit("respawned"),
                ],
            },
            TransitionSchema {
                name: "MarkCompleted".into(),
                from: vec!["Running".into(), "Stopped".into()],
                on: InputMatch {
                    kind: mob_trigger_kind("MarkCompleted"),
                    variant: "MarkCompleted".into(),
                    bindings: vec![],
                },
                guards: vec![Guard {
                    name: "no_inflight_work".into(),
                    expr: Expr::Eq(
                        Box::new(Expr::Field("inflight_work_id".into())),
                        Box::new(Expr::None),
                    ),
                }],
                updates: vec![],
                to: "Completed".into(),
                emit: vec![lifecycle_notice_emit("completed")],
            },
            TransitionSchema {
                name: "DestroyMob".into(),
                from: vec![
                    "Creating".into(),
                    "Running".into(),
                    "Stopped".into(),
                    "Completed".into(),
                ],
                on: InputMatch {
                    kind: mob_trigger_kind("DestroyMob"),
                    variant: "DestroyMob".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: destroy_mob_projection_updates(),
                to: "Destroyed".into(),
                emit: vec![runtime_observation_emit("RequestRuntimeDestroy")],
            },
            TransitionSchema {
                name: "ObserveRuntimeDestroyed".into(),
                from: vec![
                    "Running".into(),
                    "Stopped".into(),
                    "Completed".into(),
                    "Destroyed".into(),
                ],
                on: InputMatch {
                    kind: mob_trigger_kind("ObserveRuntimeDestroyed"),
                    variant: "ObserveRuntimeDestroyed".into(),
                    bindings: vec!["agent_runtime_id".into(), "fence_token".into()],
                },
                guards: vec![],
                updates: destroy_mob_projection_updates(),
                to: "Destroyed".into(),
                emit: vec![lifecycle_notice_emit("destroyed")],
            },
        ]
        .into_iter()
        .chain(absorbed_mob_transitions())
        .collect(),
        ci_step_limit: Some(6),
        effect_dispositions: vec![
            routed_disposition("RequestRuntimeBinding", &["MeerkatMachine"]),
            routed_disposition("SubmitMemberWork", &["MeerkatMachine"]),
            routed_disposition("RequestRuntimeRetire", &["MeerkatMachine"]),
            routed_disposition("RequestRuntimeDestroy", &["MeerkatMachine"]),
            external_disposition("EmitMemberLifecycleNotice"),
        ]
        .into_iter()
        .chain(absorbed_mob_effect_dispositions())
        .collect(),
    }
}

fn identity_runtime_fields() -> Vec<FieldSchema> {
    vec![
        field("agent_identity", TypeRef::Named("AgentIdentity".into())),
        field("agent_runtime_id", TypeRef::Named("AgentRuntimeId".into())),
        field("fence_token", TypeRef::Named("FenceToken".into())),
        field("generation", TypeRef::Named("Generation".into())),
    ]
}

fn runtime_observation_fields() -> Vec<FieldSchema> {
    vec![
        field("agent_runtime_id", TypeRef::Named("AgentRuntimeId".into())),
        field("fence_token", TypeRef::Named("FenceToken".into())),
    ]
}

fn runtime_binding_request_fields() -> Vec<FieldSchema> {
    vec![
        field("agent_identity", TypeRef::Named("AgentIdentity".into())),
        field("agent_runtime_id", TypeRef::Named("AgentRuntimeId".into())),
        field("fence_token", TypeRef::Named("FenceToken".into())),
        field("generation", TypeRef::Named("Generation".into())),
    ]
}

fn work_submission_fields() -> Vec<FieldSchema> {
    vec![
        field("agent_runtime_id", TypeRef::Named("AgentRuntimeId".into())),
        field("fence_token", TypeRef::Named("FenceToken".into())),
        field("work_id", TypeRef::Named("WorkId".into())),
    ]
}

fn work_observation_fields() -> Vec<FieldSchema> {
    vec![
        field("agent_runtime_id", TypeRef::Named("AgentRuntimeId".into())),
        field("fence_token", TypeRef::Named("FenceToken".into())),
        field("work_id", TypeRef::Named("WorkId".into())),
    ]
}

fn runtime_binding_emit(variant: &str) -> EffectEmit {
    EffectEmit {
        variant: variant.into(),
        fields: IndexMap::from([
            ("agent_identity".into(), option_value("active_identity")),
            ("agent_runtime_id".into(), option_value("active_runtime_id")),
            ("fence_token".into(), option_value("active_fence_token")),
            ("generation".into(), option_value("current_generation")),
        ]),
    }
}

fn runtime_observation_emit(variant: &str) -> EffectEmit {
    EffectEmit {
        variant: variant.into(),
        fields: IndexMap::from([
            ("agent_runtime_id".into(), option_value("active_runtime_id")),
            ("fence_token".into(), option_value("active_fence_token")),
        ]),
    }
}

fn work_submission_emit(variant: &str, binding: &str) -> EffectEmit {
    EffectEmit {
        variant: variant.into(),
        fields: IndexMap::from([
            (
                "agent_runtime_id".into(),
                Expr::Binding("agent_runtime_id".into()),
            ),
            ("fence_token".into(), Expr::Binding("fence_token".into())),
            ("work_id".into(), Expr::Binding(binding.into())),
        ]),
    }
}

fn lifecycle_notice_emit(kind: &str) -> EffectEmit {
    EffectEmit {
        variant: "EmitMemberLifecycleNotice".into(),
        fields: IndexMap::from([
            ("agent_identity".into(), option_value("active_identity")),
            ("kind".into(), Expr::String(kind.into())),
        ]),
    }
}

fn simple_emit(variant: &str) -> EffectEmit {
    EffectEmit {
        variant: variant.into(),
        fields: IndexMap::new(),
    }
}

fn clear_work_updates() -> Vec<Update> {
    clear_runtime_projection_updates()
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

fn direct_mob_trigger_variants() -> Vec<VariantSchema> {
    vec![
        variant("Start"),
        VariantSchema {
            name: "Spawn".into(),
            fields: identity_runtime_fields(),
        },
        VariantSchema {
            name: "ObserveRuntimeReady".into(),
            fields: runtime_observation_fields(),
        },
        VariantSchema {
            name: "SubmitWork".into(),
            fields: work_submission_fields(),
        },
        VariantSchema {
            name: "ObserveWorkCompleted".into(),
            fields: work_observation_fields(),
        },
        VariantSchema {
            name: "ObserveWorkFailed".into(),
            fields: work_observation_fields(),
        },
        VariantSchema {
            name: "ObserveWorkCancelled".into(),
            fields: work_observation_fields(),
        },
        VariantSchema {
            name: "RetireMember".into(),
            fields: runtime_observation_fields(),
        },
        VariantSchema {
            name: "ObserveRuntimeRetired".into(),
            fields: runtime_observation_fields(),
        },
        VariantSchema {
            name: "ResetMember".into(),
            fields: identity_runtime_fields(),
        },
        VariantSchema {
            name: "RespawnMember".into(),
            fields: identity_runtime_fields(),
        },
        variant("DestroyMob"),
        VariantSchema {
            name: "ObserveRuntimeDestroyed".into(),
            fields: runtime_observation_fields(),
        },
        variant("MarkCompleted"),
    ]
}

fn split_mob_variants(variants: Vec<VariantSchema>) -> (Vec<VariantSchema>, Vec<VariantSchema>) {
    let mut inputs = Vec::new();
    let mut signals = Vec::new();
    for variant in variants {
        if is_mob_runtime_input_variant(&variant.name) {
            inputs.push(variant);
        } else {
            signals.push(variant);
        }
    }
    (inputs, signals)
}

fn is_mob_runtime_input_variant(variant: &str) -> bool {
    matches!(
        variant,
        "RunFlow"
            | "CancelFlow"
            | "FlowStatus"
            | "Spawn"
            | "Retire"
            | "Respawn"
            | "RetireAll"
            | "Wire"
            | "Unwire"
            | "ExternalTurn"
            | "InternalTurn"
            | "SubmitWork"
            | "CancelWork"
            | "CancelAllWork"
            | "Stop"
            | "Resume"
            | "Complete"
            | "Reset"
            | "Destroy"
            | "TaskCreate"
            | "TaskUpdate"
            | "TaskList"
            | "TaskGet"
            | "McpServerStates"
            | "RosterSnapshot"
            | "ListMembers"
            | "ListMembersIncludingRetiring"
            | "ListAllMembers"
            | "MemberStatus"
            | "SubscribeAgentEvents"
            | "SubscribeAllAgentEvents"
            | "SubscribeMobEvents"
            | "PollEvents"
            | "ReplayAllEvents"
            | "RecordOperatorActionProvenance"
            | "GetMember"
            | "SetSpawnPolicy"
            | "Shutdown"
            | "ForceCancel"
    )
}

fn absorbed_mob_input_variants() -> Vec<VariantSchema> {
    vec![
        variant("RunFlow"),
        variant("CancelFlow"),
        variant("FlowStatus"),
        VariantSchema {
            name: "Retire".into(),
            fields: vec![field("agent_runtime_id", named_runtime_id())],
        },
        VariantSchema {
            name: "Respawn".into(),
            fields: vec![field("agent_runtime_id", named_runtime_id())],
        },
        variant("RetireAll"),
        variant("Wire"),
        variant("Unwire"),
        variant("ExternalTurn"),
        variant("InternalTurn"),
        VariantSchema {
            name: "CancelWork".into(),
            fields: vec![field("work_id", named_work_id())],
        },
        VariantSchema {
            name: "CancelAllWork".into(),
            fields: runtime_observation_fields(),
        },
        variant("Stop"),
        variant("Resume"),
        variant("Complete"),
        variant("Reset"),
        variant("Destroy"),
        variant("TaskCreate"),
        variant("TaskUpdate"),
        variant("TaskList"),
        variant("TaskGet"),
        variant("McpServerStates"),
        variant("RosterSnapshot"),
        variant("ListMembers"),
        variant("ListMembersIncludingRetiring"),
        variant("ListAllMembers"),
        variant("MemberStatus"),
        variant("SubscribeAgentEvents"),
        variant("SubscribeAllAgentEvents"),
        variant("SubscribeMobEvents"),
        variant("PollEvents"),
        variant("ReplayAllEvents"),
        variant("RecordOperatorActionProvenance"),
        variant("GetMember"),
        variant("SetSpawnPolicy"),
        variant("Shutdown"),
        variant("ForceCancel"),
        variant("StartRun"),
        variant("FinishRun"),
        variant("BeginCleanup"),
        variant("FinishCleanup"),
        variant("InitializeOrchestrator"),
        variant("BindCoordinator"),
        variant("UnbindCoordinator"),
        variant("StageSpawn"),
        variant("CompleteSpawn"),
        variant("StartFlow"),
        variant("CompleteFlow"),
        variant("StopOrchestrator"),
        variant("ResumeOrchestrator"),
        variant("DestroyOrchestrator"),
        variant("ForceCancelMember"),
        variant("MemberPeerExposed"),
        variant("MemberTerminalized"),
        variant("OperationPeerTrusted"),
        variant("PeerInputAdmitted"),
        variant("RuntimeWorkAdmitted"),
        variant("KickoffStarted"),
        variant("KickoffCallbackPending"),
        variant("KickoffFailed"),
        variant("KickoffCancelled"),
        variant("KickoffForceCancelled"),
        variant("RuntimeRunSubmitted"),
        variant("RuntimeRunCompleted"),
        variant("RuntimeRunFailed"),
        variant("RuntimeRunCancelled"),
        variant("RuntimeStopRequested"),
        variant("CreateRun"),
        variant("DispatchStep"),
        variant("CompleteStep"),
        variant("RecordStepOutput"),
        variant("ConditionPassed"),
        variant("ConditionRejected"),
        variant("FailStep"),
        variant("SkipStep"),
        variant("ProjectFrameStepStatus"),
        variant("CancelStep"),
        variant("RegisterTargets"),
        variant("RecordTargetSuccess"),
        variant("RecordTargetTerminalFailure"),
        variant("RecordTargetCanceled"),
        variant("RecordTargetFailure"),
        variant("RegisterReadyFrame"),
        variant("RegisterPendingBodyFrame"),
        variant("NodeExecutionReleased"),
        variant("FrameTerminated"),
        variant("TerminalizeCompleted"),
        variant("TerminalizeFailed"),
        variant("TerminalizeCanceled"),
        variant("StartRootFrame"),
        variant("StartBodyFrame"),
        variant("CompleteNode"),
        variant("RecordNodeOutput"),
        variant("FailNode"),
        variant("SkipNode"),
        variant("CancelNode"),
        variant("StartLoop"),
        variant("BodyFrameStarted"),
        variant("BodyFrameCompleted"),
        variant("BodyFrameFailed"),
        variant("BodyFrameCanceled"),
        variant("UntilConditionMet"),
        variant("UntilConditionFailed"),
        variant("CancelLoop"),
    ]
}

fn absorbed_mob_transitions() -> Vec<TransitionSchema> {
    let mut transitions = Vec::new();

    // Read-only / observation commands: no phase guard in runtime — work from any state.
    let all_phases: Vec<String> = ["Creating", "Running", "Stopped", "Completed", "Destroyed"]
        .iter()
        .map(|p| (*p).into())
        .collect();
    for variant in [
        "FlowStatus",
        "McpServerStates",
        "RosterSnapshot",
        "ListMembers",
        "ListMembersIncludingRetiring",
        "ListAllMembers",
        "MemberStatus",
        "TaskList",
        "TaskGet",
        "PollEvents",
        "ReplayAllEvents",
        "RecordOperatorActionProvenance",
        "GetMember",
        "SetSpawnPolicy",
    ] {
        for phase in &all_phases {
            transitions.push(mob_self_loop_transition(
                variant,
                phase,
                variant,
                vec![],
                vec![],
                vec![],
            ));
        }
    }

    // --- Phase-changing transitions (NOT self-loops) ---

    // Stop: Running → Stopped
    transitions.push(TransitionSchema {
        name: "StopRunning".into(),
        from: vec!["Running".into()],
        on: InputMatch {
            kind: mob_trigger_kind("Stop"),
            variant: "Stop".into(),
            bindings: vec![],
        },
        guards: vec![],
        updates: clear_runtime_projection_updates(),
        to: "Stopped".into(),
        emit: vec![simple_emit("EmitRunLifecycleNotice")],
    });

    // Resume: Stopped → Running
    transitions.push(TransitionSchema {
        name: "ResumeStopped".into(),
        from: vec!["Stopped".into()],
        on: InputMatch {
            kind: mob_trigger_kind("Resume"),
            variant: "Resume".into(),
            bindings: vec![],
        },
        guards: vec![],
        updates: vec![],
        to: "Running".into(),
        emit: vec![simple_emit("EmitRunLifecycleNotice")],
    });

    // Complete: Running → Completed
    transitions.push(TransitionSchema {
        name: "CompleteRunning".into(),
        from: vec!["Running".into()],
        on: InputMatch {
            kind: mob_trigger_kind("Complete"),
            variant: "Complete".into(),
            bindings: vec![],
        },
        guards: vec![],
        updates: clear_runtime_projection_updates(),
        to: "Completed".into(),
        emit: vec![simple_emit("EmitRunLifecycleNotice")],
    });

    // Reset: Running|Stopped|Completed → Running
    transitions.push(TransitionSchema {
        name: "ResetToRunning".into(),
        from: vec!["Running".into(), "Stopped".into(), "Completed".into()],
        on: InputMatch {
            kind: mob_trigger_kind("Reset"),
            variant: "Reset".into(),
            bindings: vec![],
        },
        guards: vec![],
        updates: reset_mob_projection_updates(),
        to: "Running".into(),
        emit: vec![simple_emit("EmitRunLifecycleNotice")],
    });

    // --- Multi-phase self-loops (Creating + Running) ---
    for (variant, emit_variant, updates) in [
        (
            "Wire",
            Some("NotifyCoordinator"),
            vec![Update::Increment {
                field: "wiring_edge_count".into(),
                amount: 1,
            }],
        ),
        ("ExternalTurn", Some("EmitProgressNote"), vec![]),
        ("InternalTurn", Some("EmitProgressNote"), vec![]),
        (
            "TaskCreate",
            Some("EmitTaskNotice"),
            vec![Update::Increment {
                field: "task_count".into(),
                amount: 1,
            }],
        ),
        ("TaskUpdate", Some("EmitTaskNotice"), vec![]),
        (
            "ForceCancel",
            Some("FlowTerminalized"),
            clear_runtime_projection_updates(),
        ),
    ] {
        for phase in ["Creating", "Running"] {
            transitions.push(mob_self_loop_transition(
                variant,
                phase,
                variant,
                vec![],
                updates.clone(),
                emit_variant.into_iter().map(simple_emit).collect(),
            ));
        }
    }

    // --- Subscribe commands: no phase guard in runtime ---
    for (variant, updates) in [
        (
            "SubscribeAgentEvents",
            vec![Update::Increment {
                field: "event_subscription_count".into(),
                amount: 1,
            }],
        ),
        (
            "SubscribeAllAgentEvents",
            vec![Update::Increment {
                field: "event_subscription_count".into(),
                amount: 1,
            }],
        ),
        (
            "SubscribeMobEvents",
            vec![Update::Increment {
                field: "event_subscription_count".into(),
                amount: 1,
            }],
        ),
    ] {
        for phase in &all_phases {
            transitions.push(mob_self_loop_transition(
                variant,
                phase,
                variant,
                vec![],
                updates.clone(),
                vec![],
            ));
        }
    }

    // Shutdown: from any non-Destroyed state. Running → Stopped; others self-loop.
    transitions.push(TransitionSchema {
        name: "ShutdownRunning".into(),
        from: vec!["Running".into()],
        on: InputMatch {
            kind: mob_trigger_kind("Shutdown"),
            variant: "Shutdown".into(),
            bindings: vec![],
        },
        guards: vec![],
        updates: clear_runtime_projection_updates(),
        to: "Stopped".into(),
        emit: vec![simple_emit("EmitRunLifecycleNotice")],
    });
    for phase in ["Creating", "Stopped", "Completed"] {
        transitions.push(mob_self_loop_transition(
            "Shutdown",
            phase,
            "Shutdown",
            vec![],
            clear_runtime_projection_updates(),
            vec![simple_emit("EmitRunLifecycleNotice")],
        ));
    }

    // --- Signals and other Running-only self-loops ---
    for (variant, emit_variant, updates) in [
        ("CancelFlow", Some("FlowTerminalized"), clear_work_updates()),
        (
            "InitializeOrchestrator",
            Some("NotifyCoordinator"),
            vec![Update::Assign {
                field: "coordinator_bound".into(),
                expr: Expr::Bool(true),
            }],
        ),
        (
            "BindCoordinator",
            Some("NotifyCoordinator"),
            vec![Update::Assign {
                field: "coordinator_bound".into(),
                expr: Expr::Bool(true),
            }],
        ),
        (
            "UnbindCoordinator",
            Some("NotifyCoordinator"),
            vec![Update::Assign {
                field: "coordinator_bound".into(),
                expr: Expr::Bool(false),
            }],
        ),
        (
            "StageSpawn",
            Some("ExposePendingSpawn"),
            vec![Update::Increment {
                field: "pending_spawn_count".into(),
                amount: 1,
            }],
        ),
        (
            "StopOrchestrator",
            Some("NotifyCoordinator"),
            vec![Update::Assign {
                field: "coordinator_bound".into(),
                expr: Expr::Bool(false),
            }],
        ),
        (
            "ResumeOrchestrator",
            Some("NotifyCoordinator"),
            vec![Update::Assign {
                field: "coordinator_bound".into(),
                expr: Expr::Bool(true),
            }],
        ),
        (
            "DestroyOrchestrator",
            Some("NotifyCoordinator"),
            vec![Update::Assign {
                field: "coordinator_bound".into(),
                expr: Expr::Bool(false),
            }],
        ),
        (
            "ForceCancelMember",
            Some("EmitMemberTerminalNotice"),
            vec![],
        ),
        ("MemberPeerExposed", Some("AdmitPeerInput"), vec![]),
        (
            "MemberTerminalized",
            Some("EmitMemberTerminalNotice"),
            vec![],
        ),
        ("OperationPeerTrusted", Some("AdmitPeerInput"), vec![]),
        ("PeerInputAdmitted", Some("AdmitPeerInput"), vec![]),
        ("RuntimeWorkAdmitted", Some("AdmitStepWork"), vec![]),
        (
            "KickoffFailed",
            Some("EmitMemberTerminalNotice"),
            vec![Update::Assign {
                field: "kickoff_pending".into(),
                expr: Expr::Bool(false),
            }],
        ),
        (
            "KickoffCancelled",
            Some("EmitMemberTerminalNotice"),
            vec![Update::Assign {
                field: "kickoff_pending".into(),
                expr: Expr::Bool(false),
            }],
        ),
        (
            "KickoffForceCancelled",
            Some("EmitMemberTerminalNotice"),
            vec![Update::Assign {
                field: "kickoff_pending".into(),
                expr: Expr::Bool(false),
            }],
        ),
        (
            "RuntimeRunSubmitted",
            Some("EmitRunLifecycleNotice"),
            vec![],
        ),
        (
            "RuntimeRunCompleted",
            Some("EmitRunLifecycleNotice"),
            clear_work_updates(),
        ),
        (
            "RuntimeRunFailed",
            Some("EmitRunLifecycleNotice"),
            clear_work_updates(),
        ),
        (
            "RuntimeRunCancelled",
            Some("EmitRunLifecycleNotice"),
            clear_work_updates(),
        ),
        (
            "RuntimeStopRequested",
            Some("EmitRunLifecycleNotice"),
            vec![],
        ),
        ("DispatchStep", Some("AdmitStepWork"), vec![]),
        ("CompleteStep", Some("EmitStepNotice"), vec![]),
        ("RecordStepOutput", Some("PersistStepOutput"), vec![]),
        ("ConditionPassed", Some("EmitStepNotice"), vec![]),
        ("ConditionRejected", Some("EmitStepNotice"), vec![]),
        ("FailStep", Some("EmitStepNotice"), vec![]),
        ("SkipStep", Some("EmitStepNotice"), vec![]),
        ("ProjectFrameStepStatus", Some("EmitStepNotice"), vec![]),
        ("CancelStep", Some("EmitStepNotice"), vec![]),
        ("RegisterTargets", Some("NotifyCoordinator"), vec![]),
        ("RecordTargetSuccess", Some("ProjectTargetSuccess"), vec![]),
        (
            "RecordTargetTerminalFailure",
            Some("ProjectTargetFailure"),
            vec![],
        ),
        (
            "RecordTargetCanceled",
            Some("ProjectTargetCanceled"),
            vec![],
        ),
        ("RecordTargetFailure", Some("ProjectTargetFailure"), vec![]),
        (
            "NodeExecutionReleased",
            Some("NodeExecutionReleased"),
            vec![],
        ),
        ("TerminalizeCompleted", Some("RootFrameCompleted"), vec![]),
        ("TerminalizeFailed", Some("RootFrameFailed"), vec![]),
        ("TerminalizeCanceled", Some("RootFrameCanceled"), vec![]),
        ("CompleteNode", Some("EmitStepNotice"), vec![]),
        ("RecordNodeOutput", Some("PersistStepOutput"), vec![]),
        ("FailNode", Some("EmitStepNotice"), vec![]),
        ("SkipNode", Some("EmitStepNotice"), vec![]),
        ("CancelNode", Some("EmitStepNotice"), vec![]),
        ("UntilConditionMet", Some("EvaluateUntilCondition"), vec![]),
        ("BeginCleanup", Some("EmitRunLifecycleNotice"), vec![]),
        ("FinishCleanup", Some("EmitRunLifecycleNotice"), vec![]),
    ] {
        transitions.push(mob_self_loop_transition(
            variant,
            "Running",
            variant,
            vec![],
            updates,
            emit_variant.into_iter().map(simple_emit).collect(),
        ));
    }

    transitions.push(TransitionSchema {
        name: "KickoffStartedRunning".into(),
        from: vec!["Running".into()],
        on: InputMatch {
            kind: mob_trigger_kind("KickoffStarted"),
            variant: "KickoffStarted".into(),
            bindings: vec![],
        },
        guards: vec![Guard {
            name: "active_members_present".into(),
            expr: Expr::Gt(
                Box::new(Expr::Field("active_member_count".into())),
                Box::new(Expr::U64(0)),
            ),
        }],
        updates: vec![Update::Assign {
            field: "kickoff_pending".into(),
            expr: Expr::Bool(true),
        }],
        to: "Running".into(),
        emit: vec![simple_emit("AdmitKickoffTurn")],
    });

    transitions.push(TransitionSchema {
        name: "KickoffCallbackPendingRunning".into(),
        from: vec!["Running".into()],
        on: InputMatch {
            kind: mob_trigger_kind("KickoffCallbackPending"),
            variant: "KickoffCallbackPending".into(),
            bindings: vec![],
        },
        guards: vec![Guard {
            name: "active_members_present".into(),
            expr: Expr::Gt(
                Box::new(Expr::Field("active_member_count".into())),
                Box::new(Expr::U64(0)),
            ),
        }],
        updates: vec![Update::Assign {
            field: "kickoff_pending".into(),
            expr: Expr::Bool(true),
        }],
        to: "Running".into(),
        emit: vec![],
    });

    transitions.push(TransitionSchema {
        name: "RunFlowRunning".into(),
        from: vec!["Running".into()],
        on: InputMatch {
            kind: mob_trigger_kind("RunFlow"),
            variant: "RunFlow".into(),
            bindings: vec![],
        },
        guards: vec![active_members_present_guard(), runtime_is_bound_guard()],
        updates: vec![Update::Increment {
            field: "active_run_count".into(),
            amount: 1,
        }],
        to: "Running".into(),
        emit: vec![simple_emit("EmitFlowRunNotice")],
    });

    transitions.push(TransitionSchema {
        name: "StartFlowRunning".into(),
        from: vec!["Running".into()],
        on: InputMatch {
            kind: mob_trigger_kind("StartFlow"),
            variant: "StartFlow".into(),
            bindings: vec![],
        },
        guards: vec![active_members_present_guard(), runtime_is_bound_guard()],
        updates: vec![Update::Increment {
            field: "active_run_count".into(),
            amount: 1,
        }],
        to: "Running".into(),
        emit: vec![simple_emit("EmitFlowRunNotice")],
    });

    transitions.push(TransitionSchema {
        name: "CreateRunRunning".into(),
        from: vec!["Running".into()],
        on: InputMatch {
            kind: mob_trigger_kind("CreateRun"),
            variant: "CreateRun".into(),
            bindings: vec![],
        },
        guards: vec![active_members_present_guard(), runtime_is_bound_guard()],
        updates: vec![Update::Increment {
            field: "active_run_count".into(),
            amount: 1,
        }],
        to: "Running".into(),
        emit: vec![simple_emit("EmitRunLifecycleNotice")],
    });

    transitions.push(TransitionSchema {
        name: "StartRunRunning".into(),
        from: vec!["Running".into()],
        on: InputMatch {
            kind: mob_trigger_kind("StartRun"),
            variant: "StartRun".into(),
            bindings: vec![],
        },
        guards: vec![active_members_present_guard(), runtime_is_bound_guard()],
        updates: vec![Update::Increment {
            field: "active_run_count".into(),
            amount: 1,
        }],
        to: "Running".into(),
        emit: vec![simple_emit("EmitRunLifecycleNotice")],
    });

    transitions.push(TransitionSchema {
        name: "RegisterReadyFrameRunning".into(),
        from: vec!["Running".into()],
        on: InputMatch {
            kind: mob_trigger_kind("RegisterReadyFrame"),
            variant: "RegisterReadyFrame".into(),
            bindings: vec![],
        },
        guards: vec![Guard {
            name: "active_runs_present".into(),
            expr: Expr::Gt(
                Box::new(Expr::Field("active_run_count".into())),
                Box::new(Expr::U64(0)),
            ),
        }],
        updates: vec![Update::Increment {
            field: "active_frame_count".into(),
            amount: 1,
        }],
        to: "Running".into(),
        emit: vec![],
    });

    // Unwire: per-phase self-loops for Creating and Running
    for phase in ["Creating", "Running"] {
        transitions.push(TransitionSchema {
            name: format!("Unwire{phase}"),
            from: vec![phase.into()],
            on: InputMatch {
                kind: mob_trigger_kind("Unwire"),
                variant: "Unwire".into(),
                bindings: vec![],
            },
            guards: vec![Guard {
                name: "wired_edges_present".into(),
                expr: Expr::Gt(
                    Box::new(Expr::Field("wiring_edge_count".into())),
                    Box::new(Expr::U64(0)),
                ),
            }],
            updates: vec![Update::Decrement {
                field: "wiring_edge_count".into(),
                amount: 1,
            }],
            to: phase.into(),
            emit: vec![simple_emit("NotifyCoordinator")],
        });
    }

    transitions.push(TransitionSchema {
        name: "RegisterPendingBodyFrameRunning".into(),
        from: vec!["Running".into()],
        on: InputMatch {
            kind: mob_trigger_kind("RegisterPendingBodyFrame"),
            variant: "RegisterPendingBodyFrame".into(),
            bindings: vec![],
        },
        guards: vec![Guard {
            name: "active_runs_present".into(),
            expr: Expr::Gt(
                Box::new(Expr::Field("active_run_count".into())),
                Box::new(Expr::U64(0)),
            ),
        }],
        updates: vec![Update::Increment {
            field: "active_frame_count".into(),
            amount: 1,
        }],
        to: "Running".into(),
        emit: vec![simple_emit("RequestBodyFrameStart")],
    });

    transitions.push(TransitionSchema {
        name: "CompleteFlowRunning".into(),
        from: vec!["Running".into()],
        on: InputMatch {
            kind: mob_trigger_kind("CompleteFlow"),
            variant: "CompleteFlow".into(),
            bindings: vec![],
        },
        guards: vec![Guard {
            name: "active_runs_present".into(),
            expr: Expr::Gt(
                Box::new(Expr::Field("active_run_count".into())),
                Box::new(Expr::U64(0)),
            ),
        }],
        updates: clear_runtime_projection_updates(),
        to: "Running".into(),
        emit: vec![simple_emit("FlowTerminalized")],
    });

    transitions.push(TransitionSchema {
        name: "StartRootFrameRunning".into(),
        from: vec!["Running".into()],
        on: InputMatch {
            kind: mob_trigger_kind("StartRootFrame"),
            variant: "StartRootFrame".into(),
            bindings: vec![],
        },
        guards: vec![Guard {
            name: "active_runs_present".into(),
            expr: Expr::Gt(
                Box::new(Expr::Field("active_run_count".into())),
                Box::new(Expr::U64(0)),
            ),
        }],
        updates: vec![Update::Increment {
            field: "active_frame_count".into(),
            amount: 1,
        }],
        to: "Running".into(),
        emit: vec![],
    });

    transitions.push(TransitionSchema {
        name: "StartBodyFrameRunning".into(),
        from: vec!["Running".into()],
        on: InputMatch {
            kind: mob_trigger_kind("StartBodyFrame"),
            variant: "StartBodyFrame".into(),
            bindings: vec![],
        },
        guards: vec![Guard {
            name: "active_runs_present".into(),
            expr: Expr::Gt(
                Box::new(Expr::Field("active_run_count".into())),
                Box::new(Expr::U64(0)),
            ),
        }],
        updates: vec![Update::Increment {
            field: "active_frame_count".into(),
            amount: 1,
        }],
        to: "Running".into(),
        emit: vec![simple_emit("RequestBodyFrameStart")],
    });

    transitions.push(TransitionSchema {
        name: "FrameTerminatedRunning".into(),
        from: vec!["Running".into()],
        on: InputMatch {
            kind: mob_trigger_kind("FrameTerminated"),
            variant: "FrameTerminated".into(),
            bindings: vec![],
        },
        guards: vec![Guard {
            name: "active_frames_present".into(),
            expr: Expr::Gt(
                Box::new(Expr::Field("active_frame_count".into())),
                Box::new(Expr::U64(0)),
            ),
        }],
        updates: vec![
            Update::Decrement {
                field: "active_frame_count".into(),
                amount: 1,
            },
            Update::Assign {
                field: "active_loop_count".into(),
                expr: Expr::U64(0),
            },
        ],
        to: "Running".into(),
        emit: vec![],
    });

    transitions.push(TransitionSchema {
        name: "StartLoopRunning".into(),
        from: vec!["Running".into()],
        on: InputMatch {
            kind: mob_trigger_kind("StartLoop"),
            variant: "StartLoop".into(),
            bindings: vec![],
        },
        guards: vec![
            Guard {
                name: "active_runs_present".into(),
                expr: Expr::Gt(
                    Box::new(Expr::Field("active_run_count".into())),
                    Box::new(Expr::U64(0)),
                ),
            },
            Guard {
                name: "active_frames_present".into(),
                expr: Expr::Gt(
                    Box::new(Expr::Field("active_frame_count".into())),
                    Box::new(Expr::U64(0)),
                ),
            },
        ],
        updates: vec![Update::Increment {
            field: "active_loop_count".into(),
            amount: 1,
        }],
        to: "Running".into(),
        emit: vec![simple_emit("StartLoopNode")],
    });

    transitions.push(TransitionSchema {
        name: "BodyFrameStartedRunning".into(),
        from: vec!["Running".into()],
        on: InputMatch {
            kind: mob_trigger_kind("BodyFrameStarted"),
            variant: "BodyFrameStarted".into(),
            bindings: vec![],
        },
        guards: vec![Guard {
            name: "active_runs_present".into(),
            expr: Expr::Gt(
                Box::new(Expr::Field("active_run_count".into())),
                Box::new(Expr::U64(0)),
            ),
        }],
        updates: vec![Update::Increment {
            field: "active_frame_count".into(),
            amount: 1,
        }],
        to: "Running".into(),
        emit: vec![simple_emit("RequestBodyFrameStart")],
    });

    transitions.push(TransitionSchema {
        name: "BodyFrameCompletedRunning".into(),
        from: vec!["Running".into()],
        on: InputMatch {
            kind: mob_trigger_kind("BodyFrameCompleted"),
            variant: "BodyFrameCompleted".into(),
            bindings: vec![],
        },
        guards: vec![Guard {
            name: "active_frames_present".into(),
            expr: Expr::Gt(
                Box::new(Expr::Field("active_frame_count".into())),
                Box::new(Expr::U64(0)),
            ),
        }],
        updates: vec![
            Update::Decrement {
                field: "active_frame_count".into(),
                amount: 1,
            },
            Update::Assign {
                field: "active_loop_count".into(),
                expr: Expr::U64(0),
            },
        ],
        to: "Running".into(),
        emit: vec![simple_emit("BodyFrameCompleted")],
    });

    transitions.push(TransitionSchema {
        name: "BodyFrameFailedRunning".into(),
        from: vec!["Running".into()],
        on: InputMatch {
            kind: mob_trigger_kind("BodyFrameFailed"),
            variant: "BodyFrameFailed".into(),
            bindings: vec![],
        },
        guards: vec![Guard {
            name: "active_frames_present".into(),
            expr: Expr::Gt(
                Box::new(Expr::Field("active_frame_count".into())),
                Box::new(Expr::U64(0)),
            ),
        }],
        updates: vec![
            Update::Decrement {
                field: "active_frame_count".into(),
                amount: 1,
            },
            Update::Assign {
                field: "active_loop_count".into(),
                expr: Expr::U64(0),
            },
        ],
        to: "Running".into(),
        emit: vec![simple_emit("BodyFrameFailed")],
    });

    transitions.push(TransitionSchema {
        name: "BodyFrameCanceledRunning".into(),
        from: vec!["Running".into()],
        on: InputMatch {
            kind: mob_trigger_kind("BodyFrameCanceled"),
            variant: "BodyFrameCanceled".into(),
            bindings: vec![],
        },
        guards: vec![Guard {
            name: "active_frames_present".into(),
            expr: Expr::Gt(
                Box::new(Expr::Field("active_frame_count".into())),
                Box::new(Expr::U64(0)),
            ),
        }],
        updates: vec![
            Update::Decrement {
                field: "active_frame_count".into(),
                amount: 1,
            },
            Update::Assign {
                field: "active_loop_count".into(),
                expr: Expr::U64(0),
            },
        ],
        to: "Running".into(),
        emit: vec![simple_emit("BodyFrameCanceled")],
    });

    transitions.push(TransitionSchema {
        name: "UntilConditionFailedRunning".into(),
        from: vec!["Running".into()],
        on: InputMatch {
            kind: mob_trigger_kind("UntilConditionFailed"),
            variant: "UntilConditionFailed".into(),
            bindings: vec![],
        },
        guards: vec![Guard {
            name: "active_loops_present".into(),
            expr: Expr::Gt(
                Box::new(Expr::Field("active_loop_count".into())),
                Box::new(Expr::U64(0)),
            ),
        }],
        updates: vec![Update::Decrement {
            field: "active_loop_count".into(),
            amount: 1,
        }],
        to: "Running".into(),
        emit: vec![simple_emit("LoopCompleted")],
    });

    transitions.push(TransitionSchema {
        name: "CancelLoopRunning".into(),
        from: vec!["Running".into()],
        on: InputMatch {
            kind: mob_trigger_kind("CancelLoop"),
            variant: "CancelLoop".into(),
            bindings: vec![],
        },
        guards: vec![Guard {
            name: "active_loops_present".into(),
            expr: Expr::Gt(
                Box::new(Expr::Field("active_loop_count".into())),
                Box::new(Expr::U64(0)),
            ),
        }],
        updates: vec![Update::Decrement {
            field: "active_loop_count".into(),
            amount: 1,
        }],
        to: "Running".into(),
        emit: vec![simple_emit("LoopCanceled")],
    });

    transitions.push(TransitionSchema {
        name: "FinishRunRunning".into(),
        from: vec!["Running".into()],
        on: InputMatch {
            kind: mob_trigger_kind("FinishRun"),
            variant: "FinishRun".into(),
            bindings: vec![],
        },
        guards: vec![Guard {
            name: "active_runs_present".into(),
            expr: Expr::Gt(
                Box::new(Expr::Field("active_run_count".into())),
                Box::new(Expr::U64(0)),
            ),
        }],
        updates: clear_runtime_projection_updates(),
        to: "Running".into(),
        emit: vec![simple_emit("EmitRunLifecycleNotice")],
    });

    // Retire: per-phase self-loops for Creating, Running, Stopped
    let retire_guards = vec![
        Guard {
            name: "active_members_present".into(),
            expr: Expr::Gt(
                Box::new(Expr::Field("active_member_count".into())),
                Box::new(Expr::U64(0)),
            ),
        },
        Guard {
            name: "unretired_members_present".into(),
            expr: Expr::Gt(
                Box::new(Expr::Field("active_member_count".into())),
                Box::new(Expr::Field("retiring_member_count".into())),
            ),
        },
    ];
    for phase in ["Creating", "Running", "Stopped"] {
        transitions.push(TransitionSchema {
            name: format!("Retire{phase}"),
            from: vec![phase.into()],
            on: InputMatch {
                kind: mob_trigger_kind("Retire"),
                variant: "Retire".into(),
                bindings: vec!["agent_runtime_id".into()],
            },
            guards: retire_guards.clone(),
            updates: vec![Update::Increment {
                field: "retiring_member_count".into(),
                amount: 1,
            }],
            to: phase.into(),
            emit: vec![runtime_observation_emit("RequestRuntimeRetire")],
        });
    }

    // RetireAll: per-phase self-loops for Creating, Running, Stopped
    for phase in ["Creating", "Running", "Stopped"] {
        transitions.push(TransitionSchema {
            name: format!("RetireAll{phase}"),
            from: vec![phase.into()],
            on: InputMatch {
                kind: mob_trigger_kind("RetireAll"),
                variant: "RetireAll".into(),
                bindings: vec![],
            },
            guards: vec![],
            updates: vec![Update::Assign {
                field: "retiring_member_count".into(),
                expr: Expr::Field("active_member_count".into()),
            }],
            to: phase.into(),
            emit: vec![lifecycle_notice_emit("retiring")],
        });
    }

    transitions.push(TransitionSchema {
        name: "CompleteSpawnRunning".into(),
        from: vec!["Running".into()],
        on: InputMatch {
            kind: mob_trigger_kind("CompleteSpawn"),
            variant: "CompleteSpawn".into(),
            bindings: vec![],
        },
        guards: vec![Guard {
            name: "pending_spawns_present".into(),
            expr: Expr::Gt(
                Box::new(Expr::Field("pending_spawn_count".into())),
                Box::new(Expr::U64(0)),
            ),
        }],
        updates: vec![
            Update::Decrement {
                field: "pending_spawn_count".into(),
                amount: 1,
            },
            Update::Increment {
                field: "active_member_count".into(),
                amount: 1,
            },
        ],
        to: "Running".into(),
        emit: vec![lifecycle_notice_emit("spawned")],
    });

    // Destroy input: runtime allows from all non-Destroyed phases via lifecycle authority.
    transitions.push(TransitionSchema {
        name: "DestroyFromAny".into(),
        from: vec![
            "Creating".into(),
            "Running".into(),
            "Stopped".into(),
            "Completed".into(),
        ],
        on: InputMatch {
            kind: mob_trigger_kind("Destroy"),
            variant: "Destroy".into(),
            bindings: vec![],
        },
        guards: vec![],
        updates: destroy_mob_projection_updates(),
        to: "Destroyed".into(),
        emit: vec![lifecycle_notice_emit("destroyed")],
    });

    // Respawn: from Creating or Running (self-loop per phase)
    for phase in ["Creating", "Running"] {
        transitions.push(mob_self_loop_transition(
            "Respawn",
            phase,
            "Respawn",
            vec!["agent_runtime_id"],
            vec![Update::Increment {
                field: "pending_spawn_count".into(),
                amount: 1,
            }],
            vec![simple_emit("ExposePendingSpawn")],
        ));
    }

    // CancelWork: always errors in runtime, no phase check. Running-only is fine.
    transitions.push(mob_self_loop_transition(
        "CancelWork",
        "Running",
        "CancelWork",
        vec!["work_id"],
        clear_work_updates(),
        vec![simple_emit("FlowTerminalized")],
    ));

    // CancelAllWork: delegates to ForceCancel which accepts [Creating, Running]
    for phase in ["Creating", "Running"] {
        transitions.push(mob_self_loop_transition(
            "CancelAllWork",
            phase,
            "CancelAllWork",
            vec![],
            clear_work_updates(),
            vec![simple_emit("FlowTerminalized")],
        ));
    }

    transitions
}

fn mob_self_loop_transition(
    name: &str,
    phase: &str,
    variant: &str,
    bindings: Vec<&str>,
    updates: Vec<Update>,
    emit: Vec<EffectEmit>,
) -> TransitionSchema {
    TransitionSchema {
        name: format!("{name}{phase}"),
        from: vec![phase.into()],
        on: InputMatch {
            kind: mob_trigger_kind(variant),
            variant: variant.into(),
            bindings: bindings.into_iter().map(Into::into).collect(),
        },
        guards: vec![],
        updates,
        to: phase.into(),
        emit,
    }
}

fn mob_trigger_kind(variant: &str) -> TriggerKind {
    if is_mob_runtime_input_variant(variant) {
        TriggerKind::Input
    } else {
        TriggerKind::Signal
    }
}

fn active_members_present_guard() -> Guard {
    Guard {
        name: "active_members_present".into(),
        expr: Expr::Gt(
            Box::new(Expr::Field("active_member_count".into())),
            Box::new(Expr::U64(0)),
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

fn clear_runtime_projection_updates() -> Vec<Update> {
    vec![
        Update::Assign {
            field: "inflight_work_id".into(),
            expr: Expr::None,
        },
        Update::Assign {
            field: "active_run_count".into(),
            expr: Expr::U64(0),
        },
        Update::Assign {
            field: "active_frame_count".into(),
            expr: Expr::U64(0),
        },
        Update::Assign {
            field: "active_loop_count".into(),
            expr: Expr::U64(0),
        },
        Update::Assign {
            field: "kickoff_pending".into(),
            expr: Expr::Bool(false),
        },
    ]
}

fn reset_member_runtime_updates() -> Vec<Update> {
    let mut updates = clear_runtime_projection_updates();
    updates.extend(vec![
        Update::Assign {
            field: "pending_spawn_count".into(),
            expr: Expr::U64(0),
        },
        Update::Assign {
            field: "retiring_member_count".into(),
            expr: Expr::U64(0),
        },
    ]);
    updates
}

fn reset_mob_projection_updates() -> Vec<Update> {
    let mut updates = clear_runtime_projection_updates();
    updates.extend(vec![
        Update::Assign {
            field: "pending_spawn_count".into(),
            expr: Expr::U64(0),
        },
        Update::Assign {
            field: "retiring_member_count".into(),
            expr: Expr::U64(0),
        },
        Update::Assign {
            field: "wiring_edge_count".into(),
            expr: Expr::U64(0),
        },
        Update::Assign {
            field: "task_count".into(),
            expr: Expr::U64(0),
        },
        Update::Assign {
            field: "event_subscription_count".into(),
            expr: Expr::U64(0),
        },
        Update::Assign {
            field: "coordinator_bound".into(),
            expr: Expr::Bool(false),
        },
    ]);
    updates
}

fn destroy_mob_projection_updates() -> Vec<Update> {
    let mut updates = reset_mob_projection_updates();
    updates.extend(vec![
        Update::Assign {
            field: "active_identity".into(),
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
            field: "current_generation".into(),
            expr: Expr::None,
        },
        Update::Assign {
            field: "active_member_count".into(),
            expr: Expr::U64(0),
        },
    ]);
    updates
}

fn absorbed_mob_effect_variants() -> Vec<VariantSchema> {
    vec![
        variant("EmitRunLifecycleNotice"),
        variant("EmitFlowRunNotice"),
        variant("EmitStepNotice"),
        variant("AppendFailureLedger"),
        variant("PersistStepOutput"),
        variant("AdmitStepWork"),
        variant("FlowTerminalized"),
        variant("EscalateSupervisor"),
        variant("ProjectTargetSuccess"),
        variant("ProjectTargetFailure"),
        variant("ProjectTargetCanceled"),
        variant("GrantNodeSlot"),
        variant("GrantBodyFrameStart"),
        variant("NotifyCoordinator"),
        variant("ExposePendingSpawn"),
        variant("AdmitKickoffTurn"),
        variant("EmitMemberTerminalNotice"),
        variant("AdmitPeerInput"),
        variant("EmitProgressNote"),
        variant("EmitTaskNotice"),
        variant("ReadyFrontierChanged"),
        variant("StartLoopNode"),
        variant("NodeExecutionReleased"),
        variant("RootFrameCompleted"),
        variant("RootFrameFailed"),
        variant("RootFrameCanceled"),
        variant("BodyFrameCompleted"),
        variant("BodyFrameFailed"),
        variant("BodyFrameCanceled"),
        variant("RequestBodyFrameStart"),
        variant("EvaluateUntilCondition"),
        variant("LoopCompleted"),
        variant("LoopExhausted"),
        variant("LoopFailed"),
        variant("LoopCanceled"),
    ]
}

fn absorbed_mob_effect_dispositions() -> Vec<EffectDispositionRule> {
    vec![
        external_disposition("EmitRunLifecycleNotice"),
        external_disposition("EmitFlowRunNotice"),
        external_disposition("EmitStepNotice"),
        local_disposition("AppendFailureLedger"),
        local_disposition("PersistStepOutput"),
        local_disposition("AdmitStepWork"),
        external_disposition("FlowTerminalized"),
        external_disposition("EscalateSupervisor"),
        external_disposition("ProjectTargetSuccess"),
        external_disposition("ProjectTargetFailure"),
        external_disposition("ProjectTargetCanceled"),
        local_disposition("GrantNodeSlot"),
        local_disposition("GrantBodyFrameStart"),
        external_disposition("NotifyCoordinator"),
        external_disposition("ExposePendingSpawn"),
        local_disposition("AdmitKickoffTurn"),
        external_disposition("EmitMemberTerminalNotice"),
        external_disposition("AdmitPeerInput"),
        external_disposition("EmitProgressNote"),
        external_disposition("EmitTaskNotice"),
        local_disposition("ReadyFrontierChanged"),
        local_disposition("StartLoopNode"),
        local_disposition("NodeExecutionReleased"),
        external_disposition("RootFrameCompleted"),
        external_disposition("RootFrameFailed"),
        external_disposition("RootFrameCanceled"),
        external_disposition("BodyFrameCompleted"),
        external_disposition("BodyFrameFailed"),
        external_disposition("BodyFrameCanceled"),
        local_disposition("RequestBodyFrameStart"),
        local_disposition("EvaluateUntilCondition"),
        external_disposition("LoopCompleted"),
        external_disposition("LoopExhausted"),
        external_disposition("LoopFailed"),
        external_disposition("LoopCanceled"),
    ]
}

fn local_disposition(effect_variant: &str) -> EffectDispositionRule {
    EffectDispositionRule {
        effect_variant: effect_variant.into(),
        disposition: EffectDisposition::Local,
        handoff_protocol: None,
    }
}

fn named_runtime_id() -> TypeRef {
    TypeRef::Named("AgentRuntimeId".into())
}

fn named_work_id() -> TypeRef {
    TypeRef::Named("WorkId".into())
}
