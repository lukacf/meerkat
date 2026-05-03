//! Typed provider runtime catalog.
//!
//! This module is the single backend/auth compatibility authority for the
//! runtime registry. Provider crates receive `ValidatedBinding` values after
//! this catalog has normalized profile strings into typed provider enums.

use std::sync::Arc;

use meerkat_core::provider_matrix::{
    AnthropicAuthMethod, AnthropicBackendKind, GoogleAuthMethod, GoogleBackendKind,
    OpenAiAuthMethod, OpenAiBackendKind, SelfHostedAuthMethod, SelfHostedBackendKind,
};
use meerkat_core::{AuthProfile, BackendProfile, BindingPolicy, ConnectionRef, Provider};

use crate::provider_runtime::binding::{
    NormalizedAuthMethod, NormalizedBackendKind, ValidatedBinding,
};
use crate::provider_runtime::errors::ProviderBindingError;

pub struct ProviderRuntimeCatalog;

impl ProviderRuntimeCatalog {
    pub fn is_supported_provider(provider: Provider) -> bool {
        matches!(
            provider,
            Provider::Anthropic | Provider::OpenAI | Provider::Gemini | Provider::SelfHosted
        )
    }

    pub fn validate_binding(
        connection_ref: &ConnectionRef,
        backend: &BackendProfile,
        auth: &AuthProfile,
        policy: &BindingPolicy,
    ) -> Result<ValidatedBinding, ProviderBindingError> {
        if backend.provider != auth.provider {
            return Err(ProviderBindingError::ProviderMismatch);
        }
        Self::validate_binding_for_provider(backend.provider, connection_ref, backend, auth, policy)
    }

    pub fn validate_binding_for_provider(
        provider: Provider,
        connection_ref: &ConnectionRef,
        backend: &BackendProfile,
        auth: &AuthProfile,
        policy: &BindingPolicy,
    ) -> Result<ValidatedBinding, ProviderBindingError> {
        if backend.provider != provider || auth.provider != provider {
            return Err(ProviderBindingError::ProviderMismatch);
        }

        let backend_kind = Self::normalize_backend(provider, &backend.backend_kind)?;
        let auth_method = Self::normalize_auth(provider, &auth.auth_method)?;
        if !Self::supports(backend_kind, auth_method) {
            return Err(ProviderBindingError::UnsupportedCombination {
                backend: backend.backend_kind.clone(),
                auth: auth.auth_method.clone(),
            });
        }

        Ok(ValidatedBinding {
            connection_ref: connection_ref.clone(),
            provider,
            backend: backend_kind,
            auth: auth_method,
            backend_profile: Arc::new(backend.clone()),
            auth_profile: Arc::new(auth.clone()),
            policy: policy.clone(),
        })
    }

    pub fn normalize_backend(
        provider: Provider,
        raw: &str,
    ) -> Result<NormalizedBackendKind, ProviderBindingError> {
        match provider {
            Provider::Anthropic => AnthropicBackendKind::parse(raw)
                .map(NormalizedBackendKind::Anthropic)
                .ok_or_else(|| ProviderBindingError::UnknownBackendKind(raw.to_string())),
            Provider::OpenAI => OpenAiBackendKind::parse(raw)
                .map(NormalizedBackendKind::OpenAi)
                .ok_or_else(|| ProviderBindingError::UnknownBackendKind(raw.to_string())),
            Provider::Gemini => GoogleBackendKind::parse(raw)
                .map(NormalizedBackendKind::Google)
                .ok_or_else(|| ProviderBindingError::UnknownBackendKind(raw.to_string())),
            Provider::SelfHosted => SelfHostedBackendKind::parse(raw)
                .map(NormalizedBackendKind::SelfHosted)
                .ok_or_else(|| ProviderBindingError::UnknownBackendKind(raw.to_string())),
            Provider::Other => Err(ProviderBindingError::UnknownBackendKind(raw.to_string())),
        }
    }

    pub fn normalize_auth(
        provider: Provider,
        raw: &str,
    ) -> Result<NormalizedAuthMethod, ProviderBindingError> {
        match provider {
            Provider::Anthropic => AnthropicAuthMethod::parse(raw)
                .map(NormalizedAuthMethod::Anthropic)
                .ok_or_else(|| ProviderBindingError::UnknownAuthMethod(raw.to_string())),
            Provider::OpenAI => OpenAiAuthMethod::parse(raw)
                .map(NormalizedAuthMethod::OpenAi)
                .ok_or_else(|| ProviderBindingError::UnknownAuthMethod(raw.to_string())),
            Provider::Gemini => GoogleAuthMethod::parse(raw)
                .map(NormalizedAuthMethod::Google)
                .ok_or_else(|| ProviderBindingError::UnknownAuthMethod(raw.to_string())),
            Provider::SelfHosted => SelfHostedAuthMethod::parse(raw)
                .map(NormalizedAuthMethod::SelfHosted)
                .ok_or_else(|| ProviderBindingError::UnknownAuthMethod(raw.to_string())),
            Provider::Other => Err(ProviderBindingError::UnknownAuthMethod(raw.to_string())),
        }
    }

    pub fn supports(backend: NormalizedBackendKind, auth: NormalizedAuthMethod) -> bool {
        matches!(
            (backend, auth),
            (
                NormalizedBackendKind::OpenAi(OpenAiBackendKind::OpenAiApi),
                NormalizedAuthMethod::OpenAi(
                    OpenAiAuthMethod::ApiKey
                        | OpenAiAuthMethod::StaticBearer
                        | OpenAiAuthMethod::ExternalAuthorizer,
                ),
            ) | (
                NormalizedBackendKind::OpenAi(OpenAiBackendKind::ChatGptBackend),
                NormalizedAuthMethod::OpenAi(
                    OpenAiAuthMethod::ManagedChatGptOauth | OpenAiAuthMethod::ExternalChatGptTokens,
                ),
            ) | (
                NormalizedBackendKind::Anthropic(AnthropicBackendKind::AnthropicApi),
                NormalizedAuthMethod::Anthropic(
                    AnthropicAuthMethod::ApiKey
                        | AnthropicAuthMethod::StaticBearer
                        | AnthropicAuthMethod::ClaudeAiOauth
                        | AnthropicAuthMethod::OauthToApiKey
                        | AnthropicAuthMethod::ExternalAuthorizer,
                ),
            ) | (
                NormalizedBackendKind::Anthropic(AnthropicBackendKind::Bedrock),
                NormalizedAuthMethod::Anthropic(
                    AnthropicAuthMethod::BedrockBearer
                        | AnthropicAuthMethod::BedrockAwsSigv4
                        | AnthropicAuthMethod::ExternalAuthorizer,
                ),
            ) | (
                NormalizedBackendKind::Anthropic(AnthropicBackendKind::Vertex),
                NormalizedAuthMethod::Anthropic(
                    AnthropicAuthMethod::VertexGoogleAuth | AnthropicAuthMethod::ExternalAuthorizer,
                ),
            ) | (
                NormalizedBackendKind::Anthropic(AnthropicBackendKind::Foundry),
                NormalizedAuthMethod::Anthropic(
                    AnthropicAuthMethod::FoundryApiKey
                        | AnthropicAuthMethod::FoundryAzureAd
                        | AnthropicAuthMethod::ExternalAuthorizer,
                ),
            ) | (
                NormalizedBackendKind::Google(GoogleBackendKind::GoogleGenAi),
                NormalizedAuthMethod::Google(
                    GoogleAuthMethod::ApiKey
                        | GoogleAuthMethod::BearerApiKey
                        | GoogleAuthMethod::ExternalAuthorizer,
                ),
            ) | (
                NormalizedBackendKind::Google(GoogleBackendKind::VertexAi),
                NormalizedAuthMethod::Google(
                    GoogleAuthMethod::Adc
                        | GoogleAuthMethod::ApiKeyExpress
                        | GoogleAuthMethod::ExternalAuthorizer,
                ),
            ) | (
                NormalizedBackendKind::Google(GoogleBackendKind::GoogleCodeAssist),
                NormalizedAuthMethod::Google(
                    GoogleAuthMethod::GoogleOauth
                        | GoogleAuthMethod::ComputeAdc
                        | GoogleAuthMethod::ExternalAuthorizer,
                ),
            ) | (
                NormalizedBackendKind::SelfHosted(
                    SelfHostedBackendKind::SelfHosted | SelfHostedBackendKind::OpenAiCompatible,
                ),
                NormalizedAuthMethod::SelfHosted(
                    SelfHostedAuthMethod::ApiKey
                        | SelfHostedAuthMethod::None
                        | SelfHostedAuthMethod::StaticBearer,
                ),
            )
        )
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use meerkat_core::CredentialSourceSpec;

    fn connection_ref() -> ConnectionRef {
        ConnectionRef {
            realm: meerkat_core::connection::RealmId::parse("dev").unwrap(),
            binding: meerkat_core::connection::BindingId::parse("default").unwrap(),
            profile: None,
        }
    }

    fn backend(provider: Provider, kind: &str) -> BackendProfile {
        BackendProfile {
            id: "b".into(),
            provider,
            backend_kind: kind.into(),
            base_url: None,
            options: serde_json::Value::Null,
        }
    }

    fn auth(provider: Provider, method: &str) -> AuthProfile {
        AuthProfile {
            id: "a".into(),
            provider,
            auth_method: method.into(),
            source: CredentialSourceSpec::InlineSecret {
                secret: "secret".into(),
            },
            constraints: Default::default(),
            metadata_defaults: Default::default(),
        }
    }

    #[test]
    fn validate_accepts_openai_allowed_combination() {
        let binding = ProviderRuntimeCatalog::validate_binding(
            &connection_ref(),
            &backend(Provider::OpenAI, "openai_api"),
            &auth(Provider::OpenAI, "api_key"),
            &BindingPolicy::default(),
        )
        .unwrap();
        assert_eq!(binding.provider, Provider::OpenAI);
        assert_eq!(
            binding.backend,
            NormalizedBackendKind::OpenAi(OpenAiBackendKind::OpenAiApi)
        );
        assert_eq!(
            binding.auth,
            NormalizedAuthMethod::OpenAi(OpenAiAuthMethod::ApiKey)
        );
    }

    #[test]
    fn validate_rejects_unknown_backend_kind() {
        let err = ProviderRuntimeCatalog::validate_binding(
            &connection_ref(),
            &backend(Provider::OpenAI, "bogus_backend"),
            &auth(Provider::OpenAI, "api_key"),
            &BindingPolicy::default(),
        )
        .unwrap_err();
        assert!(matches!(err, ProviderBindingError::UnknownBackendKind(_)));
    }

    #[test]
    fn validate_rejects_unknown_auth_method() {
        let err = ProviderRuntimeCatalog::validate_binding(
            &connection_ref(),
            &backend(Provider::OpenAI, "openai_api"),
            &auth(Provider::OpenAI, "bogus_auth"),
            &BindingPolicy::default(),
        )
        .unwrap_err();
        assert!(matches!(err, ProviderBindingError::UnknownAuthMethod(_)));
    }

    #[test]
    fn validate_rejects_incompatible_backend_auth() {
        let err = ProviderRuntimeCatalog::validate_binding(
            &connection_ref(),
            &backend(Provider::OpenAI, "openai_api"),
            &auth(Provider::OpenAI, "managed_chatgpt_oauth"),
            &BindingPolicy::default(),
        )
        .unwrap_err();
        assert!(matches!(
            err,
            ProviderBindingError::UnsupportedCombination { .. }
        ));
    }

    #[test]
    fn validate_rejects_profile_provider_mismatch() {
        let err = ProviderRuntimeCatalog::validate_binding(
            &connection_ref(),
            &backend(Provider::OpenAI, "openai_api"),
            &auth(Provider::Anthropic, "api_key"),
            &BindingPolicy::default(),
        )
        .unwrap_err();
        assert!(matches!(err, ProviderBindingError::ProviderMismatch));
    }

    #[test]
    fn catalog_supports_expected_provider_edges() {
        assert!(ProviderRuntimeCatalog::supports(
            NormalizedBackendKind::OpenAi(OpenAiBackendKind::ChatGptBackend),
            NormalizedAuthMethod::OpenAi(OpenAiAuthMethod::ManagedChatGptOauth),
        ));
        assert!(ProviderRuntimeCatalog::supports(
            NormalizedBackendKind::Anthropic(AnthropicBackendKind::Bedrock),
            NormalizedAuthMethod::Anthropic(AnthropicAuthMethod::BedrockAwsSigv4),
        ));
        assert!(ProviderRuntimeCatalog::supports(
            NormalizedBackendKind::Google(GoogleBackendKind::GoogleCodeAssist),
            NormalizedAuthMethod::Google(GoogleAuthMethod::GoogleOauth),
        ));
        assert!(ProviderRuntimeCatalog::supports(
            NormalizedBackendKind::SelfHosted(SelfHostedBackendKind::SelfHosted),
            NormalizedAuthMethod::SelfHosted(SelfHostedAuthMethod::None),
        ));
    }

    #[test]
    fn catalog_rejects_cross_provider_typed_pairs() {
        assert!(!ProviderRuntimeCatalog::supports(
            NormalizedBackendKind::OpenAi(OpenAiBackendKind::OpenAiApi),
            NormalizedAuthMethod::Anthropic(AnthropicAuthMethod::ApiKey),
        ));
    }

    #[test]
    fn catalog_declares_supported_provider_identities() {
        for provider in Provider::ALL_CONCRETE {
            assert!(ProviderRuntimeCatalog::is_supported_provider(*provider));
        }
        assert!(!ProviderRuntimeCatalog::is_supported_provider(
            Provider::Other
        ));
    }
}
