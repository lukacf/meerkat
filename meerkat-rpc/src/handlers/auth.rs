//! `auth/*` + `realm/*` method handlers.
//!
//! Real implementations using the shared `SessionRuntime.token_store()`.
//! OAuth login is split across two calls (server keeps state -> PKCE verifier):
//!
//!   auth/login/start     → returns authorize_url + state
//!   auth/login/complete  → verifies state, exchanges code → persists

use std::sync::Arc;

use serde_json::value::RawValue;

use meerkat_anthropic::runtime::oauth as a_oauth;
use meerkat_contracts::{
    WireAuthProfile, WireBackendProfile, WireProviderBinding, WireRealmConnectionSet,
};
use meerkat_core::{ConnectionRef, RealmConnectionSet};
use meerkat_gemini::runtime::oauth as g_oauth;
use meerkat_openai::runtime::oauth as o_oauth;
use meerkat_providers::auth_oauth::{
    DevicePollOutcome, OAuthEndpoints, OAuthError, PkcePair, exchange_authorization_code,
    poll_device_code, request_device_code,
};
use meerkat_providers::auth_store::{PersistedAuthMode, PersistedTokens, TokenKey};
use meerkat_providers::oauth_flow::{OAuthFlowError, global_oauth_flow_registry};

use super::{RpcResponseExt, parse_params};
use crate::error;
use crate::protocol::{RpcId, RpcResponse};
use crate::session_runtime::SessionRuntime;

#[derive(serde::Deserialize)]
struct RealmIdParams {
    realm_id: String,
}

#[derive(serde::Deserialize)]
struct BindingIdParams {
    realm_id: String,
    binding_id: String,
}

async fn load_config(runtime: &SessionRuntime) -> Result<meerkat_core::Config, RpcResponse> {
    if let Some(cfg_runtime) = runtime.config_runtime() {
        cfg_runtime
            .get()
            .await
            .map(|snap| snap.config)
            .map_err(|e| {
                RpcResponse::error(
                    None,
                    error::INTERNAL_ERROR,
                    format!("Failed to load config: {e}"),
                )
            })
    } else {
        Ok(meerkat_core::Config::default())
    }
}

async fn resolve_realm(
    runtime: &SessionRuntime,
    realm_id: &str,
) -> Result<RealmConnectionSet, RpcResponse> {
    let config = load_config(runtime).await?;
    let section = config.realm.get(realm_id).ok_or_else(|| {
        RpcResponse::error(
            None,
            error::INVALID_PARAMS,
            format!("Unknown realm: {realm_id}"),
        )
    })?;
    RealmConnectionSet::from_config(realm_id, section).map_err(|e| {
        RpcResponse::error(
            None,
            error::INTERNAL_ERROR,
            format!("Realm config invalid: {e}"),
        )
    })
}

async fn resolve_binding_identity(
    runtime: &SessionRuntime,
    realm_id: &str,
    binding_id: &str,
) -> Result<
    (
        ConnectionRef,
        meerkat_core::ProviderBinding,
        meerkat_core::AuthProfile,
    ),
    RpcResponse,
> {
    let realm = resolve_realm(runtime, realm_id).await?;
    let resolved_realm_id = realm.realm_id.clone();
    let (binding, _, auth_profile) = realm.lookup_binding(binding_id).map_err(|e| {
        RpcResponse::error(
            None,
            error::INVALID_PARAMS,
            format!("Unknown binding {realm_id}:{binding_id}: {e}"),
        )
    })?;
    let realm_typed = meerkat_core::connection::RealmId::parse(resolved_realm_id).map_err(|e| {
        RpcResponse::error(
            None,
            error::INVALID_PARAMS,
            format!("Invalid realm id {realm_id}: {e}"),
        )
    })?;
    let binding_typed =
        meerkat_core::connection::BindingId::parse(binding.id.clone()).map_err(|e| {
            RpcResponse::error(
                None,
                error::INVALID_PARAMS,
                format!("Invalid binding id {}: {e}", binding.id),
            )
        })?;
    Ok((
        ConnectionRef {
            realm: realm_typed,
            binding: binding_typed,
            profile: None,
        },
        binding.clone(),
        auth_profile.clone(),
    ))
}

#[allow(clippy::result_large_err)]
fn require_token_store(
    runtime: &SessionRuntime,
    id: Option<RpcId>,
) -> Result<Arc<dyn meerkat_providers::auth_store::TokenStore>, RpcResponse> {
    runtime.token_store().ok_or_else(|| {
        RpcResponse::error(
            id.clone(),
            error::INTERNAL_ERROR,
            "TokenStore not configured for this runtime",
        )
    })
}

fn provider_endpoints(
    provider: &str,
    redirect_uri: &str,
) -> Result<(OAuthEndpoints, PersistedAuthMode, Option<&'static str>), String> {
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
        other => Err(format!(
            "Unknown provider '{other}'. Supported: anthropic, openai, google."
        )),
    }
}

// --- Realm projection -------------------------------------------------

pub async fn handle_realm_list(id: Option<RpcId>, runtime: &SessionRuntime) -> RpcResponse {
    let config = match load_config(runtime).await {
        Ok(c) => c,
        Err(r) => return r.with_id(id),
    };
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
    RpcResponse::success(id, serde_json::json!({ "realms": realms }))
}

pub async fn handle_realm_get(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: &SessionRuntime,
) -> RpcResponse {
    let parsed: RealmIdParams = match parse_params(params) {
        Ok(v) => v,
        Err(r) => return r.with_id(id),
    };
    let realm = match resolve_realm(runtime, &parsed.realm_id).await {
        Ok(r) => r,
        Err(r) => return r.with_id(id),
    };
    let wire = WireRealmConnectionSet::from(&realm);
    match serde_json::to_value(wire) {
        Ok(v) => RpcResponse::success(id, v),
        Err(e) => RpcResponse::error(id, error::INTERNAL_ERROR, format!("Serialize error: {e}")),
    }
}

// --- Auth profile CRUD ------------------------------------------------

pub async fn handle_auth_profile_list(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: &SessionRuntime,
) -> RpcResponse {
    let parsed: RealmIdParams = match parse_params(params) {
        Ok(v) => v,
        Err(r) => return r.with_id(id),
    };
    let realm = match resolve_realm(runtime, &parsed.realm_id).await {
        Ok(r) => r,
        Err(r) => return r.with_id(id),
    };
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
    RpcResponse::success(
        id,
        serde_json::json!({
            "realm_id": realm.realm_id,
            "auth_profiles": profiles,
            "backend_profiles": backends,
            "bindings": bindings,
        }),
    )
}

pub async fn handle_auth_profile_get(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: &SessionRuntime,
) -> RpcResponse {
    let parsed: BindingIdParams = match parse_params(params) {
        Ok(v) => v,
        Err(r) => return r.with_id(id),
    };
    match resolve_binding_identity(runtime, &parsed.realm_id, &parsed.binding_id).await {
        Ok((connection_ref, binding, auth_profile)) => RpcResponse::success(
            id,
            serde_json::json!({
                "connection_ref": &connection_ref,
                "binding_id": &binding.id,
                "profile_id": &binding.auth_profile,
                "auth_profile": WireAuthProfile::from(&auth_profile),
            }),
        ),
        Err(r) => r.with_id(id),
    }
}

#[derive(serde::Deserialize)]
struct CreateProfileParams {
    realm_id: String,
    binding_id: String,
    auth_method: String,
    secret: String,
}

pub async fn handle_auth_profile_create(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: &SessionRuntime,
) -> RpcResponse {
    let parsed: CreateProfileParams = match parse_params(params) {
        Ok(v) => v,
        Err(r) => return r.with_id(id),
    };
    let auth_mode = match parsed.auth_method.as_str() {
        "api_key" => PersistedAuthMode::ApiKey,
        "static_bearer" => PersistedAuthMode::StaticBearer,
        other => {
            return RpcResponse::error(
                id,
                error::INVALID_PARAMS,
                format!(
                    "auth_method '{other}' cannot be created via RPC. \
                     OAuth methods use auth/login/start + auth/login/complete; \
                     managed_store and external_resolver are configured via TOML."
                ),
            );
        }
    };
    let (connection_ref, binding, auth_profile) =
        match resolve_binding_identity(runtime, &parsed.realm_id, &parsed.binding_id).await {
            Ok(v) => v,
            Err(r) => return r.with_id(id),
        };
    if parsed.auth_method != auth_profile.auth_method {
        return RpcResponse::error(
            id,
            error::INVALID_PARAMS,
            format!(
                "binding {} resolves auth_method '{}' not '{}'",
                parsed.binding_id, auth_profile.auth_method, parsed.auth_method,
            ),
        );
    }
    let store = match require_token_store(runtime, id.clone()) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let tokens = PersistedTokens {
        auth_mode,
        primary_secret: Some(parsed.secret),
        refresh_token: None,
        id_token: None,
        expires_at: None,
        last_refresh: Some(chrono::Utc::now()),
        scopes: Vec::new(),
        account_id: None,
        metadata: serde_json::Value::Null,
    };
    let key = TokenKey::new(connection_ref.realm.clone(), connection_ref.binding.clone());
    match store.save(&key, &tokens).await {
        Ok(()) => {
            tracing::info!(
                target: "meerkat::auth::audit",
                binding_key = ?connection_ref,
                action = "create_profile",
                provider = %auth_profile.provider.as_str(),
                auth_method = %auth_profile.auth_method,
                "binding-scoped auth credentials stored via RPC"
            );
            RpcResponse::success(
                id,
                serde_json::json!({
                    "realm_id": connection_ref.realm.as_str(),
                    "binding_id": connection_ref.binding.as_str(),
                    "connection_ref": &connection_ref,
                    "profile_id": &binding.auth_profile,
                    "provider": auth_profile.provider.as_str(),
                    "auth_method": &auth_profile.auth_method,
                    "stored": true,
                }),
            )
        }
        Err(e) => RpcResponse::error(
            id,
            error::INTERNAL_ERROR,
            format!("TokenStore save failed: {e}"),
        ),
    }
}

pub async fn handle_auth_profile_delete(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: &SessionRuntime,
) -> RpcResponse {
    let parsed: BindingIdParams = match parse_params(params) {
        Ok(v) => v,
        Err(r) => return r.with_id(id),
    };
    let (connection_ref, binding, _) =
        match resolve_binding_identity(runtime, &parsed.realm_id, &parsed.binding_id).await {
            Ok(v) => v,
            Err(r) => return r.with_id(id),
        };
    let store = match require_token_store(runtime, id.clone()) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let key = TokenKey::new(connection_ref.realm.clone(), connection_ref.binding.clone());
    match store.clear(&key).await {
        Ok(()) => {
            tracing::info!(
                target: "meerkat::auth::audit",
                binding_key = ?connection_ref,
                action = "delete_profile",
                "binding-scoped auth credentials deleted via RPC"
            );
            RpcResponse::success(
                id,
                serde_json::json!({
                    "realm_id": connection_ref.realm.as_str(),
                    "binding_id": connection_ref.binding.as_str(),
                    "connection_ref": &connection_ref,
                    "profile_id": &binding.auth_profile,
                    "cleared": true,
                }),
            )
        }
        Err(e) => RpcResponse::error(
            id,
            error::INTERNAL_ERROR,
            format!("TokenStore clear failed: {e}"),
        ),
    }
}

// --- OAuth login ------------------------------------------------------

#[derive(serde::Deserialize)]
struct LoginStartParams {
    provider: String,
    redirect_uri: String,
}

pub async fn handle_auth_login_start(id: Option<RpcId>, params: Option<&RawValue>) -> RpcResponse {
    let parsed: LoginStartParams = match parse_params(params) {
        Ok(v) => v,
        Err(r) => return r.with_id(id),
    };
    let (endpoints, _mode, _secret) =
        match provider_endpoints(&parsed.provider, &parsed.redirect_uri) {
            Ok(v) => v,
            Err(msg) => {
                return RpcResponse::error(id, error::INVALID_PARAMS, msg);
            }
        };
    let pkce = PkcePair::generate_s256();
    let verifier = pkce.verifier.secret().clone();
    let state_token = match global_oauth_flow_registry().start(
        parsed.provider.clone(),
        parsed.redirect_uri.clone(),
        verifier,
    ) {
        Ok(state) => state,
        Err(OAuthFlowError::CapacityExceeded { .. }) => {
            return RpcResponse::error(
                id,
                error::INTERNAL_ERROR,
                "oauth state registry is at capacity",
            );
        }
        Err(e) => {
            return RpcResponse::error(
                id,
                error::INTERNAL_ERROR,
                format!("oauth state initialization failed: {e}"),
            );
        }
    };
    let authorize_url = endpoints.authorize_url_with_pkce(&pkce.challenge, &state_token);
    RpcResponse::success(
        id,
        serde_json::json!({
            "authorize_url": authorize_url,
            "state": state_token,
            "redirect_uri": parsed.redirect_uri,
            "provider": parsed.provider,
        }),
    )
}

#[derive(serde::Deserialize)]
struct LoginCompleteParams {
    provider: String,
    code: String,
    state: String,
    redirect_uri: String,
    realm_id: String,
    #[serde(default)]
    binding_id: Option<String>,
}

pub async fn handle_auth_login_complete(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: &SessionRuntime,
) -> RpcResponse {
    let parsed: LoginCompleteParams = match parse_params(params) {
        Ok(v) => v,
        Err(r) => return r.with_id(id),
    };
    let (endpoints, mode, client_secret) =
        match provider_endpoints(&parsed.provider, &parsed.redirect_uri) {
            Ok(v) => v,
            Err(msg) => {
                return RpcResponse::error(id, error::INVALID_PARAMS, msg);
            }
        };
    let binding_id = parsed.binding_id;
    let (connection_ref, binding, auth_profile) = match resolve_binding_identity(
        runtime,
        &parsed.realm_id,
        binding_id.as_deref().unwrap_or(""),
    )
    .await
    {
        Ok(v) => v,
        Err(r) => return r.with_id(id),
    };
    if parsed.provider != auth_profile.provider.as_str() {
        return RpcResponse::error(
            id,
            error::INVALID_PARAMS,
            format!(
                "binding {} resolves provider '{}' not '{}'",
                binding.id,
                auth_profile.provider.as_str(),
                parsed.provider,
            ),
        );
    }

    let flow = match global_oauth_flow_registry().consume(
        &parsed.state,
        &parsed.provider,
        &parsed.redirect_uri,
    ) {
        Ok(flow) => flow,
        Err(OAuthFlowError::Missing) => {
            return RpcResponse::error(
                id,
                error::INVALID_PARAMS,
                "oauth state is missing or expired",
            );
        }
        Err(e) => {
            return RpcResponse::error(
                id,
                error::INVALID_PARAMS,
                format!("oauth state verification failed: {e}"),
            );
        }
    };

    let store = match require_token_store(runtime, id.clone()) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let http = reqwest::Client::new();
    let result = match exchange_authorization_code(
        &http,
        &endpoints,
        &parsed.code,
        &flow.pkce_verifier,
        client_secret,
    )
    .await
    {
        Ok(r) => r,
        Err(OAuthError::TokenEndpoint { status, body }) => {
            return RpcResponse::error(
                id,
                error::INTERNAL_ERROR,
                format!("token endpoint returned {status}: {body}"),
            );
        }
        Err(e) => {
            return RpcResponse::error(
                id,
                error::INTERNAL_ERROR,
                format!("Token exchange failed: {e}"),
            );
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
    let key = TokenKey::new(connection_ref.realm.clone(), connection_ref.binding.clone());
    if let Err(e) = store.save(&key, &tokens).await {
        return RpcResponse::error(
            id,
            error::INTERNAL_ERROR,
            format!("TokenStore save failed: {e}"),
        );
    }
    tracing::info!(
        target: "meerkat::auth::audit",
        binding_key = ?connection_ref,
        action = "login_oauth_complete",
        provider = %parsed.provider,
        has_refresh_token = %tokens.refresh_token.is_some(),
        "OAuth login completed via RPC"
    );
    RpcResponse::success(
        id,
        serde_json::json!({
            "realm_id": connection_ref.realm.as_str(),
            "binding_id": connection_ref.binding.as_str(),
            "connection_ref": &connection_ref,
            "profile_id": &binding.auth_profile,
            "provider": parsed.provider,
            "expires_at": expires_at.map(|e| e.to_rfc3339()),
            "has_refresh_token": tokens.refresh_token.is_some(),
            "scopes": tokens.scopes,
        }),
    )
}

#[derive(serde::Deserialize)]
struct DeviceStartParams {
    provider: String,
}

pub async fn handle_auth_login_device_start(
    id: Option<RpcId>,
    params: Option<&RawValue>,
) -> RpcResponse {
    let parsed: DeviceStartParams = match parse_params(params) {
        Ok(v) => v,
        Err(r) => return r.with_id(id),
    };
    let (endpoints, _mode, _secret) = match provider_endpoints(&parsed.provider, "") {
        Ok(v) => v,
        Err(msg) => return RpcResponse::error(id, error::INVALID_PARAMS, msg),
    };
    if endpoints.device_code_url.is_none() {
        return RpcResponse::error(
            id,
            error::INVALID_PARAMS,
            format!(
                "provider '{}' does not support the device-code flow",
                parsed.provider,
            ),
        );
    }
    let http = reqwest::Client::new();
    match request_device_code(&http, &endpoints).await {
        Ok(resp) => RpcResponse::success(
            id,
            serde_json::json!({
                "device_code": resp.device_code,
                "user_code": resp.user_code,
                "verification_uri": resp.verification_uri,
                "verification_uri_complete": resp.verification_uri_complete,
                "expires_in": resp.expires_in,
                "interval": resp.interval,
                "provider": parsed.provider,
            }),
        ),
        Err(e) => RpcResponse::error(
            id,
            error::INTERNAL_ERROR,
            format!("device-code request failed: {e}"),
        ),
    }
}

#[derive(serde::Deserialize)]
struct DeviceCompleteParams {
    provider: String,
    device_code: String,
    realm_id: String,
    #[serde(default)]
    binding_id: Option<String>,
}

/// Plan §1.5r.9 device-flow completion leg. Single-poll semantics — the
/// caller runs the outer retry loop using the `interval` returned from
/// `auth.login.device_start`. Returns one of:
///   * `{ state: "pending" }`     (202-equivalent)
///   * `{ state: "slow_down" }`   (429-equivalent; bump caller's interval)
///   * `{ state: "access_denied" }` / `{ state: "expired" }` (terminal)
///   * `{ state: "ready", realm_id, binding_id, expires_at, ... }`
///     (tokens persisted to TokenStore under `<realm_id>:<binding_id>`).
pub async fn handle_auth_login_device_complete(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: &SessionRuntime,
) -> RpcResponse {
    let parsed: DeviceCompleteParams = match parse_params(params) {
        Ok(v) => v,
        Err(r) => return r.with_id(id),
    };
    let (endpoints, mode, client_secret) = match provider_endpoints(&parsed.provider, "") {
        Ok(v) => v,
        Err(msg) => return RpcResponse::error(id, error::INVALID_PARAMS, msg),
    };
    if endpoints.device_code_url.is_none() {
        return RpcResponse::error(
            id,
            error::INVALID_PARAMS,
            format!(
                "provider '{}' does not support the device-code flow",
                parsed.provider,
            ),
        );
    }
    let binding_id = parsed.binding_id;
    let (connection_ref, binding, auth_profile) = match resolve_binding_identity(
        runtime,
        &parsed.realm_id,
        binding_id.as_deref().unwrap_or(""),
    )
    .await
    {
        Ok(v) => v,
        Err(r) => return r.with_id(id),
    };
    if parsed.provider != auth_profile.provider.as_str() {
        return RpcResponse::error(
            id,
            error::INVALID_PARAMS,
            format!(
                "binding {} resolves provider '{}' not '{}'",
                binding.id,
                auth_profile.provider.as_str(),
                parsed.provider,
            ),
        );
    }
    let http = reqwest::Client::new();
    let outcome =
        match poll_device_code(&http, &endpoints, &parsed.device_code, client_secret).await {
            Ok(o) => o,
            Err(e) => {
                return RpcResponse::error(
                    id,
                    error::INTERNAL_ERROR,
                    format!("device-code poll failed: {e}"),
                );
            }
        };
    match outcome {
        DevicePollOutcome::Pending => {
            RpcResponse::success(id, serde_json::json!({ "state": "pending" }))
        }
        DevicePollOutcome::SlowDown => {
            RpcResponse::success(id, serde_json::json!({ "state": "slow_down" }))
        }
        DevicePollOutcome::AccessDenied => {
            RpcResponse::success(id, serde_json::json!({ "state": "access_denied" }))
        }
        DevicePollOutcome::Expired => {
            RpcResponse::success(id, serde_json::json!({ "state": "expired" }))
        }
        DevicePollOutcome::Ready(result) => {
            let store = match require_token_store(runtime, id.clone()) {
                Ok(s) => s,
                Err(r) => return r,
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
            let key = TokenKey::new(connection_ref.realm.clone(), connection_ref.binding.clone());
            if let Err(e) = store.save(&key, &tokens).await {
                return RpcResponse::error(
                    id,
                    error::INTERNAL_ERROR,
                    format!("TokenStore save failed: {e}"),
                );
            }
            tracing::info!(
                target: "meerkat::auth::audit",
                binding_key = ?connection_ref,
                action = "login_device_complete",
                provider = %parsed.provider,
                has_refresh_token = %tokens.refresh_token.is_some(),
                "OAuth device-flow login completed via RPC"
            );
            RpcResponse::success(
                id,
                serde_json::json!({
                    "state": "ready",
                    "realm_id": connection_ref.realm.as_str(),
                    "binding_id": connection_ref.binding.as_str(),
                    "connection_ref": &connection_ref,
                    "profile_id": &binding.auth_profile,
                    "provider": parsed.provider,
                    "expires_at": expires_at.map(|e| e.to_rfc3339()),
                    "has_refresh_token": tokens.refresh_token.is_some(),
                    "scopes": tokens.scopes,
                }),
            )
        }
    }
}

#[derive(serde::Deserialize)]
struct ProvisionApiKeyParams {
    /// Access token acquired from a prior Console-OAuth flow.
    access_token: String,
    realm_id: String,
    #[serde(default)]
    binding_id: Option<String>,
}

/// Plan §4b.5 closure: Console OAuth → API key provisioning. The
/// caller runs a Console-scope OAuth flow (scope
/// `org:create_api_key user:profile`), then hands the resulting
/// access_token to this method. We POST to
/// `https://api.anthropic.com/api/oauth/claude_cli/create_api_key`
/// and persist the returned API key as a stable credential under
/// `<realm_id>:<binding_id>` with auth_mode=OauthToApiKey — so
/// future `resolve_binding` for the `oauth_to_api_key` method reads
/// the provisioned api_key without needing any refresh.
pub async fn handle_auth_login_provision_api_key(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: &SessionRuntime,
) -> RpcResponse {
    use meerkat_anthropic::runtime::oauth as a_oauth;
    let parsed: ProvisionApiKeyParams = match parse_params(params) {
        Ok(v) => v,
        Err(r) => return r.with_id(id),
    };
    let binding_id = parsed.binding_id;
    let store = match require_token_store(runtime, id.clone()) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let key = match TokenKey::parse(&parsed.realm_id, binding_id.as_deref().unwrap_or_default()) {
        Ok(k) => k,
        Err(err) => {
            return RpcResponse::error(
                id,
                error::INVALID_PARAMS,
                format!("invalid realm/binding: {err}"),
            );
        }
    };
    // Console endpoints drive `scope = org:create_api_key user:profile`.
    // The runtime wrapper's `provision_api_key` POSTs to
    // API_KEY_CREATE_URL with `Authorization: Bearer <access_token>`
    // and persists the returned api_key via `save_persisted`.
    let endpoints = a_oauth::console_endpoints(a_oauth::MANUAL_REDIRECT_URL);
    let oauth_runtime =
        a_oauth::AnthropicOAuthRuntime::new_with_default_coordinator(store, endpoints, key);
    match oauth_runtime.provision_api_key(&parsed.access_token).await {
        Ok(tokens) => RpcResponse::success(
            id,
            serde_json::json!({
                "realm_id": &parsed.realm_id,
                "binding_id": &binding_id,
                "connection_ref": {
                    "realm_id": &parsed.realm_id,
                    "binding_id": &binding_id,
                },
                "provider": "anthropic",
                "auth_mode": "oauth_to_api_key",
                "has_api_key": tokens.primary_secret.is_some(),
                "scopes": tokens.scopes,
            }),
        ),
        Err(e) => RpcResponse::error(
            id,
            error::INTERNAL_ERROR,
            format!("provision_api_key failed: {e}"),
        ),
    }
}

pub async fn handle_auth_status_get(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: &SessionRuntime,
) -> RpcResponse {
    let parsed: BindingIdParams = match parse_params(params) {
        Ok(v) => v,
        Err(r) => return r.with_id(id),
    };
    let (connection_ref, binding, auth_profile) =
        match resolve_binding_identity(runtime, &parsed.realm_id, &parsed.binding_id).await {
            Ok(v) => v,
            Err(r) => return r.with_id(id),
        };
    let stored = if let Some(store) = runtime.token_store() {
        store
            .load(&TokenKey::new(
                connection_ref.realm.clone(),
                connection_ref.binding.clone(),
            ))
            .await
            .unwrap_or(None)
    } else {
        None
    };
    let state_label = match &stored {
        Some(t) => match t.expires_at {
            Some(exp) if exp - chrono::Utc::now() < chrono::Duration::zero() => "expired",
            Some(exp) if exp - chrono::Utc::now() < chrono::Duration::seconds(60) => "expiring",
            _ => "valid",
        },
        None => "unknown",
    };
    RpcResponse::success(
        id,
        serde_json::json!({
            "realm_id": connection_ref.realm.as_str(),
            "binding_id": connection_ref.binding.as_str(),
            "connection_ref": &connection_ref,
            "profile_id": &binding.auth_profile,
            "provider": auth_profile.provider.as_str(),
            "auth_method": &auth_profile.auth_method,
            "state": state_label,
            "expires_at": stored.as_ref().and_then(|t| t.expires_at.map(|e| e.to_rfc3339())),
            "last_refresh_at": stored.as_ref().and_then(|t| t.last_refresh.map(|e| e.to_rfc3339())),
            "account_id": stored.as_ref().and_then(|t| t.account_id.clone()),
            "has_refresh_token": stored.as_ref().map(|t| t.refresh_token.is_some()).unwrap_or(false),
        }),
    )
}

pub async fn handle_auth_logout(
    id: Option<RpcId>,
    params: Option<&RawValue>,
    runtime: &SessionRuntime,
) -> RpcResponse {
    let parsed: BindingIdParams = match parse_params(params) {
        Ok(v) => v,
        Err(r) => return r.with_id(id),
    };
    let (connection_ref, binding, _) =
        match resolve_binding_identity(runtime, &parsed.realm_id, &parsed.binding_id).await {
            Ok(v) => v,
            Err(r) => return r.with_id(id),
        };
    let store = match require_token_store(runtime, id.clone()) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let key = TokenKey::new(connection_ref.realm.clone(), connection_ref.binding.clone());
    match store.clear(&key).await {
        Ok(()) => {
            tracing::info!(
                target: "meerkat::auth::audit",
                binding_key = ?connection_ref,
                action = "logout",
                "binding-scoped auth credentials logged out via RPC"
            );
            RpcResponse::success(
                id,
                serde_json::json!({
                    "realm_id": connection_ref.realm.as_str(),
                    "binding_id": connection_ref.binding.as_str(),
                    "connection_ref": &connection_ref,
                    "profile_id": &binding.auth_profile,
                    "cleared": true,
                }),
            )
        }
        Err(e) => RpcResponse::error(
            id,
            error::INTERNAL_ERROR,
            format!("TokenStore clear failed: {e}"),
        ),
    }
}
