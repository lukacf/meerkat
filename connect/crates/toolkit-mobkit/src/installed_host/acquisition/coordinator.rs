//! One optional tool invocation through the existing acquisition-effect owner.
//! WorkGraph coalescing precedes helper materialization. Publication and native
//! input acceptance stay distinct; retries retain the native item and source.

use super::*;
use crate::admission_progress::{AdmissionProgress, AdmissionStage};
use crate::{
    original_ingress::helper_originals::{
        AdmittedHelperWork, HelperAdmissionSelection, HelperOperation, bind_work_request,
    },
    private_originals::{PrivateOriginalError, PrivateOriginalSubmission, PrivateSourceSettlement},
};
use meerkat_core::types::ToolCallView;
use meerkat_toolkit::{
    authority::{SetupProposalConstraints, SetupRequestEvidence},
    request::ValidatedSetupRequest,
    setup::{
        NativeSetupWorkOwner, NativeWorkCompletion, NativeWorkObservation, NativeWorkStep,
        PreparedWorkPublicationConditions, SetupSnapshot, SetupWorkDelegation,
        SetupWorkPreparation,
    },
    work_admission::{SetupWorkAdmissionError, SetupWorkObservationAuthorizer},
};
use meerkat_workgraph::{WorkAdmission, WorkAdmissionOutcome};
use toolkit_package::{ContentDigest, canonical_json};
use toolkit_store::{EffectKey, EffectLease, StoreError};

/// An observation of the setup handoff, never a tool installation or original
/// task completion. Native uncertainty remains visible and replayable.
#[derive(serde::Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(crate) enum SetupWorkerObservation {
    AcquisitionTerminal,
    Pending,
    RetrySafe,
    DispatchBudgetExhausted,
    DispatchNotAuthorized,
    InputAdmitted {
        deduplicated: bool,
        source_settlement: PrivateSourceSettlement,
    },
    /// Shared planning launch, durable transfer and native context admission
    /// are separate observations. None is an individual setup decision or
    /// completion; pending context delivery remains owned by host recovery.
    WorkflowLead {
        observation: crate::private_originals::WorkflowLaunchObservation,
        #[serde(skip_serializing_if = "Option::is_none")]
        context_transfer:
            Option<crate::private_originals::setup_workflow::WorkflowTransferObservation>,
        #[serde(skip_serializing_if = "Option::is_none")]
        context_delivery: Option<WorkflowContextDeliveryStep>,
    },
    AdmissionUnresolved,
    NativeOutcomeUnavailable,
}

/// Delivery failures do not erase an already completed disclosure or G launch.
/// The caller sees the typed boundary; host recovery retains its entered owner.
#[derive(serde::Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(crate) enum WorkflowContextDeliveryStep {
    Observed {
        observation: crate::private_originals::WorkflowDeliveryObservation,
    },
    Denied,
    Conflict,
    Unavailable,
}
impl From<Result<crate::private_originals::WorkflowDeliveryObservation, PrivateOriginalError>>
    for WorkflowContextDeliveryStep
{
    fn from(
        result: Result<crate::private_originals::WorkflowDeliveryObservation, PrivateOriginalError>,
    ) -> Self {
        match result {
            Ok(observation) => Self::Observed { observation },
            Err(PrivateOriginalError::Denied) => Self::Denied,
            Err(PrivateOriginalError::Conflict) => Self::Conflict,
            Err(PrivateOriginalError::Unavailable) => Self::Unavailable,
        }
    }
}

// Preserve the native stale-observation fact through the installed adapter.
// This carries no nonentry certificate or authority to replay earlier effects.
#[derive(Debug, thiserror::Error)]
pub(crate) enum SetupAdvanceError {
    #[error(transparent)]
    Setup(#[from] SetupError),
    #[error("workflow binding observation is stale")]
    ObservationChanged,
}

impl InstalledAcquisition {
    // Keep the existing prepare/depart owner and error precedence together. The
    // marker retains the refusal boundary before cleanup changes the last stage;
    // it does not identify an inner cause or grant admission.
    pub(crate) async fn prepare_workflow_effects(
        planning: &Arc<crate::private_originals::InstalledWorkflowPlanning>,
        membership: &meerkat_toolkit::setup::SetupWorkflowWaiter,
        original: &crate::private_originals::acquisition::AcquisitionOriginal,
        access: SetupAccess<'_>,
        progress: &AdmissionProgress,
    ) -> Result<(), SetupError> {
        progress.enter(AdmissionStage::PrepareEffects);
        if let Err(error) = planning
            .prepare_waiter_effects(membership, original, access)
            .await
        {
            progress.refused();
            #[cfg(any(test, feature = "test-support"))]
            aa1_private_failure("advance.workflow_prepare_effects", error);
            // Close the exact interest and reconcile every deterministic
            // child ID, including a write whose response was uncertain.
            // Cleanup uncertainty remains unavailable, never success.
            progress.enter(AdmissionStage::DepartAfterFailure);
            progress.cleanup_started();
            planning.depart_waiter(membership).await.map_err(|error| {
                progress.cleanup_failed();
                #[cfg(any(test, feature = "test-support"))]
                aa1_private_failure("advance.workflow_depart_after_failure", error);
                source_error(error)
            })?;
            progress.cleanup_completed();
            return Err(source_error(error));
        }
        Ok(())
    }

    pub(crate) async fn advance(
        &self,
        operation: &Id,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        entered: &meerkat_core::EnteredDispatchOperation,
        review: crate::review_custody::ReviewCustody,
        progress: &AdmissionProgress,
    ) -> Result<SetupWorkerObservation, SetupAdvanceError> {
        let started = tokio::time::Instant::now();
        progress.enter(AdmissionStage::AdvanceHost);
        let admission = self
            .host
            .admit_invocation(call.id, context)
            .await
            .map_err(|error| {
                #[cfg(any(test, feature = "test-support"))]
                crate::aa1_admission_failure("advance.host", &error);
                host_admission_error(error)
            })?;
        let authorization = context.work_authorization().ok_or(SetupError::Denied)?;
        let current = admission.context.borrowed(authorization);
        let store = SetupStore::new_conditional(self.store.as_ref(), self.scope.clone())?;
        progress.enter(AdmissionStage::AdvanceLoad);
        let snapshot = store
            .load(
                operation,
                &admission.evidence,
                &current,
                self.references.as_ref(),
            )
            .await
            .inspect_err(|_error| {
                #[cfg(any(test, feature = "test-support"))]
                crate::aa1_setup_failure("advance.load", _error);
            })?;
        if snapshot.state().terminal_revision != 0 {
            return Ok(SetupWorkerObservation::AcquisitionTerminal);
        }
        // Anchor the remaining request budget at this call's start, so time
        // spent admitting/loading is never added back. Both helper paths reuse
        // it. The complete path also retains its existing outer owner timeout;
        // the work lease allocation is not a proof of liveness after storage.
        let remaining = snapshot
            .request()
            .host()
            .limits
            .deadline_ms
            .checked_sub(current.now_ms)
            .ok_or(SetupError::Rejected)?;
        let source_deadline = started
            .checked_add(Duration::from_millis(remaining))
            .ok_or(SetupError::Rejected)?;
        let content = String::from_utf8(
            canonical_json(snapshot.request().request()).map_err(SetupError::from)?,
        )
        .map_err(|_| SetupError::InvalidRecord)?;
        let originals = &self.host.originals;
        progress.enter(AdmissionStage::AdvanceOriginal);
        let original = originals
            .acquisition_original(context)
            .await
            .map_err(|error| {
                #[cfg(any(test, feature = "test-support"))]
                aa1_private_failure("advance.workflow_original", error);
                source_error(error)
            })?;
        if let Some(planning) = originals
            .retained_workflow_selection(&original.original, &snapshot.request().host().correlation)
            .map_err(|error| {
                #[cfg(any(test, feature = "test-support"))]
                aa1_private_failure("advance.workflow_selection", error);
                source_error(error)
            })?
        {
            // Durable membership is independent for every A/B/C request. The
            // existing single launch outbox arbitrates G, without creating a
            // per-waiter helper or interpreting a workflow receipt as consent.
            let membership = store.workflow_waiter(
                planning.terms(),
                &snapshot,
                original.source_record.key.clone(),
            )?;
            progress.enter(AdmissionStage::WaiterJoin);
            let joined = planning
                .join_waiter_conditionally(
                    &self.store,
                    &membership,
                    &original,
                    SetupAccess {
                        evidence: &admission.evidence,
                        context: &current,
                        references: self.references.as_ref(),
                    },
                    &self.work_backend,
                    (context, source_deadline),
                )
                .await
                .map_err(|error| {
                    #[cfg(any(test, feature = "test-support"))]
                    aa1_private_failure("advance.workflow_waiter_join", error);
                    source_error(error)
                })?
                .inspect_err(|_error| {
                    #[cfg(any(test, feature = "test-support"))]
                    crate::aa1_setup_failure("advance.workflow_waiter_admission", _error);
                })?;
            progress.enter(AdmissionStage::RetainInterest);
            planning
                .retain_launch_interest(
                    &joined.snapshot,
                    &membership,
                    &original,
                    Some((context, source_deadline)),
                )
                .await
                .map_err(|error| {
                    #[cfg(any(test, feature = "test-support"))]
                    aa1_private_failure("advance.workflow_retain_interest", error);
                    source_error(error)
                })?;
            Self::prepare_workflow_effects(
                &planning,
                &membership,
                &original,
                SetupAccess {
                    evidence: &admission.evidence,
                    context: &current,
                    references: self.references.as_ref(),
                },
                progress,
            )
            .await?;
            let transfer = originals
                .retained_workflow_context_transfer(
                    &original.original,
                    &snapshot.request().host().correlation,
                )
                .map_err(|error| {
                    #[cfg(any(test, feature = "test-support"))]
                    aa1_private_failure("advance.workflow_transfer_selection", error);
                    source_error(error)
                })?;
            let context_transfer = if let Some(transfer) = &transfer {
                let selected_control = (|| {
                    let selected = self
                        .transfer_recovery
                        .lock()
                        .map_err(|_| SetupError::NativeExecutionUnavailable)?;
                    selected
                        .as_ref()
                        .and_then(std::sync::Weak::upgrade)
                        .ok_or(SetupError::NativeExecutionUnavailable)
                })();
                // The selection guard is gone before any test diagnostic I/O.
                let control = selected_control.inspect_err(|_error| {
                    #[cfg(any(test, feature = "test-support"))]
                    crate::aa1_setup_failure("advance.workflow_recovery_owner", _error);
                })?;
                progress.enter(AdmissionStage::CommitTransfer);
                Some(
                    transfer
                        .commit_entered(
                            &snapshot,
                            &membership,
                            call,
                            context,
                            &self.work_backend,
                            &control,
                            entered,
                            review.clone(),
                            source_deadline,
                        )
                        .await
                        .map_err(|error| {
                            #[cfg(any(test, feature = "test-support"))]
                            aa1_private_failure("advance.workflow_commit", error);
                            source_error(error)
                        })?,
                )
            } else {
                None
            };
            progress.enter(AdmissionStage::Launch);
            let observation = planning
                .launch_from_dispatch_with_review(
                    &joined.snapshot,
                    review.clone(),
                    context,
                    source_deadline,
                )
                .await
                .map_err(|error| match error {
                    crate::private_originals::WorkflowChildPreparationError::ObservationChanged => {
                        crate::aa1_failure(
                            "advance.workflow_launch",
                            "WorkflowChildPreparationError::ObservationChanged",
                        );
                        SetupAdvanceError::ObservationChanged
                    }
                    crate::private_originals::WorkflowChildPreparationError::Refused(error) => {
                        #[cfg(any(test, feature = "test-support"))]
                        aa1_private_failure("advance.workflow_launch", error);
                        SetupAdvanceError::Setup(source_error(error))
                    }
                })?;
            // This bounded poll may admit the exact completed context after G
            // becomes available. Caller loss or a slow native/store step leaves
            // the registered delivery owner for the existing recovery worker.
            let context_delivery = match transfer {
                Some(transfer) => {
                    progress.enter(AdmissionStage::ContextDelivery);
                    Some(
                        self.poll_workflow_context_delivery_with_review(&transfer, review.clone())
                            .await
                            .into(),
                    )
                }
                None => None,
            };
            return Ok(SetupWorkerObservation::WorkflowLead {
                observation,
                context_transfer,
                context_delivery,
            });
        }
        progress.enter(AdmissionStage::PrepareWork);
        let preparation = self
            .prepare_bound_work(
                &store,
                &snapshot,
                SetupAccess {
                    evidence: &admission.evidence,
                    context: &current,
                    references: self.references.as_ref(),
                },
                context,
            )
            .await
            .inspect_err(|_error| {
                #[cfg(test)]
                crate::aa1_setup_failure("advance.prepare_work", _error);
            })?;
        if preparation.input_reference().content_digest
            != ContentDigest::of_bytes(content.as_bytes())
        {
            return Err(SetupError::InvalidRecord.into());
        }
        let owner = SameHostSetupWorkOwner {
            acquisition: self,
            selection: self.selected_helper(snapshot.request())?,
            operation,
            call,
            context,
            content: &content,
            source_deadline,
            entered,
            review: &review,
        };
        progress.enter(AdmissionStage::WorkStep);
        let step = store
            .step_native_work_admission(
                &preparation,
                &owner,
                Some(SetupAccess {
                    evidence: &admission.evidence,
                    context: &current,
                    references: self.references.as_ref(),
                }),
                &self.work_step,
            )
            .await
            .inspect_err(|_error| {
                #[cfg(test)]
                crate::aa1_setup_failure("advance.work_step", _error);
            })?;
        let observed = match step {
            NativeWorkStep::Completed(observed) => observed,
            NativeWorkStep::Pending => return Ok(SetupWorkerObservation::Pending),
            NativeWorkStep::RetrySafe => return Ok(SetupWorkerObservation::RetrySafe),
            NativeWorkStep::DispatchBudgetExhausted => {
                return Ok(SetupWorkerObservation::DispatchBudgetExhausted);
            }
            NativeWorkStep::DispatchNotAuthorized => {
                return Ok(SetupWorkerObservation::DispatchNotAuthorized);
            }
        };
        // Completion is a persisted work/source publication fact. Prepare the
        // same native destination, recover that exact source, and only then
        // submit input. This path is also used after lost effect settlement.
        let originals = &self.host.originals;
        // Metadata recovery carries no live source handles. Current M1 replay
        // reacquires the exact complete contributor set before native runtime
        // preparation, without ever creating work on this completed path.
        progress.enter(AdmissionStage::ObserveHelper);
        #[cfg(any(test, feature = "test-support"))]
        crate::observe_helper_probe::observe(call.id);
        let admitted = originals
            .observe_setup_helper_for_live_replay(
                context,
                entered,
                self.work_backend.service(),
                preparation.admission_key().clone(),
                preparation.work_request().clone(),
                owner.operation(&preparation)?,
                owner.selection(),
                review.clone(),
            )
            .await
            .map_err(|error| {
                #[cfg(test)]
                aa1_private_failure("advance.observe_helper", error);
                source_error(error)
            })?
            .ok_or_else(|| {
                #[cfg(test)]
                crate::aa1_failure("advance.helper_absent", "None");
                SetupError::NativeExecutionUnavailable
            })?;
        match_observation(&observed, &SameHostSetupWorkOwner::observation(&admitted)?)?;
        progress.enter(AdmissionStage::PrepareHelper);
        let prepared = originals
            .prepare_setup_helper(admitted, content.clone(), source_deadline, review.clone())
            .await
            .map_err(|error| {
                #[cfg(test)]
                aa1_private_failure("advance.prepare_helper", error);
                source_error(error)
            })?;
        progress.enter(AdmissionStage::RecoverHelper);
        // One churn for this recovery's waits: its retention's lineage
        // resolution.
        let mut churn = context.owner_change_churn();
        let held = originals
            .recover_setup_helper_source(&prepared, context, &mut churn)
            .await
            .map_err(|error| {
                #[cfg(test)]
                aa1_private_failure("advance.recover_helper", error);
                source_error(error)
            })?
            .ok_or_else(|| {
                #[cfg(test)]
                crate::aa1_failure("advance.source_absent", "None");
                SetupError::NativeExecutionUnavailable
            })?;
        progress.enter(AdmissionStage::HostAfterPublication);
        let admission = self
            .host
            .admit_invocation(call.id, context)
            .await
            .map_err(|error| {
                #[cfg(test)]
                crate::aa1_admission_failure("advance.host_after_publication", &error);
                host_admission_error(error)
            })?;
        let current = admission.context.borrowed(authorization);
        progress.enter(AdmissionStage::LoadAfterPublication);
        let snapshot = store
            .load(
                operation,
                &admission.evidence,
                &current,
                self.references.as_ref(),
            )
            .await
            .inspect_err(|_error| {
                #[cfg(test)]
                crate::aa1_setup_failure("advance.load", _error);
            })?;
        if snapshot.state().terminal_revision != 0
            || current.now_ms >= snapshot.request().host().limits.deadline_ms
        {
            return Err(SetupError::Rejected.into());
        }
        // Historical publication must match this exact source and actual
        // destination. No new Dispatch basis is fabricated on this read path.
        progress.enter(AdmissionStage::LoadPublication);
        let published = store
            .load_prepared_work_exact(
                operation,
                &held.original().association().candidate().original_work,
                SetupAccess {
                    evidence: &admission.evidence,
                    context: &current,
                    references: self.references.as_ref(),
                },
                &self.work_backend,
            )
            .await
            .inspect_err(|_error| {
                #[cfg(test)]
                crate::aa1_setup_failure("advance.load_publication", _error);
            })?;
        if published.child().association() != held.original().association()
            || &published.child().association().candidate().target != prepared.target()
        {
            return Err(SetupError::InvalidRecord.into());
        }
        progress.enter(AdmissionStage::SubmitHelper);
        let submission = originals
            .submit_setup_helper(
                &prepared,
                held,
                &snapshot,
                content,
                source_deadline,
                review.clone(),
                Some(context),
            )
            .await
            .map_err(|error| {
                #[cfg(test)]
                aa1_private_failure("advance.submit_helper", error);
                source_error(error)
            })?;
        let crate::private_originals::setup_helpers::SetupHelperSubmission::Native(submission) =
            submission
        else {
            // Both in-call attempts were definitely not admitted. Only this
            // same current invocation can replay its completed work/source;
            // a later model call has another correlation, and no worker is
            // scheduled here. The original deadline and effect stay unchanged.
            return Ok(SetupWorkerObservation::RetrySafe);
        };
        Ok(match submission {
            PrivateOriginalSubmission::Admitted {
                deduplicated,
                source_settlement,
                ..
            } => SetupWorkerObservation::InputAdmitted {
                deduplicated,
                source_settlement,
            },
            PrivateOriginalSubmission::AdmissionUnresolved => {
                SetupWorkerObservation::AdmissionUnresolved
            }
            PrivateOriginalSubmission::NativeOutcomeUnavailable => {
                SetupWorkerObservation::NativeOutcomeUnavailable
            }
        })
    }
}

struct SameHostSetupWorkOwner<'a> {
    acquisition: &'a InstalledAcquisition,
    selection: HelperAdmissionSelection,
    operation: &'a Id,
    call: ToolCallView<'a>,
    context: &'a ToolDispatchContext,
    content: &'a str,
    source_deadline: tokio::time::Instant,
    entered: &'a meerkat_core::EnteredDispatchOperation,
    review: &'a crate::review_custody::ReviewCustody,
}

impl SameHostSetupWorkOwner<'_> {
    fn selection(&self) -> HelperAdmissionSelection {
        self.selection.clone()
    }
    fn operation(
        &self,
        preparation: &NativeWorkPreparation,
    ) -> Result<HelperOperation, SetupError> {
        if preparation.input_reference().content_digest
            != ContentDigest::of_bytes(self.content.as_bytes())
        {
            return Err(SetupError::InvalidRecord);
        }
        Ok(HelperOperation {
            acquisition: preparation.effect_key().record.clone(),
            effect: preparation.effect_id().to_owned(),
            input_digest: preparation.input_reference().content_digest.clone(),
        })
    }
    fn observation(admitted: &AdmittedHelperWork) -> Result<NativeWorkObservation<()>, SetupError> {
        Ok(NativeWorkObservation {
            realm: admitted.realm().to_owned(),
            namespace: admitted.namespace().clone(),
            item: admitted.item_id().clone(),
            admission: admitted.admission().clone(),
            admitted_request: admitted
                .admitted_request()
                .map_err(|_| SetupError::InvalidRecord)?,
            retained: (),
        })
    }
}

#[async_trait]
impl NativeSetupWorkOwner for SameHostSetupWorkOwner<'_> {
    type Retained = ();

    async fn observe(
        &self,
        preparation: &NativeWorkPreparation,
    ) -> Result<Option<NativeWorkObservation<Self::Retained>>, SetupError> {
        self.acquisition
            .observe_private_work(preparation, self.review.clone())
            .await
            .inspect_err(|_error| {
                #[cfg(test)]
                crate::aa1_setup_failure("observe.private_work", _error);
            })
    }

    async fn complete(
        &self,
        preparation: &NativeWorkPreparation,
        claim: &EffectLease,
        observed: Option<NativeWorkObservation<Self::Retained>>,
    ) -> Result<NativeWorkCompletion<Self::Retained>, SetupError> {
        let acquisition = self.acquisition;
        let originals = &acquisition.host.originals;
        let operation = self.operation(preparation)?;
        // One churn for this work step's owner-change waits: its helper
        // admission, its retention's lineage resolution and its two reads
        // after the helper's grants were issued and its source bound.
        let mut churn = self.context.owner_change_churn();
        // Observation is historical metadata only. Creation or replay must
        // enter the actual native current full-source fence on every attempt.
        let admitted = originals
            .admit_setup_helper(
                self.context,
                self.entered,
                acquisition.work_backend.service(),
                preparation.admission_key().clone(),
                preparation.work_request().clone(),
                operation,
                self.selection(),
                self.review.clone(),
                &mut churn,
            )
            .await
            .map_err(|error| {
                #[cfg(test)]
                aa1_private_failure("complete.admit_helper", error);
                source_error(error)
            })?;
        if let Some(observed) = observed.as_ref() {
            match_observation(observed, &Self::observation(&admitted)?)?;
        }
        // No helper runtime is touched until the native graph has selected its
        // exact item. Native preparation owns any session materialization.
        let prepared = originals
            .prepare_setup_helper(
                admitted.clone(),
                self.content.to_owned(),
                self.source_deadline,
                self.review.clone(),
            )
            .await
            .map_err(|error| {
                #[cfg(test)]
                aa1_private_failure("complete.prepare_helper", error);
                source_error(error)
            })?;
        let held = match originals
            .recover_setup_helper_source(&prepared, self.context, &mut churn)
            .await
            .map_err(|error| {
                #[cfg(test)]
                aa1_private_failure("complete.recover_helper", error);
                source_error(error)
            })? {
            Some(source) => source,
            None => {
                // Native must reconcile a child issued before a lost source
                // append. Public issue_child and a prior authority check do not
                // provide the required current check-and-issue reservation.
                let (lineage, operation_basis) = originals
                    .issue_or_reconcile_setup_helper_grants(
                        self.context,
                        self.entered,
                        preparation,
                        &prepared,
                        self.review.clone(),
                    )
                    .await
                    .map_err(|error| {
                        #[cfg(test)]
                        aa1_private_failure("complete.issue_controller", error);
                        source_error(error)
                    })?;
                originals
                    .retain_setup_helper_source(
                        &prepared,
                        lineage,
                        operation_basis,
                        self.context,
                        &mut churn,
                    )
                    .await
                    .map_err(|error| {
                        #[cfg(test)]
                        aa1_private_failure("complete.retain_source", error);
                        source_error(error)
                    })?
            }
        };
        let admission = acquisition
            .host
            .admit_invocation(self.call.id, self.context)
            .await
            .map_err(|error| {
                #[cfg(test)]
                crate::aa1_admission_failure("complete.host_after_issue", &error);
                host_admission_error(error)
            })?;
        let authorization = self
            .context
            .work_authorization()
            .ok_or(SetupError::Denied)?;
        let current = admission.context.borrowed(authorization);
        let store =
            SetupStore::new_conditional(acquisition.store.as_ref(), acquisition.scope.clone())?;
        let snapshot = store
            .load(
                self.operation,
                &admission.evidence,
                &current,
                acquisition.references.as_ref(),
            )
            .await?;
        // Lost settlement keeps the original publication's Dispatch basis. A
        // fresh effect attempt may observe it, never replace it with its claim.
        let published = match store
            .load_prepared_work_for_operation(
                self.operation,
                prepared.target(),
                SetupAccess {
                    evidence: &admission.evidence,
                    context: &current,
                    references: acquisition.references.as_ref(),
                },
                &acquisition.work_backend,
            )
            .await
        {
            Ok(published) => {
                if published.child().association() != held.original().association() {
                    return Err(SetupError::InvalidRecord);
                }
                published
            }
            Err(SetupError::Store(StoreError::NotFound)) => {
                #[cfg(any(test, feature = "test-support"))]
                if let Some(hold) = crate::source_hold_probe::observe(
                    crate::source_hold_probe::Site::PublicationSource,
                    self.call.id,
                ) {
                    crate::source_hold_probe::retain(originals.hold_sources_for_test(hold));
                }
                let helper_source = originals
                    .setup_helper_publication_source(
                        &prepared,
                        &held,
                        acquisition.work_backend.service(),
                        self.context,
                        &mut churn,
                    )
                    .await
                    .map_err(|error| {
                        #[cfg(test)]
                        aa1_private_failure("complete.publication_source", error);
                        source_error(error)
                    })?;
                let mut constraints: SetupProposalConstraints =
                    admission.evidence.constraints().clone();
                // These are Toolkit proposal bounds only. The association,
                // operation and controller grants all come from native owners.
                constraints.expires_at_ms = constraints
                    .expires_at_ms
                    .min(snapshot.request().host().limits.deadline_ms);
                let child = SetupRequestEvidence::new(
                    helper_source.association.clone(),
                    constraints.clone(),
                )
                .map_err(|_| SetupError::InvalidRecord)?;
                let delegation = SetupWorkDelegation {
                    destination: prepared.target().clone(),
                    realm_id: helper_source
                        .association
                        .candidate()
                        .ingress_namespace
                        .realm
                        .clone(),
                    original_work: helper_source.association.candidate().original_work.clone(),
                    delegation_resource: snapshot.request().host().target.clone(),
                    ceiling: constraints.ceiling,
                    expires_at_ms: constraints.expires_at_ms,
                    composition_digest: ContentDigest::of_bytes(&canonical_json(&(
                        "toolkit.same-host-helper-composition.v1",
                        admitted.selection(),
                        prepared.target(),
                        prepared.controller().selection(),
                        preparation.backend_pin(),
                    ))?),
                };
                let proposal = store
                    .bind_native_work(
                        &snapshot,
                        preparation,
                        delegation,
                        SetupAccess {
                            evidence: &admission.evidence,
                            context: &current,
                            references: acquisition.references.as_ref(),
                        },
                    )
                    .await?;
                let mut conditions = originals
                    .setup_helper_contributor_conditions(&admitted, self.context, &mut churn)
                    .await
                    .map_err(|error| {
                        #[cfg(test)]
                        aa1_private_failure("complete.contributor_conditions", error);
                        source_error(error)
                    })?;
                conditions.push(helper_source.source.clone());
                store
                    .publish_prepared_helper_work(
                        &snapshot,
                        &proposal,
                        child,
                        helper_source,
                        SetupAccess {
                            evidence: &admission.evidence,
                            context: &current,
                            references: acquisition.references.as_ref(),
                        },
                        PreparedWorkPublicationConditions {
                            owners: conditions,
                            dispatch_claim: claim.clone(),
                            before_ms: i64::try_from(constraints.expires_at_ms)
                                .map_err(|_| SetupError::InvalidRecord)?,
                        },
                    )
                    .await
                    .inspect_err(|_error| {
                        #[cfg(test)]
                        crate::aa1_setup_failure("complete.publish_helper", _error);
                    })?
            }
            Err(error) => return Err(error),
        };
        // Native input has not been submitted. The acquisition effect owner
        // validates publication and settles this Dispatch claim first.
        Ok(NativeWorkCompletion {
            observation: Self::observation(&admitted)?,
            published,
        })
    }
}

#[cfg(any(test, feature = "test-support"))]
fn aa1_private_failure(stage: &'static str, error: PrivateOriginalError) {
    crate::aa1_failure(
        stage,
        match error {
            PrivateOriginalError::Denied => "PrivateOriginalError::Denied",
            PrivateOriginalError::Conflict => "PrivateOriginalError::Conflict",
            PrivateOriginalError::Unavailable => "PrivateOriginalError::Unavailable",
        },
    );
}

/// A source owner's answer as a step's: its own refusal denies; a conflict
/// in the source's facts is contention (another writer's change in
/// flight), unavailable like an owner that cannot answer now, retryable.
fn source_error(error: PrivateOriginalError) -> SetupError {
    match error {
        PrivateOriginalError::Denied => SetupError::Denied,
        PrivateOriginalError::Conflict | PrivateOriginalError::Unavailable => {
            SetupError::NativeExecutionUnavailable
        }
    }
}

fn match_observation(
    expected: &NativeWorkObservation<()>,
    actual: &NativeWorkObservation<()>,
) -> Result<(), SetupError> {
    if expected.realm != actual.realm
        || expected.namespace != actual.namespace
        || expected.item != actual.item
        || expected.admission != actual.admission
        || canonical_json(&expected.admitted_request)? != canonical_json(&actual.admitted_request)?
    {
        return Err(SetupError::InvalidRecord);
    }
    Ok(())
}

/// Source-bound preparation through the existing immutable input owner. Raw
/// user context retains its existing input reference. Only this source owner's
/// reserved final reference is appended; arbitrary provider refs stay verbatim.
struct BoundSetupPreparer<'a> {
    inner: &'a dyn SetupWorkPreparer,
    effect: EffectKey,
    origin: crate::original_ingress::helper_originals::HelperSetupOrigin,
    contributors: Vec<Arc<crate::original_ingress::RetainedToolkitOriginal>>,
    selection: HelperAdmissionSelection,
    backend: &'a SetupWorkBackend,
}
impl SetupWorkPreparer for BoundSetupPreparer<'_> {
    fn prepare(
        &self,
        request: &ValidatedSetupRequest,
    ) -> Result<SetupWorkPreparation, SetupWorkAdmissionError> {
        let mut proposed = self.inner.prepare(request)?;
        if proposed.delegation.is_some()
            || proposed.scope.realm() != self.backend.pin().realm()
            || proposed.scope.namespace() != self.backend.pin().namespace()
            || proposed
                .work_request
                .realm_id
                .as_ref()
                .is_some_and(|realm| realm != proposed.scope.realm().as_str())
            || proposed
                .work_request
                .namespace
                .as_ref()
                .is_some_and(|namespace| namespace != proposed.scope.namespace())
        {
            return Err(SetupWorkAdmissionError::ScopeMismatch);
        }
        proposed.work_request.realm_id = Some(proposed.scope.realm().to_string());
        proposed.work_request.namespace = Some(proposed.scope.namespace().clone());
        let operation = HelperOperation {
            acquisition: self.effect.record.clone(),
            effect: self.effect.id.clone(),
            input_digest: proposed.input_reference.content_digest.clone(),
        };
        bind_work_request(
            &mut proposed.work_request,
            &operation,
            &self.origin,
            &self.contributors,
            &self.selection,
        )
        .map_err(|_| SetupWorkAdmissionError::ReservedCorrelation)?;
        Ok(proposed)
    }
}

impl InstalledAcquisition {
    pub(super) async fn prepare_bound_work(
        &self,
        store: &SetupStore<'_>,
        snapshot: &SetupSnapshot,
        access: SetupAccess<'_>,
        context: &ToolDispatchContext,
    ) -> Result<NativeWorkPreparation, SetupError> {
        let original = self
            .host
            .originals
            .acquisition_original(context)
            .await
            .map_err(|error| {
                #[cfg(test)]
                aa1_private_failure("prepare.original", error);
                source_error(error)
            })?;
        let bound = BoundSetupPreparer {
            inner: self.preparer.as_ref(),
            effect: store.native_work_effect_key(snapshot).await?,
            origin: original.setup_origin(),
            contributors: original.contributors,
            selection: self.selected_helper(snapshot.request())?,
            backend: &self.work_backend,
        };
        store
            .prepare_native_work(snapshot, access, &bound, &self.work_backend)
            .await
    }

    /// Read only the exact frozen request for this host's internal settlement
    /// sink. This method never returns source handles or renewed permissions.
    async fn observe_private_work(
        &self,
        preparation: &NativeWorkPreparation,
        review: crate::review_custody::ReviewCustody,
    ) -> Result<Option<NativeWorkObservation<()>>, SetupError> {
        self.authorize_observation(preparation).await?;
        let service = self.work_backend.service().clone();
        let admission = service
            .admission_identity(
                preparation.admission_key().clone(),
                preparation.work_request().clone(),
            )
            .map_err(SetupWorkAdmissionError::from)?;
        let key = preparation.admission_key().clone();
        let request = preparation.work_request().clone();
        let runtime = tokio::runtime::Handle::current();
        // The private async read is outside all publication/activation custody.
        // A timed-out blocking backend may finish later, but cannot disclose.
        let read = tokio::task::spawn_blocking(move || {
            let _review = review;
            runtime.block_on(service.observe_admission(key, request))
        })
        .await
        .map_err(|_| SetupError::NativeExecutionUnavailable)
        .and_then(|result| result.map_err(|_| SetupError::NativeExecutionUnavailable));
        let projected = read.and_then(|read| match read {
            None => Ok(None),
            Some(WorkAdmissionOutcome::Replayed(item)) => Ok(Some(NativeWorkObservation {
                realm: item.realm_id,
                namespace: item.namespace,
                item: item.id,
                admission: admission.clone(),
                admitted_request: preparation.work_request().clone(),
                retained: (),
            })),
            Some(WorkAdmissionOutcome::Created(_) | WorkAdmissionOutcome::Conflict { .. }) => {
                Err(SetupError::InvalidRecord)
            }
        });
        let mut disclosed = None;
        self.with_current_disclosure(
            preparation,
            &admission,
            Box::new(|| disclosed = Some(projected)),
        )?;
        // No await or I/O after the same owner's synchronous guarded transfer.
        disclosed.ok_or(SetupError::NativeExecutionUnavailable)?
    }
}

#[async_trait]
impl SetupWorkObservationAuthorizer<NativeWorkPreparation> for InstalledAcquisition {
    async fn authorize_observation(
        &self,
        preparation: &NativeWorkPreparation,
    ) -> Result<(), SetupWorkAdmissionError> {
        SetupStore::new_conditional(self.store.as_ref(), self.scope.clone())
            .map_err(recovery_error)?
            .validate_native_work_recovery(preparation, &self.work_backend)
            .await
            .map_err(recovery_error)?;
        let admission = self.work_backend.service().admission_identity(
            preparation.admission_key().clone(),
            preparation.work_request().clone(),
        )?;
        self.with_current_disclosure(preparation, &admission, Box::new(|| {}))
    }
    fn with_current_disclosure(
        &self,
        preparation: &NativeWorkPreparation,
        admission: &WorkAdmission,
        disclose: Box<dyn FnOnce() + Send + '_>,
    ) -> Result<(), SetupWorkAdmissionError> {
        if preparation.backend_pin() != self.work_backend.pin()
            || preparation.effect_key().record.realm != self.host.tenant.as_str()
        {
            return Err(SetupWorkAdmissionError::Denied);
        }
        let expected = self.work_backend.service().admission_identity(
            preparation.admission_key().clone(),
            preparation.work_request().clone(),
        )?;
        if admission != &expected {
            return Err(SetupWorkAdmissionError::Denied);
        }
        self.host
            .originals
            .with_private_setup_recovery(
                self.host.tenant.as_str(),
                self.host.deployment.as_str(),
                disclose,
            )
            .map_err(|error| match error {
                PrivateOriginalError::Unavailable | PrivateOriginalError::Conflict => {
                    SetupWorkAdmissionError::NativeExecutionUnavailable
                }
                PrivateOriginalError::Denied => SetupWorkAdmissionError::Denied,
            })
    }
}

fn recovery_error(error: SetupError) -> SetupWorkAdmissionError {
    match error {
        SetupError::InvalidRecord | SetupError::WrongScope | SetupError::Denied => {
            SetupWorkAdmissionError::Denied
        }
        _ => SetupWorkAdmissionError::NativeExecutionUnavailable,
    }
}

#[cfg(test)]
mod tests {
    use meerkat_toolkit::host_lifetime::{
        NativeHostLifetime,
        control::{HostCloseReason, PrivateHostLifetimeControl},
    };
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    // The source owner's answer at a Find's advance and work step: a
    // conflict in the source's facts is contention (another writer's change
    // in flight), so the step answers it unavailable, retryable, not an
    // inconsistent record; only the owner's own refusal denies.
    //
    // RED before the fix: a conflict answers `InvalidRecord`.
    #[test]
    fn a_source_conflict_in_a_step_is_unavailable_not_an_invalid_record() {
        use super::{PrivateOriginalError, SetupError, source_error};
        assert!(
            matches!(
                source_error(PrivateOriginalError::Conflict),
                SetupError::NativeExecutionUnavailable
            ),
            "a conflict is contention"
        );
        assert!(matches!(
            source_error(PrivateOriginalError::Unavailable),
            SetupError::NativeExecutionUnavailable
        ));
        assert!(matches!(
            source_error(PrivateOriginalError::Denied),
            SetupError::Denied
        ));
    }

    // Real selected-artifact and activation custody. The value is already a
    // private immutable projection when close/replacement occurs. No provider,
    // native work grant, helper launch or source custody is faked by this test.
    #[test]
    fn private_projection_is_withheld_if_host_closes_after_read() {
        let (_directory, selection) = super::super::super::tests::fixture();
        let owner = NativeHostLifetime::bootstrap(selection).unwrap();
        owner.with_current_setup(|| Ok(())).unwrap();
        let private_projection = Some("observed immutable identity");
        owner
            .begin_close_activation(HostCloseReason::Shutdown)
            .unwrap();
        let disclosed = Arc::new(AtomicBool::new(false));
        let seen = disclosed.clone();
        assert!(
            owner
                .with_current_setup(|| {
                    assert!(private_projection.is_some());
                    seen.store(true, Ordering::SeqCst);
                    Ok(())
                })
                .is_err()
        );
        assert!(!disclosed.load(Ordering::SeqCst));
    }

    #[test]
    fn private_projection_is_withheld_if_selected_artifact_changes_after_read() {
        let (_directory, selection) = super::super::super::tests::fixture();
        let owner = NativeHostLifetime::bootstrap(selection.clone()).unwrap();
        owner.with_current_setup(|| Ok(())).unwrap();
        let private_projection = Some("observed immutable identity");
        std::fs::write(&selection.local_policy.path, b"changed policy").unwrap();
        let disclosed = Arc::new(AtomicBool::new(false));
        let seen = disclosed.clone();
        assert!(
            owner
                .with_current_setup(|| {
                    assert!(private_projection.is_some());
                    seen.store(true, Ordering::SeqCst);
                    Ok(())
                })
                .is_err()
        );
        assert!(!disclosed.load(Ordering::SeqCst));
    }
}
