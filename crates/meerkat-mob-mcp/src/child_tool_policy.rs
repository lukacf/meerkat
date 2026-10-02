//! The application tool policy for members of mobs that callers of the mob
//! tools create (agent `mob_create` and public `meerkat_mob_create`).
//!
//! The binding is the host's explicit choice and is never caller-settable. A
//! host that runs managed (a consequence-policy registry is installed) must
//! choose one, possibly an explicit `Unmanaged`; otherwise a constrained
//! member could create a child mob whose members run unconstrained. Without
//! that choice, creating a child mob and spawning into one are refused.

use std::sync::Arc;

use meerkat_core::{ApplicationToolPolicyBinding, ToolConsequencePolicyRegistry};
use meerkat_mob::{MobError, SpawnCustomizationContext, SpawnMemberCustomizer, SpawnMemberSpec};

/// Why child mob creation or a child spawn was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ChildToolPolicyRefused {
    /// The host runs managed but chose no child policy.
    #[error(
        "child mob members need a host-configured application tool policy. Configure one with \
         MobMcpState::with_child_application_tool_policy(binding) (MobKit hosts: the \
         `child_application_tool_policy` init parameter, proposed for MobKit 0.8.46), or \
         explicitly choose ApplicationToolPolicyBinding::Unmanaged ({{\"kind\":\"unmanaged\"}}) \
         to allow unconstrained child members"
    )]
    PolicyRequired,
    /// The child policy names a provider but no registry is installed.
    #[error(
        "the child application tool policy names a provider, but no tool consequence policy \
         registry is installed (MobMcpState::with_tool_consequence_policy_registry)"
    )]
    RegistryMissing,
    /// `Inherit` has no member to inherit from at a child mob's creation.
    #[error(
        "ApplicationToolPolicyBinding::Inherit is not a valid child application tool policy; \
         choose a provider binding or Unmanaged"
    )]
    InheritNotAllowed,
}

/// Resolve the binding every child member is built with.
pub(crate) fn resolve_child_policy(
    registry: Option<&Arc<ToolConsequencePolicyRegistry>>,
    configured: Option<&ApplicationToolPolicyBinding>,
) -> Result<ApplicationToolPolicyBinding, ChildToolPolicyRefused> {
    match (registry, configured) {
        (_, Some(ApplicationToolPolicyBinding::Inherit)) => {
            Err(ChildToolPolicyRefused::InheritNotAllowed)
        }
        (None, Some(ApplicationToolPolicyBinding::Provider { .. })) => {
            Err(ChildToolPolicyRefused::RegistryMissing)
        }
        (_, Some(binding)) => Ok(binding.clone()),
        // The one decision point for a managed host without a child policy:
        // refuse until configured. A warn-only variant would return
        // `Ok(ApplicationToolPolicyBinding::Unmanaged)` here instead.
        (Some(_), None) => Err(ChildToolPolicyRefused::PolicyRequired),
        (None, None) => Ok(ApplicationToolPolicyBinding::Unmanaged),
    }
}

/// Installed on every child mob builder. Every spawn into the child mob is
/// built with the host's child policy, or refused when there is none.
pub(crate) struct ChildPolicyCustomizer(
    pub(crate) Result<ApplicationToolPolicyBinding, ChildToolPolicyRefused>,
);

impl SpawnMemberCustomizer for ChildPolicyCustomizer {
    fn customize_spawn(
        &self,
        _ctx: &SpawnCustomizationContext,
        spec: &mut SpawnMemberSpec,
    ) -> Result<(), MobError> {
        match &self.0 {
            Ok(binding) => {
                spec.application_tool_policy = binding.clone();
                Ok(())
            }
            Err(refusal) => Err(MobError::WiringError(refusal.to_string())),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn provider() -> ApplicationToolPolicyBinding {
        ApplicationToolPolicyBinding::Provider {
            provider_id: meerkat_core::PolicyProviderId::new("host").unwrap(),
            policy_id: meerkat_core::PolicyId::new("child-tools").unwrap(),
        }
    }

    fn registry() -> Arc<ToolConsequencePolicyRegistry> {
        Arc::new(
            ToolConsequencePolicyRegistry::new(
                Vec::new(),
                meerkat_core::PolicyEvaluationSupervisorConfig::default(),
                None,
            )
            .unwrap(),
        )
    }

    #[test]
    fn resolution_never_defaults_a_managed_host_to_unmanaged() {
        let registry = registry();
        assert_eq!(
            resolve_child_policy(None, None),
            Ok(ApplicationToolPolicyBinding::Unmanaged)
        );
        assert_eq!(
            resolve_child_policy(Some(&registry), None),
            Err(ChildToolPolicyRefused::PolicyRequired)
        );
        assert_eq!(
            resolve_child_policy(
                Some(&registry),
                Some(&ApplicationToolPolicyBinding::Unmanaged)
            ),
            Ok(ApplicationToolPolicyBinding::Unmanaged)
        );
        assert_eq!(
            resolve_child_policy(Some(&registry), Some(&provider())),
            Ok(provider())
        );
        assert_eq!(
            resolve_child_policy(None, Some(&provider())),
            Err(ChildToolPolicyRefused::RegistryMissing)
        );
        assert_eq!(
            resolve_child_policy(None, Some(&ApplicationToolPolicyBinding::Inherit)),
            Err(ChildToolPolicyRefused::InheritNotAllowed)
        );
    }
}
