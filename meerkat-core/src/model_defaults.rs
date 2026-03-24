//! Core-owned trait seam for model operational defaults lookup.
//!
//! `meerkat-core` must not depend on `meerkat-models`. This trait allows
//! the facade/factory layer to inject model-aware operational defaults
//! (e.g., profile-derived call timeouts) into the agent at build time.
//!
//! The resolver is invoked at **call time** (not build time) so that
//! hot-swapped model/provider identity is reflected in the resolved defaults.

use std::time::Duration;

/// Resolver for model-specific operational defaults.
///
/// Implemented by the facade/factory layer using `meerkat-models::profile::profile_for(...)`.
/// Injected into the agent at build time and consulted at each LLM call to resolve
/// profile-derived defaults for the current effective model/provider.
pub trait ModelOperationalDefaultsResolver: Send + Sync {
    /// Return the profile-derived default call timeout for the given model/provider,
    /// or `None` if the model is unknown or has no profiled default.
    fn call_timeout_for(&self, provider: &str, model: &str) -> Option<Duration>;
}
