//! Live bridge admission captures the exact original work of the dispatching
//! staged run, and the outcome resolves that binding from the runtime's own
//! rows (B7 A).

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::sync::Arc;

use meerkat_authorization_contracts::evidence::EvidenceId;
use meerkat_authorization_contracts::work_association::InputAuthorityAssociation;
use meerkat_core::ControllerModelSelection;
use meerkat_core::lifecycle::{InputId, RunId};
use meerkat_core::retained_work::{RetainedWorkIdentity, SelectedInputBinding};
use meerkat_core::types::SessionId;

use super::MeerkatMachine;
use super::driver::DriverEntry;
use crate::accept::AcceptOutcome;
use crate::identifiers::LogicalRuntimeId;
use crate::input_authority::NativeIngressContext;
use crate::input_authority::tests::TestIngress;
use crate::traits::{RuntimeDriver as _, RuntimeDriverError};

struct Client(ControllerModelSelection);

#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
impl meerkat_core::AgentLlmClient for Client {
    fn controller_model_selection(&self) -> Option<ControllerModelSelection> {
        Some(self.0.clone())
    }
    async fn stream_response(
        &self,
        _: &[meerkat_core::Message],
        _: &[Arc<meerkat_core::ToolDef>],
        _: u32,
        _: Option<f32>,
        _: Option<&meerkat_core::ProviderParamsOverride>,
    ) -> Result<meerkat_core::LlmStreamResult, meerkat_core::AgentError> {
        panic!("the bridge capture fixture never dispatches a model")
    }
    fn provider(&self) -> meerkat_core::Provider {
        self.0.provider()
    }
    fn model(&self) -> &str {
        self.0.model()
    }
}

type Driver = Arc<crate::tokio::sync::Mutex<DriverEntry>>;

/// A governed machine (test ingress host installed before any session) with
/// one session and a bound live context channel.
struct GovernedBridge {
    machine: MeerkatMachine,
    session_id: SessionId,
    host: Arc<TestIngress>,
    driver: Driver,
    runtime_id: LogicalRuntimeId,
    binding: crate::live_execution::LiveDelegationRuntimeBinding,
}

async fn governed_bridge() -> GovernedBridge {
    let machine = MeerkatMachine::ephemeral();
    let host = Arc::new(TestIngress::isolated(machine.generated_auth_lease_handle()));
    let installed: Arc<dyn crate::input_authority::NativeWorkAuthorizationHost> =
        Arc::clone(&host) as Arc<dyn crate::input_authority::NativeWorkAuthorizationHost>;
    let machine = machine
        .with_native_work_authorization_host(installed)
        .expect("configured host");
    let session_id = SessionId::new();
    machine
        .register_session(session_id.clone())
        .await
        .expect("register");
    machine
        .prepare_bindings(session_id.clone())
        .await
        .expect("runtime binding");
    let (driver, runtime_id) = {
        let sessions = machine.sessions.read().await;
        let entry = sessions.get(&session_id).expect("session");
        (Arc::clone(&entry.driver), entry.runtime_id.clone())
    };
    {
        let mut driver = driver.lock().await;
        let DriverEntry::Ephemeral(driver) = &mut *driver else {
            panic!("storeless")
        };
        driver.set_executor_work_authorization_support(true);
    }
    let binding = machine
        .__test_open_live_context_channel(&session_id, 0)
        .await
        .expect("bound live channel");
    GovernedBridge {
        machine,
        session_id,
        host,
        driver,
        runtime_id,
        binding,
    }
}

/// Admit an authenticated original prompt from `requester`, as the governed
/// ingress does, and return its input id.
async fn admit_original(fixture: &GovernedBridge, requester: &str) -> InputId {
    let mut prompt = fixture.host.input(requester);
    // Each original is its own event: the shared test input's fixed
    // idempotency key would deduplicate a second admission.
    prompt.header_mut().idempotency_key = Some(crate::identifiers::IdempotencyKey::new(
        uuid::Uuid::new_v4().to_string(),
    ));
    let ingress = Arc::clone(prompt.header().ingress_context.as_ref().expect("ingress"));
    let mut candidate = prompt
        .header()
        .authority_association
        .as_ref()
        .expect("claims")
        .candidate()
        .clone();
    candidate.target.logical_runtime =
        EvidenceId::new(fixture.runtime_id.to_string()).expect("runtime id");
    let selection = candidate.controller_model.clone().expect("selection");
    prompt.header_mut().authority_association =
        Some(InputAuthorityAssociation::new(candidate).expect("claims"));
    let actual = NativeIngressContext::from_trusted_ingress(
        &prompt,
        ingress.requester().clone(),
        ingress.ingress_actor().clone(),
        ingress.realm().clone(),
        ingress.authentication().clone(),
    )
    .expect("bound submission")
    .with_controller_client(
        &prompt,
        meerkat_core::ControllerModelClient::new(selection.clone(), Arc::new(Client(selection))),
    )
    .expect("same controller");
    let prompt = prompt.with_ingress_context(actual).expect("context");
    let mut driver = fixture.driver.lock().await;
    let DriverEntry::Ephemeral(driver) = &mut *driver else {
        panic!("storeless")
    };
    match driver
        .accept_input(prompt)
        .await
        .expect("original admitted")
    {
        AcceptOutcome::Accepted { state, .. } => state.input_id.clone(),
        other => panic!("original admitted: {other:?}"),
    }
}

/// Stage `inputs` as one run the way the runtime loop does, and return the
/// identity the driver minted for that staged work context.
async fn stage_run(fixture: &GovernedBridge, inputs: &[InputId]) -> RetainedWorkIdentity {
    let run = RunId::new();
    let mut driver = fixture.driver.lock().await;
    let DriverEntry::Ephemeral(driver) = &mut *driver else {
        panic!("storeless")
    };
    let context = driver
        .batch_work_authorization(&run, inputs)
        .expect("staged batch")
        .expect("work context");
    driver
        .contract_begin_run_authority(run.clone())
        .expect("actual run");
    driver
        .machine_realize_authorized_stage_batch(
            crate::meerkat_machine::driver::test_authorized_stage_for_run(
                inputs.to_vec(),
                run.clone(),
            ),
        )
        .expect("staged");
    let minted = context.retained_work().cloned().expect("minted identity");
    assert_eq!(driver.staged_retained_work().as_ref(), Some(&minted));
    minted
}

/// End `run` the way a completed turn does (run terminal, then the service
/// turn commit), so a later run can be prepared and staged.
async fn finish_run(fixture: &GovernedBridge, run: &RunId) {
    use crate::meerkat_machine::dsl as mm;
    let entry = fixture.driver.lock().await;
    let shared = entry.shared_dsl_authority();
    let mut machine = shared.lock().expect("generated owner");
    mm::MeerkatMachineMutator::apply(
        &mut *machine,
        mm::MeerkatMachineInput::RunCompleted {
            run_id: mm::RunId::from_domain(run),
        },
    )
    .expect("run terminal");
    mm::MeerkatMachineMutator::apply(
        &mut *machine,
        mm::MeerkatMachineInput::ServiceTurnCommitted {
            run_id: mm::RunId::from_domain(run),
        },
    )
    .expect("service turn committed");
}

/// Start a provider turn on the bound channel and admit one bridge operation
/// carrying `original_work`.
async fn admit_bridge(
    fixture: &GovernedBridge,
    provider_turn_ref: &str,
    original_work: Option<&RetainedWorkIdentity>,
) -> Result<crate::live_execution::LiveBridgeOperationAdmission, RuntimeDriverError> {
    use crate::meerkat_machine::dsl as mm;
    let binding = &fixture.binding;
    let interaction_id = meerkat_core::InteractionId::new();
    fixture
        .machine
        .apply_session_dsl_input(
            &fixture.session_id,
            mm::MeerkatMachineInput::ObserveLiveProviderTurnStarted {
                channel_id: binding.channel_id().to_string(),
                runtime_id: mm::AgentRuntimeId::from_domain(binding.runtime_id()),
                fence_token: mm::FenceToken(binding.fence_token()),
                generation: mm::Generation(binding.generation()),
                interaction_id: interaction_id.to_string(),
                provider_turn_ref: provider_turn_ref.to_string(),
            },
            "test:ObserveLiveProviderTurnStarted",
        )
        .await
        .expect("provider turn lineage");
    let provider = meerkat_core::LiveBridgeProviderCorrelation::new(
        provider_turn_ref,
        format!("{provider_turn_ref}:delegation"),
        format!("{provider_turn_ref}:call"),
    )
    .expect("provider correlation");
    let correlation = meerkat_core::LiveBridgeOperationCorrelation::new(
        binding.channel_id().clone(),
        interaction_id,
        provider,
    )
    .expect("bridge correlation");
    let canonical_context_revision = meerkat_core::Session::with_id(fixture.session_id.clone())
        .canonical_context_revision()
        .expect("context revision");
    let admission = fixture
        .machine
        .admit_live_bridge_operation(
            &fixture.session_id,
            correlation,
            "test-durable-member",
            &canonical_context_revision,
            meerkat_core::LiveBridgeRequestDigest::derive("check the garden irrigation")
                .expect("digest"),
            original_work,
        )
        .await;
    if admission.is_err() {
        // A refused admission leaves its provider turn active on the channel;
        // complete it so the next case can start its own turn there.
        fixture
            .machine
            .apply_session_dsl_input(
                &fixture.session_id,
                mm::MeerkatMachineInput::CompleteLiveInteraction {
                    channel_id: binding.channel_id().to_string(),
                    runtime_id: mm::AgentRuntimeId::from_domain(binding.runtime_id()),
                    fence_token: mm::FenceToken(binding.fence_token()),
                    generation: mm::Generation(binding.generation()),
                    provider_turn_ref: provider_turn_ref.to_string(),
                },
                "test:CompleteLiveInteraction",
            )
            .await
            .expect("refused turn completes");
    }
    admission
}

/// The admission was refused because the supplied original work is not the
/// dispatch's actual native work identity, and for no other cause.
fn assert_original_work_mismatch(
    admission: Result<crate::live_execution::LiveBridgeOperationAdmission, RuntimeDriverError>,
    case: &str,
) {
    match admission {
        Err(RuntimeDriverError::ValidationFailed { reason }) => assert_eq!(
            reason, "live bridge original work does not match the actual dispatching batch",
            "{case}"
        ),
        Err(other) => panic!("{case}: refused for another cause: {other:?}"),
        Ok(_) => panic!("{case}: admitted"),
    }
}

/// The same identity with one field changed.
fn with(
    identity: &RetainedWorkIdentity,
    run: Option<RunId>,
    contributors: Option<Vec<meerkat_core::retained_work::RetainedContributorRef>>,
    selected: Option<BTreeMap<String, SelectedInputBinding>>,
) -> RetainedWorkIdentity {
    RetainedWorkIdentity::new(
        identity.runtime_id(),
        run.unwrap_or_else(|| identity.run_id().clone()),
        contributors.unwrap_or_else(|| identity.contributors().to_vec()),
        selected.unwrap_or_else(|| identity.selected_input_bindings().clone()),
        identity.controller().cloned(),
    )
}

/// Evidence limit: both batches are staged through the driver and the
/// generated machine only; no Agent or provider execution runs, so the second
/// run proves historical resolution against a later staged run, not against
/// executed work.
#[tokio::test]
async fn admission_records_the_exact_staged_original_work_and_the_outcome_resolves_it() {
    let fixture = governed_bridge().await;
    let first = admit_original(&fixture, "original-requester").await;
    let second = admit_original(&fixture, "original-requester").await;
    let staged = stage_run(&fixture, &[first, second]).await;

    let admission = admit_bridge(&fixture, "provider:turn:1", Some(&staged))
        .await
        .expect("the exact staged batch admits");
    let authority = fixture
        .machine
        .live_bridge_outcome_authority(&fixture.session_id, admission.operation())
        .await
        .expect("the recorded binding resolves");
    assert_eq!(authority.identity(), &staged);
    assert_eq!(authority.contributors().len(), staged.contributors().len());

    // The original run completes and a real second run is staged with
    // unrelated fresh work: the completed original still resolves from its
    // recorded binding, never from the current run.
    finish_run(&fixture, staged.run_id()).await;
    let fresh = admit_original(&fixture, "original-requester").await;
    let later = stage_run(&fixture, &[fresh]).await;
    assert_ne!(later.run_id(), staged.run_id());
    let again = fixture
        .machine
        .live_bridge_outcome_authority(&fixture.session_id, admission.operation())
        .await
        .expect("the completed original still resolves");
    assert_eq!(again.identity(), &staged);
    assert_ne!(again.identity(), &later);
}

#[tokio::test]
async fn admission_refuses_anything_but_the_exact_dispatching_batch() {
    let fixture = governed_bridge().await;
    let first = admit_original(&fixture, "original-requester").await;
    let second = admit_original(&fixture, "original-requester").await;
    let staged = stage_run(&fixture, &[first, second]).await;

    // A changed run with the same rows.
    let changed_run = with(&staged, Some(RunId::new()), None, None);
    assert_original_work_mismatch(
        admit_bridge(&fixture, "provider:turn:run", Some(&changed_run)).await,
        "provider:turn:run",
    );
    // A subset of the contributors.
    let mut subset = staged.contributors().to_vec();
    subset.pop();
    let subset = with(&staged, None, Some(subset), None);
    assert_original_work_mismatch(
        admit_bridge(&fixture, "provider:turn:subset", Some(&subset)).await,
        "provider:turn:subset",
    );
    // The contributors reordered.
    let mut reordered = staged.contributors().to_vec();
    reordered.reverse();
    let reordered = with(&staged, None, Some(reordered), None);
    assert_original_work_mismatch(
        admit_bridge(&fixture, "provider:turn:reorder", Some(&reordered)).await,
        "provider:turn:reorder",
    );
    // A changed selected binding.
    let mut selected = staged.selected_input_bindings().clone();
    let first_key = selected.keys().next().cloned().expect("selected");
    selected.get_mut(&first_key).expect("selected").binding = "another-binding".into();
    let changed_binding = with(&staged, None, None, Some(selected));
    assert_original_work_mismatch(
        admit_bridge(&fixture, "provider:turn:binding", Some(&changed_binding)).await,
        "provider:turn:binding",
    );
    // No original work on a governed runtime.
    assert_original_work_mismatch(
        admit_bridge(&fixture, "provider:turn:none", None).await,
        "provider:turn:none",
    );
    // The exact batch still admits.
    assert!(
        admit_bridge(&fixture, "provider:turn:exact", Some(&staged))
            .await
            .is_ok()
    );
}

/// Claim limited to a separately constructed owner of another session (its
/// runtime differs). Same-session owner replacement is explicitly untested.
#[tokio::test]
async fn another_sessions_owner_cannot_admit_with_a_copied_identity() {
    let issuing = governed_bridge().await;
    let original = admit_original(&issuing, "original-requester").await;
    let staged = stage_run(&issuing, &[original]).await;
    // A separately constructed owner never staged this batch.
    let replacement = governed_bridge().await;
    assert_original_work_mismatch(
        admit_bridge(&replacement, "provider:turn:copied", Some(&staged)).await,
        "provider:turn:copied",
    );
}

#[tokio::test]
async fn an_operation_admitted_without_original_work_has_no_governed_outcome() {
    // No host: admission takes no identity and records none.
    let machine = MeerkatMachine::ephemeral();
    let session_id = SessionId::new();
    machine
        .register_session(session_id.clone())
        .await
        .expect("register");
    machine
        .prepare_bindings(session_id.clone())
        .await
        .expect("runtime binding");
    let binding = machine
        .__test_open_live_context_channel(&session_id, 0)
        .await
        .expect("bound live channel");
    let driver = Arc::clone(
        &machine
            .sessions
            .read()
            .await
            .get(&session_id)
            .expect("entry")
            .driver,
    );
    let fixture = GovernedBridge {
        host: Arc::new(TestIngress::isolated(machine.generated_auth_lease_handle())),
        driver,
        runtime_id: MeerkatMachine::logical_runtime_id(&session_id),
        machine,
        session_id,
        binding,
    };
    let admission = admit_bridge(&fixture, "provider:turn:legacy", None)
        .await
        .expect("ungoverned admission");
    assert!(
        fixture
            .machine
            .live_bridge_outcome_authority(&fixture.session_id, admission.operation())
            .await
            .is_err()
    );
}

/// A legacy operation (recorded without original work, as before the field
/// existed) replayed by a governed caller: missing evidence, not a mismatch.
/// The replay acquires no authority and fills nothing in, the channel stays
/// live, and the governed outcome stays unavailable.
#[tokio::test]
async fn a_legacy_operation_replay_keeps_the_channel_and_hydrates_nothing() {
    use crate::meerkat_machine::dsl as mm;
    let fixture = governed_bridge().await;
    let original = admit_original(&fixture, "original-requester").await;
    let staged = stage_run(&fixture, &[original]).await;
    let binding = &fixture.binding;
    let provider_turn_ref = "provider:turn:legacy";
    let interaction_id = meerkat_core::InteractionId::new();
    fixture
        .machine
        .apply_session_dsl_input(
            &fixture.session_id,
            mm::MeerkatMachineInput::ObserveLiveProviderTurnStarted {
                channel_id: binding.channel_id().to_string(),
                runtime_id: mm::AgentRuntimeId::from_domain(binding.runtime_id()),
                fence_token: mm::FenceToken(binding.fence_token()),
                generation: mm::Generation(binding.generation()),
                interaction_id: interaction_id.to_string(),
                provider_turn_ref: provider_turn_ref.to_string(),
            },
            "test:ObserveLiveProviderTurnStarted",
        )
        .await
        .expect("provider turn lineage");
    let canonical_context_revision = meerkat_core::Session::with_id(fixture.session_id.clone())
        .canonical_context_revision()
        .expect("context revision");
    let request_digest =
        meerkat_core::LiveBridgeRequestDigest::derive("check the garden irrigation")
            .expect("digest");
    let legacy_operation = meerkat_core::OperationId::new();
    // Recorded the way a pre-upgrade machine recorded it: no original work.
    fixture
        .machine
        .apply_session_dsl_input(
            &fixture.session_id,
            mm::MeerkatMachineInput::AdmitLiveBridgeOperation {
                session_id: fixture.session_id.to_string(),
                channel_id: binding.channel_id().to_string(),
                runtime_id: mm::AgentRuntimeId::from_domain(binding.runtime_id()),
                fence_token: mm::FenceToken(binding.fence_token()),
                generation: mm::Generation(binding.generation()),
                interaction_id: interaction_id.to_string(),
                operation_id: mm::OperationId::from_domain(&legacy_operation),
                provider_turn_ref: provider_turn_ref.to_string(),
                provider_delegation_ref: format!("{provider_turn_ref}:delegation"),
                provider_call_ref: format!("{provider_turn_ref}:call"),
                agent_identity: mm::AgentIdentity::from("test-durable-member"),
                canonical_context_revision: canonical_context_revision.as_str().to_string(),
                request_digest: request_digest.as_str().to_string(),
                original_work: String::new(),
                structural_lineage_proven: true,
            },
            "test:LegacyAdmitLiveBridgeOperation",
        )
        .await
        .expect("legacy operation recorded");

    // The same call replayed by a governed caller with an exact identity.
    let provider = meerkat_core::LiveBridgeProviderCorrelation::new(
        provider_turn_ref,
        format!("{provider_turn_ref}:delegation"),
        format!("{provider_turn_ref}:call"),
    )
    .expect("provider correlation");
    let correlation = meerkat_core::LiveBridgeOperationCorrelation::new(
        binding.channel_id().clone(),
        interaction_id,
        provider,
    )
    .expect("bridge correlation");
    let replay = fixture
        .machine
        .admit_live_bridge_operation(
            &fixture.session_id,
            correlation.clone(),
            "test-durable-member",
            &canonical_context_revision,
            request_digest,
            Some(&staged),
        )
        .await;
    assert!(replay.is_err(), "a replay acquires no execution authority");

    let state = fixture
        .machine
        .session_dsl_state(&fixture.session_id)
        .await
        .expect("state");
    let channel = binding.channel_id().to_string();
    assert_eq!(
        state.live_execution_phase_by_channel.get(&channel).copied(),
        Some(mm::LiveExecutionChannelPhase::Active),
        "missing evidence never revokes the channel"
    );
    assert!(!state.live_revoked_execution_channels.contains(&channel));
    let legacy_key = mm::OperationId::from_domain(&legacy_operation);
    assert!(
        state
            .live_bridge_channel_by_operation
            .contains_key(&legacy_key)
    );
    assert!(
        !state
            .live_bridge_original_work_by_operation
            .contains_key(&legacy_key),
        "a replay never fills in a missing identity"
    );
    let legacy = meerkat_core::exact_operation::ExactOperationIdentity::for_domain(
        legacy_operation,
        correlation,
    );
    assert!(
        fixture
            .machine
            .live_bridge_outcome_authority(&fixture.session_id, &legacy)
            .await
            .is_err(),
        "a legacy replay is never authority for a governed outcome"
    );
}

/// With a host installed but no staged run, the dispatch has no native work
/// identity: it admits without one and refuses one supplied anyway. Whether
/// the caller supplied an identity never chooses the profile.
#[tokio::test]
async fn an_ungoverned_dispatch_on_a_governed_runtime_admits_without_original_work() {
    let fixture = governed_bridge().await;
    let original = admit_original(&fixture, "original-requester").await;
    // A staged identity of another fixture's run is still not this
    // dispatch's native work identity.
    let elsewhere = governed_bridge().await;
    let other = admit_original(&elsewhere, "original-requester").await;
    let foreign = stage_run(&elsewhere, &[other]).await;
    let _ = original;
    assert_original_work_mismatch(
        admit_bridge(&fixture, "provider:turn:supplied", Some(&foreign)).await,
        "provider:turn:supplied",
    );
    let admission = admit_bridge(&fixture, "provider:turn:ungoverned", None)
        .await
        .expect("no native work identity: ungoverned dispatch");
    assert!(
        !fixture
            .machine
            .live_bridge_operation_has_original_work(&fixture.session_id, admission.operation())
            .await
            .expect("operation present")
    );
}

/// A persisted identity that does not parse (injected through the generated
/// input as recovery would load it) stays present for profile purposes (the
/// operation counts as governed, so it is never downgraded to a plain append)
/// but is refused on decode: the governed outcome is unavailable.
#[tokio::test]
async fn a_malformed_persisted_identity_is_unavailable() {
    use crate::meerkat_machine::dsl as mm;
    let fixture = governed_bridge().await;
    let binding = &fixture.binding;
    let provider_turn_ref = "provider:turn:malformed";
    let interaction_id = meerkat_core::InteractionId::new();
    fixture
        .machine
        .apply_session_dsl_input(
            &fixture.session_id,
            mm::MeerkatMachineInput::ObserveLiveProviderTurnStarted {
                channel_id: binding.channel_id().to_string(),
                runtime_id: mm::AgentRuntimeId::from_domain(binding.runtime_id()),
                fence_token: mm::FenceToken(binding.fence_token()),
                generation: mm::Generation(binding.generation()),
                interaction_id: interaction_id.to_string(),
                provider_turn_ref: provider_turn_ref.to_string(),
            },
            "test:ObserveLiveProviderTurnStarted",
        )
        .await
        .expect("provider turn lineage");
    let operation = meerkat_core::OperationId::new();
    let canonical_context_revision = meerkat_core::Session::with_id(fixture.session_id.clone())
        .canonical_context_revision()
        .expect("context revision");
    fixture
        .machine
        .apply_session_dsl_input(
            &fixture.session_id,
            mm::MeerkatMachineInput::AdmitLiveBridgeOperation {
                session_id: fixture.session_id.to_string(),
                channel_id: binding.channel_id().to_string(),
                runtime_id: mm::AgentRuntimeId::from_domain(binding.runtime_id()),
                fence_token: mm::FenceToken(binding.fence_token()),
                generation: mm::Generation(binding.generation()),
                interaction_id: interaction_id.to_string(),
                operation_id: mm::OperationId::from_domain(&operation),
                provider_turn_ref: provider_turn_ref.to_string(),
                provider_delegation_ref: format!("{provider_turn_ref}:delegation"),
                provider_call_ref: format!("{provider_turn_ref}:call"),
                agent_identity: mm::AgentIdentity::from("test-durable-member"),
                canonical_context_revision: canonical_context_revision.as_str().to_string(),
                request_digest: meerkat_core::LiveBridgeRequestDigest::derive("check")
                    .expect("digest")
                    .as_str()
                    .to_string(),
                original_work: "{not a retained work identity".to_string(),
                structural_lineage_proven: true,
            },
            "test:MalformedOriginalWork",
        )
        .await
        .expect("operation recorded");
    let provider = meerkat_core::LiveBridgeProviderCorrelation::new(
        provider_turn_ref,
        format!("{provider_turn_ref}:delegation"),
        format!("{provider_turn_ref}:call"),
    )
    .expect("provider correlation");
    let correlation = meerkat_core::LiveBridgeOperationCorrelation::new(
        binding.channel_id().clone(),
        interaction_id,
        provider,
    )
    .expect("bridge correlation");
    let malformed =
        meerkat_core::exact_operation::ExactOperationIdentity::for_domain(operation, correlation);
    assert!(
        fixture
            .machine
            .live_bridge_operation_has_original_work(&fixture.session_id, &malformed)
            .await
            .expect("operation present"),
        "a binding is recorded"
    );
    assert!(
        fixture
            .machine
            .live_bridge_outcome_authority(&fixture.session_id, &malformed)
            .await
            .is_err(),
        "an unparseable binding is unavailable"
    );
}

/// Infrastructure that can clear on its own stays transient; an absent,
/// malformed or refused binding is unavailable.
#[test]
fn binding_errors_keep_infrastructure_transient_and_absent_authority_unavailable() {
    use crate::live_execution::LiveBridgeOutcomeBindingError as B;
    let transient = [
        RuntimeDriverError::NotReady {
            state: crate::runtime_state::RuntimeState::Destroyed,
        },
        RuntimeDriverError::Internal("store read failed".into()),
        RuntimeDriverError::RecoveryBackoff {
            reason: "owner recovering".into(),
        },
        // Keeps live reconciliation and retry by contract.
        RuntimeDriverError::InterruptDispatchOutcomeUnknown {
            run_id: RunId::new(),
            reason: "acknowledgement pending".into(),
        },
        // A callback unwind releases the retry slot for the same run.
        RuntimeDriverError::InterruptDispatchPanicked {
            run_id: RunId::new(),
            reason: "callback unwound".into(),
        },
        // The session's hosting claim clears once it can be taken.
        RuntimeDriverError::HostingUnavailable {
            session_id: SessionId::new(),
        },
    ];
    for error in transient {
        assert!(matches!(B::from(error), B::Transient(_)));
    }
    let unavailable = [
        crate::input_authority::unavailable(),
        RuntimeDriverError::RetainedResumeRefused {
            reason: crate::retained_work::RetainedResumeRefusal::NoAdmissibleWorkBinding,
        },
        RuntimeDriverError::Destroyed,
        // Superseded exact witnesses: a same-state retry is wrong.
        RuntimeDriverError::StaleAuthority {
            reason: "superseded witness".into(),
        },
        RuntimeDriverError::MaterializationRegistrationNotCurrent {
            session_id: SessionId::new(),
        },
        // This runtime no longer hosts the session.
        RuntimeDriverError::ServedElsewhere {
            session_id: SessionId::new(),
        },
        // A broken hosting invariant, as recovery corruption.
        RuntimeDriverError::HostingClaimInvariantViolated {
            runtime_id: "runtime".into(),
            input_id: "input".into(),
            idempotency_key: None,
            constraint: crate::store::InputIdempotencyIndexConstraint::RuntimeKey,
        },
    ];
    for error in unavailable {
        assert!(matches!(B::from(error), B::Unavailable(_)));
    }
}
