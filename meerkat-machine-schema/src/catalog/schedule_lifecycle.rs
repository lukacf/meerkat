use indexmap::IndexMap;

use crate::{
    EffectDisposition, EffectDispositionRule, EffectEmit, EnumSchema, Expr, FieldInit, FieldSchema,
    Guard, InitSchema, InputMatch, InvariantSchema, MachineSchema, RustBinding, StateSchema,
    TransitionSchema, TriggerKind, TypeRef, Update, VariantSchema,
};

pub fn schedule_lifecycle_machine() -> MachineSchema {
    MachineSchema {
        machine: "ScheduleLifecycleMachine".into(),
        version: 1,
        rust: RustBinding {
            crate_name: "meerkat-schedule".into(),
            module: "generated::schedule_lifecycle".into(),
        },
        state: StateSchema {
            phase: EnumSchema {
                name: "ScheduleLifecycleState".into(),
                variants: vec![variant("Active"), variant("Paused"), variant("Deleted")],
            },
            fields: vec![
                field("revision", TypeRef::U64),
                field("trigger_key", TypeRef::String),
                field("target_binding_key", TypeRef::String),
                field("misfire_policy", TypeRef::Enum("MisfirePolicy".into())),
                field("overlap_policy", TypeRef::Enum("OverlapPolicy".into())),
                field(
                    "missing_target_policy",
                    TypeRef::Enum("MissingTargetPolicy".into()),
                ),
                field(
                    "planning_cursor_utc_ms",
                    TypeRef::Option(Box::new(TypeRef::U64)),
                ),
                field("next_occurrence_ordinal", TypeRef::U64),
            ],
            init: InitSchema {
                phase: "Active".into(),
                fields: vec![
                    init("revision", Expr::U64(1)),
                    init("trigger_key", Expr::String("trigger-0".into())),
                    init("target_binding_key", Expr::String("target-0".into())),
                    init("misfire_policy", misfire_policy(MisfirePolicyVariant::Skip)),
                    init(
                        "overlap_policy",
                        overlap_policy(OverlapPolicyVariant::SkipIfRunning),
                    ),
                    init(
                        "missing_target_policy",
                        missing_target_policy(MissingTargetPolicyVariant::MarkMisfired),
                    ),
                    init("planning_cursor_utc_ms", Expr::None),
                    init("next_occurrence_ordinal", Expr::U64(0)),
                ],
            },
            terminal_phases: vec!["Deleted".into()],
        },
        inputs: EnumSchema {
            name: "ScheduleLifecycleInput".into(),
            variants: vec![
                VariantSchema {
                    name: "Revise".into(),
                    fields: vec![
                        field("trigger_key", TypeRef::String),
                        field("target_binding_key", TypeRef::String),
                        field("misfire_policy", TypeRef::Enum("MisfirePolicy".into())),
                        field("overlap_policy", TypeRef::Enum("OverlapPolicy".into())),
                        field(
                            "missing_target_policy",
                            TypeRef::Enum("MissingTargetPolicy".into()),
                        ),
                    ],
                },
                VariantSchema {
                    name: "RecordPlanningWindow".into(),
                    fields: vec![
                        field("planning_cursor_utc_ms", TypeRef::U64),
                        field("next_occurrence_ordinal", TypeRef::U64),
                    ],
                },
                variant("Pause"),
                variant("Resume"),
                variant("Delete"),
            ],
        },
        signals: EnumSchema {
            name: "ScheduleLifecycleSignal".into(),
            variants: vec![],
        },
        effects: EnumSchema {
            name: "ScheduleLifecycleEffect".into(),
            variants: vec![
                VariantSchema {
                    name: "EmitScheduleNotice".into(),
                    fields: vec![
                        field("new_state", TypeRef::Named("ScheduleLifecycleState".into())),
                        field("revision", TypeRef::U64),
                    ],
                },
                VariantSchema {
                    name: "SupersedePendingOccurrences".into(),
                    fields: vec![field("superseding_revision", TypeRef::U64)],
                },
                VariantSchema {
                    name: "PlanningWindowRecorded".into(),
                    fields: vec![
                        field("planning_cursor_utc_ms", TypeRef::U64),
                        field("next_occurrence_ordinal", TypeRef::U64),
                    ],
                },
            ],
        },
        helpers: vec![],
        derived: vec![],
        invariants: vec![
            InvariantSchema {
                name: "revision_is_positive".into(),
                expr: Expr::Gt(
                    Box::new(Expr::Field("revision".into())),
                    Box::new(Expr::U64(0)),
                ),
            },
            InvariantSchema {
                name: "deleted_has_no_planning_cursor".into(),
                expr: Expr::Or(vec![
                    Expr::Neq(
                        Box::new(Expr::CurrentPhase),
                        Box::new(Expr::Phase("Deleted".into())),
                    ),
                    Expr::Eq(
                        Box::new(Expr::Field("planning_cursor_utc_ms".into())),
                        Box::new(Expr::None),
                    ),
                ]),
            },
            InvariantSchema {
                name: "planning_cursor_requires_occurrence_progress".into(),
                expr: Expr::Or(vec![
                    Expr::Eq(
                        Box::new(Expr::Field("planning_cursor_utc_ms".into())),
                        Box::new(Expr::None),
                    ),
                    Expr::Gt(
                        Box::new(Expr::Field("next_occurrence_ordinal".into())),
                        Box::new(Expr::U64(0)),
                    ),
                ]),
            },
        ],
        transitions: vec![
            revise_transition("ReviseActive", "Active"),
            revise_transition("RevisePaused", "Paused"),
            planning_window_transition("RecordPlanningWindowActive", "Active"),
            planning_window_transition("RecordPlanningWindowPaused", "Paused"),
            TransitionSchema {
                name: "PauseActive".into(),
                from: vec!["Active".into()],
                on: InputMatch {
                    kind: TriggerKind::Input,
                    variant: "Pause".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![],
                to: "Paused".into(),
                emit: vec![schedule_notice()],
            },
            TransitionSchema {
                name: "ResumePaused".into(),
                from: vec!["Paused".into()],
                on: InputMatch {
                    kind: TriggerKind::Input,
                    variant: "Resume".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: vec![],
                to: "Active".into(),
                emit: vec![schedule_notice()],
            },
            delete_transition("DeleteActive", "Active"),
            delete_transition("DeletePaused", "Paused"),
        ],
        effect_dispositions: vec![
            disposition("EmitScheduleNotice", EffectDisposition::External),
            disposition(
                "SupersedePendingOccurrences",
                EffectDisposition::Routed {
                    consumer_machines: vec!["OccurrenceLifecycleMachine".into()],
                },
            ),
            disposition("PlanningWindowRecorded", EffectDisposition::Local),
        ],
        ci_step_limit: None,
    }
}

fn revise_transition(name: &str, phase: &str) -> TransitionSchema {
    TransitionSchema {
        name: name.into(),
        from: vec![phase.into()],
        on: InputMatch {
            kind: TriggerKind::Input,
            variant: "Revise".into(),
            bindings: vec![
                "trigger_key".into(),
                "target_binding_key".into(),
                "misfire_policy".into(),
                "overlap_policy".into(),
                "missing_target_policy".into(),
            ],
        },
        guards: vec![],
        updates: vec![
            Update::Assign {
                field: "trigger_key".into(),
                expr: Expr::Binding("trigger_key".into()),
            },
            Update::Assign {
                field: "target_binding_key".into(),
                expr: Expr::Binding("target_binding_key".into()),
            },
            Update::Assign {
                field: "misfire_policy".into(),
                expr: Expr::Binding("misfire_policy".into()),
            },
            Update::Assign {
                field: "overlap_policy".into(),
                expr: Expr::Binding("overlap_policy".into()),
            },
            Update::Assign {
                field: "missing_target_policy".into(),
                expr: Expr::Binding("missing_target_policy".into()),
            },
            Update::Increment {
                field: "revision".into(),
                amount: 1,
            },
            Update::Assign {
                field: "planning_cursor_utc_ms".into(),
                expr: Expr::None,
            },
        ],
        to: phase.into(),
        emit: vec![
            schedule_notice(),
            EffectEmit {
                variant: "SupersedePendingOccurrences".into(),
                fields: IndexMap::from([(
                    "superseding_revision".into(),
                    Expr::Field("revision".into()),
                )]),
            },
        ],
    }
}

fn planning_window_transition(name: &str, phase: &str) -> TransitionSchema {
    TransitionSchema {
        name: name.into(),
        from: vec![phase.into()],
        on: InputMatch {
            kind: TriggerKind::Input,
            variant: "RecordPlanningWindow".into(),
            bindings: vec![
                "planning_cursor_utc_ms".into(),
                "next_occurrence_ordinal".into(),
            ],
        },
        guards: vec![Guard {
            name: "planning_window_advances_ordinal".into(),
            expr: Expr::Gt(
                Box::new(Expr::Binding("next_occurrence_ordinal".into())),
                Box::new(Expr::U64(0)),
            ),
        }],
        updates: vec![
            Update::Assign {
                field: "planning_cursor_utc_ms".into(),
                expr: Expr::Some(Box::new(Expr::Binding("planning_cursor_utc_ms".into()))),
            },
            Update::Assign {
                field: "next_occurrence_ordinal".into(),
                expr: Expr::Binding("next_occurrence_ordinal".into()),
            },
        ],
        to: phase.into(),
        emit: vec![
            schedule_notice(),
            EffectEmit {
                variant: "PlanningWindowRecorded".into(),
                fields: IndexMap::from([
                    (
                        "planning_cursor_utc_ms".into(),
                        Expr::Binding("planning_cursor_utc_ms".into()),
                    ),
                    (
                        "next_occurrence_ordinal".into(),
                        Expr::Binding("next_occurrence_ordinal".into()),
                    ),
                ]),
            },
        ],
    }
}

fn delete_transition(name: &str, phase: &str) -> TransitionSchema {
    TransitionSchema {
        name: name.into(),
        from: vec![phase.into()],
        on: InputMatch {
            kind: TriggerKind::Input,
            variant: "Delete".into(),
            bindings: vec![],
        },
        guards: vec![],
        updates: vec![
            Update::Increment {
                field: "revision".into(),
                amount: 1,
            },
            Update::Assign {
                field: "planning_cursor_utc_ms".into(),
                expr: Expr::None,
            },
        ],
        to: "Deleted".into(),
        emit: vec![
            schedule_notice(),
            EffectEmit {
                variant: "SupersedePendingOccurrences".into(),
                fields: IndexMap::from([(
                    "superseding_revision".into(),
                    Expr::Field("revision".into()),
                )]),
            },
        ],
    }
}

fn schedule_notice() -> EffectEmit {
    EffectEmit {
        variant: "EmitScheduleNotice".into(),
        fields: IndexMap::from([
            ("new_state".into(), Expr::CurrentPhase),
            ("revision".into(), Expr::Field("revision".into())),
        ]),
    }
}

fn disposition(name: &str, disposition: EffectDisposition) -> EffectDispositionRule {
    EffectDispositionRule {
        effect_variant: name.into(),
        disposition,
        handoff_protocol: None,
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

#[derive(Clone, Copy)]
enum MisfirePolicyVariant {
    Skip,
}

fn misfire_policy(variant: MisfirePolicyVariant) -> Expr {
    Expr::NamedVariant {
        enum_name: "MisfirePolicy".into(),
        variant: match variant {
            MisfirePolicyVariant::Skip => "Skip".into(),
        },
    }
}

#[derive(Clone, Copy)]
enum OverlapPolicyVariant {
    SkipIfRunning,
}

fn overlap_policy(variant: OverlapPolicyVariant) -> Expr {
    Expr::NamedVariant {
        enum_name: "OverlapPolicy".into(),
        variant: match variant {
            OverlapPolicyVariant::SkipIfRunning => "SkipIfRunning".into(),
        },
    }
}

#[derive(Clone, Copy)]
enum MissingTargetPolicyVariant {
    MarkMisfired,
}

fn missing_target_policy(variant: MissingTargetPolicyVariant) -> Expr {
    Expr::NamedVariant {
        enum_name: "MissingTargetPolicy".into(),
        variant: match variant {
            MissingTargetPolicyVariant::MarkMisfired => "MarkMisfired".into(),
        },
    }
}
