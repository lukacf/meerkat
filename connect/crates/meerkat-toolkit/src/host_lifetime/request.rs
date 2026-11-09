//! Selected component/source observations only. Native authorization and
//! runtime custody are not implemented by this application lifetime owner.
use super::*;
use crate::authority::ChannelInboxObservation;
use meerkat_authorization_contracts::work_association::InputAuthorityAssociation;
use serde::Deserialize;
use std::collections::BTreeSet;

impl NativeHostLifetime {
    /// Compare exact source/target facts under the selected deployment fence.
    /// This produces no admission, grant, custody, or reusable allow verdict.
    pub fn validate_source_selection(
        &self,
        source: &ChannelInboxObservation,
        association: &InputAuthorityAssociation,
    ) -> Result<()> {
        let _deny = self.deny_fence.shared()?;
        self.check_selected()?;
        if source.source.policy_digest != self.policy_revision.as_str() {
            return Err(HostLifetimeError::Denied);
        }
        let row = self
            .policy
            .binding(&source.source, association, &source.target_identity)?;
        let (_activation, _) = self.activation.retain(self.incarnation, None)?;
        let (_component, _) = self
            .component(&row.component)?
            .retain(self.incarnation, None)?;
        self.check_surfaces(&row.required_surfaces)
    }
    fn check_surfaces(&self, required: &[String]) -> Result<()> {
        let path = self
            .selection
            .deployment_root
            .join("shared/control/deny.json");
        let overlay = if path.exists() {
            if path.is_symlink() {
                return Err(HostLifetimeError::InvalidSelection);
            }
            let bytes = std::fs::read(path).map_err(|_| HostLifetimeError::Unavailable)?;
            serde_json::from_slice::<DenyOverlay>(&bytes)
                .map_err(|_| HostLifetimeError::InvalidSelection)?
        } else {
            DenyOverlay {
                revision: 0,
                denied: vec![],
                applied_unlogged: BTreeMap::new(),
            }
        };
        overlay.validate()?;
        if required
            .iter()
            .any(|surface| overlay.denied.contains(surface))
        {
            return Err(HostLifetimeError::Denied);
        }
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DenyOverlay {
    revision: u64,
    denied: Vec<String>,
    applied_unlogged: BTreeMap<String, DenyReceipt>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DenyReceipt {
    revision: u64,
    result_digest: String,
}
impl DenyOverlay {
    fn validate(&self) -> Result<()> {
        let mut unique = BTreeSet::new();
        if self
            .denied
            .iter()
            .any(|value| !policy::valid_surface(value) || !unique.insert(value))
            || self
                .applied_unlogged
                .iter()
                .any(|(id, receipt)| id.is_empty() || receipt.result_digest.is_empty())
        {
            return Err(HostLifetimeError::InvalidSelection);
        }
        // Receipt revisions are crash evidence. The native reader neither
        // consumes nor normalizes them, and never substitutes a policy copy.
        let _ = self.revision;
        for receipt in self.applied_unlogged.values() {
            let _ = receipt.revision;
        }
        Ok(())
    }
}
