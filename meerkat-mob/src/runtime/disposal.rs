//! Disposal pipeline types for member cleanup.
//!
//! Separates **what** to clean up ([`DisposalStep`]) from **how** to handle
//! failures ([`ErrorPolicy`]). The pipeline is driven by
//! `MobActor::dispose_member`.

use crate::error::MobError;
use crate::ids::MeerkatId;
use crate::roster::RosterEntry;

// ---------------------------------------------------------------------------
// DisposalStep
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum DisposalStep {
    StopHostLoop,
    NotifyPeers,
    ArchiveSession,
}

impl DisposalStep {
    /// The ordered sequence of policy-driven steps.
    pub(super) const ORDERED: [DisposalStep; 3] = [
        DisposalStep::StopHostLoop,
        DisposalStep::NotifyPeers,
        DisposalStep::ArchiveSession,
    ];

    /// Whether this step involves peer communication that is expected to fail
    /// during concurrent bulk teardown.
    pub(super) fn is_peer_step(self) -> bool {
        matches!(self, Self::NotifyPeers)
    }
}

impl std::fmt::Display for DisposalStep {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::StopHostLoop => f.write_str("StopHostLoop"),
            Self::NotifyPeers => f.write_str("NotifyPeers"),
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
    pub(crate) agent_identity: MeerkatId,
    pub entry: RosterEntry,
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
            meerkat_id = %ctx.agent_identity,
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
                meerkat_id = %ctx.agent_identity,
                step = %step,
                error = %error,
                "retire(bulk): step failed (expected during concurrent teardown)"
            );
        } else {
            tracing::warn!(
                meerkat_id = %ctx.agent_identity,
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
    use crate::ids::{AgentIdentity, AgentRuntimeId, FenceToken, Generation, ProfileName};
    use crate::roster::MemberState;
    use meerkat_core::types::SessionId;
    use std::collections::BTreeSet;

    fn test_ctx() -> DisposalContext {
        DisposalContext {
            agent_identity: MeerkatId::from("test-member"),
            entry: RosterEntry {
                agent_identity: AgentIdentity::from("test-member"),
                generation: Generation::INITIAL,
                fence_token: FenceToken::new(0),
                agent_runtime_id: AgentRuntimeId::initial(AgentIdentity::from("test-member")),
                role: ProfileName::from("worker"),
                member_ref: MemberRef::from_bridge_session_id(SessionId::new()),
                runtime_mode: crate::MobRuntimeMode::TurnDriven,
                peer_id: None,
                transport_public_key: None,
                state: MemberState::Retiring,
                wired_to: BTreeSet::new(),
                external_peer_specs: std::collections::BTreeMap::new(),
                labels: std::collections::BTreeMap::new(),
                kickoff: None,
                effective_profile_override: None,
            },
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
        let stop_idx = steps
            .iter()
            .position(|s| *s == DisposalStep::StopHostLoop)
            .unwrap();
        let notify_idx = steps
            .iter()
            .position(|s| *s == DisposalStep::NotifyPeers)
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
            notify_idx < archive_idx,
            "NotifyPeers must precede ArchiveSession"
        );
    }

    #[test]
    fn test_disposal_step_display() {
        assert_eq!(DisposalStep::StopHostLoop.to_string(), "StopHostLoop");
        assert_eq!(DisposalStep::NotifyPeers.to_string(), "NotifyPeers");
        assert_eq!(DisposalStep::ArchiveSession.to_string(), "ArchiveSession");
    }
}
