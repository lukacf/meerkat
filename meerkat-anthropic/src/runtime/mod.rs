//! Anthropic provider runtime.

#[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
pub mod oauth;

use std::sync::Arc;

use async_trait::async_trait;

#[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
use meerkat_core::AuthError;
#[cfg(any(
    all(not(target_arch = "wasm32"), feature = "oauth"),
    all(not(target_arch = "wasm32"), feature = "bedrock"),
    all(not(target_arch = "wasm32"), feature = "vertex"),
    all(not(target_arch = "wasm32"), feature = "foundry")
))]
use meerkat_core::HttpAuthorizer;
use meerkat_core::{AuthLease, AuthMetadata, Provider};

#[cfg(not(all(not(target_arch = "wasm32"), feature = "oauth")))]
use meerkat_auth_core::resolver::interactive_login_error;
#[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
use meerkat_auth_core::resolver::{
    ManagedStoreLifecycle, begin_managed_store_oauth_refresh_lifecycle,
    load_managed_store_tokens_with_lifecycle, managed_store_oauth_refresh_failure_coordinator,
    managed_store_oauth_refresh_failure_is_permanent, mark_managed_store_oauth_refresh_failed,
    publish_managed_store_tokens_lifecycle_and_save, refresh_allowed,
};
use meerkat_auth_core::resolver::{
    finalize_auth_metadata, resolve_external_authorizer, resolve_simple_secret,
};
#[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
use meerkat_auth_core::{
    auth_store::PersistedAuthMode, oauth_flow::validate_oauth_target_for_auth_mode,
};
use meerkat_llm_core::LlmClient;
#[cfg(any(
    all(not(target_arch = "wasm32"), feature = "oauth"),
    all(not(target_arch = "wasm32"), feature = "bedrock"),
    all(not(target_arch = "wasm32"), feature = "vertex"),
    all(not(target_arch = "wasm32"), feature = "foundry")
))]
use meerkat_llm_core::provider_runtime::binding::DynamicLease;
use meerkat_llm_core::provider_runtime::binding::{
    NormalizedAuthMethod, NormalizedBackendKind, ResolvedConnection, StaticLease, ValidatedBinding,
};
use meerkat_llm_core::provider_runtime::errors::{
    ProviderAuthError, ProviderBindingError, ProviderClientError,
};
use meerkat_llm_core::provider_runtime::registry::ResolverEnvironment;
use meerkat_llm_core::provider_runtime::runtime::ProviderRuntime;

pub use meerkat_core::provider_matrix::anthropic::{AnthropicAuthMethod, AnthropicBackendKind};

#[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
struct ClaudeAiOAuthAuthorizer {
    access_token: String,
}

#[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
impl ClaudeAiOAuthAuthorizer {
    fn new(access_token: String) -> Self {
        Self { access_token }
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
impl HttpAuthorizer for ClaudeAiOAuthAuthorizer {
    async fn authorize(
        &self,
        req: &mut meerkat_core::HttpAuthorizationRequest<'_>,
    ) -> Result<(), meerkat_core::AuthError> {
        req.headers.push((
            "Authorization".to_string(),
            format!("Bearer {}", self.access_token),
        ));
        req.headers.push((
            oauth::OAUTH_BETA_HEADER_NAME.to_string(),
            oauth::OAUTH_BETA_HEADER_VALUE.to_string(),
        ));
        req.headers.push(("x-app".to_string(), "cli".to_string()));
        Ok(())
    }

    fn label(&self) -> &'static str {
        "claude-ai-oauth"
    }
}

#[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
fn anthropic_oauth_refresh_failure_is_permanent(error: &oauth::AnthropicOAuthError) -> bool {
    match error {
        oauth::AnthropicOAuthError::InteractiveLoginRequired
        | oauth::AnthropicOAuthError::MissingRefreshToken => true,
        oauth::AnthropicOAuthError::Refresh(meerkat_auth_core::RefreshError::Refresh(message)) => {
            managed_store_oauth_refresh_failure_is_permanent(message)
        }
        oauth::AnthropicOAuthError::OAuth(error) => {
            managed_store_oauth_refresh_failure_is_permanent(&error.to_string())
        }
        _ => false,
    }
}

#[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
fn anthropic_oauth_refresh_error(
    error: oauth::AnthropicOAuthError,
    authmachine_failure: String,
) -> ProviderAuthError {
    let detail = if authmachine_failure.is_empty() {
        error.to_string()
    } else {
        format!("{error}{authmachine_failure}")
    };
    if authmachine_failure.is_empty() {
        match error {
            oauth::AnthropicOAuthError::InteractiveLoginRequired
            | oauth::AnthropicOAuthError::MissingRefreshToken => {
                return ProviderAuthError::Auth(AuthError::UserReauthRequired);
            }
            _ => {}
        }
    }
    ProviderAuthError::Auth(AuthError::RefreshFailed(detail))
}

pub struct AnthropicProviderRuntime;

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl ProviderRuntime for AnthropicProviderRuntime {
    fn provider_id(&self) -> Provider {
        Provider::Anthropic
    }

    async fn resolve_binding(
        &self,
        binding: &ValidatedBinding,
        env: &ResolverEnvironment,
    ) -> Result<ResolvedConnection, ProviderAuthError> {
        if binding.provider() != Provider::Anthropic {
            return Err(ProviderAuthError::Binding(
                ProviderBindingError::ProviderMismatch,
            ));
        }
        let auth_method = match binding.auth() {
            NormalizedAuthMethod::Anthropic(m) => m,
            _ => {
                return Err(ProviderAuthError::Binding(
                    ProviderBindingError::ProviderMismatch,
                ));
            }
        };
        let backend_kind = match binding.backend() {
            NormalizedBackendKind::Anthropic(k) => k,
            _ => {
                return Err(ProviderAuthError::Binding(
                    ProviderBindingError::ProviderMismatch,
                ));
            }
        };

        let source_label = format!("anthropic:{}", binding.auth_profile().id);
        let lease: Arc<dyn AuthLease> = match auth_method {
            AnthropicAuthMethod::ApiKey
            | AnthropicAuthMethod::StaticBearer
            | AnthropicAuthMethod::BedrockBearer
            | AnthropicAuthMethod::FoundryApiKey => {
                let secret =
                    resolve_simple_secret(&binding.auth_profile().source, env, binding).await?;
                let metadata = finalize_auth_metadata(binding, AuthMetadata::default())?;
                Arc::new(StaticLease::inline_secret(
                    secret,
                    metadata,
                    None,
                    source_label.clone(),
                ))
            }
            AnthropicAuthMethod::ExternalAuthorizer => {
                resolve_external_authorizer(&binding.auth_profile().source, env, binding).await?
            }
            AnthropicAuthMethod::BedrockAwsSigv4 => {
                #[cfg(all(not(target_arch = "wasm32"), feature = "bedrock"))]
                {
                    let region = bedrock_region(binding)?;
                    let lookup = env.env_lookup.clone();
                    let authorizer: Arc<dyn HttpAuthorizer> =
                        Arc::new(meerkat_auth_core::authorizers::AwsStsAuthorizer::new(
                            region.clone(),
                            meerkat_auth_core::authorizers::AwsCredentialProvider::from_env(
                                move |key| lookup(key),
                            ),
                        ));
                    let metadata = finalize_auth_metadata(
                        binding,
                        AuthMetadata {
                            provider_metadata: Some(meerkat_core::ProviderAuthMetadata::Anthropic(
                                meerkat_core::AnthropicAuthMetadata {
                                    aws_region: Some(region),
                                    ..Default::default()
                                },
                            )),
                            ..Default::default()
                        },
                    )?;
                    Arc::new(DynamicLease::from_authorizer(
                        authorizer,
                        metadata,
                        source_label.clone(),
                    ))
                }
                #[cfg(any(target_arch = "wasm32", not(feature = "bedrock")))]
                {
                    return Err(ProviderAuthError::SourceResolutionFailed(
                        "bedrock_aws_sigv4 requires the anthropic `bedrock` feature on non-wasm32"
                            .into(),
                    ));
                }
            }
            AnthropicAuthMethod::VertexGoogleAuth => {
                #[cfg(all(not(target_arch = "wasm32"), feature = "vertex"))]
                {
                    let mut authorizer =
                        meerkat_auth_core::authorizers::GoogleAuthAuthorizer::with_env_lookup(
                            meerkat_auth_core::authorizers::GoogleAuthChain::Default,
                            env.env_lookup.clone(),
                        );
                    if let Some(handle) = env.auth_lease_handle.clone() {
                        authorizer = authorizer.with_auth_lease_observer(
                            handle,
                            meerkat_core::handles::LeaseKey::from_auth_binding(
                                binding.auth_binding_ref(),
                            ),
                        );
                    }
                    let authorizer: Arc<dyn HttpAuthorizer> = Arc::new(authorizer);
                    let metadata = finalize_auth_metadata(
                        binding,
                        AuthMetadata {
                            provider_metadata: Some(meerkat_core::ProviderAuthMetadata::Anthropic(
                                meerkat_core::AnthropicAuthMetadata {
                                    vertex_project_id: backend_option_string(binding, "project_id")
                                        .or_else(|| {
                                            backend_option_string(binding, "vertex_project_id")
                                        }),
                                    vertex_region: backend_option_string(binding, "region")
                                        .or_else(|| {
                                            backend_option_string(binding, "vertex_region")
                                        }),
                                    ..Default::default()
                                },
                            )),
                            ..Default::default()
                        },
                    )?;
                    Arc::new(DynamicLease::from_authorizer(
                        authorizer,
                        metadata,
                        source_label.clone(),
                    ))
                }
                #[cfg(any(target_arch = "wasm32", not(feature = "vertex")))]
                {
                    return Err(ProviderAuthError::SourceResolutionFailed(
                        "vertex_google_auth requires the anthropic `vertex` feature on non-wasm32"
                            .into(),
                    ));
                }
            }
            AnthropicAuthMethod::FoundryAzureAd => {
                #[cfg(all(not(target_arch = "wasm32"), feature = "foundry"))]
                {
                    let lookup = env.env_lookup.clone();
                    let creds = meerkat_auth_core::authorizers::AzureClientCredentials::from_env(
                        move |key| lookup(key),
                    )
                    .map_err(|err| ProviderAuthError::Auth(err.into()))?;
                    let mut authorizer = meerkat_auth_core::authorizers::AzureAdAuthorizer::new(
                        "https://cognitiveservices.azure.com/.default",
                        creds,
                    );
                    if let Some(handle) = env.auth_lease_handle.clone() {
                        authorizer = authorizer.with_auth_lease_observer(
                            handle,
                            meerkat_core::handles::LeaseKey::from_auth_binding(
                                binding.auth_binding_ref(),
                            ),
                        );
                    }
                    let authorizer: Arc<dyn HttpAuthorizer> = Arc::new(authorizer);
                    let metadata = finalize_auth_metadata(
                        binding,
                        AuthMetadata {
                            provider_metadata: Some(meerkat_core::ProviderAuthMetadata::Anthropic(
                                meerkat_core::AnthropicAuthMetadata {
                                    foundry_deployment: binding.backend_profile().base_url.clone(),
                                    ..Default::default()
                                },
                            )),
                            ..Default::default()
                        },
                    )?;
                    Arc::new(DynamicLease::from_authorizer(
                        authorizer,
                        metadata,
                        source_label.clone(),
                    ))
                }
                #[cfg(any(target_arch = "wasm32", not(feature = "foundry")))]
                {
                    return Err(ProviderAuthError::SourceResolutionFailed(
                        "foundry_azure_ad requires the anthropic `foundry` feature on non-wasm32"
                            .into(),
                    ));
                }
            }
            AnthropicAuthMethod::ClaudeAiOauth | AnthropicAuthMethod::OauthToApiKey => {
                #[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
                {
                    let expected_mode = match auth_method {
                        AnthropicAuthMethod::ClaudeAiOauth => PersistedAuthMode::ClaudeAiOauth,
                        AnthropicAuthMethod::OauthToApiKey => PersistedAuthMode::OauthToApiKey,
                        _ => unreachable!("OAuth branch only handles OAuth auth methods"),
                    };
                    validate_oauth_target_for_auth_mode(
                        binding.auth_profile(),
                        Provider::Anthropic,
                        expected_mode,
                    )
                    .map_err(|e| ProviderAuthError::SourceResolutionFailed(e.to_string()))?;
                    let mut managed =
                        load_managed_store_tokens_with_lifecycle(env, binding).await?;
                    let lifecycle = managed.lifecycle;
                    let persisted = managed.tokens.clone();
                    let effective_tokens = match auth_method {
                        AnthropicAuthMethod::OauthToApiKey => {
                            if lifecycle == ManagedStoreLifecycle::RefreshRequired {
                                return Err(ProviderAuthError::Auth(AuthError::RefreshRequired));
                            }
                            persisted
                        }
                        AnthropicAuthMethod::ClaudeAiOauth => {
                            if lifecycle == ManagedStoreLifecycle::Authorized
                                && persisted.primary_secret.is_some()
                                && !env.force_refresh
                            {
                                persisted
                            } else {
                                if !refresh_allowed(binding) {
                                    return Err(ProviderAuthError::Auth(
                                        AuthError::RefreshRequired,
                                    ));
                                }
                                let refresh_started = begin_managed_store_oauth_refresh_lifecycle(
                                    env,
                                    binding,
                                    &mut managed,
                                )?;
                                let coord = env.refresh_coord.clone().unwrap_or_else(|| {
                                    Arc::new(meerkat_auth_core::InMemoryCoordinator::new())
                                });
                                let coord = managed_store_oauth_refresh_failure_coordinator(
                                    coord,
                                    env.clone(),
                                    binding.clone(),
                                    refresh_started,
                                );
                                let endpoints =
                                    oauth::claude_ai_endpoints(oauth::MANUAL_REDIRECT_URL);
                                let runtime = oauth::AnthropicOAuthRuntime::new(
                                    managed.store.clone(),
                                    coord,
                                    endpoints,
                                    managed.key.clone(),
                                );
                                let commit_env = env.clone();
                                let commit_binding = binding.clone();
                                let commit: oauth::TokenCommitFn = Box::new(move |tokens| {
                                    Box::pin(async move {
                                        publish_managed_store_tokens_lifecycle_and_save(
                                            &commit_env,
                                            &commit_binding,
                                            &managed,
                                            &tokens,
                                        )
                                        .await
                                        .map_err(|e| {
                                            meerkat_auth_core::RefreshError::Refresh(e.to_string())
                                        })
                                    })
                                });
                                let refreshed = runtime
                                    .refresh_tokens_with_commit(commit, env.force_refresh)
                                    .await;
                                refreshed.map_err(|e| {
                                    let permanent =
                                        anthropic_oauth_refresh_failure_is_permanent(&e);
                                    let failure = mark_managed_store_oauth_refresh_failed(
                                        env,
                                        binding,
                                        refresh_started,
                                        permanent,
                                    )
                                    .err()
                                    .map(|err| format!("; {err}"))
                                    .unwrap_or_default();
                                    anthropic_oauth_refresh_error(e, failure)
                                })?
                            }
                        }
                        _ => unreachable!("arm guarded by outer match"),
                    };
                    let secret = effective_tokens
                        .primary_secret
                        .clone()
                        .ok_or(ProviderAuthError::Auth(AuthError::MissingSecret))?;
                    let mut anthropic_email: Option<String> = None;
                    let mut anthropic_user_id: Option<String> = None;
                    let mut anthropic_subscription_tier: Option<String> = None;
                    // Plan §4b.12: lift id_token claims.
                    if let Some(id_token) = effective_tokens.id_token.as_deref()
                        && let Ok(claims) =
                            meerkat_auth_core::auth_oauth::jwt::decode_payload(id_token)
                    {
                        let lifted = oauth::AnthropicIdClaims::lift_from_claims(&claims.raw);
                        anthropic_email = lifted.email;
                        anthropic_user_id = lifted.user_id;
                        anthropic_subscription_tier = lifted.subscription_tier;
                    }
                    let mut metadata = AuthMetadata::default();
                    if anthropic_email.is_some()
                        || anthropic_user_id.is_some()
                        || anthropic_subscription_tier.is_some()
                    {
                        metadata.account_id = anthropic_user_id.or(anthropic_email);
                        metadata.plan = anthropic_subscription_tier.clone();
                        metadata.provider_metadata =
                            Some(meerkat_core::ProviderAuthMetadata::Anthropic(
                                meerkat_core::AnthropicAuthMetadata {
                                    subscription_tier: anthropic_subscription_tier,
                                    ..Default::default()
                                },
                            ));
                    }
                    let metadata = finalize_auth_metadata(binding, metadata)?;
                    match auth_method {
                        AnthropicAuthMethod::ClaudeAiOauth => {
                            let authorizer: Arc<dyn HttpAuthorizer> =
                                Arc::new(ClaudeAiOAuthAuthorizer::new(secret));
                            Arc::new(DynamicLease::new(
                                authorizer,
                                metadata,
                                effective_tokens.expires_at,
                                source_label.clone(),
                            ))
                        }
                        AnthropicAuthMethod::OauthToApiKey => Arc::new(StaticLease::inline_secret(
                            secret,
                            metadata,
                            effective_tokens.expires_at,
                            source_label.clone(),
                        )),
                        _ => unreachable!("OAuth branch only handles OAuth auth methods"),
                    }
                }
                #[cfg(not(all(not(target_arch = "wasm32"), feature = "oauth")))]
                {
                    return Err(interactive_login_error(binding));
                }
            }
        };

        Ok(ResolvedConnection {
            provider: Provider::Anthropic,
            backend: NormalizedBackendKind::Anthropic(backend_kind),
            backend_profile: binding.backend_profile().clone(),
            auth_lease: lease,
        })
    }

    fn build_client(
        &self,
        connection: ResolvedConnection,
    ) -> Result<Arc<dyn LlmClient>, ProviderClientError> {
        // ProviderRuntimeRegistry dispatches on Provider enum, so this
        // runtime only receives Anthropic-backend connections. The match
        // is defensive; the non-Anthropic arms are unreachable at runtime.
        let backend_kind = match connection.backend {
            NormalizedBackendKind::Anthropic(k) => k,
            other => unreachable!(
                "AnthropicProviderRuntime received non-Anthropic backend: {other:?} \
                 — registry dispatch invariant violated"
            ),
        };
        // Plan §6.11: derive credential material from the auth lease
        // directly, directly from the lease. resolved_authorizer()
        // returns Some when the lease is a DynamicAuthorizer (Bedrock
        // SigV4 / Vertex GoogleAuth / Foundry AzureAd /
        // ExternalAuthorizer-DynamicAuthorizer); resolved_secret()
        // returns Some when the lease is a StaticHeaders with the
        // the resolved inline secret (api_key / static_bearer /
        // oauth_to_api_key / bedrock_bearer / pre-resolved Bearer).
        let authorizer_opt = connection.resolved_authorizer();
        let secret_opt = connection.resolved_secret();

        #[cfg(not(target_arch = "wasm32"))]
        if let Some(authorizer) = authorizer_opt {
            // All authorizer-backed backends wire the same way:
            // AnthropicClient with .authorizer(...) + .base_url(...).
            // Bedrock / Vertex require a non-empty base URL; AnthropicApi
            // falls back to its default.
            let base_url = match backend_kind {
                AnthropicBackendKind::Bedrock => connection
                    .backend_profile
                    .base_url
                    .clone()
                    .filter(|u| !u.is_empty())
                    .ok_or_else(|| {
                        ProviderClientError::InvalidBaseUrl(
                            "bedrock backend requires BackendProfile.base_url".to_string(),
                        )
                    })?,
                AnthropicBackendKind::Vertex | AnthropicBackendKind::Foundry => connection
                    .backend_profile
                    .base_url
                    .clone()
                    .unwrap_or_default(),
                AnthropicBackendKind::AnthropicApi => connection
                    .backend_profile
                    .base_url
                    .clone()
                    .unwrap_or_else(|| {
                        AnthropicBackendKind::AnthropicApi.default_base_url().into()
                    }),
            };
            let client = crate::AnthropicClient::builder(String::new())
                .authorizer(authorizer)
                .base_url(base_url)
                .build()
                .map_err(ProviderClientError::from)?;
            return Ok(Arc::new(client));
        }

        #[cfg(target_arch = "wasm32")]
        if authorizer_opt.is_some() {
            return Err(ProviderClientError::MissingFeature(
                "authorizer-backed auth not available on wasm32",
            ));
        }

        let secret = secret_opt.ok_or(ProviderClientError::NoCredentialMaterial)?;

        match backend_kind {
            // Native Anthropic API — plain x-api-key auth.
            AnthropicBackendKind::AnthropicApi | AnthropicBackendKind::Foundry => {
                let mut client =
                    crate::AnthropicClient::new(secret).map_err(ProviderClientError::from)?;
                if let Some(url) = &connection.backend_profile.base_url {
                    client = client.with_base_url(url.clone());
                }
                Ok(Arc::new(client))
            }
            // Bedrock static bearer (AWS_BEARER_TOKEN_BEDROCK).
            #[cfg(not(target_arch = "wasm32"))]
            AnthropicBackendKind::Bedrock => {
                let base_url = connection
                    .backend_profile
                    .base_url
                    .clone()
                    .filter(|u| !u.is_empty())
                    .ok_or_else(|| {
                        ProviderClientError::InvalidBaseUrl(
                            "bedrock backend requires BackendProfile.base_url \
                             (e.g. https://bedrock-runtime.us-east-1.amazonaws.com)"
                                .to_string(),
                        )
                    })?;
                let authorizer: std::sync::Arc<dyn meerkat_core::HttpAuthorizer> =
                    std::sync::Arc::new(
                        meerkat_auth_core::authorizers::StaticBearerAuthorizer::new(
                            secret,
                            "bedrock-bearer",
                        ),
                    );
                let client = crate::AnthropicClient::builder(String::new())
                    .authorizer(authorizer)
                    .base_url(base_url)
                    .build()
                    .map_err(ProviderClientError::from)?;
                Ok(Arc::new(client))
            }
            #[cfg(target_arch = "wasm32")]
            AnthropicBackendKind::Bedrock => Err(ProviderClientError::MissingFeature(
                "bedrock-backend not available on wasm32",
            )),
            // Vertex with a pre-resolved bearer secret (ExternalAuthorizer
            // producing an InlineSecret envelope).
            #[cfg(not(target_arch = "wasm32"))]
            AnthropicBackendKind::Vertex => {
                let base_url = connection
                    .backend_profile
                    .base_url
                    .clone()
                    .filter(|u| !u.is_empty())
                    .ok_or_else(|| {
                        ProviderClientError::InvalidBaseUrl(
                            "vertex backend requires BackendProfile.base_url \
                             (e.g. https://<region>-aiplatform.googleapis.com)"
                                .to_string(),
                        )
                    })?;
                let authorizer: std::sync::Arc<dyn meerkat_core::HttpAuthorizer> =
                    std::sync::Arc::new(
                        meerkat_auth_core::authorizers::StaticBearerAuthorizer::new(
                            secret,
                            "vertex-bearer",
                        ),
                    );
                let client = crate::AnthropicClient::builder(String::new())
                    .authorizer(authorizer)
                    .base_url(base_url)
                    .build()
                    .map_err(ProviderClientError::from)?;
                Ok(Arc::new(client))
            }
            #[cfg(target_arch = "wasm32")]
            AnthropicBackendKind::Vertex => Err(ProviderClientError::MissingFeature(
                "vertex-backend with authorizer-backed auth not available on wasm32",
            )),
        }
    }
}

#[cfg(any(
    all(not(target_arch = "wasm32"), feature = "bedrock"),
    all(not(target_arch = "wasm32"), feature = "vertex")
))]
fn backend_option_string(binding: &ValidatedBinding, key: &str) -> Option<String> {
    binding
        .backend_profile()
        .options
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(ToString::to_string)
}

#[cfg(all(not(target_arch = "wasm32"), feature = "bedrock"))]
fn bedrock_region(binding: &ValidatedBinding) -> Result<String, ProviderAuthError> {
    explicit_anthropic_aws_region(binding)
        .or_else(|| backend_option_string(binding, "aws_region").and_then(non_empty_region))
        .or_else(|| backend_option_string(binding, "region").and_then(non_empty_region))
        .ok_or_else(|| {
            ProviderAuthError::SourceResolutionFailed(format!(
                "bedrock_aws_sigv4 requires an explicit AWS signing region for binding {}/{}; \
                 set auth_profile.metadata_defaults.provider_metadata.aws_region or \
                 backend_profile.options.aws_region/region. Region is not inferred from \
                 BackendProfile.base_url.",
                binding.auth_binding_ref().realm.as_str(),
                binding.auth_binding_ref().binding.as_str()
            ))
        })
}

#[cfg(all(not(target_arch = "wasm32"), feature = "bedrock"))]
fn explicit_anthropic_aws_region(binding: &ValidatedBinding) -> Option<String> {
    match &binding.auth_profile().metadata_defaults.provider_metadata {
        Some(meerkat_core::ProviderAuthMetadata::Anthropic(metadata)) => metadata
            .aws_region
            .as_deref()
            .and_then(|region| non_empty_region(region.to_string())),
        _ => None,
    }
}

#[cfg(all(not(target_arch = "wasm32"), feature = "bedrock"))]
fn non_empty_region(region: String) -> Option<String> {
    let region = region.trim();
    (!region.is_empty()).then(|| region.to_string())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    #[cfg(all(not(target_arch = "wasm32"), feature = "bedrock"))]
    use meerkat_core::{AuthProfile, BackendProfile, BindingPolicy};
    use meerkat_llm_core::provider_runtime::ProviderRuntimeCatalog;
    #[cfg(all(not(target_arch = "wasm32"), feature = "bedrock"))]
    use meerkat_llm_core::provider_runtime::runtime::ProviderRuntime;

    #[test]
    fn typed_catalog_covers_api_key_and_oauth_variants() {
        assert!(ProviderRuntimeCatalog::supports(
            NormalizedBackendKind::Anthropic(AnthropicBackendKind::AnthropicApi),
            NormalizedAuthMethod::Anthropic(AnthropicAuthMethod::ApiKey),
        ));
        assert!(ProviderRuntimeCatalog::supports(
            NormalizedBackendKind::Anthropic(AnthropicBackendKind::AnthropicApi),
            NormalizedAuthMethod::Anthropic(AnthropicAuthMethod::ClaudeAiOauth),
        ));
    }

    #[test]
    fn provider_id_is_anthropic() {
        assert_eq!(AnthropicProviderRuntime.provider_id(), Provider::Anthropic);
    }

    #[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
    #[tokio::test]
    async fn claude_ai_oauth_authorizer_sets_bearer_and_beta_headers() {
        let authorizer = ClaudeAiOAuthAuthorizer::new("tok-claude".to_string());
        let mut headers = Vec::new();
        let mut request = meerkat_core::HttpAuthorizationRequest {
            method: "POST",
            url: "https://api.anthropic.com/v1/messages",
            headers: &mut headers,
        };

        authorizer.authorize(&mut request).await.unwrap();

        assert!(headers.contains(&("Authorization".to_string(), "Bearer tok-claude".to_string(),)));
        assert!(headers.contains(&(
            oauth::OAUTH_BETA_HEADER_NAME.to_string(),
            oauth::OAUTH_BETA_HEADER_VALUE.to_string(),
        )));
        assert!(headers.contains(&("x-app".to_string(), "cli".to_string())));
    }

    #[cfg(all(not(target_arch = "wasm32"), feature = "bedrock"))]
    fn bedrock_sigv4_binding(
        options: serde_json::Value,
        base_url: Option<&str>,
        metadata_region: Option<&str>,
    ) -> ValidatedBinding {
        let backend = BackendProfile {
            id: "bedrock-backend".into(),
            provider: Provider::Anthropic,
            backend_kind: AnthropicBackendKind::Bedrock.as_str().into(),
            base_url: base_url.map(str::to_string),
            options,
        };
        let auth = AuthProfile {
            id: "bedrock-auth".into(),
            provider: Provider::Anthropic,
            auth_method: AnthropicAuthMethod::BedrockAwsSigv4.as_str().into(),
            source: meerkat_core::CredentialSourceSpec::PlatformDefault,
            constraints: Default::default(),
            metadata_defaults: meerkat_core::AuthMetadataDefaults {
                provider_metadata: metadata_region.map(|region| {
                    meerkat_core::ProviderAuthMetadata::Anthropic(
                        meerkat_core::AnthropicAuthMetadata {
                            aws_region: Some(region.into()),
                            ..Default::default()
                        },
                    )
                }),
                ..Default::default()
            },
        };
        ProviderRuntimeCatalog::validate_binding(
            &meerkat_core::AuthBindingRef {
                realm: meerkat_core::RealmId::parse("dev").unwrap(),
                binding: meerkat_core::BindingId::parse("bedrock").unwrap(),
                profile: None,
            },
            &backend,
            &auth,
            &BindingPolicy::default(),
        )
        .unwrap()
    }

    #[cfg(all(not(target_arch = "wasm32"), feature = "bedrock"))]
    #[test]
    fn bedrock_region_prefers_typed_auth_metadata_region() {
        let binding = bedrock_sigv4_binding(
            serde_json::json!({ "aws_region": "eu-central-1" }),
            Some("https://bedrock-runtime.us-east-1.amazonaws.com"),
            Some("us-west-2"),
        );

        assert_eq!(bedrock_region(&binding).unwrap(), "us-west-2");
    }

    #[cfg(all(not(target_arch = "wasm32"), feature = "bedrock"))]
    #[test]
    fn bedrock_region_retains_explicit_backend_option_compatibility() {
        let binding = bedrock_sigv4_binding(
            serde_json::json!({ "region": "eu-central-1" }),
            Some("https://bedrock-runtime.us-east-1.amazonaws.com"),
            None,
        );

        assert_eq!(bedrock_region(&binding).unwrap(), "eu-central-1");
    }

    #[cfg(all(not(target_arch = "wasm32"), feature = "bedrock"))]
    #[tokio::test]
    async fn bedrock_sigv4_resolve_records_explicit_region_metadata() {
        let binding = bedrock_sigv4_binding(serde_json::Value::Null, None, Some("ap-southeast-2"));
        let resolved = AnthropicProviderRuntime
            .resolve_binding(&binding, &ResolverEnvironment::testing())
            .await
            .unwrap();

        match &resolved.auth_lease.metadata().provider_metadata {
            Some(meerkat_core::ProviderAuthMetadata::Anthropic(metadata)) => {
                assert_eq!(metadata.aws_region.as_deref(), Some("ap-southeast-2"));
            }
            other => panic!("unexpected provider metadata: {other:?}"),
        }
    }

    #[cfg(all(not(target_arch = "wasm32"), feature = "bedrock"))]
    #[tokio::test]
    async fn bedrock_sigv4_missing_region_fails_instead_of_inferring_from_endpoint() {
        let binding = bedrock_sigv4_binding(
            serde_json::Value::Null,
            Some("https://bedrock-runtime.eu-west-1.amazonaws.com"),
            None,
        );
        let err = AnthropicProviderRuntime
            .resolve_binding(&binding, &ResolverEnvironment::testing())
            .await
            .unwrap_err();

        match err {
            ProviderAuthError::SourceResolutionFailed(message) => {
                assert!(message.contains("requires an explicit AWS signing region"));
                assert!(message.contains("not inferred"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }
}
