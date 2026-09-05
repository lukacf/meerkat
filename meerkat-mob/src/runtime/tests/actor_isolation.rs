//! #1102 (OB3 fleet-wide delivery stall): actor-loop isolation.
//!
//! One member's blocking work must not delay another member's dispatch or
//! the actor liveness probe. These tests drive runtime-backed members over a
//! persistent `MeerkatMachine` whose `RuntimeStore` is wrapped with two
//! switches keyed by session: `fail_commit` makes the committed-boundary
//! commit return `WriteFailed` (exactly the path that degrades a runtime to
//! `ReloadRequired`, as OB3's continuity save did) and `park_admissions`
//! blocks the durable admission write. The mock session service adds a
//! parkable live-session lookup (the pre-#1102 inline step that wedged the
//! loop) and a slow `comms_runtime` for resume readiness.
//!
//! Every bound below is a tight wall-clock envelope chosen to stay green on a
//! loaded CI box while failing decisively on the pre-#1102 loop, whose stall
//! for one member queued every other member's delivery behind it.

use super::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::Notify;

use meerkat_core::lifecycle::run_receipt::RunBoundaryReceipt;
use meerkat_core::lifecycle::{InputId, RunId};
use meerkat_runtime::store::RuntimeStore;
use meerkat_runtime::{InMemoryRuntimeStore, LogicalRuntimeId, MeerkatMachine};

use crate::ids::WorkOrigin;
use crate::runtime::handle::{MEMBER_ADMISSION_LANE_CAPACITY, MemberReloadDisposition};
use crate::runtime::state::MobCommand;

/// `InMemoryRuntimeStore` decorator with per-session fault switches.
struct FaultInjectingRuntimeStore {
    inner: Arc<InMemoryRuntimeStore>,
    fail_commit: std::sync::Mutex<HashSet<LogicalRuntimeId>>,
    parked_admissions: std::sync::Mutex<HashSet<LogicalRuntimeId>>,
    release_admissions: Notify,
    parked_admission_arrivals: std::sync::Mutex<HashMap<LogicalRuntimeId, usize>>,
    /// Sessions whose boundary commit parks (slow uplink) and then succeeds.
    parked_commits: std::sync::Mutex<HashSet<LogicalRuntimeId>>,
    release_commits: Notify,
    parked_commit_arrivals: std::sync::Mutex<HashMap<LogicalRuntimeId, usize>>,
    failed_commits: AtomicU64,
}

impl FaultInjectingRuntimeStore {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Arc::new(InMemoryRuntimeStore::new()),
            fail_commit: std::sync::Mutex::new(HashSet::new()),
            parked_admissions: std::sync::Mutex::new(HashSet::new()),
            release_admissions: Notify::new(),
            parked_admission_arrivals: std::sync::Mutex::new(HashMap::new()),
            parked_commits: std::sync::Mutex::new(HashSet::new()),
            release_commits: Notify::new(),
            parked_commit_arrivals: std::sync::Mutex::new(HashMap::new()),
            failed_commits: AtomicU64::new(0),
        })
    }

    /// Park every committed-boundary commit for `session_id` until
    /// [`Self::release_commits`], then let it succeed. Models OB3's real
    /// trigger: a 30 s HTTP wait on a large session save that had in fact
    /// committed server-side.
    fn park_commits(&self, session_id: &SessionId) {
        self.parked_commits
            .lock()
            .expect("parked_commits mutex")
            .insert(LogicalRuntimeId::for_session(session_id));
    }

    fn release_commits(&self) {
        self.parked_commits
            .lock()
            .expect("parked_commits mutex")
            .clear();
        self.release_commits.notify_waiters();
    }

    /// Boundary commits that entered the park for `session_id`.
    fn parked_commit_arrivals(&self, session_id: &SessionId) -> usize {
        self.parked_commit_arrivals
            .lock()
            .expect("parked_commit_arrivals mutex")
            .get(&LogicalRuntimeId::for_session(session_id))
            .copied()
            .unwrap_or(0)
    }

    async fn park_commit_if_flagged(&self, runtime_id: &LogicalRuntimeId) {
        let mut arrived = false;
        loop {
            let released = self.release_commits.notified();
            let parked = self
                .parked_commits
                .lock()
                .expect("parked_commits mutex")
                .contains(runtime_id);
            if !parked {
                break;
            }
            if !arrived {
                arrived = true;
                *self
                    .parked_commit_arrivals
                    .lock()
                    .expect("parked_commit_arrivals mutex")
                    .entry(runtime_id.clone())
                    .or_default() += 1;
            }
            released.await;
        }
    }

    /// Make every committed-boundary commit for `session_id` fail with
    /// `WriteFailed`, which the persistent driver converts into
    /// `mark_durability_reload_required` (OB3's path).
    fn fail_commit(&self, session_id: &SessionId, enabled: bool) {
        let runtime_id = LogicalRuntimeId::for_session(session_id);
        let mut flagged = self.fail_commit.lock().expect("fail_commit mutex");
        if enabled {
            flagged.insert(runtime_id);
        } else {
            flagged.remove(&runtime_id);
        }
    }

    fn failed_commits(&self) -> u64 {
        self.failed_commits.load(Ordering::Relaxed)
    }

    /// Block the durable admission write for `session_id` until
    /// [`Self::release_admissions`] (hang mode).
    fn park_admissions(&self, session_id: &SessionId) {
        self.parked_admissions
            .lock()
            .expect("parked_admissions mutex")
            .insert(LogicalRuntimeId::for_session(session_id));
    }

    fn release_admissions(&self) {
        self.parked_admissions
            .lock()
            .expect("parked_admissions mutex")
            .clear();
        self.release_admissions.notify_waiters();
    }

    /// Admission writes that entered the park for `session_id`.
    fn parked_admission_arrivals(&self, session_id: &SessionId) -> usize {
        self.parked_admission_arrivals
            .lock()
            .expect("parked_admission_arrivals mutex")
            .get(&LogicalRuntimeId::for_session(session_id))
            .copied()
            .unwrap_or(0)
    }

    fn fail_commit_if_flagged(
        &self,
        runtime_id: &LogicalRuntimeId,
        operation: &str,
    ) -> Result<(), meerkat_runtime::store::RuntimeStoreError> {
        if self
            .fail_commit
            .lock()
            .expect("fail_commit mutex")
            .contains(runtime_id)
        {
            self.failed_commits.fetch_add(1, Ordering::Relaxed);
            return Err(meerkat_runtime::store::RuntimeStoreError::WriteFailed(
                format!("injected {operation} failure for {runtime_id}"),
            ));
        }
        Ok(())
    }

    async fn park_admission_if_flagged(&self, runtime_id: &LogicalRuntimeId) {
        let mut arrived = false;
        loop {
            let released = self.release_admissions.notified();
            let parked = self
                .parked_admissions
                .lock()
                .expect("parked_admissions mutex")
                .contains(runtime_id);
            if !parked {
                break;
            }
            if !arrived {
                arrived = true;
                *self
                    .parked_admission_arrivals
                    .lock()
                    .expect("parked_admission_arrivals mutex")
                    .entry(runtime_id.clone())
                    .or_default() += 1;
            }
            released.await;
        }
    }
}

#[async_trait::async_trait]
impl RuntimeStore for FaultInjectingRuntimeStore {
    fn session_authority_ops(&self) -> &dyn meerkat_runtime::store::RuntimeSessionAuthorityOps {
        self.inner.session_authority_ops()
    }

    fn session_persistence_profile(
        &self,
    ) -> meerkat_runtime::store::RuntimeSessionPersistenceProfile {
        RuntimeStore::session_persistence_profile(self.inner.as_ref())
    }

    async fn commit_prepared_session_boundary(
        &self,
        runtime_id: &LogicalRuntimeId,
        request: meerkat_runtime::store::PreparedRuntimeSessionCommit,
    ) -> Result<
        meerkat_runtime::store::PreparedRuntimeSessionCommitResult,
        meerkat_runtime::store::RuntimeStoreError,
    > {
        self.park_commit_if_flagged(runtime_id).await;
        self.fail_commit_if_flagged(runtime_id, "commit_prepared_session_boundary")?;
        self.inner
            .commit_prepared_session_boundary(runtime_id, request)
            .await
    }

    fn input_state_batch_cas_implementation_profile(
        &self,
    ) -> meerkat_runtime::store::InputStateBatchCasImplementationProfile {
        self.inner.input_state_batch_cas_implementation_profile()
    }

    fn supports_compaction_projection_outbox(&self) -> bool {
        self.inner.supports_compaction_projection_outbox()
    }

    async fn observe_machine_lifecycle(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<
        meerkat_runtime::store::MachineLifecycleObservation,
        meerkat_runtime::store::RuntimeStoreError,
    > {
        self.inner.observe_machine_lifecycle(runtime_id).await
    }

    async fn compare_and_swap_machine_lifecycle(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected: meerkat_runtime::store::MachineLifecycleExpectedVersion,
        replacement: meerkat_runtime::store::MachineLifecycleCommit,
    ) -> Result<
        meerkat_runtime::store::MachineLifecycleCasOutcome,
        meerkat_runtime::store::RuntimeStoreError,
    > {
        self.inner
            .compare_and_swap_machine_lifecycle(runtime_id, expected, replacement)
            .await
    }

    async fn commit_session_snapshot(
        &self,
        runtime_id: &LogicalRuntimeId,
        session_delta: meerkat_runtime::store::SerializedSessionSnapshot,
    ) -> Result<(), meerkat_runtime::store::RuntimeStoreError> {
        self.inner
            .commit_session_snapshot(runtime_id, session_delta)
            .await
    }

    async fn commit_prepared_whole_blob_rewrite_boundary(
        &self,
        runtime_id: &LogicalRuntimeId,
        boundary: meerkat_runtime::store::PreparedWholeBlobRewriteStoreParts,
    ) -> Result<
        meerkat_runtime::store::WholeBlobStoreAuthority,
        meerkat_runtime::store::RuntimeStoreError,
    > {
        self.inner
            .commit_prepared_whole_blob_rewrite_boundary(runtime_id, boundary)
            .await
    }

    async fn atomic_apply(
        &self,
        runtime_id: &LogicalRuntimeId,
        session_delta: Option<meerkat_runtime::store::SerializedSessionSnapshot>,
        receipt: RunBoundaryReceipt,
        input_updates: Vec<meerkat_runtime::input_state::InputStatePersistenceRecord>,
        session_store_key: Option<SessionId>,
    ) -> Result<(), meerkat_runtime::store::RuntimeStoreError> {
        self.fail_commit_if_flagged(runtime_id, "atomic_apply")?;
        self.inner
            .atomic_apply(
                runtime_id,
                session_delta,
                receipt,
                input_updates,
                session_store_key,
            )
            .await
    }

    async fn load_input_states(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Vec<meerkat_runtime::store::InputStateRow>, meerkat_runtime::store::RuntimeStoreError>
    {
        self.inner.load_input_states(runtime_id).await
    }

    async fn load_input_states_with_versions(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<
        meerkat_runtime::store::PreparedRecoveryInputSnapshot,
        meerkat_runtime::store::RuntimeStoreError,
    > {
        self.inner.load_input_states_with_versions(runtime_id).await
    }

    async fn compare_and_swap_recovery_input_states_atomically(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected_revision: meerkat_runtime::store::RecoveryInputSetRevision,
        mutations: &[meerkat_runtime::store::RecoveryInputStateMutation],
    ) -> Result<
        meerkat_runtime::store::InputStateBatchCasOutcome,
        meerkat_runtime::store::RuntimeStoreError,
    > {
        self.inner
            .compare_and_swap_recovery_input_states_atomically(
                runtime_id,
                expected_revision,
                mutations,
            )
            .await
    }

    async fn load_boundary_receipt(
        &self,
        runtime_id: &LogicalRuntimeId,
        run_id: &RunId,
        sequence: u64,
    ) -> Result<Option<RunBoundaryReceipt>, meerkat_runtime::store::RuntimeStoreError> {
        self.inner
            .load_boundary_receipt(runtime_id, run_id, sequence)
            .await
    }

    async fn load_session_snapshot(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<std::sync::Arc<Vec<u8>>>, meerkat_runtime::store::RuntimeStoreError> {
        self.inner.load_session_snapshot(runtime_id).await
    }

    async fn load_pending_compaction_projections(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<
        Vec<meerkat_core::CompactionProjectionIntent>,
        meerkat_runtime::store::RuntimeStoreError,
    > {
        self.inner
            .load_pending_compaction_projections(runtime_id)
            .await
    }

    async fn mark_compaction_projection_finalized(
        &self,
        runtime_id: &LogicalRuntimeId,
        projection: &meerkat_core::CompactionProjectionId,
    ) -> Result<(), meerkat_runtime::store::RuntimeStoreError> {
        self.inner
            .mark_compaction_projection_finalized(runtime_id, projection)
            .await
    }

    async fn clear_session_snapshot(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<(), meerkat_runtime::store::RuntimeStoreError> {
        self.inner.clear_session_snapshot(runtime_id).await
    }

    async fn replace_session_snapshot_if_current(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected_current: &[u8],
        replacement: Vec<u8>,
    ) -> Result<bool, meerkat_runtime::store::RuntimeStoreError> {
        self.inner
            .replace_session_snapshot_if_current(runtime_id, expected_current, replacement)
            .await
    }

    async fn clear_session_snapshot_if_current(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected_current: &[u8],
    ) -> Result<bool, meerkat_runtime::store::RuntimeStoreError> {
        self.inner
            .clear_session_snapshot_if_current(runtime_id, expected_current)
            .await
    }

    async fn persist_input_state(
        &self,
        runtime_id: &LogicalRuntimeId,
        state: &meerkat_runtime::input_state::InputStatePersistenceRecord,
    ) -> Result<(), meerkat_runtime::store::RuntimeStoreError> {
        self.inner.persist_input_state(runtime_id, state).await
    }

    async fn persist_input_states_atomically(
        &self,
        runtime_id: &LogicalRuntimeId,
        states: &[meerkat_runtime::input_state::InputStatePersistenceRecord],
    ) -> Result<(), meerkat_runtime::store::RuntimeStoreError> {
        self.park_admission_if_flagged(runtime_id).await;
        self.inner
            .persist_input_states_atomically(runtime_id, states)
            .await
    }

    async fn compare_and_swap_input_states_atomically(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected: &[meerkat_runtime::input_state::StoredInputState],
        replacements: &[meerkat_runtime::input_state::InputStatePersistenceRecord],
    ) -> Result<
        meerkat_runtime::store::InputStateBatchCasOutcome,
        meerkat_runtime::store::RuntimeStoreError,
    > {
        self.inner
            .compare_and_swap_input_states_atomically(runtime_id, expected, replacements)
            .await
    }

    async fn compare_and_swap_input_states_atomically_with_fence(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected: &[meerkat_runtime::input_state::StoredInputState],
        replacements: &[meerkat_runtime::input_state::InputStatePersistenceRecord],
        write_fence: Arc<dyn meerkat_runtime::store::RuntimeStoreWriteFence>,
    ) -> Result<
        meerkat_runtime::store::FencedInputStateBatchCasOutcome,
        meerkat_runtime::store::RuntimeStoreError,
    > {
        self.inner
            .compare_and_swap_input_states_atomically_with_fence(
                runtime_id,
                expected,
                replacements,
                write_fence,
            )
            .await
    }

    async fn compare_and_swap_recovery_input_states_atomically_with_fence(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected_revision: meerkat_runtime::store::RecoveryInputSetRevision,
        mutations: &[meerkat_runtime::store::RecoveryInputStateMutation],
        write_fence: Arc<dyn meerkat_runtime::store::RuntimeStoreWriteFence>,
    ) -> Result<
        meerkat_runtime::store::FencedInputStateBatchCasOutcome,
        meerkat_runtime::store::RuntimeStoreError,
    > {
        self.inner
            .compare_and_swap_recovery_input_states_atomically_with_fence(
                runtime_id,
                expected_revision,
                mutations,
                write_fence,
            )
            .await
    }

    async fn load_input_state(
        &self,
        runtime_id: &LogicalRuntimeId,
        input_id: &InputId,
    ) -> Result<
        Option<meerkat_runtime::input_state::StoredInputState>,
        meerkat_runtime::store::RuntimeStoreError,
    > {
        self.inner.load_input_state(runtime_id, input_id).await
    }

    async fn load_input_state_by_idempotency_key(
        &self,
        runtime_id: &LogicalRuntimeId,
        key: &meerkat_runtime::IdempotencyKey,
    ) -> Result<
        Option<meerkat_runtime::store::ExactInputStateObservation>,
        meerkat_runtime::store::RuntimeStoreError,
    > {
        self.inner
            .load_input_state_by_idempotency_key(runtime_id, key)
            .await
    }

    async fn load_input_states_by_ids(
        &self,
        runtime_id: &LogicalRuntimeId,
        input_ids: &[InputId],
    ) -> Result<
        Vec<Option<meerkat_runtime::input_state::StoredInputState>>,
        meerkat_runtime::store::RuntimeStoreError,
    > {
        self.inner
            .load_input_states_by_ids(runtime_id, input_ids)
            .await
    }

    async fn load_pending_terminal_owner_ids_page(
        &self,
        runtime_id: &LogicalRuntimeId,
        after: Option<&InputId>,
        limit: usize,
    ) -> Result<Vec<InputId>, meerkat_runtime::store::RuntimeStoreError> {
        self.inner
            .load_pending_terminal_owner_ids_page(runtime_id, after, limit)
            .await
    }

    async fn load_machine_lifecycle_record(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<Vec<u8>>, meerkat_runtime::store::RuntimeStoreError> {
        self.inner.load_machine_lifecycle_record(runtime_id).await
    }

    async fn commit_machine_lifecycle(
        &self,
        runtime_id: &LogicalRuntimeId,
        commit: meerkat_runtime::store::MachineLifecycleCommit,
        input_states: &[meerkat_runtime::input_state::InputStatePersistenceRecord],
    ) -> Result<(), meerkat_runtime::store::RuntimeStoreError> {
        self.inner
            .commit_machine_lifecycle(runtime_id, commit, input_states)
            .await
    }

    async fn commit_unregister_finalization(
        &self,
        runtime_id: &LogicalRuntimeId,
        finalization: meerkat_runtime::store::UnregisterFinalizationCommit,
    ) -> Result<(), meerkat_runtime::store::RuntimeStoreError> {
        self.inner
            .commit_unregister_finalization(runtime_id, finalization)
            .await
    }

    async fn persist_ops_lifecycle(
        &self,
        runtime_id: &LogicalRuntimeId,
        snapshot: &meerkat_runtime::ops_lifecycle::PersistedOpsSnapshot,
    ) -> Result<(), meerkat_runtime::store::RuntimeStoreError> {
        self.inner.persist_ops_lifecycle(runtime_id, snapshot).await
    }

    async fn initialize_ops_lifecycle_if_absent(
        &self,
        runtime_id: &LogicalRuntimeId,
        candidate: &meerkat_runtime::ops_lifecycle::PersistedOpsSnapshot,
    ) -> Result<
        meerkat_runtime::ops_lifecycle::PersistedOpsSnapshot,
        meerkat_runtime::store::RuntimeStoreError,
    > {
        self.inner
            .initialize_ops_lifecycle_if_absent(runtime_id, candidate)
            .await
    }

    async fn load_ops_lifecycle(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<
        Option<meerkat_runtime::ops_lifecycle::PersistedOpsSnapshot>,
        meerkat_runtime::store::RuntimeStoreError,
    > {
        self.inner.load_ops_lifecycle(runtime_id).await
    }

    async fn delete_ops_lifecycle(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<(), meerkat_runtime::store::RuntimeStoreError> {
        self.inner.delete_ops_lifecycle(runtime_id).await
    }
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

struct IsolationMob {
    handle: MobHandle,
    service: Arc<MockSessionService>,
    store: Arc<FaultInjectingRuntimeStore>,
    adapter: Arc<MeerkatMachine>,
    members: Vec<(AgentIdentity, SessionId)>,
}

fn turn_driven_definition() -> MobDefinition {
    let mut definition = sample_definition();
    let worker = definition
        .profiles
        .get_mut(&ProfileName::from("worker"))
        .expect("worker profile")
        .as_inline_mut()
        .unwrap();
    worker.runtime_mode = crate::MobRuntimeMode::TurnDriven;
    // Deliveries in these tests are external (the OB3 console path).
    worker.external_addressable = true;
    definition
}

/// Runtime-backed TurnDriven members over a persistent machine whose store
/// is the fault-injecting wrapper. Members are named `w-0..w-{n-1}`.
async fn create_isolation_mob(member_count: usize) -> IsolationMob {
    create_isolation_mob_with_definition(member_count, turn_driven_definition()).await
}

/// `create_isolation_mob` with every worker wired to every other worker
/// (the review-cycle hand-off graph).
async fn create_wired_isolation_mob(member_count: usize) -> IsolationMob {
    let mut definition = turn_driven_definition();
    definition.wiring.role_wiring = vec![RoleWiringRule {
        a: ProfileName::from("worker"),
        b: ProfileName::from("worker"),
    }];
    create_isolation_mob_with_definition(member_count, definition).await
}

async fn create_isolation_mob_with_definition(
    member_count: usize,
    definition: MobDefinition,
) -> IsolationMob {
    let store = FaultInjectingRuntimeStore::new();
    let runtime_store: Arc<dyn RuntimeStore> = Arc::clone(&store) as Arc<dyn RuntimeStore>;
    let blob_store: Arc<dyn meerkat_core::BlobStore> =
        Arc::new(meerkat_store::MemoryBlobStore::new());
    let adapter = Arc::new(MeerkatMachine::persistent(runtime_store, blob_store));
    let service = Arc::new(MockSessionService::new());
    service.set_runtime_adapter(Arc::clone(&adapter));
    let handle = MobBuilder::new(definition, MobStorage::in_memory())
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create isolation mob");
    let mut members = Vec::with_capacity(member_count);
    for index in 0..member_count {
        let identity = AgentIdentity::from(format!("w-{index}"));
        let receipt = handle
            .spawn(ProfileName::from("worker"), identity.clone(), None)
            .await
            .unwrap_or_else(|error| panic!("spawn {identity}: {error}"));
        let session_id = receipt
            .bridge_session_id()
            .expect("runtime-backed member has a bridge session")
            .clone();
        members.push((identity, session_id));
    }
    IsolationMob {
        handle,
        service,
        store,
        adapter,
        members,
    }
}

impl IsolationMob {
    fn member(&self, index: usize) -> &AgentIdentity {
        &self.members[index].0
    }

    fn session(&self, index: usize) -> &SessionId {
        &self.members[index].1
    }

    async fn send(&self, index: usize, text: &str) -> Result<MemberDeliveryReceipt, MobError> {
        self.handle
            .member(self.member(index))
            .await?
            .send(text.to_string(), HandlingMode::Queue)
            .await
    }

    /// Prompts the runtime executor actually ran on `index`'s session, in
    /// execution order.
    async fn executed_prompts(&self, index: usize) -> Vec<String> {
        let session_id = self.session(index).clone();
        self.service
            .start_turn_prompts
            .read()
            .await
            .iter()
            .filter(|(session, _)| session == &session_id)
            .map(|(_, prompt)| prompt.clone())
            .collect()
    }

    async fn wait_for_executed_prompts(&self, index: usize, count: usize) {
        wait_until(
            &format!("member {index} executed {count} prompts"),
            Duration::from_secs(5),
            || async { self.executed_prompts(index).await.len() >= count },
        )
        .await;
    }

    /// Run one turn on `index` with `fail_commit` armed and wait until the
    /// runtime registration is durability-degraded.
    async fn degrade_member(&self, index: usize) {
        let session_id = self.session(index).clone();
        self.store.fail_commit(&session_id, true);
        self.send(index, "degrade me")
            .await
            .expect("delivery before degradation is admitted");
        let adapter = Arc::clone(&self.adapter);
        wait_until(
            &format!("member {index} runtime degraded to ReloadRequired"),
            Duration::from_secs(5),
            || {
                let adapter = Arc::clone(&adapter);
                let session_id = session_id.clone();
                async move {
                    adapter
                        .durability_reload_required(&session_id)
                        .await
                        .is_some()
                }
            },
        )
        .await;
        assert!(self.store.failed_commits() >= 1);
    }

    /// One `QueryPhase` round trip; `Err(())` when it misses `budget`.
    async fn probe(&self, budget: Duration) -> Result<Duration, ()> {
        let started = Instant::now();
        let reply_rx = self
            .handle
            .enqueue_actor_command_for_test(|reply_tx| MobCommand::QueryPhase { reply_tx })
            .await
            .expect("probe enqueue");
        match tokio::time::timeout(budget, reply_rx).await {
            Ok(reply) => {
                reply
                    .expect("probe reply channel")
                    .expect("probe phase read");
                Ok(started.elapsed())
            }
            Err(_) => Err(()),
        }
    }

    /// Spawn a probe loop that pages on any round trip slower than `budget`.
    /// Returns (stop switch, page counter, join handle); the join handle
    /// yields the slowest observed round trip.
    fn spawn_probe_loop(
        &self,
        cadence: Duration,
        budget: Duration,
    ) -> (
        Arc<Notify>,
        Arc<AtomicU64>,
        tokio::task::JoinHandle<Duration>,
    ) {
        let stop = Arc::new(Notify::new());
        let pages = Arc::new(AtomicU64::new(0));
        let handle = self.handle.clone();
        let task = tokio::spawn({
            let stop = Arc::clone(&stop);
            let pages = Arc::clone(&pages);
            async move {
                let mut slowest = Duration::ZERO;
                let mut stopped = Box::pin(stop.notified());
                loop {
                    let started = Instant::now();
                    let reply_rx = match handle
                        .enqueue_actor_command_for_test(|reply_tx| MobCommand::QueryPhase {
                            reply_tx,
                        })
                        .await
                    {
                        Ok(reply_rx) => reply_rx,
                        Err(_) => return slowest,
                    };
                    tokio::select! {
                        reply = tokio::time::timeout(budget, reply_rx) => {
                            slowest = slowest.max(started.elapsed());
                            if reply.is_err() {
                                pages.fetch_add(1, Ordering::Relaxed);
                                tracing::warn!(
                                    elapsed_ms = started.elapsed().as_millis() as u64,
                                    "test probe paged: actor loop stalled"
                                );
                            }
                        }
                        () = &mut stopped => return slowest,
                    }
                    tokio::select! {
                        () = tokio::time::sleep(cadence) => {}
                        () = &mut stopped => return slowest,
                    }
                }
            }
        });
        (stop, pages, task)
    }
}

async fn wait_until<F, Fut>(what: &str, budget: Duration, mut predicate: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = Instant::now() + budget;
    loop {
        if predicate().await {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out after {budget:?} waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn percentile(samples: &mut [Duration], percentile: f64) -> Duration {
    assert!(!samples.is_empty());
    samples.sort();
    let rank = ((samples.len() as f64 - 1.0) * percentile).round() as usize;
    samples[rank.min(samples.len() - 1)]
}

/// TurnCompleted-ack delivery (`MemberHandle::internal_turn`): resolves once
/// the member's turn has run, not merely once it was admitted.
fn internal_turn_task(
    handle: &MobHandle,
    identity: &AgentIdentity,
    text: String,
) -> tokio::task::JoinHandle<Result<MemberDeliveryReceipt, MobError>> {
    let handle = handle.clone();
    let identity = identity.clone();
    tokio::spawn(async move {
        handle
            .member(&identity)
            .await
            .expect("member handle")
            .internal_turn(ContentInput::from(text))
            .await
    })
}

fn send_task(
    handle: &MobHandle,
    identity: &AgentIdentity,
    text: String,
) -> tokio::task::JoinHandle<Result<MemberDeliveryReceipt, MobError>> {
    let handle = handle.clone();
    let identity = identity.clone();
    tokio::spawn(async move {
        handle
            .member(&identity)
            .await
            .expect("member handle")
            .send(text, HandlingMode::Queue)
            .await
    })
}

async fn timed_send(
    handle: MobHandle,
    identity: AgentIdentity,
    text: String,
    budget: Duration,
) -> (
    Duration,
    Result<Result<MemberDeliveryReceipt, MobError>, tokio::time::error::Elapsed>,
) {
    let started = Instant::now();
    let result = tokio::time::timeout(budget, async {
        handle
            .member(&identity)
            .await
            .expect("member handle")
            .send(text, HandlingMode::Queue)
            .await
    })
    .await;
    (started.elapsed(), result)
}

// ---------------------------------------------------------------------------
// Isolation
// ---------------------------------------------------------------------------

/// Member 0's live-session lookup (the inline pre-#1102 step) never answers.
/// Every other member's delivery and the liveness probe must be unaffected;
/// member 0's own caller stays pending until the lookup is released.
///
/// Fails on the pre-#1102 loop: the wedged lookup ran inline and queued
/// members 1..7 and the probe behind it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wedged_member_readiness_does_not_delay_peer_admissions() {
    let mob = create_isolation_mob(8).await;
    mob.service.park_live_session_lookups(mob.session(0)).await;

    let wedged = send_task(&mob.handle, mob.member(0), "wedged delivery".to_string());
    // Let the wedged delivery enter the actor before the peers.
    tokio::time::sleep(Duration::from_millis(100)).await;

    let started = Instant::now();
    let peers = futures::future::join_all((1..8).map(|index| {
        timed_send(
            mob.handle.clone(),
            mob.member(index).clone(),
            format!("peer delivery {index}"),
            Duration::from_secs(2),
        )
    }))
    .await;
    for (offset, (_, outcome)) in peers.into_iter().enumerate() {
        outcome
            .unwrap_or_else(|_| {
                panic!(
                    "member {} admission waited behind member 0's wedged readiness",
                    offset + 1
                )
            })
            .expect("peer delivery admitted");
    }
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "peer admissions took {:?}",
        started.elapsed()
    );
    for _ in 0..3 {
        let round_trip = mob
            .probe(Duration::from_secs(1))
            .await
            .expect("QueryPhase must answer while member 0 is wedged");
        assert!(round_trip < Duration::from_secs(1));
    }
    assert!(
        !wedged.is_finished(),
        "member 0's delivery must still be waiting on its own parked readiness"
    );

    mob.service.release_live_session_lookups().await;
    tokio::time::timeout(Duration::from_secs(2), wedged)
        .await
        .expect("released delivery completes")
        .expect("released delivery task")
        .expect("released delivery admitted");
    for index in 1..8 {
        mob.wait_for_executed_prompts(index, 1).await;
    }
}

/// OB3 shape: member 0's runtime is durability-degraded (`fail_commit` on
/// its boundary commit) AND its inline lookup is parked. The delivery must
/// be rejected typed before any dispatch work, peers must be admitted, and
/// the probe must keep answering.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn degraded_member_is_rejected_typed_without_delaying_peers() {
    let mob = create_isolation_mob(8).await;
    mob.degrade_member(0).await;
    mob.service.park_live_session_lookups(mob.session(0)).await;
    let (probe_stop, probe_pages, probe_task) =
        mob.spawn_probe_loop(Duration::from_millis(100), Duration::from_secs(1));

    let degraded = timed_send(
        mob.handle.clone(),
        mob.member(0).clone(),
        "delivery to degraded member".to_string(),
        Duration::from_secs(1),
    );
    let peers = futures::future::join_all((1..8).map(|index| {
        timed_send(
            mob.handle.clone(),
            mob.member(index).clone(),
            format!("peer delivery {index}"),
            Duration::from_secs(2),
        )
    }));
    let ((elapsed, degraded), peers) = tokio::join!(degraded, peers);
    let degraded = degraded.expect("degraded member must answer within 1 s, not time out");
    match degraded {
        Err(MobError::MemberReloadRequired { member_id, reason }) => {
            assert_eq!(&member_id, mob.member(0));
            assert!(
                reason.contains("completed_boundary_commit"),
                "reason must name the failed durable operation: {reason}"
            );
        }
        other => panic!("expected MemberReloadRequired, got {other:?}"),
    }
    assert!(
        elapsed < Duration::from_secs(1),
        "typed rejection took {elapsed:?}"
    );
    for (offset, (_, outcome)) in peers.into_iter().enumerate() {
        outcome
            .unwrap_or_else(|_| panic!("member {} admission stalled", offset + 1))
            .expect("peer delivery admitted");
    }
    probe_stop.notify_waiters();
    let slowest_probe = probe_task.await.expect("probe loop");
    assert_eq!(
        probe_pages.load(Ordering::Relaxed),
        0,
        "probe paged during run (slowest round trip {slowest_probe:?})"
    );
    mob.service.release_live_session_lookups().await;
}

/// A delivery whose caller left while it was parked behind the member's
/// in-flight admission must never execute.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn abandoned_delivery_behind_a_parked_lane_is_never_executed() {
    let mob = create_isolation_mob(2).await;
    mob.store.park_admissions(mob.session(1));

    let first = send_task(&mob.handle, mob.member(1), "first".to_string());
    let store = Arc::clone(&mob.store);
    let session = mob.session(1).clone();
    wait_until(
        "first admission parked in the store",
        Duration::from_secs(2),
        || {
            let store = Arc::clone(&store);
            let session = session.clone();
            async move { store.parked_admission_arrivals(&session) == 1 }
        },
    )
    .await;

    let abandoned = send_task(&mob.handle, mob.member(1), "abandoned".to_string());
    let handle = mob.handle.clone();
    let identity = mob.member(1).clone();
    wait_until(
        "second delivery parked in the lane",
        Duration::from_secs(2),
        || {
            let handle = handle.clone();
            let identity = identity.clone();
            async move {
                handle
                    .member_admission_backlog()
                    .parked
                    .get(&identity)
                    .copied()
                    == Some(1)
            }
        },
    )
    .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    // The caller gives up: dropping the future closes the reply channel.
    abandoned.abort();
    let _ = abandoned.await;

    mob.store.release_admissions();
    tokio::time::timeout(Duration::from_secs(2), first)
        .await
        .expect("first delivery completes after release")
        .expect("first delivery task")
        .expect("first delivery admitted");
    mob.wait_for_executed_prompts(1, 1).await;
    // Give a ghost turn every chance to appear before asserting it did not.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(mob.executed_prompts(1).await, vec!["first".to_string()]);
    assert!(
        mob.handle.member_admission_backlog().parked.is_empty(),
        "lane must be idle after the abandoned entry was skipped"
    );
}

/// Deliveries to one member execute in submission order; interleaved
/// deliveries to two members preserve each member's order independently.
/// Both ack modes share the member's lane: IngressAccepted deliveries
/// (`send`) and TurnCompleted deliveries (`internal_turn`) are interleaved
/// so a completion-bearing send cannot overtake parked ordinary ones.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn per_member_delivery_order_is_preserved() {
    let mob = create_isolation_mob(3).await;
    mob.store.park_admissions(mob.session(1));
    mob.store.park_admissions(mob.session(2));

    let mut deliveries = Vec::new();
    let plan = [
        (1, "a", false),
        (2, "x", true),
        (1, "b", true),
        (2, "y", false),
        (1, "c", false),
        (2, "z", true),
    ];
    let mut sent = HashMap::new();
    for (index, text, turn_completed) in plan {
        deliveries.push(if turn_completed {
            internal_turn_task(&mob.handle, mob.member(index), text.to_string())
        } else {
            send_task(&mob.handle, mob.member(index), text.to_string())
        });
        let count = sent.entry(index).or_insert(0usize);
        *count += 1;
        let expected_parked = *count - 1;
        // Each delivery must be in its lane before the next is sent: the
        // first occupies the (parked) in-flight slot, later ones park.
        let store = Arc::clone(&mob.store);
        let session = mob.session(index).clone();
        let handle = mob.handle.clone();
        let identity = mob.member(index).clone();
        wait_until(
            &format!("delivery {text} to member {index} in lane"),
            Duration::from_secs(2),
            || {
                let store = Arc::clone(&store);
                let session = session.clone();
                let handle = handle.clone();
                let identity = identity.clone();
                async move {
                    store.parked_admission_arrivals(&session) == 1
                        && handle
                            .member_admission_backlog()
                            .parked
                            .get(&identity)
                            .copied()
                            .unwrap_or(0)
                            == expected_parked
                }
            },
        )
        .await;
    }
    assert_eq!(mob.handle.member_admission_backlog().peak_parked, 2);

    mob.store.release_admissions();
    for delivery in deliveries {
        tokio::time::timeout(Duration::from_secs(3), delivery)
            .await
            .expect("delivery admitted after release")
            .expect("delivery task")
            .expect("delivery admitted");
    }
    mob.wait_for_executed_prompts(1, 3).await;
    mob.wait_for_executed_prompts(2, 3).await;
    assert_eq!(mob.executed_prompts(1).await, vec!["a", "b", "c"]);
    assert_eq!(mob.executed_prompts(2).await, vec!["x", "y", "z"]);
    assert!(mob.handle.member_admission_backlog().parked.is_empty());
}

/// HomeCore boot shape: many members with slow readiness. Explicit Resume
/// runs their readiness concurrently and off the loop, so it completes in
/// about one readiness latency and the probe never pages. (In-crate test
/// builds cap the resume admission deadline at 2 s, so the per-member
/// latency is 500 ms here; the pre-#1102 serial loops held the actor for
/// 17 x 500 ms, missed that deadline, and paged every probe.)
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resume_readiness_fanout_keeps_the_loop_responsive() {
    const READINESS_LATENCY: Duration = Duration::from_millis(500);
    let mob = create_isolation_mob(17).await;
    mob.handle.stop().await.expect("stop mob");
    // Exactly the 17 pre-commit readiness reads are slow. The post-commit
    // topology reconciliation still runs inline (a #1102 follow-on), so it
    // must not inherit the latency or the progress watchdog trips on it.
    mob.service.set_comms_runtime_delay_for_next_calls(
        17,
        u64::try_from(READINESS_LATENCY.as_millis()).expect("latency fits"),
    );
    let (probe_stop, probe_pages, probe_task) =
        mob.spawn_probe_loop(Duration::from_millis(100), Duration::from_secs(1));

    let started = Instant::now();
    mob.handle.resume().await.expect("resume completes");
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_secs(2),
        "resume with 17 slow members took {elapsed:?}; readiness must fan out"
    );
    assert!(
        elapsed >= READINESS_LATENCY.saturating_sub(Duration::from_millis(50)),
        "readiness latency must actually have been observed: {elapsed:?}"
    );
    probe_stop.notify_waiters();
    let slowest_probe = probe_task.await.expect("probe loop");
    assert_eq!(
        probe_pages.load(Ordering::Relaxed),
        0,
        "probe paged during resume (slowest round trip {slowest_probe:?})"
    );
    assert_eq!(
        mob.service.comms_runtime_delayed_calls_remaining(),
        0,
        "every slow readiness read must have been consumed by the fan-out"
    );
    mob.send(3, "after resume")
        .await
        .expect("delivery after resume is admitted");
    mob.wait_for_executed_prompts(3, 1).await;
}

/// Load: 48 members, 200 concurrent deliveries, member 0's durable
/// admission parked. Peer admission p99 stays tight, the probe never pages,
/// and member 0's lane depth is bounded by its own deliveries.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn load_one_wedged_member_does_not_page_or_delay_peers() {
    const MEMBERS: usize = 48;
    const DELIVERIES: usize = 200;
    let mob = create_isolation_mob(MEMBERS).await;
    mob.store.park_admissions(mob.session(0));
    // 200 deliveries queue 200 inline DSL admissions ahead of any probe; the
    // probe budget matches the admission p99 bound (a probe is one more
    // command in that queue), well below the production 30 s page.
    let (probe_stop, probe_pages, probe_task) =
        mob.spawn_probe_loop(Duration::from_millis(100), Duration::from_secs(3));

    let deliveries = futures::future::join_all((0..DELIVERIES).map(|n| {
        let index = n % MEMBERS;
        let send = timed_send(
            mob.handle.clone(),
            mob.member(index).clone(),
            format!("load delivery {n}"),
            Duration::from_secs(5),
        );
        async move {
            let (elapsed, result) = send.await;
            (index, elapsed, result)
        }
    }));
    let wedged_deliveries = (0..DELIVERIES).filter(|n| n % MEMBERS == 0).count();
    let (outcomes, peak_parked) = tokio::time::timeout(Duration::from_secs(8), async {
        // Member 0's deliveries stay parked; release them once its lane
        // holds every one of them so the join can complete.
        let store = Arc::clone(&mob.store);
        let handle = mob.handle.clone();
        let identity = mob.member(0).clone();
        let releaser = async move {
            wait_until(
                "member 0 lane holds every one of its deliveries",
                Duration::from_secs(5),
                || {
                    let handle = handle.clone();
                    let identity = identity.clone();
                    async move {
                        handle
                            .member_admission_backlog()
                            .parked
                            .get(&identity)
                            .copied()
                            .unwrap_or(0)
                            == wedged_deliveries - 1
                    }
                },
            )
            .await;
            let peak = handle.member_admission_backlog().peak_parked;
            store.release_admissions();
            peak
        };
        tokio::join!(deliveries, releaser)
    })
    .await
    .expect("load run completes");
    probe_stop.notify_waiters();
    let slowest_probe = probe_task.await.expect("probe loop");

    let mut peer_latencies = Vec::new();
    for (index, elapsed, result) in outcomes {
        result
            .unwrap_or_else(|_| panic!("delivery to member {index} timed out"))
            .unwrap_or_else(|error| panic!("delivery to member {index} failed: {error}"));
        if index != 0 {
            peer_latencies.push(elapsed);
        }
    }
    let p99 = percentile(&mut peer_latencies, 0.99);
    assert!(
        p99 < Duration::from_secs(3),
        "peer admission p99 was {p99:?}"
    );
    assert_eq!(
        probe_pages.load(Ordering::Relaxed),
        0,
        "probe paged under load (slowest round trip {slowest_probe:?}, p99 {p99:?})"
    );
    assert_eq!(peak_parked, wedged_deliveries - 1);
    assert!(peak_parked < MEMBER_ADMISSION_LANE_CAPACITY);
    assert!(mob.handle.member_admission_backlog().parked.is_empty());
}

/// The review-cycle graph (OB3's production wedge): every worker hands off to
/// one reviewer, and the reviewer is the degraded member. Each of the N-1
/// workers must keep completing its own turns while both of its hand-offs to
/// the reviewer, the direct delivery and the wired peer send, are rejected
/// typed within 1 s; the probe never pages.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn review_cycle_handoffs_to_wedged_reviewer_are_rejected_typed_while_peers_keep_working() {
    let mob = create_wired_isolation_mob(8).await;
    mob.degrade_member(0).await;
    mob.service.park_live_session_lookups(mob.session(0)).await;
    let (probe_stop, probe_pages, probe_task) =
        mob.spawn_probe_loop(Duration::from_millis(100), Duration::from_secs(1));

    let workers = futures::future::join_all((1..8).map(|index| {
        let handle = mob.handle.clone();
        let worker = mob.member(index).clone();
        let reviewer = mob.member(0).clone();
        async move {
            let own_turn = timed_send(
                handle.clone(),
                worker.clone(),
                format!("worker {index} own turn"),
                Duration::from_secs(2),
            );
            let direct_handoff = timed_send(
                handle.clone(),
                reviewer.clone(),
                format!("worker {index} review request"),
                Duration::from_secs(1),
            );
            let peer_handoff = async {
                let started = Instant::now();
                let result = tokio::time::timeout(
                    Duration::from_secs(1),
                    handle.send_peer_message(
                        worker.clone(),
                        reviewer.clone(),
                        format!("worker {index} peer review request"),
                        HandlingMode::Queue,
                    ),
                )
                .await;
                (started.elapsed(), result)
            };
            let (own_turn, direct_handoff, peer_handoff) =
                tokio::join!(own_turn, direct_handoff, peer_handoff);
            (index, own_turn, direct_handoff, peer_handoff)
        }
    }))
    .await;
    for (index, (_, own_turn), (direct_elapsed, direct), (peer_elapsed, peer)) in workers {
        own_turn
            .unwrap_or_else(|_| panic!("worker {index} own turn admission stalled"))
            .expect("worker's own turn admitted");
        let direct = direct.unwrap_or_else(|_| panic!("worker {index} direct hand-off timed out"));
        assert!(
            matches!(direct, Err(MobError::MemberReloadRequired { ref member_id, .. }) if member_id == mob.member(0)),
            "worker {index} direct hand-off must be rejected typed, got {direct:?}"
        );
        assert!(direct_elapsed < Duration::from_secs(1));
        let peer = peer.unwrap_or_else(|_| panic!("worker {index} peer hand-off timed out"));
        assert!(
            matches!(peer, Err(MobError::MemberReloadRequired { ref member_id, .. }) if member_id == mob.member(0)),
            "worker {index} peer hand-off must be rejected typed, got {peer:?}"
        );
        assert!(peer_elapsed < Duration::from_secs(1));
    }
    for index in 1..8 {
        mob.wait_for_executed_prompts(index, 1).await;
    }
    probe_stop.notify_waiters();
    let slowest_probe = probe_task.await.expect("probe loop");
    assert_eq!(
        probe_pages.load(Ordering::Relaxed),
        0,
        "probe paged during the review cycle (slowest round trip {slowest_probe:?})"
    );
    mob.service.release_live_session_lookups().await;
}

/// OB3's measured trigger before the false failure: a slow but ultimately
/// successful boundary commit on one member (a 30 s-class upload). It must
/// delay neither the other members' deliveries nor the probe, and the slow
/// member must come out healthy with its queued follow-up delivered.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn slow_but_successful_commit_on_one_member_does_not_delay_peers() {
    let mob = create_isolation_mob(8).await;
    mob.store.park_commits(mob.session(0));
    mob.send(0, "slow upload")
        .await
        .expect("delivery to the slow member is admitted");
    let store = Arc::clone(&mob.store);
    let session = mob.session(0).clone();
    wait_until(
        "member 0 boundary commit parked",
        Duration::from_secs(5),
        || {
            let store = Arc::clone(&store);
            let session = session.clone();
            async move { store.parked_commit_arrivals(&session) == 1 }
        },
    )
    .await;
    let (probe_stop, probe_pages, probe_task) =
        mob.spawn_probe_loop(Duration::from_millis(100), Duration::from_secs(1));

    // The slow member's own follow-up is serialized behind its commit by the
    // runtime's per-session driver lock. That wait is per session and must
    // stay per session: it may not leak into the peers or the probe.
    let queued_behind_slow_commit = send_task(
        &mob.handle,
        mob.member(0),
        "queued behind the slow upload".to_string(),
    );
    let peers = futures::future::join_all((1..8).map(|index| {
        timed_send(
            mob.handle.clone(),
            mob.member(index).clone(),
            format!("peer delivery {index}"),
            Duration::from_secs(2),
        )
    }))
    .await;
    for (offset, (_, outcome)) in peers.into_iter().enumerate() {
        outcome
            .unwrap_or_else(|_| panic!("member {} admission stalled", offset + 1))
            .expect("peer delivery admitted");
    }
    for index in 1..8 {
        mob.wait_for_executed_prompts(index, 1).await;
    }
    assert_eq!(
        mob.executed_prompts(0).await.len(),
        1,
        "the slow member's queued follow-up must wait for the commit"
    );
    probe_stop.notify_waiters();
    let slowest_probe = probe_task.await.expect("probe loop");
    assert_eq!(
        probe_pages.load(Ordering::Relaxed),
        0,
        "probe paged during the slow commit (slowest round trip {slowest_probe:?})"
    );

    mob.store.release_commits();
    tokio::time::timeout(Duration::from_secs(5), queued_behind_slow_commit)
        .await
        .expect("the slow member's follow-up is admitted once its commit lands")
        .expect("follow-up task")
        .expect("follow-up admitted");
    mob.wait_for_executed_prompts(0, 2).await;
    assert!(
        mob.adapter.is_durability_ready(mob.session(0)).await,
        "a slow but successful commit must leave the member durability-ready"
    );
    assert_eq!(mob.store.failed_commits(), 0);
}

/// Per-member backpressure is decided before the MobMachine SubmitWork apply:
/// a full lane refuses typed and the machine records no ingress for it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn full_member_lane_rejects_before_any_machine_transition() {
    let mob = create_isolation_mob(2).await;
    mob.store.park_admissions(mob.session(1));
    // One admission in flight (parked in the store) plus a full lane.
    let deliveries = (0..=MEMBER_ADMISSION_LANE_CAPACITY)
        .map(|n| send_task(&mob.handle, mob.member(1), format!("parked {n}")))
        .collect::<Vec<_>>();
    let handle = mob.handle.clone();
    let identity = mob.member(1).clone();
    wait_until("member 1 lane full", Duration::from_secs(5), || {
        let handle = handle.clone();
        let identity = identity.clone();
        async move {
            handle
                .member_admission_backlog()
                .parked
                .get(&identity)
                .copied()
                == Some(MEMBER_ADMISSION_LANE_CAPACITY)
        }
    })
    .await;

    let mut machine_state = mob.handle.machine_state_watch_rx.clone();
    machine_state.borrow_and_update();
    let started = Instant::now();
    let rejected = mob.send(1, "one too many").await;
    match rejected {
        Err(MobError::MemberAdmissionBacklogFull { member_id, depth }) => {
            assert_eq!(&member_id, mob.member(1));
            assert_eq!(depth, MEMBER_ADMISSION_LANE_CAPACITY);
        }
        other => panic!("expected MemberAdmissionBacklogFull, got {other:?}"),
    }
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(
        !machine_state.has_changed().expect("actor alive"),
        "a lane-full rejection must not apply a MobMachine transition"
    );

    mob.store.release_admissions();
    for delivery in deliveries {
        tokio::time::timeout(Duration::from_secs(5), delivery)
            .await
            .expect("parked delivery admitted after release")
            .expect("delivery task")
            .expect("delivery admitted");
    }
    mob.wait_for_executed_prompts(1, MEMBER_ADMISSION_LANE_CAPACITY + 1)
        .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let executed = mob.executed_prompts(1).await;
    assert_eq!(executed.len(), MEMBER_ADMISSION_LANE_CAPACITY + 1);
    assert!(
        !executed.iter().any(|prompt| prompt == "one too many"),
        "the refused delivery must never execute"
    );
    assert!(mob.handle.member_admission_backlog().parked.is_empty());
}

/// A member retired while its readiness step is in flight must not fail the
/// explicit Resume of everyone else: its stale outcome is dropped.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resume_survives_a_member_retired_during_the_readiness_fanout() {
    let mob = create_isolation_mob(17).await;
    mob.handle.stop().await.expect("stop mob");
    mob.service.set_comms_runtime_delay_for_next_calls(17, 500);

    let resume = {
        let handle = mob.handle.clone();
        tokio::spawn(async move { handle.resume().await })
    };
    // Interleave the retire with the fan-out: wait until readiness steps have
    // entered their slow comms reads, which only happens off the loop.
    let service = Arc::clone(&mob.service);
    wait_until("readiness fan-out started", Duration::from_secs(2), || {
        let service = Arc::clone(&service);
        async move { service.comms_runtime_delayed_calls_remaining() < 17 }
    })
    .await;
    mob.handle
        .retire(mob.member(3).clone())
        .await
        .expect("retire during the readiness fan-out");

    tokio::time::timeout(Duration::from_secs(5), resume)
        .await
        .expect("resume completes")
        .expect("resume task")
        .expect("resume must succeed for the remaining members");
    assert!(
        mob.handle
            .get_member(mob.member(3))
            .await
            .expect("read roster")
            .is_none(),
        "the retired member stays retired"
    );
    mob.send(4, "after resume")
        .await
        .expect("delivery after resume is admitted");
    mob.wait_for_executed_prompts(4, 1).await;
}

// ---------------------------------------------------------------------------
// Reload primitive
// ---------------------------------------------------------------------------

/// While the durable session authority is unreadable (the store is still
/// failing), a reload must be refused typed BEFORE the live shell is
/// discarded: the member stays degraded and repairable instead of becoming
/// Broken through a revival that could only fail.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reload_is_refused_typed_while_durable_authority_is_unreadable() {
    let mob = create_isolation_mob(2).await;
    mob.degrade_member(0).await;
    // The store keeps failing and the durable authority read reports nothing.
    mob.service.set_resume_authority_absent_remaining(1);

    let refused = tokio::time::timeout(
        Duration::from_secs(5),
        mob.handle.reload_member_registration(mob.member(0)),
    )
    .await
    .expect("reload answers");
    match refused {
        Err(MobError::MemberReloadRefused { session_id, reason }) => {
            assert_eq!(&session_id, mob.session(0));
            assert!(
                reason.contains("not resumable"),
                "refusal names the unreadable authority: {reason}"
            );
        }
        other => panic!("expected MemberReloadRefused, got {other:?}"),
    }
    assert!(
        mob.adapter
            .durability_reload_required(mob.session(0))
            .await
            .is_some(),
        "the degraded registration must be retained, not discarded"
    );
    assert!(
        mob.service
            .has_live_session(mob.session(0))
            .await
            .expect("has_live_session"),
        "the live shell must be retained"
    );
    assert!(
        matches!(
            mob.send(0, "still degraded").await,
            Err(MobError::MemberReloadRequired { .. })
        ),
        "the member stays degraded-and-repairable, never Broken"
    );

    // The store recovers and the authority is readable again: the reload
    // proceeds.
    mob.store.fail_commit(mob.session(0), false);
    let outcome = tokio::time::timeout(
        Duration::from_secs(5),
        mob.handle.reload_member_registration(mob.member(0)),
    )
    .await
    .expect("reload completes")
    .expect("reload succeeds once the authority is readable");
    assert_eq!(outcome.disposition, MemberReloadDisposition::Discarded);
    mob.send(0, "after reload")
        .await
        .expect("delivery after reload");
    mob.wait_for_executed_prompts(0, 2).await;
}

/// Degrade member 0 through a failed boundary commit, reload its runtime
/// registration in place, and deliver again on the same session and
/// generation. A second reload is a typed no-op.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reload_member_registration_replaces_the_degraded_registration_in_place() {
    let mob = create_isolation_mob(2).await;
    let generation_before = mob
        .handle
        .member_status(mob.member(0))
        .await
        .expect("member status")
        .agent_runtime_id
        .expect("active runtime id")
        .generation;
    mob.degrade_member(0).await;
    assert!(matches!(
        mob.send(0, "rejected while degraded").await,
        Err(MobError::MemberReloadRequired { .. })
    ));
    // The store recovers (OB3: the continuity backend came back) but the
    // shell stays fail-closed until a registration-authorized reload.
    mob.store.fail_commit(mob.session(0), false);
    assert!(matches!(
        mob.send(0, "still rejected after the store recovered")
            .await,
        Err(MobError::MemberReloadRequired { .. })
    ));

    let outcome = tokio::time::timeout(
        Duration::from_secs(5),
        mob.handle.reload_member_registration(mob.member(0)),
    )
    .await
    .expect("reload completes")
    .expect("reload succeeds");
    assert_eq!(outcome.disposition, MemberReloadDisposition::Discarded);
    assert_eq!(&outcome.session_id, mob.session(0));
    assert_eq!(outcome.generation, generation_before);
    assert!(
        mob.adapter.is_durability_ready(mob.session(0)).await,
        "the fresh registration must be durability-ready"
    );
    assert!(
        mob.adapter
            .current_executor_attachment_witness(mob.session(0))
            .await
            .is_some(),
        "the same session must have a fresh executor registration"
    );
    assert!(
        mob.service
            .has_live_session(mob.session(0))
            .await
            .expect("has_live_session"),
        "the live session must be re-materialized"
    );
    let generation_after = mob
        .handle
        .member_status(mob.member(0))
        .await
        .expect("member status")
        .agent_runtime_id
        .expect("active runtime id")
        .generation;
    assert_eq!(
        generation_after, generation_before,
        "reload keeps continuity"
    );

    let executed_before = mob.executed_prompts(0).await.len();
    mob.send(0, "after reload")
        .await
        .expect("delivery after reload is admitted");
    mob.wait_for_executed_prompts(0, executed_before + 1).await;
    assert!(mob.adapter.is_durability_ready(mob.session(0)).await);

    let again = mob
        .handle
        .reload_member_registration(mob.member(0))
        .await
        .expect("second reload succeeds");
    assert_eq!(again.disposition, MemberReloadDisposition::NotDegraded);
    assert_eq!(again.generation, generation_before);
}

/// A reply timeout does not cancel admission already in flight. Only the
/// second delivery, still parked in the member lane when its caller leaves,
/// is skipped.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bounded_submit_timeout_does_not_cancel_inflight_admission_but_skips_parked_delivery() {
    let mob = create_isolation_mob(2).await;
    mob.store.park_admissions(mob.session(1));
    let status = mob
        .handle
        .member_status(mob.member(1))
        .await
        .expect("member status");
    let runtime_id = status.agent_runtime_id.expect("runtime id");
    let fence_token = status.fence_token.expect("fence token");
    let first_deadline = Instant::now() + Duration::from_secs(2);
    let first = tokio::spawn({
        let handle = mob.handle.clone();
        let runtime_id = runtime_id.clone();
        async move {
            handle
                .submit_work_with_mode_bounded(
                    runtime_id,
                    fence_token,
                    WorkRef::new(),
                    WorkSpec::new("bounded delivery", WorkOrigin::External),
                    HandlingMode::Queue,
                    first_deadline,
                )
                .await
        }
    });
    wait_until(
        "bounded delivery admission entered the store",
        Duration::from_secs(2),
        || async { mob.store.parked_admission_arrivals(mob.session(1)) == 1 },
    )
    .await;
    assert!(
        Instant::now() < first_deadline,
        "runtime admission must be in flight before the caller deadline"
    );
    let error = first
        .await
        .expect("bounded delivery task")
        .expect_err("in-flight admission outlives the observation deadline");
    let data = error.structured_data().expect("typed timeout data");
    assert!(data.get("executed").is_none());
    assert!(data.get("retryable").is_none());
    match error {
        MobError::ActorCommandTimedOut {
            command_kind,
            stage,
        } => {
            assert_eq!(command_kind, "SubmitWork");
            assert_eq!(stage, "actor_command_reply");
        }
        other => panic!("expected ActorCommandTimedOut, got {other:?}"),
    }
    // The first admission is still in the store; the second delivery parks
    // behind it and its caller leaves before the lane reaches it.
    let second_deadline = Instant::now() + Duration::from_secs(2);
    let second = tokio::spawn({
        let handle = mob.handle.clone();
        async move {
            handle
                .submit_work_with_mode_bounded(
                    runtime_id,
                    fence_token,
                    WorkRef::new(),
                    WorkSpec::new("second bounded delivery", WorkOrigin::External),
                    HandlingMode::Queue,
                    second_deadline,
                )
                .await
        }
    });
    wait_until(
        "second bounded delivery parked in the member lane",
        Duration::from_secs(2),
        || async {
            mob.handle
                .member_admission_backlog()
                .parked
                .get(mob.member(1))
                .copied()
                == Some(1)
        },
    )
    .await;
    assert!(
        Instant::now() < second_deadline,
        "second delivery must park before its caller deadline"
    );
    assert!(matches!(
        second.await.expect("second bounded delivery task"),
        Err(MobError::ActorCommandTimedOut {
            command_kind: "SubmitWork",
            stage: "actor_command_reply",
        })
    ));
    mob.store.release_admissions();
    // Completing later work in this same FIFO proves the abandoned entry has
    // been consumed, rather than merely waiting a fixed time for a ghost.
    let settled = internal_turn_task(&mob.handle, mob.member(1), "settlement witness".to_string());
    tokio::time::timeout(Duration::from_secs(5), settled)
        .await
        .expect("later turn completes")
        .expect("later turn task")
        .expect("later turn receipt");
    wait_until(
        "member admission backlog drained",
        Duration::from_secs(2),
        || async { mob.handle.member_admission_backlog().parked.is_empty() },
    )
    .await;
    assert_eq!(
        mob.executed_prompts(1).await,
        vec!["bounded delivery", "settlement witness"],
        "the in-flight first delivery executes exactly once; the parked second never executes"
    );
}
