use super::*;
use meerkat_core::time_compat::Instant;

#[cfg(feature = "live")]
fn live_context_execution_binding_is_complete(
    state: &crate::meerkat_machine::dsl::MeerkatMachineState,
    channel: &str,
) -> Result<bool, &'static str> {
    let presence = [
        state
            .live_execution_runtime_id_by_channel
            .contains_key(channel),
        state.live_execution_fence_by_channel.contains_key(channel),
        state
            .live_execution_generation_by_channel
            .contains_key(channel),
        state.live_context_cursor_by_channel.contains_key(channel),
    ];
    if presence.iter().all(|present| !present) {
        return Ok(false);
    }
    presence
        .iter()
        .all(|present| *present)
        .then_some(true)
        .ok_or("active live channel has a partial experimental execution binding")
}

#[cfg(all(test, feature = "live"))]
mod live_context_mirror_tests {
    use super::*;

    #[derive(Default)]
    struct RecordingMirrorHost {
        appends: std::sync::Mutex<Vec<(String, String)>>,
    }

    #[derive(Default)]
    struct AmbiguousMirrorHost {
        appends: std::sync::Mutex<Vec<crate::live_execution::LiveContextAppendAuthority>>,
        recoveries: std::sync::Mutex<Vec<(String, String, u64)>>,
    }

    #[async_trait::async_trait]
    impl crate::live_context_mirror::LiveContextMirrorHost for RecordingMirrorHost {
        async fn append_context(
            &self,
            authority: crate::live_execution::LiveContextAppendAuthority,
            context: String,
        ) -> Result<
            (
                crate::live_execution::LiveContextAppendAuthority,
                meerkat_core::LiveAppendDeliveryOutcome,
            ),
            String,
        > {
            self.appends
                .lock()
                .map_err(|_| "append record lock poisoned".to_string())?
                .push((authority.channel_id().to_string(), context));
            Ok((
                authority,
                meerkat_core::LiveAppendDeliveryOutcome::Acknowledged,
            ))
        }

        async fn recover_ambiguous_append(
            &self,
            _authority: crate::live_execution::LiveContextAmbiguityRecoveryAuthority,
        ) -> Result<(), String> {
            Err("unexpected ambiguity in acknowledged append fixture".to_string())
        }

        async fn recover_ambiguous_delegation_result(
            &self,
            _authority: crate::live_execution::LiveDelegationResultAmbiguityRecoveryAuthority,
        ) -> Result<(), String> {
            Err("unexpected result ambiguity in context fixture".to_string())
        }
    }

    #[async_trait::async_trait]
    impl crate::live_context_mirror::LiveContextMirrorHost for AmbiguousMirrorHost {
        async fn append_context(
            &self,
            authority: crate::live_execution::LiveContextAppendAuthority,
            _context: String,
        ) -> Result<
            (
                crate::live_execution::LiveContextAppendAuthority,
                meerkat_core::LiveAppendDeliveryOutcome,
            ),
            String,
        > {
            self.appends
                .lock()
                .map_err(|_| "append record lock poisoned".to_string())?
                .push(authority.clone());
            Ok((
                authority,
                meerkat_core::LiveAppendDeliveryOutcome::Ambiguous,
            ))
        }

        async fn recover_ambiguous_append(
            &self,
            authority: crate::live_execution::LiveContextAmbiguityRecoveryAuthority,
        ) -> Result<(), String> {
            self.recoveries
                .lock()
                .map_err(|_| "recovery record lock poisoned".to_string())?
                .push((
                    authority.closing_channel_id().to_string(),
                    authority.replacement_channel_id().to_string(),
                    authority.canonical_seed_cursor(),
                ));
            Ok(())
        }

        async fn recover_ambiguous_delegation_result(
            &self,
            _authority: crate::live_execution::LiveDelegationResultAmbiguityRecoveryAuthority,
        ) -> Result<(), String> {
            Err("unexpected result ambiguity in context fixture".to_string())
        }
    }

    async fn prepared_experimental_live_machine() -> (
        crate::MeerkatMachine,
        SessionId,
        meerkat_live::LiveChannelId,
    ) {
        let machine = crate::MeerkatMachine::ephemeral();
        let session_id = SessionId::new();
        machine
            .register_session(session_id.clone())
            .await
            .expect("register session");
        let registered = machine
            .session_dsl_state(&session_id)
            .await
            .expect("read registered runtime epoch");
        machine
            .apply_session_dsl_input(
                &session_id,
                crate::meerkat_machine::dsl::MeerkatMachineInput::PrepareBindings {
                    agent_runtime_id: crate::meerkat_machine::dsl::AgentRuntimeId(
                        "runtime-bound-experimental-live".to_string(),
                    ),
                    fence_token: crate::meerkat_machine::dsl::FenceToken(41),
                    generation: Some(crate::meerkat_machine::dsl::Generation(0)),
                    runtime_epoch_id: registered.active_runtime_epoch_id,
                    session_id: crate::meerkat_machine::dsl::SessionId::from_domain(&session_id),
                },
                "test:PrepareBindings",
            )
            .await
            .expect("prepare exact runtime binding");
        let channel_id = meerkat_live::LiveChannelId::new("bound-experimental-live");
        let identity = meerkat_core::SessionLlmIdentity {
            model: "experimental-realtime-model".to_string(),
            provider: meerkat_core::Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        };
        machine
            .resolve_live_open_admission(&session_id, &channel_id, &identity)
            .await
            .expect("admit exact live channel");
        (machine, session_id, channel_id)
    }

    async fn stage_experimental_live_machine(
        machine: &crate::MeerkatMachine,
        session_id: &SessionId,
        channel_id: &meerkat_live::LiveChannelId,
        canonical_seed_cursor: u64,
    ) {
        machine
            .stage_experimental_live_execution(session_id, channel_id, canonical_seed_cursor)
            .await
            .expect("stage exact experimental execution");
    }

    async fn bind_experimental_live_machine(
        machine: &crate::MeerkatMachine,
        session_id: &SessionId,
        channel_id: &meerkat_live::LiveChannelId,
        canonical_seed_cursor: u64,
    ) {
        let state = machine
            .session_dsl_state(session_id)
            .await
            .expect("read active runtime identity");
        machine
            .apply_session_dsl_input(
                session_id,
                crate::meerkat_machine::dsl::MeerkatMachineInput::RecordLiveWebrtcAnswerAcceptedAndBindExecution {
                    session_id: session_id.to_string(),
                    channel_id: channel_id.to_string(),
                    answer_observation_sequence: 1,
                    runtime_id: state.active_runtime_id.expect("active runtime id"),
                    fence_token: state.active_fence_token.expect("active runtime fence"),
                    generation: state
                        .active_runtime_generation
                        .expect("active runtime generation"),
                    canonical_seed_cursor,
                },
                "test:RecordLiveWebrtcAnswerAcceptedAndBindExecution",
            )
            .await
            .expect("bind experimental live execution");
    }

    async fn bound_experimental_live_machine(
        canonical_seed_cursor: u64,
    ) -> (
        crate::MeerkatMachine,
        SessionId,
        meerkat_live::LiveChannelId,
    ) {
        let (machine, session_id, channel_id) = prepared_experimental_live_machine().await;
        stage_experimental_live_machine(&machine, &session_id, &channel_id, canonical_seed_cursor)
            .await;
        bind_experimental_live_machine(&machine, &session_id, &channel_id, canonical_seed_cursor)
            .await;
        (machine, session_id, channel_id)
    }

    fn insert_test_assistant_output_handle(
        machine: &crate::MeerkatMachine,
        binding: crate::live_execution::LiveDelegationRuntimeBinding,
        assistant_turn_ref: &str,
        output_id: &str,
    ) {
        let handle = LiveAssistantOutputHandle {
            binding: binding.clone(),
            interaction_id: meerkat_core::InteractionId::new(),
            assistant_turn_ref: assistant_turn_ref.to_string(),
            output_id: output_id.to_string(),
            target: Arc::new(std::sync::Mutex::new(Some((
                "response".to_string(),
                "item".to_string(),
                0,
            )))),
            terminal_reserved: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            terminal_consumed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        };
        machine
            .shared
            .live_assistant_output_by_turn
            .lock()
            .expect("assistant output turn registry lock")
            .insert(
                (
                    binding.session_id().clone(),
                    binding.channel_id().clone(),
                    assistant_turn_ref.to_string(),
                ),
                handle.clone(),
            );
        machine
            .shared
            .live_assistant_output_by_id
            .lock()
            .expect("assistant output id registry lock")
            .insert(output_id.to_string(), handle);
    }

    #[tokio::test]
    async fn assistant_output_reservation_requires_current_binding_and_retirement_removes_lookup() {
        let (machine, session_id, channel_id) = bound_experimental_live_machine(0).await;
        let current = machine
            .live_delegation_runtime_binding(&session_id, &channel_id)
            .await
            .expect("read exact current execution binding");
        insert_test_assistant_output_handle(
            &machine,
            current.clone(),
            "assistant-current",
            "output-current",
        );

        let reservation = machine
            .reserve_live_assistant_output_handle(&session_id, &channel_id, "output-current")
            .await
            .expect("exact current output reserves under lifecycle lease");
        drop(reservation);
        machine
            .reserve_live_assistant_output_handle(&session_id, &channel_id, "output-current")
            .await
            .expect("dropped pre-acceptance reservation releases exact custody")
            .release();

        let stale = crate::live_execution::LiveDelegationRuntimeBinding::new(
            session_id.clone(),
            channel_id.clone(),
            current.runtime_id().clone(),
            current.fence_token().saturating_add(1),
            current.generation(),
        );
        insert_test_assistant_output_handle(&machine, stale, "assistant-stale", "output-stale");
        assert!(
            machine
                .reserve_live_assistant_output_handle(&session_id, &channel_id, "output-stale")
                .await
                .is_err(),
            "a stale replacement incarnation cannot reserve terminal authority"
        );

        machine.retire_live_assistant_output_handles(&session_id, &channel_id);
        assert!(
            machine
                .live_assistant_output_handle("output-current")
                .is_none()
        );
        assert!(
            machine
                .live_assistant_output_handle("output-stale")
                .is_none()
        );
    }

    #[tokio::test]
    async fn exact_publication_custody_holds_lifecycle_and_stale_binding_is_rejected() {
        let (machine, session_id, channel_id) = bound_experimental_live_machine(0).await;
        let current = machine
            .live_delegation_runtime_binding(&session_id, &channel_id)
            .await
            .expect("read exact current execution binding");
        let provider_binding = meerkat_live::ProviderWebrtcBinding::new(
            channel_id.clone(),
            session_id.clone(),
            meerkat_live::LiveRuntimeBindingGeneration::new(current.generation()),
            meerkat_live::LiveRuntimeBindingFence::new(current.fence_token()),
        );
        let admission = machine
            .acquire_live_binding_publication_custody(&provider_binding)
            .await
            .expect("current publication binding is classified");
        assert!(matches!(
            admission,
            LiveBindingPublicationAdmission::Current(_)
        ));
        let LiveBindingPublicationAdmission::Current(custody) = admission else {
            return;
        };
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(20),
                machine.acquire_live_open_lifecycle_lease(&session_id),
            )
            .await
            .is_err(),
            "publication custody retains the lifecycle gate through write settlement"
        );
        drop(custody);
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            machine.acquire_live_open_lifecycle_lease(&session_id),
        )
        .await
        .expect("dropping publication custody releases lifecycle progress")
        .expect("lifecycle lease remains available");

        for stale in [
            meerkat_live::ProviderWebrtcBinding::new(
                channel_id.clone(),
                session_id.clone(),
                meerkat_live::LiveRuntimeBindingGeneration::new(
                    current.generation().saturating_add(1),
                ),
                meerkat_live::LiveRuntimeBindingFence::new(current.fence_token()),
            ),
            meerkat_live::ProviderWebrtcBinding::new(
                channel_id.clone(),
                session_id.clone(),
                meerkat_live::LiveRuntimeBindingGeneration::new(current.generation()),
                meerkat_live::LiveRuntimeBindingFence::new(current.fence_token().saturating_add(1)),
            ),
        ] {
            assert!(matches!(
                machine
                    .acquire_live_binding_publication_custody(&stale)
                    .await
                    .expect("stale publication binding is classified"),
                LiveBindingPublicationAdmission::Stale
            ));
        }
    }

    #[tokio::test]
    async fn ordinary_public_live_channel_makes_post_commit_mirror_a_no_op() {
        let machine = crate::MeerkatMachine::ephemeral();
        let session_id = SessionId::new();
        machine
            .register_session(session_id.clone())
            .await
            .expect("register session");
        let registered = machine
            .session_dsl_state(&session_id)
            .await
            .expect("read registered runtime epoch");
        machine
            .apply_session_dsl_input(
                &session_id,
                crate::meerkat_machine::dsl::MeerkatMachineInput::PrepareBindings {
                    agent_runtime_id: crate::meerkat_machine::dsl::AgentRuntimeId(
                        "runtime-ordinary-public-live".to_string(),
                    ),
                    fence_token: crate::meerkat_machine::dsl::FenceToken(43),
                    generation: Some(crate::meerkat_machine::dsl::Generation(0)),
                    runtime_epoch_id: registered.active_runtime_epoch_id,
                    session_id: crate::meerkat_machine::dsl::SessionId::from_domain(&session_id),
                },
                "test:PrepareBindings",
            )
            .await
            .expect("prepare ordinary runtime binding");
        let channel_id = meerkat_live::LiveChannelId::new("ordinary-public-live");
        let identity = meerkat_core::SessionLlmIdentity {
            model: "ordinary-realtime-model".to_string(),
            provider: meerkat_core::Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        };
        machine
            .resolve_live_open_admission(&session_id, &channel_id, &identity)
            .await
            .expect("ordinary public live admission");

        let mut session = meerkat_core::Session::with_id(session_id.clone());
        session.push(meerkat_core::Message::User(
            meerkat_core::UserMessage::text("ordinary parent text"),
        ));
        let committed =
            meerkat_core::lifecycle::core_executor::BoundSessionCommit::sealed(Arc::new(session))
                .expect("seal exact committed session boundary");

        let rows = machine
            .enqueue_committed_parent_session_boundary(
                &session_id,
                &committed,
                "store-issued-commit-authority",
            )
            .await
            .expect("ordinary public channel must not make canonical commit fail");
        assert_eq!(rows, 0);

        let state = machine
            .session_dsl_state(&session_id)
            .await
            .expect("read generated session state");
        assert_eq!(
            live_context_execution_binding_is_complete(&state, channel_id.as_str()),
            Ok(false),
            "ordinary public realtime channels have no experimental outbox binding"
        );
        assert!(state.live_context_queued_session_by_append.is_empty());
        assert!(
            machine
                .shared
                .live_context_queued_rows
                .lock()
                .expect("queued rows lock")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn prebind_commit_retains_only_the_suffix_after_staged_seed_k() {
        let (machine, session_id, channel_id) = prepared_experimental_live_machine().await;
        stage_experimental_live_machine(&machine, &session_id, &channel_id, 1).await;
        let host = Arc::new(RecordingMirrorHost::default());
        machine.set_live_context_mirror_host(host.clone());

        let mut session = meerkat_core::Session::with_id(session_id.clone());
        session.push(meerkat_core::Message::User(
            meerkat_core::UserMessage::text("seed acknowledged during answer"),
        ));
        session.push(meerkat_core::Message::User(
            meerkat_core::UserMessage::text("committed while answer was pending"),
        ));
        let committed =
            meerkat_core::lifecycle::core_executor::BoundSessionCommit::sealed(Arc::new(session))
                .expect("seal exact pre-bind committed boundary");

        let rows = machine
            .enqueue_committed_parent_session_boundary(
                &session_id,
                &committed,
                "pre-bind-current-store-authority",
            )
            .await
            .expect("staged experimental channel retains the suffix after K");
        assert_eq!(rows, 1);
        assert!(
            host.appends.lock().expect("append record lock").is_empty(),
            "pre-bind custody never sends before provider seed acknowledgement"
        );
        {
            let queued = machine
                .shared
                .live_context_queued_rows
                .lock()
                .expect("queued rows lock");
            assert_eq!(queued.len(), 1);
            assert!(queued.contains_key(&(session_id.clone(), 2)));
        }

        bind_experimental_live_machine(&machine, &session_id, &channel_id, 1).await;
        machine
            .drain_live_context_outbox(&session_id)
            .await
            .expect("answer-ready drain sends the retained suffix");
        assert_eq!(host.appends.lock().expect("append record lock").len(), 1);

        let caught_up = machine
            .enqueue_committed_parent_session_boundary(
                &session_id,
                &committed,
                "answer-ready-current-store-authority",
            )
            .await
            .expect("defensive store catch-up does not duplicate retained rows");
        assert_eq!(caught_up, 0);
        assert_eq!(
            host.appends.lock().expect("append record lock").len(),
            1,
            "the exact canonical suffix reaches the provider once"
        );
    }

    #[tokio::test]
    async fn exact_parent_commit_reaches_only_the_active_bound_channel_once() {
        let (machine, session_id, channel_id) = bound_experimental_live_machine(0).await;

        let host = Arc::new(RecordingMirrorHost::default());
        machine.set_live_context_mirror_host(host.clone());
        let mut session = meerkat_core::Session::with_id(session_id.clone());
        session.push(meerkat_core::Message::User(
            meerkat_core::UserMessage::text("one exact external parent append"),
        ));
        let committed =
            meerkat_core::lifecycle::core_executor::BoundSessionCommit::sealed(Arc::new(session))
                .expect("seal exact committed session boundary");

        let rows = machine
            .enqueue_committed_parent_session_boundary(
                &session_id,
                &committed,
                "exact-store-issued-commit-authority",
            )
            .await
            .expect("committed parent boundary mirrors");
        assert_eq!(rows, 1);

        let duplicate_rows = machine
            .enqueue_committed_parent_session_boundary(
                &session_id,
                &committed,
                "exact-store-issued-commit-authority",
            )
            .await
            .expect("repeated exact committed boundary is idempotent");
        assert_eq!(
            duplicate_rows, 0,
            "the exact committed row is not re-enqueued"
        );

        let appends = host.appends.lock().expect("append record lock");
        assert_eq!(appends.len(), 1, "one commit produces one provider append");
        assert_eq!(appends[0].0, channel_id.to_string());
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&appends[0].1)
                .expect("context payload is JSON"),
            serde_json::json!({
                "role": "user",
                "text": "one exact external parent append",
            })
        );
        drop(appends);

        let state = machine
            .session_dsl_state(&session_id)
            .await
            .expect("read acknowledged cursor");
        assert_eq!(
            state
                .live_context_cursor_by_channel
                .get(channel_id.as_str()),
            Some(&1),
            "only provider acknowledgement advances exact canonical coverage"
        );
        assert!(
            machine
                .shared
                .live_context_queued_rows
                .lock()
                .expect("queued rows lock")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn answer_ready_catchup_starts_after_the_acknowledged_seed_cursor() {
        let (machine, session_id, channel_id) = bound_experimental_live_machine(1).await;
        let host = Arc::new(RecordingMirrorHost::default());
        machine.set_live_context_mirror_host(host.clone());

        let mut session = meerkat_core::Session::with_id(session_id.clone());
        session.push(meerkat_core::Message::User(
            meerkat_core::UserMessage::text("already acknowledged seed"),
        ));
        session.push(meerkat_core::Message::User(
            meerkat_core::UserMessage::text("committed while answer was pending"),
        ));
        let committed =
            meerkat_core::lifecycle::core_executor::BoundSessionCommit::sealed(Arc::new(session))
                .expect("seal exact answer-ready committed boundary");

        let rows = machine
            .enqueue_committed_parent_session_boundary(
                &session_id,
                &committed,
                "answer-ready-current-store-authority",
            )
            .await
            .expect("answer-ready catch-up mirrors only the suffix after seed K");
        assert_eq!(rows, 1);

        let appends = host.appends.lock().expect("append record lock");
        assert_eq!(appends.len(), 1, "the acknowledged seed is never replayed");
        assert_eq!(appends[0].0, channel_id.to_string());
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&appends[0].1)
                .expect("context payload is JSON"),
            serde_json::json!({
                "role": "user",
                "text": "committed while answer was pending",
            })
        );
        drop(appends);

        let state = machine
            .session_dsl_state(&session_id)
            .await
            .expect("read acknowledged catch-up cursor");
        assert_eq!(
            state
                .live_context_cursor_by_channel
                .get(channel_id.as_str()),
            Some(&2),
            "provider acknowledgement advances from seed K to the exact suffix boundary"
        );
    }

    #[tokio::test]
    async fn ambiguous_delivery_is_never_retried_and_rejects_a_late_old_ack() {
        let (machine, session_id, channel_id) = bound_experimental_live_machine(0).await;
        let host = Arc::new(AmbiguousMirrorHost::default());
        machine.set_live_context_mirror_host(host.clone());
        let mut session = meerkat_core::Session::with_id(session_id.clone());
        session.push(meerkat_core::Message::User(
            meerkat_core::UserMessage::text("ambiguous external parent append"),
        ));
        let committed =
            meerkat_core::lifecycle::core_executor::BoundSessionCommit::sealed(Arc::new(session))
                .expect("seal exact committed session boundary");

        machine
            .enqueue_committed_parent_session_boundary(
                &session_id,
                &committed,
                "ambiguous-store-issued-commit-authority",
            )
            .await
            .expect("ambiguity resolves into generated recovery authority");
        machine
            .drain_live_context_outbox(&session_id)
            .await
            .expect("a later drain must not resend the ambiguous edge");

        let sent = host.appends.lock().expect("append record lock");
        assert_eq!(sent.len(), 1, "ambiguous delivery is never retried");
        let stale_authority = sent[0].clone();
        drop(sent);
        let recoveries = host.recoveries.lock().expect("recovery record lock");
        assert_eq!(recoveries.len(), 1);
        assert_eq!(recoveries[0].0, channel_id.to_string());
        assert_ne!(recoveries[0].1, channel_id.to_string());
        assert_eq!(recoveries[0].2, 1);
        drop(recoveries);

        let binding = machine
            .live_delegation_runtime_binding(&session_id, &channel_id)
            .await
            .expect("read exact old binding");
        assert!(
            machine
                .resolve_live_context_append(
                    binding.runtime_id(),
                    binding.fence_token(),
                    binding.generation(),
                    &stale_authority,
                    meerkat_core::LiveAppendDeliveryOutcome::Acknowledged,
                )
                .await
                .is_err(),
            "a late acknowledgement for the ambiguous old edge is stale"
        );
    }
}

#[cfg(not(target_arch = "wasm32"))]
type AcceptInputWithCompletionFuture<'a> = std::pin::Pin<
    Box<
        dyn std::future::Future<
                Output = Result<
                    (AcceptOutcome, Option<crate::completion::CompletionHandle>),
                    RuntimeDriverError,
                >,
            > + Send
            + 'a,
    >,
>;

#[cfg(target_arch = "wasm32")]
type AcceptInputWithCompletionFuture<'a> = std::pin::Pin<
    Box<
        dyn std::future::Future<
                Output = Result<
                    (AcceptOutcome, Option<crate::completion::CompletionHandle>),
                    RuntimeDriverError,
                >,
            > + 'a,
    >,
>;

#[cfg(feature = "live")]
fn dsl_live_channel_status_from_observation(
    status: &meerkat_core::live_adapter::LiveAdapterStatus,
) -> (
    crate::meerkat_machine::dsl::LiveChannelPublicStatus,
    Option<crate::meerkat_machine::dsl::LiveChannelDegradationReason>,
    Option<String>,
) {
    use crate::meerkat_machine::dsl::{
        LiveChannelDegradationReason as DslReason, LiveChannelPublicStatus as DslStatus,
    };
    use meerkat_core::live_adapter::LiveAdapterStatus;

    match status {
        LiveAdapterStatus::Idle => (DslStatus::Idle, None, None),
        LiveAdapterStatus::Opening => (DslStatus::Opening, None, None),
        LiveAdapterStatus::Ready => (DslStatus::Ready, None, None),
        LiveAdapterStatus::Closing => (DslStatus::Closing, None, None),
        LiveAdapterStatus::Closed => (DslStatus::Closed, None, None),
        LiveAdapterStatus::Degraded { reason } => {
            let (reason, detail) = dsl_live_channel_degradation_reason(reason);
            (DslStatus::Degraded, Some(reason), detail)
        }
        other => (
            DslStatus::Degraded,
            Some(DslReason::Unknown),
            Some(format!("{other:?}")),
        ),
    }
}

#[cfg(feature = "live")]
fn dsl_live_channel_degradation_reason(
    reason: &meerkat_core::live_adapter::LiveDegradationReason,
) -> (
    crate::meerkat_machine::dsl::LiveChannelDegradationReason,
    Option<String>,
) {
    use crate::meerkat_machine::dsl::LiveChannelDegradationReason as DslReason;
    use meerkat_core::live_adapter::LiveDegradationReason;

    match reason {
        LiveDegradationReason::RateLimited => (DslReason::RateLimited, None),
        LiveDegradationReason::ProviderThrottled => (DslReason::ProviderThrottled, None),
        LiveDegradationReason::NetworkUnstable => (DslReason::NetworkUnstable, None),
        LiveDegradationReason::Other { detail } => {
            (DslReason::Other, Some(detail.clone().into_owned()))
        }
        other => (DslReason::Unknown, Some(format!("{other:?}"))),
    }
}

#[cfg(feature = "live")]
fn dsl_live_command_kind(
    kind: meerkat_live::LiveCommandAcceptanceKind,
) -> crate::meerkat_machine::dsl::LiveCommandPublicKind {
    match kind {
        meerkat_live::LiveCommandAcceptanceKind::SendInput => {
            crate::meerkat_machine::dsl::LiveCommandPublicKind::SendInput
        }
        meerkat_live::LiveCommandAcceptanceKind::CommitInput => {
            crate::meerkat_machine::dsl::LiveCommandPublicKind::CommitInput
        }
        meerkat_live::LiveCommandAcceptanceKind::Interrupt => {
            crate::meerkat_machine::dsl::LiveCommandPublicKind::Interrupt
        }
        meerkat_live::LiveCommandAcceptanceKind::TruncateAssistantOutput => {
            crate::meerkat_machine::dsl::LiveCommandPublicKind::TruncateAssistantOutput
        }
        meerkat_live::LiveCommandAcceptanceKind::CompleteAssistantPlayback => {
            crate::meerkat_machine::dsl::LiveCommandPublicKind::CompleteAssistantPlayback
        }
    }
}

#[cfg(feature = "live")]
fn dsl_live_command_rejection_reason(
    error: &meerkat_live::LiveAdapterHostError,
) -> crate::meerkat_machine::dsl::LiveCommandRejectionReason {
    use crate::meerkat_machine::dsl::LiveCommandRejectionReason as DslReason;
    use meerkat_live::LiveAdapterHostError;

    match error {
        LiveAdapterHostError::ChannelNotFound(_) => DslReason::ChannelNotFound,
        LiveAdapterHostError::NoAdapter(_) => DslReason::NoAdapter,
        LiveAdapterHostError::ChannelNotReady(_, _) => DslReason::ChannelNotReady,
        LiveAdapterHostError::UnsupportedCommand(_) => DslReason::UnsupportedCommand,
        LiveAdapterHostError::AdapterError(_) => DslReason::AdapterError,
        _ => DslReason::InternalHostError,
    }
}

#[cfg(feature = "live")]
fn dsl_live_channel_request_rejection_reason(
    error: &meerkat_live::LiveAdapterHostError,
) -> crate::meerkat_machine::dsl::LiveChannelRequestRejectionReason {
    use crate::meerkat_machine::dsl::LiveChannelRequestRejectionReason as DslReason;
    use meerkat_live::LiveAdapterHostError;

    match error {
        LiveAdapterHostError::ChannelNotFound(_) => DslReason::ChannelNotFound,
        LiveAdapterHostError::NoAdapter(_) => DslReason::NoAdapter,
        _ => DslReason::InternalHostError,
    }
}

#[cfg(feature = "live")]
fn extract_live_websocket_token_admission(
    effects: &[crate::meerkat_machine::dsl::MeerkatMachineEffect],
    session_id: &str,
    channel_id: &str,
    token: &str,
    transition: &str,
) -> Result<LiveWebsocketTokenAdmissionAuthority, RuntimeDriverError> {
    effects
        .iter()
        .find_map(|effect| match effect {
            crate::meerkat_machine::dsl::MeerkatMachineEffect::LiveWebsocketTokenAdmissionResolved {
                session_id: effect_session_id,
                channel_id: effect_channel_id,
                token: effect_token,
                admitted,
                rejection,
                public_error_class,
                sequence,
            } if effect_session_id == session_id
                && effect_channel_id == channel_id
                && effect_token == token =>
            {
                Some(LiveWebsocketTokenAdmissionAuthority {
                    admitted: *admitted,
                    rejection: *rejection,
                    public_error_class: *public_error_class,
                    sequence: *sequence,
                })
            }
            _ => None,
        })
        .ok_or_else(|| {
            RuntimeDriverError::Internal(format!(
                "{transition} for channel '{channel_id}' emitted no LiveWebsocketTokenAdmissionResolved effect"
            ))
        })
}

/// Machine-generated authority for runtime cleanup after a completion waiter
/// resolves. The action is projected from a generated DSL effect; surfaces use
/// this wrapper instead of matching completion outcomes locally.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeCompletionCleanupAuthority {
    pub action: crate::meerkat_machine::dsl::RuntimeCompletionCleanupAction,
    pub pre_admission_action: crate::meerkat_machine::dsl::RuntimeCompletionPreAdmissionAction,
    pub outcome: crate::meerkat_machine::dsl::RuntimeCompletionObservedOutcome,
    pub live_session: crate::meerkat_machine::dsl::RuntimeCompletionLiveSessionObservation,
    pub archived_by_authority: bool,
}

impl RuntimeCompletionCleanupAuthority {
    pub fn requires_runtime_cleanup(self) -> bool {
        matches!(
            self.action,
            crate::meerkat_machine::dsl::RuntimeCompletionCleanupAction::CleanupRuntime
        )
    }

    pub fn releases_pre_admission(self) -> bool {
        matches!(
            self.pre_admission_action,
            crate::meerkat_machine::dsl::RuntimeCompletionPreAdmissionAction::ReleasePreAdmission
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RuntimeCompletionCleanupEffect {
    action: crate::meerkat_machine::dsl::RuntimeCompletionCleanupAction,
    pre_admission_action: crate::meerkat_machine::dsl::RuntimeCompletionPreAdmissionAction,
}

fn runtime_completion_cleanup_effect_from_effects(
    session_id: &SessionId,
    effects: &[crate::meerkat_machine::dsl::MeerkatMachineEffect],
) -> Result<RuntimeCompletionCleanupEffect, RuntimeDriverError> {
    let expected_session_id = crate::meerkat_machine::dsl::SessionId::from_domain(session_id);
    effects
        .iter()
        .find_map(|effect| match effect {
            crate::meerkat_machine::dsl::MeerkatMachineEffect::RuntimeCompletionCleanupResolved {
                session_id: effect_session_id,
                action,
                pre_admission_action,
            } if effect_session_id == &expected_session_id => Some(RuntimeCompletionCleanupEffect {
                action: *action,
                pre_admission_action: *pre_admission_action,
            }),
            _ => None,
        })
        .ok_or_else(|| {
            RuntimeDriverError::Internal(format!(
                "ResolveRuntimeCompletionCleanup for session '{session_id}' emitted no RuntimeCompletionCleanupResolved effect"
            ))
        })
}

/// Machine-generated authority for mechanical completion-waiter failures.
/// The generated effect owns both admission release and the public failure
/// class/reason; surfaces only map these closed values to transport envelopes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeCompletionWaitFailureAuthority {
    pub failure: crate::meerkat_machine::dsl::RuntimeCompletionWaitFailureObservation,
    pub pre_admission_action: crate::meerkat_machine::dsl::RuntimeCompletionPreAdmissionAction,
    pub public_error_class:
        crate::meerkat_machine::dsl::RuntimeCompletionWaitFailurePublicErrorClass,
    pub public_reason: crate::meerkat_machine::dsl::RuntimeCompletionWaitFailurePublicReason,
    pub resumable: bool,
}

impl RuntimeCompletionWaitFailureAuthority {
    pub fn releases_pre_admission(self) -> bool {
        matches!(
            self.pre_admission_action,
            crate::meerkat_machine::dsl::RuntimeCompletionPreAdmissionAction::ReleasePreAdmission
        )
    }
}

fn runtime_completion_wait_failure_authority_from_effects(
    session_id: &SessionId,
    failure: crate::meerkat_machine::dsl::RuntimeCompletionWaitFailureObservation,
    effects: &[crate::meerkat_machine::dsl::MeerkatMachineEffect],
) -> Result<RuntimeCompletionWaitFailureAuthority, RuntimeDriverError> {
    let expected_session_id = crate::meerkat_machine::dsl::SessionId::from_domain(session_id);
    effects
        .iter()
        .find_map(|effect| match effect {
            crate::meerkat_machine::dsl::MeerkatMachineEffect::RuntimeCompletionWaitFailureResolved {
                session_id: effect_session_id,
                failure: effect_failure,
                pre_admission_action,
                public_error_class,
                public_reason,
                resumable,
            } if effect_session_id == &expected_session_id && *effect_failure == failure => {
                Some(RuntimeCompletionWaitFailureAuthority {
                    failure: *effect_failure,
                    pre_admission_action: *pre_admission_action,
                    public_error_class: *public_error_class,
                    public_reason: *public_reason,
                    resumable: *resumable,
                })
            }
            _ => None,
        })
        .ok_or_else(|| {
            RuntimeDriverError::Internal(format!(
                "ResolveRuntimeCompletionWaitFailure for session '{session_id}' emitted no RuntimeCompletionWaitFailureResolved effect"
            ))
        })
}

impl MeerkatMachine {
    pub async fn resolve_runtime_completion_cleanup(
        &self,
        session_id: &SessionId,
        observation: crate::completion::CompletionCleanupObservation,
        archived_by_authority: bool,
        live_session: crate::meerkat_machine::dsl::RuntimeCompletionLiveSessionObservation,
    ) -> Result<RuntimeCompletionCleanupAuthority, RuntimeDriverError> {
        let observed_outcome = observation.observed_outcome();
        let input =
            crate::meerkat_machine::dsl::MeerkatMachineInput::ResolveRuntimeCompletionCleanup {
                session_id: crate::meerkat_machine::dsl::SessionId::from_domain(session_id),
                observation_session_id: crate::meerkat_machine::dsl::SessionId::from_domain(
                    observation.owner_session_id(),
                ),
                observation_agent_runtime_id: observation.owner_agent_runtime_id().cloned(),
                observation_fence_token: observation.owner_fence_token(),
                observation_runtime_generation: observation.owner_runtime_generation(),
                observation_runtime_epoch_id: observation.owner_runtime_epoch_id().cloned(),
                outcome: observed_outcome,
                archived_by_authority,
                live_session,
            };
        let effects = self
            .preview_session_dsl_input(session_id, input, "ResolveRuntimeCompletionCleanup")
            .await
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
        let cleanup_effect = runtime_completion_cleanup_effect_from_effects(session_id, &effects)?;
        Ok(RuntimeCompletionCleanupAuthority {
            action: cleanup_effect.action,
            pre_admission_action: cleanup_effect.pre_admission_action,
            outcome: observed_outcome,
            live_session,
            archived_by_authority,
        })
    }

    pub async fn resolve_runtime_completion_wait_failure(
        &self,
        session_id: &SessionId,
        error: &crate::completion::CompletionWaitError,
    ) -> Result<RuntimeCompletionWaitFailureAuthority, RuntimeDriverError> {
        let failure = error.wait_failure_observation();
        let input =
            crate::meerkat_machine::dsl::MeerkatMachineInput::ResolveRuntimeCompletionWaitFailure {
                session_id: crate::meerkat_machine::dsl::SessionId::from_domain(session_id),
                failure,
            };
        let effects = self
            .preview_session_dsl_input(session_id, input, "ResolveRuntimeCompletionWaitFailure")
            .await
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
        runtime_completion_wait_failure_authority_from_effects(session_id, failure, &effects)
    }

    /// Project the exact generated binding used for every live delegation input.
    pub async fn live_delegation_runtime_binding(
        &self,
        session_id: &SessionId,
        channel_id: &meerkat_core::LiveChannelId,
    ) -> Result<crate::live_execution::LiveDelegationRuntimeBinding, RuntimeDriverError> {
        let state = self.session_dsl_state(session_id).await.map_err(|reason| {
            RuntimeDriverError::ValidationFailed {
                reason: reason.to_string(),
            }
        })?;
        let channel = channel_id.to_string();
        if state.live_channel_session_by_channel.get(&channel) != Some(&session_id.to_string()) {
            return Err(RuntimeDriverError::ValidationFailed {
                reason: "live delegation channel is not bound to the requested session".to_string(),
            });
        }
        let runtime_id = state
            .live_execution_runtime_id_by_channel
            .get(&channel)
            .ok_or_else(|| RuntimeDriverError::ValidationFailed {
                reason: "live delegation channel has no generated runtime binding".to_string(),
            })?;
        let fence = state
            .live_execution_fence_by_channel
            .get(&channel)
            .ok_or_else(|| RuntimeDriverError::ValidationFailed {
                reason: "live delegation channel has no generated fence binding".to_string(),
            })?;
        let generation = state
            .live_execution_generation_by_channel
            .get(&channel)
            .ok_or_else(|| RuntimeDriverError::ValidationFailed {
                reason: "live delegation channel has no generated generation binding".to_string(),
            })?;
        Ok(crate::live_execution::LiveDelegationRuntimeBinding::new(
            session_id.clone(),
            channel_id.clone(),
            crate::identifiers::LogicalRuntimeId::new(runtime_id.0.clone()),
            fence.0,
            generation.0,
        ))
    }

    /// Acquire writer-settlement custody for one exact provider binding.
    ///
    /// The session lifecycle lease is acquired before generated binding
    /// validation and remains embedded in the returned opaque custody. A
    /// close or replacement therefore cannot cross the caller's write and
    /// flush boundary. Stale copied binding atoms yield `Stale` without any
    /// machine mutation.
    #[cfg(feature = "live")]
    pub async fn acquire_live_binding_publication_custody(
        &self,
        binding: &meerkat_live::ProviderWebrtcBinding,
    ) -> Result<LiveBindingPublicationAdmission, RuntimeDriverError> {
        let lifecycle_lease = self
            .acquire_live_open_lifecycle_lease(binding.session_id())
            .await?;
        let current = self
            .live_delegation_runtime_binding(binding.session_id(), binding.channel_id())
            .await;
        let exact = current.is_ok_and(|current| {
            current.generation() == binding.runtime_generation().get()
                && current.fence_token() == binding.runtime_fence().get()
        });
        if !exact {
            return Ok(LiveBindingPublicationAdmission::Stale);
        }
        Ok(LiveBindingPublicationAdmission::Current(
            LiveBindingPublicationCustody {
                binding: binding.clone(),
                _lifecycle_lease: lifecycle_lease,
            },
        ))
    }

    /// Stage generated pre-answer custody for a strict experimental live
    /// channel. The shared strict-open coordinator calls this only while
    /// holding its sealed experimental admission witness.
    #[cfg(feature = "live")]
    pub async fn stage_experimental_live_execution(
        &self,
        session_id: &SessionId,
        channel_id: &meerkat_live::LiveChannelId,
        canonical_seed_cursor: u64,
    ) -> Result<ExperimentalLiveExecutionStageAuthority, RuntimeDriverError> {
        let _mutation_guard = self
            .lock_current_durability_ready_session_mutation_gate(session_id)
            .await?;
        let state = self.session_dsl_state(session_id).await.map_err(|reason| {
            RuntimeDriverError::ValidationFailed {
                reason: reason.to_string(),
            }
        })?;
        let runtime_id = state.active_runtime_id.as_ref().ok_or_else(|| {
            RuntimeDriverError::ValidationFailed {
                reason: "experimental live staging has no active runtime identity".to_string(),
            }
        })?;
        let fence =
            state
                .active_fence_token
                .ok_or_else(|| RuntimeDriverError::ValidationFailed {
                    reason: "experimental live staging has no active runtime fence".to_string(),
                })?;
        let generation = state.active_runtime_generation.ok_or_else(|| {
            RuntimeDriverError::ValidationFailed {
                reason: "experimental live staging has no active runtime generation".to_string(),
            }
        })?;
        let channel = channel_id.to_string();
        let runtime_binding = crate::live_execution::LiveDelegationRuntimeBinding::new(
            session_id.clone(),
            channel_id.clone(),
            crate::identifiers::LogicalRuntimeId::new(runtime_id.0.clone()),
            fence.0,
            generation.0,
        );
        let (_, effects) = self
            .apply_session_dsl_input(
                session_id,
                crate::meerkat_machine::dsl::MeerkatMachineInput::StageExperimentalLiveExecution {
                    session_id: session_id.to_string(),
                    channel_id: channel.clone(),
                    runtime_id: runtime_id.clone(),
                    fence_token: fence,
                    generation,
                    canonical_seed_cursor,
                },
                "StageExperimentalLiveExecution",
            )
            .await
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
        if effects.as_slice().iter().any(|effect| {
            matches!(
                effect,
                crate::meerkat_machine::dsl::MeerkatMachineEffect::ExperimentalLiveExecutionStaged {
                    session_id: effect_session,
                    channel_id: effect_channel,
                    canonical_seed_cursor: effect_cursor,
                    ..
                } if effect_session == &session_id.to_string()
                    && effect_channel == &channel
                    && *effect_cursor == canonical_seed_cursor
            )
        }) {
            Ok(ExperimentalLiveExecutionStageAuthority {
                binding: runtime_binding,
                canonical_seed_cursor,
            })
        } else {
            Err(RuntimeDriverError::Internal(
                "generated experimental live staging emitted no matching authority".to_string(),
            ))
        }
    }

    /// Admit one exact typed provider TurnStarted observation and mint the
    /// sole Meerkat InteractionId for that foreground voice turn.
    #[cfg(feature = "live")]
    pub async fn observe_live_provider_turn_started(
        &self,
        observation: &meerkat_live::LiveSidebandObservation,
    ) -> Result<LiveProviderTurnStartedAuthority, RuntimeDriverError> {
        let meerkat_live::LiveSidebandObservationKind::TurnStarted {
            turn,
            role: meerkat_live::LiveSidebandTurnRole::User,
        } = observation.kind()
        else {
            return Err(RuntimeDriverError::ValidationFailed {
                reason: "provider turn-start bridge requires typed TurnStarted evidence"
                    .to_string(),
            });
        };
        let provider_binding = observation.binding();
        let session_id = provider_binding.session_id();
        let channel_id = provider_binding.channel_id();
        let _mutation_guard = self
            .lock_current_durability_ready_session_mutation_gate(session_id)
            .await?;
        let state = self.session_dsl_state(session_id).await.map_err(|reason| {
            RuntimeDriverError::ValidationFailed {
                reason: reason.to_string(),
            }
        })?;
        let channel = channel_id.to_string();
        let runtime_id = state
            .live_execution_runtime_id_by_channel
            .get(&channel)
            .ok_or_else(|| RuntimeDriverError::ValidationFailed {
                reason: "provider turn started before generated execution binding".to_string(),
            })?;
        let fence = state
            .live_execution_fence_by_channel
            .get(&channel)
            .ok_or_else(|| RuntimeDriverError::ValidationFailed {
                reason: "provider turn has no generated fence binding".to_string(),
            })?;
        let generation = state
            .live_execution_generation_by_channel
            .get(&channel)
            .ok_or_else(|| RuntimeDriverError::ValidationFailed {
                reason: "provider turn has no generated generation binding".to_string(),
            })?;
        if fence.0 != provider_binding.runtime_fence().get()
            || generation.0 != provider_binding.runtime_generation().get()
            || state.live_channel_session_by_channel.get(&channel) != Some(&session_id.to_string())
        {
            return Err(RuntimeDriverError::ValidationFailed {
                reason: "provider turn-start evidence is stale for the active execution binding"
                    .to_string(),
            });
        }
        let interaction_id = meerkat_core::InteractionId::new();
        let provider_turn_ref = turn.adapter_key().to_string();
        let runtime_binding = crate::live_execution::LiveDelegationRuntimeBinding::new(
            session_id.clone(),
            channel_id.clone(),
            crate::identifiers::LogicalRuntimeId::new(runtime_id.0.clone()),
            fence.0,
            generation.0,
        );
        let (_, effects) = self
            .apply_session_dsl_input(
                session_id,
                crate::meerkat_machine::dsl::MeerkatMachineInput::ObserveLiveProviderTurnStarted {
                    channel_id: channel.clone(),
                    runtime_id: runtime_id.clone(),
                    fence_token: *fence,
                    generation: *generation,
                    interaction_id: interaction_id.to_string(),
                    provider_turn_ref: provider_turn_ref.clone(),
                },
                "ObserveLiveProviderTurnStarted",
            )
            .await
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
        if effects.as_slice().iter().any(|effect| {
            matches!(
                effect,
                crate::meerkat_machine::dsl::MeerkatMachineEffect::LiveProviderTurnStarted {
                    channel_id: effect_channel,
                    interaction_id: effect_interaction,
                    provider_turn_ref: effect_turn,
                    ..
                } if effect_channel == &channel
                    && effect_interaction == &interaction_id.to_string()
                    && effect_turn == &provider_turn_ref
            )
        }) {
            Ok(LiveProviderTurnStartedAuthority {
                binding: runtime_binding,
                interaction_id,
                provider_turn_ref,
            })
        } else {
            Err(RuntimeDriverError::Internal(
                "generated provider turn start emitted no matching authority".to_string(),
            ))
        }
    }

    /// Freeze one typed provider Assistant TurnStarted observation to the
    /// foreground InteractionId that was current at that exact boundary.
    /// Later user turns cannot rewrite the returned opaque output handle.
    #[cfg(feature = "live")]
    pub async fn observe_live_assistant_turn_started(
        &self,
        observation: &meerkat_live::LiveSidebandObservation,
    ) -> Result<LiveAssistantOutputHandle, RuntimeDriverError> {
        let meerkat_live::LiveSidebandObservationKind::TurnStarted {
            turn,
            role: meerkat_live::LiveSidebandTurnRole::Assistant,
        } = observation.kind()
        else {
            return Err(RuntimeDriverError::ValidationFailed {
                reason: "assistant turn-start bridge requires typed Assistant TurnStarted evidence"
                    .to_string(),
            });
        };
        let provider_binding = observation.binding();
        let session_id = provider_binding.session_id();
        let channel_id = provider_binding.channel_id();
        let _mutation_guard = self
            .lock_current_durability_ready_session_mutation_gate(session_id)
            .await?;
        let state = self.session_dsl_state(session_id).await.map_err(|reason| {
            RuntimeDriverError::ValidationFailed {
                reason: reason.to_string(),
            }
        })?;
        let channel = channel_id.to_string();
        let runtime_id = state
            .live_execution_runtime_id_by_channel
            .get(&channel)
            .ok_or_else(|| RuntimeDriverError::ValidationFailed {
                reason: "assistant turn started before generated execution binding".to_string(),
            })?;
        let fence = state
            .live_execution_fence_by_channel
            .get(&channel)
            .ok_or_else(|| RuntimeDriverError::ValidationFailed {
                reason: "assistant turn has no generated fence binding".to_string(),
            })?;
        let generation = state
            .live_execution_generation_by_channel
            .get(&channel)
            .ok_or_else(|| RuntimeDriverError::ValidationFailed {
                reason: "assistant turn has no generated generation binding".to_string(),
            })?;
        if fence.0 != provider_binding.runtime_fence().get()
            || generation.0 != provider_binding.runtime_generation().get()
            || state.live_channel_session_by_channel.get(&channel) != Some(&session_id.to_string())
        {
            return Err(RuntimeDriverError::ValidationFailed {
                reason: "assistant turn-start evidence is stale for the active execution binding"
                    .to_string(),
            });
        }
        let assistant_turn_ref = turn.adapter_key().to_string();
        let runtime_binding = crate::live_execution::LiveDelegationRuntimeBinding::new(
            session_id.clone(),
            channel_id.clone(),
            crate::identifiers::LogicalRuntimeId::new(runtime_id.0.clone()),
            fence.0,
            generation.0,
        );
        let (_, effects) = self
            .apply_session_dsl_input(
                session_id,
                crate::meerkat_machine::dsl::MeerkatMachineInput::ObserveLiveAssistantTurnStarted {
                    channel_id: channel.clone(),
                    runtime_id: runtime_id.clone(),
                    fence_token: *fence,
                    generation: *generation,
                    assistant_turn_ref: assistant_turn_ref.clone(),
                },
                "ObserveLiveAssistantTurnStarted",
            )
            .await
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
        let interaction = effects.as_slice().iter().find_map(|effect| {
            let crate::meerkat_machine::dsl::MeerkatMachineEffect::LiveAssistantTurnStarted {
                channel_id: effect_channel,
                interaction_id,
                assistant_turn_ref: effect_turn,
            } = effect
            else {
                return None;
            };
            (effect_channel == &channel && effect_turn == &assistant_turn_ref)
                .then_some(interaction_id.as_str())
        });
        let interaction_id = interaction
            .and_then(|value| value.parse::<uuid::Uuid>().ok())
            .map(meerkat_core::InteractionId)
            .ok_or_else(|| {
                RuntimeDriverError::Internal(
                    "generated assistant turn start emitted no matching interaction authority"
                        .to_string(),
                )
            })?;
        let handle = LiveAssistantOutputHandle {
            binding: runtime_binding,
            interaction_id,
            assistant_turn_ref,
            output_id: uuid::Uuid::new_v4().to_string(),
            target: Arc::new(std::sync::Mutex::new(None)),
            terminal_reserved: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            terminal_consumed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        };
        self.live_assistant_output_by_turn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(
                (
                    session_id.clone(),
                    channel_id.clone(),
                    handle.__assistant_turn_ref().to_string(),
                ),
                handle.clone(),
            );
        self.live_assistant_output_by_id
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(handle.output_id().to_string(), handle.clone());
        Ok(handle)
    }

    /// Resolve the generated assistant handle captured at the exact typed
    /// Assistant TurnStarted boundary.
    #[cfg(feature = "live")]
    pub fn live_assistant_output_handle_for_turn(
        &self,
        session_id: &meerkat_core::SessionId,
        channel_id: &meerkat_core::LiveChannelId,
        assistant_turn_ref: &str,
    ) -> Option<LiveAssistantOutputHandle> {
        self.live_assistant_output_by_turn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&(
                session_id.clone(),
                channel_id.clone(),
                assistant_turn_ref.to_string(),
            ))
            .cloned()
    }

    #[cfg(feature = "live")]
    pub fn live_assistant_output_handle(
        &self,
        output_id: &str,
    ) -> Option<LiveAssistantOutputHandle> {
        self.live_assistant_output_by_id
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(output_id)
            .cloned()
    }

    /// Reserve one exact public output address for terminal dispatch.
    ///
    /// The reservation auto-releases on pre-acceptance failure. Only
    /// `commit_live_assistant_output_terminal` permanently consumes and
    /// removes the address after the terminal operation succeeds.
    #[cfg(feature = "live")]
    pub async fn reserve_live_assistant_output_handle(
        &self,
        session_id: &meerkat_core::SessionId,
        channel_id: &meerkat_core::LiveChannelId,
        output_id: &str,
    ) -> Result<LiveAssistantOutputTerminalReservation, RuntimeDriverError> {
        let lifecycle_lease = self.acquire_live_open_lifecycle_lease(session_id).await?;
        let handle = self
            .live_assistant_output_by_id
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(output_id)
            .cloned()
            .ok_or_else(|| RuntimeDriverError::ValidationFailed {
                reason: "assistant output handle is stale or already consumed".to_string(),
            })?;
        if handle.binding().session_id() != session_id
            || handle.binding().channel_id() != channel_id
        {
            return Err(RuntimeDriverError::ValidationFailed {
                reason: "assistant output handle does not belong to this channel".to_string(),
            });
        }
        let current = self
            .live_delegation_runtime_binding(session_id, channel_id)
            .await
            .map_err(|_| RuntimeDriverError::ValidationFailed {
                reason: "assistant output handle has no current generated channel binding"
                    .to_string(),
            })?;
        if &current != handle.binding() {
            return Err(RuntimeDriverError::ValidationFailed {
                reason: "assistant output handle belongs to a stale channel incarnation"
                    .to_string(),
            });
        }
        handle
            .__reserve_for_playback_terminal(
                handle.binding(),
                handle.interaction_id(),
                handle.__assistant_turn_ref(),
            )
            .map_err(|error| RuntimeDriverError::ValidationFailed {
                reason: error.to_string(),
            })?;
        Ok(LiveAssistantOutputTerminalReservation {
            handle,
            finalized: false,
            _lifecycle_lease: lifecycle_lease,
        })
    }

    /// Permanently consume a reserved output only after its exact terminal
    /// operation has succeeded.
    #[cfg(feature = "live")]
    pub fn commit_live_assistant_output_terminal(
        &self,
        reservation: LiveAssistantOutputTerminalReservation,
    ) -> Result<LiveAssistantOutputHandle, RuntimeDriverError> {
        let handle =
            reservation
                .commit()
                .map_err(|error| RuntimeDriverError::ValidationFailed {
                    reason: error.to_string(),
                })?;
        let output_id = handle.output_id().to_string();
        let session_id = handle.binding().session_id().clone();
        let channel_id = handle.binding().channel_id().clone();
        self.live_assistant_output_by_id
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&output_id);
        self.live_assistant_output_by_turn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&(
                session_id.clone(),
                channel_id.clone(),
                handle.__assistant_turn_ref().to_string(),
            ));
        Ok(handle)
    }

    #[cfg(feature = "live")]
    pub fn retire_live_assistant_output_handles(
        &self,
        session_id: &meerkat_core::SessionId,
        channel_id: &meerkat_core::LiveChannelId,
    ) {
        let retired_ids = {
            let mut by_turn = self
                .live_assistant_output_by_turn
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let keys = by_turn
                .keys()
                .filter(|(session, channel, _)| session == session_id && channel == channel_id)
                .cloned()
                .collect::<Vec<_>>();
            keys.into_iter()
                .filter_map(|key| {
                    by_turn
                        .remove(&key)
                        .map(|handle| handle.output_id().to_string())
                })
                .collect::<Vec<_>>()
        };
        let mut by_id = self
            .live_assistant_output_by_id
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for output_id in retired_ids {
            by_id.remove(&output_id);
        }
    }

    /// Complete only the exact typed provider turn previously joined to its
    /// machine-minted InteractionId.
    #[cfg(feature = "live")]
    pub async fn observe_live_provider_turn_finished(
        &self,
        observation: &meerkat_live::LiveSidebandObservation,
    ) -> Result<LiveProviderTurnFinishedAuthority, RuntimeDriverError> {
        let meerkat_live::LiveSidebandObservationKind::TurnFinished {
            turn,
            role: meerkat_live::LiveSidebandTurnRole::User,
            ..
        } = observation.kind()
        else {
            return Err(RuntimeDriverError::ValidationFailed {
                reason: "provider turn-finish bridge requires typed TurnFinished evidence"
                    .to_string(),
            });
        };
        let provider_binding = observation.binding();
        let session_id = provider_binding.session_id();
        let channel_id = provider_binding.channel_id();
        let _mutation_guard = self
            .lock_current_durability_ready_session_mutation_gate(session_id)
            .await?;
        let state = self.session_dsl_state(session_id).await.map_err(|reason| {
            RuntimeDriverError::ValidationFailed {
                reason: reason.to_string(),
            }
        })?;
        let channel = channel_id.to_string();
        let runtime_id = state
            .live_execution_runtime_id_by_channel
            .get(&channel)
            .ok_or_else(|| RuntimeDriverError::ValidationFailed {
                reason: "provider turn finished without generated execution binding".to_string(),
            })?;
        let fence = state
            .live_execution_fence_by_channel
            .get(&channel)
            .ok_or_else(|| RuntimeDriverError::ValidationFailed {
                reason: "provider turn finish has no generated fence binding".to_string(),
            })?;
        let generation = state
            .live_execution_generation_by_channel
            .get(&channel)
            .ok_or_else(|| RuntimeDriverError::ValidationFailed {
                reason: "provider turn finish has no generated generation binding".to_string(),
            })?;
        if fence.0 != provider_binding.runtime_fence().get()
            || generation.0 != provider_binding.runtime_generation().get()
            || state.live_channel_session_by_channel.get(&channel) != Some(&session_id.to_string())
        {
            return Err(RuntimeDriverError::ValidationFailed {
                reason: "provider turn-finish evidence is stale for the active execution binding"
                    .to_string(),
            });
        }
        let provider_turn_ref = turn.adapter_key().to_string();
        let runtime_binding = crate::live_execution::LiveDelegationRuntimeBinding::new(
            session_id.clone(),
            channel_id.clone(),
            crate::identifiers::LogicalRuntimeId::new(runtime_id.0.clone()),
            fence.0,
            generation.0,
        );
        let (_, effects) = self
            .apply_session_dsl_input(
                session_id,
                crate::meerkat_machine::dsl::MeerkatMachineInput::CompleteLiveInteraction {
                    channel_id: channel.clone(),
                    runtime_id: runtime_id.clone(),
                    fence_token: *fence,
                    generation: *generation,
                    provider_turn_ref: provider_turn_ref.clone(),
                },
                "CompleteLiveInteraction",
            )
            .await
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
        for effect in effects.as_slice() {
            let crate::meerkat_machine::dsl::MeerkatMachineEffect::LiveProviderTurnFinished {
                channel_id: effect_channel,
                interaction_id,
                provider_turn_ref: effect_turn,
                ..
            } = effect
            else {
                continue;
            };
            if effect_channel != &channel || effect_turn != &provider_turn_ref {
                return Err(RuntimeDriverError::Internal(
                    "generated provider turn finish effect did not match exact evidence"
                        .to_string(),
                ));
            }
            let interaction_id = uuid::Uuid::parse_str(interaction_id)
                .map(meerkat_core::InteractionId)
                .map_err(|_| {
                    RuntimeDriverError::Internal(
                        "generated provider turn finish carried an invalid InteractionId"
                            .to_string(),
                    )
                })?;
            return Ok(LiveProviderTurnFinishedAuthority {
                binding: runtime_binding,
                interaction_id,
                provider_turn_ref,
            });
        }
        Err(RuntimeDriverError::Internal(
            "generated provider turn finish emitted no matching authority".to_string(),
        ))
    }

    /// Admit the interaction and its exact actionable delegation join.
    pub async fn admit_live_delegation(
        &self,
        binding: &crate::live_execution::LiveDelegationRuntimeBinding,
        operation: &meerkat_core::exact_operation::ExactOperationIdentity<
            meerkat_core::LiveUserTurnCorrelation,
        >,
        provisional: &meerkat_core::ProvisionalLiveHandoff,
    ) -> Result<(), RuntimeDriverError> {
        let session_id = binding.session_id();
        let correlation = operation.domain_correlation();
        if correlation.channel_id() != binding.channel_id()
            || provisional.correlation() != correlation
        {
            return Err(RuntimeDriverError::ValidationFailed {
                reason: "live delegation admission does not match the exact runtime binding"
                    .to_string(),
            });
        }
        let _mutation_guard = self
            .lock_current_durability_ready_session_mutation_gate(session_id)
            .await?;
        let (_, effects) = self
            .apply_session_dsl_input(
                session_id,
                crate::meerkat_machine::dsl::MeerkatMachineInput::AdmitLiveDelegation {
                    channel_id: correlation.channel_id().to_string(),
                    runtime_id: crate::meerkat_machine::dsl::AgentRuntimeId::from_domain(
                        binding.runtime_id(),
                    ),
                    fence_token: crate::meerkat_machine::dsl::FenceToken::from_domain(
                        binding.fence_token(),
                    ),
                    generation: crate::meerkat_machine::dsl::Generation::from_domain(
                        binding.generation(),
                    ),
                    interaction_id: correlation.interaction_id().to_string(),
                    operation_id: crate::meerkat_machine::dsl::OperationId::from_domain(
                        operation.operation_id(),
                    ),
                    provider_turn_correlation: correlation.provider().user_turn_id().to_string(),
                    delegation_identity_present: true,
                    actionable_input_present: !provisional.executor_input().trim().is_empty(),
                    exact_join: true,
                },
                "AdmitLiveDelegationForProviderTurn",
            )
            .await
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
        let admitted = effects.as_slice().iter().any(|effect| matches!(
            effect,
            crate::meerkat_machine::dsl::MeerkatMachineEffect::LiveDelegationAdmitted {
                operation_id,
                interaction_id,
                ..
            } if operation_id == &crate::meerkat_machine::dsl::OperationId::from_domain(operation.operation_id())
                && interaction_id == &correlation.interaction_id().to_string()
        ));
        if admitted {
            Ok(())
        } else {
            Err(RuntimeDriverError::Internal(
                "generated live delegation admission emitted no matching effect".to_string(),
            ))
        }
    }

    /// Admit only the delegation join when a generated supersession transition
    /// already admitted the replacement interaction atomically.
    pub async fn admit_live_delegation_for_active_interaction(
        &self,
        binding: &crate::live_execution::LiveDelegationRuntimeBinding,
        operation: &meerkat_core::exact_operation::ExactOperationIdentity<
            meerkat_core::LiveUserTurnCorrelation,
        >,
        provisional: &meerkat_core::ProvisionalLiveHandoff,
    ) -> Result<(), RuntimeDriverError> {
        let session_id = binding.session_id();
        let correlation = operation.domain_correlation();
        if correlation.channel_id() != binding.channel_id()
            || provisional.correlation() != correlation
        {
            return Err(RuntimeDriverError::ValidationFailed {
                reason: "live delegation replacement does not match the exact runtime binding"
                    .to_string(),
            });
        }
        let _mutation_guard = self
            .lock_current_durability_ready_session_mutation_gate(session_id)
            .await?;
        let (_, effects) = self
            .apply_session_dsl_input(
                session_id,
                crate::meerkat_machine::dsl::MeerkatMachineInput::AdmitLiveDelegation {
                    channel_id: correlation.channel_id().to_string(),
                    runtime_id: crate::meerkat_machine::dsl::AgentRuntimeId::from_domain(
                        binding.runtime_id(),
                    ),
                    fence_token: crate::meerkat_machine::dsl::FenceToken::from_domain(
                        binding.fence_token(),
                    ),
                    generation: crate::meerkat_machine::dsl::Generation::from_domain(
                        binding.generation(),
                    ),
                    interaction_id: correlation.interaction_id().to_string(),
                    operation_id: crate::meerkat_machine::dsl::OperationId::from_domain(
                        operation.operation_id(),
                    ),
                    provider_turn_correlation: correlation.provider().user_turn_id().to_string(),
                    delegation_identity_present: true,
                    actionable_input_present: !provisional.executor_input().trim().is_empty(),
                    exact_join: true,
                },
                "AdmitLiveDelegationAfterSupersession",
            )
            .await
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
        if effects.as_slice().iter().any(|effect| matches!(
            effect,
            crate::meerkat_machine::dsl::MeerkatMachineEffect::LiveDelegationAdmitted {
                operation_id,
                ..
            } if operation_id == &crate::meerkat_machine::dsl::OperationId::from_domain(operation.operation_id())
        )) {
            Ok(())
        } else {
            Err(RuntimeDriverError::Internal(
                "generated replacement delegation admission emitted no matching effect".to_string(),
            ))
        }
    }

    /// Bind and authorize the durable worker for one exact admitted delegation.
    pub async fn authorize_live_delegation_worker_start(
        &self,
        session_id: &SessionId,
        runtime_id: &crate::identifiers::LogicalRuntimeId,
        fence_token: u64,
        generation: u64,
        operation: &meerkat_core::exact_operation::ExactOperationIdentity<
            meerkat_core::LiveUserTurnCorrelation,
        >,
        provisional: &meerkat_core::ProvisionalLiveHandoff,
        worker_identity: &str,
    ) -> Result<crate::live_execution::LiveDelegationExecutionAdmission, RuntimeDriverError> {
        let correlation = operation.domain_correlation();
        if provisional.correlation() != correlation || worker_identity.is_empty() {
            return Err(RuntimeDriverError::ValidationFailed {
                reason: "live worker start does not match the exact admitted operation".to_string(),
            });
        }
        let _mutation_guard = self
            .lock_current_durability_ready_session_mutation_gate(session_id)
            .await?;
        let (_, effects) = self
            .apply_session_dsl_input(
                session_id,
                crate::meerkat_machine::dsl::MeerkatMachineInput::AuthorizeLiveDelegationWorkerStart {
                    channel_id: correlation.channel_id().to_string(),
                    runtime_id: crate::meerkat_machine::dsl::AgentRuntimeId::from_domain(runtime_id),
                    fence_token: crate::meerkat_machine::dsl::FenceToken::from_domain(fence_token),
                    generation: crate::meerkat_machine::dsl::Generation::from_domain(generation),
                    interaction_id: correlation.interaction_id().to_string(),
                    operation_id: crate::meerkat_machine::dsl::OperationId::from_domain(operation.operation_id()),
                    provider_turn_correlation: correlation.provider().user_turn_id().to_string(),
                    worker_identity: worker_identity.to_string(),
                },
                "AuthorizeLiveDelegationWorkerStart",
            )
            .await
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
        for effect in effects.as_slice() {
            if let Some(admission) =
                crate::live_execution::LiveDelegationExecutionAdmission::from_generated_effect(
                    session_id,
                    operation,
                    provisional,
                    worker_identity,
                    effect,
                )
                .map_err(|error| RuntimeDriverError::Internal(error.to_string()))?
            {
                return Ok(admission);
            }
        }
        Err(RuntimeDriverError::Internal(
            "generated worker start authorization emitted no matching effect".to_string(),
        ))
    }

    /// Resolve the shell's mechanical worker-start attempt.
    pub async fn resolve_live_delegation_worker_start(
        &self,
        runtime_id: &crate::identifiers::LogicalRuntimeId,
        fence_token: u64,
        generation: u64,
        admission: &crate::live_execution::LiveDelegationExecutionAdmission,
        started: bool,
    ) -> Result<(), RuntimeDriverError> {
        let session_id = admission.session_id();
        let operation = admission.operation();
        let correlation = operation.domain_correlation();
        let _mutation_guard = self
            .lock_current_durability_ready_session_mutation_gate(session_id)
            .await?;
        let (_, effects) = self
            .apply_session_dsl_input(
                session_id,
                crate::meerkat_machine::dsl::MeerkatMachineInput::ResolveLiveDelegationWorkerStart {
                    channel_id: correlation.channel_id().to_string(),
                    runtime_id: crate::meerkat_machine::dsl::AgentRuntimeId::from_domain(runtime_id),
                    fence_token: crate::meerkat_machine::dsl::FenceToken::from_domain(fence_token),
                    generation: crate::meerkat_machine::dsl::Generation::from_domain(generation),
                    interaction_id: correlation.interaction_id().to_string(),
                    operation_id: crate::meerkat_machine::dsl::OperationId::from_domain(operation.operation_id()),
                    worker_identity: admission.worker_identity().to_string(),
                    started,
                },
                "ResolveLiveDelegationWorkerStart",
            )
            .await
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
        let matched = effects.as_slice().iter().any(|effect| matches!(
            effect,
            crate::meerkat_machine::dsl::MeerkatMachineEffect::LiveDelegationWorkerStartResolved {
                operation_id,
                worker_identity,
                started: observed,
                ..
            } if operation_id == &crate::meerkat_machine::dsl::OperationId::from_domain(operation.operation_id())
                && worker_identity == admission.worker_identity()
                && observed == &started
        ));
        if matched {
            if !started {
                admission.close_tool_execution_after_generated_terminal();
            }
            Ok(())
        } else {
            Err(RuntimeDriverError::Internal(
                "generated worker start resolution emitted no matching effect".to_string(),
            ))
        }
    }

    /// Authorize cancellation after transcript reconciliation reached a
    /// machine-derived negative terminal classification.
    pub async fn authorize_live_delegation_transcript_cancellation(
        &self,
        runtime_id: &crate::identifiers::LogicalRuntimeId,
        fence_token: u64,
        generation: u64,
        admission: &crate::live_execution::LiveDelegationExecutionAdmission,
    ) -> Result<crate::live_execution::LiveDelegationCancellationAuthority, RuntimeDriverError>
    {
        let session_id = admission.session_id();
        let operation = admission.operation();
        let correlation = operation.domain_correlation();
        let _mutation_guard = self
            .lock_current_durability_ready_session_mutation_gate(session_id)
            .await?;
        let (_, effects) = self
            .apply_session_dsl_input(
                session_id,
                crate::meerkat_machine::dsl::MeerkatMachineInput::AuthorizeLiveDelegationTranscriptTerminalCancellation {
                    channel_id: correlation.channel_id().to_string(),
                    runtime_id: crate::meerkat_machine::dsl::AgentRuntimeId::from_domain(runtime_id),
                    fence_token: crate::meerkat_machine::dsl::FenceToken::from_domain(fence_token),
                    generation: crate::meerkat_machine::dsl::Generation::from_domain(generation),
                    interaction_id: correlation.interaction_id().to_string(),
                    operation_id: crate::meerkat_machine::dsl::OperationId::from_domain(operation.operation_id()),
                    worker_identity: admission.worker_identity().to_string(),
                },
                "AuthorizeLiveDelegationTranscriptTerminalCancellation",
            )
            .await
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
        self.live_delegation_cancellation_authority_from_effects(admission, &effects)
    }

    /// Abandon one active live delegation and return the generated total
    /// cancellation classification. The tool gate closes only after the exact
    /// generated lifecycle effect.
    pub async fn abandon_live_delegation(
        &self,
        runtime_id: &crate::identifiers::LogicalRuntimeId,
        fence_token: u64,
        generation: u64,
        admission: &crate::live_execution::LiveDelegationExecutionAdmission,
    ) -> Result<crate::live_execution::LiveDelegationCancellationDirective, RuntimeDriverError>
    {
        let session_id = admission.session_id();
        let operation = admission.operation();
        let correlation = operation.domain_correlation();
        let _mutation_guard = self
            .lock_current_durability_ready_session_mutation_gate(session_id)
            .await?;
        let (_, effects) = self
            .apply_session_dsl_input(
                session_id,
                crate::meerkat_machine::dsl::MeerkatMachineInput::AbandonLiveInteraction {
                    channel_id: correlation.channel_id().to_string(),
                    runtime_id: crate::meerkat_machine::dsl::AgentRuntimeId::from_domain(
                        runtime_id,
                    ),
                    fence_token: crate::meerkat_machine::dsl::FenceToken::from_domain(fence_token),
                    generation: crate::meerkat_machine::dsl::Generation::from_domain(generation),
                    interaction_id: correlation.interaction_id().to_string(),
                },
                "AbandonLiveInteractionWithDelegationCancellation",
            )
            .await
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
        for effect in effects.as_slice() {
            if let Some(authority) =
                crate::live_execution::LiveDelegationCancellationAuthority::from_generated_effect(
                    admission.session_id(),
                    admission.operation(),
                    admission.worker_identity(),
                    effect,
                )
                .map_err(|error| RuntimeDriverError::Internal(error.to_string()))?
            {
                if authority.reason()
                    != crate::live_execution::LiveDelegationCancellationReason::Abandoned
                {
                    return Err(RuntimeDriverError::Internal(
                        "generated abandon edge emitted the wrong cancellation reason".to_string(),
                    ));
                }
                admission.close_tool_execution_after_generated_terminal();
                return Ok(crate::live_execution::LiveDelegationCancellationDirective::CancellationAuthorized(authority));
            }
            if let Some(receipt) = crate::live_execution::LiveDelegationNoCancellationReceipt::from_generated_abandonment_effect(
                admission.operation(),
                effect,
            )
            .map_err(|error| RuntimeDriverError::Internal(error.to_string()))?
            {
                admission.close_tool_execution_after_generated_terminal();
                return Ok(crate::live_execution::LiveDelegationCancellationDirective::NoCancellationRequired(receipt));
            }
        }
        Err(RuntimeDriverError::Internal(
            "generated abandon edge emitted no exact cancellation classification".to_string(),
        ))
    }

    /// Atomically supersede one live interaction and return the generated total
    /// cancellation classification for its exact worker binding.
    pub async fn supersede_live_delegation(
        &self,
        runtime_id: &crate::identifiers::LogicalRuntimeId,
        fence_token: u64,
        generation: u64,
        admission: &crate::live_execution::LiveDelegationExecutionAdmission,
        superseding_interaction_id: meerkat_core::InteractionId,
    ) -> Result<crate::live_execution::LiveDelegationCancellationDirective, RuntimeDriverError>
    {
        let session_id = admission.session_id();
        let operation = admission.operation();
        let correlation = operation.domain_correlation();
        let _mutation_guard = self
            .lock_current_durability_ready_session_mutation_gate(session_id)
            .await?;
        let (_, effects) = self
            .apply_session_dsl_input(
                session_id,
                crate::meerkat_machine::dsl::MeerkatMachineInput::SupersedeLiveInteraction {
                    session_id: session_id.to_string(),
                    channel_id: correlation.channel_id().to_string(),
                    runtime_id: crate::meerkat_machine::dsl::AgentRuntimeId::from_domain(
                        runtime_id,
                    ),
                    fence_token: crate::meerkat_machine::dsl::FenceToken::from_domain(fence_token),
                    generation: crate::meerkat_machine::dsl::Generation::from_domain(generation),
                    interaction_id: correlation.interaction_id().to_string(),
                    superseding_interaction_id: superseding_interaction_id.to_string(),
                    operation_id: crate::meerkat_machine::dsl::OperationId::from_domain(
                        operation.operation_id(),
                    ),
                    worker_identity: admission.worker_identity().to_string(),
                },
                "SupersedeLiveInteractionWithDelegationCancellation",
            )
            .await
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
        for effect in effects.as_slice() {
            if let Some(authority) = crate::live_execution::LiveDelegationCancellationAuthority::from_generated_supersession_effect(
                admission.session_id(),
                admission.operation(),
                admission.worker_identity(),
                superseding_interaction_id,
                effect,
            )
            .map_err(|error| RuntimeDriverError::Internal(error.to_string()))?
            {
                admission.close_tool_execution_after_generated_terminal();
                return Ok(crate::live_execution::LiveDelegationCancellationDirective::CancellationAuthorized(authority));
            }
            if let Some(receipt) = crate::live_execution::LiveDelegationNoCancellationReceipt::from_generated_supersession_effect(
                admission.operation(),
                superseding_interaction_id,
                effect,
            )
            .map_err(|error| RuntimeDriverError::Internal(error.to_string()))?
            {
                admission.close_tool_execution_after_generated_terminal();
                return Ok(crate::live_execution::LiveDelegationCancellationDirective::NoCancellationRequired(receipt));
            }
        }
        Err(RuntimeDriverError::Internal(
            "generated supersede edge emitted no exact cancellation classification".to_string(),
        ))
    }

    fn live_delegation_cancellation_authority_from_effects(
        &self,
        admission: &crate::live_execution::LiveDelegationExecutionAdmission,
        effects: &DslTransitionEffects,
    ) -> Result<crate::live_execution::LiveDelegationCancellationAuthority, RuntimeDriverError>
    {
        for effect in effects.as_slice() {
            if let Some(authority) =
                crate::live_execution::LiveDelegationCancellationAuthority::from_generated_effect(
                    admission.session_id(),
                    admission.operation(),
                    admission.worker_identity(),
                    effect,
                )
                .map_err(|error| RuntimeDriverError::Internal(error.to_string()))?
            {
                admission.close_tool_execution_after_generated_terminal();
                return Ok(authority);
            }
        }
        Err(RuntimeDriverError::Internal(
            "generated live cancellation authorization emitted no matching effect".to_string(),
        ))
    }

    /// Reconcile provider-final user input for one exact live delegation.
    ///
    /// `Confirmed` is derived only from exact digest equality with sealed
    /// SessionDocument canonical-commit evidence. Callers cannot provide a
    /// classification or copied generated effect.
    pub async fn reconcile_live_delegation_transcript(
        &self,
        session_id: &SessionId,
        runtime_id: &crate::identifiers::LogicalRuntimeId,
        fence_token: u64,
        generation: u64,
        operation: &meerkat_core::exact_operation::ExactOperationIdentity<
            meerkat_core::LiveUserTurnCorrelation,
        >,
        provisional: &meerkat_core::ProvisionalLiveHandoff,
        final_transcript: &meerkat_core::FinalLiveUserTranscriptCommitEvidence,
    ) -> Result<crate::live_execution::LiveHandoffReconciliationReceipt, RuntimeDriverError> {
        if final_transcript.session_id() != session_id {
            return Err(RuntimeDriverError::ValidationFailed {
                reason: "canonical live transcript evidence belongs to another session".to_string(),
            });
        }
        let derived_reconciliation = crate::live_execution::reconciliation_from_final_transcript(
            operation,
            provisional,
            final_transcript,
        )
        .map_err(|error| RuntimeDriverError::ValidationFailed {
            reason: error.to_string(),
        })?;
        let _mutation_guard = self
            .lock_current_durability_ready_session_mutation_gate(session_id)
            .await?;
        let correlation = operation.domain_correlation();
        let (final_transcript_committed, normalized_digest_matches) = match derived_reconciliation {
            meerkat_core::LiveHandoffReconciliation::Confirmed => (true, true),
            meerkat_core::LiveHandoffReconciliation::MaterialConflict => (true, false),
            meerkat_core::LiveHandoffReconciliation::Missing => (false, false),
        };
        let (_, effects) = self
            .apply_session_dsl_input(
                session_id,
                crate::meerkat_machine::dsl::MeerkatMachineInput::ReconcileLiveDelegationTranscript {
                    channel_id: correlation.channel_id().to_string(),
                    runtime_id: crate::meerkat_machine::dsl::AgentRuntimeId::from_domain(runtime_id),
                    fence_token: crate::meerkat_machine::dsl::FenceToken::from_domain(fence_token),
                    generation: crate::meerkat_machine::dsl::Generation::from_domain(generation),
                    interaction_id: correlation.interaction_id().to_string(),
                    operation_id: crate::meerkat_machine::dsl::OperationId::from_domain(
                        operation.operation_id(),
                    ),
                    provider_turn_correlation: correlation.provider().user_turn_id().to_string(),
                    final_transcript_committed,
                    normalized_digest_matches,
                },
                "ReconcileLiveDelegationTranscript",
            )
            .await
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;

        for effect in effects.as_slice() {
            if let Some(receipt) =
                crate::live_execution::LiveHandoffReconciliationReceipt::from_generated_effect(
                    session_id,
                    operation,
                    provisional,
                    derived_reconciliation,
                    effect,
                )
                .map_err(|error| RuntimeDriverError::Internal(error.to_string()))?
            {
                return Ok(receipt);
            }
        }
        Err(RuntimeDriverError::Internal(
            "generated live delegation reconciliation emitted no matching authority effect"
                .to_string(),
        ))
    }

    /// Authorize consequential dispatch for one confirmed live delegation.
    ///
    /// A fresh authority identity is minted inside the runtime. The caller
    /// cannot provide a copied generated effect or choose the authority key.
    pub async fn authorize_live_consequential_effect(
        &self,
        session_id: &SessionId,
        runtime_id: &crate::identifiers::LogicalRuntimeId,
        fence_token: u64,
        generation: u64,
        operation: &meerkat_core::exact_operation::ExactOperationIdentity<
            meerkat_core::LiveUserTurnCorrelation,
        >,
        reconciliation: &crate::live_execution::LiveHandoffReconciliationReceipt,
    ) -> Result<crate::live_execution::FinalUserInputOperationWitness, RuntimeDriverError> {
        if reconciliation.admission().session_id() != session_id
            || reconciliation.admission().operation() != operation
            || reconciliation.disposition() != meerkat_core::LiveHandoffReconciliation::Confirmed
        {
            return Err(RuntimeDriverError::ValidationFailed {
                reason: "live consequential authority does not match the exact confirmed session operation"
                    .to_string(),
            });
        }
        let _mutation_guard = self
            .lock_current_durability_ready_session_mutation_gate(session_id)
            .await?;
        let correlation = operation.domain_correlation();
        let (_, effects) = self
            .apply_session_dsl_input(
                session_id,
                crate::meerkat_machine::dsl::MeerkatMachineInput::AuthorizeLiveConsequentialEffect {
                    channel_id: correlation.channel_id().to_string(),
                    runtime_id: crate::meerkat_machine::dsl::AgentRuntimeId::from_domain(runtime_id),
                    fence_token: crate::meerkat_machine::dsl::FenceToken::from_domain(fence_token),
                    generation: crate::meerkat_machine::dsl::Generation::from_domain(generation),
                    interaction_id: correlation.interaction_id().to_string(),
                    operation_id: crate::meerkat_machine::dsl::OperationId::from_domain(
                        operation.operation_id(),
                    ),
                    authority_id: uuid::Uuid::new_v4().to_string(),
                },
                "AuthorizeLiveConsequentialEffect",
            )
            .await
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;

        for effect in effects.as_slice() {
            if let Some(witness) =
                crate::live_execution::FinalUserInputOperationWitness::from_generated_effect(
                    session_id,
                    operation,
                    reconciliation,
                    effect,
                )
                .map_err(|error| RuntimeDriverError::Internal(error.to_string()))?
            {
                return Ok(witness);
            }
        }
        Err(RuntimeDriverError::Internal(
            "generated consequential authorization emitted no matching authority effect"
                .to_string(),
        ))
    }

    /// Record the exact worker terminal observation. The generated machine
    /// classifies late-terminal and result eligibility.
    pub async fn record_live_delegation_worker_terminal(
        &self,
        runtime_id: &crate::identifiers::LogicalRuntimeId,
        fence_token: u64,
        generation: u64,
        admission: &crate::live_execution::LiveDelegationExecutionAdmission,
        terminal: crate::live_execution::LiveDelegationWorkerTerminalKind,
    ) -> Result<crate::live_execution::LiveDelegationWorkerTerminalReceipt, RuntimeDriverError>
    {
        let session_id = admission.session_id();
        let operation = admission.operation();
        let correlation = operation.domain_correlation();
        let _mutation_guard = self
            .lock_current_durability_ready_session_mutation_gate(session_id)
            .await?;
        let (_, effects) = self
            .apply_session_dsl_input(
                session_id,
                crate::meerkat_machine::dsl::MeerkatMachineInput::RecordLiveDelegationWorkerTerminal {
                    channel_id: correlation.channel_id().to_string(),
                    runtime_id: crate::meerkat_machine::dsl::AgentRuntimeId::from_domain(runtime_id),
                    fence_token: crate::meerkat_machine::dsl::FenceToken::from_domain(fence_token),
                    generation: crate::meerkat_machine::dsl::Generation::from_domain(generation),
                    interaction_id: correlation.interaction_id().to_string(),
                    operation_id: crate::meerkat_machine::dsl::OperationId::from_domain(operation.operation_id()),
                    worker_identity: admission.worker_identity().to_string(),
                    terminal: terminal.into(),
                },
                "RecordLiveDelegationWorkerTerminal",
            )
            .await
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
        admission.close_tool_execution_after_generated_terminal();
        for effect in effects.as_slice() {
            if let Some(receipt) =
                crate::live_execution::LiveDelegationWorkerTerminalReceipt::from_generated_effect(
                    operation,
                    admission.worker_identity(),
                    terminal,
                    effect,
                )
                .map_err(|error| RuntimeDriverError::Internal(error.to_string()))?
            {
                return Ok(receipt);
            }
        }
        Err(RuntimeDriverError::Internal(
            "generated worker terminal observation emitted no matching effect".to_string(),
        ))
    }

    /// Resolve the mechanical cancellation attempt under exact machine authority.
    pub async fn resolve_live_delegation_cancellation(
        &self,
        runtime_id: &crate::identifiers::LogicalRuntimeId,
        fence_token: u64,
        generation: u64,
        authority: &crate::live_execution::LiveDelegationCancellationAuthority,
        outcome: crate::live_execution::LiveDelegationCancellationOutcome,
    ) -> Result<(), RuntimeDriverError> {
        let session_id = authority.session_id();
        let operation = authority.operation();
        let correlation = operation.domain_correlation();
        let observed: crate::meerkat_machine::dsl::LiveDelegationCancellationOutcome =
            outcome.into();
        let _mutation_guard = self
            .lock_current_durability_ready_session_mutation_gate(session_id)
            .await?;
        let (_, effects) = self
            .apply_session_dsl_input(
                session_id,
                crate::meerkat_machine::dsl::MeerkatMachineInput::ResolveLiveDelegationCancellation {
                    channel_id: correlation.channel_id().to_string(),
                    runtime_id: crate::meerkat_machine::dsl::AgentRuntimeId::from_domain(runtime_id),
                    fence_token: crate::meerkat_machine::dsl::FenceToken::from_domain(fence_token),
                    generation: crate::meerkat_machine::dsl::Generation::from_domain(generation),
                    interaction_id: correlation.interaction_id().to_string(),
                    operation_id: crate::meerkat_machine::dsl::OperationId::from_domain(operation.operation_id()),
                    worker_identity: authority.worker_identity().to_string(),
                    outcome: observed,
                },
                "ResolveLiveDelegationCancellation",
            )
            .await
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
        let matched = effects.as_slice().iter().any(|effect| matches!(
            effect,
            crate::meerkat_machine::dsl::MeerkatMachineEffect::LiveDelegationCancellationResolved {
                operation_id,
                worker_identity,
                outcome: resolved,
                ..
            } if operation_id == &crate::meerkat_machine::dsl::OperationId::from_domain(operation.operation_id())
                && worker_identity == authority.worker_identity()
                && resolved == &observed
        ));
        if matched {
            Ok(())
        } else {
            Err(RuntimeDriverError::Internal(
                "generated cancellation resolution emitted no matching effect".to_string(),
            ))
        }
    }

    /// Authorize retirement only after the generated worker terminal edge.
    pub async fn authorize_live_delegation_worker_retirement(
        &self,
        runtime_id: &crate::identifiers::LogicalRuntimeId,
        fence_token: u64,
        generation: u64,
        admission: &crate::live_execution::LiveDelegationExecutionAdmission,
    ) -> Result<crate::live_execution::LiveDelegationWorkerRetirementAuthority, RuntimeDriverError>
    {
        let session_id = admission.session_id();
        let operation = admission.operation();
        let correlation = operation.domain_correlation();
        let _mutation_guard = self
            .lock_current_durability_ready_session_mutation_gate(session_id)
            .await?;
        let (_, effects) = self
            .apply_session_dsl_input(
                session_id,
                crate::meerkat_machine::dsl::MeerkatMachineInput::AuthorizeLiveDelegationWorkerRetirement {
                    channel_id: correlation.channel_id().to_string(),
                    runtime_id: crate::meerkat_machine::dsl::AgentRuntimeId::from_domain(runtime_id),
                    fence_token: crate::meerkat_machine::dsl::FenceToken::from_domain(fence_token),
                    generation: crate::meerkat_machine::dsl::Generation::from_domain(generation),
                    interaction_id: correlation.interaction_id().to_string(),
                    operation_id: crate::meerkat_machine::dsl::OperationId::from_domain(operation.operation_id()),
                    worker_identity: admission.worker_identity().to_string(),
                },
                "AuthorizeLiveDelegationWorkerRetirement",
            )
            .await
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
        for effect in effects.as_slice() {
            if let Some(authority) =
                crate::live_execution::LiveDelegationWorkerRetirementAuthority::from_generated_effect(
                    session_id,
                    operation,
                    admission.worker_identity(),
                    effect,
                )
                .map_err(|error| RuntimeDriverError::Internal(error.to_string()))?
            {
                return Ok(authority);
            }
        }
        Err(RuntimeDriverError::Internal(
            "generated worker retirement authorization emitted no matching effect".to_string(),
        ))
    }

    /// Resolve the shell's exact worker retirement attempt.
    pub async fn resolve_live_delegation_worker_retirement(
        &self,
        runtime_id: &crate::identifiers::LogicalRuntimeId,
        fence_token: u64,
        generation: u64,
        authority: &crate::live_execution::LiveDelegationWorkerRetirementAuthority,
        retired: bool,
    ) -> Result<(), RuntimeDriverError> {
        let session_id = authority.session_id();
        let operation = authority.operation();
        let correlation = operation.domain_correlation();
        let _mutation_guard = self
            .lock_current_durability_ready_session_mutation_gate(session_id)
            .await?;
        let (_, effects) = self
            .apply_session_dsl_input(
                session_id,
                crate::meerkat_machine::dsl::MeerkatMachineInput::ResolveLiveDelegationWorkerRetirement {
                    channel_id: correlation.channel_id().to_string(),
                    runtime_id: crate::meerkat_machine::dsl::AgentRuntimeId::from_domain(runtime_id),
                    fence_token: crate::meerkat_machine::dsl::FenceToken::from_domain(fence_token),
                    generation: crate::meerkat_machine::dsl::Generation::from_domain(generation),
                    interaction_id: correlation.interaction_id().to_string(),
                    operation_id: crate::meerkat_machine::dsl::OperationId::from_domain(operation.operation_id()),
                    worker_identity: authority.worker_identity().to_string(),
                    retired,
                },
                "ResolveLiveDelegationWorkerRetirement",
            )
            .await
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
        let matched = effects.as_slice().iter().any(|effect| matches!(
            effect,
            crate::meerkat_machine::dsl::MeerkatMachineEffect::LiveDelegationWorkerRetirementResolved {
                operation_id,
                worker_identity,
                retired: observed,
                ..
            } if operation_id == &crate::meerkat_machine::dsl::OperationId::from_domain(operation.operation_id())
                && worker_identity == authority.worker_identity()
                && observed == &retired
        ));
        if matched {
            Ok(())
        } else {
            Err(RuntimeDriverError::Internal(
                "generated worker retirement resolution emitted no matching effect".to_string(),
            ))
        }
    }

    /// Project the exact store-committed parent-session boundary into the
    /// generated live-context outbox. Absence of an active live channel is a
    /// truthful no-op; rows are never reconstructed from RunResult or events.
    #[cfg(feature = "live")]
    pub async fn enqueue_committed_parent_session_boundary(
        &self,
        session_id: &SessionId,
        committed: &meerkat_core::lifecycle::core_executor::BoundSessionCommit,
        store_commit_authority: &str,
    ) -> Result<usize, RuntimeDriverError> {
        self.enqueue_committed_session_boundary_with_provenance(
            session_id,
            committed,
            store_commit_authority,
            meerkat_core::generated::session_document::LiveContextCommittedTextProvenance::ParentSessionServiceTurn,
        )
        .await
    }

    /// Advance canonical coverage for rows committed by the active live
    /// transcript pipeline without echoing them to that same channel.
    #[cfg(feature = "live")]
    pub async fn enqueue_committed_live_transcript_boundary(
        &self,
        session_id: &SessionId,
        committed: &meerkat_core::lifecycle::core_executor::BoundSessionCommit,
        store_commit_authority: &str,
    ) -> Result<usize, RuntimeDriverError> {
        self.enqueue_committed_session_boundary_with_provenance(
            session_id,
            committed,
            store_commit_authority,
            meerkat_core::generated::session_document::LiveContextCommittedTextProvenance::LiveRealtimeTranscript,
        )
        .await
    }

    #[cfg(feature = "live")]
    async fn enqueue_committed_session_boundary_with_provenance(
        &self,
        session_id: &SessionId,
        committed: &meerkat_core::lifecycle::core_executor::BoundSessionCommit,
        store_commit_authority: &str,
        provenance: meerkat_core::generated::session_document::LiveContextCommittedTextProvenance,
    ) -> Result<usize, RuntimeDriverError> {
        let Some(channel_id) = self.live_active_channel_for_session(session_id).await else {
            return Ok(0);
        };
        let state = self.session_dsl_state(session_id).await.map_err(|reason| {
            RuntimeDriverError::ValidationFailed {
                reason: reason.to_string(),
            }
        })?;
        let channel = channel_id.as_str();
        let bound =
            live_context_execution_binding_is_complete(&state, channel).map_err(|reason| {
                RuntimeDriverError::ValidationFailed {
                    reason: reason.to_string(),
                }
            })?;
        let staged_presence = [
            state
                .live_experimental_staged_runtime_by_channel
                .contains_key(channel),
            state
                .live_experimental_staged_fence_by_channel
                .contains_key(channel),
            state
                .live_experimental_staged_generation_by_channel
                .contains_key(channel),
            state
                .live_experimental_staged_seed_cursor_by_channel
                .contains_key(channel),
        ];
        if staged_presence.iter().any(|present| *present)
            && !staged_presence.iter().all(|present| *present)
        {
            return Err(RuntimeDriverError::ValidationFailed {
                reason: "active live channel has a partial staged experimental execution binding"
                    .to_string(),
            });
        }
        let staged = staged_presence.iter().all(|present| *present);
        let recovery_source = state
            .live_context_recovery_source_by_replacement
            .get(channel)
            .cloned();
        if bound && !state.live_experimental_execution_channels.contains(channel) {
            return Ok(0);
        }
        if bound && (staged || recovery_source.is_some()) {
            return Err(RuntimeDriverError::ValidationFailed {
                reason: "bound live channel retained pre-bind experimental authority".to_string(),
            });
        }
        let (binding, authority_cursor) = if bound {
            let cursor = state
                .live_context_cursor_by_channel
                .get(channel)
                .copied()
                .ok_or_else(|| RuntimeDriverError::ValidationFailed {
                    reason: "bound live context channel has no acknowledged cursor".to_string(),
                })?;
            (
                self.live_delegation_runtime_binding(session_id, &channel_id)
                    .await?,
                cursor,
            )
        } else if staged {
            let runtime_id = state
                .live_experimental_staged_runtime_by_channel
                .get(channel)
                .expect("complete staged runtime checked above");
            let fence = state
                .live_experimental_staged_fence_by_channel
                .get(channel)
                .expect("complete staged fence checked above");
            let generation = state
                .live_experimental_staged_generation_by_channel
                .get(channel)
                .expect("complete staged generation checked above");
            let cursor = *state
                .live_experimental_staged_seed_cursor_by_channel
                .get(channel)
                .expect("complete staged seed cursor checked above");
            (
                crate::live_execution::LiveDelegationRuntimeBinding::new(
                    session_id.clone(),
                    channel_id.clone(),
                    crate::identifiers::LogicalRuntimeId::new(runtime_id.0.clone()),
                    fence.0,
                    generation.0,
                ),
                cursor,
            )
        } else if let Some(source_channel) = recovery_source {
            let runtime_id = state
                .live_context_recovery_runtime_id_by_channel
                .get(&source_channel)
                .ok_or_else(|| RuntimeDriverError::ValidationFailed {
                    reason: "context recovery replacement has no generated runtime identity"
                        .to_string(),
                })?;
            let fence = state
                .live_context_recovery_fence_by_channel
                .get(&source_channel)
                .ok_or_else(|| RuntimeDriverError::ValidationFailed {
                    reason: "context recovery replacement has no generated runtime fence"
                        .to_string(),
                })?;
            let generation = state
                .live_context_recovery_generation_by_channel
                .get(&source_channel)
                .ok_or_else(|| RuntimeDriverError::ValidationFailed {
                    reason: "context recovery replacement has no generated runtime generation"
                        .to_string(),
                })?;
            let cursor = *state
                .live_context_recovery_seed_cursor_by_channel
                .get(&source_channel)
                .ok_or_else(|| RuntimeDriverError::ValidationFailed {
                    reason: "context recovery replacement has no generated seed cursor".to_string(),
                })?;
            if state
                .live_context_recovery_session_by_channel
                .get(&source_channel)
                != Some(&session_id.to_string())
            {
                return Err(RuntimeDriverError::ValidationFailed {
                    reason: "context recovery replacement belongs to another session".to_string(),
                });
            }
            (
                crate::live_execution::LiveDelegationRuntimeBinding::new(
                    session_id.clone(),
                    channel_id.clone(),
                    crate::identifiers::LogicalRuntimeId::new(runtime_id.0.clone()),
                    fence.0,
                    generation.0,
                ),
                cursor,
            )
        } else {
            // An ordinary public realtime channel never enters the generated
            // experimental staging or recovery transitions. Its canonical
            // commit succeeds without creating experimental outbox truth.
            return Ok(0);
        };
        let queued_cursor = self
            .shared
            .live_context_queued_rows
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .keys()
            .filter_map(|(queued_session, sequence)| {
                (queued_session == session_id).then_some(*sequence)
            })
            .max()
            .unwrap_or(0);
        let canonical_cursor = authority_cursor.max(queued_cursor);
        let rows = crate::live_context_mirror::classify_committed_boundary_rows_after(
            session_id,
            committed,
            canonical_cursor,
            provenance,
            store_commit_authority,
        )
        .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
        let row_count = rows.len();
        for row in rows {
            let sequence = row.canonical_row_sequence();
            let queued = self.enqueue_live_context_row(&binding, row).await?;
            self.shared
                .live_context_queued_rows
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert((session_id.clone(), sequence), queued);
        }
        self.drain_live_context_outbox(session_id).await?;
        Ok(row_count)
    }

    /// Drain exact-next canonical rows while generated state proves a safe
    /// provider boundary. A deferred head remains under sealed local custody
    /// and is retried only when another generated lifecycle trigger calls this
    /// method.
    #[cfg(feature = "live")]
    pub async fn drain_live_context_outbox(
        &self,
        session_id: &SessionId,
    ) -> Result<(), RuntimeDriverError> {
        loop {
            let Some(channel_id) = self.live_active_channel_for_session(session_id).await else {
                return Ok(());
            };
            let state = self.session_dsl_state(session_id).await.map_err(|reason| {
                RuntimeDriverError::ValidationFailed {
                    reason: reason.to_string(),
                }
            })?;
            if !live_context_execution_binding_is_complete(&state, channel_id.as_str()).map_err(
                |reason| RuntimeDriverError::ValidationFailed {
                    reason: reason.to_string(),
                },
            )? {
                return Ok(());
            }
            let binding = self
                .live_delegation_runtime_binding(session_id, &channel_id)
                .await?;
            let cursor = state
                .live_context_cursor_by_channel
                .get(channel_id.as_str())
                .copied()
                .ok_or_else(|| RuntimeDriverError::ValidationFailed {
                    reason: "active live channel has no generated canonical context cursor"
                        .to_string(),
                })?;
            let next_cursor =
                cursor
                    .checked_add(1)
                    .ok_or_else(|| RuntimeDriverError::ValidationFailed {
                        reason: "live context cursor is exhausted".to_string(),
                    })?;
            let key = (session_id.clone(), next_cursor);
            let Some(queued) = self
                .shared
                .live_context_queued_rows
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(&key)
                .cloned()
            else {
                return Ok(());
            };

            if queued.row().provider_context().is_none() {
                self.advance_live_context_canonical_coverage(&queued)
                    .await?;
                self.shared
                    .live_context_queued_rows
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .remove(&key);
                continue;
            }

            if state
                .live_provider_turn_by_channel
                .contains_key(channel_id.as_str())
            {
                return Ok(());
            }
            let Some(host) = self.live_context_mirror_host() else {
                return Ok(());
            };
            let authority = self.authorize_queued_live_context_append(&queued).await?;
            let context = queued
                .row()
                .provider_context()
                .expect("mirrorable row checked above")
                .to_string();
            let (returned_authority, outcome) = host
                .append_context(authority, context)
                .await
                .map_err(RuntimeDriverError::Internal)?;
            let resolution = self
                .resolve_live_context_append(
                    binding.runtime_id(),
                    binding.fence_token(),
                    binding.generation(),
                    &returned_authority,
                    outcome,
                )
                .await?;
            match resolution {
                crate::live_execution::LiveContextAppendResolution::Resolved(receipt)
                    if receipt.outcome()
                        == meerkat_core::LiveAppendDeliveryOutcome::Acknowledged =>
                {
                    self.shared
                        .live_context_queued_rows
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .remove(&key);
                }
                crate::live_execution::LiveContextAppendResolution::Resolved(receipt) => {
                    if receipt.outcome() == meerkat_core::LiveAppendDeliveryOutcome::Rejected {
                        let retry = self
                            .enqueue_live_context_row(&binding, queued.row().clone())
                            .await?;
                        self.shared
                            .live_context_queued_rows
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .insert(key, retry);
                    }
                    return Ok(());
                }
                crate::live_execution::LiveContextAppendResolution::AmbiguityRecovery(recovery) => {
                    host.recover_ambiguous_append(recovery)
                        .await
                        .map_err(RuntimeDriverError::Internal)?;
                    self.shared
                        .live_context_queued_rows
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .remove(&key);
                    return Ok(());
                }
            }
        }
    }

    /// Admit one exact SessionDocument-classified committed row into the
    /// generated per-session live-context outbox.
    #[cfg(feature = "live")]
    pub async fn enqueue_live_context_row(
        &self,
        binding: &crate::live_execution::LiveDelegationRuntimeBinding,
        row: crate::live_context_mirror::CommittedLiveContextRow,
    ) -> Result<crate::live_execution::LiveContextQueuedRow, RuntimeDriverError> {
        if row.session_id() != binding.session_id() {
            return Err(RuntimeDriverError::ValidationFailed {
                reason: "committed live-context row belongs to another session".to_string(),
            });
        }
        let append_id = uuid::Uuid::new_v4().to_string();
        let disposition = match row.disposition() {
            meerkat_core::generated::session_document::LiveContextCommittedRowDisposition::MirrorParentText => {
                crate::meerkat_machine::dsl::LiveContextRowDisposition::MirrorParentText
            }
            meerkat_core::generated::session_document::LiveContextCommittedRowDisposition::AlreadyPresentInLiveChannel => {
                crate::meerkat_machine::dsl::LiveContextRowDisposition::AlreadyPresentInLiveChannel
            }
            meerkat_core::generated::session_document::LiveContextCommittedRowDisposition::ExcludedFromLiveContext => {
                crate::meerkat_machine::dsl::LiveContextRowDisposition::ExcludedFromLiveContext
            }
        };
        let _mutation_guard = self
            .lock_current_durability_ready_session_mutation_gate(binding.session_id())
            .await?;
        let (_, effects) = self
            .apply_session_dsl_input(
                binding.session_id(),
                crate::meerkat_machine::dsl::MeerkatMachineInput::EnqueueLiveContextRow {
                    channel_id: binding.channel_id().to_string(),
                    runtime_id: crate::meerkat_machine::dsl::AgentRuntimeId::from_domain(
                        binding.runtime_id(),
                    ),
                    fence_token: crate::meerkat_machine::dsl::FenceToken::from_domain(
                        binding.fence_token(),
                    ),
                    generation: crate::meerkat_machine::dsl::Generation::from_domain(
                        binding.generation(),
                    ),
                    append_id: append_id.clone(),
                    canonical_cursor: row.canonical_row_sequence(),
                    content_digest: row.content_digest().to_string(),
                    commit_authority_token: row.store_commit_authority().to_string(),
                    disposition,
                },
                "EnqueueLiveContextRow",
            )
            .await
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
        for effect in effects.as_slice() {
            if let Some(queued) =
                crate::live_execution::LiveContextQueuedRow::from_generated_effect(
                    binding,
                    &append_id,
                    row.clone(),
                    effect,
                )
                .map_err(|error| RuntimeDriverError::Internal(error.to_string()))?
            {
                return Ok(queued);
            }
        }
        Err(RuntimeDriverError::Internal(
            "generated context enqueue emitted no matching custody effect".to_string(),
        ))
    }

    /// Advance canonical coverage for an exact queued row that requires no
    /// provider send, including already-present live transcript rows.
    pub async fn advance_live_context_canonical_coverage(
        &self,
        queued: &crate::live_execution::LiveContextQueuedRow,
    ) -> Result<crate::live_execution::LiveContextCanonicalCoverageReceipt, RuntimeDriverError>
    {
        let binding = queued.binding();
        let next_cursor = queued.row().canonical_row_sequence();
        let previous_cursor =
            next_cursor
                .checked_sub(1)
                .ok_or_else(|| RuntimeDriverError::ValidationFailed {
                    reason: "canonical live-context row sequence must be one-based".to_string(),
                })?;
        let disposition = match queued.row().disposition() {
            meerkat_core::generated::session_document::LiveContextCommittedRowDisposition::AlreadyPresentInLiveChannel => {
                crate::meerkat_machine::dsl::LiveContextRowDisposition::AlreadyPresentInLiveChannel
            }
            meerkat_core::generated::session_document::LiveContextCommittedRowDisposition::ExcludedFromLiveContext => {
                crate::meerkat_machine::dsl::LiveContextRowDisposition::ExcludedFromLiveContext
            }
            meerkat_core::generated::session_document::LiveContextCommittedRowDisposition::MirrorParentText => {
                return Err(RuntimeDriverError::ValidationFailed {
                    reason: "mirrorable live-context row requires provider append authorization"
                        .to_string(),
                });
            }
        };
        let _mutation_guard = self
            .lock_current_durability_ready_session_mutation_gate(binding.session_id())
            .await?;
        let (_, effects) = self
            .apply_session_dsl_input(
                binding.session_id(),
                crate::meerkat_machine::dsl::MeerkatMachineInput::AdvanceLiveContextCanonicalCoverage {
                    channel_id: binding.channel_id().to_string(),
                    runtime_id: crate::meerkat_machine::dsl::AgentRuntimeId::from_domain(
                        binding.runtime_id(),
                    ),
                    fence_token: crate::meerkat_machine::dsl::FenceToken::from_domain(
                        binding.fence_token(),
                    ),
                    generation: crate::meerkat_machine::dsl::Generation::from_domain(
                        binding.generation(),
                    ),
                    append_id: queued.append_id().to_string(),
                    previous_cursor,
                    next_cursor,
                    disposition,
                },
                "AdvanceLiveContextCanonicalCoverage",
            )
            .await
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
        for effect in effects.as_slice() {
            if let Some(receipt) =
                crate::live_execution::LiveContextCanonicalCoverageReceipt::from_generated_effect(
                    queued, effect,
                )
                .map_err(|error| RuntimeDriverError::Internal(error.to_string()))?
            {
                return Ok(receipt);
            }
        }
        Err(RuntimeDriverError::Internal(
            "generated canonical coverage advance emitted no matching receipt".to_string(),
        ))
    }

    /// Mint pre-send authority for the exact mirrorable head of the generated
    /// live-context outbox. Generated state rejects active provider turns.
    pub async fn authorize_queued_live_context_append(
        &self,
        queued: &crate::live_execution::LiveContextQueuedRow,
    ) -> Result<crate::live_execution::LiveContextAppendAuthority, RuntimeDriverError> {
        if queued.row().provider_context().is_none() {
            return Err(RuntimeDriverError::ValidationFailed {
                reason: "queued live-context row has no provider context payload".to_string(),
            });
        }
        let binding = queued.binding();
        let next_cursor = queued.row().canonical_row_sequence();
        let previous_cursor =
            next_cursor
                .checked_sub(1)
                .ok_or_else(|| RuntimeDriverError::ValidationFailed {
                    reason: "canonical live-context row sequence must be one-based".to_string(),
                })?;
        let _mutation_guard = self
            .lock_current_durability_ready_session_mutation_gate(binding.session_id())
            .await?;
        let (_, effects) = self
            .apply_session_dsl_input(
                binding.session_id(),
                crate::meerkat_machine::dsl::MeerkatMachineInput::AuthorizeLiveContextAppend {
                    channel_id: binding.channel_id().to_string(),
                    runtime_id: crate::meerkat_machine::dsl::AgentRuntimeId::from_domain(
                        binding.runtime_id(),
                    ),
                    fence_token: crate::meerkat_machine::dsl::FenceToken::from_domain(
                        binding.fence_token(),
                    ),
                    generation: crate::meerkat_machine::dsl::Generation::from_domain(
                        binding.generation(),
                    ),
                    append_id: queued.append_id().to_string(),
                    previous_cursor,
                    next_cursor,
                },
                "AuthorizeLiveContextAppend",
            )
            .await
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
        for effect in effects.as_slice() {
            if let Some(authority) =
                crate::live_execution::LiveContextAppendAuthority::from_generated_effect(
                    binding.session_id(),
                    binding.channel_id(),
                    queued.append_id(),
                    previous_cursor,
                    next_cursor,
                    effect,
                )
                .map_err(|error| RuntimeDriverError::Internal(error.to_string()))?
            {
                return Ok(authority);
            }
        }
        Err(RuntimeDriverError::Internal(
            "generated context append authorization emitted no matching authority effect"
                .to_string(),
        ))
    }

    /// Resolve the delivery of one exact pre-authorized context append.
    pub async fn resolve_live_context_append(
        &self,
        runtime_id: &crate::identifiers::LogicalRuntimeId,
        fence_token: u64,
        generation: u64,
        authority: &crate::live_execution::LiveContextAppendAuthority,
        outcome: meerkat_core::LiveAppendDeliveryOutcome,
    ) -> Result<crate::live_execution::LiveContextAppendResolution, RuntimeDriverError> {
        let session_id = authority.session_id();
        let _mutation_guard = self
            .lock_current_durability_ready_session_mutation_gate(session_id)
            .await?;
        let observation = match outcome {
            meerkat_core::LiveAppendDeliveryOutcome::Acknowledged => {
                crate::meerkat_machine::dsl::LiveContextAppendObservation::Delivered
            }
            meerkat_core::LiveAppendDeliveryOutcome::Rejected => {
                crate::meerkat_machine::dsl::LiveContextAppendObservation::Rejected
            }
            meerkat_core::LiveAppendDeliveryOutcome::Ambiguous => {
                crate::meerkat_machine::dsl::LiveContextAppendObservation::Ambiguous
            }
        };
        let replacement_channel_id =
            matches!(outcome, meerkat_core::LiveAppendDeliveryOutcome::Ambiguous)
                .then(|| meerkat_core::LiveChannelId::new(uuid::Uuid::new_v4().to_string()));
        let (_, effects) = self
            .apply_session_dsl_input(
                session_id,
                crate::meerkat_machine::dsl::MeerkatMachineInput::ResolveLiveContextAppend {
                    channel_id: authority.channel_id().to_string(),
                    runtime_id: crate::meerkat_machine::dsl::AgentRuntimeId::from_domain(
                        runtime_id,
                    ),
                    fence_token: crate::meerkat_machine::dsl::FenceToken::from_domain(fence_token),
                    generation: crate::meerkat_machine::dsl::Generation::from_domain(generation),
                    append_id: authority.append_id().to_string(),
                    previous_cursor: authority.previous_cursor(),
                    next_cursor: authority.next_cursor(),
                    replacement_channel_id: replacement_channel_id
                        .as_ref()
                        .map_or_else(String::new, ToString::to_string),
                    observation,
                },
                "ResolveLiveContextAppend",
            )
            .await
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
        for effect in effects.as_slice() {
            if let Some(receipt) =
                crate::live_execution::LiveContextAppendResolutionReceipt::from_generated_effect(
                    authority, outcome, effect,
                )
                .map_err(|error| RuntimeDriverError::Internal(error.to_string()))?
            {
                return Ok(crate::live_execution::LiveContextAppendResolution::Resolved(receipt));
            }
            if let Some(replacement_channel_id) = replacement_channel_id.as_ref() {
                if let Some(recovery) = crate::live_execution::LiveContextAmbiguityRecoveryAuthority::from_generated_effect(
                    authority,
                    replacement_channel_id,
                    effect,
                )
                .map_err(|error| RuntimeDriverError::Internal(error.to_string()))?
                {
                    return Ok(
                        crate::live_execution::LiveContextAppendResolution::AmbiguityRecovery(
                            recovery,
                        ),
                    );
                }
            }
        }
        Err(RuntimeDriverError::Internal(
            "generated context append resolution emitted no matching authority effect".to_string(),
        ))
    }

    /// Atomically accept the exact replacement WebRTC answer and bind its
    /// execution only after provider SessionReady and canonical seed
    /// acknowledgement prove the recovery cursor.
    #[cfg(feature = "live")]
    pub async fn accept_live_context_recovery_webrtc_answer_and_bind_execution(
        &self,
        provider_binding: &meerkat_live::ProviderWebrtcBinding,
        bound_ready: &meerkat_live::ProviderWebrtcBoundReadyReceipt,
        answer_observation_sequence: u64,
        recovery: &crate::live_execution::LiveContextAmbiguityRecoveryAuthority,
    ) -> Result<LiveWebrtcAnswerExecutionBindingAuthority, RuntimeDriverError> {
        if provider_binding.session_id() != recovery.session_id()
            || provider_binding.channel_id() != recovery.replacement_channel_id()
            || provider_binding.runtime_fence().get() != recovery.fence_token()
            || provider_binding.runtime_generation().get() != recovery.generation()
        {
            return Err(RuntimeDriverError::ValidationFailed {
                reason:
                    "provider recovery binding does not match exact generated recovery authority"
                        .to_string(),
            });
        }
        let _mutation_guard = self
            .lock_current_durability_ready_session_mutation_gate(recovery.session_id())
            .await?;
        let state = self
            .session_dsl_state(recovery.session_id())
            .await
            .map_err(|reason| RuntimeDriverError::ValidationFailed {
                reason: reason.to_string(),
            })?;
        let dsl_runtime_id = state.active_runtime_id.clone().ok_or_else(|| {
            RuntimeDriverError::ValidationFailed {
                reason: "atomic recovery answer has no active generated runtime binding"
                    .to_string(),
            }
        })?;
        if dsl_runtime_id.0.as_str() != recovery.runtime_id().0.as_str()
            || state.active_fence_token
                != Some(crate::meerkat_machine::dsl::FenceToken::from_domain(
                    recovery.fence_token(),
                ))
            || state.active_runtime_generation
                != Some(crate::meerkat_machine::dsl::Generation::from_domain(
                    recovery.generation(),
                ))
        {
            return Err(RuntimeDriverError::ValidationFailed {
                reason:
                    "generated recovery authority does not match the active runtime incarnation"
                        .to_string(),
            });
        }
        let canonical_seed_cursor = bound_ready
            .__consume_for_generated_bind(provider_binding)
            .map_err(|error| RuntimeDriverError::ValidationFailed {
                reason: format!(
                    "provider recovery bound-ready authority rejected: {}",
                    error.reason_code()
                ),
            })?;
        if canonical_seed_cursor != recovery.canonical_seed_cursor() {
            return Err(RuntimeDriverError::ValidationFailed {
                reason: "provider recovery seed acknowledgement does not match canonical recovery cursor"
                    .to_string(),
            });
        }
        let (_, effects) = self
            .apply_session_dsl_input(
                recovery.session_id(),
                crate::meerkat_machine::dsl::MeerkatMachineInput::BindLiveContextRecoveryChannel {
                    session_id: recovery.session_id().to_string(),
                    closing_channel_id: recovery.closing_channel_id().to_string(),
                    replacement_channel_id: recovery.replacement_channel_id().to_string(),
                    answer_observation_sequence,
                    runtime_id: crate::meerkat_machine::dsl::AgentRuntimeId::from_domain(
                        recovery.runtime_id(),
                    ),
                    fence_token: crate::meerkat_machine::dsl::FenceToken::from_domain(
                        recovery.fence_token(),
                    ),
                    generation: crate::meerkat_machine::dsl::Generation::from_domain(
                        recovery.generation(),
                    ),
                    append_id: recovery.append_id().to_string(),
                    canonical_seed_cursor,
                },
                "BindLiveContextRecoveryChannel",
            )
            .await
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
        for effect in effects.as_slice() {
            let crate::meerkat_machine::dsl::MeerkatMachineEffect::LiveContextRecoveryChannelBound {
                session_id,
                closing_channel_id,
                replacement_channel_id,
                append_id,
                canonical_seed_cursor: effect_seed_cursor,
                status,
                answered,
                sequence,
                answer_observation_sequence: effect_answer_sequence,
                runtime_id,
                fence_token,
                generation,
            } = effect else {
                continue;
            };
            if session_id != &recovery.session_id().to_string()
                || closing_channel_id != recovery.closing_channel_id().as_str()
                || replacement_channel_id != recovery.replacement_channel_id().as_str()
                || append_id != recovery.append_id()
                || *effect_seed_cursor != canonical_seed_cursor
                || *effect_answer_sequence != answer_observation_sequence
                || runtime_id.0.as_str() != recovery.runtime_id().0.as_str()
                || fence_token.0 != recovery.fence_token()
                || generation.0 != recovery.generation()
            {
                return Err(RuntimeDriverError::Internal(
                    "atomic recovery WebRTC answer binding effect did not match exact authority input"
                        .to_string(),
                ));
            }
            let answer = LiveWebrtcAnswerResultAuthority {
                status: *status,
                answered: *answered,
                sequence: *sequence,
                answer_observation_sequence: *effect_answer_sequence,
            };
            let binding = crate::live_execution::LiveDelegationRuntimeBinding::new(
                recovery.session_id().clone(),
                recovery.replacement_channel_id().clone(),
                recovery.runtime_id().clone(),
                recovery.fence_token(),
                recovery.generation(),
            );
            self.shared
                .live_context_queued_rows
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .retain(|(queued_session, sequence), _| {
                    queued_session != recovery.session_id() || *sequence > canonical_seed_cursor
                });
            return Ok(LiveWebrtcAnswerExecutionBindingAuthority::new(
                answer, binding,
            ));
        }
        Err(RuntimeDriverError::Internal(
            "atomic recovery WebRTC answer binding emitted no matching generated effect"
                .to_string(),
        ))
    }

    /// Atomic answer-and-bind for a replacement channel authorized by one
    /// ambiguous delegation-result delivery. This is distinct from canonical
    /// context recovery and joins the exact operation plus result digest.
    #[cfg(feature = "live")]
    pub async fn accept_live_delegation_result_recovery_webrtc_answer_and_bind_execution(
        &self,
        provider_binding: &meerkat_live::ProviderWebrtcBinding,
        bound_ready: &meerkat_live::ProviderWebrtcBoundReadyReceipt,
        answer_observation_sequence: u64,
        recovery: &crate::live_execution::LiveDelegationResultAmbiguityRecoveryAuthority,
    ) -> Result<LiveWebrtcAnswerExecutionBindingAuthority, RuntimeDriverError> {
        if provider_binding.session_id() != recovery.session_id()
            || provider_binding.channel_id() != recovery.replacement_channel_id()
            || provider_binding.runtime_fence().get() != recovery.fence_token()
            || provider_binding.runtime_generation().get() != recovery.generation()
        {
            return Err(RuntimeDriverError::ValidationFailed {
                reason: "provider result-recovery binding does not match exact generated authority"
                    .to_string(),
            });
        }
        let _mutation_guard = self
            .lock_current_durability_ready_session_mutation_gate(recovery.session_id())
            .await?;
        let state = self
            .session_dsl_state(recovery.session_id())
            .await
            .map_err(|reason| RuntimeDriverError::ValidationFailed {
                reason: reason.to_string(),
            })?;
        let dsl_runtime_id = state.active_runtime_id.clone().ok_or_else(|| {
            RuntimeDriverError::ValidationFailed {
                reason: "atomic result-recovery answer has no active runtime identity".to_string(),
            }
        })?;
        if dsl_runtime_id.0.as_str() != recovery.runtime_id().0.as_str()
            || state.active_fence_token
                != Some(crate::meerkat_machine::dsl::FenceToken::from_domain(
                    recovery.fence_token(),
                ))
            || state.active_runtime_generation
                != Some(crate::meerkat_machine::dsl::Generation::from_domain(
                    recovery.generation(),
                ))
        {
            return Err(RuntimeDriverError::ValidationFailed {
                reason: "result-recovery authority does not match active runtime incarnation"
                    .to_string(),
            });
        }
        let canonical_seed_cursor = bound_ready
            .__consume_for_generated_bind(provider_binding)
            .map_err(|error| RuntimeDriverError::ValidationFailed {
                reason: format!(
                    "provider result-recovery bound-ready authority rejected: {}",
                    error.reason_code()
                ),
            })?;
        if canonical_seed_cursor != recovery.canonical_seed_cursor() {
            return Err(RuntimeDriverError::ValidationFailed {
                reason:
                    "provider result-recovery seed acknowledgement does not match generated cursor"
                        .to_string(),
            });
        }
        let operation = recovery.delivery().operation();
        let (_, effects) = self
            .apply_session_dsl_input(
                recovery.session_id(),
                crate::meerkat_machine::dsl::MeerkatMachineInput::BindLiveDelegationResultRecoveryChannel {
                    session_id: recovery.session_id().to_string(),
                    closing_channel_id: recovery.closing_channel_id().to_string(),
                    replacement_channel_id: recovery.replacement_channel_id().to_string(),
                    answer_observation_sequence,
                    runtime_id: crate::meerkat_machine::dsl::AgentRuntimeId::from_domain(
                        recovery.runtime_id(),
                    ),
                    fence_token: crate::meerkat_machine::dsl::FenceToken::from_domain(
                        recovery.fence_token(),
                    ),
                    generation: crate::meerkat_machine::dsl::Generation::from_domain(
                        recovery.generation(),
                    ),
                    operation_id: crate::meerkat_machine::dsl::OperationId::from_domain(
                        operation.operation_id(),
                    ),
                    result_digest: recovery.delivery().result_digest().to_string(),
                    canonical_seed_cursor,
                },
                "BindLiveDelegationResultRecoveryChannel",
            )
            .await
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
        for effect in effects.as_slice() {
            let crate::meerkat_machine::dsl::MeerkatMachineEffect::LiveDelegationResultRecoveryChannelBound {
                session_id,
                closing_channel_id,
                replacement_channel_id,
                operation_id,
                result_digest,
                canonical_seed_cursor: effect_seed_cursor,
                status,
                answered,
                sequence,
                answer_observation_sequence: effect_answer_sequence,
                runtime_id,
                fence_token,
                generation,
            } = effect else {
                continue;
            };
            if session_id != &recovery.session_id().to_string()
                || closing_channel_id != recovery.closing_channel_id().as_str()
                || replacement_channel_id != recovery.replacement_channel_id().as_str()
                || operation_id
                    != &crate::meerkat_machine::dsl::OperationId::from_domain(
                        operation.operation_id(),
                    )
                || result_digest != recovery.delivery().result_digest()
                || *effect_seed_cursor != canonical_seed_cursor
                || *effect_answer_sequence != answer_observation_sequence
                || runtime_id.0.as_str() != recovery.runtime_id().0.as_str()
                || fence_token.0 != recovery.fence_token()
                || generation.0 != recovery.generation()
            {
                return Err(RuntimeDriverError::Internal(
                    "atomic result-recovery answer binding effect mismatched exact authority"
                        .to_string(),
                ));
            }
            let answer = LiveWebrtcAnswerResultAuthority {
                status: *status,
                answered: *answered,
                sequence: *sequence,
                answer_observation_sequence: *effect_answer_sequence,
            };
            let binding = crate::live_execution::LiveDelegationRuntimeBinding::new(
                recovery.session_id().clone(),
                recovery.replacement_channel_id().clone(),
                recovery.runtime_id().clone(),
                recovery.fence_token(),
                recovery.generation(),
            );
            self.shared
                .live_context_queued_rows
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .retain(|(queued_session, sequence), _| {
                    queued_session != recovery.session_id() || *sequence > canonical_seed_cursor
                });
            return Ok(LiveWebrtcAnswerExecutionBindingAuthority::new(
                answer, binding,
            ));
        }
        Err(RuntimeDriverError::Internal(
            "atomic result-recovery answer binding emitted no exact generated effect".to_string(),
        ))
    }

    /// Mint pre-send authority for the exact confirmed delegation result.
    pub async fn authorize_live_delegation_result_release(
        &self,
        session_id: &SessionId,
        runtime_id: &crate::identifiers::LogicalRuntimeId,
        fence_token: u64,
        generation: u64,
        operation: &meerkat_core::exact_operation::ExactOperationIdentity<
            meerkat_core::LiveUserTurnCorrelation,
        >,
        reconciliation: &crate::live_execution::LiveHandoffReconciliationReceipt,
    ) -> Result<crate::live_execution::LiveDelegationResultReleaseAuthority, RuntimeDriverError>
    {
        if reconciliation.admission().session_id() != session_id
            || reconciliation.admission().operation() != operation
            || reconciliation.disposition() != meerkat_core::LiveHandoffReconciliation::Confirmed
        {
            return Err(RuntimeDriverError::ValidationFailed {
                reason: "live result release does not match the exact confirmed session operation"
                    .to_string(),
            });
        }
        let correlation = operation.domain_correlation();
        let _mutation_guard = self
            .lock_current_durability_ready_session_mutation_gate(session_id)
            .await?;
        let (_, effects) = self
            .apply_session_dsl_input(
                session_id,
                crate::meerkat_machine::dsl::MeerkatMachineInput::AuthorizeLiveDelegationResultRelease {
                    channel_id: correlation.channel_id().to_string(),
                    runtime_id: crate::meerkat_machine::dsl::AgentRuntimeId::from_domain(runtime_id),
                    fence_token: crate::meerkat_machine::dsl::FenceToken::from_domain(fence_token),
                    generation: crate::meerkat_machine::dsl::Generation::from_domain(generation),
                    interaction_id: correlation.interaction_id().to_string(),
                    operation_id: crate::meerkat_machine::dsl::OperationId::from_domain(
                        operation.operation_id(),
                    ),
                    provider_turn_correlation: correlation.provider().user_turn_id().to_string(),
                },
                "AuthorizeLiveDelegationResultRelease",
            )
            .await
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
        for effect in effects.as_slice() {
            let expected_disposition = match effect {
                crate::meerkat_machine::dsl::MeerkatMachineEffect::LiveDelegationResultReleaseAuthorized {
                    disposition: crate::meerkat_machine::dsl::LiveDelegationResultDisposition::OpenTurn,
                    ..
                } => meerkat_core::LiveResultDisposition::OpenTurn,
                crate::meerkat_machine::dsl::MeerkatMachineEffect::LiveDelegationResultReleaseAuthorized {
                    disposition: crate::meerkat_machine::dsl::LiveDelegationResultDisposition::DeferredContext,
                    ..
                } => meerkat_core::LiveResultDisposition::DeferredContext,
                _ => continue,
            };
            if let Some(authority) =
                crate::live_execution::LiveDelegationResultReleaseAuthority::from_generated_effect(
                    session_id,
                    operation,
                    reconciliation,
                    expected_disposition,
                    effect,
                )
                .map_err(|error| RuntimeDriverError::Internal(error.to_string()))?
            {
                return Ok(authority);
            }
        }
        Err(RuntimeDriverError::Internal(
            "generated result release authorization emitted no matching authority effect"
                .to_string(),
        ))
    }

    /// Mint the distinct one-use provider-context delivery authority for an
    /// exact released worker result. The result text is digest-bound here and
    /// never enters the canonical SessionDocument cursor lifecycle.
    pub async fn authorize_live_delegation_result_delivery(
        &self,
        release: &crate::live_execution::LiveDelegationResultReleaseAuthority,
        result_text: &str,
    ) -> Result<crate::live_execution::LiveDelegationResultDeliveryAuthority, RuntimeDriverError>
    {
        if result_text.trim().is_empty() {
            return Err(RuntimeDriverError::ValidationFailed {
                reason: "live delegation result delivery text must not be empty".to_string(),
            });
        }
        let session_id = release.session_id();
        let operation = release.operation();
        let correlation = operation.domain_correlation();
        let channel = correlation.channel_id().to_string();
        let result_digest = crate::live_execution::live_delegation_result_digest(result_text);
        let _mutation_guard = self
            .lock_current_durability_ready_session_mutation_gate(session_id)
            .await?;
        let state = self.session_dsl_state(session_id).await.map_err(|reason| {
            RuntimeDriverError::ValidationFailed {
                reason: reason.to_string(),
            }
        })?;
        let runtime_id = state
            .live_execution_runtime_id_by_channel
            .get(&channel)
            .cloned()
            .ok_or_else(|| RuntimeDriverError::ValidationFailed {
                reason: "live result delivery has no generated runtime binding".to_string(),
            })?;
        let fence_token = state
            .live_execution_fence_by_channel
            .get(&channel)
            .copied()
            .ok_or_else(|| RuntimeDriverError::ValidationFailed {
                reason: "live result delivery has no generated fence binding".to_string(),
            })?;
        let generation = state
            .live_execution_generation_by_channel
            .get(&channel)
            .copied()
            .ok_or_else(|| RuntimeDriverError::ValidationFailed {
                reason: "live result delivery has no generated generation binding".to_string(),
            })?;
        let disposition = match release.disposition() {
            meerkat_core::LiveResultDisposition::OpenTurn => {
                crate::meerkat_machine::dsl::LiveDelegationResultDisposition::OpenTurn
            }
            meerkat_core::LiveResultDisposition::DeferredContext => {
                crate::meerkat_machine::dsl::LiveDelegationResultDisposition::DeferredContext
            }
        };
        let (_, effects) = self
            .apply_session_dsl_input(
                session_id,
                crate::meerkat_machine::dsl::MeerkatMachineInput::AuthorizeLiveDelegationResultDelivery {
                    channel_id: channel,
                    runtime_id,
                    fence_token,
                    generation,
                    interaction_id: correlation.interaction_id().to_string(),
                    operation_id: crate::meerkat_machine::dsl::OperationId::from_domain(
                        operation.operation_id(),
                    ),
                    provider_turn_correlation: correlation.provider().user_turn_id().to_string(),
                    result_digest: result_digest.clone(),
                    disposition,
                },
                "AuthorizeLiveDelegationResultDelivery",
            )
            .await
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
        for effect in effects.as_slice() {
            if let Some(authority) =
                crate::live_execution::LiveDelegationResultDeliveryAuthority::from_generated_effect(
                    release,
                    &result_digest,
                    effect,
                )
                .map_err(|error| RuntimeDriverError::Internal(error.to_string()))?
            {
                return Ok(authority);
            }
        }
        Err(RuntimeDriverError::Internal(
            "generated result delivery authorization emitted no matching authority effect"
                .to_string(),
        ))
    }

    /// Resolve the exact provider observation for one result-context send.
    /// Every observation is terminal; ambiguous delivery additionally carries
    /// generated recovery debt and can never be blindly replayed.
    pub async fn resolve_live_delegation_result_delivery(
        &self,
        authority: &crate::live_execution::LiveDelegationResultDeliveryAuthority,
        observation: crate::live_execution::LiveDelegationResultDeliveryObservation,
    ) -> Result<crate::live_execution::LiveDelegationResultDeliveryResolution, RuntimeDriverError>
    {
        let session_id = authority.session_id();
        let operation = authority.operation();
        let channel = operation.domain_correlation().channel_id().to_string();
        let _mutation_guard = self
            .lock_current_durability_ready_session_mutation_gate(session_id)
            .await?;
        let state = self.session_dsl_state(session_id).await.map_err(|reason| {
            RuntimeDriverError::ValidationFailed {
                reason: reason.to_string(),
            }
        })?;
        let runtime_id = state
            .live_execution_runtime_id_by_channel
            .get(&channel)
            .cloned()
            .ok_or_else(|| RuntimeDriverError::ValidationFailed {
                reason: "live result resolution has no generated runtime binding".to_string(),
            })?;
        let fence_token = state
            .live_execution_fence_by_channel
            .get(&channel)
            .copied()
            .ok_or_else(|| RuntimeDriverError::ValidationFailed {
                reason: "live result resolution has no generated fence binding".to_string(),
            })?;
        let generation = state
            .live_execution_generation_by_channel
            .get(&channel)
            .copied()
            .ok_or_else(|| RuntimeDriverError::ValidationFailed {
                reason: "live result resolution has no generated generation binding".to_string(),
            })?;
        let dsl_observation = match observation {
            crate::live_execution::LiveDelegationResultDeliveryObservation::Delivered => {
                crate::meerkat_machine::dsl::LiveDelegationResultDeliveryObservation::Delivered
            }
            crate::live_execution::LiveDelegationResultDeliveryObservation::Rejected => {
                crate::meerkat_machine::dsl::LiveDelegationResultDeliveryObservation::Rejected
            }
            crate::live_execution::LiveDelegationResultDeliveryObservation::Ambiguous => {
                crate::meerkat_machine::dsl::LiveDelegationResultDeliveryObservation::Ambiguous
            }
        };
        let replacement_channel_id = matches!(
            observation,
            crate::live_execution::LiveDelegationResultDeliveryObservation::Ambiguous
        )
        .then(|| meerkat_core::LiveChannelId::new(uuid::Uuid::new_v4().to_string()));
        let (_, effects) = self
            .apply_session_dsl_input(
                session_id,
                crate::meerkat_machine::dsl::MeerkatMachineInput::ResolveLiveDelegationResultDelivery {
                    channel_id: channel,
                    runtime_id,
                    fence_token,
                    generation,
                    operation_id: crate::meerkat_machine::dsl::OperationId::from_domain(
                        operation.operation_id(),
                    ),
                    result_digest: authority.result_digest().to_string(),
                    replacement_channel_id: replacement_channel_id
                        .as_ref()
                        .map_or_else(String::new, ToString::to_string),
                    observation: dsl_observation,
                },
                "ResolveLiveDelegationResultDelivery",
            )
            .await
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
        for effect in effects.as_slice() {
            if let Some(receipt) =
                crate::live_execution::LiveDelegationResultDeliveryReceipt::from_generated_effect(
                    authority,
                    observation,
                    effect,
                )
                .map_err(|error| RuntimeDriverError::Internal(error.to_string()))?
            {
                return Ok(
                    crate::live_execution::LiveDelegationResultDeliveryResolution::Resolved(
                        receipt,
                    ),
                );
            }
            if let Some(replacement_channel_id) = replacement_channel_id.as_ref() {
                if let Some(recovery) = crate::live_execution::LiveDelegationResultAmbiguityRecoveryAuthority::from_generated_effect(
                    authority,
                    replacement_channel_id,
                    effect,
                )
                .map_err(|error| RuntimeDriverError::Internal(error.to_string()))?
                {
                    return Ok(
                        crate::live_execution::LiveDelegationResultDeliveryResolution::AmbiguityRecovery(
                            recovery,
                        ),
                    );
                }
            }
        }
        Err(RuntimeDriverError::Internal(
            "generated result delivery resolution emitted no matching receipt effect".to_string(),
        ))
    }

    /// Hand committed ambiguity recovery authority to the installed recovery
    /// host. A host failure leaves typed recovery pending; callers must not
    /// retry result resolution or replay the provider context append.
    #[cfg(feature = "live")]
    pub async fn realize_live_delegation_result_ambiguity_recovery(
        &self,
        authority: crate::live_execution::LiveDelegationResultAmbiguityRecoveryAuthority,
    ) -> Result<(), RuntimeDriverError> {
        let host = self.live_context_mirror_host().ok_or_else(|| {
            RuntimeDriverError::ValidationFailed {
                reason: "live result ambiguity recovery host is not installed".to_string(),
            }
        })?;
        host.recover_ambiguous_delegation_result(authority)
            .await
            .map_err(RuntimeDriverError::Internal)
    }

    #[cfg(feature = "live")]
    pub async fn resolve_live_open_admission(
        &self,
        session_id: &SessionId,
        channel_id: &meerkat_live::LiveChannelId,
        llm_identity: &meerkat_core::SessionLlmIdentity,
    ) -> Result<LiveOpenAdmissionAuthority, RuntimeDriverError> {
        let _mutation_guard = self
            .lock_current_durability_ready_session_mutation_gate(session_id)
            .await?;
        let channel_id_string = channel_id.to_string();
        let (_, effects) = self
            .apply_session_dsl_input(
                session_id,
                crate::meerkat_machine::dsl::MeerkatMachineInput::ResolveLiveOpenAdmission {
                    session_id: session_id.to_string(),
                    channel_id: channel_id_string.clone(),
                    llm_identity: crate::meerkat_machine::dsl::SessionLlmIdentity::from_domain(
                        llm_identity,
                    ),
                },
                "ResolveLiveOpenAdmission",
            )
            .await
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;

        let authority = effects.as_slice().iter().find_map(|effect| match effect {
            crate::meerkat_machine::dsl::MeerkatMachineEffect::LiveOpenAdmissionResolved {
                session_id: effect_session_id,
                channel_id: effect_channel_id,
                bound_llm_identity,
                admitted,
                rejection,
                sequence,
            } if *effect_session_id == session_id.to_string()
                && *effect_channel_id == channel_id_string =>
            {
                Some(LiveOpenAdmissionAuthority::from_generated_effect(
                    session_id.clone(),
                    channel_id.clone(),
                    *admitted,
                    *rejection,
                    bound_llm_identity.clone(),
                    *sequence,
                ))
            }
            _ => None,
        });
        match authority {
            Some(authority) => authority.map_err(RuntimeDriverError::Internal),
            None => Err(RuntimeDriverError::Internal(format!(
                "ResolveLiveOpenAdmission for channel '{channel_id_string}' emitted no LiveOpenAdmissionResolved effect"
            ))),
        }
    }

    #[cfg(feature = "live")]
    pub async fn live_channel_bound_llm_identity(
        &self,
        session_id: &SessionId,
        channel_id: &meerkat_live::LiveChannelId,
    ) -> Result<Option<meerkat_core::SessionLlmIdentity>, RuntimeDriverError> {
        let state = self.session_dsl_state(session_id).await.map_err(|reason| {
            RuntimeDriverError::ValidationFailed {
                reason: reason.to_string(),
            }
        })?;
        state
            .live_channel_identity_by_channel
            .get(&channel_id.to_string())
            .cloned()
            .map(meerkat_core::SessionLlmIdentity::try_from)
            .transpose()
            .map_err(RuntimeDriverError::Internal)
    }

    #[cfg(feature = "live")]
    pub async fn abandon_live_open_admission(
        &self,
        session_id: &SessionId,
        channel_id: &meerkat_live::LiveChannelId,
    ) -> Result<(), RuntimeDriverError> {
        let _mutation_guard = self
            .lock_current_durability_ready_session_mutation_gate(session_id)
            .await?;
        self.apply_session_dsl_input(
            session_id,
            crate::meerkat_machine::dsl::MeerkatMachineInput::AbandonLiveOpenAdmission {
                session_id: session_id.to_string(),
                channel_id: channel_id.to_string(),
            },
            "AbandonLiveOpenAdmission",
        )
        .await
        .map(|_| ())
        .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })
    }

    #[cfg(feature = "live")]
    pub async fn live_channel_is_active_for_session(
        &self,
        session_id: &SessionId,
        channel_id: &meerkat_live::LiveChannelId,
    ) -> bool {
        self.session_dsl_state(session_id)
            .await
            .ok()
            .and_then(|state| {
                state
                    .live_active_channel_by_session
                    .get(&session_id.to_string())
                    .cloned()
            })
            .is_some_and(|active| active == channel_id.to_string())
    }

    #[cfg(feature = "live")]
    pub async fn live_session_for_active_channel(
        &self,
        channel_id: &meerkat_live::LiveChannelId,
    ) -> Option<SessionId> {
        let channel_id = channel_id.to_string();
        let session_ids = {
            let sessions = self.sessions.read().await;
            sessions.keys().cloned().collect::<Vec<_>>()
        };

        for session_id in session_ids {
            let Ok(state) = self.session_dsl_state(&session_id).await else {
                continue;
            };
            if state
                .live_channel_session_by_channel
                .get(&channel_id)
                .is_some_and(|owner| owner == &session_id.to_string())
            {
                return Some(session_id);
            }
        }
        None
    }

    /// Read-only routing projection over generated live channel status
    /// authority. Active channels route through the active binding; closed
    /// retained channels route through the machine-owned close result map.
    #[cfg(feature = "live")]
    pub async fn live_session_for_status_channel(
        &self,
        channel_id: &meerkat_live::LiveChannelId,
    ) -> Option<SessionId> {
        let channel_id = channel_id.to_string();
        let session_ids = {
            let sessions = self.sessions.read().await;
            sessions.keys().cloned().collect::<Vec<_>>()
        };

        for session_id in session_ids {
            let Ok(state) = self.session_dsl_state(&session_id).await else {
                continue;
            };
            if state
                .live_channel_session_by_channel
                .get(&channel_id)
                .is_some_and(|owner| owner == &session_id.to_string())
                || state.live_close_status_by_channel.contains_key(&channel_id)
            {
                return Some(session_id);
            }
        }
        None
    }

    /// Read-only routing projection over generated WebRTC token-owner state.
    /// Admission still occurs only when the selected machine resolves the
    /// typed admission input.
    #[cfg(feature = "live")]
    pub async fn live_session_for_webrtc_token(&self, token: &str) -> Option<SessionId> {
        let session_ids = {
            let sessions = self.sessions.read().await;
            sessions.keys().cloned().collect::<Vec<_>>()
        };

        for session_id in session_ids {
            let Ok(state) = self.session_dsl_state(&session_id).await else {
                continue;
            };
            if state.live_webrtc_token_channel_by_token.contains_key(token) {
                return Some(session_id);
            }
        }
        None
    }

    /// Read-only routing projection over generated WebSocket token-owner
    /// state. The token lookup selects which machine receives the admission
    /// input; it does not decide token validity or public result class.
    #[cfg(feature = "live")]
    pub async fn live_session_for_websocket_token(&self, token: &str) -> Option<SessionId> {
        let session_ids = {
            let sessions = self.sessions.read().await;
            sessions.keys().cloned().collect::<Vec<_>>()
        };

        for session_id in session_ids {
            let Ok(state) = self.session_dsl_state(&session_id).await else {
                continue;
            };
            if state
                .live_websocket_token_channel_by_token
                .contains_key(token)
            {
                return Some(session_id);
            }
        }
        None
    }

    #[cfg(feature = "live")]
    pub async fn live_active_channel_for_session(
        &self,
        session_id: &SessionId,
    ) -> Option<meerkat_live::LiveChannelId> {
        self.session_dsl_state(session_id)
            .await
            .ok()
            .and_then(|state| {
                state
                    .live_active_channel_by_session
                    .get(&session_id.to_string())
                    .cloned()
            })
            .map(meerkat_live::LiveChannelId::new)
    }

    /// Read-only projection of the exact runtime generation/fence used to
    /// bind remote WebRTC transport custody. Absence is preserved so a remote
    /// answer strategy can fail closed; local WebRTC does not require these
    /// member-incarnation facts.
    #[cfg(feature = "live")]
    pub async fn live_webrtc_runtime_binding(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<meerkat_live::LiveWebrtcRuntimeBinding>, RuntimeDriverError> {
        let state = self.session_dsl_state(session_id).await.map_err(|reason| {
            RuntimeDriverError::ValidationFailed {
                reason: reason.to_string(),
            }
        })?;
        Ok(state
            .active_runtime_generation
            .zip(state.active_fence_token)
            .map(
                |(generation, fence)| meerkat_live::LiveWebrtcRuntimeBinding {
                    generation: generation.0,
                    fence: fence.0,
                },
            ))
    }

    #[cfg(feature = "live")]
    pub async fn resolve_live_refresh_queued_result(
        &self,
        session_id: &SessionId,
        acceptance: &meerkat_live::LiveRefreshQueueAcceptance,
    ) -> Result<LiveRefreshResultAuthority, RuntimeDriverError> {
        let _mutation_guard = self
            .lock_current_durability_ready_session_mutation_gate(session_id)
            .await?;
        let channel_id = acceptance.channel_id().to_string();
        let (_, effects) = self
            .apply_session_dsl_input(
                session_id,
                crate::meerkat_machine::dsl::MeerkatMachineInput::RecordLiveRefreshQueued {
                    channel_id: channel_id.clone(),
                    queue_acceptance_sequence: acceptance.acceptance_sequence(),
                },
                "RecordLiveRefreshQueued",
            )
            .await
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;

        effects
            .as_slice()
            .iter()
            .find_map(|effect| match effect {
                crate::meerkat_machine::dsl::MeerkatMachineEffect::LiveRefreshResultResolved {
                    channel_id: effect_channel_id,
                    status,
                    sequence,
                    queue_acceptance_sequence,
                } if *effect_channel_id == channel_id => Some(LiveRefreshResultAuthority {
                    status: *status,
                    sequence: *sequence,
                    queue_acceptance_sequence: *queue_acceptance_sequence,
                }),
                _ => None,
            })
            .ok_or_else(|| {
                RuntimeDriverError::Internal(format!(
                    "RecordLiveRefreshQueued for channel '{channel_id}' emitted no LiveRefreshResultResolved effect"
                ))
            })
    }

    #[cfg(feature = "live")]
    pub async fn resolve_live_close_result(
        &self,
        session_id: &SessionId,
        observation: &meerkat_live::LiveChannelCloseObservation,
    ) -> Result<LiveCloseResultAuthority, RuntimeDriverError> {
        let _mutation_guard = self
            .lock_current_durability_ready_session_mutation_gate(session_id)
            .await?;
        let channel_id = observation.channel_id().to_string();
        let (_, effects) = self
            .apply_session_dsl_input(
                session_id,
                crate::meerkat_machine::dsl::MeerkatMachineInput::RecordLiveCloseClosed {
                    session_id: session_id.to_string(),
                    channel_id: channel_id.clone(),
                    close_observation_sequence: observation.close_sequence(),
                },
                "RecordLiveCloseClosed",
            )
            .await
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;

        let authority = effects.as_slice().iter().find_map(|effect| match effect {
            crate::meerkat_machine::dsl::MeerkatMachineEffect::LiveCloseResultResolved {
                channel_id: effect_channel_id,
                status,
                sequence,
                close_observation_sequence,
            } if *effect_channel_id == channel_id
                && *close_observation_sequence == observation.close_sequence() =>
            {
                Some(LiveCloseResultAuthority::from_generated_effect(
                    channel_id.clone(),
                    *status,
                    *sequence,
                    *close_observation_sequence,
                ))
            }
            _ => None,
        });
        match authority {
            Some(authority) => authority.map_err(RuntimeDriverError::Internal),
            None => Err(RuntimeDriverError::Internal(format!(
                "RecordLiveCloseClosed for channel '{channel_id}' emitted no LiveCloseResultResolved effect"
            ))),
        }
    }

    /// Consume the exact one-use rollback capability from an atomic accepted
    /// WebRTC answer after physical close/reject has produced its observation.
    #[cfg(feature = "live")]
    pub async fn rollback_live_webrtc_answer_execution_binding(
        &self,
        authority: LiveWebrtcAnswerExecutionRollbackAuthority,
        observation: &meerkat_live::LiveChannelCloseObservation,
    ) -> Result<LiveCloseResultAuthority, RuntimeDriverError> {
        let session_id = authority.binding().session_id().clone();
        let observed_channel = meerkat_live::LiveChannelId::new(observation.channel_id());
        if !authority.authorizes(&session_id, &observed_channel) {
            return Err(RuntimeDriverError::ValidationFailed {
                reason: "WebRTC answer rollback observation does not match exact bound channel"
                    .to_string(),
            });
        }
        self.resolve_live_close_result(&session_id, observation)
            .await
    }

    #[cfg(feature = "live")]
    pub async fn resolve_live_command_result(
        &self,
        session_id: &SessionId,
        acceptance: &meerkat_live::LiveCommandQueueAcceptance,
    ) -> Result<LiveCommandResultAuthority, RuntimeDriverError> {
        let _mutation_guard = self
            .lock_current_durability_ready_session_mutation_gate(session_id)
            .await?;
        let channel_id = acceptance.channel_id().to_string();
        let command = dsl_live_command_kind(acceptance.kind());
        let (_, effects) = self
            .apply_session_dsl_input(
                session_id,
                crate::meerkat_machine::dsl::MeerkatMachineInput::RecordLiveCommandAccepted {
                    channel_id: channel_id.clone(),
                    command,
                    command_acceptance_sequence: acceptance.acceptance_sequence(),
                },
                "RecordLiveCommandAccepted",
            )
            .await
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;

        effects
            .as_slice()
            .iter()
            .find_map(|effect| match effect {
                crate::meerkat_machine::dsl::MeerkatMachineEffect::LiveCommandResultResolved {
                    channel_id: effect_channel_id,
                    command: effect_command,
                    sequence,
                    command_acceptance_sequence,
                } if *effect_channel_id == channel_id
                    && *effect_command == command
                    && *command_acceptance_sequence == acceptance.acceptance_sequence() =>
                {
                    Some(LiveCommandResultAuthority {
                        command: *effect_command,
                        sequence: *sequence,
                        command_acceptance_sequence: *command_acceptance_sequence,
                    })
                }
                _ => None,
            })
            .ok_or_else(|| {
                RuntimeDriverError::Internal(format!(
                    "RecordLiveCommandAccepted for channel '{channel_id}' emitted no LiveCommandResultResolved effect"
                ))
            })
    }

    #[cfg(feature = "live")]
    pub async fn resolve_live_command_rejection_result(
        &self,
        session_id: &SessionId,
        channel_id: &meerkat_live::LiveChannelId,
        command: crate::meerkat_machine::dsl::LiveCommandPublicKind,
        error: &meerkat_live::LiveAdapterHostError,
    ) -> Result<LiveCommandRejectionAuthority, RuntimeDriverError> {
        let _mutation_guard = self
            .lock_current_durability_ready_session_mutation_gate(session_id)
            .await?;
        let channel_id = channel_id.to_string();
        let rejection = dsl_live_command_rejection_reason(error);
        let (_, effects) = self
            .apply_session_dsl_input(
                session_id,
                crate::meerkat_machine::dsl::MeerkatMachineInput::RecordLiveCommandRejected {
                    channel_id: channel_id.clone(),
                    command,
                    rejection,
                },
                "RecordLiveCommandRejected",
            )
            .await
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;

        effects
            .as_slice()
            .iter()
            .find_map(|effect| match effect {
                crate::meerkat_machine::dsl::MeerkatMachineEffect::LiveCommandRejectionResolved {
                    channel_id: effect_channel_id,
                    command: effect_command,
                    rejection: effect_rejection,
                    public_error_class,
                    sequence,
                } if *effect_channel_id == channel_id
                    && *effect_command == command
                    && *effect_rejection == rejection =>
                {
                    Some(LiveCommandRejectionAuthority {
                        command: *effect_command,
                        rejection: *effect_rejection,
                        public_error_class: *public_error_class,
                        sequence: *sequence,
                    })
                }
                _ => None,
            })
            .ok_or_else(|| {
                RuntimeDriverError::Internal(format!(
                    "RecordLiveCommandRejected for channel '{channel_id}' emitted no LiveCommandRejectionResolved effect"
                ))
            })
    }

    #[cfg(feature = "live")]
    pub async fn resolve_unbound_live_command_rejection_result(
        &self,
        channel_id: &meerkat_live::LiveChannelId,
        command: crate::meerkat_machine::dsl::LiveCommandPublicKind,
    ) -> Result<LiveCommandRejectionAuthority, RuntimeDriverError> {
        let channel_id = channel_id.to_string();
        let rejection = crate::meerkat_machine::dsl::LiveCommandRejectionReason::ChannelNotFound;
        let effects = apply_dsl_transition_on_authority(
            &self.live_unbound_rejection_authority,
            crate::meerkat_machine::dsl::MeerkatMachineInput::RecordLiveCommandRejected {
                channel_id: channel_id.clone(),
                command,
                rejection,
            },
            "RecordLiveCommandRejected:UnboundChannel",
        )
        .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;

        effects
            .as_slice()
            .iter()
            .find_map(|effect| match effect {
                crate::meerkat_machine::dsl::MeerkatMachineEffect::LiveCommandRejectionResolved {
                    channel_id: effect_channel_id,
                    command: effect_command,
                    rejection: effect_rejection,
                    public_error_class,
                    sequence,
                } if *effect_channel_id == channel_id
                    && *effect_command == command
                    && *effect_rejection == rejection =>
                {
                    Some(LiveCommandRejectionAuthority {
                        command: *effect_command,
                        rejection: *effect_rejection,
                        public_error_class: *public_error_class,
                        sequence: *sequence,
                    })
                }
                _ => None,
            })
            .ok_or_else(|| {
                RuntimeDriverError::Internal(format!(
                    "RecordLiveCommandRejected for unbound channel '{channel_id}' emitted no LiveCommandRejectionResolved effect"
                ))
            })
    }

    #[cfg(feature = "live")]
    pub async fn resolve_live_channel_request_rejection_result(
        &self,
        session_id: &SessionId,
        channel_id: &meerkat_live::LiveChannelId,
        request: crate::meerkat_machine::dsl::LiveChannelRequestPublicKind,
        error: &meerkat_live::LiveAdapterHostError,
    ) -> Result<LiveChannelRequestRejectionAuthority, RuntimeDriverError> {
        self.resolve_live_channel_request_rejection_reason_result(
            session_id,
            channel_id,
            request,
            dsl_live_channel_request_rejection_reason(error),
        )
        .await
    }

    #[cfg(feature = "live")]
    pub async fn resolve_live_channel_request_rejection_reason_result(
        &self,
        session_id: &SessionId,
        channel_id: &meerkat_live::LiveChannelId,
        request: crate::meerkat_machine::dsl::LiveChannelRequestPublicKind,
        rejection: crate::meerkat_machine::dsl::LiveChannelRequestRejectionReason,
    ) -> Result<LiveChannelRequestRejectionAuthority, RuntimeDriverError> {
        let _mutation_guard = self
            .lock_current_durability_ready_session_mutation_gate(session_id)
            .await?;
        let channel_id = channel_id.to_string();
        let (_, effects) = self
            .apply_session_dsl_input(
                session_id,
                crate::meerkat_machine::dsl::MeerkatMachineInput::RecordLiveChannelRequestRejected {
                    channel_id: channel_id.clone(),
                    request,
                    rejection,
                },
                "RecordLiveChannelRequestRejected",
            )
            .await
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;

        effects
            .as_slice()
            .iter()
            .find_map(|effect| match effect {
                crate::meerkat_machine::dsl::MeerkatMachineEffect::LiveChannelRequestRejectionResolved {
                    channel_id: effect_channel_id,
                    request: effect_request,
                    rejection: effect_rejection,
                    public_error_class,
                    sequence,
                } if *effect_channel_id == channel_id
                    && *effect_request == request
                    && *effect_rejection == rejection =>
                {
                    Some(LiveChannelRequestRejectionAuthority {
                        request: *effect_request,
                        rejection: *effect_rejection,
                        public_error_class: *public_error_class,
                        sequence: *sequence,
                    })
                }
                _ => None,
            })
            .ok_or_else(|| {
                RuntimeDriverError::Internal(format!(
                    "RecordLiveChannelRequestRejected for channel '{channel_id}' emitted no LiveChannelRequestRejectionResolved effect"
                ))
            })
    }

    #[cfg(feature = "live")]
    pub async fn resolve_unbound_live_channel_request_rejection_result(
        &self,
        channel_id: &meerkat_live::LiveChannelId,
        request: crate::meerkat_machine::dsl::LiveChannelRequestPublicKind,
    ) -> Result<LiveChannelRequestRejectionAuthority, RuntimeDriverError> {
        let channel_id = channel_id.to_string();
        let rejection =
            crate::meerkat_machine::dsl::LiveChannelRequestRejectionReason::ChannelNotFound;
        let effects = apply_dsl_transition_on_authority(
            &self.live_unbound_rejection_authority,
            crate::meerkat_machine::dsl::MeerkatMachineInput::RecordLiveChannelRequestRejected {
                channel_id: channel_id.clone(),
                request,
                rejection,
            },
            "RecordLiveChannelRequestRejected:UnboundChannel",
        )
        .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;

        effects
            .as_slice()
            .iter()
            .find_map(|effect| match effect {
                crate::meerkat_machine::dsl::MeerkatMachineEffect::LiveChannelRequestRejectionResolved {
                    channel_id: effect_channel_id,
                    request: effect_request,
                    rejection: effect_rejection,
                    public_error_class,
                    sequence,
                } if *effect_channel_id == channel_id
                    && *effect_request == request
                    && *effect_rejection == rejection =>
                {
                    Some(LiveChannelRequestRejectionAuthority {
                        request: *effect_request,
                        rejection: *effect_rejection,
                        public_error_class: *public_error_class,
                        sequence: *sequence,
                    })
                }
                _ => None,
            })
            .ok_or_else(|| {
                RuntimeDriverError::Internal(format!(
                    "RecordLiveChannelRequestRejected for unbound channel '{channel_id}' emitted no LiveChannelRequestRejectionResolved effect"
                ))
            })
    }

    #[cfg(feature = "live")]
    pub async fn record_live_webrtc_token_issued(
        &self,
        session_id: &SessionId,
        channel_id: &meerkat_live::LiveChannelId,
        token: &str,
        issued_at_ms: u64,
        ttl_ms: u64,
    ) -> Result<LiveWebrtcTokenAuthority, RuntimeDriverError> {
        let _mutation_guard = self
            .lock_current_durability_ready_session_mutation_gate(session_id)
            .await?;
        let channel_id = channel_id.to_string();
        let token = token.to_string();
        let (_, effects) = self
            .apply_session_dsl_input(
                session_id,
                crate::meerkat_machine::dsl::MeerkatMachineInput::RecordLiveWebrtcTokenIssued {
                    session_id: session_id.to_string(),
                    channel_id: channel_id.clone(),
                    token: token.clone(),
                    issued_at_ms,
                    ttl_ms,
                },
                "RecordLiveWebrtcTokenIssued",
            )
            .await
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;

        effects
            .as_slice()
            .iter()
            .find_map(|effect| match effect {
                crate::meerkat_machine::dsl::MeerkatMachineEffect::LiveWebrtcTokenIssued {
                    session_id: effect_session_id,
                    channel_id: effect_channel_id,
                    token: effect_token,
                    expires_at_ms,
                    sequence,
                } if *effect_session_id == session_id.to_string()
                    && *effect_channel_id == channel_id
                    && *effect_token == token =>
                {
                    Some(LiveWebrtcTokenAuthority {
                        token: effect_token.clone(),
                        expires_at_ms: *expires_at_ms,
                        sequence: *sequence,
                    })
                }
                _ => None,
            })
            .ok_or_else(|| {
                RuntimeDriverError::Internal(format!(
                    "RecordLiveWebrtcTokenIssued for channel '{channel_id}' emitted no LiveWebrtcTokenIssued effect"
                ))
            })
    }

    #[cfg(feature = "live")]
    pub async fn resolve_live_webrtc_answer_admission(
        &self,
        session_id: &SessionId,
        channel_id: &meerkat_live::LiveChannelId,
        token: &str,
        observed_at_ms: u64,
    ) -> Result<LiveWebrtcAnswerAdmissionAuthority, RuntimeDriverError> {
        let _mutation_guard = self
            .lock_current_durability_ready_session_mutation_gate(session_id)
            .await?;
        let channel_id = channel_id.to_string();
        let token = token.to_string();
        let (_, effects) = self
            .apply_session_dsl_input(
                session_id,
                crate::meerkat_machine::dsl::MeerkatMachineInput::ResolveLiveWebrtcAnswerAdmission {
                    session_id: session_id.to_string(),
                    channel_id: channel_id.clone(),
                    token: token.clone(),
                    observed_at_ms,
                },
                "ResolveLiveWebrtcAnswerAdmission",
            )
            .await
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;

        effects
            .as_slice()
            .iter()
            .find_map(|effect| match effect {
                crate::meerkat_machine::dsl::MeerkatMachineEffect::LiveWebrtcAnswerAdmissionResolved {
                    session_id: effect_session_id,
                    channel_id: effect_channel_id,
                    token: effect_token,
                    admitted,
                    rejection,
                    public_error_class,
                    sequence,
                } if *effect_session_id == session_id.to_string()
                    && *effect_channel_id == channel_id
                    && *effect_token == token =>
                {
                    Some(LiveWebrtcAnswerAdmissionAuthority {
                        admitted: *admitted,
                        rejection: *rejection,
                        public_error_class: *public_error_class,
                        sequence: *sequence,
                        transport_seal: admitted.then(|| {
                            meerkat_live::LiveWebrtcAnswerAdmissionSeal::__from_generated_admission(
                                meerkat_live::LiveChannelId::new(effect_channel_id),
                                session_id.clone(),
                            )
                        }),
                    })
                }
                _ => None,
            })
            .ok_or_else(|| {
                RuntimeDriverError::Internal(format!(
                    "ResolveLiveWebrtcAnswerAdmission for channel '{channel_id}' emitted no LiveWebrtcAnswerAdmissionResolved effect"
                ))
            })
    }

    #[cfg(feature = "live")]
    pub async fn accept_live_webrtc_answer_and_bind_execution(
        &self,
        provider_binding: &meerkat_live::ProviderWebrtcBinding,
        bound_ready: &meerkat_live::ProviderWebrtcBoundReadyReceipt,
        answer_observation_sequence: u64,
    ) -> Result<LiveWebrtcAnswerExecutionBindingAuthority, RuntimeDriverError> {
        let session_id = provider_binding.session_id();
        let channel_id = provider_binding.channel_id();
        let fence_token = provider_binding.runtime_fence().get();
        let generation = provider_binding.runtime_generation().get();
        let _mutation_guard = self
            .lock_current_durability_ready_session_mutation_gate(session_id)
            .await?;
        let state = self.session_dsl_state(session_id).await.map_err(|reason| {
            RuntimeDriverError::ValidationFailed {
                reason: reason.to_string(),
            }
        })?;
        let dsl_runtime_id = state.active_runtime_id.clone().ok_or_else(|| {
            RuntimeDriverError::ValidationFailed {
                reason: "atomic WebRTC answer has no active generated runtime binding".to_string(),
            }
        })?;
        if state.active_fence_token
            != Some(crate::meerkat_machine::dsl::FenceToken::from_domain(
                fence_token,
            ))
            || state.active_runtime_generation
                != Some(crate::meerkat_machine::dsl::Generation::from_domain(
                    generation,
                ))
        {
            return Err(RuntimeDriverError::ValidationFailed {
                reason:
                    "provider bound-ready authority does not match the active runtime incarnation"
                        .to_string(),
            });
        }
        let runtime_id = crate::identifiers::LogicalRuntimeId::new(dsl_runtime_id.0.clone());
        let canonical_seed_cursor = bound_ready
            .__consume_for_generated_bind(provider_binding)
            .map_err(|error| RuntimeDriverError::ValidationFailed {
                reason: format!(
                    "provider bound-ready authority rejected: {}",
                    error.reason_code()
                ),
            })?;
        let channel = channel_id.to_string();
        let (_, effects) = self
            .apply_session_dsl_input(
                session_id,
                crate::meerkat_machine::dsl::MeerkatMachineInput::RecordLiveWebrtcAnswerAcceptedAndBindExecution {
                    session_id: session_id.to_string(),
                    channel_id: channel.clone(),
                    answer_observation_sequence,
                    runtime_id: crate::meerkat_machine::dsl::AgentRuntimeId::from_domain(&runtime_id),
                    fence_token: crate::meerkat_machine::dsl::FenceToken::from_domain(fence_token),
                    generation: crate::meerkat_machine::dsl::Generation::from_domain(generation),
                    canonical_seed_cursor,
                },
                "RecordLiveWebrtcAnswerAcceptedAndBindExecution",
            )
            .await
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;

        for effect in effects.as_slice() {
            let crate::meerkat_machine::dsl::MeerkatMachineEffect::LiveWebrtcAnswerAcceptedAndExecutionBound {
                session_id: effect_session_id,
                channel_id: effect_channel_id,
                status,
                answered,
                sequence,
                answer_observation_sequence: effect_answer_sequence,
                runtime_id: effect_runtime_id,
                fence_token: effect_fence,
                generation: effect_generation,
                canonical_seed_cursor: effect_seed_cursor,
            } = effect else {
                continue;
            };
            if effect_session_id != &session_id.to_string()
                || effect_channel_id != &channel
                || *effect_answer_sequence != answer_observation_sequence
                || effect_runtime_id
                    != &crate::meerkat_machine::dsl::AgentRuntimeId::from_domain(&runtime_id)
                || effect_fence
                    != &crate::meerkat_machine::dsl::FenceToken::from_domain(fence_token)
                || effect_generation
                    != &crate::meerkat_machine::dsl::Generation::from_domain(generation)
                || *effect_seed_cursor != canonical_seed_cursor
            {
                return Err(RuntimeDriverError::Internal(
                    "atomic WebRTC answer binding effect did not match exact authority input"
                        .to_string(),
                ));
            }
            let answer = LiveWebrtcAnswerResultAuthority {
                status: *status,
                answered: *answered,
                sequence: *sequence,
                answer_observation_sequence: *effect_answer_sequence,
            };
            let binding = crate::live_execution::LiveDelegationRuntimeBinding::new(
                session_id.clone(),
                channel_id.clone(),
                runtime_id,
                fence_token,
                generation,
            );
            self.shared
                .live_context_queued_rows
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .retain(|(queued_session, sequence), _| {
                    queued_session != session_id || *sequence > canonical_seed_cursor
                });
            return Ok(LiveWebrtcAnswerExecutionBindingAuthority::new(
                answer, binding,
            ));
        }
        Err(RuntimeDriverError::Internal(
            "atomic WebRTC answer binding emitted no matching generated effect".to_string(),
        ))
    }

    #[cfg(feature = "live")]
    pub async fn resolve_live_webrtc_answer_result(
        &self,
        session_id: &SessionId,
        channel_id: &meerkat_live::LiveChannelId,
        answer_observation_sequence: u64,
    ) -> Result<LiveWebrtcAnswerResultAuthority, RuntimeDriverError> {
        let _mutation_guard = self
            .lock_current_durability_ready_session_mutation_gate(session_id)
            .await?;
        let channel_id = channel_id.to_string();
        let (_, effects) = self
            .apply_session_dsl_input(
                session_id,
                crate::meerkat_machine::dsl::MeerkatMachineInput::RecordLiveWebrtcAnswerAccepted {
                    session_id: session_id.to_string(),
                    channel_id: channel_id.clone(),
                    answer_observation_sequence,
                },
                "RecordLiveWebrtcAnswerAccepted",
            )
            .await
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;

        effects
            .as_slice()
            .iter()
            .find_map(|effect| match effect {
                crate::meerkat_machine::dsl::MeerkatMachineEffect::LiveWebrtcAnswerResultResolved {
                    channel_id: effect_channel_id,
                    status,
                    answered,
                    sequence,
                    answer_observation_sequence: effect_observation_sequence,
                } if *effect_channel_id == channel_id
                    && *effect_observation_sequence == answer_observation_sequence =>
                {
                    Some(LiveWebrtcAnswerResultAuthority {
                        status: *status,
                        answered: *answered,
                        sequence: *sequence,
                        answer_observation_sequence: *effect_observation_sequence,
                    })
                }
                _ => None,
            })
            .ok_or_else(|| {
                RuntimeDriverError::Internal(format!(
                    "RecordLiveWebrtcAnswerAccepted for channel '{channel_id}' emitted no LiveWebrtcAnswerResultResolved effect"
                ))
            })
    }

    #[cfg(feature = "live")]
    pub async fn record_live_websocket_token_issued(
        &self,
        session_id: &SessionId,
        channel_id: &meerkat_live::LiveChannelId,
        token: &str,
        issued_at_ms: u64,
        ttl_ms: u64,
    ) -> Result<LiveWebsocketTokenAuthority, RuntimeDriverError> {
        let _mutation_guard = self
            .lock_current_durability_ready_session_mutation_gate(session_id)
            .await?;
        let channel_id = channel_id.to_string();
        let token = token.to_string();
        let (_, effects) = self
            .apply_session_dsl_input(
                session_id,
                crate::meerkat_machine::dsl::MeerkatMachineInput::RecordLiveWebsocketTokenIssued {
                    session_id: session_id.to_string(),
                    channel_id: channel_id.clone(),
                    token: token.clone(),
                    issued_at_ms,
                    ttl_ms,
                },
                "RecordLiveWebsocketTokenIssued",
            )
            .await
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;

        effects
            .as_slice()
            .iter()
            .find_map(|effect| match effect {
                crate::meerkat_machine::dsl::MeerkatMachineEffect::LiveWebsocketTokenIssued {
                    session_id: effect_session_id,
                    channel_id: effect_channel_id,
                    token: effect_token,
                    expires_at_ms,
                    sequence,
                } if *effect_session_id == session_id.to_string()
                    && *effect_channel_id == channel_id
                    && *effect_token == token =>
                {
                    Some(LiveWebsocketTokenAuthority {
                        token: effect_token.clone(),
                        expires_at_ms: *expires_at_ms,
                        sequence: *sequence,
                    })
                }
                _ => None,
            })
            .ok_or_else(|| {
                RuntimeDriverError::Internal(format!(
                    "RecordLiveWebsocketTokenIssued for channel '{channel_id}' emitted no LiveWebsocketTokenIssued effect"
                ))
            })
    }

    #[cfg(feature = "live")]
    pub async fn resolve_live_websocket_token_admission(
        &self,
        session_id: &SessionId,
        channel_id: &meerkat_live::LiveChannelId,
        token: &str,
        observed_at_ms: u64,
    ) -> Result<LiveWebsocketTokenAdmissionAuthority, RuntimeDriverError> {
        let _mutation_guard = self
            .lock_current_durability_ready_session_mutation_gate(session_id)
            .await?;
        let channel_id = channel_id.to_string();
        let token = token.to_string();
        let (_, effects) = self
            .apply_session_dsl_input(
                session_id,
                crate::meerkat_machine::dsl::MeerkatMachineInput::ResolveLiveWebsocketTokenAdmission {
                    session_id: session_id.to_string(),
                    channel_id: channel_id.clone(),
                    token: token.clone(),
                    observed_at_ms,
                },
                "ResolveLiveWebsocketTokenAdmission",
            )
            .await
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;

        extract_live_websocket_token_admission(
            effects.as_slice(),
            &session_id.to_string(),
            &channel_id,
            &token,
            "ResolveLiveWebsocketTokenAdmission",
        )
    }

    #[cfg(feature = "live")]
    pub async fn resolve_unbound_live_websocket_token_admission(
        &self,
        channel_id: &meerkat_live::LiveChannelId,
        token: &str,
        observed_at_ms: u64,
    ) -> Result<LiveWebsocketTokenAdmissionAuthority, RuntimeDriverError> {
        let channel_id = channel_id.to_string();
        let token = token.to_string();
        let effects = apply_dsl_transition_on_authority(
            &self.live_unbound_rejection_authority,
            crate::meerkat_machine::dsl::MeerkatMachineInput::ResolveLiveWebsocketTokenAdmission {
                session_id: String::new(),
                channel_id: channel_id.clone(),
                token: token.clone(),
                observed_at_ms,
            },
            "ResolveLiveWebsocketTokenAdmission:UnboundChannel",
        )
        .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;

        extract_live_websocket_token_admission(
            effects.as_slice(),
            "",
            &channel_id,
            &token,
            "ResolveLiveWebsocketTokenAdmission:UnboundChannel",
        )
    }

    #[cfg(feature = "live")]
    pub async fn resolve_live_channel_status_result(
        &self,
        session_id: &SessionId,
        observation: &meerkat_live::LiveChannelStatusObservation,
    ) -> Result<LiveChannelStatusAuthority, RuntimeDriverError> {
        let _mutation_guard = self
            .lock_current_durability_ready_session_mutation_gate(session_id)
            .await?;
        let channel_id = observation.channel_id().to_string();
        let (status, degradation_reason, degradation_detail) =
            dsl_live_channel_status_from_observation(observation.status());
        let (_, effects) = self
            .apply_session_dsl_input(
                session_id,
                crate::meerkat_machine::dsl::MeerkatMachineInput::RecordLiveChannelStatus {
                    channel_id: channel_id.clone(),
                    status,
                    status_observation_sequence: observation.observation_sequence(),
                    degradation_reason,
                    degradation_detail: degradation_detail.clone(),
                },
                "RecordLiveChannelStatus",
            )
            .await
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;

        let authority = effects.as_slice().iter().find_map(|effect| match effect {
            crate::meerkat_machine::dsl::MeerkatMachineEffect::LiveChannelStatusResolved {
                channel_id: effect_channel_id,
                status,
                sequence,
                status_observation_sequence,
                degradation_reason,
                degradation_detail,
            } if *effect_channel_id == channel_id
                && *status_observation_sequence == observation.observation_sequence() =>
            {
                Some(LiveChannelStatusAuthority::from_generated_effect(
                    effect_channel_id.clone(),
                    *status,
                    *sequence,
                    *status_observation_sequence,
                    *degradation_reason,
                    degradation_detail.clone(),
                ))
            }
            _ => None,
        });
        match authority {
            Some(Ok(authority)) => Ok(authority),
            Some(Err(reason)) => Err(RuntimeDriverError::Internal(reason)),
            None => Err(RuntimeDriverError::Internal(format!(
                "RecordLiveChannelStatus for channel '{channel_id}' emitted no LiveChannelStatusResolved effect"
            ))),
        }
    }

    pub(super) async fn cancel_after_boundary_inner(
        &self,
        session_id: &SessionId,
    ) -> Result<(), RuntimeDriverError> {
        self.cancel_after_boundary_inner_for_incarnation(session_id, None, false, None)
            .await
            .map(|_| ())
    }

    pub(super) async fn cancel_after_boundary_inner_for_incarnation(
        &self,
        session_id: &SessionId,
        expected_member: Option<
            &meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation,
        >,
        fence_member_residency: bool,
        requested_run_id: Option<&meerkat_core::RunId>,
    ) -> Result<bool, RuntimeDriverError> {
        let expected_member = expected_member.cloned();
        let (
            member_lease,
            witness,
            held_mutation_gate,
            boundary_handle,
            expected_run_id,
            projected_effect,
            pending_dispatch,
            dispatch_generation,
            dispatch_lifecycle_phase,
        ) = {
            let member_lease = if fence_member_residency {
                Some(
                    self.acquire_member_effect_authority_lease(
                        session_id,
                        expected_member.as_ref(),
                    )
                    .await?,
                )
            } else {
                match expected_member.as_ref() {
                    Some(expected_member) => Some(
                        self.acquire_member_effect_authority_lease(
                            session_id,
                            Some(expected_member),
                        )
                        .await?,
                    ),
                    None => None,
                }
            };
            let (captured_session_gate, preacquired_gate_guard) = match &member_lease {
                Some(lease) => (Arc::clone(&lease.session_mutation_gate), None),
                None => {
                    let guard = self
                        .lock_current_durability_ready_session_mutation_gate(session_id)
                        .await?;
                    let gate = {
                        let sessions = self.sessions.read().await;
                        let entry =
                            sessions
                                .get(session_id)
                                .ok_or(RuntimeDriverError::NotReady {
                                    state: RuntimeState::Destroyed,
                                })?;
                        Arc::clone(&entry.mutation_gate)
                    };
                    (gate, Some(guard))
                }
            };
            let held_mutation_gate = match preacquired_gate_guard {
                Some(guard) => guard,
                None => Arc::clone(&captured_session_gate).lock_owned().await,
            };
            let captured_dsl_authority = {
                let sessions = self.sessions.read().await;
                let Some(entry) = sessions.get(session_id) else {
                    return Err(if member_lease.is_some() {
                        RuntimeDriverError::StaleAuthority {
                            reason: "boundary-cancel runtime session disappeared".to_string(),
                        }
                    } else {
                        RuntimeDriverError::NotReady {
                            state: RuntimeState::Destroyed,
                        }
                    });
                };
                if !Arc::ptr_eq(&entry.mutation_gate, &captured_session_gate) {
                    return Err(RuntimeDriverError::StaleAuthority {
                        reason: "boundary-cancel runtime session was replaced".to_string(),
                    });
                }
                entry.require_durability_ready().map_err(|required| {
                    RuntimeDriverError::RecoveryRepairBlocked {
                        evidence_digest: None,
                        reason: required.to_string(),
                    }
                })?;
                Arc::clone(&entry.dsl_authority)
            };
            let state = self
                .existing_session_runtime_state(session_id)
                .await
                .unwrap_or(RuntimeState::Destroyed);
            // An exact-run request is teardown control, not new ingress. An
            // unregister/archive drain can begin after a queued input was
            // admitted but before that input binds its run. Retirement must
            // still be able to cancel that exact run once it becomes current.
            // Ordinary and member-routed cancellation retain the Draining
            // admission fence.
            if requested_run_id.is_none() {
                self.reject_unregistration_drain_ingress(session_id, state)
                    .await?;
            }
            if let Some(requested_run_id) = requested_run_id {
                let (raw_phase, current_run_id) = {
                    let authority = captured_dsl_authority
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    (
                        crate::meerkat_machine::dsl_authority::runtime_phase_from_authority(
                            &authority,
                        ),
                        crate::meerkat_machine::dsl_authority::current_run_id_from_authority(
                            &authority,
                        ),
                    )
                };
                if !matches!(raw_phase, RuntimeState::Running | RuntimeState::Retired)
                    || current_run_id.as_ref() != Some(requested_run_id)
                {
                    return Ok(false);
                }
            }
            let staged_result = if let Some(requested_run_id) = requested_run_id {
                // The exact teardown caller already proved the current run,
                // session gate, and DSL authority under M above. Stage on
                // that captured authority so an unregister-owned closed
                // handle gate cannot misclassify teardown control as a new
                // lifecycle mutation. Its distinct generated input also owns
                // the Retired-with-a-draining-run case without opening the
                // ambient CancelAfterBoundary input in Retired.
                Self::stage_dsl_transition_on_authority(
                    &captured_dsl_authority,
                    crate::meerkat_machine::dsl::MeerkatMachineInput::CancelAfterBoundaryForRun {
                        run_id: crate::meerkat_machine::dsl::RunId::from_domain(requested_run_id),
                        reason: "boundary cancel".to_string(),
                    },
                    "CancelAfterBoundaryForRun",
                )
            } else {
                self.stage_session_dsl_transition(
                    session_id,
                    crate::meerkat_machine::dsl::MeerkatMachineInput::CancelAfterBoundary {
                        reason: "boundary cancel".to_string(),
                    },
                    "CancelAfterBoundary",
                )
                .await
            };
            let staged = match staged_result {
                Ok(staged) => staged,
                Err(_) => {
                    // Stage-first classification (dispatch_user_interrupt
                    // shape): the machine rejected the input; a Destroyed
                    // binding surfaces as the terminal `Destroyed` truth,
                    // every other phase as `NotReady`.
                    let state = self
                        .existing_session_runtime_state(session_id)
                        .await
                        .unwrap_or(RuntimeState::Destroyed);
                    if state == RuntimeState::Destroyed {
                        return Err(RuntimeDriverError::Destroyed);
                    }
                    return Err(RuntimeDriverError::NotReady { state });
                }
            };
            let projected_effect =
                match crate::effect::runtime_effect_projection_optional_from_dsl_effects(
                    &staged.effects,
                ) {
                    Ok(projected_effect) => projected_effect,
                    Err(error) => {
                        let dispatch_generation = staged
                            .committed_snapshot
                            .state()
                            .boundary_cancel_dispatch_generation;
                        Self::abort_dsl_boundary_cancel_dispatch_if_current(
                            &captured_dsl_authority,
                            dispatch_generation,
                        )?;
                        return Err(RuntimeDriverError::Internal(error));
                    }
                };
            let Some(projected_effect) = projected_effect else {
                // Machine-owned convergence: a boundary-cancel dispatch is
                // already outstanding, so the machine took the typed
                // AlreadyPending arm (no RuntimeEffectFact). The request is
                // satisfied by the outstanding dispatch — and this bound is
                // what stops a boundary handle that re-enters
                // `cancel_after_boundary` from recursing unboundedly.
                let already_pending = staged.effects.as_slice().iter().any(|effect| {
                    matches!(
                        effect,
                        crate::meerkat_machine::dsl::MeerkatMachineEffect::BoundaryCancelAlreadyPending
                    )
                });
                if !already_pending {
                    return Err(RuntimeDriverError::Internal(
                        "CancelAfterBoundary emitted neither a RuntimeEffectFact nor BoundaryCancelAlreadyPending"
                            .to_string(),
                    ));
                }
                return Ok(true);
            };
            let expected_run_id = staged
                .committed_snapshot
                .state()
                .current_run_id
                .as_ref()
                .and_then(crate::meerkat_machine::dsl_authority::current_run_id_from_dsl);
            let dispatch_generation = staged
                .committed_snapshot
                .state()
                .boundary_cancel_dispatch_generation;
            let dispatch_lifecycle_phase = staged.committed_snapshot.state().lifecycle_phase;
            // Arm exact compensation before the first post-transition await.
            // From this point caller cancellation may only drop an
            // acknowledgement, never strand BoundaryCancelAlreadyPending.
            let pending_dispatch = PendingBoundaryCancelDispatchGuard::new(
                Arc::clone(&captured_dsl_authority),
                dispatch_generation,
            );

            let sessions = self.sessions.read().await;
            let Some(entry) = sessions.get(session_id) else {
                drop(sessions);
                Self::abort_dsl_boundary_cancel_dispatch_if_current(
                    &captured_dsl_authority,
                    dispatch_generation,
                )?;
                return Err(RuntimeDriverError::NotReady {
                    state: RuntimeState::Destroyed,
                });
            };
            if !Arc::ptr_eq(&entry.mutation_gate, &captured_session_gate)
                || !Arc::ptr_eq(&entry.dsl_authority, &captured_dsl_authority)
            {
                drop(sessions);
                Self::abort_dsl_boundary_cancel_dispatch_if_current(
                    &captured_dsl_authority,
                    dispatch_generation,
                )?;
                return Err(RuntimeDriverError::StaleAuthority {
                    reason: "boundary-cancel runtime session authority changed while staging"
                        .to_string(),
                });
            }
            let Some(attachment_id) = entry.live_attachment_id() else {
                drop(sessions);
                Self::abort_dsl_boundary_cancel_dispatch_if_current(
                    &captured_dsl_authority,
                    dispatch_generation,
                )?;
                return Err(RuntimeDriverError::NotReady {
                    state: RuntimeState::Idle,
                });
            };
            let Some(effect_tx) = entry.effect_sender() else {
                drop(sessions);
                Self::abort_dsl_boundary_cancel_dispatch_if_current(
                    &captured_dsl_authority,
                    dispatch_generation,
                )?;
                return Err(RuntimeDriverError::NotReady {
                    state: RuntimeState::Idle,
                });
            };
            (
                member_lease,
                RuntimeEffectDispatchAttachmentWitness {
                    mutation_gate: captured_session_gate,
                    driver: entry.driver.clone(),
                    dsl_authority: captured_dsl_authority,
                    attachment_id,
                    effect_tx,
                },
                held_mutation_gate,
                entry.boundary_handle(),
                expected_run_id,
                projected_effect,
                pending_dispatch,
                dispatch_generation,
                dispatch_lifecycle_phase,
            )
        };

        let member_authority = member_lease.map(|lease| RuntimeEffectDispatchMemberAuthority {
            lease,
            expected_member,
        });
        let gate_guard = self
            .dispatch_cancel_after_boundary_runtime_effect(
                session_id,
                witness,
                held_mutation_gate,
                boundary_handle,
                member_authority,
                pending_dispatch,
                expected_run_id.as_ref(),
                projected_effect,
                dispatch_generation,
                dispatch_lifecycle_phase,
                requested_run_id.is_some(),
                "CancelAfterBoundary",
            )
            .await?;
        drop(gate_guard);
        Ok(true)
    }

    /// Stop the attached runtime executor through the out-of-band control
    /// channel. When no loop is attached yet, a stop command is applied directly
    /// against the driver so queued work is still terminated consistently.
    pub async fn stop_runtime_executor(
        &self,
        session_id: &SessionId,
        reason: impl Into<String>,
    ) -> Result<(), RuntimeDriverError> {
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::StopRuntimeExecutor {
                    session_id: session_id.clone(),
                    reason: reason.into(),
                },
            )
            .await
            .map_err(MeerkatMachine::driver_error_from_command_error)?
        {
            MeerkatMachineCommandResult::Unit => Ok(()),
            other => Err(RuntimeDriverError::Internal(format!(
                "stop_runtime_executor: unexpected command result variant: {other:?}"
            ))),
        }
    }

    pub(super) async fn stop_runtime_executor_inner(
        &self,
        session_id: &SessionId,
        reason: String,
    ) -> Result<(), RuntimeDriverError> {
        self.request_runtime_stop(session_id, reason).await
    }

    /// Accept an input and return a completion handle that resolves when the
    /// input reaches a terminal state (Consumed or Abandoned).
    ///
    /// Returns `(AcceptOutcome, Option<CompletionHandle>)`:
    /// - `(Accepted, Some(handle))` — await handle for result
    /// - `(Accepted, None)` — input reached a terminal state during admission
    /// - `(Deduplicated, Some(handle))` — joined in-flight waiter
    /// - `(Deduplicated, None)` — input already terminal; no waiter needed
    /// - `(Rejected, _)` — returned as `Err(ValidationFailed)`
    pub async fn accept_input_with_completion(
        &self,
        session_id: &SessionId,
        input: Input,
    ) -> Result<(AcceptOutcome, Option<crate::completion::CompletionHandle>), RuntimeDriverError>
    {
        self.accept_input_with_completion_boxed(session_id, input)
            .await
    }

    pub(crate) async fn accept_peer_ingress_with_completion(
        &self,
        facts: meerkat_core::interaction::PeerIngressClaimCommitFacts,
        session_id: &SessionId,
        input: Input,
    ) -> super::PeerIngressAcceptFinalization {
        let submitted_input_id = input.id().clone();
        self.finalize_peer_ingress_accept(
            facts,
            submitted_input_id,
            self.accept_input_with_completion_boxed(session_id, input)
                .await,
        )
    }

    pub(crate) async fn accept_peer_ingress_with_completion_for_member_residency(
        &self,
        facts: meerkat_core::interaction::PeerIngressClaimCommitFacts,
        session_id: &SessionId,
        input: Input,
        expected_member: Option<
            &meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation,
        >,
    ) -> super::PeerIngressAcceptFinalization {
        let submitted_input_id = input.id().clone();
        let result = self
            .accept_input_with_completion_for_member_residency(session_id, input, expected_member)
            .await;
        // An exact member-residency mismatch is an authoritative rejection of
        // this addressed delivery, not an uncertain transport failure.  The
        // runtime-owned peer-ingress seam therefore terminalizes the queue
        // claim as Rejected so the sender cannot mistake a definite fence
        // refusal for ambiguous delivery.
        let result = match result {
            Err(RuntimeDriverError::StaleAuthority { reason }) => {
                Err(RuntimeDriverError::ValidationFailed { reason })
            }
            other => other,
        };
        self.finalize_peer_ingress_accept(facts, submitted_input_id, result)
    }

    fn finalize_peer_ingress_accept(
        &self,
        facts: meerkat_core::interaction::PeerIngressClaimCommitFacts,
        submitted_input_id: meerkat_core::lifecycle::InputId,
        result: Result<
            (AcceptOutcome, Option<crate::completion::CompletionHandle>),
            RuntimeDriverError,
        >,
    ) -> super::PeerIngressAcceptFinalization {
        use meerkat_core::interaction::PeerIngressClaimTerminalOutcome as Terminal;
        let (terminal, completion) = match result {
            Ok((outcome, completion)) => {
                let terminal = match &outcome {
                    AcceptOutcome::Accepted { input_id, .. } => Terminal::RuntimeAccepted {
                        input_id: input_id.clone(),
                    },
                    AcceptOutcome::Deduplicated {
                        input_id,
                        existing_id,
                        ..
                    } => Terminal::RuntimeDeduplicated {
                        input_id: input_id.clone(),
                        existing_id: existing_id.clone(),
                    },
                    AcceptOutcome::Rejected { .. } => Terminal::RuntimeRejected {
                        input_id: submitted_input_id,
                    },
                };
                (terminal, completion)
            }
            Err(RuntimeDriverError::ValidationFailed { .. }) => (
                Terminal::RuntimeRejected {
                    input_id: submitted_input_id,
                },
                None,
            ),
            Err(error) => return super::PeerIngressAcceptFinalization::MechanismError(error),
        };
        super::PeerIngressAcceptFinalization::Finalized {
            receipt: Arc::new(super::DurablePeerIngressAdmissionReceipt::new(
                facts, terminal,
            )),
            completion,
        }
    }

    /// Accept input only if one exact committed executor attachment still owns
    /// the session. The machine holds that session's mutation gate from the
    /// witness check through durable admission, so an attachment replacement
    /// cannot split surface request context from the executor that consumes it.
    pub async fn accept_input_with_completion_for_attachment(
        &self,
        witness: &RuntimeExecutorAttachmentWitness,
        input: Input,
    ) -> Result<(AcceptOutcome, Option<crate::completion::CompletionHandle>), RuntimeDriverError>
    {
        if !witness.belongs_to(self) {
            return Err(RuntimeDriverError::StaleAuthority {
                reason: "input admission attachment witness belongs to another machine".to_string(),
            });
        }
        match self
            .execute_meerkat_machine_ingress_command(MeerkatMachineCommand::AcceptWithCompletion {
                session_id: witness.session_id().clone(),
                input,
                register_completion: true,
                member_residency: MemberResidencyExpectation::Unfenced,
                expected_attachment: Some(witness.clone()),
            })
            .await?
        {
            MeerkatMachineCommandResult::AcceptWithCompletion {
                outcome,
                handle,
                admission_signal: _,
            } => Ok((outcome, handle)),
            other => Err(RuntimeDriverError::Internal(format!(
                "unexpected exact-attachment accept result: {other:?}"
            ))),
        }
    }

    /// Accept input only while both the exact executor attachment and the
    /// expected mob-member residency still own the session. `None` means true
    /// PeerOnly (never VacantPlaced). Both fences are checked under the same
    /// session mutation gate before durable admission, so a request cannot be
    /// transferred to a replacement attachment or member incarnation.
    pub async fn accept_input_with_completion_for_attachment_and_member_residency(
        &self,
        witness: &RuntimeExecutorAttachmentWitness,
        input: Input,
        expected_member: Option<
            &meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation,
        >,
    ) -> Result<(AcceptOutcome, Option<crate::completion::CompletionHandle>), RuntimeDriverError>
    {
        if !witness.belongs_to(self) {
            return Err(RuntimeDriverError::StaleAuthority {
                reason: "input admission attachment witness belongs to another machine".to_string(),
            });
        }
        let member_residency = expected_member
            .map_or(MemberResidencyExpectation::PeerOnly, |expected| {
                MemberResidencyExpectation::Placed(expected.clone())
            });
        match self
            .execute_meerkat_machine_ingress_command(MeerkatMachineCommand::AcceptWithCompletion {
                session_id: witness.session_id().clone(),
                input,
                register_completion: true,
                member_residency,
                expected_attachment: Some(witness.clone()),
            })
            .await?
        {
            MeerkatMachineCommandResult::AcceptWithCompletion {
                outcome,
                handle,
                admission_signal: _,
            } => Ok((outcome, handle)),
            other => Err(RuntimeDriverError::Internal(format!(
                "unexpected exact-attachment member-residency accept result: {other:?}"
            ))),
        }
    }

    /// Accept one bridge/raw-peer input under an exact member-residency
    /// expectation. `None` means true PeerOnly (never VacantPlaced). The
    /// ingress command holds the stable slot and uses the session gate
    /// captured by that lease, so same-SessionId replacement cannot occur
    /// between fence validation and durable acceptance.
    pub async fn accept_input_with_completion_for_member_residency(
        &self,
        session_id: &SessionId,
        input: Input,
        expected_member: Option<
            &meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation,
        >,
    ) -> Result<(AcceptOutcome, Option<crate::completion::CompletionHandle>), RuntimeDriverError>
    {
        let member_residency = expected_member
            .map_or(MemberResidencyExpectation::PeerOnly, |expected| {
                MemberResidencyExpectation::Placed(expected.clone())
            });
        match self
            .execute_meerkat_machine_ingress_command(MeerkatMachineCommand::AcceptWithCompletion {
                session_id: session_id.clone(),
                input,
                register_completion: true,
                member_residency,
                expected_attachment: None,
            })
            .await?
        {
            MeerkatMachineCommandResult::AcceptWithCompletion {
                outcome,
                handle,
                admission_signal: _,
            } => Ok((outcome, handle)),
            other => Err(RuntimeDriverError::Internal(format!(
                "unexpected fenced accept result: {other:?}"
            ))),
        }
    }

    /// Converge one exact durable tracked input to a runtime terminal under a
    /// full member-residency fence. Queued work is abandoned directly and
    /// persisted; staged/applied work is cancelled only through its exact run
    /// id so a retry can never interrupt a newer run.
    ///
    /// The host installs its durable `Cancelling` receipt before calling this
    /// method. Consequently a transient error is safe to retry and no delayed
    /// delivery can re-enter while runtime quiescence is incomplete.
    pub async fn cancel_tracked_input_for_member_incarnation(
        &self,
        session_id: &SessionId,
        idempotency_key: &str,
        expected_member: &meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation,
    ) -> Result<(), RuntimeDriverError> {
        use crate::input_state::{InputAbandonReason, InputLifecycleState};

        const SETTLE: std::time::Duration = std::time::Duration::from_secs(5);
        const RETRY: std::time::Duration = std::time::Duration::from_millis(25);
        let deadline = Instant::now() + SETTLE;
        loop {
            enum Action {
                Done,
                PublishQueued {
                    driver: crate::meerkat_machine::driver::SharedDriver,
                    completions: crate::meerkat_machine::driver::SharedCompletionRegistry,
                    mutation_gate: Arc<crate::tokio::sync::Mutex<()>>,
                    publication_handle: Option<
                        std::sync::Arc<dyn meerkat_core::lifecycle::CoreExecutorPublicationHandle>,
                    >,
                    input_id: meerkat_core::lifecycle::InputId,
                    candidate_owner_input_id: Option<meerkat_core::lifecycle::InputId>,
                },
                CancelRun(meerkat_core::RunId),
                Retry,
            }

            let authority = self
                .lock_member_effect_authority(session_id, expected_member)
                .await?;
            let (driver, completions, mutation_gate, publication_handle) = {
                let sessions = self.sessions.read().await;
                let entry =
                    sessions
                        .get(session_id)
                        .ok_or_else(|| RuntimeDriverError::StaleAuthority {
                            reason: "tracked-input cancellation runtime session disappeared"
                                .to_string(),
                        })?;
                (
                    entry.driver.clone(),
                    entry.completions.clone(),
                    Arc::clone(&entry.mutation_gate),
                    entry.publication_handle(),
                )
            };
            let action = {
                let mut driver_guard = driver.lock().await;
                let input_id = driver_guard
                    .as_driver()
                    .input_id_for_idempotency_key(idempotency_key);
                match input_id {
                    None => Action::Done,
                    Some(input_id) => {
                        let stored = driver_guard.as_driver().stored_input_state(&input_id);
                        match stored {
                            None => Action::Done,
                            Some(stored) if stored.seed.terminal_outcome.is_some() => Action::Done,
                            Some(stored) if stored.seed.phase == InputLifecycleState::Queued => {
                                let input_ids = vec![input_id.clone()];
                                let reason = "tracked input cancelled before run".to_string();
                                let prepared = driver_guard
                                    .prepare_runless_runtime_terminated_interaction_outboxes(
                                        &input_ids, reason,
                                    )?;
                                if let Err(error) = driver_guard
                                    .abandon_queued_input(&input_id, InputAbandonReason::Cancelled)
                                    .await
                                {
                                    driver_guard
                                        .rollback_prepared_runless_interaction_terminal_outboxes(
                                            prepared,
                                        );
                                    return Err(error);
                                }
                                let candidate_owner_input_id = crate::meerkat_machine::driver::DriverEntry::commit_prepared_runless_interaction_terminal_outboxes(prepared);
                                Action::PublishQueued {
                                    driver: driver.clone(),
                                    completions: completions.clone(),
                                    mutation_gate: Arc::clone(&mutation_gate),
                                    publication_handle: publication_handle.clone(),
                                    input_id,
                                    candidate_owner_input_id,
                                }
                            }
                            Some(stored) => stored
                                .seed
                                .last_run_id
                                .map_or(Action::Retry, Action::CancelRun),
                        }
                    }
                }
            };
            match action {
                Action::Done => return Ok(()),
                Action::PublishQueued {
                    driver,
                    completions,
                    mutation_gate,
                    publication_handle,
                    input_id,
                    candidate_owner_input_id,
                } => {
                    let dispatch = match (
                        candidate_owner_input_id.as_ref(),
                        publication_handle.clone(),
                    ) {
                        (Some(_), Some(publication_handle)) => {
                            Some(self.prepare_runless_terminal_publication_dispatch(
                                &driver,
                                &completions,
                                &mutation_gate,
                                publication_handle,
                            )?)
                        }
                        _ => None,
                    };
                    // The terminal carrier is durable and any issued
                    // publication handle is actor-exact. Release both M and
                    // the member-residency slot before polling arbitrary
                    // publication IO so replacement is never pinned behind a
                    // wedged callback.
                    drop(authority);
                    let publication = if let Some((result_rx, start_tx)) = dispatch {
                        if let Some(start_tx) = start_tx {
                            let _ = start_tx.send(());
                        }
                        self.await_runless_terminal_publication_dispatch(
                            &LogicalRuntimeId::for_session(session_id),
                            result_rx,
                            Some(deadline),
                        )
                        .await
                    } else {
                        crate::control_plane::publish_and_resolve_runless_runtime_termination_before(
                            &driver,
                            Some(&completions),
                            publication_handle.as_deref(),
                            std::slice::from_ref(&input_id),
                            candidate_owner_input_id.as_ref(),
                            "tracked input cancelled before run",
                            Some(deadline),
                        )
                        .await
                    };
                    if let Err(error) = publication {
                        if candidate_owner_input_id.is_none() {
                            return Err(error);
                        }
                        crate::control_plane::converge_known_committed_runless_runtime_terminations_before(
                            &driver,
                            Some(&completions),
                            publication_handle.as_deref(),
                            Some(deadline),
                        )
                        .await?;
                    }
                    return Ok(());
                }
                Action::CancelRun(run_id) => {
                    drop(authority);
                    let _ = self
                        .hard_cancel_run_if_current_for_member_incarnation(
                            session_id,
                            &run_id,
                            expected_member,
                            "tracked input cancelled by supervisor".to_string(),
                        )
                        .await?;
                }
                Action::Retry => drop(authority),
            }
            if Instant::now() >= deadline {
                return Err(RuntimeDriverError::Internal(format!(
                    "tracked input '{idempotency_key}' did not reach a runtime terminal before the cancellation settle deadline"
                )));
            }
            crate::tokio::time::sleep(RETRY).await;
        }
    }

    /// Converge one exact accepted input to a runtime terminal without ever
    /// falling back to the session's ambient current run.
    ///
    /// Request surfaces install this action only after admission has produced
    /// the input id. A delayed cancellation for input A therefore either
    /// abandons A while it is still queued or interrupts A's exact run id; it
    /// cannot interrupt a newer input B that has since become current. The
    /// captured driver pointer also turns same-`SessionId` replacement into a
    /// level-triggered no-op.
    ///
    /// Returns `true` when the exact input was observed (including already
    /// terminal) and `false` when its runtime attachment is gone or the input
    /// is absent.
    pub async fn cancel_input_if_present(
        &self,
        session_id: &SessionId,
        input_id: &meerkat_core::lifecycle::InputId,
        reason: impl Into<String>,
    ) -> Result<bool, RuntimeDriverError> {
        let machine = self.clone();
        let session_id = session_id.clone();
        let input_id = input_id.clone();
        let reason = reason.into();
        let cleanup_spawner = MachineCleanupTaskSpawner::acquire()?;
        let completion = cleanup_spawner.spawn(async move {
            machine
                .cancel_input_if_present_owned(&session_id, &input_id, reason)
                .await
        });
        completion.await.map_err(|error| {
            RuntimeDriverError::Internal(format!(
                "owned exact-input cancellation ended without a result: {error}"
            ))
        })?
    }

    async fn cancel_input_if_present_owned(
        &self,
        session_id: &SessionId,
        input_id: &meerkat_core::lifecycle::InputId,
        reason: String,
    ) -> Result<bool, RuntimeDriverError> {
        const SETTLE: std::time::Duration = std::time::Duration::from_secs(5);

        self.cancel_input_if_present_owned_before(
            session_id,
            input_id,
            reason,
            Instant::now() + SETTLE,
        )
        .await
    }

    async fn cancel_input_if_present_owned_before(
        &self,
        session_id: &SessionId,
        input_id: &meerkat_core::lifecycle::InputId,
        reason: String,
        deadline: Instant,
    ) -> Result<bool, RuntimeDriverError> {
        use crate::input_state::{InputAbandonReason, InputLifecycleState};

        const RETRY: std::time::Duration = std::time::Duration::from_millis(25);

        let driver = {
            let sessions = self.sessions.read().await;
            let Some(entry) = sessions.get(session_id) else {
                return Ok(false);
            };
            entry.driver.clone()
        };
        loop {
            enum Action {
                Missing,
                DrainTerminal {
                    completions: crate::meerkat_machine::driver::SharedCompletionRegistry,
                    publication_handle: Option<
                        std::sync::Arc<dyn meerkat_core::lifecycle::CoreExecutorPublicationHandle>,
                    >,
                },
                PublishQueued {
                    completions: crate::meerkat_machine::driver::SharedCompletionRegistry,
                    publication_handle: Option<
                        std::sync::Arc<dyn meerkat_core::lifecycle::CoreExecutorPublicationHandle>,
                    >,
                    candidate_owner_input_id: Option<meerkat_core::lifecycle::InputId>,
                },
                CancelRun(meerkat_core::RunId),
                Retry,
            }

            let authority = match self
                .lock_current_session_driver_gate(session_id, &driver)
                .await
            {
                Ok(authority) => authority,
                Err(
                    RuntimeDriverError::NotReady {
                        state: RuntimeState::Destroyed,
                    }
                    | RuntimeDriverError::Destroyed,
                ) => return Ok(false),
                Err(error) => return Err(error),
            };
            let (completions, publication_handle, mutation_gate) = {
                let sessions = self.sessions.read().await;
                let Some(entry) = sessions.get(session_id) else {
                    return Ok(false);
                };
                if !std::sync::Arc::ptr_eq(&entry.driver, &driver) {
                    return Ok(false);
                }
                (
                    entry.completions.clone(),
                    entry.publication_handle(),
                    Arc::clone(&entry.mutation_gate),
                )
            };
            let action = {
                let mut driver_guard = driver.lock().await;
                match driver_guard.as_driver().stored_input_state(input_id) {
                    None => Action::Missing,
                    Some(stored) if stored.seed.terminal_outcome.is_some() => {
                        Action::DrainTerminal {
                            completions,
                            publication_handle,
                        }
                    }
                    Some(stored) if stored.seed.phase == InputLifecycleState::Queued => {
                        let prepared = driver_guard
                            .prepare_runless_runtime_terminated_interaction_outboxes(
                                std::slice::from_ref(input_id),
                                reason.clone(),
                            )?;
                        if let Err(error) = driver_guard
                            .abandon_queued_input(input_id, InputAbandonReason::Cancelled)
                            .await
                        {
                            driver_guard
                                .rollback_prepared_runless_interaction_terminal_outboxes(prepared);
                            return Err(error);
                        }
                        let candidate_owner_input_id = crate::meerkat_machine::driver::DriverEntry::commit_prepared_runless_interaction_terminal_outboxes(prepared);
                        Action::PublishQueued {
                            completions,
                            publication_handle,
                            candidate_owner_input_id,
                        }
                    }
                    Some(stored) => stored
                        .seed
                        .last_run_id
                        .map_or(Action::Retry, Action::CancelRun),
                }
            };

            match action {
                Action::Missing => return Ok(false),
                Action::DrainTerminal {
                    completions,
                    publication_handle,
                } => {
                    // A prior attempt may have committed the runless terminal
                    // carrier and then lost publication. Terminal observation
                    // is not success until canonical recovery has published
                    // that carrier and resolved its waiter.
                    let dispatch =
                        match self.existing_runless_terminal_publication_dispatch(&driver) {
                            Some(result_rx) => Some((result_rx, None)),
                            None => publication_handle
                                .map(|publication_handle| {
                                    self.prepare_runless_terminal_publication_dispatch(
                                        &driver,
                                        &completions,
                                        &mutation_gate,
                                        publication_handle,
                                    )
                                })
                                .transpose()?,
                        };
                    drop(authority);
                    if let Some((result_rx, start_tx)) = dispatch {
                        if let Some(start_tx) = start_tx {
                            let _ = start_tx.send(());
                        }
                        self.await_runless_terminal_publication_dispatch(
                            &LogicalRuntimeId::for_session(session_id),
                            result_rx,
                            Some(deadline),
                        )
                        .await?;
                    } else {
                        crate::control_plane::converge_known_committed_runless_runtime_terminations_before(
                            &driver,
                            Some(&completions),
                            None,
                            Some(deadline),
                        )
                        .await?;
                    }
                    return Ok(true);
                }
                Action::PublishQueued {
                    completions,
                    publication_handle,
                    candidate_owner_input_id,
                } => {
                    let dispatch = match (
                        candidate_owner_input_id.as_ref(),
                        publication_handle.clone(),
                    ) {
                        (Some(_), Some(publication_handle)) => {
                            Some(self.prepare_runless_terminal_publication_dispatch(
                                &driver,
                                &completions,
                                &mutation_gate,
                                publication_handle,
                            )?)
                        }
                        _ => None,
                    };
                    drop(authority);
                    let publication = if let Some((result_rx, start_tx)) = dispatch {
                        if let Some(start_tx) = start_tx {
                            let _ = start_tx.send(());
                        }
                        self.await_runless_terminal_publication_dispatch(
                            &LogicalRuntimeId::for_session(session_id),
                            result_rx,
                            Some(deadline),
                        )
                        .await
                    } else {
                        crate::control_plane::publish_and_resolve_runless_runtime_termination_before(
                            &driver,
                            Some(&completions),
                            None,
                            std::slice::from_ref(input_id),
                            candidate_owner_input_id.as_ref(),
                            &reason,
                            Some(deadline),
                        )
                        .await
                    };
                    if let Err(error) = publication {
                        if candidate_owner_input_id.is_none() {
                            // Nondirected completion has no durable outbox to
                            // authorize replay. Its process-owned handoff
                            // either completed or returned the exact fatal
                            // error; an empty recovery scan is not success.
                            return Err(error);
                        }
                        tracing::warn!(
                            %session_id,
                            %input_id,
                            error = %error,
                            "exact queued-input cancellation committed its terminal carrier; retrying canonical publication recovery"
                        );
                        crate::control_plane::converge_known_committed_runless_runtime_terminations_before(
                            &driver,
                            Some(&completions),
                            publication_handle.as_deref(),
                            Some(deadline),
                        )
                        .await?;
                    }
                    return Ok(true);
                }
                Action::CancelRun(run_id) => {
                    drop(authority);
                    let _ = self
                        .hard_cancel_run_if_current(session_id, &run_id, reason.clone())
                        .await?;
                    // The run may have terminalized between the exact input
                    // read and the run-fenced interrupt. Re-read A instead of
                    // ever widening to the ambient current run.
                }
                Action::Retry => {
                    drop(authority);
                }
            }

            if Instant::now() >= deadline {
                return Err(RuntimeDriverError::Internal(format!(
                    "input '{input_id}' did not reach a runtime terminal before the cancellation settle deadline"
                )));
            }
            crate::tokio::time::sleep(RETRY).await;
        }
    }

    pub fn accept_input_with_completion_boxed<'a>(
        &'a self,
        session_id: &'a SessionId,
        input: Input,
    ) -> AcceptInputWithCompletionFuture<'a> {
        let input_id = input.id().clone();
        self.accept_boxed_input_with_completion(session_id, Box::new(input), input_id)
    }

    pub fn accept_boxed_input_with_completion<'a>(
        &'a self,
        session_id: &'a SessionId,
        input: Box<Input>,
        _input_id: InputId,
    ) -> AcceptInputWithCompletionFuture<'a> {
        let session_id = session_id.clone();
        Box::pin(async move {
            let input = *input;
            match self
                .execute_meerkat_machine_ingress_command(
                    MeerkatMachineCommand::AcceptWithCompletion {
                        session_id: session_id.clone(),
                        input,
                        register_completion: true,
                        member_residency: MemberResidencyExpectation::Unfenced,
                        expected_attachment: None,
                    },
                )
                .await?
            {
                MeerkatMachineCommandResult::AcceptWithCompletion {
                    outcome,
                    handle,
                    admission_signal: _,
                } => Ok((outcome, handle)),
                other => Err(RuntimeDriverError::Internal(format!(
                    "unexpected command result for accept_input_with_completion: {other:?}"
                ))),
            }
        })
    }

    /// Accept an input but intentionally do not wake the runtime loop.
    ///
    /// This is reserved for explicitly queued-only surface contracts that
    /// stage work for the next turn boundary instead of waking an idle session
    /// immediately.
    pub async fn accept_input_without_wake(
        &self,
        session_id: &SessionId,
        input: Input,
    ) -> Result<AcceptOutcome, RuntimeDriverError> {
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::AcceptWithoutWake {
                    session_id: session_id.clone(),
                    input,
                },
            )
            .await
            .map_err(MeerkatMachine::driver_error_from_command_error)?
        {
            MeerkatMachineCommandResult::AcceptOutcome(outcome) => Ok(outcome),
            other => Err(RuntimeDriverError::Internal(format!(
                "unexpected command result for accept_input_without_wake: {other:?}"
            ))),
        }
    }

    /// Get the shared ops lifecycle registry for a session/runtime instance.
    pub async fn ops_lifecycle_registry(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<crate::ops_lifecycle::RuntimeOpsLifecycleRegistry>> {
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::OpsLifecycleRegistry {
                    session_id: session_id.clone(),
                },
            )
            .await
        {
            Ok(MeerkatMachineCommandResult::OpsLifecycleRegistry(registry)) => registry,
            Ok(_) => {
                tracing::error!("ops_lifecycle_registry: unexpected command result variant");
                None
            }
            Err(_) => None,
        }
    }

    /// Prepare canonical runtime bindings for a session.
    ///
    /// This is the single canonical helper that replaces the hand-rolled
    /// `register_session()` + `ops_lifecycle_registry()` + manual threading
    /// dance. All runtime-backed surfaces should call this instead.
    ///
    /// The method is idempotent: if the session is already registered, it
    /// returns bindings from the existing entry. The epoch_id is stable
    /// across repeated calls for the same session.
    pub async fn prepare_bindings(
        &self,
        session_id: SessionId,
    ) -> Result<meerkat_core::SessionRuntimeBindings, RuntimeBindingsError> {
        match Box::pin(self.prepare_session_runtime_bindings(
            session_id.clone(),
            super::dispatch_session::SessionBindingPreparation::AuthoritativeRuntimeBinding,
        ))
        .await
        {
            Ok(MeerkatMachineCommandResult::Bindings(bindings)) => Ok(bindings),
            Ok(_) => {
                tracing::error!("prepare_bindings: unexpected command result variant");
                Err(RuntimeBindingsError::SessionNotFound(session_id))
            }
            Err(err) => Err(RuntimeBindingsError::PrepareFailed(
                session_id,
                err.to_string(),
            )),
        }
    }

    /// Install the exact runtime-placement tuple for a session whose local
    /// resources were created with [`Self::prepare_local_session_bindings`].
    ///
    /// Remote mob hosts cannot use [`Self::prepare_bindings`]: that helper
    /// derives a session-owned runtime id with fence/generation zero. The
    /// controller already supplied the canonical member incarnation, so the
    /// host must commit those exact facts through generated
    /// `PrepareBindings` authority before attaching an executor or accepting a
    /// turn.
    pub async fn prepare_runtime_placement_binding(
        &self,
        session_id: SessionId,
        agent_runtime_id: crate::identifiers::LogicalRuntimeId,
        fence_token: u64,
        generation: u64,
    ) -> Result<(), RuntimeBindingsError> {
        let _mutation_guard = self
            .lock_current_durability_ready_session_mutation_gate(&session_id)
            .await
            .map_err(|error| match error {
                RuntimeDriverError::NotReady {
                    state: RuntimeState::Destroyed,
                } => RuntimeBindingsError::SessionNotFound(session_id.clone()),
                error => RuntimeBindingsError::PrepareFailed(session_id.clone(), error.to_string()),
            })?;
        let (driver_handle, epoch_id) = {
            let sessions = self.sessions.read().await;
            let entry = sessions
                .get(&session_id)
                .ok_or_else(|| RuntimeBindingsError::SessionNotFound(session_id.clone()))?;
            (Arc::clone(&entry.driver), entry.epoch_id.clone())
        };
        self.commit_runtime_placement_binding(
            &session_id,
            &driver_handle,
            &epoch_id,
            agent_runtime_id,
            fence_token,
            generation,
        )
        .await
        .map_err(|error| RuntimeBindingsError::PrepareFailed(session_id, error.to_string()))
    }

    /// Prepare factory-consumable session runtime resources without emitting
    /// cross-machine binding signals.
    ///
    /// Mob provisioning uses this to pre-create the session-owned handle bundle
    /// before `MobMachine::Spawn` has committed the member runtime id. The
    /// authoritative mob binding is routed later through
    /// `RequestRuntimeBinding -> PrepareBindings`, which emits the typed
    /// `RuntimeBound` signal with the mob-owned `AgentRuntimeId` and fence.
    pub async fn prepare_local_session_bindings(
        &self,
        session_id: SessionId,
    ) -> Result<meerkat_core::SessionRuntimeBindings, RuntimeBindingsError> {
        match Box::pin(self.prepare_session_runtime_bindings(
            session_id.clone(),
            super::dispatch_session::SessionBindingPreparation::LocalSessionResources(
                super::LocalSessionMaterializationMode::Ordinary,
            ),
        ))
        .await
        {
            Ok(MeerkatMachineCommandResult::Bindings(bindings)) => Ok(bindings),
            Ok(_) => {
                tracing::error!(
                    "prepare_local_session_bindings: unexpected command result variant"
                );
                Err(RuntimeBindingsError::SessionNotFound(session_id))
            }
            // Never launder the real preparation failure into "session not
            // found": a typed machine rejection (e.g. a PrepareBindings
            // guard) must reach the caller verbatim — the field spent a
            // debugging cycle on "not found in runtime adapter after
            // registration" that was actually a binding guard rejection.
            Err(err) => Err(RuntimeBindingsError::PrepareFailed(
                session_id,
                err.to_string(),
            )),
        }
    }
}
