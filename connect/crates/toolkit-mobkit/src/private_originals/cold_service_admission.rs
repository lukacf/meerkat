//! Restore only approved service B's historical source custody. The source row
//! is evidence, and native owns admission, actor/controller identity and phase.
use super::*;
use crate::original_ingress::OriginalCancellationSource;
use meerkat_mobkit::identity_first::ControllerPinSource;
use meerkat_runtime::{Input, identifiers::IdempotencyKey, input_state::InputLifecycleState,
    meerkat_machine::{AdmittedOriginalRefusal, AuthenticatedOriginal}};
use super::service_approved_input::ServiceInputAdmission;

/// Native Input intentionally has no Eq and omits process-only ingress/resume
/// handles from serde. Keep the exact canonical serialized value and validate
/// its typed roundtrip; no deserialization restores those handles.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct RetainedBoundInput(serde_json::Value);

impl RetainedBoundInput {
    pub(crate) fn from_bound(input: &Input) -> Result<Self, PrivateOriginalError> {
        if input.header().ingress_context.is_none() || input.header().retained_resume.is_some() {
            return Err(PrivateOriginalError::Conflict);
        }
        let value = Self(serde_json::to_value(input).map_err(|_| PrivateOriginalError::Conflict)?);
        value.decode()?;
        Ok(value)
    }

    /// Retain first-input bytes under the existing live source. Retry matching
    /// uses native replay equivalence, but returns the original exact ID/time.
    /// The caller must rebind current process ingress/controller before use.
    pub(super) fn retain_for_attempt(
        slot: &OnceLock<Self>, input: &Input,
    ) -> Result<Option<Input>, PrivateOriginalError> {
        let candidate = Self::from_bound(input)?;
        if slot.set(candidate).is_ok() { return Ok(None); }
        let retained = slot.get().ok_or(PrivateOriginalError::Unavailable)?;
        if !retained.same_binding(input)? { return Err(PrivateOriginalError::Conflict); }
        retained.decode().map(Some)
    }

    pub(crate) fn decode(&self) -> Result<Input, PrivateOriginalError> {
        let input: Input = serde_json::from_value(self.0.clone()).map_err(|_| PrivateOriginalError::Conflict)?;
        if !matches!(input, Input::Prompt(_))
            || input.header().authority_association.is_none()
            || input.header().idempotency_key.is_none()
            || serde_json::to_value(&input).map_err(|_| PrivateOriginalError::Conflict)? != self.0
        { return Err(PrivateOriginalError::Conflict); }
        Ok(input)
    }

    fn same_binding(&self, input: &Input) -> Result<bool, PrivateOriginalError> {
        let Input::Prompt(saved) = self.decode()? else { return Err(PrivateOriginalError::Conflict); };
        let Input::Prompt(mut candidate) = input.clone() else { return Err(PrivateOriginalError::Conflict); };
        // Match native prompt replay's observation-only ID/time treatment.
        // The retained original values and the actual outgoing input remain
        // untouched; key, association and every semantic slot must be exact.
        candidate.header.id = saved.header.id;
        candidate.header.timestamp = saved.header.timestamp;
        Ok(serde_json::to_value(Input::Prompt(candidate)).map_err(|_| PrivateOriginalError::Conflict)? == self.0)
    }
}

pub(super) fn source_event_key(record: &SourceRecord) -> Result<IdempotencyKey, PrivateOriginalError> {
    let coordinate = canonical_json(&serde_json::json!({
        "schema": "toolkit.original-occurrence.v1", "realm": record.source.realm,
        "ingress": record.source.ingress, "source_authority": record.source.source_authority,
        "original": record.event.original, "occurrence": record.event.occurrence,
    })).map_err(|_| PrivateOriginalError::Conflict)?;
    Ok(IdempotencyKey::new(format!("toolkit-original/{}", digest(&coordinate))))
}

pub(super) fn validate_retained_input(record: &SourceRecord) -> Result<(), PrivateOriginalError> {
    let Some(saved) = &record.bound_service_input else { return Ok(()); };
    let Input::Prompt(input) = saved.decode()? else { return Err(PrivateOriginalError::Conflict); };
    if (record.approved_task.is_none() && record.setup_request.is_none()
        && record.configured_setup.is_none())
        || input.header.authority_association != record.final_association
        || input.header.idempotency_key.as_ref() != Some(&source_event_key(record)?)
        || !matches!(&input.content, meerkat_core::ContentInput::Text(text)
            if digest(text.as_bytes()) == record.event.content_sha256)
    { return Err(PrivateOriginalError::Conflict); }
    Ok(())
}

pub(super) struct HistoricalServiceAdmission {
    pub admission: ServiceInputAdmission,
    pub phase: InputLifecycleState,
}

pub(super) fn writer_error(error: OriginalWriterError) -> PrivateOriginalError {
    match error {
        OriginalWriterError::OperationCancelled | OriginalWriterError::OperationDenied => PrivateOriginalError::Denied,
        OriginalWriterError::Conflict | OriginalWriterError::WrongOwner | OriginalWriterError::UnsupportedMandate => PrivateOriginalError::Conflict,
        _ => PrivateOriginalError::Unavailable,
    }
}

impl PrivateOriginals {
    pub(crate) fn bind_controller_pin_source(&self, pins: Arc<dyn ControllerPinSource>)
        -> Result<(), PrivateOriginalError>
    {
        // The existing runtime owns this service. A strong reverse link would
        // cycle through the service's machine and its installed source policy.
        self.controller_pin_source.set(Arc::downgrade(&pins)).map_err(|_| PrivateOriginalError::Denied)
    }

    pub(super) async fn retain_bound_service_input(
        &self, reference: &PrivateOriginalRef, original: &Arc<RetainedToolkitOriginal>, input: &Input,
    ) -> Result<(), PrivateOriginalError> {
        let _writer = self.owner.source_writer().await;
        self.owner.check_bound_source(original).map_err(writer_error)?;
        let key = self.key(reference)?;
        let stored = self.store.get(&key).await.map_err(store_error)?.ok_or(PrivateOriginalError::Unavailable)?;
        let mut record = self.decode_record(&stored, reference)?;
        if record.final_association.as_ref() != Some(original.association()) {
            return Err(PrivateOriginalError::Conflict);
        }
        if record.approved_task.is_none() {
            // Preserve ordinary fresh input behavior for other source classes.
            // They receive no new cold-recovery evidence or classification.
            if !self.stock_setup_source(&record, reference)? { return Ok(()); }
            self.check_historical_setup_selection(&record, reference).await?;
        }
        if record.operation_cancelled || self.approved_task_is_cancelled(&record).await? {
            return Err(PrivateOriginalError::Denied);
        }
        if let Some(saved) = &record.bound_service_input {
            return if saved.same_binding(input)? { Ok(()) } else { Err(PrivateOriginalError::Conflict) };
        }
        record.bound_service_input = Some(RetainedBoundInput::from_bound(input)?);
        validate_retained_input(&record)?;
        self.store.compare_and_swap(Transition {
            key, expected_revision: stored.revision, expected_digest: Some(stored.content_digest),
            claim: None, data: encode(record)?, events: vec![], effects: vec![],
        }).await.map_err(store_error)?;
        Ok(())
    }

    /// None is native NotAdmitted, never permission to submit again. Errors
    /// retain conflict/unavailable/cancellation distinctions. This method does
    /// not restore other original classes, a Console login or a live attachment.
    pub(super) async fn restore_service_admission(&self, reference: &PrivateOriginalRef)
        -> Result<Option<HistoricalServiceAdmission>, PrivateOriginalError>
    {
        let writer = self.owner.try_source_writer().map_err(writer_error)?;
        // Enclose the canonical reads in the existing publication sequence.
        // This is conservative invalidation, not a new authority/revision owner.
        let (_, stamp) = self.owner.publication().observe(|| ())
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        let stored = self.store.get(&self.key(reference)?).await.map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        let record = self.decode_record(&stored, reference)?;
        let task = record.approved_task.as_ref().ok_or(PrivateOriginalError::Conflict)?;
        let association = record.final_association.as_ref().ok_or(PrivateOriginalError::Unavailable)?;
        let input = record.bound_service_input.as_ref().ok_or(PrivateOriginalError::Unavailable)?.decode()?;
        let login = self.service_console_login(&record)?;
        let snapshot = self.service_task_snapshot(&record).await?;
        if snapshot.approved_service() != Some(&self.service_selection(&stored, &record)?) {
            return Err(PrivateOriginalError::Conflict);
        }
        let mcp = match task {
            service_approved_input::ApprovedTaskLink::Single(member) =>
                vec![self.historical_task_access(reference, &member.grant, association).await?],
            service_approved_input::ApprovedTaskLink::Batch(_) => {
                let (basis, mcp) = self.service_batch_basis(reference, task, false).await?;
                if association.candidate().authority_basis != basis { return Err(PrivateOriginalError::Conflict); }
                mcp
            }
        };
        // Reopen the exact source and its complete canonical cancellation chain
        // under the existing writer. Pending local cancellation is checked again
        // synchronously inside native's callback, not inferred from this read.
        let mut sources = vec![];
        let mut next = Some(reference.clone());
        while let Some(current) = next {
            let key = self.key(&current)?;
            if sources.len() >= 64 || sources.iter().any(|source: &OriginalCancellationSource| source.record.key == key) {
                return Err(PrivateOriginalError::Conflict);
            }
            let row = self.store.get(&key).await.map_err(store_error)?.ok_or(PrivateOriginalError::Unavailable)?;
            let value = self.decode_record(&row, &current)?;
            if current == *reference && value != record { return Err(PrivateOriginalError::Conflict); }
            if value.operation_cancelled { return Err(PrivateOriginalError::Denied); }
            next = value.approved_task.as_ref().map(|link| link.parent.clone());
            sources.push(OriginalCancellationSource {
                realm: value.source.realm.clone(),
                original: meerkat_authorization_contracts::work_association::OriginalWorkRef {
                    authority: value.source.source_authority, work: value.event.original,
                }, record: Arc::new(row), association: value.final_association,
            });
        }
        stamp.check_current().map_err(|_| PrivateOriginalError::Unavailable)?;
        // Do not hold the global source writer across an actor/B wait. Existing
        // source cancellations must remain able to enter their own publication.
        drop(writer);
        let pins = self.controller_pin_source.get().and_then(Weak::upgrade)
            .ok_or(PrivateOriginalError::Unavailable)?;
        let machine = self.machine()?;
        let session = &task.original_task.origin_session_id;
        let registration = machine.current_session_registration_witness(session).await
            .ok_or(PrivateOriginalError::Unavailable)?;
        let attachment = machine.current_executor_attachment_witness(session).await
            .ok_or(PrivateOriginalError::Unavailable)?;
        let actor = pins.live_actor(session).await.ok_or(PrivateOriginalError::Unavailable)?;
        let claims = association.candidate();
        let custody = AuthenticatedOriginal {
            original_work: claims.original_work.clone(), occurrence_scope: claims.ingress_namespace.occurrence_scope.clone(),
            original_authentication: claims.original_authentication.clone(), root_event: claims.root_event.clone(),
            contributing_work: claims.contributing_work.clone(), target: claims.target.clone(),
            authority_basis: claims.authority_basis.clone(),
        };
        // No caller-acquired B or pin_actor here. Core acquires the actual
        // attachment owner's B, pins under it, then holds M/D through callback.
        let observed = machine.with_admitted_original(&registration, &attachment,
            &actor.executor_actor_identity(), &input, |admitted| {
                let _writer = self.owner.try_source_writer().map_err(writer_error)?;
                if let Some(login) = &login {
                    login.try_check().map_err(|reason| match reason {
                        crate::private_host::ConsoleLoginRefusal::Unavailable => PrivateOriginalError::Unavailable,
                        _ => PrivateOriginalError::Denied,
                    })?;
                }
                // Check before our own publication reservation invalidates the
                // old stamp. Keep the source writer through promotion; the final
                // originals check also covers settlement/release outside it.
                stamp.check_current().map_err(|_| PrivateOriginalError::Unavailable)?;
                let retained = self.owner.promote_historical_task(association, custody, &sources, admitted)
                    .map_err(writer_error)?;
                Ok::<_, PrivateOriginalError>((retained, HistoricalServiceAdmission {
                    admission: ServiceInputAdmission { session_id: admitted.session_id().to_string(),
                        input_id: admitted.input_id().to_string() }, phase: admitted.phase(),
                }))
            }).await;
        let (retained, admission) = match observed {
            Ok(result) => result?,
            Err(AdmittedOriginalRefusal::NotAdmitted) => return Ok(None),
            Err(AdmittedOriginalRefusal::Conflict) => return Err(PrivateOriginalError::Conflict),
            Err(AdmittedOriginalRefusal::Unavailable) => return Err(PrivateOriginalError::Unavailable),
        };
        // The callback has released native locks. This existing holder retains
        // the same original and exact MCP decision; it is no finish receipt.
        self.hold_binding_with_mcp(reference.clone(), retained, mcp).await?;
        Ok(Some(admission))
    }
}
