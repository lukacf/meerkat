//! Passive review material from the actual current installed policy.
//! No tool arguments, transcript, credential locator or saved digest supplies
//! a mandate or account. Native permission and physical entry stay separate.
use super::review_diagnostics::{Stage, observe};
use super::*;

impl InstalledApplicationPolicy {
    pub(super) fn installed_review_mandate(
        &self,
        association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        now_ms: u64,
    ) -> Result<Option<String>, OperationAuthorizationError> {
        let source = if observe(
            self.originals.review_observations(),
            Stage::MandateCollectorSource,
            self.collector_source(association, LocalPolicyPurpose::Operation),
        )?
        .is_some()
        {
            "The installed collector policy admits this retained service original for its selected operations."
        } else {
            observe(
                self.originals.review_observations(),
                Stage::MandateCurrentInvocation,
                self.current_invocation(association),
            )?;
            if observe(
                self.originals.review_observations(),
                Stage::MandateHelperSource,
                self.originals
                    .helper_policy_source(association)
                    .map_err(mcp_control_error),
            )?
            .is_some()
            {
                "The installed application selected this setup helper independently of its contributing originals."
            } else {
                let route = observe(
                    self.originals.review_observations(),
                    Stage::MandateSourceRoute,
                    self.originals
                        .route_for_binding(association)
                        .map_err(mcp_control_error),
                )?;
                let (route, _, _) = observe(
                    self.originals.review_observations(),
                    Stage::MandateSelectedSource,
                    self.originals
                        .selected_source_route(&route)
                        .map_err(mcp_control_error),
                )?;
                match route.origin {
                    meerkat_toolkit::host_lifetime::policy::CompiledNativeSourceOrigin::LocalConsole { .. } =>
                        "The installed application's current local-console source policy admits this retained work for its selected member.",
                    meerkat_toolkit::host_lifetime::policy::CompiledNativeSourceOrigin::ProviderEvent { .. } =>
                        "The installed application's current provider-event source policy admits this retained work for its selected member. Source authentication is not a person's approval of this operation.",
                }
            }
        };
        let bounds = observe(
            self.originals.review_observations(),
            Stage::MandateGrantBounds,
            self.originals
                .review_grant_bounds(association, binding, now_ms),
        )?;
        // This states the live policy's own mandate, not a reconstruction of
        // the user's request. ScopedInvocation adds current contributor bounds.
        Ok(Some(format!(
            "{source} Its own current native grant-lineage mandate, conjoined with the work's admitted ceiling, bounds it to: {bounds} This is not human consent; separate operation and contributor checks remain required."
        )))
    }

    pub(super) fn installed_review_action_account(
        &self,
        association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        now_ms: u64,
    ) -> Result<Option<String>, OperationAuthorizationError> {
        // Reobserve the exact existing local mapping without staging a call
        // or invoking a review-floor callback. Existing source-currentness
        // checks retain their fixed failure diagnostics.
        self.current_operation_allowance(
            association,
            binding,
            LocalPolicyPurpose::Operation,
            now_ms,
            true,
        )?;
        match &binding.facts().operation {
            AuthorizationOperation::Tool(facts) => {
                let meerkat_core::authorization::ToolAuthorizationTarget::Dispatcher(plan) =
                    &facts.target
                else {
                    return observe(
                        self.originals.review_observations(),
                        Stage::AccountDispatcher,
                        Err(OperationAuthorizationError::Unavailable),
                    );
                };
                let target = observe(
                    self.originals.review_observations(),
                    Stage::AccountTarget,
                    meerkat_authorization::grant_policy::single_tool_operation_target(plan)
                        .and_then(|target| target.ok_or(OperationAuthorizationError::Unavailable)),
                )?;
                observe(
                    self.originals.review_observations(),
                    Stage::AccountProjection,
                    reviewed_target_account(target),
                )
            }
            // The mapping above accepts only the owner's exact session memory.
            AuthorizationOperation::Source(_) => Ok(None),
            _ => observe(
                self.originals.review_observations(),
                Stage::AccountProjection,
                Err(OperationAuthorizationError::Unavailable),
            ),
        }
    }
}

// The caller first validates the supported installed operation. This projection
// states only the native owner's account semantics, never permission to call.
pub(super) fn reviewed_target_account(
    target: &meerkat_core::ToolOperationTarget,
) -> Result<Option<String>, OperationAuthorizationError> {
    match target {
        meerkat_core::ToolOperationTarget::CatalogControl { .. }
        | meerkat_core::ToolOperationTarget::PeerSend { .. }
        | meerkat_core::ToolOperationTarget::PeerDirectoryRead
        | meerkat_core::ToolOperationTarget::MemoryOperation { .. } => Ok(None),
        _ => match target.account() {
            Some(meerkat_core::OperationAccount::Selected(account)) => {
                Ok(Some(account.to_string()))
            }
            Some(meerkat_core::OperationAccount::DeclaredNone) => Ok(None),
            _ => Err(OperationAuthorizationError::Unavailable),
        },
    }
}
