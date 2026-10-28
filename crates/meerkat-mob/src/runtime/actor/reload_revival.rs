//! The warm recipe: the in-process reload of a member already built in this
//! process (a durability reload over the exact successor registration the
//! reload published). Its rebuild reuses the overlay this process retained
//! when it built the member, which the host's customizer produced at that
//! build. Every wake of a Dormant member, and every missing-live revival,
//! goes through the cold recipe (`member_wake_turn`), which asks the
//! customizer again.
//!
//! The reload's discard never unregisters the member's registration, so the
//! member is still Live in MobMachine when the revival starts. The revival
//! takes the drift arm (`ClassifyMemberLiveMaterialization`: Live -> Waking
//! under a machine-minted attempt), and every resolution after it is fenced
//! by that attempt: `ResolveMemberRevivalBound` once the provision is built,
//! `ResolveMemberRevivalSucceeded` after readiness and topology,
//! `ResolveMemberRevival{Failed,HostRefused}` or `AbandonMemberWake` when it
//! ends otherwise. The machine answers each with a verdict: the provision is
//! committed only on an applied Bound and rolled back on a stale one, and no
//! verdict fail-stops the mob.
//!
//! The revival uses the same actor-owned effect lane as other member work.
//! Only owned I/O material leaves the actor; generated admission and
//! publication remain actor commits.

use super::member_effect_lane::{
    MemberEffectAck, MemberEffectCommit, MemberEffectCommitFuture, MemberEffectRequest,
    MemberEffectRetention, MemberEffectSend, MemberEffectSettlement, MemberFence,
};
use super::*;

#[cfg(test)]
type ReadinessTestBarrier = (oneshot::Sender<()>, oneshot::Receiver<()>);

#[cfg(test)]
static READINESS_TEST_BARRIERS: std::sync::LazyLock<
    std::sync::Mutex<HashMap<SessionId, ReadinessTestBarrier>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(HashMap::new()));

#[cfg(test)]
pub(in crate::runtime) fn pause_before_readiness_for_test(
    session_id: SessionId,
) -> (oneshot::Receiver<()>, oneshot::Sender<()>) {
    let (entered_tx, entered_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    assert!(
        READINESS_TEST_BARRIERS
            .lock()
            .expect("readiness barrier")
            .insert(session_id, (entered_tx, release_rx))
            .is_none()
    );
    (entered_rx, release_tx)
}

struct RevivalWork {
    entry: RosterEntry,
    member_ref: MemberRef,
    session_id: SessionId,
    scope: MemberLiveRevivalScope,
    reply: std::sync::Mutex<Option<oneshot::Sender<Result<MemberLiveRevivalOutcome, MobError>>>>,
    /// The wake attempt the drift arm minted for this revival once it
    /// classified the member (a revival that resumes an earlier publication
    /// mints none). Every resolution of the revival is fenced by it.
    attempt: std::sync::OnceLock<u64>,
    /// What rolls back the provision this revival committed on its Bound
    /// verdict, until its Ready verdict: a later stale or failed step rolls
    /// it back, so no materialization outlives the attempt that made it.
    committed: std::sync::Mutex<Option<WarmRollback>>,
}

impl RevivalWork {
    fn reply(&self, result: Result<MemberLiveRevivalOutcome, MobError>) {
        if let Some(reply) = self
            .reply
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            let _ = reply.send(result);
        }
    }

    fn attempt(&self) -> Option<u64> {
        self.attempt.get().copied()
    }

    fn take_committed(&self) -> Option<WarmRollback> {
        self.committed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }
}

/// The receipt facts that roll a committed revival provision back, kept
/// after its `PendingProvision` was committed.
#[derive(Clone)]
struct WarmRollback {
    member_ref: MemberRef,
    operation_id: meerkat_core::ops::OperationId,
    session_origin: super::super::provisioner::ProvisionSessionOrigin,
    rollback_authority: Option<super::super::provisioner::ResumedMemberRollbackAuthority>,
}

impl WarmRollback {
    fn pending(
        self,
        identity: &AgentIdentity,
        provisioner: Arc<dyn MobProvisioner>,
    ) -> PendingProvision {
        PendingProvision::new(
            self.member_ref,
            identity.clone(),
            provisioner,
            self.operation_id,
            self.session_origin,
            self.rollback_authority,
            // A failed warm revival keeps the member seated.
            super::super::provisioner::RollbackOrigin::RevivalOrRebuild,
        )
    }
}

/// How a revival finishes once the provision it committed is rolled back.
enum WarmRollbackThen {
    /// It is not current any more: a stale fence, or a stale verdict.
    NotCurrent,
    /// A step after the provision failed: the attempt ends with `kind`, and
    /// the caller gets `error`.
    Failed {
        kind: mob_dsl::MemberWakeAbandonKind,
        error: MobError,
    },
}

#[derive(Default)]
struct RevivalResources {
    #[cfg(feature = "runtime-adapter")]
    claim: Option<meerkat_runtime::PreparedSessionMaterialization>,
}

enum RevivalObservation {
    Inert(MemberLiveRevivalOutcome),
    Missing(RevivalResources),
    Present {
        profile: Result<Box<crate::profile::Profile>, MobError>,
        resources: RevivalResources,
    },
}

struct RevivalPublication {
    provision: PendingProvision,
    publication: Option<super::super::provisioner::ResumedMemberRollbackAuthority>,
    rollback: WarmRollback,
}

struct RevivalTopology {
    publication: Option<super::super::provisioner::ResumedMemberRollbackAuthority>,
    remaining: VecDeque<(super::super::handle::PeerTarget, RespawnTopologyPeerId)>,
    failed: Vec<RespawnTopologyPeerId>,
}

enum RevivalStep {
    Observed(Result<RevivalObservation, MobError>),
    Built(Result<RevivalPublication, MobError>),
    Ready {
        publication: Option<super::super::provisioner::ResumedMemberRollbackAuthority>,
        result: Result<(), MobError>,
    },
    Topology {
        topology: RevivalTopology,
        peer: RespawnTopologyPeerId,
        result: Result<(), MobError>,
    },
    Failed(MobError),
    AlreadyLiveAfterBuild,
    Inert(MemberLiveRevivalOutcome),
    /// The committed provision was rolled back (`false`: it could not be
    /// settled and its custody was abandoned); the revival then finishes.
    RolledBack {
        settled: bool,
        then: WarmRollbackThen,
    },
    Unsettled,
}

struct RevivalCommit {
    work: Arc<RevivalWork>,
    step: RevivalStep,
}

impl MemberEffectCommit for RevivalCommit {
    fn commit(
        self: Box<Self>,
        actor: &mut MobActor,
        settlement: MemberEffectSettlement,
    ) -> MemberEffectCommitFuture<'_> {
        Box::pin(async move {
            if actor.durable_uncertainty_fail_stop.is_set() {
                return MemberEffectAck::Retained(MemberEffectRetention::resumable(
                    MobError::Internal("warm revival commit is held by fail-stop".to_string()),
                    self,
                ));
            }
            boxed_arm_future(|| actor.commit_warm_revival(self.work, self.step, settlement)).await;
            if actor.durable_uncertainty_fail_stop.is_set() {
                MemberEffectAck::Retained(MemberEffectRetention::unresumable(MobError::Internal(
                    "warm revival still lacks owner settlement".to_string(),
                )))
            } else {
                MemberEffectAck::Settled
            }
        })
    }
}

impl MobActor {
    pub(super) async fn begin_member_live_revival(
        &mut self,
        identity: AgentIdentity,
        session_id: SessionId,
        scope: MemberLiveRevivalScope,
        reply_tx: oneshot::Sender<Result<MemberLiveRevivalOutcome, MobError>>,
    ) {
        // A missing-live revival is a wake (`WakeMember` with the observed
        // miss, through the cold recipe), and the actor routes it there; only
        // the registration-scoped reload reaches this recipe.
        if scope.is_missing_live() {
            let _ = reply_tx.send(Err(MobError::Internal(format!(
                "the missing-live revival of member '{identity}' is a member wake, not a warm reload"
            ))));
            return;
        }
        let prepared = async {
            let entry = self
                .roster
                .read()
                .await
                .get(&identity)
                .cloned()
                .ok_or_else(|| MobError::MemberNotFound(identity.clone()))?;
            self.ensure_member_not_broken(&identity).await?;
            let member_ref = self.machine_member_ref_for_behavior(&entry, "member turn revival")?;
            if member_ref.bridge_session_id() != Some(&session_id) {
                return Err(MobError::StaleMemberOperatorAuthority {
                    member_id: identity.clone(),
                    reason: "member session changed before live revival".to_string(),
                });
            }
            Ok((entry, member_ref))
        }
        .await;
        let (entry, member_ref) = match prepared {
            Ok(material) => material,
            Err(error) => {
                let _ = reply_tx.send(Err(error));
                return;
            }
        };
        let work = Arc::new(RevivalWork {
            entry,
            member_ref,
            session_id,
            scope,
            reply: std::sync::Mutex::new(Some(reply_tx)),
            attempt: std::sync::OnceLock::new(),
            committed: std::sync::Mutex::new(None),
        });
        #[cfg(feature = "runtime-adapter")]
        if let Some(publication) = work.scope.publication.clone() {
            self.dispatch_warm_readiness(work, Some(publication));
            return;
        }
        let service = Arc::clone(&self.session_service);
        let definition = Arc::clone(&self.definition);
        let profiles = self.realm_profile_store.clone();
        #[cfg(feature = "runtime-adapter")]
        let adapter = self.runtime_adapter.clone();
        let observed_work = Arc::clone(&work);
        self.dispatch_warm_revival(work, "warm_revival_observation", async move {
            let result = async {
                match service.has_live_session(&observed_work.session_id).await {
                    Ok(true) => return Ok(RevivalObservation::Inert(MemberLiveRevivalOutcome::AlreadyLive)),
                    Ok(false) | Err(meerkat_core::service::SessionError::NotFound { .. }) => {}
                    Err(error) => return Err(MobError::SessionError(error)),
                }
                let present = service.supports_persistent_sessions()
                    && !matches!(
                        service.observe_session_resume_authority(&observed_work.session_id).await?.lifecycle(),
                        super::super::session_service::SessionResumeLifecycle::NoCurrentDurableAuthority
                    );
                let profile = if present {
                    Some(match observed_work.entry.effective_profile_override.clone() {
                        Some(profile) => Ok(profile),
                        None => definition.resolve_profile(&observed_work.entry.role, profiles.as_ref()).await,
                    }.map(|mut profile| {
                        if let Some(model) = &observed_work.entry.effective_model_override {
                            profile.model.clone_from(model);
                        }
                        Box::new(profile)
                    }))
                } else {
                    None
                };
                #[cfg(feature = "runtime-adapter")]
                let mut resources = RevivalResources::default();
                #[cfg(not(feature = "runtime-adapter"))]
                let resources = RevivalResources::default();
                #[cfg(feature = "runtime-adapter")]
                if let Some(registration) = &observed_work.scope.registration {
                    let adapter = adapter.as_ref().ok_or_else(|| MobError::MemberReloadRefused {
                        session_id: observed_work.session_id.clone(),
                        reason: "reload materialization has no runtime owner".to_string(),
                    })?;
                    resources.claim = Some(match adapter
                        .prepare_local_session_materialization_for_registration(registration.clone()).await
                    {
                        Ok(prepared) => prepared,
                        Err(meerkat_runtime::RuntimeBindingsError::RegistrationNotCurrent(_)) => {
                            return Ok(RevivalObservation::Inert(MemberLiveRevivalOutcome::NotCurrent));
                        }
                        Err(meerkat_runtime::RuntimeBindingsError::RegistrationOwned(_)) => {
                            return Ok(RevivalObservation::Inert(MemberLiveRevivalOutcome::CurrentButOwned));
                        }
                        Err(error) => return Err(MobError::MemberReloadRefused {
                            session_id: observed_work.session_id.clone(),
                            reason: error.to_string(),
                        }),
                    });
                }
                Ok(match profile {
                    Some(profile) => RevivalObservation::Present { profile, resources },
                    None => RevivalObservation::Missing(resources),
                })
            }.await;
            RevivalStep::Observed(result)
        });
    }

    fn dispatch_warm_revival<F>(
        &mut self,
        work: Arc<RevivalWork>,
        context: &'static str,
        effects: F,
    ) where
        F: std::future::Future<Output = RevivalStep> + MemberEffectSend + 'static,
    {
        let worker_work = Arc::clone(&work);
        self.dispatch_member_wake_effect(MemberEffectRequest {
            context,
            members: vec![MemberFence::exact(&work.entry)],
            effects: Box::pin(async move {
                Box::new(RevivalCommit {
                    work: worker_work,
                    step: effects.await,
                }) as Box<dyn MemberEffectCommit>
            }),
            unsettled_commit: Box::new(RevivalCommit {
                work,
                step: RevivalStep::Unsettled,
            }),
        });
    }

    async fn commit_warm_revival(
        &mut self,
        work: Arc<RevivalWork>,
        step: RevivalStep,
        settlement: MemberEffectSettlement,
    ) {
        if settlement.unsettled.is_some() || matches!(step, RevivalStep::Unsettled) {
            // Whether the effects took place is unknown, so the mob
            // fail-stops; the revival's attempt is closed first, explicitly,
            // so no caller waits on it.
            self.abandon_warm_revival(
                &work,
                mob_dsl::MemberWakeAbandonKind::Unsettled,
                "warm revival effect ended without owned settlement evidence".to_string(),
            )
            .await;
            let fail_stop_reply = self
                .durable_uncertainty_fail_stop
                .record(MobError::Internal(
                    "warm revival effect ended without owned settlement evidence".to_string(),
                ));
            work.reply(Err(fail_stop_reply));
            return;
        }
        if !settlement.fence.is_current()
            && matches!(
                &step,
                RevivalStep::Observed(Err(_)) | RevivalStep::Built(Err(_)) | RevivalStep::Failed(_)
            )
        {
            self.finish_warm_revival_inert(&work, MemberLiveRevivalOutcome::NotCurrent)
                .await;
            return;
        }
        match step {
            RevivalStep::Observed(Ok(RevivalObservation::Inert(outcome))) => {
                self.finish_warm_revival_inert(&work, outcome).await;
            }
            RevivalStep::Observed(Err(error)) | RevivalStep::Failed(error) => {
                self.finish_warm_revival_failure(&work, error).await;
            }
            RevivalStep::Inert(outcome) => self.finish_warm_revival_inert(&work, outcome).await,
            RevivalStep::Observed(Ok(observation)) => {
                let (profile, resources) = match observation {
                    RevivalObservation::Present { profile, resources } => {
                        (Some(profile.map(|profile| *profile)), resources)
                    }
                    RevivalObservation::Missing(resources) => (None, resources),
                    RevivalObservation::Inert(_) => return,
                };
                if !settlement.fence.is_current() {
                    self.cleanup_warm_claim(work, resources, None);
                    return;
                }
                let recipe = match self.prepare_warm_revival_recipe(&work, profile).await {
                    Ok(Some(recipe)) => recipe,
                    // Another wake holds the member: it materializes the
                    // member, this reload does not.
                    Ok(None) => {
                        self.cleanup_warm_claim(work, resources, None);
                        return;
                    }
                    Err(error) => {
                        self.cleanup_warm_claim(work, resources, Some(error));
                        return;
                    }
                };
                let service = Arc::clone(&self.session_service);
                let provisioner = Arc::clone(&self.provisioner);
                #[cfg(feature = "runtime-adapter")]
                let adapter = self.runtime_adapter.clone();
                let worker_work = Arc::clone(&work);
                self.dispatch_warm_revival(work, "warm_revival_construction", async move {
                    #[cfg(feature = "runtime-adapter")]
                    let mut resources = resources;
                    #[cfg(feature = "runtime-adapter")]
                    if worker_work.scope.registration.is_none()
                        && let Some(adapter) = adapter
                    {
                        match adapter.update_peer_ingress_context(&worker_work.session_id, false, None).await {
                            Ok(_) | Err(
                                meerkat_runtime::RuntimeDriverError::NotFound { .. }
                                | meerkat_runtime::RuntimeDriverError::Destroyed
                                | meerkat_runtime::RuntimeDriverError::NotReady { .. }
                            ) => {}
                            Err(error) => return cleanup_resources(resources, Some(MobError::Internal(
                                format!("detach stale warm-revival ingress failed: {error}"),
                            ))).await,
                        }
                    }
                    let requested_auth_binding = recipe.auth_binding.clone();
                    let request = match recipe.into_request(Arc::clone(&service)).await {
                        Ok(mut request) => {
                            request.runtime_revival_intent =
                                super::super::provisioner::RuntimeRevivalIntent::MissingLiveMaterialization;
                            request
                        }
                        Err(error) => return cleanup_resources(resources, Some(error)).await,
                    };
                    #[cfg(feature = "runtime-adapter")]
                    let receipt = match resources.claim.take() {
                        Some(claim) => provisioner.provision_member_from_reload_claim(request, claim).await,
                        None => provisioner.provision_member(request).await,
                    };
                    #[cfg(not(feature = "runtime-adapter"))]
                    let receipt = provisioner.provision_member(request).await;
                    if let Err(error) = &receipt
                        && revival_error_means_session_already_live(error, &worker_work.session_id)
                    {
                        match service.has_live_session(&worker_work.session_id).await {
                            Ok(true) => {
                                // The live actor is kept only when it runs the
                                // binding the rebuild names, as every reuse of
                                // a live actor (`live_actor_binding`); otherwise
                                // the reload fails with the check's error and
                                // the actor is retired, not reused.
                                let Some(requested) = requested_auth_binding.as_ref() else {
                                    return RevivalStep::AlreadyLiveAfterBuild;
                                };
                                let Err(refused) = super::super::live_actor_binding::verify_live_actor_auth_binding(
                                    service.as_ref(),
                                    &worker_work.entry.agent_identity,
                                    &worker_work.session_id,
                                    requested,
                                )
                                .await
                                else {
                                    return RevivalStep::AlreadyLiveAfterBuild;
                                };
                                if let Err(error) = super::super::live_actor_binding::retire_conflicting_live_actor(
                                    service.as_ref(),
                                    &worker_work.session_id,
                                )
                                .await
                                {
                                    tracing::error!(
                                        session_id = %worker_work.session_id,
                                        %error,
                                        "a reloaded member's live actor refused by its auth binding check could not be retired"
                                    );
                                }
                                return RevivalStep::Built(Err(refused));
                            }
                            Ok(false) | Err(meerkat_core::service::SessionError::NotFound { .. }) => {}
                            Err(_) => return RevivalStep::Unsettled,
                        }
                    }
                    RevivalStep::Built(receipt.map(|receipt| {
                        let publication = receipt.rollback_authority.clone();
                        let rollback = WarmRollback {
                            member_ref: receipt.member_ref.clone(),
                            operation_id: receipt.operation_id.clone(),
                            session_origin: receipt.session_origin,
                            rollback_authority: receipt.rollback_authority.clone(),
                        };
                        RevivalPublication {
                            provision: PendingProvision::new(
                                receipt.member_ref,
                                worker_work.entry.agent_identity.clone(),
                                provisioner,
                                receipt.operation_id,
                                receipt.session_origin,
                                receipt.rollback_authority,
                                // A failed warm revival keeps the member seated.
                                super::super::provisioner::RollbackOrigin::RevivalOrRebuild,
                            ),
                            publication,
                            rollback,
                        }
                    }))
                });
            }
            RevivalStep::AlreadyLiveAfterBuild => {
                if !settlement.fence.is_current() {
                    self.finish_warm_revival_inert(&work, MemberLiveRevivalOutcome::NotCurrent)
                        .await;
                    return;
                }
                // The session was already live when the build ran: the
                // attempt binds to it (its Bound step), and readiness and
                // topology run before the Ready resolution, as for a build.
                if self.resolve_warm_revival_bound(&work) {
                    self.clear_member_restore_failure(&work.entry.agent_identity)
                        .await;
                    self.dispatch_warm_readiness(work, None);
                } else {
                    work.reply(Ok(MemberLiveRevivalOutcome::NotCurrent));
                }
            }
            RevivalStep::Built(Err(error)) => self.finish_warm_revival_failure(&work, error).await,
            RevivalStep::Built(Ok(built)) => {
                let matches_session = built
                    .provision
                    .member_ref()
                    .is_ok_and(|member| member.bridge_session_id() == Some(&work.session_id));
                if !settlement.fence.is_current() || !matches_session {
                    self.roll_back_warm_build(work, built, matches_session);
                    return;
                }
                // The attempt's Bound verdict alone chooses between
                // commit and rollback. A stale attempt (a stop, a retire or
                // another resolution closed it meanwhile) rolls the provision
                // back; it never fail-stops the mob.
                if !self.resolve_warm_revival_bound(&work) {
                    self.roll_back_warm_build(work, built, true);
                    return;
                }
                #[cfg(feature = "runtime-adapter")]
                if let Some(successor) = &work.scope.registration {
                    let Some(publication) = built.publication.as_ref() else {
                        let fail_stop_reply =
                            self.durable_uncertainty_fail_stop
                                .record(MobError::Internal(
                                    "warm reload lost its publication receipt".to_string(),
                                ));
                        work.reply(Err(fail_stop_reply));
                        return;
                    };
                    if let Err(error) = self
                        .provisioner
                        .record_reload_publication(&work.member_ref, successor, publication.clone())
                        .await
                    {
                        let fail_stop_reply = self.durable_uncertainty_fail_stop.record(error);
                        work.reply(Err(fail_stop_reply));
                        return;
                    }
                }
                if let Err(error) = built.provision.commit() {
                    let fail_stop_reply = self.durable_uncertainty_fail_stop.record(error);
                    work.reply(Err(fail_stop_reply));
                    return;
                }
                // Until the Ready verdict, a stale or failed step rolls this
                // provision back (no materialization the machine does not
                // hold outlives its attempt).
                *work
                    .committed
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(built.rollback);
                self.clear_member_restore_failure(&work.entry.agent_identity)
                    .await;
                self.dispatch_warm_readiness(work, built.publication);
            }
            RevivalStep::Ready {
                publication,
                result,
            } => {
                if let Err(error) = result {
                    // A reload whose readiness fails is retryable, as the
                    // reload contract always had it (the member is not
                    // Broken): its provision is rolled back, the member is
                    // Dormant, and its next delivery wakes it through the
                    // cold recipe.
                    self.roll_back_committed_warm_build(
                        work,
                        WarmRollbackThen::Failed {
                            kind: mob_dsl::MemberWakeAbandonKind::ReloadRefused,
                            error,
                        },
                    )
                    .await;
                    return;
                }
                if !settlement.fence.is_current() || !self.warm_revival_attempt_is_open(&work) {
                    self.roll_back_committed_warm_build(work, WarmRollbackThen::NotCurrent)
                        .await;
                    return;
                }
                let plan = match self.machine_restore_wiring_plan(&work.entry.agent_identity) {
                    Ok(plan) => plan,
                    Err(error) => {
                        self.roll_back_committed_warm_build(
                            work,
                            WarmRollbackThen::Failed {
                                kind: mob_dsl::MemberWakeAbandonKind::TopologyFailed,
                                error,
                            },
                        )
                        .await;
                        return;
                    }
                };
                let remaining = plan
                    .local_peers
                    .into_iter()
                    .filter(|peer| peer != &work.entry.agent_identity)
                    .map(|peer| {
                        let key = RespawnTopologyPeerId::from(peer.as_str());
                        (super::super::handle::PeerTarget::Local(peer), key)
                    })
                    .chain(plan.external_peers.into_iter().map(|peer| {
                        let key = RespawnTopologyPeerId::from(peer.peer_id.as_str());
                        (super::super::handle::PeerTarget::External(peer), key)
                    }))
                    .collect();
                self.advance_warm_topology(
                    work,
                    RevivalTopology {
                        publication,
                        remaining,
                        failed: Vec::new(),
                    },
                )
                .await;
            }
            RevivalStep::Topology {
                mut topology,
                peer,
                result,
            } => {
                if !settlement.fence.is_current() || !self.warm_revival_attempt_is_open(&work) {
                    self.roll_back_committed_warm_build(work, WarmRollbackThen::NotCurrent)
                        .await;
                    return;
                }
                if result.is_err() {
                    topology.failed.push(peer);
                }
                self.advance_warm_topology(work, topology).await;
            }
            RevivalStep::RolledBack { settled, then } => {
                if !settled {
                    tracing::error!(
                        mob_id = %self.definition.id,
                        agent_identity = %work.entry.agent_identity,
                        "a warm reload could not roll back the provision it committed; its custody was abandoned"
                    );
                }
                self.finish_after_warm_rollback(&work, then).await;
            }
            RevivalStep::Unsettled => {}
        }
    }

    /// Roll back a build this revival will not commit (a stale fence, a
    /// changed session, or a stale attempt), then finish by what the
    /// rollback settled.
    fn roll_back_warm_build(
        &mut self,
        work: Arc<RevivalWork>,
        built: RevivalPublication,
        matches_session: bool,
    ) {
        self.dispatch_warm_revival(work, "warm_revival_stale_cleanup", async move {
            match built.provision.rollback_retaining_custody().await {
                Ok(()) if matches_session => {
                    RevivalStep::Inert(MemberLiveRevivalOutcome::NotCurrent)
                }
                Ok(()) => RevivalStep::Failed(MobError::Internal(
                    "warm revival provision changed its exact session".to_string(),
                )),
                Err(retained) => {
                    let _ = retained.abandon("warm revival lost its member incarnation");
                    RevivalStep::Unsettled
                }
            }
        });
    }

    /// Roll back the provision this revival committed on its Bound verdict
    /// (off the actor), then finish with `then`. A revival that committed
    /// none (a session that was already live, a resumed publication) has
    /// nothing of its own to roll back and finishes at once.
    async fn roll_back_committed_warm_build(
        &mut self,
        work: Arc<RevivalWork>,
        then: WarmRollbackThen,
    ) {
        let Some(rollback) = work.take_committed() else {
            self.finish_after_warm_rollback(&work, then).await;
            return;
        };
        self.withdraw_warm_revival_binding_request(&work);
        let identity = work.entry.agent_identity.clone();
        let provisioner = Arc::clone(&self.provisioner);
        self.dispatch_warm_revival(work, "warm_revival_committed_rollback", async move {
            let settled = match rollback
                .pending(&identity, provisioner)
                .rollback_retaining_custody()
                .await
            {
                Ok(()) => true,
                Err(retained) => {
                    let _ = retained.abandon("warm reload rollback lost its member incarnation");
                    false
                }
            };
            RevivalStep::RolledBack { settled, then }
        });
    }

    /// Withdraw the runtime binding request the revival's Bound step queued
    /// for the provision its rollback is about to remove, when it is still
    /// waiting for its dispatch. A revival's steps commit off any command
    /// boundary, so that request can still be queued here; dispatched after
    /// the rollback removed the registration it names, the runtime would
    /// refuse it, and a refusal of the attempt that is still open records
    /// the member Broken where the attempt's end leaves it Dormant. The
    /// attempt's end clears the request in MobMachine.
    fn withdraw_warm_revival_binding_request(&mut self, work: &RevivalWork) {
        let Some(attempt) = work.attempt() else {
            return;
        };
        let identity = mob_dsl::AgentIdentity::from_domain(&work.entry.agent_identity);
        self.pending_routed_effects.retain(|effect| {
            !matches!(
                effect.body(),
                mob_dsl::MobMachineEffect::RequestRuntimeBinding {
                    agent_identity,
                    attempt: Some(requested),
                    ..
                } if agent_identity == &identity && *requested == attempt
            )
        });
    }

    /// Finish a revival whose committed provision was rolled back (or that
    /// committed none): by the machine's verdict for its attempt, then the
    /// caller's answer.
    async fn finish_after_warm_rollback(&mut self, work: &RevivalWork, then: WarmRollbackThen) {
        match then {
            WarmRollbackThen::NotCurrent => {
                self.finish_warm_revival_inert(work, MemberLiveRevivalOutcome::NotCurrent)
                    .await;
            }
            WarmRollbackThen::Failed { kind, error } => {
                self.abandon_warm_revival(work, kind, error.to_string())
                    .await;
                work.reply(Err(error));
            }
        }
    }

    /// Whether the revival's attempt is still the member's open attempt. A
    /// Stop, a Complete or a retirement start closes it; a step that
    /// returns after that rolls the revival's provision back (as the stale
    /// verdict of its Ready resolution would) without touching the member's
    /// topology. A revival that minted no attempt has none to lose.
    fn warm_revival_attempt_is_open(&self, work: &RevivalWork) -> bool {
        work.attempt().is_none_or(|attempt| {
            self.member_wake_attempt_is_open(&work.entry.agent_identity, attempt)
        })
    }

    /// Apply `ResolveMemberRevivalBound` for the revival's attempt (its
    /// Bound step: the runtime binding is requested, the member stays Waking).
    /// `true` only when the machine applied it; a stale attempt, or a
    /// revival that minted none, answers `false`.
    fn resolve_warm_revival_bound(&mut self, work: &RevivalWork) -> bool {
        let Some(attempt) = work.attempt() else {
            tracing::error!(
                mob_id = %self.definition.id,
                agent_identity = %work.entry.agent_identity,
                "a warm revival built a member without a wake attempt; its provision is not committed"
            );
            return false;
        };
        self.apply_member_wake_feedback(
            &work.entry.agent_identity,
            attempt,
            mob_dsl::MobMachineSignal::ResolveMemberRevivalBound {
                agent_identity: mob_dsl::AgentIdentity::from_domain(&work.entry.agent_identity),
                attempt,
            },
            "resolve_warm_revival_bound",
        ) == mob_dsl::MemberWakeFeedbackVerdict::Applied
    }

    /// End the revival's attempt with `AbandonMemberWake` (a no-op without
    /// an attempt). NotCurrent, ReloadRefused and Unsettled leave the member
    /// Dormant; ReadinessFailed and TopologyFailed leave it Broken with
    /// `reason`. On an applied verdict, every caller that joined the attempt
    /// is answered from the state it left.
    async fn abandon_warm_revival(
        &mut self,
        work: &RevivalWork,
        kind: mob_dsl::MemberWakeAbandonKind,
        reason: String,
    ) {
        let Some(attempt) = work.attempt() else {
            return;
        };
        let verdict = self.apply_member_wake_feedback(
            &work.entry.agent_identity,
            attempt,
            mob_dsl::MobMachineSignal::AbandonMemberWake {
                agent_identity: mob_dsl::AgentIdentity::from_domain(&work.entry.agent_identity),
                attempt,
                kind,
                reason: reason.clone(),
            },
            "abandon_warm_revival",
        );
        if verdict != mob_dsl::MemberWakeFeedbackVerdict::Applied {
            return;
        }
        let broken = matches!(
            kind,
            mob_dsl::MemberWakeAbandonKind::ReadinessFailed
                | mob_dsl::MemberWakeAbandonKind::TopologyFailed
        );
        if broken {
            self.record_member_restore_failure(
                &work.entry.agent_identity,
                super::super::handle::RestoreFailureDiagnostic {
                    bridge_session_id: Some(work.session_id.clone()),
                    reason,
                    hold: None,
                    registration_refusal: None,
                },
            )
            .await;
        }
        self.finish_closed_member_wake(&work.entry.agent_identity, attempt)
            .await;
        if !broken {
            self.schedule_owed_member_wake_retry(&work.entry.agent_identity);
        }
    }

    /// The revival ended without materializing anything: close its attempt
    /// (when it minted one) and answer `outcome`.
    async fn finish_warm_revival_inert(
        &mut self,
        work: &RevivalWork,
        outcome: MemberLiveRevivalOutcome,
    ) {
        self.abandon_warm_revival(
            work,
            mob_dsl::MemberWakeAbandonKind::NotCurrent,
            format!(
                "the warm reload of member '{}' ended without materializing it",
                work.entry.agent_identity
            ),
        )
        .await;
        work.reply(Ok(outcome));
    }

    fn dispatch_warm_readiness(
        &mut self,
        work: Arc<RevivalWork>,
        publication: Option<super::super::provisioner::ResumedMemberRollbackAuthority>,
    ) {
        let readiness = self.detached_member_readiness_context();
        // The drain rule every readiness path shares.
        let drain_required =
            super::member_requires_comms_drain(self.dsl_authority.state(), &work.entry);
        let worker_work = Arc::clone(&work);
        self.dispatch_warm_revival(work, "warm_revival_readiness", async move {
            // A reload that published its successor serves only through that
            // publication's exact executor attachment. A session that was
            // already live when the build ran published nothing to check.
            #[cfg(feature = "runtime-adapter")]
            if let Some(registration) = worker_work.scope.registration.as_ref()
                && publication.is_some()
            {
                let Some((adapter, attachment)) = readiness.runtime_adapter.as_ref().zip(
                    publication
                        .as_ref()
                        .and_then(|receipt| receipt.attachment_witness()),
                ) else {
                    return RevivalStep::Ready {
                        publication,
                        result: Err(MobError::MemberReloadRefused {
                            session_id: worker_work.session_id.clone(),
                            reason: "published reload has no exact attachment owner".into(),
                        }),
                    };
                };
                if !adapter
                    .executor_attachment_cleanup_is_current_for_registration(
                        attachment,
                        registration,
                    )
                    .await
                    || adapter
                        .current_executor_attachment_witness(&worker_work.session_id)
                        .await
                        .as_ref()
                        != Some(attachment)
                {
                    return RevivalStep::Inert(MemberLiveRevivalOutcome::NotCurrent);
                }
            }
            #[cfg(test)]
            {
                let barrier = READINESS_TEST_BARRIERS
                    .lock()
                    .expect("readiness barrier")
                    .remove(&worker_work.session_id);
                if let Some((entered, release)) = barrier {
                    let _ = entered.send(());
                    let _ = release.await;
                }
            }
            let result = readiness
                .ensure_member_comms_drain(
                    &worker_work.entry.agent_identity,
                    &worker_work.member_ref,
                    drain_required,
                )
                .await;
            RevivalStep::Ready {
                publication,
                result,
            }
        });
    }

    /// The reload's recipe: its attempt first (the drift arm, Live ->
    /// Waking), then the rebuild material. `Ok(None)` when another wake
    /// holds the member, which then materializes it in this reload's place.
    async fn prepare_warm_revival_recipe(
        &mut self,
        work: &RevivalWork,
        profile: Option<Result<crate::profile::Profile, MobError>>,
    ) -> Result<Option<Box<DeferredResumeProvision>>, MobError> {
        let identity = mob_dsl::AgentIdentity::from_domain(&work.entry.agent_identity);
        let observation = if profile.is_some() {
            mob_dsl::MemberLiveMaterializationObservationKind::DurableSnapshotPresent
        } else {
            mob_dsl::MemberLiveMaterializationObservationKind::DurableSnapshotMissing
        };
        let reason = if profile.is_some() {
            format!(
                "live session materialization missing for '{}'",
                work.session_id
            )
        } else {
            format!("missing bridge session snapshot for '{}'", work.session_id)
        };
        // Revival is admitted only while the mob runs (every classification
        // transition guards on the Running phase). Say so typed, as command
        // admission does, so a caller can tell a mob that is not running yet
        // from a failed revival and retry once it runs.
        if self.state() != MobState::Running {
            return Err(self.invalid_transition_to(MobState::Running));
        }
        // The reload's own attempt: the reload's discard did not release the
        // member, so it is still Live, and the drift arm (a counted
        // materialization loss) moves it to Waking under an attempt the
        // machine mints.
        let transition = match self.apply_dsl_signal_collect_transition(
            mob_dsl::MobMachineSignal::ClassifyMemberLiveMaterialization {
                agent_identity: identity.clone(),
                observation,
                reason: reason.clone(),
            },
            "classify_warm_revival",
        ) {
            Ok(transition) => transition,
            Err(error) => {
                if self
                    .dsl_authority
                    .state()
                    .member_wake_attempts
                    .contains_key(&identity)
                {
                    return Ok(None);
                }
                return Err(error);
            }
        };
        let classified = transition.effects().iter().find_map(|effect| match effect {
            mob_dsl::MobMachineEffect::MemberLiveMaterializationClassified {
                agent_identity,
                observation: echoed,
                verdict,
                attempt,
                ..
            } if agent_identity == &identity && *echoed == observation => {
                Some((*verdict, *attempt))
            }
            _ => None,
        });
        match classified {
            Some((mob_dsl::MemberRevivalVerdictKind::ReviveAuthorized, Some(attempt))) => {
                let _ = work.attempt.set(attempt);
            }
            Some((mob_dsl::MemberRevivalVerdictKind::ReviveAuthorized, None)) => {
                return Err(MobError::Internal(
                    "warm revival classification authorized no wake attempt".to_string(),
                ));
            }
            Some((mob_dsl::MemberRevivalVerdictKind::BrokenRecorded, _)) => {
                return Err(MobError::MemberRestoreFailed {
                    member_id: work.entry.agent_identity.clone(),
                    session_id: Some(work.session_id.clone()),
                    reason,
                    hold: None,
                });
            }
            None => {
                return Err(MobError::Internal(
                    "warm revival classification emitted no exact verdict".to_string(),
                ));
            }
        }
        let profile = profile.ok_or_else(|| MobError::MemberRestoreFailed {
            member_id: work.entry.agent_identity.clone(),
            session_id: Some(work.session_id.clone()),
            reason: "durable profile material is unavailable".to_string(),
            hold: None,
        })??;
        self.authorize_spawn_profile_material(
            &work.entry.agent_identity,
            &work.entry.role,
            &profile,
            "warm_revival_profile",
        )?;
        // The overlay the member was last built with, except for a fork whose
        // source is gone or is another build (see `warm_revival_overlay`).
        let overlay = self
            .warm_revival_overlay(&work.entry, &work.session_id)
            .await?;
        let external_tools = self.external_tools_for_profile(&profile, overlay)?;
        // The continuity intent the member's incarnation was spawned with, read
        // before the provision owner is taken: a durable worker is revived
        // with its resume gate.
        let continuity_intent =
            Box::pin(self.recorded_member_continuity_intent(&work.entry, &work.session_id)).await?;
        let session_id = mob_dsl::SessionId::from_domain(&work.session_id);
        let transition = self.apply_dsl_signal_collect_transition(
            mob_dsl::MobMachineSignal::RecoverMemberSessionBinding {
                agent_identity: identity.clone(),
                agent_runtime_id: mob_dsl::AgentRuntimeId::from_domain(
                    &work.entry.agent_runtime_id,
                ),
                bridge_session_id: session_id.clone(),
                replacing: self
                    .dsl_authority
                    .state()
                    .member_session_bindings
                    .get(&identity)
                    .cloned(),
            },
            "warm_revival_provision_owner",
        )?;
        if !transition.effects().iter().any(|effect| {
            matches!(
                effect,
                mob_dsl::MobMachineEffect::SessionProvisionOperationOwnerAuthorized {
                    agent_identity, session_id: expected,
                } if agent_identity == &identity && expected == &session_id
            )
        }) {
            return Err(MobError::Internal(
                "warm revival has no generated provision owner".to_string(),
            ));
        }
        let bundle_tools =
            super::super::tools::resolve_profile_bundle_tools(&profile, &self.tool_bundles);
        // The rebuild runs on the binding the host's customizer gives the
        // member now (its resume hook, which every rebuild asks), never on the
        // binding its session recorded: a reload can rebuild a member built
        // under an earlier launch (a start's recovery of a member it left
        // out). A fork-derived member keeps its own records.
        let auth_binding = self
            .warm_revival_auth_binding(&work.entry, &work.session_id)
            .await?;
        Ok(Some(Box::new(DeferredResumeProvision {
            definition: Arc::clone(&self.definition),
            mob_incarnation: self.incarnation,
            realm_profile_store: self.realm_profile_store.clone(),
            profile_name: work.entry.role.clone(),
            agent_identity: work.entry.agent_identity.clone(),
            profile,
            external_tools,
            bundle_tools,
            compaction_curator_override: None,
            // No context: the rebuild carries the session's persisted one
            // forward (`build_resumed_agent_config`).
            context: None,
            // A fork-derived member is rebuilt with its persisted lineage.
            fork_source: work.entry.fork_source.clone(),
            labels: Some(work.entry.labels.clone()),
            additional_instructions: None,
            shell_env: None,
            inherited_tool_filter: None,
            tool_access_policy: None,
            tool_dispatch_admission: None,
            web_search_override: Default::default(),
            // No policy choice: the member keeps its durable binding, which
            // the current registry realizes.
            application_tool_policy: None,
            tool_consequence_policy_registry: self.tool_consequence_policy_registry.clone(),
            system_prompt_override: None,
            resume_from_role: None,
            resume_id: work.session_id.clone(),
            prompt: ContentInput::from(
                self.fallback_spawn_prompt(&work.entry.role, &work.entry.agent_identity),
            ),
            budget_limits: None,
            keep_alive: work.entry.runtime_mode == crate::MobRuntimeMode::AutonomousHost,
            default_llm_client: self.default_llm_client.clone(),
            binding: crate::RuntimeBinding::Session,
            peer_name: render_member_comms_name(
                self.definition.id.as_str(),
                work.entry.role.as_str(),
                work.entry.agent_identity.as_str(),
            )?,
            owner_bridge_session_id: None,
            ops_registry: None,
            generated_self_owned_operation_owner: Some(work.session_id.clone()),
            direct_member_incarnation: None,
            continuity_intent,
            auth_binding,
        })))
    }

    /// The auth binding the host's spawn customizer gives `entry` for a
    /// rebuild on `session_id` now (`customize_resume`, with what the session
    /// holds), or `None` when no customizer is configured, the member is
    /// fork-derived, or the customizer names none: the session's recorded
    /// binding then stays.
    async fn warm_revival_auth_binding(
        &self,
        entry: &RosterEntry,
        session_id: &SessionId,
    ) -> Result<Option<meerkat_core::AuthBindingRef>, MobError> {
        if entry.fork_source.is_some() || self.spawn_member_customizer.is_none() {
            return Ok(None);
        }
        let durable = super::super::fork_build::load_resumed_member_view(
            self.session_service.as_ref(),
            session_id,
        )
        .await?;
        let spec = super::super::fork_build::rebuild_resume_spec(
            &self.definition.id,
            self.spawn_member_customizer.as_ref(),
            entry,
            &durable,
        )?;
        Ok(spec.auth_binding)
    }

    fn cleanup_warm_claim(
        &mut self,
        work: Arc<RevivalWork>,
        resources: RevivalResources,
        error: Option<MobError>,
    ) {
        self.dispatch_warm_revival(
            work,
            "warm_revival_claim_cleanup",
            cleanup_resources(resources, error),
        );
    }

    async fn finish_warm_revival_failure(&mut self, work: &RevivalWork, error: MobError) {
        // The failure ends the revival's attempt, by the machine's verdict:
        // a refused reload leaves the member Dormant (retryable), any other
        // failure records it Broken, as a failed restore is. A stale attempt
        // is the machine's no-op; neither verdict fail-stops the mob.
        let mut closed = None;
        if let Some(attempt) = work.attempt() {
            let identity = mob_dsl::AgentIdentity::from_domain(&work.entry.agent_identity);
            let reload_refused = matches!(&error, MobError::MemberReloadRefused { .. });
            let signal = if reload_refused {
                mob_dsl::MobMachineSignal::AbandonMemberWake {
                    agent_identity: identity,
                    attempt,
                    kind: mob_dsl::MemberWakeAbandonKind::ReloadRefused,
                    reason: error.to_string(),
                }
            } else {
                Self::member_revival_failure_signal(identity, attempt, &work.session_id, &error).0
            };
            if self.apply_member_wake_feedback(
                &work.entry.agent_identity,
                attempt,
                signal,
                "resolve_warm_revival_failure",
            ) == mob_dsl::MemberWakeFeedbackVerdict::Applied
            {
                closed = Some((attempt, reload_refused));
            }
        }
        let answer = self.warm_revival_failure_answer(work, error).await;
        // Callers that joined the attempt are answered after the restore
        // diagnostic is recorded, so a Broken answer carries its hold.
        if let Some((attempt, reload_refused)) = closed {
            self.finish_closed_member_wake(&work.entry.agent_identity, attempt)
                .await;
            if reload_refused {
                self.schedule_owed_member_wake_retry(&work.entry.agent_identity);
            }
        }
        work.reply(Err(answer));
    }

    /// The error a failed revival answers with: the machine's recorded
    /// restore failure when it recorded the member Broken (keeping the typed
    /// durable-resume hold and the host's registration refusal), else the
    /// failure itself.
    async fn warm_revival_failure_answer(
        &mut self,
        work: &RevivalWork,
        error: MobError,
    ) -> MobError {
        let Some(reason) = self
            .dsl_authority
            .state()
            .member_restore_failures
            .get(&mob_dsl::AgentIdentity::from_domain(
                &work.entry.agent_identity,
            ))
            .cloned()
        else {
            return error;
        };
        // A typed durable resume hold (a WholeBlob document that needs the
        // sanctioned audited-endpoint repair, a held tail, a refused
        // recovery, quarantined evidence) is the caller's classification
        // fact: the session is intact and withheld, and only the hold class
        // says what clears it. Rewriting it into restore-failure prose hid a
        // production wedge behind a string every host could only retry. The
        // diagnostic keeps the machine's record AND the hold, so every later
        // `MemberRestoreFailed` minted for this Broken member carries it;
        // this reply keeps meerkat's typed refusal itself.
        let hold = error.durable_resume_hold();
        self.record_member_restore_failure(
            &work.entry.agent_identity,
            super::super::handle::RestoreFailureDiagnostic {
                bridge_session_id: Some(work.session_id.clone()),
                reason: reason.clone(),
                hold,
                registration_refusal: None,
            },
        )
        .await;
        if hold.is_some() {
            return error;
        }
        if let Some(refusal) = self.recorded_registration_refusal(
            &work.entry.agent_identity,
            Some(work.session_id.clone()),
            reason.clone(),
        ) {
            return refusal;
        }
        MobError::MemberRestoreFailed {
            member_id: work.entry.agent_identity.clone(),
            session_id: Some(work.session_id.clone()),
            reason,
            hold: None,
        }
    }

    async fn advance_warm_topology(
        &mut self,
        work: Arc<RevivalWork>,
        mut topology: RevivalTopology,
    ) {
        while let Some((target, peer)) = topology.remaining.pop_front() {
            match self
                .prepare_member_wire(work.entry.agent_identity.clone(), target)
                .await
            {
                Ok(prepared) => {
                    if let Some(prepared) = prepared.into_prepared() {
                        let continuation = self.realize_wiring_detached(*prepared);
                        self.dispatch_warm_revival(work, "warm_revival_topology", async move {
                            let result = continuation.settled().await;
                            RevivalStep::Topology {
                                topology,
                                peer,
                                result,
                            }
                        });
                        return;
                    }
                }
                Err(_) => topology.failed.push(peer),
            }
        }
        if let Err(error) = self
            .resolve_respawn_topology_restore_result(&work.entry.agent_identity, topology.failed)
        {
            self.roll_back_committed_warm_build(
                work,
                WarmRollbackThen::Failed {
                    kind: mob_dsl::MemberWakeAbandonKind::TopologyFailed,
                    error,
                },
            )
            .await;
            return;
        }
        // A session that was already live when the build ran published
        // nothing: the reload answers it as already live, as before.
        let outcome = match topology.publication {
            Some(publication) => MemberLiveRevivalOutcome::Materialized(Some(publication)),
            None => MemberLiveRevivalOutcome::AlreadyLive,
        };
        let Some(attempt) = work.attempt() else {
            // A revival of an earlier publication minted no attempt.
            work.reply(Ok(outcome));
            return;
        };
        // The Ready step: readiness and topology settled, so the member
        // is Live. The verdict alone decides the provision: Applied keeps
        // it (the machine now holds it Live); a stale attempt means another
        // transition closed it meanwhile, so the provision this reload
        // committed is rolled back and the reload is not current.
        if self.apply_member_wake_feedback(
            &work.entry.agent_identity,
            attempt,
            mob_dsl::MobMachineSignal::ResolveMemberRevivalSucceeded {
                agent_identity: mob_dsl::AgentIdentity::from_domain(&work.entry.agent_identity),
                attempt,
            },
            "resolve_warm_revival_ready",
        ) == mob_dsl::MemberWakeFeedbackVerdict::Applied
        {
            let _ = work.take_committed();
            self.finish_closed_member_wake(&work.entry.agent_identity, attempt)
                .await;
            work.reply(Ok(outcome));
        } else {
            self.roll_back_committed_warm_build(work, WarmRollbackThen::NotCurrent)
                .await;
        }
    }
}

async fn cleanup_resources(resources: RevivalResources, error: Option<MobError>) -> RevivalStep {
    #[cfg(feature = "runtime-adapter")]
    let mut resources = resources;
    #[cfg(feature = "runtime-adapter")]
    if let Some(mut claim) = resources.claim.take()
        && claim.rollback_now().await.is_err()
    {
        return RevivalStep::Unsettled;
    }
    #[cfg(not(feature = "runtime-adapter"))]
    let _ = resources;
    match error {
        Some(error) => RevivalStep::Failed(error),
        None => RevivalStep::Inert(MemberLiveRevivalOutcome::NotCurrent),
    }
}
