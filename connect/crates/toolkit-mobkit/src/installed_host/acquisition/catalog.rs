//! Explicit host catalog parent, separate from every helper operation target.
use super::*;

#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfiguredHelperCatalogCapability {
    pub parent: Vec<GrantLineageRef>,
}

impl ConfiguredHelperCatalogCapability {
    pub(crate) fn validate(&self, basis: &WorkAuthorityBasis) -> Result<(), HostLifetimeError> {
        if !matches!(basis, WorkAuthorityBasis::TargetedGrantLineages { base: None, .. })
            || self.parent.is_empty() || self.parent.len() >= MAX_ASSOCIATION_REFERENCES
            || self.parent.iter().any(|reference| reference.validate().is_err())
        { return Err(HostLifetimeError::InvalidSelection); }
        Ok(())
    }

    pub(crate) fn project(&self, basis: &WorkAuthorityBasis, lineage: Vec<GrantLineageRef>)
        -> Result<WorkAuthorityBasis, HostLifetimeError>
    {
        self.validate(basis)?;
        if lineage.len() != self.parent.len() + 1 || !lineage.starts_with(&self.parent)
            || lineage.iter().any(|reference| reference.validate().is_err())
        { return Err(HostLifetimeError::InvalidSelection); }
        let WorkAuthorityBasis::TargetedGrantLineages { targets, .. } = basis else { unreachable!() };
        Ok(WorkAuthorityBasis::TargetedGrantLineages { base: Some(lineage), targets: targets.clone() })
    }
}
