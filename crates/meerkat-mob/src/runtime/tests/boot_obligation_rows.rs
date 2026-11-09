//! Boot obligation rows: the runtime obligation scan a mob start plans its
//! members with. A background operation completion that a member's runtime
//! still owes after a crash wakes that member at the start, under the
//! default policy and with no touch, and the woken runtime delivers it. A
//! scan that fails leaves the start running, every member planned to wake.
//!
//! Unrun until the candidate's validation.

use super::member_wake_rows::refused_for_build_preparation_retry;
use super::*;
use crate::machines::mob_machine as dsl;
use crate::runtime::{MobBootSetBasis, MobBootSetUnobservedCause};
use meerkat_core::ops_lifecycle::{
    OperationId, OperationKind, OperationResult, OperationSpec, OpsLifecycleRegistry,
};
use meerkat_runtime::store::{RuntimeDeliveryAuthorityRecord, RuntimeStore};
use meerkat_runtime::{
    InMemoryRuntimeStore, LogicalRuntimeId, MeerkatMachine, PersistedOpsSnapshot,
    RuntimeOpsLifecycleRegistry,
};

const ROW_STEP: Duration = Duration::from_mins(1);

fn worker(index: usize) -> AgentIdentity {
    AgentIdentity::from(format!("obligation-{index:02}"))
}

fn turn_driven(identity: &AgentIdentity) -> SpawnMemberSpec {
    let mut spec = SpawnMemberSpec::new("worker", identity.clone());
    spec.runtime_mode = Some(crate::MobRuntimeMode::TurnDriven);
    spec
}

fn phase_of(
    handle: &MobHandle,
    identity: &AgentIdentity,
) -> Option<dsl::MemberMaterializationPhase> {
    handle.member_materialization_for_test(identity)
}

/// The host preparation MobMachine records `identity` owing.
fn preparation_of(
    handle: &MobHandle,
    identity: &AgentIdentity,
) -> Option<dsl::HostPreparationCause> {
    handle
        .machine_state_watch_rx
        .borrow()
        .member_host_preparation_causes
        .get(&dsl::AgentIdentity::from_domain(identity))
        .copied()
}

/// A persistent runtime over `store`, as one process opens it.
fn runtime_over(store: &Arc<InMemoryRuntimeStore>) -> Arc<MeerkatMachine> {
    Arc::new(
        MeerkatMachine::persistent(
            Arc::clone(store) as Arc<dyn RuntimeStore>,
            Arc::new(meerkat_store::MemoryBlobStore::new()),
        )
        .expect("construct runtime authority"),
    )
}

/// The ops lifecycle snapshot of a runtime whose session `owner` ran one
/// background tool operation to completion, its completion taken by no
/// consumer: what the runtime persisted before a crash that came before the
/// delivery.
fn undelivered_completion(owner: &SessionId) -> PersistedOpsSnapshot {
    let registry = RuntimeOpsLifecycleRegistry::new();
    let spec = OperationSpec {
        id: OperationId::new(),
        kind: OperationKind::BackgroundToolOp,
        owner_session_id: owner.clone(),
        display_name: "background operation".into(),
        source_label: "boot-obligation-rows".into(),
        operation_source: None,
        child_session_id: None,
        expect_peer_channel: false,
    };
    let operation_id = spec.id.clone();
    registry
        .register_operation(spec)
        .expect("register the operation");
    registry
        .provisioning_succeeded(&operation_id)
        .expect("start the operation");
    registry
        .complete_operation(
            &operation_id,
            OperationResult {
                id: operation_id.clone(),
                content: "done".into(),
                is_error: false,
                duration_ms: 1,
                tokens_used: 0,
            },
        )
        .expect("complete the operation");
    registry
        .capture_persistence_snapshot(
            meerkat_core::RuntimeEpochId::new(),
            &meerkat_core::EpochCursorState::new(),
        )
        .expect("the ops lifecycle snapshot")
}

/// A mob whose turn-driven workers ran over a durable runtime store and
/// crashed. The restarted session service opens a new runtime over the same
/// store.
struct CrashedStoreMob {
    events: Arc<dyn MobEventStore>,
    runtime_metadata: Arc<dyn MobRuntimeMetadataStore>,
    /// The durable runtime store both lives share.
    store: Arc<InMemoryRuntimeStore>,
    /// The restarted service: durable session state only.
    service: Arc<MockSessionService>,
    /// Each worker and its session, in spawn order.
    sessions: Vec<(AgentIdentity, SessionId)>,
}

impl CrashedStoreMob {
    /// Spawn `members` workers, crash, and restart the session service over
    /// the same runtime store.
    async fn new(label: &str, members: usize) -> Self {
        Self::with_stop(label, members, false).await
    }

    /// [`Self::new`], with the mob stopped before the crash when `stop`.
    async fn with_stop(label: &str, members: usize, stop: bool) -> Self {
        let definition = with_unique_mob_id(sample_definition(), label);
        let store = Arc::new(InMemoryRuntimeStore::new());
        let service = Arc::new(MockSessionService::new());
        service.set_runtime_adapter(runtime_over(&store));
        let storage = MobStorage::in_memory();
        let events = storage.events.clone();
        let runtime_metadata = storage.runtime_metadata.clone();
        let handle = MobBuilder::new(definition, storage)
            .with_session_service(service.clone())
            .create()
            .await
            .expect("create the mob");
        let mut sessions = Vec::with_capacity(members);
        for index in 0..members {
            let identity = worker(index);
            handle
                .spawn_spec(turn_driven(&identity))
                .await
                .expect("spawn a worker");
            let session = handle
                .resolve_bridge_session_id(&identity)
                .await
                .expect("a session-backed worker");
            sessions.push((identity, session));
        }
        if stop {
            handle.stop().await.expect("stop before the crash");
        }
        let restarted = Arc::new(service.cold_restart_preserving_durable_state().await);
        crash_stop_and_release_routes(handle).await;
        drop(service);
        restarted.set_runtime_adapter(runtime_over(&store));
        Self {
            events,
            runtime_metadata,
            store,
            service: restarted,
            sessions,
        }
    }

    async fn resume(&self) -> MobHandle {
        let builder = MobBuilder::for_resume(MobStorage::with_events_and_runtime_metadata(
            self.events.clone(),
            self.runtime_metadata.clone(),
        ))
        .with_session_service(self.service.clone())
        .notify_orchestrator_on_resume(false);
        tokio::time::timeout(ROW_STEP, Box::pin(builder.resume()))
            .await
            .expect("the resume answers")
            .expect("the resume succeeds")
    }

    /// [`Self::resume`], with the host's build `preparer` and the mob's wake
    /// retry backoff at `backoff`.
    async fn resume_with_preparer(
        &self,
        preparer: Arc<dyn crate::runtime::MemberBuildPreparer>,
        backoff: Duration,
    ) -> MobHandle {
        let builder = MobBuilder::for_resume(MobStorage::with_events_and_runtime_metadata(
            self.events.clone(),
            self.runtime_metadata.clone(),
        ))
        .with_session_service(self.service.clone())
        .notify_orchestrator_on_resume(false)
        .with_member_build_preparer(preparer)
        .with_member_wake_retry_backoff(backoff, backoff);
        tokio::time::timeout(ROW_STEP, Box::pin(builder.resume()))
            .await
            .expect("the resume answers")
            .expect("the resume succeeds")
    }

    /// The sessions the restarted service built an agent for, in order.
    async fn materialized(&self) -> Vec<SessionId> {
        self.service
            .recorded_create_requests()
            .await
            .into_iter()
            .map(|record| record.session_id)
            .collect()
    }

    fn session(&self, index: usize) -> SessionId {
        self.sessions[index].1.clone()
    }
}

/// A background operation completion that one member's runtime still owed
/// at the crash is a boot obligation: under the default (`OnDemand`) policy
/// the start materializes exactly that member, with no host delivery to it,
/// and its runtime delivers the completion as a turn. Every other member
/// stays Dormant and runs nothing, and the plan was based on an observed
/// store.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pending_background_op_completion_across_a_restart_is_delivered_with_no_touch() {
    let mob = CrashedStoreMob::new("boot-rows-pending-completion", 4).await;
    let owed = mob.session(2);
    mob.store
        .persist_ops_lifecycle(
            &LogicalRuntimeId::for_session(&owed),
            &undelivered_completion(&owed),
        )
        .await
        .expect("the completion is durable");

    let handle = mob.resume().await;
    // The builder answers once the actor is spawned; the boot set's
    // readiness, which settles its Waking members Live, runs before the
    // actor serves its first command. One round trip reads the phases after
    // it.
    assert_eq!(
        handle.status().await.expect("the mob's phase"),
        MobState::Running
    );
    assert_eq!(handle.boot_set_basis(), MobBootSetBasis::Observed);
    assert_eq!(
        mob.materialized().await,
        vec![owed.clone()],
        "the start woke exactly the member that owes the completion"
    );
    for (index, (identity, _)) in mob.sessions.iter().enumerate() {
        let expected = if index == 2 {
            dsl::MemberMaterializationPhase::Live
        } else {
            dsl::MemberMaterializationPhase::Dormant
        };
        assert_eq!(phase_of(&handle, identity), Some(expected), "{identity}");
    }

    tokio::time::timeout(ROW_STEP, async {
        while !mob.service.has_started_turn_for(&owed).await {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("the woken runtime delivers the completion as a turn");
    for (identity, session) in &mob.sessions {
        if session != &owed {
            assert!(
                !mob.service.has_started_turn_for(session).await,
                "{identity} ran no turn"
            );
        }
    }
}

/// A scan that fails does not fail the start. Here the store holds a
/// delivery authority that does not decode, so the pending-delivery listing
/// fails: the start still runs, every member is planned to wake and is
/// materialized, and the plan's basis names the failure.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_scan_leaves_the_start_running_with_every_member_planned_wake() {
    let mob = CrashedStoreMob::new("boot-rows-failed-scan", 3).await;
    mob.store
        .compare_and_swap_runtime_delivery_authority(
            &LogicalRuntimeId::new("boot-rows-undecodable-delivery-authority"),
            None,
            RuntimeDeliveryAuthorityRecord::from_parts(1, b"not a delivery authority".to_vec()),
            None,
        )
        .await
        .expect("the store keeps the record it is given");

    let handle = mob.resume().await;
    assert_eq!(
        handle.status().await.expect("the mob's phase"),
        MobState::Running
    );
    assert_eq!(
        handle.boot_set_basis(),
        MobBootSetBasis::Unobserved {
            cause: MobBootSetUnobservedCause::ScanFailed,
        }
    );
    let mut materialized = mob.materialized().await;
    materialized.sort();
    let mut expected = mob
        .sessions
        .iter()
        .map(|(_, session)| session.clone())
        .collect::<Vec<_>>();
    expected.sort();
    assert_eq!(materialized, expected, "the start woke every member");
    for (identity, _) in &mob.sessions {
        assert_eq!(
            phase_of(&handle, identity),
            Some(dsl::MemberMaterializationPhase::Live),
            "{identity}"
        );
    }
}

/// A stopped mob's start registers and materializes nothing for its dormant
/// members, and work a member's runtime still owes is not dropped by that:
/// a background operation completion it owes is boot work at the lift,
/// which wakes exactly that member and delivers the completion with no
/// touch. Every other member stays Dormant and runs nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn work_a_dormant_member_of_a_stopped_mob_owes_is_delivered_at_the_lift() {
    let mob = CrashedStoreMob::with_stop("boot-rows-stopped-owed-work", 3, true).await;
    let owed = mob.session(1);
    mob.store
        .persist_ops_lifecycle(
            &LogicalRuntimeId::for_session(&owed),
            &undelivered_completion(&owed),
        )
        .await
        .expect("the completion is durable");

    let handle = mob.resume().await;
    assert_eq!(
        handle.status().await.expect("the mob's phase"),
        MobState::Stopped
    );
    assert!(
        mob.materialized().await.is_empty(),
        "the stopped start materialized nothing"
    );
    for (identity, _) in &mob.sessions {
        assert_eq!(
            phase_of(&handle, identity),
            Some(dsl::MemberMaterializationPhase::Dormant),
            "{identity}"
        );
    }

    tokio::time::timeout(ROW_STEP, handle.resume())
        .await
        .expect("the lift answers")
        .expect("the lift succeeds");
    tokio::time::timeout(ROW_STEP, async {
        while !mob.service.has_started_turn_for(&owed).await {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("the lift wakes the member and its runtime delivers the completion");
    assert_eq!(
        mob.materialized().await,
        vec![owed.clone()],
        "the lift woke exactly the member that owes the completion"
    );
    for (identity, session) in &mob.sessions {
        if session != &owed {
            assert_eq!(
                phase_of(&handle, identity),
                Some(dsl::MemberMaterializationPhase::Dormant),
                "{identity}"
            );
            assert!(
                !mob.service.has_started_turn_for(session).await,
                "{identity} ran no turn"
            );
        }
    }
}

/// The profile the member's host grants it to spawn.
const GRANTED_PROFILE: &str = "worker";

/// Row (RED): a member whose runtime still owed a background operation
/// completion at a crash is restored by the start that follows, and that
/// restore build carries the spawn grant its host mints there. The build
/// reaches the host's member build hook with the witness of the mob runtime
/// performing it, so the hook, which holds no mob handle, enables the
/// member's mob tools and grants it spawn scope for one profile in the mob as
/// it runs. The build's authority spawns that profile in the mob as it runs,
/// and no other. The session service here builds no agent: it binds the
/// build's authority to the build's external tools as the agent factory
/// does, so the runtime's receiver gets it, and the runtime's read of the
/// restored member's spawn grant finds that profile too (the factory's own
/// binding is covered by
/// `a_member_spawn_grant_is_read_through_the_runtime_receiver`). The
/// restored member then delivers the completion.
///
/// RED on the old tree: a build carried no witness (the build options had no
/// `mob_incarnation`; the row does not compile there), so a hook without the
/// mob's handle minted nothing in the boot's restore build.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_member_restored_at_boot_with_pending_work_keeps_its_spawn_grant_in_that_build() {
    use meerkat_runtime::mob_operator_authority::{
        create_only_mob_operator_authority, grant_spawn_profiles_in_mob,
    };

    let mob = CrashedStoreMob::new("boot-rows-member-keeps-spawn-grant", 3).await;
    let member = mob.session(1);
    mob.store
        .persist_ops_lifecycle(
            &LogicalRuntimeId::for_session(&member),
            &undelivered_completion(&member),
        )
        .await
        .expect("the completion is durable");
    // The host's member build hook holds no mob handle: it mints from the
    // build alone.
    mob.service
        .set_member_build_hook(Arc::new(|req: &mut CreateSessionRequest| {
            let Some(build) = req.build.as_mut() else {
                return;
            };
            let (Some(binding), Some(witness)) =
                (build.mob_member_binding.as_ref(), build.mob_incarnation)
            else {
                return;
            };
            let grant = create_only_mob_operator_authority()
                .and_then(|authority| {
                    grant_spawn_profiles_in_mob(
                        &authority,
                        binding.mob_id.clone(),
                        witness,
                        [GRANTED_PROFILE],
                    )
                })
                .expect("mint the spawn grant in the build");
            build.mob_tool_authority_context = Some(grant);
            build.override_mob = ToolCategoryOverride::Enable;
        }));

    let handle = mob.resume().await;
    assert_eq!(
        handle.status().await.expect("the mob's phase"),
        MobState::Running
    );
    assert_eq!(
        mob.materialized().await,
        vec![member.clone()],
        "the start restored exactly the member, which owes the completion"
    );
    let build = mob
        .service
        .recorded_create_requests()
        .await
        .into_iter()
        .find(|record| record.session_id == member)
        .expect("the member's restore build");
    assert_eq!(
        build
            .mob_incarnation
            .map(meerkat_core::service::LiveMobIncarnation::incarnation),
        Some(handle.incarnation()),
        "the restore build carries the witness of the runtime performing it"
    );
    let grant = build
        .mob_tool_authority_context
        .as_ref()
        .expect("the hook's spawn grant in the restore build");
    let mob_id = handle.mob_id();
    assert!(
        grant.spawn_profile_scope_contains(mob_id.as_str(), handle.incarnation(), GRANTED_PROFILE),
        "the member keeps its spawn grant in that build"
    );
    assert!(!grant.spawn_profile_scope_contains(mob_id.as_str(), handle.incarnation(), "lead"));
    assert!(
        handle.member_spawn_profile_scope_contains(&member, GRANTED_PROFILE),
        "the runtime received the restored member's authority"
    );
    assert!(!handle.member_spawn_profile_scope_contains(&member, "lead"));

    tokio::time::timeout(ROW_STEP, async {
        while !mob.service.has_started_turn_for(&member).await {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("the restored member's runtime delivers the completion as a turn");
}

/// A host that asks to retry the first `refusals` build preparations and
/// prepares every later one; it counts every preparation asked.
struct RetriesFirstPreparations {
    refusals: usize,
    asked: AtomicUsize,
}

#[async_trait]
impl crate::runtime::MemberBuildPreparer for RetriesFirstPreparations {
    async fn prepare_member_build(
        &self,
        _request: crate::runtime::MemberBuildPreparation,
    ) -> Result<crate::runtime::MemberBuildPrepared, crate::runtime::MemberBuildPreparationRefusal>
    {
        if self.asked.fetch_add(1, Ordering::SeqCst) < self.refusals {
            return Err(crate::runtime::MemberBuildPreparationRefusal::Retryable {
                reason: "the host is not ready for this member yet".to_string(),
            });
        }
        Ok(crate::runtime::MemberBuildPrepared::Prepared)
    }
}

/// The wake retry backoff of the build preparation retry row: long enough
/// that the row's steps run while the retry stands, short enough to wait
/// for its timer.
const PREPARATION_RETRY: Duration = Duration::from_secs(5);

/// A member that still owes work across a restart (a background operation
/// completion its runtime owes), whose host asks to retry its build
/// preparation at the start's wake: MobMachine records the retry
/// (`HostPreparationCause::BuildPreparationRetry`) and the member stays
/// Dormant, its wakes refused for it without asking the host again (a
/// host's wake meanwhile is refused `BuildPreparationRetry`). The retry
/// timer ends the cause at the wake retry backoff and, the work still owed,
/// wakes the member: the host is asked once more, prepares the build, and
/// the woken runtime delivers the completion as a turn. The other member
/// stays Dormant and runs nothing.
///
/// RED before MobMachine recorded the retry: no cause was recorded, so a
/// host's wake meanwhile asked the host again and woke the member.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owed_work_whose_host_asks_to_retry_its_build_runs_after_the_retry() {
    let mob = CrashedStoreMob::new("boot-rows-build-preparation-retry", 2).await;
    let (owed_member, owed) = mob.sessions[1].clone();
    mob.store
        .persist_ops_lifecycle(
            &LogicalRuntimeId::for_session(&owed),
            &undelivered_completion(&owed),
        )
        .await
        .expect("the completion is durable");
    let preparer = Arc::new(RetriesFirstPreparations {
        refusals: 1,
        asked: AtomicUsize::new(0),
    });

    let handle = mob
        .resume_with_preparer(preparer.clone(), PREPARATION_RETRY)
        .await;
    assert_eq!(
        handle.status().await.expect("the mob's phase"),
        MobState::Running
    );
    assert_eq!(
        preparer.asked.load(Ordering::SeqCst),
        1,
        "the start's wake asked the host to prepare the member's build"
    );
    assert_eq!(
        phase_of(&handle, &owed_member),
        Some(dsl::MemberMaterializationPhase::Dormant)
    );
    assert_eq!(
        preparation_of(&handle, &owed_member),
        Some(dsl::HostPreparationCause::BuildPreparationRetry),
        "MobMachine records the build preparation the host asked to retry"
    );

    let refused = tokio::time::timeout(ROW_STEP, handle.ensure_member_live(&owed_member))
        .await
        .expect("the wake answers");
    assert!(
        refused_for_build_preparation_retry(&refused),
        "a host's wake while the retry stands is refused for it: {refused:?}"
    );
    assert_eq!(
        preparer.asked.load(Ordering::SeqCst),
        1,
        "the host is not asked again while the retry stands"
    );

    tokio::time::timeout(ROW_STEP, async {
        while !mob.service.has_started_turn_for(&owed).await {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("the retry wakes the member and its runtime delivers the completion");
    assert_eq!(
        preparer.asked.load(Ordering::SeqCst),
        2,
        "the retry asked the host once more"
    );
    assert_eq!(
        phase_of(&handle, &owed_member),
        Some(dsl::MemberMaterializationPhase::Live)
    );
    assert_eq!(preparation_of(&handle, &owed_member), None);
    let (other, other_session) = mob.sessions[0].clone();
    assert_eq!(
        phase_of(&handle, &other),
        Some(dsl::MemberMaterializationPhase::Dormant)
    );
    assert!(
        !mob.service.has_started_turn_for(&other_session).await,
        "{other} ran no turn"
    );
}

/// A member that owes boot work and whose host asked to retry its build
/// preparation is left to that preparation's own retry timer: its refused
/// owed wake arms no owed-boot retry, whose wakes MobMachine would only
/// refuse for the cause (`HostPreparationCause::BuildPreparationRetry`).
///
/// Pin: an owed-boot retry armed beside the cause (the early return in
/// `schedule_owed_member_wake_retry` removed) is armed here.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owed_work_whose_host_asks_to_retry_its_build_arms_no_owed_retry() {
    let mob = CrashedStoreMob::new("boot-rows-build-preparation-no-owed-retry", 2).await;
    let (owed_member, owed) = mob.sessions[1].clone();
    mob.store
        .persist_ops_lifecycle(
            &LogicalRuntimeId::for_session(&owed),
            &undelivered_completion(&owed),
        )
        .await
        .expect("the completion is durable");
    let preparer = Arc::new(RetriesFirstPreparations {
        refusals: 1,
        asked: AtomicUsize::new(0),
    });

    let handle = mob
        .resume_with_preparer(preparer.clone(), PREPARATION_RETRY)
        .await;
    assert_eq!(
        preparation_of(&handle, &owed_member),
        Some(dsl::HostPreparationCause::BuildPreparationRetry),
        "MobMachine records the build preparation the host asked to retry"
    );

    let armed = handle
        .enqueue_actor_command_for_test(|reply_tx| MobCommand::OwedMemberWakeRetryArmedForTest {
            agent_identity: owed_member.clone(),
            reply_tx,
        })
        .await
        .expect("the probe is queued")
        .await
        .expect("the actor answers")
        .expect("the probe reads");
    assert!(
        !armed,
        "no owed-boot retry is armed while the cause stands: its own timer wakes the member"
    );
}
