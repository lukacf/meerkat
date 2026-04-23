use indexmap::IndexMap;

use crate::{
    EffectDisposition, EffectDispositionRule, EffectEmit, EnumSchema, Expr, FieldInit, FieldSchema,
    Guard, HelperSchema, InvariantSchema, MachineSchema, NamedTypeBinding, Quantifier, RustBinding,
    StateSchema, TransitionSchema, TypeRef, Update, VariantSchema, TriggerMatch,
};
use crate::identity::{
    EffectVariantId, EnumTypeId, EnumVariantId, FieldId, InputVariantId, MachineId,
    NamedTypeId, PhaseId, ProtocolId, TransitionId,
};

// ---------------------------------------------------------------------------
// Expression helpers
// ---------------------------------------------------------------------------

/// Returns an Expr that is true if a dep node (by binding name) is in a terminal
/// state (Completed, Failed, Skipped, or Canceled). Ready and Running are NOT
/// terminal — this matters during StartFrame where nodes transition Pending→Ready
/// and we must not treat "Ready" as a completed prerequisite.
fn dep_is_terminal_expr(dep_binding: &str) -> Expr {
    Expr::Or(vec![
        Expr::Eq(
            Box::new(Expr::MapGet {
                map: Box::new(Expr::Field(FieldId::parse("node_status").expect("valid field slug"))),
                key: Box::new(Expr::Binding(dep_binding.into())),
            }),
            Box::new(Expr::NamedVariant {
                enum_name: EnumTypeId::parse("NodeRunStatus").expect("valid enum-type slug"),
                variant: EnumVariantId::parse("Completed").expect("valid enum-variant slug"),
            }),
        ),
        Expr::Eq(
            Box::new(Expr::MapGet {
                map: Box::new(Expr::Field(FieldId::parse("node_status").expect("valid field slug"))),
                key: Box::new(Expr::Binding(dep_binding.into())),
            }),
            Box::new(Expr::NamedVariant {
                enum_name: EnumTypeId::parse("NodeRunStatus").expect("valid enum-type slug"),
                variant: EnumVariantId::parse("Failed").expect("valid enum-variant slug"),
            }),
        ),
        Expr::Eq(
            Box::new(Expr::MapGet {
                map: Box::new(Expr::Field(FieldId::parse("node_status").expect("valid field slug"))),
                key: Box::new(Expr::Binding(dep_binding.into())),
            }),
            Box::new(Expr::NamedVariant {
                enum_name: EnumTypeId::parse("NodeRunStatus").expect("valid enum-type slug"),
                variant: EnumVariantId::parse("Skipped").expect("valid enum-variant slug"),
            }),
        ),
        Expr::Eq(
            Box::new(Expr::MapGet {
                map: Box::new(Expr::Field(FieldId::parse("node_status").expect("valid field slug"))),
                key: Box::new(Expr::Binding(dep_binding.into())),
            }),
            Box::new(Expr::NamedVariant {
                enum_name: EnumTypeId::parse("NodeRunStatus").expect("valid enum-type slug"),
                variant: EnumVariantId::parse("Canceled").expect("valid enum-variant slug"),
            }),
        ),
    ])
}

fn dependency_ids_expr(node_id_expr: Expr) -> Expr {
    Expr::SeqElements(Box::new(Expr::MapGet {
        map: Box::new(Expr::Field(FieldId::parse("node_dependencies").expect("valid field slug"))),
        key: Box::new(node_id_expr),
    }))
}

// ---------------------------------------------------------------------------
// Shared helpers: update blocks re-used across multiple transitions
// ---------------------------------------------------------------------------

/// Update block that seeds admission-eligible Pending nodes into ready_queue.
///
/// This is deliberately split into TWO sequential ForEach loops to produce
/// correct TLA+ semantics. The TLA codegen generates separate recursive
/// functions per field within a ForEach+Conditional: the first field's function
/// uses a live accumulator, while subsequent field functions receive the FINAL
/// computed value of earlier fields as a snapshot argument.
///
/// Step 1: Promote eligible Pending nodes to Ready in node_status.
/// Step 2: Append nodes that are NOW Ready (and not yet in queue) to ready_queue.
///
/// This split ensures that in TLA+, the ready_queue loop's "captured_node_status"
/// argument is the fully-computed node_status' (from Step 1), which has the
/// eligible nodes marked Ready — so the queue loop checks `== "Ready"` (not
/// "Pending") and correctly identifies them. The Rust runtime sees the same
/// behavior because Step 2's ForEach runs AFTER Step 1 has mutated node_status.
fn refresh_ready_frontier_updates() -> Vec<Update> {
    vec![
        // Step 1: Promote eligible Pending nodes to Ready.
        Update::ForEach {
            binding: "rf_node".into(),
            over: Expr::Field(FieldId::parse("ordered_nodes").expect("valid field slug")),
            updates: vec![Update::Conditional {
                condition: Expr::And(vec![
                    // Node must be Pending
                    Expr::Eq(
                        Box::new(Expr::MapGet {
                            map: Box::new(Expr::Field(FieldId::parse("node_status").expect("valid field slug"))),
                            key: Box::new(Expr::Binding("rf_node".into())),
                        }),
                        Box::new(Expr::NamedVariant {
                            enum_name: EnumTypeId::parse("NodeRunStatus").expect("valid enum-type slug"),
                            variant: EnumVariantId::parse("Pending").expect("valid enum-variant slug"),
                        }),
                    ),
                    // Dep conditions are satisfied for admission eligibility
                    Expr::Call {
                        helper: "NodeAdmissionEligible".into(),
                        args: vec![Expr::Binding("rf_node".into())],
                    },
                ]),
                then_updates: vec![Update::MapInsert {
                    field: FieldId::parse("node_status").expect("valid field slug"),
                    key: Expr::Binding("rf_node".into()),
                    value: Expr::NamedVariant {
                        enum_name: EnumTypeId::parse("NodeRunStatus").expect("valid enum-type slug"),
                        variant: EnumVariantId::parse("Ready").expect("valid enum-variant slug"),
                    },
                }],
                else_updates: vec![],
            }],
        },
        // Step 2: Append nodes that are NOW Ready (and not yet in queue) to ready_queue.
        // The condition uses node_status[rf_node] == "Ready" — after Step 1, the eligible
        // nodes have been promoted to Ready, so this correctly identifies them.
        // In TLA+, the codegen passes the FULLY COMPUTED node_status' from Step 1 as the
        // snapshot for this loop's condition, so "== Ready" matches correctly.
        Update::ForEach {
            binding: "rf_node".into(),
            over: Expr::Field(FieldId::parse("ordered_nodes").expect("valid field slug")),
            updates: vec![Update::Conditional {
                condition: Expr::And(vec![
                    // Node must be Ready (was just promoted in Step 1, or was already Ready)
                    Expr::Eq(
                        Box::new(Expr::MapGet {
                            map: Box::new(Expr::Field(FieldId::parse("node_status").expect("valid field slug"))),
                            key: Box::new(Expr::Binding("rf_node".into())),
                        }),
                        Box::new(Expr::NamedVariant {
                            enum_name: EnumTypeId::parse("NodeRunStatus").expect("valid enum-type slug"),
                            variant: EnumVariantId::parse("Ready").expect("valid enum-variant slug"),
                        }),
                    ),
                    // Node must NOT already be in ready_queue
                    Expr::Not(Box::new(Expr::Contains {
                        collection: Box::new(Expr::SeqElements(Box::new(Expr::Field(FieldId::parse("ready_queue").expect("valid field slug"))))),
                        value: Box::new(Expr::Binding("rf_node".into())),
                    })),
                ]),
                then_updates: vec![Update::SeqAppend {
                    field: FieldId::parse("ready_queue").expect("valid field slug"),
                    value: Expr::Binding("rf_node".into()),
                }],
                else_updates: vec![],
            }],
        },
    ]
}

/// Emit a ReadyFrontierChanged effect referencing the frame_id in state.
fn emit_ready_frontier_changed() -> EffectEmit {
    EffectEmit { variant: EffectVariantId::parse("ReadyFrontierChanged").expect("valid effect-variant slug"),
        fields: IndexMap::from([(FieldId::parse("frame_id").expect("valid field slug"), Expr::Field(FieldId::parse("frame_id").expect("valid field slug")))]),
    }
}

/// Emit a NodeExecutionReleased effect for a given node_id binding.
fn emit_node_execution_released(node_id_expr: Expr) -> EffectEmit {
    EffectEmit { variant: EffectVariantId::parse("NodeExecutionReleased").expect("valid effect-variant slug"),
        fields: IndexMap::from([
            (FieldId::parse("frame_id").expect("valid field slug"), Expr::Field(FieldId::parse("frame_id").expect("valid field slug"))),
            (FieldId::parse("node_id").expect("valid field slug"), node_id_expr),
        ]),
    }
}

fn emit_root_frame_terminal(effect_variant: &str) -> EffectEmit {
    EffectEmit {
        variant: EffectVariantId::parse(effect_variant).expect("valid effect-variant slug"),
        fields: IndexMap::from([(FieldId::parse("frame_id").expect("valid field slug"), Expr::Field(FieldId::parse("frame_id").expect("valid field slug")))]),
    }
}

fn emit_body_frame_terminal(effect_variant: &str) -> EffectEmit {
    EffectEmit {
        variant: EffectVariantId::parse(effect_variant).expect("valid effect-variant slug"),
        fields: IndexMap::from([
            (FieldId::parse("frame_id").expect("valid field slug"), Expr::Field(FieldId::parse("frame_id").expect("valid field slug"))),
            (FieldId::parse("loop_instance_id").expect("valid field slug"), Expr::Field(FieldId::parse("loop_instance_id").expect("valid field slug")),
            ),
            (FieldId::parse("iteration").expect("valid field slug"), Expr::Field(FieldId::parse("iteration").expect("valid field slug"))),
        ]),
    }
}

fn any_node_with_status_expr(status_variant: &str) -> Expr {
    Expr::Quantified {
        quantifier: Quantifier::Any,
        binding: "status_node".into(),
        over: Box::new(Expr::Field(FieldId::parse("tracked_nodes").expect("valid field slug"))),
        body: Box::new(Expr::Eq(
            Box::new(Expr::MapGet {
                map: Box::new(Expr::Field(FieldId::parse("node_status").expect("valid field slug"))),
                key: Box::new(Expr::Binding("status_node".into())),
            }),
            Box::new(Expr::NamedVariant {
                enum_name: EnumTypeId::parse("NodeRunStatus").expect("valid enum-type slug"),
                variant: EnumVariantId::parse(status_variant).expect("valid enum-variant slug"),
            }),
        )),
    }
}

fn frame_scope_expr(scope_variant: &str) -> Expr {
    Expr::Eq(
        Box::new(Expr::Field(FieldId::parse("frame_scope").expect("valid field slug"))),
        Box::new(Expr::NamedVariant {
            enum_name: EnumTypeId::parse("FrameScope").expect("valid enum-type slug"),
            variant: EnumVariantId::parse(scope_variant).expect("valid enum-variant slug"),
        }),
    )
}

/// Build updates for StartFrame: assign topology fields, init all nodes Pending,
/// then seed the ready frontier.
fn start_frame_updates() -> Vec<Update> {
    let mut updates = vec![
        Update::Assign {
            field: FieldId::parse("frame_id").expect("valid field slug"),
            expr: Expr::Binding("frame_id".into()),
        },
        Update::Assign {
            field: FieldId::parse("tracked_nodes").expect("valid field slug"),
            expr: Expr::Binding("tracked_nodes".into()),
        },
        Update::Assign {
            field: FieldId::parse("ordered_nodes").expect("valid field slug"),
            expr: Expr::Binding("ordered_nodes".into()),
        },
        Update::Assign {
            field: FieldId::parse("node_kind").expect("valid field slug"),
            expr: Expr::Binding("node_kind".into()),
        },
        Update::Assign {
            field: FieldId::parse("node_dependencies").expect("valid field slug"),
            expr: Expr::Binding("node_dependencies".into()),
        },
        Update::Assign {
            field: FieldId::parse("node_dependency_modes").expect("valid field slug"),
            expr: Expr::Binding("node_dependency_modes".into()),
        },
        Update::Assign {
            field: FieldId::parse("node_branches").expect("valid field slug"),
            expr: Expr::Binding("node_branches".into()),
        },
        // Initialize all nodes as Pending
        Update::ForEach {
            binding: "init_node".into(),
            over: Expr::Binding("tracked_nodes".into()),
            updates: vec![Update::MapInsert {
                field: FieldId::parse("node_status").expect("valid field slug"),
                key: Expr::Binding("init_node".into()),
                value: Expr::NamedVariant {
                    enum_name: EnumTypeId::parse("NodeRunStatus").expect("valid enum-type slug"),
                    variant: EnumVariantId::parse("Pending").expect("valid enum-variant slug"),
                },
            }],
        },
    ];
    // Seed the ready frontier (roots → Ready, appended to ready_queue)
    updates.extend(refresh_ready_frontier_updates());
    updates
}

fn start_root_frame_updates() -> Vec<Update> {
    let mut updates = vec![
        Update::Assign {
            field: FieldId::parse("frame_scope").expect("valid field slug"),
            expr: Expr::NamedVariant {
                enum_name: EnumTypeId::parse("FrameScope").expect("valid enum-type slug"),
                variant: EnumVariantId::parse("Root").expect("valid enum-variant slug"),
            },
        },
        Update::Assign {
            field: FieldId::parse("loop_instance_id").expect("valid field slug"),
            expr: Expr::String(String::new()),
        },
        Update::Assign {
            field: FieldId::parse("iteration").expect("valid field slug"),
            expr: Expr::U64(0),
        },
    ];
    updates.extend(start_frame_updates());
    updates
}

fn start_body_frame_updates() -> Vec<Update> {
    let mut updates = vec![
        Update::Assign {
            field: FieldId::parse("frame_scope").expect("valid field slug"),
            expr: Expr::NamedVariant {
                enum_name: EnumTypeId::parse("FrameScope").expect("valid enum-type slug"),
                variant: EnumVariantId::parse("Body").expect("valid enum-variant slug"),
            },
        },
        Update::Assign {
            field: FieldId::parse("loop_instance_id").expect("valid field slug"),
            expr: Expr::Binding("loop_instance_id".into()),
        },
        Update::Assign {
            field: FieldId::parse("iteration").expect("valid field slug"),
            expr: Expr::Binding("iteration".into()),
        },
    ];
    updates.extend(start_frame_updates());
    updates
}

/// Guard: ready_queue is non-empty
fn guard_queue_non_empty() -> Guard {
    Guard {
        name: "ready_queue_non_empty".into(),
        expr: Expr::Gt(
            Box::new(Expr::Len(Box::new(Expr::Field(FieldId::parse("ready_queue").expect("valid field slug"))))),
            Box::new(Expr::U64(0)),
        ),
    }
}

/// Guard: head of ready_queue is a Step node
fn guard_head_is_step() -> Guard {
    Guard {
        name: "head_is_step".into(),
        expr: Expr::Eq(
            Box::new(Expr::MapGet {
                map: Box::new(Expr::Field(FieldId::parse("node_kind").expect("valid field slug"))),
                key: Box::new(Expr::Head(Box::new(Expr::Field(FieldId::parse("ready_queue").expect("valid field slug"))))),
            }),
            Box::new(Expr::NamedVariant {
                enum_name: EnumTypeId::parse("FlowNodeKind").expect("valid enum-type slug"),
                variant: EnumVariantId::parse("Step").expect("valid enum-variant slug"),
            }),
        ),
    }
}

/// Guard: head of ready_queue is a Loop node
fn guard_head_is_loop() -> Guard {
    Guard {
        name: "head_is_loop".into(),
        expr: Expr::Eq(
            Box::new(Expr::MapGet {
                map: Box::new(Expr::Field(FieldId::parse("node_kind").expect("valid field slug"))),
                key: Box::new(Expr::Head(Box::new(Expr::Field(FieldId::parse("ready_queue").expect("valid field slug"))))),
            }),
            Box::new(Expr::NamedVariant {
                enum_name: EnumTypeId::parse("FlowNodeKind").expect("valid enum-type slug"),
                variant: EnumVariantId::parse("Loop").expect("valid enum-variant slug"),
            }),
        ),
    }
}

/// Guard: head node's deps are satisfied for a run (all Completed or no deps for All-mode;
/// any Completed for Any-mode), AND its branch (if any) has not already won.
fn guard_head_can_run() -> Guard {
    Guard {
        name: "head_deps_eligible_for_run".into(),
        expr: Expr::And(vec![
            // Branch must not have already won: the head node's branch (if any) is NOT in branch_winners.
            // For nodes without a branch (None), Contains(branch_winners, None) is always false because
            // branch_winners only contains BranchId strings — so this AND arm is a no-op for branchless nodes.
            Expr::Not(Box::new(Expr::Contains {
                collection: Box::new(Expr::Field(FieldId::parse("branch_winners").expect("valid field slug"))),
                value: Box::new(Expr::MapGet {
                    map: Box::new(Expr::Field(FieldId::parse("node_branches").expect("valid field slug"))),
                    key: Box::new(Expr::Head(Box::new(Expr::Field(FieldId::parse("ready_queue").expect("valid field slug"))))),
                }),
            })),
            // Dep satisfaction: any of the three eligibility cases.
            Expr::Or(vec![
                // No deps at all
                Expr::Eq(
                    Box::new(Expr::Len(Box::new(Expr::MapGet {
                        map: Box::new(Expr::Field(FieldId::parse("node_dependencies").expect("valid field slug"))),
                        key: Box::new(Expr::Head(Box::new(Expr::Field(FieldId::parse("ready_queue").expect("valid field slug"))))),
                    }))),
                    Box::new(Expr::U64(0)),
                ),
                // All-mode AND all deps Completed
                Expr::And(vec![
                    Expr::Eq(
                        Box::new(Expr::MapGet {
                            map: Box::new(Expr::Field(FieldId::parse("node_dependency_modes").expect("valid field slug"))),
                            key: Box::new(Expr::Head(Box::new(Expr::Field(FieldId::parse("ready_queue").expect("valid field slug"))))),
                        }),
                        Box::new(Expr::NamedVariant {
                            enum_name: EnumTypeId::parse("DependencyMode").expect("valid enum-type slug"),
                            variant: EnumVariantId::parse("All").expect("valid enum-variant slug"),
                        }),
                    ),
                    Expr::Call {
                        helper: "AllDepsCompleted".into(),
                        args: vec![Expr::Head(Box::new(Expr::Field(FieldId::parse("ready_queue").expect("valid field slug"))))],
                    },
                ]),
                // Any-mode AND any dep Completed
                Expr::And(vec![
                    Expr::Eq(
                        Box::new(Expr::MapGet {
                            map: Box::new(Expr::Field(FieldId::parse("node_dependency_modes").expect("valid field slug"))),
                            key: Box::new(Expr::Head(Box::new(Expr::Field(FieldId::parse("ready_queue").expect("valid field slug"))))),
                        }),
                        Box::new(Expr::NamedVariant {
                            enum_name: EnumTypeId::parse("DependencyMode").expect("valid enum-type slug"),
                            variant: EnumVariantId::parse("Any").expect("valid enum-variant slug"),
                        }),
                    ),
                    Expr::Call {
                        helper: "AnyDepCompleted".into(),
                        args: vec![Expr::Head(Box::new(Expr::Field(FieldId::parse("ready_queue").expect("valid field slug"))))],
                    },
                ]),
            ]),
        ]),
    }
}

/// Guard: head node should be skipped.
/// Fires when either:
///   (a) All-mode: any dep has a non-Completed terminal status (Failed, Skipped, Canceled), OR
///   (b) Branch-already-won: the head node belongs to a branch group and another node in
///       that group has already completed (its BranchId is in branch_winners).
fn guard_head_should_skip() -> Guard {
    Guard {
        name: "head_should_skip".into(),
        expr: Expr::Or(vec![
            // (a) All-mode AND at least one dep is Failed, Skipped, or Canceled (not Completed)
            Expr::And(vec![
                Expr::Eq(
                    Box::new(Expr::MapGet {
                        map: Box::new(Expr::Field(FieldId::parse("node_dependency_modes").expect("valid field slug"))),
                        key: Box::new(Expr::Head(Box::new(Expr::Field(FieldId::parse("ready_queue").expect("valid field slug"))))),
                    }),
                    Box::new(Expr::NamedVariant {
                        enum_name: EnumTypeId::parse("DependencyMode").expect("valid enum-type slug"),
                        variant: EnumVariantId::parse("All").expect("valid enum-variant slug"),
                    }),
                ),
                // At least one dep is in a non-Completed terminal state
                Expr::Quantified {
                    quantifier: Quantifier::Any,
                    binding: "dep_id".into(),
                    over: Box::new(dependency_ids_expr(Expr::Head(Box::new(Expr::Field(FieldId::parse("ready_queue").expect("valid field slug")))))),
                    body: Box::new(Expr::Or(vec![
                        Expr::Eq(
                            Box::new(Expr::MapGet {
                                map: Box::new(Expr::Field(FieldId::parse("node_status").expect("valid field slug"))),
                                key: Box::new(Expr::Binding("dep_id".into())),
                            }),
                            Box::new(Expr::NamedVariant {
                                enum_name: EnumTypeId::parse("NodeRunStatus").expect("valid enum-type slug"),
                                variant: EnumVariantId::parse("Failed").expect("valid enum-variant slug"),
                            }),
                        ),
                        Expr::Eq(
                            Box::new(Expr::MapGet {
                                map: Box::new(Expr::Field(FieldId::parse("node_status").expect("valid field slug"))),
                                key: Box::new(Expr::Binding("dep_id".into())),
                            }),
                            Box::new(Expr::NamedVariant {
                                enum_name: EnumTypeId::parse("NodeRunStatus").expect("valid enum-type slug"),
                                variant: EnumVariantId::parse("Skipped").expect("valid enum-variant slug"),
                            }),
                        ),
                        Expr::Eq(
                            Box::new(Expr::MapGet {
                                map: Box::new(Expr::Field(FieldId::parse("node_status").expect("valid field slug"))),
                                key: Box::new(Expr::Binding("dep_id".into())),
                            }),
                            Box::new(Expr::NamedVariant {
                                enum_name: EnumTypeId::parse("NodeRunStatus").expect("valid enum-type slug"),
                                variant: EnumVariantId::parse("Canceled").expect("valid enum-variant slug"),
                            }),
                        ),
                    ])),
                },
            ]),
            // (b) Branch-already-won: head node has a branch AND it's already in branch_winners.
            // Nodes without a branch have None as their branch value; branch_winners only contains
            // BranchId strings, so Contains(branch_winners, None) is always false — branchless
            // nodes are unaffected.
            Expr::Contains {
                collection: Box::new(Expr::Field(FieldId::parse("branch_winners").expect("valid field slug"))),
                value: Box::new(Expr::MapGet {
                    map: Box::new(Expr::Field(FieldId::parse("node_branches").expect("valid field slug"))),
                    key: Box::new(Expr::Head(Box::new(Expr::Field(FieldId::parse("ready_queue").expect("valid field slug"))))),
                }),
            },
        ]),
    }
}

/// Guard: head node should fail.
/// Any-mode: all deps are terminal AND none is Completed.
fn guard_head_should_fail() -> Guard {
    Guard {
        name: "head_should_fail".into(),
        expr: Expr::And(vec![
            Expr::Eq(
                Box::new(Expr::MapGet {
                    map: Box::new(Expr::Field(FieldId::parse("node_dependency_modes").expect("valid field slug"))),
                    key: Box::new(Expr::Head(Box::new(Expr::Field(FieldId::parse("ready_queue").expect("valid field slug"))))),
                }),
                Box::new(Expr::NamedVariant {
                    enum_name: EnumTypeId::parse("DependencyMode").expect("valid enum-type slug"),
                    variant: EnumVariantId::parse("Any").expect("valid enum-variant slug"),
                }),
            ),
            // All deps are in a terminal state (Completed/Failed/Skipped/Canceled)
            Expr::Quantified {
                quantifier: Quantifier::All,
                binding: "dep_id".into(),
                over: Box::new(dependency_ids_expr(Expr::Head(Box::new(Expr::Field(FieldId::parse("ready_queue").expect("valid field slug")))))),
                body: Box::new(dep_is_terminal_expr("dep_id")),
            },
            // No dep is Completed (otherwise would have been admitted as run)
            Expr::Not(Box::new(Expr::Call {
                helper: "AnyDepCompleted".into(),
                args: vec![Expr::Head(Box::new(Expr::Field(FieldId::parse("ready_queue").expect("valid field slug"))))],
            })),
        ]),
    }
}

/// Updates for AdmitNextReadyNode when admitted as Running (step or loop):
/// Sets node_status to Running for the head node, then pops it from the queue.
/// The last_admitted_node field is set first so effects can reference it.
fn admit_run_updates(status_variant: &str) -> Vec<Update> {
    vec![
        // Store the head node ID so effects can reference it after the pop
        Update::Assign {
            field: FieldId::parse("last_admitted_node").expect("valid field slug"),
            expr: Expr::Head(Box::new(Expr::Field(FieldId::parse("ready_queue").expect("valid field slug")))),
        },
        // Set status to Running
        Update::MapInsert {
            field: FieldId::parse("node_status").expect("valid field slug"),
            key: Expr::Head(Box::new(Expr::Field(FieldId::parse("ready_queue").expect("valid field slug")))),
            value: Expr::NamedVariant {
                enum_name: EnumTypeId::parse("NodeRunStatus").expect("valid enum-type slug"),
                variant: EnumVariantId::parse(status_variant).expect("valid enum-variant slug"),
            },
        },
        // Pop the head from the queue
        Update::SeqPopFront {
            field: FieldId::parse("ready_queue").expect("valid field slug"),
        },
    ]
}

/// Updates for AdmitNextReadyNode when admitted as terminal (Skipped/Failed):
/// Sets node_status, pops from queue, then refreshes the frontier.
fn admit_terminal_updates(status_variant: &str) -> Vec<Update> {
    let mut updates = vec![
        // Store the head node ID so effects can reference it after the pop
        Update::Assign {
            field: FieldId::parse("last_admitted_node").expect("valid field slug"),
            expr: Expr::Head(Box::new(Expr::Field(FieldId::parse("ready_queue").expect("valid field slug")))),
        },
        // Set terminal status
        Update::MapInsert {
            field: FieldId::parse("node_status").expect("valid field slug"),
            key: Expr::Head(Box::new(Expr::Field(FieldId::parse("ready_queue").expect("valid field slug")))),
            value: Expr::NamedVariant {
                enum_name: EnumTypeId::parse("NodeRunStatus").expect("valid enum-type slug"),
                variant: EnumVariantId::parse(status_variant).expect("valid enum-variant slug"),
            },
        },
        // Pop the head from the queue
        Update::SeqPopFront {
            field: FieldId::parse("ready_queue").expect("valid field slug"),
        },
    ];
    // Refresh frontier: any newly-eligible nodes become Ready
    updates.extend(refresh_ready_frontier_updates());
    updates
}

pub fn flow_frame_machine() -> MachineSchema {
    MachineSchema {
        machine: MachineId::parse("FlowFrameMachine").expect("valid machine slug"),
        version: 3,
        rust: RustBinding {
            crate_name: "meerkat-mob".into(),
            module: "generated::flow_frame".into(),
        },
        state: StateSchema {
            phase: EnumSchema {
                name: "FlowFrameStatus".into(),
                variants: vec![
                    variant("Absent"),
                    variant("Running"),
                    variant("Completed"),
                    variant("Failed"),
                    variant("Canceled"),
                ],
            },
            fields: vec![
                // Frame identity (stored so effects can reference it without bindings)
                field("frame_id", TypeRef::Named(NamedTypeId::parse("FrameId").expect("valid named-type slug"))),
                field("frame_scope", TypeRef::Enum(EnumTypeId::parse("FrameScope").expect("valid enum-type slug"))),
                field("loop_instance_id", TypeRef::Named(NamedTypeId::parse("LoopInstanceId").expect("valid named-type slug"))),
                field("iteration", TypeRef::U32),
                // Transient scratch: the node ID admitted in the most recent
                // AdmitNextReadyNode transition. Set BEFORE SeqPopFront so that
                // effects emitted in the same transition can reference it via
                // Expr::Field("last_admitted_node"). Stale between transitions —
                // do not read this field outside of an AdmitNextReadyNode context.
                field("last_admitted_node", TypeRef::Named(NamedTypeId::parse("FlowNodeId").expect("valid named-type slug"))),
                // Node graph
                field(
                    "tracked_nodes",
                    TypeRef::Set(Box::new(TypeRef::Named(NamedTypeId::parse("FlowNodeId").expect("valid named-type slug")))),
                ),
                field(
                    "ordered_nodes",
                    TypeRef::Seq(Box::new(TypeRef::Named(NamedTypeId::parse("FlowNodeId").expect("valid named-type slug")))),
                ),
                field(
                    "node_kind",
                    TypeRef::Map(
                        Box::new(TypeRef::Named(NamedTypeId::parse("FlowNodeId").expect("valid named-type slug"))),
                        Box::new(TypeRef::Enum(EnumTypeId::parse("FlowNodeKind").expect("valid enum-type slug"))),
                    ),
                ),
                // Dependency graph
                field(
                    "node_dependencies",
                    TypeRef::Map(
                        Box::new(TypeRef::Named(NamedTypeId::parse("FlowNodeId").expect("valid named-type slug"))),
                        Box::new(TypeRef::Seq(Box::new(TypeRef::Named(NamedTypeId::parse("FlowNodeId").expect("valid named-type slug"))))),
                    ),
                ),
                field(
                    "node_dependency_modes",
                    TypeRef::Map(
                        Box::new(TypeRef::Named(NamedTypeId::parse("FlowNodeId").expect("valid named-type slug"))),
                        Box::new(TypeRef::Enum(EnumTypeId::parse("DependencyMode").expect("valid enum-type slug"))),
                    ),
                ),
                field(
                    "node_branches",
                    TypeRef::Map(
                        Box::new(TypeRef::Named(NamedTypeId::parse("FlowNodeId").expect("valid named-type slug"))),
                        Box::new(TypeRef::Option(Box::new(TypeRef::Named(NamedTypeId::parse("BranchId").expect("valid named-type slug"))))),
                    ),
                ),
                // Branch winner tracking: once a node in a branch group completes,
                // its BranchId is added here to suppress all sibling branch nodes.
                field(
                    "branch_winners",
                    TypeRef::Set(Box::new(TypeRef::Named(NamedTypeId::parse("BranchId").expect("valid named-type slug")))),
                ),
                // Status
                field(
                    "node_status",
                    TypeRef::Map(
                        Box::new(TypeRef::Named(NamedTypeId::parse("FlowNodeId").expect("valid named-type slug"))),
                        Box::new(TypeRef::Enum(EnumTypeId::parse("NodeRunStatus").expect("valid enum-type slug"))),
                    ),
                ),
                // Scheduler
                field(
                    "ready_queue",
                    TypeRef::Seq(Box::new(TypeRef::Named(NamedTypeId::parse("FlowNodeId").expect("valid named-type slug")))),
                ),
                // Output tracking
                field(
                    "output_recorded",
                    TypeRef::Map(
                        Box::new(TypeRef::Named(NamedTypeId::parse("FlowNodeId").expect("valid named-type slug"))),
                        Box::new(TypeRef::Bool),
                    ),
                ),
                // Condition results
                field(
                    "node_condition_results",
                    TypeRef::Map(
                        Box::new(TypeRef::Named(NamedTypeId::parse("FlowNodeId").expect("valid named-type slug"))),
                        Box::new(TypeRef::Option(Box::new(TypeRef::Bool))),
                    ),
                ),
            ],
            init: InitSchema {
            phase: PhaseId::parse("Absent").expect("valid phase slug"),
                fields: vec![
                    init("frame_id", Expr::String(String::new())),
                    init(
                        "frame_scope",
                        Expr::NamedVariant {
                            enum_name: EnumTypeId::parse("FrameScope").expect("valid enum-type slug"),
                            variant: EnumVariantId::parse("Root").expect("valid enum-variant slug"),
                        },
                    ),
                    init("loop_instance_id", Expr::String(String::new())),
                    init("iteration", Expr::U64(0)),
                    init("last_admitted_node", Expr::String(String::new())),
                    init("tracked_nodes", Expr::EmptySet),
                    init("ordered_nodes", Expr::SeqLiteral(vec![])),
                    init("node_kind", Expr::EmptyMap),
                    init("node_dependencies", Expr::EmptyMap),
                    init("node_dependency_modes", Expr::EmptyMap),
                    init("node_branches", Expr::EmptyMap),
                    init("branch_winners", Expr::EmptySet),
                    init("node_status", Expr::EmptyMap),
                    init("ready_queue", Expr::SeqLiteral(vec![])),
                    init("output_recorded", Expr::EmptyMap),
                    init("node_condition_results", Expr::EmptyMap),
                ],
            },
            terminal_phases: vec![PhaseId::parse("Completed").expect("valid phase slug"), PhaseId::parse("Failed").expect("valid phase slug"), PhaseId::parse("Canceled").expect("valid phase slug")],
        },
        inputs: EnumSchema {
            name: "FlowFrameInput".into(),
            variants: vec![
                VariantSchema {
                name: EnumVariantId::parse("StartRootFrame").expect("valid variant slug"),
                    fields: vec![
                        field("frame_id", TypeRef::Named(NamedTypeId::parse("FrameId").expect("valid named-type slug"))),
                        field(
                            "tracked_nodes",
                            TypeRef::Set(Box::new(TypeRef::Named(NamedTypeId::parse("FlowNodeId").expect("valid named-type slug")))),
                        ),
                        field(
                            "ordered_nodes",
                            TypeRef::Seq(Box::new(TypeRef::Named(NamedTypeId::parse("FlowNodeId").expect("valid named-type slug")))),
                        ),
                        field(
                            "node_kind",
                            TypeRef::Map(
                                Box::new(TypeRef::Named(NamedTypeId::parse("FlowNodeId").expect("valid named-type slug"))),
                                Box::new(TypeRef::Enum(EnumTypeId::parse("FlowNodeKind").expect("valid enum-type slug"))),
                            ),
                        ),
                        field(
                            "node_dependencies",
                            TypeRef::Map(
                                Box::new(TypeRef::Named(NamedTypeId::parse("FlowNodeId").expect("valid named-type slug"))),
                                Box::new(TypeRef::Seq(Box::new(TypeRef::Named(NamedTypeId::parse("FlowNodeId").expect("valid named-type slug"))))),
                            ),
                        ),
                        field(
                            "node_dependency_modes",
                            TypeRef::Map(
                                Box::new(TypeRef::Named(NamedTypeId::parse("FlowNodeId").expect("valid named-type slug"))),
                                Box::new(TypeRef::Enum(EnumTypeId::parse("DependencyMode").expect("valid enum-type slug"))),
                            ),
                        ),
                        field(
                            "node_branches",
                            TypeRef::Map(
                                Box::new(TypeRef::Named(NamedTypeId::parse("FlowNodeId").expect("valid named-type slug"))),
                                Box::new(TypeRef::Option(Box::new(TypeRef::Named(NamedTypeId::parse("BranchId").expect("valid named-type slug"))))),
                            ),
                        ),
                    ],
                },
                VariantSchema {
                name: EnumVariantId::parse("StartBodyFrame").expect("valid variant slug"),
                    fields: vec![
                        field("frame_id", TypeRef::Named(NamedTypeId::parse("FrameId").expect("valid named-type slug"))),
                        field("loop_instance_id", TypeRef::Named(NamedTypeId::parse("LoopInstanceId").expect("valid named-type slug"))),
                        field("iteration", TypeRef::U32),
                        field(
                            "tracked_nodes",
                            TypeRef::Set(Box::new(TypeRef::Named(NamedTypeId::parse("FlowNodeId").expect("valid named-type slug")))),
                        ),
                        field(
                            "ordered_nodes",
                            TypeRef::Seq(Box::new(TypeRef::Named(NamedTypeId::parse("FlowNodeId").expect("valid named-type slug")))),
                        ),
                        field(
                            "node_kind",
                            TypeRef::Map(
                                Box::new(TypeRef::Named(NamedTypeId::parse("FlowNodeId").expect("valid named-type slug"))),
                                Box::new(TypeRef::Enum(EnumTypeId::parse("FlowNodeKind").expect("valid enum-type slug"))),
                            ),
                        ),
                        field(
                            "node_dependencies",
                            TypeRef::Map(
                                Box::new(TypeRef::Named(NamedTypeId::parse("FlowNodeId").expect("valid named-type slug"))),
                                Box::new(TypeRef::Seq(Box::new(TypeRef::Named(NamedTypeId::parse("FlowNodeId").expect("valid named-type slug"))))),
                            ),
                        ),
                        field(
                            "node_dependency_modes",
                            TypeRef::Map(
                                Box::new(TypeRef::Named(NamedTypeId::parse("FlowNodeId").expect("valid named-type slug"))),
                                Box::new(TypeRef::Enum(EnumTypeId::parse("DependencyMode").expect("valid enum-type slug"))),
                            ),
                        ),
                        field(
                            "node_branches",
                            TypeRef::Map(
                                Box::new(TypeRef::Named(NamedTypeId::parse("FlowNodeId").expect("valid named-type slug"))),
                                Box::new(TypeRef::Option(Box::new(TypeRef::Named(NamedTypeId::parse("BranchId").expect("valid named-type slug"))))),
                            ),
                        ),
                    ],
                },
                variant("AdmitNextReadyNode"),
                VariantSchema {
                name: EnumVariantId::parse("CompleteNode").expect("valid variant slug"),
                    fields: vec![field("node_id", TypeRef::Named(NamedTypeId::parse("FlowNodeId").expect("valid named-type slug")))],
                },
                VariantSchema {
                name: EnumVariantId::parse("RecordNodeOutput").expect("valid variant slug"),
                    fields: vec![field("node_id", TypeRef::Named(NamedTypeId::parse("FlowNodeId").expect("valid named-type slug")))],
                },
                VariantSchema {
                name: EnumVariantId::parse("FailNode").expect("valid variant slug"),
                    fields: vec![field("node_id", TypeRef::Named(NamedTypeId::parse("FlowNodeId").expect("valid named-type slug")))],
                },
                VariantSchema {
                name: EnumVariantId::parse("SkipNode").expect("valid variant slug"),
                    fields: vec![field("node_id", TypeRef::Named(NamedTypeId::parse("FlowNodeId").expect("valid named-type slug")))],
                },
                VariantSchema {
                name: EnumVariantId::parse("CancelNode").expect("valid variant slug"),
                    fields: vec![field("node_id", TypeRef::Named(NamedTypeId::parse("FlowNodeId").expect("valid named-type slug")))],
                },
                variant("SealFrame"),
            ],
        },
        surface_only_inputs: vec![],
        signals: EnumSchema {
            name: "FlowFrameSignal".into(),
            variants: vec![],
        },
        effects: EnumSchema {
            name: "FlowFrameEffect".into(),
            variants: vec![
                VariantSchema {
                name: EnumVariantId::parse("ReadyFrontierChanged").expect("valid variant slug"),
                    fields: vec![field("frame_id", TypeRef::Named(NamedTypeId::parse("FrameId").expect("valid named-type slug")))],
                },
                VariantSchema {
                name: EnumVariantId::parse("AdmitStepWork").expect("valid variant slug"),
                    fields: vec![
                        field("frame_id", TypeRef::Named(NamedTypeId::parse("FrameId").expect("valid named-type slug"))),
                        field("node_id", TypeRef::Named(NamedTypeId::parse("FlowNodeId").expect("valid named-type slug"))),
                    ],
                },
                VariantSchema {
                name: EnumVariantId::parse("StartLoopNode").expect("valid variant slug"),
                    fields: vec![
                        field("frame_id", TypeRef::Named(NamedTypeId::parse("FrameId").expect("valid named-type slug"))),
                        field("node_id", TypeRef::Named(NamedTypeId::parse("FlowNodeId").expect("valid named-type slug"))),
                    ],
                },
                VariantSchema {
                name: EnumVariantId::parse("PersistStepOutput").expect("valid variant slug"),
                    fields: vec![
                        field("frame_id", TypeRef::Named(NamedTypeId::parse("FrameId").expect("valid named-type slug"))),
                        field("node_id", TypeRef::Named(NamedTypeId::parse("FlowNodeId").expect("valid named-type slug"))),
                    ],
                },
                VariantSchema {
                name: EnumVariantId::parse("NodeExecutionReleased").expect("valid variant slug"),
                    fields: vec![
                        field("frame_id", TypeRef::Named(NamedTypeId::parse("FrameId").expect("valid named-type slug"))),
                        field("node_id", TypeRef::Named(NamedTypeId::parse("FlowNodeId").expect("valid named-type slug"))),
                    ],
                },
                VariantSchema {
                name: EnumVariantId::parse("RootFrameCompleted").expect("valid variant slug"),
                    fields: vec![field("frame_id", TypeRef::Named(NamedTypeId::parse("FrameId").expect("valid named-type slug")))],
                },
                VariantSchema {
                name: EnumVariantId::parse("RootFrameFailed").expect("valid variant slug"),
                    fields: vec![field("frame_id", TypeRef::Named(NamedTypeId::parse("FrameId").expect("valid named-type slug")))],
                },
                VariantSchema {
                name: EnumVariantId::parse("RootFrameCanceled").expect("valid variant slug"),
                    fields: vec![field("frame_id", TypeRef::Named(NamedTypeId::parse("FrameId").expect("valid named-type slug")))],
                },
                VariantSchema {
                name: EnumVariantId::parse("BodyFrameCompleted").expect("valid variant slug"),
                    fields: vec![
                        field("frame_id", TypeRef::Named(NamedTypeId::parse("FrameId").expect("valid named-type slug"))),
                        field("loop_instance_id", TypeRef::Named(NamedTypeId::parse("LoopInstanceId").expect("valid named-type slug"))),
                        field("iteration", TypeRef::U32),
                    ],
                },
                VariantSchema {
                name: EnumVariantId::parse("BodyFrameFailed").expect("valid variant slug"),
                    fields: vec![
                        field("frame_id", TypeRef::Named(NamedTypeId::parse("FrameId").expect("valid named-type slug"))),
                        field("loop_instance_id", TypeRef::Named(NamedTypeId::parse("LoopInstanceId").expect("valid named-type slug"))),
                        field("iteration", TypeRef::U32),
                    ],
                },
                VariantSchema {
                name: EnumVariantId::parse("BodyFrameCanceled").expect("valid variant slug"),
                    fields: vec![
                        field("frame_id", TypeRef::Named(NamedTypeId::parse("FrameId").expect("valid named-type slug"))),
                        field("loop_instance_id", TypeRef::Named(NamedTypeId::parse("LoopInstanceId").expect("valid named-type slug"))),
                        field("iteration", TypeRef::U32),
                    ],
                },
            ],
        },
        helpers: vec![
            // NodeAdmissionEligible(node_id): true if a Pending node should be promoted
            // to Ready (placed in the queue for eventual admission as run/skip/fail).
            // A dep is "terminal" if it is Completed, Failed, Skipped, or Canceled.
            // Critically, "Ready" and "Running" are NOT terminal — this prevents B from
            // being promoted while A is merely Ready (not yet started).
            HelperSchema {
                name: "NodeAdmissionEligible".into(),
                params: vec![field("node_id", TypeRef::Named(NamedTypeId::parse("FlowNodeId").expect("valid named-type slug")))],
                returns: TypeRef::Bool,
                body: Expr::IfElse {
                    // No deps → immediately eligible
                    condition: Box::new(Expr::Eq(
                        Box::new(Expr::Len(Box::new(Expr::MapGet {
                            map: Box::new(Expr::Field(FieldId::parse("node_dependencies").expect("valid field slug"))),
                            key: Box::new(Expr::Binding("node_id".into())),
                        }))),
                        Box::new(Expr::U64(0)),
                    )),
                    then_expr: Box::new(Expr::Bool(true)),
                    else_expr: Box::new(Expr::IfElse {
                        // All-mode: eligible when ALL deps are in a terminal state
                        // (Completed, Failed, Skipped, or Canceled — NOT Ready or Running)
                        condition: Box::new(Expr::Eq(
                            Box::new(Expr::MapGet {
                                map: Box::new(Expr::Field(FieldId::parse("node_dependency_modes").expect("valid field slug"))),
                                key: Box::new(Expr::Binding("node_id".into())),
                            }),
                            Box::new(Expr::NamedVariant {
                                enum_name: EnumTypeId::parse("DependencyMode").expect("valid enum-type slug"),
                                variant: EnumVariantId::parse("All").expect("valid enum-variant slug"),
                            }),
                        )),
                        then_expr: Box::new(Expr::Quantified {
                            quantifier: Quantifier::All,
                            binding: "dep_id".into(),
                            over: Box::new(dependency_ids_expr(Expr::Binding("node_id".into()))),
                            // dep is terminal iff its status is one of the 4 terminal states
                            body: Box::new(dep_is_terminal_expr("dep_id")),
                        }),
                        // Any-mode: eligible when any dep is Completed OR all deps are terminal
                        else_expr: Box::new(Expr::Or(vec![
                            Expr::Quantified {
                                quantifier: Quantifier::Any,
                                binding: "dep_id".into(),
                                over: Box::new(dependency_ids_expr(Expr::Binding("node_id".into()))),
                                body: Box::new(Expr::Eq(
                                    Box::new(Expr::MapGet {
                                        map: Box::new(Expr::Field(FieldId::parse("node_status").expect("valid field slug"))),
                                        key: Box::new(Expr::Binding("dep_id".into())),
                                    }),
                                    Box::new(Expr::NamedVariant {
                                        enum_name: EnumTypeId::parse("NodeRunStatus").expect("valid enum-type slug"),
                                        variant: EnumVariantId::parse("Completed").expect("valid enum-variant slug"),
                                    }),
                                )),
                            },
                            // All deps terminal (explicitly checked)
                            Expr::Quantified {
                                quantifier: Quantifier::All,
                                binding: "dep_id".into(),
                                over: Box::new(dependency_ids_expr(Expr::Binding("node_id".into()))),
                                body: Box::new(dep_is_terminal_expr("dep_id")),
                            },
                        ])),
                    }),
                },
            },
            // AllDepsCompleted(node_id): true if ALL deps are Completed
            HelperSchema {
                name: "AllDepsCompleted".into(),
                params: vec![field("node_id", TypeRef::Named(NamedTypeId::parse("FlowNodeId").expect("valid named-type slug")))],
                returns: TypeRef::Bool,
                body: Expr::Quantified {
                    quantifier: Quantifier::All,
                    binding: "dep_id".into(),
                    over: Box::new(dependency_ids_expr(Expr::Binding("node_id".into()))),
                    body: Box::new(Expr::Eq(
                        Box::new(Expr::MapGet {
                            map: Box::new(Expr::Field(FieldId::parse("node_status").expect("valid field slug"))),
                            key: Box::new(Expr::Binding("dep_id".into())),
                        }),
                        Box::new(Expr::NamedVariant {
                            enum_name: EnumTypeId::parse("NodeRunStatus").expect("valid enum-type slug"),
                            variant: EnumVariantId::parse("Completed").expect("valid enum-variant slug"),
                        }),
                    )),
                },
            },
            // AnyDepCompleted(node_id): true if ANY dep is Completed
            HelperSchema {
                name: "AnyDepCompleted".into(),
                params: vec![field("node_id", TypeRef::Named(NamedTypeId::parse("FlowNodeId").expect("valid named-type slug")))],
                returns: TypeRef::Bool,
                body: Expr::Quantified {
                    quantifier: Quantifier::Any,
                    binding: "dep_id".into(),
                    over: Box::new(dependency_ids_expr(Expr::Binding("node_id".into()))),
                    body: Box::new(Expr::Eq(
                        Box::new(Expr::MapGet {
                            map: Box::new(Expr::Field(FieldId::parse("node_status").expect("valid field slug"))),
                            key: Box::new(Expr::Binding("dep_id".into())),
                        }),
                        Box::new(Expr::NamedVariant {
                            enum_name: EnumTypeId::parse("NodeRunStatus").expect("valid enum-type slug"),
                            variant: EnumVariantId::parse("Completed").expect("valid enum-variant slug"),
                        }),
                    )),
                },
            },
            // AllNodesTerminal(): true if every tracked node has a terminal status
            HelperSchema {
                name: "AllNodesTerminal".into(),
                params: vec![],
                returns: TypeRef::Bool,
                body: Expr::Quantified {
                    quantifier: Quantifier::All,
                    binding: "t_node".into(),
                    over: Box::new(Expr::Field(FieldId::parse("tracked_nodes").expect("valid field slug"))),
                    body: Box::new(Expr::Or(vec![
                        Expr::Eq(
                            Box::new(Expr::MapGet {
                                map: Box::new(Expr::Field(FieldId::parse("node_status").expect("valid field slug"))),
                                key: Box::new(Expr::Binding("t_node".into())),
                            }),
                            Box::new(Expr::NamedVariant {
                                enum_name: EnumTypeId::parse("NodeRunStatus").expect("valid enum-type slug"),
                                variant: EnumVariantId::parse("Completed").expect("valid enum-variant slug"),
                            }),
                        ),
                        Expr::Eq(
                            Box::new(Expr::MapGet {
                                map: Box::new(Expr::Field(FieldId::parse("node_status").expect("valid field slug"))),
                                key: Box::new(Expr::Binding("t_node".into())),
                            }),
                            Box::new(Expr::NamedVariant {
                                enum_name: EnumTypeId::parse("NodeRunStatus").expect("valid enum-type slug"),
                                variant: EnumVariantId::parse("Failed").expect("valid enum-variant slug"),
                            }),
                        ),
                        Expr::Eq(
                            Box::new(Expr::MapGet {
                                map: Box::new(Expr::Field(FieldId::parse("node_status").expect("valid field slug"))),
                                key: Box::new(Expr::Binding("t_node".into())),
                            }),
                            Box::new(Expr::NamedVariant {
                                enum_name: EnumTypeId::parse("NodeRunStatus").expect("valid enum-type slug"),
                                variant: EnumVariantId::parse("Skipped").expect("valid enum-variant slug"),
                            }),
                        ),
                        Expr::Eq(
                            Box::new(Expr::MapGet {
                                map: Box::new(Expr::Field(FieldId::parse("node_status").expect("valid field slug"))),
                                key: Box::new(Expr::Binding("t_node".into())),
                            }),
                            Box::new(Expr::NamedVariant {
                                enum_name: EnumTypeId::parse("NodeRunStatus").expect("valid enum-type slug"),
                                variant: EnumVariantId::parse("Canceled").expect("valid enum-variant slug"),
                            }),
                        ),
                    ])),
                },
            },
        ],
        derived: vec![],
        invariants: vec![InvariantSchema {
            name: "ready_queue_membership_matches_ready_status".into(),
            expr: Expr::And(vec![
                // Every node in ready_queue has Ready status
                Expr::Quantified {
                    quantifier: Quantifier::All,
                    binding: "q_node".into(),
                    over: Box::new(Expr::SeqElements(Box::new(Expr::Field(FieldId::parse("ready_queue").expect("valid field slug"))))),
                    body: Box::new(Expr::Eq(
                        Box::new(Expr::MapGet {
                            map: Box::new(Expr::Field(FieldId::parse("node_status").expect("valid field slug"))),
                            key: Box::new(Expr::Binding("q_node".into())),
                        }),
                        Box::new(Expr::NamedVariant {
                            enum_name: EnumTypeId::parse("NodeRunStatus").expect("valid enum-type slug"),
                            variant: EnumVariantId::parse("Ready").expect("valid enum-variant slug"),
                        }),
                    )),
                },
                // Every node with Ready status is in ready_queue
                Expr::Quantified {
                    quantifier: Quantifier::All,
                    binding: "t_node".into(),
                    over: Box::new(Expr::Field(FieldId::parse("tracked_nodes").expect("valid field slug"))),
                    body: Box::new(Expr::Or(vec![
                        Expr::Neq(
                            Box::new(Expr::MapGet {
                                map: Box::new(Expr::Field(FieldId::parse("node_status").expect("valid field slug"))),
                                key: Box::new(Expr::Binding("t_node".into())),
                            }),
                            Box::new(Expr::NamedVariant {
                                enum_name: EnumTypeId::parse("NodeRunStatus").expect("valid enum-type slug"),
                                variant: EnumVariantId::parse("Ready").expect("valid enum-variant slug"),
                            }),
                        ),
                        Expr::Contains {
                            collection: Box::new(Expr::SeqElements(Box::new(Expr::Field(FieldId::parse("ready_queue").expect("valid field slug"))))),
                            value: Box::new(Expr::Binding("t_node".into())),
                        },
                    ])),
                },
            ]),
        }],
        transitions: vec![
            // ----------------------------------------------------------------
            // StartRootFrame: Absent -> Running
            // Stores frame topology, initializes all nodes as Pending,
            // then seeds the ready frontier.
            // ----------------------------------------------------------------
            TransitionSchema {
                name: TransitionId::parse("StartRootFrame").expect("valid transition slug"),
                from: vec![PhaseId::parse("Absent").expect("valid phase slug")],
                on: TriggerMatch::Input { variant: InputVariantId::parse("StartRootFrame").expect("valid input-variant slug"), bindings: vec![
                        FieldId::parse("frame_id").expect("valid field slug"),
                        FieldId::parse("tracked_nodes").expect("valid field slug"),
                        FieldId::parse("ordered_nodes").expect("valid field slug"),
                        FieldId::parse("node_kind").expect("valid field slug"),
                        FieldId::parse("node_dependencies").expect("valid field slug"),
                        FieldId::parse("node_dependency_modes").expect("valid field slug"),
                        FieldId::parse("node_branches").expect("valid field slug"),
                    ] },
                guards: vec![],
                updates: start_root_frame_updates(),
                to: PhaseId::parse("Running").expect("valid phase slug"),
                emit: vec![emit_ready_frontier_changed()],
            },
            TransitionSchema {
                name: TransitionId::parse("StartBodyFrame").expect("valid transition slug"),
                from: vec![PhaseId::parse("Absent").expect("valid phase slug")],
                on: TriggerMatch::Input { variant: InputVariantId::parse("StartBodyFrame").expect("valid input-variant slug"), bindings: vec![
                        FieldId::parse("frame_id").expect("valid field slug"),
                        FieldId::parse("loop_instance_id").expect("valid field slug"),
                        FieldId::parse("iteration").expect("valid field slug"),
                        FieldId::parse("tracked_nodes").expect("valid field slug"),
                        FieldId::parse("ordered_nodes").expect("valid field slug"),
                        FieldId::parse("node_kind").expect("valid field slug"),
                        FieldId::parse("node_dependencies").expect("valid field slug"),
                        FieldId::parse("node_dependency_modes").expect("valid field slug"),
                        FieldId::parse("node_branches").expect("valid field slug"),
                    ] },
                guards: vec![],
                updates: start_body_frame_updates(),
                to: PhaseId::parse("Running").expect("valid phase slug"),
                emit: vec![emit_ready_frontier_changed()],
            },
            // ----------------------------------------------------------------
            // AdmitNextReadyNode_StepRun
            // Head of ready_queue is a Step node whose deps allow running.
            // ----------------------------------------------------------------
            TransitionSchema {
                name: TransitionId::parse("AdmitNextReadyNode_StepRun").expect("valid transition slug"),
                from: vec![PhaseId::parse("Running").expect("valid phase slug")],
                on: TriggerMatch::Input { variant: InputVariantId::parse("AdmitNextReadyNode").expect("valid input-variant slug"), bindings: vec![] },
                guards: vec![
                    guard_queue_non_empty(),
                    guard_head_is_step(),
                    guard_head_can_run(),
                ],
                updates: admit_run_updates("Running"),
                to: PhaseId::parse("Running").expect("valid phase slug"),
                emit: vec![
                    EffectEmit { variant: EffectVariantId::parse("AdmitStepWork").expect("valid effect-variant slug"),
                        fields: IndexMap::from([
                            (FieldId::parse("frame_id").expect("valid field slug"), Expr::Field(FieldId::parse("frame_id").expect("valid field slug"))),
                            (FieldId::parse("node_id").expect("valid field slug"), Expr::Field(FieldId::parse("last_admitted_node").expect("valid field slug"))),
                        ]),
                    },
                    emit_ready_frontier_changed(),
                ],
            },
            // ----------------------------------------------------------------
            // AdmitNextReadyNode_LoopRun
            // Head of ready_queue is a Loop node whose deps allow running.
            // ----------------------------------------------------------------
            TransitionSchema {
                name: TransitionId::parse("AdmitNextReadyNode_LoopRun").expect("valid transition slug"),
                from: vec![PhaseId::parse("Running").expect("valid phase slug")],
                on: TriggerMatch::Input { variant: InputVariantId::parse("AdmitNextReadyNode").expect("valid input-variant slug"), bindings: vec![] },
                guards: vec![
                    guard_queue_non_empty(),
                    guard_head_is_loop(),
                    guard_head_can_run(),
                ],
                updates: admit_run_updates("Running"),
                to: PhaseId::parse("Running").expect("valid phase slug"),
                emit: vec![
                    EffectEmit { variant: EffectVariantId::parse("StartLoopNode").expect("valid effect-variant slug"),
                        fields: IndexMap::from([
                            (FieldId::parse("frame_id").expect("valid field slug"), Expr::Field(FieldId::parse("frame_id").expect("valid field slug"))),
                            (FieldId::parse("node_id").expect("valid field slug"), Expr::Field(FieldId::parse("last_admitted_node").expect("valid field slug"))),
                        ]),
                    },
                    emit_node_execution_released(Expr::Field(FieldId::parse("last_admitted_node").expect("valid field slug"))),
                    emit_ready_frontier_changed(),
                ],
            },
            // ----------------------------------------------------------------
            // AdmitNextReadyNode_Skip
            // Head of ready_queue should be skipped (All-mode with failed dep).
            // ----------------------------------------------------------------
            TransitionSchema {
                name: TransitionId::parse("AdmitNextReadyNode_Skip").expect("valid transition slug"),
                from: vec![PhaseId::parse("Running").expect("valid phase slug")],
                on: TriggerMatch::Input { variant: InputVariantId::parse("AdmitNextReadyNode").expect("valid input-variant slug"), bindings: vec![] },
                guards: vec![guard_queue_non_empty(), guard_head_should_skip()],
                updates: admit_terminal_updates("Skipped"),
                to: PhaseId::parse("Running").expect("valid phase slug"),
                emit: vec![
                    emit_node_execution_released(Expr::Field(FieldId::parse("last_admitted_node").expect("valid field slug"))),
                    emit_ready_frontier_changed(),
                ],
            },
            // ----------------------------------------------------------------
            // AdmitNextReadyNode_Fail
            // Head of ready_queue should fail (Any-mode, all deps terminal, none Completed).
            // ----------------------------------------------------------------
            TransitionSchema {
                name: TransitionId::parse("AdmitNextReadyNode_Fail").expect("valid transition slug"),
                from: vec![PhaseId::parse("Running").expect("valid phase slug")],
                on: TriggerMatch::Input { variant: InputVariantId::parse("AdmitNextReadyNode").expect("valid input-variant slug"), bindings: vec![] },
                guards: vec![guard_queue_non_empty(), guard_head_should_fail()],
                updates: admit_terminal_updates("Failed"),
                to: PhaseId::parse("Running").expect("valid phase slug"),
                emit: vec![
                    emit_node_execution_released(Expr::Field(FieldId::parse("last_admitted_node").expect("valid field slug"))),
                    emit_ready_frontier_changed(),
                ],
            },
            // ----------------------------------------------------------------
            // CompleteNode_Step: marks a Running Step node as Completed, refreshes frontier
            // ----------------------------------------------------------------
            TransitionSchema {
                name: TransitionId::parse("CompleteNode_Step").expect("valid transition slug"),
                from: vec![PhaseId::parse("Running").expect("valid phase slug")],
                on: TriggerMatch::Input { variant: InputVariantId::parse("CompleteNode").expect("valid input-variant slug"), bindings: vec![FieldId::parse("node_id").expect("valid field slug")] },
                guards: vec![
                    Guard {
                        name: "node_is_running".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::MapGet {
                                map: Box::new(Expr::Field(FieldId::parse("node_status").expect("valid field slug"))),
                                key: Box::new(Expr::Binding("node_id".into())),
                            }),
                            Box::new(Expr::NamedVariant {
                                enum_name: EnumTypeId::parse("NodeRunStatus").expect("valid enum-type slug"),
                                variant: EnumVariantId::parse("Running").expect("valid enum-variant slug"),
                            }),
                        ),
                    },
                    Guard {
                        name: "node_is_step".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::MapGet {
                                map: Box::new(Expr::Field(FieldId::parse("node_kind").expect("valid field slug"))),
                                key: Box::new(Expr::Binding("node_id".into())),
                            }),
                            Box::new(Expr::NamedVariant {
                                enum_name: EnumTypeId::parse("FlowNodeKind").expect("valid enum-type slug"),
                                variant: EnumVariantId::parse("Step").expect("valid enum-variant slug"),
                            }),
                        ),
                    },
                ],
                updates: {
                    let mut updates = vec![
                        Update::MapInsert {
                            field: FieldId::parse("node_status").expect("valid field slug"),
                            key: Expr::Binding("node_id".into()),
                            value: Expr::NamedVariant {
                                enum_name: EnumTypeId::parse("NodeRunStatus").expect("valid enum-type slug"),
                                variant: EnumVariantId::parse("Completed").expect("valid enum-variant slug"),
                            },
                        },
                        // If this node belongs to a branch group, record it as the branch winner
                        // so that sibling branch nodes are subsequently skipped.
                        Update::Conditional {
                            condition: Expr::Neq(
                                Box::new(Expr::MapGet {
                                    map: Box::new(Expr::Field(FieldId::parse("node_branches").expect("valid field slug"))),
                                    key: Box::new(Expr::Binding("node_id".into())),
                                }),
                                Box::new(Expr::None),
                            ),
                            then_updates: vec![Update::SetInsert {
                                field: FieldId::parse("branch_winners").expect("valid field slug"),
                                value: Expr::MapGet {
                                    map: Box::new(Expr::Field(FieldId::parse("node_branches").expect("valid field slug"))),
                                    key: Box::new(Expr::Binding("node_id".into())),
                                },
                            }],
                            else_updates: vec![],
                        },
                    ];
                    updates.extend(refresh_ready_frontier_updates());
                    updates
                },
                to: PhaseId::parse("Running").expect("valid phase slug"),
                emit: vec![
                    emit_node_execution_released(Expr::Binding("node_id".into())),
                    emit_ready_frontier_changed(),
                ],
            },
            // ----------------------------------------------------------------
            // CompleteNode_Loop: marks a Running Loop node as Completed, refreshes frontier
            // without releasing a node slot (the slot was released on loop handoff).
            // ----------------------------------------------------------------
            TransitionSchema {
                name: TransitionId::parse("CompleteNode_Loop").expect("valid transition slug"),
                from: vec![PhaseId::parse("Running").expect("valid phase slug")],
                on: TriggerMatch::Input { variant: InputVariantId::parse("CompleteNode").expect("valid input-variant slug"), bindings: vec![FieldId::parse("node_id").expect("valid field slug")] },
                guards: vec![
                    Guard {
                        name: "node_is_running".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::MapGet {
                                map: Box::new(Expr::Field(FieldId::parse("node_status").expect("valid field slug"))),
                                key: Box::new(Expr::Binding("node_id".into())),
                            }),
                            Box::new(Expr::NamedVariant {
                                enum_name: EnumTypeId::parse("NodeRunStatus").expect("valid enum-type slug"),
                                variant: EnumVariantId::parse("Running").expect("valid enum-variant slug"),
                            }),
                        ),
                    },
                    Guard {
                        name: "node_is_loop".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::MapGet {
                                map: Box::new(Expr::Field(FieldId::parse("node_kind").expect("valid field slug"))),
                                key: Box::new(Expr::Binding("node_id".into())),
                            }),
                            Box::new(Expr::NamedVariant {
                                enum_name: EnumTypeId::parse("FlowNodeKind").expect("valid enum-type slug"),
                                variant: EnumVariantId::parse("Loop").expect("valid enum-variant slug"),
                            }),
                        ),
                    },
                ],
                updates: {
                    let mut updates = vec![
                        Update::MapInsert {
                            field: FieldId::parse("node_status").expect("valid field slug"),
                            key: Expr::Binding("node_id".into()),
                            value: Expr::NamedVariant {
                                enum_name: EnumTypeId::parse("NodeRunStatus").expect("valid enum-type slug"),
                                variant: EnumVariantId::parse("Completed").expect("valid enum-variant slug"),
                            },
                        },
                        Update::Conditional {
                            condition: Expr::Neq(
                                Box::new(Expr::MapGet {
                                    map: Box::new(Expr::Field(FieldId::parse("node_branches").expect("valid field slug"))),
                                    key: Box::new(Expr::Binding("node_id".into())),
                                }),
                                Box::new(Expr::None),
                            ),
                            then_updates: vec![Update::SetInsert {
                                field: FieldId::parse("branch_winners").expect("valid field slug"),
                                value: Expr::MapGet {
                                    map: Box::new(Expr::Field(FieldId::parse("node_branches").expect("valid field slug"))),
                                    key: Box::new(Expr::Binding("node_id".into())),
                                },
                            }],
                            else_updates: vec![],
                        },
                    ];
                    updates.extend(refresh_ready_frontier_updates());
                    updates
                },
                to: PhaseId::parse("Running").expect("valid phase slug"),
                emit: vec![emit_ready_frontier_changed()],
            },
            // ----------------------------------------------------------------
            // RecordNodeOutput: marks output as recorded for a node
            // ----------------------------------------------------------------
            TransitionSchema {
                name: TransitionId::parse("RecordNodeOutput").expect("valid transition slug"),
                from: vec![PhaseId::parse("Running").expect("valid phase slug")],
                on: TriggerMatch::Input { variant: InputVariantId::parse("RecordNodeOutput").expect("valid input-variant slug"), bindings: vec![FieldId::parse("node_id").expect("valid field slug")] },
                guards: vec![],
                updates: vec![Update::MapInsert {
                    field: FieldId::parse("output_recorded").expect("valid field slug"),
                    key: Expr::Binding("node_id".into()),
                    value: Expr::Bool(true),
                }],
                to: PhaseId::parse("Running").expect("valid phase slug"),
                emit: vec![EffectEmit { variant: EffectVariantId::parse("PersistStepOutput").expect("valid effect-variant slug"),
                    fields: IndexMap::from([
                        (FieldId::parse("frame_id").expect("valid field slug"), Expr::Field(FieldId::parse("frame_id").expect("valid field slug"))),
                        (FieldId::parse("node_id").expect("valid field slug"), Expr::Binding("node_id".into())),
                    ]),
                }],
            },
            // ----------------------------------------------------------------
            // FailNode_Step: marks a Running Step node as Failed, refreshes frontier
            // ----------------------------------------------------------------
            TransitionSchema {
                name: TransitionId::parse("FailNode_Step").expect("valid transition slug"),
                from: vec![PhaseId::parse("Running").expect("valid phase slug")],
                on: TriggerMatch::Input { variant: InputVariantId::parse("FailNode").expect("valid input-variant slug"), bindings: vec![FieldId::parse("node_id").expect("valid field slug")] },
                guards: vec![
                    Guard {
                        name: "node_is_running".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::MapGet {
                                map: Box::new(Expr::Field(FieldId::parse("node_status").expect("valid field slug"))),
                                key: Box::new(Expr::Binding("node_id".into())),
                            }),
                            Box::new(Expr::NamedVariant {
                                enum_name: EnumTypeId::parse("NodeRunStatus").expect("valid enum-type slug"),
                                variant: EnumVariantId::parse("Running").expect("valid enum-variant slug"),
                            }),
                        ),
                    },
                    Guard {
                        name: "node_is_step".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::MapGet {
                                map: Box::new(Expr::Field(FieldId::parse("node_kind").expect("valid field slug"))),
                                key: Box::new(Expr::Binding("node_id".into())),
                            }),
                            Box::new(Expr::NamedVariant {
                                enum_name: EnumTypeId::parse("FlowNodeKind").expect("valid enum-type slug"),
                                variant: EnumVariantId::parse("Step").expect("valid enum-variant slug"),
                            }),
                        ),
                    },
                ],
                updates: {
                    let mut updates = vec![Update::MapInsert {
                        field: FieldId::parse("node_status").expect("valid field slug"),
                        key: Expr::Binding("node_id".into()),
                        value: Expr::NamedVariant {
                            enum_name: EnumTypeId::parse("NodeRunStatus").expect("valid enum-type slug"),
                            variant: EnumVariantId::parse("Failed").expect("valid enum-variant slug"),
                        },
                    }];
                    updates.extend(refresh_ready_frontier_updates());
                    updates
                },
                to: PhaseId::parse("Running").expect("valid phase slug"),
                emit: vec![
                    emit_node_execution_released(Expr::Binding("node_id".into())),
                    emit_ready_frontier_changed(),
                ],
            },
            // ----------------------------------------------------------------
            // FailNode_Loop: marks a Running Loop node as Failed, refreshes frontier
            // without releasing a node slot (the slot was released on loop handoff).
            // ----------------------------------------------------------------
            TransitionSchema {
                name: TransitionId::parse("FailNode_Loop").expect("valid transition slug"),
                from: vec![PhaseId::parse("Running").expect("valid phase slug")],
                on: TriggerMatch::Input { variant: InputVariantId::parse("FailNode").expect("valid input-variant slug"), bindings: vec![FieldId::parse("node_id").expect("valid field slug")] },
                guards: vec![
                    Guard {
                        name: "node_is_running".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::MapGet {
                                map: Box::new(Expr::Field(FieldId::parse("node_status").expect("valid field slug"))),
                                key: Box::new(Expr::Binding("node_id".into())),
                            }),
                            Box::new(Expr::NamedVariant {
                                enum_name: EnumTypeId::parse("NodeRunStatus").expect("valid enum-type slug"),
                                variant: EnumVariantId::parse("Running").expect("valid enum-variant slug"),
                            }),
                        ),
                    },
                    Guard {
                        name: "node_is_loop".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::MapGet {
                                map: Box::new(Expr::Field(FieldId::parse("node_kind").expect("valid field slug"))),
                                key: Box::new(Expr::Binding("node_id".into())),
                            }),
                            Box::new(Expr::NamedVariant {
                                enum_name: EnumTypeId::parse("FlowNodeKind").expect("valid enum-type slug"),
                                variant: EnumVariantId::parse("Loop").expect("valid enum-variant slug"),
                            }),
                        ),
                    },
                ],
                updates: {
                    let mut updates = vec![Update::MapInsert {
                        field: FieldId::parse("node_status").expect("valid field slug"),
                        key: Expr::Binding("node_id".into()),
                        value: Expr::NamedVariant {
                            enum_name: EnumTypeId::parse("NodeRunStatus").expect("valid enum-type slug"),
                            variant: EnumVariantId::parse("Failed").expect("valid enum-variant slug"),
                        },
                    }];
                    updates.extend(refresh_ready_frontier_updates());
                    updates
                },
                to: PhaseId::parse("Running").expect("valid phase slug"),
                emit: vec![emit_ready_frontier_changed()],
            },
            // ----------------------------------------------------------------
            // SkipNode_Step: marks a Running Step node as Skipped, refreshes frontier
            // ----------------------------------------------------------------
            TransitionSchema {
                name: TransitionId::parse("SkipNode_Step").expect("valid transition slug"),
                from: vec![PhaseId::parse("Running").expect("valid phase slug")],
                on: TriggerMatch::Input { variant: InputVariantId::parse("SkipNode").expect("valid input-variant slug"), bindings: vec![FieldId::parse("node_id").expect("valid field slug")] },
                guards: vec![
                    Guard {
                        name: "node_is_running".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::MapGet {
                                map: Box::new(Expr::Field(FieldId::parse("node_status").expect("valid field slug"))),
                                key: Box::new(Expr::Binding("node_id".into())),
                            }),
                            Box::new(Expr::NamedVariant {
                                enum_name: EnumTypeId::parse("NodeRunStatus").expect("valid enum-type slug"),
                                variant: EnumVariantId::parse("Running").expect("valid enum-variant slug"),
                            }),
                        ),
                    },
                    Guard {
                        name: "node_is_step".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::MapGet {
                                map: Box::new(Expr::Field(FieldId::parse("node_kind").expect("valid field slug"))),
                                key: Box::new(Expr::Binding("node_id".into())),
                            }),
                            Box::new(Expr::NamedVariant {
                                enum_name: EnumTypeId::parse("FlowNodeKind").expect("valid enum-type slug"),
                                variant: EnumVariantId::parse("Step").expect("valid enum-variant slug"),
                            }),
                        ),
                    },
                ],
                updates: {
                    let mut updates = vec![Update::MapInsert {
                        field: FieldId::parse("node_status").expect("valid field slug"),
                        key: Expr::Binding("node_id".into()),
                        value: Expr::NamedVariant {
                            enum_name: EnumTypeId::parse("NodeRunStatus").expect("valid enum-type slug"),
                            variant: EnumVariantId::parse("Skipped").expect("valid enum-variant slug"),
                        },
                    }];
                    updates.extend(refresh_ready_frontier_updates());
                    updates
                },
                to: PhaseId::parse("Running").expect("valid phase slug"),
                emit: vec![
                    emit_node_execution_released(Expr::Binding("node_id".into())),
                    emit_ready_frontier_changed(),
                ],
            },
            // ----------------------------------------------------------------
            // SkipNode_Loop: marks a Running Loop node as Skipped, refreshes frontier
            // without releasing a node slot (the slot was released on loop handoff).
            // ----------------------------------------------------------------
            TransitionSchema {
                name: TransitionId::parse("SkipNode_Loop").expect("valid transition slug"),
                from: vec![PhaseId::parse("Running").expect("valid phase slug")],
                on: TriggerMatch::Input { variant: InputVariantId::parse("SkipNode").expect("valid input-variant slug"), bindings: vec![FieldId::parse("node_id").expect("valid field slug")] },
                guards: vec![
                    Guard {
                        name: "node_is_running".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::MapGet {
                                map: Box::new(Expr::Field(FieldId::parse("node_status").expect("valid field slug"))),
                                key: Box::new(Expr::Binding("node_id".into())),
                            }),
                            Box::new(Expr::NamedVariant {
                                enum_name: EnumTypeId::parse("NodeRunStatus").expect("valid enum-type slug"),
                                variant: EnumVariantId::parse("Running").expect("valid enum-variant slug"),
                            }),
                        ),
                    },
                    Guard {
                        name: "node_is_loop".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::MapGet {
                                map: Box::new(Expr::Field(FieldId::parse("node_kind").expect("valid field slug"))),
                                key: Box::new(Expr::Binding("node_id".into())),
                            }),
                            Box::new(Expr::NamedVariant {
                                enum_name: EnumTypeId::parse("FlowNodeKind").expect("valid enum-type slug"),
                                variant: EnumVariantId::parse("Loop").expect("valid enum-variant slug"),
                            }),
                        ),
                    },
                ],
                updates: {
                    let mut updates = vec![Update::MapInsert {
                        field: FieldId::parse("node_status").expect("valid field slug"),
                        key: Expr::Binding("node_id".into()),
                        value: Expr::NamedVariant {
                            enum_name: EnumTypeId::parse("NodeRunStatus").expect("valid enum-type slug"),
                            variant: EnumVariantId::parse("Skipped").expect("valid enum-variant slug"),
                        },
                    }];
                    updates.extend(refresh_ready_frontier_updates());
                    updates
                },
                to: PhaseId::parse("Running").expect("valid phase slug"),
                emit: vec![emit_ready_frontier_changed()],
            },
            // ----------------------------------------------------------------
            // CancelNode_Step: marks a Running Step node as Canceled, refreshes frontier
            // ----------------------------------------------------------------
            TransitionSchema {
                name: TransitionId::parse("CancelNode_Step").expect("valid transition slug"),
                from: vec![PhaseId::parse("Running").expect("valid phase slug")],
                on: TriggerMatch::Input { variant: InputVariantId::parse("CancelNode").expect("valid input-variant slug"), bindings: vec![FieldId::parse("node_id").expect("valid field slug")] },
                guards: vec![
                    Guard {
                        name: "node_is_running".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::MapGet {
                                map: Box::new(Expr::Field(FieldId::parse("node_status").expect("valid field slug"))),
                                key: Box::new(Expr::Binding("node_id".into())),
                            }),
                            Box::new(Expr::NamedVariant {
                                enum_name: EnumTypeId::parse("NodeRunStatus").expect("valid enum-type slug"),
                                variant: EnumVariantId::parse("Running").expect("valid enum-variant slug"),
                            }),
                        ),
                    },
                    Guard {
                        name: "node_is_step".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::MapGet {
                                map: Box::new(Expr::Field(FieldId::parse("node_kind").expect("valid field slug"))),
                                key: Box::new(Expr::Binding("node_id".into())),
                            }),
                            Box::new(Expr::NamedVariant {
                                enum_name: EnumTypeId::parse("FlowNodeKind").expect("valid enum-type slug"),
                                variant: EnumVariantId::parse("Step").expect("valid enum-variant slug"),
                            }),
                        ),
                    },
                ],
                updates: {
                    let mut updates = vec![Update::MapInsert {
                        field: FieldId::parse("node_status").expect("valid field slug"),
                        key: Expr::Binding("node_id".into()),
                        value: Expr::NamedVariant {
                            enum_name: EnumTypeId::parse("NodeRunStatus").expect("valid enum-type slug"),
                            variant: EnumVariantId::parse("Canceled").expect("valid enum-variant slug"),
                        },
                    }];
                    updates.extend(refresh_ready_frontier_updates());
                    updates
                },
                to: PhaseId::parse("Running").expect("valid phase slug"),
                emit: vec![
                    emit_node_execution_released(Expr::Binding("node_id".into())),
                    emit_ready_frontier_changed(),
                ],
            },
            // ----------------------------------------------------------------
            // CancelNode_Loop: marks a Running Loop node as Canceled, refreshes frontier
            // without releasing a node slot (the slot was released on loop handoff).
            // ----------------------------------------------------------------
            TransitionSchema {
                name: TransitionId::parse("CancelNode_Loop").expect("valid transition slug"),
                from: vec![PhaseId::parse("Running").expect("valid phase slug")],
                on: TriggerMatch::Input { variant: InputVariantId::parse("CancelNode").expect("valid input-variant slug"), bindings: vec![FieldId::parse("node_id").expect("valid field slug")] },
                guards: vec![
                    Guard {
                        name: "node_is_running".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::MapGet {
                                map: Box::new(Expr::Field(FieldId::parse("node_status").expect("valid field slug"))),
                                key: Box::new(Expr::Binding("node_id".into())),
                            }),
                            Box::new(Expr::NamedVariant {
                                enum_name: EnumTypeId::parse("NodeRunStatus").expect("valid enum-type slug"),
                                variant: EnumVariantId::parse("Running").expect("valid enum-variant slug"),
                            }),
                        ),
                    },
                    Guard {
                        name: "node_is_loop".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::MapGet {
                                map: Box::new(Expr::Field(FieldId::parse("node_kind").expect("valid field slug"))),
                                key: Box::new(Expr::Binding("node_id".into())),
                            }),
                            Box::new(Expr::NamedVariant {
                                enum_name: EnumTypeId::parse("FlowNodeKind").expect("valid enum-type slug"),
                                variant: EnumVariantId::parse("Loop").expect("valid enum-variant slug"),
                            }),
                        ),
                    },
                ],
                updates: {
                    let mut updates = vec![Update::MapInsert {
                        field: FieldId::parse("node_status").expect("valid field slug"),
                        key: Expr::Binding("node_id".into()),
                        value: Expr::NamedVariant {
                            enum_name: EnumTypeId::parse("NodeRunStatus").expect("valid enum-type slug"),
                            variant: EnumVariantId::parse("Canceled").expect("valid enum-variant slug"),
                        },
                    }];
                    updates.extend(refresh_ready_frontier_updates());
                    updates
                },
                to: PhaseId::parse("Running").expect("valid phase slug"),
                emit: vec![emit_ready_frontier_changed()],
            },
            // ----------------------------------------------------------------
            // SealFrame_*: Running -> Completed|Failed|Canceled.
            // Ordinary frame closeout is machine-owned and derived from node truth:
            // canceled outranks failed; failed outranks completed.
            TransitionSchema {
                name: TransitionId::parse("SealRootFrameCanceled").expect("valid transition slug"),
                from: vec![PhaseId::parse("Running").expect("valid phase slug")],
                on: TriggerMatch::Input { variant: InputVariantId::parse("SealFrame").expect("valid input-variant slug"), bindings: vec![] },
                guards: vec![
                    Guard {
                        name: "all_nodes_terminal".into(),
                        expr: Expr::Call {
                            helper: "AllNodesTerminal".into(),
                            args: vec![],
                        },
                    },
                    Guard {
                        name: "root_frame".into(),
                        expr: frame_scope_expr("Root"),
                    },
                    Guard {
                        name: "has_canceled_nodes".into(),
                        expr: any_node_with_status_expr("Canceled"),
                    },
                ],
                updates: vec![],
                to: PhaseId::parse("Canceled").expect("valid phase slug"),
                emit: vec![emit_root_frame_terminal("RootFrameCanceled")],
            },
            TransitionSchema {
                name: TransitionId::parse("SealRootFrameFailed").expect("valid transition slug"),
                from: vec![PhaseId::parse("Running").expect("valid phase slug")],
                on: TriggerMatch::Input { variant: InputVariantId::parse("SealFrame").expect("valid input-variant slug"), bindings: vec![] },
                guards: vec![
                    Guard {
                        name: "all_nodes_terminal".into(),
                        expr: Expr::Call {
                            helper: "AllNodesTerminal".into(),
                            args: vec![],
                        },
                    },
                    Guard {
                        name: "root_frame".into(),
                        expr: frame_scope_expr("Root"),
                    },
                    Guard {
                        name: "no_canceled_nodes".into(),
                        expr: Expr::Not(Box::new(any_node_with_status_expr("Canceled"))),
                    },
                    Guard {
                        name: "has_failed_nodes".into(),
                        expr: any_node_with_status_expr("Failed"),
                    },
                ],
                updates: vec![],
                to: PhaseId::parse("Failed").expect("valid phase slug"),
                emit: vec![emit_root_frame_terminal("RootFrameFailed")],
            },
            TransitionSchema {
                name: TransitionId::parse("SealRootFrameCompleted").expect("valid transition slug"),
                from: vec![PhaseId::parse("Running").expect("valid phase slug")],
                on: TriggerMatch::Input { variant: InputVariantId::parse("SealFrame").expect("valid input-variant slug"), bindings: vec![] },
                guards: vec![
                    Guard {
                        name: "all_nodes_terminal".into(),
                        expr: Expr::Call {
                            helper: "AllNodesTerminal".into(),
                            args: vec![],
                        },
                    },
                    Guard {
                        name: "root_frame".into(),
                        expr: frame_scope_expr("Root"),
                    },
                    Guard {
                        name: "no_canceled_nodes".into(),
                        expr: Expr::Not(Box::new(any_node_with_status_expr("Canceled"))),
                    },
                    Guard {
                        name: "no_failed_nodes".into(),
                        expr: Expr::Not(Box::new(any_node_with_status_expr("Failed"))),
                    },
                ],
                updates: vec![],
                to: PhaseId::parse("Completed").expect("valid phase slug"),
                emit: vec![emit_root_frame_terminal("RootFrameCompleted")],
            },
            TransitionSchema {
                name: TransitionId::parse("SealBodyFrameCanceled").expect("valid transition slug"),
                from: vec![PhaseId::parse("Running").expect("valid phase slug")],
                on: TriggerMatch::Input { variant: InputVariantId::parse("SealFrame").expect("valid input-variant slug"), bindings: vec![] },
                guards: vec![
                    Guard {
                        name: "all_nodes_terminal".into(),
                        expr: Expr::Call {
                            helper: "AllNodesTerminal".into(),
                            args: vec![],
                        },
                    },
                    Guard {
                        name: "body_frame".into(),
                        expr: frame_scope_expr("Body"),
                    },
                    Guard {
                        name: "has_canceled_nodes".into(),
                        expr: any_node_with_status_expr("Canceled"),
                    },
                ],
                updates: vec![],
                to: PhaseId::parse("Canceled").expect("valid phase slug"),
                emit: vec![emit_body_frame_terminal("BodyFrameCanceled")],
            },
            TransitionSchema {
                name: TransitionId::parse("SealBodyFrameFailed").expect("valid transition slug"),
                from: vec![PhaseId::parse("Running").expect("valid phase slug")],
                on: TriggerMatch::Input { variant: InputVariantId::parse("SealFrame").expect("valid input-variant slug"), bindings: vec![] },
                guards: vec![
                    Guard {
                        name: "all_nodes_terminal".into(),
                        expr: Expr::Call {
                            helper: "AllNodesTerminal".into(),
                            args: vec![],
                        },
                    },
                    Guard {
                        name: "body_frame".into(),
                        expr: frame_scope_expr("Body"),
                    },
                    Guard {
                        name: "no_canceled_nodes".into(),
                        expr: Expr::Not(Box::new(any_node_with_status_expr("Canceled"))),
                    },
                    Guard {
                        name: "has_failed_nodes".into(),
                        expr: any_node_with_status_expr("Failed"),
                    },
                ],
                updates: vec![],
                to: PhaseId::parse("Failed").expect("valid phase slug"),
                emit: vec![emit_body_frame_terminal("BodyFrameFailed")],
            },
            TransitionSchema {
                name: TransitionId::parse("SealBodyFrameCompleted").expect("valid transition slug"),
                from: vec![PhaseId::parse("Running").expect("valid phase slug")],
                on: TriggerMatch::Input { variant: InputVariantId::parse("SealFrame").expect("valid input-variant slug"), bindings: vec![] },
                guards: vec![
                    Guard {
                        name: "all_nodes_terminal".into(),
                        expr: Expr::Call {
                            helper: "AllNodesTerminal".into(),
                            args: vec![],
                        },
                    },
                    Guard {
                        name: "body_frame".into(),
                        expr: frame_scope_expr("Body"),
                    },
                    Guard {
                        name: "no_canceled_nodes".into(),
                        expr: Expr::Not(Box::new(any_node_with_status_expr("Canceled"))),
                    },
                    Guard {
                        name: "no_failed_nodes".into(),
                        expr: Expr::Not(Box::new(any_node_with_status_expr("Failed"))),
                    },
                ],
                updates: vec![],
                to: PhaseId::parse("Completed").expect("valid phase slug"),
                emit: vec![emit_body_frame_terminal("BodyFrameCompleted")],
            },
        ],
        ci_step_limit: None,
        effect_dispositions: vec![
            routed_disposition("ReadyFrontierChanged", &["FlowRunMachine"]),
            disposition("AdmitStepWork", EffectDisposition::External),
            disposition("StartLoopNode", EffectDisposition::External),
            disposition("PersistStepOutput", EffectDisposition::Local),
            routed_disposition("NodeExecutionReleased", &["FlowRunMachine"]),
            disposition("RootFrameCompleted", EffectDisposition::External),
            disposition("RootFrameFailed", EffectDisposition::External),
            disposition("RootFrameCanceled", EffectDisposition::External),
            routed_disposition("BodyFrameCompleted", &["LoopIterationMachine"]),
            routed_disposition("BodyFrameFailed", &["LoopIterationMachine"]),
            routed_disposition("BodyFrameCanceled", &["LoopIterationMachine"]),
        ],
        named_types: vec![
            NamedTypeBinding::string("FrameId"),
            NamedTypeBinding::string("LoopInstanceId"),
            NamedTypeBinding::string("BranchId"),
            NamedTypeBinding::string("FlowNodeId"),
        ],
    }
}

// ---------------------------------------------------------------------------
// DSL builder helpers
// ---------------------------------------------------------------------------

fn disposition(name: &str, d: EffectDisposition) -> EffectDispositionRule {
    EffectDispositionRule {
        effect_variant: EffectVariantId::parse(name).expect("valid effect-variant slug"),
        disposition: d,
        handoff_protocol: None,
    }
}

fn routed_disposition(name: &str, consumer_machines: &[&str]) -> EffectDispositionRule {
    EffectDispositionRule {
        effect_variant: EffectVariantId::parse(name).expect("valid effect-variant slug"),
        disposition: EffectDisposition::Routed {
            consumer_machines: consumer_machines
                .iter().map(|item| MachineId::parse(*item).expect("valid machine slug"))
                .collect(),
        },
        handoff_protocol: None,
    }
}

fn variant(name: &str) -> VariantSchema {
    VariantSchema {
        name: EnumVariantId::parse(name).expect("valid variant slug"),
        fields: vec![],
    }
}

fn field(name: &str, ty: TypeRef) -> FieldSchema {
    FieldSchema {
        name: FieldId::parse(name).expect("valid field slug"),
        ty,
    }
}

fn init(field: &str, expr: Expr) -> FieldInit {
    FieldInit {
        field: FieldId::parse(field).expect("valid field slug"),
        expr,
    }
}

// Suppress unused import warning - InitSchema is used via the struct literal in MachineSchema
use crate::InitSchema;
