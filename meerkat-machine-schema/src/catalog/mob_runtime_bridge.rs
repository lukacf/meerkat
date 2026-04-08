use indexmap::IndexMap;

use crate::{
    EffectDisposition, EffectDispositionRule, EffectEmit, EnumSchema, Expr, FieldInit, FieldSchema,
    InitSchema, InputMatch, InvariantSchema, MachineSchema, RustBinding, StateSchema,
    TransitionSchema, TypeRef, Update, VariantSchema,
};

/// Canonical owner for runtime-bridge-related routes in `mob_bundle`.
///
/// This machine tracks run submission, terminalization, and stop-request
/// boundaries at the mob/runtime bridge. Parent-session association and
/// lifecycle notice emission in live code must remain aligned with this owner.
pub fn mob_runtime_bridge_machine() -> MachineSchema {
    MachineSchema {
        machine: "MobRuntimeBridgeMachine".into(),
        version: 1,
        rust: RustBinding {
            crate_name: "meerkat-mob".into(),
            module: "generated::mob_runtime_bridge".into(),
        },
        state: StateSchema {
            phase: EnumSchema {
                name: "MobRuntimeBridgeState".into(),
                variants: vec![variant("Stable")],
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
                phase: "Stable".into(),
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
            name: "MobRuntimeBridgeInput".into(),
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
            name: "MobRuntimeBridgeEffect".into(),
            variants: vec![variant("RuntimeBridgeStateUpdated")],
        },
        helpers: vec![],
        derived: vec![],
        invariants: Vec::<InvariantSchema>::new(),
        transitions: vec![
            TransitionSchema {
                name: "RuntimeRunSubmitted".into(),
                from: vec!["Stable".into()],
                on: InputMatch {
                    variant: "RuntimeRunSubmitted".into(),
                    bindings: vec!["run_id".into()],
                },
                guards: vec![],
                updates: vec![Update::SetInsert {
                    field: "observed_submitted_runs".into(),
                    value: Expr::Binding("run_id".into()),
                }],
                to: "Stable".into(),
                emit: vec![EffectEmit {
                    variant: "RuntimeBridgeStateUpdated".into(),
                    fields: IndexMap::new(),
                }],
            },
            TransitionSchema {
                name: "RuntimeRunCompleted".into(),
                from: vec!["Stable".into()],
                on: InputMatch {
                    variant: "RuntimeRunCompleted".into(),
                    bindings: vec!["run_id".into()],
                },
                guards: vec![],
                updates: vec![Update::SetInsert {
                    field: "observed_completed_runs".into(),
                    value: Expr::Binding("run_id".into()),
                }],
                to: "Stable".into(),
                emit: vec![EffectEmit {
                    variant: "RuntimeBridgeStateUpdated".into(),
                    fields: IndexMap::new(),
                }],
            },
            TransitionSchema {
                name: "RuntimeRunFailed".into(),
                from: vec!["Stable".into()],
                on: InputMatch {
                    variant: "RuntimeRunFailed".into(),
                    bindings: vec!["run_id".into()],
                },
                guards: vec![],
                updates: vec![Update::SetInsert {
                    field: "observed_failed_runs".into(),
                    value: Expr::Binding("run_id".into()),
                }],
                to: "Stable".into(),
                emit: vec![EffectEmit {
                    variant: "RuntimeBridgeStateUpdated".into(),
                    fields: IndexMap::new(),
                }],
            },
            TransitionSchema {
                name: "RuntimeRunCancelled".into(),
                from: vec!["Stable".into()],
                on: InputMatch {
                    variant: "RuntimeRunCancelled".into(),
                    bindings: vec!["run_id".into()],
                },
                guards: vec![],
                updates: vec![Update::SetInsert {
                    field: "observed_cancelled_runs".into(),
                    value: Expr::Binding("run_id".into()),
                }],
                to: "Stable".into(),
                emit: vec![EffectEmit {
                    variant: "RuntimeBridgeStateUpdated".into(),
                    fields: IndexMap::new(),
                }],
            },
            TransitionSchema {
                name: "RuntimeStopRequested".into(),
                from: vec!["Stable".into()],
                on: InputMatch {
                    variant: "RuntimeStopRequested".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![Update::Increment {
                    field: "stop_request_count".into(),
                    amount: 1,
                }],
                to: "Stable".into(),
                emit: vec![EffectEmit {
                    variant: "RuntimeBridgeStateUpdated".into(),
                    fields: IndexMap::new(),
                }],
            },
        ],
        ci_step_limit: None,
        effect_dispositions: vec![disposition(
            "RuntimeBridgeStateUpdated",
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
