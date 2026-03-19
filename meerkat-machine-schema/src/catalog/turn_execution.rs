use indexmap::IndexMap;

use crate::{
    EffectEmit, EnumSchema, Expr, FieldInit, FieldSchema, Guard, InitSchema, InputMatch,
    InvariantSchema, MachineSchema, RustBinding, StateSchema, TransitionSchema, TypeRef, Update,
    VariantSchema,
};

pub fn turn_execution_machine() -> MachineSchema {
    MachineSchema {
        machine: "TurnExecutionMachine".into(),
        version: 1,
        rust: RustBinding {
            crate_name: "meerkat-core".into(),
            module: "generated::turn_execution".into(),
        },
        state: StateSchema {
            phase: EnumSchema {
                name: "TurnExecutionState".into(),
                variants: vec![
                    variant("Ready"),
                    variant("ApplyingPrimitive"),
                    variant("CallingLlm"),
                    variant("WaitingForOps"),
                    variant("DrainingBoundary"),
                    variant("ErrorRecovery"),
                    variant("Cancelling"),
                    variant("Completed"),
                    variant("Failed"),
                    variant("Cancelled"),
                ],
            },
            fields: vec![
                field(
                    "active_run",
                    TypeRef::Option(Box::new(TypeRef::Named("RunId".into()))),
                ),
                field("primitive_kind", TypeRef::Named("TurnPrimitiveKind".into())),
                field(
                    "admitted_content_shape",
                    TypeRef::Option(Box::new(TypeRef::Named("ContentShape".into()))),
                ),
                field("vision_enabled", TypeRef::Bool),
                field("image_tool_results_enabled", TypeRef::Bool),
                field("tool_calls_pending", TypeRef::U32),
                field("boundary_count", TypeRef::U32),
                field("cancel_after_boundary", TypeRef::Bool),
                field(
                    "terminal_outcome",
                    TypeRef::Named("TurnTerminalOutcome".into()),
                ),
            ],
            init: InitSchema {
                phase: "Ready".into(),
                fields: vec![
                    init("active_run", Expr::None),
                    init("primitive_kind", Expr::String("None".into())),
                    init("admitted_content_shape", Expr::None),
                    init("vision_enabled", Expr::Bool(false)),
                    init("image_tool_results_enabled", Expr::Bool(false)),
                    init("tool_calls_pending", Expr::U64(0)),
                    init("boundary_count", Expr::U64(0)),
                    init("cancel_after_boundary", Expr::Bool(false)),
                    init("terminal_outcome", Expr::String("None".into())),
                ],
            },
            terminal_phases: vec!["Completed".into(), "Failed".into(), "Cancelled".into()],
        },
        inputs: EnumSchema {
            name: "TurnExecutionInput".into(),
            variants: vec![
                VariantSchema {
                    name: "StartConversationRun".into(),
                    fields: vec![field("run_id", TypeRef::Named("RunId".into()))],
                },
                VariantSchema {
                    name: "StartImmediateAppend".into(),
                    fields: vec![field("run_id", TypeRef::Named("RunId".into()))],
                },
                VariantSchema {
                    name: "StartImmediateContext".into(),
                    fields: vec![field("run_id", TypeRef::Named("RunId".into()))],
                },
                VariantSchema {
                    name: "PrimitiveApplied".into(),
                    fields: vec![
                        field("run_id", TypeRef::Named("RunId".into())),
                        field(
                            "admitted_content_shape",
                            TypeRef::Named("ContentShape".into()),
                        ),
                        field("vision_enabled", TypeRef::Bool),
                        field("image_tool_results_enabled", TypeRef::Bool),
                    ],
                },
                VariantSchema {
                    name: "LlmReturnedToolCalls".into(),
                    fields: vec![
                        field("run_id", TypeRef::Named("RunId".into())),
                        field("tool_count", TypeRef::U32),
                    ],
                },
                VariantSchema {
                    name: "LlmReturnedTerminal".into(),
                    fields: vec![field("run_id", TypeRef::Named("RunId".into()))],
                },
                VariantSchema {
                    name: "ToolCallsResolved".into(),
                    fields: vec![field("run_id", TypeRef::Named("RunId".into()))],
                },
                VariantSchema {
                    name: "BoundaryContinue".into(),
                    fields: vec![field("run_id", TypeRef::Named("RunId".into()))],
                },
                VariantSchema {
                    name: "BoundaryComplete".into(),
                    fields: vec![field("run_id", TypeRef::Named("RunId".into()))],
                },
                VariantSchema {
                    name: "RecoverableFailure".into(),
                    fields: vec![field("run_id", TypeRef::Named("RunId".into()))],
                },
                VariantSchema {
                    name: "FatalFailure".into(),
                    fields: vec![field("run_id", TypeRef::Named("RunId".into()))],
                },
                VariantSchema {
                    name: "RetryRequested".into(),
                    fields: vec![field("run_id", TypeRef::Named("RunId".into()))],
                },
                VariantSchema {
                    name: "CancelNow".into(),
                    fields: vec![field("run_id", TypeRef::Named("RunId".into()))],
                },
                VariantSchema {
                    name: "CancelAfterBoundary".into(),
                    fields: vec![field("run_id", TypeRef::Named("RunId".into()))],
                },
                VariantSchema {
                    name: "CancellationObserved".into(),
                    fields: vec![field("run_id", TypeRef::Named("RunId".into()))],
                },
                VariantSchema {
                    name: "AcknowledgeTerminal".into(),
                    fields: vec![field("run_id", TypeRef::Named("RunId".into()))],
                },
                VariantSchema {
                    name: "RunCompleted".into(),
                    fields: vec![field("run_id", TypeRef::Named("RunId".into()))],
                },
                VariantSchema {
                    name: "RunFailed".into(),
                    fields: vec![field("run_id", TypeRef::Named("RunId".into()))],
                },
                VariantSchema {
                    name: "RunCancelled".into(),
                    fields: vec![field("run_id", TypeRef::Named("RunId".into()))],
                },
            ],
        },
        effects: EnumSchema {
            name: "TurnExecutionEffect".into(),
            variants: vec![
                VariantSchema {
                    name: "RunStarted".into(),
                    fields: vec![field("run_id", TypeRef::Named("RunId".into()))],
                },
                VariantSchema {
                    name: "BoundaryApplied".into(),
                    fields: vec![
                        field("run_id", TypeRef::Named("RunId".into())),
                        field("boundary_sequence", TypeRef::U64),
                    ],
                },
                VariantSchema {
                    name: "RunCompleted".into(),
                    fields: vec![field("run_id", TypeRef::Named("RunId".into()))],
                },
                VariantSchema {
                    name: "RunFailed".into(),
                    fields: vec![field("run_id", TypeRef::Named("RunId".into()))],
                },
                VariantSchema {
                    name: "RunCancelled".into(),
                    fields: vec![field("run_id", TypeRef::Named("RunId".into()))],
                },
            ],
        },
        helpers: vec![],
        derived: vec![],
        invariants: vec![
            InvariantSchema {
                name: "ready_has_no_active_run".into(),
                expr: Expr::Or(vec![
                    Expr::Neq(
                        Box::new(Expr::CurrentPhase),
                        Box::new(Expr::Phase("Ready".into())),
                    ),
                    Expr::Eq(
                        Box::new(Expr::Field("active_run".into())),
                        Box::new(Expr::None),
                    ),
                ]),
            },
            InvariantSchema {
                name: "ready_has_no_admitted_content".into(),
                expr: Expr::Or(vec![
                    Expr::Neq(
                        Box::new(Expr::CurrentPhase),
                        Box::new(Expr::Phase("Ready".into())),
                    ),
                    Expr::Eq(
                        Box::new(Expr::Field("admitted_content_shape".into())),
                        Box::new(Expr::None),
                    ),
                ]),
            },
            InvariantSchema {
                name: "non_ready_has_active_run".into(),
                expr: Expr::Or(vec![
                    Expr::Eq(
                        Box::new(Expr::CurrentPhase),
                        Box::new(Expr::Phase("Ready".into())),
                    ),
                    Expr::Neq(
                        Box::new(Expr::Field("active_run".into())),
                        Box::new(Expr::None),
                    ),
                ]),
            },
            InvariantSchema {
                name: "waiting_for_ops_implies_pending_tools".into(),
                expr: Expr::Or(vec![
                    Expr::Neq(
                        Box::new(Expr::CurrentPhase),
                        Box::new(Expr::Phase("WaitingForOps".into())),
                    ),
                    Expr::Gt(
                        Box::new(Expr::Field("tool_calls_pending".into())),
                        Box::new(Expr::U64(0)),
                    ),
                ]),
            },
            InvariantSchema {
                name: "ready_has_no_boundary_cancel_request".into(),
                expr: Expr::Or(vec![
                    Expr::Neq(
                        Box::new(Expr::CurrentPhase),
                        Box::new(Expr::Phase("Ready".into())),
                    ),
                    Expr::Eq(
                        Box::new(Expr::Field("cancel_after_boundary".into())),
                        Box::new(Expr::Bool(false)),
                    ),
                ]),
            },
            InvariantSchema {
                name: "immediate_primitives_skip_llm_and_recovery".into(),
                expr: Expr::Or(vec![
                    Expr::Eq(
                        Box::new(Expr::Field("primitive_kind".into())),
                        Box::new(Expr::String("ConversationTurn".into())),
                    ),
                    Expr::And(vec![
                        Expr::Neq(
                            Box::new(Expr::CurrentPhase),
                            Box::new(Expr::Phase("CallingLlm".into())),
                        ),
                        Expr::Neq(
                            Box::new(Expr::CurrentPhase),
                            Box::new(Expr::Phase("WaitingForOps".into())),
                        ),
                        Expr::Neq(
                            Box::new(Expr::CurrentPhase),
                            Box::new(Expr::Phase("ErrorRecovery".into())),
                        ),
                    ]),
                ]),
            },
            InvariantSchema {
                name: "terminal_states_match_terminal_outcome".into(),
                expr: Expr::And(vec![
                    Expr::Or(vec![
                        Expr::Neq(
                            Box::new(Expr::CurrentPhase),
                            Box::new(Expr::Phase("Completed".into())),
                        ),
                        Expr::Eq(
                            Box::new(Expr::Field("terminal_outcome".into())),
                            Box::new(Expr::String("Completed".into())),
                        ),
                    ]),
                    Expr::Or(vec![
                        Expr::Neq(
                            Box::new(Expr::CurrentPhase),
                            Box::new(Expr::Phase("Failed".into())),
                        ),
                        Expr::Eq(
                            Box::new(Expr::Field("terminal_outcome".into())),
                            Box::new(Expr::String("Failed".into())),
                        ),
                    ]),
                    Expr::Or(vec![
                        Expr::Neq(
                            Box::new(Expr::CurrentPhase),
                            Box::new(Expr::Phase("Cancelled".into())),
                        ),
                        Expr::Eq(
                            Box::new(Expr::Field("terminal_outcome".into())),
                            Box::new(Expr::String("Cancelled".into())),
                        ),
                    ]),
                ]),
            },
            InvariantSchema {
                name: "completed_runs_have_seen_a_boundary".into(),
                expr: Expr::Or(vec![
                    Expr::Neq(
                        Box::new(Expr::CurrentPhase),
                        Box::new(Expr::Phase("Completed".into())),
                    ),
                    Expr::Gt(
                        Box::new(Expr::Field("boundary_count".into())),
                        Box::new(Expr::U64(0)),
                    ),
                ]),
            },
        ],
        transitions: vec![
            TransitionSchema {
                name: "StartConversationRun".into(),
                from: vec!["Ready".into()],
                on: InputMatch {
                    variant: "StartConversationRun".into(),
                    bindings: vec!["run_id".into()],
                },
                guards: vec![],
                updates: vec![
                    Update::Assign {
                        field: "active_run".into(),
                        expr: Expr::Some(Box::new(Expr::Binding("run_id".into()))),
                    },
                    Update::Assign {
                        field: "primitive_kind".into(),
                        expr: Expr::String("ConversationTurn".into()),
                    },
                    Update::Assign {
                        field: "tool_calls_pending".into(),
                        expr: Expr::U64(0),
                    },
                    Update::Assign {
                        field: "admitted_content_shape".into(),
                        expr: Expr::None,
                    },
                    Update::Assign {
                        field: "vision_enabled".into(),
                        expr: Expr::Bool(false),
                    },
                    Update::Assign {
                        field: "image_tool_results_enabled".into(),
                        expr: Expr::Bool(false),
                    },
                    Update::Assign {
                        field: "boundary_count".into(),
                        expr: Expr::U64(0),
                    },
                    Update::Assign {
                        field: "cancel_after_boundary".into(),
                        expr: Expr::Bool(false),
                    },
                    Update::Assign {
                        field: "terminal_outcome".into(),
                        expr: Expr::String("None".into()),
                    },
                ],
                to: "ApplyingPrimitive".into(),
                emit: vec![EffectEmit {
                    variant: "RunStarted".into(),
                    fields: IndexMap::from([("run_id".into(), Expr::Binding("run_id".into()))]),
                }],
            },
            TransitionSchema {
                name: "StartImmediateAppend".into(),
                from: vec!["Ready".into()],
                on: InputMatch {
                    variant: "StartImmediateAppend".into(),
                    bindings: vec!["run_id".into()],
                },
                guards: vec![],
                updates: vec![
                    Update::Assign {
                        field: "active_run".into(),
                        expr: Expr::Some(Box::new(Expr::Binding("run_id".into()))),
                    },
                    Update::Assign {
                        field: "primitive_kind".into(),
                        expr: Expr::String("ImmediateAppend".into()),
                    },
                    Update::Assign {
                        field: "tool_calls_pending".into(),
                        expr: Expr::U64(0),
                    },
                    Update::Assign {
                        field: "admitted_content_shape".into(),
                        expr: Expr::None,
                    },
                    Update::Assign {
                        field: "vision_enabled".into(),
                        expr: Expr::Bool(false),
                    },
                    Update::Assign {
                        field: "image_tool_results_enabled".into(),
                        expr: Expr::Bool(false),
                    },
                    Update::Assign {
                        field: "boundary_count".into(),
                        expr: Expr::U64(0),
                    },
                    Update::Assign {
                        field: "cancel_after_boundary".into(),
                        expr: Expr::Bool(false),
                    },
                    Update::Assign {
                        field: "terminal_outcome".into(),
                        expr: Expr::String("None".into()),
                    },
                ],
                to: "ApplyingPrimitive".into(),
                emit: vec![EffectEmit {
                    variant: "RunStarted".into(),
                    fields: IndexMap::from([("run_id".into(), Expr::Binding("run_id".into()))]),
                }],
            },
            TransitionSchema {
                name: "StartImmediateContext".into(),
                from: vec!["Ready".into()],
                on: InputMatch {
                    variant: "StartImmediateContext".into(),
                    bindings: vec!["run_id".into()],
                },
                guards: vec![],
                updates: vec![
                    Update::Assign {
                        field: "active_run".into(),
                        expr: Expr::Some(Box::new(Expr::Binding("run_id".into()))),
                    },
                    Update::Assign {
                        field: "primitive_kind".into(),
                        expr: Expr::String("ImmediateContextAppend".into()),
                    },
                    Update::Assign {
                        field: "tool_calls_pending".into(),
                        expr: Expr::U64(0),
                    },
                    Update::Assign {
                        field: "admitted_content_shape".into(),
                        expr: Expr::None,
                    },
                    Update::Assign {
                        field: "vision_enabled".into(),
                        expr: Expr::Bool(false),
                    },
                    Update::Assign {
                        field: "image_tool_results_enabled".into(),
                        expr: Expr::Bool(false),
                    },
                    Update::Assign {
                        field: "boundary_count".into(),
                        expr: Expr::U64(0),
                    },
                    Update::Assign {
                        field: "cancel_after_boundary".into(),
                        expr: Expr::Bool(false),
                    },
                    Update::Assign {
                        field: "terminal_outcome".into(),
                        expr: Expr::String("None".into()),
                    },
                ],
                to: "ApplyingPrimitive".into(),
                emit: vec![EffectEmit {
                    variant: "RunStarted".into(),
                    fields: IndexMap::from([("run_id".into(), Expr::Binding("run_id".into()))]),
                }],
            },
            TransitionSchema {
                name: "PrimitiveAppliedConversationTurn".into(),
                from: vec!["ApplyingPrimitive".into()],
                on: InputMatch {
                    variant: "PrimitiveApplied".into(),
                    bindings: vec![
                        "run_id".into(),
                        "admitted_content_shape".into(),
                        "vision_enabled".into(),
                        "image_tool_results_enabled".into(),
                    ],
                },
                guards: vec![
                    Guard {
                        name: "run_matches_active".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field("active_run".into())),
                            Box::new(Expr::Some(Box::new(Expr::Binding("run_id".into())))),
                        ),
                    },
                    Guard {
                        name: "conversation_turn".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field("primitive_kind".into())),
                            Box::new(Expr::String("ConversationTurn".into())),
                        ),
                    },
                ],
                updates: vec![
                    Update::Assign {
                        field: "admitted_content_shape".into(),
                        expr: Expr::Some(Box::new(Expr::Binding("admitted_content_shape".into()))),
                    },
                    Update::Assign {
                        field: "vision_enabled".into(),
                        expr: Expr::Binding("vision_enabled".into()),
                    },
                    Update::Assign {
                        field: "image_tool_results_enabled".into(),
                        expr: Expr::Binding("image_tool_results_enabled".into()),
                    },
                ],
                to: "CallingLlm".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "PrimitiveAppliedImmediateAppend".into(),
                from: vec!["ApplyingPrimitive".into()],
                on: InputMatch {
                    variant: "PrimitiveApplied".into(),
                    bindings: vec![
                        "run_id".into(),
                        "admitted_content_shape".into(),
                        "vision_enabled".into(),
                        "image_tool_results_enabled".into(),
                    ],
                },
                guards: vec![
                    Guard {
                        name: "run_matches_active".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field("active_run".into())),
                            Box::new(Expr::Some(Box::new(Expr::Binding("run_id".into())))),
                        ),
                    },
                    Guard {
                        name: "immediate_append".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field("primitive_kind".into())),
                            Box::new(Expr::String("ImmediateAppend".into())),
                        ),
                    },
                    Guard {
                        name: "cancel_after_boundary_not_requested".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field("cancel_after_boundary".into())),
                            Box::new(Expr::Bool(false)),
                        ),
                    },
                ],
                updates: vec![
                    Update::Assign {
                        field: "admitted_content_shape".into(),
                        expr: Expr::Some(Box::new(Expr::Binding("admitted_content_shape".into()))),
                    },
                    Update::Assign {
                        field: "vision_enabled".into(),
                        expr: Expr::Binding("vision_enabled".into()),
                    },
                    Update::Assign {
                        field: "image_tool_results_enabled".into(),
                        expr: Expr::Binding("image_tool_results_enabled".into()),
                    },
                    Update::Increment {
                        field: "boundary_count".into(),
                        amount: 1,
                    },
                    Update::Assign {
                        field: "terminal_outcome".into(),
                        expr: Expr::String("Completed".into()),
                    },
                ],
                to: "Completed".into(),
                emit: vec![
                    EffectEmit {
                        variant: "BoundaryApplied".into(),
                        fields: IndexMap::from([
                            ("run_id".into(), Expr::Binding("run_id".into())),
                            (
                                "boundary_sequence".into(),
                                Expr::Field("boundary_count".into()),
                            ),
                        ]),
                    },
                    EffectEmit {
                        variant: "RunCompleted".into(),
                        fields: IndexMap::from([("run_id".into(), Expr::Binding("run_id".into()))]),
                    },
                ],
            },
            TransitionSchema {
                name: "PrimitiveAppliedImmediateAppendCancelsAfterBoundary".into(),
                from: vec!["ApplyingPrimitive".into()],
                on: InputMatch {
                    variant: "PrimitiveApplied".into(),
                    bindings: vec![
                        "run_id".into(),
                        "admitted_content_shape".into(),
                        "vision_enabled".into(),
                        "image_tool_results_enabled".into(),
                    ],
                },
                guards: vec![
                    Guard {
                        name: "run_matches_active".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field("active_run".into())),
                            Box::new(Expr::Some(Box::new(Expr::Binding("run_id".into())))),
                        ),
                    },
                    Guard {
                        name: "immediate_append".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field("primitive_kind".into())),
                            Box::new(Expr::String("ImmediateAppend".into())),
                        ),
                    },
                    Guard {
                        name: "boundary_cancel_requested".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field("cancel_after_boundary".into())),
                            Box::new(Expr::Bool(true)),
                        ),
                    },
                ],
                updates: vec![
                    Update::Assign {
                        field: "admitted_content_shape".into(),
                        expr: Expr::Some(Box::new(Expr::Binding("admitted_content_shape".into()))),
                    },
                    Update::Assign {
                        field: "vision_enabled".into(),
                        expr: Expr::Binding("vision_enabled".into()),
                    },
                    Update::Assign {
                        field: "image_tool_results_enabled".into(),
                        expr: Expr::Binding("image_tool_results_enabled".into()),
                    },
                    Update::Increment {
                        field: "boundary_count".into(),
                        amount: 1,
                    },
                    Update::Assign {
                        field: "cancel_after_boundary".into(),
                        expr: Expr::Bool(false),
                    },
                    Update::Assign {
                        field: "terminal_outcome".into(),
                        expr: Expr::String("Cancelled".into()),
                    },
                ],
                to: "Cancelled".into(),
                emit: vec![
                    EffectEmit {
                        variant: "BoundaryApplied".into(),
                        fields: IndexMap::from([
                            ("run_id".into(), Expr::Binding("run_id".into())),
                            (
                                "boundary_sequence".into(),
                                Expr::Field("boundary_count".into()),
                            ),
                        ]),
                    },
                    EffectEmit {
                        variant: "RunCancelled".into(),
                        fields: IndexMap::from([("run_id".into(), Expr::Binding("run_id".into()))]),
                    },
                ],
            },
            TransitionSchema {
                name: "PrimitiveAppliedImmediateContext".into(),
                from: vec!["ApplyingPrimitive".into()],
                on: InputMatch {
                    variant: "PrimitiveApplied".into(),
                    bindings: vec![
                        "run_id".into(),
                        "admitted_content_shape".into(),
                        "vision_enabled".into(),
                        "image_tool_results_enabled".into(),
                    ],
                },
                guards: vec![
                    Guard {
                        name: "run_matches_active".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field("active_run".into())),
                            Box::new(Expr::Some(Box::new(Expr::Binding("run_id".into())))),
                        ),
                    },
                    Guard {
                        name: "immediate_context".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field("primitive_kind".into())),
                            Box::new(Expr::String("ImmediateContextAppend".into())),
                        ),
                    },
                    Guard {
                        name: "cancel_after_boundary_not_requested".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field("cancel_after_boundary".into())),
                            Box::new(Expr::Bool(false)),
                        ),
                    },
                ],
                updates: vec![
                    Update::Assign {
                        field: "admitted_content_shape".into(),
                        expr: Expr::Some(Box::new(Expr::Binding("admitted_content_shape".into()))),
                    },
                    Update::Assign {
                        field: "vision_enabled".into(),
                        expr: Expr::Binding("vision_enabled".into()),
                    },
                    Update::Assign {
                        field: "image_tool_results_enabled".into(),
                        expr: Expr::Binding("image_tool_results_enabled".into()),
                    },
                    Update::Increment {
                        field: "boundary_count".into(),
                        amount: 1,
                    },
                    Update::Assign {
                        field: "terminal_outcome".into(),
                        expr: Expr::String("Completed".into()),
                    },
                ],
                to: "Completed".into(),
                emit: vec![
                    EffectEmit {
                        variant: "BoundaryApplied".into(),
                        fields: IndexMap::from([
                            ("run_id".into(), Expr::Binding("run_id".into())),
                            (
                                "boundary_sequence".into(),
                                Expr::Field("boundary_count".into()),
                            ),
                        ]),
                    },
                    EffectEmit {
                        variant: "RunCompleted".into(),
                        fields: IndexMap::from([("run_id".into(), Expr::Binding("run_id".into()))]),
                    },
                ],
            },
            TransitionSchema {
                name: "PrimitiveAppliedImmediateContextCancelsAfterBoundary".into(),
                from: vec!["ApplyingPrimitive".into()],
                on: InputMatch {
                    variant: "PrimitiveApplied".into(),
                    bindings: vec![
                        "run_id".into(),
                        "admitted_content_shape".into(),
                        "vision_enabled".into(),
                        "image_tool_results_enabled".into(),
                    ],
                },
                guards: vec![
                    Guard {
                        name: "run_matches_active".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field("active_run".into())),
                            Box::new(Expr::Some(Box::new(Expr::Binding("run_id".into())))),
                        ),
                    },
                    Guard {
                        name: "immediate_context".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field("primitive_kind".into())),
                            Box::new(Expr::String("ImmediateContextAppend".into())),
                        ),
                    },
                    Guard {
                        name: "boundary_cancel_requested".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field("cancel_after_boundary".into())),
                            Box::new(Expr::Bool(true)),
                        ),
                    },
                ],
                updates: vec![
                    Update::Assign {
                        field: "admitted_content_shape".into(),
                        expr: Expr::Some(Box::new(Expr::Binding("admitted_content_shape".into()))),
                    },
                    Update::Assign {
                        field: "vision_enabled".into(),
                        expr: Expr::Binding("vision_enabled".into()),
                    },
                    Update::Assign {
                        field: "image_tool_results_enabled".into(),
                        expr: Expr::Binding("image_tool_results_enabled".into()),
                    },
                    Update::Increment {
                        field: "boundary_count".into(),
                        amount: 1,
                    },
                    Update::Assign {
                        field: "cancel_after_boundary".into(),
                        expr: Expr::Bool(false),
                    },
                    Update::Assign {
                        field: "terminal_outcome".into(),
                        expr: Expr::String("Cancelled".into()),
                    },
                ],
                to: "Cancelled".into(),
                emit: vec![
                    EffectEmit {
                        variant: "BoundaryApplied".into(),
                        fields: IndexMap::from([
                            ("run_id".into(), Expr::Binding("run_id".into())),
                            (
                                "boundary_sequence".into(),
                                Expr::Field("boundary_count".into()),
                            ),
                        ]),
                    },
                    EffectEmit {
                        variant: "RunCancelled".into(),
                        fields: IndexMap::from([("run_id".into(), Expr::Binding("run_id".into()))]),
                    },
                ],
            },
            TransitionSchema {
                name: "LlmReturnedToolCalls".into(),
                from: vec!["CallingLlm".into()],
                on: InputMatch {
                    variant: "LlmReturnedToolCalls".into(),
                    bindings: vec!["run_id".into(), "tool_count".into()],
                },
                guards: vec![
                    Guard {
                        name: "run_matches_active".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field("active_run".into())),
                            Box::new(Expr::Some(Box::new(Expr::Binding("run_id".into())))),
                        ),
                    },
                    Guard {
                        name: "tool_count_positive".into(),
                        expr: Expr::Gt(
                            Box::new(Expr::Binding("tool_count".into())),
                            Box::new(Expr::U64(0)),
                        ),
                    },
                ],
                updates: vec![Update::Assign {
                    field: "tool_calls_pending".into(),
                    expr: Expr::Binding("tool_count".into()),
                }],
                to: "WaitingForOps".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "ToolCallsResolved".into(),
                from: vec!["WaitingForOps".into()],
                on: InputMatch {
                    variant: "ToolCallsResolved".into(),
                    bindings: vec!["run_id".into()],
                },
                guards: vec![
                    Guard {
                        name: "run_matches_active".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field("active_run".into())),
                            Box::new(Expr::Some(Box::new(Expr::Binding("run_id".into())))),
                        ),
                    },
                    Guard {
                        name: "tool_calls_pending_positive".into(),
                        expr: Expr::Gt(
                            Box::new(Expr::Field("tool_calls_pending".into())),
                            Box::new(Expr::U64(0)),
                        ),
                    },
                ],
                updates: vec![
                    Update::Assign {
                        field: "tool_calls_pending".into(),
                        expr: Expr::U64(0),
                    },
                    Update::Increment {
                        field: "boundary_count".into(),
                        amount: 1,
                    },
                ],
                to: "DrainingBoundary".into(),
                emit: vec![EffectEmit {
                    variant: "BoundaryApplied".into(),
                    fields: IndexMap::from([
                        ("run_id".into(), Expr::Binding("run_id".into())),
                        (
                            "boundary_sequence".into(),
                            Expr::Field("boundary_count".into()),
                        ),
                    ]),
                }],
            },
            TransitionSchema {
                name: "LlmReturnedTerminal".into(),
                from: vec!["CallingLlm".into()],
                on: InputMatch {
                    variant: "LlmReturnedTerminal".into(),
                    bindings: vec!["run_id".into()],
                },
                guards: vec![Guard {
                    name: "run_matches_active".into(),
                    expr: Expr::Eq(
                        Box::new(Expr::Field("active_run".into())),
                        Box::new(Expr::Some(Box::new(Expr::Binding("run_id".into())))),
                    ),
                }],
                updates: vec![Update::Increment {
                    field: "boundary_count".into(),
                    amount: 1,
                }],
                to: "DrainingBoundary".into(),
                emit: vec![EffectEmit {
                    variant: "BoundaryApplied".into(),
                    fields: IndexMap::from([
                        ("run_id".into(), Expr::Binding("run_id".into())),
                        (
                            "boundary_sequence".into(),
                            Expr::Field("boundary_count".into()),
                        ),
                    ]),
                }],
            },
            TransitionSchema {
                name: "BoundaryContinue".into(),
                from: vec!["DrainingBoundary".into()],
                on: InputMatch {
                    variant: "BoundaryContinue".into(),
                    bindings: vec!["run_id".into()],
                },
                guards: vec![
                    Guard {
                        name: "run_matches_active".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field("active_run".into())),
                            Box::new(Expr::Some(Box::new(Expr::Binding("run_id".into())))),
                        ),
                    },
                    Guard {
                        name: "conversation_turn".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field("primitive_kind".into())),
                            Box::new(Expr::String("ConversationTurn".into())),
                        ),
                    },
                    Guard {
                        name: "cancel_after_boundary_not_requested".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field("cancel_after_boundary".into())),
                            Box::new(Expr::Bool(false)),
                        ),
                    },
                ],
                updates: vec![],
                to: "CallingLlm".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "BoundaryContinueCancelsAfterBoundary".into(),
                from: vec!["DrainingBoundary".into()],
                on: InputMatch {
                    variant: "BoundaryContinue".into(),
                    bindings: vec!["run_id".into()],
                },
                guards: vec![
                    Guard {
                        name: "run_matches_active".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field("active_run".into())),
                            Box::new(Expr::Some(Box::new(Expr::Binding("run_id".into())))),
                        ),
                    },
                    Guard {
                        name: "conversation_turn".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field("primitive_kind".into())),
                            Box::new(Expr::String("ConversationTurn".into())),
                        ),
                    },
                    Guard {
                        name: "boundary_cancel_requested".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field("cancel_after_boundary".into())),
                            Box::new(Expr::Bool(true)),
                        ),
                    },
                ],
                updates: vec![
                    Update::Assign {
                        field: "cancel_after_boundary".into(),
                        expr: Expr::Bool(false),
                    },
                    Update::Assign {
                        field: "terminal_outcome".into(),
                        expr: Expr::String("Cancelled".into()),
                    },
                ],
                to: "Cancelled".into(),
                emit: vec![EffectEmit {
                    variant: "RunCancelled".into(),
                    fields: IndexMap::from([("run_id".into(), Expr::Binding("run_id".into()))]),
                }],
            },
            TransitionSchema {
                name: "BoundaryComplete".into(),
                from: vec!["DrainingBoundary".into()],
                on: InputMatch {
                    variant: "BoundaryComplete".into(),
                    bindings: vec!["run_id".into()],
                },
                guards: vec![
                    Guard {
                        name: "run_matches_active".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field("active_run".into())),
                            Box::new(Expr::Some(Box::new(Expr::Binding("run_id".into())))),
                        ),
                    },
                    Guard {
                        name: "cancel_after_boundary_not_requested".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field("cancel_after_boundary".into())),
                            Box::new(Expr::Bool(false)),
                        ),
                    },
                ],
                updates: vec![Update::Assign {
                    field: "terminal_outcome".into(),
                    expr: Expr::String("Completed".into()),
                }],
                to: "Completed".into(),
                emit: vec![EffectEmit {
                    variant: "RunCompleted".into(),
                    fields: IndexMap::from([("run_id".into(), Expr::Binding("run_id".into()))]),
                }],
            },
            TransitionSchema {
                name: "BoundaryCompleteCancelsAfterBoundary".into(),
                from: vec!["DrainingBoundary".into()],
                on: InputMatch {
                    variant: "BoundaryComplete".into(),
                    bindings: vec!["run_id".into()],
                },
                guards: vec![
                    Guard {
                        name: "run_matches_active".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field("active_run".into())),
                            Box::new(Expr::Some(Box::new(Expr::Binding("run_id".into())))),
                        ),
                    },
                    Guard {
                        name: "boundary_cancel_requested".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field("cancel_after_boundary".into())),
                            Box::new(Expr::Bool(true)),
                        ),
                    },
                ],
                updates: vec![
                    Update::Assign {
                        field: "cancel_after_boundary".into(),
                        expr: Expr::Bool(false),
                    },
                    Update::Assign {
                        field: "terminal_outcome".into(),
                        expr: Expr::String("Cancelled".into()),
                    },
                ],
                to: "Cancelled".into(),
                emit: vec![EffectEmit {
                    variant: "RunCancelled".into(),
                    fields: IndexMap::from([("run_id".into(), Expr::Binding("run_id".into()))]),
                }],
            },
            TransitionSchema {
                name: "RecoverableFailureFromCallingLlm".into(),
                from: vec!["CallingLlm".into()],
                on: InputMatch {
                    variant: "RecoverableFailure".into(),
                    bindings: vec!["run_id".into()],
                },
                guards: vec![Guard {
                    name: "run_matches_active".into(),
                    expr: Expr::Eq(
                        Box::new(Expr::Field("active_run".into())),
                        Box::new(Expr::Some(Box::new(Expr::Binding("run_id".into())))),
                    ),
                }],
                updates: vec![],
                to: "ErrorRecovery".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "RecoverableFailureFromWaitingForOps".into(),
                from: vec!["WaitingForOps".into()],
                on: InputMatch {
                    variant: "RecoverableFailure".into(),
                    bindings: vec!["run_id".into()],
                },
                guards: vec![Guard {
                    name: "run_matches_active".into(),
                    expr: Expr::Eq(
                        Box::new(Expr::Field("active_run".into())),
                        Box::new(Expr::Some(Box::new(Expr::Binding("run_id".into())))),
                    ),
                }],
                updates: vec![],
                to: "ErrorRecovery".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "RecoverableFailureFromDrainingBoundary".into(),
                from: vec!["DrainingBoundary".into()],
                on: InputMatch {
                    variant: "RecoverableFailure".into(),
                    bindings: vec!["run_id".into()],
                },
                guards: vec![Guard {
                    name: "run_matches_active".into(),
                    expr: Expr::Eq(
                        Box::new(Expr::Field("active_run".into())),
                        Box::new(Expr::Some(Box::new(Expr::Binding("run_id".into())))),
                    ),
                }],
                updates: vec![],
                to: "ErrorRecovery".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "RetryRequested".into(),
                from: vec!["ErrorRecovery".into()],
                on: InputMatch {
                    variant: "RetryRequested".into(),
                    bindings: vec!["run_id".into()],
                },
                guards: vec![Guard {
                    name: "run_matches_active".into(),
                    expr: Expr::Eq(
                        Box::new(Expr::Field("active_run".into())),
                        Box::new(Expr::Some(Box::new(Expr::Binding("run_id".into())))),
                    ),
                }],
                updates: vec![],
                to: "CallingLlm".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "FatalFailureFromApplyingPrimitive".into(),
                from: vec!["ApplyingPrimitive".into()],
                on: InputMatch {
                    variant: "FatalFailure".into(),
                    bindings: vec!["run_id".into()],
                },
                guards: vec![Guard {
                    name: "run_matches_active".into(),
                    expr: Expr::Eq(
                        Box::new(Expr::Field("active_run".into())),
                        Box::new(Expr::Some(Box::new(Expr::Binding("run_id".into())))),
                    ),
                }],
                updates: vec![Update::Assign {
                    field: "terminal_outcome".into(),
                    expr: Expr::String("Failed".into()),
                }],
                to: "Failed".into(),
                emit: vec![EffectEmit {
                    variant: "RunFailed".into(),
                    fields: IndexMap::from([("run_id".into(), Expr::Binding("run_id".into()))]),
                }],
            },
            TransitionSchema {
                name: "FatalFailureFromCallingLlm".into(),
                from: vec!["CallingLlm".into()],
                on: InputMatch {
                    variant: "FatalFailure".into(),
                    bindings: vec!["run_id".into()],
                },
                guards: vec![Guard {
                    name: "run_matches_active".into(),
                    expr: Expr::Eq(
                        Box::new(Expr::Field("active_run".into())),
                        Box::new(Expr::Some(Box::new(Expr::Binding("run_id".into())))),
                    ),
                }],
                updates: vec![Update::Assign {
                    field: "terminal_outcome".into(),
                    expr: Expr::String("Failed".into()),
                }],
                to: "Failed".into(),
                emit: vec![EffectEmit {
                    variant: "RunFailed".into(),
                    fields: IndexMap::from([("run_id".into(), Expr::Binding("run_id".into()))]),
                }],
            },
            TransitionSchema {
                name: "FatalFailureFromWaitingForOps".into(),
                from: vec!["WaitingForOps".into()],
                on: InputMatch {
                    variant: "FatalFailure".into(),
                    bindings: vec!["run_id".into()],
                },
                guards: vec![Guard {
                    name: "run_matches_active".into(),
                    expr: Expr::Eq(
                        Box::new(Expr::Field("active_run".into())),
                        Box::new(Expr::Some(Box::new(Expr::Binding("run_id".into())))),
                    ),
                }],
                updates: vec![Update::Assign {
                    field: "terminal_outcome".into(),
                    expr: Expr::String("Failed".into()),
                }],
                to: "Failed".into(),
                emit: vec![EffectEmit {
                    variant: "RunFailed".into(),
                    fields: IndexMap::from([("run_id".into(), Expr::Binding("run_id".into()))]),
                }],
            },
            TransitionSchema {
                name: "FatalFailureFromDrainingBoundary".into(),
                from: vec!["DrainingBoundary".into()],
                on: InputMatch {
                    variant: "FatalFailure".into(),
                    bindings: vec!["run_id".into()],
                },
                guards: vec![Guard {
                    name: "run_matches_active".into(),
                    expr: Expr::Eq(
                        Box::new(Expr::Field("active_run".into())),
                        Box::new(Expr::Some(Box::new(Expr::Binding("run_id".into())))),
                    ),
                }],
                updates: vec![Update::Assign {
                    field: "terminal_outcome".into(),
                    expr: Expr::String("Failed".into()),
                }],
                to: "Failed".into(),
                emit: vec![EffectEmit {
                    variant: "RunFailed".into(),
                    fields: IndexMap::from([("run_id".into(), Expr::Binding("run_id".into()))]),
                }],
            },
            TransitionSchema {
                name: "FatalFailureFromErrorRecovery".into(),
                from: vec!["ErrorRecovery".into()],
                on: InputMatch {
                    variant: "FatalFailure".into(),
                    bindings: vec!["run_id".into()],
                },
                guards: vec![Guard {
                    name: "run_matches_active".into(),
                    expr: Expr::Eq(
                        Box::new(Expr::Field("active_run".into())),
                        Box::new(Expr::Some(Box::new(Expr::Binding("run_id".into())))),
                    ),
                }],
                updates: vec![Update::Assign {
                    field: "terminal_outcome".into(),
                    expr: Expr::String("Failed".into()),
                }],
                to: "Failed".into(),
                emit: vec![EffectEmit {
                    variant: "RunFailed".into(),
                    fields: IndexMap::from([("run_id".into(), Expr::Binding("run_id".into()))]),
                }],
            },
            TransitionSchema {
                name: "CancelNowFromApplyingPrimitive".into(),
                from: vec!["ApplyingPrimitive".into()],
                on: InputMatch {
                    variant: "CancelNow".into(),
                    bindings: vec!["run_id".into()],
                },
                guards: vec![Guard {
                    name: "run_matches_active".into(),
                    expr: Expr::Eq(
                        Box::new(Expr::Field("active_run".into())),
                        Box::new(Expr::Some(Box::new(Expr::Binding("run_id".into())))),
                    ),
                }],
                updates: vec![],
                to: "Cancelling".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "CancelNowFromCallingLlm".into(),
                from: vec!["CallingLlm".into()],
                on: InputMatch {
                    variant: "CancelNow".into(),
                    bindings: vec!["run_id".into()],
                },
                guards: vec![Guard {
                    name: "run_matches_active".into(),
                    expr: Expr::Eq(
                        Box::new(Expr::Field("active_run".into())),
                        Box::new(Expr::Some(Box::new(Expr::Binding("run_id".into())))),
                    ),
                }],
                updates: vec![],
                to: "Cancelling".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "CancelNowFromWaitingForOps".into(),
                from: vec!["WaitingForOps".into()],
                on: InputMatch {
                    variant: "CancelNow".into(),
                    bindings: vec!["run_id".into()],
                },
                guards: vec![Guard {
                    name: "run_matches_active".into(),
                    expr: Expr::Eq(
                        Box::new(Expr::Field("active_run".into())),
                        Box::new(Expr::Some(Box::new(Expr::Binding("run_id".into())))),
                    ),
                }],
                updates: vec![],
                to: "Cancelling".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "CancelNowFromDrainingBoundary".into(),
                from: vec!["DrainingBoundary".into()],
                on: InputMatch {
                    variant: "CancelNow".into(),
                    bindings: vec!["run_id".into()],
                },
                guards: vec![Guard {
                    name: "run_matches_active".into(),
                    expr: Expr::Eq(
                        Box::new(Expr::Field("active_run".into())),
                        Box::new(Expr::Some(Box::new(Expr::Binding("run_id".into())))),
                    ),
                }],
                updates: vec![],
                to: "Cancelling".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "CancelNowFromErrorRecovery".into(),
                from: vec!["ErrorRecovery".into()],
                on: InputMatch {
                    variant: "CancelNow".into(),
                    bindings: vec!["run_id".into()],
                },
                guards: vec![Guard {
                    name: "run_matches_active".into(),
                    expr: Expr::Eq(
                        Box::new(Expr::Field("active_run".into())),
                        Box::new(Expr::Some(Box::new(Expr::Binding("run_id".into())))),
                    ),
                }],
                updates: vec![],
                to: "Cancelling".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "CancelAfterBoundaryFromApplyingPrimitive".into(),
                from: vec!["ApplyingPrimitive".into()],
                on: InputMatch {
                    variant: "CancelAfterBoundary".into(),
                    bindings: vec!["run_id".into()],
                },
                guards: vec![Guard {
                    name: "run_matches_active".into(),
                    expr: Expr::Eq(
                        Box::new(Expr::Field("active_run".into())),
                        Box::new(Expr::Some(Box::new(Expr::Binding("run_id".into())))),
                    ),
                }],
                updates: vec![Update::Assign {
                    field: "cancel_after_boundary".into(),
                    expr: Expr::Bool(true),
                }],
                to: "ApplyingPrimitive".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "CancelAfterBoundaryFromCallingLlm".into(),
                from: vec!["CallingLlm".into()],
                on: InputMatch {
                    variant: "CancelAfterBoundary".into(),
                    bindings: vec!["run_id".into()],
                },
                guards: vec![Guard {
                    name: "run_matches_active".into(),
                    expr: Expr::Eq(
                        Box::new(Expr::Field("active_run".into())),
                        Box::new(Expr::Some(Box::new(Expr::Binding("run_id".into())))),
                    ),
                }],
                updates: vec![Update::Assign {
                    field: "cancel_after_boundary".into(),
                    expr: Expr::Bool(true),
                }],
                to: "CallingLlm".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "CancelAfterBoundaryFromWaitingForOps".into(),
                from: vec!["WaitingForOps".into()],
                on: InputMatch {
                    variant: "CancelAfterBoundary".into(),
                    bindings: vec!["run_id".into()],
                },
                guards: vec![Guard {
                    name: "run_matches_active".into(),
                    expr: Expr::Eq(
                        Box::new(Expr::Field("active_run".into())),
                        Box::new(Expr::Some(Box::new(Expr::Binding("run_id".into())))),
                    ),
                }],
                updates: vec![Update::Assign {
                    field: "cancel_after_boundary".into(),
                    expr: Expr::Bool(true),
                }],
                to: "WaitingForOps".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "CancelAfterBoundaryFromDrainingBoundary".into(),
                from: vec!["DrainingBoundary".into()],
                on: InputMatch {
                    variant: "CancelAfterBoundary".into(),
                    bindings: vec!["run_id".into()],
                },
                guards: vec![Guard {
                    name: "run_matches_active".into(),
                    expr: Expr::Eq(
                        Box::new(Expr::Field("active_run".into())),
                        Box::new(Expr::Some(Box::new(Expr::Binding("run_id".into())))),
                    ),
                }],
                updates: vec![Update::Assign {
                    field: "cancel_after_boundary".into(),
                    expr: Expr::Bool(true),
                }],
                to: "DrainingBoundary".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "CancelAfterBoundaryFromErrorRecovery".into(),
                from: vec!["ErrorRecovery".into()],
                on: InputMatch {
                    variant: "CancelAfterBoundary".into(),
                    bindings: vec!["run_id".into()],
                },
                guards: vec![Guard {
                    name: "run_matches_active".into(),
                    expr: Expr::Eq(
                        Box::new(Expr::Field("active_run".into())),
                        Box::new(Expr::Some(Box::new(Expr::Binding("run_id".into())))),
                    ),
                }],
                updates: vec![Update::Assign {
                    field: "cancel_after_boundary".into(),
                    expr: Expr::Bool(true),
                }],
                to: "ErrorRecovery".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "CancellationObserved".into(),
                from: vec!["Cancelling".into()],
                on: InputMatch {
                    variant: "CancellationObserved".into(),
                    bindings: vec!["run_id".into()],
                },
                guards: vec![Guard {
                    name: "run_matches_active".into(),
                    expr: Expr::Eq(
                        Box::new(Expr::Field("active_run".into())),
                        Box::new(Expr::Some(Box::new(Expr::Binding("run_id".into())))),
                    ),
                }],
                updates: vec![
                    Update::Assign {
                        field: "terminal_outcome".into(),
                        expr: Expr::String("Cancelled".into()),
                    },
                    Update::Assign {
                        field: "cancel_after_boundary".into(),
                        expr: Expr::Bool(false),
                    },
                ],
                to: "Cancelled".into(),
                emit: vec![EffectEmit {
                    variant: "RunCancelled".into(),
                    fields: IndexMap::from([("run_id".into(), Expr::Binding("run_id".into()))]),
                }],
            },
            TransitionSchema {
                name: "AcknowledgeTerminalFromCompleted".into(),
                from: vec!["Completed".into()],
                on: InputMatch {
                    variant: "AcknowledgeTerminal".into(),
                    bindings: vec!["run_id".into()],
                },
                guards: vec![Guard {
                    name: "run_matches_active".into(),
                    expr: Expr::Eq(
                        Box::new(Expr::Field("active_run".into())),
                        Box::new(Expr::Some(Box::new(Expr::Binding("run_id".into())))),
                    ),
                }],
                updates: vec![
                    Update::Assign {
                        field: "active_run".into(),
                        expr: Expr::None,
                    },
                    Update::Assign {
                        field: "admitted_content_shape".into(),
                        expr: Expr::None,
                    },
                    Update::Assign {
                        field: "primitive_kind".into(),
                        expr: Expr::String("None".into()),
                    },
                    Update::Assign {
                        field: "vision_enabled".into(),
                        expr: Expr::Bool(false),
                    },
                    Update::Assign {
                        field: "image_tool_results_enabled".into(),
                        expr: Expr::Bool(false),
                    },
                    Update::Assign {
                        field: "tool_calls_pending".into(),
                        expr: Expr::U64(0),
                    },
                    Update::Assign {
                        field: "cancel_after_boundary".into(),
                        expr: Expr::Bool(false),
                    },
                    Update::Assign {
                        field: "boundary_count".into(),
                        expr: Expr::U64(0),
                    },
                    Update::Assign {
                        field: "terminal_outcome".into(),
                        expr: Expr::String("None".into()),
                    },
                ],
                to: "Ready".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "AcknowledgeTerminalFromFailed".into(),
                from: vec!["Failed".into()],
                on: InputMatch {
                    variant: "AcknowledgeTerminal".into(),
                    bindings: vec!["run_id".into()],
                },
                guards: vec![Guard {
                    name: "run_matches_active".into(),
                    expr: Expr::Eq(
                        Box::new(Expr::Field("active_run".into())),
                        Box::new(Expr::Some(Box::new(Expr::Binding("run_id".into())))),
                    ),
                }],
                updates: vec![
                    Update::Assign {
                        field: "active_run".into(),
                        expr: Expr::None,
                    },
                    Update::Assign {
                        field: "admitted_content_shape".into(),
                        expr: Expr::None,
                    },
                    Update::Assign {
                        field: "primitive_kind".into(),
                        expr: Expr::String("None".into()),
                    },
                    Update::Assign {
                        field: "vision_enabled".into(),
                        expr: Expr::Bool(false),
                    },
                    Update::Assign {
                        field: "image_tool_results_enabled".into(),
                        expr: Expr::Bool(false),
                    },
                    Update::Assign {
                        field: "tool_calls_pending".into(),
                        expr: Expr::U64(0),
                    },
                    Update::Assign {
                        field: "cancel_after_boundary".into(),
                        expr: Expr::Bool(false),
                    },
                    Update::Assign {
                        field: "boundary_count".into(),
                        expr: Expr::U64(0),
                    },
                    Update::Assign {
                        field: "terminal_outcome".into(),
                        expr: Expr::String("None".into()),
                    },
                ],
                to: "Ready".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "AcknowledgeTerminalFromCancelled".into(),
                from: vec!["Cancelled".into()],
                on: InputMatch {
                    variant: "AcknowledgeTerminal".into(),
                    bindings: vec!["run_id".into()],
                },
                guards: vec![Guard {
                    name: "run_matches_active".into(),
                    expr: Expr::Eq(
                        Box::new(Expr::Field("active_run".into())),
                        Box::new(Expr::Some(Box::new(Expr::Binding("run_id".into())))),
                    ),
                }],
                updates: vec![
                    Update::Assign {
                        field: "active_run".into(),
                        expr: Expr::None,
                    },
                    Update::Assign {
                        field: "admitted_content_shape".into(),
                        expr: Expr::None,
                    },
                    Update::Assign {
                        field: "primitive_kind".into(),
                        expr: Expr::String("None".into()),
                    },
                    Update::Assign {
                        field: "vision_enabled".into(),
                        expr: Expr::Bool(false),
                    },
                    Update::Assign {
                        field: "image_tool_results_enabled".into(),
                        expr: Expr::Bool(false),
                    },
                    Update::Assign {
                        field: "tool_calls_pending".into(),
                        expr: Expr::U64(0),
                    },
                    Update::Assign {
                        field: "cancel_after_boundary".into(),
                        expr: Expr::Bool(false),
                    },
                    Update::Assign {
                        field: "boundary_count".into(),
                        expr: Expr::U64(0),
                    },
                    Update::Assign {
                        field: "terminal_outcome".into(),
                        expr: Expr::String("None".into()),
                    },
                ],
                to: "Ready".into(),
                emit: vec![],
            },
        ],
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

fn variant(name: &str) -> VariantSchema {
    VariantSchema {
        name: name.into(),
        fields: vec![],
    }
}
