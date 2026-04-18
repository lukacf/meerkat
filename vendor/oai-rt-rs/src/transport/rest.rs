use crate::error::Result;
use crate::protocol::models::{Session, SessionConfig, SessionKind};
use reqwest::{
    Client,
    header::{AUTHORIZATION, HeaderValue, LOCATION},
    multipart,
};
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EphemeralSecretResponse {
    pub value: String,
    pub expires_at: u64,
    pub session: Session,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpiresAfter {
    pub anchor: String,
    pub seconds: u64,
}

#[derive(Debug, Clone, Serialize)]
struct CreateClientSecretRequest {
    pub session: SessionConfig,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_after: Option<ExpiresAfter>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallCreationResponse {
    pub sdp: String,
    pub call_id: Option<String>,
}

const BASE_URL: &str = "https://api.openai.com/v1/realtime";
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_POOL_IDLE_TIMEOUT: Duration = Duration::from_secs(90);

/// An adapter for the `OpenAI` Realtime REST API.
#[derive(Clone, Debug)]
pub struct RealtimeRestAdapter {
    client: Client,
    auth_header: HeaderValue,
}

impl RealtimeRestAdapter {
    /// Create a new adapter with the given API key.
    ///
    /// # Errors
    /// Returns an error if the API key results in an invalid header or client build fails.
    #[allow(clippy::result_large_err)]
    pub fn new(api_key: &str) -> Result<Self> {
        Self::new_with_timeouts(api_key, DEFAULT_TIMEOUT, DEFAULT_POOL_IDLE_TIMEOUT)
    }

    /// Create a new adapter with custom timeouts.
    ///
    /// # Errors
    /// Returns an error if the API key results in an invalid header or client build fails.
    #[allow(clippy::result_large_err)]
    pub fn new_with_timeouts(
        api_key: &str,
        timeout: Duration,
        pool_idle_timeout: Duration,
    ) -> Result<Self> {
        let client = Client::builder()
            .timeout(timeout)
            .pool_idle_timeout(pool_idle_timeout)
            .build()?;

        let auth_header = HeaderValue::from_str(&format!("Bearer {api_key}"))?;

        Ok(Self {
            client,
            auth_header,
        })
    }

    /// Create an ephemeral client secret for browser usage (GA).
    ///
    /// # Errors
    /// Returns an error if the HTTP request fails.
    pub async fn create_client_secret(
        &self,
        session: SessionConfig,
    ) -> Result<EphemeralSecretResponse> {
        self.create_client_secret_with_expiry(session, None).await
    }

    /// Create an ephemeral client secret with an explicit expiry configuration.
    ///
    /// # Errors
    /// Returns an error if the HTTP request fails.
    pub async fn create_client_secret_with_expiry(
        &self,
        session: SessionConfig,
        expires_after: Option<ExpiresAfter>,
    ) -> Result<EphemeralSecretResponse> {
        if session.kind != SessionKind::Realtime {
            return Err(crate::error::Error::InvalidClientEvent(
                "client_secrets only supports realtime sessions".to_string(),
            ));
        }

        let res = self
            .client
            .post(format!("{BASE_URL}/client_secrets"))
            .header(AUTHORIZATION, &self.auth_header)
            .json(&CreateClientSecretRequest {
                session,
                expires_after,
            })
            .send()
            .await?
            .error_for_status()?;

        Ok(res.json().await?)
    }

    /// Post an SDP offer to initiate a WebRTC call (Direct - raw SDP).
    ///
    /// # Errors
    /// Returns an error if the HTTP request fails.
    pub async fn post_sdp_offer_raw(&self, sdp_offer: String) -> Result<String> {
        Ok(self.post_sdp_offer_raw_with_call_id(sdp_offer).await?.sdp)
    }

    /// Post an SDP offer to initiate a WebRTC call (Direct - raw SDP) and return `call_id`.
    ///
    /// # Errors
    /// Returns an error if the HTTP request fails.
    pub async fn post_sdp_offer_raw_with_call_id(
        &self,
        sdp_offer: String,
    ) -> Result<CallCreationResponse> {
        let url = format!("{BASE_URL}/calls");

        let res = self
            .client
            .post(url)
            .header(AUTHORIZATION, &self.auth_header)
            .header("Content-Type", "application/sdp")
            .body(sdp_offer)
            .send()
            .await?
            .error_for_status()?;

        let call_id = res.headers().get(LOCATION).and_then(extract_call_id);
        Ok(CallCreationResponse {
            sdp: res.text().await?,
            call_id,
        })
    }

    /// Post an SDP offer to initiate a WebRTC call (Unified - Multipart).
    ///
    /// # Errors
    /// Returns an error if the HTTP request fails.
    pub async fn post_sdp_offer_multipart(
        &self,
        sdp_offer: String,
        session: Option<SessionConfig>,
    ) -> Result<String> {
        Ok(self
            .post_sdp_offer_multipart_with_call_id(sdp_offer, session)
            .await?
            .sdp)
    }

    /// Post an SDP offer to initiate a WebRTC call (Unified - Multipart) and return `call_id`.
    ///
    /// # Errors
    /// Returns an error if the HTTP request fails.
    pub async fn post_sdp_offer_multipart_with_call_id(
        &self,
        sdp_offer: String,
        session: Option<SessionConfig>,
    ) -> Result<CallCreationResponse> {
        let url = format!("{BASE_URL}/calls");

        let sdp_part = multipart::Part::text(sdp_offer)
            .mime_str("application/sdp")
            .map_err(|e| crate::error::Error::Mime(e.to_string()))?;
        let mut form = multipart::Form::new().part("sdp", sdp_part);

        if let Some(s) = session {
            let session_part = multipart::Part::text(serde_json::to_string(&s)?)
                .mime_str("application/json")
                .map_err(|e| crate::error::Error::Mime(e.to_string()))?;
            form = form.part("session", session_part);
        }

        let res = self
            .client
            .post(url)
            .header(AUTHORIZATION, &self.auth_header)
            .multipart(form)
            .send()
            .await?
            .error_for_status()?;

        let call_id = res.headers().get(LOCATION).and_then(extract_call_id);
        Ok(CallCreationResponse {
            sdp: res.text().await?,
            call_id,
        })
    }

    /// Accept an incoming SIP call.
    ///
    /// # Errors
    /// Returns an error if the HTTP request fails or returns a non-success status.
    pub async fn sip_accept(&self, call_id: &str, session: SessionConfig) -> Result<()> {
        let url = format!("{BASE_URL}/calls/{call_id}/accept");

        if session.kind != SessionKind::Realtime {
            return Err(crate::error::Error::InvalidClientEvent(
                "sip.accept only supports realtime sessions".to_string(),
            ));
        }

        self.client
            .post(&url)
            .header(AUTHORIZATION, &self.auth_header)
            .json(&session)
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }

    /// Reject an incoming SIP call.
    ///
    /// # Errors
    /// Returns an error if the HTTP request fails.
    pub async fn sip_reject(&self, call_id: &str) -> Result<()> {
        let url = format!("{BASE_URL}/calls/{call_id}/reject");
        self.client
            .post(&url)
            .header(AUTHORIZATION, &self.auth_header)
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }

    /// Hang up a call (WebRTC or SIP).
    ///
    /// # Errors
    /// Returns an error if the HTTP request fails.
    pub async fn hangup(&self, call_id: &str) -> Result<()> {
        let url = format!("{BASE_URL}/calls/{call_id}/hangup");
        self.client
            .post(&url)
            .header(AUTHORIZATION, &self.auth_header)
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }

    /// Refer (transfer) a SIP call to another URI.
    ///
    /// # Errors
    /// Returns an error if the HTTP request fails.
    pub async fn sip_refer(&self, call_id: &str, target_uri: impl Into<String>) -> Result<()> {
        let url = format!("{BASE_URL}/calls/{call_id}/refer");
        let body = SipReferRequest {
            target_uri: target_uri.into(),
        };

        self.client
            .post(&url)
            .header(AUTHORIZATION, &self.auth_header)
            .json(&body)
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientSecret {
    pub value: String,
    pub expires_at: u64,
}

#[derive(Debug, Clone, Serialize)]
struct SipReferRequest {
    pub target_uri: String,
}

fn extract_call_id(location: &HeaderValue) -> Option<String> {
    let value = location.to_str().ok()?;
    value
        .rsplit('/')
        .find(|segment| !segment.is_empty())
        .map(str::to_owned)
}
