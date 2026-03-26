use indexmap::IndexMap;

use crate::{
    EffectDisposition, EffectDispositionRule, EffectEmit, EnumSchema, Expr, FieldInit, FieldSchema,
    InitSchema, InputMatch, InvariantSchema, MachineSchema, RustBinding, StateSchema,
    TransitionSchema, TypeRef, Update, VariantSchema,
};

pub fn mob_lifecycle_machine() -> MachineSchema {
    MachineSchema {
        machine: "MobLifecycleMachine".into(),
        version: 2,
        rust: RustBinding {
            crate_name: "meerkat-mob".into(),
            module: "generated::mob_lifecycle".into(),
        },
        state: StateSchema {
            phase: EnumSchema {
                name: "MobLifecycleState".into(),
                variants: vec![
                    variant("Creating"),
                    variant("Running"),
                    variant("Stopped"),
                    variant("Completed"),
                    variant("Destroyed"),
                ],
            },
            fields: vec![
                field("active_run_count", TypeRef::U32),
                field("cleanup_pending", TypeRef::Bool),
            ],
            init: InitSchema {
                phase: "Creating".into(),
                fields: vec![
                    init("active_run_count", Expr::U64(0)),
                    init("cleanup_pending", Expr::Bool(false)),
                ],
            },
            terminal_phases: vec!["Destroyed".into()],
        },
        inputs: EnumSchema {
            name: "MobLifecycleInput".into(),
            variants: vec![
                variant("Start"),
                variant("Stop"),
                variant("Resume"),
                variant("MarkCompleted"),
                variant("Destroy"),
                variant("StartRun"),
                variant("FinishRun"),
                variant("BeginCleanup"),
                variant("FinishCleanup"),
            ],
        },
        effects: EnumSchema {
            name: "MobLifecycleEffect".into(),
            variants: vec![variant("EmitLifecycleNotice"), variant("RequestCleanup")],
        },
        helpers: vec![],
        derived: vec![],
        invariants: vec![
            InvariantSchema {
                name: "destroyed_has_no_active_runs".into(),
                expr: Expr::Or(vec![
                    Expr::Neq(
                        Box::new(Expr::CurrentPhase),
                        Box::new(Expr::Phase("Destroyed".into())),
                    ),
                    Expr::Eq(
                        Box::new(Expr::Field("active_run_count".into())),
                        Box::new(Expr::U64(0)),
                    ),
                ]),
            },
            InvariantSchema {
                name: "completed_has_no_active_runs".into(),
                expr: Expr::Or(vec![
                    Expr::Neq(
                        Box::new(Expr::CurrentPhase),
                        Box::new(Expr::Phase("Completed".into())),
                    ),
                    Expr::Eq(
                        Box::new(Expr::Field("active_run_count".into())),
                        Box::new(Expr::U64(0)),
                    ),
                ]),
            },
        ],
        transitions: vec![
            transition(
                "Start",
                &["Creating", "Stopped"],
                "Start",
                &[],
                &[],
                "Running",
            ),
            transition("Stop", &["Running"], "Stop", &[], &[], "Stopped"),
            transition("Resume", &["Stopped"], "Resume", &[], &[], "Running"),
            TransitionSchema {
                name: "MarkCompleted".into(),
                from: vec!["Running".into(), "Stopped".into()],
                on: InputMatch {
                    variant: "MarkCompleted".into(),
                    bindings: vec![],
                },
                guards: vec![crate::Guard {
                    name: "no_active_runs".into(),
                    expr: crate::Expr::Eq(
                        Box::new(crate::Expr::Field("active_run_count".into())),
                        Box::new(crate::Expr::U64(0)),
                    ),
                }],
                updates: vec![],
                to: "Completed".into(),
                emit: vec![],
            },
            transition(
                "Destroy",
                &["Creating", "Running", "Stopped", "Completed"],
                "Destroy",
                &[],
                &[EffectEmit {
                    variant: "EmitLifecycleNotice".into(),
                    fields: IndexMap::new(),
                }],
                "Destroyed",
            ),
            TransitionSchema {
                name: "StartRun".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    variant: "StartRun".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![Update::Increment {
                    field: "active_run_count".into(),
                    amount: 1,
                }],
                to: "Running".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "FinishRun".into(),
                from: vec!["Running".into(), "Stopped".into()],
                on: InputMatch {
                    variant: "FinishRun".into(),
                    bindings: vec![],
                },
                guards: vec![crate::Guard {
                    name: "has_active_runs".into(),
                    expr: crate::Expr::Gt(
                        Box::new(crate::Expr::Field("active_run_count".into())),
                        Box::new(crate::Expr::U64(0)),
                    ),
                }],
                updates: vec![Update::Decrement {
                    field: "active_run_count".into(),
                    amount: 1,
                }],
                to: "Running".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "BeginCleanup".into(),
                from: vec!["Stopped".into(), "Completed".into()],
                on: InputMatch {
                    variant: "BeginCleanup".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![Update::Assign {
                    field: "cleanup_pending".into(),
                    expr: Expr::Bool(true),
                }],
                to: "Stopped".into(),
                emit: vec![EffectEmit {
                    variant: "RequestCleanup".into(),
                    fields: IndexMap::new(),
                }],
            },
            TransitionSchema {
                name: "FinishCleanup".into(),
                from: vec!["Stopped".into(), "Completed".into()],
                on: InputMatch {
                    variant: "FinishCleanup".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![Update::Assign {
                    field: "cleanup_pending".into(),
                    expr: Expr::Bool(false),
                }],
                to: "Stopped".into(),
                emit: vec![],
            },
        ],
        effect_dispositions: vec![
            disposition("EmitLifecycleNotice", EffectDisposition::External),
            disposition(
                "RequestCleanup",
                EffectDisposition::Routed {
                    consumer_machines: vec!["MobOrchestratorMachine".into()],
                },
            ),
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
        updates: if name == "Destroy" {
            vec![
                Update::Assign {
                    field: "active_run_count".into(),
                    expr: Expr::U64(0),
                },
                Update::Assign {
                    field: "cleanup_pending".into(),
                    expr: Expr::Bool(false),
                },
            ]
        } else {
            vec![]
        },
        to: to.into(),
        emit: emit.to_vec(),
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
