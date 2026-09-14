use super::*;
use crate::input_state::{InputStatePersistenceRecord, StoredInputState};
use crate::live_ledger::authority::store::LiveRequestStoreOwner;
use crate::store::live_read::{
    LiveCompositeCapture, LiveCompositeReadProfile, LiveCompositeReadRequest,
    LiveLedgerWriteProfile,
};
use crate::store::*;
use meerkat_core::RunBoundaryReceipt;
use meerkat_core::execution_scope::ScopedRunAuthority;
use meerkat_core::lifecycle::{InputId, RunId};
use std::sync::atomic::{AtomicBool, AtomicUsize};

#[derive(Clone)]
enum Collision {
    Advance,
    Revoke,
    Policy(Arc<AtomicBool>),
    Lifecycle(Arc<MeerkatMachine>),
    Actor,
    Source,
    LostAcknowledgement,
    AlreadyCommitted,
}

struct CollisionStore {
    inner: Arc<dyn RuntimeStore>,
    scope: ScopedRunAuthority,
    source: meerkat_core::live_execution::request::LiveSourceKey,
    collisions: AtomicUsize,
    commits: AtomicUsize,
    action: Collision,
}

impl CollisionStore {
    fn new(
        owned: &OwnedFixture,
        scope: ScopedRunAuthority,
        collisions: usize,
        action: Collision,
    ) -> Arc<Self> {
        Arc::new(Self {
            inner: owned.fixture.store.clone(),
            scope,
            source: owned.source.clone(),
            collisions: AtomicUsize::new(collisions),
            commits: AtomicUsize::new(0),
            action,
        })
    }

    fn owner(self: &Arc<Self>) -> LiveRequestStoreOwner {
        LiveRequestStoreOwner::new(
            self.clone(),
            self.scope.record().executor.session_id.clone(),
        )
    }

    fn inner_ops(&self) -> &dyn RuntimeLiveLedgerOps {
        self.inner.live_ledger_ops().expect("fixture Live store")
    }
}

#[async_trait::async_trait]
impl RuntimeLiveLedgerOps for CollisionStore {
    fn composite_read_profile(&self) -> LiveCompositeReadProfile {
        self.inner_ops().composite_read_profile()
    }

    fn ledger_write_profile(&self) -> LiveLedgerWriteProfile {
        self.inner_ops().ledger_write_profile()
    }

    async fn capture_live_composite(
        &self,
        request: &LiveCompositeReadRequest,
    ) -> Result<Option<LiveCompositeCapture>, RuntimeStoreError> {
        self.inner_ops().capture_live_composite(request).await
    }

    async fn lookup_live_source(
        &self,
        source: &meerkat_core::live_execution::request::LiveSourceKey,
    ) -> Result<Option<crate::live_ledger::source::LiveSourceRow>, RuntimeStoreError> {
        self.inner_ops().lookup_live_source(source).await
    }

    async fn load_live_head(
        &self,
        session_id: &meerkat_core::SessionId,
    ) -> Result<Option<LiveLedgerStoredHead>, RuntimeStoreError> {
        self.inner_ops().load_live_head(session_id).await
    }

    async fn commit_live_ledger(
        &self,
        prepared: PreparedLiveLedgerCommit,
        fence: Arc<dyn RuntimeStoreWriteFence>,
    ) -> Result<LiveLedgerCommitOutcome, RuntimeStoreError> {
        self.commits.fetch_add(1, Ordering::SeqCst);
        let collide = self
            .collisions
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |count| {
                count.checked_sub(1)
            })
            .is_ok();
        if !collide {
            return self.inner_ops().commit_live_ledger(prepared, fence).await;
        }
        let session_id = &self.scope.record().executor.session_id;
        let runtime_id = LogicalRuntimeId::for_session(session_id);
        let owner = LiveRequestStoreOwner::new(self.inner.clone(), session_id.clone());
        match &self.action {
            Collision::Actor => {
                return Ok(LiveLedgerCommitOutcome::ActorConflict {
                    current: self
                        .inner
                        .load_session_boundary_authority(&runtime_id)
                        .await?,
                });
            }
            Collision::Source => {
                return Ok(LiveLedgerCommitOutcome::SourceConflict {
                    source: self.source.clone(),
                    current: self
                        .inner_ops()
                        .lookup_live_source(&self.source)
                        .await?
                        .map(|row| row.digest()),
                });
            }
            Collision::AlreadyCommitted | Collision::LostAcknowledgement => {
                let committed = self.inner_ops().commit_live_ledger(prepared, fence).await?;
                let LiveLedgerCommitOutcome::Committed { head } = committed else {
                    return Err(RuntimeStoreError::WriteFailed(
                        "fault fixture failed to commit".into(),
                    ));
                };
                return if matches!(self.action, Collision::AlreadyCommitted) {
                    Ok(LiveLedgerCommitOutcome::AlreadyCommitted { head })
                } else {
                    Err(RuntimeStoreError::WriteFailed(
                        "injected lost acknowledgement".into(),
                    ))
                };
            }
            Collision::Revoke => {
                owner
                    .commit(
                        crate::live_ledger::authority::dsl::LiveRequestInput::Revoke {
                            grant_id: self.scope.record().grant.id.as_uuid().to_string(),
                            generation: self.scope.record().grant.generation.get(),
                        },
                        current_fence(),
                    )
                    .await
                    .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
            }
            Collision::Advance | Collision::Policy(_) | Collision::Lifecycle(_) => {
                owner
                    .restore_run_scope(self.scope.scope_id(), self.scope.record().clone())
                    .await
                    .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
            }
        }
        let outcome = self.inner_ops().commit_live_ledger(prepared, fence).await?;
        if !matches!(outcome, LiveLedgerCommitOutcome::Conflict { .. }) {
            return Err(RuntimeStoreError::WriteFailed(format!(
                "fixture must induce actual head conflict, got {outcome:?}"
            )));
        }
        match &self.action {
            Collision::Policy(current) => {
                current.store(false, Ordering::SeqCst);
            }
            Collision::Lifecycle(machine) => {
                machine
                    .stop_runtime_executor(session_id, "collision lifecycle fence")
                    .await
                    .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
            }
            _ => {}
        }
        Ok(outcome)
    }
}

#[async_trait::async_trait]
impl RuntimeStore for CollisionStore {
    fn session_authority_ops(&self) -> &dyn RuntimeSessionAuthorityOps {
        self.inner.session_authority_ops()
    }

    fn live_ledger_ops(&self) -> Option<&dyn RuntimeLiveLedgerOps> {
        Some(self)
    }

    async fn commit_session_snapshot(
        &self,
        id: &LogicalRuntimeId,
        delta: SerializedSessionSnapshot,
    ) -> Result<(), RuntimeStoreError> {
        self.inner.commit_session_snapshot(id, delta).await
    }

    async fn commit_prepared_whole_blob_rewrite_boundary(
        &self,
        id: &LogicalRuntimeId,
        boundary: PreparedWholeBlobRewriteStoreParts,
    ) -> Result<WholeBlobStoreAuthority, RuntimeStoreError> {
        self.inner
            .commit_prepared_whole_blob_rewrite_boundary(id, boundary)
            .await
    }

    async fn atomic_apply(
        &self,
        id: &LogicalRuntimeId,
        delta: Option<SerializedSessionSnapshot>,
        receipt: RunBoundaryReceipt,
        updates: Vec<InputStatePersistenceRecord>,
        key: Option<meerkat_core::SessionId>,
    ) -> Result<(), RuntimeStoreError> {
        self.inner
            .atomic_apply(id, delta, receipt, updates, key)
            .await
    }

    async fn load_input_states(
        &self,
        id: &LogicalRuntimeId,
    ) -> Result<Vec<InputStateRow>, RuntimeStoreError> {
        self.inner.load_input_states(id).await
    }

    async fn load_boundary_receipt(
        &self,
        id: &LogicalRuntimeId,
        run: &RunId,
        sequence: u64,
    ) -> Result<Option<RunBoundaryReceipt>, RuntimeStoreError> {
        self.inner.load_boundary_receipt(id, run, sequence).await
    }

    async fn load_session_snapshot(
        &self,
        id: &LogicalRuntimeId,
    ) -> Result<Option<Arc<Vec<u8>>>, RuntimeStoreError> {
        self.inner.load_session_snapshot(id).await
    }

    async fn clear_session_snapshot(&self, id: &LogicalRuntimeId) -> Result<(), RuntimeStoreError> {
        self.inner.clear_session_snapshot(id).await
    }

    async fn replace_session_snapshot_if_current(
        &self,
        id: &LogicalRuntimeId,
        expected: &[u8],
        replacement: Vec<u8>,
    ) -> Result<bool, RuntimeStoreError> {
        self.inner
            .replace_session_snapshot_if_current(id, expected, replacement)
            .await
    }

    async fn clear_session_snapshot_if_current(
        &self,
        id: &LogicalRuntimeId,
        expected: &[u8],
    ) -> Result<bool, RuntimeStoreError> {
        self.inner
            .clear_session_snapshot_if_current(id, expected)
            .await
    }

    async fn persist_input_state(
        &self,
        id: &LogicalRuntimeId,
        state: &InputStatePersistenceRecord,
    ) -> Result<(), RuntimeStoreError> {
        self.inner.persist_input_state(id, state).await
    }

    async fn load_input_state(
        &self,
        id: &LogicalRuntimeId,
        input: &InputId,
    ) -> Result<Option<StoredInputState>, RuntimeStoreError> {
        self.inner.load_input_state(id, input).await
    }

    async fn load_input_state_by_idempotency_key(
        &self,
        id: &LogicalRuntimeId,
        key: &crate::identifiers::IdempotencyKey,
    ) -> Result<Option<ExactInputStateObservation>, RuntimeStoreError> {
        self.inner
            .load_input_state_by_idempotency_key(id, key)
            .await
    }

    async fn load_machine_lifecycle_record(
        &self,
        id: &LogicalRuntimeId,
    ) -> Result<Option<Vec<u8>>, RuntimeStoreError> {
        self.inner.load_machine_lifecycle_record(id).await
    }

    async fn observe_machine_lifecycle(
        &self,
        id: &LogicalRuntimeId,
    ) -> Result<MachineLifecycleObservation, RuntimeStoreError> {
        self.inner.observe_machine_lifecycle(id).await
    }

    async fn commit_machine_lifecycle(
        &self,
        id: &LogicalRuntimeId,
        commit: MachineLifecycleCommit,
        inputs: &[InputStatePersistenceRecord],
    ) -> Result<(), RuntimeStoreError> {
        self.inner
            .commit_machine_lifecycle(id, commit, inputs)
            .await
    }
}

struct CurrentPolicy(Arc<AtomicBool>);

impl RuntimeStoreWriteFence for CurrentPolicy {
    fn execute_if_current(
        &self,
        operation: Box<dyn FnOnce() -> Result<(), RuntimeStoreError> + '_>,
    ) -> Result<RuntimeStoreWriteFenceOutcome, RuntimeStoreError> {
        if !self.0.load(Ordering::SeqCst) {
            return Ok(RuntimeStoreWriteFenceOutcome::Conflict {
                reason: "policy changed".into(),
            });
        }
        operation()?;
        Ok(RuntimeStoreWriteFenceOutcome::Applied)
    }
}

#[tokio::test]
async fn native_claim_collision_reprepares_same_effect_and_bounds_contention() -> TestResult {
    for backend in backends() {
        for collisions in [1, 8] {
            let owned = OwnedFixture::new(backend).await?;
            let scope = owned.staged_scope().await?;
            let store = CollisionStore::new(&owned, scope.clone(), collisions, Collision::Advance);
            let effect_id = OperationId::new();
            let target = tool_target("allowed_tool", ToolMutationClass::ReadOnly);
            let result = store
                .owner()
                .claim_effect(
                    scope,
                    effect_id.clone(),
                    target.clone(),
                    read_only_observation()?,
                )
                .await;
            let head = owned
                .fixture
                .ops()?
                .load_live_head(owned.fixture.session.id())
                .await?
                .ok_or("head")?;
            let state =
                crate::generated::live_request_state::decode(&head.payload.request_snapshot)?;
            if collisions == 1 {
                let permit = result?;
                assert_eq!(permit.claim().effect_id, effect_id);
                assert_eq!(permit.claim().target, target);
                assert_eq!(store.commits.load(Ordering::SeqCst), 2);
                assert_eq!(state.claim_records.len(), 1);
            } else {
                assert!(matches!(result,
                    Err(LiveRequestAuthorityError::NotNewlyCommitted(outcome))
                        if matches!(*outcome, LiveLedgerCommitOutcome::Conflict { .. })));
                assert_eq!(store.commits.load(Ordering::SeqCst), 8);
                assert!(state.claim_records.is_empty());
            }
        }
    }
    Ok(())
}

#[tokio::test]
async fn native_claim_collision_rechecks_revocation_policy_and_lifecycle() -> TestResult {
    for backend in backends() {
        for variant in 0..3 {
            let owned = OwnedFixture::new(backend).await?;
            let scope = owned.staged_scope().await?;
            let current = Arc::new(AtomicBool::new(true));
            let action = match variant {
                0 => Collision::Revoke,
                1 => Collision::Policy(current.clone()),
                _ => Collision::Lifecycle(owned.machine.clone()),
            };
            let store = CollisionStore::new(&owned, scope.clone(), 1, action);
            let policy = policy_observation(
                ToolExecutionPolicy::resolve(ToolAccessPolicy::ReadOnly)?,
                Arc::new(CurrentPolicy(current)),
            )?;
            let result = store
                .owner()
                .claim_effect(
                    scope,
                    OperationId::new(),
                    tool_target("allowed_tool", ToolMutationClass::ReadOnly),
                    policy,
                )
                .await;
            match variant {
                0 => assert!(matches!(
                    result,
                    Err(LiveRequestAuthorityError::Transition(_))
                )),
                1 => assert!(matches!(
                    result,
                    Err(LiveRequestAuthorityError::Store(
                        RuntimeStoreError::WriteFenceConflict { .. }
                    ))
                )),
                _ => assert!(matches!(
                    result,
                    Err(LiveRequestAuthorityError::ScopeNotCurrent(_))
                )),
            }
            assert_eq!(
                store.commits.load(Ordering::SeqCst),
                if variant == 1 { 2 } else { 1 }
            );
            let head = owned
                .fixture
                .ops()?
                .load_live_head(owned.fixture.session.id())
                .await?
                .ok_or("head")?;
            assert!(
                crate::generated::live_request_state::decode(&head.payload.request_snapshot)?
                    .claim_records
                    .is_empty()
            );
        }
    }
    Ok(())
}

#[tokio::test]
async fn native_claim_collision_does_not_retry_other_conflicts_or_ambiguous_commit() -> TestResult {
    for backend in backends() {
        for action in [
            Collision::Actor,
            Collision::Source,
            Collision::AlreadyCommitted,
            Collision::LostAcknowledgement,
        ] {
            let committed = matches!(
                action,
                Collision::AlreadyCommitted | Collision::LostAcknowledgement
            );
            let owned = OwnedFixture::new(backend).await?;
            let scope = owned.staged_scope().await?;
            let store = CollisionStore::new(&owned, scope.clone(), 1, action);
            let effect_id = OperationId::new();
            let result = store
                .owner()
                .claim_effect(
                    scope.clone(),
                    effect_id.clone(),
                    tool_target("allowed_tool", ToolMutationClass::ReadOnly),
                    read_only_observation()?,
                )
                .await;
            assert!(result.is_err());
            assert_eq!(store.commits.load(Ordering::SeqCst), 1);
            let head = owned
                .fixture
                .ops()?
                .load_live_head(owned.fixture.session.id())
                .await?
                .ok_or("head")?;
            assert_eq!(
                crate::generated::live_request_state::decode(&head.payload.request_snapshot)?
                    .claim_records
                    .len(),
                usize::from(committed)
            );
            if committed {
                assert!(
                    store
                        .owner()
                        .claim_effect(
                            scope,
                            effect_id,
                            tool_target("allowed_tool", ToolMutationClass::ReadOnly),
                            read_only_observation()?,
                        )
                        .await
                        .is_err()
                );
                assert_eq!(store.commits.load(Ordering::SeqCst), 1);
            }
        }
    }
    Ok(())
}
