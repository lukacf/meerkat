//! Deterministic realtime fixtures (ADJ-P6B-4, gotcha #16 lift).
//!
//! [`ScriptedRealtimeSessionFactory`] is the shared deterministic
//! [`RealtimeSessionFactory`] fake for the live-pipeline and cross-host
//! batteries: no provider socket is ever opened; every open mints a
//! [`ScriptedLiveAdapter`] that records the commands it receives and holds
//! its observation stream open until closed. The factory advertises the
//! `pcm_24k_mono` audio policy in both directions so the extracted open
//! pipeline resolves a WS `&format=` token deterministically.
//!
//! The scripted precedents this lifts were test-private
//! (`meerkat-openai/tests/mock_realtime_ws`, the facade's
//! `RecordingProviderRealtimeFactory`); this module is the ONE shared home
//! (facade tests, `meerkat-mob/tests` via dev-dependency + feature, and
//! the integration crate's thin re-export shim).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use async_trait::async_trait;
use meerkat_contracts::{RealtimeAudioFormat, RealtimeCapabilities, RealtimeTurningMode};
use meerkat_core::live_adapter::{
    LiveAdapter, LiveAdapterCommand, LiveAdapterError, LiveAdapterObservation, LiveAdapterStatus,
    LiveChannelCapabilities,
};
use meerkat_core::{Provider, SessionLlmIdentity};
use meerkat_llm_core::LlmError;
use meerkat_llm_core::realtime_session::{
    RealtimeExternalSessionTarget, RealtimeSession, RealtimeSessionFactory,
    RealtimeSessionOpenConfig,
};

/// One recorded scripted open.
#[derive(Debug, Clone, PartialEq)]
pub struct ScriptedRealtimeOpen {
    /// The owning session identity the pipeline resolved for the open.
    pub identity: SessionLlmIdentity,
    /// The turning mode the pipeline resolved (absent-on-the-wire opens
    /// must arrive as `ProviderManaged` — the pipeline owns the default).
    pub turning_mode: RealtimeTurningMode,
}

/// Deterministic scripted realtime session factory.
pub struct ScriptedRealtimeSessionFactory {
    supported_provider: Option<Provider>,
    fail_opens: AtomicBool,
    opens: StdMutex<Vec<ScriptedRealtimeOpen>>,
    adapters: StdMutex<Vec<Arc<ScriptedLiveAdapter>>>,
}

impl Default for ScriptedRealtimeSessionFactory {
    fn default() -> Self {
        Self::new()
    }
}

impl ScriptedRealtimeSessionFactory {
    /// Factory supporting the OpenAI provider (the one realtime provider
    /// the catalog flags today).
    #[must_use]
    pub fn new() -> Self {
        Self {
            supported_provider: Some(Provider::OpenAI),
            fail_opens: AtomicBool::new(false),
            opens: StdMutex::new(Vec::new()),
            adapters: StdMutex::new(Vec::new()),
        }
    }

    /// Factory supporting NO provider — drives the B18
    /// `LiveAdapterUnavailable { provider }` rows.
    #[must_use]
    pub fn supporting_no_provider() -> Self {
        Self {
            supported_provider: None,
            ..Self::new()
        }
    }

    /// Script every subsequent adapter open to fail with a connection
    /// reset — drives the S9 partial-open-failure cleanup rows.
    pub fn fail_opens(&self) {
        self.fail_opens.store(true, Ordering::SeqCst);
    }

    /// Every recorded open, in order.
    #[must_use]
    pub fn opens(&self) -> Vec<ScriptedRealtimeOpen> {
        self.opens
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Count of successful + failed open attempts (the T-C9 / T-L24
    /// no-auto-resend pin).
    #[must_use]
    pub fn open_count(&self) -> usize {
        self.opens
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    /// Every adapter minted so far, in mint order.
    #[must_use]
    pub fn adapters(&self) -> Vec<Arc<ScriptedLiveAdapter>> {
        self.adapters
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn record_open(&self, open_config: &RealtimeSessionOpenConfig) {
        self.opens
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(ScriptedRealtimeOpen {
                identity: open_config.llm_identity.clone(),
                turning_mode: open_config.turning_mode,
            });
    }
}

#[async_trait]
impl RealtimeSessionFactory for ScriptedRealtimeSessionFactory {
    fn capabilities(&self) -> RealtimeCapabilities {
        // PCM 24 kHz mono both directions: the ONE binary format the live
        // WS transport negotiates, so the pipeline's #176 format
        // resolution succeeds deterministically.
        RealtimeCapabilities {
            audio_input_format: Some(RealtimeAudioFormat::pcm(24_000, 1)),
            audio_output_format: Some(RealtimeAudioFormat::pcm(24_000, 1)),
            ..RealtimeCapabilities::default()
        }
    }

    fn supports_provider(&self, provider: Provider) -> bool {
        self.supported_provider == Some(provider)
    }

    async fn open_session(
        &self,
        _open_config: &RealtimeSessionOpenConfig,
    ) -> Result<Box<dyn RealtimeSession>, LlmError> {
        Err(LlmError::InvalidConfig {
            message: "ScriptedRealtimeSessionFactory serves open_live_adapter only".to_string(),
        })
    }

    async fn attach_external_session(
        &self,
        _target: &RealtimeExternalSessionTarget,
        _open_config: &RealtimeSessionOpenConfig,
    ) -> Result<Box<dyn RealtimeSession>, LlmError> {
        Err(LlmError::InvalidConfig {
            message: "ScriptedRealtimeSessionFactory serves open_live_adapter only".to_string(),
        })
    }

    async fn open_live_adapter(
        &self,
        open_config: &RealtimeSessionOpenConfig,
    ) -> Result<Arc<dyn LiveAdapter>, LlmError> {
        self.record_open(open_config);
        if self.fail_opens.load(Ordering::SeqCst) {
            return Err(LlmError::ConnectionReset);
        }
        let adapter = Arc::new(ScriptedLiveAdapter::new());
        self.adapters
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(Arc::clone(&adapter));
        Ok(adapter)
    }
}

/// Deterministic scripted live adapter: records every command, reports
/// `Ready`, and holds `next_observation` open until closed.
pub struct ScriptedLiveAdapter {
    commands: StdMutex<Vec<LiveAdapterCommand>>,
    ready_emitted: AtomicBool,
    closed: AtomicBool,
    close_notify: tokio::sync::Notify,
}

impl ScriptedLiveAdapter {
    #[must_use]
    pub fn new() -> Self {
        Self {
            commands: StdMutex::new(Vec::new()),
            ready_emitted: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            close_notify: tokio::sync::Notify::new(),
        }
    }

    /// Every command the host dispatched to this adapter, in order.
    #[must_use]
    pub fn commands(&self) -> Vec<LiveAdapterCommand> {
        self.commands
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl Default for ScriptedLiveAdapter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl LiveAdapter for ScriptedLiveAdapter {
    async fn send_command(&self, command: LiveAdapterCommand) -> Result<(), LiveAdapterError> {
        self.commands
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(command);
        Ok(())
    }

    async fn next_observation(&self) -> Result<Option<LiveAdapterObservation>, LiveAdapterError> {
        // Provider parity: a real session's first observation is the
        // Ready status transition — the host's CHANNEL status (Opening →
        // Ready) folds from this stream, and input is rejected until it
        // lands. Emit it once, then park until close.
        if !self.ready_emitted.swap(true, Ordering::SeqCst) {
            return Ok(Some(LiveAdapterObservation::StatusChanged {
                status: LiveAdapterStatus::Ready,
            }));
        }
        // Register the wakeup BEFORE re-checking the flag: a close() that
        // lands between the check and the await would otherwise be a lost
        // notify_waiters wakeup and park this stream forever.
        loop {
            let notified = self.close_notify.notified();
            if self.closed.load(Ordering::SeqCst) {
                return Ok(None);
            }
            notified.await;
        }
    }

    fn status(&self) -> LiveAdapterStatus {
        if self.closed.load(Ordering::SeqCst) {
            LiveAdapterStatus::Closed
        } else {
            LiveAdapterStatus::Ready
        }
    }

    async fn close(&self) -> Result<(), LiveAdapterError> {
        self.closed.store(true, Ordering::SeqCst);
        self.close_notify.notify_waiters();
        Ok(())
    }

    fn capabilities(&self) -> LiveChannelCapabilities {
        LiveChannelCapabilities {
            audio_in: true,
            audio_out: true,
            text_in: true,
            text_out: true,
            ..LiveChannelCapabilities::default()
        }
    }
}
