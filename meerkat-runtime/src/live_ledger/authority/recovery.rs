use super::*;
use crate::RuntimeDriverError;
use crate::identifiers::LogicalRuntimeId;
use crate::input::{Input, InputDurability, InputOrigin};
use crate::input_state::{InputLifecycleState, StoredInputState};
use crate::live_source::{LiveSourceDisposition, LiveSourceEntryRecord};
use crate::store::live_read::{LiveCompositeReadRequest, read_live_composite};
use meerkat_core::execution_scope::{RunEffectScopeId, RunEffectScopeRecord};
use meerkat_core::session::StagedCallbackResultsObservation;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LiveInputRecoveryObservation {
    pub disposition: dsl::LiveInputRecoveryDisposition,
    pub evidence_digest: String,
}

fn corrupt(error: impl std::fmt::Display) -> RuntimeDriverError {
    RuntimeDriverError::RecoveryCorruption {
        reason: format!("Live input recovery: {error}"),
    }
}

fn backoff(error: impl std::fmt::Display) -> RuntimeDriverError {
    RuntimeDriverError::RecoveryBackoff {
        reason: format!("Live input recovery observation: {error}"),
    }
}

/// Classify retained scoped work before generic input normalization can change
/// its run association or work-lane membership. This never publishes a result.
pub(crate) async fn authorize_input_normalization(
    store: &dyn RuntimeStore,
    runtime_id: &LogicalRuntimeId,
    bundle: &StoredInputState,
    boundary_committed: Option<bool>,
) -> Result<(), RuntimeDriverError> {
    let observation = observe_input_recovery(
        store,
        runtime_id,
        bundle,
        boundary_committed,
        dsl::LiveInputRecoveryPurpose::NormalizeColdInput,
        None,
    )
    .await?;
    match observation {
        None => Ok(()),
        Some(LiveInputRecoveryObservation {
            disposition:
                dsl::LiveInputRecoveryDisposition::NoBoundRun
                | dsl::LiveInputRecoveryDisposition::AppliedBoundary,
            ..
        }) => Ok(()),
        Some(LiveInputRecoveryObservation {
            disposition: dsl::LiveInputRecoveryDisposition::RuntimePending,
            ..
        }) => Err(corrupt("cold normalization returned a live observation")),
        Some(observation) => Err(RuntimeDriverError::RecoveryRepairBlocked {
            evidence_digest: Some(observation.evidence_digest),
            reason: format!(
                "scoped input {} retains unresolved run {:?}: {:?}",
                bundle.state.input_id, bundle.seed.last_run_id, observation.disposition
            ),
        }),
    }
}

pub(crate) async fn observe_input_recovery(
    store: &dyn RuntimeStore,
    runtime_id: &LogicalRuntimeId,
    bundle: &StoredInputState,
    boundary_committed: Option<bool>,
    purpose: dsl::LiveInputRecoveryPurpose,
    expected_head: Option<&LiveHeadReference>,
) -> Result<Option<LiveInputRecoveryObservation>, RuntimeDriverError> {
    if crate::meerkat_machine::input_seed_behavioral_terminality_via_authority(
        &bundle.state.input_id,
        &bundle.seed,
    )
    .map_err(corrupt)?
    {
        return Ok(None);
    }
    let persisted = bundle.state.persisted_input.as_ref().ok_or_else(|| {
        RuntimeDriverError::RecoveryCorruption {
            reason: crate::meerkat_machine::driver::missing_recovered_ingress_entry_reason(
                &bundle.state,
                &bundle.seed,
            ),
        }
    })?;
    let Input::LiveRequest(input) = persisted else {
        return match purpose {
            dsl::LiveInputRecoveryPurpose::NormalizeColdInput => Ok(None),
            dsl::LiveInputRecoveryPurpose::ObserveUnfinishedInput => Err(corrupt(
                "unfinished Live request has an ordinary input payload",
            )),
        };
    };
    let (provenance, source_digest) = input.request.source_reference();
    let source = provenance.source();
    let session_id = source.session_id();
    let key = input
        .header
        .idempotency_key
        .as_ref()
        .ok_or_else(|| corrupt("missing input key"))?;
    if runtime_id != &LogicalRuntimeId::for_session(session_id)
        || input.header.id != bundle.state.input_id
        || input.header.source != InputOrigin::LiveRequest
        || input.header.durability != InputDurability::Durable
        || bundle.state.durability != Some(InputDurability::Durable)
        || bundle.state.idempotency_key.as_ref() != Some(key)
    {
        return Err(corrupt("persisted Live input identity differs"));
    }
    let exact = store
        .load_input_state_by_idempotency_key(runtime_id, key)
        .await
        .map_err(backoff)?
        .ok_or_else(|| backoff("input disappeared"))?;
    if serde_json::to_value(exact.state()).map_err(corrupt)?
        != serde_json::to_value(bundle).map_err(corrupt)?
    {
        return Err(backoff("input changed after the recovery snapshot"));
    }
    let ops = store
        .live_ledger_ops()
        .ok_or_else(|| RuntimeDriverError::RecoveryRepairBlocked {
            evidence_digest: Some(exact.exact_row_digest().into()),
            reason: "Live input recovery requires its retained ledger owner".into(),
        })?;
    let head = ops
        .load_live_head(session_id)
        .await
        .map_err(backoff)?
        .ok_or_else(|| corrupt("missing Live head"))?;
    head.validate_payload().map_err(corrupt)?;
    if expected_head.is_some_and(|expected| expected != &head.reference) {
        return Err(backoff("Live head changed after request discovery"));
    }
    let row = ops
        .lookup_live_source(source)
        .await
        .map_err(backoff)?
        .ok_or_else(|| corrupt("missing immutable source"))?;
    let LiveSourceEntryRecord::Reservation { record } = row.record().map_err(corrupt)? else {
        return Err(corrupt("source is not a request"));
    };
    let LiveSourceDisposition::Admitted { receipt } = record.disposition() else {
        return Err(corrupt("source was not admitted"));
    };
    if record.request_id() != provenance.request_id()
        || record.frozen_digest().map_err(corrupt)? != *source_digest
        || receipt.source() != source
    {
        return Err(corrupt("input does not bind its retained request source"));
    }
    let state = crate::generated::live_request_state::decode(&head.payload.request_snapshot)
        .map_err(corrupt)?;
    match &input.request {
        crate::live_request::LiveExecutionRequestRecord::LiveRequest { .. } => {
            if receipt.input_id() != &bundle.state.input_id {
                return Err(corrupt("original admission input differs"));
            }
        }
        crate::live_request::LiveExecutionRequestRecord::CallbackContinuation { .. } => {
            input
                .request
                .validate_continuation_binding(&state, &bundle.state.input_id)
                .map_err(corrupt)?;
        }
    }
    let input_id = bundle.state.input_id.to_string();
    let request_id = record.request_id().to_string();
    let mut runs = state
        .run_inputs
        .iter()
        .filter_map(|(run, input)| (input == &input_id).then_some(run));
    let run = runs.next().cloned();
    if runs.next().is_some() {
        return Err(corrupt("one scoped input has multiple execution runs"));
    }
    if bundle.seed.last_run_id.as_ref().map(ToString::to_string) != run {
        return Err(corrupt(
            "ordinary staged run differs from the retained scope",
        ));
    }
    let mut application = dsl::LiveRecoveryApplicationEvidence::NotApplicable;
    let mut actor = None;
    let mut runtime_run_current = false;
    let mut lifecycle_version = None;
    if let Some(run) = &run {
        let scope: RunEffectScopeRecord = serde_json::from_str(
            state
                .run_scope_records
                .get(run)
                .ok_or_else(|| corrupt("missing scope record"))?,
        )
        .map_err(corrupt)?;
        let scope_id = RunEffectScopeId::from_uuid(
            uuid::Uuid::parse_str(
                state
                    .run_scopes
                    .get(run)
                    .ok_or_else(|| corrupt("missing scope identity"))?,
            )
            .map_err(corrupt)?,
        );
        scope
            .validate_callback_continuation(scope_id)
            .map_err(corrupt)?;
        if scope.request_id != *record.request_id()
            || scope.input_id != bundle.state.input_id
            || scope.run_id.to_string() != *run
            || scope.executor != *receipt.executor()
            || scope.grant != *receipt.grant()
        {
            return Err(corrupt(
                "scope does not bind the original input and executor",
            ));
        }
        if purpose == dsl::LiveInputRecoveryPurpose::ObserveUnfinishedInput {
            let crate::store::MachineLifecycleObservation::Decoded { record, version } = store
                .observe_machine_lifecycle(runtime_id)
                .await
                .map_err(backoff)?
            else {
                return Err(backoff("runtime lifecycle cannot be decoded"));
            };
            runtime_run_current = record.runtime_state() == Some(crate::RuntimeState::Running)
                && record.run().current_run_id() == Some(&scope.run_id)
                && record.run().pre_run_phase().is_some()
                && record.binding().runtime_generation() == Some(scope.executor.binding_generation)
                && record.binding().runtime_epoch_id()
                    == Some(scope.executor.runtime_epoch.to_string().as_str())
                && record.binding().agent_runtime_id().is_some()
                && record.binding().fence_token().is_some();
            lifecycle_version = Some(version);
        }
        for (claim_id, claim_run) in &state.claim_runs {
            if claim_run == run {
                super::claim::read_retained_claim(&state, head.reference.revision, claim_id)
                    .map_err(corrupt)?;
            }
        }
        if let Some(continuation) = &scope.callback_continuation {
            let body = read_live_composite(
                ops,
                LiveCompositeReadRequest::new(
                    session_id.clone(),
                    None,
                    head.reference.event_count,
                    1,
                )
                .map_err(backoff)?,
            )
            .await
            .map_err(backoff)?
            .ok_or_else(|| corrupt("missing committed actor body"))?;
            if body.authority().live_head() != Some(&head.reference) {
                return Err(backoff("Live head changed during actor observation"));
            }
            actor = Some(match body.authority().actor() {
                crate::store::RuntimeSessionAuthority::WholeBlob(authority) => (
                    "whole_blob_v1",
                    authority.store_revision(),
                    authority.blob_sha256().to_string(),
                ),
                crate::store::RuntimeSessionAuthority::HeadCanonical(authority) => (
                    "head_canonical_v1",
                    authority.store_revision(),
                    authority.committed_head_token().to_string(),
                ),
            });
            application = match body
                .session()
                .observe_staged_callback_results(&continuation.target)
            {
                Ok(StagedCallbackResultsObservation::AlreadyApplied {
                    results_digest: Some(digest),
                    ..
                }) if digest == continuation.results_digest => {
                    dsl::LiveRecoveryApplicationEvidence::Applied
                }
                Ok(
                    StagedCallbackResultsObservation::Complete(_)
                    | StagedCallbackResultsObservation::Incomplete { .. },
                ) => dsl::LiveRecoveryApplicationEvidence::NotApplied,
                Ok(StagedCallbackResultsObservation::AlreadyApplied { .. })
                | Err(meerkat_core::session::CallbackBatchObservationError::TargetMismatch) => {
                    dsl::LiveRecoveryApplicationEvidence::Unobserved
                }
                Err(error) => return Err(corrupt(error)),
            };
        }
    }
    let phase = match bundle.seed.phase {
        InputLifecycleState::Accepted | InputLifecycleState::Queued => {
            dsl::LiveRecoveryInputPhase::Queued
        }
        InputLifecycleState::Staged => dsl::LiveRecoveryInputPhase::Staged,
        InputLifecycleState::Applied | InputLifecycleState::AppliedPendingConsumption => {
            dsl::LiveRecoveryInputPhase::Applied
        }
        InputLifecycleState::Consumed
        | InputLifecycleState::Superseded
        | InputLifecycleState::Coalesced
        | InputLifecycleState::Abandoned => dsl::LiveRecoveryInputPhase::Terminal,
    };
    let run_id = run.unwrap_or_default();
    let mut owner = dsl::LiveRequestMachineAuthority::recover_from_state(state).map_err(corrupt)?;
    let transition = dsl::LiveRequestMachineMutator::apply(
        &mut owner,
        dsl::LiveRequestInput::ResolveInputRecovery {
            request_id: request_id.clone(),
            input_id: input_id.clone(),
            run_id: run_id.clone(),
            source: serde_json::to_string(source).map_err(corrupt)?,
            observed_phase: phase,
            boundary_committed: boundary_committed == Some(true),
            application_evidence: application,
            purpose,
            runtime_run_current,
        },
    )
    .map_err(corrupt)?;
    let [
        dsl::LiveRequestEffect::InputRecoveryResolved {
            request_id: request,
            input_id: input,
            run_id: run,
            disposition,
        },
    ] = transition.effects()
    else {
        return Err(corrupt("missing generated input recovery disposition"));
    };
    if request != &request_id || input != &input_id || run != &run_id {
        return Err(corrupt("generated recovery changed input or run identity"));
    }
    if purpose == dsl::LiveInputRecoveryPurpose::ObserveUnfinishedInput {
        let latest = store
            .load_input_state_by_idempotency_key(runtime_id, key)
            .await
            .map_err(backoff)?
            .ok_or_else(|| backoff("input disappeared during observation"))?;
        if latest.exact_row_digest() != exact.exact_row_digest() {
            return Err(backoff("input changed during observation"));
        }
        if let Some(version) = &lifecycle_version {
            let crate::store::MachineLifecycleObservation::Decoded {
                version: latest, ..
            } = store
                .observe_machine_lifecycle(runtime_id)
                .await
                .map_err(backoff)?
            else {
                return Err(backoff("runtime lifecycle disappeared during observation"));
            };
            if &latest != version {
                return Err(backoff("runtime lifecycle changed during observation"));
            }
        }
        if ops
            .load_live_head(session_id)
            .await
            .map_err(backoff)?
            .as_ref()
            .map(|latest| &latest.reference)
            != Some(&head.reference)
        {
            return Err(backoff("Live head changed during observation"));
        }
    }
    let evidence = serde_json::to_vec(&(
        "meerkat/live-input-recovery/v2",
        exact.exact_row_digest(),
        &head.reference,
        row.digest(),
        actor,
        lifecycle_version
            .as_ref()
            .map(MachineLifecycleObservationVersion::as_str),
        boundary_committed,
        application,
        purpose,
        runtime_run_current,
        disposition,
    ))
    .map_err(corrupt)?;
    Ok(Some(LiveInputRecoveryObservation {
        disposition: *disposition,
        evidence_digest: format!("{:x}", Sha256::digest(evidence)),
    }))
}
