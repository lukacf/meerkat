//! Boot obligation rows: the runtime obligation scan a mob start plans its
//! members with. A background operation completion that a member's runtime
//! still owes after a crash wakes that member at the start, under the
//! default policy and with no touch, and the woken runtime delivers it. A
//! scan that fails leaves the start running, every member planned to wake.
//!
//! Unrun until the candidate's validation.

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

const ROW_STEP: Duration = Duration::from_secs(60);

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
