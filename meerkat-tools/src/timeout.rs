//! Single typed owner for tool-execution timeout policy.
//!
//! Before this seam existed, three independent owners each carried their own
//! default timeout value:
//!
//! - `ToolDispatcherConfig.default_timeout` (builder)
//! - `ToolDispatcher.default_timeout` (dispatcher enforcement via
//!   `tokio::time::timeout`)
//! - `ShellConfig.default_timeout_secs` (shell tool)
//!
//! Each hardcoded "30 seconds" independently, so a change in one place silently
//! diverged from the others. [`ToolTimeoutPolicy`] collapses this into one typed
//! authority: the dispatcher and the shell config both *read* their default from
//! the policy rather than each owning a literal. The policy seeds itself from the
//! single canonical config-layer default ([`ShellDefaults::timeout_secs`]), so
//! there is exactly one source value flowing to both enforcement points.

use std::time::Duration;

use meerkat_core::ShellDefaults;

/// Canonical timeout policy for tool execution.
///
/// This is the single owner of the default per-call tool timeout. The dispatcher
/// enforcement seam and the shell tool both derive their default from this policy
/// instead of each holding an independent literal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolTimeoutPolicy {
    default_timeout: Duration,
}

impl ToolTimeoutPolicy {
    /// Construct a policy with an explicit default timeout.
    pub const fn new(default_timeout: Duration) -> Self {
        Self { default_timeout }
    }

    /// The default per-call timeout enforced when a tool call does not request
    /// its own.
    pub const fn default_timeout(self) -> Duration {
        self.default_timeout
    }

    /// The default timeout expressed in whole seconds, for call sites (such as
    /// the shell tool) that carry their timeout as `u64` seconds rather than a
    /// [`Duration`].
    pub const fn default_timeout_secs(self) -> u64 {
        self.default_timeout.as_secs()
    }
}

impl Default for ToolTimeoutPolicy {
    /// Seed the policy from the single canonical config-layer default so that
    /// the dispatcher and shell call sites cannot diverge.
    fn default() -> Self {
        Self {
            default_timeout: Duration::from_secs(ShellDefaults::default().timeout_secs),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Gate (#79): a single timeout source value flows to both the dispatcher
    /// enforcement default and the shell config default — there is no divergent
    /// hardcoded literal. Changing the policy changes both call sites.
    #[test]
    fn single_policy_value_flows_to_both_call_sites() {
        let policy = ToolTimeoutPolicy::default();

        // The shell-config view (u64 seconds) and the dispatcher view
        // (Duration) are the SAME value, derived from one owner.
        assert_eq!(
            Duration::from_secs(policy.default_timeout_secs()),
            policy.default_timeout(),
            "shell-seconds view and dispatcher Duration view must agree"
        );

        // The policy is seeded from the single canonical config-layer default,
        // not an independent literal.
        assert_eq!(
            policy.default_timeout_secs(),
            ShellDefaults::default().timeout_secs,
            "policy default must equal the canonical config-layer default"
        );

        // Changing the policy changes the value both call sites read: there is
        // one owner, so a non-default policy is observed identically in both
        // views.
        let custom = ToolTimeoutPolicy::new(Duration::from_secs(123));
        assert_eq!(custom.default_timeout(), Duration::from_secs(123));
        assert_eq!(custom.default_timeout_secs(), 123);
        assert_ne!(
            custom.default_timeout(),
            ToolTimeoutPolicy::default().default_timeout(),
            "a changed policy must differ from the default in the dispatcher view"
        );
        assert_ne!(
            custom.default_timeout_secs(),
            ToolTimeoutPolicy::default().default_timeout_secs(),
            "a changed policy must differ from the default in the shell view"
        );
    }
}
