//! Disposal pipeline types for member cleanup.
//!
//! Separates **what** to clean up ([`DisposalStep`]) from **how** to handle
//! failures ([`ErrorPolicy`]). The pipeline is driven by
//! `MobActor::dispose_member`.

use crate::error::MobError;
use crate::ids::MeerkatId;
use crate::roster::RosterEntry;
use meerkat_core::agent::CommsRuntime as CoreCommsRuntime;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// DisposalStep
// ---------------------------------------------------------------------------

/// Named cleanup step, ordered by execution priority.
///
/// Ordering invariants (documented + tested):
///   - `StopHostLoop` first — member must not be running when peers learn of
///     retirement
///   - `NotifyPeers` before `RemoveTrustEdges` — peers should hear "I'm
///     leaving" while they can still verify the sender's identity
///   - `ArchiveSession` after trust cleanup — session cannot receive new comms
///     after trust is gone
///   - `PruneWireEdgeLocks` is infallible finalization, runs in the finally
///     block alongside roster removal
///
/// `RemoveFromRoster` is deliberately **not** a variant — it runs
/// unconditionally in `dispose_member`'s finally block, outside the
/// policy-driven loop. This makes it structurally impossible for a policy to
/// skip or abort before roster removal.
///
/// Adding a variant forces a match arm in `MobActor::execute_step`
/// (compiler-enforced).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum DisposalStep {
    StopHostLoop,
    NotifyPeers,
    RemoveTrustEdges,
    ArchiveSession,
}

impl DisposalStep {
    /// The ordered sequence of policy-driven steps.
    pub(super) const ORDERED: [DisposalStep; 4] = [
        DisposalStep::StopHostLoop,
        DisposalStep::NotifyPeers,
        DisposalStep::RemoveTrustEdges,
        DisposalStep::ArchiveSession,
    ];

    /// Whether this step involves peer communication that is expected to fail
    /// during concurrent bulk teardown.
    pub(super) fn is_peer_step(self) -> bool {
        matches!(self, Self::NotifyPeers | Self::RemoveTrustEdges)
    }
}

impl std::fmt::Display for DisposalStep {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StopHostLoop => f.write_str("StopHostLoop"),
            Self::NotifyPeers => f.write_str("NotifyPeers"),
            Self::RemoveTrustEdges => f.write_str("RemoveTrustEdges"),
            Self::ArchiveSession => f.write_str("ArchiveSession"),
        }
    }
}

// ---------------------------------------------------------------------------
// DisposalContext
// ---------------------------------------------------------------------------

/// Snapshotted member state, created once before the pipeline runs.
///
/// Steps never re-read the roster — prevents TOCTOU races.
pub(super) struct DisposalContext {
    pub meerkat_id: MeerkatId,
    pub entry: RosterEntry,
    pub retiring_comms: Option<Arc<dyn CoreCommsRuntime>>,
    pub retiring_key: Option<String>,
}

// ---------------------------------------------------------------------------
// DisposalReport
// ---------------------------------------------------------------------------

/// What happened during disposal. Ephemeral — returned to caller, not stored.
pub(super) struct DisposalReport {
    /// Steps that completed successfully, in execution order.
    pub completed: Vec<DisposalStep>,
    /// Steps where the error policy chose to continue.
    pub skipped: Vec<(DisposalStep, MobError)>,
    /// Step where the error policy chose to abort (if any).
    pub aborted_at: Option<(DisposalStep, MobError)>,
    /// Always true — roster removal runs unconditionally.
    pub roster_removed: bool,
}

impl DisposalReport {
    pub(super) fn new() -> Self {
        Self {
            completed: Vec::new(),
            skipped: Vec::new(),
            aborted_at: None,
            roster_removed: false,
        }
    }

    /// Whether every step completed without error.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn is_clean(&self) -> bool {
        self.skipped.is_empty() && self.aborted_at.is_none()
    }
}

// ---------------------------------------------------------------------------
// ErrorPolicy
// ---------------------------------------------------------------------------

/// Caller-injected error strategy for the disposal pipeline.
pub(super) trait ErrorPolicy: Send {
    /// Returns `true` to continue (skip the failed step), `false` to abort the
    /// pipeline.
    fn on_step_error(
        &mut self,
        step: DisposalStep,
        error: &MobError,
        ctx: &DisposalContext,
    ) -> bool;
}

// ---------------------------------------------------------------------------
// Concrete policies
// ---------------------------------------------------------------------------

/// Best-effort: log warn, always continue. Used for single-member retire.
pub(super) struct WarnAndContinue;

impl ErrorPolicy for WarnAndContinue {
    fn on_step_error(
        &mut self,
        step: DisposalStep,
        error: &MobError,
        ctx: &DisposalContext,
    ) -> bool {
        tracing::warn!(
            meerkat_id = %ctx.meerkat_id,
            step = %step,
            error = %error,
            "retire: step failed (continuing)"
        );
        true
    }
}

/// Bulk: peer-step failures are logged at debug (expected during concurrent
/// teardown), others at warn. Always continues.
pub(super) struct BulkBestEffort;

impl ErrorPolicy for BulkBestEffort {
    fn on_step_error(
        &mut self,
        step: DisposalStep,
        error: &MobError,
        ctx: &DisposalContext,
    ) -> bool {
        if step.is_peer_step() {
            tracing::debug!(
                meerkat_id = %ctx.meerkat_id,
                step = %step,
                error = %error,
                "retire(bulk): step failed (expected during concurrent teardown)"
            );
        } else {
            tracing::warn!(
                meerkat_id = %ctx.meerkat_id,
                step = %step,
                error = %error,
                "retire(bulk): step failed (continuing)"
            );
        }
        true
    }
}

/// Strict: abort on first failure.
#[cfg_attr(not(test), allow(dead_code))]
pub(super) struct AbortOnError;

impl ErrorPolicy for AbortOnError {
    fn on_step_error(
        &mut self,
        _step: DisposalStep,
        _error: &MobError,
        _ctx: &DisposalContext,
    ) -> bool {
        false
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::event::MemberRef;
    use crate::ids::ProfileName;
    use crate::roster::MemberState;
    use meerkat_core::types::SessionId;
    use std::collections::BTreeSet;

    fn test_ctx() -> DisposalContext {
        DisposalContext {
            meerkat_id: MeerkatId::from("test-member"),
            entry: RosterEntry {
                meerkat_id: MeerkatId::from("test-member"),
                profile: ProfileName::from("worker"),
                member_ref: MemberRef::from_session_id(SessionId::new()),
                runtime_mode: crate::MobRuntimeMode::TurnDriven,
                state: MemberState::Retiring,
                wired_to: BTreeSet::new(),
                labels: std::collections::BTreeMap::new(),
            },
            retiring_comms: None,
            retiring_key: None,
        }
    }

    fn test_error() -> MobError {
        MobError::Internal("test error".to_string())
    }

    #[test]
    fn test_warn_and_continue_always_continues() {
        let mut policy = WarnAndContinue;
        let ctx = test_ctx();
        for step in DisposalStep::ORDERED {
            assert!(policy.on_step_error(step, &test_error(), &ctx));
        }
    }

    #[test]
    fn test_bulk_best_effort_always_continues() {
        let mut policy = BulkBestEffort;
        let ctx = test_ctx();
        for step in DisposalStep::ORDERED {
            assert!(policy.on_step_error(step, &test_error(), &ctx));
        }
    }

    #[test]
    fn test_bulk_best_effort_uses_is_peer_step() {
        // Verify the predicate classifies steps correctly.
        assert!(!DisposalStep::StopHostLoop.is_peer_step());
        assert!(DisposalStep::NotifyPeers.is_peer_step());
        assert!(DisposalStep::RemoveTrustEdges.is_peer_step());
        assert!(!DisposalStep::ArchiveSession.is_peer_step());
    }

    #[test]
    fn test_abort_on_error_stops() {
        let mut policy = AbortOnError;
        let ctx = test_ctx();
        for step in DisposalStep::ORDERED {
            assert!(!policy.on_step_error(step, &test_error(), &ctx));
        }
    }

    #[test]
    fn test_disposal_report_is_clean() {
        let mut report = DisposalReport::new();
        assert!(report.is_clean());
        report.completed.push(DisposalStep::StopHostLoop);
        assert!(report.is_clean());
    }

    #[test]
    fn test_disposal_report_tracks_skipped_steps() {
        let mut report = DisposalReport::new();
        report
            .skipped
            .push((DisposalStep::NotifyPeers, test_error()));
        assert!(!report.is_clean());
    }

    #[test]
    fn test_disposal_report_tracks_abort() {
        let mut report = DisposalReport::new();
        report.aborted_at = Some((DisposalStep::ArchiveSession, test_error()));
        assert!(!report.is_clean());
    }

    #[test]
    fn test_disposal_step_ordering_invariants() {
        let steps = DisposalStep::ORDERED;
        // StopHostLoop must come before NotifyPeers
        let stop_idx = steps
            .iter()
            .position(|s| *s == DisposalStep::StopHostLoop)
            .unwrap();
        let notify_idx = steps
            .iter()
            .position(|s| *s == DisposalStep::NotifyPeers)
            .unwrap();
        let trust_idx = steps
            .iter()
            .position(|s| *s == DisposalStep::RemoveTrustEdges)
            .unwrap();
        let archive_idx = steps
            .iter()
            .position(|s| *s == DisposalStep::ArchiveSession)
            .unwrap();

        assert!(
            stop_idx < notify_idx,
            "StopHostLoop must precede NotifyPeers"
        );
        assert!(
            notify_idx < trust_idx,
            "NotifyPeers must precede RemoveTrustEdges"
        );
        assert!(
            trust_idx < archive_idx,
            "RemoveTrustEdges must precede ArchiveSession"
        );
    }

    #[test]
    fn test_disposal_step_display() {
        assert_eq!(DisposalStep::StopHostLoop.to_string(), "StopHostLoop");
        assert_eq!(DisposalStep::NotifyPeers.to_string(), "NotifyPeers");
        assert_eq!(
            DisposalStep::RemoveTrustEdges.to_string(),
            "RemoveTrustEdges"
        );
        assert_eq!(DisposalStep::ArchiveSession.to_string(), "ArchiveSession");
    }
}
