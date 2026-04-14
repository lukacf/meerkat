use indexmap::IndexMap;

use crate::{
    EffectDisposition, EffectDispositionRule, EffectEmit, EnumSchema, Expr, FieldSchema, Guard,
    InitSchema, InputMatch, InvariantSchema, MachineSchema, RustBinding, StateSchema,
    TransitionSchema, TypeRef, Update, VariantSchema,
};

pub fn meerkat_machine() -> MachineSchema {
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
                field("committed_visibility_revision", TypeRef::U32),
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
                    init("committed_visibility_revision", Expr::U64(0)),
                ],
            },
            terminal_phases: vec!["Destroyed".into()],
        },
        inputs: EnumSchema {
            name: "MeerkatMachineInput".into(),
            variants: vec![
                variant("Initialize"),
                VariantSchema {
                    name: "RegisterSession".into(),
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
                variant("InterruptCurrentRun"),
                variant("CancelAfterBoundary"),
                VariantSchema {
                    name: "PublishCommittedVisibleSet".into(),
                    fields: vec![field("revision", TypeRef::U32)],
                },
                VariantSchema {
                    name: "BoundaryApplied".into(),
                    fields: vec![field("revision", TypeRef::U32)],
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
                variant("RecoverRuntime"),
                variant("RetireRuntime"),
                variant("ResetRuntime"),
                variant("StopRuntimeExecutor"),
                variant("DestroyRuntime"),
            ],
        },
        effects: EnumSchema {
            name: "MeerkatMachineEffect".into(),
            variants: vec![
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
                VariantSchema {
                    name: "CommittedVisibleSetPublished".into(),
                    fields: vec![field("revision", TypeRef::U32)],
                },
                VariantSchema {
                    name: "RuntimeNotice".into(),
                    fields: vec![
                        field("kind", TypeRef::String),
                        field("detail", TypeRef::String),
                    ],
                },
            ],
        },
        helpers: vec![],
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
        ],
        transitions: vec![
            TransitionSchema {
                name: "Initialize".into(),
                from: vec!["Initializing".into()],
                on: InputMatch {
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
                    variant: "RegisterSession".into(),
                    bindings: vec!["session_id".into()],
                },
                guards: vec![],
                updates: vec![assign_some("session_id", "session_id")],
                to: "Idle".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "PrepareBindings".into(),
                from: vec!["Idle".into(), "Stopped".into(), "Retired".into()],
                on: InputMatch {
                    variant: "PrepareBindings".into(),
                    bindings: vec![
                        "agent_runtime_id".into(),
                        "fence_token".into(),
                        "generation".into(),
                    ],
                },
                guards: vec![Guard {
                    name: "session_registered".into(),
                    expr: Expr::Neq(
                        Box::new(Expr::Field("session_id".into())),
                        Box::new(Expr::None),
                    ),
                }],
                updates: vec![
                    assign_some("active_runtime_id", "agent_runtime_id"),
                    assign_some("active_fence_token", "fence_token"),
                    assign_some("active_generation", "generation"),
                ],
                to: "Attached".into(),
                emit: vec![runtime_identity_emit("RuntimeBound")],
            },
            TransitionSchema {
                name: "BeginRunFromIdle".into(),
                from: vec!["Attached".into()],
                on: InputMatch {
                    variant: "SubmitMobWork".into(),
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
                    assign_some("active_work_id", "work_id"),
                    Update::Assign {
                        field: "wake_pending".into(),
                        expr: Expr::Bool(true),
                    },
                ],
                to: "Running".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "AdmissionAcceptedIdleSteer".into(),
                from: vec!["Attached".into()],
                on: InputMatch {
                    variant: "SubmitMobWork".into(),
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
                updates: vec![assign_some("active_work_id", "work_id")],
                to: "Running".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "InterruptCurrentRun".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    variant: "InterruptCurrentRun".into(),
                    bindings: vec![],
                },
                guards: vec![Guard {
                    name: "has_active_work".into(),
                    expr: Expr::Neq(
                        Box::new(Expr::Field("active_work_id".into())),
                        Box::new(Expr::None),
                    ),
                }],
                updates: vec![],
                to: "Running".into(),
                emit: vec![EffectEmit {
                    variant: "RequestCancellationAtBoundary".into(),
                    fields: IndexMap::new(),
                }],
            },
            TransitionSchema {
                name: "CancelAfterBoundary".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    variant: "CancelAfterBoundary".into(),
                    bindings: vec![],
                },
                guards: vec![Guard {
                    name: "has_active_work".into(),
                    expr: Expr::Neq(
                        Box::new(Expr::Field("active_work_id".into())),
                        Box::new(Expr::None),
                    ),
                }],
                updates: vec![],
                to: "Running".into(),
                emit: vec![EffectEmit {
                    variant: "RequestCancellationAtBoundary".into(),
                    fields: IndexMap::new(),
                }],
            },
            TransitionSchema {
                name: "BoundaryApplied".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    variant: "BoundaryApplied".into(),
                    bindings: vec!["revision".into()],
                },
                guards: vec![],
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
                name: "PublishCommittedVisibleSet".into(),
                from: vec!["Attached".into(), "Running".into()],
                on: InputMatch {
                    variant: "PublishCommittedVisibleSet".into(),
                    bindings: vec!["revision".into()],
                },
                guards: vec![],
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
                name: "RunCompleted".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    variant: "RunCompleted".into(),
                    bindings: vec!["work_id".into()],
                },
                guards: vec![Guard {
                    name: "work_matches_active".into(),
                    expr: Expr::Eq(
                        Box::new(Expr::Field("active_work_id".into())),
                        Box::new(Expr::Some(Box::new(Expr::Binding("work_id".into())))),
                    ),
                }],
                updates: vec![Update::Assign {
                    field: "active_work_id".into(),
                    expr: Expr::None,
                }],
                to: "Attached".into(),
                emit: vec![work_identity_emit("WorkCompleted", "work_id")],
            },
            TransitionSchema {
                name: "RunFailed".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    variant: "RunFailed".into(),
                    bindings: vec!["work_id".into()],
                },
                guards: vec![Guard {
                    name: "work_matches_active".into(),
                    expr: Expr::Eq(
                        Box::new(Expr::Field("active_work_id".into())),
                        Box::new(Expr::Some(Box::new(Expr::Binding("work_id".into())))),
                    ),
                }],
                updates: vec![Update::Assign {
                    field: "active_work_id".into(),
                    expr: Expr::None,
                }],
                to: "Attached".into(),
                emit: vec![work_identity_emit("WorkFailed", "work_id")],
            },
            TransitionSchema {
                name: "RunCancelled".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    variant: "RunCancelled".into(),
                    bindings: vec!["work_id".into()],
                },
                guards: vec![Guard {
                    name: "work_matches_active".into(),
                    expr: Expr::Eq(
                        Box::new(Expr::Field("active_work_id".into())),
                        Box::new(Expr::Some(Box::new(Expr::Binding("work_id".into())))),
                    ),
                }],
                updates: vec![Update::Assign {
                    field: "active_work_id".into(),
                    expr: Expr::None,
                }],
                to: "Attached".into(),
                emit: vec![work_identity_emit("WorkCancelled", "work_id")],
            },
            TransitionSchema {
                name: "RecoverRuntime".into(),
                from: vec!["Idle".into(), "Stopped".into(), "Retired".into()],
                on: InputMatch {
                    variant: "RecoverRuntime".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![],
                to: "Recovering".into(),
                emit: vec![notice_emit("recover", "runtime recovering")],
            },
            TransitionSchema {
                name: "RetireRequestedFromIdle".into(),
                from: vec!["Attached".into(), "Running".into()],
                on: InputMatch {
                    variant: "RetireRuntime".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![Update::Assign {
                    field: "active_work_id".into(),
                    expr: Expr::None,
                }],
                to: "Retired".into(),
                emit: vec![runtime_identity_emit("RuntimeRetired")],
            },
            TransitionSchema {
                name: "ResetRuntime".into(),
                from: vec![
                    "Attached".into(),
                    "Retired".into(),
                    "Stopped".into(),
                    "Recovering".into(),
                ],
                on: InputMatch {
                    variant: "ResetRuntime".into(),
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
                ],
                to: "Idle".into(),
                emit: vec![notice_emit("reset", "runtime reset")],
            },
            TransitionSchema {
                name: "StopRuntimeExecutor".into(),
                from: vec!["Attached".into(), "Retired".into(), "Recovering".into()],
                on: InputMatch {
                    variant: "StopRuntimeExecutor".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![Update::Assign {
                    field: "active_work_id".into(),
                    expr: Expr::None,
                }],
                to: "Stopped".into(),
                emit: vec![notice_emit("stop", "runtime executor stopped")],
            },
            TransitionSchema {
                name: "DestroyRuntime".into(),
                from: vec![
                    "Attached".into(),
                    "Running".into(),
                    "Recovering".into(),
                    "Retired".into(),
                    "Stopped".into(),
                ],
                on: InputMatch {
                    variant: "DestroyRuntime".into(),
                    bindings: vec![],
                },
                guards: vec![Guard {
                    name: "runtime_is_bound".into(),
                    expr: Expr::Neq(
                        Box::new(Expr::Field("active_runtime_id".into())),
                        Box::new(Expr::None),
                    ),
                }],
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
                ],
                to: "Destroyed".into(),
                emit: vec![runtime_identity_emit("RuntimeDestroyed")],
            },
        ],
        ci_step_limit: Some(6),
        effect_dispositions: vec![
            routed_disposition("RuntimeBound", &["MobMachine"]),
            routed_disposition("RuntimeRetired", &["MobMachine"]),
            routed_disposition("RuntimeDestroyed", &["MobMachine"]),
            routed_disposition("WorkCompleted", &["MobMachine"]),
            routed_disposition("WorkFailed", &["MobMachine"]),
            routed_disposition("WorkCancelled", &["MobMachine"]),
            local_disposition("RequestCancellationAtBoundary"),
            external_disposition("CommittedVisibleSetPublished"),
            external_disposition("RuntimeNotice"),
        ],
    }
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
            (
                "agent_runtime_id".into(),
                Expr::Field("active_runtime_id".into()),
            ),
            (
                "fence_token".into(),
                Expr::Field("active_fence_token".into()),
            ),
            ("generation".into(), Expr::Field("active_generation".into())),
        ]),
    }
}

fn work_identity_emit(variant: &str, binding: &str) -> EffectEmit {
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
            ("generation".into(), Expr::Field("active_generation".into())),
            (
                "work_id".into(),
                Expr::Some(Box::new(Expr::Binding(binding.into()))),
            ),
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

fn assign_some(field: &str, binding: &str) -> Update {
    Update::Assign {
        field: field.into(),
        expr: Expr::Some(Box::new(Expr::Binding(binding.into()))),
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
