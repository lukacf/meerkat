//! D50 S3: the scoped session document destination on the real native loop.
//! The same grant, admission and Agent owners as the parent loop test; the
//! machine binds the exact destination before work arrives.

use super::*;
use meerkat_runtime::RuntimeDriverError;
use meerkat_runtime::completion::CompletionWaitError;
use meerkat_runtime::terminal_status::InteractionSelector;

fn governed_input(
    runtime: &LogicalRuntimeId,
    controller: &GrantLineageRef,
    operation: &GrantLineageRef,
    pin: &meerkat_core::ControllerModelClient,
    retarget: impl FnOnce(&mut NativeWorkTarget),
) -> Input {
    let mut candidate = association(
        runtime,
        controller.clone(),
        operation.clone(),
        pin.selection().clone(),
    )
    .candidate()
    .clone();
    retarget(&mut candidate.target);
    let mut prompt = PromptInput::new("Try delete, then read if the operation is refused", None);
    prompt.header.authority_association =
        Some(InputAuthorityAssociation::new(candidate).expect("claims"));
    let input = Input::Prompt(prompt);
    let current = NativeIngressContext::from_trusted_ingress(
        &input,
        principal("requester"),
        principal("ingress"),
        RealmId::parse("native-loop").expect("realm"),
        evidence("fresh-transport-authentication"),
    )
    .expect("current trusted ingress observation")
    .with_controller_client(&input, pin.clone())
    .expect("bind real pin before native acceptance");
    input
        .with_ingress_context(current)
        .expect("exact finalized input binding")
}

/// The run that consumed `input_id`, from its row-bound audit.
async fn run_of(
    machine: &MeerkatMachine,
    session_id: &SessionId,
    input_id: &meerkat_core::InputId,
) -> meerkat_core::RunId {
    let stored = machine
        .input_state(session_id, input_id)
        .await
        .expect("actual native row query")
        .expect("contributor row");
    let wire = serde_json::to_value(&stored).expect("row serialization");
    let audit: Vec<StoredAuthorizationAuditObservation> =
        serde_json::from_value(wire["authorization_audit"].clone()).expect("audit");
    audit[0].observation.run_id.clone().expect("admitted run")
}

/// Input -> model -> refused tool + permitted sibling -> model -> reply, all
/// under the machine-bound destination; a governed input naming another
/// destination, or an older generation after a rebind, is refused at
/// admission without touching the session.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scoped_destination_carries_a_native_turn_and_refuses_other_destinations() {
    let grants = Arc::new(
        LocalGrantAuthority::new(
            LocalGrantConfiguration {
                root: principal("grant-owner"),
                namespace: id("native-grants"),
                generation: 1,
            },
            LocalAuthorizationPublication::new(),
            Arc::new(HostAuthorizationClock),
        )
        .expect("actual generated grant owner"),
    );
    let controller = grants
        .issue_root(
            &principal("grant-owner"),
            id("controller"),
            principal("executor"),
            None,
            ceiling("infer"),
        )
        .expect("actual controller lineage with unrestricted lifetime");
    let operation = grants
        .issue_root(
            &principal("grant-owner"),
            id("read-only"),
            principal("executor"),
            None,
            ceiling("read"),
        )
        .expect("actual read-only operation grant");
    let expected_controller = controller.clone();
    let expected_operation = operation.clone();
    let ingress: Arc<NativeIngressCheck> = Arc::new(move |runtime, _, current, claimed| {
        let candidate = claimed.candidate();
        if current.requester() != &principal("requester")
            || current.ingress_actor() != &principal("ingress")
            || current.realm() != &RealmId::parse("native-loop").expect("realm")
            || candidate.logical_executor != principal("executor")
            || candidate.represented_subject.is_some()
            || candidate.target.logical_runtime != id(&runtime.to_string())
            || candidate.controller_grant_lineage != [expected_controller.clone()]
            || candidate.authority_basis
                != (WorkAuthorityBasis::GrantLineage {
                    lineage: vec![expected_operation.clone()],
                })
            || candidate.controller_model.as_ref() != Some(&selection())
            || candidate.controller_ceiling != ceiling("infer")
        {
            return Err(denied().into());
        }
        Ok(())
    });
    let machine = Arc::new(
        MeerkatMachine::ephemeral()
            .with_local_grant_authorization(NativeGrantWorkConfiguration {
                retained_resume: None,
                require_scoped_destination: false,
                grants,
                ingress,
                invocation_owner: Arc::new(InvocationOwner),
                operation_owner: Arc::new(RecordOwner),
                configured_mcp_attachment: None,
            })
            .expect("exclusive actual native host setup"),
    );
    let provider = Arc::new(ScriptedProvider::default());
    let tools = Arc::new(RecordingTools::default());
    let mut builder = FactoryAgentBuilder::new(AgentFactory::minimal(), Config::default());
    builder.default_llm_client = Some(provider.clone());
    builder.default_tool_dispatcher = Some(tools.clone());
    builder.default_session_store = Some(Arc::new(RecordingStore::default()));
    let service = Arc::new(EphemeralSessionService::new(builder, 2));
    let created = meerkat::surface::materialize_ephemeral_runtime_session(
        &service,
        &machine,
        CreateSessionRequest {
            model: MODEL.into(),
            prompt: "".into(),
            injected_context: Vec::new(),
            system_prompt: meerkat_core::config::SystemPromptOverride::Inherit,
            max_tokens: None,
            event_tx: None,
            initial_turn: InitialTurnPolicy::Defer,
            deferred_prompt_policy: DeferredPromptPolicy::Discard,
            build: None,
            labels: None,
        },
        false,
    )
    .await
    .expect("real native executor and deferred Agent actor");
    let session_id = created.session_id;
    let actor = service
        .live_session_actor_witness(&session_id)
        .await
        .expect("actual live actor");
    let pin = service
        .pin_controller_client_for_actor(&actor)
        .await
        .expect("actual immutable selected LlmClientAdapter");
    assert_eq!(pin.selection(), &selection());
    // This scripted transport has no physical vault or external credential.
    // Native readiness still uses the actual machine's generated owner and the
    // exact credential identity pinned by the real adapter.
    let credential_owner = machine.generated_auth_lease_handle();
    meerkat_core::publish_token_lifecycle_acquired_for_identity_within(
        &credential_owner,
        pin.selection().credential(),
        &meerkat_core::auth::PersistedTokens::api_key("synthetic-native-loop-credential"),
        tokio::time::Instant::now() + CREDENTIAL_PUBLICATION_WAIT,
    )
    .await
    .expect("actual generated fixture credential before native admission");
    assert_eq!(
        credential_owner
            .resolve_credential_use_admission(
                &meerkat_core::handles::LeaseKey::from_credential_identity(
                    pin.selection().credential()
                ),
                meerkat_core::handles::CredentialUseIntent::HoldAuthority,
            )
            .expect("actual generated readiness"),
        meerkat_core::handles::CredentialUseDisposition::Authorized
    );
    let runtime = LogicalRuntimeId::for_session(&session_id);
    let registration = machine
        .current_session_registration_witness(&session_id)
        .await
        .expect("current registration");
    let target = association(
        &runtime,
        controller.clone(),
        operation.clone(),
        pin.selection().clone(),
    )
    .candidate()
    .target
    .clone();
    machine
        .bind_scoped_context(&registration, &target)
        .await
        .expect("bind the session document's destination");

    let foreign = governed_input(&runtime, &controller, &operation, &pin, |target| {
        target.context = id("another-context");
    });
    assert!(
        machine
            .accept_input_with_completion(&session_id, foreign)
            .await
            .is_err(),
        "a governed input naming another destination is refused at admission"
    );

    let input = governed_input(&runtime, &controller, &operation, &pin, |_| {});
    let input_id = input.id().clone();
    let (_, completion) = machine
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("admitted for the bound destination");
    let completion = completion.expect("actual native completion waiter");
    tokio::time::timeout(Duration::from_secs(20), provider.ready_to_finish.notified())
        .await
        .expect("same native run reached the third controller request");
    assert_eq!(*tools.0.lock().expect("body entries"), ["read_record"]);
    provider.finish.notify_one();
    let outcome = tokio::time::timeout(Duration::from_secs(20), completion.wait())
        .await
        .expect("native run completes")
        .expect("native completion observation");
    let CompletionOutcome::Completed(result) = outcome else {
        panic!("the bound destination releases its reply: {outcome:?}")
    };
    assert_eq!(result.text, "read completed after refusal");
    let Ok(Some(CompletionOutcome::Completed(read))) = machine
        .input_terminal_completion(&session_id, &input_id)
        .await
    else {
        panic!("the current destination reads its result")
    };
    assert_eq!(read.text, "read completed after refusal");
    assert!(
        matches!(
            machine
                .input_terminal_receipt(&session_id, InteractionSelector::InputId(input_id.clone()))
                .await,
            Ok(Some(_))
        ),
        "the current destination reads its receipt"
    );
    let row = machine
        .input_state(&session_id, &input_id)
        .await
        .expect("actual native row query")
        .expect("the consumed row");
    assert!(
        serde_json::to_string(&row)
            .expect("row serialization")
            .contains("read completed after refusal"),
        "the current destination reads its row's result"
    );
    let run = run_of(&machine, &session_id, &input_id).await;
    machine
        .authorize_scoped_reply(&registration, &run, &target)
        .await
        .expect("the reply may be disclosed to the current destination");
    let mut other_audience = target.clone();
    other_audience.audience = AudienceRef::Principal {
        principal: principal("someone-else"),
    };
    assert!(
        machine
            .authorize_scoped_reply(&registration, &run, &other_audience)
            .await
            .is_err(),
        "never to another audience"
    );

    // Rebind to the next generation: work naming the previous one is refused.
    let mut next = target.clone();
    next.context_generation += 1;
    machine
        .bind_scoped_context(&registration, &next)
        .await
        .expect("rebind with an advanced generation");
    let stale = governed_input(&runtime, &controller, &operation, &pin, |_| {});
    assert!(
        machine
            .accept_input_with_completion(&session_id, stale)
            .await
            .is_err(),
        "work naming the previous generation is refused"
    );
}

/// The destination is retired while the run waits for its last model
/// response (already prepared): the run's known result is preserved and
/// checkpointed, the executor is not stopped and the session registration
/// stays. The reply-release owner refuses disclosing it to the retired
/// destination, and so does every public result reader (the completion
/// waiter, after its cleanup relay ran on the body-free cleanup evidence;
/// the terminal completion and receipt reads and the receipt wait), also
/// after the same scope is rebound to its next generation. The refusal is
/// typed; it neither fails the input nor reports a waiter failure.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_retired_destination_keeps_the_known_result_and_the_executor() {
    let grants = Arc::new(
        LocalGrantAuthority::new(
            LocalGrantConfiguration {
                root: principal("grant-owner"),
                namespace: id("native-grants"),
                generation: 1,
            },
            LocalAuthorizationPublication::new(),
            Arc::new(HostAuthorizationClock),
        )
        .expect("actual generated grant owner"),
    );
    let controller = grants
        .issue_root(
            &principal("grant-owner"),
            id("controller"),
            principal("executor"),
            None,
            ceiling("infer"),
        )
        .expect("actual controller lineage with unrestricted lifetime");
    let operation = grants
        .issue_root(
            &principal("grant-owner"),
            id("read-only"),
            principal("executor"),
            None,
            ceiling("read"),
        )
        .expect("actual read-only operation grant");
    let expected_controller = controller.clone();
    let expected_operation = operation.clone();
    let ingress: Arc<NativeIngressCheck> = Arc::new(move |runtime, _, current, claimed| {
        let candidate = claimed.candidate();
        if current.requester() != &principal("requester")
            || current.ingress_actor() != &principal("ingress")
            || current.realm() != &RealmId::parse("native-loop").expect("realm")
            || candidate.logical_executor != principal("executor")
            || candidate.represented_subject.is_some()
            || candidate.target.logical_runtime != id(&runtime.to_string())
            || candidate.controller_grant_lineage != [expected_controller.clone()]
            || candidate.authority_basis
                != (WorkAuthorityBasis::GrantLineage {
                    lineage: vec![expected_operation.clone()],
                })
            || candidate.controller_model.as_ref() != Some(&selection())
            || candidate.controller_ceiling != ceiling("infer")
        {
            return Err(denied().into());
        }
        Ok(())
    });
    let machine = Arc::new(
        MeerkatMachine::ephemeral()
            .with_local_grant_authorization(NativeGrantWorkConfiguration {
                retained_resume: None,
                require_scoped_destination: false,
                grants,
                ingress,
                invocation_owner: Arc::new(InvocationOwner),
                operation_owner: Arc::new(RecordOwner),
                configured_mcp_attachment: None,
            })
            .expect("exclusive actual native host setup"),
    );
    let provider = Arc::new(ScriptedProvider::default());
    let tools = Arc::new(RecordingTools::default());
    let mut builder = FactoryAgentBuilder::new(AgentFactory::minimal(), Config::default());
    builder.default_llm_client = Some(provider.clone());
    builder.default_tool_dispatcher = Some(tools.clone());
    let checkpoints = Arc::new(RecordingStore::default());
    builder.default_session_store = Some(checkpoints.clone());
    let service = Arc::new(EphemeralSessionService::new(builder, 2));
    let created = meerkat::surface::materialize_ephemeral_runtime_session(
        &service,
        &machine,
        CreateSessionRequest {
            model: MODEL.into(),
            prompt: "".into(),
            injected_context: Vec::new(),
            system_prompt: meerkat_core::config::SystemPromptOverride::Inherit,
            max_tokens: None,
            event_tx: None,
            initial_turn: InitialTurnPolicy::Defer,
            deferred_prompt_policy: DeferredPromptPolicy::Discard,
            build: None,
            labels: None,
        },
        false,
    )
    .await
    .expect("real native executor and deferred Agent actor");
    let session_id = created.session_id;
    let actor = service
        .live_session_actor_witness(&session_id)
        .await
        .expect("actual live actor");
    let pin = service
        .pin_controller_client_for_actor(&actor)
        .await
        .expect("actual immutable selected LlmClientAdapter");
    assert_eq!(pin.selection(), &selection());
    // This scripted transport has no physical vault or external credential.
    // Native readiness still uses the actual machine's generated owner and the
    // exact credential identity pinned by the real adapter.
    let credential_owner = machine.generated_auth_lease_handle();
    meerkat_core::publish_token_lifecycle_acquired_for_identity_within(
        &credential_owner,
        pin.selection().credential(),
        &meerkat_core::auth::PersistedTokens::api_key("synthetic-native-loop-credential"),
        tokio::time::Instant::now() + CREDENTIAL_PUBLICATION_WAIT,
    )
    .await
    .expect("actual generated fixture credential before native admission");
    assert_eq!(
        credential_owner
            .resolve_credential_use_admission(
                &meerkat_core::handles::LeaseKey::from_credential_identity(
                    pin.selection().credential()
                ),
                meerkat_core::handles::CredentialUseIntent::HoldAuthority,
            )
            .expect("actual generated readiness"),
        meerkat_core::handles::CredentialUseDisposition::Authorized
    );
    let runtime = LogicalRuntimeId::for_session(&session_id);
    let registration = machine
        .current_session_registration_witness(&session_id)
        .await
        .expect("current registration");
    let target = association(
        &runtime,
        controller.clone(),
        operation.clone(),
        pin.selection().clone(),
    )
    .candidate()
    .target
    .clone();
    machine
        .bind_scoped_context(&registration, &target)
        .await
        .expect("bind the session document's destination");

    let input = governed_input(&runtime, &controller, &operation, &pin, |_| {});
    let input_id = input.id().clone();
    let (_, completion) = machine
        .accept_input_with_completion(&session_id, input)
        .await
        .expect("admitted for the bound destination");
    let (cleanup_tx, cleanup_rx) = tokio::sync::oneshot::channel();
    let completion = completion
        .expect("actual native completion waiter")
        .with_completion_cleanup(move |observation| async move {
            let _ = cleanup_tx.send(observation.is_ok());
        });
    tokio::time::timeout(Duration::from_secs(20), provider.ready_to_finish.notified())
        .await
        .expect("the run reached its last controller request");
    machine
        .retire_scoped_context(&registration, target.context_generation)
        .await
        .expect("retire the destination mid-run");
    provider.finish.notify_one();
    let outcome = tokio::time::timeout(Duration::from_secs(20), completion.wait())
        .await
        .expect("the run settles");
    assert!(
        matches!(outcome, Err(CompletionWaitError::DisclosureRefused { .. })),
        "the retired destination's waiter receives a typed refusal: {outcome:?}"
    );
    assert!(
        cleanup_rx.await.expect("the cleanup relay ran"),
        "the relay's cleanup observed the generated cleanup evidence"
    );
    // The known result is preserved: the run's checkpoint holds it.
    let checkpoint = checkpoints
        .load(&session_id.to_string())
        .await
        .expect("fixture checkpoint read")
        .expect("the run's checkpoint");
    assert!(
        format!("{checkpoint:?}").contains("read completed after refusal"),
        "the known result is checkpointed"
    );
    // The public row read keeps the lifecycle facts but not the payload.
    let row = machine
        .input_state(&session_id, &input_id)
        .await
        .expect("actual native row query")
        .expect("the consumed row");
    let wire = serde_json::to_value(&row).expect("row serialization");
    assert!(!wire["terminal_outcome"].is_null(), "the input is terminal");
    assert!(
        !wire["terminal_completion"].is_null(),
        "its finalized receipt identity stays"
    );
    assert!(
        !wire.to_string().contains("read completed after refusal"),
        "the public row read withholds the result: {wire}"
    );
    assert_eq!(wire["scoped_destination"]["kind"], "bound");
    let assert_reads_withheld = |reads: [Result<(), RuntimeDriverError>; 3], when: &str| {
        for read in reads {
            assert!(
                matches!(
                    &read,
                    Err(RuntimeDriverError::ScopedResultWithheld { input_id: withheld })
                        if withheld == &input_id
                ),
                "{when}: {read:?}"
            );
        }
    };
    let reads = async || {
        [
            machine
                .input_terminal_completion(&session_id, &input_id)
                .await
                .map(drop),
            machine
                .input_terminal_receipt(&session_id, InteractionSelector::InputId(input_id.clone()))
                .await
                .map(drop),
            machine
                .wait_input_terminal_receipt(&session_id, &input_id)
                .await
                .map(drop),
        ]
    };
    assert_reads_withheld(reads().await, "retired destination");
    let run = run_of(&machine, &session_id, &input_id).await;
    assert!(
        machine
            .authorize_scoped_reply(&registration, &run, &target)
            .await
            .is_err(),
        "the retired destination receives no disclosure of the preserved result"
    );
    let mut next = target.clone();
    next.context_generation += 1;
    machine
        .bind_scoped_context(&registration, &next)
        .await
        .expect("rebind the same scope at its next generation");
    assert_reads_withheld(
        reads().await,
        "another current destination of the same scope",
    );
    assert!(
        machine
            .current_session_registration_witness(&session_id)
            .await
            .is_some(),
        "the session registration survives"
    );
}
