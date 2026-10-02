//! Typed per-member result of a mob Stop (#1500).
//!
//! Stop pauses a mob. Each member's runtime is held, so input admitted before
//! the stop stays queued until Resume instead of starting a run while the mob
//! is Stopped. The report says, for every member, what happened to the run it
//! had and whether its run starts are actually held. A member whose runtime
//! cannot be held (a remote peer without the capability) is reported as such
//! rather than hidden behind a bare `Ok`.

use std::collections::BTreeMap;

use meerkat_core::lifecycle::RunId;

use crate::AgentIdentity;

/// What a mob Stop did to each member.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MobStopReport {
    /// One outcome per member the stop reached, by identity.
    pub members: BTreeMap<AgentIdentity, MemberStopOutcome>,
}

impl MobStopReport {
    /// Members whose run starts are not held: they may still start a turn
    /// while the mob is Stopped.
    pub fn not_holdable(&self) -> impl Iterator<Item = (&AgentIdentity, &NotHoldableReason)> {
        self.members
            .iter()
            .filter_map(|(identity, outcome)| match &outcome.starts {
                MemberRunStarts::NotHoldable { reason } => Some((identity, reason)),
                MemberRunStarts::Held => None,
            })
    }
}

/// One member's stop outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberStopOutcome {
    /// The member's run when the stop reached it.
    pub run: MemberStopRun,
    /// Whether the member can start a new run before Resume.
    pub starts: MemberRunStarts,
}

/// The member's run when the stop reached it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum MemberStopRun {
    /// The member had no run, so there was nothing to cancel.
    NoRun,
    /// The run was cancelled at its next boundary; the stop waits for it.
    CancelledAtBoundary { run_id: RunId },
    /// The run ended on its own between the hold and the cancel.
    RunEndedBeforeCancel { run_id: RunId },
    /// A turn-driven member's run: Stop does not cancel it, and it finishes
    /// normally. No new run starts after it until Resume.
    LeftRunning { run_id: RunId },
    /// A remote member that cannot report its run (it lacks the run-start
    /// hold capability); the stop sent it the ambient interrupt.
    Interrupted,
}

/// Whether a member's run starts are held until Resume.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum MemberRunStarts {
    /// No new run starts until Resume.
    Held,
    /// The member's runtime could not be held: it may start a turn while the
    /// mob is Stopped.
    NotHoldable { reason: NotHoldableReason },
}

/// Why a member's run starts could not be held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum NotHoldableReason {
    /// A remote member whose host does not support the run-start hold.
    PeerLacksCapability,
    /// The provisioner serving the member does not implement the hold.
    ProvisionerLacksCapability,
}
