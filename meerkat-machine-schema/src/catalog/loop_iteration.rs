use indexmap::IndexMap;

use crate::{
    EffectDisposition, EffectDispositionRule, EffectEmit, EnumSchema, Expr, FieldInit, FieldSchema,
    Guard, InitSchema, InputMatch, MachineSchema, RustBinding, StateSchema, TransitionSchema,
    TypeRef, Update, VariantSchema,
};

pub fn loop_iteration_machine() -> MachineSchema {
    MachineSchema {
        machine: "LoopIterationMachine".into(),
        version: 2,
        rust: RustBinding {
            crate_name: "meerkat-mob".into(),
            module: "generated::loop_iteration".into(),
        },
        state: StateSchema {
            phase: EnumSchema {
                name: "LoopIterationStatus".into(),
                variants: vec![
                    variant("Absent"),
                    variant("Running"),
                    variant("Completed"),
                    variant("Exhausted"),
                    variant("Failed"),
                    variant("Canceled"),
                ],
            },
            fields: vec![
                // Stored so effects on later transitions can reference the loop's identity
                field("loop_instance_id", TypeRef::Named("LoopInstanceId".into())),
                field("parent_frame_id", TypeRef::Named("FrameId".into())),
                field("parent_node_id", TypeRef::Named("FlowNodeId".into())),
                field("loop_id", TypeRef::Named("LoopId".into())),
                field("depth", TypeRef::U32),
                field("stage", TypeRef::Enum("LoopIterationStage".into())),
                field("current_iteration", TypeRef::U32),
                field("last_completed_iteration", TypeRef::U32),
                field("max_iterations", TypeRef::U32),
                field(
                    "active_body_frame_id",
                    TypeRef::Option(Box::new(TypeRef::Named("FrameId".into()))),
                ),
            ],
            init: InitSchema {
                phase: "Absent".into(),
                fields: vec![
                    init("loop_instance_id", Expr::String(String::new())),
                    init("parent_frame_id", Expr::String(String::new())),
                    init("parent_node_id", Expr::String(String::new())),
                    init("loop_id", Expr::String(String::new())),
                    init("depth", Expr::U64(0)),
                    init(
                        "stage",
                        Expr::NamedVariant {
                            enum_name: "LoopIterationStage".into(),
                            variant: "AwaitingBodyFrame".into(),
                        },
                    ),
                    init("current_iteration", Expr::U64(0)),
                    init("last_completed_iteration", Expr::U64(0)),
                    init("max_iterations", Expr::U64(0)),
                    init("active_body_frame_id", Expr::None),
                ],
            },
            terminal_phases: vec![
                "Completed".into(),
                "Exhausted".into(),
                "Failed".into(),
                "Canceled".into(),
            ],
        },
        inputs: EnumSchema {
            name: "LoopIterationInput".into(),
            variants: vec![
                VariantSchema {
                    name: "StartLoop".into(),
                    fields: vec![
                        field("loop_instance_id", TypeRef::Named("LoopInstanceId".into())),
                        field("max_iterations", TypeRef::U32),
                        field("parent_frame_id", TypeRef::Named("FrameId".into())),
                        field("parent_node_id", TypeRef::Named("FlowNodeId".into())),
                        field("loop_id", TypeRef::Named("LoopId".into())),
                        field("depth", TypeRef::U32),
                    ],
                },
                VariantSchema {
                    name: "BodyFrameStarted".into(),
                    fields: vec![
                        field("loop_instance_id", TypeRef::Named("LoopInstanceId".into())),
                        field("frame_id", TypeRef::Named("FrameId".into())),
                        field("iteration", TypeRef::U32),
                    ],
                },
                VariantSchema {
                    name: "BodyFrameCompleted".into(),
                    fields: vec![
                        field("loop_instance_id", TypeRef::Named("LoopInstanceId".into())),
                        field("iteration", TypeRef::U32),
                    ],
                },
                VariantSchema {
                    name: "BodyFrameFailed".into(),
                    fields: vec![
                        field("loop_instance_id", TypeRef::Named("LoopInstanceId".into())),
                        field("iteration", TypeRef::U32),
                    ],
                },
                VariantSchema {
                    name: "BodyFrameCanceled".into(),
                    fields: vec![
                        field("loop_instance_id", TypeRef::Named("LoopInstanceId".into())),
                        field("iteration", TypeRef::U32),
                    ],
                },
                VariantSchema {
                    name: "UntilConditionMet".into(),
                    fields: vec![
                        field("loop_instance_id", TypeRef::Named("LoopInstanceId".into())),
                        field("iteration", TypeRef::U32),
                    ],
                },
                VariantSchema {
                    name: "UntilConditionFailed".into(),
                    fields: vec![
                        field("loop_instance_id", TypeRef::Named("LoopInstanceId".into())),
                        field("iteration", TypeRef::U32),
                    ],
                },
                VariantSchema {
                    name: "CancelLoop".into(),
                    fields: vec![field(
                        "loop_instance_id",
                        TypeRef::Named("LoopInstanceId".into()),
                    )],
                },
            ],
        },
        effects: EnumSchema {
            name: "LoopIterationEffect".into(),
            variants: vec![
                VariantSchema {
                    name: "RequestBodyFrameStart".into(),
                    fields: vec![
                        field("loop_instance_id", TypeRef::Named("LoopInstanceId".into())),
                        field("depth", TypeRef::U32),
                    ],
                },
                VariantSchema {
                    name: "EvaluateUntilCondition".into(),
                    fields: vec![
                        field("loop_instance_id", TypeRef::Named("LoopInstanceId".into())),
                        field("iteration", TypeRef::U32),
                        field("parent_frame_id", TypeRef::Named("FrameId".into())),
                        field("parent_node_id", TypeRef::Named("FlowNodeId".into())),
                        field("loop_id", TypeRef::Named("LoopId".into())),
                    ],
                },
                VariantSchema {
                    name: "LoopCompleted".into(),
                    fields: vec![
                        field("loop_instance_id", TypeRef::Named("LoopInstanceId".into())),
                        field("parent_frame_id", TypeRef::Named("FrameId".into())),
                        field("parent_node_id", TypeRef::Named("FlowNodeId".into())),
                    ],
                },
                VariantSchema {
                    name: "LoopExhausted".into(),
                    fields: vec![
                        field("loop_instance_id", TypeRef::Named("LoopInstanceId".into())),
                        field("parent_frame_id", TypeRef::Named("FrameId".into())),
                        field("parent_node_id", TypeRef::Named("FlowNodeId".into())),
                    ],
                },
                VariantSchema {
                    name: "LoopFailed".into(),
                    fields: vec![
                        field("loop_instance_id", TypeRef::Named("LoopInstanceId".into())),
                        field("parent_frame_id", TypeRef::Named("FrameId".into())),
                        field("parent_node_id", TypeRef::Named("FlowNodeId".into())),
                    ],
                },
                VariantSchema {
                    name: "LoopCanceled".into(),
                    fields: vec![
                        field("loop_instance_id", TypeRef::Named("LoopInstanceId".into())),
                        field("parent_frame_id", TypeRef::Named("FrameId".into())),
                        field("parent_node_id", TypeRef::Named("FlowNodeId".into())),
                    ],
                },
            ],
        },
        helpers: vec![],
        derived: vec![],
        invariants: vec![],
        transitions: vec![
            // StartLoop: Absent -> Running, stores loop identity, emits RequestBodyFrameStart
            TransitionSchema {
                name: "StartLoop".into(),
                from: vec!["Absent".into()],
                on: InputMatch {
                    variant: "StartLoop".into(),
                    bindings: vec![
                        "loop_instance_id".into(),
                        "max_iterations".into(),
                        "parent_frame_id".into(),
                        "parent_node_id".into(),
                        "loop_id".into(),
                        "depth".into(),
                    ],
                },
                guards: vec![],
                updates: vec![
                    Update::Assign {
                        field: "loop_instance_id".into(),
                        expr: Expr::Binding("loop_instance_id".into()),
                    },
                    Update::Assign {
                        field: "parent_frame_id".into(),
                        expr: Expr::Binding("parent_frame_id".into()),
                    },
                    Update::Assign {
                        field: "parent_node_id".into(),
                        expr: Expr::Binding("parent_node_id".into()),
                    },
                    Update::Assign {
                        field: "loop_id".into(),
                        expr: Expr::Binding("loop_id".into()),
                    },
                    Update::Assign {
                        field: "depth".into(),
                        expr: Expr::Binding("depth".into()),
                    },
                    Update::Assign {
                        field: "stage".into(),
                        expr: Expr::NamedVariant {
                            enum_name: "LoopIterationStage".into(),
                            variant: "AwaitingBodyFrame".into(),
                        },
                    },
                    Update::Assign {
                        field: "max_iterations".into(),
                        expr: Expr::Binding("max_iterations".into()),
                    },
                    Update::Assign {
                        field: "current_iteration".into(),
                        expr: Expr::U64(0),
                    },
                    Update::Assign {
                        field: "last_completed_iteration".into(),
                        expr: Expr::U64(0),
                    },
                    Update::Assign {
                        field: "active_body_frame_id".into(),
                        expr: Expr::None,
                    },
                ],
                to: "Running".into(),
                emit: vec![effect(
                    "RequestBodyFrameStart",
                    vec![
                        ("loop_instance_id", Expr::Field("loop_instance_id".into())),
                        ("depth", Expr::Field("depth".into())),
                    ],
                )],
            },
            // BodyFrameStarted: Running -> Running, records active frame
            TransitionSchema {
                name: "BodyFrameStarted".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    variant: "BodyFrameStarted".into(),
                    bindings: vec![
                        "loop_instance_id".into(),
                        "frame_id".into(),
                        "iteration".into(),
                    ],
                },
                guards: vec![
                    Guard {
                        name: "loop_identity_matches".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field("loop_instance_id".into())),
                            Box::new(Expr::Binding("loop_instance_id".into())),
                        ),
                    },
                    Guard {
                        name: "awaiting_body_frame".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field("stage".into())),
                            Box::new(Expr::NamedVariant {
                                enum_name: "LoopIterationStage".into(),
                                variant: "AwaitingBodyFrame".into(),
                            }),
                        ),
                    },
                    Guard {
                        name: "iteration_matches_current".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field("current_iteration".into())),
                            Box::new(Expr::Binding("iteration".into())),
                        ),
                    },
                ],
                updates: vec![
                    Update::Assign {
                        field: "active_body_frame_id".into(),
                        expr: Expr::Some(Box::new(Expr::Binding("frame_id".into()))),
                    },
                    Update::Assign {
                        field: "stage".into(),
                        expr: Expr::NamedVariant {
                            enum_name: "LoopIterationStage".into(),
                            variant: "BodyFrameActive".into(),
                        },
                    },
                ],
                to: "Running".into(),
                emit: vec![],
            },
            // BodyFrameCompleted: Running -> Running, increments iteration, emits EvaluateUntilCondition
            TransitionSchema {
                name: "BodyFrameCompleted".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    variant: "BodyFrameCompleted".into(),
                    bindings: vec!["loop_instance_id".into(), "iteration".into()],
                },
                guards: vec![
                    Guard {
                        name: "loop_identity_matches".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field("loop_instance_id".into())),
                            Box::new(Expr::Binding("loop_instance_id".into())),
                        ),
                    },
                    Guard {
                        name: "body_frame_active".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field("stage".into())),
                            Box::new(Expr::NamedVariant {
                                enum_name: "LoopIterationStage".into(),
                                variant: "BodyFrameActive".into(),
                            }),
                        ),
                    },
                    Guard {
                        name: "iteration_matches_current".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field("current_iteration".into())),
                            Box::new(Expr::Binding("iteration".into())),
                        ),
                    },
                ],
                updates: vec![
                    Update::Assign {
                        field: "active_body_frame_id".into(),
                        expr: Expr::None,
                    },
                    Update::Assign {
                        field: "last_completed_iteration".into(),
                        expr: Expr::Binding("iteration".into()),
                    },
                    Update::Increment {
                        field: "current_iteration".into(),
                        amount: 1,
                    },
                    Update::Assign {
                        field: "stage".into(),
                        expr: Expr::NamedVariant {
                            enum_name: "LoopIterationStage".into(),
                            variant: "AwaitingUntil".into(),
                        },
                    },
                ],
                to: "Running".into(),
                emit: vec![effect(
                    "EvaluateUntilCondition",
                    vec![
                        ("loop_instance_id", Expr::Field("loop_instance_id".into())),
                        ("iteration", Expr::Field("last_completed_iteration".into())),
                        ("parent_frame_id", Expr::Field("parent_frame_id".into())),
                        ("parent_node_id", Expr::Field("parent_node_id".into())),
                        ("loop_id", Expr::Field("loop_id".into())),
                    ],
                )],
            },
            // UntilConditionMet: Running -> Completed, emits LoopCompleted
            TransitionSchema {
                name: "UntilConditionMet".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    variant: "UntilConditionMet".into(),
                    bindings: vec!["loop_instance_id".into(), "iteration".into()],
                },
                guards: vec![
                    Guard {
                        name: "loop_identity_matches".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field("loop_instance_id".into())),
                            Box::new(Expr::Binding("loop_instance_id".into())),
                        ),
                    },
                    Guard {
                        name: "awaiting_until".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field("stage".into())),
                            Box::new(Expr::NamedVariant {
                                enum_name: "LoopIterationStage".into(),
                                variant: "AwaitingUntil".into(),
                            }),
                        ),
                    },
                    Guard {
                        name: "iteration_matches_last_completed".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field("last_completed_iteration".into())),
                            Box::new(Expr::Binding("iteration".into())),
                        ),
                    },
                ],
                updates: vec![],
                to: "Completed".into(),
                emit: vec![effect(
                    "LoopCompleted",
                    vec![
                        ("loop_instance_id", Expr::Field("loop_instance_id".into())),
                        ("parent_frame_id", Expr::Field("parent_frame_id".into())),
                        ("parent_node_id", Expr::Field("parent_node_id".into())),
                    ],
                )],
            },
            // UntilConditionFailed: Running -> Running (guard: not yet exhausted), re-requests body frame
            TransitionSchema {
                name: "UntilConditionFailed".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    variant: "UntilConditionFailed".into(),
                    bindings: vec!["loop_instance_id".into(), "iteration".into()],
                },
                guards: vec![
                    Guard {
                        name: "loop_identity_matches".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field("loop_instance_id".into())),
                            Box::new(Expr::Binding("loop_instance_id".into())),
                        ),
                    },
                    Guard {
                        name: "awaiting_until".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field("stage".into())),
                            Box::new(Expr::NamedVariant {
                                enum_name: "LoopIterationStage".into(),
                                variant: "AwaitingUntil".into(),
                            }),
                        ),
                    },
                    Guard {
                        name: "iteration_matches_last_completed".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field("last_completed_iteration".into())),
                            Box::new(Expr::Binding("iteration".into())),
                        ),
                    },
                    Guard {
                        name: "iterations_not_exhausted".into(),
                        expr: Expr::Lt(
                            Box::new(Expr::Field("current_iteration".into())),
                            Box::new(Expr::Field("max_iterations".into())),
                        ),
                    },
                ],
                updates: vec![Update::Assign {
                    field: "stage".into(),
                    expr: Expr::NamedVariant {
                        enum_name: "LoopIterationStage".into(),
                        variant: "AwaitingBodyFrame".into(),
                    },
                }],
                to: "Running".into(),
                emit: vec![effect(
                    "RequestBodyFrameStart",
                    vec![
                        ("loop_instance_id", Expr::Field("loop_instance_id".into())),
                        ("depth", Expr::Field("depth".into())),
                    ],
                )],
            },
            // ExhaustedIterations: Running -> Exhausted (guard: current_iteration >= max_iterations)
            // Uses same input variant as UntilConditionFailed but with complementary guard
            TransitionSchema {
                name: "ExhaustedIterations".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    variant: "UntilConditionFailed".into(),
                    bindings: vec!["loop_instance_id".into(), "iteration".into()],
                },
                guards: vec![
                    Guard {
                        name: "loop_identity_matches".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field("loop_instance_id".into())),
                            Box::new(Expr::Binding("loop_instance_id".into())),
                        ),
                    },
                    Guard {
                        name: "awaiting_until".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field("stage".into())),
                            Box::new(Expr::NamedVariant {
                                enum_name: "LoopIterationStage".into(),
                                variant: "AwaitingUntil".into(),
                            }),
                        ),
                    },
                    Guard {
                        name: "iteration_matches_last_completed".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field("last_completed_iteration".into())),
                            Box::new(Expr::Binding("iteration".into())),
                        ),
                    },
                    Guard {
                        name: "iterations_exhausted".into(),
                        expr: Expr::Gte(
                            Box::new(Expr::Field("current_iteration".into())),
                            Box::new(Expr::Field("max_iterations".into())),
                        ),
                    },
                ],
                updates: vec![],
                to: "Exhausted".into(),
                emit: vec![effect(
                    "LoopExhausted",
                    vec![
                        ("loop_instance_id", Expr::Field("loop_instance_id".into())),
                        ("parent_frame_id", Expr::Field("parent_frame_id".into())),
                        ("parent_node_id", Expr::Field("parent_node_id".into())),
                    ],
                )],
            },
            // BodyFrameFailed: Running -> Failed, emits LoopFailed
            TransitionSchema {
                name: "BodyFrameFailed".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    variant: "BodyFrameFailed".into(),
                    bindings: vec!["loop_instance_id".into(), "iteration".into()],
                },
                guards: vec![
                    Guard {
                        name: "loop_identity_matches".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field("loop_instance_id".into())),
                            Box::new(Expr::Binding("loop_instance_id".into())),
                        ),
                    },
                    Guard {
                        name: "body_frame_active".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field("stage".into())),
                            Box::new(Expr::NamedVariant {
                                enum_name: "LoopIterationStage".into(),
                                variant: "BodyFrameActive".into(),
                            }),
                        ),
                    },
                    Guard {
                        name: "iteration_matches_current".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field("current_iteration".into())),
                            Box::new(Expr::Binding("iteration".into())),
                        ),
                    },
                ],
                updates: vec![Update::Assign {
                    field: "active_body_frame_id".into(),
                    expr: Expr::None,
                }],
                to: "Failed".into(),
                emit: vec![effect(
                    "LoopFailed",
                    vec![
                        ("loop_instance_id", Expr::Field("loop_instance_id".into())),
                        ("parent_frame_id", Expr::Field("parent_frame_id".into())),
                        ("parent_node_id", Expr::Field("parent_node_id".into())),
                    ],
                )],
            },
            // BodyFrameCanceled: Running -> Canceled, emits LoopCanceled
            TransitionSchema {
                name: "BodyFrameCanceled".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    variant: "BodyFrameCanceled".into(),
                    bindings: vec!["loop_instance_id".into(), "iteration".into()],
                },
                guards: vec![
                    Guard {
                        name: "loop_identity_matches".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field("loop_instance_id".into())),
                            Box::new(Expr::Binding("loop_instance_id".into())),
                        ),
                    },
                    Guard {
                        name: "body_frame_active".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field("stage".into())),
                            Box::new(Expr::NamedVariant {
                                enum_name: "LoopIterationStage".into(),
                                variant: "BodyFrameActive".into(),
                            }),
                        ),
                    },
                    Guard {
                        name: "iteration_matches_current".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field("current_iteration".into())),
                            Box::new(Expr::Binding("iteration".into())),
                        ),
                    },
                ],
                updates: vec![Update::Assign {
                    field: "active_body_frame_id".into(),
                    expr: Expr::None,
                }],
                to: "Canceled".into(),
                emit: vec![effect(
                    "LoopCanceled",
                    vec![
                        ("loop_instance_id", Expr::Field("loop_instance_id".into())),
                        ("parent_frame_id", Expr::Field("parent_frame_id".into())),
                        ("parent_node_id", Expr::Field("parent_node_id".into())),
                    ],
                )],
            },
            // CancelLoop: Running -> Canceled, emits LoopCanceled
            TransitionSchema {
                name: "CancelLoop".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    variant: "CancelLoop".into(),
                    bindings: vec!["loop_instance_id".into()],
                },
                guards: vec![Guard {
                    name: "loop_identity_matches".into(),
                    expr: Expr::Eq(
                        Box::new(Expr::Field("loop_instance_id".into())),
                        Box::new(Expr::Binding("loop_instance_id".into())),
                    ),
                }],
                updates: vec![],
                to: "Canceled".into(),
                emit: vec![effect(
                    "LoopCanceled",
                    vec![
                        ("loop_instance_id", Expr::Field("loop_instance_id".into())),
                        ("parent_frame_id", Expr::Field("parent_frame_id".into())),
                        ("parent_node_id", Expr::Field("parent_node_id".into())),
                    ],
                )],
            },
        ],
        ci_step_limit: None,
        effect_dispositions: vec![
            routed_disposition("RequestBodyFrameStart", &["FlowRunMachine"]),
            handoff_disposition("EvaluateUntilCondition", "flow_loop_until_evaluation"),
            routed_disposition("LoopCompleted", &["FlowFrameMachine"]),
            routed_disposition("LoopExhausted", &["FlowFrameMachine"]),
            routed_disposition("LoopFailed", &["FlowFrameMachine"]),
            routed_disposition("LoopCanceled", &["FlowFrameMachine"]),
        ],
    }
}

fn routed_disposition(name: &str, consumer_machines: &[&str]) -> EffectDispositionRule {
    EffectDispositionRule {
        effect_variant: name.into(),
        disposition: EffectDisposition::Routed {
            consumer_machines: consumer_machines
                .iter()
                .map(|item| (*item).into())
                .collect(),
        },
        handoff_protocol: None,
    }
}

fn handoff_disposition(name: &str, protocol: &str) -> EffectDispositionRule {
    EffectDispositionRule {
        effect_variant: name.into(),
        disposition: EffectDisposition::External,
        handoff_protocol: Some(protocol.into()),
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

fn init(field: &str, expr: Expr) -> FieldInit {
    FieldInit {
        field: field.into(),
        expr,
    }
}

fn effect(variant: &str, fields: Vec<(&str, Expr)>) -> EffectEmit {
    EffectEmit {
        variant: variant.into(),
        fields: fields
            .into_iter()
            .map(|(name, expr)| (name.into(), expr))
            .collect::<IndexMap<String, Expr>>(),
    }
}
