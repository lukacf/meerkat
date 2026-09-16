use super::*;
use futures::StreamExt as _;
use meerkat_mob::MobSessionService as _;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Workspace(std::path::PathBuf);

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct GateClient {
    calls: AtomicUsize,
    entered: tokio::sync::mpsc::UnboundedSender<usize>,
    cancelled: tokio::sync::mpsc::UnboundedSender<usize>,
    release_text: tokio::sync::Semaphore,
}

struct InFlightCall {
    index: usize,
    cancelled: tokio::sync::mpsc::UnboundedSender<usize>,
    completed: bool,
}

impl Drop for InFlightCall {
    fn drop(&mut self) {
        if !self.completed {
            let _ = self.cancelled.send(self.index);
        }
    }
}

#[async_trait::async_trait]
impl meerkat_client::LlmClient for GateClient {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, meerkat_client::LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        _request: &'a meerkat_client::LlmRequest,
    ) -> meerkat_client::types::LlmStream<'a> {
        let response = futures::stream::once(async move {
            let index = self.calls.fetch_add(1, Ordering::SeqCst);
            let mut call = InFlightCall {
                index,
                cancelled: self.cancelled.clone(),
                completed: false,
            };
            let _ = self.entered.send(index);
            match index {
                0 => std::future::pending::<()>().await,
                1 => self
                    .release_text
                    .acquire()
                    .await
                    .expect("text gate")
                    .forget(),
                _ => {}
            }
            call.completed = true;
            Ok::<_, meerkat_client::LlmError>(meerkat_client::LlmEvent::TextDelta {
                delta: "completed".to_string(),
                meta: None,
            })
        });
        Box::pin(response.chain(futures::stream::once(async {
            Ok(meerkat_client::LlmEvent::Done {
                outcome: meerkat_client::LlmDoneOutcome::Success {
                    stop_reason: meerkat_core::StopReason::EndTurn,
                },
            })
        })))
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::OpenAI
    }

    async fn health_check(&self) -> Result<(), meerkat_client::LlmError> {
        Ok(())
    }
}

#[tokio::test]
async fn existing_voice_supersession_precedes_source_commit_and_preserves_text() {
    let workspace = Workspace(std::path::PathBuf::from(format!(
        ".live-supersession-{}",
        uuid::Uuid::new_v4(),
    )));
    std::fs::create_dir(&workspace.0).expect("project-local test root");
    for name in ["user", "runtime", "project", "context"] {
        std::fs::create_dir(workspace.0.join(name)).expect("factory root");
    }
    let (entered_tx, mut entered_rx) = tokio::sync::mpsc::unbounded_channel();
    let (cancelled_tx, mut cancelled_rx) = tokio::sync::mpsc::unbounded_channel();
    let client = Arc::new(GateClient {
        calls: AtomicUsize::new(0),
        entered: entered_tx,
        cancelled: cancelled_tx,
        release_text: tokio::sync::Semaphore::new(0),
    });
    let factory = meerkat::AgentFactory::new(workspace.0.join("factory"))
        .user_config_root(workspace.0.join("user"))
        .runtime_root(workspace.0.join("runtime"))
        .project_root(workspace.0.join("project"))
        .context_root(workspace.0.join("context"))
        .builtins(false)
        .comms(true);
    let mut builder = meerkat::FactoryAgentBuilder::new(factory, meerkat::Config::default());
    builder.default_llm_client = Some(client.clone());
    let store = Arc::new(meerkat_store::MemoryStore::new());
    builder.default_session_store = Some(Arc::new(meerkat_store::StoreAdapter::new(store.clone())));
    let service = Arc::new(meerkat_session::PersistentSessionService::new(
        builder,
        8,
        store,
        Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
        Arc::new(meerkat_store::MemoryBlobStore::new()),
    ));
    let runtime = service.runtime_adapter().expect("runtime");
    let mobs = Arc::new(crate::MobMcpState::new(
        service.clone(),
        meerkat_mob::MobControlPrincipal::Owner,
    ));
    let mob_id = mobs
        .mob_create_definition(meerkat_mob::MobDefinition::implicit(
            &format!("supersession-{}", uuid::Uuid::new_v4()),
            "gpt-5.5",
        ))
        .await
        .expect("mob");
    let identity = AgentIdentity::from("existing-source");
    let mut spec = meerkat_mob::SpawnMemberSpec::new("delegate", identity.as_str());
    spec.runtime_mode = Some(meerkat_mob::MobRuntimeMode::TurnDriven);
    mobs.mob_spawn_spec(&mob_id, spec)
        .await
        .expect("source member");
    let handle = mobs.handle_for(&mob_id).await.expect("mob handle");
    let session_id = handle
        .resolve_bridge_session_id(&identity)
        .await
        .expect("source session");
    let coordinator = Arc::new(
        ExperimentalLiveDelegationCoordinator::new(Arc::clone(&runtime), mobs)
            .with_execution_policy(LiveDelegationExecutionPolicy::ExistingMember),
    );
    let control: Arc<dyn ExperimentalGptLiveControlPlane> =
        Arc::new(ExactProjectionControl::default());

    let binding = runtime
        .__test_open_live_delegation_channel(&session_id)
        .await
        .expect("binding");
    let provider_binding = provider_binding_from_runtime(&binding);
    let first_turn = LiveSidebandTurnRef::__from_provider_observation(
        binding.channel_id(),
        "first-turn".to_string(),
        "first-provider-turn".to_string(),
    )
    .expect("first turn");
    let first_delegation = LiveSidebandDelegationRef::__from_provider_observation(
        "first-delegation".to_string(),
        "first-provider-delegation".to_string(),
    )
    .expect("first delegation");
    coordinator
        .observe_turn_started(&LiveSidebandObservation::new(
            provider_binding.clone(),
            LiveSidebandObservationKind::TurnStarted {
                turn: first_turn.clone(),
                role: meerkat_live::LiveSidebandTurnRole::User,
            },
        ))
        .await
        .expect("first user starts");
    coordinator
        .observe_delegation_turn_finished(
            &LiveSidebandObservation::new(
                provider_binding.clone(),
                LiveSidebandObservationKind::TurnFinished {
                    turn: first_turn.clone(),
                    role: meerkat_live::LiveSidebandTurnRole::User,
                    transcript: "old voice work that remains blocked".to_string(),
                },
            ),
            &first_delegation,
            "old voice work that remains blocked",
        )
        .await
        .expect("first user turn finished");
    let first_operation = coordinator
        .completed_delegation_turns
        .lock()
        .await
        .get(&(
            session_id.clone(),
            binding.channel_id().clone(),
            first_turn.adapter_key().to_string(),
        ))
        .expect("first operation admitted by coordinator")
        .operation
        .clone();
    coordinator
        .start_client_context_delegation(
            &provider_binding,
            Arc::clone(&control),
            first_turn,
            first_delegation,
            "old voice work that remains blocked".to_string(),
        )
        .await
        .expect("first existing voice started");
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(5), entered_rx.recv())
            .await
            .expect("old voice entered model"),
        Some(0)
    );

    let text = handle
        .start_work_for_identity_bounded(
            identity.clone(),
            meerkat_mob::WorkSpec::new(
                "unrelated ordinary text",
                meerkat_mob::WorkOrigin::Internal,
            ),
            meerkat_core::types::HandlingMode::Queue,
            BoundedResultSpec::new("text", 256).expect("bound"),
        )
        .await
        .expect("queue unrelated text");
    let second_turn = LiveSidebandTurnRef::__from_provider_observation(
        binding.channel_id(),
        "second-turn".to_string(),
        "second-provider-turn".to_string(),
    )
    .expect("second turn");
    let second_delegation = LiveSidebandDelegationRef::__from_provider_observation(
        "second-delegation".to_string(),
        "second-provider-delegation".to_string(),
    )
    .expect("second delegation");
    coordinator
        .observe_turn_started(&LiveSidebandObservation::new(
            provider_binding.clone(),
            LiveSidebandObservationKind::TurnStarted {
                turn: second_turn.clone(),
                role: meerkat_live::LiveSidebandTurnRole::User,
            },
        ))
        .await
        .expect("second user begins");
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        coordinator.observe_delegation_turn_finished(
            &LiveSidebandObservation::new(
                provider_binding.clone(),
                LiveSidebandObservationKind::TurnFinished {
                    turn: second_turn.clone(),
                    role: meerkat_live::LiveSidebandTurnRole::User,
                    transcript: "replacement voice request".to_string(),
                },
            ),
            &second_delegation,
            "replacement voice request",
        ),
    )
    .await
    .expect("supersession must not wait for old voice's natural completion")
    .expect("second delegation admitted after exact old cancellation");
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(5), cancelled_rx.recv())
            .await
            .expect("old voice cancellation"),
        Some(0)
    );
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(5), entered_rx.recv())
            .await
            .expect("unrelated text starts"),
        Some(1)
    );

    let start_owner = Arc::clone(&coordinator);
    let next_binding = provider_binding.clone();
    let mut replacement = tokio::spawn(async move {
        start_owner
            .start_client_context_delegation(
                &next_binding,
                control,
                second_turn,
                second_delegation,
                "replacement voice request".to_string(),
            )
            .await
    });
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), &mut replacement)
            .await
            .is_err(),
        "new canonical transcript commit must wait for unrelated text, not interrupt it"
    );
    assert!(
        cancelled_rx.try_recv().is_err(),
        "unrelated text must remain running"
    );
    assert_eq!(
        client.calls.load(Ordering::SeqCst),
        2,
        "replacement cannot execute before final commit"
    );

    client.release_text.add_permits(1);
    text.wait_bounded(BoundedResultSpec::new("text", 256).expect("bound"))
        .await
        .expect("unrelated text completes normally");
    tokio::time::timeout(std::time::Duration::from_secs(5), replacement)
        .await
        .expect("replacement admission after text completion")
        .expect("replacement task")
        .expect("replacement accepted");
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(5), entered_rx.recv())
            .await
            .expect("replacement executes"),
        Some(2)
    );
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let snapshots = runtime
                .live_delegation_recovery_snapshots(&session_id)
                .await
                .expect("snapshots");
            if snapshots.len() == 2
                && snapshots.iter().all(|snapshot| {
                    snapshot.phase()
                        == meerkat_runtime::live_execution::LiveDelegationRecoveryPhase::Retired
                })
            {
                assert_eq!(
                    snapshots
                        .iter()
                        .find(|snapshot| snapshot.operation_id() == first_operation.operation_id())
                        .expect("old operation")
                        .terminal(),
                    Some(LiveDelegationWorkerTerminalKind::Cancelled)
                );
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("both exact operations terminalize");
    assert!(
        cancelled_rx.try_recv().is_err(),
        "no unrelated run was interrupted"
    );
    assert_eq!(
        handle.resolve_bridge_session_id(&identity).await,
        Some(session_id.clone())
    );
    coordinator.cancel_channel_binding(&provider_binding).await;
    runtime
        .abandon_live_open_admission(&session_id, binding.channel_id())
        .await
        .expect("close fixture channel");
    handle.shutdown().await.expect("shutdown");
}
