#![allow(clippy::expect_used)]

use meerkat_machine_schema::catalog::dsl::{
    dsl_auth_machine_production_schema, dsl_meerkat_machine, dsl_meerkat_machine_production_schema,
    dsl_mob_machine, dsl_mob_machine_production_schema, dsl_occurrence_lifecycle_machine,
    dsl_schedule_lifecycle_machine, dsl_workgraph_lifecycle_machine,
    meerkat_machine::{MeerkatMachineInput, MeerkatMachineInputVariant},
    meerkat_machine_runtime_internal_input_variants,
    mob_machine::{MobMachineInput, MobMachineInputVariant},
    mob_machine_runtime_internal_input_variants,
};
use meerkat_machine_schema::identity::{EnumVariantId, IdentityError, InputVariantId};
use meerkat_machine_schema::{
    EffectDispositionRule, FieldSchema, MachineSchema, NamedTypeBinding, TransitionSchema,
    VariantSchema, canonical_machine_schemas,
};
use meerkat_mob::MobMachineCatalogInput;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

fn input_alphabet(schema: &MachineSchema) -> Result<BTreeSet<InputVariantId>, IdentityError> {
    schema
        .inputs
        .variants
        .iter()
        .map(|variant| InputVariantId::parse(variant.name.as_str()))
        .collect()
}

struct SchemaParityCase {
    machine: &'static str,
    catalog_schema: fn() -> MachineSchema,
    production_schema: fn() -> MachineSchema,
}

fn schema_shape_mismatches(case: SchemaParityCase) -> Vec<String> {
    let catalog = (case.catalog_schema)();
    let production = (case.production_schema)();
    schema_shape_mismatches_for_schemas(&catalog, &production)
}

fn schema_shape_mismatches_for_schemas(
    catalog: &MachineSchema,
    production: &MachineSchema,
) -> Vec<String> {
    let mut mismatches = Vec::new();

    if catalog.machine != production.machine {
        mismatches.push("machine id".to_owned());
    }
    if catalog.version != production.version {
        mismatches.push("version".to_owned());
    }
    if catalog.rust != production.rust {
        mismatches.push("rust binding metadata".to_owned());
    }
    if catalog.state != production.state {
        mismatches.push("state".to_owned());
    }
    if catalog.inputs != production.inputs {
        mismatches.push("inputs".to_owned());
    }
    if catalog.surface_only_inputs != production.surface_only_inputs {
        mismatches.push("surface-only input metadata".to_owned());
    }
    if catalog.runtime_internal_inputs != production.runtime_internal_inputs {
        mismatches.push("runtime-internal input metadata".to_owned());
    }
    if catalog.named_types != production.named_types {
        mismatches.push("named types".to_owned());
    }
    if catalog.signals != production.signals {
        mismatches.push("signals".to_owned());
    }
    if catalog.effects != production.effects {
        mismatches.push("effects".to_owned());
    }
    if catalog.helpers != production.helpers {
        mismatches.push("helpers".to_owned());
    }
    if catalog.derived != production.derived {
        mismatches.push("derived helpers".to_owned());
    }
    if catalog.invariants != production.invariants {
        mismatches.push("invariants".to_owned());
    }
    if catalog.transitions != production.transitions {
        mismatches.push("transitions".to_owned());
    }
    if catalog.effect_dispositions != production.effect_dispositions {
        mismatches.push("effect dispositions / handoff metadata".to_owned());
    }
    if catalog.ci_step_limit != production.ci_step_limit {
        mismatches.push("CI semantic-model step-limit metadata".to_owned());
    }

    mismatches
}

fn phase1_schema_parity_cases() -> [SchemaParityCase; 6] {
    [
        SchemaParityCase {
            machine: "MeerkatMachine",
            catalog_schema: dsl_meerkat_machine_production_schema,
            production_schema: meerkat_runtime::machine_schema_exports::meerkat_machine_schema,
        },
        SchemaParityCase {
            machine: "AuthMachine",
            catalog_schema: dsl_auth_machine_production_schema,
            production_schema: meerkat_runtime::machine_schema_exports::auth_machine_schema,
        },
        SchemaParityCase {
            machine: "MobMachine",
            catalog_schema: dsl_mob_machine_production_schema,
            production_schema: meerkat_mob::machine_schema_exports::mob_machine_schema,
        },
        SchemaParityCase {
            machine: "ScheduleLifecycleMachine",
            catalog_schema: dsl_schedule_lifecycle_machine,
            production_schema: meerkat_schedule::machine_schema_exports::schedule_lifecycle_schema,
        },
        SchemaParityCase {
            machine: "OccurrenceLifecycleMachine",
            catalog_schema: dsl_occurrence_lifecycle_machine,
            production_schema:
                meerkat_schedule::machine_schema_exports::occurrence_lifecycle_schema,
        },
        SchemaParityCase {
            machine: "WorkGraphLifecycleMachine",
            catalog_schema: dsl_workgraph_lifecycle_machine,
            production_schema:
                meerkat_workgraph::machine_schema_exports::workgraph_lifecycle_schema,
        },
    ]
}

fn phase1_schema_drift_report() -> Vec<String> {
    let mut failures = Vec::new();
    for case in phase1_schema_parity_cases() {
        let machine = case.machine;
        let mismatches = schema_shape_mismatches(case);
        if !mismatches.is_empty() {
            failures.push(format!("{machine}: {}", mismatches.join(", ")));
        }
    }
    failures
}

fn fields_by_name(fields: &[FieldSchema]) -> BTreeMap<String, &FieldSchema> {
    fields
        .iter()
        .map(|field| (field.name.as_str().to_owned(), field))
        .collect()
}

fn variants_by_name(variants: &[VariantSchema]) -> BTreeMap<String, &VariantSchema> {
    variants
        .iter()
        .map(|variant| (variant.name.as_str().to_owned(), variant))
        .collect()
}

fn transitions_by_name(transitions: &[TransitionSchema]) -> BTreeMap<String, &TransitionSchema> {
    transitions
        .iter()
        .map(|transition| (transition.name.as_str().to_owned(), transition))
        .collect()
}

fn dispositions_by_name(
    dispositions: &[EffectDispositionRule],
) -> BTreeMap<String, &EffectDispositionRule> {
    dispositions
        .iter()
        .map(|rule| (rule.effect_variant.as_str().to_owned(), rule))
        .collect()
}

fn named_types_by_name(bindings: &[NamedTypeBinding]) -> BTreeMap<String, &NamedTypeBinding> {
    bindings
        .iter()
        .map(|binding| (binding.name.as_str().to_owned(), binding))
        .collect()
}

fn describe_key_diff<T: PartialEq + std::fmt::Debug>(
    label: &str,
    catalog: BTreeMap<String, &T>,
    production: BTreeMap<String, &T>,
    out: &mut Vec<String>,
) {
    for name in production.keys() {
        if !catalog.contains_key(name) {
            out.push(format!("{label}.production_only.{name}"));
        }
    }
    for name in catalog.keys() {
        if !production.contains_key(name) {
            out.push(format!("{label}.catalog_only.{name}"));
        }
    }
    for (name, catalog_item) in &catalog {
        if let Some(production_item) = production.get(name)
            && *catalog_item != *production_item
        {
            out.push(format!("{label}.changed.{name}"));
        }
    }
}

fn schema_drift_items_for_schemas(
    catalog: &MachineSchema,
    production: &MachineSchema,
) -> Vec<String> {
    let mut items = Vec::new();

    describe_key_diff(
        "state_field",
        fields_by_name(&catalog.state.fields),
        fields_by_name(&production.state.fields),
        &mut items,
    );
    describe_key_diff(
        "init_field",
        catalog
            .state
            .init
            .fields
            .iter()
            .map(|field| (field.field.as_str().to_owned(), field))
            .collect(),
        production
            .state
            .init
            .fields
            .iter()
            .map(|field| (field.field.as_str().to_owned(), field))
            .collect(),
        &mut items,
    );
    describe_key_diff(
        "phase",
        variants_by_name(&catalog.state.phase.variants),
        variants_by_name(&production.state.phase.variants),
        &mut items,
    );
    describe_key_diff(
        "input",
        variants_by_name(&catalog.inputs.variants),
        variants_by_name(&production.inputs.variants),
        &mut items,
    );
    describe_key_diff(
        "signal",
        variants_by_name(&catalog.signals.variants),
        variants_by_name(&production.signals.variants),
        &mut items,
    );
    describe_key_diff(
        "effect",
        variants_by_name(&catalog.effects.variants),
        variants_by_name(&production.effects.variants),
        &mut items,
    );
    describe_key_diff(
        "transition",
        transitions_by_name(&catalog.transitions),
        transitions_by_name(&production.transitions),
        &mut items,
    );
    describe_key_diff(
        "effect_disposition",
        dispositions_by_name(&catalog.effect_dispositions),
        dispositions_by_name(&production.effect_dispositions),
        &mut items,
    );
    describe_key_diff(
        "named_type",
        named_types_by_name(&catalog.named_types),
        named_types_by_name(&production.named_types),
        &mut items,
    );

    items
}

fn phase1_schema_drift_item_report() -> Vec<String> {
    let mut items = Vec::new();
    for case in phase1_schema_parity_cases() {
        let catalog = (case.catalog_schema)();
        let production = (case.production_schema)();
        for item in schema_drift_items_for_schemas(&catalog, &production) {
            items.push(format!("{}::{item}", case.machine));
        }
    }
    items
}

fn phase1_schema_drift_item_counts() -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for item in phase1_schema_drift_item_report() {
        let mut parts = item.split("::");
        let machine = parts.next().expect("machine");
        let rest = parts.next().expect("item");
        let mut item_parts = rest.split('.');
        let category = item_parts.next().expect("category");
        let side = item_parts.next().expect("side");
        *counts
            .entry(format!("{machine}::{category}.{side}"))
            .or_insert(0) += 1;
    }
    counts
}

fn repo_root() -> PathBuf {
    if let Some(root) = std::env::var_os("WORKSPACE_ROOT") {
        return PathBuf::from(root);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("repo root")
        .to_path_buf()
}

fn generated_mob_input_variants() -> BTreeSet<MobMachineInputVariant> {
    MobMachineInput::variant_manifest()
        .iter()
        .copied()
        .collect()
}

fn generated_meerkat_input_variants() -> BTreeSet<MeerkatMachineInputVariant> {
    MeerkatMachineInput::variant_manifest()
        .iter()
        .copied()
        .collect()
}

fn mob_catalog_input_variants(
    inputs: impl IntoIterator<Item = MobMachineCatalogInput>,
) -> BTreeSet<MobMachineInputVariant> {
    inputs
        .into_iter()
        .map(MobMachineCatalogInput::input_variant)
        .collect()
}

fn critical_mob_runtime_command_inputs() -> BTreeSet<MobMachineInputVariant> {
    mob_catalog_input_variants([
        MobMachineCatalogInput::RunFlow,
        MobMachineCatalogInput::CancelFlow,
        MobMachineCatalogInput::SetSpawnPolicy,
        MobMachineCatalogInput::ForceCancel,
    ])
}

#[test]
fn phase1_production_machine_schemas_match_catalog_shape() {
    let failures = phase1_schema_drift_report();
    assert!(
        failures.is_empty(),
        "catalog/production schema drift detected outside crate/module binding metadata:\n- {}",
        failures.join("\n- ")
    );
}

#[test]
fn phase1_schema_parity_inventory_reports_no_current_drift() {
    let failures = phase1_schema_drift_report();
    assert_eq!(
        failures,
        Vec::<String>::new(),
        "Phase 1 schema parity must stay closed once catalog-owned production bodies converge"
    );
}

#[test]
fn phase1_schema_parity_inventory_has_itemized_drift() {
    let items = phase1_schema_drift_item_report();
    assert!(
        !items.iter().any(|item| item.starts_with("AuthMachine::")),
        "AuthMachine drift should be resolved by the first Phase 1 batch, got {items:#?}"
    );
    assert!(
        !items
            .iter()
            .any(|item| item.starts_with("MeerkatMachine::")),
        "MeerkatMachine drift should be resolved by the catalog-owned Phase 1 lift, got {items:#?}"
    );
    assert!(
        !items.iter().any(|item| item.starts_with("MobMachine::")),
        "MobMachine drift should be resolved by the catalog-owned Phase 1 lift, got {items:#?}"
    );
    assert!(
        items.is_empty(),
        "Phase 1 schema parity inventory should be empty after convergence, got {items:#?}"
    );
}

#[test]
fn phase1_schema_parity_inventory_pins_remaining_drift_counts() {
    let counts = phase1_schema_drift_item_counts();
    let expected = BTreeMap::new();

    assert_eq!(
        counts, expected,
        "Phase 1 drift must stay empty; update the parity ledger before reopening it"
    );
}

#[test]
fn mob_flow_projection_kernels_are_audited_as_non_canonical_support() {
    let canonical_machine_names = canonical_machine_schemas()
        .into_iter()
        .map(|schema| schema.machine.as_str().to_owned())
        .collect::<BTreeSet<_>>();
    let mob_inputs = generated_mob_input_variants();
    let audit = meerkat_mob::run::flow_projection_kernel_audit();

    assert_eq!(
        audit.iter().map(|entry| entry.module).collect::<Vec<_>>(),
        vec!["flow_run", "flow_frame", "loop_iteration"],
        "the live flow projection kernel audit must name every carried-forward helper"
    );

    for entry in audit {
        assert_eq!(
            entry.canonical_owner, "MobMachine",
            "{} must remain owned by MobMachine",
            entry.module
        );
        assert_eq!(
            entry.role,
            meerkat_mob::run::FlowProjectionKernelRole::MobMachineOwnedFailClosedProjection,
            "{} must be classified as MobMachine-owned fail-closed support, not as a production DSL",
            entry.module
        );
        assert!(
            !entry.canonical_machine,
            "{} must not claim canonical-machine status",
            entry.module
        );
        assert!(
            !canonical_machine_names.contains(entry.module),
            "{} must not be registered as a canonical machine",
            entry.module
        );
    }
    let expected_flow_authority_inputs = std::iter::once(MobMachineCatalogInput::RunFlow)
        .chain(
            audit
                .iter()
                .flat_map(|entry| entry.owning_inputs.iter().copied()),
        )
        .map(MobMachineCatalogInput::input_variant)
        .collect::<BTreeSet<_>>();
    let flow_authority_inputs = meerkat_mob::run::canonical_flow_authority_input_variant_manifest()
        .into_iter()
        .collect::<BTreeSet<_>>();
    assert_eq!(
        flow_authority_inputs, expected_flow_authority_inputs,
        "flow authority input manifest must exactly match the audited MobMachine authority alphabet"
    );
    assert!(
        flow_authority_inputs.is_subset(&mob_inputs),
        "flow authority input manifest must name only canonical MobMachine inputs"
    );
}

#[test]
fn mob_runtime_parity_production_command_manifest_closes_command_backed_inputs() {
    let catalog_inputs = generated_mob_input_variants();
    let production_command_manifest =
        meerkat_mob::canonical_mob_machine_command_input_variant_manifest()
            .into_iter()
            .collect::<BTreeSet<_>>();
    let command_backed_runtime_inputs = mob_catalog_input_variants([
        MobMachineCatalogInput::RunFlow,
        MobMachineCatalogInput::CancelFlow,
        MobMachineCatalogInput::EnsureMember,
        MobMachineCatalogInput::Reconcile,
        MobMachineCatalogInput::WireMembers,
        MobMachineCatalogInput::UnwireMembers,
        MobMachineCatalogInput::WireExternalPeer,
        MobMachineCatalogInput::UnwireExternalPeer,
        MobMachineCatalogInput::SetSpawnPolicy,
        MobMachineCatalogInput::ForceCancel,
    ]);

    assert!(
        production_command_manifest.is_subset(&catalog_inputs),
        "production mob command classifications must name only catalog inputs"
    );
    assert!(
        command_backed_runtime_inputs.is_subset(&production_command_manifest),
        "command-backed MobMachine inputs must be proved by the production command manifest"
    );
}

#[test]
fn mob_runtime_parity_critical_command_inputs_cannot_be_shell_mechanics() {
    let production_runtime_path =
        meerkat_mob::canonical_mob_machine_command_input_variant_manifest()
            .into_iter()
            .collect::<BTreeSet<_>>();
    let critical = critical_mob_runtime_command_inputs();

    assert!(
        critical.is_subset(&production_runtime_path),
        "critical runtime commands must be catalog-command backed, not hidden behind shell-mechanic debt"
    );
}

#[test]
fn mob_runtime_parity_runtime_internal_manifest_has_typed_reasons() {
    let catalog_inputs = generated_mob_input_variants();
    let records = meerkat_mob::canonical_mob_machine_runtime_internal_classifications();
    let classified = records
        .iter()
        .map(|record| record.input.input_variant())
        .collect::<BTreeSet<_>>();
    let declared = mob_machine_runtime_internal_input_variants()
        .into_iter()
        .collect::<BTreeSet<_>>();
    let typed_manifest =
        meerkat_mob::canonical_mob_machine_runtime_internal_input_variant_manifest()
            .into_iter()
            .collect::<BTreeSet<_>>();

    assert!(
        !records.is_empty(),
        "runtime-internal MobMachine feedback paths must be declared by typed production records"
    );
    assert_eq!(
        classified, declared,
        "typed runtime-internal records must exactly match schema.runtime_internal_inputs"
    );
    assert_eq!(
        typed_manifest, declared,
        "runtime-internal manifest must expose typed generated input variants"
    );
    for record in records {
        assert!(
            catalog_inputs.contains(&record.input.input_variant()),
            "runtime-internal record {:?} must name a catalog input",
            record.input
        );
    }
}

#[test]
fn meerkat_runtime_parity_runtime_internal_manifest_has_typed_reasons() {
    let catalog_inputs = generated_meerkat_input_variants();
    let records = meerkat_runtime::canonical_meerkat_machine_runtime_internal_classifications();
    let distinct_reasons = records.iter().fold(Vec::new(), |mut reasons, record| {
        if !reasons.contains(&record.reason) {
            reasons.push(record.reason);
        }
        reasons
    });
    let classified = records
        .iter()
        .map(|record| record.input.input_variant())
        .collect::<BTreeSet<_>>();
    let declared = meerkat_machine_runtime_internal_input_variants()
        .into_iter()
        .collect::<BTreeSet<_>>();
    let typed_manifest =
        meerkat_runtime::canonical_meerkat_machine_runtime_internal_input_variant_manifest()
            .into_iter()
            .collect::<BTreeSet<_>>();
    let interrupt_reason = records
        .iter()
        .find(|record| {
            record.input.input_variant() == MeerkatMachineInputVariant::InterruptCurrentRun
        })
        .map(|record| record.reason)
        .expect("InterruptCurrentRun production evidence");

    assert!(
        !records.is_empty(),
        "runtime-internal MeerkatMachine paths must be declared by typed production records"
    );
    assert!(
        distinct_reasons.len() >= 8,
        "runtime-internal MeerkatMachine paths must be anchored to real typed owner/path categories, got {distinct_reasons:?}"
    );
    assert_eq!(
        classified, declared,
        "typed MeerkatMachine runtime-internal records must exactly match schema.runtime_internal_inputs"
    );
    assert_eq!(
        typed_manifest, declared,
        "MeerkatMachine runtime-internal manifest must expose typed generated input variants"
    );
    for record in records {
        assert!(
            catalog_inputs.contains(&record.input.input_variant()),
            "runtime-internal MeerkatMachine record {:?} must name a catalog input",
            record.input
        );
    }
    assert_eq!(
        interrupt_reason,
        meerkat_runtime::MeerkatMachineRuntimeInternalReason::UserInterruptDispatch,
        "public user interrupt path must have typed production evidence"
    );
}

#[test]
fn meerkat_runtime_parity_fieldless_runtime_internal_manifest_matches_schema() {
    let typed_owner_manifest = meerkat_runtime::MeerkatMachineFieldlessRuntimeInternalInput::ALL
        .iter()
        .copied()
        .map(meerkat_runtime::MeerkatMachineFieldlessRuntimeInternalInput::input_variant)
        .collect::<BTreeSet<_>>();
    let typed_fieldless_manifest =
        meerkat_runtime::canonical_meerkat_machine_runtime_internal_fieldless_input_variant_manifest()
            .into_iter()
            .collect::<BTreeSet<_>>();
    let typed_runtime_internal_manifest =
        meerkat_runtime::canonical_meerkat_machine_runtime_internal_input_variant_manifest()
            .into_iter()
            .collect::<BTreeSet<_>>();

    assert_eq!(
        typed_fieldless_manifest, typed_owner_manifest,
        "fieldless runtime-internal inputs must be declared by the typed fieldless manifest"
    );
    assert!(
        typed_fieldless_manifest.is_subset(&typed_runtime_internal_manifest),
        "fieldless runtime-internal inputs must be a typed subset of runtime-internal production evidence"
    );
}

#[test]
fn mob_runtime_parity_legacy_manifests_preserve_string_api() {
    let command_manifest: BTreeSet<&'static str> =
        meerkat_mob::canonical_mob_machine_command_manifest()
            .into_iter()
            .collect();
    let typed_command_manifest: BTreeSet<&'static str> =
        meerkat_mob::canonical_mob_machine_command_input_variant_manifest()
            .into_iter()
            .map(|variant| variant.as_str())
            .collect();
    assert_eq!(
        command_manifest, typed_command_manifest,
        "legacy MobMachine command manifest must remain a string projection of the typed manifest"
    );

    let runtime_internal_manifest: BTreeSet<&'static str> =
        meerkat_mob::canonical_mob_machine_runtime_internal_manifest()
            .into_iter()
            .collect();
    let typed_runtime_internal_manifest: BTreeSet<&'static str> =
        meerkat_mob::canonical_mob_machine_runtime_internal_input_variant_manifest()
            .into_iter()
            .map(|variant| variant.as_str())
            .collect();
    assert_eq!(
        runtime_internal_manifest, typed_runtime_internal_manifest,
        "legacy MobMachine runtime-internal manifest must remain a string projection of the typed manifest"
    );

    let flow_authority_manifest: BTreeSet<&'static str> =
        meerkat_mob::run::canonical_flow_authority_input_manifest()
            .into_iter()
            .collect();
    let typed_flow_authority_manifest: BTreeSet<&'static str> =
        meerkat_mob::run::canonical_flow_authority_input_variant_manifest()
            .into_iter()
            .map(|variant| variant.as_str())
            .collect();
    assert_eq!(
        flow_authority_manifest, typed_flow_authority_manifest,
        "legacy flow authority manifest must remain a string projection of the typed manifest"
    );
}

#[test]
fn mob_machine_native_reducer_helpers_are_formally_defined() {
    let model = std::fs::read_to_string(repo_root().join("specs/machines/mob_machine/model.tla"))
        .expect("read generated MobMachine TLA");
    for helper in [
        "mob_machine_node_terminal",
        "mob_machine_frame_node_status_after_admit",
        "mob_machine_frame_ready_queue_after_admit",
        "mob_machine_frame_node_status_after_terminal",
        "mob_machine_frame_ready_queue_after_terminal",
    ] {
        assert!(
            model.contains(&format!("{helper}(")),
            "MobMachine TLA must call or define native helper `{helper}`"
        );
        assert!(
            model
                .lines()
                .any(|line| line.starts_with(&format!("{helper}(")) && line.contains("==")),
            "native helper `{helper}` must have a generated TLA operator definition"
        );
    }
}

#[test]
#[ignore = "diagnostic: prints itemized catalog/production drift for parity-ledger classification"]
fn print_phase1_schema_drift_items() {
    for item in phase1_schema_drift_item_report() {
        println!("{item}");
    }
}

#[test]
fn schema_parity_gate_rejects_production_only_non_input_drift() {
    let catalog_schema = dsl_meerkat_machine();
    let mut production_schema = catalog_schema.clone();
    production_schema.effects.variants.push(VariantSchema {
        name: EnumVariantId::parse("ProductionOnlyEffectDrift").expect("variant id"),
        fields: vec![],
    });

    assert_eq!(
        schema_shape_mismatches_for_schemas(&catalog_schema, &production_schema),
        vec!["effects"],
        "full schema parity comparator must reject production-only non-input drift"
    );
    assert_eq!(
        input_alphabet(&catalog_schema).expect("catalog alphabet"),
        input_alphabet(&production_schema).expect("production alphabet"),
        "the old input-only gate would miss this effect drift"
    );
}

#[test]
fn schema_parity_gate_rejects_named_type_metadata_drift() {
    let catalog_schema = dsl_meerkat_machine();
    let mut production_schema = catalog_schema.clone();
    production_schema.named_types.pop();

    assert_eq!(
        schema_shape_mismatches_for_schemas(&catalog_schema, &production_schema),
        vec!["named types"],
        "full schema parity comparator must reject missing named-type bindings"
    );
    assert_eq!(
        input_alphabet(&catalog_schema).expect("catalog alphabet"),
        input_alphabet(&production_schema).expect("production alphabet"),
        "the old input-only gate would miss named-type metadata drift"
    );
}

#[test]
fn schema_parity_gate_rejects_rust_binding_metadata_drift() {
    let catalog_schema = dsl_mob_machine();
    let mut production_schema = catalog_schema.clone();
    production_schema.rust.module = "wrong::module".to_owned();

    assert_eq!(
        schema_shape_mismatches_for_schemas(&catalog_schema, &production_schema),
        vec!["rust binding metadata"],
        "full schema parity comparator must reject production rust binding drift"
    );
    assert_eq!(
        input_alphabet(&catalog_schema).expect("catalog alphabet"),
        input_alphabet(&production_schema).expect("production alphabet"),
        "the old input-only gate would miss rust binding metadata drift"
    );
}

#[test]
fn schema_parity_gate_rejects_runtime_internal_input_metadata_drift() {
    let catalog_schema = dsl_meerkat_machine();
    let mut production_schema = catalog_schema.clone();
    production_schema.runtime_internal_inputs.pop();

    assert_eq!(
        schema_shape_mismatches_for_schemas(&catalog_schema, &production_schema),
        vec!["runtime-internal input metadata"],
        "full schema parity comparator must reject missing runtime-internal input metadata"
    );
    assert_eq!(
        input_alphabet(&catalog_schema).expect("catalog alphabet"),
        input_alphabet(&production_schema).expect("production alphabet"),
        "the old input-only gate would miss runtime-internal input metadata drift"
    );
}

#[test]
fn production_schema_exports_include_metadata_without_catalog_schema_splicing() {
    for case in phase1_schema_parity_cases() {
        let catalog = (case.catalog_schema)();
        let production = (case.production_schema)();
        assert_eq!(
            production.named_types, catalog.named_types,
            "{} production export must carry named-type metadata before parity comparison",
            case.machine
        );
        assert_eq!(
            production.runtime_internal_inputs, catalog.runtime_internal_inputs,
            "{} production export must carry runtime-internal input metadata before parity comparison",
            case.machine
        );
    }
}

#[test]
#[ignore = "phase-0 failure fixture: old input gate misses non-input drift"]
fn input_only_parity_misses_production_only_effect_drift() -> Result<(), IdentityError> {
    let catalog_schema = dsl_meerkat_machine();
    let mut production_schema = catalog_schema.clone();
    production_schema.effects.variants.push(VariantSchema {
        name: EnumVariantId::parse("ProductionOnlyEffectDrift")?,
        fields: vec![],
    });

    assert_eq!(
        input_alphabet(&catalog_schema)?,
        input_alphabet(&production_schema)?,
        "an input-only parity gate would pass even though the production schema drifted"
    );
    assert!(
        catalog_schema != production_schema,
        "full schema parity must detect non-input drift"
    );

    Ok(())
}
