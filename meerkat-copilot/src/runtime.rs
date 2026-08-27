use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use futures::StreamExt;
use meerkat_auth_core::auth_oauth::{exchange_refresh_token, oauth_refresh_error};
use meerkat_auth_core::resolver::{
    LockedManagedStoreOAuthRefresh, ManagedStoreOAuthRefreshPreparationSlot,
    OAuthLoginCredentialAdmission, load_managed_store_tokens_with_lifecycle,
    prepare_managed_store_oauth_refresh_under_lock, resolve_oauth_login_credential_disposition,
};
use meerkat_core::auth::{
    PersistedAuthMode, PersistedTokens, ProviderAuthPersistence, RefreshError, RefreshFn,
};
use meerkat_core::{
    AuthCredentialIdentity, AuthError, AuthMetadata, HttpAuthorizationReceipt,
    HttpAuthorizationRequest, HttpAuthorizationResponse, HttpAuthorizationResponseAction,
    HttpAuthorizer, Provider,
};
use meerkat_llm_core::provider_runtime::{
    ProviderAuthError, ProviderClientError, ResolverEnvironment, ValidatedBinding,
};
use parking_lot::{Mutex, RwLock};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};

use crate::protocol::{
    CopilotBackendConfig, CopilotEndpoint, CopilotModelSnapshot, CopilotModelsEnvelope,
    CopilotTokenEnvelope, CopilotTokenErrorEnvelope, DEFAULT_COPILOT_API_BASE,
};

const CAPI_EXPIRY_SKEW_SECONDS: i64 = 300;
const MODEL_DISCOVERY_RETRY_SECONDS: i64 = 30;
const BEARER_SCHEME: &str = "Bearer";

#[derive(Clone, Copy)]
enum CopilotRequestIntent {
    ModelAccess,
    ConversationPanel,
}

impl CopilotRequestIntent {
    fn as_str(self) -> &'static str {
        match self {
            Self::ModelAccess => "model-access",
            Self::ConversationPanel => "conversation-panel",
        }
    }
}

struct CopilotHttpResponse {
    status: u16,
    body: Vec<u8>,
}

#[derive(Debug, thiserror::Error)]
enum CopilotHttpTransportError {
    #[error("Copilot HTTP request failed")]
    Request(#[source] reqwest::Error),
    #[cfg(test)]
    #[error("{0}")]
    Scripted(String),
}

#[async_trait]
trait CopilotHttpTransport: Send + Sync {
    async fn get(
        &self,
        url: &str,
        headers: HeaderMap,
    ) -> Result<CopilotHttpResponse, CopilotHttpTransportError>;
}

#[derive(Default)]
struct ReqwestCopilotHttpTransport {
    client: reqwest::Client,
}

#[async_trait]
impl CopilotHttpTransport for ReqwestCopilotHttpTransport {
    async fn get(
        &self,
        url: &str,
        headers: HeaderMap,
    ) -> Result<CopilotHttpResponse, CopilotHttpTransportError> {
        let response = self
            .client
            .get(url)
            .headers(headers)
            .send()
            .await
            .map_err(CopilotHttpTransportError::Request)?;
        let status = response.status().as_u16();
        let body = response
            .bytes()
            .await
            .map_err(CopilotHttpTransportError::Request)?
            .to_vec();
        Ok(CopilotHttpResponse { status, body })
    }
}

pub struct CopilotRuntime {
    accounts: Mutex<BTreeMap<CopilotAccountCacheKey, Weak<CopilotAccountState>>>,
    transport: Arc<dyn CopilotHttpTransport>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct CopilotAccountCacheKey {
    persistence_id: meerkat_core::auth::ProviderAuthPersistenceId,
    credential_identity: AuthCredentialIdentity,
}

impl CopilotRuntime {
    pub fn new() -> Self {
        Self {
            accounts: Mutex::new(BTreeMap::new()),
            transport: Arc::new(ReqwestCopilotHttpTransport::default()),
        }
    }

    pub async fn resolve(
        &self,
        binding: &ValidatedBinding,
        env: &ResolverEnvironment,
    ) -> Result<CopilotResolvedAuth, ProviderAuthError> {
        if binding.auth().persisted_auth_mode() != Some(PersistedAuthMode::GithubCopilotOauth) {
            return Err(ProviderAuthError::SourceResolutionFailed(
                "Copilot backend requires github_copilot_oauth".to_string(),
            ));
        }
        if !matches!(
            binding.auth_profile().source,
            meerkat_core::CredentialSourceSpec::ManagedStore
        ) {
            return Err(ProviderAuthError::SourceResolutionFailed(
                "github_copilot_oauth requires source.kind = managed_store".to_string(),
            ));
        }
        let Some(persistence) = env.provider_auth_persistence() else {
            return Err(ProviderAuthError::SourceResolutionFailed(
                "Copilot requires provider-auth persistence and AuthMachine authority".to_string(),
            ));
        };
        if env.auth_lease_handle.is_none() {
            return Err(ProviderAuthError::SourceResolutionFailed(
                "Copilot requires provider-auth persistence and AuthMachine authority".to_string(),
            ));
        }

        let config = CopilotBackendConfig::from_options(&binding.backend_profile().options)
            .map_err(|error| ProviderAuthError::SourceResolutionFailed(error.to_string()))?;
        let state = self.account_state(
            persistence.authority_id(),
            binding.credential_identity(),
            config,
        )?;
        let authorizer = Arc::new(CopilotAuthorizer {
            state,
            binding: binding.clone(),
            env: env.clone(),
            transport: Arc::clone(&self.transport),
        });
        authorizer.prime().await?;
        let metadata =
            meerkat_auth_core::resolver::finalize_auth_metadata(binding, AuthMetadata::default())?;
        Ok(CopilotResolvedAuth {
            authorizer,
            metadata,
        })
    }

    fn account_state(
        &self,
        persistence_id: meerkat_core::auth::ProviderAuthPersistenceId,
        identity: &AuthCredentialIdentity,
        config: CopilotBackendConfig,
    ) -> Result<Arc<CopilotAccountState>, ProviderAuthError> {
        let key = CopilotAccountCacheKey {
            persistence_id,
            credential_identity: identity.clone(),
        };
        let mut accounts = self.accounts.lock();
        accounts.retain(|_, state| state.strong_count() > 0);
        if let Some(existing) = accounts.get(&key).and_then(Weak::upgrade) {
            if existing.config != config {
                return Err(ProviderAuthError::SourceResolutionFailed(format!(
                    "Copilot routes sharing credential identity '{identity}' disagree on backend options"
                )));
            }
            return Ok(existing);
        }
        let state = Arc::new(CopilotAccountState {
            config,
            cache: RwLock::new(None),
            refresh_lock: tokio::sync::Mutex::new(()),
            next_generation: AtomicU64::new(1),
        });
        accounts.insert(key, Arc::downgrade(&state));
        Ok(state)
    }

    pub fn resolve_route(
        &self,
        connection: &meerkat_llm_core::provider_runtime::ResolvedConnection,
        provider: Provider,
        model: &str,
    ) -> Option<CopilotRouteResolution> {
        let persistence_id = match connection.auth_lease.kind() {
            meerkat_core::ResolvedAuthKind::DynamicAuthorizer(authorizer) => {
                authorizer.persistence_authority_id()?
            }
            _ => return None,
        };
        let key = CopilotAccountCacheKey {
            persistence_id,
            credential_identity: connection.credential_identity.clone(),
        };
        let Some(state) = self.accounts.lock().get(&key).and_then(Weak::upgrade) else {
            return None;
        };
        let cache = state.cache.read();
        let cached = cache.as_ref()?;
        let (access, capabilities, available_model_ids) = match cached.models.as_ref() {
            Some(snapshot) => {
                let model = snapshot.model(model);
                let access = model.map_or(CopilotModelAccess::Unavailable, |model| {
                    model
                        .route_for(provider)
                        .map_or(CopilotModelAccess::Unavailable, |endpoint| {
                            CopilotModelAccess::Available { endpoint }
                        })
                });
                (
                    access,
                    model.map(|model| model.capabilities.clone()),
                    snapshot.available_model_ids(provider),
                )
            }
            None => (CopilotModelAccess::Unknown, None, Vec::new()),
        };
        Some(CopilotRouteResolution {
            access,
            api_base: cached.api_base.clone(),
            capabilities,
            available_model_ids,
        })
    }
}

impl Default for CopilotRuntime {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone)]
pub struct CopilotResolvedAuth {
    authorizer: Arc<CopilotAuthorizer>,
    metadata: AuthMetadata,
}

impl CopilotResolvedAuth {
    pub fn authorizer(&self) -> Arc<dyn HttpAuthorizer> {
        self.authorizer.clone()
    }

    pub fn concrete_authorizer(&self) -> Arc<CopilotAuthorizer> {
        self.authorizer.clone()
    }

    pub fn metadata(&self) -> &AuthMetadata {
        &self.metadata
    }

    pub fn model_snapshot(&self) -> Option<CopilotModelSnapshot> {
        self.authorizer.model_snapshot()
    }

    pub fn route_for(&self, provider: Provider, model: &str) -> CopilotModelAccess {
        let Some(snapshot) = self.model_snapshot() else {
            return CopilotModelAccess::Unknown;
        };
        let Some(model_access) = snapshot.model(model) else {
            return CopilotModelAccess::Unavailable;
        };
        model_access
            .route_for(provider)
            .map_or(CopilotModelAccess::Unavailable, |endpoint| {
                CopilotModelAccess::Available { endpoint }
            })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopilotModelAccess {
    Available { endpoint: CopilotEndpoint },
    Unavailable,
    Unknown,
}

pub struct CopilotRouteResolution {
    pub access: CopilotModelAccess,
    pub api_base: String,
    pub capabilities: Option<crate::protocol::CopilotModelCapabilities>,
    pub available_model_ids: Vec<String>,
}

impl CopilotRouteResolution {
    pub fn unavailable_message(
        &self,
        provider: Provider,
        model: &str,
        endpoint: CopilotEndpoint,
    ) -> String {
        let suffix = if self.available_model_ids.is_empty() {
            String::new()
        } else {
            format!(
                "; available account models: {}",
                self.available_model_ids.join(", ")
            )
        };
        format!(
            "Copilot account does not expose {} for {} model '{model}'{suffix}",
            endpoint.path(),
            provider.as_str(),
        )
    }
}

pub struct CopilotChatCompletionsClientSpec {
    provider: Provider,
    model: String,
    api_base: String,
    authorizer: Arc<dyn HttpAuthorizer>,
    supports_temperature: bool,
    supports_thinking: bool,
    supports_reasoning: bool,
    supports_image_tool_results: bool,
}

impl CopilotChatCompletionsClientSpec {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        provider: Provider,
        model: String,
        api_base: String,
        authorizer: Arc<dyn HttpAuthorizer>,
        supports_temperature: bool,
        supports_thinking: bool,
        supports_reasoning: bool,
        supports_image_tool_results: bool,
    ) -> Self {
        Self {
            provider,
            model,
            api_base,
            authorizer,
            supports_temperature,
            supports_thinking,
            supports_reasoning,
            supports_image_tool_results,
        }
    }

    #[allow(clippy::type_complexity)]
    pub fn into_parts(
        self,
    ) -> (
        Provider,
        String,
        String,
        Arc<dyn HttpAuthorizer>,
        bool,
        bool,
        bool,
        bool,
    ) {
        (
            self.provider,
            self.model,
            self.api_base,
            self.authorizer,
            self.supports_temperature,
            self.supports_thinking,
            self.supports_reasoning,
            self.supports_image_tool_results,
        )
    }
}

pub trait CopilotChatCompletionsClientFactory: Send + Sync {
    fn build(
        &self,
        spec: CopilotChatCompletionsClientSpec,
    ) -> Result<Arc<dyn meerkat_llm_core::LlmClient>, ProviderClientError>;
}

pub fn capability_gated_client(
    client: Arc<dyn meerkat_llm_core::LlmClient>,
    capabilities: Option<crate::protocol::CopilotModelCapabilities>,
) -> Arc<dyn meerkat_llm_core::LlmClient> {
    match capabilities {
        Some(capabilities) => Arc::new(CopilotCapabilityGatedClient {
            inner: client,
            capabilities,
        }),
        None => client,
    }
}

pub type CopilotRouteClientFactory = Arc<
    dyn Fn(
            &CopilotRouteResolution,
            &meerkat_llm_core::provider_runtime::ResolvedConnection,
        ) -> Result<Arc<dyn meerkat_llm_core::LlmClient>, ProviderClientError>
        + Send
        + Sync,
>;

pub fn routed_client(
    runtime: Arc<CopilotRuntime>,
    connection: meerkat_llm_core::provider_runtime::ResolvedConnection,
    provider: Provider,
    model: String,
    factory: CopilotRouteClientFactory,
) -> Result<Arc<dyn meerkat_llm_core::LlmClient>, ProviderClientError> {
    let client = CopilotRoutedClient {
        runtime,
        connection,
        provider,
        model,
        factory,
    };
    client.current_client()?;
    Ok(Arc::new(client))
}

struct CopilotRoutedClient {
    runtime: Arc<CopilotRuntime>,
    connection: meerkat_llm_core::provider_runtime::ResolvedConnection,
    provider: Provider,
    model: String,
    factory: CopilotRouteClientFactory,
}

impl CopilotRoutedClient {
    fn current_client(&self) -> Result<Arc<dyn meerkat_llm_core::LlmClient>, ProviderClientError> {
        let route = self
            .runtime
            .resolve_route(&self.connection, self.provider, &self.model)
            .ok_or_else(|| {
                ProviderClientError::ClientInit(
                    "Copilot route was not initialized during auth resolution".to_string(),
                )
            })?;
        let capabilities = route.capabilities.clone();
        let client = (self.factory)(&route, &self.connection)?;
        Ok(capability_gated_client(client, capabilities))
    }

    fn client_error(error: ProviderClientError) -> meerkat_llm_core::LlmError {
        meerkat_llm_core::LlmError::InvalidRequest {
            message: error.to_string(),
        }
    }
}

#[async_trait]
impl meerkat_llm_core::LlmClient for CopilotRoutedClient {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, meerkat_llm_core::LlmError> {
        self.current_client()
            .map_err(Self::client_error)?
            .project_replay_messages(messages)
    }

    fn request_pressure(
        &self,
        request: &meerkat_llm_core::LlmRequest,
    ) -> Result<Option<meerkat_core::ProviderRequestPressure>, meerkat_llm_core::LlmError> {
        self.current_client()
            .map_err(Self::client_error)?
            .request_pressure(request)
    }

    fn authored_cache_breakpoints(
        &self,
        request: &meerkat_llm_core::LlmRequest,
        canonical_messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::ProviderCacheBreakpointClaim>, meerkat_llm_core::LlmError> {
        self.current_client()
            .map_err(Self::client_error)?
            .authored_cache_breakpoints(request, canonical_messages)
    }

    fn stream<'a>(
        &'a self,
        request: &'a meerkat_llm_core::LlmRequest,
    ) -> meerkat_llm_core::LlmStream<'a> {
        let request = request.clone();
        Box::pin(async_stream::try_stream! {
            let authorizer = self
                .connection
                .resolved_authorizer()
                .ok_or_else(|| meerkat_llm_core::LlmError::AuthenticationFailed {
                    message: "Copilot route has no dynamic authorizer".to_string(),
                })?;
            authorizer
                .prepare_request()
                .await
                .map_err(|error| meerkat_llm_core::LlmError::AuthenticationFailed {
                    message: error.to_string(),
                })?;
            let client = self.current_client().map_err(Self::client_error)?;
            let mut stream = client.stream(&request);
            while let Some(event) = stream.next().await {
                yield event?;
            }
        })
    }

    fn provider(&self) -> Provider {
        self.provider
    }

    async fn health_check(&self) -> Result<(), meerkat_llm_core::LlmError> {
        let authorizer = self.connection.resolved_authorizer().ok_or_else(|| {
            meerkat_llm_core::LlmError::AuthenticationFailed {
                message: "Copilot route has no dynamic authorizer".to_string(),
            }
        })?;
        authorizer.prepare_request().await.map_err(|error| {
            meerkat_llm_core::LlmError::AuthenticationFailed {
                message: error.to_string(),
            }
        })?;
        self.current_client()
            .map_err(Self::client_error)?
            .health_check()
            .await
    }

    fn compile_schema(
        &self,
        output_schema: &meerkat_core::OutputSchema,
    ) -> Result<meerkat_core::schema::CompiledSchema, meerkat_core::schema::SchemaError> {
        self.current_client()
            .map_err(
                |error| meerkat_core::schema::SchemaError::UnsupportedFeatures {
                    provider: self.provider,
                    warnings: vec![meerkat_core::schema::SchemaWarning {
                        provider: self.provider,
                        path: "$".to_string(),
                        message: error.to_string(),
                    }],
                },
            )?
            .compile_schema(output_schema)
    }
}

struct CopilotCapabilityGatedClient {
    inner: Arc<dyn meerkat_llm_core::LlmClient>,
    capabilities: crate::protocol::CopilotModelCapabilities,
}

impl CopilotCapabilityGatedClient {
    fn validate_request(
        &self,
        request: &meerkat_llm_core::LlmRequest,
    ) -> Result<(), meerkat_llm_core::LlmError> {
        let supports = &self.capabilities.supports;
        if supports.streaming == Some(false) {
            return Err(meerkat_llm_core::LlmError::InvalidRequest {
                message: format!(
                    "Copilot account model '{}' does not support streaming",
                    request.model
                ),
            });
        }
        if supports.tool_calls == Some(false) && !request.tools.is_empty() {
            return Err(meerkat_llm_core::LlmError::InvalidRequest {
                message: format!(
                    "Copilot account model '{}' does not support tool calls",
                    request.model
                ),
            });
        }
        if supports.vision == Some(false) && request_contains_images(request) {
            return Err(meerkat_llm_core::LlmError::InvalidRequest {
                message: format!(
                    "Copilot account model '{}' does not support image input",
                    request.model
                ),
            });
        }
        if let Some(max_output_tokens) = self.capabilities.limits.max_output_tokens
            && request.max_tokens > max_output_tokens
        {
            return Err(meerkat_llm_core::LlmError::InvalidRequest {
                message: format!(
                    "Copilot account model '{}' allows at most {max_output_tokens} output tokens, requested {}",
                    request.model, request.max_tokens
                ),
            });
        }
        Ok(())
    }
}

fn request_contains_images(request: &meerkat_llm_core::LlmRequest) -> bool {
    request.messages.iter().any(|message| match message {
        meerkat_core::Message::User(user) => user.has_images(),
        meerkat_core::Message::BlockAssistant(assistant) => assistant
            .blocks
            .iter()
            .any(|block| matches!(block, meerkat_core::AssistantBlock::Image { .. })),
        meerkat_core::Message::ToolResults { results, .. } => {
            results.iter().any(meerkat_core::ToolResult::has_images)
        }
        meerkat_core::Message::System(_) | meerkat_core::Message::SystemNotice(_) => false,
    })
}

#[async_trait]
impl meerkat_llm_core::LlmClient for CopilotCapabilityGatedClient {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, meerkat_llm_core::LlmError> {
        self.inner.project_replay_messages(messages)
    }

    fn request_pressure(
        &self,
        request: &meerkat_llm_core::LlmRequest,
    ) -> Result<Option<meerkat_core::ProviderRequestPressure>, meerkat_llm_core::LlmError> {
        self.validate_request(request)?;
        self.inner.request_pressure(request)
    }

    fn authored_cache_breakpoints(
        &self,
        request: &meerkat_llm_core::LlmRequest,
        canonical_messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::ProviderCacheBreakpointClaim>, meerkat_llm_core::LlmError> {
        self.inner
            .authored_cache_breakpoints(request, canonical_messages)
    }

    fn stream<'a>(
        &'a self,
        request: &'a meerkat_llm_core::LlmRequest,
    ) -> meerkat_llm_core::LlmStream<'a> {
        if let Err(error) = self.validate_request(request) {
            return Box::pin(futures::stream::once(async move { Err(error) }));
        }
        self.inner.stream(request)
    }

    fn provider(&self) -> Provider {
        self.inner.provider()
    }

    async fn health_check(&self) -> Result<(), meerkat_llm_core::LlmError> {
        self.inner.health_check().await
    }

    fn compile_schema(
        &self,
        output_schema: &meerkat_core::OutputSchema,
    ) -> Result<meerkat_core::schema::CompiledSchema, meerkat_core::schema::SchemaError> {
        if self.capabilities.supports.structured_outputs == Some(false) {
            return Err(meerkat_core::schema::SchemaError::UnsupportedFeatures {
                provider: self.provider(),
                warnings: vec![meerkat_core::schema::SchemaWarning {
                    provider: self.provider(),
                    path: "$".to_string(),
                    message: "Copilot account model does not support structured outputs"
                        .to_string(),
                }],
            });
        }
        self.inner.compile_schema(output_schema)
    }
}

struct CopilotAccountState {
    config: CopilotBackendConfig,
    cache: RwLock<Option<CachedCopilotToken>>,
    refresh_lock: tokio::sync::Mutex<()>,
    next_generation: AtomicU64,
}

struct CachedCopilotToken {
    generation: u64,
    token: String,
    source_generation: u64,
    source_expires_at: Option<DateTime<Utc>>,
    refresh_at: DateTime<Utc>,
    api_base: String,
    models: Option<CopilotModelSnapshot>,
    model_discovery_retry_at: Option<DateTime<Utc>>,
}

impl std::fmt::Debug for CachedCopilotToken {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CachedCopilotToken")
            .field("generation", &self.generation)
            .field("token", &"<redacted>")
            .field("source_generation", &self.source_generation)
            .field("source_expires_at", &self.source_expires_at)
            .field("refresh_at", &self.refresh_at)
            .field("api_base", &self.api_base)
            .field("has_models", &self.models.is_some())
            .field("model_discovery_retry_at", &self.model_discovery_retry_at)
            .finish()
    }
}

pub struct CopilotAuthorizer {
    state: Arc<CopilotAccountState>,
    binding: ValidatedBinding,
    env: ResolverEnvironment,
    transport: Arc<dyn CopilotHttpTransport>,
}

impl CopilotAuthorizer {
    pub async fn prime(&self) -> Result<(), ProviderAuthError> {
        self.ensure_token().await.map(|_| ())
    }

    pub fn credential_identity(&self) -> &AuthCredentialIdentity {
        self.binding.credential_identity()
    }

    pub fn model_snapshot(&self) -> Option<CopilotModelSnapshot> {
        self.state
            .cache
            .read()
            .as_ref()
            .and_then(|cached| cached.models.clone())
    }

    pub fn api_base(&self) -> Option<String> {
        self.state
            .cache
            .read()
            .as_ref()
            .map(|cached| cached.api_base.clone())
    }

    async fn invalidate_derived_token_generation(&self, generation: u64) {
        let _guard = self.state.refresh_lock.lock().await;
        let mut cache = self.state.cache.write();
        if cache
            .as_ref()
            .is_some_and(|cached| cached.generation == generation)
        {
            *cache = None;
        }
    }

    async fn ensure_token(&self) -> Result<CachedTokenView, ProviderAuthError> {
        let source = self.load_source_tokens().await?;
        let source_generation = meerkat_core::tokens_lifecycle_published_generation(&source)
            .ok_or_else(|| {
                ProviderAuthError::SourceResolutionFailed(
                    "Copilot source credential has no AuthMachine publication generation"
                        .to_string(),
                )
            })?;
        let now = (self.env.now)();
        if let Some(cached) = self.fresh_cached(source_generation, now)
            && !cached.model_discovery_due
        {
            return Ok(cached);
        }

        let _guard = self.state.refresh_lock.lock().await;
        let source = self.load_source_tokens().await?;
        let source_generation = meerkat_core::tokens_lifecycle_published_generation(&source)
            .ok_or_else(|| {
                ProviderAuthError::SourceResolutionFailed(
                    "Copilot source credential has no AuthMachine publication generation"
                        .to_string(),
                )
            })?;
        let now = (self.env.now)();
        if let Some(cached) = self.fresh_cached(source_generation, now) {
            if cached.model_discovery_due {
                let models = self.fetch_models(&cached.api_base, &cached.token).await;
                let mut cache = self.state.cache.write();
                if let Some(current) = cache.as_mut()
                    && current.source_generation == source_generation
                    && current.token == cached.token
                {
                    match models {
                        Ok(snapshot) => {
                            current.models = Some(snapshot);
                            current.model_discovery_retry_at = None;
                        }
                        Err(error) => {
                            tracing::warn!(
                                credential_identity = %self.binding.credential_identity(),
                                error = %error,
                                "Copilot account model discovery retry failed"
                            );
                            current.model_discovery_retry_at =
                                Some(now + Duration::seconds(MODEL_DISCOVERY_RETRY_SECONDS));
                        }
                    }
                }
            }
            return Ok(cached);
        }

        let github_token = source
            .primary_secret
            .as_deref()
            .ok_or(ProviderAuthError::Auth(AuthError::MissingSecret))?;
        let token = self.mint_copilot_token(github_token).await?;
        let api_base = resolve_api_base(&token)?;
        let refresh_at = token_refresh_at(&token, now)?;
        let (models, model_discovery_retry_at) =
            match self.fetch_models(&api_base, &token.token).await {
                Ok(snapshot) => (Some(snapshot), None),
                Err(error) => {
                    tracing::warn!(
                        credential_identity = %self.binding.credential_identity(),
                        error = %error,
                        "Copilot authentication succeeded but account model discovery failed"
                    );
                    (
                        None,
                        Some(now + Duration::seconds(MODEL_DISCOVERY_RETRY_SECONDS)),
                    )
                }
            };
        let generation = self.state.next_generation.fetch_add(1, Ordering::Relaxed);
        let view = CachedTokenView {
            generation,
            token: token.token.clone(),
            api_base: api_base.clone(),
            model_discovery_due: false,
        };
        *self.state.cache.write() = Some(CachedCopilotToken {
            generation,
            token: token.token,
            source_generation,
            source_expires_at: source.expires_at,
            refresh_at,
            api_base,
            models,
            model_discovery_retry_at,
        });
        Ok(view)
    }

    fn fresh_cached(&self, source_generation: u64, now: DateTime<Utc>) -> Option<CachedTokenView> {
        let cache = self.state.cache.read();
        let cached = cache.as_ref()?;
        (cached.source_generation == source_generation && now < cached.refresh_at).then(|| {
            CachedTokenView {
                generation: cached.generation,
                token: cached.token.clone(),
                api_base: cached.api_base.clone(),
                model_discovery_due: cached
                    .model_discovery_retry_at
                    .is_some_and(|retry_at| now >= retry_at),
            }
        })
    }

    async fn load_source_tokens(&self) -> Result<PersistedTokens, ProviderAuthError> {
        let mut managed =
            load_managed_store_tokens_with_lifecycle(&self.env, &self.binding).await?;
        match resolve_oauth_login_credential_disposition(
            &self.env,
            &self.binding,
            managed.tokens.primary_secret.is_some(),
        )? {
            OAuthLoginCredentialAdmission::UseCached => Ok(managed.tokens),
            OAuthLoginCredentialAdmission::BeginRefresh => {
                managed.release_prelock_lifecycle_guard();
                self.refresh_source_token(managed).await
            }
        }
    }

    async fn refresh_source_token(
        &self,
        managed: meerkat_auth_core::resolver::ManagedStoreTokens,
    ) -> Result<PersistedTokens, ProviderAuthError> {
        let persistence = self
            .env
            .provider_auth_persistence()
            .cloned()
            .ok_or_else(|| {
                ProviderAuthError::SourceResolutionFailed(
                    "Copilot source refresh requires provider-auth persistence".to_string(),
                )
            })?;
        let preparation_env = self.env.clone();
        let preparation_binding = self.binding.clone();
        let preparation: meerkat_auth_core::resolver::ManagedStoreOAuthRefreshPrepareFn =
            Box::new(move |locked_baseline, mode| {
                Box::pin(async move {
                    prepare_managed_store_oauth_refresh_under_lock(
                        &preparation_env,
                        &preparation_binding,
                        managed,
                        locked_baseline,
                        mode,
                    )
                    .await
                    .map_err(|error| RefreshError::Refresh(error.to_string()))
                })
            });
        refresh_github_tokens(
            persistence,
            preparation,
            self.binding.credential_identity().clone(),
            self.state.config.clone(),
            self.env.force_refresh,
        )
        .await
        .map_err(|error| ProviderAuthError::Auth(auth_error_from_refresh(error)))
    }

    async fn mint_copilot_token(
        &self,
        github_token: &str,
    ) -> Result<CopilotTokenEnvelope, ProviderAuthError> {
        let endpoints = self
            .state
            .config
            .endpoints()
            .map_err(|error| ProviderAuthError::SourceResolutionFailed(error.to_string()))?;
        let mut headers = HeaderMap::new();
        insert_header(
            &mut headers,
            "Authorization",
            &format!("token {github_token}"),
        )?;
        insert_header(&mut headers, "Accept", "application/json")?;
        insert_header(
            &mut headers,
            "X-GitHub-Api-Version",
            &self.state.config.token_api_version,
        )?;
        insert_header(&mut headers, "User-Agent", &self.state.config.user_agent)?;
        insert_header(
            &mut headers,
            "Editor-Version",
            &self.state.config.editor_version,
        )?;
        insert_header(
            &mut headers,
            "Editor-Plugin-Version",
            &self.state.config.editor_plugin_version,
        )?;
        insert_header(
            &mut headers,
            "Copilot-Integration-Id",
            &self.state.config.integration_id,
        )?;
        let response = self
            .transport
            .get(endpoints.copilot_token_url.as_str(), headers)
            .await
            .map_err(|error| {
                ProviderAuthError::SourceResolutionFailed(format!(
                    "Copilot token exchange failed: {error}"
                ))
            })?;
        if !(200..=299).contains(&response.status) {
            let body = String::from_utf8_lossy(&response.body);
            if token_exchange_requires_account_action(response.status, &response.body) {
                if let Some(auth_lease) = self.env.auth_lease_handle.as_ref() {
                    let lease_key = meerkat_core::handles::LeaseKey::from_credential_identity(
                        self.binding.credential_identity(),
                    );
                    auth_lease.mark_reauth_required(&lease_key).map_err(|error| {
                        ProviderAuthError::SourceResolutionFailed(format!(
                            "Copilot source rejection could not be published to AuthMachine: {error}"
                        ))
                    })?;
                }
                return Err(ProviderAuthError::Auth(AuthError::UserReauthRequired));
            }
            return Err(ProviderAuthError::SourceResolutionFailed(format!(
                "Copilot token exchange returned {}: {body}",
                response.status
            )));
        }
        let token: CopilotTokenEnvelope =
            serde_json::from_slice(&response.body).map_err(|error| {
                ProviderAuthError::SourceResolutionFailed(format!(
                    "invalid Copilot token response: {error}"
                ))
            })?;
        if token.token.trim().is_empty() {
            return Err(ProviderAuthError::SourceResolutionFailed(
                "Copilot token response contains an empty token".to_string(),
            ));
        }
        let now_epoch = (self.env.now)().timestamp().max(0) as u64;
        if token.expires_at <= now_epoch {
            return Err(ProviderAuthError::SourceResolutionFailed(
                "Copilot token response is already expired".to_string(),
            ));
        }
        Ok(token)
    }

    async fn fetch_models(
        &self,
        api_base: &str,
        token: &str,
    ) -> Result<CopilotModelSnapshot, ProviderAuthError> {
        let url = format!("{}/models", api_base.trim_end_matches('/'));
        let response = self
            .transport
            .get(
                &url,
                self.inference_headers(token, CopilotRequestIntent::ModelAccess)?,
            )
            .await
            .map_err(|error| {
                ProviderAuthError::SourceResolutionFailed(format!(
                    "Copilot model discovery failed: {error}"
                ))
            })?;
        if !(200..=299).contains(&response.status) {
            let body = String::from_utf8_lossy(&response.body);
            return Err(ProviderAuthError::SourceResolutionFailed(format!(
                "Copilot model discovery returned {}: {body}",
                response.status
            )));
        }
        let body: CopilotModelsEnvelope =
            serde_json::from_slice(&response.body).map_err(|error| {
                ProviderAuthError::SourceResolutionFailed(format!(
                    "invalid Copilot models response: {error}"
                ))
            })?;
        CopilotModelSnapshot::from_models(body.data)
            .map_err(|error| ProviderAuthError::SourceResolutionFailed(error.to_string()))
    }

    fn inference_headers(
        &self,
        token: &str,
        intent: CopilotRequestIntent,
    ) -> Result<HeaderMap, ProviderAuthError> {
        let config = &self.state.config;
        let mut headers = HeaderMap::new();
        insert_header(
            &mut headers,
            "Authorization",
            &format!("{BEARER_SCHEME} {token}"),
        )?;
        insert_header(
            &mut headers,
            "X-GitHub-Api-Version",
            &config.inference_api_version,
        )?;
        insert_header(&mut headers, "User-Agent", &config.user_agent)?;
        insert_header(&mut headers, "Editor-Version", &config.editor_version)?;
        insert_header(
            &mut headers,
            "Editor-Plugin-Version",
            &config.editor_plugin_version,
        )?;
        insert_header(
            &mut headers,
            "Copilot-Integration-Id",
            &config.integration_id,
        )?;
        insert_header(&mut headers, "OpenAI-Intent", intent.as_str())?;
        insert_header(
            &mut headers,
            "X-Request-Id",
            &meerkat_core::time_compat::new_uuid_v7().to_string(),
        )?;
        Ok(headers)
    }
}

#[async_trait]
impl HttpAuthorizer for CopilotAuthorizer {
    async fn prepare_request(&self) -> Result<(), AuthError> {
        self.ensure_token()
            .await
            .map(|_| ())
            .map_err(auth_error_from_provider)
    }

    async fn authorize(&self, request: &mut HttpAuthorizationRequest<'_>) -> Result<(), AuthError> {
        self.authorize_attempt(request).await.map(|_| ())
    }

    async fn authorize_with_receipt(
        &self,
        request: &mut HttpAuthorizationRequest<'_>,
    ) -> Result<HttpAuthorizationReceipt, AuthError> {
        self.authorize_attempt(request)
            .await
            .map(HttpAuthorizationReceipt::tracked)
    }

    async fn observe_response(
        &self,
        response: &HttpAuthorizationResponse<'_>,
    ) -> Result<HttpAuthorizationResponseAction, AuthError> {
        if response.status == 401 {
            return Err(AuthError::Other(
                "Copilot 401 observation requires its authorization receipt".to_string(),
            ));
        }
        Ok(HttpAuthorizationResponseAction::Propagate)
    }

    async fn observe_response_with_receipt(
        &self,
        receipt: HttpAuthorizationReceipt,
        response: &HttpAuthorizationResponse<'_>,
    ) -> Result<HttpAuthorizationResponseAction, AuthError> {
        if response.status == 401 {
            let generation = receipt.generation().ok_or_else(|| {
                AuthError::Other(
                    "Copilot 401 observation received an untracked authorization receipt"
                        .to_string(),
                )
            })?;
            self.invalidate_derived_token_generation(generation).await;
            return Ok(HttpAuthorizationResponseAction::RetryWithFreshAuthorization);
        }
        Ok(HttpAuthorizationResponseAction::Propagate)
    }

    #[allow(clippy::unnecessary_literal_bound)]
    fn label(&self) -> &str {
        crate::GITHUB_COPILOT_AUTHORIZER_LABEL
    }

    fn persistence_authority_id(&self) -> Option<meerkat_core::auth::ProviderAuthPersistenceId> {
        self.env
            .provider_auth_persistence()
            .map(meerkat_core::auth::ProviderAuthPersistence::authority_id)
    }

    fn expires_at(&self) -> Option<DateTime<Utc>> {
        self.state
            .cache
            .read()
            .as_ref()
            .and_then(|cached| cached.source_expires_at)
    }
}

impl CopilotAuthorizer {
    async fn authorize_attempt(
        &self,
        request: &mut HttpAuthorizationRequest<'_>,
    ) -> Result<u64, AuthError> {
        let token = self
            .ensure_token()
            .await
            .map_err(auth_error_from_provider)?;
        if !url_is_within_api_base(request.url, &token.api_base) {
            return Err(AuthError::Other(format!(
                "Copilot authorizer refused URL outside resolved API base: {}",
                request.url
            )));
        }
        for (name, value) in self
            .inference_headers(&token.token, CopilotRequestIntent::ConversationPanel)
            .map_err(|error| AuthError::Other(error.to_string()))?
        {
            let Some(name) = name else {
                continue;
            };
            let value = value
                .to_str()
                .map_err(|error| AuthError::Other(error.to_string()))?;
            request.headers.push((name.to_string(), value.to_string()));
        }
        Ok(token.generation)
    }
}

fn auth_error_from_provider(error: ProviderAuthError) -> AuthError {
    match error {
        ProviderAuthError::Auth(error) => error,
        other => AuthError::RefreshFailed(other.to_string()),
    }
}

fn auth_error_from_refresh(error: RefreshError) -> AuthError {
    match error {
        RefreshError::ReauthRequired(_)
        | RefreshError::Classified {
            disposition: meerkat_core::auth::RefreshFailureDisposition::ReauthRequired,
            ..
        }
        | RefreshError::DurableTerminalCommit {
            disposition: meerkat_core::auth::RefreshFailureDisposition::ReauthRequired,
            ..
        } => AuthError::UserReauthRequired,
        other => AuthError::RefreshFailed(other.to_string()),
    }
}

#[derive(Clone)]
struct CachedTokenView {
    generation: u64,
    token: String,
    api_base: String,
    model_discovery_due: bool,
}

fn resolve_api_base(token: &CopilotTokenEnvelope) -> Result<String, ProviderAuthError> {
    let candidate = token
        .endpoints
        .api
        .clone()
        .or_else(|| api_base_from_proxy_token(&token.token))
        .unwrap_or_else(|| DEFAULT_COPILOT_API_BASE.to_string());
    let url = url::Url::parse(&candidate).map_err(|error| {
        ProviderAuthError::SourceResolutionFailed(format!("invalid Copilot API endpoint: {error}"))
    })?;
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
        return Err(ProviderAuthError::SourceResolutionFailed(
            "Copilot API endpoint must be HTTPS without user information".to_string(),
        ));
    }

    Ok(candidate.trim_end_matches('/').to_string())
}

fn url_is_within_api_base(request_url: &str, api_base: &str) -> bool {
    let (Ok(request), Ok(base)) = (url::Url::parse(request_url), url::Url::parse(api_base)) else {
        return false;
    };
    if request.scheme() != base.scheme()
        || request.host_str() != base.host_str()
        || request.port_or_known_default() != base.port_or_known_default()
        || !request.username().is_empty()
        || request.password().is_some()
    {
        return false;
    }
    let base_path = base.path().trim_end_matches('/');
    base_path.is_empty()
        || request.path() == base_path
        || request
            .path()
            .strip_prefix(base_path)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

fn api_base_from_proxy_token(token: &str) -> Option<String> {
    let proxy = token
        .split(';')
        .find_map(|part| part.strip_prefix("proxy-ep="))?;
    let host = proxy
        .strip_prefix("proxy.")
        .map_or_else(|| proxy.to_string(), |suffix| format!("api.{suffix}"));
    Some(format!("https://{host}"))
}

fn token_exchange_requires_account_action(status: u16, body: &[u8]) -> bool {
    if status == 401 {
        return true;
    }
    if status != 403 {
        return false;
    }
    let envelope = serde_json::from_slice::<CopilotTokenErrorEnvelope>(body).unwrap_or_default();
    if envelope.message.as_deref().is_some_and(|message| {
        message
            .to_ascii_lowercase()
            .contains("api rate limit exceeded")
    }) {
        return false;
    }
    envelope
        .error_details
        .and_then(|details| details.notification_id)
        .is_some()
}

fn token_refresh_at(
    token: &CopilotTokenEnvelope,
    now: DateTime<Utc>,
) -> Result<DateTime<Utc>, ProviderAuthError> {
    let expires_at = i64::try_from(token.expires_at)
        .ok()
        .and_then(|timestamp| DateTime::<Utc>::from_timestamp(timestamp, 0))
        .ok_or_else(|| {
            ProviderAuthError::SourceResolutionFailed(
                "Copilot token expiry is out of range".to_string(),
            )
        })?;
    let lifetime_seconds = expires_at.signed_duration_since(now).num_seconds();
    if lifetime_seconds <= 1 {
        return Err(ProviderAuthError::SourceResolutionFailed(
            "Copilot token expiry is not in the usable future".to_string(),
        ));
    }
    let refresh_seconds = i64::try_from(token.refresh_in).map_err(|_| {
        ProviderAuthError::SourceResolutionFailed(
            "Copilot token refresh interval is out of range".to_string(),
        )
    })?;
    let refresh_from_hint = now
        .checked_add_signed(Duration::seconds(refresh_seconds))
        .ok_or_else(|| {
            ProviderAuthError::SourceResolutionFailed(
                "Copilot token refresh deadline is out of range".to_string(),
            )
        })?;
    let expiry_skew_seconds =
        CAPI_EXPIRY_SKEW_SECONDS.min((lifetime_seconds / 5).clamp(1, CAPI_EXPIRY_SKEW_SECONDS));
    let refresh_before_expiry = expires_at
        .checked_sub_signed(Duration::seconds(expiry_skew_seconds))
        .ok_or_else(|| {
            ProviderAuthError::SourceResolutionFailed(
                "Copilot token expiry refresh deadline is out of range".to_string(),
            )
        })?;
    let minimum_future = now
        .checked_add_signed(Duration::seconds(1))
        .ok_or_else(|| {
            ProviderAuthError::SourceResolutionFailed(
                "Copilot minimum refresh deadline is out of range".to_string(),
            )
        })?;
    Ok(refresh_from_hint
        .min(refresh_before_expiry)
        .max(minimum_future)
        .min(expires_at))
}

fn insert_header(
    headers: &mut HeaderMap,
    name: &'static str,
    value: &str,
) -> Result<(), ProviderAuthError> {
    let name = HeaderName::from_bytes(name.as_bytes()).map_err(|error| {
        ProviderAuthError::SourceResolutionFailed(format!(
            "invalid Copilot header name {name}: {error}"
        ))
    })?;
    let value = HeaderValue::from_str(value).map_err(|error| {
        ProviderAuthError::SourceResolutionFailed(format!("invalid Copilot header {name}: {error}"))
    })?;
    headers.insert(name, value);
    Ok(())
}

async fn refresh_github_tokens(
    persistence: ProviderAuthPersistence,
    prepare: meerkat_auth_core::resolver::ManagedStoreOAuthRefreshPrepareFn,
    credential_identity: AuthCredentialIdentity,
    config: CopilotBackendConfig,
    force_refresh: bool,
) -> Result<PersistedTokens, RefreshError> {
    let preparation = ManagedStoreOAuthRefreshPreparationSlot::new(prepare);
    let token_store = persistence.token_store();
    let key = meerkat_core::auth::TokenKey::from_credential_identity(&credential_identity);
    let refresh_key = key.clone();
    let preparation_for_refresh = preparation.clone();
    let refresh_fn: RefreshFn = Box::new(move || {
        let token_store = token_store.clone();
        let key = refresh_key.clone();
        let preparation = preparation_for_refresh.clone();
        let config = config.clone();
        Box::pin(async move {
            let current = token_store
                .load(&key)
                .await
                .map_err(|error| RefreshError::Refresh(error.to_string()))?
                .ok_or_else(|| {
                    RefreshError::Refresh(
                        "Copilot source credential disappeared before refresh".to_string(),
                    )
                })?;
            match preparation.claim_refresh_owner(current.clone()).await? {
                LockedManagedStoreOAuthRefresh::UseCached(cached) => Ok(cached),
                LockedManagedStoreOAuthRefresh::Refresh(transaction) => {
                    let refresh_token = match current.refresh_token.clone() {
                        Some(refresh_token) => refresh_token,
                        None => {
                            return Err(transaction.fail(RefreshError::Observed {
                            message: "GitHub credential has no refresh token".to_string(),
                            observation:
                                meerkat_core::RefreshFailureObservation::local_credential_unusable(),
                            }));
                        }
                    };
                    let mut endpoints = meerkat_auth_core::oauth_flow::oauth_provider_endpoints(
                        meerkat_core::OAuthProviderIdentity::GitHubCopilot,
                        "",
                    );
                    let configured = match config.endpoints() {
                        Ok(configured) => configured,
                        Err(error) => {
                            return Err(transaction.fail(RefreshError::Refresh(error.to_string())));
                        }
                    };
                    endpoints.token_url = configured.oauth_token_url.to_string();
                    let result = match exchange_refresh_token(
                        &reqwest::Client::new(),
                        &endpoints,
                        &refresh_token,
                        None,
                    )
                    .await
                    {
                        Ok(result) => result,
                        Err(error) => {
                            return Err(transaction.fail(oauth_refresh_error(error)));
                        }
                    };
                    let now = Utc::now();
                    let expires_at = match result.expires_at_from(now) {
                        Ok(expires_at) => expires_at,
                        Err(error) => {
                            return Err(transaction.fail(RefreshError::Refresh(error.to_string())));
                        }
                    };
                    let refreshed = PersistedTokens {
                        auth_mode: PersistedAuthMode::GithubCopilotOauth,
                        primary_secret: Some(result.access_token),
                        refresh_token: result.refresh_token.or(Some(refresh_token)),
                        id_token: result.id_token,
                        expires_at,
                        last_refresh: Some(now),
                        scopes: result
                            .scope
                            .as_deref()
                            .map(|scope| scope.split_whitespace().map(str::to_string).collect())
                            .unwrap_or_default(),
                        account_id: current.account_id,
                        metadata: serde_json::Value::Null,
                    };
                    transaction.commit(refreshed).await
                }
            }
        })
    });
    let refreshed = if force_refresh {
        persistence
            .refresh_coordinator()
            .with_forced_refresh(key.clone(), refresh_fn)
            .await
    } else {
        persistence
            .refresh_coordinator()
            .with_refresh(key.clone(), refresh_fn)
            .await
    }?;
    preparation
        .finish_coordinated_refresh(
            persistence.refresh_coordinator(),
            persistence.token_store(),
            key,
            refreshed,
        )
        .await
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use meerkat_core::auth::TokenStore;
    use meerkat_core::provider_matrix::openai::{OpenAiAuthMethod, OpenAiBackendKind};
    use meerkat_core::{
        AuthBindingRef, AuthProfile, BackendProfile, BindingId, BindingOrigin, BindingPolicy,
        CredentialAccountId, CredentialAccountRef, CredentialSourceSpec, RealmId,
    };
    use meerkat_llm_core::provider_runtime::ProviderRuntimeCatalog;
    use std::collections::VecDeque;

    fn account_identity() -> AuthCredentialIdentity {
        AuthCredentialIdentity::Account(CredentialAccountRef {
            realm: RealmId::global(),
            account: CredentialAccountId::parse("github_copilot").expect("valid account id"),
        })
    }

    fn persistence_id() -> meerkat_core::auth::ProviderAuthPersistenceId {
        ProviderAuthPersistence::new(
            Arc::new(meerkat_auth_core::auth_store::EphemeralTokenStore::new()),
            Arc::new(meerkat_auth_core::auth_store::InMemoryCoordinator::new()),
        )
        .authority_id()
    }

    struct ScriptedTransport {
        responses: Mutex<VecDeque<CopilotHttpResponse>>,
        requests: Mutex<Vec<(String, HeaderMap)>>,
    }

    struct NoopLlmClient;

    #[async_trait]
    impl meerkat_llm_core::LlmClient for NoopLlmClient {
        fn project_replay_messages(
            &self,
            messages: &[meerkat_core::Message],
        ) -> Result<Vec<meerkat_core::Message>, meerkat_llm_core::LlmError> {
            Ok(messages.to_vec())
        }

        fn stream<'a>(
            &'a self,
            _request: &'a meerkat_llm_core::LlmRequest,
        ) -> meerkat_llm_core::LlmStream<'a> {
            Box::pin(futures::stream::empty())
        }

        fn provider(&self) -> Provider {
            Provider::OpenAI
        }

        async fn health_check(&self) -> Result<(), meerkat_llm_core::LlmError> {
            Ok(())
        }
    }

    impl ScriptedTransport {
        fn new(responses: impl IntoIterator<Item = CopilotHttpResponse>) -> Self {
            Self {
                responses: Mutex::new(responses.into_iter().collect()),
                requests: Mutex::new(Vec::new()),
            }
        }

        fn request_count(&self) -> usize {
            self.requests.lock().len()
        }
    }

    #[async_trait]
    impl CopilotHttpTransport for ScriptedTransport {
        async fn get(
            &self,
            url: &str,
            headers: HeaderMap,
        ) -> Result<CopilotHttpResponse, CopilotHttpTransportError> {
            self.requests.lock().push((url.to_string(), headers));
            self.responses.lock().pop_front().ok_or_else(|| {
                CopilotHttpTransportError::Scripted("unexpected Copilot HTTP request".to_string())
            })
        }
    }

    fn json_response(status: u16, body: serde_json::Value) -> CopilotHttpResponse {
        CopilotHttpResponse {
            status,
            body: serde_json::to_vec(&body).expect("serialize scripted response"),
        }
    }

    fn token_response(token: &str) -> CopilotHttpResponse {
        json_response(
            200,
            serde_json::json!({
                "token": token,
                "expires_at": Utc::now().timestamp() + 3600,
                "refresh_in": 1800,
                "endpoints": {"api": "https://copilot.example.test"}
            }),
        )
    }

    fn models_response() -> CopilotHttpResponse {
        json_response(
            200,
            serde_json::json!({
                "data": [
                    {
                        "id": "gpt-test",
                        "supported_endpoints": ["/responses", "/chat/completions"]
                    },
                    {
                        "id": "claude-test",
                        "supported_endpoints": ["/v1/messages"]
                    },
                    {
                        "id": "gemini-test",
                        "supported_endpoints": ["/chat/completions"]
                    }
                ]
            }),
        )
    }

    fn validated_binding() -> (AuthBindingRef, ValidatedBinding) {
        let auth_binding = AuthBindingRef {
            realm: RealmId::global(),
            binding: BindingId::parse("copilot_openai").expect("valid binding"),
            profile: None,
            origin: BindingOrigin::Configured,
        };
        let backend = BackendProfile {
            id: "copilot".to_string(),
            provider: Provider::OpenAI,
            backend_kind: OpenAiBackendKind::Copilot.as_str().to_string(),
            base_url: None,
            options: serde_json::Value::Null,
            server: None,
        };
        let auth = AuthProfile {
            id: "copilot".to_string(),
            provider: Provider::OpenAI,
            auth_method: OpenAiAuthMethod::GitHubCopilotOauth.as_str().to_string(),
            source: CredentialSourceSpec::ManagedStore,
            constraints: Default::default(),
            metadata_defaults: Default::default(),
        };
        let validated = ProviderRuntimeCatalog::validate_binding(
            &auth_binding,
            &backend,
            &auth,
            &BindingPolicy::default(),
        )
        .expect("valid Copilot binding");
        (auth_binding, validated)
    }

    async fn resolver_environment(
        auth_binding: &AuthBindingRef,
    ) -> (
        ResolverEnvironment,
        meerkat_core::handles::GeneratedAuthLeaseHandle,
    ) {
        resolver_environment_with_token(auth_binding, "github-source-token").await
    }

    async fn resolver_environment_with_token(
        auth_binding: &AuthBindingRef,
        github_token: &str,
    ) -> (
        ResolverEnvironment,
        meerkat_core::handles::GeneratedAuthLeaseHandle,
    ) {
        let machine = meerkat_runtime::MeerkatMachine::ephemeral();
        let auth_lease = machine.generated_auth_lease_handle();
        let store = Arc::new(meerkat_auth_core::auth_store::EphemeralTokenStore::new());
        let persistence = ProviderAuthPersistence::new(
            store.clone(),
            Arc::new(meerkat_auth_core::auth_store::InMemoryCoordinator::new()),
        );
        let identity = AuthCredentialIdentity::from_auth_binding(auth_binding);
        let tokens = PersistedTokens {
            auth_mode: PersistedAuthMode::GithubCopilotOauth,
            primary_secret: Some(github_token.to_string()),
            refresh_token: None,
            id_token: None,
            expires_at: None,
            last_refresh: Some(Utc::now()),
            scopes: vec!["read:user".to_string()],
            account_id: None,
            metadata: serde_json::Value::Null,
        };
        let transition = meerkat_core::publish_token_lifecycle_acquired_for_identity(
            &auth_lease,
            &identity,
            &tokens,
        )
        .expect("publish source lifecycle");
        let marked = meerkat_core::mark_tokens_lifecycle_published_for_transition(
            &meerkat_core::auth::TokenKey::from_credential_identity(&identity),
            &tokens,
            &transition,
        )
        .expect("mark source token");
        store
            .save(
                &meerkat_core::auth::TokenKey::from_credential_identity(&identity),
                &marked,
            )
            .await
            .expect("save source token");
        (
            ResolverEnvironment::testing()
                .with_provider_auth_persistence(persistence)
                .with_auth_lease_handle(auth_lease.clone()),
            auth_lease,
        )
    }

    #[test]
    fn authorization_scope_requires_exact_origin_and_path_boundary() {
        assert!(url_is_within_api_base(
            "https://api.githubcopilot.com/chat/completions",
            "https://api.githubcopilot.com"
        ));
        assert!(url_is_within_api_base(
            "https://example.test/copilot/responses",
            "https://example.test/copilot"
        ));
        assert!(!url_is_within_api_base(
            "https://api.githubcopilot.com.evil.test/chat/completions",
            "https://api.githubcopilot.com"
        ));
        assert!(!url_is_within_api_base(
            "https://example.test/copilot-evil/responses",
            "https://example.test/copilot"
        ));
        assert!(!url_is_within_api_base(
            "https://example.test:444/copilot/responses",
            "https://example.test/copilot"
        ));
    }

    #[test]
    fn proxy_endpoint_is_derived_without_leaking_token_material() {
        assert_eq!(
            api_base_from_proxy_token("tid=abc;proxy-ep=proxy.individual.githubcopilot.com;ol=1"),
            Some("https://api.individual.githubcopilot.com".to_string())
        );
        assert_eq!(
            api_base_from_proxy_token("proxy-ep=enterprise.example.test"),
            Some("https://enterprise.example.test".to_string())
        );
    }

    #[test]
    fn refresh_hint_never_outlives_expiry_skew() {
        let now = DateTime::from_timestamp(1_800_000_000, 0).expect("valid timestamp");
        let token = CopilotTokenEnvelope {
            token: "derived".to_string(),
            expires_at: 1_800_003_600,
            refresh_in: 3_500,
            endpoints: Default::default(),
            sku: None,
            individual: None,
        };
        assert_eq!(
            token_refresh_at(&token, now).expect("refresh instant"),
            DateTime::from_timestamp(1_800_003_300, 0).expect("valid timestamp")
        );
    }

    #[test]
    fn short_lived_token_refresh_deadline_remains_in_the_future() {
        let now = DateTime::from_timestamp(1_800_000_000, 0).expect("valid timestamp");
        let token = CopilotTokenEnvelope {
            token: "derived".to_string(),
            expires_at: 1_800_000_300,
            refresh_in: 300,
            endpoints: Default::default(),
            sku: None,
            individual: None,
        };

        assert_eq!(
            token_refresh_at(&token, now).expect("refresh instant"),
            DateTime::from_timestamp(1_800_000_240, 0).expect("valid timestamp")
        );
    }

    #[test]
    fn hostile_refresh_interval_fails_without_datetime_overflow() {
        let now = DateTime::from_timestamp(1_800_000_000, 0).expect("valid timestamp");
        let token = CopilotTokenEnvelope {
            token: "derived".to_string(),
            expires_at: 1_800_003_600,
            refresh_in: u64::MAX,
            endpoints: Default::default(),
            sku: None,
            individual: None,
        };

        assert!(token_refresh_at(&token, now).is_err());
    }

    #[test]
    fn terminal_auth_failures_preserve_reauth_classification() {
        assert_eq!(
            auth_error_from_provider(ProviderAuthError::Auth(AuthError::UserReauthRequired)),
            AuthError::UserReauthRequired
        );
        assert_eq!(
            auth_error_from_refresh(RefreshError::ReauthRequired("invalid grant".to_string())),
            AuthError::UserReauthRequired
        );
    }

    #[test]
    fn routes_with_one_account_share_derived_state() {
        let runtime = CopilotRuntime::new();
        let identity = account_identity();
        let persistence_id = persistence_id();
        let first = runtime
            .account_state(persistence_id, &identity, CopilotBackendConfig::default())
            .expect("first route");
        let second = runtime
            .account_state(persistence_id, &identity, CopilotBackendConfig::default())
            .expect("second route");
        assert!(Arc::ptr_eq(&first, &second));
    }

    #[test]
    fn equal_account_keys_from_distinct_persistence_authorities_do_not_share_state() {
        let runtime = CopilotRuntime::new();
        let identity = account_identity();
        let first = runtime
            .account_state(persistence_id(), &identity, CopilotBackendConfig::default())
            .expect("first persistence authority");
        let second = runtime
            .account_state(persistence_id(), &identity, CopilotBackendConfig::default())
            .expect("second persistence authority");

        assert!(!Arc::ptr_eq(&first, &second));
    }

    #[tokio::test]
    async fn routed_client_rebuilds_from_current_api_base() {
        let runtime = Arc::new(CopilotRuntime::new());
        let (auth_binding, binding) = validated_binding();
        let (env, _) = resolver_environment(&auth_binding).await;
        let persistence_id = env
            .provider_auth_persistence()
            .expect("test persistence")
            .authority_id();
        let state = runtime
            .account_state(
                persistence_id,
                binding.credential_identity(),
                CopilotBackendConfig::default(),
            )
            .expect("account state");
        let snapshot = CopilotModelSnapshot::from_models(vec![crate::CopilotModel {
            id: "gpt-test".to_string(),
            vendor: None,
            name: None,
            version: None,
            model_picker_enabled: None,
            capabilities: crate::CopilotModelCapabilities::default(),
            policy: None,
            supported_endpoints: vec![CopilotEndpoint::Responses],
        }])
        .expect("model snapshot");
        *state.cache.write() = Some(CachedCopilotToken {
            generation: 1,
            token: "derived".to_string(),
            source_generation: 1,
            source_expires_at: None,
            refresh_at: Utc::now() + Duration::hours(1),
            api_base: "https://first.example.test".to_string(),
            models: Some(snapshot),
            model_discovery_retry_at: None,
        });
        let authorizer = Arc::new(CopilotAuthorizer {
            state: Arc::clone(&state),
            binding: binding.clone(),
            env,
            transport: Arc::new(ScriptedTransport::new([])),
        });
        let connection = meerkat_llm_core::provider_runtime::ResolvedConnection {
            provider: Provider::OpenAI,
            backend: binding.backend(),
            backend_profile: binding.backend_profile().clone(),
            credential_identity: binding.credential_identity().clone(),
            auth_lease: Arc::new(
                meerkat_llm_core::provider_runtime::DynamicLease::from_authorizer(
                    authorizer,
                    AuthMetadata::default(),
                    crate::GITHUB_COPILOT_AUTHORIZER_LABEL,
                ),
            ),
        };
        runtime.accounts.lock().insert(
            CopilotAccountCacheKey {
                persistence_id,
                credential_identity: binding.credential_identity().clone(),
            },
            Arc::downgrade(&state),
        );
        let observed = Arc::new(Mutex::new(Vec::new()));
        let observed_for_factory = Arc::clone(&observed);
        let factory: CopilotRouteClientFactory = Arc::new(move |route, _| {
            observed_for_factory.lock().push(route.api_base.clone());
            Ok(Arc::new(NoopLlmClient))
        });
        let client = routed_client(
            Arc::clone(&runtime),
            connection,
            Provider::OpenAI,
            "gpt-test".to_string(),
            factory,
        )
        .expect("routed client");

        state.cache.write().as_mut().expect("cache").api_base =
            "https://second.example.test".to_string();
        client
            .request_pressure(&meerkat_llm_core::LlmRequest::new("gpt-test", Vec::new()))
            .expect("route refresh");

        assert_eq!(
            observed.lock().as_slice(),
            ["https://first.example.test", "https://second.example.test"]
        );
    }

    #[test]
    fn account_capability_gate_rejects_excess_output_budget() {
        let client = capability_gated_client(
            Arc::new(NoopLlmClient),
            Some(crate::CopilotModelCapabilities {
                limits: crate::CopilotModelLimits {
                    max_output_tokens: Some(16),
                    ..Default::default()
                },
                supports: Default::default(),
            }),
        );
        let request = meerkat_llm_core::LlmRequest::new("gpt-test", Vec::new()).with_max_tokens(17);

        assert!(matches!(
            client.request_pressure(&request),
            Err(meerkat_llm_core::LlmError::InvalidRequest { .. })
        ));
    }

    #[test]
    fn shared_account_rejects_conflicting_backend_options() {
        let runtime = CopilotRuntime::new();
        let identity = account_identity();
        let persistence_id = persistence_id();
        let _first = runtime
            .account_state(persistence_id, &identity, CopilotBackendConfig::default())
            .expect("first route");
        let conflicting = CopilotBackendConfig {
            integration_id: "different-client".to_string(),
            ..CopilotBackendConfig::default()
        };
        assert!(
            runtime
                .account_state(persistence_id, &identity, conflicting)
                .is_err()
        );
    }

    #[test]
    fn token_exchange_403_distinguishes_entitlement_from_rate_limit() {
        assert!(token_exchange_requires_account_action(
            403,
            br#"{"error_details":{"notification_id":"no_copilot_access"}}"#
        ));
        assert!(!token_exchange_requires_account_action(
            403,
            br#"{"message":"API rate limit exceeded for user"}"#
        ));
        assert!(token_exchange_requires_account_action(401, b""));
        assert!(!token_exchange_requires_account_action(500, b""));
    }

    #[tokio::test]
    async fn mint_discovery_cache_and_derived_401_preserve_source_lifecycle() {
        let (auth_binding, binding) = validated_binding();
        let (env, auth_lease) = resolver_environment(&auth_binding).await;
        let transport = Arc::new(ScriptedTransport::new([
            token_response("derived-one"),
            models_response(),
            token_response("derived-two"),
            models_response(),
        ]));
        let authorizer = CopilotAuthorizer {
            state: Arc::new(CopilotAccountState {
                config: CopilotBackendConfig::default(),
                cache: RwLock::new(None),
                refresh_lock: tokio::sync::Mutex::new(()),
                next_generation: AtomicU64::new(1),
            }),
            binding,
            env,
            transport: transport.clone(),
        };

        authorizer
            .prime()
            .await
            .expect("initial mint and discovery");
        assert_eq!(transport.request_count(), 2);
        assert!(matches!(
            authorizer
                .model_snapshot()
                .and_then(|snapshot| snapshot.model("gpt-test").cloned())
                .and_then(|model| model.route_for(Provider::OpenAI)),
            Some(CopilotEndpoint::Responses)
        ));

        let mut headers = Vec::new();
        let receipt = authorizer
            .authorize_with_receipt(&mut HttpAuthorizationRequest {
                method: "POST",
                url: "https://copilot.example.test/responses",
                headers: &mut headers,
            })
            .await
            .expect("cached authorization");
        assert_eq!(transport.request_count(), 2);
        assert!(headers.iter().any(|(name, value)| {
            name.eq_ignore_ascii_case("authorization") && value == "Bearer derived-one"
        }));
        assert!(headers.iter().any(|(name, value)| {
            name.eq_ignore_ascii_case("openai-intent") && value == "conversation-panel"
        }));
        assert!(
            headers
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case("x-request-id"))
        );

        assert_eq!(
            authorizer
                .observe_response_with_receipt(
                    receipt,
                    &HttpAuthorizationResponse {
                        method: "POST",
                        url: "https://copilot.example.test/responses",
                        status: 401,
                    },
                )
                .await
                .expect("401 observation"),
            HttpAuthorizationResponseAction::RetryWithFreshAuthorization
        );
        let mut refreshed_headers = Vec::new();
        let refreshed_receipt = authorizer
            .authorize_with_receipt(&mut HttpAuthorizationRequest {
                method: "POST",
                url: "https://copilot.example.test/responses",
                headers: &mut refreshed_headers,
            })
            .await
            .expect("remint derived token");
        assert_eq!(transport.request_count(), 4);
        assert!(refreshed_headers.iter().any(|(name, value)| {
            name.eq_ignore_ascii_case("authorization") && value == "Bearer derived-two"
        }));
        assert_ne!(receipt, refreshed_receipt);
        authorizer
            .observe_response_with_receipt(
                receipt,
                &HttpAuthorizationResponse {
                    method: "POST",
                    url: "https://copilot.example.test/responses",
                    status: 401,
                },
            )
            .await
            .expect("delayed stale 401 observation");
        let mut still_fresh_headers = Vec::new();
        authorizer
            .authorize_with_receipt(&mut HttpAuthorizationRequest {
                method: "POST",
                url: "https://copilot.example.test/responses",
                headers: &mut still_fresh_headers,
            })
            .await
            .expect("newer derived token survives stale 401");
        assert_eq!(transport.request_count(), 4);
        assert_eq!(
            auth_lease
                .snapshot(&meerkat_core::handles::LeaseKey::from_auth_binding(
                    &auth_binding
                ))
                .phase,
            Some(meerkat_core::handles::AuthLeasePhase::Valid)
        );

        let requests = transport.requests.lock();
        let (_, mint_headers) = &requests[0];
        assert_eq!(
            mint_headers
                .get("authorization")
                .and_then(|value| value.to_str().ok()),
            Some("token github-source-token")
        );
        assert!(mint_headers.contains_key("editor-version"));
        assert!(mint_headers.contains_key("copilot-integration-id"));
        let (_, model_headers) = &requests[1];
        assert_eq!(
            model_headers
                .get("openai-intent")
                .and_then(|value| value.to_str().ok()),
            Some("model-access")
        );
    }

    #[tokio::test]
    async fn failed_model_discovery_retries_without_reminting_derived_token() {
        let (auth_binding, binding) = validated_binding();
        let (env, _) = resolver_environment(&auth_binding).await;
        let transport = Arc::new(ScriptedTransport::new([
            token_response("derived"),
            json_response(503, serde_json::json!({"message": "temporary"})),
            models_response(),
        ]));
        let state = Arc::new(CopilotAccountState {
            config: CopilotBackendConfig::default(),
            cache: RwLock::new(None),
            refresh_lock: tokio::sync::Mutex::new(()),
            next_generation: AtomicU64::new(1),
        });
        let authorizer = CopilotAuthorizer {
            state: Arc::clone(&state),
            binding,
            env,
            transport: transport.clone(),
        };
        authorizer
            .prime()
            .await
            .expect("token mint survives discovery failure");
        assert_eq!(transport.request_count(), 2);
        state
            .cache
            .write()
            .as_mut()
            .expect("derived cache")
            .model_discovery_retry_at = Some(Utc::now() - Duration::seconds(1));

        let mut headers = Vec::new();
        authorizer
            .authorize(&mut HttpAuthorizationRequest {
                method: "POST",
                url: "https://copilot.example.test/responses",
                headers: &mut headers,
            })
            .await
            .expect("discovery retry succeeds");

        assert_eq!(
            transport.request_count(),
            3,
            "discovery retry must reuse the derived token"
        );
        assert!(
            authorizer
                .model_snapshot()
                .is_some_and(|snapshot| snapshot.model("gpt-test").is_some())
        );
    }

    #[tokio::test]
    #[ignore = "requires a live COPILOT_GITHUB_TOKEN with Copilot entitlement"]
    async fn live_token_exchange_and_model_discovery() {
        let github_token =
            std::env::var("COPILOT_GITHUB_TOKEN").expect("COPILOT_GITHUB_TOKEN is required");
        let (auth_binding, binding) = validated_binding();
        let (env, _) = resolver_environment_with_token(&auth_binding, &github_token).await;
        let authorizer = CopilotAuthorizer {
            state: Arc::new(CopilotAccountState {
                config: CopilotBackendConfig::default(),
                cache: RwLock::new(None),
                refresh_lock: tokio::sync::Mutex::new(()),
                next_generation: AtomicU64::new(1),
            }),
            binding,
            env,
            transport: Arc::new(ReqwestCopilotHttpTransport::default()),
        };
        authorizer
            .prime()
            .await
            .expect("live Copilot token exchange");
        assert!(
            authorizer
                .model_snapshot()
                .is_some_and(|snapshot| snapshot.models().next().is_some()),
            "live Copilot model discovery returned no usable model snapshot"
        );
    }
}
