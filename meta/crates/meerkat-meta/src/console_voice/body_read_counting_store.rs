//! Test-only `RuntimeStore` that forwards every method to an inner store and
//! counts whole-blob session body reads, so readiness tests can prove they
//! never load the body.

#![allow(clippy::too_many_arguments)]

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use meerkat_core::lifecycle::{InputId, RunBoundaryReceipt, RunId};
use meerkat_runtime::input_state::{InputStatePersistenceRecord, StoredInputState};
use meerkat_runtime::store::*;
use meerkat_runtime::*;

pub(crate) struct BodyReadCountingRuntimeStore {
    pub inner: Arc<dyn RuntimeStore>,
    pub body_reads: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl RuntimeStore for BodyReadCountingRuntimeStore {
    fn session_authority_ops(&self) -> &dyn RuntimeSessionAuthorityOps {
        self.inner.session_authority_ops()
    }

    fn execution_custody(&self) -> Option<&RuntimeStoreExecutionCustody> {
        self.inner.execution_custody()
    }

    fn try_controller_mutation_custody<'a>(
        &'a self,
        claim: &'a RuntimeStoreExecutionClaim,
    ) -> Result<Box<dyn RuntimeStoreControllerCustody + 'a>, RuntimeStoreError> {
        self.inner.try_controller_mutation_custody(claim)
    }

    fn hosting_capability(&self) -> HostingCapability {
        self.inner.hosting_capability()
    }

    fn self_fence(&self) -> Option<Arc<dyn meerkat_core::host_store::SelfFence>> {
        self.inner.self_fence()
    }

    fn session_persistence_profile(&self) -> RuntimeSessionPersistenceProfile {
        self.inner.session_persistence_profile()
    }

    fn session_boundary_authority_read_cost(&self) -> RuntimeSessionAuthorityReadCost {
        self.inner.session_boundary_authority_read_cost()
    }

    async fn activate_head_canonical_runtime_authority(
        &self,
        authority: meerkat_core::VerifiedHeadCanonicalAuthority,
    ) -> Result<HeadCanonicalRuntimeAuthorityActivation, RuntimeStoreError> {
        self.inner
            .activate_head_canonical_runtime_authority(authority)
            .await
    }

    async fn commit_prepared_session_boundary(
        &self,
        runtime_id: &LogicalRuntimeId,
        request: PreparedRuntimeSessionCommit,
    ) -> Result<PreparedRuntimeSessionCommitResult, RuntimeStoreError> {
        self.inner
            .commit_prepared_session_boundary(runtime_id, request)
            .await
    }

    async fn commit_prepared_session_boundary_with_fence(
        &self,
        runtime_id: &LogicalRuntimeId,
        request: PreparedRuntimeSessionCommit,
        write_fence: std::sync::Arc<dyn RuntimeStoreWriteFence>,
    ) -> Result<FencedPreparedRuntimeSessionCommitOutcome, RuntimeStoreError> {
        self.inner
            .commit_prepared_session_boundary_with_fence(runtime_id, request, write_fence)
            .await
    }

    async fn load_session_boundary_authority(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<RuntimeSessionAuthority>, RuntimeStoreError> {
        self.inner.load_session_boundary_authority(runtime_id).await
    }

    async fn load_head_canonical_metadata(
        &self,
        authority: &HeadCanonicalStoreAuthority,
    ) -> Result<serde_json::Map<String, serde_json::Value>, RuntimeStoreError> {
        self.inner.load_head_canonical_metadata(authority).await
    }

    async fn load_current_head_canonical_metadata(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<serde_json::Map<String, serde_json::Value>>, RuntimeStoreError> {
        self.inner
            .load_current_head_canonical_metadata(runtime_id)
            .await
    }

    async fn load_session_resume_observation(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<RuntimeSessionResumeObservation, RuntimeStoreError> {
        self.inner.load_session_resume_observation(runtime_id).await
    }

    async fn load_whole_blob_store_authority(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<WholeBlobStoreAuthority>, RuntimeStoreError> {
        self.inner.load_whole_blob_store_authority(runtime_id).await
    }

    async fn load_committed_whole_blob_snapshot(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<CommittedWholeBlobSnapshot>, RuntimeStoreError> {
        self.body_reads
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.inner
            .load_committed_whole_blob_snapshot(runtime_id)
            .await
    }

    /// The metadata read loads the committed document's bytes (meerkat
    /// #1255 decodes only its metadata), so it is a body read too.
    async fn load_committed_whole_blob_metadata(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<meerkat_runtime::CommittedWholeBlobMetadata>, RuntimeStoreError> {
        self.body_reads
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.inner
            .load_committed_whole_blob_metadata(runtime_id)
            .await
    }

    async fn commit_prepared_whole_blob_snapshot_cas(
        &self,
        runtime_id: &LogicalRuntimeId,
        prepared: PreparedWholeBlobSnapshotCas,
    ) -> Result<WholeBlobSnapshotCasOutcome, RuntimeStoreError> {
        self.inner
            .commit_prepared_whole_blob_snapshot_cas(runtime_id, prepared)
            .await
    }

    async fn delete_runtime_session_catalog_entry(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<(), RuntimeStoreError> {
        self.inner
            .delete_runtime_session_catalog_entry(runtime_id)
            .await
    }

    async fn load_runtime_session_catalog_entry(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<RuntimeSessionCatalogEntry>, RuntimeStoreError> {
        self.inner
            .load_runtime_session_catalog_entry(runtime_id)
            .await
    }

    async fn list_runtime_session_catalog_entries(
        &self,
        filter: meerkat_core::SessionFilter,
    ) -> Result<Vec<RuntimeSessionCatalogEntry>, RuntimeStoreError> {
        self.inner
            .list_runtime_session_catalog_entries(filter)
            .await
    }

    async fn write_prepared_whole_blob_provisional_tail(
        &self,
        runtime_id: &LogicalRuntimeId,
        prepared: PreparedWholeBlobProvisionalTail,
    ) -> Result<WholeBlobProvisionalTailAuthority, RuntimeStoreError> {
        self.inner
            .write_prepared_whole_blob_provisional_tail(runtime_id, prepared)
            .await
    }

    async fn load_whole_blob_provisional_tail(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<CommittedWholeBlobProvisionalTail>, RuntimeStoreError> {
        self.inner
            .load_whole_blob_provisional_tail(runtime_id)
            .await
    }

    async fn discard_whole_blob_provisional_tail(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected: &WholeBlobProvisionalTailAuthority,
    ) -> Result<bool, RuntimeStoreError> {
        self.inner
            .discard_whole_blob_provisional_tail(runtime_id, expected)
            .await
    }

    async fn write_prepared_head_canonical_provisional_tail(
        &self,
        runtime_id: &LogicalRuntimeId,
        prepared: PreparedHeadCanonicalProvisionalTail,
    ) -> Result<HeadCanonicalProvisionalTailAuthority, RuntimeStoreError> {
        self.inner
            .write_prepared_head_canonical_provisional_tail(runtime_id, prepared)
            .await
    }

    async fn load_head_canonical_provisional_tail(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<HeadCanonicalProvisionalTailAuthority>, RuntimeStoreError> {
        self.inner
            .load_head_canonical_provisional_tail(runtime_id)
            .await
    }

    async fn discard_head_canonical_provisional_tail(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected: &HeadCanonicalProvisionalTailAuthority,
    ) -> Result<bool, RuntimeStoreError> {
        self.inner
            .discard_head_canonical_provisional_tail(runtime_id, expected)
            .await
    }

    async fn load_durable_tail_recovery_source(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<PreparedDurableTailRecoverySource>, RuntimeStoreError> {
        self.inner
            .load_durable_tail_recovery_source(runtime_id)
            .await
    }

    async fn load_durable_tail_recovery_receipts(
        &self,
        runtime_id: &LogicalRuntimeId,
        run_id: &RunId,
    ) -> Result<Vec<PreparedRecoveryReceiptSource>, RuntimeStoreError> {
        self.inner
            .load_durable_tail_recovery_receipts(runtime_id, run_id)
            .await
    }

    async fn load_committed_recovery_boundary(
        &self,
        runtime_id: &LogicalRuntimeId,
        candidate_id: &str,
    ) -> Result<Option<CommittedRecoveryBoundary>, RuntimeStoreError> {
        self.inner
            .load_committed_recovery_boundary(runtime_id, candidate_id)
            .await
    }

    fn supports_compaction_projection_outbox(&self) -> bool {
        self.inner.supports_compaction_projection_outbox()
    }

    fn auth_authority_key(&self) -> Option<String> {
        self.inner.auth_authority_key()
    }

    async fn load_runtime_delivery_authority(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<RuntimeDeliveryAuthorityRecord>, RuntimeStoreError> {
        self.inner.load_runtime_delivery_authority(runtime_id).await
    }

    async fn load_runtime_delivery_record(
        &self,
        runtime_id: &LogicalRuntimeId,
        delivery_id: &str,
    ) -> Result<Option<RuntimeDeliveryStoreRecord>, RuntimeStoreError> {
        self.inner
            .load_runtime_delivery_record(runtime_id, delivery_id)
            .await
    }

    async fn compare_and_swap_runtime_delivery_authority(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected_revision: Option<u64>,
        replacement: RuntimeDeliveryAuthorityRecord,
        inserted_delivery: Option<RuntimeDeliveryStoreRecord>,
    ) -> Result<RuntimeDeliveryAuthorityCasOutcome, RuntimeStoreError> {
        self.inner
            .compare_and_swap_runtime_delivery_authority(
                runtime_id,
                expected_revision,
                replacement,
                inserted_delivery,
            )
            .await
    }

    async fn load_continuation_key_binding(
        &self,
        owner: &str,
        key: &str,
    ) -> Result<Option<ContinuationKeyBinding>, RuntimeStoreError> {
        self.inner.load_continuation_key_binding(owner, key).await
    }

    async fn compare_and_swap_runtime_delivery_authority_with_key_binding(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected_revision: Option<u64>,
        replacement: RuntimeDeliveryAuthorityRecord,
        inserted_delivery: RuntimeDeliveryStoreRecord,
        binding: ContinuationKeyBinding,
    ) -> Result<KeyedRuntimeDeliveryCasOutcome, RuntimeStoreError> {
        self.inner
            .compare_and_swap_runtime_delivery_authority_with_key_binding(
                runtime_id,
                expected_revision,
                replacement,
                inserted_delivery,
                binding,
            )
            .await
    }

    async fn load_continuation_admission(
        &self,
        address: &LogicalRuntimeId,
        delivery_id: &str,
    ) -> Result<Option<ContinuationAdmission>, RuntimeStoreError> {
        self.inner
            .load_continuation_admission(address, delivery_id)
            .await
    }

    async fn transition_continuation_admission(
        &self,
        address: &LogicalRuntimeId,
        delivery_id: &str,
        transition: ContinuationAdmissionTransition,
    ) -> Result<ContinuationAdmissionOutcome, RuntimeStoreError> {
        self.inner
            .transition_continuation_admission(address, delivery_id, transition)
            .await
    }

    async fn load_delivery_generation(&self) -> Result<u64, RuntimeStoreError> {
        self.inner.load_delivery_generation().await
    }

    fn delivery_generation_reader(&self) -> Option<Box<dyn DeliveryGenerationReader>> {
        self.inner.delivery_generation_reader()
    }

    fn commit_outcome_deadline(&self) -> Option<std::time::Duration> {
        self.inner.commit_outcome_deadline()
    }

    async fn list_runtime_delivery_authorities(
        &self,
    ) -> Result<Vec<(LogicalRuntimeId, RuntimeDeliveryAuthorityRecord)>, RuntimeStoreError> {
        self.inner.list_runtime_delivery_authorities().await
    }

    async fn list_runtime_delivery_records(
        &self,
        runtime_id: &LogicalRuntimeId,
        after_sequence: u64,
        limit: usize,
    ) -> Result<Vec<RuntimeDeliveryStoreRecord>, RuntimeStoreError> {
        self.inner
            .list_runtime_delivery_records(runtime_id, after_sequence, limit)
            .await
    }

    fn persist_auth_oauth_flow_snapshot(
        &self,
        snapshot_json: &[u8],
    ) -> Result<(), RuntimeStoreError> {
        self.inner.persist_auth_oauth_flow_snapshot(snapshot_json)
    }

    fn load_auth_oauth_flow_snapshot(&self) -> Result<Option<Vec<u8>>, RuntimeStoreError> {
        self.inner.load_auth_oauth_flow_snapshot()
    }

    fn update_auth_oauth_flow_snapshot(
        &self,
        update: &mut AuthOAuthFlowSnapshotUpdate<'_>,
    ) -> Result<(), RuntimeStoreError> {
        self.inner.update_auth_oauth_flow_snapshot(update)
    }

    async fn commit_session_snapshot(
        &self,
        runtime_id: &LogicalRuntimeId,
        session_delta: SerializedSessionSnapshot,
    ) -> Result<(), RuntimeStoreError> {
        self.inner
            .commit_session_snapshot(runtime_id, session_delta)
            .await
    }

    async fn commit_prepared_whole_blob_rewrite_boundary(
        &self,
        runtime_id: &LogicalRuntimeId,
        boundary: PreparedWholeBlobRewriteStoreParts,
    ) -> Result<WholeBlobStoreAuthority, RuntimeStoreError> {
        self.inner
            .commit_prepared_whole_blob_rewrite_boundary(runtime_id, boundary)
            .await
    }

    async fn commit_prepared_whole_blob_run_rebase(
        &self,
        runtime_id: &LogicalRuntimeId,
        rebase: PreparedWholeBlobRunRebaseStoreParts,
    ) -> Result<WholeBlobRunRebaseOutcome, RuntimeStoreError> {
        self.inner
            .commit_prepared_whole_blob_run_rebase(runtime_id, rebase)
            .await
    }

    fn supports_whole_blob_run_rebase(&self) -> bool {
        self.inner.supports_whole_blob_run_rebase()
    }

    async fn import_whole_blob_session(
        &self,
        runtime_id: &LogicalRuntimeId,
        snapshot: SerializedSessionSnapshot,
    ) -> Result<WholeBlobSessionImport, RuntimeStoreError> {
        self.inner
            .import_whole_blob_session(runtime_id, snapshot)
            .await
    }

    async fn append_service_call_audit(
        &self,
        batch: ServiceCallAuditBatch,
    ) -> Result<(), RuntimeStoreError> {
        self.inner.append_service_call_audit(batch).await
    }

    async fn read_service_call_audit(
        &self,
        key: &ServiceCallAuditKey,
    ) -> Result<Vec<ServiceCallAuditRecord>, RuntimeStoreError> {
        self.inner.read_service_call_audit(key).await
    }

    async fn atomic_apply(
        &self,
        runtime_id: &LogicalRuntimeId,
        session_delta: Option<SerializedSessionSnapshot>,
        receipt: RunBoundaryReceipt,
        input_updates: Vec<InputStatePersistenceRecord>,
        session_store_key: Option<meerkat_core::types::SessionId>,
    ) -> Result<(), RuntimeStoreError> {
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

    async fn load_pending_compaction_projections(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Vec<meerkat_core::CompactionProjectionIntent>, RuntimeStoreError> {
        self.inner
            .load_pending_compaction_projections(runtime_id)
            .await
    }

    async fn mark_compaction_projection_finalized(
        &self,
        runtime_id: &LogicalRuntimeId,
        projection: &meerkat_core::CompactionProjectionId,
    ) -> Result<(), RuntimeStoreError> {
        self.inner
            .mark_compaction_projection_finalized(runtime_id, projection)
            .await
    }

    async fn atomic_apply_with_machine_lifecycle(
        &self,
        runtime_id: &LogicalRuntimeId,
        session_delta: SerializedSessionSnapshot,
        receipt: RunBoundaryReceipt,
        machine_lifecycle: MachineLifecycleCommit,
        input_updates: Vec<InputStatePersistenceRecord>,
        session_store_key: meerkat_core::types::SessionId,
    ) -> Result<(), RuntimeStoreError> {
        self.inner
            .atomic_apply_with_machine_lifecycle(
                runtime_id,
                session_delta,
                receipt,
                machine_lifecycle,
                input_updates,
                session_store_key,
            )
            .await
    }

    async fn load_input_states(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Vec<InputStateRow>, RuntimeStoreError> {
        self.inner.load_input_states(runtime_id).await
    }

    async fn load_input_states_strict(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Vec<StoredInputState>, RuntimeStoreError> {
        self.inner.load_input_states_strict(runtime_id).await
    }

    async fn load_boundary_receipt(
        &self,
        runtime_id: &LogicalRuntimeId,
        run_id: &RunId,
        sequence: u64,
    ) -> Result<Option<RunBoundaryReceipt>, RuntimeStoreError> {
        self.inner
            .load_boundary_receipt(runtime_id, run_id, sequence)
            .await
    }

    async fn load_committed_boundary_receipts(
        &self,
        runtime_id: &LogicalRuntimeId,
        run_id: &RunId,
    ) -> Result<Vec<RunBoundaryReceipt>, RuntimeStoreError> {
        self.inner
            .load_committed_boundary_receipts(runtime_id, run_id)
            .await
    }

    async fn load_input_states_with_versions(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<PreparedRecoveryInputSnapshot, RuntimeStoreError> {
        self.inner.load_input_states_with_versions(runtime_id).await
    }

    async fn load_session_snapshot(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<std::sync::Arc<Vec<u8>>>, RuntimeStoreError> {
        self.body_reads
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.inner.load_session_snapshot(runtime_id).await
    }

    async fn clear_session_snapshot(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<(), RuntimeStoreError> {
        self.inner.clear_session_snapshot(runtime_id).await
    }

    async fn replace_session_snapshot_if_current(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected_current: &[u8],
        replacement: Vec<u8>,
    ) -> Result<bool, RuntimeStoreError> {
        self.inner
            .replace_session_snapshot_if_current(runtime_id, expected_current, replacement)
            .await
    }

    async fn clear_session_snapshot_if_current(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected_current: &[u8],
    ) -> Result<bool, RuntimeStoreError> {
        self.inner
            .clear_session_snapshot_if_current(runtime_id, expected_current)
            .await
    }

    async fn is_runtime_projection_quarantined(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<bool, RuntimeStoreError> {
        self.inner
            .is_runtime_projection_quarantined(runtime_id)
            .await
    }

    async fn persist_input_state(
        &self,
        runtime_id: &LogicalRuntimeId,
        state: &InputStatePersistenceRecord,
    ) -> Result<(), RuntimeStoreError> {
        self.inner.persist_input_state(runtime_id, state).await
    }

    async fn persist_input_states_atomically(
        &self,
        runtime_id: &LogicalRuntimeId,
        states: &[InputStatePersistenceRecord],
    ) -> Result<(), RuntimeStoreError> {
        self.inner
            .persist_input_states_atomically(runtime_id, states)
            .await
    }

    fn input_state_batch_cas_implementation_profile(
        &self,
    ) -> InputStateBatchCasImplementationProfile {
        self.inner.input_state_batch_cas_implementation_profile()
    }

    async fn compare_and_swap_input_states_atomically(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected: &[StoredInputState],
        replacements: &[InputStatePersistenceRecord],
    ) -> Result<InputStateBatchCasOutcome, RuntimeStoreError> {
        self.inner
            .compare_and_swap_input_states_atomically(runtime_id, expected, replacements)
            .await
    }

    async fn compare_and_swap_input_states_atomically_with_fence(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected: &[StoredInputState],
        replacements: &[InputStatePersistenceRecord],
        write_fence: std::sync::Arc<dyn RuntimeStoreWriteFence>,
    ) -> Result<FencedInputStateBatchCasOutcome, RuntimeStoreError> {
        self.inner
            .compare_and_swap_input_states_atomically_with_fence(
                runtime_id,
                expected,
                replacements,
                write_fence,
            )
            .await
    }

    async fn compare_and_swap_recovery_input_states_atomically(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected_revision: RecoveryInputSetRevision,
        mutations: &[RecoveryInputStateMutation],
    ) -> Result<InputStateBatchCasOutcome, RuntimeStoreError> {
        self.inner
            .compare_and_swap_recovery_input_states_atomically(
                runtime_id,
                expected_revision,
                mutations,
            )
            .await
    }

    async fn compare_and_swap_recovery_input_states_atomically_with_fence(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected_revision: RecoveryInputSetRevision,
        mutations: &[RecoveryInputStateMutation],
        write_fence: std::sync::Arc<dyn RuntimeStoreWriteFence>,
    ) -> Result<FencedInputStateBatchCasOutcome, RuntimeStoreError> {
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
    ) -> Result<Option<StoredInputState>, RuntimeStoreError> {
        self.inner.load_input_state(runtime_id, input_id).await
    }

    async fn load_input_state_by_idempotency_key(
        &self,
        runtime_id: &LogicalRuntimeId,
        key: &IdempotencyKey,
    ) -> Result<Option<ExactInputStateObservation>, RuntimeStoreError> {
        self.inner
            .load_input_state_by_idempotency_key(runtime_id, key)
            .await
    }

    async fn load_input_states_by_ids(
        &self,
        runtime_id: &LogicalRuntimeId,
        input_ids: &[InputId],
    ) -> Result<Vec<Option<StoredInputState>>, RuntimeStoreError> {
        self.inner
            .load_input_states_by_ids(runtime_id, input_ids)
            .await
    }

    async fn load_pending_terminal_owner_ids_page(
        &self,
        runtime_id: &LogicalRuntimeId,
        after: Option<&InputId>,
        limit: usize,
    ) -> Result<Vec<InputId>, RuntimeStoreError> {
        self.inner
            .load_pending_terminal_owner_ids_page(runtime_id, after, limit)
            .await
    }

    async fn observe_machine_lifecycle(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<MachineLifecycleObservation, RuntimeStoreError> {
        self.inner.observe_machine_lifecycle(runtime_id).await
    }

    async fn compare_and_swap_machine_lifecycle(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected: MachineLifecycleExpectedVersion,
        replacement: MachineLifecycleCommit,
    ) -> Result<MachineLifecycleCasOutcome, RuntimeStoreError> {
        self.inner
            .compare_and_swap_machine_lifecycle(runtime_id, expected, replacement)
            .await
    }

    async fn compare_and_swap_machine_lifecycle_with_fence(
        &self,
        runtime_id: &LogicalRuntimeId,
        expected: MachineLifecycleExpectedVersion,
        replacement: MachineLifecycleCommit,
        write_fence: std::sync::Arc<dyn RuntimeStoreWriteFence>,
    ) -> Result<FencedMachineLifecycleCasOutcome, RuntimeStoreError> {
        self.inner
            .compare_and_swap_machine_lifecycle_with_fence(
                runtime_id,
                expected,
                replacement,
                write_fence,
            )
            .await
    }

    async fn load_machine_lifecycle_record(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<Vec<u8>>, RuntimeStoreError> {
        self.inner.load_machine_lifecycle_record(runtime_id).await
    }

    async fn commit_machine_lifecycle(
        &self,
        runtime_id: &LogicalRuntimeId,
        commit: MachineLifecycleCommit,
        input_states: &[InputStatePersistenceRecord],
    ) -> Result<(), RuntimeStoreError> {
        self.inner
            .commit_machine_lifecycle(runtime_id, commit, input_states)
            .await
    }

    async fn commit_unregister_finalization(
        &self,
        runtime_id: &LogicalRuntimeId,
        finalization: UnregisterFinalizationCommit,
    ) -> Result<(), RuntimeStoreError> {
        self.inner
            .commit_unregister_finalization(runtime_id, finalization)
            .await
    }

    async fn initialize_ops_lifecycle_if_absent(
        &self,
        runtime_id: &LogicalRuntimeId,
        candidate: &meerkat_runtime::ops_lifecycle::PersistedOpsSnapshot,
    ) -> Result<meerkat_runtime::ops_lifecycle::PersistedOpsSnapshot, RuntimeStoreError> {
        self.inner
            .initialize_ops_lifecycle_if_absent(runtime_id, candidate)
            .await
    }

    async fn persist_ops_lifecycle(
        &self,
        runtime_id: &LogicalRuntimeId,
        snapshot: &meerkat_runtime::ops_lifecycle::PersistedOpsSnapshot,
    ) -> Result<(), RuntimeStoreError> {
        self.inner.persist_ops_lifecycle(runtime_id, snapshot).await
    }

    async fn load_ops_lifecycle(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<Option<meerkat_runtime::ops_lifecycle::PersistedOpsSnapshot>, RuntimeStoreError>
    {
        self.inner.load_ops_lifecycle(runtime_id).await
    }

    async fn delete_ops_lifecycle(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<(), RuntimeStoreError> {
        self.inner.delete_ops_lifecycle(runtime_id).await
    }

    async fn admit_direct_member_incarnation_high_water(
        &self,
        member_session_id: &str,
        candidate: &meerkat_contracts::wire::supervisor_bridge::BridgeDirectMemberIncarnation,
    ) -> Result<
        meerkat_contracts::wire::supervisor_bridge::BridgeDirectMemberIncarnation,
        RuntimeStoreError,
    > {
        self.inner
            .admit_direct_member_incarnation_high_water(member_session_id, candidate)
            .await
    }

    async fn load_mob_host_binding(
        &self,
        mob_id: &str,
    ) -> Result<Option<Vec<u8>>, RuntimeStoreError> {
        self.inner.load_mob_host_binding(mob_id).await
    }

    async fn list_mob_host_bindings(&self) -> Result<Vec<(String, Vec<u8>)>, RuntimeStoreError> {
        self.inner.list_mob_host_bindings().await
    }

    async fn put_mob_host_binding_if_absent(
        &self,
        mob_id: &str,
        record_json: &[u8],
    ) -> Result<bool, RuntimeStoreError> {
        self.inner
            .put_mob_host_binding_if_absent(mob_id, record_json)
            .await
    }

    async fn compare_and_put_mob_host_binding(
        &self,
        mob_id: &str,
        expected_json: &[u8],
        next_json: &[u8],
    ) -> Result<bool, RuntimeStoreError> {
        self.inner
            .compare_and_put_mob_host_binding(mob_id, expected_json, next_json)
            .await
    }

    async fn delete_mob_host_binding(
        &self,
        mob_id: &str,
        expected_json: &[u8],
    ) -> Result<bool, RuntimeStoreError> {
        self.inner
            .delete_mob_host_binding(mob_id, expected_json)
            .await
    }

    async fn load_mob_host_revocation(
        &self,
        mob_id: &str,
    ) -> Result<Option<Vec<u8>>, RuntimeStoreError> {
        self.inner.load_mob_host_revocation(mob_id).await
    }

    async fn list_mob_host_revocations(&self) -> Result<Vec<(String, Vec<u8>)>, RuntimeStoreError> {
        self.inner.list_mob_host_revocations().await
    }

    async fn revoke_mob_host_binding(
        &self,
        mob_id: &str,
        expected_binding_json: &[u8],
        receipt_json: &[u8],
    ) -> Result<bool, RuntimeStoreError> {
        self.inner
            .revoke_mob_host_binding(mob_id, expected_binding_json, receipt_json)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The counting store forwards the 0.9 runtime-store methods whose trait
    /// defaults refuse, answer none or route around the inner store.
    #[tokio::test]
    async fn the_counting_store_forwards_the_09_store_methods() {
        let probe =
            Arc::new(crate::runtime_store_forwarding_probe::ForwardingRuntimeStoreProbe::default());
        let store = BodyReadCountingRuntimeStore {
            inner: probe.clone(),
            body_reads: Arc::new(AtomicUsize::new(0)),
        };
        crate::runtime_store_forwarding_probe::assert_forwards_to(&store, &probe).await;
    }
}
