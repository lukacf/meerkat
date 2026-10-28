//! Owned configuration of member wakes.

use std::time::Duration;

/// Backoff of the retry that wakes a member whose boot work is still owed
/// after its boot wake ended retryably (an unavailable resume gate, a host
/// that asked to retry its build preparation, the wake bound): the mob wakes
/// it again after `initial`, doubling up to `max`, until it is live, its
/// restore fails for good, or its obligation is gone. A delivery to the
/// member meanwhile wakes it at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemberWakeRetryBackoff {
    pub initial: Duration,
    pub max: Duration,
}

impl MemberWakeRetryBackoff {
    pub const DEFAULT_INITIAL: Duration = Duration::from_secs(1);
    pub const DEFAULT_MAX: Duration = Duration::from_secs(60);

    /// The delay before retry number `retry` (0 is the first).
    pub fn delay(&self, retry: u32) -> Duration {
        let factor = 1u32.checked_shl(retry.min(31)).unwrap_or(u32::MAX);
        self.initial
            .checked_mul(factor)
            .unwrap_or(self.max)
            .min(self.max)
    }
}

impl Default for MemberWakeRetryBackoff {
    fn default() -> Self {
        Self {
            initial: Self::DEFAULT_INITIAL,
            max: Self::DEFAULT_MAX,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owed_wake_retry_backoff_doubles_up_to_its_cap() {
        let backoff = MemberWakeRetryBackoff::default();
        assert_eq!(backoff.delay(0), Duration::from_secs(1));
        assert_eq!(backoff.delay(1), Duration::from_secs(2));
        assert_eq!(backoff.delay(5), Duration::from_secs(32));
        assert_eq!(backoff.delay(6), Duration::from_secs(60));
        assert_eq!(backoff.delay(40), Duration::from_secs(60));
    }
}
