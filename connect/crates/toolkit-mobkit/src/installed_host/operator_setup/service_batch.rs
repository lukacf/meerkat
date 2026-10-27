//! One actual protected decision, with independently named native parents.
//! The singleton decision format and approval-ID derivation remain unchanged.
use super::*;
use serde_json::Value;
use crate::private_originals::service_grants::catalog::ServiceCatalogDecision;
use crate::private_originals::PrivateOriginalError;

/// A deliberately bounded public MCP selection, not a generic batch runner.
pub const MAX_PUBLIC_SERVICE_BATCH: usize = 8;

pub(crate) fn member_decision_id(original: &EvidenceId, connection: Option<&Id>)
    -> Result<Id, HostLifetimeError>
{
    Id::new(format!("service-batch-{}", digest(&canonical_json(&(
        "toolkit.public-service-batch-member.v1", original, connection,
    )).map_err(|_| HostLifetimeError::InvalidSelection)?)))
        .map_err(|_| HostLifetimeError::InvalidSelection)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum ServiceBatchMemberIdentity {
    PublicServicesV1 { set_sha256: String, index: u8 },
    PublicServiceCatalogV1 { set_sha256: String },
    PublicServicesWithSetupV2 { set_sha256: String, index: u8 },
    PublicServiceCatalogWithSetupV2 { set_sha256: String },
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BatchMemberBody {
    server: String,
    parent: Value,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum BatchReviewBody {
    PublicServicesV1 {
        members: Vec<BatchMemberBody>,
        catalog: Value,
        access_request: Value,
    },
    PublicServicesWithSetupV2 {
        members: Vec<BatchMemberBody>, catalog: Value,
        setup: crate::private_originals::ServiceSetupSelection,
        access_request: Value,
    },
}

/// Created by the same native operator that prepared every member. The one
/// returned request goes to the existing ApprovalService without fake records.
pub struct ServiceMcpParentBatchReview {
    request: ApprovalRequest,
    members: Vec<ServiceMcpParentReview>,
    catalog: ServiceCatalogParentReview,
    set_sha256: String,
}

impl ServiceMcpParentBatchReview {
    pub fn request(&self) -> &ApprovalRequest { &self.request }

    pub(crate) fn set_sha256(&self) -> &str { &self.set_sha256 }
    pub(crate) fn members(&self) -> &[ServiceMcpParentReview] { &self.members }
    pub(crate) fn catalog(&self) -> &ServiceCatalogParentReview { &self.catalog }
    pub(crate) fn into_parts(self) -> (Vec<ServiceMcpParentReview>, ServiceCatalogParentReview) {
        (self.members, self.catalog)
    }
}

fn selection_digest(members: &[BatchMemberBody], catalog: &Value) -> Result<String, HostLifetimeError> {
    if members.len() < 2 || members.len() > MAX_PUBLIC_SERVICE_BATCH
        || members.iter().any(|member| member.server.is_empty())
        || members.windows(2).any(|pair| pair[0].server >= pair[1].server)
    {
        return Err(HostLifetimeError::InvalidSelection);
    }
    canonical_json(&("toolkit.public-service-batch-selection.v1", members, catalog))
        .map(|bytes| digest(&bytes)).map_err(|_| HostLifetimeError::InvalidSelection)
}

fn setup_selection_digest(members: &[BatchMemberBody], catalog: &Value,
    setup: Option<&crate::private_originals::ServiceSetupSelection>)
    -> Result<String, HostLifetimeError> {
    let Some(setup) = setup else { return selection_digest(members, catalog); };
    setup.validate().map_err(|_| HostLifetimeError::InvalidSelection)?;
    if members.is_empty() || members.len() > MAX_PUBLIC_SERVICE_BATCH
        || members.iter().any(|member| member.server.is_empty())
        || members.windows(2).any(|pair| pair[0].server >= pair[1].server) {
        return Err(HostLifetimeError::InvalidSelection);
    }
    canonical_json(&("toolkit.public-service-setup-selection.v2", members, catalog, setup))
        .map(|bytes| digest(&bytes)).map_err(|_| HostLifetimeError::InvalidSelection)
}

impl OperatorSetup {
    /// Freeze the complete set before native issuance. No member can change
    /// its request after the real shared approval has been returned.
    pub(crate) fn prepare_service_mcp_parent_batch_review(
        &self, members: Vec<ServiceMcpParentReview>, catalog: ServiceCatalogDecision,
        request: ApprovalRequest,
    ) -> Result<ServiceMcpParentBatchReview, HostLifetimeError> {
        self.prepare_service_mcp_parent_set_review(members, catalog, request, None)
    }

    pub(crate) fn prepare_service_mcp_parent_set_review(
        &self, mut members: Vec<ServiceMcpParentReview>, catalog: ServiceCatalogDecision,
        mut request: ApprovalRequest,
        setup: Option<crate::private_originals::ServiceSetupSelection>,
    ) -> Result<ServiceMcpParentBatchReview, HostLifetimeError> {
        if members.len() < (if setup.is_some() { 1 } else { 2 }) || members.len() > MAX_PUBLIC_SERVICE_BATCH {
            return Err(HostLifetimeError::InvalidSelection);
        }
        members.sort_by(|left, right| left.decision().permission.connection
            .cmp(&right.decision().permission.connection));
        let first = members.first().ok_or(HostLifetimeError::InvalidSelection)?;
        let route = first.decision().route.clone();
        let lifetime = first.decision().permission.lifetime.scope.clone();
        let mut selected = Vec::with_capacity(members.len());
        for member in &members {
            self.check_parent_proposal_owner(&member.proposal)?;
            if member.proposal.batch_member.is_some()
                || member.proposal.purpose != OperatorParentPurpose::Operations
                || member.proposal.setup_request.is_some()
                || member.decision().route != route
                || member.decision().permission.lifetime.scope != lifetime
                || member.decision().live_attachment.is_none()
                || member.decision().catalog.is_some()
            {
                return Err(HostLifetimeError::InvalidSelection);
            }
            // Revalidate the selected correspondence, not only its label.
            if self.service_mcp_parent_restrictions(member.decision(), member.selection())?
                != member.proposal.restrictions
            {
                return Err(HostLifetimeError::InvalidSelection);
            }
            selected.push(BatchMemberBody {
                server: member.decision().permission.connection.to_string(),
                parent: member.request().proposed_action.body.clone()
                    .ok_or(HostLifetimeError::InvalidSelection)?,
            });
        }
        if catalog.route != route || catalog.lifetime.scope != lifetime
            || members.iter().any(|member| member.decision().live_attachment.as_ref()
                .is_none_or(|effect| effect.destination.context.as_str() != catalog.session.to_string()))
        { return Err(HostLifetimeError::InvalidSelection); }
        match (&catalog.kind, &setup) {
            (crate::private_originals::service_grants::catalog::CatalogDecisionKind::PublicServiceCatalogV1, None) => {},
            (crate::private_originals::service_grants::catalog::CatalogDecisionKind::PublicServiceCatalogWithSetupV2 { helper }, Some(setup)) => {
                setup.validate().map_err(|_| HostLifetimeError::InvalidSelection)?;
                let route_row = self.native.routes.iter().find(|row| row.id == route)
                    .ok_or(HostLifetimeError::InvalidSelection)?;
                let (binding, _, _) = route_row.source_binding(self.lifetime.selected_policy())?;
                let member = self.native.members.iter().find(|row| &row.member == binding)
                    .ok_or(HostLifetimeError::InvalidSelection)?;
                let selected_helper = self.native.members.iter().find(|row| row.member == helper.member)
                    .ok_or(HostLifetimeError::InvalidSelection)?;
                if member.setup_capability.as_ref() != Some(&setup.configured.capability)
                    || setup.configured.capability.route != route || setup.configured.session_id != catalog.session
                    || &setup.helper != helper || catalog.lifetime.expires_at_ms != Some(setup.expires_at_ms)
                    || selected_helper.logical_owner != helper.logical_executor
                    || selected_helper.audience != helper.audience || member.audience != helper.audience {
                    return Err(HostLifetimeError::InvalidSelection);
                }
            },
            _ => return Err(HostLifetimeError::InvalidSelection),
        }
        let mut catalog_restrictions = catalog.restrictions(&self.service_mcp_route(&route)?, 0)
            .map_err(|_| HostLifetimeError::InvalidSelection)?;
        catalog_restrictions.delegation_depth = meerkat_authorization_contracts::constraints::DelegationDepth::remaining(1);
        let mut catalog_proposal = self.prepare_parent_review(route, catalog_restrictions, request.clone())?;
        let catalog_body = catalog_proposal.request.proposed_action.body.as_mut()
            .and_then(Value::as_object_mut).ok_or(HostLifetimeError::InvalidSelection)?;
        catalog_body.insert("catalog_access".into(), serde_json::to_value(&catalog)
            .map_err(|_| HostLifetimeError::InvalidSelection)?);
        catalog_body.insert("access_request".into(), request.proposed_action.body.clone()
            .ok_or(HostLifetimeError::InvalidSelection)?);
        let catalog_body = catalog_proposal.request.proposed_action.body.clone()
            .ok_or(HostLifetimeError::InvalidSelection)?;
        let access_request = request.proposed_action.body.take().ok_or(HostLifetimeError::InvalidSelection)?;
        let set_sha256 = setup_selection_digest(&selected, &catalog_body, setup.as_ref())?;
        let with_setup = setup.is_some();
        let body = match setup {
            Some(setup) => BatchReviewBody::PublicServicesWithSetupV2 {
                members: selected, catalog: catalog_body, setup, access_request,
            },
            None => BatchReviewBody::PublicServicesV1 {
                members: selected, catalog: catalog_body, access_request,
            },
        };
        request.proposed_action.summary = "Review this exact set of public services for the task".into();
        request.proposed_action.body = Some(serde_json::to_value(body)
            .map_err(|_| HostLifetimeError::InvalidSelection)?);
        request.request_body = request.proposed_action.body.clone();
        for (index, member) in members.iter_mut().enumerate() {
            member.proposal.batch_member = Some(if with_setup {
                ServiceBatchMemberIdentity::PublicServicesWithSetupV2 { set_sha256: set_sha256.clone(), index: index as u8 }
            } else {
                ServiceBatchMemberIdentity::PublicServicesV1 { set_sha256: set_sha256.clone(), index: index as u8 }
            });
            member.proposal.request = request.clone();
        }
        catalog_proposal.batch_member = Some(if with_setup {
            ServiceBatchMemberIdentity::PublicServiceCatalogWithSetupV2 { set_sha256: set_sha256.clone() }
        } else {
            ServiceBatchMemberIdentity::PublicServiceCatalogV1 { set_sha256: set_sha256.clone() }
        });
        catalog_proposal.request = request.clone();
        Ok(ServiceMcpParentBatchReview { request, members, set_sha256,
            catalog: ServiceCatalogParentReview { proposal: catalog_proposal, decision: catalog } })
    }
}

/// Return the selected member's retained review body. This is an evidence
/// projection, never a synthetic ApprovalRecord or a second review decision.
pub(super) fn member_body(record: &OperatorDecision) -> Result<Option<Value>, HostLifetimeError> {
    let Some(identity) = &record.batch_member else {
        return Ok(record.approval.proposed_action.body.clone());
    };
    if record.purpose != OperatorParentPurpose::Operations || record.setup_request.is_some() {
        return Err(HostLifetimeError::InvalidSelection);
    }
    let body: BatchReviewBody = serde_json::from_value(record.approval.proposed_action.body.clone()
        .ok_or(HostLifetimeError::InvalidSelection)?)
        .map_err(|_| HostLifetimeError::InvalidSelection)?;
    let (members, catalog, setup) = match body {
        BatchReviewBody::PublicServicesV1 { members, catalog, .. } => (members, catalog, None),
        BatchReviewBody::PublicServicesWithSetupV2 { members, catalog, setup, .. } => (members, catalog, Some(setup)),
    };
    let with_setup = matches!(identity, ServiceBatchMemberIdentity::PublicServicesWithSetupV2 { .. }
        | ServiceBatchMemberIdentity::PublicServiceCatalogWithSetupV2 { .. });
    if with_setup != setup.is_some() { return Err(HostLifetimeError::InvalidSelection); }
    let (set_sha256, selected, server) = match identity {
        ServiceBatchMemberIdentity::PublicServicesV1 { set_sha256, index }
        | ServiceBatchMemberIdentity::PublicServicesWithSetupV2 { set_sha256, index } => {
            let member = members.get(usize::from(*index)).ok_or(HostLifetimeError::InvalidSelection)?;
            (set_sha256, &member.parent, Some(member.server.as_str()))
        }
        ServiceBatchMemberIdentity::PublicServiceCatalogV1 { set_sha256 }
        | ServiceBatchMemberIdentity::PublicServiceCatalogWithSetupV2 { set_sha256 } => (set_sha256, &catalog, None),
    };
    if setup_selection_digest(&members, &catalog, setup.as_ref())? != *set_sha256 {
        return Err(HostLifetimeError::InvalidSelection);
    }
    let selected_body = selected.clone();
    let selected = selected.as_object().ok_or(HostLifetimeError::InvalidSelection)?;
    let equals = |name: &str, value: Value| selected.get(name) == Some(&value);
    if !equals("operator", serde_json::to_value(&record.operator).map_err(|_| HostLifetimeError::InvalidSelection)?)
        || !equals("application", serde_json::to_value(&record.application).map_err(|_| HostLifetimeError::InvalidSelection)?)
        || !equals("route", serde_json::to_value(&record.route).map_err(|_| HostLifetimeError::InvalidSelection)?)
        || !equals("purpose", serde_json::to_value(record.purpose).map_err(|_| HostLifetimeError::InvalidSelection)?)
        || !equals("represented_subject", serde_json::to_value(&record.represented_subject).map_err(|_| HostLifetimeError::InvalidSelection)?)
        || !equals("restrictions", serde_json::to_value(&record.requested).map_err(|_| HostLifetimeError::InvalidSelection)?)
        || server.is_some_and(|server| selected.get("connector_selection").and_then(|selection| selection.get("server"))
            .and_then(Value::as_str) != Some(server))
    {
        return Err(HostLifetimeError::InvalidSelection);
    }
    Ok(Some(selected_body))
}

pub(crate) struct ServiceCatalogParentReview {
    proposal: OperatorParentProposal,
    decision: ServiceCatalogDecision,
}
impl ServiceCatalogParentReview {
    pub(crate) fn decision(&self) -> &ServiceCatalogDecision { &self.decision }
}

pub(crate) struct ReviewedServiceCatalogParent {
    decision: ServiceCatalogDecision,
    lineage: Vec<GrantLineageRef>,
    source: std::sync::Weak<ToolkitOriginalOwner>,
    lifetime: std::sync::Weak<NativeHostLifetime>,
}
impl ReviewedServiceCatalogParent {
    pub(crate) fn into_parts(self, source: &Arc<ToolkitOriginalOwner>)
        -> Result<(ServiceCatalogDecision, Vec<GrantLineageRef>, std::sync::Weak<NativeHostLifetime>), PrivateOriginalError>
    {
        let owner = self.source.upgrade().ok_or(PrivateOriginalError::Unavailable)?;
        if !Arc::ptr_eq(source, &owner) { return Err(PrivateOriginalError::Denied); }
        self.lifetime.upgrade().ok_or(PrivateOriginalError::Unavailable)?
            .validate_current_selection().map_err(|_| PrivateOriginalError::Denied)?;
        Ok((self.decision, self.lineage, self.lifetime))
    }
}

impl OperatorSetup {
    pub(crate) fn catalog_parent_compensation(&self, review: &ServiceCatalogParentReview,
        approval: &ApprovalRecord) -> Result<ServiceMcpParentCompensation, HostLifetimeError>
    {
        let expected = self.parent_decision_record(&review.proposal, approval.clone())?;
        validate_member(&expected)?;
        let key = self.parent_decision_key(&expected)?;
        Ok(self.retained_service_compensation(expected, key))
    }

    pub(crate) async fn apply_catalog_parent(&self, review: ServiceCatalogParentReview,
        approval: ApprovalRecord, presented: Arc<dyn Fn() -> bool + Send + Sync>)
        -> Result<ReviewedServiceCatalogParent, HostLifetimeError>
    {
        let expected = self.parent_decision_record(&review.proposal, approval.clone())?;
        validate_member(&expected)?;
        match self.apply_parent_decision_current(review.proposal, approval, presented).await? {
            OperatorParentOutcome::Selected { route, purpose: OperatorParentPurpose::Operations, lineage }
                if route == review.decision.route => Ok(ReviewedServiceCatalogParent {
                    decision: review.decision, lineage, source: Arc::downgrade(&self.source),
                    lifetime: Arc::downgrade(&self.lifetime),
                }),
            _ => Err(HostLifetimeError::Unavailable),
        }
    }
}

pub(super) fn validate_member(record: &OperatorDecision) -> Result<(), HostLifetimeError> {
    if record.batch_member.is_some() { member_body(record)?; }
    Ok(())
}

#[cfg(test)]
#[path = "service_batch_tests.rs"]
mod tests;
