//! One bounded existing acquisition-effect step. WorkGraph admission and helper
//! publication remain distinct facts; neither is a worker execution receipt.
use super::*;
use crate::setup::work_executor::validate_options;
use std::{future::Future, time::Duration};
use toolkit_store::{EffectClaimOutcome, EffectOutcome, EffectStatus, Reconciliation};

/// Actual immutable native admission identity plus owner-local historical data.
/// admitted_request is the exact already-known request, never copied from the
/// observed mutable WorkItem. Retained carries no active custody or permission.
/// This private host result must not be forwarded as work-content disclosure.
pub struct NativeWorkObservation<R> {
    pub realm: String,
    pub namespace: WorkNamespace,
    pub item: WorkItemId,
    pub admission: WorkAdmission,
    pub admitted_request: CreateWorkItemRequest,
    pub retained: R,
}

/// The callback has both observed native admission and published the canonical
/// helper association under the supplied live Dispatch claim. It must not submit
/// helper input before that publication. Native input settlement is separate.
pub struct NativeWorkCompletion<R> {
    pub observation: NativeWorkObservation<R>,
    pub published: PublishedSetupWork,
}

/// Host-owned native composition. Observation uses the selected owner's narrow
/// current metadata authority, which can survive source cancellation or expiry.
/// It first reads identity privately, then gates disclosure through that same
/// observation owner's synchronous current guard, without I/O or await inside
/// it. That guard covers Some, None, conflict and stored-identity error disclosure.
/// This does not borrow the creation fence or renew content access. It performs
/// no create and confers no retry, child, launch or user-disclosure authority.
/// Completion separately requires current complete contributor and operation
/// authority at every native effect and replay disclosure. Native admission uses
/// the exact stable key and its durable atomic Created/Replayed contract. It must recover
/// the observed item/source, never create a replacement after an uncertain reply.
/// Dropping either future must release local waits. An already-entered native
/// effect must transfer its cleanup to the native owner's existing guard, with
/// no detached private worker or leaked custody. A timeout is not cancellation
/// settlement, absence or permission to retry; the exact key is reobserved.
#[async_trait]
pub trait NativeSetupWorkOwner: Send + Sync {
    type Retained: Send + Sync;
    async fn observe(
        &self,
        preparation: &NativeWorkPreparation,
    ) -> Result<Option<NativeWorkObservation<Self::Retained>>, SetupError>;
    async fn complete(
        &self,
        preparation: &NativeWorkPreparation,
        claim: &EffectLease,
        observed: Option<NativeWorkObservation<Self::Retained>>,
    ) -> Result<NativeWorkCompletion<Self::Retained>, SetupError>;
}

pub enum NativeWorkStep<R> {
    /// Historical native admission and prepared helper publication are recorded.
    /// This is neither current authority nor proof that the worker input ran.
    Completed(NativeWorkObservation<R>),
    Pending,
    RetrySafe,
    DispatchBudgetExhausted,
    DispatchNotAuthorized,
}

#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct AdmissionReceipt {
    input_digest: String,
    realm: String,
    namespace: WorkNamespace,
    item: WorkItemId,
    admission: WorkAdmission,
    admitted_request_digest: ContentDigest,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CompletionReceipt {
    admission: AdmissionReceipt,
    published_key: StoreKey,
    published_digest: ContentDigest,
}
const RECEIPT_SCHEMA: &str = "meerkat_toolkit.native_helper_preparation";

impl NativeWorkPreparation {
    fn receipt<R>(
        &self,
        observed: &NativeWorkObservation<R>,
    ) -> Result<AdmissionReceipt, SetupError> {
        if observed.realm != self.work.realm.as_str()
            || observed.namespace != self.work.namespace
            || observed.admission.key != self.admission_key
            || observed.admitted_request.realm_id.as_deref() != Some(observed.realm.as_str())
            || observed.admitted_request.namespace.as_ref() != Some(&observed.namespace)
        {
            return Err(SetupError::InvalidRecord);
        }
        self.metadata
            .validate_admission(&observed.admitted_request, &observed.admission)?;
        Ok(AdmissionReceipt {
            input_digest: self
                .input_record
                .as_ref()
                .ok_or(SetupError::InvalidRecord)?
                .content_digest
                .clone(),
            realm: observed.realm.clone(),
            namespace: observed.namespace.clone(),
            item: observed.item.clone(),
            admission: observed.admission.clone(),
            admitted_request_digest: ContentDigest::of_bytes(&canonical_json(
                &observed.admitted_request,
            )?),
        })
    }
    fn observation(
        &self,
        kind: &str,
        actual: Option<&AdmissionReceipt>,
    ) -> Result<RecordData, SetupError> {
        Ok(RecordData {
            schema: "meerkat_toolkit.native_helper_observation".into(),
            version: 1,
            body: serde_json::json!({"input": self.input_record.as_ref().ok_or(SetupError::InvalidRecord)?.content_digest,
                "admission_key":self.admission_key, "kind":kind, "observed_admission":actual}),
        })
    }
}

impl SetupStore<'_> {
    /// Reopen the exact frozen input internally for restart reconciliation.
    /// The native owner authorizes observation before any result is returned;
    /// this does not expose stored work content or renew expired setup access.
    pub async fn step_native_work_admission_for_operation<O: NativeSetupWorkOwner>(
        &self,
        operation: &Id,
        owner: &O,
        access: Option<SetupAccess<'_>>,
        options: &WorkAdmissionStepOptions,
    ) -> Result<NativeWorkStep<O::Retained>, SetupError> {
        let snapshot = self.recover(
            self.store
                .get(&self.scope.key(operation))
                .await?
                .ok_or(StoreError::NotFound)?,
        )?;
        let saved = self
            .store
            .get(&self.native_input_key(operation))
            .await?
            .ok_or(StoreError::NotFound)?;
        let preparation = self.recover_native_input(&snapshot, saved).await?;
        self.step_native_work_admission(&preparation, owner, access, options)
            .await
    }

    /// Runs at most one admission/publication attempt. Prior native commit is
    /// observed first. A crash between graph commit and source publication stays
    /// uncertain; recovery never reports absence or turns a Reconcile lease into
    /// Dispatch. Exact-key replay still requires fresh access, live cancellation
    /// checks and the original attempt budget.
    pub async fn step_native_work_admission<O: NativeSetupWorkOwner>(
        &self,
        preparation: &NativeWorkPreparation,
        owner: &O,
        access: Option<SetupAccess<'_>>,
        options: &WorkAdmissionStepOptions,
    ) -> Result<NativeWorkStep<O::Retained>, SetupError> {
        validate_options(options)?;
        let snapshot = self.native_step_snapshot(preparation).await?;
        let effect = self.original_work_effect(&snapshot).await?;
        if effect.status == EffectStatus::Failed {
            return Err(SetupError::InvalidRecord);
        }
        let observed = bounded(options, owner.observe(preparation)).await?;
        let observed_receipt = observed
            .as_ref()
            .map(|value| preparation.receipt(value))
            .transpose()?;
        let succeeded_before = effect.status == EffectStatus::Succeeded;
        let snapshot = self.native_step_snapshot(preparation).await?;
        let effect = self.original_work_effect(&snapshot).await?;
        if effect.status == EffectStatus::Failed {
            return Err(SetupError::InvalidRecord);
        }
        if effect.status == EffectStatus::Succeeded {
            if observed.is_none() && !succeeded_before {
                return Ok(NativeWorkStep::Pending);
            }
            let actual = observed.ok_or(SetupWorkAdmissionError::MissingReceipt)?;
            let Some(EffectOutcome::Succeeded { receipt }) = &effect.outcome else {
                return Err(SetupError::InvalidRecord);
            };
            if receipt.schema != RECEIPT_SCHEMA || receipt.version != 1 {
                return Err(SetupError::InvalidRecord);
            }
            let saved: CompletionReceipt = serde_json::from_value(receipt.body.clone())?;
            if saved.admission != preparation.receipt(&actual)?
                || saved.published_key != preparation.key
            {
                return Err(SetupError::InvalidRecord);
            }
            let published = self
                .store
                .get(&saved.published_key)
                .await?
                .ok_or(StoreError::NotFound)?;
            if published.content_digest != saved.published_digest.to_string()
                || published.content_digest != published.data.digest()?
                || published.revision != 1
            {
                return Err(SetupError::InvalidRecord);
            }
            return Ok(NativeWorkStep::Completed(actual));
        }
        // A real admission cannot precede this acquisition's first committed
        // Dispatch claim. Do not fabricate an attempt to adopt foreign work.
        if observed.is_some() && effect.attempt == 0 {
            return Err(SetupError::InvalidRecord);
        }
        if effect.status != EffectStatus::Pending {
            let Some(claimed) = self.claim_work_observation(&effect, options).await? else {
                return Ok(NativeWorkStep::Pending);
            };
            // This relies on the owner's exact-key durable admission contract,
            // not on absence. If the item exists, the next Dispatch attempt can
            // only recover that exact item and its unfinished source/publication.
            self.store
                .reconcile_effect(
                    claimed.lease.as_ref().ok_or(SetupError::InvalidRecord)?,
                    Reconciliation::RetrySafe {
                        evidence: preparation
                            .observation("exact_owner_replay_safe", observed_receipt.as_ref())?,
                    },
                )
                .await?;
            return Ok(NativeWorkStep::RetrySafe);
        }
        let Some(access) = access else {
            return Ok(NativeWorkStep::DispatchNotAuthorized);
        };
        if snapshot.state().terminal_revision != 0 {
            return Ok(NativeWorkStep::DispatchNotAuthorized);
        }
        self.check_native_step_access(&snapshot, preparation, &access)?;
        let maximum = u64::from(snapshot.request.host().limits.max_effect_attempts);
        if effect.attempt >= maximum {
            return Ok(NativeWorkStep::DispatchBudgetExhausted);
        }
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
            Ok(EffectClaimOutcome::Claimed(claimed)) => claimed,
            Ok(EffectClaimOutcome::NeedsReconciliation(_)) | Err(StoreError::EffectState) => {
                return Ok(NativeWorkStep::Pending);
            }
            Err(error) => return Err(error.into()),
        };
        if claimed.attempt > maximum {
            self.mark_work_evidence(
                &claimed,
                preparation.observation("budget_exhausted", observed_receipt.as_ref())?,
            )
            .await?;
            return Ok(NativeWorkStep::DispatchBudgetExhausted);
        }
        let current = self.native_step_snapshot(preparation).await?;
        if current.state().revision != snapshot.state().revision
            || current.state().terminal_revision != 0
        {
            self.mark_work_evidence(
                &claimed,
                preparation.observation("dispatch_not_authorized", observed_receipt.as_ref())?,
            )
            .await?;
            return Ok(NativeWorkStep::DispatchNotAuthorized);
        }
        if let Err(error) = self.check_native_step_access(&current, preparation, &access) {
            self.mark_work_evidence(
                &claimed,
                preparation.observation("dispatch_not_authorized", observed_receipt.as_ref())?,
            )
            .await?;
            return Err(error);
        }
        let completion = match bounded(
            options,
            owner.complete(
                preparation,
                claimed.lease.as_ref().ok_or(SetupError::InvalidRecord)?,
                observed,
            ),
        )
        .await
        {
            Ok(value) => value,
            Err(error) => {
                self.mark_work_evidence(
                    &claimed,
                    preparation.observation("completion_uncertain", observed_receipt.as_ref())?,
                )
                .await?;
                return Err(error);
            }
        };
        let result = self
            .validate_native_completion(preparation, &completion, observed_receipt.as_ref())
            .await;
        let completed = match result {
            Ok(receipt) => receipt,
            Err(error) => {
                self.mark_work_evidence(
                    &claimed,
                    preparation
                        .observation("publication_unconfirmed", observed_receipt.as_ref())?,
                )
                .await?;
                return Err(error);
            }
        };
        let after = match bounded(options, owner.observe(preparation)).await {
            Ok(Some(value)) => value,
            Ok(None) => {
                self.mark_work_evidence(
                    &claimed,
                    preparation.observation("missing_admission", Some(&completed.admission))?,
                )
                .await?;
                return Err(SetupWorkAdmissionError::MissingReceipt.into());
            }
            Err(error) => {
                self.mark_work_evidence(
                    &claimed,
                    preparation.observation("observation_uncertain", Some(&completed.admission))?,
                )
                .await?;
                return Err(error);
            }
        };
        if preparation
            .receipt(&after)
            .is_ok_and(|receipt| receipt == completed.admission)
        {
            self.settle_work_receipt(
                &claimed,
                RecordData {
                    schema: RECEIPT_SCHEMA.into(),
                    version: 1,
                    body: serde_json::to_value(completed)?,
                },
            )
            .await?;
            Ok(NativeWorkStep::Completed(completion.observation))
        } else {
            self.mark_work_evidence(
                &claimed,
                preparation.observation("admission_changed", Some(&completed.admission))?,
            )
            .await?;
            Err(SetupError::InvalidRecord)
        }
    }

    async fn native_step_snapshot(
        &self,
        preparation: &NativeWorkPreparation,
    ) -> Result<SetupSnapshot, SetupError> {
        let snapshot = self.recover(
            self.store
                .get(&preparation.effect.record)
                .await?
                .ok_or(StoreError::NotFound)?,
        )?;
        let effect = self.original_work_effect(&snapshot).await?;
        validate_preparation(self, &snapshot, &effect, preparation)?;
        let expected = preparation
            .input_record
            .as_ref()
            .ok_or(SetupError::InvalidRecord)?;
        let saved = self
            .store
            .get(&expected.key)
            .await?
            .ok_or(StoreError::NotFound)?;
        if saved.revision != expected.revision
            || saved.content_digest != expected.content_digest
            || saved.content_digest != saved.data.digest()?
        {
            return Err(SetupError::InvalidRecord);
        }
        Ok(snapshot)
    }

    fn check_native_step_access(
        &self,
        snapshot: &SetupSnapshot,
        preparation: &NativeWorkPreparation,
        access: &SetupAccess<'_>,
    ) -> Result<(), SetupError> {
        self.check_snapshot(snapshot, access, true)?;
        crate::setup::installed_input::verify_work_input_reference(
            snapshot.request(),
            preparation.input_reference(),
            access.evidence,
            access.context,
            access.references,
        )?;
        Ok(())
    }

    async fn validate_native_completion<R>(
        &self,
        preparation: &NativeWorkPreparation,
        completion: &NativeWorkCompletion<R>,
        prior: Option<&AdmissionReceipt>,
    ) -> Result<CompletionReceipt, SetupError> {
        let admission = preparation.receipt(&completion.observation)?;
        if prior.is_some_and(|prior| prior != &admission) {
            return Err(SetupError::InvalidRecord);
        }
        let published = &completion.published;
        let helper = published
            .record
            .helper_source
            .as_ref()
            .ok_or(SetupError::InvalidRecord)?;
        if published.key != preparation.key
            || canonical_json(&published.record.work)? != canonical_json(&preparation.work)?
            || helper.effect != preparation.effect
            || helper.item != admission.item
            || helper.admission != admission.admission
            || canonical_json(&helper.admitted_request)?
                != canonical_json(&completion.observation.admitted_request)?
        {
            return Err(SetupError::InvalidRecord);
        }
        let saved = self
            .store
            .get(&published.key)
            .await?
            .ok_or(StoreError::NotFound)?;
        if saved.content_digest != published.digest.to_string()
            || saved.content_digest != saved.data.digest()?
            || saved.revision != 1
            || saved.data.schema != SCHEMA
            || saved.data.version != VERSION
            || canonical_json(&saved.data.body)? != canonical_json(&published.record)?
        {
            return Err(SetupError::InvalidRecord);
        }
        Ok(CompletionReceipt {
            admission,
            published_key: published.key.clone(),
            published_digest: published.digest.clone(),
        })
    }
}

async fn bounded<T>(
    options: &WorkAdmissionStepOptions,
    call: impl Future<Output = Result<T, SetupError>>,
) -> Result<T, SetupError> {
    tokio::time::timeout(Duration::from_millis(options.owner_timeout_ms), call)
        .await
        .map_err(|_| SetupWorkAdmissionError::Timeout)?
}
