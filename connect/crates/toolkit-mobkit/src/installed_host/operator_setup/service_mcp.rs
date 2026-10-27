//! Exact service decision through the existing protected operator owner.
use super::*;
use crate::private_originals::{
    PrivateAccessChoice, PrivateOriginalRoute, PrivateServiceMcpAccessDecision, ServiceMcpSelection,
};

/// The existing native approval request, with the whole proposed service scope
/// and any caller task context included in the same retained review body.
pub struct ServiceMcpParentReview {
    pub(super) proposal: OperatorParentProposal,
    decision: PrivateServiceMcpAccessDecision,
    selection: ServiceMcpSelection,
}
impl ServiceMcpParentReview {
    pub fn request(&self) -> &ApprovalRequest {
        self.proposal.request()
    }
    pub fn decision(&self) -> &PrivateServiceMcpAccessDecision {
        &self.decision
    }
    pub fn selection(&self) -> &ServiceMcpSelection {
        &self.selection
    }
}
/// Exact result of that operator review and native root issuance. It is not
/// serializable authority or an update to a currently selected route. Every
/// later use still resolves the native parent and its full restrictions.
#[derive(Clone)]
pub struct ReviewedServiceMcpParent {
    decision: PrivateServiceMcpAccessDecision,
    selection: ServiceMcpSelection,
    lineage: Vec<GrantLineageRef>,
    source: std::sync::Weak<ToolkitOriginalOwner>,
    lifetime: std::sync::Weak<NativeHostLifetime>,
}
impl ReviewedServiceMcpParent {
    pub fn decision(&self) -> &PrivateServiceMcpAccessDecision {
        &self.decision
    }
    pub fn selection(&self) -> &ServiceMcpSelection {
        &self.selection
    }
    pub(crate) fn into_parts(
        self,
        source: &Arc<ToolkitOriginalOwner>,
    ) -> Result<
        (
            PrivateServiceMcpAccessDecision,
            ServiceMcpSelection,
            Vec<GrantLineageRef>,
            std::sync::Weak<NativeHostLifetime>,
        ),
        crate::private_originals::PrivateOriginalError,
    > {
        self.into_parts_with_errors(source, false)
    }

    pub(crate) fn into_parts_current(
        self, source: &Arc<ToolkitOriginalOwner>,
    ) -> Result<(PrivateServiceMcpAccessDecision, ServiceMcpSelection, Vec<GrantLineageRef>,
        std::sync::Weak<NativeHostLifetime>), crate::private_originals::PrivateOriginalError>
    {
        self.into_parts_with_errors(source, true)
    }

    pub(crate) fn into_parts_operation(self, source: &Arc<ToolkitOriginalOwner>)
        -> Result<(PrivateServiceMcpAccessDecision, ServiceMcpSelection, Vec<GrantLineageRef>,
            std::sync::Weak<NativeHostLifetime>), crate::private_originals::ServiceOperationError>
    {
        use crate::private_originals::PrivateOriginalError;
        let retained = self.source.upgrade().ok_or(PrivateOriginalError::Unavailable)?;
        if !Arc::ptr_eq(source, &retained) { return Err(PrivateOriginalError::Denied.into()); }
        let lifetime = self.lifetime.upgrade().ok_or(PrivateOriginalError::Unavailable)?;
        lifetime.validate_current_selection()?;
        Ok((self.decision, self.selection, self.lineage, self.lifetime))
    }

    fn into_parts_with_errors(self, source: &Arc<ToolkitOriginalOwner>, typed: bool)
        -> Result<(PrivateServiceMcpAccessDecision, ServiceMcpSelection, Vec<GrantLineageRef>,
            std::sync::Weak<NativeHostLifetime>), crate::private_originals::PrivateOriginalError>
    {
        use crate::private_originals::PrivateOriginalError;
        let retained = self
            .source
            .upgrade()
            .ok_or(PrivateOriginalError::Unavailable)?;
        if !Arc::ptr_eq(source, &retained) {
            return Err(PrivateOriginalError::Denied);
        }
        let lifetime = self
            .lifetime
            .upgrade()
            .ok_or(PrivateOriginalError::Unavailable)?;
        lifetime
            .validate_current_selection()
            .map_err(|error| match error {
                HostLifetimeError::Unavailable | HostLifetimeError::NotSettled if typed => PrivateOriginalError::Unavailable,
                _ => PrivateOriginalError::Denied,
            })?;
        Ok((self.decision, self.selection, self.lineage, self.lifetime))
    }
}
pub enum ServiceMcpParentOutcome {
    Approved(ReviewedServiceMcpParent),
    Denied,
    Unavailable,
    PendingReconciliation,
}
impl OperatorSetup {
    pub(super) fn service_parent_result(&self, decision: PrivateServiceMcpAccessDecision,
        selection: ServiceMcpSelection, lineage: Vec<GrantLineageRef>) -> ReviewedServiceMcpParent {
        ReviewedServiceMcpParent { decision, selection, lineage,
            source: Arc::downgrade(&self.source), lifetime: Arc::downgrade(&self.lifetime) }
    }
    pub(super) fn service_mcp_route(
        &self,
        route_id: &Id,
    ) -> Result<PrivateOriginalRoute, HostLifetimeError> {
        let selected = self.route(route_id)?;
        let (binding, requester, actor) =
            selected.source_binding(self.lifetime.selected_policy())?;
        let member = self
            .native
            .members
            .iter()
            .find(|member| &member.member == binding)
            .ok_or(HostLifetimeError::InvalidSelection)?;
        // Same immutable selected route facts used by ordinary installed ingress.
        Ok(PrivateOriginalRoute {
            id: selected.id.clone(),
            grant_caller: self.native.grant_administrator.clone(),
            application_parent: selected.application_parent.clone(),
            controller_parent: selected.controller_parent.clone(),
            grant_target: Id::new(binding.member.clone())
                .map_err(|_| HostLifetimeError::InvalidSelection)?,
            grant_audience: member.audience.clone(),
            realm: self.native.realm.clone(),
            requester: requester.clone(),
            ingress_actor: actor.clone(),
            logical_executor: selected.logical_executor.clone(),
            represented_subject: selected.represented_subject.clone(),
            source_authority: selected.source_authority.clone(),
            authentication_domain: selected.authentication_domain.clone(),
            event_domain: selected.event_domain.clone(),
        })
    }

    pub fn prepare_service_mcp_parent_review(
        &self,
        decision: PrivateServiceMcpAccessDecision,
        selection: ServiceMcpSelection,
        mut request: ApprovalRequest,
    ) -> Result<ServiceMcpParentReview, HostLifetimeError> {
        let restrictions = self.service_mcp_parent_restrictions(&decision, &selection)?;
        let access_request = request.proposed_action.body.take();
        let mut proposal =
            self.prepare_parent_review(decision.route.clone(), restrictions, request)?;
        let body = proposal
            .request
            .proposed_action
            .body
            .as_mut()
            .and_then(serde_json::Value::as_object_mut)
            .ok_or(HostLifetimeError::InvalidSelection)?;
        body.insert("service_access".into(),
            serde_json::to_value(&decision).map_err(|_| HostLifetimeError::InvalidSelection)?);
        body.insert("connector_selection".into(), selection.reviewed_content()
            .map_err(|_| HostLifetimeError::InvalidSelection)?);
        if let Some(context) = access_request { body.insert("access_request".into(), context); }
        Ok(ServiceMcpParentReview { proposal, decision, selection })
    }

    pub(super) fn service_mcp_parent_restrictions(&self,
        decision: &PrivateServiceMcpAccessDecision, selection: &ServiceMcpSelection,
    ) -> Result<ExecutionRestrictions, HostLifetimeError> {
        if decision.choice != PrivateAccessChoice::Approve {
            return Err(HostLifetimeError::Denied);
        }
        let route = self.service_mcp_route(&decision.route)?;
        if matches!(&decision.permission.lifetime.scope,
            meerkat_toolkit::authority::LifetimeScope::Deployment { deployment }
            if deployment != &self.lifetime.selected_policy().deployment)
        {
            return Err(HostLifetimeError::Denied);
        }
        if let Some(effect) = &decision.live_attachment {
            let (binding, _, _) = self.route(&decision.route)?
                .source_binding(self.lifetime.selected_policy())?;
            let member = self.native.members.iter().find(|member| &member.member == binding)
                .ok_or(HostLifetimeError::InvalidSelection)?;
            if effect.destination.logical_owner != member.logical_owner
                || effect.destination.context_generation != member.context_generation
                || effect.destination.audience != member.audience
            { return Err(HostLifetimeError::Denied); }
        }
        let now = self
            .source
            .clock()
            .now()
            .map_err(|_| HostLifetimeError::Unavailable)?
            .unix_ms;
        let mut restrictions = selection
            .restrictions(&route, &decision, now)
            .map_err(|_| HostLifetimeError::Denied)?;
        restrictions.delegation_depth =
            meerkat_authorization_contracts::constraints::DelegationDepth::remaining(1);
        Ok(restrictions)
    }

    /// Keep this future through settlement, as for apply_parent_decision. A lost
    /// result does not authorize another issuance or automatic decision replay.
    pub async fn apply_service_mcp_parent_decision(
        &self,
        review: ServiceMcpParentReview,
        approval: ApprovalRecord,
    ) -> Result<ServiceMcpParentOutcome, HostLifetimeError> {
        self.apply_service_mcp_parent_decision_current(review, approval, Arc::new(|| true)).await
    }

    pub(crate) fn validate_service_mcp_parent_approval(&self, review: &ServiceMcpParentReview,
        approval: &ApprovalRecord) -> Result<bool, HostLifetimeError> {
        self.check_parent_proposal_owner(&review.proposal)?;
        if !same_request(&review.proposal.request, approval) { return Err(HostLifetimeError::Denied); }
        match (&approval.decision, approval.status) {
            (Some(decision), ApprovalStatus::Approved) if decision.decision == ApprovalDecision::Approve => Ok(true),
            (Some(decision), ApprovalStatus::Denied) if decision.decision == ApprovalDecision::Deny => Ok(false),
            _ => Err(HostLifetimeError::Denied),
        }
    }

    pub(crate) async fn apply_service_mcp_parent_decision_current(
        &self, review: ServiceMcpParentReview, approval: ApprovalRecord,
        presented: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Result<ServiceMcpParentOutcome, HostLifetimeError> {
        let ServiceMcpParentReview {
            proposal,
            decision,
            selection,
        } = review;
        Ok(
            match self.apply_parent_decision_current(proposal, approval, presented).await? {
                OperatorParentOutcome::Selected {
                    route,
                    purpose: OperatorParentPurpose::Operations,
                    lineage,
                } if route == decision.route => {
                    ServiceMcpParentOutcome::Approved(ReviewedServiceMcpParent {
                        decision,
                        selection,
                        lineage,
                        source: Arc::downgrade(&self.source),
                        lifetime: Arc::downgrade(&self.lifetime),
                    })
                }
                OperatorParentOutcome::Selected { .. } | OperatorParentOutcome::SetupRequestSelected { .. } => {
                    return Err(HostLifetimeError::WrongOwner);
                }
                OperatorParentOutcome::Denied => ServiceMcpParentOutcome::Denied,
                OperatorParentOutcome::Unavailable => ServiceMcpParentOutcome::Unavailable,
                OperatorParentOutcome::PendingReconciliation => {
                    ServiceMcpParentOutcome::PendingReconciliation
                }
            },
        )
    }
}

/// Exact retained issuance intent from one validated approval. This is an
/// in-process failure handle, not a second grant ledger or cold recovery permit.
pub(crate) struct ServiceMcpParentCompensation {
    expected: OperatorDecision,
    key: StoreKey,
    originals: Arc<dyn ToolkitStore>,
    source: Arc<ToolkitOriginalOwner>,
}
impl OperatorSetup {
    pub(super) fn retained_service_compensation(&self, expected: OperatorDecision, key: StoreKey)
        -> ServiceMcpParentCompensation
    {
        ServiceMcpParentCompensation { expected, key,
            originals: self.originals.clone(), source: self.source.clone() }
    }

    pub(crate) fn service_mcp_parent_compensation(&self, review: &ServiceMcpParentReview,
        approval: &ApprovalRecord) -> Result<Option<ServiceMcpParentCompensation>, HostLifetimeError>
    {
        if !self.validate_service_mcp_parent_approval(review, approval)? { return Ok(None); }
        let expected = self.parent_decision_record(&review.proposal, approval.clone())?;
        let key = self.parent_decision_key(&expected)?;
        Ok(Some(ServiceMcpParentCompensation { expected, key,
            originals: self.originals.clone(), source: self.source.clone() }))
    }
}
impl ServiceMcpParentCompensation {
    pub(crate) fn belongs_to(&self, source: &Arc<ToolkitOriginalOwner>) -> bool {
        Arc::ptr_eq(&self.source, source)
    }

    pub(crate) fn issuer(&self) -> &meerkat_core::PrincipalRef { &self.expected.operator }

    /// Called only after the issuance future and any native worker have joined.
    /// Pending store acknowledgement is reconciled with the actual native owner;
    /// a failed native lookup is never absence or permission to issue again.
    pub(crate) async fn issued(&self) -> Result<Option<GrantLineageRef>, HostLifetimeError> {
        self.reference_observation().await
    }

    /// Pure native identity observation. Absence while a worker may remain is
    /// unresolved, never final nonentry or a cancellation receipt.
    pub(crate) async fn reference_observation(&self) -> Result<Option<GrantLineageRef>, HostLifetimeError> {
        let saved = self.originals.get(&self.key).await.map_err(|_| HostLifetimeError::Unavailable)?;
        let native = self.source.grants().issued_reference(
            &EvidenceId::new(self.key.id.clone()).map_err(|_| HostLifetimeError::InvalidSelection)?)
            .map_err(|_| HostLifetimeError::Unavailable)?;
        let Some(saved) = saved else {
            return if native.is_none() { Ok(None) } else { Err(HostLifetimeError::Unavailable) };
        };
        let actual = decode(&saved, &self.key)?;
        if decision_body(&actual)? != decision_body(&self.expected)? {
            return Err(HostLifetimeError::Denied);
        }
        match (&actual.issued, &native) {
            (ParentIssuance::Pending, _) => {},
            (ParentIssuance::Issued { lineage }, Some(leaf)) if lineage.as_slice() == [leaf.clone()] => {},
            (ParentIssuance::NativeRefused | ParentIssuance::Denied, None) => {},
            _ => return Err(HostLifetimeError::Unavailable),
        }
        Ok(native)
    }
}
