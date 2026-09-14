use std::sync::Arc;

use meerkat_core::SessionId;
use meerkat_core::live_execution::LiveChannelId;
use meerkat_core::live_observation::{
    LiveObservationReceiveClock, LiveObservationReceiveReceipt, LiveObservationValueError,
    LiveTranscriptObservation,
};
use meerkat_live::host::{
    ContinuousControlResult, ContinuousTranscriptResult, LiveContinuousTranscriptIngress,
    LiveProjectionError,
};

use super::{LiveTranscriptChannelIngress, LiveTranscriptWriteError};

/// One shared attachment around the existing exclusive store writer. Receive
/// accounting is synchronous; persistence and source selection serialize here.
pub struct LiveContinuousChannel {
    session_id: SessionId,
    channel_id: LiveChannelId,
    receive_clock: LiveObservationReceiveClock,
    writer: crate::tokio::sync::Mutex<LiveTranscriptChannelIngress>,
    provider_progress: crate::tokio::sync::Notify,
}

impl LiveTranscriptChannelIngress {
    pub fn into_shared(self) -> Arc<LiveContinuousChannel> {
        Arc::new(LiveContinuousChannel {
            session_id: self.session_id().clone(),
            channel_id: self.channel_id().clone(),
            receive_clock: self.receive_clock(),
            writer: crate::tokio::sync::Mutex::new(self),
            provider_progress: crate::tokio::sync::Notify::new(),
        })
    }
}

impl LiveContinuousChannel {
    /// Wait for an owner fact, not readiness or permission. The notification
    /// is only a local-pump wakeup; every wake re-reads the durable owner.
    pub async fn wait_provider_start(
        &self,
        timeout: std::time::Duration,
    ) -> Result<super::CommittedLiveProviderControl, LiveTranscriptWriteError> {
        crate::tokio::time::timeout(timeout, async {
            loop {
                let progress = self.provider_progress.notified();
                crate::tokio::pin!(progress);
                progress.as_mut().enable();
                {
                    let writer = self.writer.lock().await;
                    if let Some(receipt) = writer.read_provider_start().await? {
                        return Ok(receipt);
                    }
                    if writer.observation_closure_committed().await? {
                        return Err(LiveTranscriptWriteError::Retired);
                    }
                }
                progress.await;
            }
        })
        .await
        .map_err(|_| LiveTranscriptWriteError::ProviderStartWaitTimedOut)?
    }

    pub async fn read_provider_start(
        &self,
    ) -> Result<Option<super::CommittedLiveProviderControl>, LiveTranscriptWriteError> {
        self.writer.lock().await.read_provider_start().await
    }

    fn projection_error(&self, error: LiveTranscriptWriteError) -> LiveProjectionError {
        match error {
            LiveTranscriptWriteError::Store(error) => LiveProjectionError::from_session_error(
                &self.session_id,
                meerkat_core::service::SessionError::Store(Box::new(error)),
            ),
            error => LiveProjectionError::Rejected(error.to_string()),
        }
    }
    pub async fn reserve_client_source(
        &self,
        delegation: meerkat_core::live_execution::request::LiveProviderReference,
        offset_ms: f64,
        grant: Option<&crate::live_grant::LiveExecutionGrant<()>>,
    ) -> Result<super::LiveSourceReservationOutcome, super::LiveSourceReservationError> {
        self.writer
            .lock()
            .await
            .reserve_client_source(delegation, offset_ms, grant)
            .await
    }

    pub async fn reserve_and_admit_client_source(
        &self,
        runtime: &crate::MeerkatMachine,
        delegation: meerkat_core::live_execution::request::LiveProviderReference,
        offset_ms: f64,
        grant: Option<&crate::live_grant::LiveExecutionGrant<()>>,
    ) -> Result<super::LiveClientDelegationOutcome, super::LiveClientDelegationError> {
        self.writer
            .lock()
            .await
            .reserve_and_admit_client_source(runtime, delegation, offset_ms, grant)
            .await
    }

    pub async fn close(
        &self,
    ) -> Result<crate::live_ledger::transcript::LiveHeadReference, LiveTranscriptWriteError> {
        self.writer.lock().await.close().await
    }
}

#[async_trait::async_trait]
impl LiveContinuousTranscriptIngress for LiveContinuousChannel {
    fn session_id(&self) -> &SessionId {
        &self.session_id
    }
    fn channel_id(&self) -> &LiveChannelId {
        &self.channel_id
    }
    fn receive_clock(&self) -> LiveObservationReceiveClock {
        self.receive_clock.clone()
    }

    async fn apply_provider_control(
        &self,
        event: &meerkat_core::live_execution::observation::ContinuousLiveObservation,
    ) -> Result<ContinuousControlResult, LiveProjectionError> {
        match self
            .writer
            .lock()
            .await
            .observe_provider_control(event)
            .await
            .map_err(|error| self.projection_error(error))?
        {
            super::LiveProviderControlOutcome::Accepted(_) => {
                self.provider_progress.notify_waiters();
                Ok(ContinuousControlResult::Accepted)
            }
            super::LiveProviderControlOutcome::Refused(reason) => {
                Ok(ContinuousControlResult::Refused(reason))
            }
        }
    }

    async fn begin_drain(&self) -> Result<(), LiveProjectionError> {
        self.writer
            .lock()
            .await
            .begin_drain()
            .await
            .map(|_| ())
            .map_err(|error| self.projection_error(error))
    }

    async fn observation_closure_committed(&self) -> Result<bool, LiveProjectionError> {
        self.writer
            .lock()
            .await
            .observation_closure_committed()
            .await
            .map_err(|error| self.projection_error(error))
    }

    async fn apply_voice_observation(
        &self,
        event: &meerkat_core::live_execution::observation::ContinuousLiveObservation,
    ) -> Result<(), LiveProjectionError> {
        use meerkat_core::live_execution::observation::{
            ContinuousLiveObservation as Event, LiveUsageSnapshot,
        };
        let stream_ended = LiveUsageSnapshot::CloseUnconfirmed {
            last_observed_seconds: None,
        };
        let (snapshot, close_ingress) = match event {
            Event::VoiceUsage(snapshot) => (
                snapshot,
                matches!(snapshot, LiveUsageSnapshot::CloseUnconfirmed { .. }),
            ),
            Event::ProviderClosed { usage } => (usage, true),
            Event::ObservationStreamEnded => (&stream_ended, true),
            _ => {
                return Err(LiveProjectionError::Rejected(
                    "not a continuous voice accounting observation".into(),
                ));
            }
        };
        let mut writer = self.writer.lock().await;
        if close_ingress {
            writer
                .close()
                .await
                .map_err(|error| self.projection_error(error))?;
        }
        writer
            .observe_voice_usage(snapshot)
            .await
            .map_err(|error| self.projection_error(error))?;
        self.provider_progress.notify_waiters();
        Ok(())
    }

    async fn append(
        &self,
        receive: LiveObservationReceiveReceipt,
        observation: Result<LiveTranscriptObservation, LiveObservationValueError>,
    ) -> Result<ContinuousTranscriptResult, LiveProjectionError> {
        match self
            .writer
            .lock()
            .await
            .append_with_receive_receipt(receive, observation)
            .await
        {
            Ok(committed) => Ok(ContinuousTranscriptResult::Committed(
                committed.observation().record().clone(),
            )),
            Err(LiveTranscriptWriteError::InvalidObservation(error)) => {
                Ok(ContinuousTranscriptResult::Rejected(error))
            }
            Err(error) => Err(self.projection_error(error)),
        }
    }
}
