use indexmap::IndexMap;

use crate::{
    EffectDisposition, EffectDispositionRule, EffectEmit, EnumSchema, Expr, FieldInit, FieldSchema,
    Guard, InitSchema, InputMatch, InvariantSchema, MachineSchema, RustBinding, StateSchema,
    TransitionSchema, TypeRef, Update, VariantSchema,
};

pub fn runtime_control_machine() -> MachineSchema {
    MachineSchema {
        machine: "RuntimeControlMachine".into(),
        version: 2,
        rust: RustBinding {
            crate_name: "meerkat-runtime".into(),
            module: "generated::runtime_control".into(),
        },
        state: StateSchema {
            phase: EnumSchema {
                name: "RuntimeState".into(),
                variants: vec![
                    variant("Initializing"),
                    variant("Idle"),
                    variant("Attached"),
                    variant("Running"),
                    variant("Recovering"),
                    variant("Retired"),
                    variant("Stopped"),
                    variant("Destroyed"),
                ],
            },
            fields: vec![
                field(
                    "current_run_id",
                    TypeRef::Option(Box::new(TypeRef::Named("RunId".into()))),
                ),
                field(
                    "pre_run_state",
                    TypeRef::Option(Box::new(TypeRef::Named("RuntimeState".into()))),
                ),
                field("wake_pending", TypeRef::Bool),
                field("process_pending", TypeRef::Bool),
            ],
            init: InitSchema {
                phase: "Initializing".into(),
                fields: vec![
                    init("current_run_id", Expr::None),
                    init("pre_run_state", Expr::None),
                    init("wake_pending", Expr::Bool(false)),
                    init("process_pending", Expr::Bool(false)),
                ],
            },
            terminal_phases: vec!["Stopped".into(), "Destroyed".into()],
        },
        inputs: EnumSchema {
            name: "RuntimeControlInput".into(),
            variants: vec![
                variant("Initialize"),
                VariantSchema {
                    name: "SubmitWork".into(),
                    fields: vec![
                        field("work_id", TypeRef::Named("WorkId".into())),
                        field("content_shape", TypeRef::Named("ContentShape".into())),
                        field("handling_mode", TypeRef::Named("HandlingMode".into())),
                        field(
                            "request_id",
                            TypeRef::Option(Box::new(TypeRef::Named("RequestId".into()))),
                        ),
                        field(
                            "reservation_key",
                            TypeRef::Option(Box::new(TypeRef::Named("ReservationKey".into()))),
                        ),
                    ],
                },
                VariantSchema {
                    name: "AdmissionAccepted".into(),
                    fields: vec![
                        field("work_id", TypeRef::Named("WorkId".into())),
                        field("content_shape", TypeRef::Named("ContentShape".into())),
                        field("handling_mode", TypeRef::Named("HandlingMode".into())),
                        field(
                            "request_id",
                            TypeRef::Option(Box::new(TypeRef::Named("RequestId".into()))),
                        ),
                        field(
                            "reservation_key",
                            TypeRef::Option(Box::new(TypeRef::Named("ReservationKey".into()))),
                        ),
                        field("admission_effect", TypeRef::Named("AdmissionEffect".into())),
                    ],
                },
                VariantSchema {
                    name: "AdmissionRejected".into(),
                    fields: vec![
                        field("work_id", TypeRef::Named("WorkId".into())),
                        field("reason", TypeRef::String),
                    ],
                },
                VariantSchema {
                    name: "AdmissionDeduplicated".into(),
                    fields: vec![
                        field("work_id", TypeRef::Named("WorkId".into())),
                        field("existing_work_id", TypeRef::Named("WorkId".into())),
                    ],
                },
                VariantSchema {
                    name: "BeginRun".into(),
                    fields: vec![field("run_id", TypeRef::Named("RunId".into()))],
                },
                VariantSchema {
                    name: "RunCompleted".into(),
                    fields: vec![field("run_id", TypeRef::Named("RunId".into()))],
                },
                VariantSchema {
                    name: "RunFailed".into(),
                    fields: vec![field("run_id", TypeRef::Named("RunId".into()))],
                },
                VariantSchema {
                    name: "RunCancelled".into(),
                    fields: vec![field("run_id", TypeRef::Named("RunId".into()))],
                },
                variant("AttachExecutor"),
                variant("DetachExecutor"),
                variant("RecoverRequested"),
                variant("RecoverySucceeded"),
                variant("RetireRequested"),
                variant("ResetRequested"),
                variant("StopRequested"),
                variant("DestroyRequested"),
                variant("ResumeRequested"),
                variant("ExternalToolDeltaReceived"),
                // Phase D: recycle preserves identity, resets driver
                variant("RecycleRequested"),
                variant("RecycleSucceeded"),
            ],
        },
        effects: EnumSchema {
            name: "RuntimeControlEffect".into(),
            variants: vec![
                VariantSchema {
                    name: "ResolveAdmission".into(),
                    fields: vec![field("work_id", TypeRef::Named("WorkId".into()))],
                },
                VariantSchema {
                    name: "SubmitAdmittedIngressEffect".into(),
                    fields: vec![
                        field("work_id", TypeRef::Named("WorkId".into())),
                        field("content_shape", TypeRef::Named("ContentShape".into())),
                        field("handling_mode", TypeRef::Named("HandlingMode".into())),
                        field(
                            "request_id",
                            TypeRef::Option(Box::new(TypeRef::Named("RequestId".into()))),
                        ),
                        field(
                            "reservation_key",
                            TypeRef::Option(Box::new(TypeRef::Named("ReservationKey".into()))),
                        ),
                        field("admission_effect", TypeRef::Named("AdmissionEffect".into())),
                    ],
                },
                VariantSchema {
                    name: "SubmitRunPrimitive".into(),
                    fields: vec![field("run_id", TypeRef::Named("RunId".into()))],
                },
                variant("SignalWake"),
                variant("SignalImmediateProcess"),
                VariantSchema {
                    name: "EmitRuntimeNotice".into(),
                    fields: vec![
                        field("kind", TypeRef::String),
                        field("detail", TypeRef::String),
                    ],
                },
                VariantSchema {
                    name: "ResolveCompletionAsTerminated".into(),
                    fields: vec![field("reason", TypeRef::String)],
                },
                VariantSchema {
                    name: "ApplyControlPlaneCommand".into(),
                    fields: vec![field("command", TypeRef::String)],
                },
                // Phase D: recycle — same identity, reset driver
                VariantSchema {
                    name: "InitiateRecycle".into(),
                    fields: vec![],
                },
            ],
        },
        helpers: vec![],
        derived: vec![],
        invariants: vec![
            InvariantSchema {
                name: "running_implies_active_run".into(),
                expr: Expr::Or(vec![
                    Expr::Neq(
                        Box::new(Expr::CurrentPhase),
                        Box::new(Expr::Phase("Running".into())),
                    ),
                    Expr::Neq(
                        Box::new(Expr::Field("current_run_id".into())),
                        Box::new(Expr::None),
                    ),
                ]),
            },
            InvariantSchema {
                name: "active_run_only_while_running_or_retired".into(),
                expr: Expr::Or(vec![
                    Expr::Eq(
                        Box::new(Expr::Field("current_run_id".into())),
                        Box::new(Expr::None),
                    ),
                    Expr::Eq(
                        Box::new(Expr::CurrentPhase),
                        Box::new(Expr::Phase("Running".into())),
                    ),
                    Expr::Eq(
                        Box::new(Expr::CurrentPhase),
                        Box::new(Expr::Phase("Retired".into())),
                    ),
                ]),
            },
        ],
        transitions: vec![
            TransitionSchema {
                name: "Initialize".into(),
                from: vec!["Initializing".into()],
                on: InputMatch {
                    variant: "Initialize".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![],
                to: "Idle".into(),
                emit: vec![],
            },
            // Attached: executor attachment/detachment
            TransitionSchema {
                name: "AttachFromIdle".into(),
                from: vec!["Idle".into()],
                on: InputMatch {
                    variant: "AttachExecutor".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![],
                to: "Attached".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "DetachToIdle".into(),
                from: vec!["Attached".into()],
                on: InputMatch {
                    variant: "DetachExecutor".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![],
                to: "Idle".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "BeginRunFromIdle".into(),
                from: vec!["Idle".into()],
                on: InputMatch {
                    variant: "BeginRun".into(),
                    bindings: vec!["run_id".into()],
                },
                guards: vec![Guard {
                    name: "no_active_run".into(),
                    expr: Expr::Eq(
                        Box::new(Expr::Field("current_run_id".into())),
                        Box::new(Expr::None),
                    ),
                }],
                updates: vec![
                    Update::Assign {
                        field: "current_run_id".into(),
                        expr: Expr::Some(Box::new(Expr::Binding("run_id".into()))),
                    },
                    Update::Assign {
                        field: "pre_run_state".into(),
                        expr: Expr::Some(Box::new(Expr::Phase("Idle".into()))),
                    },
                    Update::Assign {
                        field: "wake_pending".into(),
                        expr: Expr::Bool(false),
                    },
                    Update::Assign {
                        field: "process_pending".into(),
                        expr: Expr::Bool(false),
                    },
                ],
                to: "Running".into(),
                emit: vec![EffectEmit {
                    variant: "SubmitRunPrimitive".into(),
                    fields: IndexMap::from([("run_id".into(), Expr::Binding("run_id".into()))]),
                }],
            },
            TransitionSchema {
                name: "BeginRunFromRetired".into(),
                from: vec!["Retired".into()],
                on: InputMatch {
                    variant: "BeginRun".into(),
                    bindings: vec!["run_id".into()],
                },
                guards: vec![Guard {
                    name: "no_active_run".into(),
                    expr: Expr::Eq(
                        Box::new(Expr::Field("current_run_id".into())),
                        Box::new(Expr::None),
                    ),
                }],
                updates: vec![
                    Update::Assign {
                        field: "current_run_id".into(),
                        expr: Expr::Some(Box::new(Expr::Binding("run_id".into()))),
                    },
                    Update::Assign {
                        field: "pre_run_state".into(),
                        expr: Expr::Some(Box::new(Expr::Phase("Retired".into()))),
                    },
                    Update::Assign {
                        field: "wake_pending".into(),
                        expr: Expr::Bool(false),
                    },
                    Update::Assign {
                        field: "process_pending".into(),
                        expr: Expr::Bool(false),
                    },
                ],
                to: "Running".into(),
                emit: vec![EffectEmit {
                    variant: "SubmitRunPrimitive".into(),
                    fields: IndexMap::from([("run_id".into(), Expr::Binding("run_id".into()))]),
                }],
            },
            TransitionSchema {
                name: "BeginRunFromAttached".into(),
                from: vec!["Attached".into()],
                on: InputMatch {
                    variant: "BeginRun".into(),
                    bindings: vec!["run_id".into()],
                },
                guards: vec![Guard {
                    name: "no_active_run".into(),
                    expr: Expr::Eq(
                        Box::new(Expr::Field("current_run_id".into())),
                        Box::new(Expr::None),
                    ),
                }],
                updates: vec![
                    Update::Assign {
                        field: "current_run_id".into(),
                        expr: Expr::Some(Box::new(Expr::Binding("run_id".into()))),
                    },
                    Update::Assign {
                        field: "pre_run_state".into(),
                        expr: Expr::Some(Box::new(Expr::Phase("Attached".into()))),
                    },
                    Update::Assign {
                        field: "wake_pending".into(),
                        expr: Expr::Bool(false),
                    },
                    Update::Assign {
                        field: "process_pending".into(),
                        expr: Expr::Bool(false),
                    },
                ],
                to: "Running".into(),
                emit: vec![EffectEmit {
                    variant: "SubmitRunPrimitive".into(),
                    fields: IndexMap::from([("run_id".into(), Expr::Binding("run_id".into()))]),
                }],
            },
            TransitionSchema {
                name: "BeginRunFromRecovering".into(),
                from: vec!["Recovering".into()],
                on: InputMatch {
                    variant: "BeginRun".into(),
                    bindings: vec!["run_id".into()],
                },
                guards: vec![Guard {
                    name: "no_active_run".into(),
                    expr: Expr::Eq(
                        Box::new(Expr::Field("current_run_id".into())),
                        Box::new(Expr::None),
                    ),
                }],
                updates: vec![
                    Update::Assign {
                        field: "current_run_id".into(),
                        expr: Expr::Some(Box::new(Expr::Binding("run_id".into()))),
                    },
                    Update::Assign {
                        field: "pre_run_state".into(),
                        expr: Expr::Some(Box::new(Expr::Phase("Recovering".into()))),
                    },
                    Update::Assign {
                        field: "wake_pending".into(),
                        expr: Expr::Bool(false),
                    },
                    Update::Assign {
                        field: "process_pending".into(),
                        expr: Expr::Bool(false),
                    },
                ],
                to: "Running".into(),
                emit: vec![EffectEmit {
                    variant: "SubmitRunPrimitive".into(),
                    fields: IndexMap::from([("run_id".into(), Expr::Binding("run_id".into()))]),
                }],
            },
            // RunCompleted — split by pre_run_state guard
            // RunCompleted — split by pre_run_state
            run_terminal_transition("RunCompletedToIdle", "RunCompleted", "Idle"),
            run_terminal_transition("RunCompletedToAttached", "RunCompleted", "Attached"),
            run_terminal_transition("RunCompletedToRetired", "RunCompleted", "Retired"),
            // RunFailed — split by pre_run_state
            run_terminal_transition("RunFailedToIdle", "RunFailed", "Idle"),
            run_terminal_transition("RunFailedToAttached", "RunFailed", "Attached"),
            run_terminal_transition("RunFailedToRetired", "RunFailed", "Retired"),
            // RunCancelled — split by pre_run_state
            run_terminal_transition("RunCancelledToIdle", "RunCancelled", "Idle"),
            run_terminal_transition("RunCancelledToAttached", "RunCancelled", "Attached"),
            run_terminal_transition("RunCancelledToRetired", "RunCancelled", "Retired"),
            // Run-terminal from Retired: run was in-flight when RetireRequested fired
            retired_inflight_run_terminal("RunCompletedFromRetiredInFlight", "RunCompleted"),
            retired_inflight_run_terminal("RunFailedFromRetiredInFlight", "RunFailed"),
            retired_inflight_run_terminal("RunCancelledFromRetiredInFlight", "RunCancelled"),
            TransitionSchema {
                name: "RecoverRequestedFromIdle".into(),
                from: vec!["Idle".into()],
                on: InputMatch {
                    variant: "RecoverRequested".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![Update::Assign {
                    field: "pre_run_state".into(),
                    expr: Expr::Some(Box::new(Expr::Phase("Idle".into()))),
                }],
                to: "Recovering".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "RecoverRequestedFromRunning".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    variant: "RecoverRequested".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![
                    Update::Assign {
                        field: "current_run_id".into(),
                        expr: Expr::None,
                    },
                    Update::Assign {
                        field: "pre_run_state".into(),
                        expr: Expr::Some(Box::new(Expr::Phase("Running".into()))),
                    },
                ],
                to: "Recovering".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "RecoverRequestedFromAttached".into(),
                from: vec!["Attached".into()],
                on: InputMatch {
                    variant: "RecoverRequested".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![Update::Assign {
                    field: "pre_run_state".into(),
                    expr: Expr::Some(Box::new(Expr::Phase("Attached".into()))),
                }],
                to: "Recovering".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "RecoverySucceeded".into(),
                from: vec!["Recovering".into()],
                on: InputMatch {
                    variant: "RecoverySucceeded".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![
                    Update::Assign {
                        field: "current_run_id".into(),
                        expr: Expr::None,
                    },
                    Update::Assign {
                        field: "pre_run_state".into(),
                        expr: Expr::None,
                    },
                    Update::Assign {
                        field: "wake_pending".into(),
                        expr: Expr::Bool(false),
                    },
                    Update::Assign {
                        field: "process_pending".into(),
                        expr: Expr::Bool(false),
                    },
                ],
                to: "Idle".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "RetireRequestedFromIdle".into(),
                from: vec!["Idle".into()],
                on: InputMatch {
                    variant: "RetireRequested".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![],
                to: "Retired".into(),
                emit: vec![EffectEmit {
                    variant: "ApplyControlPlaneCommand".into(),
                    fields: IndexMap::from([("command".into(), Expr::String("Retire".into()))]),
                }],
            },
            TransitionSchema {
                name: "RetireRequestedFromRunning".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    variant: "RetireRequested".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![],
                to: "Retired".into(),
                emit: vec![EffectEmit {
                    variant: "ApplyControlPlaneCommand".into(),
                    fields: IndexMap::from([("command".into(), Expr::String("Retire".into()))]),
                }],
            },
            TransitionSchema {
                name: "RetireRequestedFromAttached".into(),
                from: vec!["Attached".into()],
                on: InputMatch {
                    variant: "RetireRequested".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![],
                to: "Retired".into(),
                emit: vec![EffectEmit {
                    variant: "ApplyControlPlaneCommand".into(),
                    fields: IndexMap::from([("command".into(), Expr::String("Retire".into()))]),
                }],
            },
            TransitionSchema {
                name: "ResetRequested".into(),
                from: vec![
                    "Initializing".into(),
                    "Idle".into(),
                    "Attached".into(),
                    "Recovering".into(),
                    "Retired".into(),
                ],
                on: InputMatch {
                    variant: "ResetRequested".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![
                    Update::Assign {
                        field: "current_run_id".into(),
                        expr: Expr::None,
                    },
                    Update::Assign {
                        field: "pre_run_state".into(),
                        expr: Expr::None,
                    },
                    Update::Assign {
                        field: "wake_pending".into(),
                        expr: Expr::Bool(false),
                    },
                    Update::Assign {
                        field: "process_pending".into(),
                        expr: Expr::Bool(false),
                    },
                ],
                to: "Idle".into(),
                emit: vec![
                    EffectEmit {
                        variant: "ApplyControlPlaneCommand".into(),
                        fields: IndexMap::from([("command".into(), Expr::String("Reset".into()))]),
                    },
                    EffectEmit {
                        variant: "ResolveCompletionAsTerminated".into(),
                        fields: IndexMap::from([("reason".into(), Expr::String("Reset".into()))]),
                    },
                ],
            },
            TransitionSchema {
                name: "StopRequested".into(),
                from: vec![
                    "Initializing".into(),
                    "Idle".into(),
                    "Attached".into(),
                    "Running".into(),
                    "Recovering".into(),
                    "Retired".into(),
                ],
                on: InputMatch {
                    variant: "StopRequested".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![
                    Update::Assign {
                        field: "current_run_id".into(),
                        expr: Expr::None,
                    },
                    Update::Assign {
                        field: "pre_run_state".into(),
                        expr: Expr::None,
                    },
                ],
                to: "Stopped".into(),
                emit: vec![
                    EffectEmit {
                        variant: "ApplyControlPlaneCommand".into(),
                        fields: IndexMap::from([("command".into(), Expr::String("Stop".into()))]),
                    },
                    EffectEmit {
                        variant: "ResolveCompletionAsTerminated".into(),
                        fields: IndexMap::from([("reason".into(), Expr::String("Stopped".into()))]),
                    },
                ],
            },
            TransitionSchema {
                name: "DestroyRequested".into(),
                from: vec![
                    "Initializing".into(),
                    "Idle".into(),
                    "Attached".into(),
                    "Running".into(),
                    "Recovering".into(),
                    "Retired".into(),
                    "Stopped".into(),
                ],
                on: InputMatch {
                    variant: "DestroyRequested".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![
                    Update::Assign {
                        field: "current_run_id".into(),
                        expr: Expr::None,
                    },
                    Update::Assign {
                        field: "pre_run_state".into(),
                        expr: Expr::None,
                    },
                ],
                to: "Destroyed".into(),
                emit: vec![
                    EffectEmit {
                        variant: "ApplyControlPlaneCommand".into(),
                        fields: IndexMap::from([(
                            "command".into(),
                            Expr::String("Destroy".into()),
                        )]),
                    },
                    EffectEmit {
                        variant: "ResolveCompletionAsTerminated".into(),
                        fields: IndexMap::from([(
                            "reason".into(),
                            Expr::String("Destroyed".into()),
                        )]),
                    },
                ],
            },
            TransitionSchema {
                name: "ResumeRequested".into(),
                from: vec!["Recovering".into()],
                on: InputMatch {
                    variant: "ResumeRequested".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![],
                to: "Idle".into(),
                emit: vec![],
            },
            TransitionSchema {
                name: "SubmitWorkFromIdle".into(),
                from: vec!["Idle".into()],
                on: InputMatch {
                    variant: "SubmitWork".into(),
                    bindings: vec![
                        "work_id".into(),
                        "content_shape".into(),
                        "handling_mode".into(),
                        "request_id".into(),
                        "reservation_key".into(),
                    ],
                },
                guards: vec![],
                updates: vec![],
                to: "Idle".into(),
                emit: vec![EffectEmit {
                    variant: "ResolveAdmission".into(),
                    fields: IndexMap::from([("work_id".into(), Expr::Binding("work_id".into()))]),
                }],
            },
            TransitionSchema {
                name: "SubmitWorkFromRunning".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    variant: "SubmitWork".into(),
                    bindings: vec![
                        "work_id".into(),
                        "content_shape".into(),
                        "handling_mode".into(),
                        "request_id".into(),
                        "reservation_key".into(),
                    ],
                },
                guards: vec![],
                updates: vec![],
                to: "Running".into(),
                emit: vec![EffectEmit {
                    variant: "ResolveAdmission".into(),
                    fields: IndexMap::from([("work_id".into(), Expr::Binding("work_id".into()))]),
                }],
            },
            TransitionSchema {
                name: "SubmitWorkFromAttached".into(),
                from: vec!["Attached".into()],
                on: InputMatch {
                    variant: "SubmitWork".into(),
                    bindings: vec![
                        "work_id".into(),
                        "content_shape".into(),
                        "handling_mode".into(),
                        "request_id".into(),
                        "reservation_key".into(),
                    ],
                },
                guards: vec![],
                updates: vec![],
                to: "Attached".into(),
                emit: vec![EffectEmit {
                    variant: "ResolveAdmission".into(),
                    fields: IndexMap::from([("work_id".into(), Expr::Binding("work_id".into()))]),
                }],
            },
            runtime_control_admission_transition("AdmissionAcceptedIdleQueue", "Idle", "Queue"),
            runtime_control_admission_transition("AdmissionAcceptedIdleSteer", "Idle", "Steer"),
            runtime_control_admission_transition(
                "AdmissionAcceptedRunningQueue",
                "Running",
                "Queue",
            ),
            runtime_control_admission_transition(
                "AdmissionAcceptedRunningSteer",
                "Running",
                "Steer",
            ),
            runtime_control_admission_transition(
                "AdmissionAcceptedAttachedQueue",
                "Attached",
                "Queue",
            ),
            runtime_control_admission_transition(
                "AdmissionAcceptedAttachedSteer",
                "Attached",
                "Steer",
            ),
            TransitionSchema {
                name: "AdmissionRejectedIdle".into(),
                from: vec!["Idle".into()],
                on: InputMatch {
                    variant: "AdmissionRejected".into(),
                    bindings: vec!["work_id".into(), "reason".into()],
                },
                guards: vec![],
                updates: vec![],
                to: "Idle".into(),
                emit: vec![EffectEmit {
                    variant: "EmitRuntimeNotice".into(),
                    fields: IndexMap::from([
                        ("kind".into(), Expr::String("AdmissionRejected".into())),
                        ("detail".into(), Expr::Binding("reason".into())),
                    ]),
                }],
            },
            TransitionSchema {
                name: "AdmissionRejectedRunning".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    variant: "AdmissionRejected".into(),
                    bindings: vec!["work_id".into(), "reason".into()],
                },
                guards: vec![],
                updates: vec![],
                to: "Running".into(),
                emit: vec![EffectEmit {
                    variant: "EmitRuntimeNotice".into(),
                    fields: IndexMap::from([
                        ("kind".into(), Expr::String("AdmissionRejected".into())),
                        ("detail".into(), Expr::Binding("reason".into())),
                    ]),
                }],
            },
            TransitionSchema {
                name: "AdmissionRejectedAttached".into(),
                from: vec!["Attached".into()],
                on: InputMatch {
                    variant: "AdmissionRejected".into(),
                    bindings: vec!["work_id".into(), "reason".into()],
                },
                guards: vec![],
                updates: vec![],
                to: "Attached".into(),
                emit: vec![EffectEmit {
                    variant: "EmitRuntimeNotice".into(),
                    fields: IndexMap::from([
                        ("kind".into(), Expr::String("AdmissionRejected".into())),
                        ("detail".into(), Expr::Binding("reason".into())),
                    ]),
                }],
            },
            TransitionSchema {
                name: "AdmissionDeduplicatedIdle".into(),
                from: vec!["Idle".into()],
                on: InputMatch {
                    variant: "AdmissionDeduplicated".into(),
                    bindings: vec!["work_id".into(), "existing_work_id".into()],
                },
                guards: vec![],
                updates: vec![],
                to: "Idle".into(),
                emit: vec![EffectEmit {
                    variant: "EmitRuntimeNotice".into(),
                    fields: IndexMap::from([
                        ("kind".into(), Expr::String("AdmissionDeduplicated".into())),
                        ("detail".into(), Expr::String("ExistingInputLinked".into())),
                    ]),
                }],
            },
            TransitionSchema {
                name: "AdmissionDeduplicatedRunning".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    variant: "AdmissionDeduplicated".into(),
                    bindings: vec!["work_id".into(), "existing_work_id".into()],
                },
                guards: vec![],
                updates: vec![],
                to: "Running".into(),
                emit: vec![EffectEmit {
                    variant: "EmitRuntimeNotice".into(),
                    fields: IndexMap::from([
                        ("kind".into(), Expr::String("AdmissionDeduplicated".into())),
                        ("detail".into(), Expr::String("ExistingInputLinked".into())),
                    ]),
                }],
            },
            TransitionSchema {
                name: "AdmissionDeduplicatedAttached".into(),
                from: vec!["Attached".into()],
                on: InputMatch {
                    variant: "AdmissionDeduplicated".into(),
                    bindings: vec!["work_id".into(), "existing_work_id".into()],
                },
                guards: vec![],
                updates: vec![],
                to: "Attached".into(),
                emit: vec![EffectEmit {
                    variant: "EmitRuntimeNotice".into(),
                    fields: IndexMap::from([
                        ("kind".into(), Expr::String("AdmissionDeduplicated".into())),
                        ("detail".into(), Expr::String("ExistingInputLinked".into())),
                    ]),
                }],
            },
            TransitionSchema {
                name: "ExternalToolDeltaReceivedIdle".into(),
                from: vec!["Idle".into()],
                on: InputMatch {
                    variant: "ExternalToolDeltaReceived".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![],
                to: "Idle".into(),
                emit: vec![EffectEmit {
                    variant: "EmitRuntimeNotice".into(),
                    fields: IndexMap::from([
                        ("kind".into(), Expr::String("ExternalToolDelta".into())),
                        ("detail".into(), Expr::String("Received".into())),
                    ]),
                }],
            },
            TransitionSchema {
                name: "ExternalToolDeltaReceivedRunning".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    variant: "ExternalToolDeltaReceived".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![],
                to: "Running".into(),
                emit: vec![EffectEmit {
                    variant: "EmitRuntimeNotice".into(),
                    fields: IndexMap::from([
                        ("kind".into(), Expr::String("ExternalToolDelta".into())),
                        ("detail".into(), Expr::String("Received".into())),
                    ]),
                }],
            },
            TransitionSchema {
                name: "ExternalToolDeltaReceivedRecovering".into(),
                from: vec!["Recovering".into()],
                on: InputMatch {
                    variant: "ExternalToolDeltaReceived".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![],
                to: "Recovering".into(),
                emit: vec![EffectEmit {
                    variant: "EmitRuntimeNotice".into(),
                    fields: IndexMap::from([
                        ("kind".into(), Expr::String("ExternalToolDelta".into())),
                        ("detail".into(), Expr::String("Received".into())),
                    ]),
                }],
            },
            TransitionSchema {
                name: "ExternalToolDeltaReceivedRetired".into(),
                from: vec!["Retired".into()],
                on: InputMatch {
                    variant: "ExternalToolDeltaReceived".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![],
                to: "Retired".into(),
                emit: vec![EffectEmit {
                    variant: "EmitRuntimeNotice".into(),
                    fields: IndexMap::from([
                        ("kind".into(), Expr::String("ExternalToolDelta".into())),
                        ("detail".into(), Expr::String("Received".into())),
                    ]),
                }],
            },
            TransitionSchema {
                name: "ExternalToolDeltaReceivedAttached".into(),
                from: vec!["Attached".into()],
                on: InputMatch {
                    variant: "ExternalToolDeltaReceived".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![],
                to: "Attached".into(),
                emit: vec![EffectEmit {
                    variant: "EmitRuntimeNotice".into(),
                    fields: IndexMap::from([
                        ("kind".into(), Expr::String("ExternalToolDelta".into())),
                        ("detail".into(), Expr::String("Received".into())),
                    ]),
                }],
            },
            // Phase D: Recycle — preserves member identity, resets driver
            TransitionSchema {
                name: "RecycleRequestedFromRetired".into(),
                from: vec!["Retired".into()],
                on: InputMatch {
                    variant: "RecycleRequested".into(),
                    bindings: vec![],
                },
                guards: vec![Guard {
                    name: "no_active_run".into(),
                    expr: Expr::Eq(
                        Box::new(Expr::Field("current_run_id".into())),
                        Box::new(Expr::None),
                    ),
                }],
                updates: vec![Update::Assign {
                    field: "pre_run_state".into(),
                    expr: Expr::Some(Box::new(Expr::Phase("Retired".into()))),
                }],
                to: "Recovering".into(),
                emit: vec![EffectEmit {
                    variant: "InitiateRecycle".into(),
                    fields: IndexMap::new(),
                }],
            },
            TransitionSchema {
                name: "RecycleRequestedFromIdle".into(),
                from: vec!["Idle".into()],
                on: InputMatch {
                    variant: "RecycleRequested".into(),
                    bindings: vec![],
                },
                guards: vec![Guard {
                    name: "no_active_run".into(),
                    expr: Expr::Eq(
                        Box::new(Expr::Field("current_run_id".into())),
                        Box::new(Expr::None),
                    ),
                }],
                updates: vec![Update::Assign {
                    field: "pre_run_state".into(),
                    expr: Expr::Some(Box::new(Expr::Phase("Idle".into()))),
                }],
                to: "Recovering".into(),
                emit: vec![EffectEmit {
                    variant: "InitiateRecycle".into(),
                    fields: IndexMap::new(),
                }],
            },
            TransitionSchema {
                name: "RecycleRequestedFromAttached".into(),
                from: vec!["Attached".into()],
                on: InputMatch {
                    variant: "RecycleRequested".into(),
                    bindings: vec![],
                },
                guards: vec![Guard {
                    name: "no_active_run".into(),
                    expr: Expr::Eq(
                        Box::new(Expr::Field("current_run_id".into())),
                        Box::new(Expr::None),
                    ),
                }],
                updates: vec![Update::Assign {
                    field: "pre_run_state".into(),
                    expr: Expr::Some(Box::new(Expr::Phase("Attached".into()))),
                }],
                to: "Recovering".into(),
                emit: vec![EffectEmit {
                    variant: "InitiateRecycle".into(),
                    fields: IndexMap::new(),
                }],
            },
            // Recycle success goes Recovering → Idle
            TransitionSchema {
                name: "RecycleSucceeded".into(),
                from: vec!["Recovering".into()],
                on: InputMatch {
                    variant: "RecycleSucceeded".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![
                    Update::Assign {
                        field: "current_run_id".into(),
                        expr: Expr::None,
                    },
                    Update::Assign {
                        field: "pre_run_state".into(),
                        expr: Expr::None,
                    },
                    Update::Assign {
                        field: "wake_pending".into(),
                        expr: Expr::Bool(false),
                    },
                    Update::Assign {
                        field: "process_pending".into(),
                        expr: Expr::Bool(false),
                    },
                ],
                to: "Idle".into(),
                emit: vec![EffectEmit {
                    variant: "EmitRuntimeNotice".into(),
                    fields: IndexMap::from([
                        ("kind".into(), Expr::String("Recycle".into())),
                        ("detail".into(), Expr::String("Succeeded".into())),
                    ]),
                }],
            },
        ],
        effect_dispositions: vec![
            disposition("ResolveAdmission", EffectDisposition::Local),
            disposition(
                "SubmitAdmittedIngressEffect",
                EffectDisposition::Routed {
                    consumer_machines: vec!["RuntimeIngressMachine".into()],
                },
            ),
            disposition(
                "SubmitRunPrimitive",
                EffectDisposition::Routed {
                    consumer_machines: vec!["TurnExecutionMachine".into()],
                },
            ),
            disposition("SignalWake", EffectDisposition::Local),
            disposition("SignalImmediateProcess", EffectDisposition::Local),
            disposition("EmitRuntimeNotice", EffectDisposition::External),
            disposition("ResolveCompletionAsTerminated", EffectDisposition::External),
            disposition("ApplyControlPlaneCommand", EffectDisposition::External),
            disposition("InitiateRecycle", EffectDisposition::Local),
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

fn submit_admitted_ingress_effect_fields() -> IndexMap<String, Expr> {
    IndexMap::from([
        ("work_id".into(), Expr::Binding("work_id".into())),
        (
            "content_shape".into(),
            Expr::Binding("content_shape".into()),
        ),
        (
            "handling_mode".into(),
            Expr::Binding("handling_mode".into()),
        ),
        ("request_id".into(), Expr::Binding("request_id".into())),
        (
            "reservation_key".into(),
            Expr::Binding("reservation_key".into()),
        ),
        (
            "admission_effect".into(),
            Expr::Binding("admission_effect".into()),
        ),
    ])
}

/// Helper for run terminal transitions (RunCompleted/RunFailed/RunCancelled)
/// split by pre_run_state guard.
fn run_terminal_transition(
    name: &str,
    input_variant: &str,
    target_phase: &str,
) -> TransitionSchema {
    let guard_expr = if target_phase == "Idle" {
        // Idle is the default — matches None, Some(Idle), or Some(Recovering)
        Expr::Or(vec![
            Expr::Eq(
                Box::new(Expr::Field("pre_run_state".into())),
                Box::new(Expr::None),
            ),
            Expr::Eq(
                Box::new(Expr::Field("pre_run_state".into())),
                Box::new(Expr::Some(Box::new(Expr::Phase("Idle".into())))),
            ),
            Expr::Eq(
                Box::new(Expr::Field("pre_run_state".into())),
                Box::new(Expr::Some(Box::new(Expr::Phase("Recovering".into())))),
            ),
        ])
    } else {
        Expr::Eq(
            Box::new(Expr::Field("pre_run_state".into())),
            Box::new(Expr::Some(Box::new(Expr::Phase(target_phase.into())))),
        )
    };

    TransitionSchema {
        name: name.into(),
        from: vec!["Running".into()],
        on: InputMatch {
            variant: input_variant.into(),
            bindings: vec!["run_id".into()],
        },
        guards: vec![
            Guard {
                name: "active_run_matches".into(),
                expr: Expr::Eq(
                    Box::new(Expr::Field("current_run_id".into())),
                    Box::new(Expr::Some(Box::new(Expr::Binding("run_id".into())))),
                ),
            },
            Guard {
                name: format!("pre_run_state_is_{}", target_phase.to_lowercase()),
                expr: guard_expr,
            },
        ],
        updates: vec![
            Update::Assign {
                field: "current_run_id".into(),
                expr: Expr::None,
            },
            Update::Assign {
                field: "pre_run_state".into(),
                expr: Expr::None,
            },
        ],
        to: target_phase.into(),
        emit: vec![],
    }
}

/// Run-terminal from Retired: run was in-flight when RetireRequestedFromRunning fired.
/// Phase is already Retired; run completes and clears its state.
fn retired_inflight_run_terminal(name: &str, input_variant: &str) -> TransitionSchema {
    TransitionSchema {
        name: name.into(),
        from: vec!["Retired".into()],
        on: InputMatch {
            variant: input_variant.into(),
            bindings: vec!["run_id".into()],
        },
        guards: vec![Guard {
            name: "active_run_matches".into(),
            expr: Expr::Eq(
                Box::new(Expr::Field("current_run_id".into())),
                Box::new(Expr::Some(Box::new(Expr::Binding("run_id".into())))),
            ),
        }],
        updates: vec![
            Update::Assign {
                field: "current_run_id".into(),
                expr: Expr::None,
            },
            Update::Assign {
                field: "pre_run_state".into(),
                expr: Expr::None,
            },
        ],
        to: "Retired".into(),
        emit: vec![],
    }
}

fn runtime_control_admission_transition(
    name: &str,
    from_phase: &str,
    handling_mode: &str,
) -> TransitionSchema {
    let wake_when_idle = from_phase == "Idle" || from_phase == "Attached";
    let process_immediately = handling_mode == "Steer";

    let mut emit = vec![EffectEmit {
        variant: "SubmitAdmittedIngressEffect".into(),
        fields: submit_admitted_ingress_effect_fields(),
    }];

    if wake_when_idle || process_immediately {
        emit.push(EffectEmit {
            variant: "SignalWake".into(),
            fields: IndexMap::new(),
        });
    }

    if process_immediately {
        emit.push(EffectEmit {
            variant: "SignalImmediateProcess".into(),
            fields: IndexMap::new(),
        });
    }

    TransitionSchema {
        name: name.into(),
        from: vec![from_phase.into()],
        on: InputMatch {
            variant: "AdmissionAccepted".into(),
            bindings: vec![
                "work_id".into(),
                "content_shape".into(),
                "handling_mode".into(),
                "request_id".into(),
                "reservation_key".into(),
                "admission_effect".into(),
            ],
        },
        guards: vec![Guard {
            name: format!("handling_mode_is_{}", handling_mode.to_lowercase()),
            expr: Expr::Eq(
                Box::new(Expr::Binding("handling_mode".into())),
                Box::new(Expr::String(handling_mode.into())),
            ),
        }],
        updates: vec![
            Update::Assign {
                field: "wake_pending".into(),
                expr: Expr::Bool(wake_when_idle || process_immediately),
            },
            Update::Assign {
                field: "process_pending".into(),
                expr: Expr::Bool(process_immediately),
            },
        ],
        to: from_phase.into(),
        emit,
    }
}
