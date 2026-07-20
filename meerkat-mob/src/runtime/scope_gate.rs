//! Chokepoint (a) vocabulary: the routed command wrapper, the closed-world
//! `MobCommand → ControlScope` classification, and typed deny replies
//! (multi-host mobs phase 5, DEC-P5E-2/3/5; ADJ-P5-12/15).
//!
//! NOTE (lane relocation, FLAG-W-E): DEC-P5E-2 placed these beside
//! `MobCommand` in `state.rs`; the phase-5 wave plan assigns `state.rs` to
//! the policy lane (grant command payloads only), so the enforcement half
//! lives in this sibling module instead. Same `runtime` module, same
//! visibility surface.

use crate::MobError;
use crate::control_policy::{CommandAuthority, ScopeDenial};
use crate::machines::mob_machine::ControlScope;

use super::state::MobCommand;

/// Every command on the actor channel carries the authority lane it was
/// admitted under. Deferral (`deferred_commands` in `MobActor::run`)
/// preserves it for free.
///
/// There is deliberately NO `From<MobCommand>` and no default lane: an
/// unrouted send is a compile error, never a silently-Internal (deny) or
/// silently-Owner (admit) command.
pub(super) struct RoutedMobCommand {
    pub(super) authority: CommandAuthority,
    pub(super) cmd: MobCommand,
}

impl RoutedMobCommand {
    /// Actor/machine-internal self-send (provisioner completions, revival
    /// triggers, flow commits, signal projection). Internal authority
    /// carries no principal semantics: an internal send of an
    /// operator-class command is a wiring fault and is denied loudly at the
    /// gate (ADJ-P5-15).
    pub(super) fn internal(cmd: MobCommand) -> Self {
        Self {
            authority: CommandAuthority::internal(),
            cmd,
        }
    }
}

/// Gate verdict for one routed command (DEC-P5E-4). On `Denied` the typed
/// denial has already been sent down the command's own reply channel.
pub(super) enum ScopeAdmission {
    Admitted(MobCommand),
    Denied,
}

/// One wall-clock read per enforcement DECISION (DEC-P5P-6): UNIX epoch ms,
/// read at the seam and passed to `ResolvedControlPolicy::resolve` as data.
/// Saturates to u64::MAX on a pre-epoch clock: a broken clock must FAIL
/// CLOSED (every finite expiry reads as expired), never revive expired
/// grants. The machine itself never sees a clock (grant transitions in
/// `meerkat_machine_schema::catalog::dsl::mob_machine`).
pub(crate) fn control_now_ms() -> u64 {
    u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or(u64::MAX)
}

impl MobCommand {
    /// Closed-world operator classification (DEC-P5E-3, ratified ADJ-P5-12).
    ///
    /// `Some(scope)` = operator-class: a `Principal` sender must hold
    /// `scope` (owner implicit). `None` = internal/machine lane: no
    /// principal semantics. Exhaustive — NO wildcard arm — so every future
    /// variant forces a classification decision at compile time (E-F6
    /// posture).
    pub(super) fn required_control_scope(&self) -> Option<ControlScope> {
        match self {
            // ── SendCommand: drive members / work / flows ──
            Self::Spawn { .. }
            | Self::SubmitWork { .. }
            | Self::SendPeerMessage { .. }
            | Self::PreviewRunFlowAdmission { .. }
            | Self::RunFlow { .. }
            | Self::ConcludeObjective { .. }
            | Self::BindObjectiveOwner { .. } => Some(ControlScope::SendCommand),

            // ── Cancel ──
            Self::CancelAllWork { .. }
            | Self::CancelFlow { .. }
            | Self::ForceCancel { .. }
            | Self::HardCancelMember { .. } => Some(ControlScope::Cancel),

            // ── ReadHistory: member transcript reads (phase 6 — the
            //    ADJ-P5-13 marker's real verb) ──
            Self::MemberHistory { .. } => Some(ControlScope::ReadHistory),

            // ── Live: the duplex media-plane bearer family (phase 6b,
            //    DL8/DEC-P6B-C2). A WS bootstrap does not decompose into
            //    SendCommand/SubscribeEvents halves — issuing it IS the
            //    authorization event, so the ENTIRE verb family gates on
            //    Live, status point read included. ──
            Self::MemberLiveOpen { .. }
            | Self::MemberLiveClose { .. }
            | Self::MemberLiveStatus { .. }
            | Self::MemberLiveControl { .. } => Some(ControlScope::Live),

            // ── Retire: member retirement + the mob-lifecycle five
            //    (ADJ-P5-12: the frozen catalog has no mob-lifecycle scope;
            //    lifecycle mutations classify as the destructive class).
            //    Respawn = destroy+recreate ⇒ its most privileged half. ──
            Self::Retire { .. }
            | Self::RetireAll { .. }
            | Self::Respawn { .. }
            | Self::Stop { .. }
            | Self::ResumeLifecycle { .. }
            | Self::Complete { .. }
            | Self::Destroy { .. }
            | Self::Reset { .. }
            | Self::ApplyIdentityDeclarationManifest { .. } => Some(ControlScope::Retire),

            // ── WireTopology: trust/topology configuration ──
            Self::Wire { .. }
            | Self::WireMembersBatch { .. }
            | Self::Unwire { .. }
            | Self::DriveRouteInstalls { .. }
            | Self::DeclareMemberOutboundTaint { .. } => Some(ControlScope::WireTopology),

            // ── AdminHost: host-plane administration (A9: exactly
            //    bind/revoke hosts; supervisor authority is host-plane). ──
            Self::BindHost { .. } | Self::RevokeHost { .. } | Self::RotateSupervisor { .. } => {
                Some(ControlScope::AdminHost)
            }

            // ── SubscribeEvents: the mob event journal surface ──
            Self::PollEvents { .. } | Self::ReplayAllEvents { .. } => {
                Some(ControlScope::SubscribeEvents)
            }

            // ── List: pure reads with typed reply channels (the projection
            //    trio joined in phase 6 per the ADJ-P5-18 hard prerequisite:
            //    their reply channels now carry `Result<_, MobError>`, so an
            //    (a)-deny is a typed error, never a dropped channel) ──
            Self::FlowStatus { .. }
            | Self::ProjectMemberStatus { .. }
            | Self::ProjectMemberList { .. }
            | Self::GetIdentityIntent { .. }
            | Self::GetIdentityDeclarationReceipt { .. }
            | Self::GetIdentityConvergenceStatus { .. }
            | Self::MemberMachineProjection { .. }
            | Self::QueryPhase { .. } => Some(ControlScope::List),

            // Composite/projection surfaces choose their outer scope as data
            // and enter the same serialized actor admission before any raw
            // machine projection or roster/session lookup.
            Self::AdmitControlScope { required, .. } => Some(*required),

            // ── AdminGrants: the grant verb family gates itself (§17.2:815) ──
            Self::GrantScopes { .. } | Self::RevokeScopes { .. } | Self::Grants { .. } => {
                Some(ControlScope::AdminGrants)
            }

            // ── Internal / machine-authority plumbing (enumerated so the
            //    closed world stays reviewable) ──
            Self::SpawnProvisioned { .. }
            | Self::RevivePlacedMember { .. }
            | Self::HostStatusPollCompleted { .. }
            | Self::HostRuntimeIncarnationObserved { .. }
            | Self::HostOrphanReleaseCompleted { .. }
            | Self::PlacedBehaviorCompleted { .. }
            | Self::CommitFlowRunCommand { .. }
            | Self::CommitFlowTerminalization { .. }
            | Self::CommitFlowFrameStorePlan { .. }
            | Self::ProjectMachineInput { .. }
            | Self::ApplyMachineInputEffects { .. }
            | Self::ValidateCommandAuthority { .. }
            | Self::PruneStaleMemberOperatorRequests { .. }
            | Self::ReserveRemoteTurnObligation { .. }
            | Self::CommitRemoteTurnReceipt { .. }
            | Self::CloseRemoteTurnAfterTrackedCancel { .. }
            | Self::EnsureRemoteTurnRecord { .. }
            | Self::FinalizeRemoteTurnPrivacyCleanup { .. }
            | Self::ConvergeRecoveredFlowRun { .. }
            | Self::ResolveRemoteTurnOutcome { .. }
            | Self::AcknowledgeRemoteTurnOutcome { .. }
            | Self::RequestPlacedCompletionCancellation { .. }
            | Self::ResolvePlacedCompletionOutcome { .. }
            | Self::ClosePlacedCompletionOutcome { .. }
            | Self::AcknowledgePlacedCompletionOutcome { .. }
            | Self::ResolvePlacedKickoffOutcome { .. }
            | Self::ResolvePlacedKickoffCancelled { .. }
            | Self::AcknowledgePlacedKickoffOutcome { .. }
            | Self::RejectPlacedKickoffBeforeAdmission { .. }
            | Self::PreviewMachineInput { .. }
            | Self::QueryMachineState { .. }
            | Self::ApplyExternalPeerReciprocalTrust { .. }
            | Self::ProjectMachineSignal { .. }
            | Self::RecordMissingMemberBridgeSession { .. }
            | Self::FlowFinished { .. }
            | Self::FlowCanceledCleanup { .. }
            | Self::StartupKickoffSnapshot { .. }
            // A17 pump-liveness poke from the flow lane / recovery — an
            // internal mechanism, never an operator verb.
            | Self::EnsureMemberEventPump { .. }
            // Realization of a machine-authorized external subscription
            // (authorization happened at the machine input).
            | Self::EnsureMemberEventTap { .. }
            // Provenance recording is informational, never enforcement.
            | Self::RecordOperatorActionProvenance { .. }
            // Builder/test config seam; no principal caller today —
            // reclassify the moment a surface exposes it.
            | Self::SetSpawnPolicy { .. }
            | Self::Shutdown { .. } => None,

            #[cfg(feature = "runtime-adapter")]
            Self::KickoffOutcomeResolved { .. } => None,

            #[cfg(any(test, feature = "test-support"))]
            Self::CrashStopPreservingDurableWorkForTest { .. } => None,

            #[cfg(test)]
            Self::AuthorizeMemberTrustCleanupForTest { .. }
            | Self::StagePendingSpawnForRetireTest { .. }
            | Self::FlowTrackerCounts { .. }
            | Self::OrchestratorSnapshot { .. }
            | Self::LifecycleSnapshot { .. }
            | Self::LifecycleNotificationBurst { .. }
            | Self::DslT2Snapshot { .. } => None,
        }
    }

    /// Consume a denied command, sending the typed denial down the
    /// variant's own reply channel (DEC-P5E-5). Exhaustive — no wildcard —
    /// so a new variant cannot silently gain a lossy deny path. Non-MobError
    /// reply channels embed via their existing `Mob(#[from] MobError)`
    /// variants. A fenced member-operator handle can deny even a
    /// `None`-classified internal command before the scope classification
    /// early return, so Result-bearing internal arms preserve that typed
    /// failure and only truly reply-less variants log-drop.
    pub(super) fn reject_scope_denied(self, denial: ScopeDenial) {
        self.reject_with_error(MobError::ScopeDenied(denial));
    }

    /// Send any typed actor-admission failure through the command's native
    /// reply channel. The member-operator execution fence uses this after a
    /// stale residency is detected before handler/effect admission.
    pub(super) fn reject_with_error(self, error: MobError) {
        match self {
            Self::Spawn { reply_tx, .. } => {
                let _ = reply_tx.send(Err(error));
            }
            Self::Retire { reply_tx, .. } => {
                let _ = reply_tx.send(Err(error));
            }
            Self::Respawn { reply_tx, .. } => {
                let _ = reply_tx.send(Err(super::handle::MobRespawnError::from(error)));
            }
            Self::RetireAll { reply_tx } => {
                let _ = reply_tx.send(Err(error));
            }
            Self::SubmitWork { reply_tx, .. } => {
                let _ = reply_tx.send(Err(error));
            }
            Self::SendPeerMessage { reply_tx, .. } => {
                let _ = reply_tx.send(Err(error));
            }
            Self::ConcludeObjective { reply_tx, .. }
            | Self::BindObjectiveOwner { reply_tx, .. } => {
                let _ = reply_tx.send(Err(error));
            }
            Self::DeclareMemberOutboundTaint { reply_tx, .. } => {
                let _ = reply_tx.send(Err(error));
            }
            Self::CancelAllWork { reply_tx, .. } => {
                let _ = reply_tx.send(Err(error));
            }
            Self::RunFlow { reply_tx, .. } => {
                let _ = reply_tx.send(Err(error));
            }
            Self::PreviewRunFlowAdmission { reply_tx } => {
                let _ = reply_tx.send(Err(error));
            }
            Self::CancelFlow { reply_tx, .. } => {
                let _ = reply_tx.send(Err(error));
            }
            Self::FlowStatus { reply_tx, .. } => {
                let _ = reply_tx.send(Err(error));
            }
            Self::ProjectMemberStatus { reply_tx, .. } => {
                let _ = reply_tx.send(Err(error));
            }
            Self::ApplyIdentityDeclarationManifest { reply_tx, .. } => {
                let _ = reply_tx.send(Err(error));
            }
            Self::GetIdentityIntent { reply_tx, .. } => {
                let _ = reply_tx.send(Err(error));
            }
            Self::GetIdentityDeclarationReceipt { reply_tx, .. } => {
                let _ = reply_tx.send(Err(error));
            }
            Self::GetIdentityConvergenceStatus { reply_tx, .. } => {
                let _ = reply_tx.send(Err(error));
            }
            Self::AdmitControlScope { reply_tx, .. } => {
                let _ = reply_tx.send(Err(error));
            }
            Self::Stop { reply_tx } | Self::ResumeLifecycle { reply_tx } => {
                let _ = reply_tx.send(Err(error));
            }
            Self::Complete { reply_tx } | Self::Reset { reply_tx } => {
                let _ = reply_tx.send(Err(error));
            }
            Self::Destroy { reply_tx } => {
                let _ = reply_tx.send(Err(super::handle::MobDestroyError::from(error)));
            }
            Self::RotateSupervisor { reply_tx } => {
                let _ = reply_tx.send(Err(error));
            }
            Self::BindHost { reply_tx, .. } => {
                let _ = reply_tx.send(Err(error));
            }
            Self::RevokeHost { reply_tx, .. } => {
                let _ = reply_tx.send(Err(error));
            }
            Self::PollEvents { reply_tx, .. } | Self::ReplayAllEvents { reply_tx } => {
                let _ = reply_tx.send(Err(error));
            }
            Self::ForceCancel { reply_tx, .. } => {
                let _ = reply_tx.send(Err(error));
            }
            Self::HardCancelMember { reply_tx, .. } => {
                let _ = reply_tx.send(Err(error));
            }
            Self::MemberHistory { reply_tx, .. } => {
                let _ = reply_tx.send(Err(error));
            }
            Self::MemberLiveOpen { reply_tx, .. } => {
                let _ = reply_tx.send(Err(error));
            }
            Self::MemberLiveClose { reply_tx, .. } => {
                let _ = reply_tx.send(Err(error));
            }
            Self::MemberLiveStatus { reply_tx, .. } => {
                let _ = reply_tx.send(Err(error));
            }
            Self::MemberLiveControl { reply_tx, .. } => {
                let _ = reply_tx.send(Err(error));
            }
            // ADJ-P5-18: the projection trio now denies typed through its
            // Result reply channels — the closed-channel→WATCH laundering in
            // `MobHandle::status()` is dead.
            Self::ProjectMemberList { reply_tx, .. } => {
                let _ = reply_tx.send(Err(error));
            }
            Self::MemberMachineProjection { reply_tx, .. } => {
                let _ = reply_tx.send(Err(error));
            }
            Self::QueryPhase { reply_tx } => {
                let _ = reply_tx.send(Err(error));
            }
            Self::Wire { reply_tx, .. } | Self::Unwire { reply_tx, .. } => {
                let _ = reply_tx.send(Err(error));
            }
            Self::WireMembersBatch { reply_tx, .. } => {
                let _ = reply_tx.send(Err(error));
            }
            Self::DriveRouteInstalls { reply_tx } => {
                let _ = reply_tx.send(Err(error));
            }
            Self::GrantScopes { reply_tx, .. } => {
                let _ = reply_tx.send(Err(error));
            }
            Self::RevokeScopes { reply_tx, .. } => {
                let _ = reply_tx.send(Err(error));
            }
            Self::Grants { reply_tx, .. } => {
                let _ = reply_tx.send(Err(error));
            }
            // Fenced remote-upcall handles also route their machine admission
            // and other actor-serialized queries through this gate. Preserve
            // the typed stale-authority error on every Result-bearing internal
            // channel instead of laundering it into channel closure.
            Self::CommitFlowRunCommand { reply_tx, .. } => {
                let _ = reply_tx.send(Err(error));
            }
            Self::CommitFlowTerminalization { reply_tx, .. } => {
                let _ = reply_tx.send(Err(error));
            }
            Self::CommitFlowFrameStorePlan { reply_tx, .. } => {
                let _ = reply_tx.send(Err(error));
            }
            Self::ProjectMachineInput { reply_tx, .. }
            | Self::PreviewMachineInput { reply_tx, .. } => {
                let _ = reply_tx.send(Err(error));
            }
            Self::ApplyMachineInputEffects { reply_tx, .. } => {
                let _ = reply_tx.send(Err(error));
            }
            Self::ValidateCommandAuthority { reply_tx } => {
                let _ = reply_tx.send(Err(error));
            }
            Self::PruneStaleMemberOperatorRequests { reply_tx } => {
                let _ = reply_tx.send(Err(error));
            }
            Self::ReserveRemoteTurnObligation { reply_tx, .. } => {
                let _ = reply_tx.send(Err(error));
            }
            Self::CommitRemoteTurnReceipt { reply_tx, .. }
            | Self::CloseRemoteTurnAfterTrackedCancel { reply_tx, .. }
            | Self::EnsureRemoteTurnRecord { reply_tx, .. }
            | Self::FinalizeRemoteTurnPrivacyCleanup { reply_tx, .. }
            | Self::ConvergeRecoveredFlowRun { reply_tx, .. }
            | Self::ResolveRemoteTurnOutcome { reply_tx, .. }
            | Self::AcknowledgeRemoteTurnOutcome { reply_tx, .. }
            | Self::RequestPlacedCompletionCancellation { reply_tx, .. }
            | Self::ResolvePlacedCompletionOutcome { reply_tx, .. }
            | Self::ClosePlacedCompletionOutcome { reply_tx, .. }
            | Self::AcknowledgePlacedCompletionOutcome { reply_tx, .. }
            | Self::ResolvePlacedKickoffOutcome { reply_tx, .. }
            | Self::ResolvePlacedKickoffCancelled { reply_tx, .. }
            | Self::AcknowledgePlacedKickoffOutcome { reply_tx, .. }
            | Self::RejectPlacedKickoffBeforeAdmission { reply_tx, .. }
            | Self::ApplyExternalPeerReciprocalTrust { reply_tx, .. }
            | Self::EnsureMemberEventPump { reply_tx, .. }
            | Self::HostRuntimeIncarnationObserved { reply_tx, .. }
            | Self::RecordOperatorActionProvenance { reply_tx, .. }
            | Self::SetSpawnPolicy { reply_tx, .. }
            | Self::Shutdown { reply_tx } => {
                let _ = reply_tx.send(Err(error));
            }
            Self::EnsureMemberEventTap { reply_tx, .. } => {
                let _ = reply_tx.send(Err(error));
            }
            #[cfg(test)]
            Self::AuthorizeMemberTrustCleanupForTest { reply_tx, .. } => {
                let _ = reply_tx.send(Err(error));
            }
            #[cfg(test)]
            Self::StagePendingSpawnForRetireTest { reply_tx, .. } => {
                let _ = reply_tx.send(Err(error));
            }
            #[cfg(test)]
            Self::LifecycleNotificationBurst { reply_tx, .. } => {
                let _ = reply_tx.send(Err(error));
            }
            #[cfg(any(test, feature = "test-support"))]
            Self::CrashStopPreservingDurableWorkForTest { reply_tx } => {
                let _ = reply_tx.send(Err(error));
            }
            // Internal-class / non-Result-channel arms have no typed error
            // carrier; log-drop honestly if a fenced handle reaches one.
            Self::SpawnProvisioned { .. }
            | Self::RevivePlacedMember { .. }
            | Self::HostStatusPollCompleted { .. }
            | Self::HostOrphanReleaseCompleted { .. }
            | Self::PlacedBehaviorCompleted { .. }
            | Self::QueryMachineState { .. }
            | Self::ProjectMachineSignal { .. }
            | Self::RecordMissingMemberBridgeSession { .. }
            | Self::FlowFinished { .. }
            | Self::FlowCanceledCleanup { .. }
            | Self::StartupKickoffSnapshot { .. } => {
                tracing::error!(
                    command_kind = self.kind(),
                    required = ?error,
                    "scope denial reached a command without a typed reply \
                     channel; reply dropped (wiring fault)"
                );
            }
            #[cfg(feature = "runtime-adapter")]
            Self::KickoffOutcomeResolved { .. } => {
                tracing::error!("scope denial reached internal KickoffOutcomeResolved; dropped");
            }
            #[cfg(test)]
            Self::FlowTrackerCounts { .. }
            | Self::OrchestratorSnapshot { .. }
            | Self::LifecycleSnapshot { .. }
            | Self::DslT2Snapshot { .. } => {
                tracing::error!("scope denial reached a test-only command; dropped");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use tokio::sync::oneshot;

    // ADJ-P5-12 ratified-row pins (cheap-to-construct variants).
    #[test]
    fn ratified_classifications_hold() {
        let (tx, _rx) = oneshot::channel();
        assert_eq!(
            MobCommand::Respawn {
                agent_identity: crate::ids::AgentIdentity::from("m"),
                initial_message: None,
                reply_tx: tx,
            }
            .required_control_scope(),
            Some(ControlScope::Retire),
            "Respawn classifies as its most privileged half"
        );
        let (tx, _rx) = oneshot::channel();
        assert_eq!(
            MobCommand::Stop { reply_tx: tx }.required_control_scope(),
            Some(ControlScope::Retire),
        );
        let (tx, _rx) = oneshot::channel();
        assert_eq!(
            MobCommand::RotateSupervisor { reply_tx: tx }.required_control_scope(),
            Some(ControlScope::AdminHost),
        );
        let (tx, _rx) = oneshot::channel();
        assert_eq!(
            MobCommand::DriveRouteInstalls { reply_tx: tx }.required_control_scope(),
            Some(ControlScope::WireTopology),
        );
        let (tx, _rx) = oneshot::channel();
        assert_eq!(
            MobCommand::ReplayAllEvents { reply_tx: tx }.required_control_scope(),
            Some(ControlScope::SubscribeEvents),
        );
        let (tx, _rx) = oneshot::channel();
        assert_eq!(
            MobCommand::PreviewRunFlowAdmission { reply_tx: tx }.required_control_scope(),
            Some(ControlScope::SendCommand),
            "flow preflight is part of the mutating RunFlow verb, not an independent read",
        );
        let (tx, _rx) = oneshot::channel();
        assert_eq!(
            MobCommand::AdmitControlScope {
                required: ControlScope::Cancel,
                reply_tx: tx,
            }
            .required_control_scope(),
            Some(ControlScope::Cancel),
            "composite admission carries the operation's exact scope through the actor gate",
        );
        let (tx, _rx) = oneshot::channel();
        assert_eq!(
            MobCommand::Grants {
                caller: crate::control_policy::MobControlPrincipal::Owner,
                reply_tx: tx,
            }
            .required_control_scope(),
            Some(ControlScope::AdminGrants),
        );
        // ADJ-P5-18 prerequisite landed: the projection trio is List-scoped
        // with typed Result reply channels.
        let (tx, _rx) = oneshot::channel();
        assert_eq!(
            MobCommand::QueryPhase { reply_tx: tx }.required_control_scope(),
            Some(ControlScope::List),
        );
        let (tx, _rx) = oneshot::channel();
        assert_eq!(
            MobCommand::HardCancelMember {
                agent_identity: crate::ids::AgentIdentity::from("m"),
                reason: "test".to_string(),
                reply_tx: tx,
            }
            .required_control_scope(),
            Some(ControlScope::Cancel),
        );
        let (tx, _rx) = oneshot::channel();
        assert_eq!(
            MobCommand::MemberHistory {
                agent_identity: crate::ids::AgentIdentity::from("m"),
                from_index: None,
                limit: None,
                reply_tx: tx,
            }
            .required_control_scope(),
            Some(ControlScope::ReadHistory),
        );
        let (tx, _rx) = oneshot::channel();
        assert_eq!(
            MobCommand::ConcludeObjective {
                agent_identity: crate::ids::AgentIdentity::from("m"),
                objective_id: meerkat_core::interaction::ObjectiveId::new(),
                outcome: "completed".to_string(),
                reply_tx: tx,
            }
            .required_control_scope(),
            Some(ControlScope::SendCommand),
        );
        let (tx, _rx) = oneshot::channel();
        assert_eq!(
            MobCommand::BindObjectiveOwner {
                owner_identity: crate::ids::AgentIdentity::from("lead"),
                objective_id: meerkat_core::interaction::ObjectiveId::new(),
                reply_tx: tx,
            }
            .required_control_scope(),
            Some(ControlScope::SendCommand),
        );
    }

    #[tokio::test]
    async fn objective_mutation_denials_are_typed() {
        let denial = || ScopeDenial {
            required: ControlScope::SendCommand,
            presented: BTreeSet::from([ControlScope::List]),
        };

        let (reply_tx, reply_rx) = oneshot::channel();
        MobCommand::ConcludeObjective {
            agent_identity: crate::ids::AgentIdentity::from("m"),
            objective_id: meerkat_core::interaction::ObjectiveId::new(),
            outcome: "completed".to_string(),
            reply_tx,
        }
        .reject_scope_denied(denial());
        assert!(
            matches!(
                reply_rx.await,
                Ok(Err(MobError::ScopeDenied(ScopeDenial {
                    required: ControlScope::SendCommand,
                    ..
                })))
            ),
            "ConcludeObjective denial must use its typed reply channel"
        );

        let (reply_tx, reply_rx) = oneshot::channel();
        MobCommand::BindObjectiveOwner {
            owner_identity: crate::ids::AgentIdentity::from("lead"),
            objective_id: meerkat_core::interaction::ObjectiveId::new(),
            reply_tx,
        }
        .reject_scope_denied(denial());
        assert!(
            matches!(
                reply_rx.await,
                Ok(Err(MobError::ScopeDenied(ScopeDenial {
                    required: ControlScope::SendCommand,
                    ..
                })))
            ),
            "BindObjectiveOwner denial must use its typed reply channel"
        );
    }

    // T-C12 (phase 6b, DEC-P6B-C2): the ENTIRE live verb family classifies
    // `Live` — bootstrap issuance is the scope-gated act (DL8), the status
    // point read included.
    #[test]
    fn live_family_classifies_live() {
        let (tx, _rx) = oneshot::channel();
        assert_eq!(
            MobCommand::MemberLiveOpen {
                agent_identity: crate::ids::AgentIdentity::from("m"),
                turning_mode: None,
                transport: None,
                reply_tx: tx,
            }
            .required_control_scope(),
            Some(ControlScope::Live),
        );
        let (tx, _rx) = oneshot::channel();
        assert_eq!(
            MobCommand::MemberLiveClose {
                agent_identity: crate::ids::AgentIdentity::from("m"),
                channel_id: "chan-1".to_string(),
                reply_tx: tx,
            }
            .required_control_scope(),
            Some(ControlScope::Live),
        );
        let (tx, _rx) = oneshot::channel();
        assert_eq!(
            MobCommand::MemberLiveStatus {
                agent_identity: crate::ids::AgentIdentity::from("m"),
                channel_id: None,
                reply_tx: tx,
            }
            .required_control_scope(),
            Some(ControlScope::Live),
        );
        let (tx, _rx) = oneshot::channel();
        assert_eq!(
            MobCommand::MemberLiveControl {
                agent_identity: crate::ids::AgentIdentity::from("m"),
                channel_id: "chan-1".to_string(),
                verb: super::super::bridge_protocol::BridgeLiveControlVerb::CommitInput,
                reply_tx: tx,
            }
            .required_control_scope(),
            Some(ControlScope::Live),
        );
    }

    // T-C12: each live variant's denial travels TYPED down its own reply
    // channel (DEC-P5E-5 — never a dropped channel).
    #[tokio::test]
    async fn live_family_denials_are_typed_per_channel() {
        let denial = || ScopeDenial {
            required: ControlScope::Live,
            presented: BTreeSet::from([ControlScope::SendCommand]),
        };
        let expect_denied = |result: Result<Result<_, MobError>, _>| match result {
            Ok(Err(MobError::ScopeDenied(denial))) => {
                assert_eq!(denial.required, ControlScope::Live);
                assert_eq!(
                    denial.presented,
                    BTreeSet::from([ControlScope::SendCommand])
                );
            }
            other => panic!("expected typed ScopeDenied reply, got {other:?}"),
        };

        let (reply_tx, reply_rx) = oneshot::channel();
        MobCommand::MemberLiveOpen {
            agent_identity: crate::ids::AgentIdentity::from("m"),
            turning_mode: None,
            transport: None,
            reply_tx,
        }
        .reject_scope_denied(denial());
        expect_denied(reply_rx.await.map(|reply| reply.map(|_| ())));

        let (reply_tx, reply_rx) = oneshot::channel();
        MobCommand::MemberLiveClose {
            agent_identity: crate::ids::AgentIdentity::from("m"),
            channel_id: "chan-1".to_string(),
            reply_tx,
        }
        .reject_scope_denied(denial());
        expect_denied(reply_rx.await.map(|reply| reply.map(|_| ())));

        let (reply_tx, reply_rx) = oneshot::channel();
        MobCommand::MemberLiveStatus {
            agent_identity: crate::ids::AgentIdentity::from("m"),
            channel_id: None,
            reply_tx,
        }
        .reject_scope_denied(denial());
        expect_denied(reply_rx.await.map(|reply| reply.map(|_| ())));

        let (reply_tx, reply_rx) = oneshot::channel();
        MobCommand::MemberLiveControl {
            agent_identity: crate::ids::AgentIdentity::from("m"),
            channel_id: "chan-1".to_string(),
            verb: super::super::bridge_protocol::BridgeLiveControlVerb::Interrupt,
            reply_tx,
        }
        .reject_scope_denied(denial());
        expect_denied(reply_rx.await.map(|reply| reply.map(|_| ())));
    }

    // ADJ-P5-18: a denied projection read is a typed error on its own reply
    // channel — never a dropped channel laundered into lifecycle data.
    #[tokio::test]
    async fn query_phase_denial_is_typed() {
        let (reply_tx, reply_rx) = oneshot::channel();
        MobCommand::QueryPhase { reply_tx }.reject_scope_denied(ScopeDenial {
            required: ControlScope::List,
            presented: BTreeSet::new(),
        });
        match reply_rx.await {
            Ok(Err(MobError::ScopeDenied(denial))) => {
                assert_eq!(denial.required, ControlScope::List);
            }
            other => panic!("expected typed ScopeDenied reply, got {other:?}"),
        }
    }

    // The deny reply is TYPED end-to-end (T-M14's gate-unit half).
    #[tokio::test]
    async fn reject_scope_denied_sends_typed_error() {
        let (reply_tx, reply_rx) = oneshot::channel();
        let cmd = MobCommand::Retire {
            agent_identity: crate::ids::AgentIdentity::from("m"),
            reply_tx,
        };
        cmd.reject_scope_denied(ScopeDenial {
            required: ControlScope::Retire,
            presented: BTreeSet::from([ControlScope::List]),
        });
        match reply_rx.await {
            Ok(Err(MobError::ScopeDenied(denial))) => {
                assert_eq!(denial.required, ControlScope::Retire);
                assert_eq!(denial.presented, BTreeSet::from([ControlScope::List]));
            }
            other => panic!("expected typed ScopeDenied reply, got {other:?}"),
        }
    }

    // Non-MobError reply channels embed via their Mob(#[from]) variants.
    #[tokio::test]
    async fn destroy_denial_embeds_in_mob_destroy_error() {
        let (reply_tx, reply_rx) = oneshot::channel();
        MobCommand::Destroy { reply_tx }.reject_scope_denied(ScopeDenial {
            required: ControlScope::Retire,
            presented: BTreeSet::new(),
        });
        match reply_rx.await {
            Ok(Err(super::super::handle::MobDestroyError::Mob(MobError::ScopeDenied(denial)))) => {
                assert_eq!(denial.required, ControlScope::Retire);
                assert!(denial.presented.is_empty());
            }
            other => panic!("expected embedded ScopeDenied, got {other:?}"),
        }
    }
}
