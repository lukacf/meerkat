use super::*;

use super::placement_support as support;

use crate::runtime::event_pump::MemberEventPumpManager;

async fn fixture(
    label: &str,
) -> (
    support::ControllingMob,
    support::ScriptedHostPeer,
    support::ScriptedMemberTurnResponder,
    RosterEntry,
) {
    let host = support::spawn_scripted_host_peer(label).await;
    let endpoint =
        Arc::new(support::spawn_peer_comms_endpoint(&format!("{label}-member"), true, None).await);
    let responder = support::spawn_scripted_member_turn_responder(endpoint.clone());
    responder.complete_plain_deliveries_via_event_pump();
    host.script_member_identity("remote", support::member_identity_of(&endpoint));
    host.bind_member_endpoint("remote", endpoint);
    let controlling = support::create_controlling_mob(label).await;
    let report = controlling.bind_scripted(&host).await;
    controlling
        .spawn_placed("worker", "remote", &report.host_id)
        .await
        .expect("placed member");
    let entry = controlling
        .handle
        .get_member(&AgentIdentity::from("remote"))
        .await
        .expect("member projection")
        .expect("remote exists");
    responder.set_expected_incarnation(entry.generation.get(), entry.fence_token.get());
    (controlling, host, responder, entry)
}

async fn submit(
    handle: &MobHandle,
    entry: &RosterEntry,
    interaction_id: meerkat_core::interaction::InteractionId,
    content: &str,
) -> tokio::sync::oneshot::Receiver<Result<(), MobError>> {
    let (reply_tx, reply) = tokio::sync::oneshot::channel();
    let payload = Box::new(crate::runtime::state::SubmitWorkPayload {
        runtime_id: entry.agent_runtime_id.clone(),
        fence_token: entry.fence_token,
        work_ref: WorkRef::new(),
        content: content.into(),
        origin: WorkOrigin::Internal,
        system_prompt: None,
        injected_context: Vec::new(),
        interaction_id: Some(interaction_id),
        objective_id: None,
        handling_mode: HandlingMode::Queue,
        external_delivery_identity: None,
        turn_metadata: None,
        event_tx: None,
        completion_tx: None,
        bounded_result_spec: None,
        llm_identity_applied_tx: None,
        ack_mode: crate::mob_machine::SubmitWorkAckMode::TurnCompleted,
    });
    handle
        .command_tx
        .send(crate::runtime::scope_gate::RoutedMobCommand {
            authority: handle.command_authority.clone(),
            cmd: crate::runtime::state::MobCommand::SubmitWork { payload, reply_tx },
        })
        .await
        .expect("submit original scoped command");
    reply
}

async fn reload(
    handle: &MobHandle,
    identity: &AgentIdentity,
) -> tokio::sync::oneshot::Receiver<Result<crate::runtime::handle::MemberReloadOutcome, MobError>> {
    let (reply_tx, reply) = tokio::sync::oneshot::channel();
    handle
        .command_tx
        .send(crate::runtime::scope_gate::RoutedMobCommand {
            authority: handle.command_authority.clone(),
            cmd: crate::runtime::state::MobCommand::ReloadMemberRegistration {
                agent_identity: identity.clone(),
                deadline: meerkat_core::time_compat::Instant::now() + Duration::from_secs(20),
                reply_tx,
            },
        })
        .await
        .expect("enqueue later same-member control");
    reply
}

async fn assert_no_record(controlling: &support::ControllingMob) {
    assert!(
        !controlling
            .storage_events
            .replay_all()
            .await
            .expect("read durable records")
            .iter()
            .any(|event| matches!(
                &event.kind,
                MobEventKind::PlacedCompletionObligationRecorded { obligation }
                    if obligation.agent_identity.as_str() == "remote"
            ))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn submit_work_pump_preflight_preserves_fifo_and_actor_progress() {
    let (controlling, host, responder, entry) = fixture("pump-preflight-fifo").await;
    controlling
        .handle
        .spawn_spec(SpawnMemberSpec::new("worker", "unrelated"))
        .await
        .expect("unrelated local member");
    let held = MemberEventPumpManager::hold_completion_preparation_for_test(
        controlling.mob_id.clone(),
        entry.agent_identity.clone(),
    );
    let first_id = meerkat_core::interaction::InteractionId(Uuid::new_v4());
    let second_id = meerkat_core::interaction::InteractionId(Uuid::new_v4());
    let mut first = submit(&controlling.handle, &entry, first_id, "first").await;
    tokio::time::timeout(Duration::from_secs(5), held.entered())
        .await
        .expect("owned pump ensure reached the hold");
    let mut second = submit(&controlling.handle, &entry, second_id, "second").await;
    let mut later_reload = reload(&controlling.handle, &entry.agent_identity).await;
    tokio::time::timeout(Duration::from_secs(2), controlling.handle.status())
        .await
        .expect("QueryPhase progresses past the queued commands")
        .expect("phase");
    tokio::time::timeout(Duration::from_secs(5), async {
        controlling
            .handle
            .member(&AgentIdentity::from("unrelated"))
            .await
            .expect("unrelated member")
            .internal_turn("progress while pump ensure is held")
            .await
    })
    .await
    .expect("unrelated member remains responsive")
    .expect("unrelated turn");
    assert!(matches!(
        first.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));
    assert!(matches!(
        second.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));
    assert!(matches!(
        later_reload.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    ));
    assert!(
        responder.received_deliveries().is_empty(),
        "no premature remote send"
    );
    assert_no_record(&controlling).await;

    drop(held);
    tokio::time::timeout(Duration::from_secs(15), first)
        .await
        .expect("first completes")
        .expect("first reply")
        .expect("first exact terminal");
    tokio::time::timeout(Duration::from_secs(15), second)
        .await
        .expect("second completes")
        .expect("second reply")
        .expect("second exact terminal");
    let _ = tokio::time::timeout(Duration::from_secs(5), later_reload)
        .await
        .expect("later reload is released")
        .expect("reload reply");
    let deliveries = responder.received_deliveries();
    assert_eq!(deliveries.len(), 2);
    assert_eq!(deliveries[0].input_id, first_id.0.to_string());
    assert_eq!(deliveries[1].input_id, second_id.0.to_string());
    responder.shutdown();
    host.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn submit_work_pump_preflight_caller_drop_releases_tap_and_continuation() {
    let (controlling, host, responder, entry) = fixture("pump-preflight-dropped").await;
    let held = MemberEventPumpManager::hold_completion_preparation_for_test(
        controlling.mob_id.clone(),
        entry.agent_identity.clone(),
    );
    let reply = submit(
        &controlling.handle,
        &entry,
        meerkat_core::interaction::InteractionId(Uuid::new_v4()),
        "abandoned",
    )
    .await;
    tokio::time::timeout(Duration::from_secs(5), held.entered())
        .await
        .expect("held ensure");
    let manager = held.manager().expect("exact pump owner");
    drop(reply);
    let later = reload(&controlling.handle, &entry.agent_identity).await;
    drop(held);
    let _ = tokio::time::timeout(Duration::from_secs(10), later)
        .await
        .expect("later control proves the preflight released its FIFO")
        .expect("reload reply");
    assert_eq!(manager.live_tap_count_for_test(&entry.agent_identity), 0);
    assert!(responder.received_deliveries().is_empty());
    assert_no_record(&controlling).await;
    responder.shutdown();
    host.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn submit_work_pump_preflight_closed_manager_refuses_before_record_or_send() {
    let (controlling, host, responder, entry) = fixture("pump-preflight-closed").await;
    let held = MemberEventPumpManager::hold_completion_preparation_for_test(
        controlling.mob_id.clone(),
        entry.agent_identity.clone(),
    );
    let reply = submit(
        &controlling.handle,
        &entry,
        meerkat_core::interaction::InteractionId(Uuid::new_v4()),
        "must not be admitted",
    )
    .await;
    tokio::time::timeout(Duration::from_secs(5), held.entered())
        .await
        .expect("held ensure");
    let manager = held.manager().expect("exact pump owner");
    manager.stop_all_and_join().await;
    drop(held);
    let result = tokio::time::timeout(Duration::from_secs(10), reply)
        .await
        .expect("closed owner returns")
        .expect("typed reply")
        .expect_err("closed pump is not ready");
    assert!(matches!(result, MobError::Internal(reason) if reason.contains("pump")));
    assert_eq!(manager.live_tap_count_for_test(&entry.agent_identity), 0);
    assert!(responder.received_deliveries().is_empty());
    assert_no_record(&controlling).await;
    responder.shutdown();
    host.shutdown();
}
