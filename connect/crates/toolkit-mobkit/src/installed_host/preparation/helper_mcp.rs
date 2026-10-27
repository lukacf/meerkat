//! Finite bundled-helper MCP scope on the existing preparation grant owner.
//! This contains no runtime, configuration registry or independent authority.
use super::*;
use meerkat_authorization::grants::GrantRefusal;
use meerkat_authorization_contracts::{
    constraints::{AudienceRef, ExecutionRestrictions},
    evidence::EvidenceId,
    grant::GrantLineageRef,
    work_association::{GrantTarget, TargetGrantLineage},
};
use meerkat_core::{McpServerConfig, PrincipalRef, mcp_config::McpConnectorApproval};

pub(super) struct PreparedHelperMcp {
    id: EvidenceId,
    target: GrantTarget,
    restrictions: ExecutionRestrictions,
}

impl PreparedHelperMcp {
    pub(super) fn select(
        source: &ToolkitOriginalOwner, application: &PrincipalRef, operator: &PrincipalRef,
        helper: &acquisition::InstalledSetupSelection, audience: &AudienceRef,
        request: Option<&PreparedHelperMcpRequest>, server: Option<&McpServerConfig>, report_slots: usize,
    ) -> Result<Option<Self>, HostLifetimeError> {
        let Some(request) = request else { return Ok(None); };
        let server = server.ok_or(HostLifetimeError::Unavailable)?;
        let access = server.access.as_ref().ok_or(HostLifetimeError::Denied)?;
        if server.name.trim().is_empty() || server.name.trim() != server.name
            || server.connector.is_some() || access.allowed_operations.is_empty()
            || access.allowed_operations.iter().any(|name| name.trim().is_empty() || name.trim() != name)
        { return Err(HostLifetimeError::Denied); }
        // This path is for the compiler's bundled no-credential setup server.
        // A provider connector uses the existing explicit service-access owner.
        let selected = Self {
            id: request.issuance_id.clone(),
            target: GrantTarget::McpServer { server: server.name.clone() },
            restrictions: application_operations::mcp_access_restrictions(
                application, &helper.logical_executor, audience, &server.name,
                access.allowed_operations.iter().map(String::as_str), &McpConnectorApproval::NoConnector,
            ),
        };
        acquisition::ConfiguredHelperReportCapability::validate_basis(&helper.operation_basis)?;
        let WorkAuthorityBasis::TargetedGrantLineages { targets, .. } = &helper.operation_basis else {
            return Err(HostLifetimeError::InvalidSelection);
        };
        let existing = targets.iter().find(|row| row.target == selected.target);
        // Explicitly selected existing access is never broadened, replaced or
        // normalized. Reusing this request requires this exact one-root scope.
        if existing.is_some_and(|row| row.lineage.len() != 1
            || row.lineage[0].grant_id != selected.id || &row.lineage[0].root_authority != operator)
        { return Err(HostLifetimeError::Denied); }
        let current = selected.observe(source, operator, helper)?;
        if existing.is_some_and(|row| current.as_ref() != row.lineage.first()) {
            return Err(HostLifetimeError::Denied);
        }
        // Inserting one exact target preserves every unrelated base/target.
        // Capacity is checked without inventing a reference before issuance.
        let final_targets = targets.len() + usize::from(existing.is_none());
        let maximum = meerkat_authorization_contracts::work_association::MAX_ASSOCIATION_REFERENCES;
        if final_targets >= maximum || final_targets + report_slots > maximum {
            return Err(HostLifetimeError::InvalidSelection);
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
                &helper.logical_executor, helper.represented_subject.as_ref()).map_err(refusal)?;
            if current.restrictions() != &self.restrictions { return Err(HostLifetimeError::Denied); }
            current.check_current().map_err(|_| HostLifetimeError::Unavailable)?;
        }
        Ok(parent)
    }

    pub(super) fn issue_or_recover(self, source: &ToolkitOriginalOwner, operator: &PrincipalRef,
        helper: &mut acquisition::InstalledSetupSelection,
    ) -> Result<(), HostLifetimeError> {
        // The native issuer owns durability and duplicate identity. A lost
        // reply always observes this same ID, never an alternate grant owner.
        if self.observe(source, operator, helper)?.is_none() {
            source.grants().issue_root(operator, self.id.clone(), helper.logical_executor.clone(),
                helper.represented_subject.clone(), self.restrictions.clone()).map_err(refusal)?;
        }
        let parent = self.observe(source, operator, helper)?.ok_or(HostLifetimeError::Unavailable)?;
        let WorkAuthorityBasis::TargetedGrantLineages { targets, .. } = &mut helper.operation_basis else {
            return Err(HostLifetimeError::InvalidSelection);
        };
        match targets.binary_search_by(|row| row.target.cmp(&self.target)) {
            Ok(at) if targets[at].lineage == vec![parent.clone()] => {},
            Ok(_) => return Err(HostLifetimeError::Denied),
            Err(at) => targets.insert(at, TargetGrantLineage { target: self.target, lineage: vec![parent] }),
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
