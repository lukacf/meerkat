//! Exact setup-only review on the existing operator decision owner.
use super::*;
use crate::private_originals::{LocalControllerSetup, PrivateSetupRequestSelection};
use meerkat_toolkit::host_lifetime::policy::CompiledNativeSourceOrigin;

impl OperatorSetup {
    /// This preparation has already left the native identity lease. The actual
    /// source owner and actual SessionId are retained, never chosen by the UI.
    pub fn prepare_original_setup_parent_review(
        &self,
        prepared: &LocalControllerSetup,
        request: ApprovalRequest,
    ) -> Result<OperatorParentProposal, HostLifetimeError> {
        if !prepared.matches_grant_owner(&self.source)
            || request.owner
                != (meerkat_core::ApprovalOwnerRef::Session {
                    session_id: prepared.session_id().clone(),
                })
            || !matches!(
                self.route(&prepared.original().route)?.origin,
                CompiledNativeSourceOrigin::LocalConsole { .. }
            )
        {
            return Err(HostLifetimeError::WrongOwner);
        }
        let mut proposal = self.prepare_setup_parent_review(
            prepared.original().route.clone(),
            prepared.session_id(),
            request,
        )?;
        let origin = crate::original_ingress::helper_originals::HelperSetupOrigin {
            realm: self.native.realm.clone(), source_owner: self.native.ingress_domain.clone(),
            original_authority: self.route(&prepared.original().route)?.source_authority.clone(),
            original: prepared.original().clone(),
        };
        let targets = self.original_helper_targets(&origin)?;
        proposal.setup_request = Some(SetupRequestIntent {
            session_id: prepared.session_id().clone(),
            original: Some(prepared.original().clone()),
            origin: (!targets.is_empty()).then_some(origin),
            targets,
        });
        proposal
            .request
            .proposed_action
            .body
            .as_mut()
            .and_then(serde_json::Value::as_object_mut)
            .ok_or(HostLifetimeError::InvalidSelection)?
            .insert(
                "original".into(),
                serde_json::to_value(prepared.original())
                    .map_err(|_| HostLifetimeError::InvalidSelection)?,
            );
        if let Some(intent) = &proposal.setup_request {
            proposal.request.proposed_action.body.as_mut().and_then(serde_json::Value::as_object_mut)
                .ok_or(HostLifetimeError::InvalidSelection)?.insert("correlated_helper_targets".into(),
                    serde_json::to_value(intent).map_err(|_| HostLifetimeError::InvalidSelection)?);
        }
        Ok(proposal)
    }
}

/// Read the one existing canonical decision named by its actual issued parent.
/// This is a historical selection join, not an alternative permission owner.
/// The native authority still resolves the complete current lineage at bind.
pub(crate) async fn validate_setup_selection(
    lifetime: &NativeHostLifetime,
    source: &ToolkitOriginalOwner,
    store: &dyn ToolkitStore,
    selection: &PrivateSetupRequestSelection,
) -> Result<ExecutionRestrictions, HostLifetimeError> {
    validate_selection(lifetime, source, store, selection, true).await
}

/// Reopen the same canonical approved selection without resolving unrelated
/// target currentness. The caller must already have its exact bound source.
pub(crate) async fn validate_retained_setup_selection(
    lifetime: &NativeHostLifetime, source: &ToolkitOriginalOwner, store: &dyn ToolkitStore,
    selection: &PrivateSetupRequestSelection,
) -> Result<ExecutionRestrictions, HostLifetimeError> {
    validate_selection(lifetime, source, store, selection, false).await
}

async fn validate_selection(
    lifetime: &NativeHostLifetime, source: &ToolkitOriginalOwner, store: &dyn ToolkitStore,
    selection: &PrivateSetupRequestSelection, require_current: bool,
) -> Result<ExecutionRestrictions, HostLifetimeError> {
    lifetime.validate_current_selection()?;
    let policy = lifetime.selected_policy();
    let native = policy
        .native_host
        .as_ref()
        .ok_or(HostLifetimeError::Unavailable)?;
    let route = native
        .routes
        .iter()
        .find(|route| route.id == selection.original.route)
        .ok_or(HostLifetimeError::Denied)?;
    if selection.parent.len() != 1
        || !matches!(
            route.origin,
            CompiledNativeSourceOrigin::LocalConsole { .. }
        )
    {
        return Err(HostLifetimeError::Denied);
    }
    let (member, _, _) = route.source_binding(policy)?;
    let member = native
        .members
        .iter()
        .find(|selected| &selected.member == member)
        .ok_or(HostLifetimeError::InvalidSelection)?;
    let expected =
        application_operations::parent_restrictions(application_operations::find_and_install(
            &native.grant_administrator,
            &route.logical_executor,
            &member.audience,
            &selection.session_id,
        ));
    let key = StoreKey {
        realm: native.realm.to_string(),
        owner: policy.deployment.to_string(),
        id: selection.parent[0].grant_id.as_str().to_owned(),
    };
    let stored = store
        .get(&key)
        .await
        .map_err(|_| HostLifetimeError::Unavailable)?
        .ok_or(HostLifetimeError::Unavailable)?;
    let record = decode(&stored, &key)?;
    if !super::setup_recovery::decision_key_matches(lifetime, native, &key, &record)?
        || record.route != selection.original.route
        || record.purpose != OperatorParentPurpose::SetupRequest
        || !record.setup_request.as_ref().is_some_and(|intent|
            intent.session_id == selection.session_id && intent.original.as_ref() == Some(&selection.original)
                && (intent.targets.is_empty() && intent.origin.is_none() || intent.origin.as_ref().is_some_and(|origin|
                    origin.realm == native.realm && origin.source_owner == native.ingress_domain
                        && origin.original_authority == route.source_authority
                        && origin.original == selection.original)))
        || setup_targets::compiled_targets(&record)? != selection.helper_targets
        || record.operator != selection.parent[0].root_authority
        || record.application != native.grant_administrator
        || record.represented_subject != route.represented_subject
        || record.requested != expected
        || record.approval.owner
            != (meerkat_core::ApprovalOwnerRef::Session {
                session_id: selection.session_id.clone(),
            })
        || record.approval.status != ApprovalStatus::Approved
        || !record
            .approval
            .decision
            .as_ref()
            .is_some_and(|decision| decision.decision == ApprovalDecision::Approve)
        || !matches!(&record.issued, ParentIssuance::Issued { lineage } if lineage == &selection.parent)
    {
        return Err(HostLifetimeError::Denied);
    }
    if require_current {
        let current = source
            .grants()
            .resolve_lineage(
                &selection.parent,
                &native.grant_administrator,
                route.represented_subject.as_ref(),
            )
            .map_err(|_| HostLifetimeError::Denied)?;
        if current.restrictions() != &expected {
            return Err(HostLifetimeError::Denied);
        }
        current
            .check_current()
            .map_err(|_| HostLifetimeError::Unavailable)?;
        for target in &selection.helper_targets {
            let current = source.grants().resolve_lineage(std::slice::from_ref(&target.parent),
                &native.grant_administrator, route.represented_subject.as_ref()).map_err(|_| HostLifetimeError::Denied)?;
            if current.restrictions() != &target.restrictions { return Err(HostLifetimeError::Denied); }
            current.check_current().map_err(|_| HostLifetimeError::Unavailable)?;
        }
    }
    lifetime.validate_current_selection()?;
    Ok(expected)
}

impl OperatorSetup {
    fn original_helper_targets(&self, origin: &crate::original_ingress::helper_originals::HelperSetupOrigin)
        -> Result<Vec<setup_targets::SetupTargetIntent>, HostLifetimeError>
    {
        use meerkat_authorization_contracts::work_association::WorkAuthorityBasis;
        use meerkat_authorization_contracts::constraints::DelegationDepth;
        use crate::installed_host::acquisition::ConfiguredHelperReportCapability;
        let Some(helper) = &self.helper_selection else { return Ok(vec![]); };
        let member = helper.selected_member(&self.native)?;
        let WorkAuthorityBasis::TargetedGrantLineages { base, targets } = &helper.operation_basis else {
            return Err(HostLifetimeError::InvalidSelection);
        };
        let mut selected = targets.iter().map(|target| Some(target.target.clone())).collect::<Vec<_>>();
        if base.is_some() || helper.catalog_capability.is_some() { selected.push(None); }
        if let Some(report) = &helper.report_capability {
            selected.push(Some(ConfiguredHelperReportCapability::target()));
            if report.account_parent.is_some() { selected.push(Some(ConfiguredHelperReportCapability::account_target())); }
            if report.public_parent.is_some() { selected.push(Some(ConfiguredHelperReportCapability::public_target())); }
        }
        selected.sort();
        let mut intents = setup_targets::helper_intents(&self.source, helper,
            &self.native.grant_administrator, &member.audience, selected)?;
        for intent in &mut intents {
            intent.requested = setup_targets::child_bounds(&self.native.grant_administrator, origin,
                &intent.target, &intent.requested)?;
            intent.requested.delegation_depth = DelegationDepth::remaining(1);
        }
        Ok(intents)
    }
}
