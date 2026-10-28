//! Retryable restore rows: a restore that waits on work still in flight on
//! its member's session (the takeover of the attachment an earlier handle
//! left), or on a store that cannot serve it now, never breaks the member;
//! and the explicit resume's seam retires a Dormant member's retained
//! attachment before its durable Resume, as it retires every foreign one.

use super::*;
use crate::machines::mob_machine as dsl;
use crate::runtime::MemberRestorePolicy;

const ROW_STEP: Duration = Duration::from_secs(30);

fn phase_of(
    handle: &MobHandle,
    identity: &AgentIdentity,
) -> Option<dsl::MemberMaterializationPhase> {
    handle.member_materialization_for_test(identity)
}

/// Whether MobMachine holds the mob's member plan sealed: every Dormant or
/// Waking member has a durable endpoint, so every one is peer-wakeable. A
/// stopped mob a new handle reconstructed has none sealed until its lift.
fn boot_plan_sealed(handle: &MobHandle) -> bool {
    handle.machine_state_watch_rx.borrow().boot_plan_sealed
}

/// One turn-driven worker spawned live on a session service the whole row
/// shares, whose mob's actor then went away (after a stop when `stop`): the
/// runtime still serves the worker's session with that handle's attachment
/// when a new handle is reconstructed from the same storage.
struct RetainedAttachmentMob {
    service: Arc<MockSessionService>,
    adapter: Arc<meerkat_runtime::MeerkatMachine>,
    events: Arc<dyn MobEventStore>,
    runtime_metadata: Arc<dyn MobRuntimeMetadataStore>,
    identity: AgentIdentity,
    session: SessionId,
    foreign: meerkat_runtime::RuntimeExecutorAttachmentWitness,
}

impl RetainedAttachmentMob {
    async fn new(label: &str, stop: bool) -> Self {
        let service = Arc::new(MockSessionService::new());
        let adapter = service.enable_runtime_adapter();
        let storage = MobStorage::in_memory();
        let events = storage.events.clone();
        let runtime_metadata = storage.runtime_metadata.clone();
        let handle = MobBuilder::new(with_unique_mob_id(sample_definition(), label), storage)
            .with_session_service(service.clone())
            .create()
            .await
            .expect("create the mob");
        let identity = AgentIdentity::from("retained-worker");
        let session = handle
            .spawn_with_options(
                ProfileName::from("worker"),
                identity.clone(),
                None,
                Some(crate::MobRuntimeMode::TurnDriven),
                None,
            )
            .await
            .expect("spawn the worker")
            .bridge_session_id()
            .cloned()
            .expect("the worker is session-backed");
        if stop {
            handle.stop().await.expect("stop the mob");
        }
        crash_stop_and_release_routes(handle).await;
        let foreign = adapter
            .current_executor_attachment_witness(&session)
            .await
            .expect("the runtime still serves the worker with the old handle's attachment");
        Self {
            service,
            adapter,
            events,
            runtime_metadata,
            identity,
            session,
            foreign,
        }
    }

    /// A new handle of the mob over the same service, under `policy`.
    async fn reconstruct(&self, policy: MemberRestorePolicy) -> MobHandle {
        let storage = MobStorage::with_events_and_runtime_metadata(
            self.events.clone(),
            self.runtime_metadata.clone(),
        );
        tokio::time::timeout(
            ROW_STEP,
            Box::pin(
                MobBuilder::for_resume(storage)
                    .with_session_service(self.service.clone())
                    .notify_orchestrator_on_resume(false)
                    .with_member_restore_policy(policy)
                    .resume(),
            ),
        )
        .await
        .expect("the reconstruction answers")
        .expect("the reconstruction succeeds")
    }
}

/// A wake whose restore cannot take the member's retained attachment over
/// within its budget (another holder keeps the session's turn boundary)
/// ends retryable: the member is Dormant again, never Broken, and its caller
/// gets a retryable refusal. Once the boundary is free a later wake joins the
/// takeover still in flight and materializes the member once, on a new
/// attachment.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_wake_whose_takeover_outlasts_its_budget_ends_retryable_not_broken() {
    let mob = RetainedAttachmentMob::new("retryable-takeover-wake", false).await;
    let resumed = mob.reconstruct(MemberRestorePolicy::OnDemand).await;
    resumed
        .status()
        .await
        .expect("the reconstructed mob answers");
    assert_eq!(
        phase_of(&resumed, &mob.identity),
        Some(dsl::MemberMaterializationPhase::Dormant),
        "the lazy start materialized nothing"
    );
    let creates_before = mob.service.recorded_create_requests().await.len();

    // Another holder keeps every session's turn boundary.
    let gate = mob.service.install_non_reentrant_turn_finalization_gate();
    let held = Arc::clone(&gate).lock_owned().await;
    let refused = tokio::time::timeout(ROW_STEP, resumed.ensure_member_live(&mob.identity))
        .await
        .expect("the wake answers once its takeover budget is spent");
    assert!(
        matches!(
            &refused,
            Err(MobError::MemberWakeRefused {
                refusal: crate::MemberWakeRefusal::ResumeGateUnavailable,
                ..
            })
        ),
        "a takeover still in flight ends the wake retryable: {refused:?}"
    );
    resumed.status().await.expect("the mob answers");
    assert_eq!(
        phase_of(&resumed, &mob.identity),
        Some(dsl::MemberMaterializationPhase::Dormant),
        "the member is Dormant again"
    );
    let status = resumed
        .member_status(&mob.identity)
        .await
        .expect("the member's status");
    assert_ne!(
        status.status,
        crate::runtime::handle::MobMemberStatus::Broken,
        "a retryable restore never breaks the member: {status:?}"
    );

    drop(held);
    tokio::time::timeout(ROW_STEP, resumed.ensure_member_live(&mob.identity))
        .await
        .expect("the later wake answers")
        .expect("the later wake joins the takeover and materializes the member");
    resumed.status().await.expect("the mob answers");
    assert_eq!(
        phase_of(&resumed, &mob.identity),
        Some(dsl::MemberMaterializationPhase::Live)
    );
    let replacement = mob
        .adapter
        .current_executor_attachment_witness(&mob.session)
        .await
        .expect("the member is served by a new attachment");
    assert_ne!(
        replacement, mob.foreign,
        "the old handle's attachment is replaced, never adopted"
    );
    assert_eq!(
        mob.service.recorded_create_requests().await.len(),
        creates_before + 1,
        "the member is materialized once"
    );
}

/// The explicit resume of a reconstructed stopped mob retires the attachment
/// an earlier handle left serving a Dormant member before its durable
/// Resume, and does nothing else for that member: under the on-demand policy
/// it stays Dormant and nothing is materialized.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_explicit_resume_retires_a_dormant_members_retained_attachment_and_keeps_it_dormant() {
    let mob = RetainedAttachmentMob::new("retained-attachment-lift", true).await;
    let resumed = mob.reconstruct(MemberRestorePolicy::OnDemand).await;
    assert_eq!(
        resumed
            .status()
            .await
            .expect("the reconstructed mob answers"),
        MobState::Stopped
    );
    assert_eq!(
        phase_of(&resumed, &mob.identity),
        Some(dsl::MemberMaterializationPhase::Dormant)
    );
    let creates_before = mob.service.recorded_create_requests().await.len();

    tokio::time::timeout(ROW_STEP, resumed.resume())
        .await
        .expect("the explicit resume answers")
        .expect("the explicit resume succeeds");
    assert_eq!(
        resumed.status().await.expect("the mob answers"),
        MobState::Running
    );
    assert!(
        mob.adapter
            .current_executor_attachment_witness(&mob.session)
            .await
            .is_none(),
        "the seam retired the old handle's attachment"
    );
    assert_eq!(
        phase_of(&resumed, &mob.identity),
        Some(dsl::MemberMaterializationPhase::Dormant),
        "the member stays Dormant"
    );
    assert_eq!(
        mob.service.recorded_create_requests().await.len(),
        creates_before,
        "nothing is materialized"
    );
}

/// `count` turn-driven workers built on a session service, their mob stopped
/// and its process gone: a fresh service holds their durable sessions only,
/// and the stopped mob is reconstructed from its storage under the Eager
/// policy, every member Dormant, with the host's build `preparer` and an
/// owed retry that runs within a few [`RETRY_STEP`]s.
async fn built_stopped_mob(
    label: &str,
    count: usize,
    preparer: Option<Arc<dyn crate::runtime::MemberBuildPreparer>>,
) -> (
    MobHandle,
    Arc<MockSessionService>,
    Vec<(AgentIdentity, SessionId)>,
) {
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
    let runtime_metadata = storage.runtime_metadata.clone();
    let handle = MobBuilder::new(with_unique_mob_id(sample_definition(), label), storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create the mob");
    let mut members = Vec::with_capacity(count);
    for index in 0..count {
        let identity = AgentIdentity::from(format!("lifted-{index}"));
        let mut spec = SpawnMemberSpec::new("worker", identity.clone());
        spec.runtime_mode = Some(crate::MobRuntimeMode::TurnDriven);
        handle.spawn_spec(spec).await.expect("spawn a worker");
        let session = handle
            .resolve_bridge_session_id(&identity)
            .await
            .expect("a session-backed worker");
        members.push((identity, session));
    }
    handle.stop().await.expect("stop the mob");
    let restarted = Arc::new(service.cold_restart_preserving_durable_state().await);
    let _ = restarted.enable_runtime_adapter();
    crash_stop_and_release_routes(handle).await;
    drop(service);
    let resumed = tokio::time::timeout(
        ROW_STEP,
        Box::pin(
            {
                let builder = MobBuilder::for_resume(MobStorage::with_events_and_runtime_metadata(
                    events,
                    runtime_metadata,
                ))
                .with_session_service(restarted.clone())
                .notify_orchestrator_on_resume(false)
                .with_member_restore_policy(MemberRestorePolicy::Eager)
                .with_member_wake_retry_backoff(RETRY_STEP, RETRY_STEP * 2);
                match preparer {
                    Some(preparer) => builder.with_member_build_preparer(preparer),
                    None => builder,
                }
            }
            .resume(),
        ),
    )
    .await
    .expect("the reconstruction answers")
    .expect("the reconstruction succeeds");
    assert_eq!(
        resumed
            .status()
            .await
            .expect("the reconstructed mob answers"),
        MobState::Stopped
    );
    (resumed, restarted, members)
}

/// The lift of a stopped mob rebuilds the members its Eager plan
/// materializes through the explicit-resume work path. One member's durable
/// observation meets a store that cannot serve it now: that member ends
/// Dormant with its owed retry, never Broken. That is the member's own
/// outcome: the lift answers Ok with the mob Running, every other member is
/// Live, and the member's owed retry materializes it once the store serves
/// it again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lifted_member_whose_durable_observation_fails_is_retried_and_the_lift_answers_ok() {
    let (resumed, service, members) =
        built_stopped_mob("lift-observation-store-failure", 3, None).await;
    let (failing, failing_session) = members[1].clone();
    service.fail_next_resume_authority_observes_for(&failing_session, 1);
    // The explicit-resume work path's observations pass its probe (none is
    // held); the actor's wake path never does.
    let observations =
        crate::runtime::actor::probe_explicit_resume_observations_for_test(resumed.mob_id());
    observations.release(members.len());

    let lifted = tokio::time::timeout(ROW_STEP, resumed.resume())
        .await
        .expect("the explicit resume answers");
    assert_eq!(
        observations.arrived(),
        members.len(),
        "the lift observed every member, the failing one included, on the explicit-resume work path"
    );
    lifted.expect("a member that ends retryable is its own outcome: the lift answers Ok");
    assert_eq!(
        resumed.status().await.expect("the mob answers"),
        MobState::Running
    );
    for (identity, _) in members.iter().filter(|(identity, _)| identity != &failing) {
        assert_eq!(
            phase_of(&resumed, identity),
            Some(dsl::MemberMaterializationPhase::Live),
            "{identity} is Live"
        );
    }
    assert_left_dormant_with_its_retry(&resumed, &failing).await;
}

/// `member` ended retryable at the lift: it is not Broken and carries no
/// restore failure, and its owed retry, with no delivery, materializes it.
async fn assert_left_dormant_with_its_retry(handle: &MobHandle, member: &AgentIdentity) {
    let status = handle
        .member_status(member)
        .await
        .expect("the member's status");
    assert_ne!(
        status.status,
        crate::runtime::handle::MobMemberStatus::Broken,
        "a member that ends retryable is never Broken: {status:?}"
    );
    assert!(
        status.error.is_none(),
        "the member carries no restore failure: {status:?}"
    );
    let mut published = handle.machine_state_watch_rx.clone();
    let member_id = dsl::AgentIdentity::from_domain(member);
    let retried = tokio::time::timeout(
        ROW_STEP,
        published.wait_for(|state| {
            state.member_materialization.get(&member_id)
                == Some(&dsl::MemberMaterializationPhase::Live)
        }),
    )
    .await
    .is_ok_and(|seen| seen.is_ok());
    assert!(retried, "the member's owed retry materializes it");
}

/// The backoff of the owed retry in the rows that check no retry runs.
const RETRY_STEP: Duration = Duration::from_millis(20);

/// The mob of [`seat_member_with_provider_params_then_crash`] restarted under
/// `policy`, with a resume customizer (so the restore reads the member's view)
/// and an owed retry that would run within a few [`RETRY_STEP`]s.
async fn restarted_with_customized_restore(
    storage: MobStorage,
    service: &Arc<MockSessionService>,
    policy: MemberRestorePolicy,
) -> MobHandle {
    tokio::time::timeout(
        ROW_STEP,
        Box::pin(
            MobBuilder::for_resume(storage)
                .with_session_service(service.clone())
                .with_spawn_member_customizer(Arc::new(PromptCacheMigration::default()))
                .with_member_restore_policy(policy)
                .with_member_wake_retry_backoff(RETRY_STEP, RETRY_STEP * 2)
                .resume(),
        ),
    )
    .await
    .expect("the restart answers")
    .expect("the restart succeeds")
}

/// `member` is Broken with the malformed state's typed reason, and stays so:
/// no owed retry reads its session again.
async fn assert_broken_by_the_malformed_state(
    handle: &MobHandle,
    service: &MockSessionService,
    member: &AgentIdentity,
    session: &SessionId,
) {
    let status = handle
        .member_status(member)
        .await
        .expect("the member's status");
    assert_eq!(
        status.status,
        crate::runtime::handle::MobMemberStatus::Broken,
        "a malformed persisted state breaks the member: {status:?}"
    );
    assert!(
        status
            .error
            .as_deref()
            .is_some_and(|error| error.contains("mock malformed session metadata")),
        "the member carries the typed reason: {status:?}"
    );
    let reads = service.persisted_session_metadata_reads_for(session);
    tokio::time::sleep(RETRY_STEP * 20).await;
    assert_eq!(
        service.persisted_session_metadata_reads_for(session),
        reads,
        "no owed retry reads the member's session again"
    );
    assert_eq!(
        handle
            .member_status(member)
            .await
            .expect("the member's status")
            .status,
        crate::runtime::handle::MobMemberStatus::Broken
    );
}

/// A wake whose view read finds the member's persisted state malformed (a
/// typed decode failure: the same state reads the same way again) breaks the
/// member with the typed reason. The wake never ends retryable, so no owed
/// retry reads the session again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_wake_whose_view_read_finds_the_state_malformed_breaks_the_member() {
    let (service, storage, member, session) =
        seat_member_with_provider_params_then_crash(false).await;
    let resumed =
        restarted_with_customized_restore(storage, &service, MemberRestorePolicy::OnDemand).await;
    assert_eq!(
        resumed.status().await.expect("the restarted mob answers"),
        MobState::Running
    );
    assert_eq!(
        phase_of(&resumed, &member),
        Some(dsl::MemberMaterializationPhase::Dormant),
        "the lazy start materialized nothing"
    );
    service.find_persisted_session_metadata_malformed_for(session.clone());

    let refused = tokio::time::timeout(ROW_STEP, resumed.ensure_member_live(&member))
        .await
        .expect("the wake answers");
    assert!(
        matches!(
            &refused,
            Err(MobError::MemberRestoreFailed { reason, .. })
                if reason.contains("mock malformed session metadata")
        ),
        "the wake answers the member's typed restore failure: {refused:?}"
    );
    assert_broken_by_the_malformed_state(&resumed, &service, &member, &session).await;
}

/// The lift of a stopped mob whose planned member's view read finds its
/// persisted state malformed breaks that member with the typed reason, the
/// explicit resume answers that typed error, and no owed retry reads the
/// session again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lift_whose_view_read_finds_the_state_malformed_breaks_the_member() {
    let (service, storage, member, session) =
        seat_member_with_provider_params_then_crash(true).await;
    let resumed =
        restarted_with_customized_restore(storage, &service, MemberRestorePolicy::Eager).await;
    assert_eq!(
        resumed
            .status()
            .await
            .expect("the reconstructed mob answers"),
        MobState::Stopped
    );
    service.find_persisted_session_metadata_malformed_for(session.clone());
    assert!(
        !boot_plan_sealed(&resumed),
        "the stopped mob's plan is not sealed before its lift"
    );

    let error = tokio::time::timeout(ROW_STEP, resumed.resume())
        .await
        .expect("the explicit resume answers")
        .expect_err("the explicit resume answers the member's typed restore failure");
    assert!(
        matches!(
            shared_cause(&error),
            MobError::SessionError(SessionError::Store(_))
        ),
        "a typed store error: {error:?}"
    );
    assert_eq!(
        resumed.status().await.expect("the mob answers"),
        MobState::Running
    );
    assert!(
        boot_plan_sealed(&resumed),
        "a lift that committed seals its member plan whatever it answers"
    );
    assert_broken_by_the_malformed_state(&resumed, &service, &member, &session).await;
}

/// A lift whose planned member cannot be repointed to its successor session
/// (the journal write of its recovered binding fails) ends that member alone,
/// with its own typed error. Every other member the lift plans is rebuilt and
/// Live: one member's failure never ends another, nor lends it its error.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lifted_member_whose_repoint_fails_ends_alone_with_its_own_error() {
    let events = Arc::new(FaultInjectedMobEventStore::new());
    let (handle, service, runs, specs, runtime_metadata, identity_store, identity_status) =
        create_test_mob_with_recoverable_fault_events(sample_definition(), events.clone()).await;
    // Ordered so the failing member is rebuilt first.
    let repointed = AgentIdentity::from("a-repointed");
    let plain = AgentIdentity::from("b-plain");
    let mut sessions = Vec::new();
    for member in [&repointed, &plain] {
        let mut spec = SpawnMemberSpec::new("worker", member.clone());
        spec.runtime_mode = Some(crate::MobRuntimeMode::TurnDriven);
        handle.spawn_spec(spec).await.expect("spawn a worker");
        sessions.push(
            handle
                .resolve_bridge_session_id(member)
                .await
                .expect("a session-backed worker"),
        );
    }
    handle.stop().await.expect("stop the mob");
    let restarted = Arc::new(service.cold_restart_preserving_durable_state().await);
    let _ = restarted.enable_runtime_adapter();
    crash_stop_and_release_routes(handle).await;
    drop(service);
    // The repointed member's bound session is gone; its successor is in the
    // realm.
    restarted.delete_persisted_session(&sessions[0]).await;
    let successor = create_realm_session_for_test(&restarted, Some(&repointed)).await;
    let resumed = tokio::time::timeout(
        ROW_STEP,
        Box::pin(
            MobBuilder::for_resume(MobStorage::custom_with_runtime_metadata(
                events.clone(),
                runs,
                specs,
                runtime_metadata,
                identity_store,
                identity_status,
            ))
            .with_session_service(restarted.clone())
            .notify_orchestrator_on_resume(false)
            .with_member_restore_policy(MemberRestorePolicy::Eager)
            .resume(),
        ),
    )
    .await
    .expect("the reconstruction answers")
    .expect("the reconstruction succeeds");
    assert_eq!(
        resumed
            .status()
            .await
            .expect("the reconstructed mob answers"),
        MobState::Stopped
    );
    events
        .fail_appends_for("MemberSessionBindingRecovered")
        .await;

    let _lifted = tokio::time::timeout(ROW_STEP, resumed.resume())
        .await
        .expect("the explicit resume answers");
    assert_eq!(
        phase_of(&resumed, &plain),
        Some(dsl::MemberMaterializationPhase::Live),
        "the other planned member is rebuilt"
    );
    let plain_status = resumed
        .member_status(&plain)
        .await
        .expect("the other member's status");
    assert!(
        plain_status.error.is_none(),
        "the other member carries no error of another member: {plain_status:?}"
    );
    let status = resumed
        .member_status(&repointed)
        .await
        .expect("the repointed member's status");
    assert!(
        status.error.as_deref().is_some_and(|error| error
            .contains("fault-injected append failure for MemberSessionBindingRecovered")),
        "the member ends with its own typed error: {status:?}"
    );
    assert_ne!(
        status.current_session_id.as_ref(),
        Some(&successor),
        "a member whose repoint failed is not bound to its successor"
    );
}

/// A lift's planned member whose bound session is gone is rebuilt on its
/// successor although another member's persisted state reads malformed. The
/// lift prepares its members one unit at a time, and a successor search
/// never reads a session bound to any member of the roster: the other
/// member's unreadable session is never read for this member's successor,
/// so it cannot fail this member's rebuild.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lifted_member_is_repointed_past_another_members_unreadable_session() {
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    let storage = MobStorage::in_memory();
    let events = storage.events.clone();
    let runtime_metadata = storage.runtime_metadata.clone();
    // The sample mob's own id: the successor is named for it.
    let handle = MobBuilder::new(sample_definition(), storage)
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create the mob");
    let repointed = AgentIdentity::from("a-repointed");
    let unreadable = AgentIdentity::from("b-unreadable");
    let mut sessions = Vec::new();
    for member in [&repointed, &unreadable] {
        let mut spec = SpawnMemberSpec::new("worker", member.clone());
        spec.runtime_mode = Some(crate::MobRuntimeMode::TurnDriven);
        handle.spawn_spec(spec).await.expect("spawn a worker");
        sessions.push(
            handle
                .resolve_bridge_session_id(member)
                .await
                .expect("a session-backed worker"),
        );
    }
    handle.stop().await.expect("stop the mob");
    let restarted = Arc::new(service.cold_restart_preserving_durable_state().await);
    let _ = restarted.enable_runtime_adapter();
    crash_stop_and_release_routes(handle).await;
    drop(service);
    // The repointed member's bound session is gone; its successor is in the
    // realm. The other member's session is bound and reads malformed.
    restarted.delete_persisted_session(&sessions[0]).await;
    let successor = create_realm_session_for_test(&restarted, Some(&repointed)).await;
    let resumed = tokio::time::timeout(
        ROW_STEP,
        Box::pin(
            MobBuilder::for_resume(MobStorage::with_events_and_runtime_metadata(
                events,
                runtime_metadata,
            ))
            .with_session_service(restarted.clone())
            .notify_orchestrator_on_resume(false)
            .with_member_restore_policy(MemberRestorePolicy::Eager)
            .resume(),
        ),
    )
    .await
    .expect("the reconstruction answers")
    .expect("the reconstruction succeeds");
    assert_eq!(
        resumed
            .status()
            .await
            .expect("the reconstructed mob answers"),
        MobState::Stopped
    );
    // The realm lists the other member's persisted session, as a persistent
    // service lists every session it stores (the mock lists the sessions it
    // holds data for), so the successor search meets it; it reads malformed.
    let other_session = restarted
        .persisted_session_clone(&sessions[1])
        .await
        .expect("the other member's persisted session");
    restarted.replace_live_session(other_session).await;
    restarted.find_persisted_session_metadata_malformed_for(sessions[1].clone());

    let _lifted = tokio::time::timeout(ROW_STEP, resumed.resume())
        .await
        .expect("the explicit resume answers");
    assert_eq!(
        phase_of(&resumed, &repointed),
        Some(dsl::MemberMaterializationPhase::Live),
        "the repointed member is rebuilt"
    );
    let status = resumed
        .member_status(&repointed)
        .await
        .expect("the repointed member's status");
    assert_eq!(
        status.current_session_id.as_ref(),
        Some(&successor),
        "the member is rebuilt on its successor: {status:?}"
    );
    assert!(
        status.error.is_none(),
        "the member carries no error of another member: {status:?}"
    );
}

/// The cause an explicit resume answered: a shared lifecycle operation's
/// failure reaches every caller inside the shared envelope.
fn shared_cause(error: &MobError) -> &MobError {
    match error {
        MobError::SharedLifecycleFailure(inner) => inner.as_ref(),
        other => other,
    }
}

/// A host that asks to prepare one member's build later, once, and prepares
/// every other build.
struct PreparesLaterOnce {
    member: AgentIdentity,
    asked: AtomicBool,
}

#[async_trait]
impl crate::runtime::MemberBuildPreparer for PreparesLaterOnce {
    async fn prepare_member_build(
        &self,
        request: crate::runtime::MemberBuildPreparation,
    ) -> Result<crate::runtime::MemberBuildPrepared, crate::runtime::MemberBuildPreparationRefusal>
    {
        if request.agent_identity == self.member && !self.asked.swap(true, Ordering::SeqCst) {
            return Err(crate::runtime::MemberBuildPreparationRefusal::Retryable {
                reason: "the host is not ready for this member yet".to_string(),
            });
        }
        Ok(crate::runtime::MemberBuildPrepared::Prepared)
    }
}

/// The lift of a stopped mob whose host asks to prepare a planned member's
/// build later ends that member retryable: Dormant, never Broken, its owed
/// retry armed. That is the member's own outcome: the lift answers Ok with
/// the mob Running, every other member is Live, and the owed retry
/// materializes the member once the host prepares its build.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lifted_member_whose_host_asks_to_prepare_later_is_retried_and_the_lift_answers_ok() {
    let deferred = AgentIdentity::from("lifted-1");
    let preparer = Arc::new(PreparesLaterOnce {
        member: deferred.clone(),
        asked: AtomicBool::new(false),
    });
    let (resumed, _service, members) =
        built_stopped_mob("lift-host-prepares-later", 3, Some(preparer)).await;
    assert!(
        !boot_plan_sealed(&resumed),
        "the stopped mob's plan is not sealed before its lift"
    );

    tokio::time::timeout(ROW_STEP, resumed.resume())
        .await
        .expect("the explicit resume answers")
        .expect(
            "a host that asks to prepare later is the member's own outcome: the lift answers Ok",
        );
    assert_eq!(
        resumed.status().await.expect("the mob answers"),
        MobState::Running
    );
    assert!(
        boot_plan_sealed(&resumed),
        "the lift sealed its member plan: the member left Dormant is peer-wakeable"
    );
    for (identity, _) in members.iter().filter(|(identity, _)| identity != &deferred) {
        assert_eq!(
            phase_of(&resumed, identity),
            Some(dsl::MemberMaterializationPhase::Live),
            "{identity} is Live"
        );
    }
    assert_left_dormant_with_its_retry(&resumed, &deferred).await;
}

/// A fork child the lift rebuilds in the same round as its source, whose
/// source the host asks to prepare later, is still built with the overlay
/// its source would be rebuilt with: the lift resolved it from the source's
/// own rebuild request while preparing the child, so the source's deferral
/// does not leave the child without it. The source's deferral is its own
/// outcome: the lift answers Ok.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lifted_fork_child_whose_source_the_host_defers_is_built_as_its_source() {
    let (service, storage, source_identity, child_identity, first, expected_source) =
        seat_fork_child_then_crash(true).await;
    let preparer = Arc::new(PreparesLaterOnce {
        member: source_identity.clone(),
        asked: AtomicBool::new(false),
    });
    let resumed = tokio::time::timeout(
        ROW_STEP,
        Box::pin(
            MobBuilder::for_resume(storage)
                .with_session_service(service.clone())
                .with_spawn_member_customizer(Arc::new(IdentityOverlayCustomizer {
                    source: source_identity.clone(),
                }))
                .with_member_restore_policy(MemberRestorePolicy::Eager)
                .with_member_build_preparer(preparer)
                .resume(),
        ),
    )
    .await
    .expect("the reconstruction answers")
    .expect("the reconstruction succeeds");

    tokio::time::timeout(ROW_STEP, resumed.resume())
        .await
        .expect("the explicit resume answers")
        .expect("the source's deferral is its own outcome: the lift answers Ok");
    assert_eq!(
        phase_of(&resumed, &child_identity),
        Some(dsl::MemberMaterializationPhase::Live),
        "the child is rebuilt in its source's round"
    );
    let rebuilt = last_member_build(&service, &child_identity).await;
    assert_rebuilt_as_first_build(&first, &rebuilt, &expected_source);
    for tool in FORK_SOURCE_OVERLAY_TOOLS {
        assert!(
            rebuilt.external_tool_names.iter().any(|name| name == tool),
            "the child carries the overlay its source would be rebuilt with ('{tool}'): {:?}",
            rebuilt.external_tool_names
        );
    }
    assert!(
        !rebuilt
            .external_tool_names
            .iter()
            .any(|name| name == GENERIC_RESUME_OVERLAY_TOOL),
        "the child is not given the host's overlay for its own identity: {:?}",
        rebuilt.external_tool_names
    );
}
