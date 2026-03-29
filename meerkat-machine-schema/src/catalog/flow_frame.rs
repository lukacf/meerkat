use indexmap::IndexMap;

use crate::{
    EffectDisposition, EffectDispositionRule, EffectEmit, EnumSchema, Expr, FieldInit, FieldSchema,
    Guard, HelperSchema, InputMatch, InvariantSchema, MachineSchema, Quantifier, RustBinding,
    StateSchema, TransitionSchema, TypeRef, Update, VariantSchema,
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
                map: Box::new(Expr::Field("node_status".into())),
                key: Box::new(Expr::Binding(dep_binding.into())),
            }),
            Box::new(Expr::NamedVariant {
                enum_name: "NodeRunStatus".into(),
                variant: "Completed".into(),
            }),
        ),
        Expr::Eq(
            Box::new(Expr::MapGet {
                map: Box::new(Expr::Field("node_status".into())),
                key: Box::new(Expr::Binding(dep_binding.into())),
            }),
            Box::new(Expr::NamedVariant {
                enum_name: "NodeRunStatus".into(),
                variant: "Failed".into(),
            }),
        ),
        Expr::Eq(
            Box::new(Expr::MapGet {
                map: Box::new(Expr::Field("node_status".into())),
                key: Box::new(Expr::Binding(dep_binding.into())),
            }),
            Box::new(Expr::NamedVariant {
                enum_name: "NodeRunStatus".into(),
                variant: "Skipped".into(),
            }),
        ),
        Expr::Eq(
            Box::new(Expr::MapGet {
                map: Box::new(Expr::Field("node_status".into())),
                key: Box::new(Expr::Binding(dep_binding.into())),
            }),
            Box::new(Expr::NamedVariant {
                enum_name: "NodeRunStatus".into(),
                variant: "Canceled".into(),
            }),
        ),
    ])
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
            over: Expr::Field("ordered_nodes".into()),
            updates: vec![Update::Conditional {
                condition: Expr::And(vec![
                    // Node must be Pending
                    Expr::Eq(
                        Box::new(Expr::MapGet {
                            map: Box::new(Expr::Field("node_status".into())),
                            key: Box::new(Expr::Binding("rf_node".into())),
                        }),
                        Box::new(Expr::NamedVariant {
                            enum_name: "NodeRunStatus".into(),
                            variant: "Pending".into(),
                        }),
                    ),
                    // Dep conditions are satisfied for admission eligibility
                    Expr::Call {
                        helper: "NodeAdmissionEligible".into(),
                        args: vec![Expr::Binding("rf_node".into())],
                    },
                ]),
                then_updates: vec![Update::MapInsert {
                    field: "node_status".into(),
                    key: Expr::Binding("rf_node".into()),
                    value: Expr::NamedVariant {
                        enum_name: "NodeRunStatus".into(),
                        variant: "Ready".into(),
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
            over: Expr::Field("ordered_nodes".into()),
            updates: vec![Update::Conditional {
                condition: Expr::And(vec![
                    // Node must be Ready (was just promoted in Step 1, or was already Ready)
                    Expr::Eq(
                        Box::new(Expr::MapGet {
                            map: Box::new(Expr::Field("node_status".into())),
                            key: Box::new(Expr::Binding("rf_node".into())),
                        }),
                        Box::new(Expr::NamedVariant {
                            enum_name: "NodeRunStatus".into(),
                            variant: "Ready".into(),
                        }),
                    ),
                    // Node must NOT already be in ready_queue
                    Expr::Not(Box::new(Expr::Contains {
                        collection: Box::new(Expr::SeqElements(Box::new(Expr::Field(
                            "ready_queue".into(),
                        )))),
                        value: Box::new(Expr::Binding("rf_node".into())),
                    })),
                ]),
                then_updates: vec![Update::SeqAppend {
                    field: "ready_queue".into(),
                    value: Expr::Binding("rf_node".into()),
                }],
                else_updates: vec![],
            }],
        },
    ]
}

/// Emit a ReadyFrontierChanged effect referencing the frame_id in state.
fn emit_ready_frontier_changed() -> EffectEmit {
    EffectEmit {
        variant: "ReadyFrontierChanged".into(),
        fields: IndexMap::from([("frame_id".into(), Expr::Field("frame_id".into()))]),
    }
}

/// Emit a NodeExecutionReleased effect for a given node_id binding.
fn emit_node_execution_released(node_id_expr: Expr) -> EffectEmit {
    EffectEmit {
        variant: "NodeExecutionReleased".into(),
        fields: IndexMap::from([
            ("frame_id".into(), Expr::Field("frame_id".into())),
            ("node_id".into(), node_id_expr),
        ]),
    }
}

/// Build updates for StartFrame: assign topology fields, init all nodes Pending,
/// then seed the ready frontier.
fn start_frame_updates() -> Vec<Update> {
    let mut updates = vec![
        Update::Assign {
            field: "frame_id".into(),
            expr: Expr::Binding("frame_id".into()),
        },
        Update::Assign {
            field: "tracked_nodes".into(),
            expr: Expr::Binding("tracked_nodes".into()),
        },
        Update::Assign {
            field: "ordered_nodes".into(),
            expr: Expr::Binding("ordered_nodes".into()),
        },
        Update::Assign {
            field: "node_kind".into(),
            expr: Expr::Binding("node_kind".into()),
        },
        Update::Assign {
            field: "node_dependencies".into(),
            expr: Expr::Binding("node_dependencies".into()),
        },
        Update::Assign {
            field: "node_dependency_modes".into(),
            expr: Expr::Binding("node_dependency_modes".into()),
        },
        Update::Assign {
            field: "node_branches".into(),
            expr: Expr::Binding("node_branches".into()),
        },
        // Initialize all nodes as Pending
        Update::ForEach {
            binding: "init_node".into(),
            over: Expr::Binding("tracked_nodes".into()),
            updates: vec![Update::MapInsert {
                field: "node_status".into(),
                key: Expr::Binding("init_node".into()),
                value: Expr::NamedVariant {
                    enum_name: "NodeRunStatus".into(),
                    variant: "Pending".into(),
                },
            }],
        },
    ];
    // Seed the ready frontier (roots → Ready, appended to ready_queue)
    updates.extend(refresh_ready_frontier_updates());
    updates
}

/// Guard: ready_queue is non-empty
fn guard_queue_non_empty() -> Guard {
    Guard {
        name: "ready_queue_non_empty".into(),
        expr: Expr::Gt(
            Box::new(Expr::Len(Box::new(Expr::Field("ready_queue".into())))),
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
                map: Box::new(Expr::Field("node_kind".into())),
                key: Box::new(Expr::Head(Box::new(Expr::Field("ready_queue".into())))),
            }),
            Box::new(Expr::NamedVariant {
                enum_name: "FlowNodeKind".into(),
                variant: "Step".into(),
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
                map: Box::new(Expr::Field("node_kind".into())),
                key: Box::new(Expr::Head(Box::new(Expr::Field("ready_queue".into())))),
            }),
            Box::new(Expr::NamedVariant {
                enum_name: "FlowNodeKind".into(),
                variant: "Loop".into(),
            }),
        ),
    }
}

/// Guard: head node's deps are satisfied for a run (all Completed or no deps for All-mode;
/// any Completed for Any-mode).
fn guard_head_can_run() -> Guard {
    Guard {
        name: "head_deps_eligible_for_run".into(),
        expr: Expr::Or(vec![
            // No deps at all
            Expr::Eq(
                Box::new(Expr::Len(Box::new(Expr::MapGet {
                    map: Box::new(Expr::Field("node_dependencies".into())),
                    key: Box::new(Expr::Head(Box::new(Expr::Field("ready_queue".into())))),
                }))),
                Box::new(Expr::U64(0)),
            ),
            // All-mode AND all deps Completed
            Expr::And(vec![
                Expr::Eq(
                    Box::new(Expr::MapGet {
                        map: Box::new(Expr::Field("node_dependency_modes".into())),
                        key: Box::new(Expr::Head(Box::new(Expr::Field("ready_queue".into())))),
                    }),
                    Box::new(Expr::NamedVariant {
                        enum_name: "DependencyMode".into(),
                        variant: "All".into(),
                    }),
                ),
                Expr::Call {
                    helper: "AllDepsCompleted".into(),
                    args: vec![Expr::Head(Box::new(Expr::Field("ready_queue".into())))],
                },
            ]),
            // Any-mode AND any dep Completed
            Expr::And(vec![
                Expr::Eq(
                    Box::new(Expr::MapGet {
                        map: Box::new(Expr::Field("node_dependency_modes".into())),
                        key: Box::new(Expr::Head(Box::new(Expr::Field("ready_queue".into())))),
                    }),
                    Box::new(Expr::NamedVariant {
                        enum_name: "DependencyMode".into(),
                        variant: "Any".into(),
                    }),
                ),
                Expr::Call {
                    helper: "AnyDepCompleted".into(),
                    args: vec![Expr::Head(Box::new(Expr::Field("ready_queue".into())))],
                },
            ]),
        ]),
    }
}

/// Guard: head node should be skipped.
/// All-mode: any dep has a non-Completed terminal status (Failed, Skipped, Canceled).
fn guard_head_should_skip() -> Guard {
    Guard {
        name: "head_should_skip".into(),
        // All-mode AND at least one dep is Failed, Skipped, or Canceled (not Completed)
        expr: Expr::And(vec![
            Expr::Eq(
                Box::new(Expr::MapGet {
                    map: Box::new(Expr::Field("node_dependency_modes".into())),
                    key: Box::new(Expr::Head(Box::new(Expr::Field("ready_queue".into())))),
                }),
                Box::new(Expr::NamedVariant {
                    enum_name: "DependencyMode".into(),
                    variant: "All".into(),
                }),
            ),
            // At least one dep is in a non-Completed terminal state
            Expr::Quantified {
                quantifier: Quantifier::Any,
                binding: "dep_id".into(),
                over: Box::new(Expr::MapGet {
                    map: Box::new(Expr::Field("node_dependencies".into())),
                    key: Box::new(Expr::Head(Box::new(Expr::Field("ready_queue".into())))),
                }),
                body: Box::new(Expr::Or(vec![
                    Expr::Eq(
                        Box::new(Expr::MapGet {
                            map: Box::new(Expr::Field("node_status".into())),
                            key: Box::new(Expr::Binding("dep_id".into())),
                        }),
                        Box::new(Expr::NamedVariant {
                            enum_name: "NodeRunStatus".into(),
                            variant: "Failed".into(),
                        }),
                    ),
                    Expr::Eq(
                        Box::new(Expr::MapGet {
                            map: Box::new(Expr::Field("node_status".into())),
                            key: Box::new(Expr::Binding("dep_id".into())),
                        }),
                        Box::new(Expr::NamedVariant {
                            enum_name: "NodeRunStatus".into(),
                            variant: "Skipped".into(),
                        }),
                    ),
                    Expr::Eq(
                        Box::new(Expr::MapGet {
                            map: Box::new(Expr::Field("node_status".into())),
                            key: Box::new(Expr::Binding("dep_id".into())),
                        }),
                        Box::new(Expr::NamedVariant {
                            enum_name: "NodeRunStatus".into(),
                            variant: "Canceled".into(),
                        }),
                    ),
                ])),
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
                    map: Box::new(Expr::Field("node_dependency_modes".into())),
                    key: Box::new(Expr::Head(Box::new(Expr::Field("ready_queue".into())))),
                }),
                Box::new(Expr::NamedVariant {
                    enum_name: "DependencyMode".into(),
                    variant: "Any".into(),
                }),
            ),
            // All deps are in a terminal state (Completed/Failed/Skipped/Canceled)
            Expr::Quantified {
                quantifier: Quantifier::All,
                binding: "dep_id".into(),
                over: Box::new(Expr::MapGet {
                    map: Box::new(Expr::Field("node_dependencies".into())),
                    key: Box::new(Expr::Head(Box::new(Expr::Field("ready_queue".into())))),
                }),
                body: Box::new(dep_is_terminal_expr("dep_id")),
            },
            // No dep is Completed (otherwise would have been admitted as run)
            Expr::Not(Box::new(Expr::Call {
                helper: "AnyDepCompleted".into(),
                args: vec![Expr::Head(Box::new(Expr::Field("ready_queue".into())))],
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
            field: "last_admitted_node".into(),
            expr: Expr::Head(Box::new(Expr::Field("ready_queue".into()))),
        },
        // Set status to Running
        Update::MapInsert {
            field: "node_status".into(),
            key: Expr::Head(Box::new(Expr::Field("ready_queue".into()))),
            value: Expr::NamedVariant {
                enum_name: "NodeRunStatus".into(),
                variant: status_variant.into(),
            },
        },
        // Pop the head from the queue
        Update::SeqPopFront {
            field: "ready_queue".into(),
        },
    ]
}

/// Updates for AdmitNextReadyNode when admitted as terminal (Skipped/Failed):
/// Sets node_status, pops from queue, then refreshes the frontier.
fn admit_terminal_updates(status_variant: &str) -> Vec<Update> {
    let mut updates = vec![
        // Store the head node ID so effects can reference it after the pop
        Update::Assign {
            field: "last_admitted_node".into(),
            expr: Expr::Head(Box::new(Expr::Field("ready_queue".into()))),
        },
        // Set terminal status
        Update::MapInsert {
            field: "node_status".into(),
            key: Expr::Head(Box::new(Expr::Field("ready_queue".into()))),
            value: Expr::NamedVariant {
                enum_name: "NodeRunStatus".into(),
                variant: status_variant.into(),
            },
        },
        // Pop the head from the queue
        Update::SeqPopFront {
            field: "ready_queue".into(),
        },
    ];
    // Refresh frontier: any newly-eligible nodes become Ready
    updates.extend(refresh_ready_frontier_updates());
    updates
}

pub fn flow_frame_machine() -> MachineSchema {
    MachineSchema {
        machine: "FlowFrameMachine".into(),
        version: 1,
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
                field("frame_id", TypeRef::Named("FrameId".into())),
                // Last admitted node (set before pop so effects can reference it)
                field("last_admitted_node", TypeRef::Named("FlowNodeId".into())),
                // Node graph
                field(
                    "tracked_nodes",
                    TypeRef::Set(Box::new(TypeRef::Named("FlowNodeId".into()))),
                ),
                field(
                    "ordered_nodes",
                    TypeRef::Seq(Box::new(TypeRef::Named("FlowNodeId".into()))),
                ),
                field(
                    "node_kind",
                    TypeRef::Map(
                        Box::new(TypeRef::Named("FlowNodeId".into())),
                        Box::new(TypeRef::Enum("FlowNodeKind".into())),
                    ),
                ),
                // Dependency graph
                field(
                    "node_dependencies",
                    TypeRef::Map(
                        Box::new(TypeRef::Named("FlowNodeId".into())),
                        Box::new(TypeRef::Seq(Box::new(TypeRef::Named("FlowNodeId".into())))),
                    ),
                ),
                field(
                    "node_dependency_modes",
                    TypeRef::Map(
                        Box::new(TypeRef::Named("FlowNodeId".into())),
                        Box::new(TypeRef::Enum("DependencyMode".into())),
                    ),
                ),
                field(
                    "node_branches",
                    TypeRef::Map(
                        Box::new(TypeRef::Named("FlowNodeId".into())),
                        Box::new(TypeRef::Option(Box::new(TypeRef::Named("BranchId".into())))),
                    ),
                ),
                // Status
                field(
                    "node_status",
                    TypeRef::Map(
                        Box::new(TypeRef::Named("FlowNodeId".into())),
                        Box::new(TypeRef::Enum("NodeRunStatus".into())),
                    ),
                ),
                // Scheduler
                field(
                    "ready_queue",
                    TypeRef::Seq(Box::new(TypeRef::Named("FlowNodeId".into()))),
                ),
                // Output tracking
                field(
                    "output_recorded",
                    TypeRef::Map(
                        Box::new(TypeRef::Named("FlowNodeId".into())),
                        Box::new(TypeRef::Bool),
                    ),
                ),
                // Condition results
                field(
                    "node_condition_results",
                    TypeRef::Map(
                        Box::new(TypeRef::Named("FlowNodeId".into())),
                        Box::new(TypeRef::Option(Box::new(TypeRef::Bool))),
                    ),
                ),
            ],
            init: InitSchema {
                phase: "Absent".into(),
                fields: vec![
                    init("frame_id", Expr::String(String::new())),
                    init("last_admitted_node", Expr::String(String::new())),
                    init("tracked_nodes", Expr::EmptySet),
                    init("ordered_nodes", Expr::SeqLiteral(vec![])),
                    init("node_kind", Expr::EmptyMap),
                    init("node_dependencies", Expr::EmptyMap),
                    init("node_dependency_modes", Expr::EmptyMap),
                    init("node_branches", Expr::EmptyMap),
                    init("node_status", Expr::EmptyMap),
                    init("ready_queue", Expr::SeqLiteral(vec![])),
                    init("output_recorded", Expr::EmptyMap),
                    init("node_condition_results", Expr::EmptyMap),
                ],
            },
            terminal_phases: vec!["Completed".into(), "Failed".into(), "Canceled".into()],
        },
        inputs: EnumSchema {
            name: "FlowFrameInput".into(),
            variants: vec![
                VariantSchema {
                    name: "StartFrame".into(),
                    fields: vec![
                        field("frame_id", TypeRef::Named("FrameId".into())),
                        field(
                            "tracked_nodes",
                            TypeRef::Set(Box::new(TypeRef::Named("FlowNodeId".into()))),
                        ),
                        field(
                            "ordered_nodes",
                            TypeRef::Seq(Box::new(TypeRef::Named("FlowNodeId".into()))),
                        ),
                        field(
                            "node_kind",
                            TypeRef::Map(
                                Box::new(TypeRef::Named("FlowNodeId".into())),
                                Box::new(TypeRef::Enum("FlowNodeKind".into())),
                            ),
                        ),
                        field(
                            "node_dependencies",
                            TypeRef::Map(
                                Box::new(TypeRef::Named("FlowNodeId".into())),
                                Box::new(TypeRef::Seq(Box::new(TypeRef::Named(
                                    "FlowNodeId".into(),
                                )))),
                            ),
                        ),
                        field(
                            "node_dependency_modes",
                            TypeRef::Map(
                                Box::new(TypeRef::Named("FlowNodeId".into())),
                                Box::new(TypeRef::Enum("DependencyMode".into())),
                            ),
                        ),
                        field(
                            "node_branches",
                            TypeRef::Map(
                                Box::new(TypeRef::Named("FlowNodeId".into())),
                                Box::new(TypeRef::Option(Box::new(TypeRef::Named(
                                    "BranchId".into(),
                                )))),
                            ),
                        ),
                    ],
                },
                variant("AdmitNextReadyNode"),
                VariantSchema {
                    name: "CompleteNode".into(),
                    fields: vec![field("node_id", TypeRef::Named("FlowNodeId".into()))],
                },
                VariantSchema {
                    name: "RecordNodeOutput".into(),
                    fields: vec![field("node_id", TypeRef::Named("FlowNodeId".into()))],
                },
                VariantSchema {
                    name: "FailNode".into(),
                    fields: vec![field("node_id", TypeRef::Named("FlowNodeId".into()))],
                },
                VariantSchema {
                    name: "SkipNode".into(),
                    fields: vec![field("node_id", TypeRef::Named("FlowNodeId".into()))],
                },
                VariantSchema {
                    name: "CancelNode".into(),
                    fields: vec![field("node_id", TypeRef::Named("FlowNodeId".into()))],
                },
                variant("TerminalizeCompleted"),
                variant("TerminalizeFailed"),
                variant("TerminalizeCanceled"),
            ],
        },
        effects: EnumSchema {
            name: "FlowFrameEffect".into(),
            variants: vec![
                VariantSchema {
                    name: "ReadyFrontierChanged".into(),
                    fields: vec![field("frame_id", TypeRef::Named("FrameId".into()))],
                },
                VariantSchema {
                    name: "AdmitStepWork".into(),
                    fields: vec![
                        field("frame_id", TypeRef::Named("FrameId".into())),
                        field("node_id", TypeRef::Named("FlowNodeId".into())),
                    ],
                },
                VariantSchema {
                    name: "StartLoopNode".into(),
                    fields: vec![
                        field("frame_id", TypeRef::Named("FrameId".into())),
                        field("node_id", TypeRef::Named("FlowNodeId".into())),
                    ],
                },
                VariantSchema {
                    name: "PersistStepOutput".into(),
                    fields: vec![
                        field("frame_id", TypeRef::Named("FrameId".into())),
                        field("node_id", TypeRef::Named("FlowNodeId".into())),
                    ],
                },
                VariantSchema {
                    name: "NodeExecutionReleased".into(),
                    fields: vec![
                        field("frame_id", TypeRef::Named("FrameId".into())),
                        field("node_id", TypeRef::Named("FlowNodeId".into())),
                    ],
                },
                VariantSchema {
                    name: "FrameTerminalized".into(),
                    fields: vec![
                        field("frame_id", TypeRef::Named("FrameId".into())),
                        field("status", TypeRef::Enum("FlowFrameStatus".into())),
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
                params: vec![field("node_id", TypeRef::Named("FlowNodeId".into()))],
                returns: TypeRef::Bool,
                body: Expr::IfElse {
                    // No deps → immediately eligible
                    condition: Box::new(Expr::Eq(
                        Box::new(Expr::Len(Box::new(Expr::MapGet {
                            map: Box::new(Expr::Field("node_dependencies".into())),
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
                                map: Box::new(Expr::Field("node_dependency_modes".into())),
                                key: Box::new(Expr::Binding("node_id".into())),
                            }),
                            Box::new(Expr::NamedVariant {
                                enum_name: "DependencyMode".into(),
                                variant: "All".into(),
                            }),
                        )),
                        then_expr: Box::new(Expr::Quantified {
                            quantifier: Quantifier::All,
                            binding: "dep_id".into(),
                            over: Box::new(Expr::MapGet {
                                map: Box::new(Expr::Field("node_dependencies".into())),
                                key: Box::new(Expr::Binding("node_id".into())),
                            }),
                            // dep is terminal iff its status is one of the 4 terminal states
                            body: Box::new(dep_is_terminal_expr("dep_id")),
                        }),
                        // Any-mode: eligible when any dep is Completed OR all deps are terminal
                        else_expr: Box::new(Expr::Or(vec![
                            Expr::Quantified {
                                quantifier: Quantifier::Any,
                                binding: "dep_id".into(),
                                over: Box::new(Expr::MapGet {
                                    map: Box::new(Expr::Field("node_dependencies".into())),
                                    key: Box::new(Expr::Binding("node_id".into())),
                                }),
                                body: Box::new(Expr::Eq(
                                    Box::new(Expr::MapGet {
                                        map: Box::new(Expr::Field("node_status".into())),
                                        key: Box::new(Expr::Binding("dep_id".into())),
                                    }),
                                    Box::new(Expr::NamedVariant {
                                        enum_name: "NodeRunStatus".into(),
                                        variant: "Completed".into(),
                                    }),
                                )),
                            },
                            // All deps terminal (explicitly checked)
                            Expr::Quantified {
                                quantifier: Quantifier::All,
                                binding: "dep_id".into(),
                                over: Box::new(Expr::MapGet {
                                    map: Box::new(Expr::Field("node_dependencies".into())),
                                    key: Box::new(Expr::Binding("node_id".into())),
                                }),
                                body: Box::new(dep_is_terminal_expr("dep_id")),
                            },
                        ])),
                    }),
                },
            },
            // AllDepsCompleted(node_id): true if ALL deps are Completed
            HelperSchema {
                name: "AllDepsCompleted".into(),
                params: vec![field("node_id", TypeRef::Named("FlowNodeId".into()))],
                returns: TypeRef::Bool,
                body: Expr::Quantified {
                    quantifier: Quantifier::All,
                    binding: "dep_id".into(),
                    over: Box::new(Expr::MapGet {
                        map: Box::new(Expr::Field("node_dependencies".into())),
                        key: Box::new(Expr::Binding("node_id".into())),
                    }),
                    body: Box::new(Expr::Eq(
                        Box::new(Expr::MapGet {
                            map: Box::new(Expr::Field("node_status".into())),
                            key: Box::new(Expr::Binding("dep_id".into())),
                        }),
                        Box::new(Expr::NamedVariant {
                            enum_name: "NodeRunStatus".into(),
                            variant: "Completed".into(),
                        }),
                    )),
                },
            },
            // AnyDepCompleted(node_id): true if ANY dep is Completed
            HelperSchema {
                name: "AnyDepCompleted".into(),
                params: vec![field("node_id", TypeRef::Named("FlowNodeId".into()))],
                returns: TypeRef::Bool,
                body: Expr::Quantified {
                    quantifier: Quantifier::Any,
                    binding: "dep_id".into(),
                    over: Box::new(Expr::MapGet {
                        map: Box::new(Expr::Field("node_dependencies".into())),
                        key: Box::new(Expr::Binding("node_id".into())),
                    }),
                    body: Box::new(Expr::Eq(
                        Box::new(Expr::MapGet {
                            map: Box::new(Expr::Field("node_status".into())),
                            key: Box::new(Expr::Binding("dep_id".into())),
                        }),
                        Box::new(Expr::NamedVariant {
                            enum_name: "NodeRunStatus".into(),
                            variant: "Completed".into(),
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
                    over: Box::new(Expr::Field("tracked_nodes".into())),
                    body: Box::new(Expr::Or(vec![
                        Expr::Eq(
                            Box::new(Expr::MapGet {
                                map: Box::new(Expr::Field("node_status".into())),
                                key: Box::new(Expr::Binding("t_node".into())),
                            }),
                            Box::new(Expr::NamedVariant {
                                enum_name: "NodeRunStatus".into(),
                                variant: "Completed".into(),
                            }),
                        ),
                        Expr::Eq(
                            Box::new(Expr::MapGet {
                                map: Box::new(Expr::Field("node_status".into())),
                                key: Box::new(Expr::Binding("t_node".into())),
                            }),
                            Box::new(Expr::NamedVariant {
                                enum_name: "NodeRunStatus".into(),
                                variant: "Failed".into(),
                            }),
                        ),
                        Expr::Eq(
                            Box::new(Expr::MapGet {
                                map: Box::new(Expr::Field("node_status".into())),
                                key: Box::new(Expr::Binding("t_node".into())),
                            }),
                            Box::new(Expr::NamedVariant {
                                enum_name: "NodeRunStatus".into(),
                                variant: "Skipped".into(),
                            }),
                        ),
                        Expr::Eq(
                            Box::new(Expr::MapGet {
                                map: Box::new(Expr::Field("node_status".into())),
                                key: Box::new(Expr::Binding("t_node".into())),
                            }),
                            Box::new(Expr::NamedVariant {
                                enum_name: "NodeRunStatus".into(),
                                variant: "Canceled".into(),
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
                    over: Box::new(Expr::SeqElements(Box::new(Expr::Field(
                        "ready_queue".into(),
                    )))),
                    body: Box::new(Expr::Eq(
                        Box::new(Expr::MapGet {
                            map: Box::new(Expr::Field("node_status".into())),
                            key: Box::new(Expr::Binding("q_node".into())),
                        }),
                        Box::new(Expr::NamedVariant {
                            enum_name: "NodeRunStatus".into(),
                            variant: "Ready".into(),
                        }),
                    )),
                },
                // Every node with Ready status is in ready_queue
                Expr::Quantified {
                    quantifier: Quantifier::All,
                    binding: "t_node".into(),
                    over: Box::new(Expr::Field("tracked_nodes".into())),
                    body: Box::new(Expr::Or(vec![
                        Expr::Neq(
                            Box::new(Expr::MapGet {
                                map: Box::new(Expr::Field("node_status".into())),
                                key: Box::new(Expr::Binding("t_node".into())),
                            }),
                            Box::new(Expr::NamedVariant {
                                enum_name: "NodeRunStatus".into(),
                                variant: "Ready".into(),
                            }),
                        ),
                        Expr::Contains {
                            collection: Box::new(Expr::SeqElements(Box::new(Expr::Field(
                                "ready_queue".into(),
                            )))),
                            value: Box::new(Expr::Binding("t_node".into())),
                        },
                    ])),
                },
            ]),
        }],
        transitions: vec![
            // ----------------------------------------------------------------
            // StartFrame: Absent -> Running
            // Stores frame topology, initializes all nodes as Pending,
            // then seeds the ready frontier.
            // ----------------------------------------------------------------
            TransitionSchema {
                name: "StartFrame".into(),
                from: vec!["Absent".into()],
                on: InputMatch {
                    variant: "StartFrame".into(),
                    bindings: vec![
                        "frame_id".into(),
                        "tracked_nodes".into(),
                        "ordered_nodes".into(),
                        "node_kind".into(),
                        "node_dependencies".into(),
                        "node_dependency_modes".into(),
                        "node_branches".into(),
                    ],
                },
                guards: vec![],
                updates: start_frame_updates(),
                to: "Running".into(),
                emit: vec![emit_ready_frontier_changed()],
            },
            // ----------------------------------------------------------------
            // AdmitNextReadyNode_StepRun
            // Head of ready_queue is a Step node whose deps allow running.
            // ----------------------------------------------------------------
            TransitionSchema {
                name: "AdmitNextReadyNode_StepRun".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    variant: "AdmitNextReadyNode".into(),
                    bindings: vec![],
                },
                guards: vec![
                    guard_queue_non_empty(),
                    guard_head_is_step(),
                    guard_head_can_run(),
                ],
                updates: admit_run_updates("Running"),
                to: "Running".into(),
                emit: vec![EffectEmit {
                    variant: "AdmitStepWork".into(),
                    fields: IndexMap::from([
                        ("frame_id".into(), Expr::Field("frame_id".into())),
                        ("node_id".into(), Expr::Field("last_admitted_node".into())),
                    ]),
                }],
            },
            // ----------------------------------------------------------------
            // AdmitNextReadyNode_LoopRun
            // Head of ready_queue is a Loop node whose deps allow running.
            // ----------------------------------------------------------------
            TransitionSchema {
                name: "AdmitNextReadyNode_LoopRun".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    variant: "AdmitNextReadyNode".into(),
                    bindings: vec![],
                },
                guards: vec![
                    guard_queue_non_empty(),
                    guard_head_is_loop(),
                    guard_head_can_run(),
                ],
                updates: admit_run_updates("Running"),
                to: "Running".into(),
                emit: vec![EffectEmit {
                    variant: "StartLoopNode".into(),
                    fields: IndexMap::from([
                        ("frame_id".into(), Expr::Field("frame_id".into())),
                        ("node_id".into(), Expr::Field("last_admitted_node".into())),
                    ]),
                }],
            },
            // ----------------------------------------------------------------
            // AdmitNextReadyNode_Skip
            // Head of ready_queue should be skipped (All-mode with failed dep).
            // ----------------------------------------------------------------
            TransitionSchema {
                name: "AdmitNextReadyNode_Skip".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    variant: "AdmitNextReadyNode".into(),
                    bindings: vec![],
                },
                guards: vec![guard_queue_non_empty(), guard_head_should_skip()],
                updates: admit_terminal_updates("Skipped"),
                to: "Running".into(),
                emit: vec![
                    emit_node_execution_released(Expr::Field("last_admitted_node".into())),
                    emit_ready_frontier_changed(),
                ],
            },
            // ----------------------------------------------------------------
            // AdmitNextReadyNode_Fail
            // Head of ready_queue should fail (Any-mode, all deps terminal, none Completed).
            // ----------------------------------------------------------------
            TransitionSchema {
                name: "AdmitNextReadyNode_Fail".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    variant: "AdmitNextReadyNode".into(),
                    bindings: vec![],
                },
                guards: vec![guard_queue_non_empty(), guard_head_should_fail()],
                updates: admit_terminal_updates("Failed"),
                to: "Running".into(),
                emit: vec![
                    emit_node_execution_released(Expr::Field("last_admitted_node".into())),
                    emit_ready_frontier_changed(),
                ],
            },
            // ----------------------------------------------------------------
            // CompleteNode: marks a Running node as Completed, refreshes frontier
            // ----------------------------------------------------------------
            TransitionSchema {
                name: "CompleteNode".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    variant: "CompleteNode".into(),
                    bindings: vec!["node_id".into()],
                },
                guards: vec![Guard {
                    name: "node_is_running".into(),
                    expr: Expr::Eq(
                        Box::new(Expr::MapGet {
                            map: Box::new(Expr::Field("node_status".into())),
                            key: Box::new(Expr::Binding("node_id".into())),
                        }),
                        Box::new(Expr::NamedVariant {
                            enum_name: "NodeRunStatus".into(),
                            variant: "Running".into(),
                        }),
                    ),
                }],
                updates: {
                    let mut updates = vec![Update::MapInsert {
                        field: "node_status".into(),
                        key: Expr::Binding("node_id".into()),
                        value: Expr::NamedVariant {
                            enum_name: "NodeRunStatus".into(),
                            variant: "Completed".into(),
                        },
                    }];
                    updates.extend(refresh_ready_frontier_updates());
                    updates
                },
                to: "Running".into(),
                emit: vec![
                    emit_node_execution_released(Expr::Binding("node_id".into())),
                    emit_ready_frontier_changed(),
                ],
            },
            // ----------------------------------------------------------------
            // RecordNodeOutput: marks output as recorded for a node
            // ----------------------------------------------------------------
            TransitionSchema {
                name: "RecordNodeOutput".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    variant: "RecordNodeOutput".into(),
                    bindings: vec!["node_id".into()],
                },
                guards: vec![],
                updates: vec![Update::MapInsert {
                    field: "output_recorded".into(),
                    key: Expr::Binding("node_id".into()),
                    value: Expr::Bool(true),
                }],
                to: "Running".into(),
                emit: vec![EffectEmit {
                    variant: "PersistStepOutput".into(),
                    fields: IndexMap::from([
                        ("frame_id".into(), Expr::Field("frame_id".into())),
                        ("node_id".into(), Expr::Binding("node_id".into())),
                    ]),
                }],
            },
            // ----------------------------------------------------------------
            // FailNode: marks a Running node as Failed, refreshes frontier
            // ----------------------------------------------------------------
            TransitionSchema {
                name: "FailNode".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    variant: "FailNode".into(),
                    bindings: vec!["node_id".into()],
                },
                guards: vec![Guard {
                    name: "node_is_running".into(),
                    expr: Expr::Eq(
                        Box::new(Expr::MapGet {
                            map: Box::new(Expr::Field("node_status".into())),
                            key: Box::new(Expr::Binding("node_id".into())),
                        }),
                        Box::new(Expr::NamedVariant {
                            enum_name: "NodeRunStatus".into(),
                            variant: "Running".into(),
                        }),
                    ),
                }],
                updates: {
                    let mut updates = vec![Update::MapInsert {
                        field: "node_status".into(),
                        key: Expr::Binding("node_id".into()),
                        value: Expr::NamedVariant {
                            enum_name: "NodeRunStatus".into(),
                            variant: "Failed".into(),
                        },
                    }];
                    updates.extend(refresh_ready_frontier_updates());
                    updates
                },
                to: "Running".into(),
                emit: vec![
                    emit_node_execution_released(Expr::Binding("node_id".into())),
                    emit_ready_frontier_changed(),
                ],
            },
            // ----------------------------------------------------------------
            // SkipNode: marks a Running node as Skipped, refreshes frontier
            // ----------------------------------------------------------------
            TransitionSchema {
                name: "SkipNode".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    variant: "SkipNode".into(),
                    bindings: vec!["node_id".into()],
                },
                guards: vec![Guard {
                    name: "node_is_running".into(),
                    expr: Expr::Eq(
                        Box::new(Expr::MapGet {
                            map: Box::new(Expr::Field("node_status".into())),
                            key: Box::new(Expr::Binding("node_id".into())),
                        }),
                        Box::new(Expr::NamedVariant {
                            enum_name: "NodeRunStatus".into(),
                            variant: "Running".into(),
                        }),
                    ),
                }],
                updates: {
                    let mut updates = vec![Update::MapInsert {
                        field: "node_status".into(),
                        key: Expr::Binding("node_id".into()),
                        value: Expr::NamedVariant {
                            enum_name: "NodeRunStatus".into(),
                            variant: "Skipped".into(),
                        },
                    }];
                    updates.extend(refresh_ready_frontier_updates());
                    updates
                },
                to: "Running".into(),
                emit: vec![
                    emit_node_execution_released(Expr::Binding("node_id".into())),
                    emit_ready_frontier_changed(),
                ],
            },
            // ----------------------------------------------------------------
            // CancelNode: marks a Running node as Canceled, refreshes frontier
            // ----------------------------------------------------------------
            TransitionSchema {
                name: "CancelNode".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    variant: "CancelNode".into(),
                    bindings: vec!["node_id".into()],
                },
                guards: vec![Guard {
                    name: "node_is_running".into(),
                    expr: Expr::Eq(
                        Box::new(Expr::MapGet {
                            map: Box::new(Expr::Field("node_status".into())),
                            key: Box::new(Expr::Binding("node_id".into())),
                        }),
                        Box::new(Expr::NamedVariant {
                            enum_name: "NodeRunStatus".into(),
                            variant: "Running".into(),
                        }),
                    ),
                }],
                updates: {
                    let mut updates = vec![Update::MapInsert {
                        field: "node_status".into(),
                        key: Expr::Binding("node_id".into()),
                        value: Expr::NamedVariant {
                            enum_name: "NodeRunStatus".into(),
                            variant: "Canceled".into(),
                        },
                    }];
                    updates.extend(refresh_ready_frontier_updates());
                    updates
                },
                to: "Running".into(),
                emit: vec![
                    emit_node_execution_released(Expr::Binding("node_id".into())),
                    emit_ready_frontier_changed(),
                ],
            },
            // ----------------------------------------------------------------
            // TerminalizeCompleted: Running -> Completed
            // Guard: all tracked nodes must be in a terminal state.
            // ----------------------------------------------------------------
            TransitionSchema {
                name: "TerminalizeCompleted".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    variant: "TerminalizeCompleted".into(),
                    bindings: vec![],
                },
                guards: vec![Guard {
                    name: "all_nodes_terminal".into(),
                    expr: Expr::Call {
                        helper: "AllNodesTerminal".into(),
                        args: vec![],
                    },
                }],
                updates: vec![],
                to: "Completed".into(),
                emit: vec![EffectEmit {
                    variant: "FrameTerminalized".into(),
                    fields: IndexMap::from([
                        ("frame_id".into(), Expr::Field("frame_id".into())),
                        (
                            "status".into(),
                            Expr::NamedVariant {
                                enum_name: "FlowFrameStatus".into(),
                                variant: "Completed".into(),
                            },
                        ),
                    ]),
                }],
            },
            // ----------------------------------------------------------------
            // TerminalizeFailed: Running|Absent -> Failed
            // ----------------------------------------------------------------
            TransitionSchema {
                name: "TerminalizeFailed".into(),
                from: vec!["Running".into(), "Absent".into()],
                on: InputMatch {
                    variant: "TerminalizeFailed".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![],
                to: "Failed".into(),
                emit: vec![EffectEmit {
                    variant: "FrameTerminalized".into(),
                    fields: IndexMap::from([
                        ("frame_id".into(), Expr::Field("frame_id".into())),
                        (
                            "status".into(),
                            Expr::NamedVariant {
                                enum_name: "FlowFrameStatus".into(),
                                variant: "Failed".into(),
                            },
                        ),
                    ]),
                }],
            },
            // ----------------------------------------------------------------
            // TerminalizeCanceled: Running|Absent -> Canceled
            // ----------------------------------------------------------------
            TransitionSchema {
                name: "TerminalizeCanceled".into(),
                from: vec!["Running".into(), "Absent".into()],
                on: InputMatch {
                    variant: "TerminalizeCanceled".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![],
                to: "Canceled".into(),
                emit: vec![EffectEmit {
                    variant: "FrameTerminalized".into(),
                    fields: IndexMap::from([
                        ("frame_id".into(), Expr::Field("frame_id".into())),
                        (
                            "status".into(),
                            Expr::NamedVariant {
                                enum_name: "FlowFrameStatus".into(),
                                variant: "Canceled".into(),
                            },
                        ),
                    ]),
                }],
            },
        ],
        effect_dispositions: vec![
            disposition("ReadyFrontierChanged", EffectDisposition::External),
            disposition("AdmitStepWork", EffectDisposition::External),
            disposition("StartLoopNode", EffectDisposition::External),
            disposition("PersistStepOutput", EffectDisposition::Local),
            disposition("NodeExecutionReleased", EffectDisposition::External),
            disposition("FrameTerminalized", EffectDisposition::External),
        ],
    }
}

// ---------------------------------------------------------------------------
// DSL builder helpers
// ---------------------------------------------------------------------------

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

// Suppress unused import warning - InitSchema is used via the struct literal in MachineSchema
use crate::InitSchema;
