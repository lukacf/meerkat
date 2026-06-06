//! Runtime-discovered OAuth for HTTP MCP servers.
//!
//! The MCP config owns only server intent. This module owns OAuth discovery,
//! dynamic client registration, PKCE loopback flow, token persistence, and
//! refresh for MCP resources.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::auth_oauth::{
    OAuthEndpoints, OAuthError, OAuthTokenRequestFormat, OAuthTokenResult, PkcePair,
    bind_loopback_callback, exchange_authorization_code_with_state, exchange_refresh_token,
    oauth_refresh_observation,
};
use crate::auth_store::{PersistedAuthMode, PersistedTokens, TokenKey, TokenStore};
use meerkat_core::connection::{BindingId, RealmId};
use meerkat_core::handles::{
    AUTH_LEASE_TTL_REFRESH_WINDOW_SECS, CredentialUseDisposition, GeneratedAuthLeaseHandle,
    LeaseKey, OAuthLoginCredentialFacts,
};

const MCP_TOKEN_REALM: &str = "mcp-oauth";
const CALLBACK_PATH: &str = "/mcp/oauth/callback";
const CLIENT_NAME: &str = "Meerkat rkat";
pub const MCP_INTERACTIVE_LOGIN_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpAuthMode {
    Stored,
    Interactive,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct McpAuthTarget {
    pub server_name: String,
    pub server_url: String,
}

impl McpAuthTarget {
    pub fn new(server_name: impl Into<String>, server_url: impl Into<String>) -> Self {
        Self {
            server_name: server_name.into(),
            server_url: server_url.into(),
        }
    }

    pub fn token_key(&self) -> Result<TokenKey, McpOAuthError> {
        let mut hasher = Sha256::new();
        hasher.update(self.server_name.as_bytes());
        hasher.update(b"\0");
        hasher.update(self.server_url.as_bytes());
        let digest = URL_SAFE_NO_PAD.encode(hasher.finalize());
        let name_slug = slug_component(&self.server_name);
        TokenKey::parse(MCP_TOKEN_REALM, format!("{name_slug}-{}", &digest[..16])).map_err(
            |error| McpOAuthError::TokenKey {
                server_name: self.server_name.clone(),
                reason: error.to_string(),
            },
        )
    }

    /// The per-binding `AuthMachine` lease key for this MCP server, structurally
    /// identical to [`token_key`](Self::token_key) (realm `mcp-oauth` + the
    /// `<slug>-<digest>` binding slug). The credential-freshness / reauth
    /// decision for MCP-OAuth bearer tokens is owned by the `AuthMachine` keyed
    /// on this lease — the authority never re-derives expiry policy.
    pub fn lease_key(&self) -> Result<LeaseKey, McpOAuthError> {
        let mut hasher = Sha256::new();
        hasher.update(self.server_name.as_bytes());
        hasher.update(b"\0");
        hasher.update(self.server_url.as_bytes());
        let digest = URL_SAFE_NO_PAD.encode(hasher.finalize());
        let name_slug = slug_component(&self.server_name);
        let to_err = |error: meerkat_core::connection::IdentityError| McpOAuthError::TokenKey {
            server_name: self.server_name.clone(),
            reason: error.to_string(),
        };
        let realm = RealmId::parse(MCP_TOKEN_REALM).map_err(to_err)?;
        let binding = BindingId::parse(format!("{name_slug}-{}", &digest[..16])).map_err(to_err)?;
        Ok(LeaseKey::new(realm, binding, None))
    }
}

fn slug_component(raw: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in raw.chars() {
        let next = if ch.is_ascii_alphanumeric() || ch == '_' || ch == '.' {
            last_dash = false;
            Some(ch.to_ascii_lowercase())
        } else if !last_dash {
            last_dash = true;
            Some('-')
        } else {
            None
        };
        if let Some(ch) = next {
            out.push(ch);
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "server".to_string()
    } else {
        trimmed
    }
}

#[async_trait]
pub trait BrowserOpener: Send + Sync {
    async fn open(&self, url: &str) -> Result<(), McpOAuthError>;
}

pub struct SystemBrowserOpener;

#[async_trait]
impl BrowserOpener for SystemBrowserOpener {
    async fn open(&self, url: &str) -> Result<(), McpOAuthError> {
        webbrowser::open(url).map_err(|error| McpOAuthError::Browser(error.to_string()))?;
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum McpOAuthError {
    #[error("MCP OAuth token not found for '{server_name}'. Run: rkat mcp login {server_name}")]
    MissingStoredToken { server_name: String },
    #[error("MCP OAuth interactive login requires a TTY")]
    InteractiveRequiresTty,
    #[error("MCP OAuth discovery failed for '{server_name}': {reason}")]
    DiscoveryFailed { server_name: String, reason: String },
    #[error("MCP OAuth dynamic client registration failed for '{server_name}': {reason}")]
    RegistrationFailed { server_name: String, reason: String },
    #[error("MCP OAuth browser open failed: {0}")]
    Browser(String),
    #[error("MCP OAuth token exchange failed for '{server_name}': {reason}")]
    TokenExchangeFailed { server_name: String, reason: String },
    #[error("MCP OAuth token refresh failed for '{server_name}': {reason}")]
    RefreshFailed { server_name: String, reason: String },
    #[error("MCP OAuth token store error: {0}")]
    TokenStore(String),
    #[error("MCP OAuth token key error for '{server_name}': {reason}")]
    TokenKey { server_name: String, reason: String },
    #[error("MCP OAuth metadata missing from stored token for '{server_name}'")]
    MissingStoredMetadata { server_name: String },
    #[error("MCP OAuth stored credentials for '{server_name}' require reauth")]
    ReauthRequired { server_name: String },
    #[error("MCP OAuth credential lifecycle error for '{server_name}': {reason}")]
    AuthLifecycle { server_name: String, reason: String },
}

#[derive(Clone)]
pub struct McpOAuthAuthority {
    http: Client,
    token_store: Arc<dyn TokenStore>,
    browser: Arc<dyn BrowserOpener>,
    login_timeout: Duration,
    /// Generated `AuthMachine` lease handle that owns the credential
    /// freshness/refresh/reauth decision for the `mcp-oauth` realm. Injected by
    /// the surface that owns the runtime (the CLI) — `meerkat-auth-core` sits
    /// below `meerkat-runtime` in the dep graph and cannot mint a certified
    /// handle itself.
    auth_lease: GeneratedAuthLeaseHandle,
}

impl McpOAuthAuthority {
    pub fn new(
        token_store: Arc<dyn TokenStore>,
        browser: Arc<dyn BrowserOpener>,
        auth_lease: GeneratedAuthLeaseHandle,
    ) -> Self {
        Self {
            http: Client::new(),
            token_store,
            browser,
            login_timeout: MCP_INTERACTIVE_LOGIN_TIMEOUT,
            auth_lease,
        }
    }

    pub fn with_http(
        token_store: Arc<dyn TokenStore>,
        browser: Arc<dyn BrowserOpener>,
        http: Client,
        auth_lease: GeneratedAuthLeaseHandle,
    ) -> Self {
        Self {
            http,
            token_store,
            browser,
            login_timeout: MCP_INTERACTIVE_LOGIN_TIMEOUT,
            auth_lease,
        }
    }

    #[cfg(test)]
    fn with_http_and_login_timeout(
        token_store: Arc<dyn TokenStore>,
        browser: Arc<dyn BrowserOpener>,
        http: Client,
        login_timeout: Duration,
        auth_lease: GeneratedAuthLeaseHandle,
    ) -> Self {
        Self {
            http,
            token_store,
            browser,
            login_timeout,
            auth_lease,
        }
    }

    pub async fn stored_bearer_token(
        &self,
        target: &McpAuthTarget,
    ) -> Result<Option<String>, McpOAuthError> {
        let key = target.token_key()?;
        let Some(tokens) = self
            .token_store
            .load(&key)
            .await
            .map_err(|error| McpOAuthError::TokenStore(error.to_string()))?
        else {
            return Ok(None);
        };
        if tokens.auth_mode != PersistedAuthMode::McpOauth {
            return Ok(None);
        }
        let metadata = stored_metadata_for_target(target, &tokens)?;
        let tokens = self
            .refresh_if_needed(target, &key, tokens, metadata)
            .await?;
        Ok(tokens.primary_secret)
    }

    pub async fn require_stored_bearer_token(
        &self,
        target: &McpAuthTarget,
    ) -> Result<String, McpOAuthError> {
        self.stored_bearer_token(target)
            .await?
            .ok_or_else(|| McpOAuthError::MissingStoredToken {
                server_name: target.server_name.clone(),
            })
    }

    pub async fn interactive_login(
        &self,
        target: &McpAuthTarget,
        www_authenticate: Option<&str>,
    ) -> Result<String, McpOAuthError> {
        let binding = bind_loopback_callback(CALLBACK_PATH)
            .await
            .map_err(|error| McpOAuthError::DiscoveryFailed {
                server_name: target.server_name.clone(),
                reason: error.to_string(),
            })?;
        let redirect_uri = binding.redirect_url.clone();
        let discovery = self
            .discover(target, www_authenticate, &redirect_uri)
            .await?;
        let client = self
            .register_client(target, &discovery, &redirect_uri)
            .await?;
        let pkce = PkcePair::generate_s256();
        let state = URL_SAFE_NO_PAD.encode(uuid::Uuid::new_v4().as_bytes());
        let endpoints = OAuthEndpoints {
            client_id: client.client_id.clone(),
            authorize_url: discovery.authorization_endpoint.clone(),
            token_url: discovery.token_endpoint.clone(),
            device_code_url: None,
            redirect_uri: redirect_uri.clone(),
            scopes: discovery.scopes.clone(),
            extra_authorize_params: vec![("resource".to_string(), discovery.resource.clone())],
            token_request_format: OAuthTokenRequestFormat::FormUrlEncoded,
            include_state_in_token_exchange: false,
            extra_token_params: vec![("resource".to_string(), discovery.resource.clone())],
            refresh_scopes: discovery.scopes.clone(),
            extra_headers: Vec::new(),
        };
        let authorize_url = endpoints.authorize_url_with_pkce(&pkce.challenge, &state);
        let callback = binding.expect_state(state.clone());
        self.browser.open(&authorize_url).await?;
        let outcome = callback
            .wait(self.login_timeout)
            .await
            .map_err(|error| map_oauth_exchange_error(target, error))?;
        let token = exchange_authorization_code_with_state(
            &self.http,
            &endpoints,
            &outcome.code,
            pkce.verifier.secret(),
            client.client_secret.as_deref(),
            Some(&outcome.state),
        )
        .await
        .map_err(|error| map_oauth_exchange_error(target, error))?;
        let persisted =
            persisted_tokens_from_result(&token, &discovery, &client, target, Utc::now())?;
        let key = target.token_key()?;
        self.token_store
            .save(&key, &persisted)
            .await
            .map_err(|error| McpOAuthError::TokenStore(error.to_string()))?;
        Ok(token.access_token)
    }

    async fn refresh_if_needed(
        &self,
        target: &McpAuthTarget,
        key: &TokenKey,
        tokens: PersistedTokens,
        metadata: StoredMcpOAuthMetadata,
    ) -> Result<PersistedTokens, McpOAuthError> {
        let Some(expires_at) = tokens.expires_at else {
            return Ok(tokens);
        };

        // Route the cached-vs-refresh-vs-reauth decision through the injected
        // per-binding `AuthMachine` lease for the `mcp-oauth` realm. The machine
        // owns the freshness policy (observed expiry vs the canonical refresh
        // window); this authority only feeds it the expiry and the pure
        // OAuth-login facts, then mirrors the verdict. No local skew comparison.
        let auth_lease = &self.auth_lease;
        let lease_key = target.lease_key()?;
        let now_secs = epoch_secs(Utc::now());
        let lifecycle_err =
            |error: meerkat_core::handles::DslTransitionError| McpOAuthError::AuthLifecycle {
                server_name: target.server_name.clone(),
                reason: error.to_string(),
            };
        // The handle is shared across calls and servers; reset this key's prior
        // lease state, then record the current credential's expiry so the
        // freshness verdict reflects THIS token. Release is best-effort: an
        // untracked key has nothing to clear.
        let _ = auth_lease.release_lease(&lease_key);
        auth_lease
            .acquire_lease(&lease_key, epoch_secs(expires_at))
            .map_err(lifecycle_err)?;
        auth_lease
            .observe_credential_freshness(&lease_key, now_secs, AUTH_LEASE_TTL_REFRESH_WINDOW_SECS)
            .map_err(lifecycle_err)?;
        let facts = OAuthLoginCredentialFacts {
            credential_present: tokens.primary_secret.is_some(),
            force_refresh: false,
            refresh_allowed: tokens.refresh_token.is_some(),
        };
        let disposition = auth_lease
            .resolve_oauth_login_credential_disposition(&lease_key, facts)
            .map_err(lifecycle_err)?;
        match disposition {
            // The machine authorizes the cached credential as fresh.
            CredentialUseDisposition::Authorized => Ok(tokens),
            // The machine asks for a refresh: run the OAuth refresh exchange.
            CredentialUseDisposition::RefreshRequired => {
                let Some(refresh_token) = tokens.refresh_token.clone() else {
                    return Err(McpOAuthError::ReauthRequired {
                        server_name: target.server_name.clone(),
                    });
                };
                let endpoints = OAuthEndpoints {
                    client_id: metadata.client.client_id.clone(),
                    authorize_url: metadata.discovery.authorization_endpoint.clone(),
                    token_url: metadata.discovery.token_endpoint.clone(),
                    device_code_url: None,
                    redirect_uri: metadata.client.redirect_uri.clone(),
                    scopes: metadata.discovery.scopes.clone(),
                    extra_authorize_params: Vec::new(),
                    token_request_format: OAuthTokenRequestFormat::FormUrlEncoded,
                    include_state_in_token_exchange: false,
                    extra_token_params: vec![(
                        "resource".to_string(),
                        metadata.discovery.resource.clone(),
                    )],
                    refresh_scopes: metadata.discovery.scopes.clone(),
                    extra_headers: Vec::new(),
                };
                let refreshed = exchange_refresh_token(
                    &self.http,
                    &endpoints,
                    &refresh_token,
                    metadata.client.client_secret.as_deref(),
                )
                .await
                .map_err(|error| map_refresh_error(target, error))?;
                let mut persisted = persisted_tokens_from_result(
                    &refreshed,
                    &metadata.discovery,
                    &metadata.client,
                    target,
                    Utc::now(),
                )?;
                if persisted.refresh_token.is_none() {
                    persisted.refresh_token = Some(refresh_token);
                }
                self.token_store
                    .save(key, &persisted)
                    .await
                    .map_err(|error| McpOAuthError::TokenStore(error.to_string()))?;
                Ok(persisted)
            }
            // Refresh is not permitted or the credential is otherwise unusable:
            // the user must reauthenticate. (`LeaseAbsent`/`AlreadyRefreshing`
            // cannot arise for a freshly acquired single-shot lease, but are
            // handled exhaustively and fail closed to reauth.)
            CredentialUseDisposition::RefreshDisallowed
            | CredentialUseDisposition::ReauthRequired
            | CredentialUseDisposition::LeaseAbsent
            | CredentialUseDisposition::AlreadyRefreshing => Err(McpOAuthError::ReauthRequired {
                server_name: target.server_name.clone(),
            }),
        }
    }

    async fn discover(
        &self,
        target: &McpAuthTarget,
        www_authenticate: Option<&str>,
        _redirect_uri: &str,
    ) -> Result<StoredMcpOAuthDiscovery, McpOAuthError> {
        require_https_or_loopback(target, &target.server_url, "MCP protected resource")?;
        let resource_metadata_url = match www_authenticate.and_then(resource_metadata_from_header) {
            Some(value) => absolutize_url(&target.server_url, &value).map_err(|error| {
                McpOAuthError::DiscoveryFailed {
                    server_name: target.server_name.clone(),
                    reason: error,
                }
            })?,
            None => {
                self.discover_resource_metadata_url_by_well_known(target)
                    .await?
            }
        };
        let resource: ProtectedResourceMetadata = self
            .http
            .get(resource_metadata_url.clone())
            .send()
            .await
            .map_err(|error| McpOAuthError::DiscoveryFailed {
                server_name: target.server_name.clone(),
                reason: error.to_string(),
            })?
            .error_for_status()
            .map_err(|error| McpOAuthError::DiscoveryFailed {
                server_name: target.server_name.clone(),
                reason: error.to_string(),
            })?
            .json()
            .await
            .map_err(|error| McpOAuthError::DiscoveryFailed {
                server_name: target.server_name.clone(),
                reason: format!("decode protected resource metadata: {error}"),
            })?;
        if resource.resource != target.server_url {
            return Err(McpOAuthError::DiscoveryFailed {
                server_name: target.server_name.clone(),
                reason: format!(
                    "protected resource metadata resource '{}' does not match MCP server '{}'",
                    resource.resource, target.server_url
                ),
            });
        }
        let auth_server = resource
            .authorization_servers
            .first()
            .cloned()
            .ok_or_else(|| McpOAuthError::DiscoveryFailed {
                server_name: target.server_name.clone(),
                reason: "protected resource metadata has no authorization_servers".to_string(),
            })?;
        require_https_or_loopback(target, &auth_server, "authorization server issuer")?;
        let authorization_metadata_url = authorization_server_metadata_url(&auth_server)?;
        let auth: AuthorizationServerMetadata = self
            .http
            .get(&authorization_metadata_url)
            .send()
            .await
            .map_err(|error| McpOAuthError::DiscoveryFailed {
                server_name: target.server_name.clone(),
                reason: error.to_string(),
            })?
            .error_for_status()
            .map_err(|error| McpOAuthError::DiscoveryFailed {
                server_name: target.server_name.clone(),
                reason: error.to_string(),
            })?
            .json()
            .await
            .map_err(|error| McpOAuthError::DiscoveryFailed {
                server_name: target.server_name.clone(),
                reason: format!("decode authorization server metadata: {error}"),
            })?;
        if auth.issuer != auth_server {
            return Err(McpOAuthError::DiscoveryFailed {
                server_name: target.server_name.clone(),
                reason: format!(
                    "authorization server metadata issuer '{}' does not match discovered issuer '{}'",
                    auth.issuer, auth_server
                ),
            });
        }
        let registration_endpoint =
            auth.registration_endpoint
                .clone()
                .ok_or_else(|| McpOAuthError::DiscoveryFailed {
                    server_name: target.server_name.clone(),
                    reason: "authorization server metadata has no registration_endpoint"
                        .to_string(),
                })?;
        let authorization_endpoint =
            absolutize_url(&authorization_metadata_url, &auth.authorization_endpoint).map_err(
                |reason| McpOAuthError::DiscoveryFailed {
                    server_name: target.server_name.clone(),
                    reason,
                },
            )?;
        let token_endpoint = absolutize_url(&authorization_metadata_url, &auth.token_endpoint)
            .map_err(|reason| McpOAuthError::DiscoveryFailed {
                server_name: target.server_name.clone(),
                reason,
            })?;
        let registration_endpoint =
            absolutize_url(&authorization_metadata_url, &registration_endpoint).map_err(
                |reason| McpOAuthError::DiscoveryFailed {
                    server_name: target.server_name.clone(),
                    reason,
                },
            )?;
        require_https_or_loopback(target, &authorization_endpoint, "authorization endpoint")?;
        require_https_or_loopback(target, &token_endpoint, "token endpoint")?;
        require_https_or_loopback(target, &registration_endpoint, "registration endpoint")?;
        Ok(StoredMcpOAuthDiscovery {
            resource: resource.resource,
            resource_metadata_url,
            authorization_server: auth_server,
            authorization_metadata_url,
            authorization_endpoint,
            token_endpoint,
            registration_endpoint,
            scopes: Vec::new(),
        })
    }

    async fn discover_resource_metadata_url_by_well_known(
        &self,
        target: &McpAuthTarget,
    ) -> Result<String, McpOAuthError> {
        for candidate in protected_resource_well_known_candidates(&target.server_url)? {
            let response = self.http.get(&candidate).send().await;
            if let Ok(response) = response
                && response.status().is_success()
            {
                return Ok(candidate);
            }
        }
        Err(McpOAuthError::DiscoveryFailed {
            server_name: target.server_name.clone(),
            reason: "no oauth-protected-resource metadata found".to_string(),
        })
    }

    async fn register_client(
        &self,
        target: &McpAuthTarget,
        discovery: &StoredMcpOAuthDiscovery,
        redirect_uri: &str,
    ) -> Result<StoredMcpOAuthClient, McpOAuthError> {
        let body = serde_json::json!({
            "client_name": CLIENT_NAME,
            "redirect_uris": [redirect_uri],
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none",
        });
        let wire: DynamicClientRegistrationResponse = self
            .http
            .post(&discovery.registration_endpoint)
            .json(&body)
            .send()
            .await
            .map_err(|error| McpOAuthError::RegistrationFailed {
                server_name: target.server_name.clone(),
                reason: error.to_string(),
            })?
            .error_for_status()
            .map_err(|error| McpOAuthError::RegistrationFailed {
                server_name: target.server_name.clone(),
                reason: error.to_string(),
            })?
            .json()
            .await
            .map_err(|error| McpOAuthError::RegistrationFailed {
                server_name: target.server_name.clone(),
                reason: format!("decode: {error}"),
            })?;
        let token_endpoint_auth_method =
            wire.token_endpoint_auth_method
                .ok_or_else(|| McpOAuthError::RegistrationFailed {
                    server_name: target.server_name.clone(),
                    reason:
                        "token_endpoint_auth_method missing; public PKCE clients require explicit 'none'"
                            .to_string(),
                })?;
        if token_endpoint_auth_method != "none" {
            return Err(McpOAuthError::RegistrationFailed {
                server_name: target.server_name.clone(),
                reason: format!(
                    "unsupported token_endpoint_auth_method '{token_endpoint_auth_method}'"
                ),
            });
        }
        Ok(StoredMcpOAuthClient {
            client_id: wire.client_id,
            client_secret: None,
            token_endpoint_auth_method,
            redirect_uri: redirect_uri.to_string(),
        })
    }
}

fn stored_metadata_for_target(
    target: &McpAuthTarget,
    tokens: &PersistedTokens,
) -> Result<StoredMcpOAuthMetadata, McpOAuthError> {
    let metadata: StoredMcpOAuthMetadata = serde_json::from_value(tokens.metadata.clone())
        .map_err(|_| McpOAuthError::MissingStoredMetadata {
            server_name: target.server_name.clone(),
        })?;
    if metadata.server_name != target.server_name || metadata.server_url != target.server_url {
        return Err(McpOAuthError::ReauthRequired {
            server_name: target.server_name.clone(),
        });
    }
    Ok(metadata)
}

fn persisted_tokens_from_result(
    result: &OAuthTokenResult,
    discovery: &StoredMcpOAuthDiscovery,
    client: &StoredMcpOAuthClient,
    target: &McpAuthTarget,
    now: DateTime<Utc>,
) -> Result<PersistedTokens, McpOAuthError> {
    let expires_at =
        result
            .expires_at_from(now)
            .map_err(|error| McpOAuthError::TokenExchangeFailed {
                server_name: target.server_name.clone(),
                reason: error.to_string(),
            })?;
    let scopes = result
        .scope
        .as_deref()
        .map(|scope| scope.split_whitespace().map(str::to_string).collect())
        .unwrap_or_else(|| discovery.scopes.clone());
    let metadata = StoredMcpOAuthMetadata {
        server_name: target.server_name.clone(),
        server_url: target.server_url.clone(),
        discovery: discovery.clone(),
        client: client.clone(),
    };
    Ok(PersistedTokens {
        auth_mode: PersistedAuthMode::McpOauth,
        primary_secret: Some(result.access_token.clone()),
        refresh_token: result.refresh_token.clone(),
        id_token: result.id_token.clone(),
        expires_at,
        last_refresh: Some(now),
        scopes,
        account_id: Some(target.server_name.clone()),
        metadata: serde_json::to_value(metadata).map_err(|error| {
            McpOAuthError::TokenExchangeFailed {
                server_name: target.server_name.clone(),
                reason: format!("failed to serialize token metadata: {error}"),
            }
        })?,
    })
}

fn map_oauth_exchange_error(target: &McpAuthTarget, error: OAuthError) -> McpOAuthError {
    McpOAuthError::TokenExchangeFailed {
        server_name: target.server_name.clone(),
        reason: error.to_string(),
    }
}

fn map_refresh_error(target: &McpAuthTarget, error: OAuthError) -> McpOAuthError {
    // Route the OAuth error through the canonical boundary-observation
    // classifier the AuthMachine + lease lifecycle already consume, then ask
    // the typed observation whether reauth is required. One owner of the
    // reauth-vs-retry decision instead of a substring match on the raw body.
    if oauth_refresh_observation(&error).requires_reauth() {
        return McpOAuthError::ReauthRequired {
            server_name: target.server_name.clone(),
        };
    }
    McpOAuthError::RefreshFailed {
        server_name: target.server_name.clone(),
        reason: error.to_string(),
    }
}

/// Whole seconds since the Unix epoch, clamped to a non-negative `u64` for the
/// `AuthMachine` lease lifecycle inputs.
fn epoch_secs(time: DateTime<Utc>) -> u64 {
    time.timestamp().max(0) as u64
}

fn resource_metadata_from_header(header: &str) -> Option<String> {
    auth_param(header, "resource_metadata")
}

fn auth_param(header: &str, key: &str) -> Option<String> {
    for segment in header.split(',') {
        let segment = segment.trim();
        let Some((name, raw_value)) = segment.split_once('=') else {
            continue;
        };
        let name = name.split_whitespace().last().unwrap_or(name).trim();
        if name != key {
            continue;
        }
        return unquote_auth_value(raw_value.trim());
    }
    None
}

fn unquote_auth_value(raw: &str) -> Option<String> {
    if let Some(body) = raw.strip_prefix('"') {
        let mut out = String::new();
        let mut escaped = false;
        for ch in body.chars() {
            if escaped {
                out.push(ch);
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                return Some(out);
            } else {
                out.push(ch);
            }
        }
        None
    } else {
        Some(raw.trim_end_matches(';').to_string())
    }
}

fn absolutize_url(base: &str, value: &str) -> Result<String, String> {
    let base = reqwest::Url::parse(base).map_err(|error| error.to_string())?;
    base.join(value)
        .map(|url| url.to_string())
        .map_err(|error| error.to_string())
}

fn protected_resource_well_known_candidates(
    server_url: &str,
) -> Result<Vec<String>, McpOAuthError> {
    let url = reqwest::Url::parse(server_url).map_err(|error| McpOAuthError::DiscoveryFailed {
        server_name: "unknown".to_string(),
        reason: error.to_string(),
    })?;
    let origin = url.origin().ascii_serialization();
    let path = url.path().trim_matches('/');
    let mut candidates = Vec::new();
    if !path.is_empty() {
        candidates.push(format!(
            "{origin}/.well-known/oauth-protected-resource/{path}"
        ));
        candidates.push(format!(
            "{origin}/{path}/.well-known/oauth-protected-resource"
        ));
    }
    candidates.push(format!("{origin}/.well-known/oauth-protected-resource"));
    Ok(candidates)
}

fn authorization_server_metadata_url(server: &str) -> Result<String, McpOAuthError> {
    let url = reqwest::Url::parse(server).map_err(|error| McpOAuthError::DiscoveryFailed {
        server_name: "unknown".to_string(),
        reason: error.to_string(),
    })?;
    let origin = url.origin().ascii_serialization();
    let path = url.path().trim_matches('/');
    if path.is_empty() {
        Ok(format!("{origin}/.well-known/oauth-authorization-server"))
    } else {
        Ok(format!(
            "{origin}/.well-known/oauth-authorization-server/{path}"
        ))
    }
}

fn require_https_or_loopback(
    target: &McpAuthTarget,
    value: &str,
    label: &str,
) -> Result<(), McpOAuthError> {
    let url = reqwest::Url::parse(value).map_err(|error| McpOAuthError::DiscoveryFailed {
        server_name: target.server_name.clone(),
        reason: format!("invalid {label}: {error}"),
    })?;
    if url.scheme() == "https" || is_loopback_url(&url) {
        return Ok(());
    }
    Err(McpOAuthError::DiscoveryFailed {
        server_name: target.server_name.clone(),
        reason: format!("{label} must use https"),
    })
}

fn is_loopback_url(url: &reqwest::Url) -> bool {
    matches!(
        url.host_str(),
        Some("localhost" | "127.0.0.1" | "::1" | "[::1]")
    )
}

#[derive(Debug, Deserialize)]
struct ProtectedResourceMetadata {
    resource: String,
    authorization_servers: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct AuthorizationServerMetadata {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    #[serde(default)]
    registration_endpoint: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DynamicClientRegistrationResponse {
    client_id: String,
    #[serde(default)]
    token_endpoint_auth_method: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredMcpOAuthDiscovery {
    resource: String,
    resource_metadata_url: String,
    authorization_server: String,
    authorization_metadata_url: String,
    authorization_endpoint: String,
    token_endpoint: String,
    registration_endpoint: String,
    scopes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredMcpOAuthClient {
    client_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    client_secret: Option<String>,
    token_endpoint_auth_method: String,
    redirect_uri: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredMcpOAuthMetadata {
    server_name: String,
    server_url: String,
    discovery: StoredMcpOAuthDiscovery,
    client: StoredMcpOAuthClient,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::EphemeralTokenStore;
    use axum::extract::{Query, State};
    use axum::http::{HeaderMap, StatusCode};
    use axum::response::{IntoResponse, Redirect};
    use axum::routing::{get, post};
    use axum::{Form, Json, Router};
    use parking_lot::Mutex;
    use serde_json::Value;
    use std::collections::HashMap;
    use tokio::net::TcpListener;

    /// A certified `AuthMachine` lease handle for tests. `meerkat-runtime` is a
    /// dev-dependency, so tests can mint the same generated lease the CLI
    /// injects in production (the lib cannot, to avoid a dep cycle).
    fn test_auth_lease() -> GeneratedAuthLeaseHandle {
        let handle = Arc::new(meerkat_runtime::RuntimeAuthLeaseHandle::new());
        meerkat_runtime::protocol_auth_lease_lifecycle_publication::generated_auth_lease_handle(
            handle,
        )
        .expect("test auth lease must be certified by generated AuthMachine authority")
    }

    #[derive(Debug, Clone, Copy, Default)]
    enum AuthorizeOutcome {
        #[default]
        Success,
        StateMismatch,
        Denied,
        NoCallback,
    }

    #[derive(Default)]
    struct TestState {
        opened_url: Mutex<Option<String>>,
        redirect_uri: Mutex<Option<String>>,
        registration_requests: Mutex<Vec<Value>>,
        token_requests: Mutex<Vec<Value>>,
        include_registration_endpoint: Mutex<bool>,
        omit_token_endpoint_auth_method: Mutex<bool>,
        resource_override: Mutex<Option<String>>,
        issuer_override: Mutex<Option<String>>,
        authorize_outcome: Mutex<AuthorizeOutcome>,
        token_fails: Mutex<bool>,
    }

    struct RecordingBrowser {
        state: Arc<TestState>,
        http: Client,
    }

    #[async_trait]
    impl BrowserOpener for RecordingBrowser {
        async fn open(&self, url: &str) -> Result<(), McpOAuthError> {
            *self.state.opened_url.lock() = Some(url.to_string());
            let _response = self.http.get(url).send().await.unwrap();
            Ok(())
        }
    }

    struct NoCallbackBrowser {
        state: Arc<TestState>,
    }

    #[async_trait]
    impl BrowserOpener for NoCallbackBrowser {
        async fn open(&self, url: &str) -> Result<(), McpOAuthError> {
            *self.state.opened_url.lock() = Some(url.to_string());
            Ok(())
        }
    }

    async fn spawn_oauth_fixture() -> (String, Arc<TestState>) {
        let state = Arc::new(TestState {
            include_registration_endpoint: Mutex::new(true),
            ..TestState::default()
        });
        let app = Router::new()
            .route("/mcp", post(mcp_endpoint))
            .route(
                "/.well-known/oauth-protected-resource/mcp",
                get(protected_resource),
            )
            .route(
                "/.well-known/oauth-authorization-server",
                get(authorization_metadata),
            )
            .route("/register", post(register_client))
            .route("/authorize", get(authorize))
            .route("/token", post(token))
            .with_state(Arc::clone(&state));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{addr}"), state)
    }

    async fn mcp_endpoint(headers: HeaderMap) -> impl IntoResponse {
        if headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            == Some("Bearer access-token")
        {
            return (
                StatusCode::OK,
                Json(serde_json::json!({"jsonrpc":"2.0","id":1,"result":{}})),
            )
                .into_response();
        }
        (
            StatusCode::UNAUTHORIZED,
            [(
                "www-authenticate",
                r#"Bearer error="invalid_request", resource_metadata="/.well-known/oauth-protected-resource/mcp""#,
            )],
            "",
        )
            .into_response()
    }

    async fn protected_resource(
        State(state): State<Arc<TestState>>,
        headers: HeaderMap,
    ) -> impl IntoResponse {
        let host = headers
            .get("host")
            .and_then(|value| value.to_str().ok())
            .unwrap();
        let resource = state
            .resource_override
            .lock()
            .clone()
            .unwrap_or_else(|| format!("http://{host}/mcp"));
        Json(serde_json::json!({
            "resource": resource,
            "authorization_servers": [format!("http://{host}")],
            "scopes_supported": ["mcp.read"]
        }))
    }

    async fn authorization_metadata(
        State(state): State<Arc<TestState>>,
        headers: HeaderMap,
    ) -> impl IntoResponse {
        let host = headers
            .get("host")
            .and_then(|value| value.to_str().ok())
            .unwrap();
        let issuer = state
            .issuer_override
            .lock()
            .clone()
            .unwrap_or_else(|| format!("http://{host}"));
        let mut body = serde_json::json!({
            "issuer": issuer,
            "authorization_endpoint": "/authorize",
            "token_endpoint": "/token",
        });
        if *state.include_registration_endpoint.lock() {
            body["registration_endpoint"] = serde_json::json!("/register");
        }
        Json(body)
    }

    async fn register_client(
        State(state): State<Arc<TestState>>,
        Json(body): Json<Value>,
    ) -> impl IntoResponse {
        let redirect_uri = body["redirect_uris"][0].as_str().unwrap().to_string();
        state.registration_requests.lock().push(body);
        *state.redirect_uri.lock() = Some(redirect_uri);
        Json(serde_json::json!({
            "client_id": "client-123",
            "client_secret": "ignored-secret-for-public-client",
            "token_endpoint_auth_method": if *state.omit_token_endpoint_auth_method.lock() {
                Value::Null
            } else {
                Value::String("none".to_string())
            }
        }))
    }

    async fn authorize(
        State(state): State<Arc<TestState>>,
        Query(params): Query<HashMap<String, String>>,
    ) -> impl IntoResponse {
        let redirect_uri = state.redirect_uri.lock().clone().unwrap();
        let state_param = params.get("state").unwrap();
        match *state.authorize_outcome.lock() {
            AuthorizeOutcome::Success => Redirect::temporary(&format!(
                "{redirect_uri}?code=fixture-code&state={state_param}"
            ))
            .into_response(),
            AuthorizeOutcome::StateMismatch => {
                Redirect::temporary(&format!("{redirect_uri}?code=fixture-code&state=wrong"))
                    .into_response()
            }
            AuthorizeOutcome::Denied => Redirect::temporary(&format!(
                "{redirect_uri}?error=access_denied&state={state_param}"
            ))
            .into_response(),
            AuthorizeOutcome::NoCallback => {
                (StatusCode::OK, "authorization left pending").into_response()
            }
        }
    }

    async fn token(
        State(state): State<Arc<TestState>>,
        Form(body): Form<HashMap<String, String>>,
    ) -> impl IntoResponse {
        if *state.token_fails.lock() {
            // RFC 6749 §5.2 error response: a JSON body carrying the typed
            // `error` code. The refresh-failure classifier parses this `error`
            // field to decide reauth-vs-retry.
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": "invalid_grant" })),
            )
                .into_response();
        }
        state
            .token_requests
            .lock()
            .push(serde_json::to_value(body).unwrap());
        Json(serde_json::json!({
            "access_token": "access-token",
            "refresh_token": "refresh-token",
            "expires_in": 3600,
            "scope": "mcp.read"
        }))
        .into_response()
    }

    #[test]
    fn target_key_is_typed_and_stable() {
        let a = McpAuthTarget::new("glean", "https://king-be.glean.com/mcp/default")
            .token_key()
            .unwrap();
        let b = McpAuthTarget::new("glean", "https://king-be.glean.com/mcp/default")
            .token_key()
            .unwrap();
        let c = McpAuthTarget::new("glean", "https://other.example/mcp/default")
            .token_key()
            .unwrap();
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.realm.as_str(), MCP_TOKEN_REALM);
    }

    #[test]
    fn parses_resource_metadata_from_www_authenticate() {
        let header = r#"Bearer error="invalid_request", resource_metadata="/.well-known/oauth-protected-resource/mcp""#;
        assert_eq!(
            resource_metadata_from_header(header).as_deref(),
            Some("/.well-known/oauth-protected-resource/mcp")
        );
    }

    fn recording_browser(state: Arc<TestState>) -> Arc<dyn BrowserOpener> {
        Arc::new(RecordingBrowser {
            state,
            http: Client::builder()
                .redirect(reqwest::redirect::Policy::limited(10))
                .build()
                .unwrap(),
        })
    }

    async fn assert_login_fails_closed(
        state: Arc<TestState>,
        store: Arc<EphemeralTokenStore>,
        target: &McpAuthTarget,
        result: Result<String, McpOAuthError>,
        expected_reason: &str,
    ) {
        let err = result.expect_err("login should fail");
        assert!(
            err.to_string().contains(expected_reason),
            "expected {expected_reason:?} in error, got {err}"
        );
        assert!(
            store
                .load(&target.token_key().unwrap())
                .await
                .unwrap()
                .is_none(),
            "failed login must not persist MCP OAuth tokens"
        );
        assert!(
            state.token_requests.lock().len() <= 1,
            "failed login should not retry token exchange unexpectedly"
        );
    }

    #[tokio::test]
    async fn interactive_login_discovers_registers_exchanges_and_stores_token() {
        let (base, state) = spawn_oauth_fixture().await;
        let store = Arc::new(EphemeralTokenStore::new());
        let browser = recording_browser(Arc::clone(&state));
        let authority =
            McpOAuthAuthority::with_http(store.clone(), browser, Client::new(), test_auth_lease());
        let target = McpAuthTarget::new("glean", format!("{base}/mcp"));
        let token = authority
            .interactive_login(
                &target,
                Some(r#"Bearer resource_metadata="/.well-known/oauth-protected-resource/mcp""#),
            )
            .await
            .expect("login succeeds");
        assert_eq!(token, "access-token");
        let stored = store
            .load(&target.token_key().unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.auth_mode, PersistedAuthMode::McpOauth);
        assert_eq!(stored.primary_secret.as_deref(), Some("access-token"));
        assert!(stored.metadata["client"]["client_id"] == "client-123");
        assert!(
            stored.metadata["client"].get("client_secret").is_none(),
            "MCP DCR requests a public PKCE client and must not persist a DCR secret"
        );
        assert_eq!(
            stored.metadata["client"]["token_endpoint_auth_method"],
            "none"
        );
        assert_eq!(
            stored.metadata["discovery"]["resource"],
            format!("{base}/mcp")
        );

        let opened_url = state.opened_url.lock().clone().unwrap();
        assert!(
            opened_url.contains("resource="),
            "authorize URL should carry an OAuth resource indicator"
        );
        assert!(
            !opened_url.contains("scope="),
            "MCP OAuth v1 should not request every advertised supported scope"
        );
        let registration = state.registration_requests.lock();
        assert_eq!(
            registration[0]["token_endpoint_auth_method"], "none",
            "DCR should explicitly request a public token endpoint auth method"
        );
        let token_requests = state.token_requests.lock();
        assert_eq!(
            token_requests[0]["resource"],
            format!("{base}/mcp"),
            "authorization-code exchange should carry the MCP resource indicator"
        );
        assert!(
            token_requests[0].get("client_secret").is_none(),
            "public-client token exchange must not send a DCR secret"
        );
        assert!(
            token_requests[0].get("scope").is_none(),
            "authorization-code exchange should not request every advertised supported scope"
        );
    }

    #[tokio::test]
    async fn interactive_login_uses_well_known_fallback_without_challenge_header() {
        let (base, state) = spawn_oauth_fixture().await;
        let store = Arc::new(EphemeralTokenStore::new());
        let browser = recording_browser(Arc::clone(&state));
        let authority =
            McpOAuthAuthority::with_http(store.clone(), browser, Client::new(), test_auth_lease());
        let target = McpAuthTarget::new("glean", format!("{base}/mcp"));

        let token = authority
            .interactive_login(&target, None)
            .await
            .expect("well-known discovery fallback should login");

        assert_eq!(token, "access-token");
        let stored = store
            .load(&target.token_key().unwrap())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            stored.metadata["discovery"]["resource_metadata_url"],
            format!("{base}/.well-known/oauth-protected-resource/mcp")
        );
    }

    #[tokio::test]
    async fn stored_token_refresh_uses_resource_and_invalid_grant_requires_reauth() {
        let (base, state) = spawn_oauth_fixture().await;
        let store = Arc::new(EphemeralTokenStore::new());
        let browser = recording_browser(Arc::clone(&state));
        let authority =
            McpOAuthAuthority::with_http(store.clone(), browser, Client::new(), test_auth_lease());
        let target = McpAuthTarget::new("glean", format!("{base}/mcp"));

        authority
            .interactive_login(&target, None)
            .await
            .expect("initial login succeeds");
        let key = target.token_key().unwrap();
        let mut stored = store.load(&key).await.unwrap().unwrap();
        stored.expires_at = Some(Utc::now() - chrono::Duration::seconds(1));
        store.save(&key, &stored).await.unwrap();
        *state.token_fails.lock() = true;

        let error = authority
            .stored_bearer_token(&target)
            .await
            .expect_err("invalid refresh grant should require reauth");

        assert!(matches!(error, McpOAuthError::ReauthRequired { .. }));
        let token_requests = state.token_requests.lock();
        assert_eq!(
            token_requests.last().unwrap()["resource"],
            format!("{base}/mcp"),
            "refresh exchange should carry the MCP resource indicator"
        );
    }

    #[tokio::test]
    async fn stored_token_metadata_must_match_requested_target() {
        let (base, state) = spawn_oauth_fixture().await;
        let store = Arc::new(EphemeralTokenStore::new());
        let browser = recording_browser(Arc::clone(&state));
        let authority =
            McpOAuthAuthority::with_http(store.clone(), browser, Client::new(), test_auth_lease());
        let original = McpAuthTarget::new("glean", format!("{base}/mcp"));
        let other = McpAuthTarget::new("glean", format!("{base}/other-mcp"));

        authority
            .interactive_login(&original, None)
            .await
            .expect("initial login succeeds");
        let stored = store
            .load(&original.token_key().unwrap())
            .await
            .unwrap()
            .unwrap();
        store
            .save(&other.token_key().unwrap(), &stored)
            .await
            .unwrap();

        let error = authority
            .stored_bearer_token(&other)
            .await
            .expect_err("mismatched stored metadata should not be trusted");

        assert!(matches!(error, McpOAuthError::ReauthRequired { .. }));
    }

    #[tokio::test]
    async fn interactive_login_missing_dcr_fails_closed() {
        let (base, state) = spawn_oauth_fixture().await;
        *state.include_registration_endpoint.lock() = false;
        let store = Arc::new(EphemeralTokenStore::new());
        let browser = recording_browser(Arc::clone(&state));
        let authority =
            McpOAuthAuthority::with_http(store.clone(), browser, Client::new(), test_auth_lease());
        let target = McpAuthTarget::new("glean", format!("{base}/mcp"));

        let result = authority.interactive_login(&target, None).await;

        assert_login_fails_closed(
            Arc::clone(&state),
            store,
            &target,
            result,
            "registration_endpoint",
        )
        .await;
        assert!(
            state.opened_url.lock().is_none(),
            "browser should not open when discovery cannot find DCR"
        );
    }

    #[tokio::test]
    async fn interactive_login_missing_dcr_auth_method_fails_closed() {
        let (base, state) = spawn_oauth_fixture().await;
        *state.omit_token_endpoint_auth_method.lock() = true;
        let store = Arc::new(EphemeralTokenStore::new());
        let browser = recording_browser(Arc::clone(&state));
        let authority =
            McpOAuthAuthority::with_http(store.clone(), browser, Client::new(), test_auth_lease());
        let target = McpAuthTarget::new("glean", format!("{base}/mcp"));

        let result = authority.interactive_login(&target, None).await;

        assert_login_fails_closed(
            Arc::clone(&state),
            store,
            &target,
            result,
            "token_endpoint_auth_method missing",
        )
        .await;
        assert!(
            state.opened_url.lock().is_none(),
            "browser should not open when DCR does not explicitly confirm public-client auth"
        );
    }

    #[tokio::test]
    async fn interactive_login_rejects_mismatched_protected_resource_metadata() {
        let (base, state) = spawn_oauth_fixture().await;
        *state.resource_override.lock() = Some(format!("{base}/other-mcp"));
        let store = Arc::new(EphemeralTokenStore::new());
        let browser = recording_browser(Arc::clone(&state));
        let authority =
            McpOAuthAuthority::with_http(store.clone(), browser, Client::new(), test_auth_lease());
        let target = McpAuthTarget::new("glean", format!("{base}/mcp"));

        let result = authority.interactive_login(&target, None).await;

        assert_login_fails_closed(
            Arc::clone(&state),
            store,
            &target,
            result,
            "does not match MCP server",
        )
        .await;
        assert!(
            state.opened_url.lock().is_none(),
            "browser should not open when protected resource metadata is mismatched"
        );
    }

    #[tokio::test]
    async fn interactive_login_rejects_relative_protected_resource_metadata() {
        let (base, state) = spawn_oauth_fixture().await;
        *state.resource_override.lock() = Some("/mcp".to_string());
        let store = Arc::new(EphemeralTokenStore::new());
        let browser = recording_browser(Arc::clone(&state));
        let authority =
            McpOAuthAuthority::with_http(store.clone(), browser, Client::new(), test_auth_lease());
        let target = McpAuthTarget::new("glean", format!("{base}/mcp"));

        let result = authority.interactive_login(&target, None).await;

        assert_login_fails_closed(
            Arc::clone(&state),
            store,
            &target,
            result,
            "does not match MCP server",
        )
        .await;
    }

    #[tokio::test]
    async fn interactive_login_rejects_remote_http_protected_resource() {
        let (_base, state) = spawn_oauth_fixture().await;
        let store = Arc::new(EphemeralTokenStore::new());
        let browser = recording_browser(Arc::clone(&state));
        let authority =
            McpOAuthAuthority::with_http(store.clone(), browser, Client::new(), test_auth_lease());
        let target = McpAuthTarget::new("glean", "http://mcp.example.test/mcp");

        let result = authority.interactive_login(&target, None).await;

        assert_login_fails_closed(
            Arc::clone(&state),
            store,
            &target,
            result,
            "MCP protected resource must use https",
        )
        .await;
        assert!(
            state.opened_url.lock().is_none(),
            "browser should not open for remote HTTP protected resources"
        );
    }

    #[tokio::test]
    async fn interactive_login_rejects_mismatched_authorization_server_issuer() {
        let (base, state) = spawn_oauth_fixture().await;
        *state.issuer_override.lock() = Some("https://issuer.example.invalid".to_string());
        let store = Arc::new(EphemeralTokenStore::new());
        let browser = recording_browser(Arc::clone(&state));
        let authority =
            McpOAuthAuthority::with_http(store.clone(), browser, Client::new(), test_auth_lease());
        let target = McpAuthTarget::new("glean", format!("{base}/mcp"));

        let result = authority.interactive_login(&target, None).await;

        assert_login_fails_closed(
            Arc::clone(&state),
            store,
            &target,
            result,
            "does not match discovered issuer",
        )
        .await;
        assert!(
            state.opened_url.lock().is_none(),
            "browser should not open when authorization-server metadata is mismatched"
        );
    }

    #[tokio::test]
    async fn interactive_login_rejects_issuer_without_exact_codepoint_match() {
        let (base, state) = spawn_oauth_fixture().await;
        *state.issuer_override.lock() = Some(format!("{base}/"));
        let store = Arc::new(EphemeralTokenStore::new());
        let browser = recording_browser(Arc::clone(&state));
        let authority =
            McpOAuthAuthority::with_http(store.clone(), browser, Client::new(), test_auth_lease());
        let target = McpAuthTarget::new("glean", format!("{base}/mcp"));

        let result = authority.interactive_login(&target, None).await;

        assert_login_fails_closed(
            Arc::clone(&state),
            store,
            &target,
            result,
            "does not match discovered issuer",
        )
        .await;
    }

    #[test]
    fn oauth_metadata_urls_must_be_https_unless_loopback() {
        let target = McpAuthTarget::new("glean", "https://mcp.example.test/mcp");

        require_https_or_loopback(
            &target,
            "https://issuer.example.test",
            "authorization server issuer",
        )
        .expect("https issuer is allowed");
        require_https_or_loopback(
            &target,
            "http://127.0.0.1:1234/register",
            "registration endpoint",
        )
        .expect("loopback http is allowed for local tests/dev");
        let error = require_https_or_loopback(
            &target,
            "http://issuer.example.test",
            "authorization server issuer",
        )
        .expect_err("remote http issuer should fail closed");
        assert!(error.to_string().contains("must use https"));
    }

    #[tokio::test]
    async fn interactive_login_token_exchange_failure_fails_closed() {
        let (base, state) = spawn_oauth_fixture().await;
        *state.token_fails.lock() = true;
        let store = Arc::new(EphemeralTokenStore::new());
        let browser = recording_browser(Arc::clone(&state));
        let authority =
            McpOAuthAuthority::with_http(store.clone(), browser, Client::new(), test_auth_lease());
        let target = McpAuthTarget::new("glean", format!("{base}/mcp"));

        let result = authority.interactive_login(&target, None).await;

        assert_login_fails_closed(Arc::clone(&state), store, &target, result, "invalid_grant")
            .await;
    }

    #[tokio::test]
    async fn interactive_login_state_mismatch_fails_closed() {
        let (base, state) = spawn_oauth_fixture().await;
        *state.authorize_outcome.lock() = AuthorizeOutcome::StateMismatch;
        let store = Arc::new(EphemeralTokenStore::new());
        let browser = recording_browser(Arc::clone(&state));
        let authority =
            McpOAuthAuthority::with_http(store.clone(), browser, Client::new(), test_auth_lease());
        let target = McpAuthTarget::new("glean", format!("{base}/mcp"));

        let result = authority.interactive_login(&target, None).await;

        assert_login_fails_closed(Arc::clone(&state), store, &target, result, "state mismatch")
            .await;
    }

    #[tokio::test]
    async fn interactive_login_denied_auth_fails_closed() {
        let (base, state) = spawn_oauth_fixture().await;
        *state.authorize_outcome.lock() = AuthorizeOutcome::Denied;
        let store = Arc::new(EphemeralTokenStore::new());
        let browser = recording_browser(Arc::clone(&state));
        let authority =
            McpOAuthAuthority::with_http(store.clone(), browser, Client::new(), test_auth_lease());
        let target = McpAuthTarget::new("glean", format!("{base}/mcp"));

        let result = authority.interactive_login(&target, None).await;

        assert_login_fails_closed(Arc::clone(&state), store, &target, result, "user denied").await;
    }

    #[tokio::test]
    async fn interactive_login_timeout_fails_closed() {
        let (base, state) = spawn_oauth_fixture().await;
        *state.authorize_outcome.lock() = AuthorizeOutcome::NoCallback;
        let store = Arc::new(EphemeralTokenStore::new());
        let browser: Arc<dyn BrowserOpener> = Arc::new(NoCallbackBrowser {
            state: Arc::clone(&state),
        });
        let authority = McpOAuthAuthority::with_http_and_login_timeout(
            store.clone(),
            browser,
            Client::new(),
            Duration::from_millis(10),
            test_auth_lease(),
        );
        let target = McpAuthTarget::new("glean", format!("{base}/mcp"));

        let result = authority.interactive_login(&target, None).await;

        assert_login_fails_closed(Arc::clone(&state), store, &target, result, "timeout").await;
        assert!(
            state.opened_url.lock().is_some(),
            "timeout test should still reach the browser-open boundary"
        );
    }
}
