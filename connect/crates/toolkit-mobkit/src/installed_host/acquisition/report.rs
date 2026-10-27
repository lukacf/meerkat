//! Trusted selection of a separate report parent. Native issuance and every
//! report invocation still validate actual authority, source and currentness.
use super::*;
use meerkat_authorization_contracts::work_association::{GrantTarget, TargetGrantLineage};

#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfiguredHelperReportCapability {
    pub parent: Vec<GrantLineageRef>,
    /// Independently issued one-action parent for the protected account request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_parent: Option<Vec<GrantLineageRef>>,
    /// Independently issued one-action parent for the public service request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_parent: Option<Vec<GrantLineageRef>>,
}

impl ConfiguredHelperReportCapability {
    pub(crate) fn target() -> GrantTarget {
        GrantTarget::RegisteredToolBundleOperation {
            bundle: meerkat_toolkit::setup::SETUP_RESULTS_BUNDLE.into(),
            operation: meerkat_toolkit::setup::SETUP_REPORT.into(),
        }
    }

    pub(crate) fn account_target() -> GrantTarget {
        GrantTarget::RegisteredToolBundleOperation {
            bundle: meerkat_toolkit::setup::SETUP_ACCOUNTS_BUNDLE.into(),
            operation: meerkat_toolkit::setup::SETUP_ACCOUNT_REQUEST.into(),
        }
    }

    pub(crate) fn public_target() -> GrantTarget {
        GrantTarget::RegisteredToolBundleOperation {
            bundle: meerkat_toolkit::setup::SETUP_ACCOUNTS_BUNDLE.into(),
            operation: meerkat_toolkit::setup::SETUP_PUBLIC_SERVICE_REQUEST.into(),
        }
    }

    pub(crate) fn validate_basis(basis: &WorkAuthorityBasis) -> Result<(), HostLifetimeError> {
        let WorkAuthorityBasis::TargetedGrantLineages { targets, .. } = basis else {
            return Err(HostLifetimeError::InvalidSelection);
        };
        // Reserve one slot for the exact report target. Other targets and the
        // catalog base remain separate. Never normalize conflicting input.
        if targets.len() >= MAX_ASSOCIATION_REFERENCES
            || targets.windows(2).any(|pair| pair[0].target >= pair[1].target)
            || targets.iter().any(|selected| selected.target == Self::target() || match &selected.target {
                GrantTarget::McpServer { server } => server.is_empty(),
                GrantTarget::RegisteredToolBundleOperation { bundle, operation } => bundle.is_empty() || operation.is_empty(),
                _ => true,
            })
            || basis.grant_lineages().any(|lineage| lineage.is_empty()
                || lineage.len() > MAX_ASSOCIATION_REFERENCES
                || lineage.iter().any(|reference| reference.validate().is_err()))
        { return Err(HostLifetimeError::InvalidSelection); }
        Ok(())
    }

    pub(crate) fn validate(&self, basis: &WorkAuthorityBasis) -> Result<(), HostLifetimeError> {
        if self.parent.is_empty() || self.parent.len() >= MAX_ASSOCIATION_REFERENCES
            || self.parent.iter().any(|reference| reference.validate().is_err())
        { return Err(HostLifetimeError::InvalidSelection); }
        Self::validate_basis(basis)?;
        let WorkAuthorityBasis::TargetedGrantLineages { targets, .. } = basis else { unreachable!() };
        let optional = [(&self.account_parent, Self::account_target()),
            (&self.public_parent, Self::public_target())];
        if targets.len() + 1 + optional.iter().filter(|(parent, _)| parent.is_some()).count()
            > MAX_ASSOCIATION_REFERENCES
            || self.account_parent.as_ref().is_some_and(|parent| self.public_parent.as_ref() == Some(parent))
        { return Err(HostLifetimeError::InvalidSelection); }
        for (parent, target) in optional {
            if let Some(parent) = parent {
                if parent.is_empty() || parent.len() >= MAX_ASSOCIATION_REFERENCES
                    || parent.iter().any(|reference| reference.validate().is_err())
                    || parent == &self.parent
                    || targets.iter().any(|selected| selected.target == target)
                { return Err(HostLifetimeError::InvalidSelection); }
            }
        }
        Ok(())
    }

    /// Select only this bundle operation. Preserve every unrelated selection,
    /// including the optional catalog base, and reject an occupied target.
    pub(crate) fn project(&self, basis: &WorkAuthorityBasis, lineage: Vec<GrantLineageRef>)
        -> Result<WorkAuthorityBasis, HostLifetimeError>
    {
        self.project_with_account(basis, lineage, None)
    }

    pub(crate) fn project_with_account(&self, basis: &WorkAuthorityBasis,
        report: Vec<GrantLineageRef>, account: Option<Vec<GrantLineageRef>>)
        -> Result<WorkAuthorityBasis, HostLifetimeError>
    {
        self.project_with_operations(basis, report, account, None)
    }

    pub(crate) fn project_with_operations(&self, basis: &WorkAuthorityBasis,
        report: Vec<GrantLineageRef>, account: Option<Vec<GrantLineageRef>>,
        public: Option<Vec<GrantLineageRef>>)
        -> Result<WorkAuthorityBasis, HostLifetimeError>
    {
        self.validate(basis)?;
        let WorkAuthorityBasis::TargetedGrantLineages { base, targets } = basis else { unreachable!() };
        let mut targets = targets.clone();
        insert_child(&mut targets, Self::target(), &self.parent, report)?;
        match (&self.account_parent, account) {
            (Some(parent), Some(lineage)) => insert_child(&mut targets, Self::account_target(), parent, lineage)?,
            (None, None) => {},
            _ => return Err(HostLifetimeError::InvalidSelection),
        }
        match (&self.public_parent, public) {
            (Some(parent), Some(lineage)) => insert_child(&mut targets, Self::public_target(), parent, lineage)?,
            (None, None) => {},
            _ => return Err(HostLifetimeError::InvalidSelection),
        }
        Ok(WorkAuthorityBasis::TargetedGrantLineages { base: base.clone(), targets })
    }

    pub(crate) fn selected_public_lineage(basis: &WorkAuthorityBasis) -> Result<&[GrantLineageRef], HostLifetimeError> {
        if !matches!(basis, WorkAuthorityBasis::TargetedGrantLineages { .. }) {
            return Err(HostLifetimeError::InvalidSelection);
        }
        basis.lineage_for(Some(&Self::public_target())).ok_or(HostLifetimeError::InvalidSelection)
    }

    pub(crate) fn selected_account_lineage(basis: &WorkAuthorityBasis) -> Result<&[GrantLineageRef], HostLifetimeError> {
        if !matches!(basis, WorkAuthorityBasis::TargetedGrantLineages { .. }) {
            return Err(HostLifetimeError::InvalidSelection);
        }
        basis.lineage_for(Some(&Self::account_target())).ok_or(HostLifetimeError::InvalidSelection)
    }

    pub(crate) fn selected_lineage(basis: &WorkAuthorityBasis) -> Result<&[GrantLineageRef], HostLifetimeError> {
        if !matches!(basis, WorkAuthorityBasis::TargetedGrantLineages { .. }) {
            return Err(HostLifetimeError::InvalidSelection);
        }
        basis.lineage_for(Some(&Self::target())).ok_or(HostLifetimeError::InvalidSelection)
    }
}

fn insert_child(targets: &mut Vec<TargetGrantLineage>, target: GrantTarget,
    parent: &[GrantLineageRef], lineage: Vec<GrantLineageRef>) -> Result<(), HostLifetimeError>
{
    if lineage.len() != parent.len() + 1 || !lineage.starts_with(parent)
        || lineage.iter().any(|reference| reference.validate().is_err())
    { return Err(HostLifetimeError::InvalidSelection); }
    let index = targets.binary_search_by(|selected| selected.target.cmp(&target))
        .err().ok_or(HostLifetimeError::InvalidSelection)?;
    targets.insert(index, TargetGrantLineage { target, lineage });
    Ok(())
}
