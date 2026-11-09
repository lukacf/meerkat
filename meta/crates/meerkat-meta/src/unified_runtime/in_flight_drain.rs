//! The report of
//! [`UnifiedRuntime::drain_in_flight_turns`](super::UnifiedRuntime::drain_in_flight_turns).

use std::time::Duration;

/// What a drain of the mob's in-flight turns did.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct InFlightDrainReport {
    /// How the drain ended.
    pub outcome: InFlightDrainOutcome,
    /// The window the drain was given, from its start to its bound.
    pub window: Duration,
    /// How long the drain ran.
    pub waited: Duration,
}

/// How a drain of the mob's in-flight turns ended.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum InFlightDrainOutcome {
    /// The mob stopped, leaving its members' runs running, and the drain
    /// waited for them: each member's run as the drain saw it.
    Drained(meerkat_mob::MobRunDrainReport),
    /// The window ended while identity foreground operations (a held member
    /// add, a roster pass) were still settling, before the drain's Stop. The
    /// shutdown that follows settles them inside its own budget, and nothing
    /// was drained.
    IdentityForegroundBusy,
    /// The drain's Stop failed, so nothing was drained. The shutdown that
    /// follows stops the mob as it does without a drain.
    StopFailed { error: String },
    /// The mob stopped, but the wait for its members' runs failed (the mob
    /// left the Stopped state, or its actor is gone), so nothing was
    /// drained. The shutdown that follows proceeds as it does without a
    /// drain.
    DrainFailed { error: String },
}
