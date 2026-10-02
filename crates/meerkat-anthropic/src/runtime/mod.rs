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
    ManagedStoreLifecycle, OAuthLoginCredentialAdmission, load_managed_store_tokens_with_lifecycle,
    prepare_managed_store_oauth_refresh_under_lock, resolve_oauth_login_credential_disposition,
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
    all(not(target_arch = "wasm32"), feature = "foundry"),
    all(not(target_arch = "wasm32"), feature = "copilot")
))]
use meerkat_llm_core::provider_runtime::binding::DynamicLease;
use meerkat_llm_core::provider_runtime::binding::{
    NormalizedAuthMethod, NormalizedBackendKind, ResolvedConnection, ResolvedTextTarget,
    StaticLease, ValidatedBinding,
};
use meerkat_llm_core::provider_runtime::errors::{
    ProviderAuthError, ProviderBindingError, ProviderClientError,
};
use meerkat_llm_core::provider_runtime::registry::ResolverEnvironment;
use meerkat_llm_core::provider_runtime::runtime::ProviderRuntime;

use meerkat_core::lifecycle::run_primitive::AnthropicCacheControlPolicy;

pub use meerkat_core::provider_matrix::anthropic::{AnthropicAuthMethod, AnthropicBackendKind};

/// The runtime's single authority for the Anthropic prompt-cache default.
///
/// The documented default (docs/rust/advanced.mdx): Anthropic's request-wide
/// automatic breakpoint wherever the backend supports it, disabled where it
/// does not. A profile or request opts out per agent with
/// `provider_tag.cache_control = "disabled"`. 0.8.22 flipped this to a
/// blanket disabled default while the docs kept promising automatic; the
/// capability-derived default is the one hosts were told they had. Every
/// client `build_anthropic_client` constructs, including the plain API-key
/// path, takes its default from here; `AnthropicClientBuilder::new` carries
/// the same value only for direct builder users.
fn default_cache_control_for_backend(backend: AnthropicBackendKind) -> AnthropicCacheControlPolicy {
    if backend_supports_automatic_cache_control(backend) {
        AnthropicCacheControlPolicy::Automatic
    } else {
        AnthropicCacheControlPolicy::Disabled
    }
}

/// Whether a backend accepts Anthropic's request-wide automatic cache policy.
///
/// Exhaustive on purpose: automatic caching is a billing-affecting default,
/// so a new `AnthropicBackendKind` must be classified here explicitly (a
/// compile error) instead of silently inheriting automatic cache writes.
fn backend_supports_automatic_cache_control(backend: AnthropicBackendKind) -> bool {
    match backend {
        AnthropicBackendKind::AnthropicApi
        | AnthropicBackendKind::Vertex
        | AnthropicBackendKind::Foundry => true,
        // Amazon Bedrock's Anthropic Messages backend and the GitHub Copilot
        // route accept manual breakpoints but reject the request-wide
        // automatic policy.
        AnthropicBackendKind::Bedrock | AnthropicBackendKind::Copilot => false,
    }
}

#[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
struct ManagedClaudeAiCredential {
    access_token: Arc<str>,
    publication: meerkat_core::auth::lifecycle::TokenLifecyclePublication,
}

#[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
struct ClaudeAiOAuthAuthorizer {
    env: ResolverEnvironment,
    binding: ValidatedBinding,
    metadata: AuthMetadata,
    runtime: oauth::AnthropicOAuthRuntime,
    cached: std::sync::Mutex<Option<ManagedClaudeAiCredential>>,
}

#[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
impl ClaudeAiOAuthAuthorizer {
    async fn new(
        env: &ResolverEnvironment,
        binding: &ValidatedBinding,
        metadata: &AuthMetadata,
        tokens: &meerkat_core::auth::PersistedTokens,
    ) -> Result<Self, ProviderAuthError> {
        let persistence = env
            .provider_auth_persistence()
            .cloned()
            .ok_or(ProviderAuthError::Auth(AuthError::HostOwnedUnavailable))?;
        let key =
            meerkat_core::auth::TokenKey::from_credential_identity(binding.credential_identity());
        let runtime = oauth::AnthropicOAuthRuntime::new(
            persistence,
            oauth::claude_ai_endpoints(oauth::MANUAL_REDIRECT_URL),
            key,
        );
        let mut retained_env = env.clone();
        // Force is a resolution instruction, not a retained per-request policy.
        retained_env.force_refresh = false;
        let authorizer = Self {
            env: retained_env,
            binding: binding.clone(),
            metadata: metadata.clone(),
            runtime,
            cached: std::sync::Mutex::new(None),
        };
        let lease = authorizer.lease_key();
        let guard = meerkat_core::acquire_auth_login_lifecycle_guard(&lease).await;
        let (snapshot, lifecycle) =
            meerkat_auth_core::resolver::observe_existing_managed_store_lifecycle_with_guard(
                &authorizer.env,
                &authorizer.binding,
                &guard,
            )?;
        if lifecycle != ManagedStoreLifecycle::Authorized {
            return Err(ProviderAuthError::Auth(AuthError::RefreshRequired));
        }
        let cached = authorizer
            .credential_from_tokens(tokens, &snapshot)
            .map_err(ProviderAuthError::Auth)?;
        *authorizer
            .cached
            .lock()
            .map_err(|_| ProviderAuthError::Auth(AuthError::HostOwnedUnavailable))? = Some(cached);
        drop(guard);
        Ok(authorizer)
    }

    fn lease_key(&self) -> meerkat_core::handles::LeaseKey {
        meerkat_core::handles::LeaseKey::from_credential_identity(
            self.binding.credential_identity(),
        )
    }

    fn provider_error(error: ProviderAuthError) -> AuthError {
        match error {
            ProviderAuthError::Auth(error) => error,
            other => AuthError::RefreshFailed(other.to_string()),
        }
    }

    fn credential_from_tokens(
        &self,
        tokens: &meerkat_core::auth::PersistedTokens,
        snapshot: &meerkat_core::handles::AuthLeaseSnapshot,
    ) -> Result<ManagedClaudeAiCredential, AuthError> {
        use meerkat_core::generated::auth_lease_durable_lifecycle_marker as marker;
        if marker::marker_relation_for_tokens_and_snapshot(tokens, snapshot, self.runtime.key())
            != marker::AuthLeaseDurableMarkerRelation::Matches
        {
            return Err(AuthError::StaleCredential);
        }
        // These providers select account metadata from optional ID-token claims.
        // An absent optional claim does not change the retained selected route.
        let claimed_account = tokens
            .id_token
            .as_deref()
            .and_then(|token| meerkat_auth_core::auth_oauth::jwt::decode_payload(token).ok())
            .and_then(|claims| {
                let lifted = oauth::AnthropicIdClaims::lift_from_claims(&claims.raw);
                lifted.user_id.or(lifted.email)
            });
        if claimed_account.is_some() && claimed_account != self.metadata.account_id {
            return Err(AuthError::ResolveRequired(
                "managed Claude.ai credential no longer matches the selected account".into(),
            ));
        }
        let publication =
            meerkat_core::tokens_lifecycle_publication(tokens).ok_or(AuthError::StaleCredential)?;
        let access_token = tokens
            .primary_secret
            .as_deref()
            .ok_or(AuthError::MissingSecret)?;
        Ok(ManagedClaudeAiCredential {
            access_token: Arc::from(access_token),
            publication,
        })
    }

    fn cache_matches(
        cached: &ManagedClaudeAiCredential,
        current: &meerkat_core::handles::AuthLeaseSnapshot,
    ) -> bool {
        current.credential_present
            && cached.publication.generation == Some(current.generation)
            && Some(cached.publication.expires_at) == current.expires_at
            && cached.publication.credential_published_at_millis
                == current.credential_published_at_millis
    }

    // The returned guard keeps the actual owner current through the caller's
    // synchronous header copy. No cache guard or lifecycle guard spans HTTP.
    async fn current_credential(
        &self,
    ) -> Result<(meerkat_core::AuthLoginLifecycleGuard, Arc<str>), AuthError> {
        let lease = self.lease_key();
        let guard = meerkat_core::acquire_auth_login_lifecycle_guard(&lease).await;
        let (snapshot, lifecycle) =
            match meerkat_auth_core::resolver::observe_existing_managed_store_lifecycle_with_guard(
                &self.env,
                &self.binding,
                &guard,
            ) {
                Ok(current) => current,
                Err(error) => {
                    *self
                        .cached
                        .lock()
                        .map_err(|_| AuthError::HostOwnedUnavailable)? = None;
                    return Err(Self::provider_error(error));
                }
            };
        if lifecycle == ManagedStoreLifecycle::Authorized {
            let cache = self
                .cached
                .lock()
                .map_err(|_| AuthError::HostOwnedUnavailable)?;
            if let Some(cached) = cache
                .as_ref()
                .filter(|cached| Self::cache_matches(cached, &snapshot))
            {
                return Ok((guard, Arc::clone(&cached.access_token)));
            }
        }
        drop(guard);

        let mut loaded =
            meerkat_auth_core::resolver::load_existing_managed_store_tokens_with_lifecycle(
                &self.env,
                &self.binding,
            )
            .await
            .map_err(Self::provider_error)?;
        match resolve_oauth_login_credential_disposition(
            &self.env,
            &self.binding,
            loaded.tokens.primary_secret.is_some(),
        )
        .map_err(Self::provider_error)?
        {
            OAuthLoginCredentialAdmission::UseCached => {}
            OAuthLoginCredentialAdmission::BeginRefresh => {
                loaded.release_prelock_lifecycle_guard();
                let env = self.env.clone();
                let binding = self.binding.clone();
                let prepare: oauth::TokenPrepareFn = Box::new(move |locked, mode| {
                    Box::pin(async move {
                        meerkat_auth_core::resolver::prepare_existing_managed_store_oauth_refresh_under_lock(
                            &env, &binding, loaded, locked, mode,
                        ).await.map_err(meerkat_auth_core::resolver::refresh_error_from_provider)
                    })
                });
                self.runtime
                    .refresh_tokens_with_locked_preparation(prepare, false)
                    .await
                    .map_err(|error| {
                        Self::provider_error(anthropic_oauth_refresh_error(error, String::new()))
                    })?;
                // A returned exchange value is not cache authority. Re-read only
                // on this cold path and bind the exact committed current owner.
                loaded =
                    meerkat_auth_core::resolver::load_existing_managed_store_tokens_with_lifecycle(
                        &self.env,
                        &self.binding,
                    )
                    .await
                    .map_err(Self::provider_error)?;
            }
        }
        let guard = loaded
            .lifecycle_guard
            .take()
            .ok_or(AuthError::LeaseAbsent)?;
        let (snapshot, lifecycle) =
            meerkat_auth_core::resolver::observe_existing_managed_store_lifecycle_with_guard(
                &self.env,
                &self.binding,
                &guard,
            )
            .map_err(Self::provider_error)?;
        if lifecycle != ManagedStoreLifecycle::Authorized {
            return Err(AuthError::RefreshRequired);
        }
        let cached = self.credential_from_tokens(&loaded.tokens, &snapshot)?;
        let access_token = Arc::clone(&cached.access_token);
        *self
            .cached
            .lock()
            .map_err(|_| AuthError::HostOwnedUnavailable)? = Some(cached);
        Ok((guard, access_token))
    }
}

#[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
#[async_trait]
impl meerkat_core::HttpAuthorizer for ClaudeAiOAuthAuthorizer {
    async fn prepare_request(&self) -> Result<(), AuthError> {
        self.current_credential().await.map(|_| ())
    }

    async fn authorize(
        &self,
        req: &mut meerkat_core::HttpAuthorizationRequest<'_>,
    ) -> Result<(), AuthError> {
        let (_guard, access_token) = self.current_credential().await?;
        req.headers
            .push(("Authorization".into(), format!("Bearer {access_token}")));
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

    fn persistence_authority_id(&self) -> Option<meerkat_core::auth::ProviderAuthPersistenceId> {
        self.env
            .provider_auth_persistence()
            .map(|persistence| persistence.authority_id())
    }

    fn expires_at(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        let snapshot = self
            .env
            .auth_lease_handle
            .as_ref()?
            .snapshot(&self.lease_key());
        chrono::DateTime::from_timestamp(i64::try_from(snapshot.expires_at?).ok()?, 0)
    }
}

#[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
fn anthropic_oauth_refresh_error(
    error: oauth::AnthropicOAuthError,
    authmachine_failure: String,
) -> ProviderAuthError {
    if matches!(
        &error,
        oauth::AnthropicOAuthError::Refresh(meerkat_auth_core::RefreshError::StalePreparation)
    ) {
        return ProviderAuthError::Auth(AuthError::StaleCredential);
    }
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

#[derive(Default)]
pub struct AnthropicProviderRuntime {
    #[cfg(all(feature = "copilot", not(target_arch = "wasm32")))]
    copilot: Option<Arc<meerkat_copilot::CopilotRuntime>>,
}

#[allow(non_upper_case_globals)]
pub const AnthropicProviderRuntime: AnthropicProviderRuntime = AnthropicProviderRuntime {
    #[cfg(all(feature = "copilot", not(target_arch = "wasm32")))]
    copilot: None,
};

impl AnthropicProviderRuntime {
    #[cfg(all(feature = "copilot", not(target_arch = "wasm32")))]
    pub fn with_copilot(copilot: Arc<meerkat_copilot::CopilotRuntime>) -> Self {
        Self {
            copilot: Some(copilot),
        }
    }
}

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
            AnthropicAuthMethod::GitHubCopilotOauth => {
                #[cfg(all(feature = "copilot", not(target_arch = "wasm32")))]
                {
                    let runtime = self.copilot.as_ref().ok_or_else(|| {
                        ProviderAuthError::SourceResolutionFailed(
                            "Anthropic Copilot backend is not composed with CopilotRuntime"
                                .to_string(),
                        )
                    })?;
                    let resolved = runtime.resolve(binding, env).await?;
                    Arc::new(DynamicLease::from_authorizer(
                        resolved.authorizer(),
                        resolved.metadata().clone(),
                        meerkat_copilot::GITHUB_COPILOT_AUTHORIZER_LABEL,
                    ))
                }
                #[cfg(not(all(feature = "copilot", not(target_arch = "wasm32"))))]
                {
                    return Err(ProviderAuthError::SourceResolutionFailed(
                        "Anthropic Copilot backend is not compiled".to_string(),
                    ));
                }
            }
            AnthropicAuthMethod::BedrockAwsSigv4 => {
                #[cfg(all(not(target_arch = "wasm32"), feature = "bedrock"))]
                {
                    let region = bedrock_region(binding)?;
                    let lookup = env.env_lookup.clone();
                    let mut aws_authorizer = meerkat_auth_core::authorizers::AwsStsAuthorizer::new(
                        region.clone(),
                        meerkat_auth_core::authorizers::AwsCredentialProvider::from_env(
                            move |key| lookup(key),
                        ),
                    );
                    if let Some(handle) = env.auth_lease_handle.clone() {
                        // Bind SigV4 signing to the per-binding AuthMachine
                        // lease so credential freshness is consulted before
                        // signing instead of trusting wall-clock time.
                        aws_authorizer = aws_authorizer.with_auth_lease_observer(
                            handle,
                            meerkat_core::handles::LeaseKey::from_auth_binding(
                                binding.auth_binding_ref(),
                            ),
                        );
                    }
                    let authorizer: Arc<dyn HttpAuthorizer> = Arc::new(aws_authorizer);
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
                            // The cached-vs-refresh disposition is owned by the
                            // per-binding AuthMachine: feed only the pure
                            // observations (persisted secret presence,
                            // force_refresh, refresh-allowed config) and mirror
                            // the verdict. UseCached -> persisted; BeginRefresh ->
                            // run the OAuth refresh; refresh-disallowed/reauth ->
                            // the matching error surfaced by the resolver.
                            match resolve_oauth_login_credential_disposition(
                                env,
                                binding,
                                persisted.primary_secret.is_some(),
                            )? {
                                OAuthLoginCredentialAdmission::UseCached => {
                                    managed.release_prelock_lifecycle_guard();
                                    persisted
                                }
                                OAuthLoginCredentialAdmission::BeginRefresh => {
                                    managed.release_prelock_lifecycle_guard();
                                    let persistence = env
                                        .provider_auth_persistence()
                                        .cloned()
                                        .ok_or_else(|| {
                                        ProviderAuthError::SourceResolutionFailed(
                                            "managed_store OAuth requires provider-auth persistence authority"
                                                .into(),
                                        )
                                    })?;
                                    let endpoints =
                                        oauth::claude_ai_endpoints(oauth::MANUAL_REDIRECT_URL);
                                    let runtime = oauth::AnthropicOAuthRuntime::new(
                                        persistence,
                                        endpoints,
                                        managed.key.clone(),
                                    );
                                    let prepare_env = env.clone();
                                    let prepare_binding = binding.clone();
                                    let prepare: oauth::TokenPrepareFn = Box::new(
                                        move |locked_baseline, mode| {
                                            Box::pin(async move {
                                                prepare_managed_store_oauth_refresh_under_lock(
                                                    &prepare_env,
                                                    &prepare_binding,
                                                    managed,
                                                    locked_baseline,
                                                    mode,
                                                )
                                                .await
                                                .map_err(meerkat_auth_core::resolver::refresh_error_from_provider)
                                            })
                                        },
                                    );
                                    runtime
                                        .refresh_tokens_with_locked_preparation(
                                            prepare,
                                            env.force_refresh,
                                        )
                                        .await
                                        .map_err(|error| {
                                            anthropic_oauth_refresh_error(error, String::new())
                                        })?
                                }
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
                            let authorizer = ClaudeAiOAuthAuthorizer::new(
                                env,
                                binding,
                                &metadata,
                                &effective_tokens,
                            )
                            .await?;
                            Arc::new(DynamicLease::from_authorizer(
                                Arc::new(authorizer),
                                metadata,
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
            credential_identity: binding.credential_identity().clone(),
            auth_lease: lease,
        })
    }

    fn build_client(
        &self,
        connection: ResolvedConnection,
    ) -> Result<Arc<dyn LlmClient>, ProviderClientError> {
        let client = build_anthropic_client(connection)?;
        Ok(Arc::new(client))
    }

    fn build_text_client(
        &self,
        target: ResolvedTextTarget,
    ) -> Result<Arc<dyn LlmClient>, ProviderClientError> {
        if !matches!(
            target.connection().backend,
            NormalizedBackendKind::Anthropic(AnthropicBackendKind::Copilot)
        ) {
            let (_, _, connection) = target.into_parts();
            return self.build_client(connection);
        }
        #[cfg(all(feature = "copilot", not(target_arch = "wasm32")))]
        {
            let runtime = self.copilot.as_ref().ok_or_else(|| {
                ProviderClientError::ClientInit(
                    "Anthropic Copilot backend is not composed with CopilotRuntime".to_string(),
                )
            })?;
            let (identity, _, connection) = target.into_parts();
            let model = identity.model.clone();
            let factory: meerkat_copilot::CopilotRouteClientFactory =
                Arc::new(move |route, connection| {
                    let endpoint = match route.access {
                        meerkat_copilot::CopilotModelAccess::Available { endpoint } => endpoint,
                        meerkat_copilot::CopilotModelAccess::Unknown => {
                            meerkat_copilot::CopilotEndpoint::optimistic_for_provider(
                                Provider::Anthropic,
                            )
                            .ok_or_else(|| {
                                ProviderClientError::ClientInit(
                                    "Copilot has no optimistic Anthropic route".to_string(),
                                )
                            })?
                        }
                        meerkat_copilot::CopilotModelAccess::Unavailable => {
                            return Err(ProviderClientError::ClientInit(
                                route.unavailable_message(
                                    Provider::Anthropic,
                                    &model,
                                    meerkat_copilot::CopilotEndpoint::Messages,
                                ),
                            ));
                        }
                    };
                    if endpoint != meerkat_copilot::CopilotEndpoint::Messages {
                        return Err(ProviderClientError::ClientInit(route.unavailable_message(
                            Provider::Anthropic,
                            &model,
                            meerkat_copilot::CopilotEndpoint::Messages,
                        )));
                    }
                    let authorizer = connection
                        .resolved_authorizer()
                        .ok_or(ProviderClientError::NoCredentialMaterial)?;
                    let authorizer = route.bind_authorizer(authorizer);
                    let client = crate::AnthropicClient::builder(String::new())
                        .authorizer(authorizer)
                        .base_url(route.api_base.clone())
                        .default_cache_control(default_cache_control_for_backend(
                            AnthropicBackendKind::Copilot,
                        ))
                        .automatic_cache_control_supported(
                            backend_supports_automatic_cache_control(AnthropicBackendKind::Copilot),
                        )
                        .build()
                        .map_err(ProviderClientError::from)?;
                    Ok(Arc::new(client) as Arc<dyn LlmClient>)
                });
            meerkat_copilot::routed_client(
                Arc::clone(runtime),
                connection,
                Provider::Anthropic,
                identity.model,
                factory,
            )
        }
        #[cfg(not(all(feature = "copilot", not(target_arch = "wasm32"))))]
        {
            let _ = target;
            Err(ProviderClientError::MissingFeature("copilot"))
        }
    }
}

/// Build the concrete Anthropic client for a resolved connection.
///
/// Every arm routes through `AnthropicClientBuilder` with the cache defaults
/// derived by [`default_cache_control_for_backend`] and
/// [`backend_supports_automatic_cache_control`], so those two helpers are the
/// single runtime authority for the Anthropic cache default. `build_client`
/// erases the type at the `ProviderRuntime` seam; the runtime tests drive this
/// function directly to read the request body a backend's default produces.
fn build_anthropic_client(
    connection: ResolvedConnection,
) -> Result<crate::AnthropicClient, ProviderClientError> {
    // ProviderRuntimeRegistry dispatches on Provider enum, so this
    // runtime only receives Anthropic-backend connections. The match
    // is defensive; the non-Anthropic arms are unreachable at runtime.
    let backend_kind = match connection.backend {
        NormalizedBackendKind::Anthropic(k) => k,
        other => unreachable!(
            "AnthropicProviderRuntime received non-Anthropic backend: {other:?} \
             - registry dispatch invariant violated"
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
                .unwrap_or_else(|| AnthropicBackendKind::AnthropicApi.default_base_url().into()),
            AnthropicBackendKind::Copilot => {
                return Err(ProviderClientError::MissingFeature("copilot-text-target"));
            }
        };
        let client = crate::AnthropicClient::builder(String::new())
            .authorizer(authorizer)
            .base_url(base_url)
            .default_cache_control(default_cache_control_for_backend(backend_kind))
            .automatic_cache_control_supported(backend_supports_automatic_cache_control(
                backend_kind,
            ))
            .build()
            .map_err(ProviderClientError::from)?;
        return Ok(client);
    }

    #[cfg(target_arch = "wasm32")]
    if authorizer_opt.is_some() {
        return Err(ProviderClientError::MissingFeature(
            "authorizer-backed auth not available on wasm32",
        ));
    }

    let secret = secret_opt.ok_or(ProviderClientError::NoCredentialMaterial)?;

    match backend_kind {
        // Native Anthropic API (and Foundry API-key auth): plain x-api-key
        // auth through the same builder path as every other backend, so the
        // cache default comes from `default_cache_control_for_backend`
        // rather than from the builder's own constant.
        AnthropicBackendKind::AnthropicApi | AnthropicBackendKind::Foundry => {
            let mut builder = crate::AnthropicClient::builder(secret)
                .default_cache_control(default_cache_control_for_backend(backend_kind))
                .automatic_cache_control_supported(backend_supports_automatic_cache_control(
                    backend_kind,
                ));
            if let Some(url) = &connection.backend_profile.base_url {
                builder = builder.base_url(url.clone());
            }
            let client = builder.build().map_err(ProviderClientError::from)?;
            Ok(client)
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
                std::sync::Arc::new(meerkat_auth_core::authorizers::StaticBearerAuthorizer::new(
                    secret,
                    "bedrock-bearer",
                ));
            let client = crate::AnthropicClient::builder(String::new())
                .authorizer(authorizer)
                .base_url(base_url)
                .default_cache_control(default_cache_control_for_backend(backend_kind))
                .automatic_cache_control_supported(backend_supports_automatic_cache_control(
                    backend_kind,
                ))
                .build()
                .map_err(ProviderClientError::from)?;
            Ok(client)
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
                std::sync::Arc::new(meerkat_auth_core::authorizers::StaticBearerAuthorizer::new(
                    secret,
                    "vertex-bearer",
                ));
            let client = crate::AnthropicClient::builder(String::new())
                .authorizer(authorizer)
                .base_url(base_url)
                .default_cache_control(default_cache_control_for_backend(backend_kind))
                .automatic_cache_control_supported(backend_supports_automatic_cache_control(
                    backend_kind,
                ))
                .build()
                .map_err(ProviderClientError::from)?;
            Ok(client)
        }
        #[cfg(target_arch = "wasm32")]
        AnthropicBackendKind::Vertex => Err(ProviderClientError::MissingFeature(
            "vertex-backend with authorizer-backed auth not available on wasm32",
        )),
        AnthropicBackendKind::Copilot => {
            Err(ProviderClientError::MissingFeature("copilot-text-target"))
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

    #[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
    mod managed_authorizer_controls {
        use super::*;
        use meerkat_auth_core::{EphemeralTokenStore, InMemoryCoordinator};
        use meerkat_core::auth::{
            PersistedTokens, ProviderAuthPersistence, TokenKey, TokenStore, TokenStoreError,
        };
        use meerkat_core::handles::{GeneratedAuthLeaseHandle, LeaseKey};
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct CountingStore {
            inner: EphemeralTokenStore,
            calls: AtomicUsize,
        }

        #[async_trait::async_trait]
        impl TokenStore for CountingStore {
            async fn load(
                &self,
                key: &TokenKey,
            ) -> Result<Option<PersistedTokens>, TokenStoreError> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                self.inner.load(key).await
            }
            async fn save(
                &self,
                key: &TokenKey,
                tokens: &PersistedTokens,
            ) -> Result<(), TokenStoreError> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                self.inner.save(key, tokens).await
            }
            async fn clear(&self, key: &TokenKey) -> Result<(), TokenStoreError> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                self.inner.clear(key).await
            }
            async fn list(&self) -> Result<Vec<TokenKey>, TokenStoreError> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                self.inner.list().await
            }
            fn backend_name(&self) -> &'static str {
                self.inner.backend_name()
            }
        }

        pub(super) struct Fixture {
            pub(super) authorizer: ClaudeAiOAuthAuthorizer,
            persistence: ProviderAuthPersistence,
            owner: GeneratedAuthLeaseHandle,
            identity: meerkat_core::AuthCredentialIdentity,
            lease: LeaseKey,
            store: Arc<CountingStore>,
            tokens: PersistedTokens,
        }

        pub(super) async fn fixture(name: &str, account: Option<&str>) -> Fixture {
            let reference = meerkat_core::AuthBindingRef {
                realm: meerkat_core::RealmId::parse("anthropic-authorizer-controls").unwrap(),
                binding: meerkat_core::BindingId::parse(name).unwrap(),
                profile: None,
                origin: meerkat_core::BindingOrigin::Configured,
            };
            let binding = ProviderRuntimeCatalog::validate_binding(
                &reference,
                &meerkat_core::BackendProfile {
                    id: "managed-control".into(),
                    provider: Provider::Anthropic,
                    backend_kind: "anthropic_api".into(),
                    base_url: Some("http://127.0.0.1:9".into()),
                    options: serde_json::Value::Null,
                    server: None,
                },
                &meerkat_core::AuthProfile {
                    id: "managed-control".into(),
                    provider: Provider::Anthropic,
                    auth_method: "claude_ai_oauth".into(),
                    source: meerkat_core::CredentialSourceSpec::ManagedStore,
                    constraints: Default::default(),
                    metadata_defaults: Default::default(),
                },
                &meerkat_core::BindingPolicy::default(),
            )
            .unwrap();
            let identity = binding.credential_identity().clone();
            let key = TokenKey::from_credential_identity(&identity);
            let lease = LeaseKey::from_credential_identity(&identity);
            let owner = meerkat_runtime::protocol_auth_lease_lifecycle_publication::generated_auth_lease_handle(
                Arc::new(meerkat_runtime::RuntimeAuthLeaseHandle::new()),
            ).unwrap();
            let store = Arc::new(CountingStore {
                inner: EphemeralTokenStore::new(),
                calls: AtomicUsize::new(0),
            });
            let token_store: Arc<dyn TokenStore> = store.clone();
            let persistence = ProviderAuthPersistence::new(
                token_store.clone(),
                Arc::new(InMemoryCoordinator::new()),
            );
            let now = chrono::Utc::now();
            let expiry = now + chrono::Duration::hours(1);
            let tokens = PersistedTokens {
                auth_mode: PersistedAuthMode::ClaudeAiOauth,
                primary_secret: Some("tok-claude".into()),
                refresh_token: Some("refresh-control".into()),
                id_token: account.map(|_| "header.eyJzdWIiOiJzYW1lLWFjY291bnQifQ.signature".into()),
                expires_at: Some(expiry),
                last_refresh: Some(now),
                scopes: Vec::new(),
                // These providers project account metadata from optional ID-token claims.
                account_id: Some("stored-account-is-not-the-selected-claim".into()),
                metadata: serde_json::Value::Null,
            };
            let guard = meerkat_core::acquire_auth_login_lifecycle_guard(&lease).await;
            let transition = owner
                .acquire_lease(&lease, expiry.timestamp() as u64)
                .unwrap();
            let tokens = meerkat_core::mark_tokens_lifecycle_published_for_transition(
                &key,
                &tokens,
                &transition,
            )
            .unwrap();
            token_store.save(&key, &tokens).await.unwrap();
            drop(guard);
            let mut env = ResolverEnvironment::testing()
                .with_provider_auth_persistence(persistence.clone())
                .with_auth_lease_handle(owner.clone());
            env.now = Arc::new(move || now);
            env.force_refresh = true;
            let metadata = AuthMetadata {
                account_id: account.map(str::to_string),
                ..Default::default()
            };
            let mut authorizer = ClaudeAiOAuthAuthorizer::new(&env, &binding, &metadata, &tokens)
                .await
                .unwrap();
            // Bound even an erroneous refresh attempt to a local endpoint, without global env.
            let mut endpoints = oauth::claude_ai_endpoints(oauth::MANUAL_REDIRECT_URL);
            endpoints.token_url = "http://127.0.0.1:9/token".into();
            authorizer.runtime =
                oauth::AnthropicOAuthRuntime::new(persistence.clone(), endpoints, key);
            Fixture {
                authorizer,
                persistence,
                owner,
                identity,
                lease,
                store,
                tokens,
            }
        }

        #[tokio::test]
        async fn prepare_uses_current_cache_and_refuses_released_owner_without_store_io() {
            let fixture = fixture("prepare", None).await;
            assert!(!fixture.authorizer.env.force_refresh);
            let before = fixture.owner.snapshot(&fixture.lease);
            let calls = fixture.store.calls.load(Ordering::SeqCst);
            assert!(
                calls > 0,
                "the counter observed the actual initial token save"
            );
            tokio::time::timeout(
                std::time::Duration::from_secs(2),
                fixture.authorizer.prepare_request(),
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(fixture.store.calls.load(Ordering::SeqCst), calls);
            assert_eq!(fixture.owner.snapshot(&fixture.lease), before);
            meerkat_core::clear_tokens_and_publish_lifecycle_released_coordinated_for_identity(
                fixture.persistence.clone(),
                fixture.owner.clone(),
                fixture.identity.clone(),
            )
            .await
            .unwrap();
            let released = fixture.owner.snapshot(&fixture.lease);
            assert!(!released.credential_present);
            let calls = fixture.store.calls.load(Ordering::SeqCst);
            let result = tokio::time::timeout(
                std::time::Duration::from_secs(2),
                fixture.authorizer.prepare_request(),
            )
            .await
            .unwrap();
            assert!(matches!(result, Err(AuthError::LeaseAbsent)));
            let mut headers = Vec::new();
            let mut request = meerkat_core::HttpAuthorizationRequest {
                method: "POST",
                url: "http://127.0.0.1:9/model",
                headers: &mut headers,
            };
            let result = tokio::time::timeout(
                std::time::Duration::from_secs(2),
                fixture.authorizer.authorize(&mut request),
            )
            .await
            .unwrap();
            assert!(matches!(result, Err(AuthError::LeaseAbsent)));
            assert!(headers.is_empty());
            assert_eq!(fixture.store.calls.load(Ordering::SeqCst), calls);
            assert_eq!(fixture.owner.snapshot(&fixture.lease), released);
        }

        #[tokio::test]
        async fn optional_account_claim_may_be_absent_but_cannot_repin_the_authorizer() {
            let fixture = fixture("claims", Some("same-account")).await;
            let snapshot = fixture.owner.snapshot(&fixture.lease);
            assert!(
                fixture
                    .authorizer
                    .credential_from_tokens(&fixture.tokens, &snapshot)
                    .is_ok()
            );
            let mut absent = fixture.tokens.clone();
            absent.id_token = None;
            assert!(
                fixture
                    .authorizer
                    .credential_from_tokens(&absent, &snapshot)
                    .is_ok()
            );
            let mut conflict = fixture.tokens.clone();
            conflict.id_token = Some("header.eyJzdWIiOiJvdGhlci1hY2NvdW50In0.signature".into());
            assert!(matches!(
                fixture
                    .authorizer
                    .credential_from_tokens(&conflict, &snapshot),
                Err(AuthError::ResolveRequired(_))
            ));
            assert_eq!(
                fixture.authorizer.metadata.account_id.as_deref(),
                Some("same-account")
            );
            assert_eq!(fixture.owner.snapshot(&fixture.lease), snapshot);
        }
    }

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

    /// The documented default: automatic prompt caching wherever the backend
    /// supports Anthropic's request-wide breakpoint, disabled where it does not.
    /// 0.8.22 inverted this test to pin a blanket disabled default; the docs
    /// never followed, so the capability-derived default is restored. Every
    /// variant in `AnthropicBackendKind::ALL` is classified, and the
    /// automatic-unsupported set is exactly Bedrock and Copilot.
    #[test]
    fn cache_control_defaults_to_automatic_where_the_backend_supports_it() {
        let unsupported: Vec<AnthropicBackendKind> = AnthropicBackendKind::ALL
            .iter()
            .copied()
            .filter(|backend| !backend_supports_automatic_cache_control(*backend))
            .collect();
        assert_eq!(
            unsupported,
            vec![AnthropicBackendKind::Bedrock, AnthropicBackendKind::Copilot],
            "only Bedrock and Copilot reject the request-wide automatic policy"
        );
        for backend in AnthropicBackendKind::ALL.iter().copied() {
            let expected = if backend_supports_automatic_cache_control(backend) {
                AnthropicCacheControlPolicy::Automatic
            } else {
                AnthropicCacheControlPolicy::Disabled
            };
            assert_eq!(
                default_cache_control_for_backend(backend),
                expected,
                "{backend:?} must default to {expected:?}"
            );
        }
        assert_eq!(
            default_cache_control_for_backend(AnthropicBackendKind::AnthropicApi),
            AnthropicCacheControlPolicy::Automatic
        );
        assert_eq!(
            default_cache_control_for_backend(AnthropicBackendKind::Bedrock),
            AnthropicCacheControlPolicy::Disabled
        );
    }

    /// A resolved connection whose lease is an inline secret, which is what the
    /// API-key and Bedrock static-bearer paths receive from `resolve_binding`.
    #[cfg(not(target_arch = "wasm32"))]
    fn inline_secret_connection(
        backend: AnthropicBackendKind,
        base_url: Option<&str>,
    ) -> ResolvedConnection {
        use meerkat_core::{
            AuthBindingRef, AuthCredentialIdentity, AuthMetadata, BackendProfile, BindingId,
            BindingOrigin, RealmId,
        };

        ResolvedConnection {
            provider: Provider::Anthropic,
            backend: NormalizedBackendKind::Anthropic(backend),
            backend_profile: Arc::new(BackendProfile {
                id: format!("{}-backend", backend.as_str()),
                provider: Provider::Anthropic,
                backend_kind: backend.as_str().to_string(),
                base_url: base_url.map(str::to_string),
                options: serde_json::Value::Null,
                server: None,
            }),
            credential_identity: AuthCredentialIdentity::Binding(AuthBindingRef {
                realm: RealmId::parse("dev").unwrap(),
                binding: BindingId::parse("primary").unwrap(),
                profile: None,
                origin: BindingOrigin::Configured,
            }),
            auth_lease: Arc::new(StaticLease::inline_secret(
                "test-key".to_string(),
                AuthMetadata::default(),
                None,
                "test-inline-secret",
            )),
        }
    }

    /// The runtime-selected defaults reach the wire through the real
    /// `build_anthropic_client` seam: the API-key connection (the path most
    /// hosts take, previously built through `AnthropicClient::new` and its
    /// independent builder constant) emits the request-wide automatic
    /// breakpoint and a Bedrock connection emits none.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn runtime_backend_defaults_drive_the_request_body() {
        use meerkat_core::{Message, UserMessage};
        use meerkat_llm_core::LlmRequest;

        let request = LlmRequest::new(
            "claude-sonnet-4-6",
            vec![Message::User(UserMessage::text("hello"))],
        );

        let api_key_client = build_anthropic_client(inline_secret_connection(
            AnthropicBackendKind::AnthropicApi,
            None,
        ))
        .expect("api-key client");
        let api_key_body = api_key_client
            .build_request_body(&request)
            .expect("api-key request body");
        assert_eq!(
            api_key_body["cache_control"],
            serde_json::json!({"type": "ephemeral"}),
            "API-key backend defaults to the automatic breakpoint: {api_key_body}"
        );

        let bedrock_client = build_anthropic_client(inline_secret_connection(
            AnthropicBackendKind::Bedrock,
            Some("https://bedrock-runtime.us-east-1.amazonaws.com"),
        ))
        .expect("bedrock client");
        let bedrock_body = bedrock_client
            .build_request_body(&request)
            .expect("bedrock request body");
        assert!(
            bedrock_body.get("cache_control").is_none(),
            "Bedrock stays disabled by default: {bedrock_body}"
        );
    }

    #[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
    #[tokio::test]
    async fn claude_ai_oauth_authorizer_sets_bearer_and_beta_headers() {
        let fixture = managed_authorizer_controls::fixture("headers", None).await;
        let authorizer = fixture.authorizer;
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
            server: None,
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
                origin: meerkat_core::BindingOrigin::Configured,
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

#[cfg(all(test, not(target_arch = "wasm32"), feature = "oauth"))]
#[test]
fn ce_stale_refresh_remains_stale_credential() {
    let result = anthropic_oauth_refresh_error(
        oauth::AnthropicOAuthError::Refresh(meerkat_auth_core::RefreshError::StalePreparation),
        String::new(),
    );
    assert!(matches!(
        result,
        ProviderAuthError::Auth(AuthError::StaleCredential)
    ));
}

#[cfg(all(test, feature = "oauth", not(target_arch = "wasm32")))]
#[path = "managed_lifetime_tests.rs"]
mod managed_lifetime_tests;
