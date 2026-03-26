use indexmap::IndexMap;

use crate::{
    EffectDisposition, EffectDispositionRule, EffectEmit, EnumSchema, Expr, FieldInit, FieldSchema,
    InitSchema, InputMatch, InvariantSchema, MachineSchema, RustBinding, StateSchema,
    TransitionSchema, TypeRef, Update, VariantSchema,
};

/// Observation anchor for runtime-bridge-related routes in `mob_bundle`.
///
/// This machine does not own canonical bridge/session/queued-turn truth yet.
/// It only records which run-lifecycle and stop-request events the composition
/// has observed at the mob/runtime bridge boundary. The live bridge owner is
/// still elsewhere, so this anchor exists purely for visibility and should not
/// be mistaken for the ultimate owner of those routes.
pub fn mob_runtime_bridge_anchor_machine() -> MachineSchema {
    MachineSchema {
        machine: "MobRuntimeBridgeAnchorMachine".into(),
        version: 1,
        rust: RustBinding {
            crate_name: "meerkat-mob".into(),
            module: "generated::mob_runtime_bridge_anchor".into(),
        },
        state: StateSchema {
            phase: EnumSchema {
                name: "MobRuntimeBridgeAnchorState".into(),
                variants: vec![variant("Tracking")],
            },
            fields: vec![
                field(
                    "observed_submitted_runs",
                    TypeRef::Set(Box::new(TypeRef::Named("RunId".into()))),
                ),
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
                field("stop_request_count", TypeRef::U32),
            ],
            init: InitSchema {
                phase: "Tracking".into(),
                fields: vec![
                    init("observed_submitted_runs", Expr::EmptySet),
                    init("observed_completed_runs", Expr::EmptySet),
                    init("observed_failed_runs", Expr::EmptySet),
                    init("observed_cancelled_runs", Expr::EmptySet),
                    init("stop_request_count", Expr::U64(0)),
                ],
            },
            terminal_phases: vec![],
        },
        inputs: EnumSchema {
            name: "MobRuntimeBridgeAnchorInput".into(),
            variants: vec![
                VariantSchema {
                    name: "RuntimeRunSubmitted".into(),
                    fields: vec![field("run_id", TypeRef::Named("RunId".into()))],
                },
                VariantSchema {
                    name: "RuntimeRunCompleted".into(),
                    fields: vec![field("run_id", TypeRef::Named("RunId".into()))],
                },
                VariantSchema {
                    name: "RuntimeRunFailed".into(),
                    fields: vec![field("run_id", TypeRef::Named("RunId".into()))],
                },
                VariantSchema {
                    name: "RuntimeRunCancelled".into(),
                    fields: vec![field("run_id", TypeRef::Named("RunId".into()))],
                },
                variant("RuntimeStopRequested"),
            ],
        },
        effects: EnumSchema {
            name: "MobRuntimeBridgeAnchorEffect".into(),
            variants: vec![variant("RuntimeBridgeSnapshotUpdated")],
        },
        helpers: vec![],
        derived: vec![],
        invariants: Vec::<InvariantSchema>::new(),
        transitions: vec![
            TransitionSchema {
                name: "RuntimeRunSubmitted".into(),
                from: vec!["Tracking".into()],
                on: InputMatch {
                    variant: "RuntimeRunSubmitted".into(),
                    bindings: vec!["run_id".into()],
                },
                guards: vec![],
                updates: vec![Update::SetInsert {
                    field: "observed_submitted_runs".into(),
                    value: Expr::Binding("run_id".into()),
                }],
                to: "Tracking".into(),
                emit: vec![EffectEmit {
                    variant: "RuntimeBridgeSnapshotUpdated".into(),
                    fields: IndexMap::new(),
                }],
            },
            TransitionSchema {
                name: "RuntimeRunCompleted".into(),
                from: vec!["Tracking".into()],
                on: InputMatch {
                    variant: "RuntimeRunCompleted".into(),
                    bindings: vec!["run_id".into()],
                },
                guards: vec![],
                updates: vec![Update::SetInsert {
                    field: "observed_completed_runs".into(),
                    value: Expr::Binding("run_id".into()),
                }],
                to: "Tracking".into(),
                emit: vec![EffectEmit {
                    variant: "RuntimeBridgeSnapshotUpdated".into(),
                    fields: IndexMap::new(),
                }],
            },
            TransitionSchema {
                name: "RuntimeRunFailed".into(),
                from: vec!["Tracking".into()],
                on: InputMatch {
                    variant: "RuntimeRunFailed".into(),
                    bindings: vec!["run_id".into()],
                },
                guards: vec![],
                updates: vec![Update::SetInsert {
                    field: "observed_failed_runs".into(),
                    value: Expr::Binding("run_id".into()),
                }],
                to: "Tracking".into(),
                emit: vec![EffectEmit {
                    variant: "RuntimeBridgeSnapshotUpdated".into(),
                    fields: IndexMap::new(),
                }],
            },
            TransitionSchema {
                name: "RuntimeRunCancelled".into(),
                from: vec!["Tracking".into()],
                on: InputMatch {
                    variant: "RuntimeRunCancelled".into(),
                    bindings: vec!["run_id".into()],
                },
                guards: vec![],
                updates: vec![Update::SetInsert {
                    field: "observed_cancelled_runs".into(),
                    value: Expr::Binding("run_id".into()),
                }],
                to: "Tracking".into(),
                emit: vec![EffectEmit {
                    variant: "RuntimeBridgeSnapshotUpdated".into(),
                    fields: IndexMap::new(),
                }],
            },
            TransitionSchema {
                name: "RuntimeStopRequested".into(),
                from: vec!["Tracking".into()],
                on: InputMatch {
                    variant: "RuntimeStopRequested".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![Update::Increment {
                    field: "stop_request_count".into(),
                    amount: 1,
                }],
                to: "Tracking".into(),
                emit: vec![EffectEmit {
                    variant: "RuntimeBridgeSnapshotUpdated".into(),
                    fields: IndexMap::new(),
                }],
            },
        ],
        effect_dispositions: vec![disposition(
            "RuntimeBridgeSnapshotUpdated",
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
