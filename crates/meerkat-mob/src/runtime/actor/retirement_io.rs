//! Owned retirement effects. The actor retains the exact roster anchor and
//! alone publishes generated lifecycle transitions after each effect settles.

use super::member_effect_lane::{
    MemberEffectAck, MemberEffectCommit, MemberEffectCommitFuture, MemberEffectRequest,
    MemberEffectRetention, MemberEffectSettlement, MemberFence, MemberIncarnationFence,
};
use super::*;

pub(super) enum RetirementReply {
    Retire(oneshot::Sender<Result<(), MobError>>),
    BatchRetire(oneshot::Sender<Result<(), MobError>>),
    Respawn {
        snapshot: RespawnSnapshot,
        replacement: Box<super::super::handle::SpawnMemberSpec>,
        operation_owner: Option<(
            SessionId,
            Arc<dyn meerkat_core::ops_lifecycle::OpsLifecycleRegistry>,
        )>,
        reply: oneshot::Sender<
            Result<
                super::super::handle::MemberRespawnReceipt,
                super::super::handle::MobRespawnError,
            >,
        >,
    },
    IdentityReconcile(IdentityReconcileCompletionAuthority),
    SpawnRollback,
}

#[derive(Clone, Copy)]
enum PendingCleanupNext {
    OperatorAdmission,
    Disposal,
}

pub(super) enum RetirementAfter {
    RetireAll(oneshot::Sender<Result<(), MobError>>),
    Complete(oneshot::Sender<Result<(), MobError>>),
    Reset {
        prior_state: MobState,
        reply: oneshot::Sender<Result<(), MobError>>,
    },
}

pub(in crate::runtime) struct RetirementBatch {
    context: &'static str,
    after: RetirementAfter,
    pending_members: VecDeque<AgentIdentity>,
    active_member: Option<(AgentIdentity, oneshot::Receiver<Result<(), MobError>>)>,
    settled_members: Vec<(AgentIdentity, Result<(), MobError>)>,
}

pub(in crate::runtime) type RetirementContinuation = Box<RetirementState>;

pub(in crate::runtime) struct RetirementState {
    ticket: u64,
    entry: RosterEntry,
    preserve_binding: bool,
    preserve_topology: bool,
    deadline: Instant,
    admission: Option<tokio::sync::watch::Sender<bool>>,
    reply: RetirementReply,
    detach: Vec<MobDestroyingSessionIngressObligation>,
    retired_comms: Option<Arc<dyn CoreCommsRuntime>>,
    routes: Vec<super::super::composition::MobSeamEffect>,
    retirement_started: bool,
    terminal_published: bool,
    disposal: Option<DisposalContext>,
    placed_peers: Vec<AgentIdentity>,
    remote_peers: Vec<AgentIdentity>,
    archive_disposal: Option<mob_dsl::MemberSessionDisposal>,
    participants: Vec<MemberIncarnationFence>,
    external_edges: Vec<mob_dsl::ExternalPeerEdge>,
    external_comms: Option<Arc<dyn CoreCommsRuntime>>,
    kickoff_notices: Vec<&'static str>,
    placement: Option<RetirementPlacementFence>,
    release_reservation: Option<super::super::state::HostOrphanReleaseKey>,
    retained_outcomes: Vec<RetainedRetirementOutcome>,
    pending_next: PendingCleanupNext,
    pending_slots: VecDeque<super::super::pending_spawn_lineage::PendingSpawnSlot>,
    pending_slot: Option<Box<super::super::pending_spawn_lineage::PendingSpawnSlot>>,
    pending_errors: Vec<String>,
    pending_retire_incarnation: Option<RetirePendingSpawnCleanupIncarnation>,
    rollback: Option<Box<SpawnRollbackState>>,
}

struct SpawnRollbackState {
    custody: spawn_activation::SpawnActivationCustody,
    material: spawn_activation::FailedSpawnRollbackMaterial,
    phase: SpawnRollbackPhase,
    endpoints: BTreeMap<AgentIdentity, SpawnRollbackEndpoint>,
    notices: Vec<TrustedPeerDescriptor>,
    notice_index: usize,
    trust_index: usize,
    trust_retry: bool,
    cleanup_peers: Vec<AgentIdentity>,
    placed_cleanup_index: usize,
    placed_edges_cleaned: BTreeSet<AgentIdentity>,
    placement_fences: BTreeMap<AgentIdentity, Option<RetirementPlacementFence>>,
    resume_authority: Option<super::super::provisioner::ResumedMemberRollbackAuthority>,
    failure: Option<MobError>,
    compensating: bool,
    peer_description: String,
    sender: Option<Arc<dyn CoreCommsRuntime>>,
    unsettled: bool,
}

#[derive(Clone)]
struct SpawnRollbackEndpoint {
    spec: TrustedPeerDescriptor,
    comms: Option<Arc<dyn CoreCommsRuntime>>,
    binding: Option<crate::RuntimeBinding>,
}

struct SpawnRollbackRetirePlan {
    input: mob_dsl::MobMachineInput,
    journal: mob_dsl::MobLifecycleJournalKind,
    session: Option<mob_dsl::SessionId>,
}

#[derive(Clone, Copy)]
enum SpawnRollbackPhase {
    CaptureResume,
    Endpoints,
    Journal,
    Notices,
    PlacedTrust,
    Trust,
    Retire,
    RestoreResume,
    TerminalJournal,
    Projection,
    FreshDisposal,
}

enum SpawnRollbackObservation {
    ResumeAuthority(Result<super::super::provisioner::ResumedMemberRollbackAuthority, MobError>),
    Endpoints {
        observed: RetirementEndpointObservations,
        description: String,
    },
    Journal(Result<(), MobError>),
    Notice {
        target: TrustedPeerDescriptor,
        result: Result<(), MobError>,
    },
    Trust(Result<(), MobError>),
    PlacedTrust(wiring_io::WireRealized),
    RemoteTrust {
        peer: TrustedPeerDescriptor,
        result: RetirementRevokeObservation,
    },
    Restored(Result<(), MobError>),
    TerminalJournal(Result<(), MobError>),
    Projection(Result<(), MobError>),
    Compensated(Result<(), MobError>),
}

struct RetainedRetirementOutcome {
    _ticket: u64,
    _observation: RetirementObservation,
}

#[derive(PartialEq, Eq)]
struct RetirementPlacementFence {
    host: mob_dsl::HostId,
    session: Option<mob_dsl::SessionId>,
    binding_generation: Option<u64>,
    binding_incarnation: Option<u64>,
}

enum RetirementObservation {
    SpawnRollback(SpawnRollbackObservation),
    LiveMutations(Vec<Result<MemberLiveMutationCompletion, tokio::task::JoinError>>),
    LiveClosed(Result<(), MobError>),
    BeforeAdmissionQuiesced(Result<Option<Arc<dyn CoreCommsRuntime>>, MobError>),
    RuntimeQuiesced(Result<Option<Arc<dyn CoreCommsRuntime>>, MobError>),
    IngressDetached(Result<(), MobError>),
    RouteObserved(Result<RetirementRouteObservation, MobError>),
    Routed {
        effect: super::super::composition::MobSeamEffect,
        result: Result<
            Option<meerkat_runtime::composition::DispatchOutcome>,
            meerkat_runtime::composition::DispatchRefusal,
        >,
    },
    Archived(Result<mob_dsl::MemberSessionDisposal, MobError>),
    PlacedEdge(wiring_io::WireRealized),
    HostStopped(Result<(), MobError>),
    TrustRemoved(Result<(), MobError>),
    RemoteEdge(wiring_io::WireRealized),
    #[cfg(not(target_arch = "wasm32"))]
    AcceptorRemoved {
        state: ControllingAcceptorState,
        result: Result<(), MobError>,
    },
    AttachmentsReleased(Result<(), MobError>),
    CarrierDeleted {
        obligation: mob_dsl::PlacedCarrierCleanupObligation,
        result: Result<(), MobError>,
    },
    ProjectionCleaned(Result<(), MobError>),
    SupervisorRevoked {
        peer: TrustedPeerDescriptor,
        result: RetirementRevokeObservation,
    },
    SupervisorTrustRemoved(Result<(), MobError>),
    TrustEndpoints(RetirementEndpointObservations),
    NotificationEndpoints(RetirementEndpointObservations),
    ExternalCommsObserved(Option<Arc<dyn CoreCommsRuntime>>),
    ExternalTrustRemoved {
        edge: mob_dsl::ExternalPeerEdge,
        result: Result<(), MobError>,
    },
    ExternalTrustRestored {
        original: MobError,
        result: Result<(), MobError>,
    },
    PeerDeliveriesSettled(Result<(), MobError>),
    PendingAnchorsRetried(Vec<(PendingSpawnCleanupAnchor, Result<(), MobError>)>),
    PendingTaskObserved,
    PendingAnchorAborted {
        anchor: PendingSpawnCleanupAnchor,
        result: Result<(), MobError>,
    },
    PendingRemoteCleaned {
        obligation: mob_dsl::PlacedCarrierCleanupObligation,
        result: Result<(), MobError>,
    },
    PendingRemoteReleased {
        cleanup: Box<PendingRemoteCleanup>,
        result: Result<(), MobError>,
    },
}

struct PendingRemoteCleanup {
    carrier: crate::store::MobPlacedSpawnCarrierRecord,
    obligation: mob_dsl::PlacedCarrierCleanupObligation,
    authority: crate::store::MobPlacedSpawnCleanupAuthority,
    display: String,
}

pub(super) struct RetirementEndpointObservations {
    pub(super) endpoints: BTreeMap<AgentIdentity, Result<WiringEndpoint, MobError>>,
    pub(super) retained_spec: Option<Result<Option<TrustedPeerDescriptor>, MobError>>,
    pub(super) supervisor_comms: Arc<dyn CoreCommsRuntime>,
}

enum RetirementEndpointPlan {
    Ready(WiringEndpoint),
    Local {
        entry: RosterEntry,
        member_ref: MemberRef,
        comms_name: String,
    },
    Refused(MobError),
}

enum RetirementRevokeObservation {
    Confirmed,
    Refused {
        error: MobError,
        rollback: Result<(), MobError>,
    },
    InstallUncertain(MobError),
}

struct RetirementRouteObservation {
    archive_complete: bool,
    exact_local_target_quiescent: bool,
}

struct RetirementArchiveObservationSource {
    service: Arc<dyn MobSessionService>,
    #[cfg(feature = "runtime-adapter")]
    adapter: Option<Arc<meerkat_runtime::MeerkatMachine>>,
}

impl RetirementArchiveObservationSource {
    async fn already_complete(&self, session: &SessionId) -> Result<bool, MobError> {
        if self.service.has_live_session(session).await?
            || self
                .service
                .load_persisted_session(session)
                .await?
                .is_some()
        {
            return Ok(false);
        }
        #[cfg(feature = "runtime-adapter")]
        if let Some(adapter) = &self.adapter
            && adapter
                .archive_runtime_residue_present(session)
                .await
                .map_err(|error| MobError::Internal(error.to_string()))?
        {
            return Ok(false);
        }
        self.service
            .session_known_to_archive_authority(session)
            .await
            .map_err(MobError::from)
    }
}

struct RetirementCommit {
    identity: AgentIdentity,
    ticket: u64,
    observation: Option<RetirementObservation>,
}

impl MemberEffectCommit for RetirementCommit {
    fn commit(
        self: Box<Self>,
        actor: &mut MobActor,
        settlement: MemberEffectSettlement,
    ) -> MemberEffectCommitFuture<'_> {
        Box::pin(async move {
            let Some(mut continuation) = actor.retirements.remove(&self.identity) else {
                actor.durable_uncertainty_fail_stop = true;
                return MemberEffectAck::Retained(MemberEffectRetention::resumable(
                    MobError::Internal("retirement effect lost its continuation".to_string()),
                    self,
                ));
            };
            if continuation.ticket != self.ticket {
                if let Some(observation) = self.observation {
                    continuation
                        .retained_outcomes
                        .push(RetainedRetirementOutcome {
                            _ticket: self.ticket,
                            _observation: observation,
                        });
                }
                actor.retirements.insert(self.identity, continuation);
                actor.durable_uncertainty_fail_stop = true;
                return MemberEffectAck::Retained(MemberEffectRetention::unresumable(
                    MobError::Internal("retirement effect ticket was superseded".to_string()),
                ));
            }
            continuation
                .routes
                .extend(actor.take_retirement_routes(&continuation.entry));
            let Some(observation) = self.observation else {
                if let Some(rollback) = continuation.rollback.as_mut() {
                    rollback.unsettled = true;
                    let custody = rollback.custody.clone();
                    actor
                        .finish_spawn_rollback_attempt(
                            &custody,
                            Some(continuation),
                            Err(MobError::Internal(
                                "spawn rollback worker ended without physical settlement".into(),
                            )),
                        )
                        .await;
                    return MemberEffectAck::Settled;
                }
                actor.retirements.insert(self.identity, continuation);
                actor.durable_uncertainty_fail_stop = true;
                return MemberEffectAck::Retained(MemberEffectRetention::unresumable(
                    MobError::Internal("retirement effect has no owner result".to_string()),
                ));
            };
            if actor.durable_uncertainty_fail_stop
                || !settlement.is_current()
                || !actor.spawn_rollback_placement_fences_current(&continuation)
                || actor
                    .retirement_effect_is_current(&continuation.entry)
                    .await
                    .is_err()
                || !continuation.terminal_published
                    && actor.retirement_placement_fence(&continuation.entry.agent_identity)
                        != continuation.placement
            {
                continuation
                    .retained_outcomes
                    .push(RetainedRetirementOutcome {
                        _ticket: self.ticket,
                        _observation: observation,
                    });
                if let Some(rollback) = &continuation.rollback {
                    let custody = rollback.custody.clone();
                    actor
                        .finish_spawn_rollback_attempt(
                            &custody,
                            Some(continuation),
                            Err(MobError::StaleMemberOperatorAuthority {
                                member_id: self.identity,
                                reason: "spawn rollback retains its exact unsettled predecessor"
                                    .into(),
                            }),
                        )
                        .await;
                    return MemberEffectAck::Settled;
                }
                actor.retirements.insert(self.identity, continuation);
                actor.durable_uncertainty_fail_stop = true;
                return MemberEffectAck::Retained(MemberEffectRetention::unresumable(
                    MobError::Internal(
                        "retirement effect no longer has current authority".to_string(),
                    ),
                ));
            }
            actor
                .pending_routed_effects
                .append(&mut continuation.routes);
            actor.resume_retirement(continuation, observation).await;
            if !actor.durable_uncertainty_fail_stop {
                actor.notify_spawn_cleanup_waiters();
                MemberEffectAck::Settled
            } else {
                MemberEffectAck::Retained(MemberEffectRetention::unresumable(MobError::Internal(
                    "retirement continuation remains unsettled".to_string(),
                )))
            }
        })
    }
}

impl MobActor {
    pub(super) fn retirement_has_pending_spawn_cleanup(&self, identity: &AgentIdentity) -> bool {
        self.retirements.get(identity).is_some_and(|continuation| {
            continuation.pending_slot.is_some() || !continuation.pending_slots.is_empty()
        })
    }

    fn pending_anchor_matches(
        a: &PendingSpawnCleanupAnchor,
        b: &PendingSpawnCleanupAnchor,
    ) -> bool {
        a.spawn_ticket == b.spawn_ticket
            && a.agent_identity == b.agent_identity
            && a.session_id == b.session_id
            && a.operation_id == b.operation_id
            && a.retire_incarnation == b.retire_incarnation
    }

    async fn retirement_retry_pending_anchors(&mut self, continuation: RetirementContinuation) {
        let anchors = self
            .pending_spawn_cleanup_anchors
            .values()
            .filter(|anchor| anchor.agent_identity == continuation.entry.agent_identity)
            .cloned()
            .collect::<Vec<_>>();
        if anchors.is_empty() {
            self.retirement_select_pending_slots(continuation).await;
            return;
        }
        let provisioner = self.provisioner.clone();
        self.dispatch_retirement(continuation, "retire-pending-anchor-retry", async move {
            let mut results = Vec::new();
            for anchor in anchors {
                let result = provisioner
                    .abort_member_provision(
                        &MemberRef::from_bridge_session_id(anchor.session_id.clone()),
                        &anchor.operation_id,
                        &anchor.reason,
                    )
                    .await;
                results.push((anchor, result));
            }
            RetirementObservation::PendingAnchorsRetried(results)
        });
    }

    async fn retirement_pending_anchors_retried(
        &mut self,
        mut continuation: RetirementContinuation,
        results: Vec<(PendingSpawnCleanupAnchor, Result<(), MobError>)>,
    ) {
        for (anchor, result) in results {
            if self
                .pending_spawn_cleanup_anchors
                .get(&anchor.spawn_ticket)
                .is_some_and(|current| !Self::pending_anchor_matches(current, &anchor))
            {
                self.durable_uncertainty_fail_stop = true;
                self.finish_retirement(
                    continuation,
                    Err(MobError::Internal(
                        "pending spawn cleanup receipt targets a different exact anchor".into(),
                    )),
                )
                .await;
                return;
            }
            match result {
                Ok(()) => {
                    self.pending_spawn_cleanup_anchors
                        .remove(&anchor.spawn_ticket);
                }
                Err(error) => {
                    continuation.pending_errors.push(format!(
                        "{} ticket {} session {} operation {}: {error}",
                        anchor.agent_identity,
                        anchor.spawn_ticket,
                        anchor.session_id,
                        anchor.operation_id,
                    ));
                    self.pending_spawn_cleanup_anchors
                        .entry(anchor.spawn_ticket)
                        .or_insert(anchor);
                }
            }
        }
        if continuation.pending_errors.is_empty() {
            if self
                .pending_spawn_cleanup_anchors
                .values()
                .any(|anchor| anchor.agent_identity == continuation.entry.agent_identity)
            {
                Box::pin(self.retirement_retry_pending_anchors(continuation)).await;
            } else {
                self.retirement_select_pending_slots(continuation).await;
            }
        } else {
            let error = Self::pending_spawn_cleanup_error(
                "retire command retained pending-spawn cleanup",
                std::mem::take(&mut continuation.pending_errors),
            );
            self.finish_retirement(continuation, Err(error)).await;
        }
    }

    async fn retirement_select_pending_slots(&mut self, mut continuation: RetirementContinuation) {
        let identity = continuation.entry.agent_identity.clone();
        let slots = match continuation.pending_next {
            PendingCleanupNext::OperatorAdmission => {
                match self.classify_retire_pending_spawn_disposition(&identity) {
                    Ok(RetirePendingSpawnVerdict::CancelCommittedIncarnation {
                        agent_runtime_id,
                        generation,
                        pending_spawn_session_id,
                    }) => {
                        let session = match SessionId::parse(&pending_spawn_session_id.0) {
                            Ok(session) => session,
                            Err(error) => {
                                self.finish_retirement(
                                    continuation,
                                    Err(MobError::Internal(error.to_string())),
                                )
                                .await;
                                return;
                            }
                        };
                        continuation.pending_retire_incarnation =
                            Some(RetirePendingSpawnCleanupIncarnation {
                                agent_runtime_id,
                                generation,
                                pending_spawn_session_id,
                            });
                        let slots = self
                            .pending_spawns
                            .take_for_member_session(&identity, &session);
                        if slots.is_empty() {
                            self.finish_retirement(continuation, Err(MobError::Internal(
                                "generated pending-spawn cancellation has no exact shell capability".into(),
                            ))).await;
                            return;
                        }
                        slots
                    }
                    Ok(
                        RetirePendingSpawnVerdict::CommittedIncarnationWithoutPendingSpawn {
                            ..
                        }
                        | RetirePendingSpawnVerdict::PreservePendingSpawnForAbsentIdentity,
                    ) => Vec::new(),
                    Err(error) => {
                        self.finish_retirement(continuation, Err(error)).await;
                        return;
                    }
                }
            }
            PendingCleanupNext::Disposal => self.pending_spawns.take_for_member(&identity),
        };
        continuation.pending_slots = slots.into();
        for slot in &continuation.pending_slots {
            if let Some(task) = slot.task.as_ref() {
                task.abort();
            }
        }
        for _ in 0..continuation.pending_slots.len() {
            if let Err(error) = self.apply_dsl_input(
                mob_dsl::MobMachineInput::CancelPendingSpawn {
                    agent_identity: mob_dsl::AgentIdentity::from_domain(&identity),
                },
                "cancel_pending_spawn_slots",
            ) {
                self.durable_uncertainty_fail_stop = true;
                self.finish_retirement(continuation, Err(error)).await;
                return;
            }
        }
        self.retirement_next_pending_slot(continuation).await;
    }

    fn retirement_next_pending_slot(
        &mut self,
        mut continuation: RetirementContinuation,
    ) -> ActorCommandFuture<'_, ()> {
        if let Some(slot) = continuation.pending_slots.pop_front() {
            continuation.pending_slot = Some(Box::new(slot));
            boxed_arm_future(|| self.retirement_observe_pending_task(continuation))
        } else if !continuation.pending_errors.is_empty() {
            let error = Self::pending_spawn_cleanup_error(
                "member retirement pending-spawn cleanup",
                std::mem::take(&mut continuation.pending_errors),
            );
            self.finish_retirement(continuation, Err(error))
        } else {
            match continuation.pending_next {
                PendingCleanupNext::OperatorAdmission => {
                    boxed_arm_future(|| self.retirement_begin_member_effects(continuation))
                }
                PendingCleanupNext::Disposal => boxed_arm_future(|| {
                    self.retirement_prepare_disposal_after_pending(continuation)
                }),
            }
        }
    }

    async fn retirement_observe_pending_task(&mut self, mut continuation: RetirementContinuation) {
        let retire_incarnation = continuation.pending_retire_incarnation.clone();
        let Some(slot) = continuation.pending_slot.as_mut() else {
            self.durable_uncertainty_fail_stop = true;
            self.finish_retirement(
                continuation,
                Err(MobError::Internal(
                    "pending spawn custody is missing".into(),
                )),
            )
            .await;
            return;
        };
        if slot.task.as_ref().is_some_and(|task| !task.is_finished()) {
            self.dispatch_retirement(
                continuation,
                "retire-pending-task-observation",
                async move {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    RetirementObservation::PendingTaskObserved
                },
            );
            return;
        }
        // Tokio's completed-task observation makes this join ready. It only
        // releases the nested task handle; the provision abort below is the
        // cleanup receipt, never the join or the polling interval.
        if let Some(task) = slot.task.take() {
            let _ = task.await;
        }
        let anchor = match Self::pending_spawn_cleanup_anchor_for_slot(
            slot,
            "retire command received",
            retire_incarnation.as_ref(),
        ) {
            Ok(anchor) => anchor,
            Err(error) => {
                self.durable_uncertainty_fail_stop = true;
                self.finish_retirement(continuation, Err(error)).await;
                return;
            }
        };
        let Some(anchor) = anchor else {
            self.retirement_cleanup_pending_remote(continuation).await;
            return;
        };
        if self
            .pending_spawn_cleanup_anchors
            .get(&anchor.spawn_ticket)
            .is_some_and(|current| !Self::pending_anchor_matches(current, &anchor))
        {
            self.durable_uncertainty_fail_stop = true;
            self.finish_retirement(
                continuation,
                Err(MobError::Internal(
                    "pending spawn cleanup anchor was superseded".into(),
                )),
            )
            .await;
            return;
        }
        self.pending_spawn_cleanup_anchors
            .insert(anchor.spawn_ticket, anchor.clone());
        let provisioner = self.provisioner.clone();
        self.dispatch_retirement(continuation, "retire-pending-provision-abort", async move {
            let result = provisioner
                .abort_member_provision(
                    &MemberRef::from_bridge_session_id(anchor.session_id.clone()),
                    &anchor.operation_id,
                    &anchor.reason,
                )
                .await;
            RetirementObservation::PendingAnchorAborted { anchor, result }
        });
    }

    async fn retirement_pending_anchor_aborted(
        &mut self,
        mut continuation: RetirementContinuation,
        anchor: PendingSpawnCleanupAnchor,
        result: Result<(), MobError>,
    ) {
        if self
            .pending_spawn_cleanup_anchors
            .get(&anchor.spawn_ticket)
            .is_none_or(|current| !Self::pending_anchor_matches(current, &anchor))
        {
            self.durable_uncertainty_fail_stop = true;
            self.finish_retirement(
                continuation,
                Err(MobError::Internal(
                    "pending provision abort lost its exact anchor".into(),
                )),
            )
            .await;
            return;
        }
        match result {
            Ok(()) => {
                self.pending_spawn_cleanup_anchors
                    .remove(&anchor.spawn_ticket);
            }
            Err(error) => continuation.pending_errors.push(format!(
                "{} ticket {}: pending spawn cleanup failed: {error}",
                anchor.agent_identity, anchor.spawn_ticket,
            )),
        }
        self.retirement_cleanup_pending_remote(continuation).await;
    }

    async fn retirement_cleanup_pending_remote(
        &mut self,
        mut continuation: RetirementContinuation,
    ) {
        let pending_carrier = continuation
            .pending_slot
            .as_ref()
            .and_then(|slot| slot.spawn.remote.as_ref())
            .map(|remote| remote.pending_carrier.clone());
        let Some(carrier) = pending_carrier else {
            self.retirement_finish_pending_slot(continuation).await;
            return;
        };
        let identity = continuation.entry.agent_identity.clone();
        let prepared = async {
            if let Err(error) = self.apply_dsl_input(
                mob_dsl::MobMachineInput::RecordMemberMaterializationFailure {
                    agent_identity: mob_dsl::AgentIdentity::from_domain(&identity),
                    kind: "materialize_canceled".into(),
                },
                "cancel_pending_spawns_for_member_remote",
            ) {
                tracing::error!(%error, "pending remote cancellation could not record materialization failure");
            }
            let abort = self.apply_dsl_input_collect_transition(
                mob_dsl::MobMachineInput::AbortSpawnExec { agent_identity: mob_dsl::AgentIdentity::from_domain(&identity) },
                "cancel_pending_spawns_for_member_remote",
            )?;
            let obligation = abort.effects().iter().find_map(|effect| match effect {
                mob_dsl::MobMachineEffect::PlacedCarrierCleanupRequested { obligation } => Some(obligation.clone()),
                _ => None,
            }).ok_or_else(|| MobError::Internal("pending remote abort produced no exact cleanup obligation".into()))?;
            let authority = self.authorize_placed_carrier_cleanup(&carrier, &obligation, "cancel_pending_spawns_for_member_remote")?;
            let release = super::super::placed_carrier_cleanup::prepare_placed_release(&self.definition.id, &carrier, &self.dsl_authority)?;
            let display = render_member_comms_name(self.definition.id.as_str(), &carrier.spec.profile_name, identity.as_str())?;
            let host_id = mob_dsl::HostId::from(carrier.host_id.to_string());
            if self.dsl_authority.state().host_binding_generations.get(&host_id).copied() == Some(carrier.host_binding_generation)
                && self.dsl_authority.state().host_bind_phase.get(&host_id) == Some(&mob_dsl::HostBindPhase::Bound)
            {
                let key = super::super::state::HostOrphanReleaseKey {
                    binding_incarnation: self.current_host_binding_incarnation(&host_id)?,
                    host_id,
                    agent_identity: mob_dsl::AgentIdentity::from(carrier.agent_identity.clone()),
                    generation: mob_dsl::Generation(carrier.generation),
                    fence_token: mob_dsl::FenceToken(carrier.fence_token),
                };
                if !Self::reserve_host_orphan_release(&mut self.orphan_release_reservations, &key) {
                    return Err(MobError::Internal("pending remote cancellation collided with an exact outstanding release".into()));
                }
                continuation.release_reservation = Some(key);
            }
            Ok::<_, MobError>((obligation, authority, release, display))
        }.await;
        let (obligation, authority, release, display) = match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                self.durable_uncertainty_fail_stop = true;
                self.finish_retirement(continuation, Err(error)).await;
                return;
            }
        };
        let provisioner = self.provisioner.clone();
        let bridge = self.supervisor_bridge.clone();
        let cleanup = Box::new(PendingRemoteCleanup {
            carrier,
            obligation,
            authority,
            display,
        });
        self.dispatch_retirement(continuation, "retire-pending-remote-release", async move {
            let result = super::super::placed_carrier_cleanup::realize_placed_release(
                release,
                provisioner.as_ref(),
                bridge,
            )
            .await;
            RetirementObservation::PendingRemoteReleased { cleanup, result }
        });
    }

    async fn retirement_pending_remote_released(
        &mut self,
        mut continuation: RetirementContinuation,
        cleanup: Box<PendingRemoteCleanup>,
        result: Result<(), MobError>,
    ) {
        if let Some(key) = continuation.release_reservation.take()
            && !Self::absorb_host_orphan_release_completion(
                &mut self.orphan_release_reservations,
                &key,
                result.is_ok(),
            )
        {
            self.durable_uncertainty_fail_stop = true;
            self.finish_retirement(
                continuation,
                Err(MobError::Internal(
                    "pending release lost exact reservation".into(),
                )),
            )
            .await;
            return;
        }
        if let Err(error) = result {
            self.durable_uncertainty_fail_stop = true;
            self.finish_retirement(continuation, Err(error)).await;
            return;
        }
        let PendingRemoteCleanup {
            carrier,
            obligation,
            authority,
            display,
        } = *cleanup;
        let provisioner = self.provisioner.clone();
        let metadata = self.runtime_metadata.clone();
        let mob = self.definition.id.clone();
        self.dispatch_retirement(
            continuation,
            "retire-pending-remote-operation-cleanup",
            async move {
                let result = async {
                    provisioner
                        .abort_placed_provision_operation(
                            &carrier.operation_owner_session_id,
                            &carrier.provision_operation_id,
                            &display,
                        )
                        .await?;
                    match metadata
                        .compare_and_delete_placed_spawn(&mob, &carrier, &authority)
                        .await?
                    {
                        crate::store::DeletePlacedSpawnResult::Deleted
                        | crate::store::DeletePlacedSpawnResult::AlreadyAbsent => Ok(()),
                        crate::store::DeletePlacedSpawnResult::Conflict => Err(MobError::Internal(
                            "pending remote carrier cleanup conflicted".into(),
                        )),
                    }
                }
                .await;
                RetirementObservation::PendingRemoteCleaned { obligation, result }
            },
        );
    }

    async fn retirement_finish_pending_slot(&mut self, mut continuation: RetirementContinuation) {
        let Some(slot) = continuation.pending_slot.take() else {
            self.durable_uncertainty_fail_stop = true;
            self.finish_retirement(
                continuation,
                Err(MobError::Internal(
                    "pending cancellation lost its owned slot".into(),
                )),
            )
            .await;
            return;
        };
        if let Some(origin) = slot.spawn.respawn_origin.as_ref()
            && let Err(error) = self
                .durably_abandon_respawn_topology_if_terminal_exact(
                    &continuation.entry.agent_identity,
                    origin,
                )
                .await
        {
            continuation.pending_slot = Some(slot);
            self.durable_uncertainty_fail_stop = true;
            self.respawn_topology_reply_withheld = true;
            self.finish_retirement(continuation, Err(error)).await;
            return;
        }
        (*slot).fail(&format!(
            "spawn canceled for '{}': retire command received",
            continuation.entry.agent_identity
        ));
        self.retirement_next_pending_slot(continuation).await;
    }

    fn retirement_placement_fence(
        &self,
        identity: &AgentIdentity,
    ) -> Option<RetirementPlacementFence> {
        let identity = mob_dsl::AgentIdentity::from_domain(identity);
        let state = self.dsl_authority.state();
        state
            .member_placement
            .get(&identity)
            .map(|host| RetirementPlacementFence {
                host: host.clone(),
                session: state.member_session_bindings.get(&identity).cloned(),
                binding_generation: state
                    .current_placed_spawn_host_binding_generations
                    .get(&identity)
                    .copied(),
                binding_incarnation: self.host_binding_incarnations.get(host).copied(),
            })
    }

    async fn retirement_start_external_cleanup(
        &mut self,
        mut continuation: RetirementContinuation,
    ) {
        if continuation.preserve_topology {
            self.retirement_prepare_archive(continuation).await;
            return;
        }
        continuation.external_edges =
            self.machine_external_peer_edges_for(&continuation.entry.agent_identity);
        if continuation.external_edges.is_empty() {
            self.retirement_finish_machine_wiring(continuation).await;
            return;
        }
        if super::super::member_runtime_is_host_owned(
            self.dsl_authority.state(),
            &continuation.entry.agent_identity,
        ) {
            let error = MobError::WiringError(format!(
                "retire external-peer cleanup is unsupported for placed member '{}'",
                continuation.entry.agent_identity,
            ));
            self.finish_retirement(continuation, Err(error)).await;
            return;
        }
        let provisioner = self.provisioner.clone();
        let member = continuation.entry.member_ref.clone();
        self.dispatch_retirement(
            continuation,
            "retire-external-comms-observation",
            async move {
                RetirementObservation::ExternalCommsObserved(
                    provisioner.comms_runtime(&member).await,
                )
            },
        );
    }

    async fn retirement_finish_machine_wiring(&mut self, continuation: RetirementContinuation) {
        let result = match continuation.disposal.as_ref() {
            Some(ctx) => self.cleanup_retired_member_machine_wiring(ctx),
            None => Err(MobError::Internal(
                "retirement lost its disposal context".into(),
            )),
        };
        match result {
            Ok(()) => self.retirement_prepare_archive(continuation).await,
            Err(error) => self.finish_retirement(continuation, Err(error)).await,
        }
    }

    async fn retirement_next_external_edge(&mut self, mut continuation: RetirementContinuation) {
        let Some(edge) = continuation.external_edges.pop() else {
            self.retirement_finish_machine_wiring(continuation).await;
            return;
        };
        let key = Self::external_peer_key_for_edge(&edge);
        if let Err(error) = meerkat_core::comms::PeerName::new(edge.endpoint.name.0.clone()) {
            self.finish_retirement(continuation, Err(MobError::WiringError(error.to_string())))
                .await;
            return;
        }
        let prior = match Self::trusted_peer_descriptor_from_machine_endpoint(&edge.endpoint) {
            Ok(prior) => prior,
            Err(error) => {
                self.finish_retirement(continuation, Err(error)).await;
                return;
            }
        };
        if let Some(comms) = continuation.external_comms.clone() {
            let authority =
                match self.apply_cleanup_retiring_external_peer(&continuation.entry, &key, &edge) {
                    Ok(authority) => authority,
                    Err(error) => {
                        self.finish_retirement(continuation, Err(error)).await;
                        return;
                    }
                };
            let owner = self.dsl_authority.generated_authority_owner_token();
            self.dispatch_retirement(continuation, "retire-external-trust-remove", async move {
                let result = Self::apply_trusted_peer_remove_with_owner_token(
                    comms.as_ref(),
                    Self::trusted_peer_removal_key(&prior),
                    authority,
                    &owner,
                )
                .await
                .map(|_| ())
                .map_err(MobError::from);
                RetirementObservation::ExternalTrustRemoved { edge, result }
            });
        } else {
            match self.apply_cleanup_retiring_external_peer_observed_absent(
                &continuation.entry,
                &key,
                &edge,
            ) {
                Ok(()) => {
                    self.retirement_external_edge_removed(continuation, edge, Ok(()))
                        .await;
                }
                Err(error) => self.finish_retirement(continuation, Err(error)).await,
            }
        }
    }

    async fn retirement_external_edge_removed(
        &mut self,
        continuation: RetirementContinuation,
        edge: mob_dsl::ExternalPeerEdge,
        result: Result<(), MobError>,
    ) {
        let key = Self::external_peer_key_for_edge(&edge);
        if let Err(error) = result {
            let error = match self.apply_restore_retiring_external_peer(
                &continuation.entry,
                &key,
                &edge,
            ) {
                Ok(_) => error,
                Err(rollback) => MobError::WiringError(format!(
                    "retiring external trust removal failed: {error}; machine rollback failed: {rollback}",
                )),
            };
            self.finish_retirement(continuation, Err(error)).await;
            return;
        }
        let peer_name = match meerkat_core::comms::PeerName::new(edge.endpoint.name.0.clone()) {
            Ok(name) => name,
            Err(error) => {
                self.finish_retirement(continuation, Err(MobError::WiringError(error.to_string())))
                    .await;
                return;
            }
        };
        let stored = self
            .events
            .append(NewMobEvent {
                mob_id: self.definition.id.clone(),
                timestamp: None,
                kind: MobEventKind::ExternalPeerUnwired {
                    local: continuation.entry.agent_identity.clone(),
                    peer_name,
                },
            })
            .await;
        match stored {
            Ok(stored) => {
                self.roster.write().await.apply_event(&stored);
                Box::pin(self.retirement_next_external_edge(continuation)).await;
            }
            Err(error) => {
                let original = MobError::from(error);
                if let Some(comms) = continuation.external_comms.clone() {
                    let rollback = self
                        .apply_restore_retiring_external_peer(&continuation.entry, &key, &edge)
                        .and_then(|handoff| handoff.external_authority().cloned());
                    let authority = match rollback {
                        Ok(authority) => authority,
                        Err(error) => {
                            self.finish_retirement(continuation, Err(error)).await;
                            return;
                        }
                    };
                    let prior =
                        match Self::trusted_peer_descriptor_from_machine_endpoint(&edge.endpoint) {
                            Ok(prior) => prior,
                            Err(error) => {
                                self.finish_retirement(continuation, Err(error)).await;
                                return;
                            }
                        };
                    let owner = self.dsl_authority.generated_authority_owner_token();
                    self.dispatch_retirement(
                        continuation,
                        "retire-external-trust-rollback",
                        async move {
                            let result = Self::apply_trusted_peer_add_with_owner_token(
                                comms.as_ref(),
                                prior,
                                authority,
                                &owner,
                            )
                            .await
                            .map_err(MobError::from);
                            RetirementObservation::ExternalTrustRestored { original, result }
                        },
                    );
                } else {
                    let result = self.apply_restore_retiring_external_peer_observed_absent(
                        &continuation.entry,
                        &key,
                        &edge,
                    );
                    self.finish_retirement(continuation, Err(result.err().unwrap_or(original)))
                        .await;
                }
            }
        }
    }

    pub(super) async fn retirement_endpoint_observation(
        &self,
        entry: &RosterEntry,
        context: &'static str,
        observed: &mut Option<RetirementEndpointObservations>,
    ) -> Result<WiringEndpoint, MobError> {
        match observed {
            Some(observed) => observed
                .endpoints
                .remove(&entry.agent_identity)
                .ok_or_else(|| {
                    MobError::Internal(format!("{context}: exact endpoint observation is missing"))
                })?,
            None => self.resolve_wiring_endpoint(entry, context).await,
        }
    }

    async fn retirement_observe_endpoints(
        &mut self,
        mut continuation: RetirementContinuation,
        notification: bool,
    ) {
        let mut identities =
            self.machine_member_wired_peer_identities_for(&continuation.entry.agent_identity);
        if let Some(rollback) = &continuation.rollback
            && matches!(rollback.phase, SpawnRollbackPhase::Endpoints)
        {
            identities.extend(rollback.cleanup_peers.iter().cloned());
        }
        let entries = {
            let roster = self.roster.read().await;
            std::iter::once(continuation.entry.clone())
                .chain(
                    identities
                        .iter()
                        .filter_map(|identity| roster.get(identity).cloned()),
                )
                .collect::<Vec<_>>()
        };
        let mut plans = Vec::new();
        let mut retained_sources = Vec::new();
        if continuation
            .rollback
            .as_ref()
            .is_none_or(|rollback| matches!(rollback.phase, SpawnRollbackPhase::Endpoints))
        {
            continuation.participants = entries
                .iter()
                .map(MemberIncarnationFence::from_entry)
                .collect();
        }
        if let Some(rollback) = continuation.rollback.as_mut()
            && matches!(rollback.phase, SpawnRollbackPhase::Endpoints)
        {
            rollback.placement_fences = entries
                .iter()
                .map(|entry| {
                    (
                        entry.agent_identity.clone(),
                        self.retirement_placement_fence(&entry.agent_identity),
                    )
                })
                .collect();
        }
        for entry in entries {
            let identity = entry.agent_identity.clone();
            let plan = (|| {
                let placement = self.ensure_placed_carrier_binding_active(
                    &identity,
                    "retirement endpoint observation",
                )?;
                let comms_name = self.comms_name_for(&entry)?;
                let member_ref = self
                    .machine_member_ref_for_behavior(&entry, "retirement endpoint observation")?;
                if let Some(host) = placement {
                    let spec = self
                        .machine_member_peer_spec_for(&identity, "retirement endpoint observation")?
                        .ok_or_else(|| {
                            MobError::WiringError("placed retirement endpoint missing".into())
                        })?;
                    return Ok(RetirementEndpointPlan::Ready(WiringEndpoint::Placed {
                        identity: identity.clone(),
                        host,
                        spec,
                    }));
                }
                Ok(RetirementEndpointPlan::Local {
                    entry: entry.clone(),
                    member_ref,
                    comms_name,
                })
            })()
            .unwrap_or_else(RetirementEndpointPlan::Refused);
            if identities.contains(&identity)
                && !super::super::member_runtime_is_host_owned(
                    self.dsl_authority.state(),
                    &identity,
                )
            {
                retained_sources.push((identity.clone(), entry.member_ref));
            }
            plans.push((identity, plan));
        }
        let provisioner = self.provisioner.clone();
        let bridge = self.supervisor_bridge.clone();
        let expected_name = self.comms_name_for(&continuation.entry);
        let rollback_profile = continuation
            .rollback
            .as_ref()
            .filter(|rollback| matches!(rollback.phase, SpawnRollbackPhase::Endpoints))
            .map(|_| {
                (
                    self.definition.clone(),
                    self.realm_profile_store.clone(),
                    continuation.entry.role.clone(),
                )
            });
        self.dispatch_retirement(continuation, "retire-endpoint-observations", async move {
            let mut endpoints = BTreeMap::new();
            for (identity, plan) in plans {
                let endpoint = match plan {
                    RetirementEndpointPlan::Ready(endpoint) => Ok(endpoint),
                    RetirementEndpointPlan::Refused(error) => Err(error),
                    RetirementEndpointPlan::Local { entry, member_ref, comms_name } => async {
                        if let Some(comms) = provisioner.comms_runtime(&member_ref).await {
                            let public_key = comms.public_key().ok_or_else(|| MobError::WiringError(format!(
                                "retirement endpoint requires public key for '{}'", entry.agent_identity,
                            )))?;
                            let mut spec = provisioner.trusted_peer_spec(&member_ref, &comms_name, &public_key).await?;
                            if let Some(address) = comms.advertised_address() {
                                spec.address = PeerAddress::parse(&address)
                                    .map_err(|error| MobError::WiringError(error.to_string()))?;
                            }
                            return Ok(WiringEndpoint::Local { entry: Box::new(entry), comms, spec, comms_name });
                        }
                        let binding = Self::runtime_binding_for_member_ref(&member_ref).ok_or_else(|| {
                            MobError::WiringError(format!("retirement requires comms runtime for '{}'", entry.agent_identity))
                        })?;
                        let spec = Self::peer_only_spec_for_binding(&binding, "retirement endpoint observation")?;
                        Ok(WiringEndpoint::PeerOnly { spec, binding })
                    }.await,
                };
                endpoints.insert(identity, endpoint);
            }
            let retained_spec = async {
                let expected_name = expected_name?;
                let mut retained: Option<TrustedPeerDescriptor> = None;
                for (identity, member_ref) in retained_sources {
                    let Some(comms) = provisioner.comms_runtime(&member_ref).await else { continue; };
                    let peers = comms.trusted_peer_projection_snapshot_for_source(
                        meerkat_core::comms::GeneratedCommsTrustAuthoritySourceKind::MobMachineMemberTrustWiring,
                    ).await.map_err(|error| MobError::WiringError(format!(
                        "retirement retained peer observation for '{identity}' failed: {error}",
                    )))?;
                    for peer in peers {
                        if peer.name.as_str() == expected_name {
                            if retained.as_ref().is_some_and(|previous| previous.peer_id != peer.peer_id) {
                                return Err(MobError::WiringError("retained retirement peer descriptor disagrees across wired peers".into()));
                            }
                            retained = Some(peer);
                        }
                    }
                }
                Ok(retained)
            }.await;
            let observed = RetirementEndpointObservations {
                endpoints, retained_spec: Some(retained_spec), supervisor_comms: bridge.runtime_core().await,
            };
            if let Some((definition, store, profile)) = rollback_profile {
                let description = definition.resolve_profile(&profile, store.as_ref()).await
                    .map(|profile| profile.peer_description).unwrap_or_default();
                RetirementObservation::SpawnRollback(SpawnRollbackObservation::Endpoints {
                    observed,
                    description,
                })
            } else if notification {
                RetirementObservation::NotificationEndpoints(observed)
            } else {
                RetirementObservation::TrustEndpoints(observed)
            }
        });
    }

    pub(super) fn retirement_control_is_pending(&self, command: &MobCommand) -> bool {
        let activation = self.spawn_activation_quiescence();
        if let MobCommand::Retire { agent_identity, .. }
        | MobCommand::Respawn { agent_identity, .. } = command
            && activation.members.contains(agent_identity)
        {
            return true;
        }
        if matches!(
            command,
            MobCommand::Complete { .. } | MobCommand::Reset { .. } | MobCommand::RetireAll { .. }
        ) && (!self.member_admission_lanes.is_empty() || activation.total != 0)
        {
            return true;
        }
        if self.retirements.is_empty() && self.retirement_batch.is_none() {
            return false;
        }
        if self.retirement_batch.is_some()
            && matches!(
                command,
                MobCommand::Spawn { .. }
                    | MobCommand::SpawnAttachedForkedParticipant { .. }
                    | MobCommand::RunFlow { .. }
                    | MobCommand::PreviewRunFlowAdmission { .. }
            )
        {
            return true;
        }
        match command {
            MobCommand::Stop { .. }
            | MobCommand::Complete { .. }
            | MobCommand::Reset { .. }
            | MobCommand::RetireAll { .. }
            | MobCommand::Destroy { .. }
            | MobCommand::Shutdown { .. }
            | MobCommand::ResumeLifecycle { .. }
            | MobCommand::BindHost { .. }
            | MobCommand::RevokeHost { .. }
            | MobCommand::RotateSupervisor { .. } => true,
            MobCommand::Retire { agent_identity, .. }
            | MobCommand::Respawn { agent_identity, .. }
            | MobCommand::ReloadMemberRegistration { agent_identity, .. } => {
                self.retirements.contains_key(agent_identity)
            }
            MobCommand::SubmitWork { payload, .. } => {
                self.retirements.contains_key(&payload.runtime_id.identity)
            }
            MobCommand::Spawn { spec, .. } => self.retirements.contains_key(&spec.identity),
            _ => false,
        }
    }

    pub(super) async fn begin_retirement_batch(
        &mut self,
        context: &'static str,
        after: RetirementAfter,
    ) {
        if self.retirement_batch.is_some() {
            Self::answer_retirement_after(
                after,
                Err(MobError::LifecycleOperationPending {
                    intent: "member retirement batch".into(),
                }),
            );
            return;
        }
        let prepared = async {
            let admission = self.prepare_command_admission(
                mob_dsl::MobMachineInput::RetireAll,
                MobState::Running,
                context,
            )?;
            self.commit_prepared_dsl_input(admission)?;
            self.ensure_pending_spawn_alignment("retire_all_members preflight")?;
            Ok(self
                .roster
                .read()
                .await
                .list_all()
                .map(|entry| entry.agent_identity.clone())
                .collect::<Vec<_>>())
        }
        .await;
        let identities = match prepared {
            Ok(identities) => identities,
            Err(error) => {
                Self::answer_retirement_after(after, Err(error));
                return;
            }
        };
        self.retirement_batch = Some(RetirementBatch {
            context,
            after,
            pending_members: identities.into(),
            active_member: None,
            settled_members: Vec::new(),
        });
    }

    fn answer_retirement_after(after: RetirementAfter, result: Result<(), MobError>) {
        match after {
            RetirementAfter::RetireAll(reply)
            | RetirementAfter::Complete(reply)
            | RetirementAfter::Reset { reply, .. } => {
                let _ = reply.send(result);
            }
        }
    }

    /// Drive from the outer actor wake, after member-effect ACKs have been
    /// consumed. Each member's physical receipt precedes the next member's
    /// prepare, so retiring one endpoint cannot supersede an in-flight peer's
    /// cleanup plan. The batch owns receivers, never a self-blocking effect
    /// ticket around the final global lifecycle drain.
    pub(super) async fn continue_retirement_batch_after_settlement(&mut self) -> bool {
        if self.durable_uncertainty_fail_stop {
            return false;
        }
        let Some(mut batch) = self.retirement_batch.take() else {
            return false;
        };
        if let Some((identity, mut receipt)) = batch.active_member.take() {
            let result = match receipt.try_recv() {
                Ok(result) => result,
                Err(oneshot::error::TryRecvError::Empty) => {
                    batch.active_member = Some((identity, receipt));
                    self.retirement_batch = Some(batch);
                    return false;
                }
                Err(oneshot::error::TryRecvError::Closed) => Err(MobError::ActorReplyChannelClosed),
            };
            batch.settled_members.push((identity, result));
        }
        if let Some(identity) = batch.pending_members.pop_front() {
            let (reply, receipt) = oneshot::channel();
            batch.active_member = Some((identity.clone(), receipt));
            self.retirement_batch = Some(batch);
            self.start_retirement(
                identity,
                Instant::now() + super::super::provisioner::MEMBER_RETIRE_TOTAL_TIMEOUT,
                None,
                RetirementReply::BatchRetire(reply),
            )
            .await;
            return true;
        }
        let results = std::mem::take(&mut batch.settled_members);
        self.finish_retirement_batch(batch, results).await;
        true
    }

    async fn finish_retirement_batch(
        &mut self,
        batch: RetirementBatch,
        results: Vec<(AgentIdentity, Result<(), MobError>)>,
    ) {
        let result = async {
            let mut failures = Vec::new();
            let mut starts = Vec::new();
            for (identity, result) in results {
                if let Err(error) = result {
                    let generation = self
                        .roster
                        .read()
                        .await
                        .get(&identity)
                        .map(|entry| entry.generation);
                    let Some(generation) = generation else {
                        continue;
                    };
                    match self
                        .retirement_started_event_exists(&identity, generation)
                        .await
                    {
                        Ok(true) => {}
                        Ok(false) => {
                            starts.push(format!("{identity}: retirement start is not durable"));
                        }
                        Err(error) => starts.push(format!(
                            "{identity}: retirement start observation failed: {error}"
                        )),
                    }
                    failures.push(format!("{identity}: {error}"));
                }
            }
            if !starts.is_empty() {
                return Err(MobError::Internal(format!(
                    "{} aborted before pending-spawn drain: {}",
                    batch.context,
                    starts.join("; ")
                )));
            }
            self.fail_all_pending_spawns(&format!(
                "{}: draining pending spawns after member retirement starts",
                batch.context
            ))
            .await?;
            self.ensure_pending_spawn_alignment("retire_all_members after pending drain")?;
            if !failures.is_empty() {
                return Err(MobError::Internal(format!(
                    "{} aborted: {}",
                    batch.context,
                    failures.join("; ")
                )));
            }
            match &batch.after {
                RetirementAfter::RetireAll(_) => {
                    self.drive_placed_completion_lifecycle_cleanup(
                        None,
                        false,
                        Some(mob_dsl::PlacedCompletionLifecycleIntentKind::RetireAll),
                    )
                    .await?;
                    self.end_placed_completion_lifecycle_quiesce(
                        mob_dsl::PlacedCompletionLifecycleIntentKind::RetireAll,
                    )
                    .await
                }
                RetirementAfter::Complete(_) => self.handle_complete().await,
                RetirementAfter::Reset { prior_state, .. } => self.handle_reset(*prior_state).await,
            }
        }
        .await;
        if result.is_err() && matches!(&batch.after, RetirementAfter::Reset { .. }) {
            if self.state() == MobState::Stopped {
                self.provisioner.cancel_all_checkpointers().await;
            } else {
                self.fail_reset_to_stopped().await;
            }
        }
        if !self.respawn_topology_reply_withheld && !self.durable_uncertainty_fail_stop {
            Self::answer_retirement_after(batch.after, result);
        }
    }

    pub(super) async fn start_respawn_retirement(
        &mut self,
        identity: AgentIdentity,
        mut replacement: super::super::handle::SpawnMemberSpec,
        snapshot: RespawnSnapshot,
    ) -> RespawnProgress {
        if replacement.placement.is_none() {
            replacement.binding = Some(snapshot.binding.clone());
        }
        let (reply, result) = oneshot::channel();
        #[cfg(feature = "runtime-adapter")]
        let operation_owner = if replacement.placement.is_none()
            && matches!(&snapshot.binding, crate::RuntimeBinding::External { .. })
        {
            match self
                .generated_peer_only_operation_owner_context(
                    &identity,
                    &snapshot.binding,
                    "respawn_peer_only_operation_owner",
                )
                .await
            {
                Ok(owner) => Some(owner),
                Err(error) => {
                    let _ = reply.send(Err(super::super::handle::MobRespawnError::from(error)));
                    return RespawnProgress::DeferredRetirement { result };
                }
            }
        } else {
            None
        };
        #[cfg(not(feature = "runtime-adapter"))]
        let operation_owner = None;
        self.start_retirement(
            identity,
            Instant::now() + super::super::provisioner::MEMBER_RETIRE_TOTAL_TIMEOUT,
            None,
            RetirementReply::Respawn {
                snapshot,
                replacement: Box::new(replacement),
                operation_owner,
                reply,
            },
        )
        .await;
        RespawnProgress::DeferredRetirement { result }
    }

    async fn retirement_notifications(
        &mut self,
        mut continuation: RetirementContinuation,
        mut observed: RetirementEndpointObservations,
    ) {
        let Some(ctx) = continuation.disposal.as_ref() else {
            self.finish_retirement(
                continuation,
                Err(MobError::Internal(
                    "retirement lost disposal context".into(),
                )),
            )
            .await;
            return;
        };
        let Some(retiring_spec) = ctx.retiring_spec.clone() else {
            self.retirement_next_remote_edge(continuation).await;
            return;
        };
        let retiring_comms = ctx.retiring_comms.clone();
        let retired = ctx.entry.clone();
        let owner_token = self.dsl_authority.generated_authority_owner_token();
        let mut jobs = Vec::new();
        let mut notice_targets = Vec::new();
        for identity in &ctx.machine_wired_peer_identities {
            let entry = self.roster.read().await.get(identity).cloned();
            let Some(entry) = entry else {
                continue;
            };
            let endpoint = observed.endpoints.remove(identity).unwrap_or_else(|| {
                Err(MobError::Internal(
                    "retirement notification endpoint observation missing".into(),
                ))
            });
            match endpoint {
                Ok(WiringEndpoint::Local { comms, spec, .. }) => {
                    notice_targets.push(spec.clone());
                    let Some(authority) = ctx.trust_unwire_authority_by_peer.get(identity).cloned()
                    else {
                        let error = MobError::RetirementTopologyIncomplete(format!(
                            "missing generated retire trust handoff for '{identity}'"
                        ));
                        self.finish_retirement(continuation, Err(error)).await;
                        return;
                    };
                    jobs.push((
                        comms,
                        authority,
                        ctx.historical_trust_unwire_authorities_by_peer
                            .get(identity)
                            .cloned()
                            .unwrap_or_default(),
                    ));
                    if Self::runtime_binding_for_entry(&ctx.entry).is_some() {
                        continuation.remote_peers.push(identity.clone());
                    }
                }
                Ok(WiringEndpoint::PeerOnly { spec, .. }) => {
                    notice_targets.push(spec);
                    continuation.remote_peers.push(identity.clone());
                }
                Ok(WiringEndpoint::Placed { .. }) => {}
                Err(error) if Self::runtime_binding_for_entry(&entry).is_none() => {
                    let retained = self
                        .machine_member_peer_spec_for(
                            identity,
                            "dispose_notify_peers retained endpoint",
                        )
                        .and_then(|spec| match spec {
                            Some(spec) => Ok(Some(spec)),
                            None => self.roster_member_peer_spec_for(
                                &entry,
                                "dispose_notify_peers retained endpoint",
                            ),
                        });
                    match retained {
                        Ok(Some(_)) => {
                            if Self::runtime_binding_for_entry(&ctx.entry).is_some()
                                && !ctx.preserve_machine_topology
                            {
                                continuation.remote_peers.push(identity.clone());
                            }
                        }
                        _ => {
                            self.finish_retirement(
                                continuation,
                                Err(MobError::RetirementTopologyIncomplete(error.to_string())),
                            )
                            .await;
                            return;
                        }
                    }
                }
                Err(error) => {
                    self.finish_retirement(
                        continuation,
                        Err(MobError::RetirementTopologyIncomplete(error.to_string())),
                    )
                    .await;
                    return;
                }
            }
        }
        let kickoff_notices = std::mem::take(&mut continuation.kickoff_notices);
        self.dispatch_retirement(continuation, "retire-local-trust-cleanup", async move {
            if let Some(sender) = retiring_comms.as_ref() {
                futures::stream::iter(notice_targets)
                    .for_each_concurrent(RETIRE_LOCAL_TRUST_CLEANUP_CONCURRENCY, |target| {
                        let retired = &retired;
                        let retiring_spec = &retiring_spec;
                        let kickoff_notices = &kickoff_notices;
                        async move {
                            for intent in kickoff_notices {
                                if let Err(error) = Self::notify_peer_event_with_spec_owned(
                                    intent,
                                    &target,
                                    &retired.agent_identity,
                                    retired,
                                    retiring_spec,
                                    sender,
                                )
                                .await
                                {
                                    tracing::warn!(%error, "advisory kickoff cleanup notice failed");
                                }
                            }
                            if let Err(error) = Self::notify_peer_retired_bounded(
                                &target,
                                &retired.agent_identity,
                                retired,
                                retiring_spec,
                                sender,
                            )
                            .await
                            {
                                tracing::warn!(%error, "advisory retirement notice failed");
                            }
                        }
                    })
                    .await;
            }
            let mut effects: Vec<ActorCommandFuture<'static, Result<(), MobError>>> =
                Vec::with_capacity(jobs.len());
            for (comms, authority, historical) in jobs {
                let retiring_spec = retiring_spec.clone();
                let owner_token = owner_token.clone();
                effects.push(Box::pin(async move {
                    for (peer, authority) in historical {
                        Self::apply_trusted_peer_remove_with_owner_token(
                            comms.as_ref(),
                            peer,
                            authority,
                            &owner_token,
                        )
                        .await
                        .map_err(|error| {
                            MobError::RetirementTopologyIncomplete(error.to_string())
                        })?;
                    }
                    Self::apply_trusted_peer_remove_with_owner_token(
                        comms.as_ref(),
                        Self::trusted_peer_removal_key(&retiring_spec),
                        authority,
                        &owner_token,
                    )
                    .await
                    .map_err(|error| MobError::RetirementTopologyIncomplete(error.to_string()))?;
                    Ok(())
                }));
            }
            let outcomes = futures::stream::iter(effects)
                .buffer_unordered(RETIRE_LOCAL_TRUST_CLEANUP_CONCURRENCY)
                .collect::<Vec<Result<(), MobError>>>()
                .await;
            RetirementObservation::TrustRemoved(outcomes.into_iter().collect())
        });
    }

    fn retirement_unwire_error(error: MobError) -> MobError {
        match error {
            MobError::RetirementTopologyIncomplete(_) => error,
            error => MobError::RetirementTopologyIncomplete(error.to_string()),
        }
    }

    async fn retirement_next_remote_edge(&mut self, mut continuation: RetirementContinuation) {
        while let Some(peer) = continuation.remote_peers.pop() {
            let preparation = match self
                .prepare_member_unwire_for_retirement(
                    continuation.entry.agent_identity.clone(),
                    peer,
                    continuation.preserve_topology,
                )
                .await
                .map_err(Self::retirement_unwire_error)
            {
                Ok(preparation) => preparation,
                Err(error) => {
                    self.finish_retirement(continuation, Err(error)).await;
                    return;
                }
            };
            if let Some(prepared) = preparation.into_prepared() {
                self.dispatch_retirement(continuation, "retire-remote-trust-cleanup", async move {
                    RetirementObservation::RemoteEdge(prepared.realize().await)
                });
                return;
            }
        }
        self.retirement_start_external_cleanup(continuation).await;
    }

    async fn retirement_post_archive(
        &mut self,
        mut continuation: RetirementContinuation,
        disposal: mob_dsl::MemberSessionDisposal,
    ) {
        continuation.archive_disposal = Some(disposal);
        if continuation.terminal_published {
            let identity = mob_dsl::AgentIdentity::from_domain(&continuation.entry.agent_identity);
            let open = self
                .dsl_authority
                .state()
                .pending_placed_carrier_cleanup
                .iter()
                .any(|obligation| obligation.agent_identity == identity);
            let record = self
                .runtime_metadata
                .load_placed_spawn(
                    &self.definition.id,
                    continuation.entry.agent_identity.as_str(),
                )
                .await;
            let record = match record {
                Ok(record) => record,
                Err(error) => {
                    self.finish_retirement(continuation, Err(error.into()))
                        .await;
                    return;
                }
            };
            if record.is_some_and(|record| {
                record.generation == continuation.entry.generation.get()
                    && matches!(
                        record.phase,
                        crate::store::PlacedSpawnCarrierPhase::Committed(_)
                    )
            }) && !open
            {
                let ctx = Self::disposal_context_from_entry(
                    &continuation.entry.agent_identity,
                    &continuation.entry,
                    RetireTrustCleanupPlan::empty(),
                    false,
                );
                if let Err(error) = self
                    .observe_member_retirement_archived(
                        &ctx,
                        mob_dsl::MemberSessionDisposal::RuntimeReleasedOnlyHostOwned,
                    )
                    .await
                {
                    self.finish_retirement(continuation, Err(error)).await;
                    return;
                }
            }
            self.retirement_terminal_carrier_cleanup(continuation).await;
            return;
        }
        let result = async {
            let identity = mob_dsl::AgentIdentity::from_domain(&continuation.entry.agent_identity);
            let placed = self
                .dsl_authority
                .state()
                .member_placement
                .get(&identity)
                .cloned();
            if let Some(host) = placed {
                let session = self
                    .dsl_authority
                    .state()
                    .member_session_bindings
                    .get(&identity)
                    .cloned()
                    .ok_or_else(|| {
                        MobError::Internal("placed retirement lost exact session".into())
                    })?;
                let record = self
                    .runtime_metadata
                    .load_placed_spawn(
                        &self.definition.id,
                        continuation.entry.agent_identity.as_str(),
                    )
                    .await?
                    .ok_or_else(|| {
                        MobError::Internal("placed retirement lost exact carrier".into())
                    })?;
                self.ensure_exact_structural_event(MobEventKind::RemoteMemberReleaseConfirmed {
                    agent_identity: continuation.entry.agent_identity.clone(),
                    host_id: host.0.clone(),
                    member_session_id: session.0.clone(),
                    generation: continuation.entry.generation,
                    fence_token: continuation.entry.fence_token,
                })
                .await?;
                self.dispose_remote_turn_custody_after_host_release(
                    &continuation.entry.agent_identity,
                    &host,
                    record.host_binding_generation,
                    &session,
                    continuation.entry.generation.get(),
                    continuation.entry.fence_token.get(),
                )
                .await?;
            } else if Self::runtime_binding_for_entry(&continuation.entry).is_some() {
                self.record_remote_member_runtime_retired(&continuation.entry)
                    .await?;
            }
            Ok(())
        }
        .await;
        if let Err(error) = result {
            self.finish_retirement(continuation, Err(error)).await;
            return;
        }
        #[cfg(not(target_arch = "wasm32"))]
        if continuation.preserve_topology
            && !super::super::member_runtime_is_host_owned(
                self.dsl_authority.state(),
                &continuation.entry.agent_identity,
            )
        {
            let endpoint = match self.machine_member_peer_spec_for(
                &continuation.entry.agent_identity,
                "retiring reverse-lane endpoint",
            ) {
                Ok(endpoint) => endpoint,
                Err(error) => {
                    self.finish_retirement(continuation, Err(error)).await;
                    return;
                }
            };
            if let Some(endpoint) = endpoint
                && let Some(mut state) = self.controlling_acceptor.take()
            {
                self.dispatch_retirement(
                    continuation,
                    "retire-acceptor-registration",
                    async move {
                        let result = state
                            .remove_registration(&meerkat_comms::PubKey::new(endpoint.pubkey))
                            .await;
                        RetirementObservation::AcceptorRemoved { state, result }
                    },
                );
                return;
            }
        }
        self.retirement_prepare_supervisor(continuation).await;
    }

    async fn retirement_prepare_supervisor(&mut self, continuation: RetirementContinuation) {
        if super::super::member_runtime_is_host_owned(
            self.dsl_authority.state(),
            &continuation.entry.agent_identity,
        ) {
            self.retirement_release_attachments(continuation).await;
            return;
        }
        let Some(binding) = Self::runtime_binding_for_entry(&continuation.entry) else {
            self.retirement_release_attachments(continuation).await;
            return;
        };
        if !self.remote_runtime_retired_for_entry(&continuation.entry) {
            self.finish_retirement(
                continuation,
                Err(MobError::Internal(
                    "peer-only retirement has no durable remote-runtime checkpoint".into(),
                )),
            )
            .await;
            return;
        }
        let peer = match Self::peer_only_spec_for_binding(&binding, "retirement revoke supervisor")
        {
            Ok(peer) => peer,
            Err(error) => {
                self.finish_retirement(continuation, Err(error)).await;
                return;
            }
        };
        if self.remote_supervisor_revoked_for_entry(&continuation.entry) {
            self.retirement_remove_supervisor_trust(continuation, peer);
            return;
        }
        if let Err(error) =
            self.record_pending_recipient_trust_obligation(&peer, "send_bridge_command_typed")
        {
            self.finish_retirement(continuation, Err(error)).await;
            return;
        }
        let bridge = self.supervisor_bridge.clone();
        self.dispatch_retirement(continuation, "retire-supervisor-revoke", async move {
            let install = match bridge.trust_recipient(&peer).await {
                Ok(install) => install,
                Err(error) => {
                    return RetirementObservation::SupervisorRevoked {
                        peer,
                        result: RetirementRevokeObservation::InstallUncertain(error),
                    };
                }
            };
            let result = async {
                let authority = bridge.authority().await;
                let spec = bridge
                    .supervisor_spec_for_authority_and_recipient(&authority, &peer)
                    .await?;
                let command = super::super::bridge_protocol::BridgeCommand::RevokeSupervisor(
                    super::super::bridge_protocol::BridgeSupervisorPayload {
                        supervisor: spec.into(),
                        epoch: authority.epoch,
                        protocol_version: authority.protocol_version,
                    },
                );
                let value = bridge
                    .send_bridge_command(&peer, &command, std::time::Duration::from_secs(5))
                    .await?;
                if let Some(rejection) =
                    Self::bridge_rejection_reply(command.protocol_version(), &value)
                {
                    return Err(Self::bridge_rejection_error(rejection));
                }
                super::super::bridge_protocol::decode_bridge_payload::<
                    super::super::bridge_protocol::BridgeAck,
                >(&command, value, "retirement supervisor revoke")
                .map(|_| ())
            }
            .await;
            let result = match result {
                Ok(()) => RetirementRevokeObservation::Confirmed,
                Err(error) => {
                    let rollback = if Self::recipient_trust_was_newly_installed(install) {
                        bridge.untrust_recipient(&peer).await
                    } else {
                        Ok(())
                    };
                    RetirementRevokeObservation::Refused { error, rollback }
                }
            };
            RetirementObservation::SupervisorRevoked { peer, result }
        });
    }

    async fn retirement_supervisor_settled(
        &mut self,
        continuation: RetirementContinuation,
        peer: TrustedPeerDescriptor,
        result: RetirementRevokeObservation,
    ) {
        let result = match result {
            RetirementRevokeObservation::Confirmed => self
                .resolve_pending_recipient_trust_obligation(
                    &peer,
                    "send_bridge_command_typed confirmed",
                ),
            RetirementRevokeObservation::Refused {
                error,
                rollback: Ok(()),
            } => {
                match self.rollback_pending_recipient_trust_obligation(
                    &peer,
                    "rollback_supervisor_recipient_trust",
                ) {
                    Ok(()) if Self::expected_revoke_cleanup_failure(&error).is_some() => Ok(()),
                    Ok(()) => Err(error),
                    Err(error) => {
                        self.durable_uncertainty_fail_stop = true;
                        Err(error)
                    }
                }
            }
            RetirementRevokeObservation::Refused {
                error,
                rollback: Err(rollback),
            } => {
                self.durable_uncertainty_fail_stop = true;
                Err(MobError::RetirementTopologyIncomplete(format!(
                    "supervisor revoke failed ({error}); recipient trust rollback failed ({rollback})",
                )))
            }
            RetirementRevokeObservation::InstallUncertain(error) => Err(self
                .quarantine_uncertain_recipient_trust_install(
                    &peer,
                    "send_bridge_command_typed",
                    error,
                )),
        };
        if let Err(error) = result {
            self.finish_retirement(
                continuation,
                Err(MobError::RetirementTopologyIncomplete(error.to_string())),
            )
            .await;
            return;
        }
        if let Err(error) = self
            .record_remote_member_supervisor_revoked(&continuation.entry)
            .await
        {
            self.finish_retirement(continuation, Err(error)).await;
            return;
        }
        #[cfg(test)]
        let injected = {
            if let Ok(mut target) = FAIL_AFTER_REMOTE_SUPERVISOR_REVOKED_FOR_IDENTITY.lock()
                && target.as_ref() == Some(&continuation.entry.agent_identity)
            {
                target.take();
                true
            } else {
                false
            }
        };
        #[cfg(test)]
        if injected {
            self.finish_retirement(
                continuation,
                Err(MobError::Internal(
                    "fault-injected cancellation after remote supervisor revoke checkpoint".into(),
                )),
            )
            .await;
            return;
        }
        self.retirement_remove_supervisor_trust(continuation, peer);
    }

    fn retirement_remove_supervisor_trust(
        &mut self,
        continuation: RetirementContinuation,
        peer: TrustedPeerDescriptor,
    ) {
        let bridge = self.supervisor_bridge.clone();
        self.dispatch_retirement(
            continuation,
            "retire-supervisor-trust-removal",
            async move {
                RetirementObservation::SupervisorTrustRemoved(bridge.untrust_recipient(&peer).await)
            },
        );
    }

    async fn retirement_release_attachments(&mut self, continuation: RetirementContinuation) {
        let prepared = async {
            let records = self
                .runtime_metadata
                .list_forked_participant_member_associations(&self.definition.id)
                .await?;
            let mut prepared = Vec::new();
            for record in records
                .into_iter()
                .filter(|record| record.agent_identity == continuation.entry.agent_identity)
            {
                let realm = match record.association.capability.owner_route().clone() {
                    ForkedParticipantOwnerRoute::Local { realm_id } => realm_id,
                    ForkedParticipantOwnerRoute::Host { .. } => {
                        return Err(MobError::ForkedParticipantRemoteLeaseUnsupported {
                            operation: crate::ForkedParticipantLeaseOperation::Release,
                        });
                    }
                };
                let service = self.local_forked_participant_service(
                    record.association.capability.source_identity(),
                    realm,
                )?;
                prepared.push((record, service));
            }
            Ok(prepared)
        }
        .await;
        let prepared = match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                self.finish_retirement(continuation, Err(error)).await;
                return;
            }
        };
        let metadata = self.runtime_metadata.clone();
        let mob = self.definition.id.clone();
        let entry = continuation.entry.clone();
        self.dispatch_retirement(continuation, "retire-capability-release", async move {
            let result = async {
                metadata.delete_external_binding_overlay(&mob, &entry.agent_identity, entry.generation).await?;
                for (record, service) in prepared {
                    let release = async {
                        if matches!(record.obligation, Some(crate::store::MobForkedParticipantObligationCause::AttachPending)) {
                            service.attach(&record.association.capability, &record.association.attachment_id, true, chrono::Utc::now())
                                .await.map_err(Self::forked_participant_refused)?;
                        }
                        service.release(&record.association.capability, &record.association.attachment_id)
                            .await.map_err(Self::forked_participant_refused)?;
                        metadata.delete_forked_participant_member_association(&mob, &record).await?;
                        Ok::<(), MobError>(())
                    }.await;
                    if let Err(error) = release {
                        let mut retained = record.clone();
                        retained.obligation = Some(crate::store::MobForkedParticipantObligationCause::TeardownReleaseUnproven);
                        retained.detail = error.to_string();
                        if let Err(store_error) = metadata.put_forked_participant_member_association(&mob, &retained).await {
                            tracing::error!(
                                member_id = %entry.agent_identity,
                                error = %store_error,
                                "capability release failure could not refine its retained durable obligation",
                            );
                        }
                        return Err(MobError::ForkedParticipantAttachmentReleaseUnproven {
                            member_id: entry.agent_identity.clone(),
                            attachment_id: record.association.attachment_id.as_str().to_string(),
                            detail: error.to_string(),
                        });
                    }
                }
                Ok(())
            }.await;
            RetirementObservation::AttachmentsReleased(result)
        });
    }

    async fn retirement_terminal_carrier_cleanup(&mut self, continuation: RetirementContinuation) {
        let identity = mob_dsl::AgentIdentity::from_domain(&continuation.entry.agent_identity);
        let obligation = self
            .dsl_authority
            .state()
            .pending_placed_carrier_cleanup
            .iter()
            .find(|obligation| obligation.agent_identity == identity)
            .cloned();
        let record = match self
            .runtime_metadata
            .load_placed_spawn(
                &self.definition.id,
                continuation.entry.agent_identity.as_str(),
            )
            .await
        {
            Ok(record) => record,
            Err(error) => {
                self.finish_retirement(continuation, Err(error.into()))
                    .await;
                return;
            }
        };
        match (obligation, record) {
            (None, None) => self.retirement_clean_projection(continuation),
            (Some(obligation), Some(record)) => {
                let authority = match self.authorize_placed_carrier_cleanup(
                    &record,
                    &obligation,
                    "retired_placed_carrier_cleanup_authorize",
                ) {
                    Ok(authority) => authority,
                    Err(error) => {
                        self.durable_uncertainty_fail_stop = true;
                        self.finish_retirement(continuation, Err(error)).await;
                        return;
                    }
                };
                let display = match render_member_comms_name(
                    self.definition.id.as_str(),
                    &record.spec.profile_name,
                    &record.agent_identity,
                ) {
                    Ok(display) => display,
                    Err(error) => {
                        self.finish_retirement(continuation, Err(error)).await;
                        return;
                    }
                };
                let metadata = self.runtime_metadata.clone();
                let provisioner = self.provisioner.clone();
                let mob = self.definition.id.clone();
                self.dispatch_retirement(
                    continuation,
                    "retire-terminal-carrier-cleanup",
                    async move {
                        let result = async {
                            provisioner
                                .retire_committed_placed_provision_operation(
                                    &record.operation_owner_session_id,
                                    &record.provision_operation_id,
                                    &display,
                                )
                                .await?;
                            match metadata
                                .compare_and_delete_placed_spawn(&mob, &record, &authority)
                                .await?
                            {
                                crate::store::DeletePlacedSpawnResult::Deleted
                                | crate::store::DeletePlacedSpawnResult::AlreadyAbsent => Ok(()),
                                crate::store::DeletePlacedSpawnResult::Conflict => {
                                    Err(MobError::Internal(
                                        "exact retirement carrier deletion conflicted".into(),
                                    ))
                                }
                            }
                        }
                        .await;
                        RetirementObservation::CarrierDeleted { obligation, result }
                    },
                );
            }
            _ => {
                self.durable_uncertainty_fail_stop = true;
                self.finish_retirement(
                    continuation,
                    Err(MobError::Internal(
                        "retirement carrier and generated cleanup obligation disagree".into(),
                    )),
                )
                .await;
            }
        }
    }

    fn retirement_clean_projection(&mut self, continuation: RetirementContinuation) {
        let identity = continuation.entry.agent_identity.clone();
        let diagnostics = self.restore_diagnostics.clone();
        let pumps = self.member_event_pumps.clone();
        let locks = self.edge_locks.clone();
        let metadata = self.runtime_metadata.clone();
        let mob = self.definition.id.clone();
        let generation = continuation.entry.generation;
        self.dispatch_retirement(continuation, "retire-projection-cleanup", async move {
            if let Err(error) = metadata
                .delete_external_binding_overlay(&mob, &identity, generation)
                .await
            {
                return RetirementObservation::ProjectionCleaned(Err(error.into()));
            }
            locks.prune(identity.as_str()).await;
            diagnostics.write().await.remove(&identity);
            pumps.stop_pump(&identity).await;
            RetirementObservation::ProjectionCleaned(Ok(()))
        });
    }

    async fn retirement_prepare_disposal(&mut self, mut continuation: RetirementContinuation) {
        if continuation.rollback.is_some() {
            self.retirement_prepare_archive(continuation).await;
            return;
        }
        continuation.pending_next = PendingCleanupNext::Disposal;
        continuation.pending_retire_incarnation = None;
        self.retirement_retry_pending_anchors(continuation).await;
    }

    async fn retirement_prepare_disposal_after_pending(
        &mut self,
        mut continuation: RetirementContinuation,
    ) {
        let result = async {
            if matches!(&continuation.reply, RetirementReply::IdentityReconcile(_))
                || matches!(&continuation.reply, RetirementReply::Respawn { .. })
                    && super::super::member_runtime_is_host_owned(
                        self.dsl_authority.state(),
                        &continuation.entry.agent_identity,
                    )
            {
                self.remote_flow_tickets.note_member_rematerializing(
                    &continuation.entry.agent_identity,
                    continuation.entry.generation.get(),
                );
            }
            let identity = mob_dsl::AgentIdentity::from_domain(&continuation.entry.agent_identity);
            let state = self.dsl_authority.state();
            continuation.placed_peers = state
                .wiring_edges
                .iter()
                .filter(|edge| edge.a == identity || edge.b == identity)
                .filter(|edge| {
                    state.member_placement.contains_key(&edge.a)
                        || state.member_placement.contains_key(&edge.b)
                })
                .map(|edge| {
                    AgentIdentity::from(if edge.a == identity {
                        edge.b.0.as_str()
                    } else {
                        edge.a.0.as_str()
                    })
                })
                .collect();
            Ok(())
        }
        .await;
        if let Err(error) = result {
            self.finish_retirement(continuation, Err(error)).await;
        } else {
            let identity = &continuation.entry.agent_identity;
            let mut waiters = self
                .peer_delivery_inflight
                .values()
                .filter(|delivery| &delivery.from == identity || &delivery.to == identity)
                .map(|delivery| {
                    delivery.cancel_token.cancel();
                    delivery.settled.clone()
                })
                .collect::<Vec<_>>();
            self.dispatch_retirement(continuation, "retire-peer-delivery-cancellation", async move {
                for waiter in &mut waiters {
                    loop {
                        if *waiter.borrow_and_update() { break; }
                        if waiter.changed().await.is_err() {
                            return RetirementObservation::PeerDeliveriesSettled(Err(
                                MobError::ExternalMemberCleanupUncertain {
                                    reason: "peer delivery task disappeared without actual settlement".into(),
                                },
                            ));
                        }
                    }
                }
                RetirementObservation::PeerDeliveriesSettled(Ok(()))
            });
        }
    }

    async fn retirement_next_placed_edge(&mut self, mut continuation: RetirementContinuation) {
        while let Some(peer) = continuation.placed_peers.pop() {
            let preparation = match self
                .prepare_member_unwire_for_retirement(
                    continuation.entry.agent_identity.clone(),
                    peer,
                    continuation.preserve_topology,
                )
                .await
                .map_err(Self::retirement_unwire_error)
            {
                Ok(preparation) => preparation,
                Err(error) => {
                    self.finish_retirement(continuation, Err(error)).await;
                    return;
                }
            };
            if let Some(prepared) = preparation.into_prepared() {
                self.dispatch_retirement(continuation, "retire-placed-edge", async move {
                    RetirementObservation::PlacedEdge(prepared.realize().await)
                });
                return;
            }
        }
        self.retirement_observe_endpoints(continuation, false).await;
    }

    async fn retirement_trust_observed(
        &mut self,
        mut continuation: RetirementContinuation,
        observed: RetirementEndpointObservations,
    ) {
        let mut trust = match self
            .member_retire_trust_cleanup_plan_observed(
                &continuation.entry.agent_identity,
                &continuation.entry,
                Some(observed),
            )
            .await
        {
            Ok(trust) => trust,
            Err(error) => {
                self.finish_retirement(continuation, Err(error)).await;
                return;
            }
        };
        if trust.retiring_comms.is_none() {
            trust.retiring_comms = continuation.retired_comms.take();
        }
        let mut ctx = Self::disposal_context_from_entry(
            &continuation.entry.agent_identity,
            &continuation.entry,
            trust,
            continuation.preserve_topology,
        );
        ctx.retirement_deadline = Some(continuation.deadline);
        continuation.disposal = Some(ctx);
        self.retirement_stop_host(continuation).await;
    }

    async fn retirement_stop_host(&mut self, mut continuation: RetirementContinuation) {
        let identity = continuation.entry.agent_identity.clone();
        let autonomous = continuation.entry.runtime_mode == crate::MobRuntimeMode::AutonomousHost;
        let handle = if autonomous {
            self.autonomous_initial_turns.lock().await.remove(&identity)
        } else {
            None
        };
        if autonomous {
            let member = mob_dsl::AgentIdentity::from_domain(&identity);
            let state = self.dsl_authority.state();
            if state.member_kickoff_pending.contains(&member)
                || state.member_kickoff_starting.contains(&member)
                || state.member_kickoff_callback_pending.contains(&member)
            {
                let effects = self
                    .commit_kickoff_input_effects(
                        &identity,
                        mob_dsl::MobMachineInput::KickoffCancelRequested { member_id: member },
                        "request_autonomous_kickoff_stop",
                    )
                    .await;
                match effects {
                    Ok(effects) => {
                        continuation
                            .kickoff_notices
                            .extend(effects.into_iter().filter_map(|effect| match effect {
                                mob_dsl::MobMachineEffect::EmitKickoffLifecycleNotice {
                                    intent,
                                    ..
                                } => Some(Self::kickoff_notice_intent(intent)),
                                _ => None,
                            }));
                    }
                    Err(error) => {
                        if let Some(handle) = handle {
                            self.autonomous_initial_turns
                                .lock()
                                .await
                                .insert(identity, handle);
                        }
                        self.finish_retirement(continuation, Err(error)).await;
                        return;
                    }
                }
            }
        }
        let placed =
            super::super::member_runtime_is_host_owned(self.dsl_authority.state(), &identity);
        let session = continuation.entry.bridge_session_id().cloned();
        #[cfg(feature = "runtime-adapter")]
        let adapter = self.runtime_adapter.clone();
        self.dispatch_retirement(continuation, "retire-stop-host-loop", async move {
            if let Some(handle) = handle { handle.abort_and_join().await; }
            #[cfg(feature = "runtime-adapter")]
            if autonomous && !placed
                && let (Some(adapter), Some(session)) = (adapter, session)
                && let Err(error) = adapter.abort_comms_drain(&session).await
            {
                tracing::warn!(%identity, %error, "retirement drain abort deferred to exact archive quiescence");
            }
            #[cfg(not(feature = "runtime-adapter"))]
            let _ = (placed, session);
            RetirementObservation::HostStopped(Ok(()))
        });
    }

    async fn retirement_prepare_archive(&mut self, mut continuation: RetirementContinuation) {
        let identity = &continuation.entry.agent_identity;
        let placed =
            super::super::member_runtime_is_host_owned(self.dsl_authority.state(), identity);
        if placed {
            let prepared = async {
                let record = self
                    .runtime_metadata
                    .load_placed_spawn(&self.definition.id, identity.as_str())
                    .await?
                    .filter(|record| {
                        record.generation == continuation.entry.generation.get()
                            && record.fence_token == continuation.entry.fence_token.get()
                            && matches!(
                                record.phase,
                                crate::store::PlacedSpawnCarrierPhase::Committed(_)
                            )
                    })
                    .ok_or_else(|| {
                        MobError::Internal(format!(
                            "placed retirement for '{identity}' lacks its exact committed carrier"
                        ))
                    })?;
                let prepared = super::super::placed_carrier_cleanup::prepare_placed_release(
                    &self.definition.id,
                    &record,
                    &self.dsl_authority,
                )?;
                let host_id = mob_dsl::HostId::from(record.host_id.to_string());
                if self
                    .dsl_authority
                    .state()
                    .host_binding_generations
                    .get(&host_id)
                    .copied()
                    == Some(record.host_binding_generation)
                    && self.dsl_authority.state().host_bind_phase.get(&host_id)
                        == Some(&mob_dsl::HostBindPhase::Bound)
                {
                    let key = super::super::state::HostOrphanReleaseKey {
                        binding_incarnation: self.current_host_binding_incarnation(&host_id)?,
                        host_id,
                        agent_identity: mob_dsl::AgentIdentity::from_domain(identity),
                        generation: mob_dsl::Generation::from_domain(continuation.entry.generation),
                        fence_token: mob_dsl::FenceToken::from_domain(
                            continuation.entry.fence_token,
                        ),
                    };
                    if !Self::reserve_host_orphan_release(
                        &mut self.orphan_release_reservations,
                        &key,
                    ) {
                        return Err(MobError::Internal(
                            "retirement collided with an exact outstanding host release".into(),
                        ));
                    }
                    continuation.release_reservation = Some(key);
                }
                Ok(prepared)
            }
            .await;
            let prepared = match prepared {
                Ok(prepared) => prepared,
                Err(error) => {
                    self.finish_retirement(continuation, Err(error)).await;
                    return;
                }
            };
            let provisioner = self.provisioner.clone();
            let bridge = self.supervisor_bridge.clone();
            self.dispatch_retirement(continuation, "retire-placed-archive", async move {
                RetirementObservation::Archived(
                    super::super::placed_carrier_cleanup::realize_placed_release(
                        prepared,
                        provisioner.as_ref(),
                        bridge,
                    )
                    .await
                    .map(|()| mob_dsl::MemberSessionDisposal::RuntimeReleasedOnlyHostOwned),
                )
            });
            return;
        }
        if Self::runtime_binding_for_entry(&continuation.entry).is_some()
            && self.remote_runtime_retired_for_entry(&continuation.entry)
        {
            self.retirement_post_archive(
                continuation,
                mob_dsl::MemberSessionDisposal::RuntimeReleasedOnlyHostOwned,
            )
            .await;
            return;
        }
        let provisioner = self.provisioner.clone();
        let member = continuation.entry.member_ref.clone();
        let identity = identity.clone();
        let deadline = continuation.deadline;
        let observation = RetirementArchiveObservationSource {
            service: self.session_service.clone(),
            #[cfg(feature = "runtime-adapter")]
            adapter: self.runtime_adapter.clone(),
        };
        self.dispatch_retirement(continuation, "retire-session-archive", async move {
            let result = async {
                if let Some(session) = member.bridge_session_id()
                    && observation.already_complete(session).await?
                {
                    return Ok(mob_dsl::MemberSessionDisposal::Archived);
                }
                let result = provisioner
                    .retire_member_until(&member, &identity, deadline)
                    .await;
                if matches!(
                    &result,
                    Err(MobError::SessionError(
                        meerkat_core::service::SessionError::NotFound { .. }
                    ))
                ) && let Some(session) = member.bridge_session_id()
                    && observation.already_complete(session).await?
                {
                    return Ok(mob_dsl::MemberSessionDisposal::Archived);
                }
                result
            }
            .await;
            RetirementObservation::Archived(result)
        });
    }

    fn retirement_observe_route(&mut self, continuation: RetirementContinuation) {
        let session = continuation.entry.bridge_session_id().cloned();
        let service = self.session_service.clone();
        #[cfg(feature = "runtime-adapter")]
        let adapter = self.runtime_adapter.clone();
        self.dispatch_retirement(continuation, "retire-route-observation", async move {
            let result = async {
                let Some(session) = session else {
                    return Ok(RetirementRouteObservation {
                        archive_complete: false,
                        exact_local_target_quiescent: false,
                    });
                };
                if service.has_live_session(&session).await?
                    || service.load_persisted_session(&session).await?.is_some()
                {
                    return Ok(RetirementRouteObservation {
                        archive_complete: false,
                        exact_local_target_quiescent: false,
                    });
                }
                #[cfg(feature = "runtime-adapter")]
                let (runtime_absent, attachment_absent) = match adapter {
                    Some(adapter) => (
                        !adapter
                            .archive_runtime_residue_present(&session)
                            .await
                            .map_err(|error| MobError::Internal(error.to_string()))?,
                        adapter
                            .current_executor_attachment_witness(&session)
                            .await
                            .is_none(),
                    ),
                    None => (true, false),
                };
                #[cfg(not(feature = "runtime-adapter"))]
                let (runtime_absent, attachment_absent) = (true, false);
                let archive_complete =
                    runtime_absent && service.session_known_to_archive_authority(&session).await?;
                Ok(RetirementRouteObservation {
                    archive_complete,
                    exact_local_target_quiescent: runtime_absent && attachment_absent,
                })
            }
            .await;
            RetirementObservation::RouteObserved(result)
        });
    }

    async fn retirement_route_observed(
        &mut self,
        mut continuation: RetirementContinuation,
        observation: Result<RetirementRouteObservation, MobError>,
    ) {
        let observation = match observation {
            Ok(observation) => observation,
            Err(error) => {
                self.finish_retirement(continuation, Err(error)).await;
                return;
            }
        };
        let identity = mob_dsl::AgentIdentity::from_domain(&continuation.entry.agent_identity);
        let runtime = mob_dsl::AgentRuntimeId::from_domain(&continuation.entry.agent_runtime_id);
        let session = continuation
            .entry
            .bridge_session_id()
            .map(mob_dsl::SessionId::from_domain);
        if !observation.archive_complete
            && continuation.routes.is_empty()
            && let Some(pending) = self
                .dsl_authority
                .state()
                .runtime_retire_pending_sessions
                .get(&runtime)
                .cloned()
        {
            if let Err(error) = self.ensure_runtime_retire_route_after_detach(
                &identity,
                &runtime,
                &pending,
                "retry_runtime_retire_after_consumer_refusal",
            ) {
                self.finish_retirement(continuation, Err(error)).await;
                return;
            }
            continuation
                .routes
                .extend(self.take_retirement_routes(&continuation.entry));
        }
        let exact_queue = continuation.routes.len() == 1 && continuation.routes.iter().all(|effect| {
            matches!(effect.body(), mob_dsl::MobMachineEffect::RequestRuntimeRetire {
                agent_identity, agent_runtime_id, session_id,
            } if agent_identity == &identity && agent_runtime_id == &runtime && Some(session_id) == session.as_ref())
        });
        if !continuation.routes.is_empty() && !exact_queue {
            self.finish_retirement(
                continuation,
                Err(MobError::Internal(
                    "retirement owns a mismatched or duplicate routed-retire effect".to_string(),
                )),
            )
            .await;
            return;
        }
        let exact_local_target = continuation.preserve_topology
            && matches!(continuation.entry.member_ref, MemberRef::Session { .. })
            && self
                .dsl_authority
                .state()
                .member_session_bindings
                .get(&identity)
                == session.as_ref()
            && self
                .dsl_authority
                .state()
                .runtime_retire_pending_sessions
                .get(&runtime)
                == session.as_ref()
            && !self
                .dsl_authority
                .state()
                .member_placement
                .contains_key(&identity)
            && exact_queue;
        if observation.archive_complete
            || exact_local_target && observation.exact_local_target_quiescent
        {
            continuation.routes.clear();
        }
        let Some(effect) = continuation.routes.pop() else {
            self.retirement_prepare_disposal(continuation).await;
            return;
        };
        let binding = match &self.composition_binding {
            meerkat_runtime::composition::CompositionBinding::Standalone => {
                meerkat_runtime::composition::CompositionBinding::Standalone
            }
            meerkat_runtime::composition::CompositionBinding::Wired(dispatcher) => {
                meerkat_runtime::composition::CompositionBinding::Wired(dispatcher.clone())
            }
            meerkat_runtime::composition::CompositionBinding::OwnerProvided {
                dispatcher,
                context,
            } => meerkat_runtime::composition::CompositionBinding::OwnerProvided {
                dispatcher: dispatcher.clone(),
                context: context.clone(),
            },
        };
        self.dispatch_retirement(continuation, "retire-runtime-route", async move {
            let result =
                super::super::composition::dispatch_routed_effect(&binding, effect.clone()).await;
            RetirementObservation::Routed { effect, result }
        });
    }

    async fn retirement_route_settled(
        &mut self,
        mut continuation: RetirementContinuation,
        effect: super::super::composition::MobSeamEffect,
        result: Result<
            Option<meerkat_runtime::composition::DispatchOutcome>,
            meerkat_runtime::composition::DispatchRefusal,
        >,
    ) {
        use meerkat_runtime::composition::DispatchRefusal;
        match result {
            Ok(_) => self.retirement_prepare_disposal(continuation).await,
            Err(DispatchRefusal::ConsumerRefused { error, .. }) => {
                let closed = (|| {
                    let feedback =
                        super::super::composition::refusal_feedback_input(&effect, &error)?;
                    let transition = self.apply_dsl_input_collect_transition(
                        feedback,
                        "close_routed_effect_consumer_refusal",
                    )?;
                    closed_runtime_effect_refusal_from_transition(&transition)
                })();
                let error = match closed {
                    Ok(closed) => closed.into_mob_error(),
                    Err(error) => {
                        continuation.routes.push(effect);
                        error
                    }
                };
                self.finish_retirement(continuation, Err(error)).await;
            }
            Err(error) => {
                continuation.routes.push(effect);
                self.finish_retirement(
                    continuation,
                    Err(super::super::composition::dispatch_refusal_to_mob_error(
                        error,
                    )),
                )
                .await;
            }
        }
    }

    fn take_retirement_routes(
        &mut self,
        entry: &RosterEntry,
    ) -> Vec<super::super::composition::MobSeamEffect> {
        let runtime = mob_dsl::AgentRuntimeId::from_domain(&entry.agent_runtime_id);
        let mut retained = Vec::new();
        let mut selected = Vec::new();
        for effect in std::mem::take(&mut self.pending_routed_effects) {
            if matches!(effect.body(), mob_dsl::MobMachineEffect::RequestRuntimeRetire {
                agent_runtime_id, ..
            } if agent_runtime_id == &runtime)
            {
                selected.push(effect);
            } else {
                retained.push(effect);
            }
        }
        self.pending_routed_effects = retained;
        selected
    }

    fn retirement_started_journal_kind(
        &self,
        entry: &RosterEntry,
        releasing: Option<&mob_dsl::SessionId>,
        session: Option<&mob_dsl::SessionId>,
        released: bool,
    ) -> Result<mob_dsl::MobLifecycleJournalKind, MobError> {
        if released {
            return Ok(mob_dsl::MobLifecycleJournalKind::MemberRetirementStartedReleasing);
        }
        let placed = super::super::member_runtime_is_host_owned(
            self.dsl_authority.state(),
            &entry.agent_identity,
        );
        match (placed, releasing.is_some(), session.is_some()) {
            (true, _, true) | (false, false, true) => {
                Ok(mob_dsl::MobLifecycleJournalKind::MemberRetirementStartedPreservingBinding)
            }
            (false, true, true) => {
                Ok(mob_dsl::MobLifecycleJournalKind::MemberRetirementStartedReleasing)
            }
            (false, false, false) => {
                Ok(mob_dsl::MobLifecycleJournalKind::MemberRetirementStartedPeerOnly)
            }
            _ => Err(MobError::Internal(format!(
                "retirement for '{}' has inconsistent exact session correlation",
                entry.agent_identity,
            ))),
        }
    }

    async fn retirement_admit(&mut self, mut continuation: RetirementContinuation) {
        let entry = &continuation.entry;
        let identity = mob_dsl::AgentIdentity::from_domain(&entry.agent_identity);
        let runtime = mob_dsl::AgentRuntimeId::from_domain(&entry.agent_runtime_id);
        let state = self.dsl_authority.state();
        let bound = state.member_session_bindings.get(&identity).cloned();
        let pending = state.runtime_retire_pending_sessions.get(&runtime).cloned();
        let placed = state.member_placement.contains_key(&identity);
        let live = state.live_runtime_ids.contains(&runtime);
        let retiring =
            state.member_state_markers.get(&runtime) == Some(&mob_dsl::MobMemberState::Retiring);
        let releasing = if continuation.preserve_binding {
            None
        } else {
            bound.clone()
        };
        let session = bound.clone().or(pending.clone());
        let released = !placed && bound.is_none() && pending.is_some();
        let result = async {
            let journal = self.retirement_started_journal_kind(
                entry,
                releasing.as_ref(),
                session.as_ref(),
                released,
            )?;
            if live && !released {
                let prepared = self.prepare_dsl_input_transition(
                    mob_dsl::MobMachineInput::Retire {
                        mob_id: mob_dsl::MobId::from_domain(&self.definition.id),
                        agent_identity: identity.clone(),
                        agent_runtime_id: runtime.clone(),
                        generation: mob_dsl::Generation::from_domain(entry.generation),
                        releasing,
                        session_id: session.clone(),
                    },
                    "handle_retire_inner_mark_retiring",
                )?;
                Self::require_member_lifecycle_journal_effect(
                    &prepared.transition,
                    journal,
                    &entry.agent_identity,
                    &entry.agent_runtime_id,
                    None,
                    entry.generation,
                    session.clone(),
                    "handle_retire_inner_mark_retiring",
                )?;
                if !continuation.retirement_started {
                    self.append_retirement_started_event_for_entry(
                        entry,
                        journal,
                        session,
                        continuation.preserve_topology,
                    )
                    .await?;
                }
                continuation.detach =
                    crate::generated::protocol_mob_destroying_session_ingress::extract_obligations(
                        &prepared.transition,
                    );
                self.commit_prepared_dsl_transition(prepared)?;
            } else if !retiring || !continuation.retirement_started {
                return Err(MobError::Internal(format!(
                    "member '{}' has a non-live runtime without an exact durable retirement anchor",
                    entry.agent_identity,
                )));
            }
            continuation.retirement_started = true;
            if let Some(admission) = continuation.admission.take() {
                admission.send_replace(true);
            }
            Ok(())
        }
        .await;
        continuation
            .routes
            .extend(self.take_retirement_routes(&continuation.entry));
        match result {
            Ok(()) => self.retirement_runtime_quiesce(continuation, false),
            Err(error) => self.finish_retirement(continuation, Err(error)).await,
        }
    }

    async fn retirement_detach(&mut self, mut continuation: RetirementContinuation) {
        let entry = &continuation.entry;
        let runtime = mob_dsl::AgentRuntimeId::from_domain(&entry.agent_runtime_id);
        let preflight = async {
            if super::super::member_runtime_is_host_owned(
                self.dsl_authority.state(),
                &entry.agent_identity,
            ) {
                self.drive_placed_completion_lifecycle_cleanup(
                    Some(&entry.agent_identity),
                    false,
                    None,
                )
                .await?;
            }
            if continuation.preserve_topology {
                self.apply_dsl_signal(
                    mob_dsl::MobMachineSignal::ObserveRespawnTopologyPreservationStarted {
                        agent_identity: mob_dsl::AgentIdentity::from_domain(&entry.agent_identity),
                        agent_runtime_id: runtime.clone(),
                        fence_token: mob_dsl::FenceToken::from_domain(entry.fence_token),
                        generation: mob_dsl::Generation::from_domain(entry.generation),
                    },
                    "record_respawn_topology_preservation_start",
                )?;
            }
            Ok::<(), MobError>(())
        }
        .await;
        if let Err(error) = preflight {
            self.finish_retirement(continuation, Err(error)).await;
            return;
        }
        let result = (|| {
            if continuation.detach.is_empty()
                && self
                    .dsl_authority
                    .state()
                    .pending_session_ingress_detach_runtime_ids
                    .contains(&runtime)
            {
                let transition = self.apply_dsl_input_collect_transition(
                    mob_dsl::MobMachineInput::RequestPendingSessionIngressDetachForMobDestroy {
                        mob_id: mob_dsl::MobId::from_domain(&self.definition.id),
                        agent_runtime_id: runtime.clone(),
                    },
                    "retire_request_pending_session_ingress_detach",
                )?;
                continuation.detach =
                    crate::generated::protocol_mob_destroying_session_ingress::extract_obligations(
                        &transition,
                    );
            }
            if continuation.detach.len() > 1 {
                return Err(MobError::Internal(
                    "retirement produced multiple ingress-detach obligations".to_string(),
                ));
            }
            if let Some(obligation) = continuation.detach.first()
                && (obligation.mob_id() != &mob_dsl::MobId::from_domain(&self.definition.id)
                    || obligation.agent_runtime_id() != &runtime
                    || super::super::member_runtime_is_host_owned(
                        self.dsl_authority.state(),
                        &entry.agent_identity,
                    ))
            {
                return Err(MobError::Internal(
                    "retirement ingress-detach obligation mismatches its exact local runtime"
                        .to_string(),
                ));
            }
            Ok(())
        })();
        continuation
            .routes
            .extend(self.take_retirement_routes(&continuation.entry));
        if let Err(error) = result {
            self.finish_retirement(continuation, Err(error)).await;
            return;
        }
        let session = (!continuation.detach.is_empty())
            .then(|| continuation.entry.bridge_session_id().cloned())
            .flatten();
        #[cfg(feature = "runtime-adapter")]
        let adapter = self.runtime_adapter.clone();
        self.dispatch_retirement(continuation, "retire-ingress-detach", async move {
            let result = async {
                #[cfg(test)]
                if let Some(session_id) = session.as_ref()
                    && let Ok(mut target) = FAIL_SESSION_INGRESS_DETACH_FOR_SESSION.lock()
                    && target.as_ref() == Some(session_id)
                {
                    target.take();
                    return Err(MobError::Internal(format!(
                        "fault-injected session-ingress detach failure for {session_id}"
                    )));
                }
                #[cfg(feature = "runtime-adapter")]
                if let (Some(adapter), Some(session)) = (adapter, session.as_ref()) {
                    match adapter
                        .update_peer_ingress_context(session, false, None)
                        .await
                    {
                        Ok(_)
                        | Err(
                            meerkat_runtime::RuntimeDriverError::NotFound { .. }
                            | meerkat_runtime::RuntimeDriverError::Destroyed
                            | meerkat_runtime::RuntimeDriverError::NotReady { .. },
                        ) => {}
                        Err(error) => {
                            return Err(MobError::Internal(format!(
                                "failed to detach peer ingress for session {session}: {error}"
                            )));
                        }
                    }
                    let owner = adapter.peer_ingress_owner(session).await;
                    if !matches!(owner, meerkat_runtime::PeerIngressOwner::Unattached) {
                        return Err(MobError::Internal(format!(
                            "peer ingress owner remained attached after detach: {owner:?}"
                        )));
                    }
                }
                #[cfg(not(feature = "runtime-adapter"))]
                let _ = session;
                Ok(())
            }
            .await;
            RetirementObservation::IngressDetached(result)
        });
    }

    async fn retirement_detach_settled(
        &mut self,
        mut continuation: RetirementContinuation,
        result: Result<(), MobError>,
    ) {
        use crate::generated::protocol_mob_destroying_session_ingress::{
            submit_session_ingress_detach_failed_for_mob_destroy,
            submit_session_ingress_detached_for_mob_destroy,
        };
        for obligation in std::mem::take(&mut continuation.detach) {
            match &result {
                Ok(()) => {
                    if let Err(error) = submit_session_ingress_detached_for_mob_destroy(
                        &mut self.dsl_authority,
                        obligation,
                    ) {
                        self.publish_machine_state_projection();
                        self.finish_retirement(
                            continuation,
                            Err(MobError::Internal(error.to_string())),
                        )
                        .await;
                        return;
                    }
                }
                Err(error) => {
                    let _ = submit_session_ingress_detach_failed_for_mob_destroy(
                        &mut self.dsl_authority,
                        obligation,
                        error.to_string(),
                    );
                }
            }
            self.publish_machine_state_projection();
        }
        if let Err(error) = result {
            self.finish_retirement(continuation, Err(error)).await;
            return;
        }
        continuation
            .routes
            .extend(self.take_retirement_routes(&continuation.entry));
        self.retirement_observe_route(continuation);
    }

    pub(super) async fn start_retirement(
        &mut self,
        identity: AgentIdentity,
        deadline: Instant,
        admission: Option<tokio::sync::watch::Sender<bool>>,
        reply: RetirementReply,
    ) {
        let reply = match reply {
            RetirementReply::Retire(reply) => {
                match self.request_spawn_rollback_retry(&identity, reply).await {
                    Ok(()) => return,
                    Err(reply) => RetirementReply::Retire(reply),
                }
            }
            RetirementReply::BatchRetire(reply) => {
                match self.request_spawn_rollback_retry(&identity, reply).await {
                    Ok(()) => return,
                    Err(reply) => RetirementReply::BatchRetire(reply),
                }
            }
            reply => reply,
        };
        if self.retirements.contains_key(&identity) {
            let error = MobError::LifecycleOperationPending {
                intent: format!("retirement member {identity}"),
            };
            match reply {
                RetirementReply::Retire(reply) | RetirementReply::BatchRetire(reply) => {
                    let _ = reply.send(Err(error));
                }
                RetirementReply::Respawn { reply, .. } => {
                    let _ = reply.send(Err(super::super::handle::MobRespawnError::from(error)));
                }
                RetirementReply::IdentityReconcile(_) => {}
                RetirementReply::SpawnRollback => {}
            }
            return;
        }
        let entry = self.roster.read().await.get(&identity).cloned();
        let Some(entry) = entry else {
            let result = self
                .apply_command_admission(
                    mob_dsl::MobMachineInput::RetireAbsent {
                        agent_identity: mob_dsl::AgentIdentity::from_domain(&identity),
                    },
                    MobState::Running,
                    "handle_retire_inner_absent",
                )
                .map(|_| ());
            match reply {
                RetirementReply::Retire(reply) | RetirementReply::BatchRetire(reply) => {
                    let _ = reply.send(result);
                }
                RetirementReply::Respawn { reply, .. } => {
                    let _ = reply.send(Err(super::super::handle::MobRespawnError::from(
                        MobError::MemberNotFound(identity),
                    )));
                }
                RetirementReply::IdentityReconcile(authority) => {
                    if let Err(error) = result {
                        self.record_identity_reconcile_disposition(
                            &identity,
                            &authority,
                            identity_actuation_error_disposition(&error),
                        )
                        .await;
                    }
                    self.enqueue_identity_reconcile(identity);
                }
                RetirementReply::SpawnRollback => {}
            }
            return;
        };
        let respawn = matches!(
            &reply,
            RetirementReply::Respawn { .. } | RetirementReply::IdentityReconcile(_)
        );
        let placed =
            super::super::member_runtime_is_host_owned(self.dsl_authority.state(), &identity);
        let placement = self.retirement_placement_fence(&identity);
        let mut continuation = Box::new(RetirementState {
            ticket: 0,
            entry,
            preserve_binding: respawn && !placed,
            preserve_topology: respawn,
            deadline,
            admission,
            reply,
            detach: Vec::new(),
            retired_comms: None,
            routes: Vec::new(),
            retirement_started: false,
            terminal_published: false,
            disposal: None,
            placed_peers: Vec::new(),
            remote_peers: Vec::new(),
            archive_disposal: None,
            participants: Vec::new(),
            external_edges: Vec::new(),
            external_comms: None,
            kickoff_notices: Vec::new(),
            placement,
            release_reservation: None,
            retained_outcomes: Vec::new(),
            pending_next: PendingCleanupNext::OperatorAdmission,
            pending_slots: VecDeque::new(),
            pending_slot: None,
            pending_errors: Vec::new(),
            pending_retire_incarnation: None,
            rollback: None,
        });
        let prepared = async {
            self.ensure_pending_spawn_alignment("handle_retire preflight")?;
            self.retirement_effect_is_current(&continuation.entry).await?;
            continuation.terminal_published = self.retire_event_exists(
                &identity, continuation.entry.generation,
            ).await?;
            continuation.retirement_started = self.retirement_started_event_exists(
                &identity, continuation.entry.generation,
            ).await?;
            if continuation.retirement_started {
                let preserved = self.preserved_respawn_topology_event_exists(
                    &identity, continuation.entry.generation,
                ).await;
                let abandoned = self.dsl_authority.state().abandoned_respawn_topology
                    .get(&mob_dsl::AgentIdentity::from_domain(&identity))
                    .is_some_and(|generation| generation.0 == continuation.entry.generation.get());
                if respawn && !preserved && !abandoned && !continuation.terminal_published {
                    return Err(MobError::WiringError(format!(
                        "cannot respawn '{identity}' after an ordinary retirement start is already durable; retry or complete that retirement first"
                    )));
                }
                continuation.preserve_topology = preserved && !abandoned;
                continuation.preserve_binding = continuation.preserve_topology && !placed;
            }
            Ok(())
        }.await;
        if let Err(error) = prepared {
            self.finish_retirement(continuation, Err(error)).await;
            return;
        }
        if matches!(&continuation.reply, RetirementReply::Retire(_)) {
            self.retirement_retry_pending_anchors(continuation).await;
        } else {
            self.retirement_begin_member_effects(continuation).await;
        }
    }

    async fn retirement_begin_member_effects(&mut self, mut continuation: RetirementContinuation) {
        if continuation.terminal_published {
            self.retirement_post_archive(
                continuation,
                mob_dsl::MemberSessionDisposal::RuntimeReleasedOnlyHostOwned,
            )
            .await;
        } else if continuation.retirement_started {
            if let Some(admission) = continuation.admission.take() {
                admission.send_replace(true);
            }
            self.retirement_runtime_quiesce(continuation, true);
        } else {
            if let Err(error) = self.schedule_retained_member_live_open_cleanups() {
                self.finish_retirement(continuation, Err(error)).await;
                return;
            }
            let mut tasks = std::mem::take(&mut self.member_live_mutation_tasks);
            self.dispatch_retirement(continuation, "retire-live-mutation-drain", async move {
                let mut completed = Vec::new();
                while let Some(joined) = tasks.join_next().await {
                    completed.push(joined);
                }
                RetirementObservation::LiveMutations(completed)
            });
        }
    }

    fn retirement_runtime_quiesce(
        &mut self,
        continuation: RetirementContinuation,
        before_admission: bool,
    ) {
        let session = continuation.entry.bridge_session_id().cloned();
        let member_ref = continuation.entry.member_ref.clone();
        let deadline = continuation.deadline;
        let provisioner = self.provisioner.clone();
        let bridge = self.supervisor_bridge.clone();
        let placed = super::super::member_runtime_is_host_owned(
            self.dsl_authority.state(),
            &continuation.entry.agent_identity,
        );
        #[cfg(feature = "runtime-adapter")]
        let adapter = self.runtime_adapter.clone();
        #[cfg(feature = "runtime-adapter")]
        let ops = self.session_ops_adapter.clone();
        self.dispatch_retirement(continuation, "retire-runtime-quiesce", async move {
            let result = async {
                #[cfg(feature = "runtime-adapter")]
                if let Some(session) = session.as_ref() {
                    super::super::provisioner::MemberSessionDisposalArc::cancel_active_runtime_turn_before_retire_with_adapter_until(
                        adapter.as_ref(), Some(&ops), session, deadline,
                    ).await.map_err(|error| {
                        super::super::provisioner::MemberSessionDisposalArc::map_runtime_retirement_error(session, error)
                    })?;
                }
                #[cfg(not(feature = "runtime-adapter"))]
                let _ = (session, deadline);
                if placed {
                    return Ok(None);
                }
                if let Some(comms) = provisioner.comms_runtime(&member_ref).await {
                    return Ok(Some(comms));
                }
                if matches!(member_ref, MemberRef::BackendPeer { session_id: None, .. }) {
                    return Ok(Some(bridge.runtime_core().await));
                }
                Ok(None)
            }.await;
            if before_admission {
                RetirementObservation::BeforeAdmissionQuiesced(result)
            } else {
                RetirementObservation::RuntimeQuiesced(result)
            }
        });
    }

    // Construct only the selected stage future; an async match reserves the
    // aggregate debug poll frame of every retirement branch.
    fn resume_retirement(
        &mut self,
        mut continuation: RetirementContinuation,
        observation: RetirementObservation,
    ) -> ActorCommandFuture<'_, ()> {
        match observation {
            RetirementObservation::SpawnRollback(observation) => {
                boxed_arm_future(move || async move {
                    self.resume_spawn_rollback(continuation, observation).await;
                })
            }
            RetirementObservation::LiveMutations(completed) => {
                boxed_arm_future(move || async move {
                    let mut failure = None;
                    for completed in completed {
                        if let Err(error) = self
                            .reconcile_joined_member_live_mutation(
                                completed,
                                MemberLiveReconcileMode::Lifecycle,
                            )
                            .await
                        {
                            failure.get_or_insert(error);
                        }
                    }
                    if failure.is_none() && !self.member_live_open_cleanup_obligations.is_empty() {
                        failure = Some(MobError::Internal(
                            "member-live cleanup retains an unproven exact close".to_string(),
                        ));
                    }
                    if let Some(error) = failure {
                        self.finish_retirement(continuation, Err(error)).await;
                        return;
                    }
                    let target = match self
                        .member_live_cleanup_target(&continuation.entry.agent_identity)
                        .await
                    {
                        Ok(target) => target,
                        Err(error) => {
                            self.finish_retirement(continuation, Err(error)).await;
                            return;
                        }
                    };
                    let bridge = self.supervisor_bridge.clone();
                    let host = self.member_live_host.clone();
                    self.dispatch_retirement(continuation, "retire-live-close", async move {
                        let result = async {
                            let Some(target) = target else { return Ok(()) };
                            let status = match Self::member_live_status_for_target_owned(
                                &bridge,
                                host.as_ref(),
                                &target,
                            )
                            .await
                            {
                                Ok(status) => status,
                                Err(error) if Self::member_live_status_proves_absent(&error) => {
                                    return Ok(());
                                }
                                Err(error) => return Err(error),
                            };
                            Self::close_exact_member_live_channel_owned(
                                bridge,
                                host,
                                target,
                                status.channel_id,
                            )
                            .await
                        }
                        .await;
                        RetirementObservation::LiveClosed(result)
                    });
                })
            }
            RetirementObservation::LiveClosed(result) => boxed_arm_future(move || async move {
                if let Err(error) = result {
                    self.finish_retirement(continuation, Err(error)).await;
                } else {
                    self.retirement_admit(continuation).await;
                }
            }),
            RetirementObservation::BeforeAdmissionQuiesced(result) => {
                boxed_arm_future(move || async move {
                    match result {
                        Ok(comms) => {
                            continuation.retired_comms = comms;
                            self.retirement_admit(continuation).await;
                        }
                        Err(error) => self.finish_retirement(continuation, Err(error)).await,
                    }
                })
            }
            RetirementObservation::RuntimeQuiesced(result) => {
                boxed_arm_future(move || async move {
                    match result {
                        Ok(comms) => {
                            continuation.retired_comms = comms;
                            self.retirement_detach(continuation).await;
                        }
                        Err(error) => self.finish_retirement(continuation, Err(error)).await,
                    }
                })
            }
            RetirementObservation::IngressDetached(result) => {
                boxed_arm_future(move || async move {
                    self.retirement_detach_settled(continuation, result).await;
                })
            }
            RetirementObservation::RouteObserved(result) => boxed_arm_future(move || async move {
                self.retirement_route_observed(continuation, result).await;
            }),
            RetirementObservation::Routed { effect, result } => {
                boxed_arm_future(move || async move {
                    self.retirement_route_settled(continuation, effect, result)
                        .await;
                })
            }
            RetirementObservation::Archived(result) => boxed_arm_future(move || async move {
                if let Some(key) = continuation.release_reservation.take()
                    && !Self::absorb_host_orphan_release_completion(
                        &mut self.orphan_release_reservations,
                        &key,
                        result.is_ok(),
                    )
                {
                    self.durable_uncertainty_fail_stop = true;
                    self.finish_retirement(
                        continuation,
                        Err(MobError::Internal(
                            "retirement lost its exact host-release reservation".into(),
                        )),
                    )
                    .await;
                    return;
                }
                match result {
                    Ok(disposal) => self.retirement_post_archive(continuation, disposal).await,
                    Err(error) => self.finish_retirement(continuation, Err(error)).await,
                }
            }),
            RetirementObservation::PlacedEdge(realized) => boxed_arm_future(move || async move {
                match self
                    .commit_realized_wiring(realized)
                    .await
                    .map_err(Self::retirement_unwire_error)
                {
                    Ok(()) => self.retirement_next_placed_edge(continuation).await,
                    Err(error) => self.finish_retirement(continuation, Err(error)).await,
                }
            }),
            RetirementObservation::HostStopped(result) => boxed_arm_future(move || async move {
                let result = match result {
                    Ok(()) => {
                        self.commit_kickoff_input_effects(
                            &continuation.entry.agent_identity,
                            mob_dsl::MobMachineInput::KickoffQuiesced {
                                member_id: mob_dsl::AgentIdentity::from_domain(
                                    &continuation.entry.agent_identity,
                                ),
                            },
                            "dispose_stop_host_loop",
                        )
                        .await
                    }
                    Err(error) => Err(error),
                };
                match result {
                    Err(error) => self.finish_retirement(continuation, Err(error)).await,
                    Ok(effects) => {
                        continuation
                            .kickoff_notices
                            .extend(effects.into_iter().filter_map(|effect| match effect {
                                mob_dsl::MobMachineEffect::EmitKickoffLifecycleNotice {
                                    intent,
                                    ..
                                } => Some(Self::kickoff_notice_intent(intent)),
                                _ => None,
                            }));
                        self.retirement_observe_endpoints(continuation, true).await;
                    }
                }
            }),
            RetirementObservation::TrustRemoved(result) => boxed_arm_future(move || async move {
                if let Err(error) = result {
                    self.finish_retirement(continuation, Err(error)).await;
                } else {
                    self.retirement_next_remote_edge(continuation).await;
                }
            }),
            RetirementObservation::RemoteEdge(realized) => boxed_arm_future(move || async move {
                match self
                    .commit_realized_wiring(realized)
                    .await
                    .map_err(Self::retirement_unwire_error)
                {
                    Ok(()) => self.retirement_next_remote_edge(continuation).await,
                    Err(error) => self.finish_retirement(continuation, Err(error)).await,
                }
            }),
            #[cfg(not(target_arch = "wasm32"))]
            RetirementObservation::AcceptorRemoved { state, result } => {
                boxed_arm_future(move || async move {
                    self.controlling_acceptor = Some(state);
                    match result {
                        Ok(()) => self.retirement_prepare_supervisor(continuation).await,
                        Err(error) => self.finish_retirement(continuation, Err(error)).await,
                    }
                })
            }
            RetirementObservation::AttachmentsReleased(result) => {
                boxed_arm_future(move || async move {
                    let result = match result {
                        Err(error) => Err(error),
                        Ok(()) => match (
                            continuation.disposal.as_ref(),
                            continuation.archive_disposal,
                        ) {
                            (Some(ctx), Some(disposal)) => {
                                self.observe_member_retirement_archived(ctx, disposal).await
                            }
                            _ => Err(MobError::Internal(
                                "retirement lost its typed archive disposition".into(),
                            )),
                        },
                    };
                    match result {
                        Ok(()) => {
                            continuation.terminal_published = true;
                            self.retirement_terminal_carrier_cleanup(continuation).await;
                        }
                        Err(error) => self.finish_retirement(continuation, Err(error)).await,
                    }
                })
            }
            RetirementObservation::CarrierDeleted { obligation, result } => {
                boxed_arm_future(move || async move {
                    let result = result.and_then(|()| {
                        self.apply_dsl_input(
                            mob_dsl::MobMachineInput::ResolvePlacedCarrierCleanup { obligation },
                            "retired_placed_carrier_cleanup",
                        )
                    });
                    match result {
                        Ok(()) => self.retirement_clean_projection(continuation),
                        Err(error) => {
                            self.durable_uncertainty_fail_stop = true;
                            self.finish_retirement(continuation, Err(error)).await;
                        }
                    }
                })
            }
            RetirementObservation::ProjectionCleaned(Err(error)) => {
                boxed_arm_future(move || async move {
                    self.finish_retirement(continuation, Err(error)).await;
                })
            }
            RetirementObservation::ProjectionCleaned(Ok(())) => {
                boxed_arm_future(move || async move {
                    self.per_spawn_external_tools
                        .write()
                        .await
                        .remove(&continuation.entry.agent_identity);
                    self.roster
                        .write()
                        .await
                        .remove_member(&continuation.entry.agent_identity);
                    self.reachability_observations
                        .clear_member(&continuation.entry.agent_identity);
                    if matches!(
                        continuation.reply,
                        RetirementReply::Retire(_) | RetirementReply::BatchRetire(_)
                    ) {
                        self.remote_flow_tickets
                            .drop_lane(&continuation.entry.agent_identity);
                    }
                    self.finish_retirement(continuation, Ok(())).await;
                })
            }
            RetirementObservation::SupervisorRevoked { peer, result } => {
                boxed_arm_future(move || async move {
                    self.retirement_supervisor_settled(continuation, peer, result)
                        .await;
                })
            }
            RetirementObservation::SupervisorTrustRemoved(result) => {
                boxed_arm_future(move || async move {
                    match result {
                        Ok(()) => self.retirement_release_attachments(continuation).await,
                        Err(error) => self.finish_retirement(continuation, Err(error)).await,
                    }
                })
            }
            RetirementObservation::TrustEndpoints(observed) => {
                boxed_arm_future(move || async move {
                    self.retirement_trust_observed(continuation, observed).await;
                })
            }
            RetirementObservation::NotificationEndpoints(observed) => {
                boxed_arm_future(move || async move {
                    self.retirement_notifications(continuation, observed).await;
                })
            }
            RetirementObservation::ExternalCommsObserved(comms) => {
                boxed_arm_future(move || async move {
                    continuation.external_comms = comms;
                    self.retirement_next_external_edge(continuation).await;
                })
            }
            RetirementObservation::ExternalTrustRemoved { edge, result } => {
                boxed_arm_future(move || async move {
                    self.retirement_external_edge_removed(continuation, edge, result)
                        .await;
                })
            }
            RetirementObservation::ExternalTrustRestored { original, result } => {
                boxed_arm_future(move || async move {
                    let error = match result {
                        Ok(()) => original,
                        Err(error) => MobError::WiringError(format!(
                            "retiring external event append failed: {original}; trust rollback failed: {error}",
                        )),
                    };
                    self.finish_retirement(continuation, Err(error)).await;
                })
            }
            RetirementObservation::PeerDeliveriesSettled(result) => {
                boxed_arm_future(move || async move {
                    self.drain_completed_peer_delivery_tasks();
                    match result {
                        Ok(()) => self.retirement_next_placed_edge(continuation).await,
                        Err(error) => {
                            self.durable_uncertainty_fail_stop = true;
                            self.finish_retirement(continuation, Err(error)).await;
                        }
                    }
                })
            }
            RetirementObservation::PendingAnchorsRetried(results) => {
                boxed_arm_future(move || async move {
                    self.retirement_pending_anchors_retried(continuation, results)
                        .await;
                })
            }
            RetirementObservation::PendingTaskObserved => boxed_arm_future(move || async move {
                self.retirement_observe_pending_task(continuation).await;
            }),
            RetirementObservation::PendingAnchorAborted { anchor, result } => {
                boxed_arm_future(move || async move {
                    self.retirement_pending_anchor_aborted(continuation, anchor, result)
                        .await;
                })
            }
            RetirementObservation::PendingRemoteCleaned { obligation, result } => {
                boxed_arm_future(move || async move {
                    let result = result.and_then(|()| {
                        self.apply_dsl_input(
                            mob_dsl::MobMachineInput::ResolvePlacedCarrierCleanup { obligation },
                            "cancel_pending_spawns_for_member_remote",
                        )
                    });
                    match result {
                        Ok(()) => self.retirement_finish_pending_slot(continuation).await,
                        Err(error) => {
                            self.durable_uncertainty_fail_stop = true;
                            self.finish_retirement(continuation, Err(error)).await;
                        }
                    }
                })
            }
            RetirementObservation::PendingRemoteReleased { cleanup, result } => {
                boxed_arm_future(move || async move {
                    self.retirement_pending_remote_released(continuation, cleanup, result)
                        .await;
                })
            }
        }
    }

    fn dispatch_retirement(
        &mut self,
        mut continuation: RetirementContinuation,
        context: &'static str,
        effect: impl std::future::Future<Output = RetirementObservation>
        + super::member_effect_lane::MemberEffectSend
        + 'static,
    ) {
        let identity = continuation.entry.agent_identity.clone();
        let ticket = self.next_retirement_ticket;
        self.next_retirement_ticket = self.next_retirement_ticket.wrapping_add(1);
        continuation.ticket = ticket;
        let mut members = vec![MemberIncarnationFence::from_entry(&continuation.entry)];
        members.extend(
            continuation
                .participants
                .iter()
                .filter(|fence| fence.identity != continuation.entry.agent_identity)
                .cloned(),
        );
        self.retirements.insert(identity.clone(), continuation);
        let commit_identity = identity.clone();
        self.dispatch_member_effect(MemberEffectRequest {
            context,
            members: members.into_iter().map(MemberFence::Exact).collect(),
            effects: Box::pin(async move {
                Box::new(RetirementCommit {
                    identity: commit_identity,
                    ticket,
                    observation: Some(effect.await),
                }) as Box<dyn MemberEffectCommit>
            }),
            unsettled_commit: Box::new(RetirementCommit {
                identity,
                ticket,
                observation: None,
            }),
        });
    }

    fn finish_retirement(
        &mut self,
        mut continuation: RetirementContinuation,
        result: Result<(), MobError>,
    ) -> ActorCommandFuture<'_, ()> {
        boxed_arm_future(move || async move {
            if let Some(rollback) = continuation.rollback.as_mut() {
                if let Err(error) = result {
                    rollback.failure = Some(error);
                    rollback.compensating = true;
                    boxed_arm_future(|| self.compensate_spawn_rollback(continuation)).await;
                } else {
                    let custody = rollback.custody.clone();
                    self.finish_spawn_rollback_attempt(&custody, None, Ok(()))
                        .await;
                }
                return;
            }
            if self.respawn_topology_reply_withheld || self.durable_uncertainty_fail_stop {
                self.retirements
                    .insert(continuation.entry.agent_identity.clone(), continuation);
                return;
            }
            match continuation.reply {
                RetirementReply::Retire(reply) | RetirementReply::BatchRetire(reply) => {
                    let _ = reply.send(result);
                }
                RetirementReply::Respawn {
                    snapshot,
                    replacement,
                    operation_owner,
                    reply,
                } => {
                    if let Err(error) = result {
                        let origin = RespawnOrigin {
                            old_runtime_id: snapshot.old_runtime_id,
                            old_fence_token: snapshot.old_fence_token,
                        };
                        if self
                            .durably_abandon_respawn_topology_if_terminal_exact(
                                &continuation.entry.agent_identity,
                                &origin,
                            )
                            .await
                            .is_err()
                        {
                            self.durable_uncertainty_fail_stop = true;
                            self.respawn_topology_reply_withheld = true;
                            return;
                        }
                        let _ = reply.send(Err(super::super::handle::MobRespawnError::from(error)));
                        return;
                    }
                    let identity = continuation.entry.agent_identity;
                    let (spawn_reply, spawn_result) = oneshot::channel();
                    let restore_wiring = (!snapshot.restore_wiring.local_peers.is_empty()
                        || !snapshot.restore_wiring.external_peers.is_empty())
                    .then_some(snapshot.restore_wiring);
                    let origin = Some(RespawnOrigin {
                        old_runtime_id: snapshot.old_runtime_id,
                        old_fence_token: snapshot.old_fence_token,
                    });
                    let (owner_bridge_session_id, ops_registry) = operation_owner
                        .map_or((None, None), |(owner, registry)| {
                            (Some(owner), Some(registry))
                        });
                    #[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
                    if replacement.placement.is_some() {
                        boxed_arm_future(|| {
                            self.enqueue_spawn_remote(
                                *replacement,
                                None,
                                None,
                                origin,
                                restore_wiring,
                                spawn_reply,
                            )
                        })
                        .await;
                    } else {
                        boxed_arm_future(|| {
                            self.enqueue_spawn_with_origin(
                                *replacement,
                                SpawnEnqueueOrigin::Respawn {
                                    origin,
                                    restore_wiring,
                                },
                                owner_bridge_session_id,
                                ops_registry,
                                spawn_reply,
                            )
                        })
                        .await;
                    }
                    #[cfg(not(all(feature = "runtime-adapter", not(target_arch = "wasm32"))))]
                    boxed_arm_future(|| {
                        self.enqueue_spawn_with_origin(
                            *replacement,
                            SpawnEnqueueOrigin::Respawn {
                                origin,
                                restore_wiring,
                            },
                            owner_bridge_session_id,
                            ops_registry,
                            spawn_reply,
                        )
                    })
                    .await;
                    if !self.respawn_topology_reply_withheld {
                        let roster = self.roster.clone();
                        self.actor_io_tasks.spawn(async move {
                            if let Some(result) = Self::complete_respawn(
                                roster,
                                identity,
                                snapshot.old_fence_token,
                                spawn_result,
                            )
                            .await
                            {
                                let _ = reply.send(result);
                            }
                        });
                    }
                }
                RetirementReply::IdentityReconcile(authority) => {
                    if let Err(error) = result {
                        self.record_identity_reconcile_disposition(
                            &continuation.entry.agent_identity,
                            &authority,
                            identity_actuation_error_disposition(&error),
                        )
                        .await;
                    }
                    self.enqueue_identity_reconcile(continuation.entry.agent_identity);
                }
                RetirementReply::SpawnRollback => {
                    self.durable_uncertainty_fail_stop = true;
                }
            }
        })
    }
}

impl MobActor {
    pub(super) async fn retirement_effect_is_current(
        &self,
        entry: &RosterEntry,
    ) -> Result<(), MobError> {
        let roster = self.roster.read().await;
        if roster.get(&entry.agent_identity).is_some_and(|current| {
            current.agent_runtime_id == entry.agent_runtime_id
                && current.generation == entry.generation
                && current.fence_token == entry.fence_token
                && current.member_ref == entry.member_ref
        }) {
            Ok(())
        } else {
            Err(MobError::StaleMemberOperatorAuthority {
                member_id: entry.agent_identity.clone(),
                reason: "retirement effect no longer owns the exact roster incarnation".to_string(),
            })
        }
    }
}

impl MobActor {
    pub(super) async fn start_spawn_rollback(
        &mut self,
        custody: spawn_activation::SpawnActivationCustody,
        material: spawn_activation::FailedSpawnRollbackMaterial,
        retained: Option<RetirementContinuation>,
    ) -> Result<(), MobError> {
        if self.retirements.contains_key(&custody.agent_identity) {
            let error = MobError::LifecycleOperationPending {
                intent: format!("spawn rollback for {}", custody.agent_identity),
            };
            self.finish_spawn_rollback_attempt(&custody, retained, Err(error))
                .await;
            return Ok(());
        }
        if let Some(continuation) = retained {
            let participants_current = {
                let roster = self.roster.read().await;
                continuation.participants.iter().all(|fence| {
                    roster
                        .get(&fence.identity)
                        .is_some_and(|entry| fence.matches_entry(entry))
                })
            };
            if self
                .retirement_effect_is_current(&continuation.entry)
                .await
                .is_err()
                || !participants_current
                || !self.spawn_rollback_placement_fences_current(&continuation)
                || !continuation.terminal_published
                    && self.retirement_placement_fence(&continuation.entry.agent_identity)
                        != continuation.placement
                || !continuation.retained_outcomes.is_empty()
                || continuation
                    .rollback
                    .as_ref()
                    .is_some_and(|rollback| rollback.unsettled)
            {
                self.finish_spawn_rollback_attempt(
                    &custody,
                    Some(continuation),
                    Err(MobError::StaleMemberOperatorAuthority {
                        member_id: custody.agent_identity.clone(),
                        reason: "retained rollback cannot prove the predecessor is still current"
                            .into(),
                    }),
                )
                .await;
                return Ok(());
            }
            self.drive_spawn_rollback(continuation).await;
            return Ok(());
        }
        let entry = self
            .roster
            .read()
            .await
            .get(&custody.agent_identity)
            .cloned()
            .filter(|entry| {
                entry.generation == custody.generation
                    && entry.fence_token == custody.fence_token
                    && entry.agent_runtime_id == custody.agent_runtime_id
                    && entry.member_ref == Self::sanitized_member_ref(&custody.member_ref)
            })
            .ok_or_else(|| MobError::StaleMemberOperatorAuthority {
                member_id: custody.agent_identity.clone(),
                reason: "spawn rollback requires its exact roster and physical owner".into(),
            })?;
        let mut cleanup_peers = material.successful_wiring_targets.clone();
        cleanup_peers.extend(material.planned_wiring_targets.iter().cloned());
        cleanup_peers.retain(|peer| peer != &custody.agent_identity);
        cleanup_peers.sort();
        cleanup_peers.dedup();
        let resumed = matches!(
            material.session_origin,
            super::super::provisioner::ProvisionSessionOrigin::ResumedDurable
                | super::super::provisioner::ProvisionSessionOrigin::RevivedRetired
        );
        let placement = self.retirement_placement_fence(&custody.agent_identity);
        let continuation = Box::new(RetirementState {
            ticket: 0,
            entry,
            preserve_binding: resumed,
            preserve_topology: false,
            deadline: Instant::now() + Duration::from_secs(30),
            admission: None,
            reply: RetirementReply::SpawnRollback,
            detach: Vec::new(),
            retired_comms: None,
            routes: Vec::new(),
            retirement_started: false,
            terminal_published: false,
            disposal: None,
            placed_peers: Vec::new(),
            remote_peers: Vec::new(),
            archive_disposal: None,
            participants: Vec::new(),
            external_edges: Vec::new(),
            external_comms: None,
            kickoff_notices: Vec::new(),
            placement,
            release_reservation: None,
            retained_outcomes: Vec::new(),
            pending_next: PendingCleanupNext::Disposal,
            pending_slots: VecDeque::new(),
            pending_slot: None,
            pending_errors: Vec::new(),
            pending_retire_incarnation: None,
            rollback: Some(Box::new(SpawnRollbackState {
                custody,
                material,
                phase: if resumed {
                    SpawnRollbackPhase::CaptureResume
                } else {
                    SpawnRollbackPhase::Endpoints
                },
                endpoints: BTreeMap::new(),
                notices: Vec::new(),
                notice_index: 0,
                trust_index: 0,
                trust_retry: false,
                cleanup_peers,
                placed_cleanup_index: 0,
                placed_edges_cleaned: BTreeSet::new(),
                placement_fences: BTreeMap::new(),
                resume_authority: None,
                failure: None,
                compensating: false,
                peer_description: String::new(),
                sender: None,
                unsettled: false,
            })),
        });
        self.drive_spawn_rollback(continuation).await;
        Ok(())
    }

    fn spawn_rollback_retire_input(
        &self,
        continuation: &RetirementState,
    ) -> Result<SpawnRollbackRetirePlan, MobError> {
        let entry = &continuation.entry;
        let state = self.dsl_authority.state();
        let session =
            state
                .member_session_bindings
                .get(&mob_dsl::AgentIdentity::from_domain(&entry.agent_identity))
                .or_else(|| {
                    state.runtime_retire_pending_sessions.get(
                        &mob_dsl::AgentRuntimeId::from_domain(&entry.agent_runtime_id),
                    )
                })
                .cloned();
        let releasing = if continuation.preserve_binding {
            None
        } else {
            session.clone()
        };
        let journal = self.retirement_started_journal_kind(
            entry,
            releasing.as_ref(),
            session.as_ref(),
            false,
        )?;
        Ok(SpawnRollbackRetirePlan {
            input: mob_dsl::MobMachineInput::Retire {
                mob_id: mob_dsl::MobId::from_domain(&self.definition.id),
                agent_runtime_id: mob_dsl::AgentRuntimeId::from_domain(&entry.agent_runtime_id),
                agent_identity: mob_dsl::AgentIdentity::from_domain(&entry.agent_identity),
                generation: mob_dsl::Generation::from_domain(entry.generation),
                releasing,
                session_id: session.clone(),
            },
            journal,
            session,
        })
    }

    fn spawn_rollback_placement_fences_current(&self, continuation: &RetirementState) -> bool {
        continuation.rollback.as_ref().is_none_or(|rollback| {
            rollback
                .placement_fences
                .iter()
                .all(|(identity, expected)| {
                    continuation.terminal_published
                        && identity == &continuation.entry.agent_identity
                        || self.retirement_placement_fence(identity).as_ref() == expected.as_ref()
                })
        })
    }

    async fn spawn_rollback_next_placed_edge(&mut self, mut continuation: RetirementContinuation) {
        loop {
            let Some(rollback) = continuation.rollback.as_mut() else {
                self.durable_uncertainty_fail_stop = true;
                return;
            };
            let Some(peer) = rollback
                .cleanup_peers
                .get(rollback.placed_cleanup_index)
                .cloned()
            else {
                rollback.phase = SpawnRollbackPhase::Trust;
                Box::pin(self.drive_spawn_rollback(continuation)).await;
                return;
            };
            let retiring = continuation.entry.agent_identity.clone();
            let placed =
                super::super::member_runtime_is_host_owned(self.dsl_authority.state(), &retiring)
                    || super::super::member_runtime_is_host_owned(
                        self.dsl_authority.state(),
                        &peer,
                    );
            if !placed {
                rollback.placed_cleanup_index += 1;
                continue;
            }
            // The shared placement transaction obtains and realizes every
            // surviving host's Remove before committing the unwire. Keep the
            // cursor unchanged on error; an Install outbox row is not cleanup.
            let result = self
                .prepare_member_unwire_for_retirement(retiring, peer.clone(), false)
                .await
                .map_err(Self::retirement_unwire_error);
            match result {
                Ok(wiring_io::WiringPreparation::Settled) => {
                    if let Some(rollback) = continuation.rollback.as_mut() {
                        rollback.placed_edges_cleaned.insert(peer);
                        rollback.placed_cleanup_index += 1;
                    }
                }
                Ok(wiring_io::WiringPreparation::Prepared(prepared)) => {
                    self.dispatch_retirement(
                        continuation,
                        "spawn-rollback-placed-unwire",
                        async move {
                            RetirementObservation::SpawnRollback(
                                SpawnRollbackObservation::PlacedTrust(prepared.realize().await),
                            )
                        },
                    );
                    return;
                }
                Err(error) => {
                    self.finish_retirement(continuation, Err(error)).await;
                    return;
                }
            }
        }
    }

    async fn drive_spawn_rollback(&mut self, mut continuation: RetirementContinuation) {
        if continuation
            .rollback
            .as_ref()
            .is_some_and(|rollback| rollback.compensating)
        {
            self.compensate_spawn_rollback(continuation).await;
            return;
        }
        let result = self.prepare_spawn_rollback_step(&mut continuation);
        match result {
            Ok(Some(effect)) => self.dispatch_retirement(continuation, "spawn-rollback", effect),
            Ok(None) => {
                let phase = continuation
                    .rollback
                    .as_ref()
                    .map(|rollback| rollback.phase);
                match phase {
                    Some(SpawnRollbackPhase::Endpoints) => {
                        self.retirement_observe_endpoints(continuation, false).await;
                    }
                    Some(SpawnRollbackPhase::PlacedTrust) => {
                        self.spawn_rollback_next_placed_edge(continuation).await;
                    }
                    Some(SpawnRollbackPhase::FreshDisposal) => {
                        continuation.deadline = Instant::now() + Duration::from_secs(30);
                        if continuation.terminal_published {
                            self.retirement_terminal_carrier_cleanup(continuation).await;
                        } else if let Some(disposal) = continuation.archive_disposal {
                            self.retirement_post_archive(continuation, disposal).await;
                        } else {
                            self.retirement_detach(continuation).await;
                        }
                    }
                    _ => {
                        self.finish_retirement(
                            continuation,
                            Err(MobError::Internal(
                                "spawn rollback lost its executable stage".into(),
                            )),
                        )
                        .await;
                    }
                }
            }
            Err(error) => self.finish_retirement(continuation, Err(error)).await,
        }
    }

    fn prepare_spawn_rollback_step(
        &mut self,
        continuation: &mut RetirementContinuation,
    ) -> Result<Option<ActorCommandFuture<'static, RetirementObservation>>, MobError> {
        loop {
            let rollback = continuation
                .rollback
                .as_mut()
                .ok_or_else(|| MobError::Internal("missing spawn rollback owner".into()))?;
            let identity = continuation.entry.agent_identity.clone();
            let provisioner = self.provisioner.clone();
            let member = rollback.material.member_ref.clone();
            match rollback.phase {
                SpawnRollbackPhase::CaptureResume => {
                    return Ok(Some(Box::pin(async move {
                        RetirementObservation::SpawnRollback(
                            SpawnRollbackObservation::ResumeAuthority(
                                provisioner
                                    .capture_resumed_member_rollback_authority(&member)
                                    .await,
                            ),
                        )
                    })));
                }
                SpawnRollbackPhase::Endpoints
                | SpawnRollbackPhase::PlacedTrust
                | SpawnRollbackPhase::FreshDisposal => {
                    return Ok(None);
                }
                SpawnRollbackPhase::Journal => {
                    let endpoint = rollback
                        .endpoints
                        .get(&identity)
                        .map(|endpoint| endpoint.spec.clone());
                    let plan = self.spawn_rollback_retire_input(continuation)?;
                    let prepared = self.prepare_dsl_input_transition(
                        plan.input,
                        "rollback_failed_spawn_prepare_retiring_before_cleanup",
                    )?;
                    Self::require_member_lifecycle_journal_effect(
                        &prepared.transition,
                        plan.journal,
                        &identity,
                        &continuation.entry.agent_runtime_id,
                        None,
                        continuation.entry.generation,
                        plan.session.clone(),
                        "rollback_failed_spawn_prepare_retiring_before_cleanup",
                    )?;
                    let session = plan
                        .session
                        .map(|session| SessionId::parse(&session.0))
                        .transpose()
                        .map_err(|error| {
                            MobError::Internal(format!(
                                "spawn rollback retirement journal has an invalid session: {error}"
                            ))
                        })?;
                    let desired = MobEventKind::MemberRetirementStarted {
                        agent_identity: identity,
                        agent_runtime_id: continuation.entry.agent_runtime_id.clone(),
                        generation: continuation.entry.generation,
                        role: continuation.entry.role.clone(),
                        releasing: if plan.journal
                            == mob_dsl::MobLifecycleJournalKind::MemberRetirementStartedReleasing
                        {
                            session.clone()
                        } else {
                            None
                        },
                        session_id: session,
                        retiring_peer_endpoint: endpoint,
                        preserve_machine_topology: false,
                    };
                    return Ok(Some(self.spawn_rollback_journal_effect(desired, false)));
                }
                SpawnRollbackPhase::Notices => {
                    if continuation.placement.is_some() {
                        // A host-owned member has no controller-local sender.
                        // Its exact release owns disposal of the remote runtime.
                        rollback.phase = SpawnRollbackPhase::PlacedTrust;
                        continue;
                    }
                    let mut peers = rollback.material.successful_wiring_targets.clone();
                    peers.sort();
                    peers.dedup();
                    let Some(peer) = peers.get(rollback.notice_index) else {
                        rollback.phase = SpawnRollbackPhase::PlacedTrust;
                        continue;
                    };
                    let target = rollback
                        .endpoints
                        .get(peer)
                        .ok_or_else(|| {
                            MobError::WiringError(format!(
                                "spawn rollback lost wired peer '{peer}'"
                            ))
                        })?
                        .spec
                        .clone();
                    let endpoint = rollback.endpoints.get(&identity).ok_or_else(|| {
                        MobError::WiringError(format!("spawn rollback lost sender '{identity}'"))
                    })?;
                    let sender = rollback.sender.clone().ok_or_else(|| {
                        MobError::WiringError(format!(
                            "spawn rollback requires sender runtime for '{identity}'"
                        ))
                    })?;
                    let spec = endpoint.spec.clone();
                    let entry = continuation.entry.clone();
                    return Ok(Some(Box::pin(async move {
                        let result = Self::notify_peer_event_with_spec_owned(
                            "mob.peer_retired",
                            &target,
                            &identity,
                            &entry,
                            &spec,
                            &sender,
                        )
                        .await;
                        RetirementObservation::SpawnRollback(SpawnRollbackObservation::Notice {
                            target,
                            result,
                        })
                    })));
                }
                SpawnRollbackPhase::Trust => {
                    let Some(peer) = rollback
                        .cleanup_peers
                        .get(rollback.trust_index / 2)
                        .cloned()
                    else {
                        rollback.phase = SpawnRollbackPhase::Retire;
                        continue;
                    };
                    if rollback.placed_edges_cleaned.contains(&peer) {
                        rollback.trust_index += 2;
                        continue;
                    }
                    let Some(spawned) = rollback.endpoints.get(&identity).cloned() else {
                        // No endpoint was ever published, so no local trust
                        // effect can have been installed for this attempt.
                        rollback.phase = SpawnRollbackPhase::Retire;
                        continue;
                    };
                    let Some(other) = rollback.endpoints.get(&peer).cloned() else {
                        rollback.trust_index += 2;
                        continue;
                    };
                    let (recipient, removed, removed_identity) = if rollback.trust_index % 2 == 0 {
                        (spawned, other, peer.clone())
                    } else {
                        (other, spawned, identity.clone())
                    };
                    let edge = mob_dsl::WiringEdge::new(
                        mob_dsl::AgentIdentity::from_domain(&identity),
                        mob_dsl::AgentIdentity::from_domain(&peer),
                    );
                    if let Some(comms) = recipient.comms {
                        let handoff = self.authorize_member_trust_cleanup(
                            &edge,
                            "spawn_rollback_trust_cleanup_authority",
                        )?;
                        let key = Self::trusted_peer_removal_key(&removed.spec);
                        let authority = handoff.unwiring_authority_for(&removed_identity, &key)?;
                        let owner = self.dsl_authority.generated_authority_owner_token();
                        return Ok(Some(Box::pin(async move {
                            let result = Self::apply_trusted_peer_remove_with_owner_token(
                                comms.as_ref(),
                                key,
                                authority,
                                &owner,
                            )
                            .await
                            .map(|_| ())
                            .map_err(MobError::from);
                            RetirementObservation::SpawnRollback(SpawnRollbackObservation::Trust(
                                result,
                            ))
                        })));
                    }
                    if recipient.binding.is_some() {
                        self.cleanup_member_machine_wiring_edge(
                            &identity,
                            &peer,
                            "spawn_rollback_peer_only_machine_wiring_cleanup",
                        )?;
                        let overlay = self.mob_peer_overlay_for_recipient(
                            &recipient.spec,
                            "spawn_rollback_peer_only_cleanup",
                        )?;
                        self.record_pending_recipient_trust_obligation(
                            &recipient.spec,
                            "spawn_rollback_peer_only_cleanup",
                        )?;
                        let bridge = self.supervisor_bridge.clone();
                        return Ok(Some(Box::pin(async move {
                            let peer = recipient.spec;
                            let result = match bridge.trust_recipient(&peer).await {
                                Err(error) => RetirementRevokeObservation::InstallUncertain(error),
                                Ok(install) => {
                                    let result = async {
                                        let authority = bridge.authority().await;
                                        let supervisor = bridge.supervisor_spec_for_authority_and_recipient(
                                            &authority, &peer,
                                        ).await?;
                                        let command = super::super::bridge_protocol::BridgeCommand::UnwireMember(
                                            super::super::bridge_protocol::BridgePeerWiringPayload {
                                                supervisor: supervisor.into(), epoch: authority.epoch,
                                                protocol_version: authority.protocol_version,
                                                peer_spec: removed.spec.into(),
                                                mob_peer_overlay: Some(overlay.bridge_handoff()),
                                            },
                                        );
                                        let value = bridge.send_bridge_command(&peer, &command, Duration::from_secs(2)).await?;
                                        if let Some(rejection) = Self::bridge_rejection_reply(command.protocol_version(), &value) {
                                            return Err(Self::bridge_rejection_error(rejection));
                                        }
                                        super::super::bridge_protocol::decode_bridge_payload::<
                                            super::super::bridge_protocol::BridgeAck,
                                        >(&command, value, "spawn rollback peer cleanup").map(|_| ())
                                    }.await;
                                    match result {
                                        Ok(()) => RetirementRevokeObservation::Confirmed,
                                        Err(error) => {
                                            let rollback =
                                                if Self::recipient_trust_was_newly_installed(
                                                    install,
                                                ) {
                                                    bridge.untrust_recipient(&peer).await
                                                } else {
                                                    Ok(())
                                                };
                                            RetirementRevokeObservation::Refused { error, rollback }
                                        }
                                    }
                                }
                            };
                            RetirementObservation::SpawnRollback(
                                SpawnRollbackObservation::RemoteTrust { peer, result },
                            )
                        })));
                    }
                    // Placed endpoints keep physical cleanup in the existing
                    // host-owned retirement path; never claim local trust.
                    rollback.trust_index += 1;
                }
                SpawnRollbackPhase::Retire => {
                    for peer in &rollback.cleanup_peers {
                        if rollback.placed_edges_cleaned.contains(peer) {
                            continue;
                        }
                        self.cleanup_member_machine_wiring_edge(
                            &identity,
                            peer,
                            "spawn_rollback_wiring_cleanup",
                        )?;
                    }
                    let plan = self.spawn_rollback_retire_input(continuation)?;
                    let prepared = self.prepare_dsl_input_transition(
                        plan.input,
                        "rollback_failed_spawn_mark_retiring_after_cleanup",
                    )?;
                    Self::require_member_lifecycle_journal_effect(
                        &prepared.transition,
                        plan.journal,
                        &identity,
                        &continuation.entry.agent_runtime_id,
                        None,
                        continuation.entry.generation,
                        plan.session,
                        "rollback_failed_spawn_mark_retiring_after_cleanup",
                    )?;
                    continuation.detach = crate::generated::protocol_mob_destroying_session_ingress::extract_obligations(
                        &prepared.transition,
                    );
                    self.commit_prepared_dsl_transition(prepared)?;
                    let rollback = continuation.rollback.as_mut().ok_or_else(|| {
                        MobError::Internal("missing rollback after retirement admission".into())
                    })?;
                    if continuation.preserve_binding {
                        if let Some(session) = continuation.entry.bridge_session_id() {
                            self.discard_pending_routed_effects_for_session(session);
                        }
                        rollback.phase = SpawnRollbackPhase::RestoreResume;
                    } else {
                        rollback.phase = SpawnRollbackPhase::FreshDisposal;
                        continuation.disposal = Some(Self::disposal_context_from_entry(
                            &identity,
                            &continuation.entry,
                            RetireTrustCleanupPlan::empty(),
                            false,
                        ));
                    }
                }
                SpawnRollbackPhase::RestoreResume => {
                    let authority = rollback.resume_authority.clone().ok_or_else(|| {
                        MobError::Internal(
                            "resumed rollback lost its exact attachment authority".into(),
                        )
                    })?;
                    let operation = rollback.material.operation_id.clone();
                    let origin = rollback.material.session_origin;
                    return Ok(Some(Box::pin(async move {
                        RetirementObservation::SpawnRollback(SpawnRollbackObservation::Restored(
                            provisioner
                                .restore_resumed_member(&member, &operation, origin, &authority)
                                .await,
                        ))
                    })));
                }
                SpawnRollbackPhase::TerminalJournal => {
                    let desired = MobEventKind::MemberRetired {
                        agent_identity: identity,
                        generation: continuation.entry.generation,
                        role: continuation.entry.role.clone(),
                    };
                    return Ok(Some(self.spawn_rollback_journal_effect(desired, true)));
                }
                SpawnRollbackPhase::Projection => {
                    let metadata = self.runtime_metadata.clone();
                    let mob = self.definition.id.clone();
                    let generation = continuation.entry.generation;
                    return Ok(Some(Box::pin(async move {
                        RetirementObservation::SpawnRollback(SpawnRollbackObservation::Projection(
                            metadata
                                .delete_external_binding_overlay(&mob, &identity, generation)
                                .await
                                .map_err(MobError::from),
                        ))
                    })));
                }
            }
        }
    }

    fn spawn_rollback_journal_effect(
        &self,
        desired: MobEventKind,
        terminal: bool,
    ) -> ActorCommandFuture<'static, RetirementObservation> {
        let events = self.events.clone();
        let mob = self.definition.id.clone();
        Box::pin(async move {
            let result = async {
                // Retry the exact carrier, including wrote-then-error from a
                // prior attempt, without appending a second terminal event.
                let all = events.replay_all().await?;
                let current = all
                    .iter()
                    .filter(|event| event.mob_id == mob)
                    .collect::<Vec<_>>();
                let epoch = current
                    .iter()
                    .rposition(|event| matches!(event.kind, MobEventKind::MobReset))
                    .map_or(0, |index| index + 1);
                if current[epoch..].iter().any(|event| event.kind == desired) {
                    return Ok(());
                }
                if let Err(error) = events
                    .append(NewMobEvent {
                        mob_id: mob.clone(),
                        timestamp: None,
                        kind: desired.clone(),
                    })
                    .await
                {
                    let all = events.replay_all().await?;
                    let current = all
                        .iter()
                        .filter(|event| event.mob_id == mob)
                        .collect::<Vec<_>>();
                    let epoch = current
                        .iter()
                        .rposition(|event| matches!(event.kind, MobEventKind::MobReset))
                        .map_or(0, |index| index + 1);
                    if !current[epoch..].iter().any(|event| event.kind == desired) {
                        return Err(MobError::from(error));
                    }
                }
                Ok(())
            }
            .await;
            RetirementObservation::SpawnRollback(if terminal {
                SpawnRollbackObservation::TerminalJournal(result)
            } else {
                SpawnRollbackObservation::Journal(result)
            })
        })
    }

    async fn resume_spawn_rollback(
        &mut self,
        mut continuation: RetirementContinuation,
        observation: SpawnRollbackObservation,
    ) {
        let Some(rollback) = continuation.rollback.as_mut() else {
            self.durable_uncertainty_fail_stop = true;
            return;
        };
        let result = match observation {
            SpawnRollbackObservation::ResumeAuthority(result) => result.map(|authority| {
                rollback.resume_authority = Some(authority);
                rollback.phase = SpawnRollbackPhase::Endpoints;
            }),
            SpawnRollbackObservation::Endpoints {
                observed,
                description,
            } => {
                rollback.peer_description = description;
                let mut error = None;
                for (identity, endpoint) in observed.endpoints {
                    match endpoint {
                        Ok(WiringEndpoint::Local { comms, spec, .. }) => {
                            if identity == continuation.entry.agent_identity {
                                rollback.sender = Some(comms.clone());
                            }
                            rollback.endpoints.insert(
                                identity,
                                SpawnRollbackEndpoint {
                                    spec,
                                    comms: Some(comms),
                                    binding: None,
                                },
                            );
                        }
                        Ok(WiringEndpoint::PeerOnly { spec, binding }) => {
                            if identity == continuation.entry.agent_identity
                                && matches!(
                                    rollback.material.member_ref,
                                    MemberRef::BackendPeer {
                                        session_id: None,
                                        ..
                                    }
                                )
                            {
                                rollback.sender = Some(observed.supervisor_comms.clone());
                            }
                            rollback.endpoints.insert(
                                identity,
                                SpawnRollbackEndpoint {
                                    spec,
                                    comms: None,
                                    binding: Some(binding),
                                },
                            );
                        }
                        Ok(WiringEndpoint::Placed { spec, .. }) => {
                            rollback.endpoints.insert(
                                identity,
                                SpawnRollbackEndpoint {
                                    spec,
                                    comms: None,
                                    binding: None,
                                },
                            );
                        }
                        Err(problem) => {
                            if rollback
                                .material
                                .successful_wiring_targets
                                .contains(&identity)
                                || identity == continuation.entry.agent_identity
                                    && !rollback.material.successful_wiring_targets.is_empty()
                            {
                                error.get_or_insert(problem);
                            }
                        }
                    }
                }
                if error.is_none() {
                    rollback.phase = SpawnRollbackPhase::Journal;
                }
                error.map_or(Ok(()), Err)
            }
            SpawnRollbackObservation::Journal(result) => match result {
                Ok(()) => {
                    continuation.retirement_started = true;
                    self.retirement_started_event_index
                        .write()
                        .await
                        .insert(format!(
                            "{}:{}",
                            continuation.entry.agent_identity,
                            continuation.entry.generation.get(),
                        ));
                    rollback.phase = SpawnRollbackPhase::Notices;
                    Ok(())
                }
                Err(error) => Err(error),
            },
            SpawnRollbackObservation::Notice { target, result } => result.map(|()| {
                rollback.notices.push(target);
                rollback.notice_index += 1;
            }),
            SpawnRollbackObservation::Trust(result) => match result {
                Ok(()) => {
                    rollback.trust_retry = false;
                    rollback.trust_index += 1;
                    Ok(())
                }
                Err(_) if !rollback.trust_retry => {
                    rollback.trust_retry = true;
                    Ok(())
                }
                Err(error) => Err(error),
            },
            SpawnRollbackObservation::PlacedTrust(realized) => self
                .commit_realized_wiring(realized)
                .await
                .map_err(Self::retirement_unwire_error)
                .map(|()| {
                    if let Some(peer) = rollback.cleanup_peers.get(rollback.placed_cleanup_index) {
                        rollback.placed_edges_cleaned.insert(peer.clone());
                    }
                    rollback.placed_cleanup_index += 1;
                }),
            SpawnRollbackObservation::RemoteTrust { peer, result } => {
                let result = match result {
                    RetirementRevokeObservation::Confirmed => self
                        .resolve_pending_recipient_trust_obligation(
                            &peer,
                            "spawn_rollback_peer_only_cleanup",
                        ),
                    RetirementRevokeObservation::Refused {
                        error,
                        rollback: Ok(()),
                    } => {
                        let resolved = self.resolve_pending_recipient_trust_obligation(
                            &peer,
                            "spawn_rollback_peer_only_cleanup_refused",
                        );
                        resolved.and(Err(error))
                    }
                    RetirementRevokeObservation::Refused {
                        error,
                        rollback: Err(cleanup),
                    } => Err(MobError::RetirementTopologyIncomplete(format!(
                        "{error}; recipient cleanup retained: {cleanup}"
                    ))),
                    RetirementRevokeObservation::InstallUncertain(error) => Err(error),
                };
                result.map(|()| rollback.trust_index += 1)
            }
            SpawnRollbackObservation::Restored(result) => result.map(|()| {
                rollback.phase = SpawnRollbackPhase::TerminalJournal;
            }),
            SpawnRollbackObservation::TerminalJournal(result) => match result {
                Err(error) => Err(error),
                Ok(()) => {
                    self.retired_event_index
                        .write()
                        .await
                        .insert(Self::retire_event_key(
                            &continuation.entry.agent_identity,
                            continuation.entry.generation,
                        ));
                    self.apply_dsl_signal(
                        mob_dsl::MobMachineSignal::RecoverRosterMemberRetired {
                            agent_identity: mob_dsl::AgentIdentity::from_domain(
                                &continuation.entry.agent_identity,
                            ),
                            agent_runtime_id: mob_dsl::AgentRuntimeId::from_domain(
                                &continuation.entry.agent_runtime_id,
                            ),
                            generation: mob_dsl::Generation::from_domain(
                                continuation.entry.generation,
                            ),
                            preserve_machine_topology: false,
                            preservation_started: false,
                        },
                        "rollback_resumed_spawn_membership",
                    )
                    .map(|()| {
                        continuation.terminal_published = true;
                        rollback.phase = SpawnRollbackPhase::Projection;
                    })
                }
            },
            SpawnRollbackObservation::Projection(result) => {
                match result {
                    Ok(()) => {
                        self.roster
                            .write()
                            .await
                            .remove_member(&continuation.entry.agent_identity);
                        self.per_spawn_external_tools
                            .write()
                            .await
                            .remove(&continuation.entry.agent_identity);
                        self.restore_diagnostics
                            .write()
                            .await
                            .remove(&continuation.entry.agent_identity);
                        self.finish_retirement(continuation, Ok(())).await;
                    }
                    Err(error) => self.finish_retirement(continuation, Err(error)).await,
                }
                return;
            }
            SpawnRollbackObservation::Compensated(result) => {
                match result {
                    Ok(())
                    | Err(MobError::CommsError(meerkat_core::comms::SendError::PeerNotFound(_))) => {
                        rollback.notices.pop();
                        self.compensate_spawn_rollback(continuation).await;
                    }
                    Err(error) => {
                        let custody = rollback.custody.clone();
                        let primary = rollback
                            .failure
                            .take()
                            .map_or_else(String::new, |error| error.to_string());
                        rollback.failure = Some(MobError::Internal(format!(
                            "{primary}; peer notice compensation retained: {error}"
                        )));
                        self.finish_spawn_rollback_attempt(
                            &custody,
                            Some(continuation),
                            Err(error),
                        )
                        .await;
                    }
                }
                return;
            }
        };
        match result {
            Ok(()) => self.drive_spawn_rollback(continuation).await,
            Err(error) => self.finish_retirement(continuation, Err(error)).await,
        }
    }

    async fn compensate_spawn_rollback(&mut self, mut continuation: RetirementContinuation) {
        let Some(rollback) = continuation.rollback.as_mut() else {
            self.durable_uncertainty_fail_stop = true;
            return;
        };
        if let Some(target) = rollback.notices.last().cloned()
            && let Some(endpoint) = rollback.endpoints.get(&continuation.entry.agent_identity)
            && let Some(sender) = rollback.sender.clone()
        {
            let identity = continuation.entry.agent_identity.clone();
            let role = continuation.entry.role.clone();
            let description = rollback.peer_description.clone();
            let spec = endpoint.spec.clone();
            self.dispatch_retirement(
                continuation,
                "spawn-rollback-notice-compensation",
                async move {
                    let result = async {
                        let params = meerkat_contracts::CommsPeerLifecycleParams {
                            peer: identity.to_string(),
                            role: Some(role.to_string()),
                            description: Some(description),
                            peer_spec: Some(spec.into()),
                        };
                        let params = serde_json::to_value(params)
                            .map_err(|error| MobError::WiringError(error.to_string()))?;
                        sender
                            .send(CommsCommand::PeerLifecycle {
                                to: PeerRoute::with_display_name(target.peer_id, target.name),
                                kind: PeerLifecycleKind::PeerAdded,
                                params,
                            })
                            .await?;
                        Ok(())
                    }
                    .await;
                    RetirementObservation::SpawnRollback(SpawnRollbackObservation::Compensated(
                        result,
                    ))
                },
            );
            return;
        }
        let custody = rollback.custody.clone();
        let error = rollback.failure.take().unwrap_or_else(|| {
            MobError::Internal("spawn rollback compensation has no owner result".into())
        });
        rollback.compensating = false;
        self.finish_spawn_rollback_attempt(&custody, Some(continuation), Err(error))
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{FenceToken, Generation};

    #[tokio::test]
    async fn respawn_receipt_matches_sanitized_peer_address_without_accepting_a_successor() {
        let identity = AgentIdentity::from("respawn-receipt-peer");
        let raw_ref = MemberRef::BackendPeer {
            peer_id: "retirement-receipt-peer".to_string(),
            address: "tcp://127.0.0.1:9000?mob_supervisor_bootstrap_token=test-only&transport=tcp"
                .to_string(),
            pubkey: [7; 32],
            bootstrap_token: None,
            session_id: Some(SessionId::new()),
        };
        let sanitized_ref = MobActor::sanitized_member_ref(&raw_ref);
        assert_ne!(raw_ref, sanitized_ref);
        let generation = Generation::new(1);
        let fence_token = FenceToken::new(2);
        for superseded in [false, true] {
            let mut member_ref = sanitized_ref.clone();
            if superseded && let MemberRef::BackendPeer { session_id, .. } = &mut member_ref {
                *session_id = Some(SessionId::new());
            }
            let entry = RosterEntry {
                agent_identity: identity.clone(),
                generation,
                fence_token,
                agent_runtime_id: AgentRuntimeId::new(identity.clone(), generation),
                role: crate::ids::ProfileName::from("worker"),
                runtime_mode: crate::MobRuntimeMode::TurnDriven,
                member_ref,
                peer_id: None,
                transport_public_key: None,
                direct_member_fence: None,
                wired_to: BTreeSet::new(),
                external_peer_specs: BTreeMap::new(),
                labels: BTreeMap::new(),
                kickoff: None,
                effective_profile_override: None,
                effective_model_override: None,
            };
            let roster = Arc::new(RwLock::new(RosterAuthority::from_roster(
                crate::roster::Roster::from_projected_entries([entry]),
            )));
            let (reply_tx, reply_rx) = oneshot::channel();
            assert!(
                reply_tx
                    .send(Ok(crate::runtime::handle::MemberSpawnReceipt {
                        member_ref: raw_ref.clone(),
                        direct_member_fence: None,
                        operation_id: meerkat_core::ops::OperationId::new(),
                        session_origin: crate::runtime::provisioner::ProvisionSessionOrigin::Fresh,
                        rollback_authority: None,
                        materialized_ack: None,
                        failed_restore_peer_ids: Vec::new(),
                    }))
                    .is_ok()
            );
            let result =
                MobActor::complete_respawn(roster, identity.clone(), FenceToken::new(1), reply_rx)
                    .await
                    .expect("ordinary receipt is observable");
            if superseded {
                assert!(matches!(
                    result,
                    Err(crate::runtime::handle::MobRespawnError::SpawnAfterRetire { .. }),
                ));
            } else {
                let receipt = result.expect("sanitized address is the same incarnation");
                assert_eq!(receipt.fence_token, fence_token);
                assert_eq!(receipt.previous_fence_token, FenceToken::new(1));
            }
        }
    }
}
