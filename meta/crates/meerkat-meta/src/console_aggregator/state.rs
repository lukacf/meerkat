use super::types::ConsoleFrameStatus;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendState {
    Requested,
    AcceptedPersisted,
    Dispatching,
    Delivered,
    DeliveryFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendTransition {
    PersistAccepted,
    StartDispatch,
    MarkDelivered,
    MarkDeliveryFailed,
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
            (Self::Dispatching, SendTransition::MarkDelivered) => Ok((
                Self::Delivered,
                vec![SendEffect::UpdateFrameStatus, SendEffect::EmitFrame],
            )),
            (Self::Dispatching, SendTransition::MarkDeliveryFailed) => Ok((
                Self::DeliveryFailed,
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
            ConsoleFrameStatus::Delivered => Some(Self::Delivered),
            ConsoleFrameStatus::DeliveryFailed => Some(Self::DeliveryFailed),
            ConsoleFrameStatus::Completed => None,
        }
    }

    /// Whether the send has not settled: accepted or dispatching.
    pub(crate) fn is_unsettled(self) -> bool {
        matches!(self, Self::AcceptedPersisted | Self::Dispatching)
    }

    /// Reconcile an unsettled send, whose dispatch no live owner holds, from
    /// the runtime's admission evidence. Only native evidence settles it: an
    /// admitted input is a delivery; an absent one is dispatched again under
    /// the send's own delivery identity, into the session it recorded. Absent
    /// evidence never settles a send as failed here.
    pub(crate) fn reconcile(
        self,
        evidence: SendEvidence,
    ) -> Result<(Self, Vec<SendEffect>), &'static str> {
        match (self, evidence) {
            (Self::AcceptedPersisted | Self::Dispatching, SendEvidence::Admitted) => Ok((
                Self::Delivered,
                vec![SendEffect::UpdateFrameStatus, SendEffect::EmitFrame],
            )),
            (
                state @ (Self::AcceptedPersisted | Self::Dispatching),
                SendEvidence::AbsentRedispatchable,
            ) => Ok((state, vec![SendEffect::DispatchSend])),
            _ => Err("illegal send reconciliation"),
        }
    }
}

/// Native admission evidence for one unsettled console send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SendEvidence {
    /// The runtime holds an input under the send's delivery key.
    Admitted,
    /// The runtime holds no input under the send's delivery keys, and the
    /// send's delivery is keyed and pinned to the session it recorded, so
    /// dispatching it again cannot duplicate or retarget it.
    AbsentRedispatchable,
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
        }
        for settled in [
            SendState::Requested,
            SendState::Delivered,
            SendState::DeliveryFailed,
        ] {
            for evidence in [SendEvidence::Admitted, SendEvidence::AbsentRedispatchable] {
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
        assert!(!SendState::Delivered.is_unsettled());
        assert!(!SendState::DeliveryFailed.is_unsettled());
    }

    #[test]
    fn send_reducer_rejects_terminal_to_dispatch() {
        assert_eq!(
            SendState::Delivered.apply(SendTransition::StartDispatch),
            Err("illegal send state transition")
        );
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
