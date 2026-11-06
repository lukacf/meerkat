//! The implicit `delegate` mob of a session on an endpoint-only model: the
//! engine asks for the `delegate` profile's provider, and this answers the
//! provider of the realm's `[models.<id>]` profile of the model, so the
//! engine's definition validation (otherwise catalogued models only)
//! accepts it. The member build reads that profile from the realm config.

use std::sync::Arc;

use meerkat_core::Provider;
use meerkat_mob::MobError;
use meerkat_mob_mcp::{ImplicitMobContext, ImplicitMobCustomizer};

/// The provider of the realm's current `[models.<id>]` profile of a model
/// id, if it has one (catalogued models have none).
pub type ModelProfiles = Arc<dyn Fn(&str) -> Option<Provider> + Send + Sync>;

/// Answers the delegating model's profiled provider for its implicit mob.
pub(super) struct EndpointModels {
    profiles: ModelProfiles,
}

impl EndpointModels {
    pub(super) fn new(profiles: ModelProfiles) -> Self {
        Self { profiles }
    }
}

impl ImplicitMobCustomizer for EndpointModels {
    fn delegate_provider(&self, ctx: &ImplicitMobContext) -> Result<Option<Provider>, MobError> {
        Ok((self.profiles)(&ctx.model))
    }
}
