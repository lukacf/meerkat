//! PersistentRuntimeDriver — wraps EphemeralRuntimeDriver + RuntimeStore.
//!
//! Provides durable-before-ack guarantee: InputState is persisted via
//! RuntimeStore BEFORE returning AcceptOutcome. Delegates state machine
//! logic to the ephemeral driver.

use std::sync::Arc;
use std::sync::RwLock as StdRwLock;

use meerkat_core::BlobStore;
use meerkat_core::lifecycle::core_executor::BoundSessionCommit;
use meerkat_core::lifecycle::{InputId, RunBoundaryReceipt, RunId};

use crate::accept::AcceptOutcome;
use crate::identifiers::LogicalRuntimeId;
use crate::input::{Input, externalize_input_images};
use crate::input_state::{
    InputAbandonReason, InputLifecycleState, InputState, InputStatePersistenceRecord,
    InputStateSeed, StoredInputState,
};
use crate::runtime_event::RuntimeEventEnvelope;
use crate::runtime_state::RuntimeState;
use crate::store::{
    FencedInputStateBatchCasOutcome, InputStateBatchCasImplementationProfile,
    InputStateBatchCasOutcome, MachineLifecycleCommit, PreparedHeadCanonicalProvisionalPromotion,
    PreparedRuntimeSessionCommit, PreparedRuntimeSessionCommitResult,
    PreparedWholeBlobProvisionalPromotion, RecoveryInputStateMutation,
    RuntimeSessionPersistenceProfile, RuntimeStore, RuntimeStoreError, RuntimeStoreWriteFence,
};
use crate::traits::{DestroyReport, RecoveryReport, RuntimeDriver, RuntimeDriverError};

use super::ephemeral::{
    EphemeralDriverRollbackSnapshot, EphemeralRuntimeDriver, SharedIngressDslAuthority,
};

/// Persistent runtime driver — durable InputState via RuntimeStore.
pub struct PersistentRuntimeDriver {
    /// Underlying ephemeral driver for state machine logic.
    inner: EphemeralRuntimeDriver,
    /// Durable store for InputState + receipts.
    store: Arc<dyn RuntimeStore>,
    /// Blob store used to externalize durable input payloads.
    blob_store: Arc<dyn BlobStore>,
    /// Runtime ID for store operations.
    runtime_id: LogicalRuntimeId,
    /// Shared session-entry durability gate. Production registration always
    /// supplies this handle; direct constructor users retain compatibility
    /// rollback behavior but cannot participate in fail-stop rehydration.
    durability_health: Option<crate::meerkat_machine::DurabilityHealthHandle>,
    /// Exact durable writer epoch retained from conditional registration.
    ///
    /// Multi-writer stores never consume this capability. An
    /// `ExclusiveWriterFenced` store must validate this same guard inside each
    /// complete exact-batch write.
    input_state_write_fence: Option<Arc<dyn RuntimeStoreWriteFence>>,
    /// Test-only fault injection: forces the input-state snapshot step of
    /// [`Self::commit_lifecycle_with_rollback`] to fail so tests can pin the
    /// checkpoint-restore contract for that arm.
    #[cfg(test)]
    pub(crate) force_input_snapshot_failure_for_test: bool,
}

enum PreparedProvisionalPromotion {
    WholeBlob(PreparedWholeBlobProvisionalPromotion),
    HeadCanonical(PreparedHeadCanonicalProvisionalPromotion),
}

impl PersistentRuntimeDriver {
    fn prepare_provisional_promotion(
        &self,
        checkpoint: &meerkat_core::RunCheckpointReceipt,
        receipt: &RunBoundaryReceipt,
        owner_session_id: &meerkat_core::types::SessionId,
    ) -> Result<PreparedProvisionalPromotion, RuntimeStoreError> {
        if checkpoint.session_id() != owner_session_id {
            return Err(RuntimeStoreError::SessionKeyMismatch {
                expected: checkpoint.session_id().clone(),
                actual: owner_session_id.clone(),
            });
        }
        if checkpoint.run_id() != &receipt.run_id {
            return Err(RuntimeStoreError::SessionPersistenceAuthorityConflict {
                runtime_id: self.runtime_id.to_string(),
                detail: "provisional promotion receipt run differs from terminal boundary run"
                    .to_string(),
            });
        }
        match self.store.session_persistence_profile() {
            RuntimeSessionPersistenceProfile::WholeBlobV1 if checkpoint.whole_blob().is_some() => {
                PreparedWholeBlobProvisionalPromotion::prepare(checkpoint.clone(), &receipt.run_id)
                    .map(PreparedProvisionalPromotion::WholeBlob)
            }
            RuntimeSessionPersistenceProfile::HeadCanonicalV1
                if checkpoint.head_canonical().is_some() =>
            {
                PreparedHeadCanonicalProvisionalPromotion::prepare(
                    checkpoint.clone(),
                    &receipt.run_id,
                )
                .map(PreparedProvisionalPromotion::HeadCanonical)
            }
            profile => Err(RuntimeStoreError::SessionPersistenceAuthorityConflict {
                runtime_id: self.runtime_id.to_string(),
                detail: format!(
                    "provisional promotion receipt profile {checkpoint:?} cannot commit through {profile}"
                ),
            }),
        }
    }

    fn prepare_success_boundary(
        &self,
        session: Option<BoundSessionCommit>,
        receipt: RunBoundaryReceipt,
        input_updates: Vec<InputStatePersistenceRecord>,
        owner_session_id: meerkat_core::types::SessionId,
    ) -> Result<PreparedRuntimeSessionCommit, RuntimeStoreError> {
        let Some(session) = session else {
            return Ok(PreparedRuntimeSessionCommit::success(
                None,
                receipt,
                input_updates,
                Some(owner_session_id),
            ));
        };
        let Some(checkpoint_receipt) = session.provisional_promotion_receipt().cloned() else {
            return Ok(PreparedRuntimeSessionCommit::success(
                Some(session),
                receipt,
                input_updates,
                Some(owner_session_id),
            ));
        };
        match self.prepare_provisional_promotion(
            &checkpoint_receipt,
            &receipt,
            &owner_session_id,
        )? {
            PreparedProvisionalPromotion::WholeBlob(promotion) => {
                PreparedRuntimeSessionCommit::promote_whole_blob_success(
                    promotion,
                    receipt,
                    input_updates,
                    owner_session_id,
                )
            }
            PreparedProvisionalPromotion::HeadCanonical(promotion) => {
                PreparedRuntimeSessionCommit::promote_head_canonical_success(
                    promotion,
                    receipt,
                    input_updates,
                    owner_session_id,
                )
            }
        }
    }

    fn prepare_machine_terminal_boundary(
        &self,
        session: BoundSessionCommit,
        receipt: RunBoundaryReceipt,
        machine_lifecycle: MachineLifecycleCommit,
        input_updates: Vec<InputStatePersistenceRecord>,
        owner_session_id: meerkat_core::types::SessionId,
    ) -> Result<PreparedRuntimeSessionCommit, RuntimeStoreError> {
        let Some(checkpoint_receipt) = session.provisional_promotion_receipt().cloned() else {
            return Ok(PreparedRuntimeSessionCommit::machine_terminal(
                session,
                receipt,
                machine_lifecycle,
                input_updates,
                owner_session_id,
            ));
        };
        match self.prepare_provisional_promotion(
            &checkpoint_receipt,
            &receipt,
            &owner_session_id,
        )? {
            PreparedProvisionalPromotion::WholeBlob(promotion) => {
                PreparedRuntimeSessionCommit::promote_whole_blob_machine_terminal(
                    promotion,
                    receipt,
                    machine_lifecycle,
                    input_updates,
                    owner_session_id,
                )
            }
            PreparedProvisionalPromotion::HeadCanonical(promotion) => {
                PreparedRuntimeSessionCommit::promote_head_canonical_machine_terminal(
                    promotion,
                    receipt,
                    machine_lifecycle,
                    input_updates,
                    owner_session_id,
                )
            }
        }
    }

    async fn recover_and_prepare_input_mutations(
        &mut self,
        recovered_unregister_progress: Option<&crate::store::MachineUnregisterProgressSnapshot>,
    ) -> Result<
        (
            RecoveryReport,
            crate::store::RecoveryInputSetRevision,
            Vec<RecoveryInputStateMutation>,
        ),
        RuntimeDriverError,
    > {
        let snapshot = self
            .store
            .load_input_states_with_versions(&self.runtime_id)
            .await
            .map_err(|error| match error {
                crate::store::RuntimeStoreError::Unsupported(reason) => {
                    RuntimeDriverError::RecoveryRepairBlocked {
                        evidence_digest: None,
                        reason: format!(
                            "runtime store cannot produce an exact recovery input-set witness: \
                             {reason}"
                        ),
                    }
                }
                other => RuntimeDriverError::RecoveryBackoff {
                    reason: format!("failed to observe durable inputs for recovery: {other}"),
                },
            })?;
        if snapshot.runtime_id() != &self.runtime_id {
            return Err(RuntimeDriverError::RecoveryCorruption {
                reason: format!(
                    "runtime store prepared recovery input-set evidence for `{}` while \
                     recovering `{}`",
                    snapshot.runtime_id(),
                    self.runtime_id
                ),
            });
        }
        let (rows, input_set_revision, exact_set_token) = snapshot.into_parts();
        let mut observed = Vec::with_capacity(rows.len());
        let mut exact_observations = Vec::with_capacity(rows.len());
        for (bundle, row_digest) in rows {
            let disposition =
                crate::meerkat_machine::driver::machine_classify_recovered_input_durability(
                    &bundle.state,
                )?;
            exact_observations.push((bundle.state.input_id.clone(), row_digest, disposition));
            observed.push(bundle);
        }
        // Terminal rows are outside the recovery nonterminal set by design,
        // but unfinished completion/publication carriers must still be
        // rehydrated so their exact durable saga can converge after restart.
        // The store-owned input-set revision advances for every input-row
        // mutation, including these terminal rows, so the final recovery CAS
        // still fences this second indexed observation without hashing or
        // rescanning historical terminal rows.
        let pending_terminal = self.durable_pending_terminal_input_states().await?;
        let mut observed_ids = observed
            .iter()
            .map(|stored| stored.state.input_id.clone())
            .collect::<std::collections::HashSet<_>>();
        for stored in pending_terminal {
            if !observed_ids.insert(stored.state.input_id.clone()) {
                return Err(RuntimeDriverError::RecoveryCorruption {
                    reason: format!(
                        "input {} appeared in both nonterminal recovery and pending-terminal \
                         observations",
                        stored.state.input_id
                    ),
                });
            }
            observed.push(stored);
        }

        let report = crate::meerkat_machine::machine_recover_persistent_inputs_from_observed(
            self.store.as_ref(),
            &self.runtime_id,
            &mut self.inner,
            observed,
            recovered_unregister_progress,
        )
        .await?;

        let mut mutations = Vec::with_capacity(exact_observations.len());
        for (input_id, row_digest, disposition) in exact_observations {
            if matches!(
                disposition,
                crate::meerkat_machine::dsl::RecoveredInputRecoveryDisposition::Discard
            ) {
                mutations.push(
                    RecoveryInputStateMutation::delete(input_id, row_digest).map_err(|error| {
                        RuntimeDriverError::RecoveryCorruption {
                            reason: format!(
                                "machine-authorized recovery delete lost its exact row witness: \
                                 {error}"
                            ),
                        }
                    })?,
                );
                continue;
            }

            let record = self
                .inner
                .authorized_stored_input_state(&input_id)?
                .ok_or_else(|| RuntimeDriverError::RecoveryCorruption {
                    reason: format!(
                        "recovered durable input {input_id} is absent from machine authority"
                    ),
                })?
                .with_expected_row_digest(row_digest);
            mutations.push(RecoveryInputStateMutation::Upsert(record));
        }
        tracing::debug!(
            runtime_id = %self.runtime_id,
            recovery_input_set_token = %exact_set_token,
            recovery_input_mutations = mutations.len(),
            "prepared exact revision-fenced cold input recovery"
        );
        Ok((report, input_set_revision, mutations))
    }

    pub(crate) async fn recover_inputs_after_runtime_authority(
        &mut self,
        recovered_unregister_progress: Option<&crate::store::MachineUnregisterProgressSnapshot>,
    ) -> Result<RecoveryReport, RuntimeDriverError> {
        match self.store.input_state_batch_cas_implementation_profile() {
            InputStateBatchCasImplementationProfile::MultiWriter => {}
            InputStateBatchCasImplementationProfile::ExclusiveWriterFenced => {
                return Err(RuntimeDriverError::RecoveryRepairBlocked {
                    evidence_digest: None,
                    reason: "exclusive-writer input recovery requires conditional registration \
                             with a durable write fence"
                        .to_string(),
                });
            }
            InputStateBatchCasImplementationProfile::Unsupported => {
                return Err(RuntimeDriverError::RecoveryRepairBlocked {
                    evidence_digest: None,
                    reason: "runtime store does not implement exact input-state batch CAS"
                        .to_string(),
                });
            }
        }

        let (report, input_set_revision, mutations) = self
            .recover_and_prepare_input_mutations(recovered_unregister_progress)
            .await?;

        match self
            .store
            .compare_and_swap_recovery_input_states_atomically(
                &self.runtime_id,
                input_set_revision,
                &mutations,
            )
            .await
            .map_err(|err| RuntimeDriverError::RecoveryBackoff {
                reason: format!("recovered input exact-batch CAS failed: {err}"),
            })? {
            InputStateBatchCasOutcome::Swapped => Ok(report),
            InputStateBatchCasOutcome::Stale => Err(RuntimeDriverError::RecoveryBackoff {
                reason: "durable input state changed while cold recovery was preparing".to_string(),
            }),
        }
    }

    /// Recover durable input work and publish the normalized target image only
    /// while both the original input rows and the caller's external authority
    /// fence remain current.
    pub(crate) async fn recover_inputs_after_runtime_authority_with_fence(
        &mut self,
        recovered_unregister_progress: Option<&crate::store::MachineUnregisterProgressSnapshot>,
        write_fence: Arc<dyn RuntimeStoreWriteFence>,
    ) -> Result<RecoveryReport, RuntimeDriverError> {
        let (report, input_set_revision, mutations) = self
            .recover_and_prepare_input_mutations(recovered_unregister_progress)
            .await?;

        match self.store.input_state_batch_cas_implementation_profile() {
            InputStateBatchCasImplementationProfile::MultiWriter => {
                match self
                    .store
                    .compare_and_swap_recovery_input_states_atomically(
                        &self.runtime_id,
                        input_set_revision,
                        &mutations,
                    )
                    .await
                    .map_err(|error| RuntimeDriverError::RecoveryBackoff {
                        reason: format!("recovered input exact-batch CAS failed: {error}"),
                    })? {
                    InputStateBatchCasOutcome::Swapped => Ok(report),
                    InputStateBatchCasOutcome::Stale => Err(RuntimeDriverError::RecoveryBackoff {
                        reason: "durable input state changed while cold recovery was preparing"
                            .to_string(),
                    }),
                }
            }
            InputStateBatchCasImplementationProfile::ExclusiveWriterFenced => {
                match self
                    .store
                    .compare_and_swap_recovery_input_states_atomically_with_fence(
                        &self.runtime_id,
                        input_set_revision,
                        &mutations,
                        write_fence,
                    )
                    .await
                    .map_err(|error| match error {
                        crate::store::RuntimeStoreError::Unsupported(reason) => {
                            RuntimeDriverError::RecoveryRepairBlocked {
                                evidence_digest: None,
                                reason: format!(
                                    "runtime store lacks fenced input recovery capability: {reason}"
                                ),
                            }
                        }
                        other => RuntimeDriverError::RecoveryBackoff {
                            reason: format!("fenced recovered input persistence failed: {other}"),
                        },
                    })? {
                    FencedInputStateBatchCasOutcome::Swapped => Ok(report),
                    FencedInputStateBatchCasOutcome::Stale => {
                        Err(RuntimeDriverError::StaleAuthority {
                            reason: "durable input state changed while cold recovery was preparing"
                                .to_string(),
                        })
                    }
                    FencedInputStateBatchCasOutcome::FenceConflict { reason } => {
                        Err(RuntimeDriverError::StaleAuthority { reason })
                    }
                    FencedInputStateBatchCasOutcome::FenceBackoff { reason } => {
                        Err(RuntimeDriverError::RecoveryBackoff { reason })
                    }
                }
            }
            InputStateBatchCasImplementationProfile::Unsupported => {
                Err(RuntimeDriverError::RecoveryRepairBlocked {
                    evidence_digest: None,
                    reason: "runtime store does not implement exact input-state batch CAS"
                        .to_string(),
                })
            }
        }
    }

    /// Create a new persistent runtime driver.
    pub fn new(
        runtime_id: LogicalRuntimeId,
        store: Arc<dyn RuntimeStore>,
        blob_store: Arc<dyn BlobStore>,
    ) -> Self {
        Self::new_with_control(
            runtime_id,
            store,
            blob_store,
            Arc::new(StdRwLock::new(
                crate::driver::ephemeral::RuntimeControlProjection::default(),
            )),
            crate::driver::ephemeral::new_ingress_dsl_authority(),
        )
    }

    pub(crate) fn new_with_control(
        runtime_id: LogicalRuntimeId,
        store: Arc<dyn RuntimeStore>,
        blob_store: Arc<dyn BlobStore>,
        control: Arc<StdRwLock<crate::driver::ephemeral::RuntimeControlProjection>>,
        dsl: SharedIngressDslAuthority,
    ) -> Self {
        Self {
            inner: EphemeralRuntimeDriver::new_with_control_and_dsl(
                runtime_id.clone(),
                control,
                dsl,
            ),
            store,
            blob_store,
            runtime_id,
            durability_health: None,
            input_state_write_fence: None,
            #[cfg(test)]
            force_input_snapshot_failure_for_test: false,
        }
    }

    pub(crate) fn new_with_control_and_durability_health(
        runtime_id: LogicalRuntimeId,
        store: Arc<dyn RuntimeStore>,
        blob_store: Arc<dyn BlobStore>,
        control: Arc<StdRwLock<crate::driver::ephemeral::RuntimeControlProjection>>,
        dsl: SharedIngressDslAuthority,
        durability_health: crate::meerkat_machine::DurabilityHealthHandle,
    ) -> Self {
        Self {
            inner: EphemeralRuntimeDriver::new_with_control_and_dsl(
                runtime_id.clone(),
                control,
                dsl,
            ),
            store,
            blob_store,
            runtime_id,
            durability_health: Some(durability_health),
            input_state_write_fence: None,
            #[cfg(test)]
            force_input_snapshot_failure_for_test: false,
        }
    }

    pub(crate) fn set_input_state_write_fence(
        &mut self,
        write_fence: Arc<dyn RuntimeStoreWriteFence>,
    ) {
        self.input_state_write_fence = Some(write_fence);
    }

    pub(crate) fn require_durability_ready(&self) -> Result<(), RuntimeDriverError> {
        match self.durability_health.as_ref() {
            Some(health) => health.require_ready().map_err(|required| {
                RuntimeDriverError::RecoveryRepairBlocked {
                    evidence_digest: None,
                    reason: required.to_string(),
                }
            }),
            None => Ok(()),
        }
    }

    /// Clone the shared fail-closed handle for a cancellation guard that must
    /// outlive a borrow of this driver across an async durable commit.
    pub(crate) fn durability_health_handle(
        &self,
    ) -> Option<crate::meerkat_machine::DurabilityHealthHandle> {
        self.durability_health.clone()
    }

    /// Degrade this production persistent shell after a transition or durable
    /// commit can no longer be reconciled in place. The shared session gate
    /// retains the first failure and refuses every later ordinary mutation
    /// until registration cold-loads a fresh driver.
    pub(crate) fn mark_durability_reload_required(
        &self,
        operation: &'static str,
        reason: impl Into<String>,
    ) -> RuntimeDriverError {
        let reason = reason.into();
        if let Some(health) = self.durability_health.as_ref() {
            health.mark_reload_required(operation, reason.clone());
            RuntimeDriverError::RecoveryRepairBlocked {
                evidence_digest: None,
                reason: format!(
                    "durable state may differ from the live runtime after `{operation}`; \
                     registration-authorized cold reload is required: {reason}"
                ),
            }
        } else {
            RuntimeDriverError::Internal(reason)
        }
    }

    fn persistence_rollback_checkpoint(&self) -> Option<EphemeralDriverRollbackSnapshot> {
        self.durability_health
            .is_none()
            .then(|| self.inner.rollback_snapshot())
    }

    fn restore_compatibility_checkpoint(
        &mut self,
        checkpoint: Option<EphemeralDriverRollbackSnapshot>,
    ) {
        if let Some(checkpoint) = checkpoint {
            self.inner.restore_rollback_snapshot(checkpoint);
        }
    }

    fn post_transition_failure(
        &mut self,
        checkpoint: Option<EphemeralDriverRollbackSnapshot>,
        operation: &'static str,
        reason: impl Into<String>,
    ) -> RuntimeDriverError {
        let reason = reason.into();
        if self.durability_health.is_some() {
            self.mark_durability_reload_required(operation, reason)
        } else {
            self.restore_compatibility_checkpoint(checkpoint);
            RuntimeDriverError::Internal(reason)
        }
    }

    pub(crate) fn input_state_batch_cas_implementation_profile(
        &self,
    ) -> InputStateBatchCasImplementationProfile {
        self.store.input_state_batch_cas_implementation_profile()
    }

    pub(crate) fn input_state_write_fence(&self) -> Option<Arc<dyn RuntimeStoreWriteFence>> {
        self.input_state_write_fence.clone()
    }

    async fn durable_idempotency_duplicate(
        &self,
        input: &Input,
    ) -> Result<Option<(InputId, InputStateSeed)>, RuntimeDriverError> {
        let Some(key) = input.header().idempotency_key.as_ref() else {
            return Ok(None);
        };
        let observation = self
            .store
            .load_input_state_by_idempotency_key(&self.runtime_id, key)
            .await
            .map_err(|error| match error {
                crate::store::RuntimeStoreError::Unsupported(reason) => {
                    RuntimeDriverError::RecoveryRepairBlocked {
                        evidence_digest: None,
                        reason: format!(
                            "persistent idempotency admission requires the exact store-owned \
                             index: {reason}"
                        ),
                    }
                }
                error @ crate::store::RuntimeStoreError::InputIdempotencyIndexUncertain {
                    ..
                } => RuntimeDriverError::RecoveryRepairBlocked {
                    evidence_digest: None,
                    reason: format!(
                        "persistent idempotency admission found durable index corruption: {error}"
                    ),
                },
                other => RuntimeDriverError::Internal(format!(
                    "persistent idempotency admission lookup failed: {other}"
                )),
            })?;
        let Some(observation) = observation else {
            return Ok(None);
        };
        let (stored, _exact_row_digest) = observation.into_parts();
        if stored.state.idempotency_key.as_ref() != Some(key) {
            return Err(RuntimeDriverError::RecoveryCorruption {
                reason: format!(
                    "store idempotency index for key `{key}` returned input {} with a different \
                     key",
                    stored.state.input_id
                ),
            });
        }
        Ok(Some((stored.state.input_id, stored.seed)))
    }

    /// Get immutable reference to the inner ephemeral driver.
    pub fn inner_ref(&self) -> &EphemeralRuntimeDriver {
        &self.inner
    }

    pub(crate) fn inner_mut(&mut self) -> &mut EphemeralRuntimeDriver {
        &mut self.inner
    }

    #[cfg(test)]
    pub(crate) async fn compare_and_swap_interaction_terminal_outbox_inputs(
        &self,
        expected: &[StoredInputState],
        input_ids: &[InputId],
    ) -> Result<InputStateBatchCasOutcome, RuntimeDriverError> {
        let mut replacements = Vec::with_capacity(input_ids.len());
        for input_id in input_ids {
            let replacement = self
                .inner
                .authorized_stored_input_state(input_id)?
                .ok_or_else(|| {
                    RuntimeDriverError::Internal(format!(
                        "interaction terminal outbox input {input_id} disappeared before compare-and-swap"
                    ))
                })?;
            replacements.push(replacement);
        }
        self.compare_and_swap_interaction_terminal_outbox_replacements(expected, &replacements)
            .await
    }

    pub(crate) async fn compare_and_swap_interaction_terminal_outbox_replacements(
        &self,
        expected: &[StoredInputState],
        replacements: &[crate::input_state::InputStatePersistenceRecord],
    ) -> Result<InputStateBatchCasOutcome, RuntimeDriverError> {
        self.require_durability_ready()?;
        match self.store.input_state_batch_cas_implementation_profile() {
            InputStateBatchCasImplementationProfile::MultiWriter => self
                .store
                .compare_and_swap_input_states_atomically(&self.runtime_id, expected, replacements)
                .await
                .map_err(|error| {
                    self.mark_durability_reload_required(
                        "interaction_terminal_batch_cas",
                        format!(
                            "multi-writer input-state batch compare-and-swap outcome is unknown: \
                             {error}"
                        ),
                    )
                }),
            InputStateBatchCasImplementationProfile::ExclusiveWriterFenced => {
                let write_fence = self.input_state_write_fence.clone().ok_or_else(|| {
                    self.mark_durability_reload_required(
                        "interaction_terminal_batch_cas_fence",
                        "exclusive-writer input-state CAS has no durable registration fence",
                    )
                })?;
                match self
                    .store
                    .compare_and_swap_input_states_atomically_with_fence(
                        &self.runtime_id,
                        expected,
                        replacements,
                        write_fence,
                    )
                    .await
                    .map_err(|error| {
                        self.mark_durability_reload_required(
                            "interaction_terminal_fenced_batch_cas",
                            format!(
                                "fenced input-state batch compare-and-swap outcome is unknown: \
                                 {error}"
                            ),
                        )
                    })? {
                    FencedInputStateBatchCasOutcome::Swapped => {
                        Ok(InputStateBatchCasOutcome::Swapped)
                    }
                    FencedInputStateBatchCasOutcome::Stale => Ok(InputStateBatchCasOutcome::Stale),
                    FencedInputStateBatchCasOutcome::FenceConflict { reason } => Err(self
                        .mark_durability_reload_required(
                            "interaction_terminal_batch_cas_fence_conflict",
                            reason,
                        )),
                    FencedInputStateBatchCasOutcome::FenceBackoff { reason } => {
                        Err(RuntimeDriverError::RecoveryBackoff { reason })
                    }
                }
            }
            InputStateBatchCasImplementationProfile::Unsupported => {
                Err(RuntimeDriverError::RecoveryRepairBlocked {
                    evidence_digest: None,
                    reason: "runtime store does not implement exact input-state batch CAS"
                        .to_string(),
                })
            }
        }
    }

    /// Release terminal live state after its exact completion/publication CAS
    /// has committed. The ephemeral helper rechecks that every named row is
    /// terminal and carries no open durable obligation; any archive mismatch
    /// degrades the shared shell rather than continuing with split authority.
    pub(crate) fn archive_terminal_inputs_after_durable_obligations(
        &mut self,
        input_ids: &[InputId],
    ) -> Result<(), RuntimeDriverError> {
        self.require_durability_ready()?;
        let archivable = match self.inner.archivable_terminal_input_ids_in(input_ids) {
            Ok(archivable) if archivable.len() == input_ids.len() => archivable,
            Ok(archivable) => {
                return Err(self.post_transition_failure(
                    None,
                    "terminal_obligation_archive_classification",
                    format!(
                        "only {} of {} exact terminal-obligation inputs were durably quiescent",
                        archivable.len(),
                        input_ids.len()
                    ),
                ));
            }
            Err(error) => {
                return Err(self.post_transition_failure(
                    None,
                    "terminal_obligation_archive_classification",
                    error.to_string(),
                ));
            }
        };
        if let Err(error) = self
            .inner
            .archive_archivable_terminal_inputs_after_durable_commit(&archivable)
        {
            return Err(self.post_transition_failure(
                None,
                "terminal_obligation_archive",
                error.to_string(),
            ));
        }
        Ok(())
    }

    pub(crate) async fn committed_session_snapshot_for_terminal_recovery(
        &self,
    ) -> Result<Option<Arc<Vec<u8>>>, RuntimeDriverError> {
        self.store
            .load_session_snapshot(&self.runtime_id)
            .await
            .map_err(|error| {
                RuntimeDriverError::Internal(format!(
                    "interaction terminal recovery failed to load committed session snapshot: {error}"
                ))
            })
    }

    pub(crate) async fn pending_terminal_owner_ids(
        &self,
    ) -> Result<Vec<InputId>, RuntimeDriverError> {
        let mut owners = Vec::new();
        let mut after = None;
        loop {
            let page = self
                .store
                .load_pending_terminal_owner_ids_page(
                    &self.runtime_id,
                    after.as_ref(),
                    crate::store::MAX_PENDING_TERMINAL_OWNER_PAGE,
                )
                .await
                .map_err(|error| match error {
                    crate::store::RuntimeStoreError::Unsupported(reason) => {
                        RuntimeDriverError::RecoveryRepairBlocked {
                            evidence_digest: None,
                            reason: format!(
                                "runtime store cannot discover pending terminal owners: {reason}"
                            ),
                        }
                    }
                    other => RuntimeDriverError::Internal(format!(
                        "pending terminal owner discovery failed: {other}"
                    )),
                })?;
            crate::store::validate_pending_terminal_owner_page(
                after.as_ref(),
                crate::store::MAX_PENDING_TERMINAL_OWNER_PAGE,
                &page,
            )
            .map_err(|error| RuntimeDriverError::RecoveryCorruption {
                reason: error.to_string(),
            })?;
            let short = page.len() < crate::store::MAX_PENDING_TERMINAL_OWNER_PAGE;
            after = page.last().cloned();
            owners.extend(page);
            if short {
                return Ok(owners);
            }
        }
    }

    pub(crate) async fn durable_pending_terminal_input_states(
        &self,
    ) -> Result<Vec<StoredInputState>, RuntimeDriverError> {
        let owners = self.pending_terminal_owner_ids().await?;
        let mut rows = std::collections::HashMap::<InputId, StoredInputState>::new();
        for owner_input_id in owners {
            let mut owner_rows = self
                .store
                .load_input_states_by_ids(&self.runtime_id, std::slice::from_ref(&owner_input_id))
                .await
                .map_err(|error| {
                    RuntimeDriverError::Internal(format!(
                        "pending terminal owner row read failed: {error}"
                    ))
                })?;
            let owner = owner_rows
                .pop()
                .ok_or_else(|| RuntimeDriverError::RecoveryCorruption {
                    reason: "pending terminal owner read returned the wrong cardinality"
                        .to_string(),
                })?
                .ok_or_else(|| RuntimeDriverError::RecoveryCorruption {
                    reason: format!(
                        "pending terminal owner index points to missing input {owner_input_id}"
                    ),
                })?;
            if !crate::store::input_state_is_pending_terminal_owner(&owner.state) {
                return Err(RuntimeDriverError::RecoveryCorruption {
                    reason: format!(
                        "pending terminal owner index points to non-owner input {owner_input_id}"
                    ),
                });
            }

            let mut recipient_ids = Vec::new();
            if let Some(completion) = owner.state.terminal_completion.as_ref() {
                let completion_owner = if completion.owner_input_id == owner_input_id {
                    owner.clone()
                } else {
                    self.store
                        .load_input_state(&self.runtime_id, &completion.owner_input_id)
                        .await
                        .map_err(|error| {
                            RuntimeDriverError::Internal(format!(
                                "pending terminal completion owner read failed: {error}"
                            ))
                        })?
                        .ok_or_else(|| RuntimeDriverError::RecoveryCorruption {
                            reason: format!(
                                "pending terminal owner {owner_input_id} points to missing completion owner {}",
                                completion.owner_input_id
                            ),
                        })?
                };
                recipient_ids.extend(
                    completion_owner
                        .state
                        .terminal_completion
                        .as_ref()
                        .ok_or_else(|| RuntimeDriverError::RecoveryCorruption {
                            reason: format!(
                                "pending terminal completion owner {} lost its completion row",
                                completion.owner_input_id
                            ),
                        })?
                        .completion_input_ids
                        .as_ref()
                        .ok_or_else(|| RuntimeDriverError::RecoveryCorruption {
                            reason: format!(
                                "pending terminal completion owner {owner_input_id} lost recipients"
                            ),
                        })?
                        .iter()
                        .cloned(),
                );
            }
            recipient_ids.sort_by_key(|input_id| input_id.0);
            recipient_ids.dedup();
            if recipient_ids.is_empty()
                || recipient_ids.len() > crate::store::MAX_INPUT_STATE_BATCH_CAS
            {
                return Err(RuntimeDriverError::RecoveryCorruption {
                    reason: format!(
                        "pending terminal owner {owner_input_id} declares an invalid recipient set"
                    ),
                });
            }
            let recipient_rows = self
                .store
                .load_input_states_by_ids(&self.runtime_id, &recipient_ids)
                .await
                .map_err(|error| {
                    RuntimeDriverError::Internal(format!(
                        "pending terminal recipient batch read failed: {error}"
                    ))
                })?;
            if recipient_rows.len() != recipient_ids.len() {
                return Err(RuntimeDriverError::RecoveryCorruption {
                    reason: "pending terminal recipient read returned the wrong cardinality"
                        .to_string(),
                });
            }
            for (input_id, row) in recipient_ids.into_iter().zip(recipient_rows) {
                let row = row.ok_or_else(|| RuntimeDriverError::RecoveryCorruption {
                    reason: format!(
                        "pending terminal owner {owner_input_id} points to missing recipient {input_id}"
                    ),
                })?;
                rows.insert(input_id, row);
            }
        }
        let mut rows = rows.into_values().collect::<Vec<_>>();
        rows.sort_by_key(|row| row.state.input_id.0);
        Ok(rows)
    }

    /// Get the logical runtime ID for this driver.
    pub fn runtime_id(&self) -> &LogicalRuntimeId {
        &self.runtime_id
    }

    pub(crate) fn session_persistence_profile(
        &self,
    ) -> crate::store::RuntimeSessionPersistenceProfile {
        self.store.session_persistence_profile()
    }

    pub(crate) async fn load_pending_compaction_projections(
        &self,
    ) -> Result<Vec<meerkat_core::CompactionProjectionIntent>, RuntimeDriverError> {
        self.store
            .load_pending_compaction_projections(&self.runtime_id)
            .await
            .map_err(|error| {
                RuntimeDriverError::Internal(format!(
                    "failed to load compaction projection outbox: {error}"
                ))
            })
    }

    pub(crate) async fn mark_compaction_projection_finalized(
        &self,
        projection: &meerkat_core::CompactionProjectionId,
    ) -> Result<(), RuntimeDriverError> {
        self.store
            .mark_compaction_projection_finalized(&self.runtime_id, projection)
            .await
            .map_err(|error| {
                RuntimeDriverError::Internal(format!(
                    "failed to finalize compaction projection outbox: {error}"
                ))
            })
    }

    pub(crate) async fn load_compaction_checkpoint_snapshot(
        &self,
    ) -> Result<Option<Arc<Vec<u8>>>, RuntimeDriverError> {
        self.store
            .load_session_snapshot(&self.runtime_id)
            .await
            .map_err(|error| {
                RuntimeDriverError::Internal(format!(
                    "failed to load authoritative compaction checkpoint snapshot: {error}"
                ))
            })
    }

    pub(crate) async fn commit_compaction_checkpoint_snapshot(
        &self,
        session_snapshot: Arc<Vec<u8>>,
    ) -> Result<(), RuntimeDriverError> {
        self.store
            .commit_session_snapshot(
                &self.runtime_id,
                crate::store::SerializedSessionSnapshot { session_snapshot },
            )
            .await
            .map_err(|error| {
                RuntimeDriverError::Internal(format!(
                    "failed to prepare authoritative compaction checkpoint snapshot: {error}"
                ))
            })
    }

    pub fn silent_comms_intents(&self) -> Vec<String> {
        self.inner.silent_comms_intents()
    }

    /// Check if the runtime is idle (delegates to inner).
    pub fn is_idle(&self) -> bool {
        self.inner.is_idle()
    }

    /// Ask generated MeerkatMachine authority for the store-visible lifecycle.
    fn runtime_state_for_persistence(&self) -> Result<RuntimeState, RuntimeDriverError> {
        Self::runtime_state_for_persistence_from_inner(&self.inner)
    }

    fn runtime_state_for_persistence_from_inner(
        inner: &EphemeralRuntimeDriver,
    ) -> Result<RuntimeState, RuntimeDriverError> {
        crate::meerkat_machine::classify_runtime_lifecycle_durable_state_with_pre_run_phase(
            inner.runtime_state(),
            inner.pre_run_phase(),
        )
        .map_err(|err| {
            RuntimeDriverError::Internal(format!(
                "generated runtime lifecycle durability classification failed: {err}"
            ))
        })
    }

    fn lifecycle_commit_for_persistence(
        &self,
    ) -> Result<MachineLifecycleCommit, RuntimeDriverError> {
        Self::lifecycle_commit_for_persistence_from_inner(&self.inner)
    }

    fn lifecycle_commit_for_persistence_with_supervisor_authority(
        &self,
        supervisor_authority: crate::store::SupervisorAuthoritySnapshot,
    ) -> Result<MachineLifecycleCommit, RuntimeDriverError> {
        Ok(
            MachineLifecycleCommit::new_with_binding_run_unregister_progress_and_live_bridge(
                Self::runtime_state_for_persistence_from_inner(&self.inner)?,
                self.inner.machine_lifecycle_binding_facts(),
                crate::store::MachineLifecycleRunFacts::default(),
                supervisor_authority,
                Self::unregister_progress_for_persistence_from_inner(&self.inner),
                Self::live_bridge_recovery_for_persistence_from_inner(&self.inner)?,
            ),
        )
    }

    fn lifecycle_commit_for_persistence_from_inner(
        inner: &EphemeralRuntimeDriver,
    ) -> Result<MachineLifecycleCommit, RuntimeDriverError> {
        Ok(
            MachineLifecycleCommit::new_with_binding_run_unregister_progress_and_live_bridge(
                Self::runtime_state_for_persistence_from_inner(inner)?,
                inner.machine_lifecycle_binding_facts(),
                crate::store::MachineLifecycleRunFacts::default(),
                inner.supervisor_authority_snapshot(),
                Self::unregister_progress_for_persistence_from_inner(inner),
                Self::live_bridge_recovery_for_persistence_from_inner(inner)?,
            ),
        )
    }

    /// Project a committed final `UnregisterSession` for durable storage.
    ///
    /// The live entry deliberately keeps `registration_phase = Draining` as a
    /// same-process rematerialization tombstone until exact entry removal. That
    /// mechanical fence is not durable unregister progress: final generated
    /// authority has cleared the session binding and all drain obligations, so
    /// persisting a progress row would make a later process replay a completed
    /// teardown and reject fresh registration.
    fn lifecycle_commit_for_completed_unregister(
        &self,
    ) -> Result<MachineLifecycleCommit, RuntimeDriverError> {
        let completed = {
            let authority = self.inner.shared_dsl_authority();
            let authority = authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let state = authority.state();
            state.registration_phase == crate::meerkat_machine::dsl::RegistrationPhase::Draining
                && state.session_id.is_none()
                && state.active_runtime_id.is_none()
                && state.active_fence_token.is_none()
                && state.active_runtime_generation.is_none()
                && state.active_runtime_epoch_id.is_none()
                && !state.unregister_runtime_loop_drain_pending
                && !state.unregister_comms_drain_exit_pending
                && !state.unregister_completion_waiter_drain_pending
        };
        if !completed {
            return Err(RuntimeDriverError::Internal(
                "completed unregister persistence requires the generated final lifecycle image"
                    .to_string(),
            ));
        }
        Ok(
            MachineLifecycleCommit::new_with_binding_unregister_progress_and_live_bridge(
                Self::runtime_state_for_persistence_from_inner(&self.inner)?,
                self.inner.machine_lifecycle_binding_facts(),
                self.inner.supervisor_authority_snapshot(),
                None,
                Self::live_bridge_recovery_for_persistence_from_inner(&self.inner)?,
            ),
        )
    }

    fn unregister_progress_for_persistence_from_inner(
        inner: &EphemeralRuntimeDriver,
    ) -> Option<crate::store::MachineUnregisterProgressSnapshot> {
        let authority = inner.shared_dsl_authority();
        let authority = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let state = authority.state();
        (state.registration_phase == crate::meerkat_machine::dsl::RegistrationPhase::Draining).then(
            || {
                crate::store::MachineUnregisterProgressSnapshot::new(
                    state.unregister_runtime_loop_drain_pending,
                    state.unregister_comms_drain_exit_pending,
                    state.unregister_completion_waiter_drain_pending,
                    state.unregister_runtime_loop_forced_abort,
                    state.unregister_comms_drain_forced_abort,
                )
            },
        )
    }

    fn live_bridge_recovery_for_persistence_from_inner(
        inner: &EphemeralRuntimeDriver,
    ) -> Result<crate::live_execution::LiveBridgeRecoveryImage, RuntimeDriverError> {
        let authority = inner.shared_dsl_authority();
        let authority = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        crate::live_execution::LiveBridgeRecoveryImage::capture(authority.state()).map_err(
            |reason| {
                RuntimeDriverError::Internal(format!(
                    "generated live bridge recovery image is invalid: {reason}"
                ))
            },
        )
    }

    /// Snapshot + classify the lifecycle persistence payload, restoring the
    /// caller's checkpoint on failure.
    ///
    /// Contract (Dogma K11): every fallible step between a staged `&mut` DSL
    /// transition and the rollback-guarded durable commit restores the
    /// caller's checkpoint. A bare `?` here would leave the staged lifecycle
    /// live in driver state while reporting failure to the caller. The
    /// checkpoint is returned on success so the durable commit arm can keep
    /// using it.
    fn lifecycle_persistence_payload_with_rollback(
        &mut self,
        checkpoint: Option<super::ephemeral::EphemeralDriverRollbackSnapshot>,
        changed_input_ids: &[InputId],
        context: &str,
    ) -> Result<
        (
            Option<super::ephemeral::EphemeralDriverRollbackSnapshot>,
            Vec<InputStatePersistenceRecord>,
            MachineLifecycleCommit,
        ),
        RuntimeDriverError,
    > {
        if let Err(err) = self
            .inner
            .retire_durably_quiescent_terminal_payloads_in(changed_input_ids)
        {
            return Err(self.post_transition_failure(
                checkpoint,
                "terminal_payload_retirement",
                format!("{context} terminal payload retirement failed: {err}"),
            ));
        }
        let input_states_result = self
            .inner
            .authorized_stored_input_states_for_ids(changed_input_ids);
        #[cfg(test)]
        let input_states_result = if self.force_input_snapshot_failure_for_test {
            Err(RuntimeDriverError::Internal(
                "forced input-state snapshot failure for checkpoint-restore contract test"
                    .to_string(),
            ))
        } else {
            input_states_result
        };
        let input_states = match input_states_result {
            Ok(input_states) => input_states,
            Err(err) => {
                return Err(self.post_transition_failure(
                    checkpoint,
                    "input_state_materialization",
                    format!("{context} input-state snapshot failed: {err}"),
                ));
            }
        };
        let commit = match self.lifecycle_commit_for_persistence() {
            Ok(commit) => commit,
            Err(err) => {
                return Err(self.post_transition_failure(
                    checkpoint,
                    "lifecycle_commit_classification",
                    format!("{context} lifecycle commit classification failed: {err}"),
                ));
            }
        };
        Ok((checkpoint, input_states, commit))
    }

    async fn commit_lifecycle_with_rollback(
        &mut self,
        checkpoint: Option<super::ephemeral::EphemeralDriverRollbackSnapshot>,
        changed_input_ids: &[InputId],
        target_state: RuntimeState,
        context: &str,
    ) -> Result<(), RuntimeDriverError> {
        // Contract: every fallible step between the staged DSL transition and
        // the durable commit restores the caller's checkpoint on failure. A
        // bare `?` here would leave the staged lifecycle (e.g. Destroy) live
        // in driver state while reporting failure to the caller.
        let (checkpoint, input_states, commit) = self.lifecycle_persistence_payload_with_rollback(
            checkpoint,
            changed_input_ids,
            context,
        )?;
        let target_durable_state =
            match crate::meerkat_machine::classify_runtime_lifecycle_durable_state_with_pre_run_phase(
                target_state,
                self.inner.pre_run_phase(),
            ) {
                Ok(target_durable_state) => target_durable_state,
                Err(err) => {
                    return Err(self.post_transition_failure(
                        checkpoint,
                        "lifecycle_target_classification",
                        format!(
                            "{context} generated target lifecycle durability classification failed: {err}"
                        ),
                    ));
                }
            };
        if commit.runtime_state() != target_durable_state {
            return Err(self.post_transition_failure(
                checkpoint,
                "lifecycle_target_validation",
                format!(
                    "{context} durable persist target {target_durable_state:?} from live \
                     {target_state:?} disagreed with generated lifecycle commit {:?}",
                    commit.runtime_state()
                ),
            ));
        }
        if let Err(err) = self
            .store
            .commit_machine_lifecycle(&self.runtime_id, commit, &input_states)
            .await
        {
            return Err(self.post_transition_failure(
                checkpoint,
                "lifecycle_commit",
                format!("{context} persist failed: {err}"),
            ));
        }
        Ok(())
    }

    pub(crate) async fn publish_service_turn_terminal(
        &mut self,
        checkpoint: Option<super::ephemeral::EphemeralDriverRollbackSnapshot>,
        target_state: RuntimeState,
        session: BoundSessionCommit,
        receipt: meerkat_core::lifecycle::RunBoundaryReceipt,
        owner_session_id: meerkat_core::types::SessionId,
    ) -> Result<PreparedRuntimeSessionCommitResult, RuntimeDriverError> {
        self.require_durability_ready()?;
        let commit = match self.lifecycle_commit_for_persistence() {
            Ok(commit) => commit,
            Err(error) => {
                return Err(self.post_transition_failure(
                    checkpoint,
                    "service_turn_terminal_lifecycle_classification",
                    format!(
                        "service turn terminal receipt lifecycle classification failed: {error}"
                    ),
                ));
            }
        };
        let target_durable_state =
            match crate::meerkat_machine::classify_runtime_lifecycle_durable_state(target_state) {
                Ok(target_durable_state) => target_durable_state,
                Err(error) => {
                    return Err(self.post_transition_failure(
                        checkpoint,
                        "service_turn_terminal_target_classification",
                        format!(
                            "service turn terminal receipt target classification failed: {error}"
                        ),
                    ));
                }
            };
        if commit.runtime_state() != target_durable_state {
            return Err(self.post_transition_failure(
                checkpoint,
                "service_turn_terminal_target_validation",
                format!(
                    "service turn terminal receipt durable target {target_durable_state:?} disagreed with generated lifecycle {:?}",
                    commit.runtime_state()
                ),
            ));
        }
        let promotion = session.provisional_promotion_receipt().cloned();
        let request = match promotion {
            Some(checkpoint_receipt) => {
                match self.prepare_provisional_promotion(
                    &checkpoint_receipt,
                    &receipt,
                    &owner_session_id,
                ) {
                    Ok(PreparedProvisionalPromotion::WholeBlob(promotion)) => {
                        PreparedRuntimeSessionCommit::promote_whole_blob_service_turn_terminal(
                            promotion,
                            receipt,
                            commit,
                            owner_session_id,
                        )
                    }
                    Ok(PreparedProvisionalPromotion::HeadCanonical(promotion)) => {
                        PreparedRuntimeSessionCommit::promote_head_canonical_service_turn_terminal(
                            promotion,
                            receipt,
                            commit,
                            owner_session_id,
                        )
                    }
                    Err(error) => Err(error),
                }
            }
            None => Ok(PreparedRuntimeSessionCommit::service_turn_terminal(
                session,
                receipt,
                commit,
                owner_session_id,
            )),
        };
        let request = match request {
            Ok(request) => request,
            Err(error) => {
                return Err(self.post_transition_failure(
                    checkpoint,
                    "service_turn_terminal_promotion_validation",
                    format!("service turn terminal promotion is invalid: {error}"),
                ));
            }
        };
        let result = match self
            .store
            .commit_prepared_session_boundary(&self.runtime_id, request)
            .await
        {
            Ok(result) => result,
            Err(error) => {
                return Err(self.post_transition_failure(
                    checkpoint,
                    "service_turn_terminal_commit",
                    format!("service turn terminal receipt persist failed: {error}"),
                ));
            }
        };
        self.inner.sync_control_projection_from_dsl_authority();
        Ok(result)
    }

    pub(crate) fn set_control_projection(
        &mut self,
        next_phase: RuntimeState,
        current_run_id: Option<RunId>,
        pre_run_phase: Option<RuntimeState>,
    ) {
        self.inner
            .set_control_projection(next_phase, current_run_id, pre_run_phase);
    }

    /// Low-level control projection shim for external contract tests.
    ///
    /// This does not decide lifecycle legality; it only applies an already
    /// chosen MeerkatMachine control projection to the concrete driver shell.
    pub(crate) fn sync_control_projection_from_dsl_authority(&mut self) {
        self.inner.sync_control_projection_from_dsl_authority();
    }

    pub(crate) async fn persist_current_machine_lifecycle(
        &mut self,
        context: &str,
    ) -> Result<(), RuntimeDriverError> {
        self.require_durability_ready()?;
        let commit = match self.lifecycle_commit_for_persistence() {
            Ok(commit) => commit,
            Err(error) => {
                return Err(self.post_transition_failure(
                    None,
                    "ordinary_lifecycle_classification",
                    format!("{context} lifecycle classification failed: {error}"),
                ));
            }
        };
        if let Err(error) = self
            .store
            .commit_machine_lifecycle(&self.runtime_id, commit, &[])
            .await
        {
            return Err(self.post_transition_failure(
                None,
                "ordinary_lifecycle_commit",
                format!("{context} lifecycle persist failed: {error}"),
            ));
        }
        Ok(())
    }

    /// Explicit teardown/recovery write that is allowed to operate while an
    /// entry is not durability-ready. Callers must already hold the unregister
    /// recovery authority and must not roll a possibly-committed ordinary
    /// transition back through this seam.
    pub(crate) async fn persist_recovery_machine_lifecycle(
        &mut self,
        context: &str,
    ) -> Result<(), RuntimeDriverError> {
        let commit = self.lifecycle_commit_for_persistence()?;
        self.store
            .commit_machine_lifecycle(&self.runtime_id, commit, &[])
            .await
            .map_err(|error| {
                RuntimeDriverError::Internal(format!(
                    "{context} recovery lifecycle persist failed: {error}"
                ))
            })
    }

    pub(crate) async fn commit_unregister_finalization(
        &mut self,
        context: &str,
        retired_ops_epoch: &meerkat_core::RuntimeEpochId,
        authority: crate::meerkat_machine::DeleteOpsFinalizationAuthority,
    ) -> Result<(), RuntimeDriverError> {
        let commit = self.lifecycle_commit_for_completed_unregister()?;
        let finalization = crate::store::UnregisterFinalizationCommit::new(
            commit,
            Vec::new(),
            retired_ops_epoch.clone(),
            authority,
        );
        self.store
            .commit_unregister_finalization(&self.runtime_id, finalization)
            .await
            .map_err(|err| match err {
                crate::store::RuntimeStoreError::UnregisterFinalizationOutcomeUnknown(reason) => {
                    RuntimeDriverError::UnregisterFinalizationOutcomeUnknown {
                        reason: format!("{context} lifecycle+ops finalization: {reason}"),
                    }
                }
                err => RuntimeDriverError::Internal(format!(
                    "{context} lifecycle+ops finalization failed: {err}"
                )),
            })
    }

    pub(crate) async fn persist_completed_unregister_machine_lifecycle(
        &mut self,
        context: &str,
        _authority: crate::meerkat_machine::RetainOpsFinalizationAuthority,
    ) -> Result<(), RuntimeDriverError> {
        let commit = self.lifecycle_commit_for_completed_unregister()?;
        self.store
            .commit_machine_lifecycle(&self.runtime_id, commit, &[])
            .await
            .map_err(|error| {
                // The generic lifecycle commit contract is atomic, but unlike
                // commit_unregister_finalization it does not distinguish a
                // definitely-uncommitted error from a lost acknowledgement.
                // RetainSnapshot finalization must therefore treat every
                // error as ambiguous: rolling local authority back to
                // Draining could overwrite a terminal image that already
                // committed durably.
                RuntimeDriverError::UnregisterFinalizationOutcomeUnknown {
                    reason: format!(
                        "{context} retained lifecycle finalization acknowledgement unavailable: {error}"
                    ),
                }
            })
    }

    /// Persist a previewed closed supervisor projection alongside the current
    /// machine lifecycle. This lets the supervisor saga commit durable truth
    /// before changing the shared live authority, avoiding a whole-authority
    /// rollback across asynchronous store I/O (peer ingress may concurrently
    /// mutate unrelated generated fields).
    pub(crate) async fn persist_current_machine_lifecycle_with_supervisor_authority(
        &mut self,
        context: &str,
        supervisor_authority: crate::store::SupervisorAuthoritySnapshot,
    ) -> Result<(), RuntimeDriverError> {
        self.require_durability_ready()?;
        let commit = match self
            .lifecycle_commit_for_persistence_with_supervisor_authority(supervisor_authority)
        {
            Ok(commit) => commit,
            Err(error) => {
                return Err(self.post_transition_failure(
                    None,
                    "supervisor_lifecycle_classification",
                    format!("{context} lifecycle classification failed: {error}"),
                ));
            }
        };
        if let Err(error) = self
            .store
            .commit_machine_lifecycle(&self.runtime_id, commit, &[])
            .await
        {
            return Err(self.post_transition_failure(
                None,
                "supervisor_lifecycle_commit",
                format!("{context} lifecycle persist failed: {error}"),
            ));
        }
        Ok(())
    }

    /// Contract helper for external tests that need to start a run through the
    /// same DSL authority used by the runtime loop.
    #[doc(hidden)]
    pub fn contract_begin_run_authority(
        &mut self,
        run_id: RunId,
    ) -> Result<(), RuntimeDriverError> {
        self.inner.contract_begin_run_authority(run_id)
    }

    /// Get pending events (delegates to inner).
    pub fn drain_events(&mut self) -> Vec<RuntimeEventEnvelope> {
        self.inner.drain_events()
    }

    /// Drain the typed post-admission signal (delegates to inner).
    pub fn take_post_admission_signal(&mut self) -> crate::driver::ephemeral::PostAdmissionSignal {
        self.inner.take_post_admission_signal()
    }

    /// Inspect the current typed post-admission signal without draining it.
    pub fn post_admission_signal(&self) -> crate::driver::ephemeral::PostAdmissionSignal {
        self.inner.post_admission_signal()
    }

    /// Check and clear wake flag (backward-compat, delegates to inner).
    pub fn take_wake_requested(&mut self) -> bool {
        self.inner.take_wake_requested()
    }

    /// Check and clear immediate processing flag (backward-compat, delegates to inner).
    pub fn take_process_requested(&mut self) -> bool {
        self.inner.take_process_requested()
    }

    /// Contract helper for recovery tests. Production runtime execution must
    /// hydrate payloads through generated batch authority.
    #[cfg(any(test, debug_assertions, feature = "test-support"))]
    #[doc(hidden)]
    pub fn contract_peek_next_for_recovery_tests(&self) -> Option<(InputId, Input)> {
        self.inner.contract_peek_next_for_recovery_tests()
    }

    pub(crate) fn hydrate_authorized_batch(
        &self,
        batch: &crate::meerkat_machine::driver::AuthorizedRuntimeLoopBatch,
    ) -> Result<Vec<(InputId, Input)>, RuntimeDriverError> {
        self.inner.hydrate_authorized_batch(batch)
    }

    pub fn has_queued_input_outside(&self, excluded: &[InputId]) -> bool {
        self.inner.has_queued_input_outside(excluded)
    }

    pub(crate) fn has_queued_input_in_any_lane(&self) -> bool {
        self.inner.has_queued_input_in_any_lane()
    }

    pub(crate) fn defer_queued_inputs_behind_backlog(
        &mut self,
        input_ids: &[InputId],
    ) -> Result<(), RuntimeDriverError> {
        self.inner.defer_queued_inputs_behind_backlog(input_ids)
    }

    pub(crate) fn absorb_post_admission_effects(
        &mut self,
        effects: &[crate::meerkat_machine::dsl::MeerkatMachineEffect],
    ) {
        self.inner.absorb_post_admission_effects(effects);
    }

    pub(crate) fn resolve_admission(
        &self,
        input: &Input,
    ) -> Result<crate::accept::ResolvedAdmission, RuntimeDriverError> {
        self.inner.resolve_admission(input)
    }

    pub(crate) fn resolve_admission_with_active_turn_boundary(
        &self,
        input: &Input,
        active_turn_boundary_available: bool,
    ) -> Result<crate::accept::ResolvedAdmission, RuntimeDriverError> {
        self.inner
            .resolve_admission_with_active_turn_boundary(input, active_turn_boundary_available)
    }

    pub(crate) fn resolve_admission_without_wake_with_active_turn_boundary(
        &self,
        input: &Input,
        active_turn_boundary_available: bool,
    ) -> Result<crate::accept::ResolvedAdmission, RuntimeDriverError> {
        self.inner
            .resolve_admission_without_wake_with_active_turn_boundary(
                input,
                active_turn_boundary_available,
            )
    }

    pub(crate) async fn accept_resolved_input(
        &mut self,
        input: Input,
        resolved: crate::accept::ResolvedAdmission,
    ) -> Result<AcceptOutcome, RuntimeDriverError> {
        self.require_durability_ready()?;
        self.inner.ensure_contract_session_authority()?;
        if let Some((existing_id, existing_seed)) =
            self.durable_idempotency_duplicate(&input).await?
        {
            let input_id = input.id().clone();
            self.inner
                .record_durable_idempotency_deduplication(input_id.clone(), existing_id.clone());
            return Ok(AcceptOutcome::Deduplicated {
                input_id,
                existing_id,
                existing_seed,
            });
        }
        let preview = self
            .inner
            .preview_accept_resolved_input_bounded(&input, &resolved)?;
        let AcceptOutcome::Accepted {
            input_id: expected_input_id,
            ..
        } = preview
        else {
            return self.inner.accept_resolved_input(input, resolved).await;
        };

        let flags = resolved.coarse_flags();
        let changed_input_ids = resolved.persistence_changed_input_ids(&expected_input_id);
        let mut input_for_recovery = input.clone();
        externalize_input_images(self.blob_store.as_ref(), &mut input_for_recovery)
            .await
            .map_err(|err| {
                RuntimeDriverError::Internal(format!(
                    "failed to externalize runtime input images: {err}"
                ))
            })?;

        // Production registrations carry no rollback image: mutate once, then
        // either commit the exact one/two-row admission delta or degrade the
        // shared entry to ReloadRequired. Direct/test constructors retain one
        // compatibility checkpoint.
        let checkpoint = self.persistence_rollback_checkpoint();
        let mut outcome = match self.inner.accept_resolved_input(input, resolved).await {
            Ok(outcome) => outcome,
            Err(error) => {
                return Err(self.post_transition_failure(
                    checkpoint,
                    "admission_apply",
                    error.to_string(),
                ));
            }
        };
        let AcceptOutcome::Accepted {
            ref input_id,
            ref mut state,
            ref mut seed,
            ..
        } = outcome
        else {
            return Err(self.post_transition_failure(
                checkpoint,
                "admission_outcome_validation",
                format!(
                    "accepted admission preview for {expected_input_id} committed as {outcome:?}"
                ),
            ));
        };
        if input_id != &expected_input_id {
            return Err(self.post_transition_failure(
                checkpoint,
                "admission_identity_validation",
                format!(
                    "accepted admission preview named {expected_input_id} but committed {input_id}"
                ),
            ));
        }
        if let Err(error) = self
            .inner
            .machine_apply_accept_with_completion_signal(input_id, flags)
        {
            return Err(self.post_transition_failure(
                checkpoint,
                "admission_completion_signal",
                error.to_string(),
            ));
        }
        let Some(mut bundle) = self.inner.stored_input_state(input_id) else {
            return Err(self.post_transition_failure(
                checkpoint,
                "admission_input_materialization",
                format!("generated input lifecycle phase missing for accepted input {input_id}"),
            ));
        };
        bundle.state.persisted_input = Some(input_for_recovery);
        self.inner.ledger_mut().accept(bundle.state.clone());
        *state = bundle.state;
        *seed = bundle.seed;

        // Admission may atomically supersede/coalesce an older queued row.
        // Retire that terminal row's payload in this same admission delta;
        // doing it after the write would strand one full historical prompt
        // per replacement even though the live row is immediately archived.
        if let Err(error) = self
            .inner
            .retire_durably_quiescent_terminal_payloads_in(&changed_input_ids)
        {
            return Err(self.post_transition_failure(
                checkpoint,
                "admission_terminal_payload_retirement",
                error.to_string(),
            ));
        }
        let records = match self
            .inner
            .authorized_stored_input_states_for_ids(&changed_input_ids)
        {
            Ok(records) => records,
            Err(error) => {
                return Err(self.post_transition_failure(
                    checkpoint,
                    "admission_delta_materialization",
                    error.to_string(),
                ));
            }
        };
        if let Err(error) = self
            .store
            .persist_input_states_atomically(&self.runtime_id, &records)
            .await
        {
            return Err(self.post_transition_failure(
                checkpoint,
                "admission_commit",
                format!("atomic admission delta persist failed: {error}"),
            ));
        }
        let terminal_input_ids = match self
            .inner
            .archivable_terminal_input_ids_in(&changed_input_ids)
        {
            Ok(input_ids) => input_ids,
            Err(error) => {
                return Err(self.post_transition_failure(
                    None,
                    "admission_terminal_classification",
                    error.to_string(),
                ));
            }
        };
        if let Err(error) = self
            .inner
            .archive_archivable_terminal_inputs_after_durable_commit(&terminal_input_ids)
        {
            return Err(self.post_transition_failure(
                None,
                "admission_terminal_archive",
                error.to_string(),
            ));
        }

        Ok(outcome)
    }

    pub(crate) async fn preview_accept_resolved_input(
        &self,
        input: Input,
        resolved: &crate::accept::ResolvedAdmission,
    ) -> Result<AcceptOutcome, RuntimeDriverError> {
        self.require_durability_ready()?;
        if let Some((existing_id, existing_seed)) =
            self.durable_idempotency_duplicate(&input).await?
        {
            return Ok(AcceptOutcome::Deduplicated {
                input_id: input.id().clone(),
                existing_id,
                existing_seed,
            });
        }
        self.inner
            .preview_accept_resolved_input_bounded(&input, resolved)
    }

    pub(crate) fn machine_realize_authorized_stage_batch(
        &mut self,
        authority: crate::meerkat_machine::driver::AuthorizedStageForRun,
    ) -> Result<(), crate::traits::RuntimeDriverError> {
        self.inner.machine_realize_authorized_stage_batch(authority)
    }

    pub(crate) async fn machine_normalize_live_boundary_unavailable(
        &mut self,
        input_id: &InputId,
    ) -> Result<(), RuntimeDriverError> {
        self.require_durability_ready()?;
        let checkpoint = self.persistence_rollback_checkpoint();
        if let Err(error) = self
            .inner
            .machine_normalize_live_boundary_unavailable(input_id)
        {
            return Err(self.post_transition_failure(
                checkpoint,
                "live_boundary_unavailable_normalization",
                error.to_string(),
            ));
        }
        let records = match self
            .inner
            .authorized_stored_input_states_for_ids(std::slice::from_ref(input_id))
        {
            Ok(records) => records,
            Err(error) => {
                return Err(self.post_transition_failure(
                    checkpoint,
                    "live_boundary_unavailable_materialization",
                    error.to_string(),
                ));
            }
        };
        if let Err(error) = self
            .store
            .persist_input_states_atomically(&self.runtime_id, &records)
            .await
        {
            return Err(self.post_transition_failure(
                checkpoint,
                "live_boundary_unavailable_commit",
                format!("unavailable-boundary input normalization persist failed: {error}"),
            ));
        }
        Ok(())
    }

    /// Apply input (delegates to inner).
    pub fn apply_input(
        &mut self,
        input_id: &InputId,
        run_id: &meerkat_core::lifecycle::RunId,
    ) -> Result<(), crate::traits::RuntimeDriverError> {
        self.inner.apply_input(input_id, run_id)
    }

    pub(crate) fn machine_realize_terminal_failure_applied(
        &mut self,
        run_id: &meerkat_core::lifecycle::RunId,
        input_ids: &[InputId],
    ) -> Result<(), crate::traits::RuntimeDriverError> {
        self.inner
            .machine_realize_terminal_failure_applied(run_id, input_ids)
    }

    /// Roll back staged inputs (delegates to inner).
    pub fn rollback_staged(
        &mut self,
        input_ids: &[InputId],
    ) -> Result<(), crate::traits::RuntimeDriverError> {
        self.inner.rollback_staged(input_ids)
    }

    /// Resolve queued inputs the generated staging authority refused, making
    /// the machine's disposition durable before returning it.
    ///
    /// Unlike `rollback_staged` - whose mutation is persisted by its caller
    /// (`persist_machine_realized_run_failed`) - nothing downstream of a
    /// staging refusal persists: `prepare_runtime_loop_batch_start` returns a
    /// typed `StageRefused` outcome and the runtime loop simply keeps draining.
    /// So this wrapper owns the commit, exactly as `abandon_queued_input` does.
    ///
    /// BOTH dispositions are durable facts, not just the terminal one:
    /// - Abandoned: the terminal truth the caller's waiter was already failed
    ///   on. Without the commit the durable row still reads Queued with its
    ///   lane, recovery re-admits it, and work the caller was told was
    ///   abandoned executes after a restart.
    /// - Deferred: the incremented attempt count and the re-minted admission
    ///   sequence. Without the commit a restart restores the refused input as
    ///   the fifo head with its old attempt count, so the valve never advances
    ///   and the 0.8.22 wedge reconstitutes across the restart boundary.
    ///
    /// Fail-closed: every fallible step between the staged DSL transition and
    /// the durable commit restores the pre-transition checkpoint (K11).
    pub(crate) async fn resolve_unstageable_queued_inputs(
        &mut self,
        input_ids: &[InputId],
    ) -> Result<Vec<InputId>, crate::traits::RuntimeDriverError> {
        self.require_durability_ready()?;
        let checkpoint = self.persistence_rollback_checkpoint();
        // The change set mirrors the inner skip: the persistence helpers fail
        // closed on an untracked id, and the inner resolution deliberately
        // skips ids that own no queued state, so a batch member the driver does
        // not track must not turn the never-starve floor back into a hard
        // failure that drops the wake.
        let changed_input_ids = input_ids
            .iter()
            .filter(|input_id| self.inner.stored_input_state(input_id).is_some())
            .cloned()
            .collect::<Vec<_>>();
        let abandoned = match self.inner.resolve_unstageable_queued_inputs(input_ids) {
            Ok(abandoned) => abandoned,
            Err(error) => {
                return Err(self.post_transition_failure(
                    checkpoint,
                    "resolve_unstageable_queued_inputs",
                    error.to_string(),
                ));
            }
        };
        if changed_input_ids.is_empty() {
            return Ok(abandoned);
        }
        let (checkpoint, input_states, commit) = self.lifecycle_persistence_payload_with_rollback(
            checkpoint,
            &changed_input_ids,
            "unstageable queued input resolution",
        )?;
        if let Err(error) = self
            .store
            .commit_machine_lifecycle(&self.runtime_id, commit, &input_states)
            .await
        {
            return Err(self.post_transition_failure(
                checkpoint,
                "resolve_unstageable_queued_inputs_commit",
                format!("unstageable queued input resolution persist failed: {error}"),
            ));
        }
        if let Err(error) = self
            .inner
            .archive_archivable_terminal_inputs_after_durable_commit(&changed_input_ids)
        {
            return Err(self.post_transition_failure(
                None,
                "resolve_unstageable_queued_inputs_archive",
                error.to_string(),
            ));
        }
        Ok(abandoned)
    }

    /// Persist the just-staged run bindings BEFORE the run executes.
    ///
    /// `StageForRun` binds each contributing input to the run inside the
    /// generated machine, but that fact was previously durable only with the
    /// boundary commit — so a crash mid-run left the executed turn's inputs
    /// durably unbound, indistinguishable by identity from freshly queued
    /// work. Recovery refuses to guess (text is content evidence, never
    /// identity) and would hold such a tail; making the binding durable at
    /// staging closes that window for every run started by this binary.
    /// Fail-closed: a persist failure aborts the run start.
    pub(crate) async fn persist_staged_input_bindings(
        &self,
        input_ids: &[InputId],
    ) -> Result<(), RuntimeDriverError> {
        self.require_durability_ready()?;
        let records = self
            .inner
            .authorized_stored_input_states_for_ids(input_ids)?;
        if records.is_empty() {
            return Ok(());
        }
        match self
            .store
            .persist_input_states_atomically(&self.runtime_id, &records)
            .await
        {
            Ok(()) => Ok(()),
            Err(error) => Err(self.mark_durability_reload_required(
                "staged_input_binding_commit",
                format!("atomic staged input binding persist failed: {error}"),
            )),
        }
    }

    pub(crate) async fn abandon_pending_inputs(
        &mut self,
        reason: InputAbandonReason,
    ) -> Result<usize, RuntimeDriverError> {
        self.require_durability_ready()?;
        let changed_input_ids = self.inner.active_input_ids();
        let checkpoint = self.persistence_rollback_checkpoint();
        let abandoned = match self.inner.abandon_pending_inputs(reason) {
            Ok(abandoned) => abandoned,
            Err(err) => {
                return Err(self.post_transition_failure(
                    checkpoint,
                    "abandon_pending_inputs",
                    err.to_string(),
                ));
            }
        };
        let (checkpoint, input_states, commit) = self.lifecycle_persistence_payload_with_rollback(
            checkpoint,
            &changed_input_ids,
            "pending input abandon",
        )?;
        if let Err(err) = self
            .store
            .commit_machine_lifecycle(&self.runtime_id, commit, &input_states)
            .await
        {
            return Err(self.post_transition_failure(
                checkpoint,
                "abandon_pending_inputs_commit",
                format!("pending input abandon persist failed: {err}"),
            ));
        }
        if let Err(error) = self
            .inner
            .archive_archivable_terminal_inputs_after_durable_commit(&changed_input_ids)
        {
            return Err(self.post_transition_failure(
                None,
                "abandon_pending_inputs_archive",
                error.to_string(),
            ));
        }
        Ok(abandoned)
    }

    pub(crate) async fn abandon_queued_input(
        &mut self,
        input_id: &meerkat_core::lifecycle::InputId,
        reason: InputAbandonReason,
    ) -> Result<bool, RuntimeDriverError> {
        self.require_durability_ready()?;
        let checkpoint = self.persistence_rollback_checkpoint();
        let abandoned = match self.inner.abandon_queued_input(input_id, reason) {
            Ok(abandoned) => abandoned,
            Err(error) => {
                return Err(self.post_transition_failure(
                    checkpoint,
                    "abandon_queued_input",
                    error.to_string(),
                ));
            }
        };
        if !abandoned {
            return Ok(false);
        }
        let (checkpoint, input_states, commit) = self.lifecycle_persistence_payload_with_rollback(
            checkpoint,
            std::slice::from_ref(input_id),
            "tracked input cancel",
        )?;
        if let Err(error) = self
            .store
            .commit_machine_lifecycle(&self.runtime_id, commit, &input_states)
            .await
        {
            return Err(self.post_transition_failure(
                checkpoint,
                "abandon_queued_input_commit",
                format!("tracked input cancel persist failed: {error}"),
            ));
        }
        if let Err(error) = self
            .inner
            .archive_archivable_terminal_inputs_after_durable_commit(std::slice::from_ref(input_id))
        {
            return Err(self.post_transition_failure(
                None,
                "abandon_queued_input_archive",
                error.to_string(),
            ));
        }
        Ok(true)
    }

    /// Recycle the in-memory driver shell while preserving canonical pending
    /// work from durable runtime truth.
    ///
    /// Unlike `reset()`, this must not abandon queued/staged work.
    pub(crate) async fn recycle_preserving_work(&mut self) -> Result<usize, RuntimeDriverError> {
        self.require_durability_ready()?;
        let checkpoint = self.persistence_rollback_checkpoint();
        let transferred = match self.inner.recycle_preserving_work() {
            Ok(transferred) => transferred,
            Err(err) => {
                return Err(self.post_transition_failure(
                    checkpoint,
                    "recycle_preserving_work",
                    err.to_string(),
                ));
            }
        };
        let (checkpoint, input_states, commit) =
            self.lifecycle_persistence_payload_with_rollback(checkpoint, &[], "recycle")?;
        if let Err(err) = self
            .store
            .commit_machine_lifecycle(&self.runtime_id, commit, &input_states)
            .await
        {
            return Err(self.post_transition_failure(
                checkpoint,
                "recycle_commit",
                format!("recycle persist failed: {err}"),
            ));
        }

        self.inner.sync_control_projection_from_dsl_authority();
        Ok(transferred)
    }

    pub(crate) async fn realize_retire_lifecycle(
        &mut self,
    ) -> Result<crate::traits::RetireReport, RuntimeDriverError> {
        self.require_durability_ready()?;
        let checkpoint = self.persistence_rollback_checkpoint();
        let report = self.inner.finalize_retire();
        // Restore the checkpoint on classification failure: an early `?` here
        // would leave the finalized retire state live without rollback.
        let target_state = match self.runtime_state_for_persistence() {
            Ok(target_state) => target_state,
            Err(err) => {
                return Err(self.post_transition_failure(
                    checkpoint,
                    "retire_lifecycle_classification",
                    err.to_string(),
                ));
            }
        };
        self.commit_lifecycle_with_rollback(checkpoint, &[], target_state, "retire")
            .await?;
        self.inner.sync_control_projection_from_dsl_authority();
        Ok(report)
    }

    pub(crate) async fn realize_reset_lifecycle(
        &mut self,
    ) -> Result<crate::traits::ResetReport, RuntimeDriverError> {
        self.require_durability_ready()?;
        let changed_input_ids = self.inner.active_input_ids();
        let checkpoint = self.persistence_rollback_checkpoint();
        let report = match self.inner.reset_cleanup() {
            Ok(report) => report,
            Err(err) => {
                return Err(self.post_transition_failure(
                    checkpoint,
                    "reset_cleanup",
                    err.to_string(),
                ));
            }
        };
        // Restore the checkpoint on classification failure: an early `?` here
        // would leave the reset-cleaned state live without rollback.
        let target_state = match self.runtime_state_for_persistence() {
            Ok(target_state) => target_state,
            Err(err) => {
                return Err(self.post_transition_failure(
                    checkpoint,
                    "reset_lifecycle_classification",
                    err.to_string(),
                ));
            }
        };
        self.commit_lifecycle_with_rollback(checkpoint, &changed_input_ids, target_state, "reset")
            .await?;
        if let Err(error) = self
            .inner
            .archive_archivable_terminal_inputs_after_durable_commit(&changed_input_ids)
        {
            return Err(self.post_transition_failure(
                None,
                "reset_terminal_archive",
                error.to_string(),
            ));
        }
        self.inner.sync_control_projection_from_dsl_authority();
        Ok(report)
    }

    pub(crate) fn prepare_destroy_lifecycle(
        &mut self,
    ) -> Result<(Vec<InputId>, DestroyReport), RuntimeDriverError> {
        self.require_durability_ready()?;
        let changed_input_ids = self.inner.active_input_ids();
        let abandoned = match self.inner.destroy_cleanup() {
            Ok(abandoned) => abandoned,
            Err(err) => {
                return Err(self.post_transition_failure(None, "destroy_cleanup", err.to_string()));
            }
        };
        Ok((
            changed_input_ids,
            DestroyReport {
                inputs_abandoned: abandoned,
            },
        ))
    }

    pub(crate) async fn commit_prepared_destroy_lifecycle(
        &mut self,
        changed_input_ids: Vec<InputId>,
    ) -> Result<(), RuntimeDriverError> {
        self.require_durability_ready()?;
        let target_state = match self.runtime_state_for_persistence() {
            Ok(target_state) => target_state,
            Err(err) => {
                return Err(self.post_transition_failure(
                    None,
                    "destroy_lifecycle_classification",
                    err.to_string(),
                ));
            }
        };
        self.commit_lifecycle_with_rollback(None, &changed_input_ids, target_state, "destroy")
            .await?;
        if let Err(error) = self
            .inner
            .archive_archivable_terminal_inputs_after_durable_commit(&changed_input_ids)
        {
            return Err(self.post_transition_failure(
                None,
                "destroy_terminal_archive",
                error.to_string(),
            ));
        }
        self.inner.sync_control_projection_from_dsl_authority();
        Ok(())
    }

    pub(crate) fn rollback_prepared_destroy_lifecycle(&self) -> RuntimeDriverError {
        self.mark_durability_reload_required(
            "destroy_preparation_rollback",
            "prepared destroy could not reach its durable commit boundary",
        )
    }

    pub(crate) async fn finalize_runtime_executor_exit(
        &mut self,
    ) -> Result<(), RuntimeDriverError> {
        self.require_durability_ready()?;
        let changed_input_ids = self.inner.active_input_ids();
        let checkpoint = self.persistence_rollback_checkpoint();
        if let Err(err) = self.inner.apply_runtime_executor_exited_authority() {
            return Err(self.post_transition_failure(
                checkpoint,
                "runtime_executor_exit",
                err.to_string(),
            ));
        }
        if let Err(err) = self.inner.stop_runtime_cleanup() {
            return Err(self.post_transition_failure(
                checkpoint,
                "stop_runtime_cleanup",
                err.to_string(),
            ));
        }
        // Resolve the durable target BEFORE handing the checkpoint to the
        // commit helper, so a classification failure restores the staged
        // executor-exit state instead of leaving it live without rollback.
        let target_state = match self.runtime_state_for_persistence() {
            Ok(target_state) => target_state,
            Err(err) => {
                return Err(self.post_transition_failure(
                    checkpoint,
                    "stop_lifecycle_classification",
                    err.to_string(),
                ));
            }
        };
        self.commit_lifecycle_with_rollback(checkpoint, &changed_input_ids, target_state, "stop")
            .await?;
        if let Err(error) = self
            .inner
            .archive_archivable_terminal_inputs_after_durable_commit(&changed_input_ids)
        {
            return Err(self.post_transition_failure(
                None,
                "stop_terminal_archive",
                error.to_string(),
            ));
        }
        self.inner.sync_control_projection_from_dsl_authority();
        Ok(())
    }

    pub(crate) fn machine_realize_boundary_applied_in_memory(
        &mut self,
        run_id: &RunId,
        receipt: &RunBoundaryReceipt,
    ) -> Result<(), RuntimeDriverError> {
        self.inner.machine_realize_boundary_applied(run_id, receipt)
    }

    pub(crate) fn machine_realize_run_completed_in_memory(
        &mut self,
        run_id: &RunId,
        consumed_input_ids: &[InputId],
    ) -> Result<(), RuntimeDriverError> {
        self.inner
            .machine_realize_run_completed(run_id, consumed_input_ids)
    }

    pub(crate) async fn machine_realize_live_boundary_context_injected(
        &mut self,
        run_id: &RunId,
        input_ids: &[InputId],
        stage_authority: crate::meerkat_machine::driver::AuthorizedStageForRun,
        session: Option<BoundSessionCommit>,
        owner_session_id: &meerkat_core::types::SessionId,
    ) -> Result<PreparedRuntimeSessionCommitResult, RuntimeDriverError> {
        self.require_durability_ready()?;
        let checkpoint = self.persistence_rollback_checkpoint();
        let receipt = match self.inner.machine_realize_live_boundary_context_injected(
            run_id,
            input_ids,
            stage_authority,
        ) {
            Ok(receipt) => receipt,
            Err(err) => {
                return Err(self.post_transition_failure(
                    checkpoint,
                    "live_boundary_realization",
                    err.to_string(),
                ));
            }
        };
        let input_updates = match self.inner.authorized_stored_input_states_for_ids(input_ids) {
            Ok(input_updates) => input_updates,
            Err(err) => {
                return Err(self.post_transition_failure(
                    checkpoint,
                    "live_boundary_input_materialization",
                    err.to_string(),
                ));
            }
        };
        let request = match self.prepare_success_boundary(
            session,
            receipt.clone(),
            input_updates,
            owner_session_id.clone(),
        ) {
            Ok(request) => request,
            Err(error) => {
                return Err(self.post_transition_failure(
                    checkpoint,
                    "live_boundary_promotion_validation",
                    format!("runtime live-boundary promotion is invalid: {error}"),
                ));
            }
        };
        let result = match self
            .store
            .commit_prepared_session_boundary(&self.runtime_id, request)
            .await
        {
            Ok(result) => result,
            Err(err) => {
                return Err(self.post_transition_failure(
                    checkpoint,
                    "live_boundary_commit",
                    format!("runtime live-boundary context commit failed: {err}"),
                ));
            }
        };
        Ok(result)
    }

    pub(crate) async fn machine_commit_completed_boundary_snapshot(
        &mut self,
        receipt: &RunBoundaryReceipt,
        session: Option<BoundSessionCommit>,
        owner_session_id: &meerkat_core::types::SessionId,
    ) -> Result<PreparedRuntimeSessionCommitResult, RuntimeDriverError> {
        self.require_durability_ready()?;
        let input_updates = self
            .inner
            .authorized_stored_input_states_for_ids(&receipt.contributing_input_ids)?;
        let request = self
            .prepare_success_boundary(
                session,
                receipt.clone(),
                input_updates,
                owner_session_id.clone(),
            )
            .map_err(|error| {
                self.post_transition_failure(
                    None,
                    "completed_boundary_promotion_validation",
                    format!("runtime completed-boundary promotion is invalid: {error}"),
                )
            })?;
        let result = self
            .store
            .commit_prepared_session_boundary(&self.runtime_id, request)
            .await
            .map_err(|e| {
                self.post_transition_failure(
                    None,
                    "completed_boundary_commit",
                    format!("runtime completed-boundary commit failed: {e}"),
                )
            })?;
        if let Err(error) = self
            .inner
            .archive_archivable_terminal_inputs_after_durable_commit(
                &receipt.contributing_input_ids,
            )
        {
            return Err(self.post_transition_failure(
                None,
                "completed_boundary_archive",
                error.to_string(),
            ));
        }
        Ok(result)
    }

    /// Persist a failed-run realization whose generated input transitions and
    /// directed terminal outboxes have already been staged in `inner` by the
    /// shared `DriverEntry` owner. Keeping this persistence step after the
    /// shared realization makes the queued/abandoned split and its exact
    /// terminal recipient batch one atomic store commit.
    pub(crate) async fn persist_machine_realized_run_failed(
        &mut self,
        realization: crate::meerkat_machine::driver::MachineRunFailureRealization,
    ) -> Result<Option<PreparedRuntimeSessionCommitResult>, RuntimeDriverError> {
        let crate::meerkat_machine::driver::MachineRunFailureRealization {
            run_id,
            contributing_input_ids,
            replay_plan,
            terminal_error,
            runtime_apply_failure,
            contributor_disposition,
            applied_commit,
        } = realization;
        self.require_durability_ready()?;
        let terminal_input_ids = self
            .inner
            .archivable_terminal_input_ids_in(&contributing_input_ids)?;
        let checkpoint = self.persistence_rollback_checkpoint();
        let failure_cause = runtime_apply_failure.as_ref().map(|failure| failure.kind);
        tracing::debug!(
            run_id = ?run_id,
            contributors = contributing_input_ids.len(),
            replay_kind = replay_plan.notice_kind,
            contributor_disposition = contributor_disposition.as_str(),
            error = terminal_error,
            failure_cause = ?failure_cause,
            "persistent driver realized machine-owned failed-run replay"
        );
        let (checkpoint, input_states, commit) = self.lifecycle_persistence_payload_with_rollback(
            checkpoint,
            &contributing_input_ids,
            "failed-run terminal event",
        )?;
        let persist_result = if let Some(applied_commit) = applied_commit {
            let request = self.prepare_machine_terminal_boundary(
                applied_commit.session,
                applied_commit.receipt,
                commit,
                input_states,
                applied_commit.owner_session_id,
            );
            match request {
                Ok(request) => self
                    .store
                    .commit_prepared_session_boundary(&self.runtime_id, request)
                    .await
                    .map(Some),
                Err(error) => Err(error),
            }
        } else {
            self.store
                .commit_machine_lifecycle(&self.runtime_id, commit, &input_states)
                .await
                .map(|()| None)
        };
        match persist_result {
            Ok(result) => {
                if let Err(error) = self
                    .inner
                    .archive_archivable_terminal_inputs_after_durable_commit(&terminal_input_ids)
                {
                    return Err(self.post_transition_failure(
                        None,
                        "failed_run_terminal_archive",
                        error.to_string(),
                    ));
                }
                Ok(result)
            }
            Err(err) => Err(self.post_transition_failure(
                checkpoint,
                "failed_run_terminal_commit",
                format!("terminal event persist failed: {err}"),
            )),
        }
    }

    pub(crate) async fn machine_realize_run_cancelled(
        &mut self,
        run_id: &RunId,
        contributing_input_ids: &[InputId],
    ) -> Result<(), RuntimeDriverError> {
        self.require_durability_ready()?;
        let checkpoint = self.persistence_rollback_checkpoint();
        if let Err(err) = self
            .inner
            .machine_realize_run_cancelled(run_id, contributing_input_ids)
        {
            return Err(self.post_transition_failure(
                checkpoint,
                "cancelled_run_realization",
                err.to_string(),
            ));
        }
        tracing::debug!(
            run_id = ?run_id,
            contributors = contributing_input_ids.len(),
            "persistent driver realized machine-owned cancelled run"
        );
        let (checkpoint, input_states, commit) = self.lifecycle_persistence_payload_with_rollback(
            checkpoint,
            contributing_input_ids,
            "cancelled-run terminal event",
        )?;
        if let Err(err) = self
            .store
            .commit_machine_lifecycle(&self.runtime_id, commit, &input_states)
            .await
        {
            return Err(self.post_transition_failure(
                checkpoint,
                "cancelled_run_terminal_commit",
                format!("terminal cancellation persist failed: {err}"),
            ));
        }
        if let Err(error) = self
            .inner
            .archive_archivable_terminal_inputs_after_durable_commit(contributing_input_ids)
        {
            return Err(self.post_transition_failure(
                None,
                "cancelled_run_terminal_archive",
                error.to_string(),
            ));
        }
        Ok(())
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl RuntimeDriver for PersistentRuntimeDriver {
    async fn accept_input(&mut self, input: Input) -> Result<AcceptOutcome, RuntimeDriverError> {
        let resolved = self.resolve_admission(&input)?;
        self.accept_resolved_input(input, resolved).await
    }

    async fn on_runtime_event(
        &mut self,
        event: RuntimeEventEnvelope,
    ) -> Result<(), RuntimeDriverError> {
        self.require_durability_ready()?;
        self.inner.on_runtime_event(event).await
    }

    async fn recover(&mut self) -> Result<RecoveryReport, RuntimeDriverError> {
        Err(RuntimeDriverError::RecoveryRepairBlocked {
            evidence_digest: None,
            reason: "persistent driver recovery requires registration-authorized lifecycle \
                     convergence and an exact store-owned input-set revision; direct compatibility \
                     recovery is no longer supported"
                .to_string(),
        })
    }

    fn runtime_state(&self) -> RuntimeState {
        self.inner.runtime_state()
    }

    fn input_state(&self, input_id: &InputId) -> Option<&InputState> {
        self.inner.input_state(input_id)
    }

    fn input_phase(&self, input_id: &InputId) -> Option<InputLifecycleState> {
        self.inner.input_phase(input_id)
    }

    fn input_last_run_id(&self, input_id: &InputId) -> Option<RunId> {
        self.inner.input_last_run_id(input_id)
    }

    fn input_last_boundary_sequence(&self, input_id: &InputId) -> Option<u64> {
        self.inner.input_last_boundary_sequence(input_id)
    }

    fn stored_input_state(&self, input_id: &InputId) -> Option<StoredInputState> {
        self.inner.stored_input_state(input_id)
    }

    fn stored_input_states_snapshot(&self) -> Result<Vec<StoredInputState>, RuntimeDriverError> {
        self.inner.stored_input_states_snapshot()
    }

    fn input_id_for_idempotency_key(&self, idempotency_key: &str) -> Option<InputId> {
        self.inner.input_id_for_idempotency_key(idempotency_key)
    }

    fn active_input_ids(&self) -> Vec<InputId> {
        self.inner.active_input_ids()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use chrono::Utc;
    use meerkat_core::lifecycle::InputId;
    use meerkat_core::types::SessionId;

    fn durable_live_bridge_evidence() -> crate::live_execution::LiveBridgeRecoveryImage {
        serde_json::from_value(serde_json::json!({
            "operations": [
                {
                    "operation_id": "op-in-flight",
                    "channel_id": "channel-in-flight",
                    "interaction_id": "interaction-in-flight",
                    "provider_turn_ref": "turn-in-flight",
                    "provider_delegation_ref": "delegation-in-flight",
                    "provider_call_ref": "call-in-flight",
                    "source_agent_identity": "executor-in-flight",
                    "canonical_context_revision": "context-in-flight",
                    "request_digest": "sha256:request-in-flight",
                    "phase": "execution_running",
                    "terminal": null,
                    "result_digest": null,
                    "cancellation_reason": "restart",
                    "submission_output_kind": null,
                    "submission_digest": null,
                    "submission_state": null,
                    "current_for_channel": false,
                    "channel_revoked": true
                },
                {
                    "operation_id": "op-ambiguous",
                    "channel_id": "channel-ambiguous",
                    "interaction_id": "interaction-ambiguous",
                    "provider_turn_ref": "turn-ambiguous",
                    "provider_delegation_ref": "delegation-ambiguous",
                    "provider_call_ref": "call-ambiguous",
                    "source_agent_identity": "executor-ambiguous",
                    "canonical_context_revision": "context-ambiguous",
                    "request_digest": "sha256:request-ambiguous",
                    "phase": "execution_terminal",
                    "terminal": "completed",
                    "result_digest": "sha256:result-ambiguous",
                    "cancellation_reason": "channel_close",
                    "submission_output_kind": "success",
                    "submission_digest": "sha256:submission-ambiguous",
                    "submission_state": "submission_ambiguous",
                    "current_for_channel": false,
                    "channel_revoked": true
                }
            ]
        }))
        .expect("valid durable live bridge test image")
    }

    fn apply_machine_input(
        driver: &mut PersistentRuntimeDriver,
        input: crate::meerkat_machine::dsl::MeerkatMachineInput,
    ) {
        let authority = driver.inner_ref().shared_dsl_authority();
        let mut authority = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(&mut *authority, input)
            .expect("generated test transition");
    }

    fn make_prompt(text: &str) -> Input {
        Input::Prompt(crate::input::PromptInput {
            injected_context: Vec::new(),
            header: crate::input::InputHeader {
                id: InputId::new(),
                timestamp: Utc::now(),
                source: crate::input::InputOrigin::Operator,
                durability: crate::input::InputDurability::Durable,
                visibility: crate::input::InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            content: text.into(),
            typed_turn_appends: Vec::new(),
            turn_metadata: None,
        })
    }

    async fn recover_after_registration_authority(
        store: &crate::store::InMemoryRuntimeStore,
        session_id: &SessionId,
        driver: &mut PersistentRuntimeDriver,
    ) -> RecoveryReport {
        let recovery =
            crate::meerkat_machine::driver::reconcile_runtime_authority_for_cold_recovery(
                store,
                &driver.runtime_id,
                session_id,
                &meerkat_core::RuntimeEpochId::new(),
            )
            .await
            .expect("registration must converge durable runtime authority");
        driver
            .inner_mut()
            .replace_runtime_authority(recovery.authority);
        driver
            .recover_inputs_after_runtime_authority(recovery.unregister_progress.as_ref())
            .await
            .expect("registration-authorized input recovery must commit by exact batch CAS")
    }

    #[test]
    fn completed_unregister_commit_preserves_captured_live_bridge_recovery_image() {
        let store = Arc::new(crate::store::InMemoryRuntimeStore::new());
        let blob_store: Arc<dyn BlobStore> = Arc::new(meerkat_store::MemoryBlobStore::new());
        let mut driver = PersistentRuntimeDriver::new(
            LogicalRuntimeId::new("completed-unregister-live-bridge"),
            store,
            blob_store,
        );
        let session_id = crate::meerkat_machine::dsl::SessionId::from(
            "completed-unregister-live-bridge".to_string(),
        );
        driver
            .inner_mut()
            .install_registered_authority_for_test(
                session_id.clone(),
                None,
                None,
                None,
                None,
                crate::store::SupervisorAuthoritySnapshot::UnboundNoReceipt,
            )
            .expect("install registered test authority");

        let live_bridge_recovery = durable_live_bridge_evidence();
        let authority = driver.inner_ref().shared_dsl_authority();
        let mut recovered_state = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .state()
            .clone();
        live_bridge_recovery
            .restore_into(&mut recovered_state)
            .expect("restore durable bridge evidence into registered authority");
        let recovered = crate::meerkat_machine::dsl::MeerkatMachineAuthority::recover_from_state(
            recovered_state,
        )
        .expect("bridge evidence satisfies generated invariants");
        driver.inner_mut().replace_runtime_authority(recovered);
        let captured_live_bridge_recovery = {
            let authority = driver.inner_ref().shared_dsl_authority();
            let authority = authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            crate::live_execution::LiveBridgeRecoveryImage::capture(authority.state())
                .expect("capture canonical bridge evidence before unregister")
        };

        let (agent_runtime_id, fence_token, generation, runtime_epoch_id) = {
            let authority = driver.inner_ref().shared_dsl_authority();
            let authority = authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let state = authority.state();
            (
                state.active_runtime_id.clone(),
                state.active_fence_token,
                state.active_runtime_generation,
                state.active_runtime_epoch_id.clone(),
            )
        };
        apply_machine_input(
            &mut driver,
            crate::meerkat_machine::dsl::MeerkatMachineInput::BeginUnregisterSession {
                session_id: session_id.clone(),
                agent_runtime_id: agent_runtime_id.clone(),
                fence_token,
                generation,
                runtime_epoch_id: runtime_epoch_id.clone(),
            },
        );
        let (runtime_pending, comms_pending, waiters_pending) = {
            let authority = driver.inner_ref().shared_dsl_authority();
            let authority = authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let state = authority.state();
            (
                state.unregister_runtime_loop_drain_pending,
                state.unregister_comms_drain_exit_pending,
                state.unregister_completion_waiter_drain_pending,
            )
        };
        if runtime_pending {
            apply_machine_input(
                &mut driver,
                crate::meerkat_machine::dsl::MeerkatMachineInput::RuntimeLoopStoppedForUnregister {
                    session_id: session_id.clone(),
                    forced_abort: false,
                },
            );
        }
        if comms_pending {
            apply_machine_input(
                &mut driver,
                crate::meerkat_machine::dsl::MeerkatMachineInput::CommsDrainExitedForUnregister {
                    session_id: session_id.clone(),
                    forced_abort: false,
                },
            );
        }
        if waiters_pending {
            apply_machine_input(
                &mut driver,
                crate::meerkat_machine::dsl::MeerkatMachineInput::CompletionWaitersResolvedForUnregister {
                    session_id: session_id.clone(),
                },
            );
        }
        apply_machine_input(
            &mut driver,
            crate::meerkat_machine::dsl::MeerkatMachineInput::ResolveRuntimeOpsLifecycleDurability {
                session_id: session_id.clone(),
                agent_runtime_id: agent_runtime_id.clone(),
                fence_token,
                generation,
                runtime_epoch_id: runtime_epoch_id.clone(),
            },
        );
        apply_machine_input(
            &mut driver,
            crate::meerkat_machine::dsl::MeerkatMachineInput::UnregisterSession {
                session_id,
                agent_runtime_id,
                fence_token,
                generation,
                runtime_epoch_id,
            },
        );
        driver.sync_control_projection_from_dsl_authority();

        let commit = driver
            .lifecycle_commit_for_completed_unregister()
            .expect("completed unregister commit");

        assert_eq!(commit.runtime_state(), RuntimeState::Idle);
        assert_eq!(commit.snapshot().unregister_progress(), None);
        assert_eq!(
            commit.snapshot().live_bridge_recovery(),
            &captured_live_bridge_recovery,
            "completed unregister must retain in-flight and ambiguous executor evidence"
        );
    }

    #[test]
    fn provisional_promotion_is_bound_to_run_session_and_store_profile() {
        let store = Arc::new(crate::store::InMemoryRuntimeStore::new());
        let blob_store: Arc<dyn BlobStore> = Arc::new(meerkat_store::MemoryBlobStore::new());
        let driver = PersistentRuntimeDriver::new(
            LogicalRuntimeId::new("provisional-promotion-profile"),
            store,
            blob_store,
        );
        let session_id = meerkat_core::Session::new().id().clone();
        let run_id = RunId::new();
        let receipt = RunBoundaryReceipt {
            run_id: run_id.clone(),
            boundary: meerkat_core::lifecycle::run_primitive::RunApplyBoundary::Immediate,
            contributing_input_ids: Vec::new(),
            conversation_digest: Some("checkpoint-digest".to_string()),
            message_count: 1,
            sequence: 1,
        };
        let whole_blob = meerkat_core::RunCheckpointReceipt::issued(
            meerkat_core::RunCheckpointAuthority::WholeBlob(
                meerkat_core::WholeBlobProvisionalTailAuthority::issued(
                    session_id.clone(),
                    4,
                    "row-sha256:base".to_string(),
                    run_id.clone(),
                    "row-sha256:candidate".to_string(),
                    1,
                )
                .unwrap(),
            ),
            "checkpoint-digest".to_string(),
            1,
        )
        .unwrap();
        assert!(matches!(
            driver
                .prepare_provisional_promotion(&whole_blob, &receipt, &session_id)
                .unwrap(),
            PreparedProvisionalPromotion::WholeBlob(_)
        ));

        let wrong_run_receipt = RunBoundaryReceipt {
            run_id: RunId::new(),
            ..receipt.clone()
        };
        assert!(matches!(
            driver.prepare_provisional_promotion(&whole_blob, &wrong_run_receipt, &session_id),
            Err(RuntimeStoreError::SessionPersistenceAuthorityConflict { .. })
        ));

        let head_canonical = meerkat_core::RunCheckpointReceipt::issued(
            meerkat_core::RunCheckpointAuthority::HeadCanonical(
                meerkat_core::HeadCanonicalProvisionalTailAuthority::issued(
                    session_id.clone(),
                    4,
                    "head:base".to_string(),
                    5,
                    "head:candidate".to_string(),
                    run_id,
                    1,
                )
                .unwrap(),
            ),
            "checkpoint-digest".to_string(),
            1,
        )
        .unwrap();
        assert!(matches!(
            driver.prepare_provisional_promotion(&head_canonical, &receipt, &session_id),
            Err(RuntimeStoreError::SessionPersistenceAuthorityConflict { .. })
        ));
    }

    /// Dogma K11 (Persistent destroy / driver-side shadow truth): every
    /// fallible step of `commit_lifecycle_with_rollback` AFTER the caller has
    /// staged a DSL lifecycle transition must restore the caller's checkpoint.
    /// The input-state snapshot read used to escape with a bare `?`, leaving
    /// the staged lifecycle live in driver state while reporting failure.
    #[tokio::test]
    async fn commit_lifecycle_snapshot_failure_restores_checkpoint() {
        let store = Arc::new(crate::store::InMemoryRuntimeStore::new());
        let blob_store: Arc<dyn BlobStore> = Arc::new(meerkat_store::MemoryBlobStore::new());
        let rid = LogicalRuntimeId::new("commit-lifecycle-rollback-contract");
        let mut driver = PersistentRuntimeDriver::new(rid, store, blob_store);

        // Checkpoint BEFORE any state mutation (the caller's pre-stage view).
        let checkpoint = driver.inner.rollback_snapshot();

        // Mutate driver state past the checkpoint (stands in for a staged
        // Destroy/lifecycle transition awaiting durable commit).
        let input = make_prompt("staged work");
        let input_id = input.id().clone();
        let outcome = driver.accept_input(input).await.unwrap();
        assert!(outcome.is_accepted());
        assert!(driver.input_phase(&input_id).is_some());

        // Inject a failure into the input-state snapshot step.
        driver.force_input_snapshot_failure_for_test = true;
        let target_state = driver.inner_ref().runtime_state();
        let result = driver
            .commit_lifecycle_with_rollback(Some(checkpoint), &[], target_state, "test destroy")
            .await;

        // The failure must propagate typed AND the staged driver state must be
        // rolled back to the checkpoint — no half-destroyed shadow truth.
        assert!(result.is_err(), "forced snapshot failure must propagate");
        assert!(
            driver.input_phase(&input_id).is_none(),
            "staged driver state must be restored to the pre-stage checkpoint"
        );
        assert!(driver.active_input_ids().is_empty());
    }

    /// Same K11 checkpoint-restore contract for `abandon_pending_inputs`: the
    /// input-state snapshot / lifecycle-commit classification steps between
    /// the staged `&mut` abandon and the durable commit used to escape with a
    /// bare `?`, leaving the abandon applied in memory while reporting
    /// failure (and never persisting it).
    #[tokio::test]
    async fn abandon_pending_inputs_snapshot_failure_restores_checkpoint() {
        let store = Arc::new(crate::store::InMemoryRuntimeStore::new());
        let blob_store: Arc<dyn BlobStore> = Arc::new(meerkat_store::MemoryBlobStore::new());
        let rid = LogicalRuntimeId::new("abandon-rollback-contract");
        let mut driver = PersistentRuntimeDriver::new(rid, store, blob_store);

        // Accept a pending input so the abandon has staged work to mutate.
        let input = make_prompt("pending work");
        let input_id = input.id().clone();
        let outcome = driver.accept_input(input).await.unwrap();
        assert!(outcome.is_accepted());
        assert!(driver.input_phase(&input_id).is_some());

        // Inject a failure into the input-state snapshot step that runs after
        // the staged abandon mutation.
        driver.force_input_snapshot_failure_for_test = true;
        let result = driver
            .abandon_pending_inputs(InputAbandonReason::Reset)
            .await;

        assert!(result.is_err(), "forced snapshot failure must propagate");
        assert!(
            driver.input_phase(&input_id).is_some(),
            "staged abandon must be rolled back: the pending input must still be live"
        );
    }

    /// Both dispositions of a refused staging attempt must be DURABLE on a
    /// persistent runtime, not just in-memory.
    ///
    /// Nothing downstream of a staging refusal persists: unlike
    /// `rollback_staged` (whose mutation its caller commits through
    /// `persist_machine_realized_run_failed`),
    /// `prepare_runtime_loop_batch_start` returns a typed `StageRefused`
    /// outcome and the runtime loop just keeps draining. So an uncommitted
    /// resolution reconstitutes the 0.8.22 wedge across a restart:
    /// - Abandoned: the durable row still reads Queued with its lane, recovery
    ///   re-admits it, and work whose caller was already told "abandoned"
    ///   executes anyway.
    /// - Deferred: the durable row keeps its old admission sequence and attempt
    ///   count, so the refused input returns as the fifo head with the
    ///   max-attempts valve reset to zero progress.
    #[tokio::test]
    async fn resolve_unstageable_queued_inputs_commits_both_dispositions_durably() {
        async fn durable_seed(
            store: &crate::store::InMemoryRuntimeStore,
            runtime_id: &LogicalRuntimeId,
            input_id: &InputId,
        ) -> InputStateSeed {
            store
                .load_input_states_strict(runtime_id)
                .await
                .expect("durable input rows must decode")
                .into_iter()
                .find(|bundle| &bundle.state.input_id == input_id)
                .expect("a resolved input must keep a durable row")
                .seed
        }

        let store = Arc::new(crate::store::InMemoryRuntimeStore::new());
        let runtime_store: Arc<dyn RuntimeStore> = store.clone();
        let blob_store: Arc<dyn BlobStore> = Arc::new(meerkat_store::MemoryBlobStore::new());
        let runtime_id = LogicalRuntimeId::new("unstageable-queued-durability");
        let mut driver =
            PersistentRuntimeDriver::new(runtime_id.clone(), runtime_store, blob_store);

        let input = make_prompt("refused head");
        let input_id = input.id().clone();
        assert!(driver.accept_input(input).await.unwrap().is_accepted());
        assert_eq!(
            driver.input_phase(&input_id),
            Some(InputLifecycleState::Queued)
        );
        let queued_seed = durable_seed(&store, &runtime_id, &input_id).await;
        assert_eq!(queued_seed.phase, InputLifecycleState::Queued);
        assert_eq!(queued_seed.attempt_count, 0);

        // Deferral: attempt count and re-minted admission order are durable.
        assert!(
            driver
                .resolve_unstageable_queued_inputs(std::slice::from_ref(&input_id))
                .await
                .expect("machine resolves a refused queued input")
                .is_empty(),
            "attempts remain, so nothing terminalizes yet"
        );
        let deferred_seed = durable_seed(&store, &runtime_id, &input_id).await;
        assert_eq!(deferred_seed.phase, InputLifecycleState::Queued);
        assert_eq!(
            deferred_seed.attempt_count, 1,
            "the refusal must be durably counted; an in-memory-only count means a restart \
             resets the max-attempts valve and the refused head starves forever"
        );
        assert!(
            deferred_seed.admission_sequence > queued_seed.admission_sequence,
            "the deferral must durably re-mint the admission order: {:?} -> {:?}",
            queued_seed.admission_sequence,
            deferred_seed.admission_sequence
        );

        // Burn the remaining generated attempts.
        for _ in 0..2 {
            assert!(
                driver
                    .resolve_unstageable_queued_inputs(std::slice::from_ref(&input_id))
                    .await
                    .expect("machine resolves a refused queued input")
                    .is_empty(),
                "the retry cap is not reached yet"
            );
        }
        assert_eq!(
            driver
                .resolve_unstageable_queued_inputs(std::slice::from_ref(&input_id))
                .await
                .expect("machine resolves a refused queued input"),
            vec![input_id.clone()],
            "at the generated retry cap the refused input terminalizes"
        );

        let terminal_seed = durable_seed(&store, &runtime_id, &input_id).await;
        assert_eq!(
            terminal_seed.phase,
            InputLifecycleState::Abandoned,
            "the machine's terminal truth must be durable: a Queued row here means recovery \
             re-admits work whose caller was already told it was abandoned"
        );
        assert_eq!(
            terminal_seed.terminal_outcome,
            Some(crate::input_state::InputTerminalOutcome::Abandoned {
                reason: InputAbandonReason::MaxAttemptsExhausted { attempts: 3 }
            }),
            "the durable terminal outcome is the machine-owned typed class"
        );
        assert_eq!(
            terminal_seed.recovery_lane, None,
            "a terminalized row durably leaves the work lane"
        );
    }

    #[tokio::test]
    async fn retiring_active_run_persists_retired_while_retaining_live_witness() {
        let store = Arc::new(crate::store::InMemoryRuntimeStore::new());
        let runtime_id = LogicalRuntimeId::new("retire-active-run-durability");
        let runtime_store: Arc<dyn RuntimeStore> = store.clone();
        let blob_store: Arc<dyn BlobStore> = Arc::new(meerkat_store::MemoryBlobStore::new());
        let mut driver =
            PersistentRuntimeDriver::new(runtime_id.clone(), runtime_store, blob_store);
        let run_id = RunId::new();

        driver
            .contract_begin_run_authority(run_id.clone())
            .expect("contract run admission");
        assert_eq!(driver.runtime_state(), RuntimeState::Running);
        assert_eq!(driver.inner_ref().current_run_id(), Some(run_id.clone()));
        assert!(driver.inner_ref().pre_run_phase().is_some());

        let session_id = driver.inner_ref().session_authority_id_for_recovery();
        {
            let authority = driver.inner_ref().shared_dsl_authority();
            let mut authority = authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(
                &mut *authority,
                crate::meerkat_machine::dsl::MeerkatMachineInput::Retire { session_id },
            )
            .expect("machine-authorized mid-run retire transition");
        }
        driver.sync_control_projection_from_dsl_authority();
        assert_eq!(driver.runtime_state(), RuntimeState::Running);
        assert!(
            driver.inner_ref().pre_run_phase().is_some(),
            "Retire commits before the live run witness is dropped"
        );

        driver
            .realize_retire_lifecycle()
            .await
            .expect("mid-run retire must durably commit");

        assert_eq!(driver.runtime_state(), RuntimeState::Running);
        assert_eq!(driver.inner_ref().current_run_id(), Some(run_id));
        assert_eq!(
            driver.inner_ref().pre_run_phase(),
            Some(RuntimeState::Retired),
            "durable retirement must retain the live run's Retired terminal destination"
        );
        assert_eq!(
            crate::store::load_runtime_state(store.as_ref(), &runtime_id)
                .await
                .expect("reload durable lifecycle"),
            Some(RuntimeState::Retired)
        );
    }

    #[tokio::test]
    async fn interaction_terminal_outbox_delegator_swaps_exact_rows_and_reports_stale() {
        let store = Arc::new(crate::store::InMemoryRuntimeStore::new());
        let store_trait: Arc<dyn RuntimeStore> = store.clone();
        let blob_store: Arc<dyn BlobStore> = Arc::new(meerkat_store::MemoryBlobStore::new());
        let rid = LogicalRuntimeId::new("interaction-outbox-cas-delegator");
        let mut driver = PersistentRuntimeDriver::new(rid.clone(), store_trait, blob_store);

        let mut input_ids = Vec::new();
        for text in ["first", "second"] {
            let input = make_prompt(text);
            input_ids.push(input.id().clone());
            assert!(driver.accept_input(input).await.unwrap().is_accepted());
        }
        // The persistent accept path intentionally previews and durably
        // commits an isolated staged driver before realizing the same
        // admission in the live driver.  Capture the CAS witness from the
        // durable store, as recovery adoption does, instead of assuming the
        // two independently timestamped admission shells are byte-identical.
        let expected = store.load_input_states_strict(&rid).await.unwrap();
        for input_id in &input_ids {
            driver
                .inner_mut()
                .ledger_mut()
                .get_mut(input_id)
                .unwrap()
                .recovery_count = 1;
        }

        assert_eq!(
            driver
                .compare_and_swap_interaction_terminal_outbox_inputs(&expected, &input_ids)
                .await
                .unwrap(),
            InputStateBatchCasOutcome::Swapped
        );
        assert!(
            store
                .load_input_states_strict(&rid)
                .await
                .unwrap()
                .iter()
                .all(|row| row.state.recovery_count == 1)
        );

        for input_id in &input_ids {
            driver
                .inner_mut()
                .ledger_mut()
                .get_mut(input_id)
                .unwrap()
                .recovery_count = 2;
        }
        assert_eq!(
            driver
                .compare_and_swap_interaction_terminal_outbox_inputs(&expected, &input_ids)
                .await
                .unwrap(),
            InputStateBatchCasOutcome::Stale
        );
        assert!(
            store
                .load_input_states_strict(&rid)
                .await
                .unwrap()
                .iter()
                .all(|row| row.state.recovery_count == 1),
            "a stale delegator CAS must not mutate any durable row"
        );
    }

    #[tokio::test]
    async fn recover_atomically_rewrites_cold_running_lifecycle_to_idle() {
        let store = Arc::new(crate::store::InMemoryRuntimeStore::new());
        let session_id = SessionId::new();
        let runtime_id = LogicalRuntimeId::for_session(&session_id);
        store
            .commit_machine_lifecycle(
                &runtime_id,
                MachineLifecycleCommit::new_with_binding(
                    RuntimeState::Running,
                    crate::store::MachineLifecycleBindingFacts::new(
                        Some("rt:cold-running".to_string()),
                        Some(9),
                        Some(2),
                        Some("epoch-cold-running".to_string()),
                    ),
                    crate::store::SupervisorAuthoritySnapshot::UnboundNoReceipt,
                ),
                &[],
            )
            .await
            .expect("seed torn cold Running lifecycle");

        let runtime_store: Arc<dyn RuntimeStore> = store.clone();
        let blob_store: Arc<dyn BlobStore> = Arc::new(meerkat_store::MemoryBlobStore::new());
        let mut driver =
            PersistentRuntimeDriver::new(runtime_id.clone(), runtime_store, blob_store);

        recover_after_registration_authority(store.as_ref(), &session_id, &mut driver).await;

        assert_eq!(driver.runtime_state(), RuntimeState::Idle);
        assert_eq!(
            crate::store::load_runtime_state(store.as_ref(), &runtime_id)
                .await
                .expect("reload durable lifecycle"),
            Some(RuntimeState::Idle),
            "recovery acknowledgement must mean the torn lifecycle row is repaired"
        );
    }

    #[tokio::test]
    async fn exact_batch_cas_fences_stale_two_handle_finalization_and_publication_writes() {
        let store = Arc::new(crate::store::InMemoryRuntimeStore::new());
        let store_trait: Arc<dyn RuntimeStore> = store.clone();
        let blob_store: Arc<dyn BlobStore> = Arc::new(meerkat_store::MemoryBlobStore::new());
        let session_id = SessionId::new();
        let rid = LogicalRuntimeId::for_session(&session_id);
        let mut owner =
            PersistentRuntimeDriver::new(rid.clone(), store_trait.clone(), blob_store.clone());
        recover_after_registration_authority(store.as_ref(), &session_id, &mut owner).await;
        let mut input_ids = Vec::new();
        for text in ["first", "second"] {
            let input = make_prompt(text);
            input_ids.push(input.id().clone());
            assert!(owner.accept_input(input).await.unwrap().is_accepted());
        }

        // First owner acquires the durable batch witness.
        let initial = store.load_input_states_strict(&rid).await.unwrap();
        for input_id in &input_ids {
            owner
                .inner_mut()
                .ledger_mut()
                .get_mut(input_id)
                .unwrap()
                .recovery_count = 10;
        }
        assert_eq!(
            owner
                .compare_and_swap_interaction_terminal_outbox_inputs(&initial, &input_ids)
                .await
                .unwrap(),
            InputStateBatchCasOutcome::Swapped
        );
        let owner_witness = store.load_input_states_strict(&rid).await.unwrap();

        // A second store handle takes over before Candidate -> Finalized.
        let mut takeover =
            PersistentRuntimeDriver::new(rid.clone(), store_trait.clone(), blob_store.clone());
        recover_after_registration_authority(store.as_ref(), &session_id, &mut takeover).await;
        let takeover_expected = store.load_input_states_strict(&rid).await.unwrap();
        for input_id in &input_ids {
            takeover
                .inner_mut()
                .ledger_mut()
                .get_mut(input_id)
                .unwrap()
                .recovery_count = 20;
        }
        assert_eq!(
            takeover
                .compare_and_swap_interaction_terminal_outbox_inputs(
                    &takeover_expected,
                    &input_ids,
                )
                .await
                .unwrap(),
            InputStateBatchCasOutcome::Swapped
        );
        for input_id in &input_ids {
            owner
                .inner_mut()
                .ledger_mut()
                .get_mut(input_id)
                .unwrap()
                .recovery_count = 30;
        }
        assert_eq!(
            owner
                .compare_and_swap_interaction_terminal_outbox_inputs(&owner_witness, &input_ids)
                .await
                .unwrap(),
            InputStateBatchCasOutcome::Stale,
            "the superseded owner must not overwrite takeover at finalization"
        );
        assert!(
            store
                .load_input_states_strict(&rid)
                .await
                .unwrap()
                .iter()
                .all(|row| row.state.recovery_count == 20)
        );

        // The takeover owner finalizes, then a third handle takes ownership
        // before Finalized -> Published. The old finalizer's receipt write is
        // fenced by its exact pre-publication witness.
        let takeover_witness = store.load_input_states_strict(&rid).await.unwrap();
        for input_id in &input_ids {
            takeover
                .inner_mut()
                .ledger_mut()
                .get_mut(input_id)
                .unwrap()
                .recovery_count = 40;
        }
        assert_eq!(
            takeover
                .compare_and_swap_interaction_terminal_outbox_inputs(&takeover_witness, &input_ids,)
                .await
                .unwrap(),
            InputStateBatchCasOutcome::Swapped
        );
        let finalized_witness = store.load_input_states_strict(&rid).await.unwrap();
        let mut publisher = PersistentRuntimeDriver::new(rid.clone(), store_trait, blob_store);
        recover_after_registration_authority(store.as_ref(), &session_id, &mut publisher).await;
        let publisher_expected = store.load_input_states_strict(&rid).await.unwrap();
        for input_id in &input_ids {
            publisher
                .inner_mut()
                .ledger_mut()
                .get_mut(input_id)
                .unwrap()
                .recovery_count = 50;
        }
        assert_eq!(
            publisher
                .compare_and_swap_interaction_terminal_outbox_inputs(
                    &publisher_expected,
                    &input_ids,
                )
                .await
                .unwrap(),
            InputStateBatchCasOutcome::Swapped
        );
        for input_id in &input_ids {
            takeover
                .inner_mut()
                .ledger_mut()
                .get_mut(input_id)
                .unwrap()
                .recovery_count = 60;
        }
        assert_eq!(
            takeover
                .compare_and_swap_interaction_terminal_outbox_inputs(
                    &finalized_witness,
                    &input_ids,
                )
                .await
                .unwrap(),
            InputStateBatchCasOutcome::Stale,
            "the superseded finalizer must not overwrite takeover at publication"
        );
        assert!(
            store
                .load_input_states_strict(&rid)
                .await
                .unwrap()
                .iter()
                .all(|row| row.state.recovery_count == 50)
        );
    }
}
