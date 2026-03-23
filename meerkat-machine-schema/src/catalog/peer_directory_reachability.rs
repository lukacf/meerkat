use crate::{
    EnumSchema, Expr, FieldInit, FieldSchema, Guard, InitSchema, InputMatch, InvariantSchema,
    MachineSchema, Quantifier, RustBinding, StateSchema, TransitionSchema, TypeRef, Update,
    VariantSchema,
};

pub fn peer_directory_reachability_machine() -> MachineSchema {
    MachineSchema {
        machine: "PeerDirectoryReachabilityMachine".into(),
        version: 1,
        rust: RustBinding {
            crate_name: "meerkat-comms".into(),
            module: "generated::peer_directory_reachability".into(),
        },
        state: StateSchema {
            phase: EnumSchema {
                name: "PeerDirectoryReachabilityPhase".into(),
                variants: vec![variant("Tracking")],
            },
            fields: vec![
                field(
                    "resolved_keys",
                    TypeRef::Set(Box::new(TypeRef::Named("ReachabilityKey".into()))),
                ),
                field(
                    "reachability",
                    TypeRef::Map(
                        Box::new(TypeRef::Named("ReachabilityKey".into())),
                        Box::new(TypeRef::Named("PeerReachability".into())),
                    ),
                ),
                field(
                    "last_reason",
                    TypeRef::Map(
                        Box::new(TypeRef::Named("ReachabilityKey".into())),
                        Box::new(TypeRef::Option(Box::new(TypeRef::Named(
                            "PeerReachabilityReason".into(),
                        )))),
                    ),
                ),
            ],
            init: InitSchema {
                phase: "Tracking".into(),
                fields: vec![
                    init("resolved_keys", Expr::EmptySet),
                    init("reachability", Expr::EmptyMap),
                    init("last_reason", Expr::EmptyMap),
                ],
            },
            terminal_phases: vec![],
        },
        inputs: EnumSchema {
            name: "PeerDirectoryReachabilityInput".into(),
            variants: vec![
                VariantSchema {
                    name: "ReconcileResolvedDirectory".into(),
                    fields: vec![
                        field(
                            "keys",
                            TypeRef::Set(Box::new(TypeRef::Named("ReachabilityKey".into()))),
                        ),
                        field(
                            "reachability",
                            TypeRef::Map(
                                Box::new(TypeRef::Named("ReachabilityKey".into())),
                                Box::new(TypeRef::Named("PeerReachability".into())),
                            ),
                        ),
                        field(
                            "last_reason",
                            TypeRef::Map(
                                Box::new(TypeRef::Named("ReachabilityKey".into())),
                                Box::new(TypeRef::Option(Box::new(TypeRef::Named(
                                    "PeerReachabilityReason".into(),
                                )))),
                            ),
                        ),
                    ],
                },
                VariantSchema {
                    name: "RecordSendSucceeded".into(),
                    fields: vec![field("key", TypeRef::Named("ReachabilityKey".into()))],
                },
                VariantSchema {
                    name: "RecordSendFailed".into(),
                    fields: vec![
                        field("key", TypeRef::Named("ReachabilityKey".into())),
                        field("reason", TypeRef::Named("PeerReachabilityReason".into())),
                    ],
                },
            ],
        },
        effects: EnumSchema {
            name: "PeerDirectoryReachabilityEffect".into(),
            variants: vec![],
        },
        helpers: vec![],
        derived: vec![],
        invariants: vec![
            InvariantSchema {
                name: "reachability_keys_are_resolved".into(),
                expr: Expr::Quantified {
                    quantifier: Quantifier::All,
                    binding: "key".into(),
                    over: Box::new(Expr::MapKeys(Box::new(Expr::Field("reachability".into())))),
                    body: Box::new(Expr::Contains {
                        collection: Box::new(Expr::Field("resolved_keys".into())),
                        value: Box::new(Expr::Binding("key".into())),
                    }),
                },
            },
            InvariantSchema {
                name: "last_reason_keys_are_resolved".into(),
                expr: Expr::Quantified {
                    quantifier: Quantifier::All,
                    binding: "key".into(),
                    over: Box::new(Expr::MapKeys(Box::new(Expr::Field("last_reason".into())))),
                    body: Box::new(Expr::Contains {
                        collection: Box::new(Expr::Field("resolved_keys".into())),
                        value: Box::new(Expr::Binding("key".into())),
                    }),
                },
            },
        ],
        transitions: vec![
            TransitionSchema {
                name: "ReconcileResolvedDirectory".into(),
                from: vec!["Tracking".into()],
                on: InputMatch {
                    variant: "ReconcileResolvedDirectory".into(),
                    bindings: vec!["keys".into(), "reachability".into(), "last_reason".into()],
                },
                guards: vec![],
                updates: vec![
                    Update::Assign {
                        field: "resolved_keys".into(),
                        expr: Expr::Binding("keys".into()),
                    },
                    Update::Assign {
                        field: "reachability".into(),
                        expr: Expr::Binding("reachability".into()),
                    },
                    Update::Assign {
                        field: "last_reason".into(),
                        expr: Expr::Binding("last_reason".into()),
                    },
                ],
                to: "Tracking".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "RecordSendSucceeded".into(),
                from: vec!["Tracking".into()],
                on: InputMatch {
                    variant: "RecordSendSucceeded".into(),
                    bindings: vec!["key".into()],
                },
                guards: vec![Guard {
                    name: "key_is_resolved".into(),
                    expr: Expr::Contains {
                        collection: Box::new(Expr::Field("resolved_keys".into())),
                        value: Box::new(Expr::Binding("key".into())),
                    },
                }],
                updates: vec![
                    Update::MapInsert {
                        field: "reachability".into(),
                        key: Expr::Binding("key".into()),
                        value: peer_reachability(PeerReachabilityVariant::Reachable),
                    },
                    Update::MapInsert {
                        field: "last_reason".into(),
                        key: Expr::Binding("key".into()),
                        value: Expr::None,
                    },
                ],
                to: "Tracking".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "RecordSendFailed".into(),
                from: vec!["Tracking".into()],
                on: InputMatch {
                    variant: "RecordSendFailed".into(),
                    bindings: vec!["key".into(), "reason".into()],
                },
                guards: vec![Guard {
                    name: "key_is_resolved".into(),
                    expr: Expr::Contains {
                        collection: Box::new(Expr::Field("resolved_keys".into())),
                        value: Box::new(Expr::Binding("key".into())),
                    },
                }],
                updates: vec![
                    Update::MapInsert {
                        field: "reachability".into(),
                        key: Expr::Binding("key".into()),
                        value: peer_reachability(PeerReachabilityVariant::Unreachable),
                    },
                    Update::MapInsert {
                        field: "last_reason".into(),
                        key: Expr::Binding("key".into()),
                        value: Expr::Some(Box::new(Expr::Binding("reason".into()))),
                    },
                ],
                to: "Tracking".into(),
                emit: vec![],
            },
        ],
        effect_dispositions: vec![],
    }
}

#[derive(Clone, Copy)]
enum PeerReachabilityVariant {
    Reachable,
    Unreachable,
}

fn peer_reachability(variant: PeerReachabilityVariant) -> Expr {
    Expr::NamedVariant {
        enum_name: "PeerReachability".into(),
        variant: match variant {
            PeerReachabilityVariant::Reachable => "Reachable".into(),
            PeerReachabilityVariant::Unreachable => "Unreachable".into(),
        },
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

fn variant(name: &str) -> VariantSchema {
    VariantSchema {
        name: name.into(),
        fields: vec![],
    }
}
