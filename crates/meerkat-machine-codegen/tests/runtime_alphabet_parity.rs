#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::redundant_closure_for_method_calls
)]

use meerkat_machine_schema::catalog::dsl::meerkat_machine::{
    MeerkatMachineInput, MeerkatMachineInputVariant,
};
use meerkat_machine_schema::catalog::dsl::mob_machine::{MobMachineInput, MobMachineInputVariant};
use meerkat_machine_schema::catalog::dsl::{
    dsl_meerkat_machine as meerkat_machine, dsl_mob_machine as mob_machine,
    meerkat_machine_runtime_internal_input_variants, mob_machine_runtime_internal_input_variants,
};
use meerkat_machine_schema::identity::{IdentityError, InputVariantId, SignalVariantId};
use meerkat_mob::{
    MobMachineCatalogInput as MobInput, MobMachineCommandClassification,
    MobMachineCommandClassificationRecord, canonical_mob_machine_command_classifications,
    canonical_mob_machine_command_input_variant_manifest, canonical_mob_machine_command_manifest,
};
use meerkat_runtime::{
    MeerkatMachineCatalogInput as MeerkatInput, MeerkatMachineCommandClassification,
    MeerkatMachineCommandClassificationRecord, MeerkatMachineFieldlessRuntimeInternalInput,
    canonical_meerkat_machine_command_classifications,
    canonical_meerkat_machine_command_input_variant_manifest,
    canonical_meerkat_machine_command_manifest,
    canonical_meerkat_machine_runtime_internal_fieldless_input_variant_manifest,
    canonical_meerkat_machine_runtime_internal_input_variant_manifest,
    canonical_meerkat_machine_runtime_internal_manifest,
};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn generated_meerkat_input_variants() -> BTreeSet<MeerkatMachineInputVariant> {
    MeerkatMachineInput::variant_manifest()
        .iter()
        .copied()
        .collect()
}

fn generated_mob_input_variants() -> BTreeSet<MobMachineInputVariant> {
    MobMachineInput::variant_manifest()
        .iter()
        .copied()
        .collect()
}

fn meerkat_runtime_command_input_variants(
    records: &[MeerkatMachineCommandClassificationRecord],
) -> BTreeSet<MeerkatMachineInputVariant> {
    records
        .iter()
        .flat_map(|record| record.classification.catalog_input_variants())
        .collect()
}

fn mob_runtime_command_input_variants(
    records: &[MobMachineCommandClassificationRecord],
) -> BTreeSet<MobMachineInputVariant> {
    records
        .iter()
        .flat_map(|record| record.classification.catalog_input_variants())
        .collect()
}

fn assert_meerkat_command_records_are_identity_checked(
    generated_inputs: &BTreeSet<MeerkatMachineInputVariant>,
    records: &[MeerkatMachineCommandClassificationRecord],
) {
    for record in records {
        let command_name = record.command.as_str();
        let command_input = record
            .command
            .catalog_input()
            .map(MeerkatInput::input_variant);
        let catalog_inputs = record.classification.catalog_inputs();
        let catalog_input_variants = catalog_inputs
            .iter()
            .copied()
            .map(meerkat_runtime::MeerkatMachineCatalogInput::input_variant)
            .collect::<BTreeSet<_>>();

        assert_eq!(
            catalog_input_variants.len(),
            catalog_inputs.len(),
            "MeerkatMachine command `{command_name}` must not duplicate catalog input classifications"
        );
        assert!(
            catalog_input_variants.is_subset(generated_inputs),
            "MeerkatMachine command `{command_name}` classifies to inputs absent from the generated input alphabet: {:?}",
            catalog_input_variants
                .difference(generated_inputs)
                .collect::<Vec<_>>()
        );

        match record.classification {
            MeerkatMachineCommandClassification::CatalogInput(input) => {
                if let Some(command_input) = command_input {
                    assert_eq!(
                        input.input_variant(),
                        command_input,
                        "MeerkatMachine command `{command_name}` is itself a catalog input and must classify to that exact typed input"
                    );
                }
                assert!(
                    generated_inputs.contains(&input.input_variant()),
                    "MeerkatMachine command `{command_name}` classifies to an input absent from the generated input alphabet"
                );
            }
            MeerkatMachineCommandClassification::CatalogInputs(inputs) => {
                assert!(
                    !inputs.is_empty(),
                    "MeerkatMachine command `{command_name}` must not use an empty typed catalog classification"
                );
                if let Some(command_input) = command_input {
                    assert!(
                        catalog_input_variants.contains(&command_input),
                        "MeerkatMachine command `{command_name}` is itself a catalog input and must include that exact typed input"
                    );
                }
            }
            MeerkatMachineCommandClassification::ShellMechanic(_) => {
                assert!(
                    command_input.is_none(),
                    "MeerkatMachine shell-mechanic command `{command_name}` must not bypass a catalog input"
                );
            }
        }
    }
}

fn assert_mob_command_records_are_identity_checked(
    generated_inputs: &BTreeSet<MobMachineInputVariant>,
    records: &[MobMachineCommandClassificationRecord],
) {
    for record in records {
        let command_name = record.command.as_str();
        let command_input = record.command.catalog_input().map(MobInput::input_variant);
        let catalog_inputs = record.classification.catalog_inputs();
        let catalog_input_variants = catalog_inputs
            .iter()
            .copied()
            .map(MobInput::input_variant)
            .collect::<BTreeSet<_>>();

        assert_eq!(
            catalog_input_variants.len(),
            catalog_inputs.len(),
            "MobMachine command `{command_name}` must not duplicate catalog input classifications"
        );
        assert!(
            catalog_input_variants.is_subset(generated_inputs),
            "MobMachine command `{command_name}` classifies to inputs absent from the generated input alphabet: {:?}",
            catalog_input_variants
                .difference(generated_inputs)
                .collect::<Vec<_>>()
        );

        match record.classification {
            MobMachineCommandClassification::CatalogInput(input) => {
                if let Some(command_input) = command_input {
                    assert_eq!(
                        input.input_variant(),
                        command_input,
                        "MobMachine command `{command_name}` is itself a catalog input and must classify to that exact typed input"
                    );
                }
                assert!(
                    generated_inputs.contains(&input.input_variant()),
                    "MobMachine command `{command_name}` classifies to an input absent from the generated input alphabet"
                );
            }
            MobMachineCommandClassification::CatalogInputs(inputs) => {
                assert!(
                    !inputs.is_empty(),
                    "MobMachine command `{command_name}` must not use an empty typed catalog classification"
                );
                if let Some(command_input) = command_input {
                    assert!(
                        catalog_input_variants.contains(&command_input),
                        "MobMachine command `{command_name}` is itself a catalog input and must include that exact typed input"
                    );
                }
            }
            MobMachineCommandClassification::ShellMechanic(_) => {
                assert!(
                    command_input.is_none(),
                    "MobMachine shell-mechanic command `{command_name}` must not bypass a catalog input"
                );
            }
        }
    }
}

fn assert_all_meerkat_catalog_inputs_are_identity_checked(
    generated_inputs: &BTreeSet<MeerkatMachineInputVariant>,
    runtime_commands: &BTreeSet<MeerkatMachineInputVariant>,
) {
    let all_input_variants = MeerkatInput::ALL
        .iter()
        .copied()
        .map(MeerkatInput::input_variant)
        .collect::<BTreeSet<_>>();

    assert!(
        all_input_variants.is_subset(generated_inputs),
        "MeerkatMachineCatalogInput::ALL must name only generated catalog input variants: {:?}",
        all_input_variants
            .difference(generated_inputs)
            .collect::<Vec<_>>()
    );

    for input in MeerkatInput::ALL {
        assert!(
            generated_inputs.contains(&input.input_variant()),
            "typed MeerkatMachine catalog input {input:?} must map to its exact generated input variant"
        );
    }

    assert_eq!(
        all_input_variants, *runtime_commands,
        "MeerkatMachineCatalogInput::ALL must exactly mirror the runtime command-backed input alphabet"
    );
}

fn assert_all_mob_catalog_inputs_are_identity_checked(
    generated_inputs: &BTreeSet<MobMachineInputVariant>,
) {
    let all_input_variants = MobInput::ALL
        .iter()
        .copied()
        .map(MobInput::input_variant)
        .collect::<BTreeSet<_>>();

    assert_eq!(
        all_input_variants, *generated_inputs,
        "MobMachineCatalogInput::ALL must exactly mirror the generated catalog input alphabet"
    );

    for input in MobInput::ALL {
        assert!(
            generated_inputs.contains(&input.input_variant()),
            "typed MobMachine catalog input {input:?} must map to its exact generated input variant"
        );
    }
}

fn assert_typed_runtime_manifest_matches_generated_inputs<T>(
    machine: &str,
    generated_inputs: &BTreeSet<T>,
    dsl_internal_inputs: &BTreeSet<T>,
    runtime_commands: &BTreeSet<T>,
) where
    T: Copy + Ord + std::fmt::Debug,
{
    assert!(
        dsl_internal_inputs.is_subset(generated_inputs),
        "{machine} DSL-internal typed input declarations contain variants absent from the generated input alphabet: {:?}",
        dsl_internal_inputs
            .difference(generated_inputs)
            .copied()
            .collect::<Vec<_>>()
    );

    assert!(
        runtime_commands.is_subset(generated_inputs),
        "{machine} runtime typed command manifest contains variants absent from the generated input alphabet: {:?}",
        runtime_commands
            .difference(generated_inputs)
            .copied()
            .collect::<Vec<_>>()
    );

    let unexpected_generated_inputs = generated_inputs
        .difference(runtime_commands)
        .filter(|input| !dsl_internal_inputs.contains(input))
        .copied()
        .collect::<Vec<_>>();
    assert!(
        unexpected_generated_inputs.is_empty(),
        "{machine} generated input variants are missing from the runtime typed command manifest and are not declared DSL-internal inputs: {unexpected_generated_inputs:?}"
    );
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("repo root")
        .to_path_buf()
}

fn function_body(source: &str, start_marker: &str, _end_marker: &str) -> String {
    let start = source
        .find(start_marker)
        .unwrap_or_else(|| panic!("missing start marker `{start_marker}`"));
    let rest = &source[start..];
    let body_start = rest
        .find('{')
        .unwrap_or_else(|| panic!("missing classifier body for `{start_marker}`"));
    let mut depth = 0usize;
    for (offset, ch) in rest[body_start..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth = depth
                    .checked_sub(1)
                    .unwrap_or_else(|| panic!("brace underflow in `{start_marker}`"));
                if depth == 0 {
                    return rest[body_start..body_start + offset + ch.len_utf8()].to_string();
                }
            }
            _ => {}
        }
    }
    panic!("missing classifier body end for `{start_marker}`")
}

fn is_identifier_pattern(pattern: &str) -> bool {
    let mut pattern = pattern
        .split(" if ")
        .next()
        .unwrap_or(pattern)
        .trim()
        .trim_end_matches('{')
        .trim();
    if let Some(stripped) = pattern.strip_prefix("mut ") {
        pattern = stripped.trim();
    }
    if let Some(stripped) = pattern.strip_prefix("ref ") {
        pattern = stripped.trim();
    }
    let mut chars = pattern.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn assert_body_has_no_catch_all_match_arms(path: &str, body: &str, context: &str) {
    for line in body.lines() {
        let Some((pattern, _arm_body)) = line.split_once("=>") else {
            continue;
        };
        let pattern = pattern.trim().trim_start_matches('|').trim();
        assert!(
            pattern != "_" && !is_identifier_pattern(pattern),
            "{path} {context} must enumerate variants without wildcard or named catch-all arm `{pattern} =>`"
        );
    }
}

fn assert_classifier_body_uses_typed_variants(path: &str, start_marker: &str, end_marker: &str) {
    let source = std::fs::read_to_string(repo_root().join(path)).expect("read classifier source");
    let body = function_body(&source, start_marker, end_marker);
    assert!(
        body.contains("match variant"),
        "{path} command classifier must dispatch on typed command variants"
    );
    assert!(
        !body.contains(".as_str()"),
        "{path} command classifier must not classify by stringified command names"
    );
    assert!(
        !body.contains("=> _"),
        "{path} command classifier must not discard typed classification results with `=> _`"
    );
    assert_body_has_no_catch_all_match_arms(path, &body, "command classifier");
    assert!(
        !body.contains('"'),
        "{path} command classifier must not contain string-literal command/input whitelists"
    );
}

fn assert_command_manifest_body_uses_typed_variants(path: &str, start_marker: &str) {
    let source = std::fs::read_to_string(repo_root().join(path)).expect("read manifest source");
    let body = function_body(&source, start_marker, "");
    assert!(
        !body.contains(".as_str()"),
        "{path} canonical command manifest must not stringify catalog inputs"
    );
    assert!(
        body.contains("catalog_input_variants()"),
        "{path} canonical command manifest must expose typed generated input variants"
    );
}

fn assert_flow_authority_manifest_body_uses_typed_variants(path: &str, start_marker: &str) {
    let source = std::fs::read_to_string(repo_root().join(path)).expect("read manifest source");
    let body = function_body(&source, start_marker, "");
    assert!(
        !body.contains(".as_str()") && !body.contains("InputVariantId::parse"),
        "{path} typed flow authority manifest must not project through string names"
    );
    assert!(
        body.contains("flow_projection_kernel_audit()"),
        "{path} typed flow authority manifest must derive from the typed projection-kernel audit"
    );
    assert!(
        !body.contains('"'),
        "{path} typed flow authority manifest must not contain string-literal input whitelists"
    );
    assert!(
        body.contains("MobMachineCatalogInput::input_variant"),
        "{path} typed flow authority manifest must expose typed generated input variants"
    );
}

fn assert_flow_authority_record_conversion_uses_typed_manifest(path: &str, start_marker: &str) {
    let source = std::fs::read_to_string(repo_root().join(path)).expect("read manifest source");
    let body = function_body(&source, start_marker, "");
    assert!(
        body.contains("canonical_flow_authority_input_variant_manifest()"),
        "{path} flow authority input conversion must validate against the typed canonical manifest"
    );
    assert!(
        !body.contains(".as_str()") && !body.contains("InputVariantId::parse"),
        "{path} flow authority input conversion must not gate through string names"
    );
    assert_body_has_no_catch_all_match_arms(path, &body, "flow authority conversion");
}

fn assert_flow_authority_body_has_no_catch_all(path: &str, start_marker: &str, context: &str) {
    let source = std::fs::read_to_string(repo_root().join(path)).expect("read authority source");
    let body = function_body(&source, start_marker, "");
    assert!(
        !body.contains(".as_str()") && !body.contains("InputVariantId::parse"),
        "{path} {context} must not gate through string names"
    );
    assert_body_has_no_catch_all_match_arms(path, &body, context);
}

fn assert_meerkat_runtime_internal_manifest_body_uses_typed_records(
    path: &str,
    start_marker: &str,
) {
    let source = std::fs::read_to_string(repo_root().join(path)).expect("read manifest source");
    let body = function_body(&source, start_marker, "");
    assert!(
        !body.contains(".as_str()") && !body.contains("InputVariantId::parse"),
        "{path} typed runtime-internal manifest must not project through string names"
    );
    assert!(
        body.contains("canonical_meerkat_machine_runtime_internal_classifications()"),
        "{path} typed runtime-internal manifest must derive from production records"
    );
    assert!(
        body.contains("input.input_variant()"),
        "{path} typed runtime-internal manifest must expose typed generated input variants"
    );
}

fn assert_meerkat_runtime_internal_classification_body_uses_typed_records(
    path: &str,
    start_marker: &str,
) {
    let source = std::fs::read_to_string(repo_root().join(path)).expect("read manifest source");
    let body = function_body(&source, start_marker, "");
    assert!(
        body.contains("MeerkatMachineRuntimeInternalInput::CLASSIFICATIONS"),
        "{path} runtime-internal classifications must be backed by typed classification records"
    );
    assert!(
        !body.contains("MeerkatMachineRuntimeInternalInput::ALL") && !body.contains(".map(|input|"),
        "{path} runtime-internal classifications must not walk a local input manifest and attach reasons afterward"
    );
}

fn assert_runtime_internal_stager_validates_typed_manifest(path: &str, start_marker: &str) {
    let source = std::fs::read_to_string(repo_root().join(path)).expect("read stager source");
    let body = function_body(&source, start_marker, "");
    assert!(
        body.contains("canonical_meerkat_machine_runtime_internal_input_variant_manifest()"),
        "{path} runtime-internal stager must validate against the typed production manifest"
    );
    assert!(
        body.contains(
            "canonical_meerkat_machine_runtime_internal_fieldless_input_variant_manifest()"
        ),
        "{path} fieldless runtime-internal stager must validate against the typed fieldless manifest"
    );
    assert!(
        body.contains("input.input_variant()"),
        "{path} runtime-internal stager must derive manifest evidence from the canonical typed input"
    );
    assert!(
        !body.contains("\"InterruptCurrentRun\""),
        "{path} runtime-internal stager must not special-case InterruptCurrentRun with a string gate"
    );
}

fn assert_no_local_runtime_internal_stager_alphabet(path: &str) {
    let source = std::fs::read_to_string(repo_root().join(path)).expect("read stager source");
    assert!(
        !source.contains("enum MeerkatMachineRuntimeInternalDslInput"),
        "{path} runtime-internal stager must not maintain a local shadow input alphabet"
    );
}

#[test]
fn machine_inputs_equal_runtime_manifest_through_typed_generated_facts() {
    let meerkat_records = canonical_meerkat_machine_command_classifications();
    let meerkat_runtime_commands = meerkat_runtime_command_input_variants(&meerkat_records);
    let meerkat_internal_inputs = meerkat_machine_runtime_internal_input_variants()
        .into_iter()
        .collect::<BTreeSet<_>>();
    assert_typed_runtime_manifest_matches_generated_inputs(
        "MeerkatMachine",
        &generated_meerkat_input_variants(),
        &meerkat_internal_inputs,
        &meerkat_runtime_commands,
    );

    let mob_records = canonical_mob_machine_command_classifications();
    let mob_runtime_commands = mob_runtime_command_input_variants(&mob_records);
    let mob_internal_inputs = mob_machine_runtime_internal_input_variants()
        .into_iter()
        .collect::<BTreeSet<_>>();
    assert_typed_runtime_manifest_matches_generated_inputs(
        "MobMachine",
        &generated_mob_input_variants(),
        &mob_internal_inputs,
        &mob_runtime_commands,
    );
}

#[test]
fn canonical_command_manifests_are_generated_input_variants() {
    let meerkat_manifest: BTreeSet<MeerkatMachineInputVariant> =
        canonical_meerkat_machine_command_input_variant_manifest()
            .into_iter()
            .collect();
    let meerkat_records = canonical_meerkat_machine_command_classifications();
    assert_eq!(
        meerkat_manifest,
        meerkat_runtime_command_input_variants(&meerkat_records),
        "MeerkatMachine canonical command manifest must expose typed generated input variants"
    );

    let meerkat_internal_manifest: BTreeSet<MeerkatMachineInputVariant> =
        canonical_meerkat_machine_runtime_internal_input_variant_manifest()
            .into_iter()
            .collect();
    let meerkat_declared_internal = meerkat_machine_runtime_internal_input_variants()
        .into_iter()
        .collect::<BTreeSet<_>>();
    assert_eq!(
        meerkat_internal_manifest, meerkat_declared_internal,
        "MeerkatMachine canonical runtime-internal manifest must expose typed generated input variants"
    );

    let mob_manifest: BTreeSet<MobMachineInputVariant> =
        canonical_mob_machine_command_input_variant_manifest()
            .into_iter()
            .collect();
    let mob_records = canonical_mob_machine_command_classifications();
    assert_eq!(
        mob_manifest,
        mob_runtime_command_input_variants(&mob_records),
        "MobMachine canonical command manifest must expose typed generated input variants"
    );
}

#[test]
fn legacy_canonical_command_manifests_preserve_string_api() {
    let meerkat_manifest: BTreeSet<&'static str> = canonical_meerkat_machine_command_manifest()
        .into_iter()
        .collect();
    let typed_meerkat_manifest: BTreeSet<&'static str> =
        canonical_meerkat_machine_command_input_variant_manifest()
            .into_iter()
            .map(|variant| variant.as_str())
            .collect();
    assert_eq!(
        meerkat_manifest, typed_meerkat_manifest,
        "legacy MeerkatMachine command manifest must remain a string projection of the typed manifest"
    );

    let meerkat_runtime_internal_manifest: BTreeSet<&'static str> =
        canonical_meerkat_machine_runtime_internal_manifest()
            .into_iter()
            .collect();
    let typed_meerkat_runtime_internal_manifest: BTreeSet<&'static str> =
        canonical_meerkat_machine_runtime_internal_input_variant_manifest()
            .into_iter()
            .map(|variant| variant.as_str())
            .collect();
    assert_eq!(
        meerkat_runtime_internal_manifest, typed_meerkat_runtime_internal_manifest,
        "legacy MeerkatMachine runtime-internal manifest must remain a string projection of the typed manifest"
    );

    let mob_manifest: BTreeSet<&'static str> = canonical_mob_machine_command_manifest()
        .into_iter()
        .collect();
    let typed_mob_manifest: BTreeSet<&'static str> =
        canonical_mob_machine_command_input_variant_manifest()
            .into_iter()
            .map(|variant| variant.as_str())
            .collect();
    assert_eq!(
        mob_manifest, typed_mob_manifest,
        "legacy MobMachine command manifest must remain a string projection of the typed manifest"
    );
}

#[test]
fn meerkat_machine_inputs_equal_runtime_manifest_exactly() {
    let generated_inputs = generated_meerkat_input_variants();
    let dsl_internal_inputs = meerkat_machine_runtime_internal_input_variants()
        .into_iter()
        .collect::<BTreeSet<_>>();
    let records = canonical_meerkat_machine_command_classifications();
    assert_meerkat_command_records_are_identity_checked(&generated_inputs, &records);
    let runtime_commands = meerkat_runtime_command_input_variants(&records);
    assert_all_meerkat_catalog_inputs_are_identity_checked(&generated_inputs, &runtime_commands);

    assert_typed_runtime_manifest_matches_generated_inputs(
        "MeerkatMachine",
        &generated_inputs,
        &dsl_internal_inputs,
        &runtime_commands,
    );
}

#[test]
fn mob_machine_inputs_equal_runtime_manifest_exactly() {
    let generated_inputs = generated_mob_input_variants();
    let dsl_internal_inputs = mob_machine_runtime_internal_input_variants()
        .into_iter()
        .collect::<BTreeSet<_>>();
    let records = canonical_mob_machine_command_classifications();
    assert_mob_command_records_are_identity_checked(&generated_inputs, &records);
    assert_all_mob_catalog_inputs_are_identity_checked(&generated_inputs);
    let runtime_commands = mob_runtime_command_input_variants(&records);

    assert_typed_runtime_manifest_matches_generated_inputs(
        "MobMachine",
        &generated_inputs,
        &dsl_internal_inputs,
        &runtime_commands,
    );
}

#[test]
fn runtime_classifications_do_not_expose_string_catalog_input_names() {
    for path in [
        "meerkat-runtime/src/meerkat_machine_types.rs",
        "meerkat-mob/src/mob_machine.rs",
    ] {
        let source =
            std::fs::read_to_string(repo_root().join(path)).expect("read classification source");
        assert!(
            !source.contains("catalog_input_names"),
            "{path} must expose typed catalog input variants to the parity gate, not string names"
        );
    }
}

#[test]
fn canonical_command_manifests_do_not_project_through_strings() {
    assert_command_manifest_body_uses_typed_variants(
        "meerkat-runtime/src/meerkat_machine_types.rs",
        "pub fn canonical_meerkat_machine_command_input_variant_manifest",
    );
    assert_meerkat_runtime_internal_manifest_body_uses_typed_records(
        "meerkat-runtime/src/meerkat_machine_types.rs",
        "pub fn canonical_meerkat_machine_runtime_internal_input_variant_manifest",
    );
    assert_meerkat_runtime_internal_classification_body_uses_typed_records(
        "meerkat-runtime/src/meerkat_machine_types.rs",
        "pub fn canonical_meerkat_machine_runtime_internal_classifications",
    );
    assert_command_manifest_body_uses_typed_variants(
        "meerkat-mob/src/mob_machine.rs",
        "pub fn canonical_mob_machine_command_input_variant_manifest",
    );
}

#[test]
fn flow_authority_manifest_does_not_project_through_strings() {
    assert_flow_authority_manifest_body_uses_typed_variants(
        "meerkat-mob/src/run.rs",
        "pub fn canonical_flow_authority_input_variant_manifest",
    );
    assert_flow_authority_record_conversion_uses_typed_manifest(
        "meerkat-mob/src/run.rs",
        "pub(crate) fn from_accepted_mob_machine_input",
    );
    assert_flow_authority_record_conversion_uses_typed_manifest(
        "meerkat-mob/src/run.rs",
        "pub(crate) fn from_machine_input",
    );
    assert_flow_authority_body_has_no_catch_all(
        "meerkat-mob/src/run.rs",
        "pub(crate) fn from_accepted_mob_machine_body_frame_seed",
        "body-frame seed flow authority conversion",
    );
}

#[test]
fn user_interrupt_path_uses_typed_runtime_internal_authority() {
    assert_runtime_internal_stager_validates_typed_manifest(
        "meerkat-runtime/src/meerkat_machine/dsl_effects.rs",
        "pub(super) fn stage_runtime_internal_dsl_transition_on_authority",
    );
    assert_no_local_runtime_internal_stager_alphabet(
        "meerkat-runtime/src/meerkat_machine/dsl_effects.rs",
    );
}

#[test]
fn fieldless_runtime_internal_inputs_are_typed_generated_manifest() {
    let typed_owner_manifest = MeerkatMachineFieldlessRuntimeInternalInput::ALL
        .iter()
        .copied()
        .map(MeerkatMachineFieldlessRuntimeInternalInput::input_variant)
        .collect::<BTreeSet<_>>();
    let typed_fieldless_manifest =
        canonical_meerkat_machine_runtime_internal_fieldless_input_variant_manifest()
            .into_iter()
            .collect::<BTreeSet<_>>();
    let typed_runtime_internal_manifest =
        canonical_meerkat_machine_runtime_internal_input_variant_manifest()
            .into_iter()
            .collect::<BTreeSet<_>>();

    assert_eq!(
        typed_fieldless_manifest, typed_owner_manifest,
        "fieldless runtime-internal inputs must be owned by the typed fieldless runtime-internal manifest"
    );
    assert!(
        typed_fieldless_manifest.is_subset(&typed_runtime_internal_manifest),
        "fieldless runtime-internal inputs must be a subset of the typed runtime-internal manifest"
    );
}

#[test]
fn command_classifiers_do_not_use_string_whitelists_or_wildcards() {
    assert_classifier_body_uses_typed_variants(
        "meerkat-runtime/src/meerkat_machine_types.rs",
        "const fn meerkat_machine_command_classification",
        "/// Snapshot of completion waiters",
    );
    assert_classifier_body_uses_typed_variants(
        "meerkat-mob/src/mob_machine.rs",
        "const fn mob_machine_command_classification",
        "",
    );
}

#[test]
fn every_canonical_input_has_transition_coverage() -> Result<(), IdentityError> {
    for schema in [meerkat_machine(), mob_machine()] {
        let surface_only_inputs: BTreeSet<InputVariantId> =
            schema.surface_only_inputs.iter().cloned().collect();
        let covered: BTreeSet<InputVariantId> = schema
            .transitions
            .iter()
            .filter_map(|transition| match &transition.on {
                meerkat_machine_schema::TriggerMatch::Input { variant, .. } => {
                    Some(variant.clone())
                }
                meerkat_machine_schema::TriggerMatch::Signal { .. } => None,
            })
            .collect();

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
            .filter_map(|transition| match &transition.on {
                meerkat_machine_schema::TriggerMatch::Input { .. } => None,
                meerkat_machine_schema::TriggerMatch::Signal { variant, .. } => {
                    Some(variant.clone())
                }
            })
            .collect();

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
