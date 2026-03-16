use indexmap::IndexMap;

use crate::{
    EffectEmit, EnumSchema, Expr, FieldInit, FieldSchema, Guard, InitSchema, InputMatch,
    InvariantSchema, MachineSchema, Quantifier, RustBinding, StateSchema, TransitionSchema,
    TypeRef, Update, VariantSchema,
};

pub fn runtime_ingress_machine() -> MachineSchema {
    MachineSchema {
        machine: "RuntimeIngressMachine".into(),
        version: 1,
        rust: RustBinding {
            crate_name: "meerkat-runtime".into(),
            module: "machines::runtime_ingress".into(),
        },
        state: StateSchema {
            phase: EnumSchema {
                name: "IngressPhase".into(),
                variants: vec![variant("Active"), variant("Retired"), variant("Destroyed")],
            },
            fields: vec![
                field(
                    "admitted_inputs",
                    TypeRef::Set(Box::new(TypeRef::Named("InputId".into()))),
                ),
                field(
                    "admission_order",
                    TypeRef::Seq(Box::new(TypeRef::Named("InputId".into()))),
                ),
                field(
                    "input_kind",
                    TypeRef::Map(
                        Box::new(TypeRef::Named("InputId".into())),
                        Box::new(TypeRef::Named("InputKind".into())),
                    ),
                ),
                field(
                    "policy_snapshot",
                    TypeRef::Map(
                        Box::new(TypeRef::Named("InputId".into())),
                        Box::new(TypeRef::Named("PolicyDecision".into())),
                    ),
                ),
                field(
                    "lifecycle",
                    TypeRef::Map(
                        Box::new(TypeRef::Named("InputId".into())),
                        Box::new(TypeRef::Named("InputLifecycleState".into())),
                    ),
                ),
                field(
                    "terminal_outcome",
                    TypeRef::Map(
                        Box::new(TypeRef::Named("InputId".into())),
                        Box::new(TypeRef::Option(Box::new(TypeRef::Named(
                            "InputTerminalOutcome".into(),
                        )))),
                    ),
                ),
                field(
                    "queue",
                    TypeRef::Seq(Box::new(TypeRef::Named("InputId".into()))),
                ),
                field(
                    "current_run",
                    TypeRef::Option(Box::new(TypeRef::Named("RunId".into()))),
                ),
                field(
                    "current_run_contributors",
                    TypeRef::Seq(Box::new(TypeRef::Named("InputId".into()))),
                ),
                field(
                    "last_run",
                    TypeRef::Map(
                        Box::new(TypeRef::Named("InputId".into())),
                        Box::new(TypeRef::Option(Box::new(TypeRef::Named("RunId".into())))),
                    ),
                ),
                field(
                    "last_boundary_sequence",
                    TypeRef::Map(
                        Box::new(TypeRef::Named("InputId".into())),
                        Box::new(TypeRef::Option(Box::new(TypeRef::Named(
                            "BoundarySequence".into(),
                        )))),
                    ),
                ),
                field("wake_requested", TypeRef::Bool),
                field("process_requested", TypeRef::Bool),
            ],
            init: InitSchema {
                phase: "Active".into(),
                fields: vec![
                    init("admitted_inputs", Expr::EmptySet),
                    init("admission_order", Expr::SeqLiteral(vec![])),
                    init("input_kind", Expr::EmptyMap),
                    init("policy_snapshot", Expr::EmptyMap),
                    init("lifecycle", Expr::EmptyMap),
                    init("terminal_outcome", Expr::EmptyMap),
                    init("queue", Expr::SeqLiteral(vec![])),
                    init("current_run", Expr::None),
                    init("current_run_contributors", Expr::SeqLiteral(vec![])),
                    init("last_run", Expr::EmptyMap),
                    init("last_boundary_sequence", Expr::EmptyMap),
                    init("wake_requested", Expr::Bool(false)),
                    init("process_requested", Expr::Bool(false)),
                ],
            },
            terminal_phases: vec!["Destroyed".into()],
        },
        inputs: EnumSchema {
            name: "RuntimeIngressInput".into(),
            variants: vec![
                VariantSchema {
                    name: "AdmitQueued".into(),
                    fields: vec![
                        field("input_id", TypeRef::Named("InputId".into())),
                        field("input_kind", TypeRef::Named("InputKind".into())),
                        field("policy", TypeRef::Named("PolicyDecision".into())),
                        field("wake", TypeRef::Bool),
                        field("process", TypeRef::Bool),
                    ],
                },
                VariantSchema {
                    name: "AdmitConsumedOnAccept".into(),
                    fields: vec![
                        field("input_id", TypeRef::Named("InputId".into())),
                        field("input_kind", TypeRef::Named("InputKind".into())),
                        field("policy", TypeRef::Named("PolicyDecision".into())),
                    ],
                },
                VariantSchema {
                    name: "StageDrainSnapshot".into(),
                    fields: vec![
                        field("run_id", TypeRef::Named("RunId".into())),
                        field(
                            "contributing_input_ids",
                            TypeRef::Seq(Box::new(TypeRef::Named("InputId".into()))),
                        ),
                    ],
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
                VariantSchema {
                    name: "SupersedeQueuedInput".into(),
                    fields: vec![
                        field("new_input_id", TypeRef::Named("InputId".into())),
                        field("old_input_id", TypeRef::Named("InputId".into())),
                    ],
                },
                VariantSchema {
                    name: "CoalesceQueuedInputs".into(),
                    fields: vec![
                        field("aggregate_input_id", TypeRef::Named("InputId".into())),
                        field(
                            "source_input_ids",
                            TypeRef::Seq(Box::new(TypeRef::Named("InputId".into()))),
                        ),
                    ],
                },
                variant("Retire"),
                variant("Reset"),
                variant("Destroy"),
                variant("Recover"),
            ],
        },
        effects: EnumSchema {
            name: "RuntimeIngressEffect".into(),
            variants: vec![
                VariantSchema {
                    name: "IngressAccepted".into(),
                    fields: vec![field("input_id", TypeRef::Named("InputId".into()))],
                },
                VariantSchema {
                    name: "ReadyForRun".into(),
                    fields: vec![
                        field("run_id", TypeRef::Named("RunId".into())),
                        field(
                            "contributing_input_ids",
                            TypeRef::Seq(Box::new(TypeRef::Named("InputId".into()))),
                        ),
                    ],
                },
                VariantSchema {
                    name: "InputLifecycleNotice".into(),
                    fields: vec![
                        field("input_id", TypeRef::Named("InputId".into())),
                        field("new_state", TypeRef::Named("InputLifecycleState".into())),
                    ],
                },
                variant("WakeRuntime"),
                variant("RequestImmediateProcessing"),
                VariantSchema {
                    name: "CompletionResolved".into(),
                    fields: vec![
                        field("input_id", TypeRef::Named("InputId".into())),
                        field("outcome", TypeRef::Named("InputTerminalOutcome".into())),
                    ],
                },
                VariantSchema {
                    name: "IngressNotice".into(),
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
                name: "queue_entries_are_queued".into(),
                expr: Expr::Quantified {
                    quantifier: Quantifier::All,
                    binding: "input_id".into(),
                    over: Box::new(Expr::Field("queue".into())),
                    body: Box::new(Expr::Eq(
                        Box::new(Expr::MapGet {
                            map: Box::new(Expr::Field("lifecycle".into())),
                            key: Box::new(Expr::Binding("input_id".into())),
                        }),
                        Box::new(Expr::String("Queued".into())),
                    )),
                },
            },
            InvariantSchema {
                name: "terminal_inputs_do_not_appear_in_queue".into(),
                expr: Expr::Quantified {
                    quantifier: Quantifier::All,
                    binding: "input_id".into(),
                    over: Box::new(Expr::Field("queue".into())),
                    body: Box::new(Expr::And(vec![
                        Expr::Neq(
                            Box::new(Expr::MapGet {
                                map: Box::new(Expr::Field("lifecycle".into())),
                                key: Box::new(Expr::Binding("input_id".into())),
                            }),
                            Box::new(Expr::String("Consumed".into())),
                        ),
                        Expr::Neq(
                            Box::new(Expr::MapGet {
                                map: Box::new(Expr::Field("lifecycle".into())),
                                key: Box::new(Expr::Binding("input_id".into())),
                            }),
                            Box::new(Expr::String("Superseded".into())),
                        ),
                        Expr::Neq(
                            Box::new(Expr::MapGet {
                                map: Box::new(Expr::Field("lifecycle".into())),
                                key: Box::new(Expr::Binding("input_id".into())),
                            }),
                            Box::new(Expr::String("Coalesced".into())),
                        ),
                        Expr::Neq(
                            Box::new(Expr::MapGet {
                                map: Box::new(Expr::Field("lifecycle".into())),
                                key: Box::new(Expr::Binding("input_id".into())),
                            }),
                            Box::new(Expr::String("Abandoned".into())),
                        ),
                    ])),
                },
            },
            InvariantSchema {
                name: "current_run_matches_contributor_presence".into(),
                expr: Expr::Eq(
                    Box::new(Expr::Eq(
                        Box::new(Expr::Field("current_run".into())),
                        Box::new(Expr::None),
                    )),
                    Box::new(Expr::Eq(
                        Box::new(Expr::Len(Box::new(Expr::Field(
                            "current_run_contributors".into(),
                        )))),
                        Box::new(Expr::U64(0)),
                    )),
                ),
            },
            InvariantSchema {
                name: "staged_contributors_are_not_queued".into(),
                expr: Expr::Quantified {
                    quantifier: Quantifier::All,
                    binding: "input_id".into(),
                    over: Box::new(Expr::Field("current_run_contributors".into())),
                    body: Box::new(Expr::Not(Box::new(Expr::Contains {
                        collection: Box::new(Expr::Field("queue".into())),
                        value: Box::new(Expr::Binding("input_id".into())),
                    }))),
                },
            },
            InvariantSchema {
                name: "applied_pending_consumption_has_last_run".into(),
                expr: Expr::Quantified {
                    quantifier: Quantifier::All,
                    binding: "input_id".into(),
                    over: Box::new(Expr::Field("admitted_inputs".into())),
                    body: Box::new(Expr::Or(vec![
                        Expr::Neq(
                            Box::new(Expr::MapGet {
                                map: Box::new(Expr::Field("lifecycle".into())),
                                key: Box::new(Expr::Binding("input_id".into())),
                            }),
                            Box::new(Expr::String("AppliedPendingConsumption".into())),
                        ),
                        Expr::Neq(
                            Box::new(Expr::MapGet {
                                map: Box::new(Expr::Field("last_run".into())),
                                key: Box::new(Expr::Binding("input_id".into())),
                            }),
                            Box::new(Expr::None),
                        ),
                    ])),
                },
            },
        ],
        transitions: vec![
            runtime_ingress_admit_queued_transition("AdmitQueuedNone", false, false),
            runtime_ingress_admit_queued_transition("AdmitQueuedWake", true, false),
            runtime_ingress_admit_queued_transition("AdmitQueuedProcess", false, true),
            runtime_ingress_admit_queued_transition("AdmitQueuedWakeAndProcess", true, true),
            TransitionSchema {
                name: "AdmitConsumedOnAccept".into(),
                from: vec!["Active".into()],
                on: InputMatch {
                    variant: "AdmitConsumedOnAccept".into(),
                    bindings: vec!["input_id".into(), "input_kind".into(), "policy".into()],
                },
                guards: vec![Guard {
                    name: "input_is_new".into(),
                    expr: Expr::Not(Box::new(Expr::Contains {
                        collection: Box::new(Expr::Field("admitted_inputs".into())),
                        value: Box::new(Expr::Binding("input_id".into())),
                    })),
                }],
                updates: vec![
                    Update::SetInsert {
                        field: "admitted_inputs".into(),
                        value: Expr::Binding("input_id".into()),
                    },
                    Update::SeqAppend {
                        field: "admission_order".into(),
                        value: Expr::Binding("input_id".into()),
                    },
                    Update::MapInsert {
                        field: "input_kind".into(),
                        key: Expr::Binding("input_id".into()),
                        value: Expr::Binding("input_kind".into()),
                    },
                    Update::MapInsert {
                        field: "policy_snapshot".into(),
                        key: Expr::Binding("input_id".into()),
                        value: Expr::Binding("policy".into()),
                    },
                    Update::MapInsert {
                        field: "lifecycle".into(),
                        key: Expr::Binding("input_id".into()),
                        value: Expr::String("Consumed".into()),
                    },
                    Update::MapInsert {
                        field: "terminal_outcome".into(),
                        key: Expr::Binding("input_id".into()),
                        value: Expr::Some(Box::new(Expr::String("Consumed".into()))),
                    },
                    Update::MapInsert {
                        field: "last_run".into(),
                        key: Expr::Binding("input_id".into()),
                        value: Expr::None,
                    },
                    Update::MapInsert {
                        field: "last_boundary_sequence".into(),
                        key: Expr::Binding("input_id".into()),
                        value: Expr::None,
                    },
                ],
                to: "Active".into(),
                emit: vec![
                    EffectEmit {
                        variant: "IngressAccepted".into(),
                        fields: IndexMap::from([(
                            "input_id".into(),
                            Expr::Binding("input_id".into()),
                        )]),
                    },
                    EffectEmit {
                        variant: "InputLifecycleNotice".into(),
                        fields: IndexMap::from([
                            ("input_id".into(), Expr::Binding("input_id".into())),
                            ("new_state".into(), Expr::String("Consumed".into())),
                        ]),
                    },
                    EffectEmit {
                        variant: "CompletionResolved".into(),
                        fields: IndexMap::from([
                            ("input_id".into(), Expr::Binding("input_id".into())),
                            ("outcome".into(), Expr::String("Consumed".into())),
                        ]),
                    },
                ],
            },
            TransitionSchema {
                name: "StageDrainSnapshot".into(),
                from: vec!["Active".into(), "Retired".into()],
                on: InputMatch {
                    variant: "StageDrainSnapshot".into(),
                    bindings: vec!["run_id".into(), "contributing_input_ids".into()],
                },
                guards: vec![
                    Guard {
                        name: "no_current_run".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field("current_run".into())),
                            Box::new(Expr::None),
                        ),
                    },
                    Guard {
                        name: "contributors_non_empty".into(),
                        expr: Expr::Gt(
                            Box::new(Expr::Len(Box::new(Expr::Binding(
                                "contributing_input_ids".into(),
                            )))),
                            Box::new(Expr::U64(0)),
                        ),
                    },
                    Guard {
                        name: "contributors_are_queue_prefix".into(),
                        expr: Expr::SeqStartsWith {
                            seq: Box::new(Expr::Field("queue".into())),
                            prefix: Box::new(Expr::Binding("contributing_input_ids".into())),
                        },
                    },
                    Guard {
                        name: "all_contributors_are_queued".into(),
                        expr: Expr::Quantified {
                            quantifier: Quantifier::All,
                            binding: "input_id".into(),
                            over: Box::new(Expr::Binding("contributing_input_ids".into())),
                            body: Box::new(Expr::Eq(
                                Box::new(Expr::MapGet {
                                    map: Box::new(Expr::Field("lifecycle".into())),
                                    key: Box::new(Expr::Binding("input_id".into())),
                                }),
                                Box::new(Expr::String("Queued".into())),
                            )),
                        },
                    },
                ],
                updates: vec![
                    Update::SeqRemoveAll {
                        field: "queue".into(),
                        values: Expr::Binding("contributing_input_ids".into()),
                    },
                    Update::Assign {
                        field: "current_run".into(),
                        expr: Expr::Some(Box::new(Expr::Binding("run_id".into()))),
                    },
                    Update::Assign {
                        field: "current_run_contributors".into(),
                        expr: Expr::Binding("contributing_input_ids".into()),
                    },
                    Update::Assign {
                        field: "wake_requested".into(),
                        expr: Expr::Bool(false),
                    },
                    Update::Assign {
                        field: "process_requested".into(),
                        expr: Expr::Bool(false),
                    },
                    Update::ForEach {
                        binding: "input_id".into(),
                        over: Expr::Binding("contributing_input_ids".into()),
                        updates: vec![
                            Update::MapInsert {
                                field: "last_run".into(),
                                key: Expr::Binding("input_id".into()),
                                value: Expr::Some(Box::new(Expr::Binding("run_id".into()))),
                            },
                            Update::MapInsert {
                                field: "lifecycle".into(),
                                key: Expr::Binding("input_id".into()),
                                value: Expr::String("Staged".into()),
                            },
                        ],
                    },
                ],
                to: "Active".into(),
                emit: vec![EffectEmit {
                    variant: "ReadyForRun".into(),
                    fields: IndexMap::from([
                        ("run_id".into(), Expr::Binding("run_id".into())),
                        (
                            "contributing_input_ids".into(),
                            Expr::Binding("contributing_input_ids".into()),
                        ),
                    ]),
                }],
            },
            TransitionSchema {
                name: "BoundaryApplied".into(),
                from: vec!["Active".into(), "Retired".into()],
                on: InputMatch {
                    variant: "BoundaryApplied".into(),
                    bindings: vec!["run_id".into(), "boundary_sequence".into()],
                },
                guards: vec![
                    Guard {
                        name: "run_matches_current".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field("current_run".into())),
                            Box::new(Expr::Some(Box::new(Expr::Binding("run_id".into())))),
                        ),
                    },
                    Guard {
                        name: "contributors_are_staged".into(),
                        expr: Expr::Quantified {
                            quantifier: Quantifier::All,
                            binding: "input_id".into(),
                            over: Box::new(Expr::Field("current_run_contributors".into())),
                            body: Box::new(Expr::Eq(
                                Box::new(Expr::MapGet {
                                    map: Box::new(Expr::Field("lifecycle".into())),
                                    key: Box::new(Expr::Binding("input_id".into())),
                                }),
                                Box::new(Expr::String("Staged".into())),
                            )),
                        },
                    },
                ],
                updates: vec![Update::ForEach {
                    binding: "input_id".into(),
                    over: Expr::Field("current_run_contributors".into()),
                    updates: vec![
                        Update::MapInsert {
                            field: "lifecycle".into(),
                            key: Expr::Binding("input_id".into()),
                            value: Expr::String("AppliedPendingConsumption".into()),
                        },
                        Update::MapInsert {
                            field: "last_boundary_sequence".into(),
                            key: Expr::Binding("input_id".into()),
                            value: Expr::Some(Box::new(Expr::Binding("boundary_sequence".into()))),
                        },
                    ],
                }],
                to: "Active".into(),
                emit: vec![EffectEmit {
                    variant: "IngressNotice".into(),
                    fields: IndexMap::from([
                        ("kind".into(), Expr::String("BoundaryApplied".into())),
                        (
                            "detail".into(),
                            Expr::String("ContributorsPendingConsumption".into()),
                        ),
                    ]),
                }],
            },
            TransitionSchema {
                name: "RunCompleted".into(),
                from: vec!["Active".into(), "Retired".into()],
                on: InputMatch {
                    variant: "RunCompleted".into(),
                    bindings: vec!["run_id".into()],
                },
                guards: vec![
                    Guard {
                        name: "run_matches_current".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field("current_run".into())),
                            Box::new(Expr::Some(Box::new(Expr::Binding("run_id".into())))),
                        ),
                    },
                    Guard {
                        name: "contributors_pending_consumption".into(),
                        expr: Expr::Quantified {
                            quantifier: Quantifier::All,
                            binding: "input_id".into(),
                            over: Box::new(Expr::Field("current_run_contributors".into())),
                            body: Box::new(Expr::Eq(
                                Box::new(Expr::MapGet {
                                    map: Box::new(Expr::Field("lifecycle".into())),
                                    key: Box::new(Expr::Binding("input_id".into())),
                                }),
                                Box::new(Expr::String("AppliedPendingConsumption".into())),
                            )),
                        },
                    },
                ],
                updates: vec![
                    Update::ForEach {
                        binding: "input_id".into(),
                        over: Expr::Field("current_run_contributors".into()),
                        updates: vec![
                            Update::MapInsert {
                                field: "lifecycle".into(),
                                key: Expr::Binding("input_id".into()),
                                value: Expr::String("Consumed".into()),
                            },
                            Update::MapInsert {
                                field: "terminal_outcome".into(),
                                key: Expr::Binding("input_id".into()),
                                value: Expr::Some(Box::new(Expr::String("Consumed".into()))),
                            },
                        ],
                    },
                    Update::Assign {
                        field: "current_run".into(),
                        expr: Expr::None,
                    },
                    Update::Assign {
                        field: "current_run_contributors".into(),
                        expr: Expr::SeqLiteral(vec![]),
                    },
                ],
                to: "Active".into(),
                emit: vec![
                    EffectEmit {
                        variant: "IngressNotice".into(),
                        fields: IndexMap::from([
                            ("kind".into(), Expr::String("RunCompleted".into())),
                            ("detail".into(), Expr::String("ContributorsConsumed".into())),
                        ]),
                    },
                    EffectEmit {
                        variant: "CompletionResolved".into(),
                        fields: IndexMap::from([
                            (
                                "input_id".into(),
                                Expr::Head(Box::new(Expr::Field(
                                    "current_run_contributors".into(),
                                ))),
                            ),
                            ("outcome".into(), Expr::String("Consumed".into())),
                        ]),
                    },
                ],
            },
            TransitionSchema {
                name: "RunFailed".into(),
                from: vec!["Active".into(), "Retired".into()],
                on: InputMatch {
                    variant: "RunFailed".into(),
                    bindings: vec!["run_id".into()],
                },
                guards: vec![
                    Guard {
                        name: "run_matches_current".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field("current_run".into())),
                            Box::new(Expr::Some(Box::new(Expr::Binding("run_id".into())))),
                        ),
                    },
                    Guard {
                        name: "contributors_are_staged".into(),
                        expr: Expr::Quantified {
                            quantifier: Quantifier::All,
                            binding: "input_id".into(),
                            over: Box::new(Expr::Field("current_run_contributors".into())),
                            body: Box::new(Expr::Eq(
                                Box::new(Expr::MapGet {
                                    map: Box::new(Expr::Field("lifecycle".into())),
                                    key: Box::new(Expr::Binding("input_id".into())),
                                }),
                                Box::new(Expr::String("Staged".into())),
                            )),
                        },
                    },
                ],
                updates: vec![
                    Update::ForEach {
                        binding: "input_id".into(),
                        over: Expr::Field("current_run_contributors".into()),
                        updates: vec![Update::MapInsert {
                            field: "lifecycle".into(),
                            key: Expr::Binding("input_id".into()),
                            value: Expr::String("Queued".into()),
                        }],
                    },
                    Update::Conditional {
                        condition: Expr::Gt(
                            Box::new(Expr::Len(Box::new(Expr::Field(
                                "current_run_contributors".into(),
                            )))),
                            Box::new(Expr::U64(0)),
                        ),
                        then_updates: vec![
                            Update::SeqPrepend {
                                field: "queue".into(),
                                values: Expr::Field("current_run_contributors".into()),
                            },
                            Update::Assign {
                                field: "wake_requested".into(),
                                expr: Expr::Bool(true),
                            },
                        ],
                        else_updates: vec![],
                    },
                    Update::Assign {
                        field: "current_run".into(),
                        expr: Expr::None,
                    },
                    Update::Assign {
                        field: "current_run_contributors".into(),
                        expr: Expr::SeqLiteral(vec![]),
                    },
                ],
                to: "Active".into(),
                emit: vec![EffectEmit {
                    variant: "IngressNotice".into(),
                    fields: IndexMap::from([
                        ("kind".into(), Expr::String("RunFailed".into())),
                        ("detail".into(), Expr::String("ContributorsRequeued".into())),
                    ]),
                }],
            },
            TransitionSchema {
                name: "RunCancelled".into(),
                from: vec!["Active".into(), "Retired".into()],
                on: InputMatch {
                    variant: "RunCancelled".into(),
                    bindings: vec!["run_id".into()],
                },
                guards: vec![
                    Guard {
                        name: "run_matches_current".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field("current_run".into())),
                            Box::new(Expr::Some(Box::new(Expr::Binding("run_id".into())))),
                        ),
                    },
                    Guard {
                        name: "contributors_are_staged".into(),
                        expr: Expr::Quantified {
                            quantifier: Quantifier::All,
                            binding: "input_id".into(),
                            over: Box::new(Expr::Field("current_run_contributors".into())),
                            body: Box::new(Expr::Eq(
                                Box::new(Expr::MapGet {
                                    map: Box::new(Expr::Field("lifecycle".into())),
                                    key: Box::new(Expr::Binding("input_id".into())),
                                }),
                                Box::new(Expr::String("Staged".into())),
                            )),
                        },
                    },
                ],
                updates: vec![
                    Update::ForEach {
                        binding: "input_id".into(),
                        over: Expr::Field("current_run_contributors".into()),
                        updates: vec![Update::MapInsert {
                            field: "lifecycle".into(),
                            key: Expr::Binding("input_id".into()),
                            value: Expr::String("Queued".into()),
                        }],
                    },
                    Update::Conditional {
                        condition: Expr::Gt(
                            Box::new(Expr::Len(Box::new(Expr::Field(
                                "current_run_contributors".into(),
                            )))),
                            Box::new(Expr::U64(0)),
                        ),
                        then_updates: vec![
                            Update::SeqPrepend {
                                field: "queue".into(),
                                values: Expr::Field("current_run_contributors".into()),
                            },
                            Update::Assign {
                                field: "wake_requested".into(),
                                expr: Expr::Bool(true),
                            },
                        ],
                        else_updates: vec![],
                    },
                    Update::Assign {
                        field: "current_run".into(),
                        expr: Expr::None,
                    },
                    Update::Assign {
                        field: "current_run_contributors".into(),
                        expr: Expr::SeqLiteral(vec![]),
                    },
                ],
                to: "Active".into(),
                emit: vec![EffectEmit {
                    variant: "IngressNotice".into(),
                    fields: IndexMap::from([
                        ("kind".into(), Expr::String("RunCancelled".into())),
                        ("detail".into(), Expr::String("ContributorsRequeued".into())),
                    ]),
                }],
            },
            TransitionSchema {
                name: "SupersedeQueuedInput".into(),
                from: vec!["Active".into()],
                on: InputMatch {
                    variant: "SupersedeQueuedInput".into(),
                    bindings: vec!["new_input_id".into(), "old_input_id".into()],
                },
                guards: vec![
                    Guard {
                        name: "new_input_is_admitted".into(),
                        expr: Expr::Contains {
                            collection: Box::new(Expr::Field("admitted_inputs".into())),
                            value: Box::new(Expr::Binding("new_input_id".into())),
                        },
                    },
                    Guard {
                        name: "old_input_is_queued".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::MapGet {
                                map: Box::new(Expr::Field("lifecycle".into())),
                                key: Box::new(Expr::Binding("old_input_id".into())),
                            }),
                            Box::new(Expr::String("Queued".into())),
                        ),
                    },
                ],
                updates: vec![
                    Update::SeqRemoveValue {
                        field: "queue".into(),
                        value: Expr::Binding("old_input_id".into()),
                    },
                    Update::MapInsert {
                        field: "lifecycle".into(),
                        key: Expr::Binding("old_input_id".into()),
                        value: Expr::String("Superseded".into()),
                    },
                    Update::MapInsert {
                        field: "terminal_outcome".into(),
                        key: Expr::Binding("old_input_id".into()),
                        value: Expr::Some(Box::new(Expr::String("Superseded".into()))),
                    },
                ],
                to: "Active".into(),
                emit: vec![EffectEmit {
                    variant: "IngressNotice".into(),
                    fields: IndexMap::from([
                        ("kind".into(), Expr::String("SupersedeQueuedInput".into())),
                        (
                            "detail".into(),
                            Expr::String("QueuedInputSuperseded".into()),
                        ),
                    ]),
                }],
            },
            TransitionSchema {
                name: "CoalesceQueuedInputs".into(),
                from: vec!["Active".into()],
                on: InputMatch {
                    variant: "CoalesceQueuedInputs".into(),
                    bindings: vec!["aggregate_input_id".into(), "source_input_ids".into()],
                },
                guards: vec![
                    Guard {
                        name: "aggregate_input_is_admitted".into(),
                        expr: Expr::Contains {
                            collection: Box::new(Expr::Field("admitted_inputs".into())),
                            value: Box::new(Expr::Binding("aggregate_input_id".into())),
                        },
                    },
                    Guard {
                        name: "sources_non_empty".into(),
                        expr: Expr::Gt(
                            Box::new(Expr::Len(Box::new(Expr::Binding(
                                "source_input_ids".into(),
                            )))),
                            Box::new(Expr::U64(0)),
                        ),
                    },
                    Guard {
                        name: "all_sources_are_queued".into(),
                        expr: Expr::Quantified {
                            quantifier: Quantifier::All,
                            binding: "input_id".into(),
                            over: Box::new(Expr::Binding("source_input_ids".into())),
                            body: Box::new(Expr::Eq(
                                Box::new(Expr::MapGet {
                                    map: Box::new(Expr::Field("lifecycle".into())),
                                    key: Box::new(Expr::Binding("input_id".into())),
                                }),
                                Box::new(Expr::String("Queued".into())),
                            )),
                        },
                    },
                ],
                updates: vec![
                    Update::SeqRemoveAll {
                        field: "queue".into(),
                        values: Expr::Binding("source_input_ids".into()),
                    },
                    Update::ForEach {
                        binding: "input_id".into(),
                        over: Expr::Binding("source_input_ids".into()),
                        updates: vec![
                            Update::MapInsert {
                                field: "lifecycle".into(),
                                key: Expr::Binding("input_id".into()),
                                value: Expr::String("Coalesced".into()),
                            },
                            Update::MapInsert {
                                field: "terminal_outcome".into(),
                                key: Expr::Binding("input_id".into()),
                                value: Expr::Some(Box::new(Expr::String("Coalesced".into()))),
                            },
                        ],
                    },
                ],
                to: "Active".into(),
                emit: vec![EffectEmit {
                    variant: "IngressNotice".into(),
                    fields: IndexMap::from([
                        ("kind".into(), Expr::String("CoalesceQueuedInputs".into())),
                        (
                            "detail".into(),
                            Expr::String("SourcesCoalescedIntoAggregate".into()),
                        ),
                    ]),
                }],
            },
            TransitionSchema {
                name: "Retire".into(),
                from: vec!["Active".into()],
                on: InputMatch {
                    variant: "Retire".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![],
                to: "Retired".into(),
                emit: vec![EffectEmit {
                    variant: "IngressNotice".into(),
                    fields: IndexMap::from([
                        ("kind".into(), Expr::String("Retire".into())),
                        ("detail".into(), Expr::String("QueuePreserved".into())),
                    ]),
                }],
            },
            TransitionSchema {
                name: "ResetFromActive".into(),
                from: vec!["Active".into()],
                on: InputMatch {
                    variant: "Reset".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![
                    Update::ForEach {
                        binding: "input_id".into(),
                        over: Expr::Field("queue".into()),
                        updates: vec![
                            Update::MapInsert {
                                field: "lifecycle".into(),
                                key: Expr::Binding("input_id".into()),
                                value: Expr::String("Abandoned".into()),
                            },
                            Update::MapInsert {
                                field: "terminal_outcome".into(),
                                key: Expr::Binding("input_id".into()),
                                value: Expr::Some(Box::new(Expr::String("AbandonedReset".into()))),
                            },
                        ],
                    },
                    Update::ForEach {
                        binding: "input_id".into(),
                        over: Expr::Field("current_run_contributors".into()),
                        updates: vec![
                            Update::MapInsert {
                                field: "lifecycle".into(),
                                key: Expr::Binding("input_id".into()),
                                value: Expr::String("Abandoned".into()),
                            },
                            Update::MapInsert {
                                field: "terminal_outcome".into(),
                                key: Expr::Binding("input_id".into()),
                                value: Expr::Some(Box::new(Expr::String("AbandonedReset".into()))),
                            },
                        ],
                    },
                    Update::Assign {
                        field: "queue".into(),
                        expr: Expr::SeqLiteral(vec![]),
                    },
                    Update::Assign {
                        field: "current_run".into(),
                        expr: Expr::None,
                    },
                    Update::Assign {
                        field: "current_run_contributors".into(),
                        expr: Expr::SeqLiteral(vec![]),
                    },
                    Update::Assign {
                        field: "wake_requested".into(),
                        expr: Expr::Bool(false),
                    },
                    Update::Assign {
                        field: "process_requested".into(),
                        expr: Expr::Bool(false),
                    },
                ],
                to: "Active".into(),
                emit: vec![EffectEmit {
                    variant: "IngressNotice".into(),
                    fields: IndexMap::from([
                        ("kind".into(), Expr::String("Reset".into())),
                        (
                            "detail".into(),
                            Expr::String("NonTerminalInputsAbandoned".into()),
                        ),
                    ]),
                }],
            },
            TransitionSchema {
                name: "ResetFromRetired".into(),
                from: vec!["Retired".into()],
                on: InputMatch {
                    variant: "Reset".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![
                    Update::ForEach {
                        binding: "input_id".into(),
                        over: Expr::Field("queue".into()),
                        updates: vec![
                            Update::MapInsert {
                                field: "lifecycle".into(),
                                key: Expr::Binding("input_id".into()),
                                value: Expr::String("Abandoned".into()),
                            },
                            Update::MapInsert {
                                field: "terminal_outcome".into(),
                                key: Expr::Binding("input_id".into()),
                                value: Expr::Some(Box::new(Expr::String("AbandonedReset".into()))),
                            },
                        ],
                    },
                    Update::ForEach {
                        binding: "input_id".into(),
                        over: Expr::Field("current_run_contributors".into()),
                        updates: vec![
                            Update::MapInsert {
                                field: "lifecycle".into(),
                                key: Expr::Binding("input_id".into()),
                                value: Expr::String("Abandoned".into()),
                            },
                            Update::MapInsert {
                                field: "terminal_outcome".into(),
                                key: Expr::Binding("input_id".into()),
                                value: Expr::Some(Box::new(Expr::String("AbandonedReset".into()))),
                            },
                        ],
                    },
                    Update::Assign {
                        field: "queue".into(),
                        expr: Expr::SeqLiteral(vec![]),
                    },
                    Update::Assign {
                        field: "current_run".into(),
                        expr: Expr::None,
                    },
                    Update::Assign {
                        field: "current_run_contributors".into(),
                        expr: Expr::SeqLiteral(vec![]),
                    },
                    Update::Assign {
                        field: "wake_requested".into(),
                        expr: Expr::Bool(false),
                    },
                    Update::Assign {
                        field: "process_requested".into(),
                        expr: Expr::Bool(false),
                    },
                ],
                to: "Active".into(),
                emit: vec![EffectEmit {
                    variant: "IngressNotice".into(),
                    fields: IndexMap::from([
                        ("kind".into(), Expr::String("Reset".into())),
                        (
                            "detail".into(),
                            Expr::String("NonTerminalInputsAbandoned".into()),
                        ),
                    ]),
                }],
            },
            TransitionSchema {
                name: "Destroy".into(),
                from: vec!["Active".into(), "Retired".into()],
                on: InputMatch {
                    variant: "Destroy".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![
                    Update::ForEach {
                        binding: "input_id".into(),
                        over: Expr::Field("queue".into()),
                        updates: vec![
                            Update::MapInsert {
                                field: "lifecycle".into(),
                                key: Expr::Binding("input_id".into()),
                                value: Expr::String("Abandoned".into()),
                            },
                            Update::MapInsert {
                                field: "terminal_outcome".into(),
                                key: Expr::Binding("input_id".into()),
                                value: Expr::Some(Box::new(Expr::String(
                                    "AbandonedDestroyed".into(),
                                ))),
                            },
                        ],
                    },
                    Update::ForEach {
                        binding: "input_id".into(),
                        over: Expr::Field("current_run_contributors".into()),
                        updates: vec![
                            Update::MapInsert {
                                field: "lifecycle".into(),
                                key: Expr::Binding("input_id".into()),
                                value: Expr::String("Abandoned".into()),
                            },
                            Update::MapInsert {
                                field: "terminal_outcome".into(),
                                key: Expr::Binding("input_id".into()),
                                value: Expr::Some(Box::new(Expr::String(
                                    "AbandonedDestroyed".into(),
                                ))),
                            },
                        ],
                    },
                    Update::Assign {
                        field: "queue".into(),
                        expr: Expr::SeqLiteral(vec![]),
                    },
                    Update::Assign {
                        field: "current_run".into(),
                        expr: Expr::None,
                    },
                    Update::Assign {
                        field: "current_run_contributors".into(),
                        expr: Expr::SeqLiteral(vec![]),
                    },
                    Update::Assign {
                        field: "wake_requested".into(),
                        expr: Expr::Bool(false),
                    },
                    Update::Assign {
                        field: "process_requested".into(),
                        expr: Expr::Bool(false),
                    },
                ],
                to: "Destroyed".into(),
                emit: vec![EffectEmit {
                    variant: "IngressNotice".into(),
                    fields: IndexMap::from([
                        ("kind".into(), Expr::String("Destroy".into())),
                        ("detail".into(), Expr::String("IngressDestroyed".into())),
                    ]),
                }],
            },
            TransitionSchema {
                name: "RecoverFromActive".into(),
                from: vec!["Active".into()],
                on: InputMatch {
                    variant: "Recover".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![
                    Update::ForEach {
                        binding: "input_id".into(),
                        over: Expr::Field("current_run_contributors".into()),
                        updates: vec![Update::MapInsert {
                            field: "lifecycle".into(),
                            key: Expr::Binding("input_id".into()),
                            value: Expr::String("Queued".into()),
                        }],
                    },
                    Update::Conditional {
                        condition: Expr::Gt(
                            Box::new(Expr::Len(Box::new(Expr::Field(
                                "current_run_contributors".into(),
                            )))),
                            Box::new(Expr::U64(0)),
                        ),
                        then_updates: vec![
                            Update::SeqPrepend {
                                field: "queue".into(),
                                values: Expr::Field("current_run_contributors".into()),
                            },
                            Update::Assign {
                                field: "wake_requested".into(),
                                expr: Expr::Bool(true),
                            },
                        ],
                        else_updates: vec![],
                    },
                    Update::Assign {
                        field: "current_run".into(),
                        expr: Expr::None,
                    },
                    Update::Assign {
                        field: "current_run_contributors".into(),
                        expr: Expr::SeqLiteral(vec![]),
                    },
                ],
                to: "Active".into(),
                emit: vec![EffectEmit {
                    variant: "IngressNotice".into(),
                    fields: IndexMap::from([
                        ("kind".into(), Expr::String("Recover".into())),
                        (
                            "detail".into(),
                            Expr::String("RecoveryAppliedToCurrentRun".into()),
                        ),
                    ]),
                }],
            },
            TransitionSchema {
                name: "RecoverFromRetired".into(),
                from: vec!["Retired".into()],
                on: InputMatch {
                    variant: "Recover".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![
                    Update::ForEach {
                        binding: "input_id".into(),
                        over: Expr::Field("current_run_contributors".into()),
                        updates: vec![Update::MapInsert {
                            field: "lifecycle".into(),
                            key: Expr::Binding("input_id".into()),
                            value: Expr::String("Queued".into()),
                        }],
                    },
                    Update::Conditional {
                        condition: Expr::Gt(
                            Box::new(Expr::Len(Box::new(Expr::Field(
                                "current_run_contributors".into(),
                            )))),
                            Box::new(Expr::U64(0)),
                        ),
                        then_updates: vec![
                            Update::SeqPrepend {
                                field: "queue".into(),
                                values: Expr::Field("current_run_contributors".into()),
                            },
                            Update::Assign {
                                field: "wake_requested".into(),
                                expr: Expr::Bool(true),
                            },
                        ],
                        else_updates: vec![],
                    },
                    Update::Assign {
                        field: "current_run".into(),
                        expr: Expr::None,
                    },
                    Update::Assign {
                        field: "current_run_contributors".into(),
                        expr: Expr::SeqLiteral(vec![]),
                    },
                ],
                to: "Retired".into(),
                emit: vec![EffectEmit {
                    variant: "IngressNotice".into(),
                    fields: IndexMap::from([
                        ("kind".into(), Expr::String("Recover".into())),
                        (
                            "detail".into(),
                            Expr::String("RecoveryAppliedToCurrentRun".into()),
                        ),
                    ]),
                }],
            },
        ],
    }
}

fn runtime_ingress_admit_queued_transition(
    name: &str,
    wake: bool,
    process: bool,
) -> TransitionSchema {
    let mut emit = vec![
        EffectEmit {
            variant: "IngressAccepted".into(),
            fields: IndexMap::from([("input_id".into(), Expr::Binding("input_id".into()))]),
        },
        EffectEmit {
            variant: "InputLifecycleNotice".into(),
            fields: IndexMap::from([
                ("input_id".into(), Expr::Binding("input_id".into())),
                ("new_state".into(), Expr::String("Queued".into())),
            ]),
        },
    ];
    if wake {
        emit.push(EffectEmit {
            variant: "WakeRuntime".into(),
            fields: IndexMap::new(),
        });
    }
    if process {
        emit.push(EffectEmit {
            variant: "RequestImmediateProcessing".into(),
            fields: IndexMap::new(),
        });
    }

    TransitionSchema {
        name: name.into(),
        from: vec!["Active".into()],
        on: InputMatch {
            variant: "AdmitQueued".into(),
            bindings: vec![
                "input_id".into(),
                "input_kind".into(),
                "policy".into(),
                "wake".into(),
                "process".into(),
            ],
        },
        guards: vec![
            Guard {
                name: "input_is_new".into(),
                expr: Expr::Not(Box::new(Expr::Contains {
                    collection: Box::new(Expr::Field("admitted_inputs".into())),
                    value: Box::new(Expr::Binding("input_id".into())),
                })),
            },
            Guard {
                name: if wake {
                    "wake_is_true".into()
                } else {
                    "wake_is_false".into()
                },
                expr: Expr::Eq(
                    Box::new(Expr::Binding("wake".into())),
                    Box::new(Expr::Bool(wake)),
                ),
            },
            Guard {
                name: if process {
                    "process_is_true".into()
                } else {
                    "process_is_false".into()
                },
                expr: Expr::Eq(
                    Box::new(Expr::Binding("process".into())),
                    Box::new(Expr::Bool(process)),
                ),
            },
        ],
        updates: vec![
            Update::SetInsert {
                field: "admitted_inputs".into(),
                value: Expr::Binding("input_id".into()),
            },
            Update::SeqAppend {
                field: "admission_order".into(),
                value: Expr::Binding("input_id".into()),
            },
            Update::MapInsert {
                field: "input_kind".into(),
                key: Expr::Binding("input_id".into()),
                value: Expr::Binding("input_kind".into()),
            },
            Update::MapInsert {
                field: "policy_snapshot".into(),
                key: Expr::Binding("input_id".into()),
                value: Expr::Binding("policy".into()),
            },
            Update::MapInsert {
                field: "lifecycle".into(),
                key: Expr::Binding("input_id".into()),
                value: Expr::String("Queued".into()),
            },
            Update::MapInsert {
                field: "terminal_outcome".into(),
                key: Expr::Binding("input_id".into()),
                value: Expr::None,
            },
            Update::MapInsert {
                field: "last_run".into(),
                key: Expr::Binding("input_id".into()),
                value: Expr::None,
            },
            Update::MapInsert {
                field: "last_boundary_sequence".into(),
                key: Expr::Binding("input_id".into()),
                value: Expr::None,
            },
            Update::SeqAppend {
                field: "queue".into(),
                value: Expr::Binding("input_id".into()),
            },
            Update::Assign {
                field: "wake_requested".into(),
                expr: Expr::Or(vec![
                    Expr::Field("wake_requested".into()),
                    Expr::Binding("wake".into()),
                ]),
            },
            Update::Assign {
                field: "process_requested".into(),
                expr: Expr::Or(vec![
                    Expr::Field("process_requested".into()),
                    Expr::Binding("process".into()),
                ]),
            },
        ],
        to: "Active".into(),
        emit,
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
