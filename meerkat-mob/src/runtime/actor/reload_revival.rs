//! Warm revival uses the same actor-owned effect lane as other member work.
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
            if actor.durable_uncertainty_fail_stop {
                return MemberEffectAck::Retained(MemberEffectRetention::resumable(
                    MobError::Internal("warm revival commit is held by fail-stop".to_string()),
                    self,
                ));
            }
            boxed_arm_future(|| actor.commit_warm_revival(self.work, self.step, settlement)).await;
            if actor.durable_uncertainty_fail_stop {
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
        self.dispatch_member_effect(MemberEffectRequest {
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
            self.durable_uncertainty_fail_stop = true;
            work.reply(Err(MobError::Internal(
                "warm revival effect ended without owned settlement evidence".to_string(),
            )));
            return;
        }
        if !settlement.fence.is_current()
            && matches!(
                &step,
                RevivalStep::Observed(Err(_)) | RevivalStep::Built(Err(_)) | RevivalStep::Failed(_)
            )
        {
            work.reply(Ok(MemberLiveRevivalOutcome::NotCurrent));
            return;
        }
        match step {
            RevivalStep::Observed(Ok(RevivalObservation::Inert(outcome))) => {
                work.reply(Ok(outcome));
            }
            RevivalStep::Observed(Err(error)) | RevivalStep::Failed(error) => {
                self.finish_warm_revival_failure(&work, error).await;
            }
            RevivalStep::Inert(outcome) => work.reply(Ok(outcome)),
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
                let recipe = self.prepare_warm_revival_recipe(&work, profile).await;
                let recipe = match recipe {
                    Ok(recipe) => recipe,
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
                            Ok(true) => return RevivalStep::AlreadyLiveAfterBuild,
                            Ok(false) | Err(meerkat_core::service::SessionError::NotFound { .. }) => {}
                            Err(_) => return RevivalStep::Unsettled,
                        }
                    }
                    RevivalStep::Built(receipt.map(|receipt| {
                        let publication = receipt.rollback_authority.clone();
                        RevivalPublication {
                            provision: PendingProvision::new(
                                receipt.member_ref,
                                worker_work.entry.agent_identity.clone(),
                                provisioner,
                                receipt.operation_id,
                                receipt.session_origin,
                                receipt.rollback_authority,
                            ),
                            publication,
                        }
                    }))
                });
            }
            RevivalStep::AlreadyLiveAfterBuild => {
                if !settlement.fence.is_current() {
                    work.reply(Ok(MemberLiveRevivalOutcome::NotCurrent));
                    return;
                }
                let identity = mob_dsl::AgentIdentity::from_domain(&work.entry.agent_identity);
                let result = self.apply_dsl_signal(
                    mob_dsl::MobMachineSignal::ResolveMemberRevivalSucceeded {
                        agent_identity: identity,
                    },
                    "resolve_warm_revival_already_live",
                );
                if let Err(error) = result {
                    work.reply(Err(error));
                } else {
                    self.restore_diagnostics
                        .write()
                        .await
                        .remove(&work.entry.agent_identity);
                    work.reply(Ok(MemberLiveRevivalOutcome::AlreadyLive));
                }
            }
            RevivalStep::Built(Err(error)) => self.finish_warm_revival_failure(&work, error).await,
            RevivalStep::Built(Ok(built)) => {
                let matches_session = built
                    .provision
                    .member_ref()
                    .is_ok_and(|member| member.bridge_session_id() == Some(&work.session_id));
                if !settlement.fence.is_current() || !matches_session {
                    self.dispatch_warm_revival(work, "warm_revival_stale_cleanup", async move {
                        match built.provision.rollback_retaining_custody().await {
                            Ok(()) if matches_session => {
                                RevivalStep::Inert(MemberLiveRevivalOutcome::NotCurrent)
                            }
                            Ok(()) => RevivalStep::Failed(MobError::Internal(
                                "warm revival provision changed its exact session".to_string(),
                            )),
                            Err(retained) => {
                                let _ =
                                    retained.abandon("warm revival lost its member incarnation");
                                RevivalStep::Unsettled
                            }
                        }
                    });
                    return;
                }
                #[cfg(feature = "runtime-adapter")]
                if let Some(successor) = &work.scope.registration {
                    let Some(publication) = built.publication.as_ref() else {
                        self.durable_uncertainty_fail_stop = true;
                        work.reply(Err(MobError::Internal(
                            "warm reload lost its publication receipt".to_string(),
                        )));
                        return;
                    };
                    if let Err(error) = self
                        .provisioner
                        .record_reload_publication(&work.member_ref, successor, publication.clone())
                        .await
                    {
                        self.durable_uncertainty_fail_stop = true;
                        work.reply(Err(error));
                        return;
                    }
                }
                let identity = mob_dsl::AgentIdentity::from_domain(&work.entry.agent_identity);
                if self
                    .dsl_authority
                    .state()
                    .member_revival_pending
                    .contains(&identity)
                    && let Err(error) = self.apply_dsl_signal(
                        mob_dsl::MobMachineSignal::ResolveMemberRevivalSucceeded {
                            agent_identity: identity,
                        },
                        "resolve_warm_revival_publication",
                    )
                {
                    self.durable_uncertainty_fail_stop = true;
                    work.reply(Err(error));
                    return;
                }
                if let Err(error) = built.provision.commit() {
                    self.durable_uncertainty_fail_stop = true;
                    work.reply(Err(error));
                    return;
                }
                self.restore_diagnostics
                    .write()
                    .await
                    .remove(&work.entry.agent_identity);
                self.dispatch_warm_readiness(work, built.publication);
            }
            RevivalStep::Ready {
                publication,
                result,
            } => {
                if let Err(error) = result {
                    work.reply(Err(error));
                    return;
                }
                if !settlement.fence.is_current() {
                    work.reply(Ok(MemberLiveRevivalOutcome::NotCurrent));
                    return;
                }
                let plan = match self.machine_restore_wiring_plan(&work.entry.agent_identity) {
                    Ok(plan) => plan,
                    Err(error) => {
                        work.reply(Err(error));
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
                if !settlement.fence.is_current() {
                    work.reply(Ok(MemberLiveRevivalOutcome::NotCurrent));
                    return;
                }
                if result.is_err() {
                    topology.failed.push(peer);
                }
                self.advance_warm_topology(work, topology).await;
            }
            RevivalStep::Unsettled => {}
        }
    }

    fn dispatch_warm_readiness(
        &mut self,
        work: Arc<RevivalWork>,
        publication: Option<super::super::provisioner::ResumedMemberRollbackAuthority>,
    ) {
        let readiness = self.detached_member_readiness_context();
        let worker_work = Arc::clone(&work);
        self.dispatch_warm_revival(work, "warm_revival_readiness", async move {
            #[cfg(feature = "runtime-adapter")]
            if let Some(registration) = worker_work.scope.registration.as_ref() {
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
                .ensure_mob_comms_drain(&worker_work.entry.agent_identity, &worker_work.member_ref)
                .await;
            RevivalStep::Ready {
                publication,
                result,
            }
        });
    }

    async fn prepare_warm_revival_recipe(
        &mut self,
        work: &RevivalWork,
        profile: Option<Result<crate::profile::Profile, MobError>>,
    ) -> Result<Box<DeferredResumeProvision>, MobError> {
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
        let continuing = self
            .dsl_authority
            .state()
            .member_revival_pending
            .contains(&identity);
        if !continuing {
            let transition = self.apply_dsl_signal_collect_transition(
                mob_dsl::MobMachineSignal::ClassifyMemberLiveMaterialization {
                    agent_identity: identity.clone(),
                    observation,
                    reason: reason.clone(),
                },
                "classify_warm_revival",
            )?;
            let verdict = transition.effects().iter().find_map(|effect| match effect {
                mob_dsl::MobMachineEffect::MemberLiveMaterializationClassified {
                    agent_identity,
                    observation: echoed,
                    verdict,
                    ..
                } if agent_identity == &identity && *echoed == observation => Some(*verdict),
                _ => None,
            });
            match verdict {
                Some(mob_dsl::MemberRevivalVerdictKind::ReviveAuthorized) => {}
                Some(mob_dsl::MemberRevivalVerdictKind::BrokenRecorded) => {
                    return Err(MobError::MemberRestoreFailed {
                        member_id: work.entry.agent_identity.clone(),
                        session_id: Some(work.session_id.clone()),
                        reason,
                    });
                }
                None => {
                    return Err(MobError::Internal(
                        "warm revival classification emitted no exact verdict".to_string(),
                    ));
                }
            }
        }
        let profile = profile.ok_or_else(|| MobError::MemberRestoreFailed {
            member_id: work.entry.agent_identity.clone(),
            session_id: Some(work.session_id.clone()),
            reason: "durable profile material is unavailable".to_string(),
        })??;
        self.authorize_spawn_profile_material(
            &work.entry.agent_identity,
            &work.entry.role,
            &profile,
            "warm_revival_profile",
        )?;
        let overlay = self
            .per_spawn_external_tools
            .read()
            .await
            .get(&work.entry.agent_identity)
            .cloned();
        let external_tools = self.external_tools_for_profile(&profile, overlay)?;
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
        Ok(Box::new(DeferredResumeProvision {
            definition: Arc::clone(&self.definition),
            profile_name: work.entry.role.clone(),
            agent_identity: work.entry.agent_identity.clone(),
            profile,
            external_tools,
            compaction_curator_override: None,
            context: None,
            labels: Some(work.entry.labels.clone()),
            additional_instructions: None,
            shell_env: None,
            inherited_tool_filter: None,
            tool_access_policy: None,
            tool_dispatch_admission: None,
            web_search_override: Default::default(),
            application_tool_policy: Default::default(),
            tool_consequence_policy_registry: None,
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
        }))
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
        let identity = mob_dsl::AgentIdentity::from_domain(&work.entry.agent_identity);
        if !matches!(&error, MobError::MemberReloadRefused { .. })
            && self
                .dsl_authority
                .state()
                .member_revival_pending
                .contains(&identity)
            && let Err(error) = self.apply_dsl_signal(
                mob_dsl::MobMachineSignal::ResolveMemberRevivalFailed {
                    agent_identity: identity,
                    reason: format!(
                        "machine-authorized revival of bridge session '{}' failed: {error}",
                        work.session_id,
                    ),
                },
                "resolve_warm_revival_failure",
            )
        {
            self.durable_uncertainty_fail_stop = true;
            work.reply(Err(error));
            return;
        }
        if let Some(reason) = self
            .dsl_authority
            .state()
            .member_restore_failures
            .get(&mob_dsl::AgentIdentity::from_domain(
                &work.entry.agent_identity,
            ))
            .cloned()
        {
            self.restore_diagnostics.write().await.insert(
                work.entry.agent_identity.clone(),
                super::super::handle::RestoreFailureDiagnostic {
                    bridge_session_id: Some(work.session_id.clone()),
                    reason: reason.clone(),
                },
            );
            work.reply(Err(MobError::MemberRestoreFailed {
                member_id: work.entry.agent_identity.clone(),
                session_id: Some(work.session_id.clone()),
                reason,
            }));
            return;
        }
        work.reply(Err(error));
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
        match self
            .resolve_respawn_topology_restore_result(&work.entry.agent_identity, topology.failed)
        {
            Ok(_) => work.reply(Ok(MemberLiveRevivalOutcome::Materialized(
                topology.publication,
            ))),
            Err(error) => work.reply(Err(error)),
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
