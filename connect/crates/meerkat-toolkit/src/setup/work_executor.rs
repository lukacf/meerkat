//! One bounded, recoverable handoff to the existing WorkGraph owner.

use super::*;
use crate::work_admission::{
    NativeWorkAvailable, SetupWorkAdmissionError, SetupWorkAdmissionObservation,
    SetupWorkAuthorizer, SetupWorkObservationAuthorizer, admit_setup_work_available,
    observe_setup_work,
};
use meerkat_workgraph::{WorkAdmissionKey, WorkItemId, WorkNamespace};
use std::{future::Future, time::Duration};
use toolkit_store::{
    EffectClaimKind, EffectClaimOutcome, EffectOutcome, EffectStatus, Reconciliation, StoredEffect,
};

const RECEIPT_SCHEMA: &str = "meerkat_toolkit.work_admission";
const OBSERVATION_SCHEMA: &str = "meerkat_toolkit.work_observation";

/// Trusted host composition. Keep the same authoritative WorkGraph backend for
/// this pinned environment across restart and migration. Namespace equality is
/// not proof that a replacement database contains the original admission ledger.
pub struct SetupWorkHost<'a> {
    /// Actual provider-open service and its validated durability declaration.
    /// This selection grants no work execution or publication permission.
    pub backend: SetupWorkBackend,
    pub dispatch: &'a dyn SetupWorkAuthorizer,
    pub observation: &'a dyn SetupWorkObservationAuthorizer,
}

/// Host settings for one step, never model-supplied arguments. The lease covers
/// admission and observation calls plus settlement. Storage delays still fail
/// closed at the fence. The host schedules subsequent steps.
pub struct WorkAdmissionStepOptions {
    pub worker: Id,
    pub lease_ms: u64,
    pub owner_timeout_ms: u64,
    pub settlement_budget_ms: u64,
}

impl WorkAdmissionStepOptions {
    /// Validate trusted bootstrap bounds through the same work-step owner used
    /// at dispatch. This performs no claim, admission, observation or effect.
    pub fn validate(&self) -> Result<(), SetupError> { validate_options(self) }
}

/// An observation of one handoff, not a work-execution or acquisition state.
pub enum WorkAdmissionStep {
    Admitted(SetupWorkAdmissionObservation),
    Pending,
    /// The original owner's exact-key contract permits safe replay. Current
    /// authority, deadline, cancellation and attempt budget still apply.
    RetrySafe,
    DispatchBudgetExhausted,
    DispatchNotAuthorized,
}

#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Receipt {
    preparation_digest: ContentDigest,
    realm: Id,
    namespace: WorkNamespace,
    work_item_id: WorkItemId,
    admission_key: WorkAdmissionKey,
    correlation_digest: ContentDigest,
}

impl Receipt {
    fn observed(prepared: &PreparedSetupWork, actual: &SetupWorkAdmissionObservation) -> Self {
        Self {
            preparation_digest: prepared.digest().clone(),
            realm: actual.scope().realm().clone(),
            namespace: actual.scope().namespace().clone(),
            work_item_id: actual.work_item_id().clone(),
            admission_key: actual.admission_key().clone(),
            correlation_digest: actual.correlation_digest().clone(),
        }
    }

    fn encode(&self) -> Result<RecordData, SetupError> {
        Ok(RecordData {
            schema: RECEIPT_SCHEMA.into(),
            version: 1,
            body: serde_json::to_value(self)?,
        })
    }

    fn decode(effect: &StoredEffect) -> Result<Self, SetupError> {
        let Some(EffectOutcome::Succeeded { receipt }) = &effect.outcome else {
            return Err(SetupError::InvalidRecord);
        };
        if receipt.schema != RECEIPT_SCHEMA || receipt.version != 1 {
            return Err(SetupError::InvalidRecord);
        }
        Ok(serde_json::from_value(receipt.body.clone())?)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum ObservationKind {
    Ambiguous,
    MissingAdmission,
    ExactOwnerReplaySafe,
    BudgetExhausted,
    DispatchNotAuthorized,
}

impl SetupStore<'_> {
    /// Execute at most one work admission, or observe an earlier attempt.
    /// `prepare_work` must have frozen the exact outbound request first. Passing
    /// no live access permits only host-authorized metadata recovery. This path
    /// never reads worker content or claims that a setup agent has run.
    async fn step_work_admission_available(
        &self,
        operation: &Id,
        host: &SetupWorkHost<'_>,
        access: Option<SetupAccess<'_>>,
        options: &WorkAdmissionStepOptions,
        available: NativeWorkAvailable,
    ) -> Result<WorkAdmissionStep, SetupError> {
        validate_options(options)?;
        let service = host.backend.service();
        let snapshot = self.recover(
            self.store
                .get(&self.scope.key(operation))
                .await?
                .ok_or(StoreError::NotFound)?,
        )?;
        let effect = self.original_work_effect(&snapshot).await?;
        let saved = self
            .store
            .get(&self.preparation_key(operation))
            .await?
            .ok_or(StoreError::NotFound)?;
        let prepared = self.recover_prepared_work(&snapshot, &effect, saved, available)?;
        if effect.status == EffectStatus::Failed {
            return Err(SetupError::InvalidRecord);
        }
        // This narrow authority is independent of expired setup/content access.
        // Observing before dispatch also recovers an owner commit whose local
        // effect was returned to Pending by an earlier retry-safe observation.
        let observed = bounded(
            options,
            observe_setup_work(service, prepared.admission(), host.observation),
        )
        .await?;
        // A peer may claim or settle while the owner observation is awaited.
        // Diagnose the original effect from current storage, not the snapshot
        // taken before that observation.
        let succeeded_before_observation = effect.status == EffectStatus::Succeeded;
        let effect = self.original_work_effect(&snapshot).await?;
        if effect.status == EffectStatus::Failed {
            return Err(SetupError::InvalidRecord);
        }
        if effect.status == EffectStatus::Succeeded {
            if observed.is_none() && !succeeded_before_observation {
                // The owner may have committed after our absence read but
                // before this peer's receipt. Retry the observation; neither a
                // missing receipt nor success is established by these reads.
                return Ok(WorkAdmissionStep::Pending);
            }
            let actual = observed.ok_or(SetupWorkAdmissionError::MissingReceipt)?;
            if Receipt::decode(&effect)? != Receipt::observed(&prepared, &actual) {
                return Err(SetupError::InvalidRecord);
            }
            return Ok(WorkAdmissionStep::Admitted(actual));
        }
        if let Some(actual) = observed {
            let Some(claimed) = self.claim_work_observation(&effect, options).await? else {
                return Ok(WorkAdmissionStep::Pending);
            };
            self.settle_work_admission(&claimed, &Receipt::observed(&prepared, &actual))
                .await?;
            return Ok(WorkAdmissionStep::Admitted(actual));
        }

        if effect.status != EffectStatus::Pending {
            let Some(claimed) = self.claim_work_observation(&effect, options).await? else {
                return Ok(WorkAdmissionStep::Pending);
            };
            // A read performed before this claim can race a late commit. That
            // does not undermine the owner's durable atomic dedup guarantee;
            // it does mean we cannot record NotApplied or terminal failure.
            self.store
                .reconcile_effect(
                    claimed.lease.as_ref().ok_or(SetupError::InvalidRecord)?,
                    Reconciliation::RetrySafe {
                        evidence: work_observation(
                            &prepared,
                            ObservationKind::ExactOwnerReplaySafe,
                        )?,
                    },
                )
                .await?;
            return Ok(WorkAdmissionStep::RetrySafe);
        }

        let Some(access) = access else {
            return self
                .work_dispatch_blocked(&effect, &prepared, options, false)
                .await;
        };
        if snapshot.state().terminal_revision != 0 {
            return self
                .work_dispatch_blocked(&effect, &prepared, options, false)
                .await;
        }
        // Authentication failures must not prevent the receipt recovery above.
        self.check_work_dispatch(&snapshot, &prepared, &access)?;
        let maximum = u64::from(snapshot.request.host().limits.max_effect_attempts);
        if effect.attempt >= maximum {
            return self
                .work_dispatch_blocked(&effect, &prepared, options, true)
                .await;
        }
        let exact_admission = service
            .admission_identity(
                prepared.admission().admission_key().clone(),
                prepared.admission().work_request().clone(),
            )
            .map_err(SetupWorkAdmissionError::from)?;
        // Preserve pre-claim screening without retaining it as commit custody.
        // The actual admission below prepares a fresh fence and executes its
        // target operation under the current authority owner's guard.
        drop(
            bounded(
                options,
                host.dispatch
                    .admission_fence(prepared.admission(), &exact_admission),
            )
            .await?,
        );
        let claimed = match self
            .store
            .claim_effect(
                &effect.key,
                snapshot.state().revision,
                options.worker.as_str(),
                options.lease_ms,
            )
            .await
        {
            Ok(EffectClaimOutcome::Claimed(effect)) => effect,
            Ok(EffectClaimOutcome::NeedsReconciliation(_)) | Err(StoreError::EffectState) => {
                return Ok(WorkAdmissionStep::Pending);
            }
            Err(error) => return Err(error.into()),
        };
        if claimed.attempt > maximum {
            self.mark_work_observation(&claimed, &prepared, ObservationKind::BudgetExhausted)
                .await?;
            return Ok(WorkAdmissionStep::DispatchBudgetExhausted);
        }
        // The local revision fence is not a reservation of host policy. The
        // actual admission authorizer checks again after this fresh read.
        let current = self.recover(
            self.store
                .get(snapshot.key())
                .await?
                .ok_or(StoreError::NotFound)?,
        )?;
        if current.state().revision != snapshot.state().revision
            || current.state().terminal_revision != 0
        {
            self.mark_work_observation(&claimed, &prepared, ObservationKind::DispatchNotAuthorized)
                .await?;
            return Ok(WorkAdmissionStep::DispatchNotAuthorized);
        }
        if let Err(error) = self.check_work_dispatch(&current, &prepared, &access) {
            self.mark_work_observation(&claimed, &prepared, ObservationKind::DispatchNotAuthorized)
                .await?;
            return Err(error);
        }
        let result = bounded(
            options,
            admit_setup_work_available(service, prepared.admission(), host.dispatch, available),
        )
        .await;
        let admitted = match result {
            Ok(result) => result.into_item().id,
            Err(error) => {
                self.mark_work_observation(&claimed, &prepared, ObservationKind::Ambiguous)
                    .await?;
                return Err(error.into());
            }
        };
        let actual = match bounded(
            options,
            observe_setup_work(service, prepared.admission(), host.observation),
        )
        .await
        {
            Ok(Some(actual)) if actual.work_item_id() == &admitted => actual,
            Ok(_) => {
                self.mark_work_observation(&claimed, &prepared, ObservationKind::MissingAdmission)
                    .await?;
                return Err(SetupWorkAdmissionError::MissingReceipt.into());
            }
            Err(error) => {
                self.mark_work_observation(&claimed, &prepared, ObservationKind::Ambiguous)
                    .await?;
                return Err(error.into());
            }
        };
        self.settle_work_admission(&claimed, &Receipt::observed(&prepared, &actual))
            .await?;
        Ok(WorkAdmissionStep::Admitted(actual))
    }

    fn check_work_dispatch(
        &self,
        snapshot: &SetupSnapshot,
        prepared: &PreparedSetupWork,
        access: &SetupAccess<'_>,
    ) -> Result<(), SetupError> {
        self.check_snapshot(snapshot, access, true)?;
        super::installed_input::verify_work_input_reference(
            snapshot.request(),
            &prepared.admission().correlation().input_reference,
            access.evidence,
            access.context,
            access.references,
        )?;
        Ok(())
    }

    pub(in crate::setup) async fn claim_work_observation(
        &self,
        effect: &StoredEffect,
        options: &WorkAdmissionStepOptions,
    ) -> Result<Option<StoredEffect>, SetupError> {
        if effect.status == EffectStatus::Pending && effect.attempt == 0 {
            // All owner calls require a prior committed dispatch claim. An
            // admission with no attempted handoff violates that composition;
            // do not mint a fake dispatch attempt merely to settle observation.
            return Err(SetupError::InvalidRecord);
        }
        match self
            .store
            .claim_reconciliation(&effect.key, options.worker.as_str(), options.lease_ms)
            .await
        {
            Ok(claimed) => Ok(Some(claimed)),
            // The refreshed nonterminal effect can still be settled by a peer
            // before this claim. Reobserve on the next step rather than treating
            // that normal state race as a permanent dispatch-state failure.
            Err(StoreError::EffectState) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    async fn settle_work_admission(
        &self,
        effect: &StoredEffect,
        receipt: &Receipt,
    ) -> Result<(), SetupError> {
        self.settle_work_receipt(effect, receipt.encode()?).await
    }

    pub(in crate::setup) async fn settle_work_receipt(
        &self,
        effect: &StoredEffect,
        receipt: RecordData,
    ) -> Result<(), SetupError> {
        let lease = effect.lease.as_ref().ok_or(SetupError::InvalidRecord)?;
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

    async fn mark_work_observation(
        &self,
        effect: &StoredEffect,
        prepared: &PreparedSetupWork,
        kind: ObservationKind,
    ) -> Result<(), SetupError> {
        self.mark_work_evidence(effect, work_observation(prepared, kind)?)
            .await
    }

    pub(in crate::setup) async fn mark_work_evidence(
        &self,
        effect: &StoredEffect,
        evidence: RecordData,
    ) -> Result<(), SetupError> {
        let lease = effect.lease.as_ref().ok_or(SetupError::InvalidRecord)?;
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

    async fn work_dispatch_blocked(
        &self,
        effect: &StoredEffect,
        prepared: &PreparedSetupWork,
        options: &WorkAdmissionStepOptions,
        budget: bool,
    ) -> Result<WorkAdmissionStep, SetupError> {
        if effect.attempt > 0 {
            let Some(claimed) = self.claim_work_observation(effect, options).await? else {
                return Ok(WorkAdmissionStep::Pending);
            };
            self.mark_work_observation(
                &claimed,
                prepared,
                if budget {
                    ObservationKind::BudgetExhausted
                } else {
                    ObservationKind::DispatchNotAuthorized
                },
            )
            .await?;
        }
        Ok(if budget {
            WorkAdmissionStep::DispatchBudgetExhausted
        } else {
            WorkAdmissionStep::DispatchNotAuthorized
        })
    }
}

fn work_observation(
    prepared: &PreparedSetupWork,
    kind: ObservationKind,
) -> Result<RecordData, SetupError> {
    Ok(RecordData {
        schema: OBSERVATION_SCHEMA.into(),
        version: 1,
        body: serde_json::json!({
            "preparation_digest": prepared.digest(),
            "admission_key": prepared.admission().admission_key(),
            "correlation_digest": prepared.admission().correlation_digest(),
            "kind": kind,
        }),
    })
}

pub(in crate::setup) fn validate_options(
    options: &WorkAdmissionStepOptions,
) -> Result<(), SetupError> {
    let minimum = options
        .owner_timeout_ms
        .checked_mul(2)
        .and_then(|calls| calls.checked_add(options.settlement_budget_ms));
    if !(1..=86_400_000).contains(&options.lease_ms)
        || options.owner_timeout_ms == 0
        || options.settlement_budget_ms == 0
        || minimum.is_none_or(|minimum| options.lease_ms < minimum)
    {
        return Err(StoreError::Invalid("invalid work admission step bounds".into()).into());
    }
    Ok(())
}

async fn bounded<T>(
    options: &WorkAdmissionStepOptions,
    call: impl Future<Output = Result<T, SetupWorkAdmissionError>>,
) -> Result<T, SetupWorkAdmissionError> {
    tokio::time::timeout(Duration::from_millis(options.owner_timeout_ms), call)
        .await
        .map_err(|_| SetupWorkAdmissionError::Timeout)?
}

impl SetupStore<'_> {
    /// Native execution is unavailable until committed child context propagates
    /// through WorkGraph and runtime admission. No lease or external call occurs.
    pub async fn step_work_admission(
        &self,
        operation: &Id,
        host: &SetupWorkHost<'_>,
        access: Option<SetupAccess<'_>>,
        options: &WorkAdmissionStepOptions,
    ) -> Result<WorkAdmissionStep, SetupError> {
        let available = self.native_work_availability(operation).await?;
        self.step_work_admission_available(operation, host, access, options, available)
            .await
    }

    #[cfg(test)]
    pub(crate) async fn step_work_admission_protocol_fixture(
        &self,
        operation: &Id,
        host: &SetupWorkHost<'_>,
        access: Option<SetupAccess<'_>>,
        options: &WorkAdmissionStepOptions,
    ) -> Result<WorkAdmissionStep, SetupError> {
        self.step_work_admission_available(
            operation,
            host,
            access,
            options,
            NativeWorkAvailable::protocol_fixture(),
        )
        .await
    }
}
