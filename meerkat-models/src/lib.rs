//! Canonical provider-model catalog and capability data for Meerkat.
//!
//! `meerkat-core` owns the catalog *vocabulary* (types) and *mechanics*
//! ([`meerkat_core::model_profile::ModelCatalog`]); this crate owns the
//! *data*: which models exist, their capability rows, per-provider defaults,
//! image-generation routes, and the cross-provider priority order.
//!
//! Consumers either call the free functions here (which delegate to the
//! canonical [`ModelCatalog`] instance) or obtain the instance via
//! [`canonical`] and pass it explicitly to core seams such as
//! `ModelRegistry::from_config`.
//!
//! The dependency direction is strict: this crate depends on `meerkat-core`,
//! never the reverse. Core must not embed provider data.

pub mod capabilities;
pub mod catalog;
pub mod profile;

// Vocabulary types re-exported from meerkat-core (the types stay there).
pub use meerkat_core::model_profile::ModelCatalog;
pub use meerkat_core::model_profile::ModelProfile;
pub use meerkat_core::model_profile::capabilities::{
    BetaHeader, BetaValue, EffortLevel, ModelCapabilities, ThinkingSupport,
};
pub use meerkat_core::model_profile::catalog::{
    CatalogEntry, ImageGenerationModelProfile, ModelTier, ProviderDefaults,
};

pub use capabilities::{all_capabilities, capabilities_for};
pub use catalog::{
    allowed_models, canonical, catalog, catalog_providers, default_image_generation_model,
    default_model, entry_for, global_default_model, image_generation_model,
    image_generation_provider_defaults, image_generation_provider_for_model, infer_provider,
    provider_defaults, provider_names, provider_priority,
};
pub use profile::{inline_video_support_for, profile_for};
