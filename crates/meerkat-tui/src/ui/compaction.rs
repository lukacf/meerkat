//! Context-compaction policy on Meerkat's native auto-compaction.
//!
//! The engine compacts a session itself: before a model call its compactor
//! summarises older history once the context crosses the session's
//! threshold (`SessionBuildOptions::auto_compact_threshold_override`, read
//! when the agent is built), indexes the discarded messages into semantic
//! memory and reports `CompactionStarted`/`Completed`/`Failed`. The TUI
//! only chooses that threshold and watches the gauge:
//!
//! - **warn** (default): the native trigger is off (threshold
//!   [`NonZeroU64::MAX`]; upstream still compacts to recover a request the
//!   provider refused as too large). One warning when usage crosses
//!   `compact_warn_percent` (re-armed once usage drops below it or a
//!   compaction succeeds), and a warning after every turn at or above
//!   `compact_auto_percent`.
//! - **auto**: the threshold is `compact_auto_percent` of the model's
//!   context window ([`CompactionPolicy::native_threshold`]); the engine
//!   compacts before the first model call that would cross it.
//!
//! `/compact` reports the policy; `/compact now` rebuilds the idle session
//! with a one-turn threshold of one token ([`COMPACT_NOW_THRESHOLD`]), so
//! the next turn's first model call compacts first; after that turn the
//! session is rebuilt on the policy's threshold again. Upstream exposes no
//! on-demand compaction call, hence the rebuild. A rebuilt agent does not
//! keep the provider's last input count, so the one-turn threshold cannot
//! be a fraction of the current context: a long tool loop in that turn can
//! compact again after upstream's cadence (`min_turns_between_compactions`
//! model calls, 3 by default).
//!
//! Upstream publishes `CompactionCompleted` when the runtime finalizes the
//! compaction's semantic-memory projection, which happens when the session
//! next runs; until then the gauge shows the turn's own usage.
//!
//! When the model's context window is unknown, fixed token thresholds apply
//! instead: [`FALLBACK_WARN_TOKENS`] and [`FALLBACK_AUTO_TOKENS`].
//!
//! Everything here is pure; [`super::App`] owns a [`CompactionGauge`].

use std::num::NonZeroU64;

use serde::{Deserialize, Serialize};

use super::render::fmt_ctx_tokens;

/// Default `compact_warn_percent`.
pub const DEFAULT_WARN_PERCENT: u8 = 75;
/// Default `compact_auto_percent`.
pub const DEFAULT_AUTO_PERCENT: u8 = 85;
/// Tokens at which to warn when the context window is unknown.
pub const FALLBACK_WARN_TOKENS: u64 = 120_000;
/// Tokens at which to compact (auto) or warn every turn (warn) when the
/// context window is unknown.
pub const FALLBACK_AUTO_TOKENS: u64 = 150_000;

/// What happens when the context fills up.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CompactMode {
    /// Warn; compaction only on `/compact now`.
    #[default]
    Warn,
    /// Let the engine compact at `compact_auto_percent`.
    Auto,
}

/// Thresholds and mode for [`CompactionGauge`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompactionPolicy {
    /// Warn or compact automatically.
    pub mode: CompactMode,
    /// Usage (percent of the window) at which to warn once.
    pub warn_percent: u8,
    /// Usage (percent of the window) at which the engine compacts (auto
    /// mode) or the gauge warns after every turn (warn mode). Never below
    /// `warn_percent`.
    pub auto_percent: u8,
}

impl Default for CompactionPolicy {
    fn default() -> Self {
        Self {
            mode: CompactMode::Warn,
            warn_percent: DEFAULT_WARN_PERCENT,
            auto_percent: DEFAULT_AUTO_PERCENT,
        }
    }
}

impl CompactionPolicy {
    /// Clamp `warn_percent` to `1..=100` and `auto_percent` to
    /// `warn_percent..=100`. Returns the clamped policy and whether anything
    /// changed.
    #[must_use]
    pub fn normalized(self) -> (Self, bool) {
        let warn_percent = self.warn_percent.clamp(1, 100);
        let auto_percent = self.auto_percent.clamp(warn_percent, 100);
        let fixed = Self {
            mode: self.mode,
            warn_percent,
            auto_percent,
        };
        (fixed, fixed != self)
    }

    /// Tokens at `auto_percent` of `window` (or [`FALLBACK_AUTO_TOKENS`]
    /// when the window is unknown or zero).
    #[must_use]
    pub fn auto_tokens(&self, window: Option<u64>) -> u64 {
        match window.filter(|w| *w > 0) {
            Some(window) => window.saturating_mul(u64::from(self.auto_percent)) / 100,
            None => FALLBACK_AUTO_TOKENS,
        }
    }

    /// The native auto-compaction threshold for a session whose model has
    /// `window` tokens of context: [`NonZeroU64::MAX`] (never by size) in
    /// warn mode, [`Self::auto_tokens`] in auto mode.
    #[must_use]
    pub fn native_threshold(&self, window: Option<u64>) -> NonZeroU64 {
        match self.mode {
            CompactMode::Warn => NonZeroU64::MAX,
            CompactMode::Auto => {
                NonZeroU64::new(self.auto_tokens(window)).unwrap_or(NonZeroU64::MIN)
            }
        }
    }

    /// What `/compact` says about the policy.
    #[must_use]
    pub fn describe(&self, window: Option<u64>) -> String {
        let at = match window.filter(|w| *w > 0) {
            Some(_) => format!(
                "{}% of the context window ({} tokens)",
                self.auto_percent,
                fmt_ctx_tokens(self.auto_tokens(window))
            ),
            None => format!("{} tokens", fmt_ctx_tokens(FALLBACK_AUTO_TOKENS)),
        };
        match self.mode {
            CompactMode::Auto => format!(
                "auto-compaction is on: the runtime compacts the context before a model call once it reaches {at}; /compact now compacts before the next turn's first model call"
            ),
            CompactMode::Warn => format!(
                "auto-compaction is off (compact = \"warn\"; the warning repeats from {at}); /compact now compacts before the next turn's first model call, compact = \"auto\" in tui.toml or --auto-compact compacts automatically"
            ),
        }
    }

    fn zone(&self, used: u64, window: Option<u64>) -> Zone {
        let (warn, auto) = match window {
            Some(window) => {
                let percent = used.saturating_mul(100) / window;
                (
                    percent >= u64::from(self.warn_percent),
                    percent >= u64::from(self.auto_percent),
                )
            }
            None => (used >= FALLBACK_WARN_TOKENS, used >= FALLBACK_AUTO_TOKENS),
        };
        if auto {
            Zone::Auto
        } else if warn {
            Zone::Warn
        } else {
            Zone::Below
        }
    }
}

/// The one-turn threshold `/compact now` arms: any history crosses it, so
/// the next turn's first model call compacts (upstream never compacts on a
/// session's very first model call).
pub const COMPACT_NOW_THRESHOLD: NonZeroU64 = NonZeroU64::MIN;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Zone {
    Below,
    Warn,
    Auto,
}

/// Why `/compact now` could not arm a compaction, as reported in
/// [`super::Msg::CompactArmed`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CompactFailure {
    /// No turn has reported its context yet: nothing to compact.
    #[error("nothing to compact yet")]
    NothingYet,
    /// A turn is running (the session can only be rebuilt between turns).
    #[error("a turn is running; interrupt it first (Ctrl-C)")]
    Busy,
    /// The session could not be rebuilt with the one-turn threshold.
    #[error("could not rebuild the session: {0}")]
    Rebuild(String),
}

/// Tracks the context gauge across turns and applies a [`CompactionPolicy`].
#[derive(Debug, Clone, Default)]
pub struct CompactionGauge {
    policy: CompactionPolicy,
    /// A warn-threshold notice was shown and usage has not dropped below it.
    warned: bool,
}

impl CompactionGauge {
    /// A gauge applying `policy` (normalized).
    #[must_use]
    pub fn new(policy: CompactionPolicy) -> Self {
        Self {
            policy: policy.normalized().0,
            warned: false,
        }
    }

    /// The policy in effect.
    #[must_use]
    pub fn policy(&self) -> CompactionPolicy {
        self.policy
    }

    /// A turn finished with `used` tokens in context and a model window of
    /// `window` tokens (`None` or `0` when unknown). Returns the warning to
    /// show, if any. Auto mode never warns: the engine compacts.
    pub fn after_turn(&mut self, used: u64, window: Option<u64>) -> Option<String> {
        let window = window.filter(|w| *w > 0);
        match self.policy.zone(used, window) {
            Zone::Below => {
                self.warned = false;
                None
            }
            _ if self.policy.mode == CompactMode::Auto => None,
            Zone::Warn if self.warned => None,
            Zone::Warn => {
                self.warned = true;
                Some(format!(
                    "{}; run /compact now to free space",
                    usage_text(used, window)
                ))
            }
            Zone::Auto => {
                self.warned = true;
                Some(format!(
                    "{}; auto-compaction is off: run /compact now, or set compact = \"auto\" in tui.toml (--auto-compact)",
                    usage_text(used, window)
                ))
            }
        }
    }

    /// A compaction completed: the next crossing warns again.
    pub fn compacted(&mut self) {
        self.warned = false;
    }
}

/// `context 80% full (160k/200k tokens)` or `context at 130k tokens`.
fn usage_text(used: u64, window: Option<u64>) -> String {
    match window {
        Some(window) => format!(
            "context {}% full ({}/{} tokens)",
            used.saturating_mul(100) / window,
            fmt_ctx_tokens(used),
            fmt_ctx_tokens(window)
        ),
        None => format!("context at {} tokens", fmt_ctx_tokens(used)),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    const W: Option<u64> = Some(100_000);

    fn auto() -> CompactionPolicy {
        CompactionPolicy {
            mode: CompactMode::Auto,
            ..CompactionPolicy::default()
        }
    }

    #[test]
    fn normalization_clamps_auto_to_warn() {
        let (p, changed) = CompactionPolicy {
            mode: CompactMode::Warn,
            warn_percent: 90,
            auto_percent: 80,
        }
        .normalized();
        assert!(changed);
        assert_eq!((p.warn_percent, p.auto_percent), (90, 90));
        let (p, changed) = CompactionPolicy {
            mode: CompactMode::Warn,
            warn_percent: 0,
            auto_percent: 200,
        }
        .normalized();
        assert!(changed);
        assert_eq!((p.warn_percent, p.auto_percent), (1, 100));
        assert!(!CompactionPolicy::default().normalized().1);
    }

    #[test]
    fn native_threshold_follows_the_mode() {
        assert_eq!(
            CompactionPolicy::default().native_threshold(W),
            NonZeroU64::MAX
        );
        assert_eq!(auto().native_threshold(W).get(), 85_000);
        assert_eq!(auto().native_threshold(None).get(), FALLBACK_AUTO_TOKENS);
        assert_eq!(auto().native_threshold(Some(0)).get(), FALLBACK_AUTO_TOKENS);
        assert_eq!(auto().native_threshold(Some(1)).get(), 1);
        assert_eq!(COMPACT_NOW_THRESHOLD.get(), 1);
    }

    #[test]
    fn describe_names_the_threshold_and_the_commands() {
        let text = auto().describe(W);
        assert!(
            text.contains("85% of the context window (85k tokens)"),
            "{text}"
        );
        assert!(text.contains("/compact now"), "{text}");
        let text = CompactionPolicy::default().describe(None);
        assert!(text.contains("auto-compaction is off"), "{text}");
        assert!(text.contains("150k tokens"), "{text}");
        assert!(text.contains("--auto-compact"), "{text}");
    }

    #[test]
    fn warn_mode_warns_once_then_every_turn_at_auto() {
        let mut g = CompactionGauge::default();
        assert_eq!(g.after_turn(70_000, W), None);
        assert_eq!(
            g.after_turn(76_000, W).as_deref(),
            Some("context 76% full (76k/100k tokens); run /compact now to free space")
        );
        assert_eq!(g.after_turn(80_000, W), None);
        for used in [85_000, 86_000] {
            let text = g.after_turn(used, W).expect("a warning");
            assert!(text.contains("auto-compaction is off"), "{text}");
        }
        // Dropping below re-arms the single warning.
        assert_eq!(g.after_turn(80_000, W), None);
        assert_eq!(g.after_turn(50_000, W), None);
        assert!(g.after_turn(75_000, W).is_some());
    }

    #[test]
    fn a_compaction_rearms_the_warning() {
        let mut g = CompactionGauge::default();
        assert!(g.after_turn(76_000, W).is_some());
        assert!(g.after_turn(77_000, W).is_none());
        g.compacted();
        assert!(g.after_turn(77_000, W).is_some());
    }

    #[test]
    fn auto_mode_never_warns() {
        let mut g = CompactionGauge::new(auto());
        for used in [76_000, 90_000, 99_000] {
            assert_eq!(g.after_turn(used, W), None);
        }
    }

    #[test]
    fn unknown_window_uses_token_thresholds() {
        let mut g = CompactionGauge::default();
        assert_eq!(g.after_turn(119_999, None), None);
        assert_eq!(
            g.after_turn(120_000, Some(0)).as_deref(),
            Some("context at 120k tokens; run /compact now to free space")
        );
        assert_eq!(g.after_turn(149_999, None), None);
        assert!(
            g.after_turn(150_000, None)
                .is_some_and(|t| t.contains("auto-compaction is off"))
        );
    }

    #[test]
    fn mode_parses_lowercase() {
        #[derive(Deserialize)]
        struct T {
            m: CompactMode,
        }
        let t: T = toml::from_str("m = \"auto\"").unwrap();
        assert_eq!(t.m, CompactMode::Auto);
        assert!(toml::from_str::<T>("m = \"never\"").is_err());
    }
}
