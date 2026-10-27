//! Exact helper-session catalog scope on the existing preparation grant owner.
//! Catalog loading changes visibility; it supplies no MCP operation authority.
use super::*;
use meerkat_authorization::grants::GrantRefusal;
use meerkat_authorization_contracts::{
    constraints::{AudienceRef, DelegationDepth, ExecutionRestrictions},
    evidence::EvidenceId,
    grant::GrantLineageRef,
};
use meerkat_core::{MobMemberBinding, PrincipalRef, SessionId};

pub(super) struct PreparedHelperCatalog {
    id: EvidenceId,
    scope: PreparedHelperCatalogScope,
    grantee: PrincipalRef,
    restrictions: ExecutionRestrictions,
}

impl PreparedHelperCatalog {
    pub(super) fn select(
        source: &ToolkitOriginalOwner, application: &PrincipalRef, operator: &PrincipalRef,
        helper: &acquisition::InstalledSetupSelection, audience: &AudienceRef,
        request: Option<&PreparedHelperCatalogRequest>,
        session: Option<(&MobMemberBinding, &SessionId)>,
    ) -> Result<Option<Self>, HostLifetimeError> {
        let Some(request) = request else { return Ok(None); };
        let (member, session) = session.ok_or(HostLifetimeError::Unavailable)?;
        if member != &helper.member { return Err(HostLifetimeError::Denied); }
        acquisition::ConfiguredHelperReportCapability::validate_basis(&helper.operation_basis)?;
        let WorkAuthorityBasis::TargetedGrantLineages { base, .. } = &helper.operation_basis else {
            return Err(HostLifetimeError::InvalidSelection);
        };
        let (grantee, restrictions) = match request.scope {
            PreparedHelperCatalogScope::ExactSession => {
                if helper.catalog_capability.is_some() { return Err(HostLifetimeError::Denied); }
                let mut restrictions = application_operations::parent_restrictions(
                    application_operations::catalog_read(application, &helper.logical_executor, audience, session),
                );
                restrictions.delegation_depth = DelegationDepth::remaining(0);
                (helper.logical_executor.clone(), restrictions)
            }
            PreparedHelperCatalogScope::CurrentHelperSuccessors => {
                if base.is_some() { return Err(HostLifetimeError::Denied); }
                (application.clone(), application_operations::configured_catalog_parent(application, &helper.logical_executor, audience))
            }
        };
        let selected = Self { id: request.issuance_id.clone(), scope: request.scope, grantee, restrictions };
        // An existing base is a whole lineage, not a bag of capabilities. Never
        // union it with this root or replace an incompatible prior selection.
        if base.as_ref().is_some_and(|lineage| lineage.len() != 1
            || lineage[0].grant_id != selected.id || &lineage[0].root_authority != operator)
        { return Err(HostLifetimeError::Denied); }
        if helper.catalog_capability.as_ref().is_some_and(|capability| capability.parent.len() != 1
            || capability.parent[0].grant_id != selected.id || &capability.parent[0].root_authority != operator)
        { return Err(HostLifetimeError::Denied); }
        let current = selected.observe(source, operator, helper)?;
        if base.as_ref().is_some_and(|lineage| current.as_ref() != lineage.first()) {
            return Err(HostLifetimeError::Denied);
        }
        if helper.catalog_capability.as_ref().is_some_and(|capability| current.as_ref() != capability.parent.first()) {
            return Err(HostLifetimeError::Denied);
        }
        Ok(Some(selected))
    }

    fn observe(&self, source: &ToolkitOriginalOwner, operator: &PrincipalRef,
        helper: &acquisition::InstalledSetupSelection,
    ) -> Result<Option<GrantLineageRef>, HostLifetimeError> {
        let grants = source.grants();
        let parent = grants.issued_reference(&self.id).map_err(refusal)?;
        if let Some(parent) = &parent {
            if &parent.root_authority != operator || parent.grant_id != self.id {
                return Err(HostLifetimeError::Denied);
            }
            let current = grants.resolve_lineage(std::slice::from_ref(parent),
                &self.grantee, helper.represented_subject.as_ref()).map_err(refusal)?;
            if current.restrictions() != &self.restrictions { return Err(HostLifetimeError::Denied); }
            current.check_current().map_err(|_| HostLifetimeError::Unavailable)?;
        }
        Ok(parent)
    }

    pub(super) fn issue_or_recover(self, source: &ToolkitOriginalOwner, operator: &PrincipalRef,
        helper: &mut acquisition::InstalledSetupSelection,
    ) -> Result<(), HostLifetimeError> {
        // Native owns durable issuance. Uncertain output is recovered only by
        // the original ID, then resolved against the same exact session tuple.
        if self.observe(source, operator, helper)?.is_none() {
            source.grants().issue_root(operator, self.id.clone(), self.grantee.clone(),
                helper.represented_subject.clone(), self.restrictions.clone()).map_err(refusal)?;
        }
        let parent = self.observe(source, operator, helper)?.ok_or(HostLifetimeError::Unavailable)?;
        if self.scope == PreparedHelperCatalogScope::CurrentHelperSuccessors {
            let capability = acquisition::ConfiguredHelperCatalogCapability { parent: vec![parent] };
            capability.validate(&helper.operation_basis)?;
            if helper.catalog_capability.as_ref().is_some_and(|old| old != &capability) {
                return Err(HostLifetimeError::Denied);
            }
            helper.catalog_capability = Some(capability);
            return Ok(());
        }
        let WorkAuthorityBasis::TargetedGrantLineages { base, .. } = &mut helper.operation_basis else {
            return Err(HostLifetimeError::InvalidSelection);
        };
        match base {
            Some(lineage) if lineage == &vec![parent.clone()] => {},
            Some(_) => return Err(HostLifetimeError::Denied),
            None => *base = Some(vec![parent]),
        }
        Ok(())
    }
}

fn refusal(error: GrantRefusal) -> HostLifetimeError {
    match error {
        GrantRefusal::Unavailable => HostLifetimeError::Unavailable,
        GrantRefusal::Denied | GrantRefusal::Duplicate | GrantRefusal::ControllerInUse => HostLifetimeError::Denied,
    }
}
