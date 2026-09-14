//! Public Live transport construction. Executor permission and channel
//! lifecycle remain with the composing runtime; this factory issues neither.

use meerkat_core::Provider;
use meerkat_core::provider_matrix::openai::OpenAiBackendKind;
use meerkat_llm_core::provider_runtime::{NormalizedBackendKind, ResolvedLiveTarget};
use oai_rt_rs::live::{
    ClientOptions, Codec, CreateRequest, CreateResponse, LiveClient, LiveConnection, SessionConfig,
    WebRtcTransport,
};

use super::config::{PublicLiveConfigError, PublicLiveVoiceSettings, session_config};

pub use meerkat_llm_core::live_adapter_factory::LiveTransportLimits as PublicLiveTransportLimits;

pub struct OpenAiPublicLiveSessionFactory {
    client: LiveClient,
    session: SessionConfig,
    max_request_bytes: usize,
}

impl std::fmt::Debug for OpenAiPublicLiveSessionFactory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("OpenAiPublicLiveSessionFactory([REDACTED])")
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PublicLiveSessionError {
    #[error("public Live requires the OpenAI API backend")]
    UnsupportedBackend,
    #[error("public Live requires API-key authentication, not a request authorizer")]
    UnsupportedAuthentication,
    #[error("public Live connection has no resolved API key")]
    MissingCredential,
    #[error("public Live API root is invalid")]
    InvalidApiRoot,
    #[error(
        "public Live transport bounds must be positive and queues must fit the runtime semaphore"
    )]
    InvalidLimits,
    #[error("public Live signaling request exceeds its encoded byte bound")]
    RequestTooLarge,
    #[error(transparent)]
    Configuration(#[from] PublicLiveConfigError),
    #[error(transparent)]
    Transport(#[from] oai_rt_rs::live::Error),
}

struct RequestByteBudget {
    remaining: usize,
    exceeded: bool,
}

impl std::io::Write for RequestByteBudget {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let Some(remaining) = self.remaining.checked_sub(bytes.len()) else {
            self.exceeded = true;
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Live signaling byte bound exceeded",
            ));
        };
        self.remaining = remaining;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl OpenAiPublicLiveSessionFactory {
    pub fn new(
        target: &ResolvedLiveTarget,
        settings: PublicLiveVoiceSettings<'_>,
        limits: PublicLiveTransportLimits,
    ) -> Result<Self, PublicLiveSessionError> {
        if limits.max_event_bytes == 0
            || limits.event_capacity == 0
            || limits.command_capacity == 0
            || limits.io_timeout.is_zero()
            || std::time::Instant::now()
                .checked_add(limits.io_timeout)
                .is_none()
            || limits.event_capacity > tokio::sync::Semaphore::MAX_PERMITS
            || limits.command_capacity > tokio::sync::Semaphore::MAX_PERMITS
        {
            return Err(PublicLiveSessionError::InvalidLimits);
        }
        let connection = target.connection();
        if target.voice_identity().provider != Provider::OpenAI
            || !matches!(
                connection.backend,
                NormalizedBackendKind::OpenAi(OpenAiBackendKind::OpenAiApi)
            )
        {
            return Err(PublicLiveSessionError::UnsupportedBackend);
        }
        if connection.resolved_authorizer().is_some() {
            return Err(PublicLiveSessionError::UnsupportedAuthentication);
        }
        let session = session_config(target, settings)?;
        let mut options = ClientOptions {
            request_timeout: limits.io_timeout,
            codec: Codec {
                max_event_bytes: limits.max_event_bytes,
            },
            command_capacity: limits.command_capacity,
            event_capacity: limits.event_capacity,
            ..ClientOptions::default()
        };
        if let Some(root) = connection.backend_profile.base_url.as_deref() {
            options.base_url = format!("{}/", root.trim_end_matches('/'))
                .parse()
                .map_err(|_| PublicLiveSessionError::InvalidApiRoot)?;
        }
        let secret = connection
            .resolved_secret()
            .ok_or(PublicLiveSessionError::MissingCredential)?;
        Ok(Self {
            client: LiveClient::with_options(&secret, options)?,
            session,
            max_request_bytes: limits.max_event_bytes,
        })
    }

    /// Completes the primary handshake and session.start/session.started
    /// exchange. Startup events remain in the connection's event receiver.
    pub async fn open_websocket(&self) -> Result<LiveConnection, PublicLiveSessionError> {
        Ok(self.client.connect(self.session.clone()).await?)
    }

    /// Returns signaling content and provider resource identity, not readiness
    /// or final usage. The caller owns subsequent attachment and cleanup.
    pub async fn create_webrtc(
        &self,
        offer_sdp: String,
    ) -> Result<CreateResponse, PublicLiveSessionError> {
        let mut session = self.session.clone();
        if let Some(audio) = session.audio.as_mut() {
            audio.format = None;
        }
        let request = CreateRequest {
            session,
            transport: WebRtcTransport::WebRtc { sdp: offer_sdp },
        };
        let mut budget = RequestByteBudget {
            remaining: self.max_request_bytes,
            exceeded: false,
        };
        if let Err(error) = serde_json::to_writer(&mut budget, &request) {
            return Err(if budget.exceeded {
                PublicLiveSessionError::RequestTooLarge
            } else {
                oai_rt_rs::live::Error::Json(error).into()
            });
        }
        Ok(self.client.create_webrtc(&request).await?)
    }

    /// Attaches to an already-created provider session without starting
    /// another one. Failed attachment does not claim the resource was closed.
    pub async fn attach_sideband(
        &self,
        provider_session_id: &str,
    ) -> Result<LiveConnection, PublicLiveSessionError> {
        Ok(self.client.attach(provider_session_id).await?)
    }
}
