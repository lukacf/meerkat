use indexmap::IndexMap;

use crate::{
    EffectDisposition, EffectDispositionRule, EffectEmit, EnumSchema, Expr, FieldInit, FieldSchema,
    Guard, HelperSchema, InitSchema, InputMatch, InvariantSchema, MachineSchema, Quantifier,
    RustBinding, StateSchema, TransitionSchema, TypeRef, Update, VariantSchema,
};

pub fn ops_lifecycle_machine() -> MachineSchema {
    MachineSchema {
        machine: "OpsLifecycleMachine".into(),
        version: 4,
        rust: RustBinding {
            crate_name: "meerkat-runtime".into(),
            module: "generated::ops_lifecycle".into(),
        },
        state: StateSchema {
            phase: EnumSchema {
                name: "OpsLifecyclePhase".into(),
                variants: vec![variant("Active")],
            },
            fields: vec![
                field(
                    "known_operations",
                    TypeRef::Set(Box::new(TypeRef::Named("OperationId".into()))),
                ),
                field(
                    "operation_status",
                    TypeRef::Map(
                        Box::new(TypeRef::Named("OperationId".into())),
                        Box::new(TypeRef::Named("OperationStatus".into())),
                    ),
                ),
                field(
                    "operation_kind",
                    TypeRef::Map(
                        Box::new(TypeRef::Named("OperationId".into())),
                        Box::new(TypeRef::Named("OperationKind".into())),
                    ),
                ),
                field(
                    "peer_ready",
                    TypeRef::Map(
                        Box::new(TypeRef::Named("OperationId".into())),
                        Box::new(TypeRef::Bool),
                    ),
                ),
                field(
                    "progress_count",
                    TypeRef::Map(
                        Box::new(TypeRef::Named("OperationId".into())),
                        Box::new(TypeRef::U32),
                    ),
                ),
                field(
                    "watcher_count",
                    TypeRef::Map(
                        Box::new(TypeRef::Named("OperationId".into())),
                        Box::new(TypeRef::U32),
                    ),
                ),
                field(
                    "terminal_outcome",
                    TypeRef::Map(
                        Box::new(TypeRef::Named("OperationId".into())),
                        Box::new(TypeRef::Named("OperationTerminalOutcome".into())),
                    ),
                ),
                field(
                    "terminal_buffered",
                    TypeRef::Map(
                        Box::new(TypeRef::Named("OperationId".into())),
                        Box::new(TypeRef::Bool),
                    ),
                ),
                // Phase B: bounded completed-operation retention (FIFO ordering)
                field(
                    "completed_order",
                    TypeRef::Seq(Box::new(TypeRef::Named("OperationId".into()))),
                ),
                // Phase B: maximum completed operations to retain
                field("max_completed", TypeRef::U32),
                // Phase B: maximum concurrent non-terminal operations (0 = unlimited)
                field("max_concurrent", TypeRef::U32),
                // Phase B: count of currently non-terminal operations
                field("active_count", TypeRef::U32),
                // Phase B: timestamps — wall-clock epoch millis
                field(
                    "created_at_ms",
                    TypeRef::Map(
                        Box::new(TypeRef::Named("OperationId".into())),
                        Box::new(TypeRef::U64),
                    ),
                ),
                field(
                    "completed_at_ms",
                    TypeRef::Map(
                        Box::new(TypeRef::Named("OperationId".into())),
                        Box::new(TypeRef::U64),
                    ),
                ),
                field("wait_active", TypeRef::Bool),
                field("wait_request_id", TypeRef::Named("WaitRequestId".into())),
                field(
                    "wait_operation_ids",
                    TypeRef::Seq(Box::new(TypeRef::Named("OperationId".into()))),
                ),
            ],
            init: InitSchema {
                phase: "Active".into(),
                fields: vec![
                    init("known_operations", Expr::EmptySet),
                    init("operation_status", Expr::EmptyMap),
                    init("operation_kind", Expr::EmptyMap),
                    init("peer_ready", Expr::EmptyMap),
                    init("progress_count", Expr::EmptyMap),
                    init("watcher_count", Expr::EmptyMap),
                    init("terminal_outcome", Expr::EmptyMap),
                    init("terminal_buffered", Expr::EmptyMap),
                    init("completed_order", Expr::SeqLiteral(vec![])),
                    init("max_completed", Expr::U64(256)),
                    init("max_concurrent", Expr::U64(0)),
                    init("active_count", Expr::U64(0)),
                    init("created_at_ms", Expr::EmptyMap),
                    init("completed_at_ms", Expr::EmptyMap),
                    init("wait_active", Expr::Bool(false)),
                    init("wait_request_id", Expr::String("wait_request_none".into())),
                    init("wait_operation_ids", Expr::SeqLiteral(vec![])),
                ],
            },
            terminal_phases: vec![],
        },
        inputs: EnumSchema {
            name: "OpsLifecycleInput".into(),
            variants: vec![
                VariantSchema {
                    name: "RegisterOperation".into(),
                    fields: vec![
                        field("operation_id", TypeRef::Named("OperationId".into())),
                        field("operation_kind", TypeRef::Named("OperationKind".into())),
                    ],
                },
                VariantSchema {
                    name: "ProvisioningSucceeded".into(),
                    fields: vec![field("operation_id", TypeRef::Named("OperationId".into()))],
                },
                VariantSchema {
                    name: "ProvisioningFailed".into(),
                    fields: vec![field("operation_id", TypeRef::Named("OperationId".into()))],
                },
                VariantSchema {
                    name: "AbortProvisioning".into(),
                    fields: vec![field("operation_id", TypeRef::Named("OperationId".into()))],
                },
                VariantSchema {
                    name: "PeerReady".into(),
                    fields: vec![field("operation_id", TypeRef::Named("OperationId".into()))],
                },
                VariantSchema {
                    name: "RegisterWatcher".into(),
                    fields: vec![field("operation_id", TypeRef::Named("OperationId".into()))],
                },
                VariantSchema {
                    name: "ProgressReported".into(),
                    fields: vec![field("operation_id", TypeRef::Named("OperationId".into()))],
                },
                VariantSchema {
                    name: "CompleteOperation".into(),
                    fields: vec![field("operation_id", TypeRef::Named("OperationId".into()))],
                },
                VariantSchema {
                    name: "FailOperation".into(),
                    fields: vec![field("operation_id", TypeRef::Named("OperationId".into()))],
                },
                VariantSchema {
                    name: "CancelOperation".into(),
                    fields: vec![field("operation_id", TypeRef::Named("OperationId".into()))],
                },
                VariantSchema {
                    name: "RetireRequested".into(),
                    fields: vec![field("operation_id", TypeRef::Named("OperationId".into()))],
                },
                VariantSchema {
                    name: "RetireCompleted".into(),
                    fields: vec![field("operation_id", TypeRef::Named("OperationId".into()))],
                },
                VariantSchema {
                    name: "CollectTerminal".into(),
                    fields: vec![field("operation_id", TypeRef::Named("OperationId".into()))],
                },
                variant("OwnerTerminated"),
                // Phase C: authority-owned barrier wait lifecycle
                VariantSchema {
                    name: "BeginWaitAll".into(),
                    fields: vec![
                        field("wait_request_id", TypeRef::Named("WaitRequestId".into())),
                        field(
                            "operation_ids",
                            TypeRef::Seq(Box::new(TypeRef::Named("OperationId".into()))),
                        ),
                    ],
                },
                VariantSchema {
                    name: "CancelWaitAll".into(),
                    fields: vec![field(
                        "wait_request_id",
                        TypeRef::Named("WaitRequestId".into()),
                    )],
                },
                // Phase B: drain all completed operations
                variant("CollectCompleted"),
            ],
        },
        effects: EnumSchema {
            name: "OpsLifecycleEffect".into(),
            variants: vec![
                VariantSchema {
                    name: "SubmitOpEvent".into(),
                    fields: vec![
                        field("operation_id", TypeRef::Named("OperationId".into())),
                        field("event_kind", TypeRef::Named("OpEventKind".into())),
                    ],
                },
                VariantSchema {
                    name: "NotifyOpWatcher".into(),
                    fields: vec![
                        field("operation_id", TypeRef::Named("OperationId".into())),
                        field(
                            "terminal_outcome",
                            TypeRef::Named("OperationTerminalOutcome".into()),
                        ),
                    ],
                },
                VariantSchema {
                    name: "ExposeOperationPeer".into(),
                    fields: vec![field("operation_id", TypeRef::Named("OperationId".into()))],
                },
                VariantSchema {
                    name: "RetainTerminalRecord".into(),
                    fields: vec![
                        field("operation_id", TypeRef::Named("OperationId".into())),
                        field(
                            "terminal_outcome",
                            TypeRef::Named("OperationTerminalOutcome".into()),
                        ),
                    ],
                },
                // Phase B: evict oldest completed operation from retention
                VariantSchema {
                    name: "EvictCompletedRecord".into(),
                    fields: vec![field("operation_id", TypeRef::Named("OperationId".into()))],
                },
                // Phase B: signal that wait_all barrier is satisfied
                VariantSchema {
                    name: "WaitAllSatisfied".into(),
                    fields: vec![
                        field("wait_request_id", TypeRef::Named("WaitRequestId".into())),
                        field(
                            "operation_ids",
                            TypeRef::Seq(Box::new(TypeRef::Named("OperationId".into()))),
                        ),
                    ],
                },
                // Phase B: result of collect_completed drain
                VariantSchema {
                    name: "CollectCompletedResult".into(),
                    fields: vec![field(
                        "operation_ids",
                        TypeRef::Seq(Box::new(TypeRef::Named("OperationId".into()))),
                    )],
                },
                // Phase B: concurrency limit exceeded
                VariantSchema {
                    name: "ConcurrencyLimitExceeded".into(),
                    fields: vec![
                        field("operation_id", TypeRef::Named("OperationId".into())),
                        field("limit", TypeRef::U32),
                        field("active", TypeRef::U32),
                    ],
                },
            ],
        },
        helpers: vec![
            HelperSchema {
                name: "status_of".into(),
                params: vec![field("operation_id", TypeRef::Named("OperationId".into()))],
                returns: TypeRef::Named("OperationStatus".into()),
                body: Expr::IfElse {
                    condition: Box::new(Expr::Contains {
                        collection: Box::new(Expr::Field("known_operations".into())),
                        value: Box::new(Expr::Binding("operation_id".into())),
                    }),
                    then_expr: Box::new(Expr::MapGet {
                        map: Box::new(Expr::Field("operation_status".into())),
                        key: Box::new(Expr::Binding("operation_id".into())),
                    }),
                    else_expr: Box::new(Expr::String("Absent".into())),
                },
            },
            HelperSchema {
                name: "kind_of".into(),
                params: vec![field("operation_id", TypeRef::Named("OperationId".into()))],
                returns: TypeRef::Named("OperationKind".into()),
                body: Expr::IfElse {
                    condition: Box::new(Expr::Contains {
                        collection: Box::new(Expr::Field("known_operations".into())),
                        value: Box::new(Expr::Binding("operation_id".into())),
                    }),
                    then_expr: Box::new(Expr::MapGet {
                        map: Box::new(Expr::Field("operation_kind".into())),
                        key: Box::new(Expr::Binding("operation_id".into())),
                    }),
                    else_expr: Box::new(Expr::String("None".into())),
                },
            },
            HelperSchema {
                name: "wait_is_active".into(),
                params: vec![],
                returns: TypeRef::Bool,
                body: Expr::Field("wait_active".into()),
            },
            HelperSchema {
                name: "all_operations_known".into(),
                params: vec![field(
                    "operation_ids",
                    TypeRef::Seq(Box::new(TypeRef::Named("OperationId".into()))),
                )],
                returns: TypeRef::Bool,
                body: Expr::Quantified {
                    quantifier: Quantifier::All,
                    binding: "operation_id".into(),
                    over: Box::new(Expr::SeqElements(Box::new(Expr::Binding(
                        "operation_ids".into(),
                    )))),
                    body: Box::new(Expr::Contains {
                        collection: Box::new(Expr::Field("known_operations".into())),
                        value: Box::new(Expr::Binding("operation_id".into())),
                    }),
                },
            },
            HelperSchema {
                name: "all_operations_terminal".into(),
                params: vec![field(
                    "operation_ids",
                    TypeRef::Seq(Box::new(TypeRef::Named("OperationId".into()))),
                )],
                returns: TypeRef::Bool,
                body: Expr::Quantified {
                    quantifier: Quantifier::All,
                    binding: "operation_id".into(),
                    over: Box::new(Expr::SeqElements(Box::new(Expr::Binding(
                        "operation_ids".into(),
                    )))),
                    body: Box::new(Expr::Call {
                        helper: "is_terminal_status".into(),
                        args: vec![Expr::Call {
                            helper: "status_of".into(),
                            args: vec![Expr::Binding("operation_id".into())],
                        }],
                    }),
                },
            },
            HelperSchema {
                name: "wait_tracks_operation".into(),
                params: vec![field("operation_id", TypeRef::Named("OperationId".into()))],
                returns: TypeRef::Bool,
                body: Expr::Contains {
                    collection: Box::new(Expr::Field("wait_operation_ids".into())),
                    value: Box::new(Expr::Binding("operation_id".into())),
                },
            },
            HelperSchema {
                name: "wait_completes_on_terminal".into(),
                params: vec![field("operation_id", TypeRef::Named("OperationId".into()))],
                returns: TypeRef::Bool,
                body: Expr::And(vec![
                    Expr::Call {
                        helper: "wait_is_active".into(),
                        args: vec![],
                    },
                    Expr::Call {
                        helper: "wait_tracks_operation".into(),
                        args: vec![Expr::Binding("operation_id".into())],
                    },
                    Expr::Quantified {
                        quantifier: Quantifier::All,
                        binding: "tracked_operation_id".into(),
                        over: Box::new(Expr::SeqElements(Box::new(Expr::Field(
                            "wait_operation_ids".into(),
                        )))),
                        body: Box::new(Expr::Or(vec![
                            Expr::Eq(
                                Box::new(Expr::Binding("tracked_operation_id".into())),
                                Box::new(Expr::Binding("operation_id".into())),
                            ),
                            Expr::Call {
                                helper: "is_terminal_status".into(),
                                args: vec![Expr::Call {
                                    helper: "status_of".into(),
                                    args: vec![Expr::Binding("tracked_operation_id".into())],
                                }],
                            },
                        ])),
                    },
                ]),
            },
            HelperSchema {
                name: "peer_ready_of".into(),
                params: vec![field("operation_id", TypeRef::Named("OperationId".into()))],
                returns: TypeRef::Bool,
                body: Expr::IfElse {
                    condition: Box::new(Expr::Contains {
                        collection: Box::new(Expr::Field("known_operations".into())),
                        value: Box::new(Expr::Binding("operation_id".into())),
                    }),
                    then_expr: Box::new(Expr::MapGet {
                        map: Box::new(Expr::Field("peer_ready".into())),
                        key: Box::new(Expr::Binding("operation_id".into())),
                    }),
                    else_expr: Box::new(Expr::Bool(false)),
                },
            },
            HelperSchema {
                name: "watcher_count_of".into(),
                params: vec![field("operation_id", TypeRef::Named("OperationId".into()))],
                returns: TypeRef::U32,
                body: Expr::IfElse {
                    condition: Box::new(Expr::Contains {
                        collection: Box::new(Expr::Field("known_operations".into())),
                        value: Box::new(Expr::Binding("operation_id".into())),
                    }),
                    then_expr: Box::new(Expr::MapGet {
                        map: Box::new(Expr::Field("watcher_count".into())),
                        key: Box::new(Expr::Binding("operation_id".into())),
                    }),
                    else_expr: Box::new(Expr::U64(0)),
                },
            },
            HelperSchema {
                name: "progress_count_of".into(),
                params: vec![field("operation_id", TypeRef::Named("OperationId".into()))],
                returns: TypeRef::U32,
                body: Expr::IfElse {
                    condition: Box::new(Expr::Contains {
                        collection: Box::new(Expr::Field("known_operations".into())),
                        value: Box::new(Expr::Binding("operation_id".into())),
                    }),
                    then_expr: Box::new(Expr::MapGet {
                        map: Box::new(Expr::Field("progress_count".into())),
                        key: Box::new(Expr::Binding("operation_id".into())),
                    }),
                    else_expr: Box::new(Expr::U64(0)),
                },
            },
            HelperSchema {
                name: "terminal_outcome_of".into(),
                params: vec![field("operation_id", TypeRef::Named("OperationId".into()))],
                returns: TypeRef::Named("OperationTerminalOutcome".into()),
                body: Expr::IfElse {
                    condition: Box::new(Expr::Contains {
                        collection: Box::new(Expr::Field("known_operations".into())),
                        value: Box::new(Expr::Binding("operation_id".into())),
                    }),
                    then_expr: Box::new(Expr::MapGet {
                        map: Box::new(Expr::Field("terminal_outcome".into())),
                        key: Box::new(Expr::Binding("operation_id".into())),
                    }),
                    else_expr: Box::new(Expr::String("None".into())),
                },
            },
            HelperSchema {
                name: "terminal_buffered_of".into(),
                params: vec![field("operation_id", TypeRef::Named("OperationId".into()))],
                returns: TypeRef::Bool,
                body: Expr::IfElse {
                    condition: Box::new(Expr::Contains {
                        collection: Box::new(Expr::Field("known_operations".into())),
                        value: Box::new(Expr::Binding("operation_id".into())),
                    }),
                    then_expr: Box::new(Expr::MapGet {
                        map: Box::new(Expr::Field("terminal_buffered".into())),
                        key: Box::new(Expr::Binding("operation_id".into())),
                    }),
                    else_expr: Box::new(Expr::Bool(false)),
                },
            },
            HelperSchema {
                name: "is_terminal_status".into(),
                params: vec![field("status", TypeRef::Named("OperationStatus".into()))],
                returns: TypeRef::Bool,
                body: Expr::Or(vec![
                    Expr::Eq(
                        Box::new(Expr::Binding("status".into())),
                        Box::new(Expr::String("Completed".into())),
                    ),
                    Expr::Eq(
                        Box::new(Expr::Binding("status".into())),
                        Box::new(Expr::String("Failed".into())),
                    ),
                    Expr::Eq(
                        Box::new(Expr::Binding("status".into())),
                        Box::new(Expr::String("Aborted".into())),
                    ),
                    Expr::Eq(
                        Box::new(Expr::Binding("status".into())),
                        Box::new(Expr::String("Cancelled".into())),
                    ),
                    Expr::Eq(
                        Box::new(Expr::Binding("status".into())),
                        Box::new(Expr::String("Retired".into())),
                    ),
                    Expr::Eq(
                        Box::new(Expr::Binding("status".into())),
                        Box::new(Expr::String("Terminated".into())),
                    ),
                ]),
            },
            HelperSchema {
                name: "is_owner_terminatable_status".into(),
                params: vec![field("status", TypeRef::Named("OperationStatus".into()))],
                returns: TypeRef::Bool,
                body: Expr::Or(vec![
                    Expr::Eq(
                        Box::new(Expr::Binding("status".into())),
                        Box::new(Expr::String("Provisioning".into())),
                    ),
                    Expr::Eq(
                        Box::new(Expr::Binding("status".into())),
                        Box::new(Expr::String("Running".into())),
                    ),
                    Expr::Eq(
                        Box::new(Expr::Binding("status".into())),
                        Box::new(Expr::String("Retiring".into())),
                    ),
                ]),
            },
            HelperSchema {
                name: "terminal_outcome_matches_status".into(),
                params: vec![
                    field("status", TypeRef::Named("OperationStatus".into())),
                    field(
                        "terminal_outcome",
                        TypeRef::Named("OperationTerminalOutcome".into()),
                    ),
                ],
                returns: TypeRef::Bool,
                body: Expr::Or(vec![
                    Expr::And(vec![
                        Expr::Eq(
                            Box::new(Expr::Binding("status".into())),
                            Box::new(Expr::String("Completed".into())),
                        ),
                        Expr::Eq(
                            Box::new(Expr::Binding("terminal_outcome".into())),
                            Box::new(Expr::String("Completed".into())),
                        ),
                    ]),
                    Expr::And(vec![
                        Expr::Eq(
                            Box::new(Expr::Binding("status".into())),
                            Box::new(Expr::String("Failed".into())),
                        ),
                        Expr::Eq(
                            Box::new(Expr::Binding("terminal_outcome".into())),
                            Box::new(Expr::String("Failed".into())),
                        ),
                    ]),
                    Expr::And(vec![
                        Expr::Eq(
                            Box::new(Expr::Binding("status".into())),
                            Box::new(Expr::String("Aborted".into())),
                        ),
                        Expr::Eq(
                            Box::new(Expr::Binding("terminal_outcome".into())),
                            Box::new(Expr::String("Aborted".into())),
                        ),
                    ]),
                    Expr::And(vec![
                        Expr::Eq(
                            Box::new(Expr::Binding("status".into())),
                            Box::new(Expr::String("Cancelled".into())),
                        ),
                        Expr::Eq(
                            Box::new(Expr::Binding("terminal_outcome".into())),
                            Box::new(Expr::String("Cancelled".into())),
                        ),
                    ]),
                    Expr::And(vec![
                        Expr::Eq(
                            Box::new(Expr::Binding("status".into())),
                            Box::new(Expr::String("Retired".into())),
                        ),
                        Expr::Eq(
                            Box::new(Expr::Binding("terminal_outcome".into())),
                            Box::new(Expr::String("Retired".into())),
                        ),
                    ]),
                    Expr::And(vec![
                        Expr::Eq(
                            Box::new(Expr::Binding("status".into())),
                            Box::new(Expr::String("Terminated".into())),
                        ),
                        Expr::Eq(
                            Box::new(Expr::Binding("terminal_outcome".into())),
                            Box::new(Expr::String("Terminated".into())),
                        ),
                    ]),
                ]),
            },
        ],
        derived: vec![],
        invariants: vec![
            InvariantSchema {
                name: "terminal_buffered_only_for_terminal_states".into(),
                expr: Expr::Quantified {
                    quantifier: Quantifier::All,
                    binding: "operation_id".into(),
                    over: Box::new(Expr::Field("known_operations".into())),
                    body: Box::new(Expr::Or(vec![
                        Expr::Not(Box::new(Expr::Call {
                            helper: "terminal_buffered_of".into(),
                            args: vec![Expr::Binding("operation_id".into())],
                        })),
                        Expr::Call {
                            helper: "is_terminal_status".into(),
                            args: vec![Expr::Call {
                                helper: "status_of".into(),
                                args: vec![Expr::Binding("operation_id".into())],
                            }],
                        },
                    ])),
                },
            },
            InvariantSchema {
                name: "peer_ready_implies_mob_member_child".into(),
                expr: Expr::Quantified {
                    quantifier: Quantifier::All,
                    binding: "operation_id".into(),
                    over: Box::new(Expr::Field("known_operations".into())),
                    body: Box::new(Expr::Or(vec![
                        Expr::Not(Box::new(Expr::Call {
                            helper: "peer_ready_of".into(),
                            args: vec![Expr::Binding("operation_id".into())],
                        })),
                        Expr::Eq(
                            Box::new(Expr::Call {
                                helper: "kind_of".into(),
                                args: vec![Expr::Binding("operation_id".into())],
                            }),
                            Box::new(Expr::String("MobMemberChild".into())),
                        ),
                    ])),
                },
            },
            InvariantSchema {
                name: "peer_ready_implies_present".into(),
                expr: Expr::Quantified {
                    quantifier: Quantifier::All,
                    binding: "operation_id".into(),
                    over: Box::new(Expr::Field("known_operations".into())),
                    body: Box::new(Expr::Or(vec![
                        Expr::Not(Box::new(Expr::Call {
                            helper: "peer_ready_of".into(),
                            args: vec![Expr::Binding("operation_id".into())],
                        })),
                        Expr::Neq(
                            Box::new(Expr::Call {
                                helper: "status_of".into(),
                                args: vec![Expr::Binding("operation_id".into())],
                            }),
                            Box::new(Expr::String("Absent".into())),
                        ),
                    ])),
                },
            },
            InvariantSchema {
                name: "present_operations_keep_kind_identity".into(),
                expr: Expr::Quantified {
                    quantifier: Quantifier::All,
                    binding: "operation_id".into(),
                    over: Box::new(Expr::Field("known_operations".into())),
                    body: Box::new(Expr::And(vec![
                        Expr::Neq(
                            Box::new(Expr::Call {
                                helper: "status_of".into(),
                                args: vec![Expr::Binding("operation_id".into())],
                            }),
                            Box::new(Expr::String("Absent".into())),
                        ),
                        Expr::Neq(
                            Box::new(Expr::Call {
                                helper: "kind_of".into(),
                                args: vec![Expr::Binding("operation_id".into())],
                            }),
                            Box::new(Expr::String("None".into())),
                        ),
                    ])),
                },
            },
            InvariantSchema {
                name: "terminal_statuses_have_matching_terminal_outcome".into(),
                expr: Expr::Quantified {
                    quantifier: Quantifier::All,
                    binding: "operation_id".into(),
                    over: Box::new(Expr::Field("known_operations".into())),
                    body: Box::new(Expr::Or(vec![
                        Expr::Not(Box::new(Expr::Call {
                            helper: "is_terminal_status".into(),
                            args: vec![Expr::Call {
                                helper: "status_of".into(),
                                args: vec![Expr::Binding("operation_id".into())],
                            }],
                        })),
                        Expr::Call {
                            helper: "terminal_outcome_matches_status".into(),
                            args: vec![
                                Expr::Call {
                                    helper: "status_of".into(),
                                    args: vec![Expr::Binding("operation_id".into())],
                                },
                                Expr::Call {
                                    helper: "terminal_outcome_of".into(),
                                    args: vec![Expr::Binding("operation_id".into())],
                                },
                            ],
                        },
                    ])),
                },
            },
            InvariantSchema {
                name: "nonterminal_statuses_have_no_terminal_outcome".into(),
                expr: Expr::Quantified {
                    quantifier: Quantifier::All,
                    binding: "operation_id".into(),
                    over: Box::new(Expr::Field("known_operations".into())),
                    body: Box::new(Expr::Or(vec![
                        Expr::Call {
                            helper: "is_terminal_status".into(),
                            args: vec![Expr::Call {
                                helper: "status_of".into(),
                                args: vec![Expr::Binding("operation_id".into())],
                            }],
                        },
                        Expr::Eq(
                            Box::new(Expr::Call {
                                helper: "terminal_outcome_of".into(),
                                args: vec![Expr::Binding("operation_id".into())],
                            }),
                            Box::new(Expr::String("None".into())),
                        ),
                    ])),
                },
            },
        ],
        transitions: vec![
            TransitionSchema {
                name: "RegisterOperation".into(),
                from: vec!["Active".into()],
                on: InputMatch {
                    variant: "RegisterOperation".into(),
                    bindings: vec!["operation_id".into(), "operation_kind".into()],
                },
                guards: vec![
                    Guard {
                        name: "operation_absent".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Call {
                                helper: "status_of".into(),
                                args: vec![Expr::Binding("operation_id".into())],
                            }),
                            Box::new(Expr::String("Absent".into())),
                        ),
                    },
                    Guard {
                        name: "kind_is_real".into(),
                        expr: Expr::Neq(
                            Box::new(Expr::Binding("operation_kind".into())),
                            Box::new(Expr::String("None".into())),
                        ),
                    },
                    // Phase B: concurrency enforcement
                    Guard {
                        name: "concurrency_limit_allows".into(),
                        expr: Expr::Or(vec![
                            // 0 means unlimited
                            Expr::Eq(
                                Box::new(Expr::Field("max_concurrent".into())),
                                Box::new(Expr::U64(0)),
                            ),
                            Expr::Lt(
                                Box::new(Expr::Field("active_count".into())),
                                Box::new(Expr::Field("max_concurrent".into())),
                            ),
                        ]),
                    },
                ],
                updates: vec![
                    Update::SetInsert {
                        field: "known_operations".into(),
                        value: Expr::Binding("operation_id".into()),
                    },
                    Update::MapInsert {
                        field: "operation_status".into(),
                        key: Expr::Binding("operation_id".into()),
                        value: Expr::String("Provisioning".into()),
                    },
                    Update::MapInsert {
                        field: "operation_kind".into(),
                        key: Expr::Binding("operation_id".into()),
                        value: Expr::Binding("operation_kind".into()),
                    },
                    Update::MapInsert {
                        field: "peer_ready".into(),
                        key: Expr::Binding("operation_id".into()),
                        value: Expr::Bool(false),
                    },
                    Update::MapInsert {
                        field: "progress_count".into(),
                        key: Expr::Binding("operation_id".into()),
                        value: Expr::U64(0),
                    },
                    Update::MapInsert {
                        field: "watcher_count".into(),
                        key: Expr::Binding("operation_id".into()),
                        value: Expr::U64(0),
                    },
                    Update::MapInsert {
                        field: "terminal_outcome".into(),
                        key: Expr::Binding("operation_id".into()),
                        value: Expr::String("None".into()),
                    },
                    Update::MapInsert {
                        field: "terminal_buffered".into(),
                        key: Expr::Binding("operation_id".into()),
                        value: Expr::Bool(false),
                    },
                    // Phase B: record creation timestamp
                    Update::MapInsert {
                        field: "created_at_ms".into(),
                        key: Expr::Binding("operation_id".into()),
                        value: Expr::U64(0),
                    },
                    Update::MapInsert {
                        field: "completed_at_ms".into(),
                        key: Expr::Binding("operation_id".into()),
                        value: Expr::U64(0),
                    },
                    // Phase B: track active count
                    Update::Increment {
                        field: "active_count".into(),
                        amount: 1,
                    },
                ],
                to: "Active".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "ProvisioningSucceeded".into(),
                from: vec!["Active".into()],
                on: InputMatch {
                    variant: "ProvisioningSucceeded".into(),
                    bindings: vec!["operation_id".into()],
                },
                guards: vec![status_guard(
                    "is_provisioning",
                    "operation_id",
                    "Provisioning",
                )],
                updates: vec![Update::MapInsert {
                    field: "operation_status".into(),
                    key: Expr::Binding("operation_id".into()),
                    value: Expr::String("Running".into()),
                }],
                to: "Active".into(),
                emit: vec![submit_op_event("operation_id", "Started")],
            },
            terminal_transition_satisfies_wait(
                "ProvisioningFailedCompletesWait",
                "ProvisioningFailed",
                &["Provisioning"],
                "Failed",
                "Failed",
            ),
            terminal_transition(
                "ProvisioningFailed",
                "ProvisioningFailed",
                &["Provisioning"],
                "Failed",
                "Failed",
            ),
            terminal_transition_satisfies_wait(
                "AbortProvisioningCompletesWait",
                "AbortProvisioning",
                &["Provisioning"],
                "Aborted",
                "Aborted",
            ),
            terminal_transition(
                "AbortProvisioning",
                "AbortProvisioning",
                &["Provisioning"],
                "Aborted",
                "Aborted",
            ),
            TransitionSchema {
                name: "PeerReady".into(),
                from: vec!["Active".into()],
                on: InputMatch {
                    variant: "PeerReady".into(),
                    bindings: vec!["operation_id".into()],
                },
                guards: vec![
                    Guard {
                        name: "operation_running_or_retiring".into(),
                        expr: Expr::Or(vec![
                            Expr::Eq(
                                Box::new(Expr::Call {
                                    helper: "status_of".into(),
                                    args: vec![Expr::Binding("operation_id".into())],
                                }),
                                Box::new(Expr::String("Running".into())),
                            ),
                            Expr::Eq(
                                Box::new(Expr::Call {
                                    helper: "status_of".into(),
                                    args: vec![Expr::Binding("operation_id".into())],
                                }),
                                Box::new(Expr::String("Retiring".into())),
                            ),
                        ]),
                    },
                    Guard {
                        name: "operation_is_mob_member_child".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Call {
                                helper: "kind_of".into(),
                                args: vec![Expr::Binding("operation_id".into())],
                            }),
                            Box::new(Expr::String("MobMemberChild".into())),
                        ),
                    },
                    Guard {
                        name: "peer_not_already_ready".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Call {
                                helper: "peer_ready_of".into(),
                                args: vec![Expr::Binding("operation_id".into())],
                            }),
                            Box::new(Expr::Bool(false)),
                        ),
                    },
                ],
                updates: vec![Update::MapInsert {
                    field: "peer_ready".into(),
                    key: Expr::Binding("operation_id".into()),
                    value: Expr::Bool(true),
                }],
                to: "Active".into(),
                emit: vec![EffectEmit {
                    variant: "ExposeOperationPeer".into(),
                    fields: IndexMap::from([(
                        "operation_id".into(),
                        Expr::Binding("operation_id".into()),
                    )]),
                }],
            },
            TransitionSchema {
                name: "RegisterWatcher".into(),
                from: vec!["Active".into()],
                on: InputMatch {
                    variant: "RegisterWatcher".into(),
                    bindings: vec!["operation_id".into()],
                },
                guards: vec![Guard {
                    name: "operation_exists".into(),
                    expr: Expr::Neq(
                        Box::new(Expr::Call {
                            helper: "status_of".into(),
                            args: vec![Expr::Binding("operation_id".into())],
                        }),
                        Box::new(Expr::String("Absent".into())),
                    ),
                }],
                updates: vec![Update::MapInsert {
                    field: "watcher_count".into(),
                    key: Expr::Binding("operation_id".into()),
                    value: Expr::Add(
                        Box::new(Expr::Call {
                            helper: "watcher_count_of".into(),
                            args: vec![Expr::Binding("operation_id".into())],
                        }),
                        Box::new(Expr::U64(1)),
                    ),
                }],
                to: "Active".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "ProgressReported".into(),
                from: vec!["Active".into()],
                on: InputMatch {
                    variant: "ProgressReported".into(),
                    bindings: vec!["operation_id".into()],
                },
                guards: vec![Guard {
                    name: "operation_running_or_retiring".into(),
                    expr: Expr::Or(vec![
                        Expr::Eq(
                            Box::new(Expr::Call {
                                helper: "status_of".into(),
                                args: vec![Expr::Binding("operation_id".into())],
                            }),
                            Box::new(Expr::String("Running".into())),
                        ),
                        Expr::Eq(
                            Box::new(Expr::Call {
                                helper: "status_of".into(),
                                args: vec![Expr::Binding("operation_id".into())],
                            }),
                            Box::new(Expr::String("Retiring".into())),
                        ),
                    ]),
                }],
                updates: vec![Update::MapInsert {
                    field: "progress_count".into(),
                    key: Expr::Binding("operation_id".into()),
                    value: Expr::Add(
                        Box::new(Expr::Call {
                            helper: "progress_count_of".into(),
                            args: vec![Expr::Binding("operation_id".into())],
                        }),
                        Box::new(Expr::U64(1)),
                    ),
                }],
                to: "Active".into(),
                emit: vec![submit_op_event("operation_id", "Progress")],
            },
            terminal_transition_satisfies_wait(
                "CompleteOperationCompletesWait",
                "CompleteOperation",
                &["Running", "Retiring"],
                "Completed",
                "Completed",
            ),
            terminal_transition(
                "CompleteOperation",
                "CompleteOperation",
                &["Running", "Retiring"],
                "Completed",
                "Completed",
            ),
            terminal_transition_satisfies_wait(
                "FailOperationCompletesWait",
                "FailOperation",
                &["Provisioning", "Running", "Retiring"],
                "Failed",
                "Failed",
            ),
            terminal_transition(
                "FailOperation",
                "FailOperation",
                &["Provisioning", "Running", "Retiring"],
                "Failed",
                "Failed",
            ),
            terminal_transition_satisfies_wait(
                "CancelOperationCompletesWait",
                "CancelOperation",
                &["Provisioning", "Running", "Retiring"],
                "Cancelled",
                "Cancelled",
            ),
            terminal_transition(
                "CancelOperation",
                "CancelOperation",
                &["Provisioning", "Running", "Retiring"],
                "Cancelled",
                "Cancelled",
            ),
            TransitionSchema {
                name: "RetireRequested".into(),
                from: vec!["Active".into()],
                on: InputMatch {
                    variant: "RetireRequested".into(),
                    bindings: vec!["operation_id".into()],
                },
                guards: vec![status_guard("operation_running", "operation_id", "Running")],
                updates: vec![Update::MapInsert {
                    field: "operation_status".into(),
                    key: Expr::Binding("operation_id".into()),
                    value: Expr::String("Retiring".into()),
                }],
                to: "Active".into(),
                emit: vec![],
            },
            terminal_transition_satisfies_wait(
                "RetireCompletedCompletesWait",
                "RetireCompleted",
                &["Running", "Retiring"],
                "Retired",
                "Retired",
            ),
            terminal_transition(
                "RetireCompleted",
                "RetireCompleted",
                &["Running", "Retiring"],
                "Retired",
                "Retired",
            ),
            TransitionSchema {
                name: "CollectTerminal".into(),
                from: vec!["Active".into()],
                on: InputMatch {
                    variant: "CollectTerminal".into(),
                    bindings: vec!["operation_id".into()],
                },
                guards: vec![
                    Guard {
                        name: "operation_is_terminal".into(),
                        expr: Expr::Call {
                            helper: "is_terminal_status".into(),
                            args: vec![Expr::Call {
                                helper: "status_of".into(),
                                args: vec![Expr::Binding("operation_id".into())],
                            }],
                        },
                    },
                    Guard {
                        name: "terminal_is_buffered".into(),
                        expr: Expr::Call {
                            helper: "terminal_buffered_of".into(),
                            args: vec![Expr::Binding("operation_id".into())],
                        },
                    },
                ],
                updates: vec![Update::MapInsert {
                    field: "terminal_buffered".into(),
                    key: Expr::Binding("operation_id".into()),
                    value: Expr::Bool(false),
                }],
                to: "Active".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "OwnerTerminatedCompletesWait".into(),
                from: vec!["Active".into()],
                on: InputMatch {
                    variant: "OwnerTerminated".into(),
                    bindings: vec![],
                },
                guards: vec![Guard {
                    name: "wait_is_active".into(),
                    expr: Expr::Call {
                        helper: "wait_is_active".into(),
                        args: vec![],
                    },
                }],
                updates: vec![
                    Update::ForEach {
                        binding: "operation_id".into(),
                        over: Expr::Field("known_operations".into()),
                        updates: vec![Update::Conditional {
                            condition: Expr::Call {
                                helper: "is_owner_terminatable_status".into(),
                                args: vec![Expr::Call {
                                    helper: "status_of".into(),
                                    args: vec![Expr::Binding("operation_id".into())],
                                }],
                            },
                            then_updates: vec![
                                Update::MapInsert {
                                    field: "operation_status".into(),
                                    key: Expr::Binding("operation_id".into()),
                                    value: Expr::String("Terminated".into()),
                                },
                                Update::MapInsert {
                                    field: "terminal_outcome".into(),
                                    key: Expr::Binding("operation_id".into()),
                                    value: Expr::String("Terminated".into()),
                                },
                                Update::MapInsert {
                                    field: "terminal_buffered".into(),
                                    key: Expr::Binding("operation_id".into()),
                                    value: Expr::Bool(true),
                                },
                            ],
                            else_updates: vec![],
                        }],
                    },
                    Update::Assign {
                        field: "active_count".into(),
                        expr: Expr::U64(0),
                    },
                    Update::Assign {
                        field: "wait_active".into(),
                        expr: Expr::Bool(false),
                    },
                ],
                to: "Active".into(),
                emit: vec![EffectEmit {
                    variant: "WaitAllSatisfied".into(),
                    fields: IndexMap::from([
                        (
                            "wait_request_id".into(),
                            Expr::Field("wait_request_id".into()),
                        ),
                        (
                            "operation_ids".into(),
                            Expr::Field("wait_operation_ids".into()),
                        ),
                    ]),
                }],
            },
            TransitionSchema {
                name: "OwnerTerminated".into(),
                from: vec!["Active".into()],
                on: InputMatch {
                    variant: "OwnerTerminated".into(),
                    bindings: vec![],
                },
                guards: vec![Guard {
                    name: "wait_not_active".into(),
                    expr: Expr::Not(Box::new(Expr::Call {
                        helper: "wait_is_active".into(),
                        args: vec![],
                    })),
                }],
                updates: vec![
                    Update::ForEach {
                        binding: "operation_id".into(),
                        over: Expr::Field("known_operations".into()),
                        updates: vec![Update::Conditional {
                            condition: Expr::Call {
                                helper: "is_owner_terminatable_status".into(),
                                args: vec![Expr::Call {
                                    helper: "status_of".into(),
                                    args: vec![Expr::Binding("operation_id".into())],
                                }],
                            },
                            then_updates: vec![
                                Update::MapInsert {
                                    field: "operation_status".into(),
                                    key: Expr::Binding("operation_id".into()),
                                    value: Expr::String("Terminated".into()),
                                },
                                Update::MapInsert {
                                    field: "terminal_outcome".into(),
                                    key: Expr::Binding("operation_id".into()),
                                    value: Expr::String("Terminated".into()),
                                },
                                Update::MapInsert {
                                    field: "terminal_buffered".into(),
                                    key: Expr::Binding("operation_id".into()),
                                    value: Expr::Bool(true),
                                },
                            ],
                            else_updates: vec![],
                        }],
                    },
                    Update::Assign {
                        field: "active_count".into(),
                        expr: Expr::U64(0),
                    },
                ],
                to: "Active".into(),
                // Authority emits NotifyOpWatcher + RetainTerminalRecord per terminated op
                // inside the ForEach loop. Schema DSL cannot express per-iteration effects,
                // so these are documented here rather than in the emit list.
                emit: vec![],
            },
            TransitionSchema {
                name: "BeginWaitAllImmediate".into(),
                from: vec!["Active".into()],
                on: InputMatch {
                    variant: "BeginWaitAll".into(),
                    bindings: vec!["wait_request_id".into(), "operation_ids".into()],
                },
                guards: vec![
                    Guard {
                        name: "wait_not_already_active".into(),
                        expr: Expr::Not(Box::new(Expr::Call {
                            helper: "wait_is_active".into(),
                            args: vec![],
                        })),
                    },
                    Guard {
                        name: "all_wait_operations_known".into(),
                        expr: Expr::Call {
                            helper: "all_operations_known".into(),
                            args: vec![Expr::Binding("operation_ids".into())],
                        },
                    },
                    Guard {
                        name: "all_wait_operations_terminal".into(),
                        expr: Expr::Call {
                            helper: "all_operations_terminal".into(),
                            args: vec![Expr::Binding("operation_ids".into())],
                        },
                    },
                ],
                updates: vec![],
                to: "Active".into(),
                emit: vec![EffectEmit {
                    variant: "WaitAllSatisfied".into(),
                    fields: IndexMap::from([
                        (
                            "wait_request_id".into(),
                            Expr::Binding("wait_request_id".into()),
                        ),
                        (
                            "operation_ids".into(),
                            Expr::Binding("operation_ids".into()),
                        ),
                    ]),
                }],
            },
            TransitionSchema {
                name: "BeginWaitAllPending".into(),
                from: vec!["Active".into()],
                on: InputMatch {
                    variant: "BeginWaitAll".into(),
                    bindings: vec!["wait_request_id".into(), "operation_ids".into()],
                },
                guards: vec![
                    Guard {
                        name: "wait_not_already_active".into(),
                        expr: Expr::Not(Box::new(Expr::Call {
                            helper: "wait_is_active".into(),
                            args: vec![],
                        })),
                    },
                    Guard {
                        name: "all_wait_operations_known".into(),
                        expr: Expr::Call {
                            helper: "all_operations_known".into(),
                            args: vec![Expr::Binding("operation_ids".into())],
                        },
                    },
                    Guard {
                        name: "not_all_wait_operations_terminal".into(),
                        expr: Expr::Not(Box::new(Expr::Call {
                            helper: "all_operations_terminal".into(),
                            args: vec![Expr::Binding("operation_ids".into())],
                        })),
                    },
                ],
                updates: vec![
                    Update::Assign {
                        field: "wait_active".into(),
                        expr: Expr::Bool(true),
                    },
                    Update::Assign {
                        field: "wait_request_id".into(),
                        expr: Expr::Binding("wait_request_id".into()),
                    },
                    Update::Assign {
                        field: "wait_operation_ids".into(),
                        expr: Expr::Binding("operation_ids".into()),
                    },
                ],
                to: "Active".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "CancelWaitAll".into(),
                from: vec!["Active".into()],
                on: InputMatch {
                    variant: "CancelWaitAll".into(),
                    bindings: vec!["wait_request_id".into()],
                },
                guards: vec![
                    Guard {
                        name: "wait_is_active".into(),
                        expr: Expr::Call {
                            helper: "wait_is_active".into(),
                            args: vec![],
                        },
                    },
                    Guard {
                        name: "wait_request_matches".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field("wait_request_id".into())),
                            Box::new(Expr::Binding("wait_request_id".into())),
                        ),
                    },
                ],
                updates: vec![
                    Update::Assign {
                        field: "wait_active".into(),
                        expr: Expr::Bool(false),
                    },
                    Update::Assign {
                        field: "wait_request_id".into(),
                        expr: Expr::String("wait_request_none".into()),
                    },
                    Update::Assign {
                        field: "wait_operation_ids".into(),
                        expr: Expr::SeqLiteral(vec![]),
                    },
                ],
                to: "Active".into(),
                emit: vec![],
            },
            // Phase B: CollectCompleted — drain all buffered terminal operations
            TransitionSchema {
                name: "CollectCompleted".into(),
                from: vec!["Active".into()],
                on: InputMatch {
                    variant: "CollectCompleted".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![],
                to: "Active".into(),
                emit: vec![EffectEmit {
                    variant: "CollectCompletedResult".into(),
                    fields: IndexMap::from([(
                        "operation_ids".into(),
                        Expr::Field("completed_order".into()),
                    )]),
                }],
            },
        ],
        effect_dispositions: vec![
            disposition(
                "SubmitOpEvent",
                EffectDisposition::Routed {
                    consumer_machines: vec!["RuntimeControlMachine".into()],
                },
            ),
            disposition("NotifyOpWatcher", EffectDisposition::Local),
            disposition(
                "ExposeOperationPeer",
                EffectDisposition::Routed {
                    consumer_machines: vec!["PeerCommsMachine".into()],
                },
            ),
            disposition("RetainTerminalRecord", EffectDisposition::Local),
            disposition("EvictCompletedRecord", EffectDisposition::Local),
            EffectDispositionRule {
                effect_variant: "WaitAllSatisfied".into(),
                disposition: EffectDisposition::Local,
                handoff_protocol: Some("ops_barrier_satisfaction".into()),
            },
            disposition("CollectCompletedResult", EffectDisposition::Local),
            disposition("ConcurrencyLimitExceeded", EffectDisposition::External),
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

fn status_guard(name: &str, operation_binding: &str, expected: &str) -> Guard {
    Guard {
        name: name.into(),
        expr: Expr::Eq(
            Box::new(Expr::Call {
                helper: "status_of".into(),
                args: vec![Expr::Binding(operation_binding.into())],
            }),
            Box::new(Expr::String(expected.into())),
        ),
    }
}

fn submit_op_event(operation_binding: &str, event_kind: &str) -> EffectEmit {
    EffectEmit {
        variant: "SubmitOpEvent".into(),
        fields: IndexMap::from([
            (
                "operation_id".into(),
                Expr::Binding(operation_binding.into()),
            ),
            ("event_kind".into(), Expr::String(event_kind.into())),
        ]),
    }
}

fn notify_op_watcher(operation_binding: &str, terminal_outcome: &str) -> EffectEmit {
    EffectEmit {
        variant: "NotifyOpWatcher".into(),
        fields: IndexMap::from([
            (
                "operation_id".into(),
                Expr::Binding(operation_binding.into()),
            ),
            (
                "terminal_outcome".into(),
                Expr::String(terminal_outcome.into()),
            ),
        ]),
    }
}

fn retain_terminal_record(operation_binding: &str, terminal_outcome: &str) -> EffectEmit {
    EffectEmit {
        variant: "RetainTerminalRecord".into(),
        fields: IndexMap::from([
            (
                "operation_id".into(),
                Expr::Binding(operation_binding.into()),
            ),
            (
                "terminal_outcome".into(),
                Expr::String(terminal_outcome.into()),
            ),
        ]),
    }
}

fn terminal_transition(
    name: &str,
    input_variant: &str,
    from_statuses: &[&str],
    terminal_status: &str,
    event_kind: &str,
) -> TransitionSchema {
    let operation_id = Expr::Binding("operation_id".into());
    TransitionSchema {
        name: name.into(),
        from: vec!["Active".into()],
        on: InputMatch {
            variant: input_variant.into(),
            bindings: vec!["operation_id".into()],
        },
        guards: vec![
            Guard {
                name: "status_allows_terminalization".into(),
                expr: Expr::Or(
                    from_statuses
                        .iter()
                        .map(|status| {
                            Expr::Eq(
                                Box::new(Expr::Call {
                                    helper: "status_of".into(),
                                    args: vec![operation_id.clone()],
                                }),
                                Box::new(Expr::String((*status).into())),
                            )
                        })
                        .collect(),
                ),
            },
            Guard {
                name: "terminalization_does_not_complete_wait".into(),
                expr: Expr::Not(Box::new(Expr::Call {
                    helper: "wait_completes_on_terminal".into(),
                    args: vec![operation_id.clone()],
                })),
            },
        ],
        updates: vec![
            Update::MapInsert {
                field: "operation_status".into(),
                key: operation_id.clone(),
                value: Expr::String(terminal_status.into()),
            },
            Update::MapInsert {
                field: "terminal_outcome".into(),
                key: operation_id.clone(),
                value: Expr::String(terminal_status.into()),
            },
            Update::MapInsert {
                field: "terminal_buffered".into(),
                key: operation_id.clone(),
                value: Expr::Bool(true),
            },
            // Phase B: decrement active count on terminalization
            Update::Decrement {
                field: "active_count".into(),
                amount: 1,
            },
            // Phase B: append to completed ordering for bounded retention
            Update::SeqAppend {
                field: "completed_order".into(),
                value: operation_id,
            },
        ],
        to: "Active".into(),
        emit: vec![
            submit_op_event("operation_id", event_kind),
            notify_op_watcher("operation_id", terminal_status),
            retain_terminal_record("operation_id", terminal_status),
        ],
    }
}

fn terminal_transition_satisfies_wait(
    name: &str,
    input_variant: &str,
    from_statuses: &[&str],
    terminal_status: &str,
    event_kind: &str,
) -> TransitionSchema {
    let operation_id = Expr::Binding("operation_id".into());
    TransitionSchema {
        name: name.into(),
        from: vec!["Active".into()],
        on: InputMatch {
            variant: input_variant.into(),
            bindings: vec!["operation_id".into()],
        },
        guards: vec![
            Guard {
                name: "status_allows_terminalization".into(),
                expr: Expr::Or(
                    from_statuses
                        .iter()
                        .map(|status| {
                            Expr::Eq(
                                Box::new(Expr::Call {
                                    helper: "status_of".into(),
                                    args: vec![operation_id.clone()],
                                }),
                                Box::new(Expr::String((*status).into())),
                            )
                        })
                        .collect(),
                ),
            },
            Guard {
                name: "terminalization_satisfies_wait".into(),
                expr: Expr::Call {
                    helper: "wait_completes_on_terminal".into(),
                    args: vec![operation_id.clone()],
                },
            },
        ],
        updates: vec![
            Update::MapInsert {
                field: "operation_status".into(),
                key: operation_id.clone(),
                value: Expr::String(terminal_status.into()),
            },
            Update::MapInsert {
                field: "terminal_outcome".into(),
                key: operation_id.clone(),
                value: Expr::String(terminal_status.into()),
            },
            Update::MapInsert {
                field: "terminal_buffered".into(),
                key: operation_id.clone(),
                value: Expr::Bool(true),
            },
            Update::Decrement {
                field: "active_count".into(),
                amount: 1,
            },
            Update::SeqAppend {
                field: "completed_order".into(),
                value: operation_id,
            },
            Update::Assign {
                field: "wait_active".into(),
                expr: Expr::Bool(false),
            },
        ],
        to: "Active".into(),
        emit: vec![
            submit_op_event("operation_id", event_kind),
            notify_op_watcher("operation_id", terminal_status),
            retain_terminal_record("operation_id", terminal_status),
            EffectEmit {
                variant: "WaitAllSatisfied".into(),
                fields: IndexMap::from([
                    (
                        "wait_request_id".into(),
                        Expr::Field("wait_request_id".into()),
                    ),
                    (
                        "operation_ids".into(),
                        Expr::Field("wait_operation_ids".into()),
                    ),
                ]),
            },
        ],
    }
}
