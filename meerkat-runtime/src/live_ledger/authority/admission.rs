use super::*;
use crate::identifiers::LogicalRuntimeId;
use crate::input::{
    Input, InputDurability, InputHeader, InputOrigin, InputVisibility, LiveRequestInput,
};
use crate::input_state::InputStatePersistenceRecord;
use crate::live_grant::LiveExecutionGrant;
use crate::live_ledger::authority::LiveInputAdmissionValidation;
use crate::live_ledger::source::LiveSourceRow;
use crate::live_request::{
    AdmittedLiveExecutionAuthority, AdmittedLiveExecutionRecord, LiveExecutionRequestRecord,
};
use crate::live_source::{LiveSourceDisposition, LiveSourceEntryRecord};
use crate::store::MachineLifecycleObservation;
use crate::traits::RuntimeDriverError;
use meerkat_core::execution_scope::ExecutionAdmissionCommitRef;
use meerkat_core::lifecycle::InputId;
use meerkat_core::live_execution::evidence::DelegatedRequestProvenance;
use meerkat_core::live_execution::request::LiveSourceKey;
use sha2::{Digest, Sha256};
use std::num::NonZeroU64;

#[path = "admission/callback.rs"]
mod callback;

pub(crate) struct LiveAdmissionRuntimeBinding {
    authority: crate::driver::ephemeral::SharedIngressDslAuthority,
    external: Option<Arc<dyn RuntimeStoreWriteFence>>,
}

impl LiveAdmissionRuntimeBinding {
    pub(crate) fn new(
        authority: crate::driver::ephemeral::SharedIngressDslAuthority,
        external: Option<Arc<dyn RuntimeStoreWriteFence>>,
    ) -> Self {
        Self {
            authority,
            external,
        }
    }

    pub(super) fn fence(
        &self,
        input: &Input,
        binding: &meerkat_core::execution_scope::ScopedExecutorBinding,
    ) -> Result<Arc<dyn RuntimeStoreWriteFence>, RuntimeDriverError> {
        use crate::meerkat_machine::dsl as mm;
        let authority = self.authority.lock().map_err(internal)?;
        let state = authority.state();
        let command = mm::MeerkatMachineInput::Ingest {
            session_id: mm::SessionId::from_domain(&binding.session_id),
            runtime_id: state
                .active_runtime_id
                .clone()
                .ok_or_else(|| refused("Live admission requires a bound runtime"))?,
            fence_token: state
                .active_fence_token
                .ok_or_else(|| refused("Live admission requires a bound runtime fence"))?,
            generation: Some(mm::Generation::from(binding.binding_generation)),
            runtime_epoch_id: Some(mm::RuntimeEpochId::from_domain(&binding.runtime_epoch)),
            work_id: mm::WorkId::from_domain(input.id()),
            origin: mm::WorkOrigin::Ingest,
        };
        Ok(Arc::new(LiveRuntimeIngressFence {
            authority: Arc::clone(&self.authority),
            command,
            external: self.external.clone(),
        }))
    }
}

struct LiveRuntimeIngressFence {
    authority: crate::driver::ephemeral::SharedIngressDslAuthority,
    command: crate::meerkat_machine::dsl::MeerkatMachineInput,
    external: Option<Arc<dyn RuntimeStoreWriteFence>>,
}

impl RuntimeStoreWriteFence for LiveRuntimeIngressFence {
    fn execute_if_current(
        &self,
        operation: Box<dyn FnOnce() -> Result<(), RuntimeStoreError> + '_>,
    ) -> Result<RuntimeStoreWriteFenceOutcome, RuntimeStoreError> {
        let checked: Box<dyn FnOnce() -> Result<(), RuntimeStoreError> + '_> = Box::new(|| {
            use crate::meerkat_machine::dsl as mm;
            let authority = self.authority.lock().map_err(|error| {
                RuntimeStoreError::WriteFailed(format!(
                    "Live admission runtime authority lock failed: {error}"
                ))
            })?;
            let mut candidate = authority.prepare_authority();
            let transition = mm::MeerkatMachineMutator::apply(&mut candidate, self.command.clone())
                .map_err(|error| {
                    RuntimeStoreError::WriteFailed(format!(
                        "generated runtime rejected Live admission binding: {error}"
                    ))
                })?;
            if !matches!(
                transition.effects(),
                [mm::MeerkatMachineEffect::ResolveAdmission]
            ) {
                return Err(RuntimeStoreError::WriteFailed(
                    "generated runtime did not validate Live admission ingress".into(),
                ));
            }
            let result = operation();
            drop(authority);
            result
        });
        match &self.external {
            Some(external) => execute_runtime_store_write_fence(external.as_ref(), checked),
            None => {
                checked()?;
                Ok(RuntimeStoreWriteFenceOutcome::Applied)
            }
        }
    }
}

pub(crate) struct PendingLiveAdmission {
    store: Arc<dyn RuntimeStore>,
    input: Input,
    input_digest: [u8; 32],
    source_before: LiveSourceRow,
    operation: LiveAdmissionOperation,
    runtime_binding: LiveAdmissionRuntimeBinding,
    receipt_tx: tokio::sync::oneshot::Sender<AdmittedLiveExecutionAuthority>,
}

enum LiveAdmissionOperation {
    Admit(Box<PreparedLiveInputAdmission>),
    Observe,
}

struct PreparedLiveInputAdmission {
    prepared: PreparedLiveLedgerCommit,
    source_after: LiveSourceRow,
    lifecycle: MachineLifecycleObservationVersion,
    fence: Arc<dyn RuntimeStoreWriteFence>,
    receipt: AdmittedLiveExecutionRecord,
    callback: Option<CallbackAdmissionFence>,
}

struct CallbackAdmissionFence {
    origin: crate::store::ExactInputStateObservation,
    results: crate::store::CommittedCallbackResultsObservation,
}

impl std::fmt::Debug for PendingLiveAdmission {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingLiveAdmission")
            .field("input_id", self.input.id())
            .finish_non_exhaustive()
    }
}

// This is handoff identity, not a comparison of permission currentness.
impl PartialEq for PendingLiveAdmission {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}

pub(crate) struct CommittedLiveAdmission {
    record: AdmittedLiveExecutionRecord,
}

impl CommittedLiveAdmission {
    pub(crate) fn into_record(self) -> AdmittedLiveExecutionRecord {
        self.record
    }
}

fn internal(error: impl std::fmt::Display) -> RuntimeDriverError {
    RuntimeDriverError::Internal(error.to_string())
}

fn refused(reason: &str) -> RuntimeDriverError {
    RuntimeDriverError::ValidationFailed {
        reason: reason.into(),
    }
}

pub(super) fn source_input_key(
    source: &LiveSourceKey,
) -> Result<crate::identifiers::IdempotencyKey, RuntimeDriverError> {
    let encoded = serde_json::to_vec(source).map_err(internal)?;
    Ok(crate::identifiers::IdempotencyKey(format!(
        "live-source:{:x}",
        Sha256::digest(encoded)
    )))
}

pub(super) fn continuation_input_key(
    request_id: &meerkat_core::ops::OperationId,
    callback: &meerkat_core::session::CallbackBatchIdentity,
) -> Result<crate::identifiers::IdempotencyKey, RuntimeDriverError> {
    let encoded = serde_json::to_vec(&(request_id, callback)).map_err(internal)?;
    Ok(crate::identifiers::IdempotencyKey(format!(
        "live-callback:{:x}",
        Sha256::digest(encoded)
    )))
}

pub(super) fn run_input_key(
    state: &dsl::LiveRequestMachineState,
    session_id: &SessionId,
    request_id: &meerkat_core::ops::OperationId,
    run_id: &str,
) -> Result<crate::identifiers::IdempotencyKey, RuntimeDriverError> {
    let request = request_id.to_string();
    if state.run_requests.get(run_id) != Some(&request) {
        return Err(refused("Live run does not belong to this request"));
    }
    let predecessor = state
        .run_predecessors
        .get(run_id)
        .ok_or_else(|| refused("Live run lost its predecessor link"))?;
    if predecessor.is_empty() {
        let source: LiveSourceKey = serde_json::from_str(
            state
                .request_sources
                .get(&request)
                .ok_or_else(|| refused("Live request lost its source"))?,
        )
        .map_err(internal)?;
        if source.session_id() != session_id {
            return Err(refused("Live request source belongs to another session"));
        }
        return source_input_key(&source);
    }
    let callback: meerkat_core::session::CallbackBatchIdentity = serde_json::from_str(
        state
            .run_callback_records
            .get(predecessor)
            .ok_or_else(|| refused("Live continuation lost its callback predecessor"))?,
    )
    .map_err(internal)?;
    if callback.session_id() != session_id
        || callback.run_id().to_string() != *predecessor
        || state.run_requests.get(predecessor) != Some(&request)
        || state.run_successors.get(predecessor).map(String::as_str) != Some(run_id)
    {
        return Err(refused(
            "Live callback predecessor differs from its exact run chain",
        ));
    }
    continuation_input_key(request_id, &callback)
}

fn source_input(
    record: &crate::live_source::LiveSourceReservationRecord,
) -> Result<Input, RuntimeDriverError> {
    let evidence = record
        .frozen_request()
        .ok_or_else(|| refused("Live reservation has no frozen request"))?;
    Ok(Input::LiveRequest(LiveRequestInput {
        header: InputHeader {
            id: InputId::new(),
            timestamp: chrono::Utc::now(),
            source: InputOrigin::LiveRequest,
            durability: InputDurability::Durable,
            visibility: InputVisibility::default(),
            idempotency_key: Some(source_input_key(record.source())?),
            supersession_key: None,
            correlation_id: None,
        },
        request: LiveExecutionRequestRecord::LiveRequest {
            provenance: DelegatedRequestProvenance::new(
                record.request_id().clone(),
                record.source().clone(),
                evidence.kind(),
                evidence.request().digest(),
            )
            .map_err(internal)?,
            source_row: record.frozen_digest().map_err(internal)?,
        },
    }))
}

impl LiveRequestStoreOwner {
    pub(crate) async fn prepare_input_admission<Member: PartialEq + serde::Serialize>(
        &self,
        source: &LiveSourceKey,
        grant: &LiveExecutionGrant<Member>,
        runtime_binding: LiveAdmissionRuntimeBinding,
        receipt_tx: tokio::sync::oneshot::Sender<AdmittedLiveExecutionAuthority>,
    ) -> Result<PendingLiveAdmission, RuntimeDriverError> {
        let ops = self.store.live_ledger_ops().ok_or_else(|| {
            refused("Live input admission requires independent Live ledger storage")
        })?;
        if !ops.ledger_write_profile().supports_input_admission() {
            return Err(refused("store does not support joint Live input admission"));
        }
        let grant_record = grant.record();
        let binding = &grant_record.executor().binding;
        if source.session_id() != &self.session_id || binding.session_id != self.session_id {
            return Err(refused("Live source and executor must bind this session"));
        }
        // Lookup precedes every other observation. No request body is recaptured.
        let source_before = ops
            .lookup_live_source(source)
            .await
            .map_err(internal)?
            .ok_or_else(|| refused("Live source has no committed reservation"))?;
        let LiveSourceEntryRecord::Reservation { record } =
            source_before.record().map_err(internal)?
        else {
            return Err(refused("Live source is not a request reservation"));
        };
        if record.grant() != Some(&grant_record.grant_ref()) {
            return Err(refused("Live source does not bind this issued grant"));
        }
        let input = source_input(&record)?;
        let input_digest: [u8; 32] =
            Sha256::digest(serde_json::to_vec(&input).map_err(internal)?).into();
        match record.disposition() {
            LiveSourceDisposition::Admitted { .. } => {
                return Ok(PendingLiveAdmission {
                    store: Arc::clone(&self.store),
                    input,
                    input_digest,
                    source_before,
                    operation: LiveAdmissionOperation::Observe,
                    runtime_binding,
                    receipt_tx,
                });
            }
            LiveSourceDisposition::Reserved {} => {}
            _ => {
                return Err(refused(
                    "Live source has a non-admission terminal disposition",
                ));
            }
        }
        let before = ops
            .load_live_head(&self.session_id)
            .await
            .map_err(internal)?
            .ok_or_else(|| refused("Live source has no committed owner head"))?;
        before.validate_payload().map_err(internal)?;
        let state = crate::generated::live_request_state::decode(&before.payload.request_snapshot)
            .map_err(internal)?;
        let owner =
            dsl::LiveRequestMachineAuthority::recover_from_state(state).map_err(internal)?;
        if owner.state().grant_record != serde_json::to_string(grant_record).map_err(internal)? {
            return Err(refused(
                "Live grant is not the exact currently activated declaration",
            ));
        }
        let MachineLifecycleObservation::Decoded {
            record: lifecycle,
            version,
        } = self
            .store
            .observe_machine_lifecycle(&LogicalRuntimeId::for_session(&self.session_id))
            .await
            .map_err(internal)?
        else {
            return Err(refused("Live executor has no current runtime lifecycle"));
        };
        if lifecycle.binding().runtime_epoch_id()
            != Some(binding.runtime_epoch.to_string().as_str())
            || lifecycle.binding().runtime_generation() != Some(binding.binding_generation)
            || lifecycle.binding().agent_runtime_id().is_none()
            || lifecycle.binding().fence_token().is_none()
        {
            return Err(refused("Live executor binding is no longer current"));
        }
        let revision = before
            .reference
            .revision
            .checked_add(1)
            .and_then(NonZeroU64::new)
            .ok_or_else(|| internal("Live admission revision overflow"))?;
        let ingress = NonZeroU64::new(before.payload.ingress_generation)
            .ok_or_else(|| internal("invalid Live ingress generation"))?;
        let mut digest = Sha256::new();
        digest.update(b"meerkat.live-input-admission.v1\0");
        digest.update(
            serde_json::to_vec(&(
                &before.reference,
                record.frozen_digest().map_err(internal)?,
                input_digest,
                grant_record.grant_ref(),
                binding,
                ingress,
            ))
            .map_err(internal)?,
        );
        let commit = ExecutionAdmissionCommitRef {
            revision,
            digest: digest.finalize().into(),
        };
        let encoded_commit = serde_json::to_string(&commit).map_err(internal)?;
        let receipt = AdmittedLiveExecutionRecord::new(
            source.clone(),
            input.id().clone(),
            binding.clone(),
            grant_record.grant_ref(),
            ingress,
            commit,
        )
        .map_err(internal)?;
        let completion_budget =
            super::request_credits::RequestCompletionBudget::measured().map_err(internal)?;
        let transcript = crate::live_ledger::transcript_authority::dsl::LiveTranscriptMachineAuthority::recover_from_state(
            crate::generated::live_transcript_state::decode(&before.payload.transcript_snapshot)
                .map_err(internal)?,
        ).map_err(internal)?;
        use crate::live_ledger::transcript_authority::dsl as transcript_dsl;
        let observed = transcript_dsl::LiveTranscriptMachineMutator::apply(
            &mut transcript.prepare_authority(),
            transcript_dsl::LiveTranscriptInput::ObserveSourceIngress {
                channel: source.channel_id().to_string(),
            },
        )
        .map_err(internal)?;
        let [
            transcript_dsl::LiveTranscriptEffect::SourceIngressObserved {
                channel,
                ingress_open,
            },
        ] = observed.effects()
        else {
            return Err(internal("unexpected source ingress observation"));
        };
        if channel != source.channel_id().as_str() {
            return Err(internal("source ingress observation changed channel"));
        }
        let command = dsl::LiveRequestInput::Admit {
            source_ingress_open: *ingress_open,
            request_id: record.request_id().to_string(),
            source: serde_json::to_string(source).map_err(internal)?,
            payload: serde_json::to_string(&record.frozen_digest().map_err(internal)?)
                .map_err(internal)?,
            input_id: input.id().to_string(),
            admission_commit: encoded_commit.clone(),
            ingress_generation: ingress.get(),
            credit_records: completion_budget.envelope.total().records,
            credit_bytes: completion_budget.envelope.total().encoded_bytes,
            snapshot_ceiling: completion_budget.snapshot_ceiling,
            profile_revision: serde_json::to_string(&grant_record.declaration().profile_revision)
                .map_err(internal)?,
            now: (self.clock)().map_err(internal)?,
        };
        let mut candidate = owner.prepare_authority();
        if record.cancellation().is_some() {
            dsl::LiveRequestMachineMutator::apply(
                &mut candidate,
                dsl::LiveRequestInput::Cancel {
                    request_id: record.request_id().to_string(),
                },
            )
            .map_err(internal)?;
        }
        let transition = dsl::LiveRequestMachineMutator::apply(&mut candidate, command.clone())
            .map_err(internal)?;
        if !matches!(transition.effects(), [
            dsl::LiveRequestEffect::InputAdmitted { request_id, input_id, admission_commit }
        ] if request_id == &record.request_id().to_string()
            && input_id == &input.id().to_string()
            && admission_commit == &encoded_commit)
        {
            return Err(internal(
                "generated Live admission returned a mismatched handoff",
            ));
        }
        let prepared = PreparedLiveLedgerCommit::from_request_transition(
            &self.session_id,
            Some(&before),
            &candidate,
        )
        .map_err(internal)?;
        let fence = Arc::new(CurrentLiveRequestFence {
            time: super::LiveRequestTimeFence {
                predecessor: owner,
                input: command,
                expected_snapshot: Arc::clone(&prepared.successor().payload.request_snapshot),
                clock: Arc::clone(&self.clock),
            },
            registration: runtime_binding.fence(&input, binding)?,
        });
        let source_after = LiveSourceRow::encode(&LiveSourceEntryRecord::Reservation {
            record: Box::new(
                (*record)
                    .with_admission(receipt.clone())
                    .map_err(internal)?,
            ),
        })
        .map_err(internal)?;
        Ok(PendingLiveAdmission {
            store: Arc::clone(&self.store),
            input,
            input_digest,
            source_before,
            operation: LiveAdmissionOperation::Admit(Box::new(PreparedLiveInputAdmission {
                prepared,
                source_after,
                lifecycle: version,
                fence,
                receipt,
                callback: None,
            })),
            runtime_binding,
            receipt_tx,
        })
    }
}

impl PendingLiveAdmission {
    pub(crate) fn require_new_admission(&self) -> Result<(), RuntimeDriverError> {
        match self.operation {
            LiveAdmissionOperation::Admit(_) => Ok(()),
            LiveAdmissionOperation::Observe => Err(refused(
                "committed Live source lost its indexed ordinary input",
            )),
        }
    }

    pub(crate) async fn reconcile_existing(
        self,
        existing: &crate::input_state::StoredInputState,
    ) -> Result<(), RuntimeDriverError> {
        let Input::LiveRequest(expected_input) = &self.input else {
            return Err(internal("Live handoff lost its typed input"));
        };
        if existing.state.durability != Some(InputDurability::Durable)
            || existing.state.idempotency_key != expected_input.header.idempotency_key
        {
            return Err(refused(
                "Live source index does not retain its durable admission identity",
            ));
        }
        match &existing.state.persisted_input {
            Some(Input::LiveRequest(input))
                if input.header.id == existing.state.input_id
                    && input.header.source == InputOrigin::LiveRequest
                    && input.header.durability == InputDurability::Durable
                    && input
                        .request
                        .same_submission_content(&expected_input.request) => {}
            None if crate::meerkat_machine::input_seed_behavioral_terminality_via_authority(
                &existing.state.input_id,
                &existing.seed,
            )
            .map_err(internal)? => {}
            _ => {
                return Err(refused(
                    "Live source index does not identify its original input",
                ));
            }
        }
        if expected_input.request.is_callback_continuation() {
            return self.reconcile_callback_existing(existing).await;
        }
        let ops = self.store.live_ledger_ops().ok_or_else(|| {
            internal("Live store capability disappeared during admission reconciliation")
        })?;
        let source = ops
            .lookup_live_source(self.source_before.source())
            .await
            .map_err(internal)?
            .ok_or_else(|| refused("indexed Live input lost its committed source"))?;
        let (
            LiveSourceEntryRecord::Reservation { record },
            LiveSourceEntryRecord::Reservation { record: original },
        ) = (
            source.record().map_err(internal)?,
            self.source_before.record().map_err(internal)?,
        )
        else {
            return Err(refused("indexed Live input has no reservation"));
        };
        if record.frozen_digest().map_err(internal)?
            != original.frozen_digest().map_err(internal)?
        {
            return Err(refused("indexed Live input source content changed"));
        }
        let LiveSourceDisposition::Admitted { receipt } = record.disposition() else {
            return Err(refused("indexed Live input has no committed admission"));
        };
        if receipt.input_id() != &existing.state.input_id {
            return Err(refused("Live source receipt and input index disagree"));
        }
        let head = ops
            .load_live_head(source.source().session_id())
            .await
            .map_err(internal)?
            .ok_or_else(|| refused("indexed Live input has no committed request owner"))?;
        head.validate_payload().map_err(internal)?;
        let state = crate::generated::live_request_state::decode(&head.payload.request_snapshot)
            .map_err(internal)?;
        let mut owner =
            dsl::LiveRequestMachineAuthority::recover_from_state(state).map_err(internal)?;
        let request_id = record.request_id().to_string();
        let input_id = receipt.input_id().to_string();
        let admission_commit =
            serde_json::to_string(receipt.admission_commit()).map_err(internal)?;
        let transition = dsl::LiveRequestMachineMutator::apply(
            &mut owner,
            dsl::LiveRequestInput::ObserveAdmission {
                request_id: request_id.clone(),
                source: serde_json::to_string(record.source()).map_err(internal)?,
                payload: serde_json::to_string(&record.frozen_digest().map_err(internal)?)
                    .map_err(internal)?,
                input_id: input_id.clone(),
                admission_commit: admission_commit.clone(),
                grant_id: receipt.grant().id.as_uuid().to_string(),
                generation: receipt.grant().generation.get(),
                executor: serde_json::to_string(receipt.executor()).map_err(internal)?,
                ingress_generation: receipt.ingress_generation_at_admission().get(),
            },
        )
        .map_err(internal)?;
        if !matches!(transition.effects(), [
            dsl::LiveRequestEffect::AdmissionObserved {
                request_id: observed_request,
                input_id: observed_input,
                admission_commit: observed_commit,
            }
        ] if observed_request == &request_id
            && observed_input == &input_id
            && observed_commit == &admission_commit)
        {
            return Err(internal(
                "generated admission observation did not bind its receipt",
            ));
        }
        return_admission_receipt(self.receipt_tx, receipt.as_ref().clone());
        Ok(())
    }

    pub(crate) fn input(&self) -> &Input {
        &self.input
    }

    pub(crate) fn validation(&self) -> LiveInputAdmissionValidation {
        LiveInputAdmissionValidation {
            input_digest: self.input_digest,
        }
    }

    pub(crate) fn validate_store(
        &self,
        store: &Arc<dyn RuntimeStore>,
    ) -> Result<(), RuntimeDriverError> {
        if !Arc::ptr_eq(&self.store, store) {
            return Err(refused(
                "Live admission handoff belongs to another runtime store",
            ));
        }
        Ok(())
    }

    pub(crate) fn validate_runtime_authority(
        &self,
        authority: &crate::driver::ephemeral::SharedIngressDslAuthority,
    ) -> Result<(), RuntimeDriverError> {
        if !Arc::ptr_eq(&self.runtime_binding.authority, authority) {
            return Err(refused(
                "Live admission handoff belongs to another runtime registration",
            ));
        }
        Ok(())
    }

    pub(crate) async fn commit(
        self,
        records: &[InputStatePersistenceRecord],
    ) -> Result<(), RuntimeDriverError> {
        let [record] = records else {
            return Err(refused(
                "Live admission must persist exactly one ordinary input",
            ));
        };
        let input = record
            .as_stored()
            .state
            .persisted_input
            .as_ref()
            .ok_or_else(|| internal("Live admission ordinary input payload is missing"))?;
        self.validation().validate(input)?;
        let LiveAdmissionOperation::Admit(admission) = self.operation else {
            return Err(refused("an observed admission cannot insert another input"));
        };
        let prepared = match admission.callback {
            Some(callback) => admission.prepared.with_callback_input_admission(
                record.clone(),
                self.source_before,
                admission.lifecycle,
                &callback.origin,
                &callback.results,
            ),
            None => admission.prepared.with_input_admission(
                record.clone(),
                &self.source_before,
                admission.source_after,
                admission.lifecycle,
            ),
        }
        .map_err(internal)?;
        let outcome = self
            .store
            .live_ledger_ops()
            .ok_or_else(|| internal("Live store capability disappeared"))?
            .commit_live_ledger(prepared, admission.fence)
            .await
            .map_err(internal)?;
        if !matches!(outcome, LiveLedgerCommitOutcome::Committed { .. }) {
            return Err(internal(format!(
                "Live input admission did not commit: {outcome:?}"
            )));
        }
        return_admission_receipt(self.receipt_tx, admission.receipt);
        Ok(())
    }
}

fn return_admission_receipt(
    sender: tokio::sync::oneshot::Sender<AdmittedLiveExecutionAuthority>,
    record: AdmittedLiveExecutionRecord,
) {
    let authority =
        AdmittedLiveExecutionAuthority::from_committed_admission(CommittedLiveAdmission { record });
    if sender.send(authority).is_err() {
        tracing::debug!("Live admission is durable after acknowledgement receiver was dropped");
    }
}
