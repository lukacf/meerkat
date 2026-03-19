use std::collections::BTreeMap;

use meerkat_machine_schema::catalog::{
    flow_run_machine, mob_bundle_composition, mob_lifecycle_machine, mob_orchestrator_machine,
    ops_lifecycle_machine, ops_runtime_bundle_composition, peer_comms_machine,
    peer_runtime_bundle_composition, runtime_control_machine, runtime_ingress_machine,
    runtime_pipeline_composition, turn_execution_machine,
};
use meerkat_machine_schema::{
    CompositionSchemaError, Route, RouteBindingSource, RouteDelivery, RouteFieldBinding,
    RouteTarget, canonical_machine_schemas, input_lifecycle_machine,
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
