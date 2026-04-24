use indexmap::IndexMap;

use crate::identity::{
    EffectVariantId, EnumTypeId, EnumVariantId, FieldId, InputVariantId, MachineId, NamedTypeId,
    PhaseId, TransitionId,
};
use crate::{
    EffectDisposition, EffectDispositionRule, EffectEmit, EnumSchema, Expr, FieldInit, FieldSchema,
    Guard, InitSchema, MachineSchema, NamedTypeBinding, RustBinding, StateSchema, TransitionSchema,
    TriggerMatch, TypeRef, Update, VariantSchema,
};

pub fn loop_iteration_machine() -> MachineSchema {
    MachineSchema {
        machine: MachineId::parse("LoopIterationMachine").expect("valid machine slug"),
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
                field(
                    "loop_instance_id",
                    TypeRef::Named(
                        NamedTypeId::parse("LoopInstanceId").expect("valid named-type slug"),
                    ),
                ),
                field(
                    "parent_frame_id",
                    TypeRef::Named(NamedTypeId::parse("FrameId").expect("valid named-type slug")),
                ),
                field(
                    "parent_node_id",
                    TypeRef::Named(
                        NamedTypeId::parse("FlowNodeId").expect("valid named-type slug"),
                    ),
                ),
                field(
                    "loop_id",
                    TypeRef::Named(NamedTypeId::parse("LoopId").expect("valid named-type slug")),
                ),
                field("depth", TypeRef::U32),
                field(
                    "stage",
                    TypeRef::Enum(
                        EnumTypeId::parse("LoopIterationStage").expect("valid enum-type slug"),
                    ),
                ),
                field("current_iteration", TypeRef::U32),
                field("last_completed_iteration", TypeRef::U32),
                field("max_iterations", TypeRef::U32),
                field(
                    "active_body_frame_id",
                    TypeRef::Option(Box::new(TypeRef::Named(
                        NamedTypeId::parse("FrameId").expect("valid named-type slug"),
                    ))),
                ),
            ],
            init: InitSchema {
                phase: PhaseId::parse("Absent").expect("valid phase slug"),
                fields: vec![
                    init("loop_instance_id", Expr::String(String::new())),
                    init("parent_frame_id", Expr::String(String::new())),
                    init("parent_node_id", Expr::String(String::new())),
                    init("loop_id", Expr::String(String::new())),
                    init("depth", Expr::U64(0)),
                    init(
                        "stage",
                        Expr::NamedVariant {
                            enum_name: EnumTypeId::parse("LoopIterationStage")
                                .expect("valid enum-type slug"),
                            variant: EnumVariantId::parse("AwaitingBodyFrame")
                                .expect("valid enum-variant slug"),
                        },
                    ),
                    init("current_iteration", Expr::U64(0)),
                    init("last_completed_iteration", Expr::U64(0)),
                    init("max_iterations", Expr::U64(0)),
                    init("active_body_frame_id", Expr::None),
                ],
            },
            terminal_phases: vec![
                PhaseId::parse("Completed").expect("valid phase slug"),
                PhaseId::parse("Exhausted").expect("valid phase slug"),
                PhaseId::parse("Failed").expect("valid phase slug"),
                PhaseId::parse("Canceled").expect("valid phase slug"),
            ],
        },
        inputs: EnumSchema {
            name: "LoopIterationInput".into(),
            variants: vec![
                VariantSchema {
                    name: EnumVariantId::parse("StartLoop").expect("valid variant slug"),
                    fields: vec![
                        field(
                            "loop_instance_id",
                            TypeRef::Named(
                                NamedTypeId::parse("LoopInstanceId")
                                    .expect("valid named-type slug"),
                            ),
                        ),
                        field("max_iterations", TypeRef::U32),
                        field(
                            "parent_frame_id",
                            TypeRef::Named(
                                NamedTypeId::parse("FrameId").expect("valid named-type slug"),
                            ),
                        ),
                        field(
                            "parent_node_id",
                            TypeRef::Named(
                                NamedTypeId::parse("FlowNodeId").expect("valid named-type slug"),
                            ),
                        ),
                        field(
                            "loop_id",
                            TypeRef::Named(
                                NamedTypeId::parse("LoopId").expect("valid named-type slug"),
                            ),
                        ),
                        field("depth", TypeRef::U32),
                    ],
                },
                VariantSchema {
                    name: EnumVariantId::parse("BodyFrameStarted").expect("valid variant slug"),
                    fields: vec![
                        field(
                            "loop_instance_id",
                            TypeRef::Named(
                                NamedTypeId::parse("LoopInstanceId")
                                    .expect("valid named-type slug"),
                            ),
                        ),
                        field(
                            "frame_id",
                            TypeRef::Named(
                                NamedTypeId::parse("FrameId").expect("valid named-type slug"),
                            ),
                        ),
                        field("iteration", TypeRef::U32),
                    ],
                },
                VariantSchema {
                    name: EnumVariantId::parse("BodyFrameCompleted").expect("valid variant slug"),
                    fields: vec![
                        field(
                            "loop_instance_id",
                            TypeRef::Named(
                                NamedTypeId::parse("LoopInstanceId")
                                    .expect("valid named-type slug"),
                            ),
                        ),
                        field("iteration", TypeRef::U32),
                    ],
                },
                VariantSchema {
                    name: EnumVariantId::parse("BodyFrameFailed").expect("valid variant slug"),
                    fields: vec![
                        field(
                            "loop_instance_id",
                            TypeRef::Named(
                                NamedTypeId::parse("LoopInstanceId")
                                    .expect("valid named-type slug"),
                            ),
                        ),
                        field("iteration", TypeRef::U32),
                    ],
                },
                VariantSchema {
                    name: EnumVariantId::parse("BodyFrameCanceled").expect("valid variant slug"),
                    fields: vec![
                        field(
                            "loop_instance_id",
                            TypeRef::Named(
                                NamedTypeId::parse("LoopInstanceId")
                                    .expect("valid named-type slug"),
                            ),
                        ),
                        field("iteration", TypeRef::U32),
                    ],
                },
                VariantSchema {
                    name: EnumVariantId::parse("UntilConditionMet").expect("valid variant slug"),
                    fields: vec![
                        field(
                            "loop_instance_id",
                            TypeRef::Named(
                                NamedTypeId::parse("LoopInstanceId")
                                    .expect("valid named-type slug"),
                            ),
                        ),
                        field("iteration", TypeRef::U32),
                    ],
                },
                VariantSchema {
                    name: EnumVariantId::parse("UntilConditionFailed").expect("valid variant slug"),
                    fields: vec![
                        field(
                            "loop_instance_id",
                            TypeRef::Named(
                                NamedTypeId::parse("LoopInstanceId")
                                    .expect("valid named-type slug"),
                            ),
                        ),
                        field("iteration", TypeRef::U32),
                    ],
                },
                VariantSchema {
                    name: EnumVariantId::parse("CancelLoop").expect("valid variant slug"),
                    fields: vec![field(
                        "loop_instance_id",
                        TypeRef::Named(
                            NamedTypeId::parse("LoopInstanceId").expect("valid named-type slug"),
                        ),
                    )],
                },
            ],
        },
        surface_only_inputs: vec![],
        signals: EnumSchema {
            name: "LoopIterationSignal".into(),
            variants: vec![],
        },
        effects: EnumSchema {
            name: "LoopIterationEffect".into(),
            variants: vec![
                VariantSchema {
                    name: EnumVariantId::parse("RequestBodyFrameStart")
                        .expect("valid variant slug"),
                    fields: vec![
                        field(
                            "loop_instance_id",
                            TypeRef::Named(
                                NamedTypeId::parse("LoopInstanceId")
                                    .expect("valid named-type slug"),
                            ),
                        ),
                        field("depth", TypeRef::U32),
                    ],
                },
                VariantSchema {
                    name: EnumVariantId::parse("EvaluateUntilCondition")
                        .expect("valid variant slug"),
                    fields: vec![
                        field(
                            "loop_instance_id",
                            TypeRef::Named(
                                NamedTypeId::parse("LoopInstanceId")
                                    .expect("valid named-type slug"),
                            ),
                        ),
                        field("iteration", TypeRef::U32),
                        field(
                            "parent_frame_id",
                            TypeRef::Named(
                                NamedTypeId::parse("FrameId").expect("valid named-type slug"),
                            ),
                        ),
                        field(
                            "parent_node_id",
                            TypeRef::Named(
                                NamedTypeId::parse("FlowNodeId").expect("valid named-type slug"),
                            ),
                        ),
                        field(
                            "loop_id",
                            TypeRef::Named(
                                NamedTypeId::parse("LoopId").expect("valid named-type slug"),
                            ),
                        ),
                    ],
                },
                VariantSchema {
                    name: EnumVariantId::parse("LoopCompleted").expect("valid variant slug"),
                    fields: vec![
                        field(
                            "loop_instance_id",
                            TypeRef::Named(
                                NamedTypeId::parse("LoopInstanceId")
                                    .expect("valid named-type slug"),
                            ),
                        ),
                        field(
                            "parent_frame_id",
                            TypeRef::Named(
                                NamedTypeId::parse("FrameId").expect("valid named-type slug"),
                            ),
                        ),
                        field(
                            "parent_node_id",
                            TypeRef::Named(
                                NamedTypeId::parse("FlowNodeId").expect("valid named-type slug"),
                            ),
                        ),
                    ],
                },
                VariantSchema {
                    name: EnumVariantId::parse("LoopExhausted").expect("valid variant slug"),
                    fields: vec![
                        field(
                            "loop_instance_id",
                            TypeRef::Named(
                                NamedTypeId::parse("LoopInstanceId")
                                    .expect("valid named-type slug"),
                            ),
                        ),
                        field(
                            "parent_frame_id",
                            TypeRef::Named(
                                NamedTypeId::parse("FrameId").expect("valid named-type slug"),
                            ),
                        ),
                        field(
                            "parent_node_id",
                            TypeRef::Named(
                                NamedTypeId::parse("FlowNodeId").expect("valid named-type slug"),
                            ),
                        ),
                    ],
                },
                VariantSchema {
                    name: EnumVariantId::parse("LoopFailed").expect("valid variant slug"),
                    fields: vec![
                        field(
                            "loop_instance_id",
                            TypeRef::Named(
                                NamedTypeId::parse("LoopInstanceId")
                                    .expect("valid named-type slug"),
                            ),
                        ),
                        field(
                            "parent_frame_id",
                            TypeRef::Named(
                                NamedTypeId::parse("FrameId").expect("valid named-type slug"),
                            ),
                        ),
                        field(
                            "parent_node_id",
                            TypeRef::Named(
                                NamedTypeId::parse("FlowNodeId").expect("valid named-type slug"),
                            ),
                        ),
                    ],
                },
                VariantSchema {
                    name: EnumVariantId::parse("LoopCanceled").expect("valid variant slug"),
                    fields: vec![
                        field(
                            "loop_instance_id",
                            TypeRef::Named(
                                NamedTypeId::parse("LoopInstanceId")
                                    .expect("valid named-type slug"),
                            ),
                        ),
                        field(
                            "parent_frame_id",
                            TypeRef::Named(
                                NamedTypeId::parse("FrameId").expect("valid named-type slug"),
                            ),
                        ),
                        field(
                            "parent_node_id",
                            TypeRef::Named(
                                NamedTypeId::parse("FlowNodeId").expect("valid named-type slug"),
                            ),
                        ),
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
                name: TransitionId::parse("StartLoop").expect("valid transition slug"),
                from: vec![PhaseId::parse("Absent").expect("valid phase slug")],
                on: TriggerMatch::Input {
                    variant: InputVariantId::parse("StartLoop").expect("valid input-variant slug"),
                    bindings: vec![
                        FieldId::parse("loop_instance_id").expect("valid field slug"),
                        FieldId::parse("max_iterations").expect("valid field slug"),
                        FieldId::parse("parent_frame_id").expect("valid field slug"),
                        FieldId::parse("parent_node_id").expect("valid field slug"),
                        FieldId::parse("loop_id").expect("valid field slug"),
                        FieldId::parse("depth").expect("valid field slug"),
                    ],
                },
                guards: vec![],
                updates: vec![
                    Update::Assign {
                        field: FieldId::parse("loop_instance_id").expect("valid field slug"),
                        expr: Expr::Binding("loop_instance_id".into()),
                    },
                    Update::Assign {
                        field: FieldId::parse("parent_frame_id").expect("valid field slug"),
                        expr: Expr::Binding("parent_frame_id".into()),
                    },
                    Update::Assign {
                        field: FieldId::parse("parent_node_id").expect("valid field slug"),
                        expr: Expr::Binding("parent_node_id".into()),
                    },
                    Update::Assign {
                        field: FieldId::parse("loop_id").expect("valid field slug"),
                        expr: Expr::Binding("loop_id".into()),
                    },
                    Update::Assign {
                        field: FieldId::parse("depth").expect("valid field slug"),
                        expr: Expr::Binding("depth".into()),
                    },
                    Update::Assign {
                        field: FieldId::parse("stage").expect("valid field slug"),
                        expr: Expr::NamedVariant {
                            enum_name: EnumTypeId::parse("LoopIterationStage")
                                .expect("valid enum-type slug"),
                            variant: EnumVariantId::parse("AwaitingBodyFrame")
                                .expect("valid enum-variant slug"),
                        },
                    },
                    Update::Assign {
                        field: FieldId::parse("max_iterations").expect("valid field slug"),
                        expr: Expr::Binding("max_iterations".into()),
                    },
                    Update::Assign {
                        field: FieldId::parse("current_iteration").expect("valid field slug"),
                        expr: Expr::U64(0),
                    },
                    Update::Assign {
                        field: FieldId::parse("last_completed_iteration")
                            .expect("valid field slug"),
                        expr: Expr::U64(0),
                    },
                    Update::Assign {
                        field: FieldId::parse("active_body_frame_id").expect("valid field slug"),
                        expr: Expr::None,
                    },
                ],
                to: PhaseId::parse("Running").expect("valid phase slug"),
                emit: vec![effect(
                    "RequestBodyFrameStart",
                    vec![
                        (
                            "loop_instance_id",
                            Expr::Field(
                                FieldId::parse("loop_instance_id").expect("valid field slug"),
                            ),
                        ),
                        (
                            "depth",
                            Expr::Field(FieldId::parse("depth").expect("valid field slug")),
                        ),
                    ],
                )],
            },
            // BodyFrameStarted: Running -> Running, records active frame
            TransitionSchema {
                name: TransitionId::parse("BodyFrameStarted").expect("valid transition slug"),
                from: vec![PhaseId::parse("Running").expect("valid phase slug")],
                on: TriggerMatch::Input {
                    variant: InputVariantId::parse("BodyFrameStarted")
                        .expect("valid input-variant slug"),
                    bindings: vec![
                        FieldId::parse("loop_instance_id").expect("valid field slug"),
                        FieldId::parse("frame_id").expect("valid field slug"),
                        FieldId::parse("iteration").expect("valid field slug"),
                    ],
                },
                guards: vec![
                    Guard {
                        name: "loop_identity_matches".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field(
                                FieldId::parse("loop_instance_id").expect("valid field slug"),
                            )),
                            Box::new(Expr::Binding("loop_instance_id".into())),
                        ),
                    },
                    Guard {
                        name: "awaiting_body_frame".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field(
                                FieldId::parse("stage").expect("valid field slug"),
                            )),
                            Box::new(Expr::NamedVariant {
                                enum_name: EnumTypeId::parse("LoopIterationStage")
                                    .expect("valid enum-type slug"),
                                variant: EnumVariantId::parse("AwaitingBodyFrame")
                                    .expect("valid enum-variant slug"),
                            }),
                        ),
                    },
                    Guard {
                        name: "iteration_matches_current".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field(
                                FieldId::parse("current_iteration").expect("valid field slug"),
                            )),
                            Box::new(Expr::Binding("iteration".into())),
                        ),
                    },
                ],
                updates: vec![
                    Update::Assign {
                        field: FieldId::parse("active_body_frame_id").expect("valid field slug"),
                        expr: Expr::Some(Box::new(Expr::Binding("frame_id".into()))),
                    },
                    Update::Assign {
                        field: FieldId::parse("stage").expect("valid field slug"),
                        expr: Expr::NamedVariant {
                            enum_name: EnumTypeId::parse("LoopIterationStage")
                                .expect("valid enum-type slug"),
                            variant: EnumVariantId::parse("BodyFrameActive")
                                .expect("valid enum-variant slug"),
                        },
                    },
                ],
                to: PhaseId::parse("Running").expect("valid phase slug"),
                emit: vec![],
            },
            // BodyFrameCompleted: Running -> Running, increments iteration, emits EvaluateUntilCondition
            TransitionSchema {
                name: TransitionId::parse("BodyFrameCompleted").expect("valid transition slug"),
                from: vec![PhaseId::parse("Running").expect("valid phase slug")],
                on: TriggerMatch::Input {
                    variant: InputVariantId::parse("BodyFrameCompleted")
                        .expect("valid input-variant slug"),
                    bindings: vec![
                        FieldId::parse("loop_instance_id").expect("valid field slug"),
                        FieldId::parse("iteration").expect("valid field slug"),
                    ],
                },
                guards: vec![
                    Guard {
                        name: "loop_identity_matches".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field(
                                FieldId::parse("loop_instance_id").expect("valid field slug"),
                            )),
                            Box::new(Expr::Binding("loop_instance_id".into())),
                        ),
                    },
                    Guard {
                        name: "body_frame_active".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field(
                                FieldId::parse("stage").expect("valid field slug"),
                            )),
                            Box::new(Expr::NamedVariant {
                                enum_name: EnumTypeId::parse("LoopIterationStage")
                                    .expect("valid enum-type slug"),
                                variant: EnumVariantId::parse("BodyFrameActive")
                                    .expect("valid enum-variant slug"),
                            }),
                        ),
                    },
                    Guard {
                        name: "iteration_matches_current".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field(
                                FieldId::parse("current_iteration").expect("valid field slug"),
                            )),
                            Box::new(Expr::Binding("iteration".into())),
                        ),
                    },
                ],
                updates: vec![
                    Update::Assign {
                        field: FieldId::parse("active_body_frame_id").expect("valid field slug"),
                        expr: Expr::None,
                    },
                    Update::Assign {
                        field: FieldId::parse("last_completed_iteration")
                            .expect("valid field slug"),
                        expr: Expr::Binding("iteration".into()),
                    },
                    Update::Increment {
                        field: FieldId::parse("current_iteration").expect("valid field slug"),
                        amount: 1,
                    },
                    Update::Assign {
                        field: FieldId::parse("stage").expect("valid field slug"),
                        expr: Expr::NamedVariant {
                            enum_name: EnumTypeId::parse("LoopIterationStage")
                                .expect("valid enum-type slug"),
                            variant: EnumVariantId::parse("AwaitingUntil")
                                .expect("valid enum-variant slug"),
                        },
                    },
                ],
                to: PhaseId::parse("Running").expect("valid phase slug"),
                emit: vec![effect(
                    "EvaluateUntilCondition",
                    vec![
                        (
                            "loop_instance_id",
                            Expr::Field(
                                FieldId::parse("loop_instance_id").expect("valid field slug"),
                            ),
                        ),
                        (
                            "iteration",
                            Expr::Field(
                                FieldId::parse("last_completed_iteration")
                                    .expect("valid field slug"),
                            ),
                        ),
                        (
                            "parent_frame_id",
                            Expr::Field(
                                FieldId::parse("parent_frame_id").expect("valid field slug"),
                            ),
                        ),
                        (
                            "parent_node_id",
                            Expr::Field(
                                FieldId::parse("parent_node_id").expect("valid field slug"),
                            ),
                        ),
                        (
                            "loop_id",
                            Expr::Field(FieldId::parse("loop_id").expect("valid field slug")),
                        ),
                    ],
                )],
            },
            // UntilConditionMet: Running -> Completed, emits LoopCompleted
            TransitionSchema {
                name: TransitionId::parse("UntilConditionMet").expect("valid transition slug"),
                from: vec![PhaseId::parse("Running").expect("valid phase slug")],
                on: TriggerMatch::Input {
                    variant: InputVariantId::parse("UntilConditionMet")
                        .expect("valid input-variant slug"),
                    bindings: vec![
                        FieldId::parse("loop_instance_id").expect("valid field slug"),
                        FieldId::parse("iteration").expect("valid field slug"),
                    ],
                },
                guards: vec![
                    Guard {
                        name: "loop_identity_matches".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field(
                                FieldId::parse("loop_instance_id").expect("valid field slug"),
                            )),
                            Box::new(Expr::Binding("loop_instance_id".into())),
                        ),
                    },
                    Guard {
                        name: "awaiting_until".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field(
                                FieldId::parse("stage").expect("valid field slug"),
                            )),
                            Box::new(Expr::NamedVariant {
                                enum_name: EnumTypeId::parse("LoopIterationStage")
                                    .expect("valid enum-type slug"),
                                variant: EnumVariantId::parse("AwaitingUntil")
                                    .expect("valid enum-variant slug"),
                            }),
                        ),
                    },
                    Guard {
                        name: "iteration_matches_last_completed".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field(
                                FieldId::parse("last_completed_iteration")
                                    .expect("valid field slug"),
                            )),
                            Box::new(Expr::Binding("iteration".into())),
                        ),
                    },
                ],
                updates: vec![],
                to: PhaseId::parse("Completed").expect("valid phase slug"),
                emit: vec![effect(
                    "LoopCompleted",
                    vec![
                        (
                            "loop_instance_id",
                            Expr::Field(
                                FieldId::parse("loop_instance_id").expect("valid field slug"),
                            ),
                        ),
                        (
                            "parent_frame_id",
                            Expr::Field(
                                FieldId::parse("parent_frame_id").expect("valid field slug"),
                            ),
                        ),
                        (
                            "parent_node_id",
                            Expr::Field(
                                FieldId::parse("parent_node_id").expect("valid field slug"),
                            ),
                        ),
                    ],
                )],
            },
            // UntilConditionFailed: Running -> Running (guard: not yet exhausted), re-requests body frame
            TransitionSchema {
                name: TransitionId::parse("UntilConditionFailed").expect("valid transition slug"),
                from: vec![PhaseId::parse("Running").expect("valid phase slug")],
                on: TriggerMatch::Input {
                    variant: InputVariantId::parse("UntilConditionFailed")
                        .expect("valid input-variant slug"),
                    bindings: vec![
                        FieldId::parse("loop_instance_id").expect("valid field slug"),
                        FieldId::parse("iteration").expect("valid field slug"),
                    ],
                },
                guards: vec![
                    Guard {
                        name: "loop_identity_matches".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field(
                                FieldId::parse("loop_instance_id").expect("valid field slug"),
                            )),
                            Box::new(Expr::Binding("loop_instance_id".into())),
                        ),
                    },
                    Guard {
                        name: "awaiting_until".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field(
                                FieldId::parse("stage").expect("valid field slug"),
                            )),
                            Box::new(Expr::NamedVariant {
                                enum_name: EnumTypeId::parse("LoopIterationStage")
                                    .expect("valid enum-type slug"),
                                variant: EnumVariantId::parse("AwaitingUntil")
                                    .expect("valid enum-variant slug"),
                            }),
                        ),
                    },
                    Guard {
                        name: "iteration_matches_last_completed".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field(
                                FieldId::parse("last_completed_iteration")
                                    .expect("valid field slug"),
                            )),
                            Box::new(Expr::Binding("iteration".into())),
                        ),
                    },
                    Guard {
                        name: "iterations_not_exhausted".into(),
                        expr: Expr::Lt(
                            Box::new(Expr::Field(
                                FieldId::parse("current_iteration").expect("valid field slug"),
                            )),
                            Box::new(Expr::Field(
                                FieldId::parse("max_iterations").expect("valid field slug"),
                            )),
                        ),
                    },
                ],
                updates: vec![Update::Assign {
                    field: FieldId::parse("stage").expect("valid field slug"),
                    expr: Expr::NamedVariant {
                        enum_name: EnumTypeId::parse("LoopIterationStage")
                            .expect("valid enum-type slug"),
                        variant: EnumVariantId::parse("AwaitingBodyFrame")
                            .expect("valid enum-variant slug"),
                    },
                }],
                to: PhaseId::parse("Running").expect("valid phase slug"),
                emit: vec![effect(
                    "RequestBodyFrameStart",
                    vec![
                        (
                            "loop_instance_id",
                            Expr::Field(
                                FieldId::parse("loop_instance_id").expect("valid field slug"),
                            ),
                        ),
                        (
                            "depth",
                            Expr::Field(FieldId::parse("depth").expect("valid field slug")),
                        ),
                    ],
                )],
            },
            // ExhaustedIterations: Running -> Exhausted (guard: current_iteration >= max_iterations)
            // Uses same input variant as UntilConditionFailed but with complementary guard
            TransitionSchema {
                name: TransitionId::parse("ExhaustedIterations").expect("valid transition slug"),
                from: vec![PhaseId::parse("Running").expect("valid phase slug")],
                on: TriggerMatch::Input {
                    variant: InputVariantId::parse("UntilConditionFailed")
                        .expect("valid input-variant slug"),
                    bindings: vec![
                        FieldId::parse("loop_instance_id").expect("valid field slug"),
                        FieldId::parse("iteration").expect("valid field slug"),
                    ],
                },
                guards: vec![
                    Guard {
                        name: "loop_identity_matches".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field(
                                FieldId::parse("loop_instance_id").expect("valid field slug"),
                            )),
                            Box::new(Expr::Binding("loop_instance_id".into())),
                        ),
                    },
                    Guard {
                        name: "awaiting_until".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field(
                                FieldId::parse("stage").expect("valid field slug"),
                            )),
                            Box::new(Expr::NamedVariant {
                                enum_name: EnumTypeId::parse("LoopIterationStage")
                                    .expect("valid enum-type slug"),
                                variant: EnumVariantId::parse("AwaitingUntil")
                                    .expect("valid enum-variant slug"),
                            }),
                        ),
                    },
                    Guard {
                        name: "iteration_matches_last_completed".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field(
                                FieldId::parse("last_completed_iteration")
                                    .expect("valid field slug"),
                            )),
                            Box::new(Expr::Binding("iteration".into())),
                        ),
                    },
                    Guard {
                        name: "iterations_exhausted".into(),
                        expr: Expr::Gte(
                            Box::new(Expr::Field(
                                FieldId::parse("current_iteration").expect("valid field slug"),
                            )),
                            Box::new(Expr::Field(
                                FieldId::parse("max_iterations").expect("valid field slug"),
                            )),
                        ),
                    },
                ],
                updates: vec![],
                to: PhaseId::parse("Exhausted").expect("valid phase slug"),
                emit: vec![effect(
                    "LoopExhausted",
                    vec![
                        (
                            "loop_instance_id",
                            Expr::Field(
                                FieldId::parse("loop_instance_id").expect("valid field slug"),
                            ),
                        ),
                        (
                            "parent_frame_id",
                            Expr::Field(
                                FieldId::parse("parent_frame_id").expect("valid field slug"),
                            ),
                        ),
                        (
                            "parent_node_id",
                            Expr::Field(
                                FieldId::parse("parent_node_id").expect("valid field slug"),
                            ),
                        ),
                    ],
                )],
            },
            // BodyFrameFailed: Running -> Failed, emits LoopFailed
            TransitionSchema {
                name: TransitionId::parse("BodyFrameFailed").expect("valid transition slug"),
                from: vec![PhaseId::parse("Running").expect("valid phase slug")],
                on: TriggerMatch::Input {
                    variant: InputVariantId::parse("BodyFrameFailed")
                        .expect("valid input-variant slug"),
                    bindings: vec![
                        FieldId::parse("loop_instance_id").expect("valid field slug"),
                        FieldId::parse("iteration").expect("valid field slug"),
                    ],
                },
                guards: vec![
                    Guard {
                        name: "loop_identity_matches".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field(
                                FieldId::parse("loop_instance_id").expect("valid field slug"),
                            )),
                            Box::new(Expr::Binding("loop_instance_id".into())),
                        ),
                    },
                    Guard {
                        name: "body_frame_active".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field(
                                FieldId::parse("stage").expect("valid field slug"),
                            )),
                            Box::new(Expr::NamedVariant {
                                enum_name: EnumTypeId::parse("LoopIterationStage")
                                    .expect("valid enum-type slug"),
                                variant: EnumVariantId::parse("BodyFrameActive")
                                    .expect("valid enum-variant slug"),
                            }),
                        ),
                    },
                    Guard {
                        name: "iteration_matches_current".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field(
                                FieldId::parse("current_iteration").expect("valid field slug"),
                            )),
                            Box::new(Expr::Binding("iteration".into())),
                        ),
                    },
                ],
                updates: vec![Update::Assign {
                    field: FieldId::parse("active_body_frame_id").expect("valid field slug"),
                    expr: Expr::None,
                }],
                to: PhaseId::parse("Failed").expect("valid phase slug"),
                emit: vec![effect(
                    "LoopFailed",
                    vec![
                        (
                            "loop_instance_id",
                            Expr::Field(
                                FieldId::parse("loop_instance_id").expect("valid field slug"),
                            ),
                        ),
                        (
                            "parent_frame_id",
                            Expr::Field(
                                FieldId::parse("parent_frame_id").expect("valid field slug"),
                            ),
                        ),
                        (
                            "parent_node_id",
                            Expr::Field(
                                FieldId::parse("parent_node_id").expect("valid field slug"),
                            ),
                        ),
                    ],
                )],
            },
            // BodyFrameCanceled: Running -> Canceled, emits LoopCanceled
            TransitionSchema {
                name: TransitionId::parse("BodyFrameCanceled").expect("valid transition slug"),
                from: vec![PhaseId::parse("Running").expect("valid phase slug")],
                on: TriggerMatch::Input {
                    variant: InputVariantId::parse("BodyFrameCanceled")
                        .expect("valid input-variant slug"),
                    bindings: vec![
                        FieldId::parse("loop_instance_id").expect("valid field slug"),
                        FieldId::parse("iteration").expect("valid field slug"),
                    ],
                },
                guards: vec![
                    Guard {
                        name: "loop_identity_matches".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field(
                                FieldId::parse("loop_instance_id").expect("valid field slug"),
                            )),
                            Box::new(Expr::Binding("loop_instance_id".into())),
                        ),
                    },
                    Guard {
                        name: "body_frame_active".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field(
                                FieldId::parse("stage").expect("valid field slug"),
                            )),
                            Box::new(Expr::NamedVariant {
                                enum_name: EnumTypeId::parse("LoopIterationStage")
                                    .expect("valid enum-type slug"),
                                variant: EnumVariantId::parse("BodyFrameActive")
                                    .expect("valid enum-variant slug"),
                            }),
                        ),
                    },
                    Guard {
                        name: "iteration_matches_current".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field(
                                FieldId::parse("current_iteration").expect("valid field slug"),
                            )),
                            Box::new(Expr::Binding("iteration".into())),
                        ),
                    },
                ],
                updates: vec![Update::Assign {
                    field: FieldId::parse("active_body_frame_id").expect("valid field slug"),
                    expr: Expr::None,
                }],
                to: PhaseId::parse("Canceled").expect("valid phase slug"),
                emit: vec![effect(
                    "LoopCanceled",
                    vec![
                        (
                            "loop_instance_id",
                            Expr::Field(
                                FieldId::parse("loop_instance_id").expect("valid field slug"),
                            ),
                        ),
                        (
                            "parent_frame_id",
                            Expr::Field(
                                FieldId::parse("parent_frame_id").expect("valid field slug"),
                            ),
                        ),
                        (
                            "parent_node_id",
                            Expr::Field(
                                FieldId::parse("parent_node_id").expect("valid field slug"),
                            ),
                        ),
                    ],
                )],
            },
            // CancelLoop: Running -> Canceled, emits LoopCanceled
            TransitionSchema {
                name: TransitionId::parse("CancelLoop").expect("valid transition slug"),
                from: vec![PhaseId::parse("Running").expect("valid phase slug")],
                on: TriggerMatch::Input {
                    variant: InputVariantId::parse("CancelLoop").expect("valid input-variant slug"),
                    bindings: vec![FieldId::parse("loop_instance_id").expect("valid field slug")],
                },
                guards: vec![Guard {
                    name: "loop_identity_matches".into(),
                    expr: Expr::Eq(
                        Box::new(Expr::Field(
                            FieldId::parse("loop_instance_id").expect("valid field slug"),
                        )),
                        Box::new(Expr::Binding("loop_instance_id".into())),
                    ),
                }],
                updates: vec![],
                to: PhaseId::parse("Canceled").expect("valid phase slug"),
                emit: vec![effect(
                    "LoopCanceled",
                    vec![
                        (
                            "loop_instance_id",
                            Expr::Field(
                                FieldId::parse("loop_instance_id").expect("valid field slug"),
                            ),
                        ),
                        (
                            "parent_frame_id",
                            Expr::Field(
                                FieldId::parse("parent_frame_id").expect("valid field slug"),
                            ),
                        ),
                        (
                            "parent_node_id",
                            Expr::Field(
                                FieldId::parse("parent_node_id").expect("valid field slug"),
                            ),
                        ),
                    ],
                )],
            },
        ],
        ci_step_limit: None,
        effect_dispositions: vec![
            routed_disposition("RequestBodyFrameStart", &["FlowRunMachine"]),
            disposition("EvaluateUntilCondition", EffectDisposition::Local),
            routed_disposition("LoopCompleted", &["FlowFrameMachine"]),
            routed_disposition("LoopExhausted", &["FlowFrameMachine"]),
            routed_disposition("LoopFailed", &["FlowFrameMachine"]),
            routed_disposition("LoopCanceled", &["FlowFrameMachine"]),
        ],
        named_types: vec![
            NamedTypeBinding::string("LoopId"),
            NamedTypeBinding::string("LoopInstanceId"),
            NamedTypeBinding::string("FrameId"),
            NamedTypeBinding::string("FlowNodeId"),
        ],
    }
}

fn routed_disposition(name: &str, consumer_machines: &[&str]) -> EffectDispositionRule {
    EffectDispositionRule {
        effect_variant: EffectVariantId::parse(name).expect("valid effect-variant slug"),
        disposition: EffectDisposition::Routed {
            consumer_machines: consumer_machines
                .iter()
                .map(|item| MachineId::parse(*item).expect("valid machine slug"))
                .collect(),
        },
        handoff_protocol: None,
    }
}

fn disposition(name: &str, disposition: EffectDisposition) -> EffectDispositionRule {
    EffectDispositionRule {
        effect_variant: EffectVariantId::parse(name).expect("valid effect-variant slug"),
        disposition,
        handoff_protocol: None,
    }
}

fn variant(name: &str) -> VariantSchema {
    VariantSchema {
        name: EnumVariantId::parse(name).expect("valid variant slug"),
        fields: vec![],
    }
}

fn field(name: &str, ty: TypeRef) -> FieldSchema {
    FieldSchema {
        name: FieldId::parse(name).expect("valid field slug"),
        ty,
    }
}

fn init(field: &str, expr: Expr) -> FieldInit {
    FieldInit {
        field: FieldId::parse(field).expect("valid field slug"),
        expr,
    }
}

fn effect(variant: &str, fields: Vec<(&str, Expr)>) -> EffectEmit {
    EffectEmit {
        variant: EffectVariantId::parse(variant).expect("valid effect-variant slug"),
        fields: fields
            .into_iter()
            .map(|(name, expr)| (FieldId::parse(name).expect("valid field slug"), expr))
            .collect::<IndexMap<FieldId, Expr>>(),
    }
}
