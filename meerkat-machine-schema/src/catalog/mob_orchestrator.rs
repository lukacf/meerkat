use indexmap::IndexMap;

use crate::{
    EffectDisposition, EffectDispositionRule, EffectEmit, EnumSchema, Expr, FieldInit, FieldSchema,
    Guard, InitSchema, InputMatch, InvariantSchema, MachineSchema, RustBinding, StateSchema,
    TransitionSchema, TypeRef, Update, VariantSchema,
};

pub fn mob_orchestrator_machine() -> MachineSchema {
    MachineSchema {
        machine: "MobOrchestratorMachine".into(),
        version: 2,
        rust: RustBinding {
            crate_name: "meerkat-mob".into(),
            module: "generated::mob_orchestrator".into(),
        },
        state: StateSchema {
            phase: EnumSchema {
                name: "OrchestratorState".into(),
                variants: vec![
                    variant("Creating"),
                    variant("Running"),
                    variant("Stopped"),
                    variant("Completed"),
                    variant("Destroyed"),
                ],
            },
            fields: vec![
                field("coordinator_bound", TypeRef::Bool),
                field("pending_spawn_count", TypeRef::U32),
                field("active_flow_count", TypeRef::U32),
                field("topology_revision", TypeRef::U32),
                field("supervisor_active", TypeRef::Bool),
            ],
            init: InitSchema {
                phase: "Creating".into(),
                fields: vec![
                    init("coordinator_bound", Expr::Bool(false)),
                    init("pending_spawn_count", Expr::U64(0)),
                    init("active_flow_count", Expr::U64(0)),
                    init("topology_revision", Expr::U64(0)),
                    init("supervisor_active", Expr::Bool(false)),
                ],
            },
            terminal_phases: vec!["Destroyed".into()],
        },
        inputs: EnumSchema {
            name: "MobOrchestratorInput".into(),
            variants: vec![
                variant("InitializeOrchestrator"),
                variant("BindCoordinator"),
                variant("UnbindCoordinator"),
                variant("StageSpawn"),
                variant("CompleteSpawn"),
                variant("StartFlow"),
                variant("CompleteFlow"),
                variant("StopOrchestrator"),
                variant("ResumeOrchestrator"),
                variant("MarkCompleted"),
                variant("DestroyOrchestrator"),
                // Phase C: force-cancel a member's in-flight turn
                variant("ForceCancelMember"),
            ],
        },
        effects: EnumSchema {
            name: "MobOrchestratorEffect".into(),
            variants: vec![
                variant("ActivateSupervisor"),
                variant("DeactivateSupervisor"),
                variant("FlowActivated"),
                variant("FlowDeactivated"),
                variant("EmitOrchestratorNotice"),
                // Phase C: member force-cancel initiated
                variant("MemberForceCancelled"),
            ],
        },
        helpers: vec![],
        derived: vec![],
        invariants: vec![InvariantSchema {
            name: "destroyed_is_terminal".into(),
            expr: Expr::Or(vec![
                Expr::Neq(
                    Box::new(Expr::CurrentPhase),
                    Box::new(Expr::Phase("Destroyed".into())),
                ),
                Expr::Neq(
                    Box::new(Expr::Field("supervisor_active".into())),
                    Box::new(Expr::Bool(true)),
                ),
            ]),
        }],
        transitions: vec![
            TransitionSchema {
                name: "InitializeOrchestrator".into(),
                from: vec!["Creating".into()],
                on: InputMatch {
                    variant: "InitializeOrchestrator".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![Update::Assign {
                    field: "supervisor_active".into(),
                    expr: Expr::Bool(true),
                }],
                to: "Running".into(),
                emit: vec![EffectEmit {
                    variant: "ActivateSupervisor".into(),
                    fields: IndexMap::new(),
                }],
            },
            TransitionSchema {
                name: "BindCoordinator".into(),
                from: vec!["Running".into(), "Stopped".into(), "Completed".into()],
                on: InputMatch {
                    variant: "BindCoordinator".into(),
                    bindings: vec![],
                },
                guards: vec![Guard {
                    name: "coordinator_is_not_bound".into(),
                    expr: Expr::Eq(
                        Box::new(Expr::Field("coordinator_bound".into())),
                        Box::new(Expr::Bool(false)),
                    ),
                }],
                updates: vec![
                    Update::Assign {
                        field: "coordinator_bound".into(),
                        expr: Expr::Bool(true),
                    },
                    Update::Increment {
                        field: "topology_revision".into(),
                        amount: 1,
                    },
                ],
                to: "Running".into(),
                emit: vec![EffectEmit {
                    variant: "EmitOrchestratorNotice".into(),
                    fields: IndexMap::new(),
                }],
            },
            TransitionSchema {
                name: "UnbindCoordinator".into(),
                from: vec!["Running".into(), "Stopped".into(), "Completed".into()],
                on: InputMatch {
                    variant: "UnbindCoordinator".into(),
                    bindings: vec![],
                },
                guards: vec![
                    Guard {
                        name: "coordinator_is_bound".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field("coordinator_bound".into())),
                            Box::new(Expr::Bool(true)),
                        ),
                    },
                    Guard {
                        name: "no_pending_spawns".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field("pending_spawn_count".into())),
                            Box::new(Expr::U64(0)),
                        ),
                    },
                ],
                updates: vec![
                    Update::Assign {
                        field: "coordinator_bound".into(),
                        expr: Expr::Bool(false),
                    },
                    Update::Increment {
                        field: "topology_revision".into(),
                        amount: 1,
                    },
                ],
                to: "Stopped".into(),
                emit: vec![EffectEmit {
                    variant: "EmitOrchestratorNotice".into(),
                    fields: IndexMap::new(),
                }],
            },
            TransitionSchema {
                name: "StageSpawn".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    variant: "StageSpawn".into(),
                    bindings: vec![],
                },
                guards: vec![Guard {
                    name: "coordinator_is_bound".into(),
                    expr: Expr::Eq(
                        Box::new(Expr::Field("coordinator_bound".into())),
                        Box::new(Expr::Bool(true)),
                    ),
                }],
                updates: vec![
                    Update::Increment {
                        field: "pending_spawn_count".into(),
                        amount: 1,
                    },
                    Update::Increment {
                        field: "topology_revision".into(),
                        amount: 1,
                    },
                ],
                to: "Running".into(),
                emit: vec![EffectEmit {
                    variant: "EmitOrchestratorNotice".into(),
                    fields: IndexMap::new(),
                }],
            },
            TransitionSchema {
                name: "CompleteSpawn".into(),
                from: vec!["Running".into(), "Stopped".into()],
                on: InputMatch {
                    variant: "CompleteSpawn".into(),
                    bindings: vec![],
                },
                guards: vec![Guard {
                    name: "has_pending_spawns".into(),
                    expr: Expr::Gt(
                        Box::new(Expr::Field("pending_spawn_count".into())),
                        Box::new(Expr::U64(0)),
                    ),
                }],
                updates: vec![
                    Update::Decrement {
                        field: "pending_spawn_count".into(),
                        amount: 1,
                    },
                    Update::Increment {
                        field: "topology_revision".into(),
                        amount: 1,
                    },
                ],
                to: "Running".into(),
                emit: vec![EffectEmit {
                    variant: "EmitOrchestratorNotice".into(),
                    fields: IndexMap::new(),
                }],
            },
            TransitionSchema {
                name: "StartFlow".into(),
                from: vec!["Running".into(), "Completed".into()],
                on: InputMatch {
                    variant: "StartFlow".into(),
                    bindings: vec![],
                },
                guards: vec![Guard {
                    name: "coordinator_is_bound".into(),
                    expr: Expr::Eq(
                        Box::new(Expr::Field("coordinator_bound".into())),
                        Box::new(Expr::Bool(true)),
                    ),
                }],
                updates: vec![Update::Increment {
                    field: "active_flow_count".into(),
                    amount: 1,
                }],
                to: "Running".into(),
                emit: vec![
                    EffectEmit {
                        variant: "FlowActivated".into(),
                        fields: IndexMap::new(),
                    },
                    EffectEmit {
                        variant: "EmitOrchestratorNotice".into(),
                        fields: IndexMap::new(),
                    },
                ],
            },
            TransitionSchema {
                name: "CompleteFlow".into(),
                from: vec!["Running".into(), "Completed".into()],
                on: InputMatch {
                    variant: "CompleteFlow".into(),
                    bindings: vec![],
                },
                guards: vec![Guard {
                    name: "has_active_flows".into(),
                    expr: Expr::Gt(
                        Box::new(Expr::Field("active_flow_count".into())),
                        Box::new(Expr::U64(0)),
                    ),
                }],
                updates: vec![Update::Decrement {
                    field: "active_flow_count".into(),
                    amount: 1,
                }],
                to: "Running".into(),
                emit: vec![
                    EffectEmit {
                        variant: "FlowDeactivated".into(),
                        fields: IndexMap::new(),
                    },
                    EffectEmit {
                        variant: "EmitOrchestratorNotice".into(),
                        fields: IndexMap::new(),
                    },
                ],
            },
            TransitionSchema {
                name: "StopOrchestrator".into(),
                from: vec!["Running".into(), "Completed".into()],
                on: InputMatch {
                    variant: "StopOrchestrator".into(),
                    bindings: vec![],
                },
                guards: vec![Guard {
                    name: "no_active_flows".into(),
                    expr: Expr::Eq(
                        Box::new(Expr::Field("active_flow_count".into())),
                        Box::new(Expr::U64(0)),
                    ),
                }],
                updates: vec![Update::Assign {
                    field: "supervisor_active".into(),
                    expr: Expr::Bool(false),
                }],
                to: "Stopped".into(),
                emit: vec![EffectEmit {
                    variant: "DeactivateSupervisor".into(),
                    fields: IndexMap::new(),
                }],
            },
            TransitionSchema {
                name: "ResumeOrchestrator".into(),
                from: vec!["Stopped".into()],
                on: InputMatch {
                    variant: "ResumeOrchestrator".into(),
                    bindings: vec![],
                },
                guards: vec![Guard {
                    name: "coordinator_is_bound".into(),
                    expr: Expr::Eq(
                        Box::new(Expr::Field("coordinator_bound".into())),
                        Box::new(Expr::Bool(true)),
                    ),
                }],
                updates: vec![Update::Assign {
                    field: "supervisor_active".into(),
                    expr: Expr::Bool(true),
                }],
                to: "Running".into(),
                emit: vec![EffectEmit {
                    variant: "ActivateSupervisor".into(),
                    fields: IndexMap::new(),
                }],
            },
            TransitionSchema {
                name: "MarkCompleted".into(),
                from: vec!["Running".into(), "Stopped".into()],
                on: InputMatch {
                    variant: "MarkCompleted".into(),
                    bindings: vec![],
                },
                guards: vec![
                    Guard {
                        name: "no_active_flows".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field("active_flow_count".into())),
                            Box::new(Expr::U64(0)),
                        ),
                    },
                    Guard {
                        name: "no_pending_spawns".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field("pending_spawn_count".into())),
                            Box::new(Expr::U64(0)),
                        ),
                    },
                ],
                updates: vec![],
                to: "Completed".into(),
                emit: vec![EffectEmit {
                    variant: "EmitOrchestratorNotice".into(),
                    fields: IndexMap::new(),
                }],
            },
            TransitionSchema {
                name: "DestroyOrchestrator".into(),
                from: vec!["Stopped".into(), "Completed".into()],
                on: InputMatch {
                    variant: "DestroyOrchestrator".into(),
                    bindings: vec![],
                },
                guards: vec![
                    Guard {
                        name: "no_pending_spawns".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field("pending_spawn_count".into())),
                            Box::new(Expr::U64(0)),
                        ),
                    },
                    Guard {
                        name: "no_active_flows".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field("active_flow_count".into())),
                            Box::new(Expr::U64(0)),
                        ),
                    },
                ],
                updates: vec![
                    Update::Assign {
                        field: "supervisor_active".into(),
                        expr: Expr::Bool(false),
                    },
                    Update::Assign {
                        field: "coordinator_bound".into(),
                        expr: Expr::Bool(false),
                    },
                ],
                to: "Destroyed".into(),
                emit: vec![
                    EffectEmit {
                        variant: "DeactivateSupervisor".into(),
                        fields: IndexMap::new(),
                    },
                    EffectEmit {
                        variant: "EmitOrchestratorNotice".into(),
                        fields: IndexMap::new(),
                    },
                ],
            },
            // Phase C: ForceCancelMember — cancels in-flight turn
            TransitionSchema {
                name: "ForceCancelMember".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    variant: "ForceCancelMember".into(),
                    bindings: vec![],
                },
                guards: vec![Guard {
                    name: "coordinator_is_bound".into(),
                    expr: Expr::Eq(
                        Box::new(Expr::Field("coordinator_bound".into())),
                        Box::new(Expr::Bool(true)),
                    ),
                }],
                updates: vec![],
                to: "Running".into(),
                emit: vec![
                    EffectEmit {
                        variant: "MemberForceCancelled".into(),
                        fields: IndexMap::new(),
                    },
                    EffectEmit {
                        variant: "EmitOrchestratorNotice".into(),
                        fields: IndexMap::new(),
                    },
                ],
            },
        ],
        effect_dispositions: vec![
            disposition(
                "ActivateSupervisor",
                EffectDisposition::Routed {
                    consumer_machines: vec!["MobLifecycleMachine".into()],
                },
            ),
            disposition(
                "DeactivateSupervisor",
                EffectDisposition::Routed {
                    consumer_machines: vec!["MobLifecycleMachine".into()],
                },
            ),
            disposition(
                "FlowActivated",
                EffectDisposition::Routed {
                    consumer_machines: vec!["FlowRunMachine".into(), "MobLifecycleMachine".into()],
                },
            ),
            disposition(
                "FlowDeactivated",
                EffectDisposition::Routed {
                    consumer_machines: vec!["MobLifecycleMachine".into()],
                },
            ),
            disposition("EmitOrchestratorNotice", EffectDisposition::External),
            disposition(
                "MemberForceCancelled",
                EffectDisposition::Routed {
                    consumer_machines: vec!["RuntimeControlMachine".into()],
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

#[cfg(test)]
mod tests {
    use super::mob_orchestrator_machine;

    #[test]
    fn validates_mob_orchestrator_style_machine() {
        let schema = mob_orchestrator_machine();

        assert_eq!(schema.machine, "MobOrchestratorMachine");
        assert_eq!(schema.rust.crate_name, "meerkat-mob");
        assert_eq!(schema.rust.module, "generated::mob_orchestrator");
        assert!(
            schema
                .transitions
                .iter()
                .any(|transition| transition.name == "InitializeOrchestrator")
        );
        assert_eq!(schema.validate(), Ok(()));
    }
}
