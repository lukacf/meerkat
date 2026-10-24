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
        content_attribution: crate::mob_machine::WorkContentAttribution::Conversational,
        governed_input: None,
        expected_session_id: None,
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

/// Request a Shutdown behind everything already in the actor's mailbox, then
/// round-trip a status read behind it: the actor serves the read after the
/// Shutdown, so a deferred Shutdown leaves the read answered (and not
/// admitted), while an admitted one answers it from its drain.
async fn request_shutdown_then_read(
    handle: &MobHandle,
) -> (
    tokio::sync::oneshot::Receiver<Result<(), MobError>>,
    Result<MobState, MobError>,
) {
    let shutdown = handle
        .enqueue_actor_command_for_test(|reply_tx| crate::runtime::state::MobCommand::Shutdown {
            deadline: None,
            queued_inputs: crate::runtime::QueuedInputsOnShutdown::Stop,
            reply_tx,
        })
        .await
        .expect("enqueue the Shutdown");
    let read = tokio::time::timeout(Duration::from_secs(10), handle.status())
        .await
        .expect("the actor answers the read");
    (shutdown, read)
}

/// A Shutdown is admitted only once no lane holds submit-work pump state and
/// no member effect runs, so a lane's revival of its submission never meets a
/// parked Shutdown. A held pump preflight is both pump state and a running
/// member effect: either guard alone defers this Shutdown, and the two
/// guarantee it together. The pump dispatches the submission once, before
/// the Shutdown's admission, no member work reaches the parked-Shutdown
/// refusal, and once the Shutdown commits a request finds the command channel
/// closed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_is_admitted_only_after_a_submit_work_pump_preflight_settles() {
    let (controlling, host, responder, entry) = fixture("pump-preflight-shutdown").await;
    let observations = Arc::clone(&controlling.handle.lifecycle_observations);
    let held = MemberEventPumpManager::hold_completion_preparation_for_test(
        controlling.mob_id.clone(),
        entry.agent_identity.clone(),
    );
    let reply = submit(
        &controlling.handle,
        &entry,
        meerkat_core::interaction::InteractionId(Uuid::new_v4()),
        "submitted before the shutdown",
    )
    .await;
    tokio::time::timeout(Duration::from_secs(5), held.entered())
        .await
        .expect("the pump preflight reached the hold");

    let mut task = controlling.handle.actor_task_watch();
    let (shutdown, read) = request_shutdown_then_read(&controlling.handle).await;
    assert_eq!(
        read.expect("a deferred Shutdown leaves the actor serving reads"),
        MobState::Running
    );
    assert!(
        !*observations.shutdown_admitted.borrow(),
        "a Shutdown was admitted while a submit-work pump preflight was held"
    );
    assert_eq!(*observations.submit_work_pump_dispatches.borrow(), 0);

    drop(held);
    tokio::time::timeout(Duration::from_secs(15), shutdown)
        .await
        .expect("the Shutdown answers")
        .expect("shutdown reply")
        .expect("the Shutdown finishes");
    let _ = tokio::time::timeout(Duration::from_secs(15), reply)
        .await
        .expect("the submission is answered");
    assert_eq!(
        *observations.submit_work_pump_dispatches.borrow(),
        1,
        "the pump dispatched its submission once"
    );
    assert_eq!(
        *observations.parked_shutdown_member_work_refusals.borrow(),
        0,
        "no member work reached a parked Shutdown"
    );
    assert_eq!(
        task.wait().await,
        crate::runtime::MobActorTaskWait::Returned
    );
    match controlling.handle.stop().await {
        Err(MobError::ActorCommandChannelClosed) => {}
        other => panic!("expected a closed command channel after the commit, got {other:?}"),
    }
    responder.shutdown();
    host.shutdown();
}

/// The lane-pump guard on its own: a submission parked behind the member's
/// held turn admission is submit-work pump state while no member effect
/// runs, and the Shutdown still waits for it. Narrowing that guard alone lets
/// this Shutdown in.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_waits_for_a_parked_submit_work_pump_without_a_member_effect() {
    let (controlling, host, responder, entry) = fixture("pump-parked-shutdown").await;
    let observations = Arc::clone(&controlling.handle.lifecycle_observations);
    let (turn_entered, release_turn) = MobHandle::arm_member_turn_admission_test_gate(
        entry.agent_identity.clone(),
        crate::MemberTurnAdmissionTestStage::BeforeRuntimeAdmission,
    );
    let _first = submit(
        &controlling.handle,
        &entry,
        meerkat_core::interaction::InteractionId(Uuid::new_v4()),
        "held at its turn admission",
    )
    .await;
    tokio::time::timeout(Duration::from_secs(10), turn_entered)
        .await
        .expect("the first submission reached its turn admission")
        .expect("turn admission gate");
    assert_eq!(*observations.submit_work_pump_dispatches.borrow(), 1);
    let mut second = submit(
        &controlling.handle,
        &entry,
        meerkat_core::interaction::InteractionId(Uuid::new_v4()),
        "parked behind the held turn admission",
    )
    .await;

    let (shutdown, read) = request_shutdown_then_read(&controlling.handle).await;
    assert_eq!(
        read.expect("a deferred Shutdown leaves the actor serving reads"),
        MobState::Running
    );
    assert!(
        !*observations.shutdown_admitted.borrow(),
        "a Shutdown was admitted while a parked submission held pump state"
    );
    assert!(
        matches!(
            second.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ),
        "the second submission is parked, not answered"
    );
    assert_eq!(
        *observations.submit_work_pump_dispatches.borrow(),
        1,
        "the parked submission has not started its preflight"
    );

    drop(release_turn);
    tokio::time::timeout(Duration::from_secs(30), shutdown)
        .await
        .expect("the Shutdown answers")
        .expect("shutdown reply")
        .expect("the Shutdown finishes");
    assert_eq!(
        *observations.submit_work_pump_dispatches.borrow(),
        2,
        "the parked submission dispatched once, before the Shutdown's admission"
    );
    assert_eq!(
        *observations.parked_shutdown_member_work_refusals.borrow(),
        0,
        "no member work reached a parked Shutdown"
    );
    responder.shutdown();
    host.shutdown();
}
