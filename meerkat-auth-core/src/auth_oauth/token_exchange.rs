//! OAuth token exchange: authorization_code + refresh_token grants.

use reqwest::Client;

use super::{OAuthEndpoints, OAuthError, OAuthTokenResult};

#[derive(serde::Deserialize)]
struct TokenResponseWire {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    id_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
    #[serde(default)]
    scope: Option<String>,
}

/// Exchange an authorization code (with PKCE verifier) for a token bundle.
pub async fn exchange_authorization_code(
    http: &Client,
    endpoints: &OAuthEndpoints,
    code: &str,
    pkce_verifier: &str,
    client_secret: Option<&str>,
) -> Result<OAuthTokenResult, OAuthError> {
    let mut form = vec![
        ("grant_type", "authorization_code".to_string()),
        ("code", code.to_string()),
        ("redirect_uri", endpoints.redirect_uri.clone()),
        ("client_id", endpoints.client_id.clone()),
        ("code_verifier", pkce_verifier.to_string()),
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
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(OAuthError::TokenEndpoint {
            status: status.as_u16(),
            body,
        });
    }
    let wire: TokenResponseWire = resp
        .json()
        .await
        .map_err(|e| OAuthError::Network(format!("decode: {e}")))?;
    Ok(OAuthTokenResult {
        access_token: wire.access_token,
        refresh_token: wire.refresh_token,
        id_token: wire.id_token,
        expires_in_secs: wire.expires_in,
        scope: wire.scope,
    })
}

/// Exchange a refresh_token for a fresh access token.
pub async fn exchange_refresh_token(
    http: &Client,
    endpoints: &OAuthEndpoints,
    refresh_token: &str,
    client_secret: Option<&str>,
) -> Result<OAuthTokenResult, OAuthError> {
    let mut form = vec![
        ("grant_type", "refresh_token".to_string()),
        ("refresh_token", refresh_token.to_string()),
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
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(OAuthError::TokenEndpoint {
            status: status.as_u16(),
            body,
        });
    }
    let wire: TokenResponseWire = resp
        .json()
        .await
        .map_err(|e| OAuthError::Network(format!("decode: {e}")))?;
    Ok(OAuthTokenResult {
        access_token: wire.access_token,
        refresh_token: wire.refresh_token,
        id_token: wire.id_token,
        expires_in_secs: wire.expires_in,
        scope: wire.scope,
    })
}
