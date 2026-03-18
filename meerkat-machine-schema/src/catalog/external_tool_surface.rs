use indexmap::IndexMap;

use crate::{
    EffectEmit, EnumSchema, Expr, FieldInit, FieldSchema, Guard, HelperSchema, InitSchema,
    InputMatch, InvariantSchema, MachineSchema, Quantifier, RustBinding, StateSchema,
    TransitionSchema, TypeRef, Update, VariantSchema,
};

pub fn external_tool_surface_machine() -> MachineSchema {
    MachineSchema {
        machine: "ExternalToolSurfaceMachine".into(),
        version: 1,
        rust: RustBinding {
            crate_name: "meerkat-mcp".into(),
            module: "machines::external_tool_surface".into(),
        },
        state: StateSchema {
            phase: EnumSchema {
                name: "ExternalToolSurfacePhase".into(),
                variants: vec![variant("Operating"), variant("Shutdown")],
            },
            fields: vec![
                field("known_surfaces", TypeRef::Set(Box::new(named("SurfaceId")))),
                field(
                    "visible_surfaces",
                    TypeRef::Set(Box::new(named("SurfaceId"))),
                ),
                field(
                    "base_state",
                    TypeRef::Map(
                        Box::new(named("SurfaceId")),
                        Box::new(named("SurfaceBaseState")),
                    ),
                ),
                field(
                    "pending_op",
                    TypeRef::Map(
                        Box::new(named("SurfaceId")),
                        Box::new(named("PendingSurfaceOp")),
                    ),
                ),
                field(
                    "staged_op",
                    TypeRef::Map(
                        Box::new(named("SurfaceId")),
                        Box::new(named("StagedSurfaceOp")),
                    ),
                ),
                field(
                    "inflight_calls",
                    TypeRef::Map(Box::new(named("SurfaceId")), Box::new(TypeRef::U64)),
                ),
                field(
                    "last_delta_operation",
                    TypeRef::Map(
                        Box::new(named("SurfaceId")),
                        Box::new(named("SurfaceDeltaOperation")),
                    ),
                ),
                field(
                    "last_delta_phase",
                    TypeRef::Map(
                        Box::new(named("SurfaceId")),
                        Box::new(named("SurfaceDeltaPhase")),
                    ),
                ),
            ],
            init: InitSchema {
                phase: "Operating".into(),
                fields: vec![
                    init("known_surfaces", Expr::EmptySet),
                    init("visible_surfaces", Expr::EmptySet),
                    init("base_state", Expr::EmptyMap),
                    init("pending_op", Expr::EmptyMap),
                    init("staged_op", Expr::EmptyMap),
                    init("inflight_calls", Expr::EmptyMap),
                    init("last_delta_operation", Expr::EmptyMap),
                    init("last_delta_phase", Expr::EmptyMap),
                ],
            },
            terminal_phases: vec!["Shutdown".into()],
        },
        inputs: EnumSchema {
            name: "ExternalToolSurfaceInput".into(),
            variants: vec![
                VariantSchema {
                    name: "StageAdd".into(),
                    fields: vec![field("surface_id", named("SurfaceId"))],
                },
                VariantSchema {
                    name: "StageRemove".into(),
                    fields: vec![field("surface_id", named("SurfaceId"))],
                },
                VariantSchema {
                    name: "StageReload".into(),
                    fields: vec![field("surface_id", named("SurfaceId"))],
                },
                VariantSchema {
                    name: "ApplyBoundary".into(),
                    fields: vec![
                        field("surface_id", named("SurfaceId")),
                        field("applied_at_turn", named("TurnNumber")),
                    ],
                },
                VariantSchema {
                    name: "PendingSucceeded".into(),
                    fields: vec![
                        field("surface_id", named("SurfaceId")),
                        field("applied_at_turn", named("TurnNumber")),
                    ],
                },
                VariantSchema {
                    name: "PendingFailed".into(),
                    fields: vec![
                        field("surface_id", named("SurfaceId")),
                        field("applied_at_turn", named("TurnNumber")),
                    ],
                },
                VariantSchema {
                    name: "CallStarted".into(),
                    fields: vec![field("surface_id", named("SurfaceId"))],
                },
                VariantSchema {
                    name: "CallFinished".into(),
                    fields: vec![field("surface_id", named("SurfaceId"))],
                },
                VariantSchema {
                    name: "FinalizeRemovalClean".into(),
                    fields: vec![
                        field("surface_id", named("SurfaceId")),
                        field("applied_at_turn", named("TurnNumber")),
                    ],
                },
                VariantSchema {
                    name: "FinalizeRemovalForced".into(),
                    fields: vec![
                        field("surface_id", named("SurfaceId")),
                        field("applied_at_turn", named("TurnNumber")),
                    ],
                },
                variant("Shutdown"),
            ],
        },
        effects: EnumSchema {
            name: "ExternalToolSurfaceEffect".into(),
            variants: vec![
                VariantSchema {
                    name: "ScheduleSurfaceCompletion".into(),
                    fields: vec![
                        field("surface_id", named("SurfaceId")),
                        field("operation", named("SurfaceDeltaOperation")),
                    ],
                },
                variant("RefreshVisibleSurfaceSet"),
                VariantSchema {
                    name: "EmitExternalToolDelta".into(),
                    fields: vec![
                        field("surface_id", named("SurfaceId")),
                        field("operation", named("SurfaceDeltaOperation")),
                        field("phase", named("SurfaceDeltaPhase")),
                        field("persisted", TypeRef::Bool),
                        field("applied_at_turn", named("TurnNumber")),
                    ],
                },
                VariantSchema {
                    name: "CloseSurfaceConnection".into(),
                    fields: vec![field("surface_id", named("SurfaceId"))],
                },
                VariantSchema {
                    name: "RejectSurfaceCall".into(),
                    fields: vec![
                        field("surface_id", named("SurfaceId")),
                        field("reason", TypeRef::String),
                    ],
                },
            ],
        },
        helpers: vec![
            lookup_string_helper("SurfaceBase", "base_state", "Absent", "SurfaceBaseState"),
            lookup_string_helper("PendingOp", "pending_op", "None", "PendingSurfaceOp"),
            lookup_string_helper("StagedOp", "staged_op", "None", "StagedSurfaceOp"),
            lookup_u64_helper("InflightCallCount", "inflight_calls"),
            lookup_string_helper(
                "LastDeltaOperation",
                "last_delta_operation",
                "None",
                "SurfaceDeltaOperation",
            ),
            lookup_string_helper(
                "LastDeltaPhase",
                "last_delta_phase",
                "None",
                "SurfaceDeltaPhase",
            ),
            HelperSchema {
                name: "IsVisible".into(),
                params: vec![field("surface_id", named("SurfaceId"))],
                returns: TypeRef::Bool,
                body: Expr::Contains {
                    collection: Box::new(Expr::Field("visible_surfaces".into())),
                    value: Box::new(binding("surface_id")),
                },
            },
        ],
        derived: vec![],
        invariants: vec![
            quantified_surface_invariant(
                "removing_or_removed_surfaces_are_not_visible",
                Expr::Or(vec![
                    Expr::And(vec![
                        eq(
                            call("SurfaceBase", vec![binding("surface_id")]),
                            string("Removing"),
                        ),
                        Expr::Not(Box::new(call("IsVisible", vec![binding("surface_id")]))),
                    ]),
                    Expr::And(vec![
                        eq(
                            call("SurfaceBase", vec![binding("surface_id")]),
                            string("Removed"),
                        ),
                        Expr::Not(Box::new(call("IsVisible", vec![binding("surface_id")]))),
                    ]),
                    Expr::And(vec![
                        neq(
                            call("SurfaceBase", vec![binding("surface_id")]),
                            string("Removing"),
                        ),
                        neq(
                            call("SurfaceBase", vec![binding("surface_id")]),
                            string("Removed"),
                        ),
                    ]),
                ]),
            ),
            quantified_surface_invariant(
                "visible_membership_matches_active_base_state",
                eq(
                    call("IsVisible", vec![binding("surface_id")]),
                    eq(
                        call("SurfaceBase", vec![binding("surface_id")]),
                        string("Active"),
                    ),
                ),
            ),
            quantified_surface_invariant(
                "removing_surfaces_have_no_pending_add_or_reload",
                Expr::Or(vec![
                    neq(
                        call("SurfaceBase", vec![binding("surface_id")]),
                        string("Removing"),
                    ),
                    eq(
                        call("PendingOp", vec![binding("surface_id")]),
                        string("None"),
                    ),
                ]),
            ),
            quantified_surface_invariant(
                "removed_surfaces_only_allow_pending_none_or_add",
                Expr::Or(vec![
                    neq(
                        call("SurfaceBase", vec![binding("surface_id")]),
                        string("Removed"),
                    ),
                    Expr::Or(vec![
                        eq(
                            call("PendingOp", vec![binding("surface_id")]),
                            string("None"),
                        ),
                        eq(
                            call("PendingOp", vec![binding("surface_id")]),
                            string("Add"),
                        ),
                    ]),
                ]),
            ),
            quantified_surface_invariant(
                "inflight_calls_only_exist_for_active_or_removing_surfaces",
                Expr::Or(vec![
                    eq(
                        call("InflightCallCount", vec![binding("surface_id")]),
                        Expr::U64(0),
                    ),
                    Expr::Or(vec![
                        eq(
                            call("SurfaceBase", vec![binding("surface_id")]),
                            string("Active"),
                        ),
                        eq(
                            call("SurfaceBase", vec![binding("surface_id")]),
                            string("Removing"),
                        ),
                    ]),
                ]),
            ),
            quantified_surface_invariant(
                "reload_pending_requires_active_base_state",
                Expr::Or(vec![
                    neq(
                        call("PendingOp", vec![binding("surface_id")]),
                        string("Reload"),
                    ),
                    eq(
                        call("SurfaceBase", vec![binding("surface_id")]),
                        string("Active"),
                    ),
                ]),
            ),
            quantified_surface_invariant(
                "removed_surfaces_have_zero_inflight_calls",
                Expr::Or(vec![
                    neq(
                        call("SurfaceBase", vec![binding("surface_id")]),
                        string("Removed"),
                    ),
                    eq(
                        call("InflightCallCount", vec![binding("surface_id")]),
                        Expr::U64(0),
                    ),
                ]),
            ),
            quantified_surface_invariant(
                "forced_delta_phase_is_always_a_remove_delta",
                Expr::Or(vec![
                    neq(
                        call("LastDeltaPhase", vec![binding("surface_id")]),
                        string("Forced"),
                    ),
                    eq(
                        call("LastDeltaOperation", vec![binding("surface_id")]),
                        string("Remove"),
                    ),
                ]),
            ),
        ],
        transitions: vec![
            stage_transition("StageAdd", "Add"),
            stage_transition("StageRemove", "Remove"),
            TransitionSchema {
                name: "StageReload".into(),
                from: vec!["Operating".into()],
                on: InputMatch {
                    variant: "StageReload".into(),
                    bindings: vec!["surface_id".into()],
                },
                guards: vec![Guard {
                    name: "surface_is_active".into(),
                    expr: eq(
                        call("SurfaceBase", vec![binding("surface_id")]),
                        string("Active"),
                    ),
                }],
                updates: vec![
                    track_surface("surface_id"),
                    Update::MapInsert {
                        field: "staged_op".into(),
                        key: binding("surface_id"),
                        value: string("Reload"),
                    },
                ],
                to: "Operating".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "ApplyBoundaryAdd".into(),
                from: vec!["Operating".into()],
                on: InputMatch {
                    variant: "ApplyBoundary".into(),
                    bindings: vec!["surface_id".into(), "applied_at_turn".into()],
                },
                guards: vec![
                    Guard {
                        name: "staged_add_present".into(),
                        expr: eq(call("StagedOp", vec![binding("surface_id")]), string("Add")),
                    },
                    Guard {
                        name: "base_state_accepts_add".into(),
                        expr: Expr::Or(vec![
                            eq(
                                call("SurfaceBase", vec![binding("surface_id")]),
                                string("Absent"),
                            ),
                            eq(
                                call("SurfaceBase", vec![binding("surface_id")]),
                                string("Active"),
                            ),
                            eq(
                                call("SurfaceBase", vec![binding("surface_id")]),
                                string("Removed"),
                            ),
                        ]),
                    },
                ],
                updates: vec![
                    track_surface("surface_id"),
                    set_map("pending_op", "surface_id", string("Add")),
                    set_map("staged_op", "surface_id", string("None")),
                    set_map("last_delta_operation", "surface_id", string("Add")),
                    set_map("last_delta_phase", "surface_id", string("Pending")),
                ],
                to: "Operating".into(),
                emit: vec![
                    schedule_completion("surface_id", "Add"),
                    emit_delta("surface_id", "Add", "Pending", false),
                ],
            },
            TransitionSchema {
                name: "ApplyBoundaryReload".into(),
                from: vec!["Operating".into()],
                on: InputMatch {
                    variant: "ApplyBoundary".into(),
                    bindings: vec!["surface_id".into(), "applied_at_turn".into()],
                },
                guards: vec![
                    Guard {
                        name: "staged_reload_present".into(),
                        expr: eq(
                            call("StagedOp", vec![binding("surface_id")]),
                            string("Reload"),
                        ),
                    },
                    Guard {
                        name: "reload_requires_active_base".into(),
                        expr: eq(
                            call("SurfaceBase", vec![binding("surface_id")]),
                            string("Active"),
                        ),
                    },
                ],
                updates: vec![
                    track_surface("surface_id"),
                    set_map("pending_op", "surface_id", string("Reload")),
                    set_map("staged_op", "surface_id", string("None")),
                    set_map("last_delta_operation", "surface_id", string("Reload")),
                    set_map("last_delta_phase", "surface_id", string("Pending")),
                ],
                to: "Operating".into(),
                emit: vec![
                    schedule_completion("surface_id", "Reload"),
                    emit_delta("surface_id", "Reload", "Pending", false),
                ],
            },
            TransitionSchema {
                name: "ApplyBoundaryRemoveDraining".into(),
                from: vec!["Operating".into()],
                on: InputMatch {
                    variant: "ApplyBoundary".into(),
                    bindings: vec!["surface_id".into(), "applied_at_turn".into()],
                },
                guards: vec![
                    Guard {
                        name: "staged_remove_present".into(),
                        expr: eq(
                            call("StagedOp", vec![binding("surface_id")]),
                            string("Remove"),
                        ),
                    },
                    Guard {
                        name: "remove_begins_from_active".into(),
                        expr: eq(
                            call("SurfaceBase", vec![binding("surface_id")]),
                            string("Active"),
                        ),
                    },
                ],
                updates: vec![
                    track_surface("surface_id"),
                    set_map("staged_op", "surface_id", string("None")),
                    set_map("pending_op", "surface_id", string("None")),
                    set_map("base_state", "surface_id", string("Removing")),
                    set_map("last_delta_operation", "surface_id", string("Remove")),
                    set_map("last_delta_phase", "surface_id", string("Draining")),
                    Update::SetRemove {
                        field: "visible_surfaces".into(),
                        value: binding("surface_id"),
                    },
                ],
                to: "Operating".into(),
                emit: vec![
                    EffectEmit {
                        variant: "RefreshVisibleSurfaceSet".into(),
                        fields: IndexMap::new(),
                    },
                    emit_delta("surface_id", "Remove", "Draining", false),
                ],
            },
            TransitionSchema {
                name: "ApplyBoundaryRemoveNoop".into(),
                from: vec!["Operating".into()],
                on: InputMatch {
                    variant: "ApplyBoundary".into(),
                    bindings: vec!["surface_id".into(), "applied_at_turn".into()],
                },
                guards: vec![
                    Guard {
                        name: "staged_remove_present".into(),
                        expr: eq(
                            call("StagedOp", vec![binding("surface_id")]),
                            string("Remove"),
                        ),
                    },
                    Guard {
                        name: "remove_not_starting_from_active".into(),
                        expr: neq(
                            call("SurfaceBase", vec![binding("surface_id")]),
                            string("Active"),
                        ),
                    },
                ],
                updates: vec![
                    track_surface("surface_id"),
                    set_map("staged_op", "surface_id", string("None")),
                    set_map("pending_op", "surface_id", string("None")),
                ],
                to: "Operating".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "PendingSucceededAdd".into(),
                from: vec!["Operating".into()],
                on: InputMatch {
                    variant: "PendingSucceeded".into(),
                    bindings: vec!["surface_id".into(), "applied_at_turn".into()],
                },
                guards: vec![Guard {
                    name: "add_is_pending".into(),
                    expr: eq(
                        call("PendingOp", vec![binding("surface_id")]),
                        string("Add"),
                    ),
                }],
                updates: vec![
                    track_surface("surface_id"),
                    set_map("pending_op", "surface_id", string("None")),
                    set_map("base_state", "surface_id", string("Active")),
                    set_map("last_delta_operation", "surface_id", string("Add")),
                    set_map("last_delta_phase", "surface_id", string("Applied")),
                    Update::SetInsert {
                        field: "visible_surfaces".into(),
                        value: binding("surface_id"),
                    },
                ],
                to: "Operating".into(),
                emit: vec![
                    EffectEmit {
                        variant: "RefreshVisibleSurfaceSet".into(),
                        fields: IndexMap::new(),
                    },
                    emit_delta("surface_id", "Add", "Applied", true),
                ],
            },
            TransitionSchema {
                name: "PendingSucceededReload".into(),
                from: vec!["Operating".into()],
                on: InputMatch {
                    variant: "PendingSucceeded".into(),
                    bindings: vec!["surface_id".into(), "applied_at_turn".into()],
                },
                guards: vec![Guard {
                    name: "reload_is_pending".into(),
                    expr: eq(
                        call("PendingOp", vec![binding("surface_id")]),
                        string("Reload"),
                    ),
                }],
                updates: vec![
                    track_surface("surface_id"),
                    set_map("pending_op", "surface_id", string("None")),
                    set_map("base_state", "surface_id", string("Active")),
                    set_map("last_delta_operation", "surface_id", string("Reload")),
                    set_map("last_delta_phase", "surface_id", string("Applied")),
                    Update::SetInsert {
                        field: "visible_surfaces".into(),
                        value: binding("surface_id"),
                    },
                ],
                to: "Operating".into(),
                emit: vec![
                    EffectEmit {
                        variant: "RefreshVisibleSurfaceSet".into(),
                        fields: IndexMap::new(),
                    },
                    emit_delta("surface_id", "Reload", "Applied", true),
                ],
            },
            TransitionSchema {
                name: "PendingFailedAdd".into(),
                from: vec!["Operating".into()],
                on: InputMatch {
                    variant: "PendingFailed".into(),
                    bindings: vec!["surface_id".into(), "applied_at_turn".into()],
                },
                guards: vec![Guard {
                    name: "add_is_pending".into(),
                    expr: eq(
                        call("PendingOp", vec![binding("surface_id")]),
                        string("Add"),
                    ),
                }],
                updates: vec![
                    track_surface("surface_id"),
                    set_map("pending_op", "surface_id", string("None")),
                    set_map("last_delta_operation", "surface_id", string("Add")),
                    set_map("last_delta_phase", "surface_id", string("Failed")),
                ],
                to: "Operating".into(),
                emit: vec![emit_delta("surface_id", "Add", "Failed", true)],
            },
            TransitionSchema {
                name: "PendingFailedReload".into(),
                from: vec!["Operating".into()],
                on: InputMatch {
                    variant: "PendingFailed".into(),
                    bindings: vec!["surface_id".into(), "applied_at_turn".into()],
                },
                guards: vec![Guard {
                    name: "reload_is_pending".into(),
                    expr: eq(
                        call("PendingOp", vec![binding("surface_id")]),
                        string("Reload"),
                    ),
                }],
                updates: vec![
                    track_surface("surface_id"),
                    set_map("pending_op", "surface_id", string("None")),
                    set_map("last_delta_operation", "surface_id", string("Reload")),
                    set_map("last_delta_phase", "surface_id", string("Failed")),
                ],
                to: "Operating".into(),
                emit: vec![emit_delta("surface_id", "Reload", "Failed", true)],
            },
            TransitionSchema {
                name: "CallStartedActive".into(),
                from: vec!["Operating".into()],
                on: InputMatch {
                    variant: "CallStarted".into(),
                    bindings: vec!["surface_id".into()],
                },
                guards: vec![Guard {
                    name: "surface_is_active".into(),
                    expr: eq(
                        call("SurfaceBase", vec![binding("surface_id")]),
                        string("Active"),
                    ),
                }],
                updates: vec![
                    track_surface("surface_id"),
                    set_map(
                        "inflight_calls",
                        "surface_id",
                        Expr::Add(
                            Box::new(call("InflightCallCount", vec![binding("surface_id")])),
                            Box::new(Expr::U64(1)),
                        ),
                    ),
                ],
                to: "Operating".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "CallStartedRejectWhileRemoving".into(),
                from: vec!["Operating".into()],
                on: InputMatch {
                    variant: "CallStarted".into(),
                    bindings: vec!["surface_id".into()],
                },
                guards: vec![Guard {
                    name: "surface_is_removing".into(),
                    expr: eq(
                        call("SurfaceBase", vec![binding("surface_id")]),
                        string("Removing"),
                    ),
                }],
                updates: vec![track_surface("surface_id")],
                to: "Operating".into(),
                emit: vec![reject_call("surface_id", "surface_draining")],
            },
            TransitionSchema {
                name: "CallStartedRejectWhileUnavailable".into(),
                from: vec!["Operating".into()],
                on: InputMatch {
                    variant: "CallStarted".into(),
                    bindings: vec!["surface_id".into()],
                },
                guards: vec![Guard {
                    name: "surface_is_not_dispatchable".into(),
                    expr: Expr::And(vec![
                        neq(
                            call("SurfaceBase", vec![binding("surface_id")]),
                            string("Active"),
                        ),
                        neq(
                            call("SurfaceBase", vec![binding("surface_id")]),
                            string("Removing"),
                        ),
                    ]),
                }],
                updates: vec![track_surface("surface_id")],
                to: "Operating".into(),
                emit: vec![reject_call("surface_id", "surface_unavailable")],
            },
            TransitionSchema {
                name: "CallFinishedActive".into(),
                from: vec!["Operating".into()],
                on: InputMatch {
                    variant: "CallFinished".into(),
                    bindings: vec!["surface_id".into()],
                },
                guards: vec![
                    Guard {
                        name: "surface_is_active".into(),
                        expr: eq(
                            call("SurfaceBase", vec![binding("surface_id")]),
                            string("Active"),
                        ),
                    },
                    Guard {
                        name: "has_inflight_calls".into(),
                        expr: Expr::Gt(
                            Box::new(call("InflightCallCount", vec![binding("surface_id")])),
                            Box::new(Expr::U64(0)),
                        ),
                    },
                ],
                updates: vec![
                    track_surface("surface_id"),
                    set_map(
                        "inflight_calls",
                        "surface_id",
                        Expr::Sub(
                            Box::new(call("InflightCallCount", vec![binding("surface_id")])),
                            Box::new(Expr::U64(1)),
                        ),
                    ),
                ],
                to: "Operating".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "CallFinishedRemoving".into(),
                from: vec!["Operating".into()],
                on: InputMatch {
                    variant: "CallFinished".into(),
                    bindings: vec!["surface_id".into()],
                },
                guards: vec![
                    Guard {
                        name: "surface_is_removing".into(),
                        expr: eq(
                            call("SurfaceBase", vec![binding("surface_id")]),
                            string("Removing"),
                        ),
                    },
                    Guard {
                        name: "has_inflight_calls".into(),
                        expr: Expr::Gt(
                            Box::new(call("InflightCallCount", vec![binding("surface_id")])),
                            Box::new(Expr::U64(0)),
                        ),
                    },
                ],
                updates: vec![
                    track_surface("surface_id"),
                    set_map(
                        "inflight_calls",
                        "surface_id",
                        Expr::Sub(
                            Box::new(call("InflightCallCount", vec![binding("surface_id")])),
                            Box::new(Expr::U64(1)),
                        ),
                    ),
                ],
                to: "Operating".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "FinalizeRemovalClean".into(),
                from: vec!["Operating".into()],
                on: InputMatch {
                    variant: "FinalizeRemovalClean".into(),
                    bindings: vec!["surface_id".into(), "applied_at_turn".into()],
                },
                guards: vec![
                    Guard {
                        name: "surface_is_removing".into(),
                        expr: eq(
                            call("SurfaceBase", vec![binding("surface_id")]),
                            string("Removing"),
                        ),
                    },
                    Guard {
                        name: "no_inflight_calls_remain".into(),
                        expr: eq(
                            call("InflightCallCount", vec![binding("surface_id")]),
                            Expr::U64(0),
                        ),
                    },
                ],
                updates: vec![
                    track_surface("surface_id"),
                    set_map("base_state", "surface_id", string("Removed")),
                    set_map("pending_op", "surface_id", string("None")),
                    set_map("last_delta_operation", "surface_id", string("Remove")),
                    set_map("last_delta_phase", "surface_id", string("Applied")),
                    Update::SetRemove {
                        field: "visible_surfaces".into(),
                        value: binding("surface_id"),
                    },
                ],
                to: "Operating".into(),
                emit: vec![
                    close_surface("surface_id"),
                    EffectEmit {
                        variant: "RefreshVisibleSurfaceSet".into(),
                        fields: IndexMap::new(),
                    },
                    emit_delta("surface_id", "Remove", "Applied", true),
                ],
            },
            TransitionSchema {
                name: "FinalizeRemovalForced".into(),
                from: vec!["Operating".into()],
                on: InputMatch {
                    variant: "FinalizeRemovalForced".into(),
                    bindings: vec!["surface_id".into(), "applied_at_turn".into()],
                },
                guards: vec![Guard {
                    name: "surface_is_removing".into(),
                    expr: eq(
                        call("SurfaceBase", vec![binding("surface_id")]),
                        string("Removing"),
                    ),
                }],
                updates: vec![
                    track_surface("surface_id"),
                    set_map("base_state", "surface_id", string("Removed")),
                    set_map("pending_op", "surface_id", string("None")),
                    set_map("inflight_calls", "surface_id", Expr::U64(0)),
                    set_map("last_delta_operation", "surface_id", string("Remove")),
                    set_map("last_delta_phase", "surface_id", string("Forced")),
                    Update::SetRemove {
                        field: "visible_surfaces".into(),
                        value: binding("surface_id"),
                    },
                ],
                to: "Operating".into(),
                emit: vec![
                    close_surface("surface_id"),
                    EffectEmit {
                        variant: "RefreshVisibleSurfaceSet".into(),
                        fields: IndexMap::new(),
                    },
                    emit_delta("surface_id", "Remove", "Forced", true),
                ],
            },
            TransitionSchema {
                name: "Shutdown".into(),
                from: vec!["Operating".into(), "Shutdown".into()],
                on: InputMatch {
                    variant: "Shutdown".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![
                    Update::Assign {
                        field: "known_surfaces".into(),
                        expr: Expr::EmptySet,
                    },
                    Update::Assign {
                        field: "visible_surfaces".into(),
                        expr: Expr::EmptySet,
                    },
                    Update::Assign {
                        field: "base_state".into(),
                        expr: Expr::EmptyMap,
                    },
                    Update::Assign {
                        field: "pending_op".into(),
                        expr: Expr::EmptyMap,
                    },
                    Update::Assign {
                        field: "staged_op".into(),
                        expr: Expr::EmptyMap,
                    },
                    Update::Assign {
                        field: "inflight_calls".into(),
                        expr: Expr::EmptyMap,
                    },
                    Update::Assign {
                        field: "last_delta_operation".into(),
                        expr: Expr::EmptyMap,
                    },
                    Update::Assign {
                        field: "last_delta_phase".into(),
                        expr: Expr::EmptyMap,
                    },
                ],
                to: "Shutdown".into(),
                emit: vec![],
            },
        ],
    }
}

fn stage_transition(name: &str, op: &str) -> TransitionSchema {
    TransitionSchema {
        name: name.into(),
        from: vec!["Operating".into()],
        on: InputMatch {
            variant: name.into(),
            bindings: vec!["surface_id".into()],
        },
        guards: vec![],
        updates: vec![
            track_surface("surface_id"),
            set_map("staged_op", "surface_id", string(op)),
        ],
        to: "Operating".into(),
        emit: vec![],
    }
}

fn quantified_surface_invariant(name: &str, body: Expr) -> InvariantSchema {
    InvariantSchema {
        name: name.into(),
        expr: Expr::Quantified {
            quantifier: Quantifier::All,
            binding: "surface_id".into(),
            over: Box::new(Expr::Field("known_surfaces".into())),
            body: Box::new(body),
        },
    }
}

fn lookup_string_helper(
    name: &str,
    map_field: &str,
    default_value: &str,
    return_type: &str,
) -> HelperSchema {
    let key = binding("surface_id");
    let has_key = Expr::Contains {
        collection: Box::new(Expr::MapKeys(Box::new(Expr::Field(map_field.into())))),
        value: Box::new(key.clone()),
    };
    let lookup = Expr::MapGet {
        map: Box::new(Expr::Field(map_field.into())),
        key: Box::new(key),
    };
    HelperSchema {
        name: name.into(),
        params: vec![field("surface_id", named("SurfaceId"))],
        returns: named(return_type),
        body: Expr::IfElse {
            condition: Box::new(Expr::Not(Box::new(has_key))),
            then_expr: Box::new(string(default_value)),
            else_expr: Box::new(lookup),
        },
    }
}

fn lookup_u64_helper(name: &str, map_field: &str) -> HelperSchema {
    let key = binding("surface_id");
    let has_key = Expr::Contains {
        collection: Box::new(Expr::MapKeys(Box::new(Expr::Field(map_field.into())))),
        value: Box::new(key.clone()),
    };
    let lookup = Expr::MapGet {
        map: Box::new(Expr::Field(map_field.into())),
        key: Box::new(key),
    };
    HelperSchema {
        name: name.into(),
        params: vec![field("surface_id", named("SurfaceId"))],
        returns: TypeRef::U64,
        body: Expr::IfElse {
            condition: Box::new(Expr::Not(Box::new(has_key))),
            then_expr: Box::new(Expr::U64(0)),
            else_expr: Box::new(lookup),
        },
    }
}

fn schedule_completion(surface_binding: &str, operation: &str) -> EffectEmit {
    EffectEmit {
        variant: "ScheduleSurfaceCompletion".into(),
        fields: IndexMap::from([
            ("surface_id".into(), binding(surface_binding)),
            ("operation".into(), string(operation)),
        ]),
    }
}

fn emit_delta(surface_binding: &str, operation: &str, phase: &str, persisted: bool) -> EffectEmit {
    EffectEmit {
        variant: "EmitExternalToolDelta".into(),
        fields: IndexMap::from([
            ("surface_id".into(), binding(surface_binding)),
            ("operation".into(), string(operation)),
            ("phase".into(), string(phase)),
            ("persisted".into(), Expr::Bool(persisted)),
            ("applied_at_turn".into(), binding("applied_at_turn")),
        ]),
    }
}

fn reject_call(surface_binding: &str, reason: &str) -> EffectEmit {
    EffectEmit {
        variant: "RejectSurfaceCall".into(),
        fields: IndexMap::from([
            ("surface_id".into(), binding(surface_binding)),
            ("reason".into(), string(reason)),
        ]),
    }
}

fn close_surface(surface_binding: &str) -> EffectEmit {
    EffectEmit {
        variant: "CloseSurfaceConnection".into(),
        fields: IndexMap::from([("surface_id".into(), binding(surface_binding))]),
    }
}

fn track_surface(binding_name: &str) -> Update {
    Update::SetInsert {
        field: "known_surfaces".into(),
        value: binding(binding_name),
    }
}

fn set_map(field_name: &str, key_binding: &str, value: Expr) -> Update {
    Update::MapInsert {
        field: field_name.into(),
        key: binding(key_binding),
        value,
    }
}

fn named(name: &str) -> TypeRef {
    TypeRef::Named(name.into())
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

fn call(helper: &str, args: Vec<Expr>) -> Expr {
    Expr::Call {
        helper: helper.into(),
        args,
    }
}

fn binding(name: &str) -> Expr {
    Expr::Binding(name.into())
}

fn string(value: &str) -> Expr {
    Expr::String(value.into())
}

fn eq(left: Expr, right: Expr) -> Expr {
    Expr::Eq(Box::new(left), Box::new(right))
}

fn neq(left: Expr, right: Expr) -> Expr {
    Expr::Neq(Box::new(left), Box::new(right))
}
