use meerkat_machine_schema::TriggerKind;
use meerkat_machine_schema::catalog::dsl::{
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

#[test]
fn meerkat_machine_inputs_equal_runtime_manifest_exactly() {
    let schema = meerkat_machine();
    let actual = variant_names(&schema.inputs.variants);
    let expected: BTreeSet<&str> = canonical_meerkat_machine_command_manifest()
        .into_iter()
        .collect();

    assert_eq!(actual, expected);
}

#[test]
fn mob_machine_inputs_equal_runtime_manifest_exactly() {
    let schema = mob_machine();
    let actual = variant_names(&schema.inputs.variants);
    let expected: BTreeSet<&str> = canonical_mob_machine_command_manifest()
        .into_iter()
        .collect();

    assert_eq!(actual, expected);
}

#[test]
fn every_canonical_input_has_transition_coverage() {
    for schema in [meerkat_machine(), mob_machine()] {
        let surface_only_inputs: BTreeSet<&str> = schema
            .surface_only_inputs
            .iter()
            .map(String::as_str)
            .collect();
        let covered: BTreeSet<&str> = schema
            .transitions
            .iter()
            .filter(|transition| transition.on.kind == TriggerKind::Input)
            .map(|transition| transition.on.variant.as_str())
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
            .filter(|transition| transition.on.kind == TriggerKind::Signal)
            .map(|transition| transition.on.variant.as_str())
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
