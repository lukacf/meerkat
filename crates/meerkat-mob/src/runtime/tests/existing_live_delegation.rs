use super::*;
use meerkat_runtime::live_execution::{
    LiveDelegationCancellationDirective, LiveDelegationCancellationOutcome,
    LiveDelegationExecutionAdmission, LiveDelegationRecoveryPhase, LiveDelegationWorkerOwnership,
    LiveDelegationWorkerTerminalKind,
};

const TASK: &str = "answer the confirmed voice request";

fn execution_context() -> String {
    crate::runtime::delegation::render_live_delegation_execution_context(TASK)
}

#[cfg(not(target_arch = "wasm32"))]
struct CanonicalTranscriptTestRoot(std::path::PathBuf);

#[cfg(not(target_arch = "wasm32"))]
impl Drop for CanonicalTranscriptTestRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[tokio::test]
async fn existing_member_committed_voice_utterance_has_one_conversational_user_row() {
    let root = CanonicalTranscriptTestRoot(std::path::PathBuf::from(format!(
        ".existing-live-canonical-{}",
        uuid::Uuid::new_v4(),
    )));
    std::fs::create_dir(&root.0).expect("create isolated project-local test root");
    for directory in ["user", "runtime", "project", "context"] {
        std::fs::create_dir(root.0.join(directory)).expect("create explicit factory root");
    }
    let factory = meerkat::AgentFactory::new(root.0.join("factory"))
        .user_config_root(root.0.join("user"))
        .runtime_root(root.0.join("runtime"))
        .project_root(root.0.join("project"))
        .context_root(root.0.join("context"))
        .builtins(false)
        .comms(true);
    let mut builder = meerkat::FactoryAgentBuilder::new(factory, meerkat::Config::default());
    let client = HeadCanonicalQueueGateClient::new();
    client.release();
    builder.default_llm_client = Some(Arc::new(client));
    let session_store = Arc::new(meerkat_store::MemoryStore::new());
    builder.default_session_store = Some(Arc::new(meerkat_store::StoreAdapter::new(
        session_store.clone(),
    )));
    let service = Arc::new(meerkat_session::PersistentSessionService::new(
        builder,
        8,
        session_store,
        Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
        Arc::new(meerkat_store::MemoryBlobStore::new()),
    ));
    let mut definition = with_unique_mob_id(sample_definition(), "canonical-voice-utterance");
    let profile = definition
        .profiles
        .get_mut(&ProfileName::from("worker"))
        .and_then(ProfileBinding::as_inline_mut)
        .expect("worker profile");
    profile.model = "gpt-5.5".to_string();
    profile.runtime_mode = crate::MobRuntimeMode::TurnDriven;
    profile.tools = ToolConfig {
        comms: true,
        ..Default::default()
    };
    let handle = MobBuilder::new(definition, MobStorage::in_memory())
        .with_session_service(service.clone())
        .create()
        .await
        .expect("real source mob");
    let identity = AgentIdentity::from("canonical-source");
    let mut spec = SpawnMemberSpec::new("worker", identity.as_str());
    spec.runtime_mode = Some(crate::MobRuntimeMode::TurnDriven);
    handle.spawn_spec(spec).await.expect("real source agent");
    let session_id = handle
        .resolve_bridge_session_id(&identity)
        .await
        .expect("source session");
    let runtime = handle.runtime_adapter.clone().expect("runtime");
    let (operation, provisional) = runtime
        .__test_begin_live_delegation(&session_id, TASK)
        .await
        .expect("admit provisional voice turn");
    let evidence = service
        .commit_live_user_transcript_final(
            &session_id,
            provisional.clone(),
            Some(meerkat_core::RealtimeTranscriptEvent::UserTranscriptFinal {
                item_id: operation
                    .domain_correlation()
                    .provider()
                    .user_turn_id()
                    .to_string(),
                previous_item_id: None,
                content_index: 0,
                text: TASK.to_string(),
            }),
        )
        .await
        .expect("canonical final voice commit");
    let binding = runtime
        .live_delegation_runtime_binding(&session_id, operation.domain_correlation().channel_id())
        .await
        .expect("binding");
    let reconciled = runtime
        .reconcile_live_delegation_transcript(
            &session_id,
            binding.runtime_id(),
            binding.fence_token(),
            binding.generation(),
            &operation,
            &provisional,
            &evidence,
        )
        .await
        .expect("reconcile actual committed voice evidence");
    let admission = runtime
        .authorize_live_delegation_worker_start_with_ownership(
            &session_id,
            binding.runtime_id(),
            binding.fence_token(),
            binding.generation(),
            &operation,
            &provisional,
            identity.as_str(),
            LiveDelegationWorkerOwnership::ExistingMember,
        )
        .await
        .expect("existing worker authority");
    let consequential = runtime
        .authorize_live_consequential_effect(
            &session_id,
            binding.runtime_id(),
            binding.fence_token(),
            binding.generation(),
            &operation,
            &reconciled,
        )
        .await
        .expect("consequential authority");
    admission
        .release_tool_execution(&consequential)
        .expect("release exact execution");
    let before = SessionServiceHistoryExt::read_history(
        service.as_ref(),
        &session_id,
        meerkat_core::service::SessionHistoryQuery {
            offset: 0,
            limit: None,
        },
    )
    .await
    .expect("canonical history before work");
    assert_eq!(before.messages.iter().filter(|message| matches!(message,
        Message::User(user) if user.transcript_role == meerkat_core::TranscriptUserRole::Conversational
            && user.text_content() == TASK
    )).count(), 1);
    let executor = DelegationExecutionService::new(handle.clone());
    let execution = executor
        .start(request(identity.clone(), admission.clone()))
        .await
        .expect("admit existing source execution");
    mark_started(&runtime, &admission).await;
    let terminal = execution.await_terminal().await;
    assert!(matches!(
        terminal.terminal(),
        DelegationTurnTerminal::Completed(_)
    ));
    retire(
        &runtime,
        &executor,
        &admission,
        &terminal,
        LiveDelegationWorkerTerminalKind::Completed,
    )
    .await;
    let after = SessionServiceHistoryExt::read_history(
        service.as_ref(),
        &session_id,
        meerkat_core::service::SessionHistoryQuery {
            offset: 0,
            limit: None,
        },
    )
    .await
    .expect("canonical history after work");
    let conversational_count = |messages: &[Message]| {
        messages.iter().filter(|message| matches!(message,
            Message::User(user) if user.transcript_role == meerkat_core::TranscriptUserRole::Conversational
        )).count()
    };
    assert_eq!(
        conversational_count(&after.messages),
        conversational_count(&before.messages),
        "execution must not invent an additional conversational user event, including an empty one"
    );
    assert_eq!(after.messages.iter().filter(|message| matches!(message,
        Message::User(user) if user.transcript_role == meerkat_core::TranscriptUserRole::Conversational
            && user.text_content() == TASK
    )).count(), 1, "one real voice utterance must not materialize twice as conversational input");
    let original = before
        .messages
        .iter()
        .find(|message| {
            matches!(message,
                Message::User(user) if user.text_content() == TASK
            )
        })
        .expect("original committed voice row");
    assert!(
        after.messages.contains(original),
        "execution must not rewrite the voice row"
    );
    let contexts = after.messages.iter().filter(|message| matches!(message,
        Message::User(user) if user.transcript_role == meerkat_core::TranscriptUserRole::InjectedContext
    )).collect::<Vec<_>>();
    assert_eq!(
        contexts.len(),
        1,
        "one explicitly attributed execution context"
    );
    let Message::User(context) = contexts[0] else {
        panic!("typed context must be a user-channel context")
    };
    assert_eq!(context.text_content(), execution_context());
    assert_eq!(
        context.identity.interaction_id,
        Some(admission.interaction_id())
    );
    assert!(
        context.identity.run_id.is_some(),
        "execution context belongs to its actual runtime turn"
    );
    let assistant_run = after
        .messages
        .iter()
        .rev()
        .find_map(|message| match message {
            Message::BlockAssistant(assistant) => assistant.identity.run_id.as_ref(),
            _ => None,
        });
    assert_eq!(
        context.identity.run_id.as_ref(),
        assistant_run,
        "execution context and worker reply share the generated runtime run"
    );
    assert!(
        !contexts[0].indexable_content().is_indexable(),
        "do not index the copied request twice"
    );
    assert_eq!(
        serde_json::to_value(context).expect("serialize context")["transcript_role"],
        "injected_context"
    );
    shutdown(&handle, &runtime, &admission).await;
}

async fn fixture() -> (
    MobHandle,
    Arc<MockSessionService>,
    Arc<meerkat_runtime::MeerkatMachine>,
    AgentIdentity,
    LiveDelegationExecutionAdmission,
) {
    fixture_with_mode(crate::MobRuntimeMode::TurnDriven).await
}

async fn fixture_with_mode(
    mode: crate::MobRuntimeMode,
) -> (
    MobHandle,
    Arc<MockSessionService>,
    Arc<meerkat_runtime::MeerkatMachine>,
    AgentIdentity,
    LiveDelegationExecutionAdmission,
) {
    let (handle, service) = create_persistent_runtime_test_mob(with_unique_mob_id(
        sample_definition(),
        "existing-live-delegation",
    ))
    .await;
    service.set_return_exact_run_result(true);
    service.set_keep_alive_turns_complete_immediately(true);
    let identity = AgentIdentity::from("existing-console-agent");
    let mut spec = SpawnMemberSpec::new(ProfileName::from("worker"), identity.clone());
    spec.runtime_mode = Some(mode);
    handle
        .spawn_spec(spec)
        .await
        .expect("spawn existing console member");
    let session = handle
        .resolve_bridge_session_id(&identity)
        .await
        .expect("member session");
    let runtime = handle
        .runtime_adapter
        .clone()
        .expect("runtime-backed member");
    let admission = runtime
        .__test_admit_confirmed_live_delegation(
            &session,
            identity.as_str(),
            LiveDelegationWorkerOwnership::ExistingMember,
            TASK,
        )
        .await
        .expect("generated confirmed live admission");
    (handle, service, runtime, identity, admission)
}

fn request(
    identity: AgentIdentity,
    admission: LiveDelegationExecutionAdmission,
) -> DelegationExecutionRequest {
    DelegationExecutionRequest::new_live(
        identity,
        TASK,
        BoundedResultSpec::new("voice", 256).expect("bound"),
        admission,
    )
    .with_existing_member()
}

async fn shutdown(
    handle: &MobHandle,
    runtime: &meerkat_runtime::MeerkatMachine,
    admission: &LiveDelegationExecutionAdmission,
) {
    runtime
        .abandon_live_open_admission(
            admission.session_id(),
            admission.operation().domain_correlation().channel_id(),
        )
        .await
        .expect("close fixture live channel");
    handle.shutdown().await.expect("shutdown");
}

async fn mark_started(
    runtime: &meerkat_runtime::MeerkatMachine,
    admission: &LiveDelegationExecutionAdmission,
) {
    let binding = runtime
        .live_delegation_runtime_binding(
            admission.session_id(),
            admission.operation().domain_correlation().channel_id(),
        )
        .await
        .expect("live binding");
    runtime
        .resolve_live_delegation_worker_start(
            binding.runtime_id(),
            binding.fence_token(),
            binding.generation(),
            admission,
            true,
        )
        .await
        .expect("publish generated worker start");
}

async fn cancel(
    runtime: &meerkat_runtime::MeerkatMachine,
    admission: &LiveDelegationExecutionAdmission,
    cancellation: &DelegationCancellationHandle,
) -> LiveDelegationCancellationOutcome {
    let binding = runtime
        .live_delegation_runtime_binding(
            admission.session_id(),
            admission.operation().domain_correlation().channel_id(),
        )
        .await
        .expect("live binding");
    let LiveDelegationCancellationDirective::CancellationAuthorized(authority) = runtime
        .abandon_live_delegation(
            binding.runtime_id(),
            binding.fence_token(),
            binding.generation(),
            admission,
        )
        .await
        .expect("generated exact cancellation")
    else {
        panic!("running worker must receive cancellation authority");
    };
    let outcome = cancellation
        .cancel(&authority)
        .await
        .expect("scoped input cancellation");
    runtime
        .resolve_live_delegation_cancellation(
            binding.runtime_id(),
            binding.fence_token(),
            binding.generation(),
            &authority,
            outcome,
        )
        .await
        .expect("publish cancellation outcome");
    outcome
}

async fn retire(
    runtime: &meerkat_runtime::MeerkatMachine,
    service: &DelegationExecutionService,
    admission: &LiveDelegationExecutionAdmission,
    terminal: &DelegationTerminalizedExecution,
    kind: LiveDelegationWorkerTerminalKind,
) {
    let binding = runtime
        .live_delegation_runtime_binding(
            admission.session_id(),
            admission.operation().domain_correlation().channel_id(),
        )
        .await
        .expect("live binding");
    runtime
        .record_live_delegation_worker_terminal(
            binding.runtime_id(),
            binding.fence_token(),
            binding.generation(),
            admission,
            kind,
        )
        .await
        .expect("generated terminal");
    let authority = runtime
        .authorize_live_delegation_worker_retirement(
            binding.runtime_id(),
            binding.fence_token(),
            binding.generation(),
            admission,
        )
        .await
        .expect("generated custody release");
    service
        .retire_live_terminalized(terminal, &authority)
        .await
        .expect("release borrowed work custody");
    runtime
        .resolve_live_delegation_worker_retirement(
            binding.runtime_id(),
            binding.fence_token(),
            binding.generation(),
            &authority,
            true,
        )
        .await
        .expect("publish logical retirement");
}

#[tokio::test]
async fn existing_member_completes_without_fork_and_retains_exact_recovery() {
    let (handle, mock, runtime, identity, admission) = fixture().await;
    let sessions_before = mock.sessions.read().await.len();
    let source_before = mock
        .persisted_session_clone(admission.session_id())
        .await
        .expect("source");
    let executor = DelegationExecutionService::new(handle.clone());
    let execution = executor
        .start(request(identity.clone(), admission.clone()))
        .await
        .expect("admit existing member");
    assert!(execution.spawn().is_none(), "no spawned member receipt");
    mark_started(&runtime, &admission).await;
    let terminal = execution.await_terminal().await;
    let DelegationTurnTerminal::Completed(result) = terminal.terminal() else {
        panic!("exact voice turn must complete: {:?}", terminal.terminal());
    };
    assert_eq!(result.result().result().text(), execution_context());
    retire(
        &runtime,
        &executor,
        &admission,
        &terminal,
        LiveDelegationWorkerTerminalKind::Completed,
    )
    .await;
    assert_eq!(mock.sessions.read().await.len(), sessions_before);
    assert_eq!(
        handle.resolve_bridge_session_id(&identity).await.as_ref(),
        Some(admission.session_id())
    );
    let source_after = mock
        .persisted_session_clone(admission.session_id())
        .await
        .expect("retained source");
    assert_eq!(
        source_before.session_metadata().map(|m| m.model),
        source_after.session_metadata().map(|m| m.model)
    );
    let snapshot = runtime
        .live_delegation_recovery_snapshots(admission.session_id())
        .await
        .expect("recovery")
        .remove(0);
    assert_eq!(
        snapshot.worker_ownership(),
        LiveDelegationWorkerOwnership::ExistingMember
    );
    assert_eq!(snapshot.phase(), LiveDelegationRecoveryPhase::Retired);
    assert!(snapshot.result_eligible());
    let delivery = MobDeliveryIdentity::new(
        admission.operation().operation_id().to_string(),
        admission.interaction_id().to_string(),
    )
    .expect("delivery");
    let recovery = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let recovery = handle
                .recover_bounded_work_for_identity_with_delivery_identity(
                    &identity,
                    &delivery,
                    &BoundedResultSpec::new("voice", 256).expect("bound"),
                )
                .await
                .expect("recover exact admitted work");
            if !matches!(recovery.work(), DurableBoundedWorkState::InFlight { .. }) {
                break recovery;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("durable terminal converges");
    assert!(matches!(
        recovery.member(),
        DurableBoundedMemberState::Active { .. }
    ));
    assert!(
        matches!(
            recovery.work(),
            DurableBoundedWorkState::Terminal { result: Ok(_), .. }
        ),
        "{:?}",
        recovery.work()
    );
    shutdown(&handle, &runtime, &admission).await;
}

#[tokio::test]
async fn existing_member_queued_voice_cancellation_does_not_interrupt_running_text() {
    let (handle, mock, runtime, identity, admission) = fixture().await;
    mock.set_start_turn_delay_ms(300);
    let text = handle
        .start_work_for_identity_bounded(
            identity.clone(),
            crate::WorkSpec::new("ordinary text", crate::WorkOrigin::Internal),
            HandlingMode::Queue,
            BoundedResultSpec::new("text", 256).expect("bound"),
        )
        .await
        .expect("admit text while live is open");
    wait_for_start_turn_call_count(&mock, 1, "text starts").await;
    let executor = DelegationExecutionService::new(handle.clone());
    let execution = executor
        .start(request(identity.clone(), admission.clone()))
        .await
        .expect("queue voice behind text");
    mark_started(&runtime, &admission).await;
    let interrupts_before = mock.interrupt_calls.load(Ordering::Relaxed);
    assert_eq!(
        cancel(
            &runtime,
            &admission,
            &execution.cancellation_handle().expect("cancellation")
        )
        .await,
        LiveDelegationCancellationOutcome::Cancelled
    );
    assert_eq!(
        mock.interrupt_calls.load(Ordering::Relaxed),
        interrupts_before
    );
    assert_eq!(
        text.wait_bounded(BoundedResultSpec::new("text", 256).expect("bound"))
            .await
            .expect("unrelated text succeeds")
            .result()
            .result()
            .text(),
        "ordinary text"
    );
    assert!(matches!(
        execution.await_terminal().await.terminal(),
        DelegationTurnTerminal::Failed(_)
    ));
    assert_eq!(
        mock.start_turn_call_count(),
        1,
        "cancelled queued voice must never run"
    );
    assert!(
        handle
            .get_member(&identity)
            .await
            .expect("member")
            .is_some()
    );
    shutdown(&handle, &runtime, &admission).await;
}

#[tokio::test]
async fn existing_member_running_voice_cancellation_preserves_queued_text() {
    let (handle, mock, runtime, identity, admission) = fixture().await;
    mock.set_start_turn_delay_ms(600_000);
    let executor = DelegationExecutionService::new(handle.clone());
    let execution = executor
        .start(request(identity.clone(), admission.clone()))
        .await
        .expect("start voice");
    mark_started(&runtime, &admission).await;
    wait_for_start_turn_call_count(&mock, 1, "voice starts").await;
    let text = handle
        .start_work_for_identity_bounded(
            identity.clone(),
            crate::WorkSpec::new("independent queued text", crate::WorkOrigin::Internal),
            HandlingMode::Queue,
            BoundedResultSpec::new("text", 256).expect("bound"),
        )
        .await
        .expect("concurrent text admission");
    mock.set_start_turn_delay_ms(0);
    cancel(
        &runtime,
        &admission,
        &execution.cancellation_handle().expect("cancellation"),
    )
    .await;
    let terminal = execution.await_terminal().await;
    assert!(matches!(
        terminal.terminal(),
        DelegationTurnTerminal::Failed(_)
    ));
    assert_eq!(
        tokio::time::timeout(
            Duration::from_secs(5),
            text.wait_bounded(BoundedResultSpec::new("text", 256).expect("bound")),
        )
        .await
        .expect("text converges")
        .expect("unrelated text succeeds")
        .result()
        .result()
        .text(),
        "independent queued text"
    );
    retire(
        &runtime,
        &executor,
        &admission,
        &terminal,
        LiveDelegationWorkerTerminalKind::Cancelled,
    )
    .await;
    assert!(
        handle
            .get_member(&identity)
            .await
            .expect("member")
            .is_some()
    );
    let snapshot = runtime
        .live_delegation_recovery_snapshots(admission.session_id())
        .await
        .expect("recovery")
        .remove(0);
    assert!(!snapshot.result_eligible());
    assert!(snapshot.late());
    shutdown(&handle, &runtime, &admission).await;
}

#[tokio::test]
async fn existing_member_failed_start_cleanup_never_retires_source() {
    let (handle, _mock, runtime, identity, admission) = fixture().await;
    let executor = DelegationExecutionService::new(handle.clone());
    let mut invalid = request(identity.clone(), admission.clone());
    invalid.member.additional_instructions =
        Some(vec!["cannot replace existing policy".to_string()]);
    assert!(matches!(
        executor.start(invalid).await,
        Err(DelegationExecutionError::ExistingMemberAdmissionRequired)
    ));
    let binding = runtime
        .live_delegation_runtime_binding(
            admission.session_id(),
            admission.operation().domain_correlation().channel_id(),
        )
        .await
        .expect("binding");
    runtime
        .resolve_live_delegation_worker_start(
            binding.runtime_id(),
            binding.fence_token(),
            binding.generation(),
            &admission,
            false,
        )
        .await
        .expect("failed start");
    let retirement = runtime
        .authorize_live_delegation_worker_retirement(
            binding.runtime_id(),
            binding.fence_token(),
            binding.generation(),
            &admission,
        )
        .await
        .expect("failed-start retirement");
    executor
        .retire_live_failed_start(&admission, &retirement)
        .await
        .expect("release failed start custody");
    assert!(
        handle
            .get_member(&identity)
            .await
            .expect("source")
            .is_some()
    );
    shutdown(&handle, &runtime, &admission).await;
}

#[tokio::test]
async fn existing_member_execution_failure_preserves_source_and_is_result_ineligible() {
    let (handle, mock, runtime, identity, admission) = fixture().await;
    mock.set_fail_start_turn(true);
    let executor = DelegationExecutionService::new(handle.clone());
    let execution = executor
        .start(request(identity.clone(), admission.clone()))
        .await
        .expect("admit work before executor failure");
    mark_started(&runtime, &admission).await;
    let terminal = execution.await_terminal().await;
    assert!(matches!(
        terminal.terminal(),
        DelegationTurnTerminal::Failed(_)
    ));
    retire(
        &runtime,
        &executor,
        &admission,
        &terminal,
        LiveDelegationWorkerTerminalKind::Failed,
    )
    .await;
    assert!(
        handle
            .get_member(&identity)
            .await
            .expect("source")
            .is_some()
    );
    let snapshot = runtime
        .live_delegation_recovery_snapshots(admission.session_id())
        .await
        .expect("failed terminal")
        .remove(0);
    assert!(!snapshot.result_eligible());
    mock.set_fail_start_turn(false);
    shutdown(&handle, &runtime, &admission).await;
}

#[tokio::test]
async fn existing_member_late_cancellation_never_interrupts_successor_text() {
    let (handle, mock, runtime, identity, admission) = fixture().await;
    let executor = DelegationExecutionService::new(handle.clone());
    let execution = executor
        .start(request(identity.clone(), admission.clone()))
        .await
        .expect("voice admission");
    let cancellation = execution
        .cancellation_handle()
        .expect("exact cancellation endpoint");
    mark_started(&runtime, &admission).await;
    assert!(matches!(
        execution.await_terminal().await.terminal(),
        DelegationTurnTerminal::Completed(_)
    ));
    mock.set_start_turn_delay_ms(200);
    let text = handle
        .start_work_for_identity_bounded(
            identity.clone(),
            crate::WorkSpec::new("successor text", crate::WorkOrigin::Internal),
            HandlingMode::Queue,
            BoundedResultSpec::new("text", 256).expect("bound"),
        )
        .await
        .expect("successor text");
    wait_for_start_turn_call_count(&mock, 2, "successor text starts").await;
    let interrupts_before = mock.interrupt_calls.load(Ordering::Relaxed);
    assert_eq!(
        cancel(&runtime, &admission, &cancellation).await,
        LiveDelegationCancellationOutcome::AlreadyTerminal
    );
    assert_eq!(
        mock.interrupt_calls.load(Ordering::Relaxed),
        interrupts_before
    );
    text.wait_bounded(BoundedResultSpec::new("text", 256).expect("bound"))
        .await
        .expect("successor text is unaffected");
    shutdown(&handle, &runtime, &admission).await;
}

#[tokio::test]
async fn existing_autonomous_member_uses_exact_runtime_input_without_changing_its_mode() {
    let (handle, mock, runtime, identity, admission) =
        fixture_with_mode(crate::MobRuntimeMode::AutonomousHost).await;
    let executor = DelegationExecutionService::new(handle.clone());
    let execution = executor
        .start(request(identity.clone(), admission.clone()))
        .await
        .expect("autonomous source admits exact tracked live input");
    assert!(execution.spawn().is_none());
    mark_started(&runtime, &admission).await;
    let terminal = tokio::time::timeout(Duration::from_secs(5), execution.await_terminal())
        .await
        .expect("bounded autonomous completion");
    assert!(matches!(
        terminal.terminal(),
        DelegationTurnTerminal::Completed(_)
    ));
    assert!(
        mock.recorded_start_turn_user_channel()
            .await
            .iter()
            .any(|(session, messages)| session == admission.session_id()
                && messages == &vec![(true, execution_context())])
    );
    retire(
        &runtime,
        &executor,
        &admission,
        &terminal,
        LiveDelegationWorkerTerminalKind::Completed,
    )
    .await;
    assert_eq!(
        handle
            .get_member(&identity)
            .await
            .expect("member")
            .expect("retained member")
            .runtime_mode,
        crate::MobRuntimeMode::AutonomousHost
    );
    shutdown(&handle, &runtime, &admission).await;
}

#[tokio::test]
async fn existing_member_three_voice_rounds_interleave_text_on_one_channel() {
    let (handle, mock, runtime, identity, mut admission) = fixture().await;
    let session_id = admission.session_id().clone();
    let channel_id = admission
        .operation()
        .domain_correlation()
        .channel_id()
        .clone();
    let sessions_before = mock.sessions.read().await.len();
    let model_before = mock
        .persisted_session_clone(&session_id)
        .await
        .expect("source before rounds")
        .session_metadata()
        .expect("source metadata")
        .model;
    let executor = DelegationExecutionService::new(handle.clone());
    let mut operations = std::collections::HashSet::new();

    for round in 0..3 {
        assert!(operations.insert(admission.operation().operation_id().clone()));
        assert_eq!(
            admission.operation().domain_correlation().channel_id(),
            &channel_id
        );
        let execution = executor
            .start(request(identity.clone(), admission.clone()))
            .await
            .expect("same member accepts next voice operation");
        assert!(execution.spawn().is_none());
        mark_started(&runtime, &admission).await;
        let terminal = execution.await_terminal().await;
        let DelegationTurnTerminal::Completed(result) = terminal.terminal() else {
            panic!("voice round {round} failed: {:?}", terminal.terminal());
        };
        assert_eq!(result.result().result().text(), execution_context());
        retire(
            &runtime,
            &executor,
            &admission,
            &terminal,
            LiveDelegationWorkerTerminalKind::Completed,
        )
        .await;
        runtime
            .__test_finish_live_delegation_provider_turn(&admission)
            .await
            .expect("finish user turn without assistant playback completion");

        let text = format!("ordinary text between voice rounds {round}");
        let text_result = handle
            .start_work_for_identity_bounded(
                identity.clone(),
                crate::WorkSpec::new(text.clone(), crate::WorkOrigin::Internal),
                HandlingMode::Queue,
                BoundedResultSpec::new("text", 256).expect("bound"),
            )
            .await
            .expect("ordinary text admission between voice rounds")
            .wait_bounded(BoundedResultSpec::new("text", 256).expect("bound"))
            .await
            .expect("ordinary text completes");
        assert_eq!(text_result.result().result().text(), text);
        assert_eq!(
            handle.resolve_bridge_session_id(&identity).await.as_ref(),
            Some(&session_id)
        );

        if round < 2 {
            admission = runtime
                .__test_admit_confirmed_live_delegation(
                    &session_id,
                    identity.as_str(),
                    LiveDelegationWorkerOwnership::ExistingMember,
                    TASK,
                )
                .await
                .expect("next voice turn on unchanged live channel");
        }
    }
    assert_eq!(mock.start_turn_call_count(), 6);
    assert_eq!(mock.sessions.read().await.len(), sessions_before);
    assert_eq!(
        mock.persisted_session_clone(&session_id)
            .await
            .expect("source after rounds")
            .session_metadata()
            .expect("source metadata")
            .model,
        model_before
    );
    let recovered = runtime
        .live_delegation_recovery_snapshots(&session_id)
        .await
        .expect("all exact operation receipts retained");
    assert_eq!(recovered.len(), 3);
    assert!(recovered.iter().all(|snapshot| {
        snapshot.phase() == LiveDelegationRecoveryPhase::Retired
            && snapshot.worker_ownership() == LiveDelegationWorkerOwnership::ExistingMember
    }));
    shutdown(&handle, &runtime, &admission).await;
}
