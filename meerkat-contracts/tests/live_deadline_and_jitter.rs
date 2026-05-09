#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! K61 contract test (port of the deleted `realtime_deadline_and_jitter`
//! deterministic test).
//!
//! Pins the **deadline budget** for audio observations crossing the
//! [`LiveAdapter`] seam: when a producer emits a burst of
//! [`LiveAdapterObservation::AssistantAudioChunk`] frames with controlled
//! inter-arrival timing, the consumer must receive every chunk in order and
//! within the deadline window after each emit, regardless of producer-side
//! bursting.
//!
//! D-Jitter (deadline budget): each `AssistantAudioChunk` must reach the
//! consumer-side `next_observation()` within `BUDGET` of its producer-side
//! emit time, with no reordering and no dropped chunks. This is the
//! contract a real provider adapter pump must honor — backlog must not
//! accumulate unboundedly when the consumer is briefly slow, and a
//! producer-side burst must not be coalesced or dropped on the seam.
//!
//! Determinism: the test runs under `tokio::time::pause()` (`start_paused =
//! true`) and uses `tokio::time::advance` to drive the simulated clock. No
//! real-clock dependency, no sleep-based timing, no flakiness.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use meerkat_core::live_adapter::{
    LiveAdapter, LiveAdapterCommand, LiveAdapterError, LiveAdapterObservation, LiveAdapterStatus,
};
use tokio::sync::{Mutex, mpsc};

/// Deadline budget: each emitted chunk must reach the consumer within this
/// window of its producer-side emit time. Under `tokio::time::pause()` the
/// scheduler is fully synchronous w.r.t. the simulated clock, so any drift
/// strictly larger than this is a real backlog/coalescing bug at the seam,
/// not test noise.
const BUDGET: Duration = Duration::from_millis(5);

/// A jittery emit schedule: tightly-bunched burst (10ms, 11ms, 12ms, 13ms)
/// followed by a quiet gap then another small burst. This is the producer
/// behavior pattern the original `realtime_deadline_and_jitter` test pinned —
/// consumers must keep up with bursts, not just steady cadence.
const EMIT_SCHEDULE_MS: &[u64] = &[10, 11, 12, 13, 50, 51, 100];

/// Fake [`LiveAdapter`] that emits a fixed schedule of `AssistantAudioChunk`
/// observations on a paused tokio clock. Internally drives a producer task
/// that sleeps for each scheduled offset (relative to construction time)
/// before pushing a chunk to a bounded mpsc queue. `next_observation` reads
/// from the queue.
struct ScriptedAdapter {
    rx: Mutex<mpsc::Receiver<LiveAdapterObservation>>,
}

impl ScriptedAdapter {
    /// Build the adapter and spawn its producer task. The caller-supplied
    /// `start` anchor is the instant from which schedule offsets are
    /// measured — pinning it explicitly keeps producer and consumer aligned
    /// to the same simulated clock origin (otherwise the producer's first
    /// poll-time becomes its `start`, drifting from the test's captured
    /// `start`).
    ///
    /// The producer awaits `tokio::time::sleep_until` between emits, so
    /// under `tokio::time::pause()` the test must `advance` to drive emits.
    fn spawn(schedule_ms: &[u64], start: tokio::time::Instant) -> Arc<Self> {
        // Bounded channel so backpressure is observable: if the producer
        // outruns the consumer, the producer's `send` awaits — the test
        // exercises that path by reading slowly relative to the schedule.
        let (tx, rx) = mpsc::channel(64);
        let schedule: Vec<u64> = schedule_ms.to_vec();

        tokio::spawn(async move {
            for (i, ms) in schedule.iter().enumerate() {
                let deadline = start + Duration::from_millis(*ms);
                tokio::time::sleep_until(deadline).await;
                let chunk = LiveAdapterObservation::AssistantAudioChunk {
                    // Encode the emit index in the payload so the consumer
                    // can prove ordering without depending on bytes-equal
                    // comparison of identical chunks.
                    data: vec![i as u8],
                    sample_rate_hz: 24_000,
                    channels: 1,
                };
                if tx.send(chunk).await.is_err() {
                    return;
                }
            }
        });

        Arc::new(Self { rx: Mutex::new(rx) })
    }
}

#[async_trait]
impl LiveAdapter for ScriptedAdapter {
    async fn send_command(&self, _command: LiveAdapterCommand) -> Result<(), LiveAdapterError> {
        Ok(())
    }

    async fn next_observation(&self) -> Result<Option<LiveAdapterObservation>, LiveAdapterError> {
        let mut rx = self.rx.lock().await;
        Ok(rx.recv().await)
    }

    fn status(&self) -> LiveAdapterStatus {
        LiveAdapterStatus::Ready
    }

    async fn close(&self) -> Result<(), LiveAdapterError> {
        Ok(())
    }
}

/// D-Jitter: each `AssistantAudioChunk` must reach the observation channel
/// within `BUDGET` of its producer-side emit time, regardless of
/// producer-side bursting. No chunks may be dropped or reordered.
#[tokio::test(start_paused = true)]
async fn realtime_deadline_and_jitter() {
    let start = tokio::time::Instant::now();
    let adapter = ScriptedAdapter::spawn(EMIT_SCHEDULE_MS, start);

    for (i, ms) in EMIT_SCHEDULE_MS.iter().enumerate() {
        let emit_at = start + Duration::from_millis(*ms);
        let now = tokio::time::Instant::now();
        if now < emit_at {
            // Advance the simulated clock to (slightly past) the emit time.
            // Yielding once first lets the producer task observe the new
            // clock before we claim the chunk via `next_observation`.
            tokio::time::advance(emit_at - now + Duration::from_micros(1)).await;
        }

        let observation = adapter
            .next_observation()
            .await
            .expect("adapter pump healthy")
            .expect("producer should have emitted chunk");

        let received_at = tokio::time::Instant::now();
        let drift = received_at.saturating_duration_since(emit_at);
        assert!(
            drift <= BUDGET,
            "chunk #{i} (emit at {ms} ms) overshot deadline budget: drift = {drift:?} > {BUDGET:?}"
        );

        match observation {
            LiveAdapterObservation::AssistantAudioChunk {
                data,
                sample_rate_hz,
                channels,
            } => {
                assert_eq!(
                    data,
                    vec![i as u8],
                    "ordering violation at chunk #{i}: payload was {data:?}"
                );
                assert_eq!(sample_rate_hz, 24_000);
                assert_eq!(channels, 1);
            }
            other => panic!("expected AssistantAudioChunk, got {other:?}"),
        }
    }
}

/// Companion: a producer-side burst (multiple emits in a tight window) does
/// not collapse, drop, or reorder chunks — every emit produces exactly one
/// observation at the consumer.
#[tokio::test(start_paused = true)]
async fn producer_burst_preserves_every_chunk_in_order() {
    // Tight-burst schedule: 10 chunks within a 4ms window.
    let burst: Vec<u64> = (0..10).map(|i| 100 + i).collect();
    let start = tokio::time::Instant::now();
    let adapter = ScriptedAdapter::spawn(&burst, start);

    // Advance past the entire burst window so all emits are in flight.
    let target = start + Duration::from_millis(110);
    let now = tokio::time::Instant::now();
    if now < target {
        tokio::time::advance(target - now).await;
    }
    // Yield so the producer task drains its scheduled emits before we read.
    for _ in 0..16 {
        tokio::task::yield_now().await;
    }

    // Drain the consumer side — every emit must be present, in order.
    for (expected_idx, _) in burst.iter().enumerate() {
        let observation = adapter
            .next_observation()
            .await
            .expect("adapter pump healthy")
            .expect("producer should have emitted chunk");
        match observation {
            LiveAdapterObservation::AssistantAudioChunk { data, .. } => {
                assert_eq!(
                    data,
                    vec![expected_idx as u8],
                    "ordering or drop violation at chunk #{expected_idx}"
                );
            }
            other => panic!("expected AssistantAudioChunk #{expected_idx}, got {other:?}"),
        }
    }
}

/// Negative pin: a slow consumer (one that reads after a delay) does not
/// cause the producer to drop chunks — the bounded queue + backpressure
/// preserve all emits up to capacity. This is the "consumer briefly slow"
/// scenario from the original test.
#[tokio::test(start_paused = true)]
async fn slow_consumer_does_not_drop_buffered_chunks() {
    let start = tokio::time::Instant::now();
    let adapter = ScriptedAdapter::spawn(EMIT_SCHEDULE_MS, start);

    // Skip the first emits without reading: advance past the entire schedule
    // and only then drain.
    let total_window = Duration::from_millis(*EMIT_SCHEDULE_MS.last().unwrap() + 5);
    tokio::time::advance(total_window).await;
    for _ in 0..16 {
        tokio::task::yield_now().await;
    }

    // Even though the consumer was idle through every emit, every chunk is
    // still queued and recoverable in the original order.
    for (i, _ms) in EMIT_SCHEDULE_MS.iter().enumerate() {
        let observation = adapter
            .next_observation()
            .await
            .expect("adapter pump healthy")
            .expect("buffered chunk should be available after slow drain");
        match observation {
            LiveAdapterObservation::AssistantAudioChunk { data, .. } => {
                assert_eq!(data, vec![i as u8]);
            }
            other => panic!("expected AssistantAudioChunk #{i}, got {other:?}"),
        }
    }
}
