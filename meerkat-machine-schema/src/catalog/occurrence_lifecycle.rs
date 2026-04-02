use indexmap::IndexMap;

use crate::{
    EffectDisposition, EffectDispositionRule, EffectEmit, EnumSchema, Expr, FieldInit, FieldSchema,
    Guard, HelperSchema, InitSchema, InputMatch, InvariantSchema, MachineSchema, RustBinding,
    StateSchema, TransitionSchema, TypeRef, Update, VariantSchema,
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
                field("lease_held", TypeRef::Bool),
                field("lease_owner", TypeRef::String),
                field("lease_expiry_utc_ms", TypeRef::U64),
                field(
                    "delivery_correlation_id",
                    TypeRef::Option(Box::new(TypeRef::String)),
                ),
                field(
                    "open_delivery_protocol",
                    TypeRef::Option(Box::new(TypeRef::Enum("OccurrenceDeliveryProtocol".into()))),
                ),
                field(
                    "last_receipt_stage",
                    TypeRef::Option(Box::new(TypeRef::Enum("DeliveryReceiptStage".into()))),
                ),
                field(
                    "failure_class",
                    TypeRef::Option(Box::new(TypeRef::Enum("OccurrenceFailureClass".into()))),
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
                    init("lease_held", Expr::Bool(false)),
                    init("lease_owner", Expr::String("".into())),
                    init("lease_expiry_utc_ms", Expr::U64(0)),
                    init("delivery_correlation_id", Expr::None),
                    init("open_delivery_protocol", Expr::None),
                    init(
                        "last_receipt_stage",
                        Expr::Some(Box::new(receipt_stage(ReceiptStageVariant::Planned))),
                    ),
                    init("failure_class", Expr::None),
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
                        field("claim_time_utc_ms", TypeRef::U64),
                        field("owner_id", TypeRef::String),
                        field("lease_expiry_utc_ms", TypeRef::U64),
                    ],
                },
                VariantSchema {
                    name: "StartRuntimeDispatch".into(),
                    fields: vec![field("delivery_correlation_id", TypeRef::String)],
                },
                VariantSchema {
                    name: "StartMobDispatch".into(),
                    fields: vec![field("delivery_correlation_id", TypeRef::String)],
                },
                VariantSchema {
                    name: "RuntimeAccepted".into(),
                    fields: vec![
                        field("occurrence_id", TypeRef::Named("OccurrenceId".into())),
                        field("attempt_count", TypeRef::U64),
                    ],
                },
                VariantSchema {
                    name: "MobAccepted".into(),
                    fields: vec![
                        field("occurrence_id", TypeRef::Named("OccurrenceId".into())),
                        field("attempt_count", TypeRef::U64),
                    ],
                },
                VariantSchema {
                    name: "RuntimeCompleted".into(),
                    fields: vec![
                        field("occurrence_id", TypeRef::Named("OccurrenceId".into())),
                        field("attempt_count", TypeRef::U64),
                    ],
                },
                VariantSchema {
                    name: "MobCompleted".into(),
                    fields: vec![
                        field("occurrence_id", TypeRef::Named("OccurrenceId".into())),
                        field("attempt_count", TypeRef::U64),
                    ],
                },
                VariantSchema {
                    name: "RuntimeSkipped".into(),
                    fields: vec![
                        field("occurrence_id", TypeRef::Named("OccurrenceId".into())),
                        field("attempt_count", TypeRef::U64),
                        field(
                            "failure_class",
                            TypeRef::Enum("OccurrenceFailureClass".into()),
                        ),
                    ],
                },
                VariantSchema {
                    name: "MobSkipped".into(),
                    fields: vec![
                        field("occurrence_id", TypeRef::Named("OccurrenceId".into())),
                        field("attempt_count", TypeRef::U64),
                        field(
                            "failure_class",
                            TypeRef::Enum("OccurrenceFailureClass".into()),
                        ),
                    ],
                },
                VariantSchema {
                    name: "RuntimeMisfired".into(),
                    fields: vec![
                        field("occurrence_id", TypeRef::Named("OccurrenceId".into())),
                        field("attempt_count", TypeRef::U64),
                        field(
                            "failure_class",
                            TypeRef::Enum("OccurrenceFailureClass".into()),
                        ),
                    ],
                },
                VariantSchema {
                    name: "MobMisfired".into(),
                    fields: vec![
                        field("occurrence_id", TypeRef::Named("OccurrenceId".into())),
                        field("attempt_count", TypeRef::U64),
                        field(
                            "failure_class",
                            TypeRef::Enum("OccurrenceFailureClass".into()),
                        ),
                    ],
                },
                VariantSchema {
                    name: "RuntimeDeliveryFailed".into(),
                    fields: vec![
                        field("occurrence_id", TypeRef::Named("OccurrenceId".into())),
                        field("attempt_count", TypeRef::U64),
                        field(
                            "failure_class",
                            TypeRef::Enum("OccurrenceFailureClass".into()),
                        ),
                    ],
                },
                VariantSchema {
                    name: "MobDeliveryFailed".into(),
                    fields: vec![
                        field("occurrence_id", TypeRef::Named("OccurrenceId".into())),
                        field("attempt_count", TypeRef::U64),
                        field(
                            "failure_class",
                            TypeRef::Enum("OccurrenceFailureClass".into()),
                        ),
                    ],
                },
                VariantSchema {
                    name: "SupersedeByRevision".into(),
                    fields: vec![field("superseding_revision", TypeRef::U64)],
                },
                variant("LeaseExpired"),
            ],
        },
        effects: EnumSchema {
            name: "OccurrenceLifecycleEffect".into(),
            variants: vec![
                VariantSchema {
                    name: "ClaimLeaseGranted".into(),
                    fields: vec![
                        field("occurrence_id", TypeRef::Named("OccurrenceId".into())),
                        field("owner_id", TypeRef::String),
                        field("lease_expiry_utc_ms", TypeRef::U64),
                        field("attempt_count", TypeRef::U64),
                    ],
                },
                VariantSchema {
                    name: "DispatchToRuntime".into(),
                    fields: vec![
                        field("occurrence_id", TypeRef::Named("OccurrenceId".into())),
                        field("schedule_id", TypeRef::Named("ScheduleId".into())),
                        field("schedule_revision", TypeRef::U64),
                        field("occurrence_ordinal", TypeRef::U64),
                        field("attempt_count", TypeRef::U64),
                        field("target_binding_key", TypeRef::String),
                        field(
                            "delivery_correlation_id",
                            TypeRef::Option(Box::new(TypeRef::String)),
                        ),
                    ],
                },
                VariantSchema {
                    name: "DispatchToMob".into(),
                    fields: vec![
                        field("occurrence_id", TypeRef::Named("OccurrenceId".into())),
                        field("schedule_id", TypeRef::Named("ScheduleId".into())),
                        field("schedule_revision", TypeRef::U64),
                        field("occurrence_ordinal", TypeRef::U64),
                        field("attempt_count", TypeRef::U64),
                        field("target_binding_key", TypeRef::String),
                        field(
                            "delivery_correlation_id",
                            TypeRef::Option(Box::new(TypeRef::String)),
                        ),
                    ],
                },
                VariantSchema {
                    name: "AppendReceipt".into(),
                    fields: vec![field("stage", TypeRef::Enum("DeliveryReceiptStage".into()))],
                },
                VariantSchema {
                    name: "EmitOccurrenceNotice".into(),
                    fields: vec![field(
                        "new_state",
                        TypeRef::Named("OccurrenceLifecycleState".into()),
                    )],
                },
            ],
        },
        helpers: vec![
            HelperSchema {
                name: "claimable_at".into(),
                params: vec![field("store_now_utc_ms", TypeRef::U64)],
                returns: TypeRef::Bool,
                body: Expr::And(vec![
                    Expr::Eq(
                        Box::new(Expr::CurrentPhase),
                        Box::new(Expr::Phase("Pending".into())),
                    ),
                    Expr::Lte(
                        Box::new(Expr::Field("due_at_utc_ms".into())),
                        Box::new(Expr::Binding("store_now_utc_ms".into())),
                    ),
                    Expr::Or(vec![
                        Expr::Not(Box::new(Expr::Field("lease_held".into()))),
                        Expr::Lte(
                            Box::new(Expr::Field("lease_expiry_utc_ms".into())),
                            Box::new(Expr::Binding("store_now_utc_ms".into())),
                        ),
                    ]),
                ]),
            },
            HelperSchema {
                name: "phase_is_terminal".into(),
                params: vec![field(
                    "phase",
                    TypeRef::Named("OccurrenceLifecycleState".into()),
                )],
                returns: TypeRef::Bool,
                body: Expr::Or(vec![
                    Expr::Eq(
                        Box::new(Expr::Binding("phase".into())),
                        Box::new(Expr::Phase("Completed".into())),
                    ),
                    Expr::Eq(
                        Box::new(Expr::Binding("phase".into())),
                        Box::new(Expr::Phase("Skipped".into())),
                    ),
                    Expr::Eq(
                        Box::new(Expr::Binding("phase".into())),
                        Box::new(Expr::Phase("Misfired".into())),
                    ),
                    Expr::Eq(
                        Box::new(Expr::Binding("phase".into())),
                        Box::new(Expr::Phase("Superseded".into())),
                    ),
                    Expr::Eq(
                        Box::new(Expr::Binding("phase".into())),
                        Box::new(Expr::Phase("DeliveryFailed".into())),
                    ),
                ]),
            },
        ],
        derived: vec![],
        invariants: vec![
            InvariantSchema {
                name: "terminal_has_no_open_delivery_protocol".into(),
                expr: Expr::Or(vec![
                    Expr::Not(Box::new(Expr::Call {
                        helper: "phase_is_terminal".into(),
                        args: vec![Expr::CurrentPhase],
                    })),
                    Expr::Eq(
                        Box::new(Expr::Field("open_delivery_protocol".into())),
                        Box::new(Expr::None),
                    ),
                ]),
            },
            InvariantSchema {
                name: "live_claim_requires_lease_holder".into(),
                expr: Expr::Or(vec![
                    Expr::And(vec![
                        Expr::Neq(
                            Box::new(Expr::CurrentPhase),
                            Box::new(Expr::Phase("Claimed".into())),
                        ),
                        Expr::Neq(
                            Box::new(Expr::CurrentPhase),
                            Box::new(Expr::Phase("Dispatching".into())),
                        ),
                        Expr::Neq(
                            Box::new(Expr::CurrentPhase),
                            Box::new(Expr::Phase("AwaitingCompletion".into())),
                        ),
                    ]),
                    Expr::Eq(
                        Box::new(Expr::Field("lease_held".into())),
                        Box::new(Expr::Bool(true)),
                    ),
                ]),
            },
            InvariantSchema {
                name: "awaiting_completion_requires_protocol".into(),
                expr: Expr::Or(vec![
                    Expr::Neq(
                        Box::new(Expr::CurrentPhase),
                        Box::new(Expr::Phase("AwaitingCompletion".into())),
                    ),
                    Expr::Neq(
                        Box::new(Expr::Field("open_delivery_protocol".into())),
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
                    variant: "Claim".into(),
                    bindings: vec![
                        "claim_time_utc_ms".into(),
                        "owner_id".into(),
                        "lease_expiry_utc_ms".into(),
                    ],
                },
                guards: vec![Guard {
                    name: "claimable_at_store_time".into(),
                    expr: Expr::Call {
                        helper: "claimable_at".into(),
                        args: vec![Expr::Binding("claim_time_utc_ms".into())],
                    },
                }],
                updates: vec![
                    Update::Assign {
                        field: "lease_held".into(),
                        expr: Expr::Bool(true),
                    },
                    Update::Assign {
                        field: "lease_owner".into(),
                        expr: Expr::Binding("owner_id".into()),
                    },
                    Update::Assign {
                        field: "lease_expiry_utc_ms".into(),
                        expr: Expr::Binding("lease_expiry_utc_ms".into()),
                    },
                    Update::Assign {
                        field: "delivery_correlation_id".into(),
                        expr: Expr::None,
                    },
                    Update::Assign {
                        field: "open_delivery_protocol".into(),
                        expr: Expr::None,
                    },
                    Update::Assign {
                        field: "failure_class".into(),
                        expr: Expr::None,
                    },
                    Update::Assign {
                        field: "superseded_by_revision".into(),
                        expr: Expr::None,
                    },
                    Update::Increment {
                        field: "attempt_count".into(),
                        amount: 1,
                    },
                    Update::Assign {
                        field: "last_receipt_stage".into(),
                        expr: Expr::Some(Box::new(receipt_stage(ReceiptStageVariant::Claimed))),
                    },
                ],
                to: "Claimed".into(),
                emit: vec![
                    EffectEmit {
                        variant: "ClaimLeaseGranted".into(),
                        fields: IndexMap::from([
                            ("occurrence_id".into(), Expr::Field("occurrence_id".into())),
                            ("owner_id".into(), Expr::Binding("owner_id".into())),
                            (
                                "lease_expiry_utc_ms".into(),
                                Expr::Binding("lease_expiry_utc_ms".into()),
                            ),
                            ("attempt_count".into(), Expr::Field("attempt_count".into())),
                        ]),
                    },
                    append_receipt(ReceiptStageVariant::Claimed),
                    occurrence_notice(),
                ],
            },
            dispatch_transition(
                "StartRuntimeDispatchFromClaimed",
                "StartRuntimeDispatch",
                "DispatchToRuntime",
                DeliveryProtocolVariant::Runtime,
            ),
            dispatch_transition(
                "StartMobDispatchFromClaimed",
                "StartMobDispatch",
                "DispatchToMob",
                DeliveryProtocolVariant::Mob,
            ),
            accepted_transition(
                "RuntimeAcceptedFromDispatching",
                "RuntimeAccepted",
                DeliveryProtocolVariant::Runtime,
            ),
            accepted_transition(
                "MobAcceptedFromDispatching",
                "MobAccepted",
                DeliveryProtocolVariant::Mob,
            ),
            terminal_transition(
                "RuntimeCompletedFromDispatchingOrAwaiting",
                &["Dispatching", "AwaitingCompletion"],
                "RuntimeCompleted",
                "Completed",
                DeliveryProtocolVariant::Runtime,
                None,
                ReceiptStageVariant::Completed,
            ),
            terminal_transition(
                "MobCompletedFromDispatchingOrAwaiting",
                &["Dispatching", "AwaitingCompletion"],
                "MobCompleted",
                "Completed",
                DeliveryProtocolVariant::Mob,
                None,
                ReceiptStageVariant::Completed,
            ),
            terminal_transition(
                "RuntimeSkippedFromDispatchingOrAwaiting",
                &["Dispatching", "AwaitingCompletion"],
                "RuntimeSkipped",
                "Skipped",
                DeliveryProtocolVariant::Runtime,
                Some("failure_class"),
                ReceiptStageVariant::Skipped,
            ),
            terminal_transition(
                "MobSkippedFromDispatchingOrAwaiting",
                &["Dispatching", "AwaitingCompletion"],
                "MobSkipped",
                "Skipped",
                DeliveryProtocolVariant::Mob,
                Some("failure_class"),
                ReceiptStageVariant::Skipped,
            ),
            terminal_transition(
                "RuntimeMisfiredFromDispatchingOrAwaiting",
                &["Dispatching", "AwaitingCompletion"],
                "RuntimeMisfired",
                "Misfired",
                DeliveryProtocolVariant::Runtime,
                Some("failure_class"),
                ReceiptStageVariant::Misfired,
            ),
            terminal_transition(
                "MobMisfiredFromDispatchingOrAwaiting",
                &["Dispatching", "AwaitingCompletion"],
                "MobMisfired",
                "Misfired",
                DeliveryProtocolVariant::Mob,
                Some("failure_class"),
                ReceiptStageVariant::Misfired,
            ),
            terminal_transition(
                "RuntimeDeliveryFailedFromDispatchingOrAwaiting",
                &["Dispatching", "AwaitingCompletion"],
                "RuntimeDeliveryFailed",
                "DeliveryFailed",
                DeliveryProtocolVariant::Runtime,
                Some("failure_class"),
                ReceiptStageVariant::DeliveryFailed,
            ),
            terminal_transition(
                "MobDeliveryFailedFromDispatchingOrAwaiting",
                &["Dispatching", "AwaitingCompletion"],
                "MobDeliveryFailed",
                "DeliveryFailed",
                DeliveryProtocolVariant::Mob,
                Some("failure_class"),
                ReceiptStageVariant::DeliveryFailed,
            ),
            TransitionSchema {
                name: "SupersedePending".into(),
                from: vec!["Pending".into()],
                on: InputMatch {
                    variant: "SupersedeByRevision".into(),
                    bindings: vec!["superseding_revision".into()],
                },
                guards: vec![Guard {
                    name: "superseding_revision_is_newer".into(),
                    expr: Expr::Gt(
                        Box::new(Expr::Binding("superseding_revision".into())),
                        Box::new(Expr::Field("schedule_revision".into())),
                    ),
                }],
                updates: vec![
                    Update::Assign {
                        field: "superseded_by_revision".into(),
                        expr: Expr::Some(Box::new(Expr::Binding("superseding_revision".into()))),
                    },
                    Update::Assign {
                        field: "last_receipt_stage".into(),
                        expr: Expr::Some(Box::new(receipt_stage(ReceiptStageVariant::Superseded))),
                    },
                    Update::Assign {
                        field: "open_delivery_protocol".into(),
                        expr: Expr::None,
                    },
                ],
                to: "Superseded".into(),
                emit: vec![
                    append_receipt(ReceiptStageVariant::Superseded),
                    occurrence_notice(),
                ],
            },
            lease_expired_transition("LeaseExpiredFromClaimed", "Claimed"),
            lease_expired_transition("LeaseExpiredFromDispatching", "Dispatching"),
            lease_expired_transition("LeaseExpiredFromAwaitingCompletion", "AwaitingCompletion"),
        ],
        effect_dispositions: vec![
            disposition("ClaimLeaseGranted", EffectDisposition::Local),
            EffectDispositionRule {
                effect_variant: "DispatchToRuntime".into(),
                disposition: EffectDisposition::Local,
                handoff_protocol: Some("occurrence_runtime_delivery".into()),
            },
            EffectDispositionRule {
                effect_variant: "DispatchToMob".into(),
                disposition: EffectDisposition::Local,
                handoff_protocol: Some("occurrence_mob_delivery".into()),
            },
            disposition("AppendReceipt", EffectDisposition::Local),
            disposition("EmitOccurrenceNotice", EffectDisposition::External),
        ],
        ci_step_limit: None,
    }
}

fn dispatch_transition(
    name: &str,
    input_variant: &str,
    effect_variant: &str,
    protocol: DeliveryProtocolVariant,
) -> TransitionSchema {
    TransitionSchema {
        name: name.into(),
        from: vec!["Claimed".into()],
        on: InputMatch {
            variant: input_variant.into(),
            bindings: vec!["delivery_correlation_id".into()],
        },
        guards: vec![],
        updates: vec![
            Update::Assign {
                field: "open_delivery_protocol".into(),
                expr: Expr::Some(Box::new(delivery_protocol(protocol))),
            },
            Update::Assign {
                field: "delivery_correlation_id".into(),
                expr: Expr::Some(Box::new(Expr::Binding("delivery_correlation_id".into()))),
            },
            Update::Assign {
                field: "last_receipt_stage".into(),
                expr: Expr::Some(Box::new(receipt_stage(
                    ReceiptStageVariant::DispatchStarted,
                ))),
            },
        ],
        to: "Dispatching".into(),
        emit: vec![
            EffectEmit {
                variant: effect_variant.into(),
                fields: IndexMap::from([
                    ("occurrence_id".into(), Expr::Field("occurrence_id".into())),
                    ("schedule_id".into(), Expr::Field("schedule_id".into())),
                    (
                        "schedule_revision".into(),
                        Expr::Field("schedule_revision".into()),
                    ),
                    (
                        "occurrence_ordinal".into(),
                        Expr::Field("occurrence_ordinal".into()),
                    ),
                    ("attempt_count".into(), Expr::Field("attempt_count".into())),
                    (
                        "target_binding_key".into(),
                        Expr::Field("target_binding_key".into()),
                    ),
                    (
                        "delivery_correlation_id".into(),
                        Expr::Field("delivery_correlation_id".into()),
                    ),
                ]),
            },
            append_receipt(ReceiptStageVariant::DispatchStarted),
            occurrence_notice(),
        ],
    }
}

fn accepted_transition(
    name: &str,
    input_variant: &str,
    protocol: DeliveryProtocolVariant,
) -> TransitionSchema {
    TransitionSchema {
        name: name.into(),
        from: vec!["Dispatching".into()],
        on: InputMatch {
            variant: input_variant.into(),
            bindings: vec!["occurrence_id".into(), "attempt_count".into()],
        },
        guards: correlation_guards(protocol),
        updates: vec![Update::Assign {
            field: "last_receipt_stage".into(),
            expr: Expr::Some(Box::new(receipt_stage(
                ReceiptStageVariant::AwaitingCompletion,
            ))),
        }],
        to: "AwaitingCompletion".into(),
        emit: vec![
            append_receipt(ReceiptStageVariant::AwaitingCompletion),
            occurrence_notice(),
        ],
    }
}

fn terminal_transition(
    name: &str,
    from: &[&str],
    input_variant: &str,
    to: &str,
    protocol: DeliveryProtocolVariant,
    failure_class_binding: Option<&str>,
    stage: ReceiptStageVariant,
) -> TransitionSchema {
    let mut bindings = vec!["occurrence_id".into(), "attempt_count".into()];
    if let Some(binding) = failure_class_binding {
        bindings.push(binding.into());
    }

    let mut updates = vec![
        Update::Assign {
            field: "lease_held".into(),
            expr: Expr::Bool(false),
        },
        Update::Assign {
            field: "lease_owner".into(),
            expr: Expr::String("".into()),
        },
        Update::Assign {
            field: "lease_expiry_utc_ms".into(),
            expr: Expr::U64(0),
        },
        Update::Assign {
            field: "open_delivery_protocol".into(),
            expr: Expr::None,
        },
        Update::Assign {
            field: "last_receipt_stage".into(),
            expr: Expr::Some(Box::new(receipt_stage(stage))),
        },
    ];

    updates.push(Update::Assign {
        field: "failure_class".into(),
        expr: failure_class_binding.map_or(Expr::None, |binding| {
            Expr::Some(Box::new(Expr::Binding(binding.into())))
        }),
    });

    TransitionSchema {
        name: name.into(),
        from: from.iter().map(|phase| (*phase).into()).collect(),
        on: InputMatch {
            variant: input_variant.into(),
            bindings,
        },
        guards: correlation_guards(protocol),
        updates,
        to: to.into(),
        emit: vec![append_receipt(stage), occurrence_notice()],
    }
}

fn lease_expired_transition(name: &str, from: &str) -> TransitionSchema {
    TransitionSchema {
        name: name.into(),
        from: vec![from.into()],
        on: InputMatch {
            variant: "LeaseExpired".into(),
            bindings: vec![],
        },
        guards: vec![Guard {
            name: "lease_was_held".into(),
            expr: Expr::Eq(
                Box::new(Expr::Field("lease_held".into())),
                Box::new(Expr::Bool(true)),
            ),
        }],
        updates: vec![
            Update::Assign {
                field: "lease_held".into(),
                expr: Expr::Bool(false),
            },
            Update::Assign {
                field: "lease_owner".into(),
                expr: Expr::String("".into()),
            },
            Update::Assign {
                field: "lease_expiry_utc_ms".into(),
                expr: Expr::U64(0),
            },
            Update::Assign {
                field: "delivery_correlation_id".into(),
                expr: Expr::None,
            },
            Update::Assign {
                field: "open_delivery_protocol".into(),
                expr: Expr::None,
            },
            Update::Assign {
                field: "failure_class".into(),
                expr: Expr::None,
            },
            Update::Assign {
                field: "last_receipt_stage".into(),
                expr: Expr::Some(Box::new(receipt_stage(ReceiptStageVariant::LeaseExpired))),
            },
        ],
        to: "Pending".into(),
        emit: vec![
            append_receipt(ReceiptStageVariant::LeaseExpired),
            occurrence_notice(),
        ],
    }
}

fn append_receipt(stage: ReceiptStageVariant) -> EffectEmit {
    EffectEmit {
        variant: "AppendReceipt".into(),
        fields: IndexMap::from([("stage".into(), receipt_stage(stage))]),
    }
}

fn occurrence_notice() -> EffectEmit {
    EffectEmit {
        variant: "EmitOccurrenceNotice".into(),
        fields: IndexMap::from([("new_state".into(), Expr::CurrentPhase)]),
    }
}

fn correlation_guards(protocol: DeliveryProtocolVariant) -> Vec<Guard> {
    vec![
        Guard {
            name: "matching_protocol_is_open".into(),
            expr: Expr::Eq(
                Box::new(Expr::Field("open_delivery_protocol".into())),
                Box::new(Expr::Some(Box::new(delivery_protocol(protocol)))),
            ),
        },
        Guard {
            name: "occurrence_identity_matches".into(),
            expr: Expr::Eq(
                Box::new(Expr::Field("occurrence_id".into())),
                Box::new(Expr::Binding("occurrence_id".into())),
            ),
        },
        Guard {
            name: "attempt_matches".into(),
            expr: Expr::Eq(
                Box::new(Expr::Field("attempt_count".into())),
                Box::new(Expr::Binding("attempt_count".into())),
            ),
        },
    ]
}

fn disposition(name: &str, disposition: EffectDisposition) -> EffectDispositionRule {
    EffectDispositionRule {
        effect_variant: name.into(),
        disposition,
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

#[derive(Clone, Copy)]
enum DeliveryProtocolVariant {
    Runtime,
    Mob,
}

fn delivery_protocol(variant: DeliveryProtocolVariant) -> Expr {
    Expr::NamedVariant {
        enum_name: "OccurrenceDeliveryProtocol".into(),
        variant: match variant {
            DeliveryProtocolVariant::Runtime => "Runtime".into(),
            DeliveryProtocolVariant::Mob => "Mob".into(),
        },
    }
}

#[derive(Clone, Copy)]
enum ReceiptStageVariant {
    Planned,
    Claimed,
    DispatchStarted,
    AwaitingCompletion,
    Completed,
    Skipped,
    Misfired,
    Superseded,
    DeliveryFailed,
    LeaseExpired,
}

fn receipt_stage(variant: ReceiptStageVariant) -> Expr {
    Expr::NamedVariant {
        enum_name: "DeliveryReceiptStage".into(),
        variant: match variant {
            ReceiptStageVariant::Planned => "Planned".into(),
            ReceiptStageVariant::Claimed => "Claimed".into(),
            ReceiptStageVariant::DispatchStarted => "DispatchStarted".into(),
            ReceiptStageVariant::AwaitingCompletion => "AwaitingCompletion".into(),
            ReceiptStageVariant::Completed => "Completed".into(),
            ReceiptStageVariant::Skipped => "Skipped".into(),
            ReceiptStageVariant::Misfired => "Misfired".into(),
            ReceiptStageVariant::Superseded => "Superseded".into(),
            ReceiptStageVariant::DeliveryFailed => "DeliveryFailed".into(),
            ReceiptStageVariant::LeaseExpired => "LeaseExpired".into(),
        },
    }
}
