use super::*;
use crate::live_grant::LiveExecutionGrantRecord;
use crate::store::{CommittedCallbackResultsObservation, ExactInputStateObservation};
use meerkat_core::execution_scope::{
    RunEffectScopeId, RunEffectScopeRecord, ScopedCallbackContinuationRecord, ScopedEffectBudget,
};
use meerkat_core::lifecycle::RunId;
use meerkat_core::session::StagedCallbackResultsObservation;

impl LiveRequestStoreOwner {
    pub(crate) async fn prepare_callback_input_admission(
        &self,
        source: &LiveSourceKey,
        results: CommittedCallbackResultsObservation,
        runtime_binding: LiveAdmissionRuntimeBinding,
        receipt_tx: tokio::sync::oneshot::Sender<AdmittedLiveExecutionAuthority>,
    ) -> Result<PendingLiveAdmission, RuntimeDriverError> {
        let ops = self
            .store
            .live_ledger_ops()
            .filter(|ops| {
                ops.ledger_write_profile().supports_input_admission()
                    && ops.ledger_write_profile().supports_execution_fence()
            })
            .ok_or_else(|| refused("callback admission requires joint execution-fenced storage"))?;
        if source.session_id() != &self.session_id
            || results.target().session_id() != &self.session_id
            || results.authority().session_id() != &self.session_id
        {
            return Err(refused("callback observation belongs to another session"));
        }
        let source_before = ops
            .lookup_live_source(source)
            .await
            .map_err(internal)?
            .ok_or_else(|| refused("callback request lost its retained source"))?;
        let LiveSourceEntryRecord::Reservation { record } =
            source_before.record().map_err(internal)?
        else {
            return Err(refused("callback request has no retained reservation"));
        };
        let LiveSourceDisposition::Admitted { receipt: original } = record.disposition() else {
            return Err(refused("callback request has no original admission"));
        };
        let before = ops
            .load_live_head(&self.session_id)
            .await
            .map_err(internal)?
            .ok_or_else(|| refused("callback request has no Live owner"))?;
        before.validate_payload().map_err(internal)?;
        let owner = dsl::LiveRequestMachineAuthority::recover_from_state(
            crate::generated::live_request_state::decode(&before.payload.request_snapshot)
                .map_err(internal)?,
        )
        .map_err(internal)?;
        let state = owner.state();
        let request = record.request_id().to_string();
        let run = results.target().run_id().to_string();
        let callback_record = serde_json::to_string(results.target()).map_err(internal)?;
        if state.run_requests.get(&run) != Some(&request)
            || state.run_callback_records.get(&run) != Some(&callback_record)
            || state.request_sources.get(&request)
                != Some(&serde_json::to_string(source).map_err(internal)?)
            || state.request_payloads.get(&request)
                != Some(
                    &serde_json::to_string(&record.frozen_digest().map_err(internal)?)
                        .map_err(internal)?,
                )
        {
            return Err(refused("callback target differs from its retained request"));
        }
        let retained_input = state
            .run_continuation_inputs
            .get(&run)
            .ok_or_else(|| refused("callback run lost its continuation slot"))?;
        let historical = !retained_input.is_empty();
        let digest = match results.results() {
            StagedCallbackResultsObservation::Complete(complete) => {
                if complete.identity() != results.target() {
                    return Err(refused("callback result identity differs"));
                }
                *complete.digest()
            }
            StagedCallbackResultsObservation::AlreadyApplied { results_digest, .. }
                if historical =>
            {
                results_digest.ok_or_else(|| {
                    refused("historical applied callback lacks a retained complete-result digest")
                })?
            }
            StagedCallbackResultsObservation::Incomplete { .. } => {
                return Err(refused("callback results are incomplete"));
            }
            StagedCallbackResultsObservation::AlreadyApplied { .. } => {
                return Err(refused(
                    "applied callback has no committed continuation admission",
                ));
            }
        };
        let input_id = if historical {
            InputId::from_uuid(uuid::Uuid::parse_str(retained_input).map_err(internal)?)
        } else {
            InputId::new()
        };
        let commit = if historical {
            serde_json::from_str(
                state
                    .run_continuation_admission_commits
                    .get(&run)
                    .ok_or_else(|| refused("callback admission receipt is missing"))?,
            )
            .map_err(internal)?
        } else {
            let revision = before
                .reference
                .revision
                .checked_add(1)
                .and_then(NonZeroU64::new)
                .ok_or_else(|| internal("callback admission revision overflow"))?;
            let mut hash = Sha256::new();
            hash.update(b"meerkat.live-callback-admission.v1\0");
            hash.update(
                serde_json::to_vec(&(
                    &before.reference,
                    record.frozen_digest().map_err(internal)?,
                    &input_id,
                    results.target(),
                    digest,
                    original.grant(),
                    original.executor(),
                ))
                .map_err(internal)?,
            );
            ExecutionAdmissionCommitRef {
                revision,
                digest: hash.finalize().into(),
            }
        };
        let mut input = source_input(&record)?;
        let Input::LiveRequest(live) = &mut input else {
            return Err(internal("callback input template lost its Live type"));
        };
        live.header.id = input_id.clone();
        live.header.idempotency_key = Some(continuation_input_key(
            record.request_id(),
            results.target(),
        )?);
        let (provenance, source_row) = live.request.source_reference();
        live.request = LiveExecutionRequestRecord::CallbackContinuation {
            provenance: provenance.clone(),
            source_row: *source_row,
            continuation: ScopedCallbackContinuationRecord {
                target: results.target().clone(),
                results_digest: digest,
            },
            admission_commit: commit.clone(),
        };
        let input_digest = Sha256::digest(serde_json::to_vec(&input).map_err(internal)?).into();
        if historical {
            live_reference(&input)?
                .validate_continuation_binding(state, &input_id)
                .map_err(internal)?;
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
        if record.cancellation().is_some() {
            return Err(refused(
                "cancelled Live source cannot admit a callback continuation",
            ));
        }
        let grant: LiveExecutionGrantRecord<()> = serde_json::from_str(
            state
                .request_grant_records
                .get(&request)
                .ok_or_else(|| refused("callback request lost its grant"))?,
        )
        .map_err(internal)?;
        if !matches!(
            grant.executor().selector,
            meerkat_core::live_execution::activation::LiveExecutorSelector::Session { .. }
        ) || grant.grant_ref() != *original.grant()
            || grant.executor().binding != *original.executor()
        {
            return Err(refused(
                "callback admission requires its exact session grant",
            ));
        }
        let runtime_id = LogicalRuntimeId::for_session(&self.session_id);
        let MachineLifecycleObservation::Decoded {
            record: lifecycle,
            version,
        } = self
            .store
            .observe_machine_lifecycle(&runtime_id)
            .await
            .map_err(internal)?
        else {
            return Err(refused("callback executor has no decoded lifecycle"));
        };
        if lifecycle.binding().runtime_generation() != Some(original.executor().binding_generation)
            || lifecycle.binding().runtime_epoch_id()
                != Some(original.executor().runtime_epoch.to_string().as_str())
            || lifecycle.binding().agent_runtime_id().is_none()
            || lifecycle.binding().fence_token().is_none()
        {
            return Err(refused("callback executor binding is no longer current"));
        }
        let origin: ExactInputStateObservation = self
            .store
            .load_input_state_by_idempotency_key(
                &runtime_id,
                &run_input_key(state, &self.session_id, record.request_id(), &run)?,
            )
            .await
            .map_err(internal)?
            .ok_or_else(|| refused("callback lost its originating input"))?;
        if state.run_inputs.get(&run) != Some(&origin.state().state.input_id.to_string()) {
            return Err(refused("callback origin differs from its exact run input"));
        }
        let result_digest: sha2::digest::Output<Sha256> = digest.into();
        let mut command = dsl::LiveRequestInput::AdmitCallbackContinuation {
            request_id: request.clone(),
            run_id: run.clone(),
            callback_record,
            result_digest: format!("{result_digest:x}"),
            input_id: input_id.to_string(),
            admission_commit: serde_json::to_string(&commit).map_err(internal)?,
            executor: serde_json::to_string(original.executor()).map_err(internal)?,
            profile_revision: serde_json::to_string(&grant.declaration().profile_revision)
                .map_err(internal)?,
            stage_credit_bytes: 1,
            now: (self.clock)().map_err(internal)?,
        };
        let mut probe = owner.prepare_authority();
        dsl::LiveRequestMachineMutator::apply(&mut probe, command.clone()).map_err(internal)?;
        let stage_credit = measured_stage_growth(&probe, &input, &run, &command)?;
        let dsl::LiveRequestInput::AdmitCallbackContinuation {
            stage_credit_bytes, ..
        } = &mut command
        else {
            return Err(internal("callback admission command changed"));
        };
        *stage_credit_bytes = stage_credit;
        let mut candidate = owner.prepare_authority();
        let transition = dsl::LiveRequestMachineMutator::apply(&mut candidate, command.clone())
            .map_err(internal)?;
        if !matches!(transition.effects(), [dsl::LiveRequestEffect::CallbackContinuationAdmitted {
            request_id, run_id, input_id: admitted, ..
        }] if request_id == &request && run_id == &run && admitted == &input_id.to_string())
        {
            return Err(internal(
                "callback admission owner returned another identity",
            ));
        }
        let prepared = PreparedLiveLedgerCommit::from_request_transition(
            &self.session_id,
            Some(&before),
            &candidate,
        )
        .map_err(internal)?;
        let fence = Arc::new(CurrentLiveRequestFence {
            time: super::super::LiveRequestTimeFence {
                predecessor: owner,
                input: command,
                expected_snapshot: Arc::clone(&prepared.successor().payload.request_snapshot),
                clock: Arc::clone(&self.clock),
            },
            registration: runtime_binding.fence(&input, original.executor())?,
        });
        let receipt = AdmittedLiveExecutionRecord::new(
            source.clone(),
            input_id,
            original.executor().clone(),
            original.grant().clone(),
            original.ingress_generation_at_admission(),
            commit,
        )
        .map_err(internal)?;
        Ok(PendingLiveAdmission {
            store: Arc::clone(&self.store),
            input,
            input_digest,
            source_before: source_before.clone(),
            operation: LiveAdmissionOperation::Admit(Box::new(PreparedLiveInputAdmission {
                prepared,
                source_after: source_before,
                lifecycle: version,
                fence,
                receipt,
                callback: Some(CallbackAdmissionFence { origin, results }),
            })),
            runtime_binding,
            receipt_tx,
        })
    }
}

fn live_reference(input: &Input) -> Result<&LiveExecutionRequestRecord, RuntimeDriverError> {
    match input {
        Input::LiveRequest(input) => Ok(&input.request),
        _ => Err(internal("callback admission lost its typed input")),
    }
}

// The disposable candidate measures encoding only. Its sizing IDs never enter
// storage or authorize a run; the ordinary runtime still allocates the real run.
fn measured_stage_growth(
    admitted: &dsl::LiveRequestMachinePreparedAuthority,
    input: &Input,
    previous_run: &str,
    admission: &dsl::LiveRequestInput,
) -> Result<u64, RuntimeDriverError> {
    let LiveExecutionRequestRecord::CallbackContinuation {
        continuation,
        admission_commit,
        ..
    } = live_reference(input)?
    else {
        return Err(internal("callback sizing lost its reference"));
    };
    let state = admitted.state();
    let mut scope: RunEffectScopeRecord = serde_json::from_str(
        state
            .run_scope_records
            .get(previous_run)
            .ok_or_else(|| refused("callback sizing lost its prior scope"))?,
    )
    .map_err(internal)?;
    let remaining = *state
        .remaining_effects
        .get(&scope.request_id.to_string())
        .ok_or_else(|| refused("callback sizing lost its effect budget"))?;
    scope.input_id = input.id().clone();
    scope.run_id = RunId::new();
    scope.admission_commit = admission_commit.clone();
    scope.callback_continuation = Some(continuation.clone());
    scope.remaining = ScopedEffectBudget {
        model_computations: remaining,
        tool_dispatches: remaining,
        descendant_admissions: remaining,
    };
    let dsl::LiveRequestInput::AdmitCallbackContinuation {
        executor,
        profile_revision,
        now,
        result_digest,
        ..
    } = admission
    else {
        return Err(internal("callback sizing requires admission"));
    };
    let before = crate::generated::live_request_state::encode(state)
        .map_err(internal)?
        .len();
    let mut staged = dsl::LiveRequestMachineAuthority::recover_from_state(state.clone())
        .map_err(internal)?
        .prepare_authority();
    dsl::LiveRequestMachineMutator::apply(
        &mut staged,
        dsl::LiveRequestInput::StageCallbackContinuation {
            request_id: scope.request_id.to_string(),
            previous_run_id: previous_run.into(),
            input_id: input.id().to_string(),
            admission_commit: serde_json::to_string(admission_commit).map_err(internal)?,
            result_digest: result_digest.clone(),
            run_id: scope.run_id.to_string(),
            scope_id: RunEffectScopeId::from_uuid(uuid::Uuid::new_v4())
                .as_uuid()
                .to_string(),
            scope_record: serde_json::to_string(&scope).map_err(internal)?,
            executor: executor.clone(),
            profile_revision: profile_revision.clone(),
            now: *now,
        },
    )
    .map_err(internal)?;
    let after = crate::generated::live_request_state::encode(staged.state())
        .map_err(internal)?
        .len();
    u64::try_from(
        after
            .checked_sub(before)
            .ok_or_else(|| internal("callback stage unexpectedly shrank its sizing image"))?
            .max(1),
    )
    .map_err(internal)
}

impl PendingLiveAdmission {
    pub(super) async fn reconcile_callback_existing(
        self,
        existing: &crate::input_state::StoredInputState,
    ) -> Result<(), RuntimeDriverError> {
        let LiveExecutionRequestRecord::CallbackContinuation {
            provenance,
            continuation,
            ..
        } = live_reference(&self.input)?
        else {
            return Err(internal("callback reconciliation lost its content"));
        };
        let ops = self
            .store
            .live_ledger_ops()
            .ok_or_else(|| internal("Live store disappeared"))?;
        let head = ops
            .load_live_head(provenance.source().session_id())
            .await
            .map_err(internal)?
            .ok_or_else(|| refused("callback reconciliation lost its owner"))?;
        head.validate_payload().map_err(internal)?;
        let mut owner = dsl::LiveRequestMachineAuthority::recover_from_state(
            crate::generated::live_request_state::decode(&head.payload.request_snapshot)
                .map_err(internal)?,
        )
        .map_err(internal)?;
        if let Some(Input::LiveRequest(input)) = &existing.state.persisted_input {
            input
                .request
                .validate_continuation_binding(owner.state(), &existing.state.input_id)
                .map_err(internal)?;
        }
        let run = continuation.target.run_id().to_string();
        let commit: ExecutionAdmissionCommitRef = serde_json::from_str(
            owner
                .state()
                .run_continuation_admission_commits
                .get(&run)
                .ok_or_else(|| refused("callback reconciliation lost its receipt"))?,
        )
        .map_err(internal)?;
        let digest: sha2::digest::Output<Sha256> = continuation.results_digest.into();
        let transition = dsl::LiveRequestMachineMutator::apply(
            &mut owner,
            dsl::LiveRequestInput::ObserveCallbackContinuation {
                request_id: provenance.request_id().to_string(),
                run_id: run,
                callback_record: serde_json::to_string(&continuation.target).map_err(internal)?,
                result_digest: format!("{digest:x}"),
                input_id: existing.state.input_id.to_string(),
                admission_commit: serde_json::to_string(&commit).map_err(internal)?,
            },
        )
        .map_err(internal)?;
        if !matches!(
            transition.effects(),
            [dsl::LiveRequestEffect::CallbackContinuationObserved { .. }]
        ) {
            return Err(internal(
                "callback observation did not return a historical receipt",
            ));
        }
        let LiveSourceEntryRecord::Reservation { record } =
            self.source_before.record().map_err(internal)?
        else {
            return Err(refused("callback source has no original receipt"));
        };
        let LiveSourceDisposition::Admitted { receipt: original } = record.disposition() else {
            return Err(refused("callback source is not admitted"));
        };
        let receipt = AdmittedLiveExecutionRecord::new(
            provenance.source().clone(),
            existing.state.input_id.clone(),
            original.executor().clone(),
            original.grant().clone(),
            original.ingress_generation_at_admission(),
            commit,
        )
        .map_err(internal)?;
        return_admission_receipt(self.receipt_tx, receipt);
        Ok(())
    }
}
