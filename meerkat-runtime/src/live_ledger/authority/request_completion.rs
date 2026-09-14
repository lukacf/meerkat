use super::*;
use crate::completion::CompletionOutcome;
use crate::identifiers::LogicalRuntimeId;
use crate::input_state::{
    InputTerminalCompletionBatchKey, InputTerminalCompletionPhase,
    input_terminal_completion_outcome,
};
use crate::live_ledger::completion::{
    LiveCompletionEvent, LiveCompletionRecord, LiveCompletionText, LiveRequestCompletionFact,
};
use crate::live_ledger::transcript::LiveLedgerFormatV1;
use meerkat_core::live_execution::request::LiveSourceKey;
use meerkat_core::live_observation::LiveObservationSeq;
use meerkat_core::ops::OperationId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LiveRequestCompletionProgress {
    OrdinaryPending,
    RecoveryHeld(super::recovery::LiveInputRecoveryObservation),
    CallbackSuspended(LiveObservationSeq),
    CancelledCallbackHeld(LiveCancelledCallbackHold),
    CallbackUnattributed,
    OrdinaryUnclassified,
    FinalizationFailed,
    Committed(LiveObservationSeq),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LiveCancelledCallbackHold {
    pub input_id: meerkat_core::InputId,
    pub callback: meerkat_core::session::CallbackBatchIdentity,
    pub ordinary_receipt_digest: String,
    pub reason: meerkat_core::live_execution::request::LiveRequestCancellationReason,
    pub callback_sequence: LiveObservationSeq,
}

impl LiveRequestCompletionProgress {
    pub(crate) fn trace_recovery_hold(&self, session_id: &SessionId, request_id: &OperationId) {
        if let Self::RecoveryHeld(hold) = self {
            tracing::warn!(
                %session_id, %request_id, evidence_digest = %hold.evidence_digest,
                disposition = ?hold.disposition,
                "Live request retains unresolved execution; no replay or terminal outcome authorized"
            );
        }
        if let Self::CancelledCallbackHeld(hold) = self {
            tracing::warn!(
                %session_id, %request_id, input_id = %hold.input_id,
                run_id = %hold.callback.run_id(), reason = ?hold.reason,
                ordinary_receipt_digest = %hold.ordinary_receipt_digest,
                "cancelled Live callback remains held; no terminal result, continuation, or body cleanup authorized"
            );
        }
    }
}

pub(crate) struct LiveRequestCompletionReconciliation {
    pub request_id: OperationId,
    pub result: Result<LiveRequestCompletionProgress, LiveRequestAuthorityError>,
}

pub(in crate::live_ledger) struct PreparedLiveRequestCompletion {
    session_id: SessionId,
    preparation: RequestCompletionPreparation,
}

enum RequestCompletionPreparation {
    Observed(LiveRequestCompletionProgress),
    Append {
        progress: LiveRequestCompletionProgress,
        commit: Box<PreparedLiveLedgerCommit>,
        fence: Box<LiveRequestTimeFence>,
    },
}

enum CallbackObservation {
    Unattributed,
    Attributed {
        identity: meerkat_core::session::CallbackBatchIdentity,
        claims: std::collections::BTreeSet<String>,
    },
}

fn unstaged_continuation_run<'a>(
    state: &'a dsl::LiveRequestMachineState,
    request: &str,
) -> Option<&'a String> {
    state.request_runs.get(request).filter(|run| {
        state
            .run_continuation_inputs
            .get(*run)
            .is_some_and(|input| !input.is_empty())
    })
}

pub(super) fn current_request_input<'a>(
    state: &'a dsl::LiveRequestMachineState,
    request: &str,
) -> Option<&'a String> {
    if let Some(run) = unstaged_continuation_run(state, request) {
        return state.run_continuation_inputs.get(run);
    }
    match state.request_runs.get(request) {
        Some(run) => state.run_inputs.get(run),
        None => state.request_inputs.get(request),
    }
}

impl LiveRequestStoreOwner {
    pub(crate) async fn observe_cancelled_callback_hold(
        &self,
        request_id: &OperationId,
    ) -> Result<Option<LiveCancelledCallbackHold>, LiveRequestAuthorityError> {
        let prepared = self.prepare_request_completion(request_id).await?;
        Ok(match prepared.preparation {
            RequestCompletionPreparation::Observed(
                LiveRequestCompletionProgress::CancelledCallbackHeld(hold),
            ) => Some(hold),
            _ => None,
        })
    }

    pub(crate) async fn reconcile_input_completions(
        &self,
        input_ids: Option<&[meerkat_core::lifecycle::InputId]>,
    ) -> Result<Vec<LiveRequestCompletionReconciliation>, LiveRequestAuthorityError> {
        let ops = self
            .store
            .live_ledger_ops()
            .ok_or(LiveRequestAuthorityError::Unsupported)?;
        let Some(head) = ops.load_live_head(&self.session_id).await? else {
            return Ok(Vec::new());
        };
        head.validate_payload()?;
        if head.reference.session_id != self.session_id {
            return Err(LiveRequestAuthorityError::SessionMismatch);
        }
        let owner = dsl::LiveRequestMachineAuthority::recover_from_state(
            crate::generated::live_request_state::decode(&head.payload.request_snapshot)?,
        )?;
        let mut reconciliations = Vec::new();
        for request in &owner.state().admitted_requests {
            let input = current_request_input(owner.state(), request).ok_or(
                LiveRequestAuthorityError::InvalidOrdinaryCompletion(
                    "request has no exact current input",
                ),
            )?;
            if owner.state().request_phases.get(request) == Some(&dsl::LiveRequestPhase::Terminal)
                || input_ids.is_some_and(|ids| !ids.iter().any(|id| id.to_string() == *input))
            {
                continue;
            }
            let request_id = OperationId(uuid::Uuid::parse_str(request).map_err(|error| {
                RuntimeStoreError::ReadFailed(format!(
                    "invalid retained Live request identity: {error}"
                ))
            })?);
            reconciliations.push(LiveRequestCompletionReconciliation {
                result: self.reconcile_request_completion(&request_id).await,
                request_id,
            });
        }
        Ok(reconciliations)
    }

    pub(crate) async fn reconcile_request_completion(
        &self,
        request_id: &OperationId,
    ) -> Result<LiveRequestCompletionProgress, LiveRequestAuthorityError> {
        let prepared = self.prepare_request_completion(request_id).await?;
        let progress = self.commit_request_completion(prepared).await?;
        if matches!(
            progress,
            LiveRequestCompletionProgress::CallbackSuspended(_)
        ) && let Some(hold) = self.observe_cancelled_callback_hold(request_id).await?
        {
            return Ok(LiveRequestCompletionProgress::CancelledCallbackHeld(hold));
        }
        Ok(progress)
    }

    pub(in crate::live_ledger) async fn prepare_request_completion(
        &self,
        request_id: &OperationId,
    ) -> Result<PreparedLiveRequestCompletion, LiveRequestAuthorityError> {
        let invalid = LiveRequestAuthorityError::InvalidOrdinaryCompletion;
        let observed = |progress| PreparedLiveRequestCompletion {
            session_id: self.session_id.clone(),
            preparation: RequestCompletionPreparation::Observed(progress),
        };
        let ops = self
            .store
            .live_ledger_ops()
            .ok_or(LiveRequestAuthorityError::Unsupported)?;
        let head = ops
            .load_live_head(&self.session_id)
            .await?
            .ok_or_else(|| invalid("missing Live request owner"))?;
        if head.reference.session_id != self.session_id {
            return Err(LiveRequestAuthorityError::SessionMismatch);
        }
        head.validate_payload()?;
        let owner = dsl::LiveRequestMachineAuthority::recover_from_state(
            crate::generated::live_request_state::decode(&head.payload.request_snapshot)?,
        )?;
        let state = owner.state();
        let request = request_id.to_string();
        let source: LiveSourceKey = serde_json::from_str(
            state
                .request_sources
                .get(&request)
                .ok_or_else(|| invalid("missing retained request source"))?,
        )?;
        if source.session_id() != &self.session_id {
            return Err(LiveRequestAuthorityError::SessionMismatch);
        }
        let expected_input = current_request_input(state, &request)
            .ok_or_else(|| invalid("request has no exact current input"))?;
        let unstaged_run = unstaged_continuation_run(state, &request);
        let key = if let Some(run) = unstaged_run {
            let callback: meerkat_core::session::CallbackBatchIdentity = serde_json::from_str(
                state
                    .run_callback_records
                    .get(run)
                    .ok_or_else(|| invalid("unstaged continuation lost its callback"))?,
            )?;
            if callback.session_id() != &self.session_id
                || callback.run_id().to_string() != *run
                || state.run_requests.get(run) != Some(&request)
                || state.run_successors.get(run).map(String::as_str) != Some("")
            {
                return Err(invalid(
                    "unstaged continuation has a foreign callback or successor",
                ));
            }
            admission::continuation_input_key(request_id, &callback)
        } else {
            match state.request_runs.get(&request) {
                Some(run) => admission::run_input_key(state, &self.session_id, request_id, run),
                None => admission::source_input_key(&source),
            }
        }
        .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?;
        let input = self
            .store
            .load_input_state_by_idempotency_key(
                &LogicalRuntimeId::for_session(&self.session_id),
                &key,
            )
            .await?
            .ok_or_else(|| invalid("missing admitted ordinary input"))?;
        let stored = input.state();
        let input_id = &stored.state.input_id;
        if expected_input != &input_id.to_string() {
            return Err(invalid("ordinary row differs from the admitted input"));
        }

        let completion_observations = self.observe_completion_batch(&input).await?;
        let completion_rows: Vec<_> = completion_observations
            .iter()
            .map(|row| row.state().clone())
            .collect();
        let Some(outcome) = input_terminal_completion_outcome(&completion_rows, input_id)
            .map_err(|error| RuntimeStoreError::ReadFailed(error.to_string()))?
        else {
            if matches!(
                stored
                    .state
                    .terminal_completion
                    .as_ref()
                    .map(|completion| &completion.phase),
                Some(InputTerminalCompletionPhase::Pending)
            ) {
                return Ok(observed(LiveRequestCompletionProgress::OrdinaryPending));
            }
            let runtime_id = LogicalRuntimeId::for_session(&self.session_id);
            let boundary_committed = match (
                stored.seed.last_run_id.as_ref(),
                stored.seed.last_boundary_sequence,
            ) {
                (Some(run), Some(sequence)) => Some(
                    self.store
                        .load_boundary_receipt(&runtime_id, run, sequence)
                        .await?
                        .is_some(),
                ),
                _ => None,
            };
            let observation = super::recovery::observe_input_recovery(
                self.store.as_ref(),
                &runtime_id,
                stored,
                boundary_committed,
                dsl::LiveInputRecoveryPurpose::ObserveUnfinishedInput,
                Some(&head.reference),
            )
            .await?;
            let progress = match observation {
                Some(super::recovery::LiveInputRecoveryObservation {
                    disposition:
                        dsl::LiveInputRecoveryDisposition::NoBoundRun
                        | dsl::LiveInputRecoveryDisposition::RuntimePending
                        | dsl::LiveInputRecoveryDisposition::AppliedBoundary,
                    ..
                }) => LiveRequestCompletionProgress::OrdinaryPending,
                Some(hold) => LiveRequestCompletionProgress::RecoveryHeld(hold),
                None => LiveRequestCompletionProgress::OrdinaryUnclassified,
            };
            return Ok(observed(progress));
        };
        let completion = stored
            .state
            .terminal_completion
            .as_ref()
            .ok_or_else(|| invalid("finalized outcome lost its owner"))?;
        let InputTerminalCompletionPhase::Finalized {
            receipt_digest,
            finalization,
        } = &completion.phase
        else {
            return Err(invalid("ordinary outcome is not finalized"));
        };
        let run_id = match &completion.batch_key {
            InputTerminalCompletionBatchKey::Run { run_id } => Some(run_id),
            InputTerminalCompletionBatchKey::RuntimeTermination { owner_input_id } => {
                if owner_input_id != &completion.owner_input_id
                    || stored.seed.last_boundary_sequence.is_some()
                {
                    return Err(invalid(
                        "runless completion has a foreign owner or boundary",
                    ));
                }
                None
            }
        };
        let request_run_matches = if unstaged_run.is_some() {
            run_id.is_none()
        } else {
            state.request_runs.get(&request) == run_id.map(ToString::to_string).as_ref()
        };
        if stored.seed.last_run_id.as_ref() != run_id
            || !request_run_matches
            || (run_id.is_some()
                && (completion.owner_input_id != *input_id
                    || completion.completion_input_ids.as_deref()
                        != Some(std::slice::from_ref(input_id))))
        {
            return Err(invalid(
                "ordinary completion differs from the exclusive input/run batch",
            ));
        }
        if *finalization == crate::input_state::InputTerminalCompletionFinalizationVerdict::Failed {
            return Ok(observed(LiveRequestCompletionProgress::FinalizationFailed));
        }
        if run_id.is_none()
            && !matches!(
                outcome,
                CompletionOutcome::RuntimeTerminated { .. }
                    | CompletionOutcome::Cancelled
                    | CompletionOutcome::Abandoned { .. }
                    | CompletionOutcome::AbandonedWithError { .. }
            )
        {
            return Err(invalid("runless completion contains a run-owned outcome"));
        }
        match outcome {
            CompletionOutcome::Completed(ref result) if result.session_id != self.session_id => {
                return Err(invalid("ordinary result belongs to another session"));
            }
            CompletionOutcome::CallbackPending {
                ref tool_use_id,
                ref callback_identity,
                ..
            } => {
                let callback = self.callback_progress(
                    state,
                    head.reference.revision,
                    completion,
                    callback_identity.as_ref(),
                    std::iter::once(tool_use_id.as_str()),
                )?;
                return self.prepare_callback_suspension(
                    &head, owner, &input, callback, &source, request_id,
                );
            }
            CompletionOutcome::CallbackBatchPending {
                ref pending_tool_calls,
                ref callback_identity,
            } => {
                let callback = self.callback_progress(
                    state,
                    head.reference.revision,
                    completion,
                    callback_identity.as_ref(),
                    pending_tool_calls
                        .iter()
                        .map(|call| call.tool_use_id.as_str()),
                )?;
                return self.prepare_callback_suspension(
                    &head, owner, &input, callback, &source, request_id,
                );
            }
            CompletionOutcome::CompletedWithFinalizationFailure { .. } => {
                return Ok(observed(LiveRequestCompletionProgress::FinalizationFailed));
            }
            CompletionOutcome::CompletedWithoutResult => {
                return Ok(observed(
                    LiveRequestCompletionProgress::OrdinaryUnclassified,
                ));
            }
            CompletionOutcome::Completed(_)
            | CompletionOutcome::Cancelled
            | CompletionOutcome::Abandoned { .. }
            | CompletionOutcome::AbandonedWithError { .. }
            | CompletionOutcome::RuntimeTerminated { .. } => {}
        }
        if state.request_ordinary_completion_digests.get(&request) == Some(receipt_digest) {
            let mut candidate = owner.prepare_authority();
            let command = match (run_id, unstaged_run) {
                (Some(run_id), _) => dsl::LiveRequestInput::ObserveRequestCompletion {
                    request_id: request.clone(),
                    run_id: run_id.to_string(),
                    input_id: input_id.to_string(),
                    ordinary_completion_digest: receipt_digest.clone(),
                },
                (None, Some(previous_run)) => {
                    dsl::LiveRequestInput::ObserveUnstagedContinuationCompletion {
                        request_id: request.clone(),
                        previous_run_id: previous_run.clone(),
                        input_id: input_id.to_string(),
                        ordinary_completion_digest: receipt_digest.clone(),
                    }
                }
                (None, None) => dsl::LiveRequestInput::ObserveRunlessCompletion {
                    request_id: request.clone(),
                    input_id: input_id.to_string(),
                    ordinary_completion_digest: receipt_digest.clone(),
                },
            };
            let transition = dsl::LiveRequestMachineMutator::apply(&mut candidate, command)?;
            let [
                dsl::LiveRequestEffect::RequestCompletionObserved {
                    request_id: retained_request,
                    completion_sequence,
                    completion_digest,
                },
            ] = transition.effects()
            else {
                return Err(invalid("generated observation lost its request receipt"));
            };
            if retained_request != &request
                || *completion_sequence > head.reference.event_count
                || state.request_terminal_digests.get(&request) != Some(completion_digest)
            {
                return Err(invalid("generated observation changed its request receipt"));
            }
            return Ok(observed(LiveRequestCompletionProgress::Committed(
                LiveObservationSeq::new(*completion_sequence)
                    .map_err(|_| invalid("invalid retained completion sequence"))?,
            )));
        }
        let next = head
            .reference
            .event_count
            .checked_add(1)
            .ok_or_else(|| invalid("completion sequence overflow"))?;
        let receipt_digest_text = LiveCompletionText::new(receipt_digest.as_str())
            .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?;
        let outcome = match run_id {
            Some(run_id) => LiveRequestCompletionFact::OrdinaryTerminal {
                input_id: input_id.clone(),
                run_id: run_id.clone(),
                receipt_digest: receipt_digest_text,
            },
            None => LiveRequestCompletionFact::OrdinaryRunlessTerminal {
                input_id: input_id.clone(),
                receipt_digest: receipt_digest_text,
            },
        };
        let record = LiveCompletionRecord {
            format: LiveLedgerFormatV1::V1,
            session_id: self.session_id.clone(),
            channel_id: source.channel_id().clone(),
            sequence: LiveObservationSeq::new(next)
                .map_err(|_| invalid("invalid completion sequence"))?,
            event: LiveCompletionEvent::RequestOutcome {
                request_id: request_id.clone(),
                outcome,
            },
        };
        let charge = record
            .encode()
            .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?
            .charge();
        let completion_digest = request_credits::completion_digest(&record)?;
        let command = match (run_id, unstaged_run) {
            (Some(run_id), _) => dsl::LiveRequestInput::Complete {
                request_id: request.clone(),
                run_id: run_id.to_string(),
                input_id: input_id.to_string(),
                ordinary_completion_digest: receipt_digest.clone(),
                completion_records: charge.records,
                completion_bytes: charge.encoded_bytes,
                completion_sequence: record.sequence.get(),
                completion_digest,
            },
            (None, Some(previous_run)) => dsl::LiveRequestInput::CompleteUnstagedContinuation {
                request_id: request.clone(),
                previous_run_id: previous_run.clone(),
                input_id: input_id.to_string(),
                ordinary_completion_digest: receipt_digest.clone(),
                completion_records: charge.records,
                completion_bytes: charge.encoded_bytes,
                completion_sequence: record.sequence.get(),
                completion_digest,
            },
            (None, None) => dsl::LiveRequestInput::CompleteRunless {
                request_id: request.clone(),
                input_id: input_id.to_string(),
                ordinary_completion_digest: receipt_digest.clone(),
                completion_records: charge.records,
                completion_bytes: charge.encoded_bytes,
                completion_sequence: record.sequence.get(),
                completion_digest,
            },
        };
        let mut candidate = owner.prepare_authority();
        let transition = dsl::LiveRequestMachineMutator::apply(&mut candidate, command.clone())?;
        let exact_effect = match (run_id, transition.effects()) {
            (
                Some(run_id),
                [
                    dsl::LiveRequestEffect::RequestCompleted {
                        request_id: completed_request,
                        run_id: completed_run,
                    },
                ],
            ) => completed_request == &request && completed_run == &run_id.to_string(),
            (
                None,
                [
                    dsl::LiveRequestEffect::RunlessRequestCompleted {
                        request_id: completed_request,
                        input_id: completed_input,
                    },
                ],
            ) => completed_request == &request && completed_input == &input_id.to_string(),
            _ => false,
        };
        if !exact_effect {
            return Err(invalid(
                "generated completion changed the exact request/input/run",
            ));
        }
        let sequence = record.sequence;
        let commit = PreparedLiveLedgerCommit::from_request_transition_with_completions(
            &self.session_id,
            Some(&head),
            &candidate,
            vec![record],
        )?
        .with_completion_batch_fence(&completion_observations, input_id)?;
        let fence = LiveRequestTimeFence {
            predecessor: owner,
            input: command,
            expected_snapshot: Arc::clone(&commit.successor().payload.request_snapshot),
            clock: Arc::clone(&self.clock),
        };
        Ok(PreparedLiveRequestCompletion {
            session_id: self.session_id.clone(),
            preparation: RequestCompletionPreparation::Append {
                progress: LiveRequestCompletionProgress::Committed(sequence),
                commit: Box::new(commit),
                fence: Box::new(fence),
            },
        })
    }

    async fn observe_completion_batch(
        &self,
        input: &crate::store::ExactInputStateObservation,
    ) -> Result<Vec<crate::store::ExactInputStateObservation>, LiveRequestAuthorityError> {
        let invalid = LiveRequestAuthorityError::InvalidOrdinaryCompletion;
        let Some(completion) = &input.state().state.terminal_completion else {
            return Ok(vec![input.clone()]);
        };
        if matches!(
            completion.batch_key,
            InputTerminalCompletionBatchKey::Run { .. }
        ) || completion.completion_input_ids.as_deref()
            == Some(std::slice::from_ref(&input.state().state.input_id))
        {
            return Ok(vec![input.clone()]);
        }
        let runtime_id = LogicalRuntimeId::for_session(&self.session_id);
        let owner_rows = self
            .store
            .load_input_states_by_ids_with_versions(
                &runtime_id,
                std::slice::from_ref(&completion.owner_input_id),
            )
            .await?;
        let [Some(owner)] = owner_rows.as_slice() else {
            return Err(invalid("runless completion lost its exact batch owner"));
        };
        if owner.state().state.input_id != completion.owner_input_id {
            return Err(invalid("runless completion lookup returned another owner"));
        }
        let ids = owner
            .state()
            .state
            .terminal_completion
            .as_ref()
            .and_then(|owner| owner.completion_input_ids.as_deref())
            .ok_or_else(|| invalid("runless completion owner lost its recipient set"))?;
        let observations = self
            .store
            .load_input_states_by_ids_with_versions(&runtime_id, ids)
            .await?;
        if observations.len() != ids.len() {
            return Err(invalid(
                "runless completion lookup returned a partial batch",
            ));
        }
        let observations = observations
            .into_iter()
            .zip(ids)
            .map(|(row, id)| {
                let row = row.ok_or_else(|| invalid("runless completion lost a recipient"))?;
                if &row.state().state.input_id != id {
                    return Err(invalid(
                        "runless completion lookup reordered its recipients",
                    ));
                }
                Ok(row)
            })
            .collect::<Result<Vec<_>, LiveRequestAuthorityError>>()?;
        if observations
            .iter()
            .find(|row| row.state().state.input_id == input.state().state.input_id)
            .is_none_or(|row| row.exact_row_digest() != input.exact_row_digest())
        {
            return Err(invalid(
                "runless completion changed its admitted input during discovery",
            ));
        }
        Ok(observations)
    }

    fn callback_progress<'a>(
        &self,
        state: &dsl::LiveRequestMachineState,
        head_revision: u64,
        completion: &crate::input_state::InputTerminalCompletion,
        identity: Option<&meerkat_core::session::CallbackBatchIdentity>,
        pending_ids: impl Iterator<Item = &'a str>,
    ) -> Result<CallbackObservation, LiveRequestAuthorityError> {
        let invalid = LiveRequestAuthorityError::InvalidOrdinaryCompletion;
        let Some(identity) = identity else {
            return Ok(CallbackObservation::Unattributed);
        };
        let run_id = completion
            .batch_key
            .run_id()
            .ok_or_else(|| invalid("callback has no run"))?;
        let request = state
            .run_requests
            .get(&run_id.to_string())
            .ok_or_else(|| invalid("callback run has no Live request"))?;
        let scope = identity
            .execution_scope()
            .ok_or_else(|| invalid("Live callback has no scope"))?;
        if identity.session_id() != &self.session_id
            || identity.run_id() != run_id
            || state.run_inputs.get(&run_id.to_string()) != Some(&completion.input_id.to_string())
            || state.run_scopes.get(&run_id.to_string()) != Some(&scope.as_uuid().to_string())
        {
            return Err(invalid(
                "callback identity differs from its retained request scope",
            ));
        }
        let mut seen = std::collections::BTreeSet::new();
        let mut claims = std::collections::BTreeSet::new();
        for call_id in pending_ids {
            if call_id.is_empty() || !seen.insert(call_id) {
                return Err(invalid("callback set has empty or duplicate call IDs"));
            }
            let effect_id = match identity.scoped_tool_effect_id(call_id) {
                Ok(Some(effect)) => effect,
                Err(
                    meerkat_core::session::CallbackBatchObservationError::EffectIdentityUnavailable,
                ) => {
                    return Ok(CallbackObservation::Unattributed);
                }
                _ => return Err(invalid("callback effect identity is invalid")),
            };
            let effect_id = effect_id.to_string();
            let key = state
                .claim_effects
                .iter()
                .find_map(|(key, effect)| (effect == &effect_id).then_some(key))
                .ok_or_else(|| invalid("callback has no exact committed physical claim"))?;
            let claim = claim::read_retained_claim(state, head_revision, key)?;
            if claim.request_id.to_string() != *request
                || claim.scope_id != scope
                || claim.run_id != *run_id
                || claim.input_id != completion.input_id
                || !matches!(&claim.target,
                    meerkat_core::execution_scope::ScopedEffectTarget::ToolDispatch { call_id: retained, .. }
                        if retained == call_id)
                || !matches!(
                    state.claim_phases.get(key),
                    Some(
                        dsl::LiveEffectPhase::Succeeded
                            | dsl::LiveEffectPhase::Failed
                            | dsl::LiveEffectPhase::Cancelled
                            | dsl::LiveEffectPhase::Unknown
                    )
                )
            {
                return Err(invalid(
                    "callback does not identify an observed physical tool claim",
                ));
            }
            claims.insert(key.clone());
        }
        if seen.is_empty() {
            return Err(invalid("callback set is empty"));
        }
        Ok(CallbackObservation::Attributed {
            identity: identity.clone(),
            claims,
        })
    }

    fn prepare_callback_suspension(
        &self,
        head: &crate::live_ledger::write::LiveLedgerStoredHead,
        owner: dsl::LiveRequestMachineAuthority,
        input: &crate::store::ExactInputStateObservation,
        callback: CallbackObservation,
        source: &LiveSourceKey,
        request_id: &OperationId,
    ) -> Result<PreparedLiveRequestCompletion, LiveRequestAuthorityError> {
        let observed = |progress| PreparedLiveRequestCompletion {
            session_id: self.session_id.clone(),
            preparation: RequestCompletionPreparation::Observed(progress),
        };
        let CallbackObservation::Attributed { identity, claims } = callback else {
            return Ok(observed(
                LiveRequestCompletionProgress::CallbackUnattributed,
            ));
        };
        let invalid = LiveRequestAuthorityError::InvalidOrdinaryCompletion;
        let completion = input
            .state()
            .state
            .terminal_completion
            .as_ref()
            .ok_or_else(|| invalid("callback lost its ordinary completion"))?;
        let InputTerminalCompletionPhase::Finalized { receipt_digest, .. } = &completion.phase
        else {
            return Err(invalid("callback completion is not finalized"));
        };
        let run = identity.run_id().to_string();
        let request = request_id.to_string();
        let callback_record = serde_json::to_string(&identity)?;
        let digest =
            callback_credits::suspension_digest(&callback_record, receipt_digest, &claims)?;
        let state = owner.state();
        if state
            .run_callback_records
            .get(&run)
            .is_some_and(|record| !record.is_empty())
        {
            let mut candidate = owner.prepare_authority();
            let cancellation = state
                .source_cancellations
                .get(&serde_json::to_string(source)?);
            let command = if let Some(reason) = cancellation {
                dsl::LiveRequestInput::ObserveCancelledCallback {
                    request_id: request.clone(),
                    run_id: run.clone(),
                    callback_record,
                    ordinary_completion_digest: receipt_digest.clone(),
                    callback_claims: claims,
                    completion_digest: digest,
                    reason: *reason,
                }
            } else {
                dsl::LiveRequestInput::ObserveCallbackSuspension {
                    request_id: request.clone(),
                    run_id: run.clone(),
                    callback_record,
                    ordinary_completion_digest: receipt_digest.clone(),
                    callback_claims: claims,
                    completion_digest: digest,
                }
            };
            let transition = dsl::LiveRequestMachineMutator::apply(&mut candidate, command)?;
            let sequence = *state
                .run_callback_sequences
                .get(&run)
                .ok_or_else(|| invalid("missing callback sequence"))?;
            if sequence > head.reference.event_count {
                return Err(invalid("callback receipt exceeds retained event head"));
            }
            let sequence = LiveObservationSeq::new(sequence)
                .map_err(|_| invalid("invalid callback sequence"))?;
            let progress = match transition.effects() {
                [
                    dsl::LiveRequestEffect::RequestSuspended {
                        request_id: observed_request,
                        run_id: observed_run,
                    },
                ] if observed_request == &request
                    && observed_run == &run
                    && cancellation.is_none() =>
                {
                    LiveRequestCompletionProgress::CallbackSuspended(sequence)
                }
                [
                    dsl::LiveRequestEffect::CancelledCallbackHeld {
                        request_id: observed_request,
                        run_id: observed_run,
                        reason,
                    },
                ] if observed_request == &request
                    && observed_run == &run
                    && cancellation == Some(reason) =>
                {
                    LiveRequestCompletionProgress::CancelledCallbackHeld(
                        LiveCancelledCallbackHold {
                            input_id: input.state().state.input_id.clone(),
                            callback: identity,
                            ordinary_receipt_digest: receipt_digest.clone(),
                            reason: *reason,
                            callback_sequence: sequence,
                        },
                    )
                }
                _ => {
                    return Err(invalid(
                        "generated callback observation changed its identity or purpose",
                    ));
                }
            };
            return Ok(observed(progress));
        }
        let mut spent_records = state.claim_credit_spent_records.clone();
        let mut spent_bytes = state.claim_credit_spent_bytes.clone();
        let mut records = Vec::with_capacity(claims.len());
        let mut next = head.reference.event_count;
        for claim in &claims {
            next = next
                .checked_add(1)
                .ok_or_else(|| invalid("callback sequence overflow"))?;
            let record = LiveCompletionRecord {
                format: LiveLedgerFormatV1::V1,
                session_id: self.session_id.clone(),
                channel_id: source.channel_id().clone(),
                sequence: LiveObservationSeq::new(next)
                    .map_err(|_| invalid("invalid callback sequence"))?,
                event: LiveCompletionEvent::CallbackSuspended {
                    claim_id: OperationId(
                        uuid::Uuid::parse_str(claim)
                            .map_err(|_| invalid("callback claim is not a canonical UUID"))?,
                    ),
                    request_id: request_id.clone(),
                    input_id: completion.input_id.clone(),
                    run_id: identity.run_id().clone(),
                    batch_digest:
                        meerkat_core::live_execution::evidence::LiveContentDigest::from_sha256(
                            *identity.batch_digest(),
                        ),
                },
            };
            let charge = record
                .encode()
                .map_err(|error| RuntimeStoreError::WriteFailed(error.to_string()))?
                .charge();
            let record_spend = spent_records
                .get_mut(claim)
                .ok_or_else(|| invalid("missing callback record credit"))?;
            *record_spend = record_spend
                .checked_add(charge.records)
                .ok_or_else(|| invalid("callback record spend overflow"))?;
            let byte_spend = spent_bytes
                .get_mut(claim)
                .ok_or_else(|| invalid("missing callback byte credit"))?;
            *byte_spend = byte_spend
                .checked_add(charge.encoded_bytes)
                .ok_or_else(|| invalid("callback byte spend overflow"))?;
            records.push(record);
        }
        let command = dsl::LiveRequestInput::Suspend {
            request_id: request.clone(),
            run_id: run.clone(),
            callback_record,
            ordinary_completion_digest: receipt_digest.clone(),
            callback_claims: claims,
            spent_records,
            spent_bytes,
            completion_sequence: next,
            completion_digest: digest,
        };
        let mut candidate = owner.prepare_authority();
        let transition = dsl::LiveRequestMachineMutator::apply(&mut candidate, command.clone())?;
        if !matches!(transition.effects(), [dsl::LiveRequestEffect::RequestSuspended {
            request_id: suspended_request, run_id: suspended_run,
        }] if suspended_request == &request && suspended_run == &run)
        {
            return Err(invalid("generated callback suspension changed its run"));
        }
        let commit = PreparedLiveLedgerCommit::from_request_transition_with_completions(
            &self.session_id,
            Some(head),
            &candidate,
            records,
        )?
        .with_completion_fence(input)?;
        let fence = LiveRequestTimeFence {
            predecessor: owner,
            input: command,
            expected_snapshot: Arc::clone(&commit.successor().payload.request_snapshot),
            clock: Arc::clone(&self.clock),
        };
        Ok(PreparedLiveRequestCompletion {
            session_id: self.session_id.clone(),
            preparation: RequestCompletionPreparation::Append {
                progress: LiveRequestCompletionProgress::CallbackSuspended(
                    LiveObservationSeq::new(next)
                        .map_err(|_| invalid("invalid callback sequence"))?,
                ),
                commit: Box::new(commit),
                fence: Box::new(fence),
            },
        })
    }

    pub(in crate::live_ledger) async fn commit_request_completion(
        &self,
        prepared: PreparedLiveRequestCompletion,
    ) -> Result<LiveRequestCompletionProgress, LiveRequestAuthorityError> {
        if prepared.session_id != self.session_id {
            return Err(LiveRequestAuthorityError::SessionMismatch);
        }
        match prepared.preparation {
            RequestCompletionPreparation::Observed(progress) => Ok(progress),
            RequestCompletionPreparation::Append {
                progress,
                commit,
                fence,
            } => {
                let expected = commit.successor().reference.clone();
                let ops = self
                    .store
                    .live_ledger_ops()
                    .ok_or(LiveRequestAuthorityError::Unsupported)?;
                match ops.commit_live_ledger(*commit, Arc::new(*fence)).await? {
                    LiveLedgerCommitOutcome::Committed { head }
                    | LiveLedgerCommitOutcome::AlreadyCommitted { head }
                        if head == expected =>
                    {
                        Ok(progress)
                    }
                    outcome => Err(LiveRequestAuthorityError::NotNewlyCommitted(Box::new(
                        outcome,
                    ))),
                }
            }
        }
    }
}
