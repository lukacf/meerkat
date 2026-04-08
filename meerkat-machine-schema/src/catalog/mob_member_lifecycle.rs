use indexmap::IndexMap;

use crate::{
    EffectDisposition, EffectDispositionRule, EffectEmit, EnumSchema, Expr, FieldInit, FieldSchema,
    InitSchema, InputMatch, InvariantSchema, MachineSchema, RustBinding, StateSchema,
    TransitionSchema, TypeRef, Update, VariantSchema,
};

/// Canonical owner for member lifecycle truth in `mob_bundle`.
///
/// This machine tracks peer exposure and member terminalization routes that
/// determine whether a member remains active, is retiring, or has fully
/// terminalized. Bootstrap failures are modeled separately in
/// `MobMemberBootstrapMachine` and must not collapse member lifecycle into
/// corruption.
pub fn mob_member_lifecycle_machine() -> MachineSchema {
    MachineSchema {
        machine: "MobMemberLifecycleMachine".into(),
        version: 1,
        rust: RustBinding {
            crate_name: "meerkat-mob".into(),
            module: "generated::mob_member_lifecycle".into(),
        },
        state: StateSchema {
            phase: EnumSchema {
                name: "MobMemberLifecycleState".into(),
                variants: vec![variant("Stable")],
            },
            fields: vec![
                field(
                    "observed_peer_exposed_operations",
                    TypeRef::Set(Box::new(TypeRef::Named("OperationId".into()))),
                ),
                field(
                    "observed_terminalized_operations",
                    TypeRef::Set(Box::new(TypeRef::Named("OperationId".into()))),
                ),
                field("peer_exposure_count", TypeRef::U32),
                field("terminalization_count", TypeRef::U32),
            ],
            init: InitSchema {
                phase: "Stable".into(),
                fields: vec![
                    init("observed_peer_exposed_operations", Expr::EmptySet),
                    init("observed_terminalized_operations", Expr::EmptySet),
                    init("peer_exposure_count", Expr::U64(0)),
                    init("terminalization_count", Expr::U64(0)),
                ],
            },
            terminal_phases: vec![],
        },
        inputs: EnumSchema {
            name: "MobMemberLifecycleInput".into(),
            variants: vec![
                VariantSchema {
                    name: "MemberPeerExposed".into(),
                    fields: vec![field("operation_id", TypeRef::Named("OperationId".into()))],
                },
                VariantSchema {
                    name: "MemberTerminalized".into(),
                    fields: vec![
                        field("operation_id", TypeRef::Named("OperationId".into())),
                        field(
                            "terminal_outcome",
                            TypeRef::Named("OperationTerminalOutcome".into()),
                        ),
                    ],
                },
            ],
        },
        effects: EnumSchema {
            name: "MobMemberLifecycleEffect".into(),
            variants: vec![variant("MemberLifecycleStateUpdated")],
        },
        helpers: vec![],
        derived: vec![],
        invariants: Vec::<InvariantSchema>::new(),
        transitions: vec![
            TransitionSchema {
                name: "MemberPeerExposed".into(),
                from: vec!["Stable".into()],
                on: InputMatch {
                    variant: "MemberPeerExposed".into(),
                    bindings: vec!["operation_id".into()],
                },
                guards: vec![],
                updates: vec![
                    Update::SetInsert {
                        field: "observed_peer_exposed_operations".into(),
                        value: Expr::Binding("operation_id".into()),
                    },
                    Update::Increment {
                        field: "peer_exposure_count".into(),
                        amount: 1,
                    },
                ],
                to: "Stable".into(),
                emit: vec![EffectEmit {
                    variant: "MemberLifecycleStateUpdated".into(),
                    fields: IndexMap::new(),
                }],
            },
            TransitionSchema {
                name: "MemberTerminalized".into(),
                from: vec!["Stable".into()],
                on: InputMatch {
                    variant: "MemberTerminalized".into(),
                    bindings: vec!["operation_id".into(), "terminal_outcome".into()],
                },
                guards: vec![],
                updates: vec![
                    Update::SetInsert {
                        field: "observed_terminalized_operations".into(),
                        value: Expr::Binding("operation_id".into()),
                    },
                    Update::Increment {
                        field: "terminalization_count".into(),
                        amount: 1,
                    },
                ],
                to: "Stable".into(),
                emit: vec![EffectEmit {
                    variant: "MemberLifecycleStateUpdated".into(),
                    fields: IndexMap::new(),
                }],
            },
        ],
        ci_step_limit: None,
        effect_dispositions: vec![disposition(
            "MemberLifecycleStateUpdated",
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
