//! The resume lap's wait at a store fence gap (`wait_out_store_fence_gap`),
//! which the resumable block and the controller re-ask path both take before
//! they go round again: each lap waits the realm's own bounded wait, then
//! the repoll floor while the fence still reads closed, so a lap never spins
//! through a gap, even on a fence whose wait answers at once. An open or a
//! lost fence is not waited for. Unrun until the candidate's validation.

use super::*;
use meerkat_core::host_store::{SelfFence, SelfFenceWindow};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// A fence whose window reads as the row sets it, and whose wait answers at
/// once: the floor alone keeps a lap from spinning. Counts its waits.
struct InstantFence {
    open: bool,
    lost: bool,
    waits: AtomicUsize,
}

impl InstantFence {
    fn new(open: bool, lost: bool) -> Self {
        Self {
            open,
            lost,
            waits: AtomicUsize::new(0),
        }
    }

    fn waits(&self) -> usize {
        self.waits.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl SelfFence for InstantFence {
    fn window(&self) -> SelfFenceWindow {
        SelfFenceWindow {
            elapsed: Duration::from_secs(if self.open { 1 } else { 3 }),
            window: Duration::from_secs(2),
            renew_failed: !self.open,
        }
    }

    async fn dispatch_window(&self) -> SelfFenceWindow {
        self.waits.fetch_add(1, Ordering::SeqCst);
        self.window()
    }

    fn lost(&self) -> bool {
        self.lost
    }
}

#[tokio::test(start_paused = true)]
async fn a_lap_at_a_fence_gap_waits_even_when_the_fence_answers_at_once() {
    let fence = InstantFence::new(false, false);
    let started = tokio::time::Instant::now();
    for _ in 0..3 {
        wait_out_store_fence_gap(&fence).await;
    }
    assert_eq!(fence.waits(), 3, "each lap asks the realm's wait once");
    assert!(
        started.elapsed() >= FENCE_GAP_REPOLL * 3,
        "each lap waits at least the repoll floor, so three laps take three floors: {:?}",
        started.elapsed()
    );
}

#[tokio::test(start_paused = true)]
async fn an_open_or_a_lost_fence_is_not_waited_for() {
    for (fence, case) in [
        (InstantFence::new(true, false), "an open fence"),
        (InstantFence::new(false, true), "a lost fence"),
    ] {
        let started = tokio::time::Instant::now();
        wait_out_store_fence_gap(&fence).await;
        assert_eq!(fence.waits(), 0, "{case} asks no wait");
        assert_eq!(started.elapsed(), Duration::ZERO, "{case} returns at once");
    }
}
