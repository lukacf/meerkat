//! Canonical preparation joined to the existing native publication. The
//! retained data is immutable evidence; its old stamp is never refreshed.

use super::*;
use crate::private_originals::service_approved_input::attachment::attachment_preparation_diagnostic;
use meerkat_authorization::{policy::LocalOperationValues, publication::LocalPublicationStamp};
use meerkat_core::authorization::{
    AuthorizationOperation, SourceAuthorizationTarget, SourceAuthorizationUse,
};
use meerkat_core::exact_operation::OperationExecutionScope;
use meerkat_core::mcp_attachment_control::McpAttachmentFacts;
use meerkat_runtime::meerkat_machine::{AuthenticatedOriginalSource, OriginalDisposition};
use std::sync::{Arc, Weak};
use toolkit_store::{RecordChangePublication, SqliteStore};

struct ServiceMcpControl {
    source: Weak<PrivateOriginals>,
    original: OriginalWorkRef,
    parent: InputAuthorityAssociation,
    grant: PrivateServiceGrantRef,
    access: AccessRecord,
    before_ms: u64,
    publication: LocalPublicationStamp,
}

pub(crate) struct CurrentServiceMcpControl {
    pub connection: Id,
    pub resource: Id,
    pub configuration_revision: Revision,
    pub operations: BTreeSet<Id>,
    pub parent: InputAuthorityAssociation,
    pub values: LocalOperationValues,
    pub restrictions: ExecutionRestrictions,
    pub expires_at_ms: u64,
}

impl PrivateOriginals {
    /// Prepare an exact live-only control from approved B and its canonical
    /// task, source and grant owners. The caller must still use native actor
    /// registration and B/R custody; this method attaches or persists nothing.
    pub(crate) async fn prepare_service_mcp_control(
        self: &Arc<Self>,
        store: &Arc<SqliteStore>,
        reference: &PrivateOriginalRef,
        server: McpServerConfig,
    ) -> Result<Arc<McpAttachmentControlRequest>, PrivateOriginalError> {
        let publication = self.capture_service_mcp_publication(store)?;
        self.prepare_service_mcp_control_at(reference, server, publication).await
    }

    /// Production recovery captures currentness before package readback as well
    /// as canonical task reads. A changed owner never refreshes this old stamp.
    pub(crate) async fn prepare_service_mcp_installation(
        self: &Arc<Self>, store: &Arc<SqliteStore>, reference: &PrivateOriginalRef,
    ) -> Result<Arc<McpAttachmentControlRequest>, PrivateOriginalError> {
        let stored = self.store.get(&self.key(reference)?).await.map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        let record = self.decode_record(&stored, reference)?;
        let grant = record.approved_task.as_ref().ok_or(PrivateOriginalError::Denied)?.singleton_grant()?.clone();
        self.prepare_service_mcp_installation_member(store, reference, &grant).await
    }

    pub(crate) async fn prepare_service_mcp_installation_member(
        self: &Arc<Self>, store: &Arc<SqliteStore>, reference: &PrivateOriginalRef, grant: &PrivateServiceGrantRef,
    ) -> Result<Arc<McpAttachmentControlRequest>, PrivateOriginalError> {
        attachment_preparation_diagnostic::<()>("capture_publication", None);
        let publication = self.capture_service_mcp_publication(store);
        attachment_preparation_diagnostic("capture_publication", Some(&publication));
        let publication = publication?;
        attachment_preparation_diagnostic::<()>("installation_readback", None);
        let server = self.read_service_installation_member(reference, grant).await;
        attachment_preparation_diagnostic("installation_readback", Some(&server));
        let server = server?;
        attachment_preparation_diagnostic::<()>("canonical_control", None);
        let control = self.prepare_service_mcp_control_at(reference, server, publication).await;
        attachment_preparation_diagnostic("canonical_control", Some(&control));
        control
    }

    fn capture_service_mcp_publication(
        self: &Arc<Self>, store: &Arc<SqliteStore>,
    ) -> Result<LocalPublicationStamp, PrivateOriginalError> {
        let source_store: Arc<dyn ToolkitStore> = store.clone();
        let publication_owner: Arc<dyn RecordChangePublication> = self.owner.clone();
        if !Arc::ptr_eq(&self.store, &source_store)
            || !store.record_publication_is(&publication_owner)
        {
            return Err(PrivateOriginalError::Unavailable);
        }
        // Capture before every canonical await. The bound SQLite owner keeps
        // publication in progress through commit/rollback. A later preparation
        // may read new facts; this exact control may never acquire a new stamp.
        let (_, publication) = self
            .owner
            .publication()
            .observe(|| ())
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        Ok(publication)
    }

    async fn prepare_service_mcp_control_at(
        self: &Arc<Self>, reference: &PrivateOriginalRef, server: McpServerConfig,
        publication: LocalPublicationStamp,
    ) -> Result<Arc<McpAttachmentControlRequest>, PrivateOriginalError> {
        attachment_preparation_diagnostic::<()>("canonical_records", None);
        let stored = self
            .store
            .get(&self.key(reference)?)
            .await
            .map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        let record = self.decode_record(&stored, reference)?;
        let task = record
            .approved_task
            .as_ref()
            .ok_or(PrivateOriginalError::Denied)?;
        let grant = task.attachment_grant(&server.name)?;
        if task.parent.route != reference.route
            || grant.route != reference.route
            || record.operation_cancelled
            || record.final_association.is_some()
            || record.service_admission.is_some()
        {
            return Err(PrivateOriginalError::Denied);
        }
        let parent_stored = self
            .store
            .get(&self.key(&task.parent)?)
            .await
            .map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        let parent_record = self.decode_record(&parent_stored, &task.parent)?;
        let parent = parent_record
            .final_association
            .ok_or_else(|| {
                attachment_preparation_diagnostic::<()>(
                    "canonical_parent_association", Some(&Err(PrivateOriginalError::Unavailable)),
                );
                PrivateOriginalError::Unavailable
            })?;
        let access_stored = self
            .store
            .get(&self.access_key(&grant)?)
            .await
            .map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        let access = self.decode_access(&access_stored, &grant)?;
        let route = self.route(&reference.route)?;
        let control = McpAttachmentControlRequest::from_trusted_ingress(
            task.original_task.origin_session_id.clone(),
            server,
            McpAttachmentFacts {
                requester: route.requester.clone(),
                actor: route.ingress_actor.clone(),
                realm: route.realm.clone(),
                destination: meerkat_runtime::meerkat_machine::scoped_destination_target(
                    &parent.candidate().target,
                ),
            },
            Arc::new(ServiceMcpControl {
                source: Arc::downgrade(self),
                original: OriginalWorkRef {
                    authority: route.source_authority.clone(),
                    work: reference.original.clone(),
                },
                parent,
                grant: grant.clone(),
                access,
                before_ms: task.before_ms,
                publication,
            }),
        )
        .map_err(|_| PrivateOriginalError::Denied)?;
        attachment_preparation_diagnostic("canonical_records", Some(&Ok(())));
        let binding = control.effect_binding(McpAttachmentEffect::Apply);
        // Reuse the existing canonical task decoder and A-completed checks,
        // including cancellation ancestors and exact reviewed descriptor.
        let observation = self.observe_service_mcp_attachment(reference, &control, &binding)
            .await;
        attachment_preparation_diagnostic("canonical_task_observation", Some(&observation));
        observation?;
        let clock = self
            .owner
            .clock()
            .now()
            .map_err(|_| PrivateOriginalError::Unavailable);
        attachment_preparation_diagnostic("canonical_clock", Some(&clock));
        let now = clock?.unix_ms;
        let current = self.current_service_mcp_control(&control, &binding, now, false);
        attachment_preparation_diagnostic("canonical_current_control", Some(&current));
        current?;
        Ok(control)
    }

    pub(in crate::private_originals) fn service_control_grant<'a>(
        &self, control: &'a McpAttachmentControlRequest,
    ) -> Result<&'a PrivateServiceGrantRef, PrivateOriginalError> {
        let held = control.evidence::<ServiceMcpControl>().ok_or(PrivateOriginalError::Denied)?;
        let source = held.source.upgrade().ok_or(PrivateOriginalError::Unavailable)?;
        if !std::ptr::eq(self, source.as_ref()) { return Err(PrivateOriginalError::Denied); }
        Ok(&held.grant)
    }

    /// Synchronous native policy callback. No SQLite, network, filesystem or
    /// async owner read occurs here. Native independently checks actor identity,
    /// connector credentials and physical entry using this same exact control.
    pub(crate) fn current_service_mcp_control(
        &self,
        control: &McpAttachmentControlRequest,
        binding: &PreparedAuthorizationBinding,
        now_ms: u64,
        source_operation: bool,
    ) -> Result<CurrentServiceMcpControl, PrivateOriginalError> {
        let held = control
            .evidence::<ServiceMcpControl>()
            .ok_or(PrivateOriginalError::Denied)?;
        let source = held
            .source
            .upgrade()
            .ok_or(PrivateOriginalError::Unavailable)?;
        if !std::ptr::eq(self, source.as_ref()) || now_ms >= held.before_ms {
            return Err(PrivateOriginalError::Denied);
        }
        held.publication
            .check_current()
            .map_err(|_| {
                attachment_preparation_diagnostic::<()>(
                    "control_publication_before", Some(&Err(PrivateOriginalError::Unavailable)),
                );
                PrivateOriginalError::Unavailable
            })?;
        let apply = control.effect_binding(McpAttachmentEffect::Apply);
        let facts = binding.facts();
        let server = control.server_target();
        let hydrate = source_operation
            && facts.operation_id == *control.operation_id()
            && facts.execution_scope == OperationExecutionScope::Domain
            && facts.run_id.is_none()
            && facts.context_revision.is_none()
            && matches!(&facts.operation, AuthorizationOperation::Source(source)
                if source.usage == SourceAuthorizationUse::Hydrate
                && matches!(&source.target, SourceAuthorizationTarget::External(target)
                    if target.authority == server.authority && target.namespace == server.namespace
                    && target.id == server.id));
        if control.effect_of(binding) != Some(McpAttachmentEffect::Apply) && !hydrate {
            return Err(PrivateOriginalError::Denied);
        }
        let route = self.route(&held.grant.route)?;
        let (AccessDecision::Mcp(decision), Some(selection)) =
            (&held.access.decision, &held.access.mcp)
        else {
            return Err(PrivateOriginalError::Denied);
        };
        if held.original.authority != route.source_authority
            || !matches!(&decision.permission.lifetime.scope,
                LifetimeScope::Task { task } if task.as_str() == held.original.work.as_str())
            || !matches!(
                self.owner.original_disposition(
                    &held.parent.candidate().ingress_namespace,
                    &held.parent.candidate().original_work,
                ),
                Ok(OriginalDisposition::Active)
            )
        {
            return Err(PrivateOriginalError::Denied);
        }
        selection.validate_live_attachment(
            route,
            decision,
            &held.parent.candidate().target,
            control,
            &apply,
            now_ms,
        )?;
        if self
            .access_observation(held.grant.clone(), &held.access)?
            .state
            != PrivateServiceGrantState::GrantIssued
        {
            return Err(PrivateOriginalError::Denied);
        }
        let IssuanceObservation::Issued { lineage } = &held.access.issuance else {
            attachment_preparation_diagnostic::<()>(
                "control_issued_lineage", Some(&Err(PrivateOriginalError::Unavailable)),
            );
            return Err(PrivateOriginalError::Unavailable);
        };
        let resolved = self
            .owner
            .grants()
            .resolve_lineage(
                lineage,
                &route.logical_executor,
                route.represented_subject.as_ref(),
            )
            .map_err(|_| PrivateOriginalError::Denied)?;
        resolved
            .check_current()
            .map_err(|_| {
                attachment_preparation_diagnostic::<()>(
                    "control_lineage_currentness", Some(&Err(PrivateOriginalError::Unavailable)),
                );
                PrivateOriginalError::Unavailable
            })?;
        let values = selection
            .attachment_operation(route, decision)?
            .ok_or(PrivateOriginalError::Denied)?;
        let expires_at_ms = held.before_ms.min(resolved.expires_at_ms()).min(
            decision
                .permission
                .lifetime
                .expires_at_ms
                .ok_or(PrivateOriginalError::Denied)?,
        );
        if now_ms >= expires_at_ms {
            return Err(PrivateOriginalError::Denied);
        }
        held.publication
            .check_current()
            .map_err(|_| {
                attachment_preparation_diagnostic::<()>(
                    "control_publication_after", Some(&Err(PrivateOriginalError::Unavailable)),
                );
                PrivateOriginalError::Unavailable
            })?;
        Ok(CurrentServiceMcpControl {
            connection: decision.permission.connection.clone(),
            resource: decision.permission.resource.clone(),
            configuration_revision: decision.connection_revision.clone(),
            operations: decision.permission.operations.clone(),
            parent: held.parent.clone(),
            values,
            restrictions: resolved.restrictions().clone(),
            expires_at_ms,
        })
    }
}
