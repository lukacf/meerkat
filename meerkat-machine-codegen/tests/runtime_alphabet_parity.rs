#![allow(clippy::redundant_closure_for_method_calls)]

use meerkat_machine_schema::TriggerKind;
use meerkat_machine_schema::catalog::dsl::{
    MEERKAT_MACHINE_DSL_INTERNAL_INPUTS, MOB_MACHINE_DSL_INTERNAL_INPUTS,
    dsl_meerkat_machine as meerkat_machine, dsl_mob_machine as mob_machine,
};
use meerkat_mob::canonical_mob_machine_command_manifest;
use meerkat_runtime::canonical_meerkat_machine_command_manifest;
use std::collections::BTreeSet;

fn variant_names<'a>(
    variants: impl IntoIterator<Item = &'a meerkat_machine_schema::VariantSchema>,
) -> BTreeSet<&'a str> {
    variants
        .into_iter()
        .map(|variant| variant.name.as_str())
        .collect()
}

fn assert_runtime_manifest_matches_schema(
    machine: &str,
    schema_inputs: &BTreeSet<&str>,
    dsl_internal_inputs: &BTreeSet<&str>,
    runtime_commands: &BTreeSet<&str>,
) {
    assert!(
        dsl_internal_inputs.is_subset(schema_inputs),
        "{machine} DSL-internal input allowlist contains variants absent from the schema: {:?}",
        dsl_internal_inputs
            .difference(schema_inputs)
            .collect::<Vec<_>>()
    );

    assert!(
        runtime_commands.is_subset(schema_inputs),
        "{machine} runtime command manifest contains variants absent from the schema: {:?}",
        runtime_commands
            .difference(schema_inputs)
            .collect::<Vec<_>>()
    );

    let unexpected_schema_inputs = schema_inputs
        .difference(runtime_commands)
        .copied()
        .filter(|input| !dsl_internal_inputs.contains(input))
        .collect::<Vec<_>>();
    assert!(
        unexpected_schema_inputs.is_empty(),
        "{machine} schema inputs are missing from the runtime command manifest and are not declared DSL-internal inputs: {unexpected_schema_inputs:?}"
    );
}

#[test]
fn meerkat_machine_inputs_equal_runtime_manifest_exactly() {
    let schema = meerkat_machine();
    let schema_inputs: BTreeSet<&str> = variant_names(&schema.inputs.variants);
    let dsl_internal_inputs: BTreeSet<&str> = MEERKAT_MACHINE_DSL_INTERNAL_INPUTS
        .iter()
        .copied()
        .collect();
    let runtime_commands: BTreeSet<&str> = canonical_meerkat_machine_command_manifest()
        .into_iter()
        .filter(|name| *name != "RuntimeRealtimeChannelStatus")
        .collect();

    assert_runtime_manifest_matches_schema(
        "MeerkatMachine",
        &schema_inputs,
        &dsl_internal_inputs,
        &runtime_commands,
    );
}

#[test]
fn mob_machine_inputs_equal_runtime_manifest_exactly() {
    let schema = mob_machine();
    let schema_inputs: BTreeSet<&str> = variant_names(&schema.inputs.variants);
    let dsl_internal_inputs: BTreeSet<&str> =
        MOB_MACHINE_DSL_INTERNAL_INPUTS.iter().copied().collect();
    let runtime_commands: BTreeSet<&str> = canonical_mob_machine_command_manifest()
        .into_iter()
        .collect();

    assert_runtime_manifest_matches_schema(
        "MobMachine",
        &schema_inputs,
        &dsl_internal_inputs,
        &runtime_commands,
    );
}

#[test]
fn every_canonical_input_has_transition_coverage() {
    for schema in [meerkat_machine(), mob_machine()] {
        let surface_only_inputs: BTreeSet<&str> = schema
            .surface_only_inputs
            .iter()
            .map(|v| v.as_str())
            .collect();
        let covered: BTreeSet<&str> = schema
            .transitions
            .iter()
            .filter(|transition| transition.on.kind() == TriggerKind::Input)
            .map(|transition| transition.on.variant_str())
            .collect();

        for input in &schema.inputs.variants {
            if surface_only_inputs.contains(input.name.as_str()) {
                continue;
            }
            assert!(
                covered.contains(input.name.as_str()),
                "{} input `{}` has no transition coverage",
                schema.machine,
                input.name
            );
        }
    }
}

#[test]
fn every_canonical_signal_has_transition_coverage() {
    for schema in [meerkat_machine(), mob_machine()] {
        let covered: BTreeSet<&str> = schema
            .transitions
            .iter()
            .filter(|transition| transition.on.kind() == TriggerKind::Signal)
            .map(|transition| transition.on.variant_str())
            .collect();

        for signal in &schema.signals.variants {
            assert!(
                covered.contains(signal.name.as_str()),
                "{} signal `{}` has no transition coverage",
                schema.machine,
                signal.name
            );
        }
    }
}
