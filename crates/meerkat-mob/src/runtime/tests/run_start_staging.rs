//! A mob Stop stages `MobStop` for every local member before it holds
//! the member, so a registration that replaces a held one while the mob is
//! Stopped is held too.
//!
//! A staged reason belongs to the runtime the mob shares with others, not to
//! the mob: it outlives the registration it was staged for, and a runtime
//! unregister leaves it in place on purpose (a reload unregisters and
//! registers the same session again). So the mob drops it wherever its
//! members are gone for good (retire, Shutdown, destroy), and a restore
//! reconciles it to the restored posture before any member registers.

use super::*;
use crate::runtime::stop_report::{MemberRunStarts, MemberStopRun};

/// Bound for test steps that must happen promptly; it only turns a hang into
/// a failure.
const STEP: Duration = Duration::from_secs(10);

/// A member identity of its own: test gates and in-process routes are keyed
/// by identity and mob id, and tests share one process.
fn unique_identity(label: &str) -> AgentIdentity {
    AgentIdentity::from(format!("{label}-{}", uuid::Uuid::new_v4().simple()))
}

/// One runtime-backed TurnDriven member over the persistent machine the
/// isolation tests use, in a mob with its own id.
async fn persistent_single_member_mob(label: &str) -> IsolationMob {
    let store = FaultInjectingRuntimeStore::new();
    let runtime_store: Arc<dyn RuntimeStore> = Arc::clone(&store) as Arc<dyn RuntimeStore>;
    let blob_store: Arc<dyn meerkat_core::BlobStore> =
        Arc::new(meerkat_store::MemoryBlobStore::new());
    let adapter = Arc::new(
        MeerkatMachine::persistent(runtime_store, blob_store).expect("construct runtime authority"),
    );
    let service = Arc::new(MockSessionService::new());
    service.set_runtime_adapter(Arc::clone(&adapter));
    let handle = MobBuilder::new(
        with_unique_mob_id(turn_driven_definition(), label),
        MobStorage::in_memory(),
    )
    .with_session_service(service.clone())
    .create()
    .await
    .expect("create mob");
    let identity = unique_identity(label);
    let session_id = handle
        .spawn(ProfileName::from("worker"), identity.clone(), None)
        .await
        .expect("spawn member")
        .bridge_session_id()
        .expect("runtime-backed member has a bridge session")
        .clone();
    IsolationMob {
        handle,
        service,
        store,
        adapter,
        members: vec![(identity, session_id)],
    }
}

/// One TurnDriven member of a mob over an ephemeral runtime, reopened
/// Stopped in the same process: a Stop, a Shutdown, and a restore over the
/// same stores, session service and runtime. The restore stages `MobStop`
/// for the member, because the restored posture holds member run starts.
async fn reopened_stopped_member(
    label: &str,
) -> (MobHandle, Arc<MeerkatMachine>, AgentIdentity, SessionId) {
    let service = Arc::new(MockSessionService::new());
    let adapter = service.enable_runtime_adapter();
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
    let runtime_metadata = storage.runtime_metadata.clone();
    let handle = MobBuilder::new(with_unique_mob_id(turn_driven_definition(), label), storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob");
    let identity = unique_identity(label);
    let session_id = handle
        .spawn(ProfileName::from("worker"), identity.clone(), None)
        .await
        .expect("spawn member")
        .bridge_session_id()
        .expect("runtime-backed member has a bridge session")
        .clone();
    handle.stop().await.expect("stop the mob");
    handle.shutdown().await.expect("shut the stopped mob down");
    let reopened = MobBuilder::for_resume(MobStorage::with_events_and_runtime_metadata(
        events,
        runtime_metadata,
    ))
    .with_session_service(service)
    .notify_orchestrator_on_resume(false)
    .resume()
    .await
    .expect("reopen the stopped mob");
    assert_eq!(reopened.status().await.expect("phase"), MobState::Stopped);
    (reopened, adapter, identity, session_id)
}

/// Register `session_id` again in the runtime its member used, as a later
/// owner of the same session id would, and read whether that registration
/// holds run starts: a registration applies the reasons staged for it.
async fn held_on_registering_again(
    adapter: &MeerkatMachine,
    session_id: &SessionId,
) -> Option<bool> {
    assert!(
        !adapter.contains_session(session_id).await,
        "the departed member's registration is gone"
    );
    adapter
        .register_session(session_id.clone())
        .await
        .expect("register the session again");
    adapter.run_starts_held_for_test(session_id).await
}

/// Prompts the runtime executor ran on `session_id`.
async fn executed_prompts(service: &MockSessionService, session_id: &SessionId) -> Vec<String> {
    service
        .start_turn_prompts
        .read()
        .await
        .iter()
        .filter(|(session, _)| session == session_id)
        .map(|(_, prompt)| prompt.clone())
        .collect()
}

/// A Stop stages `MobStop` before it holds a member, so the registration a
/// reload publishes while the mob is Stopped is held as well. The reload
/// answers `Discarded` and its revival waits for Running, so the successor it
/// published stays a cold registration until Resume; input admitted to it
/// runs only once Resume released the member.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_member_reloaded_while_stopped_registers_its_successor_held() {
    let mob = persistent_single_member_mob("reload-while-stopped").await;
    mob.handle.stop().await.expect("stop the mob");
    assert_eq!(
        mob.adapter.run_starts_held_for_test(mob.session(0)).await,
        Some(true),
        "the Stop holds the member"
    );

    // The member's registration degrades while the mob is Stopped, and a
    // reload replaces it with a fresh registration of the same session.
    assert!(
        mob.adapter
            .force_session_durability_reload_required_for_test(mob.session(0))
            .await,
        "the member's registration degrades"
    );
    let reload = tokio::time::timeout(STEP, mob.handle.reload_member_registration(mob.member(0)))
        .await
        .expect("the reload answers")
        .expect("the reload succeeds while the mob is Stopped");
    assert_eq!(
        reload.disposition,
        crate::runtime::handle::MemberReloadDisposition::Discarded,
        "the reload discards the degraded registration; its revival waits for Running"
    );
    assert!(
        mob.adapter
            .current_session_registration_witness(mob.session(0))
            .await
            .is_some(),
        "the reload published a successor registration of the session"
    );
    assert!(
        mob.adapter
            .current_executor_attachment_witness(mob.session(0))
            .await
            .is_none(),
        "the successor has no executor while the mob is Stopped"
    );
    assert_eq!(
        mob.adapter.run_starts_held_for_test(mob.session(0)).await,
        Some(true),
        "the reload's successor registration applies the staged MobStop"
    );

    // Input admitted to the successor runs only after Resume.
    let prompt = "admitted to the reloaded member while the mob is stopped";
    let (accepted, _completion) = mob
        .adapter
        .accept_input_with_completion(
            mob.session(0),
            meerkat_runtime::Input::Prompt(meerkat_runtime::PromptInput::new(prompt, None)),
        )
        .await
        .expect("the runtime admits the input");
    assert!(accepted.is_accepted());
    mob.handle.resume().await.expect("resume the mob");
    wait_until("the held input runs after Resume", STEP, || async {
        mob.executed_prompts(0)
            .await
            .iter()
            .any(|executed| executed == prompt)
    })
    .await;
}

/// A `MobStop` staged for a member by an earlier incarnation in this process
/// is dropped by a restore whose posture releases run starts, before the
/// member registers: the reopened member runs its input. (A Stop that
/// fail-stops reopens Stopped, since its quiesce intent is durable before it
/// stages, so the earlier incarnation's leftover is set up directly here.)
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_running_restore_drops_a_stale_staged_hold() {
    let label = "running-restore-stale-stage";
    let service = Arc::new(MockSessionService::new());
    let adapter = service.enable_runtime_adapter();
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
    let runtime_metadata = storage.runtime_metadata.clone();
    let handle = MobBuilder::new(with_unique_mob_id(turn_driven_definition(), label), storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create mob");
    let identity = unique_identity(label);
    let session_id = handle
        .spawn(ProfileName::from("worker"), identity.clone(), None)
        .await
        .expect("spawn member")
        .bridge_session_id()
        .expect("runtime-backed member has a bridge session")
        .clone();
    adapter
        .stage_registration_run_start_hold(
            &session_id,
            meerkat_runtime::RunStartHoldReason::MobStop,
        )
        .await
        .expect("stage MobStop as an earlier incarnation left it");
    assert_eq!(
        adapter.run_starts_held_for_test(&session_id).await,
        Some(true),
        "the staged MobStop holds the member"
    );
    crash_stop_and_release_routes(handle).await;

    let reopened = MobBuilder::for_resume(MobStorage::with_events_and_runtime_metadata(
        events,
        runtime_metadata,
    ))
    .with_session_service(service.clone())
    .notify_orchestrator_on_resume(false)
    .resume()
    .await
    .expect("reopen the mob");
    assert_eq!(reopened.status().await.expect("phase"), MobState::Running);

    // The member is materialized by its first delivery, after the restore
    // dropped the stale stage: its registration is not held, and it runs.
    let prompt = "delivered after the reopen";
    reopened
        .member(&identity)
        .await
        .expect("member handle")
        .send(prompt.to_string(), HandlingMode::Queue)
        .await
        .expect("the delivery is admitted");
    wait_until("the reopened member runs the delivery", STEP, || async {
        executed_prompts(&service, &session_id)
            .await
            .iter()
            .any(|executed| executed == prompt)
    })
    .await;
    assert_eq!(
        adapter.run_starts_held_for_test(&session_id).await,
        Some(false),
        "the reopened member's registration is not held"
    );
}

/// A member retired while its mob is Stopped takes the `MobStop` staged for
/// it along (here by the restore of the Stopped mob; a Stop stages the same
/// reason): Resume releases only the members the mob still has, so otherwise
/// a later registration of the same session would be held for good.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_member_retired_while_stopped_leaves_no_staged_hold() {
    let (handle, adapter, identity, session_id) =
        reopened_stopped_member("retired-while-stopped").await;
    handle
        .retire(identity)
        .await
        .expect("retire the member while the mob is Stopped");
    handle.resume().await.expect("resume the mob");
    assert_eq!(
        held_on_registering_again(&adapter, &session_id).await,
        Some(false),
        "a later registration of the retired member's session is not held"
    );
}

/// A Shutdown drops the `MobStop` staged for the members it tears down.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_shutdown_of_a_stopped_mob_leaves_no_staged_hold() {
    let (handle, adapter, _identity, session_id) =
        reopened_stopped_member("shutdown-while-stopped").await;
    handle.shutdown().await.expect("shut the mob down");
    assert_eq!(
        held_on_registering_again(&adapter, &session_id).await,
        Some(false),
        "a later registration of a torn-down member's session is not held"
    );
}

/// A destroy drops the `MobStop` staged for the members it disposes of.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_destroy_of_a_stopped_mob_leaves_no_staged_hold() {
    let (handle, adapter, _identity, session_id) =
        reopened_stopped_member("destroy-while-stopped").await;
    handle.destroy().await.expect("destroy the mob");
    assert_eq!(
        held_on_registering_again(&adapter, &session_id).await,
        Some(false),
        "a later registration of a disposed member's session is not held"
    );
}

/// A Stop of a member whose registration is durability-degraded reports the
/// member `ReloadRequired` instead of failing: the degraded registration
/// refuses the hold and starts no run, and the `MobStop` the Stop staged
/// first holds the registration a reload replaces it with. Nothing runs
/// while the mob is Stopped, and the input admitted to the reloaded member
/// runs only once Resume released it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stop_reports_a_degraded_member_reload_required_and_holds_its_reload() {
    let mob = persistent_single_member_mob("stop-degraded").await;
    assert!(
        mob.adapter
            .force_session_durability_reload_required_for_test(mob.session(0))
            .await,
        "the member's registration degrades"
    );
    let report = tokio::time::timeout(STEP, mob.handle.stop())
        .await
        .expect("the stop answers")
        .expect("the stop succeeds");
    let outcome = report
        .members
        .get(mob.member(0))
        .expect("the degraded member's stop outcome");
    assert_eq!(outcome.run, MemberStopRun::ReloadRequired, "{report:?}");
    assert_eq!(
        outcome.starts,
        MemberRunStarts::ReloadRequired,
        "{report:?}"
    );
    assert_eq!(
        report.not_holdable().count(),
        0,
        "a degraded member is not reported as able to start a turn: {report:?}"
    );

    // The degraded registration admits no input while the mob is Stopped.
    let refused = mob
        .adapter
        .accept_input_with_completion(
            mob.session(0),
            meerkat_runtime::Input::Prompt(meerkat_runtime::PromptInput::new(
                "offered to the degraded member",
                None,
            )),
        )
        .await;
    assert!(
        !matches!(&refused, Ok((accepted, _)) if accepted.is_accepted()),
        "the degraded registration admits no input"
    );

    // A reload while Stopped publishes a successor that applies the staged
    // MobStop and answers Discarded; its revival waits for Running.
    let reload = tokio::time::timeout(STEP, mob.handle.reload_member_registration(mob.member(0)))
        .await
        .expect("the reload answers")
        .expect("the reload succeeds while the mob is Stopped");
    assert_eq!(
        reload.disposition,
        crate::runtime::handle::MemberReloadDisposition::Discarded,
        "the reload discards the degraded registration; its revival waits for Running"
    );
    assert_eq!(
        mob.adapter.run_starts_held_for_test(mob.session(0)).await,
        Some(true),
        "the reload's successor registration applies the staged MobStop"
    );
    let prompt = "admitted to the reloaded member while the mob is stopped";
    let (accepted, _completion) = mob
        .adapter
        .accept_input_with_completion(
            mob.session(0),
            meerkat_runtime::Input::Prompt(meerkat_runtime::PromptInput::new(prompt, None)),
        )
        .await
        .expect("the runtime admits the input");
    assert!(accepted.is_accepted());
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        !mob.executed_prompts(0)
            .await
            .iter()
            .any(|executed| executed == prompt),
        "no run starts while the mob is Stopped"
    );
    mob.handle.resume().await.expect("resume the mob");
    wait_until("the held input runs after Resume", STEP, || async {
        mob.executed_prompts(0)
            .await
            .iter()
            .any(|executed| executed == prompt)
    })
    .await;
}

/// A member whose registration needs a durability reload: while the mob is
/// Running its status names the reload and a request to it is refused typed
/// (`MemberReloadRequired`) instead of waiting. After a Stop, a Resume
/// refuses typed while the member still needs the reload, before anything
/// moves: the mob stays Stopped. The member's reload, admitted while Stopped,
/// publishes a successor the staged `MobStop` holds, and the Resume then
/// releases it and the member serves again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_resume_refuses_typed_while_a_member_needs_a_reload() {
    let mob = persistent_single_member_mob("resume-degraded").await;
    assert!(
        mob.adapter
            .force_session_durability_reload_required_for_test(mob.session(0))
            .await,
        "the member's registration degrades"
    );

    // Running: the status names the reload, and a request is refused typed.
    let status = tokio::time::timeout(STEP, mob.handle.member_status(mob.member(0)))
        .await
        .expect("the member status answers")
        .expect("the member status reads");
    assert!(
        status
            .error
            .as_deref()
            .is_some_and(|error| error.contains("requires a reload")),
        "the member's status names the reload it needs: {status:?}"
    );
    let refused = tokio::time::timeout(STEP, mob.send(0, "sent to the degraded member"))
        .await
        .expect("the send answers");
    assert!(
        matches!(
            &refused,
            Err(MobError::MemberReloadRequired { member_id, .. }) if member_id == mob.member(0)
        ),
        "a request to the degraded member is refused typed: {refused:?}"
    );

    let report = tokio::time::timeout(STEP, mob.handle.stop())
        .await
        .expect("the stop answers")
        .expect("the stop succeeds");
    assert_eq!(
        report
            .members
            .get(mob.member(0))
            .map(|outcome| outcome.starts.clone()),
        Some(MemberRunStarts::ReloadRequired),
        "{report:?}"
    );

    // The Resume refuses typed, naming the member, and nothing moves.
    let refused = tokio::time::timeout(STEP, mob.handle.resume())
        .await
        .expect("the resume answers")
        .expect_err("a member that needs a reload blocks the resume");
    let refused = match &refused {
        MobError::SharedLifecycleFailure(inner) => inner.as_ref(),
        other => other,
    };
    assert!(
        matches!(
            refused,
            MobError::MemberReloadRequired { member_id, .. } if member_id == mob.member(0)
        ),
        "the resume names the member to reload: {refused:?}"
    );
    assert_eq!(mob.handle.status().await.expect("phase"), MobState::Stopped);

    // The reload while Stopped publishes a successor that applies the staged
    // MobStop and answers Discarded; its revival waits for Running.
    let reload = tokio::time::timeout(STEP, mob.handle.reload_member_registration(mob.member(0)))
        .await
        .expect("the reload answers")
        .expect("the reload succeeds while the mob is Stopped");
    assert_eq!(
        reload.disposition,
        crate::runtime::handle::MemberReloadDisposition::Discarded,
        "the reload discards the degraded registration; its revival waits for Running"
    );
    assert_eq!(
        mob.adapter.run_starts_held_for_test(mob.session(0)).await,
        Some(true),
        "the reloaded registration stays held while Stopped"
    );

    // The Resume now releases the reloaded member, which serves again.
    tokio::time::timeout(STEP, mob.handle.resume())
        .await
        .expect("the resume answers")
        .expect("the resume releases the reloaded member");
    assert_eq!(mob.handle.status().await.expect("phase"), MobState::Running);
    assert_eq!(
        mob.adapter.run_starts_held_for_test(mob.session(0)).await,
        Some(false),
        "the reloaded member is not held after Resume"
    );
    tokio::time::timeout(STEP, mob.send(0, "sent after the reload"))
        .await
        .expect("the send answers")
        .expect("the reloaded member accepts the request");
}

/// A member whose runtime unregister is finalizing (its final unregister
/// transition is committed and the store transaction that ends it is still
/// in flight) refuses every lifecycle mutation, the Stop's hold among them.
/// The Stop reports the member with no run and its run starts held instead
/// of failing, and the unregister then finishes and removes the registration.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stop_holds_a_member_whose_unregister_is_finalizing() {
    let mob = persistent_single_member_mob("unregister-finalizing").await;
    let session = mob.session(0).clone();
    mob.store.park_unregister_finalizations(&session);
    let unregister = tokio::spawn({
        let adapter = Arc::clone(&mob.adapter);
        let session = session.clone();
        async move {
            adapter
                .unregister_current_session_registration_until_terminal(&session)
                .await
        }
    });
    wait_until(
        "the member's unregister finalization parks",
        STEP,
        || async { mob.store.parked_unregister_finalization_arrivals(&session) == 1 },
    )
    .await;
    assert!(
        mob.adapter.unregister_owed(&session).await,
        "the finalizing registration refuses lifecycle mutations"
    );

    let report = tokio::time::timeout(STEP, mob.handle.stop())
        .await
        .expect("the stop answers")
        .expect("the stop succeeds");
    assert_eq!(
        report.members.get(mob.member(0)),
        Some(&crate::runtime::stop_report::MemberStopOutcome {
            run: MemberStopRun::NoRun,
            starts: MemberRunStarts::Held,
        }),
        "{report:?}"
    );

    mob.store.release_unregister_finalizations();
    let removed = tokio::time::timeout(STEP, unregister)
        .await
        .expect("the unregister finishes")
        .expect("the unregister task")
        .expect("the unregister succeeds");
    assert!(removed, "the unregister removed the registration");
    assert!(!mob.adapter.contains_session(&session).await);
}

/// A member whose runtime unregister ended with its finalization's outcome
/// unknown keeps a registration that refuses every lifecycle mutation until
/// an unregister is retried. A Shutdown reports the member's run starts held
/// instead of failing at its hold, and its teardown retries the unregister,
/// which removes the registration once the store answers again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_shutdown_retries_the_owed_unregister_of_a_member() {
    use crate::runtime::{MemberShutdownOutcome, ShutdownOptions};

    let mob = persistent_single_member_mob("unregister-owed-shutdown").await;
    let session = mob.session(0).clone();
    mob.store
        .fail_unregister_finalizations(&session, Some(UnregisterFinalizationFault::OutcomeUnknown));
    let unregistered = tokio::time::timeout(
        STEP,
        mob.adapter
            .unregister_current_session_registration_until_terminal(&session),
    )
    .await
    .expect("the unregister answers");
    assert!(
        matches!(
            unregistered,
            Err(meerkat_runtime::RuntimeDriverError::UnregisterFinalizationOutcomeUnknown { .. })
        ),
        "{unregistered:?}"
    );
    assert!(
        mob.adapter.unregister_owed(&session).await,
        "the registration waits for a retried unregister"
    );
    // The store answers again; nothing retries the unregister on its own.
    mob.store.fail_unregister_finalizations(&session, None);

    let report = tokio::time::timeout(
        STEP,
        mob.handle.shutdown_with_report(
            ShutdownOptions::default().with_deadline(Instant::now() + Duration::from_secs(5)),
        ),
    )
    .await
    .expect("the shutdown answers")
    .expect("the shutdown succeeds");
    assert_eq!(
        report.run_starts.get(mob.member(0)),
        Some(&MemberRunStarts::Held),
        "{report:?}"
    );
    assert!(
        matches!(
            report.runs.get(mob.member(0)),
            None | Some(MemberStopRun::NoRun)
        ),
        "{report:?}"
    );
    assert!(
        matches!(
            report.members.get(mob.member(0)),
            Some(MemberShutdownOutcome::Unregistered)
        ),
        "{report:?}"
    );
    assert!(
        !mob.adapter.contains_session(&session).await,
        "the teardown's retried unregister removed the registration"
    );
}

/// A member whose runtime unregister failed definitely at its finalization is
/// the teardown-only anchor the next unregister retries from, and it refuses
/// every lifecycle mutation until then. A Stop reports the member with no run
/// and its run starts held instead of failing, and a Shutdown's teardown
/// retries the unregister, which removes the registration.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stop_and_a_shutdown_settle_a_member_left_as_an_unregister_anchor() {
    use crate::runtime::{MemberShutdownOutcome, ShutdownOptions};

    let mob = persistent_single_member_mob("unregister-anchor").await;
    let session = mob.session(0).clone();
    mob.store
        .fail_unregister_finalizations(&session, Some(UnregisterFinalizationFault::WriteFailed));
    let unregistered = tokio::time::timeout(
        STEP,
        mob.adapter
            .unregister_current_session_registration_until_terminal(&session),
    )
    .await
    .expect("the unregister answers");
    assert!(unregistered.is_err(), "{unregistered:?}");
    assert!(
        mob.adapter.unregister_owed(&session).await,
        "the anchor waits for the next unregister"
    );

    let report = tokio::time::timeout(STEP, mob.handle.stop())
        .await
        .expect("the stop answers")
        .expect("the stop succeeds");
    assert_eq!(
        report.members.get(mob.member(0)),
        Some(&crate::runtime::stop_report::MemberStopOutcome {
            run: MemberStopRun::NoRun,
            starts: MemberRunStarts::Held,
        }),
        "{report:?}"
    );

    // The store answers again; the Shutdown's teardown retries the unregister.
    mob.store.fail_unregister_finalizations(&session, None);
    let report = tokio::time::timeout(
        STEP,
        mob.handle.shutdown_with_report(
            ShutdownOptions::default().with_deadline(Instant::now() + Duration::from_secs(5)),
        ),
    )
    .await
    .expect("the shutdown answers")
    .expect("the shutdown succeeds");
    assert!(
        matches!(
            report.members.get(mob.member(0)),
            Some(MemberShutdownOutcome::Unregistered)
        ),
        "{report:?}"
    );
    assert!(
        !mob.adapter.contains_session(&session).await,
        "the teardown's retried unregister removed the registration"
    );
}
