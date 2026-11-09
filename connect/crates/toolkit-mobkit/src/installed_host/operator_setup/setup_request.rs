//! Exact setup-only review on the existing operator decision owner.
use super::*;
use crate::private_originals::{LocalControllerSetup, PrivateSetupRequestSelection};
use meerkat_toolkit::host_lifetime::policy::CompiledNativeSourceOrigin;

/// A proposal retains an observation of the actual prepared caller, never an
/// authority snapshot. Every use reopens the same live source/member owner.
#[derive(Clone)]
pub(super) struct ReviewedSetupCaller {
    source: std::sync::Weak<crate::private_originals::PrivateOriginals>,
    original: crate::private_originals::PrivateOriginalRef,
    session: meerkat_core::SessionId,
    route: meerkat_toolkit::host_lifetime::policy::CompiledNativeSourceRoute,
    pub(super) member: meerkat_toolkit::host_lifetime::policy::PreparedNativeMember,
    context: Option<EvidenceId>,
    admission: Option<crate::installed_host::dynamic_members::MemberClassAdmission>,
}

impl ReviewedSetupCaller {
    fn capture(
        owner: &OperatorSetup,
        prepared: &LocalControllerSetup,
    ) -> Result<Self, HostLifetimeError> {
        let source = prepared
            .source_owner()
            .ok_or(HostLifetimeError::Unavailable)?;
        source.check_setup_owner(&owner.lifetime, &owner.source)?;
        let (route, member, context) = source
            .selected_source_route(&prepared.original().route)
            .map_err(setup_source_error)?;
        let admission =
            source.selected_setup_admission(&(route.clone(), member.clone(), context.clone()))?;
        let caller = Self {
            source: Arc::downgrade(&source),
            original: prepared.original().clone(),
            session: prepared.session_id().clone(),
            route,
            member,
            context,
            admission,
        };
        caller.check_current(owner)?;
        Ok(caller)
    }

    pub(super) fn check_current(&self, owner: &OperatorSetup) -> Result<(), HostLifetimeError> {
        let source = self
            .source
            .upgrade()
            .ok_or(HostLifetimeError::Unavailable)?;
        source.check_setup_owner(&owner.lifetime, &owner.source)?;
        let (route, member, context) = source
            .selected_source_route(&self.original.route)
            .map_err(setup_source_error)?;
        if route != self.route
            || member != self.member
            || context != self.context
            || context
                .as_ref()
                .is_some_and(|id| id.as_str() != self.session.to_string())
            || !matches!(
                &route.origin,
                CompiledNativeSourceOrigin::LocalConsole { .. }
            )
        {
            return Err(HostLifetimeError::Denied);
        }
        if source.selected_setup_admission(&(route, member, context))? != self.admission {
            return Err(HostLifetimeError::Denied);
        }
        Ok(())
    }

    pub(super) fn route(
        &self,
    ) -> &meerkat_toolkit::host_lifetime::policy::CompiledNativeSourceRoute {
        &self.route
    }

    pub(super) async fn with_current<T>(
        &self,
        owner: &OperatorSetup,
        effect: impl FnOnce() -> Result<T, HostLifetimeError>,
    ) -> Result<T, HostLifetimeError> {
        self.check_current(owner)?;
        let source = self
            .source
            .upgrade()
            .ok_or(HostLifetimeError::Unavailable)?;
        source
            .with_selected_setup_caller(
                &(
                    self.route.clone(),
                    self.member.clone(),
                    self.context.clone(),
                ),
                self.admission.as_ref(),
                || {
                    self.check_current(owner)?;
                    effect()
                },
            )
            .await
    }
}

pub(super) fn setup_source_error(
    error: crate::private_originals::PrivateOriginalError,
) -> HostLifetimeError {
    match error {
        crate::private_originals::PrivateOriginalError::Unavailable => {
            HostLifetimeError::Unavailable
        }
        _ => HostLifetimeError::Denied,
    }
}

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
        {
            return Err(HostLifetimeError::WrongOwner);
        }
        let caller = ReviewedSetupCaller::capture(self, prepared)?;
        let mut proposal = self.prepare_setup_parent_review_for(
            prepared.original().route.clone(),
            prepared.session_id(),
            request,
            Some(caller.clone()),
        )?;
        let origin = crate::original_ingress::helper_originals::HelperSetupOrigin {
            realm: self.native.realm.clone(),
            source_owner: self.native.ingress_domain.clone(),
            original_authority: caller.route.source_authority.clone(),
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
            proposal
                .request
                .proposed_action
                .body
                .as_mut()
                .and_then(serde_json::Value::as_object_mut)
                .ok_or(HostLifetimeError::InvalidSelection)?
                .insert(
                    "correlated_helper_targets".into(),
                    serde_json::to_value(intent)
                        .map_err(|_| HostLifetimeError::InvalidSelection)?,
                );
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
    originals: &crate::private_originals::PrivateOriginals,
) -> Result<ExecutionRestrictions, HostLifetimeError> {
    validate_selection(lifetime, source, store, selection, Some(originals), true).await
}

/// Same bound decision and current caller, without requiring every unrelated
/// target to remain current. Each actual target operation owns that check.
pub(crate) async fn validate_bound_setup_selection(
    lifetime: &NativeHostLifetime,
    source: &ToolkitOriginalOwner,
    store: &dyn ToolkitStore,
    selection: &PrivateSetupRequestSelection,
    originals: &crate::private_originals::PrivateOriginals,
) -> Result<ExecutionRestrictions, HostLifetimeError> {
    validate_selection(lifetime, source, store, selection, Some(originals), false).await
}

/// Reopen the same canonical approved selection without resolving unrelated
/// target currentness. The caller must already have its exact bound source.
pub(crate) async fn validate_retained_setup_selection(
    lifetime: &NativeHostLifetime,
    source: &ToolkitOriginalOwner,
    store: &dyn ToolkitStore,
    selection: &PrivateSetupRequestSelection,
) -> Result<ExecutionRestrictions, HostLifetimeError> {
    validate_selection(lifetime, source, store, selection, None, false).await
}

async fn validate_selection(
    lifetime: &NativeHostLifetime,
    source: &ToolkitOriginalOwner,
    store: &dyn ToolkitStore,
    selection: &PrivateSetupRequestSelection,
    originals: Option<&crate::private_originals::PrivateOriginals>,
    resolve_parents: bool,
) -> Result<ExecutionRestrictions, HostLifetimeError> {
    lifetime.validate_current_selection()?;
    let policy = lifetime.selected_policy();
    let native = policy
        .native_host
        .as_ref()
        .ok_or(HostLifetimeError::Unavailable)?;
    let current = originals
        .map(|owner| {
            owner
                .selected_source_route(&selection.original.route)
                .map_err(setup_source_error)
        })
        .transpose()?;
    let admission = match (originals, current.as_ref()) {
        (Some(owner), Some(selected)) => owner.selected_setup_admission(selected)?,
        _ => None,
    };
    let route = if let Some((route, _, context)) = &current {
        if context
            .as_ref()
            .is_some_and(|id| id.as_str() != selection.session_id.to_string())
        {
            return Err(HostLifetimeError::Denied);
        }
        route
    } else {
        native
            .routes
            .iter()
            .find(|route| route.id == selection.original.route)
            .ok_or(HostLifetimeError::Denied)?
    };
    if selection.parent.len() != 1
        || !matches!(
            route.origin,
            CompiledNativeSourceOrigin::LocalConsole { .. }
        )
    {
        return Err(HostLifetimeError::Denied);
    }
    let (member, _, _) = route.source_binding(policy)?;
    let audience = if let Some((_, selected, _)) = &current {
        &selected.audience
    } else {
        &native
            .members
            .iter()
            .find(|selected| &selected.member == member)
            .ok_or(HostLifetimeError::InvalidSelection)?
            .audience
    };
    let expected =
        application_operations::parent_restrictions(application_operations::find_and_install(
            &native.grant_administrator,
            &route.logical_executor,
            audience,
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
    if !super::setup_recovery::decision_key_matches_for(
        lifetime,
        native,
        &key,
        &record,
        Some(route),
    )? || record.route != selection.original.route
        || record.purpose != OperatorParentPurpose::SetupRequest
        || !record.setup_request.as_ref().is_some_and(|intent| {
            intent.session_id == selection.session_id
                && intent.original.as_ref() == Some(&selection.original)
                && (intent.targets.is_empty() && intent.origin.is_none()
                    || intent.origin.as_ref().is_some_and(|origin| {
                        origin.realm == native.realm
                            && origin.source_owner == native.ingress_domain
                            && origin.original_authority == route.source_authority
                            && origin.original == selection.original
                    }))
        })
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
    if resolve_parents {
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
            let current = source
                .grants()
                .resolve_lineage(
                    std::slice::from_ref(&target.parent),
                    &native.grant_administrator,
                    route.represented_subject.as_ref(),
                )
                .map_err(|_| HostLifetimeError::Denied)?;
            if current.restrictions() != &target.restrictions {
                return Err(HostLifetimeError::Denied);
            }
            current
                .check_current()
                .map_err(|_| HostLifetimeError::Unavailable)?;
        }
    }
    lifetime.validate_current_selection()?;
    if let Some(owner) = originals {
        if Some(
            owner
                .selected_source_route(&selection.original.route)
                .map_err(setup_source_error)?,
        ) != current
        {
            return Err(HostLifetimeError::Denied);
        }
        if let Some(selected) = &current
            && owner.selected_setup_admission(selected)? != admission
        {
            return Err(HostLifetimeError::Denied);
        }
    }
    Ok(expected)
}

impl OperatorSetup {
    fn original_helper_targets(
        &self,
        origin: &crate::original_ingress::helper_originals::HelperSetupOrigin,
    ) -> Result<Vec<setup_targets::SetupTargetIntent>, HostLifetimeError> {
        use crate::installed_host::acquisition::ConfiguredHelperReportCapability;
        use meerkat_authorization_contracts::constraints::DelegationDepth;
        use meerkat_authorization_contracts::work_association::WorkAuthorityBasis;
        let Some(helper) = &self.helper_selection else {
            return Ok(vec![]);
        };
        let member = helper.selected_member(&self.native)?;
        let WorkAuthorityBasis::TargetedGrantLineages { base, targets } = &helper.operation_basis
        else {
            return Err(HostLifetimeError::InvalidSelection);
        };
        let mut selected = targets
            .iter()
            .map(|target| Some(target.target.clone()))
            .collect::<Vec<_>>();
        if base.is_some() || helper.catalog_capability.is_some() {
            selected.push(None);
        }
        if let Some(report) = &helper.report_capability {
            selected.push(Some(ConfiguredHelperReportCapability::target()));
            if report.account_parent.is_some() {
                selected.push(Some(ConfiguredHelperReportCapability::account_target()));
            }
            if report.public_parent.is_some() {
                selected.push(Some(ConfiguredHelperReportCapability::public_target()));
            }
        }
        selected.sort();
        let mut intents = setup_targets::helper_intents(
            &self.source,
            helper,
            &self.native.grant_administrator,
            &member.audience,
            selected,
        )?;
        for intent in &mut intents {
            intent.requested = setup_targets::child_bounds(
                &self.native.grant_administrator,
                origin,
                &intent.target,
                &intent.requested,
            )?;
            intent.requested.delegation_depth = DelegationDepth::remaining(1);
        }
        Ok(intents)
    }
}
