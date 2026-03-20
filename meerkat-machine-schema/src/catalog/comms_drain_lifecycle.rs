use indexmap::IndexMap;

use crate::{
    EffectDisposition, EffectDispositionRule, EffectEmit, EnumSchema, Expr, FieldInit, FieldSchema,
    Guard, InitSchema, InputMatch, InvariantSchema, MachineSchema, RustBinding, StateSchema,
    TransitionSchema, TypeRef, Update, VariantSchema,
};

pub fn comms_drain_lifecycle_machine() -> MachineSchema {
    MachineSchema {
        machine: "CommsDrainLifecycleMachine".into(),
        version: 1,
        rust: RustBinding {
            crate_name: "meerkat-runtime".into(),
            module: "generated::comms_drain_lifecycle".into(),
        },
        state: StateSchema {
            phase: EnumSchema {
                name: "CommsDrainPhase".into(),
                variants: vec![
                    variant("Inactive"),
                    variant("Starting"),
                    variant("Running"),
                    variant("ExitedRespawnable"),
                    variant("Stopped"),
                ],
            },
            fields: vec![
                field(
                    "mode",
                    TypeRef::Option(Box::new(TypeRef::Enum("CommsDrainMode".into()))),
                ),
                field("suppresses_turn_boundary_drain", TypeRef::Bool),
            ],
            init: InitSchema {
                phase: "Inactive".into(),
                fields: vec![
                    init("mode", Expr::None),
                    init("suppresses_turn_boundary_drain", Expr::Bool(false)),
                ],
            },
            terminal_phases: vec!["Stopped".into()],
        },
        inputs: EnumSchema {
            name: "CommsDrainLifecycleInput".into(),
            variants: vec![
                VariantSchema {
                    name: "EnsureRunning".into(),
                    fields: vec![field("mode", TypeRef::Enum("CommsDrainMode".into()))],
                },
                variant("TaskSpawned"),
                VariantSchema {
                    name: "TaskExited".into(),
                    fields: vec![field("reason", TypeRef::Enum("DrainExitReason".into()))],
                },
                variant("StopRequested"),
                variant("AbortObserved"),
            ],
        },
        effects: EnumSchema {
            name: "CommsDrainLifecycleEffect".into(),
            variants: vec![
                VariantSchema {
                    name: "SpawnDrainTask".into(),
                    fields: vec![field("mode", TypeRef::Enum("CommsDrainMode".into()))],
                },
                VariantSchema {
                    name: "SetTurnBoundaryDrainSuppressed".into(),
                    fields: vec![field("active", TypeRef::Bool)],
                },
                variant("AbortDrainTask"),
            ],
        },
        helpers: vec![],
        derived: vec![],
        invariants: vec![
            // Running or Starting implies mode is set
            InvariantSchema {
                name: "active_implies_mode_set".into(),
                expr: Expr::Or(vec![
                    Expr::And(vec![
                        Expr::Neq(
                            Box::new(Expr::CurrentPhase),
                            Box::new(Expr::Phase("Starting".into())),
                        ),
                        Expr::Neq(
                            Box::new(Expr::CurrentPhase),
                            Box::new(Expr::Phase("Running".into())),
                        ),
                    ]),
                    Expr::Neq(Box::new(Expr::Field("mode".into())), Box::new(Expr::None)),
                ]),
            },
            // Running or Starting implies suppression is active
            InvariantSchema {
                name: "active_implies_suppression".into(),
                expr: Expr::Or(vec![
                    Expr::And(vec![
                        Expr::Neq(
                            Box::new(Expr::CurrentPhase),
                            Box::new(Expr::Phase("Starting".into())),
                        ),
                        Expr::Neq(
                            Box::new(Expr::CurrentPhase),
                            Box::new(Expr::Phase("Running".into())),
                        ),
                    ]),
                    Expr::Eq(
                        Box::new(Expr::Field("suppresses_turn_boundary_drain".into())),
                        Box::new(Expr::Bool(true)),
                    ),
                ]),
            },
        ],
        transitions: vec![
            // (Inactive, EnsureRunning{mode}) -> Starting
            TransitionSchema {
                name: "EnsureRunningFromInactive".into(),
                from: vec!["Inactive".into()],
                on: InputMatch {
                    variant: "EnsureRunning".into(),
                    bindings: vec!["mode".into()],
                },
                guards: vec![],
                updates: vec![
                    Update::Assign {
                        field: "mode".into(),
                        expr: Expr::Some(Box::new(Expr::Binding("mode".into()))),
                    },
                    Update::Assign {
                        field: "suppresses_turn_boundary_drain".into(),
                        expr: Expr::Bool(true),
                    },
                ],
                to: "Starting".into(),
                emit: vec![
                    EffectEmit {
                        variant: "SpawnDrainTask".into(),
                        fields: IndexMap::from([("mode".into(), Expr::Binding("mode".into()))]),
                    },
                    EffectEmit {
                        variant: "SetTurnBoundaryDrainSuppressed".into(),
                        fields: IndexMap::from([("active".into(), Expr::Bool(true))]),
                    },
                ],
            },
            // (Starting, TaskSpawned) -> Running
            TransitionSchema {
                name: "TaskSpawnedFromStarting".into(),
                from: vec!["Starting".into()],
                on: InputMatch {
                    variant: "TaskSpawned".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![],
                to: "Running".into(),
                emit: vec![],
            },
            // (Starting, TaskExited{reason=Failed}) when mode=PersistentHost -> ExitedRespawnable
            TransitionSchema {
                name: "TaskExitedFromStartingRespawnable".into(),
                from: vec!["Starting".into()],
                on: InputMatch {
                    variant: "TaskExited".into(),
                    bindings: vec!["reason".into()],
                },
                guards: vec![
                    Guard {
                        name: "reason_is_failed".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Binding("reason".into())),
                            Box::new(drain_exit_reason(DrainExitReasonVariant::Failed)),
                        ),
                    },
                    Guard {
                        name: "mode_is_persistent_host".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field("mode".into())),
                            Box::new(Expr::Some(Box::new(comms_drain_mode(
                                CommsDrainModeVariant::PersistentHost,
                            )))),
                        ),
                    },
                ],
                updates: vec![Update::Assign {
                    field: "suppresses_turn_boundary_drain".into(),
                    expr: Expr::Bool(false),
                }],
                to: "ExitedRespawnable".into(),
                emit: vec![EffectEmit {
                    variant: "SetTurnBoundaryDrainSuppressed".into(),
                    fields: IndexMap::from([("active".into(), Expr::Bool(false))]),
                }],
            },
            // (Starting, TaskExited{reason}) -> Stopped (non-respawnable cases)
            TransitionSchema {
                name: "TaskExitedFromStartingStopped".into(),
                from: vec!["Starting".into()],
                on: InputMatch {
                    variant: "TaskExited".into(),
                    bindings: vec!["reason".into()],
                },
                guards: vec![Guard {
                    name: "not_respawnable".into(),
                    expr: Expr::Or(vec![
                        // reason is not Failed
                        Expr::Neq(
                            Box::new(Expr::Binding("reason".into())),
                            Box::new(drain_exit_reason(DrainExitReasonVariant::Failed)),
                        ),
                        // OR mode is not PersistentHost
                        Expr::Neq(
                            Box::new(Expr::Field("mode".into())),
                            Box::new(Expr::Some(Box::new(comms_drain_mode(
                                CommsDrainModeVariant::PersistentHost,
                            )))),
                        ),
                    ]),
                }],
                updates: vec![Update::Assign {
                    field: "suppresses_turn_boundary_drain".into(),
                    expr: Expr::Bool(false),
                }],
                to: "Stopped".into(),
                emit: vec![EffectEmit {
                    variant: "SetTurnBoundaryDrainSuppressed".into(),
                    fields: IndexMap::from([("active".into(), Expr::Bool(false))]),
                }],
            },
            // (Running, TaskExited{reason=Failed}) when mode=PersistentHost -> ExitedRespawnable
            TransitionSchema {
                name: "TaskExitedFromRunningRespawnable".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    variant: "TaskExited".into(),
                    bindings: vec!["reason".into()],
                },
                guards: vec![
                    Guard {
                        name: "reason_is_failed".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Binding("reason".into())),
                            Box::new(drain_exit_reason(DrainExitReasonVariant::Failed)),
                        ),
                    },
                    Guard {
                        name: "mode_is_persistent_host".into(),
                        expr: Expr::Eq(
                            Box::new(Expr::Field("mode".into())),
                            Box::new(Expr::Some(Box::new(comms_drain_mode(
                                CommsDrainModeVariant::PersistentHost,
                            )))),
                        ),
                    },
                ],
                updates: vec![Update::Assign {
                    field: "suppresses_turn_boundary_drain".into(),
                    expr: Expr::Bool(false),
                }],
                to: "ExitedRespawnable".into(),
                emit: vec![EffectEmit {
                    variant: "SetTurnBoundaryDrainSuppressed".into(),
                    fields: IndexMap::from([("active".into(), Expr::Bool(false))]),
                }],
            },
            // (Running, TaskExited{reason}) -> Stopped (non-respawnable cases)
            TransitionSchema {
                name: "TaskExitedFromRunningStopped".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    variant: "TaskExited".into(),
                    bindings: vec!["reason".into()],
                },
                guards: vec![Guard {
                    name: "not_respawnable".into(),
                    expr: Expr::Or(vec![
                        Expr::Neq(
                            Box::new(Expr::Binding("reason".into())),
                            Box::new(drain_exit_reason(DrainExitReasonVariant::Failed)),
                        ),
                        Expr::Neq(
                            Box::new(Expr::Field("mode".into())),
                            Box::new(Expr::Some(Box::new(comms_drain_mode(
                                CommsDrainModeVariant::PersistentHost,
                            )))),
                        ),
                    ]),
                }],
                updates: vec![Update::Assign {
                    field: "suppresses_turn_boundary_drain".into(),
                    expr: Expr::Bool(false),
                }],
                to: "Stopped".into(),
                emit: vec![EffectEmit {
                    variant: "SetTurnBoundaryDrainSuppressed".into(),
                    fields: IndexMap::from([("active".into(), Expr::Bool(false))]),
                }],
            },
            // (Running, StopRequested) -> Stopped
            TransitionSchema {
                name: "StopRequestedFromRunning".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    variant: "StopRequested".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![Update::Assign {
                    field: "suppresses_turn_boundary_drain".into(),
                    expr: Expr::Bool(false),
                }],
                to: "Stopped".into(),
                emit: vec![
                    EffectEmit {
                        variant: "AbortDrainTask".into(),
                        fields: IndexMap::new(),
                    },
                    EffectEmit {
                        variant: "SetTurnBoundaryDrainSuppressed".into(),
                        fields: IndexMap::from([("active".into(), Expr::Bool(false))]),
                    },
                ],
            },
            // (Starting, StopRequested) -> Stopped
            TransitionSchema {
                name: "StopRequestedFromStarting".into(),
                from: vec!["Starting".into()],
                on: InputMatch {
                    variant: "StopRequested".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![Update::Assign {
                    field: "suppresses_turn_boundary_drain".into(),
                    expr: Expr::Bool(false),
                }],
                to: "Stopped".into(),
                emit: vec![
                    EffectEmit {
                        variant: "AbortDrainTask".into(),
                        fields: IndexMap::new(),
                    },
                    EffectEmit {
                        variant: "SetTurnBoundaryDrainSuppressed".into(),
                        fields: IndexMap::from([("active".into(), Expr::Bool(false))]),
                    },
                ],
            },
            // (ExitedRespawnable, EnsureRunning{mode}) -> Starting
            TransitionSchema {
                name: "EnsureRunningFromExitedRespawnable".into(),
                from: vec!["ExitedRespawnable".into()],
                on: InputMatch {
                    variant: "EnsureRunning".into(),
                    bindings: vec!["mode".into()],
                },
                guards: vec![],
                updates: vec![
                    Update::Assign {
                        field: "mode".into(),
                        expr: Expr::Some(Box::new(Expr::Binding("mode".into()))),
                    },
                    Update::Assign {
                        field: "suppresses_turn_boundary_drain".into(),
                        expr: Expr::Bool(true),
                    },
                ],
                to: "Starting".into(),
                emit: vec![
                    EffectEmit {
                        variant: "SpawnDrainTask".into(),
                        fields: IndexMap::from([("mode".into(), Expr::Binding("mode".into()))]),
                    },
                    EffectEmit {
                        variant: "SetTurnBoundaryDrainSuppressed".into(),
                        fields: IndexMap::from([("active".into(), Expr::Bool(true))]),
                    },
                ],
            },
            // (ExitedRespawnable, StopRequested) -> Stopped
            TransitionSchema {
                name: "StopRequestedFromExitedRespawnable".into(),
                from: vec!["ExitedRespawnable".into()],
                on: InputMatch {
                    variant: "StopRequested".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![],
                to: "Stopped".into(),
                emit: vec![],
            },
            // (Stopped, EnsureRunning{mode}) -> Starting
            TransitionSchema {
                name: "EnsureRunningFromStopped".into(),
                from: vec!["Stopped".into()],
                on: InputMatch {
                    variant: "EnsureRunning".into(),
                    bindings: vec!["mode".into()],
                },
                guards: vec![],
                updates: vec![
                    Update::Assign {
                        field: "mode".into(),
                        expr: Expr::Some(Box::new(Expr::Binding("mode".into()))),
                    },
                    Update::Assign {
                        field: "suppresses_turn_boundary_drain".into(),
                        expr: Expr::Bool(true),
                    },
                ],
                to: "Starting".into(),
                emit: vec![
                    EffectEmit {
                        variant: "SpawnDrainTask".into(),
                        fields: IndexMap::from([("mode".into(), Expr::Binding("mode".into()))]),
                    },
                    EffectEmit {
                        variant: "SetTurnBoundaryDrainSuppressed".into(),
                        fields: IndexMap::from([("active".into(), Expr::Bool(true))]),
                    },
                ],
            },
            // (Running|Starting, AbortObserved) -> Stopped
            TransitionSchema {
                name: "AbortObservedFromActive".into(),
                from: vec!["Running".into(), "Starting".into()],
                on: InputMatch {
                    variant: "AbortObserved".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![Update::Assign {
                    field: "suppresses_turn_boundary_drain".into(),
                    expr: Expr::Bool(false),
                }],
                to: "Stopped".into(),
                emit: vec![EffectEmit {
                    variant: "SetTurnBoundaryDrainSuppressed".into(),
                    fields: IndexMap::from([("active".into(), Expr::Bool(false))]),
                }],
            },
        ],
        effect_dispositions: vec![
            disposition("SpawnDrainTask", EffectDisposition::Local),
            disposition("SetTurnBoundaryDrainSuppressed", EffectDisposition::Local),
            disposition("AbortDrainTask", EffectDisposition::Local),
        ],
    }
}

fn disposition(name: &str, d: EffectDisposition) -> EffectDispositionRule {
    EffectDispositionRule {
        effect_variant: name.into(),
        disposition: d,
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

// ---------------------------------------------------------------------------
// Local enum variant helpers
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
enum CommsDrainModeVariant {
    PersistentHost,
    #[allow(dead_code)]
    Timed,
}

#[derive(Debug, Clone, Copy)]
enum DrainExitReasonVariant {
    Failed,
    #[allow(dead_code)]
    IdleTimeout,
    #[allow(dead_code)]
    Dismissed,
    #[allow(dead_code)]
    Aborted,
    #[allow(dead_code)]
    SessionShutdown,
}

fn comms_drain_mode(variant: CommsDrainModeVariant) -> Expr {
    Expr::NamedVariant {
        enum_name: "CommsDrainMode".into(),
        variant: match variant {
            CommsDrainModeVariant::PersistentHost => "PersistentHost".into(),
            CommsDrainModeVariant::Timed => "Timed".into(),
        },
    }
}

fn drain_exit_reason(variant: DrainExitReasonVariant) -> Expr {
    Expr::NamedVariant {
        enum_name: "DrainExitReason".into(),
        variant: match variant {
            DrainExitReasonVariant::IdleTimeout => "IdleTimeout".into(),
            DrainExitReasonVariant::Dismissed => "Dismissed".into(),
            DrainExitReasonVariant::Failed => "Failed".into(),
            DrainExitReasonVariant::Aborted => "Aborted".into(),
            DrainExitReasonVariant::SessionShutdown => "SessionShutdown".into(),
        },
    }
}
