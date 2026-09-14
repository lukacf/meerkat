use super::*;
use meerkat_core::live_execution::observation::ContinuousLiveObservation;
use meerkat_core::live_observation::{
    LiveObservationReceiveClock, LiveObservationReceiveReceipt, LiveObservationValueError,
    LiveTranscriptObservation,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::Notify;

struct ClosureProbe {
    session: SessionId,
    channel: LiveChannelId,
    clock: LiveObservationReceiveClock,
    begin_count: AtomicUsize,
    closed: AtomicBool,
    changed: Notify,
}

#[async_trait::async_trait]
impl LiveContinuousTranscriptIngress for ClosureProbe {
    fn session_id(&self) -> &SessionId {
        &self.session
    }
    fn channel_id(&self) -> &LiveChannelId {
        &self.channel
    }
    fn receive_clock(&self) -> LiveObservationReceiveClock {
        self.clock.clone()
    }
    async fn begin_drain(&self) -> Result<(), LiveProjectionError> {
        self.begin_count.fetch_add(1, Ordering::SeqCst);
        self.changed.notify_waiters();
        Ok(())
    }
    async fn observation_closure_committed(&self) -> Result<bool, LiveProjectionError> {
        Ok(self.closed.load(Ordering::SeqCst))
    }
    async fn apply_voice_observation(
        &self,
        event: &ContinuousLiveObservation,
    ) -> Result<(), LiveProjectionError> {
        if !matches!(event, ContinuousLiveObservation::ObservationStreamEnded) {
            return Err(LiveProjectionError::Rejected(
                "unexpected probe event".into(),
            ));
        }
        self.closed.store(true, Ordering::SeqCst);
        self.changed.notify_waiters();
        Ok(())
    }
    async fn append(
        &self,
        _: LiveObservationReceiveReceipt,
        _: Result<LiveTranscriptObservation, LiveObservationValueError>,
    ) -> Result<ContinuousTranscriptResult, LiveProjectionError> {
        Err(LiveProjectionError::Rejected(
            "probe has no transcript".into(),
        ))
    }
}

struct GatedClose {
    calls: AtomicUsize,
    changed: Notify,
    released: Notify,
}

#[async_trait::async_trait]
impl LiveAdapter for GatedClose {
    async fn send_command(&self, _: LiveAdapterCommand) -> Result<(), LiveAdapterError> {
        Err(LiveAdapterError::Closed)
    }
    async fn next_observation(&self) -> Result<Option<LiveAdapterObservation>, LiveAdapterError> {
        Ok(None)
    }
    fn status(&self) -> LiveAdapterStatus {
        LiveAdapterStatus::Closing
    }
    async fn close(&self) -> Result<(), LiveAdapterError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.changed.notify_waiters();
        self.released.notified().await;
        Ok(())
    }
}

async fn wait_until(
    notify: &Notify,
    predicate: impl Fn() -> bool,
) -> Result<(), tokio::time::error::Elapsed> {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let changed = notify.notified();
            if predicate() {
                return;
            }
            changed.await;
        }
    })
    .await
}

#[tokio::test]
async fn external_close_joins_pump_even_after_native_observation_closure()
-> Result<(), Box<dyn std::error::Error>> {
    let host = Arc::new(LiveAdapterHost::new(Arc::new(NoOpProjectionSink)));
    let session = SessionId::new();
    let channel = host
        .open_channel_with_generated_test_machine_authority(session.clone())
        .await?;
    let ingress = Arc::new(ClosureProbe {
        session,
        channel: channel.clone(),
        clock: Default::default(),
        begin_count: AtomicUsize::new(0),
        closed: AtomicBool::new(false),
        changed: Notify::new(),
    });
    let adapter = Arc::new(GatedClose {
        calls: AtomicUsize::new(0),
        changed: Notify::new(),
        released: Notify::new(),
    });
    host.attach_continuous_adapter(&channel, adapter.clone(), ingress.clone())
        .await?;
    let mut pump = host.claim_observation_pump(&channel).await?;
    let active = tokio::spawn(async move { pump.drain(Duration::from_secs(3)).await });
    wait_until(&ingress.changed, || ingress.closed.load(Ordering::SeqCst)).await?;
    wait_until(&adapter.changed, || {
        adapter.calls.load(Ordering::SeqCst) > 0
    })
    .await?;
    let observation = host.reserve_channel_close_observation(&channel).await?;
    let external = {
        let host = Arc::clone(&host);
        tokio::spawn(async move { host.prepare_channel_physical_close(&observation).await })
    };
    wait_until(&ingress.changed, || {
        ingress.begin_count.load(Ordering::SeqCst) >= 2
    })
    .await?;
    let calls = adapter.calls.load(Ordering::SeqCst);
    adapter.released.notify_waiters();
    active.await??;
    external.await??;
    assert_eq!(
        calls, 1,
        "final usage is not permission to race the existing physical closer"
    );
    Ok(())
}
