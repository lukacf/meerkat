//! One retained native registration participant across application completion.
//!
//! The preparation hook selects actual native surfaces. Once the same held
//! constructions have finished and their bundle is authenticated, the installed
//! host binds its normal registration owner here. Native keeps this same Arc;
//! attached recovery therefore consults the current installed owner.

use async_trait::async_trait;
use meerkat_core::{PreparedToolSurface, ToolConsequencePolicyRegistry, ToolDef, ToolSurfaceSelection};
use meerkat_mob::{
    MemberDestinationSelection, MemberRuntimeBuildIdentity, MemberRuntimeRegistrationHook, MobError,
};
use meerkat_runtime::meerkat_machine::SessionAgentMandate;
use meerkat_toolkit::host_lifetime::control::HostLifetimeError;
use std::sync::{Arc, OnceLock};

/// Post-selection composition on the actual native identity and destination.
/// The installed hook remains the authority for all later constructions.
pub type SelectedDestinationParticipant = Arc<dyn Fn(&MemberRuntimeBuildIdentity, &MemberDestinationSelection) -> Result<(), MobError> + Send + Sync>;

pub struct PreparedMemberRegistration {
    preparation: Arc<dyn MemberRuntimeRegistrationHook>,
    registry: Arc<ToolConsequencePolicyRegistry>,
    selected_destination: SelectedDestinationParticipant,
    installed: OnceLock<Arc<dyn MemberRuntimeRegistrationHook>>,
}

impl PreparedMemberRegistration {
    /// Ordinary completed-bundle startup already owns its installed hook. The
    /// wrapper forwards to that exact owner without opening a preparation or
    /// requiring another adoption. Expected destination remains read-only.
    pub(crate) fn from_installed(
        installed: Arc<dyn MemberRuntimeRegistrationHook>,
        registry: Arc<ToolConsequencePolicyRegistry>,
        selected_destination: SelectedDestinationParticipant,
    ) -> Self {
        Self::new(installed, registry, selected_destination)
    }

    /// Supply this same value to native member construction and later to
    /// `InstalledToolkitHost::compose_with_prepared_registration`.
    pub fn new(
        preparation: Arc<dyn MemberRuntimeRegistrationHook>,
        registry: Arc<ToolConsequencePolicyRegistry>,
        selected_destination: SelectedDestinationParticipant,
    ) -> Self {
        Self { preparation, registry, selected_destination, installed: OnceLock::new() }
    }

    /// Only final installed composition can bind the relay. The driver must
    /// join every native finish before this call; it cannot retarget a live
    /// construction by treating a preparation callback as a completed member.
    pub(super) fn adopt(
        &self,
        installed: Arc<dyn MemberRuntimeRegistrationHook>,
        registry: &Arc<ToolConsequencePolicyRegistry>,
    ) -> Result<(), HostLifetimeError> {
        if !Arc::ptr_eq(&self.registry, registry) {
            return Err(HostLifetimeError::InvalidSelection);
        }
        self.installed.set(installed).map_err(|_| HostLifetimeError::Denied)
    }

    fn current(&self) -> Arc<dyn MemberRuntimeRegistrationHook> {
        self.installed.get().unwrap_or(&self.preparation).clone()
    }
}

#[async_trait]
impl MemberRuntimeRegistrationHook for PreparedMemberRegistration {
    async fn select_destination(
        &self,
        identity: &MemberRuntimeBuildIdentity,
    ) -> Result<MemberDestinationSelection, MobError> {
        let destination = self.current().select_destination(identity).await?;
        (self.selected_destination)(identity, &destination)?;
        Ok(destination)
    }

    async fn expected_destination(
        &self,
        identity: &MemberRuntimeBuildIdentity,
    ) -> Result<MemberDestinationSelection, MobError> {
        // Actor-only recovery reuses the exact canonical bindings and their
        // already selected resource participant. A read-only expectation must
        // never qualify a replacement registration.
        self.current().expected_destination(identity).await
    }

    async fn select_agent_mandate(
        &self,
        identity: &MemberRuntimeBuildIdentity,
    ) -> Result<Option<SessionAgentMandate>, MobError> {
        self.current().select_agent_mandate(identity).await
    }

    async fn expected_agent_mandate(
        &self,
        identity: &MemberRuntimeBuildIdentity,
    ) -> Result<Option<SessionAgentMandate>, MobError> {
        self.current().expected_agent_mandate(identity).await
    }

    async fn prepare_member_surface(
        &self,
        identity: &MemberRuntimeBuildIdentity,
        surface: &PreparedToolSurface,
    ) -> Result<ToolSurfaceSelection, MobError> {
        self.current().prepare_member_surface(identity, surface).await
    }

    async fn verify_member_surface(
        &self,
        identity: &MemberRuntimeBuildIdentity,
        surface: &[ToolDef],
    ) -> Result<(), MobError> {
        self.current().verify_member_surface(identity, surface).await
    }
}
