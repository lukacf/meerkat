use indexmap::IndexMap;

use crate::{
    EffectDisposition, EffectDispositionRule, EffectEmit, EnumSchema, Expr, FieldInit, FieldSchema,
    InitSchema, InputMatch, InvariantSchema, MachineSchema, RustBinding, StateSchema,
    TransitionSchema, TypeRef, Update, VariantSchema,
};

/// Canonical owner for mob wiring-related routes in `mob_bundle`.
///
/// This machine records the trusted operation peers, peer-input candidates,
/// and runtime-admitted work items that define the mob's effective peer-wiring
/// state. It is the formal owner for the mob wiring seam; projections such as
/// roster wiring views are derived from this authority rather than the reverse.
pub fn mob_wiring_machine() -> MachineSchema {
    MachineSchema {
        machine: "MobWiringMachine".into(),
        version: 1,
        rust: RustBinding {
            crate_name: "meerkat-mob".into(),
            module: "generated::mob_wiring".into(),
        },
        state: StateSchema {
            phase: EnumSchema {
                name: "MobWiringState".into(),
                variants: vec![variant("Stable")],
            },
            fields: vec![
                field(
                    "trusted_operation_peers",
                    TypeRef::Set(Box::new(TypeRef::Named("OperationId".into()))),
                ),
                field(
                    "admitted_peer_input_candidates",
                    TypeRef::Set(Box::new(TypeRef::Named("RawItemId".into()))),
                ),
                field(
                    "admitted_runtime_work_items",
                    TypeRef::Set(Box::new(TypeRef::Named("WorkId".into()))),
                ),
            ],
            init: InitSchema {
                phase: "Stable".into(),
                fields: vec![
                    init("trusted_operation_peers", Expr::EmptySet),
                    init("admitted_peer_input_candidates", Expr::EmptySet),
                    init("admitted_runtime_work_items", Expr::EmptySet),
                ],
            },
            terminal_phases: vec![],
        },
        inputs: EnumSchema {
            name: "MobWiringInput".into(),
            variants: vec![
                VariantSchema {
                    name: "OperationPeerTrusted".into(),
                    fields: vec![field("operation_id", TypeRef::Named("OperationId".into()))],
                },
                VariantSchema {
                    name: "PeerInputAdmitted".into(),
                    fields: vec![
                        field("raw_item_id", TypeRef::Named("RawItemId".into())),
                        field("peer_input_class", TypeRef::Named("PeerInputClass".into())),
                    ],
                },
                VariantSchema {
                    name: "RuntimeWorkAdmitted".into(),
                    fields: vec![
                        field("work_id", TypeRef::Named("WorkId".into())),
                        field("handling_mode", TypeRef::Named("HandlingMode".into())),
                    ],
                },
            ],
        },
        effects: EnumSchema {
            name: "MobWiringEffect".into(),
            variants: vec![variant("WiringStateUpdated")],
        },
        helpers: vec![],
        derived: vec![],
        invariants: Vec::<InvariantSchema>::new(),
        transitions: vec![
            TransitionSchema {
                name: "OperationPeerTrusted".into(),
                from: vec!["Stable".into()],
                on: InputMatch {
                    variant: "OperationPeerTrusted".into(),
                    bindings: vec!["operation_id".into()],
                },
                guards: vec![],
                updates: vec![Update::SetInsert {
                    field: "trusted_operation_peers".into(),
                    value: Expr::Binding("operation_id".into()),
                }],
                to: "Stable".into(),
                emit: vec![EffectEmit {
                    variant: "WiringStateUpdated".into(),
                    fields: IndexMap::new(),
                }],
            },
            TransitionSchema {
                name: "PeerInputAdmitted".into(),
                from: vec!["Stable".into()],
                on: InputMatch {
                    variant: "PeerInputAdmitted".into(),
                    bindings: vec!["raw_item_id".into(), "peer_input_class".into()],
                },
                guards: vec![],
                updates: vec![Update::SetInsert {
                    field: "admitted_peer_input_candidates".into(),
                    value: Expr::Binding("raw_item_id".into()),
                }],
                to: "Stable".into(),
                emit: vec![EffectEmit {
                    variant: "WiringStateUpdated".into(),
                    fields: IndexMap::new(),
                }],
            },
            TransitionSchema {
                name: "RuntimeWorkAdmitted".into(),
                from: vec!["Stable".into()],
                on: InputMatch {
                    variant: "RuntimeWorkAdmitted".into(),
                    bindings: vec!["work_id".into(), "handling_mode".into()],
                },
                guards: vec![],
                updates: vec![Update::SetInsert {
                    field: "admitted_runtime_work_items".into(),
                    value: Expr::Binding("work_id".into()),
                }],
                to: "Stable".into(),
                emit: vec![EffectEmit {
                    variant: "WiringStateUpdated".into(),
                    fields: IndexMap::new(),
                }],
            },
        ],
        ci_step_limit: None,
        effect_dispositions: vec![disposition("WiringStateUpdated", EffectDisposition::Local)],
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
