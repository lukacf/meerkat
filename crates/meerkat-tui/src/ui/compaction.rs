//! TUI-side context-compaction policy.
//!
//! The engine's own auto-compaction is disabled at bootstrap (its compactor
//! keeps whole user turns, which in agentic sessions often discards nothing
//! and re-triggers every few tool rounds). Instead, after every finished turn
//! the UI reads the context gauge and either warns or runs `/compact`:
//!
//! - **warn** (default): one warning when usage crosses `warn_percent`
//!   (re-armed once usage drops below it again or a compaction succeeds), and
//!   a warning after every turn at or above `auto_percent`.
//! - **auto**: at or above `auto_percent`, when the session is idle, emit
//!   [`super::Effect::Compact`]. After a compaction that gained nothing, no
//!   retry until usage has grown by [`RETRY_GROWTH_PERCENT`] points.
//!
//! When the model's context window is unknown, fixed token thresholds apply
//! instead: [`FALLBACK_WARN_TOKENS`] and [`FALLBACK_AUTO_TOKENS`].
//!
//! Everything here is pure; [`super::App`] owns a [`CompactionGauge`].

use serde::{Deserialize, Serialize};

use super::render::fmt_ctx_tokens;

/// Default `compact_warn_percent`.
pub const DEFAULT_WARN_PERCENT: u8 = 75;
/// Default `compact_auto_percent`.
pub const DEFAULT_AUTO_PERCENT: u8 = 85;
/// Tokens at which to warn when the context window is unknown.
pub const FALLBACK_WARN_TOKENS: u64 = 120_000;
/// Tokens at which to auto-compact (or warn every turn) when the context
/// window is unknown.
pub const FALLBACK_AUTO_TOKENS: u64 = 150_000;
/// Growth in context usage (percentage points of the window, or this percent
/// of [`FALLBACK_AUTO_TOKENS`] when the window is unknown) required before
/// retrying a compaction that gained nothing.
pub const RETRY_GROWTH_PERCENT: u8 = 10;

/// What happens when the context fills up.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CompactMode {
    /// Warn and leave compaction to `/compact`.
    #[default]
    Warn,
    /// Run `/compact` automatically between turns.
    Auto,
}

/// Thresholds and mode for [`CompactionGauge`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompactionPolicy {
    /// Warn or auto-compact.
    pub mode: CompactMode,
    /// Usage (percent of the window) at which to warn once.
    pub warn_percent: u8,
    /// Usage (percent of the window) at which to auto-compact (auto mode) or
    /// warn after every turn (warn mode). Never below `warn_percent`.
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Zone {
    Below,
    Warn,
    Auto,
}

/// Why a compaction did not happen, as reported in
/// [`super::Msg::CompactFinished`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CompactFailure {
    /// Compaction would not have discarded anything (all messages are recent).
    #[error("nothing to compact: all {0} messages are recent turns")]
    NoGain(usize),
    /// Too little history to be worth summarizing.
    #[error("nothing to compact yet ({0} messages)")]
    TooShort(usize),
    /// Anything else (busy, summarization failed, save failed, ...).
    #[error("{0}")]
    Other(String),
}

impl CompactFailure {
    /// Whether compacting again at the same usage would fail the same way.
    #[must_use]
    pub fn is_no_gain(&self) -> bool {
        matches!(self, Self::NoGain(_) | Self::TooShort(_))
    }
}

/// What the gauge asks the app to do after a turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GaugeAction {
    /// Show a warning notice.
    Warn(String),
    /// Start a compaction, announcing it with this info notice.
    Compact(String),
}

/// Tracks the context gauge across turns and applies a [`CompactionPolicy`].
#[derive(Debug, Clone, Default)]
pub struct CompactionGauge {
    policy: CompactionPolicy,
    /// A warn-threshold notice was shown and usage has not dropped below it.
    warned: bool,
    /// Usage when the running compaction started (manual or automatic).
    attempt_used: Option<u64>,
    /// Whether the running compaction was started by the gauge.
    auto_running: bool,
    /// Usage at which the last compaction gained nothing.
    no_gain_at: Option<u64>,
}

impl CompactionGauge {
    /// A gauge applying `policy` (normalized).
    #[must_use]
    pub fn new(policy: CompactionPolicy) -> Self {
        Self {
            policy: policy.normalized().0,
            ..Self::default()
        }
    }

    /// The policy in effect.
    #[must_use]
    pub fn policy(&self) -> CompactionPolicy {
        self.policy
    }

    /// Whether the running compaction was started automatically.
    #[must_use]
    pub fn auto_running(&self) -> bool {
        self.auto_running
    }

    /// A turn finished with `used` tokens in context and a model window of
    /// `window` tokens (`None` or `0` when unknown). `idle` says whether a
    /// compaction could start now (no turn, nothing queued, no modal, not
    /// already compacting).
    pub fn after_turn(
        &mut self,
        used: u64,
        window: Option<u64>,
        idle: bool,
    ) -> Option<GaugeAction> {
        let window = window.filter(|w| *w > 0);
        if self.no_gain_at.is_some_and(|at| used < at) {
            // Usage dropped (new session, external compaction): start over.
            self.no_gain_at = None;
        }
        match self.policy.zone(used, window) {
            Zone::Below => {
                self.warned = false;
                None
            }
            Zone::Warn if self.policy.mode == CompactMode::Auto || self.warned => None,
            Zone::Warn => {
                self.warned = true;
                Some(GaugeAction::Warn(warn_text(used, window)))
            }
            Zone::Auto if self.policy.mode == CompactMode::Warn => {
                self.warned = true;
                Some(GaugeAction::Warn(format!(
                    "{} (auto-compaction is off; set compact = \"auto\" in tui.toml or pass --auto-compact)",
                    warn_text(used, window)
                )))
            }
            Zone::Auto => {
                if !idle {
                    return None;
                }
                if let Some(at) = self.no_gain_at
                    && used < at.saturating_add(retry_growth(window))
                {
                    return None;
                }
                self.started(Some(used));
                self.auto_running = true;
                Some(GaugeAction::Compact(format!(
                    "{}, compacting…",
                    usage_text(used, window)
                )))
            }
        }
    }

    /// A compaction started (manual `/compact` or [`GaugeAction::Compact`])
    /// with `used` tokens in context, if known.
    pub fn started(&mut self, used: Option<u64>) {
        self.attempt_used = used;
        self.auto_running = false;
    }

    /// A compaction finished with `result`.
    pub fn finished(&mut self, result: &Result<(), CompactFailure>) {
        let attempt = self.attempt_used.take();
        self.auto_running = false;
        match result {
            Ok(()) => {
                self.warned = false;
                self.no_gain_at = None;
            }
            Err(failure) if failure.is_no_gain() => self.no_gain_at = attempt,
            Err(_) => {}
        }
    }
}

/// Tokens of growth needed before retrying after a no-gain compaction.
fn retry_growth(window: Option<u64>) -> u64 {
    window.unwrap_or(FALLBACK_AUTO_TOKENS) * u64::from(RETRY_GROWTH_PERCENT) / 100
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

fn warn_text(used: u64, window: Option<u64>) -> String {
    format!("{}; run /compact to free space", usage_text(used, window))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    const W: Option<u64> = Some(100_000);

    fn auto() -> CompactionGauge {
        CompactionGauge::new(CompactionPolicy {
            mode: CompactMode::Auto,
            ..CompactionPolicy::default()
        })
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
    fn warn_mode_warns_once_then_every_turn_at_auto() {
        let mut g = CompactionGauge::default();
        assert_eq!(g.after_turn(70_000, W, true), None);
        let Some(GaugeAction::Warn(text)) = g.after_turn(76_000, W, true) else {
            panic!("expected a warning");
        };
        assert_eq!(
            text,
            "context 76% full (76k/100k tokens); run /compact to free space"
        );
        assert_eq!(g.after_turn(80_000, W, true), None);
        for used in [85_000, 86_000] {
            let Some(GaugeAction::Warn(text)) = g.after_turn(used, W, true) else {
                panic!("expected a warning at {used}");
            };
            assert!(text.contains("auto-compaction is off"), "{text}");
        }
        // Dropping below re-arms the single warning.
        assert_eq!(g.after_turn(80_000, W, true), None);
        assert_eq!(g.after_turn(50_000, W, true), None);
        assert!(matches!(
            g.after_turn(75_000, W, true),
            Some(GaugeAction::Warn(_))
        ));
    }

    #[test]
    fn successful_compaction_rearms_the_warning() {
        let mut g = CompactionGauge::default();
        assert!(g.after_turn(76_000, W, true).is_some());
        g.started(Some(76_000));
        g.finished(&Ok(()));
        assert!(g.after_turn(77_000, W, true).is_some());
    }

    #[test]
    fn auto_mode_compacts_only_when_idle() {
        let mut g = auto();
        assert_eq!(
            g.after_turn(80_000, W, true),
            None,
            "no warning in auto mode"
        );
        assert_eq!(g.after_turn(90_000, W, false), None);
        assert_eq!(
            g.after_turn(90_000, W, true),
            Some(GaugeAction::Compact(
                "context 90% full (90k/100k tokens), compacting…".into()
            ))
        );
        assert!(g.auto_running());
        g.finished(&Ok(()));
        assert!(!g.auto_running());
        assert!(matches!(
            g.after_turn(90_000, W, true),
            Some(GaugeAction::Compact(_))
        ));
    }

    #[test]
    fn no_gain_backs_off_until_usage_grows_ten_points() {
        let mut g = auto();
        assert!(g.after_turn(86_000, W, true).is_some());
        g.finished(&Err(CompactFailure::NoGain(12)));
        assert_eq!(g.after_turn(90_000, W, true), None);
        assert_eq!(g.after_turn(95_999, W, true), None);
        assert!(g.after_turn(96_000, W, true).is_some());
        g.finished(&Err(CompactFailure::TooShort(2)));
        assert_eq!(g.after_turn(97_000, W, true), None);
        // Other failures do not back off.
        let mut g = auto();
        assert!(g.after_turn(86_000, W, true).is_some());
        g.finished(&Err(CompactFailure::Other("llm down".into())));
        assert!(g.after_turn(86_000, W, true).is_some());
    }

    #[test]
    fn backoff_resets_when_usage_drops() {
        let mut g = auto();
        assert!(g.after_turn(86_000, W, true).is_some());
        g.finished(&Err(CompactFailure::NoGain(12)));
        assert_eq!(g.after_turn(10_000, W, true), None);
        assert!(g.after_turn(86_000, W, true).is_some());
    }

    #[test]
    fn manual_no_gain_also_backs_off() {
        let mut g = auto();
        g.started(Some(90_000));
        g.finished(&Err(CompactFailure::NoGain(8)));
        assert_eq!(g.after_turn(91_000, W, true), None);
    }

    #[test]
    fn unknown_window_uses_token_thresholds() {
        let mut g = CompactionGauge::default();
        assert_eq!(g.after_turn(119_999, None, true), None);
        assert_eq!(
            g.after_turn(120_000, Some(0), true),
            Some(GaugeAction::Warn(
                "context at 120k tokens; run /compact to free space".into()
            ))
        );
        assert_eq!(g.after_turn(149_999, None, true), None);
        assert!(matches!(
            g.after_turn(150_000, None, true),
            Some(GaugeAction::Warn(t)) if t.contains("auto-compaction is off")
        ));
        let mut g = auto();
        assert!(matches!(
            g.after_turn(150_000, None, true),
            Some(GaugeAction::Compact(_))
        ));
        g.finished(&Err(CompactFailure::NoGain(9)));
        assert_eq!(g.after_turn(164_999, None, true), None);
        assert!(g.after_turn(165_000, None, true).is_some());
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
