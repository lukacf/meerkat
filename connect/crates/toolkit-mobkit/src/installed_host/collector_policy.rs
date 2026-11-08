//! The installed collector's operation owner, separate from agent membership.
use super::*;
use crate::private_originals::collector_setup::CollectorRecord;

#[cfg(test)]
mod tests;

impl InstalledApplicationPolicy {
    pub(super) fn collector_source(
        &self,
        association: &InputAuthorityAssociation,
        purpose: LocalPolicyPurpose,
    ) -> Result<Option<CollectorRecord>, OperationAuthorizationError> {
        let Some(record) = self
            .originals
            .collector_for_policy(association)
            .map_err(|_| OperationAuthorizationError::Unavailable)?
        else {
            return Ok(None);
        };
        let claims = association.candidate();
        if purpose != LocalPolicyPurpose::Operation
            || claims.ingress_namespace.realm != self.native.realm
            || claims.ingress_namespace.ingress != self.native.ingress_domain
        {
            return Err(OperationRefused::new(OperationRefusalKind::Denied).into());
        }
        // Enrollment does not exempt the collector from the installed host's
        // admission fence. There is no agent component or borrowed session.
        self.validate_source_entry(None)?;
        Ok(Some(record))
    }

    pub(super) fn collector_operation(
        &self,
        record: &CollectorRecord,
        binding: &PreparedAuthorizationBinding,
        now_ms: u64,
    ) -> Result<LocalPolicyAllowance, OperationAuthorizationError> {
        let AuthorizationOperation::Tool(facts) = &binding.facts().operation else {
            return Err(OperationRefused::new(OperationRefusalKind::Denied).into());
        };
        let meerkat_core::authorization::ToolAuthorizationTarget::Dispatcher(plan) = &facts.target
        else {
            return Err(OperationRefused::new(OperationRefusalKind::Denied).into());
        };
        let target = meerkat_authorization::grant_policy::single_tool_operation_target(plan)?
            .ok_or(OperationAuthorizationError::Unavailable)?;
        record.check_operation(target).map_err(|_| {
            OperationAuthorizationError::from(OperationRefused::new(OperationRefusalKind::Denied))
        })?;
        let meerkat_core::ToolOperationTarget::McpServerOperation {
            server,
            raw_operation,
            connector,
            ..
        } = target
        else {
            return Err(OperationRefused::new(OperationRefusalKind::Denied).into());
        };
        let claims = &record.association;
        let authority = &claims.ingress_namespace.ingress.authority;
        Ok(LocalPolicyAllowance {
            operation_values: vec![
                application_operations::mcp_operation(
                    authority,
                    &claims.logical_executor,
                    &claims.target.audience,
                    server,
                    raw_operation,
                ),
                application_operations::mcp_connector_operation(
                    authority,
                    &claims.logical_executor,
                    &claims.target.audience,
                    server,
                    connector,
                ),
            ],
            restrictions: ExecutionRestrictions::unrestricted(),
            expires_at_ms: now_ms.saturating_add(ORDINARY_POLICY_ALLOWANCE_MS),
            review_tier: OperationReviewTier::R1,
        })
    }
}
