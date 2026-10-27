//! Test-only inner `RuntimeStore` for decorator forwarding tests.
//!
//! A real in-memory store answers every required method. The methods whose
//! trait defaults refuse, answer none or route around the inner store instead
//! record their call and answer something the default never would, so a
//! decorator that leaves one to its default fails [`assert_forwards_to`].

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use meerkat_runtime::store::{
    ContinuationAdmission, ContinuationAdmissionOutcome, ContinuationAdmissionTransition,
    ContinuationKeyBinding, DeliveryGenerationReader, KeyedRuntimeDeliveryCasOutcome,
    RuntimeDeliveryAuthorityRecord, RuntimeDeliveryStoreRecord, RuntimeStoreControllerCustody,
    RuntimeStoreError, RuntimeStoreExecutionClaim, RuntimeStoreExecutionCustody,
    ServiceCallAuditBatch, ServiceCallAuditKey, ServiceCallAuditRecord,
};
use meerkat_runtime::{HostingCapability, LogicalRuntimeId, RuntimeStore};

const REFUSAL: &str = "forwarding probe: controller mutation custody";
const DELIVERY_GENERATION: u64 = 41;
/// The probe's commit outcome deadline: neither the trait default
/// (`DEFAULT_COMMIT_OUTCOME_DEADLINE`) nor the in-memory store's `None`.
pub(crate) const COMMIT_OUTCOME_DEADLINE: std::time::Duration =
    std::time::Duration::from_millis(41_017);
/// The store revision of the probe's import answer: the trait default
/// refuses the import.
pub(crate) const IMPORT_REVISION: u64 = 41_023;
/// The probe's self-fence window: the trait default answers no fence at all.
pub(crate) const SELF_FENCE_WINDOW: meerkat_core::host_store::SelfFenceWindow =
    meerkat_core::host_store::SelfFenceWindow {
        elapsed: std::time::Duration::from_millis(41),
        window: std::time::Duration::from_millis(43_019),
        renew_failed: false,
    };

pub(crate) struct ForwardingRuntimeStoreProbe {
    inner: meerkat_runtime::InMemoryRuntimeStore,
    custody: RuntimeStoreExecutionCustody,
    admission: ContinuationAdmission,
    calls: Mutex<Vec<&'static str>>,
    /// The service-call audit keys the probe's audit append and read saw.
    audit_keys: Mutex<Vec<ServiceCallAuditKey>>,
}

impl Default for ForwardingRuntimeStoreProbe {
    fn default() -> Self {
        Self {
            inner: meerkat_runtime::InMemoryRuntimeStore::new(),
            custody: RuntimeStoreExecutionCustody::new(),
            admission: ContinuationAdmission::Applied {
                session_id: meerkat_core::types::SessionId::new(),
                input_id: meerkat_core::lifecycle::InputId::new(),
            },
            calls: Mutex::new(Vec::new()),
            audit_keys: Mutex::new(Vec::new()),
        }
    }
}

impl ForwardingRuntimeStoreProbe {
    fn record(&self, call: &'static str) {
        self.calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(call);
    }

    fn calls(&self) -> Vec<&'static str> {
        self.calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn record_audit_key(&self, key: ServiceCallAuditKey) {
        self.audit_keys
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(key);
    }

    fn audit_keys(&self) -> Vec<ServiceCallAuditKey> {
        self.audit_keys
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

fn binding(owner: &str, key: &str) -> ContinuationKeyBinding {
    ContinuationKeyBinding {
        owner: owner.to_string(),
        key: key.to_string(),
        address: LogicalRuntimeId::new("forwarding-probe"),
        delivery_id: "probe-delivery".to_string(),
        submission_digest: "probe-digest".to_string(),
        committed_at_ms: 7,
    }
}

/// Store parts of a real WholeBlob run re-base, prepared over an in-memory
/// store of its own. The probe answers without reading them, but only the
/// runtime's preparation mints the value the call takes.
async fn run_rebase_parts() -> meerkat_runtime::store::PreparedWholeBlobRunRebaseStoreParts {
    use meerkat_core::types::{Message, UserMessage};

    let source = meerkat_runtime::InMemoryRuntimeStore::new();
    let mut original = meerkat_core::Session::new();
    original.push(Message::User(UserMessage::text("hello".to_string())));
    original.push(Message::User(UserMessage::text("again".to_string())));
    let runtime_id = LogicalRuntimeId::for_session(original.id());
    source
        .commit_session_snapshot(
            &runtime_id,
            meerkat_runtime::store::SerializedSessionSnapshot {
                session_snapshot: serde_json::to_vec(&original)
                    .expect("the session serializes")
                    .into(),
            },
        )
        .await
        .expect("the committed base lands");
    let committed = source
        .load_committed_whole_blob_snapshot(&runtime_id)
        .await
        .expect("the committed base loads")
        .expect("a committed base");
    let payload = meerkat_runtime::store::VerifiedCommittedWholeBlobPayload::from_committed(
        original.id(),
        committed,
    )
    .expect("the committed base verifies");
    let run_id = meerkat_core::lifecycle::RunId::new();
    let mut live = original.clone();
    live.push(Message::User(UserMessage::text("next".to_string())));
    live.set_run_cursor(&meerkat_core::SessionRunCursor::new(
        run_id.clone(),
        Vec::new(),
        0,
        meerkat_core::RunResumePoint::AwaitingModel,
    ))
    .expect("the run cursor is recorded");
    let mut compacted = live.clone();
    let parent = compacted
        .transcript_revision()
        .expect("the live transcript has a revision");
    let commit = compacted
        .commit_transcript_rewrite(
            meerkat_core::TranscriptRewriteSelection::MessageRange { start: 0, end: 2 },
            vec![Message::User(UserMessage::compaction_summary("compacted"))],
            meerkat_core::TranscriptRewriteReason::new("compaction"),
            Some("forwarding-probe-rebase".to_string()),
            Some(parent),
        )
        .expect("the compaction rewrite commits");
    meerkat_runtime::store::PreparedWholeBlobRunRebase::prepare(
        payload,
        None,
        run_id,
        compacted,
        std::slice::from_ref(&commit),
    )
    .expect("the run re-base prepares")
    .store_parts()
}

/// Drive every forwarded method through `store`, a decorator over `probe`,
/// and require the probe's own answer from exactly one call of each.
pub(crate) async fn assert_forwards_to(
    store: &dyn RuntimeStore,
    probe: &Arc<ForwardingRuntimeStoreProbe>,
) {
    assert!(
        store
            .execution_custody()
            .is_some_and(|custody| std::ptr::eq(custody, &raw const probe.custody)),
        "the inner store's own execution custody"
    );
    let claim = RuntimeStoreExecutionCustody::new()
        .try_acquire_shared()
        .expect("an ordinary claim");
    assert!(matches!(
        store.try_controller_mutation_custody(&claim),
        Err(RuntimeStoreError::Unsupported(reason)) if reason == REFUSAL
    ));
    assert!(matches!(
        store.hosting_capability(),
        HostingCapability::ProcessLocal
    ));
    assert_eq!(
        store.self_fence().map(|fence| fence.window()),
        Some(SELF_FENCE_WINDOW),
        "the inner store's self-fence"
    );
    assert_eq!(
        store
            .load_continuation_key_binding("owner", "key")
            .await
            .expect("the inner store's binding"),
        Some(binding("owner", "key"))
    );
    let address = LogicalRuntimeId::new("forwarded-address");
    assert_eq!(
        store
            .compare_and_swap_runtime_delivery_authority_with_key_binding(
                &address,
                Some(3),
                RuntimeDeliveryAuthorityRecord::from_parts(4, b"{}".to_vec()),
                RuntimeDeliveryStoreRecord::from_parts("delivery", 1, b"{}".to_vec()),
                binding("owner", "bound"),
            )
            .await
            .expect("the inner store's outcome"),
        KeyedRuntimeDeliveryCasOutcome::KeyAlreadyBound(binding("owner", "bound"))
    );
    assert_eq!(
        store
            .load_continuation_admission(&address, "delivery")
            .await
            .expect("the inner store's admission"),
        Some(probe.admission.clone())
    );
    assert_eq!(
        store
            .transition_continuation_admission(
                &address,
                "delivery",
                ContinuationAdmissionTransition::Apply {
                    session_id: probe.admission.session_id().clone(),
                    input_id: probe.admission.input_id().clone(),
                },
            )
            .await
            .expect("the inner store's outcome"),
        ContinuationAdmissionOutcome::Transitioned(probe.admission.clone())
    );
    assert_eq!(
        store
            .load_delivery_generation()
            .await
            .expect("the inner store's generation"),
        DELIVERY_GENERATION
    );
    let mut reader = store
        .delivery_generation_reader()
        .expect("the inner store's generation reader");
    assert_eq!(
        reader.read().await.expect("the inner reader's generation"),
        DELIVERY_GENERATION
    );
    assert_eq!(
        store.commit_outcome_deadline(),
        Some(COMMIT_OUTCOME_DEADLINE),
        "the inner store's commit outcome deadline"
    );
    assert!(
        store.supports_whole_blob_run_rebase(),
        "the inner store's run re-base support"
    );
    let rebase = run_rebase_parts().await;
    assert_eq!(
        store
            .commit_prepared_whole_blob_run_rebase(&address, rebase)
            .await
            .expect("the inner store's outcome"),
        meerkat_runtime::store::WholeBlobRunRebaseOutcome::Stale
    );
    let audit_key = ServiceCallAuditKey::new("forwarding-probe-original", "probe-occurrence");
    store
        .append_service_call_audit(ServiceCallAuditBatch::new(audit_key.clone(), Vec::new()))
        .await
        .expect("the inner store's audit append");
    assert_eq!(
        store
            .read_service_call_audit(&audit_key)
            .await
            .expect("the inner store's audit read"),
        Vec::<ServiceCallAuditRecord>::new()
    );
    assert_eq!(
        probe.audit_keys(),
        vec![audit_key.clone(), audit_key],
        "the audit append and read reach the inner store with their own key"
    );
    let imported = store
        .import_whole_blob_session(
            &address,
            meerkat_runtime::store::SerializedSessionSnapshot {
                session_snapshot: Arc::new(b"{}".to_vec()),
            },
        )
        .await
        .expect("the inner store's import");
    assert!(
        matches!(
            &imported,
            meerkat_runtime::WholeBlobSessionImport::AlreadyCurrent(authority)
                if authority.store_revision() == IMPORT_REVISION
        ),
        "the inner store's import outcome: {imported:?}"
    );
    assert_eq!(
        probe.calls(),
        vec![
            "execution_custody",
            "try_controller_mutation_custody",
            "hosting_capability",
            "self_fence",
            "load_continuation_key_binding",
            "compare_and_swap_runtime_delivery_authority_with_key_binding",
            "load_continuation_admission",
            "transition_continuation_admission",
            "load_delivery_generation",
            "delivery_generation_reader",
            "commit_outcome_deadline",
            "supports_whole_blob_run_rebase",
            "commit_prepared_whole_blob_run_rebase",
            "append_service_call_audit",
            "read_service_call_audit",
            "import_whole_blob_session",
        ],
        "each method reaches the inner store exactly once"
    );
}

// The decorator verifier (scripts/verify-session-service-decorators.py)
// skips only impls inside a `#[cfg(test)]` module. This whole file is
// test-only, so the double's impl says so where the verifier looks.
#[cfg(test)]
mod store_impl {
    use super::*;

    /// The probe's own generation reader: the trait default answers none, so
    /// a decorator that keeps the default fails the forwarding check.
    struct ProbeGenerationReader;

    #[async_trait]
    impl DeliveryGenerationReader for ProbeGenerationReader {
        async fn read(&mut self) -> Result<u64, RuntimeStoreError> {
            Ok(DELIVERY_GENERATION)
        }

        fn keeps_connection(&self) -> bool {
            false
        }

        fn release(&mut self) {}
    }

    /// The probe's own self-fence: the trait default answers none, so a
    /// decorator that keeps the default fails the forwarding check.
    struct ProbeSelfFence;

    impl meerkat_core::host_store::SelfFence for ProbeSelfFence {
        fn window(&self) -> meerkat_core::host_store::SelfFenceWindow {
            SELF_FENCE_WINDOW
        }
    }

    #[async_trait]
    impl RuntimeStore for ForwardingRuntimeStoreProbe {
        fn session_authority_ops(&self) -> &dyn meerkat_runtime::store::RuntimeSessionAuthorityOps {
            self.inner.session_authority_ops()
        }

        fn execution_custody(&self) -> Option<&RuntimeStoreExecutionCustody> {
            self.record("execution_custody");
            Some(&self.custody)
        }

        fn try_controller_mutation_custody<'a>(
            &'a self,
            _claim: &'a RuntimeStoreExecutionClaim,
        ) -> Result<Box<dyn RuntimeStoreControllerCustody + 'a>, RuntimeStoreError> {
            self.record("try_controller_mutation_custody");
            Err(RuntimeStoreError::Unsupported(REFUSAL.to_string()))
        }

        fn hosting_capability(&self) -> HostingCapability {
            self.record("hosting_capability");
            HostingCapability::ProcessLocal
        }

        fn self_fence(&self) -> Option<Arc<dyn meerkat_core::host_store::SelfFence>> {
            self.record("self_fence");
            Some(Arc::new(ProbeSelfFence))
        }

        async fn load_continuation_key_binding(
            &self,
            owner: &str,
            key: &str,
        ) -> Result<Option<ContinuationKeyBinding>, RuntimeStoreError> {
            self.record("load_continuation_key_binding");
            Ok(Some(binding(owner, key)))
        }

        async fn compare_and_swap_runtime_delivery_authority_with_key_binding(
            &self,
            _runtime_id: &LogicalRuntimeId,
            _expected_revision: Option<u64>,
            _replacement: RuntimeDeliveryAuthorityRecord,
            _inserted_delivery: RuntimeDeliveryStoreRecord,
            binding: ContinuationKeyBinding,
        ) -> Result<KeyedRuntimeDeliveryCasOutcome, RuntimeStoreError> {
            self.record("compare_and_swap_runtime_delivery_authority_with_key_binding");
            Ok(KeyedRuntimeDeliveryCasOutcome::KeyAlreadyBound(binding))
        }

        async fn load_continuation_admission(
            &self,
            _address: &LogicalRuntimeId,
            _delivery_id: &str,
        ) -> Result<Option<ContinuationAdmission>, RuntimeStoreError> {
            self.record("load_continuation_admission");
            Ok(Some(self.admission.clone()))
        }

        async fn transition_continuation_admission(
            &self,
            _address: &LogicalRuntimeId,
            _delivery_id: &str,
            _transition: ContinuationAdmissionTransition,
        ) -> Result<ContinuationAdmissionOutcome, RuntimeStoreError> {
            self.record("transition_continuation_admission");
            Ok(ContinuationAdmissionOutcome::Transitioned(
                self.admission.clone(),
            ))
        }

        async fn load_delivery_generation(&self) -> Result<u64, RuntimeStoreError> {
            self.record("load_delivery_generation");
            Ok(DELIVERY_GENERATION)
        }

        fn delivery_generation_reader(&self) -> Option<Box<dyn DeliveryGenerationReader>> {
            self.record("delivery_generation_reader");
            Some(Box::new(ProbeGenerationReader))
        }

        fn commit_outcome_deadline(&self) -> Option<std::time::Duration> {
            self.record("commit_outcome_deadline");
            Some(COMMIT_OUTCOME_DEADLINE)
        }

        // The trait default answers `false`.
        fn supports_whole_blob_run_rebase(&self) -> bool {
            self.record("supports_whole_blob_run_rebase");
            true
        }

        // The trait default refuses with `Unsupported`.
        async fn commit_prepared_whole_blob_run_rebase(
            &self,
            _runtime_id: &LogicalRuntimeId,
            _rebase: meerkat_runtime::store::PreparedWholeBlobRunRebaseStoreParts,
        ) -> Result<meerkat_runtime::store::WholeBlobRunRebaseOutcome, RuntimeStoreError> {
            self.record("commit_prepared_whole_blob_run_rebase");
            Ok(meerkat_runtime::store::WholeBlobRunRebaseOutcome::Stale)
        }

        // The trait default refuses with `Unsupported`.
        async fn append_service_call_audit(
            &self,
            batch: ServiceCallAuditBatch,
        ) -> Result<(), RuntimeStoreError> {
            self.record("append_service_call_audit");
            self.record_audit_key(batch.key);
            Ok(())
        }

        // The trait default refuses with `Unsupported`.
        async fn import_whole_blob_session(
            &self,
            _runtime_id: &LogicalRuntimeId,
            _snapshot: meerkat_runtime::store::SerializedSessionSnapshot,
        ) -> Result<meerkat_runtime::WholeBlobSessionImport, RuntimeStoreError> {
            self.record("import_whole_blob_session");
            meerkat_runtime::store::WholeBlobStoreAuthority::from_store_record(
                meerkat_runtime::store::WholeBlobStoreAuthority::VERSION,
                meerkat_core::types::SessionId::new(),
                IMPORT_REVISION,
                format!("row-sha256:{}", "4".repeat(64)),
            )
            .map(meerkat_runtime::WholeBlobSessionImport::AlreadyCurrent)
        }

        // The trait default refuses with `Unsupported`.
        async fn read_service_call_audit(
            &self,
            key: &ServiceCallAuditKey,
        ) -> Result<Vec<ServiceCallAuditRecord>, RuntimeStoreError> {
            self.record("read_service_call_audit");
            self.record_audit_key(key.clone());
            Ok(Vec::new())
        }

        async fn commit_session_snapshot(
            &self,
            runtime_id: &LogicalRuntimeId,
            session_delta: meerkat_runtime::store::SerializedSessionSnapshot,
        ) -> Result<(), RuntimeStoreError> {
            self.inner
                .commit_session_snapshot(runtime_id, session_delta)
                .await
        }

        async fn commit_prepared_whole_blob_rewrite_boundary(
            &self,
            runtime_id: &LogicalRuntimeId,
            boundary: meerkat_runtime::store::PreparedWholeBlobRewriteStoreParts,
        ) -> Result<meerkat_runtime::store::WholeBlobStoreAuthority, RuntimeStoreError> {
            self.inner
                .commit_prepared_whole_blob_rewrite_boundary(runtime_id, boundary)
                .await
        }

        async fn atomic_apply(
            &self,
            runtime_id: &LogicalRuntimeId,
            session_delta: Option<meerkat_runtime::store::SerializedSessionSnapshot>,
            receipt: meerkat_core::lifecycle::RunBoundaryReceipt,
            input_updates: Vec<meerkat_runtime::input_state::InputStatePersistenceRecord>,
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

        async fn load_input_states(
            &self,
            runtime_id: &LogicalRuntimeId,
        ) -> Result<Vec<meerkat_runtime::InputStateRow>, RuntimeStoreError> {
            self.inner.load_input_states(runtime_id).await
        }

        async fn load_boundary_receipt(
            &self,
            runtime_id: &LogicalRuntimeId,
            run_id: &meerkat_core::lifecycle::RunId,
            sequence: u64,
        ) -> Result<Option<meerkat_core::lifecycle::RunBoundaryReceipt>, RuntimeStoreError>
        {
            self.inner
                .load_boundary_receipt(runtime_id, run_id, sequence)
                .await
        }

        async fn load_session_snapshot(
            &self,
            runtime_id: &LogicalRuntimeId,
        ) -> Result<Option<Arc<Vec<u8>>>, RuntimeStoreError> {
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

        async fn persist_input_state(
            &self,
            runtime_id: &LogicalRuntimeId,
            state: &meerkat_runtime::input_state::InputStatePersistenceRecord,
        ) -> Result<(), RuntimeStoreError> {
            self.inner.persist_input_state(runtime_id, state).await
        }

        async fn load_input_state(
            &self,
            runtime_id: &LogicalRuntimeId,
            input_id: &meerkat_core::lifecycle::InputId,
        ) -> Result<Option<meerkat_runtime::input_state::StoredInputState>, RuntimeStoreError>
        {
            self.inner.load_input_state(runtime_id, input_id).await
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
            commit: meerkat_runtime::store::MachineLifecycleCommit,
            input_states: &[meerkat_runtime::input_state::InputStatePersistenceRecord],
        ) -> Result<(), RuntimeStoreError> {
            self.inner
                .commit_machine_lifecycle(runtime_id, commit, input_states)
                .await
        }
    }
}
