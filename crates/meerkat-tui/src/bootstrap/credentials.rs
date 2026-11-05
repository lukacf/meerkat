//! Provider credentials for the list-models client.
//!
//! **Temporary home.** The old crate kept [`ProviderCredentials`] in
//! `bootstrap/credentials.rs` together with the credential resolution; that
//! resolution is rewritten on upstream's `auth_binding` model in P1/P4 and
//! this type moves there with it.

use meerkat_core::Provider;

/// Resolved credentials for one provider endpoint.
#[derive(Clone)]
pub struct ProviderCredentials {
    /// The provider (wire dialect) these credentials belong to.
    pub provider: Provider,
    /// API key (never logged).
    pub api_key: String,
    /// Optional base URL override.
    pub base_url: Option<String>,
}

impl std::fmt::Debug for ProviderCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderCredentials")
            .field("provider", &self.provider)
            .field("api_key", &"<redacted>")
            .field("base_url", &self.base_url)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_key_never_debugs() {
        let creds = ProviderCredentials {
            provider: Provider::Anthropic,
            api_key: "sk-very-secret".to_string(),
            base_url: None,
        };
        assert!(!format!("{creds:?}").contains("sk-very-secret"));
    }
}
