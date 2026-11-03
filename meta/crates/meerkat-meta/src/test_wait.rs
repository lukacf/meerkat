//! Shared polling backstop for tests that guard STRUCTURAL properties.
//!
//! A structural property is one that either happens or never will: a
//! background task rotates a lease, a read model self-heals, a frame lands in
//! the store. Guarding one with `sleep(fixed); assert!(happened)` does not
//! measure correctness - it measures whether the runner got around to polling
//! the task inside that window. Standalone it passes; under full-suite
//! parallelism the same code fails, and the failure is indistinguishable from
//! a real regression, so the suite trains everyone to rerun instead of read.
//!
//! [`poll_until`] replaces that shape. The ceiling is a hang backstop, never a
//! measurement, so it is deliberately generous; on expiry the panic names the
//! thing that never happened rather than reporting a bare timeout.
//!
//! Note what this is NOT for. A test whose assertion is NEGATIVE ("no renewal
//! happened in this window", "the probe stayed idle") is genuinely temporal:
//! the window has to be long enough that a buggy implementation would have
//! acted. Load only makes those safer. Leave them alone.

use std::time::{Duration, Instant};

/// Generous default ceiling for structural waits. Sized so full-suite CPU
/// starvation cannot reach it while a genuine hang still fails the test.
pub(crate) const STRUCTURAL_BACKSTOP: Duration = Duration::from_mins(1);

/// Poll `condition` until it holds, or fail naming `what` when `ceiling`
/// expires.
pub(crate) async fn poll_until<F>(what: &str, ceiling: Duration, mut condition: F)
where
    F: AsyncFnMut() -> bool,
{
    let deadline = Instant::now() + ceiling;
    loop {
        if condition().await {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "never happened within {ceiling:?}: {what}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}
