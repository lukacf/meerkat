use indexmap::IndexMap;

use crate::{
    EffectDisposition, EffectDispositionRule, EffectEmit, EnumSchema, Expr, FieldSchema, Guard,
    InitSchema, InputMatch, MachineSchema, RustBinding, StateSchema, TransitionSchema, TypeRef,
    Update, VariantSchema, machine::TriggerKind,
};

/// Canonical MobMachine schema. Delegates to the DSL-generated version.
///
/// The hand-written builder below is retained only for historical reference
/// and for any downstream callers that may rely on the `mob_machine()` name.
pub fn mob_machine() -> MachineSchema {
    super::dsl::dsl_mob_machine()
}

#[allow(dead_code)]
fn mob_machine_hand_written() -> MachineSchema {
    let mut trigger_variants = direct_mob_trigger_variants();
    trigger_variants.extend(absorbed_mob_input_variants());
    let (input_variants, signal_variants) = split_mob_variants(trigger_variants);

    MachineSchema {
        machine: "MobMachine".into(),
        version: 1,
        rust: RustBinding {
            crate_name: "meerkat-mob".into(),
            module: "generated::mob_machine".into(),
        },
        state: StateSchema {
            phase: EnumSchema {
                name: "MobPhase".into(),
                variants: vec![
                    variant("Running"),
                    variant("Stopped"),
                    variant("Completed"),
                    variant("Destroyed"),
                ],
            },
            fields: vec![
                field(
                    "live_runtime_ids",
                    TypeRef::Set(Box::new(TypeRef::Named("AgentRuntimeId".into()))),
                ),
                field(
                    "externally_addressable_runtime_ids",
                    TypeRef::Set(Box::new(TypeRef::Named("AgentRuntimeId".into()))),
                ),
                field(
                    "runtime_fence_tokens",
                    TypeRef::Map(
                        Box::new(TypeRef::Named("AgentRuntimeId".into())),
                        Box::new(TypeRef::Named("FenceToken".into())),
                    ),
                ),
                field("active_run_count", TypeRef::U32),
                field("pending_spawn_count", TypeRef::U32),
                field("coordinator_bound", TypeRef::Bool),
            ],
            init: InitSchema {
                phase: "Running".into(),
                fields: vec![
                    init("live_runtime_ids", Expr::EmptySet),
                    init("externally_addressable_runtime_ids", Expr::EmptySet),
                    init("runtime_fence_tokens", Expr::EmptyMap),
                    init("active_run_count", Expr::U64(0)),
                    init("pending_spawn_count", Expr::U64(0)),
                    init("coordinator_bound", Expr::Bool(true)),
                ],
            },
            terminal_phases: vec!["Destroyed".into()],
        },
        inputs: EnumSchema {
            name: "MobMachineInput".into(),
            variants: input_variants,
        },
        surface_only_inputs: vec![
            "FlowStatus".into(),
            "TaskList".into(),
            "TaskGet".into(),
            "McpServerStates".into(),
            "RosterSnapshot".into(),
            "ListMembers".into(),
            "ListMembersIncludingRetiring".into(),
            "ListAllMembers".into(),
            "MemberStatus".into(),
            "CancelWork".into(),
            "PollEvents".into(),
            "ReplayAllEvents".into(),
            "GetMember".into(),
        ],
        signals: EnumSchema {
            name: "MobMachineSignal".into(),
            variants: signal_variants,
        },
        effects: EnumSchema {
            name: "MobMachineEffect".into(),
            variants: {
                let mut variants = vec![
                    VariantSchema {
                        name: "RequestRuntimeBinding".into(),
                        fields: runtime_binding_request_fields(),
                    },
                    VariantSchema {
                        name: "RequestRuntimeIngress".into(),
                        fields: work_submission_fields(),
                    },
                    VariantSchema {
                        name: "RequestRuntimeRetire".into(),
                        fields: vec![],
                    },
                    VariantSchema {
                        name: "RequestRuntimeDestroy".into(),
                        fields: vec![],
                    },
                    VariantSchema {
                        name: "EmitMemberLifecycleNotice".into(),
                        fields: vec![field("kind", TypeRef::String)],
                    },
                ];
                variants.extend(absorbed_mob_effect_variants());
                variants
            },
        },
        helpers: vec![],
        derived: vec![],
        invariants: vec![],
        transitions: vec![
            // Spawn is a Running self-loop: the real runtime starts in Running
            // and does not expose a durable pre-start top-level phase.
            TransitionSchema {
                name: "SpawnRunning".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    kind: mob_trigger_kind("Spawn"),
                    variant: "Spawn".into(),
                    bindings: vec![
                        "agent_identity".into(),
                        "agent_runtime_id".into(),
                        "fence_token".into(),
                        "generation".into(),
                        "external_addressable".into(),
                    ],
                },
                guards: vec![coordinator_bound_guard()],
                updates: {
                    let mut updates = reset_member_runtime_updates();
                    updates.extend(vec![
                        Update::SetInsert {
                            field: "live_runtime_ids".into(),
                            value: Expr::Binding("agent_runtime_id".into()),
                        },
                        Update::Conditional {
                            condition: Expr::Binding("external_addressable".into()),
                            then_updates: vec![Update::SetInsert {
                                field: "externally_addressable_runtime_ids".into(),
                                value: Expr::Binding("agent_runtime_id".into()),
                            }],
                            else_updates: vec![Update::SetRemove {
                                field: "externally_addressable_runtime_ids".into(),
                                value: Expr::Binding("agent_runtime_id".into()),
                            }],
                        },
                        Update::MapInsert {
                            field: "runtime_fence_tokens".into(),
                            key: Expr::Binding("agent_runtime_id".into()),
                            value: Expr::Binding("fence_token".into()),
                        },
                    ]);
                    updates
                },
                to: "Running".into(),
                emit: vec![
                    runtime_binding_emit("RequestRuntimeBinding"),
                    lifecycle_notice_emit("spawned"),
                ],
            },
            TransitionSchema {
                name: "ObserveRuntimeReady".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    kind: mob_trigger_kind("ObserveRuntimeReady"),
                    variant: "ObserveRuntimeReady".into(),
                    bindings: vec!["agent_runtime_id".into(), "fence_token".into()],
                },
                guards: vec![],
                updates: vec![],
                to: "Running".into(),
                emit: vec![],
            },
            // SubmitWork is a Running self-loop in the formal model. Fresh and
            // resumed mobs normalize directly to Running at runtime as well.
            TransitionSchema {
                name: "SubmitWorkRunningExternal".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    kind: mob_trigger_kind("SubmitWork"),
                    variant: "SubmitWork".into(),
                    bindings: vec![
                        "agent_runtime_id".into(),
                        "fence_token".into(),
                        "work_id".into(),
                        "origin".into(),
                    ],
                },
                guards: vec![
                    active_members_present_guard(),
                    current_binding_matches_guard(),
                    external_origin_guard(),
                    runtime_externally_addressable_guard(),
                ],
                updates: vec![],
                to: "Running".into(),
                emit: vec![work_submission_emit("RequestRuntimeIngress")],
            },
            TransitionSchema {
                name: "SubmitWorkRunningInternal".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    kind: mob_trigger_kind("SubmitWork"),
                    variant: "SubmitWork".into(),
                    bindings: vec![
                        "agent_runtime_id".into(),
                        "fence_token".into(),
                        "work_id".into(),
                        "origin".into(),
                    ],
                },
                guards: vec![
                    active_members_present_guard(),
                    current_binding_matches_guard(),
                    internal_origin_guard(),
                ],
                updates: vec![],
                to: "Running".into(),
                // SubmitWork formally requests runtime ingress on the bound
                // member runtime. The WorkRef -> InputId translation still
                // happens below the machine boundary.
                emit: vec![work_submission_emit("RequestRuntimeIngress")],
            },
            TransitionSchema {
                name: "RetireMember".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    kind: mob_trigger_kind("RetireMember"),
                    variant: "RetireMember".into(),
                    bindings: vec!["agent_runtime_id".into(), "fence_token".into()],
                },
                guards: vec![current_binding_matches_guard()],
                updates: vec![],
                to: "Running".into(),
                emit: vec![runtime_observation_emit("RequestRuntimeRetire")],
            },
            TransitionSchema {
                name: "ObserveRuntimeRetired".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    kind: mob_trigger_kind("ObserveRuntimeRetired"),
                    variant: "ObserveRuntimeRetired".into(),
                    bindings: vec!["agent_runtime_id".into(), "fence_token".into()],
                },
                guards: vec![current_binding_matches_guard()],
                updates: clear_current_runtime_projection_updates(),
                to: "Stopped".into(),
                emit: vec![lifecycle_notice_emit("retired")],
            },
            TransitionSchema {
                name: "ResetMember".into(),
                from: vec!["Running".into(), "Stopped".into()],
                on: InputMatch {
                    kind: mob_trigger_kind("ResetMember"),
                    variant: "ResetMember".into(),
                    bindings: vec![
                        "agent_identity".into(),
                        "agent_runtime_id".into(),
                        "fence_token".into(),
                        "generation".into(),
                        "external_addressable".into(),
                    ],
                },
                guards: vec![],
                updates: {
                    let mut updates = reset_member_runtime_updates();
                    updates.extend(vec![
                        Update::SetInsert {
                            field: "live_runtime_ids".into(),
                            value: Expr::Binding("agent_runtime_id".into()),
                        },
                        Update::Conditional {
                            condition: Expr::Binding("external_addressable".into()),
                            then_updates: vec![Update::SetInsert {
                                field: "externally_addressable_runtime_ids".into(),
                                value: Expr::Binding("agent_runtime_id".into()),
                            }],
                            else_updates: vec![Update::SetRemove {
                                field: "externally_addressable_runtime_ids".into(),
                                value: Expr::Binding("agent_runtime_id".into()),
                            }],
                        },
                        Update::MapInsert {
                            field: "runtime_fence_tokens".into(),
                            key: Expr::Binding("agent_runtime_id".into()),
                            value: Expr::Binding("fence_token".into()),
                        },
                    ]);
                    updates
                },
                to: "Running".into(),
                emit: vec![
                    runtime_binding_emit("RequestRuntimeBinding"),
                    lifecycle_notice_emit("reset"),
                ],
            },
            TransitionSchema {
                name: "RespawnMember".into(),
                from: vec!["Running".into()],
                on: InputMatch {
                    kind: mob_trigger_kind("RespawnMember"),
                    variant: "RespawnMember".into(),
                    bindings: vec![
                        "agent_identity".into(),
                        "agent_runtime_id".into(),
                        "fence_token".into(),
                        "generation".into(),
                        "external_addressable".into(),
                    ],
                },
                guards: vec![],
                updates: {
                    let mut updates = reset_member_runtime_updates();
                    updates.extend(vec![
                        Update::SetInsert {
                            field: "live_runtime_ids".into(),
                            value: Expr::Binding("agent_runtime_id".into()),
                        },
                        Update::Conditional {
                            condition: Expr::Binding("external_addressable".into()),
                            then_updates: vec![Update::SetInsert {
                                field: "externally_addressable_runtime_ids".into(),
                                value: Expr::Binding("agent_runtime_id".into()),
                            }],
                            else_updates: vec![Update::SetRemove {
                                field: "externally_addressable_runtime_ids".into(),
                                value: Expr::Binding("agent_runtime_id".into()),
                            }],
                        },
                        Update::MapInsert {
                            field: "runtime_fence_tokens".into(),
                            key: Expr::Binding("agent_runtime_id".into()),
                            value: Expr::Binding("fence_token".into()),
                        },
                    ]);
                    updates
                },
                to: "Running".into(),
                emit: vec![
                    runtime_binding_emit("RequestRuntimeBinding"),
                    lifecycle_notice_emit("respawned"),
                ],
            },
            TransitionSchema {
                name: "MarkCompleted".into(),
                from: vec!["Running".into(), "Stopped".into()],
                on: InputMatch {
                    kind: mob_trigger_kind("MarkCompleted"),
                    variant: "MarkCompleted".into(),
                    bindings: vec![],
                },
                guards: vec![no_active_runs_guard()],
                updates: vec![],
                to: "Completed".into(),
                emit: vec![lifecycle_notice_emit("completed")],
            },
            TransitionSchema {
                name: "DestroyMob".into(),
                from: vec!["Running".into(), "Stopped".into(), "Completed".into()],
                on: InputMatch {
                    kind: mob_trigger_kind("DestroyMob"),
                    variant: "DestroyMob".into(),
                    bindings: vec![],
                },
                guards: vec![],
                updates: destroy_mob_projection_updates(),
                to: "Destroyed".into(),
                emit: vec![runtime_observation_emit("RequestRuntimeDestroy")],
            },
            TransitionSchema {
                name: "ObserveRuntimeDestroyed".into(),
                from: vec![
                    "Running".into(),
                    "Stopped".into(),
                    "Completed".into(),
                    "Destroyed".into(),
                ],
                on: InputMatch {
                    kind: mob_trigger_kind("ObserveRuntimeDestroyed"),
                    variant: "ObserveRuntimeDestroyed".into(),
                    bindings: vec!["agent_runtime_id".into(), "fence_token".into()],
                },
                guards: vec![current_binding_matches_guard()],
                updates: destroy_mob_projection_updates(),
                to: "Destroyed".into(),
                emit: vec![lifecycle_notice_emit("destroyed")],
            },
        ]
        .into_iter()
        .chain(absorbed_mob_transitions())
        .collect(),
        ci_step_limit: Some(6),
        effect_dispositions: vec![
            routed_disposition("RequestRuntimeBinding", &["MeerkatMachine"]),
            routed_disposition("RequestRuntimeIngress", &["MeerkatMachine"]),
            routed_disposition("RequestRuntimeRetire", &["MeerkatMachine"]),
            routed_disposition("RequestRuntimeDestroy", &["MeerkatMachine"]),
            external_disposition("EmitMemberLifecycleNotice"),
        ]
        .into_iter()
        .chain(absorbed_mob_effect_dispositions())
        .collect(),
    }
}

fn identity_runtime_fields() -> Vec<FieldSchema> {
    vec![
        field("agent_identity", TypeRef::Named("AgentIdentity".into())),
        field("agent_runtime_id", TypeRef::Named("AgentRuntimeId".into())),
        field("fence_token", TypeRef::Named("FenceToken".into())),
        field("generation", TypeRef::Named("Generation".into())),
        field("external_addressable", TypeRef::Bool),
    ]
}

fn runtime_observation_fields() -> Vec<FieldSchema> {
    vec![
        field("agent_runtime_id", TypeRef::Named("AgentRuntimeId".into())),
        field("fence_token", TypeRef::Named("FenceToken".into())),
    ]
}

fn runtime_binding_request_fields() -> Vec<FieldSchema> {
    vec![
        field("agent_identity", TypeRef::Named("AgentIdentity".into())),
        field("agent_runtime_id", TypeRef::Named("AgentRuntimeId".into())),
        field("fence_token", TypeRef::Named("FenceToken".into())),
        field("generation", TypeRef::Named("Generation".into())),
    ]
}

fn work_submission_fields() -> Vec<FieldSchema> {
    vec![
        field("agent_runtime_id", TypeRef::Named("AgentRuntimeId".into())),
        field("fence_token", TypeRef::Named("FenceToken".into())),
        field("work_id", TypeRef::Named("WorkId".into())),
        field("origin", TypeRef::String),
    ]
}

fn runtime_binding_emit(variant: &str) -> EffectEmit {
    EffectEmit {
        variant: variant.into(),
        fields: IndexMap::from([
            (
                "agent_identity".into(),
                Expr::Binding("agent_identity".into()),
            ),
            (
                "agent_runtime_id".into(),
                Expr::Binding("agent_runtime_id".into()),
            ),
            ("fence_token".into(), Expr::Binding("fence_token".into())),
            ("generation".into(), Expr::Binding("generation".into())),
        ]),
    }
}

fn runtime_observation_emit(variant: &str) -> EffectEmit {
    EffectEmit {
        variant: variant.into(),
        fields: IndexMap::new(),
    }
}

fn work_submission_emit(variant: &str) -> EffectEmit {
    EffectEmit {
        variant: variant.into(),
        fields: IndexMap::from([
            (
                "agent_runtime_id".into(),
                Expr::Binding("agent_runtime_id".into()),
            ),
            ("fence_token".into(), Expr::Binding("fence_token".into())),
            ("work_id".into(), Expr::Binding("work_id".into())),
            ("origin".into(), Expr::Binding("origin".into())),
        ]),
    }
}

fn lifecycle_notice_emit(kind: &str) -> EffectEmit {
    EffectEmit {
        variant: "EmitMemberLifecycleNotice".into(),
        fields: IndexMap::from([("kind".into(), Expr::String(kind.into()))]),
    }
}

fn simple_emit(variant: &str) -> EffectEmit {
    EffectEmit {
        variant: variant.into(),
        fields: IndexMap::new(),
    }
}

fn clear_work_updates() -> Vec<Update> {
    clear_runtime_projection_updates()
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

fn init(field: &str, expr: Expr) -> crate::FieldInit {
    crate::FieldInit {
        field: field.into(),
        expr,
    }
}

fn external_disposition(effect_variant: &str) -> EffectDispositionRule {
    EffectDispositionRule {
        effect_variant: effect_variant.into(),
        disposition: EffectDisposition::External,
        handoff_protocol: None,
    }
}

fn routed_disposition(effect_variant: &str, consumers: &[&str]) -> EffectDispositionRule {
    EffectDispositionRule {
        effect_variant: effect_variant.into(),
        disposition: EffectDisposition::Routed {
            consumer_machines: consumers.iter().map(|item| (*item).into()).collect(),
        },
        handoff_protocol: None,
    }
}

fn direct_mob_trigger_variants() -> Vec<VariantSchema> {
    vec![
        VariantSchema {
            name: "Spawn".into(),
            fields: identity_runtime_fields(),
        },
        VariantSchema {
            name: "ObserveRuntimeReady".into(),
            fields: runtime_observation_fields(),
        },
        VariantSchema {
            name: "SubmitWork".into(),
            fields: work_submission_fields(),
        },
        VariantSchema {
            name: "RetireMember".into(),
            fields: runtime_observation_fields(),
        },
        VariantSchema {
            name: "ObserveRuntimeRetired".into(),
            fields: runtime_observation_fields(),
        },
        VariantSchema {
            name: "ResetMember".into(),
            fields: identity_runtime_fields(),
        },
        VariantSchema {
            name: "RespawnMember".into(),
            fields: identity_runtime_fields(),
        },
        variant("DestroyMob"),
        VariantSchema {
            name: "ObserveRuntimeDestroyed".into(),
            fields: runtime_observation_fields(),
        },
        variant("MarkCompleted"),
    ]
}

fn split_mob_variants(variants: Vec<VariantSchema>) -> (Vec<VariantSchema>, Vec<VariantSchema>) {
    let mut inputs = Vec::new();
    let mut signals = Vec::new();
    for variant in variants {
        if is_mob_runtime_input_variant(&variant.name) {
            inputs.push(variant);
        } else {
            signals.push(variant);
        }
    }
    (inputs, signals)
}

fn is_mob_runtime_input_variant(variant: &str) -> bool {
    matches!(
        variant,
        "RunFlow"
            | "CancelFlow"
            | "FlowStatus"
            | "Spawn"
            | "Retire"
            | "Respawn"
            | "RetireAll"
            | "Wire"
            | "Unwire"
            | "ExternalTurn"
            | "InternalTurn"
            | "SubmitWork"
            | "CancelWork"
            | "CancelAllWork"
            | "Stop"
            | "Resume"
            | "Complete"
            | "Reset"
            | "Destroy"
            | "TaskCreate"
            | "TaskUpdate"
            | "TaskList"
            | "TaskGet"
            | "McpServerStates"
            | "RosterSnapshot"
            | "ListMembers"
            | "ListMembersIncludingRetiring"
            | "ListAllMembers"
            | "MemberStatus"
            | "SubscribeAgentEvents"
            | "SubscribeAllAgentEvents"
            | "SubscribeMobEvents"
            | "PollEvents"
            | "ReplayAllEvents"
            | "RecordOperatorActionProvenance"
            | "GetMember"
            | "SetSpawnPolicy"
            | "Shutdown"
            | "ForceCancel"
    )
}

fn absorbed_mob_input_variants() -> Vec<VariantSchema> {
    vec![
        variant("RunFlow"),
        variant("CancelFlow"),
        variant("FlowStatus"),
        VariantSchema {
            name: "Retire".into(),
            fields: vec![field("agent_runtime_id", named_runtime_id())],
        },
        VariantSchema {
            name: "Respawn".into(),
            fields: vec![field("agent_runtime_id", named_runtime_id())],
        },
        variant("RetireAll"),
        variant("Wire"),
        variant("Unwire"),
        variant("ExternalTurn"),
        variant("InternalTurn"),
        VariantSchema {
            name: "CancelWork".into(),
            fields: vec![field("work_id", named_work_id())],
        },
        VariantSchema {
            name: "CancelAllWork".into(),
            fields: runtime_observation_fields(),
        },
        variant("Stop"),
        variant("Resume"),
        variant("Complete"),
        variant("Reset"),
        variant("Destroy"),
        variant("TaskCreate"),
        variant("TaskUpdate"),
        variant("TaskList"),
        variant("TaskGet"),
        variant("McpServerStates"),
        variant("RosterSnapshot"),
        variant("ListMembers"),
        variant("ListMembersIncludingRetiring"),
        variant("ListAllMembers"),
        variant("MemberStatus"),
        variant("SubscribeAgentEvents"),
        variant("SubscribeAllAgentEvents"),
        variant("SubscribeMobEvents"),
        variant("PollEvents"),
        variant("ReplayAllEvents"),
        variant("RecordOperatorActionProvenance"),
        variant("GetMember"),
        variant("SetSpawnPolicy"),
        variant("Shutdown"),
        variant("ForceCancel"),
        variant("StartRun"),
        variant("FinishRun"),
        variant("BeginCleanup"),
        variant("FinishCleanup"),
        variant("InitializeOrchestrator"),
        variant("BindCoordinator"),
        variant("UnbindCoordinator"),
        variant("StageSpawn"),
        variant("CompleteSpawn"),
        variant("StartFlow"),
        variant("CompleteFlow"),
        variant("StopOrchestrator"),
        variant("ResumeOrchestrator"),
        variant("DestroyOrchestrator"),
        variant("ForceCancelMember"),
        variant("MemberPeerExposed"),
        variant("MemberTerminalized"),
        variant("OperationPeerTrusted"),
        variant("PeerInputAdmitted"),
        variant("CreateRun"),
    ]
}

fn absorbed_mob_transitions() -> Vec<TransitionSchema> {
    let mut transitions = Vec::new();

    // Read-only / observation commands: no phase guard in runtime — work from any state.
    let all_phases: Vec<String> = ["Running", "Stopped", "Completed", "Destroyed"]
        .iter()
        .map(|p| (*p).into())
        .collect();
    for variant in ["RecordOperatorActionProvenance", "SetSpawnPolicy"] {
        for phase in &all_phases {
            transitions.push(mob_self_loop_transition(
                variant,
                phase,
                variant,
                vec![],
                vec![],
                vec![],
            ));
        }
    }

    // --- Phase-changing transitions (NOT self-loops) ---

    // Stop: Running → Stopped
    transitions.push(TransitionSchema {
        name: "StopRunning".into(),
        from: vec!["Running".into()],
        on: InputMatch {
            kind: mob_trigger_kind("Stop"),
            variant: "Stop".into(),
            bindings: vec![],
        },
        guards: vec![no_active_runs_guard()],
        updates: vec![set_bool("coordinator_bound", false), clear_active_runs()],
        to: "Stopped".into(),
        emit: vec![simple_emit("EmitRunLifecycleNotice")],
    });

    // Resume: Stopped → Running
    transitions.push(TransitionSchema {
        name: "ResumeStopped".into(),
        from: vec!["Stopped".into()],
        on: InputMatch {
            kind: mob_trigger_kind("Resume"),
            variant: "Resume".into(),
            bindings: vec![],
        },
        guards: vec![],
        updates: vec![set_bool("coordinator_bound", true)],
        to: "Running".into(),
        emit: vec![simple_emit("EmitRunLifecycleNotice")],
    });

    // Complete: Running → Completed
    transitions.push(TransitionSchema {
        name: "CompleteRunning".into(),
        from: vec!["Running".into()],
        on: InputMatch {
            kind: mob_trigger_kind("Complete"),
            variant: "Complete".into(),
            bindings: vec![],
        },
        guards: vec![],
        updates: clear_runtime_projection_updates(),
        to: "Completed".into(),
        emit: vec![simple_emit("EmitRunLifecycleNotice")],
    });

    // Reset: Running|Stopped|Completed → Running
    transitions.push(TransitionSchema {
        name: "ResetToRunning".into(),
        from: vec!["Running".into(), "Stopped".into(), "Completed".into()],
        on: InputMatch {
            kind: mob_trigger_kind("Reset"),
            variant: "Reset".into(),
            bindings: vec![],
        },
        guards: vec![],
        updates: reset_mob_projection_updates_with_coordinator(true),
        to: "Running".into(),
        emit: vec![simple_emit("EmitRunLifecycleNotice")],
    });

    // --- Running self-loops ---
    for (variant, emit_variant, updates) in [
        ("Wire", Some("NotifyCoordinator"), vec![]),
        ("ExternalTurn", Some("EmitProgressNote"), vec![]),
        ("InternalTurn", Some("EmitProgressNote"), vec![]),
        ("TaskCreate", Some("EmitTaskNotice"), vec![]),
        ("TaskUpdate", Some("EmitTaskNotice"), vec![]),
        (
            "ForceCancel",
            Some("FlowTerminalized"),
            clear_runtime_projection_updates(),
        ),
    ] {
        transitions.push(mob_self_loop_transition(
            variant,
            "Running",
            variant,
            vec![],
            updates.clone(),
            emit_variant.into_iter().map(simple_emit).collect(),
        ));
    }

    // --- Subscribe commands: member-specific subscription requires a live member;
    // the aggregate subscriptions remain phase-agnostic.
    for phase in &all_phases {
        transitions.push(mob_guarded_self_loop_transition(
            "SubscribeAgentEvents",
            phase,
            "SubscribeAgentEvents",
            vec![],
            vec![active_members_present_guard()],
            vec![],
            vec![],
        ));
    }
    for variant in ["SubscribeAllAgentEvents", "SubscribeMobEvents"] {
        for phase in &all_phases {
            transitions.push(mob_self_loop_transition(
                variant,
                phase,
                variant,
                vec![],
                vec![],
                vec![],
            ));
        }
    }

    // Shutdown: from any non-Destroyed state. Running → Stopped; others self-loop.
    transitions.push(TransitionSchema {
        name: "ShutdownRunning".into(),
        from: vec!["Running".into()],
        on: InputMatch {
            kind: mob_trigger_kind("Shutdown"),
            variant: "Shutdown".into(),
            bindings: vec![],
        },
        guards: vec![],
        updates: vec![set_bool("coordinator_bound", false), clear_active_runs()],
        to: "Stopped".into(),
        emit: vec![simple_emit("EmitRunLifecycleNotice")],
    });
    for phase in ["Stopped", "Completed"] {
        transitions.push(mob_self_loop_transition(
            "Shutdown",
            phase,
            "Shutdown",
            vec![],
            vec![set_bool("coordinator_bound", false), clear_active_runs()],
            vec![simple_emit("EmitRunLifecycleNotice")],
        ));
    }

    // --- Signals and other Running-only self-loops ---
    for (variant, emit_variant, updates) in [
        ("CancelFlow", Some("FlowTerminalized"), clear_work_updates()),
        (
            "InitializeOrchestrator",
            Some("NotifyCoordinator"),
            vec![Update::Assign {
                field: "coordinator_bound".into(),
                expr: Expr::Bool(true),
            }],
        ),
        (
            "BindCoordinator",
            Some("NotifyCoordinator"),
            vec![Update::Assign {
                field: "coordinator_bound".into(),
                expr: Expr::Bool(true),
            }],
        ),
        (
            "UnbindCoordinator",
            Some("NotifyCoordinator"),
            vec![Update::Assign {
                field: "coordinator_bound".into(),
                expr: Expr::Bool(false),
            }],
        ),
        (
            "StageSpawn",
            Some("ExposePendingSpawn"),
            vec![Update::Increment {
                field: "pending_spawn_count".into(),
                amount: 1,
            }],
        ),
        (
            "StopOrchestrator",
            Some("NotifyCoordinator"),
            vec![Update::Assign {
                field: "coordinator_bound".into(),
                expr: Expr::Bool(false),
            }],
        ),
        (
            "ResumeOrchestrator",
            Some("NotifyCoordinator"),
            vec![Update::Assign {
                field: "coordinator_bound".into(),
                expr: Expr::Bool(true),
            }],
        ),
        (
            "DestroyOrchestrator",
            Some("NotifyCoordinator"),
            vec![Update::Assign {
                field: "coordinator_bound".into(),
                expr: Expr::Bool(false),
            }],
        ),
        (
            "ForceCancelMember",
            Some("EmitMemberTerminalNotice"),
            vec![],
        ),
        ("MemberPeerExposed", Some("AdmitPeerInput"), vec![]),
        (
            "MemberTerminalized",
            Some("EmitMemberTerminalNotice"),
            vec![],
        ),
        ("OperationPeerTrusted", Some("AdmitPeerInput"), vec![]),
        ("PeerInputAdmitted", Some("AdmitPeerInput"), vec![]),
    ] {
        transitions.push(mob_self_loop_transition(
            variant,
            "Running",
            variant,
            vec![],
            updates,
            emit_variant.into_iter().map(simple_emit).collect(),
        ));
    }

    // BeginCleanup: runtime lifecycle authority accepts from Stopped|Completed -> Stopped.
    // From Stopped it's a self-loop; from Completed it's a phase change.
    transitions.push(mob_self_loop_transition(
        "BeginCleanup",
        "Stopped",
        "BeginCleanup",
        vec![],
        vec![],
        vec![simple_emit("EmitRunLifecycleNotice")],
    ));
    transitions.push(TransitionSchema {
        name: "BeginCleanupCompleted".into(),
        from: vec!["Completed".into()],
        on: InputMatch {
            kind: mob_trigger_kind("BeginCleanup"),
            variant: "BeginCleanup".into(),
            bindings: vec![],
        },
        guards: vec![],
        updates: vec![],
        to: "Stopped".into(),
        emit: vec![simple_emit("EmitRunLifecycleNotice")],
    });

    // FinishCleanup: runtime lifecycle authority accepts from Stopped|Completed -> Stopped.
    // From Stopped it's a self-loop; from Completed it's a phase change.
    transitions.push(mob_self_loop_transition(
        "FinishCleanup",
        "Stopped",
        "FinishCleanup",
        vec![],
        vec![],
        vec![simple_emit("EmitRunLifecycleNotice")],
    ));
    transitions.push(TransitionSchema {
        name: "FinishCleanupCompleted".into(),
        from: vec!["Completed".into()],
        on: InputMatch {
            kind: mob_trigger_kind("FinishCleanup"),
            variant: "FinishCleanup".into(),
            bindings: vec![],
        },
        guards: vec![],
        updates: vec![],
        to: "Stopped".into(),
        emit: vec![simple_emit("EmitRunLifecycleNotice")],
    });

    transitions.push(TransitionSchema {
        name: "RunFlowRunning".into(),
        from: vec!["Running".into()],
        on: InputMatch {
            kind: mob_trigger_kind("RunFlow"),
            variant: "RunFlow".into(),
            bindings: vec![],
        },
        guards: vec![active_members_present_guard(), coordinator_bound_guard()],
        updates: vec![Update::Increment {
            field: "active_run_count".into(),
            amount: 1,
        }],
        to: "Running".into(),
        emit: vec![simple_emit("EmitFlowRunNotice")],
    });

    transitions.push(TransitionSchema {
        name: "StartFlowRunning".into(),
        from: vec!["Running".into()],
        on: InputMatch {
            kind: mob_trigger_kind("StartFlow"),
            variant: "StartFlow".into(),
            bindings: vec![],
        },
        guards: vec![active_members_present_guard(), coordinator_bound_guard()],
        updates: vec![Update::Increment {
            field: "active_run_count".into(),
            amount: 1,
        }],
        to: "Running".into(),
        emit: vec![simple_emit("EmitFlowRunNotice")],
    });

    transitions.push(TransitionSchema {
        name: "CreateRunRunning".into(),
        from: vec!["Running".into()],
        on: InputMatch {
            kind: mob_trigger_kind("CreateRun"),
            variant: "CreateRun".into(),
            bindings: vec![],
        },
        guards: vec![active_members_present_guard()],
        updates: vec![Update::Increment {
            field: "active_run_count".into(),
            amount: 1,
        }],
        to: "Running".into(),
        emit: vec![simple_emit("EmitRunLifecycleNotice")],
    });

    transitions.push(TransitionSchema {
        name: "StartRunRunning".into(),
        from: vec!["Running".into()],
        on: InputMatch {
            kind: mob_trigger_kind("StartRun"),
            variant: "StartRun".into(),
            bindings: vec![],
        },
        guards: vec![active_members_present_guard()],
        updates: vec![Update::Increment {
            field: "active_run_count".into(),
            amount: 1,
        }],
        to: "Running".into(),
        emit: vec![simple_emit("EmitRunLifecycleNotice")],
    });

    // Unwire: Running self-loop
    let phase = "Running";
    transitions.push(TransitionSchema {
        name: format!("Unwire{phase}"),
        from: vec![phase.into()],
        on: InputMatch {
            kind: mob_trigger_kind("Unwire"),
            variant: "Unwire".into(),
            bindings: vec![],
        },
        guards: vec![],
        updates: vec![],
        to: phase.into(),
        emit: vec![simple_emit("NotifyCoordinator")],
    });

    // CompleteFlow: runtime orchestrator authority accepts from Running|Completed -> Running.
    transitions.push(TransitionSchema {
        name: "CompleteFlowRunning".into(),
        from: vec!["Running".into(), "Completed".into()],
        on: InputMatch {
            kind: mob_trigger_kind("CompleteFlow"),
            variant: "CompleteFlow".into(),
            bindings: vec![],
        },
        guards: vec![Guard {
            name: "active_runs_present".into(),
            expr: Expr::Gt(
                Box::new(Expr::Field("active_run_count".into())),
                Box::new(Expr::U64(0)),
            ),
        }],
        updates: clear_runtime_projection_updates(),
        to: "Running".into(),
        emit: vec![simple_emit("FlowTerminalized")],
    });

    // FinishRun: runtime lifecycle authority accepts from Running|Stopped -> Running.
    transitions.push(TransitionSchema {
        name: "FinishRunRunning".into(),
        from: vec!["Running".into(), "Stopped".into()],
        on: InputMatch {
            kind: mob_trigger_kind("FinishRun"),
            variant: "FinishRun".into(),
            bindings: vec![],
        },
        guards: vec![Guard {
            name: "active_runs_present".into(),
            expr: Expr::Gt(
                Box::new(Expr::Field("active_run_count".into())),
                Box::new(Expr::U64(0)),
            ),
        }],
        updates: clear_runtime_projection_updates(),
        to: "Running".into(),
        emit: vec![simple_emit("EmitRunLifecycleNotice")],
    });

    // Retire: per-phase self-loops for Running and Stopped
    let retire_guards = vec![active_members_present_guard(), runtime_id_present_guard()];
    for phase in ["Running", "Stopped"] {
        transitions.push(TransitionSchema {
            name: format!("Retire{phase}"),
            from: vec![phase.into()],
            on: InputMatch {
                kind: mob_trigger_kind("Retire"),
                variant: "Retire".into(),
                bindings: vec!["agent_runtime_id".into()],
            },
            guards: retire_guards.clone(),
            updates: vec![
                Update::SetRemove {
                    field: "live_runtime_ids".into(),
                    value: Expr::Binding("agent_runtime_id".into()),
                },
                Update::MapRemove {
                    field: "runtime_fence_tokens".into(),
                    key: Expr::Binding("agent_runtime_id".into()),
                },
            ],
            to: phase.into(),
            emit: vec![runtime_observation_emit("RequestRuntimeRetire")],
        });
    }

    // RetireAll: per-phase self-loops for Running and Stopped
    for phase in ["Running", "Stopped"] {
        transitions.push(TransitionSchema {
            name: format!("RetireAll{phase}"),
            from: vec![phase.into()],
            on: InputMatch {
                kind: mob_trigger_kind("RetireAll"),
                variant: "RetireAll".into(),
                bindings: vec![],
            },
            guards: vec![],
            updates: vec![
                Update::Assign {
                    field: "live_runtime_ids".into(),
                    expr: Expr::EmptySet,
                },
                Update::Assign {
                    field: "runtime_fence_tokens".into(),
                    expr: Expr::EmptyMap,
                },
            ],
            to: phase.into(),
            emit: vec![lifecycle_notice_emit("retiring")],
        });
    }

    // CompleteSpawn: runtime orchestrator authority accepts from Running|Stopped -> Running.
    transitions.push(TransitionSchema {
        name: "CompleteSpawnRunning".into(),
        from: vec!["Running".into(), "Stopped".into()],
        on: InputMatch {
            kind: mob_trigger_kind("CompleteSpawn"),
            variant: "CompleteSpawn".into(),
            bindings: vec![],
        },
        guards: vec![Guard {
            name: "pending_spawns_present".into(),
            expr: Expr::Gt(
                Box::new(Expr::Field("pending_spawn_count".into())),
                Box::new(Expr::U64(0)),
            ),
        }],
        updates: vec![Update::Decrement {
            field: "pending_spawn_count".into(),
            amount: 1,
        }],
        to: "Running".into(),
        emit: vec![lifecycle_notice_emit("spawned")],
    });

    // Destroy input: runtime allows from all non-Destroyed phases via lifecycle authority.
    transitions.push(TransitionSchema {
        name: "DestroyFromAny".into(),
        from: vec!["Running".into(), "Stopped".into(), "Completed".into()],
        on: InputMatch {
            kind: mob_trigger_kind("Destroy"),
            variant: "Destroy".into(),
            bindings: vec![],
        },
        guards: vec![],
        updates: destroy_mob_projection_updates(),
        to: "Destroyed".into(),
        emit: vec![],
    });

    // Respawn: Running self-loop
    let phase = "Running";
    transitions.push(TransitionSchema {
        name: "RespawnRunning".into(),
        from: vec![phase.into()],
        on: InputMatch {
            kind: mob_trigger_kind("Respawn"),
            variant: "Respawn".into(),
            bindings: vec!["agent_runtime_id".into()],
        },
        guards: vec![runtime_id_present_guard(), coordinator_bound_guard()],
        updates: vec![Update::MapInsert {
            field: "runtime_fence_tokens".into(),
            key: Expr::Binding("agent_runtime_id".into()),
            value: Expr::Add(
                Box::new(Expr::MapGet {
                    map: Box::new(Expr::Field("runtime_fence_tokens".into())),
                    key: Box::new(Expr::Binding("agent_runtime_id".into())),
                }),
                Box::new(Expr::U64(1)),
            ),
        }],
        to: phase.into(),
        emit: vec![simple_emit("ExposePendingSpawn")],
    });

    // CancelAllWork: delegates to ForceCancel which the formal model keeps in Running.
    let phase = "Running";
    transitions.push(mob_guarded_self_loop_transition(
        "CancelAllWork",
        phase,
        "CancelAllWork",
        vec!["agent_runtime_id", "fence_token"],
        vec![
            active_members_present_guard(),
            current_binding_matches_guard(),
        ],
        clear_work_updates(),
        vec![simple_emit("FlowTerminalized")],
    ));

    transitions
}

fn mob_self_loop_transition(
    name: &str,
    phase: &str,
    variant: &str,
    bindings: Vec<&str>,
    updates: Vec<Update>,
    emit: Vec<EffectEmit>,
) -> TransitionSchema {
    TransitionSchema {
        name: format!("{name}{phase}"),
        from: vec![phase.into()],
        on: InputMatch {
            kind: mob_trigger_kind(variant),
            variant: variant.into(),
            bindings: bindings.into_iter().map(Into::into).collect(),
        },
        guards: vec![],
        updates,
        to: phase.into(),
        emit,
    }
}

fn mob_guarded_self_loop_transition(
    name: &str,
    phase: &str,
    variant: &str,
    bindings: Vec<&str>,
    guards: Vec<Guard>,
    updates: Vec<Update>,
    emit: Vec<EffectEmit>,
) -> TransitionSchema {
    TransitionSchema {
        name: format!("{name}{phase}"),
        from: vec![phase.into()],
        on: InputMatch {
            kind: mob_trigger_kind(variant),
            variant: variant.into(),
            bindings: bindings.into_iter().map(Into::into).collect(),
        },
        guards,
        updates,
        to: phase.into(),
        emit,
    }
}

fn mob_trigger_kind(variant: &str) -> TriggerKind {
    if is_mob_runtime_input_variant(variant) {
        TriggerKind::Input
    } else {
        TriggerKind::Signal
    }
}

fn active_members_present_guard() -> Guard {
    Guard {
        name: "active_members_present".into(),
        expr: Expr::Neq(
            Box::new(Expr::Field("live_runtime_ids".into())),
            Box::new(Expr::EmptySet),
        ),
    }
}

fn current_binding_matches_guard() -> Guard {
    Guard {
        name: "current_binding_matches".into(),
        expr: Expr::And(vec![
            Expr::Contains {
                collection: Box::new(Expr::Field("live_runtime_ids".into())),
                value: Box::new(Expr::Binding("agent_runtime_id".into())),
            },
            Expr::Eq(
                Box::new(Expr::MapGet {
                    map: Box::new(Expr::Field("runtime_fence_tokens".into())),
                    key: Box::new(Expr::Binding("agent_runtime_id".into())),
                }),
                Box::new(Expr::Binding("fence_token".into())),
            ),
        ]),
    }
}

fn runtime_externally_addressable_guard() -> Guard {
    Guard {
        name: "runtime_externally_addressable".into(),
        expr: Expr::Contains {
            collection: Box::new(Expr::Field("externally_addressable_runtime_ids".into())),
            value: Box::new(Expr::Binding("agent_runtime_id".into())),
        },
    }
}

fn external_origin_guard() -> Guard {
    Guard {
        name: "external_origin".into(),
        expr: Expr::Eq(
            Box::new(Expr::Binding("origin".into())),
            Box::new(Expr::String("External".into())),
        ),
    }
}

fn internal_origin_guard() -> Guard {
    Guard {
        name: "internal_origin".into(),
        expr: Expr::Eq(
            Box::new(Expr::Binding("origin".into())),
            Box::new(Expr::String("Internal".into())),
        ),
    }
}

fn runtime_id_present_guard() -> Guard {
    Guard {
        name: "runtime_id_present".into(),
        expr: Expr::Contains {
            collection: Box::new(Expr::Field("live_runtime_ids".into())),
            value: Box::new(Expr::Binding("agent_runtime_id".into())),
        },
    }
}

fn no_active_runs_guard() -> Guard {
    Guard {
        name: "no_active_runs".into(),
        expr: Expr::Eq(
            Box::new(Expr::Field("active_run_count".into())),
            Box::new(Expr::U64(0)),
        ),
    }
}

fn coordinator_bound_guard() -> Guard {
    Guard {
        name: "coordinator_bound".into(),
        expr: Expr::Eq(
            Box::new(Expr::Field("coordinator_bound".into())),
            Box::new(Expr::Bool(true)),
        ),
    }
}

fn clear_runtime_projection_updates() -> Vec<Update> {
    vec![clear_active_runs()]
}

fn clear_current_runtime_projection_updates() -> Vec<Update> {
    let mut updates = clear_runtime_projection_updates();
    updates.splice(
        0..0,
        [
            Update::SetRemove {
                field: "live_runtime_ids".into(),
                value: Expr::Binding("agent_runtime_id".into()),
            },
            Update::SetRemove {
                field: "externally_addressable_runtime_ids".into(),
                value: Expr::Binding("agent_runtime_id".into()),
            },
            Update::MapRemove {
                field: "runtime_fence_tokens".into(),
                key: Expr::Binding("agent_runtime_id".into()),
            },
        ],
    );
    updates
}

fn clear_active_runs() -> Update {
    Update::Assign {
        field: "active_run_count".into(),
        expr: Expr::U64(0),
    }
}

fn set_bool(field: &str, value: bool) -> Update {
    Update::Assign {
        field: field.into(),
        expr: Expr::Bool(value),
    }
}

fn reset_member_runtime_updates() -> Vec<Update> {
    let mut updates = clear_runtime_projection_updates();
    updates.extend(vec![Update::Assign {
        field: "pending_spawn_count".into(),
        expr: Expr::U64(0),
    }]);
    updates
}

fn reset_mob_projection_updates_with_coordinator(coordinator_bound: bool) -> Vec<Update> {
    let mut updates = clear_runtime_projection_updates();
    updates.extend(vec![
        Update::Assign {
            field: "pending_spawn_count".into(),
            expr: Expr::U64(0),
        },
        set_bool("coordinator_bound", coordinator_bound),
    ]);
    updates
}

fn destroy_mob_projection_updates() -> Vec<Update> {
    let mut updates = vec![
        Update::Assign {
            field: "live_runtime_ids".into(),
            expr: Expr::EmptySet,
        },
        Update::Assign {
            field: "runtime_fence_tokens".into(),
            expr: Expr::EmptyMap,
        },
    ];
    updates.extend(vec![
        Update::Assign {
            field: "active_run_count".into(),
            expr: Expr::U64(0),
        },
        Update::Assign {
            field: "pending_spawn_count".into(),
            expr: Expr::U64(0),
        },
        set_bool("coordinator_bound", false),
    ]);
    updates
}

fn absorbed_mob_effect_variants() -> Vec<VariantSchema> {
    vec![
        variant("EmitRunLifecycleNotice"),
        variant("EmitFlowRunNotice"),
        variant("AppendFailureLedger"),
        variant("FlowTerminalized"),
        variant("EscalateSupervisor"),
        variant("NotifyCoordinator"),
        variant("ExposePendingSpawn"),
        variant("EmitMemberTerminalNotice"),
        variant("AdmitPeerInput"),
        variant("EmitProgressNote"),
        variant("EmitTaskNotice"),
    ]
}

fn absorbed_mob_effect_dispositions() -> Vec<EffectDispositionRule> {
    vec![
        external_disposition("EmitRunLifecycleNotice"),
        external_disposition("EmitFlowRunNotice"),
        local_disposition("AppendFailureLedger"),
        external_disposition("FlowTerminalized"),
        external_disposition("EscalateSupervisor"),
        external_disposition("NotifyCoordinator"),
        external_disposition("ExposePendingSpawn"),
        external_disposition("EmitMemberTerminalNotice"),
        external_disposition("AdmitPeerInput"),
        external_disposition("EmitProgressNote"),
        external_disposition("EmitTaskNotice"),
    ]
}

fn local_disposition(effect_variant: &str) -> EffectDispositionRule {
    EffectDispositionRule {
        effect_variant: effect_variant.into(),
        disposition: EffectDisposition::Local,
        handoff_protocol: None,
    }
}

fn named_runtime_id() -> TypeRef {
    TypeRef::Named("AgentRuntimeId".into())
}

fn named_work_id() -> TypeRef {
    TypeRef::Named("WorkId".into())
}
