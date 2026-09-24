//! Core-owned trait seam for model operational defaults lookup.
//!
//! This trait allows the facade/factory layer to inject model-aware
//! operational defaults (e.g., profile-derived call timeouts) into the agent
//! at build time.
//!
//! The resolver is invoked at **call time** (not build time) so that
//! hot-swapped model/provider identity is reflected in the resolved defaults.

use crate::Provider;
use std::time::Duration;

/// Resolver for model-specific operational defaults.
///
/// Implemented by the facade/factory layer using the injected
/// `ModelCatalog`/`ModelRegistry` profile lookup.
/// Injected into the agent at build time and consulted at each LLM call to resolve
/// profile-derived defaults for the current effective model/provider.
pub trait ModelOperationalDefaultsResolver: Send + Sync {
    /// Return the profile-derived default call timeout for the given model/provider,
    /// or `None` if the model is unknown or has no profiled default.
    ///
    /// The provider is the typed [`Provider`] owner: callers cross the string
    /// boundary once (at the LLM-client seam) rather than re-parsing a provider
    /// string the resolver implementation already needs typed.
    fn call_timeout_for(&self, provider: Provider, model: &str) -> Option<Duration>;
}
