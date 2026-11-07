//! No personal server on a member hosted elsewhere.
//!
//! A member placed on another host is built there from its portable spec,
//! which carries its profile's MCP servers, and that host satisfies their
//! credentials on its own side: a personal server there would run on the
//! host's own account, the fallback personal accounts never take. Core's
//! placed spawn shows the profile it resolved (the definition's inline
//! profile, the realm profile its role names, or the spawn's own override
//! profile) to its spawn customizers before it compiles or sends anything
//! ([`SpawnMemberCustomizer::check_placed_profile`]), on every placed spawn
//! and respawn (a respawn before it retires anything), and the revival of a
//! placed member, which sends the member's stored spec to its host again,
//! shows the profile that spec carries. The guard refuses a profile that
//! declares a server under a personal server's name, whatever its shape and
//! whichever of them asks ([`PersonalAccountRefusal::Unsupported`] with
//! [`UnsupportedPersonalServer::HostedElsewhere`]): so a member placed before
//! the server was marked personal fails its revival, broken, rather than
//! keep the server. It reads the composition's personal accounts when it is
//! asked, so it can sit in the spawn customizers of every mob a composition
//! builds from its spec and in the agent mob tools' child customizer slot,
//! for every mob an agent creates, before the composition names its
//! personal servers.

use std::sync::Arc;

use meerkat_mob::{
    MobError, Profile, SpawnCustomizationContext, SpawnMemberCustomizer, SpawnMemberSpec,
};

use super::build::PersonalAccountsSlot;
use super::{PersonalAccountRefusal, UnsupportedPersonalServer};

/// The spawn customizer [`PlacementGuard::customizer`] builds.
pub(crate) struct PlacementGuard {
    accounts: PersonalAccountsSlot,
}

impl PlacementGuard {
    /// The guard over the personal accounts `accounts` holds (or comes to
    /// hold), as a spawn customizer.
    pub(crate) fn customizer(accounts: PersonalAccountsSlot) -> Arc<dyn SpawnMemberCustomizer> {
        Arc::new(Self { accounts })
    }

    /// The refusal of a placed spawn whose resolved `profile` declares a
    /// server under a personal server's name.
    fn refusal(&self, profile: &Profile) -> Option<PersonalAccountRefusal> {
        let accounts = self.accounts.accounts()?;
        let server = profile
            .tools
            .mcp_servers
            .iter()
            .map(|server| &server.name)
            .find(|name| accounts.config().is_personal(name))?
            .clone();
        Some(PersonalAccountRefusal::Unsupported {
            server,
            reason: UnsupportedPersonalServer::HostedElsewhere,
        })
    }
}

impl SpawnMemberCustomizer for PlacementGuard {
    fn customize_spawn(
        &self,
        _ctx: &SpawnCustomizationContext,
        _spec: &mut SpawnMemberSpec,
    ) -> Result<(), MobError> {
        Ok(())
    }

    fn check_placed_profile(
        &self,
        _ctx: &SpawnCustomizationContext,
        profile: &Profile,
    ) -> Result<(), MobError> {
        match self.refusal(profile) {
            Some(refusal) => Err(MobError::WiringError(refusal.to_string())),
            None => Ok(()),
        }
    }
}
