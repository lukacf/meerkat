use indexmap::IndexMap;

use crate::{
    EffectDisposition, EffectDispositionRule, EffectEmit, EnumSchema, Expr, FieldInit, FieldSchema,
    InitSchema, InputMatch, InvariantSchema, MachineSchema, Quantifier, RustBinding, StateSchema,
    TransitionSchema, TypeRef, Update, VariantSchema,
};

/// Canonical owner for member bootstrap truth in `mob_bundle`.
///
/// This machine tracks the initial autonomous kickoff run for mob members.
/// Successful helper work after bootstrap is ordinary peer communication and
/// does not belong here; this machine owns only kickoff resolution classes.
pub fn mob_member_bootstrap_machine() -> MachineSchema {
    MachineSchema {
        machine: "MobMemberBootstrapMachine".into(),
        version: 1,
        rust: RustBinding {
            crate_name: "meerkat-mob".into(),
            module: "generated::mob_member_bootstrap".into(),
        },
        state: StateSchema {
            phase: EnumSchema {
                name: "MobMemberBootstrapState".into(),
                variants: vec![variant("Tracking")],
            },
            fields: vec![
                field(
                    "started_runs",
                    TypeRef::Set(Box::new(TypeRef::Named("RunId".into()))),
                ),
                field(
                    "callback_pending_runs",
                    TypeRef::Set(Box::new(TypeRef::Named("RunId".into()))),
                ),
                field(
                    "failed_runs",
                    TypeRef::Set(Box::new(TypeRef::Named("RunId".into()))),
                ),
                field(
                    "cancelled_runs",
                    TypeRef::Set(Box::new(TypeRef::Named("RunId".into()))),
                ),
                field("force_cancel_count", TypeRef::U32),
            ],
            init: InitSchema {
                phase: "Tracking".into(),
                fields: vec![
                    init("started_runs", Expr::EmptySet),
                    init("callback_pending_runs", Expr::EmptySet),
                    init("failed_runs", Expr::EmptySet),
                    init("cancelled_runs", Expr::EmptySet),
                    init("force_cancel_count", Expr::U64(0)),
                ],
            },
            terminal_phases: vec![],
        },
        inputs: EnumSchema {
            name: "MobMemberBootstrapInput".into(),
            variants: vec![
                VariantSchema {
                    name: "KickoffStarted".into(),
                    fields: vec![field("run_id", TypeRef::Named("RunId".into()))],
                },
                VariantSchema {
                    name: "KickoffCallbackPending".into(),
                    fields: vec![field("run_id", TypeRef::Named("RunId".into()))],
                },
                VariantSchema {
                    name: "KickoffFailed".into(),
                    fields: vec![field("run_id", TypeRef::Named("RunId".into()))],
                },
                VariantSchema {
                    name: "KickoffCancelled".into(),
                    fields: vec![field("run_id", TypeRef::Named("RunId".into()))],
                },
                variant("KickoffForceCancelled"),
            ],
        },
        effects: EnumSchema {
            name: "MobMemberBootstrapEffect".into(),
            variants: vec![variant("BootstrapStateUpdated")],
        },
        helpers: vec![],
        derived: vec![],
        invariants: vec![
            disjoint_invariant("started_not_failed", "started_runs", "failed_runs"),
            disjoint_invariant("started_not_cancelled", "started_runs", "cancelled_runs"),
            disjoint_invariant("failed_not_cancelled", "failed_runs", "cancelled_runs"),
            InvariantSchema {
                name: "callback_pending_not_terminal".into(),
                expr: Expr::Quantified {
                    quantifier: Quantifier::All,
                    binding: "run_id".into(),
                    over: Box::new(Expr::Field("callback_pending_runs".into())),
                    body: Box::new(Expr::And(vec![
                        not_contains("started_runs", "run_id"),
                        not_contains("failed_runs", "run_id"),
                        not_contains("cancelled_runs", "run_id"),
                    ])),
                },
            },
        ],
        transitions: vec![
            transition(
                "KickoffStarted",
                "KickoffStarted",
                vec![
                    remove_from("callback_pending_runs", "run_id"),
                    remove_from("failed_runs", "run_id"),
                    remove_from("cancelled_runs", "run_id"),
                    insert_into("started_runs", "run_id"),
                ],
            ),
            transition(
                "KickoffCallbackPending",
                "KickoffCallbackPending",
                vec![
                    remove_from("started_runs", "run_id"),
                    remove_from("failed_runs", "run_id"),
                    remove_from("cancelled_runs", "run_id"),
                    insert_into("callback_pending_runs", "run_id"),
                ],
            ),
            transition(
                "KickoffFailed",
                "KickoffFailed",
                vec![
                    remove_from("started_runs", "run_id"),
                    remove_from("callback_pending_runs", "run_id"),
                    remove_from("cancelled_runs", "run_id"),
                    insert_into("failed_runs", "run_id"),
                ],
            ),
            transition(
                "KickoffCancelled",
                "KickoffCancelled",
                vec![
                    remove_from("started_runs", "run_id"),
                    remove_from("callback_pending_runs", "run_id"),
                    remove_from("failed_runs", "run_id"),
                    insert_into("cancelled_runs", "run_id"),
                ],
            ),
            TransitionSchema {
                name: "KickoffForceCancelled".into(),
                from: vec!["Tracking".into()],
                on: InputMatch {
                    variant: "KickoffForceCancelled".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![Update::Increment {
                    field: "force_cancel_count".into(),
                    amount: 1,
                }],
                to: "Tracking".into(),
                emit: vec![EffectEmit {
                    variant: "BootstrapStateUpdated".into(),
                    fields: IndexMap::new(),
                }],
            },
        ],
        ci_step_limit: None,
        effect_dispositions: vec![disposition(
            "BootstrapStateUpdated",
            EffectDisposition::Local,
        )],
    }
}

fn transition(name: &str, variant_name: &str, updates: Vec<Update>) -> TransitionSchema {
    TransitionSchema {
        name: name.into(),
        from: vec!["Tracking".into()],
        on: InputMatch {
            variant: variant_name.into(),
            bindings: vec!["run_id".into()],
        },
        guards: vec![],
        updates,
        to: "Tracking".into(),
        emit: vec![EffectEmit {
            variant: "BootstrapStateUpdated".into(),
            fields: IndexMap::new(),
        }],
    }
}

fn disjoint_invariant(name: &str, left: &str, right: &str) -> InvariantSchema {
    InvariantSchema {
        name: name.into(),
        expr: Expr::Quantified {
            quantifier: Quantifier::All,
            binding: "run_id".into(),
            over: Box::new(Expr::Field(left.into())),
            body: Box::new(Expr::Not(Box::new(Expr::Contains {
                collection: Box::new(Expr::Field(right.into())),
                value: Box::new(Expr::Binding("run_id".into())),
            }))),
        },
    }
}

fn not_contains(field: &str, binding: &str) -> Expr {
    Expr::Not(Box::new(Expr::Contains {
        collection: Box::new(Expr::Field(field.into())),
        value: Box::new(Expr::Binding(binding.into())),
    }))
}

fn insert_into(field: &str, binding: &str) -> Update {
    Update::SetInsert {
        field: field.into(),
        value: Expr::Binding(binding.into()),
    }
}

fn remove_from(field: &str, binding: &str) -> Update {
    Update::SetRemove {
        field: field.into(),
        value: Expr::Binding(binding.into()),
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
