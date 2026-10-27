//! An explicit, task-specific catalog leaf for a reviewed public service set.
//! It uses the existing access intent and native grant owners. It carries no
//! MCP operation and cannot authorize a missing target through a broad base.
use meerkat_authorization_contracts::constraints::DomainRestriction;
use super::*;
use meerkat_core::SessionId;
use meerkat_toolkit::authority::RoleLifetime;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CatalogDecisionKind {
    PublicServiceCatalogV1,
    PublicServiceCatalogWithSetupV2 {
        helper: crate::private_originals::configured_setup::service::ServiceSetupHelper,
    },
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ServiceCatalogDecision {
    pub kind: CatalogDecisionKind,
    pub route: Id,
    pub decision: Id,
    pub evidence: PrivateSourceEvidence,
    pub session: SessionId,
    pub original: EvidenceId,
    pub lifetime: RoleLifetime,
}

impl ServiceCatalogDecision {
    pub(crate) fn restrictions(&self, route: &PrivateOriginalRoute, now_ms: u64)
        -> Result<ExecutionRestrictions, PrivateOriginalError>
    {
        if self.route != route.id || !valid_digest(&self.evidence.sha256)
            || !self.lifetime.active_at(now_ms)
            || !matches!(&self.lifetime.scope, LifetimeScope::Task { task }
                if task.as_str() == self.original.as_str())
        { return Err(PrivateOriginalError::Denied); }
        let mut result = crate::installed_host::application_operations::parent_restrictions(
            crate::installed_host::application_operations::catalog_read(
                &route.grant_caller, &route.logical_executor, &route.grant_audience, &self.session));
        if let CatalogDecisionKind::PublicServiceCatalogWithSetupV2 { helper } = &self.kind {
            use meerkat_authorization_contracts::constraints::{ExactRestriction, ProcessorRef};
            if helper.audience != route.grant_audience || helper.logical_executor == route.logical_executor
                || helper.session_id == self.session {
                return Err(PrivateOriginalError::Denied);
            }
            let helper_values = crate::installed_host::application_operations::catalog_read(
                &route.grant_caller, &helper.logical_executor, &helper.audience, &helper.session_id);
            let original_values = crate::installed_host::application_operations::catalog_read(
                &route.grant_caller, &route.logical_executor, &route.grant_audience, &self.session);
            // The protected review explicitly authorizes this full 2x2 metadata
            // scope. These independent restrictions do not encode paired tuples.
            result.processors = ExactRestriction::exact([
                ProcessorRef::Principal { principal: route.logical_executor.clone() },
                ProcessorRef::Principal { principal: helper.logical_executor.clone() },
            ]);
            result.resource_domains = DomainRestriction::exact([
                original_values.resource_domain, helper_values.resource_domain,
            ]);
        }
        result.delegation_depth = DelegationDepth::remaining(0);
        result.lifetime = LifetimeRestriction::window(0,
            self.lifetime.expires_at_ms.ok_or(PrivateOriginalError::Denied)?);
        Ok(result)
    }
}

impl PrivateOriginals {
    pub(crate) fn service_catalog_reference(decision: &ServiceCatalogDecision)
        -> Result<PrivateServiceGrantRef, PrivateOriginalError>
    {
        Ok(PrivateServiceGrantRef { route: decision.route.clone(), decision: decision.decision.clone(),
            decision_sha256: decision_digest(&AccessDecision::Catalog(decision.clone()), None)? })
    }

    pub(crate) async fn apply_service_catalog_access(
        &self, approved: crate::installed_host::operator_setup::ReviewedServiceCatalogParent,
        presented: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Result<PrivateServiceGrantObservation, PrivateOriginalError> {
        let (decision, parent, lifetime) = approved.into_parts(&self.owner)?;
        self.apply_service_access(AccessDecision::Catalog(decision), None, Some(parent), Some(lifetime), None,
            Arc::new(move || if presented() { Ok(()) } else { Err(PrivateOriginalError::Denied) })).await
    }
}
