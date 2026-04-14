use crate::{
    EffectDisposition, EffectDispositionRule, EnumSchema, Expr, FieldInit, FieldSchema,
    HelperSchema, InitSchema, InputMatch, InvariantSchema, MachineSchema, RustBinding, StateSchema,
    TransitionSchema, TriggerKind, TypeRef, Update, VariantSchema,
};

pub fn occurrence_lifecycle_machine() -> MachineSchema {
    MachineSchema {
        machine: "OccurrenceLifecycleMachine".into(),
        version: 1,
        rust: RustBinding {
            crate_name: "meerkat-schedule".into(),
            module: "generated::occurrence_lifecycle".into(),
        },
        state: StateSchema {
            phase: EnumSchema {
                name: "OccurrenceLifecycleState".into(),
                variants: vec![
                    variant("Pending"),
                    variant("Claimed"),
                    variant("Dispatching"),
                    variant("AwaitingCompletion"),
                    variant("Completed"),
                    variant("Skipped"),
                    variant("Misfired"),
                    variant("Superseded"),
                    variant("DeliveryFailed"),
                ],
            },
            fields: vec![
                field("occurrence_id", TypeRef::Named("OccurrenceId".into())),
                field("schedule_id", TypeRef::Named("ScheduleId".into())),
                field("schedule_revision", TypeRef::U64),
                field("occurrence_ordinal", TypeRef::U64),
                field("target_binding_key", TypeRef::String),
                field("due_at_utc_ms", TypeRef::U64),
                field("claimed_by", TypeRef::Option(Box::new(TypeRef::String))),
                field(
                    "lease_expires_at_utc_ms",
                    TypeRef::Option(Box::new(TypeRef::U64)),
                ),
                field("claimed_at_utc_ms", TypeRef::Option(Box::new(TypeRef::U64))),
                field("claim_token", TypeRef::Option(Box::new(TypeRef::String))),
                field(
                    "delivery_correlation_id",
                    TypeRef::Option(Box::new(TypeRef::String)),
                ),
                field(
                    "last_receipt_stage",
                    TypeRef::Option(Box::new(TypeRef::Enum("DeliveryReceiptStage".into()))),
                ),
                field(
                    "failure_class",
                    TypeRef::Option(Box::new(TypeRef::Enum("OccurrenceFailureClass".into()))),
                ),
                field("failure_detail", TypeRef::Option(Box::new(TypeRef::String))),
                field(
                    "dispatched_at_utc_ms",
                    TypeRef::Option(Box::new(TypeRef::U64)),
                ),
                field(
                    "completed_at_utc_ms",
                    TypeRef::Option(Box::new(TypeRef::U64)),
                ),
                field("attempt_count", TypeRef::U64),
                field(
                    "superseded_by_revision",
                    TypeRef::Option(Box::new(TypeRef::U64)),
                ),
            ],
            init: InitSchema {
                phase: "Pending".into(),
                fields: vec![
                    init("occurrence_id", Expr::String("occurrence-0".into())),
                    init("schedule_id", Expr::String("schedule-0".into())),
                    init("schedule_revision", Expr::U64(1)),
                    init("occurrence_ordinal", Expr::U64(0)),
                    init("target_binding_key", Expr::String("target-0".into())),
                    init("due_at_utc_ms", Expr::U64(1)),
                    init("claimed_by", Expr::None),
                    init("lease_expires_at_utc_ms", Expr::None),
                    init("claimed_at_utc_ms", Expr::None),
                    init("claim_token", Expr::None),
                    init("delivery_correlation_id", Expr::None),
                    init("last_receipt_stage", Expr::None),
                    init("failure_class", Expr::None),
                    init("failure_detail", Expr::None),
                    init("dispatched_at_utc_ms", Expr::None),
                    init("completed_at_utc_ms", Expr::None),
                    init("attempt_count", Expr::U64(0)),
                    init("superseded_by_revision", Expr::None),
                ],
            },
            terminal_phases: vec![
                "Completed".into(),
                "Skipped".into(),
                "Misfired".into(),
                "Superseded".into(),
                "DeliveryFailed".into(),
            ],
        },
        inputs: EnumSchema {
            name: "OccurrenceLifecycleInput".into(),
            variants: vec![
                VariantSchema {
                    name: "Claim".into(),
                    fields: vec![
                        field("owner_id", TypeRef::String),
                        field("at_utc_ms", TypeRef::U64),
                        field("lease_expires_at_utc_ms", TypeRef::U64),
                        field("claim_token", TypeRef::String),
                    ],
                },
                VariantSchema {
                    name: "DispatchStarted".into(),
                    fields: vec![
                        field("correlation_id", TypeRef::Option(Box::new(TypeRef::String))),
                        field("at_utc_ms", TypeRef::U64),
                    ],
                },
                VariantSchema {
                    name: "AwaitCompletion".into(),
                    fields: vec![field("at_utc_ms", TypeRef::U64)],
                },
                VariantSchema {
                    name: "Complete".into(),
                    fields: vec![
                        field(
                            "receipt_stage",
                            TypeRef::Enum("DeliveryReceiptStage".into()),
                        ),
                        field("at_utc_ms", TypeRef::U64),
                    ],
                },
                VariantSchema {
                    name: "Skip".into(),
                    fields: vec![
                        field("detail", TypeRef::Option(Box::new(TypeRef::String))),
                        field(
                            "failure_class",
                            TypeRef::Option(Box::new(TypeRef::Enum(
                                "OccurrenceFailureClass".into(),
                            ))),
                        ),
                        field("at_utc_ms", TypeRef::U64),
                    ],
                },
                VariantSchema {
                    name: "Misfire".into(),
                    fields: vec![
                        field("detail", TypeRef::Option(Box::new(TypeRef::String))),
                        field(
                            "failure_class",
                            TypeRef::Option(Box::new(TypeRef::Enum(
                                "OccurrenceFailureClass".into(),
                            ))),
                        ),
                        field("at_utc_ms", TypeRef::U64),
                    ],
                },
                VariantSchema {
                    name: "Supersede".into(),
                    fields: vec![
                        field("superseded_by_revision", TypeRef::U64),
                        field("at_utc_ms", TypeRef::U64),
                    ],
                },
                VariantSchema {
                    name: "DeliveryFailed".into(),
                    fields: vec![
                        field(
                            "receipt_stage",
                            TypeRef::Option(Box::new(TypeRef::Enum("DeliveryReceiptStage".into()))),
                        ),
                        field(
                            "failure_class",
                            TypeRef::Enum("OccurrenceFailureClass".into()),
                        ),
                        field("detail", TypeRef::Option(Box::new(TypeRef::String))),
                        field("at_utc_ms", TypeRef::U64),
                    ],
                },
                VariantSchema {
                    name: "LeaseExpired".into(),
                    fields: vec![field("at_utc_ms", TypeRef::U64)],
                },
            ],
        },
        signals: EnumSchema {
            name: "OccurrenceLifecycleSignal".into(),
            variants: vec![],
        },
        effects: EnumSchema {
            name: "OccurrenceLifecycleEffect".into(),
            variants: vec![
                variant("Claimed"),
                variant("DispatchStarted"),
                variant("AwaitingCompletion"),
                variant("Completed"),
                variant("Skipped"),
                variant("Misfired"),
                variant("Superseded"),
                variant("DeliveryFailed"),
                variant("LeaseExpired"),
            ],
        },
        helpers: vec![HelperSchema {
            name: "is_live_claim_phase".into(),
            params: vec![field(
                "phase",
                TypeRef::Named("OccurrenceLifecycleState".into()),
            )],
            returns: TypeRef::Bool,
            body: Expr::Or(vec![
                Expr::Eq(
                    Box::new(Expr::Binding("phase".into())),
                    Box::new(Expr::Phase("Claimed".into())),
                ),
                Expr::Eq(
                    Box::new(Expr::Binding("phase".into())),
                    Box::new(Expr::Phase("Dispatching".into())),
                ),
                Expr::Eq(
                    Box::new(Expr::Binding("phase".into())),
                    Box::new(Expr::Phase("AwaitingCompletion".into())),
                ),
            ]),
        }],
        derived: vec![],
        invariants: vec![
            InvariantSchema {
                name: "live_claim_requires_owner".into(),
                expr: Expr::Or(vec![
                    Expr::Not(Box::new(Expr::Call {
                        helper: "is_live_claim_phase".into(),
                        args: vec![Expr::CurrentPhase],
                    })),
                    Expr::Neq(
                        Box::new(Expr::Field("claimed_by".into())),
                        Box::new(Expr::None),
                    ),
                ]),
            },
            InvariantSchema {
                name: "superseded_records_revision".into(),
                expr: Expr::Or(vec![
                    Expr::Neq(
                        Box::new(Expr::CurrentPhase),
                        Box::new(Expr::Phase("Superseded".into())),
                    ),
                    Expr::Neq(
                        Box::new(Expr::Field("superseded_by_revision".into())),
                        Box::new(Expr::None),
                    ),
                ]),
            },
            InvariantSchema {
                name: "delivery_failed_records_failure_class".into(),
                expr: Expr::Or(vec![
                    Expr::Neq(
                        Box::new(Expr::CurrentPhase),
                        Box::new(Expr::Phase("DeliveryFailed".into())),
                    ),
                    Expr::Neq(
                        Box::new(Expr::Field("failure_class".into())),
                        Box::new(Expr::None),
                    ),
                ]),
            },
        ],
        transitions: vec![
            TransitionSchema {
                name: "ClaimPending".into(),
                from: vec!["Pending".into()],
                on: InputMatch {
                    kind: TriggerKind::Input,
                    variant: "Claim".into(),
                    bindings: vec![
                        "owner_id".into(),
                        "at_utc_ms".into(),
                        "lease_expires_at_utc_ms".into(),
                        "claim_token".into(),
                    ],
                },
                guards: vec![],
                updates: vec![
                    Update::Assign {
                        field: "claimed_by".into(),
                        expr: Expr::Some(Box::new(Expr::Binding("owner_id".into()))),
                    },
                    Update::Assign {
                        field: "lease_expires_at_utc_ms".into(),
                        expr: Expr::Some(Box::new(Expr::Binding("lease_expires_at_utc_ms".into()))),
                    },
                    Update::Assign {
                        field: "claimed_at_utc_ms".into(),
                        expr: Expr::Some(Box::new(Expr::Binding("at_utc_ms".into()))),
                    },
                    Update::Assign {
                        field: "claim_token".into(),
                        expr: Expr::Some(Box::new(Expr::Binding("claim_token".into()))),
                    },
                    Update::Assign {
                        field: "delivery_correlation_id".into(),
                        expr: Expr::None,
                    },
                    Update::Assign {
                        field: "last_receipt_stage".into(),
                        expr: Expr::None,
                    },
                    Update::Assign {
                        field: "failure_class".into(),
                        expr: Expr::None,
                    },
                    Update::Assign {
                        field: "failure_detail".into(),
                        expr: Expr::None,
                    },
                    Update::Assign {
                        field: "dispatched_at_utc_ms".into(),
                        expr: Expr::None,
                    },
                    Update::Assign {
                        field: "completed_at_utc_ms".into(),
                        expr: Expr::None,
                    },
                    Update::Increment {
                        field: "attempt_count".into(),
                        amount: 1,
                    },
                ],
                to: "Claimed".into(),
                emit: vec![effect("Claimed")],
            },
            TransitionSchema {
                name: "DispatchStartedFromClaimed".into(),
                from: vec!["Claimed".into()],
                on: InputMatch {
                    kind: TriggerKind::Input,
                    variant: "DispatchStarted".into(),
                    bindings: vec!["correlation_id".into(), "at_utc_ms".into()],
                },
                guards: vec![],
                updates: vec![
                    Update::Assign {
                        field: "delivery_correlation_id".into(),
                        expr: Expr::Binding("correlation_id".into()),
                    },
                    Update::Assign {
                        field: "dispatched_at_utc_ms".into(),
                        expr: Expr::Some(Box::new(Expr::Binding("at_utc_ms".into()))),
                    },
                ],
                to: "Dispatching".into(),
                emit: vec![effect("DispatchStarted")],
            },
            TransitionSchema {
                name: "AwaitCompletionFromDispatching".into(),
                from: vec!["Dispatching".into()],
                on: InputMatch {
                    kind: TriggerKind::Input,
                    variant: "AwaitCompletion".into(),
                    bindings: vec!["at_utc_ms".into()],
                },
                guards: vec![],
                updates: vec![Update::Assign {
                    field: "dispatched_at_utc_ms".into(),
                    expr: Expr::Some(Box::new(Expr::Binding("at_utc_ms".into()))),
                }],
                to: "AwaitingCompletion".into(),
                emit: vec![effect("AwaitingCompletion")],
            },
            TransitionSchema {
                name: "CompleteFromDispatchingOrAwaiting".into(),
                from: vec!["Dispatching".into(), "AwaitingCompletion".into()],
                on: InputMatch {
                    kind: TriggerKind::Input,
                    variant: "Complete".into(),
                    bindings: vec!["receipt_stage".into(), "at_utc_ms".into()],
                },
                guards: vec![],
                updates: vec![
                    Update::Assign {
                        field: "last_receipt_stage".into(),
                        expr: Expr::Some(Box::new(Expr::Binding("receipt_stage".into()))),
                    },
                    Update::Assign {
                        field: "completed_at_utc_ms".into(),
                        expr: Expr::Some(Box::new(Expr::Binding("at_utc_ms".into()))),
                    },
                ],
                to: "Completed".into(),
                emit: vec![effect("Completed")],
            },
            terminal_transition(
                "SkipFromPendingOrLive",
                &["Pending", "Claimed", "Dispatching", "AwaitingCompletion"],
                "Skip",
                "Skipped",
            ),
            terminal_transition(
                "MisfireFromPendingOrLive",
                &["Pending", "Claimed", "Dispatching", "AwaitingCompletion"],
                "Misfire",
                "Misfired",
            ),
            TransitionSchema {
                name: "SupersedePendingOrLive".into(),
                from: vec![
                    "Pending".into(),
                    "Claimed".into(),
                    "Dispatching".into(),
                    "AwaitingCompletion".into(),
                ],
                on: InputMatch {
                    kind: TriggerKind::Input,
                    variant: "Supersede".into(),
                    bindings: vec!["superseded_by_revision".into(), "at_utc_ms".into()],
                },
                guards: vec![],
                updates: vec![
                    Update::Assign {
                        field: "superseded_by_revision".into(),
                        expr: Expr::Some(Box::new(Expr::Binding("superseded_by_revision".into()))),
                    },
                    Update::Assign {
                        field: "completed_at_utc_ms".into(),
                        expr: Expr::Some(Box::new(Expr::Binding("at_utc_ms".into()))),
                    },
                ],
                to: "Superseded".into(),
                emit: vec![effect("Superseded")],
            },
            TransitionSchema {
                name: "DeliveryFailedFromClaimedOrLive".into(),
                from: vec![
                    "Claimed".into(),
                    "Dispatching".into(),
                    "AwaitingCompletion".into(),
                ],
                on: InputMatch {
                    kind: TriggerKind::Input,
                    variant: "DeliveryFailed".into(),
                    bindings: vec![
                        "receipt_stage".into(),
                        "failure_class".into(),
                        "detail".into(),
                        "at_utc_ms".into(),
                    ],
                },
                guards: vec![],
                updates: vec![
                    Update::Assign {
                        field: "last_receipt_stage".into(),
                        expr: Expr::Binding("receipt_stage".into()),
                    },
                    Update::Assign {
                        field: "failure_class".into(),
                        expr: Expr::Some(Box::new(Expr::Binding("failure_class".into()))),
                    },
                    Update::Assign {
                        field: "failure_detail".into(),
                        expr: Expr::Binding("detail".into()),
                    },
                    Update::Assign {
                        field: "completed_at_utc_ms".into(),
                        expr: Expr::Some(Box::new(Expr::Binding("at_utc_ms".into()))),
                    },
                ],
                to: "DeliveryFailed".into(),
                emit: vec![effect("DeliveryFailed")],
            },
            lease_expired_transition("LeaseExpiredFromClaimed", "Claimed"),
            lease_expired_transition("LeaseExpiredFromDispatching", "Dispatching"),
            lease_expired_transition("LeaseExpiredFromAwaitingCompletion", "AwaitingCompletion"),
        ],
        effect_dispositions: vec![
            disposition("Claimed", EffectDisposition::External),
            disposition("DispatchStarted", EffectDisposition::External),
            disposition("AwaitingCompletion", EffectDisposition::External),
            disposition("Completed", EffectDisposition::External),
            disposition("Skipped", EffectDisposition::External),
            disposition("Misfired", EffectDisposition::External),
            disposition("Superseded", EffectDisposition::External),
            disposition("DeliveryFailed", EffectDisposition::External),
            disposition("LeaseExpired", EffectDisposition::External),
        ],
        ci_step_limit: None,
    }
}

fn terminal_transition(
    name: &str,
    from: &[&str],
    input_variant: &str,
    to: &str,
) -> TransitionSchema {
    TransitionSchema {
        name: name.into(),
        from: from.iter().map(|phase| (*phase).into()).collect(),
        on: InputMatch {
            kind: TriggerKind::Input,
            variant: input_variant.into(),
            bindings: vec!["detail".into(), "failure_class".into(), "at_utc_ms".into()],
        },
        guards: vec![],
        updates: vec![
            Update::Assign {
                field: "failure_detail".into(),
                expr: Expr::Binding("detail".into()),
            },
            Update::Assign {
                field: "failure_class".into(),
                expr: Expr::Binding("failure_class".into()),
            },
            Update::Assign {
                field: "completed_at_utc_ms".into(),
                expr: Expr::Some(Box::new(Expr::Binding("at_utc_ms".into()))),
            },
        ],
        to: to.into(),
        emit: vec![effect(to)],
    }
}

fn lease_expired_transition(name: &str, from: &str) -> TransitionSchema {
    TransitionSchema {
        name: name.into(),
        from: vec![from.into()],
        on: InputMatch {
            kind: TriggerKind::Input,
            variant: "LeaseExpired".into(),
            bindings: vec!["at_utc_ms".into()],
        },
        guards: vec![],
        updates: vec![
            Update::Assign {
                field: "claimed_by".into(),
                expr: Expr::None,
            },
            Update::Assign {
                field: "lease_expires_at_utc_ms".into(),
                expr: Expr::None,
            },
            Update::Assign {
                field: "claim_token".into(),
                expr: Expr::None,
            },
            Update::Assign {
                field: "delivery_correlation_id".into(),
                expr: Expr::None,
            },
            Update::Assign {
                field: "claimed_at_utc_ms".into(),
                expr: Expr::None,
            },
            Update::Assign {
                field: "dispatched_at_utc_ms".into(),
                expr: Expr::None,
            },
        ],
        to: "Pending".into(),
        emit: vec![effect("LeaseExpired")],
    }
}

fn disposition(name: &str, disposition: EffectDisposition) -> EffectDispositionRule {
    EffectDispositionRule {
        effect_variant: name.into(),
        disposition,
        handoff_protocol: None,
    }
}

fn effect(name: &str) -> crate::EffectEmit {
    crate::EffectEmit {
        variant: name.into(),
        fields: Default::default(),
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
