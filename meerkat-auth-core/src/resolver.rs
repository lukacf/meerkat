//! Shared resolver helpers used by provider runtimes.
//!
//! Plan §6.11 closure: resolvers return credential material directly
//! (a `String` secret). Simple-secret paths cover `InlineSecret` /
//! `Env` / `ExternalResolver` (via the typed
//! `ResolvedAuthEnvelope::InlineSecret` variant — dogma §5 closure
//! replaced the `"__secret__"` synthetic-header-key convention) for
//! `api_key` / `static_bearer` auth methods. The external-authorizer
//! resolver surfaces upstream AuthError by calling the resolver handle
//! but the real `Arc<dyn HttpAuthorizer>` is constructed by the
//! provider runtime at build time (cloud-specific clients depend on
//! SDKs that can't serde-roundtrip).

use chrono::{DateTime, Utc};
use meerkat_core::{
    AuthError, AuthMetadata, AuthRouteHints, CredentialSourceSpec, ResolvedAuthEnvelope,
};

use meerkat_llm_core::provider_runtime::errors::ProviderAuthError;
use meerkat_llm_core::provider_runtime::registry::ResolverEnvironment;

pub fn interactive_login_error(
    binding: &meerkat_llm_core::provider_runtime::binding::ValidatedBinding,
) -> ProviderAuthError {
    let method_implies_interactive = match binding.auth {
        meerkat_llm_core::provider_runtime::binding::NormalizedAuthMethod::OpenAi(method) => {
            matches!(
                method,
                meerkat_core::provider_matrix::openai::OpenAiAuthMethod::ManagedChatGptOauth
                    | meerkat_core::provider_matrix::openai::OpenAiAuthMethod::ExternalChatGptTokens
            )
        }
        meerkat_llm_core::provider_runtime::binding::NormalizedAuthMethod::Anthropic(method) => {
            matches!(
                method,
                meerkat_core::provider_matrix::anthropic::AnthropicAuthMethod::ClaudeAiOauth
                    | meerkat_core::provider_matrix::anthropic::AnthropicAuthMethod::OauthToApiKey
            )
        }
        meerkat_llm_core::provider_runtime::binding::NormalizedAuthMethod::Google(method) => {
            matches!(
                method,
                meerkat_core::provider_matrix::google::GoogleAuthMethod::GoogleOauth
            )
        }
    };
    let source_implies_interactive = matches!(
        binding.auth_profile.source,
        CredentialSourceSpec::ManagedStore { .. } | CredentialSourceSpec::PlatformDefault
    );

    if binding.auth_profile.constraints.allow_interactive_login
        || method_implies_interactive
        || source_implies_interactive
    {
        ProviderAuthError::Auth(AuthError::InteractiveLoginRequired)
    } else {
        ProviderAuthError::Auth(AuthError::MissingSecret)
    }
}

pub fn refresh_allowed(
    binding: &meerkat_llm_core::provider_runtime::binding::ValidatedBinding,
) -> bool {
    binding.auth_profile.constraints.allow_refresh
}

pub fn apply_auth_resolution_policy(
    binding: &meerkat_llm_core::provider_runtime::binding::ValidatedBinding,
    mut metadata: AuthMetadata,
) -> Result<AuthMetadata, ProviderAuthError> {
    if metadata.organization_id.is_none() {
        metadata.organization_id = binding
            .auth_profile
            .metadata_defaults
            .organization_id
            .clone();
    }
    if metadata.workspace_id.is_none() {
        metadata.workspace_id = binding.auth_profile.metadata_defaults.workspace_id.clone();
    }
    if matches!(metadata.route_hints, AuthRouteHints::None) {
        metadata.route_hints = binding.auth_profile.metadata_defaults.route_hints.clone();
    }
    if metadata.provider_metadata.is_none() {
        metadata.provider_metadata = binding
            .auth_profile
            .metadata_defaults
            .provider_metadata
            .clone();
    }

    if (binding.auth_profile.constraints.require_account_id
        || binding.policy.require_metadata_account)
        && metadata.account_id.as_deref().is_none_or(str::is_empty)
    {
        return Err(ProviderAuthError::Auth(AuthError::MissingRequiredMetadata(
            "account_id".into(),
        )));
    }
    if (binding.auth_profile.constraints.require_workspace_id
        || binding.policy.require_metadata_workspace)
        && metadata.workspace_id.as_deref().is_none_or(str::is_empty)
    {
        return Err(ProviderAuthError::Auth(AuthError::MissingRequiredMetadata(
            "workspace_id".into(),
        )));
    }

    Ok(metadata)
}

/// Resolve a [`CredentialSourceSpec`] into a single secret string. Used
/// by api_key / static_bearer auth methods. Returns the resolved secret
/// directly; the provider runtime wraps it via
/// `StaticLease::inline_secret` for transport to `build_client`.
pub async fn resolve_simple_secret(
    source: &CredentialSourceSpec,
    env: &ResolverEnvironment,
    binding: &meerkat_llm_core::provider_runtime::binding::ValidatedBinding,
) -> Result<String, ProviderAuthError> {
    match source {
        CredentialSourceSpec::InlineSecret { secret } => Ok(secret.clone()),
        CredentialSourceSpec::Env { env: var, fallback } => {
            // Single canonical owner of env-var credential resolution
            // policy (dogma §1). For each var name (primary + ordered
            // fallback chain), `RKAT_<VAR>` overrides `<VAR>`. The
            // factory body no longer encodes this policy inline.
            let candidates =
                std::iter::once(var.as_str()).chain(fallback.iter().map(String::as_str));
            for candidate in candidates {
                let rkat_override = if candidate.starts_with("RKAT_") {
                    None
                } else {
                    (env.env_lookup)(&format!("RKAT_{candidate}"))
                };
                if let Some(value) = rkat_override.or_else(|| (env.env_lookup)(candidate)) {
                    return Ok(value);
                }
            }
            Err(ProviderAuthError::Auth(AuthError::MissingSecret))
        }
        CredentialSourceSpec::ExternalResolver { handle } => {
            let resolver = env
                .external_resolvers
                .get(handle)
                .ok_or_else(|| ProviderAuthError::ExternalResolverMissing(handle.clone()))?;
            let envelope = resolver.resolve(binding).await?;
            extract_secret_from_envelope(envelope)
        }
        #[cfg(not(target_arch = "wasm32"))]
        CredentialSourceSpec::Command {
            program,
            args,
            cwd,
            env: cmd_env,
            timeout_ms,
            refresh_interval_ms,
        } => {
            use crate::auth_store::{CommandCredentialRunner, CommandCredentialSpec};
            let spec = CommandCredentialSpec {
                program: program.clone(),
                args: args.clone(),
                cwd: cwd.clone(),
                env: cmd_env
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
                timeout_ms: *timeout_ms,
                refresh_interval_ms: *refresh_interval_ms,
            };
            let runner = CommandCredentialRunner::new(spec);
            let tokens = runner
                .resolve()
                .await
                .map_err(|e| ProviderAuthError::SourceResolutionFailed(e.to_string()))?;
            tokens.primary_secret.ok_or_else(|| {
                ProviderAuthError::SourceResolutionFailed(
                    "command returned no primary_secret in its persisted tokens payload".into(),
                )
            })
        }
        #[cfg(target_arch = "wasm32")]
        CredentialSourceSpec::Command { .. } => Err(ProviderAuthError::SourceResolutionFailed(
            "CredentialSourceSpec::Command requires a subprocess runner; \
             not available on the wasm32 target"
                .into(),
        )),
        CredentialSourceSpec::FileDescriptor { .. } => {
            Err(ProviderAuthError::SourceResolutionFailed(
                "CredentialSourceSpec::FileDescriptor requires a host-scoped reader; \
                 not reachable from the simple-secret resolver"
                    .into(),
            ))
        }
        CredentialSourceSpec::ManagedStore { .. } | CredentialSourceSpec::PlatformDefault => {
            Err(interactive_login_error(binding))
        }
    }
}

/// External auth for `external_authorizer` method. Calls the host-
/// registered resolver to surface any upstream `AuthError` before the
/// provider runtime constructs its concrete `HttpAuthorizer`. Returns
/// `Ok(())` on success — the provider runtime is responsible for
/// wiring the backend-specific authorizer (Bedrock SigV4, Vertex
/// GoogleAuth, Foundry AzureAd, etc.) into a `DynamicLease`.
pub async fn resolve_external_authorizer(
    source: &CredentialSourceSpec,
    env: &ResolverEnvironment,
    binding: &meerkat_llm_core::provider_runtime::binding::ValidatedBinding,
) -> Result<ResolvedAuthEnvelope, ProviderAuthError> {
    let CredentialSourceSpec::ExternalResolver { handle } = source else {
        return Err(ProviderAuthError::SourceResolutionFailed(format!(
            "external_authorizer auth requires CredentialSourceSpec::ExternalResolver, \
             got {source:?}",
        )));
    };
    let resolver = env
        .external_resolvers
        .get(handle)
        .ok_or_else(|| ProviderAuthError::ExternalResolverMissing(handle.clone()))?;
    resolver
        .resolve(binding)
        .await
        .map_err(ProviderAuthError::from)
}

/// Extract a simple secret from a resolved envelope. Dogma §5:
/// `ResolvedAuthEnvelope::InlineSecret` is the typed canonical
/// variant. The legacy `StaticHeaders` shape is still accepted
/// transitively — a single header maps to the secret — but external
/// resolvers should produce the typed variant directly.
fn extract_secret_from_envelope(
    envelope: ResolvedAuthEnvelope,
) -> Result<String, ProviderAuthError> {
    let (secret, _, _) = extract_inline_secret_envelope(envelope)?;
    Ok(secret)
}

pub fn extract_inline_secret_envelope(
    envelope: ResolvedAuthEnvelope,
) -> Result<(String, AuthMetadata, Option<DateTime<Utc>>), ProviderAuthError> {
    match envelope {
        ResolvedAuthEnvelope::InlineSecret {
            secret,
            metadata,
            expires_at,
        } => Ok((secret, metadata, expires_at)),
        ResolvedAuthEnvelope::StaticHeaders {
            headers,
            metadata,
            expires_at,
        } => {
            if let [(_, value)] = headers.as_slice() {
                Ok((
                    value.strip_prefix("Bearer ").unwrap_or(value).to_string(),
                    metadata,
                    expires_at,
                ))
            } else {
                Err(ProviderAuthError::SourceResolutionFailed(
                    "external resolver returned StaticHeaders with \
                     multiple entries; api_key/static_bearer path \
                     requires InlineSecret or a single-entry \
                     StaticHeaders envelope"
                        .into(),
                ))
            }
        }
        ResolvedAuthEnvelope::DynamicAuthorizer { .. } => {
            Err(ProviderAuthError::SourceResolutionFailed(
                "external resolver returned DynamicAuthorizer envelope; \
                 use external_authorizer auth method instead"
                    .into(),
            ))
        }
        ResolvedAuthEnvelope::None { .. } => Err(ProviderAuthError::Auth(AuthError::MissingSecret)),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn binding_with_policy(
        constraints: meerkat_core::AuthConstraints,
        policy: meerkat_core::BindingPolicy,
        metadata_defaults: AuthMetadata,
    ) -> meerkat_llm_core::provider_runtime::binding::ValidatedBinding {
        let auth_profile = meerkat_core::AuthProfile {
            id: "auth".into(),
            provider: meerkat_core::Provider::OpenAI,
            auth_method: "api_key".into(),
            source: CredentialSourceSpec::InlineSecret {
                secret: "sk-test".into(),
            },
            storage: Default::default(),
            constraints,
            metadata_defaults: meerkat_core::AuthMetadataDefaults {
                organization_id: metadata_defaults.organization_id.clone(),
                workspace_id: metadata_defaults.workspace_id.clone(),
                route_hints: metadata_defaults.route_hints.clone(),
                provider_metadata: metadata_defaults.provider_metadata,
            },
        };
        let backend_profile = meerkat_core::BackendProfile {
            id: "backend".into(),
            provider: meerkat_core::Provider::OpenAI,
            backend_kind: "openai_api".into(),
            base_url: None,
            options: serde_json::Value::Null,
        };
        meerkat_llm_core::provider_runtime::binding::ValidatedBinding {
            connection_ref: meerkat_core::ConnectionRef {
                realm_id: "realm-x".into(),
                binding_id: "binding-y".into(),
            },
            provider: meerkat_core::Provider::OpenAI,
            backend: meerkat_llm_core::provider_runtime::binding::NormalizedBackendKind::OpenAi(
                meerkat_core::provider_matrix::openai::OpenAiBackendKind::OpenAiApi,
            ),
            auth: meerkat_llm_core::provider_runtime::binding::NormalizedAuthMethod::OpenAi(
                meerkat_core::provider_matrix::openai::OpenAiAuthMethod::ApiKey,
            ),
            backend_profile: std::sync::Arc::new(backend_profile),
            auth_profile: std::sync::Arc::new(auth_profile),
            policy,
        }
    }

    #[test]
    fn extract_secret_inline_variant() {
        let env = ResolvedAuthEnvelope::InlineSecret {
            secret: "sk-x".into(),
            metadata: Default::default(),
            expires_at: None,
        };
        assert_eq!(extract_secret_from_envelope(env).unwrap(), "sk-x");
    }

    #[test]
    fn extract_secret_single_header_legacy() {
        // Back-compat: a single-header StaticHeaders envelope is still
        // accepted. Multi-header envelopes (which previously carried
        // the `__secret__` key alongside other wire headers) are an
        // error — external resolvers should use the typed InlineSecret
        // variant.
        let env = ResolvedAuthEnvelope::StaticHeaders {
            headers: vec![("Authorization".into(), "Bearer sk-y".into())],
            metadata: Default::default(),
            expires_at: None,
        };
        assert_eq!(extract_secret_from_envelope(env).unwrap(), "sk-y");
    }

    #[test]
    fn extract_secret_multi_header_errors() {
        let env = ResolvedAuthEnvelope::StaticHeaders {
            headers: vec![
                ("Authorization".into(), "Bearer x".into()),
                ("X-Provider-Id".into(), "acct".into()),
            ],
            metadata: Default::default(),
            expires_at: None,
        };
        let err = extract_secret_from_envelope(env).unwrap_err();
        assert!(matches!(err, ProviderAuthError::SourceResolutionFailed(_)));
    }

    #[test]
    fn extract_dynamic_envelope_errors() {
        let env = ResolvedAuthEnvelope::DynamicAuthorizer {
            metadata: Default::default(),
            expires_at: None,
        };
        let err = extract_secret_from_envelope(env).unwrap_err();
        assert!(matches!(err, ProviderAuthError::SourceResolutionFailed(_)));
    }

    #[test]
    fn extract_none_envelope_errors() {
        let env = ResolvedAuthEnvelope::None {
            metadata: Default::default(),
        };
        let err = extract_secret_from_envelope(env).unwrap_err();
        assert!(matches!(
            err,
            ProviderAuthError::Auth(AuthError::MissingSecret)
        ));
    }

    #[test]
    fn interactive_login_error_respects_binding_constraints() {
        let allow = binding_with_policy(
            meerkat_core::AuthConstraints {
                allow_interactive_login: true,
                ..Default::default()
            },
            Default::default(),
            AuthMetadata::default(),
        );
        assert!(matches!(
            interactive_login_error(&allow),
            ProviderAuthError::Auth(AuthError::InteractiveLoginRequired)
        ));

        let deny = binding_with_policy(
            Default::default(),
            Default::default(),
            AuthMetadata::default(),
        );
        assert!(matches!(
            interactive_login_error(&deny),
            ProviderAuthError::Auth(AuthError::MissingSecret)
        ));
    }

    #[test]
    fn apply_auth_resolution_policy_merges_defaults_and_requires_binding_metadata() {
        let binding = binding_with_policy(
            meerkat_core::AuthConstraints {
                require_workspace_id: true,
                ..Default::default()
            },
            meerkat_core::BindingPolicy {
                allow_auth_override: false,
                require_metadata_account: true,
                require_metadata_workspace: false,
            },
            AuthMetadata {
                organization_id: Some("org-x".into()),
                workspace_id: Some("ws-x".into()),
                ..AuthMetadata::default()
            },
        );

        let err = apply_auth_resolution_policy(&binding, AuthMetadata::default()).unwrap_err();
        assert!(matches!(
            err,
            ProviderAuthError::Auth(AuthError::MissingRequiredMetadata(field))
            if field == "account_id"
        ));

        let merged = apply_auth_resolution_policy(
            &binding,
            AuthMetadata {
                account_id: Some("acct-x".into()),
                ..AuthMetadata::default()
            },
        )
        .expect("metadata defaults should satisfy workspace requirement");
        assert_eq!(merged.organization_id.as_deref(), Some("org-x"));
        assert_eq!(merged.workspace_id.as_deref(), Some("ws-x"));
    }
}
