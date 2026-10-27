#![allow(clippy::expect_used, clippy::panic)]

//! Rendering a composition against a caller-supplied machine catalog: the
//! canonical catalog renders byte-identically through the new entry points,
//! an external machine renders, and every way a supplied catalog can go wrong
//! is a typed refusal rather than a panic.

use meerkat_machine_codegen::{
    CompositionTlaError, render_composition_ci_cfg, render_composition_ci_cfg_with_catalog,
    render_composition_driver, render_composition_driver_with_catalog,
    render_composition_semantic_model, render_composition_semantic_model_with_catalog,
    render_composition_witness_cfg, render_composition_witness_cfg_with_catalog,
    render_machine_semantic_model,
};
use meerkat_machine_schema::catalog::dsl::{
    dsl_work_attention_lifecycle_machine, dsl_workgraph_lifecycle_machine,
};
use meerkat_machine_schema::catalog::{
    canonical_composition_schemas, canonical_machine_schemas,
    workgraph_attention_bundle_composition,
};
use meerkat_machine_schema::identity::MachineId;
use meerkat_machine_schema::{CompositionSchema, MachineSchema, RustTypeAtom};

fn renamed(mut machine: MachineSchema, id: &str) -> MachineSchema {
    machine.machine = MachineId::parse(id).expect("machine slug");
    machine
}

const EXTERNAL_ATTENTION: &str = "ExternalAttentionMachine";

/// workgraph_attention_bundle with its attention instance pointed at an
/// external machine id. The supplied catalog is derived from the
/// composition's own instances (canonical machines for every other instance,
/// the renamed attention machine for the external one), so a machine added to
/// the composition later is picked up instead of failing as unknown.
fn external_attention_bundle() -> (CompositionSchema, Vec<MachineSchema>) {
    let mut composition = workgraph_attention_bundle_composition();
    let attention = dsl_work_attention_lifecycle_machine();
    for instance in &mut composition.machines {
        if instance.machine_name == attention.machine {
            instance.machine_name = MachineId::parse(EXTERNAL_ATTENTION).expect("machine slug");
        }
    }
    let canonical = canonical_machine_schemas();
    let mut catalog: Vec<MachineSchema> = Vec::new();
    for instance in &composition.machines {
        if catalog
            .iter()
            .any(|machine| machine.machine == instance.machine_name)
        {
            continue;
        }
        let machine = if instance.machine_name.as_str() == EXTERNAL_ATTENTION {
            renamed(attention.clone(), EXTERNAL_ATTENTION)
        } else {
            canonical
                .iter()
                .find(|machine| machine.machine == instance.machine_name)
                .cloned()
                .expect("canonical machine for a composition instance")
        };
        catalog.push(machine);
    }
    (composition, catalog)
}

fn machine_mut<'a>(catalog: &'a mut [MachineSchema], id: &str) -> &'a mut MachineSchema {
    catalog
        .iter_mut()
        .find(|machine| machine.machine.as_str() == id)
        .expect("machine in the supplied catalog")
}

#[test]
fn the_canonical_catalog_renders_byte_identically_through_the_catalog_entry_points() {
    let catalog = canonical_machine_schemas();
    // The two mob compositions render very large artifacts; machine-check-drift
    // renders them through the canonical entry points, which share the
    // catalog-parameterized implementation exercised here.
    for composition in canonical_composition_schemas()
        .into_iter()
        .filter(|composition| {
            !matches!(
                composition.name.as_str(),
                "meerkat_mob_seam" | "adaptive_mob_bundle"
            )
        })
    {
        assert_eq!(
            render_composition_semantic_model_with_catalog(&composition, &catalog)
                .expect("canonical model"),
            render_composition_semantic_model(&composition).expect("canonical model"),
            "{}",
            composition.name
        );
        for deep in [false, true] {
            assert_eq!(
                render_composition_ci_cfg_with_catalog(&composition, deep, &catalog)
                    .expect("canonical cfg"),
                render_composition_ci_cfg(&composition, deep),
                "{} deep={deep}",
                composition.name
            );
        }
        for witness in &composition.witnesses {
            assert_eq!(
                render_composition_witness_cfg_with_catalog(&composition, witness, &catalog)
                    .expect("canonical witness cfg"),
                render_composition_witness_cfg(&composition, witness),
                "{} witness {}",
                composition.name,
                witness.name
            );
        }
        assert_eq!(
            render_composition_driver_with_catalog(&composition, &catalog)
                .expect("canonical driver"),
            render_composition_driver(&composition),
            "{}",
            composition.name
        );
    }
}

#[test]
fn an_external_machine_renders_only_through_its_own_catalog() {
    let (composition, catalog) = external_attention_bundle();
    let model = render_composition_semantic_model_with_catalog(&composition, &catalog)
        .expect("external catalog renders");
    assert!(model.contains("attention_phase"));
    assert!(
        render_composition_semantic_model(&composition).is_err(),
        "the built-in catalog has no ExternalAttentionMachine"
    );
}

#[test]
fn a_supplied_machine_may_not_shadow_a_canonical_machine() {
    let mut composition = workgraph_attention_bundle_composition();
    composition.witnesses.clear();
    let mut shadow = dsl_work_attention_lifecycle_machine();
    shadow.ci_step_limit = Some(3);
    let catalog = vec![dsl_workgraph_lifecycle_machine(), shadow.clone()];
    match render_composition_semantic_model_with_catalog(&composition, &catalog) {
        Err(CompositionTlaError::ShadowsCanonicalMachine { machine }) => {
            assert_eq!(machine, shadow.machine.as_str());
        }
        other => panic!("expected ShadowsCanonicalMachine, got {other:?}"),
    }
}

#[test]
fn a_supplied_catalog_refuses_duplicate_and_invalid_machines() {
    let (composition, mut catalog) = external_attention_bundle();
    let duplicate = machine_mut(&mut catalog, EXTERNAL_ATTENTION).clone();
    catalog.push(duplicate);
    assert!(matches!(
        render_composition_semantic_model_with_catalog(&composition, &catalog),
        Err(CompositionTlaError::DuplicateSuppliedMachine { machine }) if machine == "ExternalAttentionMachine"
    ));

    let (composition, mut catalog) = external_attention_bundle();
    let external = machine_mut(&mut catalog, EXTERNAL_ATTENTION);
    let first = external.transitions[0].clone();
    external.transitions.push(first);
    assert!(matches!(
        render_composition_semantic_model_with_catalog(&composition, &catalog),
        Err(CompositionTlaError::InvalidSuppliedMachine { machine, .. }) if machine == "ExternalAttentionMachine"
    ));
}

#[test]
fn composition_machines_must_agree_on_shared_named_types() {
    let (composition, mut catalog) = external_attention_bundle();
    let workgraph = dsl_workgraph_lifecycle_machine().machine;
    let shared = machine_mut(&mut catalog, workgraph.as_str())
        .named_types
        .iter()
        .find(|binding| matches!(binding.rust, RustTypeAtom::String))
        .cloned()
        .expect("a string-bound WorkGraph named type");
    let mut divergent = shared.clone();
    divergent.rust = RustTypeAtom::U64;
    let external = machine_mut(&mut catalog, EXTERNAL_ATTENTION);
    external
        .named_types
        .retain(|binding| binding.name != shared.name);
    external.named_types.push(divergent);
    assert!(matches!(
        render_composition_semantic_model_with_catalog(&composition, &catalog),
        Err(CompositionTlaError::DivergentNamedTypeBinding { named_type, .. }) if named_type == shared.name.as_str()
    ));
}

/// A machine that keeps a canonical id but rebinds a canonical named type is
/// a typed refusal from the fallible machine renderer, not a panic.
#[test]
fn rebinding_a_canonical_named_type_is_a_typed_refusal() {
    let mut machine = dsl_workgraph_lifecycle_machine();
    let binding = machine
        .named_types
        .iter_mut()
        .find(|binding| matches!(binding.rust, RustTypeAtom::String))
        .expect("a string-bound named type");
    let name = binding.name.as_str().to_owned();
    binding.rust = RustTypeAtom::U64;
    match render_machine_semantic_model(&machine) {
        Err(CompositionTlaError::CanonicalNamedTypeMismatch { named_type, .. }) => {
            assert_eq!(named_type, name);
        }
        other => panic!("expected CanonicalNamedTypeMismatch, got {other:?}"),
    }
}
