//! Reload and turn delivery share one physical member-admission lane.

use super::*;
use crate::machines::mob_machine as mob_dsl;
use crate::runtime::handle::MemberReloadOutcome;
use tokio::sync::oneshot;

type ReloadReply = oneshot::Receiver<Result<MemberReloadOutcome, MobError>>;

async fn blocked_reload(
    mob: &IsolationMob,
    budget: Duration,
) -> (ReloadReply, oneshot::Sender<()>) {
    mob.degrade_member(0).await;
    mob.store.fail_commit(mob.session(0), false);
    let (published, release) = mob
        .adapter
        .arm_reload_required_discard_after_successor_publication_test_hook(mob.session(0).clone());
    let reply = mob
        .handle
        .enqueue_actor_command_for_test(|reply_tx| MobCommand::ReloadMemberRegistration {
            agent_identity: mob.member(0).clone(),
            deadline: Instant::now() + budget,
            reply_tx,
        })
        .await
        .expect("enqueue reload");
    tokio::time::timeout(Duration::from_secs(3), published)
        .await
        .expect("reload reaches exact successor publication")
        .expect("publication hook retained");
    (reply, release)
}

async fn wait_for_parked(mob: &IsolationMob, depth: usize) {
    wait_until(
        "same-member lane backlog",
        Duration::from_secs(2),
        || async {
            mob.handle
                .member_admission_backlog()
                .parked
                .get(mob.member(0))
                .copied()
                == Some(depth)
        },
    )
    .await;
}

async fn blocked_before_warm_claim(mob: &IsolationMob) -> (ReloadReply, oneshot::Sender<()>) {
    mob.degrade_member(0).await;
    mob.store.fail_commit(mob.session(0), false);
    let (entered, release) =
        crate::runtime::provisioner::arm_reload_before_warm_claim_for_test(mob.session(0).clone());
    let reply = mob
        .handle
        .enqueue_actor_command_for_test(|reply_tx| MobCommand::ReloadMemberRegistration {
            agent_identity: mob.member(0).clone(),
            deadline: Instant::now() + Duration::from_secs(15),
            reply_tx,
        })
        .await
        .expect("enqueue reload");
    tokio::time::timeout(Duration::from_secs(3), entered)
        .await
        .expect("cleanup settles before claim barrier")
        .expect("barrier retained");
    assert!(mob.service.actor_registry.current(mob.session(0)).is_none());
    (reply, release)
}

async fn peer_progresses(mob: &IsolationMob) {
    mob.probe(Duration::from_secs(1))
        .await
        .expect("reload must not block the actor");
    tokio::time::timeout(
        Duration::from_secs(2),
        internal_turn_task(&mob.handle, mob.member(1), "peer progresses".to_string()),
    )
    .await
    .expect("peer completes while reload is blocked")
    .expect("peer task")
    .expect("peer receipt");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reload_waits_behind_an_existing_same_member_admission() {
    let mob = create_isolation_mob(2).await;
    mob.store.park_admissions(mob.session(0));
    let delivery = internal_turn_task(&mob.handle, mob.member(0), "before reload".to_string());
    wait_until(
        "first admission enters store",
        Duration::from_secs(2),
        || async { mob.store.parked_admission_arrivals(mob.session(0)) == 1 },
    )
    .await;
    let mut reply = mob
        .handle
        .enqueue_actor_command_for_test(|reply_tx| MobCommand::ReloadMemberRegistration {
            agent_identity: mob.member(0).clone(),
            deadline: Instant::now() + Duration::from_secs(15),
            reply_tx,
        })
        .await
        .expect("enqueue reload behind admission");
    wait_for_parked(&mob, 1).await;
    peer_progresses(&mob).await;
    assert!(matches!(
        reply.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    mob.store.release_admissions();
    tokio::time::timeout(Duration::from_secs(5), delivery)
        .await
        .expect("first admission completes")
        .expect("delivery task")
        .expect("delivery receipt");
    let outcome = tokio::time::timeout(Duration::from_secs(5), reply)
        .await
        .expect("queued reload completes")
        .expect("reload reply")
        .expect("healthy reload succeeds");
    assert_eq!(outcome.disposition, MemberReloadDisposition::NotDegraded);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn blocked_reload_serializes_delivery_and_reload_without_delaying_peers() {
    let mob = create_isolation_mob(2).await;
    let (reply, release) = blocked_reload(&mob, Duration::from_secs(15)).await;
    let predecessor_actor = mob
        .service
        .actor_registry
        .current(mob.session(0))
        .expect("runtime-only cleanup retains the predecessor actor");
    let cold_successor = mob
        .adapter
        .current_session_registration_witness(mob.session(0))
        .await
        .expect("exact cold successor");
    let delivery = internal_turn_task(&mob.handle, mob.member(0), "after reload".to_string());
    wait_for_parked(&mob, 1).await;
    let second_reload = mob
        .handle
        .enqueue_actor_command_for_test(|reply_tx| MobCommand::ReloadMemberRegistration {
            agent_identity: mob.member(0).clone(),
            deadline: Instant::now() + Duration::from_secs(15),
            reply_tx,
        })
        .await
        .expect("enqueue second reload");
    wait_for_parked(&mob, 2).await;
    peer_progresses(&mob).await;
    assert!(!delivery.is_finished(), "delivery cannot overtake reload");
    release.send(()).expect("release owned discard");
    let outcome = tokio::time::timeout(Duration::from_secs(5), reply)
        .await
        .expect("reload settles")
        .expect("reload reply")
        .expect("reload succeeds");
    assert_eq!(outcome.disposition, MemberReloadDisposition::Discarded);
    tokio::time::timeout(Duration::from_secs(5), delivery)
        .await
        .expect("parked delivery finishes")
        .expect("delivery task")
        .expect("delivery receipt");
    let again = tokio::time::timeout(Duration::from_secs(5), second_reload)
        .await
        .expect("second reload settles")
        .expect("second reply")
        .expect("second reload succeeds");
    assert_eq!(again.disposition, MemberReloadDisposition::NotDegraded);
    assert_eq!(
        mob.adapter
            .current_session_registration_witness(mob.session(0))
            .await,
        Some(cold_successor),
        "executor publication and queued work retain the exact recovered registration"
    );
    // The failed boundary left its input durable but unfinished; reload
    // replays it before admitting the newly queued turn.
    assert_eq!(
        mob.executed_prompts(0).await,
        ["degrade me", "degrade me", "after reload"]
    );
    let restored_actor = mob
        .service
        .actor_registry
        .current(mob.session(0))
        .expect("restoration publishes a live actor");
    assert_ne!(predecessor_actor, restored_actor);
    assert!(
        mob.adapter
            .current_executor_attachment_witness(mob.session(0))
            .await
            .is_some(),
        "successful restoration requires a published executor, not only an actor"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reload_timeout_keeps_lane_until_late_owned_settlement() {
    let mob = create_isolation_mob(2).await;
    let (reply, release) = blocked_reload(&mob, Duration::from_secs(2)).await;
    let delivery = internal_turn_task(&mob.handle, mob.member(0), "after timeout".to_string());
    wait_for_parked(&mob, 1).await;
    let error = tokio::time::timeout(Duration::from_secs(3), reply)
        .await
        .expect("bounded observation answers before effect settlement")
        .expect("reload reply channel")
        .expect_err("reload observation times out");
    assert!(matches!(
        error,
        MobError::MemberReloadTimedOut {
            stage: "durability_reload_discard",
            ..
        }
    ));
    peer_progresses(&mob).await;
    assert!(!delivery.is_finished(), "timeout is not lane settlement");
    wait_for_parked(&mob, 1).await;
    release.send(()).expect("release late effect");
    tokio::time::timeout(Duration::from_secs(5), delivery)
        .await
        .expect("late settlement releases queued delivery")
        .expect("delivery task")
        .expect("delivery succeeds after late revival");
    assert_eq!(
        mob.executed_prompts(0).await,
        ["degrade me", "degrade me", "after timeout"]
    );
    assert!(
        mob.adapter
            .current_executor_attachment_witness(mob.session(0))
            .await
            .is_some(),
        "the observer timeout must not cancel executor re-materialization"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dropped_reload_caller_keeps_custody_and_queued_abandoned_reload_is_skipped() {
    let mob = create_isolation_mob(2).await;
    let (reply, release) = blocked_reload(&mob, Duration::from_secs(15)).await;
    drop(reply);
    let abandoned = mob
        .handle
        .enqueue_actor_command_for_test(|reply_tx| MobCommand::ReloadMemberRegistration {
            agent_identity: mob.member(0).clone(),
            deadline: Instant::now() + Duration::from_secs(15),
            reply_tx,
        })
        .await
        .expect("enqueue abandoned reload");
    wait_for_parked(&mob, 1).await;
    drop(abandoned);
    let delivery = internal_turn_task(&mob.handle, mob.member(0), "after caller drop".to_string());
    wait_for_parked(&mob, 2).await;
    peer_progresses(&mob).await;
    assert!(!delivery.is_finished(), "caller drop is not cancellation");
    release.send(()).expect("release still-owned reload");
    tokio::time::timeout(Duration::from_secs(5), delivery)
        .await
        .expect("lane skips abandoned queued reload")
        .expect("delivery task")
        .expect("delivery receipt");
    assert_eq!(
        mob.executed_prompts(0).await,
        ["degrade me", "degrade me", "after caller drop"]
    );
    assert_eq!(
        mob.handle
            .member_admission_backlog()
            .reload_invocations
            .get(mob.member(0)),
        Some(&1),
        "the abandoned queued reload must not invoke even a healthy no-op"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn retry_recovers_after_sidecar_cleanup_before_successor_publication_fails() {
    let mob = create_isolation_mob(2).await;
    mob.degrade_member(0).await;
    mob.store.fail_commit(mob.session(0), false);
    let registration = mob
        .adapter
        .current_session_registration_witness(mob.session(0))
        .await
        .expect("degraded registration");
    let actor = mob
        .service
        .actor_registry
        .current(mob.session(0))
        .expect("predecessor actor");
    mob.store
        .fail_ops_load
        .lock()
        .expect("failure switch")
        .insert(LogicalRuntimeId::for_session(mob.session(0)));
    let failure = tokio::time::timeout(
        Duration::from_secs(5),
        mob.handle.reload_member_registration(mob.member(0)),
    )
    .await
    .expect("failed cold preparation settles")
    .expect_err("cold recovery read fails after cleanup");
    assert!(
        failure
            .to_string()
            .contains("injected cold recovery ops read"),
        "{failure}"
    );
    assert_eq!(mob.store.failed_ops_loads.load(Ordering::Relaxed), 1);
    assert_eq!(
        mob.adapter
            .current_session_registration_witness(mob.session(0))
            .await,
        Some(registration),
        "failed cold preparation keeps the exact degraded registration"
    );
    assert!(
        mob.adapter
            .current_executor_attachment_witness(mob.session(0))
            .await
            .is_none(),
        "the predecessor executor and sidecar cleanup preceded the failing read"
    );
    assert_eq!(
        mob.service.actor_registry.current(mob.session(0)),
        Some(actor.clone())
    );
    mob.store
        .fail_ops_load
        .lock()
        .expect("failure switch")
        .clear();
    let recovered = tokio::time::timeout(
        Duration::from_secs(5),
        mob.handle.reload_member_registration(mob.member(0)),
    )
    .await
    .expect("retry settles")
    .expect("retained exact custody repairs the registration");
    assert_eq!(recovered.disposition, MemberReloadDisposition::Discarded);
    assert_ne!(
        mob.service.actor_registry.current(mob.session(0)),
        Some(actor)
    );
    assert!(
        mob.adapter
            .current_executor_attachment_witness(mob.session(0))
            .await
            .is_some()
    );
    tokio::time::timeout(
        Duration::from_secs(5),
        internal_turn_task(
            &mob.handle,
            mob.member(0),
            "after recovery retry".to_string(),
        ),
    )
    .await
    .expect("delivery after retry")
    .expect("delivery task")
    .expect("restored executor accepts delivery");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn late_reload_receipt_cannot_touch_a_successor_registration() {
    let mob = create_isolation_mob(2).await;
    let (reply, release) = blocked_reload(&mob, Duration::from_secs(15)).await;
    let cold = mob
        .adapter
        .current_session_registration_witness(mob.session(0))
        .await
        .expect("cold registration");
    mob.adapter
        .unregister_session_registration_until_terminal_if_current(&cold)
        .await
        .expect("remove exact cold registration");
    mob.adapter
        .register_session(mob.session(0).clone())
        .await
        .expect("publish independent successor");
    let successor = mob
        .adapter
        .current_session_registration_witness(mob.session(0))
        .await
        .expect("independent successor witness");
    assert_ne!(cold, successor);
    let actor_before_stale_completion = mob.service.actor_registry.current(mob.session(0));
    let stale_preparation = mob
        .adapter
        .prepare_local_session_materialization_for_registration(cold.clone())
        .await;
    assert!(
        stale_preparation.is_err(),
        "a stale reload cannot claim the replacement"
    );
    release
        .send(())
        .expect("allow stale reload receipt to arrive");
    let outcome = tokio::time::timeout(Duration::from_secs(5), reply)
        .await
        .expect("stale reload settles")
        .expect("reload reply")
        .expect("stale reload is an inert outcome");
    assert_eq!(outcome.disposition, MemberReloadDisposition::NotCurrent);
    assert_eq!(
        mob.adapter
            .current_session_registration_witness(mob.session(0))
            .await,
        Some(successor),
        "late reload cannot adopt or retire an independently published successor"
    );
    assert_eq!(
        mob.service.actor_registry.current(mob.session(0)),
        actor_before_stale_completion
    );
    peer_progresses(&mob).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn busy_exact_successor_preserves_reload_custody_until_claim_settles() {
    let mob = create_isolation_mob(2).await;
    let (reply, release) = blocked_reload(&mob, Duration::from_secs(15)).await;
    let cold = mob
        .adapter
        .current_session_registration_witness(mob.session(0))
        .await
        .expect("exact cold successor");
    let actor = mob
        .service
        .actor_registry
        .current(mob.session(0))
        .expect("predecessor actor");
    let mut competing = mob
        .adapter
        .prepare_local_session_materialization_for_registration(cold.clone())
        .await
        .expect("legitimate owner reserves the cold registration");
    release.send(()).expect("release old reload completion");
    let result = tokio::time::timeout(Duration::from_secs(5), reply)
        .await
        .expect("busy reload answers")
        .expect("reply channel");
    assert!(matches!(result, Err(MobError::MemberReloadRefused { .. })));
    assert_eq!(
        mob.service.actor_registry.current(mob.session(0)),
        Some(actor.clone())
    );
    assert!(competing.owns_current_materialization_claim().await);
    competing
        .rollback_now()
        .await
        .expect("settle competing claim");
    let outcome = tokio::time::timeout(
        Duration::from_secs(5),
        mob.handle.reload_member_registration(mob.member(0)),
    )
    .await
    .expect("retry finishes")
    .expect("retained successor custody repairs");
    assert_eq!(outcome.disposition, MemberReloadDisposition::Discarded);
    assert_ne!(
        mob.service.actor_registry.current(mob.session(0)),
        Some(actor)
    );
    assert!(
        mob.adapter
            .current_executor_attachment_witness(mob.session(0))
            .await
            .is_some()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn exact_actor_cleanup_failure_retries_the_published_successor() {
    let mob = create_isolation_mob(2).await;
    let (reply, release) = blocked_reload(&mob, Duration::from_secs(15)).await;
    let cold = mob
        .adapter
        .current_session_registration_witness(mob.session(0))
        .await
        .expect("cold successor");
    mob.service
        .discard_actor_failures_remaining
        .store(1, Ordering::Relaxed);
    release.send(()).expect("release first cleanup attempt");
    let result = tokio::time::timeout(Duration::from_secs(5), reply)
        .await
        .expect("cleanup failure answers")
        .expect("reply channel");
    assert!(result.is_err());
    let outcome = tokio::time::timeout(
        Duration::from_secs(5),
        mob.handle.reload_member_registration(mob.member(0)),
    )
    .await
    .expect("retry finishes")
    .expect("retry reuses exact successor");
    assert_eq!(outcome.disposition, MemberReloadDisposition::Discarded);
    assert_eq!(
        mob.adapter
            .current_session_registration_witness(mob.session(0))
            .await,
        Some(cold)
    );
    assert!(
        mob.adapter
            .current_executor_attachment_witness(mob.session(0))
            .await
            .is_some()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn retirement_can_cancel_an_admitted_turn_while_completion_is_waiting() {
    let mob = create_isolation_mob(1).await;
    mob.service.set_start_turn_delay_ms(600_000);
    let baseline = mob.service.start_turn_call_count();
    let completion = internal_turn_task(&mob.handle, mob.member(0), "hold active".to_string());
    wait_until(
        "runtime owns the admitted active turn",
        Duration::from_secs(3),
        || async { mob.service.start_turn_call_count() > baseline },
    )
    .await;
    assert!(!completion.is_finished());
    tokio::time::timeout(
        Duration::from_secs(8),
        mob.handle.retire(mob.member(0).clone()),
    )
    .await
    .expect("retirement is not gated by the admitted turn's completion waiter")
    .expect("retirement cancels the active run");
    let result = tokio::time::timeout(Duration::from_secs(3), completion)
        .await
        .expect("cancelled completion settles")
        .expect("completion task");
    assert!(
        result.is_err(),
        "retirement cannot report a cancelled turn as completed"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn respawn_can_replace_an_admitted_turn_without_waiting_for_execution() {
    let mob = create_isolation_mob(1).await;
    mob.service.set_start_turn_delay_ms(600_000);
    let baseline = mob.service.start_turn_call_count();
    let completion = internal_turn_task(&mob.handle, mob.member(0), "hold for respawn".to_string());
    wait_until(
        "active turn is owned by runtime",
        Duration::from_secs(3),
        || async { mob.service.start_turn_call_count() > baseline },
    )
    .await;
    assert!(!completion.is_finished());
    tokio::time::timeout(
        Duration::from_secs(8),
        mob.handle.respawn(mob.member(0).clone(), None),
    )
    .await
    .expect("respawn is not fenced by terminal observation")
    .expect("respawn replaces the active run");
    assert!(
        tokio::time::timeout(Duration::from_secs(3), completion)
            .await
            .expect("old completion settles")
            .expect("completion task")
            .is_err()
    );
    let status = mob
        .handle
        .member_status(mob.member(0))
        .await
        .expect("successor status");
    assert_ne!(status.current_session_id.as_ref(), Some(mob.session(0)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn competing_warm_claim_is_retryable_without_broken_or_repeated_cleanup() {
    let mob = create_isolation_mob(1).await;
    let (reply, release) = blocked_before_warm_claim(&mob).await;
    let cold = mob
        .adapter
        .current_session_registration_witness(mob.session(0))
        .await
        .expect("cold successor awaiting restoration");
    let mut competing = mob
        .adapter
        .prepare_local_session_materialization_for_registration(cold.clone())
        .await
        .expect("temporary competing claim");
    release.send(()).expect("allow warm claim");
    let result = tokio::time::timeout(Duration::from_secs(5), reply)
        .await
        .expect("busy claim answers")
        .expect("reply");
    assert!(matches!(result, Err(MobError::MemberReloadRefused { .. })));
    assert_ne!(
        mob.handle
            .member_status(mob.member(0))
            .await
            .expect("member status")
            .status,
        crate::runtime::handle::MobMemberStatus::Broken
    );
    assert!(competing.owns_current_materialization_claim().await);
    competing
        .rollback_now()
        .await
        .expect("release temporary claim");
    // Cleanup already succeeded. A repeated cleanup would now fail, proving
    // that retry resumes restoration rather than replaying cleanup/rebinding.
    mob.service
        .discard_actor_failures_remaining
        .store(1, Ordering::Relaxed);
    let restored = tokio::time::timeout(
        Duration::from_secs(5),
        mob.handle.reload_member_registration(mob.member(0)),
    )
    .await
    .expect("retry restores")
    .expect("retry succeeds");
    assert_eq!(restored.disposition, MemberReloadDisposition::Discarded);
    assert_eq!(
        mob.service
            .discard_actor_failures_remaining
            .load(Ordering::Relaxed),
        1
    );
    mob.service
        .discard_actor_failures_remaining
        .store(0, Ordering::Relaxed);
    assert_eq!(
        mob.adapter
            .current_session_registration_witness(mob.session(0))
            .await,
        Some(cold)
    );
    assert!(mob.service.actor_registry.current(mob.session(0)).is_some());
    assert!(
        mob.adapter
            .current_executor_attachment_witness(mob.session(0))
            .await
            .is_some()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn replaced_before_warm_claim_is_inert_and_never_marks_member_broken() {
    let mob = create_isolation_mob(1).await;
    let (reply, release) = blocked_before_warm_claim(&mob).await;
    let cold = mob
        .adapter
        .current_session_registration_witness(mob.session(0))
        .await
        .expect("old cold successor");
    mob.adapter
        .unregister_session_registration_until_terminal_if_current(&cold)
        .await
        .expect("replace exact cold registration");
    mob.adapter
        .register_session(mob.session(0).clone())
        .await
        .expect("register successor");
    let successor = mob
        .adapter
        .current_session_registration_witness(mob.session(0))
        .await;
    let creates = mob.service.session_counter.load(Ordering::Relaxed);
    release.send(()).expect("release stale claim attempt");
    let result = tokio::time::timeout(Duration::from_secs(5), reply)
        .await
        .expect("stale claim answers")
        .expect("reply")
        .expect("inert outcome");
    assert_eq!(result.disposition, MemberReloadDisposition::NotCurrent);
    assert_eq!(
        mob.adapter
            .current_session_registration_witness(mob.session(0))
            .await,
        successor
    );
    assert_eq!(mob.service.session_counter.load(Ordering::Relaxed), creates);
    assert!(mob.service.actor_registry.current(mob.session(0)).is_none());
    assert_ne!(
        mob.handle
            .member_status(mob.member(0))
            .await
            .expect("member status")
            .status,
        crate::runtime::handle::MobMemberStatus::Broken
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn standalone_handoff_waits_for_real_session_command_admission() {
    use crate::runtime::provisioner::{MobProvisioner, SessionBackend};
    let service = Arc::new(meerkat_session::EphemeralSessionService::new(
        PersistentMockBuilder,
        4,
    ));
    let created = SessionService::create_session(
        service.as_ref(),
        CreateSessionRequest {
            injected_context: Vec::new(),
            model: "mock-model".to_string(),
            prompt: ContentInput::from("create"),
            system_prompt: meerkat_core::SystemPromptOverride::Inherit,
            max_tokens: None,
            event_tx: None,
            initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
            build: None,
            labels: None,
        },
    )
    .await
    .expect("create actual standalone service actor");
    let session_id = created.session_id;
    let boundary = service
        .acquire_runtime_turn_finalization_guard(&session_id)
        .await;
    let backend = Arc::new(SessionBackend::new(service.clone(), None, None));
    let (completion_tx, mut completion_rx) = oneshot::channel();
    let admission = tokio::spawn({
        let session_id = session_id.clone();
        async move {
            backend
                .admit_tracked_turn(
                    &MemberRef::from_bridge_session_id(session_id),
                    StartTurnRequest {
                        injected_context: Vec::new(),
                        prompt: ContentInput::from("admit after boundary"),
                        system_prompt: None,
                        event_tx: None,
                        runtime: Default::default(),
                    },
                    completion_tx,
                    None,
                )
                .await
        }
    });
    wait_until(
        "canonical session claim reserved before B",
        Duration::from_secs(3),
        || async {
            SessionService::read(service.as_ref(), &session_id)
                .await
                .is_ok_and(|view| view.state.is_active)
        },
    )
    .await;
    assert!(
        !admission.is_finished(),
        "task scheduling and claim reservation are not command handoff"
    );
    assert!(matches!(
        completion_rx.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    drop(boundary);
    tokio::time::timeout(Duration::from_secs(3), admission)
        .await
        .expect("real admission acknowledged")
        .expect("admission task")
        .expect("admission");
    let completion = tokio::time::timeout(Duration::from_secs(3), completion_rx)
        .await
        .expect("terminal result follows")
        .expect("completion channel")
        .expect("completion");
    assert_eq!(completion.session_id, session_id);
}

#[tokio::test]
async fn standalone_service_without_admission_contract_is_rejected_not_scheduled_success() {
    use crate::runtime::provisioner::{MobProvisioner, SessionBackend};
    let backend = SessionBackend::new(
        Arc::new(InactiveReadSessionService::new(Arc::new(
            MockSessionService::new(),
        ))),
        None,
        None,
    );
    let (completion_tx, _completion_rx) = oneshot::channel();
    let result = backend
        .admit_tracked_turn(
            &MemberRef::from_bridge_session_id(SessionId::new()),
            StartTurnRequest {
                injected_context: Vec::new(),
                prompt: ContentInput::from("no fake admission"),
                system_prompt: None,
                event_tx: None,
                runtime: Default::default(),
            },
            completion_tx,
            None,
        )
        .await;
    assert!(matches!(
        result,
        Err(MobError::SessionError(SessionError::Unsupported(_)))
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn queued_reload_and_predecessor_settle_before_topology_acquires_graph_fence() {
    let mut definition = turn_driven_definition();
    definition.wiring.role_wiring = vec![RoleWiringRule {
        a: ProfileName::from("worker"),
        b: ProfileName::from("worker"),
    }];
    let mob = reconstructed_isolation_mob_with_definition(2, definition).await;
    let construction = mob
        .service
        .park_session_creation(mob.session(0).clone())
        .await;
    let release_construction = ReleaseConstructionGate(Arc::clone(&construction));
    let resume = tokio::spawn({
        let handle = mob.handle.clone();
        async move { handle.resume().await }
    });
    wait_until(
        "constructor zero holds resume before topology",
        Duration::from_secs(5),
        || async { construction.boundary_calls.load(Ordering::Acquire) > 0 },
    )
    .await;
    wait_until(
        "member one construction settled",
        Duration::from_secs(5),
        || async {
            mob.service
                .sessions
                .read()
                .await
                .contains_key(mob.session(1))
                && !mob
                    .handle
                    .machine_state_watch_rx
                    .borrow()
                    .explicit_resume_member_work
                    .contains_key(&mob_dsl::AgentIdentity::from_domain(mob.member(1)))
        },
    )
    .await;
    let comms = mob
        .service
        .sessions
        .read()
        .await
        .get(mob.session(1))
        .cloned()
        .expect("peer comms");
    let topology_gate = Arc::new(TestRuntimeControlBarrier::new());
    let release_topology = ReleaseConstructionGate(Arc::clone(&topology_gate));
    mob.service.park_live_session_lookups(mob.session(1)).await;
    let lookups = mob
        .service
        .live_session_admission_lookups
        .load(Ordering::Acquire);
    let predecessor =
        internal_turn_task(&mob.handle, mob.member(1), "late predecessor".to_string());
    wait_until(
        "predecessor is paused before runtime admission",
        Duration::from_secs(3),
        || async {
            mob.service
                .live_session_admission_lookups
                .load(Ordering::Acquire)
                > lookups
        },
    )
    .await;
    let reload = mob
        .handle
        .enqueue_actor_command_for_test(|reply_tx| MobCommand::ReloadMemberRegistration {
            agent_identity: mob.member(1).clone(),
            deadline: Instant::now() + Duration::from_secs(30),
            reply_tx,
        })
        .await
        .expect("queue reload before topology starts");
    wait_until(
        "reload parked behind predecessor",
        Duration::from_secs(3),
        || async {
            mob.handle
                .member_admission_backlog()
                .parked
                .get(mob.member(1))
                == Some(&1)
        },
    )
    .await;
    drop(release_construction);
    wait_until(
        "resume readiness settled behind prior admission",
        Duration::from_secs(5),
        || async {
            mob.handle
                .machine_state_watch_rx
                .borrow()
                .explicit_resume_readiness_settled
        },
    )
    .await;
    assert!(
        !mob.handle
            .machine_state_watch_rx
            .borrow()
            .explicit_resume_topology_pending
    );
    *comms.trust_mutation_gate.write().expect("trust gate") = Some(Arc::clone(&topology_gate));
    mob.probe(Duration::from_secs(1))
        .await
        .expect("query progresses while topology waits");
    assert_eq!(
        topology_gate.boundary_calls.load(Ordering::Acquire),
        0,
        "topology_pending={}, backlog={:?}",
        mob.handle
            .machine_state_watch_rx
            .borrow()
            .explicit_resume_topology_pending,
        mob.handle.member_admission_backlog(),
    );
    assert!(
        !mob.handle
            .member_admission_backlog()
            .reload_invocations
            .contains_key(mob.member(1))
    );
    assert!(!resume.is_finished());
    mob.service.release_live_session_lookups().await;
    let outcome = tokio::time::timeout(Duration::from_secs(5), reload)
        .await
        .expect("queued reload settles before graph mutation")
        .expect("reply")
        .expect("reload");
    assert_eq!(outcome.disposition, MemberReloadDisposition::NotDegraded);
    assert_eq!(
        mob.handle
            .member_admission_backlog()
            .reload_invocations
            .get(mob.member(1)),
        Some(&1)
    );
    wait_until(
        "graph starts after the prior lane releases custody",
        Duration::from_secs(5),
        || async { topology_gate.boundary_calls.load(Ordering::Acquire) > 0 },
    )
    .await;
    drop(release_topology);
    tokio::time::timeout(Duration::from_secs(10), resume)
        .await
        .expect("resume settles")
        .expect("resume task")
        .expect("resume succeeds");
    tokio::time::timeout(Duration::from_secs(5), predecessor)
        .await
        .expect("predecessor completes")
        .expect("task")
        .expect("delivery");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unconsumed_transferred_claim_rolls_back_before_reply_or_lane_settlement() {
    let mob = create_isolation_mob(2).await;
    mob.degrade_member(0).await;
    mob.store.fail_commit(mob.session(0), false);
    let (cleanup_entered, release_cleanup) =
        crate::runtime::provisioner::arm_reload_unused_claim_cleanup_for_test(
            mob.session(0).clone(),
        );
    let mut reply = mob
        .handle
        .enqueue_actor_command_for_test(|reply_tx| MobCommand::ReloadMemberRegistration {
            agent_identity: mob.member(0).clone(),
            deadline: Instant::now() + Duration::from_secs(15),
            reply_tx,
        })
        .await
        .expect("enqueue reload");
    tokio::time::timeout(Duration::from_secs(5), cleanup_entered)
        .await
        .expect("early provisioning failure reaches exact unused-claim cleanup")
        .expect("cleanup pause retained");
    let registration = mob
        .adapter
        .current_session_registration_witness(mob.session(0))
        .await
        .expect("cold registration still present");
    assert!(
        !mob.adapter
            .registration_is_current_without_runtime_owner(&registration)
            .await,
        "unconsumed Prepared remains the actual claim owner during cleanup"
    );
    assert!(matches!(
        reply.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    assert!(
        !mob.handle
            .member_admission_backlog()
            .settled_reload_invocations
            .contains_key(mob.member(0)),
        "cleanup observation is not ticket settlement"
    );
    peer_progresses(&mob).await;
    release_cleanup.send(()).expect("release exact rollback");
    let result = tokio::time::timeout(Duration::from_secs(5), reply)
        .await
        .expect("reply after actual rollback")
        .expect("reply channel");
    assert!(
        result.is_err(),
        "the real early provisioning failure must still surface"
    );
    assert!(
        mob.adapter
            .registration_is_current_without_runtime_owner(&registration)
            .await,
        "reply requires the unused claim's actual rollback"
    );
    wait_until(
        "reload ticket settles after rollback",
        Duration::from_secs(3),
        || async {
            mob.handle
                .member_admission_backlog()
                .settled_reload_invocations
                .get(mob.member(0))
                == Some(&1)
        },
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn blocked_warm_constructor_keeps_query_and_peer_turn_progress() {
    let mob = create_isolation_mob(2).await;
    mob.degrade_member(0).await;
    mob.store.fail_commit(mob.session(0), false);
    let construction = mob
        .service
        .park_session_creation(mob.session(0).clone())
        .await;
    let release = ReleaseConstructionGate(Arc::clone(&construction));
    let reload = mob
        .handle
        .enqueue_actor_command_for_test(|reply_tx| MobCommand::ReloadMemberRegistration {
            agent_identity: mob.member(0).clone(),
            deadline: Instant::now() + Duration::from_secs(15),
            reply_tx,
        })
        .await
        .expect("enqueue reload");
    wait_until(
        "warm constructor owns the exact claim off actor",
        Duration::from_secs(5),
        || async { construction.boundary_calls.load(Ordering::Acquire) > 0 },
    )
    .await;
    let delivery = internal_turn_task(
        &mob.handle,
        mob.member(0),
        "after warm constructor".to_string(),
    );
    wait_for_parked(&mob, 1).await;
    peer_progresses(&mob).await;
    assert!(!delivery.is_finished());
    drop(release);
    let result = tokio::time::timeout(Duration::from_secs(5), reload)
        .await
        .expect("reload completes")
        .expect("reply")
        .expect("reload");
    assert_eq!(result.disposition, MemberReloadDisposition::Discarded);
    tokio::time::timeout(Duration::from_secs(5), delivery)
        .await
        .expect("queued delivery completes")
        .expect("task")
        .expect("delivery");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn readiness_retry_reuses_the_exact_published_actor_without_reconstruction() {
    let mob = create_isolation_mob(2).await;
    mob.handle
        .wire(mob.member(0).clone(), mob.member(1).clone())
        .await
        .expect("wire peers");
    mob.degrade_member(0).await;
    mob.store.fail_commit(mob.session(0), false);
    let (entered, release) = crate::runtime::actor::reload_revival::pause_before_readiness_for_test(
        mob.session(0).clone(),
    );
    let reload = mob
        .handle
        .enqueue_actor_command_for_test(|reply_tx| MobCommand::ReloadMemberRegistration {
            agent_identity: mob.member(0).clone(),
            deadline: Instant::now() + Duration::from_secs(15),
            reply_tx,
        })
        .await
        .expect("enqueue reload");
    tokio::time::timeout(Duration::from_secs(5), entered)
        .await
        .expect("construction published before readiness")
        .expect("barrier");
    let actor = mob
        .service
        .actor_registry
        .current(mob.session(0))
        .expect("published actor");
    let attachment = mob
        .adapter
        .current_executor_attachment_witness(mob.session(0))
        .await
        .expect("published executor");
    let creates = mob.service.session_counter.load(Ordering::Relaxed);
    mob.service.set_missing_comms_runtime(mob.session(0)).await;
    release
        .send(())
        .expect("let readiness observe unavailable comms");
    assert!(
        tokio::time::timeout(Duration::from_secs(5), reload)
            .await
            .expect("readiness failure answers")
            .expect("reply")
            .is_err()
    );
    assert_ne!(
        mob.handle
            .member_status(mob.member(0))
            .await
            .expect("member status")
            .status,
        crate::runtime::handle::MobMemberStatus::Broken
    );
    mob.service
        .missing_comms_sessions
        .write()
        .await
        .remove(mob.session(0));
    mob.service
        .discard_actor_failures_remaining
        .store(1, Ordering::Relaxed);
    let retried = tokio::time::timeout(
        Duration::from_secs(5),
        mob.handle.reload_member_registration(mob.member(0)),
    )
    .await
    .expect("readiness retry finishes")
    .expect("retry");
    assert_eq!(retried.disposition, MemberReloadDisposition::Discarded);
    assert_eq!(mob.service.session_counter.load(Ordering::Relaxed), creates);
    assert_eq!(
        mob.service.actor_registry.current(mob.session(0)),
        Some(actor)
    );
    assert_eq!(
        mob.adapter
            .current_executor_attachment_witness(mob.session(0))
            .await,
        Some(attachment)
    );
    assert_eq!(
        mob.service
            .discard_actor_failures_remaining
            .load(Ordering::Relaxed),
        1
    );
    let restored = mob
        .service
        .sessions
        .read()
        .await
        .get(mob.session(0))
        .cloned()
        .expect("restored comms");
    let peer = mob
        .service
        .sessions
        .read()
        .await
        .get(mob.session(1))
        .cloned()
        .expect("peer comms");
    assert!(
        restored
            .trusted_peers
            .read()
            .await
            .contains_key(&peer.peer_id().expect("peer identity").to_string())
    );
    assert!(
        peer.trusted_peers
            .read()
            .await
            .contains_key(&restored.peer_id().expect("restored identity").to_string())
    );
    mob.handle
        .send_peer_message(
            mob.member(0).clone(),
            mob.member(1).clone(),
            "peer delivery after resumed topology",
            HandlingMode::Queue,
        )
        .await
        .expect("restored peer delivery");
    mob.service
        .discard_actor_failures_remaining
        .store(0, Ordering::Relaxed);
    peer_progresses(&mob).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stop_waits_for_warm_construction_without_blocking_query_progress() {
    let mob = create_isolation_mob(2).await;
    mob.degrade_member(0).await;
    mob.store.fail_commit(mob.session(0), false);
    let construction = mob
        .service
        .park_session_creation(mob.session(0).clone())
        .await;
    let release = ReleaseConstructionGate(Arc::clone(&construction));
    let reload = mob
        .handle
        .enqueue_actor_command_for_test(|reply_tx| MobCommand::ReloadMemberRegistration {
            agent_identity: mob.member(0).clone(),
            deadline: Instant::now() + Duration::from_secs(15),
            reply_tx,
        })
        .await
        .expect("enqueue reload");
    wait_until(
        "warm constructor owns its claim",
        Duration::from_secs(5),
        || async { construction.boundary_calls.load(Ordering::Acquire) > 0 },
    )
    .await;
    peer_progresses(&mob).await;
    let mut stop = mob
        .handle
        .enqueue_actor_command_for_test(|reply_tx| MobCommand::Stop { reply_tx })
        .await
        .expect("enqueue Stop");
    mob.probe(Duration::from_secs(1))
        .await
        .expect("lifecycle wait does not own the actor loop");
    assert!(matches!(
        stop.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    drop(release);
    tokio::time::timeout(Duration::from_secs(10), stop)
        .await
        .expect("Stop settles after construction")
        .expect("Stop reply")
        .expect("Stop");
    let _ = tokio::time::timeout(Duration::from_secs(5), reload)
        .await
        .expect("reload observation settles")
        .expect("reload reply");
}
