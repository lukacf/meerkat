//! Actor-free historical metadata on the existing canonical source page.
//! The host explicitly selects the real private reader. No local transfer,
//! source custody, native input, negative witness or retry owner is created.
use super::*;
use crate::installed_host::acquisition::context_recovery::{
    InstalledWorkflowContextRead, InstalledWorkflowContextSelector, WorkflowContextRecovery,
};
use crate::private_originals::planning_source_evidence::PlanningSourceEvidence;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod worker_stdio;
#[cfg(test)]
tokio::task_local! {
    static AFTER_PRIVATE_READ: Arc<(tokio::sync::Notify, tokio::sync::Notify)>;
}

impl PrivateOriginals {
    pub(crate) async fn recover_workflow_context_source(
        &self,
        observed: &StoredRecord,
        backend: &SetupWorkBackend,
        selector: &InstalledWorkflowContextSelector,
        enter: impl Fn() -> bool + Send + Sync,
    ) -> Result<WorkflowContextRecovery, PrivateOriginalError> {
        if !enter() {
            return Ok(WorkflowContextRecovery::Stopped);
        }
        if observed.key.scope() != self.scope {
            return Err(PrivateOriginalError::Denied);
        }
        self.service_input_host_current()?;
        if observed.data.schema != SOURCE_SCHEMA {
            return Ok(WorkflowContextRecovery::NotSelected);
        }
        let row = self
            .store
            .get(&observed.key)
            .await
            .map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        if !enter() {
            return Ok(WorkflowContextRecovery::Stopped);
        }
        let record = self.decode_stored(&row)?;
        let Some(
            PlanningSourceEvidence::Waiter { workflow, planning }
            | PlanningSourceEvidence::EffectWaiter { workflow, planning },
        ) = &record.planning_source_evidence
        else {
            return Ok(WorkflowContextRecovery::NotSelected);
        };
        let original = PrivateOriginalRef {
            route: record.event.route.clone(),
            original: record.event.original.clone(),
            event_sha256: digest(
                &canonical_json(&record.event).map_err(|_| PrivateOriginalError::Conflict)?,
            ),
        };
        self.decode_record(&row, &original)?;
        self.check_planning_evidence_source(&record, &original)?;
        let association = record
            .final_association
            .as_ref()
            .ok_or(PrivateOriginalError::Unavailable)?;
        let canonical = self
            .workflow_store()?
            .load_setup_workflow(workflow)
            .await
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        if !enter() {
            return Ok(WorkflowContextRecovery::Stopped);
        }
        if canonical.terms().planning_source != self.key(planning)?
            || canonical.terms().backend != *backend.pin()
        {
            return Err(PrivateOriginalError::Conflict);
        }
        let memberships = canonical.waiters_for_source(&row.key);
        if memberships.is_empty() {
            return Ok(WorkflowContextRecovery::NotSelected);
        }
        // Compiled source ownership and the exact canonical G are checked
        // before the trusted callback sees terms or supplies private access.
        let g_row = self
            .store
            .get(&self.key(planning)?)
            .await
            .map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        if !enter() {
            return Ok(WorkflowContextRecovery::Stopped);
        }
        let g = self.decode_record(&g_row, planning)?;
        self.check_planning_evidence_source(&g, planning)?;
        let Some(PlanningSourceEvidence::Lead { parent }) = &g.planning_source_evidence else {
            return Err(PrivateOriginalError::Unavailable);
        };
        let parent_row = self
            .store
            .get(&self.key(parent)?)
            .await
            .map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        if !enter() {
            return Ok(WorkflowContextRecovery::Stopped);
        }
        let parent_record = self.decode_record(&parent_row, parent)?;
        self.check_planning_evidence_source(&parent_record, parent)?;
        if !crate::private_originals::planning_source_evidence::is_parent(
            parent_record.planning_source_evidence.as_ref(),
        ) {
            return Err(PrivateOriginalError::Conflict);
        }
        let parent_association = parent_record
            .final_association
            .as_ref()
            .ok_or(PrivateOriginalError::Unavailable)?;
        let g_association = g
            .final_association
            .as_ref()
            .ok_or(PrivateOriginalError::Unavailable)?;
        if g_association.candidate().contributing_work.as_slice()
            != [parent_association.candidate().original_work.clone()]
        {
            return Err(PrivateOriginalError::Conflict);
        }
        let lifetime = self
            .host_lifetime
            .get()
            .and_then(Weak::upgrade)
            .ok_or(PrivateOriginalError::Unavailable)?;
        lifetime
            .validate_current_selection()
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        let selected_host = lifetime.selected_policy();
        let native = selected_host
            .native_host
            .as_ref()
            .ok_or(PrivateOriginalError::Unavailable)?;
        let route = native
            .routes
            .iter()
            .find(|route| route.id == planning.route)
            .ok_or(PrivateOriginalError::Denied)?;
        let (member, _, _) = route
            .source_binding(selected_host)
            .map_err(|_| PrivateOriginalError::Denied)?;
        if member != &canonical.terms().lead || native.realm != canonical.terms().realm {
            return Err(PrivateOriginalError::Conflict);
        }
        let selection =
            selector(workflow, canonical.terms(), backend).map_err(|error| match error {
                HostLifetimeError::Unavailable => PrivateOriginalError::Unavailable,
                _ => PrivateOriginalError::Denied,
            })?;
        let Some(selection) = selection else {
            return Ok(WorkflowContextRecovery::NotSelected);
        };
        validate_reader(self, workflow, canonical.terms(), backend, &selection)?;
        let mut admitted = 0;
        let mut pending = 0;
        for membership in &memberships {
            if !enter() {
                return Ok(WorkflowContextRecovery::Stopped);
            }
            self.service_input_host_current()?;
            let Some((envelope, admission)) = read_committed(
                self,
                &original,
                association,
                planning,
                parent_association,
                membership,
                canonical.terms(),
                backend,
                &selection,
            )
            .await?
            else {
                pending += 1;
                continue;
            };
            #[cfg(test)]
            if let Some(probe) = self.metadata_probe.get() {
                probe.after_read(&row.key).await;
            }
            #[cfg(test)]
            if let Ok(pause) = AFTER_PRIVATE_READ.try_with(Clone::clone) {
                pause.0.notify_one();
                pause.1.notified().await;
            }
            if !enter() {
                return Ok(WorkflowContextRecovery::Stopped);
            }
            // The joined worker owns the complete accepted-only query/CAS.
            // Closure after this boundary cannot recall an actual admission.
            match delivery::historical::acknowledge_envelope(self, &envelope, &admission).await? {
                WorkflowDeliveryObservation::Admitted { .. } => admitted += 1,
                WorkflowDeliveryObservation::Pending => pending += 1,
                WorkflowDeliveryObservation::NotApplied => {
                    return Err(PrivateOriginalError::Conflict);
                }
            }
        }
        Ok(WorkflowContextRecovery::Observed { admitted, pending })
    }
}

fn validate_reader(
    source: &PrivateOriginals,
    workflow: &SetupWorkflowRef,
    terms: &meerkat_toolkit::setup::SetupWorkflowTerms,
    backend: &SetupWorkBackend,
    selected: &InstalledWorkflowContextRead,
) -> Result<(), PrivateOriginalError> {
    source.service_input_host_current()?;
    let recipient = &selected.recipient;
    let grant = recipient.namespace_grant();
    if selected.output_audiences.is_empty()
        || backend.pin() != &terms.backend
        || !Arc::ptr_eq(backend.service().store(), recipient.store())
        || recipient.store().kind() != WorkGraphStoreKind::Sqlite
        || grant.realm_id != terms.realm.to_string()
        || grant.realm_id != recipient.default_realm_id()
        || grant.namespace != recipient.default_namespace().as_str()
        || recipient.default_namespace() != &PrivateOriginals::workflow_context_namespace(workflow)?
        || recipient.default_namespace() == backend.service().default_namespace()
    {
        return Err(PrivateOriginalError::Denied);
    }
    Ok(())
}

async fn read_committed(
    source: &PrivateOriginals,
    original: &PrivateOriginalRef,
    association: &InputAuthorityAssociation,
    planning: &PrivateOriginalRef,
    parent: &InputAuthorityAssociation,
    membership: &SetupWorkflowWaiter,
    terms: &meerkat_toolkit::setup::SetupWorkflowTerms,
    backend: &SetupWorkBackend,
    selected: &InstalledWorkflowContextRead,
) -> Result<Option<(TransferEnvelope, WorkAdmission)>, PrivateOriginalError> {
    let recipient = &selected.recipient;
    let key = admission_key(membership)?;
    let observed = recipient
        .store()
        .observe_item_admission(
            recipient.default_realm_id(),
            recipient.default_namespace(),
            &key,
        )
        .await
        .map_err(|_| PrivateOriginalError::Unavailable)?;
    source.service_input_host_current()?;
    let Some((item, _)) = observed else {
        return Ok(None);
    };
    let envelope: TransferEnvelope = serde_json::from_str(
        item.description
            .as_deref()
            .ok_or(PrivateOriginalError::Conflict)?,
    )
    .map_err(|_| PrivateOriginalError::Conflict)?;
    let saved = &envelope.recipient;
    let audience = &source.route(&planning.route)?.grant_audience;
    if envelope.purpose != PURPOSE
        || &envelope.membership != membership
        || &saved.original != original
        || &saved.association != association
        || &saved.planning_source != planning
        || &saved.parent != parent
        || &saved.workflow != membership.workflow()
        || saved.lead != terms.lead
        || saved.backend != *backend.pin()
        || saved.recipient_realm != recipient.default_realm_id()
        || &saved.recipient_namespace != recipient.default_namespace()
        || &saved.native_audience != audience
        || saved.output_audiences != selected.output_audiences
        || &saved.association.candidate().target.audience != audience
        || membership.source() != &source.key(original)?
        || envelope.find_call.is_empty()
        || envelope
            .request
            .user_context
            .iter()
            .any(|item| matches!(item, RequestContext::Reference(_)))
    {
        return Err(PrivateOriginalError::Conflict);
    }
    let correlation = crate::installed_host::acquisition::installed_invocation_correlation(
        original,
        &envelope.find_scope,
        &envelope.find_call,
    )
    .map_err(|_| PrivateOriginalError::Conflict)?;
    if correlation != saved.correlation {
        return Err(PrivateOriginalError::Conflict);
    }
    let checked = source
        .workflow_store()?
        .validate_committed_workflow_context(
            membership,
            association,
            &correlation,
            &envelope.input_digest,
            &envelope.request,
            envelope.deadline_ms,
            &selected.output_audiences,
        )
        .await
        .map_err(|error| match error {
            meerkat_toolkit::setup::SetupError::Denied => PrivateOriginalError::Denied,
            meerkat_toolkit::setup::SetupError::Store(error) => store_error(error),
            meerkat_toolkit::setup::SetupError::NativeExecutionUnavailable
            | meerkat_toolkit::setup::SetupError::WorkGraph(_) => PrivateOriginalError::Unavailable,
            _ => PrivateOriginalError::Conflict,
        })?;
    if checked.terms() != terms {
        return Err(PrivateOriginalError::Conflict);
    }
    let request = creation_request(&envelope)?;
    let admission = recipient
        .admission_identity(key.clone(), request.clone())
        .map_err(|_| PrivateOriginalError::Conflict)?;
    validate_item(recipient, &admission, &request, &item)?;
    match recipient
        .observe_admission(key, request.clone())
        .await
        .map_err(|_| PrivateOriginalError::Unavailable)?
    {
        Some(WorkAdmissionOutcome::Replayed(exact)) => {
            validate_item(recipient, &admission, &request, &exact)?
        }
        None => return Ok(None),
        Some(_) => return Err(PrivateOriginalError::Conflict),
    }
    source.service_input_host_current()?;
    Ok(Some((envelope, admission)))
}
