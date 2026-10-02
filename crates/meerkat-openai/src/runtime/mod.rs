//! OpenAI provider runtime.

#[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
pub mod oauth;

use std::sync::Arc;

use async_trait::async_trait;

/// Catalog model family served by the public Live broker rather than the
/// Realtime WebSocket factory.
pub const GPT_LIVE_MODEL_FAMILY: &str = "gpt-live";

#[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
use meerkat_core::AuthError;
use meerkat_core::{AuthLease, AuthMetadata, Provider};

#[cfg(not(all(not(target_arch = "wasm32"), feature = "oauth")))]
use meerkat_auth_core::resolver::interactive_login_error;
#[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
use meerkat_auth_core::resolver::{
    ManagedStoreLifecycle, OAuthLoginCredentialAdmission, load_managed_store_tokens_with_lifecycle,
    prepare_managed_store_oauth_refresh_under_lock, resolve_oauth_login_credential_disposition,
};
use meerkat_auth_core::resolver::{
    finalize_auth_metadata, observe_simple_secret_readiness, resolve_external_authorizer,
    resolve_simple_secret,
};
#[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
use meerkat_auth_core::{
    auth_store::PersistedAuthMode, oauth_flow::validate_oauth_target_for_auth_mode,
};
#[cfg(all(
    not(target_arch = "wasm32"),
    any(feature = "copilot", feature = "oauth")
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
use meerkat_llm_core::provider_runtime::runtime::{CredentialReadiness, ProviderRuntime};
use meerkat_llm_core::{ImageGenerationExecutor, LlmClient};

use crate::client::AzureOpenAiWireConfig;

pub use meerkat_core::provider_matrix::openai::{OpenAiAuthMethod, OpenAiBackendKind};

#[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
fn openai_oauth_refresh_error(
    error: oauth::OpenAiOAuthError,
    authmachine_failure: String,
) -> ProviderAuthError {
    if matches!(
        &error,
        oauth::OpenAiOAuthError::Refresh(meerkat_auth_core::RefreshError::StalePreparation)
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
            oauth::OpenAiOAuthError::InteractiveLoginRequired
            | oauth::OpenAiOAuthError::MissingRefreshToken => {
                return ProviderAuthError::Auth(AuthError::UserReauthRequired);
            }
            _ => {}
        }
    }
    ProviderAuthError::Auth(AuthError::RefreshFailed(detail))
}

#[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
fn chatgpt_metadata_from_tokens(tokens: &meerkat_core::auth::PersistedTokens) -> AuthMetadata {
    let mut chatgpt_account_id = tokens.account_id.clone();
    let mut chatgpt_user_id: Option<String> = None;
    let mut chatgpt_email: Option<String> = None;
    let mut chatgpt_is_fedramp: Option<bool> = None;
    let mut chatgpt_plan_type: Option<String> = None;
    if let Some(id_token) = tokens.id_token.as_deref()
        && let Ok(claims) = meerkat_auth_core::auth_oauth::jwt::decode_payload(id_token)
    {
        let lifted = oauth::ChatGptIdClaims::lift_from_claims(&claims.raw);
        if chatgpt_account_id.is_none() {
            chatgpt_account_id = lifted.account_id;
        }
        chatgpt_user_id = lifted.user_id;
        chatgpt_email = lifted.email;
        chatgpt_is_fedramp = lifted.is_fedramp;
        chatgpt_plan_type = lifted.plan_type;
    }
    let mut metadata = AuthMetadata::default();
    if chatgpt_account_id.is_some()
        || chatgpt_user_id.is_some()
        || chatgpt_email.is_some()
        || chatgpt_is_fedramp.is_some()
        || chatgpt_plan_type.is_some()
    {
        metadata.account_id = chatgpt_account_id.clone();
        metadata.plan = chatgpt_plan_type.clone();
        metadata.provider_metadata = Some(meerkat_core::ProviderAuthMetadata::OpenAi(
            meerkat_core::OpenAiAuthMetadata {
                plan_type: chatgpt_plan_type,
                user_id: chatgpt_user_id,
                account_id: chatgpt_account_id,
                is_fedramp: chatgpt_is_fedramp,
                email: chatgpt_email,
            },
        ));
    }
    metadata
}

#[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
struct ManagedChatGptCredential {
    access_token: Arc<str>,
    publication: meerkat_core::auth::lifecycle::TokenLifecyclePublication,
}

#[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
struct ManagedChatGptAuthorizer {
    env: ResolverEnvironment,
    binding: ValidatedBinding,
    metadata: AuthMetadata,
    runtime: oauth::OpenAiOAuthRuntime,
    cached: std::sync::Mutex<Option<ManagedChatGptCredential>>,
}

#[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
impl ManagedChatGptAuthorizer {
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
        let runtime = oauth::OpenAiOAuthRuntime::new(
            persistence,
            oauth::chatgpt_endpoints("http://127.0.0.1:0/callback"),
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
    ) -> Result<ManagedChatGptCredential, AuthError> {
        use meerkat_core::generated::auth_lease_durable_lifecycle_marker as marker;
        if marker::marker_relation_for_tokens_and_snapshot(tokens, snapshot, self.runtime.key())
            != marker::AuthLeaseDurableMarkerRelation::Matches
        {
            return Err(AuthError::StaleCredential);
        }
        let observed = chatgpt_metadata_from_tokens(tokens);
        let observed_provider = match &observed.provider_metadata {
            Some(meerkat_core::ProviderAuthMetadata::OpenAi(value)) => Some(value),
            _ => None,
        };
        let pinned_provider = match &self.metadata.provider_metadata {
            Some(meerkat_core::ProviderAuthMetadata::OpenAi(value)) => Some(value),
            _ => None,
        };
        let account_changed =
            observed.account_id.is_some() && observed.account_id != self.metadata.account_id;
        // A refresh response may preserve the old stored account while carrying
        // an explicit conflicting ID-token claim. Neither may silently repin.
        let claimed_account = tokens
            .id_token
            .as_deref()
            .and_then(|token| meerkat_auth_core::auth_oauth::jwt::decode_payload(token).ok())
            .and_then(|claims| oauth::ChatGptIdClaims::lift_from_claims(&claims.raw).account_id);
        let claim_changed =
            claimed_account.is_some() && claimed_account != self.metadata.account_id;
        let observed_fedramp = observed_provider.and_then(|value| value.is_fedramp);
        let pinned_fedramp = pinned_provider.and_then(|value| value.is_fedramp);
        if account_changed
            || claim_changed
            || (observed_fedramp.is_some() && observed_fedramp != pinned_fedramp)
        {
            return Err(AuthError::ResolveRequired(
                "managed ChatGPT credential no longer matches the selected account route".into(),
            ));
        }
        let publication =
            meerkat_core::tokens_lifecycle_publication(tokens).ok_or(AuthError::StaleCredential)?;
        let access_token = tokens
            .primary_secret
            .as_deref()
            .ok_or(AuthError::MissingSecret)?;
        Ok(ManagedChatGptCredential {
            access_token: Arc::from(access_token),
            publication,
        })
    }

    fn cache_matches(
        cached: &ManagedChatGptCredential,
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
                        Self::provider_error(openai_oauth_refresh_error(error, String::new()))
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
impl meerkat_core::HttpAuthorizer for ManagedChatGptAuthorizer {
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
        Ok(())
    }

    fn label(&self) -> &'static str {
        "managed-chatgpt-oauth"
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

/// The pre-`/codex` ChatGPT backend base URL that older persisted `.rkat`
/// backend profiles carried. Rejected, never healed: a stale config fails
/// loudly once with a typed [`ProviderClientError::InvalidBaseUrl`] and
/// re-seeds the canonical base URL at the next login.
const LEGACY_CHATGPT_BASE_URL: &str = "https://chatgpt.com/backend-api";

fn chatgpt_backend_base_url(configured: Option<&str>) -> Result<String, ProviderClientError> {
    let Some(raw) = configured.filter(|url| !url.trim().is_empty()) else {
        return Ok(OpenAiBackendKind::ChatGptBackend.default_base_url().into());
    };
    let trimmed = raw.trim_end_matches('/');
    if trimmed == LEGACY_CHATGPT_BASE_URL {
        return Err(ProviderClientError::InvalidBaseUrl(format!(
            "legacy ChatGPT backend base_url `{LEGACY_CHATGPT_BASE_URL}` is no longer \
             accepted; update the backend profile to `{}` (re-running `rkat login` \
             re-seeds it)",
            OpenAiBackendKind::ChatGptBackend.default_base_url()
        )));
    }
    Ok(trimmed.to_string())
}

fn azure_openai_base_url(configured: Option<&str>) -> Result<String, ProviderClientError> {
    let Some(raw) = configured.map(str::trim).filter(|url| !url.is_empty()) else {
        return Err(ProviderClientError::InvalidBaseUrl(
            "azure_openai requires backend base_url".to_string(),
        ));
    };
    let trimmed = raw.trim_end_matches('/');
    if let Some(base) = trimmed.strip_suffix("/openai/v1") {
        Ok(format!("{base}/openai"))
    } else if trimmed.ends_with("/openai") {
        Ok(trimmed.to_string())
    } else {
        Ok(format!("{trimmed}/openai"))
    }
}

fn backend_option_string(connection: &ResolvedConnection, key: &str) -> Option<String> {
    connection
        .backend_profile
        .options
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn azure_openai_wire_config(connection: &ResolvedConnection) -> AzureOpenAiWireConfig {
    AzureOpenAiWireConfig {
        image_generation_deployment: backend_option_string(
            connection,
            "image_generation_deployment",
        ),
        image_generation_api_version: backend_option_string(
            connection,
            "image_generation_api_version",
        )
        .unwrap_or_else(|| "preview".to_string()),
    }
}

/// Single textual owner of the fail-closed OpenAI realtime connection gate.
///
/// Decides which resolved OpenAI connections may carry a realtime transport
/// and returns the inline secret the realtime adapters authenticate with.
/// Both realtime builders (`build_realtime_text_client` and
/// `build_realtime_session_factory`) route through this helper so the
/// semantic fact "which resolved OpenAI connections support realtime" cannot
/// drift between them. Rejections are typed `MissingFeature` codes pinned by
/// tests:
///
/// - `openai-realtime-chatgpt-backend` / `openai-realtime-azure-openai` —
///   only the plain OpenAI API backend speaks the realtime protocol
/// - `openai-realtime-authorizer-auth` — authorizer-backed leases cannot
///   sign a WebSocket upgrade
/// - `openai-realtime-custom-base-url` — the realtime endpoint is not
///   base-url-configurable
/// - `NoCredentialMaterial` — the lease resolved without an inline secret
fn gate_realtime_connection(
    connection: &ResolvedConnection,
) -> Result<String, ProviderClientError> {
    // ProviderRuntimeRegistry dispatches on Provider enum; non-OpenAI
    // arms are unreachable at runtime.
    let backend_kind = match connection.backend {
        NormalizedBackendKind::OpenAi(k) => k,
        other => unreachable!(
            "OpenAiProviderRuntime received non-OpenAi backend: {other:?} \
             — registry dispatch invariant violated"
        ),
    };
    match backend_kind {
        OpenAiBackendKind::OpenAiApi => {}
        OpenAiBackendKind::ChatGptBackend => {
            return Err(ProviderClientError::MissingFeature(
                "openai-realtime-chatgpt-backend",
            ));
        }
        OpenAiBackendKind::AzureOpenAi => {
            return Err(ProviderClientError::MissingFeature(
                "openai-realtime-azure-openai",
            ));
        }
        OpenAiBackendKind::Copilot => {
            return Err(ProviderClientError::MissingFeature(
                "openai-realtime-copilot",
            ));
        }
    }
    if connection.resolved_authorizer().is_some() {
        return Err(ProviderClientError::MissingFeature(
            "openai-realtime-authorizer-auth",
        ));
    }
    if connection.backend_profile.base_url.is_some() {
        return Err(ProviderClientError::MissingFeature(
            "openai-realtime-custom-base-url",
        ));
    }
    connection
        .resolved_secret()
        .ok_or(ProviderClientError::NoCredentialMaterial)
}

fn chatgpt_backend_extra_headers(connection: &ResolvedConnection) -> Vec<(String, String)> {
    // Pull account identity + fedramp from the resolved lease's AuthMetadata
    // so the ChatGPT backend can emit the wire headers Codex's
    // bearer_auth_provider.rs:23-38 requires.
    let (account_id, is_fedramp) = match connection.auth_lease.metadata().provider_metadata {
        Some(meerkat_core::ProviderAuthMetadata::OpenAi(ref metadata)) => {
            (metadata.account_id.clone(), metadata.is_fedramp)
        }
        _ => (connection.auth_lease.metadata().account_id.clone(), None),
    };

    let mut headers = Vec::new();
    if let Some(account_id) = account_id {
        headers.push((
            meerkat_core::provider_matrix::openai_auth::CHATGPT_ACCOUNT_HEADER.to_string(),
            account_id,
        ));
    }
    if matches!(is_fedramp, Some(true)) {
        headers.push((
            meerkat_core::provider_matrix::openai_auth::FEDRAMP_HEADER.to_string(),
            "true".to_string(),
        ));
    }
    headers
}

#[derive(Default)]
pub struct OpenAiProviderRuntime {
    #[cfg(all(feature = "copilot", not(target_arch = "wasm32")))]
    copilot: Option<Arc<meerkat_copilot::CopilotRuntime>>,
}

#[allow(non_upper_case_globals)]
pub const OpenAiProviderRuntime: OpenAiProviderRuntime = OpenAiProviderRuntime {
    #[cfg(all(feature = "copilot", not(target_arch = "wasm32")))]
    copilot: None,
};

#[cfg(all(feature = "copilot", not(target_arch = "wasm32")))]
#[derive(Default)]
pub struct OpenAiCopilotChatCompletionsClientFactory;

#[cfg(all(feature = "copilot", not(target_arch = "wasm32")))]
impl meerkat_copilot::CopilotChatCompletionsClientFactory
    for OpenAiCopilotChatCompletionsClientFactory
{
    fn build(
        &self,
        spec: meerkat_copilot::CopilotChatCompletionsClientSpec,
    ) -> Result<Arc<dyn LlmClient>, ProviderClientError> {
        let supports_image_input = spec.supports_image_input();
        let (
            provider,
            model,
            api_base,
            authorizer,
            supports_temperature,
            supports_thinking,
            supports_reasoning,
            supports_image_tool_results,
        ) = spec.into_parts();
        Ok(Arc::new(
            crate::OpenAiCompatibleClient::new_with_options(
                crate::client_compatible::OpenAiCompatibleMode::ChatCompletions,
                model,
                api_base,
                None,
                crate::OpenAiCompatibleClientOptions {
                    supports_temperature,
                    supports_thinking,
                    supports_reasoning,
                    supports_image_tool_results,
                },
            )
            .with_image_input_support(supports_image_input)
            .with_authorizer(authorizer)
            .with_provider(provider),
        ))
    }
}

impl OpenAiProviderRuntime {
    #[cfg(all(feature = "copilot", not(target_arch = "wasm32")))]
    pub fn with_copilot(copilot: Arc<meerkat_copilot::CopilotRuntime>) -> Self {
        Self {
            copilot: Some(copilot),
        }
    }
}

fn build_openai_client(
    connection: ResolvedConnection,
    supports_image_input: bool,
) -> Result<Arc<dyn LlmClient>, ProviderClientError> {
    let backend_kind = match connection.backend {
        NormalizedBackendKind::OpenAi(kind) => kind,
        other => unreachable!(
            "OpenAiProviderRuntime received non-OpenAi backend: {other:?} \
             — registry dispatch invariant violated"
        ),
    };
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(authorizer) = connection.resolved_authorizer() {
        let base_url = match backend_kind {
            OpenAiBackendKind::ChatGptBackend => {
                chatgpt_backend_base_url(connection.backend_profile.base_url.as_deref())?
            }
            OpenAiBackendKind::AzureOpenAi => {
                azure_openai_base_url(connection.backend_profile.base_url.as_deref())?
            }
            OpenAiBackendKind::OpenAiApi => connection
                .backend_profile
                .base_url
                .clone()
                .unwrap_or_else(|| backend_kind.default_base_url().into()),
            OpenAiBackendKind::Copilot => {
                return Err(ProviderClientError::MissingFeature("copilot-text-target"));
            }
        };
        let mut client =
            crate::OpenAiClient::new_with_optional_api_key_and_base_url(None, base_url)
                .with_image_input_support(supports_image_input)
                .with_authorizer(authorizer);
        if matches!(backend_kind, OpenAiBackendKind::ChatGptBackend) {
            client = client
                .with_extra_headers(chatgpt_backend_extra_headers(&connection))
                .with_chatgpt_backend_wire();
        } else if matches!(backend_kind, OpenAiBackendKind::AzureOpenAi) {
            client = client.with_azure_openai_wire(azure_openai_wire_config(&connection));
        }
        return Ok(Arc::new(client));
    }
    #[cfg(target_arch = "wasm32")]
    let secret = connection
        .resolved_secret()
        .ok_or(ProviderClientError::MissingFeature(
            "openai-authorizer-backed auth not available on wasm32",
        ))?;
    #[cfg(not(target_arch = "wasm32"))]
    let secret = connection
        .resolved_secret()
        .ok_or(ProviderClientError::NoCredentialMaterial)?;
    let client = match backend_kind {
        OpenAiBackendKind::OpenAiApi => match &connection.backend_profile.base_url {
            Some(url) => crate::OpenAiClient::new_with_base_url(secret, url.clone()),
            None => crate::OpenAiClient::new(secret),
        }
        .with_image_input_support(supports_image_input),
        OpenAiBackendKind::ChatGptBackend => {
            let base_url =
                chatgpt_backend_base_url(connection.backend_profile.base_url.as_deref())?;
            crate::OpenAiClient::new_with_base_url(secret, base_url)
                .with_image_input_support(supports_image_input)
                .with_extra_headers(chatgpt_backend_extra_headers(&connection))
                .with_chatgpt_backend_wire()
        }
        OpenAiBackendKind::AzureOpenAi => {
            let base_url = azure_openai_base_url(connection.backend_profile.base_url.as_deref())?;
            crate::OpenAiClient::new_with_base_url(secret, base_url)
                .with_image_input_support(supports_image_input)
                .with_azure_openai_wire(azure_openai_wire_config(&connection))
        }
        OpenAiBackendKind::Copilot => {
            return Err(ProviderClientError::MissingFeature("copilot-text-target"));
        }
    };
    Ok(Arc::new(client))
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]

impl ProviderRuntime for OpenAiProviderRuntime {
    fn provider_id(&self) -> Provider {
        Provider::OpenAI
    }

    /// API-key and static-bearer credentials are observed read-only through
    /// the same source table [`resolve_simple_secret`] resolves. OAuth,
    /// Copilot, and external-authorizer credentials are resolved at open.
    async fn observe_credential_readiness(
        &self,
        binding: &ValidatedBinding,
        env: &ResolverEnvironment,
    ) -> Result<CredentialReadiness, ProviderAuthError> {
        if binding.provider() != Provider::OpenAI {
            return Err(ProviderAuthError::Binding(
                ProviderBindingError::ProviderMismatch,
            ));
        }
        match binding.auth() {
            NormalizedAuthMethod::OpenAi(
                OpenAiAuthMethod::ApiKey
                | OpenAiAuthMethod::AzureApiKey
                | OpenAiAuthMethod::StaticBearer,
            ) => Ok(
                observe_simple_secret_readiness(&binding.auth_profile().source, env, binding).await,
            ),
            NormalizedAuthMethod::OpenAi(_) => Ok(CredentialReadiness::MaterializedAtOpen),
            _ => Err(ProviderAuthError::Binding(
                ProviderBindingError::ProviderMismatch,
            )),
        }
    }

    async fn resolve_binding(
        &self,
        binding: &ValidatedBinding,
        env: &ResolverEnvironment,
    ) -> Result<ResolvedConnection, ProviderAuthError> {
        if binding.provider() != Provider::OpenAI {
            return Err(ProviderAuthError::Binding(
                ProviderBindingError::ProviderMismatch,
            ));
        }
        let auth_method = match binding.auth() {
            NormalizedAuthMethod::OpenAi(m) => m,
            _ => {
                return Err(ProviderAuthError::Binding(
                    ProviderBindingError::ProviderMismatch,
                ));
            }
        };
        let backend_kind = match binding.backend() {
            NormalizedBackendKind::OpenAi(k) => k,
            _ => {
                return Err(ProviderAuthError::Binding(
                    ProviderBindingError::ProviderMismatch,
                ));
            }
        };

        let source_label = format!("openai:{}", binding.auth_profile().id);
        let lease: Arc<dyn AuthLease> = match auth_method {
            OpenAiAuthMethod::ApiKey
            | OpenAiAuthMethod::AzureApiKey
            | OpenAiAuthMethod::StaticBearer => {
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
            OpenAiAuthMethod::ExternalAuthorizer => {
                resolve_external_authorizer(&binding.auth_profile().source, env, binding).await?
            }
            OpenAiAuthMethod::GitHubCopilotOauth => {
                #[cfg(all(feature = "copilot", not(target_arch = "wasm32")))]
                {
                    let runtime = self.copilot.as_ref().ok_or_else(|| {
                        ProviderAuthError::SourceResolutionFailed(
                            "OpenAI Copilot backend is not composed with CopilotRuntime"
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
                        "OpenAI Copilot backend is not compiled".to_string(),
                    ));
                }
            }
            OpenAiAuthMethod::ManagedChatGptOauth | OpenAiAuthMethod::ExternalChatGptTokens => {
                #[cfg(all(not(target_arch = "wasm32"), feature = "oauth"))]
                {
                    let expected_mode = match auth_method {
                        OpenAiAuthMethod::ManagedChatGptOauth => PersistedAuthMode::ChatgptOauth,
                        OpenAiAuthMethod::ExternalChatGptTokens => {
                            PersistedAuthMode::ExternalTokens
                        }
                        _ => unreachable!("OAuth branch only handles OAuth auth methods"),
                    };
                    validate_oauth_target_for_auth_mode(
                        binding.auth_profile(),
                        Provider::OpenAI,
                        expected_mode,
                    )
                    .map_err(|e| ProviderAuthError::SourceResolutionFailed(e.to_string()))?;
                    let mut managed =
                        load_managed_store_tokens_with_lifecycle(env, binding).await?;
                    let lifecycle = managed.lifecycle;
                    let persisted = managed.tokens.clone();

                    let effective_tokens = match auth_method {
                        OpenAiAuthMethod::ExternalChatGptTokens => {
                            if lifecycle == ManagedStoreLifecycle::RefreshRequired {
                                return Err(ProviderAuthError::Auth(AuthError::RefreshRequired));
                            }
                            persisted
                        }
                        OpenAiAuthMethod::ManagedChatGptOauth => {
                            // Cached-vs-refresh disposition owned by the
                            // per-binding AuthMachine: feed the pure observations
                            // and mirror the verdict (see anthropic runtime for
                            // the full contract).
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
                                        oauth::chatgpt_endpoints("http://127.0.0.1:0/callback");
                                    let runtime = oauth::OpenAiOAuthRuntime::new(
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
                                            openai_oauth_refresh_error(error, String::new())
                                        })?
                                }
                            }
                        }
                        _ => unreachable!("arm guarded by outer match"),
                    };

                    let access = effective_tokens
                        .primary_secret
                        .clone()
                        .ok_or(ProviderAuthError::Auth(AuthError::MissingSecret))?;
                    let metadata = chatgpt_metadata_from_tokens(&effective_tokens);
                    let metadata = finalize_auth_metadata(binding, metadata)?;
                    if matches!(auth_method, OpenAiAuthMethod::ManagedChatGptOauth) {
                        let authorizer = ManagedChatGptAuthorizer::new(
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
                    } else {
                        Arc::new(StaticLease::inline_secret(
                            access,
                            metadata,
                            effective_tokens.expires_at,
                            source_label.clone(),
                        ))
                    }
                }
                #[cfg(not(all(not(target_arch = "wasm32"), feature = "oauth")))]
                {
                    return Err(interactive_login_error(binding));
                }
            }
        };

        Ok(ResolvedConnection {
            provider: Provider::OpenAI,
            backend: NormalizedBackendKind::OpenAi(backend_kind),
            backend_profile: binding.backend_profile().clone(),
            credential_identity: binding.credential_identity().clone(),
            auth_lease: lease,
        })
    }

    fn build_client(
        &self,
        connection: ResolvedConnection,
    ) -> Result<Arc<dyn LlmClient>, ProviderClientError> {
        // Authorizer-backed connections use HttpAuthorizer rather
        // than Authorization: Bearer <api_key>. Plan §6.11: read the
        // authorizer directly from the auth lease.
        build_openai_client(connection, true)
    }

    fn build_text_client(
        &self,
        target: ResolvedTextTarget,
    ) -> Result<Arc<dyn LlmClient>, ProviderClientError> {
        if !matches!(
            target.connection().backend,
            NormalizedBackendKind::OpenAi(OpenAiBackendKind::Copilot)
        ) {
            let (_, profile, connection) = target.into_parts();
            return build_openai_client(connection, profile.profile().image_input);
        }
        #[cfg(all(feature = "copilot", not(target_arch = "wasm32")))]
        {
            let runtime = self.copilot.as_ref().ok_or_else(|| {
                ProviderClientError::ClientInit(
                    "OpenAI Copilot backend is not composed with CopilotRuntime".to_string(),
                )
            })?;
            let (identity, profile, connection) = target.into_parts();
            let model = identity.model.clone();
            let supports_temperature = profile.profile().supports_temperature;
            let supports_image_input = profile.profile().image_input;
            let supports_image_tool_results = profile.profile().image_tool_results;
            let factory: meerkat_copilot::CopilotRouteClientFactory =
                Arc::new(move |route, connection| {
                    let endpoint = match route.access {
                        meerkat_copilot::CopilotModelAccess::Available { endpoint } => endpoint,
                        meerkat_copilot::CopilotModelAccess::Unavailable => {
                            return Err(ProviderClientError::ClientInit(
                                route.unavailable_message(
                                    Provider::OpenAI,
                                    &model,
                                    meerkat_copilot::CopilotEndpoint::Responses,
                                ),
                            ));
                        }
                        meerkat_copilot::CopilotModelAccess::Unknown => {
                            meerkat_copilot::CopilotEndpoint::optimistic_for_provider(
                                Provider::OpenAI,
                            )
                            .ok_or_else(|| {
                                ProviderClientError::ClientInit(
                                    "Copilot has no optimistic OpenAI route".to_string(),
                                )
                            })?
                        }
                    };
                    let authorizer = connection
                        .resolved_authorizer()
                        .ok_or(ProviderClientError::NoCredentialMaterial)?;
                    let authorizer = route.bind_authorizer(authorizer);
                    match endpoint {
                        meerkat_copilot::CopilotEndpoint::Responses => Ok(Arc::new(
                            crate::OpenAiClient::new_with_optional_api_key_and_base_url(
                                None,
                                route.api_base.clone(),
                            )
                            .with_image_input_support(supports_image_input)
                            .with_authorizer(authorizer)
                            .with_responses_path("responses"),
                        )
                            as Arc<dyn LlmClient>),
                        meerkat_copilot::CopilotEndpoint::ChatCompletions => Ok(Arc::new(
                            crate::OpenAiCompatibleClient::new_with_options(
                                crate::client_compatible::OpenAiCompatibleMode::ChatCompletions,
                                model.clone(),
                                route.api_base.clone(),
                                None,
                                crate::OpenAiCompatibleClientOptions {
                                    supports_temperature,
                                    supports_thinking: true,
                                    supports_reasoning: true,
                                    supports_image_tool_results,
                                },
                            )
                            .with_image_input_support(supports_image_input)
                            .with_authorizer(authorizer)
                            .with_provider(Provider::OpenAI),
                        )
                            as Arc<dyn LlmClient>),
                        meerkat_copilot::CopilotEndpoint::Messages
                        | meerkat_copilot::CopilotEndpoint::Unknown => {
                            Err(ProviderClientError::ClientInit(
                                "Copilot advertised an incompatible endpoint for an OpenAI model"
                                    .to_string(),
                            ))
                        }
                    }
                });
            meerkat_copilot::routed_client(
                Arc::clone(runtime),
                connection,
                Provider::OpenAI,
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

    fn build_realtime_text_client(
        &self,
        connection: ResolvedConnection,
    ) -> Result<Arc<dyn LlmClient>, ProviderClientError> {
        let secret = gate_realtime_connection(&connection)?;
        #[cfg(all(not(target_arch = "wasm32"), feature = "realtime"))]
        {
            Ok(Arc::new(crate::OpenAiRealtimeTextAdapter::new(secret)))
        }
        #[cfg(not(all(not(target_arch = "wasm32"), feature = "realtime")))]
        {
            let _ = secret;
            Err(ProviderClientError::MissingFeature("openai-realtime"))
        }
    }

    fn build_realtime_session_factory(
        &self,
        target: meerkat_llm_core::provider_runtime::ResolvedRealtimeTarget,
    ) -> Result<
        Arc<dyn meerkat_llm_core::realtime_session::RealtimeSessionFactory>,
        ProviderClientError,
    > {
        if !target.profile().profile().realtime {
            return Err(ProviderClientError::ClientInit(
                "resolved model profile does not admit realtime transport".to_string(),
            ));
        }
        if target.profile().profile().release_stage != meerkat_core::ModelReleaseStage::Stable {
            return Err(ProviderClientError::ClientInit(
                "experimental realtime models require their dedicated admitted factory".to_string(),
            ));
        }
        if target.profile().profile().model_family == GPT_LIVE_MODEL_FAMILY {
            // The public Live API is a different protocol (continuous audio,
            // client delegation over a WebRTC sideband); it is reached through
            // the `live/open` execution-identity seam, never this factory.
            return Err(ProviderClientError::ClientInit(
                "gpt-live models use the public Live broker, not the Realtime WebSocket factory"
                    .to_string(),
            ));
        }
        let (_, _, connection) = target.into_parts();
        let secret = gate_realtime_connection(&connection)?;
        #[cfg(all(not(target_arch = "wasm32"), feature = "realtime"))]
        {
            let live = Arc::new(crate::live::OpenAiLiveClient::new(secret))
                as Arc<dyn crate::live::OpenAiLiveSessionFactory>;
            Ok(Arc::new(crate::live::OpenAiRealtimeSessionFactory::new(
                live,
            )))
        }
        #[cfg(not(all(not(target_arch = "wasm32"), feature = "realtime")))]
        {
            let _ = secret;
            Err(ProviderClientError::MissingFeature("openai-realtime"))
        }
    }

    fn build_image_generation_executor(
        &self,
        connection: ResolvedConnection,
    ) -> Result<Option<Arc<dyn ImageGenerationExecutor>>, ProviderClientError> {
        let backend_kind = match connection.backend {
            NormalizedBackendKind::OpenAi(k) => k,
            other => unreachable!(
                "OpenAiProviderRuntime received non-OpenAi backend: {other:?} \
                 — registry dispatch invariant violated"
            ),
        };
        if matches!(backend_kind, OpenAiBackendKind::AzureOpenAi)
            && azure_openai_wire_config(&connection)
                .image_generation_deployment
                .is_none()
        {
            return Ok(None);
        }
        if matches!(backend_kind, OpenAiBackendKind::Copilot) {
            return Ok(None);
        }
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(authorizer) = connection.resolved_authorizer() {
            let base_url = match backend_kind {
                OpenAiBackendKind::ChatGptBackend => {
                    chatgpt_backend_base_url(connection.backend_profile.base_url.as_deref())?
                }
                OpenAiBackendKind::AzureOpenAi => {
                    azure_openai_base_url(connection.backend_profile.base_url.as_deref())?
                }
                OpenAiBackendKind::OpenAiApi => connection
                    .backend_profile
                    .base_url
                    .clone()
                    .unwrap_or_else(|| backend_kind.default_base_url().into()),
                OpenAiBackendKind::Copilot => return Ok(None),
            };
            let mut client =
                crate::OpenAiClient::new_with_optional_api_key_and_base_url(None, base_url)
                    .with_authorizer(authorizer);
            if matches!(backend_kind, OpenAiBackendKind::ChatGptBackend) {
                client = client
                    .with_extra_headers(chatgpt_backend_extra_headers(&connection))
                    .with_chatgpt_backend_wire();
            } else if matches!(backend_kind, OpenAiBackendKind::AzureOpenAi) {
                client = client.with_azure_openai_wire(azure_openai_wire_config(&connection));
            }
            return Ok(Some(Arc::new(client)));
        }
        #[cfg(target_arch = "wasm32")]
        let secret = connection
            .resolved_secret()
            .ok_or(ProviderClientError::MissingFeature(
                "openai-authorizer-backed auth not available on wasm32",
            ))?;
        #[cfg(not(target_arch = "wasm32"))]
        let secret = connection
            .resolved_secret()
            .ok_or(ProviderClientError::NoCredentialMaterial)?;
        let client = match backend_kind {
            OpenAiBackendKind::OpenAiApi => match &connection.backend_profile.base_url {
                Some(url) => crate::OpenAiClient::new_with_base_url(secret, url.clone()),
                None => crate::OpenAiClient::new(secret),
            },
            OpenAiBackendKind::ChatGptBackend => {
                let base_url =
                    chatgpt_backend_base_url(connection.backend_profile.base_url.as_deref())?;
                crate::OpenAiClient::new_with_base_url(secret, base_url)
                    .with_extra_headers(chatgpt_backend_extra_headers(&connection))
                    .with_chatgpt_backend_wire()
            }
            OpenAiBackendKind::AzureOpenAi => {
                let base_url =
                    azure_openai_base_url(connection.backend_profile.base_url.as_deref())?;
                crate::OpenAiClient::new_with_base_url(secret, base_url)
                    .with_azure_openai_wire(azure_openai_wire_config(&connection))
            }
            OpenAiBackendKind::Copilot => return Ok(None),
        };
        Ok(Some(Arc::new(client)))
    }

    fn image_generation_profile(
        &self,
    ) -> Option<Arc<dyn meerkat_core::ImageGenerationProviderProfile>> {
        Some(Arc::new(crate::OpenAiImageGenerationProfile))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use axum::{
        Json, Router, extract::State, http::HeaderMap, response::IntoResponse, routing::post,
    };
    use meerkat_core::{
        AuthMetadata, AuthProfile, BackendProfile, BindingPolicy, ContentBlock, ImageData,
        ImageProviderTerminalObservation, Message, OpenAiAuthMetadata, ProviderAuthMetadata,
        UserMessage,
    };
    use meerkat_llm_core::{
        ProviderImageGenerationRequest,
        provider_runtime::{ProviderRuntimeCatalog, binding::DynamicLease},
    };
    use std::sync::Mutex;
    use tokio::net::TcpListener;

    #[test]
    fn typed_catalog_contains_expected_combinations() {
        assert!(ProviderRuntimeCatalog::supports(
            NormalizedBackendKind::OpenAi(OpenAiBackendKind::OpenAiApi),
            NormalizedAuthMethod::OpenAi(OpenAiAuthMethod::ApiKey),
        ));
        assert!(ProviderRuntimeCatalog::supports(
            NormalizedBackendKind::OpenAi(OpenAiBackendKind::ChatGptBackend),
            NormalizedAuthMethod::OpenAi(OpenAiAuthMethod::ManagedChatGptOauth),
        ));
        assert!(ProviderRuntimeCatalog::supports(
            NormalizedBackendKind::OpenAi(OpenAiBackendKind::AzureOpenAi),
            NormalizedAuthMethod::OpenAi(OpenAiAuthMethod::AzureApiKey),
        ));
        assert!(!ProviderRuntimeCatalog::supports(
            NormalizedBackendKind::OpenAi(OpenAiBackendKind::AzureOpenAi),
            NormalizedAuthMethod::OpenAi(OpenAiAuthMethod::ApiKey),
        ));
    }

    #[test]
    fn provider_id_is_openai() {
        assert_eq!(OpenAiProviderRuntime.provider_id(), Provider::OpenAI);
    }

    #[test]
    fn chatgpt_backend_base_url_resolves_default_and_explicit_values() {
        // Missing / blank → the canonical default.
        assert_eq!(
            chatgpt_backend_base_url(None).unwrap(),
            OpenAiBackendKind::ChatGptBackend.default_base_url()
        );
        assert_eq!(
            chatgpt_backend_base_url(Some("   ")).unwrap(),
            OpenAiBackendKind::ChatGptBackend.default_base_url()
        );
        // Explicit non-legacy values pass through (trailing slash trimmed).
        assert_eq!(
            chatgpt_backend_base_url(Some("https://example.com/proxy/")).unwrap(),
            "https://example.com/proxy"
        );
    }

    #[test]
    fn chatgpt_backend_base_url_rejects_legacy_persisted_value() {
        // Pre-`/codex` persisted configs are REJECTED with a typed error —
        // never silently healed. Stale `.rkat` profiles fail loudly once and
        // re-seed the canonical base URL at the next login.
        for legacy in [
            "https://chatgpt.com/backend-api",
            "https://chatgpt.com/backend-api/",
        ] {
            match chatgpt_backend_base_url(Some(legacy)) {
                Err(ProviderClientError::InvalidBaseUrl(message)) => {
                    assert!(
                        message.contains("https://chatgpt.com/backend-api"),
                        "rejection must name the legacy URL: {message}"
                    );
                    assert!(
                        message.contains(OpenAiBackendKind::ChatGptBackend.default_base_url()),
                        "rejection must name the canonical replacement: {message}"
                    );
                }
                other => panic!("legacy base URL must be rejected, got {other:?}"),
            }
        }
    }

    fn backend(kind: &str) -> BackendProfile {
        BackendProfile {
            id: "b".into(),
            provider: Provider::OpenAI,
            backend_kind: kind.into(),
            base_url: None,
            options: serde_json::Value::Null,
            server: None,
        }
    }

    fn auth(method: &str) -> AuthProfile {
        AuthProfile {
            id: "a".into(),
            provider: Provider::OpenAI,
            auth_method: method.into(),
            source: meerkat_core::CredentialSourceSpec::InlineSecret {
                secret: "sk-x".into(),
            },
            constraints: Default::default(),
            metadata_defaults: Default::default(),
        }
    }

    fn auth_binding() -> meerkat_core::AuthBindingRef {
        meerkat_core::AuthBindingRef {
            realm: meerkat_core::connection::RealmId::parse("dev").unwrap(),
            binding: meerkat_core::connection::BindingId::parse("default").unwrap(),
            profile: None,
            origin: meerkat_core::connection::BindingOrigin::Configured,
        }
    }

    fn resolved_openai_connection() -> ResolvedConnection {
        ResolvedConnection {
            provider: Provider::OpenAI,
            backend: NormalizedBackendKind::OpenAi(OpenAiBackendKind::OpenAiApi),
            backend_profile: Arc::new(backend("openai_api")),
            credential_identity: meerkat_core::AuthCredentialIdentity::from_auth_binding(
                &auth_binding(),
            ),
            auth_lease: Arc::new(StaticLease::inline_secret(
                "sk-test".into(),
                AuthMetadata::default(),
                None,
                "openai:test",
            )),
        }
    }

    #[test]
    fn profile_aware_openai_builder_disables_image_replay() {
        let client = build_openai_client(resolved_openai_connection(), false)
            .expect("profile-aware OpenAI client");
        let projected = client
            .project_replay_messages(&[Message::User(UserMessage::with_blocks(vec![
                ContentBlock::Image {
                    media_type: "image/png".to_string(),
                    data: ImageData::Inline {
                        data: "IMAGE_BYTES".to_string(),
                    },
                },
            ]))])
            .expect("non-vision replay projection");

        assert!(matches!(
            &projected[0],
            Message::User(user) if matches!(user.content.as_slice(), [ContentBlock::Text { .. }])
        ));
    }

    fn resolved_realtime_target(
        connection: ResolvedConnection,
    ) -> meerkat_llm_core::provider_runtime::ResolvedRealtimeTarget {
        let registry = meerkat_core::ModelRegistry::from_config(
            &meerkat_core::Config::default(),
            meerkat_models::canonical(),
        )
        .expect("canonical model registry");
        let model = registry
            .entries_for_provider(Provider::OpenAI)
            .find(|entry| {
                entry.release_stage == meerkat_core::ModelReleaseStage::Stable
                    && registry
                        .profile_for_provider(Provider::OpenAI, &entry.id)
                        .is_some_and(|profile| {
                            // The Realtime WebSocket factory never serves the
                            // gpt-live family; that is the public Live broker.
                            profile.realtime && profile.model_family != GPT_LIVE_MODEL_FAMILY
                        })
            })
            .map(|entry| entry.id.clone())
            .expect("stable OpenAI Realtime model");
        let identity = meerkat_core::SessionLlmIdentity {
            model: model.clone(),
            provider: Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        };
        let witness = registry
            .profile_witness_for_provider(Provider::OpenAI, &model)
            .expect("stable realtime witness");
        meerkat_llm_core::provider_runtime::ResolvedRealtimeTarget::new(
            identity, witness, connection,
        )
        .expect("matching target")
    }

    fn resolved_chatgpt_connection(
        metadata: AuthMetadata,
        base_url: Option<String>,
    ) -> ResolvedConnection {
        let mut backend = backend("chatgpt_backend");
        backend.base_url = base_url;
        ResolvedConnection {
            provider: Provider::OpenAI,
            backend: NormalizedBackendKind::OpenAi(OpenAiBackendKind::ChatGptBackend),
            backend_profile: Arc::new(backend),
            credential_identity: meerkat_core::AuthCredentialIdentity::from_auth_binding(
                &auth_binding(),
            ),
            auth_lease: Arc::new(StaticLease::inline_secret(
                "oauth-access-token".into(),
                metadata,
                None,
                "openai:test",
            )),
        }
    }

    fn resolved_azure_connection(options: serde_json::Value) -> ResolvedConnection {
        let mut backend = backend("azure_openai");
        backend.base_url = Some("https://example.openai.azure.com".to_string());
        backend.options = options;
        ResolvedConnection {
            provider: Provider::OpenAI,
            backend: NormalizedBackendKind::OpenAi(OpenAiBackendKind::AzureOpenAi),
            backend_profile: Arc::new(backend),
            credential_identity: meerkat_core::AuthCredentialIdentity::from_auth_binding(
                &auth_binding(),
            ),
            auth_lease: Arc::new(StaticLease::inline_secret(
                "azure-api-key".into(),
                AuthMetadata::default(),
                None,
                "openai:azure",
            )),
        }
    }

    #[derive(Debug)]
    struct TestAuthorizer;

    #[async_trait::async_trait]
    impl meerkat_core::HttpAuthorizer for TestAuthorizer {
        async fn authorize(
            &self,
            _req: &mut meerkat_core::HttpAuthorizationRequest<'_>,
        ) -> Result<(), meerkat_core::AuthError> {
            Ok(())
        }

        fn label(&self) -> &'static str {
            "test-authorizer"
        }
    }

    fn expect_realtime_text_error(
        result: Result<Arc<dyn LlmClient>, ProviderClientError>,
    ) -> ProviderClientError {
        match result {
            Ok(_) => panic!("expected realtime text client construction to fail"),
            Err(err) => err,
        }
    }

    fn expect_realtime_session_factory_error(
        result: Result<
            Arc<dyn meerkat_llm_core::realtime_session::RealtimeSessionFactory>,
            ProviderClientError,
        >,
    ) -> ProviderClientError {
        match result {
            Ok(_) => panic!("expected realtime session factory construction to fail"),
            Err(err) => err,
        }
    }

    #[derive(Clone)]
    struct ImageHeaderState {
        seen: Arc<Mutex<Vec<HeaderMap>>>,
    }

    async fn image_header_stub(
        State(state): State<ImageHeaderState>,
        headers: HeaderMap,
        Json(_body): Json<serde_json::Value>,
    ) -> impl IntoResponse {
        state.seen.lock().expect("seen headers").push(headers);
        let payload = [
            r#"data: {"type":"response.output_item.done","item":{"id":"ig_test","type":"image_generation_call","status":"completed","result":"data:image/png;base64,aGVsbG8="}}"#,
            r#"data: {"type":"response.completed","response":{"id":"resp_test","output":[]}}"#,
            "data: [DONE]",
            "",
        ]
        .join("\n");
        ([("content-type", "text/event-stream")], payload)
    }

    async fn spawn_image_header_stub() -> (
        String,
        Arc<Mutex<Vec<HeaderMap>>>,
        tokio::task::JoinHandle<()>,
    ) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let app = Router::new()
            .route("/responses", post(image_header_stub))
            .with_state(ImageHeaderState {
                seen: Arc::clone(&seen),
            });
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test server");
        let addr = listener.local_addr().expect("local addr");
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve test server");
        });
        (format!("http://{addr}"), seen, handle)
    }

    fn hosted_image_request() -> ProviderImageGenerationRequest {
        serde_json::from_value(serde_json::json!({
            "operation_id": "00000000-0000-0000-0000-000000000101",
            "model": "gpt-5.4",
            "generate_request": {
                "intent": {
                    "intent": "generate",
                    "prompt": {"content": "draw a small red square"},
                    "prompt_source": {
                        "source": "user_provided",
                        "message_id": "00000000-0000-0000-0000-000000000102"
                    },
                    "reference_images": []
                },
                "target": {"target": "auto"},
                "size": {"size": "square1024"},
                "quality": "low",
                "format": "png",
                "count": 1
            },
            "execution_plan": {
                "provider": "openai",
                "backend": "hosted_tool",
                "max_count": 1,
                "capabilities": {
                    "hosted_image_generation_tool": true,
                    "native_image_output": false,
                    "custom_tools": true,
                    "image_search_grounding": false,
                    "image_continuity_tokens": "unsupported"
                },
                "requires_scoped_override": false,
                "provider_plan": {
                    "tool_name": "image_generation",
                    "model": "gpt-image-2",
                    "output": {
                        "size": "square1024",
                        "quality": "low",
                        "output_format": "png"
                    }
                }
            },
            "projected_messages": []
        }))
        .expect("hosted image request")
    }

    #[test]
    fn typed_catalog_validate_accepts_allowed_combination() {
        let vb = ProviderRuntimeCatalog::validate_binding(
            &auth_binding(),
            &backend("openai_api"),
            &auth("api_key"),
            &BindingPolicy::default(),
        )
        .expect("allowed combination");
        assert_eq!(vb.provider(), Provider::OpenAI);
    }

    #[test]
    fn typed_catalog_validate_accepts_azure_openai_combination() {
        let vb = ProviderRuntimeCatalog::validate_binding(
            &auth_binding(),
            &backend("azure_openai"),
            &auth("azure_api_key"),
            &BindingPolicy::default(),
        )
        .expect("allowed Azure OpenAI combination");
        assert_eq!(vb.provider(), Provider::OpenAI);
        assert_eq!(
            vb.backend(),
            NormalizedBackendKind::OpenAi(OpenAiBackendKind::AzureOpenAi)
        );
    }

    #[test]
    fn typed_catalog_validate_rejects_unknown_backend_kind() {
        let err = ProviderRuntimeCatalog::validate_binding(
            &auth_binding(),
            &backend("bogus_backend"),
            &auth("api_key"),
            &BindingPolicy::default(),
        )
        .unwrap_err();
        assert!(matches!(err, ProviderBindingError::UnknownBackendKind(_)));
    }

    #[test]
    fn typed_catalog_validate_rejects_unsupported_combo() {
        // openai_api + managed_chatgpt_oauth is not a typed catalog edge.
        let err = ProviderRuntimeCatalog::validate_binding(
            &auth_binding(),
            &backend("openai_api"),
            &auth("managed_chatgpt_oauth"),
            &BindingPolicy::default(),
        )
        .unwrap_err();
        assert!(matches!(
            err,
            ProviderBindingError::UnsupportedCombination { .. }
        ));
    }

    #[test]
    fn azure_openai_base_url_normalizes_resource_endpoint() {
        assert_eq!(
            azure_openai_base_url(Some("https://example.openai.azure.com/")).unwrap(),
            "https://example.openai.azure.com/openai"
        );
        assert_eq!(
            azure_openai_base_url(Some("https://example.openai.azure.com/openai")).unwrap(),
            "https://example.openai.azure.com/openai"
        );
        assert_eq!(
            azure_openai_base_url(Some("https://example.openai.azure.com/openai/v1/")).unwrap(),
            "https://example.openai.azure.com/openai"
        );
    }

    #[test]
    fn azure_openai_base_url_rejects_missing_value() {
        let err = azure_openai_base_url(None).unwrap_err();
        assert!(matches!(err, ProviderClientError::InvalidBaseUrl(_)));
        let err = azure_openai_base_url(Some("   ")).unwrap_err();
        assert!(matches!(err, ProviderClientError::InvalidBaseUrl(_)));
    }

    #[test]
    fn typed_catalog_validate_rejects_provider_mismatch() {
        let mut wrong = backend("openai_api");
        wrong.provider = Provider::Anthropic;
        let err = ProviderRuntimeCatalog::validate_binding(
            &auth_binding(),
            &wrong,
            &auth("api_key"),
            &BindingPolicy::default(),
        )
        .unwrap_err();
        assert!(matches!(err, ProviderBindingError::ProviderMismatch));
    }

    #[test]
    fn typed_catalog_validate_propagates_binding_policy() {
        // Dogma §16: policy declared on the binding must flow through
        // catalog validation, not default-injected at the provider seam.
        let policy = BindingPolicy {
            allow_auth_override: true,
            require_metadata_account: true,
            require_metadata_workspace: false,
        };
        let vb = ProviderRuntimeCatalog::validate_binding(
            &auth_binding(),
            &backend("openai_api"),
            &auth("api_key"),
            &policy,
        )
        .expect("allowed combination");
        assert_eq!(vb.policy(), &policy);
    }

    #[test]
    fn realtime_text_client_is_constructed_by_openai_runtime_gate() {
        let result = OpenAiProviderRuntime.build_realtime_text_client(resolved_openai_connection());

        #[cfg(all(not(target_arch = "wasm32"), feature = "realtime"))]
        assert!(
            result.is_ok(),
            "native realtime builds should construct the adapter inside the provider runtime"
        );

        #[cfg(not(all(not(target_arch = "wasm32"), feature = "realtime")))]
        assert!(matches!(
            result,
            Err(ProviderClientError::MissingFeature("openai-realtime"))
        ));
    }

    #[test]
    fn realtime_text_client_rejects_provider_specific_unsupported_backends() {
        let chatgpt = expect_realtime_text_error(OpenAiProviderRuntime.build_realtime_text_client(
            resolved_chatgpt_connection(AuthMetadata::default(), None),
        ));
        assert!(matches!(
            chatgpt,
            ProviderClientError::MissingFeature("openai-realtime-chatgpt-backend")
        ));

        let azure = expect_realtime_text_error(
            OpenAiProviderRuntime
                .build_realtime_text_client(resolved_azure_connection(serde_json::Value::Null)),
        );
        assert!(matches!(
            azure,
            ProviderClientError::MissingFeature("openai-realtime-azure-openai")
        ));
    }

    #[test]
    fn realtime_text_client_rejects_runtime_owned_unsupported_auth_and_url_shapes() {
        let mut custom_url = resolved_openai_connection();
        custom_url.backend_profile = Arc::new(BackendProfile {
            base_url: Some("https://example.test/openai".to_string()),
            ..backend("openai_api")
        });
        let err = expect_realtime_text_error(
            OpenAiProviderRuntime.build_realtime_text_client(custom_url),
        );
        assert!(matches!(
            err,
            ProviderClientError::MissingFeature("openai-realtime-custom-base-url")
        ));

        let mut dynamic_auth = resolved_openai_connection();
        dynamic_auth.auth_lease = Arc::new(DynamicLease::new(
            Arc::new(TestAuthorizer),
            AuthMetadata::default(),
            None,
            "openai:dynamic",
        ));
        let err = expect_realtime_text_error(
            OpenAiProviderRuntime.build_realtime_text_client(dynamic_auth),
        );
        assert!(matches!(
            err,
            ProviderClientError::MissingFeature("openai-realtime-authorizer-auth")
        ));
    }

    #[test]
    fn realtime_session_factory_rejects_gpt_live_family_rows() {
        let registry = meerkat_core::ModelRegistry::from_config(
            &meerkat_core::Config::default(),
            meerkat_models::canonical(),
        )
        .expect("canonical model registry");
        let live_row = registry
            .entries_for_provider(Provider::OpenAI)
            .find(|entry| {
                registry
                    .profile_for_provider(Provider::OpenAI, &entry.id)
                    .is_some_and(|profile| {
                        profile.model_family == GPT_LIVE_MODEL_FAMILY
                            && profile.release_stage == meerkat_core::ModelReleaseStage::Stable
                    })
            })
            .map(|entry| entry.id.clone())
            .expect("released gpt-live catalog row");
        let identity = meerkat_core::SessionLlmIdentity {
            model: live_row,
            provider: Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        };
        let witness = registry
            .profile_witness_for_provider(Provider::OpenAI, &identity.model)
            .expect("gpt-live witness");
        let connection = resolved_openai_connection();
        let target = meerkat_llm_core::provider_runtime::ResolvedRealtimeTarget::new(
            identity, witness, connection,
        )
        .expect("matching target");
        let err = OpenAiProviderRuntime
            .build_realtime_session_factory(target)
            .err()
            .expect("gpt-live rows never reach the Realtime WebSocket factory");
        assert!(
            matches!(err, ProviderClientError::ClientInit(message) if message.contains("gpt-live"))
        );
    }

    #[test]
    fn realtime_session_factory_is_constructed_by_openai_runtime_gate() {
        let result = OpenAiProviderRuntime
            .build_realtime_session_factory(resolved_realtime_target(resolved_openai_connection()));

        #[cfg(all(not(target_arch = "wasm32"), feature = "realtime"))]
        assert!(
            result.is_ok(),
            "native realtime builds should mint the session factory inside the provider runtime"
        );

        #[cfg(not(all(not(target_arch = "wasm32"), feature = "realtime")))]
        assert!(matches!(
            result,
            Err(ProviderClientError::MissingFeature("openai-realtime"))
        ));
    }

    #[test]
    fn realtime_session_factory_rejects_provider_specific_unsupported_backends() {
        let chatgpt = expect_realtime_session_factory_error(
            OpenAiProviderRuntime.build_realtime_session_factory(resolved_realtime_target(
                resolved_chatgpt_connection(AuthMetadata::default(), None),
            )),
        );
        assert!(matches!(
            chatgpt,
            ProviderClientError::MissingFeature("openai-realtime-chatgpt-backend")
        ));

        let azure = expect_realtime_session_factory_error(
            OpenAiProviderRuntime.build_realtime_session_factory(resolved_realtime_target(
                resolved_azure_connection(serde_json::Value::Null),
            )),
        );
        assert!(matches!(
            azure,
            ProviderClientError::MissingFeature("openai-realtime-azure-openai")
        ));
    }

    #[test]
    fn realtime_session_factory_rejects_runtime_owned_unsupported_auth_and_url_shapes() {
        let mut custom_url = resolved_openai_connection();
        custom_url.backend_profile = Arc::new(BackendProfile {
            base_url: Some("https://example.test/openai".to_string()),
            ..backend("openai_api")
        });
        let err = expect_realtime_session_factory_error(
            OpenAiProviderRuntime
                .build_realtime_session_factory(resolved_realtime_target(custom_url)),
        );
        assert!(matches!(
            err,
            ProviderClientError::MissingFeature("openai-realtime-custom-base-url")
        ));

        let mut dynamic_auth = resolved_openai_connection();
        dynamic_auth.auth_lease = Arc::new(DynamicLease::new(
            Arc::new(TestAuthorizer),
            AuthMetadata::default(),
            None,
            "openai:dynamic",
        ));
        let err = expect_realtime_session_factory_error(
            OpenAiProviderRuntime
                .build_realtime_session_factory(resolved_realtime_target(dynamic_auth)),
        );
        assert!(matches!(
            err,
            ProviderClientError::MissingFeature("openai-realtime-authorizer-auth")
        ));
    }

    #[test]
    fn azure_openai_without_image_deployment_does_not_claim_image_executor() {
        let executor = OpenAiProviderRuntime
            .build_image_generation_executor(resolved_azure_connection(serde_json::Value::Null))
            .expect("Azure text connection should build cleanly");

        assert!(
            executor.is_none(),
            "Azure OpenAI should not shadow another OpenAI image binding unless image deployment is configured"
        );
    }

    #[test]
    fn azure_openai_with_image_deployment_claims_image_executor() {
        let executor = OpenAiProviderRuntime
            .build_image_generation_executor(resolved_azure_connection(serde_json::json!({
                "image_generation_deployment": "gpt-image-2"
            })))
            .expect("Azure image-capable connection should build cleanly");

        assert!(
            executor.is_some(),
            "Azure OpenAI image executor should remain available when image deployment is configured"
        );
    }

    #[test]
    fn chatgpt_backend_headers_use_openai_oauth_metadata() {
        let connection = resolved_chatgpt_connection(
            AuthMetadata {
                provider_metadata: Some(ProviderAuthMetadata::OpenAi(OpenAiAuthMetadata {
                    account_id: Some("acct_123".into()),
                    is_fedramp: Some(true),
                    ..Default::default()
                })),
                ..Default::default()
            },
            None,
        );

        let headers = chatgpt_backend_extra_headers(&connection);

        assert!(headers.contains(&(
            meerkat_core::provider_matrix::openai_auth::CHATGPT_ACCOUNT_HEADER.to_string(),
            "acct_123".to_string(),
        )));
        assert!(headers.contains(&(
            meerkat_core::provider_matrix::openai_auth::FEDRAMP_HEADER.to_string(),
            "true".to_string(),
        )));
    }

    #[test]
    fn chatgpt_backend_headers_fall_back_to_generic_account_metadata() {
        let connection = resolved_chatgpt_connection(
            AuthMetadata {
                account_id: Some("acct_generic".into()),
                ..Default::default()
            },
            None,
        );

        let headers = chatgpt_backend_extra_headers(&connection);

        assert_eq!(
            headers,
            vec![(
                meerkat_core::provider_matrix::openai_auth::CHATGPT_ACCOUNT_HEADER.to_string(),
                "acct_generic".to_string(),
            )]
        );
    }

    // The existing static-image positive does not select the authorizer branch.
    // This fixture signs a real image HTTP request, without claiming managed
    // refresh or generated-owner coverage from the fixture authorizer itself.
    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn dynamic_chatgpt_image_executor_preserves_oauth_account_headers() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct ImageBearerAuthorizer {
            calls: Arc<AtomicUsize>,
        }
        #[async_trait::async_trait]
        impl meerkat_core::HttpAuthorizer for ImageBearerAuthorizer {
            async fn authorize(
                &self,
                request: &mut meerkat_core::HttpAuthorizationRequest<'_>,
            ) -> Result<(), meerkat_core::AuthError> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                request
                    .headers
                    .push(("Authorization".into(), "Bearer dynamic-image-token".into()));
                Ok(())
            }

            fn label(&self) -> &'static str {
                "image-header-fixture"
            }
        }

        let metadata = AuthMetadata {
            provider_metadata: Some(ProviderAuthMetadata::OpenAi(OpenAiAuthMetadata {
                account_id: Some("acct_dynamic_image".into()),
                is_fedramp: Some(true),
                ..Default::default()
            })),
            ..Default::default()
        };
        let (base_url, seen, handle) = spawn_image_header_stub().await;
        let calls = Arc::new(AtomicUsize::new(0));
        let mut connection = resolved_chatgpt_connection(metadata.clone(), Some(base_url));
        connection.auth_lease = Arc::new(DynamicLease::from_authorizer(
            Arc::new(ImageBearerAuthorizer {
                calls: Arc::clone(&calls),
            }),
            metadata,
            "openai:dynamic-image",
        ));
        // Pin which actual builder branch this regression exercises.
        let dynamic_shape =
            connection.resolved_authorizer().is_some() && connection.resolved_secret().is_none();
        let request = hosted_image_request();
        let operation_id = request.operation_id;
        let result = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let executor = OpenAiProviderRuntime
                .build_image_generation_executor(connection)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "missing ChatGPT image executor".to_string())?;
            executor
                .execute_image_generation(request)
                .await
                .map_err(|error| error.to_string())
        })
        .await;
        // Clean up before any result/header assertion, including timeout/error.
        handle.abort();
        let joined = tokio::time::timeout(std::time::Duration::from_secs(5), handle)
            .await
            .expect("image stub stops within bound");
        assert!(joined.is_err_and(|error| error.is_cancelled()));
        assert!(
            dynamic_shape,
            "test must select actual authorizer-backed branch"
        );
        let output = result
            .expect("bounded image HTTP request")
            .expect("image execution succeeds");
        assert_eq!(output.operation_id, operation_id);
        assert!(matches!(
            output.terminal_observation,
            ImageProviderTerminalObservation::Generated
        ));
        assert_eq!(output.images.len(), 1, "actual streamed image was retained");
        assert_eq!(output.images[0].base64_data, "aGVsbG8=");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "actual authorizer ran once"
        );
        let headers = seen.lock().expect("captured image headers");
        assert_eq!(headers.len(), 1, "exactly one actual image HTTP request");
        let actual = &headers[0];
        assert_eq!(
            actual.get("authorization").and_then(|v| v.to_str().ok()),
            Some("Bearer dynamic-image-token")
        );
        assert_eq!(
            actual
                .get(meerkat_core::provider_matrix::openai_auth::CHATGPT_ACCOUNT_HEADER)
                .and_then(|v| v.to_str().ok()),
            Some("acct_dynamic_image")
        );
        assert_eq!(
            actual
                .get(meerkat_core::provider_matrix::openai_auth::FEDRAMP_HEADER)
                .and_then(|v| v.to_str().ok()),
            Some("true")
        );
    }

    #[tokio::test]
    async fn chatgpt_backend_image_executor_sends_oauth_account_headers()
    -> Result<(), Box<dyn std::error::Error>> {
        let (base_url, seen, handle) = spawn_image_header_stub().await;
        let connection = resolved_chatgpt_connection(
            AuthMetadata {
                provider_metadata: Some(ProviderAuthMetadata::OpenAi(OpenAiAuthMetadata {
                    account_id: Some("acct_image".into()),
                    is_fedramp: Some(true),
                    ..Default::default()
                })),
                ..Default::default()
            },
            Some(base_url),
        );

        let executor = OpenAiProviderRuntime
            .build_image_generation_executor(connection)?
            .expect("chatgpt backend should provide image executor");
        let output = executor
            .execute_image_generation(hosted_image_request())
            .await?;

        assert!(matches!(
            output.terminal_observation,
            ImageProviderTerminalObservation::Generated
        ));
        let headers = seen.lock().expect("seen headers");
        let first = headers.first().expect("captured image request headers");
        assert_eq!(
            first
                .get(meerkat_core::provider_matrix::openai_auth::CHATGPT_ACCOUNT_HEADER)
                .and_then(|value| value.to_str().ok()),
            Some("acct_image")
        );
        assert_eq!(
            first
                .get(meerkat_core::provider_matrix::openai_auth::FEDRAMP_HEADER)
                .and_then(|value| value.to_str().ok()),
            Some("true")
        );

        handle.abort();
        Ok(())
    }
}

#[cfg(all(test, not(target_arch = "wasm32"), feature = "oauth"))]
#[test]
fn ce_stale_refresh_remains_stale_credential() {
    let result = openai_oauth_refresh_error(
        oauth::OpenAiOAuthError::Refresh(meerkat_auth_core::RefreshError::StalePreparation),
        String::new(),
    );
    assert!(matches!(
        result,
        ProviderAuthError::Auth(AuthError::StaleCredential)
    ));
}

#[cfg(all(test, feature = "oauth", not(target_arch = "wasm32")))]
mod managed_lifetime_tests;
