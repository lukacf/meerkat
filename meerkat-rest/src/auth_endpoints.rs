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

use meerkat_contracts::{
    WireAuthProfile, WireAuthProfileCleared, WireAuthProfileCreated, WireAuthProfileDetail,
    WireAuthProfilesList, WireAuthStatusDetail, WireBackendProfile, WireBindingIdentity,
    WireDeviceStart, WireLoginReady, WireLoginStart, WireProviderBinding, WireRealmConnectionSet,
    WireRealmList, WireRealmSummary,
};
use meerkat_core::connection::{BindingId, ConnectionTargetError, ProfileId, RealmId};
use meerkat_core::handles::LeaseKey;
use meerkat_core::{
    ConnectionRef, CredentialSourceSpec, Provider, RealmConnectionSet, ResolvedConnectionTarget,
};
use meerkat_providers::auth_oauth::{
    DevicePollOutcome, OAuthError, PkcePair, exchange_authorization_code, poll_device_code,
    request_device_code,
};
use meerkat_providers::auth_store::{PersistedAuthMode, PersistedTokens, TokenKey, TokenStore};
use meerkat_providers::oauth_flow::{
    OAuthFlowError, global_oauth_flow_registry, resolve_oauth_provider,
};

use crate::AppState;

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

async fn save_tokens_and_publish_lifecycle(
    token_store: &dyn TokenStore,
    auth_lease: &dyn meerkat_core::handles::AuthLeaseHandle,
    connection_ref: &ConnectionRef,
    tokens: &PersistedTokens,
) -> Result<(), (StatusCode, String)> {
    let key = TokenKey::from_connection_ref(connection_ref);
    let previous = token_store.load(&key).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("TokenStore load failed: {e}"),
        )
    })?;
    token_store.save(&key, tokens).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("TokenStore save failed: {e}"),
        )
    })?;
    if let Err(e) =
        meerkat_core::publish_token_lifecycle_acquired(auth_lease, connection_ref, tokens)
    {
        if let Err(rollback_error) =
            restore_tokens_after_lifecycle_failure(token_store, &key, previous.as_ref()).await
        {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!(
                    "AuthMachine lifecycle acquire failed: {e}; TokenStore rollback failed: {rollback_error}"
                ),
            ));
        }
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("AuthMachine lifecycle acquire failed: {e}"),
        ));
    }
    Ok(())
}

async fn restore_tokens_after_lifecycle_failure(
    token_store: &dyn TokenStore,
    key: &TokenKey,
    previous: Option<&PersistedTokens>,
) -> Result<(), meerkat_providers::auth_store::TokenStoreError> {
    match previous {
        Some(tokens) => token_store.save(key, tokens).await,
        None => token_store.clear(key).await,
    }
}

async fn clear_tokens_and_publish_lifecycle(
    token_store: &dyn TokenStore,
    auth_lease: &dyn meerkat_core::handles::AuthLeaseHandle,
    connection_ref: &ConnectionRef,
) -> Result<(), (StatusCode, String)> {
    meerkat_core::clear_tokens_and_publish_lifecycle_released(
        token_store,
        auth_lease,
        connection_ref,
    )
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
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
    if let Err((status, msg)) = save_tokens_and_publish_lifecycle(
        state.token_store.as_ref(),
        state.auth_lease.as_ref(),
        &connection_ref,
        &tokens,
    )
    .await
    {
        return (status, Json(serde_json::json!({ "error": msg }))).into_response();
    }
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
    if let Err((status, msg)) = clear_tokens_and_publish_lifecycle(
        state.token_store.as_ref(),
        state.auth_lease.as_ref(),
        &connection_ref,
    )
    .await
    {
        return (status, Json(serde_json::json!({ "error": msg }))).into_response();
    }
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

#[derive(serde::Deserialize)]
pub struct LoginStartBody {
    pub provider: String,
    /// Client-provided redirect URI (typically a loopback binding that
    /// the caller has already bound). The authorize URL will embed this.
    pub redirect_uri: String,
}

pub async fn start_login(Json(body): Json<LoginStartBody>) -> impl IntoResponse {
    let resolved = match resolve_oauth_provider(&body.provider, &body.redirect_uri) {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    };
    let pkce = PkcePair::generate_s256();
    let verifier = pkce.verifier.secret().to_string();
    let state_token = match global_oauth_flow_registry().start(
        resolved.identity,
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
    let authorize_url = resolved
        .endpoints
        .authorize_url_with_pkce(&pkce.challenge, &state_token);
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
    let resolved = match resolve_oauth_provider(&body.provider, &body.redirect_uri) {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    };
    let provider = resolved.provider;
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

    let flow = match global_oauth_flow_registry().consume(
        &body.state,
        resolved.identity,
        &body.redirect_uri,
    ) {
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
        &resolved.endpoints,
        &body.code,
        &flow.pkce_verifier,
        resolved.client_secret,
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
        auth_mode: resolved.auth_mode,
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

    if let Err((status, msg)) = save_tokens_and_publish_lifecycle(
        state.token_store.as_ref(),
        state.auth_lease.as_ref(),
        &connection_ref,
        &tokens,
    )
    .await
    {
        return (status, Json(serde_json::json!({ "error": msg }))).into_response();
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
    let resolved = match resolve_oauth_provider(&body.provider, "") {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    };
    if resolved.endpoints.device_code_url.is_none() {
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
    match request_device_code(&http, &resolved.endpoints).await {
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
    let resolved = match resolve_oauth_provider(&body.provider, "") {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    };
    let provider = resolved.provider;
    if resolved.endpoints.device_code_url.is_none() {
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
    let outcome = match poll_device_code(
        &http,
        &resolved.endpoints,
        &body.device_code,
        resolved.client_secret,
    )
    .await
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
                auth_mode: resolved.auth_mode,
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
            if let Err((status, msg)) = save_tokens_and_publish_lifecycle(
                state.token_store.as_ref(),
                state.auth_lease.as_ref(),
                &connection_ref,
                &tokens,
            )
            .await
            {
                return (status, Json(serde_json::json!({ "error": msg }))).into_response();
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
    let snapshot = state
        .auth_lease
        .snapshot(&LeaseKey::from_connection_ref(&connection_ref));
    let projection =
        meerkat_core::project_published_auth_status(chrono::Utc::now(), stored.as_ref(), &snapshot);
    let tokens = projection.tokens;
    (
        StatusCode::OK,
        Json(WireAuthStatusDetail {
            identity: WireBindingIdentity::from(&connection_ref),
            profile_id: auth_profile.id.clone(),
            provider: auth_profile.provider.as_str().to_string(),
            auth_method: auth_profile.auth_method.clone(),
            state: projection.phase.as_public_str().to_string(),
            expires_at: projection.expires_at.map(|e| e.to_rfc3339()),
            last_refresh_at: tokens.and_then(|t| t.last_refresh.map(|e| e.to_rfc3339())),
            account_id: tokens.and_then(|t| t.account_id.clone()),
            has_refresh_token: tokens.map(|t| t.refresh_token.is_some()).unwrap_or(false),
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
    if let Err((status, msg)) = clear_tokens_and_publish_lifecycle(
        state.token_store.as_ref(),
        state.auth_lease.as_ref(),
        &connection_ref,
    )
    .await
    {
        return (status, Json(serde_json::json!({ "error": msg }))).into_response();
    }
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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use meerkat_core::handles::{
        AuthLeaseHandle, AuthLeasePhase, AuthLeaseSnapshot, DslTransitionError, LeaseKey,
    };
    use meerkat_providers::auth_store::EphemeralTokenStore;
    use meerkat_runtime::RuntimeAuthLeaseHandle;

    fn managed_connection_ref() -> ConnectionRef {
        ConnectionRef {
            realm: RealmId::parse("rest-test").unwrap(),
            binding: BindingId::parse("managed").unwrap(),
            profile: None,
        }
    }

    fn api_key_tokens() -> PersistedTokens {
        api_key_tokens_with_secret("sk-test")
    }

    fn api_key_tokens_with_secret(secret: &str) -> PersistedTokens {
        PersistedTokens {
            auth_mode: PersistedAuthMode::ApiKey,
            primary_secret: Some(secret.to_string()),
            refresh_token: None,
            id_token: None,
            expires_at: None,
            last_refresh: Some(chrono::Utc::now()),
            scopes: Vec::new(),
            account_id: None,
            metadata: serde_json::Value::Null,
        }
    }

    struct RejectingAuthLeaseHandle;

    impl AuthLeaseHandle for RejectingAuthLeaseHandle {
        fn acquire_lease(
            &self,
            _lease_key: &LeaseKey,
            _expires_at: u64,
        ) -> Result<(), DslTransitionError> {
            Err(DslTransitionError::guard_rejected(
                "acquire_lease",
                "test rejection",
            ))
        }

        fn mark_expiring(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
            Ok(())
        }

        fn begin_refresh(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
            Ok(())
        }

        fn complete_refresh(
            &self,
            _lease_key: &LeaseKey,
            _new_expires_at: u64,
            _now: u64,
        ) -> Result<(), DslTransitionError> {
            Ok(())
        }

        fn refresh_failed(
            &self,
            _lease_key: &LeaseKey,
            _permanent: bool,
        ) -> Result<(), DslTransitionError> {
            Ok(())
        }

        fn mark_reauth_required(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
            Ok(())
        }

        fn release_lease(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
            Ok(())
        }

        fn snapshot(&self, _lease_key: &LeaseKey) -> AuthLeaseSnapshot {
            AuthLeaseSnapshot {
                phase: None,
                expires_at: None,
            }
        }
    }

    struct ReleaseRejectingAuthLeaseHandle;

    impl AuthLeaseHandle for ReleaseRejectingAuthLeaseHandle {
        fn acquire_lease(
            &self,
            _lease_key: &LeaseKey,
            _expires_at: u64,
        ) -> Result<(), DslTransitionError> {
            Ok(())
        }

        fn mark_expiring(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
            Ok(())
        }

        fn begin_refresh(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
            Ok(())
        }

        fn complete_refresh(
            &self,
            _lease_key: &LeaseKey,
            _new_expires_at: u64,
            _now: u64,
        ) -> Result<(), DslTransitionError> {
            Ok(())
        }

        fn refresh_failed(
            &self,
            _lease_key: &LeaseKey,
            _permanent: bool,
        ) -> Result<(), DslTransitionError> {
            Ok(())
        }

        fn mark_reauth_required(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
            Ok(())
        }

        fn release_lease(&self, _lease_key: &LeaseKey) -> Result<(), DslTransitionError> {
            Err(DslTransitionError::guard_rejected(
                "release_lease",
                "test rejection",
            ))
        }

        fn snapshot(&self, _lease_key: &LeaseKey) -> AuthLeaseSnapshot {
            AuthLeaseSnapshot {
                phase: Some(AuthLeasePhase::Valid),
                expires_at: Some(1_800_000_000),
            }
        }
    }

    fn config_with_openai_managed_store_binding() -> meerkat_core::Config {
        let mut config = meerkat_core::Config::default();
        let mut section = meerkat_core::RealmConfigSection::default();
        section.backend.insert(
            "openai_backend".into(),
            meerkat_core::BackendProfileConfig {
                provider: "openai".into(),
                backend_kind: "openai_api".into(),
                base_url: None,
                options: serde_json::json!({}),
            },
        );
        section.auth.insert(
            "openai_managed".into(),
            meerkat_core::AuthProfileConfig {
                provider: "openai".into(),
                auth_method: "api_key".into(),
                source: CredentialSourceSpec::ManagedStore,
                constraints: Default::default(),
                metadata_defaults: Default::default(),
            },
        );
        section.binding.insert(
            "default_openai".into(),
            meerkat_core::ProviderBindingConfig {
                backend_profile: "openai_backend".into(),
                auth_profile: "openai_managed".into(),
                default_model: None,
                policy: Default::default(),
            },
        );
        section.default_binding = Some("default_openai".into());
        config.realm.insert("dev".into(), section);
        config
    }

    async fn auth_status_detail(response: impl IntoResponse) -> WireAuthStatusDetail {
        let response = response.into_response();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    fn install_ephemeral_auth_state(state: &mut AppState) {
        let auth_lease: Arc<dyn AuthLeaseHandle> = Arc::new(RuntimeAuthLeaseHandle::new());
        state
            .runtime_adapter
            .set_auth_lease_handle(Arc::clone(&auth_lease));
        state.auth_lease = auth_lease;
        state.token_store = Arc::new(EphemeralTokenStore::new());
    }

    #[tokio::test]
    async fn rest_token_write_helpers_publish_auth_machine_lifecycle() {
        let store = EphemeralTokenStore::new();
        let auth_lease = RuntimeAuthLeaseHandle::new();
        let connection_ref = managed_connection_ref();
        let key = TokenKey::from_connection_ref(&connection_ref);
        let tokens = api_key_tokens();

        save_tokens_and_publish_lifecycle(&store, &auth_lease, &connection_ref, &tokens)
            .await
            .unwrap();

        assert!(store.load(&key).await.unwrap().is_some());
        let snapshot = auth_lease.snapshot(&LeaseKey::from_connection_ref(&connection_ref));
        assert_eq!(snapshot.phase, Some(AuthLeasePhase::Valid));

        clear_tokens_and_publish_lifecycle(&store, &auth_lease, &connection_ref)
            .await
            .unwrap();

        assert!(store.load(&key).await.unwrap().is_none());
        let snapshot = auth_lease.snapshot(&LeaseKey::from_connection_ref(&connection_ref));
        assert_eq!(snapshot.phase, None);
        assert_eq!(snapshot.expires_at, None);
    }

    #[tokio::test]
    async fn rest_token_save_lifecycle_failure_restores_existing_credentials() {
        let store = EphemeralTokenStore::new();
        let auth_lease = RejectingAuthLeaseHandle;
        let connection_ref = managed_connection_ref();
        let key = TokenKey::from_connection_ref(&connection_ref);
        let previous = api_key_tokens_with_secret("sk-old");
        let replacement = api_key_tokens_with_secret("sk-new");
        store.save(&key, &previous).await.unwrap();

        let err =
            save_tokens_and_publish_lifecycle(&store, &auth_lease, &connection_ref, &replacement)
                .await
                .unwrap_err();

        assert_eq!(err.0, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(err.1.contains("AuthMachine lifecycle acquire failed"));
        let stored = store.load(&key).await.unwrap().unwrap();
        assert_eq!(stored.primary_secret.as_deref(), Some("sk-old"));
    }

    #[tokio::test]
    async fn rest_token_clear_release_failure_keeps_credentials() {
        let store = EphemeralTokenStore::new();
        let auth_lease = ReleaseRejectingAuthLeaseHandle;
        let connection_ref = managed_connection_ref();
        let key = TokenKey::from_connection_ref(&connection_ref);
        store.save(&key, &api_key_tokens()).await.unwrap();

        let err = clear_tokens_and_publish_lifecycle(&store, &auth_lease, &connection_ref)
            .await
            .unwrap_err();

        assert_eq!(err.0, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(err.1.contains("AuthMachine lifecycle release failed"));
        assert!(
            store.load(&key).await.unwrap().is_some(),
            "token clear must not commit when AuthMachine release fails"
        );
    }

    #[tokio::test]
    async fn rest_auth_status_hides_stale_token_without_auth_machine_lifecycle() {
        let temp = tempfile::tempdir().unwrap();
        let mut state = AppState::load_from(temp.path().to_path_buf())
            .await
            .unwrap();
        install_ephemeral_auth_state(&mut state);
        state
            .config_runtime
            .set(config_with_openai_managed_store_binding(), None)
            .await
            .unwrap();
        let connection_ref = ConnectionRef {
            realm: RealmId::parse("dev").unwrap(),
            binding: BindingId::parse("default_openai").unwrap(),
            profile: None,
        };
        state
            .token_store
            .save(
                &TokenKey::from_connection_ref(&connection_ref),
                &PersistedTokens {
                    auth_mode: PersistedAuthMode::ApiKey,
                    primary_secret: Some("sk-stale".into()),
                    refresh_token: Some("refresh-stale".into()),
                    id_token: None,
                    expires_at: Some(chrono::DateTime::from_timestamp(1_800_000_000, 0).unwrap()),
                    last_refresh: Some(chrono::DateTime::from_timestamp(1_700_000_000, 0).unwrap()),
                    scopes: Vec::new(),
                    account_id: Some("acct-stale".into()),
                    metadata: serde_json::Value::Null,
                },
            )
            .await
            .unwrap();

        let detail = auth_status_detail(
            get_auth_status(
                State(state),
                Path(BindingId::parse("default_openai").unwrap()),
                Query(RealmQuery {
                    realm_id: RealmId::parse("dev").unwrap(),
                    profile_id: None,
                }),
            )
            .await,
        )
        .await;

        assert_eq!(detail.state, "unknown");
        assert_eq!(detail.expires_at, None);
        assert_eq!(detail.last_refresh_at, None);
        assert_eq!(detail.account_id, None);
        assert!(!detail.has_refresh_token);
    }

    #[tokio::test]
    async fn rest_auth_status_does_not_report_valid_when_token_is_missing() {
        let temp = tempfile::tempdir().unwrap();
        let mut state = AppState::load_from(temp.path().to_path_buf())
            .await
            .unwrap();
        install_ephemeral_auth_state(&mut state);
        state
            .config_runtime
            .set(config_with_openai_managed_store_binding(), None)
            .await
            .unwrap();
        let connection_ref = ConnectionRef {
            realm: RealmId::parse("dev").unwrap(),
            binding: BindingId::parse("default_openai").unwrap(),
            profile: None,
        };
        let lease_key = LeaseKey::from_connection_ref(&connection_ref);
        let now = chrono::Utc::now().timestamp() as u64;
        let bindings = state
            .runtime_adapter
            .prepare_bindings(meerkat_core::SessionId::new())
            .await
            .unwrap();
        bindings
            .auth_lease
            .acquire_lease(&lease_key, now + 3600)
            .unwrap();

        let detail = auth_status_detail(
            get_auth_status(
                State(state),
                Path(BindingId::parse("default_openai").unwrap()),
                Query(RealmQuery {
                    realm_id: RealmId::parse("dev").unwrap(),
                    profile_id: None,
                }),
            )
            .await,
        )
        .await;

        assert_eq!(detail.state, "unknown");
        assert_eq!(detail.expires_at, None);
        assert!(!detail.has_refresh_token);
    }

    #[tokio::test]
    async fn rest_auth_status_observes_session_runtime_binding_lifecycle() {
        let temp = tempfile::tempdir().unwrap();
        let mut state = AppState::load_from(temp.path().to_path_buf())
            .await
            .unwrap();
        install_ephemeral_auth_state(&mut state);
        state
            .config_runtime
            .set(config_with_openai_managed_store_binding(), None)
            .await
            .unwrap();
        let connection_ref = ConnectionRef {
            realm: RealmId::parse("dev").unwrap(),
            binding: BindingId::parse("default_openai").unwrap(),
            profile: None,
        };
        let lease_key = LeaseKey::from_connection_ref(&connection_ref);
        let now = chrono::Utc::now().timestamp() as u64;
        let query = || RealmQuery {
            realm_id: RealmId::parse("dev").unwrap(),
            profile_id: None,
        };
        let binding_id = || BindingId::parse("default_openai").unwrap();
        state
            .token_store
            .save(
                &TokenKey::from_connection_ref(&connection_ref),
                &PersistedTokens::api_key("sk-live"),
            )
            .await
            .unwrap();

        let bindings = state
            .runtime_adapter
            .prepare_bindings(meerkat_core::SessionId::new())
            .await
            .unwrap();
        bindings
            .auth_lease
            .acquire_lease(&lease_key, now + 3600)
            .unwrap();
        let detail = auth_status_detail(
            get_auth_status(State(state.clone()), Path(binding_id()), Query(query())).await,
        )
        .await;
        assert_eq!(detail.state, "valid");

        let bindings = state
            .runtime_adapter
            .prepare_bindings(meerkat_core::SessionId::new())
            .await
            .unwrap();
        bindings.auth_lease.begin_refresh(&lease_key).unwrap();
        let detail = auth_status_detail(
            get_auth_status(State(state.clone()), Path(binding_id()), Query(query())).await,
        )
        .await;
        assert_eq!(detail.state, "expiring");

        bindings
            .auth_lease
            .complete_refresh(&lease_key, now + 7200, now)
            .unwrap();
        bindings
            .auth_lease
            .mark_reauth_required(&lease_key)
            .unwrap();
        let detail = auth_status_detail(
            get_auth_status(State(state), Path(binding_id()), Query(query())).await,
        )
        .await;
        assert_eq!(detail.state, "reauth_required");
    }

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
