//! Actor-free acknowledgment of an already committed recipient disclosure.
//! All native refusals preserve uncertainty. This path owns no dispatch claim,
//! controller, ingress, source promotion, negative witness or retry permission.
use super::*;
use crate::private_originals::planning_source_evidence::PlanningSourceEvidence;
use meerkat_runtime::meerkat_machine::{DurableAdmittedOriginal, DurableAdmittedOriginalRefusal};

#[cfg(test)]
pub(in crate::private_originals::setup_workflow::launch::context_transfer) mod tests;

pub(super) async fn reconcile(
    source: Arc<PrivateOriginals>,
    transfer: Arc<InstalledWorkflowContextTransfer>,
    attempt: Arc<TransferAttempt>,
    review_custody: ReviewCustody,
) -> Result<WorkflowDeliveryObservation, PrivateOriginalError> {
    if attempt.locally_entered() {
        return Err(PrivateOriginalError::Conflict);
    }
    validate_host(&source)?;
    let control = attempt
        .recovery
        .upgrade()
        .ok_or(PrivateOriginalError::Unavailable)?;
    let task_attempt = attempt.clone();
    let run = DeliveryRun::register_advance(&control, &attempt.advance, move || async move {
        // A reviewed caller may disappear while this existing history-only
        // task is reading or acknowledging the exact retained input. Carry its
        // custody into this task and any entered store worker, not its waiter.
        // Reusing a registered run preserves that run's original custody.
        review_custody
            .retain_io(observe_and_acknowledge(&source, &transfer, &task_attempt))
            .await
    })?;
    run.join().await
}

fn validate_host(source: &PrivateOriginals) -> Result<(), PrivateOriginalError> {
    source
        .host_lifetime
        .get()
        .and_then(Weak::upgrade)
        .ok_or(PrivateOriginalError::Unavailable)?
        .validate_current_selection()
        .map_err(|_| PrivateOriginalError::Unavailable)
}

/// Temporary query data derived from canonical evidence, never input custody.
/// No process ingress/controller is constructed, retained or returned.
struct ExpectedDelivery {
    session: SessionId,
    association: InputAuthorityAssociation,
    input: Input,
}

async fn expected_delivery(
    source: &PrivateOriginals,
    transfer: &InstalledWorkflowContextTransfer,
    attempt: &TransferAttempt,
) -> Result<ExpectedDelivery, PrivateOriginalError> {
    let planning = transfer.planning()?;
    let selected = &attempt.envelope.recipient;
    if selected != &transfer.selected
        || selected.planning_source != planning.submission.original
        || selected.parent != *planning.parent.association()
        || selected.lead != planning.terms.lead
        || selected.backend != planning.terms.backend
    {
        return Err(PrivateOriginalError::Conflict);
    }
    expected_envelope(
        source,
        &attempt.envelope,
        &attempt.admission,
        Some(&planning.terms),
    )
    .await
}

async fn expected_envelope(
    source: &PrivateOriginals,
    envelope: &TransferEnvelope,
    admission: &WorkAdmission,
    selected_terms: Option<&meerkat_toolkit::setup::SetupWorkflowTerms>,
) -> Result<ExpectedDelivery, PrivateOriginalError> {
    let selected = &envelope.recipient;
    let snapshot = source
        .workflow_store()?
        .load_setup_workflow(&selected.workflow)
        .await
        .map_err(|_| PrivateOriginalError::Unavailable)?;
    if selected_terms.is_some_and(|terms| terms != snapshot.terms())
        || snapshot.terms().planning_source != source.key(&selected.planning_source)?
        || snapshot.terms().lead != selected.lead
        || snapshot.terms().backend != selected.backend
    {
        return Err(PrivateOriginalError::Conflict);
    }
    // Exact historical membership is sufficient. Cancellation, departed A or
    // an empty recipient cohort cannot erase an input already admitted by G.
    let _ = snapshot
        .departed(&envelope.membership)
        .map_err(|_| PrivateOriginalError::Conflict)?;
    let stored = source
        .store
        .get(&source.key(&selected.planning_source)?)
        .await
        .map_err(store_error)?
        .ok_or(PrivateOriginalError::Unavailable)?;
    let record = source.decode_record(&stored, &selected.planning_source)?;
    source.check_planning_evidence_source(&record, &selected.planning_source)?;
    let Some(PlanningSourceEvidence::Lead { parent }) = &record.planning_source_evidence else {
        return Err(PrivateOriginalError::Unavailable);
    };
    let parent_row = source
        .store
        .get(&source.key(parent)?)
        .await
        .map_err(store_error)?
        .ok_or(PrivateOriginalError::Unavailable)?;
    let parent_record = source.decode_record(&parent_row, parent)?;
    source.check_planning_evidence_source(&parent_record, parent)?;
    if !crate::private_originals::planning_source_evidence::is_parent(
        parent_record.planning_source_evidence.as_ref(),
    ) || parent_record.final_association.as_ref() != Some(&selected.parent)
    {
        return Err(PrivateOriginalError::Conflict);
    }
    let association = record
        .final_association
        .as_ref()
        .ok_or(PrivateOriginalError::Unavailable)?;
    let initial = record
        .bound_service_input
        .as_ref()
        .ok_or(PrivateOriginalError::Unavailable)?
        .decode()?;
    if initial.header().authority_association.as_ref() != Some(association)
        || association.candidate().contributing_work.as_slice()
            != [selected.parent.candidate().original_work.clone()]
    {
        return Err(PrivateOriginalError::Conflict);
    }
    let session = SessionId::parse(association.candidate().target.context.as_str())
        .map_err(|_| PrivateOriginalError::Conflict)?;
    if association.candidate().target.logical_runtime.as_str()
        != meerkat_runtime::LogicalRuntimeId::for_session(&session).to_string()
    {
        return Err(PrivateOriginalError::Conflict);
    }
    let (input, event) = delivery_input_for(envelope, admission)?;
    let Input::Prompt(mut input) = input else {
        return Err(PrivateOriginalError::Conflict);
    };
    // The live binder changes only these serialized fields. Its authenticated
    // ingress and actual controller are process handles, excluded by Core's
    // replay contract. The query does not recreate either handle.
    input.header.authority_association = Some(association.clone());
    input.header.idempotency_key = Some(event);
    Ok(ExpectedDelivery {
        session,
        association: association.clone(),
        input: Input::Prompt(input),
    })
}

async fn observe_and_acknowledge(
    source: &PrivateOriginals,
    transfer: &InstalledWorkflowContextTransfer,
    attempt: &TransferAttempt,
) -> Result<WorkflowDeliveryObservation, PrivateOriginalError> {
    validate_host(source)?;
    let effect = delivery_effect(source, attempt).await?;
    if effect.attempt == 0 || effect.status == EffectStatus::Failed {
        return Ok(WorkflowDeliveryObservation::Pending);
    }
    // Preserve the selected in-memory owner check before sharing inert data.
    let expected = expected_delivery(source, transfer, attempt).await?;
    acknowledge_expected(
        source,
        &attempt.envelope,
        &attempt.admission,
        effect,
        expected,
    )
    .await
}

/// The recovery worker owns this entire future. These are validated immutable
/// data, not a transfer attempt, joined task, dispatch receipt or payload handle.
pub(in crate::private_originals::setup_workflow::launch::context_transfer) async fn acknowledge_envelope(
    source: &PrivateOriginals,
    envelope: &TransferEnvelope,
    admission: &WorkAdmission,
) -> Result<WorkflowDeliveryObservation, PrivateOriginalError> {
    validate_host(source)?;
    let effect = delivery_effect_for(source, &envelope.membership).await?;
    if effect.attempt == 0 || effect.status == EffectStatus::Failed {
        return Ok(WorkflowDeliveryObservation::Pending);
    }
    let expected = expected_envelope(source, envelope, admission, None).await?;
    acknowledge_expected(source, envelope, admission, effect, expected).await
}

async fn acknowledge_expected(
    source: &PrivateOriginals,
    envelope: &TransferEnvelope,
    admission: &WorkAdmission,
    effect: StoredEffect,
    expected: ExpectedDelivery,
) -> Result<WorkflowDeliveryObservation, PrivateOriginalError> {
    validate_host(source)?;
    let admitted = source
        .machine()?
        .durable_admitted_original(&expected.session, &expected.input)
        .await;
    let Some(admitted) = accepted_only(admitted)? else {
        return Ok(WorkflowDeliveryObservation::Pending);
    };
    if admitted.session_id() != &expected.session || admitted.association() != &expected.association
    {
        return Err(PrivateOriginalError::Conflict);
    }
    // Only native's actual input id enters the receipt. The expected prompt's
    // generated id/time are irrelevant to this read and never recorded here.
    let receipt = delivery_receipt_for(
        &envelope.membership,
        admission,
        admitted.session_id(),
        admitted.input_id(),
        admitted.association(),
    )?;
    let observation = WorkflowDeliveryObservation::Admitted {
        session_id: admitted.session_id().to_string(),
        input_id: admitted.input_id().to_string(),
    };
    validate_host(source)?;
    if effect.status == EffectStatus::Succeeded {
        return if effect.outcome.as_ref() == Some(&EffectOutcome::Succeeded { receipt }) {
            Ok(observation)
        } else {
            Err(PrivateOriginalError::Conflict)
        };
    }
    let claim = match source
        .store
        .claim_reconciliation(&effect.key, "installed-context-history", 60_000)
        .await
    {
        Ok(claim) => claim,
        Err(StoreError::Claimed | StoreError::EffectState | StoreError::Conflict) => {
            return Ok(WorkflowDeliveryObservation::Pending);
        }
        Err(error) => return Err(store_error(error)),
    };
    // The claim can race another attempt while the native read awaits. It
    // must still be this exact intent/digest/attempt, under a Reconcile lease.
    let lease = exact_reconciliation_lease(&effect, &claim)?;
    let resolved = source
        .store
        .reconcile_effect(
            lease,
            Reconciliation::Applied {
                receipt: receipt.clone(),
            },
        )
        .await
        .map_err(store_error)?;
    if resolved.key != effect.key
        || resolved.intent != effect.intent
        || resolved.request_digest != effect.request_digest
        || resolved.attempt != effect.attempt
        || resolved.status != EffectStatus::Succeeded
        || resolved.lease.is_some()
        || resolved.outcome != Some(EffectOutcome::Succeeded { receipt })
    {
        return Err(PrivateOriginalError::Conflict);
    }
    Ok(observation)
}

fn accepted_only(
    observed: Result<DurableAdmittedOriginal, DurableAdmittedOriginalRefusal>,
) -> Result<Option<DurableAdmittedOriginal>, PrivateOriginalError> {
    match observed {
        Ok(admitted) => Ok(Some(admitted)),
        Err(
            DurableAdmittedOriginalRefusal::NotAdmitted
            | DurableAdmittedOriginalRefusal::UnknownRuntime,
        ) => Ok(None),
        Err(DurableAdmittedOriginalRefusal::Conflict) => Err(PrivateOriginalError::Conflict),
        Err(_) => Err(PrivateOriginalError::Unavailable),
    }
}
