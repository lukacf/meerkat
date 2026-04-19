//! OAuth device-code flow (RFC 8628).
//!
//! Step 1: POST to the device-code endpoint → receive (device_code,
//! user_code, verification_uri, expires_in, interval).
//! Step 2: display user_code + verification_uri to the user.
//! Step 3: poll the token endpoint at `interval` seconds until success,
//! `authorization_pending`, `slow_down`, `access_denied`, or
//! `expired_token`.
//!
//! Reference-CLI parity: Gemini CLI
//! `packages/core/src/code_assist/oauth2.ts:360-520` (authWithUserCode).

use reqwest::Client;
use serde::Deserialize;

use super::{OAuthEndpoints, OAuthError, OAuthTokenResult};

#[derive(Debug, Clone, Deserialize)]
pub struct DeviceCodeResponse {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    #[serde(default)]
    pub verification_uri_complete: Option<String>,
    pub expires_in: u64,
    #[serde(default = "default_interval")]
    pub interval: u64,
}

fn default_interval() -> u64 {
    5
}

/// Result of a single poll cycle.
pub enum DevicePollOutcome {
    Pending,
    SlowDown,
    AccessDenied,
    Expired,
    Ready(OAuthTokenResult),
}

#[derive(Deserialize)]
struct TokenResponseWire {
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    id_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

pub async fn request_device_code(
    http: &Client,
    endpoints: &OAuthEndpoints,
) -> Result<DeviceCodeResponse, OAuthError> {
    let url = endpoints
        .device_code_url
        .as_deref()
        .ok_or_else(|| OAuthError::InvalidConfig("device_code_url not configured".into()))?;
    let form = vec![
        ("client_id", endpoints.client_id.clone()),
        ("scope", endpoints.scopes.join(" ")),
    ];
    let mut req = http.post(url).form(&form);
    for (k, v) in &endpoints.extra_headers {
        req = req.header(k, v);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| OAuthError::Network(e.to_string()))?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(OAuthError::TokenEndpoint {
            status: status.as_u16(),
            body,
        });
    }
    resp.json()
        .await
        .map_err(|e| OAuthError::Network(format!("decode: {e}")))
}

pub async fn poll_device_code(
    http: &Client,
    endpoints: &OAuthEndpoints,
    device_code: &str,
    client_secret: Option<&str>,
) -> Result<DevicePollOutcome, OAuthError> {
    let mut form = vec![
        (
            "grant_type",
            "urn:ietf:params:oauth:grant-type:device_code".to_string(),
        ),
        ("device_code", device_code.to_string()),
        ("client_id", endpoints.client_id.clone()),
    ];
    if let Some(secret) = client_secret {
        form.push(("client_secret", secret.to_string()));
    }
    let mut req = http.post(&endpoints.token_url).form(&form);
    for (k, v) in &endpoints.extra_headers {
        req = req.header(k, v);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| OAuthError::Network(e.to_string()))?;
    let status = resp.status();

    // Device-code token polling returns 4xx with typed error codes. We
    // parse the JSON body and dispatch on `error`.
    let body: TokenResponseWire = resp
        .json()
        .await
        .map_err(|e| OAuthError::Network(format!("decode: {e}")))?;

    if status.is_success() {
        let access = body.access_token.ok_or_else(|| OAuthError::TokenEndpoint {
            status: status.as_u16(),
            body: "200 but no access_token".into(),
        })?;
        return Ok(DevicePollOutcome::Ready(OAuthTokenResult {
            access_token: access,
            refresh_token: body.refresh_token,
            id_token: body.id_token,
            expires_in_secs: body.expires_in,
            scope: body.scope,
        }));
    }

    match body.error.as_deref() {
        Some("authorization_pending") => Ok(DevicePollOutcome::Pending),
        Some("slow_down") => Ok(DevicePollOutcome::SlowDown),
        Some("access_denied") => Ok(DevicePollOutcome::AccessDenied),
        Some("expired_token") => Ok(DevicePollOutcome::Expired),
        Some(other) => Err(OAuthError::TokenEndpoint {
            status: status.as_u16(),
            body: format!("error={other}"),
        }),
        None => Err(OAuthError::TokenEndpoint {
            status: status.as_u16(),
            body: "non-2xx without error field".into(),
        }),
    }
}
