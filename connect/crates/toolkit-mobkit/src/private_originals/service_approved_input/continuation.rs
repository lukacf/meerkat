//! B's existing source owner supplies historical admission to the acquisition
//! and its existing continuation outbox. This module never submits an input.
use super::*;
use meerkat_runtime::SessionServiceRuntimeExt;
use meerkat_toolkit::{
    continuation::{ContinuationAdmissionEvidence, ContinuationError, ContinuationObligation, SetupTerminalOutcome},
    setup::{ApprovedServiceInput, ApprovedServiceSelection, ApprovedServiceSourceOwner, SetupError},
    request::ContextReference,
};
use toolkit_package::ContentDigest;

struct ServiceSource<'a> { source: &'a PrivateOriginals }

impl PrivateOriginals {
    pub(super) async fn settle_service_step_refusal(&self, stored: &StoredRecord,
        result: Result<(), PrivateOriginalError>) -> Result<(), PrivateOriginalError>
    {
        if result == Err(PrivateOriginalError::Denied) {
            let record = self.decode_stored(stored)?;
            let reference = PrivateOriginalRef { route: record.event.route.clone(), original: record.event.original.clone(),
                event_sha256: digest(&canonical_json(&record.event).map_err(|_| PrivateOriginalError::Conflict)?) };
            // Denial alone is not non-entry evidence. The source writer must
            // fence B and confirm its immutable binding was never published.
            self.reject_service_continuation(&reference).await?;
        }
        result
    }

    pub(super) fn service_setup_store(&self) -> Result<SetupStore<'_>, PrivateOriginalError> {
        SetupStore::new(self.store.as_ref(), SetupScope::new(
            Id::new(self.scope.realm.clone()).map_err(|_| PrivateOriginalError::Conflict)?,
            Id::new(self.scope.owner.clone()).map_err(|_| PrivateOriginalError::Conflict)?,
        )).map_err(|_| PrivateOriginalError::Unavailable)
    }

    pub(in crate::private_originals) fn service_selection(&self, stored: &StoredRecord, record: &SourceRecord)
        -> Result<ApprovedServiceSelection, PrivateOriginalError>
    {
        let task = record.approved_task.as_ref().ok_or(PrivateOriginalError::Denied)?;
        Ok(ApprovedServiceSelection {
            source: stored.key.clone(),
            original_work: meerkat_authorization_contracts::work_association::OriginalWorkRef {
                authority: record.source.source_authority.clone(), work: record.event.original.clone(),
            },
            source_identity: ContentDigest::of_bytes(&canonical_json(&(&record.source, &record.event, task))
                .map_err(|_| PrivateOriginalError::Conflict)?),
        })
    }

    async fn selected_service_source(&self, snapshot: &SetupSnapshot, selected: &ApprovedServiceSelection)
        -> Result<(StoredRecord, SourceRecord), PrivateOriginalError>
    {
        self.service_input_host_current()?;
        if selected.source.realm != self.scope.realm || selected.source.owner != self.scope.owner {
            return Err(PrivateOriginalError::Denied);
        }
        let stored = self.store.get(&selected.source).await.map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        let record = self.decode_stored(&stored)?;
        if self.service_selection(&stored, &record)? != *selected { return Err(PrivateOriginalError::Conflict); }
        let current = self.service_task_snapshot(&record).await?;
        if current.key() != snapshot.key()
            || current.request().input_digest() != snapshot.request().input_digest()
            || current.request().host().continuation != snapshot.request().host().continuation
        { return Err(PrivateOriginalError::Conflict); }
        self.service_console_login(&record)?;
        self.service_input_host_current()?;
        Ok((stored, record))
    }

    pub(super) async fn reserve_service_continuation(&self, stored: &StoredRecord)
        -> Result<(), PrivateOriginalError>
    {
        let record = self.decode_stored(stored)?;
        let snapshot = self.service_task_snapshot(&record).await?;
        let selected = self.service_selection(stored, &record)?;
        if let Some(prior) = snapshot.approved_service() {
            return if prior == &selected { Ok(()) } else { Err(PrivateOriginalError::Conflict) };
        }
        // A bound historical B without this pre-entry choice is not retroactive
        // authorization to replace A's continuation.
        if record.final_association.is_some() || record.service_admission.is_some()
            || !self.service_task_submission_current(&record).await?
        { return Err(PrivateOriginalError::Unavailable); }
        self.service_setup_store()?.reserve_approved_service(&snapshot, selected.clone(),
            &ServiceSource { source: self }).await.map_err(|_| PrivateOriginalError::Unavailable)?;
        // A committed CAS whose reply was lost is recovered on the next page.
        let saved = self.service_task_snapshot(&record).await?;
        if saved.approved_service() != Some(&selected) { return Err(PrivateOriginalError::Conflict); }
        Ok(())
    }

    pub(super) async fn settle_service_continuation(&self, record: &SourceRecord) -> Result<(), PrivateOriginalError> {
        let snapshot = self.service_task_snapshot(record).await?;
        let selected = snapshot.approved_service().ok_or(PrivateOriginalError::Unavailable)?;
        let key = self.key(&PrivateOriginalRef { route: record.event.route.clone(),
            original: record.event.original.clone(), event_sha256: digest(&canonical_json(&record.event)
                .map_err(|_| PrivateOriginalError::Conflict)?) })?;
        if selected.source != key { return Err(PrivateOriginalError::Conflict); }
        self.service_setup_store()?.observe_approved_service(&snapshot, &ServiceSource { source: self })
            .await.map_err(|_| PrivateOriginalError::Unavailable)?;
        Ok(())
    }

    /// Close only this approved source through the existing cancellation writer.
    /// That writer serializes with first binding and retains a native-visible
    /// cancellation before returning. A bound source is never non-entry proof.
    pub(super) async fn reject_service_continuation(&self, reference: &PrivateOriginalRef)
        -> Result<(), PrivateOriginalError>
    {
        self.reject_service_continuation_with_failure(reference, None, None, None)
            .await
    }

    pub(super) async fn reject_failed_service_attachment(
        &self, reference: &PrivateOriginalRef,
        failure: meerkat_toolkit::setup::ApprovedServiceAttachmentFailure,
    ) -> Result<(), PrivateOriginalError> {
        self.reject_failed_service_attachment_with_observation(reference, failure, None)
            .await
    }

    pub(super) async fn reject_failed_service_attachment_with_observation(
        &self,
        reference: &PrivateOriginalRef,
        failure: meerkat_toolkit::setup::ApprovedServiceAttachmentFailure,
        observation: Option<meerkat_toolkit::setup::ApprovedServiceAttachmentObservation>,
    ) -> Result<(), PrivateOriginalError> {
        self.reject_service_continuation_with_failure(reference, Some(failure), observation, None)
            .await
    }

    pub(super) async fn reject_failed_service_attachment_member(
        &self, reference: &PrivateOriginalRef, grant: &PrivateServiceGrantRef,
        failure: meerkat_toolkit::setup::ApprovedServiceAttachmentFailure,
        observation: Option<meerkat_toolkit::setup::ApprovedServiceAttachmentObservation>,
    ) -> Result<(), PrivateOriginalError> {
        let saved = self.store.get(&self.key(reference)?).await.map_err(store_error)?.ok_or(PrivateOriginalError::Unavailable)?;
        let record = self.decode_record(&saved, reference)?;
        let task = record.approved_task.as_ref().ok_or(PrivateOriginalError::Denied)?;
        task.attachment_member(grant)?;
        let batch_member = matches!(task, ApprovedTaskLink::Batch(_)).then_some(grant);
        self.reject_service_continuation_with_failure(reference, Some(failure), observation, batch_member).await?;
        self.retire_service_attachment(reference, task).await;
        self.compensate_rejected_batch(reference).await
    }

    async fn reject_service_continuation_with_failure(
        &self, reference: &PrivateOriginalRef,
        failure: Option<meerkat_toolkit::setup::ApprovedServiceAttachmentFailure>,
        observation: Option<meerkat_toolkit::setup::ApprovedServiceAttachmentObservation>,
        member: Option<&PrivateServiceGrantRef>,
    ) -> Result<(), PrivateOriginalError> {
        let row = self.store.get(&self.key(reference)?).await.map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        let record = self.decode_record(&row, reference)?;
        let snapshot = self.service_task_snapshot(&record).await?;
        let Some(selected) = snapshot.approved_service() else { return Ok(()); };
        if self.service_selection(&row, &record)? != *selected { return Err(PrivateOriginalError::Conflict); }
        self.record_task_group_cancelled_with_member(reference, failure, observation, member)
            .await?;
        let current = self.store.get(&row.key).await.map_err(store_error)?.ok_or(PrivateOriginalError::Unavailable)?;
        let current = self.decode_record(&current, reference)?;
        if current.service_admission.is_some() {
            return self.settle_service_continuation(&current).await;
        }
        let owner = ServiceSource { source: self };
        let setup = self.service_setup_store()?;
        // Replay uses the exact cause atomically retained by the cancellation
        // owner, even when a prior SetupStore commit or its reply was lost.
        match current.service_attachment_failure {
            Some(failure) => {
                setup
                    .reject_failed_service_attachment_with_observation(
                        &snapshot,
                        &owner,
                        failure,
                        current.service_attachment_observation,
                    )
                    .await
            }
            None => setup.reject_approved_service(&snapshot, &owner).await,
        }.map_err(|_| PrivateOriginalError::Unavailable)?;
        Ok(())
    }

    /// Only the canonical native-failure cause plus the source owner's durable
    /// B non-entry proof permits A's existing result path. A lost live attempt,
    /// generic denial, cancellation or expiry cannot reconstruct this cause.
    pub(crate) async fn failed_service_allows_original_result(
        &self, obligation: &ContinuationObligation,
    ) -> Result<bool, ContinuationError> {
        let Some(failure) = obligation.service_attachment_failure() else { return Ok(false); };
        if obligation.approved_service().is_none()
            || !matches!(obligation.outcome(), SetupTerminalOutcome::Unavailable { .. })
        { return Err(ContinuationError::InvalidReceipt); }
        let setup = self.service_setup_store().map_err(|_| ContinuationError::OwnerUnavailable)?;
        let canonical = setup.committed_continuation(obligation.operation()).await
            .map_err(|_| ContinuationError::OwnerUnavailable)?.ok_or(ContinuationError::OwnerUnavailable)?;
        if canonical.digest()? != obligation.digest()?
            || canonical.service_attachment_failure() != Some(failure)
            || canonical.service_attachment_observation()
                != obligation.service_attachment_observation()
        { return Err(ContinuationError::InvalidReceipt); }
        let binding = obligation.continuation().native_original.as_ref().ok_or(ContinuationError::Unsupported)?;
        let snapshot = setup.service_task_snapshot(obligation.operation(), binding).await
            .map_err(|_| ContinuationError::OwnerUnavailable)?;
        let (_, source) = self.selected_service_source(&snapshot, obligation.approved_service().unwrap()).await
            .map_err(|_| ContinuationError::OwnerUnavailable)?;
        if source.service_attachment_failure != Some(failure)
            || source.service_attachment_observation != obligation.service_attachment_observation()
        {
            return Err(ContinuationError::InvalidReceipt);
        }
        // This reads the actual source and native cancellation owner. It
        // refuses any immutable B binding/admission, including uncertainty.
        let rejected = self.observe_service_rejection(obligation).await?.is_some();
        self.service_input_host_current().map_err(|_| ContinuationError::Denied)?;
        Ok(rejected)
    }

    pub(crate) async fn observe_service_rejection(&self, obligation: &ContinuationObligation)
        -> Result<Option<ContextReference>, ContinuationError>
    {
        let selected = obligation.approved_service().ok_or(ContinuationError::Unsupported)?;
        let binding = obligation.continuation().native_original.as_ref().ok_or(ContinuationError::Unsupported)?;
        let snapshot = self.service_setup_store().map_err(|_| ContinuationError::OwnerUnavailable)?
            .service_task_snapshot(obligation.operation(), binding).await
            .map_err(|_| ContinuationError::OwnerUnavailable)?;
        if snapshot.approved_service() != Some(selected)
            || matches!(obligation.outcome(), SetupTerminalOutcome::ApprovedServiceAdmitted { .. })
        { return Err(ContinuationError::InvalidReceipt); }
        let Some(source) = (ServiceSource { source: self }).observe_rejection(&snapshot, selected).await
            .map_err(|_| ContinuationError::OwnerUnavailable)? else { return Ok(None); };
        // This is an authenticated non-entry observation, not an admission
        // receipt. The existing outbox records OwnerRejected/Failed separately.
        Ok(Some(ContextReference { owner: obligation.continuation().owner.clone(),
            id: source.id.clone(), revision: source.revision.clone(),
            content_digest: ContentDigest::of_bytes(&canonical_json(&(obligation.digest()?, source))?) }))
    }

    /// Authenticate the existing source receipt, including its native input.
    /// This is observation-only and cannot restore a lost Candidate guard.
    pub(crate) async fn observe_service_continuation(&self, obligation: &ContinuationObligation)
        -> Result<Option<ContinuationAdmissionEvidence>, ContinuationError>
    {
        let selected = obligation.approved_service().ok_or(ContinuationError::Unsupported)?;
        let setup = self.service_setup_store().map_err(|_| ContinuationError::OwnerUnavailable)?;
        let binding = obligation.continuation().native_original.as_ref().ok_or(ContinuationError::Unsupported)?;
        let snapshot = setup.service_task_snapshot(obligation.operation(), binding).await
            .map_err(|_| ContinuationError::OwnerUnavailable)?;
        if snapshot.approved_service() != Some(selected) { return Err(ContinuationError::InvalidReceipt); }
        let Some(admission) = (ServiceSource { source: self }).observe_admission(&snapshot, selected).await
            .map_err(|_| ContinuationError::OwnerUnavailable)? else { return Ok(None); };
        if let SetupTerminalOutcome::ApprovedServiceAdmitted { admission: committed, .. } = obligation.outcome() {
            if committed != &admission { return Err(ContinuationError::InvalidReceipt); }
        }
        let now = u64::try_from(self.store.now_ms().await.map_err(|_| ContinuationError::OwnerUnavailable)?)
            .map_err(|_| ContinuationError::OwnerUnavailable)?;
        self.service_input_host_current().map_err(|_| ContinuationError::Denied)?;
        // The continuation owner authenticates a reference to the actual B
        // source receipt. This is not a native ContinuationReceipt or a binding
        // receipt synthesized for A's unchanged service ceiling.
        let receipt = ContextReference { owner: obligation.continuation().owner.clone(),
            id: admission.source_admission.id.clone(), revision: admission.source_admission.revision.clone(),
            content_digest: admission.source_admission.content_digest.clone() };
        Ok(Some(ContinuationAdmissionEvidence {
            receipt, obligation_digest: obligation.digest()?, delivery_identity: obligation.delivery_identity()?,
            runtime: admission.runtime, session: admission.session,
            durable_input: ContextReference { owner: admission.source_admission.owner,
                id: admission.input, revision: admission.source_admission.revision,
                content_digest: admission.source_admission.content_digest }, observed_at_ms: now,
        }))
    }
}

#[async_trait::async_trait]
impl ApprovedServiceSourceOwner for ServiceSource<'_> {
    async fn observe_rejection(&self, snapshot: &SetupSnapshot, selected: &ApprovedServiceSelection)
        -> Result<Option<ContextReference>, SetupError>
    {
        let (stored, record) = self.source.selected_service_source(snapshot, selected).await
            .map_err(|_| SetupError::Denied)?;
        if !record.operation_cancelled || record.final_association.is_some() || record.service_admission.is_some() {
            return Ok(None);
        }
        let namespace = meerkat_authorization_contracts::work_association::QualifiedIngressNamespace {
            realm: record.source.realm.clone(), ingress: record.source.ingress.clone(),
            occurrence_scope: record.event.occurrence.clone(),
        };
        let expected = cancellation_evidence(&stored, &namespace, &selected.original_work)
            .map_err(|_| SetupError::InvalidRecord)?;
        let meerkat_runtime::meerkat_machine::OriginalDisposition::OperationCancelled { evidence } =
            self.source.owner.original_disposition(&namespace, &selected.original_work)
                .map_err(|_| ContinuationError::OwnerUnavailable)?
        else { return Ok(None); };
        if evidence != expected { return Err(SetupError::InvalidRecord); }
        let digest = ContentDigest::of_bytes(&canonical_json(&(selected, evidence))?);
        Ok(Some(ContextReference { owner: Id::new(selected.source.owner.clone()).map_err(|_| SetupError::InvalidRecord)?,
            id: Id::new(selected.source.id.clone()).map_err(|_| SetupError::InvalidRecord)?,
            revision: Revision::new(digest.to_string()).map_err(|_| SetupError::InvalidRecord)?, content_digest: digest }))
    }

    async fn verify_selection(&self, snapshot: &SetupSnapshot, selected: &ApprovedServiceSelection)
        -> Result<(), SetupError>
    {
        let (_, record) = self.source.selected_service_source(snapshot, selected).await
            .map_err(|_| SetupError::Denied)?;
        if !self.source.service_task_submission_current(&record).await.map_err(|_| SetupError::Denied)? {
            return Err(SetupError::Denied);
        }
        Ok(())
    }

    async fn observe_admission(&self, snapshot: &SetupSnapshot, selected: &ApprovedServiceSelection)
        -> Result<Option<ApprovedServiceInput>, SetupError>
    {
        let (_, record) = self.source.selected_service_source(snapshot, selected).await
            .map_err(|_| SetupError::Denied)?;
        let Some(admission) = &record.service_admission else { return Ok(None); };
        let task = record.approved_task.as_ref().ok_or(SetupError::InvalidRecord)?;
        let association = record.final_association.as_ref().ok_or(SetupError::InvalidRecord)?;
        if association.candidate().original_work != selected.original_work
            || admission.session_id != task.original_task.origin_session_id.to_string()
        { return Err(SetupError::InvalidRecord); }
        let input = meerkat_core::lifecycle::InputId::from_uuid(admission.input_id.parse().map_err(|_| SetupError::InvalidRecord)?);
        let native = self.source.machine().map_err(|_| SetupError::Denied)?
            .input_state(&task.original_task.origin_session_id, &input).await
            .map_err(|_| ContinuationError::OwnerUnavailable)?.ok_or(ContinuationError::OwnerUnavailable)?;
        if native.state.input_id != input || !native.state.authority_contributors.iter()
            .any(|source| source.input_id() == &input && source.association() == association)
        { return Err(SetupError::InvalidRecord); }
        // Native input presence authenticates historical acceptance only because
        // this same canonical row ALREADY records settled service_admission.
        // With that field absent this reader still cannot recreate custody.
        // The recovery worker must first use the actual native admitted-original
        // callback and then commit this source acknowledgement, without replay.
        let (_, current) = self.source.selected_service_source(snapshot, selected).await
            .map_err(|_| SetupError::Denied)?;
        if current.service_admission != record.service_admission
            || current.final_association != record.final_association
        { return Err(SetupError::InvalidRecord); }
        if matches!(snapshot.state().lifecycle_phase,
            toolkit_catalog::AcquisitionPhase::Investigating | toolkit_catalog::AcquisitionPhase::Preparing)
            && (current.operation_cancelled || self.source.approved_task_is_cancelled(&current).await
                .map_err(|_| SetupError::Denied)?)
        { return Err(SetupError::Denied); }
        let content = canonical_json(&(selected, association, admission))?;
        let digest = ContentDigest::of_bytes(&content);
        Ok(Some(ApprovedServiceInput {
            source_admission: ContextReference {
                owner: Id::new(selected.source.owner.clone()).map_err(|_| SetupError::InvalidRecord)?,
                id: Id::new(selected.source.id.clone()).map_err(|_| SetupError::InvalidRecord)?,
                revision: Revision::new(digest.to_string()).map_err(|_| SetupError::InvalidRecord)?, content_digest: digest,
            },
            runtime: Id::new(task.original_task.retained_work.runtime_id()).map_err(|_| SetupError::InvalidRecord)?,
            session: Id::new(admission.session_id.clone()).map_err(|_| SetupError::InvalidRecord)?,
            input: Id::new(admission.input_id.clone()).map_err(|_| SetupError::InvalidRecord)?,
        }))
    }
}
