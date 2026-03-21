use indexmap::IndexMap;

use crate::{
    EffectDisposition, EffectDispositionRule, EffectEmit, EnumSchema, Expr, FieldInit, FieldSchema,
    InitSchema, InputMatch, InvariantSchema, MachineSchema, Quantifier, RustBinding, StateSchema,
    TransitionSchema, TypeRef, Update, VariantSchema,
};

/// Observation anchor for helper-facing terminal outcome classes in `mob_bundle`.
///
/// This machine is not yet the canonical helper result classifier used by the
/// live mob runtime. It records which terminal outcome classes the composition
/// has observed for mob-originated runs so helper-surface alignment is at
/// least explicit in the formal model. It should never be mistaken for the
/// canonical helper-result owner; it merely records observations until the
/// live ownership schemas are finalized.
pub fn mob_helper_result_anchor_machine() -> MachineSchema {
    MachineSchema {
        machine: "MobHelperResultAnchorMachine".into(),
        version: 1,
        rust: RustBinding {
            crate_name: "meerkat-mob".into(),
            module: "generated::mob_helper_result_anchor".into(),
        },
        state: StateSchema {
            phase: EnumSchema {
                name: "MobHelperResultAnchorState".into(),
                variants: vec![variant("Tracking")],
            },
            fields: vec![
                field(
                    "observed_completed_runs",
                    TypeRef::Set(Box::new(TypeRef::Named("RunId".into()))),
                ),
                field(
                    "observed_failed_runs",
                    TypeRef::Set(Box::new(TypeRef::Named("RunId".into()))),
                ),
                field(
                    "observed_cancelled_runs",
                    TypeRef::Set(Box::new(TypeRef::Named("RunId".into()))),
                ),
                field("force_cancel_count", TypeRef::U32),
            ],
            init: InitSchema {
                phase: "Tracking".into(),
                fields: vec![
                    init("observed_completed_runs", Expr::EmptySet),
                    init("observed_failed_runs", Expr::EmptySet),
                    init("observed_cancelled_runs", Expr::EmptySet),
                    init("force_cancel_count", Expr::U64(0)),
                ],
            },
            terminal_phases: vec![],
        },
        inputs: EnumSchema {
            name: "MobHelperResultAnchorInput".into(),
            variants: vec![
                VariantSchema {
                    name: "AnchorCompleted".into(),
                    fields: vec![field("run_id", TypeRef::Named("RunId".into()))],
                },
                VariantSchema {
                    name: "AnchorFailed".into(),
                    fields: vec![field("run_id", TypeRef::Named("RunId".into()))],
                },
                VariantSchema {
                    name: "AnchorCancelled".into(),
                    fields: vec![field("run_id", TypeRef::Named("RunId".into()))],
                },
                variant("AnchorForceCancelled"),
            ],
        },
        effects: EnumSchema {
            name: "MobHelperResultAnchorEffect".into(),
            variants: vec![variant("HelperResultSnapshotUpdated")],
        },
        helpers: vec![],
        derived: vec![],
        invariants: vec![
            InvariantSchema {
                name: "completed_not_failed".into(),
                expr: Expr::Quantified {
                    quantifier: Quantifier::All,
                    binding: "run_id".into(),
                    over: Box::new(Expr::Field("observed_completed_runs".into())),
                    body: Box::new(Expr::Not(Box::new(Expr::Contains {
                        collection: Box::new(Expr::Field("observed_failed_runs".into())),
                        value: Box::new(Expr::Binding("run_id".into())),
                    }))),
                },
            },
            InvariantSchema {
                name: "completed_not_cancelled".into(),
                expr: Expr::Quantified {
                    quantifier: Quantifier::All,
                    binding: "run_id".into(),
                    over: Box::new(Expr::Field("observed_completed_runs".into())),
                    body: Box::new(Expr::Not(Box::new(Expr::Contains {
                        collection: Box::new(Expr::Field("observed_cancelled_runs".into())),
                        value: Box::new(Expr::Binding("run_id".into())),
                    }))),
                },
            },
            InvariantSchema {
                name: "failed_not_cancelled".into(),
                expr: Expr::Quantified {
                    quantifier: Quantifier::All,
                    binding: "run_id".into(),
                    over: Box::new(Expr::Field("observed_failed_runs".into())),
                    body: Box::new(Expr::Not(Box::new(Expr::Contains {
                        collection: Box::new(Expr::Field("observed_cancelled_runs".into())),
                        value: Box::new(Expr::Binding("run_id".into())),
                    }))),
                },
            },
        ],
        transitions: vec![
            TransitionSchema {
                name: "AnchorCompleted".into(),
                from: vec!["Tracking".into()],
                on: InputMatch {
                    variant: "AnchorCompleted".into(),
                    bindings: vec!["run_id".into()],
                },
                guards: vec![],
                updates: vec![
                    // Observation anchor normalization: latest observed terminal class wins.
                    Update::SetRemove {
                        field: "observed_failed_runs".into(),
                        value: Expr::Binding("run_id".into()),
                    },
                    Update::SetRemove {
                        field: "observed_cancelled_runs".into(),
                        value: Expr::Binding("run_id".into()),
                    },
                    Update::SetInsert {
                        field: "observed_completed_runs".into(),
                        value: Expr::Binding("run_id".into()),
                    },
                ],
                to: "Tracking".into(),
                emit: vec![EffectEmit {
                    variant: "HelperResultSnapshotUpdated".into(),
                    fields: IndexMap::new(),
                }],
            },
            TransitionSchema {
                name: "AnchorFailed".into(),
                from: vec!["Tracking".into()],
                on: InputMatch {
                    variant: "AnchorFailed".into(),
                    bindings: vec!["run_id".into()],
                },
                guards: vec![],
                updates: vec![
                    Update::SetRemove {
                        field: "observed_completed_runs".into(),
                        value: Expr::Binding("run_id".into()),
                    },
                    Update::SetRemove {
                        field: "observed_cancelled_runs".into(),
                        value: Expr::Binding("run_id".into()),
                    },
                    Update::SetInsert {
                        field: "observed_failed_runs".into(),
                        value: Expr::Binding("run_id".into()),
                    },
                ],
                to: "Tracking".into(),
                emit: vec![EffectEmit {
                    variant: "HelperResultSnapshotUpdated".into(),
                    fields: IndexMap::new(),
                }],
            },
            TransitionSchema {
                name: "AnchorCancelled".into(),
                from: vec!["Tracking".into()],
                on: InputMatch {
                    variant: "AnchorCancelled".into(),
                    bindings: vec!["run_id".into()],
                },
                guards: vec![],
                updates: vec![
                    Update::SetRemove {
                        field: "observed_completed_runs".into(),
                        value: Expr::Binding("run_id".into()),
                    },
                    Update::SetRemove {
                        field: "observed_failed_runs".into(),
                        value: Expr::Binding("run_id".into()),
                    },
                    Update::SetInsert {
                        field: "observed_cancelled_runs".into(),
                        value: Expr::Binding("run_id".into()),
                    },
                ],
                to: "Tracking".into(),
                emit: vec![EffectEmit {
                    variant: "HelperResultSnapshotUpdated".into(),
                    fields: IndexMap::new(),
                }],
            },
            TransitionSchema {
                name: "AnchorForceCancelled".into(),
                from: vec!["Tracking".into()],
                on: InputMatch {
                    variant: "AnchorForceCancelled".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![Update::Increment {
                    field: "force_cancel_count".into(),
                    amount: 1,
                }],
                to: "Tracking".into(),
                emit: vec![EffectEmit {
                    variant: "HelperResultSnapshotUpdated".into(),
                    fields: IndexMap::new(),
                }],
            },
        ],
        effect_dispositions: vec![disposition(
            "HelperResultSnapshotUpdated",
            EffectDisposition::Local,
        )],
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
