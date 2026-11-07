use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration as StdDuration;

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use futures::StreamExt;
use meerkat_auth_core::auth_oauth::{exchange_refresh_token, oauth_refresh_error};
use meerkat_auth_core::resolver::{
    LockedManagedStoreOAuthRefresh, ManagedStoreLifecycle, ManagedStoreOAuthRefreshPreparationSlot,
    OAuthLoginCredentialAdmission, load_managed_store_tokens_with_lifecycle,
    prepare_managed_store_oauth_refresh_under_lock, resolve_oauth_login_credential_disposition,
    resolve_simple_secret, simple_secret_source_publishes_lifecycle,
};
use meerkat_core::auth::{
    PersistedAuthMode, PersistedTokens, ProviderAuthPersistence, RefreshError, RefreshFn,
    TokenStoreAccess,
};
use meerkat_core::handles::{
    AUTH_LEASE_TTL_REFRESH_WINDOW_SECS, CredentialFingerprint, CredentialUseDisposition,
    CredentialUseIntent, DslTransitionError, GeneratedAuthLeaseHandle, LeaseKey,
};
use meerkat_core::{
    AuthCredentialIdentity, AuthError, AuthMetadata, CredentialSourceSpec,
    HttpAuthorizationReceipt, HttpAuthorizationRequest, HttpAuthorizationResponse,
    HttpAuthorizationResponseAction, HttpAuthorizer, Provider,
};
use meerkat_llm_core::provider_runtime::{
    ProviderAuthError, ProviderClientError, ResolverEnvironment, ValidatedBinding,
};
use parking_lot::{Mutex, RwLock};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};

use crate::protocol::{
    CopilotAccountIdentity, CopilotBackendConfig, CopilotCredentialMode, CopilotEndpoint,
    CopilotModelSnapshot, CopilotModelsEnvelope, CopilotTokenEnvelope, CopilotTokenErrorEnvelope,
    CopilotUserEnvelope, DEFAULT_COPILOT_API_BASE, GitHubErrorEnvelope, GitHubTokenKind,
    copilot_user_url,
};

const CAPI_EXPIRY_SKEW_SECONDS: i64 = 300;
const MODEL_DISCOVERY_RETRY_SECONDS: u64 = 30;
const MODEL_DISCOVERY_REFRESH_SECONDS: u64 = 300;
const MODEL_DISCOVERY_MAX_RETRY_SECONDS: u64 = 3600;
/// How long a model discovery a request started off the request path holds
/// the account's next one off: the listing's own HTTP bound with a margin.
/// The discovery sets the next deadline when it ends, so one that never ends
/// (its runtime stopped) is tried again after this.
const MODEL_DISCOVERY_CLAIM_SECONDS: u64 = 60;
/// After a token exchange that failed transiently while the current token
/// still serves, the next attempt waits this long unless the exchange sent
/// its own `Retry-After`.
const TOKEN_EXCHANGE_RETRY_SECONDS: u64 = 30;
/// The shortest wait before that next attempt, whatever `Retry-After` says.
const TOKEN_EXCHANGE_MIN_RETRY_SECONDS: u64 = 1;
/// The longest an exchange's `Retry-After` holds the next attempt off once no
/// token is left to serve.
const TOKEN_EXCHANGE_MAX_RETRY_SECONDS: u64 = 3600;
const BEARER_SCHEME: &str = "Bearer";
const COPILOT_HTTP_TIMEOUT: StdDuration = StdDuration::from_secs(30);
const MAX_COPILOT_HTTP_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
/// The pause after GitHub rate limits an operator-token request (the
/// identity lookup or the model listing): GitHub's stated time, at least a
/// minute (GitHub's rule for a limit that states no wait), at most an hour.
const GITHUB_RATE_LIMIT_MIN_SECONDS: u64 = 60;
const GITHUB_RATE_LIMIT_MAX_SECONDS: u64 = 3600;
/// After the lease withdrew the operator token (GitHub refused it, or a
/// logout released it), a source that runs something to give the token (a
/// command, a host resolver) is read again at most this often while that
/// stands.
const WITHDRAWN_TOKEN_SOURCE_REREAD_SECONDS: u64 = 60;
/// GitHub's error `message` is quoted in errors with control characters
/// removed and at most this many characters.
const GITHUB_ERROR_MESSAGE_MAX_CHARS: usize = 200;
/// How many of the bearer values an account state signed requests with it
/// keeps to redact from response diagnostics: the current one and those that
/// replaced each other before it, whose requests can still be answered.
const REDACTED_BEARER_HISTORY: usize = 4;
/// The operator token goes to no Copilot API host outside this domain.
const COPILOT_API_DOMAIN: &str = "githubcopilot.com";
const COPILOT_API_HOST_SUFFIX: &str = ".githubcopilot.com";

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
    retry_after: Option<StdDuration>,
    /// Whether GitHub rate limited the request ([`github_rate_limited`]).
    rate_limited: bool,
    /// `x-ratelimit-reset`: the epoch second the primary rate limit resets.
    rate_limit_reset: Option<u64>,
}

#[derive(Debug, thiserror::Error)]
enum CopilotHttpTransportError {
    #[error("Copilot HTTP request failed")]
    Request(#[source] reqwest::Error),
    #[error("Copilot HTTP request timed out")]
    Timeout,
    #[error("Copilot HTTP response exceeded {MAX_COPILOT_HTTP_RESPONSE_BYTES} bytes")]
    ResponseTooLarge,
    /// The endpoint answered with a redirect, refused by its status alone
    /// (no `Location`, header or body is read).
    #[error(
        "Copilot HTTP endpoint answered with a redirect (status {status}); redirects are refused"
    )]
    RedirectRefused { status: u16 },
    #[error(transparent)]
    ClientUnavailable(meerkat_auth_core::auth_oauth::CredentialHttpClientUnavailable),
    #[cfg(test)]
    #[error("{0}")]
    Scripted(String),
}

#[derive(Debug, thiserror::Error)]
enum CopilotModelDiscoveryError {
    #[error("Copilot model discovery transport failed")]
    Transport(#[source] CopilotHttpTransportError),
    /// An HTTP failure, carried as its status and GitHub's bounded error
    /// message ([`github_error_message`]), never the raw body: a proxy or
    /// gateway page can echo the request's headers, the bearer included.
    #[error("Copilot model discovery returned HTTP {status}: {message}")]
    Http {
        status: u16,
        message: String,
        retry_after: Option<StdDuration>,
        /// Whether GitHub rate limited the listing ([`github_rate_limited`]).
        rate_limited: bool,
        /// `x-ratelimit-reset`: the epoch second the primary limit resets.
        rate_limit_reset: Option<u64>,
        /// Whether the answer asks for an action on the account
        /// ([`github_account_action`]).
        account_action: bool,
    },
    #[error("invalid Copilot model-discovery request headers")]
    Header(#[source] ProviderAuthError),
    #[error("invalid Copilot model-discovery response")]
    Decode(#[source] serde_json::Error),
    #[error("invalid Copilot model-discovery protocol response")]
    Protocol(#[source] crate::protocol::CopilotProtocolError),
}

impl CopilotModelDiscoveryError {
    fn status(&self) -> Option<u16> {
        match self {
            Self::Http { status, .. } => Some(*status),
            Self::Transport(_) | Self::Header(_) | Self::Decode(_) | Self::Protocol(_) => None,
        }
    }

    /// Whether this listing answer refuses a device sign-in's derived token:
    /// a 401, or a 403 that asks for an account action. Any other answer, a
    /// 403 for a policy or a rate limit included, is a discovery failure the
    /// account retries later.
    fn refuses_derived_token(&self) -> bool {
        match self {
            Self::Http { status: 401, .. } => true,
            Self::Http {
                status: 403,
                account_action,
                ..
            } => *account_action,
            Self::Http { .. }
            | Self::Transport(_)
            | Self::Header(_)
            | Self::Decode(_)
            | Self::Protocol(_) => false,
        }
    }

    fn retry_delay(&self) -> StdDuration {
        match self {
            Self::Http {
                retry_after: Some(delay),
                ..
            } => (*delay).min(StdDuration::from_secs(MODEL_DISCOVERY_MAX_RETRY_SECONDS)),
            Self::Transport(_)
            | Self::Header(_)
            | Self::Decode(_)
            | Self::Protocol(_)
            | Self::Http { .. } => StdDuration::from_secs(MODEL_DISCOVERY_RETRY_SECONDS),
        }
    }
}

/// Why one token exchange minted no token.
enum CopilotTokenExchangeError {
    /// The exchange may answer a later attempt.
    Unavailable(CopilotExchangeUnavailable),
    /// Every other failure: an account action, or an answer that is not a
    /// usable token.
    Failed(ProviderAuthError),
}

impl From<ProviderAuthError> for CopilotTokenExchangeError {
    fn from(error: ProviderAuthError) -> Self {
        Self::Failed(error)
    }
}

impl CopilotTokenExchangeError {
    /// A non-2xx answer that asks for no account action. HTTP 408, 429, a
    /// 403 (a GitHub API rate limit) and a 5xx may pass; any other status
    /// will not.
    fn from_status(status: u16, retry_after: Option<StdDuration>, message: String) -> Self {
        let retryable = match status {
            403 | 429 => meerkat_llm_core::LlmError::RateLimited {
                retry_after_ms: retry_after.map(|delay| delay.as_millis() as u64),
            },
            408 => exchange_timeout(),
            500..=599 => meerkat_llm_core::LlmError::ServerError {
                status,
                message: message.clone(),
            },
            _ => return Self::Failed(ProviderAuthError::SourceResolutionFailed(message)),
        };
        Self::Unavailable(CopilotExchangeUnavailable {
            message,
            retry_after,
            retryable,
            not_before: None,
        })
    }

    /// An exchange that got no answer. A network failure or a timeout may
    /// pass; a refused redirect, an oversized answer or a missing HTTP
    /// client will not.
    fn from_transport(error: &CopilotHttpTransportError) -> Self {
        let message = format!("Copilot token exchange failed: {error}");
        let retryable = match error {
            CopilotHttpTransportError::Timeout => exchange_timeout(),
            CopilotHttpTransportError::Request(request) if request.is_timeout() => {
                exchange_timeout()
            }
            CopilotHttpTransportError::Request(request) if request.is_connect() => {
                meerkat_llm_core::LlmError::ConnectionReset
            }
            CopilotHttpTransportError::Request(_) => meerkat_llm_core::LlmError::Unknown {
                message: message.clone(),
            },
            #[cfg(test)]
            CopilotHttpTransportError::Scripted(_) => meerkat_llm_core::LlmError::Unknown {
                message: message.clone(),
            },
            CopilotHttpTransportError::ResponseTooLarge
            | CopilotHttpTransportError::RedirectRefused { .. }
            | CopilotHttpTransportError::ClientUnavailable(_) => {
                return Self::Failed(ProviderAuthError::SourceResolutionFailed(message));
            }
        };
        Self::Unavailable(CopilotExchangeUnavailable {
            message,
            retry_after: None,
            retryable,
            not_before: None,
        })
    }
}

/// The class of a token exchange that timed out: on its request bound, or
/// with HTTP 408 from the endpoint.
fn exchange_timeout() -> meerkat_llm_core::LlmError {
    meerkat_llm_core::LlmError::NetworkTimeout {
        duration_ms: COPILOT_HTTP_TIMEOUT.as_millis() as u64,
    }
}

/// A token exchange that may succeed on a later attempt: a network failure
/// or timeout, HTTP 408 or 429, a 5xx, or a 403 that asks for no account
/// action (a GitHub API rate limit).
#[derive(Debug, Clone)]
struct CopilotExchangeUnavailable {
    /// The failure as the exchange reported it.
    message: String,
    /// The exchange's own `Retry-After`, when it sent one.
    retry_after: Option<StdDuration>,
    /// The retryable class a turn fails with once no valid token is left.
    retryable: meerkat_llm_core::LlmError,
    /// Once recorded with no token left to serve: the instant before which
    /// the outage answers in place of the exchange, when the exchange sent
    /// `Retry-After` ([`exchange_not_before`]).
    not_before: Option<DateTime<Utc>>,
}

#[async_trait]
trait CopilotHttpTransport: Send + Sync {
    async fn get(
        &self,
        url: &str,
        headers: HeaderMap,
    ) -> Result<CopilotHttpResponse, CopilotHttpTransportError>;
}

/// Credential-bearing Copilot requests follow no redirects. A client build
/// failure is kept and every request fails with it.
struct ReqwestCopilotHttpTransport {
    client: Result<reqwest::Client, meerkat_auth_core::auth_oauth::CredentialHttpClientUnavailable>,
}

impl ReqwestCopilotHttpTransport {
    fn new() -> Self {
        Self {
            client: meerkat_auth_core::auth_oauth::credential_http_client(),
        }
    }
}

#[async_trait]
impl CopilotHttpTransport for ReqwestCopilotHttpTransport {
    async fn get(
        &self,
        url: &str,
        headers: HeaderMap,
    ) -> Result<CopilotHttpResponse, CopilotHttpTransportError> {
        tokio::time::timeout(COPILOT_HTTP_TIMEOUT, async {
            let response = self
                .client
                .as_ref()
                .map_err(|error| CopilotHttpTransportError::ClientUnavailable(*error))?
                .get(url)
                .headers(headers)
                .send()
                .await
                .map_err(CopilotHttpTransportError::Request)?;
            if response.status().is_redirection() {
                return Err(CopilotHttpTransportError::RedirectRefused {
                    status: response.status().as_u16(),
                });
            }
            if response
                .content_length()
                .is_some_and(|length| length > MAX_COPILOT_HTTP_RESPONSE_BYTES as u64)
            {
                return Err(CopilotHttpTransportError::ResponseTooLarge);
            }
            let status = response.status().as_u16();
            let retry_after = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<u64>().ok())
                .map(StdDuration::from_secs);
            let rate_limit_reset = header_text(response.headers(), "x-ratelimit-reset")
                .and_then(|value| value.parse::<u64>().ok());
            let header_rate_limited = github_rate_limited(
                status,
                header_text(response.headers(), "x-ratelimit-remaining"),
                retry_after.is_some(),
            );
            let mut stream = response.bytes_stream();
            let mut body = Vec::new();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(CopilotHttpTransportError::Request)?;
                if body.len().saturating_add(chunk.len()) > MAX_COPILOT_HTTP_RESPONSE_BYTES {
                    return Err(CopilotHttpTransportError::ResponseTooLarge);
                }
                body.extend_from_slice(&chunk);
            }
            // A secondary rate limit can show only in its message, so the
            // answer is classified once the body is read.
            let rate_limited =
                header_rate_limited || (status == 403 && github_secondary_rate_limit(&body));
            Ok(CopilotHttpResponse {
                status,
                body,
                retry_after,
                rate_limited,
                rate_limit_reset,
            })
        })
        .await
        .map_err(|_| CopilotHttpTransportError::Timeout)?
    }
}

fn header_text<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
}

/// GitHub's REST rate-limit rule by headers: a 429, or a 403 whose primary
/// limit is exhausted (`x-ratelimit-remaining: 0`) or that carries
/// `Retry-After` (a secondary limit). A secondary limit can also come without
/// any of these headers ([`github_secondary_rate_limit`]).
fn github_rate_limited(status: u16, remaining: Option<&str>, has_retry_after: bool) -> bool {
    status == 429 || (status == 403 && (remaining == Some("0") || has_retry_after))
}

/// Whether GitHub's answer says it exceeded a secondary rate limit: its
/// `message` (or, for a body that is not GitHub's JSON, the body's text)
/// names one as GitHub's own client recognises it (octokit's throttling
/// rule, `\bsecondary rate\b` case-insensitively, so "secondary rate limit"
/// and "secondary rate-limit" alike), since GitHub can send it as a 403
/// with no `Retry-After` and no rate-limit header. With no stated time the
/// wait is the minute GitHub asks for at least ([`rate_limit_until`]).
fn github_secondary_rate_limit(body: &[u8]) -> bool {
    const PHRASE: &str = "secondary rate";
    let message = serde_json::from_slice::<GitHubErrorEnvelope>(body)
        .ok()
        .and_then(|envelope| envelope.message)
        .unwrap_or_else(|| String::from_utf8_lossy(body).into_owned())
        .to_ascii_lowercase();
    let word = |character: char| character.is_ascii_alphanumeric() || character == '_';
    message.match_indices(PHRASE).any(|(start, _)| {
        !message[..start].chars().next_back().is_some_and(word)
            && !message[start + PHRASE.len()..]
                .chars()
                .next()
                .is_some_and(word)
    })
}

pub struct CopilotRuntime {
    accounts: Mutex<BTreeMap<CopilotAccountCacheKey, Weak<CopilotAccountState>>>,
    /// Pacing per credential account (GitHub's rate limits on the identity
    /// lookup, and the pause on re-reads of a source whose token the lease
    /// withdrew), held for the runtime's life so it outlives the account
    /// states that read it. The key is the account alone (the credential
    /// identity the routes share and the AuthMachine lease key comes from),
    /// not the persistence authority: every account state of one account
    /// shares one pacing, whichever credential store it was built over. The
    /// map stays bounded: it holds one small entry per credential account a
    /// route was resolved for, a set the realm configuration fixes, and
    /// nothing per request, session or persistence instance. An entry's
    /// re-read pauses are removed by the first claim that finds its lease
    /// published again; there is at most one per registry, and production
    /// registries live for the process.
    pacing: Mutex<BTreeMap<AuthCredentialIdentity, Arc<CopilotAccountPacing>>>,
    transport: Arc<dyn CopilotHttpTransport>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct CopilotAccountCacheKey {
    persistence_id: meerkat_core::auth::ProviderAuthPersistenceId,
    credential_identity: AuthCredentialIdentity,
}

impl CopilotRuntime {
    pub fn new() -> Self {
        Self::with_transport(Arc::new(ReqwestCopilotHttpTransport::new()))
    }

    fn with_transport(transport: Arc<dyn CopilotHttpTransport>) -> Self {
        Self {
            accounts: Mutex::new(BTreeMap::new()),
            pacing: Mutex::new(BTreeMap::new()),
            transport,
        }
    }

    pub async fn resolve(
        &self,
        binding: &ValidatedBinding,
        env: &ResolverEnvironment,
    ) -> Result<CopilotResolvedAuth, ProviderAuthError> {
        if !matches!(
            binding.credential_identity(),
            AuthCredentialIdentity::Account(_)
        ) {
            return Err(ProviderAuthError::SourceResolutionFailed(
                "Copilot backend requires an account-scoped credential identity".to_string(),
            ));
        }
        let Some(mode) = CopilotCredentialMode::for_auth_method(binding.auth()).filter(|mode| {
            binding.auth().persisted_auth_mode() == Some(mode.persisted_auth_mode())
        }) else {
            return Err(ProviderAuthError::SourceResolutionFailed(
                "Copilot backend requires github_copilot_oauth or github_copilot_token".to_string(),
            ));
        };
        // The device sign-in reads only the token its login stored. The
        // operator token comes from any simple-secret source; the resolver
        // refuses the ones it cannot read (`platform_default`,
        // `file_descriptor`).
        if mode == CopilotCredentialMode::DeviceSignIn
            && !matches!(
                binding.auth_profile().source,
                CredentialSourceSpec::ManagedStore
            )
        {
            return Err(ProviderAuthError::SourceResolutionFailed(
                "github_copilot_oauth requires source.kind = managed_store; for an \
                 operator-supplied GitHub token use auth_method = \"github_copilot_token\""
                    .to_string(),
            ));
        }
        // A command source's declared lifetime must outlast the AuthMachine's
        // refresh window, which starts that long before the lifetime ends:
        // an interval inside it would run the command and look the account
        // up again on every request. Refused, never clamped.
        if mode == CopilotCredentialMode::OperatorToken
            && let CredentialSourceSpec::Command {
                refresh_interval_ms: Some(interval_ms),
                ..
            } = &binding.auth_profile().source
            && command_lifetime_within_refresh_window(*interval_ms)
        {
            return Err(short_command_refresh_interval_refusal());
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

        // A token route sends the integration id the Copilot API accepts for
        // a GitHub token unless its options name one; the device sign-in
        // keeps its own default.
        let options = &binding.backend_profile().options;
        let config = match mode {
            CopilotCredentialMode::OperatorToken => {
                CopilotBackendConfig::from_token_options(options)
            }
            CopilotCredentialMode::DeviceSignIn => CopilotBackendConfig::from_options(options),
        }
        .map_err(|error| ProviderAuthError::SourceResolutionFailed(error.to_string()))?;
        let state = self.account_state(
            persistence.authority_id(),
            binding.credential_identity(),
            config,
            mode,
        )?;
        state.claim_source(
            &binding.auth_profile().source,
            binding.credential_identity(),
        )?;
        let authorizer = Arc::new(CopilotAuthorizer {
            state,
            binding: binding.clone(),
            env: env.clone(),
            transport: Arc::clone(&self.transport),
        });
        // A token exchange that is temporarily unavailable does not fail a
        // resolution that admits pending account limits (a build whose output
        // budget is fixed before resolution): the account's routes resolve on
        // its last known model list, or without one when no token was minted
        // yet, and the first request tries the exchange again, failing with a
        // retryable error while the outage lasts. Any other resolution fails
        // during the outage, and every other failure, an account action
        // included, fails it as before.
        let primed = match authorizer.ensure_token().await {
            Ok(primed) => Some(primed),
            Err(CopilotTokenError::ExchangeUnavailable(_))
                if env.admits_pending_account_limits() =>
            {
                None
            }
            Err(error) => return Err(error.into_provider()),
        };
        // An operator token names its account: the login and plan GitHub
        // reported (none for an installation token). The device sign-in
        // reports none.
        let metadata = primed
            .as_ref()
            .and_then(|primed| primed.identity.as_deref())
            .map_or_else(AuthMetadata::default, |identity| AuthMetadata {
                account_id: identity.login.clone(),
                plan: identity.plan.clone(),
                ..AuthMetadata::default()
            });
        let metadata = meerkat_auth_core::resolver::finalize_auth_metadata(binding, metadata)?;
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
        mode: CopilotCredentialMode,
    ) -> Result<Arc<CopilotAccountState>, ProviderAuthError> {
        let key = CopilotAccountCacheKey {
            persistence_id,
            credential_identity: identity.clone(),
        };
        let mut accounts = self.accounts.lock();
        accounts.retain(|_, state| state.strong_count() > 0);
        if let Some(existing) = accounts.get(&key).and_then(Weak::upgrade) {
            if existing.mode != mode {
                return Err(ProviderAuthError::SourceResolutionFailed(format!(
                    "Copilot routes sharing credential identity '{identity}' mix {} and {}; \
                     one credential account holds one Copilot auth method",
                    existing.mode.auth_method(),
                    mode.auth_method()
                )));
            }
            if existing.config != config {
                return Err(ProviderAuthError::SourceResolutionFailed(format!(
                    "Copilot routes sharing credential identity '{identity}' disagree on backend options"
                )));
            }
            return Ok(existing);
        }
        let pacing = Arc::clone(self.pacing.lock().entry(identity.clone()).or_default());
        let state = Arc::new(CopilotAccountState::with_pacing(config, mode, pacing));
        accounts.insert(key, Arc::downgrade(&state));
        Ok(state)
    }

    pub fn resolve_route(
        &self,
        connection: &meerkat_llm_core::provider_runtime::ResolvedConnection,
        provider: Provider,
        model: &str,
    ) -> Option<CopilotRouteResolution> {
        let state = self.connection_state(connection)?;
        resolve_route_for_state(&state, provider, model)
    }

    /// The account state `connection`'s Copilot authorizer shares.
    fn connection_state(
        &self,
        connection: &meerkat_llm_core::provider_runtime::ResolvedConnection,
    ) -> Option<Arc<CopilotAccountState>> {
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
        self.accounts.lock().get(&key).and_then(Weak::upgrade)
    }

    /// The retryable class of the account's last token exchange failure,
    /// while that failure leaves the account without a token to serve.
    fn exchange_unavailable(
        &self,
        connection: &meerkat_llm_core::provider_runtime::ResolvedConnection,
    ) -> Option<meerkat_llm_core::LlmError> {
        let state = self.connection_state(connection)?;
        let cache = state.cache.read();
        cache
            .as_ref()?
            .exchange_unavailable
            .as_ref()
            .map(|unavailable| unavailable.retryable.clone())
    }

    /// What the model list of `connection`'s Copilot account states about
    /// `model`, which the model registry does not define
    /// ([`CopilotModelSnapshot::account_model`]). The list is the one the
    /// account state behind the connection holds, loaded when the route
    /// resolved; while it holds none (the account's model discovery has not
    /// answered), the list defines no model, and the model is refused as
    /// unavailable.
    pub fn account_model(
        &self,
        connection: &meerkat_llm_core::provider_runtime::ResolvedConnection,
        model: &str,
    ) -> meerkat_llm_core::provider_runtime::AccountModelLookup {
        use meerkat_llm_core::provider_runtime::{AccountModelLookup, AccountModelRefusal};

        let snapshot = self.connection_state(connection).and_then(|state| {
            state
                .cache
                .read()
                .as_ref()
                .and_then(|cached| cached.models.clone())
        });
        match snapshot {
            Some(snapshot) => snapshot.account_model(&connection.credential_identity, model),
            None => AccountModelLookup::Refused(AccountModelRefusal::ModelsUnavailable {
                credential: connection.credential_identity.clone(),
                model: model.to_string(),
            }),
        }
    }
}

fn resolve_route_for_state(
    state: &Arc<CopilotAccountState>,
    provider: Provider,
    model: &str,
) -> Option<CopilotRouteResolution> {
    let cache = state.cache.read();
    let cached = cache.as_ref()?;
    let (access, capabilities, available_models) = match cached.models.as_ref() {
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
                Some(Arc::clone(snapshot)),
            )
        }
        None => (CopilotModelAccess::Unknown, None, None),
    };
    Some(CopilotRouteResolution {
        access,
        api_base: cached.api_base.clone(),
        capabilities,
        available_models,
        state: Arc::downgrade(state),
        provider,
        model: model.to_string(),
    })
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

    pub fn model_snapshot(&self) -> Option<Arc<CopilotModelSnapshot>> {
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
    available_models: Option<Arc<CopilotModelSnapshot>>,
    state: Weak<CopilotAccountState>,
    provider: Provider,
    model: String,
}

impl CopilotRouteResolution {
    pub fn bind_authorizer(&self, inner: Arc<dyn HttpAuthorizer>) -> Arc<dyn HttpAuthorizer> {
        Arc::new(CopilotRouteBoundAuthorizer {
            inner,
            state: self.state.clone(),
            provider: self.provider,
            model: self.model.clone(),
            expected: CopilotRouteFingerprint::from(self),
        })
    }

    pub fn unavailable_message(
        &self,
        provider: Provider,
        model: &str,
        endpoint: CopilotEndpoint,
    ) -> String {
        // A model the account lists under a policy that is not `enabled` is
        // refused by naming the state the account states.
        // So is a model it lists with an empty endpoint list.
        let listed = self
            .available_models
            .as_ref()
            .and_then(|snapshot| snapshot.model(model));
        let refusing_state = listed.and_then(|offering| offering.refusing_policy_state());
        let mut message = match refusing_state {
            Some(state) => format!(
                "Copilot account policy state for {} model '{model}' is '{state}', not 'enabled'",
                provider.as_str(),
            ),
            None if listed.is_some_and(|offering| offering.lists_no_endpoints()) => format!(
                "Copilot account lists no endpoints for {} model '{model}'",
                provider.as_str(),
            ),
            None => format!(
                "Copilot account does not expose {} for {} model '{model}'",
                endpoint.path(),
                provider.as_str(),
            ),
        };
        if let Some(snapshot) = self.available_models.as_ref() {
            let mut ids = snapshot.available_model_ids(provider).peekable();
            if ids.peek().is_some() {
                message.push_str("; available account models: ");
                for (index, id) in ids.enumerate() {
                    if index > 0 {
                        message.push_str(", ");
                    }
                    message.push_str(id);
                }
            }
        }
        message
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
    supports_image_input: bool,
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
            supports_image_input: true,
            supports_image_tool_results,
        }
    }

    #[must_use]
    pub fn with_image_input_support(mut self, supports_image_input: bool) -> Self {
        self.supports_image_input = supports_image_input;
        self
    }

    #[must_use]
    pub const fn supports_image_input(&self) -> bool {
        self.supports_image_input
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
    let route = runtime
        .resolve_route(&connection, provider, &model)
        .ok_or_else(|| {
            ProviderClientError::ClientInit(
                "Copilot route was not initialized during auth resolution".to_string(),
            )
        })?;
    let fingerprint = CopilotRouteFingerprint::from(&route);
    let capabilities = route.capabilities.clone();
    let schema_compiler = factory(&route, &connection)?;
    let initial_client = capability_gated_client(Arc::clone(&schema_compiler), capabilities);
    let client = CopilotRoutedClient {
        runtime,
        connection,
        provider,
        model,
        factory,
        schema_compiler,
        prepared_route: RwLock::new(PreparedCopilotRoute {
            fingerprint,
            client: initial_client,
        }),
    };
    Ok(Arc::new(client))
}

#[derive(Clone, PartialEq, Eq)]
struct CopilotRouteFingerprint {
    access: CopilotModelAccess,
    api_base: String,
    capabilities: Option<crate::protocol::CopilotModelCapabilities>,
}

impl From<&CopilotRouteResolution> for CopilotRouteFingerprint {
    fn from(route: &CopilotRouteResolution) -> Self {
        Self {
            access: route.access,
            api_base: route.api_base.clone(),
            capabilities: route.capabilities.clone(),
        }
    }
}

#[derive(Clone)]
struct PreparedCopilotRoute {
    fingerprint: CopilotRouteFingerprint,
    client: Arc<dyn meerkat_llm_core::LlmClient>,
}

struct CopilotRouteBoundAuthorizer {
    inner: Arc<dyn HttpAuthorizer>,
    state: Weak<CopilotAccountState>,
    provider: Provider,
    model: String,
    expected: CopilotRouteFingerprint,
}

impl CopilotRouteBoundAuthorizer {
    fn ensure_current_route(&self) -> Result<(), AuthError> {
        let state = self.state.upgrade().ok_or_else(|| {
            AuthError::ResolveRequired("Copilot route state was retired".to_string())
        })?;
        let current = resolve_route_for_state(&state, self.provider, &self.model)
            .map(|route| CopilotRouteFingerprint::from(&route))
            .ok_or_else(|| {
                AuthError::ResolveRequired("Copilot route requires re-resolution".to_string())
            })?;
        if current != self.expected {
            return Err(AuthError::ResolveRequired(
                "Copilot API route changed while refreshing authorization".to_string(),
            ));
        }
        Ok(())
    }
}

#[async_trait]
impl HttpAuthorizer for CopilotRouteBoundAuthorizer {
    async fn prepare_request(&self) -> Result<(), AuthError> {
        self.inner.prepare_request().await?;
        self.ensure_current_route()
    }

    async fn authorize(&self, request: &mut HttpAuthorizationRequest<'_>) -> Result<(), AuthError> {
        self.inner.authorize(request).await?;
        self.ensure_current_route()
    }

    async fn authorize_with_receipt(
        &self,
        request: &mut HttpAuthorizationRequest<'_>,
    ) -> Result<HttpAuthorizationReceipt, AuthError> {
        let receipt = self.inner.authorize_with_receipt(request).await?;
        self.ensure_current_route()?;
        Ok(receipt)
    }

    async fn observe_response(
        &self,
        response: &HttpAuthorizationResponse<'_>,
    ) -> Result<HttpAuthorizationResponseAction, AuthError> {
        self.inner.observe_response(response).await
    }

    async fn observe_response_with_receipt(
        &self,
        receipt: HttpAuthorizationReceipt,
        response: &HttpAuthorizationResponse<'_>,
    ) -> Result<HttpAuthorizationResponseAction, AuthError> {
        self.inner
            .observe_response_with_receipt(receipt, response)
            .await
    }

    fn redact_response_diagnostic(&self, diagnostic: &mut String) {
        self.inner.redact_response_diagnostic(diagnostic);
    }

    fn label(&self) -> &str {
        self.inner.label()
    }

    fn append_content_headers(
        &self,
        content: meerkat_core::HttpAuthorizationContent,
        headers: &mut Vec<(String, String)>,
    ) -> Result<(), AuthError> {
        self.inner.append_content_headers(content, headers)
    }

    fn persistence_authority_id(&self) -> Option<meerkat_core::auth::ProviderAuthPersistenceId> {
        self.inner.persistence_authority_id()
    }

    fn expires_at(&self) -> Option<DateTime<Utc>> {
        self.inner.expires_at()
    }
}

struct CopilotRoutedClient {
    runtime: Arc<CopilotRuntime>,
    connection: meerkat_llm_core::provider_runtime::ResolvedConnection,
    provider: Provider,
    model: String,
    factory: CopilotRouteClientFactory,
    schema_compiler: Arc<dyn meerkat_llm_core::LlmClient>,
    prepared_route: RwLock<PreparedCopilotRoute>,
}

impl CopilotRoutedClient {
    fn sync_client(&self) -> Arc<dyn meerkat_llm_core::LlmClient> {
        Arc::clone(&self.prepared_route.read().client)
    }

    fn refresh_prepared_route(&self) -> Result<bool, ProviderClientError> {
        let route = self
            .runtime
            .resolve_route(&self.connection, self.provider, &self.model)
            .ok_or_else(|| {
                ProviderClientError::ClientInit(
                    "Copilot route is unavailable after authorization refresh".to_string(),
                )
            })?;
        let fingerprint = CopilotRouteFingerprint::from(&route);
        if self.prepared_route.read().fingerprint == fingerprint {
            return Ok(false);
        }
        let capabilities = route.capabilities.clone();
        let client =
            capability_gated_client((self.factory)(&route, &self.connection)?, capabilities);
        let mut prepared = self.prepared_route.write();
        if prepared.fingerprint == fingerprint {
            return Ok(false);
        }
        *prepared = PreparedCopilotRoute {
            fingerprint,
            client,
        };
        Ok(true)
    }

    fn client_error(error: ProviderClientError) -> meerkat_llm_core::LlmError {
        meerkat_llm_core::LlmError::InvalidRequest {
            message: error.to_string(),
        }
    }

    fn missing_route_witness() -> meerkat_llm_core::LlmError {
        meerkat_llm_core::LlmError::InvalidRequest {
            message: "Copilot routed requests require request-scoped route preparation".to_string(),
        }
    }

    /// A Copilot authorization failure as a turn sees it: while the account's
    /// token exchange is temporarily unavailable and no token is left to
    /// serve, the exchange failure's retryable class; otherwise the
    /// authorizer's own error.
    fn authorization_error(&self, error: AuthError) -> meerkat_llm_core::LlmError {
        if matches!(error, AuthError::RefreshFailed(_))
            && let Some(retryable) = self.runtime.exchange_unavailable(&self.connection)
        {
            return retryable;
        }
        meerkat_llm_core::LlmError::from_authorizer(error)
    }

    fn ensure_prepared_route_current(
        &self,
        prepared: &PreparedCopilotRoute,
    ) -> Result<(), meerkat_llm_core::LlmError> {
        let current = self
            .runtime
            .resolve_route(&self.connection, self.provider, &self.model)
            .ok_or_else(|| meerkat_llm_core::LlmError::ModelNotFound {
                model: self.model.clone(),
            })?;
        if CopilotRouteFingerprint::from(&current) == prepared.fingerprint {
            return Ok(());
        }
        self.refresh_prepared_route().map_err(Self::client_error)?;
        Err(meerkat_llm_core::LlmError::AuthorizationRouteChanged {
            message: "Copilot route changed before request lowering completed".to_string(),
        })
    }
}

#[async_trait]
impl meerkat_llm_core::LlmClient for CopilotRoutedClient {
    /// The current route's account limit.
    fn output_token_limit(&self) -> Option<u32> {
        self.sync_client().output_token_limit()
    }

    /// The current route's account prompt limit.
    fn prompt_token_limit(&self) -> Option<u32> {
        self.sync_client().prompt_token_limit()
    }

    async fn prepare_controller_credential(&self) -> Result<(), AuthError> {
        self.connection
            .resolved_authorizer()
            .ok_or(AuthError::HostOwnedUnavailable)?
            .prepare_request()
            .await
    }

    fn project_replay_request(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<meerkat_llm_core::LlmReplayProjection, meerkat_llm_core::LlmError> {
        self.refresh_prepared_route().map_err(Self::client_error)?;
        let prepared = self.prepared_route.read().clone();
        let messages = prepared.client.project_replay_messages(messages)?;
        Ok(meerkat_llm_core::LlmReplayProjection::new(messages)
            .with_route_witness(meerkat_llm_core::LlmRequestRouteWitness::new(prepared)))
    }

    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, meerkat_llm_core::LlmError> {
        self.sync_client().project_replay_messages(messages)
    }

    fn request_pressure(
        &self,
        _request: &meerkat_llm_core::LlmRequest,
    ) -> Result<Option<meerkat_core::ProviderRequestPressure>, meerkat_llm_core::LlmError> {
        Err(Self::missing_route_witness())
    }

    fn prepared_request_pressure(
        &self,
        request: &meerkat_llm_core::PreparedLlmRequest,
    ) -> Result<Option<meerkat_core::ProviderRequestPressure>, meerkat_llm_core::LlmError> {
        let prepared = request
            .route_witness::<PreparedCopilotRoute>()
            .ok_or_else(Self::missing_route_witness)?;
        self.ensure_prepared_route_current(prepared)?;
        prepared.client.request_pressure(request.request())
    }

    fn authored_cache_breakpoints(
        &self,
        _request: &meerkat_llm_core::LlmRequest,
        _canonical_messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::ProviderCacheBreakpointClaim>, meerkat_llm_core::LlmError> {
        Err(Self::missing_route_witness())
    }

    fn prepared_cache_breakpoints(
        &self,
        request: &meerkat_llm_core::PreparedLlmRequest,
        canonical_messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::ProviderCacheBreakpointClaim>, meerkat_llm_core::LlmError> {
        let prepared = request
            .route_witness::<PreparedCopilotRoute>()
            .ok_or_else(Self::missing_route_witness)?;
        self.ensure_prepared_route_current(prepared)?;
        prepared
            .client
            .authored_cache_breakpoints(request.request(), canonical_messages)
    }

    fn stream<'a>(
        &'a self,
        _request: &'a meerkat_llm_core::LlmRequest,
    ) -> meerkat_llm_core::LlmStream<'a> {
        Box::pin(futures::stream::once(async {
            Err(Self::missing_route_witness())
        }))
    }

    fn stream_prepared<'a>(
        &'a self,
        request: &'a meerkat_llm_core::PreparedLlmRequest,
    ) -> meerkat_llm_core::LlmStream<'a> {
        let Some(prepared) = request.route_witness::<PreparedCopilotRoute>().cloned() else {
            return Box::pin(futures::stream::once(async {
                Err(Self::missing_route_witness())
            }));
        };
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
                .map_err(|error| self.authorization_error(error))?;
            self.ensure_prepared_route_current(&prepared)?;
            let client = Arc::clone(&prepared.client);
            // Keep the original replay-route witness and per-work companion
            // through delegation. The concrete provider checks its lowered
            // target again after this account/route preparation await.
            let mut stream = client.stream_prepared(request);
            let mut emitted = false;
            while let Some(result) = stream.next().await {
                match result {
                    Ok(event) => {
                    emitted = true;
                    yield event;
                    }
                    Err(error @ meerkat_llm_core::LlmError::AuthorizationRouteChanged { .. }) => {
                    if emitted {
                        Err(meerkat_llm_core::LlmError::IncompleteResponse {
                            message: "Copilot authorization route changed after streaming began"
                                .to_string(),
                        })?;
                    }
                    authorizer
                        .prepare_request()
                        .await
                        .map_err(|error| self.authorization_error(error))?;
                    self.refresh_prepared_route().map_err(Self::client_error)?;
                    Err(error)?;
                    }
                    // The provider client reports its own authorization
                    // failure (after a 401, minting a new token) this way.
                    Err(error @ meerkat_llm_core::LlmError::AuthenticationFailed { .. }) => {
                    Err(self.runtime.exchange_unavailable(&self.connection).unwrap_or(error))?;
                    }
                    Err(error) => Err(error)?,
                }
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
        self.refresh_prepared_route().map_err(Self::client_error)?;
        self.sync_client().health_check().await
    }

    fn compile_schema(
        &self,
        output_schema: &meerkat_core::OutputSchema,
    ) -> Result<meerkat_core::schema::CompiledSchema, meerkat_core::schema::SchemaError> {
        self.schema_compiler.compile_schema(output_schema)
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
        if supports.structured_outputs == Some(false)
            && request
                .provider_params
                .as_ref()
                .is_some_and(|tag| match tag {
                    meerkat_core::lifecycle::run_primitive::ProviderTag::Anthropic(tag) => {
                        tag.structured_output.is_some()
                    }
                    meerkat_core::lifecycle::run_primitive::ProviderTag::OpenAi(tag) => {
                        tag.structured_output.is_some()
                    }
                    meerkat_core::lifecycle::run_primitive::ProviderTag::Gemini(tag) => {
                        tag.structured_output.is_some()
                    }
                    meerkat_core::lifecycle::run_primitive::ProviderTag::Unknown { .. } => false,
                })
        {
            return Err(meerkat_llm_core::LlmError::InvalidRequest {
                message: format!(
                    "Copilot account model '{}' does not support structured outputs",
                    request.model
                ),
            });
        }
        if supports.vision == Some(false) && request.has_images() {
            return Err(meerkat_llm_core::LlmError::InvalidRequest {
                message: format!(
                    "Copilot account model '{}' does not support image input",
                    request.model
                ),
            });
        }
        if let Some(meerkat_core::lifecycle::run_primitive::ProviderTag::Anthropic(tag)) =
            request.provider_params.as_ref()
        {
            if supports.adaptive_thinking == Some(false)
                && matches!(
                    tag.thinking,
                    Some(meerkat_core::lifecycle::run_primitive::AnthropicThinkingConfig::Adaptive)
                )
            {
                return Err(meerkat_llm_core::LlmError::InvalidRequest {
                    message: format!(
                        "Copilot account model '{}' does not support adaptive thinking",
                        request.model
                    ),
                });
            }
            let thinking_budget = match tag.thinking {
                Some(
                    meerkat_core::lifecycle::run_primitive::AnthropicThinkingConfig::Enabled {
                        budget_tokens,
                    },
                ) => Some(budget_tokens),
                Some(
                    meerkat_core::lifecycle::run_primitive::AnthropicThinkingConfig::Adaptive
                    | meerkat_core::lifecycle::run_primitive::AnthropicThinkingConfig::BetweenTools
                    | meerkat_core::lifecycle::run_primitive::AnthropicThinkingConfig::Disabled,
                ) => None,
                None => tag.thinking_budget_tokens,
            };
            if let (Some(budget), Some(maximum)) = (thinking_budget, supports.max_thinking_budget)
                && budget > maximum
            {
                return Err(meerkat_llm_core::LlmError::InvalidRequest {
                    message: format!(
                        "Copilot account model '{}' allows at most {maximum} thinking tokens, requested {budget}",
                        request.model
                    ),
                });
            }
            if let (Some(budget), Some(minimum)) = (thinking_budget, supports.min_thinking_budget)
                && budget < minimum
            {
                return Err(meerkat_llm_core::LlmError::InvalidRequest {
                    message: format!(
                        "Copilot account model '{}' requires at least {minimum} thinking tokens, requested {budget}",
                        request.model
                    ),
                });
            }
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

#[async_trait]
impl meerkat_llm_core::LlmClient for CopilotCapabilityGatedClient {
    async fn prepare_controller_credential(&self) -> Result<(), AuthError> {
        self.inner.prepare_controller_credential().await
    }

    /// The account model's own output limit, the one the gate enforces.
    fn output_token_limit(&self) -> Option<u32> {
        self.capabilities
            .limits
            .max_output_tokens
            .or_else(|| self.inner.output_token_limit())
    }

    /// The account model's own prompt limit: the tighter of its prompt
    /// limit and its context window, either alone when only one is stated.
    fn prompt_token_limit(&self) -> Option<u32> {
        let limits = &self.capabilities.limits;
        match (limits.max_prompt_tokens, limits.max_context_window_tokens) {
            (Some(prompt), Some(window)) => Some(prompt.min(window)),
            (prompt, window) => prompt.or(window),
        }
        .or_else(|| self.inner.prompt_token_limit())
    }

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
        let pressure = self.inner.request_pressure(request)?;
        if let Some(measured) = pressure.as_ref().and_then(|value| {
            value
                .provider_issued_input_tokens
                .map(|tokens| (tokens, value))
        }) {
            let prompt_limit = self
                .capabilities
                .limits
                .max_prompt_tokens
                .or_else(|| {
                    self.capabilities
                        .limits
                        .max_context_window_tokens
                        .map(|context| context.saturating_sub(request.max_tokens))
                })
                .map(u64::from);
            // A prompt the provider measured over the account's limit is a
            // context refusal, the class the agent compacts on. Copilot's own
            // answer to such a prompt is not known, so none is assumed.
            if let Some(limit) = prompt_limit
                && measured.0 > limit
            {
                return Err(meerkat_llm_core::LlmError::ContextLengthExceeded {
                    max: usize::try_from(limit).unwrap_or(usize::MAX),
                    requested: usize::try_from(measured.0).unwrap_or(usize::MAX),
                });
            }
        }
        Ok(pressure)
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

    /// Gate the request exactly as `stream` does, then hand the adapter the
    /// request in its prepared form: its route witness and any work
    /// authorization stay with it, and the adapter checks its own lowered
    /// target.
    fn stream_prepared<'a>(
        &'a self,
        request: &'a meerkat_llm_core::PreparedLlmRequest,
    ) -> meerkat_llm_core::LlmStream<'a> {
        if let Err(error) = self.validate_request(request.request()) {
            return Box::pin(futures::stream::once(async move { Err(error) }));
        }
        self.inner.stream_prepared(request)
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
        self.inner.compile_schema(output_schema)
    }
}

struct CopilotAccountState {
    config: CopilotBackendConfig,
    mode: CopilotCredentialMode,
    cache: RwLock<Option<CachedCopilotToken>>,
    refresh_in_flight: AtomicBool,
    refresh_notify: tokio::sync::Notify,
    next_refresh_flight: AtomicU64,
    last_refresh_outcome: Mutex<Option<CopilotRefreshOutcome>>,
    next_generation: AtomicU64,
    /// Operator-token mode: the account's pacing (rate limits and withdrawn
    /// re-reads), shared with every later account state of the same account.
    pacing: Arc<CopilotAccountPacing>,
    /// The bearer values this account state's authorizers signed requests
    /// with, oldest first, at most [`REDACTED_BEARER_HISTORY`]: in token mode
    /// the operator's GitHub token, otherwise the derived Copilot token. A
    /// provider, proxy or gateway can echo a request's headers in an error,
    /// so each is redacted from what a provider client reads from a response
    /// ([`HttpAuthorizer::redact_response_diagnostic`]), a value a rotation
    /// replaced included.
    signed_bearers: Mutex<std::collections::VecDeque<String>>,
    /// The credential source the account's routes read the token from: the
    /// first route's, which every later route of the account must name too.
    source: Mutex<Option<CredentialSourceSpec>>,
}

/// Pacing of an operator-token account: GitHub's rate limits on its identity
/// lookup, and re-reads of a source that runs something while the lease
/// withdraws the token it gave (GitHub refused it, or a logout released it).
/// The runtime keeps it per account for its own life: a resolve that fails
/// on a rate limit or on a withdrawn token drops its account state, and the
/// next one must still wait.
#[derive(Default)]
struct CopilotAccountPacing {
    /// No identity lookup before this instant: GitHub rate limited the last
    /// one.
    identity_retry_at: Mutex<Option<DateTime<Utc>>>,
    /// Per AuthMachine lease registry of the account (the registry's
    /// `auth_lifecycle_restore_instance_id`): while that registry's lease
    /// withdraws the token (GitHub refused it, or a logout released it), a
    /// source that runs something is not read again before the entry's
    /// instant. Two registries for one account (two runtime stores) pause
    /// apart. An entry is removed by the first claim that finds its lease
    /// published again; there is at most one per registry, and production
    /// registries live for the process.
    withdrawn_source_rereads: Mutex<BTreeMap<usize, WithdrawnSourceReread>>,
    /// The Copilot API's refusal of a token GitHub's identity lookup did not
    /// answer: that token is not asked about again before the pause ends,
    /// and a request in between fails with the same refusal. Another token
    /// value is asked about at once.
    refused_token: Mutex<Option<RefusedTokenPause>>,
}

/// A paused Copilot refusal of one token value.
#[derive(Clone)]
struct RefusedTokenPause {
    token: CredentialFingerprint,
    until: DateTime<Utc>,
    /// The refusal a request inside the pause fails with.
    detail: String,
}

/// When the source of a token the lease withdrew (GitHub refused it, or a
/// logout released it) may be read again.
#[derive(Clone, Copy)]
struct WithdrawnSourceReread {
    /// The lease generation that withdrew the token. Any later publication
    /// (another session or machine published a value, a login stored one)
    /// ends the pause at once.
    generation: u64,
    /// The source is read again from this instant on.
    at: DateTime<Utc>,
}

impl CopilotAccountState {
    /// The routes of one account read its token from one source, as they
    /// share one auth method and one configuration: the first route's
    /// source is the account's, and a route over another one is refused.
    /// A token is then never labelled against another source's
    /// publications (for a stored token, the record's marker generation).
    fn claim_source(
        &self,
        source: &CredentialSourceSpec,
        identity: &AuthCredentialIdentity,
    ) -> Result<(), ProviderAuthError> {
        let mut claimed = self.source.lock();
        match claimed.as_ref() {
            None => {
                *claimed = Some(source.clone());
                Ok(())
            }
            Some(account_source) if account_source == source => Ok(()),
            Some(_) => Err(ProviderAuthError::SourceResolutionFailed(format!(
                "Copilot routes sharing credential identity '{identity}' disagree on the \
                 credential source; one credential account reads its token from one source"
            ))),
        }
    }

    /// An account state with pacing of its own (tests; the runtime shares
    /// its per-account pacing through `with_pacing`).
    #[cfg(test)]
    fn new(config: CopilotBackendConfig, mode: CopilotCredentialMode) -> Self {
        Self::with_pacing(config, mode, Arc::default())
    }

    fn with_pacing(
        config: CopilotBackendConfig,
        mode: CopilotCredentialMode,
        pacing: Arc<CopilotAccountPacing>,
    ) -> Self {
        Self {
            config,
            mode,
            cache: RwLock::new(None),
            refresh_in_flight: AtomicBool::new(false),
            refresh_notify: tokio::sync::Notify::new(),
            next_refresh_flight: AtomicU64::new(1),
            last_refresh_outcome: Mutex::new(None),
            next_generation: AtomicU64::new(1),
            pacing,
            signed_bearers: Mutex::new(std::collections::VecDeque::new()),
            source: Mutex::new(None),
        }
    }

    /// Keep `bearer`, the value a request was just signed with, for
    /// [`Self::redact_signed_bearers`].
    fn record_signed_bearer(&self, bearer: &str) {
        if bearer.is_empty() {
            return;
        }
        let mut signed = self.signed_bearers.lock();
        if signed.back().is_some_and(|last| last == bearer) {
            return;
        }
        signed.retain(|known| known != bearer);
        signed.push_back(bearer.to_string());
        while signed.len() > REDACTED_BEARER_HISTORY {
            signed.pop_front();
        }
    }

    /// Remove from `diagnostic` every bearer value this account state signed
    /// requests with.
    fn redact_signed_bearers(&self, diagnostic: &mut String) {
        for bearer in self.signed_bearers.lock().iter() {
            redact_secret(diagnostic, bearer);
        }
    }
}

struct CopilotRefreshOutcome {
    flight_id: u64,
    source_generation: u64,
    error: Option<CopilotTokenError>,
}

/// Why the account has no token to serve. Callers that shared a refresh
/// flight receive its leader's verdict.
#[derive(Clone)]
enum CopilotTokenError {
    /// The token exchange failed transiently and no valid token is left. The
    /// account recorded the outage; the next use tries the exchange again.
    ExchangeUnavailable(ProviderAuthError),
    /// Every other failure, an account action included.
    Failed(ProviderAuthError),
}

impl From<ProviderAuthError> for CopilotTokenError {
    fn from(error: ProviderAuthError) -> Self {
        Self::Failed(error)
    }
}

impl CopilotTokenError {
    fn into_provider(self) -> ProviderAuthError {
        match self {
            Self::ExchangeUnavailable(error) | Self::Failed(error) => error,
        }
    }
}

struct CopilotRefreshFlight<'a>(&'a CopilotAccountState);

impl Drop for CopilotRefreshFlight<'_> {
    fn drop(&mut self) {
        self.0.refresh_in_flight.store(false, Ordering::Release);
        self.0.refresh_notify.notify_waiters();
    }
}

struct CachedCopilotToken {
    generation: u64,
    token: String,
    source_generation: u64,
    /// The expiry of the source credential's publication this entry serves,
    /// which the authorizer reports (`HttpAuthorizer::expires_at`): the
    /// device sign-in's GitHub token expiry, or in token mode the expiry
    /// the account lease's publication declares (a command source's
    /// `refresh_interval_ms`; none otherwise). An agent build compares it
    /// with the lease before it publishes the lease again, so it must be the
    /// lease's own.
    source_expires_at: Option<DateTime<Utc>>,
    /// When the next token exchange is due. Never later than `expires_at`.
    refresh_at: DateTime<Utc>,
    /// When the token stops serving: its own expiry, or the moment Copilot
    /// refused it. Until then a refresh that fails transiently keeps
    /// serving it.
    expires_at: DateTime<Utc>,
    api_base: String,
    models: Option<Arc<CopilotModelSnapshot>>,
    model_discovery_refresh_at: Option<DateTime<Utc>>,
    /// Operator-token mode: the identity the token resolved to.
    identity: Option<Arc<CopilotAccountIdentity>>,
    /// The last token exchange failure, while it was transient and left no
    /// token to serve.
    exchange_unavailable: Option<CopilotExchangeUnavailable>,
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
            .field("expires_at", &self.expires_at)
            .field("api_base", &self.api_base)
            .field("has_models", &self.models.is_some())
            .field(
                "model_discovery_refresh_at",
                &self.model_discovery_refresh_at,
            )
            .field("identity", &self.identity)
            .field("exchange_unavailable", &self.exchange_unavailable.is_some())
            .finish()
    }
}

impl CachedCopilotToken {
    /// Copilot refused this token: it serves no further request, and the
    /// next use mints a new one. The entry stays, with the account's API
    /// base and model snapshot, so the routes that share the account keep
    /// resolving while the new token is minted.
    fn refuse(&mut self) {
        self.refresh_at = DateTime::<Utc>::MIN_UTC;
        self.expires_at = DateTime::<Utc>::MIN_UTC;
    }

    /// An account whose first token exchange failed transiently: no token
    /// serves, the API base is the default one until a token names the
    /// account's own, and no model list is known, so the account's routes
    /// resolve without one. The next use tries the exchange again.
    fn awaiting_first_token(
        generation: u64,
        source_generation: u64,
        source_expires_at: Option<DateTime<Utc>>,
        exchange_unavailable: CopilotExchangeUnavailable,
    ) -> Self {
        Self {
            generation,
            token: String::new(),
            source_generation,
            source_expires_at,
            refresh_at: DateTime::<Utc>::MIN_UTC,
            expires_at: DateTime::<Utc>::MIN_UTC,
            api_base: DEFAULT_COPILOT_API_BASE.to_string(),
            models: None,
            model_discovery_refresh_at: None,
            identity: None,
            exchange_unavailable: Some(exchange_unavailable),
        }
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
        self.ensure_token()
            .await
            .map(|_| ())
            .map_err(CopilotTokenError::into_provider)
    }

    pub fn credential_identity(&self) -> &AuthCredentialIdentity {
        self.binding.credential_identity()
    }

    pub fn model_snapshot(&self) -> Option<Arc<CopilotModelSnapshot>> {
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

    /// The GitHub identity the operator token of a `github_copilot_token`
    /// route resolved to, once its account state is built. `None` for the
    /// device sign-in.
    pub fn account_identity(&self) -> Option<CopilotAccountIdentity> {
        self.state
            .cache
            .read()
            .as_ref()
            .and_then(|cached| cached.identity.as_deref().cloned())
    }

    /// Copilot refused the token of `generation` (a 401 on a request it
    /// signed). A late refusal of an older generation leaves a newer token
    /// alone.
    fn refuse_derived_token_generation(&self, generation: u64) {
        let mut cache = self.state.cache.write();
        if let Some(cached) = cache.as_mut()
            && cached.generation == generation
        {
            cached.refuse();
        }
    }

    async fn ensure_token(&self) -> Result<CachedTokenView, CopilotTokenError> {
        let entry_flight_id = self
            .state
            .last_refresh_outcome
            .lock()
            .as_ref()
            .map_or(0, |outcome| outcome.flight_id);
        loop {
            let (source_generation, task) = match self.state.mode {
                CopilotCredentialMode::DeviceSignIn => {
                    let source = self.load_source_tokens().await?;
                    let source_generation = meerkat_core::tokens_lifecycle_published_generation(
                        &source,
                    )
                    .ok_or_else(|| {
                        ProviderAuthError::SourceResolutionFailed(
                            "Copilot source credential has no AuthMachine publication generation"
                                .to_string(),
                        )
                    })?;
                    let now = (self.env.now)();
                    if let Some(cached) = self.fresh_cached(source_generation, now) {
                        if cached.model_discovery_due {
                            self.refresh_models_off_the_request_path(&cached, source_generation)
                                .await;
                        }
                        return Ok(cached);
                    }
                    if let Some(outage) = self.outage_before_its_retry(source_generation, now) {
                        return Err(outage);
                    }
                    (source_generation, LeaderTask::Device(source))
                }
                CopilotCredentialMode::OperatorToken => match self.observe_operator_lease()? {
                    OperatorObservation::Cached { cached, generation } => {
                        if cached.model_discovery_due {
                            self.refresh_models_off_the_request_path(&cached, generation)
                                .await;
                        }
                        return Ok(cached);
                    }
                    OperatorObservation::Flight { generation } => {
                        (generation, LeaderTask::Operator)
                    }
                },
            };
            if let Some(outcome) = self.state.last_refresh_outcome.lock().as_ref()
                && outcome.flight_id != entry_flight_id
                && outcome.source_generation == source_generation
                && let Some(error) = outcome.error.as_ref()
            {
                return Err(error.clone());
            }

            let notified = self.state.refresh_notify.notified();
            if self
                .state
                .refresh_in_flight
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                if self.state.refresh_in_flight.load(Ordering::Acquire) {
                    notified.await;
                }
                if let Some(outcome) = self.state.last_refresh_outcome.lock().as_ref()
                    && outcome.flight_id != entry_flight_id
                    && outcome.source_generation == source_generation
                    && let Some(error) = outcome.error.as_ref()
                {
                    return Err(error.clone());
                }
                continue;
            }
            let flight_id = self
                .state
                .next_refresh_flight
                .fetch_add(1, Ordering::Relaxed);
            let _flight = CopilotRefreshFlight(self.state.as_ref());
            let result = match &task {
                LeaderTask::Device(source) => {
                    self.refresh_token_as_leader(source, source_generation)
                        .await
                }
                LeaderTask::Operator => self
                    .refresh_operator_token_as_leader()
                    .await
                    .map_err(CopilotTokenError::from),
            };
            // An operator-token flight publishes the token it read, which can
            // advance the lease generation: its outcome belongs to the
            // generation it ended on, the one its waiters observe next.
            let outcome_generation = match task {
                LeaderTask::Device(_) => source_generation,
                LeaderTask::Operator => self.auth_lease().map_or(source_generation, |auth_lease| {
                    auth_lease.snapshot(&self.lease_key()).generation
                }),
            };
            *self.state.last_refresh_outcome.lock() = Some(CopilotRefreshOutcome {
                flight_id,
                source_generation: outcome_generation,
                error: result.as_ref().err().cloned(),
            });
            return result;
        }
    }

    async fn refresh_token_as_leader(
        &self,
        source: &PersistedTokens,
        source_generation: u64,
    ) -> Result<CachedTokenView, CopilotTokenError> {
        let now = (self.env.now)();
        if let Some(cached) = self.fresh_cached(source_generation, now) {
            // Another flight minted the token this caller waited for.
            if cached.model_discovery_due {
                self.refresh_models_off_the_request_path(&cached, source_generation)
                    .await;
            }
            return Ok(cached);
        }
        self.remint(source, source_generation).await
    }

    /// Mint a new token from `source`. While the token exchange is
    /// temporarily unavailable, the current token keeps serving as long as
    /// it is valid and was minted from this source credential, and the
    /// exchange is tried again after its `Retry-After` (or shortly). Once no
    /// such token is left, the failure is recorded on the account, so a turn
    /// fails with its retryable class, and the caller receives
    /// [`CopilotTokenError::ExchangeUnavailable`].
    async fn remint(
        &self,
        source: &PersistedTokens,
        source_generation: u64,
    ) -> Result<CachedTokenView, CopilotTokenError> {
        let mut unavailable = match self.mint_and_cache_token(source, source_generation).await {
            Ok(view) => return Ok(view),
            Err(CopilotTokenExchangeError::Failed(error)) => {
                if let Some(cached) = self.state.cache.write().as_mut() {
                    cached.exchange_unavailable = None;
                }
                return Err(CopilotTokenError::Failed(error));
            }
            Err(CopilotTokenExchangeError::Unavailable(unavailable)) => unavailable,
        };
        let now = (self.env.now)();
        // Recorded below only when no token is left to serve.
        unavailable.not_before = unavailable
            .retry_after
            .map(|retry_after| exchange_not_before(now, retry_after));
        let error = ProviderAuthError::SourceResolutionFailed(unavailable.message.clone());
        let mut cache = self.state.cache.write();
        let Some(cached) = cache.as_mut() else {
            // No token was minted for this account yet. The outage is kept on
            // an entry that serves no token, so the account's routes resolve
            // (on the default API base, without a model list) and the next
            // use tries the exchange again.
            tracing::warn!(
                credential_identity = %self.binding.credential_identity(),
                error = %unavailable.message,
                retryable = %unavailable.retryable,
                "Copilot token exchange failed transiently before any token was minted; the next request tries it again"
            );
            *cache = Some(CachedCopilotToken::awaiting_first_token(
                self.state.next_generation.fetch_add(1, Ordering::Relaxed),
                source_generation,
                source.expires_at,
                unavailable,
            ));
            return Err(CopilotTokenError::ExchangeUnavailable(error));
        };
        if cached.source_generation == source_generation && now < cached.expires_at {
            cached.refresh_at = exchange_retry_at(now, unavailable.retry_after, cached.expires_at);
            tracing::warn!(
                credential_identity = %self.binding.credential_identity(),
                error = %unavailable.message,
                "Copilot token exchange failed transiently; the current token serves until it expires"
            );
            return Ok(CachedTokenView {
                generation: cached.generation,
                token: cached.token.clone(),
                api_base: cached.api_base.clone(),
                model_discovery_due: false,
                identity: cached.identity.clone(),
            });
        }
        tracing::warn!(
            credential_identity = %self.binding.credential_identity(),
            error = %unavailable.message,
            retryable = %unavailable.retryable,
            "Copilot token exchange failed transiently and no valid token is left; the turn fails with the retryable error and is retried"
        );
        if cached.source_generation != source_generation {
            // The entry's token was minted from an earlier sign-in. The entry
            // now speaks for the current one, so a build admitted during the
            // outage publishes the current sign-in's expiry, and the earlier
            // sign-in's token is refused so it never serves this one.
            cached.refuse();
            cached.source_generation = source_generation;
            cached.source_expires_at = source.expires_at;
        }
        cached.exchange_unavailable = Some(unavailable);
        Err(CopilotTokenError::ExchangeUnavailable(error))
    }

    async fn mint_and_cache_token(
        &self,
        source: &PersistedTokens,
        source_generation: u64,
    ) -> Result<CachedTokenView, CopilotTokenExchangeError> {
        let github_token = source
            .primary_secret
            .as_deref()
            .ok_or(ProviderAuthError::Auth(AuthError::MissingSecret))?;
        let mut token = self.mint_copilot_token(github_token).await?;
        let mut api_base = resolve_api_base(&token)?;
        let mut models_result = self.fetch_models(&api_base, &token.token).await;
        if models_result
            .as_ref()
            .is_err_and(|error| error.status() == Some(401))
        {
            token = self.mint_copilot_token(github_token).await?;
            api_base = resolve_api_base(&token)?;
            models_result = self.fetch_models(&api_base, &token.token).await;
        }
        // A listing that refuses the new token (a 401 again, or a 403 that
        // asks for an account action) needs a new sign-in; any other failure,
        // a 403 for a policy or a rate limit included, leaves the token
        // serving without the list until the next discovery.
        if models_result
            .as_ref()
            .is_err_and(CopilotModelDiscoveryError::refuses_derived_token)
        {
            self.mark_account_reauth_required()?;
            return Err(ProviderAuthError::Auth(AuthError::UserReauthRequired).into());
        }
        let now = (self.env.now)();
        let refresh_at = token_refresh_at(&token, now)?;
        let expires_at = token_expires_at(&token)?;
        let discovery_now = (self.env.now)();
        let (models, model_discovery_refresh_at) = match models_result {
            Ok(snapshot) => (
                Some(Arc::new(snapshot)),
                Some(model_discovery_deadline(
                    discovery_now,
                    StdDuration::from_secs(MODEL_DISCOVERY_REFRESH_SECONDS),
                )),
            ),
            Err(error) => {
                let retry_delay = error.retry_delay();
                tracing::warn!(
                    credential_identity = %self.binding.credential_identity(),
                    error = %error,
                    "Copilot authentication succeeded but account model discovery failed"
                );
                (
                    None,
                    Some(model_discovery_deadline(discovery_now, retry_delay)),
                )
            }
        };
        let generation = self.state.next_generation.fetch_add(1, Ordering::Relaxed);
        let view = CachedTokenView {
            generation,
            token: token.token.clone(),
            api_base: api_base.clone(),
            model_discovery_due: false,
            identity: None,
        };
        *self.state.cache.write() = Some(CachedCopilotToken {
            generation,
            token: token.token,
            source_generation,
            source_expires_at: source.expires_at,
            refresh_at,
            expires_at,
            api_base,
            models,
            model_discovery_refresh_at,
            identity: None,
            exchange_unavailable: None,
        });
        Ok(view)
    }

    /// The account's recorded exchange outage, while it answers in place of
    /// the exchange: no token is left to serve, the outage was recorded for
    /// the current sign-in, and the instant its exchange's `Retry-After`
    /// named is still ahead. Every request until then fails with the
    /// outage's retryable class without calling the exchange.
    fn outage_before_its_retry(
        &self,
        source_generation: u64,
        now: DateTime<Utc>,
    ) -> Option<CopilotTokenError> {
        let cache = self.state.cache.read();
        let cached = cache.as_ref()?;
        let unavailable = cached.exchange_unavailable.as_ref()?;
        (cached.source_generation == source_generation
            && now >= cached.expires_at
            && unavailable
                .not_before
                .is_some_and(|not_before| now < not_before))
        .then(|| {
            CopilotTokenError::ExchangeUnavailable(ProviderAuthError::SourceResolutionFailed(
                unavailable.message.clone(),
            ))
        })
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
                    .model_discovery_refresh_at
                    .is_some_and(|refresh_at| now >= refresh_at),
                identity: cached.identity.clone(),
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
                    .map_err(meerkat_auth_core::resolver::refresh_error_from_provider)
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

    fn mark_account_reauth_required(&self) -> Result<(), ProviderAuthError> {
        if let Some(auth_lease) = self.env.auth_lease_handle.as_ref() {
            let lease_key = meerkat_core::handles::LeaseKey::from_credential_identity(
                self.binding.credential_identity(),
            );
            auth_lease
                .mark_reauth_required(&lease_key)
                .map_err(|error| {
                    ProviderAuthError::SourceResolutionFailed(format!(
                        "Copilot source rejection could not be published to AuthMachine: {error}"
                    ))
                })?;
        }
        Ok(())
    }

    async fn mint_copilot_token(
        &self,
        github_token: &str,
    ) -> Result<CopilotTokenEnvelope, CopilotTokenExchangeError> {
        let endpoints = self
            .state
            .config
            .endpoints()
            .map_err(|error| ProviderAuthError::SourceResolutionFailed(error.to_string()))?;
        let mut headers = HeaderMap::new();
        insert_authorization_header(&mut headers, &format!("token {github_token}"))?;
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
            .map_err(|error| CopilotTokenExchangeError::from_transport(&error))?;
        if !(200..=299).contains(&response.status) {
            if token_exchange_requires_account_action(response.status, &response.body) {
                self.mark_account_reauth_required()?;
                return Err(ProviderAuthError::Auth(AuthError::UserReauthRequired).into());
            }
            // The status and GitHub's bounded message only, never the body: a
            // proxy or gateway page can echo the request's headers, the
            // GitHub token included.
            let status = response.status;
            let message = github_error_message(&response.body, github_token);
            return Err(CopilotTokenExchangeError::from_status(
                status,
                response.retry_after,
                format!("Copilot token exchange returned {status}: {message}"),
            ));
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
            )
            .into());
        }
        let now_epoch = (self.env.now)().timestamp().max(0) as u64;
        if token.expires_at <= now_epoch {
            return Err(ProviderAuthError::SourceResolutionFailed(
                "Copilot token response is already expired".to_string(),
            )
            .into());
        }
        Ok(token)
    }

    async fn fetch_models(
        &self,
        api_base: &str,
        token: &str,
    ) -> Result<CopilotModelSnapshot, CopilotModelDiscoveryError> {
        fetch_copilot_models(self.transport.as_ref(), &self.state.config, api_base, token).await
    }

    fn inference_headers(
        &self,
        token: &str,
        intent: CopilotRequestIntent,
    ) -> Result<HeaderMap, ProviderAuthError> {
        copilot_inference_headers(&self.state.config, token, intent)
    }
}

/// The account's model list: `GET {api_base}/models` with `token` as the
/// Copilot API bearer.
async fn fetch_copilot_models(
    transport: &dyn CopilotHttpTransport,
    config: &CopilotBackendConfig,
    api_base: &str,
    token: &str,
) -> Result<CopilotModelSnapshot, CopilotModelDiscoveryError> {
    let url = format!("{}/models", api_base.trim_end_matches('/'));
    let headers = copilot_inference_headers(config, token, CopilotRequestIntent::ModelAccess)
        .map_err(CopilotModelDiscoveryError::Header)?;
    let response = transport
        .get(&url, headers)
        .await
        .map_err(CopilotModelDiscoveryError::Transport)?;
    if !(200..=299).contains(&response.status) {
        return Err(CopilotModelDiscoveryError::Http {
            status: response.status,
            message: github_error_message(&response.body, token),
            retry_after: response.retry_after,
            rate_limited: response.rate_limited,
            rate_limit_reset: response.rate_limit_reset,
            account_action: github_account_action(&response.body),
        });
    }
    let body: CopilotModelsEnvelope =
        serde_json::from_slice(&response.body).map_err(CopilotModelDiscoveryError::Decode)?;
    CopilotModelSnapshot::from_models(body.data).map_err(CopilotModelDiscoveryError::Protocol)
}

/// The Copilot API request headers: `token` as the bearer plus the
/// configured client identity.
fn copilot_inference_headers(
    config: &CopilotBackendConfig,
    token: &str,
    intent: CopilotRequestIntent,
) -> Result<HeaderMap, ProviderAuthError> {
    let mut headers = HeaderMap::new();
    insert_authorization_header(&mut headers, &format!("{BEARER_SCHEME} {token}"))?;
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

/// The operator-token mode (`github_copilot_token`): the operator's GitHub
/// token is the Copilot API bearer. Its lifecycle lives in the account's
/// AuthMachine lease; GitHub's identity lookup names the account, its plan
/// and its API host.
impl CopilotAuthorizer {
    fn lease_key(&self) -> LeaseKey {
        LeaseKey::from_credential_identity(self.binding.credential_identity())
    }

    fn auth_lease(&self) -> Result<&GeneratedAuthLeaseHandle, ProviderAuthError> {
        self.env.auth_lease_handle.as_ref().ok_or_else(|| {
            ProviderAuthError::SourceResolutionFailed(
                "Copilot requires provider-auth persistence and AuthMachine authority".to_string(),
            )
        })
    }

    /// The fast path, with no I/O. A cache exists only after a flight
    /// published or restored the account lease, so without one the flight
    /// reads the source. With one the AuthMachine decides: `Authorized` uses
    /// the cache built for the current lease generation (a model list that
    /// is due is refreshed off the request path), and any other verdict has
    /// the flight read the source again (through the paced check for a
    /// source that runs something). `ReauthRequired` first drops the
    /// cache built for the refused publication: the refusal may have come
    /// from another account state on the same lease (another provider
    /// registry, another credential store), and this one must still read its
    /// source to heal when that source does.
    fn observe_operator_lease(&self) -> Result<OperatorObservation, ProviderAuthError> {
        let auth_lease = self.auth_lease()?;
        let lease_key = self.lease_key();
        if self.state.cache.read().is_none() {
            return Ok(OperatorObservation::Flight {
                generation: auth_lease.snapshot(&lease_key).generation,
            });
        }
        let now = (self.env.now)();
        let disposition = observe_credential_use(auth_lease, &lease_key, now)?;
        let generation = auth_lease.snapshot(&lease_key).generation;
        match disposition {
            CredentialUseDisposition::Authorized => {
                if let Some(cached) = self.fresh_cached(generation, now) {
                    return Ok(OperatorObservation::Cached { cached, generation });
                }
            }
            CredentialUseDisposition::ReauthRequired => {
                self.drop_cache_for_source_generation(generation);
            }
            CredentialUseDisposition::RefreshRequired
            | CredentialUseDisposition::RefreshDisallowed
            | CredentialUseDisposition::LeaseAbsent
            | CredentialUseDisposition::AlreadyRefreshing => {}
        }
        Ok(OperatorObservation::Flight { generation })
    }

    /// Drop the cached account built for the lease's publication
    /// `source_generation`, when that is what the cache holds.
    fn drop_cache_for_source_generation(&self, source_generation: u64) {
        let mut cache = self.state.cache.write();
        if cache
            .as_ref()
            .is_some_and(|cached| cached.source_generation == source_generation)
        {
            *cache = None;
        }
    }

    async fn refresh_operator_token_as_leader(&self) -> Result<CachedTokenView, ProviderAuthError> {
        let material = self.load_operator_token().await?;
        if let Some(cached) = self.fresh_cached(material.generation, (self.env.now)())
            && cached.token == material.token
        {
            // Another flight already built the account for this token.
            if cached.model_discovery_due {
                self.refresh_models_off_the_request_path(&cached, material.generation)
                    .await;
            }
            return Ok(cached);
        }
        self.discover_operator_account(material).await
    }

    /// Read the operator's GitHub token from the route's source, refuse a
    /// classic personal access token before any request, publish the token
    /// to the account lease when this account state is the lease's
    /// publisher, and take the AuthMachine's admission. While the lease
    /// withdraws the token (GitHub refused it, or a logout released it) a
    /// source that runs something is read again only at its paced instant
    /// ([`Self::claim_withdrawn_source_reread`]), and a request inside the
    /// pause gets the withdrawn token's own error. A read that the pause
    /// lets through is still not run while GitHub rate limits the account's
    /// identity lookup.
    async fn load_operator_token(&self) -> Result<OperatorTokenMaterial, ProviderAuthError> {
        let source = &self.binding.auth_profile().source;
        if let Some(withdrawn) = self.claim_withdrawn_source_reread()? {
            return Err(withdrawn);
        }
        // The identity lookup the token is read for could not run before
        // GitHub's stated time, so a command or a resolver is not run for it.
        let identity_retry_at = *self.state.pacing.identity_retry_at.lock();
        if source_reread_is_paced(source)
            && let Some(until) = identity_retry_at
            && (self.env.now)() < until
        {
            return Err(OperatorRefusal::RateLimited { until }.into_provider_error());
        }
        let (resolved, stored_generation) = match source {
            // A stored token names the publication it was stored under: its
            // record's durable marker. A store write publishes before it
            // saves, so a lease snapshot read after the value can already
            // carry a newer token's publication.
            CredentialSourceSpec::ManagedStore => {
                let managed =
                    load_managed_store_tokens_with_lifecycle(&self.env, &self.binding).await?;
                if managed.lifecycle == ManagedStoreLifecycle::RefreshRequired {
                    return Err(ProviderAuthError::Auth(AuthError::RefreshRequired));
                }
                let generation =
                    meerkat_core::tokens_lifecycle_published_generation(&managed.tokens);
                let secret = managed.tokens.primary_secret.ok_or_else(|| {
                    ProviderAuthError::SourceResolutionFailed(
                        "managed_store credential has no primary_secret".to_string(),
                    )
                })?;
                (secret, generation)
            }
            _ => (
                resolve_simple_secret(source, &self.env, &self.binding).await?,
                None,
            ),
        };
        let token = resolved.trim_matches(|character: char| character.is_ascii_whitespace());
        if token.is_empty() {
            return Err(ProviderAuthError::Auth(AuthError::MissingSecret));
        }
        let kind = GitHubTokenKind::classify(token);
        if kind == GitHubTokenKind::ClassicPersonalAccessToken {
            return Err(classic_personal_access_token_refusal());
        }
        let auth_lease = self.auth_lease()?;
        let lease_key = self.lease_key();
        if !simple_secret_source_publishes_lifecycle(source) {
            self.publish_operator_token(auth_lease, &lease_key, token, (self.env.now)())?;
        }
        match auth_lease
            .resolve_credential_use_admission(&lease_key, CredentialUseIntent::UseCredential)
            .map_err(credential_use_admission_error)?
        {
            CredentialUseDisposition::Authorized => {}
            CredentialUseDisposition::ReauthRequired => {
                // The source still gives a token the lease records as
                // refused: it is read again at the next paced instant.
                self.pace_withdrawn_source(auth_lease.snapshot(&lease_key).generation);
                return Err(ProviderAuthError::Auth(AuthError::UserReauthRequired));
            }
            CredentialUseDisposition::RefreshRequired
            | CredentialUseDisposition::RefreshDisallowed => {
                return Err(ProviderAuthError::Auth(AuthError::RefreshRequired));
            }
            CredentialUseDisposition::LeaseAbsent => {
                // The source still gives the token a logout released: it is
                // read again at the next paced instant.
                self.pace_withdrawn_source(auth_lease.snapshot(&lease_key).generation);
                return Err(ProviderAuthError::Auth(AuthError::LeaseAbsent));
            }
            CredentialUseDisposition::AlreadyRefreshing => {
                return Err(ProviderAuthError::Auth(AuthError::LeaseAbsent));
            }
        }
        // A refusal of the token names this generation: the stored record's
        // publication for a managed store, else the publication the lease
        // carries now (a source this route reads publishes the value it read
        // above, and the refusal also names the value's fingerprint).
        let publication = auth_lease.snapshot(&lease_key);
        Ok(OperatorTokenMaterial {
            token: token.to_string(),
            kind,
            generation: stored_generation.unwrap_or(publication.generation),
            expires_at: publication.expires_at,
        })
    }

    /// Publish `token` to the account lease unless the lease already carries
    /// it. The AuthMachine decides, from the token's fingerprint and its own
    /// state, fed the current time first so a declared freshness window that
    /// ran out reads as such: it publishes a value it does not record,
    /// renews the recorded value whose window ran out, and publishes nothing
    /// for the recorded value otherwise. A token GitHub refused therefore
    /// stays refused, and a logout stays in force, for every session and
    /// every machine that shares the lease, until the token value changes.
    fn publish_operator_token(
        &self,
        auth_lease: &GeneratedAuthLeaseHandle,
        lease_key: &LeaseKey,
        token: &str,
        now: DateTime<Utc>,
    ) -> Result<(), ProviderAuthError> {
        observe_credential_freshness(auth_lease, lease_key, now)?;
        auth_lease
            .publish_source_credential(
                lease_key,
                &CredentialFingerprint::of_secret(token),
                self.operator_lease_expiry(now),
            )
            .map_err(|error| {
                ProviderAuthError::SourceResolutionFailed(format!(
                    "AuthMachine credential publication failed: {error}"
                ))
            })?;
        Ok(())
    }

    /// The expiry an operator token is published with: `now` plus a `command`
    /// source's declared `refresh_interval_ms`, so the AuthMachine asks for a
    /// re-run before it; otherwise none (`u64::MAX`, the lease's encoding of
    /// no expiry), since GitHub states no expiry for these tokens.
    fn operator_lease_expiry(&self, now: DateTime<Utc>) -> u64 {
        match &self.binding.auth_profile().source {
            CredentialSourceSpec::Command {
                refresh_interval_ms: Some(interval_ms),
                ..
            } => epoch_secs(now).saturating_add(interval_ms / 1000),
            _ => u64::MAX,
        }
    }

    /// Build the account state for an admitted token: the identity lookup
    /// (skipped for an installation token), then the model list with the
    /// token as the Copilot API bearer.
    async fn discover_operator_account(
        &self,
        material: OperatorTokenMaterial,
    ) -> Result<CachedTokenView, ProviderAuthError> {
        let now = (self.env.now)();
        if material.kind != GitHubTokenKind::Installation {
            let retry_at = *self.state.pacing.identity_retry_at.lock();
            if let Some(until) = retry_at
                && now < until
            {
                return Err(OperatorRefusal::RateLimited { until }.into_provider_error());
            }
        }
        // Copilot refused this very token a moment ago, after GitHub's
        // identity lookup did not answer it: the refusal stands until the
        // pause ends, with no request.
        let token = CredentialFingerprint::of_secret(&material.token);
        if let Some(paused) = self.state.pacing.refused_token.lock().as_ref()
            && paused.token == token
            && now < paused.until
        {
            return Err(ProviderAuthError::Auth(AuthError::RefreshFailed(
                paused.detail.clone(),
            )));
        }
        let identity = match lookup_operator_identity(
            self.transport.as_ref(),
            &self.state.config,
            &material.token,
            material.kind,
            now,
        )
        .await
        {
            Ok(identity) => identity,
            Err(refusal) => {
                return Err(self.operator_refusal(refusal, &material.token, material.generation));
            }
        };
        *self.state.pacing.identity_retry_at.lock() = None;
        let models_result = self.fetch_models(&identity.api_base, &material.token).await;
        let discovery_now = (self.env.now)();
        if let Err(error) = &models_result
            && let Some(refusal) = models_refusal(error, identity.installation)
        {
            // A 401 or 403 from the Copilot API is Copilot refusing a user
            // token (no Copilot Requests permission, no seat, a policy, an
            // integration not allowed), also after a lookup that did not
            // answer it: that lookup authenticated the token, and only the
            // lookup's own 401 says it is revoked. Such a token is not asked
            // about again for a minute, so a refused account costs one
            // lookup and one listing a minute, not two requests per request.
            if !identity.looked_up && !identity.installation {
                *self.state.pacing.refused_token.lock() = Some(RefusedTokenPause {
                    token,
                    until: rate_limit_until(None, None, discovery_now),
                    detail: refusal.detail(),
                });
            }
            return Err(self.operator_refusal(refusal, &material.token, material.generation));
        }
        *self.state.pacing.refused_token.lock() = None;
        // A token GitHub's identity lookup did not answer is vouched for by
        // its model list alone, so it is used only once that list loads: a
        // listing that fails fails this request, and a rate limit pauses the
        // lookup until GitHub's stated time. An installation token keeps
        // serving without a list, as before.
        if !identity.looked_up
            && !identity.installation
            && let Err(error) = &models_result
        {
            if matches!(
                error,
                CopilotModelDiscoveryError::Http {
                    rate_limited: true,
                    ..
                }
            ) {
                *self.state.pacing.identity_retry_at.lock() =
                    Some(operator_models_retry_at(error, discovery_now));
            }
            return Err(ProviderAuthError::Auth(AuthError::RefreshFailed(format!(
                "GitHub Copilot model listing failed: {error}"
            ))));
        }
        let (models, model_discovery_refresh_at) = match models_result {
            Ok(snapshot) => (
                Some(Arc::new(snapshot)),
                Some(model_discovery_deadline(
                    discovery_now,
                    StdDuration::from_secs(MODEL_DISCOVERY_REFRESH_SECONDS),
                )),
            ),
            Err(error) => {
                tracing::warn!(
                    credential_identity = %self.binding.credential_identity(),
                    error = %error,
                    "GitHub Copilot accepted the operator token but account model discovery failed"
                );
                (None, Some(operator_models_retry_at(&error, discovery_now)))
            }
        };
        tracing::info!(
            credential_identity = %self.binding.credential_identity(),
            login = identity.login.as_deref().unwrap_or("none"),
            plan = identity.plan.as_deref().unwrap_or("none"),
            token_kind = material.kind.as_str(),
            source_kind = credential_source_kind(&self.binding.auth_profile().source),
            "GitHub Copilot accepted the operator token"
        );
        let identity = Arc::new(identity);
        let generation = self.state.next_generation.fetch_add(1, Ordering::Relaxed);
        let view = CachedTokenView {
            generation,
            token: material.token.clone(),
            api_base: identity.api_base.clone(),
            model_discovery_due: false,
            identity: Some(Arc::clone(&identity)),
        };
        // The account serves the lease publication the token was admitted
        // under, so it reports that publication's expiry: an agent build
        // that finds it equal to the lease's publishes nothing again, and a
        // command source runs at the interval it declares.
        let source_expires_at = material
            .expires_at
            .and_then(|expires_at| i64::try_from(expires_at).ok())
            .and_then(|expires_at| DateTime::<Utc>::from_timestamp(expires_at, 0));
        *self.state.cache.write() = Some(CachedCopilotToken {
            generation,
            token: material.token,
            source_generation: material.generation,
            source_expires_at,
            refresh_at: DateTime::<Utc>::MAX_UTC,
            expires_at: DateTime::<Utc>::MAX_UTC,
            api_base: identity.api_base.clone(),
            models,
            model_discovery_refresh_at,
            identity: Some(identity),
            exchange_unavailable: None,
        });
        Ok(view)
    }

    /// Publish GitHub's refusal of `token`, admitted at lease `generation`,
    /// to the account lease (`MarkPublicationRefused`), and return the lease
    /// generation the lease withdraws, or `None` when it carries a newer
    /// publication. The refusal names the publication the token was
    /// admitted under: that generation and, for a source this route
    /// publishes itself, the token's fingerprint (a managed store publishes
    /// without naming a value). The AuthMachine withdraws the lease only
    /// while it carries that publication, so a refusal that arrives after
    /// another account state published a newer token leaves the newer one
    /// valid. Its credential-use verdict after the refusal says which it
    /// was: `reauth_required` or `lease_absent` (this refusal, another
    /// account state's refusal of the same publication, or a logout)
    /// withdraws the token; any other verdict is a newer publication.
    fn mark_operator_token_refused(
        &self,
        token: &str,
        generation: u64,
    ) -> Result<Option<u64>, ProviderAuthError> {
        let auth_lease = self.auth_lease()?;
        let lease_key = self.lease_key();
        let fingerprint =
            (!simple_secret_source_publishes_lifecycle(&self.binding.auth_profile().source))
                .then(|| CredentialFingerprint::of_secret(token));
        auth_lease
            .mark_publication_refused(&lease_key, generation, fingerprint.as_ref())
            .map_err(|error| {
                ProviderAuthError::SourceResolutionFailed(format!(
                    "Copilot token refusal could not be published to AuthMachine: {error}"
                ))
            })?;
        match auth_lease
            .resolve_credential_use_admission(&lease_key, CredentialUseIntent::UseCredential)
            .map_err(credential_use_admission_error)?
        {
            CredentialUseDisposition::ReauthRequired | CredentialUseDisposition::LeaseAbsent => {
                Ok(Some(auth_lease.snapshot(&lease_key).generation))
            }
            CredentialUseDisposition::Authorized
            | CredentialUseDisposition::RefreshRequired
            | CredentialUseDisposition::RefreshDisallowed
            | CredentialUseDisposition::AlreadyRefreshing => Ok(None),
        }
    }

    /// Pace the next read of the source of a token the lease withdrew at
    /// `generation` (GitHub refused it, or a logout released it). A source
    /// that runs something to give the token (a command, a host resolver) is
    /// read again at most once per [`WITHDRAWN_TOKEN_SOURCE_REREAD_SECONDS`]
    /// while that stands. An environment variable or an inline secret is
    /// read on every request: reading it costs nothing, and a new value
    /// heals at once.
    fn pace_withdrawn_source(&self, generation: u64) {
        if !source_reread_is_paced(&self.binding.auth_profile().source) {
            return;
        }
        let Ok(auth_lease) = self.auth_lease() else {
            return;
        };
        self.state.pacing.withdrawn_source_rereads.lock().insert(
            auth_lease.auth_lifecycle_restore_instance_id(),
            WithdrawnSourceReread {
                generation,
                at: withdrawn_source_reread_at((self.env.now)()),
            },
        );
    }

    /// Claim a read of the route's source while the lease withdraws the
    /// token it gave (GitHub refused it, or a logout released it). Returns
    /// the error the request gets instead of the read while the read must
    /// wait: the source runs something, the AuthMachine answers
    /// `reauth_required` or `lease_absent` for a token that was published
    /// (generation above 0), and a pause at that generation has not reached
    /// its instant. Otherwise, while the lease withdraws the token, this read
    /// starts the pause (or, at the paced instant, moves it a pause further)
    /// before the source runs, so the first read after a withdrawal this
    /// runtime did not see (a logout, another runtime's refusal) is paced
    /// too, and a read that fails (a command that exits non-zero or hangs to
    /// its timeout, a resolver error, an empty or a classic token) is paced
    /// as a read that gives the withdrawn token again is. A new publication
    /// on the lease ends the pause at once, so a value another session,
    /// machine or login published heals this account state on its next
    /// request. The pause belongs to the lease's AuthMachine registry, and is
    /// checked and moved under its own lock, so of this runtime's account
    /// states over one registry only one reads at each paced instant.
    fn claim_withdrawn_source_reread(
        &self,
    ) -> Result<Option<ProviderAuthError>, ProviderAuthError> {
        if !source_reread_is_paced(&self.binding.auth_profile().source) {
            return Ok(None);
        }
        let auth_lease = self.auth_lease()?;
        let registry = auth_lease.auth_lifecycle_restore_instance_id();
        let lease_key = self.lease_key();
        let mut pauses = self.state.pacing.withdrawn_source_rereads.lock();
        let generation = auth_lease.snapshot(&lease_key).generation;
        let withdrawn = withdrawn_token_error(
            auth_lease
                .resolve_credential_use_admission(&lease_key, CredentialUseIntent::UseCredential)
                .map_err(credential_use_admission_error)?,
        )
        .filter(|_| generation > 0);
        let Some(withdrawn) = withdrawn else {
            pauses.remove(&registry);
            return Ok(None);
        };
        let now = (self.env.now)();
        if pauses
            .get(&registry)
            .is_some_and(|paced| paced.generation == generation && now < paced.at)
        {
            return Ok(Some(withdrawn));
        }
        pauses.insert(
            registry,
            WithdrawnSourceReread {
                generation,
                at: withdrawn_source_reread_at(now),
            },
        );
        Ok(None)
    }

    /// Apply a refusal of `token`, admitted at lease `generation`: a token
    /// GitHub refused is published to the account lease as the refusal of
    /// that publication (`MarkPublicationRefused`), the account state built
    /// for an earlier token is dropped and, when the lease withdraws the
    /// token, a source that runs something is paced; a refusal of a token
    /// the lease no longer carries (a newer one was published since) fails
    /// this request as `stale_credential` and leaves the newer token valid.
    /// A rate limit pauses the identity lookup until GitHub's stated time.
    /// Logged without the token.
    fn operator_refusal(
        &self,
        refusal: OperatorRefusal,
        token: &str,
        generation: u64,
    ) -> ProviderAuthError {
        tracing::warn!(
            credential_identity = %self.binding.credential_identity(),
            refusal = %refusal.detail(),
            "GitHub Copilot did not accept the operator token"
        );
        match &refusal {
            OperatorRefusal::TokenRejected { .. } => {
                // The cached account belongs to a token the lease no longer
                // carries. Without it the next call reads the source again
                // (a command or a resolver at its paced instant): the same
                // value fails fast (the AuthMachine records it as refused)
                // and a new value, such as the next token a command mints, is
                // published and heals the account.
                *self.state.cache.write() = None;
                match self.mark_operator_token_refused(token, generation) {
                    Ok(Some(withdrawn)) => self.pace_withdrawn_source(withdrawn),
                    // The lease carries a newer publication, which the
                    // refusal leaves valid: only this request's token is
                    // stale, and the next request reads the source again.
                    Ok(None) => return ProviderAuthError::Auth(AuthError::StaleCredential),
                    Err(error) => return error,
                }
            }
            OperatorRefusal::RateLimited { until } => {
                *self.state.pacing.identity_retry_at.lock() = Some(*until);
            }
            OperatorRefusal::Refused(_) => {}
        }
        refusal.into_provider_error()
    }
}

#[async_trait]
impl HttpAuthorizer for CopilotAuthorizer {
    async fn prepare_request(&self) -> Result<(), AuthError> {
        self.ensure_token()
            .await
            .map(|_| ())
            .map_err(|error| auth_error_from_provider(error.into_provider()))
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
            self.refuse_derived_token_generation(generation);
            return Ok(HttpAuthorizationResponseAction::RetryWithFreshAuthorization);
        }
        Ok(HttpAuthorizationResponseAction::Propagate)
    }

    /// Every bearer this account state signed requests with (the operator's
    /// GitHub token in token mode, a rotated-out one included) is redacted.
    fn redact_response_diagnostic(&self, diagnostic: &mut String) {
        self.state.redact_signed_bearers(diagnostic);
    }

    #[allow(clippy::unnecessary_literal_bound)]
    fn label(&self) -> &str {
        crate::GITHUB_COPILOT_AUTHORIZER_LABEL
    }

    /// `Copilot-Vision-Request` on a request with images, and `X-Initiator`:
    /// `agent` when the request continues the agent's own work (its last
    /// input is a tool result or the assistant's own output, or it is a
    /// request-local call such as a compaction summary), `user` otherwise.
    fn append_content_headers(
        &self,
        content: meerkat_core::HttpAuthorizationContent,
        headers: &mut Vec<(String, String)>,
    ) -> Result<(), AuthError> {
        if content.has_images {
            headers.push(("Copilot-Vision-Request".to_string(), "true".to_string()));
        }
        let initiator = match content.initiator {
            meerkat_core::HttpRequestInitiator::User => "user",
            meerkat_core::HttpRequestInitiator::Agent => "agent",
        };
        headers.push(("X-Initiator".to_string(), initiator.to_string()));
        Ok(())
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
    /// Start the account's due model discovery for `cached`, the token of
    /// source generation `source_generation`, off the request path. One
    /// request claims it (the next discovery then waits
    /// [`MODEL_DISCOVERY_CLAIM_SECONDS`] while this one runs), and every
    /// request goes on with the token and the model list the account holds
    /// ([`BackgroundModelDiscovery`]). Without a Tokio runtime to run on, the
    /// discovery runs before the request goes on.
    async fn refresh_models_off_the_request_path(
        &self,
        cached: &CachedTokenView,
        source_generation: u64,
    ) {
        let now = (self.env.now)();
        {
            let mut cache = self.state.cache.write();
            let Some(current) = cache.as_mut() else {
                return;
            };
            if current.source_generation != source_generation
                || current.token != cached.token
                || current
                    .model_discovery_refresh_at
                    .is_none_or(|refresh_at| now < refresh_at)
            {
                return;
            }
            current.model_discovery_refresh_at = Some(model_discovery_deadline(
                now,
                StdDuration::from_secs(MODEL_DISCOVERY_CLAIM_SECONDS),
            ));
        }
        let discovery = BackgroundModelDiscovery {
            state: Arc::clone(&self.state),
            transport: Arc::clone(&self.transport),
            env: self.env.clone(),
            credential_identity: self.binding.credential_identity().clone(),
            source_generation,
            token: cached.token.clone(),
            api_base: cached.api_base.clone(),
            installation: cached
                .identity
                .as_deref()
                .is_some_and(|identity| identity.installation),
        }
        .run();
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => std::mem::drop(runtime.spawn(discovery)),
            Err(_) => discovery.await,
        }
    }

    async fn authorize_attempt(
        &self,
        request: &mut HttpAuthorizationRequest<'_>,
    ) -> Result<u64, AuthError> {
        let token = self
            .ensure_token()
            .await
            .map_err(|error| auth_error_from_provider(error.into_provider()))?;
        if !url_is_within_api_base(request.url, &token.api_base) {
            return Err(AuthError::Other(format!(
                "Copilot authorizer refused URL outside resolved API base: {}",
                request.url
            )));
        }
        self.state.record_signed_bearer(&token.token);
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
    if let Some(access) = error.credential_access_error() {
        return access;
    }
    match error {
        RefreshError::StalePreparation => AuthError::StaleCredential,
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
    identity: Option<Arc<CopilotAccountIdentity>>,
}

/// One refresh of an account's model list that runs off the request path,
/// for the token a request found fresh. Its answer applies only while the
/// account still holds that token: a newer token's own mint listed its
/// models.
///
/// A list replaces the account's, and the next refresh is due five minutes
/// later. A listing that refuses the token has the next request authorize
/// again: a device sign-in's derived token is refused, so that request mints
/// a new one, whose own listing decides (a 401 again, or a 403 that asks for
/// an account action, needs a new sign-in); a token route's account is
/// dropped, so that request reads the token and looks the account up again,
/// and the lookup decides. Any other failure leaves the account's list
/// serving and schedules the next try.
struct BackgroundModelDiscovery {
    state: Arc<CopilotAccountState>,
    transport: Arc<dyn CopilotHttpTransport>,
    env: ResolverEnvironment,
    credential_identity: AuthCredentialIdentity,
    source_generation: u64,
    token: String,
    api_base: String,
    /// Token mode: the token is a GitHub App installation token.
    installation: bool,
}

impl BackgroundModelDiscovery {
    async fn run(self) {
        let models = fetch_copilot_models(
            self.transport.as_ref(),
            &self.state.config,
            &self.api_base,
            &self.token,
        )
        .await;
        let now = (self.env.now)();
        let mut cache = self.state.cache.write();
        let current = match cache.as_mut() {
            Some(current)
                if current.source_generation == self.source_generation
                    && current.token == self.token =>
            {
                current
            }
            _ => return,
        };
        let error = match models {
            Ok(snapshot) => {
                current.models = Some(Arc::new(snapshot));
                current.model_discovery_refresh_at = Some(model_discovery_deadline(
                    now,
                    StdDuration::from_secs(MODEL_DISCOVERY_REFRESH_SECONDS),
                ));
                return;
            }
            Err(error) => error,
        };
        let refused = match self.state.mode {
            CopilotCredentialMode::DeviceSignIn => error.refuses_derived_token(),
            CopilotCredentialMode::OperatorToken => {
                models_refusal(&error, self.installation).is_some()
            }
        };
        if !refused {
            tracing::warn!(
                credential_identity = %self.credential_identity,
                error = %error,
                "Copilot account model discovery failed; the account's last model list keeps serving"
            );
            current.model_discovery_refresh_at = Some(match self.state.mode {
                CopilotCredentialMode::DeviceSignIn => {
                    model_discovery_deadline(now, error.retry_delay())
                }
                CopilotCredentialMode::OperatorToken => operator_models_retry_at(&error, now),
            });
            return;
        }
        tracing::warn!(
            credential_identity = %self.credential_identity,
            error = %error,
            "Copilot refused the account's token for its model list; the next request authorizes again"
        );
        match self.state.mode {
            CopilotCredentialMode::DeviceSignIn => current.refuse(),
            CopilotCredentialMode::OperatorToken => *cache = None,
        }
    }
}

/// The work an elected leader runs, by the account's credential mode.
enum LeaderTask {
    /// Device sign-in: derive a Copilot token from this stored GitHub token.
    Device(PersistedTokens),
    /// Operator token: read the source token again.
    Operator,
}

/// What one pass of the operator-token fast path decided.
enum OperatorObservation {
    /// The cache is admitted for the current lease generation `generation`
    /// and its token is fresh.
    Cached {
        cached: CachedTokenView,
        generation: u64,
    },
    /// A flight must read the source token for the lease generation
    /// observed.
    Flight { generation: u64 },
}

/// An operator token read from its source and admitted by the AuthMachine.
/// Deliberately not `Debug`: it holds the token.
struct OperatorTokenMaterial {
    token: String,
    kind: GitHubTokenKind,
    /// The account lease generation the token was admitted under.
    generation: u64,
    /// The expiry that publication declares (epoch seconds), read with its
    /// generation.
    expires_at: Option<u64>,
}

/// Why GitHub or Copilot did not accept an operator token.
enum OperatorRefusal {
    /// GitHub refused the token itself: revoked, expired or not a valid
    /// token. A credential fact, published to the AuthMachine lease.
    TokenRejected { status: u16, message: String },
    /// GitHub or Copilot refused this identity (no seat, no Copilot Requests
    /// permission, an organization policy), or answered unusably. Not a
    /// credential fact: the same token can work once that is fixed.
    Refused(String),
    /// GitHub rate limited the identity lookup until `until`.
    RateLimited { until: DateTime<Utc> },
}

impl OperatorRefusal {
    fn detail(&self) -> String {
        match self {
            Self::TokenRejected { status, message } => {
                format!("GitHub refused this token ({status}): {message}")
            }
            Self::Refused(detail) => detail.clone(),
            Self::RateLimited { until } => format!(
                "GitHub rate limited the Copilot identity lookup until {}",
                until.to_rfc3339()
            ),
        }
    }

    fn into_provider_error(self) -> ProviderAuthError {
        match self {
            Self::TokenRejected { .. } => ProviderAuthError::Auth(AuthError::UserReauthRequired),
            Self::Refused(detail) => ProviderAuthError::Auth(AuthError::RefreshFailed(detail)),
            refusal @ Self::RateLimited { .. } => {
                ProviderAuthError::Auth(AuthError::RefreshFailed(refusal.detail()))
            }
        }
    }
}

/// Look up the Copilot identity of an operator token: `GET
/// copilot_internal/user` with the token as the bearer. An installation token
/// has no user, so it skips the lookup and uses the default Copilot API host,
/// as does a token the lookup does not answer (403 or 404): the caller then
/// requires the account's model list there before it uses the token.
async fn lookup_operator_identity(
    transport: &dyn CopilotHttpTransport,
    config: &CopilotBackendConfig,
    token: &str,
    kind: GitHubTokenKind,
    now: DateTime<Utc>,
) -> Result<CopilotAccountIdentity, OperatorRefusal> {
    if kind == GitHubTokenKind::Installation {
        return Ok(CopilotAccountIdentity {
            login: None,
            plan: None,
            api_base: operator_api_base(None)?,
            installation: true,
            looked_up: false,
        });
    }
    let url = copilot_user_url().map_err(|error| OperatorRefusal::Refused(error.to_string()))?;
    let headers = copilot_user_headers(config, token)
        .map_err(|error| OperatorRefusal::Refused(error.to_string()))?;
    let response = transport
        .get(url.as_str(), headers)
        .await
        .map_err(|error| {
            OperatorRefusal::Refused(format!("GitHub Copilot identity lookup failed: {error}"))
        })?;
    if response.rate_limited {
        return Err(OperatorRefusal::RateLimited {
            until: rate_limit_until(response.retry_after, response.rate_limit_reset, now),
        });
    }
    match response.status {
        200..=299 => {}
        401 => {
            return Err(OperatorRefusal::TokenRejected {
                status: 401,
                message: github_error_message(&response.body, token),
            });
        }
        // The lookup is an internal GitHub endpoint, and it may not answer a
        // token the Copilot API accepts: a fine-grained personal access
        // token can get a 403 or a 404 from it. The token is then tried on
        // the default Copilot API host, unnamed, and used only once the
        // account's model list loads there; a 401 or 403 from that listing
        // is Copilot refusing the token, as for a looked-up token, and only
        // this lookup's own 401 says that the token is revoked. A rate limit
        // was answered above.
        status @ (403 | 404) => {
            tracing::warn!(
                status,
                github_message = %github_error_message(&response.body, token),
                "GitHub's Copilot identity lookup did not answer the operator token; \
                 trying the default Copilot API host"
            );
            return Ok(CopilotAccountIdentity {
                login: None,
                plan: None,
                api_base: operator_api_base(None)?,
                installation: false,
                looked_up: false,
            });
        }
        status @ 400..=499 => {
            return Err(OperatorRefusal::Refused(format!(
                "GitHub Copilot refused this token ({status}): {}",
                github_error_message(&response.body, token)
            )));
        }
        status => {
            return Err(OperatorRefusal::Refused(format!(
                "GitHub Copilot identity lookup failed ({status}): {}",
                github_error_message(&response.body, token)
            )));
        }
    }
    let user: CopilotUserEnvelope = serde_json::from_slice(&response.body).map_err(|error| {
        OperatorRefusal::Refused(format!("invalid GitHub Copilot identity response: {error}"))
    })?;
    if user.chat_enabled == Some(false) {
        return Err(OperatorRefusal::Refused(
            "GitHub Copilot refused this token: Copilot chat is not enabled for this account"
                .to_string(),
        ));
    }
    let api_base = operator_api_base(
        user.endpoints
            .as_ref()
            .and_then(|endpoints| endpoints.api.as_deref()),
    )?;
    Ok(CopilotAccountIdentity {
        login: user.login,
        plan: user.copilot_plan,
        api_base,
        installation: false,
        looked_up: true,
    })
}

/// Headers of the identity lookup: the GitHub token as the bearer, GitHub's
/// API version and the configured User-Agent. No `Copilot-Integration-Id`:
/// nothing establishes that `api.github.com` reads one, so the lookup sends
/// none, as before.
fn copilot_user_headers(
    config: &CopilotBackendConfig,
    token: &str,
) -> Result<HeaderMap, ProviderAuthError> {
    let mut headers = HeaderMap::new();
    insert_authorization_header(&mut headers, &format!("{BEARER_SCHEME} {token}"))?;
    insert_header(&mut headers, "Accept", "application/json")?;
    insert_header(
        &mut headers,
        "X-GitHub-Api-Version",
        &config.token_api_version,
    )?;
    insert_header(&mut headers, "User-Agent", &config.user_agent)?;
    Ok(headers)
}

/// The refusal a model listing answer is for an operator token: a 401 or 403
/// is Copilot refusing the token, and for an installation token, which has no
/// identity lookup, a 401 is GitHub refusing the token itself. A user token
/// the lookup did not answer is a user token: the Copilot API also answers
/// 401 for a valid token it does not serve. A rate limit (a 403 with
/// `Retry-After`, an exhausted primary limit or a secondary limit) is no
/// refusal: the listing waits for GitHub's stated time
/// ([`operator_models_retry_at`]). Any other failure is transient and the
/// listing is retried later.
fn models_refusal(
    error: &CopilotModelDiscoveryError,
    installation: bool,
) -> Option<OperatorRefusal> {
    let CopilotModelDiscoveryError::Http {
        status,
        message,
        rate_limited,
        ..
    } = error
    else {
        return None;
    };
    match *status {
        _ if *rate_limited => None,
        401 if installation => Some(OperatorRefusal::TokenRejected {
            status: 401,
            message: message.clone(),
        }),
        401 | 403 => Some(OperatorRefusal::Refused(format!(
            "GitHub Copilot refused this token ({status}): {message}"
        ))),
        _ => None,
    }
}

/// The Copilot API host an operator token is sent to: GitHub's per-plan
/// `endpoints.api`, else the default host. It must be an `https`
/// `githubcopilot.com` host without user information, since the bearer is a
/// long-lived GitHub credential.
fn operator_api_base(endpoint: Option<&str>) -> Result<String, OperatorRefusal> {
    let candidate = endpoint
        .map(str::trim)
        .filter(|endpoint| !endpoint.is_empty())
        .unwrap_or(DEFAULT_COPILOT_API_BASE);
    let url = parse_api_base(candidate).map_err(OperatorRefusal::Refused)?;
    let host = url.host_str().unwrap_or_default();
    if host != COPILOT_API_DOMAIN && !host.ends_with(COPILOT_API_HOST_SUFFIX) {
        return Err(OperatorRefusal::Refused(format!(
            "GitHub Copilot named the API host '{host}', which is outside \
             {COPILOT_API_DOMAIN}; the GitHub token is not sent there"
        )));
    }
    Ok(candidate.trim_end_matches('/').to_string())
}

/// GitHub's error `message` from a response body, with control characters
/// removed, `secret` (the credential the request carried, which a proxy or
/// gateway can echo) redacted, and at most [`GITHUB_ERROR_MESSAGE_MAX_CHARS`]
/// characters. Never the raw body. The secret is redacted before the cut, so
/// no prefix of it survives.
fn github_error_message(body: &[u8], secret: &str) -> String {
    let mut message: String = serde_json::from_slice::<GitHubErrorEnvelope>(body)
        .ok()
        .and_then(|envelope| envelope.message)
        .unwrap_or_default()
        .chars()
        .filter(|character| !character.is_control())
        .collect();
    redact_secret(&mut message, secret);
    let message: String = message
        .chars()
        .take(GITHUB_ERROR_MESSAGE_MAX_CHARS)
        .collect();
    let message = message.trim();
    if message.is_empty() {
        "no message".to_string()
    } else {
        message.to_string()
    }
}

/// Replace every occurrence of `secret` in `text` with the redaction marker.
fn redact_secret(text: &mut String, secret: &str) {
    if !secret.is_empty() && text.contains(secret) {
        *text = text.replace(secret, meerkat_core::redact::REDACTED);
    }
}

/// When a rate-limited GitHub request may be retried: `Retry-After`, else the
/// primary limit's `x-ratelimit-reset`, else a minute, clamped to between a
/// minute and an hour.
fn rate_limit_until(
    retry_after: Option<StdDuration>,
    rate_limit_reset: Option<u64>,
    now: DateTime<Utc>,
) -> DateTime<Utc> {
    let delay = retry_after
        .map(|delay| delay.as_secs())
        .or_else(|| rate_limit_reset.map(|reset| reset.saturating_sub(epoch_secs(now))))
        .unwrap_or(GITHUB_RATE_LIMIT_MIN_SECONDS)
        .clamp(GITHUB_RATE_LIMIT_MIN_SECONDS, GITHUB_RATE_LIMIT_MAX_SECONDS);
    i64::try_from(delay)
        .ok()
        .and_then(Duration::try_seconds)
        .and_then(|delay| now.checked_add_signed(delay))
        .unwrap_or(DateTime::<Utc>::MAX_UTC)
}

/// When an operator-token account lists its models again after `error`: a
/// rate limit (a 429, or a 403 whose primary limit is exhausted or that
/// carries `Retry-After`) waits for GitHub's stated time, as the identity
/// lookup does; any other failure retries as model discovery does.
fn operator_models_retry_at(
    error: &CopilotModelDiscoveryError,
    now: DateTime<Utc>,
) -> DateTime<Utc> {
    match error {
        CopilotModelDiscoveryError::Http {
            rate_limited: true,
            retry_after,
            rate_limit_reset,
            ..
        } => rate_limit_until(*retry_after, *rate_limit_reset, now),
        _ => model_discovery_deadline(now, error.retry_delay()),
    }
}

/// Whether reading `source` runs something to give the token (a command, a
/// host resolver), so its re-reads while the lease withdraws the token are
/// paced.
fn source_reread_is_paced(source: &CredentialSourceSpec) -> bool {
    matches!(
        source,
        CredentialSourceSpec::Command { .. } | CredentialSourceSpec::ExternalResolver { .. }
    )
}

/// The error a request gets while the lease withdraws its token, as the
/// AuthMachine's credential-use verdict names it: GitHub refused the token
/// (`reauth_required`), or a logout released it (`lease_absent`). `None` for
/// a verdict that withdraws nothing.
fn withdrawn_token_error(disposition: CredentialUseDisposition) -> Option<ProviderAuthError> {
    match disposition {
        CredentialUseDisposition::ReauthRequired => {
            Some(ProviderAuthError::Auth(AuthError::UserReauthRequired))
        }
        CredentialUseDisposition::LeaseAbsent => {
            Some(ProviderAuthError::Auth(AuthError::LeaseAbsent))
        }
        CredentialUseDisposition::Authorized
        | CredentialUseDisposition::RefreshRequired
        | CredentialUseDisposition::RefreshDisallowed
        | CredentialUseDisposition::AlreadyRefreshing => None,
    }
}

/// The instant a withdrawn token's paced source is read again.
fn withdrawn_source_reread_at(now: DateTime<Utc>) -> DateTime<Utc> {
    i64::try_from(WITHDRAWN_TOKEN_SOURCE_REREAD_SECONDS)
        .ok()
        .and_then(Duration::try_seconds)
        .and_then(|delay| now.checked_add_signed(delay))
        .unwrap_or(DateTime::<Utc>::MAX_UTC)
}

/// A classic personal access token is refused before any request: Copilot
/// does not accept it.
fn classic_personal_access_token_refusal() -> ProviderAuthError {
    ProviderAuthError::Auth(AuthError::UnsupportedCombination {
        backend: meerkat_core::provider_matrix::OpenAiBackendKind::Copilot
            .as_str()
            .to_string(),
        auth: GitHubTokenKind::ClassicPersonalAccessToken
            .as_str()
            .to_string(),
    })
}

/// The `auth` an operator-token route's command source is refused with when
/// its `refresh_interval_ms` ends inside the AuthMachine's refresh window.
const COMMAND_REFRESH_INTERVAL_WITHIN_REFRESH_WINDOW: &str =
    "command_refresh_interval_within_refresh_window";

/// Whether a command source's `refresh_interval_ms` gives its token a
/// lifetime that already ends inside the AuthMachine's refresh window when
/// it is published. The lease counts the lifetime in whole seconds
/// (`CopilotAuthorizer::operator_lease_expiry`), so 60001 to 60999 ms is the
/// same one-minute lifetime as 60000: the first interval that outlasts the
/// window is 61000 ms.
fn command_lifetime_within_refresh_window(interval_ms: u64) -> bool {
    interval_ms / 1000 <= AUTH_LEASE_TTL_REFRESH_WINDOW_SECS
}

/// A command source whose declared lifetime ends inside the AuthMachine's
/// refresh window is refused before it runs, typed as the classic-token
/// refusal is (`unsupported_combination`): its token would be due for a
/// re-run as soon as it is published, so the command would run, and the
/// account be looked up again, on every request.
fn short_command_refresh_interval_refusal() -> ProviderAuthError {
    ProviderAuthError::Auth(AuthError::UnsupportedCombination {
        backend: meerkat_core::provider_matrix::OpenAiBackendKind::Copilot
            .as_str()
            .to_string(),
        auth: COMMAND_REFRESH_INTERVAL_WITHIN_REFRESH_WINDOW.to_string(),
    })
}

/// Feed the AuthMachine the current time, so a declared freshness window that
/// ran out moves the lease to `Expiring` or `Expired`.
fn observe_credential_freshness(
    auth_lease: &GeneratedAuthLeaseHandle,
    lease_key: &LeaseKey,
    now: DateTime<Utc>,
) -> Result<(), ProviderAuthError> {
    auth_lease
        .observe_credential_freshness(
            lease_key,
            epoch_secs(now),
            AUTH_LEASE_TTL_REFRESH_WINDOW_SECS,
        )
        .map_err(|error| {
            ProviderAuthError::SourceResolutionFailed(format!(
                "AuthMachine lifecycle freshness observation failed: {error}"
            ))
        })
}

/// Feed the AuthMachine the current time, then take its credential-use
/// verdict. The machine decides; this never inspects the lease phase.
fn observe_credential_use(
    auth_lease: &GeneratedAuthLeaseHandle,
    lease_key: &LeaseKey,
    now: DateTime<Utc>,
) -> Result<CredentialUseDisposition, ProviderAuthError> {
    observe_credential_freshness(auth_lease, lease_key, now)?;
    auth_lease
        .resolve_credential_use_admission(lease_key, CredentialUseIntent::UseCredential)
        .map_err(credential_use_admission_error)
}

fn credential_use_admission_error(error: DslTransitionError) -> ProviderAuthError {
    ProviderAuthError::SourceResolutionFailed(format!(
        "AuthMachine credential-use admission classification failed: {error}"
    ))
}

fn epoch_secs(now: DateTime<Utc>) -> u64 {
    now.timestamp().max(0) as u64
}

/// The kind label of a credential source, for logs.
fn credential_source_kind(source: &CredentialSourceSpec) -> &'static str {
    match source {
        CredentialSourceSpec::InlineSecret { .. } => "inline_secret",
        CredentialSourceSpec::ManagedStore => "managed_store",
        CredentialSourceSpec::Env { .. } => "env",
        CredentialSourceSpec::ExternalResolver { .. } => "external_resolver",
        CredentialSourceSpec::PlatformDefault => "platform_default",
        CredentialSourceSpec::Command { .. } => "command",
        CredentialSourceSpec::FileDescriptor { .. } => "file_descriptor",
    }
}

/// Check a GitHub token the way a `github_copilot_token` route uses it,
/// without AuthMachine, token-store or cache effects: refuse a classic
/// personal access token, look up the Copilot identity (an installation token
/// has none and uses the default API host, as does a token the lookup does
/// not answer), and list the account's models with the token as the Copilot
/// API bearer, which must succeed. A host calls it before it stores
/// a token, so a token GitHub or Copilot refuses is never stored. The token
/// goes only to `api.github.com` and a `githubcopilot.com` API host, and no
/// error carries it. `config` is the token routes' configuration,
/// [`CopilotBackendConfig::from_token_options`] of their backend options, so
/// the check sends the integration id the routes send.
pub async fn verify_operator_token(
    token: &str,
    config: &CopilotBackendConfig,
) -> Result<CopilotAccountIdentity, ProviderAuthError> {
    verify_operator_token_with(
        &ReqwestCopilotHttpTransport::new(),
        token,
        config,
        Utc::now(),
    )
    .await
}

async fn verify_operator_token_with(
    transport: &dyn CopilotHttpTransport,
    token: &str,
    config: &CopilotBackendConfig,
    now: DateTime<Utc>,
) -> Result<CopilotAccountIdentity, ProviderAuthError> {
    let token = token.trim_matches(|character: char| character.is_ascii_whitespace());
    if token.is_empty() {
        return Err(ProviderAuthError::Auth(AuthError::MissingSecret));
    }
    let kind = GitHubTokenKind::classify(token);
    if kind == GitHubTokenKind::ClassicPersonalAccessToken {
        return Err(classic_personal_access_token_refusal());
    }
    let identity = lookup_operator_identity(transport, config, token, kind, now)
        .await
        .map_err(OperatorRefusal::into_provider_error)?;
    match fetch_copilot_models(transport, config, &identity.api_base, token).await {
        Ok(_) => Ok(identity),
        Err(error) => Err(models_refusal(&error, identity.installation).map_or_else(
            || {
                ProviderAuthError::Auth(AuthError::RefreshFailed(format!(
                    "GitHub Copilot model listing failed: {error}"
                )))
            },
            OperatorRefusal::into_provider_error,
        )),
    }
}

/// A Copilot API base: an `https` URL without user information.
fn parse_api_base(candidate: &str) -> Result<url::Url, String> {
    let url = url::Url::parse(candidate)
        .map_err(|error| format!("invalid Copilot API endpoint: {error}"))?;
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
        return Err("Copilot API endpoint must be HTTPS without user information".to_string());
    }
    Ok(url)
}

fn resolve_api_base(token: &CopilotTokenEnvelope) -> Result<String, ProviderAuthError> {
    let candidate = token
        .endpoints
        .api
        .clone()
        .or_else(|| api_base_from_proxy_token(&token.token))
        .unwrap_or_else(|| DEFAULT_COPILOT_API_BASE.to_string());
    parse_api_base(&candidate).map_err(ProviderAuthError::SourceResolutionFailed)?;
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
    status == 401 || (status == 403 && github_account_action(body))
}

/// Whether a GitHub answer asks for an action on the account: it names an
/// `error_details.notification_id`.
fn github_account_action(body: &[u8]) -> bool {
    serde_json::from_slice::<CopilotTokenErrorEnvelope>(body)
        .unwrap_or_default()
        .error_details
        .and_then(|details| details.notification_id)
        .is_some()
}

fn token_expires_at(token: &CopilotTokenEnvelope) -> Result<DateTime<Utc>, ProviderAuthError> {
    i64::try_from(token.expires_at)
        .ok()
        .and_then(|timestamp| DateTime::<Utc>::from_timestamp(timestamp, 0))
        .ok_or_else(|| {
            ProviderAuthError::SourceResolutionFailed(
                "Copilot token expiry is out of range".to_string(),
            )
        })
}

fn token_refresh_at(
    token: &CopilotTokenEnvelope,
    now: DateTime<Utc>,
) -> Result<DateTime<Utc>, ProviderAuthError> {
    let expires_at = token_expires_at(token)?;
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

fn model_discovery_deadline(now: DateTime<Utc>, delay: StdDuration) -> DateTime<Utc> {
    let seconds = delay.as_secs().min(MODEL_DISCOVERY_MAX_RETRY_SECONDS);
    let Ok(seconds) = i64::try_from(seconds) else {
        return DateTime::<Utc>::MAX_UTC;
    };
    let Some(delta) = Duration::try_seconds(seconds) else {
        return DateTime::<Utc>::MAX_UTC;
    };
    now.checked_add_signed(delta)
        .unwrap_or(DateTime::<Utc>::MAX_UTC)
}

/// Insert the `Authorization` header, its value (the credential) marked
/// sensitive so the HTTP stack keeps it out of HPACK tables and its own
/// `Debug` output.
fn insert_authorization_header(
    headers: &mut HeaderMap,
    value: &str,
) -> Result<(), ProviderAuthError> {
    let mut value = HeaderValue::from_str(value).map_err(|error| {
        ProviderAuthError::SourceResolutionFailed(format!(
            "invalid Copilot header {}: {error}",
            reqwest::header::AUTHORIZATION
        ))
    })?;
    value.set_sensitive(true);
    headers.insert(reqwest::header::AUTHORIZATION, value);
    Ok(())
}

/// When to try the token exchange again after it failed transiently while
/// the current token still serves: after the exchange's `Retry-After`, or a
/// short delay without one, never sooner than a second (a `Retry-After: 0`
/// cannot switch the wait off) and no later than the token's expiry.
fn exchange_retry_at(
    now: DateTime<Utc>,
    retry_after: Option<StdDuration>,
    expires_at: DateTime<Utc>,
) -> DateTime<Utc> {
    let delay = retry_after
        .unwrap_or(StdDuration::from_secs(TOKEN_EXCHANGE_RETRY_SECONDS))
        .max(StdDuration::from_secs(TOKEN_EXCHANGE_MIN_RETRY_SECONDS));
    Duration::from_std(delay)
        .ok()
        .and_then(|delay| now.checked_add_signed(delay))
        .map_or(expires_at, |retry_at| retry_at.min(expires_at))
}

/// The instant before which a recorded exchange outage answers in place of
/// the exchange: `retry_after` (the exchange's own `Retry-After`) from `now`,
/// at least [`TOKEN_EXCHANGE_MIN_RETRY_SECONDS`] and at most
/// [`TOKEN_EXCHANGE_MAX_RETRY_SECONDS`].
fn exchange_not_before(now: DateTime<Utc>, retry_after: StdDuration) -> DateTime<Utc> {
    let delay = retry_after.clamp(
        StdDuration::from_secs(TOKEN_EXCHANGE_MIN_RETRY_SECONDS),
        StdDuration::from_secs(TOKEN_EXCHANGE_MAX_RETRY_SECONDS),
    );
    Duration::from_std(delay)
        .ok()
        .and_then(|delay| now.checked_add_signed(delay))
        .unwrap_or(DateTime::<Utc>::MAX_UTC)
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
    // The GitHub refresh grant follows no redirects; without that client no
    // refresh begins.
    let http = meerkat_auth_core::auth_oauth::credential_http_client()
        .map_err(|error| RefreshError::Refresh(error.to_string()))?;
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
        let http = http.clone();
        Box::pin(async move {
            let current = token_store
                .load_for(&key, TokenStoreAccess::Unattended)
                .await
                .map_err(RefreshError::from_token_store)?
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
                            }).await);
                        }
                    };
                    let mut endpoints = meerkat_auth_core::oauth_flow::oauth_provider_endpoints(
                        meerkat_core::OAuthProviderIdentity::GitHubCopilot,
                        "",
                    );
                    let configured = match config.endpoints() {
                        Ok(configured) => configured,
                        Err(error) => {
                            return Err(transaction
                                .fail(RefreshError::Refresh(error.to_string()))
                                .await);
                        }
                    };
                    endpoints.token_url = configured.oauth_token_url.to_string();
                    let result =
                        match exchange_refresh_token(&http, &endpoints, &refresh_token, None).await
                        {
                            Ok(result) => result,
                            Err(error) => {
                                return Err(transaction.fail(oauth_refresh_error(error)).await);
                            }
                        };
                    let now = Utc::now();
                    let expires_at = match result.expires_at_from(now) {
                        Ok(expires_at) => expires_at,
                        Err(error) => {
                            return Err(transaction
                                .fail(RefreshError::Refresh(error.to_string()))
                                .await);
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

    mod redirect_fixture {
        #![allow(clippy::unwrap_used)]
        /// A credential endpoint that answers every request with a `302` to a
        /// second listener, which counts the requests that reach it. The
        /// `Location` and the body carry canaries that must never be rendered.
        pub(super) const LOCATION_CANARY: &str = "redirect-location-secret-canary";
        pub(super) const BODY_CANARY: &str = "redirect-body-secret-canary";

        pub(super) async fn spawn_redirecting_endpoint()
        -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
            use std::sync::Arc;
            use std::sync::atomic::{AtomicUsize, Ordering};
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            use tokio::net::TcpListener;

            async fn read_request(stream: &mut tokio::net::TcpStream) {
                let mut buffer = Vec::new();
                let mut chunk = [0_u8; 4096];
                loop {
                    let Ok(read) = stream.read(&mut chunk).await else {
                        return;
                    };
                    if read == 0 {
                        return;
                    }
                    buffer.extend_from_slice(&chunk[..read]);
                    let text = String::from_utf8_lossy(&buffer);
                    if let Some(end) = text.find("\r\n\r\n") {
                        let length = text[..end]
                            .lines()
                            .find_map(|line| {
                                let (name, value) = line.split_once(':')?;
                                name.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse::<usize>().ok())
                                    .flatten()
                            })
                            .unwrap_or(0);
                        if buffer.len() >= end + 4 + length {
                            return;
                        }
                    }
                }
            }

            let hits = Arc::new(AtomicUsize::new(0));
            let target = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let target_addr = target.local_addr().unwrap();
            let counted = Arc::clone(&hits);
            tokio::spawn(async move {
                while let Ok((mut stream, _)) = target.accept().await {
                    counted.fetch_add(1, Ordering::SeqCst);
                    read_request(&mut stream).await;
                    let _ = stream
                        .write_all(
                            b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}",
                        )
                        .await;
                }
            });
            let endpoint = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let endpoint_addr = endpoint.local_addr().unwrap();
            tokio::spawn(async move {
                while let Ok((mut stream, _)) = endpoint.accept().await {
                    read_request(&mut stream).await;
                    let response = format!(
                        "HTTP/1.1 302 Found\r\nlocation: http://{target_addr}/token?leak={LOCATION_CANARY}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{BODY_CANARY}",
                        BODY_CANARY.len()
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                }
            });
            (format!("http://{endpoint_addr}/token"), hits)
        }
    }

    #[tokio::test]
    async fn copilot_credential_requests_never_follow_a_redirect() {
        use redirect_fixture::{BODY_CANARY, LOCATION_CANARY, spawn_redirecting_endpoint};
        let (url, target_hits) = spawn_redirecting_endpoint().await;
        let error = match ReqwestCopilotHttpTransport::new()
            .get(&url, HeaderMap::new())
            .await
        {
            Err(error) => error,
            Ok(response) => panic!("a redirect was not refused: status {}", response.status),
        };
        assert!(
            matches!(
                error,
                CopilotHttpTransportError::RedirectRefused { status: 302 }
            ),
            "{error:?}"
        );
        let rendered = format!("{error} {error:?}");
        assert!(!rendered.contains(LOCATION_CANARY) && !rendered.contains(BODY_CANARY));
        assert_eq!(target_hits.load(std::sync::atomic::Ordering::SeqCst), 0);
    }
    /// GitHub's answer to a request over its secondary rate limit, as GitHub
    /// documents and clients observe it: a 403 whose message says so, which
    /// may carry no `Retry-After` and no rate-limit header.
    const SECONDARY_RATE_LIMIT_BODY: &str = r#"{"documentation_url":"https://docs.github.com/rest/using-the-rest-api/rate-limits-for-the-rest-api#about-secondary-rate-limits","message":"You have exceeded a secondary rate limit. Please wait a few minutes before you try again."}"#;

    /// A raw HTTP/1.1 answer with `status`, the extra header lines `headers`
    /// (each ending in CRLF) and the JSON `body`.
    fn github_answer(status: u16, headers: &str, body: &str) -> String {
        let reason = match status {
            200 => "OK",
            403 => "Forbidden",
            429 => "Too Many Requests",
            _ => "Answer",
        };
        format!(
            "HTTP/1.1 {status} {reason}\r\ncontent-type: application/json; charset=utf-8\r\n{headers}content-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    /// A local server that answers its `n`th request with `answers[n]` and
    /// counts the requests it reads.
    async fn spawn_github_answers(
        answers: Vec<String>,
    ) -> (String, Arc<std::sync::atomic::AtomicUsize>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind a local GitHub");
        let address = listener.local_addr().expect("local GitHub address");
        let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = Arc::clone(&hits);
        tokio::spawn(async move {
            let mut answers = VecDeque::from(answers);
            while let Ok((mut stream, _)) = listener.accept().await {
                let mut head = Vec::new();
                let mut chunk = [0_u8; 1024];
                while !head.windows(4).any(|window| window == b"\r\n\r\n") {
                    match stream.read(&mut chunk).await {
                        Ok(0) | Err(_) => break,
                        Ok(read) => head.extend_from_slice(&chunk[..read]),
                    }
                }
                counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let answer = answers
                    .pop_front()
                    .unwrap_or_else(|| github_answer(500, "", r#"{"message":"no answer left"}"#));
                let _ = stream.write_all(answer.as_bytes()).await;
            }
        });
        (format!("http://{address}"), hits)
    }

    /// The production transport, aimed at a local server that answers as
    /// GitHub and the Copilot API do: every request keeps its path and
    /// headers, so real status lines, headers and bodies go through the
    /// production parse.
    #[cfg(unix)]
    struct LocalGitHub {
        inner: ReqwestCopilotHttpTransport,
        base: String,
    }

    #[cfg(unix)]
    #[async_trait]
    impl CopilotHttpTransport for LocalGitHub {
        async fn get(
            &self,
            url: &str,
            headers: HeaderMap,
        ) -> Result<CopilotHttpResponse, CopilotHttpTransportError> {
            let path = url::Url::parse(url)
                .map(|url| url.path().to_string())
                .unwrap_or_default();
            self.inner
                .get(&format!("{}{path}", self.base), headers)
                .await
        }
    }

    /// GitHub's rate limits as the production transport reads them from real
    /// answers: a 403 with `Retry-After`, a 403 whose primary limit is spent
    /// (`x-ratelimit-remaining: 0` with `x-ratelimit-reset`), a 429 with no
    /// header, and the secondary rate limit, a 403 whose message says so
    /// with no header at all or a blank `Retry-After` (octokit's rule: GitHub
    /// may send it without `Retry-After`), spelt "rate limit" or
    /// "rate-limit" (octokit matches `\bsecondary rate\b`). A 403 that names
    /// no limit is no rate limit.
    #[tokio::test]
    async fn github_rate_limits_are_read_from_real_answers() {
        let reset = Utc::now().timestamp() + 300;
        let cases: Vec<(&str, String, bool)> = vec![
            (
                "403 with Retry-After",
                github_answer(
                    403,
                    "retry-after: 30\r\n",
                    r#"{"message":"API rate limit exceeded for user ID 1."}"#,
                ),
                true,
            ),
            (
                "403 with a spent primary limit",
                github_answer(
                    403,
                    &format!("x-ratelimit-remaining: 0\r\nx-ratelimit-reset: {reset}\r\n"),
                    r#"{"message":"API rate limit exceeded for user ID 1."}"#,
                ),
                true,
            ),
            (
                "429 with no header",
                github_answer(429, "", r#"{"message":"Too many requests"}"#),
                true,
            ),
            (
                "403 secondary limit with no header",
                github_answer(403, "", SECONDARY_RATE_LIMIT_BODY),
                true,
            ),
            (
                "403 secondary limit with a blank Retry-After",
                github_answer(403, "retry-after: \r\n", SECONDARY_RATE_LIMIT_BODY),
                true,
            ),
            (
                "403 that names no limit",
                github_answer(
                    403,
                    "x-ratelimit-remaining: 4999\r\n",
                    r#"{"message":"Resource not accessible by personal access token"}"#,
                ),
                false,
            ),
            (
                "403 secondary rate-limit with no header",
                github_answer(
                    403,
                    "",
                    r#"{"message":"You have exceeded a secondary rate-limit. Please wait a few minutes before you try again."}"#,
                ),
                true,
            ),
        ];
        let (base, hits) =
            spawn_github_answers(cases.iter().map(|(_, answer, _)| answer.clone()).collect()).await;
        let transport = ReqwestCopilotHttpTransport::new();

        let mut read = Vec::new();
        for (label, _, _) in &cases {
            let response = transport
                .get(&format!("{base}/copilot_internal/user"), HeaderMap::new())
                .await
                .unwrap_or_else(|error| panic!("{label}: {error}"));
            read.push((
                *label,
                response.rate_limited,
                response.retry_after,
                response.rate_limit_reset,
            ));
        }

        let reset = u64::try_from(reset).expect("a positive reset");
        assert_eq!(
            read,
            vec![
                (cases[0].0, true, Some(StdDuration::from_secs(30)), None),
                (cases[1].0, true, None, Some(reset)),
                (cases[2].0, true, None, None),
                (cases[3].0, true, None, None),
                (cases[4].0, true, None, None),
                (cases[5].0, false, None, None),
                (cases[6].0, true, None, None),
            ]
        );
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), cases.len());
    }

    use meerkat_core::auth::TokenStore;
    use meerkat_core::provider_matrix::openai::{OpenAiAuthMethod, OpenAiBackendKind};
    use meerkat_core::{
        AuthBindingRef, AuthProfile, BackendProfile, BindingId, BindingOrigin, BindingPolicy,
        CredentialAccountId, CredentialAccountRef, CredentialSourceSpec, RealmId,
    };
    use meerkat_llm_core::provider_runtime::ProviderRuntimeCatalog;
    use std::collections::VecDeque;

    struct SpecAuthorizer;

    #[async_trait]
    impl HttpAuthorizer for SpecAuthorizer {
        async fn authorize(
            &self,
            _request: &mut HttpAuthorizationRequest<'_>,
        ) -> Result<(), AuthError> {
            Ok(())
        }

        fn label(&self) -> &'static str {
            "copilot-spec-test"
        }
    }

    #[test]
    fn chat_completions_spec_constructor_preserves_historical_image_input_support() {
        let spec = CopilotChatCompletionsClientSpec::new(
            Provider::OpenAI,
            "model".to_string(),
            "https://example.test".to_string(),
            Arc::new(SpecAuthorizer),
            true,
            true,
            true,
            false,
        );

        assert!(spec.supports_image_input());
        assert!(!spec.with_image_input_support(false).supports_image_input());
    }

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

    struct DelayedTransport {
        inner: ScriptedTransport,
    }

    struct CancelFirstTransport {
        inner: ScriptedTransport,
        calls: AtomicU64,
        first_started: tokio::sync::Notify,
    }

    #[async_trait]
    impl CopilotHttpTransport for DelayedTransport {
        async fn get(
            &self,
            url: &str,
            headers: HeaderMap,
        ) -> Result<CopilotHttpResponse, CopilotHttpTransportError> {
            tokio::time::sleep(StdDuration::from_millis(10)).await;
            self.inner.get(url, headers).await
        }
    }

    #[async_trait]
    impl CopilotHttpTransport for CancelFirstTransport {
        async fn get(
            &self,
            url: &str,
            headers: HeaderMap,
        ) -> Result<CopilotHttpResponse, CopilotHttpTransportError> {
            if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                self.first_started.notify_one();
                return futures::future::pending().await;
            }
            self.inner.get(url, headers).await
        }
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

    struct DispatchCountingClient {
        dispatches: Arc<AtomicU64>,
    }

    #[async_trait]
    impl meerkat_llm_core::LlmClient for DispatchCountingClient {
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
            self.dispatches.fetch_add(1, Ordering::SeqCst);
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
            retry_after: None,
            rate_limited: false,
            rate_limit_reset: None,
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

    /// Let work the code under test started in the background (a model
    /// discovery off the request path) run until `done` holds.
    async fn settle(done: impl Fn() -> bool) {
        for _ in 0..10_000 {
            if done() {
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!("the background work did not settle");
    }

    /// A model listing of `gpt-test` and `later_id`.
    fn models_listing_with(later_id: &str) -> CopilotHttpResponse {
        json_response(
            200,
            serde_json::json!({
                "data": [
                    {
                        "id": "gpt-test",
                        "supported_endpoints": ["/responses", "/chat/completions"]
                    },
                    {
                        "id": later_id,
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
        let validated = ProviderRuntimeCatalog::validate_binding_with_credential_identity(
            &auth_binding,
            account_identity(),
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
        _auth_binding: &AuthBindingRef,
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
        let identity = account_identity();
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
    fn hostile_retry_after_is_clamped_before_datetime_conversion() {
        let now = DateTime::from_timestamp(1_800_000_000, 0).expect("valid timestamp");
        let error = CopilotModelDiscoveryError::Http {
            status: 429,
            message: "busy".to_string(),
            retry_after: Some(StdDuration::from_secs(u64::MAX)),
            rate_limited: true,
            rate_limit_reset: None,
            account_action: false,
        };

        let delay = error.retry_delay();
        assert_eq!(
            delay,
            StdDuration::from_secs(MODEL_DISCOVERY_MAX_RETRY_SECONDS)
        );
        assert_eq!(
            model_discovery_deadline(now, delay),
            now + Duration::seconds(MODEL_DISCOVERY_MAX_RETRY_SECONDS as i64)
        );
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
            .account_state(
                persistence_id,
                &identity,
                CopilotBackendConfig::default(),
                CopilotCredentialMode::DeviceSignIn,
            )
            .expect("first route");
        let second = runtime
            .account_state(
                persistence_id,
                &identity,
                CopilotBackendConfig::default(),
                CopilotCredentialMode::DeviceSignIn,
            )
            .expect("second route");
        assert!(Arc::ptr_eq(&first, &second));
    }

    #[test]
    fn equal_account_keys_from_distinct_persistence_authorities_do_not_share_state() {
        let runtime = CopilotRuntime::new();
        let identity = account_identity();
        let first = runtime
            .account_state(
                persistence_id(),
                &identity,
                CopilotBackendConfig::default(),
                CopilotCredentialMode::DeviceSignIn,
            )
            .expect("first persistence authority");
        let second = runtime
            .account_state(
                persistence_id(),
                &identity,
                CopilotBackendConfig::default(),
                CopilotCredentialMode::DeviceSignIn,
            )
            .expect("second persistence authority");

        assert!(!Arc::ptr_eq(&first, &second));
    }

    #[tokio::test]
    async fn routed_client_requires_reprojection_after_route_change() {
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
                CopilotCredentialMode::DeviceSignIn,
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
            supported_endpoints: Some(vec![CopilotEndpoint::Responses]),
        }])
        .expect("model snapshot");
        *state.cache.write() = Some(CachedCopilotToken {
            generation: 1,
            token: "derived".to_string(),
            source_generation: 1,
            source_expires_at: None,
            refresh_at: Utc::now() + Duration::hours(1),
            expires_at: Utc::now() + Duration::hours(2),
            api_base: "https://first.example.test".to_string(),
            models: Some(Arc::new(snapshot)),
            model_discovery_refresh_at: None,
            identity: None,
            exchange_unavailable: None,
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
        let dispatches = Arc::new(AtomicU64::new(0));
        let dispatches_for_factory = Arc::clone(&dispatches);
        let factory: CopilotRouteClientFactory = Arc::new(move |route, _| {
            observed_for_factory.lock().push(route.api_base.clone());
            Ok(Arc::new(DispatchCountingClient {
                dispatches: Arc::clone(&dispatches_for_factory),
            }))
        });
        let client = routed_client(
            Arc::clone(&runtime),
            connection,
            Provider::OpenAI,
            "gpt-test".to_string(),
            factory,
        )
        .expect("routed client");

        let first_projection = client
            .project_replay_request(&[])
            .expect("first route projection");
        let first_request = meerkat_llm_core::PreparedLlmRequest::from_projection(
            meerkat_llm_core::LlmRequest::new("gpt-test", Vec::new()),
            first_projection,
        );
        state.cache.write().as_mut().expect("cache").api_base =
            "https://second.example.test".to_string();
        assert!(matches!(
            client.prepared_request_pressure(&first_request),
            Err(meerkat_llm_core::LlmError::AuthorizationRouteChanged { .. })
        ));
        let first_attempt = client
            .stream_prepared(&first_request)
            .collect::<Vec<_>>()
            .await;

        assert!(matches!(
            first_attempt.as_slice(),
            [Err(
                meerkat_llm_core::LlmError::AuthorizationRouteChanged { .. }
            )]
        ));
        assert_eq!(
            dispatches.load(Ordering::SeqCst),
            0,
            "a stale request route must be rejected before provider dispatch"
        );
        let second_projection = client
            .project_replay_request(&[])
            .expect("second route projection");
        let second_request = meerkat_llm_core::PreparedLlmRequest::from_projection(
            meerkat_llm_core::LlmRequest::new("gpt-test", Vec::new()),
            second_projection,
        );
        let second_attempt = client
            .stream_prepared(&second_request)
            .collect::<Vec<_>>()
            .await;
        assert!(
            second_attempt.is_empty(),
            "unexpected routed events after reprojection: {second_attempt:?}"
        );
        assert_eq!(dispatches.load(Ordering::SeqCst), 1);
        assert_eq!(
            observed.lock().as_slice(),
            ["https://first.example.test", "https://second.example.test"]
        );
    }

    /// The account model of a resolved Copilot route comes from the model
    /// list its account state holds: a listed model is offered under the
    /// family of its protocol, an unlisted one is refused naming the ids the
    /// account offers, and while no list is loaded the list defines no
    /// model.
    #[tokio::test]
    async fn account_model_reads_the_resolved_accounts_model_list() {
        use meerkat_llm_core::provider_runtime::{AccountModelLookup, AccountModelRefusal};

        let runtime = CopilotRuntime::new();
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
                CopilotCredentialMode::DeviceSignIn,
            )
            .expect("account state");
        let snapshot = CopilotModelSnapshot::from_models(vec![crate::CopilotModel {
            id: "account-model-1".to_string(),
            vendor: Some("ExampleVendor".to_string()),
            name: None,
            version: None,
            model_picker_enabled: None,
            capabilities: crate::CopilotModelCapabilities::default(),
            policy: None,
            supported_endpoints: Some(vec![CopilotEndpoint::Responses]),
        }])
        .expect("model snapshot");
        *state.cache.write() = Some(CachedCopilotToken {
            generation: 1,
            token: "derived".to_string(),
            source_generation: 1,
            source_expires_at: None,
            refresh_at: Utc::now() + Duration::hours(1),
            expires_at: Utc::now() + Duration::hours(2),
            api_base: "https://first.example.test".to_string(),
            models: Some(Arc::new(snapshot)),
            model_discovery_refresh_at: None,
            identity: None,
            exchange_unavailable: None,
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
        let credential = binding.credential_identity().clone();

        match runtime.account_model(&connection, "account-model-1") {
            AccountModelLookup::Offered(definition) => {
                assert_eq!(definition.provider, Provider::OpenAI);
            }
            other => panic!("the listed model is offered: {other:?}"),
        }
        assert_eq!(
            runtime.account_model(&connection, "absent-model"),
            AccountModelLookup::Refused(AccountModelRefusal::NotOffered {
                credential: credential.clone(),
                model: "absent-model".to_string(),
                available: vec!["account-model-1".to_string()],
            })
        );
        state.cache.write().as_mut().expect("cache").models = None;
        assert_eq!(
            runtime.account_model(&connection, "account-model-1"),
            AccountModelLookup::Refused(AccountModelRefusal::ModelsUnavailable {
                credential,
                model: "account-model-1".to_string(),
            })
        );
    }

    /// Records what the routed client hands the provider adapter and compiles
    /// schemas with a marker, standing in for the route's adapter client.
    struct StructuredOutputRecordingClient {
        seen: Arc<Mutex<Vec<meerkat_llm_core::LlmRequest>>>,
    }

    #[async_trait]
    impl meerkat_llm_core::LlmClient for StructuredOutputRecordingClient {
        fn project_replay_messages(
            &self,
            messages: &[meerkat_core::Message],
        ) -> Result<Vec<meerkat_core::Message>, meerkat_llm_core::LlmError> {
            Ok(messages.to_vec())
        }

        fn stream<'a>(
            &'a self,
            request: &'a meerkat_llm_core::LlmRequest,
        ) -> meerkat_llm_core::LlmStream<'a> {
            self.seen.lock().push(request.clone());
            Box::pin(futures::stream::empty())
        }

        fn provider(&self) -> Provider {
            Provider::OpenAI
        }

        async fn health_check(&self) -> Result<(), meerkat_llm_core::LlmError> {
            Ok(())
        }

        fn compile_schema(
            &self,
            output_schema: &meerkat_core::OutputSchema,
        ) -> Result<meerkat_core::schema::CompiledSchema, meerkat_core::schema::SchemaError>
        {
            let mut schema = output_schema.schema.as_value().clone();
            schema["x-compiled-by"] = serde_json::json!("route-adapter");
            Ok(meerkat_core::schema::CompiledSchema {
                schema,
                warnings: Vec::new(),
            })
        }
    }

    /// Structured output on every Copilot route: the routed client (and the
    /// account capability gate in front of the adapter) hands the adapter the
    /// exact request the agent loop composed, schema section and native
    /// schema slot included, and the section shows the schema the route's
    /// adapter compiles. The adapters' lowering of those requests is pinned in
    /// the provider crates.
    #[tokio::test]
    async fn routed_client_passes_structured_output_requests_through_unchanged() {
        use meerkat_core::lifecycle::run_primitive::{OpenAiProviderTag, ProviderTag};
        use meerkat_core::structured_output::{
            OUTPUT_SCHEMA_INSTRUCTIONS_OPEN, project_output_schema_instructions,
            render_output_schema_instructions,
        };

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
                CopilotCredentialMode::DeviceSignIn,
            )
            .expect("account state");
        let snapshot = CopilotModelSnapshot::from_models(vec![crate::CopilotModel {
            id: "gpt-test".to_string(),
            vendor: None,
            name: None,
            version: None,
            model_picker_enabled: None,
            capabilities: crate::CopilotModelCapabilities {
                supports: crate::CopilotModelSupports {
                    tool_calls: Some(true),
                    streaming: Some(true),
                    structured_outputs: Some(true),
                    ..Default::default()
                },
                ..Default::default()
            },
            policy: None,
            supported_endpoints: Some(vec![CopilotEndpoint::Responses]),
        }])
        .expect("model snapshot");
        *state.cache.write() = Some(CachedCopilotToken {
            generation: 1,
            token: "derived".to_string(),
            source_generation: 1,
            source_expires_at: None,
            refresh_at: Utc::now() + Duration::hours(1),
            expires_at: Utc::now() + Duration::hours(2),
            api_base: "https://copilot.example.test".to_string(),
            models: Some(Arc::new(snapshot)),
            model_discovery_refresh_at: None,
            identity: None,
            exchange_unavailable: None,
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
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_for_factory = Arc::clone(&seen);
        let factory: CopilotRouteClientFactory = Arc::new(move |_, _| {
            Ok(Arc::new(StructuredOutputRecordingClient {
                seen: Arc::clone(&seen_for_factory),
            }))
        });
        let client = routed_client(
            Arc::clone(&runtime),
            connection,
            Provider::OpenAI,
            "gpt-test".to_string(),
            factory,
        )
        .expect("routed client");

        let schema = meerkat_core::OutputSchema::new(serde_json::json!({
            "type": "object",
            "properties": {"verdict": {"type": "string"}},
            "required": ["verdict"]
        }))
        .expect("valid schema");
        let compiled = client.compile_schema(&schema).expect("compile");
        assert_eq!(
            compiled.schema["x-compiled-by"], "route-adapter",
            "the routed client compiles with the route's adapter"
        );
        let section = render_output_schema_instructions(&compiled.schema);
        let mut messages = vec![
            meerkat_core::Message::System(meerkat_core::SystemMessage::new("You are a reviewer.")),
            meerkat_core::Message::User(meerkat_core::UserMessage::text("review")),
        ];
        project_output_schema_instructions(&mut messages, &section);
        let tool = Arc::new(meerkat_core::ToolDef {
            audience: Default::default(),
            name: "lookup".into(),
            description: "returns a fixed observation".to_string(),
            input_schema: serde_json::json!({"type": "object", "properties": {}}),
            provenance: None,
        });
        let mut main_turn =
            meerkat_llm_core::LlmRequest::new("gpt-test", messages.clone()).with_tools(vec![tool]);
        main_turn.provider_params = Some(ProviderTag::OpenAi(OpenAiProviderTag::default()));
        let mut extraction =
            meerkat_llm_core::LlmRequest::new("gpt-test", messages).with_temperature(0.0);
        extraction.provider_params = Some(ProviderTag::OpenAi(OpenAiProviderTag {
            structured_output: Some(schema.clone()),
            ..Default::default()
        }));

        for request in [&main_turn, &extraction] {
            let projection = client
                .project_replay_request(&request.messages)
                .expect("route projection");
            let prepared =
                meerkat_llm_core::PreparedLlmRequest::from_projection(request.clone(), projection);
            let events = client.stream_prepared(&prepared).collect::<Vec<_>>().await;
            assert!(events.is_empty(), "unexpected routed events: {events:?}");
        }
        let seen = seen.lock().clone();
        assert_eq!(seen.len(), 2, "both requests reached the adapter");
        for (sent, received) in [&main_turn, &extraction].into_iter().zip(&seen) {
            assert_eq!(
                serde_json::to_value(received).expect("serialize"),
                serde_json::to_value(sent).expect("serialize"),
                "the routed client hands the adapter the request unchanged"
            );
            assert_eq!(
                serde_json::to_string(&received.messages)
                    .expect("serialize")
                    .matches(OUTPUT_SCHEMA_INSTRUCTIONS_OPEN)
                    .count(),
                1,
                "the section reaches the adapter exactly once"
            );
        }
        let Some(ProviderTag::OpenAi(tag)) = seen[1].provider_params.as_ref() else {
            panic!("extraction keeps its provider tag");
        };
        assert!(
            tag.structured_output.is_some(),
            "the extraction request keeps its native schema slot"
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
        assert_eq!(
            client.output_token_limit(),
            Some(16),
            "the limit the gate enforces is stated to the factory"
        );
        assert_eq!(
            capability_gated_client(Arc::new(NoopLlmClient), None).output_token_limit(),
            None
        );
    }

    /// The account model's prompt limit is the most input one request may
    /// carry on the route: the gate states it to the factory (the tighter of
    /// the prompt limit and the context window, either alone when only one is
    /// stated), which keeps the session's default compaction threshold below
    /// it.
    #[test]
    fn account_capability_gate_states_the_account_prompt_limit() {
        let gated = |max_prompt_tokens, max_context_window_tokens| {
            capability_gated_client(
                Arc::new(NoopLlmClient),
                Some(crate::CopilotModelCapabilities {
                    limits: crate::CopilotModelLimits {
                        max_context_window_tokens,
                        max_prompt_tokens,
                        ..Default::default()
                    },
                    supports: Default::default(),
                }),
            )
        };
        assert_eq!(
            gated(Some(64_000), Some(128_000)).prompt_token_limit(),
            Some(64_000)
        );
        assert_eq!(
            gated(Some(200_000), Some(128_000)).prompt_token_limit(),
            Some(128_000)
        );
        assert_eq!(gated(Some(64_000), None).prompt_token_limit(), Some(64_000));
        assert_eq!(
            gated(None, Some(128_000)).prompt_token_limit(),
            Some(128_000)
        );
        assert_eq!(gated(None, None).prompt_token_limit(), None);
        assert_eq!(
            capability_gated_client(Arc::new(NoopLlmClient), None).prompt_token_limit(),
            None
        );
    }

    /// A client that states the provider's own input-token count for every
    /// request, as a provider with a counting endpoint would.
    struct MeasuredInputClient {
        input_tokens: u64,
    }

    #[async_trait]
    impl meerkat_llm_core::LlmClient for MeasuredInputClient {
        fn project_replay_messages(
            &self,
            messages: &[meerkat_core::Message],
        ) -> Result<Vec<meerkat_core::Message>, meerkat_llm_core::LlmError> {
            Ok(messages.to_vec())
        }

        fn request_pressure(
            &self,
            _request: &meerkat_llm_core::LlmRequest,
        ) -> Result<Option<meerkat_core::ProviderRequestPressure>, meerkat_llm_core::LlmError>
        {
            Ok(Some(
                meerkat_core::ProviderRequestPressure::new(1, None)
                    .with_provider_issued_input_tokens(self.input_tokens),
            ))
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

    /// A prompt the provider measured over the account model's prompt limit
    /// is a context refusal (`ContextLengthExceeded`), the class the agent
    /// compacts on, not an invalid request; one at the limit goes out. No
    /// Copilot error body is assumed: the gate decides from the stated limit
    /// and the provider's own count.
    #[test]
    fn a_prompt_measured_over_the_account_limit_is_a_context_refusal() {
        let gated = |input_tokens| {
            capability_gated_client(
                Arc::new(MeasuredInputClient { input_tokens }),
                Some(crate::CopilotModelCapabilities {
                    limits: crate::CopilotModelLimits {
                        max_prompt_tokens: Some(64_000),
                        ..Default::default()
                    },
                    supports: Default::default(),
                }),
            )
        };
        let request = meerkat_llm_core::LlmRequest::new("gpt-test", Vec::new());
        let refused = gated(64_001).request_pressure(&request);
        assert!(
            matches!(
                refused,
                Err(meerkat_llm_core::LlmError::ContextLengthExceeded {
                    max: 64_000,
                    requested: 64_001,
                })
            ),
            "{refused:?}"
        );
        assert!(gated(64_000).request_pressure(&request).is_ok());
    }

    /// A model the account lists with a policy state other than `enabled`
    /// is unavailable on the account's routes, and the refusal names the
    /// state the account states, with the models the account does offer.
    #[test]
    fn an_unavailable_route_names_the_policy_state() {
        let listed =
            |id: &str, policy: Option<crate::CopilotModelPolicyState>| crate::CopilotModel {
                id: id.to_string(),
                vendor: None,
                name: None,
                version: None,
                model_picker_enabled: None,
                capabilities: Default::default(),
                policy: policy.map(|state| crate::CopilotModelPolicy { state }),
                supported_endpoints: Some(vec![CopilotEndpoint::Responses]),
            };
        let snapshot = CopilotModelSnapshot::from_models(vec![
            listed(
                "gpt-test",
                Some(crate::CopilotModelPolicyState::Other(
                    "unconfigured".to_string(),
                )),
            ),
            listed("gpt-other", None),
        ])
        .expect("model snapshot");
        let state = Arc::new(CopilotAccountState::new(
            CopilotBackendConfig::default(),
            CopilotCredentialMode::DeviceSignIn,
        ));
        *state.cache.write() = Some(CachedCopilotToken {
            generation: 1,
            token: "derived".to_string(),
            source_generation: 1,
            source_expires_at: None,
            refresh_at: Utc::now() + Duration::hours(1),
            expires_at: Utc::now() + Duration::hours(2),
            api_base: "https://copilot.example.test".to_string(),
            models: Some(Arc::new(snapshot)),
            model_discovery_refresh_at: None,
            identity: None,
            exchange_unavailable: None,
        });
        let route =
            resolve_route_for_state(&state, Provider::OpenAI, "gpt-test").expect("a cached route");
        assert_eq!(route.access, CopilotModelAccess::Unavailable);
        let message =
            route.unavailable_message(Provider::OpenAI, "gpt-test", CopilotEndpoint::Responses);
        assert!(
            message.contains("policy state for openai model 'gpt-test' is 'unconfigured'"),
            "{message}"
        );
        assert!(message.contains("gpt-other"), "{message}");
    }

    /// A catalog entry whose endpoint list is empty states that the model has
    /// no endpoints: no family is served it (an entry that states no list is
    /// served on Chat Completions), the account's model list refuses it with
    /// `NoEndpoints`, and a registry model's route says so.
    #[test]
    fn an_explicitly_empty_endpoint_list_is_refused() {
        use meerkat_llm_core::provider_runtime::{AccountModelLookup, AccountModelRefusal};
        let listed: crate::CopilotModelsEnvelope = serde_json::from_value(serde_json::json!({
            "data": [
                {"id": "gpt-test", "supported_endpoints": []},
                {"id": "gpt-other", "supported_endpoints": ["/responses"]}
            ]
        }))
        .expect("model list");
        let snapshot = CopilotModelSnapshot::from_models(listed.data).expect("model snapshot");
        let offering = snapshot.model("gpt-test").expect("listed model");
        for provider in [Provider::OpenAI, Provider::Gemini, Provider::Anthropic] {
            assert_eq!(offering.route_for(provider), None, "{provider:?}");
        }
        assert_eq!(offering.protocol_provider(), None);
        let credential = account_identity();
        assert_eq!(
            snapshot.account_model(&credential, "gpt-test"),
            AccountModelLookup::Refused(AccountModelRefusal::NoEndpoints {
                credential: credential.clone(),
                model: "gpt-test".to_string(),
            })
        );

        let state = Arc::new(CopilotAccountState::new(
            CopilotBackendConfig::default(),
            CopilotCredentialMode::DeviceSignIn,
        ));
        *state.cache.write() = Some(CachedCopilotToken {
            generation: 1,
            token: "derived".to_string(),
            source_generation: 1,
            source_expires_at: None,
            refresh_at: Utc::now() + Duration::hours(1),
            expires_at: Utc::now() + Duration::hours(2),
            api_base: "https://copilot.example.test".to_string(),
            models: Some(Arc::new(snapshot)),
            model_discovery_refresh_at: None,
            identity: None,
            exchange_unavailable: None,
        });
        let route =
            resolve_route_for_state(&state, Provider::OpenAI, "gpt-test").expect("a cached route");
        assert_eq!(route.access, CopilotModelAccess::Unavailable);
        let message =
            route.unavailable_message(Provider::OpenAI, "gpt-test", CopilotEndpoint::Responses);
        assert!(
            message.contains("lists no endpoints for openai model 'gpt-test'"),
            "{message}"
        );
        assert!(message.contains("gpt-other"), "{message}");
    }

    #[test]
    fn shared_account_rejects_conflicting_backend_options() {
        let runtime = CopilotRuntime::new();
        let identity = account_identity();
        let persistence_id = persistence_id();
        let _first = runtime
            .account_state(
                persistence_id,
                &identity,
                CopilotBackendConfig::default(),
                CopilotCredentialMode::DeviceSignIn,
            )
            .expect("first route");
        let conflicting = CopilotBackendConfig {
            integration_id: "different-client".to_string(),
            ..CopilotBackendConfig::default()
        };
        assert!(
            runtime
                .account_state(
                    persistence_id,
                    &identity,
                    conflicting,
                    CopilotCredentialMode::DeviceSignIn,
                )
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

    /// The class of a non-2xx exchange answer that asks for no account
    /// action: a rate limit (429, or a 403) with its Retry-After, a server
    /// error (5xx) and a request timeout (408) may pass; any other status
    /// fails as before, with its text.
    #[test]
    fn token_exchange_statuses_are_classified() {
        for status in [403, 429] {
            let classified = CopilotTokenExchangeError::from_status(
                status,
                Some(StdDuration::from_secs(7)),
                format!("Copilot token exchange returned {status}"),
            );
            assert!(
                matches!(
                    classified,
                    CopilotTokenExchangeError::Unavailable(CopilotExchangeUnavailable {
                        retry_after: Some(_),
                        retryable: meerkat_llm_core::LlmError::RateLimited {
                            retry_after_ms: Some(7000)
                        },
                        ..
                    })
                ),
                "HTTP {status} is a rate limit carrying its Retry-After"
            );
        }
        for status in [500, 502, 503] {
            let classified = CopilotTokenExchangeError::from_status(
                status,
                None,
                format!("Copilot token exchange returned {status}"),
            );
            assert!(
                matches!(
                    classified,
                    CopilotTokenExchangeError::Unavailable(CopilotExchangeUnavailable {
                        retryable: meerkat_llm_core::LlmError::ServerError {
                            status: classified_status,
                            ..
                        },
                        ..
                    }) if classified_status == status
                ),
                "HTTP {status} is a server error"
            );
        }
        for status in [400, 404, 422] {
            let message = format!("Copilot token exchange returned {status}");
            let classified = CopilotTokenExchangeError::from_status(status, None, message.clone());
            assert!(
                matches!(
                    &classified,
                    CopilotTokenExchangeError::Failed(ProviderAuthError::SourceResolutionFailed(
                        failure
                    )) if failure == &message
                ),
                "HTTP {status} fails as before"
            );
        }
        let classified = CopilotTokenExchangeError::from_status(
            408,
            None,
            "Copilot token exchange returned 408".to_string(),
        );
        assert!(
            matches!(
                classified,
                CopilotTokenExchangeError::Unavailable(CopilotExchangeUnavailable {
                    retryable: meerkat_llm_core::LlmError::NetworkTimeout { .. },
                    ..
                })
            ),
            "HTTP 408 is a timeout of the exchange"
        );
    }

    /// The exchange's transport failures as the production transport and
    /// HTTP client report them: its own timeout, a request timeout and a
    /// refused connection may pass; a refused redirect, an oversized answer
    /// or a missing HTTP client will not.
    #[tokio::test]
    async fn token_exchange_transport_failures_are_classified() {
        fn retryable_class(
            error: &CopilotHttpTransportError,
        ) -> Option<meerkat_llm_core::LlmError> {
            match CopilotTokenExchangeError::from_transport(error) {
                CopilotTokenExchangeError::Unavailable(unavailable) => Some(unavailable.retryable),
                CopilotTokenExchangeError::Failed(_) => None,
            }
        }

        assert!(matches!(
            retryable_class(&CopilotHttpTransportError::Timeout),
            Some(meerkat_llm_core::LlmError::NetworkTimeout {
                duration_ms: 30_000
            })
        ));

        // An endpoint that accepts the connection and never answers.
        let silent = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = silent.local_addr().unwrap();
        let timed_out = reqwest::Client::builder()
            .timeout(StdDuration::from_millis(50))
            .build()
            .unwrap()
            .get(format!("http://{address}/token"))
            .send()
            .await
            .expect_err("a silent endpoint times out");
        assert!(timed_out.is_timeout(), "{timed_out:?}");
        assert!(matches!(
            retryable_class(&CopilotHttpTransportError::Request(timed_out)),
            Some(meerkat_llm_core::LlmError::NetworkTimeout { .. })
        ));

        // A refused connection, through the production transport.
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = closed.local_addr().unwrap();
        drop(closed);
        let refused = match ReqwestCopilotHttpTransport::new()
            .get(&format!("http://{address}/token"), HeaderMap::new())
            .await
        {
            Err(error) => error,
            Ok(response) => panic!("a closed port answered: status {}", response.status),
        };
        assert!(
            matches!(
                retryable_class(&refused),
                Some(meerkat_llm_core::LlmError::ConnectionReset)
            ),
            "{refused:?}"
        );

        for failure in [
            CopilotHttpTransportError::RedirectRefused { status: 302 },
            CopilotHttpTransportError::ResponseTooLarge,
            CopilotHttpTransportError::ClientUnavailable(
                meerkat_auth_core::auth_oauth::CredentialHttpClientUnavailable,
            ),
        ] {
            assert!(retryable_class(&failure).is_none(), "{failure:?}");
        }
    }

    #[test]
    fn exchange_retry_waits_at_least_a_second_and_never_past_expiry() {
        let now = DateTime::from_timestamp(1_800_000_000, 0).expect("valid timestamp");
        let expires_at = now + Duration::seconds(600);
        assert_eq!(
            exchange_retry_at(now, Some(StdDuration::ZERO), expires_at),
            now + Duration::seconds(1),
            "Retry-After: 0 cannot switch the backoff off"
        );
        assert_eq!(
            exchange_retry_at(now, Some(StdDuration::from_mins(1)), expires_at),
            now + Duration::seconds(60)
        );
        assert_eq!(
            exchange_retry_at(now, None, expires_at),
            now + Duration::seconds(30)
        );
        assert_eq!(
            exchange_retry_at(now, Some(StdDuration::from_secs(u64::MAX)), expires_at),
            expires_at,
            "a retry is never scheduled past the token's expiry"
        );
    }

    #[tokio::test]
    async fn route_scoped_credential_identity_is_rejected() {
        let (auth_binding, account_binding) = validated_binding();
        let route_binding = ProviderRuntimeCatalog::validate_binding(
            &auth_binding,
            account_binding.backend_profile(),
            account_binding.auth_profile(),
            &BindingPolicy::default(),
        )
        .expect("otherwise valid route-scoped binding");
        let (env, _) = resolver_environment(&auth_binding).await;
        let runtime = CopilotRuntime::new();

        let error = runtime
            .resolve(&route_binding, &env)
            .await
            .err()
            .expect("Copilot credentials must be account scoped");

        assert!(error.to_string().contains("account-scoped"));
    }

    #[tokio::test]
    async fn model_discovery_401_remints_before_retrying_discovery() {
        let (auth_binding, binding) = validated_binding();
        let (env, _) = resolver_environment(&auth_binding).await;
        let transport = Arc::new(ScriptedTransport::new([
            token_response("derived-one"),
            json_response(401, serde_json::json!({"message": "expired"})),
            token_response("derived-two"),
            models_response(),
        ]));
        let state = Arc::new(CopilotAccountState::new(
            CopilotBackendConfig::default(),
            CopilotCredentialMode::DeviceSignIn,
        ));
        let authorizer = CopilotAuthorizer {
            state: Arc::clone(&state),
            binding,
            env,
            transport: transport.clone(),
        };

        authorizer.prime().await.expect("401 remint succeeds");

        assert_eq!(transport.request_count(), 4);
        assert_eq!(
            state
                .cache
                .read()
                .as_ref()
                .map(|cache| cache.token.as_str()),
            Some("derived-two")
        );
    }

    /// The device sign-in's credentials travel in `Authorization` values
    /// marked sensitive: the GitHub token on the exchange and the derived
    /// token on the model listing.
    #[tokio::test]
    async fn device_credential_headers_are_sensitive() {
        let (auth_binding, binding) = validated_binding();
        let (env, _) = resolver_environment(&auth_binding).await;
        let transport = Arc::new(ScriptedTransport::new([
            token_response("derived-sensitive"),
            models_response(),
        ]));
        let authorizer = CopilotAuthorizer {
            state: Arc::new(CopilotAccountState::new(
                CopilotBackendConfig::default(),
                CopilotCredentialMode::DeviceSignIn,
            )),
            binding,
            env,
            transport: transport.clone(),
        };

        authorizer.prime().await.expect("the device token resolves");

        let requests = transport.requests.lock();
        assert_eq!(requests.len(), 2);
        for (url, headers) in requests.iter() {
            assert!(
                headers
                    .get("authorization")
                    .is_some_and(|value| value.is_sensitive()),
                "{url}"
            );
        }
    }

    /// The device sign-in keeps `Copilot-Integration-Id: vscode-chat` on its
    /// token exchange, its model listing and inference: only a token route
    /// defaults to `copilot-developer-cli`.
    #[tokio::test]
    async fn device_sign_in_keeps_the_vscode_chat_integration_id() {
        let (auth_binding, binding) = validated_binding();
        let (env, _auth_lease) = resolver_environment(&auth_binding).await;
        let transport = Arc::new(ScriptedTransport::new([
            token_response("derived-integration"),
            models_response(),
        ]));
        let runtime = CopilotRuntime::with_transport(transport.clone());
        let resolved = runtime
            .resolve(&binding, &env)
            .await
            .expect("the device route resolves");
        let mut signed = Vec::new();
        resolved
            .concrete_authorizer()
            .authorize_with_receipt(&mut HttpAuthorizationRequest {
                method: "POST",
                url: "https://copilot.example.test/responses",
                headers: &mut signed,
            })
            .await
            .expect("authorize a Copilot request");

        let mut integration_ids = transport
            .requests
            .lock()
            .iter()
            .map(|(_, headers)| {
                headers
                    .get("copilot-integration-id")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string)
            })
            .collect::<Vec<_>>();
        integration_ids.push(
            signed
                .into_iter()
                .find(|(name, _)| name.eq_ignore_ascii_case("copilot-integration-id"))
                .map(|(_, value)| value),
        );
        assert_eq!(
            integration_ids,
            vec![Some("vscode-chat".to_string()); 3],
            "token exchange, model listing, inference"
        );
    }

    /// Inference names who initiated each request in `X-Initiator`: `agent`
    /// when the agent continues its own work, `user` otherwise. An image
    /// request still names vision.
    #[tokio::test]
    async fn inference_names_who_initiated_the_request() {
        use meerkat_core::HttpAuthorizationContent;
        use meerkat_core::HttpRequestInitiator::{Agent, User};
        let (auth_binding, binding) = validated_binding();
        let (env, _auth_lease) = resolver_environment(&auth_binding).await;
        let authorizer = CopilotAuthorizer {
            state: Arc::new(CopilotAccountState::new(
                CopilotBackendConfig::default(),
                CopilotCredentialMode::DeviceSignIn,
            )),
            binding,
            env,
            transport: Arc::new(ScriptedTransport::new(Vec::new())),
        };
        let headers_for = |has_images, initiator| {
            let mut headers = Vec::new();
            authorizer
                .append_content_headers(
                    HttpAuthorizationContent {
                        has_images,
                        initiator,
                    },
                    &mut headers,
                )
                .expect("content headers");
            headers
        };
        let header = |name: &str, value: &str| (name.to_string(), value.to_string());
        assert_eq!(
            headers_for(false, Agent),
            vec![header("X-Initiator", "agent")]
        );
        assert_eq!(
            headers_for(false, User),
            vec![header("X-Initiator", "user")]
        );
        assert_eq!(
            headers_for(true, Agent),
            vec![
                header("Copilot-Vision-Request", "true"),
                header("X-Initiator", "agent")
            ]
        );
    }

    #[tokio::test]
    async fn concurrent_authorization_uses_one_derived_token_flight() {
        let (auth_binding, binding) = validated_binding();
        let (env, _) = resolver_environment(&auth_binding).await;
        let transport = Arc::new(DelayedTransport {
            inner: ScriptedTransport::new([token_response("derived"), models_response()]),
        });
        let state = Arc::new(CopilotAccountState::new(
            CopilotBackendConfig::default(),
            CopilotCredentialMode::DeviceSignIn,
        ));
        let first = CopilotAuthorizer {
            state: Arc::clone(&state),
            binding: binding.clone(),
            env: env.clone(),
            transport: transport.clone(),
        };
        let second = CopilotAuthorizer {
            state,
            binding,
            env,
            transport: transport.clone(),
        };

        let (first_result, second_result) = tokio::join!(first.prime(), second.prime());

        first_result.expect("first authorization");
        second_result.expect("second authorization");
        assert_eq!(transport.inner.request_count(), 2);
    }

    #[tokio::test]
    async fn concurrent_authorization_shares_one_failed_derived_token_flight() {
        let (auth_binding, binding) = validated_binding();
        let (env, _) = resolver_environment(&auth_binding).await;
        let transport = Arc::new(DelayedTransport {
            inner: ScriptedTransport::new([json_response(
                500,
                serde_json::json!({"message": "temporary failure"}),
            )]),
        });
        let state = Arc::new(CopilotAccountState::new(
            CopilotBackendConfig::default(),
            CopilotCredentialMode::DeviceSignIn,
        ));
        let first = CopilotAuthorizer {
            state: Arc::clone(&state),
            binding: binding.clone(),
            env: env.clone(),
            transport: transport.clone(),
        };
        let second = CopilotAuthorizer {
            state,
            binding,
            env,
            transport: transport.clone(),
        };

        let (first_result, second_result) = tokio::join!(first.prime(), second.prime());

        assert!(first_result.is_err());
        assert!(second_result.is_err());
        assert_eq!(
            transport.inner.request_count(),
            1,
            "concurrent waiters must observe the leader's typed failure"
        );
    }

    #[tokio::test]
    async fn cancelled_refresh_leader_releases_waiters_for_a_new_flight() {
        let (auth_binding, binding) = validated_binding();
        let (env, _) = resolver_environment(&auth_binding).await;
        let transport = Arc::new(CancelFirstTransport {
            inner: ScriptedTransport::new([token_response("derived"), models_response()]),
            calls: AtomicU64::new(0),
            first_started: tokio::sync::Notify::new(),
        });
        let state = Arc::new(CopilotAccountState::new(
            CopilotBackendConfig::default(),
            CopilotCredentialMode::DeviceSignIn,
        ));
        let first = CopilotAuthorizer {
            state: Arc::clone(&state),
            binding: binding.clone(),
            env: env.clone(),
            transport: transport.clone(),
        };
        let second = CopilotAuthorizer {
            state,
            binding,
            env,
            transport: transport.clone(),
        };

        let first_started = transport.first_started.notified();
        let leader = tokio::spawn(async move { first.prime().await });
        first_started.await;
        leader.abort();
        assert!(
            leader
                .await
                .expect_err("leader must be cancelled")
                .is_cancelled()
        );

        second
            .prime()
            .await
            .expect("a waiter can lead the replacement flight");
        assert_eq!(transport.inner.request_count(), 2);
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
            state: Arc::new(CopilotAccountState::new(
                CopilotBackendConfig::default(),
                CopilotCredentialMode::DeviceSignIn,
            )),
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
                .snapshot(&meerkat_core::handles::LeaseKey::from_credential_identity(
                    &account_identity(),
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
        let state = Arc::new(CopilotAccountState::new(
            CopilotBackendConfig::default(),
            CopilotCredentialMode::DeviceSignIn,
        ));
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
            .model_discovery_refresh_at = Some(Utc::now() - Duration::seconds(1));

        let mut headers = Vec::new();
        authorizer
            .authorize(&mut HttpAuthorizationRequest {
                method: "POST",
                url: "https://copilot.example.test/responses",
                headers: &mut headers,
            })
            .await
            .expect("the request goes out while the discovery is retried");

        // The retry runs off the request path.
        settle(|| {
            authorizer
                .model_snapshot()
                .is_some_and(|snapshot| snapshot.model("gpt-test").is_some())
        })
        .await;
        assert_eq!(
            transport.request_count(),
            3,
            "discovery retry must reuse the derived token"
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
            state: Arc::new(CopilotAccountState::new(
                CopilotBackendConfig::default(),
                CopilotCredentialMode::DeviceSignIn,
            )),
            binding,
            env,
            transport: Arc::new(ReqwestCopilotHttpTransport::new()),
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

    /// Structural pin for credential clients built inside a function (the
    /// refresh and setup paths): the production part of this file builds no
    /// HTTP client other than `credential_http_client`, so a revert to a
    /// default client fails here.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn production_code_builds_only_the_redirect_free_credential_client() {
        let source = include_str!("runtime.rs");
        let production = &source[..source.find("\nmod tests {").unwrap()];
        for forbidden in [
            concat!("Client", "::new()"),
            concat!("Client", "::default()"),
            concat!("Client", "::builder()"),
            concat!("ReqwestCopilotHttpTransport", "::default()"),
        ] {
            assert!(!production.contains(forbidden), "{forbidden}");
        }
        assert!(production.contains("credential_http_client()"));
    }
    // The N1 common trait is the predecessor of these regression tests. The
    // actual Copilot runtime and authorizer remain the credential/route owners;
    // ScriptedTransport replaces only the external token/model HTTP transport.
    mod controller_maintenance {
        use super::*;

        struct Fixture {
            client: Arc<dyn meerkat_llm_core::LlmClient>,
            authorizer: Arc<CopilotAuthorizer>,
            auth_lease: meerkat_core::handles::GeneratedAuthLeaseHandle,
            transport: Arc<ScriptedTransport>,
            factory_routes: Arc<Mutex<Vec<String>>>,
            dispatches: Arc<AtomicU64>,
        }

        impl Fixture {
            async fn new(capability_wrapper: bool) -> Self {
                let (auth_binding, binding) = validated_binding();
                let (env, auth_lease) = resolver_environment(&auth_binding).await;
                let transport = Arc::new(ScriptedTransport::new([
                    token_response("maintenance-derived-one"),
                    models_response(),
                    token_response("maintenance-derived-two"),
                    models_response(),
                ]));
                let runtime = Arc::new(CopilotRuntime::with_transport(transport.clone()));
                let resolved = runtime.resolve(&binding, &env).await.unwrap();
                let authorizer = resolved.concrete_authorizer();
                let connection = meerkat_llm_core::provider_runtime::ResolvedConnection {
                    provider: Provider::OpenAI,
                    backend: binding.backend(),
                    backend_profile: binding.backend_profile().clone(),
                    credential_identity: binding.credential_identity().clone(),
                    auth_lease: Arc::new(
                        meerkat_llm_core::provider_runtime::DynamicLease::from_authorizer(
                            resolved.authorizer(),
                            resolved.metadata().clone(),
                            crate::GITHUB_COPILOT_AUTHORIZER_LABEL,
                        ),
                    ),
                };
                let factory_routes = Arc::new(Mutex::new(Vec::new()));
                let routes = Arc::clone(&factory_routes);
                let dispatches = Arc::new(AtomicU64::new(0));
                let child_dispatches = Arc::clone(&dispatches);
                let factory: CopilotRouteClientFactory = Arc::new(move |route, _| {
                    routes.lock().push(route.api_base.clone());
                    Ok(Arc::new(DispatchCountingClient {
                        dispatches: Arc::clone(&child_dispatches),
                    }))
                });
                let routed = routed_client(
                    runtime,
                    connection,
                    Provider::OpenAI,
                    "gpt-test".to_string(),
                    factory,
                )
                .unwrap();
                // Exercise the capability wrapper against a real maintained
                // inner route, independently of its position in a provider stack.
                let client = if capability_wrapper {
                    capability_gated_client(
                        routed,
                        Some(crate::CopilotModelCapabilities::default()),
                    )
                } else {
                    routed
                };
                Self {
                    client,
                    authorizer,
                    auth_lease,
                    transport,
                    factory_routes,
                    dispatches,
                }
            }

            fn source_snapshot(&self) -> meerkat_core::handles::AuthLeaseSnapshot {
                self.auth_lease.snapshot(
                    &meerkat_core::handles::LeaseKey::from_credential_identity(&account_identity()),
                )
            }

            fn assert_route_untouched(&self) {
                assert_eq!(self.client.provider(), Provider::OpenAI);
                assert_eq!(
                    self.factory_routes.lock().as_slice(),
                    ["https://copilot.example.test"],
                    "request-free maintenance must not reconstruct/reselect a route"
                );
                assert_eq!(self.dispatches.load(Ordering::SeqCst), 0);
                assert_eq!(
                    self.authorizer.api_base().as_deref(),
                    Some("https://copilot.example.test")
                );
                assert_eq!(
                    self.authorizer
                        .model_snapshot()
                        .unwrap()
                        .model("gpt-test")
                        .unwrap()
                        .route_for(Provider::OpenAI),
                    Some(CopilotEndpoint::Responses)
                );
            }
        }

        async fn remint(capability_wrapper: bool) {
            let fixture = Fixture::new(capability_wrapper).await;
            let source = fixture.source_snapshot();
            assert_eq!(
                source.phase,
                Some(meerkat_core::handles::AuthLeasePhase::Valid)
            );
            assert_eq!(fixture.transport.request_count(), 2);
            fixture
                .client
                .prepare_controller_credential()
                .await
                .unwrap();
            assert_eq!(
                fixture.transport.request_count(),
                2,
                "fresh maintenance is cached"
            );

            let mut headers = Vec::new();
            let first = fixture
                .authorizer
                .authorize_with_receipt(&mut HttpAuthorizationRequest {
                    method: "POST",
                    url: "https://copilot.example.test/responses",
                    headers: &mut headers,
                })
                .await
                .unwrap();
            assert!(
                headers
                    .iter()
                    .any(|(name, value)| name.eq_ignore_ascii_case("authorization")
                        && value == "Bearer maintenance-derived-one")
            );
            assert_eq!(
                fixture
                    .authorizer
                    .observe_response_with_receipt(
                        first,
                        &HttpAuthorizationResponse {
                            method: "POST",
                            url: "https://copilot.example.test/responses",
                            status: 401,
                        },
                    )
                    .await
                    .unwrap(),
                HttpAuthorizationResponseAction::RetryWithFreshAuthorization
            );
            // The observation refuses the token (the preparation below mints
            // a new one) but keeps the account's route resolvable.
            assert_eq!(
                fixture.authorizer.api_base().as_deref(),
                Some("https://copilot.example.test"),
                "a refused token keeps the account's API base"
            );

            fixture
                .client
                .prepare_controller_credential()
                .await
                .unwrap();
            assert_eq!(
                fixture.transport.request_count(),
                4,
                "the real authorizer must remint and discover before any model request"
            );
            let mut headers = Vec::new();
            let second = fixture
                .authorizer
                .authorize_with_receipt(&mut HttpAuthorizationRequest {
                    method: "POST",
                    url: "https://copilot.example.test/responses",
                    headers: &mut headers,
                })
                .await
                .unwrap();
            assert_ne!(first, second);
            assert!(
                headers
                    .iter()
                    .any(|(name, value)| name.eq_ignore_ascii_case("authorization")
                        && value == "Bearer maintenance-derived-two")
            );
            assert_eq!(
                fixture.transport.request_count(),
                4,
                "the positive readback cannot rescue missing preparation"
            );
            assert_eq!(
                fixture.source_snapshot(),
                source,
                "derived Copilot maintenance cannot invent a source credential generation"
            );
            fixture.assert_route_untouched();
            let requests = fixture.transport.requests.lock();
            for index in [0, 2] {
                assert_eq!(
                    requests[index].1.get("authorization").unwrap(),
                    "token github-source-token"
                );
            }
            for index in [1, 3] {
                assert_eq!(
                    requests[index].1.get("openai-intent").unwrap(),
                    "model-access"
                );
            }
        }

        async fn reauth(capability_wrapper: bool) {
            let fixture = Fixture::new(capability_wrapper).await;
            let key =
                meerkat_core::handles::LeaseKey::from_credential_identity(&account_identity());
            fixture.auth_lease.mark_reauth_required(&key).unwrap();
            let refused = fixture.source_snapshot();
            assert_eq!(
                refused.phase,
                Some(meerkat_core::handles::AuthLeasePhase::ReauthRequired)
            );
            let result = fixture.client.prepare_controller_credential().await;
            assert!(
                matches!(result, Err(AuthError::UserReauthRequired)),
                "{result:?}"
            );
            assert_eq!(fixture.source_snapshot(), refused);
            assert_eq!(
                fixture.transport.request_count(),
                2,
                "the generated reauth decision must arrive without extra token/model transport"
            );
            fixture.assert_route_untouched();
        }

        #[tokio::test]
        async fn routed_maintenance_uses_retained_real_authorizer() {
            remint(false).await;
        }

        #[tokio::test]
        async fn capability_maintenance_forwards_to_real_inner_route() {
            remint(true).await;
        }

        #[tokio::test]
        async fn routed_maintenance_preserves_typed_reauth() {
            reauth(false).await;
        }

        #[tokio::test]
        async fn capability_maintenance_preserves_typed_reauth() {
            reauth(true).await;
        }
    }

    /// Rows of the operator-token mode (`github_copilot_token`): the
    /// operator's GitHub token is the Copilot API bearer, its lifecycle lives
    /// in the shared account's AuthMachine lease, and it reaches no error, log
    /// line or debug rendering. The transports replace only GitHub's and
    /// Copilot's HTTP; the runtime, the credential resolver and a real
    /// generated AuthMachine run as in production.
    mod operator_token {
        use super::*;
        use crate::CopilotProviderRoute;
        use meerkat_auth_core::github_copilot::GITHUB_COPILOT_USER_URL;
        use meerkat_core::handles::{AuthLeasePhase, AuthLeaseSnapshot};

        /// Every operator token in these rows carries this canary.
        const CANARY: &str = "c4n4ry0perat0rS3cret";
        const BUSINESS_API: &str = "https://api.business.githubcopilot.com";

        fn user_token(label: &str) -> String {
            format!("github_pat_{CANARY}_{label}")
        }

        fn user_response(login: &str, plan: &str, api: &str) -> CopilotHttpResponse {
            json_response(
                200,
                serde_json::json!({
                    "login": login,
                    "copilot_plan": plan,
                    "chat_enabled": true,
                    "endpoints": {"api": api}
                }),
            )
        }

        fn svc_bot() -> CopilotHttpResponse {
            user_response("svc-bot", "business", BUSINESS_API)
        }

        fn rate_limited(status: u16, retry_after_seconds: u64) -> CopilotHttpResponse {
            CopilotHttpResponse {
                status,
                body: br#"{"message":"API rate limit exceeded for user"}"#.to_vec(),
                retry_after: Some(StdDuration::from_secs(retry_after_seconds)),
                rate_limited: true,
                rate_limit_reset: None,
            }
        }

        fn env_source() -> CredentialSourceSpec {
            CredentialSourceSpec::Env {
                env: "COPILOT_GITHUB_TOKEN".to_string(),
                fallback: vec!["GH_TOKEN".to_string(), "GITHUB_TOKEN".to_string()],
            }
        }

        /// A token route of the shared `github_copilot` account, written with
        /// wire strings.
        fn token_binding(
            route: CopilotProviderRoute,
            source: CredentialSourceSpec,
        ) -> ValidatedBinding {
            token_binding_with_options(route, source, serde_json::Value::Null)
        }

        /// A token route whose backend sets `options`.
        fn token_binding_with_options(
            route: CopilotProviderRoute,
            source: CredentialSourceSpec,
            options: serde_json::Value,
        ) -> ValidatedBinding {
            let auth_binding = AuthBindingRef {
                realm: RealmId::global(),
                binding: BindingId::parse(route.binding_id()).expect("valid binding"),
                profile: None,
                origin: BindingOrigin::Configured,
            };
            let backend = BackendProfile {
                id: route.binding_id().to_string(),
                provider: route.provider(),
                backend_kind: "copilot".to_string(),
                base_url: None,
                options,
                server: None,
            };
            let auth = AuthProfile {
                id: route.binding_id().to_string(),
                provider: route.provider(),
                auth_method: "github_copilot_token".to_string(),
                source,
                constraints: Default::default(),
                metadata_defaults: Default::default(),
            };
            ProviderRuntimeCatalog::validate_binding_with_credential_identity(
                &auth_binding,
                account_identity(),
                &backend,
                &auth,
                &BindingPolicy::default(),
            )
            .expect("valid Copilot token route")
        }

        type Outcome = Result<CopilotHttpResponse, CopilotHttpTransportError>;

        /// Answers each request with the next scripted outcome and records
        /// the request.
        struct OutcomeTransport {
            outcomes: Mutex<VecDeque<Outcome>>,
            requests: Mutex<Vec<(String, HeaderMap)>>,
        }

        impl OutcomeTransport {
            fn new(outcomes: impl IntoIterator<Item = Outcome>) -> Arc<Self> {
                Arc::new(Self {
                    outcomes: Mutex::new(outcomes.into_iter().collect()),
                    requests: Mutex::new(Vec::new()),
                })
            }

            fn request_count(&self) -> usize {
                self.requests.lock().len()
            }

            fn urls(&self) -> Vec<String> {
                self.requests
                    .lock()
                    .iter()
                    .map(|(url, _)| url.clone())
                    .collect()
            }

            fn bearer(&self, index: usize) -> Option<String> {
                self.header(index, "authorization")
            }

            /// The value of header `name` on the `index`th request.
            fn header(&self, index: usize, name: &str) -> Option<String> {
                self.requests
                    .lock()
                    .get(index)
                    .and_then(|(_, headers)| headers.get(name))
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string)
            }
        }

        #[async_trait]
        impl CopilotHttpTransport for OutcomeTransport {
            async fn get(
                &self,
                url: &str,
                headers: HeaderMap,
            ) -> Result<CopilotHttpResponse, CopilotHttpTransportError> {
                self.requests.lock().push((url.to_string(), headers));
                self.outcomes.lock().pop_front().unwrap_or_else(|| {
                    Err(CopilotHttpTransportError::Scripted(
                        "unexpected Copilot HTTP request".to_string(),
                    ))
                })
            }
        }

        /// A resolver environment like a host's: provider-auth persistence, a
        /// real generated AuthMachine lease handle, environment variables the
        /// row sets, and a clock the row moves.
        struct OperatorEnv {
            env: ResolverEnvironment,
            auth_lease: meerkat_core::handles::GeneratedAuthLeaseHandle,
            vars: Arc<Mutex<BTreeMap<String, String>>>,
            clock: Arc<Mutex<DateTime<Utc>>>,
        }

        impl OperatorEnv {
            fn new() -> Self {
                Self::configured(|env| env)
            }

            fn configured(
                configure: impl FnOnce(ResolverEnvironment) -> ResolverEnvironment,
            ) -> Self {
                let machine = meerkat_runtime::MeerkatMachine::ephemeral();
                Self::build(machine.generated_auth_lease_handle(), configure)
            }

            /// An environment over a lease handle of a host's runtime
            /// machine.
            fn over(auth_lease: meerkat_core::handles::GeneratedAuthLeaseHandle) -> Self {
                Self::build(auth_lease, |env| env)
            }

            fn build(
                auth_lease: meerkat_core::handles::GeneratedAuthLeaseHandle,
                configure: impl FnOnce(ResolverEnvironment) -> ResolverEnvironment,
            ) -> Self {
                let persistence = ProviderAuthPersistence::new(
                    Arc::new(meerkat_auth_core::auth_store::EphemeralTokenStore::new()),
                    Arc::new(meerkat_auth_core::auth_store::InMemoryCoordinator::new()),
                );
                let vars = Arc::new(Mutex::new(BTreeMap::<String, String>::new()));
                let clock = Arc::new(Mutex::new(Utc::now()));
                let lookup = Arc::clone(&vars);
                let now = Arc::clone(&clock);
                let mut env = configure(
                    ResolverEnvironment::testing()
                        .with_provider_auth_persistence(persistence)
                        .with_auth_lease_handle(auth_lease.clone())
                        .with_env_lookup(move |name| lookup.lock().get(name).cloned()),
                );
                env.now = Arc::new(move || *now.lock());
                Self {
                    env,
                    auth_lease,
                    vars,
                    clock,
                }
            }

            fn set(&self, name: &str, value: &str) {
                self.vars.lock().insert(name.to_string(), value.to_string());
            }

            fn advance(&self, seconds: i64) {
                let mut clock = self.clock.lock();
                *clock += Duration::seconds(seconds);
            }

            fn lease(&self) -> AuthLeaseSnapshot {
                self.auth_lease.snapshot(&lease_key())
            }
        }

        /// The shared account's AuthMachine lease.
        fn lease_key() -> LeaseKey {
            LeaseKey::from_credential_identity(&account_identity())
        }

        /// A persistent runtime machine over `store`, as a host builds one;
        /// every machine over one store shares its AuthMachine lease registry.
        fn persistent_machine(
            store: &Arc<dyn meerkat_runtime::RuntimeStore>,
        ) -> meerkat_runtime::MeerkatMachine {
            meerkat_runtime::MeerkatMachine::persistent_without_blobs(Arc::clone(store))
                .expect("a runtime machine over the store")
        }

        /// A runtime whose GitHub and Copilot HTTP is scripted.
        struct Harness {
            runtime: CopilotRuntime,
            transport: Arc<OutcomeTransport>,
            operator: OperatorEnv,
        }

        impl Harness {
            fn new(outcomes: impl IntoIterator<Item = Outcome>) -> Self {
                Self::with_env(outcomes, OperatorEnv::new())
            }

            fn with_env(
                outcomes: impl IntoIterator<Item = Outcome>,
                operator: OperatorEnv,
            ) -> Self {
                let transport = OutcomeTransport::new(outcomes);
                let runtime = CopilotRuntime::with_transport(transport.clone());
                Self {
                    runtime,
                    transport,
                    operator,
                }
            }

            /// Resolve a route that reads `COPILOT_GITHUB_TOKEN`, then
            /// `GH_TOKEN`, then `GITHUB_TOKEN`.
            async fn resolve(
                &self,
                route: CopilotProviderRoute,
            ) -> Result<CopilotResolvedAuth, ProviderAuthError> {
                self.resolve_from(route, env_source()).await
            }

            async fn resolve_from(
                &self,
                route: CopilotProviderRoute,
                source: CredentialSourceSpec,
            ) -> Result<CopilotResolvedAuth, ProviderAuthError> {
                self.runtime
                    .resolve(&token_binding(route, source), &self.operator.env)
                    .await
            }
        }

        /// Authorize one Copilot request; return its receipt and bearer.
        async fn authorize_once(
            authorizer: &CopilotAuthorizer,
            url: &str,
        ) -> (HttpAuthorizationReceipt, String) {
            let mut headers = Vec::new();
            let receipt = authorizer
                .authorize_with_receipt(&mut HttpAuthorizationRequest {
                    method: "POST",
                    url,
                    headers: &mut headers,
                })
                .await
                .expect("authorize a Copilot request");
            let bearer = headers
                .into_iter()
                .find(|(name, _)| name.eq_ignore_ascii_case("authorization"))
                .map(|(_, value)| value)
                .expect("an authorization header");
            (receipt, bearer)
        }

        /// Report a 401 for `receipt`: that account state is dropped and the
        /// request is retried with fresh authorization.
        async fn refuse_with_401(
            authorizer: &CopilotAuthorizer,
            url: &str,
            receipt: HttpAuthorizationReceipt,
        ) {
            assert_eq!(
                authorizer
                    .observe_response_with_receipt(
                        receipt,
                        &HttpAuthorizationResponse {
                            method: "POST",
                            url,
                            status: 401,
                        },
                    )
                    .await
                    .expect("observe the 401"),
                HttpAuthorizationResponseAction::RetryWithFreshAuthorization
            );
        }

        fn refresh_failure(error: ProviderAuthError) -> String {
            match error {
                ProviderAuthError::Auth(AuthError::RefreshFailed(detail)) => detail,
                other => panic!("expected refresh_failed, got {other:?}"),
            }
        }

        /// T1: the operator's token is the Copilot API bearer for the
        /// identity lookup, the per-plan model listing and every request; the
        /// device flow's token exchange is never called.
        #[tokio::test]
        async fn operator_token_is_the_copilot_bearer() {
            let token = user_token("primary");
            let harness = Harness::new([Ok(svc_bot()), Ok(models_response())]);
            harness.operator.set("COPILOT_GITHUB_TOKEN", &token);

            let resolved = harness
                .resolve(CopilotProviderRoute::OpenAi)
                .await
                .expect("the operator token resolves");

            let bearer = format!("Bearer {token}");
            assert_eq!(
                harness.transport.urls(),
                [
                    GITHUB_COPILOT_USER_URL.to_string(),
                    format!("{BUSINESS_API}/models")
                ]
            );
            assert_eq!(harness.transport.bearer(0), Some(bearer.clone()));
            assert_eq!(harness.transport.bearer(1), Some(bearer.clone()));
            let authorizer = resolved.concrete_authorizer();
            let (_, request_bearer) =
                authorize_once(&authorizer, &format!("{BUSINESS_API}/responses")).await;
            assert_eq!(request_bearer, bearer);
            assert_eq!(
                harness.transport.request_count(),
                2,
                "requests reuse the account state"
            );
            assert!(
                harness
                    .transport
                    .urls()
                    .iter()
                    .all(|url| !url.contains("v2/token")),
                "the operator token is never exchanged"
            );
            assert_eq!(harness.operator.lease().phase, Some(AuthLeasePhase::Valid));
            assert_eq!(resolved.metadata().account_id.as_deref(), Some("svc-bot"));
            assert_eq!(resolved.metadata().plan.as_deref(), Some("business"));
            let identity = authorizer.account_identity().expect("account identity");
            assert_eq!(identity.api_base, BUSINESS_API);
            assert!(!identity.installation);
        }

        /// The operator's token travels in `Authorization` values marked
        /// sensitive (the identity lookup and the model listing), so the HTTP
        /// stack keeps it out of HPACK tables.
        #[tokio::test]
        async fn operator_bearer_headers_are_sensitive() {
            let harness = Harness::new([Ok(svc_bot()), Ok(models_response())]);
            harness
                .operator
                .set("COPILOT_GITHUB_TOKEN", &user_token("sensitive"));
            harness
                .resolve(CopilotProviderRoute::OpenAi)
                .await
                .expect("the operator token resolves");

            let requests = harness.transport.requests.lock();
            assert_eq!(requests.len(), 2);
            for (url, headers) in requests.iter() {
                assert!(
                    headers
                        .get("authorization")
                        .is_some_and(|value| value.is_sensitive()),
                    "{url}"
                );
            }
        }

        /// Sign one Copilot request; return the value of its header `name`.
        async fn signed_header(
            authorizer: &CopilotAuthorizer,
            url: &str,
            name: &str,
        ) -> Option<String> {
            let mut headers = Vec::new();
            authorizer
                .authorize_with_receipt(&mut HttpAuthorizationRequest {
                    method: "POST",
                    url,
                    headers: &mut headers,
                })
                .await
                .expect("authorize a Copilot request");
            headers
                .into_iter()
                .find(|(header, _)| header.eq_ignore_ascii_case(name))
                .map(|(_, value)| value)
        }

        /// The `Copilot-Integration-Id` of a resolved token route's identity
        /// lookup, its model listing and one request it signs, in that order.
        async fn token_route_integration_ids(
            harness: &Harness,
            resolved: &CopilotResolvedAuth,
        ) -> Vec<Option<String>> {
            let signed = signed_header(
                &resolved.concrete_authorizer(),
                &format!("{BUSINESS_API}/responses"),
                "copilot-integration-id",
            )
            .await;
            assert_eq!(
                harness.transport.urls(),
                [
                    GITHUB_COPILOT_USER_URL.to_string(),
                    format!("{BUSINESS_API}/models")
                ]
            );
            vec![
                harness.transport.header(0, "copilot-integration-id"),
                harness.transport.header(1, "copilot-integration-id"),
                signed,
            ]
        }

        /// A token route whose backend options set no `integration_id` sends
        /// `Copilot-Integration-Id: copilot-developer-cli` on every Copilot
        /// API request that carries the operator token: the model listing and
        /// inference. That is the integration id the Copilot API accepts for a
        /// GitHub token sent as its bearer, a personal access token included;
        /// it refuses `vscode-chat`, the device sign-in's default, for one.
        /// GitHub's identity lookup gets no integration id.
        #[tokio::test]
        async fn token_routes_send_the_cli_integration_id_by_default() {
            let harness = Harness::new([Ok(svc_bot()), Ok(models_response())]);
            harness
                .operator
                .set("COPILOT_GITHUB_TOKEN", &user_token("integration"));
            let resolved = harness
                .resolve(CopilotProviderRoute::OpenAi)
                .await
                .expect("the operator token resolves");

            assert_eq!(
                token_route_integration_ids(&harness, &resolved).await,
                vec![
                    None,
                    Some("copilot-developer-cli".to_string()),
                    Some("copilot-developer-cli".to_string())
                ],
                "identity lookup, model listing, inference"
            );
        }

        /// An `integration_id` the token route's backend options set is kept on
        /// every Copilot API request that carries the operator token; GitHub's
        /// identity lookup still gets none.
        #[tokio::test]
        async fn token_routes_keep_an_explicit_integration_id() {
            let harness = Harness::new([Ok(svc_bot()), Ok(models_response())]);
            harness
                .operator
                .set("COPILOT_GITHUB_TOKEN", &user_token("explicit"));
            let binding = token_binding_with_options(
                CopilotProviderRoute::OpenAi,
                env_source(),
                serde_json::json!({"integration_id": "enterprise-approved"}),
            );
            let resolved = harness
                .runtime
                .resolve(&binding, &harness.operator.env)
                .await
                .expect("the operator token resolves");

            assert_eq!(
                token_route_integration_ids(&harness, &resolved).await,
                vec![
                    None,
                    Some("enterprise-approved".to_string()),
                    Some("enterprise-approved".to_string())
                ],
                "identity lookup, model listing, inference"
            );
        }

        /// T2: the documented route reads `COPILOT_GITHUB_TOKEN`, then
        /// `GH_TOKEN`, then `GITHUB_TOKEN`, each after its `RKAT_` form, as
        /// GitHub Copilot CLI does.
        #[tokio::test]
        async fn env_fallback_order_matches_copilot_cli() {
            let section: meerkat_core::RealmConfigSection =
                serde_json::from_value(serde_json::json!({
                    "backend": {
                        "copilot_anthropic": {"provider": "anthropic", "backend_kind": "copilot"}
                    },
                    "auth": {
                        "copilot_anthropic": {
                            "provider": "anthropic",
                            "auth_method": "github_copilot_token",
                            "source": {
                                "kind": "env",
                                "env": "COPILOT_GITHUB_TOKEN",
                                "fallback": ["GH_TOKEN", "GITHUB_TOKEN"]
                            }
                        }
                    },
                    "binding": {
                        "copilot_anthropic": {
                            "backend_profile": "copilot_anthropic",
                            "auth_profile": "copilot_anthropic",
                            "credential_account": "github_copilot"
                        }
                    }
                }))
                .expect("the documented token route");
            let realm = meerkat_core::RealmConnectionSet::from_config("global", &section)
                .expect("a valid realm");
            let auth_binding = AuthBindingRef {
                realm: RealmId::global(),
                binding: BindingId::parse("copilot_anthropic").expect("valid binding"),
                profile: None,
                origin: BindingOrigin::Configured,
            };
            let (route, backend, auth) =
                realm.lookup_auth_binding(&auth_binding).expect("the route");
            let binding = ProviderRuntimeCatalog::validate_binding_with_credential_identity(
                &auth_binding,
                route.credential_identity(&auth_binding),
                backend,
                auth,
                &route.policy,
            )
            .expect("a valid token route");

            for (variables, expected) in [
                (&[("GH_TOKEN", "gh"), ("GITHUB_TOKEN", "github")][..], "gh"),
                (
                    &[
                        ("COPILOT_GITHUB_TOKEN", "copilot"),
                        ("GH_TOKEN", "gh"),
                        ("GITHUB_TOKEN", "github"),
                    ][..],
                    "copilot",
                ),
                (
                    &[
                        ("RKAT_COPILOT_GITHUB_TOKEN", "rkat"),
                        ("COPILOT_GITHUB_TOKEN", "copilot"),
                        ("GH_TOKEN", "gh"),
                    ][..],
                    "rkat",
                ),
            ] {
                let harness = Harness::new([Ok(svc_bot()), Ok(models_response())]);
                for (name, label) in variables {
                    harness.operator.set(name, &user_token(label));
                }
                harness
                    .runtime
                    .resolve(&binding, &harness.operator.env)
                    .await
                    .expect("an operator token resolves");
                assert_eq!(
                    harness.transport.bearer(0),
                    Some(format!("Bearer {}", user_token(expected))),
                    "{expected}"
                );
            }
        }

        /// A variable that is set but blank does not hide the next one: a CI
        /// secret that expands empty in `COPILOT_GITHUB_TOKEN` (or its `RKAT_`
        /// form) leaves `GH_TOKEN` in charge, as in GitHub Copilot CLI.
        #[tokio::test]
        async fn blank_variable_does_not_hide_the_next_one() {
            let harness = Harness::new([Ok(svc_bot()), Ok(models_response())]);
            harness.operator.set("RKAT_COPILOT_GITHUB_TOKEN", " ");
            harness.operator.set("COPILOT_GITHUB_TOKEN", "");
            harness.operator.set("GH_TOKEN", &user_token("gh"));

            harness
                .resolve(CopilotProviderRoute::OpenAi)
                .await
                .expect("GH_TOKEN is read past the blank variables");

            assert_eq!(
                harness.transport.bearer(0),
                Some(format!("Bearer {}", user_token("gh")))
            );
        }

        /// T3a: a Copilot 401 on a request drops that account state; the retry
        /// reads the source again, looks the identity and the models up once
        /// more, and runs under a new receipt. The token stays valid.
        #[tokio::test]
        async fn inference_401_rediscovers_and_retries_once() {
            let token = user_token("primary");
            let harness = Harness::new([
                Ok(svc_bot()),
                Ok(models_response()),
                Ok(svc_bot()),
                Ok(models_response()),
            ]);
            harness.operator.set("COPILOT_GITHUB_TOKEN", &token);
            let resolved = harness
                .resolve(CopilotProviderRoute::OpenAi)
                .await
                .expect("the operator token resolves");
            let authorizer = resolved.concrete_authorizer();
            let url = format!("{BUSINESS_API}/responses");

            let (first, _) = authorize_once(&authorizer, &url).await;
            refuse_with_401(&authorizer, &url, first).await;
            let (second, bearer) = authorize_once(&authorizer, &url).await;

            assert_ne!(first, second, "the retry runs under a new receipt");
            assert_eq!(bearer, format!("Bearer {token}"));
            assert_eq!(
                harness.transport.request_count(),
                4,
                "one identity and one model request more"
            );
            assert_eq!(harness.transport.urls()[2], GITHUB_COPILOT_USER_URL);
            assert_eq!(harness.operator.lease().phase, Some(AuthLeasePhase::Valid));
        }

        /// An external resolver that hands out the next scripted token.
        struct QueuedResolver(Mutex<VecDeque<String>>);

        #[async_trait]
        impl meerkat_llm_core::provider_runtime::ExternalAuthResolverHandle for QueuedResolver {
            async fn resolve(
                &self,
                _binding: &ValidatedBinding,
            ) -> Result<meerkat_core::ResolvedAuthEnvelope, AuthError> {
                let secret = self.0.lock().pop_front().ok_or(AuthError::MissingSecret)?;
                Ok(meerkat_core::ResolvedAuthEnvelope::InlineSecret {
                    secret,
                    metadata: AuthMetadata::default(),
                    expires_at: None,
                })
            }
        }

        /// T3b: a rotated token from an external resolver heals the request
        /// that met the 401, and the AuthMachine credential generation
        /// advances once per distinct token, not per read of the source.
        #[tokio::test]
        async fn rotated_token_is_published_and_rediscovered() {
            let first = user_token("first");
            let rotated = user_token("rotated");
            let resolver = Arc::new(QueuedResolver(Mutex::new(VecDeque::from([
                first.clone(),
                first.clone(),
                rotated.clone(),
            ]))));
            let operator = OperatorEnv::configured(|env| {
                env.with_external_resolver("copilot-token", resolver)
            });
            let harness = Harness::with_env(
                [
                    Ok(svc_bot()),
                    Ok(models_response()),
                    Ok(svc_bot()),
                    Ok(models_response()),
                    Ok(svc_bot()),
                    Ok(models_response()),
                ],
                operator,
            );
            let source = CredentialSourceSpec::ExternalResolver {
                handle: "copilot-token".into(),
            };
            let resolved = harness
                .resolve_from(CopilotProviderRoute::Gemini, source)
                .await
                .expect("the first token resolves");
            assert_eq!(harness.operator.lease().generation, 1);
            let authorizer = resolved.concrete_authorizer();
            let url = format!("{BUSINESS_API}/chat/completions");

            let (receipt, bearer) = authorize_once(&authorizer, &url).await;
            assert_eq!(bearer, format!("Bearer {first}"));
            refuse_with_401(&authorizer, &url, receipt).await;
            let (receipt, bearer) = authorize_once(&authorizer, &url).await;
            assert_eq!(
                bearer,
                format!("Bearer {first}"),
                "the resolver re-asserted the same token"
            );
            assert_eq!(
                harness.operator.lease().generation,
                1,
                "the same token is not published again"
            );
            refuse_with_401(&authorizer, &url, receipt).await;
            let (_, bearer) = authorize_once(&authorizer, &url).await;

            assert_eq!(bearer, format!("Bearer {rotated}"));
            assert_eq!(
                harness.operator.lease().generation,
                2,
                "one publication per distinct token"
            );
            assert_eq!(harness.operator.lease().phase, Some(AuthLeasePhase::Valid));
            assert_eq!(harness.transport.request_count(), 6);
        }

        /// T3c: a command source that declares `refresh_interval_ms` publishes
        /// its token with that expiry; once the AuthMachine reports the
        /// refresh window, the next request runs the command again and sends
        /// its new token.
        #[cfg(unix)]
        #[tokio::test]
        async fn command_source_rereads_when_authmachine_requires_refresh() {
            let directory = tempfile::tempdir().expect("command directory");
            let source = CredentialSourceSpec::Command {
                program: "/bin/sh".into(),
                args: vec![
                    "-c".to_string(),
                    format!(
                        "n=0; [ -f count ] && read n < count; n=$((n + 1)); echo \"$n\" > count; printf 'github_pat_{CANARY}_command_%s' \"$n\""
                    ),
                ],
                cwd: Some(directory.path().to_path_buf()),
                env: BTreeMap::new(),
                timeout_ms: 10_000,
                refresh_interval_ms: Some(600_000),
            };
            let harness = Harness::new([
                Ok(svc_bot()),
                Ok(models_response()),
                Ok(svc_bot()),
                Ok(models_response()),
            ]);
            let resolved = harness
                .resolve_from(CopilotProviderRoute::OpenAi, source)
                .await
                .expect("the command's token resolves");
            let authorizer = resolved.concrete_authorizer();
            let url = format!("{BUSINESS_API}/responses");
            let (_, bearer) = authorize_once(&authorizer, &url).await;
            assert_eq!(bearer, format!("Bearer github_pat_{CANARY}_command_1"));
            assert_eq!(harness.transport.request_count(), 2);

            // Inside the AuthMachine's refresh window: 60 s before the
            // declared 600 s.
            harness.operator.advance(545);
            let (_, bearer) = authorize_once(&authorizer, &url).await;

            assert_eq!(
                bearer,
                format!("Bearer github_pat_{CANARY}_command_2"),
                "the command ran again"
            );
            assert_eq!(harness.transport.request_count(), 4);
            assert_eq!(harness.operator.lease().phase, Some(AuthLeasePhase::Valid));
        }

        /// A command source that mints `github_pat_<canary>_command_<n>`, `n`
        /// counting its runs in `directory`.
        #[cfg(unix)]
        fn counter_command(
            directory: &std::path::Path,
            refresh_interval_ms: Option<u64>,
        ) -> CredentialSourceSpec {
            CredentialSourceSpec::Command {
                program: "/bin/sh".into(),
                args: vec![
                    "-c".to_string(),
                    format!(
                        "n=0; [ -f count ] && read n < count; n=$((n + 1)); echo \"$n\" > count; printf 'github_pat_{CANARY}_command_%s' \"$n\""
                    ),
                ],
                cwd: Some(directory.to_path_buf()),
                env: BTreeMap::new(),
                timeout_ms: 10_000,
                refresh_interval_ms,
            }
        }

        /// A command source whose refreshed token GitHub refuses heals with
        /// the next token it mints: the refusal drops the account state built
        /// for the earlier token, so once the paced minute is over the next
        /// request runs the command again instead of failing on that state,
        /// and its new token is published.
        #[cfg(unix)]
        #[tokio::test]
        async fn command_source_heals_after_a_refusal_at_the_refresh() {
            let directory = tempfile::tempdir().expect("command directory");
            let harness = Harness::new([
                Ok(svc_bot()),
                Ok(models_response()),
                Ok(json_response(
                    401,
                    serde_json::json!({"message": "Bad credentials"}),
                )),
                Ok(svc_bot()),
                Ok(models_response()),
            ]);
            let resolved = harness
                .resolve_from(
                    CopilotProviderRoute::OpenAi,
                    counter_command(directory.path(), Some(600_000)),
                )
                .await
                .expect("the first token resolves");
            let authorizer = resolved.concrete_authorizer();
            let url = format!("{BUSINESS_API}/responses");
            let (_, bearer) = authorize_once(&authorizer, &url).await;
            assert_eq!(bearer, format!("Bearer github_pat_{CANARY}_command_1"));

            // Inside the refresh window the command mints a token GitHub
            // refuses.
            harness.operator.advance(545);
            assert_eq!(
                authorizer.prepare_request().await,
                Err(AuthError::UserReauthRequired)
            );
            assert_eq!(
                harness.operator.lease().phase,
                Some(AuthLeasePhase::ReauthRequired)
            );
            assert_eq!(harness.transport.request_count(), 3);

            // Until the paced instant the command is not run again.
            harness.operator.advance(30);
            assert_eq!(
                authorizer.prepare_request().await,
                Err(AuthError::UserReauthRequired)
            );
            assert_eq!(command_runs(directory.path()), 2);

            // Then the next request runs the command again, and its token
            // heals the account.
            harness.operator.advance(31);
            let (_, bearer) = authorize_once(&authorizer, &url).await;
            assert_eq!(
                bearer,
                format!("Bearer github_pat_{CANARY}_command_3"),
                "the command ran again after the refusal"
            );
            assert_eq!(harness.operator.lease().phase, Some(AuthLeasePhase::Valid));
            assert_eq!(harness.transport.request_count(), 5);
        }

        /// The lease publication an agent build makes once it resolved a
        /// route, as the facade's `AgentFactory::publish_auth_lease` does:
        /// none while the lease carries a credential, is valid or expiring and
        /// has the expiry the connection's lease reports; otherwise a
        /// publication with that expiry, naming no value.
        #[cfg(unix)]
        fn publish_as_an_agent_build(
            auth_lease: &meerkat_core::handles::GeneratedAuthLeaseHandle,
            connection_lease: &dyn meerkat_core::AuthLease,
        ) {
            let expires_at = connection_lease
                .expires_at()
                .map_or(u64::MAX, |at| at.timestamp().max(0) as u64);
            let snapshot = auth_lease.snapshot(&lease_key());
            if snapshot.credential_present
                && snapshot.expires_at.unwrap_or(u64::MAX) == expires_at
                && matches!(
                    snapshot.phase,
                    Some(AuthLeasePhase::Valid | AuthLeasePhase::Expiring)
                )
            {
                return;
            }
            auth_lease
                .acquire_lease(&lease_key(), expires_at)
                .expect("the build publishes the lease");
        }

        /// One agent build's auth step on `harness`: resolve the route over
        /// `source`, then the build's lease publication over the
        /// connection's lease, as a provider runtime builds it.
        #[cfg(unix)]
        async fn build_session(
            harness: &Harness,
            source: CredentialSourceSpec,
        ) -> CopilotResolvedAuth {
            let resolved = harness
                .resolve_from(CopilotProviderRoute::OpenAi, source)
                .await
                .expect("the route resolves");
            let connection_lease =
                meerkat_llm_core::provider_runtime::DynamicLease::from_authorizer(
                    resolved.authorizer(),
                    resolved.metadata().clone(),
                    crate::GITHUB_COPILOT_AUTHORIZER_LABEL,
                );
            publish_as_an_agent_build(&harness.operator.auth_lease, &connection_lease);
            resolved
        }

        /// Within a command source's `refresh_interval_ms`, a second session
        /// build reuses the account's lease: the route reports the expiry of
        /// the publication it serves, so the build's own lease publication
        /// publishes nothing, and the second build neither runs the command
        /// nor looks the account up again.
        #[cfg(unix)]
        #[tokio::test]
        async fn session_builds_within_the_refresh_interval_reuse_the_lease() {
            let directory = tempfile::tempdir().expect("command directory");
            let harness = Harness::new([
                Ok(svc_bot()),
                Ok(models_response()),
                Ok(svc_bot()),
                Ok(models_response()),
            ]);
            let source = counter_command(directory.path(), Some(600_000));

            let first = build_session(&harness, source.clone()).await;
            harness.operator.advance(60);
            let second = build_session(&harness, source).await;

            assert_eq!(
                command_runs(directory.path()),
                1,
                "one command run for both builds"
            );
            assert_eq!(
                harness.transport.request_count(),
                2,
                "one identity lookup and one model listing for both builds"
            );
            let lease = harness.operator.lease();
            assert_eq!(lease.generation, 1, "no build publishes the lease again");
            assert_eq!(
                first
                    .authorizer()
                    .expires_at()
                    .map(|at| at.timestamp().max(0) as u64),
                lease.expires_at,
                "the route reports the expiry of the publication it serves"
            );
            let (_, bearer) = authorize_once(
                &second.concrete_authorizer(),
                &format!("{BUSINESS_API}/responses"),
            )
            .await;
            assert_eq!(bearer, format!("Bearer github_pat_{CANARY}_command_1"));
            assert_eq!(command_runs(directory.path()), 1);
        }

        /// A command source that counts its runs in `directory` and prints
        /// the token the row writes to `directory/token`.
        #[cfg(unix)]
        fn token_file_command(directory: &std::path::Path) -> CredentialSourceSpec {
            CredentialSourceSpec::Command {
                program: "/bin/sh".into(),
                args: vec![
                    "-c".to_string(),
                    "n=0; [ -f count ] && read n < count; n=$((n + 1)); echo \"$n\" > count; cat token"
                        .to_string(),
                ],
                cwd: Some(directory.to_path_buf()),
                env: BTreeMap::new(),
                timeout_ms: 10_000,
                refresh_interval_ms: None,
            }
        }

        /// How many times a counting command ran in `directory`.
        #[cfg(unix)]
        fn command_runs(directory: &std::path::Path) -> u32 {
            std::fs::read_to_string(directory.join("count"))
                .map_or(0, |count| count.trim().parse().expect("a run count"))
        }

        /// After GitHub refuses a token from a source that runs something, the
        /// source is read again at most once a minute while the refusal
        /// stands: requests in between fail as `user_reauth_required` without
        /// running the command, a read that gives the refused token again
        /// waits another minute, and a new publication on the lease (another
        /// session, machine or login published a value) ends the pause at
        /// once.
        #[cfg(unix)]
        #[tokio::test]
        async fn refused_command_token_rereads_are_paced() {
            let directory = tempfile::tempdir().expect("command directory");
            let refused = user_token("refused-command");
            std::fs::write(directory.path().join("token"), &refused).expect("write the token");
            let harness = Harness::new([
                Ok(json_response(
                    401,
                    serde_json::json!({"message": "Bad credentials"}),
                )),
                Ok(svc_bot()),
                Ok(models_response()),
            ]);
            let source = token_file_command(directory.path());
            let resolve = || harness.resolve_from(CopilotProviderRoute::OpenAi, source.clone());
            let reauth = Some(ProviderAuthError::Auth(AuthError::UserReauthRequired));
            assert_eq!(resolve().await.err(), reauth);
            assert_eq!(command_runs(directory.path()), 1);
            assert_eq!(harness.transport.request_count(), 1);

            // Within the minute no request runs the command.
            assert_eq!(resolve().await.err(), reauth);
            harness.operator.advance(59);
            assert_eq!(resolve().await.err(), reauth);
            assert_eq!(
                command_runs(directory.path()),
                1,
                "no read before the paced instant"
            );

            // At the paced instant the command runs. It gives the refused
            // token again, which fails fast and waits another minute.
            harness.operator.advance(2);
            assert_eq!(resolve().await.err(), reauth);
            assert_eq!(command_runs(directory.path()), 2);
            harness.operator.advance(30);
            assert_eq!(resolve().await.err(), reauth);
            assert_eq!(command_runs(directory.path()), 2);
            assert_eq!(
                harness.transport.request_count(),
                1,
                "the refused token is never sent again"
            );

            // Another session publishes a new token on the lease: the pause
            // ends at once, and the command's new token heals the account.
            let rotated = user_token("rotated-command");
            std::fs::write(directory.path().join("token"), &rotated).expect("rotate the token");
            harness
                .operator
                .auth_lease
                .publish_source_credential(
                    &lease_key(),
                    &CredentialFingerprint::of_secret(&rotated),
                    u64::MAX,
                )
                .expect("another session publishes the new token");
            resolve().await.expect("the new token heals the account");
            assert_eq!(command_runs(directory.path()), 3);
            assert_eq!(harness.transport.request_count(), 3);
            assert_eq!(
                harness.transport.bearer(1),
                Some(format!("Bearer {rotated}"))
            );
            assert_eq!(harness.operator.lease().phase, Some(AuthLeasePhase::Valid));
        }

        /// A read at the paced instant that fails (here a command that exits
        /// non-zero) is paced as a read that gives the refused token again
        /// is: the instant moves a pause further before the command runs, so
        /// across several requests the failing command runs at most once per
        /// pause.
        #[cfg(unix)]
        #[tokio::test]
        async fn a_failing_reread_of_a_refused_source_is_paced() {
            let directory = tempfile::tempdir().expect("command directory");
            std::fs::write(
                directory.path().join("token"),
                user_token("refused-then-failing"),
            )
            .expect("write the token");
            let harness = Harness::new([Ok(json_response(
                401,
                serde_json::json!({"message": "Bad credentials"}),
            ))]);
            let source = token_file_command(directory.path());
            let resolve = || harness.resolve_from(CopilotProviderRoute::OpenAi, source.clone());
            let reauth = Some(ProviderAuthError::Auth(AuthError::UserReauthRequired));
            assert_eq!(resolve().await.err(), reauth);
            assert_eq!(command_runs(directory.path()), 1);

            // From now on the command fails: it has no token to print.
            std::fs::remove_file(directory.path().join("token")).expect("remove the token");
            harness.operator.advance(61);
            let failed = resolve().await.err().expect("the command fails");
            assert_ne!(Some(failed), reauth, "the read at the paced instant ran");
            assert_eq!(command_runs(directory.path()), 2);
            for _ in 0..3 {
                assert_eq!(resolve().await.err(), reauth);
            }
            harness.operator.advance(59);
            assert_eq!(resolve().await.err(), reauth);
            assert_eq!(
                command_runs(directory.path()),
                2,
                "a failed read waits a pause like any other"
            );

            harness.operator.advance(2);
            resolve().await.err().expect("the command fails again");
            assert_eq!(resolve().await.err(), reauth);
            assert_eq!(command_runs(directory.path()), 3);
            assert_eq!(harness.transport.request_count(), 1);
        }

        /// An account state whose cache was built for a publication that
        /// another account state on the same lease saw refused (another
        /// provider registry, here another runtime) is not stuck on that
        /// cache: it drops it and reads its own source through the paced
        /// check, so once its pause is over it reads the healed source and
        /// heals, with no other publication on the lease.
        #[cfg(unix)]
        #[tokio::test]
        async fn a_refusal_seen_by_another_account_state_lets_this_one_heal() {
            let directory = tempfile::tempdir().expect("command directory");
            std::fs::write(directory.path().join("token"), user_token("revoked"))
                .expect("write the token");
            // This runtime only sees GitHub refuse the token.
            let harness = Harness::new([Ok(json_response(
                401,
                serde_json::json!({"message": "Bad credentials"}),
            ))]);
            let other_transport = OutcomeTransport::new([
                Ok(svc_bot()),
                Ok(models_response()),
                Ok(svc_bot()),
                Ok(models_response()),
            ]);
            let other = CopilotRuntime::with_transport(other_transport.clone());
            let source = token_file_command(directory.path());
            let cached = other
                .resolve(
                    &token_binding(CopilotProviderRoute::Anthropic, source.clone()),
                    &harness.operator.env,
                )
                .await
                .expect("the token resolves before GitHub revokes it");
            let authorizer = cached.concrete_authorizer();

            let reauth = Some(ProviderAuthError::Auth(AuthError::UserReauthRequired));
            assert_eq!(
                harness
                    .resolve_from(CopilotProviderRoute::OpenAi, source.clone())
                    .await
                    .err(),
                reauth
            );
            assert_eq!(
                harness.operator.lease().phase,
                Some(AuthLeasePhase::ReauthRequired)
            );
            assert_eq!(command_runs(directory.path()), 2);

            // The cached account state reads its source again: the same
            // refused token, which starts its own pause.
            assert_eq!(
                authorizer.prepare_request().await,
                Err(AuthError::UserReauthRequired)
            );
            assert_eq!(command_runs(directory.path()), 3);

            // The source heals; the account state reads it once the pause is
            // over, and nothing else publishes in between.
            let healed = user_token("healed");
            std::fs::write(directory.path().join("token"), &healed).expect("heal the token");
            harness.operator.advance(30);
            assert_eq!(
                authorizer.prepare_request().await,
                Err(AuthError::UserReauthRequired)
            );
            assert_eq!(command_runs(directory.path()), 3);
            harness.operator.advance(31);
            authorizer
                .prepare_request()
                .await
                .expect("the healed token heals the account state");
            assert_eq!(command_runs(directory.path()), 4);
            assert_eq!(harness.operator.lease().phase, Some(AuthLeasePhase::Valid));
            assert_eq!(other_transport.request_count(), 4);
            assert_eq!(other_transport.bearer(2), Some(format!("Bearer {healed}")));
        }

        /// After a logout released the account's token, a command source is
        /// paced as after a refusal: requests in between fail as
        /// `lease_absent` without running the command, the command runs again
        /// once per pause, and the logout stays in force for the same token.
        #[cfg(unix)]
        #[tokio::test]
        async fn logged_out_command_token_rereads_are_paced() {
            let directory = tempfile::tempdir().expect("command directory");
            std::fs::write(
                directory.path().join("token"),
                user_token("logged-out-command"),
            )
            .expect("write the token");
            let harness = Harness::new([Ok(svc_bot()), Ok(models_response())]);
            let source = token_file_command(directory.path());
            let resolve = || harness.resolve_from(CopilotProviderRoute::OpenAi, source.clone());
            resolve().await.expect("the token resolves");
            assert_eq!(command_runs(directory.path()), 1);
            harness
                .operator
                .auth_lease
                .release_lease(&lease_key())
                .expect("log out");

            let absent = Some(ProviderAuthError::Auth(AuthError::LeaseAbsent));
            assert_eq!(resolve().await.err(), absent);
            assert_eq!(command_runs(directory.path()), 2);
            assert_eq!(resolve().await.err(), absent);
            harness.operator.advance(59);
            assert_eq!(resolve().await.err(), absent);
            assert_eq!(
                command_runs(directory.path()),
                2,
                "no read before the paced instant"
            );

            harness.operator.advance(2);
            assert_eq!(resolve().await.err(), absent);
            assert_eq!(command_runs(directory.path()), 3);
            assert_eq!(harness.transport.request_count(), 2);
        }

        /// After a logout this runtime did not see coming, the first read of
        /// a command source starts the pause even when it fails: requests
        /// within the minute then fail as `lease_absent` without running the
        /// failing command, instead of each running it and returning its
        /// error.
        #[cfg(unix)]
        #[tokio::test]
        async fn a_failing_source_after_a_logout_is_paced_from_the_first_read() {
            let directory = tempfile::tempdir().expect("command directory");
            std::fs::write(
                directory.path().join("token"),
                user_token("logged-out-then-failing"),
            )
            .expect("write the token");
            let harness = Harness::new([Ok(svc_bot()), Ok(models_response())]);
            let source = token_file_command(directory.path());
            let resolve = || harness.resolve_from(CopilotProviderRoute::OpenAi, source.clone());
            resolve().await.expect("the token resolves");
            harness
                .operator
                .auth_lease
                .release_lease(&lease_key())
                .expect("log out");
            std::fs::remove_file(directory.path().join("token")).expect("remove the token");

            let absent = Some(ProviderAuthError::Auth(AuthError::LeaseAbsent));
            let failed = resolve().await.err().expect("the command fails");
            assert_ne!(Some(failed), absent, "the first read after the logout ran");
            assert_eq!(command_runs(directory.path()), 2);
            for _ in 0..3 {
                assert_eq!(resolve().await.err(), absent);
            }
            harness.operator.advance(59);
            assert_eq!(resolve().await.err(), absent);
            assert_eq!(
                command_runs(directory.path()),
                2,
                "no read within the minute, though the first one failed"
            );

            harness.operator.advance(2);
            resolve().await.err().expect("the command fails again");
            assert_eq!(command_runs(directory.path()), 3);
            assert_eq!(harness.transport.request_count(), 2);
        }

        /// While GitHub rate limits the account's identity lookup, a command
        /// source is not run for a token that lookup could not take: with no
        /// cached account, requests fail as the rate limit did, without
        /// running the command, until GitHub's stated time.
        #[cfg(unix)]
        #[tokio::test]
        async fn a_rate_limited_identity_lookup_pauses_command_reads() {
            let directory = tempfile::tempdir().expect("command directory");
            std::fs::write(
                directory.path().join("token"),
                user_token("rate-limited-command"),
            )
            .expect("write the token");
            let harness = Harness::new([
                Ok(rate_limited(403, 120)),
                Ok(svc_bot()),
                Ok(models_response()),
            ]);
            let source = token_file_command(directory.path());
            let resolve = || harness.resolve_from(CopilotProviderRoute::OpenAi, source.clone());
            let detail = refresh_failure(
                resolve()
                    .await
                    .err()
                    .expect("the identity lookup is rate limited"),
            );
            assert!(detail.contains("rate limited"), "{detail}");
            assert_eq!(command_runs(directory.path()), 1);

            harness.operator.advance(60);
            for _ in 0..3 {
                let detail = refresh_failure(
                    resolve()
                        .await
                        .err()
                        .expect("the account still waits for GitHub's time"),
                );
                assert!(detail.contains("rate limited"), "{detail}");
            }
            assert_eq!(
                command_runs(directory.path()),
                1,
                "no command run while GitHub's limit stands"
            );

            harness.operator.advance(61);
            resolve()
                .await
                .expect("the lookup runs again after the wait");
            assert_eq!(command_runs(directory.path()), 2);
            assert_eq!(harness.transport.request_count(), 3);
        }

        /// GitHub's secondary rate limit, a 403 whose message says so and that
        /// may carry no `Retry-After` and no rate-limit header, pauses the
        /// account's identity lookup like any rate limit, read through the
        /// production transport: requests fail as `refresh_failed` without
        /// asking GitHub again or running the token's command, for the minute
        /// GitHub asks for at least, and the lookup runs again after it. It
        /// is no refusal of the token.
        #[cfg(unix)]
        #[tokio::test]
        async fn a_secondary_rate_limit_pauses_the_identity_lookup() {
            let directory = tempfile::tempdir().expect("command directory");
            std::fs::write(
                directory.path().join("token"),
                user_token("secondary-limit"),
            )
            .expect("write the token");
            let user = String::from_utf8(svc_bot().body).expect("a JSON identity");
            let models = String::from_utf8(models_response().body).expect("a JSON model list");
            let (base, hits) = spawn_github_answers(vec![
                github_answer(403, "", SECONDARY_RATE_LIMIT_BODY),
                github_answer(200, "", &user),
                github_answer(200, "", &models),
            ])
            .await;
            let runtime = CopilotRuntime::with_transport(Arc::new(LocalGitHub {
                inner: ReqwestCopilotHttpTransport::new(),
                base,
            }));
            let operator = OperatorEnv::new();
            let binding = token_binding(
                CopilotProviderRoute::OpenAi,
                token_file_command(directory.path()),
            );
            let resolve = || runtime.resolve(&binding, &operator.env);

            let detail = refresh_failure(
                resolve()
                    .await
                    .err()
                    .expect("GitHub's secondary rate limit"),
            );
            assert!(detail.contains("rate limited"), "{detail}");
            assert_eq!(command_runs(directory.path()), 1);
            assert_eq!(operator.lease().phase, Some(AuthLeasePhase::Valid));

            operator.advance(59);
            for _ in 0..3 {
                let detail = refresh_failure(
                    resolve()
                        .await
                        .err()
                        .expect("the account still waits for GitHub's minute"),
                );
                assert!(detail.contains("rate limited"), "{detail}");
            }
            assert_eq!(
                hits.load(std::sync::atomic::Ordering::SeqCst),
                1,
                "no request inside GitHub's minute"
            );
            assert_eq!(
                command_runs(directory.path()),
                1,
                "no command run inside GitHub's minute"
            );

            operator.advance(2);
            resolve()
                .await
                .expect("the lookup runs again after the wait");
            assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 3);
            assert_eq!(command_runs(directory.path()), 2);
        }

        /// A logout during GitHub's rate-limit window keeps its own error:
        /// the read the pause lets through is refused for the rate limit
        /// without running the command, and the requests inside the pause
        /// it started fail as `lease_absent`, not as the rate limit, still
        /// without running the command.
        #[cfg(unix)]
        #[tokio::test]
        async fn a_logout_during_a_rate_limit_keeps_lease_absent() {
            let directory = tempfile::tempdir().expect("command directory");
            std::fs::write(
                directory.path().join("token"),
                user_token("logged-out-while-limited"),
            )
            .expect("write the token");
            let harness = Harness::new([
                Ok(svc_bot()),
                Ok(models_response()),
                Ok(rate_limited(403, 600)),
            ]);
            let source = token_file_command(directory.path());
            let resolve = || harness.resolve_from(CopilotProviderRoute::OpenAi, source.clone());
            resolve().await.expect("the token resolves");
            // A later account state's identity lookup is rate limited.
            let detail = refresh_failure(
                resolve()
                    .await
                    .err()
                    .expect("the identity lookup is rate limited"),
            );
            assert!(detail.contains("rate limited"), "{detail}");
            assert_eq!(command_runs(directory.path()), 2);

            harness
                .operator
                .auth_lease
                .release_lease(&lease_key())
                .expect("log out");
            let detail = refresh_failure(
                resolve()
                    .await
                    .err()
                    .expect("the read the pause lets through waits for GitHub"),
            );
            assert!(detail.contains("rate limited"), "{detail}");
            let absent = Some(ProviderAuthError::Auth(AuthError::LeaseAbsent));
            for _ in 0..3 {
                assert_eq!(resolve().await.err(), absent);
            }
            harness.operator.advance(59);
            assert_eq!(resolve().await.err(), absent);
            assert_eq!(
                command_runs(directory.path()),
                2,
                "no command run after the logout"
            );
            assert_eq!(harness.transport.request_count(), 3);
        }

        /// One account over two AuthMachine registries (here two credential
        /// stores, each with its own runtime machine) keeps a pause per
        /// registry: a refusal in the second registry does not overwrite
        /// the first registry's pause, so neither source is read again
        /// before its own paced instant.
        #[cfg(unix)]
        #[tokio::test]
        async fn withdrawn_source_pauses_are_kept_per_registry() {
            let first_directory = tempfile::tempdir().expect("first command directory");
            let second_directory = tempfile::tempdir().expect("second command directory");
            std::fs::write(
                first_directory.path().join("token"),
                user_token("first-registry"),
            )
            .expect("write the first token");
            std::fs::write(
                second_directory.path().join("token"),
                user_token("second-registry"),
            )
            .expect("write the second token");
            let harness = Harness::new([
                Ok(json_response(
                    401,
                    serde_json::json!({"message": "Bad credentials"}),
                )),
                Ok(svc_bot()),
                Ok(models_response()),
                Ok(json_response(
                    401,
                    serde_json::json!({"message": "Bad credentials"}),
                )),
            ]);
            let second_store = OperatorEnv::new();
            *second_store.clock.lock() = *harness.operator.clock.lock();
            let first = token_file_command(first_directory.path());
            let second = token_binding(
                CopilotProviderRoute::Anthropic,
                token_file_command(second_directory.path()),
            );
            let reauth = Some(ProviderAuthError::Auth(AuthError::UserReauthRequired));

            // The first registry's lease refuses its token at generation 1.
            assert_eq!(
                harness
                    .resolve_from(CopilotProviderRoute::OpenAi, first.clone())
                    .await
                    .err(),
                reauth
            );
            // The second registry's lease refuses its rotated token at
            // generation 2.
            harness
                .runtime
                .resolve(&second, &second_store.env)
                .await
                .expect("the second store's first token resolves");
            std::fs::write(
                second_directory.path().join("token"),
                user_token("second-registry-rotated"),
            )
            .expect("rotate the second token");
            assert_eq!(
                harness
                    .runtime
                    .resolve(&second, &second_store.env)
                    .await
                    .err(),
                reauth
            );
            assert_eq!(second_store.lease().generation, 2);
            assert_eq!(command_runs(first_directory.path()), 1);
            assert_eq!(command_runs(second_directory.path()), 2);

            // Within the minute neither source is read again.
            assert_eq!(
                harness
                    .resolve_from(CopilotProviderRoute::OpenAi, first.clone())
                    .await
                    .err(),
                reauth
            );
            assert_eq!(
                harness
                    .runtime
                    .resolve(&second, &second_store.env)
                    .await
                    .err(),
                reauth
            );
            assert_eq!(command_runs(first_directory.path()), 1);
            assert_eq!(command_runs(second_directory.path()), 2);
            assert_eq!(harness.transport.request_count(), 4);
        }

        /// A refusal of another account state's publication (another runtime
        /// read a newer token from the shared source) does not strand an
        /// account state cached for an earlier token: its next request reads
        /// the source again and publishes the newest token.
        #[tokio::test]
        async fn a_refusal_of_another_publication_rereads_the_source() {
            let harness = Harness::new([
                Ok(svc_bot()),
                Ok(models_response()),
                Ok(svc_bot()),
                Ok(models_response()),
            ]);
            let other_transport = OutcomeTransport::new([Ok(json_response(
                401,
                serde_json::json!({"message": "Bad credentials"}),
            ))]);
            let other = CopilotRuntime::with_transport(other_transport.clone());
            harness
                .operator
                .set("COPILOT_GITHUB_TOKEN", &user_token("cached"));
            let resolved = harness
                .resolve(CopilotProviderRoute::OpenAi)
                .await
                .expect("the first token resolves");

            harness
                .operator
                .set("COPILOT_GITHUB_TOKEN", &user_token("refused-elsewhere"));
            assert_eq!(
                other
                    .resolve(
                        &token_binding(CopilotProviderRoute::Anthropic, env_source()),
                        &harness.operator.env,
                    )
                    .await
                    .err(),
                Some(ProviderAuthError::Auth(AuthError::UserReauthRequired))
            );
            assert_eq!(
                harness.operator.lease().phase,
                Some(AuthLeasePhase::ReauthRequired)
            );

            let newest = user_token("newest");
            harness.operator.set("COPILOT_GITHUB_TOKEN", &newest);
            let authorizer = resolved.concrete_authorizer();
            let (_, bearer) =
                authorize_once(&authorizer, &format!("{BUSINESS_API}/responses")).await;
            assert_eq!(bearer, format!("Bearer {newest}"));
            assert_eq!(harness.operator.lease().phase, Some(AuthLeasePhase::Valid));
            assert_eq!(harness.transport.request_count(), 4);
            assert_eq!(other_transport.request_count(), 1);
        }

        /// Holds the first request until the row releases it, after telling
        /// the row it arrived; answers every request with the next scripted
        /// outcome.
        struct HeldFirstRequest {
            inner: Arc<OutcomeTransport>,
            arrived: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
            release: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
        }

        #[async_trait]
        impl CopilotHttpTransport for HeldFirstRequest {
            async fn get(
                &self,
                url: &str,
                headers: HeaderMap,
            ) -> Result<CopilotHttpResponse, CopilotHttpTransportError> {
                let arrived = self.arrived.lock().take();
                if let Some(arrived) = arrived {
                    let _ = arrived.send(());
                    let release = self.release.lock().take();
                    if let Some(release) = release {
                        let _ = release.await;
                    }
                }
                self.inner.get(url, headers).await
            }
        }

        /// GitHub's refusal of a token applies only to the publication it was
        /// made for. Two account states share the account's lease (two
        /// provider registries): the first one's identity lookup for the
        /// earlier token is in flight when the second one publishes a newer
        /// token, and GitHub refuses the earlier token only after that. The
        /// newer publication stays valid and the second account state keeps
        /// serving it with no further request; the first one's request fails
        /// as `stale_credential`, and its next request reads its source again
        /// and heals on the newer token.
        #[tokio::test]
        async fn a_late_refusal_of_an_earlier_token_leaves_the_newer_one_valid() {
            let earlier = user_token("earlier");
            let newer = user_token("newer");
            let (arrived, lookup_in_flight) = tokio::sync::oneshot::channel();
            let (release_lookup, release) = tokio::sync::oneshot::channel();
            let first_transport = Arc::new(HeldFirstRequest {
                inner: OutcomeTransport::new([
                    Ok(json_response(
                        401,
                        serde_json::json!({"message": "Bad credentials"}),
                    )),
                    Ok(svc_bot()),
                    Ok(models_response()),
                ]),
                arrived: Mutex::new(Some(arrived)),
                release: Mutex::new(Some(release)),
            });
            let first = CopilotRuntime::with_transport(first_transport.clone());
            let second_transport = OutcomeTransport::new([Ok(svc_bot()), Ok(models_response())]);
            let second = CopilotRuntime::with_transport(second_transport.clone());
            let operator = OperatorEnv::new();
            operator.set("COPILOT_GITHUB_TOKEN", &earlier);
            let first_binding = token_binding(CopilotProviderRoute::OpenAi, env_source());
            let second_binding = token_binding(CopilotProviderRoute::Anthropic, env_source());

            let first_resolution = first.resolve(&first_binding, &operator.env);
            let newer_publication = async {
                lookup_in_flight
                    .await
                    .expect("the earlier token's lookup is in flight");
                // The source now gives the newer token, and the second account
                // state publishes it.
                operator.set("COPILOT_GITHUB_TOKEN", &newer);
                let resolved = second
                    .resolve(&second_binding, &operator.env)
                    .await
                    .expect("the newer token resolves");
                release_lookup
                    .send(())
                    .expect("GitHub answers the earlier token's lookup");
                resolved
            };
            let (first_result, second_resolved) = tokio::join!(first_resolution, newer_publication);

            let lease = operator.lease();
            assert_eq!(
                lease.phase,
                Some(AuthLeasePhase::Valid),
                "a refusal of the earlier token must not withdraw the newer one"
            );
            assert_eq!(lease.generation, 2, "the newer token's publication");
            let url = format!("{BUSINESS_API}/responses");
            let (_, bearer) = authorize_once(&second_resolved.concrete_authorizer(), &url).await;
            assert_eq!(bearer, format!("Bearer {newer}"));
            assert_eq!(
                second_transport.request_count(),
                2,
                "the second account state keeps serving the newer token"
            );
            assert_eq!(
                first_result.err(),
                Some(ProviderAuthError::Auth(AuthError::StaleCredential)),
                "the earlier token's request fails as stale"
            );

            // The first account state's next request reads its source again.
            let healed = first
                .resolve(&first_binding, &operator.env)
                .await
                .expect("the first account state heals on the newer token");
            let (_, bearer) = authorize_once(&healed.concrete_authorizer(), &url).await;
            assert_eq!(bearer, format!("Bearer {newer}"));
            assert_eq!(first_transport.inner.request_count(), 3);
            assert_eq!(operator.lease().phase, Some(AuthLeasePhase::Valid));
            assert_eq!(operator.lease().generation, 2);
        }

        /// A refusal of a stored token names the publication the token was
        /// stored under (its record's durable marker), not a lease snapshot
        /// read after the value. A newer token being stored meanwhile (its
        /// publication made, its record not yet saved when this request read
        /// the store) stays valid: this request fails as `stale_credential`,
        /// and once the newer record is saved it serves.
        #[tokio::test]
        async fn a_refusal_of_a_stored_token_names_its_stored_publication() {
            let harness = Harness::new([
                Ok(json_response(
                    401,
                    serde_json::json!({"message": "Bad credentials"}),
                )),
                Ok(svc_bot()),
                Ok(models_response()),
            ]);
            let store = harness
                .operator
                .env
                .provider_auth_persistence()
                .expect("provider-auth persistence")
                .token_store();
            let identity = account_identity();
            let key = meerkat_core::auth::TokenKey::from_credential_identity(&identity);
            let stored = |token: String| PersistedTokens {
                auth_mode: PersistedAuthMode::StaticBearer,
                primary_secret: Some(token),
                refresh_token: None,
                id_token: None,
                expires_at: None,
                last_refresh: Some(Utc::now()),
                scopes: Vec::new(),
                account_id: None,
                metadata: serde_json::Value::Null,
            };
            // As a store write does: publish the token's lifecycle, then mark
            // the record with that publication.
            let publish = |tokens: PersistedTokens| {
                let transition = meerkat_core::publish_token_lifecycle_acquired_for_identity(
                    &harness.operator.auth_lease,
                    &identity,
                    &tokens,
                )
                .expect("publish the stored token");
                meerkat_core::mark_tokens_lifecycle_published_for_transition(
                    &key,
                    &tokens,
                    &transition,
                )
                .expect("mark the stored token")
            };
            let earlier = publish(stored(user_token("stored-earlier")));
            store
                .save(&key, &earlier)
                .await
                .expect("store the earlier token");
            let newer = publish(stored(user_token("stored-newer")));
            let newer_generation = harness.operator.lease().generation;

            let refused = harness
                .resolve_from(
                    CopilotProviderRoute::OpenAi,
                    CredentialSourceSpec::ManagedStore,
                )
                .await
                .err();
            assert_eq!(
                harness.operator.lease().phase,
                Some(AuthLeasePhase::Valid),
                "a refusal of the earlier stored token must not withdraw the newer one"
            );
            assert_eq!(harness.operator.lease().generation, newer_generation);
            assert_eq!(
                refused,
                Some(ProviderAuthError::Auth(AuthError::StaleCredential))
            );

            store
                .save(&key, &newer)
                .await
                .expect("the newer token's record is saved");
            let resolved = harness
                .resolve_from(
                    CopilotProviderRoute::OpenAi,
                    CredentialSourceSpec::ManagedStore,
                )
                .await
                .expect("the newer stored token serves");
            let (_, bearer) = authorize_once(
                &resolved.concrete_authorizer(),
                &format!("{BUSINESS_API}/responses"),
            )
            .await;
            assert_eq!(bearer, format!("Bearer {}", user_token("stored-newer")));
        }

        /// A command source's `refresh_interval_ms` at or below the
        /// AuthMachine's refresh window would run the command and look the
        /// account up again on every request: it is refused as
        /// `unsupported_combination`, before the command runs, any request is
        /// made or the lease moves.
        #[tokio::test]
        async fn command_refresh_interval_inside_the_refresh_window_is_refused() {
            for interval_ms in [500, 30_000, AUTH_LEASE_TTL_REFRESH_WINDOW_SECS * 1000] {
                let harness = Harness::new(std::iter::empty());
                let source = CredentialSourceSpec::Command {
                    program: "/nonexistent/mint-copilot-token".into(),
                    args: Vec::new(),
                    cwd: None,
                    env: BTreeMap::new(),
                    timeout_ms: 1_000,
                    refresh_interval_ms: Some(interval_ms),
                };

                let refused = harness
                    .resolve_from(CopilotProviderRoute::OpenAi, source)
                    .await
                    .err()
                    .expect("the interval is refused");

                assert_eq!(
                    refused,
                    ProviderAuthError::Auth(AuthError::UnsupportedCombination {
                        backend: "copilot".to_string(),
                        auth: "command_refresh_interval_within_refresh_window".to_string(),
                    }),
                    "{interval_ms}"
                );
                assert_eq!(harness.transport.request_count(), 0);
                assert_eq!(harness.operator.lease().generation, 0, "no lease moved");
            }
        }

        /// The interval refusal surfaces typed, as the classic-token refusal
        /// does: the authorizer's `AuthError` (and so every surface's error
        /// kind) is `unsupported_combination` naming the backend and the
        /// refused combination, not a generic refresh failure.
        #[tokio::test]
        async fn command_refresh_interval_refusal_surfaces_typed() {
            let harness = Harness::new(std::iter::empty());
            let refused = harness
                .resolve_from(
                    CopilotProviderRoute::Gemini,
                    CredentialSourceSpec::Command {
                        program: "/nonexistent/mint-copilot-token".into(),
                        args: Vec::new(),
                        cwd: None,
                        env: BTreeMap::new(),
                        timeout_ms: 1_000,
                        refresh_interval_ms: Some(60_000),
                    },
                )
                .await
                .err()
                .expect("the interval is refused");

            let auth = auth_error_from_provider(refused);
            assert_eq!(auth.kind().as_str(), "unsupported_combination");
            assert_eq!(
                serde_json::to_value(auth.kind()).expect("AuthErrorKind JSON"),
                "unsupported_combination"
            );
            assert!(
                matches!(
                    &auth,
                    AuthError::UnsupportedCombination { backend, auth: combination }
                        if backend == "copilot"
                            && combination == "command_refresh_interval_within_refresh_window"
                ),
                "{auth:?}"
            );
        }

        /// The floor counts whole seconds, as the lease's lifetime does: 60001
        /// to 60999 ms is the same one-minute lifetime as 60000 and is refused
        /// the same way, and 61000 ms is the first interval past the floor
        /// (the command then runs; this one does not exist).
        #[tokio::test]
        async fn command_refresh_interval_floor_counts_whole_seconds() {
            let command = |interval_ms| CredentialSourceSpec::Command {
                program: "/nonexistent/mint-copilot-token".into(),
                args: Vec::new(),
                cwd: None,
                env: BTreeMap::new(),
                timeout_ms: 1_000,
                refresh_interval_ms: Some(interval_ms),
            };
            let floor_refusal = |error: &ProviderAuthError| {
                matches!(
                    error,
                    ProviderAuthError::Auth(AuthError::UnsupportedCombination { auth, .. })
                        if auth == "command_refresh_interval_within_refresh_window"
                )
            };
            for interval_ms in [60_001, 60_500, 60_999] {
                let harness = Harness::new(std::iter::empty());
                let error = harness
                    .resolve_from(CopilotProviderRoute::OpenAi, command(interval_ms))
                    .await
                    .err()
                    .expect("the interval is refused");
                assert!(floor_refusal(&error), "{interval_ms}: {error:?}");
                assert_eq!(harness.transport.request_count(), 0);
            }

            let harness = Harness::new(std::iter::empty());
            let error = harness
                .resolve_from(CopilotProviderRoute::OpenAi, command(61_000))
                .await
                .err()
                .expect("this command does not exist");
            assert!(
                !floor_refusal(&error),
                "61000 ms outlasts the refresh window: {error:?}"
            );
        }

        /// T4: GitHub refusing the token itself moves the shared account to
        /// `reauth_required`; the same token then fails at once with no
        /// request, and a new token value is accepted.
        #[tokio::test]
        async fn revoked_token_marks_reauth_and_fails_fast() {
            let harness = Harness::new([
                Ok(json_response(
                    401,
                    serde_json::json!({"message": "Bad credentials"}),
                )),
                Ok(svc_bot()),
                Ok(models_response()),
            ]);
            harness
                .operator
                .set("COPILOT_GITHUB_TOKEN", &user_token("revoked"));

            let refused = harness
                .resolve(CopilotProviderRoute::OpenAi)
                .await
                .err()
                .expect("GitHub refused the token");
            assert_eq!(
                refused,
                ProviderAuthError::Auth(AuthError::UserReauthRequired)
            );
            assert_eq!(
                harness.operator.lease().phase,
                Some(AuthLeasePhase::ReauthRequired)
            );

            let again = harness
                .resolve(CopilotProviderRoute::Anthropic)
                .await
                .err()
                .expect("the same token stays refused");
            assert_eq!(
                again,
                ProviderAuthError::Auth(AuthError::UserReauthRequired)
            );
            assert_eq!(
                harness.transport.request_count(),
                1,
                "the refused token makes no request"
            );

            let replacement = user_token("replacement");
            harness.operator.set("COPILOT_GITHUB_TOKEN", &replacement);
            harness
                .resolve(CopilotProviderRoute::OpenAi)
                .await
                .expect("a new token value is accepted");
            assert_eq!(
                harness.transport.bearer(1),
                Some(format!("Bearer {replacement}"))
            );
            assert_eq!(harness.operator.lease().phase, Some(AuthLeasePhase::Valid));
        }

        /// Two runtime machines over one durable store share its AuthMachine
        /// lease registry, so they agree on the operator token's lifecycle
        /// although each keeps its own lease handle and account state: a token
        /// one saw GitHub refuse stays refused for the other with no request,
        /// and a logout through one stays in force for both, until the token
        /// value changes.
        #[tokio::test]
        async fn two_machines_over_one_store_share_a_refusal_and_a_logout() {
            let store: Arc<dyn meerkat_runtime::RuntimeStore> =
                Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
            let first_machine = persistent_machine(&store);
            let second_machine = persistent_machine(&store);
            let first_lease = first_machine.generated_auth_lease_handle();
            let second_lease = second_machine.generated_auth_lease_handle();
            assert!(
                !first_lease.same_owner(&second_lease),
                "each machine holds a lease handle of its own"
            );
            let first = Harness::with_env(
                [
                    Ok(json_response(
                        401,
                        serde_json::json!({"message": "Bad credentials"}),
                    )),
                    Ok(svc_bot()),
                    Ok(models_response()),
                ],
                OperatorEnv::over(first_lease),
            );
            let second = Harness::with_env(std::iter::empty(), OperatorEnv::over(second_lease));
            let revoked = user_token("shared-revoked");
            first.operator.set("COPILOT_GITHUB_TOKEN", &revoked);
            second.operator.set("COPILOT_GITHUB_TOKEN", &revoked);

            assert_eq!(
                first.resolve(CopilotProviderRoute::OpenAi).await.err(),
                Some(ProviderAuthError::Auth(AuthError::UserReauthRequired))
            );
            let refused = second.operator.lease();
            assert_eq!(
                refused.phase,
                Some(AuthLeasePhase::ReauthRequired),
                "both machines read the refusal"
            );
            assert_eq!(
                second.resolve(CopilotProviderRoute::Anthropic).await.err(),
                Some(ProviderAuthError::Auth(AuthError::UserReauthRequired)),
                "the other machine does not publish the refused token again"
            );
            assert_eq!(second.transport.request_count(), 0);
            assert_eq!(second.operator.lease(), refused);

            // A new value heals the lease for both; a logout through the
            // second machine then stays in force on both for that value.
            let replacement = user_token("shared-replacement");
            first.operator.set("COPILOT_GITHUB_TOKEN", &replacement);
            second.operator.set("COPILOT_GITHUB_TOKEN", &replacement);
            first
                .resolve(CopilotProviderRoute::OpenAi)
                .await
                .expect("a new token value heals the lease");
            assert_eq!(second.operator.lease().phase, Some(AuthLeasePhase::Valid));
            second
                .operator
                .auth_lease
                .release_lease(&lease_key())
                .expect("log out");
            for harness in [&first, &second] {
                assert_eq!(
                    harness.resolve(CopilotProviderRoute::Gemini).await.err(),
                    Some(ProviderAuthError::Auth(AuthError::LeaseAbsent)),
                    "the logout stays in force for the same token value"
                );
            }
            assert_eq!(first.transport.request_count(), 3);
            assert_eq!(second.transport.request_count(), 0);
        }

        /// A logout of an environment-sourced route stays in force for the
        /// same token value, also on a machine built after the one that
        /// published it is gone (the store's lease registry outlives it), and
        /// a new token value heals it.
        #[tokio::test]
        async fn logout_stays_in_force_for_the_same_token_and_heals_for_a_new_one() {
            let store: Arc<dyn meerkat_runtime::RuntimeStore> =
                Arc::new(meerkat_runtime::InMemoryRuntimeStore::new());
            let token = user_token("logged-out");
            {
                let machine = persistent_machine(&store);
                let harness = Harness::with_env(
                    [Ok(svc_bot()), Ok(models_response())],
                    OperatorEnv::over(machine.generated_auth_lease_handle()),
                );
                harness.operator.set("COPILOT_GITHUB_TOKEN", &token);
                let resolved = harness
                    .resolve(CopilotProviderRoute::OpenAi)
                    .await
                    .expect("the token resolves");
                harness
                    .operator
                    .auth_lease
                    .release_lease(&lease_key())
                    .expect("log out");
                assert_eq!(
                    resolved.concrete_authorizer().prime().await.err(),
                    Some(ProviderAuthError::Auth(AuthError::LeaseAbsent)),
                    "the logout stays in force for the same token value"
                );
                assert_eq!(harness.transport.request_count(), 2);
            }

            // A machine built over the same store after the first is gone.
            let machine = persistent_machine(&store);
            let harness = Harness::with_env(
                [Ok(svc_bot()), Ok(models_response())],
                OperatorEnv::over(machine.generated_auth_lease_handle()),
            );
            harness.operator.set("COPILOT_GITHUB_TOKEN", &token);
            assert_eq!(
                harness.operator.lease().phase,
                None,
                "the store's lease registry holds the logout"
            );
            assert_eq!(
                harness.resolve(CopilotProviderRoute::Anthropic).await.err(),
                Some(ProviderAuthError::Auth(AuthError::LeaseAbsent)),
                "the logout stays in force for the same token value on a later machine"
            );
            assert_eq!(harness.transport.request_count(), 0);

            let replacement = user_token("after-logout");
            harness.operator.set("COPILOT_GITHUB_TOKEN", &replacement);
            harness
                .resolve(CopilotProviderRoute::Anthropic)
                .await
                .expect("a new token value heals the lease");
            assert_eq!(
                harness.transport.bearer(0),
                Some(format!("Bearer {replacement}"))
            );
            assert_eq!(harness.operator.lease().phase, Some(AuthLeasePhase::Valid));
        }

        /// Runs `before` once, when the first request arrives (a lifecycle
        /// change landing while that request is in flight), then answers as
        /// `inner` does.
        struct InterleavingTransport {
            inner: Arc<OutcomeTransport>,
            before: Mutex<Option<Box<dyn FnOnce() + Send>>>,
        }

        #[async_trait]
        impl CopilotHttpTransport for InterleavingTransport {
            async fn get(
                &self,
                url: &str,
                headers: HeaderMap,
            ) -> Result<CopilotHttpResponse, CopilotHttpTransportError> {
                let before = self.before.lock().take();
                if let Some(before) = before {
                    before();
                }
                self.inner.get(url, headers).await
            }
        }

        /// GitHub's refusal of the token lands on a lease a logout released,
        /// or another account state already marked `reauth_required`, while
        /// the identity lookup was in flight: the refusal still surfaces as
        /// `user_reauth_required`, and the lease keeps what the logout or the
        /// other refusal made of it.
        #[tokio::test]
        async fn refusal_after_a_racing_logout_or_refusal_stays_typed() {
            for logout in [true, false] {
                let operator = OperatorEnv::new();
                operator.set("COPILOT_GITHUB_TOKEN", &user_token("raced"));
                let lease = operator.auth_lease.clone();
                let before: Box<dyn FnOnce() + Send> = if logout {
                    Box::new(move || {
                        lease
                            .release_lease(&lease_key())
                            .expect("a logout lands during the lookup");
                    })
                } else {
                    Box::new(move || {
                        lease
                            .mark_reauth_required(&lease_key())
                            .expect("another refusal lands during the lookup");
                    })
                };
                let inner = OutcomeTransport::new([Ok(json_response(
                    401,
                    serde_json::json!({"message": "Bad credentials"}),
                ))]);
                let runtime = CopilotRuntime::with_transport(Arc::new(InterleavingTransport {
                    inner: Arc::clone(&inner),
                    before: Mutex::new(Some(before)),
                }));

                let refused = runtime
                    .resolve(
                        &token_binding(CopilotProviderRoute::OpenAi, env_source()),
                        &operator.env,
                    )
                    .await
                    .err();

                assert_eq!(
                    refused,
                    Some(ProviderAuthError::Auth(AuthError::UserReauthRequired)),
                    "logout: {logout}"
                );
                let expected = if logout {
                    None
                } else {
                    Some(AuthLeasePhase::ReauthRequired)
                };
                assert_eq!(operator.lease().phase, expected, "logout: {logout}");
                assert_eq!(inner.request_count(), 1, "logout: {logout}");
            }
        }

        /// T5: Copilot refusing this identity (a 403 from the model list, here
        /// after an identity lookup that did not answer the token, which
        /// pauses that token for a minute; chat disabled) is a typed
        /// `refresh_failed` that does not touch the lease, and the same token
        /// works once that is fixed. A rate limit pauses the identity lookup
        /// until GitHub's stated time.
        #[tokio::test]
        async fn copilot_refusal_is_typed_and_not_sticky() {
            let harness = Harness::new([
                Ok(unanswered_lookup(403)),
                Ok(json_response(
                    403,
                    serde_json::json!({
                        "message": "The Copilot Requests permission is required"
                    }),
                )),
                Ok(svc_bot()),
                Ok(models_response()),
            ]);
            harness
                .operator
                .set("COPILOT_GITHUB_TOKEN", &user_token("pending-seat"));
            let detail = refresh_failure(
                harness
                    .resolve(CopilotProviderRoute::OpenAi)
                    .await
                    .err()
                    .expect("Copilot refused the identity"),
            );
            assert!(detail.contains("403"), "{detail}");
            assert!(
                detail.contains("The Copilot Requests permission is required"),
                "{detail}"
            );
            assert_eq!(harness.operator.lease().phase, Some(AuthLeasePhase::Valid));
            harness.operator.advance(61);
            harness
                .resolve(CopilotProviderRoute::OpenAi)
                .await
                .expect("the same token works once Copilot accepts it");
            assert_eq!(harness.transport.request_count(), 4);

            let chat_disabled = Harness::new([Ok(json_response(
                200,
                serde_json::json!({"login": "svc-bot", "chat_enabled": false}),
            ))]);
            chat_disabled
                .operator
                .set("COPILOT_GITHUB_TOKEN", &user_token("chat-disabled"));
            let detail = refresh_failure(
                chat_disabled
                    .resolve(CopilotProviderRoute::OpenAi)
                    .await
                    .err()
                    .expect("chat is not enabled"),
            );
            assert!(detail.contains("not enabled"), "{detail}");
            assert_eq!(
                chat_disabled.operator.lease().phase,
                Some(AuthLeasePhase::Valid)
            );

            // One authorizer keeps the pause across calls; the pacing row
            // below covers later account states of the account.
            let operator = OperatorEnv::new();
            operator.set("COPILOT_GITHUB_TOKEN", &user_token("busy"));
            let transport = OutcomeTransport::new([
                Ok(rate_limited(403, 120)),
                Ok(svc_bot()),
                Ok(models_response()),
            ]);
            let authorizer = CopilotAuthorizer {
                state: Arc::new(CopilotAccountState::new(
                    CopilotBackendConfig::default(),
                    CopilotCredentialMode::OperatorToken,
                )),
                binding: token_binding(CopilotProviderRoute::OpenAi, env_source()),
                env: operator.env.clone(),
                transport: transport.clone(),
            };
            let detail = refresh_failure(
                authorizer
                    .prime()
                    .await
                    .expect_err("the identity lookup is rate limited"),
            );
            assert!(detail.contains("rate limited"), "{detail}");
            operator.advance(119);
            refresh_failure(
                authorizer
                    .prime()
                    .await
                    .expect_err("still inside GitHub's stated wait"),
            );
            assert_eq!(
                transport.request_count(),
                1,
                "no request before GitHub's stated time"
            );
            operator.advance(2);
            authorizer
                .prime()
                .await
                .expect("the lookup runs again after the wait");
            assert_eq!(transport.request_count(), 3);
            assert_eq!(operator.lease().phase, Some(AuthLeasePhase::Valid));
        }

        /// GitHub's rate limit on the identity lookup pauses the account, not
        /// one account state: a resolve that failed on it drops its account
        /// state, and the next resolve still makes no request before GitHub's
        /// stated time.
        #[tokio::test]
        async fn identity_pause_outlives_the_account_state() {
            let harness = Harness::new([
                Ok(rate_limited(403, 120)),
                Ok(svc_bot()),
                Ok(models_response()),
            ]);
            harness
                .operator
                .set("COPILOT_GITHUB_TOKEN", &user_token("paced"));
            let detail = refresh_failure(
                harness
                    .resolve(CopilotProviderRoute::OpenAi)
                    .await
                    .err()
                    .expect("the identity lookup is rate limited"),
            );
            assert!(detail.contains("rate limited"), "{detail}");

            harness.operator.advance(60);
            let detail = refresh_failure(
                harness
                    .resolve(CopilotProviderRoute::Anthropic)
                    .await
                    .err()
                    .expect("a later account state still waits"),
            );
            assert!(detail.contains("rate limited"), "{detail}");
            assert_eq!(
                harness.transport.request_count(),
                1,
                "no request before GitHub's stated time"
            );

            harness.operator.advance(61);
            harness
                .resolve(CopilotProviderRoute::Anthropic)
                .await
                .expect("the lookup runs again after the wait");
            assert_eq!(harness.transport.request_count(), 3);
        }

        /// The pause belongs to the credential account, whichever credential
        /// store an account state was built over: resolving the same account
        /// through another provider-auth persistence (another store, another
        /// host environment) waits for GitHub's stated time too, with no
        /// request.
        #[tokio::test]
        async fn identity_pause_is_shared_across_credential_stores() {
            let harness = Harness::new([
                Ok(rate_limited(403, 120)),
                Ok(svc_bot()),
                Ok(models_response()),
            ]);
            let token = user_token("paced-across-stores");
            harness.operator.set("COPILOT_GITHUB_TOKEN", &token);
            let other_store = OperatorEnv::new();
            other_store.set("COPILOT_GITHUB_TOKEN", &token);
            *other_store.clock.lock() = *harness.operator.clock.lock();
            let detail = refresh_failure(
                harness
                    .resolve(CopilotProviderRoute::OpenAi)
                    .await
                    .err()
                    .expect("the identity lookup is rate limited"),
            );
            assert!(detail.contains("rate limited"), "{detail}");

            let detail = refresh_failure(
                harness
                    .runtime
                    .resolve(
                        &token_binding(CopilotProviderRoute::Anthropic, env_source()),
                        &other_store.env,
                    )
                    .await
                    .err()
                    .expect("the account waits through another credential store too"),
            );
            assert!(detail.contains("rate limited"), "{detail}");
            assert_eq!(
                harness.transport.request_count(),
                1,
                "no request before GitHub's stated time"
            );
        }

        /// A rate-limited model listing (a 403 with `Retry-After`, GitHub's
        /// secondary limit) is no refusal: the account resolves without its
        /// model list, which is asked again (off the request path) only after
        /// GitHub's stated time, with no identity lookup in between.
        #[tokio::test]
        async fn models_rate_limit_waits_for_githubs_time() {
            let harness = Harness::new([
                Ok(svc_bot()),
                Ok(rate_limited(403, 120)),
                Ok(models_response()),
            ]);
            harness
                .operator
                .set("COPILOT_GITHUB_TOKEN", &user_token("listing-paced"));
            let resolved = harness
                .resolve(CopilotProviderRoute::OpenAi)
                .await
                .expect("a rate-limited listing does not refuse the token");
            assert!(resolved.model_snapshot().is_none());
            assert_eq!(harness.operator.lease().phase, Some(AuthLeasePhase::Valid));
            let authorizer = resolved.concrete_authorizer();
            let url = format!("{BUSINESS_API}/responses");

            harness.operator.advance(119);
            authorize_once(&authorizer, &url).await;
            assert_eq!(
                harness.transport.request_count(),
                2,
                "no listing before GitHub's stated time"
            );

            harness.operator.advance(2);
            authorize_once(&authorizer, &url).await;
            settle(|| authorizer.model_snapshot().is_some()).await;
            assert_eq!(harness.transport.request_count(), 3);
            assert_eq!(
                harness.transport.urls()[2],
                format!("{BUSINESS_API}/models"),
                "only the listing runs again"
            );
        }

        /// Two authorizers of one operator-token account, as two routes or two
        /// sessions hold them.
        fn authorizer_pair(
            transport: Arc<dyn CopilotHttpTransport>,
        ) -> (CopilotAuthorizer, CopilotAuthorizer) {
            let operator = OperatorEnv::new();
            operator.set("COPILOT_GITHUB_TOKEN", &user_token("shared"));
            let env = operator.env;
            let state = Arc::new(CopilotAccountState::new(
                CopilotBackendConfig::default(),
                CopilotCredentialMode::OperatorToken,
            ));
            let binding = token_binding(CopilotProviderRoute::OpenAi, env_source());
            let first = CopilotAuthorizer {
                state: Arc::clone(&state),
                binding: binding.clone(),
                env: env.clone(),
                transport: Arc::clone(&transport),
            };
            let second = CopilotAuthorizer {
                state,
                binding,
                env,
                transport,
            };
            (first, second)
        }

        /// T6: concurrent authorizations of one account share one discovery
        /// flight, share its typed failure, and a cancelled leader releases
        /// the waiter to a flight of its own.
        #[tokio::test]
        async fn concurrent_authorizations_share_one_discovery() {
            let delayed = Arc::new(DelayedTransport {
                inner: ScriptedTransport::new([svc_bot(), models_response()]),
            });
            let (first, second) = authorizer_pair(delayed.clone());
            let (first_result, second_result) = tokio::join!(first.prime(), second.prime());
            first_result.expect("first authorization");
            second_result.expect("second authorization");
            assert_eq!(delayed.inner.request_count(), 2);

            let failing = Arc::new(DelayedTransport {
                inner: ScriptedTransport::new([json_response(
                    500,
                    serde_json::json!({"message": "temporary failure"}),
                )]),
            });
            let (first, second) = authorizer_pair(failing.clone());
            let (first_result, second_result) = tokio::join!(first.prime(), second.prime());
            assert!(first_result.is_err());
            assert!(second_result.is_err());
            assert_eq!(
                failing.inner.request_count(),
                1,
                "the waiter observes the leader's typed failure"
            );

            let cancelling = Arc::new(CancelFirstTransport {
                inner: ScriptedTransport::new([svc_bot(), models_response()]),
                calls: AtomicU64::new(0),
                first_started: tokio::sync::Notify::new(),
            });
            let (first, second) = authorizer_pair(cancelling.clone());
            let first_started = cancelling.first_started.notified();
            let leader = tokio::spawn(async move { first.prime().await });
            first_started.await;
            leader.abort();
            assert!(
                leader
                    .await
                    .expect_err("the leader is cancelled")
                    .is_cancelled()
            );
            second
                .prime()
                .await
                .expect("the waiter leads the replacement flight");
            assert_eq!(cancelling.inner.request_count(), 2);
        }

        /// T7: a classic personal access token is refused as an unsupported
        /// combination before any request, and no lease is acquired.
        #[tokio::test]
        async fn classic_pat_is_refused_before_any_request() {
            let harness = Harness::new(std::iter::empty());
            harness
                .operator
                .set("COPILOT_GITHUB_TOKEN", &format!("ghp_{CANARY}"));

            let refused = harness
                .resolve(CopilotProviderRoute::OpenAi)
                .await
                .err()
                .expect("a classic token is refused");

            assert_eq!(
                refused,
                ProviderAuthError::Auth(AuthError::UnsupportedCombination {
                    backend: "copilot".to_string(),
                    auth: "classic_personal_access_token".to_string(),
                })
            );
            assert_eq!(harness.transport.request_count(), 0);
            let lease = harness.operator.lease();
            assert_eq!(lease.phase, None);
            assert_eq!(lease.generation, 0, "no lease was acquired");
        }

        /// T8: a GitHub App installation token (server-to-server) has no
        /// user: it skips the identity lookup and lists models on the default
        /// host. A later 401 there (off the request path) has the next
        /// request list the models again, and that 401 is GitHub refusing the
        /// token itself.
        #[tokio::test]
        async fn installation_token_skips_identity_lookup() {
            let installation = format!("ghs_{CANARY}{}", "x".repeat(500));
            let bad_credentials = || {
                Ok(json_response(
                    401,
                    serde_json::json!({"message": "Bad credentials"}),
                ))
            };
            let harness =
                Harness::new([Ok(models_response()), bad_credentials(), bad_credentials()]);
            harness.operator.set("GITHUB_TOKEN", &installation);

            let resolved = harness
                .resolve(CopilotProviderRoute::Anthropic)
                .await
                .expect("the installation token resolves");

            assert_eq!(
                harness.transport.urls(),
                [format!("{DEFAULT_COPILOT_API_BASE}/models")]
            );
            assert_eq!(
                harness.transport.bearer(0),
                Some(format!("Bearer {installation}"))
            );
            assert_eq!(resolved.metadata().account_id, None);
            let authorizer = resolved.concrete_authorizer();
            let identity = authorizer.account_identity().expect("account identity");
            assert!(identity.installation);
            assert_eq!(identity.login, None);

            // The model list is due again after five minutes: the request
            // that finds it due goes out on the account it holds, and the
            // listing's 401 drops the account.
            harness.operator.advance(301);
            authorizer
                .prepare_request()
                .await
                .expect("the request that finds the list due goes out on the account it holds");
            settle(|| harness.transport.request_count() == 2).await;
            let refused = authorizer
                .prepare_request()
                .await
                .expect_err("GitHub refused the installation token");
            assert_eq!(refused, AuthError::UserReauthRequired);
            assert_eq!(
                harness.operator.lease().phase,
                Some(AuthLeasePhase::ReauthRequired)
            );
            assert_eq!(harness.transport.request_count(), 3);
        }

        /// A token route's model list is refreshed off the request path too:
        /// a request that finds it due goes out on the account it holds, even
        /// when the listing then refuses the token; the next request reads
        /// the token again and looks the account up, which decides.
        #[tokio::test]
        async fn operator_model_discovery_runs_off_the_request_path() {
            let token = user_token("primary");
            let harness = Harness::new([
                Ok(svc_bot()),
                Ok(models_response()),
                Ok(json_response(
                    403,
                    serde_json::json!({"message": "Copilot is not enabled for this organization"}),
                )),
                Ok(svc_bot()),
                Ok(models_response()),
            ]);
            harness.operator.set("COPILOT_GITHUB_TOKEN", &token);
            let resolved = harness
                .resolve(CopilotProviderRoute::OpenAi)
                .await
                .expect("the operator token resolves");
            let authorizer = resolved.concrete_authorizer();

            // The model list is due again after five minutes.
            harness.operator.advance(301);
            authorizer
                .prepare_request()
                .await
                .expect("a request neither waits for nor fails on the account's model discovery");
            settle(|| harness.transport.request_count() == 3).await;
            authorizer
                .prepare_request()
                .await
                .expect("the account is built again through the identity lookup");
            assert_eq!(harness.transport.request_count(), 5);
            assert_eq!(harness.transport.urls()[3], GITHUB_COPILOT_USER_URL);
            assert_eq!(harness.operator.lease().phase, Some(AuthLeasePhase::Valid));
        }

        /// GitHub's answer when its identity lookup does not answer a token:
        /// what a fine-grained personal access token can get from
        /// `copilot_internal/user`.
        fn unanswered_lookup(status: u16) -> CopilotHttpResponse {
            json_response(
                status,
                serde_json::json!({
                    "message": "Resource not accessible by personal access token"
                }),
            )
        }

        /// GitHub's identity lookup may not answer a token the Copilot API
        /// accepts: a fine-grained personal access token can get a 403 or a
        /// 404 from it. Such an answer, when it is no rate limit, has the
        /// route try the default Copilot API host with no login or plan, and
        /// use the token once the account's model list loads there. The
        /// lookup's message, which can echo the request's token, reaches no
        /// log line.
        #[tokio::test]
        async fn an_unanswered_identity_lookup_uses_the_default_host_once_models_load() {
            let log = LogCapture::default();
            let capture = CaptureSubscriber::dispatch(log.clone());
            // A second registered dispatcher makes callsite interest come from
            // every dispatcher, so a callsite first reached under no
            // subscriber still reaches this one.
            let _anchor = CaptureSubscriber::dispatch(LogCapture::default());
            let _guard = tracing::dispatcher::set_default(&capture);

            for status in [403, 404] {
                let token = user_token(&format!("unanswered-{status}"));
                let bearer = format!("Bearer {token}");
                let harness =
                    Harness::new([Ok(message_echo(status, &bearer)), Ok(models_response())]);
                harness.operator.set("COPILOT_GITHUB_TOKEN", &token);

                let resolved = harness
                    .resolve(CopilotProviderRoute::OpenAi)
                    .await
                    .unwrap_or_else(|error| {
                        panic!("lookup {status}: the token is used on the default host: {error:?}")
                    });

                assert_eq!(
                    harness.transport.urls(),
                    [
                        GITHUB_COPILOT_USER_URL.to_string(),
                        format!("{DEFAULT_COPILOT_API_BASE}/models")
                    ],
                    "lookup {status}"
                );
                assert_eq!(harness.transport.bearer(1), Some(bearer.clone()));
                assert_eq!(resolved.metadata().account_id, None, "lookup {status}");
                assert_eq!(resolved.metadata().plan, None, "lookup {status}");
                let authorizer = resolved.concrete_authorizer();
                let identity = authorizer.account_identity().expect("account identity");
                assert_eq!(identity.api_base, DEFAULT_COPILOT_API_BASE);
                assert_eq!(identity.login, None);
                assert!(!identity.installation);
                let (_, signed) = authorize_once(
                    &authorizer,
                    &format!("{DEFAULT_COPILOT_API_BASE}/responses"),
                )
                .await;
                assert_eq!(signed, bearer);
                assert_eq!(
                    harness.transport.request_count(),
                    2,
                    "lookup {status}: requests reuse the account state"
                );
                assert_eq!(harness.operator.lease().phase, Some(AuthLeasePhase::Valid));
            }

            let logged = log.0.lock().clone();
            assert!(
                logged.contains("trying the default Copilot API host"),
                "the capture saw the lookup's answer: {logged}"
            );
            assert!(!logged.contains(CANARY), "{logged}");
        }

        /// The CLI's check accepts such a token the same way: unnamed, on the
        /// default host, once its model list loads there.
        #[tokio::test]
        async fn the_check_accepts_a_token_the_identity_lookup_does_not_answer() {
            let transport =
                OutcomeTransport::new([Ok(unanswered_lookup(403)), Ok(models_response())]);
            let identity = verify_operator_token_with(
                transport.as_ref(),
                &user_token("check-unanswered"),
                &CopilotBackendConfig::from_token_options(&serde_json::Value::Null)
                    .expect("default token route options"),
                Utc::now(),
            )
            .await
            .expect("the check accepts the token");

            assert_eq!(identity.login, None);
            assert_eq!(identity.plan, None);
            assert!(!identity.installation);
            assert_eq!(identity.api_base, DEFAULT_COPILOT_API_BASE);
            assert_eq!(
                transport.urls(),
                [
                    GITHUB_COPILOT_USER_URL.to_string(),
                    format!("{DEFAULT_COPILOT_API_BASE}/models")
                ]
            );
        }

        /// A Copilot API 401 does not mean the token is revoked: Copilot
        /// answers 401 for a valid token without the Copilot Requests
        /// permission, without a seat, or with its integration not allowed,
        /// and an identity lookup that answered 403 or 404 had already
        /// authenticated the token. So a 401 from the model list of a token
        /// the lookup did not answer is Copilot refusing it (`refresh_failed`
        /// with Copilot's message, the lease untouched), and the same token is
        /// not asked about again for a minute; after it, the same token works
        /// once Copilot accepts it. A 401 from a later listing (off the
        /// request path) has the next request run the identity lookup again,
        /// whose own 401 decides that the token is revoked. The CLI's check
        /// reports the same Copilot refusal.
        #[tokio::test]
        async fn an_unanswered_lookup_then_a_rejected_model_list_refuses_the_token() {
            let permission = || -> Outcome {
                Ok(json_response(
                    401,
                    serde_json::json!({
                        "message": "unauthorized: Personal Access Token does not have Copilot Requests permission"
                    }),
                ))
            };
            let copilot_refusal = |refused: &Option<ProviderAuthError>| {
                matches!(
                    refused,
                    Some(ProviderAuthError::Auth(AuthError::RefreshFailed(detail)))
                        if detail.contains("401")
                            && detail.contains("does not have Copilot Requests permission")
                )
            };

            let harness = Harness::new([
                Ok(unanswered_lookup(403)),
                permission(),
                Ok(unanswered_lookup(403)),
                Ok(models_response()),
            ]);
            harness
                .operator
                .set("COPILOT_GITHUB_TOKEN", &user_token("refused-at-first"));
            let refused = harness.resolve(CopilotProviderRoute::OpenAi).await.err();
            assert!(
                copilot_refusal(&refused),
                "a 401 from the first model listing is a Copilot refusal, not a revocation: {refused:?}"
            );
            assert_eq!(harness.operator.lease().phase, Some(AuthLeasePhase::Valid));
            assert_eq!(harness.transport.request_count(), 2);

            harness.operator.advance(59);
            let paused = harness.resolve(CopilotProviderRoute::OpenAi).await.err();
            assert!(
                copilot_refusal(&paused),
                "the same refusal within the minute: {paused:?}"
            );
            assert_eq!(
                harness.transport.request_count(),
                2,
                "no request for the same token within the minute"
            );
            harness.operator.advance(2);
            harness
                .resolve(CopilotProviderRoute::OpenAi)
                .await
                .expect("the same token works once Copilot accepts it");
            assert_eq!(harness.transport.request_count(), 4);
            assert_eq!(harness.operator.lease().phase, Some(AuthLeasePhase::Valid));

            let later = Harness::new([
                Ok(unanswered_lookup(404)),
                Ok(models_response()),
                permission(),
                Ok(json_response(
                    401,
                    serde_json::json!({"message": "Bad credentials"}),
                )),
            ]);
            later
                .operator
                .set("COPILOT_GITHUB_TOKEN", &user_token("revoked-later"));
            let resolved = later
                .resolve(CopilotProviderRoute::OpenAi)
                .await
                .expect("the token is used on the default host");
            // The model list is due again after five minutes: the request
            // that finds it due goes out on the account it holds, and the
            // listing's 401 drops the account.
            later.operator.advance(301);
            let authorizer = resolved.concrete_authorizer();
            authorizer
                .prepare_request()
                .await
                .expect("the request that finds the list due goes out on the account it holds");
            settle(|| later.transport.request_count() == 3).await;
            let refused = authorizer
                .prepare_request()
                .await
                .expect_err("GitHub's identity lookup refused the token");
            assert_eq!(refused, AuthError::UserReauthRequired);
            assert_eq!(
                later.operator.lease().phase,
                Some(AuthLeasePhase::ReauthRequired)
            );
            assert_eq!(
                later.transport.request_count(),
                4,
                "the later listing's 401 has the next request run the identity lookup again, and its 401 decides"
            );

            let check = OutcomeTransport::new([Ok(unanswered_lookup(403)), permission()]);
            let checked = verify_operator_token_with(
                check.as_ref(),
                &user_token("check-refused"),
                &CopilotBackendConfig::from_token_options(&serde_json::Value::Null)
                    .expect("default token route options"),
                Utc::now(),
            )
            .await
            .err();
            assert!(
                copilot_refusal(&checked),
                "the CLI's check reports Copilot's refusal: {checked:?}"
            );
        }

        /// A token the identity lookup does not answer never serves without
        /// its model list: a listing that fails (a server error, a timeout, a
        /// rate limit) fails the request as `refresh_failed` and leaves the
        /// lease valid, and a rate limit pauses the lookup until GitHub's
        /// stated time.
        #[tokio::test]
        async fn an_unanswered_lookup_never_serves_without_a_model_list() {
            let listings: Vec<(&str, Outcome)> = vec![
                (
                    "server error",
                    Ok(json_response(
                        502,
                        serde_json::json!({"message": "Bad gateway"}),
                    )),
                ),
                ("timeout", Err(CopilotHttpTransportError::Timeout)),
                ("rate limit", Ok(rate_limited(429, 120))),
            ];
            for (label, listing) in listings {
                let harness = Harness::new([Ok(unanswered_lookup(404)), listing]);
                harness
                    .operator
                    .set("COPILOT_GITHUB_TOKEN", &user_token("unanswered-down"));
                let failed = harness.resolve(CopilotProviderRoute::OpenAi).await.err();
                assert_eq!(
                    harness.transport.request_count(),
                    2,
                    "{label}: the model list was asked"
                );
                let detail = refresh_failure(
                    failed.unwrap_or_else(|| panic!("{label}: no route without the model list")),
                );
                assert!(detail.contains("model listing failed"), "{label}: {detail}");
                assert_eq!(
                    harness.operator.lease().phase,
                    Some(AuthLeasePhase::Valid),
                    "{label}"
                );
            }

            let harness = Harness::new([
                Ok(unanswered_lookup(403)),
                Ok(rate_limited(429, 120)),
                Ok(unanswered_lookup(403)),
                Ok(models_response()),
            ]);
            harness
                .operator
                .set("COPILOT_GITHUB_TOKEN", &user_token("unanswered-paced"));
            refresh_failure(
                harness
                    .resolve(CopilotProviderRoute::OpenAi)
                    .await
                    .err()
                    .expect("the model listing is rate limited"),
            );
            harness.operator.advance(119);
            refresh_failure(
                harness
                    .resolve(CopilotProviderRoute::OpenAi)
                    .await
                    .err()
                    .expect("still inside GitHub's stated wait"),
            );
            assert_eq!(
                harness.transport.request_count(),
                2,
                "no request before GitHub's stated time"
            );
            harness.operator.advance(2);
            harness
                .resolve(CopilotProviderRoute::OpenAi)
                .await
                .expect("the lookup and the listing run again after the wait");
            assert_eq!(harness.transport.request_count(), 4);
        }

        /// T9: the token never goes to an API host outside
        /// `githubcopilot.com`, even when GitHub's identity answer names one.
        #[tokio::test]
        async fn api_host_outside_githubcopilot_is_refused() {
            let harness = Harness::new([Ok(user_response(
                "svc-bot",
                "business",
                "https://copilot.example.test",
            ))]);
            harness
                .operator
                .set("COPILOT_GITHUB_TOKEN", &user_token("redirected"));

            let detail = refresh_failure(
                harness
                    .resolve(CopilotProviderRoute::OpenAi)
                    .await
                    .err()
                    .expect("a foreign API host is refused"),
            );

            assert!(detail.contains("copilot.example.test"), "{detail}");
            assert_eq!(harness.transport.urls(), [GITHUB_COPILOT_USER_URL]);
        }

        #[derive(Clone, Default)]
        struct LogCapture(Arc<Mutex<String>>);

        struct CaptureVisitor<'a>(&'a mut String);

        impl tracing::field::Visit for CaptureVisitor<'_> {
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                use std::fmt::Write as _;
                let _ = write!(self.0, " {}={value:?}", field.name());
            }
        }

        /// Records every event and span field as text.
        struct CaptureSubscriber {
            log: LogCapture,
            next_span: AtomicU64,
        }

        impl CaptureSubscriber {
            fn dispatch(log: LogCapture) -> tracing::Dispatch {
                tracing::Dispatch::new(Self {
                    log,
                    next_span: AtomicU64::new(1),
                })
            }
        }

        impl tracing::Subscriber for CaptureSubscriber {
            fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
                true
            }

            fn new_span(&self, span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
                span.record(&mut CaptureVisitor(&mut self.log.0.lock()));
                tracing::span::Id::from_u64(self.next_span.fetch_add(1, Ordering::Relaxed))
            }

            fn record(&self, _span: &tracing::span::Id, values: &tracing::span::Record<'_>) {
                values.record(&mut CaptureVisitor(&mut self.log.0.lock()));
            }

            fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {
            }

            fn event(&self, event: &tracing::Event<'_>) {
                let mut log = self.log.0.lock();
                event.record(&mut CaptureVisitor(&mut log));
                log.push('\n');
            }

            fn enter(&self, _span: &tracing::span::Id) {}

            fn exit(&self, _span: &tracing::span::Id) {}
        }

        /// T10: a canary token never reaches an error (`Display`, `Debug`,
        /// the `AuthError`, its kind and the serialized summary the auth
        /// surfaces emit), the cached account state's `Debug`,
        /// the resolved metadata's JSON or a log line, on any path, nor does
        /// a device sign-in's GitHub token when its token exchange fails
        /// with a page that echoes the request.
        #[tokio::test]
        async fn operator_token_is_never_rendered() {
            let log = LogCapture::default();
            let capture = CaptureSubscriber::dispatch(log.clone());
            // A second registered dispatcher makes callsite interest come from
            // every dispatcher, so a callsite first reached under no
            // subscriber still reaches this one.
            let _anchor = CaptureSubscriber::dispatch(LogCapture::default());
            let _guard = tracing::dispatcher::set_default(&capture);

            let token = user_token("render");
            let paths: Vec<(&str, Vec<Outcome>, String)> = vec![
                (
                    "success",
                    vec![Ok(svc_bot()), Ok(models_response())],
                    token.clone(),
                ),
                (
                    "identity 401",
                    vec![Ok(json_response(
                        401,
                        serde_json::json!({"message": "Bad credentials"}),
                    ))],
                    token.clone(),
                ),
                (
                    "identity 403",
                    vec![
                        Ok(json_response(
                            403,
                            serde_json::json!({"message": "Forbidden"}),
                        )),
                        Ok(json_response(
                            403,
                            serde_json::json!({"message": "Forbidden"}),
                        )),
                    ],
                    token.clone(),
                ),
                (
                    "identity 429",
                    vec![Ok(rate_limited(429, 60))],
                    token.clone(),
                ),
                (
                    "identity 500",
                    vec![Ok(json_response(
                        500,
                        serde_json::json!({"message": "Server error"}),
                    ))],
                    token.clone(),
                ),
                (
                    "timeout",
                    vec![Err(CopilotHttpTransportError::Timeout)],
                    token.clone(),
                ),
                (
                    "redirect",
                    vec![Err(CopilotHttpTransportError::RedirectRefused {
                        status: 302,
                    })],
                    token.clone(),
                ),
                (
                    "bad json",
                    vec![Ok(CopilotHttpResponse {
                        status: 200,
                        body: b"not json".to_vec(),
                        retry_after: None,
                        rate_limited: false,
                        rate_limit_reset: None,
                    })],
                    token.clone(),
                ),
                (
                    "models 401",
                    vec![
                        Ok(svc_bot()),
                        Ok(json_response(
                            401,
                            serde_json::json!({"message": "Unauthorized"}),
                        )),
                    ],
                    token.clone(),
                ),
                (
                    "models 500 echoing the request",
                    vec![Ok(svc_bot()), Ok(echoing_failure(&token))],
                    token.clone(),
                ),
                ("classic", Vec::new(), format!("ghp_{CANARY}")),
            ];
            let mut rendered = String::new();
            for (path, outcomes, path_token) in paths {
                let harness = Harness::new(outcomes);
                harness.operator.set("COPILOT_GITHUB_TOKEN", &path_token);
                match harness.resolve(CopilotProviderRoute::OpenAi).await {
                    Ok(resolved) => {
                        rendered.push_str(
                            &serde_json::to_string(resolved.metadata()).expect("metadata JSON"),
                        );
                        let authorizer = resolved.concrete_authorizer();
                        rendered.push_str(&format!("{:?}", authorizer.state.cache.read().as_ref()));
                        rendered.push_str(&format!("{:?}", authorizer.account_identity()));
                    }
                    Err(error) => {
                        rendered.push_str(&format!("{path}: {error} {error:?}"));
                        let auth = auth_error_from_provider(error);
                        // What a caller and the surfaces see: the error's
                        // Display and Debug, its stable kind, and the
                        // serialized summary the auth status surfaces emit
                        // (the kind and the Display message).
                        let summary = meerkat_core::auth::AuthErrorSummary {
                            kind: auth.kind(),
                            message: auth.to_string(),
                            occurred_at: Utc::now(),
                        };
                        rendered.push_str(&format!(
                            " {auth} {auth:?} {} {}",
                            auth.kind().as_str(),
                            serde_json::to_string(&summary).expect("auth error summary JSON")
                        ));
                    }
                }
            }
            // The CLI's check, whose error the CLI prints, on a model listing
            // that fails with a page echoing the request.
            let echoing = OutcomeTransport::new([Ok(svc_bot()), Ok(echoing_failure(&token))]);
            let checked = verify_operator_token_with(
                &*echoing,
                &token,
                &CopilotBackendConfig::default(),
                Utc::now(),
            )
            .await
            .expect_err("the model listing failed");
            rendered.push_str(&format!("check: {checked} {checked:?}"));
            // The device sign-in's token exchange, failing with a page that
            // echoes the request: the account's stored GitHub token (carrying
            // the canary here) stays out of the error and the log lines.
            let (auth_binding, device_binding) = validated_binding();
            let (device_env, _device_lease) =
                resolver_environment_with_token(&auth_binding, &token).await;
            let device = CopilotAuthorizer {
                state: Arc::new(CopilotAccountState::new(
                    CopilotBackendConfig::default(),
                    CopilotCredentialMode::DeviceSignIn,
                )),
                binding: device_binding,
                env: device_env,
                transport: Arc::new(ScriptedTransport::new([echoing_failure(&token)])),
            };
            let exchanged = device.prime().await.expect_err("the token exchange failed");
            rendered.push_str(&format!("device: {exchanged} {exchanged:?}"));
            rendered.push_str(&log.0.lock());

            assert!(
                rendered.contains("GitHub Copilot accepted the operator token"),
                "the capture saw the runtime's log lines: {rendered}"
            );
            assert!(
                rendered.contains("model discovery returned HTTP 500"),
                "the capture saw the model listing failure: {rendered}"
            );
            assert!(
                rendered.contains("token exchange returned 500"),
                "the capture saw the token exchange failure: {rendered}"
            );
            assert!(!rendered.contains(CANARY), "{rendered}");
        }

        /// A gateway's 500 page that echoes the request's headers, the
        /// bearer included.
        fn echoing_failure(token: &str) -> CopilotHttpResponse {
            CopilotHttpResponse {
                status: 500,
                body: format!(
                    "<html><body>upstream error; request headers: Authorization: Bearer {token}</body></html>"
                )
                .into_bytes(),
                retry_after: None,
                rate_limited: false,
                rate_limit_reset: None,
            }
        }

        /// A GitHub answer whose JSON `message` quotes the request's
        /// Authorization value, as a proxy or gateway that echoes the request's
        /// headers can produce.
        fn message_echo(status: u16, authorization: &str) -> CopilotHttpResponse {
            json_response(
                status,
                serde_json::json!({"message": format!("echoed by GitHub: {authorization}")}),
            )
        }

        /// The operator token never reaches an error, its `Display` or
        /// `Debug`, or a log line through GitHub's quoted `message`: an
        /// identity lookup that does not answer (403, logged before the model
        /// list refuses), rejected (401) or failing (500), a model listing
        /// failing (500) or refused (403), the CLI's check, and the device
        /// sign-in's token exchange, each answering with a `message` that
        /// echoes the request's Authorization value. The message itself is
        /// still quoted.
        #[tokio::test]
        async fn githubs_quoted_message_never_carries_the_token() {
            let log = LogCapture::default();
            let capture = CaptureSubscriber::dispatch(log.clone());
            // A second registered dispatcher makes callsite interest come from
            // every dispatcher, so a callsite first reached under no
            // subscriber still reaches this one.
            let _anchor = CaptureSubscriber::dispatch(LogCapture::default());
            let _guard = tracing::dispatcher::set_default(&capture);

            let token = user_token("quoted");
            let bearer = format!("Bearer {token}");
            let paths: Vec<(&str, Vec<Outcome>)> = vec![
                (
                    "identity 403",
                    vec![
                        Ok(message_echo(403, &bearer)),
                        Ok(message_echo(403, &bearer)),
                    ],
                ),
                ("identity 401", vec![Ok(message_echo(401, &bearer))]),
                ("identity 500", vec![Ok(message_echo(500, &bearer))]),
                (
                    "models 500",
                    vec![Ok(svc_bot()), Ok(message_echo(500, &bearer))],
                ),
                (
                    "models 403",
                    vec![Ok(svc_bot()), Ok(message_echo(403, &bearer))],
                ),
            ];
            let mut rendered = String::new();
            for (path, outcomes) in paths {
                let harness = Harness::new(outcomes);
                harness.operator.set("COPILOT_GITHUB_TOKEN", &token);
                match harness.resolve(CopilotProviderRoute::OpenAi).await {
                    Ok(resolved) => {
                        let authorizer = resolved.concrete_authorizer();
                        rendered.push_str(&format!(
                            "{path}: {:?}",
                            authorizer.state.cache.read().as_ref()
                        ));
                    }
                    Err(error) => {
                        rendered.push_str(&format!("{path}: {error} {error:?}\n"));
                    }
                }
            }
            let checked = verify_operator_token_with(
                &*OutcomeTransport::new([Ok(svc_bot()), Ok(message_echo(500, &bearer))]),
                &token,
                &CopilotBackendConfig::default(),
                Utc::now(),
            )
            .await
            .expect_err("the model listing failed");
            rendered.push_str(&format!("check: {checked} {checked:?}\n"));
            let (auth_binding, device_binding) = validated_binding();
            let (device_env, _device_lease) =
                resolver_environment_with_token(&auth_binding, &token).await;
            let device = CopilotAuthorizer {
                state: Arc::new(CopilotAccountState::new(
                    CopilotBackendConfig::default(),
                    CopilotCredentialMode::DeviceSignIn,
                )),
                binding: device_binding,
                env: device_env,
                transport: Arc::new(ScriptedTransport::new([message_echo(
                    500,
                    &format!("token {token}"),
                )])),
            };
            let exchanged = device.prime().await.expect_err("the token exchange failed");
            rendered.push_str(&format!("device: {exchanged} {exchanged:?}\n"));
            rendered.push_str(&log.0.lock());

            assert!(
                rendered.contains("GitHub Copilot refused this token (403): echoed by GitHub"),
                "GitHub's message is still quoted: {rendered}"
            );
            assert!(
                rendered.contains("model discovery returned HTTP 500: echoed by GitHub"),
                "the capture saw the model listing failure: {rendered}"
            );
            assert!(
                rendered.contains("token exchange returned 500: echoed by GitHub"),
                "the capture saw the token exchange failure: {rendered}"
            );
            assert!(!rendered.contains(CANARY), "{rendered}");
        }

        /// The route's authorizer, which provider clients ask to redact what
        /// they read from a response, removes every operator token the
        /// account signed requests with: the current one, and the one a
        /// rotation replaced, whose request can still be answered. The rest
        /// of the diagnostic is kept.
        #[tokio::test]
        async fn signed_operator_tokens_are_redacted_from_response_diagnostics() {
            let earlier = user_token("earlier");
            let current = user_token("current");
            let harness = Harness::new([
                Ok(svc_bot()),
                Ok(models_response()),
                Ok(svc_bot()),
                Ok(models_response()),
            ]);
            harness.operator.set("COPILOT_GITHUB_TOKEN", &earlier);
            let resolved = harness
                .resolve(CopilotProviderRoute::OpenAi)
                .await
                .expect("the earlier token resolves");
            let concrete = resolved.concrete_authorizer();
            let route = resolve_route_for_state(&concrete.state, Provider::OpenAI, "gpt-test")
                .expect("the account's route");
            let authorizer = route.bind_authorizer(resolved.authorizer());
            let url = format!("{BUSINESS_API}/responses");

            let (receipt, bearer) = authorize_once(&concrete, &url).await;
            assert_eq!(bearer, format!("Bearer {earlier}"));
            // The operator rotates the token, and Copilot refuses the earlier
            // one: the next request signs with the current one.
            harness.operator.set("COPILOT_GITHUB_TOKEN", &current);
            refuse_with_401(&concrete, &url, receipt).await;
            let (_, bearer) = authorize_once(&concrete, &url).await;
            assert_eq!(bearer, format!("Bearer {current}"));

            let mut diagnostic = format!("upstream echo: Bearer {earlier}, then Bearer {current}");
            authorizer.redact_response_diagnostic(&mut diagnostic);
            assert_eq!(
                diagnostic,
                "upstream echo: Bearer <redacted>, then Bearer <redacted>"
            );
        }

        /// T11: no token in any variable, or only whitespace, is
        /// `missing_secret` with no request and no lease.
        #[tokio::test]
        async fn missing_token_is_missing_secret_without_lease() {
            let harness = Harness::new(std::iter::empty());
            let absent = harness
                .resolve(CopilotProviderRoute::OpenAi)
                .await
                .err()
                .expect("no variable is set");
            assert_eq!(absent, ProviderAuthError::Auth(AuthError::MissingSecret));

            harness.operator.set("COPILOT_GITHUB_TOKEN", " \t ");
            let blank = harness
                .resolve(CopilotProviderRoute::OpenAi)
                .await
                .err()
                .expect("a blank variable is no token");
            assert_eq!(blank, ProviderAuthError::Auth(AuthError::MissingSecret));

            assert_eq!(harness.transport.request_count(), 0);
            let lease = harness.operator.lease();
            assert_eq!(lease.phase, None);
            assert_eq!(lease.generation, 0, "no lease was acquired");
        }

        /// One credential account holds one Copilot auth method at a time,
        /// in the runtime as at config load.
        #[test]
        fn shared_account_rejects_mixed_copilot_modes() {
            let runtime = CopilotRuntime::new();
            let identity = account_identity();
            let persistence_id = persistence_id();
            let _device = runtime
                .account_state(
                    persistence_id,
                    &identity,
                    CopilotBackendConfig::default(),
                    CopilotCredentialMode::DeviceSignIn,
                )
                .expect("device route");
            let Err(error) = runtime.account_state(
                persistence_id,
                &identity,
                CopilotBackendConfig::default(),
                CopilotCredentialMode::OperatorToken,
            ) else {
                panic!("a token route on a device account must be refused");
            };
            let message = error.to_string();
            assert!(
                message.contains("github_copilot_oauth")
                    && message.contains("github_copilot_token"),
                "{message}"
            );
        }

        /// The routes of one credential account read its token from one
        /// source, as they share one auth method and one configuration: a
        /// route over a second source is refused before it makes a request,
        /// so no token is labelled against another source's publications (a
        /// stored token's refusal would otherwise read as superseded on every
        /// request). Routes over the same source still share the account.
        #[tokio::test]
        async fn routes_of_one_account_refuse_a_second_credential_source() {
            let harness = Harness::new([Ok(svc_bot()), Ok(models_response())]);
            harness
                .operator
                .set("COPILOT_GITHUB_TOKEN", &user_token("first-source"));
            harness
                .operator
                .set("SECOND_COPILOT_TOKEN", &user_token("second-source"));
            let _first = harness
                .resolve(CopilotProviderRoute::OpenAi)
                .await
                .expect("the first route resolves");

            let refused = harness
                .resolve_from(
                    CopilotProviderRoute::Anthropic,
                    CredentialSourceSpec::Env {
                        env: "SECOND_COPILOT_TOKEN".to_string(),
                        fallback: Vec::new(),
                    },
                )
                .await
                .err();
            assert!(
                matches!(
                    &refused,
                    Some(ProviderAuthError::SourceResolutionFailed(message))
                        if message.contains("disagree on the credential source")
                ),
                "a second credential source on one account is refused: {refused:?}"
            );
            assert_eq!(
                harness.transport.request_count(),
                2,
                "the refused route makes no request"
            );
            harness
                .resolve(CopilotProviderRoute::Gemini)
                .await
                .expect("a route over the same source still shares the account");
            assert_eq!(harness.transport.request_count(), 2);
        }

        /// The connection a provider runtime builds for `binding` over
        /// `resolved`.
        fn copilot_connection(
            binding: &ValidatedBinding,
            resolved: &CopilotResolvedAuth,
        ) -> meerkat_llm_core::provider_runtime::ResolvedConnection {
            meerkat_llm_core::provider_runtime::ResolvedConnection {
                provider: binding.provider(),
                backend: binding.backend(),
                backend_profile: binding.backend_profile().clone(),
                credential_identity: binding.credential_identity().clone(),
                auth_lease: Arc::new(
                    meerkat_llm_core::provider_runtime::DynamicLease::from_authorizer(
                        resolved.authorizer(),
                        resolved.metadata().clone(),
                        crate::GITHUB_COPILOT_AUTHORIZER_LABEL,
                    ),
                ),
            }
        }

        /// A fresh runtime holds no account state. The first build of a model
        /// only the account defines resolves its route first, which looks the
        /// account up and loads its model list, and only then reads the list:
        /// the model is offered on the first resolution, and reading the list
        /// makes no request of its own.
        #[tokio::test]
        async fn a_fresh_runtime_defines_an_account_model_once_its_route_resolves() {
            use meerkat_llm_core::provider_runtime::AccountModelLookup;

            let harness = Harness::new([Ok(svc_bot()), Ok(models_response())]);
            harness
                .operator
                .set("COPILOT_GITHUB_TOKEN", &user_token("fresh-runtime"));
            let binding = token_binding(CopilotProviderRoute::OpenAi, env_source());
            let resolved = harness
                .runtime
                .resolve(&binding, &harness.operator.env)
                .await
                .expect("the first resolution looks the account up and lists its models");
            assert_eq!(
                harness.transport.request_count(),
                2,
                "the identity lookup and the model list"
            );
            let connection = copilot_connection(&binding, &resolved);
            match harness.runtime.account_model(&connection, "claude-test") {
                AccountModelLookup::Offered(definition) => {
                    assert_eq!(definition.provider, Provider::Anthropic);
                }
                other => panic!("the listed model is offered on the first resolution: {other:?}"),
            }
            assert_eq!(
                harness.transport.request_count(),
                2,
                "reading the list makes no request"
            );
        }

        /// The model list defines no model only when the account's model
        /// discovery failed: GitHub answers the identity lookup and the
        /// model list fails with a server error, so the route still resolves
        /// (a looked-up token serves without its list), and a model only the
        /// list could define is refused as unavailable, typed.
        #[tokio::test]
        async fn an_account_model_is_unavailable_only_after_its_model_list_failed() {
            use meerkat_llm_core::provider_runtime::{AccountModelLookup, AccountModelRefusal};

            let harness = Harness::new([
                Ok(svc_bot()),
                Ok(json_response(
                    503,
                    serde_json::json!({"message": "service unavailable"}),
                )),
            ]);
            harness
                .operator
                .set("COPILOT_GITHUB_TOKEN", &user_token("listing-fails"));
            let binding = token_binding(CopilotProviderRoute::OpenAi, env_source());
            let resolved = harness
                .runtime
                .resolve(&binding, &harness.operator.env)
                .await
                .expect("a looked-up token resolves without its model list");
            let connection = copilot_connection(&binding, &resolved);
            assert_eq!(
                harness.runtime.account_model(&connection, "claude-test"),
                AccountModelLookup::Refused(AccountModelRefusal::ModelsUnavailable {
                    credential: account_identity(),
                    model: "claude-test".to_string(),
                })
            );
        }

        /// The device sign-in still requires its stored token, and names the
        /// token method as the way to use an operator-supplied one.
        #[tokio::test]
        async fn device_route_with_env_source_points_to_the_token_method() {
            let operator = OperatorEnv::new();
            let auth_binding = AuthBindingRef {
                realm: RealmId::global(),
                binding: BindingId::parse("copilot_openai").expect("valid binding"),
                profile: None,
                origin: BindingOrigin::Configured,
            };
            let backend = BackendProfile {
                id: "copilot_openai".to_string(),
                provider: Provider::OpenAI,
                backend_kind: "copilot".to_string(),
                base_url: None,
                options: serde_json::Value::Null,
                server: None,
            };
            let auth = AuthProfile {
                id: "copilot_openai".to_string(),
                provider: Provider::OpenAI,
                auth_method: "github_copilot_oauth".to_string(),
                source: env_source(),
                constraints: Default::default(),
                metadata_defaults: Default::default(),
            };
            let binding = ProviderRuntimeCatalog::validate_binding_with_credential_identity(
                &auth_binding,
                account_identity(),
                &backend,
                &auth,
                &BindingPolicy::default(),
            )
            .expect("valid device route");

            let error = CopilotRuntime::new()
                .resolve(&binding, &operator.env)
                .await
                .err()
                .expect("the device route reads only its stored token");

            assert!(
                error.to_string().contains("github_copilot_token"),
                "{error}"
            );
        }

        #[test]
        fn github_rate_limit_follows_the_rest_rules() {
            assert!(github_rate_limited(429, None, false));
            assert!(github_rate_limited(403, Some("0"), false));
            assert!(github_rate_limited(403, None, true));
            assert!(!github_rate_limited(403, Some("12"), false));
            assert!(!github_rate_limited(403, None, false));
            assert!(!github_rate_limited(500, Some("0"), true));
        }

        #[test]
        fn operator_api_host_stays_on_githubcopilot() {
            assert_eq!(
                operator_api_base(None).ok().as_deref(),
                Some(DEFAULT_COPILOT_API_BASE)
            );
            assert_eq!(
                operator_api_base(Some("https://api.business.githubcopilot.com/"))
                    .ok()
                    .as_deref(),
                Some(BUSINESS_API)
            );
            for refused in [
                "http://api.githubcopilot.com",
                "https://user@api.githubcopilot.com",
                "https://api.githubcopilot.com.example.test",
                "https://githubcopilot.example.test",
                "https://example.test",
                "not a url",
            ] {
                assert!(operator_api_base(Some(refused)).is_err(), "{refused}");
            }
        }

        #[test]
        fn github_error_message_is_bounded_and_printable() {
            let long = "x".repeat(300);
            let body = serde_json::to_vec(&serde_json::json!({
                "message": format!("line\u{0007}one\n{long}")
            }))
            .expect("body");
            let message = github_error_message(&body, "unquoted-secret");
            assert!(message.starts_with("lineone"), "{message}");
            assert_eq!(message.chars().count(), GITHUB_ERROR_MESSAGE_MAX_CHARS);
            assert!(!message.chars().any(char::is_control));
            assert_eq!(
                github_error_message(b"<html>", "unquoted-secret"),
                "no message"
            );

            // The secret is redacted before the cut, so no prefix of it
            // survives the cut, and a control character inside it does not
            // hide it.
            let secret = user_token("quoted");
            let body = serde_json::to_vec(&serde_json::json!({
                "message": format!("{} {}\u{0007}{}", "y".repeat(189), &secret[..8], &secret[8..])
            }))
            .expect("body");
            let message = github_error_message(&body, &secret);
            assert!(!message.contains(&secret[..8]), "{message}");
            assert!(
                message.ends_with(meerkat_core::redact::REDACTED),
                "{message}"
            );
        }

        /// The CLI's check: no lease, no store and no cache effects; the same
        /// refusals as the runtime.
        #[tokio::test]
        async fn verify_operator_token_checks_without_effects() {
            let transport = OutcomeTransport::new([Ok(svc_bot()), Ok(models_response())]);
            let identity = verify_operator_token_with(
                transport.as_ref(),
                &format!("  {}  ", user_token("verify")),
                &CopilotBackendConfig::default(),
                Utc::now(),
            )
            .await
            .expect("a usable token");
            assert_eq!(identity.login.as_deref(), Some("svc-bot"));
            assert_eq!(identity.plan.as_deref(), Some("business"));
            assert_eq!(
                transport.bearer(0),
                Some(format!("Bearer {}", user_token("verify")))
            );

            let classic = OutcomeTransport::new(std::iter::empty());
            assert!(matches!(
                verify_operator_token_with(
                    classic.as_ref(),
                    "ghp_classic",
                    &CopilotBackendConfig::default(),
                    Utc::now(),
                )
                .await,
                Err(ProviderAuthError::Auth(
                    AuthError::UnsupportedCombination { .. }
                ))
            ));
            assert_eq!(classic.request_count(), 0);

            let forbidden = OutcomeTransport::new([
                Ok(svc_bot()),
                Ok(json_response(
                    403,
                    serde_json::json!({"message": "Copilot Requests permission required"}),
                )),
            ]);
            let refused = verify_operator_token_with(
                forbidden.as_ref(),
                &user_token("no-permission"),
                &CopilotBackendConfig::default(),
                Utc::now(),
            )
            .await
            .expect_err("a token without Copilot access is refused");
            assert!(
                refused
                    .to_string()
                    .contains("Copilot Requests permission required"),
                "{refused}"
            );
        }

        /// Zero-config: the `global` routes Meerkat resolves while no Copilot
        /// route is configured are this crate's three routes in token mode
        /// on the shared account, and the provider catalog accepts each.
        #[test]
        fn zero_config_routes_are_the_copilot_token_routes() {
            let config = meerkat_core::Config::default();
            for route in CopilotProviderRoute::ALL {
                let reference = AuthBindingRef {
                    realm: RealmId::global(),
                    binding: BindingId::parse(route.binding_id()).expect("valid binding"),
                    profile: None,
                    origin: BindingOrigin::Configured,
                };
                let target =
                    meerkat_core::resolve_explicit_auth_binding_target(&config, &reference)
                        .expect("a zero-config Copilot route");
                assert_eq!(target.auth_binding, reference);
                assert_eq!(target.backend.provider, route.provider());
                assert_eq!(target.backend.backend_kind, route.backend_kind());
                assert_eq!(target.auth_profile.auth_method, route.token_auth_method());
                assert_eq!(target.auth_profile.source, env_source());
                assert_eq!(target.credential_identity, account_identity());
                ProviderRuntimeCatalog::validate_binding_with_credential_identity(
                    &reference,
                    account_identity(),
                    &target.backend,
                    &target.auth_profile,
                    &target.binding.policy,
                )
                .expect("the provider catalog accepts the zero-config route");
            }
        }

        // Live probe rows P2-P5 of the operator-token design: real GitHub and
        // Copilot HTTP, ignored in every lane, run by hand before a release
        // with the token each row names. For example, P3 with the GitHub CLI
        // login of an account that has a Copilot seat:
        //
        //   COPILOT_LIVE_USER_TOKEN="$(gh auth token)" ./scripts/repo-cargo test \
        //     -p meerkat-copilot --lib live_operator_ -- --ignored --nocapture
        //
        // A row whose variable is unset fails. P1, one turn on each route, is
        // `copilot_live_operator_token_routes` in the `copilot_live_e2e`
        // integration suite.

        fn live_token(name: &str) -> String {
            std::env::var(name)
                .ok()
                .filter(|token| !token.trim().is_empty())
                .unwrap_or_else(|| panic!("{name} is required for this live probe"))
        }

        /// The configuration the token routes use with no backend options,
        /// which the live checks send, as `rkat auth login copilot
        /// --non-interactive` does.
        fn live_token_config() -> CopilotBackendConfig {
            CopilotBackendConfig::from_token_options(&serde_json::Value::Null)
                .expect("default token route options")
        }

        /// Resolve the OpenAI route of a live token over real HTTP.
        async fn resolve_live(
            operator: &OperatorEnv,
            token: &str,
        ) -> Result<CopilotResolvedAuth, ProviderAuthError> {
            operator.set("COPILOT_GITHUB_TOKEN", token);
            CopilotRuntime::new()
                .resolve(
                    &token_binding(CopilotProviderRoute::OpenAi, env_source()),
                    &operator.env,
                )
                .await
        }

        /// Live probe P2 and P3: a user token of an account with a Copilot
        /// seat (P2: a fine-grained personal access token with Copilot
        /// Requests, such as a machine user's on a Business seat; P3: the
        /// account's `gh auth token`) resolves to its login, plan and API
        /// host, lists the account's models there with the token as the
        /// bearer, and leaves the account lease valid.
        #[tokio::test]
        #[ignore = "live probe: needs COPILOT_LIVE_USER_TOKEN, a GitHub user token with Copilot access"]
        async fn live_operator_user_token_resolves_identity_and_models() {
            let token = live_token("COPILOT_LIVE_USER_TOKEN");
            let identity = verify_operator_token(&token, &live_token_config())
                .await
                .expect("GitHub and Copilot accept the token");
            eprintln!(
                "copilot-live-token login={:?} plan={:?} api_base={}",
                identity.login, identity.plan, identity.api_base
            );
            assert!(!identity.installation, "a user token has a user");
            assert!(
                identity.login.is_some(),
                "the identity lookup names the login"
            );
            assert!(
                operator_api_base(Some(identity.api_base.as_str())).is_ok(),
                "the plan's API host must be a githubcopilot.com host: {}",
                identity.api_base
            );

            let operator = OperatorEnv::new();
            let resolved = resolve_live(&operator, &token)
                .await
                .expect("the token route resolves");
            assert_eq!(resolved.metadata().account_id, identity.login);
            assert!(
                resolved
                    .model_snapshot()
                    .is_some_and(|snapshot| snapshot.models().next().is_some()),
                "the account lists no model"
            );
            assert_eq!(
                resolved.concrete_authorizer().account_identity(),
                Some(identity)
            );
            assert_eq!(operator.lease().phase, Some(AuthLeasePhase::Valid));
        }

        /// Live probe P4: a GitHub App installation token of an organization
        /// that allows server-to-server Copilot requests skips the identity
        /// lookup, lists models on the default Copilot API host, and leaves
        /// the account lease valid.
        #[tokio::test]
        #[ignore = "live probe: needs COPILOT_LIVE_INSTALLATION_TOKEN, a GitHub App installation token"]
        async fn live_operator_installation_token_lists_models() {
            let token = live_token("COPILOT_LIVE_INSTALLATION_TOKEN");
            let identity = verify_operator_token(&token, &live_token_config())
                .await
                .expect("Copilot accepts the installation token");
            assert!(
                identity.installation,
                "a ghs_ token is an installation token"
            );
            assert_eq!(identity.login, None);

            let operator = OperatorEnv::new();
            let resolved = resolve_live(&operator, &token)
                .await
                .expect("the installation token route resolves");
            assert_eq!(resolved.metadata().account_id, None);
            assert!(
                resolved
                    .model_snapshot()
                    .is_some_and(|snapshot| snapshot.models().next().is_some()),
                "the installation lists no model"
            );
            assert_eq!(operator.lease().phase, Some(AuthLeasePhase::Valid));
        }

        /// Live probe P5, revoked token: GitHub's refusal of a revoked user
        /// token is `user_reauth_required`, and the account lease moves to
        /// `reauth_required`.
        #[tokio::test]
        #[ignore = "live probe: needs COPILOT_LIVE_REVOKED_TOKEN, a revoked GitHub user token"]
        async fn live_operator_revoked_token_requires_reauth() {
            let token = live_token("COPILOT_LIVE_REVOKED_TOKEN");
            let checked = verify_operator_token(&token, &live_token_config()).await;
            assert!(
                matches!(
                    checked,
                    Err(ProviderAuthError::Auth(AuthError::UserReauthRequired))
                ),
                "{checked:?}"
            );

            let operator = OperatorEnv::new();
            let resolved = resolve_live(&operator, &token).await;
            assert!(
                matches!(
                    resolved,
                    Err(ProviderAuthError::Auth(AuthError::UserReauthRequired))
                ),
                "a revoked token needs a new token"
            );
            assert_eq!(operator.lease().phase, Some(AuthLeasePhase::ReauthRequired));
        }

        /// Live probe P5, no Copilot access: a token of an account without a
        /// Copilot seat, or a fine-grained token without the Copilot Requests
        /// permission, is `refresh_failed` with GitHub's message, and the
        /// lease stays valid, so the same token works once access is granted.
        #[tokio::test]
        #[ignore = "live probe: needs COPILOT_LIVE_NO_ACCESS_TOKEN, a GitHub token without Copilot access"]
        async fn live_operator_token_without_copilot_access_is_refused() {
            let token = live_token("COPILOT_LIVE_NO_ACCESS_TOKEN");
            let refused = verify_operator_token(&token, &live_token_config())
                .await
                .expect_err("Copilot refuses the token");
            eprintln!("copilot-live-token refusal: {refused}");
            assert!(
                matches!(
                    refused,
                    ProviderAuthError::Auth(AuthError::RefreshFailed(_))
                ),
                "{refused}"
            );

            let operator = OperatorEnv::new();
            let resolved = resolve_live(&operator, &token).await;
            assert!(
                matches!(
                    resolved,
                    Err(ProviderAuthError::Auth(AuthError::RefreshFailed(_)))
                ),
                "a refused identity is refresh_failed"
            );
            assert_eq!(operator.lease().phase, Some(AuthLeasePhase::Valid));
        }
    }

    /// Turns on a Copilot route through the actual runtime, authorizer and
    /// routed client. `ScriptedTransport` stands in for the token exchange
    /// and `/models`; a test client stands in for the provider adapter.
    mod route_turns {
        use super::*;
        use std::sync::atomic::AtomicI64;

        /// Resolve one session on `runtime` (its own authorizer on the shared
        /// account) and build its routed client over `factory`'s adapter.
        async fn routed_session(
            runtime: &Arc<CopilotRuntime>,
            binding: &ValidatedBinding,
            env: &ResolverEnvironment,
            factory: CopilotRouteClientFactory,
        ) -> Arc<dyn meerkat_llm_core::LlmClient> {
            try_routed_session(runtime, binding, env, factory)
                .await
                .unwrap()
        }

        /// The provider seam of an agent build: resolve the session's auth on
        /// `runtime`, then build its routed client over `factory`'s adapter.
        /// A resolution that fails is returned; the routed client must build
        /// once the resolution succeeded.
        async fn try_routed_session(
            runtime: &Arc<CopilotRuntime>,
            binding: &ValidatedBinding,
            env: &ResolverEnvironment,
            factory: CopilotRouteClientFactory,
        ) -> Result<Arc<dyn meerkat_llm_core::LlmClient>, ProviderAuthError> {
            let resolved = runtime.resolve(binding, env).await?;
            let connection = meerkat_llm_core::provider_runtime::ResolvedConnection {
                provider: Provider::OpenAI,
                backend: binding.backend(),
                backend_profile: binding.backend_profile().clone(),
                credential_identity: binding.credential_identity().clone(),
                auth_lease: Arc::new(
                    meerkat_llm_core::provider_runtime::DynamicLease::from_authorizer(
                        resolved.authorizer(),
                        resolved.metadata().clone(),
                        crate::GITHUB_COPILOT_AUTHORIZER_LABEL,
                    ),
                ),
            };
            Ok(routed_client(
                Arc::clone(runtime),
                connection,
                Provider::OpenAI,
                "gpt-test".to_string(),
                factory,
            )
            .expect("the routed client builds once its auth resolved"))
        }

        /// Which entry of the route's adapter a request reached.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        enum AdapterEntry {
            /// `stream`: the request arrived without its prepared form.
            Raw,
            /// `stream_prepared`, with what the prepared request carried.
            Prepared {
                authorized: bool,
                route_witness: bool,
            },
        }

        struct EntryRecordingClient {
            entries: Arc<Mutex<Vec<AdapterEntry>>>,
        }

        #[async_trait]
        impl meerkat_llm_core::LlmClient for EntryRecordingClient {
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
                self.entries.lock().push(AdapterEntry::Raw);
                Box::pin(futures::stream::empty())
            }

            fn stream_prepared<'a>(
                &'a self,
                request: &'a meerkat_llm_core::PreparedLlmRequest,
            ) -> meerkat_llm_core::LlmStream<'a> {
                self.entries.lock().push(AdapterEntry::Prepared {
                    authorized: request.authorization().is_some(),
                    route_witness: request.route_witness::<PreparedCopilotRoute>().is_some(),
                });
                Box::pin(futures::stream::empty())
            }

            fn provider(&self) -> Provider {
                Provider::OpenAI
            }

            async fn health_check(&self) -> Result<(), meerkat_llm_core::LlmError> {
                Ok(())
            }
        }

        /// The work policy of a governed turn. The stand-in adapter never
        /// prepares an operation, so it is never asked.
        struct UnaskedWorkPolicy;

        impl meerkat_core::authorization::WorkAuthorization for UnaskedWorkPolicy {
            fn prepare(
                &self,
                _binding: &meerkat_core::authorization::PreparedAuthorizationBinding,
            ) -> Result<
                Arc<dyn meerkat_core::authorization::PreparedOperationAuthorization>,
                meerkat_core::OperationAuthorizationError,
            > {
                Err(meerkat_core::authorization::OperationRefused::new(
                    meerkat_core::authorization::OperationRefusalKind::Denied,
                )
                .into())
            }
        }

        /// A route whose account model is known (so the account capability
        /// gate stands in front of the adapter) and allows 16 output tokens.
        async fn gated_route(
            entries: &Arc<Mutex<Vec<AdapterEntry>>>,
        ) -> Arc<dyn meerkat_llm_core::LlmClient> {
            let (auth_binding, binding) = validated_binding();
            let (env, _) = resolver_environment(&auth_binding).await;
            let runtime = Arc::new(CopilotRuntime {
                accounts: Mutex::new(BTreeMap::new()),
                pacing: Mutex::new(BTreeMap::new()),
                transport: Arc::new(ScriptedTransport::new([
                    token_response("derived"),
                    json_response(
                        200,
                        serde_json::json!({
                            "data": [{
                                "id": "gpt-test",
                                "supported_endpoints": ["/responses"],
                                "capabilities": {"limits": {"max_output_tokens": 16}}
                            }]
                        }),
                    ),
                ])),
            });
            let recorded = Arc::clone(entries);
            let factory: CopilotRouteClientFactory = Arc::new(move |_, _| {
                Ok(Arc::new(EntryRecordingClient {
                    entries: Arc::clone(&recorded),
                }))
            });
            routed_session(&runtime, &binding, &env, factory).await
        }

        /// A request projected through `client`, as the agent loop prepares
        /// one, optionally carrying the work authorization of a governed turn.
        fn prepared_request(
            client: &Arc<dyn meerkat_llm_core::LlmClient>,
            max_tokens: u32,
            authorized: bool,
        ) -> meerkat_llm_core::PreparedLlmRequest {
            let projection = client.project_replay_request(&[]).unwrap();
            let request = meerkat_llm_core::PreparedLlmRequest::from_projection(
                meerkat_llm_core::LlmRequest::new("gpt-test", Vec::new())
                    .with_max_tokens(max_tokens),
                projection,
            );
            if !authorized {
                return request;
            }
            request.with_authorization(Some(meerkat_core::LlmRequestAuthorization::new(
                meerkat_core::authorization::WorkAuthorizationContext::new(
                    Arc::new(UnaskedWorkPolicy),
                    meerkat_core::exact_operation::OperationExecutionScope::Domain,
                ),
                meerkat_core::OperationId::new(),
                meerkat_core::ModelAuthorizationUse::Inference,
            )))
        }

        #[tokio::test]
        async fn account_capability_gate_forwards_authorized_prepared_requests() {
            let entries = Arc::new(Mutex::new(Vec::new()));
            let client = gated_route(&entries).await;

            let events = client
                .stream_prepared(&prepared_request(&client, 16, true))
                .collect::<Vec<_>>()
                .await;
            assert!(
                events.is_empty(),
                "a governed request the account model admits must reach the route's adapter: {events:?}"
            );
            assert_eq!(
                entries.lock().as_slice(),
                [AdapterEntry::Prepared {
                    authorized: true,
                    route_witness: true,
                }],
                "the adapter receives the prepared request with its work authorization"
            );

            let events = client
                .stream_prepared(&prepared_request(&client, 17, true))
                .collect::<Vec<_>>()
                .await;
            assert!(
                matches!(
                    events.as_slice(),
                    [Err(meerkat_llm_core::LlmError::InvalidRequest { .. })]
                ),
                "the gate still refuses what the account model does not admit: {events:?}"
            );
            assert_eq!(
                entries.lock().len(),
                1,
                "a request the gate refuses never reaches the adapter"
            );
        }

        #[tokio::test]
        async fn account_capability_gate_keeps_the_route_witness_of_unauthorized_requests() {
            let entries = Arc::new(Mutex::new(Vec::new()));
            let client = gated_route(&entries).await;

            let events = client
                .stream_prepared(&prepared_request(&client, 16, false))
                .collect::<Vec<_>>()
                .await;
            assert!(events.is_empty(), "{events:?}");
            assert_eq!(
                entries.lock().as_slice(),
                [AdapterEntry::Prepared {
                    authorized: false,
                    route_witness: true,
                }],
                "the adapter receives the request in its prepared form"
            );
        }

        /// A clock the test sets: the resolver environment reads `base` plus
        /// the offset.
        #[derive(Clone)]
        struct TestClock {
            base: DateTime<Utc>,
            offset_seconds: Arc<AtomicI64>,
        }

        impl TestClock {
            fn new() -> Self {
                Self {
                    base: Utc::now(),
                    offset_seconds: Arc::new(AtomicI64::new(0)),
                }
            }

            fn at(&self, seconds: i64) -> DateTime<Utc> {
                self.base + Duration::seconds(seconds)
            }

            fn set(&self, seconds: i64) {
                self.offset_seconds.store(seconds, Ordering::SeqCst);
            }

            fn now(&self) -> DateTime<Utc> {
                self.at(self.offset_seconds.load(Ordering::SeqCst))
            }
        }

        /// A token response whose expiry and refresh hint sit on the test
        /// clock.
        fn token_at(
            token: &str,
            expires_at: DateTime<Utc>,
            refresh_in: u64,
        ) -> CopilotHttpResponse {
            json_response(
                200,
                serde_json::json!({
                    "token": token,
                    "expires_at": expires_at.timestamp(),
                    "refresh_in": refresh_in,
                    "endpoints": {"api": "https://copilot.example.test"}
                }),
            )
        }

        fn with_retry_after(response: CopilotHttpResponse, seconds: u64) -> CopilotHttpResponse {
            CopilotHttpResponse {
                retry_after: Some(StdDuration::from_secs(seconds)),
                ..response
            }
        }

        /// Stands in for a provider adapter on the route: signs each attempt
        /// through the route's authorizer, as the built-in adapters do, and
        /// answers it with the next scripted HTTP status (200 once the script
        /// is empty). A 401 is reported to the authorizer and retried once
        /// with fresh authorization; another 401 fails the call.
        struct StatusScriptedClient {
            authorizer: Arc<dyn HttpAuthorizer>,
            url: String,
            statuses: Arc<Mutex<VecDeque<u16>>>,
            bearers: Arc<Mutex<Vec<String>>>,
        }

        impl StatusScriptedClient {
            async fn call(&self) -> Result<(), meerkat_llm_core::LlmError> {
                let mut refreshed = false;
                loop {
                    let mut headers = Vec::new();
                    let receipt = self
                        .authorizer
                        .authorize_with_receipt(&mut HttpAuthorizationRequest {
                            method: "POST",
                            url: &self.url,
                            headers: &mut headers,
                        })
                        .await
                        .map_err(meerkat_llm_core::LlmError::from_authorizer)?;
                    if let Some((_, bearer)) = headers
                        .iter()
                        .find(|(name, _)| name.eq_ignore_ascii_case("authorization"))
                    {
                        self.bearers.lock().push(bearer.clone());
                    }
                    let status = self.statuses.lock().pop_front().unwrap_or(200);
                    if status != 401 {
                        return Ok(());
                    }
                    let action = self
                        .authorizer
                        .observe_response_with_receipt(
                            receipt,
                            &HttpAuthorizationResponse {
                                method: "POST",
                                url: &self.url,
                                status,
                            },
                        )
                        .await
                        .map_err(meerkat_llm_core::LlmError::from_authorizer)?;
                    if refreshed
                        || action != HttpAuthorizationResponseAction::RetryWithFreshAuthorization
                    {
                        return Err(meerkat_llm_core::LlmError::AuthenticationFailed {
                            message: "the provider answered 401".to_string(),
                        });
                    }
                    refreshed = true;
                }
            }

            fn turn(&self) -> meerkat_llm_core::LlmStream<'_> {
                Box::pin(futures::stream::once(self.call()).filter_map(|outcome| {
                    futures::future::ready(
                        outcome
                            .err()
                            .map(Err::<meerkat_llm_core::LlmEvent, meerkat_llm_core::LlmError>),
                    )
                }))
            }
        }

        #[async_trait]
        impl meerkat_llm_core::LlmClient for StatusScriptedClient {
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
                self.turn()
            }

            fn stream_prepared<'a>(
                &'a self,
                _request: &'a meerkat_llm_core::PreparedLlmRequest,
            ) -> meerkat_llm_core::LlmStream<'a> {
                self.turn()
            }

            fn provider(&self) -> Provider {
                Provider::OpenAI
            }

            async fn health_check(&self) -> Result<(), meerkat_llm_core::LlmError> {
                Ok(())
            }
        }

        /// One session's routed client, and the bearers its stand-in adapter
        /// signed with.
        struct RouteSession {
            client: Arc<dyn meerkat_llm_core::LlmClient>,
            bearers: Arc<Mutex<Vec<String>>>,
        }

        impl RouteSession {
            /// Prepare a turn's request as the agent loop does: the replay
            /// projection, then pressure and cache lowering through its route
            /// witness.
            fn prepare(
                &self,
            ) -> Result<meerkat_llm_core::PreparedLlmRequest, meerkat_llm_core::LlmError>
            {
                let projection = self.client.project_replay_request(&[])?;
                let request = meerkat_llm_core::PreparedLlmRequest::from_projection(
                    meerkat_llm_core::LlmRequest::new("gpt-test", Vec::new()),
                    projection,
                );
                self.client.prepared_request_pressure(&request)?;
                self.client.prepared_cache_breakpoints(&request, &[])?;
                Ok(request)
            }

            async fn turn(
                &self,
            ) -> Vec<Result<meerkat_llm_core::LlmEvent, meerkat_llm_core::LlmError>> {
                match self.prepare() {
                    Ok(request) => {
                        self.client
                            .stream_prepared(&request)
                            .collect::<Vec<_>>()
                            .await
                    }
                    Err(error) => vec![Err(error)],
                }
            }

            /// A turn as the agent loop runs it when the route changes: an
            /// attempt that fails with the retryable
            /// `AuthorizationRouteChanged` is prepared again on the route the
            /// account now resolves, and sent again.
            async fn turn_across_a_route_change(
                &self,
            ) -> Vec<Result<meerkat_llm_core::LlmEvent, meerkat_llm_core::LlmError>> {
                let events = self.turn().await;
                if matches!(
                    events.as_slice(),
                    [Err(
                        meerkat_llm_core::LlmError::AuthorizationRouteChanged { .. }
                    )]
                ) {
                    return self.turn().await;
                }
                events
            }

            fn bearers(&self) -> Vec<String> {
                self.bearers.lock().clone()
            }
        }

        /// One Copilot account: the runtime over a scripted transport, and
        /// the binding and resolver environment (on the test clock) every
        /// session on it resolves with.
        struct RouteAccount {
            runtime: Arc<CopilotRuntime>,
            binding: ValidatedBinding,
            env: ResolverEnvironment,
            auth_lease: meerkat_core::handles::GeneratedAuthLeaseHandle,
        }

        impl RouteAccount {
            async fn new(transport: Arc<dyn CopilotHttpTransport>, clock: &TestClock) -> Self {
                let (auth_binding, binding) = validated_binding();
                let (mut env, auth_lease) = resolver_environment(&auth_binding).await;
                let clock = clock.clone();
                env.now = Arc::new(move || clock.now());
                Self {
                    runtime: Arc::new(CopilotRuntime {
                        accounts: Mutex::new(BTreeMap::new()),
                        pacing: Mutex::new(BTreeMap::new()),
                        transport,
                    }),
                    binding,
                    env,
                    auth_lease,
                }
            }

            /// Resolve a session whose stand-in adapter answers `statuses`.
            async fn session(&self, statuses: impl IntoIterator<Item = u16>) -> RouteSession {
                self.try_session(statuses).await.unwrap()
            }

            /// Build a session as an agent build does (resolution, then its
            /// routed client), its stand-in adapter answering `statuses`.
            async fn try_session(
                &self,
                statuses: impl IntoIterator<Item = u16>,
            ) -> Result<RouteSession, ProviderAuthError> {
                let statuses = Arc::new(Mutex::new(statuses.into_iter().collect::<VecDeque<_>>()));
                let bearers = Arc::new(Mutex::new(Vec::new()));
                let signed = Arc::clone(&bearers);
                let factory: CopilotRouteClientFactory = Arc::new(move |route, connection| {
                    let authorizer = connection
                        .resolved_authorizer()
                        .ok_or(ProviderClientError::NoCredentialMaterial)?;
                    Ok(Arc::new(StatusScriptedClient {
                        authorizer: route.bind_authorizer(authorizer),
                        url: format!("{}/responses", route.api_base),
                        statuses: Arc::clone(&statuses),
                        bearers: Arc::clone(&signed),
                    }))
                });
                Ok(RouteSession {
                    client: try_routed_session(&self.runtime, &self.binding, &self.env, factory)
                        .await?,
                    bearers,
                })
            }

            fn source_phase(&self) -> Option<meerkat_core::handles::AuthLeasePhase> {
                self.auth_lease
                    .snapshot(&meerkat_core::handles::LeaseKey::from_credential_identity(
                        &account_identity(),
                    ))
                    .phase
            }

            /// An account whose builds admit a token exchange outage, as the
            /// factory's builds whose output budget is known before
            /// resolution do.
            async fn admitting_pending_limits(
                transport: Arc<dyn CopilotHttpTransport>,
                clock: &TestClock,
            ) -> Self {
                Self::new(transport, clock).await.admitting()
            }

            /// The same account (runtime, sign-in and binding), resolving with
            /// an environment that admits pending account limits.
            fn admitting(&self) -> Self {
                Self {
                    runtime: Arc::clone(&self.runtime),
                    binding: self.binding.clone(),
                    env: self.env.clone().admitting_pending_account_limits(),
                    auth_lease: self.auth_lease.clone(),
                }
            }

            /// The retryable class of the outage the account recorded.
            fn recorded_outage(&self) -> Option<meerkat_llm_core::LlmError> {
                let state = self
                    .runtime
                    .accounts
                    .lock()
                    .values()
                    .find_map(Weak::upgrade)?;
                let cache = state.cache.read();
                cache
                    .as_ref()?
                    .exchange_unavailable
                    .as_ref()
                    .map(|unavailable| unavailable.retryable.clone())
            }

            /// Whether the account's current model list names `model`.
            fn lists_model(&self, model: &str) -> bool {
                self.runtime
                    .accounts
                    .lock()
                    .values()
                    .find_map(Weak::upgrade)
                    .is_some_and(|state| {
                        state.cache.read().as_ref().is_some_and(|cached| {
                            cached
                                .models
                                .as_ref()
                                .is_some_and(|models| models.model(model).is_some())
                        })
                    })
            }

            /// A new sign-in on the account: another GitHub token, expiring
            /// at `expires_at`, published through AuthMachine and stored, as
            /// a new login does.
            async fn sign_in_again(&self, github_token: &str, expires_at: DateTime<Utc>) {
                let identity = account_identity();
                let tokens = PersistedTokens {
                    auth_mode: PersistedAuthMode::GithubCopilotOauth,
                    primary_secret: Some(github_token.to_string()),
                    refresh_token: None,
                    id_token: None,
                    expires_at: Some(expires_at),
                    last_refresh: Some(Utc::now()),
                    scopes: vec!["read:user".to_string()],
                    account_id: None,
                    metadata: serde_json::Value::Null,
                };
                let transition = meerkat_core::publish_token_lifecycle_acquired_for_identity(
                    &self.auth_lease,
                    &identity,
                    &tokens,
                )
                .expect("publish the new sign-in");
                let key = meerkat_core::auth::TokenKey::from_credential_identity(&identity);
                let marked = meerkat_core::mark_tokens_lifecycle_published_for_transition(
                    &key,
                    &tokens,
                    &transition,
                )
                .expect("mark the new sign-in");
                self.env
                    .provider_auth_persistence()
                    .expect("test persistence")
                    .token_store()
                    .save(&key, &marked)
                    .await
                    .expect("store the new sign-in");
            }
        }

        /// Whether the agent loop's retry policy retries this turn failure.
        fn retried(error: &meerkat_llm_core::LlmError) -> bool {
            meerkat_core::retry::LlmRetryFailure::from_agent_error(
                &error.clone().into_agent_error("openai"),
            )
            .is_some()
        }

        #[tokio::test]
        async fn exchange_outage_before_expiry_keeps_serving_the_current_token() {
            let clock = TestClock::new();
            let transport = Arc::new(ScriptedTransport::new([
                token_at("derived-one", clock.at(3600), 200),
                models_response(),
                with_retry_after(
                    json_response(503, serde_json::json!({"message": "unavailable"})),
                    60,
                ),
                token_at("derived-two", clock.at(311 + 3600), 200),
                models_response(),
            ]));
            let account = RouteAccount::new(transport.clone(), &clock).await;
            let session = account.session([]).await;
            assert_eq!(transport.request_count(), 2);

            // Past the token's refresh point (200 s), well before it expires
            // (3600 s).
            clock.set(250);
            let events = session.turn().await;
            assert!(
                events.is_empty(),
                "an exchange outage must not fail a turn the current token can still serve: {events:?}"
            );
            assert_eq!(
                transport.request_count(),
                3,
                "the refresh point tried the exchange"
            );

            // Within the exchange's Retry-After (60 s).
            clock.set(290);
            let events = session.turn().await;
            assert!(events.is_empty(), "{events:?}");
            assert_eq!(
                transport.request_count(),
                3,
                "the exchange is not tried again before its Retry-After"
            );

            clock.set(311);
            let events = session.turn().await;
            assert!(events.is_empty(), "{events:?}");
            assert_eq!(
                transport.request_count(),
                5,
                "after its Retry-After the exchange is tried again"
            );
            assert_eq!(
                session.bearers(),
                [
                    "Bearer derived-one",
                    "Bearer derived-one",
                    "Bearer derived-two"
                ]
            );
        }

        #[tokio::test]
        async fn exchange_outage_after_expiry_fails_the_turn_as_retryable() {
            let outages = [
                (
                    "HTTP 503",
                    Some(json_response(
                        503,
                        serde_json::json!({"message": "unavailable"}),
                    )),
                    None,
                ),
                (
                    "HTTP 429",
                    Some(with_retry_after(
                        json_response(429, serde_json::json!({"message": "slow down"})),
                        7,
                    )),
                    Some(StdDuration::from_secs(7)),
                ),
                (
                    "GitHub API rate limit 403",
                    Some(json_response(
                        403,
                        serde_json::json!({"message": "API rate limit exceeded for user"}),
                    )),
                    None,
                ),
                ("transport failure", None, None),
            ];
            for (case, outage, retry_after) in outages {
                let clock = TestClock::new();
                let mut script = vec![
                    token_at("derived-one", clock.at(3600), 200),
                    models_response(),
                ];
                script.extend(outage);
                let transport = Arc::new(ScriptedTransport::new(script));
                let account = RouteAccount::new(transport.clone(), &clock).await;
                let session = account.session([]).await;

                clock.set(3700);
                let events = session.turn().await;
                let [Err(error)] = events.as_slice() else {
                    panic!("{case}: the turn fails once: {events:?}");
                };
                assert!(
                    retried(error),
                    "{case}: an exchange outage after expiry must fail the turn as retryable, not as {error:?}"
                );
                assert_eq!(
                    error.retry_after(),
                    retry_after,
                    "{case}: the exchange's Retry-After reaches the retry policy"
                );
                assert_eq!(transport.request_count(), 3, "{case}");
                assert!(
                    session.bearers().is_empty(),
                    "{case}: nothing is sent without a token"
                );
            }
        }

        #[tokio::test]
        async fn exchange_refusal_at_the_refresh_point_stays_an_auth_failure() {
            let clock = TestClock::new();
            let transport = Arc::new(ScriptedTransport::new([
                token_at("derived-one", clock.at(3600), 200),
                models_response(),
                json_response(401, serde_json::json!({"message": "Bad credentials"})),
            ]));
            let account = RouteAccount::new(transport.clone(), &clock).await;
            let session = account.session([]).await;

            clock.set(250);
            let events = session.turn().await;
            let [Err(error)] = events.as_slice() else {
                panic!("the turn fails once: {events:?}");
            };
            assert!(
                matches!(
                    error.failure_reason(),
                    meerkat_core::error::LlmFailureReason::AuthError
                ) && !retried(error),
                "an exchange 401 is an auth failure even while the current token is valid: {error:?}"
            );
            assert_eq!(
                account.source_phase(),
                Some(meerkat_core::handles::AuthLeasePhase::ReauthRequired)
            );
            assert_eq!(transport.request_count(), 3);
            assert!(session.bearers().is_empty());
        }

        #[tokio::test]
        async fn exchange_outage_after_a_model_401_fails_the_turn_as_retryable() {
            let clock = TestClock::new();
            let transport = Arc::new(ScriptedTransport::new([
                token_at("derived-one", clock.at(3600), 200),
                models_response(),
                json_response(503, serde_json::json!({"message": "unavailable"})),
            ]));
            let account = RouteAccount::new(transport.clone(), &clock).await;
            let session = account.session([401]).await;

            let events = session.turn().await;
            let [Err(error)] = events.as_slice() else {
                panic!("the turn fails once: {events:?}");
            };
            assert!(
                retried(error),
                "a 401 whose new token meets an exchange outage must fail the turn as retryable, not as {error:?}"
            );
            assert_eq!(transport.request_count(), 3);
            assert_eq!(session.bearers(), ["Bearer derived-one"]);
        }

        /// Answers like `ScriptedTransport`, but holds request number
        /// `hold_at` (counted from zero) until the test releases it.
        struct HeldTransport {
            inner: ScriptedTransport,
            hold_at: u64,
            calls: AtomicU64,
            held: tokio::sync::Notify,
            release: tokio::sync::Notify,
        }

        #[async_trait]
        impl CopilotHttpTransport for HeldTransport {
            async fn get(
                &self,
                url: &str,
                headers: HeaderMap,
            ) -> Result<CopilotHttpResponse, CopilotHttpTransportError> {
                if self.calls.fetch_add(1, Ordering::SeqCst) == self.hold_at {
                    self.held.notify_one();
                    self.release.notified().await;
                }
                self.inner.get(url, headers).await
            }
        }

        #[tokio::test]
        async fn a_turn_after_two_model_401s_mints_a_new_token_instead_of_failing_at_projection() {
            let clock = TestClock::new();
            let transport = Arc::new(ScriptedTransport::new([
                token_response("derived-one"),
                models_response(),
                token_response("derived-two"),
                models_response(),
                token_response("derived-three"),
                models_response(),
            ]));
            let account = RouteAccount::new(transport.clone(), &clock).await;
            let session = account.session([401, 401]).await;

            let events = session.turn().await;
            assert!(
                matches!(
                    events.as_slice(),
                    [Err(meerkat_llm_core::LlmError::AuthenticationFailed { .. })]
                ),
                "{events:?}"
            );
            assert_eq!(
                transport.request_count(),
                4,
                "the first 401 minted one new token"
            );

            let events = session.turn().await;
            assert!(
                events.is_empty(),
                "the next turn must mint a new token, not fail before it reaches the network: {events:?}"
            );
            assert_eq!(transport.request_count(), 6);
            assert_eq!(
                session.bearers(),
                [
                    "Bearer derived-one",
                    "Bearer derived-two",
                    "Bearer derived-three"
                ]
            );
        }

        #[tokio::test]
        async fn a_second_session_keeps_its_route_while_the_first_mints_a_new_token() {
            let clock = TestClock::new();
            let transport = Arc::new(HeldTransport {
                inner: ScriptedTransport::new([
                    token_response("derived-one"),
                    models_response(),
                    token_response("derived-two"),
                    models_response(),
                ]),
                // The token exchange the first session's 401 starts.
                hold_at: 2,
                calls: AtomicU64::new(0),
                held: tokio::sync::Notify::new(),
                release: tokio::sync::Notify::new(),
            });
            let account = RouteAccount::new(transport.clone(), &clock).await;
            let first = account.session([401]).await;
            let second = account.session([]).await;
            assert_eq!(
                transport.inner.request_count(),
                2,
                "both sessions share one token"
            );

            let second_turn = async {
                transport.held.notified().await;
                // The first session's new token is being minted now.
                let prepared = second.prepare();
                transport.release.notify_one();
                let events = match &prepared {
                    Ok(request) => {
                        second
                            .client
                            .stream_prepared(request)
                            .collect::<Vec<_>>()
                            .await
                    }
                    Err(_) => Vec::new(),
                };
                (prepared.map(|_| ()), events)
            };
            let (first_events, (second_prepared, second_events)) =
                tokio::join!(first.turn(), second_turn);

            assert!(
                second_prepared.is_ok(),
                "a session must keep its route while another session mints the shared token anew: {second_prepared:?}"
            );
            assert!(first_events.is_empty(), "{first_events:?}");
            assert!(second_events.is_empty(), "{second_events:?}");
            assert_eq!(
                first.bearers(),
                ["Bearer derived-one", "Bearer derived-two"]
            );
            assert_eq!(second.bearers(), ["Bearer derived-two"]);
            assert_eq!(transport.inner.request_count(), 4);
        }

        /// A discovery 403 that asks for an account action refuses the
        /// token; once the new token's own listing asks for it again, every
        /// turn reports the reauthorization the account needs.
        #[tokio::test]
        async fn a_turn_after_a_discovery_403_reports_the_reauthorization_instead_of_a_lost_route()
        {
            let clock = TestClock::new();
            let transport = Arc::new(ScriptedTransport::new([
                token_response("derived-one"),
                models_response(),
                listing_needs_an_account_action(),
                token_response("derived-two"),
                listing_needs_an_account_action(),
            ]));
            let account = RouteAccount::new(transport.clone(), &clock).await;
            let session = account.session([]).await;

            // Model discovery is due again (300 s); the token is still fresh.
            clock.set(301);
            assert!(session.turn().await.is_empty());
            settle(|| transport.request_count() == 3).await;

            for turn in ["first", "second"] {
                let events = session.turn().await;
                assert!(
                    matches!(
                        events.as_slice(),
                        [Err(meerkat_llm_core::LlmError::AuthenticationFailed { .. })]
                    ),
                    "the {turn} turn after the refusal must report the reauthorization the account needs, not a lost route: {events:?}"
                );
            }
            assert_eq!(
                account.source_phase(),
                Some(meerkat_core::handles::AuthLeasePhase::ReauthRequired)
            );
            assert_eq!(transport.request_count(), 5);
        }

        #[tokio::test]
        async fn a_turn_after_a_discovery_401_mints_a_new_token_instead_of_failing_at_projection() {
            let clock = TestClock::new();
            let transport = Arc::new(ScriptedTransport::new([
                token_response("derived-one"),
                models_response(),
                json_response(401, serde_json::json!({"message": "expired"})),
                json_response(503, serde_json::json!({"message": "unavailable"})),
                token_response("derived-two"),
                models_response(),
            ]));
            let account = RouteAccount::new(transport.clone(), &clock).await;
            let session = account.session([]).await;

            // Model discovery is due again (300 s): the turn goes out on the
            // token while the listing refuses it; the token exchange is then
            // unavailable for the new one.
            clock.set(301);
            assert!(session.turn().await.is_empty());
            settle(|| transport.request_count() == 3).await;
            let events = session.turn().await;
            assert!(matches!(events.as_slice(), [Err(_)]), "{events:?}");
            assert_eq!(transport.request_count(), 4);

            let events = session.turn().await;
            assert!(
                events.is_empty(),
                "the next turn must mint a new token, not fail before it reaches the network: {events:?}"
            );
            assert_eq!(transport.request_count(), 6);
            assert_eq!(
                session.bearers(),
                ["Bearer derived-one", "Bearer derived-two"]
            );
        }

        /// A model listing 403 that asks for an account action:
        /// `error_details.notification_id`, as the token exchange reads it.
        fn listing_needs_an_account_action() -> CopilotHttpResponse {
            json_response(
                403,
                serde_json::json!({
                    "message": "Copilot access needs an action on the account",
                    "error_details": {"notification_id": "copilot_account_action"}
                }),
            )
        }

        /// The account's model list is refreshed off the request path: a turn
        /// that finds the list due goes out on the token and the list the
        /// account holds while one discovery runs for the account, and the
        /// list it brings serves the turns after it.
        #[tokio::test]
        async fn periodic_model_discovery_runs_off_the_request_path() {
            let clock = TestClock::new();
            let transport = Arc::new(HeldTransport {
                inner: ScriptedTransport::new([
                    token_response("derived-one"),
                    models_response(),
                    models_listing_with("gpt-later"),
                ]),
                hold_at: 2,
                calls: AtomicU64::new(0),
                held: tokio::sync::Notify::new(),
                release: tokio::sync::Notify::new(),
            });
            let account = RouteAccount::new(transport.clone(), &clock).await;
            let session = account.session([]).await;

            // Model discovery is due again (300 s); the token is still fresh.
            clock.set(301);
            for turn in ["first", "second"] {
                let events = tokio::time::timeout(StdDuration::from_secs(5), session.turn())
                    .await
                    .unwrap_or_else(|_| {
                        panic!("the {turn} turn must not wait for the account's model discovery")
                    });
                assert!(events.is_empty(), "{turn} turn: {events:?}");
            }
            transport.held.notified().await;
            assert_eq!(
                transport.calls.load(Ordering::SeqCst),
                3,
                "one discovery runs for the account"
            );
            assert!(!account.lists_model("gpt-later"));

            transport.release.notify_one();
            settle(|| account.lists_model("gpt-later")).await;
            assert_eq!(
                session.bearers(),
                ["Bearer derived-one", "Bearer derived-one"]
            );
            assert_eq!(
                account.source_phase(),
                Some(meerkat_core::handles::AuthLeasePhase::Valid)
            );
        }

        /// A model listing 403 that asks for no account action (a policy, an
        /// integration the account does not allow) is a discovery failure:
        /// the token keeps serving, the sign-in stays valid, and the list is
        /// asked for again after the retry delay.
        #[tokio::test]
        async fn a_discovery_403_without_an_account_action_keeps_the_route() {
            let clock = TestClock::new();
            let transport = Arc::new(ScriptedTransport::new([
                token_response("derived-one"),
                models_response(),
                json_response(
                    403,
                    serde_json::json!({"message": "Copilot model listing is not allowed"}),
                ),
                models_listing_with("gpt-later"),
            ]));
            let account = RouteAccount::new(transport.clone(), &clock).await;
            let session = account.session([]).await;

            clock.set(301);
            let events = session.turn().await;
            assert!(
                events.is_empty(),
                "a model listing 403 without an account action fails no turn: {events:?}"
            );
            settle(|| transport.request_count() == 3).await;
            assert_eq!(
                account.source_phase(),
                Some(meerkat_core::handles::AuthLeasePhase::Valid),
                "a model listing 403 without an account action needs no new sign-in"
            );

            // Within the 30 s retry delay nothing lists the models again.
            clock.set(320);
            assert!(session.turn().await.is_empty());
            assert_eq!(transport.request_count(), 3);

            clock.set(332);
            assert!(session.turn().await.is_empty());
            settle(|| account.lists_model("gpt-later")).await;
            assert_eq!(transport.request_count(), 4);
            assert_eq!(session.bearers(), ["Bearer derived-one"; 3]);
        }

        /// A model listing 403 that asks for an account action refuses the
        /// token: the turn that found the list due is served, and the next
        /// one mints a new token, whose listing decides: the account needs a
        /// new sign-in.
        #[tokio::test]
        async fn a_discovery_403_that_asks_for_an_account_action_needs_a_new_sign_in() {
            let clock = TestClock::new();
            let transport = Arc::new(ScriptedTransport::new([
                token_response("derived-one"),
                models_response(),
                listing_needs_an_account_action(),
                token_response("derived-two"),
                listing_needs_an_account_action(),
            ]));
            let account = RouteAccount::new(transport.clone(), &clock).await;
            let session = account.session([]).await;

            clock.set(301);
            let events = session.turn().await;
            assert!(
                events.is_empty(),
                "the turn that found the list due is served while the list is asked for: {events:?}"
            );
            settle(|| transport.request_count() == 3).await;

            let events = session.turn().await;
            let [Err(error)] = events.as_slice() else {
                panic!("the next turn fails once: {events:?}");
            };
            assert!(
                matches!(
                    error.failure_reason(),
                    meerkat_core::error::LlmFailureReason::AuthError
                ) && !retried(error),
                "an account action is an auth failure: {error:?}"
            );
            assert_eq!(
                account.source_phase(),
                Some(meerkat_core::handles::AuthLeasePhase::ReauthRequired)
            );
            assert_eq!(transport.request_count(), 5);
            assert_eq!(session.bearers(), ["Bearer derived-one"]);
        }

        /// A mint whose model listing answers a 403 that asks for no account
        /// action resolves without the account's list, as for any other
        /// discovery failure: the sign-in stays valid and the token serves.
        #[tokio::test]
        async fn a_mint_whose_model_listing_answers_403_resolves_without_the_list() {
            let clock = TestClock::new();
            let transport = Arc::new(ScriptedTransport::new([
                token_response("derived-one"),
                json_response(
                    403,
                    serde_json::json!({"message": "Copilot model listing is not allowed"}),
                ),
            ]));
            let account = RouteAccount::new(transport.clone(), &clock).await;
            let session = account.try_session([]).await.expect(
                "a model listing 403 without an account action is a discovery failure, not a new sign-in",
            );
            assert_eq!(
                account.source_phase(),
                Some(meerkat_core::handles::AuthLeasePhase::Valid)
            );
            assert!(session.turn().await.is_empty());
            assert_eq!(session.bearers(), ["Bearer derived-one"]);
            assert_eq!(transport.request_count(), 2);
        }

        /// The retryable class of the exchange failure the account recorded.
        fn recorded_exchange_failure(
            authorizer: &CopilotAuthorizer,
        ) -> Option<meerkat_llm_core::LlmError> {
            authorizer.state.cache.read().as_ref().and_then(|cached| {
                cached
                    .exchange_unavailable
                    .as_ref()
                    .map(|unavailable| unavailable.retryable.clone())
            })
        }

        #[tokio::test]
        async fn a_recorded_exchange_failure_lasts_until_the_exchange_answers() {
            let clock = TestClock::new();
            let transport = Arc::new(ScriptedTransport::new([
                token_at("derived-one", clock.at(3600), 200),
                models_response(),
                json_response(503, serde_json::json!({"message": "unavailable"})),
                json_response(404, serde_json::json!({"message": "Not Found"})),
                json_response(503, serde_json::json!({"message": "unavailable"})),
                token_at("derived-two", clock.at(3700 + 3600), 200),
                models_response(),
            ]));
            let account = RouteAccount::new(transport.clone(), &clock).await;
            let authorizer = account
                .runtime
                .resolve(&account.binding, &account.env)
                .await
                .unwrap()
                .concrete_authorizer();

            // The token has expired: each preparation tries the exchange.
            clock.set(3700);
            assert!(authorizer.prepare_request().await.is_err());
            assert!(
                matches!(
                    recorded_exchange_failure(&authorizer),
                    Some(meerkat_llm_core::LlmError::ServerError { status: 503, .. })
                ),
                "an outage that leaves no token to serve is recorded with its class"
            );
            assert!(authorizer.prepare_request().await.is_err());
            assert!(
                recorded_exchange_failure(&authorizer).is_none(),
                "a failure that is not transient clears the record"
            );
            assert!(authorizer.prepare_request().await.is_err());
            assert!(recorded_exchange_failure(&authorizer).is_some());
            authorizer
                .prepare_request()
                .await
                .expect("the exchange answers again");
            assert!(
                recorded_exchange_failure(&authorizer).is_none(),
                "a successful mint clears the record"
            );
            assert_eq!(transport.request_count(), 7);
        }

        /// A token exchange outage that leaves no token to serve, and whose
        /// exchange said when to come back (`Retry-After`), answers every
        /// request until then with its retryable class without calling the
        /// exchange; the first request after that instant tries it again.
        #[tokio::test]
        async fn an_exchange_outage_answers_until_its_retry_after() {
            let clock = TestClock::new();
            let transport = Arc::new(ScriptedTransport::new([
                token_at("derived-one", clock.at(3600), 200),
                models_response(),
                with_retry_after(
                    json_response(503, serde_json::json!({"message": "unavailable"})),
                    60,
                ),
                token_at("derived-two", clock.at(3761 + 3600), 200),
                models_response(),
            ]));
            let account = RouteAccount::new(transport.clone(), &clock).await;
            let session = account.session([]).await;

            // The token expired; the exchange is unavailable for 60 s.
            clock.set(3700);
            let events = session.turn().await;
            let [Err(error)] = events.as_slice() else {
                panic!("the turn fails once: {events:?}");
            };
            assert!(retried(error), "{error:?}");
            assert_eq!(transport.request_count(), 3);

            for at in [3701, 3730, 3759] {
                clock.set(at);
                let events = session.turn().await;
                let [Err(error)] = events.as_slice() else {
                    panic!("{at}: the turn fails once: {events:?}");
                };
                assert!(
                    matches!(
                        error,
                        meerkat_llm_core::LlmError::ServerError { status: 503, .. }
                    ) && retried(error),
                    "{at}: the recorded outage answers with its retryable class: {error:?}"
                );
                assert_eq!(
                    transport.request_count(),
                    3,
                    "{at}: the exchange is not called before its Retry-After"
                );
            }

            clock.set(3761);
            let events = session.turn().await;
            assert!(
                events.is_empty(),
                "after its Retry-After the exchange is tried again: {events:?}"
            );
            assert_eq!(transport.request_count(), 5);
            assert_eq!(session.bearers(), ["Bearer derived-two"]);
        }

        #[tokio::test]
        async fn a_failure_after_a_recorded_outage_fails_the_turn_as_itself() {
            let clock = TestClock::new();
            let transport = Arc::new(ScriptedTransport::new([
                token_at("derived-one", clock.at(3600), 200),
                models_response(),
                json_response(503, serde_json::json!({"message": "unavailable"})),
                json_response(404, serde_json::json!({"message": "Not Found"})),
            ]));
            let account = RouteAccount::new(transport.clone(), &clock).await;
            let session = account.session([]).await;

            clock.set(3700);
            let events = session.turn().await;
            let [Err(error)] = events.as_slice() else {
                panic!("the turn fails once: {events:?}");
            };
            assert!(retried(error), "{error:?}");

            let events = session.turn().await;
            let [Err(error)] = events.as_slice() else {
                panic!("the turn fails once: {events:?}");
            };
            assert!(
                matches!(
                    error,
                    meerkat_llm_core::LlmError::AuthenticationFailed { .. }
                ) && !retried(error),
                "a later failure that is not transient must not be reported as the earlier outage: {error:?}"
            );
            assert_eq!(transport.request_count(), 4);
        }

        #[tokio::test]
        async fn a_session_builds_during_an_exchange_outage_before_any_token() {
            let clock = TestClock::new();
            let transport = Arc::new(ScriptedTransport::new([json_response(
                503,
                serde_json::json!({"message": "unavailable"}),
            )]));
            let account = RouteAccount::admitting_pending_limits(transport.clone(), &clock).await;

            let session = account.try_session([]).await;
            assert!(
                session.is_ok(),
                "a transient token exchange outage must not fail the session's build: {:?}",
                session.as_ref().err()
            );
            assert_eq!(transport.request_count(), 1);
            assert_eq!(
                account.source_phase(),
                Some(meerkat_core::handles::AuthLeasePhase::Valid),
                "an outage marks nothing on the sign-in"
            );
        }

        #[tokio::test]
        async fn another_session_builds_during_an_exchange_outage_after_expiry() {
            let clock = TestClock::new();
            let transport = Arc::new(ScriptedTransport::new([
                token_at("derived-one", clock.at(3600), 200),
                models_response(),
                json_response(503, serde_json::json!({"message": "unavailable"})),
            ]));
            let account = RouteAccount::admitting_pending_limits(transport.clone(), &clock).await;
            let _first = account.session([]).await;

            // The account's token has expired and the exchange is down.
            clock.set(3700);
            let second = account.try_session([]).await;
            assert!(
                second.is_ok(),
                "a transient token exchange outage must not fail another session's build: {:?}",
                second.as_ref().err()
            );
            assert_eq!(transport.request_count(), 3);
        }

        #[tokio::test]
        async fn an_exchange_refusal_still_fails_the_build_and_marks_reauthorization() {
            let clock = TestClock::new();
            let transport = Arc::new(ScriptedTransport::new([json_response(
                401,
                serde_json::json!({"message": "Bad credentials"}),
            )]));
            let account = RouteAccount::admitting_pending_limits(transport.clone(), &clock).await;

            let Err(error) = account.try_session([]).await else {
                panic!("an exchange refusal must fail the build");
            };
            assert!(
                matches!(
                    error,
                    ProviderAuthError::Auth(AuthError::UserReauthRequired)
                ),
                "{error:?}"
            );
            assert_eq!(
                account.source_phase(),
                Some(meerkat_core::handles::AuthLeasePhase::ReauthRequired)
            );
            assert_eq!(transport.request_count(), 1);
        }

        #[tokio::test]
        async fn the_first_turn_after_an_exchange_outage_mints_and_succeeds() {
            let clock = TestClock::new();
            let transport = Arc::new(ScriptedTransport::new([
                json_response(503, serde_json::json!({"message": "unavailable"})),
                json_response(503, serde_json::json!({"message": "unavailable"})),
                token_at("derived-one", clock.at(3600), 200),
                models_response(),
            ]));
            let account = RouteAccount::admitting_pending_limits(transport.clone(), &clock).await;
            let session = account.try_session([]).await;
            assert!(
                session.is_ok(),
                "a transient token exchange outage must not fail the session's build: {:?}",
                session.as_ref().err()
            );
            let session = session.unwrap();

            // While the outage lasts, a turn fails with a retryable error.
            let events = session.turn().await;
            let [Err(error)] = events.as_slice() else {
                panic!("the turn fails once: {events:?}");
            };
            assert!(retried(error), "{error:?}");
            assert!(session.bearers().is_empty());

            // Once the exchange answers, the turn mints a token, moves to
            // the route the account's model list gives, and succeeds.
            let events = session.turn_across_a_route_change().await;
            assert!(events.is_empty(), "{events:?}");
            assert_eq!(session.bearers(), ["Bearer derived-one"]);
            assert_eq!(transport.request_count(), 4);
        }

        #[tokio::test]
        async fn a_build_that_needs_the_account_limits_still_fails_during_an_outage() {
            let clock = TestClock::new();
            let transport = Arc::new(ScriptedTransport::new([json_response(
                503,
                serde_json::json!({"message": "unavailable"}),
            )]));
            // A resolution that does not admit pending account limits, as a
            // new session whose output budget comes from the account's model
            // limit resolves.
            let account = RouteAccount::new(transport.clone(), &clock).await;

            let Err(error) = account.try_session([]).await else {
                panic!(
                    "a build that needs the account's limits must fail while the exchange is down"
                );
            };
            assert!(
                matches!(
                    &error,
                    ProviderAuthError::SourceResolutionFailed(message)
                        if message.contains("returned 503")
                ),
                "{error:?}"
            );
            assert_eq!(transport.request_count(), 1);
            assert_eq!(
                account.source_phase(),
                Some(meerkat_core::handles::AuthLeasePhase::Valid),
                "an outage marks nothing on the sign-in"
            );
        }

        #[tokio::test]
        async fn an_exchange_refusal_after_an_outage_fails_the_build_and_ends_the_outage() {
            let clock = TestClock::new();
            let transport = Arc::new(ScriptedTransport::new([
                json_response(503, serde_json::json!({"message": "unavailable"})),
                json_response(401, serde_json::json!({"message": "Bad credentials"})),
            ]));
            let account = RouteAccount::admitting_pending_limits(transport.clone(), &clock).await;
            let first = account.session([]).await;
            assert!(account.recorded_outage().is_some());

            let Err(error) = account.try_session([]).await else {
                panic!("an exchange refusal after an outage must fail the build");
            };
            assert!(
                matches!(
                    error,
                    ProviderAuthError::Auth(AuthError::UserReauthRequired)
                ),
                "{error:?}"
            );
            assert_eq!(
                account.source_phase(),
                Some(meerkat_core::handles::AuthLeasePhase::ReauthRequired)
            );
            assert!(
                account.recorded_outage().is_none(),
                "the refusal ends the recorded outage"
            );

            let events = first.turn().await;
            let [Err(error)] = events.as_slice() else {
                panic!("the turn fails once: {events:?}");
            };
            assert!(
                matches!(
                    error,
                    meerkat_llm_core::LlmError::AuthenticationFailed { .. }
                ) && !retried(error),
                "a turn after the refusal must fail as an authentication failure, not as the earlier outage: {error:?}"
            );
            assert_eq!(transport.request_count(), 2);
        }

        #[tokio::test]
        async fn a_non_transient_exchange_failure_after_an_outage_fails_the_build() {
            let clock = TestClock::new();
            let transport = Arc::new(ScriptedTransport::new([
                json_response(503, serde_json::json!({"message": "unavailable"})),
                json_response(404, serde_json::json!({"message": "Not Found"})),
            ]));
            let account = RouteAccount::admitting_pending_limits(transport.clone(), &clock).await;
            let _first = account.session([]).await;
            assert!(account.recorded_outage().is_some());

            let Err(error) = account.try_session([]).await else {
                panic!("an exchange failure that is not transient must fail the build");
            };
            assert!(
                matches!(
                    &error,
                    ProviderAuthError::SourceResolutionFailed(message)
                        if message.contains("returned 404")
                ),
                "{error:?}"
            );
            assert!(
                account.recorded_outage().is_none(),
                "a failure that is not transient ends the recorded outage"
            );
            assert_eq!(transport.request_count(), 2);
        }

        #[tokio::test]
        async fn concurrent_builds_share_one_failed_exchange_and_both_build() {
            let clock = TestClock::new();
            let transport = Arc::new(HeldTransport {
                inner: ScriptedTransport::new([json_response(
                    503,
                    serde_json::json!({"message": "unavailable"}),
                )]),
                // The first build's token exchange.
                hold_at: 0,
                calls: AtomicU64::new(0),
                held: tokio::sync::Notify::new(),
                release: tokio::sync::Notify::new(),
            });
            let account = RouteAccount::admitting_pending_limits(transport.clone(), &clock).await;

            let second_build = async {
                transport.held.notified().await;
                // The first build's exchange is in flight: the second build
                // waits for it instead of starting its own.
                let mut build = std::pin::pin!(account.try_session([]));
                assert!(futures::poll!(build.as_mut()).is_pending());
                transport.release.notify_one();
                build.await
            };
            let (first, second) = tokio::join!(account.try_session([]), second_build);

            assert!(first.is_ok(), "{:?}", first.as_ref().err());
            assert!(
                second.is_ok(),
                "a build that shares the failed exchange must build too: {:?}",
                second.as_ref().err()
            );
            assert_eq!(
                transport.inner.request_count(),
                1,
                "both builds share one exchange"
            );
        }

        #[tokio::test]
        async fn an_outage_after_a_new_sign_in_publishes_the_new_sign_ins_expiry() {
            let clock = TestClock::new();
            let transport = Arc::new(ScriptedTransport::new([
                token_at("derived-one", clock.at(3600), 200),
                models_response(),
                json_response(503, serde_json::json!({"message": "unavailable"})),
                json_response(503, serde_json::json!({"message": "unavailable"})),
            ]));
            let account = RouteAccount::admitting_pending_limits(transport.clone(), &clock).await;
            let first = account.session([]).await;

            // A new sign-in while the exchange is down: the account's token
            // was minted from the earlier one.
            let new_expiry =
                DateTime::from_timestamp(clock.at(86_400).timestamp(), 0).expect("a whole second");
            account.sign_in_again("github-new-token", new_expiry).await;
            let resolved = account
                .runtime
                .resolve(&account.binding, &account.env)
                .await
                .expect("a build that knows its output budget is admitted during the outage");
            assert_eq!(
                resolved.authorizer().expires_at(),
                Some(new_expiry),
                "the build must publish the expiry of the sign-in it resolved, not the earlier one's"
            );
            assert_eq!(transport.request_count(), 3);

            // The earlier sign-in's token serves none of the new sign-in's
            // requests.
            let events = first.turn().await;
            let [Err(error)] = events.as_slice() else {
                panic!("the turn fails once: {events:?}");
            };
            assert!(retried(error), "{error:?}");
            assert!(first.bearers().is_empty(), "{:?}", first.bearers());
            assert_eq!(transport.request_count(), 4);
        }

        #[tokio::test]
        async fn a_session_restored_after_expiry_recovers_once_the_exchange_answers() {
            let clock = TestClock::new();
            let transport = Arc::new(ScriptedTransport::new([
                token_at("derived-one", clock.at(3600), 200),
                models_response(),
                json_response(503, serde_json::json!({"message": "unavailable"})),
                json_response(503, serde_json::json!({"message": "unavailable"})),
                token_at("derived-two", clock.at(3700 + 3600), 200),
                models_response(),
            ]));
            let account = RouteAccount::admitting_pending_limits(transport.clone(), &clock).await;
            let _first = account.session([]).await;

            // The account's token expired while the session was dormant, and
            // the exchange is down when it is restored: it builds on the
            // account's last known route.
            clock.set(3700);
            let restored = account.try_session([]).await;
            assert!(restored.is_ok(), "{:?}", restored.as_ref().err());
            let restored = restored.unwrap();

            // While the outage lasts, a turn fails with a retryable error.
            let events = restored.turn().await;
            let [Err(error)] = events.as_slice() else {
                panic!("the turn fails once: {events:?}");
            };
            assert!(retried(error), "{error:?}");

            // Once the exchange answers, the turn mints a token and succeeds
            // on the same route.
            let events = restored.turn().await;
            assert!(events.is_empty(), "{events:?}");
            assert_eq!(restored.bearers(), ["Bearer derived-two"]);
            assert_eq!(transport.request_count(), 6);
        }

        #[tokio::test]
        async fn a_shared_failed_exchange_admits_only_the_build_that_admits_pending_limits() {
            let clock = TestClock::new();
            let transport = Arc::new(HeldTransport {
                inner: ScriptedTransport::new([json_response(
                    503,
                    serde_json::json!({"message": "unavailable"}),
                )]),
                // The strict build's token exchange.
                hold_at: 0,
                calls: AtomicU64::new(0),
                held: tokio::sync::Notify::new(),
                release: tokio::sync::Notify::new(),
            });
            let strict = RouteAccount::new(transport.clone(), &clock).await;
            let admitting = strict.admitting();

            let admitted_build = async {
                transport.held.notified().await;
                // The strict build's exchange is in flight: this build waits
                // for it and receives its verdict.
                let mut build = std::pin::pin!(admitting.try_session([]));
                assert!(futures::poll!(build.as_mut()).is_pending());
                transport.release.notify_one();
                build.await
            };
            let (strict_build, admitted_build) =
                tokio::join!(strict.try_session([]), admitted_build);

            let Err(error) = strict_build else {
                panic!("a build that does not admit pending limits must fail during the outage");
            };
            assert!(
                matches!(
                    &error,
                    ProviderAuthError::SourceResolutionFailed(message)
                        if message.contains("returned 503")
                ),
                "{error:?}"
            );
            assert!(
                admitted_build.is_ok(),
                "a build that admits pending limits builds on the same verdict: {:?}",
                admitted_build.as_ref().err()
            );
            assert_eq!(
                transport.inner.request_count(),
                1,
                "both builds share one exchange"
            );
        }
    }
}

#[cfg(test)]
#[test]
fn ce_stale_refresh_remains_stale_credential() {
    assert!(matches!(
        auth_error_from_refresh(RefreshError::StalePreparation),
        AuthError::StaleCredential
    ));
}
