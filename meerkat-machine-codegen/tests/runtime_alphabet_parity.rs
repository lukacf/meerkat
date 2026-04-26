#![allow(clippy::redundant_closure_for_method_calls)]

use meerkat_machine_schema::TriggerKind;
use meerkat_machine_schema::catalog::dsl::{
    dsl_meerkat_machine as meerkat_machine, dsl_mob_machine as mob_machine,
};
use meerkat_machine_schema::identity::{IdentityError, InputVariantId, SignalVariantId};
use meerkat_mob::canonical_mob_machine_command_manifest;
use meerkat_runtime::canonical_meerkat_machine_command_manifest;
use std::collections::BTreeSet;

fn variant_ids<'a>(
    variants: impl IntoIterator<Item = &'a meerkat_machine_schema::VariantSchema>,
) -> Result<BTreeSet<InputVariantId>, IdentityError> {
    variants
        .into_iter()
        .map(|variant| InputVariantId::parse(variant.name.as_str()))
        .collect()
}

fn command_ids(
    commands: impl IntoIterator<Item = &'static str>,
) -> Result<BTreeSet<InputVariantId>, IdentityError> {
    commands.into_iter().map(InputVariantId::parse).collect()
}

fn assert_runtime_manifest_matches_schema(
    machine: &str,
    schema_inputs: &BTreeSet<InputVariantId>,
    dsl_internal_inputs: &BTreeSet<InputVariantId>,
    runtime_commands: &BTreeSet<InputVariantId>,
) {
    assert!(
        dsl_internal_inputs.is_subset(schema_inputs),
        "{machine} DSL-internal input declarations contain variants absent from the schema: {:?}",
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
        .filter(|input| !dsl_internal_inputs.contains(input))
        .collect::<Vec<_>>();
    assert!(
        unexpected_schema_inputs.is_empty(),
        "{machine} schema inputs are missing from the runtime command manifest and are not declared DSL-internal inputs: {unexpected_schema_inputs:?}"
    );
}

#[test]
fn meerkat_machine_inputs_equal_runtime_manifest_exactly() -> Result<(), IdentityError> {
    let schema = meerkat_machine();
    let schema_inputs = variant_ids(&schema.inputs.variants)?;
    let dsl_internal_inputs: BTreeSet<InputVariantId> =
        schema.runtime_internal_inputs.iter().cloned().collect();
    let runtime_commands = command_ids(canonical_meerkat_machine_command_manifest())?;

    assert_runtime_manifest_matches_schema(
        "MeerkatMachine",
        &schema_inputs,
        &dsl_internal_inputs,
        &runtime_commands,
    );
    Ok(())
}

#[test]
fn mob_machine_inputs_equal_runtime_manifest_exactly() -> Result<(), IdentityError> {
    let schema = mob_machine();
    let schema_inputs = variant_ids(&schema.inputs.variants)?;
    let dsl_internal_inputs: BTreeSet<InputVariantId> =
        schema.runtime_internal_inputs.iter().cloned().collect();
    let runtime_commands = command_ids(canonical_mob_machine_command_manifest())?;

    assert_runtime_manifest_matches_schema(
        "MobMachine",
        &schema_inputs,
        &dsl_internal_inputs,
        &runtime_commands,
    );
    Ok(())
}

#[test]
fn every_canonical_input_has_transition_coverage() -> Result<(), IdentityError> {
    for schema in [meerkat_machine(), mob_machine()] {
        let surface_only_inputs: BTreeSet<InputVariantId> =
            schema.surface_only_inputs.iter().cloned().collect();
        let covered: BTreeSet<InputVariantId> = schema
            .transitions
            .iter()
            .filter(|transition| transition.on.kind() == TriggerKind::Input)
            .map(|transition| InputVariantId::parse(transition.on.variant_str()))
            .collect::<Result<_, _>>()?;

        for input in &schema.inputs.variants {
            let input_id = InputVariantId::parse(input.name.as_str())?;
            if surface_only_inputs.contains(&input_id) {
                continue;
            }
            assert!(
                covered.contains(&input_id),
                "{} input `{}` has no transition coverage",
                schema.machine,
                input.name
            );
        }
    }
    Ok(())
}

#[test]
fn every_canonical_signal_has_transition_coverage() -> Result<(), IdentityError> {
    for schema in [meerkat_machine(), mob_machine()] {
        let covered: BTreeSet<SignalVariantId> = schema
            .transitions
            .iter()
            .filter(|transition| transition.on.kind() == TriggerKind::Signal)
            .map(|transition| SignalVariantId::parse(transition.on.variant_str()))
            .collect::<Result<_, _>>()?;

        for signal in &schema.signals.variants {
            let signal_id = SignalVariantId::parse(signal.name.as_str())?;
            assert!(
                covered.contains(&signal_id),
                "{} signal `{}` has no transition coverage",
                schema.machine,
                signal.name
            );
        }
    }
    Ok(())
}
