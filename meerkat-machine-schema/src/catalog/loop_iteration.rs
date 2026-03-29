use indexmap::IndexMap;

use crate::{
    EffectDisposition, EffectDispositionRule, EffectEmit, EnumSchema, Expr, FieldInit, FieldSchema,
    Guard, InitSchema, InputMatch, MachineSchema, RustBinding, StateSchema, TransitionSchema,
    TypeRef, Update, VariantSchema,
};

pub fn loop_iteration_machine() -> MachineSchema {
    MachineSchema {
        machine: "LoopIterationMachine".into(),
        version: 1,
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
                field("current_iteration", TypeRef::U32),
                field("max_iterations", TypeRef::U32),
                field(
                    "active_body_frame_id",
                    TypeRef::Option(Box::new(TypeRef::Named("FrameId".into()))),
                ),
                field(
                    "loop_failure_kind",
                    TypeRef::Option(Box::new(TypeRef::Enum("LoopFailureKind".into()))),
                ),
            ],
            init: InitSchema {
                phase: "Absent".into(),
                fields: vec![
                    init("loop_instance_id", Expr::String(String::new())),
                    init("current_iteration", Expr::U64(0)),
                    init("max_iterations", Expr::U64(0)),
                    init("active_body_frame_id", Expr::None),
                    init("loop_failure_kind", Expr::None),
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
                    ],
                },
                VariantSchema {
                    name: "BodyFrameStarted".into(),
                    fields: vec![field("frame_id", TypeRef::Named("FrameId".into()))],
                },
                variant("BodyFrameCompleted"),
                variant("BodyFrameFailed"),
                variant("BodyFrameCanceled"),
                variant("UntilConditionMet"),
                variant("UntilConditionFailed"),
                variant("CancelLoop"),
            ],
        },
        effects: EnumSchema {
            name: "LoopIterationEffect".into(),
            variants: vec![
                VariantSchema {
                    name: "RequestBodyFrameStart".into(),
                    fields: vec![field(
                        "loop_instance_id",
                        TypeRef::Named("LoopInstanceId".into()),
                    )],
                },
                VariantSchema {
                    name: "EvaluateUntilCondition".into(),
                    fields: vec![
                        field("loop_instance_id", TypeRef::Named("LoopInstanceId".into())),
                        field("iteration", TypeRef::U32),
                    ],
                },
                VariantSchema {
                    name: "LoopCompleted".into(),
                    fields: vec![field(
                        "loop_instance_id",
                        TypeRef::Named("LoopInstanceId".into()),
                    )],
                },
                VariantSchema {
                    name: "LoopExhausted".into(),
                    fields: vec![field(
                        "loop_instance_id",
                        TypeRef::Named("LoopInstanceId".into()),
                    )],
                },
                VariantSchema {
                    name: "LoopFailed".into(),
                    fields: vec![field(
                        "loop_instance_id",
                        TypeRef::Named("LoopInstanceId".into()),
                    )],
                },
                VariantSchema {
                    name: "LoopCanceled".into(),
                    fields: vec![field(
                        "loop_instance_id",
                        TypeRef::Named("LoopInstanceId".into()),
                    )],
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
                    bindings: vec!["loop_instance_id".into(), "max_iterations".into()],
                },
                guards: vec![],
                updates: vec![
                    Update::Assign {
                        field: "loop_instance_id".into(),
                        expr: Expr::Binding("loop_instance_id".into()),
                    },
                    Update::Assign {
                        field: "max_iterations".into(),
                        expr: Expr::Binding("max_iterations".into()),
                    },
                    Update::Assign {
                        field: "current_iteration".into(),
                        expr: Expr::U64(0),
                    },
                ],
                to: "Running".into(),
                emit: vec![effect(
                    "RequestBodyFrameStart",
                    vec![("loop_instance_id", Expr::Field("loop_instance_id".into()))],
                )],
            },
            // BodyFrameStarted: Running -> Running, records active frame
            TransitionSchema {
                name: "BodyFrameStarted".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    variant: "BodyFrameStarted".into(),
                    bindings: vec!["frame_id".into()],
                },
                guards: vec![],
                updates: vec![Update::Assign {
                    field: "active_body_frame_id".into(),
                    expr: Expr::Some(Box::new(Expr::Binding("frame_id".into()))),
                }],
                to: "Running".into(),
                emit: vec![],
            },
            // BodyFrameCompleted: Running -> Running, increments iteration, emits EvaluateUntilCondition
            TransitionSchema {
                name: "BodyFrameCompleted".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    variant: "BodyFrameCompleted".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![
                    Update::Assign {
                        field: "active_body_frame_id".into(),
                        expr: Expr::None,
                    },
                    Update::Increment {
                        field: "current_iteration".into(),
                        amount: 1,
                    },
                ],
                to: "Running".into(),
                emit: vec![effect(
                    "EvaluateUntilCondition",
                    vec![
                        ("loop_instance_id", Expr::Field("loop_instance_id".into())),
                        ("iteration", Expr::Field("current_iteration".into())),
                    ],
                )],
            },
            // UntilConditionMet: Running -> Completed, emits LoopCompleted
            TransitionSchema {
                name: "UntilConditionMet".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    variant: "UntilConditionMet".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![],
                to: "Completed".into(),
                emit: vec![effect(
                    "LoopCompleted",
                    vec![("loop_instance_id", Expr::Field("loop_instance_id".into()))],
                )],
            },
            // UntilConditionFailed: Running -> Running (guard: not yet exhausted), re-requests body frame
            TransitionSchema {
                name: "UntilConditionFailed".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    variant: "UntilConditionFailed".into(),
                    bindings: vec![],
                },
                guards: vec![Guard {
                    name: "iterations_not_exhausted".into(),
                    expr: Expr::Lt(
                        Box::new(Expr::Field("current_iteration".into())),
                        Box::new(Expr::Field("max_iterations".into())),
                    ),
                }],
                updates: vec![],
                to: "Running".into(),
                emit: vec![effect(
                    "RequestBodyFrameStart",
                    vec![("loop_instance_id", Expr::Field("loop_instance_id".into()))],
                )],
            },
            // ExhaustedIterations: Running -> Exhausted (guard: current_iteration >= max_iterations)
            // Uses same input variant as UntilConditionFailed but with complementary guard
            TransitionSchema {
                name: "ExhaustedIterations".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    variant: "UntilConditionFailed".into(),
                    bindings: vec![],
                },
                guards: vec![Guard {
                    name: "iterations_exhausted".into(),
                    expr: Expr::Gte(
                        Box::new(Expr::Field("current_iteration".into())),
                        Box::new(Expr::Field("max_iterations".into())),
                    ),
                }],
                updates: vec![],
                to: "Exhausted".into(),
                emit: vec![effect(
                    "LoopExhausted",
                    vec![("loop_instance_id", Expr::Field("loop_instance_id".into()))],
                )],
            },
            // BodyFrameFailed: Running -> Failed, emits LoopFailed
            TransitionSchema {
                name: "BodyFrameFailed".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    variant: "BodyFrameFailed".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![Update::Assign {
                    field: "active_body_frame_id".into(),
                    expr: Expr::None,
                }],
                to: "Failed".into(),
                emit: vec![effect(
                    "LoopFailed",
                    vec![("loop_instance_id", Expr::Field("loop_instance_id".into()))],
                )],
            },
            // BodyFrameCanceled: Running -> Canceled, emits LoopCanceled
            TransitionSchema {
                name: "BodyFrameCanceled".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    variant: "BodyFrameCanceled".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![Update::Assign {
                    field: "active_body_frame_id".into(),
                    expr: Expr::None,
                }],
                to: "Canceled".into(),
                emit: vec![effect(
                    "LoopCanceled",
                    vec![("loop_instance_id", Expr::Field("loop_instance_id".into()))],
                )],
            },
            // CancelLoop: Running -> Canceled, emits LoopCanceled
            TransitionSchema {
                name: "CancelLoop".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    variant: "CancelLoop".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![],
                to: "Canceled".into(),
                emit: vec![effect(
                    "LoopCanceled",
                    vec![("loop_instance_id", Expr::Field("loop_instance_id".into()))],
                )],
            },
        ],
        effect_dispositions: vec![
            disposition("RequestBodyFrameStart", EffectDisposition::External),
            disposition("EvaluateUntilCondition", EffectDisposition::External),
            disposition("LoopCompleted", EffectDisposition::External),
            disposition("LoopExhausted", EffectDisposition::External),
            disposition("LoopFailed", EffectDisposition::External),
            disposition("LoopCanceled", EffectDisposition::External),
        ],
    }
}

fn disposition(name: &str, d: EffectDisposition) -> EffectDispositionRule {
    EffectDispositionRule {
        effect_variant: name.into(),
        disposition: d,
        handoff_protocol: None,
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
