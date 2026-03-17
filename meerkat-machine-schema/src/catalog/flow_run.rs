use indexmap::IndexMap;

use crate::{
    EffectEmit, EnumSchema, Expr, FieldInit, FieldSchema, Guard, HelperSchema, InitSchema,
    InputMatch, InvariantSchema, MachineSchema, Quantifier, RustBinding, StateSchema,
    TransitionSchema, TypeRef, Update, VariantSchema,
};

pub fn flow_run_machine() -> MachineSchema {
    MachineSchema {
        machine: "FlowRunMachine".into(),
        version: 1,
        rust: RustBinding {
            crate_name: "meerkat-mob".into(),
            module: "machines::flow_run".into(),
        },
        state: StateSchema {
            phase: EnumSchema {
                name: "FlowRunStatus".into(),
                variants: vec![
                    variant("Absent"),
                    variant("Pending"),
                    variant("Running"),
                    variant("Completed"),
                    variant("Failed"),
                    variant("Canceled"),
                ],
            },
            fields: vec![
                field(
                    "tracked_steps",
                    TypeRef::Set(Box::new(TypeRef::Named("StepId".into()))),
                ),
                field(
                    "step_status",
                    TypeRef::Map(
                        Box::new(TypeRef::Named("StepId".into())),
                        Box::new(TypeRef::Named("StepRunStatus".into())),
                    ),
                ),
                field(
                    "output_recorded",
                    TypeRef::Map(
                        Box::new(TypeRef::Named("StepId".into())),
                        Box::new(TypeRef::Bool),
                    ),
                ),
                field("failure_count", TypeRef::U32),
            ],
            init: InitSchema {
                phase: "Absent".into(),
                fields: vec![
                    init("tracked_steps", Expr::EmptySet),
                    init("step_status", Expr::EmptyMap),
                    init("output_recorded", Expr::EmptyMap),
                    init("failure_count", Expr::U64(0)),
                ],
            },
            terminal_phases: vec!["Completed".into(), "Failed".into(), "Canceled".into()],
        },
        inputs: EnumSchema {
            name: "FlowRunInput".into(),
            variants: vec![
                VariantSchema {
                    name: "CreateRun".into(),
                    fields: vec![field(
                        "step_ids",
                        TypeRef::Seq(Box::new(TypeRef::Named("StepId".into()))),
                    )],
                },
                variant("StartRun"),
                VariantSchema {
                    name: "DispatchStep".into(),
                    fields: vec![field("step_id", TypeRef::Named("StepId".into()))],
                },
                VariantSchema {
                    name: "CompleteStep".into(),
                    fields: vec![field("step_id", TypeRef::Named("StepId".into()))],
                },
                VariantSchema {
                    name: "RecordStepOutput".into(),
                    fields: vec![field("step_id", TypeRef::Named("StepId".into()))],
                },
                VariantSchema {
                    name: "FailStep".into(),
                    fields: vec![field("step_id", TypeRef::Named("StepId".into()))],
                },
                VariantSchema {
                    name: "SkipStep".into(),
                    fields: vec![field("step_id", TypeRef::Named("StepId".into()))],
                },
                VariantSchema {
                    name: "CancelStep".into(),
                    fields: vec![field("step_id", TypeRef::Named("StepId".into()))],
                },
                variant("TerminalizeCompleted"),
                variant("TerminalizeFailed"),
                variant("TerminalizeCanceled"),
            ],
        },
        effects: EnumSchema {
            name: "FlowRunEffect".into(),
            variants: vec![
                VariantSchema {
                    name: "EmitFlowRunNotice".into(),
                    fields: vec![field("run_status", TypeRef::Named("FlowRunStatus".into()))],
                },
                VariantSchema {
                    name: "EmitStepNotice".into(),
                    fields: vec![
                        field("step_id", TypeRef::Named("StepId".into())),
                        field("step_status", TypeRef::Named("StepRunStatus".into())),
                    ],
                },
                VariantSchema {
                    name: "AppendFailureLedger".into(),
                    fields: vec![field("step_id", TypeRef::Named("StepId".into()))],
                },
                VariantSchema {
                    name: "PersistStepOutput".into(),
                    fields: vec![field("step_id", TypeRef::Named("StepId".into()))],
                },
                VariantSchema {
                    name: "AdmitStepWork".into(),
                    fields: vec![field("step_id", TypeRef::Named("StepId".into()))],
                },
                VariantSchema {
                    name: "FlowTerminalized".into(),
                    fields: vec![field("run_status", TypeRef::Named("FlowRunStatus".into()))],
                },
            ],
        },
        helpers: vec![
            helper(
                "RunIsTerminal",
                vec![],
                TypeRef::Bool,
                Expr::Or(vec![
                    Expr::Eq(
                        Box::new(Expr::CurrentPhase),
                        Box::new(Expr::Phase("Completed".into())),
                    ),
                    Expr::Eq(
                        Box::new(Expr::CurrentPhase),
                        Box::new(Expr::Phase("Failed".into())),
                    ),
                    Expr::Eq(
                        Box::new(Expr::CurrentPhase),
                        Box::new(Expr::Phase("Canceled".into())),
                    ),
                ]),
            ),
            helper(
                "StepIsTracked",
                vec![field("step_id", TypeRef::Named("StepId".into()))],
                TypeRef::Bool,
                Expr::Contains {
                    collection: Box::new(Expr::Field("tracked_steps".into())),
                    value: Box::new(Expr::Binding("step_id".into())),
                },
            ),
            helper(
                "StepStatusIs",
                vec![
                    field("step_id", TypeRef::Named("StepId".into())),
                    field("expected_status", TypeRef::String),
                ],
                TypeRef::Bool,
                Expr::Eq(
                    Box::new(Expr::MapGet {
                        map: Box::new(Expr::Field("step_status".into())),
                        key: Box::new(Expr::Binding("step_id".into())),
                    }),
                    Box::new(Expr::Binding("expected_status".into())),
                ),
            ),
            helper(
                "StepOutputRecordedIs",
                vec![
                    field("step_id", TypeRef::Named("StepId".into())),
                    field("expected", TypeRef::Bool),
                ],
                TypeRef::Bool,
                Expr::Eq(
                    Box::new(Expr::MapGet {
                        map: Box::new(Expr::Field("output_recorded".into())),
                        key: Box::new(Expr::Binding("step_id".into())),
                    }),
                    Box::new(Expr::Binding("expected".into())),
                ),
            ),
            helper(
                "AllTrackedStepsInAllowedStatuses",
                vec![field(
                    "allowed_statuses",
                    TypeRef::Seq(Box::new(TypeRef::String)),
                )],
                TypeRef::Bool,
                Expr::Quantified {
                    quantifier: Quantifier::All,
                    binding: "step_id".into(),
                    over: Box::new(Expr::Field("tracked_steps".into())),
                    body: Box::new(Expr::Contains {
                        collection: Box::new(Expr::Binding("allowed_statuses".into())),
                        value: Box::new(Expr::MapGet {
                            map: Box::new(Expr::Field("step_status".into())),
                            key: Box::new(Expr::Binding("step_id".into())),
                        }),
                    }),
                },
            ),
            helper(
                "NoTrackedStepInStatus",
                vec![field("status", TypeRef::String)],
                TypeRef::Bool,
                Expr::Quantified {
                    quantifier: Quantifier::All,
                    binding: "step_id".into(),
                    over: Box::new(Expr::Field("tracked_steps".into())),
                    body: Box::new(Expr::Neq(
                        Box::new(Expr::MapGet {
                            map: Box::new(Expr::Field("step_status".into())),
                            key: Box::new(Expr::Binding("step_id".into())),
                        }),
                        Box::new(Expr::Binding("status".into())),
                    )),
                },
            ),
            helper(
                "AnyTrackedStepInStatus",
                vec![field("status", TypeRef::String)],
                TypeRef::Bool,
                Expr::Quantified {
                    quantifier: Quantifier::Any,
                    binding: "step_id".into(),
                    over: Box::new(Expr::Field("tracked_steps".into())),
                    body: Box::new(Expr::Eq(
                        Box::new(Expr::MapGet {
                            map: Box::new(Expr::Field("step_status".into())),
                            key: Box::new(Expr::Binding("step_id".into())),
                        }),
                        Box::new(Expr::Binding("status".into())),
                    )),
                },
            ),
        ],
        derived: vec![],
        invariants: vec![
            InvariantSchema {
                name: "output_only_follows_completed_steps".into(),
                expr: Expr::Quantified {
                    quantifier: Quantifier::All,
                    binding: "step_id".into(),
                    over: Box::new(Expr::Field("tracked_steps".into())),
                    body: Box::new(Expr::Or(vec![
                        Expr::Not(Box::new(Expr::Call {
                            helper: "StepOutputRecordedIs".into(),
                            args: vec![Expr::Binding("step_id".into()), Expr::Bool(true)],
                        })),
                        Expr::Call {
                            helper: "StepStatusIs".into(),
                            args: vec![
                                Expr::Binding("step_id".into()),
                                Expr::String("Completed".into()),
                            ],
                        },
                    ])),
                },
            },
            InvariantSchema {
                name: "terminal_runs_have_no_dispatched_steps".into(),
                expr: Expr::Or(vec![
                    Expr::Not(Box::new(Expr::Call {
                        helper: "RunIsTerminal".into(),
                        args: vec![],
                    })),
                    Expr::Call {
                        helper: "NoTrackedStepInStatus".into(),
                        args: vec![Expr::String("Dispatched".into())],
                    },
                ]),
            },
            InvariantSchema {
                name: "completed_runs_contain_only_completed_or_skipped_steps".into(),
                expr: Expr::Or(vec![
                    Expr::Neq(
                        Box::new(Expr::CurrentPhase),
                        Box::new(Expr::Phase("Completed".into())),
                    ),
                    Expr::Call {
                        helper: "AllTrackedStepsInAllowedStatuses".into(),
                        args: vec![Expr::SeqLiteral(vec![
                            Expr::String("Completed".into()),
                            Expr::String("Skipped".into()),
                        ])],
                    },
                ]),
            },
            InvariantSchema {
                name: "failed_step_presence_requires_failure_count".into(),
                expr: Expr::Or(vec![
                    Expr::Not(Box::new(Expr::Call {
                        helper: "AnyTrackedStepInStatus".into(),
                        args: vec![Expr::String("Failed".into())],
                    })),
                    Expr::Gte(
                        Box::new(Expr::Field("failure_count".into())),
                        Box::new(Expr::U64(1)),
                    ),
                ]),
            },
            InvariantSchema {
                name: "failed_run_has_failed_step_or_recorded_failure".into(),
                expr: Expr::Or(vec![
                    Expr::Neq(
                        Box::new(Expr::CurrentPhase),
                        Box::new(Expr::Phase("Failed".into())),
                    ),
                    Expr::Or(vec![
                        Expr::Call {
                            helper: "AnyTrackedStepInStatus".into(),
                            args: vec![Expr::String("Failed".into())],
                        },
                        Expr::Gt(
                            Box::new(Expr::Field("failure_count".into())),
                            Box::new(Expr::U64(0)),
                        ),
                    ]),
                ]),
            },
        ],
        transitions: vec![
            TransitionSchema {
                name: "CreateRun".into(),
                from: vec!["Absent".into()],
                on: InputMatch {
                    variant: "CreateRun".into(),
                    bindings: vec!["step_ids".into()],
                },
                guards: vec![Guard {
                    name: "step_ids_are_non_empty".into(),
                    expr: Expr::Gt(
                        Box::new(Expr::Len(Box::new(Expr::Binding("step_ids".into())))),
                        Box::new(Expr::U64(0)),
                    ),
                }],
                updates: vec![
                    Update::Assign {
                        field: "tracked_steps".into(),
                        expr: Expr::EmptySet,
                    },
                    Update::Assign {
                        field: "step_status".into(),
                        expr: Expr::EmptyMap,
                    },
                    Update::Assign {
                        field: "output_recorded".into(),
                        expr: Expr::EmptyMap,
                    },
                    Update::Assign {
                        field: "failure_count".into(),
                        expr: Expr::U64(0),
                    },
                    Update::ForEach {
                        binding: "step_id".into(),
                        over: Expr::Binding("step_ids".into()),
                        updates: vec![
                            Update::SetInsert {
                                field: "tracked_steps".into(),
                                value: Expr::Binding("step_id".into()),
                            },
                            Update::MapInsert {
                                field: "step_status".into(),
                                key: Expr::Binding("step_id".into()),
                                value: Expr::String("None".into()),
                            },
                            Update::MapInsert {
                                field: "output_recorded".into(),
                                key: Expr::Binding("step_id".into()),
                                value: Expr::Bool(false),
                            },
                        ],
                    },
                ],
                to: "Pending".into(),
                emit: vec![flow_run_notice("Pending")],
            },
            transition(
                "StartRun",
                &["Pending"],
                "StartRun",
                vec![],
                vec![flow_run_notice("Running")],
                "Running",
            ),
            step_transition(
                "DispatchStep",
                "DispatchStep",
                vec![
                    tracked_step_guard("step_id"),
                    step_status_guard("step_id", "item_is_not_yet_dispatched", "None"),
                ],
                vec![Update::MapInsert {
                    field: "step_status".into(),
                    key: Expr::Binding("step_id".into()),
                    value: Expr::String("Dispatched".into()),
                }],
                vec![
                    step_notice("step_id", "Dispatched"),
                    effect_with_step("AdmitStepWork", "step_id"),
                ],
            ),
            step_transition(
                "CompleteStep",
                "CompleteStep",
                vec![
                    tracked_step_guard("step_id"),
                    step_status_guard("step_id", "step_is_dispatched", "Dispatched"),
                ],
                vec![Update::MapInsert {
                    field: "step_status".into(),
                    key: Expr::Binding("step_id".into()),
                    value: Expr::String("Completed".into()),
                }],
                vec![step_notice("step_id", "Completed")],
            ),
            step_transition(
                "RecordStepOutput",
                "RecordStepOutput",
                vec![
                    tracked_step_guard("step_id"),
                    step_status_guard("step_id", "step_is_completed", "Completed"),
                    Guard {
                        name: "output_not_yet_recorded".into(),
                        expr: Expr::Call {
                            helper: "StepOutputRecordedIs".into(),
                            args: vec![Expr::Binding("step_id".into()), Expr::Bool(false)],
                        },
                    },
                ],
                vec![Update::MapInsert {
                    field: "output_recorded".into(),
                    key: Expr::Binding("step_id".into()),
                    value: Expr::Bool(true),
                }],
                vec![effect_with_step("PersistStepOutput", "step_id")],
            ),
            step_transition(
                "FailStep",
                "FailStep",
                vec![
                    tracked_step_guard("step_id"),
                    step_status_guard("step_id", "step_is_dispatched", "Dispatched"),
                ],
                vec![
                    Update::MapInsert {
                        field: "step_status".into(),
                        key: Expr::Binding("step_id".into()),
                        value: Expr::String("Failed".into()),
                    },
                    Update::Increment {
                        field: "failure_count".into(),
                        amount: 1,
                    },
                ],
                vec![
                    step_notice("step_id", "Failed"),
                    effect_with_step("AppendFailureLedger", "step_id"),
                ],
            ),
            step_transition(
                "SkipStep",
                "SkipStep",
                vec![
                    tracked_step_guard("step_id"),
                    step_status_guard("step_id", "step_is_not_started", "None"),
                ],
                vec![Update::MapInsert {
                    field: "step_status".into(),
                    key: Expr::Binding("step_id".into()),
                    value: Expr::String("Skipped".into()),
                }],
                vec![step_notice("step_id", "Skipped")],
            ),
            step_transition(
                "CancelStep",
                "CancelStep",
                vec![
                    tracked_step_guard("step_id"),
                    Guard {
                        name: "step_is_cancelable".into(),
                        expr: Expr::Or(vec![
                            Expr::Call {
                                helper: "StepStatusIs".into(),
                                args: vec![
                                    Expr::Binding("step_id".into()),
                                    Expr::String("None".into()),
                                ],
                            },
                            Expr::Call {
                                helper: "StepStatusIs".into(),
                                args: vec![
                                    Expr::Binding("step_id".into()),
                                    Expr::String("Dispatched".into()),
                                ],
                            },
                        ]),
                    },
                ],
                vec![Update::MapInsert {
                    field: "step_status".into(),
                    key: Expr::Binding("step_id".into()),
                    value: Expr::String("Canceled".into()),
                }],
                vec![step_notice("step_id", "Canceled")],
            ),
            TransitionSchema {
                name: "TerminalizeCompleted".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    variant: "TerminalizeCompleted".into(),
                    bindings: vec![],
                },
                guards: vec![Guard {
                    name: "all_steps_are_completed_or_skipped".into(),
                    expr: Expr::Call {
                        helper: "AllTrackedStepsInAllowedStatuses".into(),
                        args: vec![Expr::SeqLiteral(vec![
                            Expr::String("Completed".into()),
                            Expr::String("Skipped".into()),
                        ])],
                    },
                }],
                updates: vec![],
                to: "Completed".into(),
                emit: vec![
                    flow_run_notice("Completed"),
                    EffectEmit {
                        variant: "FlowTerminalized".into(),
                        fields: IndexMap::from([(
                            "run_status".into(),
                            Expr::String("Completed".into()),
                        )]),
                    },
                ],
            },
            TransitionSchema {
                name: "TerminalizeFailed".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    variant: "TerminalizeFailed".into(),
                    bindings: vec![],
                },
                guards: vec![
                    Guard {
                        name: "no_step_remains_dispatched".into(),
                        expr: Expr::Call {
                            helper: "NoTrackedStepInStatus".into(),
                            args: vec![Expr::String("Dispatched".into())],
                        },
                    },
                    Guard {
                        name: "at_least_one_step_failed".into(),
                        expr: Expr::Call {
                            helper: "AnyTrackedStepInStatus".into(),
                            args: vec![Expr::String("Failed".into())],
                        },
                    },
                ],
                updates: vec![],
                to: "Failed".into(),
                emit: vec![
                    flow_run_notice("Failed"),
                    EffectEmit {
                        variant: "FlowTerminalized".into(),
                        fields: IndexMap::from([(
                            "run_status".into(),
                            Expr::String("Failed".into()),
                        )]),
                    },
                ],
            },
            TransitionSchema {
                name: "TerminalizeCanceled".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    variant: "TerminalizeCanceled".into(),
                    bindings: vec![],
                },
                guards: vec![
                    Guard {
                        name: "no_step_remains_dispatched".into(),
                        expr: Expr::Call {
                            helper: "NoTrackedStepInStatus".into(),
                            args: vec![Expr::String("Dispatched".into())],
                        },
                    },
                    Guard {
                        name: "at_least_one_step_canceled".into(),
                        expr: Expr::Call {
                            helper: "AnyTrackedStepInStatus".into(),
                            args: vec![Expr::String("Canceled".into())],
                        },
                    },
                ],
                updates: vec![],
                to: "Canceled".into(),
                emit: vec![
                    flow_run_notice("Canceled"),
                    EffectEmit {
                        variant: "FlowTerminalized".into(),
                        fields: IndexMap::from([(
                            "run_status".into(),
                            Expr::String("Canceled".into()),
                        )]),
                    },
                ],
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

fn init(field: &str, expr: Expr) -> FieldInit {
    FieldInit {
        field: field.into(),
        expr,
    }
}

fn helper(name: &str, params: Vec<FieldSchema>, returns: TypeRef, body: Expr) -> HelperSchema {
    HelperSchema {
        name: name.into(),
        params,
        returns,
        body,
    }
}

fn transition(
    name: &str,
    from: &[&str],
    input_variant: &str,
    guards: Vec<Guard>,
    emit: Vec<EffectEmit>,
    to: &str,
) -> TransitionSchema {
    TransitionSchema {
        name: name.into(),
        from: from.iter().map(|phase| (*phase).to_owned()).collect(),
        on: InputMatch {
            variant: input_variant.into(),
            bindings: vec![],
        },
        guards,
        updates: vec![],
        to: to.into(),
        emit,
    }
}

fn step_transition(
    name: &str,
    input_variant: &str,
    guards: Vec<Guard>,
    updates: Vec<Update>,
    emit: Vec<EffectEmit>,
) -> TransitionSchema {
    TransitionSchema {
        name: name.into(),
        from: vec!["Running".into()],
        on: InputMatch {
            variant: input_variant.into(),
            bindings: vec!["step_id".into()],
        },
        guards,
        updates,
        to: "Running".into(),
        emit,
    }
}

fn tracked_step_guard(binding: &str) -> Guard {
    Guard {
        name: "step_is_tracked".into(),
        expr: Expr::Call {
            helper: "StepIsTracked".into(),
            args: vec![Expr::Binding(binding.into())],
        },
    }
}

fn step_status_guard(binding: &str, name: &str, status: &str) -> Guard {
    Guard {
        name: name.into(),
        expr: Expr::Call {
            helper: "StepStatusIs".into(),
            args: vec![Expr::Binding(binding.into()), Expr::String(status.into())],
        },
    }
}

fn flow_run_notice(run_status: &str) -> EffectEmit {
    EffectEmit {
        variant: "EmitFlowRunNotice".into(),
        fields: IndexMap::from([("run_status".into(), Expr::String(run_status.into()))]),
    }
}

fn step_notice(step_binding: &str, step_status: &str) -> EffectEmit {
    EffectEmit {
        variant: "EmitStepNotice".into(),
        fields: IndexMap::from([
            ("step_id".into(), Expr::Binding(step_binding.into())),
            ("step_status".into(), Expr::String(step_status.into())),
        ]),
    }
}

fn effect_with_step(variant: &str, step_binding: &str) -> EffectEmit {
    EffectEmit {
        variant: variant.into(),
        fields: IndexMap::from([("step_id".into(), Expr::Binding(step_binding.into()))]),
    }
}

#[cfg(test)]
mod tests {
    use super::flow_run_machine;

    #[test]
    fn validates_flow_run_machine_definition() {
        let schema = flow_run_machine();
        assert_eq!(schema.validate(), Ok(()));
    }
}
