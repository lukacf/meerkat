//! Retirement I/O owns one incarnation, not the mob actor's command loop.

use super::*;
use crate::roster::RosterEntry;
use crate::runtime::state::{MobCommand, RetireMemberIncarnation};

struct ReleaseRetirementGate(Arc<TestRuntimeControlBarrier>);

impl Drop for ReleaseRetirementGate {
    fn drop(&mut self) {
        self.0.release_all();
    }
}

async fn retirement_fixture() -> (
    MobHandle,
    Arc<MockSessionService>,
    RosterEntry,
    AgentIdentity,
) {
    let (handle, service) = create_test_mob(sample_definition()).await;
    let retiring = AgentIdentity::from("retirement-isolation-target");
    let peer = AgentIdentity::from("retirement-isolation-peer");
    for identity in [&retiring, &peer] {
        handle
            .spawn_with_options(
                ProfileName::from("lead"),
                identity.clone(),
                None,
                Some(crate::MobRuntimeMode::TurnDriven),
                None,
            )
            .await
            .expect("spawn isolated member");
    }
    let entry = handle
        .get_member(&retiring)
        .await
        .expect("roster")
        .expect("target");
    (handle, service, entry, peer)
}

async fn retire_exact(
    handle: &MobHandle,
    entry: &RosterEntry,
) -> tokio::sync::oneshot::Receiver<Result<(), MobError>> {
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let (admission_tx, _admission_rx) = tokio::sync::watch::channel(false);
    handle
        .command_tx
        .send(crate::runtime::scope_gate::RoutedMobCommand {
            authority: handle.command_authority.clone(),
            cmd: MobCommand::Retire {
                agent_identity: entry.agent_identity.clone(),
                expected_incarnation: RetireMemberIncarnation {
                    agent_identity: entry.agent_identity.clone(),
                    agent_runtime_id: entry.agent_runtime_id.clone(),
                    generation: entry.generation,
                    fence_token: entry.fence_token,
                    member_ref: entry.member_ref.clone(),
                },
                deadline: Instant::now() + Duration::from_secs(15),
                admission_tx,
                reply_tx,
            },
        })
        .await
        .expect("send exact retirement");
    reply_rx
}

async fn wait_for_retirement_gate(gate: &TestRuntimeControlBarrier) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while gate.boundary_calls.load(Ordering::Acquire) == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("retirement entered the exact session gate");
}

async fn archive_gate(service: &MockSessionService, entry: &RosterEntry) -> ReleaseRetirementGate {
    let gate = Arc::new(TestRuntimeControlBarrier::new());
    service.retirement_archive_gates.write().await.insert(
        entry.bridge_session_id().expect("session").clone(),
        gate.clone(),
    );
    ReleaseRetirementGate(gate)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn blocked_pending_spawn_cleanup_keeps_queries_and_peer_turns_live() {
    let (handle, service, entry, peer) = retirement_fixture().await;
    let pending = Session::new();
    let pending_id = pending.id().clone();
    service
        .create_session(CreateSessionRequest {
            injected_context: Vec::new(),
            model: "claude-sonnet-4-5".to_string(),
            prompt: ContentInput::from("retirement pending-spawn cleanup"),
            system_prompt: meerkat_core::SystemPromptOverride::Inherit,
            max_tokens: None,
            event_tx: None,
            build: Some(meerkat_core::service::SessionBuildOptions {
                resume_session: Some(pending),
                comms_name: Some(test_comms_name("lead", "retirement-pending-cleanup")),
                ..Default::default()
            }),
            initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
            labels: None,
        })
        .await
        .expect("create exact pending provision");
    handle
        .debug_stage_pending_spawn_for_retire(
            entry.agent_identity.clone(),
            pending_id.clone(),
            meerkat_core::ops::OperationId::new(),
        )
        .await
        .expect("stage exact pending provision cancellation");
    let gate = ReleaseRetirementGate(Arc::new(TestRuntimeControlBarrier::new()));
    service
        .retirement_archive_gates
        .write()
        .await
        .insert(pending_id.clone(), gate.0.clone());
    let retiring = retire_exact(&handle, &entry).await;
    wait_for_retirement_gate(&gate.0).await;
    let phase = tokio::time::timeout(Duration::from_millis(500), handle.status()).await;
    let peer_handle = handle.member(&peer).await.expect("peer");
    let peer_turn = tokio::time::timeout(
        Duration::from_secs(2),
        peer_handle.internal_turn("peer executes while predecessor pending provision is aborted"),
    )
    .await;
    drop(gate);
    let retired = tokio::time::timeout(Duration::from_secs(10), retiring).await;
    handle.shutdown().await.expect("shutdown");
    assert_eq!(
        phase
            .expect("QueryPhase during pending cleanup")
            .expect("phase"),
        MobState::Running
    );
    peer_turn
        .expect("peer turn during pending cleanup")
        .expect("peer completed");
    retired
        .expect("retire settles")
        .expect("retire observer")
        .expect("retired");
    assert_eq!(service.archive_call_count(&pending_id).await, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn blocked_archive_keeps_queries_and_peer_turns_live_and_orders_reload() {
    let (handle, service, entry, peer) = retirement_fixture().await;
    let gate = archive_gate(&service, &entry).await;
    let retiring = retire_exact(&handle, &entry).await;
    wait_for_retirement_gate(&gate.0).await;

    let phase = tokio::time::timeout(Duration::from_millis(500), handle.status()).await;
    let peer_handle = handle.member(&peer).await.expect("peer");
    let peer_turn = tokio::time::timeout(
        Duration::from_secs(2),
        peer_handle.internal_turn("peer turn while archive is blocked"),
    )
    .await;
    let mut reload = tokio::spawn({
        let handle = handle.clone();
        let identity = entry.agent_identity.clone();
        async move { handle.reload_member_registration(&identity).await }
    });
    let reload_early = tokio::time::timeout(Duration::from_millis(100), &mut reload).await;
    let reload_completed_early = reload_early.is_ok();
    drop(gate);
    let retired = tokio::time::timeout(Duration::from_secs(10), retiring).await;
    let reloaded = match reload_early {
        Ok(result) => result,
        Err(_) => tokio::time::timeout(Duration::from_secs(10), reload)
            .await
            .expect("reload settles"),
    };
    handle.shutdown().await.expect("shutdown");

    assert_eq!(
        phase.expect("QueryPhase is independent").expect("phase"),
        MobState::Running
    );
    peer_turn
        .expect("another member executes during archive")
        .expect("peer turn");
    assert!(
        !reload_completed_early,
        "reload must not race an admitted archive"
    );
    retired
        .expect("retire settles")
        .expect("retire observer")
        .expect("retired");
    assert!(
        reloaded.expect("reload task").is_err(),
        "reload cannot revive the retired incarnation"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn blocked_quiesce_keeps_queries_and_actual_peer_turns_live() {
    let (handle, service, entry, peer) = retirement_fixture().await;
    service.set_start_turn_delay_ms(600_000);
    let baseline = service.start_turn_call_count();
    handle
        .member(&entry.agent_identity)
        .await
        .expect("target")
        .send("hold this exact provider run", HandlingMode::Queue)
        .await
        .expect("turn admitted");
    wait_for_start_turn_call_count(&service, baseline + 1, "target provider entered").await;
    let gate = ReleaseRetirementGate(Arc::new(TestRuntimeControlBarrier::new()));
    service
        .retirement_runtime_control_gates
        .write()
        .await
        .insert(
            entry.bridge_session_id().expect("session").clone(),
            gate.0.clone(),
        );
    let retiring = retire_exact(&handle, &entry).await;
    wait_for_retirement_gate(&gate.0).await;
    service.set_start_turn_delay_ms(0);
    let phase = tokio::time::timeout(Duration::from_millis(500), handle.status()).await;
    let peer_handle = handle.member(&peer).await.expect("peer");
    let peer_turn = tokio::time::timeout(
        Duration::from_secs(2),
        peer_handle.internal_turn("peer turn while exact runtime quiescence is blocked"),
    )
    .await;
    drop(gate);
    let retired = tokio::time::timeout(Duration::from_secs(10), retiring).await;
    handle.shutdown().await.expect("shutdown");
    assert_eq!(
        phase.expect("QueryPhase during quiesce").expect("phase"),
        MobState::Running
    );
    peer_turn
        .expect("peer executes during quiesce")
        .expect("peer turn");
    retired
        .expect("retire settles")
        .expect("retire observer")
        .expect("retired");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn late_archive_failure_retains_exact_incarnation_and_respawn_waits() {
    let (handle, service, entry, _) = retirement_fixture().await;
    let gate = archive_gate(&service, &entry).await;
    let retiring = retire_exact(&handle, &entry).await;
    wait_for_retirement_gate(&gate.0).await;
    let session_id = entry.bridge_session_id().expect("session").clone();
    service
        .archive_fail_sessions
        .write()
        .await
        .insert(session_id.clone());
    let mut respawn = tokio::spawn({
        let handle = handle.clone();
        let identity = entry.agent_identity.clone();
        async move { handle.respawn(identity, None).await }
    });
    let respawn_early = tokio::time::timeout(Duration::from_millis(100), &mut respawn).await;
    let respawn_completed_early = respawn_early.is_ok();
    drop(gate);
    let retired = tokio::time::timeout(Duration::from_secs(10), retiring)
        .await
        .expect("retire settles")
        .expect("retire observer");
    let respawned = match respawn_early {
        Ok(result) => result,
        Err(_) => tokio::time::timeout(Duration::from_secs(10), respawn)
            .await
            .expect("respawn settles"),
    };
    let retained = handle
        .get_member(&entry.agent_identity)
        .await
        .expect("roster")
        .expect("retained");
    service
        .archive_fail_sessions
        .write()
        .await
        .remove(&session_id);
    retire_exact(&handle, &entry)
        .await
        .await
        .expect("retry observer")
        .expect("retry");
    handle.shutdown().await.expect("shutdown");
    assert!(!respawn_completed_early);
    assert!(retired.is_err());
    assert!(respawned.expect("respawn task").is_err());
    assert_eq!(retained.agent_runtime_id, entry.agent_runtime_id);
    assert_eq!(retained.fence_token, entry.fence_token);
    assert_eq!(retained.member_ref, entry.member_ref);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn archive_observer_timeout_does_not_cancel_owned_retirement() {
    let (handle, service, entry, _) = retirement_fixture().await;
    let gate = archive_gate(&service, &entry).await;
    let retiring = retire_exact(&handle, &entry).await;
    wait_for_retirement_gate(&gate.0).await;
    assert!(
        tokio::time::timeout(Duration::from_millis(50), retiring)
            .await
            .is_err()
    );
    let retained = handle
        .get_member(&entry.agent_identity)
        .await
        .expect("roster")
        .expect("retained");
    assert_eq!(retained.member_ref, entry.member_ref);
    drop(gate);
    tokio::time::timeout(Duration::from_secs(10), async {
        while handle
            .get_member(&entry.agent_identity)
            .await
            .expect("roster")
            .is_some()
        {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("owned retirement finishes without the observer");
    assert_eq!(
        service
            .archive_call_count(entry.bridge_session_id().expect("session"))
            .await,
        1
    );
    handle.shutdown().await.expect("shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn admitted_turn_completion_wait_does_not_block_retirement_cancellation() {
    let (handle, service, entry, _) = retirement_fixture().await;
    service.set_start_turn_delay_ms(600_000);
    let baseline = service.start_turn_call_count();
    let turn = tokio::spawn({
        let member = handle.member(&entry.agent_identity).await.expect("member");
        async move {
            member
                .internal_turn("hold an admitted TurnCompleted request")
                .await
        }
    });
    wait_for_start_turn_call_count(&service, baseline + 1, "provider turn entered").await;
    let mut retirement = retire_exact(&handle, &entry).await;
    let early = tokio::time::timeout(Duration::from_secs(3), &mut retirement).await;
    let retired_without_test_interrupt = early.is_ok();
    let result = match early {
        Ok(result) => result,
        Err(_) => {
            let _ = SessionService::interrupt(
                service.as_ref(),
                entry.bridge_session_id().expect("session"),
            )
            .await;
            tokio::time::timeout(Duration::from_secs(10), retirement)
                .await
                .expect("retirement settles after test rescue")
        }
    };
    let turn_result = tokio::time::timeout(Duration::from_secs(5), turn)
        .await
        .expect("turn settles")
        .expect("turn task");
    handle.shutdown().await.expect("shutdown");
    assert!(
        retired_without_test_interrupt,
        "the admission lane must not hold Retire behind a turn's terminal-completion observer",
    );
    result
        .expect("retire observer")
        .expect("retirement cancels the admitted run");
    assert!(
        turn_result.is_err(),
        "the wedged run must be cancelled, not reported completed"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn local_respawn_archive_keeps_peer_work_live_and_preserves_exact_origin() {
    let (handle, service, old, peer) = retirement_fixture().await;
    let gate = archive_gate(&service, &old).await;
    let respawn = tokio::spawn({
        let handle = handle.clone();
        let identity = old.agent_identity.clone();
        async move { handle.respawn(identity, None).await }
    });
    wait_for_retirement_gate(&gate.0).await;
    let phase = tokio::time::timeout(Duration::from_millis(500), handle.status()).await;
    let peer_handle = handle.member(&peer).await.expect("peer");
    let peer_turn = tokio::time::timeout(
        Duration::from_secs(2),
        peer_handle.internal_turn("peer executes during local respawn archive"),
    )
    .await;
    drop(gate);
    let receipt = tokio::time::timeout(Duration::from_secs(10), respawn)
        .await
        .expect("respawn settles")
        .expect("respawn task")
        .expect("respawn");
    let successor = handle
        .get_member(&old.agent_identity)
        .await
        .expect("roster")
        .expect("successor");
    handle.shutdown().await.expect("shutdown");
    assert_eq!(
        phase.expect("QueryPhase during respawn").expect("phase"),
        MobState::Running
    );
    peer_turn
        .expect("peer work does not wait for respawn")
        .expect("peer turn");
    assert_eq!(receipt.previous_fence_token, old.fence_token);
    assert_eq!(receipt.agent_runtime_id, successor.agent_runtime_id);
    assert_eq!(receipt.fence_token, successor.fence_token);
    assert_ne!(successor.member_ref, old.member_ref);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn old_retirement_incarnation_cannot_archive_respawned_successor() {
    let (handle, service, old, _) = retirement_fixture().await;
    handle
        .respawn(old.agent_identity.clone(), None)
        .await
        .expect("respawn");
    let successor = handle
        .get_member(&old.agent_identity)
        .await
        .expect("roster")
        .expect("successor");
    assert_ne!(successor.agent_runtime_id, old.agent_runtime_id);
    let stale = retire_exact(&handle, &old)
        .await
        .await
        .expect("stale observer");
    assert!(matches!(
        stale,
        Err(MobError::StaleMemberOperatorAuthority { .. })
    ));
    assert_eq!(
        service
            .archive_call_count(successor.bridge_session_id().expect("successor session"))
            .await,
        0
    );
    handle
        .member(&successor.agent_identity)
        .await
        .expect("successor handle")
        .internal_turn("successor still executes")
        .await
        .expect("successor turn");
    handle.shutdown().await.expect("shutdown");
}
