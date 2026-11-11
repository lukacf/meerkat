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
    let resp = req.send().await.map_err(super::credential_request_error)?;
    let status = resp.status();
    super::refuse_credential_redirect(status)?;
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(OAuthError::TokenEndpoint {
            status: status.as_u16(),
            body,
        });
    }
    resp.json().await.map_err(super::credential_decode_error)
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
    let resp = req.send().await.map_err(super::credential_request_error)?;
    let status = resp.status();
    super::refuse_credential_redirect(status)?;

    // Device token endpoints carry typed errors in the body. GitHub returns
    // pending with 200 while RFC-style providers commonly use 4xx, so classify
    // the body before interpreting the HTTP status.
    let body: TokenResponseWire = resp.json().await.map_err(super::credential_decode_error)?;

    if let Some(error) = body.error.as_deref() {
        return match error {
            "authorization_pending" => Ok(DevicePollOutcome::Pending),
            "slow_down" => Ok(DevicePollOutcome::SlowDown),
            "access_denied" => Ok(DevicePollOutcome::AccessDenied),
            "expired_token" => Ok(DevicePollOutcome::Expired),
            other => Err(OAuthError::TokenEndpoint {
                status: status.as_u16(),
                body: format!("error={other}"),
            }),
        };
    }

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

    Err(OAuthError::TokenEndpoint {
        status: status.as_u16(),
        body: "non-2xx without error field".into(),
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::auth_oauth::credential_http_client;
    use crate::oauth_flow::{OAuthProviderIdentity, oauth_provider_resolution};

    /// Each request as the endpoint received it: its path and its headers.
    type Recorded = std::sync::Arc<parking_lot::Mutex<Vec<(String, axum::http::HeaderMap)>>>;

    async fn record(
        axum::extract::State(recorded): axum::extract::State<Recorded>,
        uri: axum::http::Uri,
        headers: axum::http::HeaderMap,
    ) -> axum::Json<serde_json::Value> {
        let path = uri.path().to_owned();
        let answer = if path == "/device/code" {
            serde_json::json!({
                "device_code": "device",
                "user_code": "USER-CODE",
                "verification_uri": "https://example.test/device",
                "expires_in": 900,
                "interval": 5
            })
        } else {
            serde_json::json!({"error": "authorization_pending"})
        };
        recorded.lock().push((path, headers));
        axum::Json(answer)
    }

    /// The GitHub Copilot device sign-in's own requests to GitHub, the
    /// device-code request and the token poll, carry the Copilot
    /// `User-Agent`, pinned here by value: no backend option reaches these
    /// requests, so the constant is all they send. The endpoints are the
    /// sign-in's own (`oauth_provider_resolution`); only the two URLs point
    /// at a local server.
    #[tokio::test]
    async fn github_copilot_device_sign_in_requests_carry_the_pinned_user_agent() {
        let recorded = Recorded::default();
        let app = axum::Router::new()
            .route("/device/code", axum::routing::post(record))
            .route("/token", axum::routing::post(record))
            .with_state(std::sync::Arc::clone(&recorded));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let mut endpoints = oauth_provider_resolution(
            OAuthProviderIdentity::GitHubCopilot,
            "http://127.0.0.1/callback",
        )
        .endpoints;
        endpoints.device_code_url = Some(format!("http://{address}/device/code"));
        endpoints.token_url = format!("http://{address}/token");

        let http = credential_http_client().unwrap();
        let device = request_device_code(&http, &endpoints).await.unwrap();
        let outcome = poll_device_code(&http, &endpoints, &device.device_code, None)
            .await
            .unwrap();
        assert!(matches!(outcome, DevicePollOutcome::Pending));

        let recorded = recorded.lock().clone();
        let paths: Vec<&str> = recorded.iter().map(|(path, _)| path.as_str()).collect();
        assert_eq!(paths, ["/device/code", "/token"]);
        for (path, headers) in &recorded {
            let header = |name: &str| headers.get(name).and_then(|value| value.to_str().ok());
            assert_eq!(
                header("user-agent"),
                Some("GitHubCopilotChat/0.73.0"),
                "{path}"
            );
            assert_eq!(header("accept"), Some("application/json"), "{path}");
        }
    }
}
