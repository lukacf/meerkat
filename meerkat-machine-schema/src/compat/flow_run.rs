use indexmap::IndexMap;

use crate::{
    EffectDisposition, EffectDispositionRule, EffectEmit, EnumSchema, Expr, FieldInit, FieldSchema,
    Guard, HelperSchema, InitSchema, InvariantSchema, MachineSchema, NamedTypeBinding, Quantifier,
    RustBinding, StateSchema, TransitionSchema, TypeRef, Update, VariantSchema, TriggerMatch,
};
use crate::identity::{
    EffectVariantId, EnumTypeId, EnumVariantId, FieldId, InputVariantId, MachineId,
    NamedTypeId, PhaseId, ProtocolId, TransitionId,
};

pub fn flow_run_machine() -> MachineSchema {
    MachineSchema {
        machine: MachineId::parse("FlowRunMachine").expect("valid machine slug"),
        version: 5,
        rust: RustBinding {
            crate_name: "meerkat-mob".into(),
            module: "generated::flow_run".into(),
        },
        state: StateSchema {
            phase: EnumSchema {
                name: "FlowRunStatus".into(),
                variants: vec![
                    variant("Absent"),
                    variant("Pending"),
                    variant("Running"),
                    variant("Completed"),
                    variant("Failed"),
                    variant("Canceled"),
                ],
            },
            fields: vec![
                field(
                    "tracked_steps",
                    TypeRef::Set(Box::new(TypeRef::Named(NamedTypeId::parse("StepId").expect("valid named-type slug")))),
                ),
                field(
                    "ordered_steps",
                    TypeRef::Seq(Box::new(TypeRef::Named(NamedTypeId::parse("StepId").expect("valid named-type slug")))),
                ),
                field(
                    "step_status",
                    TypeRef::Map(
                        Box::new(TypeRef::Named(NamedTypeId::parse("StepId").expect("valid named-type slug"))),
                        Box::new(TypeRef::Option(Box::new(TypeRef::Enum(EnumTypeId::parse("StepRunStatus").expect("valid enum-type slug"))))),
                    ),
                ),
                field(
                    "output_recorded",
                    TypeRef::Map(
                        Box::new(TypeRef::Named(NamedTypeId::parse("StepId").expect("valid named-type slug"))),
                        Box::new(TypeRef::Bool),
                    ),
                ),
                field(
                    "step_condition_results",
                    TypeRef::Map(
                        Box::new(TypeRef::Named(NamedTypeId::parse("StepId").expect("valid named-type slug"))),
                        Box::new(TypeRef::Option(Box::new(TypeRef::Bool))),
                    ),
                ),
                field(
                    "step_has_conditions",
                    TypeRef::Map(
                        Box::new(TypeRef::Named(NamedTypeId::parse("StepId").expect("valid named-type slug"))),
                        Box::new(TypeRef::Bool),
                    ),
                ),
                field(
                    "step_dependencies",
                    TypeRef::Map(
                        Box::new(TypeRef::Named(NamedTypeId::parse("StepId").expect("valid named-type slug"))),
                        Box::new(TypeRef::Seq(Box::new(TypeRef::Named(NamedTypeId::parse("StepId").expect("valid named-type slug"))))),
                    ),
                ),
                field(
                    "step_dependency_modes",
                    TypeRef::Map(
                        Box::new(TypeRef::Named(NamedTypeId::parse("StepId").expect("valid named-type slug"))),
                        Box::new(TypeRef::Enum(EnumTypeId::parse("DependencyMode").expect("valid enum-type slug"))),
                    ),
                ),
                field(
                    "step_branches",
                    TypeRef::Map(
                        Box::new(TypeRef::Named(NamedTypeId::parse("StepId").expect("valid named-type slug"))),
                        Box::new(TypeRef::Option(Box::new(TypeRef::Named(NamedTypeId::parse("BranchId").expect("valid named-type slug"))))),
                    ),
                ),
                field(
                    "step_collection_policies",
                    TypeRef::Map(
                        Box::new(TypeRef::Named(NamedTypeId::parse("StepId").expect("valid named-type slug"))),
                        Box::new(TypeRef::Enum(EnumTypeId::parse("CollectionPolicyKind").expect("valid enum-type slug"))),
                    ),
                ),
                field(
                    "step_quorum_thresholds",
                    TypeRef::Map(
                        Box::new(TypeRef::Named(NamedTypeId::parse("StepId").expect("valid named-type slug"))),
                        Box::new(TypeRef::U32),
                    ),
                ),
                field(
                    "step_target_counts",
                    TypeRef::Map(
                        Box::new(TypeRef::Named(NamedTypeId::parse("StepId").expect("valid named-type slug"))),
                        Box::new(TypeRef::U32),
                    ),
                ),
                field(
                    "step_target_success_counts",
                    TypeRef::Map(
                        Box::new(TypeRef::Named(NamedTypeId::parse("StepId").expect("valid named-type slug"))),
                        Box::new(TypeRef::U32),
                    ),
                ),
                field(
                    "step_target_terminal_failure_counts",
                    TypeRef::Map(
                        Box::new(TypeRef::Named(NamedTypeId::parse("StepId").expect("valid named-type slug"))),
                        Box::new(TypeRef::U32),
                    ),
                ),
                field(
                    "target_retry_counts",
                    TypeRef::Map(Box::new(TypeRef::String), Box::new(TypeRef::U32)),
                ),
                field("failure_count", TypeRef::U32),
                field("consecutive_failure_count", TypeRef::U32),
                field("escalation_threshold", TypeRef::U32),
                field("max_step_retries", TypeRef::U32),
                // v2: frame/loop registries and slot schedulers
                field(
                    "ready_frames",
                    TypeRef::Seq(Box::new(TypeRef::Named(NamedTypeId::parse("FrameId").expect("valid named-type slug")))),
                ),
                field(
                    "ready_frame_membership",
                    TypeRef::Set(Box::new(TypeRef::Named(NamedTypeId::parse("FrameId").expect("valid named-type slug")))),
                ),
                field(
                    "pending_body_frame_loops",
                    TypeRef::Seq(Box::new(TypeRef::Named(NamedTypeId::parse("LoopInstanceId").expect("valid named-type slug")))),
                ),
                field(
                    "pending_body_frame_loop_membership",
                    TypeRef::Set(Box::new(TypeRef::Named(NamedTypeId::parse("LoopInstanceId").expect("valid named-type slug")))),
                ),
                field("active_node_count", TypeRef::U32),
                field("active_frame_count", TypeRef::U32),
                field("max_active_nodes", TypeRef::U32),
                field("max_active_frames", TypeRef::U32),
                field("max_frame_depth", TypeRef::U32),
                // v2: transient scratch fields — valid only within a single PumpNodeScheduler
                // or PumpFrameScheduler transition. They capture the queue head BEFORE
                // SeqPopFront runs, because effects are evaluated against post-update state
                // and the head is gone by then. Do NOT read these fields between transitions;
                // their values are stale until the next pump assigns them.
                field("last_granted_frame", TypeRef::Named(NamedTypeId::parse("FrameId").expect("valid named-type slug"))),
                field("last_granted_loop", TypeRef::Named(NamedTypeId::parse("LoopInstanceId").expect("valid named-type slug"))),
            ],
            init: InitSchema {
            phase: PhaseId::parse("Absent").expect("valid phase slug"),
                fields: vec![
                    init("tracked_steps", Expr::EmptySet),
                    init("ordered_steps", Expr::SeqLiteral(vec![])),
                    init("step_status", Expr::EmptyMap),
                    init("output_recorded", Expr::EmptyMap),
                    init("step_condition_results", Expr::EmptyMap),
                    init("step_has_conditions", Expr::EmptyMap),
                    init("step_dependencies", Expr::EmptyMap),
                    init("step_dependency_modes", Expr::EmptyMap),
                    init("step_branches", Expr::EmptyMap),
                    init("step_collection_policies", Expr::EmptyMap),
                    init("step_quorum_thresholds", Expr::EmptyMap),
                    init("step_target_counts", Expr::EmptyMap),
                    init("step_target_success_counts", Expr::EmptyMap),
                    init("step_target_terminal_failure_counts", Expr::EmptyMap),
                    init("target_retry_counts", Expr::EmptyMap),
                    init("failure_count", Expr::U64(0)),
                    init("consecutive_failure_count", Expr::U64(0)),
                    init("escalation_threshold", Expr::U64(0)),
                    init("max_step_retries", Expr::U64(0)),
                    // v2 field inits
                    init("ready_frames", Expr::SeqLiteral(vec![])),
                    init("ready_frame_membership", Expr::EmptySet),
                    init("pending_body_frame_loops", Expr::SeqLiteral(vec![])),
                    init("pending_body_frame_loop_membership", Expr::EmptySet),
                    init("active_node_count", Expr::U64(0)),
                    init("active_frame_count", Expr::U64(0)),
                    init("max_active_nodes", Expr::U64(0)),
                    init("max_active_frames", Expr::U64(0)),
                    init("max_frame_depth", Expr::U64(0)),
                    // v2: scratch fields for head capture
                    init("last_granted_frame", Expr::String(String::new())),
                    init("last_granted_loop", Expr::String(String::new())),
                ],
            },
            terminal_phases: vec![PhaseId::parse("Completed").expect("valid phase slug"), PhaseId::parse("Failed").expect("valid phase slug"), PhaseId::parse("Canceled").expect("valid phase slug")],
        },
        inputs: EnumSchema {
            name: "FlowRunInput".into(),
            variants: vec![
                VariantSchema {
                name: EnumVariantId::parse("CreateRun").expect("valid variant slug"),
                    fields: vec![
                        field(
                            "step_ids",
                            TypeRef::Seq(Box::new(TypeRef::Named(NamedTypeId::parse("StepId").expect("valid named-type slug")))),
                        ),
                        field(
                            "ordered_steps",
                            TypeRef::Seq(Box::new(TypeRef::Named(NamedTypeId::parse("StepId").expect("valid named-type slug")))),
                        ),
                        field(
                            "step_has_conditions",
                            TypeRef::Map(
                                Box::new(TypeRef::Named(NamedTypeId::parse("StepId").expect("valid named-type slug"))),
                                Box::new(TypeRef::Bool),
                            ),
                        ),
                        field(
                            "step_dependencies",
                            TypeRef::Map(
                                Box::new(TypeRef::Named(NamedTypeId::parse("StepId").expect("valid named-type slug"))),
                                Box::new(TypeRef::Seq(Box::new(TypeRef::Named(NamedTypeId::parse("StepId").expect("valid named-type slug"))))),
                            ),
                        ),
                        field(
                            "step_dependency_modes",
                            TypeRef::Map(
                                Box::new(TypeRef::Named(NamedTypeId::parse("StepId").expect("valid named-type slug"))),
                                Box::new(TypeRef::Enum(EnumTypeId::parse("DependencyMode").expect("valid enum-type slug"))),
                            ),
                        ),
                        field(
                            "step_branches",
                            TypeRef::Map(
                                Box::new(TypeRef::Named(NamedTypeId::parse("StepId").expect("valid named-type slug"))),
                                Box::new(TypeRef::Option(Box::new(TypeRef::Named(NamedTypeId::parse("BranchId").expect("valid named-type slug"))))),
                            ),
                        ),
                        field(
                            "step_collection_policies",
                            TypeRef::Map(
                                Box::new(TypeRef::Named(NamedTypeId::parse("StepId").expect("valid named-type slug"))),
                                Box::new(TypeRef::Enum(EnumTypeId::parse("CollectionPolicyKind").expect("valid enum-type slug"))),
                            ),
                        ),
                        field(
                            "step_quorum_thresholds",
                            TypeRef::Map(
                                Box::new(TypeRef::Named(NamedTypeId::parse("StepId").expect("valid named-type slug"))),
                                Box::new(TypeRef::U32),
                            ),
                        ),
                        field("escalation_threshold", TypeRef::U32),
                        field("max_step_retries", TypeRef::U32),
                        // v2 scheduler limits
                        field("max_active_nodes", TypeRef::U32),
                        field("max_active_frames", TypeRef::U32),
                        field("max_frame_depth", TypeRef::U32),
                    ],
                },
                variant("StartRun"),
                VariantSchema {
                name: EnumVariantId::parse("DispatchStep").expect("valid variant slug"),
                    fields: vec![field("step_id", TypeRef::Named(NamedTypeId::parse("StepId").expect("valid named-type slug")))],
                },
                VariantSchema {
                name: EnumVariantId::parse("CompleteStep").expect("valid variant slug"),
                    fields: vec![field("step_id", TypeRef::Named(NamedTypeId::parse("StepId").expect("valid named-type slug")))],
                },
                VariantSchema {
                name: EnumVariantId::parse("RecordStepOutput").expect("valid variant slug"),
                    fields: vec![field("step_id", TypeRef::Named(NamedTypeId::parse("StepId").expect("valid named-type slug")))],
                },
                VariantSchema {
                name: EnumVariantId::parse("ConditionPassed").expect("valid variant slug"),
                    fields: vec![field("step_id", TypeRef::Named(NamedTypeId::parse("StepId").expect("valid named-type slug")))],
                },
                VariantSchema {
                name: EnumVariantId::parse("ConditionRejected").expect("valid variant slug"),
                    fields: vec![field("step_id", TypeRef::Named(NamedTypeId::parse("StepId").expect("valid named-type slug")))],
                },
                VariantSchema {
                name: EnumVariantId::parse("FailStep").expect("valid variant slug"),
                    fields: vec![field("step_id", TypeRef::Named(NamedTypeId::parse("StepId").expect("valid named-type slug")))],
                },
                VariantSchema {
                name: EnumVariantId::parse("SkipStep").expect("valid variant slug"),
                    fields: vec![field("step_id", TypeRef::Named(NamedTypeId::parse("StepId").expect("valid named-type slug")))],
                },
                VariantSchema {
                name: EnumVariantId::parse("ProjectFrameStepStatus").expect("valid variant slug"),
                    fields: vec![
                        field("step_id", TypeRef::Named(NamedTypeId::parse("StepId").expect("valid named-type slug"))),
                        field("step_status", TypeRef::Enum(EnumTypeId::parse("StepRunStatus").expect("valid enum-type slug"))),
                        field("append_failure_ledger", TypeRef::Bool),
                    ],
                },
                VariantSchema {
                name: EnumVariantId::parse("CancelStep").expect("valid variant slug"),
                    fields: vec![field("step_id", TypeRef::Named(NamedTypeId::parse("StepId").expect("valid named-type slug")))],
                },
                VariantSchema {
                name: EnumVariantId::parse("RegisterTargets").expect("valid variant slug"),
                    fields: vec![
                        field("step_id", TypeRef::Named(NamedTypeId::parse("StepId").expect("valid named-type slug"))),
                        field("target_count", TypeRef::U32),
                    ],
                },
                VariantSchema {
                name: EnumVariantId::parse("RecordTargetSuccess").expect("valid variant slug"),
                    fields: vec![
                        field("step_id", TypeRef::Named(NamedTypeId::parse("StepId").expect("valid named-type slug"))),
                        field("target_id", TypeRef::Named(NamedTypeId::parse("MeerkatId").expect("valid named-type slug"))),
                    ],
                },
                VariantSchema {
                name: EnumVariantId::parse("RecordTargetTerminalFailure").expect("valid variant slug"),
                    fields: vec![field("step_id", TypeRef::Named(NamedTypeId::parse("StepId").expect("valid named-type slug")))],
                },
                VariantSchema {
                name: EnumVariantId::parse("RecordTargetCanceled").expect("valid variant slug"),
                    fields: vec![
                        field("step_id", TypeRef::Named(NamedTypeId::parse("StepId").expect("valid named-type slug"))),
                        field("target_id", TypeRef::Named(NamedTypeId::parse("MeerkatId").expect("valid named-type slug"))),
                    ],
                },
                VariantSchema {
                name: EnumVariantId::parse("RecordTargetFailure").expect("valid variant slug"),
                    fields: vec![
                        field("step_id", TypeRef::Named(NamedTypeId::parse("StepId").expect("valid named-type slug"))),
                        field("target_id", TypeRef::Named(NamedTypeId::parse("MeerkatId").expect("valid named-type slug"))),
                        field("retry_key", TypeRef::String),
                    ],
                },
                VariantSchema {
                name: EnumVariantId::parse("RegisterReadyFrame").expect("valid variant slug"),
                    fields: vec![field("frame_id", TypeRef::Named(NamedTypeId::parse("FrameId").expect("valid named-type slug")))],
                },
                variant("PumpNodeScheduler"),
                VariantSchema {
                name: EnumVariantId::parse("RegisterPendingBodyFrame").expect("valid variant slug"),
                    fields: vec![
                        field("loop_instance_id", TypeRef::Named(NamedTypeId::parse("LoopInstanceId").expect("valid named-type slug"))),
                        field("depth", TypeRef::U32),
                    ],
                },
                variant("PumpFrameScheduler"),
                VariantSchema {
                name: EnumVariantId::parse("NodeExecutionReleased").expect("valid variant slug"),
                    fields: vec![field("frame_id", TypeRef::Named(NamedTypeId::parse("FrameId").expect("valid named-type slug")))],
                },
                VariantSchema {
                name: EnumVariantId::parse("FrameTerminated").expect("valid variant slug"),
                    fields: vec![field("frame_id", TypeRef::Named(NamedTypeId::parse("FrameId").expect("valid named-type slug")))],
                },
                variant("TerminalizeCompleted"),
                variant("TerminalizeFailed"),
                variant("TerminalizeCanceled"),
            ],
        },
        surface_only_inputs: vec![],
        signals: EnumSchema {
            name: "FlowRunSignal".into(),
            variants: vec![],
        },
        effects: EnumSchema {
            name: "FlowRunEffect".into(),
            variants: vec![
                VariantSchema {
                name: EnumVariantId::parse("EmitFlowRunNotice").expect("valid variant slug"),
                    fields: vec![field("run_status", TypeRef::Enum(EnumTypeId::parse("FlowRunStatus").expect("valid enum-type slug")))],
                },
                VariantSchema {
                name: EnumVariantId::parse("EmitStepNotice").expect("valid variant slug"),
                    fields: vec![
                        field("step_id", TypeRef::Named(NamedTypeId::parse("StepId").expect("valid named-type slug"))),
                        field("step_status", TypeRef::Enum(EnumTypeId::parse("StepRunStatus").expect("valid enum-type slug"))),
                    ],
                },
                VariantSchema {
                name: EnumVariantId::parse("AppendFailureLedger").expect("valid variant slug"),
                    fields: vec![field("step_id", TypeRef::Named(NamedTypeId::parse("StepId").expect("valid named-type slug")))],
                },
                VariantSchema {
                name: EnumVariantId::parse("PersistStepOutput").expect("valid variant slug"),
                    fields: vec![field("step_id", TypeRef::Named(NamedTypeId::parse("StepId").expect("valid named-type slug")))],
                },
                VariantSchema {
                name: EnumVariantId::parse("AdmitStepWork").expect("valid variant slug"),
                    fields: vec![field("step_id", TypeRef::Named(NamedTypeId::parse("StepId").expect("valid named-type slug")))],
                },
                VariantSchema {
                name: EnumVariantId::parse("FlowTerminalized").expect("valid variant slug"),
                    fields: vec![field("run_status", TypeRef::Enum(EnumTypeId::parse("FlowRunStatus").expect("valid enum-type slug")))],
                },
                VariantSchema {
                name: EnumVariantId::parse("EscalateSupervisor").expect("valid variant slug"),
                    fields: vec![field("step_id", TypeRef::Named(NamedTypeId::parse("StepId").expect("valid named-type slug")))],
                },
                VariantSchema {
                name: EnumVariantId::parse("ProjectTargetSuccess").expect("valid variant slug"),
                    fields: vec![
                        field("step_id", TypeRef::Named(NamedTypeId::parse("StepId").expect("valid named-type slug"))),
                        field("target_id", TypeRef::Named(NamedTypeId::parse("MeerkatId").expect("valid named-type slug"))),
                    ],
                },
                VariantSchema {
                name: EnumVariantId::parse("ProjectTargetFailure").expect("valid variant slug"),
                    fields: vec![
                        field("step_id", TypeRef::Named(NamedTypeId::parse("StepId").expect("valid named-type slug"))),
                        field("target_id", TypeRef::Named(NamedTypeId::parse("MeerkatId").expect("valid named-type slug"))),
                    ],
                },
                VariantSchema {
                name: EnumVariantId::parse("ProjectTargetCanceled").expect("valid variant slug"),
                    fields: vec![
                        field("step_id", TypeRef::Named(NamedTypeId::parse("StepId").expect("valid named-type slug"))),
                        field("target_id", TypeRef::Named(NamedTypeId::parse("MeerkatId").expect("valid named-type slug"))),
                    ],
                },
                // v2 effects
                VariantSchema {
                name: EnumVariantId::parse("GrantNodeSlot").expect("valid variant slug"),
                    fields: vec![field("frame_id", TypeRef::Named(NamedTypeId::parse("FrameId").expect("valid named-type slug")))],
                },
                VariantSchema {
                name: EnumVariantId::parse("GrantBodyFrameStart").expect("valid variant slug"),
                    fields: vec![field(
                        "loop_instance_id",
                        TypeRef::Named(NamedTypeId::parse("LoopInstanceId").expect("valid named-type slug")),
                    )],
                },
            ],
        },
        helpers: vec![
            helper(
                "RunIsTerminal",
                vec![],
                TypeRef::Bool,
                Expr::Or(vec![
                    Expr::Eq(
                        Box::new(Expr::CurrentPhase),
                        Box::new(Expr::Phase(PhaseId::parse("Completed").expect("valid phase slug"))),
                    ),
                    Expr::Eq(
                        Box::new(Expr::CurrentPhase),
                        Box::new(Expr::Phase(PhaseId::parse("Failed").expect("valid phase slug"))),
                    ),
                    Expr::Eq(
                        Box::new(Expr::CurrentPhase),
                        Box::new(Expr::Phase(PhaseId::parse("Canceled").expect("valid phase slug"))),
                    ),
                ]),
            ),
            helper(
                "StepIsTracked",
                vec![field("step_id", TypeRef::Named(NamedTypeId::parse("StepId").expect("valid named-type slug")))],
                TypeRef::Bool,
                Expr::Contains {
                    collection: Box::new(Expr::Field(FieldId::parse("tracked_steps").expect("valid field slug"))),
                    value: Box::new(Expr::Binding("step_id".into())),
                },
            ),
            helper(
                "StepStatusIs",
                vec![
                    field("step_id", TypeRef::Named(NamedTypeId::parse("StepId").expect("valid named-type slug"))),
                    field("expected_status", TypeRef::Enum(EnumTypeId::parse("StepRunStatus").expect("valid enum-type slug"))),
                ],
                TypeRef::Bool,
                Expr::Eq(
                    Box::new(Expr::MapGet {
                        map: Box::new(Expr::Field(FieldId::parse("step_status").expect("valid field slug"))),
                        key: Box::new(Expr::Binding("step_id".into())),
                    }),
                    Box::new(Expr::Some(Box::new(Expr::Binding("expected_status".into())))),
                ),
            ),
            helper(
                "StepOutputRecordedIs",
                vec![
                    field("step_id", TypeRef::Named(NamedTypeId::parse("StepId").expect("valid named-type slug"))),
                    field("expected", TypeRef::Bool),
                ],
                TypeRef::Bool,
                Expr::Eq(
                    Box::new(Expr::MapGet {
                        map: Box::new(Expr::Field(FieldId::parse("output_recorded").expect("valid field slug"))),
                        key: Box::new(Expr::Binding("step_id".into())),
                    }),
                    Box::new(Expr::Binding("expected".into())),
                ),
            ),
            helper(
                "StepConditionRecordedIs",
                vec![
                    field("step_id", TypeRef::Named(NamedTypeId::parse("StepId").expect("valid named-type slug"))),
                    field("expected", TypeRef::Option(Box::new(TypeRef::Bool))),
                ],
                TypeRef::Bool,
                Expr::Eq(
                    Box::new(Expr::MapGet {
                        map: Box::new(Expr::Field(FieldId::parse("step_condition_results").expect("valid field slug"))),
                        key: Box::new(Expr::Binding("step_id".into())),
                    }),
                    Box::new(Expr::Binding("expected".into())),
                ),
            ),
            helper(
                "StepConditionAllowsDispatch",
                vec![field("step_id", TypeRef::Named(NamedTypeId::parse("StepId").expect("valid named-type slug")))],
                TypeRef::Bool,
                Expr::Or(vec![
                    Expr::Eq(
                        Box::new(Expr::MapGet {
                            map: Box::new(Expr::Field(FieldId::parse("step_has_conditions").expect("valid field slug"))),
                            key: Box::new(Expr::Binding("step_id".into())),
                        }),
                        Box::new(Expr::Bool(false)),
                    ),
                    Expr::Call {
                        helper: "StepConditionRecordedIs".into(),
                        args: vec![
                            Expr::Binding("step_id".into()),
                            Expr::Some(Box::new(Expr::Bool(true))),
                        ],
                    },
                ]),
            ),
            helper(
                "AllTrackedStepsInAllowedStatuses",
                vec![field(
                    "allowed_statuses",
                    TypeRef::Seq(Box::new(TypeRef::Option(Box::new(TypeRef::Enum(EnumTypeId::parse("StepRunStatus").expect("valid enum-type slug")))))),
                )],
                TypeRef::Bool,
                Expr::Quantified {
                    quantifier: Quantifier::All,
                    binding: "step_id".into(),
                    over: Box::new(Expr::Field(FieldId::parse("tracked_steps").expect("valid field slug"))),
                    body: Box::new(Expr::Contains {
                        collection: Box::new(Expr::Binding("allowed_statuses".into())),
                        value: Box::new(Expr::MapGet {
                            map: Box::new(Expr::Field(FieldId::parse("step_status").expect("valid field slug"))),
                            key: Box::new(Expr::Binding("step_id".into())),
                        }),
                    }),
                },
            ),
            helper(
                "NoTrackedStepInStatus",
                vec![field("status", TypeRef::Enum(EnumTypeId::parse("StepRunStatus").expect("valid enum-type slug")))],
                TypeRef::Bool,
                Expr::Quantified {
                    quantifier: Quantifier::All,
                    binding: "step_id".into(),
                    over: Box::new(Expr::Field(FieldId::parse("tracked_steps").expect("valid field slug"))),
                    body: Box::new(Expr::Neq(
                        Box::new(Expr::MapGet {
                            map: Box::new(Expr::Field(FieldId::parse("step_status").expect("valid field slug"))),
                            key: Box::new(Expr::Binding("step_id".into())),
                        }),
                        Box::new(Expr::Some(Box::new(Expr::Binding("status".into())))),
                    )),
                },
            ),
            helper(
                "AnyTrackedStepInStatus",
                vec![field("status", TypeRef::Enum(EnumTypeId::parse("StepRunStatus").expect("valid enum-type slug")))],
                TypeRef::Bool,
                Expr::Quantified {
                    quantifier: Quantifier::Any,
                    binding: "step_id".into(),
                    over: Box::new(Expr::Field(FieldId::parse("tracked_steps").expect("valid field slug"))),
                    body: Box::new(Expr::Eq(
                        Box::new(Expr::MapGet {
                            map: Box::new(Expr::Field(FieldId::parse("step_status").expect("valid field slug"))),
                            key: Box::new(Expr::Binding("step_id".into())),
                        }),
                        Box::new(Expr::Some(Box::new(Expr::Binding("status".into())))),
                    )),
                },
            ),
            helper(
                "StepHasDependencies",
                vec![field("step_id", TypeRef::Named(NamedTypeId::parse("StepId").expect("valid named-type slug")))],
                TypeRef::Bool,
                Expr::Gt(
                    Box::new(Expr::Len(Box::new(step_dependencies_for("step_id")))),
                    Box::new(Expr::U64(0)),
                ),
            ),
            helper(
                "AllDependenciesCompleted",
                vec![field("step_id", TypeRef::Named(NamedTypeId::parse("StepId").expect("valid named-type slug")))],
                TypeRef::Bool,
                Expr::Quantified {
                    quantifier: Quantifier::All,
                    binding: "dependency".into(),
                    over: Box::new(Expr::SeqElements(Box::new(step_dependencies_for(
                        "step_id",
                    )))),
                    body: Box::new(Expr::Call {
                        helper: "StepStatusIs".into(),
                        args: vec![
                            Expr::Binding("dependency".into()),
                            step_status(StepStatusVariant::Completed),
                        ],
                    }),
                },
            ),
            helper(
                "AllDependenciesSkipped",
                vec![field("step_id", TypeRef::Named(NamedTypeId::parse("StepId").expect("valid named-type slug")))],
                TypeRef::Bool,
                Expr::Quantified {
                    quantifier: Quantifier::All,
                    binding: "dependency".into(),
                    over: Box::new(Expr::SeqElements(Box::new(step_dependencies_for(
                        "step_id",
                    )))),
                    body: Box::new(Expr::Call {
                        helper: "StepStatusIs".into(),
                        args: vec![
                            Expr::Binding("dependency".into()),
                            step_status(StepStatusVariant::Skipped),
                        ],
                    }),
                },
            ),
            helper(
                "AnyDependencyCompleted",
                vec![field("step_id", TypeRef::Named(NamedTypeId::parse("StepId").expect("valid named-type slug")))],
                TypeRef::Bool,
                Expr::Quantified {
                    quantifier: Quantifier::Any,
                    binding: "dependency".into(),
                    over: Box::new(Expr::SeqElements(Box::new(step_dependencies_for(
                        "step_id",
                    )))),
                    body: Box::new(Expr::Call {
                        helper: "StepStatusIs".into(),
                        args: vec![
                            Expr::Binding("dependency".into()),
                            step_status(StepStatusVariant::Completed),
                        ],
                    }),
                },
            ),
            helper(
                "StepDependencyReady",
                vec![field("step_id", TypeRef::Named(NamedTypeId::parse("StepId").expect("valid named-type slug")))],
                TypeRef::Bool,
                Expr::IfElse {
                    condition: Box::new(Expr::Eq(
                        Box::new(step_dependency_mode_for("step_id")),
                        Box::new(dependency_mode(DependencyModeVariant::Any)),
                    )),
                    then_expr: Box::new(Expr::Call {
                        helper: "AnyDependencyCompleted".into(),
                        args: vec![Expr::Binding("step_id".into())],
                    }),
                    else_expr: Box::new(Expr::IfElse {
                        condition: Box::new(Expr::Not(Box::new(Expr::Call {
                            helper: "StepHasDependencies".into(),
                            args: vec![Expr::Binding("step_id".into())],
                        }))),
                        then_expr: Box::new(Expr::Bool(true)),
                        else_expr: Box::new(Expr::Call {
                            helper: "AllDependenciesCompleted".into(),
                            args: vec![Expr::Binding("step_id".into())],
                        }),
                    }),
                },
            ),
            helper(
                "StepDependencyShouldSkip",
                vec![field("step_id", TypeRef::Named(NamedTypeId::parse("StepId").expect("valid named-type slug")))],
                TypeRef::Bool,
                Expr::And(vec![
                    Expr::Eq(
                        Box::new(step_dependency_mode_for("step_id")),
                        Box::new(dependency_mode(DependencyModeVariant::Any)),
                    ),
                    Expr::Call {
                        helper: "StepHasDependencies".into(),
                        args: vec![Expr::Binding("step_id".into())],
                    },
                    Expr::Call {
                        helper: "AllDependenciesSkipped".into(),
                        args: vec![Expr::Binding("step_id".into())],
                    },
                ]),
            ),
            helper(
                "StepBranchBlocked",
                vec![field("step_id", TypeRef::Named(NamedTypeId::parse("StepId").expect("valid named-type slug")))],
                TypeRef::Bool,
                Expr::IfElse {
                    condition: Box::new(Expr::Eq(
                        Box::new(step_branch_for("step_id")),
                        Box::new(Expr::None),
                    )),
                    then_expr: Box::new(Expr::Bool(false)),
                    else_expr: Box::new(Expr::Quantified {
                        quantifier: Quantifier::Any,
                        binding: "candidate".into(),
                        over: Box::new(Expr::Field(FieldId::parse("tracked_steps").expect("valid field slug"))),
                        body: Box::new(Expr::And(vec![
                            Expr::Neq(
                                Box::new(Expr::Binding("candidate".into())),
                                Box::new(Expr::Binding("step_id".into())),
                            ),
                            Expr::Eq(
                                Box::new(step_branch_for("candidate")),
                                Box::new(step_branch_for("step_id")),
                            ),
                            Expr::Call {
                                helper: "StepStatusIs".into(),
                                args: vec![
                                    Expr::Binding("candidate".into()),
                                    step_status(StepStatusVariant::Completed),
                                ],
                            },
                        ])),
                    }),
                },
            ),
            helper(
                "EscalationWillTrigger",
                vec![],
                TypeRef::Bool,
                Expr::And(vec![
                    Expr::Gt(
                        Box::new(Expr::Field(FieldId::parse("escalation_threshold").expect("valid field slug"))),
                        Box::new(Expr::U64(0)),
                    ),
                    Expr::Gte(
                        Box::new(Expr::Add(
                            Box::new(Expr::Field(FieldId::parse("consecutive_failure_count").expect("valid field slug"))),
                            Box::new(Expr::U64(1)),
                        )),
                        Box::new(Expr::Field(FieldId::parse("escalation_threshold").expect("valid field slug"))),
                    ),
                ]),
            ),
            helper(
                "TargetRetryCount",
                vec![field("retry_key", TypeRef::String)],
                TypeRef::U32,
                target_retry_count_for("retry_key"),
            ),
            helper(
                "TargetRetryAllowed",
                vec![field("retry_key", TypeRef::String)],
                TypeRef::Bool,
                Expr::Lte(
                    Box::new(target_retry_count_for("retry_key")),
                    Box::new(Expr::Field(FieldId::parse("max_step_retries").expect("valid field slug"))),
                ),
            ),
            helper(
                "CollectionSatisfied",
                vec![field("step_id", TypeRef::Named(NamedTypeId::parse("StepId").expect("valid named-type slug")))],
                TypeRef::Bool,
                Expr::IfElse {
                    condition: Box::new(Expr::Eq(
                        Box::new(step_collection_policy_for("step_id")),
                        Box::new(collection_policy_kind(CollectionPolicyVariant::All)),
                    )),
                    then_expr: Box::new(Expr::Eq(
                        Box::new(step_target_success_count_for("step_id")),
                        Box::new(step_target_count_for("step_id")),
                    )),
                    else_expr: Box::new(Expr::IfElse {
                        condition: Box::new(Expr::Eq(
                            Box::new(step_collection_policy_for("step_id")),
                            Box::new(collection_policy_kind(CollectionPolicyVariant::Any)),
                        )),
                        then_expr: Box::new(Expr::Gte(
                            Box::new(step_target_success_count_for("step_id")),
                            Box::new(Expr::U64(1)),
                        )),
                        else_expr: Box::new(Expr::IfElse {
                            condition: Box::new(Expr::Eq(
                                Box::new(step_collection_policy_for("step_id")),
                                Box::new(collection_policy_kind(CollectionPolicyVariant::Quorum)),
                            )),
                            then_expr: Box::new(Expr::Gte(
                                Box::new(step_target_success_count_for("step_id")),
                                Box::new(step_quorum_threshold_for("step_id")),
                            )),
                            else_expr: Box::new(Expr::Bool(false)),
                        }),
                    }),
                },
            ),
            helper(
                "CollectionFeasible",
                vec![field("step_id", TypeRef::Named(NamedTypeId::parse("StepId").expect("valid named-type slug")))],
                TypeRef::Bool,
                Expr::IfElse {
                    condition: Box::new(Expr::Eq(
                        Box::new(step_collection_policy_for("step_id")),
                        Box::new(collection_policy_kind(CollectionPolicyVariant::All)),
                    )),
                    then_expr: Box::new(Expr::Eq(
                        Box::new(step_target_terminal_failure_count_for("step_id")),
                        Box::new(Expr::U64(0)),
                    )),
                    else_expr: Box::new(Expr::IfElse {
                        condition: Box::new(Expr::Eq(
                            Box::new(step_collection_policy_for("step_id")),
                            Box::new(collection_policy_kind(CollectionPolicyVariant::Any)),
                        )),
                        then_expr: Box::new(Expr::Or(vec![
                            Expr::Gte(
                                Box::new(step_target_success_count_for("step_id")),
                                Box::new(Expr::U64(1)),
                            ),
                            Expr::Gt(
                                Box::new(remaining_target_count_for("step_id")),
                                Box::new(Expr::U64(0)),
                            ),
                        ])),
                        else_expr: Box::new(Expr::IfElse {
                            condition: Box::new(Expr::Eq(
                                Box::new(step_collection_policy_for("step_id")),
                                Box::new(collection_policy_kind(CollectionPolicyVariant::Quorum)),
                            )),
                            then_expr: Box::new(Expr::Gte(
                                Box::new(Expr::Add(
                                    Box::new(step_target_success_count_for("step_id")),
                                    Box::new(remaining_target_count_for("step_id")),
                                )),
                                Box::new(step_quorum_threshold_for("step_id")),
                            )),
                            else_expr: Box::new(Expr::Bool(false)),
                        }),
                    }),
                },
            ),
            helper(
                "StepTargetCount",
                vec![field("step_id", TypeRef::Named(NamedTypeId::parse("StepId").expect("valid named-type slug")))],
                TypeRef::U32,
                step_target_count_for("step_id"),
            ),
            helper(
                "StepTargetSuccessCount",
                vec![field("step_id", TypeRef::Named(NamedTypeId::parse("StepId").expect("valid named-type slug")))],
                TypeRef::U32,
                step_target_success_count_for("step_id"),
            ),
            helper(
                "StepTargetTerminalFailureCount",
                vec![field("step_id", TypeRef::Named(NamedTypeId::parse("StepId").expect("valid named-type slug")))],
                TypeRef::U32,
                step_target_terminal_failure_count_for("step_id"),
            ),
            helper(
                "RemainingTargetCount",
                vec![field("step_id", TypeRef::Named(NamedTypeId::parse("StepId").expect("valid named-type slug")))],
                TypeRef::U32,
                remaining_target_count_for("step_id"),
            ),
        ],
        derived: vec![],
        invariants: vec![
            InvariantSchema {
                name: "output_only_follows_completed_steps".into(),
                expr: Expr::Quantified {
                    quantifier: Quantifier::All,
                    binding: "step_id".into(),
                    over: Box::new(Expr::Field(FieldId::parse("tracked_steps").expect("valid field slug"))),
                    body: Box::new(Expr::Or(vec![
                        Expr::Not(Box::new(Expr::Call {
                            helper: "StepOutputRecordedIs".into(),
                            args: vec![Expr::Binding("step_id".into()), Expr::Bool(true)],
                        })),
                        Expr::Call {
                            helper: "StepStatusIs".into(),
                            args: vec![
                                Expr::Binding("step_id".into()),
                                step_status(StepStatusVariant::Completed),
                            ],
                        },
                    ])),
                },
            },
            InvariantSchema {
                name: "terminal_runs_have_no_dispatched_steps".into(),
                expr: Expr::Or(vec![
                    Expr::Not(Box::new(Expr::Call {
                        helper: "RunIsTerminal".into(),
                        args: vec![],
                    })),
                    Expr::Call {
                        helper: "NoTrackedStepInStatus".into(),
                        args: vec![step_status(StepStatusVariant::Dispatched)],
                    },
                ]),
            },
            InvariantSchema {
                name: "completed_runs_contain_only_completed_or_skipped_steps".into(),
                expr: Expr::Or(vec![
                    Expr::Neq(
                        Box::new(Expr::CurrentPhase),
                        Box::new(Expr::Phase(PhaseId::parse("Completed").expect("valid phase slug"))),
                    ),
                    Expr::Call {
                        helper: "AllTrackedStepsInAllowedStatuses".into(),
                        args: vec![Expr::SeqLiteral(vec![
                            Expr::Some(Box::new(step_status(StepStatusVariant::Completed))),
                            Expr::Some(Box::new(step_status(StepStatusVariant::Skipped))),
                        ])],
                    },
                ]),
            },
            InvariantSchema {
                name: "failed_step_presence_requires_failure_count".into(),
                expr: Expr::Or(vec![
                    Expr::Not(Box::new(Expr::Call {
                        helper: "AnyTrackedStepInStatus".into(),
                        args: vec![step_status(StepStatusVariant::Failed)],
                    })),
                    Expr::Gte(
                        Box::new(Expr::Field(FieldId::parse("failure_count").expect("valid field slug"))),
                        Box::new(Expr::U64(1)),
                    ),
                ]),
            },
            InvariantSchema {
                name: "failed_run_has_failed_step_or_recorded_failure".into(),
                expr: Expr::Or(vec![
                    Expr::Neq(
                        Box::new(Expr::CurrentPhase),
                        Box::new(Expr::Phase(PhaseId::parse("Failed").expect("valid phase slug"))),
                    ),
                    Expr::Bool(true),
                ]),
            },
        ],
        transitions: vec![
            TransitionSchema {
                name: TransitionId::parse("CreateRun").expect("valid transition slug"),
                from: vec![PhaseId::parse("Absent").expect("valid phase slug")],
                on: TriggerMatch::Input { variant: InputVariantId::parse("CreateRun").expect("valid input-variant slug"), bindings: vec![
                        FieldId::parse("step_ids").expect("valid field slug"),
                        FieldId::parse("ordered_steps").expect("valid field slug"),
                        FieldId::parse("step_has_conditions").expect("valid field slug"),
                        FieldId::parse("step_dependencies").expect("valid field slug"),
                        FieldId::parse("step_dependency_modes").expect("valid field slug"),
                        FieldId::parse("step_branches").expect("valid field slug"),
                        FieldId::parse("step_collection_policies").expect("valid field slug"),
                        FieldId::parse("step_quorum_thresholds").expect("valid field slug"),
                        FieldId::parse("escalation_threshold").expect("valid field slug"),
                        FieldId::parse("max_step_retries").expect("valid field slug"),
                        FieldId::parse("max_active_nodes").expect("valid field slug"),
                        FieldId::parse("max_active_frames").expect("valid field slug"),
                        FieldId::parse("max_frame_depth").expect("valid field slug"),
                    ] },
                guards: vec![
                    Guard {
                        name: "step_ids_are_non_empty".into(),
                        expr: Expr::Gt(
                            Box::new(Expr::Len(Box::new(Expr::Binding("step_ids".into())))),
                            Box::new(Expr::U64(0)),
                        ),
                    },
                    Guard {
                        name: "ordered_steps_only_reference_step_ids".into(),
                        expr: sequence_members_are_in_binding("ordered_steps", "step_ids"),
                    },
                    Guard {
                        name: "step_ids_appear_in_ordered_steps".into(),
                        expr: sequence_members_are_in_binding("step_ids", "ordered_steps"),
                    },
                    map_keys_match_step_ids_guard("step_has_conditions"),
                    map_keys_match_step_ids_guard("step_dependencies"),
                    map_keys_match_step_ids_guard("step_dependency_modes"),
                    map_keys_match_step_ids_guard("step_branches"),
                    map_keys_match_step_ids_guard("step_collection_policies"),
                    map_keys_match_step_ids_guard("step_quorum_thresholds"),
                    Guard {
                        name: "step_dependencies_reference_known_steps".into(),
                        expr: Expr::Quantified {
                            quantifier: Quantifier::All,
                            binding: "step_id".into(),
                            over: Box::new(Expr::MapKeys(Box::new(Expr::Binding("step_dependencies".into())))),
                            body: Box::new(Expr::Quantified {
                                quantifier: Quantifier::All,
                                binding: "dependency".into(),
                                over: Box::new(Expr::SeqElements(Box::new(Expr::MapGet {
                                    map: Box::new(Expr::Binding("step_dependencies".into())),
                                    key: Box::new(Expr::Binding("step_id".into())),
                                }))),
                                body: Box::new(Expr::Contains {
                                    collection: Box::new(Expr::SeqElements(Box::new(
                                        Expr::Binding("step_ids".into()),
                                    ))),
                                    value: Box::new(Expr::Binding("dependency".into())),
                                }),
                            }),
                        },
                    },
                ],
                updates: vec![
                    Update::Assign {
                        field: FieldId::parse("tracked_steps").expect("valid field slug"),
                        expr: Expr::EmptySet,
                    },
                    Update::Assign {
                        field: FieldId::parse("ordered_steps").expect("valid field slug"),
                        expr: Expr::Binding("ordered_steps".into()),
                    },
                    Update::Assign {
                        field: FieldId::parse("step_status").expect("valid field slug"),
                        expr: Expr::EmptyMap,
                    },
                    Update::Assign {
                        field: FieldId::parse("output_recorded").expect("valid field slug"),
                        expr: Expr::EmptyMap,
                    },
                    Update::Assign {
                        field: FieldId::parse("step_condition_results").expect("valid field slug"),
                        expr: Expr::EmptyMap,
                    },
                    Update::Assign {
                        field: FieldId::parse("step_has_conditions").expect("valid field slug"),
                        expr: Expr::Binding("step_has_conditions".into()),
                    },
                    Update::Assign {
                        field: FieldId::parse("failure_count").expect("valid field slug"),
                        expr: Expr::U64(0),
                    },
                    Update::Assign {
                        field: FieldId::parse("consecutive_failure_count").expect("valid field slug"),
                        expr: Expr::U64(0),
                    },
                    Update::Assign {
                        field: FieldId::parse("step_dependencies").expect("valid field slug"),
                        expr: Expr::Binding("step_dependencies".into()),
                    },
                    Update::Assign {
                        field: FieldId::parse("step_dependency_modes").expect("valid field slug"),
                        expr: Expr::Binding("step_dependency_modes".into()),
                    },
                    Update::Assign {
                        field: FieldId::parse("step_branches").expect("valid field slug"),
                        expr: Expr::Binding("step_branches".into()),
                    },
                    Update::Assign {
                        field: FieldId::parse("step_collection_policies").expect("valid field slug"),
                        expr: Expr::Binding("step_collection_policies".into()),
                    },
                    Update::Assign {
                        field: FieldId::parse("step_quorum_thresholds").expect("valid field slug"),
                        expr: Expr::Binding("step_quorum_thresholds".into()),
                    },
                    Update::Assign {
                        field: FieldId::parse("step_target_counts").expect("valid field slug"),
                        expr: Expr::EmptyMap,
                    },
                    Update::Assign {
                        field: FieldId::parse("step_target_success_counts").expect("valid field slug"),
                        expr: Expr::EmptyMap,
                    },
                    Update::Assign {
                        field: FieldId::parse("step_target_terminal_failure_counts").expect("valid field slug"),
                        expr: Expr::EmptyMap,
                    },
                    Update::Assign {
                        field: FieldId::parse("target_retry_counts").expect("valid field slug"),
                        expr: Expr::EmptyMap,
                    },
                    Update::Assign {
                        field: FieldId::parse("escalation_threshold").expect("valid field slug"),
                        expr: Expr::Binding("escalation_threshold".into()),
                    },
                    Update::Assign {
                        field: FieldId::parse("max_step_retries").expect("valid field slug"),
                        expr: Expr::Binding("max_step_retries".into()),
                    },
                    // v2 field inits in CreateRun
                    Update::Assign {
                        field: FieldId::parse("ready_frames").expect("valid field slug"),
                        expr: Expr::SeqLiteral(vec![]),
                    },
                    Update::Assign {
                        field: FieldId::parse("ready_frame_membership").expect("valid field slug"),
                        expr: Expr::EmptySet,
                    },
                    Update::Assign {
                        field: FieldId::parse("pending_body_frame_loops").expect("valid field slug"),
                        expr: Expr::SeqLiteral(vec![]),
                    },
                    Update::Assign {
                        field: FieldId::parse("pending_body_frame_loop_membership").expect("valid field slug"),
                        expr: Expr::EmptySet,
                    },
                    Update::Assign {
                        field: FieldId::parse("active_node_count").expect("valid field slug"),
                        expr: Expr::U64(0),
                    },
                    Update::Assign {
                        field: FieldId::parse("active_frame_count").expect("valid field slug"),
                        expr: Expr::U64(0),
                    },
                    Update::Assign {
                        field: FieldId::parse("max_active_nodes").expect("valid field slug"),
                        expr: Expr::Binding("max_active_nodes".into()),
                    },
                    Update::Assign {
                        field: FieldId::parse("max_active_frames").expect("valid field slug"),
                        expr: Expr::Binding("max_active_frames".into()),
                    },
                    Update::Assign {
                        field: FieldId::parse("max_frame_depth").expect("valid field slug"),
                        expr: Expr::Binding("max_frame_depth".into()),
                    },
                    Update::Assign {
                        field: FieldId::parse("last_granted_frame").expect("valid field slug"),
                        expr: Expr::String(String::new()),
                    },
                    Update::Assign {
                        field: FieldId::parse("last_granted_loop").expect("valid field slug"),
                        expr: Expr::String(String::new()),
                    },
                    Update::ForEach {
                        binding: "step_id".into(),
                        over: Expr::Binding("step_ids".into()),
                        updates: vec![
                            Update::SetInsert {
                                field: FieldId::parse("tracked_steps").expect("valid field slug"),
                                value: Expr::Binding("step_id".into()),
                            },
                            Update::MapInsert {
                                field: FieldId::parse("step_status").expect("valid field slug"),
                                key: Expr::Binding("step_id".into()),
                                value: Expr::None,
                            },
                            Update::MapInsert {
                                field: FieldId::parse("output_recorded").expect("valid field slug"),
                                key: Expr::Binding("step_id".into()),
                                value: Expr::Bool(false),
                            },
                            Update::MapInsert {
                                field: FieldId::parse("step_condition_results").expect("valid field slug"),
                                key: Expr::Binding("step_id".into()),
                                value: Expr::None,
                            },
                            Update::MapInsert {
                                field: FieldId::parse("step_target_counts").expect("valid field slug"),
                                key: Expr::Binding("step_id".into()),
                                value: Expr::U64(0),
                            },
                            Update::MapInsert {
                                field: FieldId::parse("step_target_success_counts").expect("valid field slug"),
                                key: Expr::Binding("step_id".into()),
                                value: Expr::U64(0),
                            },
                            Update::MapInsert {
                                field: FieldId::parse("step_target_terminal_failure_counts").expect("valid field slug"),
                                key: Expr::Binding("step_id".into()),
                                value: Expr::U64(0),
                            },
                        ],
                    },
                ],
                to: PhaseId::parse("Pending").expect("valid phase slug"),
                emit: vec![flow_run_notice(FlowRunPhaseVariant::Pending)],
            },
            transition(
                "StartRun",
                &["Pending"],
                "StartRun",
                vec![],
                vec![flow_run_notice(FlowRunPhaseVariant::Running)],
                "Running",
            ),
            step_transition(
                "DispatchStep",
                "DispatchStep",
                vec![
                    tracked_step_guard("step_id"),
                    step_is_not_started_guard("step_id", "item_is_not_yet_dispatched"),
                    Guard {
                        name: "condition_allows_dispatch".into(),
                        expr: Expr::Call {
                            helper: "StepConditionAllowsDispatch".into(),
                            args: vec![Expr::Binding("step_id".into())],
                        },
                    },
                    Guard {
                        name: "dependencies_are_ready".into(),
                        expr: Expr::Call {
                            helper: "StepDependencyReady".into(),
                            args: vec![Expr::Binding("step_id".into())],
                        },
                    },
                    Guard {
                        name: "branch_is_not_blocked".into(),
                        expr: Expr::Not(Box::new(Expr::Call {
                            helper: "StepBranchBlocked".into(),
                            args: vec![Expr::Binding("step_id".into())],
                        })),
                    },
                ],
                vec![Update::MapInsert {
                    field: FieldId::parse("step_status").expect("valid field slug"),
                    key: Expr::Binding("step_id".into()),
                    value: Expr::Some(Box::new(step_status(StepStatusVariant::Dispatched))),
                }],
                vec![
                    step_notice("step_id", StepStatusVariant::Dispatched),
                    effect_with_step("AdmitStepWork", "step_id"),
                ],
            ),
            step_transition(
                "CompleteStep",
                "CompleteStep",
                vec![
                    tracked_step_guard("step_id"),
                    step_status_guard(
                        "step_id",
                        "step_is_dispatched",
                        StepStatusVariant::Dispatched,
                    ),
                ],
                vec![
                    Update::MapInsert {
                        field: FieldId::parse("step_status").expect("valid field slug"),
                        key: Expr::Binding("step_id".into()),
                        value: Expr::Some(Box::new(step_status(StepStatusVariant::Completed))),
                    },
                    Update::Assign {
                        field: FieldId::parse("consecutive_failure_count").expect("valid field slug"),
                        expr: Expr::U64(0),
                    },
                ],
                vec![step_notice("step_id", StepStatusVariant::Completed)],
            ),
            step_transition(
                "RecordStepOutput",
                "RecordStepOutput",
                vec![
                    tracked_step_guard("step_id"),
                    step_status_guard("step_id", "step_is_completed", StepStatusVariant::Completed),
                    Guard {
                        name: "output_not_yet_recorded".into(),
                        expr: Expr::Call {
                            helper: "StepOutputRecordedIs".into(),
                            args: vec![Expr::Binding("step_id".into()), Expr::Bool(false)],
                        },
                    },
                ],
                vec![Update::MapInsert {
                    field: FieldId::parse("output_recorded").expect("valid field slug"),
                    key: Expr::Binding("step_id".into()),
                    value: Expr::Bool(true),
                }],
                vec![effect_with_step("PersistStepOutput", "step_id")],
            ),
            step_transition(
                "ConditionPassed",
                "ConditionPassed",
                vec![
                    tracked_step_guard("step_id"),
                    step_is_not_started_guard("step_id", "step_is_not_started"),
                ],
                vec![Update::MapInsert {
                    field: FieldId::parse("step_condition_results").expect("valid field slug"),
                    key: Expr::Binding("step_id".into()),
                    value: Expr::Some(Box::new(Expr::Bool(true))),
                }],
                vec![],
            ),
            step_transition(
                "ConditionRejected",
                "ConditionRejected",
                vec![
                    tracked_step_guard("step_id"),
                    step_is_not_started_guard("step_id", "step_is_not_started"),
                ],
                vec![
                    Update::MapInsert {
                        field: FieldId::parse("step_condition_results").expect("valid field slug"),
                        key: Expr::Binding("step_id".into()),
                        value: Expr::Some(Box::new(Expr::Bool(false))),
                    },
                    Update::MapInsert {
                        field: FieldId::parse("step_status").expect("valid field slug"),
                        key: Expr::Binding("step_id".into()),
                        value: Expr::Some(Box::new(step_status(StepStatusVariant::Skipped))),
                    },
                ],
                vec![step_notice("step_id", StepStatusVariant::Skipped)],
            ),
            step_transition(
                "FailStepEscalating",
                "FailStep",
                vec![
                    tracked_step_guard("step_id"),
                    step_status_guard(
                        "step_id",
                        "step_is_dispatched",
                        StepStatusVariant::Dispatched,
                    ),
                    Guard {
                        name: "escalation_will_trigger".into(),
                        expr: Expr::Call {
                            helper: "EscalationWillTrigger".into(),
                            args: vec![],
                        },
                    },
                ],
                vec![
                    Update::MapInsert {
                        field: FieldId::parse("step_status").expect("valid field slug"),
                        key: Expr::Binding("step_id".into()),
                        value: Expr::Some(Box::new(step_status(StepStatusVariant::Failed))),
                    },
                    Update::Increment {
                        field: FieldId::parse("failure_count").expect("valid field slug"),
                        amount: 1,
                    },
                    Update::Increment {
                        field: FieldId::parse("consecutive_failure_count").expect("valid field slug"),
                        amount: 1,
                    },
                ],
                vec![
                    step_notice("step_id", StepStatusVariant::Failed),
                    effect_with_step("AppendFailureLedger", "step_id"),
                    effect_with_step("EscalateSupervisor", "step_id"),
                ],
            ),
            step_transition(
                "FailStep",
                "FailStep",
                vec![
                    tracked_step_guard("step_id"),
                    step_status_guard(
                        "step_id",
                        "step_is_dispatched",
                        StepStatusVariant::Dispatched,
                    ),
                    Guard {
                        name: "escalation_does_not_trigger".into(),
                        expr: Expr::Not(Box::new(Expr::Call {
                            helper: "EscalationWillTrigger".into(),
                            args: vec![],
                        })),
                    },
                ],
                vec![
                    Update::MapInsert {
                        field: FieldId::parse("step_status").expect("valid field slug"),
                        key: Expr::Binding("step_id".into()),
                        value: Expr::Some(Box::new(step_status(StepStatusVariant::Failed))),
                    },
                    Update::Increment {
                        field: FieldId::parse("failure_count").expect("valid field slug"),
                        amount: 1,
                    },
                    Update::Increment {
                        field: FieldId::parse("consecutive_failure_count").expect("valid field slug"),
                        amount: 1,
                    },
                ],
                vec![
                    step_notice("step_id", StepStatusVariant::Failed),
                    effect_with_step("AppendFailureLedger", "step_id"),
                ],
            ),
            step_transition(
                "SkipStep",
                "SkipStep",
                vec![
                    tracked_step_guard("step_id"),
                    step_is_not_started_guard("step_id", "step_is_not_started"),
                ],
                vec![Update::MapInsert {
                    field: FieldId::parse("step_status").expect("valid field slug"),
                    key: Expr::Binding("step_id".into()),
                    value: Expr::Some(Box::new(step_status(StepStatusVariant::Skipped))),
                }],
                vec![step_notice("step_id", StepStatusVariant::Skipped)],
            ),
            frame_projection_transition(
                "ProjectFrameStepCompleted",
                vec![
                    tracked_step_guard("step_id"),
                    frame_projectable_guard("step_id"),
                    binding_step_status_guard(
                        "step_status",
                        "frame_status_is_completed",
                        StepStatusVariant::Completed,
                    ),
                ],
                vec![
                    Update::MapInsert {
                        field: FieldId::parse("step_status").expect("valid field slug"),
                        key: Expr::Binding("step_id".into()),
                        value: Expr::Some(Box::new(step_status(StepStatusVariant::Completed))),
                    },
                    Update::MapInsert {
                        field: FieldId::parse("output_recorded").expect("valid field slug"),
                        key: Expr::Binding("step_id".into()),
                        value: Expr::Bool(true),
                    },
                    Update::Assign {
                        field: FieldId::parse("consecutive_failure_count").expect("valid field slug"),
                        expr: Expr::U64(0),
                    },
                ],
                vec![],
            ),
            frame_projection_transition(
                "ProjectFrameStepSkipped",
                vec![
                    tracked_step_guard("step_id"),
                    frame_projectable_guard("step_id"),
                    binding_step_status_guard(
                        "step_status",
                        "frame_status_is_skipped",
                        StepStatusVariant::Skipped,
                    ),
                ],
                vec![
                    Update::MapInsert {
                        field: FieldId::parse("step_status").expect("valid field slug"),
                        key: Expr::Binding("step_id".into()),
                        value: Expr::Some(Box::new(step_status(StepStatusVariant::Skipped))),
                    },
                    Update::MapInsert {
                        field: FieldId::parse("output_recorded").expect("valid field slug"),
                        key: Expr::Binding("step_id".into()),
                        value: Expr::Bool(false),
                    },
                ],
                vec![],
            ),
            frame_projection_transition(
                "ProjectFrameStepFailedEscalatingWithLedger",
                vec![
                    tracked_step_guard("step_id"),
                    frame_projectable_guard("step_id"),
                    binding_step_status_guard(
                        "step_status",
                        "frame_status_is_failed",
                        StepStatusVariant::Failed,
                    ),
                    bool_binding_guard(
                        "append_failure_ledger",
                        "append_failure_ledger_requested",
                        true,
                    ),
                    Guard {
                        name: "escalation_will_trigger".into(),
                        expr: Expr::Call {
                            helper: "EscalationWillTrigger".into(),
                            args: vec![],
                        },
                    },
                ],
                vec![
                    Update::MapInsert {
                        field: FieldId::parse("step_status").expect("valid field slug"),
                        key: Expr::Binding("step_id".into()),
                        value: Expr::Some(Box::new(step_status(StepStatusVariant::Failed))),
                    },
                    Update::MapInsert {
                        field: FieldId::parse("output_recorded").expect("valid field slug"),
                        key: Expr::Binding("step_id".into()),
                        value: Expr::Bool(false),
                    },
                    Update::Increment {
                        field: FieldId::parse("failure_count").expect("valid field slug"),
                        amount: 1,
                    },
                    Update::Increment {
                        field: FieldId::parse("consecutive_failure_count").expect("valid field slug"),
                        amount: 1,
                    },
                ],
                vec![
                    effect_with_step("AppendFailureLedger", "step_id"),
                    effect_with_step("EscalateSupervisor", "step_id"),
                ],
            ),
            frame_projection_transition(
                "ProjectFrameStepFailedEscalatingWithoutLedger",
                vec![
                    tracked_step_guard("step_id"),
                    frame_projectable_guard("step_id"),
                    binding_step_status_guard(
                        "step_status",
                        "frame_status_is_failed",
                        StepStatusVariant::Failed,
                    ),
                    bool_binding_guard(
                        "append_failure_ledger",
                        "append_failure_ledger_not_requested",
                        false,
                    ),
                    Guard {
                        name: "escalation_will_trigger".into(),
                        expr: Expr::Call {
                            helper: "EscalationWillTrigger".into(),
                            args: vec![],
                        },
                    },
                ],
                vec![
                    Update::MapInsert {
                        field: FieldId::parse("step_status").expect("valid field slug"),
                        key: Expr::Binding("step_id".into()),
                        value: Expr::Some(Box::new(step_status(StepStatusVariant::Failed))),
                    },
                    Update::MapInsert {
                        field: FieldId::parse("output_recorded").expect("valid field slug"),
                        key: Expr::Binding("step_id".into()),
                        value: Expr::Bool(false),
                    },
                    Update::Increment {
                        field: FieldId::parse("failure_count").expect("valid field slug"),
                        amount: 1,
                    },
                    Update::Increment {
                        field: FieldId::parse("consecutive_failure_count").expect("valid field slug"),
                        amount: 1,
                    },
                ],
                vec![effect_with_step("EscalateSupervisor", "step_id")],
            ),
            frame_projection_transition(
                "ProjectFrameStepFailedWithLedger",
                vec![
                    tracked_step_guard("step_id"),
                    frame_projectable_guard("step_id"),
                    binding_step_status_guard(
                        "step_status",
                        "frame_status_is_failed",
                        StepStatusVariant::Failed,
                    ),
                    bool_binding_guard(
                        "append_failure_ledger",
                        "append_failure_ledger_requested",
                        true,
                    ),
                    Guard {
                        name: "escalation_does_not_trigger".into(),
                        expr: Expr::Not(Box::new(Expr::Call {
                            helper: "EscalationWillTrigger".into(),
                            args: vec![],
                        })),
                    },
                ],
                vec![
                    Update::MapInsert {
                        field: FieldId::parse("step_status").expect("valid field slug"),
                        key: Expr::Binding("step_id".into()),
                        value: Expr::Some(Box::new(step_status(StepStatusVariant::Failed))),
                    },
                    Update::MapInsert {
                        field: FieldId::parse("output_recorded").expect("valid field slug"),
                        key: Expr::Binding("step_id".into()),
                        value: Expr::Bool(false),
                    },
                    Update::Increment {
                        field: FieldId::parse("failure_count").expect("valid field slug"),
                        amount: 1,
                    },
                    Update::Increment {
                        field: FieldId::parse("consecutive_failure_count").expect("valid field slug"),
                        amount: 1,
                    },
                ],
                vec![effect_with_step("AppendFailureLedger", "step_id")],
            ),
            frame_projection_transition(
                "ProjectFrameStepFailedWithoutLedger",
                vec![
                    tracked_step_guard("step_id"),
                    frame_projectable_guard("step_id"),
                    binding_step_status_guard(
                        "step_status",
                        "frame_status_is_failed",
                        StepStatusVariant::Failed,
                    ),
                    bool_binding_guard(
                        "append_failure_ledger",
                        "append_failure_ledger_not_requested",
                        false,
                    ),
                    Guard {
                        name: "escalation_does_not_trigger".into(),
                        expr: Expr::Not(Box::new(Expr::Call {
                            helper: "EscalationWillTrigger".into(),
                            args: vec![],
                        })),
                    },
                ],
                vec![
                    Update::MapInsert {
                        field: FieldId::parse("step_status").expect("valid field slug"),
                        key: Expr::Binding("step_id".into()),
                        value: Expr::Some(Box::new(step_status(StepStatusVariant::Failed))),
                    },
                    Update::MapInsert {
                        field: FieldId::parse("output_recorded").expect("valid field slug"),
                        key: Expr::Binding("step_id".into()),
                        value: Expr::Bool(false),
                    },
                    Update::Increment {
                        field: FieldId::parse("failure_count").expect("valid field slug"),
                        amount: 1,
                    },
                    Update::Increment {
                        field: FieldId::parse("consecutive_failure_count").expect("valid field slug"),
                        amount: 1,
                    },
                ],
                vec![],
            ),
            step_transition(
                "CancelStep",
                "CancelStep",
                vec![
                    tracked_step_guard("step_id"),
                    Guard {
                        name: "step_is_cancelable".into(),
                        expr: Expr::Or(vec![
                            step_is_not_started_expr("step_id"),
                            Expr::Call {
                                helper: "StepStatusIs".into(),
                                args: vec![
                                    Expr::Binding("step_id".into()),
                                    step_status(StepStatusVariant::Dispatched),
                                ],
                            },
                        ]),
                    },
                ],
                vec![Update::MapInsert {
                    field: FieldId::parse("step_status").expect("valid field slug"),
                    key: Expr::Binding("step_id".into()),
                    value: Expr::Some(Box::new(step_status(StepStatusVariant::Canceled))),
                }],
                vec![step_notice("step_id", StepStatusVariant::Canceled)],
            ),
            TransitionSchema {
                name: TransitionId::parse("RegisterTargets").expect("valid transition slug"),
                from: vec![PhaseId::parse("Running").expect("valid phase slug")],
                on: TriggerMatch::Input { variant: InputVariantId::parse("RegisterTargets").expect("valid input-variant slug"), bindings: vec![FieldId::parse("step_id").expect("valid field slug"), FieldId::parse("target_count").expect("valid field slug")] },
                guards: vec![
                    tracked_step_guard("step_id"),
                    step_is_not_started_guard("step_id", "step_is_not_started"),
                ],
                updates: vec![
                    Update::MapInsert {
                        field: FieldId::parse("step_target_counts").expect("valid field slug"),
                        key: Expr::Binding("step_id".into()),
                        value: Expr::Binding("target_count".into()),
                    },
                    Update::MapInsert {
                        field: FieldId::parse("step_target_success_counts").expect("valid field slug"),
                        key: Expr::Binding("step_id".into()),
                        value: Expr::U64(0),
                    },
                    Update::MapInsert {
                        field: FieldId::parse("step_target_terminal_failure_counts").expect("valid field slug"),
                        key: Expr::Binding("step_id".into()),
                        value: Expr::U64(0),
                    },
                ],
                to: PhaseId::parse("Running").expect("valid phase slug"),
                emit: vec![],
            },
            TransitionSchema {
                name: TransitionId::parse("RecordTargetSuccess").expect("valid transition slug"),
                from: vec![PhaseId::parse("Running").expect("valid phase slug")],
                on: TriggerMatch::Input { variant: InputVariantId::parse("RecordTargetSuccess").expect("valid input-variant slug"), bindings: vec![FieldId::parse("step_id").expect("valid field slug"), FieldId::parse("target_id").expect("valid field slug")] },
                guards: vec![
                    tracked_step_guard("step_id"),
                    step_status_guard(
                        "step_id",
                        "step_is_dispatched",
                        StepStatusVariant::Dispatched,
                    ),
                ],
                updates: vec![Update::MapInsert {
                    field: FieldId::parse("step_target_success_counts").expect("valid field slug"),
                    key: Expr::Binding("step_id".into()),
                    value: Expr::Add(
                        Box::new(step_target_success_count_for("step_id")),
                        Box::new(Expr::U64(1)),
                    ),
                }],
                to: PhaseId::parse("Running").expect("valid phase slug"),
                emit: vec![EffectEmit { variant: EffectVariantId::parse("ProjectTargetSuccess").expect("valid effect-variant slug"),
                    fields: IndexMap::from([
                        (FieldId::parse("step_id").expect("valid field slug"), Expr::Binding("step_id".into())),
                        (FieldId::parse("target_id").expect("valid field slug"), Expr::Binding("target_id".into())),
                    ]),
                }],
            },
            TransitionSchema {
                name: TransitionId::parse("RecordTargetTerminalFailure").expect("valid transition slug"),
                from: vec![PhaseId::parse("Running").expect("valid phase slug")],
                on: TriggerMatch::Input { variant: InputVariantId::parse("RecordTargetTerminalFailure").expect("valid input-variant slug"), bindings: vec![FieldId::parse("step_id").expect("valid field slug")] },
                guards: vec![
                    tracked_step_guard("step_id"),
                    step_status_guard(
                        "step_id",
                        "step_is_dispatched",
                        StepStatusVariant::Dispatched,
                    ),
                ],
                updates: vec![Update::MapInsert {
                    field: FieldId::parse("step_target_terminal_failure_counts").expect("valid field slug"),
                    key: Expr::Binding("step_id".into()),
                    value: Expr::Add(
                        Box::new(step_target_terminal_failure_count_for("step_id")),
                        Box::new(Expr::U64(1)),
                    ),
                }],
                to: PhaseId::parse("Running").expect("valid phase slug"),
                emit: vec![],
            },
            TransitionSchema {
                name: TransitionId::parse("RecordTargetCanceled").expect("valid transition slug"),
                from: vec![PhaseId::parse("Running").expect("valid phase slug")],
                on: TriggerMatch::Input { variant: InputVariantId::parse("RecordTargetCanceled").expect("valid input-variant slug"), bindings: vec![FieldId::parse("step_id").expect("valid field slug"), FieldId::parse("target_id").expect("valid field slug")] },
                guards: vec![
                    tracked_step_guard("step_id"),
                    step_status_guard(
                        "step_id",
                        "step_is_dispatched",
                        StepStatusVariant::Dispatched,
                    ),
                ],
                updates: vec![Update::MapInsert {
                    field: FieldId::parse("step_target_terminal_failure_counts").expect("valid field slug"),
                    key: Expr::Binding("step_id".into()),
                    value: Expr::Add(
                        Box::new(step_target_terminal_failure_count_for("step_id")),
                        Box::new(Expr::U64(1)),
                    ),
                }],
                to: PhaseId::parse("Running").expect("valid phase slug"),
                emit: vec![EffectEmit { variant: EffectVariantId::parse("ProjectTargetCanceled").expect("valid effect-variant slug"),
                    fields: IndexMap::from([
                        (FieldId::parse("step_id").expect("valid field slug"), Expr::Binding("step_id".into())),
                        (FieldId::parse("target_id").expect("valid field slug"), Expr::Binding("target_id".into())),
                    ]),
                }],
            },
            TransitionSchema {
                name: TransitionId::parse("RecordTargetFailure").expect("valid transition slug"),
                from: vec![PhaseId::parse("Running").expect("valid phase slug")],
                on: TriggerMatch::Input { variant: InputVariantId::parse("RecordTargetFailure").expect("valid input-variant slug"), bindings: vec![FieldId::parse("step_id").expect("valid field slug"), FieldId::parse("target_id").expect("valid field slug"), FieldId::parse("retry_key").expect("valid field slug")] },
                guards: vec![
                    tracked_step_guard("step_id"),
                    step_status_guard(
                        "step_id",
                        "step_is_dispatched",
                        StepStatusVariant::Dispatched,
                    ),
                ],
                updates: vec![Update::MapInsert {
                    field: FieldId::parse("target_retry_counts").expect("valid field slug"),
                    key: Expr::Binding("retry_key".into()),
                    value: Expr::Add(
                        Box::new(target_retry_count_for("retry_key")),
                        Box::new(Expr::U64(1)),
                    ),
                }],
                to: PhaseId::parse("Running").expect("valid phase slug"),
                emit: vec![
                    EffectEmit { variant: EffectVariantId::parse("ProjectTargetFailure").expect("valid effect-variant slug"),
                        fields: IndexMap::from([
                            (FieldId::parse("step_id").expect("valid field slug"), Expr::Binding("step_id".into())),
                            (FieldId::parse("target_id").expect("valid field slug"), Expr::Binding("target_id".into())),
                        ]),
                    },
                    effect_with_step("AppendFailureLedger", "step_id"),
                ],
            },
            // RegisterReadyFrame: add frame to ready queue (dedup via membership set)
            TransitionSchema {
                name: TransitionId::parse("RegisterReadyFrame").expect("valid transition slug"),
                from: vec![PhaseId::parse("Running").expect("valid phase slug")],
                on: TriggerMatch::Input { variant: InputVariantId::parse("RegisterReadyFrame").expect("valid input-variant slug"), bindings: vec![FieldId::parse("frame_id").expect("valid field slug")] },
                guards: vec![Guard {
                    name: "frame_not_already_ready".into(),
                    expr: Expr::Not(Box::new(Expr::Contains {
                        collection: Box::new(Expr::Field(FieldId::parse("ready_frame_membership").expect("valid field slug"))),
                        value: Box::new(Expr::Binding("frame_id".into())),
                    })),
                }],
                updates: vec![
                    Update::SeqAppend {
                        field: FieldId::parse("ready_frames").expect("valid field slug"),
                        value: Expr::Binding("frame_id".into()),
                    },
                    Update::SetInsert {
                        field: FieldId::parse("ready_frame_membership").expect("valid field slug"),
                        value: Expr::Binding("frame_id".into()),
                    },
                ],
                to: PhaseId::parse("Running").expect("valid phase slug"),
                emit: vec![],
            },
            // PumpNodeScheduler: grant one node slot if queue is non-empty and under limit
            TransitionSchema {
                name: TransitionId::parse("PumpNodeScheduler").expect("valid transition slug"),
                from: vec![PhaseId::parse("Running").expect("valid phase slug")],
                on: TriggerMatch::Input { variant: InputVariantId::parse("PumpNodeScheduler").expect("valid input-variant slug"), bindings: vec![] },
                guards: vec![Guard {
                    // 0 means unlimited (dogma Rule 5: zero-as-unlimited is a typed semantic,
                    // not a convention; it must be encoded in the guard, not only in comments).
                    name: "ready_frames_available_and_under_limit".into(),
                    expr: Expr::And(vec![
                        Expr::Gt(
                            Box::new(Expr::Len(Box::new(Expr::Field(FieldId::parse("ready_frames").expect("valid field slug"))))),
                            Box::new(Expr::U64(0)),
                        ),
                        Expr::Or(vec![
                            Expr::Eq(
                                Box::new(Expr::Field(FieldId::parse("max_active_nodes").expect("valid field slug"))),
                                Box::new(Expr::U64(0)),
                            ),
                            Expr::Lt(
                                Box::new(Expr::Field(FieldId::parse("active_node_count").expect("valid field slug"))),
                                Box::new(Expr::Field(FieldId::parse("max_active_nodes").expect("valid field slug"))),
                            ),
                        ]),
                    ]),
                }],
                updates: vec![
                    // Capture the head frame_id before popping
                    Update::Assign {
                        field: FieldId::parse("last_granted_frame").expect("valid field slug"),
                        expr: Expr::Head(Box::new(Expr::Field(FieldId::parse("ready_frames").expect("valid field slug")))),
                    },
                    // Remove from membership set
                    Update::SetRemove {
                        field: FieldId::parse("ready_frame_membership").expect("valid field slug"),
                        value: Expr::Head(Box::new(Expr::Field(FieldId::parse("ready_frames").expect("valid field slug")))),
                    },
                    // Pop from queue
                    Update::SeqPopFront {
                        field: FieldId::parse("ready_frames").expect("valid field slug"),
                    },
                    // Increment active node count
                    Update::Increment {
                        field: FieldId::parse("active_node_count").expect("valid field slug"),
                        amount: 1,
                    },
                ],
                to: PhaseId::parse("Running").expect("valid phase slug"),
                emit: vec![EffectEmit { variant: EffectVariantId::parse("GrantNodeSlot").expect("valid effect-variant slug"),
                    fields: IndexMap::from([(FieldId::parse("frame_id").expect("valid field slug"), Expr::Field(FieldId::parse("last_granted_frame").expect("valid field slug")),
                    )]),
                }],
            },
            // RegisterPendingBodyFrame: add loop instance to pending frame queue (with depth guard)
            TransitionSchema {
                name: TransitionId::parse("RegisterPendingBodyFrame").expect("valid transition slug"),
                from: vec![PhaseId::parse("Running").expect("valid phase slug")],
                on: TriggerMatch::Input { variant: InputVariantId::parse("RegisterPendingBodyFrame").expect("valid input-variant slug"), bindings: vec![FieldId::parse("loop_instance_id").expect("valid field slug"), FieldId::parse("depth").expect("valid field slug")] },
                guards: vec![
                    Guard {
                        // 0 means unlimited: skip the depth check when max_frame_depth == 0.
                        name: "depth_within_limit".into(),
                        expr: Expr::Or(vec![
                            Expr::Eq(
                                Box::new(Expr::Field(FieldId::parse("max_frame_depth").expect("valid field slug"))),
                                Box::new(Expr::U64(0)),
                            ),
                            Expr::Lte(
                                Box::new(Expr::Binding("depth".into())),
                                Box::new(Expr::Field(FieldId::parse("max_frame_depth").expect("valid field slug"))),
                            ),
                        ]),
                    },
                    Guard {
                        name: "loop_not_already_pending".into(),
                        expr: Expr::Not(Box::new(Expr::Contains {
                            collection: Box::new(Expr::Field(FieldId::parse("pending_body_frame_loop_membership").expect("valid field slug"))),
                            value: Box::new(Expr::Binding("loop_instance_id".into())),
                        })),
                    },
                ],
                updates: vec![
                    Update::SeqAppend {
                        field: FieldId::parse("pending_body_frame_loops").expect("valid field slug"),
                        value: Expr::Binding("loop_instance_id".into()),
                    },
                    Update::SetInsert {
                        field: FieldId::parse("pending_body_frame_loop_membership").expect("valid field slug"),
                        value: Expr::Binding("loop_instance_id".into()),
                    },
                ],
                to: PhaseId::parse("Running").expect("valid phase slug"),
                emit: vec![],
            },
            // PumpFrameScheduler: grant one body frame start if queue is non-empty and under limit
            TransitionSchema {
                name: TransitionId::parse("PumpFrameScheduler").expect("valid transition slug"),
                from: vec![PhaseId::parse("Running").expect("valid phase slug")],
                on: TriggerMatch::Input { variant: InputVariantId::parse("PumpFrameScheduler").expect("valid input-variant slug"), bindings: vec![] },
                guards: vec![Guard {
                    name: "pending_loops_available_and_under_frame_limit".into(),
                    expr: Expr::And(vec![
                        Expr::Gt(
                            Box::new(Expr::Len(Box::new(Expr::Field(FieldId::parse("pending_body_frame_loops").expect("valid field slug"))))),
                            Box::new(Expr::U64(0)),
                        ),
                        Expr::Or(vec![
                            Expr::Eq(
                                Box::new(Expr::Field(FieldId::parse("max_active_frames").expect("valid field slug"))),
                                Box::new(Expr::U64(0)),
                            ),
                            Expr::Lt(
                                Box::new(Expr::Field(FieldId::parse("active_frame_count").expect("valid field slug"))),
                                Box::new(Expr::Field(FieldId::parse("max_active_frames").expect("valid field slug"))),
                            ),
                        ]),
                    ]),
                }],
                updates: vec![
                    // Capture the head loop_instance_id before popping
                    Update::Assign {
                        field: FieldId::parse("last_granted_loop").expect("valid field slug"),
                        expr: Expr::Head(Box::new(Expr::Field(FieldId::parse("pending_body_frame_loops").expect("valid field slug")))),
                    },
                    // Remove from membership set
                    Update::SetRemove {
                        field: FieldId::parse("pending_body_frame_loop_membership").expect("valid field slug"),
                        value: Expr::Head(Box::new(Expr::Field(FieldId::parse("pending_body_frame_loops").expect("valid field slug")))),
                    },
                    // Pop from queue
                    Update::SeqPopFront {
                        field: FieldId::parse("pending_body_frame_loops").expect("valid field slug"),
                    },
                    // Increment active frame count
                    Update::Increment {
                        field: FieldId::parse("active_frame_count").expect("valid field slug"),
                        amount: 1,
                    },
                ],
                to: PhaseId::parse("Running").expect("valid phase slug"),
                emit: vec![EffectEmit { variant: EffectVariantId::parse("GrantBodyFrameStart").expect("valid effect-variant slug"),
                    fields: IndexMap::from([(FieldId::parse("loop_instance_id").expect("valid field slug"), Expr::Field(FieldId::parse("last_granted_loop").expect("valid field slug")),
                    )]),
                }],
            },
            // NodeExecutionReleased: decrements active_node_count by 1
            TransitionSchema {
                name: TransitionId::parse("NodeExecutionReleased").expect("valid transition slug"),
                from: vec![PhaseId::parse("Running").expect("valid phase slug")],
                on: TriggerMatch::Input { variant: InputVariantId::parse("NodeExecutionReleased").expect("valid input-variant slug"), bindings: vec![FieldId::parse("frame_id").expect("valid field slug")] },
                guards: vec![Guard {
                    name: "at_least_one_active_node".into(),
                    expr: Expr::Gt(
                        Box::new(Expr::Field(FieldId::parse("active_node_count").expect("valid field slug"))),
                        Box::new(Expr::U64(0)),
                    ),
                }],
                updates: vec![Update::Decrement {
                    field: FieldId::parse("active_node_count").expect("valid field slug"),
                    amount: 1,
                }],
                to: PhaseId::parse("Running").expect("valid phase slug"),
                emit: vec![],
            },
            // FrameTerminated: decrement active_frame_count by 1
            TransitionSchema {
                name: TransitionId::parse("FrameTerminated").expect("valid transition slug"),
                from: vec![PhaseId::parse("Running").expect("valid phase slug")],
                on: TriggerMatch::Input { variant: InputVariantId::parse("FrameTerminated").expect("valid input-variant slug"), bindings: vec![FieldId::parse("frame_id").expect("valid field slug")] },
                guards: vec![Guard {
                    name: "at_least_one_active_frame".into(),
                    expr: Expr::Gt(
                        Box::new(Expr::Field(FieldId::parse("active_frame_count").expect("valid field slug"))),
                        Box::new(Expr::U64(0)),
                    ),
                }],
                updates: vec![Update::Decrement {
                    field: FieldId::parse("active_frame_count").expect("valid field slug"),
                    amount: 1,
                }],
                to: PhaseId::parse("Running").expect("valid phase slug"),
                emit: vec![],
            },
            TransitionSchema {
                name: TransitionId::parse("TerminalizeCompleted").expect("valid transition slug"),
                from: vec![PhaseId::parse("Running").expect("valid phase slug")],
                on: TriggerMatch::Input { variant: InputVariantId::parse("TerminalizeCompleted").expect("valid input-variant slug"), bindings: vec![] },
                guards: vec![Guard {
                    name: "all_steps_are_completed_or_skipped".into(),
                    expr: Expr::Call {
                        helper: "AllTrackedStepsInAllowedStatuses".into(),
                        args: vec![Expr::SeqLiteral(vec![
                            Expr::Some(Box::new(step_status(StepStatusVariant::Completed))),
                            Expr::Some(Box::new(step_status(StepStatusVariant::Skipped))),
                        ])],
                    },
                }],
                updates: vec![],
                to: PhaseId::parse("Completed").expect("valid phase slug"),
                emit: vec![
                    flow_run_notice(FlowRunPhaseVariant::Completed),
                    EffectEmit { variant: EffectVariantId::parse("FlowTerminalized").expect("valid effect-variant slug"),
                        fields: IndexMap::from([(FieldId::parse("run_status").expect("valid field slug"),
                            flow_run_status(FlowRunPhaseVariant::Completed),
                        )]),
                    },
                ],
            },
            TransitionSchema {
                name: TransitionId::parse("TerminalizeFailed").expect("valid transition slug"),
                from: vec![PhaseId::parse("Pending").expect("valid phase slug"), PhaseId::parse("Running").expect("valid phase slug")],
                on: TriggerMatch::Input { variant: InputVariantId::parse("TerminalizeFailed").expect("valid input-variant slug"), bindings: vec![] },
                guards: vec![Guard {
                    name: "no_step_remains_dispatched".into(),
                    expr: Expr::Call {
                        helper: "NoTrackedStepInStatus".into(),
                        args: vec![step_status(StepStatusVariant::Dispatched)],
                    },
                }],
                updates: vec![],
                to: PhaseId::parse("Failed").expect("valid phase slug"),
                emit: vec![
                    flow_run_notice(FlowRunPhaseVariant::Failed),
                    EffectEmit { variant: EffectVariantId::parse("FlowTerminalized").expect("valid effect-variant slug"),
                        fields: IndexMap::from([(FieldId::parse("run_status").expect("valid field slug"),
                            flow_run_status(FlowRunPhaseVariant::Failed),
                        )]),
                    },
                ],
            },
            TransitionSchema {
                name: TransitionId::parse("TerminalizeCanceled").expect("valid transition slug"),
                from: vec![PhaseId::parse("Pending").expect("valid phase slug"), PhaseId::parse("Running").expect("valid phase slug")],
                on: TriggerMatch::Input { variant: InputVariantId::parse("TerminalizeCanceled").expect("valid input-variant slug"), bindings: vec![] },
                guards: vec![Guard {
                    name: "no_step_remains_dispatched".into(),
                    expr: Expr::Call {
                        helper: "NoTrackedStepInStatus".into(),
                        args: vec![step_status(StepStatusVariant::Dispatched)],
                    },
                }],
                updates: vec![],
                to: PhaseId::parse("Canceled").expect("valid phase slug"),
                emit: vec![
                    flow_run_notice(FlowRunPhaseVariant::Canceled),
                    EffectEmit { variant: EffectVariantId::parse("FlowTerminalized").expect("valid effect-variant slug"),
                        fields: IndexMap::from([(FieldId::parse("run_status").expect("valid field slug"),
                            flow_run_status(FlowRunPhaseVariant::Canceled),
                        )]),
                    },
                ],
            },
        ],
        // Standalone CI TLC for FlowRunMachine stays at the initial state only. The CreateRun
        // input surface is intentionally rich and causes a large open-exploration cross-product
        // even at one step, while the meaningful multi-step flow semantics and invariants are
        // already exercised non-vacuously in flow_frame_loop and mob_bundle.
        ci_step_limit: Some(0),
        effect_dispositions: vec![
            disposition("EmitFlowRunNotice", EffectDisposition::External),
            disposition("EmitStepNotice", EffectDisposition::External),
            disposition("AppendFailureLedger", EffectDisposition::Local),
            disposition("PersistStepOutput", EffectDisposition::Local),
            disposition(
                "AdmitStepWork",
                EffectDisposition::Routed {
                    consumer_machines: vec![MachineId::parse("RuntimeControlMachine").expect("valid machine slug")],
                },
            ),
            disposition(
                "FlowTerminalized",
                EffectDisposition::Routed {
                    consumer_machines: vec![MachineId::parse("MobOrchestratorMachine").expect("valid machine slug")],
                },
            ),
            disposition(
                "EscalateSupervisor",
                EffectDisposition::Routed {
                    consumer_machines: vec![MachineId::parse("MobOrchestratorMachine").expect("valid machine slug")],
                },
            ),
            disposition("ProjectTargetSuccess", EffectDisposition::External),
            disposition("ProjectTargetFailure", EffectDisposition::External),
            disposition("ProjectTargetCanceled", EffectDisposition::External),
            // v2 dispositions
            disposition("GrantNodeSlot", EffectDisposition::External),
            disposition("GrantBodyFrameStart", EffectDisposition::External),
        ],
        named_types: vec![
            NamedTypeBinding::string("StepId"),
            NamedTypeBinding::string("BranchId"),
            NamedTypeBinding::string("FrameId"),
            NamedTypeBinding::string("LoopInstanceId"),
            NamedTypeBinding::string("MeerkatId"),
        ],
    }
}

fn disposition(name: &str, d: EffectDisposition) -> EffectDispositionRule {
    EffectDispositionRule {
        effect_variant: EffectVariantId::parse(name).expect("valid effect-variant slug"),
        disposition: d,
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

fn helper(name: &str, params: Vec<FieldSchema>, returns: TypeRef, body: Expr) -> HelperSchema {
    HelperSchema {
        name: name.into(),
        params,
        returns,
        body,
    }
}

fn transition(
    name: &str,
    from: &[&str],
    input_variant: &str,
    guards: Vec<Guard>,
    emit: Vec<EffectEmit>,
    to: &str,
) -> TransitionSchema {
    TransitionSchema {
        name: TransitionId::parse(name).expect("valid transition slug"),
        from: from.iter().map(|phase| PhaseId::parse(*phase).expect("valid phase slug")).collect(),
        on: TriggerMatch::Input { variant: InputVariantId::parse(input_variant).expect("valid input-variant slug"), bindings: vec![] },
        guards,
        updates: vec![],
        to: PhaseId::parse(to).expect("valid phase slug"),
        emit,
    }
}

#[derive(Debug, Clone, Copy)]
enum FlowRunPhaseVariant {
    Pending,
    Running,
    Completed,
    Failed,
    Canceled,
}

#[derive(Debug, Clone, Copy)]
enum StepStatusVariant {
    Dispatched,
    Completed,
    Failed,
    Skipped,
    Canceled,
}

#[derive(Debug, Clone, Copy)]
enum DependencyModeVariant {
    All,
    Any,
}

#[derive(Debug, Clone, Copy)]
enum CollectionPolicyVariant {
    All,
    Any,
    Quorum,
}

fn step_transition(
    name: &str,
    input_variant: &str,
    guards: Vec<Guard>,
    updates: Vec<Update>,
    emit: Vec<EffectEmit>,
) -> TransitionSchema {
    TransitionSchema {
        name: TransitionId::parse(name).expect("valid transition slug"),
        from: vec![flow_run_phase_name(FlowRunPhaseVariant::Running)],
        on: TriggerMatch::Input { variant: InputVariantId::parse(input_variant).expect("valid input-variant slug"), bindings: vec![FieldId::parse("step_id").expect("valid field slug")] },
        guards,
        updates,
        to: flow_run_phase_name(FlowRunPhaseVariant::Running),
        emit,
    }
}

fn frame_projection_transition(
    name: &str,
    guards: Vec<Guard>,
    updates: Vec<Update>,
    emit: Vec<EffectEmit>,
) -> TransitionSchema {
    TransitionSchema {
        name: TransitionId::parse(name).expect("valid transition slug"),
        from: vec![flow_run_phase_name(FlowRunPhaseVariant::Running)],
        on: TriggerMatch::Input { variant: InputVariantId::parse("ProjectFrameStepStatus").expect("valid input-variant slug"), bindings: vec![
                FieldId::parse("step_id").expect("valid field slug"),
                FieldId::parse("step_status").expect("valid field slug"),
                FieldId::parse("append_failure_ledger").expect("valid field slug"),
            ] },
        guards,
        updates,
        to: flow_run_phase_name(FlowRunPhaseVariant::Running),
        emit,
    }
}

fn tracked_step_guard(binding: &str) -> Guard {
    Guard {
        name: "step_is_tracked".into(),
        expr: Expr::Call {
            helper: "StepIsTracked".into(),
            args: vec![Expr::Binding(binding.into())],
        },
    }
}

fn step_status_guard(binding: &str, name: &str, status: StepStatusVariant) -> Guard {
    Guard {
        name: name.into(),
        expr: Expr::Call {
            helper: "StepStatusIs".into(),
            args: vec![Expr::Binding(binding.into()), step_status(status)],
        },
    }
}

fn step_is_not_started_guard(binding: &str, name: &str) -> Guard {
    Guard {
        name: name.into(),
        expr: step_is_not_started_expr(binding),
    }
}

fn frame_projectable_guard(binding: &str) -> Guard {
    Guard {
        name: "frame_projection_origin_is_unstarted_or_dispatched".into(),
        expr: step_is_frame_projectable_expr(binding),
    }
}

fn step_is_not_started_expr(binding: &str) -> Expr {
    Expr::Eq(
        Box::new(Expr::MapGet {
            map: Box::new(Expr::Field(FieldId::parse("step_status").expect("valid field slug"))),
            key: Box::new(Expr::Binding(binding.into())),
        }),
        Box::new(Expr::None),
    )
}

fn step_is_frame_projectable_expr(binding: &str) -> Expr {
    Expr::Or(vec![
        step_is_not_started_expr(binding),
        Expr::Call {
            helper: "StepStatusIs".into(),
            args: vec![
                Expr::Binding(binding.into()),
                step_status(StepStatusVariant::Dispatched),
            ],
        },
    ])
}

fn binding_step_status_guard(binding: &str, name: &str, status: StepStatusVariant) -> Guard {
    Guard {
        name: name.into(),
        expr: Expr::Eq(
            Box::new(Expr::Binding(binding.into())),
            Box::new(step_status(status)),
        ),
    }
}

fn bool_binding_guard(binding: &str, name: &str, expected: bool) -> Guard {
    Guard {
        name: name.into(),
        expr: Expr::Eq(
            Box::new(Expr::Binding(binding.into())),
            Box::new(Expr::Bool(expected)),
        ),
    }
}

fn sequence_members_are_in_binding(seq_binding: &str, allowed_binding: &str) -> Expr {
    Expr::Quantified {
        quantifier: Quantifier::All,
        binding: "value".into(),
        over: Box::new(Expr::SeqElements(Box::new(Expr::Binding(
            seq_binding.into(),
        )))),
        body: Box::new(Expr::Contains {
            collection: Box::new(Expr::SeqElements(Box::new(Expr::Binding(
                allowed_binding.into(),
            )))),
            value: Box::new(Expr::Binding("value".into())),
        }),
    }
}

fn map_keys_match_step_ids_guard(map_binding: &str) -> Guard {
    Guard {
        name: format!("{map_binding}_keys_match_step_ids"),
        expr: Expr::And(vec![
            Expr::Quantified {
                quantifier: Quantifier::All,
                binding: "step_key".into(),
                over: Box::new(Expr::MapKeys(Box::new(Expr::Binding(map_binding.into())))),
                body: Box::new(Expr::Contains {
                    collection: Box::new(Expr::SeqElements(Box::new(Expr::Binding("step_ids".into())))),
                    value: Box::new(Expr::Binding("step_key".into())),
                }),
            },
            Expr::Quantified {
                quantifier: Quantifier::All,
                binding: "step_id".into(),
                over: Box::new(Expr::SeqElements(Box::new(Expr::Binding("step_ids".into())))),
                body: Box::new(Expr::Contains {
                    collection: Box::new(Expr::MapKeys(Box::new(Expr::Binding(
                        map_binding.into(),
                    )))),
                    value: Box::new(Expr::Binding("step_id".into())),
                }),
            },
        ]),
    }
}

fn step_dependencies_for(binding: &str) -> Expr {
    Expr::IfElse {
        condition: Box::new(Expr::Contains {
            collection: Box::new(Expr::MapKeys(Box::new(Expr::Field(FieldId::parse("step_dependencies").expect("valid field slug"))))),
            value: Box::new(Expr::Binding(binding.into())),
        }),
        then_expr: Box::new(Expr::MapGet {
            map: Box::new(Expr::Field(FieldId::parse("step_dependencies").expect("valid field slug"))),
            key: Box::new(Expr::Binding(binding.into())),
        }),
        else_expr: Box::new(Expr::SeqLiteral(vec![])),
    }
}

fn step_dependency_mode_for(binding: &str) -> Expr {
    Expr::IfElse {
        condition: Box::new(Expr::Contains {
            collection: Box::new(Expr::MapKeys(Box::new(Expr::Field(FieldId::parse("step_dependency_modes").expect("valid field slug"))))),
            value: Box::new(Expr::Binding(binding.into())),
        }),
        then_expr: Box::new(Expr::MapGet {
            map: Box::new(Expr::Field(FieldId::parse("step_dependency_modes").expect("valid field slug"))),
            key: Box::new(Expr::Binding(binding.into())),
        }),
        else_expr: Box::new(dependency_mode(DependencyModeVariant::All)),
    }
}

fn step_branch_for(binding: &str) -> Expr {
    Expr::IfElse {
        condition: Box::new(Expr::Contains {
            collection: Box::new(Expr::MapKeys(Box::new(Expr::Field(FieldId::parse("step_branches").expect("valid field slug"))))),
            value: Box::new(Expr::Binding(binding.into())),
        }),
        then_expr: Box::new(Expr::MapGet {
            map: Box::new(Expr::Field(FieldId::parse("step_branches").expect("valid field slug"))),
            key: Box::new(Expr::Binding(binding.into())),
        }),
        else_expr: Box::new(Expr::None),
    }
}

fn step_collection_policy_for(binding: &str) -> Expr {
    Expr::IfElse {
        condition: Box::new(Expr::Contains {
            collection: Box::new(Expr::MapKeys(Box::new(Expr::Field(FieldId::parse("step_collection_policies").expect("valid field slug"))))),
            value: Box::new(Expr::Binding(binding.into())),
        }),
        then_expr: Box::new(Expr::MapGet {
            map: Box::new(Expr::Field(FieldId::parse("step_collection_policies").expect("valid field slug"))),
            key: Box::new(Expr::Binding(binding.into())),
        }),
        else_expr: Box::new(collection_policy_kind(CollectionPolicyVariant::All)),
    }
}

fn step_quorum_threshold_for(binding: &str) -> Expr {
    Expr::IfElse {
        condition: Box::new(Expr::Contains {
            collection: Box::new(Expr::MapKeys(Box::new(Expr::Field(FieldId::parse("step_quorum_thresholds").expect("valid field slug"))))),
            value: Box::new(Expr::Binding(binding.into())),
        }),
        then_expr: Box::new(Expr::MapGet {
            map: Box::new(Expr::Field(FieldId::parse("step_quorum_thresholds").expect("valid field slug"))),
            key: Box::new(Expr::Binding(binding.into())),
        }),
        else_expr: Box::new(Expr::U64(0)),
    }
}

fn step_target_count_for(binding: &str) -> Expr {
    Expr::IfElse {
        condition: Box::new(Expr::Contains {
            collection: Box::new(Expr::MapKeys(Box::new(Expr::Field(FieldId::parse("step_target_counts").expect("valid field slug"))))),
            value: Box::new(Expr::Binding(binding.into())),
        }),
        then_expr: Box::new(Expr::MapGet {
            map: Box::new(Expr::Field(FieldId::parse("step_target_counts").expect("valid field slug"))),
            key: Box::new(Expr::Binding(binding.into())),
        }),
        else_expr: Box::new(Expr::U64(0)),
    }
}

fn step_target_success_count_for(binding: &str) -> Expr {
    Expr::IfElse {
        condition: Box::new(Expr::Contains {
            collection: Box::new(Expr::MapKeys(Box::new(Expr::Field(FieldId::parse("step_target_success_counts").expect("valid field slug"))))),
            value: Box::new(Expr::Binding(binding.into())),
        }),
        then_expr: Box::new(Expr::MapGet {
            map: Box::new(Expr::Field(FieldId::parse("step_target_success_counts").expect("valid field slug"))),
            key: Box::new(Expr::Binding(binding.into())),
        }),
        else_expr: Box::new(Expr::U64(0)),
    }
}

fn step_target_terminal_failure_count_for(binding: &str) -> Expr {
    Expr::IfElse {
        condition: Box::new(Expr::Contains {
            collection: Box::new(Expr::MapKeys(Box::new(Expr::Field(FieldId::parse("step_target_terminal_failure_counts").expect("valid field slug"))))),
            value: Box::new(Expr::Binding(binding.into())),
        }),
        then_expr: Box::new(Expr::MapGet {
            map: Box::new(Expr::Field(FieldId::parse("step_target_terminal_failure_counts").expect("valid field slug"))),
            key: Box::new(Expr::Binding(binding.into())),
        }),
        else_expr: Box::new(Expr::U64(0)),
    }
}

fn remaining_target_count_for(binding: &str) -> Expr {
    Expr::Sub(
        Box::new(step_target_count_for(binding)),
        Box::new(step_target_terminal_failure_count_for(binding)),
    )
}

fn target_retry_count_for(binding: &str) -> Expr {
    Expr::IfElse {
        condition: Box::new(Expr::Contains {
            collection: Box::new(Expr::MapKeys(Box::new(Expr::Field(FieldId::parse("target_retry_counts").expect("valid field slug"))))),
            value: Box::new(Expr::Binding(binding.into())),
        }),
        then_expr: Box::new(Expr::MapGet {
            map: Box::new(Expr::Field(FieldId::parse("target_retry_counts").expect("valid field slug"))),
            key: Box::new(Expr::Binding(binding.into())),
        }),
        else_expr: Box::new(Expr::U64(0)),
    }
}

fn flow_run_notice(status_variant: FlowRunPhaseVariant) -> EffectEmit {
    EffectEmit { variant: EffectVariantId::parse("EmitFlowRunNotice").expect("valid effect-variant slug"),
        fields: IndexMap::from([(FieldId::parse("run_status").expect("valid field slug"), flow_run_status(status_variant))]),
    }
}

fn step_notice(step_binding: &str, status_variant: StepStatusVariant) -> EffectEmit {
    EffectEmit { variant: EffectVariantId::parse("EmitStepNotice").expect("valid effect-variant slug"),
        fields: IndexMap::from([
            (FieldId::parse("step_id").expect("valid field slug"), Expr::Binding(step_binding.into())),
            (FieldId::parse("step_status").expect("valid field slug"), step_status(status_variant)),
        ]),
    }
}

fn effect_with_step(variant: &str, step_binding: &str) -> EffectEmit {
    EffectEmit {
        variant: EffectVariantId::parse(variant).expect("valid effect-variant slug"),
        fields: IndexMap::from([(FieldId::parse("step_id").expect("valid field slug"), Expr::Binding(step_binding.into()))]),
    }
}

fn flow_run_phase_name(variant: FlowRunPhaseVariant) -> PhaseId {
    let slug = match variant {
        FlowRunPhaseVariant::Pending => "Pending",
        FlowRunPhaseVariant::Running => "Running",
        FlowRunPhaseVariant::Completed => "Completed",
        FlowRunPhaseVariant::Failed => "Failed",
        FlowRunPhaseVariant::Canceled => "Canceled",
    };
    PhaseId::parse(slug).expect("valid phase slug")
}

fn flow_run_status(variant: FlowRunPhaseVariant) -> Expr {
    let slug = match variant {
        FlowRunPhaseVariant::Pending => "Pending",
        FlowRunPhaseVariant::Running => "Running",
        FlowRunPhaseVariant::Completed => "Completed",
        FlowRunPhaseVariant::Failed => "Failed",
        FlowRunPhaseVariant::Canceled => "Canceled",
    };
    Expr::NamedVariant {
        enum_name: EnumTypeId::parse("FlowRunStatus").expect("valid enum-type slug"),
        variant: EnumVariantId::parse(slug).expect("valid enum-variant slug"),
    }
}

fn step_status(variant: StepStatusVariant) -> Expr {
    let slug = match variant {
        StepStatusVariant::Dispatched => "Dispatched",
        StepStatusVariant::Completed => "Completed",
        StepStatusVariant::Failed => "Failed",
        StepStatusVariant::Skipped => "Skipped",
        StepStatusVariant::Canceled => "Canceled",
    };
    Expr::NamedVariant {
        enum_name: EnumTypeId::parse("StepRunStatus").expect("valid enum-type slug"),
        variant: EnumVariantId::parse(slug).expect("valid enum-variant slug"),
    }
}

fn dependency_mode(variant: DependencyModeVariant) -> Expr {
    let slug = match variant {
        DependencyModeVariant::All => "All",
        DependencyModeVariant::Any => "Any",
    };
    Expr::NamedVariant {
        enum_name: EnumTypeId::parse("DependencyMode").expect("valid enum-type slug"),
        variant: EnumVariantId::parse(slug).expect("valid enum-variant slug"),
    }
}

fn collection_policy_kind(variant: CollectionPolicyVariant) -> Expr {
    let slug = match variant {
        CollectionPolicyVariant::All => "All",
        CollectionPolicyVariant::Any => "Any",
        CollectionPolicyVariant::Quorum => "Quorum",
    };
    Expr::NamedVariant {
        enum_name: EnumTypeId::parse("CollectionPolicyKind").expect("valid enum-type slug"),
        variant: EnumVariantId::parse(slug).expect("valid enum-variant slug"),
    }
}
