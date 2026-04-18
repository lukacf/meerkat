//! REST endpoints for `/auth/*` + `/realms*`.
//!
//! Read-side endpoints resolve against the active `Config.realm` map.
//! Write-side endpoints (login start / complete, profile delete,
//! logout) use the shared `AppState.token_store` so persisted
//! credentials are visible to the AgentFactory's `resolve_binding`
//! path.
//!
//! The OAuth PKCE verifier is held client-side (returned in
//! /login/start and passed back in /login/complete) so the server
//! doesn't need a session map.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;

use meerkat_client::auth_oauth::{
    DevicePollOutcome, OAuthEndpoints, OAuthError, PkcePair, exchange_authorization_code,
    poll_device_code, request_device_code,
};
use meerkat_client::auth_store::{PersistedAuthMode, PersistedTokens, TokenKey};
use meerkat_client::providers::anthropic::oauth as a_oauth;
use meerkat_client::providers::google::oauth as g_oauth;
use meerkat_client::providers::openai::oauth as o_oauth;
use meerkat_contracts::{
    WireAuthProfile, WireBackendProfile, WireProviderBinding, WireRealmConnectionSet,
};
use meerkat_core::RealmConnectionSet;

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
    realm_id: &str,
) -> Result<RealmConnectionSet, (StatusCode, String)> {
    let config = load_config(state).await?;
    let section = config
        .realm
        .get(realm_id)
        .ok_or_else(|| (StatusCode::NOT_FOUND, format!("Unknown realm: {realm_id}")))?;
    RealmConnectionSet::from_config(realm_id, section).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Realm config invalid: {e}"),
        )
    })
}

// --- Realm endpoints -------------------------------------------------

pub async fn list_realms(State(state): State<AppState>) -> impl IntoResponse {
    match load_config(&state).await {
        Ok(config) => {
            let realms: Vec<serde_json::Value> = config
                .realm
                .iter()
                .map(|(realm_id, section)| {
                    serde_json::json!({
                        "realm_id": realm_id,
                        "default_binding": section.default_binding,
                        "backend_count": section.backend.len(),
                        "auth_profile_count": section.auth.len(),
                        "binding_count": section.binding.len(),
                    })
                })
                .collect();
            (
                StatusCode::OK,
                Json(serde_json::json!({ "realms": realms })),
            )
                .into_response()
        }
        Err((status, msg)) => (status, Json(serde_json::json!({ "error": msg }))).into_response(),
    }
}

pub async fn get_realm(
    State(state): State<AppState>,
    Path(realm_id): Path<String>,
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
    pub realm_id: String,
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
                Json(serde_json::json!({
                    "realm_id": realm.realm_id,
                    "auth_profiles": profiles,
                    "backend_profiles": backends,
                    "bindings": bindings,
                })),
            )
                .into_response()
        }
        Err((status, msg)) => (status, Json(serde_json::json!({ "error": msg }))).into_response(),
    }
}

#[derive(serde::Deserialize)]
pub struct CreateAuthProfileBody {
    pub realm_id: String,
    pub profile_id: String,
    pub provider: String,
    pub auth_method: String,
    pub secret: String,
}

/// Create an `api_key`-backed auth profile entry by writing the secret
/// into the TokenStore under `(realm_id, profile_id)`. The caller is
/// expected to have already declared the corresponding
/// `[realm.<id>.auth.<profile>]` entry in the config with
/// `source.kind = "managed_store"` so `resolve_binding` picks it up;
/// for `inline_secret` sources the TOML itself carries the secret and
/// this endpoint is a no-op (hence its 400 below).
pub async fn create_auth_profile(
    State(state): State<AppState>,
    Json(body): Json<CreateAuthProfileBody>,
) -> impl IntoResponse {
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
    let key = TokenKey::new(body.realm_id.clone(), body.profile_id.clone());
    match state.token_store.save(&key, &tokens).await {
        Ok(()) => (
            StatusCode::CREATED,
            Json(serde_json::json!({
                "realm_id": body.realm_id,
                "profile_id": body.profile_id,
                "provider": body.provider,
                "auth_method": body.auth_method,
                "stored": true,
            })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("TokenStore save failed: {e}") })),
        )
            .into_response(),
    }
}

pub async fn get_auth_profile(
    State(state): State<AppState>,
    Path(profile_id): Path<String>,
    Query(query): Query<RealmQuery>,
) -> impl IntoResponse {
    match resolve_realm(&state, &query.realm_id).await {
        Ok(realm) => match realm.auth_profiles.get(&profile_id) {
            Some(p) => (
                StatusCode::OK,
                Json(serde_json::to_value(WireAuthProfile::from(p)).unwrap_or_default()),
            )
                .into_response(),
            None => (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "error": format!(
                        "Auth profile {}:{} not found",
                        query.realm_id, profile_id,
                    ),
                })),
            )
                .into_response(),
        },
        Err((status, msg)) => (status, Json(serde_json::json!({ "error": msg }))).into_response(),
    }
}

pub async fn delete_auth_profile(
    State(state): State<AppState>,
    Path(profile_id): Path<String>,
    Query(query): Query<RealmQuery>,
) -> impl IntoResponse {
    let key = TokenKey::new(query.realm_id.clone(), profile_id.clone());
    match state.token_store.clear(&key).await {
        Ok(()) => (
            StatusCode::NO_CONTENT,
            Json(serde_json::json!({
                "realm_id": query.realm_id,
                "profile_id": profile_id,
                "cleared": true,
            })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("TokenStore clear failed: {e}") })),
        )
            .into_response(),
    }
}

#[derive(serde::Deserialize)]
pub struct TestBindingBody {
    pub realm_id: String,
    pub binding_id: String,
}

pub async fn test_auth_profile(
    State(state): State<AppState>,
    Path(_profile_id): Path<String>,
    Json(body): Json<TestBindingBody>,
) -> impl IntoResponse {
    match resolve_realm(&state, &body.realm_id).await {
        Ok(realm) => {
            let registry = meerkat_client::ProviderRuntimeRegistry::default();
            let env = meerkat_client::ResolverEnvironment::with_process_env()
                .with_token_store(Arc::clone(&state.token_store));
            match registry.resolve(&realm, &body.binding_id, &env).await {
                Ok(conn) => (
                    StatusCode::OK,
                    Json(serde_json::json!({
                        "state": "valid",
                        "provider": conn.provider.as_str(),
                        "backend_profile_id": conn.backend_profile.id,
                        "has_credential": conn.resolved_secret().is_some()
                            || conn.resolved_authorizer().is_some(),
                    })),
                )
                    .into_response(),
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
// The PKCE verifier + state are returned to the client in /login/start;
// the client holds them and posts them back in /login/complete along
// with the authorization code it received from the provider.

fn provider_endpoints(
    provider: &str,
    redirect_uri: &str,
) -> Result<(OAuthEndpoints, PersistedAuthMode, Option<&'static str>), (StatusCode, String)> {
    match provider {
        "anthropic" | "claude" | "claude.ai" => Ok((
            a_oauth::claude_ai_endpoints(redirect_uri),
            PersistedAuthMode::ClaudeAiOauth,
            None,
        )),
        "openai" | "chatgpt" => Ok((
            o_oauth::chatgpt_endpoints(redirect_uri),
            PersistedAuthMode::ChatgptOauth,
            None,
        )),
        "google" | "gemini" | "code_assist" => Ok((
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
    let (endpoints, _mode, _secret) = match provider_endpoints(&body.provider, &body.redirect_uri) {
        Ok(v) => v,
        Err((status, msg)) => {
            return (status, Json(serde_json::json!({ "error": msg }))).into_response();
        }
    };
    let pkce = PkcePair::generate_s256();
    let state_token = format!(
        "st-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
    );
    let authorize_url = endpoints.authorize_url_with_pkce(&pkce.challenge, &state_token);
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "authorize_url": authorize_url,
            "state": state_token,
            "pkce_verifier": pkce.verifier.secret(),
            "pkce_challenge": pkce.challenge.code,
            "redirect_uri": body.redirect_uri,
            "provider": body.provider,
        })),
    )
        .into_response()
}

#[derive(serde::Deserialize)]
pub struct LoginCompleteBody {
    pub provider: String,
    pub code: String,
    pub pkce_verifier: String,
    pub redirect_uri: String,
    #[serde(default = "default_realm")]
    pub realm_id: String,
    pub profile_id: Option<String>,
}

fn default_realm() -> String {
    "dev".to_string()
}

pub async fn complete_login(
    State(state): State<AppState>,
    Json(body): Json<LoginCompleteBody>,
) -> impl IntoResponse {
    let (endpoints, mode, client_secret) =
        match provider_endpoints(&body.provider, &body.redirect_uri) {
            Ok(v) => v,
            Err((status, msg)) => {
                return (status, Json(serde_json::json!({ "error": msg }))).into_response();
            }
        };
    let default_binding = match mode {
        PersistedAuthMode::ClaudeAiOauth => "anthropic_oauth",
        PersistedAuthMode::ChatgptOauth => "openai_oauth",
        PersistedAuthMode::GoogleOauth => "google_oauth",
        _ => "oauth_profile",
    };
    let profile_id = body
        .profile_id
        .unwrap_or_else(|| default_binding.to_string());

    let http = reqwest::Client::new();
    let result = match exchange_authorization_code(
        &http,
        &endpoints,
        &body.code,
        &body.pkce_verifier,
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

    let key = TokenKey::new(body.realm_id.clone(), profile_id.clone());
    if let Err(e) = state.token_store.save(&key, &tokens).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("TokenStore save failed: {e}") })),
        )
            .into_response();
    }

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "realm_id": body.realm_id,
            "profile_id": profile_id,
            "provider": body.provider,
            "expires_at": expires_at.map(|e| e.to_rfc3339()),
            "has_refresh_token": tokens.refresh_token.is_some(),
            "scopes": tokens.scopes,
        })),
    )
        .into_response()
}

#[derive(serde::Deserialize)]
pub struct DeviceStartBody {
    pub provider: String,
}

pub async fn start_device_login(Json(body): Json<DeviceStartBody>) -> impl IntoResponse {
    let (endpoints, _mode, _secret) = match provider_endpoints(&body.provider, "") {
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
            Json(serde_json::json!({
                "device_code": resp.device_code,
                "user_code": resp.user_code,
                "verification_uri": resp.verification_uri,
                "verification_uri_complete": resp.verification_uri_complete,
                "expires_in": resp.expires_in,
                "interval": resp.interval,
                "provider": body.provider,
            })),
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
    #[serde(default = "default_dev_realm_rest")]
    pub realm_id: String,
    #[serde(default)]
    pub profile_id: Option<String>,
}

fn default_dev_realm_rest() -> String {
    "dev".into()
}

/// Device-code flow completion leg. Single-poll semantics — the caller
/// runs the outer retry loop using the `interval` returned from
/// `POST /auth/login/device/start`. Returns:
///   * `{ "state": "pending" }`     (202)
///   * `{ "state": "slow_down" }`   (429)
///   * `{ "state": "access_denied" }` / `{ "state": "expired" }` (400)
///   * `{ "state": "ready", realm_id, profile_id, ... }` (200, tokens
///     persisted to TokenStore under `<realm_id>:<profile_id>`).
pub async fn complete_device_login(
    State(state): State<AppState>,
    Json(body): Json<DeviceCompleteBody>,
) -> impl IntoResponse {
    let (endpoints, mode, client_secret) = match provider_endpoints(&body.provider, "") {
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
    let default_binding = match mode {
        PersistedAuthMode::ClaudeAiOauth => "anthropic_oauth",
        PersistedAuthMode::ChatgptOauth => "openai_oauth",
        PersistedAuthMode::GoogleOauth => "google_oauth",
        _ => "oauth_profile",
    };
    let profile_id = body
        .profile_id
        .unwrap_or_else(|| default_binding.to_string());
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
            let key = TokenKey::new(body.realm_id.clone(), profile_id.clone());
            if let Err(e) = state.token_store.save(&key, &tokens).await {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({
                        "error": format!("TokenStore save failed: {e}"),
                    })),
                )
                    .into_response();
            }
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "state": "ready",
                    "realm_id": body.realm_id,
                    "profile_id": profile_id,
                    "provider": body.provider,
                    "expires_at": expires_at.map(|e| e.to_rfc3339()),
                    "has_refresh_token": tokens.refresh_token.is_some(),
                    "scopes": tokens.scopes,
                })),
            )
                .into_response()
        }
    }
}

pub async fn get_auth_status(
    State(state): State<AppState>,
    Path(profile_id): Path<String>,
    Query(query): Query<RealmQuery>,
) -> impl IntoResponse {
    // Check both the config's profile definition (if any) and the
    // TokenStore entry (if any) so we can report a real state.
    let config = match load_config(&state).await {
        Ok(c) => c,
        Err((status, msg)) => {
            return (status, Json(serde_json::json!({ "error": msg }))).into_response();
        }
    };
    let key = TokenKey::new(query.realm_id.clone(), profile_id.clone());
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
    let profile = config
        .realm
        .get(&query.realm_id)
        .and_then(|section| section.auth.get(&profile_id));
    let state_label = match &stored {
        Some(t) => {
            if let Some(expiry) = t.expires_at {
                if expiry - chrono::Utc::now() < chrono::Duration::zero() {
                    "expired"
                } else if expiry - chrono::Utc::now() < chrono::Duration::seconds(60) {
                    "expiring"
                } else {
                    "valid"
                }
            } else {
                "valid"
            }
        }
        None => "unknown",
    };
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "profile_id": profile_id,
            "realm_id": query.realm_id,
            "provider": profile.map(|p| p.provider.clone()),
            "auth_method": profile.map(|p| p.auth_method.clone()),
            "state": state_label,
            "expires_at": stored.as_ref().and_then(|t| t.expires_at.map(|e| e.to_rfc3339())),
            "last_refresh_at": stored.as_ref().and_then(|t| t.last_refresh.map(|e| e.to_rfc3339())),
            "account_id": stored.as_ref().and_then(|t| t.account_id.clone()),
            "has_refresh_token": stored.as_ref().map(|t| t.refresh_token.is_some()).unwrap_or(false),
        })),
    )
        .into_response()
}

pub async fn logout(
    State(state): State<AppState>,
    Path(profile_id): Path<String>,
    Query(query): Query<RealmQuery>,
) -> impl IntoResponse {
    let key = TokenKey::new(query.realm_id.clone(), profile_id.clone());
    match state.token_store.clear(&key).await {
        Ok(()) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "realm_id": query.realm_id,
                "profile_id": profile_id,
                "cleared": true,
            })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("TokenStore clear failed: {e}") })),
        )
            .into_response(),
    }
}
