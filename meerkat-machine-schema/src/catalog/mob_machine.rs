use indexmap::IndexMap;

use crate::{
    EffectDisposition, EffectDispositionRule, EffectEmit, EnumSchema, Expr, FieldSchema, Guard,
    InitSchema, InputMatch, InvariantSchema, MachineSchema, RustBinding, StateSchema,
    TransitionSchema, TypeRef, Update, VariantSchema,
};

pub fn mob_machine() -> MachineSchema {
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
                ],
            },
            terminal_phases: vec!["Destroyed".into()],
        },
        inputs: EnumSchema {
            name: "MobMachineInput".into(),
            variants: vec![
                variant("Start"),
                VariantSchema {
                    name: "SpawnMember".into(),
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
            ],
        },
        effects: EnumSchema {
            name: "MobMachineEffect".into(),
            variants: vec![
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
            ],
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
                name: "identity_and_runtime_move_together".into(),
                expr: Expr::Or(vec![
                    Expr::Eq(
                        Box::new(Expr::Field("active_identity".into())),
                        Box::new(Expr::None),
                    ),
                    Expr::Neq(
                        Box::new(Expr::Field("active_runtime_id".into())),
                        Box::new(Expr::None),
                    ),
                ]),
            },
        ],
        transitions: vec![
            TransitionSchema {
                name: "Start".into(),
                from: vec!["Creating".into(), "Stopped".into()],
                on: InputMatch {
                    variant: "Start".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![],
                to: "Running".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "SpawnMember".into(),
                from: vec!["Creating".into(), "Running".into(), "Stopped".into()],
                on: InputMatch {
                    variant: "SpawnMember".into(),
                    bindings: vec![
                        "agent_identity".into(),
                        "agent_runtime_id".into(),
                        "fence_token".into(),
                        "generation".into(),
                    ],
                },
                guards: vec![],
                updates: vec![
                    assign_some("active_identity", "agent_identity"),
                    assign_some("active_runtime_id", "agent_runtime_id"),
                    assign_some("active_fence_token", "fence_token"),
                    assign_some("current_generation", "generation"),
                    Update::Assign {
                        field: "active_member_count".into(),
                        expr: Expr::U64(1),
                    },
                ],
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
                    variant: "ObserveRuntimeRetired".into(),
                    bindings: vec!["agent_runtime_id".into(), "fence_token".into()],
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
                        field: "inflight_work_id".into(),
                        expr: Expr::None,
                    },
                    Update::Assign {
                        field: "active_run_count".into(),
                        expr: Expr::U64(0),
                    },
                ],
                to: "Stopped".into(),
                emit: vec![lifecycle_notice_emit("retired")],
            },
            TransitionSchema {
                name: "ResetMember".into(),
                from: vec!["Running".into(), "Stopped".into()],
                on: InputMatch {
                    variant: "ResetMember".into(),
                    bindings: vec![
                        "agent_identity".into(),
                        "agent_runtime_id".into(),
                        "fence_token".into(),
                        "generation".into(),
                    ],
                },
                guards: vec![],
                updates: vec![
                    assign_some("active_identity", "agent_identity"),
                    assign_some("active_runtime_id", "agent_runtime_id"),
                    assign_some("active_fence_token", "fence_token"),
                    assign_some("current_generation", "generation"),
                    Update::Assign {
                        field: "inflight_work_id".into(),
                        expr: Expr::None,
                    },
                ],
                to: "Running".into(),
                emit: vec![
                    runtime_binding_emit("RequestRuntimeBinding"),
                    lifecycle_notice_emit("reset"),
                ],
            },
            TransitionSchema {
                name: "RespawnMember".into(),
                from: vec!["Running".into(), "Stopped".into()],
                on: InputMatch {
                    variant: "RespawnMember".into(),
                    bindings: vec![
                        "agent_identity".into(),
                        "agent_runtime_id".into(),
                        "fence_token".into(),
                        "generation".into(),
                    ],
                },
                guards: vec![],
                updates: vec![
                    assign_some("active_identity", "agent_identity"),
                    assign_some("active_runtime_id", "agent_runtime_id"),
                    assign_some("active_fence_token", "fence_token"),
                    assign_some("current_generation", "generation"),
                    Update::Assign {
                        field: "inflight_work_id".into(),
                        expr: Expr::None,
                    },
                ],
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
                    variant: "DestroyMob".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![
                    Update::Assign {
                        field: "inflight_work_id".into(),
                        expr: Expr::None,
                    },
                    Update::Assign {
                        field: "active_run_count".into(),
                        expr: Expr::U64(0),
                    },
                ],
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
                    variant: "ObserveRuntimeDestroyed".into(),
                    bindings: vec!["agent_runtime_id".into(), "fence_token".into()],
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
                        field: "active_member_count".into(),
                        expr: Expr::U64(0),
                    },
                    Update::Assign {
                        field: "active_run_count".into(),
                        expr: Expr::U64(0),
                    },
                ],
                to: "Destroyed".into(),
                emit: vec![lifecycle_notice_emit("destroyed")],
            },
        ],
        ci_step_limit: Some(6),
        effect_dispositions: vec![
            routed_disposition("RequestRuntimeBinding", &["MeerkatMachine"]),
            routed_disposition("SubmitMemberWork", &["MeerkatMachine"]),
            routed_disposition("RequestRuntimeRetire", &["MeerkatMachine"]),
            routed_disposition("RequestRuntimeDestroy", &["MeerkatMachine"]),
            external_disposition("EmitMemberLifecycleNotice"),
        ],
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
            (
                "agent_identity".into(),
                Expr::Field("active_identity".into()),
            ),
            (
                "agent_runtime_id".into(),
                Expr::Field("active_runtime_id".into()),
            ),
            (
                "fence_token".into(),
                Expr::Field("active_fence_token".into()),
            ),
            (
                "generation".into(),
                Expr::Field("current_generation".into()),
            ),
        ]),
    }
}

fn runtime_observation_emit(variant: &str) -> EffectEmit {
    EffectEmit {
        variant: variant.into(),
        fields: IndexMap::from([
            (
                "agent_runtime_id".into(),
                Expr::Field("active_runtime_id".into()),
            ),
            (
                "fence_token".into(),
                Expr::Field("active_fence_token".into()),
            ),
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
            (
                "agent_identity".into(),
                Expr::Field("active_identity".into()),
            ),
            ("kind".into(), Expr::String(kind.into())),
        ]),
    }
}

fn clear_work_updates() -> Vec<Update> {
    vec![
        Update::Assign {
            field: "inflight_work_id".into(),
            expr: Expr::None,
        },
        Update::Assign {
            field: "active_run_count".into(),
            expr: Expr::U64(0),
        },
    ]
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
