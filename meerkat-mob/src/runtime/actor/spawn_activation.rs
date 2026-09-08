//! Off-actor spawn activation (#1105).
//!
//! Spawn admission (`finalize_spawn_admit`) is machine authority plus a
//! durable membership commit: it belongs on the actor and stays there. Spawn
//! ACTIVATION — trusted-peer publication, peer-ingress drain startup,
//! autonomous runtime start, kickoff admission, and the turn-driven initial
//! turn — is opaque third-party work: a provisioner call, a comms transport
//! bind, a runtime admission. Running those on the actor made one wedged
//! member's activation stall `QueryPhase` and every unrelated member's turn.
//!
//! This module keeps the split honest:
//!
//! * The **actor** runs only `prepare` and `commit` steps. Every step here is
//!   machine authority, durable append, or a bounded local projection write.
//!   No step awaits a provisioner, a bridge, a comms runtime, or a session
//!   service.
//! * A **process worker** ([`SpawnActivationWorkerContext`]) owns each opaque
//!   stage. It holds cloned resources only — never `&mut MobActor` — and
//!   carries the exact incarnation custody ([`SpawnActivationCustody`]) of the
//!   activation that dispatched it.
//! * The worker routes a typed [`SpawnActivationStageOutcome`] back through
//!   the ordinary command channel
//!   (`MobCommand::SpawnActivationStageSettled`). Callers never await a future
//!   that needs the same actor command queue to resolve; they hand the
//!   pipeline a typed [`SpawnActivationRoute`] and the driver delivers the
//!   receipt when the last stage commits.
//!
//! Custody rules:
//!
//! * A stage outcome is admitted only when the roster still holds the exact
//!   incarnation (identity + generation + fence + runtime id) that dispatched
//!   it. A late outcome for a superseded incarnation is DECLINED: it never
//!   rolls back the successor, and the decline is recorded as retained
//!   cleanup rather than silently dropped.
//! * A dropped caller (`reply_tx` closed) is not cancellation. The pipeline
//!   runs to its own terminal state and settles the machine; only the reply
//!   delivery is lost.
//! * When compensation itself fails, the failure is retained
//!   ([`RetainedSpawnActivationCleanup`]) until an owner settles it. It is
//!   never folded into a success.

use super::*;

/// Monotonic per-actor identifier for one spawn activation.
///
/// Mirrors the `actor_ticket!` shape used by the admission and resume lanes;
/// declared by hand because that macro is defined after this module's
/// declaration site.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(in crate::runtime) struct SpawnActivationTicket(u64);

impl SpawnActivationTicket {
    fn next(&mut self) -> Result<Self, MobError> {
        self.0 = self.0.checked_add(1).ok_or_else(|| {
            MobError::Internal("spawn activation ticket space exhausted".to_string())
        })?;
        Ok(*self)
    }
}

impl std::fmt::Display for SpawnActivationTicket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Exact incarnation a staged activation step belongs to.
///
/// Every off-actor stage carries this verbatim. The actor re-verifies it
/// against the live roster projection before committing the stage, so a late
/// completion from a superseded incarnation can never mutate its successor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::runtime) struct SpawnActivationCustody {
    pub(in crate::runtime) agent_identity: AgentIdentity,
    pub(in crate::runtime) generation: crate::ids::Generation,
    pub(in crate::runtime) fence_token: crate::ids::FenceToken,
    pub(in crate::runtime) agent_runtime_id: crate::ids::AgentRuntimeId,
    pub(in crate::runtime) operation_id: meerkat_core::ops::OperationId,
    /// The exact resource this custody fences. Identity + generation + fence
    /// name an incarnation; only the member ref names the SESSION/PEER the
    /// staged effect actually touched, so it belongs in the fence.
    pub(in crate::runtime) member_ref: MemberRef,
}

impl SpawnActivationCustody {
    fn from_state(state: &SpawnActivateState) -> Self {
        Self {
            agent_identity: state.agent_identity.clone(),
            generation: state.generation,
            fence_token: state.fence_token,
            agent_runtime_id: state.agent_runtime_id.clone(),
            operation_id: state.operation_id.clone(),
            member_ref: state.member_ref.clone(),
        }
    }

    /// Roster-comparable form: the roster stores the CANONICALIZED member ref
    /// (the bridge address without its bootstrap token), while custody keeps
    /// the raw ref the effect used.
    fn roster_comparable_member_ref(&self) -> MemberRef {
        MobActor::sanitized_member_ref(&self.member_ref)
    }
}

/// Typed answer to "does this custody still own its resource?".
///
/// `Absent` and `Superseded` are DIFFERENT facts and must not collapse into
/// one boolean: absence is positive evidence that a staged effect's target is
/// gone, while supersession means someone else owns the identity now and this
/// custody must touch nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SpawnActivationCustodyState {
    /// The exact incarnation, including its member ref, is still current.
    Current,
    /// The identity exists but under a different incarnation, or the machine
    /// still holds a live runtime id this custody cannot claim.
    Superseded,
    /// Neither the roster projection nor the machine's live runtime set knows
    /// this incarnation. Its effects have no target left.
    Absent,
}

/// One opaque activation stage owned by the process worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::runtime) enum SpawnActivationStage {
    /// `publish_trusted_peer_spec_for_operation` for a session-backed member.
    TrustedPeerPublish,
    /// Durable peer ingress (mob comms drain) required before wiring can send
    /// `peer_added` to the freshly spawned member.
    WiringPeerIngress,
    /// Local autonomous readiness: comms drain + injector capability.
    AutonomousReadiness,
    /// Local autonomous kickoff admission through the runtime adapter.
    AutonomousKickoff,
    /// Turn-driven persistent comms drain.
    TurnDrivenPeerIngress,
    /// Turn-driven initial turn: MobMachine already admitted it on the
    /// actor; the worker runs the member-local readiness and the runtime
    /// admission.
    InitialTurn,
}

impl SpawnActivationStage {
    pub(in crate::runtime) fn as_str(self) -> &'static str {
        match self {
            Self::TrustedPeerPublish => "trusted_peer_publish",
            Self::WiringPeerIngress => "wiring_peer_ingress",
            Self::AutonomousReadiness => "autonomous_readiness",
            Self::AutonomousKickoff => "autonomous_kickoff",
            Self::TurnDrivenPeerIngress => "turn_driven_peer_ingress",
            Self::InitialTurn => "initial_turn",
        }
    }
}

/// Typed completion of one staged activation step.
pub(in crate::runtime) struct SpawnActivationStageOutcome {
    pub(in crate::runtime) stage: SpawnActivationStage,
    pub(in crate::runtime) custody: SpawnActivationCustody,
    pub(in crate::runtime) result: Result<(), MobError>,
}

/// Off-actor work for one stage. Owns every resource it needs; it never
/// borrows actor state.
enum SpawnActivationWork {
    PublishTrustedPeer {
        member_ref: MemberRef,
        operation_id: meerkat_core::ops::OperationId,
        endpoint: TrustedPeerDescriptor,
    },
    /// Wiring-phase peer ingress. Fails closed: wiring cannot send durable
    /// peer lifecycle to a member with no ingress.
    RequiredPeerIngress {
        member_ref: MemberRef,
    },
    AutonomousReadiness {
        member_ref: MemberRef,
    },
    #[cfg(feature = "runtime-adapter")]
    AutonomousKickoff {
        bridge_session_id: SessionId,
        input: Box<meerkat_runtime::Input>,
    },
    /// Turn-driven drain. A member with no comms runtime is skipped, matching
    /// the pre-#1105 inline behavior of the turn-driven kickoff lane.
    OptionalPeerIngress {
        member_ref: MemberRef,
    },
    /// Turn-driven initial turn, already admitted by MobMachine on the actor.
    /// Only the member-local readiness and the runtime admission remain.
    InitialTurn {
        completion: Box<SubmitWorkDispatchCompletion>,
    },
}

/// Cloned actor material the activation worker runs on.
#[derive(Clone)]
pub(in crate::runtime) struct SpawnActivationWorkerContext {
    readiness: DetachedMemberReadinessContext,
    provisioner: Arc<dyn MobProvisioner>,
    session_service: Arc<dyn MobSessionService>,
    #[cfg(feature = "runtime-adapter")]
    runtime_adapter: Option<Arc<meerkat_runtime::MeerkatMachine>>,
    #[cfg(feature = "runtime-adapter")]
    autonomous_initial_turns: Arc<tokio::sync::Mutex<BTreeMap<AgentIdentity, InitialTurnHandle>>>,
    command_tx: mpsc::Sender<RoutedMobCommand>,
    mob_id: MobId,
}

impl SpawnActivationWorkerContext {
    async fn run(
        &self,
        custody: &SpawnActivationCustody,
        work: SpawnActivationWork,
    ) -> Result<(), MobError> {
        let agent_identity = &custody.agent_identity;
        match work {
            SpawnActivationWork::PublishTrustedPeer {
                member_ref,
                operation_id,
                endpoint,
            } => {
                self.provisioner
                    .publish_trusted_peer_spec_for_operation(&member_ref, &operation_id, endpoint)
                    .await
            }
            SpawnActivationWork::RequiredPeerIngress { member_ref } => {
                self.readiness
                    .ensure_mob_comms_drain(agent_identity, &member_ref)
                    .await
            }
            SpawnActivationWork::AutonomousReadiness { member_ref } => {
                self.readiness
                    .ensure_autonomous_runtime_ready(agent_identity, &member_ref)
                    .await
            }
            #[cfg(feature = "runtime-adapter")]
            SpawnActivationWork::AutonomousKickoff {
                bridge_session_id,
                input,
            } => {
                self.admit_autonomous_kickoff(agent_identity, bridge_session_id, *input)
                    .await
            }
            SpawnActivationWork::OptionalPeerIngress { member_ref } => {
                self.optional_peer_ingress(agent_identity, &member_ref)
                    .await
            }
            SpawnActivationWork::InitialTurn { completion } => {
                self.realize_initial_turn(agent_identity, *completion).await
            }
        }
    }

    /// Off-actor realization of the turn-driven spawn kickoff.
    ///
    /// Preserves the pre-#1105 inline realization exactly — including the
    /// operation-scoped admission the spawn kickoff depends on — but runs it
    /// through the detached readiness helper, whose #37 revival re-enters the
    /// actor that is now free to answer it.
    async fn realize_initial_turn(
        &self,
        agent_identity: &AgentIdentity,
        completion: SubmitWorkDispatchCompletion,
    ) -> Result<(), MobError> {
        match completion {
            SubmitWorkDispatchCompletion::Completed => Ok(()),
            SubmitWorkDispatchCompletion::AwaitPolicySpawn { .. } => {
                Err(MobError::Internal(format!(
                    "turn-driven spawn kickoff for '{agent_identity}' requested a second policy spawn"
                )))
            }
            SubmitWorkDispatchCompletion::AwaitAutonomousDispatch {
                agent_identity,
                readiness,
                material,
            } => {
                if let Some(readiness) = readiness {
                    MobActor::run_local_turn_readiness(
                        &self.readiness,
                        &self.session_service,
                        &self.command_tx,
                        &agent_identity,
                        &readiness,
                        &material.member_ref,
                    )
                    .await?;
                }
                self.readiness
                    .dispatch_autonomous(&agent_identity, *material)
                    .await
            }
            SubmitWorkDispatchCompletion::AwaitTurnAdmission {
                operation_id,
                agent_identity,
                readiness,
                member_ref,
                req,
                completion_tx,
                llm_identity_applied_tx,
                placed_identity,
                placed_incarnation,
                placed_input_id,
            } => {
                if let Some(readiness) = readiness {
                    MobActor::run_local_turn_readiness(
                        &self.readiness,
                        &self.session_service,
                        &self.command_tx,
                        &agent_identity,
                        &readiness,
                        &member_ref,
                    )
                    .await?;
                }
                debug_assert_eq!(placed_identity.is_some(), placed_incarnation.is_some());
                let result = match (placed_incarnation, placed_input_id) {
                    (Some(_), _) | (None, Some(_)) => {
                        // A placed member's kickoff rides the durable placed
                        // delivery lane, never this local admission.
                        Err(MobError::Internal(format!(
                            "turn-driven spawn kickoff for '{agent_identity}' resolved to a placed admission"
                        )))
                    }
                    (None, None) => {
                        if let Some(completion_tx) = completion_tx {
                            self.provisioner
                                .admit_tracked_turn(
                                    &member_ref,
                                    *req,
                                    completion_tx,
                                    llm_identity_applied_tx,
                                )
                                .await
                        } else if let Some(operation_id) = operation_id.as_ref() {
                            self.provisioner
                                .admit_turn_for_operation(&member_ref, operation_id, *req)
                                .await
                        } else {
                            self.provisioner.admit_turn(&member_ref, *req).await
                        }
                    }
                };
                if let Err(error) = &result {
                    MobActor::fire_placed_revival_trigger(
                        placed_identity.map(|identity| (self.command_tx.clone(), identity)),
                        error,
                    )
                    .await;
                }
                result
            }
            SubmitWorkDispatchCompletion::AwaitTurnCompletion { .. } => {
                Err(MobError::Internal(format!(
                    "turn-driven spawn kickoff for '{agent_identity}' resolved to a tracked completion delivery"
                )))
            }
        }
    }

    /// Turn-driven drain start. Unlike the wiring lane this is best-effort on
    /// capability: a member without a comms runtime simply has no drain.
    async fn optional_peer_ingress(
        &self,
        agent_identity: &AgentIdentity,
        member_ref: &MemberRef,
    ) -> Result<(), MobError> {
        #[cfg(all(not(target_arch = "wasm32"), feature = "runtime-adapter"))]
        {
            let (Some(adapter), Some(bridge_session_id)) =
                (self.runtime_adapter.clone(), member_ref.bridge_session_id())
            else {
                return Ok(());
            };
            let comms_runtime = self.provisioner.comms_runtime(member_ref).await;
            if std::env::var_os("RKAT_TRACE_COMMS_DRAIN_BIND").is_some()
                && let Some(runtime) = comms_runtime.as_ref()
            {
                tracing::info!(
                    agent_identity = %agent_identity,
                    session_id = %bridge_session_id,
                    comms_ptr = ?Arc::as_ptr(runtime),
                    "mob turn-driven spawn binding comms drain"
                );
            }
            // W2-G: route through the mob-owned spawn seam so peer-ingress
            // ownership transitions to `MobOwned { comms_runtime_id, mob_id }`.
            if let Some(comms_runtime) = comms_runtime {
                let mob_id =
                    meerkat_runtime::meerkat_machine::dsl::MobId::from(self.mob_id.as_ref());
                adapter
                    .maybe_spawn_mob_comms_drain(bridge_session_id, comms_runtime, mob_id)
                    .await
                    .map_err(|err| {
                        MobError::Internal(format!(
                            "mob comms drain spawn failed for session {bridge_session_id}: {err}"
                        ))
                    })?;
            }
        }
        #[cfg(any(target_arch = "wasm32", not(feature = "runtime-adapter")))]
        {
            let _ = (agent_identity, member_ref);
        }
        Ok(())
    }

    /// Admit the autonomous kickoff prompt and own its completion wait.
    ///
    /// The completion-wait task is the same one the inline path spawned; it
    /// re-enters the actor with `KickoffOutcomeResolved`, which is exactly why
    /// the admission must not be awaited by the actor itself.
    #[cfg(feature = "runtime-adapter")]
    async fn admit_autonomous_kickoff(
        &self,
        agent_identity: &AgentIdentity,
        bridge_session_id: SessionId,
        input: meerkat_runtime::Input,
    ) -> Result<(), MobError> {
        let adapter = self.runtime_adapter.clone().ok_or_else(|| {
            MobError::Internal(format!(
                "autonomous member '{agent_identity}' requires admission-capable substrate (runtime adapter)"
            ))
        })?;
        let (_outcome, completion_handle) = adapter
            .accept_input_with_completion(&bridge_session_id, input)
            .await
            .map_err(|e| {
                MobError::Internal(format!(
                    "autonomous prompt admission failed for '{agent_identity}': {e}"
                ))
            })?;
        let log_id = agent_identity.clone();
        let completion_command_tx = self.command_tx.clone();
        let handle = tokio::spawn(async move {
            if let Some(h) = completion_handle {
                let outcome = h.wait().await;
                let (ack_tx, ack_rx) = oneshot::channel();
                if completion_command_tx
                    .send(RoutedMobCommand::internal(
                        MobCommand::KickoffOutcomeResolved {
                            agent_identity: log_id.clone(),
                            outcome,
                            ack_tx,
                        },
                    ))
                    .await
                    .is_err()
                {
                    tracing::warn!(
                        agent_identity = %log_id,
                        "mob actor dropped before kickoff outcome could be recorded"
                    );
                } else {
                    let _ = ack_rx.await;
                }
            }
        });
        self.autonomous_initial_turns
            .lock()
            .await
            .insert(agent_identity.clone(), InitialTurnHandle { handle });
        tracing::debug!(agent_identity = %agent_identity, "autonomous member started");
        Ok(())
    }
}

/// Typed continuation for one activation's terminal outcome.
///
/// Every caller hands the pipeline one of these instead of awaiting a future
/// that would need the same actor command queue to make progress.
pub(in crate::runtime) enum SpawnActivationRoute {
    /// Spawn-command / pending-batch / respawn / policy caller: the
    /// settlement replies with the member receipt and preserves the batch's
    /// observability, identity-reconcile, and respawn-topology semantics.
    ///
    /// This is the only route. There is deliberately no inline variant: an
    /// activation that resolved on the actor task would have to await the
    /// same command queue its own stages settle through.
    Receipt(Box<SpawnReceiptRoute>),
}

/// Reply custody + terminal bookkeeping for one receipt-routed activation.
pub(in crate::runtime) struct SpawnReceiptRoute {
    pub(in crate::runtime) agent_identity: AgentIdentity,
    /// Pending-spawn ticket for per-spawn outcome observability.
    pub(in crate::runtime) spawn_ticket: Option<u64>,
    /// Enqueue instant for the `spawn built` / `spawn failed` latency fact.
    pub(in crate::runtime) enqueued_at: Option<Instant>,
    /// Exact old incarnation whose durable respawn-topology hold this spawn
    /// must abandon on failure.
    pub(in crate::runtime) respawn_origin: Option<RespawnOrigin>,
    /// Identity-reconcile actuation authority whose disposition is recorded
    /// before the reply is delivered.
    pub(in crate::runtime) identity_reconcile: Option<IdentityReconcileCompletionAuthority>,
    /// Whether the receipt's `failed_restore_peer_ids` must be classified by
    /// the generated `ResolveRespawnTopologyRestore` authority.
    pub(in crate::runtime) classify_respawn_topology: bool,
    /// Stage label used by the spawn-command failure warning.
    pub(in crate::runtime) failure_warning_stage: Option<&'static str>,
    /// Spawn source label carried by that same warning.
    pub(in crate::runtime) spawn_source: Option<&'static str>,
    pub(in crate::runtime) reply_tx:
        oneshot::Sender<Result<super::handle::MemberSpawnReceipt, MobError>>,
}

/// Actor-side phase cursor. Each variant names the next step the ACTOR runs;
/// opaque work between two phases is owned by the worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SpawnActivationPhase {
    /// Kickoff-intent validation + trusted-peer publication dispatch.
    MembershipPublish,
    /// Machine-owned membership facts, roster projection, kickoff pending.
    MembershipCommit,
    /// Wiring fan-out planning and peer-ingress decision.
    WiringPlan,
    /// Wire fan-out + respawn topology restore.
    WiringRealize,
    /// Kickoff lane selection.
    Kickoff,
    /// Local autonomous: startup readiness committed, kickoff admission next.
    AutonomousStart,
    /// Turn-driven initial turn admission.
    InitialTurn,
    /// `CommitSpawnActivation` + receipt.
    Commit,
}

/// One in-flight activation. Reply custody and the phase cursor are
/// process-local scheduling state; every canonical fact stays in MobMachine.
pub(in crate::runtime) struct PendingSpawnActivation {
    ticket: SpawnActivationTicket,
    custody: SpawnActivationCustody,
    phase: SpawnActivationPhase,
    state: Box<SpawnActivateState>,
    route: SpawnActivationRoute,
    /// Stage currently owned by the worker, if any.
    inflight: Option<SpawnActivationStage>,
    /// Set while a graph-scoped lifecycle control owns the mob. A parked
    /// activation is NOT driven, NOT settled, and NOT rolled back: it keeps
    /// full custody until the control releases it.
    parked: Option<SpawnActivationPark>,
    /// A worker outcome that arrived while parked. It is retained verbatim —
    /// committing it could realize wiring or, on failure, unwind through
    /// `rollback_failed_spawn` (retire + unwire), which is exactly the
    /// destructive graph mutation the control is serializing against.
    deferred_outcome: Option<Box<SpawnActivationStageOutcome>>,
}

/// Why one activation is parked, and since when (observability only; the
/// canonical control state belongs to the machine that owns the pause).
#[derive(Debug, Clone, Copy)]
pub(in crate::runtime) struct SpawnActivationPark {
    reason: SpawnActivationParkReason,
    since: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::runtime) enum SpawnActivationParkReason {
    /// An explicit-resume topology mutation owns the wiring graph.
    ResumeTopologyPending,
}

impl SpawnActivationParkReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::ResumeTopologyPending => "resume_topology_pending",
        }
    }
}

/// Exact spawn-activation custody census for a lifecycle gate.
///
/// `total == 0` is the only reading that authorizes a graph-scoped control to
/// begin: it means no activation holds custody at all — nothing in flight,
/// nothing parked, no retained outcome waiting to commit.
#[derive(Debug, Clone, Default)]
pub(in crate::runtime) struct SpawnActivationQuiescence {
    pub(in crate::runtime) total: usize,
    pub(in crate::runtime) parked: usize,
    pub(in crate::runtime) inflight_stages: usize,
    pub(in crate::runtime) deferred_outcomes: usize,
    pub(in crate::runtime) members: Vec<AgentIdentity>,
}

/// A compensation that could not be completed by the failing owner.
///
/// Retained (never dropped, never folded into success) until an owner settles
/// it, so a failed rollback cannot masquerade as a clean spawn failure. A
/// non-empty retained set blocks graph-scoped lifecycle controls, so this
/// lane owns a real PHYSICAL retry — the set drains by re-running the
/// compensation or by observing the exact incarnation gone, never by
/// declaring victory.
pub(in crate::runtime) struct RetainedSpawnActivationCleanup {
    pub(in crate::runtime) custody: SpawnActivationCustody,
    pub(in crate::runtime) reason: String,
    kind: RetainedSpawnCleanupKind,
    attempts: u32,
    next_attempt_at: Instant,
    /// True while this entry's physical retry is executing. The entry stays
    /// in the set for the whole attempt so a lifecycle gate reading the set
    /// can never observe a false "drained" window mid-retry.
    in_flight: bool,
    rollback_owner: Option<super::retirement_io::RetirementContinuation>,
    reply: Option<(SpawnActivationRoute, MobError)>,
    retry_replies: Vec<oneshot::Sender<Result<(), MobError>>>,
}

#[derive(Debug, Clone)]
enum RetainedSpawnCleanupKind {
    /// A worker stage completed for an activation that no longer held
    /// custody, so a live effect exists with no owner. The ONLY thing that
    /// settles it is a resource-owner compensation receipt for the exact
    /// `(member_ref, operation_id)` the stage touched — never a projection
    /// predicate. `provisioner.abort_member_provision` is that compensation,
    /// and it targets the predecessor's exact resource, so it can never
    /// disturb a successor that reused the identity.
    ProvisionAbort,
    /// The spawn's own rollback (machine retire + unwire + archive) failed.
    /// Its compensation is actor-bound machine authority owned by the
    /// retirement lane, so this lane retains it fail-closed and hands it over
    /// by explicit claim. It is never "retried" inline and never expires.
    SpawnRollback(Box<RetainedSpawnRollback>),
}

impl RetainedSpawnCleanupKind {
    fn as_str(&self) -> &'static str {
        match self {
            Self::ProvisionAbort => "provision_abort",
            Self::SpawnRollback(_) => "spawn_rollback",
        }
    }
}

/// Owned, claimable form of the failed rollback for the actor-bound owner.
#[derive(Debug, Clone)]
pub(in crate::runtime) struct FailedSpawnRollbackMaterial {
    pub(in crate::runtime) generation: crate::ids::Generation,
    pub(in crate::runtime) profile_name: ProfileName,
    pub(in crate::runtime) member_ref: MemberRef,
    pub(in crate::runtime) operation_id: meerkat_core::ops::OperationId,
    pub(in crate::runtime) session_origin: super::provisioner::ProvisionSessionOrigin,
    pub(in crate::runtime) successful_wiring_targets: Vec<AgentIdentity>,
    pub(in crate::runtime) planned_wiring_targets: Vec<AgentIdentity>,
}

impl From<&RetainedSpawnRollback> for FailedSpawnRollbackMaterial {
    fn from(value: &RetainedSpawnRollback) -> Self {
        Self {
            generation: value.generation,
            profile_name: value.profile_name.clone(),
            member_ref: value.member_ref.clone(),
            operation_id: value.operation_id.clone(),
            session_origin: value.session_origin,
            successful_wiring_targets: value.successful_wiring_targets.clone(),
            planned_wiring_targets: value.planned_wiring_targets.clone(),
        }
    }
}

/// Owned form of [`FailedSpawnRollback`] retained until an owner settles it.
#[derive(Debug, Clone)]
struct RetainedSpawnRollback {
    generation: crate::ids::Generation,
    profile_name: ProfileName,
    member_ref: MemberRef,
    operation_id: meerkat_core::ops::OperationId,
    session_origin: super::provisioner::ProvisionSessionOrigin,
    successful_wiring_targets: Vec<AgentIdentity>,
    planned_wiring_targets: Vec<AgentIdentity>,
}

/// Bounded retry schedule for a retained compensation. The last entry is the
/// steady-state cadence: a compensation that never succeeds keeps blocking
/// graph-scoped controls and keeps saying so, which is the honest outcome.
const RETAINED_SPAWN_CLEANUP_BACKOFF: [Duration; 5] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(5),
    Duration::from_secs(15),
    Duration::from_secs(60),
];

fn retained_spawn_cleanup_backoff(attempts: u32) -> Duration {
    let index = (attempts as usize).min(RETAINED_SPAWN_CLEANUP_BACKOFF.len() - 1);
    RETAINED_SPAWN_CLEANUP_BACKOFF[index]
}

/// Off-actor observer for one identity's spawn-cleanup settlement.
///
/// Await it from a detached lane; it resolves only when the actor has
/// observed every spawn-cleanup obligation for that identity as clear.
/// `Err` means the actor is gone before settlement, which is UNKNOWN, not
/// success — callers must not treat it as a clean cleanup.
pub(in crate::runtime) struct SpawnCleanupObserver {
    rx: oneshot::Receiver<()>,
}

impl SpawnCleanupObserver {
    pub(in crate::runtime) async fn settled(self) -> Result<(), MobError> {
        self.rx.await.map_err(|_| {
            MobError::Internal(
                "spawn cleanup observer lost the actor before settlement could be observed"
                    .to_string(),
            )
        })
    }
}

/// What the actor-side driver decided for one phase.
enum SpawnActivationStep {
    /// Hand one stage to the worker and return to the command loop.
    Dispatch(SpawnActivationStage, Box<SpawnActivationWork>),
    /// Keep driving on the actor.
    Continue,
    /// Terminal.
    Done(Box<Result<FinalizeSpawnOutcome, MobError>>),
}

impl MobActor {
    /// Exact spawn-activation custody census.
    ///
    /// A graph-scoped lifecycle control (explicit-resume topology today) may
    /// begin only when `total == 0`. Read it in the SAME actor step that
    /// applies the control's admission input: both run on the loop, so a
    /// zero reading can only be invalidated by an `await` placed between the
    /// read and the apply.
    pub(in crate::runtime) fn spawn_activation_quiescence(&self) -> SpawnActivationQuiescence {
        let mut census = SpawnActivationQuiescence {
            total: self.pending_spawn_activations.len(),
            ..SpawnActivationQuiescence::default()
        };
        for pending in self.pending_spawn_activations.values() {
            if pending.parked.is_some() {
                census.parked += 1;
            }
            if pending.inflight.is_some() {
                census.inflight_stages += 1;
            }
            if pending.deferred_outcome.is_some() {
                census.deferred_outcomes += 1;
            }
            census.members.push(pending.custody.agent_identity.clone());
        }
        census
    }

    /// `true` iff no spawn activation holds custody. This is the predicate a
    /// graph-scoped control gates its admission on.
    pub(in crate::runtime) fn spawn_activations_quiesced(&self) -> bool {
        self.pending_spawn_activations.is_empty()
    }

    /// Whether new activation work must park instead of running.
    ///
    /// Activation commits realize wiring, and its failure unwind runs
    /// `rollback_failed_spawn` (retire + unwire) directly — both are graph
    /// mutations that must not interleave with an owning control. Parking is
    /// the ONLY correct answer here: rejecting would fabricate terminality
    /// for an already-admitted member, and rolling back would destroy a
    /// member the control never authorized destroying.
    pub(in crate::runtime) fn spawn_activation_pause_is_active(&self) -> bool {
        #[cfg(feature = "runtime-adapter")]
        {
            self.resume_topology_mutation_pending()
        }
        #[cfg(not(feature = "runtime-adapter"))]
        {
            false
        }
    }

    fn park_spawn_activation(
        &self,
        pending: &mut PendingSpawnActivation,
        reason: SpawnActivationParkReason,
        at: &'static str,
    ) {
        if pending.parked.is_none() {
            pending.parked = Some(SpawnActivationPark {
                reason,
                since: Instant::now(),
            });
        }
        let census = self.spawn_activation_quiescence();
        tracing::debug!(
            mob_id = %self.definition.id,
            agent_identity = %pending.custody.agent_identity,
            ticket = %pending.ticket,
            reason = reason.as_str(),
            at,
            custody_total = census.total,
            custody_parked = census.parked,
            custody_inflight_stages = census.inflight_stages,
            custody_deferred_outcomes = census.deferred_outcomes,
            custody_members = ?census.members,
            "parking spawn activation while a graph-scoped control owns the mob"
        );
    }

    /// Release every parked activation once the control has settled.
    ///
    /// Resumption is ticket-ordered and re-enters the ordinary driver: a
    /// retained outcome is committed first (custody is re-verified at that
    /// commit, not at park time), then the phase machine continues.
    pub(in crate::runtime) async fn resume_parked_spawn_activations(&mut self) {
        if self.spawn_activation_pause_is_active() {
            return;
        }
        let parked: Vec<SpawnActivationTicket> = self
            .pending_spawn_activations
            .iter()
            .filter(|(_, pending)| pending.parked.is_some())
            .map(|(ticket, _)| *ticket)
            .collect();
        for ticket in parked {
            let released = {
                let Some(pending) = self.pending_spawn_activations.get_mut(&ticket) else {
                    tracing::debug!(
                        mob_id = %self.definition.id,
                        %ticket,
                        "parked activation settled before its release could run"
                    );
                    continue;
                };
                let Some(park) = pending.parked.take() else {
                    continue;
                };
                (
                    park,
                    pending.deferred_outcome.take(),
                    pending.custody.agent_identity.clone(),
                )
            };
            let (park, deferred_outcome, agent_identity) = released;
            tracing::debug!(
                mob_id = %self.definition.id,
                agent_identity = %agent_identity,
                %ticket,
                reason = park.reason.as_str(),
                parked_ms = park.since.elapsed().as_millis() as u64,
                deferred_stage = deferred_outcome
                    .as_ref()
                    .map(|outcome| outcome.stage.as_str())
                    .unwrap_or("none"),
                "resuming parked spawn activation"
            );
            match deferred_outcome {
                Some(outcome) => {
                    Box::pin(self.commit_settled_spawn_activation_stage(ticket, *outcome)).await;
                }
                None => Box::pin(self.drive_spawn_activation(ticket)).await,
            }
        }
    }

    /// Whether every spawn-cleanup obligation for one identity is settled.
    ///
    /// Covers all three shell-side custody sets at once, because a caller
    /// that observes only one of them can be told "settled" while another
    /// still owns physical cleanup:
    /// * `pending_spawn_activations` — an activation still holds custody,
    /// * `retained_spawn_activation_cleanups` — an activation compensation is
    ///   still unsettled (this lane's physical retry owner),
    /// * `pending_spawn_cleanup_anchors` — a pending-slot cancellation's
    ///   physical cleanup was retained for re-drain.
    pub(in crate::runtime) fn spawn_cleanup_settled_for(&self, identity: &AgentIdentity) -> bool {
        !self.retirement_has_pending_spawn_cleanup(identity)
            && !self
                .pending_spawn_activations
                .values()
                .any(|pending| pending.custody.agent_identity == *identity)
            && !self
                .retained_spawn_activation_cleanups
                .iter()
                .any(|retained| retained.custody.agent_identity == *identity)
            && !self
                .pending_spawn_cleanup_anchors
                .values()
                .any(|anchor| anchor.agent_identity == *identity)
    }

    /// Hand a caller an OFF-ACTOR observer for one identity's spawn-cleanup
    /// settlement.
    ///
    /// This is the API a detached lifecycle lane should await instead of
    /// running cleanup awaits on the loop: the physical work stays owned by
    /// the shell's cleanup owners (this lane's retained-compensation retry
    /// and the pending-spawn cleanup anchors), and the observer only reports
    /// when their state is actually clear. It is resolved on the actor, so it
    /// never fabricates completion; if the actor dies first the receiver
    /// errors and the caller must treat the outcome as unknown.
    pub(in crate::runtime) fn observe_spawn_cleanup_settled(
        &mut self,
        identity: &AgentIdentity,
    ) -> SpawnCleanupObserver {
        let (tx, rx) = oneshot::channel();
        if self.spawn_cleanup_settled_for(identity) {
            let _ = tx.send(());
        } else {
            self.spawn_cleanup_waiters.push((identity.clone(), tx));
        }
        SpawnCleanupObserver { rx }
    }

    /// Resolve every waiter whose identity is now fully settled. Called from
    /// this lane's tick and from each of its settlement points.
    pub(in crate::runtime) fn notify_spawn_cleanup_waiters(&mut self) {
        if self.spawn_cleanup_waiters.is_empty() {
            return;
        }
        let mut still_waiting = Vec::with_capacity(self.spawn_cleanup_waiters.len());
        for (identity, tx) in std::mem::take(&mut self.spawn_cleanup_waiters) {
            if tx.is_closed() {
                continue;
            }
            if self.spawn_cleanup_settled_for(&identity) {
                let _ = tx.send(());
            } else {
                still_waiting.push((identity, tx));
            }
        }
        self.spawn_cleanup_waiters = still_waiting;
    }

    /// One actor-wake tick for this lane: release parked activations, then
    /// make ONE bounded physical attempt at a retained compensation.
    ///
    /// A graph-scoped lifecycle control is gated on both maps being empty, so
    /// something must actually drain them. This is that owner. Call it from
    /// an existing wake path; it is a no-op when there is nothing to do, and
    /// it never runs while a control owns the mob.
    /// One actor-wake tick for this lane. Bounded and non-blocking: it
    /// resumes parked activations, DISPATCHES (never awaits) at most one
    /// retained compensation, and resolves settled observers. No opaque I/O
    /// runs on the loop, so a lifecycle lane can call it from any wake path.
    pub(in crate::runtime) async fn tick_spawn_activation_custody(&mut self) {
        Box::pin(self.maybe_resume_parked_spawn_activations()).await;
        self.dispatch_retained_spawn_cleanup().await;
        self.notify_spawn_cleanup_waiters();
    }

    /// Dispatch at most one retained compensation to the worker.
    ///
    /// The actor performs NO opaque I/O here: it selects a due obligation,
    /// marks it in flight (so the entry stays visible to every lifecycle gate
    /// until a real ack arrives) and hands the exact resource identity to the
    /// worker. `provisioner.abort_member_provision(member_ref, operation_id,
    /// reason)` is the resource owner's compensation and its typed result is
    /// the receipt; nothing else settles the obligation.
    ///
    /// Rollbacks hand their exact retained continuation to the existing
    /// retirement lane; provision aborts use the resource-owner callback.
    async fn dispatch_retained_spawn_cleanup(&mut self) {
        if self.retained_spawn_activation_cleanups.is_empty() {
            return;
        }
        if self.durable_uncertainty_fail_stop {
            // Cold recovery owns every durable anchor; a live compensation
            // here could certify cleanup this incarnation cannot prove.
            return;
        }
        if self.spawn_activation_pause_is_active() {
            // Aborting a member provision mutates member-owned resources a
            // graph-scoped control is serializing against.
            return;
        }
        let now = Instant::now();
        let Some(index) = self
            .retained_spawn_activation_cleanups
            .iter()
            .position(|retained| !retained.in_flight && retained.next_attempt_at <= now)
        else {
            return;
        };
        if let RetainedSpawnCleanupKind::SpawnRollback(rollback) =
            &self.retained_spawn_activation_cleanups[index].kind
        {
            let material = FailedSpawnRollbackMaterial::from(rollback.as_ref());
            let identity = self.retained_spawn_activation_cleanups[index]
                .custody
                .agent_identity
                .clone();
            let result = self
                .rollback_failed_spawn(
                    &identity,
                    FailedSpawnRollback {
                        generation: material.generation,
                        profile_name: &material.profile_name,
                        member_ref: &material.member_ref,
                        operation_id: &material.operation_id,
                        session_origin: material.session_origin,
                        successful_wiring_targets: &material.successful_wiring_targets,
                        planned_wiring_targets: &material.planned_wiring_targets,
                    },
                )
                .await;
            if let Err(error) = result {
                let custody = self.retained_spawn_activation_cleanups[index]
                    .custody
                    .clone();
                self.release_retained_spawn_rollback(&custody, error.to_string());
            }
            return;
        }
        let (custody, reason) = {
            let entry = &mut self.retained_spawn_activation_cleanups[index];
            entry.in_flight = true;
            (entry.custody.clone(), entry.reason.clone())
        };
        let provisioner = Arc::clone(&self.provisioner);
        let command_tx = self.command_tx.clone();
        let mob_id = self.definition.id.clone();
        self.actor_io_tasks.spawn(async move {
            let result = provisioner
                .abort_member_provision(&custody.member_ref, &custody.operation_id, &reason)
                .await;
            if command_tx
                .send(RoutedMobCommand::internal(MobCommand::SpawnCleanupSettled {
                    custody: Box::new(custody),
                    result,
                }))
                .await
                .is_err()
            {
                tracing::warn!(
                    %mob_id,
                    "retained spawn cleanup completed after the actor stopped; the obligation is unproven"
                );
            }
        });
    }

    /// Absorb one compensation ack. Only a typed success settles.
    pub(super) fn spawn_cleanup_settled(
        &mut self,
        custody: Box<SpawnActivationCustody>,
        result: Result<(), MobError>,
    ) {
        if self.retained_spawn_activation_cleanups.iter().any(|entry| {
            entry.custody == *custody
                && matches!(entry.kind, RetainedSpawnCleanupKind::SpawnRollback(_))
        }) {
            self.release_retained_spawn_rollback(
                &custody,
                "provision abort returned; the upgraded retirement rollback still needs its own settlement"
                    .to_string(),
            );
            return;
        }
        match result {
            Ok(()) => {
                self.settle_retained_spawn_cleanup(
                    &custody,
                    "the resource owner aborted the exact member provision for this operation",
                );
            }
            Err(error) => self.reschedule_retained_spawn_cleanup(&custody, error.to_string()),
        }
    }

    /// Staged replacement for the inline pending-spawn anchor cleanup.
    ///
    /// Same owner, same map, same compensation — but the provisioner abort is
    /// dispatched instead of awaited, and the anchor stays visible in
    /// `pending_spawn_cleanup_anchors` until its typed receipt lands. Callers
    /// must NOT treat the return as cleanup: it is an observer, and a retire
    /// that reports terminality before awaiting it would be a false finish.
    ///
    /// This is the seam that removes the last inline provisioner await from
    /// the pending-slot cancellation path; the retirement lane's port swaps
    /// `abort_pending_spawn_slot(..).await` for this call plus an off-actor
    /// `observer.settled().await` before it replies.
    pub(in crate::runtime) fn stage_pending_spawn_anchor_cleanup(
        &mut self,
        anchor: PendingSpawnCleanupAnchor,
    ) -> SpawnCleanupObserver {
        let identity = anchor.agent_identity.clone();
        let spawn_ticket = anchor.spawn_ticket;
        let member_ref = MemberRef::from_bridge_session_id(anchor.session_id.clone());
        let operation_id = anchor.operation_id.clone();
        let reason = anchor.reason.clone();
        // Visible before the attempt starts: no gate may observe a drained
        // window while the compensation is in flight.
        self.pending_spawn_cleanup_anchors
            .insert(spawn_ticket, anchor);
        let observer = self.observe_spawn_cleanup_settled(&identity);
        let provisioner = Arc::clone(&self.provisioner);
        let command_tx = self.command_tx.clone();
        let mob_id = self.definition.id.clone();
        self.actor_io_tasks.spawn(async move {
            let result = provisioner
                .abort_member_provision(&member_ref, &operation_id, &reason)
                .await;
            if command_tx
                .send(RoutedMobCommand::internal(
                    MobCommand::PendingSpawnAnchorSettled {
                        spawn_ticket,
                        result,
                    },
                ))
                .await
                .is_err()
            {
                tracing::warn!(
                    %mob_id,
                    spawn_ticket,
                    "pending-spawn anchor cleanup completed after the actor stopped; the anchor is unproven"
                );
            }
        });
        observer
    }

    /// Absorb one pending-spawn anchor compensation ack. Only a typed success
    /// removes the anchor; a failure leaves it retained for re-drain.
    pub(super) fn pending_spawn_anchor_settled(
        &mut self,
        spawn_ticket: u64,
        result: Result<(), MobError>,
    ) {
        match result {
            Ok(()) => {
                if let Some(anchor) = self.pending_spawn_cleanup_anchors.remove(&spawn_ticket) {
                    tracing::info!(
                        mob_id = %self.definition.id,
                        spawn_ticket,
                        agent_identity = %anchor.agent_identity,
                        operation_id = %anchor.operation_id,
                        "pending-spawn cleanup anchor settled by an exact provision abort"
                    );
                }
                self.notify_spawn_cleanup_waiters();
            }
            Err(error) => {
                tracing::warn!(
                    mob_id = %self.definition.id,
                    spawn_ticket,
                    error = %error,
                    "pending-spawn cleanup anchor remains retained after a failed compensation"
                );
            }
        }
    }

    pub(super) async fn request_spawn_rollback_retry(
        &mut self,
        identity: &AgentIdentity,
        reply: oneshot::Sender<Result<(), MobError>>,
    ) -> Result<(), oneshot::Sender<Result<(), MobError>>> {
        let Some(entry) = self
            .retained_spawn_activation_cleanups
            .iter_mut()
            .find(|entry| {
                entry.custody.agent_identity == *identity
                    && matches!(entry.kind, RetainedSpawnCleanupKind::SpawnRollback(_))
            })
        else {
            return Err(reply);
        };
        entry.retry_replies.push(reply);
        if entry.in_flight {
            return Ok(());
        }
        let Some((custody, material, owner)) = self.claim_retained_spawn_rollback(identity) else {
            return Ok(());
        };
        if let Err(error) = self
            .start_spawn_rollback(custody.clone(), material, owner)
            .await
        {
            self.finish_spawn_rollback_attempt(&custody, None, Err(error))
                .await;
        }
        Ok(())
    }

    /// Hand one retained `SpawnRollback` obligation to the owner that can
    /// actually execute it (machine retire + unwire + archive is actor-bound
    /// authority this lane does not own).
    ///
    /// The obligation STAYS retained: the claim only marks it in flight, so
    /// every lifecycle gate keeps seeing it until the claimer settles it with
    /// a physical completion. There is no expiry or projection-based clear.
    pub(in crate::runtime) fn claim_retained_spawn_rollback(
        &mut self,
        identity: &AgentIdentity,
    ) -> Option<(
        SpawnActivationCustody,
        FailedSpawnRollbackMaterial,
        Option<super::retirement_io::RetirementContinuation>,
    )> {
        let entry = self
            .retained_spawn_activation_cleanups
            .iter_mut()
            .find(|retained| {
                !retained.in_flight
                    && retained.custody.agent_identity == *identity
                    && matches!(retained.kind, RetainedSpawnCleanupKind::SpawnRollback(_))
            })?;
        entry.in_flight = true;
        let custody = entry.custody.clone();
        let RetainedSpawnCleanupKind::SpawnRollback(rollback) = &entry.kind else {
            return None;
        };
        Some((
            custody,
            FailedSpawnRollbackMaterial::from(rollback.as_ref()),
            entry.rollback_owner.take(),
        ))
    }

    pub(super) async fn finish_spawn_rollback_attempt(
        &mut self,
        custody: &SpawnActivationCustody,
        owner: Option<super::retirement_io::RetirementContinuation>,
        result: Result<(), MobError>,
    ) {
        let Some(entry) = self
            .retained_spawn_activation_cleanups
            .iter_mut()
            .find(|entry| entry.custody == *custody)
        else {
            self.durable_uncertainty_fail_stop = true;
            return;
        };
        entry.rollback_owner = owner;
        let reply = entry.reply.take();
        let retry_replies = std::mem::take(&mut entry.retry_replies);
        let error_detail = result.as_ref().err().map(ToString::to_string);
        match result {
            Ok(()) => self.settle_retained_spawn_cleanup(
                custody,
                "retirement owner settled exact spawn rollback and its physical cleanup",
            ),
            Err(error) => self.release_retained_spawn_rollback(custody, error.to_string()),
        }
        for reply in retry_replies {
            let result = match error_detail.as_ref() {
                Some(detail) => Err(MobError::Internal(format!(
                    "spawn rollback remains owned for retry: {detail}"
                ))),
                None => Ok(()),
            };
            let _ = reply.send(result);
        }
        if let Some((route, original)) = reply {
            let error = match error_detail {
                Some(detail) => MobError::Internal(format!(
                    "{original}; rollback remains owned for retry: {detail}"
                )),
                None => original,
            };
            Box::pin(self.settle_spawn_activation_route(route, Err(error))).await;
        }
    }

    /// Release a claimed `SpawnRollback` obligation that the claimer could not
    /// settle, so it becomes claimable again instead of silently stuck.
    pub(in crate::runtime) fn release_retained_spawn_rollback(
        &mut self,
        custody: &SpawnActivationCustody,
        error: String,
    ) {
        self.reschedule_retained_spawn_cleanup(custody, error);
    }

    /// Remove a retained compensation because its goal is now EVIDENCED.
    fn settle_retained_spawn_cleanup(&mut self, custody: &SpawnActivationCustody, evidence: &str) {
        let Some(index) = self
            .retained_spawn_activation_cleanups
            .iter()
            .position(|retained| retained.custody == *custody)
        else {
            return;
        };
        let settled = self.retained_spawn_activation_cleanups.remove(index);
        self.notify_spawn_cleanup_waiters();
        tracing::info!(
            mob_id = %self.definition.id,
            agent_identity = %custody.agent_identity,
            generation = custody.generation.get(),
            attempts = settled.attempts,
            original_reason = %settled.reason,
            evidence,
            "retained spawn activation cleanup settled"
        );
    }

    /// Keep a retained compensation and back its next attempt off.
    fn reschedule_retained_spawn_cleanup(
        &mut self,
        custody: &SpawnActivationCustody,
        error: String,
    ) {
        let mob_id = self.definition.id.clone();
        let Some(entry) = self
            .retained_spawn_activation_cleanups
            .iter_mut()
            .find(|retained| retained.custody == *custody)
        else {
            return;
        };
        entry.in_flight = false;
        entry.attempts = entry.attempts.saturating_add(1);
        let backoff = retained_spawn_cleanup_backoff(entry.attempts);
        entry.next_attempt_at = Instant::now() + backoff;
        tracing::warn!(
            %mob_id,
            agent_identity = %custody.agent_identity,
            generation = custody.generation.get(),
            attempts = entry.attempts,
            retry_in_ms = backoff.as_millis() as u64,
            error = %error,
            "retained spawn activation cleanup is still unsettled; graph-scoped controls stay blocked"
        );
    }

    /// Cheap tick: release parked activations when no control owns the mob.
    ///
    /// Safe to call from any actor step (it is a no-op unless something is
    /// actually parked and the pause is clear). The activation lane calls it
    /// from its own entry points so a missed control-side call can never
    /// strand custody forever; a control lane may also call it directly the
    /// moment it settles.
    pub(in crate::runtime) async fn maybe_resume_parked_spawn_activations(&mut self) {
        if self.spawn_activations_quiesced() || self.spawn_activation_pause_is_active() {
            return;
        }
        if self
            .pending_spawn_activations
            .values()
            .any(|pending| pending.parked.is_some())
        {
            Box::pin(self.resume_parked_spawn_activations()).await;
        }
    }

    /// Common staged-work gate.
    ///
    /// MEMBER-CONSTRUCTION scope only: fail-stop, explicit-resume member
    /// work, and member-operation eligibility. It deliberately does NOT
    /// answer graph-scoped questions — a wiring-graph control is handled by
    /// [`Self::spawn_activation_pause_is_active`] with a PARK, never a
    /// refusal, because the member is already admitted by then.
    pub(super) fn member_staged_work_gate(
        &self,
        agent_identity: &AgentIdentity,
        intent: &str,
    ) -> Result<(), MobError> {
        if self.durable_uncertainty_fail_stop {
            return Err(MobError::Internal(format!(
                "{intent} for '{agent_identity}' refused: the actor is fail-stopped for cold recovery"
            )));
        }
        let dsl_identity = mob_dsl::AgentIdentity::from_domain(agent_identity);
        if self
            .dsl_authority
            .state()
            .explicit_resume_member_work
            .contains_key(&dsl_identity)
        {
            return Err(MobError::LifecycleOperationPending {
                intent: format!("explicit_resume member {agent_identity}"),
            });
        }
        self.require_member_operation_eligible()
    }

    /// Verify that the live roster still holds the exact incarnation — and
    /// the exact member ref — a staged step was dispatched for.
    async fn spawn_activation_custody_state(
        &self,
        custody: &SpawnActivationCustody,
        before_roster_projection: bool,
    ) -> SpawnActivationCustodyState {
        let entry = {
            let roster = self.roster.read().await;
            roster.get(&custody.agent_identity).cloned()
        };
        if let Some(entry) = entry {
            let exact = entry.generation == custody.generation
                && entry.fence_token == custody.fence_token
                && entry.agent_runtime_id == custody.agent_runtime_id
                && entry.member_ref == custody.roster_comparable_member_ref();
            return if exact {
                SpawnActivationCustodyState::Current
            } else {
                SpawnActivationCustodyState::Superseded
            };
        }
        let dsl_runtime_id = mob_dsl::AgentRuntimeId::from_domain(&custody.agent_runtime_id);
        let identity = mob_dsl::AgentIdentity::from_domain(&custody.agent_identity);
        let state = self.dsl_authority.state();
        // Trusted-peer publication precedes the first roster projection.
        // Only that stage may use the already-committed machine binding.
        if before_roster_projection
            && state.spawn_exec_phase.get(&identity)
                == Some(&mob_dsl::SpawnExecPhase::MembershipCommitted)
            && state.identity_to_runtime.get(&identity) == Some(&dsl_runtime_id)
            && state.identity_runtime_generations.get(&identity)
                == Some(&mob_dsl::Generation::from_domain(custody.generation))
            && state.identity_runtime_fence_tokens.get(&identity)
                == Some(&mob_dsl::FenceToken::from_domain(custody.fence_token))
            && state.member_session_bindings.get(&identity)
                == custody
                    .member_ref
                    .bridge_session_id()
                    .map(mob_dsl::SessionId::from_domain)
                    .as_ref()
            && state.member_state_markers.get(&dsl_runtime_id)
                != Some(&mob_dsl::MobMemberState::Retiring)
            && state.live_runtime_ids.contains(&dsl_runtime_id)
        {
            return SpawnActivationCustodyState::Current;
        }
        if self
            .dsl_authority
            .state()
            .live_runtime_ids
            .contains(&dsl_runtime_id)
        {
            SpawnActivationCustodyState::Superseded
        } else {
            SpawnActivationCustodyState::Absent
        }
    }

    async fn spawn_activation_custody_is_current(&self, custody: &SpawnActivationCustody) -> bool {
        self.spawn_activation_custody_state(custody, false).await
            == SpawnActivationCustodyState::Current
    }

    fn spawn_activation_worker_context(&self) -> SpawnActivationWorkerContext {
        SpawnActivationWorkerContext {
            readiness: self.detached_member_readiness_context(),
            provisioner: Arc::clone(&self.provisioner),
            session_service: Arc::clone(&self.session_service),
            #[cfg(feature = "runtime-adapter")]
            runtime_adapter: self.runtime_adapter.clone(),
            #[cfg(feature = "runtime-adapter")]
            autonomous_initial_turns: Arc::clone(&self.autonomous_initial_turns),
            command_tx: self.command_tx.clone(),
            mob_id: self.definition.id.clone(),
        }
    }

    /// Retain a compensation that could not be completed, with the material
    /// its physical retry needs.
    fn retain_spawn_activation_cleanup(
        &mut self,
        custody: &SpawnActivationCustody,
        reason: String,
        kind: RetainedSpawnCleanupKind,
    ) {
        tracing::error!(
            mob_id = %self.definition.id,
            agent_identity = %custody.agent_identity,
            generation = custody.generation.get(),
            reason = %reason,
            compensation = kind.as_str(),
            "retaining unsettled spawn activation cleanup; graph-scoped controls stay blocked until it settles"
        );
        if let Some(existing) = self
            .retained_spawn_activation_cleanups
            .iter_mut()
            .find(|retained| retained.custody == *custody)
        {
            // Same incarnation, more evidence: upgrade an orphan-effect entry
            // to a rollback entry (it has the material to actually repair),
            // and keep the newest reason. Never duplicate custody.
            if matches!(kind, RetainedSpawnCleanupKind::SpawnRollback(_)) {
                existing.kind = kind;
            }
            existing.reason = reason;
            return;
        }
        self.retained_spawn_activation_cleanups
            .push(RetainedSpawnActivationCleanup {
                custody: custody.clone(),
                reason,
                kind,
                attempts: 0,
                next_attempt_at: Instant::now(),
                in_flight: false,
                rollback_owner: None,
                reply: None,
                retry_replies: Vec::new(),
            });
    }

    /// Start one activation.
    ///
    /// Returns as soon as the first opaque stage is handed to the worker (or
    /// as soon as the activation settles, when it needs no opaque stage at
    /// all). The terminal outcome reaches the caller through `route`.
    pub(super) async fn begin_spawn_activation(
        &mut self,
        ctx: Box<SpawnFinalizeCtx>,
        admitted: SpawnAdmitted,
        route: SpawnActivationRoute,
    ) {
        // A control may have settled since the last activation touched the
        // lane; release parked custody and make one retained-cleanup attempt
        // before adding new custody.
        Box::pin(self.tick_spawn_activation_custody()).await;
        let state = SpawnActivateState::admit(ctx, admitted);
        let custody = SpawnActivationCustody::from_state(&state);
        // NOTE: nothing may settle a route between here and the pause
        // decision below — a settlement replies and can emit durable
        // respawn-topology abandonment, which is exactly what a graph-scoped
        // control serializes against. The unsettled-compensation refusal
        // therefore lives in the first driven phase, not here. Ticket
        // exhaustion is the sole exception: without a ticket there is no
        // custody to park, and the refusal touches no member state.
        let ticket = match self.next_spawn_activation_ticket.next() {
            Ok(ticket) => ticket,
            Err(error) => {
                self.settle_spawn_activation_route(route, Err(error)).await;
                return;
            }
        };
        let mut pending = PendingSpawnActivation {
            ticket,
            custody,
            phase: SpawnActivationPhase::MembershipPublish,
            state: Box::new(state),
            route,
            inflight: None,
            parked: None,
            deferred_outcome: None,
        };
        // The membership commit already happened on the actor; this member
        // exists in MobMachine. If a graph-scoped control owns the mob, hold
        // the whole activation in custody rather than driving into wiring or
        // unwinding a member the control never authorized destroying.
        if self.spawn_activation_pause_is_active() {
            self.park_spawn_activation(
                &mut pending,
                SpawnActivationParkReason::ResumeTopologyPending,
                "begin_spawn_activation",
            );
            self.pending_spawn_activations.insert(ticket, pending);
            return;
        }
        self.pending_spawn_activations.insert(ticket, pending);
        Box::pin(self.drive_spawn_activation(ticket)).await;
    }

    /// Refuse to activate an incarnation whose own compensation was never
    /// completed. Retained cleanup is keyed to the EXACT incarnation, so a
    /// later generation of the same identity is never blocked by an older
    /// unsettled mess — and that older mess is never quietly forgotten.
    fn exact_spawn_activation_compensation_settled(
        &self,
        custody: &SpawnActivationCustody,
    ) -> Result<(), MobError> {
        let Some(retained) = self
            .retained_spawn_activation_cleanups
            .iter()
            .find(|retained| {
                retained.custody.agent_identity == custody.agent_identity
                    && retained.custody.generation == custody.generation
                    && retained.custody.fence_token == custody.fence_token
            })
        else {
            return Ok(());
        };
        Err(MobError::Internal(format!(
            "spawn activation for '{}' generation {} is blocked by an unsettled compensation: {}",
            custody.agent_identity,
            custody.generation.get(),
            retained.reason
        )))
    }

    /// Drive one registered activation until it dispatches a stage or settles.
    async fn drive_spawn_activation(&mut self, ticket: SpawnActivationTicket) {
        loop {
            let Some(mut pending) = self.pending_spawn_activations.remove(&ticket) else {
                return;
            };
            // Re-checked on every phase boundary: a control can take the mob
            // between two phases of the same activation.
            if self.spawn_activation_pause_is_active() {
                self.park_spawn_activation(
                    &mut pending,
                    SpawnActivationParkReason::ResumeTopologyPending,
                    "drive_spawn_activation",
                );
                self.pending_spawn_activations.insert(ticket, pending);
                return;
            }
            if pending.parked.is_some() {
                // Another path parked it while this drive was queued.
                self.pending_spawn_activations.insert(ticket, pending);
                return;
            }
            let step = Box::pin(self.advance_spawn_activation(&mut pending)).await;
            match step {
                SpawnActivationStep::Continue => {
                    self.pending_spawn_activations.insert(ticket, pending);
                }
                SpawnActivationStep::Dispatch(stage, work) => {
                    pending.inflight = Some(stage);
                    let custody = pending.custody.clone();
                    self.pending_spawn_activations.insert(ticket, pending);
                    let worker = self.spawn_activation_worker_context();
                    let command_tx = self.command_tx.clone();
                    let mob_id = self.definition.id.clone();
                    self.actor_io_tasks.spawn(async move {
                        let result = worker.run(&custody, *work).await;
                        if command_tx
                            .send(RoutedMobCommand::internal(
                                MobCommand::SpawnActivationStageSettled {
                                    ticket,
                                    outcome: Box::new(SpawnActivationStageOutcome {
                                        stage,
                                        custody,
                                        result,
                                    }),
                                },
                            ))
                            .await
                            .is_err()
                        {
                            tracing::warn!(
                                %mob_id,
                                %ticket,
                                stage = stage.as_str(),
                                "spawn activation stage settled after the actor stopped"
                            );
                        }
                    });
                    return;
                }
                SpawnActivationStep::Done(outcome) => {
                    Box::pin(self.settle_spawn_activation(pending, *outcome)).await;
                    return;
                }
            }
        }
    }

    /// Absorb one worker-owned stage completion.
    pub(super) async fn spawn_activation_stage_settled(
        &mut self,
        ticket: SpawnActivationTicket,
        outcome: Box<SpawnActivationStageOutcome>,
    ) {
        // A control may have settled while this outcome was travelling.
        Box::pin(self.tick_spawn_activation_custody()).await;
        let pause = self.spawn_activation_pause_is_active();
        let Some(mut pending) = self.pending_spawn_activations.remove(&ticket) else {
            // Terminal already settled (fail-stop, decline, actor teardown).
            // A successful stage whose activation is gone left live effects
            // behind: retain, never silently discard.
            let SpawnActivationStageOutcome {
                stage,
                custody,
                result,
            } = *outcome;
            if result.is_ok() {
                self.retain_spawn_activation_cleanup(
                    &custody,
                    format!(
                        "stage '{}' completed after its activation settled",
                        stage.as_str()
                    ),
                    RetainedSpawnCleanupKind::ProvisionAbort,
                );
            } else {
                tracing::warn!(
                    mob_id = %self.definition.id,
                    %ticket,
                    stage = stage.as_str(),
                    "ignoring failed stage for an activation that already settled"
                );
            }
            return;
        };
        if pending.inflight != Some(outcome.stage) {
            tracing::warn!(
                mob_id = %self.definition.id,
                %ticket,
                stage = outcome.stage.as_str(),
                "ignoring stale spawn activation stage completion"
            );
            self.pending_spawn_activations.insert(ticket, pending);
            return;
        }
        pending.inflight = None;
        // PARK, never commit, while a graph-scoped control owns the mob. The
        // outcome is retained verbatim (success AND failure): committing a
        // success can realize wiring, and committing a failure unwinds
        // through retire + unwire. Both are exactly what the control is
        // serializing against, and neither may be silently dropped.
        if pause {
            let stage = outcome.stage;
            pending.deferred_outcome = Some(outcome);
            self.park_spawn_activation(
                &mut pending,
                SpawnActivationParkReason::ResumeTopologyPending,
                "spawn_activation_stage_settled",
            );
            tracing::debug!(
                mob_id = %self.definition.id,
                %ticket,
                stage = stage.as_str(),
                "retained a settled activation stage until the graph control releases it"
            );
            self.pending_spawn_activations.insert(ticket, pending);
            return;
        }
        self.pending_spawn_activations.insert(ticket, pending);
        Box::pin(self.commit_settled_spawn_activation_stage(ticket, *outcome)).await;
    }

    /// Commit one settled stage against its exact incarnation.
    ///
    /// Shared by the live settlement path and the parked-resume path, so a
    /// retained outcome is verified against the CURRENT roster at commit
    /// time — never against the state it was parked in.
    async fn commit_settled_spawn_activation_stage(
        &mut self,
        ticket: SpawnActivationTicket,
        outcome: SpawnActivationStageOutcome,
    ) {
        let SpawnActivationStageOutcome {
            stage,
            custody,
            result,
        } = outcome;
        let Some(mut pending) = self.pending_spawn_activations.remove(&ticket) else {
            if result.is_ok() {
                self.retain_spawn_activation_cleanup(
                    &custody,
                    format!(
                        "stage '{}' commit found no activation custody",
                        stage.as_str()
                    ),
                    RetainedSpawnCleanupKind::ProvisionAbort,
                );
            }
            return;
        };
        let custody_state = if pending.custody != custody {
            // The outcome does not even belong to this activation's custody.
            SpawnActivationCustodyState::Superseded
        } else {
            self.spawn_activation_custody_state(
                &pending.custody,
                stage == SpawnActivationStage::TrustedPeerPublish
                    && matches!(pending.phase, SpawnActivationPhase::MembershipCommit),
            )
            .await
        };
        if custody_state != SpawnActivationCustodyState::Current {
            // Exact-incarnation decline. Never compensate here: a successor
            // may own this identity now, and destroying its member is exactly
            // the damage the fence exists to prevent.
            let reason = match custody_state {
                SpawnActivationCustodyState::Absent => format!(
                    "spawn activation stage '{}' declined: the admitted incarnation is gone from the roster and the machine's live set",
                    stage.as_str()
                ),
                _ => format!(
                    "spawn activation stage '{}' declined: the admitted incarnation is no longer current",
                    stage.as_str()
                ),
            };
            // A successful stage left a real effect on a real resource. The
            // roster/live-set projections are NOT a cleanup receipt — trust,
            // remote, and runtime effects outlive projection removal — so the
            // obligation is retained fail-closed and settles only on an exact
            // resource-owner compensation for this member ref + operation.
            if result.is_ok() {
                self.retain_spawn_activation_cleanup(
                    &custody,
                    reason.clone(),
                    RetainedSpawnCleanupKind::ProvisionAbort,
                );
            }
            Box::pin(self.settle_spawn_activation(pending, Err(MobError::Internal(reason)))).await;
            return;
        }
        match Box::pin(self.commit_spawn_activation_stage(&mut pending, stage, result)).await {
            Ok(()) => {
                self.pending_spawn_activations.insert(ticket, pending);
                Box::pin(self.drive_spawn_activation(ticket)).await;
            }
            Err(error) => {
                Box::pin(self.settle_spawn_activation(pending, Err(error))).await;
            }
        }
    }

    /// Commit one stage result: advance the phase cursor, or unwind exactly
    /// the way the pre-#1105 inline body did for that stage.
    async fn commit_spawn_activation_stage(
        &mut self,
        pending: &mut PendingSpawnActivation,
        stage: SpawnActivationStage,
        result: Result<(), MobError>,
    ) -> Result<(), MobError> {
        match stage {
            SpawnActivationStage::TrustedPeerPublish => {
                result?;
                pending.phase = SpawnActivationPhase::MembershipCommit;
            }
            SpawnActivationStage::WiringPeerIngress => {
                if let Err(drain_error) = result {
                    let agent_identity = pending.state.agent_identity.clone();
                    let surfaced_error = MobError::WiringError(format!(
                        "spawn wiring could not start durable peer ingress for '{agent_identity}': {drain_error}"
                    ));
                    return Err(Box::pin(self.unwind_failed_spawn_activation(
                        pending,
                        surfaced_error,
                        true,
                        "spawn peer-ingress bootstrap failed",
                    ))
                    .await);
                }
                pending.phase = SpawnActivationPhase::WiringRealize;
            }
            SpawnActivationStage::AutonomousReadiness => {
                if let Err(start_error) = result {
                    return Err(Box::pin(self.unwind_failed_spawn_activation(
                        pending,
                        start_error,
                        true,
                        "spawn host-loop start failed",
                    ))
                    .await);
                }
                pending.phase = SpawnActivationPhase::AutonomousStart;
            }
            SpawnActivationStage::AutonomousKickoff => {
                if let Err(start_error) = result {
                    return Err(Box::pin(self.unwind_failed_spawn_activation(
                        pending,
                        start_error,
                        true,
                        "spawn host-loop start failed",
                    ))
                    .await);
                }
                pending.phase = SpawnActivationPhase::Commit;
            }
            SpawnActivationStage::TurnDrivenPeerIngress => {
                // Pre-#1105 parity: the turn-driven drain failure propagated
                // without a spawn rollback.
                result?;
                pending.phase = SpawnActivationPhase::InitialTurn;
            }
            SpawnActivationStage::InitialTurn => {
                if let Err(start_error) = result {
                    return Err(Box::pin(self.unwind_failed_spawn_activation(
                        pending,
                        start_error,
                        false,
                        "turn-driven spawn initial turn failed",
                    ))
                    .await);
                }
                pending.phase = SpawnActivationPhase::Commit;
            }
        }
        Ok(())
    }

    /// Roll the failed activation back exactly as the inline body did:
    /// optionally clear kickoff state, then destroy the member and reset the
    /// spawn-exec phase. A failed rollback is retained, not swallowed.
    async fn unwind_failed_spawn_activation(
        &mut self,
        pending: &mut PendingSpawnActivation,
        surfaced_error: MobError,
        clear_kickoff: bool,
        context: &'static str,
    ) -> MobError {
        let agent_identity = pending.state.agent_identity.clone();
        if clear_kickoff {
            self.clear_kickoff_state(&agent_identity).await;
        }
        let generation = pending.state.generation;
        let profile_name = pending.state.profile_name.clone();
        let member_ref = pending.state.member_ref.clone();
        let operation_id = pending.state.operation_id.clone();
        let session_origin = pending.state.session_origin;
        let wired_spawn_targets = pending.state.wired_spawn_targets.clone();
        let planned_wiring_targets = pending.state.planned_wiring_targets.clone();
        self.retain_spawn_activation_cleanup(
            &pending.custody,
            format!("{context}: {surfaced_error}; rollback pending"),
            RetainedSpawnCleanupKind::SpawnRollback(Box::new(RetainedSpawnRollback {
                generation,
                profile_name: profile_name.clone(),
                member_ref: member_ref.clone(),
                operation_id: operation_id.clone(),
                session_origin,
                successful_wiring_targets: wired_spawn_targets.clone(),
                planned_wiring_targets: planned_wiring_targets.clone(),
            })),
        );
        let rollback = Box::pin(self.rollback_failed_spawn(
            &agent_identity,
            FailedSpawnRollback {
                generation,
                profile_name: &profile_name,
                member_ref: &member_ref,
                operation_id: &operation_id,
                session_origin,
                successful_wiring_targets: &wired_spawn_targets,
                planned_wiring_targets: &planned_wiring_targets,
            },
        ))
        .await;
        match rollback {
            Ok(()) => surfaced_error,
            Err(rollback_error) => {
                self.release_retained_spawn_rollback(&pending.custody, rollback_error.to_string());
                MobError::Internal(format!(
                    "{context} for '{agent_identity}': {surfaced_error}; rollback failed: {rollback_error}"
                ))
            }
        }
    }

    /// Deliver one activation's terminal outcome to its typed route.
    async fn settle_spawn_activation(
        &mut self,
        pending: PendingSpawnActivation,
        outcome: Result<FinalizeSpawnOutcome, MobError>,
    ) {
        let PendingSpawnActivation {
            ticket,
            route,
            custody,
            ..
        } = pending;
        self.pending_spawn_activations.remove(&ticket);
        let outcome = match outcome {
            Err(error) => {
                if let Some(entry) = self
                    .retained_spawn_activation_cleanups
                    .iter_mut()
                    .find(|entry| entry.custody == custody && entry.in_flight)
                {
                    entry.reply = Some((route, error));
                    return;
                }
                Err(error)
            }
            outcome => outcome,
        };
        self.notify_spawn_cleanup_waiters();
        Box::pin(self.settle_spawn_activation_route(route, outcome)).await;
    }

    /// Deliver a terminal spawn outcome to a route that may never have been
    /// registered — the pre-activation admission failures settle here too, so
    /// exactly one place owns reply custody for a routed spawn.
    pub(super) async fn settle_spawn_activation_route(
        &mut self,
        route: SpawnActivationRoute,
        outcome: Result<FinalizeSpawnOutcome, MobError>,
    ) {
        let SpawnActivationRoute::Receipt(route) = route;
        Box::pin(self.settle_spawn_activation_receipt(*route, outcome)).await;
    }

    /// Terminal bookkeeping for a receipt-routed activation.
    ///
    /// This is the pending-spawn batch tail, preserved verbatim: respawn
    /// topology classification, durable topology abandonment on failure, the
    /// per-spawn outcome log, the identity-reconcile disposition, and the
    /// reply-delivery counter.
    async fn settle_spawn_activation_receipt(
        &mut self,
        route: SpawnReceiptRoute,
        outcome: Result<FinalizeSpawnOutcome, MobError>,
    ) {
        let SpawnReceiptRoute {
            agent_identity,
            spawn_ticket,
            enqueued_at,
            respawn_origin,
            identity_reconcile,
            classify_respawn_topology,
            failure_warning_stage,
            spawn_source,
            reply_tx,
        } = route;
        let reply = match outcome {
            Ok(outcome) => {
                let mut receipt = outcome.receipt;
                if classify_respawn_topology {
                    match self.resolve_respawn_topology_restore_result(
                        &agent_identity,
                        outcome.failed_restore_peer_ids,
                    ) {
                        Ok(resolution) => {
                            receipt.failed_restore_peer_ids = resolution.failed_peer_ids;
                            Ok(receipt)
                        }
                        Err(error) => Err(error),
                    }
                } else {
                    Ok(receipt)
                }
            }
            Err(error) => Err(error),
        };
        let mut may_reply = true;
        let reply = match (reply, respawn_origin.as_ref()) {
            (Err(reply_error), Some(respawn_origin)) => match self
                .durably_abandon_respawn_topology_if_terminal_exact(&agent_identity, respawn_origin)
                .await
            {
                Ok(()) => Err(reply_error),
                Err(abandon_error) => {
                    self.durable_uncertainty_fail_stop = true;
                    self.respawn_topology_reply_withheld = true;
                    may_reply = false;
                    Err(MobError::Internal(format!(
                        "{reply_error}; durable respawn-topology abandonment failed and the actor is fail-stopping for cold recovery: {abandon_error}"
                    )))
                }
            },
            (reply, _) => reply,
        };
        if let (Err(error), Some(stage)) = (reply.as_ref(), failure_warning_stage) {
            tracing::warn!(
                mob_id = %self.definition.id,
                agent_identity = %agent_identity,
                spawn_source = spawn_source.unwrap_or("unknown"),
                stage,
                error = %error,
                "member spawn failed before asynchronous spawn custody"
            );
        }
        // Per-spawn outcome observability: the reply waiter may be detached
        // (identity reconcile deliberately drops it), so the terminal
        // provisioning outcome is logged here unconditionally.
        if let Some(spawn_ticket) = spawn_ticket {
            let elapsed_ms = enqueued_at
                .map(|enqueued_at| enqueued_at.elapsed().as_millis() as u64)
                .unwrap_or_default();
            match &reply {
                Ok(_) => tracing::info!(
                    spawn_ticket,
                    agent_identity = %agent_identity,
                    elapsed_ms,
                    "spawn built"
                ),
                Err(error) => tracing::info!(
                    spawn_ticket,
                    agent_identity = %agent_identity,
                    elapsed_ms,
                    error = %error,
                    "spawn failed"
                ),
            }
        }
        if let Some(authority) = identity_reconcile.as_ref() {
            let disposition = identity_member_actuation_disposition(&reply);
            self.record_identity_reconcile_disposition(&agent_identity, authority, disposition)
                .await;
        }
        if may_reply {
            let reply_delivered = reply_tx.send(reply).is_ok();
            #[cfg(test)]
            if identity_reconcile.is_some() && !reply_delivered {
                IDENTITY_RECONCILE_REPLY_DELIVERY_FAILURES
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            #[cfg(not(test))]
            let _ = reply_delivered;
        }
    }

    /// Actor-side phase body. Runs machine authority, durable appends, and
    /// bounded local projection writes only.
    async fn advance_spawn_activation(
        &mut self,
        pending: &mut PendingSpawnActivation,
    ) -> SpawnActivationStep {
        macro_rules! bail {
            ($result:expr) => {
                match $result {
                    Ok(value) => value,
                    Err(error) => return SpawnActivationStep::Done(Box::new(Err(error))),
                }
            };
        }
        match pending.phase {
            SpawnActivationPhase::MembershipPublish => {
                bail!(
                    self.member_staged_work_gate(
                        &pending.custody.agent_identity,
                        "spawn activation",
                    )
                );
                // Re-evaluated here rather than at registration: the retained
                // set is drained by a real retry owner, so an activation that
                // parked behind a control may find its predecessor's
                // compensation already settled by the time it runs.
                bail!(self.exact_spawn_activation_compensation_settled(&pending.custody));
                bail!(Self::validate_placed_kickoff_intent(&pending.state));
                pending.phase = SpawnActivationPhase::MembershipCommit;
                // Session-backed members publish operation readiness before
                // the roster projection can route to them. Peer-only members
                // never used that readiness path.
                if pending.state.member_ref.bridge_session_id().is_some()
                    && let Some(endpoint) = pending.state.member_peer_endpoint.clone()
                {
                    return SpawnActivationStep::Dispatch(
                        SpawnActivationStage::TrustedPeerPublish,
                        Box::new(SpawnActivationWork::PublishTrustedPeer {
                            member_ref: pending.state.member_ref.clone(),
                            operation_id: pending.state.operation_id.clone(),
                            endpoint,
                        }),
                    );
                }
                SpawnActivationStep::Continue
            }
            SpawnActivationPhase::MembershipCommit => {
                bail!(Box::pin(self.commit_spawn_membership_facts(&mut pending.state)).await);
                pending.phase = SpawnActivationPhase::WiringPlan;
                SpawnActivationStep::Continue
            }
            SpawnActivationPhase::WiringPlan => {
                Box::pin(self.plan_spawn_wiring(&mut pending.state)).await;
                pending.phase = SpawnActivationPhase::WiringRealize;
                if !pending.state.planned_wiring_targets.is_empty()
                    && pending.state.member_ref.bridge_session_id().is_some()
                    && !crate::runtime::member_runtime_is_host_owned(
                        self.dsl_authority.state(),
                        &pending.state.agent_identity,
                    )
                {
                    // Durable peer lifecycle delivery waits for the
                    // receiver's runtime admission before send returns, so
                    // the member's ingress must exist before any wiring send.
                    return SpawnActivationStep::Dispatch(
                        SpawnActivationStage::WiringPeerIngress,
                        Box::new(SpawnActivationWork::RequiredPeerIngress {
                            member_ref: pending.state.member_ref.clone(),
                        }),
                    );
                }
                SpawnActivationStep::Continue
            }
            SpawnActivationPhase::WiringRealize => {
                match Box::pin(self.realize_spawn_wiring(&mut pending.state)).await {
                    Ok(()) => {
                        pending.phase = SpawnActivationPhase::Kickoff;
                        SpawnActivationStep::Continue
                    }
                    Err(wire_error) => {
                        let error = Box::pin(self.unwind_failed_spawn_activation(
                            pending,
                            wire_error,
                            true,
                            "spawn wire fan-out failed",
                        ))
                        .await;
                        SpawnActivationStep::Done(Box::new(Err(error)))
                    }
                }
            }
            SpawnActivationPhase::Kickoff => Box::pin(self.enter_spawn_kickoff(pending)).await,
            #[cfg(feature = "runtime-adapter")]
            SpawnActivationPhase::AutonomousStart => {
                Box::pin(self.start_spawn_autonomous_kickoff(pending)).await
            }
            #[cfg(not(feature = "runtime-adapter"))]
            SpawnActivationPhase::AutonomousStart => {
                SpawnActivationStep::Done(Box::new(Err(MobError::Internal(
                    "autonomous spawn activation requires the runtime adapter".to_string(),
                ))))
            }
            SpawnActivationPhase::InitialTurn => {
                pending.phase = SpawnActivationPhase::Commit;
                let Some(initial_turn_prompt) = pending.state.initial_turn_prompt.take() else {
                    return SpawnActivationStep::Continue;
                };
                let objective_id = pending.state.objective_id;
                let admission = Box::pin(self.admit_turn_driven_spawn_initial_turn(
                    &pending.state.agent_identity.clone(),
                    &pending.state.agent_runtime_id.clone(),
                    pending.state.fence_token,
                    &pending.state.operation_id.clone(),
                    initial_turn_prompt,
                    objective_id,
                ))
                .await;
                let completion = match admission {
                    Ok(completion) => completion,
                    Err(error) => {
                        let error = Box::pin(self.unwind_failed_spawn_activation(
                            pending,
                            error,
                            false,
                            "turn-driven spawn initial turn failed",
                        ))
                        .await;
                        return SpawnActivationStep::Done(Box::new(Err(error)));
                    }
                };
                match completion {
                    SubmitWorkDispatchCompletion::Completed => SpawnActivationStep::Continue,
                    completion => SpawnActivationStep::Dispatch(
                        SpawnActivationStage::InitialTurn,
                        Box::new(SpawnActivationWork::InitialTurn {
                            completion: Box::new(completion),
                        }),
                    ),
                }
            }
            SpawnActivationPhase::Commit => SpawnActivationStep::Done(Box::new(
                Box::pin(self.commit_spawn_activation(&mut pending.state)).await,
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// Actor-owned phase bodies.
//
// Each of these is `prepare` or `commit`: MobMachine authority, a durable
// append, or a bounded local projection write. None of them awaits a
// provisioner, a bridge, a comms runtime, or a session service — that work
// belongs to `SpawnActivationWorkerContext`.
// ---------------------------------------------------------------------------

impl MobActor {
    /// Placed autonomous members must carry a durable kickoff intent that
    /// still matches the prompt/objective the carrier committed.
    fn validate_placed_kickoff_intent(state: &SpawnActivateState) -> Result<(), MobError> {
        if state.remote.is_none()
            || state.runtime_mode != crate::MobRuntimeMode::AutonomousHost
            || state.suppress_autonomous_initial_prompt
        {
            return Ok(());
        }
        let agent_identity = &state.agent_identity;
        let Some(intent) = state
            .remote
            .as_ref()
            .and_then(|remote| remote.pending_carrier.kickoff_intent.as_ref())
        else {
            return Err(MobError::Internal(format!(
                "placed autonomous member '{agent_identity}' has no durable kickoff intent"
            )));
        };
        if intent.prompt != state.prompt || state.objective_id != Some(intent.objective_id) {
            return Err(MobError::Internal(format!(
                "placed autonomous member '{agent_identity}' kickoff intent drifted after durable carrier commit"
            )));
        }
        Ok(())
    }

    /// Membership commit: machine-owned peer registration, the roster
    /// projection, the per-spawn tool overlay, the external-member rebind
    /// capability, and the pending kickoff mark.
    ///
    /// The roster insert runs AFTER the DSL `Spawn` authoritatively applies
    /// (Wave-A commit `e77ce8797` deleted the pre-DSL insert); without it the
    /// autonomous startup lane reads an empty roster (#30
    /// D-spawn-readiness-lookup).
    async fn commit_spawn_membership_facts(
        &mut self,
        state: &mut SpawnActivateState,
    ) -> Result<(), MobError> {
        let profile_name = state.profile_name.clone();
        let agent_identity = state.agent_identity.clone();
        let generation = state.generation;
        let fence_token = state.fence_token;
        let runtime_mode = state.runtime_mode;
        let suppress_autonomous_initial_prompt = state.suppress_autonomous_initial_prompt;
        let placed_kickoff_intent = state
            .remote
            .as_ref()
            .and_then(|remote| remote.pending_carrier.kickoff_intent.clone());
        let peer_id = state
            .member_peer_endpoint
            .as_ref()
            .map(|descriptor| descriptor.peer_id);
        // Host-materialized members: `CommitSpawnMembershipRemote` already
        // folded the member peer endpoint FROM THE ACK (single owner); a
        // second RegisterMemberPeer here would overwrite the machine fact
        // with a shell-derived name.
        if state.remote.is_none()
            && let Some(descriptor) = state.member_peer_endpoint.as_ref()
        {
            self.apply_dsl_input(
                mob_dsl::MobMachineInput::RegisterMemberPeer {
                    agent_identity: state.dsl_identity.clone(),
                    agent_runtime_id: mob_dsl::AgentRuntimeId::from_domain(&state.agent_runtime_id),
                    generation: mob_dsl::Generation::from_domain(generation),
                    fence_token: mob_dsl::FenceToken::from_domain(fence_token),
                    peer_endpoint: mob_dsl::MemberPeerEndpoint::from(descriptor),
                },
                "finalize_spawn_register_member_peer",
            )?;
        }
        {
            let mut roster = self.roster.write().await;
            roster.add_member(crate::roster::RosterAddEntry {
                agent_identity: state.identity.clone(),
                generation,
                fence_token,
                agent_runtime_id: state.agent_runtime_id.clone(),
                role: profile_name.clone(),
                runtime_mode,
                member_ref: Self::sanitized_member_ref(&state.member_ref),
                peer_id,
                transport_public_key: state.transport_public_key.take(),
                direct_member_fence: state.direct_member_fence.clone(),
                labels: state.labels.clone(),
                effective_profile_override: state.effective_profile_override.clone(),
                effective_model_override: state.effective_model_override.clone(),
            });
        }
        {
            // Same commit as the roster insert: retain the per-spawn overlay
            // so machine-authorized revival recomposes it. `None` clears any
            // prior incarnation's overlay (respawn replacement semantics).
            let mut per_spawn = self.per_spawn_external_tools.write().await;
            if let Some(dispatcher) = state.per_spawn_external_tools.take() {
                per_spawn.insert(state.identity.clone(), dispatcher);
            } else {
                per_spawn.remove(&state.identity);
            }
        }

        // Row #314: record the machine-owned external-member rebind capability
        // from the spawn's member_ref bootstrap proof so the external-member
        // projection reads it from machine state instead of re-deriving from
        // the roster bootstrap_token.
        self.apply_dsl_input(
            mob_dsl::MobMachineInput::SetExternalMemberRebindCapability {
                agent_identity: state.dsl_identity.clone(),
                capability: external_member_rebind_capability_from_member_ref(&state.member_ref),
            },
            "finalize_spawn_set_external_member_rebind_capability",
        )?;

        if runtime_mode == crate::MobRuntimeMode::AutonomousHost
            && !suppress_autonomous_initial_prompt
        {
            if state.is_replacing {
                self.clear_kickoff_state(&agent_identity).await;
            }
            let kickoff_objective_id = placed_kickoff_intent
                .as_ref()
                .map(|intent| intent.objective_id)
                .or(state.objective_id)
                .unwrap_or_default();
            let _ = self
                .apply_kickoff_input(
                    &agent_identity,
                    mob_dsl::MobMachineInput::KickoffMarkPending {
                        member_id: mob_dsl::AgentIdentity::from_domain(&agent_identity),
                        objective_id: kickoff_objective_id.to_string(),
                    },
                    "finalize_spawn_kickoff_mark_pending",
                )
                .await?;
        }
        tracing::debug!(
            agent_identity = %agent_identity,
            "MobActor spawn activation committed membership facts"
        );
        Ok(())
    }

    /// Wiring plan: the auto-wire + role-wiring fan-out for this spawn.
    ///
    /// Machine and definition reads only. Respawn restore edges are removed
    /// from the fail-fast fan-out: they are owned by the repair loop, whose
    /// per-peer failures are classified into a typed `TopologyRestoreFailed`
    /// result instead of destroying the replacement member.
    async fn plan_spawn_wiring(&mut self, state: &mut SpawnActivateState) {
        let profile_name = state.profile_name.clone();
        let agent_identity = state.agent_identity.clone();
        state.planned_wiring_targets =
            if state.identity_fenced_member || agent_identity.is_flow_member_namespace() {
                Vec::new()
            } else {
                Box::pin(self.spawn_wiring_targets(&profile_name, &agent_identity)).await
            };
        if state.auto_wire_parent
            && let Some(parent_target) = self
                .resolve_auto_wire_parent_target(
                    state.owner_bridge_session_id.as_ref(),
                    &agent_identity,
                )
                .await
            && !state.planned_wiring_targets.contains(&parent_target)
        {
            state.planned_wiring_targets.push(parent_target);
        }
        if let Some(plan) = state.restore_wiring.as_ref() {
            state
                .planned_wiring_targets
                .retain(|target| !plan.local_peers.contains(target));
        }
    }

    /// Wiring realization seam.
    ///
    /// The wire fan-out and the machine-owned respawn repair are owned by the
    /// wiring lane; activation only decides WHICH edges belong to this spawn
    /// and how a failure unwinds. This single function is the whole coupling
    /// point: when the wiring lane publishes its typed prepare/realize/commit
    /// API, only this body changes and the pipeline keeps its custody,
    /// continuation, and rollback contract unchanged.
    async fn realize_spawn_wiring(
        &mut self,
        state: &mut SpawnActivateState,
    ) -> Result<(), MobError> {
        let agent_identity = state.agent_identity.clone();
        let planned = state.planned_wiring_targets.clone();
        for target in &planned {
            let target_identity = crate::ids::AgentIdentity::from(target.as_str());
            let local_meerkat = agent_identity.clone();
            match Box::pin(self.handle_wire(
                local_meerkat,
                super::handle::PeerTarget::Local(target_identity),
            ))
            .await
            {
                Ok(()) => state.wired_spawn_targets.push(target.clone()),
                Err(wire_error) => {
                    // The member is in the DSL + roster but the role-wiring
                    // contract was violated: surface the failure so the
                    // caller can compensate (asserted by
                    // `test_role_wiring_failure_is_returned_to_spawn_caller`).
                    return Err(match wire_error {
                        MobError::WiringError(_) => wire_error,
                        other => MobError::WiringError(other.to_string()),
                    });
                }
            }
        }

        // Respawn restore is repair-only and precedes every kickoff/initial
        // work dispatch. The saved plan identifies candidates, while the
        // current MobMachine graph remains the authority: an edge removed
        // while a placed replacement was materializing must not be recreated
        // from a stale snapshot.
        let Some(plan) = state.restore_wiring.take() else {
            return Ok(());
        };
        for peer_identity in plan.local_peers {
            if peer_identity == agent_identity {
                continue;
            }
            let desired_now = self.dsl_authority.state().wiring_edges.iter().any(|edge| {
                (edge.a.0.as_str() == agent_identity.as_str()
                    && edge.b.0.as_str() == peer_identity.as_str())
                    || (edge.b.0.as_str() == agent_identity.as_str()
                        && edge.a.0.as_str() == peer_identity.as_str())
            });
            if !desired_now {
                tracing::debug!(
                    agent_identity = %agent_identity,
                    peer = %peer_identity,
                    "respawn: skipped stale local restore candidate absent from machine graph"
                );
                continue;
            }
            let peer_agent_identity = crate::ids::AgentIdentity::from(peer_identity.as_str());
            if let Err(error) = boxed_arm_future(|| {
                self.repair_machine_owned_respawn_wire(agent_identity.clone(), peer_agent_identity)
            })
            .await
            {
                tracing::warn!(
                    agent_identity = %agent_identity,
                    peer = %peer_identity,
                    %error,
                    "respawn: failed to restore machine-owned local peer edge"
                );
                state
                    .failed_restore_peer_ids
                    .push(RespawnTopologyPeerId::from(peer_identity.as_str()));
            }
        }
        for peer_spec in plan.external_peers {
            let desired_edge = Self::external_peer_edge(&agent_identity, &peer_spec);
            let desired_key = Self::external_peer_key(&agent_identity, &peer_spec.name);
            let machine_state = self.dsl_authority.state();
            let desired_now = machine_state.external_peer_edges.contains(&desired_edge)
                && machine_state.external_peer_edges_by_key.get(&desired_key)
                    == Some(&desired_edge);
            if !desired_now {
                tracing::debug!(
                    agent_identity = %agent_identity,
                    peer = %peer_spec.name,
                    "respawn: skipped stale external restore candidate absent from machine graph"
                );
                continue;
            }
            let peer_id = RespawnTopologyPeerId::from(peer_spec.peer_id.as_str());
            if let Err(error) = Box::pin(self.handle_wire(
                agent_identity.clone(),
                super::handle::PeerTarget::External(peer_spec.clone()),
            ))
            .await
            {
                tracing::warn!(
                    agent_identity = %agent_identity,
                    peer = %peer_spec.name,
                    %error,
                    "respawn: failed to restore machine-owned external peer edge"
                );
                state.failed_restore_peer_ids.push(peer_id);
            }
        }
        Ok(())
    }

    /// Kickoff lane selection.
    ///
    /// Placed activation commits BEFORE the record-before-send kickoff
    /// obligation opens; local members commit at the end of activation
    /// because their runtime binding/start is part of it.
    async fn enter_spawn_kickoff(
        &mut self,
        pending: &mut PendingSpawnActivation,
    ) -> SpawnActivationStep {
        #[cfg(feature = "runtime-adapter")]
        {
            pending.state.spawn_activation_committed = pending.state.runtime_mode
                == crate::MobRuntimeMode::AutonomousHost
                && crate::runtime::member_runtime_is_host_owned(
                    self.dsl_authority.state(),
                    &pending.state.agent_identity,
                );
            if pending.state.spawn_activation_committed
                && let Err(error) = self.apply_dsl_input(
                    mob_dsl::MobMachineInput::CommitSpawnActivation {
                        agent_identity: pending.state.dsl_identity.clone(),
                    },
                    "finalize_placed_spawn_activate_before_kickoff",
                )
            {
                return SpawnActivationStep::Done(Box::new(Err(error)));
            }
        }

        #[cfg(feature = "runtime-adapter")]
        if pending.state.runtime_mode == crate::MobRuntimeMode::AutonomousHost
            && crate::runtime::member_runtime_is_host_owned(
                self.dsl_authority.state(),
                &pending.state.agent_identity,
            )
            && !pending.state.suppress_autonomous_initial_prompt
        {
            // PLACED autonomous member: the loop runs on the MEMBER host
            // (ADJ-23 residency) and the kickoff prompt rides the placed
            // delivery lane, so activation only opens the durable obligation.
            if let Err(error) = Box::pin(self.kickoff_placed_member(&mut pending.state)).await {
                return SpawnActivationStep::Done(Box::new(Err(error)));
            }
            pending.phase = SpawnActivationPhase::Commit;
            return SpawnActivationStep::Continue;
        } else if pending.state.runtime_mode == crate::MobRuntimeMode::AutonomousHost {
            return Box::pin(self.begin_local_autonomous_kickoff(pending)).await;
        }

        if pending.state.runtime_mode == crate::MobRuntimeMode::TurnDriven
            && !pending.state.agent_identity.is_flow_member_namespace()
        {
            // Turn-driven mob members still need a persistent comms drain:
            // async peer requests/responses arrive between user turns, and
            // without a drain the `peer_response_terminal` notice never
            // reaches the session's runtime queue.
            pending.phase = SpawnActivationPhase::InitialTurn;
            if crate::runtime::member_runtime_is_host_owned(
                self.dsl_authority.state(),
                &pending.state.agent_identity,
            ) {
                return SpawnActivationStep::Continue;
            }
            return SpawnActivationStep::Dispatch(
                SpawnActivationStage::TurnDrivenPeerIngress,
                Box::new(SpawnActivationWork::OptionalPeerIngress {
                    member_ref: pending.state.member_ref.clone(),
                }),
            );
        }
        pending.phase = SpawnActivationPhase::Commit;
        SpawnActivationStep::Continue
    }

    /// Kickoff lane: locally hosted autonomous member. Marks the kickoff
    /// starting, drains the machine's `RequestRuntimeBinding` effect, and
    /// hands the opaque runtime readiness to the worker.
    #[cfg(feature = "runtime-adapter")]
    async fn begin_local_autonomous_kickoff(
        &mut self,
        pending: &mut PendingSpawnActivation,
    ) -> SpawnActivationStep {
        let agent_identity = pending.state.agent_identity.clone();
        if !pending.state.suppress_autonomous_initial_prompt {
            let marked = self
                .apply_kickoff_input(
                    &agent_identity,
                    mob_dsl::MobMachineInput::KickoffMarkStarting {
                        member_id: mob_dsl::AgentIdentity::from_domain(&agent_identity),
                    },
                    "finalize_spawn_kickoff_mark_starting",
                )
                .await;
            if let Err(error) = marked {
                return SpawnActivationStep::Done(Box::new(Err(error)));
            }
        }
        // Spawn emits RequestRuntimeBinding. Drain it before startup can
        // publish RuntimeBound, otherwise the session may emit a fallback
        // runtime id that MobMachine correctly rejects as not live.
        if let Err(binding_error) = Box::pin(self.flush_routed_effects()).await {
            let error = Box::pin(self.unwind_failed_spawn_activation(
                pending,
                binding_error,
                true,
                "spawn runtime binding failed",
            ))
            .await;
            return SpawnActivationStep::Done(Box::new(Err(error)));
        }
        pending.phase = SpawnActivationPhase::AutonomousStart;
        SpawnActivationStep::Dispatch(
            SpawnActivationStage::AutonomousReadiness,
            Box::new(SpawnActivationWork::AutonomousReadiness {
                member_ref: pending.state.member_ref.clone(),
            }),
        )
    }

    /// Kickoff lane (continued): the member's runtime is ready, so the
    /// machine can publish startup readiness and the kickoff prompt can be
    /// admitted off the actor.
    #[cfg(feature = "runtime-adapter")]
    async fn start_spawn_autonomous_kickoff(
        &mut self,
        pending: &mut PendingSpawnActivation,
    ) -> SpawnActivationStep {
        let agent_identity = pending.state.agent_identity.clone();
        let startup_marker = {
            let roster = self.roster.read().await;
            roster
                .get_by_identity(&AgentIdentity::from(agent_identity.as_str()))
                .map(|entry| {
                    (
                        mob_dsl::AgentRuntimeId::from_domain(&entry.agent_runtime_id),
                        mob_dsl::FenceToken::from_domain(entry.fence_token),
                    )
                })
        };
        let startup_marker = match startup_marker {
            Some(marker) => marker,
            None => {
                let error = Box::pin(self.unwind_failed_spawn_activation(
                    pending,
                    MobError::Internal(format!(
                        "autonomous member '{agent_identity}' missing roster entry for startup readiness"
                    )),
                    true,
                    "spawn host-loop start failed",
                ))
                .await;
                return SpawnActivationStep::Done(Box::new(Err(error)));
            }
        };
        if !self
            .dsl_authority
            .state()
            .member_startup_ready
            .contains(&startup_marker.0)
            && let Err(error) = self.apply_dsl_input(
                mob_dsl::MobMachineInput::StartupMarkReady {
                    agent_runtime_id: startup_marker.0,
                    fence_token: startup_marker.1,
                },
                "start_autonomous_member/startup_mark_ready",
            )
        {
            let error = Box::pin(self.unwind_failed_spawn_activation(
                pending,
                error,
                true,
                "spawn host-loop start failed",
            ))
            .await;
            return SpawnActivationStep::Done(Box::new(Err(error)));
        }

        pending.phase = SpawnActivationPhase::Commit;
        if pending.state.suppress_autonomous_initial_prompt {
            // Identity reconciliation resumed an authoritative transcript:
            // the runtime is up and must not receive a manufactured kickoff.
            tracing::debug!(
                agent_identity = %agent_identity,
                "autonomous member runtime resumed without a fresh kickoff"
            );
            return SpawnActivationStep::Continue;
        }
        let prepared = self.prepare_autonomous_kickoff_input(&pending.state);
        match prepared {
            Ok((bridge_session_id, input)) => SpawnActivationStep::Dispatch(
                SpawnActivationStage::AutonomousKickoff,
                Box::new(SpawnActivationWork::AutonomousKickoff {
                    bridge_session_id,
                    input: Box::new(input),
                }),
            ),
            Err(error) => {
                let error = Box::pin(self.unwind_failed_spawn_activation(
                    pending,
                    error,
                    true,
                    "spawn host-loop start failed",
                ))
                .await;
                SpawnActivationStep::Done(Box::new(Err(error)))
            }
        }
    }

    /// Build the kickoff admission input from machine-owned turn metadata.
    #[cfg(feature = "runtime-adapter")]
    fn prepare_autonomous_kickoff_input(
        &self,
        state: &SpawnActivateState,
    ) -> Result<(SessionId, meerkat_runtime::Input), MobError> {
        use meerkat_runtime::{Input, InputHeader, PromptInput};

        let agent_identity = &state.agent_identity;
        let bridge_session_id = state
            .member_ref
            .bridge_session_id()
            .cloned()
            .ok_or_else(|| {
                MobError::Internal(format!(
                    "autonomous member '{agent_identity}' must be session-backed"
                ))
            })?;
        if self.runtime_adapter.is_none() {
            return Err(MobError::Internal(format!(
                "autonomous member '{agent_identity}' requires admission-capable substrate (runtime adapter)"
            )));
        }
        let turn_metadata =
            machine_kickoff_turn_metadata(self.dsl_authority.state(), agent_identity)?;
        let input = Input::Prompt(PromptInput {
            injected_context: Vec::new(),
            header: InputHeader {
                id: meerkat_core::lifecycle::InputId::new(),
                timestamp: chrono::Utc::now(),
                source: meerkat_runtime::InputOrigin::Operator,
                durability: meerkat_runtime::InputDurability::Durable,
                visibility: meerkat_runtime::InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            content: state.prompt.clone(),
            typed_turn_appends: Vec::new(),
            turn_metadata,
        });
        Ok((bridge_session_id, input))
    }

    /// Kickoff lane: PLACED autonomous member (loop runs on the member host).
    #[cfg(feature = "runtime-adapter")]
    async fn kickoff_placed_member(
        &mut self,
        state: &mut SpawnActivateState,
    ) -> Result<(), MobError> {
        let agent_identity = state.agent_identity.clone();
        let entry = self
            .roster
            .read()
            .await
            .get(&agent_identity)
            .cloned()
            .ok_or_else(|| {
                MobError::Internal(format!(
                    "placed autonomous kickoff has no roster incarnation for '{agent_identity}'"
                ))
            })?;
        let expected_member = self.placed_member_incarnation(&entry)?;
        let kickoff_intent = state
            .remote
            .as_ref()
            .and_then(|remote| remote.pending_carrier.kickoff_intent.clone())
            .ok_or_else(|| {
                MobError::Internal(format!(
                    "placed autonomous kickoff has no durable intent for '{agent_identity}'"
                ))
            })?;
        let obligation_event = crate::event::PlacedKickoffObligationEvent {
            agent_identity: agent_identity.clone(),
            host_id: expected_member.host_id.clone(),
            host_binding_generation: expected_member.binding_generation,
            member_session_id: expected_member.member_session_id.clone(),
            generation: crate::ids::Generation::new(expected_member.generation),
            fence_token: crate::ids::FenceToken::new(expected_member.fence_token),
            input_id: kickoff_intent.input_id.clone(),
            objective_id: kickoff_intent.objective_id,
        };
        self.start_placed_kickoff_obligation_in_actor(obligation_event)
            .await?;
        self.ensure_member_event_pump(&agent_identity).await?;
        Ok(())
    }

    /// Machine admission for the turn-driven spawn's initial turn.
    ///
    /// The DSL `SubmitWork` transition and the dispatch preparation are actor
    /// authority; the returned completion carries only the member-local
    /// readiness and the runtime admission, which the worker realizes.
    async fn admit_turn_driven_spawn_initial_turn(
        &mut self,
        agent_identity: &AgentIdentity,
        agent_runtime_id: &AgentRuntimeId,
        fence_token: FenceToken,
        operation_id: &meerkat_core::ops::OperationId,
        content: ContentInput,
        inherited_objective_id: Option<meerkat_core::interaction::ObjectiveId>,
    ) -> Result<SubmitWorkDispatchCompletion, MobError> {
        let entry = {
            let roster = self.roster.read().await;
            roster.get(agent_identity).cloned()
        }
        .ok_or_else(|| {
            MobError::Internal(format!(
                "turn-driven spawn initial SubmitWork for '{agent_identity}' had no roster projection after Spawn admission"
            ))
        })?;

        let work_ref = WorkRef::new();
        let origin = WorkOrigin::Internal;
        let domain_identity = AgentIdentity::from(agent_identity.as_str());
        let dsl_identity = mob_dsl::AgentIdentity::from_domain(&domain_identity);
        let dsl_runtime_id = mob_dsl::AgentRuntimeId::from_domain(agent_runtime_id);
        let dsl_fence_token = mob_dsl::FenceToken::from_domain(fence_token);
        let dsl_work_id = mob_dsl::WorkId::from_work_ref(&work_ref);
        let dsl_origin = mob_dsl::WorkOrigin::from(origin);
        let transition = match mob_dsl::MobMachineMutator::apply(
            &mut self.dsl_authority,
            mob_dsl::MobMachineInput::SubmitWork {
                agent_identity: dsl_identity.clone(),
                agent_runtime_id: dsl_runtime_id.clone(),
                fence_token: dsl_fence_token,
                work_id: dsl_work_id.clone(),
                origin: dsl_origin,
            },
        ) {
            Ok(transition) => transition,
            Err(_) => {
                let current_state = self.state();
                return Err(Self::resolve_submit_work_rejection_in_authority(
                    &mut self.dsl_authority,
                    &dsl_identity,
                    &dsl_runtime_id,
                    dsl_fence_token,
                    agent_runtime_id,
                    origin,
                    agent_identity,
                    current_state,
                ));
            }
        };
        if transition.from_phase != transition.to_phase {
            let _ = self.phase_watch_tx.send(self.state());
        }
        self.publish_machine_state_projection();
        let ingress_authority = SubmitWorkIngressAuthority::from_transition(
            &transition,
            &dsl_runtime_id,
            dsl_fence_token,
            mob_dsl::Generation::from_domain(agent_runtime_id.generation),
            &dsl_work_id,
            dsl_origin,
        )?;
        drop(transition);

        let completion = self
            .dispatch_member_turn_after_machine_admission(
                &entry,
                ingress_authority,
                SubmitWorkDispatchRequest {
                    content,
                    system_prompt: None,
                    // Spawn kickoff is mob-internal coordination content; the
                    // injected-context slot belongs to the submit-work lane.
                    injected_context: Vec::new(),
                    // Mob-internal kickoff carries no host interaction id.
                    interaction_id: None,
                    objective_id: inherited_objective_id.or(machine_kickoff_objective_id(
                        self.dsl_authority.state(),
                        agent_identity,
                    )?),
                    handling_mode: meerkat_core::types::HandlingMode::Queue,
                    external_delivery_identity: None,
                    turn_metadata: None,
                    event_tx: None,
                    completion_tx: None,
                    bounded_result_spec: None,
                    llm_identity_applied_tx: None,
                    ack_mode: crate::mob_machine::SubmitWorkAckMode::IngressAccepted,
                    operation_id: Some(operation_id.clone()),
                    placed_completion_obligation: None,
                    placed_completion_context: None,
                },
            )
            .await?;
        tracing::debug!(
            agent_identity = %entry.agent_identity,
            runtime_id = %entry.agent_runtime_id,
            completion = completion.kind(),
            "turn-driven spawn initial turn admitted by the machine"
        );
        Ok(completion)
    }

    /// Activation commit: `CommitSpawnActivation`, event-pump
    /// re-materialization, and the finalize receipt.
    async fn commit_spawn_activation(
        &mut self,
        state: &mut SpawnActivateState,
    ) -> Result<FinalizeSpawnOutcome, MobError> {
        let agent_identity = state.agent_identity.clone();
        // Spawn ladder step 4: finalize. `CommitSpawnActivation` advances the
        // phase past `MembershipCommitted` and clears the per-identity
        // spawn-exec entry — the member is fully live and the ladder is
        // settled, so a future respawn of this identity can `BeginSpawnExec`
        // again. Best-effort respawn topology-restore failures
        // (`failed_restore_peer_ids`) do not block activation.
        if !state.spawn_activation_committed {
            self.apply_dsl_input(
                mob_dsl::MobMachineInput::CommitSpawnActivation {
                    agent_identity: state.dsl_identity.clone(),
                },
                "finalize_spawn_activate_commit_activation",
            )?;
        }
        // ADJ-24 + A17: a re-materialized incarnation rotates its comms
        // identity, so a LIVE pump (obligation- or tap-kept) still polls the
        // OLD transport and could never observe the bumped generation — the
        // §18.8:1004 fail-fast source. Replace it with fresh material from
        // the new roster incarnation (covers every finalize caller: spawn
        // batch, respawn, revival).
        let flow_obligation_outstanding = self
            .dsl_authority
            .state()
            .pending_remote_turn_outcomes
            .iter()
            .chain(
                self.dsl_authority
                    .state()
                    .committed_remote_turn_outcomes
                    .iter(),
            )
            .chain(
                self.dsl_authority
                    .state()
                    .resolved_remote_turn_outcomes
                    .iter(),
            )
            .any(|obligation| obligation.agent_identity.0 == agent_identity.as_str());
        let kickoff_obligation_outstanding = self
            .dsl_authority
            .state()
            .pending_placed_kickoff_outcomes
            .iter()
            .chain(
                self.dsl_authority
                    .state()
                    .resolved_placed_kickoff_outcomes
                    .iter(),
            )
            .any(|obligation| obligation.agent_identity.0 == agent_identity.as_str());
        let obligation_outstanding = flow_obligation_outstanding || kickoff_obligation_outstanding;
        if self.member_event_pumps.pump_exists(&agent_identity) || obligation_outstanding {
            tracing::debug!(
                agent_identity = %agent_identity,
                obligation_outstanding,
                "replacing live member event pump after re-materialization"
            );
            if let Err(error) = self.ensure_member_event_pump(&agent_identity).await {
                tracing::warn!(
                    agent_identity = %agent_identity,
                    error = %error,
                    "pump replacement after re-materialization failed; \
                     stale-transport polls back off until liveness lapses"
                );
            }
        }
        tracing::debug!(
            agent_identity = %agent_identity,
            "MobActor spawn activation done"
        );
        Ok(FinalizeSpawnOutcome {
            receipt: super::handle::MemberSpawnReceipt {
                member_ref: state.member_ref.clone(),
                direct_member_fence: state.direct_member_fence.clone(),
                operation_id: state.operation_id.clone(),
                session_origin: state.session_origin,
                rollback_authority: None,
                materialized_ack: None,
                failed_restore_peer_ids: Vec::new(),
            },
            failed_restore_peer_ids: std::mem::take(&mut state.failed_restore_peer_ids),
        })
    }
}

// ---------------------------------------------------------------------------
// Policy auto-spawn continuation.
//
// The work lane used to provision AND activate an absent member inline, so a
// policy auto-spawn held the actor across a whole member build. The delivery
// now parks — nothing has been admitted for it yet — the spawn runs on the
// ordinary pending-spawn path, and the parked delivery is re-submitted
// verbatim once the spawn settles.
// ---------------------------------------------------------------------------

/// One work delivery parked behind an in-flight policy auto-spawn.
pub(in crate::runtime) struct ParkedPolicyDelivery {
    pub(super) authority: CommandAuthority,
    pub(super) payload: Box<super::super::state::SubmitWorkPayload>,
    pub(super) reply_tx: oneshot::Sender<Result<(), MobError>>,
}

impl MobActor {
    /// Park one delivery and, when it is the first for this identity, stage
    /// the policy spawn.
    pub(super) async fn begin_policy_spawn_delivery(
        &mut self,
        agent_identity: AgentIdentity,
        spec: super::super::spawn_policy::SpawnSpec,
        work_ref: WorkRef,
        origin: WorkOrigin,
        delivery: ParkedPolicyDelivery,
    ) {
        let already_staged = self.policy_spawn_waiters.contains_key(&agent_identity);
        self.policy_spawn_waiters
            .entry(agent_identity.clone())
            .or_default()
            .push(delivery);
        if already_staged {
            tracing::debug!(
                mob_id = %self.definition.id,
                agent_identity = %agent_identity,
                "parked delivery behind an in-flight policy auto-spawn"
            );
            return;
        }
        if let Err(error) =
            Box::pin(self.stage_policy_spawn(&agent_identity, spec, &work_ref, origin)).await
        {
            Box::pin(self.release_policy_spawn_waiters(&agent_identity, Err(error))).await;
        }
    }

    /// Terminal classification of one policy auto-spawn.
    pub(super) async fn policy_spawn_settled(
        &mut self,
        agent_identity: &AgentIdentity,
        result: Result<super::super::handle::MemberSpawnReceipt, MobError>,
    ) {
        Box::pin(self.release_policy_spawn_waiters(agent_identity, result)).await;
    }

    async fn release_policy_spawn_waiters(
        &mut self,
        agent_identity: &AgentIdentity,
        result: Result<super::super::handle::MemberSpawnReceipt, MobError>,
    ) {
        let Some(parked) = self.policy_spawn_waiters.remove(agent_identity) else {
            return;
        };
        let result = match result {
            Ok(receipt) => {
                let roster = self.roster.read().await;
                match roster.get(agent_identity) {
                    Some(entry)
                        if entry.member_ref == Self::sanitized_member_ref(&receipt.member_ref)
                            && entry.direct_member_fence == receipt.direct_member_fence =>
                    {
                        Ok((entry.agent_runtime_id.clone(), entry.fence_token))
                    }
                    _ => Err(MobError::StaleMemberOperatorAuthority {
                        member_id: agent_identity.clone(),
                        reason: "policy spawn receipt no longer names the current member".into(),
                    }),
                }
            }
            Err(error) => Err(error),
        };
        match result {
            Ok((runtime_id, fence_token)) => {
                // Re-submit verbatim: the member now exists, so the ordinary
                // lane re-runs admission with MobMachine as the authority. A
                // dropped caller is skipped rather than executed as a ghost
                // turn. The re-submission runs off the loop: the actor is the
                // consumer of this channel and must never block on it.
                let mut resubmit = Vec::with_capacity(parked.len());
                for ParkedPolicyDelivery {
                    authority,
                    mut payload,
                    reply_tx,
                } in parked
                {
                    if reply_tx.is_closed() {
                        tracing::debug!(
                            mob_id = %self.definition.id,
                            agent_identity = %agent_identity,
                            "policy-spawn delivery abandoned before its member existed"
                        );
                        continue;
                    }
                    payload.runtime_id = runtime_id.clone();
                    payload.fence_token = fence_token;
                    resubmit.push((authority, payload, reply_tx));
                }
                if resubmit.is_empty() {
                    return;
                }
                let command_tx = self.command_tx.clone();
                let mob_id = self.definition.id.clone();
                let identity = agent_identity.clone();
                self.actor_io_tasks.spawn(async move {
                    for (authority, payload, reply_tx) in resubmit {
                        if command_tx
                            .send(RoutedMobCommand {
                                authority,
                                cmd: MobCommand::SubmitWork { payload, reply_tx },
                            })
                            .await
                            .is_err()
                        {
                            tracing::warn!(
                                %mob_id,
                                agent_identity = %identity,
                                "policy-spawn delivery could not be re-submitted; the actor is gone"
                            );
                            return;
                        }
                    }
                });
            }
            Err(error) => {
                if parked.len() == 1 {
                    if let Some(ParkedPolicyDelivery { reply_tx, .. }) = parked.into_iter().next() {
                        let _ = reply_tx.send(Err(error));
                    }
                    return;
                }
                let error = Arc::new(error);
                for ParkedPolicyDelivery { reply_tx, .. } in parked {
                    let _ =
                        reply_tx.send(Err(MobError::SharedLifecycleFailure(Arc::clone(&error))));
                }
            }
        }
    }
}

#[cfg(test)]
mod spawn_activation_ticket_tests {
    use super::SpawnActivationTicket;

    #[test]
    fn activation_tickets_are_monotonic_and_do_not_wrap() {
        let mut ticket = SpawnActivationTicket::default();
        assert_eq!(ticket.next().expect("first").0, 1);
        assert_eq!(ticket.next().expect("second").0, 2);

        let mut exhausted = SpawnActivationTicket(u64::MAX);
        assert!(exhausted.next().is_err());
        assert_eq!(exhausted.0, u64::MAX);
    }
}
