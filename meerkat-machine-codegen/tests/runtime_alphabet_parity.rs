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
    // Realtime-attachment + live-topology DSL inputs are staged directly
    // via `stage_session_dsl_input` from MeerkatMachine public methods
    // (see session_management.rs, llm_reconfigure.rs). They are internal
    // DSL transitions, not dispatched through the MeerkatMachineCommand
    // command channel — so they have no runtime manifest variants by
    // design. Exclude them from the exact-parity assertion.
    const DSL_INTERNAL_INPUTS: &[&str] = &[
        "ProjectRealtimeIntent",
        "BeginRealtimeBinding",
        "ReplaceRealtimeBinding",
        "DetachRealtimeBinding",
        "RequireRealtimeReattach",
        "PublishRealtimeSignal",
        "BeginLiveTopologyReconfigure",
        "MarkLiveTopologyDetached",
        "ApplyLiveTopologyIdentity",
        "ApplyLiveTopologyVisibility",
        "CompleteLiveTopology",
        "AbortLiveTopologyBeforeDetach",
        "FailLiveTopologyAfterDetach",
        // Phase 5G / T5g: MCP server handshake lifecycle is driven via
        // `McpServerLifecycleHandle` trait (meerkat-core/src/handles.rs)
        // staged by the MCP router shell, not through
        // `MeerkatMachineCommand`.
        "McpServerConnectPending",
        "McpServerConnected",
        "McpServerDisconnected",
        "McpServerFailed",
        "McpServerReload",
    ];
    let actual: BTreeSet<&str> = variant_names(&schema.inputs.variants)
        .into_iter()
        .filter(|name| !DSL_INTERNAL_INPUTS.contains(name))
        .collect();
    let expected: BTreeSet<&str> = canonical_meerkat_machine_command_manifest()
        .into_iter()
        .collect();

    assert_eq!(actual, expected);
}

#[test]
fn mob_machine_inputs_equal_runtime_manifest_exactly() {
    let schema = mob_machine();
    // Phase 5G declarative API additions (T4a/T4b/T5d) introduced
    // `EnsureMember`, `Reconcile`, and `ListMembersMatching` as
    // runtime-command composition helpers that dispatch through the
    // MobHandle without their own DSL transition entries — they layer on
    // top of `Spawn`/`Retire`/`ListMembers` rather than adding new
    // semantic facts. Exclude them from the exact-parity assertion until
    // the DSL schema is extended to model their composition seams (if
    // ever; they may stay shell-level by design).
    const RUNTIME_COMPOSITION_ONLY_COMMANDS: &[&str] =
        &["EnsureMember", "Reconcile", "ListMembersMatching"];
    let actual = variant_names(&schema.inputs.variants);
    let expected: BTreeSet<&str> = canonical_mob_machine_command_manifest()
        .into_iter()
        .filter(|name| !RUNTIME_COMPOSITION_ONLY_COMMANDS.contains(name))
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
