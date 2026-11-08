use super::types::ConsoleFrameStatus;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendState {
    Requested,
    AcceptedPersisted,
    Dispatching,
    Delivered,
    DeliveryFailed,
    /// A governed send whose bind answered bound: its input is being
    /// submitted, so it ends delivered or failed and is never held. It
    /// comes between `Dispatching` and the settled states; it is declared
    /// last so the earlier variants keep their discriminants.
    Submitted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendTransition {
    PersistAccepted,
    StartDispatch,
    MarkDelivered,
    MarkDeliveryFailed,
    /// A governed dispatch held past its door's budget submitted nothing:
    /// the send returns to accepted, its hold recorded, and a replay of its
    /// key dispatches it again. Only a dispatch before its submission is
    /// held.
    MarkHeld,
    /// A governed dispatch's bind answered bound, before its input is
    /// submitted.
    MarkSubmitted,
    /// A governed dispatch's submission answered unresolved: its native
    /// admission's answer was lost, and its input may have run.
    MarkUnresolved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendEffect {
    AppendFrame,
    UpdateFrameStatus,
    EmitFrame,
    DispatchSend,
}

impl SendState {
    pub fn apply(
        self,
        transition: SendTransition,
    ) -> Result<(Self, Vec<SendEffect>), &'static str> {
        match (self, transition) {
            (Self::Requested, SendTransition::PersistAccepted) => Ok((
                Self::AcceptedPersisted,
                vec![SendEffect::AppendFrame, SendEffect::EmitFrame],
            )),
            (Self::AcceptedPersisted, SendTransition::StartDispatch) => Ok((
                Self::Dispatching,
                vec![SendEffect::UpdateFrameStatus, SendEffect::DispatchSend],
            )),
            (Self::Dispatching, SendTransition::MarkSubmitted) => Ok((
                Self::Submitted,
                vec![SendEffect::UpdateFrameStatus, SendEffect::EmitFrame],
            )),
            (Self::Dispatching | Self::Submitted, SendTransition::MarkDelivered) => Ok((
                Self::Delivered,
                vec![SendEffect::UpdateFrameStatus, SendEffect::EmitFrame],
            )),
            (Self::Dispatching | Self::Submitted, SendTransition::MarkDeliveryFailed) => Ok((
                Self::DeliveryFailed,
                vec![SendEffect::UpdateFrameStatus, SendEffect::EmitFrame],
            )),
            (Self::Dispatching, SendTransition::MarkHeld) => Ok((
                Self::AcceptedPersisted,
                vec![SendEffect::UpdateFrameStatus, SendEffect::EmitFrame],
            )),
            _ => Err("illegal send state transition"),
        }
    }

    /// The lifecycle state of a stored `user_input` send frame. A send
    /// accepted under a redacting host policy is stored `redacted` until its
    /// delivery settles, so it reads as accepted.
    pub(crate) fn from_send_frame_status(status: ConsoleFrameStatus) -> Option<Self> {
        match status {
            ConsoleFrameStatus::Accepted | ConsoleFrameStatus::Redacted => {
                Some(Self::AcceptedPersisted)
            }
            ConsoleFrameStatus::Dispatching => Some(Self::Dispatching),
            ConsoleFrameStatus::Submitted => Some(Self::Submitted),
            ConsoleFrameStatus::Delivered => Some(Self::Delivered),
            ConsoleFrameStatus::DeliveryFailed => Some(Self::DeliveryFailed),
            ConsoleFrameStatus::Completed => None,
        }
    }

    /// The status a send frame records in this state; none before it is
    /// accepted.
    pub(crate) fn frame_status(self) -> Option<ConsoleFrameStatus> {
        match self {
            Self::Requested => None,
            Self::AcceptedPersisted => Some(ConsoleFrameStatus::Accepted),
            Self::Dispatching => Some(ConsoleFrameStatus::Dispatching),
            Self::Submitted => Some(ConsoleFrameStatus::Submitted),
            Self::Delivered => Some(ConsoleFrameStatus::Delivered),
            Self::DeliveryFailed => Some(ConsoleFrameStatus::DeliveryFailed),
        }
    }

    /// Whether the send has not settled: accepted, dispatching or submitted.
    pub(crate) fn is_unsettled(self) -> bool {
        matches!(
            self,
            Self::AcceptedPersisted | Self::Dispatching | Self::Submitted
        )
    }

    /// Reconcile an unsettled send, whose dispatch no live owner holds, from
    /// the runtime's admission evidence and the send's own hold. An admitted
    /// input is a delivery. An absent one is dispatched again under the
    /// send's own delivery identity, into the session it recorded; a held
    /// send is dispatched again on its hold, whatever the evidence short of
    /// an admission. A submitted send is never dispatched again: with no
    /// owner left to finish its submission and none of its input on the
    /// runtime, its outcome is unknown and it fails.
    pub(crate) fn reconcile(
        self,
        evidence: SendEvidence,
    ) -> Result<(Self, Vec<SendEffect>), &'static str> {
        match (self, evidence) {
            (
                Self::AcceptedPersisted | Self::Dispatching | Self::Submitted,
                SendEvidence::Admitted,
            ) => Ok((
                Self::Delivered,
                vec![SendEffect::UpdateFrameStatus, SendEffect::EmitFrame],
            )),
            (
                state @ (Self::AcceptedPersisted | Self::Dispatching),
                SendEvidence::AbsentRedispatchable | SendEvidence::Held,
            ) => Ok((state, vec![SendEffect::DispatchSend])),
            (Self::Submitted, SendEvidence::Absent) => Ok((
                Self::DeliveryFailed,
                vec![SendEffect::UpdateFrameStatus, SendEffect::EmitFrame],
            )),
            _ => Err("illegal send reconciliation"),
        }
    }
}

/// Native admission evidence, or the send's own hold, for one unsettled
/// console send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SendEvidence {
    /// The runtime holds an input as the send's own.
    Admitted,
    /// The runtime's read answered and holds no input as the send's own.
    Absent,
    /// The runtime holds no input under the send's delivery keys, and the
    /// send's delivery is keyed and pinned to the session it recorded, so
    /// dispatching it again cannot duplicate or retarget it.
    AbsentRedispatchable,
    /// The send's frame records a hold, and the runtime's evidence shows no
    /// admission (it reads absent, or it cannot be read: the send recorded
    /// no session, no runtime serves it, the read failed or ran late). The
    /// hold is recorded only when a governed dispatch was held before it
    /// submitted anything, and it is taken (a compare-and-set) before any
    /// dispatch again, so it is the record that nothing was submitted since:
    /// the send is dispatched again under its own keyed input.
    Held,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceIngestionState {
    Registered,
    Backfilling,
    CaughtUp,
    Live,
    GapDetected,
    ReplayUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceIngestionTransition {
    StartBackfill,
    BackfillComplete,
    StartLive,
    DetectGap,
    MarkReplayUnavailable,
    Recover,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceIngestionEffect {
    QuerySourceHistory,
    AppendFrame,
    SubscribeLive,
    MarkReplayUnavailable,
}

impl SourceIngestionState {
    pub fn apply(
        self,
        transition: SourceIngestionTransition,
    ) -> Result<(Self, Vec<SourceIngestionEffect>), &'static str> {
        match (self, transition) {
            (Self::Registered, SourceIngestionTransition::StartBackfill) => Ok((
                Self::Backfilling,
                vec![SourceIngestionEffect::QuerySourceHistory],
            )),
            (Self::Backfilling, SourceIngestionTransition::BackfillComplete) => {
                Ok((Self::CaughtUp, vec![SourceIngestionEffect::AppendFrame]))
            }
            (Self::CaughtUp, SourceIngestionTransition::StartLive) => {
                Ok((Self::Live, vec![SourceIngestionEffect::SubscribeLive]))
            }
            (Self::Live, SourceIngestionTransition::DetectGap) => {
                Ok((Self::GapDetected, Vec::new()))
            }
            (Self::GapDetected, SourceIngestionTransition::Recover) => Ok((
                Self::Backfilling,
                vec![SourceIngestionEffect::QuerySourceHistory],
            )),
            (Self::GapDetected, SourceIngestionTransition::MarkReplayUnavailable) => Ok((
                Self::ReplayUnavailable,
                vec![SourceIngestionEffect::MarkReplayUnavailable],
            )),
            (Self::ReplayUnavailable, SourceIngestionTransition::Recover) => Ok((
                Self::Backfilling,
                vec![SourceIngestionEffect::QuerySourceHistory],
            )),
            _ => Err("illegal source ingestion state transition"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplaySubscriptionState {
    Snapshotting,
    Live,
    Lagged,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplaySubscriptionTransition {
    SnapshotComplete,
    MarkLagged,
    RecoverLive,
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplaySubscriptionEffect {
    EmitSnapshotStarted,
    EmitReplayFrames,
    EmitSnapshotComplete,
    EmitLagged,
    CloseStream,
}

impl ReplaySubscriptionState {
    pub fn apply(
        self,
        transition: ReplaySubscriptionTransition,
    ) -> Result<(Self, Vec<ReplaySubscriptionEffect>), &'static str> {
        match (self, transition) {
            (Self::Snapshotting, ReplaySubscriptionTransition::SnapshotComplete) => Ok((
                Self::Live,
                vec![
                    ReplaySubscriptionEffect::EmitSnapshotStarted,
                    ReplaySubscriptionEffect::EmitReplayFrames,
                    ReplaySubscriptionEffect::EmitSnapshotComplete,
                ],
            )),
            (Self::Live, ReplaySubscriptionTransition::MarkLagged) => {
                Ok((Self::Lagged, vec![ReplaySubscriptionEffect::EmitLagged]))
            }
            (Self::Lagged, ReplaySubscriptionTransition::RecoverLive) => {
                Ok((Self::Live, Vec::new()))
            }
            (_, ReplaySubscriptionTransition::Close) if self != Self::Closed => {
                Ok((Self::Closed, vec![ReplaySubscriptionEffect::CloseStream]))
            }
            _ => Err("illegal replay subscription state transition"),
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn send_reducer_allows_happy_path() {
        let (state, effects) = SendState::Requested
            .apply(SendTransition::PersistAccepted)
            .expect("requested -> accepted should be legal");
        assert_eq!(state, SendState::AcceptedPersisted);
        assert!(effects.contains(&SendEffect::AppendFrame));

        let (state, effects) = state
            .apply(SendTransition::StartDispatch)
            .expect("accepted -> dispatching should be legal");
        assert_eq!(state, SendState::Dispatching);
        assert!(effects.contains(&SendEffect::DispatchSend));

        let (state, effects) = state
            .apply(SendTransition::MarkDelivered)
            .expect("dispatching -> delivered should be legal");
        assert_eq!(state, SendState::Delivered);
        assert!(effects.contains(&SendEffect::UpdateFrameStatus));
    }

    #[test]
    fn send_reconciliation_settles_only_unsettled_sends_from_evidence() {
        for unsettled in [SendState::AcceptedPersisted, SendState::Dispatching] {
            assert_eq!(
                unsettled.reconcile(SendEvidence::Admitted),
                Ok((
                    SendState::Delivered,
                    vec![SendEffect::UpdateFrameStatus, SendEffect::EmitFrame]
                ))
            );
            assert_eq!(
                unsettled.reconcile(SendEvidence::AbsentRedispatchable),
                Ok((unsettled, vec![SendEffect::DispatchSend]))
            );
            assert_eq!(
                unsettled.reconcile(SendEvidence::Held),
                Ok((unsettled, vec![SendEffect::DispatchSend]))
            );
            assert_eq!(
                unsettled.reconcile(SendEvidence::Absent),
                Err("illegal send reconciliation"),
                "{unsettled:?}: absent alone settles nothing before a submission"
            );
        }
        // A submitted send is delivered on an admission, fails on a read
        // that found none of its input, and is never dispatched again.
        assert_eq!(
            SendState::Submitted.reconcile(SendEvidence::Admitted),
            Ok((
                SendState::Delivered,
                vec![SendEffect::UpdateFrameStatus, SendEffect::EmitFrame]
            ))
        );
        assert_eq!(
            SendState::Submitted.reconcile(SendEvidence::Absent),
            Ok((
                SendState::DeliveryFailed,
                vec![SendEffect::UpdateFrameStatus, SendEffect::EmitFrame]
            ))
        );
        for evidence in [SendEvidence::AbsentRedispatchable, SendEvidence::Held] {
            assert_eq!(
                SendState::Submitted.reconcile(evidence),
                Err("illegal send reconciliation"),
                "{evidence:?}"
            );
        }
        for settled in [
            SendState::Requested,
            SendState::Delivered,
            SendState::DeliveryFailed,
        ] {
            for evidence in [
                SendEvidence::Admitted,
                SendEvidence::Absent,
                SendEvidence::AbsentRedispatchable,
                SendEvidence::Held,
            ] {
                assert_eq!(
                    settled.reconcile(evidence),
                    Err("illegal send reconciliation"),
                    "{settled:?} {evidence:?}"
                );
            }
        }
    }

    #[test]
    fn send_frame_statuses_map_to_send_states() {
        assert_eq!(
            SendState::from_send_frame_status(ConsoleFrameStatus::Accepted),
            Some(SendState::AcceptedPersisted)
        );
        assert_eq!(
            SendState::from_send_frame_status(ConsoleFrameStatus::Redacted),
            Some(SendState::AcceptedPersisted)
        );
        assert_eq!(
            SendState::from_send_frame_status(ConsoleFrameStatus::Dispatching),
            Some(SendState::Dispatching)
        );
        assert_eq!(
            SendState::from_send_frame_status(ConsoleFrameStatus::Submitted),
            Some(SendState::Submitted)
        );
        assert_eq!(
            SendState::from_send_frame_status(ConsoleFrameStatus::Delivered),
            Some(SendState::Delivered)
        );
        assert_eq!(
            SendState::from_send_frame_status(ConsoleFrameStatus::DeliveryFailed),
            Some(SendState::DeliveryFailed)
        );
        assert_eq!(
            SendState::from_send_frame_status(ConsoleFrameStatus::Completed),
            None
        );
        assert!(SendState::AcceptedPersisted.is_unsettled());
        assert!(SendState::Dispatching.is_unsettled());
        assert!(SendState::Submitted.is_unsettled());
        assert!(!SendState::Delivered.is_unsettled());
        assert!(!SendState::DeliveryFailed.is_unsettled());
        for state in [
            SendState::AcceptedPersisted,
            SendState::Dispatching,
            SendState::Submitted,
            SendState::Delivered,
            SendState::DeliveryFailed,
        ] {
            let status = state.frame_status().expect("a send frame's status");
            assert_eq!(SendState::from_send_frame_status(status), Some(state));
        }
        assert_eq!(SendState::Requested.frame_status(), None);
    }

    #[test]
    fn send_reducer_rejects_terminal_to_dispatch() {
        assert_eq!(
            SendState::Delivered.apply(SendTransition::StartDispatch),
            Err("illegal send state transition")
        );
    }

    /// A hold is recorded only on a dispatch held before it
    /// submitted anything. `MarkHeld` leaves only `Dispatching`, back to
    /// accepted; from every state before a dispatch began, and from every
    /// state a submission settles (delivered, failed), it is refused.
    #[test]
    fn mark_held_is_refused_from_every_state_but_a_dispatch_in_flight() {
        assert_eq!(
            SendState::Dispatching.apply(SendTransition::MarkHeld),
            Ok((
                SendState::AcceptedPersisted,
                vec![SendEffect::UpdateFrameStatus, SendEffect::EmitFrame]
            ))
        );
        for state in [
            SendState::Requested,
            SendState::AcceptedPersisted,
            SendState::Submitted,
            SendState::Delivered,
            SendState::DeliveryFailed,
        ] {
            assert_eq!(
                state.apply(SendTransition::MarkHeld),
                Err("illegal send state transition"),
                "{state:?}"
            );
        }
    }

    /// A governed dispatch is submitted only from `Dispatching`,
    /// when its bind answered bound; a submitted send is delivered or fails.
    #[test]
    fn a_submitted_send_comes_only_from_a_dispatch_and_settles_delivered_or_failed() {
        assert_eq!(
            SendState::Dispatching.apply(SendTransition::MarkSubmitted),
            Ok((
                SendState::Submitted,
                vec![SendEffect::UpdateFrameStatus, SendEffect::EmitFrame]
            ))
        );
        for state in [
            SendState::Requested,
            SendState::AcceptedPersisted,
            SendState::Submitted,
            SendState::Delivered,
            SendState::DeliveryFailed,
        ] {
            assert_eq!(
                state.apply(SendTransition::MarkSubmitted),
                Err("illegal send state transition"),
                "{state:?}"
            );
        }
        for (transition, settled) in [
            (SendTransition::MarkDelivered, SendState::Delivered),
            (
                SendTransition::MarkDeliveryFailed,
                SendState::DeliveryFailed,
            ),
        ] {
            assert_eq!(
                SendState::Submitted.apply(transition),
                Ok((
                    settled,
                    vec![SendEffect::UpdateFrameStatus, SendEffect::EmitFrame]
                ))
            );
        }
    }

    /// A submitted send whose submission answered unresolved (its input may
    /// have run) stays submitted, recording that answer: it is neither
    /// failed nor held, and the runtime's evidence settles it. From every
    /// other state the mark is refused.
    ///
    /// RED before the fix: the mark is refused from `Submitted` too.
    #[test]
    fn an_unresolved_submission_keeps_the_send_submitted() {
        assert_eq!(
            SendState::Submitted.apply(SendTransition::MarkUnresolved),
            Ok((
                SendState::Submitted,
                vec![SendEffect::UpdateFrameStatus, SendEffect::EmitFrame]
            ))
        );
        for state in [
            SendState::Requested,
            SendState::AcceptedPersisted,
            SendState::Dispatching,
            SendState::Delivered,
            SendState::DeliveryFailed,
        ] {
            assert_eq!(
                state.apply(SendTransition::MarkUnresolved),
                Err("illegal send state transition"),
                "{state:?}"
            );
        }
    }

    #[test]
    fn source_ingestion_reducer_models_backfill_to_live() {
        let (state, _) = SourceIngestionState::Registered
            .apply(SourceIngestionTransition::StartBackfill)
            .expect("registered -> backfilling");
        let (state, _) = state
            .apply(SourceIngestionTransition::BackfillComplete)
            .expect("backfilling -> caught up");
        let (state, effects) = state
            .apply(SourceIngestionTransition::StartLive)
            .expect("caught up -> live");
        assert_eq!(state, SourceIngestionState::Live);
        assert!(effects.contains(&SourceIngestionEffect::SubscribeLive));
    }

    #[test]
    fn replay_subscription_reducer_emits_snapshot_boundary() {
        let (state, effects) = ReplaySubscriptionState::Snapshotting
            .apply(ReplaySubscriptionTransition::SnapshotComplete)
            .expect("snapshotting -> live");
        assert_eq!(state, ReplaySubscriptionState::Live);
        assert_eq!(
            effects,
            vec![
                ReplaySubscriptionEffect::EmitSnapshotStarted,
                ReplaySubscriptionEffect::EmitReplayFrames,
                ReplaySubscriptionEffect::EmitSnapshotComplete,
            ]
        );
    }
}
