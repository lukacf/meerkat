//! Reopen the exact retained human or application decision, then ask the
//! installed package owner to verify its bound software configuration.
use super::*;
use std::sync::Arc;
use crate::private_originals::service_approved_input::attachment::attachment_preparation_diagnostic;

impl PrivateOriginals {
    pub(crate) fn bind_service_installation_readback(
        &self, owner: Arc<dyn ServiceInstallationReadback>,
    ) -> Result<(), PrivateOriginalError> {
        self.installation_readback.set(owner).map_err(|_| PrivateOriginalError::Conflict)
    }

    async fn installation_readback_source(&self, reference: &PrivateOriginalRef,
        record: &SourceRecord, access: &AccessRecord,
    ) -> Result<ServiceInstallationReadbackSource, PrivateOriginalError> {
        let lifetime = self.host_lifetime.get().and_then(std::sync::Weak::upgrade)
            .ok_or(PrivateOriginalError::Unavailable)?;
        let operator = self.service_operator_setup(lifetime.clone())
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        let Some(application) = &access.application_policy else {
            return operator.retained_service_installation_review(&reference.route,
                &record.event.original_authentication, &access.parent).await
                .map_err(|_| PrivateOriginalError::Denied);
        };
        let task = record.approved_task.as_ref().ok_or(PrivateOriginalError::Denied)?;
        let AccessDecision::Mcp(decision) = &access.decision else { return Err(PrivateOriginalError::Denied); };
        let route = self.route(&reference.route)?;
        application.validate(route, decision, &self.scope.owner)?;
        application.check_parent(&lifetime, route, &access.parent)?;
        if record.event.original_authentication != decision.evidence
            || decision.evidence.revision.as_str() != "application-service-policy-v1"
            || application.original != task.parent || application.acquisition != task.acquisition
            || application.instruction_sha256 != record.event.content_sha256
            || application.source_policy_sha256 != lifetime.selected_policy().source_policy_sha256
        { return Err(PrivateOriginalError::Denied); }
        if application.service_parent.is_some() {
            operator.retained_standing_service_parent(&application.original, &application.acquisition,
                &application.instruction_sha256, decision, &access.parent).await
                .map_err(|_| PrivateOriginalError::Denied)?;
        }
        let selected = application.installation_readback.clone().ok_or(PrivateOriginalError::Denied)?;
        selected.validate(decision)?;
        Ok(ServiceInstallationReadbackSource::ApplicationPolicy(ApplicationInstallationReadback { selected }))
    }

    pub(crate) async fn read_service_installation(
        self: &Arc<Self>, reference: &PrivateOriginalRef,
    ) -> Result<McpServerConfig, PrivateOriginalError> {
        let stored = self.store.get(&self.key(reference)?).await.map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        let record = self.decode_record(&stored, reference)?;
        let grant = record.approved_task.as_ref().ok_or(PrivateOriginalError::Denied)?.singleton_grant()?.clone();
        self.read_service_installation_member(reference, &grant).await
    }

    pub(crate) async fn read_service_installation_member(
        self: &Arc<Self>, reference: &PrivateOriginalRef, grant: &PrivateServiceGrantRef,
    ) -> Result<McpServerConfig, PrivateOriginalError> {
        let stored = self.store.get(&self.key(reference)?).await.map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        let record = self.decode_record(&stored, reference)?;
        let task = record.approved_task.as_ref().ok_or(PrivateOriginalError::Denied)?;
        task.attachment_member(grant)?;
        if record.final_association.is_some() || record.service_admission.is_some()
            || !self.service_task_submission_current(&record).await? {
            return Err(PrivateOriginalError::Denied);
        }
        let access_stored = self.store.get(&self.access_key(&grant)?).await.map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        let access = self.decode_access(&access_stored, &grant)?;
        let AccessDecision::Mcp(decision) = &access.decision else { return Err(PrivateOriginalError::Denied); };
        if decision.live_attachment.is_none() || access.mcp.is_none()
            || self.access_observation(grant.clone(), &access)?.state != PrivateServiceGrantState::GrantIssued {
            return Err(PrivateOriginalError::Denied);
        }
        let source = self.installation_readback_source(reference, &record, &access).await?;
        let owner = self.installation_readback.get().ok_or(PrivateOriginalError::Unavailable)?;
        attachment_preparation_diagnostic::<()>("installation_owner_readback", None);
        let server = owner.readback_source(&source).await;
        attachment_preparation_diagnostic("installation_owner_readback", Some(&server));
        let server = server?;
        // The verifier may wait on filesystem/process work. Reopen the exact
        // B and parent grant afterward; this is not the final native Apply check.
        attachment_preparation_diagnostic::<()>("installation_post_records", None);
        let current = self.store.get(&stored.key).await.map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        let current_record = self.decode_record(&current, reference)?;
        let current_access = self.store.get(&access_stored.key).await.map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        attachment_preparation_diagnostic("installation_post_records", Some(&Ok::<(), PrivateOriginalError>(())));
        if current_record.approved_task != record.approved_task
            || current_record.final_association.is_some() || current_record.service_admission.is_some() {
            attachment_preparation_diagnostic("installation_record_changed", Some(&Err::<(), _>(PrivateOriginalError::Denied)));
            return Err(PrivateOriginalError::Denied);
        }
        let current_access = self.decode_access(&current_access, &grant);
        attachment_preparation_diagnostic("installation_access_decode", Some(&current_access));
        if current_access? != access {
            attachment_preparation_diagnostic("installation_access_changed", Some(&Err::<(), _>(PrivateOriginalError::Denied)));
            return Err(PrivateOriginalError::Denied);
        }
        attachment_preparation_diagnostic::<()>("installation_currentness", None);
        let current = self.service_task_submission_current(&current_record).await;
        attachment_preparation_diagnostic("installation_currentness", Some(&current));
        if !current? {
            attachment_preparation_diagnostic("installation_not_current", Some(&Err::<(), _>(PrivateOriginalError::Denied)));
            return Err(PrivateOriginalError::Denied);
        }
        let observed = self.access_observation(grant.clone(), &access);
        attachment_preparation_diagnostic("installation_grant_observation", Some(&observed));
        if observed?.state != PrivateServiceGrantState::GrantIssued {
            attachment_preparation_diagnostic("installation_grant_not_issued", Some(&Err::<(), _>(PrivateOriginalError::Denied)));
            return Err(PrivateOriginalError::Denied);
        }
        if access.application_policy.is_some() {
            // The package verifier awaited real I/O. Reopen the same selected
            // application/administrative decision after that wait as well.
            self.installation_readback_source(reference, &current_record, &access).await?;
        }
        let bytes = canonical_json(&server).map_err(|_| PrivateOriginalError::Conflict)?;
        if digest(&bytes) != decision.connection_revision.as_str() {
            attachment_preparation_diagnostic("installation_connection_changed", Some(&Err::<(), _>(PrivateOriginalError::Denied)));
            return Err(PrivateOriginalError::Denied);
        }
        attachment_preparation_diagnostic("installation_connection_digest", Some(&Ok::<(), PrivateOriginalError>(())));
        Ok(server)
    }
}
