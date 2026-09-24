//! Data-backed model profile lookups over the canonical catalog.
//!
//! The projection from capability rows to [`ModelProfile`] is pure mechanics
//! owned by `meerkat-core` (`project_to_profile` / `ModelCatalog::profile_for`);
//! these functions bind it to this crate's canonical data.

use crate::catalog::canonical;
use meerkat_core::Provider;
use meerkat_core::model_profile::ModelProfile;

/// Look up the profile for a model by typed provider and model ID.
///
/// Catalog models project directly from their capability row. Uncatalogued
/// model IDs return `None`; semantic capability facts must come from the
/// capability catalog, not model-name prefixes.
pub fn profile_for(provider: Provider, model: &str) -> Option<ModelProfile> {
    canonical().profile_for(provider, model)
}

/// Look up whether a model accepts inline video by typed provider and model ID.
///
/// Returns `None` when the provider/model pair has no capability row.
pub fn inline_video_support_for(provider: Provider, model: &str) -> Option<bool> {
    canonical().inline_video_support_for(provider, model)
}
