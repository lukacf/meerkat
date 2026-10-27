//! One bounded handoff over the store's existing dispatch/reconciliation
//! contract. Generated acquisition truth and runtime admission remain owners.

use super::*;
use crate::continuation::{
    ContinuationReconciliation, ContinuationStep, ContinuationStepOptions,
    SetupContinuationDelivery,
};
use std::{future::Future, time::Duration};
use toolkit_store::{
    EffectClaimKind, EffectClaimOutcome, EffectKey, EffectOutcome, EffectStatus, Reconciliation,
    StoredEffect,
};

const RECEIPT_SCHEMA: &str = "meerkat_toolkit.continuation_admission";
const OBSERVATION_SCHEMA: &str = "meerkat_toolkit.continuation_observation";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Observation {
    obligation_digest: ContentDigest,
    delivery_identity: ContentDigest,
    kind: ObservationKind,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum ObservationKind {
    OwnerPending { evidence: ContextReference },
    SafeToRetry { evidence: ContextReference },
    OwnerRejected { evidence: ContextReference },
    Ambiguous,
    BudgetExhausted,
}

impl SetupStore<'_> {
    /// Run at most one admission or reconciliation call. An expired setup token
    /// does not authorize this path: the host supplies the original task owner,
    /// which checks current authority at every runtime/observation boundary.
    pub async fn step_continuation(
        &self,
        operation: &Id,
        owner: &dyn SetupContinuationDelivery,
        options: &ContinuationStepOptions,
    ) -> Result<ContinuationStep, SetupError> {
        let required_lease_ms = options
            .owner_timeout_ms
            .checked_mul(2)
            .and_then(|owner_calls| owner_calls.checked_add(options.settlement_budget_ms));
        if !(1..=86_400_000).contains(&options.lease_ms)
            || options.owner_timeout_ms == 0
            || options.settlement_budget_ms == 0
            || required_lease_ms.is_none_or(|minimum| options.lease_ms < minimum)
        {
            return Err(StoreError::Invalid("invalid continuation step bounds".into()).into());
        }
        let stored = self
            .store
            .get(&self.scope.key(operation))
            .await?
            .ok_or(StoreError::NotFound)?;
        let snapshot = self.recover(stored)?;
        let obligation = continuation_obligation(&snapshot.record, &snapshot.request)?;
        let state = snapshot.state();
        let (_, expected) = handoffs(
            &snapshot.record,
            vec![AcquisitionEffect::ContinuationRequested {
                operation_id: state.operation_id.clone(),
                continuation_id: state.continuation_id.clone(),
                terminal_revision: state.terminal_revision,
                outcome: state.lifecycle_phase,
            }],
        )?;
        let expected = expected
            .into_iter()
            .next()
            .ok_or(SetupError::InvalidRecord)?;
        let key = EffectKey {
            record: snapshot.key().clone(),
            id: expected.id.clone(),
        };
        let effect = self
            .store
            .get_effect(&key)
            .await?
            .ok_or(SetupError::InvalidRecord)?;
        check_effect(&effect, &key, &expected, &obligation)?;

        if effect.status == EffectStatus::Succeeded {
            let evidence = decode_receipt(&effect, &obligation)?;
            return self
                .finish_continuation(operation, &evidence, owner, options)
                .await;
        }
        // The standalone receipt bridge can commit before outbox settlement.
        // Repair this exact effect without another runtime admission.
        if let Some(evidence) = &snapshot.record.continuation_receipt {
            authenticate(owner, &obligation, evidence, options).await?;
            if effect.status == EffectStatus::Failed {
                return Err(SetupError::InvalidRecord);
            }
            let claimed = if effect.status == EffectStatus::Pending && effect.attempt == 0 {
                match self
                    .store
                    .claim_effect(
                        &key,
                        state.revision,
                        options.worker.as_str(),
                        options.lease_ms,
                    )
                    .await?
                {
                    EffectClaimOutcome::Claimed(effect) => effect,
                    EffectClaimOutcome::NeedsReconciliation(_) => {
                        self.store
                            .claim_reconciliation(&key, options.worker.as_str(), options.lease_ms)
                            .await?
                    }
                }
            } else {
                self.store
                    .claim_reconciliation(&key, options.worker.as_str(), options.lease_ms)
                    .await?
            };
            check_effect(&claimed, &key, &expected, &obligation)?;
            self.settle_admission(&claimed, evidence).await?;
            return self
                .finish_continuation(operation, evidence, owner, options)
                .await;
        }
        if effect.status == EffectStatus::Failed {
            let old = decode_observation(effect.outcome.as_ref(), &obligation)?;
            let ObservationKind::OwnerRejected { evidence: old } = old.kind else {
                return Err(SetupError::InvalidRecord);
            };
            let observed = bounded(options, owner.reconcile(&obligation)).await?;
            return match observed {
                ContinuationReconciliation::DefinitivelyRejected { evidence }
                    if evidence == old =>
                {
                    Ok(ContinuationStep::OwnerRejected)
                }
                _ => Err(SetupError::InvalidRecord),
            };
        }
        let budget_exhausted =
            effect.attempt >= u64::from(snapshot.request.host().limits.max_effect_attempts);
        let mut observation_only = budget_exhausted;
        let claimed = if effect.status == EffectStatus::Pending {
            if !observation_only {
                // Current preflight does not authorize a later call by itself.
                // Losing dispatch authority does not erase a prior attempt's
                // narrower observation path, authorized by reconcile itself.
                if let Err(error) = bounded(options, owner.authorize_delivery(&obligation)).await {
                    if effect.attempt == 0 {
                        return Err(error.into());
                    }
                    observation_only = true;
                }
            }
            if observation_only {
                if effect.attempt == 0 {
                    return Ok(ContinuationStep::DispatchBudgetExhausted);
                }
                self.store
                    .claim_reconciliation(&key, options.worker.as_str(), options.lease_ms)
                    .await?
            } else {
                match self
                    .store
                    .claim_effect(
                        &key,
                        state.revision,
                        options.worker.as_str(),
                        options.lease_ms,
                    )
                    .await?
                {
                    EffectClaimOutcome::Claimed(effect) => effect,
                    EffectClaimOutcome::NeedsReconciliation(_) => {
                        self.store
                            .claim_reconciliation(&key, options.worker.as_str(), options.lease_ms)
                            .await?
                    }
                }
            }
        } else {
            self.store
                .claim_reconciliation(&key, options.worker.as_str(), options.lease_ms)
                .await?
        };
        check_effect(&claimed, &key, &expected, &obligation)?;
        let lease = claimed.lease.as_ref().ok_or(SetupError::InvalidRecord)?;
        if lease.kind == EffectClaimKind::Reconcile {
            // Another worker can spend the last attempt after the initial read.
            let budget_exhausted =
                claimed.attempt >= u64::from(snapshot.request.host().limits.max_effect_attempts);
            let result = self
                .reconcile_continuation(operation, &obligation, &claimed, owner, options)
                .await?;
            return Ok(match result {
                ContinuationStep::Pending | ContinuationStep::RetryAuthorized
                    if budget_exhausted =>
                {
                    ContinuationStep::DispatchBudgetExhausted
                }
                ContinuationStep::RetryAuthorized if observation_only => ContinuationStep::Pending,
                other => other,
            });
        }
        // A pre-claim read is not an atomic attempt budget. Inspect the returned
        // claim too; even a claimed-but-never-dispatched attempt consumes budget.
        if claimed.attempt > u64::from(snapshot.request.host().limits.max_effect_attempts) {
            self.store
                .acknowledge_effect(
                    lease,
                    EffectOutcome::Uncertain {
                        evidence: observation(&obligation, ObservationKind::BudgetExhausted)?,
                    },
                )
                .await?;
            return Ok(ContinuationStep::DispatchBudgetExhausted);
        }
        let result = bounded(options, owner.admit(&obligation)).await;
        let evidence = match result {
            Ok(evidence) => {
                if let Err(error) = authenticate(owner, &obligation, &evidence, options).await {
                    self.mark_ambiguous(&claimed, &obligation).await?;
                    return Err(error.into());
                }
                evidence
            }
            Err(error) => {
                self.mark_ambiguous(&claimed, &obligation).await?;
                return Err(error.into());
            }
        };
        self.settle_admission(&claimed, &evidence).await?;
        self.finish_continuation(operation, &evidence, owner, options)
            .await
    }

    async fn finish_continuation(
        &self,
        operation: &Id,
        evidence: &ContinuationAdmissionEvidence,
        owner: &dyn SetupContinuationDelivery,
        options: &ContinuationStepOptions,
    ) -> Result<ContinuationStep, SetupError> {
        // Exact evidence is checked again inside the acknowledgment CAS path.
        // The outer timeout bounds the owner call without changing its result.
        let result = tokio::time::timeout(
            Duration::from_millis(options.owner_timeout_ms),
            self.acknowledge_continuation_exact(
                operation,
                &evidence.receipt,
                Some(evidence),
                owner,
            ),
        )
        .await
        .map_err(|_| ContinuationError::Timeout)??;
        Ok(match result {
            Some(receipt) => ContinuationStep::Acknowledged(receipt),
            None => ContinuationStep::Pending,
        })
    }

    async fn settle_admission(
        &self,
        effect: &StoredEffect,
        evidence: &ContinuationAdmissionEvidence,
    ) -> Result<(), SetupError> {
        let lease = effect.lease.as_ref().ok_or(SetupError::InvalidRecord)?;
        let receipt = RecordData {
            schema: RECEIPT_SCHEMA.into(),
            version: 1,
            body: serde_json::to_value(evidence)?,
        };
        match lease.kind {
            EffectClaimKind::Dispatch => {
                self.store
                    .acknowledge_effect(lease, EffectOutcome::Succeeded { receipt })
                    .await?;
            }
            EffectClaimKind::Reconcile => {
                self.store
                    .reconcile_effect(lease, Reconciliation::Applied { receipt })
                    .await?;
            }
        }
        Ok(())
    }

    async fn mark_ambiguous(
        &self,
        effect: &StoredEffect,
        obligation: &ContinuationObligation,
    ) -> Result<(), SetupError> {
        let lease = effect.lease.as_ref().ok_or(SetupError::InvalidRecord)?;
        let evidence = observation(obligation, ObservationKind::Ambiguous)?;
        match lease.kind {
            EffectClaimKind::Dispatch => {
                self.store
                    .acknowledge_effect(lease, EffectOutcome::Uncertain { evidence })
                    .await?;
            }
            EffectClaimKind::Reconcile => {
                self.store
                    .reconcile_effect(lease, Reconciliation::Pending { evidence })
                    .await?;
            }
        }
        Ok(())
    }

    async fn reconcile_continuation(
        &self,
        operation: &Id,
        obligation: &ContinuationObligation,
        claimed: &StoredEffect,
        owner: &dyn SetupContinuationDelivery,
        options: &ContinuationStepOptions,
    ) -> Result<ContinuationStep, SetupError> {
        let lease = claimed.lease.as_ref().ok_or(SetupError::InvalidRecord)?;
        let result = match bounded(options, owner.reconcile(obligation)).await {
            Ok(result) => result,
            Err(error) => {
                self.mark_ambiguous(claimed, obligation).await?;
                return Err(error.into());
            }
        };
        let (resolution, result) = match result {
            ContinuationReconciliation::Applied(evidence) => {
                if let Err(error) = authenticate(owner, obligation, &evidence, options).await {
                    self.mark_ambiguous(claimed, obligation).await?;
                    return Err(error.into());
                }
                self.settle_admission(claimed, &evidence).await?;
                return self
                    .finish_continuation(operation, &evidence, owner, options)
                    .await;
            }
            ContinuationReconciliation::SafeToRetry { evidence } => (
                Reconciliation::RetrySafe {
                    evidence: observation(obligation, ObservationKind::SafeToRetry { evidence })?,
                },
                ContinuationStep::RetryAuthorized,
            ),
            ContinuationReconciliation::Pending { evidence } => (
                Reconciliation::Pending {
                    evidence: observation(obligation, ObservationKind::OwnerPending { evidence })?,
                },
                ContinuationStep::Pending,
            ),
            ContinuationReconciliation::DefinitivelyRejected { evidence } => (
                Reconciliation::Failed {
                    evidence: observation(obligation, ObservationKind::OwnerRejected { evidence })?,
                },
                ContinuationStep::OwnerRejected,
            ),
        };
        self.store.reconcile_effect(lease, resolution).await?;
        Ok(result)
    }
}

async fn bounded<T>(
    options: &ContinuationStepOptions,
    call: impl Future<Output = Result<T, ContinuationError>>,
) -> Result<T, ContinuationError> {
    tokio::time::timeout(Duration::from_millis(options.owner_timeout_ms), call)
        .await
        .map_err(|_| ContinuationError::Timeout)?
}

async fn authenticate(
    owner: &dyn SetupContinuationDelivery,
    obligation: &ContinuationObligation,
    evidence: &ContinuationAdmissionEvidence,
    options: &ContinuationStepOptions,
) -> Result<(), ContinuationError> {
    evidence.check(obligation, &evidence.receipt)?;
    let actual = bounded(
        options,
        owner.verify_admission(obligation, &evidence.receipt),
    )
    .await?
    .ok_or(ContinuationError::InvalidReceipt)?;
    if !actual.confirms(evidence) {
        return Err(ContinuationError::InvalidReceipt);
    }
    Ok(())
}

pub(super) fn check_effect(
    effect: &StoredEffect,
    key: &EffectKey,
    expected: &EffectIntent,
    obligation: &ContinuationObligation,
) -> Result<(), SetupError> {
    if &effect.key != key
        || &effect.intent != expected
        || effect.record_revision != obligation.terminal_revision()
        || effect.request_digest != effect.intent.digest()?
    {
        return Err(SetupError::InvalidRecord);
    }
    match (effect.status, effect.outcome.as_ref()) {
        (EffectStatus::Pending, None) => (),
        (EffectStatus::InFlight, None) if effect.lease.is_some() && effect.attempt > 0 => (),
        (EffectStatus::Uncertain, None) if effect.attempt > 0 => (),
        (EffectStatus::Uncertain, Some(EffectOutcome::Uncertain { .. })) if effect.attempt > 0 => {
            decode_observation(effect.outcome.as_ref(), obligation)?;
        }
        (EffectStatus::Succeeded, Some(EffectOutcome::Succeeded { .. })) => {
            decode_receipt(effect, obligation)?;
        }
        (EffectStatus::Failed, Some(EffectOutcome::Failed { .. })) => {
            decode_observation(effect.outcome.as_ref(), obligation)?;
        }
        _ => return Err(SetupError::InvalidRecord),
    }
    if let Some(lease) = &effect.lease
        && (lease.key != *key
            || lease.fence == 0
            || !matches!(
                (effect.status, lease.kind),
                (EffectStatus::InFlight, EffectClaimKind::Dispatch)
                    | (EffectStatus::Uncertain, EffectClaimKind::Reconcile)
            ))
    {
        return Err(SetupError::InvalidRecord);
    }
    Ok(())
}

/// Private recovery may skip only a structurally complete terminal pair.
/// This checks existing stored evidence, not current native admission authority.
/// Active execution still goes through step_continuation and its owner checks.
pub(super) fn recovery_effect_settled(
    effect: &StoredEffect,
    obligation: &ContinuationObligation,
    canonical: Option<&ContinuationAdmissionEvidence>,
) -> Result<bool, SetupError> {
    match effect.status {
        EffectStatus::Failed => {
            if canonical.is_some()
                || !matches!(decode_observation(effect.outcome.as_ref(), obligation)?.kind,
                    ObservationKind::OwnerRejected { .. })
            {
                return Err(SetupError::InvalidRecord);
            }
            Ok(true)
        }
        EffectStatus::Succeeded => {
            let observed = decode_receipt(effect, obligation)?;
            let Some(canonical) = canonical else { return Ok(false); };
            canonical.check(obligation, &canonical.receipt)?;
            // The standalone acknowledgement and effect settlement can race.
            // Both retain the same exact admission but may observe it at
            // different local times. Every other field must match in the
            // existing evidence contract; neither observation rewrites the other.
            if !observed.confirms(canonical) && !canonical.confirms(&observed) {
                return Err(SetupError::InvalidRecord);
            }
            Ok(true)
        }
        _ => Ok(false),
    }
}

fn decode_receipt(
    effect: &StoredEffect,
    obligation: &ContinuationObligation,
) -> Result<ContinuationAdmissionEvidence, SetupError> {
    let Some(EffectOutcome::Succeeded { receipt }) = &effect.outcome else {
        return Err(SetupError::InvalidRecord);
    };
    if receipt.schema != RECEIPT_SCHEMA || receipt.version != 1 {
        return Err(SetupError::InvalidRecord);
    }
    let evidence: ContinuationAdmissionEvidence = serde_json::from_value(receipt.body.clone())?;
    evidence.check(obligation, &evidence.receipt)?;
    Ok(evidence)
}

fn observation(
    obligation: &ContinuationObligation,
    kind: ObservationKind,
) -> Result<RecordData, SetupError> {
    Ok(RecordData {
        schema: OBSERVATION_SCHEMA.into(),
        version: 1,
        body: serde_json::to_value(Observation {
            obligation_digest: obligation.digest()?,
            delivery_identity: obligation.delivery_identity()?,
            kind,
        })?,
    })
}

fn decode_observation(
    outcome: Option<&EffectOutcome>,
    obligation: &ContinuationObligation,
) -> Result<Observation, SetupError> {
    let data = match outcome {
        Some(EffectOutcome::Failed { evidence } | EffectOutcome::Uncertain { evidence }) => {
            evidence
        }
        _ => return Err(SetupError::InvalidRecord),
    };
    if data.schema != OBSERVATION_SCHEMA || data.version != 1 {
        return Err(SetupError::InvalidRecord);
    }
    let decoded: Observation = serde_json::from_value(data.body.clone())?;
    if decoded.obligation_digest != obligation.digest()?
        || decoded.delivery_identity != obligation.delivery_identity()?
    {
        return Err(SetupError::InvalidRecord);
    }
    Ok(decoded)
}
