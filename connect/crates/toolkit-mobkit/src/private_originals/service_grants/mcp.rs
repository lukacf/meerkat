//! Native connector correspondence retained in the existing access decision.
//! The native grant remains permission; these immutable facts select its use.

use super::*;
use crate::installed_host::application_operations;
use meerkat_authorization_contracts::work_association::{InputAuthorityAssociation, NativeWorkTarget};
use meerkat_core::mcp_attachment_control::{McpAttachmentControlRequest, McpAttachmentEffect};
use meerkat_core::authorization::PreparedAuthorizationBinding;
use meerkat_core::mcp_config::{McpConnectorApproval, McpConnectorBinding, McpServerConfig};
use meerkat_toolkit::authority::RoleLifetime;
use std::collections::BTreeSet;

mod control;
mod installation;

/// Proposal data for the complete operation set. It supplies no principal,
/// native grant, lineage or preconstructed restrictions.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrivateServiceMcpPermission {
    pub target: Id,
    pub connection: Id,
    pub operations: BTreeSet<Id>,
    pub resource: Id,
    pub lifetime: RoleLifetime,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrivateServiceMcpAccessDecision {
    pub route: Id,
    pub decision: Id,
    pub evidence: PrivateSourceEvidence,
    pub choice: PrivateAccessChoice,
    pub permission: PrivateServiceMcpPermission,
    pub connection_revision: Revision,
    /// Explicit live-only installation effect in the same protected decision.
    /// Absent in older decisions and account/tool-only approvals. It never
    /// authorizes persistence or selects a setup helper as the destination.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live_attachment: Option<PrivateServiceMcpLiveAttachment>,
    /// Metadata visibility for eligible tools in the entire original session.
    /// Derive the session from the same live attachment, never from the helper.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog: Option<PrivateServiceMcpCatalog>,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrivateServiceMcpCatalog { OriginalSession }

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrivateServiceMcpLiveAttachment {
    pub destination: NativeWorkTarget,
}

/// Trusted application selection supplied beside its access proposal. This is
/// not approval, a live account observation or a permit. No wire decoder exists.
/// Software configuration and access ceilings retain their existing owners.
#[derive(Clone, PartialEq, Eq)]
pub struct ServiceMcpSelection(pub(super) StoredServiceMcpSelection);
impl ServiceMcpSelection {
    pub fn connector(installation: &meerkat::ConnectorMcpInstallation) -> Self {
        let config = installation.config();
        Self(StoredServiceMcpSelection {
            server: config.name.clone(),
            connector: config.connector.clone(),
        })
    }
    /// An explicit host selection. Missing old evidence never means NoConnector.
    pub fn no_connector(config: &McpServerConfig) -> Result<Self, PrivateOriginalError> {
        if config.connector.is_some() || config.name.trim().is_empty() {
            return Err(PrivateOriginalError::Denied);
        }
        Ok(Self(StoredServiceMcpSelection {
            server: config.name.clone(),
            connector: None,
        }))
    }
    pub(crate) fn reviewed_content(&self) -> Result<serde_json::Value, PrivateOriginalError> {
        serde_json::to_value(&self.0).map_err(|_| PrivateOriginalError::Denied)
    }
    pub(crate) fn restrictions(
        &self,
        route: &PrivateOriginalRoute,
        decision: &PrivateServiceMcpAccessDecision,
        now_ms: u64,
    ) -> Result<ExecutionRestrictions, PrivateOriginalError> {
        self.0.restrictions(route, decision, now_ms)
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StoredServiceMcpSelection {
    pub(super) server: String,
    // Explicit null is native NoConnector. An absent whole selection is legacy.
    pub(super) connector: Option<McpConnectorBinding>,
}
impl StoredServiceMcpSelection {
    pub(super) fn validate(
        &self,
        route: &PrivateOriginalRoute,
        decision: &PrivateServiceMcpAccessDecision,
    ) -> Result<(), PrivateOriginalError> {
        if self.server.trim().is_empty()
            || decision.route != route.id
            || decision.permission.target != route.grant_target
            || decision.permission.operations.is_empty()
            || !valid_digest(&decision.evidence.sha256)
            || matches!(&decision.permission.lifetime.scope, LifetimeScope::Member { member } if member != &route.grant_target)
            || self.connector.as_ref().is_some_and(|binding| {
                binding.realm_id != route.realm.as_str()
                    || decision
                        .permission
                        .operations
                        .iter()
                        .any(|operation| !binding.operations.contains_key(operation.as_str()))
            })
        {
            return Err(PrivateOriginalError::Denied);
        }
        if decision.catalog.is_some() && decision.live_attachment.is_none() {
            return Err(PrivateOriginalError::Denied);
        }
        if let Some(effect) = &decision.live_attachment {
            let destination = &effect.destination;
            let session = meerkat_core::SessionId::parse(destination.context.as_str())
                .map_err(|_| PrivateOriginalError::Denied)?;
            if !matches!(&decision.permission.lifetime.scope, LifetimeScope::Task { .. })
                || decision.permission.lifetime.expires_at_ms.is_none()
                || !valid_digest(decision.connection_revision.as_str())
                || decision.permission.connection.as_str() != self.server
                || decision.permission.resource.as_str() != self.server
                || destination.logical_owner.validate_qualified().is_err()
                || destination.logical_runtime.as_str()
                    != meerkat_runtime::LogicalRuntimeId::for_session(&session).to_string()
                || destination.audience != route.grant_audience
            { return Err(PrivateOriginalError::Denied); }
        }
        Ok(())
    }
    fn approval(&self) -> McpConnectorApproval {
        McpConnectorApproval::of(self.connector.as_ref())
    }
    pub(super) fn restrictions(
        &self,
        route: &PrivateOriginalRoute,
        decision: &PrivateServiceMcpAccessDecision,
        now_ms: u64,
    ) -> Result<ExecutionRestrictions, PrivateOriginalError> {
        self.validate(route, decision)?;
        if !decision.permission.lifetime.active_at(now_ms) {
            return Err(PrivateOriginalError::Denied);
        }
        let mut native = application_operations::mcp_access_restrictions_with_service_scope(
            &route.grant_caller,
            &route.logical_executor,
            &route.grant_audience,
            &self.server,
            decision.permission.operations.iter().map(Id::as_str),
            &self.approval(),
            self.attachment_operation(route, decision)?,
            self.catalog_operation(route, decision)?,
        );
        native.lifetime = match decision.permission.lifetime.expires_at_ms {
            Some(expiry) => LifetimeRestriction::window(0, expiry),
            None => LifetimeRestriction::unrestricted(),
        };
        Ok(native)
    }
    fn catalog_operation(&self, route: &PrivateOriginalRoute, decision: &PrivateServiceMcpAccessDecision)
        -> Result<Option<meerkat_authorization::policy::LocalOperationValues>, PrivateOriginalError> {
        if decision.catalog.is_none() { return Ok(None); }
        let attachment = decision.live_attachment.as_ref().ok_or(PrivateOriginalError::Denied)?;
        let session = meerkat_core::SessionId::parse(attachment.destination.context.as_str())
            .map_err(|_| PrivateOriginalError::Denied)?;
        Ok(Some(application_operations::catalog_read(&route.grant_caller,
            &route.logical_executor, &route.grant_audience, &session)))
    }
    fn attachment_operation(
        &self,
        route: &PrivateOriginalRoute,
        decision: &PrivateServiceMcpAccessDecision,
    ) -> Result<Option<meerkat_authorization::policy::LocalOperationValues>, PrivateOriginalError> {
        let Some(effect) = &decision.live_attachment else { return Ok(None); };
        let exact = canonical_json(&(&self.server, &decision.connection_revision, &effect.destination))
            .map_err(|_| PrivateOriginalError::Conflict)?;
        Ok(Some(application_operations::mcp_live_attachment(
            &route.grant_caller, &route.logical_executor, &route.grant_audience, &digest(&exact),
        )))
    }

    /// Match the actual native control and effect against the explicit decision.
    /// This observation is not an entry permit; native must still join current
    /// source, setup, grant, registration and actor custody at effect entry.
    fn validate_live_attachment(
        &self,
        route: &PrivateOriginalRoute,
        decision: &PrivateServiceMcpAccessDecision,
        destination: &NativeWorkTarget,
        control: &McpAttachmentControlRequest,
        binding: &PreparedAuthorizationBinding,
        now_ms: u64,
    ) -> Result<(), PrivateOriginalError> {
        self.validate(route, decision)?;
        let effect = decision.live_attachment.as_ref().ok_or(PrivateOriginalError::Denied)?;
        let facts = control.facts();
        let canonical = meerkat_runtime::meerkat_machine::scoped_destination_target(destination);
        if control.effect_of(binding) != Some(McpAttachmentEffect::Apply)
            || decision.choice != PrivateAccessChoice::Approve
            || !decision.permission.lifetime.active_at(now_ms)
            || &effect.destination != destination
            || control.session_id().to_string() != destination.context.as_str()
            || facts.requester != route.requester || facts.actor != route.ingress_actor
            || facts.realm != route.realm
            || facts.destination.authority != canonical.authority
            || facts.destination.namespace != canonical.namespace
            || facts.destination.id != canonical.id
            || control.server().name != self.server
            || control.server().connector != self.connector
            || digest(&canonical_json(control.server()).map_err(|_| PrivateOriginalError::Conflict)?)
                != decision.connection_revision.as_str()
        { return Err(PrivateOriginalError::Denied); }
        Ok(())
    }

    /// Native classifies the entire retained lineage. Raw wildcard bounds
    /// cannot originate approval; removing it at any ancestor prevents reuse.
    pub(super) fn check_grant(
        &self,
        route: &PrivateOriginalRoute,
        decision: &PrivateServiceMcpAccessDecision,
        resolved: &meerkat_authorization::grants::ResolvedGrant,
        now_ms: u64,
    ) -> Result<(), GrantRefusal> {
        use meerkat_authorization::mcp_installation::{McpConnectorBound, lineage_connector_bound};
        use meerkat_authorization_contracts::constraints::OperationRestrictionValues;
        let connector = application_operations::mcp_connector_operation(
            &route.grant_caller,
            &route.logical_executor,
            &route.grant_audience,
            &self.server,
            &self.approval(),
        );
        if lineage_connector_bound(
            resolved.lineage_restrictions(),
            resolved.restrictions(),
            &connector.action,
            &connector.resource_domain,
        ) != McpConnectorBound::Approved
        {
            return Err(GrantRefusal::Denied);
        }
        let operations = decision.permission.operations.iter().map(|operation| {
            application_operations::mcp_operation(
                &route.grant_caller,
                &route.logical_executor,
                &route.grant_audience,
                &self.server,
                operation.as_str(),
            )
        });
        let attachment = self.attachment_operation(route, decision).map_err(|_| GrantRefusal::Denied)?;
        let catalog = self.catalog_operation(route, decision).map_err(|_| GrantRefusal::Denied)?;
        for values in operations.chain(std::iter::once(connector)).chain(attachment).chain(catalog) {
            resolved
                .restrictions()
                .check_bounds(OperationRestrictionValues {
                    action: &values.action,
                    resource_domain: &values.resource_domain,
                    processor: &values.processor,
                    audience: &values.audience,
                    now_ms,
                })
                .map_err(|_| GrantRefusal::Denied)?;
        }
        Ok(())
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct RetainedServiceMcpAccess {
    pub(super) decision: PrivateServiceMcpAccessDecision,
    pub(super) selection: StoredServiceMcpSelection,
}
impl PrivateOriginals {
    /// Read the existing immutable decision and resolve its actual native leaf.
    /// Neither a grant reference nor a successful read becomes reusable authority.
    pub(crate) async fn observe_reviewed_mcp_attachment(
        &self,
        grant: &PrivateServiceGrantRef,
        original: &OriginalWorkRef,
        destination: &NativeWorkTarget,
        control: &McpAttachmentControlRequest,
        binding: &PreparedAuthorizationBinding,
    ) -> Result<(), PrivateOriginalError> {
        let stored = self.store.get(&self.access_key(grant)?).await.map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        let record = self.decode_access(&stored, grant)?;
        let (AccessDecision::Mcp(decision), Some(selection)) = (&record.decision, &record.mcp)
            else { return Err(PrivateOriginalError::Denied); };
        let route = self.route(&grant.route)?;
        if original.authority != route.source_authority
            || !matches!(&decision.permission.lifetime.scope,
                LifetimeScope::Task { task } if task.as_str() == original.work.as_str())
        { return Err(PrivateOriginalError::Denied); }
        let now = self.owner.clock().now().map_err(|_| PrivateOriginalError::Unavailable)?.unix_ms;
        selection.validate_live_attachment(route, decision, destination, control, binding, now)?;
        if self.access_observation(grant.clone(), &record)?.state != PrivateServiceGrantState::GrantIssued {
            return Err(PrivateOriginalError::Unavailable);
        }
        Ok(())
    }

    /// Native resolves this association's own grant before the callback and
    /// independently checks current authority, connector approval and entry.
    pub(crate) fn check_service_mcp_operation(
        &self,
        association: &InputAuthorityAssociation,
        server: &str,
        raw_operation: &str,
        connector: &McpConnectorApproval,
        now_ms: u64,
    ) -> Result<(), PrivateOriginalError> {
        let retained = self
            .retained
            .lock()
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        let held = retained
            .iter()
            .find(|held| held.original.association() == association)
            .ok_or(PrivateOriginalError::Unavailable)?;
        let selected = held
            .service_mcp
            .iter().find(|selected| selected.selection.server == server)
            .ok_or(PrivateOriginalError::Unavailable)?;
        let route = self.route(&held.reference.route)?;
        selected.selection.validate(route, &selected.decision)?;
        let claims = association.candidate();
        if selected.decision.route != held.reference.route
            || selected.selection.server != server
            || !selected
                .decision
                .permission
                .operations
                .iter()
                .any(|operation| operation.as_str() == raw_operation)
            || &selected.selection.approval() != connector
            || route.logical_executor != claims.logical_executor
            || route.grant_audience != claims.target.audience
            || !selected.decision.permission.lifetime.active_at(now_ms)
        {
            return Err(PrivateOriginalError::Denied);
        }
        // Connection/Revision/resource are this decision's immutable selection
        // evidence. No physical generation is converted into a Toolkit revision.
        Ok(())
    }
}
#[cfg(test)]
mod tests;
