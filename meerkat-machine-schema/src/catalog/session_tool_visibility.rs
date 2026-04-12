use crate::{
    EnumSchema, Expr, FieldInit, FieldSchema, Guard, HelperSchema, InitSchema, InputMatch,
    InvariantSchema, MachineSchema, RustBinding, StateSchema, TransitionSchema, TypeRef, Update,
    VariantSchema,
};

pub fn session_tool_visibility_machine() -> MachineSchema {
    MachineSchema {
        machine: "SessionToolVisibilityMachine".into(),
        version: 1,
        rust: RustBinding {
            crate_name: "meerkat-core".into(),
            module: "generated::session_tool_visibility".into(),
        },
        state: StateSchema {
            phase: EnumSchema {
                name: "SessionToolVisibilityPhase".into(),
                variants: vec![variant("Operating")],
            },
            fields: vec![
                field("inherited_base_filter", named("ToolFilter")),
                field("active_filter", named("ToolFilter")),
                field("staged_filter", named("ToolFilter")),
                field(
                    "active_requested_deferred_names",
                    TypeRef::Set(Box::new(TypeRef::String)),
                ),
                field(
                    "staged_requested_deferred_names",
                    TypeRef::Set(Box::new(TypeRef::String)),
                ),
                field(
                    "requested_witnesses",
                    TypeRef::Map(
                        Box::new(TypeRef::String),
                        Box::new(named("ToolVisibilityWitness")),
                    ),
                ),
                field(
                    "filter_witnesses",
                    TypeRef::Map(
                        Box::new(TypeRef::String),
                        Box::new(named("ToolVisibilityWitness")),
                    ),
                ),
                field("active_revision", TypeRef::U64),
                field("staged_revision", TypeRef::U64),
            ],
            init: InitSchema {
                phase: "Operating".into(),
                fields: vec![
                    init("inherited_base_filter", tool_filter_all()),
                    init("active_filter", tool_filter_all()),
                    init("staged_filter", tool_filter_all()),
                    init("active_requested_deferred_names", Expr::EmptySet),
                    init("staged_requested_deferred_names", Expr::EmptySet),
                    init("requested_witnesses", Expr::EmptyMap),
                    init("filter_witnesses", Expr::EmptyMap),
                    init("active_revision", Expr::U64(0)),
                    init("staged_revision", Expr::U64(0)),
                ],
            },
            terminal_phases: vec![],
        },
        inputs: EnumSchema {
            name: "SessionToolVisibilityInput".into(),
            variants: vec![
                VariantSchema {
                    name: "StagePersistentFilter".into(),
                    fields: vec![
                        field("filter", named("ToolFilter")),
                        field(
                            "witnesses",
                            TypeRef::Map(
                                Box::new(TypeRef::String),
                                Box::new(named("ToolVisibilityWitness")),
                            ),
                        ),
                    ],
                },
                VariantSchema {
                    name: "RequestDeferredTools".into(),
                    fields: vec![
                        field("names", TypeRef::Set(Box::new(TypeRef::String))),
                        field(
                            "witnesses",
                            TypeRef::Map(
                                Box::new(TypeRef::String),
                                Box::new(named("ToolVisibilityWitness")),
                            ),
                        ),
                    ],
                },
                variant("ApplyBoundary"),
            ],
        },
        effects: EnumSchema {
            name: "SessionToolVisibilityEffect".into(),
            variants: vec![],
        },
        helpers: vec![
            HelperSchema {
                name: "HasPendingPromotion".into(),
                params: vec![],
                returns: TypeRef::Bool,
                body: Expr::Gt(
                    Box::new(Expr::Field("staged_revision".into())),
                    Box::new(Expr::Field("active_revision".into())),
                ),
            },
            HelperSchema {
                name: "RequestedWitnessKeys".into(),
                params: vec![],
                returns: TypeRef::Set(Box::new(TypeRef::String)),
                body: Expr::MapKeys(Box::new(Expr::Field("requested_witnesses".into()))),
            },
            HelperSchema {
                name: "FilterWitnessKeys".into(),
                params: vec![],
                returns: TypeRef::Set(Box::new(TypeRef::String)),
                body: Expr::MapKeys(Box::new(Expr::Field("filter_witnesses".into()))),
            },
        ],
        derived: vec![],
        invariants: vec![
            InvariantSchema {
                name: "active_revision_not_ahead_of_staged".into(),
                expr: Expr::Lte(
                    Box::new(Expr::Field("active_revision".into())),
                    Box::new(Expr::Field("staged_revision".into())),
                ),
            },
            InvariantSchema {
                name: "active_requested_names_subset_of_staged".into(),
                expr: Expr::Quantified {
                    quantifier: crate::Quantifier::All,
                    binding: "name".into(),
                    over: Box::new(Expr::Field("active_requested_deferred_names".into())),
                    body: Box::new(Expr::Contains {
                        collection: Box::new(Expr::Field("staged_requested_deferred_names".into())),
                        value: Box::new(Expr::Binding("name".into())),
                    }),
                },
            },
            InvariantSchema {
                name: "equal_revision_means_equal_active_and_staged_state".into(),
                expr: Expr::Or(vec![
                    Expr::Neq(
                        Box::new(Expr::Field("active_revision".into())),
                        Box::new(Expr::Field("staged_revision".into())),
                    ),
                    Expr::And(vec![
                        Expr::Eq(
                            Box::new(Expr::Field("active_filter".into())),
                            Box::new(Expr::Field("staged_filter".into())),
                        ),
                        Expr::Eq(
                            Box::new(Expr::Field("active_requested_deferred_names".into())),
                            Box::new(Expr::Field("staged_requested_deferred_names".into())),
                        ),
                    ]),
                ]),
            },
        ],
        transitions: vec![
            TransitionSchema {
                name: "StagePersistentFilter".into(),
                from: vec!["Operating".into()],
                on: InputMatch {
                    variant: "StagePersistentFilter".into(),
                    bindings: vec!["filter".into(), "witnesses".into()],
                },
                guards: vec![],
                updates: vec![
                    Update::Assign {
                        field: "staged_filter".into(),
                        expr: Expr::Binding("filter".into()),
                    },
                    Update::ForEach {
                        binding: "name".into(),
                        over: Expr::MapKeys(Box::new(Expr::Binding("witnesses".into()))),
                        updates: vec![Update::MapInsert {
                            field: "filter_witnesses".into(),
                            key: Expr::Binding("name".into()),
                            value: Expr::MapGet {
                                map: Box::new(Expr::Binding("witnesses".into())),
                                key: Box::new(Expr::Binding("name".into())),
                            },
                        }],
                    },
                    Update::Increment {
                        field: "staged_revision".into(),
                        amount: 1,
                    },
                ],
                to: "Operating".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "RequestDeferredTools".into(),
                from: vec!["Operating".into()],
                on: InputMatch {
                    variant: "RequestDeferredTools".into(),
                    bindings: vec!["names".into(), "witnesses".into()],
                },
                guards: vec![],
                updates: vec![
                    Update::ForEach {
                        binding: "name".into(),
                        over: Expr::Binding("names".into()),
                        updates: vec![Update::SetInsert {
                            field: "staged_requested_deferred_names".into(),
                            value: Expr::Binding("name".into()),
                        }],
                    },
                    Update::ForEach {
                        binding: "name".into(),
                        over: Expr::MapKeys(Box::new(Expr::Binding("witnesses".into()))),
                        updates: vec![Update::MapInsert {
                            field: "requested_witnesses".into(),
                            key: Expr::Binding("name".into()),
                            value: Expr::MapGet {
                                map: Box::new(Expr::Binding("witnesses".into())),
                                key: Box::new(Expr::Binding("name".into())),
                            },
                        }],
                    },
                    Update::Increment {
                        field: "staged_revision".into(),
                        amount: 1,
                    },
                ],
                to: "Operating".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "ApplyBoundaryPromote".into(),
                from: vec!["Operating".into()],
                on: InputMatch {
                    variant: "ApplyBoundary".into(),
                    bindings: vec![],
                },
                guards: vec![Guard {
                    name: "has_pending_promotion".into(),
                    expr: Expr::Call {
                        helper: "HasPendingPromotion".into(),
                        args: vec![],
                    },
                }],
                updates: vec![
                    Update::Assign {
                        field: "active_filter".into(),
                        expr: Expr::Field("staged_filter".into()),
                    },
                    Update::Assign {
                        field: "active_requested_deferred_names".into(),
                        expr: Expr::Field("staged_requested_deferred_names".into()),
                    },
                    Update::Assign {
                        field: "active_revision".into(),
                        expr: Expr::Field("staged_revision".into()),
                    },
                ],
                to: "Operating".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "ApplyBoundaryNoop".into(),
                from: vec!["Operating".into()],
                on: InputMatch {
                    variant: "ApplyBoundary".into(),
                    bindings: vec![],
                },
                guards: vec![Guard {
                    name: "no_pending_promotion".into(),
                    expr: Expr::Not(Box::new(Expr::Call {
                        helper: "HasPendingPromotion".into(),
                        args: vec![],
                    })),
                }],
                updates: vec![],
                to: "Operating".into(),
                emit: vec![],
            },
        ],
        ci_step_limit: None,
        effect_dispositions: vec![],
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

fn named(name: &str) -> TypeRef {
    TypeRef::Named(name.into())
}

fn tool_filter_all() -> Expr {
    Expr::NamedVariant {
        enum_name: "ToolFilter".into(),
        variant: "All".into(),
    }
}
