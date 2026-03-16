use indexmap::IndexMap;

use crate::{
    EffectEmit, EnumSchema, Expr, FieldSchema, Guard, HelperSchema, InitSchema, InputMatch,
    InvariantSchema, MachineSchema, Quantifier, RustBinding, StateSchema, TransitionSchema,
    TypeRef, Update, VariantSchema,
};

pub fn peer_comms_machine() -> MachineSchema {
    MachineSchema {
        machine: "PeerCommsMachine".into(),
        version: 1,
        rust: RustBinding {
            crate_name: "meerkat-comms".into(),
            module: "machines::peer_comms".into(),
        },
        state: StateSchema {
            phase: EnumSchema {
                name: "PeerIngressState".into(),
                variants: vec![
                    variant("Absent"),
                    variant("Received"),
                    variant("Dropped"),
                    variant("Delivered"),
                ],
            },
            fields: vec![
                field(
                    "trusted_peers",
                    TypeRef::Set(Box::new(TypeRef::Named("PeerId".into()))),
                ),
                field(
                    "raw_item_peer",
                    TypeRef::Map(
                        Box::new(TypeRef::Named("RawItemId".into())),
                        Box::new(TypeRef::Named("PeerId".into())),
                    ),
                ),
                field(
                    "raw_item_kind",
                    TypeRef::Map(
                        Box::new(TypeRef::Named("RawItemId".into())),
                        Box::new(TypeRef::Named("RawPeerKind".into())),
                    ),
                ),
                field(
                    "classified_as",
                    TypeRef::Map(
                        Box::new(TypeRef::Named("RawItemId".into())),
                        Box::new(TypeRef::Named("PeerInputClass".into())),
                    ),
                ),
                field(
                    "trusted_snapshot",
                    TypeRef::Map(
                        Box::new(TypeRef::Named("RawItemId".into())),
                        Box::new(TypeRef::Bool),
                    ),
                ),
                field(
                    "submission_queue",
                    TypeRef::Seq(Box::new(TypeRef::Named("RawItemId".into()))),
                ),
            ],
            init: InitSchema {
                phase: "Absent".into(),
                fields: vec![],
            },
            terminal_phases: vec!["Dropped".into(), "Delivered".into()],
        },
        inputs: EnumSchema {
            name: "PeerCommsInput".into(),
            variants: vec![
                VariantSchema {
                    name: "TrustPeer".into(),
                    fields: vec![field("peer_id", TypeRef::Named("PeerId".into()))],
                },
                VariantSchema {
                    name: "ReceivePeerEnvelope".into(),
                    fields: vec![
                        field("raw_item_id", TypeRef::Named("RawItemId".into())),
                        field("peer_id", TypeRef::Named("PeerId".into())),
                        field("raw_kind", TypeRef::Named("RawPeerKind".into())),
                    ],
                },
                VariantSchema {
                    name: "SubmitTypedPeerInput".into(),
                    fields: vec![field("raw_item_id", TypeRef::Named("RawItemId".into()))],
                },
            ],
        },
        effects: EnumSchema {
            name: "PeerCommsEffect".into(),
            variants: vec![VariantSchema {
                name: "SubmitPeerInputCandidate".into(),
                fields: vec![
                    field("raw_item_id", TypeRef::Named("RawItemId".into())),
                    field("peer_input_class", TypeRef::Named("PeerInputClass".into())),
                ],
            }],
        },
        helpers: vec![HelperSchema {
            name: "ClassFor".into(),
            params: vec![field("raw_kind", TypeRef::Named("RawPeerKind".into()))],
            returns: TypeRef::Named("PeerInputClass".into()),
            body: Expr::IfElse {
                condition: Box::new(Expr::Eq(
                    Box::new(Expr::Binding("raw_kind".into())),
                    Box::new(Expr::String("request".into())),
                )),
                then_expr: Box::new(Expr::String("ActionableRequest".into())),
                else_expr: Box::new(Expr::String("ActionableMessage".into())),
            },
        }],
        derived: vec![],
        invariants: vec![InvariantSchema {
            name: "queued_items_are_classified".into(),
            expr: Expr::Quantified {
                quantifier: Quantifier::All,
                binding: "raw_item_id".into(),
                over: Box::new(Expr::Field("submission_queue".into())),
                body: Box::new(Expr::Contains {
                    collection: Box::new(Expr::MapKeys(Box::new(Expr::Field(
                        "classified_as".into(),
                    )))),
                    value: Box::new(Expr::Binding("raw_item_id".into())),
                }),
            },
        }],
        transitions: vec![
            TransitionSchema {
                name: "TrustPeer".into(),
                from: vec!["Absent".into(), "Received".into()],
                on: InputMatch {
                    variant: "TrustPeer".into(),
                    bindings: vec!["peer_id".into()],
                },
                guards: vec![],
                updates: vec![Update::SetInsert {
                    field: "trusted_peers".into(),
                    value: Expr::Binding("peer_id".into()),
                }],
                to: "Absent".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "ReceiveTrustedPeerEnvelope".into(),
                from: vec!["Absent".into(), "Received".into()],
                on: InputMatch {
                    variant: "ReceivePeerEnvelope".into(),
                    bindings: vec!["raw_item_id".into(), "peer_id".into(), "raw_kind".into()],
                },
                guards: vec![Guard {
                    name: "peer_is_trusted".into(),
                    expr: Expr::Contains {
                        collection: Box::new(Expr::Field("trusted_peers".into())),
                        value: Box::new(Expr::Binding("peer_id".into())),
                    },
                }],
                updates: vec![
                    Update::MapInsert {
                        field: "raw_item_peer".into(),
                        key: Expr::Binding("raw_item_id".into()),
                        value: Expr::Binding("peer_id".into()),
                    },
                    Update::MapInsert {
                        field: "raw_item_kind".into(),
                        key: Expr::Binding("raw_item_id".into()),
                        value: Expr::Binding("raw_kind".into()),
                    },
                    Update::MapInsert {
                        field: "classified_as".into(),
                        key: Expr::Binding("raw_item_id".into()),
                        value: Expr::Call {
                            helper: "ClassFor".into(),
                            args: vec![Expr::Binding("raw_kind".into())],
                        },
                    },
                    Update::MapInsert {
                        field: "trusted_snapshot".into(),
                        key: Expr::Binding("raw_item_id".into()),
                        value: Expr::Bool(true),
                    },
                    Update::SeqAppend {
                        field: "submission_queue".into(),
                        value: Expr::Binding("raw_item_id".into()),
                    },
                ],
                to: "Received".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "DropUntrustedPeerEnvelope".into(),
                from: vec!["Absent".into(), "Received".into()],
                on: InputMatch {
                    variant: "ReceivePeerEnvelope".into(),
                    bindings: vec!["raw_item_id".into(), "peer_id".into(), "raw_kind".into()],
                },
                guards: vec![Guard {
                    name: "peer_is_not_trusted".into(),
                    expr: Expr::Not(Box::new(Expr::Contains {
                        collection: Box::new(Expr::Field("trusted_peers".into())),
                        value: Box::new(Expr::Binding("peer_id".into())),
                    })),
                }],
                updates: vec![
                    Update::MapInsert {
                        field: "raw_item_peer".into(),
                        key: Expr::Binding("raw_item_id".into()),
                        value: Expr::Binding("peer_id".into()),
                    },
                    Update::MapInsert {
                        field: "raw_item_kind".into(),
                        key: Expr::Binding("raw_item_id".into()),
                        value: Expr::Binding("raw_kind".into()),
                    },
                    Update::MapInsert {
                        field: "trusted_snapshot".into(),
                        key: Expr::Binding("raw_item_id".into()),
                        value: Expr::Bool(false),
                    },
                ],
                to: "Dropped".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "SubmitTypedPeerInputDelivered".into(),
                from: vec!["Received".into()],
                on: InputMatch {
                    variant: "SubmitTypedPeerInput".into(),
                    bindings: vec!["raw_item_id".into()],
                },
                guards: vec![
                    Guard {
                        name: "item_was_queued".into(),
                        expr: Expr::Contains {
                            collection: Box::new(Expr::Field("submission_queue".into())),
                            value: Box::new(Expr::Binding("raw_item_id".into())),
                        },
                    },
                    Guard {
                        name: "item_was_classified".into(),
                        expr: Expr::Contains {
                            collection: Box::new(Expr::MapKeys(Box::new(Expr::Field(
                                "classified_as".into(),
                            )))),
                            value: Box::new(Expr::Binding("raw_item_id".into())),
                        },
                    },
                    Guard {
                        name: "delivery_drains_queue".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Len(Box::new(Expr::Field("submission_queue".into())))),
                            Box::new(Expr::U64(1)),
                        ),
                    },
                ],
                updates: vec![Update::SeqRemoveValue {
                    field: "submission_queue".into(),
                    value: Expr::Binding("raw_item_id".into()),
                }],
                to: "Delivered".into(),
                emit: vec![EffectEmit {
                    variant: "SubmitPeerInputCandidate".into(),
                    fields: IndexMap::from([
                        ("raw_item_id".into(), Expr::Binding("raw_item_id".into())),
                        (
                            "peer_input_class".into(),
                            Expr::MapGet {
                                map: Box::new(Expr::Field("classified_as".into())),
                                key: Box::new(Expr::Binding("raw_item_id".into())),
                            },
                        ),
                    ]),
                }],
            },
            TransitionSchema {
                name: "SubmitTypedPeerInputContinue".into(),
                from: vec!["Received".into()],
                on: InputMatch {
                    variant: "SubmitTypedPeerInput".into(),
                    bindings: vec!["raw_item_id".into()],
                },
                guards: vec![
                    Guard {
                        name: "item_was_queued".into(),
                        expr: Expr::Contains {
                            collection: Box::new(Expr::Field("submission_queue".into())),
                            value: Box::new(Expr::Binding("raw_item_id".into())),
                        },
                    },
                    Guard {
                        name: "item_was_classified".into(),
                        expr: Expr::Contains {
                            collection: Box::new(Expr::MapKeys(Box::new(Expr::Field(
                                "classified_as".into(),
                            )))),
                            value: Box::new(Expr::Binding("raw_item_id".into())),
                        },
                    },
                    Guard {
                        name: "delivery_leaves_more_work".into(),
                        expr: Expr::Gt(
                            Box::new(Expr::Len(Box::new(Expr::Field("submission_queue".into())))),
                            Box::new(Expr::U64(1)),
                        ),
                    },
                ],
                updates: vec![Update::SeqRemoveValue {
                    field: "submission_queue".into(),
                    value: Expr::Binding("raw_item_id".into()),
                }],
                to: "Received".into(),
                emit: vec![EffectEmit {
                    variant: "SubmitPeerInputCandidate".into(),
                    fields: IndexMap::from([
                        ("raw_item_id".into(), Expr::Binding("raw_item_id".into())),
                        (
                            "peer_input_class".into(),
                            Expr::MapGet {
                                map: Box::new(Expr::Field("classified_as".into())),
                                key: Box::new(Expr::Binding("raw_item_id".into())),
                            },
                        ),
                    ]),
                }],
            },
        ],
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
