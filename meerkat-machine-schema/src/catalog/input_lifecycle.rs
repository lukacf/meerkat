use indexmap::IndexMap;

use crate::{
    EffectDisposition, EffectDispositionRule, EffectEmit, EnumSchema, Expr, FieldInit, FieldSchema,
    Guard, InitSchema, InputMatch, InvariantSchema, MachineSchema, RustBinding, StateSchema,
    TransitionSchema, TypeRef, Update, VariantSchema,
};

pub fn input_lifecycle_machine() -> MachineSchema {
    MachineSchema {
        machine: "InputLifecycleMachine".into(),
        version: 1,
        rust: RustBinding {
            crate_name: "meerkat-runtime".into(),
            module: "generated::input_lifecycle".into(),
        },
        state: StateSchema {
            phase: EnumSchema {
                name: "InputLifecycleState".into(),
                variants: vec![
                    variant("Accepted"),
                    variant("Queued"),
                    variant("Staged"),
                    variant("Applied"),
                    variant("AppliedPendingConsumption"),
                    variant("Consumed"),
                    variant("Superseded"),
                    variant("Coalesced"),
                    variant("Abandoned"),
                ],
            },
            fields: vec![
                field(
                    "terminal_outcome",
                    TypeRef::Option(Box::new(TypeRef::Named("InputTerminalOutcome".into()))),
                ),
                field(
                    "last_run_id",
                    TypeRef::Option(Box::new(TypeRef::Named("RunId".into()))),
                ),
                field(
                    "last_boundary_sequence",
                    TypeRef::Option(Box::new(TypeRef::Named("BoundarySequence".into()))),
                ),
            ],
            init: InitSchema {
                phase: "Accepted".into(),
                fields: vec![
                    init("terminal_outcome", Expr::None),
                    init("last_run_id", Expr::None),
                    init("last_boundary_sequence", Expr::None),
                ],
            },
            terminal_phases: vec![
                "Consumed".into(),
                "Superseded".into(),
                "Coalesced".into(),
                "Abandoned".into(),
            ],
        },
        inputs: EnumSchema {
            name: "InputLifecycleInput".into(),
            variants: vec![
                variant("QueueAccepted"),
                VariantSchema {
                    name: "StageForRun".into(),
                    fields: vec![field("run_id", TypeRef::Named("RunId".into()))],
                },
                variant("RollbackStaged"),
                VariantSchema {
                    name: "MarkApplied".into(),
                    fields: vec![field("run_id", TypeRef::Named("RunId".into()))],
                },
                VariantSchema {
                    name: "MarkAppliedPendingConsumption".into(),
                    fields: vec![field(
                        "boundary_sequence",
                        TypeRef::Named("BoundarySequence".into()),
                    )],
                },
                variant("Consume"),
                variant("ConsumeOnAccept"),
                variant("Supersede"),
                variant("Coalesce"),
                variant("Abandon"),
            ],
        },
        effects: EnumSchema {
            name: "InputLifecycleEffect".into(),
            variants: vec![
                VariantSchema {
                    name: "InputLifecycleNotice".into(),
                    fields: vec![field(
                        "new_state",
                        TypeRef::Named("InputLifecycleState".into()),
                    )],
                },
                VariantSchema {
                    name: "RecordTerminalOutcome".into(),
                    fields: vec![field(
                        "outcome",
                        TypeRef::Named("InputTerminalOutcome".into()),
                    )],
                },
                VariantSchema {
                    name: "RecordRunAssociation".into(),
                    fields: vec![field("run_id", TypeRef::Named("RunId".into()))],
                },
                VariantSchema {
                    name: "RecordBoundarySequence".into(),
                    fields: vec![field(
                        "boundary_sequence",
                        TypeRef::Named("BoundarySequence".into()),
                    )],
                },
            ],
        },
        helpers: vec![],
        derived: vec![],
        invariants: vec![
            InvariantSchema {
                name: "accepted_has_no_run_or_boundary_metadata".into(),
                expr: Expr::Or(vec![
                    Expr::Neq(
                        Box::new(Expr::CurrentPhase),
                        Box::new(Expr::Phase("Accepted".into())),
                    ),
                    Expr::And(vec![
                        Expr::Eq(
                            Box::new(Expr::Field("last_run_id".into())),
                            Box::new(Expr::None),
                        ),
                        Expr::Eq(
                            Box::new(Expr::Field("last_boundary_sequence".into())),
                            Box::new(Expr::None),
                        ),
                    ]),
                ]),
            },
            InvariantSchema {
                name: "boundary_metadata_requires_application".into(),
                expr: Expr::Or(vec![
                    Expr::Eq(
                        Box::new(Expr::Field("last_boundary_sequence".into())),
                        Box::new(Expr::None),
                    ),
                    Expr::Or(vec![
                        Expr::Eq(
                            Box::new(Expr::CurrentPhase),
                            Box::new(Expr::Phase("AppliedPendingConsumption".into())),
                        ),
                        Expr::Eq(
                            Box::new(Expr::CurrentPhase),
                            Box::new(Expr::Phase("Consumed".into())),
                        ),
                        Expr::Eq(
                            Box::new(Expr::CurrentPhase),
                            Box::new(Expr::Phase("Abandoned".into())),
                        ),
                    ]),
                ]),
            },
        ],
        transitions: vec![
            transition(
                "QueueAccepted",
                &["Accepted"],
                "QueueAccepted",
                &[],
                &[notice("Queued")],
                "Queued",
            ),
            TransitionSchema {
                name: "StageForRun".into(),
                from: vec!["Queued".into()],
                on: InputMatch {
                    variant: "StageForRun".into(),
                    bindings: vec!["run_id".into()],
                },
                guards: vec![],
                updates: vec![Update::Assign {
                    field: "last_run_id".into(),
                    expr: Expr::Some(Box::new(Expr::Binding("run_id".into()))),
                }],
                to: "Staged".into(),
                emit: vec![
                    notice("Staged"),
                    EffectEmit {
                        variant: "RecordRunAssociation".into(),
                        fields: IndexMap::from([("run_id".into(), Expr::Binding("run_id".into()))]),
                    },
                ],
            },
            transition(
                "RollbackStaged",
                &["Staged"],
                "RollbackStaged",
                &[],
                &[notice("Queued")],
                "Queued",
            ),
            TransitionSchema {
                name: "MarkApplied".into(),
                from: vec!["Staged".into()],
                on: InputMatch {
                    variant: "MarkApplied".into(),
                    bindings: vec!["run_id".into()],
                },
                guards: vec![Guard {
                    name: "matches_last_run".into(),
                    expr: Expr::Eq(
                        Box::new(Expr::Field("last_run_id".into())),
                        Box::new(Expr::Some(Box::new(Expr::Binding("run_id".into())))),
                    ),
                }],
                updates: vec![],
                to: "Applied".into(),
                emit: vec![notice("Applied")],
            },
            TransitionSchema {
                name: "MarkAppliedPendingConsumption".into(),
                from: vec!["Applied".into()],
                on: InputMatch {
                    variant: "MarkAppliedPendingConsumption".into(),
                    bindings: vec!["boundary_sequence".into()],
                },
                guards: vec![],
                updates: vec![Update::Assign {
                    field: "last_boundary_sequence".into(),
                    expr: Expr::Some(Box::new(Expr::Binding("boundary_sequence".into()))),
                }],
                to: "AppliedPendingConsumption".into(),
                emit: vec![
                    notice("AppliedPendingConsumption"),
                    EffectEmit {
                        variant: "RecordBoundarySequence".into(),
                        fields: IndexMap::from([(
                            "boundary_sequence".into(),
                            Expr::Binding("boundary_sequence".into()),
                        )]),
                    },
                ],
            },
            TransitionSchema {
                name: "Consume".into(),
                from: vec!["AppliedPendingConsumption".into()],
                on: InputMatch {
                    variant: "Consume".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![Update::Assign {
                    field: "terminal_outcome".into(),
                    expr: Expr::Some(Box::new(Expr::String("Consumed".into()))),
                }],
                to: "Consumed".into(),
                emit: vec![
                    notice("Consumed"),
                    EffectEmit {
                        variant: "RecordTerminalOutcome".into(),
                        fields: IndexMap::from([(
                            "outcome".into(),
                            Expr::String("Consumed".into()),
                        )]),
                    },
                ],
            },
            TransitionSchema {
                name: "ConsumeOnAccept".into(),
                from: vec!["Accepted".into()],
                on: InputMatch {
                    variant: "ConsumeOnAccept".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![Update::Assign {
                    field: "terminal_outcome".into(),
                    expr: Expr::Some(Box::new(Expr::String("Consumed".into()))),
                }],
                to: "Consumed".into(),
                emit: vec![
                    notice("Consumed"),
                    EffectEmit {
                        variant: "RecordTerminalOutcome".into(),
                        fields: IndexMap::from([(
                            "outcome".into(),
                            Expr::String("Consumed".into()),
                        )]),
                    },
                ],
            },
            TransitionSchema {
                name: "Supersede".into(),
                from: vec!["Queued".into()],
                on: InputMatch {
                    variant: "Supersede".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![Update::Assign {
                    field: "terminal_outcome".into(),
                    expr: Expr::Some(Box::new(Expr::String("Superseded".into()))),
                }],
                to: "Superseded".into(),
                emit: vec![
                    notice("Superseded"),
                    EffectEmit {
                        variant: "RecordTerminalOutcome".into(),
                        fields: IndexMap::from([(
                            "outcome".into(),
                            Expr::String("Superseded".into()),
                        )]),
                    },
                ],
            },
            TransitionSchema {
                name: "Coalesce".into(),
                from: vec!["Queued".into()],
                on: InputMatch {
                    variant: "Coalesce".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![Update::Assign {
                    field: "terminal_outcome".into(),
                    expr: Expr::Some(Box::new(Expr::String("Coalesced".into()))),
                }],
                to: "Coalesced".into(),
                emit: vec![
                    notice("Coalesced"),
                    EffectEmit {
                        variant: "RecordTerminalOutcome".into(),
                        fields: IndexMap::from([(
                            "outcome".into(),
                            Expr::String("Coalesced".into()),
                        )]),
                    },
                ],
            },
            TransitionSchema {
                name: "Abandon".into(),
                from: vec![
                    "Accepted".into(),
                    "Queued".into(),
                    "Staged".into(),
                    "Applied".into(),
                    "AppliedPendingConsumption".into(),
                ],
                on: InputMatch {
                    variant: "Abandon".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![Update::Assign {
                    field: "terminal_outcome".into(),
                    expr: Expr::Some(Box::new(Expr::String("Abandoned".into()))),
                }],
                to: "Abandoned".into(),
                emit: vec![
                    notice("Abandoned"),
                    EffectEmit {
                        variant: "RecordTerminalOutcome".into(),
                        fields: IndexMap::from([(
                            "outcome".into(),
                            Expr::String("Abandoned".into()),
                        )]),
                    },
                ],
            },
        ],
        effect_dispositions: vec![
            disposition("InputLifecycleNotice", EffectDisposition::Local),
            disposition("RecordTerminalOutcome", EffectDisposition::Local),
            disposition("RecordRunAssociation", EffectDisposition::Local),
            disposition("RecordBoundarySequence", EffectDisposition::Local),
        ],
    }
}

fn disposition(name: &str, d: EffectDisposition) -> EffectDispositionRule {
    EffectDispositionRule {
        effect_variant: name.into(),
        disposition: d,
    }
}

fn transition(
    name: &str,
    from: &[&str],
    on: &str,
    bindings: &[&str],
    emit: &[EffectEmit],
    to: &str,
) -> TransitionSchema {
    TransitionSchema {
        name: name.into(),
        from: from.iter().map(|phase| (*phase).into()).collect(),
        on: InputMatch {
            variant: on.into(),
            bindings: bindings.iter().map(|binding| (*binding).into()).collect(),
        },
        guards: vec![],
        updates: vec![],
        to: to.into(),
        emit: emit.to_vec(),
    }
}

fn notice(new_state: &str) -> EffectEmit {
    EffectEmit {
        variant: "InputLifecycleNotice".into(),
        fields: IndexMap::from([("new_state".into(), Expr::String(new_state.into()))]),
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
