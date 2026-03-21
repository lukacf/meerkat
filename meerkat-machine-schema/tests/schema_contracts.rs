use std::collections::BTreeMap;

use meerkat_machine_schema::catalog::{
    flow_run_machine, mob_bundle_composition, mob_lifecycle_machine, mob_orchestrator_machine,
    ops_lifecycle_machine, ops_runtime_bundle_composition, peer_comms_machine,
    peer_runtime_bundle_composition, runtime_control_machine, runtime_ingress_machine,
    runtime_pipeline_composition, turn_execution_machine,
};
use meerkat_machine_schema::{
    CompositionSchemaError, EffectDisposition, Route, RouteBindingSource, RouteDelivery,
    RouteFieldBinding, RouteTarget, canonical_composition_schemas, canonical_machine_schemas,
    input_lifecycle_machine,
};

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

#[test]
fn validates_input_lifecycle_machine_definition() {
    assert_eq!(input_lifecycle_machine().validate(), Ok(()));
}

#[test]
fn validates_mob_lifecycle_machine_definition() {
    assert_eq!(mob_lifecycle_machine().validate(), Ok(()));
}

#[test]
fn canonical_machine_registry_is_individually_valid() {
    for schema in canonical_machine_schemas() {
        assert_eq!(
            schema.validate(),
            Ok(()),
            "machine {} should validate",
            schema.machine
        );
    }
}

#[test]
fn validates_flow_run_machine_definition() {
    assert_eq!(flow_run_machine().validate(), Ok(()));
}

#[test]
fn validates_ops_lifecycle_machine_definition() {
    assert_eq!(ops_lifecycle_machine().validate(), Ok(()));
}

#[test]
fn validates_runtime_pipeline_composition() {
    let runtime_control = runtime_control_machine();
    let runtime_ingress = runtime_ingress_machine();
    let turn_execution = turn_execution_machine();
    let composition = runtime_pipeline_composition();

    assert_eq!(
        composition.validate_against(&[&runtime_control, &runtime_ingress, &turn_execution]),
        Ok(())
    );
}

#[test]
fn rejects_route_with_type_mismatch() {
    let runtime_control = runtime_control_machine();
    let runtime_ingress = runtime_ingress_machine();
    let turn_execution = turn_execution_machine();
    let mut composition = runtime_pipeline_composition();
    composition.routes.push(Route {
        name: "bad_boundary_sequence_into_run_id".into(),
        from_machine: "runtime_ingress".into(),
        effect_variant: "ReadyForRun".into(),
        to: RouteTarget {
            machine: "turn_execution".into(),
            input_variant: "StartConversationRun".into(),
        },
        bindings: vec![RouteFieldBinding {
            to_field: "run_id".into(),
            source: RouteBindingSource::Field {
                from_field: "contributing_input_ids".into(),
                allow_named_alias: false,
            },
        }],
        delivery: RouteDelivery::Immediate,
    });
    composition.witnesses[0]
        .expected_routes
        .push("bad_boundary_sequence_into_run_id".into());

    let result =
        composition.validate_against(&[&runtime_control, &runtime_ingress, &turn_execution]);
    assert!(matches!(
        result,
        Err(CompositionSchemaError::RouteFieldTypeMismatch { .. }
            | CompositionSchemaError::MachineSchema(_))
    ));
}

#[test]
fn rejects_missing_failure_outcome_route() {
    let runtime_control = runtime_control_machine();
    let runtime_ingress = runtime_ingress_machine();
    let turn_execution = turn_execution_machine();
    let mut composition = runtime_pipeline_composition();
    composition
        .routes
        .retain(|route| route.name != "execution_failure_updates_ingress");
    for witness in &mut composition.witnesses {
        witness
            .expected_routes
            .retain(|route| route != "execution_failure_updates_ingress");
    }

    let result =
        composition.validate_against(&[&runtime_control, &runtime_ingress, &turn_execution]);
    assert!(matches!(
        result,
        Err(CompositionSchemaError::MissingOutcomeRoute { .. })
    ));
}

#[test]
fn rejects_missing_scheduler_rule() {
    let runtime_control = runtime_control_machine();
    let runtime_ingress = runtime_ingress_machine();
    let turn_execution = turn_execution_machine();
    let mut composition = runtime_pipeline_composition();
    composition.scheduler_rules.clear();
    for witness in &mut composition.witnesses {
        witness.expected_scheduler_rules.clear();
    }

    let result =
        composition.validate_against(&[&runtime_control, &runtime_ingress, &turn_execution]);
    assert!(matches!(
        result,
        Err(CompositionSchemaError::MissingRequiredSchedulerRule { .. })
    ));
}

#[test]
fn rejects_zero_deep_domain_override() {
    let mut composition = runtime_pipeline_composition();
    composition.deep_domain_overrides = BTreeMap::from([("WorkIdValues".into(), 0)]);

    let result = composition.validate();
    assert!(matches!(
        result,
        Err(CompositionSchemaError::InvalidNamedDomainCardinality { .. })
    ));
}

#[test]
fn validates_peer_runtime_bundle_with_alias_and_literal_bindings() {
    let peer_comms = peer_comms_machine();
    let runtime_control = runtime_control_machine();
    let runtime_ingress = runtime_ingress_machine();
    let composition = peer_runtime_bundle_composition();

    assert_eq!(
        composition.validate_against(&[&peer_comms, &runtime_control, &runtime_ingress]),
        Ok(())
    );
}

#[test]
fn validates_ops_runtime_bundle_with_alias_and_literal_bindings() {
    let ops_lifecycle = ops_lifecycle_machine();
    let runtime_control = runtime_control_machine();
    let runtime_ingress = runtime_ingress_machine();
    let turn_execution = turn_execution_machine();
    let composition = ops_runtime_bundle_composition();

    assert_eq!(
        composition.validate_against(&[
            &ops_lifecycle,
            &runtime_control,
            &runtime_ingress,
            &turn_execution,
        ]),
        Ok(())
    );
}

#[test]
fn rejects_unsupported_route_literal_expression() {
    let peer_comms = peer_comms_machine();
    let runtime_control = runtime_control_machine();
    let runtime_ingress = runtime_ingress_machine();
    let mut composition = peer_runtime_bundle_composition();
    composition.routes[0].bindings[1].source =
        RouteBindingSource::Literal(meerkat_machine_schema::Expr::Field("not_allowed".into()));

    let result = composition.validate_against(&[&peer_comms, &runtime_control, &runtime_ingress]);
    assert!(matches!(
        result,
        Err(CompositionSchemaError::UnsupportedRouteLiteral { .. })
    ));
}

#[test]
fn validates_mob_bundle_skeleton_routes() {
    let mob_lifecycle = mob_lifecycle_machine();
    let mob_orchestrator = mob_orchestrator_machine();
    let flow_run = flow_run_machine();
    let ops_lifecycle = ops_lifecycle_machine();
    let peer_comms = peer_comms_machine();
    let runtime_control = runtime_control_machine();
    let runtime_ingress = runtime_ingress_machine();
    let turn_execution = turn_execution_machine();
    let composition = mob_bundle_composition();

    assert_eq!(
        composition.validate_against(&[
            &mob_lifecycle,
            &mob_orchestrator,
            &flow_run,
            &ops_lifecycle,
            &peer_comms,
            &runtime_control,
            &runtime_ingress,
            &turn_execution,
        ]),
        Ok(())
    );
}

/// Closed-world audit gate: every Routed effect in every closed-world composition that
/// contains both producer and consumer machine types must have a corresponding route.
/// This runs in `cargo rct` as an automated verification that route coverage is complete.
#[test]
fn closed_world_audit_all_routed_effects_have_routes() {
    let machines = canonical_machine_schemas();
    let compositions = canonical_composition_schemas();

    let mut failures = Vec::new();
    for composition in &compositions {
        if !composition.closed_world {
            continue;
        }

        let machine_refs: Vec<&_> = machines
            .iter()
            .filter(|m| {
                composition
                    .machines
                    .iter()
                    .any(|inst| inst.machine_name == m.machine)
            })
            .collect();

        if let Err(e) = composition.validate_against(&machine_refs) {
            failures.push(format!("  {}: {}", composition.name, e));
        }
    }
    assert!(
        failures.is_empty(),
        "closed-world route audit failures:\n{}",
        failures.join("\n")
    );
}

/// Negative test: a Routed effect with no corresponding route in a closed-world
/// composition must produce a MissingRoutedEffect error.
#[test]
fn rejects_routed_effect_without_route_in_closed_world_composition() {
    use indexmap::IndexMap;
    use meerkat_machine_schema::{
        ActorKind, ActorSchema, CompositionSchema, EffectDispositionRule, EffectEmit, EntryInput,
        EnumSchema, InitSchema, MachineInstance, MachineSchema, RustBinding, StateSchema,
        VariantSchema,
    };

    let producer = MachineSchema {
        machine: "ProducerMachine".into(),
        version: 1,
        rust: RustBinding {
            crate_name: "test".into(),
            module: "test::producer".into(),
        },
        state: StateSchema {
            phase: EnumSchema {
                name: "ProducerState".into(),
                variants: vec![VariantSchema {
                    name: "Ready".into(),
                    fields: vec![],
                }],
            },
            fields: vec![],
            init: InitSchema {
                phase: "Ready".into(),
                fields: vec![],
            },
            terminal_phases: vec![],
        },
        inputs: EnumSchema {
            name: "ProducerInput".into(),
            variants: vec![VariantSchema {
                name: "Go".into(),
                fields: vec![],
            }],
        },
        effects: EnumSchema {
            name: "ProducerEffect".into(),
            variants: vec![VariantSchema {
                name: "Handoff".into(),
                fields: vec![],
            }],
        },
        helpers: vec![],
        derived: vec![],
        invariants: vec![],
        transitions: vec![meerkat_machine_schema::TransitionSchema {
            name: "Go".into(),
            from: vec!["Ready".into()],
            on: meerkat_machine_schema::InputMatch {
                variant: "Go".into(),
                bindings: vec![],
            },
            guards: vec![],
            updates: vec![],
            to: "Ready".into(),
            emit: vec![EffectEmit {
                variant: "Handoff".into(),
                fields: IndexMap::new(),
            }],
        }],
        effect_dispositions: vec![EffectDispositionRule {
            effect_variant: "Handoff".into(),
            disposition: EffectDisposition::Routed {
                consumer_machines: vec!["ConsumerMachine".into()],
            },
            handoff_protocol: None,
        }],
    };

    let consumer = MachineSchema {
        machine: "ConsumerMachine".into(),
        version: 1,
        rust: RustBinding {
            crate_name: "test".into(),
            module: "test::consumer".into(),
        },
        state: StateSchema {
            phase: EnumSchema {
                name: "ConsumerState".into(),
                variants: vec![VariantSchema {
                    name: "Idle".into(),
                    fields: vec![],
                }],
            },
            fields: vec![],
            init: InitSchema {
                phase: "Idle".into(),
                fields: vec![],
            },
            terminal_phases: vec![],
        },
        inputs: EnumSchema {
            name: "ConsumerInput".into(),
            variants: vec![VariantSchema {
                name: "Receive".into(),
                fields: vec![],
            }],
        },
        effects: EnumSchema {
            name: "ConsumerEffect".into(),
            variants: vec![],
        },
        helpers: vec![],
        derived: vec![],
        invariants: vec![],
        transitions: vec![],
        effect_dispositions: vec![],
    };

    let composition = CompositionSchema {
        name: "test_missing_route".into(),
        machines: vec![
            MachineInstance {
                instance_id: "producer".into(),
                machine_name: "ProducerMachine".into(),
                actor: "actor_a".into(),
            },
            MachineInstance {
                instance_id: "consumer".into(),
                machine_name: "ConsumerMachine".into(),
                actor: "actor_b".into(),
            },
        ],
        actors: vec![
            ActorSchema {
                name: "actor_a".into(),
                kind: ActorKind::Machine,
            },
            ActorSchema {
                name: "actor_b".into(),
                kind: ActorKind::Machine,
            },
        ],
        handoff_protocols: vec![],
        entry_inputs: vec![EntryInput {
            name: "go".into(),
            machine: "producer".into(),
            input_variant: "Go".into(),
        }],
        routes: vec![], // deliberately empty — should fail
        actor_priorities: vec![],
        scheduler_rules: vec![],
        invariants: vec![],
        witnesses: vec![],
        deep_domain_cardinality: 1,
        deep_domain_overrides: BTreeMap::new(),
        witness_domain_cardinality: 1,
        ci_limits: None,
        closed_world: true,
    };

    let result = composition.validate_against(&[&producer, &consumer]);
    assert!(
        matches!(
            result,
            Err(CompositionSchemaError::MissingRoutedEffect { .. })
        ),
        "expected MissingRoutedEffect error, got: {result:?}"
    );
}

#[test]
fn handoff_protocol_accepted_on_local_effect() {
    use meerkat_machine_schema::{
        EffectDispositionRule, EnumSchema, InitSchema, MachineSchema, RustBinding, StateSchema,
        VariantSchema,
    };

    let schema = MachineSchema {
        machine: "TestMachine".into(),
        version: 1,
        rust: RustBinding {
            crate_name: "test".into(),
            module: "test::handoff".into(),
        },
        state: StateSchema {
            phase: EnumSchema {
                name: "Phase".into(),
                variants: vec![VariantSchema {
                    name: "Ready".into(),
                    fields: vec![],
                }],
            },
            fields: vec![],
            init: InitSchema {
                phase: "Ready".into(),
                fields: vec![],
            },
            terminal_phases: vec![],
        },
        inputs: EnumSchema {
            name: "Input".into(),
            variants: vec![],
        },
        effects: EnumSchema {
            name: "Effect".into(),
            variants: vec![VariantSchema {
                name: "DoSomething".into(),
                fields: vec![],
            }],
        },
        helpers: vec![],
        derived: vec![],
        invariants: vec![],
        transitions: vec![],
        effect_dispositions: vec![EffectDispositionRule {
            effect_variant: "DoSomething".into(),
            disposition: EffectDisposition::Local,
            handoff_protocol: Some("my_protocol".into()),
        }],
    };

    assert_eq!(schema.validate(), Ok(()));
}

#[test]
fn handoff_protocol_accepted_on_external_effect() {
    use meerkat_machine_schema::{
        EffectDispositionRule, EnumSchema, InitSchema, MachineSchema, RustBinding, StateSchema,
        VariantSchema,
    };

    let schema = MachineSchema {
        machine: "TestMachine".into(),
        version: 1,
        rust: RustBinding {
            crate_name: "test".into(),
            module: "test::handoff_ext".into(),
        },
        state: StateSchema {
            phase: EnumSchema {
                name: "Phase".into(),
                variants: vec![VariantSchema {
                    name: "Ready".into(),
                    fields: vec![],
                }],
            },
            fields: vec![],
            init: InitSchema {
                phase: "Ready".into(),
                fields: vec![],
            },
            terminal_phases: vec![],
        },
        inputs: EnumSchema {
            name: "Input".into(),
            variants: vec![],
        },
        effects: EnumSchema {
            name: "Effect".into(),
            variants: vec![VariantSchema {
                name: "Notify".into(),
                fields: vec![],
            }],
        },
        helpers: vec![],
        derived: vec![],
        invariants: vec![],
        transitions: vec![],
        effect_dispositions: vec![EffectDispositionRule {
            effect_variant: "Notify".into(),
            disposition: EffectDisposition::External,
            handoff_protocol: Some("notify_protocol".into()),
        }],
    };

    assert_eq!(schema.validate(), Ok(()));
}

#[test]
fn handoff_protocol_rejected_on_routed_effect() {
    use meerkat_machine_schema::{
        EffectDispositionRule, EnumSchema, InitSchema, MachineSchema, MachineSchemaError,
        RustBinding, StateSchema, VariantSchema,
    };

    let schema = MachineSchema {
        machine: "TestMachine".into(),
        version: 1,
        rust: RustBinding {
            crate_name: "test".into(),
            module: "test::handoff_routed".into(),
        },
        state: StateSchema {
            phase: EnumSchema {
                name: "Phase".into(),
                variants: vec![VariantSchema {
                    name: "Ready".into(),
                    fields: vec![],
                }],
            },
            fields: vec![],
            init: InitSchema {
                phase: "Ready".into(),
                fields: vec![],
            },
            terminal_phases: vec![],
        },
        inputs: EnumSchema {
            name: "Input".into(),
            variants: vec![],
        },
        effects: EnumSchema {
            name: "Effect".into(),
            variants: vec![VariantSchema {
                name: "Route".into(),
                fields: vec![],
            }],
        },
        helpers: vec![],
        derived: vec![],
        invariants: vec![],
        transitions: vec![],
        effect_dispositions: vec![EffectDispositionRule {
            effect_variant: "Route".into(),
            disposition: EffectDisposition::Routed {
                consumer_machines: vec!["SomeConsumer".into()],
            },
            handoff_protocol: Some("bad_protocol".into()),
        }],
    };

    assert_eq!(
        schema.validate(),
        Err(MachineSchemaError::HandoffProtocolOnRoutedEffect {
            variant: "Route".into(),
        })
    );
}

#[test]
fn actor_kind_owner_accepted_in_composition() {
    use meerkat_machine_schema::{ActorKind, ActorSchema, CompositionSchema, MachineInstance};
    use std::collections::BTreeMap;

    // Owner actors are allowed in the composition but cannot own machine instances
    let composition = CompositionSchema {
        name: "test_owner_actor".into(),
        machines: vec![MachineInstance {
            instance_id: "some_machine".into(),
            machine_name: "SomeMachine".into(),
            actor: "machine_actor".into(),
        }],
        actors: vec![
            ActorSchema {
                name: "machine_actor".into(),
                kind: ActorKind::Machine,
            },
            ActorSchema {
                name: "session_host".into(),
                kind: ActorKind::Owner,
            },
        ],
        handoff_protocols: vec![],
        entry_inputs: vec![],
        routes: vec![],
        actor_priorities: vec![],
        scheduler_rules: vec![],
        invariants: vec![],
        witnesses: vec![],
        deep_domain_cardinality: 1,
        deep_domain_overrides: BTreeMap::new(),
        witness_domain_cardinality: 1,
        ci_limits: None,
        closed_world: false,
    };

    assert_eq!(composition.validate(), Ok(()));
}

#[test]
fn machine_instance_with_owner_actor_rejected() {
    use meerkat_machine_schema::{
        ActorKind, ActorSchema, CompositionSchema, CompositionSchemaError, MachineInstance,
    };
    use std::collections::BTreeMap;

    let composition = CompositionSchema {
        name: "test_owner_mismatch".into(),
        machines: vec![MachineInstance {
            instance_id: "some_machine".into(),
            machine_name: "SomeMachine".into(),
            actor: "owner_actor".into(),
        }],
        actors: vec![ActorSchema {
            name: "owner_actor".into(),
            kind: ActorKind::Owner,
        }],
        handoff_protocols: vec![],
        entry_inputs: vec![],
        routes: vec![],
        actor_priorities: vec![],
        scheduler_rules: vec![],
        invariants: vec![],
        witnesses: vec![],
        deep_domain_cardinality: 1,
        deep_domain_overrides: BTreeMap::new(),
        witness_domain_cardinality: 1,
        ci_limits: None,
        closed_world: false,
    };

    assert_eq!(
        composition.validate(),
        Err(CompositionSchemaError::ActorKindMismatch {
            actor: "owner_actor".into(),
            expected: ActorKind::Machine,
            actual: ActorKind::Owner,
        })
    );
}

#[test]
fn handoff_protocol_valid_round_trip() {
    use meerkat_machine_schema::{
        ActorKind, ActorSchema, ClosurePolicy, CompositionSchema, EffectDisposition,
        EffectDispositionRule, EffectHandoffProtocol, EnumSchema, FeedbackInputRef, FieldSchema,
        InitSchema, MachineInstance, MachineSchema, RustBinding, StateSchema, TypeRef,
        VariantSchema,
    };
    use std::collections::BTreeMap;

    let producer_machine = MachineSchema {
        machine: "ProducerMachine".into(),
        version: 1,
        rust: RustBinding {
            crate_name: "test".into(),
            module: "test::producer".into(),
        },
        state: StateSchema {
            phase: EnumSchema {
                name: "ProducerPhase".into(),
                variants: vec![
                    VariantSchema {
                        name: "Running".into(),
                        fields: vec![],
                    },
                    VariantSchema {
                        name: "Done".into(),
                        fields: vec![],
                    },
                ],
            },
            fields: vec![],
            init: InitSchema {
                phase: "Running".into(),
                fields: vec![],
            },
            terminal_phases: vec!["Done".into()],
        },
        inputs: EnumSchema {
            name: "ProducerInput".into(),
            variants: vec![
                VariantSchema {
                    name: "Go".into(),
                    fields: vec![],
                },
                VariantSchema {
                    name: "Ack".into(),
                    fields: vec![FieldSchema {
                        name: "op_id".into(),
                        ty: TypeRef::String,
                    }],
                },
            ],
        },
        effects: EnumSchema {
            name: "ProducerEffect".into(),
            variants: vec![VariantSchema {
                name: "RequestWork".into(),
                fields: vec![FieldSchema {
                    name: "op_id".into(),
                    ty: TypeRef::String,
                }],
            }],
        },
        helpers: vec![],
        derived: vec![],
        invariants: vec![],
        transitions: vec![],
        effect_dispositions: vec![EffectDispositionRule {
            effect_variant: "RequestWork".into(),
            disposition: EffectDisposition::External,
            handoff_protocol: Some("work_handoff".into()),
        }],
    };

    let composition = CompositionSchema {
        name: "test_handoff_valid".into(),
        machines: vec![MachineInstance {
            instance_id: "producer".into(),
            machine_name: "ProducerMachine".into(),
            actor: "machine_actor".into(),
        }],
        actors: vec![
            ActorSchema {
                name: "machine_actor".into(),
                kind: ActorKind::Machine,
            },
            ActorSchema {
                name: "host".into(),
                kind: ActorKind::Owner,
            },
        ],
        handoff_protocols: vec![EffectHandoffProtocol {
            name: "work_handoff".into(),
            producer_instance: "producer".into(),
            effect_variant: "RequestWork".into(),
            realizing_actor: "host".into(),
            correlation_fields: vec!["op_id".into()],
            allowed_feedback_inputs: vec![FeedbackInputRef {
                machine_instance: "producer".into(),
                input_variant: "Ack".into(),
            }],
            closure_policy: ClosurePolicy::AckRequired,
            liveness_annotation: None,
        }],
        entry_inputs: vec![],
        routes: vec![],
        actor_priorities: vec![],
        scheduler_rules: vec![],
        invariants: vec![],
        witnesses: vec![],
        deep_domain_cardinality: 1,
        deep_domain_overrides: BTreeMap::new(),
        witness_domain_cardinality: 1,
        ci_limits: None,
        closed_world: false,
    };

    composition
        .validate_against(&[&producer_machine])
        .expect("valid handoff protocol should pass");
}

#[test]
fn handoff_protocol_unknown_producer() {
    use meerkat_machine_schema::{
        ActorKind, ActorSchema, ClosurePolicy, CompositionSchema, EffectHandoffProtocol,
        MachineInstance,
    };
    use std::collections::BTreeMap;

    let composition = CompositionSchema {
        name: "test_unknown_producer".into(),
        machines: vec![MachineInstance {
            instance_id: "real_machine".into(),
            machine_name: "M".into(),
            actor: "actor_a".into(),
        }],
        actors: vec![
            ActorSchema {
                name: "actor_a".into(),
                kind: ActorKind::Machine,
            },
            ActorSchema {
                name: "host".into(),
                kind: ActorKind::Owner,
            },
        ],
        handoff_protocols: vec![EffectHandoffProtocol {
            name: "bad_proto".into(),
            producer_instance: "nonexistent".into(),
            effect_variant: "E".into(),
            realizing_actor: "host".into(),
            correlation_fields: vec![],
            allowed_feedback_inputs: vec![],
            closure_policy: ClosurePolicy::AckRequired,
            liveness_annotation: None,
        }],
        entry_inputs: vec![],
        routes: vec![],
        actor_priorities: vec![],
        scheduler_rules: vec![],
        invariants: vec![],
        witnesses: vec![],
        deep_domain_cardinality: 1,
        deep_domain_overrides: BTreeMap::new(),
        witness_domain_cardinality: 1,
        ci_limits: None,
        closed_world: false,
    };

    assert_eq!(
        composition.validate(),
        Err(CompositionSchemaError::UnknownHandoffProducer {
            protocol: "bad_proto".into(),
            instance: "nonexistent".into(),
        })
    );
}

#[test]
fn handoff_protocol_actor_not_owner() {
    use meerkat_machine_schema::{
        ActorKind, ActorSchema, ClosurePolicy, CompositionSchema, EffectHandoffProtocol,
        MachineInstance,
    };
    use std::collections::BTreeMap;

    let composition = CompositionSchema {
        name: "test_actor_not_owner".into(),
        machines: vec![MachineInstance {
            instance_id: "producer".into(),
            machine_name: "M".into(),
            actor: "actor_a".into(),
        }],
        actors: vec![
            ActorSchema {
                name: "actor_a".into(),
                kind: ActorKind::Machine,
            },
            ActorSchema {
                name: "also_machine".into(),
                kind: ActorKind::Machine,
            },
        ],
        handoff_protocols: vec![EffectHandoffProtocol {
            name: "bad_proto".into(),
            producer_instance: "producer".into(),
            effect_variant: "E".into(),
            realizing_actor: "also_machine".into(),
            correlation_fields: vec![],
            allowed_feedback_inputs: vec![],
            closure_policy: ClosurePolicy::AckRequired,
            liveness_annotation: None,
        }],
        entry_inputs: vec![],
        routes: vec![],
        actor_priorities: vec![],
        scheduler_rules: vec![],
        invariants: vec![],
        witnesses: vec![],
        deep_domain_cardinality: 1,
        deep_domain_overrides: BTreeMap::new(),
        witness_domain_cardinality: 1,
        ci_limits: None,
        closed_world: false,
    };

    assert_eq!(
        composition.validate(),
        Err(CompositionSchemaError::HandoffActorNotOwner {
            protocol: "bad_proto".into(),
            actor: "also_machine".into(),
        })
    );
}

#[test]
fn handoff_protocol_unknown_feedback_machine() {
    use meerkat_machine_schema::{
        ActorKind, ActorSchema, ClosurePolicy, CompositionSchema, EffectHandoffProtocol,
        FeedbackInputRef, MachineInstance,
    };
    use std::collections::BTreeMap;

    let composition = CompositionSchema {
        name: "test_unknown_feedback_machine".into(),
        machines: vec![MachineInstance {
            instance_id: "producer".into(),
            machine_name: "M".into(),
            actor: "actor_a".into(),
        }],
        actors: vec![
            ActorSchema {
                name: "actor_a".into(),
                kind: ActorKind::Machine,
            },
            ActorSchema {
                name: "host".into(),
                kind: ActorKind::Owner,
            },
        ],
        handoff_protocols: vec![EffectHandoffProtocol {
            name: "bad_proto".into(),
            producer_instance: "producer".into(),
            effect_variant: "E".into(),
            realizing_actor: "host".into(),
            correlation_fields: vec![],
            allowed_feedback_inputs: vec![FeedbackInputRef {
                machine_instance: "nonexistent".into(),
                input_variant: "Ack".into(),
            }],
            closure_policy: ClosurePolicy::AckRequired,
            liveness_annotation: None,
        }],
        entry_inputs: vec![],
        routes: vec![],
        actor_priorities: vec![],
        scheduler_rules: vec![],
        invariants: vec![],
        witnesses: vec![],
        deep_domain_cardinality: 1,
        deep_domain_overrides: BTreeMap::new(),
        witness_domain_cardinality: 1,
        ci_limits: None,
        closed_world: false,
    };

    assert_eq!(
        composition.validate(),
        Err(CompositionSchemaError::UnknownHandoffFeedbackMachine {
            protocol: "bad_proto".into(),
            machine: "nonexistent".into(),
        })
    );
}

#[test]
fn handoff_protocol_unknown_effect_cross_schema() {
    use meerkat_machine_schema::{
        ActorKind, ActorSchema, ClosurePolicy, CompositionSchema, EffectDisposition,
        EffectDispositionRule, EffectHandoffProtocol, EnumSchema, InitSchema, MachineInstance,
        MachineSchema, RustBinding, StateSchema, VariantSchema,
    };
    use std::collections::BTreeMap;

    let producer_machine = MachineSchema {
        machine: "ProducerMachine".into(),
        version: 1,
        rust: RustBinding {
            crate_name: "test".into(),
            module: "test::producer".into(),
        },
        state: StateSchema {
            phase: EnumSchema {
                name: "Phase".into(),
                variants: vec![VariantSchema {
                    name: "Ready".into(),
                    fields: vec![],
                }],
            },
            fields: vec![],
            init: InitSchema {
                phase: "Ready".into(),
                fields: vec![],
            },
            terminal_phases: vec![],
        },
        inputs: EnumSchema {
            name: "Input".into(),
            variants: vec![],
        },
        effects: EnumSchema {
            name: "Effect".into(),
            variants: vec![VariantSchema {
                name: "RealEffect".into(),
                fields: vec![],
            }],
        },
        helpers: vec![],
        derived: vec![],
        invariants: vec![],
        transitions: vec![],
        effect_dispositions: vec![EffectDispositionRule {
            effect_variant: "RealEffect".into(),
            disposition: EffectDisposition::External,
            handoff_protocol: Some("proto".into()),
        }],
    };

    let composition = CompositionSchema {
        name: "test_unknown_effect".into(),
        machines: vec![MachineInstance {
            instance_id: "producer".into(),
            machine_name: "ProducerMachine".into(),
            actor: "actor_a".into(),
        }],
        actors: vec![
            ActorSchema {
                name: "actor_a".into(),
                kind: ActorKind::Machine,
            },
            ActorSchema {
                name: "host".into(),
                kind: ActorKind::Owner,
            },
        ],
        handoff_protocols: vec![EffectHandoffProtocol {
            name: "proto".into(),
            producer_instance: "producer".into(),
            effect_variant: "NonexistentEffect".into(),
            realizing_actor: "host".into(),
            correlation_fields: vec![],
            allowed_feedback_inputs: vec![],
            closure_policy: ClosurePolicy::AckRequired,
            liveness_annotation: None,
        }],
        entry_inputs: vec![],
        routes: vec![],
        actor_priorities: vec![],
        scheduler_rules: vec![],
        invariants: vec![],
        witnesses: vec![],
        deep_domain_cardinality: 1,
        deep_domain_overrides: BTreeMap::new(),
        witness_domain_cardinality: 1,
        ci_limits: None,
        closed_world: false,
    };

    assert_eq!(
        composition.validate_against(&[&producer_machine]),
        Err(CompositionSchemaError::UnknownHandoffEffect {
            protocol: "proto".into(),
            effect: "NonexistentEffect".into(),
        })
    );
}

#[test]
fn handoff_protocol_terminal_closure_requires_terminal_phases() {
    use meerkat_machine_schema::{
        ActorKind, ActorSchema, ClosurePolicy, CompositionSchema, EffectDisposition,
        EffectDispositionRule, EffectHandoffProtocol, EnumSchema, FieldSchema, InitSchema,
        MachineInstance, MachineSchema, RustBinding, StateSchema, TypeRef, VariantSchema,
    };
    use std::collections::BTreeMap;

    let producer_machine = MachineSchema {
        machine: "ProducerMachine".into(),
        version: 1,
        rust: RustBinding {
            crate_name: "test".into(),
            module: "test::producer".into(),
        },
        state: StateSchema {
            phase: EnumSchema {
                name: "Phase".into(),
                variants: vec![VariantSchema {
                    name: "Running".into(),
                    fields: vec![],
                }],
            },
            fields: vec![],
            init: InitSchema {
                phase: "Running".into(),
                fields: vec![],
            },
            terminal_phases: vec![], // no terminal phases
        },
        inputs: EnumSchema {
            name: "Input".into(),
            variants: vec![],
        },
        effects: EnumSchema {
            name: "Effect".into(),
            variants: vec![VariantSchema {
                name: "Work".into(),
                fields: vec![FieldSchema {
                    name: "id".into(),
                    ty: TypeRef::String,
                }],
            }],
        },
        helpers: vec![],
        derived: vec![],
        invariants: vec![],
        transitions: vec![],
        effect_dispositions: vec![EffectDispositionRule {
            effect_variant: "Work".into(),
            disposition: EffectDisposition::External,
            handoff_protocol: Some("terminal_proto".into()),
        }],
    };

    let composition = CompositionSchema {
        name: "test_terminal_closure".into(),
        machines: vec![MachineInstance {
            instance_id: "producer".into(),
            machine_name: "ProducerMachine".into(),
            actor: "actor_a".into(),
        }],
        actors: vec![
            ActorSchema {
                name: "actor_a".into(),
                kind: ActorKind::Machine,
            },
            ActorSchema {
                name: "host".into(),
                kind: ActorKind::Owner,
            },
        ],
        handoff_protocols: vec![EffectHandoffProtocol {
            name: "terminal_proto".into(),
            producer_instance: "producer".into(),
            effect_variant: "Work".into(),
            realizing_actor: "host".into(),
            correlation_fields: vec!["id".into()],
            allowed_feedback_inputs: vec![],
            closure_policy: ClosurePolicy::TerminalClosure,
            liveness_annotation: None,
        }],
        entry_inputs: vec![],
        routes: vec![],
        actor_priorities: vec![],
        scheduler_rules: vec![],
        invariants: vec![],
        witnesses: vec![],
        deep_domain_cardinality: 1,
        deep_domain_overrides: BTreeMap::new(),
        witness_domain_cardinality: 1,
        ci_limits: None,
        closed_world: false,
    };

    assert_eq!(
        composition.validate_against(&[&producer_machine]),
        Err(
            CompositionSchemaError::TerminalClosureRequiresTerminalPhases {
                protocol: "terminal_proto".into(),
                producer_instance: "producer".into(),
            }
        )
    );
}

#[test]
fn closed_world_rejects_missing_handoff_protocol() {
    use meerkat_machine_schema::{
        ActorKind, ActorSchema, CompositionSchema, EffectDisposition, EffectDispositionRule,
        EnumSchema, InitSchema, MachineInstance, MachineSchema, RustBinding, StateSchema,
        VariantSchema,
    };
    use std::collections::BTreeMap;

    let producer_machine = MachineSchema {
        machine: "ProducerMachine".into(),
        version: 1,
        rust: RustBinding {
            crate_name: "test".into(),
            module: "test::producer".into(),
        },
        state: StateSchema {
            phase: EnumSchema {
                name: "Phase".into(),
                variants: vec![VariantSchema {
                    name: "Ready".into(),
                    fields: vec![],
                }],
            },
            fields: vec![],
            init: InitSchema {
                phase: "Ready".into(),
                fields: vec![],
            },
            terminal_phases: vec![],
        },
        inputs: EnumSchema {
            name: "Input".into(),
            variants: vec![],
        },
        effects: EnumSchema {
            name: "Effect".into(),
            variants: vec![VariantSchema {
                name: "Work".into(),
                fields: vec![],
            }],
        },
        helpers: vec![],
        derived: vec![],
        invariants: vec![],
        transitions: vec![],
        effect_dispositions: vec![EffectDispositionRule {
            effect_variant: "Work".into(),
            disposition: EffectDisposition::External,
            handoff_protocol: Some("missing_proto".into()),
        }],
    };

    let composition = CompositionSchema {
        name: "test_closed_world_missing_handoff".into(),
        machines: vec![MachineInstance {
            instance_id: "producer".into(),
            machine_name: "ProducerMachine".into(),
            actor: "actor_a".into(),
        }],
        actors: vec![ActorSchema {
            name: "actor_a".into(),
            kind: ActorKind::Machine,
        }],
        handoff_protocols: vec![], // deliberately empty — should fail
        entry_inputs: vec![],
        routes: vec![],
        actor_priorities: vec![],
        scheduler_rules: vec![],
        invariants: vec![],
        witnesses: vec![],
        deep_domain_cardinality: 1,
        deep_domain_overrides: BTreeMap::new(),
        witness_domain_cardinality: 1,
        ci_limits: None,
        closed_world: true,
    };

    assert_eq!(
        composition.validate_against(&[&producer_machine]),
        Err(CompositionSchemaError::MissingHandoffProtocol {
            from_instance: "producer".into(),
            effect_variant: "Work".into(),
            expected_protocol: "missing_proto".into(),
        })
    );
}

#[test]
fn closed_world_accepts_handoff_protocol_present() {
    use meerkat_machine_schema::{
        ActorKind, ActorSchema, ClosurePolicy, CompositionSchema, EffectDisposition,
        EffectDispositionRule, EffectHandoffProtocol, EnumSchema, FieldSchema, InitSchema,
        MachineInstance, MachineSchema, RustBinding, StateSchema, TypeRef, VariantSchema,
    };
    use std::collections::BTreeMap;

    let producer_machine = MachineSchema {
        machine: "ProducerMachine".into(),
        version: 1,
        rust: RustBinding {
            crate_name: "test".into(),
            module: "test::producer".into(),
        },
        state: StateSchema {
            phase: EnumSchema {
                name: "Phase".into(),
                variants: vec![
                    VariantSchema {
                        name: "Running".into(),
                        fields: vec![],
                    },
                    VariantSchema {
                        name: "Done".into(),
                        fields: vec![],
                    },
                ],
            },
            fields: vec![],
            init: InitSchema {
                phase: "Running".into(),
                fields: vec![],
            },
            terminal_phases: vec!["Done".into()],
        },
        inputs: EnumSchema {
            name: "Input".into(),
            variants: vec![VariantSchema {
                name: "Ack".into(),
                fields: vec![FieldSchema {
                    name: "op_id".into(),
                    ty: TypeRef::String,
                }],
            }],
        },
        effects: EnumSchema {
            name: "Effect".into(),
            variants: vec![VariantSchema {
                name: "Work".into(),
                fields: vec![FieldSchema {
                    name: "op_id".into(),
                    ty: TypeRef::String,
                }],
            }],
        },
        helpers: vec![],
        derived: vec![],
        invariants: vec![],
        transitions: vec![],
        effect_dispositions: vec![EffectDispositionRule {
            effect_variant: "Work".into(),
            disposition: EffectDisposition::External,
            handoff_protocol: Some("work_proto".into()),
        }],
    };

    let composition = CompositionSchema {
        name: "test_closed_world_handoff_ok".into(),
        machines: vec![MachineInstance {
            instance_id: "producer".into(),
            machine_name: "ProducerMachine".into(),
            actor: "actor_a".into(),
        }],
        actors: vec![
            ActorSchema {
                name: "actor_a".into(),
                kind: ActorKind::Machine,
            },
            ActorSchema {
                name: "host".into(),
                kind: ActorKind::Owner,
            },
        ],
        handoff_protocols: vec![EffectHandoffProtocol {
            name: "work_proto".into(),
            producer_instance: "producer".into(),
            effect_variant: "Work".into(),
            realizing_actor: "host".into(),
            correlation_fields: vec!["op_id".into()],
            allowed_feedback_inputs: vec![meerkat_machine_schema::FeedbackInputRef {
                machine_instance: "producer".into(),
                input_variant: "Ack".into(),
            }],
            closure_policy: ClosurePolicy::AckRequired,
            liveness_annotation: None,
        }],
        entry_inputs: vec![],
        routes: vec![],
        actor_priorities: vec![],
        scheduler_rules: vec![],
        invariants: vec![],
        witnesses: vec![],
        deep_domain_cardinality: 1,
        deep_domain_overrides: BTreeMap::new(),
        witness_domain_cardinality: 1,
        ci_limits: None,
        closed_world: true,
    };

    composition
        .validate_against(&[&producer_machine])
        .expect("closed-world with matching handoff protocol should pass");
}
