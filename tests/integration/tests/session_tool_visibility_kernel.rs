#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::{BTreeMap, BTreeSet};

use meerkat_machine_kernels::generated::meerkat;
use meerkat_machine_kernels::test_oracle::{
    GeneratedMachineKernel, KernelInput, KernelSignal, KernelState, KernelValue,
};
use meerkat_machine_schema::RustTypeAtom;
use meerkat_machine_schema::identity::{
    EffectVariantId, EnumVariantId, FieldId, InputVariantId, NamedTypeId, PhaseId, SignalVariantId,
};

fn field(slug: &str) -> FieldId {
    FieldId::parse(slug).expect("field id")
}

fn input(slug: &str) -> InputVariantId {
    InputVariantId::parse(slug).expect("input id")
}

fn signal(slug: &str) -> SignalVariantId {
    SignalVariantId::parse(slug).expect("signal id")
}

fn effect(slug: &str) -> EffectVariantId {
    EffectVariantId::parse(slug).expect("effect id")
}

fn enum_variant(slug: &str) -> EnumVariantId {
    EnumVariantId::parse(slug).expect("enum variant id")
}

fn phase(slug: &str) -> PhaseId {
    PhaseId::parse(slug).expect("phase id")
}

fn named_string(type_name: &str, value: &str) -> KernelValue {
    named_value(type_name, KernelValue::String(value.into()))
}

fn named_value(type_name: &str, value: KernelValue) -> KernelValue {
    KernelValue::Named {
        type_name: NamedTypeId::parse(type_name).expect("type id"),
        value: Box::new(value),
    }
}

fn named_u64(type_name: &str, value: u64) -> KernelValue {
    KernelValue::Named {
        type_name: NamedTypeId::parse(type_name).expect("type id"),
        value: Box::new(KernelValue::U64(value)),
    }
}

fn tool_filter_all() -> KernelValue {
    named_string("ToolFilter", "All")
}

fn string_key(value: &str) -> KernelValue {
    KernelValue::String(value.to_string())
}

fn witness_string(value: &str) -> KernelValue {
    named_string("ToolVisibilityWitness", value)
}

fn empty_witness() -> KernelValue {
    named_value("ToolVisibilityWitness", KernelValue::Map(BTreeMap::new()))
}

fn owner_witness(owner_key: &str) -> KernelValue {
    let provenance_source = owner_key
        .strip_prefix("callback:")
        .unwrap_or(owner_key)
        .to_string();
    named_value(
        "ToolVisibilityWitness",
        KernelValue::Map(BTreeMap::from([
            (
                string_key("stable_owner_key"),
                KernelValue::String(owner_key.to_string()),
            ),
            (
                string_key("last_seen_provenance"),
                named_value(
                    "ToolProvenance",
                    KernelValue::Map(BTreeMap::from([
                        (
                            string_key("kind"),
                            named_string("ToolSourceKind", "Callback"),
                        ),
                        (
                            string_key("source_id"),
                            KernelValue::String(provenance_source),
                        ),
                    ])),
                ),
            ),
        ])),
    )
}

fn provenance_witness(source_id: &str) -> KernelValue {
    named_value(
        "ToolVisibilityWitness",
        KernelValue::Map(BTreeMap::from([(
            string_key("last_seen_provenance"),
            named_value(
                "ToolProvenance",
                KernelValue::Map(BTreeMap::from([
                    (
                        string_key("kind"),
                        named_string("ToolSourceKind", "Callback"),
                    ),
                    (
                        string_key("source_id"),
                        KernelValue::String(source_id.to_string()),
                    ),
                ])),
            ),
        )])),
    )
}

fn string_set(values: &[&str]) -> KernelValue {
    KernelValue::Set(
        values
            .iter()
            .map(|value| KernelValue::String((*value).to_string()))
            .collect(),
    )
}

fn witness_map(entries: &[(&str, KernelValue)]) -> KernelValue {
    KernelValue::Map(
        entries
            .iter()
            .map(|(name, value)| (KernelValue::String((*name).to_string()), value.clone()))
            .collect(),
    )
}

fn witness_string_map(entries: &[(&str, &str)]) -> KernelValue {
    KernelValue::Map(
        entries
            .iter()
            .map(|(name, value)| {
                (
                    KernelValue::String((*name).to_string()),
                    witness_string(value),
                )
            })
            .collect(),
    )
}

fn prepared_meerkat_state(kernel: &GeneratedMachineKernel) -> KernelState {
    let initialized = kernel
        .transition_signal(
            &kernel.initial_state().expect("initial state"),
            &KernelSignal {
                variant: signal("Initialize"),
                fields: BTreeMap::new(),
            },
        )
        .expect("initialize")
        .next_state;
    let registered = kernel
        .transition(
            &initialized,
            &KernelInput {
                variant: input("RegisterSession"),
                fields: BTreeMap::from([(
                    field("session_id"),
                    named_string("SessionId", "session-1"),
                )]),
            },
        )
        .expect("register session")
        .next_state;
    kernel
        .transition(
            &registered,
            &KernelInput {
                variant: input("PrepareBindings"),
                fields: BTreeMap::from([
                    (
                        field("agent_runtime_id"),
                        named_string("AgentRuntimeId", "runtime-7"),
                    ),
                    (field("fence_token"), named_u64("FenceToken", 3)),
                    (field("generation"), named_u64("Generation", 1)),
                    (field("session_id"), named_string("SessionId", "session-1")),
                ]),
            },
        )
        .expect("prepare bindings")
        .next_state
}

#[test]
fn session_tool_visibility_kernel_publishes_committed_set_from_attached() {
    let kernel = GeneratedMachineKernel::new(meerkat::schema());
    let attached = prepared_meerkat_state(&kernel);

    let published = kernel
        .transition(
            &attached,
            &KernelInput {
                variant: input("PublishCommittedVisibleSet"),
                fields: BTreeMap::from([
                    (field("active_filter"), tool_filter_all()),
                    (field("staged_filter"), tool_filter_all()),
                    (
                        field("active_requested_deferred_names"),
                        KernelValue::Set(BTreeSet::new()),
                    ),
                    (
                        field("staged_requested_deferred_names"),
                        KernelValue::Set(BTreeSet::new()),
                    ),
                    (
                        field("active_deferred_authorities"),
                        KernelValue::Map(BTreeMap::new()),
                    ),
                    (
                        field("staged_deferred_authorities"),
                        KernelValue::Map(BTreeMap::new()),
                    ),
                    (field("active_visibility_revision"), KernelValue::U64(0)),
                    (field("staged_visibility_revision"), KernelValue::U64(0)),
                ]),
            },
        )
        .expect("publish committed visible set");

    assert_eq!(published.next_state.phase, phase("Attached"));
    assert_eq!(published.effects.len(), 1);
    assert_eq!(
        published.effects[0].variant,
        effect("CommittedVisibleSetPublished")
    );
}

#[test]
fn session_tool_visibility_kernel_publishes_committed_deferred_set_with_structural_authority() {
    let kernel = GeneratedMachineKernel::new(meerkat::schema());
    let attached = prepared_meerkat_state(&kernel);
    let names = string_set(&["search"]);
    let authorities = witness_map(&[("search", owner_witness("callback:search"))]);

    let published = kernel
        .transition(
            &attached,
            &KernelInput {
                variant: input("PublishCommittedVisibleSet"),
                fields: BTreeMap::from([
                    (field("active_filter"), tool_filter_all()),
                    (field("staged_filter"), tool_filter_all()),
                    (field("active_requested_deferred_names"), names.clone()),
                    (field("staged_requested_deferred_names"), names),
                    (field("active_deferred_authorities"), authorities.clone()),
                    (field("staged_deferred_authorities"), authorities),
                    (field("active_visibility_revision"), KernelValue::U64(1)),
                    (field("staged_visibility_revision"), KernelValue::U64(1)),
                ]),
            },
        )
        .expect("publish committed visible set with structural deferred authority");

    assert_eq!(published.next_state.phase, phase("Attached"));
    assert_eq!(
        published.effects[0].variant,
        effect("CommittedVisibleSetPublished")
    );
}

#[test]
fn session_tool_visibility_kernel_rejects_witness_when_source_kind_binding_changes() {
    let mut schema = meerkat::schema();
    schema
        .named_types
        .iter_mut()
        .find(|binding| binding.name.as_str() == "ToolSourceKind")
        .expect("ToolSourceKind binding")
        .rust = RustTypeAtom::StringEnum {
        variants: vec![enum_variant("Builtin")],
    };
    let kernel = GeneratedMachineKernel::new(schema);
    let attached = prepared_meerkat_state(&kernel);
    let names = string_set(&["search"]);
    let authorities = witness_map(&[("search", owner_witness("callback:search"))]);

    let err = kernel
        .transition(
            &attached,
            &KernelInput {
                variant: input("PublishCommittedVisibleSet"),
                fields: BTreeMap::from([
                    (field("active_filter"), tool_filter_all()),
                    (field("staged_filter"), tool_filter_all()),
                    (field("active_requested_deferred_names"), names.clone()),
                    (field("staged_requested_deferred_names"), names),
                    (field("active_deferred_authorities"), authorities.clone()),
                    (field("staged_deferred_authorities"), authorities),
                    (field("active_visibility_revision"), KernelValue::U64(1)),
                    (field("staged_visibility_revision"), KernelValue::U64(1)),
                ]),
            },
        )
        .expect_err("witness source kind must follow the ToolSourceKind binding");

    assert!(
        format!("{err:?}").contains("active_deferred_authorities"),
        "mutated ToolSourceKind binding must reject witness payload: {err:?}"
    );
}

#[test]
fn session_tool_visibility_kernel_rejects_witness_when_provenance_binding_is_missing() {
    let mut schema = meerkat::schema();
    schema
        .named_types
        .retain(|binding| binding.name.as_str() != "ToolProvenance");
    let kernel = GeneratedMachineKernel::new(schema);
    let attached = prepared_meerkat_state(&kernel);
    let names = string_set(&["search"]);
    let authorities = witness_map(&[("search", owner_witness("callback:search"))]);

    let err = kernel
        .transition(
            &attached,
            &KernelInput {
                variant: input("PublishCommittedVisibleSet"),
                fields: BTreeMap::from([
                    (field("active_filter"), tool_filter_all()),
                    (field("staged_filter"), tool_filter_all()),
                    (field("active_requested_deferred_names"), names.clone()),
                    (field("staged_requested_deferred_names"), names),
                    (field("active_deferred_authorities"), authorities.clone()),
                    (field("staged_deferred_authorities"), authorities),
                    (field("active_visibility_revision"), KernelValue::U64(1)),
                    (field("staged_visibility_revision"), KernelValue::U64(1)),
                ]),
            },
        )
        .expect_err("witness provenance must follow the ToolProvenance binding");

    assert!(
        format!("{err:?}").contains("active_deferred_authorities"),
        "missing ToolProvenance binding must reject witness payload: {err:?}"
    );
}

#[test]
fn session_tool_visibility_kernel_rejects_witness_when_provenance_binding_changes_shape() {
    let mut schema = meerkat::schema();
    schema
        .named_types
        .iter_mut()
        .find(|binding| binding.name.as_str() == "ToolProvenance")
        .expect("ToolProvenance binding")
        .rust = RustTypeAtom::String;
    let kernel = GeneratedMachineKernel::new(schema);
    let attached = prepared_meerkat_state(&kernel);
    let names = string_set(&["search"]);
    let authorities = witness_map(&[("search", owner_witness("callback:search"))]);

    let err = kernel
        .transition(
            &attached,
            &KernelInput {
                variant: input("PublishCommittedVisibleSet"),
                fields: BTreeMap::from([
                    (field("active_filter"), tool_filter_all()),
                    (field("staged_filter"), tool_filter_all()),
                    (field("active_requested_deferred_names"), names.clone()),
                    (field("staged_requested_deferred_names"), names),
                    (field("active_deferred_authorities"), authorities.clone()),
                    (field("staged_deferred_authorities"), authorities),
                    (field("active_visibility_revision"), KernelValue::U64(1)),
                    (field("staged_visibility_revision"), KernelValue::U64(1)),
                ]),
            },
        )
        .expect_err("witness provenance must follow the ToolProvenance binding");

    assert!(
        format!("{err:?}").contains("active_deferred_authorities"),
        "mutated ToolProvenance binding must reject witness payload: {err:?}"
    );
}

#[test]
fn session_tool_visibility_kernel_rejects_empty_authorities_when_witness_binding_is_missing() {
    let base_kernel = GeneratedMachineKernel::new(meerkat::schema());
    let attached = prepared_meerkat_state(&base_kernel);
    let mut schema = meerkat::schema();
    schema
        .named_types
        .retain(|binding| binding.name.as_str() != "ToolVisibilityWitness");
    let kernel = GeneratedMachineKernel::new(schema);

    let err = kernel
        .transition(
            &attached,
            &KernelInput {
                variant: input("PublishCommittedVisibleSet"),
                fields: BTreeMap::from([
                    (field("active_filter"), tool_filter_all()),
                    (field("staged_filter"), tool_filter_all()),
                    (
                        field("active_requested_deferred_names"),
                        KernelValue::Set(BTreeSet::new()),
                    ),
                    (
                        field("staged_requested_deferred_names"),
                        KernelValue::Set(BTreeSet::new()),
                    ),
                    (
                        field("active_deferred_authorities"),
                        KernelValue::Map(BTreeMap::new()),
                    ),
                    (
                        field("staged_deferred_authorities"),
                        KernelValue::Map(BTreeMap::new()),
                    ),
                    (field("active_visibility_revision"), KernelValue::U64(1)),
                    (field("staged_visibility_revision"), KernelValue::U64(1)),
                ]),
            },
        )
        .expect_err("missing ToolVisibilityWitness binding must reject empty authority maps");

    assert!(
        format!("{err:?}").contains("active_deferred_authorities"),
        "missing witness binding must reject empty authority map before it passes vacuously: {err:?}"
    );
}

#[test]
fn session_tool_visibility_kernel_rejects_committed_deferred_set_with_string_authority() {
    let kernel = GeneratedMachineKernel::new(meerkat::schema());
    let attached = prepared_meerkat_state(&kernel);
    let names = string_set(&["search"]);
    let authorities = witness_string_map(&[("search", "{}")]);

    let err = kernel
        .transition(
            &attached,
            &KernelInput {
                variant: input("PublishCommittedVisibleSet"),
                fields: BTreeMap::from([
                    (field("active_filter"), tool_filter_all()),
                    (field("staged_filter"), tool_filter_all()),
                    (field("active_requested_deferred_names"), names.clone()),
                    (field("staged_requested_deferred_names"), names),
                    (field("active_deferred_authorities"), authorities.clone()),
                    (field("staged_deferred_authorities"), authorities),
                    (field("active_visibility_revision"), KernelValue::U64(1)),
                    (field("staged_visibility_revision"), KernelValue::U64(1)),
                ]),
            },
        )
        .expect_err("string authority must not pass visibility admission");

    assert!(
        format!("{err:?}").contains("InvalidInputPayload"),
        "string authority must be rejected before visibility admission: {err:?}"
    );
}

#[test]
fn session_tool_visibility_kernel_accepts_deferred_request_without_phase_change() {
    let kernel = GeneratedMachineKernel::new(meerkat::schema());
    let attached = prepared_meerkat_state(&kernel);
    let requested = kernel
        .transition(
            &attached,
            &KernelInput {
                variant: input("RequestDeferredTools"),
                fields: BTreeMap::from([(
                    field("authorities"),
                    witness_map(&[
                        ("search", owner_witness("callback:search")),
                        ("view_image", provenance_witness("view-image")),
                    ]),
                )]),
            },
        )
        .expect("request deferred tools")
        .next_state;

    assert_eq!(requested.phase, phase("Attached"));
}

#[test]
fn session_tool_visibility_kernel_materializes_deferred_authority_in_state() {
    let kernel = GeneratedMachineKernel::new(meerkat::schema());
    let attached = prepared_meerkat_state(&kernel);
    let witness = owner_witness("callback:search");
    let requested = kernel
        .transition(
            &attached,
            &KernelInput {
                variant: input("RequestDeferredTools"),
                fields: BTreeMap::from([(
                    field("authorities"),
                    witness_map(&[("search", witness.clone())]),
                )]),
            },
        )
        .expect("request deferred tools")
        .next_state;

    assert_eq!(
        requested.fields.get(&field("staged_deferred_authorities")),
        Some(&witness_map(&[("search", witness)])),
        "deferred admission authority must be machine-owned, not a shell-side witness"
    );
    assert_eq!(
        requested.fields.get(&field("staged_deferred_names")),
        Some(&string_set(&["search"])),
        "names remain only the routing projection of the typed authority"
    );
}

#[test]
fn session_tool_visibility_kernel_rejects_deferred_names_without_witnesses() {
    let kernel = GeneratedMachineKernel::new(meerkat::schema());
    let attached = prepared_meerkat_state(&kernel);
    let err = kernel
        .transition(
            &attached,
            &KernelInput {
                variant: input("RequestDeferredTools"),
                fields: BTreeMap::from([(field("authorities"), KernelValue::Map(BTreeMap::new()))]),
            },
        )
        .expect_err("empty authority set must be rejected before staging names");

    assert!(
        format!("{err:?}").contains("NoMatchingTransition"),
        "missing authority must leave no admissible transition: {err:?}"
    );
}

#[test]
fn session_tool_visibility_kernel_rejects_deferred_names_with_empty_witness() {
    let kernel = GeneratedMachineKernel::new(meerkat::schema());
    let attached = prepared_meerkat_state(&kernel);
    let err = kernel
        .transition(
            &attached,
            &KernelInput {
                variant: input("RequestDeferredTools"),
                fields: BTreeMap::from([(
                    field("authorities"),
                    witness_map(&[("search", empty_witness())]),
                )]),
            },
        )
        .expect_err("empty witness must be rejected before staging names");

    assert!(
        format!("{err:?}").contains("NoMatchingTransition"),
        "empty authority must leave no admissible transition: {err:?}"
    );
}

#[test]
fn session_tool_visibility_kernel_rejects_deferred_names_with_serialized_default_witness() {
    let kernel = GeneratedMachineKernel::new(meerkat::schema());
    let attached = prepared_meerkat_state(&kernel);
    let err = kernel
        .transition(
            &attached,
            &KernelInput {
                variant: input("RequestDeferredTools"),
                fields: BTreeMap::from([(
                    field("authorities"),
                    witness_string_map(&[("search", "{}")]),
                )]),
            },
        )
        .expect_err("serialized default witness must not be accepted as typed authority");

    assert!(
        format!("{err:?}").contains("InvalidInputPayload"),
        "serialized default authority must be rejected before transition matching: {err:?}"
    );
}

#[test]
fn session_tool_visibility_kernel_rejects_legacy_stage_deferred_names_input() {
    let kernel = GeneratedMachineKernel::new(meerkat::schema());
    let attached = prepared_meerkat_state(&kernel);
    let err = kernel
        .transition(
            &attached,
            &KernelInput {
                variant: input("StageDeferredNames"),
                fields: BTreeMap::from([(field("names"), string_set(&["search"]))]),
            },
        )
        .expect_err("legacy staged names must not load deferred tools");

    assert!(
        format!("{err:?}").contains("NoMatchingTransition"),
        "legacy staged names must leave no admissible transition: {err:?}"
    );
}
