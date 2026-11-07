//! No personal server on a member hosted elsewhere.
//!
//! A member placed on another host is built there from its portable spec,
//! which carries its profile's MCP servers, and that host satisfies their
//! credentials on its own side: a personal server there would run on the
//! host's own account, the fallback personal accounts never take. So a spawn
//! that places a member on another host is refused when its profile declares
//! a personal server ([`PersonalAccountRefusal::Unsupported`] with
//! [`UnsupportedPersonalServer::HostedElsewhere`]), and so is the rebuild of
//! such a member. The guard runs among the spawn customizers of every mob a
//! composition with personal servers builds from its definition, which Core
//! asks for every spawn and rebuild but the replay of a spec it already
//! admitted.

use std::collections::BTreeMap;
use std::sync::Arc;

use meerkat_mob::{
    MobDefinition, MobError, Profile, ProfileName, ResumedMemberView, SpawnCustomizationContext,
    SpawnMemberCustomizer, SpawnMemberSpec,
};

use super::{McpAccountsConfig, PersonalAccountRefusal, PersonalMcpAccounts};

/// The spawn customizer [`PersonalMcpAccounts::placement_guard`] builds.
pub(crate) struct PlacementGuard {
    config: Arc<McpAccountsConfig>,
    /// The first personal server each inline profile of the definition
    /// declares.
    profiles: BTreeMap<ProfileName, String>,
}

impl PersonalMcpAccounts {
    /// The guard refusing a member placed on another host whose profile, in
    /// `definition` or overridden by its spawn, declares a personal server
    /// (see the module docs).
    pub(crate) fn placement_guard(&self, definition: &MobDefinition) -> PlacementGuard {
        let profiles = definition
            .profiles
            .iter()
            .filter_map(|(name, binding)| {
                let server = first_personal_server(&self.config, binding.as_inline()?)?;
                Some((name.clone(), server))
            })
            .collect();
        PlacementGuard {
            config: Arc::clone(&self.config),
            profiles,
        }
    }
}

fn first_personal_server(config: &McpAccountsConfig, profile: &Profile) -> Option<String> {
    profile
        .tools
        .mcp_servers
        .iter()
        .map(|server| &server.name)
        .find(|name| config.is_personal(name))
        .cloned()
}

impl PlacementGuard {
    /// The refusal of `spec`, when it places a member on another host whose
    /// profile declares a personal server.
    pub(crate) fn refusal(&self, spec: &SpawnMemberSpec) -> Option<PersonalAccountRefusal> {
        let _ = (spec, &self.profiles, first_personal_server);
        None
    }

    fn check(&self, spec: &SpawnMemberSpec) -> Result<(), MobError> {
        match self.refusal(spec) {
            Some(refusal) => Err(MobError::WiringError(refusal.to_string())),
            None => Ok(()),
        }
    }
}

impl SpawnMemberCustomizer for PlacementGuard {
    fn customize_spawn(
        &self,
        _ctx: &SpawnCustomizationContext,
        spec: &mut SpawnMemberSpec,
    ) -> Result<(), MobError> {
        self.check(spec)
    }

    fn customize_resume(
        &self,
        _ctx: &SpawnCustomizationContext,
        _durable: &ResumedMemberView,
        spec: &mut SpawnMemberSpec,
    ) -> Result<(), MobError> {
        self.check(spec)
    }
}
