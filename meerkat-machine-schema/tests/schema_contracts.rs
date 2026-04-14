use std::collections::BTreeMap;

use meerkat_machine_schema::{
    CompositionSchemaError, canonical_composition_coverage_manifests,
    canonical_composition_schemas, canonical_machine_coverage_manifests, canonical_machine_schemas,
    meerkat_machine, meerkat_mob_seam_composition, mob_machine, occurrence_lifecycle_machine,
    schedule_lifecycle_machine,
};

#[test]
fn canonical_machine_registry_contains_only_two_kernel_and_perimeter_entries() {
    let names = canonical_machine_schemas()
        .into_iter()
        .map(|schema| schema.machine)
        .collect::<Vec<_>>();

    assert_eq!(
        names,
        vec![
            "MeerkatMachine",
            "MobMachine",
            "PeerDirectoryReachabilityMachine",
            "ScheduleLifecycleMachine",
            "OccurrenceLifecycleMachine",
            "SessionToolVisibilityMachine",
            "SessionTurnAdmissionMachine",
        ]
    );
}

#[test]
fn canonical_composition_registry_contains_kernel_seam_and_schedule_perimeter_entries() {
    let names = canonical_composition_schemas()
        .into_iter()
        .map(|schema| schema.name)
        .collect::<Vec<_>>();

    assert_eq!(
        names,
        vec![
            "meerkat_mob_seam",
            "schedule_bundle",
            "schedule_runtime_bundle",
            "schedule_mob_bundle",
        ]
    );
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
fn canonical_composition_registry_is_individually_valid() {
    let canonical_machines = canonical_machine_schemas();
    let canonical_machine_refs = canonical_machines.iter().collect::<Vec<_>>();

    for schema in canonical_composition_schemas() {
        assert_eq!(
            schema.validate_against(&canonical_machine_refs),
            Ok(()),
            "composition {} should validate against the canonical machine set",
            schema.name
        );
    }
}

#[test]
fn kernel_seam_rejects_type_mismatched_route_binding() {
    let meerkat = meerkat_machine();
    let mob = mob_machine();
    let mut composition = meerkat_mob_seam_composition();
    let route_idx = composition
        .routes
        .iter()
        .position(|route| route.name == "binding_request_reaches_meerkat");
    assert!(route_idx.is_some(), "binding request route");
    let Some(route_idx) = route_idx else {
        return;
    };
    let route = &mut composition.routes[route_idx];
    let generation_binding_idx = route
        .bindings
        .iter()
        .position(|binding| binding.to_field == "generation");
    assert!(generation_binding_idx.is_some(), "generation binding");
    let Some(generation_binding_idx) = generation_binding_idx else {
        return;
    };
    let generation_binding = &mut route.bindings[generation_binding_idx];
    generation_binding.source = meerkat_machine_schema::RouteBindingSource::Field {
        from_field: "fence_token".into(),
        allow_named_alias: false,
    };

    let result = composition.validate_against(&[&meerkat, &mob]);
    assert!(matches!(
        result,
        Err(CompositionSchemaError::RouteFieldTypeMismatch { .. }
            | CompositionSchemaError::MachineSchema(_))
    ));
}

#[test]
fn kernel_seam_rejects_zero_named_domain_override() {
    let mut composition = meerkat_mob_seam_composition();
    composition.deep_domain_overrides = BTreeMap::from([("WorkIdValues".into(), 0)]);

    let result = composition.validate();
    assert!(matches!(
        result,
        Err(CompositionSchemaError::InvalidNamedDomainCardinality { .. })
    ));
}

#[test]
fn schedule_and_occurrence_machines_stay_in_canonical_coverage_manifests() {
    let machine_names = canonical_machine_schemas()
        .into_iter()
        .map(|schema| schema.machine)
        .collect::<Vec<_>>();
    let coverage_names = canonical_machine_coverage_manifests()
        .into_iter()
        .map(|manifest| manifest.machine)
        .collect::<Vec<_>>();

    for name in [
        schedule_lifecycle_machine().machine,
        occurrence_lifecycle_machine().machine,
    ] {
        assert!(
            machine_names.iter().any(|machine| machine == &name),
            "{name} should remain canonical"
        );
        assert!(
            coverage_names.iter().any(|machine| machine == &name),
            "{name} should retain coverage metadata"
        );
    }
}

#[test]
fn kernel_seam_retains_coverage_metadata() {
    let coverage_names = canonical_composition_coverage_manifests()
        .into_iter()
        .map(|manifest| manifest.composition)
        .collect::<Vec<_>>();

    assert_eq!(
        coverage_names,
        vec![
            "meerkat_mob_seam",
            "schedule_bundle",
            "schedule_runtime_bundle",
            "schedule_mob_bundle",
        ]
    );
}
