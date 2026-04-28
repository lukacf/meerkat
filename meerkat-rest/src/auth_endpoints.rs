//! REST endpoints for `/auth/*` + `/realms*`.
//!
//! Read-side endpoints resolve against the active `Config.realm` map.
//! Write-side endpoints (login start / complete, profile delete,
//! logout) use the shared `AppState.token_store` so persisted
//! credentials are visible to the AgentFactory's `resolve_binding`
//! path.
//!
//! OAuth state and PKCE verifier correlation is owned server-side by
//! `OAuthFlowRegistry`; complete consumes the state before exchanging
//! the provider code.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;

use meerkat_anthropic::runtime::oauth as a_oauth;
use meerkat_contracts::{
    WireAuthProfile, WireAuthProfileCleared, WireAuthProfileCreated, WireAuthProfileDetail,
    WireAuthProfilesList, WireAuthStatusDetail, WireBackendProfile, WireBindingIdentity,
    WireDeviceStart, WireLoginReady, WireLoginStart, WireProviderBinding, WireRealmConnectionSet,
    WireRealmList, WireRealmSummary,
};
use meerkat_core::connection::{BindingId, ConnectionTargetError, ProfileId, RealmId};
use meerkat_core::{
    AuthStatusPhase, ConnectionRef, CredentialSourceSpec, Provider, RealmConnectionSet,
    ResolvedConnectionTarget,
};
use meerkat_gemini::runtime::oauth as g_oauth;
use meerkat_openai::runtime::oauth as o_oauth;
use meerkat_providers::auth_oauth::{
    DevicePollOutcome, OAuthEndpoints, OAuthError, PkcePair, exchange_authorization_code,
    poll_device_code, request_device_code,
};
use meerkat_providers::auth_store::{PersistedAuthMode, PersistedTokens, TokenKey};
use meerkat_providers::oauth_flow::{OAuthFlowError, global_oauth_flow_registry};

use crate::AppState;

type ProviderEndpointResolution = (
    Provider,
    OAuthEndpoints,
    PersistedAuthMode,
    Option<&'static str>,
);

async fn load_config(state: &AppState) -> Result<meerkat_core::Config, (StatusCode, String)> {
    state
        .config_runtime
        .get()
        .await
        .map(|snap| snap.config)
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to load config: {e}"),
            )
        })
}

async fn resolve_realm(
    state: &AppState,
    realm_id: &RealmId,
) -> Result<RealmConnectionSet, (StatusCode, String)> {
    let config = load_config(state).await?;
    let section = config
        .realm
        .get(realm_id.as_str())
        .ok_or_else(|| (StatusCode::NOT_FOUND, format!("Unknown realm: {realm_id}")))?;
    RealmConnectionSet::from_config(realm_id.as_str(), section).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Realm config invalid: {e}"),
        )
    })
}

async fn resolve_binding_identity(
    state: &AppState,
    realm_id: &RealmId,
    binding_id: &BindingId,
    profile_id: Option<&ProfileId>,
) -> Result<
    (
        ConnectionRef,
        meerkat_core::ProviderBinding,
        meerkat_core::AuthProfile,
    ),
    (StatusCode, String),
> {
    let realm = resolve_realm(state, realm_id).await?;
    let connection_ref = ConnectionRef {
        realm: realm_id.clone(),
        binding: binding_id.clone(),
        profile: profile_id.cloned(),
    };
    let (binding, _, auth_profile) = realm.lookup_connection_ref(&connection_ref).map_err(|e| {
        (
            StatusCode::NOT_FOUND,
            format!("Unknown auth identity {realm_id}:{binding_id}: {e}"),
        )
    })?;
    Ok((connection_ref, binding.clone(), auth_profile.clone()))
}

fn target_error_status(error: &ConnectionTargetError) -> StatusCode {
    match error {
        ConnectionTargetError::UnknownRealm(_)
        | ConnectionTargetError::MissingDefaultBinding { .. }
        | ConnectionTargetError::BindingInvalid { .. } => StatusCode::NOT_FOUND,
        ConnectionTargetError::RealmConfigInvalid { .. } => StatusCode::INTERNAL_SERVER_ERROR,
        ConnectionTargetError::MissingRealm
        | ConnectionTargetError::InvalidRealmId { .. }
        | ConnectionTargetError::InvalidBindingId { .. }
        | ConnectionTargetError::ProviderMismatch { .. } => StatusCode::BAD_REQUEST,
    }
}

async fn resolve_oauth_target(
    state: &AppState,
    provider: Provider,
    realm_id: Option<&RealmId>,
    binding_id: Option<&BindingId>,
    profile_id: Option<&ProfileId>,
) -> Result<ResolvedConnectionTarget, (StatusCode, String)> {
    let config = load_config(state).await?;
    meerkat_core::resolve_realm_binding_target_for_provider(
        &config, provider, realm_id, binding_id, profile_id, None, false,
    )
    .map_err(|error| (target_error_status(&error), error.to_string()))
}

fn require_explicit_oauth_identity<'a>(
    realm_id: Option<&'a RealmId>,
    binding_id: Option<&'a BindingId>,
) -> Result<(&'a RealmId, &'a BindingId), (StatusCode, String)> {
    let realm_id = realm_id.ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            "realm_id is required for OAuth login completion".to_string(),
        )
    })?;
    let binding_id = binding_id.ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            "binding_id is required for OAuth login completion".to_string(),
        )
    })?;
    Ok((realm_id, binding_id))
}

fn require_managed_store_source(
    binding_id: &BindingId,
    auth_profile: &meerkat_core::AuthProfile,
) -> Result<(), (StatusCode, String)> {
    if matches!(&auth_profile.source, CredentialSourceSpec::ManagedStore) {
        return Ok(());
    }
    Err((
        StatusCode::BAD_REQUEST,
        format!(
            "binding {binding_id} resolves auth profile '{}' with source '{}'; \
             POST /auth/profiles can only persist credentials for source.kind = 'managed_store'",
            auth_profile.id,
            source_kind_label(&auth_profile.source),
        ),
    ))
}

fn source_kind_label(source: &CredentialSourceSpec) -> &'static str {
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

// --- Realm endpoints -------------------------------------------------

pub async fn list_realms(State(state): State<AppState>) -> impl IntoResponse {
    match load_config(&state).await {
        Ok(config) => {
            let realms: Vec<WireRealmSummary> = config
                .realm
                .iter()
                .map(|(realm_id, section)| WireRealmSummary {
                    realm_id: realm_id.clone(),
                    default_binding: section.default_binding.clone(),
                    backend_count: section.backend.len(),
                    auth_profile_count: section.auth.len(),
                    binding_count: section.binding.len(),
                })
                .collect();
            (StatusCode::OK, Json(WireRealmList { realms })).into_response()
        }
        Err((status, msg)) => (status, Json(serde_json::json!({ "error": msg }))).into_response(),
    }
}

pub async fn get_realm(
    State(state): State<AppState>,
    Path(realm_id): Path<RealmId>,
) -> impl IntoResponse {
    match resolve_realm(&state, &realm_id).await {
        Ok(realm) => {
            let wire = WireRealmConnectionSet::from(&realm);
            (StatusCode::OK, Json(wire)).into_response()
        }
        Err((status, msg)) => (status, Json(serde_json::json!({ "error": msg }))).into_response(),
    }
}

// --- Auth profile endpoints ------------------------------------------

#[derive(serde::Deserialize)]
pub struct RealmQuery {
    pub realm_id: RealmId,
    #[serde(default)]
    pub profile_id: Option<ProfileId>,
}

pub async fn list_auth_profiles(
    State(state): State<AppState>,
    Query(query): Query<RealmQuery>,
) -> impl IntoResponse {
    match resolve_realm(&state, &query.realm_id).await {
        Ok(realm) => {
            let profiles: Vec<WireAuthProfile> = realm
                .auth_profiles
                .values()
                .map(WireAuthProfile::from)
                .collect();
            let backends: Vec<WireBackendProfile> = realm
                .backends
                .values()
                .map(WireBackendProfile::from)
                .collect();
            let bindings: Vec<WireProviderBinding> = realm
                .bindings
                .values()
                .map(WireProviderBinding::from)
                .collect();
            (
                StatusCode::OK,
                Json(WireAuthProfilesList {
                    realm_id: realm.realm_id,
                    auth_profiles: profiles,
                    backend_profiles: backends,
                    bindings,
                }),
            )
                .into_response()
        }
        Err((status, msg)) => (status, Json(serde_json::json!({ "error": msg }))).into_response(),
    }
}

#[derive(serde::Deserialize)]
pub struct CreateAuthProfileBody {
    pub realm_id: RealmId,
    pub binding_id: BindingId,
    #[serde(default)]
    pub profile_id: Option<ProfileId>,
    pub provider: String,
    pub auth_method: String,
    pub secret: String,
}

/// Create an auth credential entry by writing the secret into the
/// TokenStore under the binding-scoped `ConnectionRef`. The resolved
/// auth profile must declare `source.kind = "managed_store"`; otherwise
/// the provider runtime would read a different credential source.
pub async fn create_auth_profile(
    State(state): State<AppState>,
    Json(body): Json<CreateAuthProfileBody>,
) -> impl IntoResponse {
    let (connection_ref, _binding, auth_profile) = match resolve_binding_identity(
        &state,
        &body.realm_id,
        &body.binding_id,
        body.profile_id.as_ref(),
    )
    .await
    {
        Ok(v) => v,
        Err((status, msg)) => {
            return (status, Json(serde_json::json!({ "error": msg }))).into_response();
        }
    };
    if body.provider != auth_profile.provider.as_str() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": format!(
                    "binding {} resolves provider '{}' not '{}'",
                    body.binding_id,
                    auth_profile.provider.as_str(),
                    body.provider,
                ),
            })),
        )
            .into_response();
    }
    if body.auth_method != auth_profile.auth_method {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": format!(
                    "binding {} resolves auth_method '{}' not '{}'",
                    body.binding_id,
                    auth_profile.auth_method,
                    body.auth_method,
                ),
            })),
        )
            .into_response();
    }
    if let Err((status, msg)) = require_managed_store_source(&body.binding_id, &auth_profile) {
        return (status, Json(serde_json::json!({ "error": msg }))).into_response();
    }
    let auth_mode = match body.auth_method.as_str() {
        "api_key" => PersistedAuthMode::ApiKey,
        "static_bearer" => PersistedAuthMode::StaticBearer,
        other => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": format!(
                        "auth_method '{other}' cannot be created via the REST endpoint. \
                         OAuth methods use /auth/login/start + /auth/login/complete; \
                         managed_store and external_resolver are configured via TOML."
                    ),
                })),
            )
                .into_response();
        }
    };
    let tokens = PersistedTokens {
        auth_mode,
        primary_secret: Some(body.secret),
        refresh_token: None,
        id_token: None,
        expires_at: None,
        last_refresh: Some(chrono::Utc::now()),
        scopes: Vec::new(),
        account_id: None,
        metadata: serde_json::Value::Null,
    };
    let key = TokenKey::from_connection_ref(&connection_ref);
    match state.token_store.save(&key, &tokens).await {
        Ok(()) => {
            tracing::info!(
                target: "meerkat::auth::audit",
                binding_key = ?connection_ref,
                action = "create_profile",
                provider = %auth_profile.provider.as_str(),
                auth_method = %auth_profile.auth_method,
                "binding-scoped auth credentials stored via REST"
            );
            (
                StatusCode::CREATED,
                Json(WireAuthProfileCreated {
                    identity: WireBindingIdentity::from(&connection_ref),
                    profile_id: auth_profile.id.clone(),
                    provider: auth_profile.provider.as_str().to_string(),
                    auth_method: auth_profile.auth_method.clone(),
                    stored: true,
                }),
            )
                .into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("TokenStore save failed: {e}") })),
        )
            .into_response(),
    }
}

pub async fn get_auth_profile(
    State(state): State<AppState>,
    Path(binding_id): Path<BindingId>,
    Query(query): Query<RealmQuery>,
) -> impl IntoResponse {
    match resolve_binding_identity(
        &state,
        &query.realm_id,
        &binding_id,
        query.profile_id.as_ref(),
    )
    .await
    {
        Ok((connection_ref, binding, auth_profile)) => (
            StatusCode::OK,
            Json(WireAuthProfileDetail {
                connection_ref: connection_ref.into(),
                binding_id: binding.id.clone(),
                profile_id: auth_profile.id.clone(),
                auth_profile: WireAuthProfile::from(&auth_profile),
            }),
        )
            .into_response(),
        Err((status, msg)) => (status, Json(serde_json::json!({ "error": msg }))).into_response(),
    }
}

pub async fn delete_auth_profile(
    State(state): State<AppState>,
    Path(binding_id): Path<BindingId>,
    Query(query): Query<RealmQuery>,
) -> impl IntoResponse {
    let (connection_ref, _binding, auth_profile) = match resolve_binding_identity(
        &state,
        &query.realm_id,
        &binding_id,
        query.profile_id.as_ref(),
    )
    .await
    {
        Ok(v) => v,
        Err((status, msg)) => {
            return (status, Json(serde_json::json!({ "error": msg }))).into_response();
        }
    };
    let key = TokenKey::from_connection_ref(&connection_ref);
    match state.token_store.clear(&key).await {
        Ok(()) => {
            tracing::info!(
                target: "meerkat::auth::audit",
                binding_key = ?connection_ref,
                action = "delete_profile",
                "binding-scoped auth credentials deleted via REST"
            );
            (
                StatusCode::NO_CONTENT,
                Json(WireAuthProfileCleared {
                    identity: WireBindingIdentity::from(&connection_ref),
                    profile_id: auth_profile.id.clone(),
                    cleared: true,
                }),
            )
                .into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("TokenStore clear failed: {e}") })),
        )
            .into_response(),
    }
}

#[derive(serde::Deserialize)]
pub struct TestBindingBody {
    pub realm_id: RealmId,
    #[serde(default)]
    pub profile_id: Option<ProfileId>,
}

pub async fn test_auth_binding(
    State(state): State<AppState>,
    Path(binding_id): Path<BindingId>,
    Json(body): Json<TestBindingBody>,
) -> impl IntoResponse {
    match resolve_realm(&state, &body.realm_id).await {
        Ok(realm) => {
            let env = meerkat_providers::ResolverEnvironment::with_process_env()
                .with_token_store(Arc::clone(&state.token_store));
            let connection_ref = ConnectionRef {
                realm: body.realm_id.clone(),
                binding: binding_id.clone(),
                profile: body.profile_id.clone(),
            };
            match state
                .provider_registry
                .resolve(&realm, &connection_ref, &env)
                .await
            {
                Ok(conn) => {
                    let (connection_ref, binding, auth_profile) = match resolve_binding_identity(
                        &state,
                        &body.realm_id,
                        &binding_id,
                        body.profile_id.as_ref(),
                    )
                    .await
                    {
                        Ok(v) => v,
                        Err((status, msg)) => {
                            return (status, Json(serde_json::json!({ "error": msg })))
                                .into_response();
                        }
                    };
                    (
                        StatusCode::OK,
                        Json(serde_json::json!({
                            "state": "valid",
                            "connection_ref": &connection_ref,
                            "binding_id": &binding.id,
                            "profile_id": &auth_profile.id,
                            "provider": conn.provider.as_str(),
                            "backend_profile_id": &conn.backend_profile.id,
                            "has_credential": conn.resolved_secret().is_some()
                                || conn.resolved_authorizer().is_some(),
                        })),
                    )
                        .into_response()
                }
                Err(e) => (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    Json(serde_json::json!({
                        "state": "error",
                        "error": format!("Binding resolution failed: {e}"),
                    })),
                )
                    .into_response(),
            }
        }
        Err((status, msg)) => (status, Json(serde_json::json!({ "error": msg }))).into_response(),
    }
}

// --- OAuth login flow -----------------------------------------------
//
// The server owns the state -> PKCE verifier correlation. The client receives
// only the authorize URL and state, then posts the provider code with that state.

fn provider_endpoints(
    provider: &str,
    redirect_uri: &str,
) -> Result<ProviderEndpointResolution, (StatusCode, String)> {
    match provider {
        "anthropic" | "claude" | "claude.ai" => Ok((
            Provider::Anthropic,
            a_oauth::claude_ai_endpoints(redirect_uri),
            PersistedAuthMode::ClaudeAiOauth,
            None,
        )),
        "openai" | "chatgpt" => Ok((
            Provider::OpenAI,
            o_oauth::chatgpt_endpoints(redirect_uri),
            PersistedAuthMode::ChatgptOauth,
            None,
        )),
        "google" | "gemini" | "code_assist" => Ok((
            Provider::Gemini,
            g_oauth::code_assist_endpoints(redirect_uri),
            PersistedAuthMode::GoogleOauth,
            Some(g_oauth::CODE_ASSIST_CLIENT_SECRET),
        )),
        other => Err((
            StatusCode::BAD_REQUEST,
            format!("Unknown provider '{other}'. Supported: anthropic, openai, google."),
        )),
    }
}

#[derive(serde::Deserialize)]
pub struct LoginStartBody {
    pub provider: String,
    /// Client-provided redirect URI (typically a loopback binding that
    /// the caller has already bound). The authorize URL will embed this.
    pub redirect_uri: String,
}

pub async fn start_login(Json(body): Json<LoginStartBody>) -> impl IntoResponse {
    let (_provider, endpoints, _mode, _secret) =
        match provider_endpoints(&body.provider, &body.redirect_uri) {
            Ok(v) => v,
            Err((status, msg)) => {
                return (status, Json(serde_json::json!({ "error": msg }))).into_response();
            }
        };
    let pkce = PkcePair::generate_s256();
    let verifier = pkce.verifier.secret().to_string();
    let state_token = match global_oauth_flow_registry().start(
        body.provider.clone(),
        body.redirect_uri.clone(),
        verifier,
    ) {
        Ok(state) => state,
        Err(OAuthFlowError::CapacityExceeded { .. }) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({ "error": "oauth state registry is at capacity" })),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("oauth state initialization failed: {e}") })),
            )
                .into_response();
        }
    };
    let authorize_url = endpoints.authorize_url_with_pkce(&pkce.challenge, &state_token);
    (
        StatusCode::OK,
        Json(WireLoginStart {
            authorize_url,
            state: state_token,
            redirect_uri: body.redirect_uri,
            provider: body.provider,
        }),
    )
        .into_response()
}

#[derive(serde::Deserialize)]
pub struct LoginCompleteBody {
    pub provider: String,
    pub code: String,
    pub state: String,
    pub redirect_uri: String,
    #[serde(default)]
    pub realm_id: Option<RealmId>,
    #[serde(default)]
    pub binding_id: Option<BindingId>,
    #[serde(default)]
    pub profile_id: Option<ProfileId>,
}

pub async fn complete_login(
    State(state): State<AppState>,
    Json(body): Json<LoginCompleteBody>,
) -> impl IntoResponse {
    let (provider, endpoints, mode, client_secret) =
        match provider_endpoints(&body.provider, &body.redirect_uri) {
            Ok(v) => v,
            Err((status, msg)) => {
                return (status, Json(serde_json::json!({ "error": msg }))).into_response();
            }
        };
    let (realm_id, binding_id) =
        match require_explicit_oauth_identity(body.realm_id.as_ref(), body.binding_id.as_ref()) {
            Ok(v) => v,
            Err((status, msg)) => {
                return (status, Json(serde_json::json!({ "error": msg }))).into_response();
            }
        };
    let target = match resolve_oauth_target(
        &state,
        provider,
        Some(realm_id),
        Some(binding_id),
        body.profile_id.as_ref(),
    )
    .await
    {
        Ok(v) => v,
        Err((status, msg)) => {
            return (status, Json(serde_json::json!({ "error": msg }))).into_response();
        }
    };
    let connection_ref = target.connection_ref;
    let binding = target.binding;
    let auth_profile = target.auth_profile;
    if provider != auth_profile.provider {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": format!(
                    "binding {} resolves provider '{}' not '{}'",
                    binding.id,
                    auth_profile.provider.as_str(),
                    body.provider,
                ),
            })),
        )
            .into_response();
    }

    let flow =
        match global_oauth_flow_registry().consume(&body.state, &body.provider, &body.redirect_uri)
        {
            Ok(flow) => flow,
            Err(OAuthFlowError::Missing) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({ "error": "oauth state is missing or expired" })),
                )
                    .into_response();
            }
            Err(e) => {
                return (
                StatusCode::BAD_REQUEST,
                Json(
                    serde_json::json!({ "error": format!("oauth state verification failed: {e}") }),
                ),
            )
                .into_response();
            }
        };

    let http = reqwest::Client::new();
    let result = match exchange_authorization_code(
        &http,
        &endpoints,
        &body.code,
        &flow.pkce_verifier,
        client_secret,
    )
    .await
    {
        Ok(r) => r,
        Err(OAuthError::TokenEndpoint { status, body }) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({
                    "error": "token endpoint rejected code",
                    "upstream_status": status,
                    "upstream_body": body,
                })),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("Token exchange failed: {e}") })),
            )
                .into_response();
        }
    };

    let expires_at = result
        .expires_in_secs
        .map(|s| chrono::Utc::now() + chrono::Duration::seconds(s as i64));
    let tokens = PersistedTokens {
        auth_mode: mode,
        primary_secret: Some(result.access_token),
        refresh_token: result.refresh_token,
        id_token: result.id_token,
        expires_at,
        last_refresh: Some(chrono::Utc::now()),
        scopes: result
            .scope
            .as_deref()
            .map(|s| s.split_whitespace().map(String::from).collect())
            .unwrap_or_default(),
        account_id: None,
        metadata: serde_json::Value::Null,
    };

    let key = TokenKey::from_connection_ref(&connection_ref);
    if let Err(e) = state.token_store.save(&key, &tokens).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("TokenStore save failed: {e}") })),
        )
            .into_response();
    }

    tracing::info!(
        target: "meerkat::auth::audit",
        binding_key = ?connection_ref,
        action = "login_oauth_complete",
        provider = %body.provider,
        has_refresh_token = %tokens.refresh_token.is_some(),
        "OAuth login completed via REST"
    );

    (
        StatusCode::OK,
        Json(WireLoginReady {
            state: None,
            identity: WireBindingIdentity::from(&connection_ref),
            profile_id: auth_profile.id.clone(),
            provider: body.provider,
            expires_at: expires_at.map(|e| e.to_rfc3339()),
            has_refresh_token: tokens.refresh_token.is_some(),
            scopes: tokens.scopes.clone(),
        }),
    )
        .into_response()
}

#[derive(serde::Deserialize)]
pub struct DeviceStartBody {
    pub provider: String,
}

pub async fn start_device_login(Json(body): Json<DeviceStartBody>) -> impl IntoResponse {
    let (_provider, endpoints, _mode, _secret) = match provider_endpoints(&body.provider, "") {
        Ok(v) => v,
        Err((status, msg)) => {
            return (status, Json(serde_json::json!({ "error": msg }))).into_response();
        }
    };
    if endpoints.device_code_url.is_none() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": format!(
                    "provider '{}' does not support the device-code flow",
                    body.provider,
                ),
            })),
        )
            .into_response();
    }
    let http = reqwest::Client::new();
    match request_device_code(&http, &endpoints).await {
        Ok(resp) => (
            StatusCode::OK,
            Json(WireDeviceStart {
                device_code: resp.device_code,
                user_code: resp.user_code,
                verification_uri: resp.verification_uri,
                verification_uri_complete: resp.verification_uri_complete,
                expires_in: resp.expires_in,
                interval: resp.interval,
                provider: body.provider,
            }),
        )
            .into_response(),
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({
                "error": format!("device-code request failed: {e}"),
            })),
        )
            .into_response(),
    }
}

#[derive(serde::Deserialize)]
pub struct DeviceCompleteBody {
    pub provider: String,
    pub device_code: String,
    #[serde(default)]
    pub realm_id: Option<RealmId>,
    #[serde(default)]
    pub binding_id: Option<BindingId>,
    #[serde(default)]
    pub profile_id: Option<ProfileId>,
}

/// Device-code flow completion leg. Single-poll semantics — the caller
/// runs the outer retry loop using the `interval` returned from
/// `POST /auth/login/device/start`. Returns:
///   * `{ "state": "pending" }`     (202)
///   * `{ "state": "slow_down" }`   (429)
///   * `{ "state": "access_denied" }` / `{ "state": "expired" }` (400)
///   * `{ "state": "ready", realm_id, binding_id, ... }` (200, tokens
///     persisted to TokenStore under the resolved `ConnectionRef`).
pub async fn complete_device_login(
    State(state): State<AppState>,
    Json(body): Json<DeviceCompleteBody>,
) -> impl IntoResponse {
    let (provider, endpoints, mode, client_secret) = match provider_endpoints(&body.provider, "") {
        Ok(v) => v,
        Err((status, msg)) => {
            return (status, Json(serde_json::json!({ "error": msg }))).into_response();
        }
    };
    if endpoints.device_code_url.is_none() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": format!(
                    "provider '{}' does not support the device-code flow",
                    body.provider,
                ),
            })),
        )
            .into_response();
    }
    let (realm_id, binding_id) =
        match require_explicit_oauth_identity(body.realm_id.as_ref(), body.binding_id.as_ref()) {
            Ok(v) => v,
            Err((status, msg)) => {
                return (status, Json(serde_json::json!({ "error": msg }))).into_response();
            }
        };
    let target = match resolve_oauth_target(
        &state,
        provider,
        Some(realm_id),
        Some(binding_id),
        body.profile_id.as_ref(),
    )
    .await
    {
        Ok(v) => v,
        Err((status, msg)) => {
            return (status, Json(serde_json::json!({ "error": msg }))).into_response();
        }
    };
    let connection_ref = target.connection_ref;
    let binding = target.binding;
    let auth_profile = target.auth_profile;
    if provider != auth_profile.provider {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": format!(
                    "binding {} resolves provider '{}' not '{}'",
                    binding.id,
                    auth_profile.provider.as_str(),
                    body.provider,
                ),
            })),
        )
            .into_response();
    }
    let http = reqwest::Client::new();
    let outcome = match poll_device_code(&http, &endpoints, &body.device_code, client_secret).await
    {
        Ok(o) => o,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({
                    "error": format!("device-code poll failed: {e}"),
                })),
            )
                .into_response();
        }
    };
    match outcome {
        DevicePollOutcome::Pending => (
            StatusCode::ACCEPTED,
            Json(serde_json::json!({ "state": "pending" })),
        )
            .into_response(),
        DevicePollOutcome::SlowDown => (
            StatusCode::TOO_MANY_REQUESTS,
            Json(serde_json::json!({ "state": "slow_down" })),
        )
            .into_response(),
        DevicePollOutcome::AccessDenied => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "state": "access_denied" })),
        )
            .into_response(),
        DevicePollOutcome::Expired => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "state": "expired" })),
        )
            .into_response(),
        DevicePollOutcome::Ready(result) => {
            let expires_at = result
                .expires_in_secs
                .map(|s| chrono::Utc::now() + chrono::Duration::seconds(s as i64));
            let tokens = PersistedTokens {
                auth_mode: mode,
                primary_secret: Some(result.access_token),
                refresh_token: result.refresh_token,
                id_token: result.id_token,
                expires_at,
                last_refresh: Some(chrono::Utc::now()),
                scopes: result
                    .scope
                    .as_deref()
                    .map(|s| s.split_whitespace().map(String::from).collect())
                    .unwrap_or_default(),
                account_id: None,
                metadata: serde_json::Value::Null,
            };
            let key = TokenKey::from_connection_ref(&connection_ref);
            if let Err(e) = state.token_store.save(&key, &tokens).await {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({
                        "error": format!("TokenStore save failed: {e}"),
                    })),
                )
                    .into_response();
            }
            tracing::info!(
                target: "meerkat::auth::audit",
                binding_key = ?connection_ref,
                action = "login_device_complete",
                provider = %body.provider,
                has_refresh_token = %tokens.refresh_token.is_some(),
                "OAuth device-flow login completed via REST"
            );
            (
                StatusCode::OK,
                Json(WireLoginReady {
                    state: Some("ready".to_string()),
                    identity: WireBindingIdentity::from(&connection_ref),
                    profile_id: auth_profile.id.clone(),
                    provider: body.provider,
                    expires_at: expires_at.map(|e| e.to_rfc3339()),
                    has_refresh_token: tokens.refresh_token.is_some(),
                    scopes: tokens.scopes.clone(),
                }),
            )
                .into_response()
        }
    }
}

pub async fn get_auth_status(
    State(state): State<AppState>,
    Path(binding_id): Path<BindingId>,
    Query(query): Query<RealmQuery>,
) -> impl IntoResponse {
    let (connection_ref, _binding, auth_profile) = match resolve_binding_identity(
        &state,
        &query.realm_id,
        &binding_id,
        query.profile_id.as_ref(),
    )
    .await
    {
        Ok(v) => v,
        Err((status, msg)) => {
            return (status, Json(serde_json::json!({ "error": msg }))).into_response();
        }
    };
    let key = TokenKey::from_connection_ref(&connection_ref);
    let stored = match state.token_store.load(&key).await {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("TokenStore load failed: {e}") })),
            )
                .into_response();
        }
    };
    let state_phase = AuthStatusPhase::from_persisted_tokens(chrono::Utc::now(), stored.as_ref());
    (
        StatusCode::OK,
        Json(WireAuthStatusDetail {
            identity: WireBindingIdentity::from(&connection_ref),
            profile_id: auth_profile.id.clone(),
            provider: auth_profile.provider.as_str().to_string(),
            auth_method: auth_profile.auth_method.clone(),
            state: state_phase.as_public_str().to_string(),
            expires_at: stored
                .as_ref()
                .and_then(|t| t.expires_at.map(|e| e.to_rfc3339())),
            last_refresh_at: stored
                .as_ref()
                .and_then(|t| t.last_refresh.map(|e| e.to_rfc3339())),
            account_id: stored.as_ref().and_then(|t| t.account_id.clone()),
            has_refresh_token: stored
                .as_ref()
                .map(|t| t.refresh_token.is_some())
                .unwrap_or(false),
        }),
    )
        .into_response()
}

pub async fn logout(
    State(state): State<AppState>,
    Path(binding_id): Path<BindingId>,
    Query(query): Query<RealmQuery>,
) -> impl IntoResponse {
    let (connection_ref, _binding, auth_profile) = match resolve_binding_identity(
        &state,
        &query.realm_id,
        &binding_id,
        query.profile_id.as_ref(),
    )
    .await
    {
        Ok(v) => v,
        Err((status, msg)) => {
            return (status, Json(serde_json::json!({ "error": msg }))).into_response();
        }
    };
    let key = TokenKey::from_connection_ref(&connection_ref);
    match state.token_store.clear(&key).await {
        Ok(()) => {
            tracing::info!(
                target: "meerkat::auth::audit",
                binding_key = ?connection_ref,
                action = "logout",
                "binding-scoped auth credentials logged out via REST"
            );
            (
                StatusCode::OK,
                Json(WireAuthProfileCleared {
                    identity: WireBindingIdentity::from(&connection_ref),
                    profile_id: auth_profile.id.clone(),
                    cleared: true,
                }),
            )
                .into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("TokenStore clear failed: {e}") })),
        )
            .into_response(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn login_complete_body_does_not_invent_default_identity() {
        let body: LoginCompleteBody = serde_json::from_value(serde_json::json!({
            "provider": "anthropic",
            "code": "code",
            "state": "state",
            "redirect_uri": "http://127.0.0.1:0/callback"
        }))
        .unwrap();

        assert!(body.realm_id.is_none());
        assert!(body.binding_id.is_none());
    }

    #[test]
    fn device_complete_body_does_not_invent_default_identity() {
        let body: DeviceCompleteBody = serde_json::from_value(serde_json::json!({
            "provider": "anthropic",
            "device_code": "device-code"
        }))
        .unwrap();

        assert!(body.realm_id.is_none());
        assert!(body.binding_id.is_none());
    }
}
