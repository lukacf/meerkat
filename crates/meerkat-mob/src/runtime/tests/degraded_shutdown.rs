//! Shutdown's teardown, and a settlement it reports, unregister a member whose
//! runtime registration is durability-degraded.
//!
//! A boundary commit that fails leaves the session `ReloadRequired`, and its
//! exited runtime loop can keep the session's mutation fence in its
//! published handoff. Only the runtime's reload-required discard releases
//! that fence, so an exact unregister admitted first waits on it for good.
//! Shutdown's teardown and a member's unregister settlement therefore run the
//! discard first, as retire and destroy do, and unregister the exact cold
//! successor it publishes.

use super::*;
use crate::runtime::actor::ShutdownUnregisterOutcome;
use crate::runtime::stop_report::{MemberRunStarts, MemberStopRun};
use crate::runtime::{
    MemberShutdownOutcome, MemberUnregisterSettlement, MemberUnregisterState, MobShutdownReport,
    ShutdownOptions,
};

/// Bound for test steps that must happen promptly; it only turns a hang into
/// a failure.
const STEP: Duration = Duration::from_secs(10);

/// Shutdown's teardown of a degraded member's session discards the degraded
/// registration and unregisters the cold successor within its deadline,
/// instead of reporting the member pending at an admission that waits on the
/// retained fence. (A whole Shutdown of a degraded member waits earlier, in
/// its run-start hold, on the same fence: that gate wait is bounded
/// separately, so the teardown is driven directly here.)
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_teardown_discards_a_degraded_registration_first() {
    let mob = create_isolation_mob(1).await;
    mob.degrade_member(0).await;
    // The store recovers; the registration stays degraded until disposed.
    mob.store.fail_commit(mob.session(0), false);

    let outcome = tokio::time::timeout(
        STEP,
        crate::runtime::actor::teardown_session(
            &mob.adapter,
            &Arc::new(crate::runtime::ops_adapter::MobOpsAdapter::new()),
            mob.session(0),
            None,
            Instant::now() + Duration::from_secs(5),
            meerkat_runtime::UnregisterQueuedInputs::Stop,
        ),
    )
    .await
    .expect("the teardown returns within its deadline");
    match outcome {
        ShutdownUnregisterOutcome::Unregistered => {}
        ShutdownUnregisterOutcome::Pending { stage, .. } => {
            panic!("the degraded member's unregister is pending at {stage}")
        }
        ShutdownUnregisterOutcome::Failed(failure) => {
            panic!("the degraded member's unregister failed: {failure}")
        }
    }
    assert!(
        mob.adapter
            .current_session_registration_witness(mob.session(0))
            .await
            .is_none(),
        "neither the degraded registration nor its cold successor is left"
    );
}

/// A settlement over a durability-degraded registration whose exact
/// unregister was never admitted (as Shutdown leaves it when its deadline
/// passed first) runs the reload-required discard itself: without it, the
/// unregister admission waits on the mutation fence the degraded session's
/// exited runtime loop holds. It settles once the cold successor the discard
/// published is unregistered. Nobody took the registration over, so the
/// use gate lets the discard and the unregister through.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pending_degraded_unregister_settles_through_the_discard() {
    let mob = create_isolation_mob(1).await;
    mob.degrade_member(0).await;
    // The store recovers; the registration stays degraded until disposed.
    mob.store.fail_commit(mob.session(0), false);
    let registration = mob
        .adapter
        .current_session_registration_witness(mob.session(0))
        .await
        .expect("the degraded registration is current");
    let registration_use = mob.adapter.session_registration_use(&registration).await;
    assert!(
        registration_use.is_some(),
        "the current registration has a use"
    );
    let mut settlement =
        MemberUnregisterSettlement::new(registration, registration_use, None, &mob.adapter, None);
    assert_eq!(settlement.try_state(), MemberUnregisterState::Pending);

    assert_eq!(
        settlement.settle_within(Instant::now() + STEP).await,
        MemberUnregisterState::Settled
    );
    assert!(
        mob.adapter
            .current_session_registration_witness(mob.session(0))
            .await
            .is_none(),
        "neither the degraded registration nor its cold successor is left"
    );
}

/// R1, the successor read after the fact: a settlement's reload-required
/// discard published a cold successor, and before the discard returned, a
/// later owner replaced that successor with a registration of its own. The
/// settlement follows only the successor its discard published, never the
/// registration current once the discard returned: the later owner's
/// registration stays, and the settlement settles on the published
/// successor's end.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pending_degraded_unregister_follows_only_the_successor_its_discard_published() {
    let mob = create_isolation_mob(1).await;
    mob.degrade_member(0).await;
    // The store recovers; the registration stays degraded until disposed.
    mob.store.fail_commit(mob.session(0), false);
    let session = mob.session(0).clone();
    let registration = mob
        .adapter
        .current_session_registration_witness(&session)
        .await
        .expect("the degraded registration is current");
    let registration_use = mob.adapter.session_registration_use(&registration).await;
    let mut settlement =
        MemberUnregisterSettlement::new(registration, registration_use, None, &mob.adapter, None);
    let (published, release) = mob
        .adapter
        .arm_reload_required_discard_after_successor_publication_test_hook(session.clone());
    let settling = tokio::spawn(async move {
        let state = settlement.settle_within(Instant::now() + STEP).await;
        (settlement, state)
    });
    tokio::time::timeout(STEP, published)
        .await
        .expect("the settlement's discard publishes its successor")
        .expect("the discard reached its publication point");

    // A later owner replaces the published successor with its own
    // registration before the discard returns.
    let successor = mob
        .adapter
        .current_session_registration_witness(&session)
        .await
        .expect("the cold successor is current");
    assert!(
        mob.adapter
            .unregister_session_registration_until_terminal_if_current(&successor)
            .await
            .expect("unregister the successor"),
        "the successor is unregistered"
    );
    mob.adapter
        .register_session(session.clone())
        .await
        .expect("the later owner registers the session");
    let later = mob
        .adapter
        .current_session_registration_witness(&session)
        .await
        .expect("the later owner's registration is current");
    assert_ne!(later, successor);
    release.send(()).expect("release the paused discard");

    let (_settlement, state) = tokio::time::timeout(STEP + STEP, settling)
        .await
        .expect("the settlement returns")
        .expect("the settlement task");
    assert_eq!(
        mob.adapter
            .current_session_registration_witness(&session)
            .await
            .as_ref(),
        Some(&later),
        "the later owner's registration stays registered (settlement: {state:?})"
    );
    assert_eq!(
        state,
        MemberUnregisterState::Settled,
        "the successor the discard published is gone"
    );
}

/// A later owner's runtime executor that never runs anything.
struct IdleLaterOwnerExecutor;

#[async_trait::async_trait]
impl meerkat_core::lifecycle::CoreExecutor for IdleLaterOwnerExecutor {
    async fn apply(
        &mut self,
        _run_id: meerkat_core::RunId,
        _primitive: meerkat_core::lifecycle::run_primitive::RunPrimitive,
    ) -> Result<
        meerkat_core::lifecycle::core_executor::CoreApplyOutput,
        meerkat_core::lifecycle::core_executor::CoreExecutorError,
    > {
        Err(
            meerkat_core::lifecycle::core_executor::CoreExecutorError::Internal(
                "the later owner's idle fixture executor runs nothing".to_string(),
            ),
        )
    }

    async fn cancel_after_boundary(
        &mut self,
        _reason: String,
    ) -> Result<(), meerkat_core::lifecycle::core_executor::CoreExecutorError> {
        Ok(())
    }

    async fn stop_runtime_executor(
        &mut self,
        _reason: String,
    ) -> Result<(), meerkat_core::lifecycle::core_executor::CoreExecutorError> {
        Ok(())
    }
}

/// A later owner's runtime executor whose runs reach their boundary. With the
/// store's boundary commit failing, that boundary degrades the registration
/// and the runtime loop stops the executor and exits.
struct CommittingLaterOwnerExecutor {
    stopped: Arc<std::sync::atomic::AtomicBool>,
}

#[async_trait::async_trait]
impl meerkat_core::lifecycle::CoreExecutor for CommittingLaterOwnerExecutor {
    async fn apply(
        &mut self,
        run_id: meerkat_core::RunId,
        primitive: meerkat_core::lifecycle::run_primitive::RunPrimitive,
    ) -> Result<
        meerkat_core::lifecycle::core_executor::CoreApplyOutput,
        meerkat_core::lifecycle::core_executor::CoreExecutorError,
    > {
        Ok(
            meerkat_core::lifecycle::core_executor::CoreApplyOutput::with_untyped_snapshot(
                meerkat_core::lifecycle::run_receipt::RunBoundaryReceiptDraft {
                    run_id,
                    boundary: meerkat_core::lifecycle::run_primitive::RunApplyBoundary::RunStart,
                    contributing_input_ids: primitive.contributing_input_ids().to_vec(),
                    conversation_digest: None,
                    message_count: 0,
                },
                None,
                None,
            ),
        )
    }

    async fn cancel_after_boundary(
        &mut self,
        _reason: String,
    ) -> Result<(), meerkat_core::lifecycle::core_executor::CoreExecutorError> {
        Ok(())
    }

    async fn stop_runtime_executor(
        &mut self,
        _reason: String,
    ) -> Result<(), meerkat_core::lifecycle::core_executor::CoreExecutorError> {
        self.stopped
            .store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
}

/// R1 at the discard step, with the later owner's runtime loop still live:
/// Shutdown left the member's registration without a runtime loop and its
/// unregister not admitted. A later owner adopted that registration in place,
/// and the registration then became durability-degraded while the later
/// owner's loop still runs. Settling must not go after the later owner's
/// registration with a reload-required discard: it reports the takeover, and
/// the later owner's loop stays attached.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pending_unregister_never_discards_a_later_owners_degraded_registration_with_a_live_loop()
{
    let runtime_store: Arc<dyn RuntimeStore> = Arc::new(InMemoryRuntimeStore::new());
    let blob_store: Arc<dyn meerkat_core::BlobStore> =
        Arc::new(meerkat_store::MemoryBlobStore::new());
    let adapter = Arc::new(
        MeerkatMachine::persistent(runtime_store, blob_store).expect("construct runtime authority"),
    );
    let session = SessionId::new();
    adapter
        .register_session(session.clone())
        .await
        .expect("register a registration without a runtime loop");
    let registration = adapter
        .current_session_registration_witness(&session)
        .await
        .expect("current registration");
    let registration_use = adapter.session_registration_use(&registration).await;
    let mut settlement = MemberUnregisterSettlement::new(
        registration.clone(),
        registration_use,
        None,
        &adapter,
        None,
    );

    adapter
        .ensure_session_with_executor(session.clone(), Box::new(IdleLaterOwnerExecutor))
        .await
        .expect("a later owner adopts the registration in place");
    let adopted = adapter
        .current_executor_attachment_witness(&session)
        .await
        .expect("the later owner's runtime loop is attached");
    assert!(
        adapter
            .force_session_durability_reload_required_for_test(&session)
            .await,
        "the later owner's registration degrades while its loop runs"
    );

    let state = settlement
        .settle_within(Instant::now() + Duration::from_secs(2))
        .await;
    assert_eq!(
        adapter
            .current_session_registration_witness(&session)
            .await
            .as_ref(),
        Some(&registration),
        "the later owner's registration stays registered (settlement: {state:?})"
    );
    assert_eq!(
        adapter
            .current_executor_attachment_witness(&session)
            .await
            .as_ref(),
        Some(&adopted),
        "the later owner's loop stays attached (settlement: {state:?})"
    );
    assert_eq!(
        state,
        MemberUnregisterState::HandedOver,
        "the settlement reports the takeover instead of discarding the later owner's registration"
    );
}

/// R1 at the discard step, after the later owner's runtime loop exited: as
/// above, but the later owner's turn degraded the registration through a
/// failed boundary commit and its loop has exited and published its
/// teardown handoff. Settling must neither discard the later owner's
/// registration nor unregister a successor of it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pending_unregister_never_discards_a_later_owners_degraded_registration_after_its_loop_exited()
 {
    use meerkat_runtime::service_ext::SessionServiceRuntimeExt as _;

    let store = FaultInjectingRuntimeStore::new();
    let runtime_store: Arc<dyn RuntimeStore> = Arc::clone(&store) as Arc<dyn RuntimeStore>;
    let blob_store: Arc<dyn meerkat_core::BlobStore> =
        Arc::new(meerkat_store::MemoryBlobStore::new());
    let adapter = Arc::new(
        MeerkatMachine::persistent(runtime_store, blob_store).expect("construct runtime authority"),
    );
    let session = SessionId::new();
    adapter
        .register_session(session.clone())
        .await
        .expect("register a registration without a runtime loop");
    let registration = adapter
        .current_session_registration_witness(&session)
        .await
        .expect("current registration");
    let registration_use = adapter.session_registration_use(&registration).await;
    let mut settlement = MemberUnregisterSettlement::new(
        registration.clone(),
        registration_use,
        None,
        &adapter,
        None,
    );

    let stopped = Arc::new(std::sync::atomic::AtomicBool::new(false));
    adapter
        .ensure_session_with_executor(
            session.clone(),
            Box::new(CommittingLaterOwnerExecutor {
                stopped: Arc::clone(&stopped),
            }),
        )
        .await
        .expect("a later owner adopts the registration in place");
    store.fail_commit(&session, true);
    adapter
        .accept_input(
            &session,
            meerkat_runtime::Input::Prompt(meerkat_runtime::PromptInput::new(
                "a later owner's turn whose boundary commit fails",
                None,
            )),
        )
        .await
        .expect("the later owner's input is admitted");
    {
        let adapter = Arc::clone(&adapter);
        let session = session.clone();
        let stopped = Arc::clone(&stopped);
        wait_until(
            "the later owner's registration degraded and its loop exited",
            Duration::from_secs(5),
            move || {
                let adapter = Arc::clone(&adapter);
                let session = session.clone();
                let stopped = Arc::clone(&stopped);
                async move {
                    adapter.durability_reload_required(&session).await.is_some()
                        && stopped.load(std::sync::atomic::Ordering::SeqCst)
                }
            },
        )
        .await;
    }
    store.fail_commit(&session, false);

    let state = settlement.settle_within(Instant::now() + STEP).await;
    assert_eq!(
        adapter
            .current_session_registration_witness(&session)
            .await
            .as_ref(),
        Some(&registration),
        "the later owner's degraded registration is neither discarded nor unregistered (settlement: {state:?})"
    );
    assert_eq!(
        state,
        MemberUnregisterState::HandedOver,
        "the settlement reports the takeover and unregisters nothing"
    );
}

/// The no-handoff case: a durability-degraded registration that no runtime
/// loop ever held has no runtime-loop teardown handoff, and nothing retains
/// its mutation fence. Its exact unregister is refused while the session is
/// degraded (the durability gate refuses the unregister's first durable
/// write), so the reload-required discard is the only way it settles. The
/// discard takes the fence itself and publishes a durability-ready cold
/// successor, which Shutdown's teardown and a pending unregister's
/// settlement each unregister.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_degraded_registration_without_a_teardown_handoff_settles_through_the_discard() {
    use crate::runtime::provisioner::{DegradedUnregisterStep, MemberSessionDisposalArc};

    async fn degraded_without_a_handoff(
        adapter: &Arc<MeerkatMachine>,
    ) -> (
        SessionId,
        meerkat_runtime::RuntimeSessionRegistrationWitness,
    ) {
        let session = SessionId::new();
        adapter
            .register_session(session.clone())
            .await
            .expect("register a registration without a runtime loop");
        assert!(
            adapter
                .force_session_durability_reload_required_for_test(&session)
                .await,
            "the registration is durability-degraded"
        );
        let registration = adapter
            .current_session_registration_witness(&session)
            .await
            .expect("the degraded registration is current");
        (session, registration)
    }

    let runtime_store: Arc<dyn RuntimeStore> = Arc::new(InMemoryRuntimeStore::new());
    let blob_store: Arc<dyn meerkat_core::BlobStore> =
        Arc::new(meerkat_store::MemoryBlobStore::new());
    let adapter = Arc::new(
        MeerkatMachine::persistent(runtime_store, blob_store).expect("construct runtime authority"),
    );

    // The discard itself.
    let (session, registration) = degraded_without_a_handoff(&adapter).await;
    let discard = match MemberSessionDisposalArc::discard_degraded_registration_first(
        &adapter,
        None,
        &registration,
        None,
        Instant::now() + STEP,
    )
    .await
    {
        DegradedUnregisterStep::Replaced { successor, .. } => {
            let current = adapter.current_session_registration_witness(&session).await;
            let ready = adapter.durability_reload_required(&session).await.is_none();
            format!(
                "replaced (successor current: {}, durability ready: {ready})",
                successor != registration && current.as_ref() == Some(&successor)
            )
        }
        DegradedUnregisterStep::Unchanged => "unchanged".to_string(),
        DegradedUnregisterStep::HandedOver => "handed over".to_string(),
        DegradedUnregisterStep::TimedOut => "timed out".to_string(),
        DegradedUnregisterStep::Failed(failure) => format!("failed: {failure}"),
    };

    // Shutdown's teardown.
    let (teardown_target, _) = degraded_without_a_handoff(&adapter).await;
    let teardown = match tokio::time::timeout(
        STEP,
        crate::runtime::actor::teardown_session(
            &adapter,
            &Arc::new(crate::runtime::ops_adapter::MobOpsAdapter::new()),
            &teardown_target,
            None,
            Instant::now() + Duration::from_secs(5),
            meerkat_runtime::UnregisterQueuedInputs::Stop,
        ),
    )
    .await
    .expect("the teardown returns within its deadline")
    {
        ShutdownUnregisterOutcome::Unregistered => "unregistered".to_string(),
        ShutdownUnregisterOutcome::Pending { stage, .. } => format!("pending at {stage}"),
        ShutdownUnregisterOutcome::Failed(failure) => format!("failed: {failure}"),
    };
    let teardown_left = adapter
        .current_session_registration_witness(&teardown_target)
        .await
        .is_some();

    // A settlement of an unregister Shutdown reported pending.
    let (settled_session, settled_registration) = degraded_without_a_handoff(&adapter).await;
    let registration_use = adapter
        .session_registration_use(&settled_registration)
        .await;
    let mut settlement = MemberUnregisterSettlement::new(
        settled_registration,
        registration_use,
        None,
        &adapter,
        None,
    );
    let settled = settlement
        .settle_within(Instant::now() + Duration::from_secs(5))
        .await;
    let settlement_left = adapter
        .current_session_registration_witness(&settled_session)
        .await
        .is_some();

    assert_eq!(
        (
            discard.as_str(),
            teardown.as_str(),
            teardown_left,
            &settled,
            settlement_left
        ),
        (
            "replaced (successor current: true, durability ready: true)",
            "unregistered",
            false,
            &MemberUnregisterState::Settled,
            false
        ),
        "discard: {discard}; teardown: {teardown} (registration left: {teardown_left}); \
         settlement: {settled:?} (registration left: {settlement_left})"
    );
}

/// The refusal stays for a runtime loop that may still hold the session's
/// mutation fence: a durability-degraded registration whose runtime loop is
/// live and has not handed its executor back is not taken over by a later
/// owner's executor registration (refused until a repair), and its discard
/// waits for that loop's teardown handoff instead of taking the fence (the
/// step times out and nothing is replaced).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_degraded_registration_whose_live_loop_holds_the_fence_is_not_taken_over() {
    use crate::runtime::provisioner::{DegradedUnregisterStep, MemberSessionDisposalArc};

    let runtime_store: Arc<dyn RuntimeStore> = Arc::new(InMemoryRuntimeStore::new());
    let blob_store: Arc<dyn meerkat_core::BlobStore> =
        Arc::new(meerkat_store::MemoryBlobStore::new());
    let adapter = Arc::new(
        MeerkatMachine::persistent(runtime_store, blob_store).expect("construct runtime authority"),
    );
    let session = SessionId::new();
    adapter
        .register_session(session.clone())
        .await
        .expect("register the session");
    adapter
        .ensure_session_with_executor(session.clone(), Box::new(IdleLaterOwnerExecutor))
        .await
        .expect("attach a live runtime loop");
    let registration = adapter
        .current_session_registration_witness(&session)
        .await
        .expect("current registration");
    let attachment = adapter
        .current_executor_attachment_witness(&session)
        .await
        .expect("the runtime loop is attached");
    assert!(
        adapter
            .force_session_durability_reload_required_for_test(&session)
            .await,
        "the registration degrades while its loop is live"
    );

    match adapter
        .ensure_session_with_executor(session.clone(), Box::new(IdleLaterOwnerExecutor))
        .await
    {
        Err(meerkat_runtime::RuntimeDriverError::RecoveryRepairBlocked { .. }) => {}
        other => panic!(
            "a later owner's executor registration must be refused while the loop holds the fence, got {other:?}"
        ),
    }
    assert!(
        matches!(
            MemberSessionDisposalArc::discard_degraded_registration_first(
                &adapter,
                None,
                &registration,
                None,
                Instant::now() + Duration::from_secs(1),
            )
            .await,
            DegradedUnregisterStep::TimedOut
        ),
        "the discard waits for the live loop's teardown handoff"
    );
    assert_eq!(
        adapter
            .current_session_registration_witness(&session)
            .await
            .as_ref(),
        Some(&registration),
        "the degraded registration is not replaced"
    );
    assert_eq!(
        adapter
            .current_executor_attachment_witness(&session)
            .await
            .as_ref(),
        Some(&attachment),
        "the live loop stays attached"
    );
}

/// A later owner's executor registration cold-reloads a durability-degraded
/// registration that no runtime loop holds, as it does one whose loop handed
/// its executor back: the reload-required discard publishes a cold
/// successor, and the later owner's executor attaches to that successor.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_later_owners_executor_cold_reloads_a_degraded_registration_without_a_teardown_handoff() {
    let runtime_store: Arc<dyn RuntimeStore> = Arc::new(InMemoryRuntimeStore::new());
    let blob_store: Arc<dyn meerkat_core::BlobStore> =
        Arc::new(meerkat_store::MemoryBlobStore::new());
    let adapter = Arc::new(
        MeerkatMachine::persistent(runtime_store, blob_store).expect("construct runtime authority"),
    );
    let session = SessionId::new();
    adapter
        .register_session(session.clone())
        .await
        .expect("register a registration without a runtime loop");
    let registration = adapter
        .current_session_registration_witness(&session)
        .await
        .expect("current registration");
    assert!(
        adapter
            .force_session_durability_reload_required_for_test(&session)
            .await,
        "the registration is durability-degraded"
    );

    adapter
        .ensure_session_with_executor(session.clone(), Box::new(IdleLaterOwnerExecutor))
        .await
        .expect("the later owner's executor registration cold-reloads the degraded registration");
    let successor = adapter
        .current_session_registration_witness(&session)
        .await
        .expect("a registration of the session is current");
    assert_ne!(
        successor, registration,
        "the executor attached to the cold successor, not to the degraded shell"
    );
    assert!(
        adapter.durability_reload_required(&session).await.is_none(),
        "the cold successor is durability-ready"
    );
    assert!(
        adapter
            .current_executor_attachment_witness(&session)
            .await
            .is_some(),
        "the later owner's executor is attached"
    );
}

/// The no-handoff discard revalidates under the registration's mutation
/// gate: a runtime-loop teardown slot that appears after the discard was
/// admitted and the registration transaction released, and before the
/// discard takes the gate, leaves the registration as it is, and the discard
/// answers `NotCurrent` without discarding anything. The interleaving is
/// reachable: an executor registration checks durability when it claims the
/// session, releases the registration transaction while it still holds the
/// mutation gate, and attaches its loop without checking again, so a session
/// that degrades in between can admit a discard that then waits on that
/// gate. The seam injects that attach deterministically. This row exercises
/// the first revalidation only: the second and third cannot be reached by an
/// attach, since an attach needs a claim decided under the gate, and a claim
/// decided after the degradation joins the discard.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_no_handoff_discard_leaves_a_registration_a_loop_reached_before_the_fence() {
    let runtime_store: Arc<dyn RuntimeStore> = Arc::new(InMemoryRuntimeStore::new());
    let blob_store: Arc<dyn meerkat_core::BlobStore> =
        Arc::new(meerkat_store::MemoryBlobStore::new());
    let adapter = Arc::new(
        MeerkatMachine::persistent(runtime_store, blob_store).expect("construct runtime authority"),
    );
    let session = SessionId::new();
    adapter
        .register_session(session.clone())
        .await
        .expect("register a registration without a runtime loop");
    let registration = adapter
        .current_session_registration_witness(&session)
        .await
        .expect("current registration");
    assert!(
        adapter
            .force_session_durability_reload_required_for_test(&session)
            .await,
        "the registration is durability-degraded"
    );

    let (reached, release) =
        adapter.arm_reload_required_discard_before_mutation_gate_test_hook(session.clone());
    let discarding = {
        let adapter = Arc::clone(&adapter);
        let registration = registration.clone();
        tokio::spawn(async move {
            adapter
                .reload_required_registration_settlement_if_current(&registration, None, None)
                .await
        })
    };
    tokio::time::timeout(STEP, reached)
        .await
        .expect("the admitted discard reaches the mutation gate")
        .expect("the discard paused before the mutation gate");
    assert!(
        adapter
            .install_runtime_loop_teardown_slot_for_test(&session)
            .await,
        "a runtime-loop teardown slot appears before the discard takes the gate"
    );
    release.send(()).expect("release the paused discard");

    let settled = tokio::time::timeout(STEP, discarding)
        .await
        .expect("the discard returns")
        .expect("the discard task");
    assert!(
        matches!(
            settled,
            Ok(meerkat_runtime::ReloadRequiredRegistrationUseSettlement::NotCurrent)
        ),
        "the discard leaves a registration a loop reached before the fence: {settled:?}"
    );
    assert_eq!(
        adapter
            .current_session_registration_witness(&session)
            .await
            .as_ref(),
        Some(&registration),
        "nothing was discarded"
    );
    assert!(
        adapter.durability_reload_required(&session).await.is_some(),
        "the registration is still durability-degraded"
    );
}

/// The isolation mob's definition with AutonomousHost workers.
fn autonomous_isolation_definition() -> MobDefinition {
    let mut definition = sample_definition();
    let worker = definition
        .profiles
        .get_mut(&ProfileName::from("worker"))
        .expect("worker profile")
        .as_inline_mut()
        .expect("inline worker profile");
    worker.runtime_mode = crate::MobRuntimeMode::AutonomousHost;
    // Deliveries in these tests are external (the operator console path).
    worker.external_addressable = true;
    definition
}

/// Shut `mob` down whole and check that its degraded member 0 and its
/// healthy member 1 are both unregistered: member 0's run-start hold reports
/// `ReloadRequired` instead of failing on the degraded registration, and the
/// teardown discards that registration and unregisters the cold successor.
/// The Shutdown is an explicit Stop, the path that interrupts an autonomous
/// member: the default keeps a durable member's run instead.
async fn whole_shutdown_settles_the_degraded_member(mob: &IsolationMob) -> MobShutdownReport {
    let report = tokio::time::timeout(
        STEP,
        mob.handle.shutdown_with_report(
            ShutdownOptions::default()
                .with_deadline(Instant::now() + Duration::from_secs(5))
                .with_queued_inputs(crate::runtime::QueuedInputsOnShutdown::Stop),
        ),
    )
    .await
    .expect("the shutdown returns within its deadline")
    .expect("the shutdown succeeds");
    for index in 0..2 {
        assert!(
            matches!(
                report.members.get(mob.member(index)),
                Some(MemberShutdownOutcome::Unregistered)
            ),
            "member {index}: {report:?}"
        );
        assert!(
            mob.adapter
                .current_session_registration_witness(mob.session(index))
                .await
                .is_none(),
            "member {index}'s session has no registration left"
        );
    }
    assert_eq!(
        report.run_starts.get(mob.member(0)),
        Some(&MemberRunStarts::ReloadRequired),
        "the degraded member's run starts: {report:?}"
    );
    assert_eq!(
        report.run_starts.get(mob.member(1)),
        Some(&MemberRunStarts::Held),
        "the healthy member's run starts: {report:?}"
    );
    report
}

/// A whole Shutdown of a member degraded by a failed boundary commit, whose
/// exited runtime loop retains the session's mutation fence, returns: the
/// run-start hold reports the member `ReloadRequired` (it no longer fails the
/// Shutdown), and the teardown discards the degraded registration and
/// unregisters the cold successor.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_whole_shutdown_settles_a_member_degraded_by_a_failed_boundary_commit() {
    let mob = create_isolation_mob(2).await;
    mob.degrade_member(0).await;
    // The store recovers; the registration stays degraded until disposed.
    mob.store.fail_commit(mob.session(0), false);
    whole_shutdown_settles_the_degraded_member(&mob).await;
}

/// The same for a member marked `ReloadRequired` while idle: its runtime
/// loop is parked and alive, and no handoff retains the fence.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_whole_shutdown_settles_a_member_degraded_while_idle() {
    let mob = create_isolation_mob(2).await;
    assert!(
        mob.adapter
            .force_session_durability_reload_required_for_test(mob.session(0))
            .await,
        "member 0's registration is marked ReloadRequired"
    );
    whole_shutdown_settles_the_degraded_member(&mob).await;
}

/// An AutonomousHost member degraded by a failed boundary commit: its stop
/// task reports the run `ReloadRequired` and skips the comms-drain abort and
/// the run settlement, which would wait on the degraded registration until
/// the hang guard, so the Shutdown reaches its teardown.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_whole_shutdown_settles_an_autonomous_member_degraded_by_a_failed_boundary_commit() {
    let mob = create_isolation_mob_with_definition(2, autonomous_isolation_definition()).await;
    mob.degrade_autonomous_member(0).await;
    mob.store.fail_commit(mob.session(0), false);
    let report = whole_shutdown_settles_the_degraded_member(&mob).await;
    assert_eq!(
        report.runs.get(mob.member(0)),
        Some(&MemberStopRun::ReloadRequired),
        "the degraded member's run: {report:?}"
    );
}

/// The same for an AutonomousHost member marked `ReloadRequired` while idle.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_whole_shutdown_settles_an_autonomous_member_degraded_while_idle() {
    let mob = create_isolation_mob_with_definition(2, autonomous_isolation_definition()).await;
    assert!(
        mob.adapter
            .force_session_durability_reload_required_for_test(mob.session(0))
            .await,
        "member 0's registration is marked ReloadRequired"
    );
    let report = whole_shutdown_settles_the_degraded_member(&mob).await;
    assert_eq!(
        report.runs.get(mob.member(0)),
        Some(&MemberStopRun::ReloadRequired),
        "the degraded member's run: {report:?}"
    );
}

/// Drive member 0 of `mob`, an AutonomousHost member, into the window
/// between a stop's run-start hold and its cancel of the run the hold found
/// current, and return what `stop` answered. That run is the member's kickoff
/// keep-alive turn, parked until released. Before the cancel, the turn is
/// released and the run's boundary commit takes the session's mutation gate
/// while the store holds it, and the cancel waits for the gate.
/// - `mark_during_wait`: the registration is marked degraded during that wait,
///   while the parked commit still holds the gate, so the cancel always ends
///   on the degradation; the store then fails the commit.
/// - otherwise the store fails the commit during the wait, which degrades
///   the registration. The failed commit gives the gate back before its
///   runtime loop retains it again, so the cancel may take it in between:
///   which of the two happens first is up to the scheduler.
async fn degrade_between_hold_and_cancel<T>(
    mob: &IsolationMob,
    mark_during_wait: bool,
    stop: impl std::future::Future<Output = T> + Send + 'static,
) -> T
where
    T: Send + 'static,
{
    let session_id = mob.session(0).clone();
    tokio::time::timeout(STEP, mob.service.wait_keep_alive_turn_entered(&session_id))
        .await
        .expect("member 0's kickoff turn is current");
    let (cancel_reached, release_cancel, cancel_answered) =
        crate::runtime::provisioner::arm_stop_member_cancel_for_test(session_id.clone());
    let stopping = tokio::spawn(stop);
    tokio::time::timeout(STEP, cancel_reached)
        .await
        .expect("the stop holds member 0 and reaches its cancel")
        .expect("the cancel hook stays armed");

    // The run's boundary commit takes the session's mutation gate and waits
    // in the store, which fails it once released.
    mob.store.park_commits(&session_id);
    mob.store.fail_commit(&session_id, true);
    mob.service.release_keep_alive_turn(&session_id).await;
    wait_until("member 0's boundary commit to park", STEP, || async {
        mob.store.parked_commit_arrivals(&session_id) == 1
    })
    .await;

    // The cancel waits for the gate behind the commit, and the registration
    // degrades during that wait.
    let parked = mob
        .adapter
        .arm_mutation_gate_wait_observer_for_test(session_id.clone());
    release_cancel
        .send(())
        .expect("the stop waits at its cancel");
    tokio::time::timeout(STEP, parked)
        .await
        .expect("the cancel waits for the gate behind the commit")
        .expect("the wait observer stays armed until the wait");
    if mark_during_wait {
        assert!(
            mob.adapter
                .force_session_durability_reload_required_for_test(&session_id)
                .await,
            "member 0's registration degrades during the cancel's wait"
        );
        let _ = tokio::time::timeout(STEP, cancel_answered)
            .await
            .expect("the cancel answers once the registration degrades");
        mob.store.release_commits();
    } else {
        mob.store.release_commits();
        let _ = tokio::time::timeout(STEP, cancel_answered)
            .await
            .expect("the cancel answers once the registration degrades");
    }
    assert!(
        mob.adapter
            .durability_reload_required(&session_id)
            .await
            .is_some(),
        "the failed boundary commit degraded member 0's registration"
    );
    // The store recovers; the registration stays degraded until disposed.
    mob.store.fail_commit(&session_id, false);
    tokio::time::timeout(STEP, stopping)
        .await
        .expect("the stop returns")
        .expect("the stop task completes")
}

/// A Stop of an AutonomousHost member whose registration degrades between
/// the Stop's run-start hold and its boundary cancel: the cancel is refused
/// typed, and the member reports its run `ReloadRequired` with its run starts
/// `Held` instead of failing the Stop. Without that mapping the refusal
/// failed the Stop as an internal error.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stop_reports_reload_required_for_a_member_degraded_between_its_hold_and_its_cancel() {
    let mob = create_isolation_mob_with_definition(1, autonomous_isolation_definition()).await;
    let handle = mob.handle.clone();
    let report = degrade_between_hold_and_cancel(&mob, true, async move { handle.stop().await })
        .await
        .expect("the stop succeeds");
    let outcome = report
        .members
        .get(mob.member(0))
        .expect("member 0's stop outcome");
    assert_eq!(outcome.run, MemberStopRun::ReloadRequired, "{report:?}");
    assert_eq!(outcome.starts, MemberRunStarts::Held, "{report:?}");
    assert_eq!(
        report.not_holdable().count(),
        0,
        "the held member is not reported as able to start a turn: {report:?}"
    );
}

/// The same window under a whole Shutdown, whose cancel is the immediate
/// exact hard cancel, with the registration degraded by the failed boundary
/// commit itself. (A registration marked degraded while that commit is still
/// in flight leaves its carrier in a state no production path reaches, which
/// the teardown's discard refuses, so this test lets the commit fail.) The
/// cancel either ends on the degradation (`ReloadRequired`) or takes the gate
/// the failed commit gave back and finds the run ended
/// (`RunEndedBeforeCancel`). Either way the member's stop leaves the degraded
/// registration to the teardown, its run starts stay `Held`, and the
/// teardown discards the registration and unregisters the cold successor.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_whole_shutdown_settles_a_member_degraded_between_its_hold_and_its_cancel() {
    let mob = create_isolation_mob_with_definition(1, autonomous_isolation_definition()).await;
    let handle = mob.handle.clone();
    let report = degrade_between_hold_and_cancel(&mob, false, async move {
        handle
            .shutdown_with_report(
                ShutdownOptions::default()
                    .with_deadline(Instant::now() + Duration::from_secs(20))
                    .with_queued_inputs(crate::runtime::QueuedInputsOnShutdown::Stop),
            )
            .await
    })
    .await
    .expect("the shutdown succeeds");
    assert!(
        matches!(
            report.runs.get(mob.member(0)),
            Some(MemberStopRun::ReloadRequired | MemberStopRun::RunEndedBeforeCancel { .. })
        ),
        "member 0's run: {report:?}"
    );
    assert_eq!(
        report.run_starts.get(mob.member(0)),
        Some(&MemberRunStarts::Held),
        "member 0's run starts: {report:?}"
    );
    assert!(
        matches!(
            report.members.get(mob.member(0)),
            Some(MemberShutdownOutcome::Unregistered)
        ),
        "member 0: {report:?}"
    );
    assert!(
        mob.adapter
            .current_session_registration_witness(mob.session(0))
            .await
            .is_none(),
        "member 0's session has no registration left"
    );
}
