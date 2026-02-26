//! Retry policy for transient errors
//!
//! Implements exponential backoff with jitter for LLM API calls.

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Configuration for retry behavior
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryPolicy {
    /// Maximum number of retry attempts
    pub max_retries: u32,
    /// Initial delay before first retry
    pub initial_delay: Duration,
    /// Maximum delay between retries
    pub max_delay: Duration,
    /// Multiplier for exponential backoff
    pub multiplier: f64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(30),
            multiplier: 2.0,
        }
    }
}

impl RetryPolicy {
    /// Create a new retry policy with default settings
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a policy with no retries
    pub fn no_retry() -> Self {
        Self {
            max_retries: 0,
            ..Default::default()
        }
    }

    /// Set maximum retries
    pub fn with_max_retries(mut self, max: u32) -> Self {
        self.max_retries = max;
        self
    }

    /// Set initial delay
    pub fn with_initial_delay(mut self, delay: Duration) -> Self {
        self.initial_delay = delay;
        self
    }

    /// Set maximum delay
    pub fn with_max_delay(mut self, delay: Duration) -> Self {
        self.max_delay = delay;
        self
    }

    /// Set backoff multiplier
    pub fn with_multiplier(mut self, multiplier: f64) -> Self {
        self.multiplier = multiplier;
        self
    }

    /// Calculate delay for a given attempt (0-indexed)
    pub fn delay_for_attempt(&self, attempt: u32) -> Duration {
        if attempt == 0 {
            return Duration::ZERO;
        }

        let base_delay =
            self.initial_delay.as_secs_f64() * self.multiplier.powi(i32::try_from(attempt - 1).unwrap_or(i32::MAX));

        // Apply jitter (±10%)
        let jitter = 1.0 + (rand_jitter() * 0.2 - 0.1);
        let delay_with_jitter = base_delay * jitter;

        // Cap at max delay
        let delay_secs = delay_with_jitter.min(self.max_delay.as_secs_f64());

        Duration::from_secs_f64(delay_secs)
    }

    /// Check if we should retry after the given attempt
    pub fn should_retry(&self, attempt: u32) -> bool {
        attempt < self.max_retries
    }
}

/// Simple pseudo-random jitter (0.0 to 1.0)
fn rand_jitter() -> f64 {
    use crate::time_compat::SystemTime;
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    crate::time_compat::SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .hash(&mut hasher);

    let hash = hasher.finish();
    (hash as f64) / (u64::MAX as f64)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn test_retry_policy_default() {
        let policy = RetryPolicy::default();
        assert_eq!(policy.max_retries, 3);
        assert_eq!(policy.initial_delay, Duration::from_millis(500));
        assert_eq!(policy.max_delay, Duration::from_secs(30));
        assert_eq!(policy.multiplier, 2.0);
    }

    #[test]
    fn test_retry_policy_no_retry() {
        let policy = RetryPolicy::no_retry();
        assert_eq!(policy.max_retries, 0);
        assert!(!policy.should_retry(0));
    }

    #[test]
    fn test_delay_calculation() {
        let policy = RetryPolicy::default();

        // First attempt has no delay
        assert_eq!(policy.delay_for_attempt(0), Duration::ZERO);

        // Subsequent attempts have increasing delays
        let delay1 = policy.delay_for_attempt(1);
        let delay2 = policy.delay_for_attempt(2);
        let delay3 = policy.delay_for_attempt(3);

        // Delays should generally increase (accounting for jitter)
        // With 500ms initial and 2x multiplier:
        // Attempt 1: ~500ms (±10%)
        // Attempt 2: ~1000ms (±10%)
        // Attempt 3: ~2000ms (±10%)

        assert!(delay1.as_millis() >= 400 && delay1.as_millis() <= 600);
        assert!(delay2 > delay1 / 2); // Allow for jitter
        assert!(delay3 > delay2 / 2); // Allow for jitter
    }

    #[test]
    fn test_max_delay_cap() {
        let policy = RetryPolicy::default()
            .with_initial_delay(Duration::from_secs(10))
            .with_max_delay(Duration::from_secs(15));

        // Even with high attempt count, should cap at max_delay
        let delay = policy.delay_for_attempt(10);
        assert!(delay <= Duration::from_secs(17)); // max + 10% jitter
    }

    #[test]
    fn test_should_retry() {
        let policy = RetryPolicy::default().with_max_retries(3);

        assert!(policy.should_retry(0));
        assert!(policy.should_retry(1));
        assert!(policy.should_retry(2));
        assert!(!policy.should_retry(3));
        assert!(!policy.should_retry(4));
    }

    #[test]
    fn test_retry_policy_serialization() {
        let policy = RetryPolicy::default();
        let json = serde_json::to_string(&policy).unwrap();
        let parsed: RetryPolicy = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.max_retries, policy.max_retries);
        assert_eq!(parsed.initial_delay, policy.initial_delay);
        assert_eq!(parsed.max_delay, policy.max_delay);
        assert_eq!(parsed.multiplier, policy.multiplier);
    }
}
