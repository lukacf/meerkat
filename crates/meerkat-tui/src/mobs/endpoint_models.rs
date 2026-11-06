//! The implicit `delegate` mob of a session on an endpoint-only model: the
//! mob's `delegate` profile names the provider of the realm's
//! `[models.<id>]` profile of the model, so the engine's definition
//! validation (otherwise catalogued models only) accepts it. The member
//! build reads that profile from the realm config. (A `models[<id>]` entry
//! in the definition would collide with the realm's own entry at the build.)

use std::sync::Arc;

use meerkat_core::Provider;
use meerkat_mob::{MobDefinition, MobError, ProfileName};
use meerkat_mob_mcp::{ImplicitMobContext, ImplicitMobDefinitionCustomizer};

/// The provider of the realm's current `[models.<id>]` profile of a model
/// id, if it has one (catalogued models have none).
pub type ModelProfiles = Arc<dyn Fn(&str) -> Option<Provider> + Send + Sync>;

/// Names the delegating model's profiled provider on its implicit mob.
pub(super) struct EndpointModels {
    profiles: ModelProfiles,
}

impl EndpointModels {
    pub(super) fn new(profiles: ModelProfiles) -> Self {
        Self { profiles }
    }
}

impl ImplicitMobDefinitionCustomizer for EndpointModels {
    fn customize_implicit_mob(
        &self,
        ctx: &ImplicitMobContext,
        definition: &mut MobDefinition,
    ) -> Result<(), MobError> {
        let Some(provider) = (self.profiles)(&ctx.model) else {
            return Ok(());
        };
        if let Some(profile) = definition
            .profiles
            .get_mut(&ProfileName::from("delegate"))
            .and_then(meerkat_mob::ProfileBinding::as_inline_mut)
            && profile.provider.is_none()
        {
            profile.provider = Some(provider);
        }
        Ok(())
    }
}
