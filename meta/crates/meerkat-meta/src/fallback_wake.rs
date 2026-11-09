//! The last-resort wake of Meta's agent-memory member-event observer.
//!
//! The observer is driven by typed events (a mob machine-state change). A
//! few transient causes have no typed signal that says they cleared (a
//! member that cannot be subscribed yet, a member stream that closes as
//! soon as it opens). For those alone the observer arms this backoff: its
//! deadline is a fallback wake, never the mechanism. It starts at
//! [`FALLBACK_WAKE_BASE`], doubles per consecutive failure up to
//! [`FALLBACK_WAKE_MAX`], and the observer drops it on the first success.

use std::time::Duration;

/// First fallback wake after a transient failure with no typed signal.
pub(crate) const FALLBACK_WAKE_BASE: Duration = Duration::from_secs(1);
/// Ceiling for the doubling fallback wake.
pub(crate) const FALLBACK_WAKE_MAX: Duration = Duration::from_secs(30);

/// Exponential fallback wake for one transient cause.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct FallbackWake {
    consecutive_failures: u32,
    next_wake: Option<tokio::time::Instant>,
}

impl FallbackWake {
    /// The delay armed after `consecutive_failures` earlier failures.
    pub(crate) fn delay(consecutive_failures: u32) -> Duration {
        FALLBACK_WAKE_BASE
            .saturating_mul(1u32 << consecutive_failures.min(5))
            .min(FALLBACK_WAKE_MAX)
    }

    /// Record a transient failure and arm the next wake; returns whether it
    /// is the first failure in a row.
    pub(crate) fn record_failure(&mut self, now: tokio::time::Instant) -> bool {
        self.next_wake = Some(now + Self::delay(self.consecutive_failures));
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        self.consecutive_failures == 1
    }

    /// The armed fallback wake, if any.
    pub(crate) fn next_wake(&self) -> Option<tokio::time::Instant> {
        self.next_wake
    }

    /// Whether an attempt is allowed now (nothing armed, or the wake is due).
    pub(crate) fn is_due(&self, now: tokio::time::Instant) -> bool {
        self.next_wake.is_none_or(|wake| now >= wake)
    }
}

/// Sleep until `wake`, or forever when nothing is armed.
pub(crate) async fn sleep_until(wake: Option<tokio::time::Instant>) {
    match wake {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_wake_doubles_to_the_cap() {
        let now = tokio::time::Instant::now();
        let mut wake = FallbackWake::default();
        assert!(wake.is_due(now));
        assert!(wake.record_failure(now));
        assert_eq!(wake.next_wake(), Some(now + Duration::from_secs(1)));
        assert!(!wake.is_due(now));
        for _ in 0..10 {
            assert!(!wake.record_failure(now));
        }
        assert_eq!(wake.next_wake(), Some(now + FALLBACK_WAKE_MAX));
        assert!(wake.is_due(now + FALLBACK_WAKE_MAX));
    }
}
