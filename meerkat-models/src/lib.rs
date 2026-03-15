//! Curated model catalog and provider profile rules for the Meerkat agent platform.
//!
//! This crate is the single source of truth for:
//! - Which models are supported and recommended (`catalog` module)
//! - Per-model capability detection and parameter schemas (`profile` module)
//!
//! It is a pure leaf crate with no I/O, no HTTP, and no meerkat crate dependencies.
//! Consumers include `meerkat-core` (config defaults), `meerkat-client` (adapter rules),
//! `meerkat-tools` (sub-agent validation), and the facade layer (surface endpoints).

pub mod catalog;
pub mod profile;

// Re-exports for convenience
pub use catalog::{CatalogEntry, ModelTier, ProviderDefaults};
pub use catalog::{
    allowed_models, catalog, default_model, entry_for, provider_defaults, provider_names,
};
pub use profile::{ModelProfile, profile_for};
