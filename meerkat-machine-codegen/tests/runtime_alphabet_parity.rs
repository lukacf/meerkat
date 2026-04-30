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
    MobMachineCommandClassificationRecord, MobMachineCommandVariant,
    canonical_mob_machine_command_classifications,
};
use meerkat_runtime::{
    MeerkatMachineCatalogInput as MeerkatInput, MeerkatMachineCommandClassification,
    MeerkatMachineCommandClassificationRecord, MeerkatMachineCommandVariant,
    canonical_meerkat_machine_command_classifications,
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

fn meerkat_command_self_catalog_input(
    command: MeerkatMachineCommandVariant,
) -> Option<MeerkatInput> {
    match command {
        MeerkatMachineCommandVariant::RegisterSession => Some(MeerkatInput::RegisterSession),
        MeerkatMachineCommandVariant::UnregisterSession => Some(MeerkatInput::UnregisterSession),
        MeerkatMachineCommandVariant::EnsureSessionWithExecutor => {
            Some(MeerkatInput::EnsureSessionWithExecutor)
        }
        MeerkatMachineCommandVariant::SetSilentIntents => Some(MeerkatInput::SetSilentIntents),
        MeerkatMachineCommandVariant::InterruptCurrentRun => {
            Some(MeerkatInput::InterruptCurrentRun)
        }
        MeerkatMachineCommandVariant::CancelAfterBoundary => {
            Some(MeerkatInput::CancelAfterBoundary)
        }
        MeerkatMachineCommandVariant::StopRuntimeExecutor => {
            Some(MeerkatInput::StopRuntimeExecutor)
        }
        MeerkatMachineCommandVariant::ContainsSession => Some(MeerkatInput::ContainsSession),
        MeerkatMachineCommandVariant::SessionHasExecutor => Some(MeerkatInput::SessionHasExecutor),
        MeerkatMachineCommandVariant::SessionHasComms => Some(MeerkatInput::SessionHasComms),
        MeerkatMachineCommandVariant::OpsLifecycleRegistry => {
            Some(MeerkatInput::OpsLifecycleRegistry)
        }
        MeerkatMachineCommandVariant::PrepareBindings => Some(MeerkatInput::PrepareBindings),
        MeerkatMachineCommandVariant::PrepareLocalSessionBindings => None,
        MeerkatMachineCommandVariant::InputState => Some(MeerkatInput::InputState),
        MeerkatMachineCommandVariant::ListActiveInputs => Some(MeerkatInput::ListActiveInputs),
        MeerkatMachineCommandVariant::ReconfigureSessionLlmIdentity => {
            Some(MeerkatInput::ReconfigureSessionLlmIdentity)
        }
        MeerkatMachineCommandVariant::StagePersistentFilter => {
            Some(MeerkatInput::StagePersistentFilter)
        }
        MeerkatMachineCommandVariant::RequestDeferredTools => {
            Some(MeerkatInput::RequestDeferredTools)
        }
        MeerkatMachineCommandVariant::PublishCommittedVisibleSet => {
            Some(MeerkatInput::PublishCommittedVisibleSet)
        }
        MeerkatMachineCommandVariant::SetPeerIngressContext => {
            Some(MeerkatInput::SetPeerIngressContext)
        }
        MeerkatMachineCommandVariant::NotifyDrainExited => Some(MeerkatInput::NotifyDrainExited),
        MeerkatMachineCommandVariant::AbortAll => Some(MeerkatInput::AbortAll),
        MeerkatMachineCommandVariant::Abort => Some(MeerkatInput::Abort),
        MeerkatMachineCommandVariant::Wait => Some(MeerkatInput::Wait),
        MeerkatMachineCommandVariant::Ingest => Some(MeerkatInput::Ingest),
        MeerkatMachineCommandVariant::PublishEvent => Some(MeerkatInput::PublishEvent),
        MeerkatMachineCommandVariant::Retire => Some(MeerkatInput::Retire),
        MeerkatMachineCommandVariant::Recycle => Some(MeerkatInput::Recycle),
        MeerkatMachineCommandVariant::Reset => Some(MeerkatInput::Reset),
        MeerkatMachineCommandVariant::Recover => Some(MeerkatInput::Recover),
        MeerkatMachineCommandVariant::Destroy => Some(MeerkatInput::Destroy),
        MeerkatMachineCommandVariant::RuntimeState => Some(MeerkatInput::RuntimeState),
        MeerkatMachineCommandVariant::RuntimeRealtimeAttachmentStatus => {
            Some(MeerkatInput::RuntimeRealtimeAttachmentStatus)
        }
        MeerkatMachineCommandVariant::RuntimeRealtimeChannelStatus => None,
        MeerkatMachineCommandVariant::ConfigureModelRoutingBaseline => None,
        MeerkatMachineCommandVariant::SessionModelRoutingStatus => None,
        MeerkatMachineCommandVariant::RequestSwitchTurn => None,
        MeerkatMachineCommandVariant::AdmitModelRoutingAssistantTurn => {
            Some(MeerkatInput::AdmitModelRoutingAssistantTurn)
        }
        MeerkatMachineCommandVariant::BeginImageOperation => {
            Some(MeerkatInput::BeginImageOperation)
        }
        MeerkatMachineCommandVariant::ActivateImageOperationOverride => {
            Some(MeerkatInput::ActivateImageOperationOverride)
        }
        MeerkatMachineCommandVariant::CompleteImageOperation => {
            Some(MeerkatInput::CompleteImageOperation)
        }
        MeerkatMachineCommandVariant::RestoreImageOperationOverride => {
            Some(MeerkatInput::RestoreImageOperationOverride)
        }
        MeerkatMachineCommandVariant::LoadBoundaryReceipt => {
            Some(MeerkatInput::LoadBoundaryReceipt)
        }
        MeerkatMachineCommandVariant::AcceptWithCompletion => {
            Some(MeerkatInput::AcceptWithCompletion)
        }
        MeerkatMachineCommandVariant::AcceptWithoutWake => Some(MeerkatInput::AcceptWithoutWake),
        MeerkatMachineCommandVariant::Prepare => Some(MeerkatInput::Prepare),
        MeerkatMachineCommandVariant::Commit => Some(MeerkatInput::Commit),
        MeerkatMachineCommandVariant::Fail => Some(MeerkatInput::Fail),
    }
}

fn mob_command_self_catalog_input(command: MobMachineCommandVariant) -> Option<MobInput> {
    match command {
        MobMachineCommandVariant::RunFlow => Some(MobInput::RunFlow),
        MobMachineCommandVariant::CancelFlow => Some(MobInput::CancelFlow),
        MobMachineCommandVariant::FlowStatus => Some(MobInput::FlowStatus),
        MobMachineCommandVariant::Spawn => Some(MobInput::Spawn),
        MobMachineCommandVariant::EnsureMember => Some(MobInput::EnsureMember),
        MobMachineCommandVariant::Reconcile => Some(MobInput::Reconcile),
        MobMachineCommandVariant::ListMembersMatching => None,
        MobMachineCommandVariant::Retire => Some(MobInput::Retire),
        MobMachineCommandVariant::Respawn => Some(MobInput::Respawn),
        MobMachineCommandVariant::RetireAll => Some(MobInput::RetireAll),
        MobMachineCommandVariant::SubmitWork => Some(MobInput::SubmitWork),
        MobMachineCommandVariant::CancelWork => Some(MobInput::CancelWork),
        MobMachineCommandVariant::CancelAllWork => Some(MobInput::CancelAllWork),
        MobMachineCommandVariant::Stop => Some(MobInput::Stop),
        MobMachineCommandVariant::Resume => Some(MobInput::Resume),
        MobMachineCommandVariant::Complete => Some(MobInput::Complete),
        MobMachineCommandVariant::Reset => Some(MobInput::Reset),
        MobMachineCommandVariant::Destroy => Some(MobInput::Destroy),
        MobMachineCommandVariant::TaskCreate => Some(MobInput::TaskCreate),
        MobMachineCommandVariant::TaskUpdate => Some(MobInput::TaskUpdate),
        MobMachineCommandVariant::TaskList => Some(MobInput::TaskList),
        MobMachineCommandVariant::TaskGet => Some(MobInput::TaskGet),
        MobMachineCommandVariant::McpServerStates => Some(MobInput::McpServerStates),
        MobMachineCommandVariant::RosterSnapshot => Some(MobInput::RosterSnapshot),
        MobMachineCommandVariant::ListMembers => Some(MobInput::ListMembers),
        MobMachineCommandVariant::ListMembersIncludingRetiring => {
            Some(MobInput::ListMembersIncludingRetiring)
        }
        MobMachineCommandVariant::ListAllMembers => Some(MobInput::ListAllMembers),
        MobMachineCommandVariant::MemberStatus => Some(MobInput::MemberStatus),
        MobMachineCommandVariant::SubscribeAgentEvents => Some(MobInput::SubscribeAgentEvents),
        MobMachineCommandVariant::SubscribeAllAgentEvents => {
            Some(MobInput::SubscribeAllAgentEvents)
        }
        MobMachineCommandVariant::SubscribeMobEvents => Some(MobInput::SubscribeMobEvents),
        MobMachineCommandVariant::PollEvents => Some(MobInput::PollEvents),
        MobMachineCommandVariant::ReplayAllEvents => Some(MobInput::ReplayAllEvents),
        MobMachineCommandVariant::RecordOperatorActionProvenance => {
            Some(MobInput::RecordOperatorActionProvenance)
        }
        MobMachineCommandVariant::GetMember => Some(MobInput::GetMember),
        MobMachineCommandVariant::SetSpawnPolicy => Some(MobInput::SetSpawnPolicy),
        MobMachineCommandVariant::Shutdown => Some(MobInput::Shutdown),
        MobMachineCommandVariant::ForceCancel => Some(MobInput::ForceCancel),
        MobMachineCommandVariant::Wire => None,
        MobMachineCommandVariant::Unwire => None,
    }
}

fn assert_meerkat_command_records_are_identity_checked(
    generated_inputs: &BTreeSet<MeerkatMachineInputVariant>,
    records: &[MeerkatMachineCommandClassificationRecord],
) {
    for record in records {
        let command_name = record.command.as_str();
        let command_input =
            meerkat_command_self_catalog_input(record.command).map(MeerkatInput::input_variant);
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
        let command_input =
            mob_command_self_catalog_input(record.command).map(MobInput::input_variant);
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
        !body.contains("=> _") && !body.contains("_ =>"),
        "{path} command classifier must enumerate variants without wildcard fallback"
    );
    assert!(
        !body.contains('"'),
        "{path} command classifier must not contain string-literal command/input whitelists"
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
fn meerkat_machine_inputs_equal_runtime_manifest_exactly() {
    let generated_inputs = generated_meerkat_input_variants();
    let dsl_internal_inputs = meerkat_machine_runtime_internal_input_variants()
        .into_iter()
        .collect::<BTreeSet<_>>();
    let records = canonical_meerkat_machine_command_classifications();
    assert_meerkat_command_records_are_identity_checked(&generated_inputs, &records);
    let runtime_commands = meerkat_runtime_command_input_variants(&records);

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
