//! What a poisoned host store scope does to a mob, and what it leaves
//! serving: a lazy boot plans only the poisoned member as holding work, the
//! delivery owner drains every healthy runtime, a status read answers a
//! refused member from its recorded refusal, and a wait on it ends with the
//! refusal. The runtime store is the host store adapter over the reference
//! in-memory host store, whose commits a row can refuse.

use std::sync::atomic::{AtomicU32, Ordering as AtomicOrdering};

use super::*;
use crate::machines::mob_machine as mob_dsl;
use crate::runtime::boot_plan::{MobBootSetBasis, scan_boot_obligations};
use meerkat_core::host_store::{
    AcquireMode, CommitBatch, FenceGrant, FenceTiming, FenceToken, HeadPage, HostStore,
    HostStoreError, Row, RowIndex, ScopeKind, ScopeName, ScopeRef, VersionedHead,
};
use meerkat_runtime::store::RuntimeStoreError;
use meerkat_runtime::store::spi::{SpiError, SpiRealm, SpiRealmConfig, SpiRuntimeStore};
use meerkat_runtime::{LogicalRuntimeId, RuntimeDeliveryInbox};
use meerkat_store_conformance::InMemoryHostStore;

/// Hang guard for awaited typed events; never a pacing mechanism.
const EVENT_GUARD: Duration = Duration::from_secs(10);

/// The reference in-memory host store, whose next commits a row refuses:
/// a refused batch poisons the scopes it would have changed.
#[derive(Default)]
struct PoisonableHost {
    inner: InMemoryHostStore,
    refused_commits: AtomicU32,
}

impl PoisonableHost {
    fn refuse_next_commit(&self) {
        self.refused_commits.fetch_add(1, AtomicOrdering::SeqCst);
    }
}

#[async_trait]
impl HostStore for PoisonableHost {
    async fn acquire_fence(&self, mode: AcquireMode) -> Result<FenceGrant, HostStoreError> {
        self.inner.acquire_fence(mode).await
    }

    async fn renew_fence(&self, token: FenceToken) -> Result<(), HostStoreError> {
        self.inner.renew_fence(token).await
    }

    fn fence_timing(&self) -> FenceTiming {
        self.inner.fence_timing()
    }

    async fn load_head(&self, scope: &ScopeRef) -> Result<Option<VersionedHead>, HostStoreError> {
        self.inner.load_head(scope).await
    }

    async fn list_heads(
        &self,
        kind: ScopeKind,
        after: Option<&ScopeName>,
        limit: usize,
    ) -> Result<HeadPage, HostStoreError> {
        self.inner.list_heads(kind, after, limit).await
    }

    async fn read_rows(
        &self,
        scope: &ScopeRef,
        from: RowIndex,
        limit: usize,
    ) -> Result<Vec<Row>, HostStoreError> {
        self.inner.read_rows(scope, from, limit).await
    }

    async fn commit(&self, batch: &CommitBatch) -> Result<(), HostStoreError> {
        let refused = self
            .refused_commits
            .fetch_update(AtomicOrdering::SeqCst, AtomicOrdering::SeqCst, |left| {
                left.checked_sub(1)
            })
            .is_ok();
        if refused {
            return Err(HostStoreError::Invalid(
                "the row refuses this batch".to_string(),
            ));
        }
        self.inner.commit(batch).await
    }
}

/// A host store realm and its runtime store, over a host whose commits a row
/// can refuse.
async fn open_realm() -> (Arc<PoisonableHost>, Arc<SpiRuntimeStore>) {
    let host = Arc::new(PoisonableHost::default());
    let realm = SpiRealm::open(Arc::clone(&host) as _, SpiRealmConfig::default())
        .await
        .expect("the realm opens");
    let store = Arc::new(
        SpiRuntimeStore::open(realm)
            .await
            .expect("the runtime store opens"),
    );
    (host, store)
}

/// Poison `runtime`'s scope: the host refuses the batch of a delivery
/// authority write, after memory changed.
async fn poison_runtime(
    host: &PoisonableHost,
    store: &dyn meerkat_runtime::RuntimeStore,
    runtime: &LogicalRuntimeId,
) {
    host.refuse_next_commit();
    let refused = store
        .compare_and_swap_runtime_delivery_authority(
            runtime,
            None,
            meerkat_runtime::store::RuntimeDeliveryAuthorityRecord::from_parts(
                1,
                br#"{"state":1}"#.to_vec(),
            ),
            None,
        )
        .await;
    assert!(refused.is_err(), "the host refuses the batch: {refused:?}");
}

/// The refusal a host store answers for a read of a session whose scope is
/// poisoned, as the session layer hands it on: the runtime store's error,
/// boxed as the store failure's source.
fn poisoned_session_read(session_id: &SessionId) -> SessionError {
    SessionError::Store(Box::new(poisoned_scope_refusal(session_id)))
}

fn poisoned_scope_refusal(session_id: &SessionId) -> RuntimeStoreError {
    SpiError::ScopePoisoned {
        scope: format!("rt:{}", LogicalRuntimeId::for_session(session_id).0),
        cause: "the host refused a batch of this scope".to_string(),
    }
    .into_read()
}

/// A lazy boot over a realm whose runtime scope of one seated member is
/// poisoned stays observed: it plans the poisoned member as holding work (its
/// runtime's facts cannot be trusted) and leaves the healthy member dormant.
/// Before, the obligation scan failed, and every seated member was planned
/// as holding work: an eager wake of the whole mob.
#[tokio::test]
async fn a_member_whose_runtime_scope_is_poisoned_is_planned_as_holding_work_and_the_others_stay_dormant()
 {
    let (host, store) = open_realm().await;
    let poisoned_session = meerkat_core::types::SessionId::new();
    let healthy_session = meerkat_core::types::SessionId::new();
    poison_runtime(
        &host,
        store.as_ref(),
        &LogicalRuntimeId::for_session(&poisoned_session),
    )
    .await;
    let adapter = Arc::new(
        meerkat_runtime::MeerkatMachine::persistent_without_blobs(
            store as Arc<dyn meerkat_runtime::RuntimeStore>,
        )
        .expect("construct runtime authority"),
    );
    let mut state = mob_dsl::MobMachineState::default();
    for (name, session) in [
        ("poisoned", &poisoned_session),
        ("healthy", &healthy_session),
    ] {
        let identity = mob_dsl::AgentIdentity(name.to_string());
        state
            .member_session_bindings
            .insert(identity.clone(), mob_dsl::SessionId(session.to_string()));
        state.identity_to_runtime.insert(
            identity.clone(),
            mob_dsl::AgentRuntimeId(format!("{name}-runtime")),
        );
        state
            .identity_runtime_generations
            .insert(identity, mob_dsl::Generation(1));
    }

    let obligations = scan_boot_obligations(
        Some(&adapter),
        &state,
        &MobId::from("poisoned-scope-boot"),
        meerkat_core::service::MobIncarnationId::mint(),
    )
    .await
    .expect("the scan never fails the plan");
    assert_eq!(obligations.basis, MobBootSetBasis::Observed);
    assert_eq!(
        obligations.by_member,
        BTreeMap::from([(
            mob_dsl::AgentIdentity("poisoned".to_string()),
            mob_dsl::MemberRecoveryObligationKind::RuntimeWork,
        )]),
        "only the poisoned member is planned as holding work"
    );
}

/// Records the job id of every applied delivery.
#[derive(Default)]
struct RecordingSink {
    applied: tokio::sync::Mutex<Vec<meerkat::JobId>>,
}

#[async_trait]
impl meerkat::JobDeliverySink for RecordingSink {
    async fn apply(
        &self,
        application: meerkat::JobDeliveryApplication,
    ) -> Result<(), meerkat::JobDeliveryApplyError> {
        let job_id = match &application {
            meerkat::JobDeliveryApplication::Record { job_id, .. }
            | meerkat::JobDeliveryApplication::Notification { job_id, .. }
            | meerkat::JobDeliveryApplication::Event { job_id, .. } => job_id.clone(),
        };
        self.applied.lock().await.push(job_id);
        Ok(())
    }
}

/// Serves every session here: a single-process host.
struct StaticHost {
    sink: Arc<RecordingSink>,
}

#[async_trait]
impl meerkat::RuntimeDeliveryHost for StaticHost {
    async fn delivery_route(&self, _session_id: &SessionId) -> Option<meerkat::DeliveryRoute> {
        Some(meerkat::DeliveryRoute::ServedHere(self.sink.clone()))
    }

    async fn claim_cold_delivery(
        &self,
        session_id: &SessionId,
    ) -> Option<Result<meerkat_runtime::HostingClaim, meerkat_runtime::HostingRefused>> {
        Some(meerkat_runtime::grant_session_hosting(
            &meerkat_runtime::HostingCapability::ProcessLocal,
            &meerkat_runtime::HostingOwner::mint(),
            session_id,
        ))
    }
}

/// A job of `session_id` that ran to completion: its terminal is in the job
/// outbox.
async fn completed_job(
    jobs: &meerkat::DetachedJobService,
    session_id: SessionId,
    key: &str,
) -> meerkat::JobId {
    let receipt = jobs
        .submit(meerkat::JobSpec::new(
            "default",
            session_id,
            meerkat::ExecutionIntentId::new(),
            meerkat::InteractionLineageId::new(),
            meerkat::ToolIdentity::new("shell", "1").expect("tool"),
            meerkat::RunnerIdentity::new("durable-shell", "1").expect("runner"),
            meerkat::RestartClass::Adoptable,
            meerkat::CanonicalArgumentsHash::new(format!("hash-{key}")).expect("hash"),
            meerkat::JobSubmissionKey::new(key).expect("submission key"),
        ))
        .await
        .expect("submit");
    let claim = jobs
        .claim_attempt(
            &receipt.job_id,
            meerkat::AttemptClaim::new(
                meerkat::WorkerId::new("worker").expect("worker"),
                1,
                100,
                meerkat::RunnerHandleRef::new("runner-handle").expect("handle"),
            ),
        )
        .await
        .expect("claim");
    jobs.complete_attempt(
        &receipt.job_id,
        (&claim).into(),
        2,
        Some(meerkat::JobResultRef::new("result").expect("result")),
    )
    .await
    .expect("complete");
    receipt.job_id
}

/// The delivery owner drains a healthy runtime's backlog while another
/// runtime's scope is poisoned. The healthy session's delivery was committed
/// by an earlier process, so only this owner's reconcile read finds it.
/// Before, that read refused while any scope was poisoned, every reconcile
/// failed, and the delivery stayed pending until a restart.
#[tokio::test]
async fn a_healthy_runtime_backlog_drains_while_another_runtime_scope_is_poisoned() {
    let (host, store) = open_realm().await;
    let store: Arc<dyn meerkat_runtime::RuntimeStore> = store;
    let job_store = Arc::new(meerkat::MemoryDetachedJobStore::new());
    let jobs = meerkat::DetachedJobService::new(job_store.clone());
    let session_id = SessionId::new();
    let job = completed_job(&jobs, session_id.clone(), "before-the-poison").await;
    // An earlier process projected the job's terminal into the inbox.
    let projected = meerkat::JobOutboxProjector::new(
        job_store.clone(),
        RuntimeDeliveryInbox::new(Arc::clone(&store)),
    )
    .project_pending(16)
    .await
    .expect("the earlier process projects the outbox");
    assert_eq!(projected.projected.len(), 1, "{projected:?}");

    let doomed = LogicalRuntimeId::new("poisoned-elsewhere");
    poison_runtime(&host, store.as_ref(), &doomed).await;

    let sink = Arc::new(RecordingSink::default());
    let owner = meerkat::RuntimeDeliveryOwner::new(job_store, RuntimeDeliveryInbox::new(store))
        .arm(Arc::new(StaticHost { sink: sink.clone() }))
        .expect("arm the owner");
    let mut passes = owner.subscribe_passes();
    tokio::time::timeout(EVENT_GUARD, async {
        loop {
            if sink.applied.lock().await.contains(&job) {
                return;
            }
            passes.changed().await.expect("owner pass channel open");
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "the healthy session's delivery was never applied; last pass: {:?}",
            passes.borrow()
        )
    });
}

/// A session read refused by a poisoned scope has a class of its own: the
/// store refuses until the realm is reopened. It is not a store that could
/// not serve now, so nothing retries it as one.
#[test]
fn a_poisoned_scope_refusal_has_its_own_class_and_is_not_retried() {
    use crate::runtime::actor::{SessionStoreFailureClass, session_store_failure_class};

    let session_id = SessionId::new();
    for refusal in [
        poisoned_scope_refusal(&session_id),
        SpiError::RealmPoisoned("fenced out".to_string()).into_read(),
    ] {
        assert_ne!(
            session_store_failure_class(&refusal),
            SessionStoreFailureClass::Unavailable,
            "{refusal:?}"
        );
        assert!(
            !crate::runtime::builder::session_error_is_retryable(&SessionError::Store(Box::new(
                refusal.clone()
            ))),
            "a refusal until the realm reopens is not retried: {refusal:?}"
        );
    }
}

/// A member whose session read is refused until the realm reopens is read
/// once: the refusal is recorded, and every later status read answers from
/// it, with no read of the session. Before, every status read read again
/// and logged again.
#[tokio::test]
async fn a_refused_member_status_is_answered_from_the_recorded_refusal() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    let member = AgentIdentity::from("refused-status");
    let session_id = handle
        .spawn(ProfileName::from("worker"), member.clone(), None)
        .await
        .expect("spawn")
        .bridge_session_id()
        .expect("session-backed member")
        .clone();
    let refused_session = session_id.clone();
    service.refuse_status_view_reads(&session_id, move || poisoned_session_read(&refused_session));
    let before = service
        .durable_reads_of(&session_id)
        .member_status_durable_head;
    for _ in 0..50 {
        let snapshot = handle.member_status(&member).await.expect("status");
        assert!(
            snapshot.preview_unavailable.is_some(),
            "the preview is unavailable, not empty"
        );
    }
    assert_eq!(
        service
            .durable_reads_of(&session_id)
            .member_status_durable_head
            - before,
        1,
        "one read of the refused session, then its recorded refusal answers"
    );
}

/// A wait on a member whose session read is refused until the realm reopens
/// ends with that refusal, both for one member and for several. Before, it
/// polled the member's status for as long as the caller waited.
#[tokio::test]
async fn a_wait_on_a_store_refused_member_ends_with_the_refusal() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    let member = AgentIdentity::from("refused-wait");
    let session_id = handle
        .spawn(ProfileName::from("worker"), member.clone(), None)
        .await
        .expect("spawn")
        .bridge_session_id()
        .expect("session-backed member")
        .clone();
    let refused_session = session_id.clone();
    service.refuse_status_view_reads(&session_id, move || poisoned_session_read(&refused_session));

    let one = tokio::time::timeout(Duration::from_secs(3), handle.wait_one(&member)).await;
    assert!(
        matches!(one, Ok(Err(_))),
        "wait_one ends with the refusal: {one:?}"
    );
    let all = tokio::time::timeout(
        Duration::from_secs(3),
        handle.wait_all(std::slice::from_ref(&member)),
    )
    .await;
    assert!(
        matches!(all, Ok(Err(_))),
        "wait_all ends with the refusal: {all:?}"
    );
}

/// A kickoff wait on an autonomous member whose kickoff is still pending and
/// whose session read is refused until the realm reopens ends with that
/// refusal, before its own timeout. Before, it polled until the timeout.
#[tokio::test]
async fn a_kickoff_wait_on_a_store_refused_member_ends_with_the_refusal() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    service.set_start_turn_delay_ms(60_000);
    let lead = AgentIdentity::from("refused-kickoff");
    let session_id = handle
        .spawn(ProfileName::from("lead"), lead.clone(), None)
        .await
        .expect("spawn")
        .bridge_session_id()
        .expect("session-backed member")
        .clone();
    let refused_session = session_id.clone();
    service.refuse_status_view_reads(&session_id, move || poisoned_session_read(&refused_session));

    let ended = tokio::time::timeout(
        EVENT_GUARD,
        handle.wait_for_members_kickoff_complete(
            std::slice::from_ref(&lead),
            Some(Duration::from_secs(3)),
        ),
    )
    .await
    .expect("the kickoff wait ends within its own timeout");
    let error = ended.expect_err("the kickoff wait ends with an error");
    assert!(
        !matches!(error, MobError::KickoffWaitTimedOut { .. }),
        "the kickoff wait ends with the refusal, not at its timeout: {error:?}"
    );
}
