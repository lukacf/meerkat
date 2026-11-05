//! Denial counters and the auto-mode state machine.
//!
//! Only classifier blocks count. Three consecutive blocks or twenty in total
//! pause auto mode; while paused, every Tier-3 action is escalated to the human.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Consecutive classifier blocks that pause auto mode.
pub const MAX_CONSECUTIVE_DENIALS: u32 = 3;
/// Total classifier blocks (per process) that pause auto mode.
pub const MAX_TOTAL_DENIALS: u32 = 20;

/// Classifier denial counters.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DenialCounters {
    /// Classifier blocks since the last reviewed allow.
    pub consecutive: u32,
    /// Classifier blocks since start (or the last `/auto resume`).
    pub total: u32,
}

/// Why auto mode paused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PauseCause {
    /// [`MAX_CONSECUTIVE_DENIALS`] reached.
    ConsecutiveDenials,
    /// [`MAX_TOTAL_DENIALS`] reached; only an explicit resume clears it.
    TotalDenials,
    /// The user turned auto mode off (`--no-auto`, `/auto off`): Tier-3 actions are denied.
    UserDisabled,
}

impl fmt::Display for PauseCause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ConsecutiveDenials => {
                write!(f, "{MAX_CONSECUTIVE_DENIALS} consecutive classifier blocks")
            }
            Self::TotalDenials => write!(f, "{MAX_TOTAL_DENIALS} classifier blocks in total"),
            Self::UserDisabled => f.write_str("auto mode disabled by the user"),
        }
    }
}

/// Auto-mode state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum AutoModeState {
    /// The classifier decides Tier-3 actions.
    Active,
    /// Tier-3 actions go to the human (or are denied when [`PauseCause::UserDisabled`]).
    Paused {
        /// Why.
        cause: PauseCause,
    },
}

/// Counters plus state, mutated together under one lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutoMode {
    /// Denial counters.
    pub counters: DenialCounters,
    /// Current state.
    pub state: AutoModeState,
}

impl AutoMode {
    /// Fresh state; `enabled == false` starts paused with [`PauseCause::UserDisabled`].
    pub fn new(enabled: bool) -> Self {
        Self {
            counters: DenialCounters::default(),
            state: if enabled {
                AutoModeState::Active
            } else {
                AutoModeState::Paused {
                    cause: PauseCause::UserDisabled,
                }
            },
        }
    }

    /// A Tier-3 action was allowed after review: reset the consecutive count.
    pub fn record_allow(&mut self) {
        self.counters.consecutive = 0;
    }

    /// The classifier blocked an action. Returns the pause cause when this
    /// block transitioned auto mode from active to paused.
    pub fn record_classifier_deny(&mut self) -> Option<PauseCause> {
        self.counters.consecutive = self.counters.consecutive.saturating_add(1);
        self.counters.total = self.counters.total.saturating_add(1);
        if self.state != AutoModeState::Active {
            return None;
        }
        let cause = if self.counters.total >= MAX_TOTAL_DENIALS {
            PauseCause::TotalDenials
        } else if self.counters.consecutive >= MAX_CONSECUTIVE_DENIALS {
            PauseCause::ConsecutiveDenials
        } else {
            return None;
        };
        self.state = AutoModeState::Paused { cause };
        Some(cause)
    }

    /// Pause (or disable) explicitly. Returns `true` when the state changed.
    pub fn pause(&mut self, cause: PauseCause) -> bool {
        let next = AutoModeState::Paused { cause };
        let changed = self.state != next;
        self.state = next;
        changed
    }

    /// Try to resume. A [`PauseCause::TotalDenials`] or [`PauseCause::UserDisabled`]
    /// pause only clears with `reset_total` (explicit `/auto resume` / `/auto on`).
    /// Returns `true` when the state changed to active.
    pub fn resume(&mut self, reset_total: bool) -> bool {
        let AutoModeState::Paused { cause } = self.state else {
            if reset_total {
                self.counters = DenialCounters::default();
            }
            return false;
        };
        if matches!(cause, PauseCause::TotalDenials | PauseCause::UserDisabled) && !reset_total {
            return false;
        }
        self.counters.consecutive = 0;
        if reset_total {
            self.counters.total = 0;
        }
        self.state = AutoModeState::Active;
        true
    }

    /// Whether Tier-3 actions are denied outright.
    pub fn disabled(&self) -> bool {
        self.state
            == AutoModeState::Paused {
                cause: PauseCause::UserDisabled,
            }
    }

    /// Whether Tier-3 actions are escalated to the human.
    pub fn escalating(&self) -> bool {
        matches!(
            self.state,
            AutoModeState::Paused {
                cause: PauseCause::ConsecutiveDenials | PauseCause::TotalDenials
            }
        )
    }
}

impl Default for AutoMode {
    fn default() -> Self {
        Self::new(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_consecutive_pause() {
        let mut m = AutoMode::default();
        assert_eq!(m.record_classifier_deny(), None);
        assert_eq!(m.record_classifier_deny(), None);
        assert_eq!(
            m.record_classifier_deny(),
            Some(PauseCause::ConsecutiveDenials)
        );
        assert!(m.escalating());
        assert_eq!(m.record_classifier_deny(), None, "already paused");
        assert!(m.resume(false));
        assert_eq!(m.counters.consecutive, 0);
        assert_eq!(m.counters.total, 4);
    }

    #[test]
    fn allows_reset_consecutive_only() {
        let mut m = AutoMode::default();
        m.record_classifier_deny();
        m.record_classifier_deny();
        m.record_allow();
        assert_eq!(m.record_classifier_deny(), None);
        assert_eq!(m.counters.total, 3);
        assert_eq!(m.state, AutoModeState::Active);
    }

    #[test]
    fn twenty_total_pause_needs_reset() {
        let mut m = AutoMode::default();
        let mut cause = None;
        for _ in 0..20 {
            m.record_allow();
            if let Some(c) = m.record_classifier_deny() {
                cause = Some(c);
            }
        }
        assert_eq!(cause, Some(PauseCause::TotalDenials));
        assert!(!m.resume(false), "allow-once does not clear a total pause");
        assert!(m.escalating());
        assert!(m.resume(true));
        assert_eq!(m.counters, DenialCounters::default());
    }

    #[test]
    fn disabled_state() {
        let mut m = AutoMode::new(false);
        assert!(m.disabled());
        assert!(!m.escalating());
        assert!(!m.resume(false));
        assert!(m.resume(true));
        assert!(!m.disabled());
        assert!(m.pause(PauseCause::UserDisabled));
        assert!(!m.pause(PauseCause::UserDisabled));
    }
}
