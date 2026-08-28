use super::*;
use crate::input_state::StoredInputState;
use crate::terminal_status::{
    self, InteractionSelector, Sourced, TerminalWitnessSource, interaction_report,
};
use meerkat_core::ToolName;

#[path = "../user_interrupt.rs"]
mod user_interrupt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SessionBindingPreparation {
    /// The runtime binding itself is the semantic event: apply
    /// `PrepareBindings` to MeerkatMachine and publish any routed seam signals.
    AuthoritativeRuntimeBinding,
    /// Only create the session-local handle bundle. A separate owner will
    /// route the authoritative binding later.
    LocalSessionResources(LocalSessionMaterializationMode),
}

struct RuntimeCompactionCommitCoordinator {
    session_id: SessionId,
    runtime_binding: std::sync::Mutex<
        Option<(
            crate::meerkat_machine::dsl::AgentRuntimeId,
            Option<crate::meerkat_machine::dsl::FenceToken>,
            Option<crate::meerkat_machine::dsl::Generation>,
        )>,
    >,
    /// The epoch this coordinator authorizes against. Re-derivable: the DSL
    /// authority owns the session's live epoch, so a coordinator whose epoch
    /// rotated under it can adopt the live value once per authorization instead
    /// of wedging the session's durable compaction forever.
    runtime_epoch_id: std::sync::Mutex<crate::meerkat_machine::dsl::RuntimeEpochId>,
    allow_late_binding: bool,
    dsl_authority: Arc<crate::handles::HandleDslAuthority>,
    store: Option<Arc<dyn crate::store::RuntimeStore>>,
}

struct RuntimeStickyModelFallbackCommitCoordinator {
    session_id: SessionId,
    store: Option<Arc<dyn crate::store::RuntimeStore>>,
}

struct RuntimeStickyModelFallbackCommitOperation {
    result_rx: crate::tokio::sync::watch::Receiver<
        Option<
            Result<
                Option<meerkat_core::SessionControlCommitReceipt>,
                meerkat_core::handles::StickyModelFallbackCommitError,
            >,
        >,
    >,
}

#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
impl meerkat_core::handles::StickyModelFallbackCommitOperation
    for RuntimeStickyModelFallbackCommitOperation
{
    async fn wait(
        &self,
    ) -> Result<
        Option<meerkat_core::SessionControlCommitReceipt>,
        meerkat_core::handles::StickyModelFallbackCommitError,
    > {
        let mut result_rx = self.result_rx.clone();
        loop {
            if let Some(result) = result_rx.borrow().clone() {
                return result;
            }
            if result_rx.changed().await.is_err() {
                return Err(meerkat_core::handles::StickyModelFallbackCommitError::SupervisorLost);
            }
        }
    }
}

impl meerkat_core::handles::StickyModelFallbackCommitCoordinator
    for RuntimeStickyModelFallbackCommitCoordinator
{
    fn begin(
        &self,
        machine_commit: Box<dyn meerkat_core::handles::StickyModelFallbackMachineCommit>,
        control_delta: meerkat_core::handles::StickyModelFallbackControlDelta,
    ) -> Result<
        Arc<dyn meerkat_core::handles::StickyModelFallbackCommitOperation>,
        meerkat_core::handles::StickyModelFallbackCommitError,
    > {
        let Some(store) = self.store.clone() else {
            // Ephemeral runtimes have no recovery boundary to split. Consume
            // the generated one-shot commit synchronously and return the same
            // retained-result operation shape used by durable runtimes.
            let result = machine_commit
                .commit()
                .map_err(meerkat_core::handles::StickyModelFallbackCommitError::MachineRejected);
            let (_result_tx, result_rx) =
                crate::tokio::sync::watch::channel(Some(result.map(|()| None)));
            return Ok(Arc::new(RuntimeStickyModelFallbackCommitOperation {
                result_rx,
            }));
        };
        let session_id = self.session_id.clone();
        let (result_tx, result_rx) = crate::tokio::sync::watch::channel(None);
        crate::tokio::spawn(async move {
            let result =
                run_sticky_model_fallback_commit(session_id, store, machine_commit, control_delta)
                    .await;
            let _ = result_tx.send(Some(result));
        });
        Ok(Arc::new(RuntimeStickyModelFallbackCommitOperation {
            result_rx,
        }))
    }
}

async fn run_sticky_model_fallback_commit(
    session_id: SessionId,
    store: Arc<dyn crate::store::RuntimeStore>,
    machine_commit: Box<dyn meerkat_core::handles::StickyModelFallbackMachineCommit>,
    control_delta: meerkat_core::handles::StickyModelFallbackControlDelta,
) -> Result<
    Option<meerkat_core::SessionControlCommitReceipt>,
    meerkat_core::handles::StickyModelFallbackCommitError,
> {
    match store.session_persistence_profile() {
        crate::store::RuntimeSessionPersistenceProfile::WholeBlobV1 => {
            run_sticky_model_fallback_whole_blob_commit(
                session_id,
                store,
                machine_commit,
                control_delta,
            )
            .await
        }
        crate::store::RuntimeSessionPersistenceProfile::HeadCanonicalV1 => {
            run_sticky_model_fallback_head_canonical_commit(
                session_id,
                store,
                machine_commit,
                control_delta,
            )
            .await
        }
    }
}

async fn run_sticky_model_fallback_whole_blob_commit(
    session_id: SessionId,
    store: Arc<dyn crate::store::RuntimeStore>,
    machine_commit: Box<dyn meerkat_core::handles::StickyModelFallbackMachineCommit>,
    control_delta: meerkat_core::handles::StickyModelFallbackControlDelta,
) -> Result<
    Option<meerkat_core::SessionControlCommitReceipt>,
    meerkat_core::handles::StickyModelFallbackCommitError,
> {
    use meerkat_core::handles::StickyModelFallbackCommitError as CommitError;

    let runtime_id = crate::identifiers::LogicalRuntimeId::for_session(&session_id);
    let committed = store
        .load_committed_whole_blob_snapshot(&runtime_id)
        .await
        .map_err(|error| CommitError::Store(error.to_string()))?
        .ok_or_else(|| CommitError::SnapshotMissing {
            session_id: session_id.clone(),
        })?;
    if committed.session().id() != &session_id {
        return Err(CommitError::SessionMismatch {
            expected: session_id,
            actual: committed.session().id().clone(),
        });
    }
    let previous_session = committed.session_arc();
    let previous_authority = committed.authority().clone();
    let mut target_session = previous_session.as_ref().clone();
    control_delta
        .validate_and_apply(&mut target_session)
        .map_err(CommitError::InvalidControlDelta)?;
    let target_commit = meerkat_core::lifecycle::core_executor::BoundSessionCommit::sealed(
        Arc::new(target_session),
    )
    .map_err(|error| CommitError::SnapshotInvalid(error.to_string()))?;
    let target_prepared = crate::store::PreparedWholeBlobSnapshotCas::prepare(
        previous_authority.clone(),
        target_commit,
    )
    .map_err(|error| CommitError::SnapshotInvalid(error.to_string()))?;

    let target_authority = match store
        .commit_prepared_whole_blob_snapshot_cas(&runtime_id, target_prepared.clone())
        .await
    {
        Ok(crate::store::WholeBlobSnapshotCasOutcome::Committed(authority))
            if target_prepared.accepts_committed_authority(&authority) =>
        {
            authority
        }
        Ok(crate::store::WholeBlobSnapshotCasOutcome::Committed(authority)) => {
            return Err(CommitError::SnapshotOutcomeUnknown(format!(
                "WholeBlob snapshot CAS acknowledged unexpected authority revision {} token {}",
                authority.store_revision(),
                authority.blob_sha256()
            )));
        }
        Ok(crate::store::WholeBlobSnapshotCasOutcome::Conflict) => {
            return Err(CommitError::SnapshotConflict);
        }
        Err(cas_error) => {
            let observed = store
                .load_whole_blob_store_authority(&runtime_id)
                .await
                .map_err(|read_error| {
                    CommitError::SnapshotOutcomeUnknown(format!(
                        "compare-and-swap failed with '{cas_error}' and bounded authority reconciliation failed with '{read_error}'"
                    ))
                })?;
            match observed {
                Some(observed) if target_prepared.accepts_committed_authority(&observed) => {
                    observed
                }
                Some(observed) if observed == previous_authority => {
                    return Err(CommitError::Store(cas_error.to_string()));
                }
                _ => {
                    return Err(CommitError::SnapshotOutcomeUnknown(cas_error.to_string()));
                }
            }
        }
    };

    if let Err(machine_error) = machine_commit.commit() {
        let rollback_commit =
            meerkat_core::lifecycle::core_executor::BoundSessionCommit::sealed(previous_session)
                .map_err(|error| {
                    CommitError::CompensationFailed(format!(
                        "{machine_error}; failed to seal durable predecessor: {error}"
                    ))
                })?;
        let rollback_prepared = crate::store::PreparedWholeBlobSnapshotCas::prepare(
            target_authority.clone(),
            rollback_commit,
        )
        .map_err(|error| {
            CommitError::CompensationFailed(format!(
                "{machine_error}; failed to prepare durable predecessor: {error}"
            ))
        })?;
        let rollback_result = store
            .commit_prepared_whole_blob_snapshot_cas(&runtime_id, rollback_prepared.clone())
            .await;
        match rollback_result {
            Ok(crate::store::WholeBlobSnapshotCasOutcome::Committed(authority))
                if rollback_prepared.accepts_committed_authority(&authority) =>
            {
                Err(CommitError::MachineRejected(machine_error))
            }
            result => {
                let observed = store.load_whole_blob_store_authority(&runtime_id).await;
                match observed {
                    Ok(Some(observed))
                        if rollback_prepared.accepts_committed_authority(&observed) =>
                    {
                        Err(CommitError::MachineRejected(machine_error))
                    }
                    Ok(Some(observed)) if observed == target_authority => {
                        let detail = match result {
                            Ok(crate::store::WholeBlobSnapshotCasOutcome::Conflict) => {
                                "durable target snapshot remains committed after compensation conflict"
                                    .to_string()
                            }
                            Ok(crate::store::WholeBlobSnapshotCasOutcome::Committed(authority)) => {
                                format!(
                                    "compensation acknowledged unexpected authority revision {} token {}",
                                    authority.store_revision(),
                                    authority.blob_sha256()
                                )
                            }
                            Err(error) => format!(
                                "durable target snapshot remains committed after compensation error: {error}"
                            ),
                        };
                        Err(CommitError::CompensationFailed(format!(
                            "{machine_error}; {detail}"
                        )))
                    }
                    Ok(Some(_)) => Err(CommitError::CompensationFailed(format!(
                        "{machine_error}; a competing durable snapshot replaced the target during compensation"
                    ))),
                    Ok(None) => Err(CommitError::CompensationFailed(format!(
                        "{machine_error}; durable snapshot disappeared during compensation"
                    ))),
                    Err(read_error) => Err(CommitError::CompensationFailed(format!(
                        "{machine_error}; compensation result could not be reconciled from bounded authority: {read_error}"
                    ))),
                }
            }
        }
    } else {
        meerkat_core::SessionControlCommitReceipt::new(
            target_authority.session_id().clone(),
            target_authority.store_revision(),
            target_authority.blob_sha256(),
        )
        .map(Some)
        .map_err(CommitError::SnapshotOutcomeUnknown)
    }
}

/// HeadCanonical durable sticky-fallback commit.
///
/// The control delta mutates only head-owned control metadata (LLM identity
/// and typed tool visibility), so the durable form is an ordinary receiptless
/// snapshot boundary with an empty message suffix: an O(delta) head-metadata
/// CAS parented on the exact committed boundary head, never an O(document)
/// materialized rewrite.
async fn run_sticky_model_fallback_head_canonical_commit(
    session_id: SessionId,
    store: Arc<dyn crate::store::RuntimeStore>,
    machine_commit: Box<dyn meerkat_core::handles::StickyModelFallbackMachineCommit>,
    control_delta: meerkat_core::handles::StickyModelFallbackControlDelta,
) -> Result<
    Option<meerkat_core::SessionControlCommitReceipt>,
    meerkat_core::handles::StickyModelFallbackCommitError,
> {
    use meerkat_core::handles::StickyModelFallbackCommitError as CommitError;

    let runtime_id = crate::identifiers::LogicalRuntimeId::for_session(&session_id);
    let source = store
        .load_durable_tail_recovery_source(&runtime_id)
        .await
        .map_err(|error| CommitError::Store(error.to_string()))?
        .ok_or_else(|| CommitError::SnapshotMissing {
            session_id: session_id.clone(),
        })?;
    if source.committed_session().id() != &session_id {
        return Err(CommitError::SessionMismatch {
            expected: session_id,
            actual: source.committed_session().id().clone(),
        });
    }
    let previous_authority = source.runtime_authority().clone();
    let committed_authority = previous_authority.head_canonical().ok_or_else(|| {
        CommitError::Store(format!(
            "HeadCanonical runtime store returned a non-HeadCanonical session authority for {session_id}"
        ))
    })?;
    // Mirror the WholeBlob refusal: a control-only CAS parented on the
    // committed boundary must not bypass a store-owned in-run provisional
    // physical tail.
    if source.provisional_authority().is_some()
        || source.physical_head() != committed_authority.boundary_head()
    {
        return Err(CommitError::Store(
            "durable sticky fallback cannot bypass a store-owned HeadCanonical \
             provisional physical tail"
                .to_string(),
        ));
    }
    let boundary_head = committed_authority.boundary_head().clone();
    let mut target_session = source.committed_session().as_ref().clone();
    control_delta
        .validate_and_apply(&mut target_session)
        .map_err(CommitError::InvalidControlDelta)?;
    let target_mutation = meerkat_core::session_store::PreparedHeadCanonicalMutation::prepare(
        &target_session,
        Some(boundary_head),
    )
    .map_err(|error| CommitError::SnapshotInvalid(error.to_string()))?;
    let target_head_token = target_mutation.successor_head_token().to_string();
    let target_boundary =
        meerkat_core::lifecycle::core_executor::BoundSessionCommit::head_canonical_from_session(
            &target_session,
            target_mutation.clone(),
        )
        .map_err(|error| CommitError::SnapshotInvalid(error.to_string()))?;

    let target_authority = commit_head_canonical_control_snapshot(
        store.as_ref(),
        &runtime_id,
        target_boundary,
        &target_head_token,
        Some(&previous_authority),
    )
    .await?;

    if let Err(machine_error) = machine_commit.commit() {
        let compensation_failed =
            |detail: String| CommitError::CompensationFailed(format!("{machine_error}; {detail}"));
        let mut rollback_session = target_session;
        target_mutation
            .acknowledge_session(&mut rollback_session, &target_head_token)
            .map_err(|error| {
                compensation_failed(format!(
                    "failed to adopt the durable target before compensation: {error}"
                ))
            })?;
        control_delta
            .inverted()
            .validate_and_apply(&mut rollback_session)
            .map_err(|error| {
                compensation_failed(format!(
                    "failed to derive the durable predecessor control state: {error}"
                ))
            })?;
        let rollback_mutation =
            meerkat_core::session_store::PreparedHeadCanonicalMutation::prepare(
                &rollback_session,
                Some(target_mutation.successor_head().clone()),
            )
            .map_err(|error| {
                compensation_failed(format!(
                    "failed to prepare the durable predecessor: {error}"
                ))
            })?;
        let rollback_head_token = rollback_mutation.successor_head_token().to_string();
        let rollback_boundary =
            meerkat_core::lifecycle::core_executor::BoundSessionCommit::head_canonical_from_session(
                &rollback_session,
                rollback_mutation,
            )
            .map_err(|error| {
                compensation_failed(format!("failed to seal the durable predecessor: {error}"))
            })?;
        match commit_head_canonical_control_snapshot(
            store.as_ref(),
            &runtime_id,
            rollback_boundary,
            &rollback_head_token,
            Some(&target_authority),
        )
        .await
        {
            Ok(_) => Err(CommitError::MachineRejected(machine_error)),
            Err(rollback_error) => Err(compensation_failed(rollback_error.to_string())),
        }
    } else {
        let committed = target_authority.head_canonical().ok_or_else(|| {
            CommitError::SnapshotOutcomeUnknown(
                "HeadCanonical control commit acknowledged a non-HeadCanonical authority"
                    .to_string(),
            )
        })?;
        meerkat_core::SessionControlCommitReceipt::new(
            committed.session_id().clone(),
            committed.store_revision(),
            committed.committed_head_token(),
        )
        .map(Some)
        .map_err(CommitError::SnapshotOutcomeUnknown)
    }
}

/// Commit one receiptless HeadCanonical control snapshot and reconcile a
/// store-reported failure against the bounded committed authority, using the
/// same vocabulary as the WholeBlob CAS: converged-to-target is success, an
/// unchanged expected predecessor is an ordinary [`CommitError::Store`]
/// refusal, anything else is an unknown outcome.
async fn commit_head_canonical_control_snapshot(
    store: &dyn crate::store::RuntimeStore,
    runtime_id: &crate::identifiers::LogicalRuntimeId,
    boundary: meerkat_core::lifecycle::core_executor::BoundSessionCommit,
    successor_head_token: &str,
    expected_previous: Option<&crate::store::RuntimeSessionAuthority>,
) -> Result<
    crate::store::RuntimeSessionAuthority,
    meerkat_core::handles::StickyModelFallbackCommitError,
> {
    use meerkat_core::handles::StickyModelFallbackCommitError as CommitError;

    let request = crate::store::PreparedRuntimeSessionCommit::snapshot_only(boundary);
    let committed_token_matches = |authority: &crate::store::RuntimeSessionAuthority| {
        authority
            .head_canonical()
            .is_some_and(|committed| committed.committed_head_token() == successor_head_token)
    };
    match store
        .commit_prepared_session_boundary(runtime_id, request)
        .await
    {
        Ok(result) => {
            let authority = result.authority().cloned().ok_or_else(|| {
                CommitError::SnapshotOutcomeUnknown(
                    "HeadCanonical control snapshot commit returned no session authority"
                        .to_string(),
                )
            })?;
            if !committed_token_matches(&authority) {
                return Err(CommitError::SnapshotOutcomeUnknown(format!(
                    "HeadCanonical control snapshot acknowledged unexpected authority revision {}",
                    authority.store_revision(),
                )));
            }
            Ok(authority)
        }
        Err(cas_error) => {
            let observed = store
                .load_session_boundary_authority(runtime_id)
                .await
                .map_err(|read_error| {
                    CommitError::SnapshotOutcomeUnknown(format!(
                        "control snapshot commit failed with '{cas_error}' and bounded \
                         authority reconciliation failed with '{read_error}'"
                    ))
                })?;
            match observed {
                Some(observed) if committed_token_matches(&observed) => Ok(observed),
                Some(observed) if Some(&observed) == expected_previous => {
                    Err(CommitError::Store(cas_error.to_string()))
                }
                _ => Err(CommitError::SnapshotOutcomeUnknown(cas_error.to_string())),
            }
        }
    }
}

impl RuntimeCompactionCommitCoordinator {
    const AUTHORIZE_CONTEXT: &'static str =
        "RuntimeCompactionCommitCoordinator::authorize_projection";

    /// Sample the session's live binding, re-deriving the epoch at most once.
    ///
    /// A rotated epoch under a live session is the benign race
    /// (`RegisterSessionResumesStopped` re-registers the entry): the DSL
    /// authority is the epoch's owner, so the coordinator re-reads it and
    /// re-validates against the fresh value. The bound is exactly one attempt
    /// per authorization, and the re-read is a second acquisition of the
    /// authority lock, so a rotation racing the retry refuses instead of
    /// looping. Safety does not rest on this check: the placement latch below
    /// still fences a re-placed runtime, and the transcript
    /// revision/fingerprint CAS at the runtime's atomic apply is what makes a
    /// commit against the wrong history impossible. This covers epoch-only
    /// changes; a placement rotation is never adopted.
    fn sample_live_binding(
        &self,
    ) -> Result<
        (
            crate::meerkat_machine::dsl::AgentRuntimeId,
            Option<crate::meerkat_machine::dsl::FenceToken>,
            Option<crate::meerkat_machine::dsl::Generation>,
        ),
        meerkat_core::memory::CompactionCommitCoordinationError,
    > {
        let dsl_session_id = crate::meerkat_machine::dsl::SessionId::from_domain(&self.session_id);
        let mut epoch = self
            .runtime_epoch_id
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let refusal = match self.dsl_authority.current_runtime_binding(
            &dsl_session_id,
            &epoch,
            Self::AUTHORIZE_CONTEXT,
        ) {
            Ok(binding) => return Ok(binding),
            Err(refusal) => refusal,
        };
        let crate::handles::RuntimeBindingSampleRefusal::EpochChanged {
            current: Some(live_epoch),
            ..
        } = &refusal
        else {
            return Err(Self::refusal_error(&refusal));
        };
        let live_epoch = live_epoch.clone();
        let binding = self
            .dsl_authority
            .current_runtime_binding(&dsl_session_id, &live_epoch, Self::AUTHORIZE_CONTEXT)
            .map_err(|retry_refusal| {
                tracing::warn!(
                    initial = %refusal,
                    retry = %retry_refusal,
                    "bounded compaction epoch re-derivation did not recover the runtime binding"
                );
                Self::refusal_error(&retry_refusal)
            })?;
        tracing::warn!(
            initial = %refusal,
            "adopted the session's live runtime epoch for compaction commit authorization"
        );
        *epoch = live_epoch;
        Ok(binding)
    }

    fn refusal_error(
        refusal: &crate::handles::RuntimeBindingSampleRefusal,
    ) -> meerkat_core::memory::CompactionCommitCoordinationError {
        use crate::handles::RuntimeBindingSampleRefusal as Refusal;
        use meerkat_core::memory::CompactionHandoffRefusal as Cause;
        let cause = match refusal {
            // A gate refusal and a session rebind both mean this
            // session/epoch pair is no longer the authority's live pair.
            Refusal::AuthorityUnavailable(_) | Refusal::SessionChanged { .. } => {
                Cause::RuntimeEpochRetired
            }
            Refusal::EpochChanged {
                current: Some(_), ..
            } => Cause::RuntimeEpochRotated,
            // No live epoch to rotate to: the entry is unregistered or its
            // epoch was cleared.
            Refusal::EpochChanged { current: None, .. } => Cause::RuntimeEpochRetired,
            Refusal::PlacementAbsent => Cause::RuntimeBindingAbsent,
        };
        meerkat_core::memory::CompactionCommitCoordinationError::refused(cause, refusal.to_string())
    }
}

impl meerkat_core::memory::CompactionCommitCoordinator for RuntimeCompactionCommitCoordinator {
    fn authorize_projection(
        &self,
        projection: &meerkat_core::memory::CompactionProjectionId,
    ) -> Result<(), meerkat_core::memory::CompactionCommitCoordinationError> {
        if projection.session_id() != &self.session_id {
            return Err(
                meerkat_core::memory::CompactionCommitCoordinationError::SessionMismatch {
                    expected: self.session_id.clone(),
                    actual: projection.session_id().clone(),
                },
            );
        }
        let store = self.store.as_ref().ok_or_else(|| {
            meerkat_core::memory::CompactionCommitCoordinationError::refused(
                meerkat_core::memory::CompactionHandoffRefusal::DurableProjectionUnsupported,
                "runtime binding has no durable RuntimeStore",
            )
        })?;
        if !store.supports_compaction_projection_outbox() {
            return Err(
                meerkat_core::memory::CompactionCommitCoordinationError::refused(
                    meerkat_core::memory::CompactionHandoffRefusal::DurableProjectionUnsupported,
                    "runtime store does not support atomic compaction projection outbox",
                ),
            );
        }
        let current = self.sample_live_binding()?;
        let mut expected = self
            .runtime_binding
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match expected.as_ref() {
            Some(expected) if expected == &current => {}
            Some(expected) => {
                return Err(
                    meerkat_core::memory::CompactionCommitCoordinationError::refused(
                        meerkat_core::memory::CompactionHandoffRefusal::RuntimeBindingRotated,
                        format!(
                            "runtime binding rotated (expected {expected:?}, current {current:?})"
                        ),
                    ),
                );
            }
            None if self.allow_late_binding => {
                // Mob/local-resource construction precedes the routed
                // RequestRuntimeBinding transition. The first resultful
                // authorization latches the exact generated binding; later
                // fence/generation rotation then fails closed.
                *expected = Some(current);
            }
            None => {
                return Err(
                    meerkat_core::memory::CompactionCommitCoordinationError::refused(
                        meerkat_core::memory::CompactionHandoffRefusal::RuntimeBindingAbsent,
                        "session resources do not carry an authoritative runtime binding",
                    ),
                );
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod compaction_coordinator_tests {
    use super::*;
    use meerkat_core::memory::CompactionCommitCoordinator;

    use crate::meerkat_machine::dsl;
    use meerkat_core::memory::CompactionHandoffRefusal;

    fn projection(session_id: &SessionId) -> meerkat_core::CompactionProjectionId {
        serde_json::from_value(serde_json::json!({
            "session_id": session_id,
            "parent_revision": "parent",
            "revision": "revision",
            "commit_fingerprint": "sha256:coordinator-persisted-fixture",
        }))
        .expect("persisted compaction projection fixture")
    }

    struct CoordinatorFixture {
        session_id: SessionId,
        dsl_session_id: dsl::SessionId,
        dsl_epoch_id: dsl::RuntimeEpochId,
        authority: Arc<std::sync::Mutex<dsl::MeerkatMachineAuthority>>,
        teardown_gate: Arc<crate::handles::HandleTeardownGate>,
        handle: Arc<crate::handles::HandleDslAuthority>,
    }

    impl CoordinatorFixture {
        fn new() -> Self {
            let session_id = SessionId::new();
            let dsl_session_id = dsl::SessionId::from_domain(&session_id);
            let dsl_epoch_id =
                dsl::RuntimeEpochId::from_domain(&meerkat_core::RuntimeEpochId::new());
            let authority = Arc::new(std::sync::Mutex::new(dsl::MeerkatMachineAuthority::new()));
            let teardown_gate = crate::handles::HandleTeardownGate::open();
            let handle = Arc::new(
                crate::handles::HandleDslAuthority::from_shared_with_teardown_gate(
                    Arc::clone(&authority),
                    Arc::clone(&teardown_gate),
                ),
            );
            Self {
                session_id,
                dsl_session_id,
                dsl_epoch_id,
                authority,
                teardown_gate,
                handle,
            }
        }

        fn coordinator(&self, allow_late_binding: bool) -> RuntimeCompactionCommitCoordinator {
            RuntimeCompactionCommitCoordinator {
                session_id: self.session_id.clone(),
                runtime_binding: std::sync::Mutex::new(None),
                runtime_epoch_id: std::sync::Mutex::new(self.dsl_epoch_id.clone()),
                allow_late_binding,
                dsl_authority: Arc::clone(&self.handle),
                store: Some(Arc::new(crate::store::memory::InMemoryRuntimeStore::new())),
            }
        }

        fn register(&self) {
            self.handle
                .apply_signal(
                    dsl::MeerkatMachineSignal::Initialize,
                    "compaction_coordinator_test::initialize",
                )
                .expect("initialize machine");
            self.handle
                .apply_input(
                    dsl::MeerkatMachineInput::RegisterSession {
                        session_id: self.dsl_session_id.clone(),
                        runtime_epoch_id: Some(self.dsl_epoch_id.clone()),
                    },
                    "compaction_coordinator_test::register",
                )
                .expect("register session");
        }

        fn bind(&self, fence_token: u64) {
            self.handle
                .apply_input(
                    dsl::MeerkatMachineInput::PrepareBindings {
                        agent_runtime_id: dsl::AgentRuntimeId::from("mob-runtime"),
                        fence_token: dsl::FenceToken::from(fence_token),
                        generation: Some(dsl::Generation::from(3)),
                        runtime_epoch_id: Some(self.dsl_epoch_id.clone()),
                        session_id: self.dsl_session_id.clone(),
                    },
                    "compaction_coordinator_test::bind",
                )
                .expect("mob binding");
        }

        fn register_and_bind(&self, fence_token: u64) {
            self.register();
            self.bind(fence_token);
        }

        /// Rewrite live DSL state directly. Production reaches these states
        /// through registration/placement transitions across process lifetimes;
        /// a unit test cannot replay a competing registrar, so it recovers the
        /// authority from the exact state that registrar would have left.
        fn rewrite_state(&self, mutate: impl FnOnce(&mut dsl::MeerkatMachineState)) {
            let mut guard = self
                .authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut state = guard.state().clone();
            mutate(&mut state);
            *guard = dsl::MeerkatMachineAuthority::recover_from_state(state)
                .expect("rewritten authority state must satisfy the machine invariants");
        }
    }

    fn refusal_of(
        error: &meerkat_core::memory::CompactionCommitCoordinationError,
    ) -> CompactionHandoffRefusal {
        error.refusal()
    }

    #[test]
    fn local_binding_rejects_before_mob_bind_accepts_after_and_rejects_stale_epoch() {
        let fixture = CoordinatorFixture::new();
        let session_id = fixture.session_id.clone();
        let teardown_gate = Arc::clone(&fixture.teardown_gate);
        let coordinator = fixture.coordinator(true);
        let projection = projection(&session_id);

        assert!(coordinator.authorize_projection(&projection).is_err());
        fixture.register_and_bind(7);
        coordinator
            .authorize_projection(&projection)
            .expect("late generated mob binding must be accepted and latched");

        fixture.rewrite_state(|state| {
            state.active_fence_token = Some(dsl::FenceToken::from(8));
        });
        let rotated = coordinator.authorize_projection(&projection).expect_err(
            "a same-gate fence rotation must not be accepted by the latched coordinator",
        );
        assert_eq!(
            refusal_of(&rotated),
            CompactionHandoffRefusal::RuntimeBindingRotated
        );

        // Restore the originally latched facts to distinguish epoch teardown
        // rejection from the binding-rotation assertion above.
        fixture.rewrite_state(|state| {
            state.active_fence_token = Some(dsl::FenceToken::from(7));
        });
        coordinator
            .authorize_projection(&projection)
            .expect("restored exact binding must match the latch");

        teardown_gate.close();
        let torn_down = coordinator
            .authorize_projection(&projection)
            .expect_err("a coordinator from the torn-down epoch must fail closed");
        assert_eq!(
            refusal_of(&torn_down),
            CompactionHandoffRefusal::RuntimeEpochRetired
        );
    }

    /// A rotated epoch under a live session is the benign race the design
    /// names: registration re-mints the entry epoch while a running agent still
    /// holds a coordinator from the previous one. One bounded re-derivation
    /// adopts the live value, so the member's durable compaction persists
    /// instead of being refused forever.
    #[test]
    fn epoch_only_rotation_is_recovered_by_one_bounded_rederivation() {
        let fixture = CoordinatorFixture::new();
        let coordinator = fixture.coordinator(true);
        let projection = projection(&fixture.session_id);
        fixture.register_and_bind(7);
        coordinator
            .authorize_projection(&projection)
            .expect("the initial binding latches");

        let rotated_epoch = dsl::RuntimeEpochId::from_domain(&meerkat_core::RuntimeEpochId::new());
        fixture.rewrite_state(|state| {
            state.active_runtime_epoch_id = Some(rotated_epoch.clone());
        });

        coordinator.authorize_projection(&projection).expect(
            "an epoch-only rotation must be recovered by the single bounded re-derivation, not \
             wedge durable compaction",
        );
        assert_eq!(
            *coordinator
                .runtime_epoch_id
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            rotated_epoch,
            "the re-derived epoch must be adopted so later authorizations start from live truth"
        );
        coordinator
            .authorize_projection(&projection)
            .expect("the adopted epoch must authorize without re-deriving again");
    }

    /// The re-derivation is recovery for the epoch alone. A placement rotation
    /// riding along still fails closed on the latch, and a second rotation
    /// racing the retry has no second attempt to catch it.
    #[test]
    fn rederivation_never_adopts_a_rotated_placement() {
        let fixture = CoordinatorFixture::new();
        let coordinator = fixture.coordinator(true);
        let projection = projection(&fixture.session_id);
        fixture.register_and_bind(7);
        coordinator
            .authorize_projection(&projection)
            .expect("the initial binding latches");

        fixture.rewrite_state(|state| {
            state.active_runtime_epoch_id = Some(dsl::RuntimeEpochId::from_domain(
                &meerkat_core::RuntimeEpochId::new(),
            ));
            state.active_fence_token = Some(dsl::FenceToken::from(9));
        });

        let error = coordinator
            .authorize_projection(&projection)
            .expect_err("a re-placed runtime must not be authorized by epoch re-derivation");
        assert_eq!(
            refusal_of(&error),
            CompactionHandoffRefusal::RuntimeBindingRotated
        );
    }

    /// A cleared epoch is not a rotation: there is no live value to adopt, so
    /// the coordinator refuses with the retired cause rather than re-deriving
    /// its way into an epochless session (the exact state the 0.8.23 wedge
    /// left behind).
    #[test]
    fn cleared_epoch_refuses_as_retired_without_rederivation() {
        let fixture = CoordinatorFixture::new();
        let coordinator = fixture.coordinator(true);
        let projection = projection(&fixture.session_id);
        fixture.register_and_bind(7);
        coordinator
            .authorize_projection(&projection)
            .expect("the initial binding latches");

        fixture.rewrite_state(|state| {
            state.active_runtime_epoch_id = None;
        });

        let error = coordinator
            .authorize_projection(&projection)
            .expect_err("an epochless session cannot authorize a durable handoff");
        assert_eq!(
            refusal_of(&error),
            CompactionHandoffRefusal::RuntimeEpochRetired
        );
        assert_eq!(
            *coordinator
                .runtime_epoch_id
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            fixture.dsl_epoch_id,
            "a refused re-derivation must not move the coordinator's epoch"
        );
    }

    /// The remaining refusal causes a host routes on.
    #[test]
    fn refusal_causes_are_typed_per_structural_reason() {
        let fixture = CoordinatorFixture::new();
        let projection = projection(&fixture.session_id);

        let mut storeless = fixture.coordinator(true);
        storeless.store = None;
        let error = storeless
            .authorize_projection(&projection)
            .expect_err("a runtime with no durable store cannot commit a durable pair");
        assert_eq!(
            refusal_of(&error),
            CompactionHandoffRefusal::DurableProjectionUnsupported
        );

        let foreign = fixture.coordinator(true);
        let error = foreign
            .authorize_projection(&projection_for_other_session())
            .expect_err("a foreign projection is refused");
        assert_eq!(
            refusal_of(&error),
            CompactionHandoffRefusal::SessionMismatch
        );

        // Registered and epoch-bearing but not yet placed: the window the
        // registration-owned epoch created.
        fixture.register();
        let unplaced = fixture.coordinator(true);
        let error = unplaced
            .authorize_projection(&projection)
            .expect_err("a registered-unplaced session has no placement to commit against");
        assert_eq!(
            refusal_of(&error),
            CompactionHandoffRefusal::RuntimeBindingAbsent
        );

        fixture.bind(7);
        let strict = fixture.coordinator(false);
        let error = strict
            .authorize_projection(&projection)
            .expect_err("a canonical coordinator refuses an unlatched binding");
        assert_eq!(
            refusal_of(&error),
            CompactionHandoffRefusal::RuntimeBindingAbsent
        );
    }

    fn projection_for_other_session() -> meerkat_core::CompactionProjectionId {
        projection(&SessionId::new())
    }
}

#[cfg(all(test, feature = "sqlite-store"))]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod sticky_model_fallback_commit_tests {
    use super::*;
    use crate::store::{RuntimeSessionAuthority, RuntimeStore, SerializedSessionSnapshot};
    use meerkat_core::handles::{
        StickyModelFallbackCommitCoordinator as _, StickyModelFallbackCommitError,
        StickyModelFallbackControlDelta, StickyModelFallbackControlDeltaError,
    };
    use meerkat_core::lifecycle::core_executor::BoundSessionCommit;
    use meerkat_core::session_store::PreparedHeadCanonicalMutation;
    use meerkat_core::{
        Message, Provider, SESSION_METADATA_SCHEMA_VERSION, Session, SessionLlmIdentity,
        SessionMetadata, SessionToolVisibilityState, SessionTooling, ToolFilter, UserMessage,
        VIEW_IMAGE_TOOL_NAME,
    };
    use tempfile::TempDir;

    struct AcceptingMachineCommit;

    impl meerkat_core::handles::StickyModelFallbackMachineCommit for AcceptingMachineCommit {
        fn commit(self: Box<Self>) -> Result<(), meerkat_core::handles::DslTransitionError> {
            Ok(())
        }
    }

    struct RejectingMachineCommit;

    impl meerkat_core::handles::StickyModelFallbackMachineCommit for RejectingMachineCommit {
        fn commit(self: Box<Self>) -> Result<(), meerkat_core::handles::DslTransitionError> {
            Err(meerkat_core::handles::DslTransitionError::no_matching(
                "sticky_model_fallback_commit_tests",
                "synthetic generated-authority rejection",
            ))
        }
    }

    fn identity(model: &str) -> SessionLlmIdentity {
        SessionLlmIdentity {
            model: model.to_string(),
            provider: Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        }
    }

    fn target_visibility() -> SessionToolVisibilityState {
        SessionToolVisibilityState {
            capability_base_filter: ToolFilter::Deny(
                [VIEW_IMAGE_TOOL_NAME.to_string()].into_iter().collect(),
            ),
            active_revision: 1,
            staged_revision: 1,
            ..Default::default()
        }
    }

    fn control_delta() -> StickyModelFallbackControlDelta {
        StickyModelFallbackControlDelta::from_control_states_for_test(
            identity("primary"),
            identity("backup"),
            SessionToolVisibilityState::default(),
            target_visibility(),
        )
    }

    fn session_with_control_state(model: &str) -> Session {
        let mut session = Session::new();
        session
            .set_session_metadata(SessionMetadata {
                schema_version: SESSION_METADATA_SCHEMA_VERSION,
                model: model.to_string(),
                max_tokens: 4096,
                structured_output_retries: 2,
                provider: Provider::OpenAI,
                self_hosted_server_id: None,
                provider_params: None,
                tooling: SessionTooling::default(),
                keep_alive: true,
                comms_name: None,
                peer_meta: None,
                realm_id: None,
                instance_id: None,
                backend: None,
                config_generation: Some(7),
                auth_binding: None,
                mob_member_binding: None,
            })
            .expect("control-state session metadata");
        session.push(Message::User(UserMessage::text("committed user turn")));
        // A persisted session carries its generated-authority visibility
        // projection as ordinary durable metadata. Model that exact persisted
        // representation; the sealed live constructor is core-private.
        let mut value = serde_json::to_value(&session).expect("serialize control-state session");
        value["metadata"][meerkat_core::SESSION_TOOL_VISIBILITY_STATE_KEY] =
            serde_json::to_value(SessionToolVisibilityState::default())
                .expect("serialize visibility state");
        serde_json::from_value(value).expect("deserialize control-state session")
    }

    fn coordinator_for(
        session_id: &SessionId,
        store: Arc<dyn RuntimeStore>,
    ) -> RuntimeStickyModelFallbackCommitCoordinator {
        RuntimeStickyModelFallbackCommitCoordinator {
            session_id: session_id.clone(),
            store: Some(store),
        }
    }

    async fn commit_head_canonical_root(
        store: &dyn RuntimeStore,
        runtime_id: &crate::identifiers::LogicalRuntimeId,
        session: &Session,
    ) -> crate::store::HeadCanonicalStoreAuthority {
        let mutation =
            PreparedHeadCanonicalMutation::prepare(session, None).expect("root mutation");
        let boundary = BoundSessionCommit::head_canonical_from_session(session, mutation)
            .expect("root boundary");
        store
            .commit_prepared_session_boundary(
                runtime_id,
                crate::store::PreparedRuntimeSessionCommit::snapshot_only(boundary),
            )
            .await
            .expect("root head-canonical commit")
            .authority()
            .and_then(RuntimeSessionAuthority::head_canonical)
            .expect("root HeadCanonical authority")
            .clone()
    }

    #[tokio::test]
    async fn head_canonical_sticky_fallback_commit_persists_control_metadata() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("sticky-head-canonical.sqlite3");
        let store: Arc<dyn RuntimeStore> =
            Arc::new(crate::store::SqliteRuntimeStore::new_head_canonical(&path).expect("store"));
        let session = session_with_control_state("primary");
        let session_id = session.id().clone();
        let runtime_id = crate::identifiers::LogicalRuntimeId::for_session(&session_id);
        let root = commit_head_canonical_root(store.as_ref(), &runtime_id, &session).await;

        let coordinator = coordinator_for(&session_id, Arc::clone(&store));
        let operation = coordinator
            .begin(Box::new(AcceptingMachineCommit), control_delta())
            .expect("begin durable sticky fallback");
        let receipt = operation
            .wait()
            .await
            .expect("durable sticky fallback commit")
            .expect("durable control commit receipt");

        assert_eq!(receipt.session_id(), &session_id);
        assert_eq!(receipt.store_revision(), root.store_revision() + 1);
        assert_ne!(receipt.authority_token(), root.committed_head_token());

        let source = store
            .load_durable_tail_recovery_source(&runtime_id)
            .await
            .expect("reload committed source")
            .expect("committed source exists");
        let committed_authority = source
            .runtime_authority()
            .head_canonical()
            .expect("HeadCanonical committed authority")
            .clone();
        assert_eq!(
            committed_authority.committed_head_token(),
            receipt.authority_token()
        );
        let committed = source.committed_session();
        assert_eq!(
            committed
                .session_metadata()
                .expect("committed metadata")
                .llm_identity(),
            identity("backup"),
            "durable head metadata must carry the fallback LLM identity"
        );
        assert_eq!(
            committed
                .try_tool_visibility_state()
                .expect("committed visibility decodes"),
            Some(target_visibility()),
            "durable head metadata must carry the fallback visibility state"
        );
        assert_eq!(
            committed.messages().len(),
            1,
            "the control commit must not touch transcript rows"
        );
    }

    #[tokio::test]
    async fn head_canonical_sticky_fallback_rejects_a_stale_control_parent() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("sticky-head-canonical-stale.sqlite3");
        let store: Arc<dyn RuntimeStore> =
            Arc::new(crate::store::SqliteRuntimeStore::new_head_canonical(&path).expect("store"));
        let session = session_with_control_state("primary");
        let session_id = session.id().clone();
        let runtime_id = crate::identifiers::LogicalRuntimeId::for_session(&session_id);
        commit_head_canonical_root(store.as_ref(), &runtime_id, &session).await;

        let coordinator = coordinator_for(&session_id, Arc::clone(&store));
        coordinator
            .begin(Box::new(AcceptingMachineCommit), control_delta())
            .expect("begin first durable sticky fallback")
            .wait()
            .await
            .expect("first durable sticky fallback commit");

        // The same delta is now parented on superseded control state.
        let stale = coordinator
            .begin(Box::new(AcceptingMachineCommit), control_delta())
            .expect("begin stale durable sticky fallback")
            .wait()
            .await
            .expect_err("a stale control parent must be refused");
        assert!(
            matches!(
                stale,
                StickyModelFallbackCommitError::InvalidControlDelta(
                    StickyModelFallbackControlDeltaError::IdentityParentMismatch { .. }
                )
            ),
            "unexpected stale-parent refusal: {stale:?}"
        );
    }

    #[tokio::test]
    async fn head_canonical_machine_rejection_compensates_the_durable_target() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("sticky-head-canonical-rollback.sqlite3");
        let store: Arc<dyn RuntimeStore> =
            Arc::new(crate::store::SqliteRuntimeStore::new_head_canonical(&path).expect("store"));
        let session = session_with_control_state("primary");
        let session_id = session.id().clone();
        let runtime_id = crate::identifiers::LogicalRuntimeId::for_session(&session_id);
        let root = commit_head_canonical_root(store.as_ref(), &runtime_id, &session).await;

        let coordinator = coordinator_for(&session_id, Arc::clone(&store));
        let error = coordinator
            .begin(Box::new(RejectingMachineCommit), control_delta())
            .expect("begin durable sticky fallback")
            .wait()
            .await
            .expect_err("generated-authority rejection must not commit the fallback");
        assert!(
            matches!(error, StickyModelFallbackCommitError::MachineRejected(_)),
            "unexpected machine-rejection result: {error:?}"
        );

        let source = store
            .load_durable_tail_recovery_source(&runtime_id)
            .await
            .expect("reload committed source")
            .expect("committed source exists");
        let committed_authority = source
            .runtime_authority()
            .head_canonical()
            .expect("HeadCanonical committed authority")
            .clone();
        assert_eq!(
            committed_authority.store_revision(),
            root.store_revision() + 2,
            "compensation commits a durable predecessor successor, not an in-place erase"
        );
        let committed = source.committed_session();
        assert_eq!(
            committed
                .session_metadata()
                .expect("committed metadata")
                .llm_identity(),
            identity("primary"),
            "compensation must restore the durable predecessor LLM identity"
        );
        assert_eq!(
            committed
                .try_tool_visibility_state()
                .expect("committed visibility decodes"),
            Some(SessionToolVisibilityState::default()),
            "compensation must restore the durable predecessor visibility state"
        );
    }

    #[tokio::test]
    async fn whole_blob_sticky_fallback_commit_path_is_unchanged() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("sticky-whole-blob.sqlite3");
        let store: Arc<dyn RuntimeStore> =
            Arc::new(crate::store::SqliteRuntimeStore::new_whole_blob(&path).expect("store"));
        let session = session_with_control_state("primary");
        let session_id = session.id().clone();
        let runtime_id = crate::identifiers::LogicalRuntimeId::for_session(&session_id);
        store
            .commit_session_snapshot(
                &runtime_id,
                SerializedSessionSnapshot {
                    session_snapshot: serde_json::to_vec(&session)
                        .expect("serialize session")
                        .into(),
                },
            )
            .await
            .expect("root whole-blob snapshot");

        let coordinator = coordinator_for(&session_id, Arc::clone(&store));
        let receipt = coordinator
            .begin(Box::new(AcceptingMachineCommit), control_delta())
            .expect("begin durable sticky fallback")
            .wait()
            .await
            .expect("durable sticky fallback commit")
            .expect("durable control commit receipt");
        assert_eq!(receipt.session_id(), &session_id);

        let committed = store
            .load_committed_whole_blob_snapshot(&runtime_id)
            .await
            .expect("reload committed snapshot")
            .expect("committed snapshot exists");
        assert_eq!(
            committed
                .session()
                .session_metadata()
                .expect("committed metadata")
                .llm_identity(),
            identity("backup"),
            "the WholeBlob control commit must keep persisting the fallback identity"
        );
    }
}

fn release_failed_materialization_claim(
    claim_state: &Arc<std::sync::Mutex<crate::RuntimeActorMaterializationClaimState>>,
    claim_id: Option<uuid::Uuid>,
) {
    let Some(claim_id) = claim_id else {
        return;
    };
    let changed = {
        let mut state = claim_state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.current != Some(claim_id) {
            return;
        }
        state.phase = crate::RuntimeActorMaterializationClaimPhase::Aborting;
        Arc::clone(&state.changed)
    };
    changed.notify_waiters();
}

fn visibility_authorities_for_names(
    names: &std::collections::BTreeSet<ToolName>,
    witnesses: &std::collections::BTreeMap<ToolName, meerkat_core::ToolVisibilityWitness>,
) -> std::collections::BTreeMap<ToolName, crate::meerkat_machine::dsl::ToolVisibilityWitness> {
    names
        .iter()
        .filter_map(|name| {
            witnesses.get(name).map(|witness| {
                (
                    name.clone(),
                    crate::meerkat_machine::dsl::ToolVisibilityWitness::from(witness),
                )
            })
        })
        .collect()
}

impl MeerkatMachine {
    /// Assemble the candidate canonical handle bundle for an already-validated
    /// session entry without registering, reviving, or otherwise mutating
    /// lifecycle authority. The caller publishes it into the entry exactly
    /// once while holding the session mutation interval.
    #[allow(clippy::too_many_arguments)]
    fn assemble_canonical_session_runtime_bindings(
        &self,
        session_id: SessionId,
        epoch_id: meerkat_core::RuntimeEpochId,
        ops_lifecycle: Arc<crate::ops_lifecycle::RuntimeOpsLifecycleRegistry>,
        cursor_state: Arc<meerkat_core::EpochCursorState>,
        post_commit_hooks: Arc<meerkat_core::PostCommitHookDispatcher>,
        tool_visibility_owner: Arc<MachineToolVisibilityOwner>,
        dsl_authority_shared: Arc<std::sync::Mutex<dsl::MeerkatMachineAuthority>>,
        handle_teardown_gate: Arc<crate::handles::HandleTeardownGate>,
        durability_health: Option<super::DurabilityHealthHandle>,
        compaction_runtime_binding: Option<(
            dsl::AgentRuntimeId,
            Option<dsl::FenceToken>,
            Option<dsl::Generation>,
        )>,
        allow_late_compaction_binding: bool,
        runtime_authority: Arc<dyn std::any::Any + Send + Sync>,
    ) -> Result<meerkat_core::SessionRuntimeBindings, RuntimeDriverError> {
        let compaction_runtime_epoch_id = dsl::RuntimeEpochId::from_domain(&epoch_id);
        let shared_handle_authority = Arc::new(
            crate::handles::HandleDslAuthority::from_shared_with_runtime_gates(
                Arc::clone(&dsl_authority_shared),
                Arc::clone(&handle_teardown_gate),
                durability_health,
            ),
        );
        let peer_comms_install = crate::handles::RuntimePeerCommsHandle::generated_install_factory(
            Arc::clone(&shared_handle_authority),
        )
        .map_err(RuntimeDriverError::Internal)?;
        let generated_visibility_owner = generated_tool_visibility_owner(Arc::clone(
            &tool_visibility_owner,
        )
            as Arc<dyn meerkat_core::ToolVisibilityOwner>)
        .map_err(RuntimeDriverError::Internal)?;

        Ok(
            meerkat_core::SessionRuntimeBindings::__from_runtime_authority(
                session_id.clone(),
                epoch_id,
                ops_lifecycle as Arc<dyn meerkat_core::OpsLifecycleRegistry>,
                cursor_state,
                generated_visibility_owner,
                Arc::new(crate::handles::RuntimeTurnStateHandle::new(Arc::clone(
                    &shared_handle_authority,
                ))),
                Arc::new(crate::handles::RuntimeCommsDrainHandle::new(Arc::clone(
                    &shared_handle_authority,
                ))),
                Arc::new(crate::handles::RuntimeExternalToolSurfaceHandle::new(
                    Arc::clone(&shared_handle_authority),
                )),
                peer_comms_install,
                Arc::new(crate::handles::RuntimeSessionAdmissionHandle::new(
                    Arc::clone(&shared_handle_authority),
                )),
                Arc::new(
                    crate::handles::RuntimeModelRoutingHandle::new_with_visibility_owner(
                        Arc::clone(&shared_handle_authority),
                        Arc::clone(&tool_visibility_owner),
                    ),
                ),
                Arc::new(RuntimeStickyModelFallbackCommitCoordinator {
                    session_id: session_id.clone(),
                    store: self.store.clone(),
                }),
                self.generated_auth_lease_handle(),
                Arc::new(crate::handles::RuntimeMcpServerLifecycleHandle::new(
                    Arc::clone(&shared_handle_authority),
                )),
                Arc::new(crate::handles::RuntimePeerInteractionHandle::new(
                    Arc::clone(&shared_handle_authority),
                )),
                Arc::new(crate::handles::RuntimeSessionContextHandle::new(
                    Arc::clone(&shared_handle_authority),
                )),
                self.session_claim_handle(),
                Arc::new(crate::handles::RuntimeInteractionStreamHandle::new(
                    Arc::clone(&shared_handle_authority),
                )),
                Arc::new(RuntimeCompactionCommitCoordinator {
                    session_id,
                    runtime_binding: std::sync::Mutex::new(compaction_runtime_binding),
                    runtime_epoch_id: std::sync::Mutex::new(compaction_runtime_epoch_id),
                    allow_late_binding: allow_late_compaction_binding,
                    dsl_authority: shared_handle_authority,
                    store: self.store.clone(),
                }),
                post_commit_hooks,
                runtime_authority,
            ),
        )
    }

    #[allow(clippy::too_many_arguments)]
    async fn canonical_session_runtime_bindings(
        &self,
        session_id: SessionId,
        epoch_id: meerkat_core::RuntimeEpochId,
        ops_lifecycle: Arc<crate::ops_lifecycle::RuntimeOpsLifecycleRegistry>,
        cursor_state: Arc<meerkat_core::EpochCursorState>,
        tool_visibility_owner: Arc<MachineToolVisibilityOwner>,
        dsl_authority_shared: Arc<std::sync::Mutex<dsl::MeerkatMachineAuthority>>,
        handle_teardown_gate: Arc<crate::handles::HandleTeardownGate>,
        compaction_runtime_binding: Option<(
            dsl::AgentRuntimeId,
            Option<dsl::FenceToken>,
            Option<dsl::Generation>,
        )>,
        allow_late_compaction_binding: bool,
        runtime_authority: Arc<dyn std::any::Any + Send + Sync>,
    ) -> Result<meerkat_core::SessionRuntimeBindings, RuntimeDriverError> {
        let (cached, durability_health, post_commit_hooks) = {
            let sessions = self.sessions.read().await;
            let entry = sessions
                .get(&session_id)
                .ok_or(RuntimeDriverError::NotReady {
                    state: RuntimeState::Destroyed,
                })?;
            if entry.epoch_id != epoch_id
                || !Arc::ptr_eq(&entry.ops_lifecycle, &ops_lifecycle)
                || !Arc::ptr_eq(&entry.cursor_state, &cursor_state)
                || !Arc::ptr_eq(&entry.tool_visibility_owner, &tool_visibility_owner)
                || !Arc::ptr_eq(&entry.dsl_authority, &dsl_authority_shared)
                || !Arc::ptr_eq(&entry.handle_teardown_gate, &handle_teardown_gate)
            {
                return Err(RuntimeDriverError::StaleAuthority {
                    reason: format!(
                        "runtime binding owner for session {session_id} changed before canonical handle lookup"
                    ),
                });
            }
            (
                entry.canonical_runtime_bindings.clone(),
                entry.durability_health.clone(),
                Arc::clone(&entry.post_commit_hooks),
            )
        };
        if let Some(cached) = cached {
            return Ok(cached.__clone_with_runtime_authority(runtime_authority));
        }

        let candidate = self.assemble_canonical_session_runtime_bindings(
            session_id.clone(),
            epoch_id.clone(),
            Arc::clone(&ops_lifecycle),
            Arc::clone(&cursor_state),
            post_commit_hooks,
            Arc::clone(&tool_visibility_owner),
            Arc::clone(&dsl_authority_shared),
            Arc::clone(&handle_teardown_gate),
            durability_health,
            compaction_runtime_binding,
            allow_late_compaction_binding,
            Arc::clone(&runtime_authority),
        )?;
        let canonical = candidate
            .__clone_with_runtime_authority(Arc::new(()) as Arc<dyn std::any::Any + Send + Sync>);
        let mut sessions = self.sessions.write().await;
        let entry = sessions
            .get_mut(&session_id)
            .ok_or(RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            })?;
        if entry.epoch_id != epoch_id
            || !Arc::ptr_eq(&entry.ops_lifecycle, &ops_lifecycle)
            || !Arc::ptr_eq(&entry.cursor_state, &cursor_state)
            || !Arc::ptr_eq(&entry.tool_visibility_owner, &tool_visibility_owner)
            || !Arc::ptr_eq(&entry.dsl_authority, &dsl_authority_shared)
            || !Arc::ptr_eq(&entry.handle_teardown_gate, &handle_teardown_gate)
        {
            return Err(RuntimeDriverError::StaleAuthority {
                reason: format!(
                    "runtime binding owner for session {session_id} changed before canonical handle publication"
                ),
            });
        }
        if let Some(existing) = &entry.canonical_runtime_bindings {
            return Ok(existing.__clone_with_runtime_authority(runtime_authority));
        }
        entry.canonical_runtime_bindings = Some(canonical);
        Ok(candidate)
    }

    async fn cleanup_failed_materialization_claim(
        &self,
        session_id: &SessionId,
        inserted_by_call: bool,
        epoch_id: &meerkat_core::RuntimeEpochId,
        claim_id: Option<uuid::Uuid>,
        claim_state: &Arc<std::sync::Mutex<crate::RuntimeActorMaterializationClaimState>>,
    ) {
        if let Some(claim_id) = claim_id {
            if let Err(error) = self
                .abort_prepared_session_materialization_claim(
                    session_id,
                    claim_id,
                    Some(epoch_id),
                    Some(claim_state),
                    false,
                )
                .await
            {
                tracing::warn!(
                    %session_id,
                    %error,
                    "failed to clean an exact rejected materialization claim"
                );
            }
        } else if inserted_by_call
            && let Err(error) = self
                .unregister_session_inner_if_epoch(session_id, epoch_id)
                .await
        {
            tracing::warn!(
                %session_id,
                %error,
                "failed to remove a rejected inserted session"
            );
        }
    }

    async fn dispatch_user_interrupt(
        &self,
        session_id: &SessionId,
        expected_run_id: Option<&meerkat_core::RunId>,
        expected_member: Option<
            &meerkat_contracts::wire::supervisor_bridge::BridgeMemberIncarnation,
        >,
        reason: String,
    ) -> Result<bool, RuntimeDriverError> {
        let run_fenced = expected_run_id.is_some();
        let member_lease = match expected_member {
            Some(expected_member) => Some(
                self.acquire_member_effect_authority_lease(session_id, Some(expected_member))
                    .await?,
            ),
            None => None,
        };
        let gate_guard = if let Some(lease) = &member_lease {
            let guard = Arc::clone(&lease.session_mutation_gate).lock_owned().await;
            let sessions = self.sessions.read().await;
            let Some(entry) = sessions.get(session_id) else {
                return Err(RuntimeDriverError::StaleAuthority {
                    reason: "user interrupt runtime session disappeared".to_string(),
                });
            };
            if !Arc::ptr_eq(&entry.mutation_gate, &lease.session_mutation_gate) {
                return Err(RuntimeDriverError::StaleAuthority {
                    reason: "user interrupt runtime session was replaced".to_string(),
                });
            }
            guard
        } else {
            match expected_run_id {
                // The run-fenced bridge path must compare and stage under the SAME
                // current gate. Session absence/replacement means the expected run
                // is already no longer current, which is the level-triggered
                // terminal condition rather than a cancellation error.
                Some(_) => match self.lock_current_session_mutation_gate(session_id).await {
                    Some(guard) => guard,
                    None => return Ok(false),
                },
                None => {
                    let gate = self.session_mutation_gate(session_id).await;
                    match gate {
                        Some(g) => match Arc::clone(&g).try_lock_owned() {
                            Ok(guard) => guard,
                            Err(_) if self.generated_stop_deferred(session_id).await => {
                                return Ok(false);
                            }
                            Err(_) => g.lock_owned().await,
                        },
                        None => return Ok(false),
                    }
                }
            }
        };

        let Ok(authority) = self.session_dsl_authority(session_id).await else {
            return Ok(false);
        };
        let (phase, current_run_id) = {
            let authority = authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            (
                crate::meerkat_machine::dsl_authority::runtime_phase_from_authority(&authority),
                crate::meerkat_machine::dsl_authority::current_run_id_from_authority(&authority),
            )
        };
        if let Some(expected_run_id) = expected_run_id {
            if !matches!(phase, RuntimeState::Running | RuntimeState::Retired)
                || current_run_id.as_ref() != Some(expected_run_id)
            {
                return Ok(false);
            }
        } else {
            let visible_state = self
                .existing_session_runtime_state(session_id)
                .await
                .unwrap_or(phase);
            self.reject_unregistration_drain_ingress(session_id, visible_state)
                .await?;
        }
        let Some(expected_run_id) = current_run_id else {
            return Ok(false);
        };

        let expected_member = expected_member.cloned();
        let (captured_gate, captured_authority, attachment_id, provisional_claim_id, handle) = {
            let sessions = self.sessions.read().await;
            let Some(entry) = sessions.get(session_id) else {
                return Ok(false);
            };
            let Some(handle) = entry.interrupt_handle() else {
                return Err(RuntimeDriverError::NotReady { state: phase });
            };
            (
                Arc::clone(&entry.mutation_gate),
                Arc::clone(&entry.dsl_authority),
                entry.live_attachment_id(),
                entry.provisional_materialization_claim_id,
                handle,
            )
        };

        let joined_result = {
            let sessions = self.sessions.read().await;
            sessions.get(session_id).and_then(|entry| {
                entry
                    .pending_user_interrupt_dispatch
                    .as_ref()
                    .and_then(|pending| {
                        (pending.expected_run_id == expected_run_id
                            && pending.attachment_id == attachment_id
                            && pending.provisional_materialization_claim_id == provisional_claim_id
                            && pending.expected_member == expected_member
                            && Arc::ptr_eq(&pending.interrupt_handle, &handle))
                        .then(|| pending.result_rx.clone())
                    })
            })
        };
        if let Some(result_rx) = joined_result {
            drop(gate_guard);
            drop(member_lease);
            return Self::await_user_interrupt_dispatch(result_rx, &expected_run_id).await;
        }

        let staged_interrupt = if run_fenced {
            Self::stage_dsl_transition_on_authority(
                &captured_authority,
                crate::meerkat_machine::dsl::MeerkatMachineInput::InterruptCurrentRunForRun {
                    run_id: crate::meerkat_machine::dsl::RunId::from_domain(&expected_run_id),
                },
                "InterruptCurrentRunForRun",
            )
        } else {
            self.stage_session_runtime_internal_dsl_transition(
                session_id,
                crate::meerkat_machine_types::MeerkatMachineFieldlessRuntimeInternalInput::InterruptCurrentRun,
            )
            .await
        };
        match staged_interrupt {
            Ok(_) => {}
            Err(_) => {
                // The generated machine rejected `InterruptCurrentRun` for the
                // current phase. Surface the terminal `Destroyed` truth as its
                // own typed variant (DestroyedShapeInvariant) so callers that
                // distinguish a destroyed binding from a merely not-ready one
                // still observe it; every other rejected phase is `NotReady`.
                let state = self
                    .existing_session_runtime_state(session_id)
                    .await
                    .unwrap_or(RuntimeState::Destroyed);
                if state == RuntimeState::Destroyed {
                    return Err(RuntimeDriverError::Destroyed);
                }
                return Err(RuntimeDriverError::NotReady { state });
            }
        }

        // Acquire process-owned execution before publishing the joinable slot.
        // If runtime acquisition fails, no receiver can be left installed with
        // no task capable of publishing or clearing its result.
        let cleanup_spawner = MachineCleanupTaskSpawner::acquire()?;
        let dispatch_id = uuid::Uuid::new_v4();
        let (result_tx, result_rx) = crate::tokio::sync::watch::channel(None);
        {
            let mut sessions = self.sessions.write().await;
            let Some(entry) = sessions.get_mut(session_id) else {
                return Ok(false);
            };
            if !Arc::ptr_eq(&entry.mutation_gate, &captured_gate)
                || !Arc::ptr_eq(&entry.dsl_authority, &captured_authority)
                || entry.live_attachment_id() != attachment_id
                || entry.provisional_materialization_claim_id != provisional_claim_id
                || entry
                    .interrupt_handle()
                    .is_none_or(|current| !Arc::ptr_eq(&current, &handle))
            {
                return Ok(false);
            }
            entry.pending_user_interrupt_dispatch = Some(PendingUserInterruptDispatch {
                dispatch_id,
                expected_run_id: expected_run_id.clone(),
                attachment_id,
                provisional_materialization_claim_id: provisional_claim_id,
                interrupt_handle: Arc::clone(&handle),
                expected_member: expected_member.clone(),
                result_rx: result_rx.clone(),
            });
        }

        let machine = self.clone();
        let owned_session_id = session_id.clone();
        let owned_run_id = expected_run_id.clone();
        drop(gate_guard);
        drop(member_lease);
        cleanup_spawner.spawn(async move {
            let callback_result = match std::panic::AssertUnwindSafe(
                handle.hard_cancel_run_if_current(&owned_run_id, reason),
            )
            .catch_unwind()
            .await
            {
                Ok(Ok(delivered)) => Ok(delivered),
                Ok(Err(error)) => Err(RuntimeDriverError::Internal(format!(
                    "failed to hard cancel exact run {owned_run_id}: {error}"
                ))),
                Err(payload) => Err(RuntimeDriverError::InterruptDispatchPanicked {
                    run_id: owned_run_id.clone(),
                    reason: meerkat_core::panic_payload::panic_payload_detail(payload.as_ref()),
                }),
            };
            let result = machine
                .reconcile_user_interrupt_dispatch(
                    &owned_session_id,
                    dispatch_id,
                    &owned_run_id,
                    &captured_gate,
                    &captured_authority,
                    attachment_id,
                    provisional_claim_id,
                    &handle,
                    &result_tx,
                    expected_member.as_ref(),
                    callback_result,
                )
                .await;
            let _ = result;
        });

        Self::await_user_interrupt_dispatch(result_rx, &expected_run_id).await
    }

    /// Classify a generated-machine rejection of a session lifecycle input.
    ///
    /// The machine already made the legality decision (stage-first shape, same
    /// as `dispatch_user_interrupt`); this only maps the rejection onto the
    /// typed wire error: a `Destroyed` binding surfaces as the terminal
    /// [`RuntimeDriverError::Destroyed`], every other rejection keeps its
    /// reason as `ValidationFailed`. Reading the runtime state here is a
    /// post-verdict projection read for classification, never a guard.
    pub(super) async fn classify_session_dsl_rejection(
        &self,
        session_id: &SessionId,
        reason: String,
    ) -> RuntimeDriverError {
        if matches!(
            self.existing_session_runtime_state(session_id).await,
            Some(RuntimeState::Destroyed)
        ) {
            return RuntimeDriverError::Destroyed;
        }
        RuntimeDriverError::ValidationFailed { reason }
    }

    /// Same stage-first classification for lifecycle errors that already carry
    /// a typed [`RuntimeDriverError`]: a rejection observed on a `Destroyed`
    /// binding is surfaced as the terminal `Destroyed` truth; everything else
    /// propagates unchanged.
    pub(super) async fn classify_session_driver_rejection(
        &self,
        session_id: &SessionId,
        err: RuntimeDriverError,
    ) -> RuntimeDriverError {
        if matches!(
            self.existing_session_runtime_state(session_id).await,
            Some(RuntimeState::Destroyed)
        ) {
            return RuntimeDriverError::Destroyed;
        }
        err
    }

    async fn generated_stop_deferred(&self, session_id: &SessionId) -> bool {
        let Ok(authority) = self.session_dsl_authority(session_id).await else {
            return false;
        };
        authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .state()
            .runtime_stop_deferred
    }

    /// Commit one exact runtime-placement tuple through the canonical
    /// `PrepareBindings` transition, then mechanically synchronize the driver
    /// projection. Callers own the surrounding session mutation gate and any
    /// higher-level cleanup if the binding is rejected.
    pub(super) async fn commit_runtime_placement_binding(
        &self,
        session_id: &SessionId,
        driver_handle: &SharedDriver,
        epoch_id: &meerkat_core::RuntimeEpochId,
        agent_runtime_id: crate::identifiers::LogicalRuntimeId,
        fence_token: u64,
        generation: u64,
    ) -> Result<(), RuntimeDriverError> {
        let dsl_input = crate::meerkat_machine::dsl::MeerkatMachineInput::PrepareBindings {
            agent_runtime_id: crate::meerkat_machine::dsl::AgentRuntimeId::from_domain(
                &agent_runtime_id,
            ),
            fence_token: crate::meerkat_machine::dsl::FenceToken::from(fence_token),
            generation: Some(crate::meerkat_machine::dsl::Generation::from(generation)),
            runtime_epoch_id: Some(crate::meerkat_machine::dsl::RuntimeEpochId::from_domain(
                epoch_id,
            )),
            session_id: crate::meerkat_machine::dsl::SessionId::from_domain(session_id),
        };
        let staged = self
            .stage_session_dsl_transition(session_id, dsl_input, "PrepareBindings")
            .await
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
        {
            let mut driver = driver_handle.lock().await;
            machine_prepare_bindings_projection(&mut driver);
        }
        if let Err(reason) = self
            .commit_session_dsl_transition(session_id, staged, "PrepareBindings")
            .await
        {
            driver_handle
                .lock()
                .await
                .sync_control_projection_from_dsl_authority();
            return Err(RuntimeDriverError::Internal(reason));
        }
        Ok(())
    }

    pub(super) async fn prepare_session_runtime_bindings(
        &self,
        session_id: SessionId,
        preparation: SessionBindingPreparation,
    ) -> Result<MeerkatMachineCommandResult, RuntimeDriverError> {
        self.prepare_session_runtime_bindings_with_claim(
            session_id,
            preparation,
            uuid::Uuid::new_v4(),
            true,
            None,
        )
        .await
    }

    async fn prepare_session_runtime_bindings_with_claim(
        &self,
        session_id: SessionId,
        preparation: SessionBindingPreparation,
        requested_claim_id: uuid::Uuid,
        release_materialization_claim_on_drop: bool,
        claim_state_sink: Option<
            &Arc<
                std::sync::Mutex<
                    Option<Arc<std::sync::Mutex<crate::RuntimeActorMaterializationClaimState>>>,
                >,
            >,
        >,
    ) -> Result<MeerkatMachineCommandResult, RuntimeDriverError> {
        let unique_materialization_transaction = !release_materialization_claim_on_drop;
        let candidate_materialization_claim_state = Arc::new(std::sync::Mutex::new(
            crate::RuntimeActorMaterializationClaimState::new(unique_materialization_transaction),
        ));
        if unique_materialization_transaction {
            let mut state = candidate_materialization_claim_state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.current = Some(requested_claim_id);
            state.phase = crate::RuntimeActorMaterializationClaimPhase::Prepared;
        }
        if let Some(claim_state_sink) = claim_state_sink {
            *claim_state_sink
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                Some(Arc::clone(&candidate_materialization_claim_state));
        }
        tracing::debug!(
            %session_id,
            ?preparation,
            "MeerkatMachine::prepare_session_runtime_bindings start"
        );
        tracing::debug!(
            %session_id,
            ?preparation,
            "MeerkatMachine::prepare_session_runtime_bindings registering session"
        );
        #[cfg(target_arch = "wasm32")]
        let inserted_by_call = if self.store.is_none() {
            {
                tracing::debug!(%session_id, "MeerkatMachine::prepare_session_runtime_bindings attempting storeless existing check lock");
                let mut sessions = self.sessions.try_write().map_err(|_| {
                    tracing::warn!(
                        %session_id,
                        "storeless session map busy while checking existing registration"
                    );
                    RuntimeDriverError::Internal(format!(
                        "storeless session map busy while registering {session_id}"
                    ))
                })?;
                tracing::debug!(%session_id, "MeerkatMachine::prepare_session_runtime_bindings locked storeless existing check");
                if let Some(existing) = sessions.get_mut(&session_id) {
                    tracing::debug!(
                        %session_id,
                        "MeerkatMachine::prepare_session_runtime_bindings found existing session"
                    );
                    if let Some(error) = existing.registration_blocked_by_unregister(&session_id) {
                        return Err(error);
                    }
                    if existing.clear_dead_attachment() {
                        existing.stage_generated_executor_exit_observation().map_err(|reason| {
                            RuntimeDriverError::Internal(format!(
                                "generated MeerkatMachine rejected executor-exit observation: {reason}"
                            ))
                        })?;
                    }
                    false
                } else {
                    drop(sessions);
                    self.register_storeless_session_inner_sync_build_step(
                        session_id.clone(),
                        Some(Arc::clone(&candidate_materialization_claim_state)),
                    )?
                }
            }
        } else {
            Box::pin(self.register_session_inner_for_actor_materialization(
                session_id.clone(),
                Arc::clone(&candidate_materialization_claim_state),
            ))
            .await?
        };
        #[cfg(not(target_arch = "wasm32"))]
        let inserted_by_call = Box::pin(self.register_session_inner_for_actor_materialization(
            session_id.clone(),
            Arc::clone(&candidate_materialization_claim_state),
        ))
        .await?;
        tracing::debug!(
            %session_id,
            inserted_by_call,
            ?preparation,
            "MeerkatMachine::prepare_session_runtime_bindings registered session"
        );
        // Serialize the full generated registration/binding transaction with
        // executor attachment and teardown. A live idempotent binding remains a
        // valid handle bundle, but it cannot reopen actor materialization.
        let mutation_guard = self
            .lock_current_durability_ready_session_mutation_gate(&session_id)
            .await?;
        let (
            driver_handle,
            epoch_id,
            ops_lifecycle,
            cursor_state,
            tool_visibility_owner,
            dsl_authority_shared,
            handle_teardown_gate,
            live_attachment,
            materialization_claim_state,
        ) = {
            let sessions = self.sessions.read().await;
            let entry = sessions
                .get(&session_id)
                .ok_or(RuntimeDriverError::Internal(format!(
                    "session {session_id} missing after register_session_inner"
                )))?;
            (
                Arc::clone(&entry.driver),
                entry.epoch_id.clone(),
                Arc::clone(&entry.ops_lifecycle),
                Arc::clone(&entry.cursor_state),
                Arc::clone(&entry.tool_visibility_owner),
                Arc::clone(&entry.dsl_authority),
                Arc::clone(&entry.handle_teardown_gate),
                entry.has_live_attachment(),
                Arc::clone(&entry.materialization_claim_state),
            )
        };
        let terminal_supervisor_cleanup_bindings = matches!(
            self.existing_session_runtime_state(&session_id).await,
            Some(RuntimeState::Destroyed)
        ) && self
            .has_terminal_supervisor_cleanup_authority(&session_id)
            .await;
        let materialization_claim_id = if live_attachment
            || terminal_supervisor_cleanup_bindings
            || !unique_materialization_transaction
        {
            if terminal_supervisor_cleanup_bindings {
                let mut state = materialization_claim_state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if state.current == Some(requested_claim_id) {
                    state.current = None;
                    state.phase = crate::RuntimeActorMaterializationClaimPhase::Vacant;
                }
            }
            None
        } else {
            let mut state = materialization_claim_state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if state.current == Some(requested_claim_id)
                && state.phase == crate::RuntimeActorMaterializationClaimPhase::Prepared
            {
                Some(requested_claim_id)
            } else if state.current.is_some()
                || state.phase != crate::RuntimeActorMaterializationClaimPhase::Vacant
            {
                return Err(RuntimeDriverError::StaleAuthority {
                    reason: format!(
                        "session {session_id} already has an active materialization owner"
                    ),
                });
            } else {
                state.current = Some(requested_claim_id);
                state.phase = crate::RuntimeActorMaterializationClaimPhase::Prepared;
                Some(requested_claim_id)
            }
        };
        let legacy_actor_materialization_generation = {
            let state = materialization_claim_state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            (!live_attachment
                && !terminal_supervisor_cleanup_bindings
                && !unique_materialization_transaction
                && state.current.is_none()
                && state.phase == crate::RuntimeActorMaterializationClaimPhase::Vacant)
                .then_some(state.legacy_capability_generation)
        };
        // A committed terminal outbox is process-owned predecessor authority.
        // It is also the durable actor-exclusion fence for cold/stored-only
        // publication: no same-SessionId actor incarnation may materialize
        // until the exact publication receipt has converged. This closes the
        // gap after a stored-only publisher's brief R absence check without
        // retaining M or R across arbitrary EventStore IO.
        if self.runless_terminal_publication_dispatch_pending(&driver_handle)
            || crate::control_plane::has_committed_runless_recovery_carrier(&driver_handle)
                .await
                .map_err(|error| RuntimeDriverError::Internal(error.to_string()))?
        {
            release_failed_materialization_claim(
                &materialization_claim_state,
                materialization_claim_id,
            );
            drop(mutation_guard);
            self.cleanup_failed_materialization_claim(
                &session_id,
                inserted_by_call,
                &epoch_id,
                materialization_claim_id,
                &materialization_claim_state,
            )
            .await;
            return Err(RuntimeDriverError::RuntimeTerminalPublicationInProgress {
                runtime_id: LogicalRuntimeId::for_session(&session_id),
            });
        }
        if materialization_claim_id.is_some()
            && let Some(claim_state_sink) = claim_state_sink
        {
            *claim_state_sink
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                Some(Arc::clone(&materialization_claim_state));
        }
        // A newly recovered entry has no in-process executor. Any machine-valid
        // durable runtime binding on that cold entry belongs to the previous
        // process, even when the recovered ops entry intentionally retains
        // the same epoch. Preserving it makes a replacement runtime id/fence/
        // generation guard-reject PrepareBindings. Drive the existing
        // generated recovery ladder instead of clearing shell fields:
        // RuntimeExecutorExited moves the supported live phases to Stopped,
        // then RegisterSessionResumesStopped below clears the epoch-scoped
        // binding and re-admits the same session. Warm/idempotent registration
        // never enters this arm.
        let current_epoch = crate::meerkat_machine::dsl::RuntimeEpochId::from_domain(&epoch_id);
        let recovered_dead_process_binding = inserted_by_call && !live_attachment && {
            let authority = dsl_authority_shared
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let phase = super::dsl_authority::runtime_phase_from_authority(&authority);
            let state = authority.state();
            matches!(
                phase,
                RuntimeState::Idle | RuntimeState::Attached | RuntimeState::Running
            ) && state.active_runtime_id.is_some()
        };
        if recovered_dead_process_binding {
            tracing::debug!(
                %session_id,
                ?current_epoch,
                "cold recovery observed a dead-process runtime binding; staging generated executor exit before re-registration"
            );
            let executor_exited = crate::meerkat_machine_types::
                MeerkatMachineFieldlessRuntimeInternalInput::RuntimeExecutorExited;
            if let Err(reason) = self
                .stage_session_dsl_transition(
                    &session_id,
                    executor_exited.dsl_input(),
                    executor_exited.input_variant().as_str(),
                )
                .await
            {
                release_failed_materialization_claim(
                    &materialization_claim_state,
                    materialization_claim_id,
                );
                drop(mutation_guard);
                self.cleanup_failed_materialization_claim(
                    &session_id,
                    inserted_by_call,
                    &epoch_id,
                    materialization_claim_id,
                    &materialization_claim_state,
                )
                .await;
                return Err(RuntimeDriverError::Internal(format!(
                    "generated MeerkatMachine rejected cold-recovery executor-exit observation: {reason}"
                )));
            }
        }
        let dsl_session_id = crate::meerkat_machine::dsl::SessionId::from_domain(&session_id);
        // Stage RegisterSession unconditionally: the generated machine owns
        // the idempotence verdict (`RegisterSessionIdempotent` no-ops a
        // same-binding re-registration), the revival verdict
        // (`RegisterSessionResumesStopped` re-admits a stopped session to
        // Idle), and the Destroyed rejection (RegisterSession is not declared
        // from Destroyed). No shell probe of the authority state precedes the
        // staging.
        let registration_input = if terminal_supervisor_cleanup_bindings {
            crate::meerkat_machine::dsl::MeerkatMachineInput::PrepareTerminalSupervisorCleanupBindings {
                session_id: dsl_session_id,
            }
        } else {
            crate::meerkat_machine::dsl::MeerkatMachineInput::RegisterSession {
                session_id: dsl_session_id,
                // The entry epoch this preparation owns. A warm/idempotent
                // registration restates it; a cold revival installs it.
                runtime_epoch_id: Some(current_epoch.clone()),
            }
        };
        match self
            .stage_session_dsl_transition(
                &session_id,
                registration_input,
                if terminal_supervisor_cleanup_bindings {
                    "PrepareTerminalSupervisorCleanupBindings"
                } else {
                    "RegisterSession"
                },
            )
            .await
        {
            Ok(staged) => {
                // The machine may ACCEPT the registration and return a refusal
                // verdict (a different entry epoch is registered under this
                // session id, or the entry is still inside its unregister drain
                // window). The verdict's reason kind owns the typed error;
                // never let a refusal read as an idempotent no-op.
                if let Some(refusal) = staged.effects.session_registration_refusal() {
                    release_failed_materialization_claim(
                        &materialization_claim_state,
                        materialization_claim_id,
                    );
                    drop(mutation_guard);
                    self.cleanup_failed_materialization_claim(
                        &session_id,
                        inserted_by_call,
                        &epoch_id,
                        materialization_claim_id,
                        &materialization_claim_state,
                    )
                    .await;
                    return Err(refusal.into_runtime_driver_error(&session_id));
                }
                if staged.revived_stopped_session() {
                    // Machine-emitted revival: refresh the durable lifecycle
                    // record so cross-process readers never observe a stale
                    // `Stopped` snapshot for a revived session.
                    let persistence_result = {
                        let mut driver = driver_handle.lock().await;
                        driver.persist_current_machine_lifecycle("resume").await
                    };
                    if let Err(err) = persistence_result {
                        let restored = Self::restore_dsl_authority_snapshot_if_current(
                            &dsl_authority_shared,
                            staged.committed_snapshot,
                            staged.previous_snapshot,
                        );
                        if restored {
                            driver_handle
                                .lock()
                                .await
                                .sync_control_projection_from_dsl_authority();
                        }
                        let err = if restored {
                            err
                        } else {
                            RuntimeDriverError::Internal(format!(
                                "{err}; additionally failed to restore generated Stopped authority after revival persistence failure"
                            ))
                        };
                        release_failed_materialization_claim(
                            &materialization_claim_state,
                            materialization_claim_id,
                        );
                        drop(mutation_guard);
                        self.cleanup_failed_materialization_claim(
                            &session_id,
                            inserted_by_call,
                            &epoch_id,
                            materialization_claim_id,
                            &materialization_claim_state,
                        )
                        .await;
                        return Err(err);
                    }
                }

                // A delivery path may have already applied the machine-owned
                // Stopped -> Idle/Queuing readmission before it asks for local
                // bindings. In that case this RegisterSession is idempotent and
                // emits no Recover notice, but the completed exact stop receipt
                // still belongs to the old attachment and must be consumed.
                // MissingLive retains its broader Attached/Active normalization
                // below; this branch handles only the exact already-re-admitted
                // shape shared by ordinary and MissingLive preparation.
                let stop_residue_retirement = {
                    let mut sessions = self.sessions.write().await;
                    match sessions.get_mut(&session_id) {
                        None => Err(RuntimeDriverError::NotReady {
                            state: RuntimeState::Destroyed,
                        }),
                        Some(entry)
                            if entry.epoch_id != epoch_id
                                || !Arc::ptr_eq(&entry.driver, &driver_handle)
                                || !Arc::ptr_eq(&entry.dsl_authority, &dsl_authority_shared)
                                || !Arc::ptr_eq(
                                    &entry.materialization_claim_state,
                                    &materialization_claim_state,
                                ) =>
                        {
                            Err(RuntimeDriverError::StaleAuthority {
                                reason: format!(
                                    "session {session_id} changed before completed stop residue retirement"
                                ),
                            })
                        }
                        Some(entry) => {
                            let idle_queuing = {
                                let authority = entry
                                    .dsl_authority
                                    .lock()
                                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                                let state = authority.state();
                                state.lifecycle_phase
                                    == crate::meerkat_machine::dsl::MeerkatPhase::Idle
                                    && state.registration_phase
                                        == crate::meerkat_machine::dsl::RegistrationPhase::Queuing
                            };
                            if idle_queuing && entry.runtime_stop_cleanup_coordinator.is_some() {
                                entry.retire_completed_runtime_stop_after_revival(&session_id)
                            } else {
                                Ok(())
                            }
                        }
                    }
                };
                if let Err(error) = stop_residue_retirement {
                    release_failed_materialization_claim(
                        &materialization_claim_state,
                        materialization_claim_id,
                    );
                    drop(mutation_guard);
                    self.cleanup_failed_materialization_claim(
                        &session_id,
                        inserted_by_call,
                        &epoch_id,
                        materialization_claim_id,
                        &materialization_claim_state,
                    )
                    .await;
                    return Err(error);
                }
            }
            Err(reason) => {
                let err = self
                    .classify_session_dsl_rejection(&session_id, reason)
                    .await;
                release_failed_materialization_claim(
                    &materialization_claim_state,
                    materialization_claim_id,
                );
                drop(mutation_guard);
                self.cleanup_failed_materialization_claim(
                    &session_id,
                    inserted_by_call,
                    &epoch_id,
                    materialization_claim_id,
                    &materialization_claim_state,
                )
                .await;
                return Err(err);
            }
        }
        if matches!(
            preparation,
            SessionBindingPreparation::LocalSessionResources(
                LocalSessionMaterializationMode::MissingLiveRevival
            )
        ) {
            let Some(exact_claim_id) = materialization_claim_id else {
                release_failed_materialization_claim(
                    &materialization_claim_state,
                    materialization_claim_id,
                );
                drop(mutation_guard);
                self.cleanup_failed_materialization_claim(
                    &session_id,
                    inserted_by_call,
                    &epoch_id,
                    materialization_claim_id,
                    &materialization_claim_state,
                )
                .await;
                return Err(RuntimeDriverError::StaleAuthority {
                    reason: format!(
                        "missing-live materialization for session {session_id} did not mint an exact prepared claim"
                    ),
                });
            };
            if let Err(error) = self
                .normalize_missing_live_session_materialization(
                    &session_id,
                    &epoch_id,
                    exact_claim_id,
                    &materialization_claim_state,
                    &mutation_guard,
                )
                .await
            {
                release_failed_materialization_claim(
                    &materialization_claim_state,
                    materialization_claim_id,
                );
                drop(mutation_guard);
                self.cleanup_failed_materialization_claim(
                    &session_id,
                    inserted_by_call,
                    &epoch_id,
                    materialization_claim_id,
                    &materialization_claim_state,
                )
                .await;
                return Err(error);
            }
        }
        tracing::debug!(
            %session_id,
            ?preparation,
            "MeerkatMachine::prepare_session_runtime_bindings prepared generated registration"
        );
        let allow_late_compaction_binding = !terminal_supervisor_cleanup_bindings
            && matches!(
                preparation,
                SessionBindingPreparation::LocalSessionResources(_)
            );
        let compaction_runtime_binding = if terminal_supervisor_cleanup_bindings {
            tracing::debug!(
                %session_id,
                "preserving Destroyed lifecycle while installing terminal supervisor cleanup handles"
            );
            None
        } else if preparation == SessionBindingPreparation::AuthoritativeRuntimeBinding {
            let runtime_id = {
                tracing::debug!(
                    %session_id,
                    ?preparation,
                    "MeerkatMachine::prepare_session_runtime_bindings locking driver for runtime id"
                );
                let driver = driver_handle.lock().await;
                driver.runtime_id().clone()
            };
            tracing::debug!(
                %session_id,
                ?preparation,
                "MeerkatMachine::prepare_session_runtime_bindings locked driver for runtime id"
            );
            let agent_runtime_id =
                crate::meerkat_machine::dsl::AgentRuntimeId::from_domain(&runtime_id);
            let fence_token = crate::meerkat_machine::dsl::FenceToken::from(0);
            let generation = crate::meerkat_machine::dsl::Generation::from(0);
            if let Err(error) = self
                .commit_runtime_placement_binding(
                    &session_id,
                    &driver_handle,
                    &epoch_id,
                    runtime_id,
                    0,
                    0,
                )
                .await
            {
                release_failed_materialization_claim(
                    &materialization_claim_state,
                    materialization_claim_id,
                );
                drop(mutation_guard);
                self.cleanup_failed_materialization_claim(
                    &session_id,
                    inserted_by_call,
                    &epoch_id,
                    materialization_claim_id,
                    &materialization_claim_state,
                )
                .await;
                return Err(error);
            }
            tracing::debug!(
                %session_id,
                ?preparation,
                "MeerkatMachine::prepare_session_runtime_bindings applied authoritative projection"
            );
            Some((agent_runtime_id, Some(fence_token), Some(generation)))
        } else {
            {
                tracing::debug!(
                    %session_id,
                    ?preparation,
                    "MeerkatMachine::prepare_session_runtime_bindings locking driver for local projection"
                );
                let mut driver = driver_handle.lock().await;
                machine_prepare_bindings_projection(&mut driver);
            }
            tracing::debug!(
                %session_id,
                ?preparation,
                "MeerkatMachine::prepare_session_runtime_bindings applied local projection"
            );
            None
        };
        let runtime_authority = match preparation {
            SessionBindingPreparation::AuthoritativeRuntimeBinding => {
                crate::session_runtime_bindings_authority(
                    session_id.clone(),
                    epoch_id.clone(),
                    Arc::clone(&dsl_authority_shared),
                    Arc::clone(&handle_teardown_gate),
                    materialization_claim_id,
                    Arc::clone(&materialization_claim_state),
                    legacy_actor_materialization_generation,
                    release_materialization_claim_on_drop,
                )
            }
            SessionBindingPreparation::LocalSessionResources(_) => {
                crate::local_session_runtime_bindings_authority(
                    session_id.clone(),
                    epoch_id.clone(),
                    Arc::clone(&dsl_authority_shared),
                    Arc::clone(&handle_teardown_gate),
                    materialization_claim_id,
                    Arc::clone(&materialization_claim_state),
                    legacy_actor_materialization_generation,
                    release_materialization_claim_on_drop,
                )
            }
        };

        tracing::debug!(
            %session_id,
            ?preparation,
            "MeerkatMachine::prepare_session_runtime_bindings assembling bindings"
        );
        let bindings = match self
            .canonical_session_runtime_bindings(
                session_id.clone(),
                epoch_id.clone(),
                Arc::clone(&ops_lifecycle),
                Arc::clone(&cursor_state),
                Arc::clone(&tool_visibility_owner),
                Arc::clone(&dsl_authority_shared),
                Arc::clone(&handle_teardown_gate),
                compaction_runtime_binding,
                allow_late_compaction_binding,
                runtime_authority,
            )
            .await
        {
            Ok(bindings) => bindings,
            Err(error) => {
                release_failed_materialization_claim(
                    &materialization_claim_state,
                    materialization_claim_id,
                );
                drop(mutation_guard);
                self.cleanup_failed_materialization_claim(
                    &session_id,
                    inserted_by_call,
                    &epoch_id,
                    materialization_claim_id,
                    &materialization_claim_state,
                )
                .await;
                return Err(error);
            }
        };
        Ok(MeerkatMachineCommandResult::Bindings(bindings))
    }

    async fn prepare_session_materialization_with_mode(
        self: &Arc<Self>,
        session_id: SessionId,
        preparation: SessionBindingPreparation,
    ) -> Result<PreparedSessionMaterialization, RuntimeBindingsError> {
        let cleanup_spawner = MachineCleanupTaskSpawner::acquire().map_err(|error| {
            RuntimeBindingsError::PrepareFailed(session_id.clone(), error.to_string())
        })?;
        let machine = Arc::clone(self);
        let owned_session_id = session_id.clone();
        let (result_tx, result_rx) = crate::tokio::sync::oneshot::channel();
        cleanup_spawner.spawn(async move {
            let result = machine
                .prepare_session_materialization_with_mode_owned(owned_session_id, preparation)
                .await;
            // The process-owned saga must reach a terminal result before an
            // abandoned caller can drop Prepared and begin exact rollback.
            // If the receiver disappeared after success, dropping the payload
            // here transfers rollback to Prepared's process-owned cleanup.
            let _ = result_tx.send(result);
        });
        result_rx.await.map_err(|error| {
            RuntimeBindingsError::PrepareFailed(
                session_id,
                format!(
                    "owned session materialization preparation ended without a result: {error}"
                ),
            )
        })?
    }

    async fn prepare_session_materialization_with_mode_owned(
        self: &Arc<Self>,
        session_id: SessionId,
        preparation: SessionBindingPreparation,
    ) -> Result<PreparedSessionMaterialization, RuntimeBindingsError> {
        let claim_id = uuid::Uuid::new_v4();
        let mut pending =
            PendingPreparedMaterialization::new(Arc::clone(self), session_id.clone(), claim_id)
                .map_err(|error| {
                    RuntimeBindingsError::PrepareFailed(session_id.clone(), error.to_string())
                })?;
        let claim_state_slot = pending.claim_state_slot();
        let result = self
            .prepare_session_runtime_bindings_with_claim(
                session_id.clone(),
                preparation,
                claim_id,
                false,
                Some(&claim_state_slot),
            )
            .await
            .map_err(|error| {
                RuntimeBindingsError::PrepareFailed(session_id.clone(), error.to_string())
            })?;
        let MeerkatMachineCommandResult::Bindings(bindings) = result else {
            return Err(RuntimeBindingsError::SessionNotFound(session_id));
        };
        let prepared = PreparedSessionMaterialization::new(
            Arc::clone(self),
            bindings,
            claim_id,
            pending.cleanup_spawner(),
        )
        .map_err(|error| {
            RuntimeBindingsError::PrepareFailed(session_id.clone(), error.to_string())
        })?;
        pending.disarm();
        Ok(prepared)
    }

    /// Begin one unique authoritative actor-materialization transaction.
    /// Cloneable bindings are exposed through the returned non-clone lease.
    pub async fn prepare_session_materialization(
        self: &Arc<Self>,
        session_id: SessionId,
    ) -> Result<PreparedSessionMaterialization, RuntimeBindingsError> {
        self.prepare_session_materialization_with_mode(
            session_id,
            SessionBindingPreparation::AuthoritativeRuntimeBinding,
        )
        .await
    }

    /// Local-resource variant used by mob hosts before authoritative placement.
    pub async fn prepare_local_session_materialization(
        self: &Arc<Self>,
        session_id: SessionId,
    ) -> Result<PreparedSessionMaterialization, RuntimeBindingsError> {
        self.prepare_local_session_materialization_with_mode(
            session_id,
            LocalSessionMaterializationMode::Ordinary,
        )
        .await
    }

    /// Local-resource preparation with a typed machine-owned revival policy.
    /// Only the Mob missing-live path may request the narrow normalization;
    /// every ordinary surface uses [`Self::prepare_local_session_materialization`].
    pub async fn prepare_local_session_materialization_with_mode(
        self: &Arc<Self>,
        session_id: SessionId,
        mode: LocalSessionMaterializationMode,
    ) -> Result<PreparedSessionMaterialization, RuntimeBindingsError> {
        self.prepare_session_materialization_with_mode(
            session_id,
            SessionBindingPreparation::LocalSessionResources(mode),
        )
        .await
    }

    /// Durable input-state witnesses for an UNREGISTERED session.
    ///
    /// Persistent machines answer from the RuntimeStore rows committed
    /// atomically at every machine lifecycle boundary (rows are never
    /// deleted, so terminal facts persist). A session with no lifecycle
    /// record AND no input rows was never admitted and fails typed
    /// `NotFound` — never an empty success. Ephemeral machines keep the
    /// existing `NotReady` class: with no store there is no durable witness
    /// to answer from.
    pub(super) async fn durable_session_input_witnesses(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<StoredInputState>, RuntimeDriverError> {
        let Some(store) = self.store.as_ref() else {
            return Err(RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            });
        };
        let runtime_id = Self::logical_runtime_id(session_id);
        let witnesses = store
            .load_input_states_strict(&runtime_id)
            .await
            .map_err(|err| {
                RuntimeDriverError::Internal(format!(
                    "terminal-status witness read failed for {runtime_id}: {err}"
                ))
            })?;
        if witnesses.is_empty() {
            let lifecycle = store
                .load_machine_lifecycle_record(&runtime_id)
                .await
                .map_err(|err| {
                    RuntimeDriverError::Internal(format!(
                        "terminal-status lifecycle read failed for {runtime_id}: {err}"
                    ))
                })?;
            if lifecycle.is_none() {
                return Err(RuntimeDriverError::NotFound { runtime_id });
            }
        }
        Ok(witnesses)
    }

    async fn require_durable_runtime_after_input_point_miss(
        store: &dyn crate::store::RuntimeStore,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<(), RuntimeDriverError> {
        let lifecycle = store
            .load_machine_lifecycle_record(runtime_id)
            .await
            .map_err(|err| {
                RuntimeDriverError::Internal(format!(
                    "terminal-status lifecycle read failed for {runtime_id}: {err}"
                ))
            })?;
        if lifecycle.is_some() {
            return Ok(());
        }
        let has_any_input = !store
            .load_input_states_strict(runtime_id)
            .await
            .map_err(|err| {
                RuntimeDriverError::Internal(format!(
                    "terminal-status existence read failed for {runtime_id}: {err}"
                ))
            })?
            .is_empty();
        if has_any_input {
            Ok(())
        } else {
            Err(RuntimeDriverError::NotFound {
                runtime_id: runtime_id.clone(),
            })
        }
    }

    fn durable_idempotency_index_error(
        runtime_id: &LogicalRuntimeId,
        context: &'static str,
        error: crate::store::RuntimeStoreError,
    ) -> RuntimeDriverError {
        match error {
            crate::store::RuntimeStoreError::Unsupported(reason) => {
                RuntimeDriverError::RecoveryRepairBlocked {
                    evidence_digest: None,
                    reason: format!(
                        "{context} requires the exact store-owned idempotency index for \
                         {runtime_id}: {reason}"
                    ),
                }
            }
            error @ crate::store::RuntimeStoreError::InputIdempotencyIndexUncertain { .. } => {
                RuntimeDriverError::RecoveryRepairBlocked {
                    evidence_digest: None,
                    reason: format!(
                        "{context} found durable idempotency-index corruption for {runtime_id}: \
                         {error}"
                    ),
                }
            }
            other => {
                RuntimeDriverError::Internal(format!("{context} failed for {runtime_id}: {other}"))
            }
        }
    }

    /// Resolve one durable input by its exact store-owned idempotency index.
    ///
    /// A miss still has to preserve the public distinction between an existing
    /// session with no such key and a session that was never admitted. The
    /// lifecycle row is the ordinary existence witness. Only the anomalous
    /// lifecycle-missing case falls back to an existence-only row read; key
    /// resolution itself never scans or text-matches input history.
    pub(super) async fn durable_session_input_witness_by_idempotency_key(
        &self,
        session_id: &SessionId,
        key: &str,
    ) -> Result<Option<StoredInputState>, RuntimeDriverError> {
        let Some(store) = self.store.as_ref() else {
            return Err(RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            });
        };
        let runtime_id = Self::logical_runtime_id(session_id);
        let key = crate::identifiers::IdempotencyKey::new(key);
        let observation = store
            .load_input_state_by_idempotency_key(&runtime_id, &key)
            .await
            .map_err(|error| {
                Self::durable_idempotency_index_error(
                    &runtime_id,
                    "indexed input witness read",
                    error,
                )
            })?;
        if let Some(observation) = observation {
            let (state, _exact_row_digest) = observation.into_parts();
            return Ok(Some(state));
        }

        Self::require_durable_runtime_after_input_point_miss(store.as_ref(), &runtime_id).await?;
        Ok(None)
    }

    /// Resolve one durable input by exact id while preserving the public
    /// existing-session versus never-admitted distinction on a miss.
    ///
    /// The ordinary selector path is one point read. Only an anomalous runtime
    /// with input rows but no lifecycle row needs the existence-only fallback.
    pub(super) async fn durable_session_input_witness_by_id(
        &self,
        session_id: &SessionId,
        input_id: &InputId,
    ) -> Result<Option<StoredInputState>, RuntimeDriverError> {
        let Some(store) = self.store.as_ref() else {
            return Err(RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            });
        };
        let runtime_id = Self::logical_runtime_id(session_id);
        let witness = store
            .load_input_state(&runtime_id, input_id)
            .await
            .map_err(|err| {
                RuntimeDriverError::Internal(format!(
                    "durable input witness read failed for {runtime_id}: {err}"
                ))
            })?;
        if witness.is_some() {
            return Ok(witness);
        }

        Self::require_durable_runtime_after_input_point_miss(store.as_ref(), &runtime_id).await?;
        Ok(None)
    }

    async fn durable_input_witness_by_id(
        &self,
        session_id: &SessionId,
        input_id: &InputId,
    ) -> Result<Option<StoredInputState>, RuntimeDriverError> {
        let Some(store) = self.store.as_ref() else {
            return Ok(None);
        };
        let runtime_id = Self::logical_runtime_id(session_id);
        store
            .load_input_state(&runtime_id, input_id)
            .await
            .map_err(|err| {
                RuntimeDriverError::Internal(format!(
                    "durable input witness read failed for {runtime_id}: {err}"
                ))
            })
    }

    async fn durable_input_witness_by_idempotency_key_if_present(
        &self,
        session_id: &SessionId,
        key: &str,
    ) -> Result<Option<StoredInputState>, RuntimeDriverError> {
        let Some(store) = self.store.as_ref() else {
            return Ok(None);
        };
        let runtime_id = Self::logical_runtime_id(session_id);
        store
            .load_input_state_by_idempotency_key(
                &runtime_id,
                &crate::identifiers::IdempotencyKey::new(key),
            )
            .await
            .map_err(|error| {
                Self::durable_idempotency_index_error(
                    &runtime_id,
                    "optional indexed input witness read",
                    error,
                )
            })
            .map(|observation| observation.map(|observation| observation.into_parts().0))
    }

    /// Input-state witnesses for a session: the live DSL-backed snapshot when
    /// the session is registered, the durable store rows otherwise. Both
    /// sides feed the same pure evaluators in [`crate::terminal_status`].
    pub(super) async fn session_input_witnesses(
        &self,
        session_id: &SessionId,
    ) -> Result<(TerminalWitnessSource, Vec<StoredInputState>), RuntimeDriverError> {
        let driver = {
            let sessions = self.sessions.read().await;
            sessions.get(session_id).map(|entry| entry.driver.clone())
        };
        if let Some(driver) = driver {
            let driver = driver.lock().await;
            let live = driver.as_driver().stored_input_states_snapshot()?;
            drop(driver);
            let Some(store) = self.store.as_ref() else {
                return Ok((TerminalWitnessSource::LiveRuntime, live));
            };
            let runtime_id = Self::logical_runtime_id(session_id);
            let mut durable = store
                .load_input_states_strict(&runtime_id)
                .await
                .map_err(|err| {
                    RuntimeDriverError::Internal(format!(
                        "terminal-status witness read failed for {runtime_id}: {err}"
                    ))
                })?;
            let live_ids = live
                .iter()
                .map(|stored| stored.state.input_id.clone())
                .collect::<HashSet<_>>();
            durable.retain(|stored| !live_ids.contains(&stored.state.input_id));
            durable.extend(live);
            return Ok((TerminalWitnessSource::LiveRuntime, durable));
        }
        Ok((
            TerminalWitnessSource::DurableStore,
            self.durable_session_input_witnesses(session_id).await?,
        ))
    }

    pub(super) async fn execute_meerkat_machine_session_command(
        &self,
        command: MeerkatMachineCommand,
    ) -> Result<MeerkatMachineCommandResult, RuntimeDriverError> {
        match command {
            MeerkatMachineCommand::RegisterSession { session_id } => {
                let sid = session_id.clone();
                // A generated Draining image reconstructed from the durable
                // unregister retry record is CONCLUDED here, not bypassed.
                // Through 0.8.23 this arm returned `Unit` on that witness -
                // reporting success for a registration the authority had not
                // admitted - and left the entry Draining, so every later
                // registration (binding preparation included) was guard-rejected
                // for the life of the durable record. Settling reuses the same
                // teardown `ensure_runtime_executor_attachment` joins.
                let registration = self
                    .register_session_settling_cold_recovered_drain(&sid, None)
                    .await?;
                debug_assert!(
                    registration
                        != super::session_management::RegisterSessionInnerOutcome::InsertedColdRecoveredDraining,
                    "settled registration must not remain in a cold-recovered drain window"
                );
                let _registration_gate_guard = self.lock_registration_gate(&sid).await?;
                // The entry that `register_session_inner` just published owns
                // the runtime epoch this registration restates.
                let entry_epoch_id = {
                    let sessions = self.sessions.read().await;
                    sessions
                        .get(&sid)
                        .map(|entry| entry.epoch_id.clone())
                        .ok_or(RuntimeDriverError::NotReady {
                            state: RuntimeState::Destroyed,
                        })?
                };
                // Stage-first: the generated machine owns the legality verdict.
                // RegisterSession is not declared from Destroyed (it is a
                // resurrection input the DestroyedShapeInvariant forbids), so a
                // resident OR cold-recovered Destroyed binding is rejected by
                // the machine and classified as the terminal `Destroyed` truth
                // — never silently skipped, never preflighted in the shell. A
                // same-binding, same-epoch re-registration is the machine-owned
                // `RegisterSessionIdempotent` no-op; a same-binding registration
                // naming a different entry epoch is the machine-owned refusal
                // verdict handled below.
                let staged = match self
                    .stage_session_dsl_transition(
                        &sid,
                        crate::meerkat_machine::dsl::MeerkatMachineInput::RegisterSession {
                            session_id: crate::meerkat_machine::dsl::SessionId::from_domain(&sid),
                            runtime_epoch_id: Some(
                                crate::meerkat_machine::dsl::RuntimeEpochId::from_domain(
                                    &entry_epoch_id,
                                ),
                            ),
                        },
                        "RegisterSession",
                    )
                    .await
                {
                    Ok(staged) => staged,
                    Err(reason) => {
                        return Err(self.classify_session_dsl_rejection(&sid, reason).await);
                    }
                };
                if let Some(refusal) = staged.effects.session_registration_refusal() {
                    return Err(refusal.into_runtime_driver_error(&sid));
                }
                Ok(MeerkatMachineCommandResult::Unit)
            }
            MeerkatMachineCommand::UnregisterSession { session_id } => {
                if !self.sessions.read().await.contains_key(&session_id) {
                    return Err(RuntimeDriverError::NotReady {
                        state: RuntimeState::Destroyed,
                    });
                }
                self.unregister_session_inner(&session_id).await?;
                Ok(MeerkatMachineCommandResult::Unit)
            }
            MeerkatMachineCommand::SetSilentIntents {
                session_id,
                intents,
            } => {
                let _gate_guard = self
                    .lock_current_durability_ready_session_mutation_gate(&session_id)
                    .await?;

                // Stage-first: SetSilentIntents is not declared from Destroyed,
                // so the machine rejects it there and the rejection is
                // classified as the terminal `Destroyed` truth.
                if let Err(reason) = self
                    .stage_session_dsl_input(
                        &session_id,
                        crate::meerkat_machine::dsl::MeerkatMachineInput::SetSilentIntents {
                            session_id: crate::meerkat_machine::dsl::SessionId::from_domain(
                                &session_id,
                            ),
                            intents: intents.into_iter().collect(),
                        },
                        "SetSilentIntents",
                    )
                    .await
                {
                    return Err(self
                        .classify_session_dsl_rejection(&session_id, reason)
                        .await);
                }
                Ok(MeerkatMachineCommandResult::Unit)
            }
            MeerkatMachineCommand::CancelAfterBoundary { session_id } => {
                // Stage-first: `cancel_after_boundary_inner` stages the
                // CancelAfterBoundary DSL input; the machine rejects it on a
                // Destroyed binding and the inner classification surfaces the
                // terminal `Destroyed` truth.
                self.cancel_after_boundary_inner(&session_id).await?;
                Ok(MeerkatMachineCommandResult::Unit)
            }
            MeerkatMachineCommand::StopRuntimeExecutor { session_id, reason } => {
                // Stage-first: `stop_runtime_executor_inner` stages the
                // StopRuntimeExecutor DSL input; the machine rejects it on a
                // Destroyed binding and the inner classification surfaces the
                // terminal `Destroyed` truth. The independently-owned
                // unregister saga acquires and retains the live/lifecycle
                // disposal lease. A caller-owned lease here would deadlock the
                // stop result against that saga once the exact runtime-loop
                // cleanup publishes generated Draining authority.
                self.stop_runtime_executor_inner(&session_id, reason)
                    .await?;
                Ok(MeerkatMachineCommandResult::Unit)
            }
            MeerkatMachineCommand::ContainsSession { session_id } => {
                Ok(MeerkatMachineCommandResult::Bool(
                    self.sessions.read().await.contains_key(&session_id),
                ))
            }
            MeerkatMachineCommand::SessionHasExecutor { session_id } => {
                let sessions = self.sessions.read().await;
                Ok(MeerkatMachineCommandResult::Bool(
                    sessions
                        .get(&session_id)
                        .map(
                            RuntimeSessionEntry::generated_executor_registration_has_viable_attachment,
                        )
                        .unwrap_or(false),
                ))
            }
            MeerkatMachineCommand::SessionHasComms { session_id } => {
                let engaged = self
                    .drain_authority_state(&session_id)
                    .await
                    .is_some_and(|state| {
                        state.peer_owner_kind
                            != crate::meerkat_machine::dsl::PeerIngressOwnerKind::Unattached
                    });
                Ok(MeerkatMachineCommandResult::Bool(engaged))
            }
            MeerkatMachineCommand::OpsLifecycleRegistry { session_id } => {
                let sessions = self.sessions.read().await;
                Ok(MeerkatMachineCommandResult::OpsLifecycleRegistry(
                    sessions
                        .get(&session_id)
                        .map(|e| Arc::clone(&e.ops_lifecycle)),
                ))
            }
            MeerkatMachineCommand::PrepareBindings { session_id } => {
                Box::pin(self.prepare_session_runtime_bindings(
                    session_id,
                    SessionBindingPreparation::AuthoritativeRuntimeBinding,
                ))
                .await
            }
            MeerkatMachineCommand::PrepareLocalSessionBindings { session_id } => {
                Box::pin(self.prepare_session_runtime_bindings(
                    session_id,
                    SessionBindingPreparation::LocalSessionResources(
                        LocalSessionMaterializationMode::Ordinary,
                    ),
                ))
                .await
            }
            MeerkatMachineCommand::InputState {
                session_id,
                input_id,
            } => {
                let driver = {
                    let sessions = self.sessions.read().await;
                    sessions.get(&session_id).map(|entry| entry.driver.clone())
                };
                match driver {
                    Some(driver) => {
                        let driver = driver.lock().await;
                        let live = driver.as_driver().stored_input_state(&input_id);
                        drop(driver);
                        Ok(MeerkatMachineCommandResult::InputState(match live {
                            Some(live) => Some(live),
                            None => {
                                self.durable_input_witness_by_id(&session_id, &input_id)
                                    .await?
                            }
                        }))
                    }
                    // Restart-first-class fallback: an unregistered session on
                    // a persistent machine answers from the durable
                    // RuntimeStore point index without reviving the runtime.
                    None => Ok(MeerkatMachineCommandResult::InputState(
                        self.durable_session_input_witness_by_id(&session_id, &input_id)
                            .await?,
                    )),
                }
            }
            MeerkatMachineCommand::InputStateByIdempotencyKey {
                session_id,
                idempotency_key,
            } => {
                let driver = {
                    let sessions = self.sessions.read().await;
                    sessions.get(&session_id).map(|entry| entry.driver.clone())
                };
                match driver {
                    Some(driver) => {
                        let live = {
                            let driver = driver.lock().await;
                            let driver = driver.as_driver();
                            driver
                                .input_id_for_idempotency_key(&idempotency_key)
                                .and_then(|input_id| driver.stored_input_state(&input_id))
                        };
                        Ok(MeerkatMachineCommandResult::InputState(match live {
                            Some(live) => Some(live),
                            None => {
                                self.durable_input_witness_by_idempotency_key_if_present(
                                    &session_id,
                                    &idempotency_key,
                                )
                                .await?
                            }
                        }))
                    }
                    // Restart-first-class fallback: the persisted shell key is
                    // the exact fact recovery re-enters as the machine-owned
                    // idempotency binding, so the durable witness and the live
                    // admission map cannot diverge.
                    None => Ok(MeerkatMachineCommandResult::InputState(
                        self.durable_session_input_witness_by_idempotency_key(
                            &session_id,
                            &idempotency_key,
                        )
                        .await?,
                    )),
                }
            }
            // Durable-only key resolution. The live admission map is
            // deliberately not consulted: a caller classifying an expired
            // submit bound must not be told "durably queued" on the strength
            // of an in-memory row, and this read must stay answerable while
            // the session driver lock is held by the admission it is
            // classifying.
            MeerkatMachineCommand::DurableInputStateByIdempotencyKey {
                session_id,
                idempotency_key,
            } => Ok(MeerkatMachineCommandResult::InputState(
                self.durable_input_witness_by_idempotency_key_if_present(
                    &session_id,
                    &idempotency_key,
                )
                .await?,
            )),
            MeerkatMachineCommand::InteractionTerminalStatus {
                session_id,
                selector,
            } => {
                let driver = {
                    let sessions = self.sessions.read().await;
                    sessions.get(&session_id).map(|entry| entry.driver.clone())
                };
                let sourced = match driver {
                    Some(driver) => {
                        let live = {
                            let driver = driver.lock().await;
                            let driver = driver.as_driver();
                            match &selector {
                                InteractionSelector::InputId(input_id) => {
                                    driver.stored_input_state(input_id)
                                }
                                // Live path: the machine-owned admission map is
                                // the authority for key -> input resolution.
                                InteractionSelector::IdempotencyKey(key) => driver
                                    .input_id_for_idempotency_key(key)
                                    .and_then(|input_id| driver.stored_input_state(&input_id)),
                            }
                        };
                        match live {
                            Some(bundle) => Some(Sourced {
                                source: TerminalWitnessSource::LiveRuntime,
                                report: interaction_report(&bundle),
                            }),
                            None => match &selector {
                                InteractionSelector::InputId(input_id) => {
                                    self.durable_input_witness_by_id(&session_id, input_id)
                                        .await?
                                }
                                InteractionSelector::IdempotencyKey(key) => {
                                    self.durable_input_witness_by_idempotency_key_if_present(
                                        &session_id,
                                        key,
                                    )
                                    .await?
                                }
                            }
                            .map(|bundle| Sourced {
                                source: TerminalWitnessSource::DurableStore,
                                report: interaction_report(&bundle),
                            }),
                        }
                    }
                    None => {
                        let bundle = match &selector {
                            InteractionSelector::InputId(input_id) => {
                                self.durable_session_input_witness_by_id(&session_id, input_id)
                                    .await?
                            }
                            InteractionSelector::IdempotencyKey(key) => {
                                self.durable_session_input_witness_by_idempotency_key(
                                    &session_id,
                                    key,
                                )
                                .await?
                            }
                        };
                        bundle.as_ref().map(|bundle| Sourced {
                            source: TerminalWitnessSource::DurableStore,
                            report: interaction_report(bundle),
                        })
                    }
                };
                Ok(MeerkatMachineCommandResult::InteractionTerminalStatus(
                    sourced,
                ))
            }
            MeerkatMachineCommand::RunTerminalStatus { session_id, run_id } => {
                let (source, witnesses) = self.session_input_witnesses(&session_id).await?;
                Ok(MeerkatMachineCommandResult::RunTerminalStatus(Sourced {
                    source,
                    report: terminal_status::evaluate_run(&run_id, &witnesses),
                }))
            }
            MeerkatMachineCommand::ListActiveInputs { session_id } => {
                let driver = {
                    let sessions = self.sessions.read().await;
                    let entry = sessions
                        .get(&session_id)
                        .ok_or(RuntimeDriverError::NotReady {
                            state: RuntimeState::Destroyed,
                        })?;
                    entry.driver.clone()
                };
                let driver = driver.lock().await;
                Ok(MeerkatMachineCommandResult::ActiveInputs(
                    driver.as_driver().active_input_ids(),
                ))
            }
            MeerkatMachineCommand::ReconfigureSessionLlmIdentity {
                session_id,
                previous_identity,
                previous_visibility_state,
                previous_capability_surface,
                previous_capability_surface_status,
                view_image_tool_available,
                previous_view_image_visible,
                next_view_image_visible,
                previous_active_visibility_revision,
                previous_staged_visibility_revision,
                target_identity,
                target_capability_surface,
                next_visibility_state,
                next_capability_base_filter,
                next_active_visibility_revision,
                tool_visibility_delta,
            } => {
                let _gate_guard = self
                    .lock_current_durability_ready_session_mutation_gate(&session_id)
                    .await?;

                use crate::meerkat_machine::dsl as mm_dsl;
                let dsl_previous_identity =
                    mm_dsl::SessionLlmIdentity::from_domain(previous_identity.as_ref());
                let dsl_previous_visibility_state = mm_dsl::SessionToolVisibilityState::from_domain(
                    previous_visibility_state.as_ref(),
                );
                let dsl_previous_capability_surface = previous_capability_surface
                    .as_ref()
                    .map(mm_dsl::SessionLlmCapabilitySurface::from_domain);
                let dsl_previous_capability_surface_status =
                    mm_dsl::SessionLlmCapabilitySurfaceStatus::from_domain(
                        &previous_capability_surface_status,
                    );
                let dsl_previous_capability_base_filter = mm_dsl::ToolFilter::from_domain(
                    &previous_visibility_state.capability_base_filter,
                );
                let dsl_target_identity =
                    mm_dsl::SessionLlmIdentity::from_domain(target_identity.as_ref());
                let dsl_target_capability_surface =
                    mm_dsl::SessionLlmCapabilitySurface::from_domain(&target_capability_surface);
                let dsl_next_visibility_state =
                    mm_dsl::SessionToolVisibilityState::from_domain(next_visibility_state.as_ref());
                let dsl_next_capability_base_filter =
                    mm_dsl::ToolFilter::from_domain(&next_capability_base_filter);
                let dsl_tool_visibility_delta =
                    mm_dsl::SessionToolVisibilityDelta::from_domain(tool_visibility_delta.as_ref());

                let staged_dsl_input = self
                    .stage_session_dsl_transition(
                        &session_id,
                        crate::meerkat_machine::dsl::MeerkatMachineInput::ReconfigureSessionLlmIdentity {
                            previous_identity: dsl_previous_identity,
                            previous_visibility_state: dsl_previous_visibility_state,
                            previous_capability_surface: dsl_previous_capability_surface,
                            previous_capability_surface_status:
                                dsl_previous_capability_surface_status,
                            previous_capability_base_filter: dsl_previous_capability_base_filter,
                            view_image_tool_available,
                            previous_view_image_visible,
                            next_view_image_visible,
                            previous_active_visibility_revision,
                            previous_staged_visibility_revision,
                            target_identity: dsl_target_identity,
                            target_capability_surface: dsl_target_capability_surface,
                            next_visibility_state: dsl_next_visibility_state,
                            next_capability_base_filter: dsl_next_capability_base_filter,
                            next_active_visibility_revision,
                            tool_visibility_delta: dsl_tool_visibility_delta,
                        },
                        "ReconfigureSessionLlmIdentity",
                    )
                    .await
                    .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
                let authority_plan =
                    Self::session_llm_reconfigure_authority_plan(&staged_dsl_input.effects)?;
                let report = match self
                    .reconfigure_session_llm_identity_inner(
                        &session_id,
                        *previous_identity,
                        previous_capability_surface,
                        *previous_visibility_state,
                        *target_identity,
                        *target_capability_surface,
                        *next_visibility_state,
                        authority_plan,
                    )
                    .await
                {
                    Ok(report) => report,
                    Err(err) => {
                        self.restore_session_dsl_state(
                            &session_id,
                            staged_dsl_input.previous_snapshot,
                        )
                        .await;
                        if err.clear_generated_llm_state {
                            self.stage_session_dsl_input(
                                &session_id,
                                crate::meerkat_machine::dsl::MeerkatMachineInput::ClearSessionLlmState,
                                "ClearSessionLlmState",
                            )
                            .await
                            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
                        }
                        return Err(err.error);
                    }
                };
                Ok(MeerkatMachineCommandResult::LlmReconfigured(report))
            }
            MeerkatMachineCommand::StagePersistentFilter {
                session_id,
                filter,
                witnesses,
            } => {
                if !self.sessions.read().await.contains_key(&session_id) {
                    return Err(RuntimeDriverError::NotReady {
                        state: RuntimeState::Destroyed,
                    });
                }

                let _gate_guard = self
                    .lock_current_durability_ready_session_mutation_gate(&session_id)
                    .await?;

                let owner = {
                    let sessions = self.sessions.read().await;
                    Arc::clone(
                        &sessions
                            .get(&session_id)
                            .ok_or(RuntimeDriverError::NotReady {
                                state: RuntimeState::Destroyed,
                            })?
                            .tool_visibility_owner,
                    )
                };
                // Delegate to the owner — the `MachineToolVisibilityOwner`
                // trait impl fires the `StageVisibilityFilter` DSL input
                // internally (dogma round 4, wave 2b #12: DSL owns the
                // `next_staged_visibility_revision` monotonic). The DSL
                // input's `update {}` increments and stamps the revision
                // under the authority lock; the owner reads the minted
                // value back and projects it onto its own state.
                // Stage-first: the owner fires the StageVisibilityFilter DSL
                // input, which is not declared from Destroyed — classify a
                // rejection on a Destroyed binding as the terminal truth.
                let revision = match owner.stage_persistent_filter(filter, witnesses) {
                    Ok(revision) => revision,
                    Err(err) => {
                        return Err(self
                            .classify_session_driver_rejection(
                                &session_id,
                                RuntimeDriverError::Internal(err.to_string()),
                            )
                            .await);
                    }
                };
                Ok(MeerkatMachineCommandResult::VisibilityRevision(revision))
            }
            MeerkatMachineCommand::RequestDeferredTools {
                session_id,
                authorities,
            } => {
                if !self.sessions.read().await.contains_key(&session_id) {
                    return Err(RuntimeDriverError::NotReady {
                        state: RuntimeState::Destroyed,
                    });
                }

                let _gate_guard = self
                    .lock_current_durability_ready_session_mutation_gate(&session_id)
                    .await?;

                let owner = {
                    let sessions = self.sessions.read().await;
                    Arc::clone(
                        &sessions
                            .get(&session_id)
                            .ok_or(RuntimeDriverError::NotReady {
                                state: RuntimeState::Destroyed,
                            })?
                            .tool_visibility_owner,
                    )
                };
                // Delegate to the owner: `request_deferred_tools` applies one
                // generated authority-bearing batch input and then mirrors the
                // accepted machine state into the owner projection. Stage-first:
                // the input is not declared from Destroyed — classify a
                // rejection on a Destroyed binding as the terminal truth.
                let revision = match owner.request_deferred_tools(authorities) {
                    Ok(revision) => revision,
                    Err(err) => {
                        return Err(self
                            .classify_session_driver_rejection(
                                &session_id,
                                RuntimeDriverError::Internal(err.to_string()),
                            )
                            .await);
                    }
                };
                Ok(MeerkatMachineCommandResult::VisibilityRevision(revision))
            }
            MeerkatMachineCommand::PublishCommittedVisibleSet {
                session_id,
                visibility_state,
            } => {
                // Guard: session must exist — publishing to an unknown session
                // has no target.
                let sessions = self.sessions.read().await;
                if !sessions.contains_key(&session_id) {
                    return Err(RuntimeDriverError::NotReady {
                        state: RuntimeState::Destroyed,
                    });
                }
                drop(sessions);

                let _gate_guard = self
                    .lock_current_durability_ready_session_mutation_gate(&session_id)
                    .await?;

                let owner = {
                    let sessions = self.sessions.read().await;
                    Arc::clone(
                        &sessions
                            .get(&session_id)
                            .ok_or(RuntimeDriverError::NotReady {
                                state: RuntimeState::Destroyed,
                            })?
                            .tool_visibility_owner,
                    )
                };

                // DSL-first: fire the canonical typed `PublishCommittedVisibleSet`
                // input. The per-phase transitions at `dsl::PublishCommittedVisibleSet*`
                // own the `VisibleSurfacesMatchAppliedStateInvariant`:
                //
                //   * `active_not_behind_staged`
                //   * `equal_revision_requires_equal_active_and_staged_input`
                //   * `active_requested_subset_of_staged_requested`
                //
                // Guard rejections surface as `RuntimeDriverError::ValidationFailed`
                // via `stage_session_dsl_input`, so the hand-written shell
                // pre-checks that previously duplicated these invariants have
                // been deleted — the DSL guard is the single source of truth.
                let previous_dsl_state = match self
                    .stage_session_dsl_input(
                        &session_id,
                        crate::meerkat_machine::dsl::MeerkatMachineInput::PublishCommittedVisibleSet {
                            active_filter: crate::meerkat_machine::dsl::ToolFilter::from(
                                &visibility_state.active_filter,
                            ),
                            staged_filter: crate::meerkat_machine::dsl::ToolFilter::from(
                                &visibility_state.staged_filter,
                            ),
                            active_requested_deferred_names: visibility_state
                                .active_requested_deferred_names
                                .clone(),
                            staged_requested_deferred_names: visibility_state
                                .staged_requested_deferred_names
                                .clone(),
                            active_deferred_authorities: visibility_authorities_for_names(
                                &visibility_state.active_requested_deferred_names,
                                &visibility_state.requested_witnesses,
                            ),
                            staged_deferred_authorities: visibility_authorities_for_names(
                                &visibility_state.staged_requested_deferred_names,
                                &visibility_state.requested_witnesses,
                            ),
                            active_visibility_revision: visibility_state.active_revision,
                            staged_visibility_revision: visibility_state.staged_revision,
                        },
                        "PublishCommittedVisibleSet",
                    )
                    .await
                {
                    Ok(previous) => previous,
                    Err(reason) => {
                        // Stage-first: PublishCommittedVisibleSet is declared
                        // per non-Destroyed phase only — classify a rejection
                        // on a Destroyed binding as the terminal truth.
                        return Err(self
                            .classify_session_dsl_rejection(&session_id, reason)
                            .await);
                    }
                };

                if let Err(err) = owner.replace_visibility_state(*visibility_state.clone()) {
                    self.restore_session_dsl_state(&session_id, previous_dsl_state)
                        .await;
                    return Err(RuntimeDriverError::Internal(err.to_string()));
                }

                Ok(MeerkatMachineCommandResult::VisibilityPublished(
                    *visibility_state,
                ))
            }
            _ => unreachable!("non-session command routed to session handler"),
        }
    }

    /// Arc-requiring session dispatch: handles commands that spawn runtime-owned
    /// background tasks.
    pub(super) async fn execute_meerkat_machine_ensure_session_command(
        self: &Arc<Self>,
        command: MeerkatMachineCommand,
    ) -> Result<MeerkatMachineCommandResult, RuntimeDriverError> {
        match command {
            MeerkatMachineCommand::EnsureSessionWithExecutor {
                session_id,
                executor,
            } => {
                // Stage-first: `inner` stages the generated executor
                // registration claim; the machine rejects it on a Destroyed
                // binding and the inner classification surfaces the terminal
                // `Destroyed` truth. `inner` creates the session entry (if
                // new), holds the per-session mutation gate across the
                // generated registration claim and shell publication, attaches
                // the executor, and spawns the runtime loop.
                // The generated registration claim, durable revival, ops
                // wiring, loop attachment, and startup reconciliation form one
                // owner saga. A surface caller may be cancelled at any await in
                // that sequence; transferring the full command to an
                // independently owned task prevents caller cancellation from
                // dropping a staged registration or a pre-attachment executor.
                let cleanup_spawner = MachineCleanupTaskSpawner::acquire()?;
                let machine = Arc::clone(self);
                cleanup_spawner
                    .spawn(async move {
                        machine
                            .ensure_session_with_executor_inner(session_id, executor)
                            .await
                    })
                    .await
                    .map_err(|error| {
                        RuntimeDriverError::Internal(format!(
                            "owned EnsureSessionWithExecutor saga ended without a result: {error}"
                        ))
                    })??;
                Ok(MeerkatMachineCommandResult::Unit)
            }
            _ => unreachable!("non-ensure-session command routed to arc session handler"),
        }
    }
}
