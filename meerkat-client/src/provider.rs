//! Provider resolution helpers shared across interfaces.

use meerkat_core::Provider;

/// Resolves providers from shared rules.
///
/// Phase 6.6 deleted the legacy `api_key_for` / `api_key_for_with_env`
/// credential helpers and the `client_for` factory shim. Env-backed
/// credential resolution now flows through
/// `ProviderRuntimeRegistry::resolve` via `ResolverEnvironment`, and
/// client construction through `build_client`; this struct is
/// retained only for `infer_from_model` model-prefix routing.
pub struct ProviderResolver;

impl ProviderResolver {
    /// Infer provider from a model string.
    ///
    /// Delegates to [`Provider::infer_from_model`] and falls back to
    /// `Provider::Other` when no prefix matches.
    pub fn infer_from_model(model: &str) -> Provider {
        Provider::infer_from_model(model).unwrap_or(Provider::Other)
    }
}
