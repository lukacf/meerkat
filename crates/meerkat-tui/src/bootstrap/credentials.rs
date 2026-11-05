//! Credentials of the built-in provider endpoints.
//!
//! Sessions resolve their own credentials through the factory (realm auth
//! bindings, the token store, the environment). The TUI only needs them for
//! two things the engine does not do: deciding which built-in endpoints exist
//! and calling their list-models endpoints for the `/model` picker.
//! [`resolve_builtin`] answers both from upstream's own connection
//! resolution ([`resolve_auth_binding_or_default_for_provider`]): the realm's
//! binding for the provider when it has one, else the synthetic environment
//! default (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GEMINI_API_KEY` /
//! `GOOGLE_API_KEY`, each also as `RKAT_<NAME>`).

use meerkat_core::connection::{
    CredentialSourceSpec, RealmId, resolve_auth_binding_or_default_for_provider,
};
use meerkat_core::{Config, Provider};

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

/// What a built-in endpoint resolves to.
#[derive(Clone, PartialEq, Eq)]
pub struct BuiltinCredentials {
    /// The key, when the source is one the TUI can read itself (an
    /// environment variable or an inline secret). `None` for token-store,
    /// OAuth, command and external sources: sessions still resolve those
    /// through the factory, but the model list cannot be fetched.
    pub api_key: Option<String>,
    /// The binding's base URL override.
    pub base_url: Option<String>,
}

impl std::fmt::Debug for BuiltinCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BuiltinCredentials")
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .field("base_url", &self.base_url)
            .finish()
    }
}

/// The built-in endpoint of `provider`, if it exists: a binding for the
/// provider on `realm`'s chain, or the environment default when its variable
/// is set. `env` looks environment variables up.
#[must_use]
pub fn resolve_builtin(
    config: &Config,
    provider: Provider,
    realm: Option<&RealmId>,
    env: &dyn Fn(&str) -> Option<String>,
) -> Option<BuiltinCredentials> {
    let target =
        resolve_auth_binding_or_default_for_provider(config, provider, None, realm, true).ok()?;
    let api_key = match &target.auth_profile.source {
        CredentialSourceSpec::Env { env: var, fallback } => std::iter::once(var)
            .chain(fallback)
            .find_map(|name| env_value(env, name)),
        CredentialSourceSpec::InlineSecret { secret } => Some(secret.clone()),
        _ => None,
    };
    // The environment default without its variable is no endpoint at all.
    if target.auth_binding.is_env_default() && api_key.is_none() {
        return None;
    }
    Some(BuiltinCredentials {
        api_key,
        base_url: target.backend.base_url,
    })
}

/// `RKAT_<name>`, else `<name>`, ignoring blank values.
fn env_value(env: &dyn Fn(&str) -> Option<String>, name: &str) -> Option<String> {
    let prefixed = (!name.starts_with("RKAT_")).then(|| format!("RKAT_{name}"));
    prefixed
        .iter()
        .map(String::as_str)
        .chain(std::iter::once(name))
        .find_map(|key| env(key).filter(|value| !value.trim().is_empty()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
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
        let builtin = BuiltinCredentials {
            api_key: Some("sk-very-secret".to_string()),
            base_url: None,
        };
        assert!(!format!("{builtin:?}").contains("sk-very-secret"));
    }

    #[test]
    fn builtins_come_from_the_environment_default() {
        let config = Config::default();
        let env = |key: &str| match key {
            "ANTHROPIC_API_KEY" => Some("sk-ant".to_string()),
            "RKAT_GOOGLE_API_KEY" => Some("g-key".to_string()),
            "OPENAI_API_KEY" => Some("  ".to_string()),
            _ => None,
        };
        let anthropic = resolve_builtin(&config, Provider::Anthropic, None, &env).unwrap();
        assert_eq!(anthropic.api_key.as_deref(), Some("sk-ant"));
        assert!(anthropic.base_url.is_none());
        // A blank variable is no key; Gemini falls back to GOOGLE_API_KEY.
        assert!(resolve_builtin(&config, Provider::OpenAI, None, &env).is_none());
        let gemini = resolve_builtin(&config, Provider::Gemini, None, &env).unwrap();
        assert_eq!(gemini.api_key.as_deref(), Some("g-key"));
        assert!(resolve_builtin(&config, Provider::Anthropic, None, &|_| None).is_none());
    }
}
