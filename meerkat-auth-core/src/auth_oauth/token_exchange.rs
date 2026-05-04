//! OAuth token exchange: authorization_code + refresh_token grants.

use reqwest::Client;

use super::{OAuthEndpoints, OAuthError, OAuthTokenRequestFormat, OAuthTokenResult};

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
    exchange_authorization_code_with_state(
        http,
        endpoints,
        code,
        pkce_verifier,
        client_secret,
        None,
    )
    .await
}

/// Exchange an authorization code while echoing the callback state for
/// providers that require it in the token request.
pub async fn exchange_authorization_code_with_state(
    http: &Client,
    endpoints: &OAuthEndpoints,
    code: &str,
    pkce_verifier: &str,
    client_secret: Option<&str>,
    state: Option<&str>,
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
    if endpoints.include_state_in_token_exchange {
        let state = state.ok_or_else(|| {
            OAuthError::InvalidConfig("provider requires state in token exchange".into())
        })?;
        form.push(("state", state.to_string()));
    }
    send_token_request(http, endpoints, &form).await
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
    if !endpoints.refresh_scopes.is_empty() {
        form.push(("scope", endpoints.refresh_scopes.join(" ")));
    }
    send_token_request(http, endpoints, &form).await
}

async fn send_token_request(
    http: &Client,
    endpoints: &OAuthEndpoints,
    params: &[(&str, String)],
) -> Result<OAuthTokenResult, OAuthError> {
    let mut req = match endpoints.token_request_format {
        OAuthTokenRequestFormat::FormUrlEncoded => http.post(&endpoints.token_url).form(params),
        OAuthTokenRequestFormat::Json => {
            let body = params
                .iter()
                .map(|(key, value)| ((*key).to_string(), serde_json::Value::String(value.clone())))
                .collect::<serde_json::Map<_, _>>();
            http.post(&endpoints.token_url).json(&body)
        }
    };
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
