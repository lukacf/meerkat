//! meerkat-models — shim re-exports.
//!
//! All model catalog / capability / profile data moved to
//! `meerkat_core::model_profile` in the B2 split (2026-04-18). This crate
//! is retained as a thin shim so that downstream `meerkat_models::*`
//! imports continue to work without an import sweep.

pub use meerkat_core::model_profile as profile;
pub use meerkat_core::model_profile::capabilities;
pub use meerkat_core::model_profile::catalog;

pub use meerkat_core::model_profile::capabilities::{
    BetaHeader, BetaValue, ModelBetaFeature, ModelCapabilities, ModelEffortLevel, ThinkingSupport,
    capabilities_for,
};
pub use meerkat_core::model_profile::catalog::{
    CatalogEntry, ModelTier, ProviderDefaults, allowed_models, catalog, default_model, entry_for,
    provider_defaults, providers,
};
pub use meerkat_core::model_profile::{ModelProfile, profile_for};
