//! Provider-neutral adapter construction. Continuous opens have no ordinary
//! transcript/tool seed; the legacy wrapper forwards its existing projection.

use std::sync::Arc;

use meerkat_core::live_adapter::{LiveAdapter, LiveAdapterError};
use meerkat_core::live_execution::LiveChannelId;
use meerkat_core::model_profile::ModelInteractionKind;
use meerkat_core::time_compat::Duration;

use crate::LlmError;
use crate::realtime_session::{RealtimeSessionFactory, RealtimeSessionOpenConfig};

#[derive(Debug, Clone, Copy)]
pub struct LiveTransportLimits {
    pub max_event_bytes: usize,
    pub event_capacity: usize,
    pub command_capacity: usize,
    pub io_timeout: Duration,
}

#[derive(Clone)]
pub struct ContinuousLiveOpenConfig {
    pub channel_id: LiveChannelId,
    pub voice: Option<String>,
    pub instructions: Option<String>,
    pub limits: LiveTransportLimits,
}

pub enum LiveAdapterOpenConfig {
    Realtime(Box<RealtimeSessionOpenConfig>),
    Continuous(ContinuousLiveOpenConfig),
}

#[derive(Debug, thiserror::Error)]
pub enum LiveAdapterOpenError {
    #[error("this adapter factory does not support the requested transport")]
    UnsupportedTransport,
    #[error("adapter interaction mismatch: expected {expected:?}, received {actual:?}")]
    InteractionMismatch {
        expected: ModelInteractionKind,
        actual: ModelInteractionKind,
    },
    #[error(transparent)]
    Realtime(#[from] LlmError),
    #[error(transparent)]
    Adapter(#[from] LiveAdapterError),
}

impl LiveAdapterOpenConfig {
    pub fn interaction_kind(&self) -> ModelInteractionKind {
        match self {
            Self::Realtime(_) => ModelInteractionKind::TurnBasedRealtime,
            Self::Continuous(_) => ModelInteractionKind::ContinuousLive,
        }
    }
}

impl std::fmt::Debug for LiveAdapterOpenConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveAdapterOpenConfig")
            .field("interaction_kind", &self.interaction_kind())
            .finish_non_exhaustive()
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
pub trait LiveAdapterFactory: Send + Sync {
    fn interaction_kind(&self) -> ModelInteractionKind;

    async fn open_adapter(
        &self,
        config: &LiveAdapterOpenConfig,
    ) -> Result<Arc<dyn LiveAdapter>, LiveAdapterOpenError>;

    async fn prepare_webrtc(
        &self,
        _config: &LiveAdapterOpenConfig,
        _offer_sdp: &str,
    ) -> Result<Box<dyn LiveWebrtcPreparation>, LiveAdapterOpenError> {
        Err(LiveAdapterOpenError::UnsupportedTransport)
    }
}

/// Provider-owned creation custody. An SDP answer is not provider-start or
/// media readiness. Failed attachment leaves this object available for recovery.
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
pub trait LiveWebrtcPreparation: Send + Sync {
    fn answer_sdp(&self) -> &str;
    async fn attach_adapter(&mut self) -> Result<Arc<dyn LiveAdapter>, LiveAdapterOpenError>;
}

pub struct LegacyLiveAdapterFactory {
    inner: Arc<dyn RealtimeSessionFactory>,
}

impl LegacyLiveAdapterFactory {
    pub fn new(inner: Arc<dyn RealtimeSessionFactory>) -> Self {
        Self { inner }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
impl LiveAdapterFactory for LegacyLiveAdapterFactory {
    fn interaction_kind(&self) -> ModelInteractionKind {
        ModelInteractionKind::TurnBasedRealtime
    }

    async fn open_adapter(
        &self,
        config: &LiveAdapterOpenConfig,
    ) -> Result<Arc<dyn LiveAdapter>, LiveAdapterOpenError> {
        let LiveAdapterOpenConfig::Realtime(realtime) = config else {
            return Err(LiveAdapterOpenError::InteractionMismatch {
                expected: self.interaction_kind(),
                actual: config.interaction_kind(),
            });
        };
        Ok(self.inner.open_live_adapter(realtime).await?)
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use crate::realtime_session::{RealtimeExternalSessionTarget, RealtimeSession};
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct LegacyProbe(Arc<AtomicUsize>);

    #[async_trait::async_trait]
    impl RealtimeSessionFactory for LegacyProbe {
        fn capabilities(&self) -> meerkat_contracts::RealtimeCapabilities {
            Default::default()
        }
        async fn open_session(
            &self,
            _: &RealtimeSessionOpenConfig,
        ) -> Result<Box<dyn RealtimeSession>, LlmError> {
            Err(LlmError::InvalidRequest {
                message: "wrong legacy method".into(),
            })
        }
        async fn attach_external_session(
            &self,
            _: &RealtimeExternalSessionTarget,
            _: &RealtimeSessionOpenConfig,
        ) -> Result<Box<dyn RealtimeSession>, LlmError> {
            Err(LlmError::InvalidRequest {
                message: "wrong legacy method".into(),
            })
        }
        async fn open_live_adapter(
            &self,
            config: &RealtimeSessionOpenConfig,
        ) -> Result<Arc<dyn LiveAdapter>, LlmError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            assert_eq!(config.llm_identity.model, "gpt-realtime-2");
            Err(LlmError::InvalidRequest {
                message: "original legacy result".into(),
            })
        }
    }

    #[tokio::test]
    async fn neutral_factory_preserves_legacy_dispatch_and_refuses_continuous_reinterpretation()
    -> Result<(), Box<dyn std::error::Error>> {
        let calls = Arc::new(AtomicUsize::new(0));
        let factory = LegacyLiveAdapterFactory::new(Arc::new(LegacyProbe(Arc::clone(&calls))));
        let continuous = LiveAdapterOpenConfig::Continuous(ContinuousLiveOpenConfig {
            channel_id: LiveChannelId::new("channel"),
            voice: None,
            instructions: Some("must not reach a legacy seed".into()),
            limits: LiveTransportLimits {
                max_event_bytes: 1024,
                event_capacity: 4,
                command_capacity: 4,
                io_timeout: Duration::from_secs(1),
            },
        });
        assert!(matches!(
            factory.open_adapter(&continuous).await,
            Err(LiveAdapterOpenError::InteractionMismatch { .. })
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        let realtime = RealtimeSessionOpenConfig::new(
            meerkat_contracts::RealtimeTurningMode::ProviderManaged,
            meerkat_core::SessionLlmIdentity {
                provider: meerkat_core::Provider::OpenAI,
                model: "gpt-realtime-2".into(),
                provider_params: None,
                auth_binding: None,
                self_hosted_server_id: None,
            },
            vec![],
            vec![],
        )?;
        assert!(
            matches!(factory.open_adapter(&LiveAdapterOpenConfig::Realtime(Box::new(realtime))).await,
                Err(LiveAdapterOpenError::Realtime(LlmError::InvalidRequest { message })) if message == "original legacy result")
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        Ok(())
    }
}
