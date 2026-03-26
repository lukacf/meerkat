use indexmap::IndexMap;

use crate::{
    EffectDisposition, EffectDispositionRule, EffectEmit, EnumSchema, Expr, FieldInit, FieldSchema,
    InitSchema, InputMatch, InvariantSchema, MachineSchema, RustBinding, StateSchema,
    TransitionSchema, TypeRef, Update, VariantSchema,
};

/// Observation anchor for mob wiring-related routes in `mob_bundle`.
///
/// This machine is deliberately narrower than a canonical peer-graph owner. It
/// records which peer-candidate, runtime-admission, and exposed-operation-peer
/// routes the composition has observed so those boundaries are explicit in the
/// formal model while canonical wiring truth is still being extracted from live
/// mob code. This anchor only preserves those observations; the canonical
/// wiring ownership is still modeled elsewhere and should remain the source of
/// truth.
pub fn mob_wiring_anchor_machine() -> MachineSchema {
    MachineSchema {
        machine: "MobWiringAnchorMachine".into(),
        version: 1,
        rust: RustBinding {
            crate_name: "meerkat-mob".into(),
            module: "generated::mob_wiring_anchor".into(),
        },
        state: StateSchema {
            phase: EnumSchema {
                name: "MobWiringAnchorState".into(),
                variants: vec![variant("Tracking")],
            },
            fields: vec![
                field(
                    "observed_trusted_operation_peers",
                    TypeRef::Set(Box::new(TypeRef::Named("OperationId".into()))),
                ),
                field(
                    "observed_peer_input_candidates",
                    TypeRef::Set(Box::new(TypeRef::Named("RawItemId".into()))),
                ),
                field(
                    "observed_runtime_work_items",
                    TypeRef::Set(Box::new(TypeRef::Named("WorkId".into()))),
                ),
            ],
            init: InitSchema {
                phase: "Tracking".into(),
                fields: vec![
                    init("observed_trusted_operation_peers", Expr::EmptySet),
                    init("observed_peer_input_candidates", Expr::EmptySet),
                    init("observed_runtime_work_items", Expr::EmptySet),
                ],
            },
            terminal_phases: vec![],
        },
        inputs: EnumSchema {
            name: "MobWiringAnchorInput".into(),
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
            name: "MobWiringAnchorEffect".into(),
            variants: vec![variant("WiringSnapshotUpdated")],
        },
        helpers: vec![],
        derived: vec![],
        invariants: Vec::<InvariantSchema>::new(),
        transitions: vec![
            TransitionSchema {
                name: "OperationPeerTrusted".into(),
                from: vec!["Tracking".into()],
                on: InputMatch {
                    variant: "OperationPeerTrusted".into(),
                    bindings: vec!["operation_id".into()],
                },
                guards: vec![],
                updates: vec![Update::SetInsert {
                    field: "observed_trusted_operation_peers".into(),
                    value: Expr::Binding("operation_id".into()),
                }],
                to: "Tracking".into(),
                emit: vec![EffectEmit {
                    variant: "WiringSnapshotUpdated".into(),
                    fields: IndexMap::new(),
                }],
            },
            TransitionSchema {
                name: "PeerInputAdmitted".into(),
                from: vec!["Tracking".into()],
                on: InputMatch {
                    variant: "PeerInputAdmitted".into(),
                    bindings: vec!["raw_item_id".into(), "peer_input_class".into()],
                },
                guards: vec![],
                updates: vec![Update::SetInsert {
                    field: "observed_peer_input_candidates".into(),
                    value: Expr::Binding("raw_item_id".into()),
                }],
                to: "Tracking".into(),
                emit: vec![EffectEmit {
                    variant: "WiringSnapshotUpdated".into(),
                    fields: IndexMap::new(),
                }],
            },
            TransitionSchema {
                name: "RuntimeWorkAdmitted".into(),
                from: vec!["Tracking".into()],
                on: InputMatch {
                    variant: "RuntimeWorkAdmitted".into(),
                    bindings: vec!["work_id".into(), "handling_mode".into()],
                },
                guards: vec![],
                updates: vec![Update::SetInsert {
                    field: "observed_runtime_work_items".into(),
                    value: Expr::Binding("work_id".into()),
                }],
                to: "Tracking".into(),
                emit: vec![EffectEmit {
                    variant: "WiringSnapshotUpdated".into(),
                    fields: IndexMap::new(),
                }],
            },
        ],
        effect_dispositions: vec![disposition(
            "WiringSnapshotUpdated",
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
