use super::*;
use crate::runtime::provision_guard::RetainedProvisionCustody;
use crate::runtime::provisioner::{ProvisionAttemptFailure, RetainedProvisionEffects};

impl MobActor {
    pub(super) async fn continue_explicit_resume_after_rebuild(&mut self) {
        if !self
            .dsl_authority
            .state()
            .explicit_resume_member_work
            .is_empty()
        {
            return;
        }
        let Some(pending) = self.pending_resume_lifecycle.take_if(|pending| {
            matches!(
                pending.phase,
                ResumeLifecyclePhase::RebuildingMembers { .. }
            )
        }) else {
            tracing::error!("settled resume member work lost its rebuilding continuation");
            self.durable_uncertainty_fail_stop = true;
            return;
        };
        let PendingResumeLifecycle {
            phase,
            admission,
            progress,
            reply_tx,
            ..
        } = pending;
        let ResumeLifecyclePhase::RebuildingMembers { post_commit_error } = phase else {
            let _ = reply_tx.send(Err(MobError::Internal(
                "resume continuation was not rebuilding members".to_string(),
            )));
            self.durable_uncertainty_fail_stop = true;
            return;
        };
        if self.dsl_authority.state().explicit_resume_cancel_requested {
            let result =
                self.finish_explicit_resume_attempt(Err(MobError::LifecycleOperationPending {
                    intent: "explicit_resume superseded by lifecycle control".to_string(),
                }));
            let _ = reply_tx.send(result);
            return;
        }
        progress
            .awaiting_stage(crate::runtime::state::LifecycleProgressStage::PostRebuildReadiness);
        match self.collect_member_readiness_targets(false).await {
            Err(error) => {
                self.begin_resume_lifecycle_post_commit(
                    admission,
                    progress,
                    post_commit_error.or(Some(error)),
                    reply_tx,
                )
                .await;
            }
            Ok(None) => {
                self.begin_resume_lifecycle_post_commit(
                    admission,
                    progress,
                    post_commit_error,
                    reply_tx,
                )
                .await;
            }
            Ok(Some(targets)) => self.spawn_resume_readiness_fanout(
                targets,
                Some(progress.clone()),
                PendingResumeLifecycle {
                    ticket: ResumeStepTicket::default(),
                    phase: ResumeLifecyclePhase::PostCommitReadiness { post_commit_error },
                    admission,
                    progress,
                    reply_tx,
                },
            ),
        }
    }

    fn classify_explicit_resume_work(
        &mut self,
        work: &ExplicitResumeMemberWork,
    ) -> Result<mob_dsl::ResumeMemberOutcomeDisposition, MobError> {
        let identity = mob_dsl::AgentIdentity::from_domain(&work.rebuild.entry.agent_identity);
        let effects = self.apply_dsl_input_collect_effects(
            mob_dsl::MobMachineInput::ClassifyExplicitResumeMemberOutcome {
                attempt: work.attempt.clone(),
                agent_identity: identity.clone(),
                binding: work.binding.clone(),
            },
            "classify_explicit_resume_member_outcome",
        )?;
        effects
            .into_iter()
            .find_map(|effect| match effect {
                mob_dsl::MobMachineEffect::ExplicitResumeMemberOutcomeClassified {
                    attempt,
                    agent_identity,
                    binding,
                    disposition,
                } if attempt == work.attempt
                    && agent_identity == identity
                    && binding == work.binding =>
                {
                    Some(disposition)
                }
                _ => None,
            })
            .ok_or_else(|| {
                MobError::Internal("resume outcome classifier emitted no exact verdict".to_string())
            })
    }

    pub(super) fn retain_explicit_resume_error(&mut self, error: MobError) {
        match self
            .pending_resume_lifecycle
            .as_mut()
            .map(|pending| &mut pending.phase)
        {
            Some(ResumeLifecyclePhase::RebuildingMembers { post_commit_error }) => {
                post_commit_error.get_or_insert(error);
            }
            _ => {
                tracing::error!(%error, "resume error lost its rebuilding continuation");
                self.durable_uncertainty_fail_stop = true;
            }
        }
    }

    pub(super) async fn explicit_resume_member_observed(
        &mut self,
        work: Arc<ExplicitResumeMemberWork>,
        observation: Result<ExplicitResumeLiveObservation, MobError>,
    ) {
        match self.classify_explicit_resume_work(&work) {
            Ok(mob_dsl::ResumeMemberOutcomeDisposition::Current) => {}
            Ok(mob_dsl::ResumeMemberOutcomeDisposition::RollbackRequired) => {
                self.explicit_resume_member_settled(
                    work,
                    ExplicitResumeMemberCompletion::Superseded,
                )
                .await;
                return;
            }
            Err(error) => {
                tracing::warn!(%error, "discarding stale resume observation without provisioning");
                return;
            }
        }
        let observation = match observation {
            Ok(observation) => observation,
            Err(error) => {
                self.explicit_resume_member_settled(
                    work,
                    ExplicitResumeMemberCompletion::Failed(error),
                )
                .await;
                return;
            }
        };
        if matches!(observation, ExplicitResumeLiveObservation::AlreadyLive) {
            self.spawn_explicit_resume_provision(work, None);
            return;
        }
        let identity = mob_dsl::AgentIdentity::from_domain(&work.rebuild.entry.agent_identity);
        let observed = match &observation {
            ExplicitResumeLiveObservation::DurableMissing => {
                mob_dsl::MemberLiveMaterializationObservationKind::DurableSnapshotMissing
            }
            ExplicitResumeLiveObservation::DurablePresent { .. } => {
                mob_dsl::MemberLiveMaterializationObservationKind::DurableSnapshotPresent
            }
            ExplicitResumeLiveObservation::AlreadyLive => {
                self.durable_uncertainty_fail_stop = true;
                return;
            }
        };
        let reason = match observed {
            mob_dsl::MemberLiveMaterializationObservationKind::DurableSnapshotMissing => {
                format!(
                    "missing bridge session snapshot for '{}'",
                    work.rebuild.bridge_session_id
                )
            }
            mob_dsl::MemberLiveMaterializationObservationKind::DurableSnapshotPresent => {
                format!(
                    "live session materialization missing for '{}'",
                    work.rebuild.bridge_session_id
                )
            }
        };
        let effects = match self.apply_dsl_input_collect_effects(
            mob_dsl::MobMachineInput::ClassifyExplicitResumeMemberLive {
                attempt: work.attempt.clone(),
                agent_identity: identity.clone(),
                binding: work.binding.clone(),
                observation: observed,
                reason: reason.clone(),
            },
            "classify_explicit_resume_member_live",
        ) {
            Ok(effects) => effects,
            Err(error) => {
                self.explicit_resume_member_settled(
                    work,
                    ExplicitResumeMemberCompletion::Failed(error),
                )
                .await;
                return;
            }
        };
        let verdict = effects.into_iter().find_map(|effect| match effect {
            mob_dsl::MobMachineEffect::MemberLiveMaterializationClassified {
                agent_identity,
                observation,
                verdict,
                ..
            } if agent_identity == identity && observation == observed => Some(verdict),
            _ => None,
        });
        match verdict {
            Some(mob_dsl::MemberRevivalVerdictKind::BrokenRecorded) => {
                self.restore_diagnostics.write().await.insert(
                    work.rebuild.entry.agent_identity.clone(),
                    crate::runtime::handle::RestoreFailureDiagnostic {
                        bridge_session_id: Some(work.rebuild.bridge_session_id.clone()),
                        reason,
                        hold: None,
                    },
                );
                self.explicit_resume_member_settled(work, ExplicitResumeMemberCompletion::Accepted)
                    .await;
            }
            Some(mob_dsl::MemberRevivalVerdictKind::ReviveAuthorized) => {
                let ExplicitResumeLiveObservation::DurablePresent { profile } = observation else {
                    self.durable_uncertainty_fail_stop = true;
                    tracing::error!(
                        "resume revival authorization disagrees with durable observation"
                    );
                    return;
                };
                // The rebuild request carries the overlay the member is rebuilt
                // with (see `recustomized_rebuild_overlay`).
                let recipe = profile
                    .and_then(|profile| self.explicit_resume_provision_recipe(&work, *profile));
                match recipe {
                    Ok(recipe) => self.spawn_explicit_resume_provision(work, Some(recipe)),
                    Err(error) => {
                        self.explicit_resume_member_settled(
                            work,
                            ExplicitResumeMemberCompletion::Failed(error),
                        )
                        .await;
                    }
                }
            }
            None => {
                tracing::error!("resume member classification emitted no matching verdict");
                self.durable_uncertainty_fail_stop = true;
            }
        }
    }

    fn explicit_resume_provision_recipe(
        &mut self,
        work: &ExplicitResumeMemberWork,
        profile: crate::profile::Profile,
    ) -> Result<Box<DeferredResumeProvision>, MobError> {
        let entry = &work.rebuild.entry;
        self.authorize_spawn_profile_material(
            &entry.agent_identity,
            &entry.role,
            &profile,
            "explicit_resume_profile",
        )?;
        let external_tools = self.external_tools_for_profile(
            &profile,
            work.rebuild.restore_spec.external_tools.clone(),
        )?;
        let identity = mob_dsl::AgentIdentity::from_domain(&entry.agent_identity);
        let session_id = mob_dsl::SessionId::from_domain(&work.rebuild.bridge_session_id);
        let transition = self.apply_dsl_signal_collect_transition(
            mob_dsl::MobMachineSignal::RecoverMemberSessionBinding {
                agent_identity: identity.clone(),
                agent_runtime_id: work.binding.agent_runtime_id.clone(),
                bridge_session_id: session_id.clone(),
                replacing: Some(session_id.clone()),
            },
            "explicit_resume_provision_owner",
        )?;
        if !transition.effects().iter().any(|effect| {
            matches!(
                effect,
                mob_dsl::MobMachineEffect::SessionProvisionOperationOwnerAuthorized {
                    agent_identity,
                    session_id: authorized_session,
                } if agent_identity == &identity && authorized_session == &session_id
            )
        }) {
            return Err(MobError::Internal(
                "explicit resume did not receive its exact provision owner".to_string(),
            ));
        }
        let crate::launch::MemberLaunchMode::Resume {
            resume_from_role, ..
        } = &work.rebuild.restore_spec.launch_mode
        else {
            return Err(MobError::Internal(
                "resume recipe lost its launch mode".to_string(),
            ));
        };
        Ok(Box::new(DeferredResumeProvision {
            definition: Arc::clone(&self.definition),
            profile_name: entry.role.clone(),
            agent_identity: entry.agent_identity.clone(),
            profile,
            external_tools,
            compaction_curator_override: work
                .rebuild
                .restore_spec
                .compaction_curator_override
                .clone(),
            // No context: the rebuild carries the session's persisted one
            // forward (`build_resumed_agent_config`).
            context: None,
            // A fork-derived member is rebuilt with its persisted lineage.
            fork_source: entry.fork_source.clone(),
            labels: Some(entry.labels.clone()),
            additional_instructions: None,
            shell_env: None,
            inherited_tool_filter: None,
            tool_access_policy: None,
            tool_dispatch_admission: None,
            web_search_override: Default::default(),
            application_tool_policy: Default::default(),
            tool_consequence_policy_registry: None,
            system_prompt_override: None,
            resume_from_role: resume_from_role.clone(),
            resume_id: work.rebuild.bridge_session_id.clone(),
            prompt: ContentInput::from(
                self.fallback_spawn_prompt(&entry.role, &entry.agent_identity),
            ),
            budget_limits: None,
            keep_alive: entry.runtime_mode == crate::MobRuntimeMode::AutonomousHost,
            default_llm_client: self.default_llm_client.clone(),
            binding: crate::RuntimeBinding::Session,
            peer_name: render_member_comms_name(
                self.definition.id.as_str(),
                entry.role.as_str(),
                entry.agent_identity.as_str(),
            )?,
            owner_bridge_session_id: None,
            ops_registry: None,
            generated_self_owned_operation_owner: Some(work.rebuild.bridge_session_id.clone()),
            direct_member_incarnation: None,
        }))
    }

    fn spawn_explicit_resume_provision(
        &mut self,
        work: Arc<ExplicitResumeMemberWork>,
        recipe: Option<Box<DeferredResumeProvision>>,
    ) {
        let service = Arc::clone(&self.session_service);
        let provisioner = Arc::clone(&self.provisioner);
        let readiness = self.detached_member_readiness_context();
        let command_tx = self.command_tx.clone();
        let worker_work = Arc::clone(&work);
        let worker = tokio::spawn(async move {
            let result = provision_explicit_resume_member(
                &worker_work,
                recipe,
                service,
                Arc::clone(&provisioner),
                readiness,
            )
            .await;
            deliver_explicit_resume_provision(worker_work, result, provisioner, command_tx).await;
        });
        let command_tx = self.command_tx.clone();
        // The observer can be aborted with the actor; the custody owner must
        // continue to either transfer ownership or acknowledge compensation.
        self.actor_io_tasks.spawn(async move {
            if let Err(error) = worker.await {
                let failure = ProvisionAttemptFailure::unproven(MobError::Internal(format!(
                    "resume provisioning owner failed before settlement: {error}"
                )));
                let _ = command_tx
                    .send(RoutedMobCommand::internal(
                        MobCommand::ResumeLifecycleMemberUnproven {
                            work,
                            failure,
                            stage: UnprovenResumeCustodyStage::BeforeAcceptance,
                        },
                    ))
                    .await;
            }
        });
    }

    pub(super) async fn explicit_resume_member_ready(
        &mut self,
        work: Arc<ExplicitResumeMemberWork>,
        recovered_endpoint: Option<TrustedPeerDescriptor>,
        decision_tx: oneshot::Sender<ExplicitResumeMemberDecision>,
    ) {
        let decision = match self.classify_explicit_resume_work(&work) {
            Ok(mob_dsl::ResumeMemberOutcomeDisposition::Current) => {
                let accepted = self
                    .accept_explicit_resume_member(&work, recovered_endpoint)
                    .await;
                match accepted {
                    Ok(()) => ExplicitResumeMemberDecision::Accept,
                    Err(error) => ExplicitResumeMemberDecision::Rollback(
                        ExplicitResumeCleanupReason::Failed(error),
                    ),
                }
            }
            Ok(mob_dsl::ResumeMemberOutcomeDisposition::RollbackRequired) => {
                ExplicitResumeMemberDecision::Rollback(ExplicitResumeCleanupReason::Superseded)
            }
            Err(error) => {
                tracing::warn!(%error, "resume offer no longer has exact outcome authority");
                ExplicitResumeMemberDecision::Rollback(ExplicitResumeCleanupReason::Failed(error))
            }
        };
        if decision_tx.send(decision).is_err() {
            tracing::error!("resume custody owner disappeared before its ownership decision");
            self.durable_uncertainty_fail_stop = true;
        }
    }

    async fn accept_explicit_resume_member(
        &mut self,
        work: &ExplicitResumeMemberWork,
        recovered_endpoint: Option<TrustedPeerDescriptor>,
    ) -> Result<(), MobError> {
        let entry = &work.rebuild.entry;
        if let Some(endpoint) = recovered_endpoint {
            let event = crate::runtime::builder::append_recovered_session_binding(
                &mut self.dsl_authority,
                &self.events,
                &self.definition.id,
                entry,
                &work.rebuild.bridge_session_id,
                Some(endpoint),
                "explicit_resume_upgrade_recovered_member_peer_endpoint",
            )
            .await?;
            self.roster.write().await.apply_event(&event);
            self.publish_machine_state_projection();
        }
        let identity = mob_dsl::AgentIdentity::from_domain(&entry.agent_identity);
        if self
            .dsl_authority
            .state()
            .member_revival_pending
            .contains(&identity)
        {
            self.apply_dsl_signal(
                mob_dsl::MobMachineSignal::ResolveMemberRevivalSucceeded {
                    agent_identity: identity,
                },
                "resolve_explicit_resume_member_revival",
            )?;
        }
        self.restore_diagnostics
            .write()
            .await
            .remove(&entry.agent_identity);
        Ok(())
    }

    pub(super) async fn explicit_resume_member_settled(
        &mut self,
        work: Arc<ExplicitResumeMemberWork>,
        completion: ExplicitResumeMemberCompletion,
    ) {
        let identity = mob_dsl::AgentIdentity::from_domain(&work.rebuild.entry.agent_identity);
        if let ExplicitResumeMemberCompletion::Failed(error) = completion {
            if matches!(
                self.classify_explicit_resume_work(&work),
                Ok(mob_dsl::ResumeMemberOutcomeDisposition::Current)
            ) {
                if self
                    .dsl_authority
                    .state()
                    .member_revival_pending
                    .contains(&identity)
                {
                    let reason = format!(
                        "machine-authorized revival of bridge session '{}' failed: {error}",
                        work.rebuild.bridge_session_id
                    );
                    if let Err(error) = self.apply_dsl_signal(
                        mob_dsl::MobMachineSignal::ResolveMemberRevivalFailed {
                            agent_identity: identity.clone(),
                            reason: reason.clone(),
                        },
                        "resolve_failed_explicit_resume_member",
                    ) {
                        self.retain_explicit_resume_error(error);
                    } else {
                        self.restore_diagnostics.write().await.insert(
                            work.rebuild.entry.agent_identity.clone(),
                            crate::runtime::handle::RestoreFailureDiagnostic {
                                bridge_session_id: Some(work.rebuild.bridge_session_id.clone()),
                                reason,
                                hold: None,
                            },
                        );
                    }
                } else {
                    self.retain_explicit_resume_error(error);
                }
            }
        }
        if let Err(error) = self.apply_dsl_input(
            mob_dsl::MobMachineInput::SettleExplicitResumeMember {
                attempt: work.attempt.clone(),
                agent_identity: identity,
                binding: work.binding.clone(),
            },
            "settle_explicit_resume_member",
        ) {
            tracing::warn!(%error, "stale resume settlement left current authority unchanged");
            return;
        }
        if let Some(pending) = self.pending_resume_lifecycle.as_ref() {
            pending.progress.member_progress(
                &work.rebuild.entry.agent_identity,
                crate::runtime::state::LifecycleProgressStage::MemberLiveMaterialization,
            );
        }
        self.continue_explicit_resume_after_rebuild().await;
    }

    pub(super) async fn explicit_resume_member_cleanup_held(
        &mut self,
        work: Arc<ExplicitResumeMemberWork>,
        retry_tx: oneshot::Sender<()>,
        error: String,
        attempts: u32,
        automatic_retry: bool,
    ) {
        tracing::error!(
            agent_identity = %work.rebuild.entry.agent_identity,
            %error,
            attempts,
            "explicit resume retains exact member cleanup custody"
        );
        if attempts == 1 && automatic_retry {
            if retry_tx.send(()).is_err() {
                Box::pin(self.explicit_resume_member_unproven(
                    work,
                    ProvisionAttemptFailure::unproven(MobError::Internal(
                        "resume cleanup owner disappeared before retry".to_string(),
                    )),
                    UnprovenResumeCustodyStage::BeforeAcceptance,
                ))
                .await;
            }
            return;
        }
        let reason = format!(
            "explicit resume of bridge session '{}' retains uncertified cleanup of its own attempt: {error}; the member is Broken until retired or respawned (repair required: retire or respawn the member); the retained cleanup obligation is retried separately on the next lifecycle control",
            work.rebuild.bridge_session_id
        );
        self.retained_resume_cleanup
            .push(RetainedExplicitResumeCleanup {
                work: Arc::clone(&work),
                retry_tx,
                error,
            });
        // Isolation (#1251): the retained custody stays parked and is retried
        // on lifecycle control, but this member's work settles as a typed
        // failure so the mob-wide Resume continues for every other member.
        Box::pin(self.explicit_resume_member_settled(
            work,
            ExplicitResumeMemberCompletion::Failed(MobError::Internal(reason)),
        ))
        .await;
    }

    /// Park an attempt whose own effects cannot be certified, then settle the
    /// member's resume work as a typed failure.
    ///
    /// The parked custody keeps the effect obligation visible and is reported
    /// on every lifecycle control. Settling the work records the member as
    /// Broken (with a repair diagnostic) instead of leaving it pending, which
    /// used to stall the whole mob-wide Resume until the inactivity watchdog
    /// fired (#1251).
    pub(super) async fn explicit_resume_member_unproven(
        &mut self,
        work: Arc<ExplicitResumeMemberWork>,
        failure: ProvisionAttemptFailure,
        stage: UnprovenResumeCustodyStage,
    ) {
        if stage == UnprovenResumeCustodyStage::AfterAcceptance {
            // The member was accepted and is live; only the provision guard's
            // custody is uncertain. Park that obligation and settle the member
            // accepted, so the mob-wide Resume continues (#1251 isolation).
            self.park_unproven_resume_cleanup(Arc::clone(&work), failure);
            Box::pin(
                self.explicit_resume_member_settled(work, ExplicitResumeMemberCompletion::Accepted),
            )
            .await;
            return;
        }
        let reason = format!(
            "explicit resume of bridge session '{}' could not certify cleanup of its own attempt: {failure}; the member is Broken until retired or respawned (repair required: retire or respawn the member); the parked cleanup obligation is reported separately on each lifecycle control",
            work.rebuild.bridge_session_id
        );
        self.park_unproven_resume_cleanup(Arc::clone(&work), failure);
        Box::pin(self.explicit_resume_member_settled(
            work,
            ExplicitResumeMemberCompletion::Failed(MobError::Internal(reason)),
        ))
        .await;
    }

    fn park_unproven_resume_cleanup(
        &mut self,
        work: Arc<ExplicitResumeMemberWork>,
        failure: ProvisionAttemptFailure,
    ) {
        tracing::error!(
            agent_identity = %work.rebuild.entry.agent_identity,
            error = %failure,
            "explicit resume cannot certify member cleanup; custody remains parked"
        );
        self.unproven_resume_cleanup
            .push(UnprovenExplicitResumeCleanup { work, failure });
    }

    pub(super) fn retry_explicit_resume_cleanups(&mut self) {
        for retained in std::mem::take(&mut self.retained_resume_cleanup) {
            tracing::warn!(
                agent_identity = %retained.work.rebuild.entry.agent_identity,
                error = %retained.error,
                "retrying retained exact resume cleanup after lifecycle control"
            );
            if retained.retry_tx.send(()).is_err() {
                // The member's work already settled when this custody was
                // parked; only the effect obligation moves.
                self.park_unproven_resume_cleanup(
                    retained.work,
                    ProvisionAttemptFailure::unproven(MobError::Internal(
                        "resume cleanup owner is unavailable".to_string(),
                    )),
                );
            }
        }
        for retained in &self.unproven_resume_cleanup {
            tracing::error!(
                agent_identity = %retained.work.rebuild.entry.agent_identity,
                error = %retained.failure,
                "lifecycle control still requires owner settlement evidence"
            );
        }
    }
}

/// #1251 test seam: sessions whose failed attempt is reported as retained
/// cleanup custody held by the actor.
#[cfg(test)]
pub(in crate::runtime) static CLEANUP_HELD_TEST_SESSIONS: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashSet<SessionId>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashSet::new()));

/// #1251 test seam: notified when a member refused by an occupied claim
/// starts awaiting the claim-release event.
#[cfg(test)]
pub(in crate::runtime) static CLAIM_RELEASE_WAIT_TEST_HOOKS: std::sync::LazyLock<
    std::sync::Mutex<HashMap<SessionId, Arc<tokio::sync::Notify>>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(HashMap::new()));

async fn provision_explicit_resume_member(
    work: &ExplicitResumeMemberWork,
    recipe: Option<Box<DeferredResumeProvision>>,
    service: Arc<dyn MobSessionService>,
    provisioner: Arc<dyn MobProvisioner>,
    readiness: DetachedMemberReadinessContext,
) -> ExplicitResumeProvisionResult {
    let custody = if let Some(recipe) = recipe {
        // An attempt refused because another owner holds this session's
        // materialization claim created nothing (typed `NoEffect`) and is
        // transient: the owner releases the claim. Re-attempt on the typed
        // claim-release event, never on a timer (#1251). Every other outcome
        // settles the member as before.
        let receipt = loop {
            let mut request = match recipe.clone().into_request(Arc::clone(&service)).await {
                Ok(request) => request,
                Err(error) => return ExplicitResumeProvisionResult::NotProvisioned(error),
            };
            request.runtime_revival_intent =
                crate::runtime::provisioner::RuntimeRevivalIntent::MissingLiveMaterialization;
            #[cfg(feature = "runtime-adapter")]
            if let Some(adapter) = readiness.runtime_adapter.as_ref() {
                match adapter
                    .update_peer_ingress_context(&work.rebuild.bridge_session_id, false, None)
                    .await
                {
                    Ok(_)
                    | Err(
                        meerkat_runtime::RuntimeDriverError::NotFound { .. }
                        | meerkat_runtime::RuntimeDriverError::Destroyed
                        | meerkat_runtime::RuntimeDriverError::NotReady { .. },
                    ) => {}
                    Err(error) => {
                        return ExplicitResumeProvisionResult::NotProvisioned(MobError::Internal(
                            format!("failed detaching stale resume ingress: {error}"),
                        ));
                    }
                }
            }
            match provisioner.provision_member_settled(request).await {
                Ok(receipt) => break receipt,
                Err(failure)
                    if failure.settlement()
                        == crate::runtime::provisioner::ProvisionEffectSettlement::NoEffect(
                            crate::runtime::provisioner::PreEffectRefusal::RegistrationOwned,
                        ) =>
                {
                    #[cfg(feature = "runtime-adapter")]
                    if let Some(adapter) = readiness.runtime_adapter.as_ref() {
                        tracing::info!(
                            agent_identity = %work.rebuild.entry.agent_identity,
                            session_id = %work.rebuild.bridge_session_id,
                            "explicit resume member awaits the materialization claim release before re-attempting"
                        );
                        #[cfg(test)]
                        if let Some(entered) = CLAIM_RELEASE_WAIT_TEST_HOOKS
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .remove(&work.rebuild.bridge_session_id)
                        {
                            entered.notify_one();
                        }
                        adapter
                            .materialization_claim_released(&work.rebuild.bridge_session_id)
                            .await;
                        continue;
                    }
                    return ExplicitResumeProvisionResult::ProvisionFailed(failure);
                }
                Err(failure) => return ExplicitResumeProvisionResult::ProvisionFailed(failure),
            }
        };
        let provision = PendingProvision::new(
            receipt.member_ref,
            work.rebuild.entry.agent_identity.clone(),
            Arc::clone(&provisioner),
            receipt.operation_id,
            receipt.session_origin,
            receipt.rollback_authority,
            // A failed rebuild keeps the member seated.
            crate::runtime::provisioner::RollbackOrigin::RevivalOrRebuild,
        );
        let matches_session = match provision.member_ref() {
            Ok(member_ref) => {
                member_ref.bridge_session_id() == Some(&work.rebuild.bridge_session_id)
            }
            Err(error) => {
                return ExplicitResumeProvisionResult::FinalizationFailed { provision, error };
            }
        };
        if !matches_session {
            return ExplicitResumeProvisionResult::FinalizationFailed {
                provision,
                error: MobError::Internal("resume provision changed its exact session".to_string()),
            };
        }
        ExplicitResumeMemberCustody::Provisioned(provision)
    } else {
        return ExplicitResumeProvisionResult::Ready {
            custody: ExplicitResumeMemberCustody::Existing,
            recovered_endpoint: None,
        };
    };
    let finalized = async {
        readiness
            .ensure_mob_comms_drain(&work.rebuild.entry.agent_identity, &work.rebuild.member_ref)
            .await?;
        if work.rebuild.superseded_session_id.is_none()
            || work.rebuild.recovered_peer_endpoint.is_some()
        {
            return Ok(None);
        }
        let runtime = provisioner
            .comms_runtime(&work.rebuild.member_ref)
            .await
            .ok_or_else(|| MobError::Internal("resumed member has no comms runtime".to_string()))?;
        let peer_name = render_member_comms_name(
            readiness.mob_id.as_str(),
            work.rebuild.entry.role.as_str(),
            work.rebuild.entry.agent_identity.as_str(),
        )?;
        crate::runtime::provisioner::trusted_peer_spec_from_runtime(&peer_name, runtime.as_ref())?
            .map(Some)
            .ok_or_else(|| {
                MobError::Internal("resumed member has no exact peer endpoint".to_string())
            })
    }
    .await;
    match (custody, finalized) {
        (custody, Ok(recovered_endpoint)) => ExplicitResumeProvisionResult::Ready {
            custody,
            recovered_endpoint,
        },
        (ExplicitResumeMemberCustody::Existing, Err(error)) => {
            ExplicitResumeProvisionResult::NotProvisioned(error)
        }
        (ExplicitResumeMemberCustody::Provisioned(provision), Err(error)) => {
            ExplicitResumeProvisionResult::FinalizationFailed { provision, error }
        }
    }
}

async fn deliver_explicit_resume_provision(
    work: Arc<ExplicitResumeMemberWork>,
    result: ExplicitResumeProvisionResult,
    provisioner: Arc<dyn MobProvisioner>,
    command_tx: mpsc::Sender<RoutedMobCommand>,
) {
    match result {
        ExplicitResumeProvisionResult::Ready {
            custody,
            recovered_endpoint,
        } => {
            let (decision_tx, decision_rx) = oneshot::channel();
            let sent = command_tx
                .send(RoutedMobCommand::internal(
                    MobCommand::ResumeLifecycleMemberReady {
                        work: Arc::clone(&work),
                        recovered_endpoint,
                        decision_tx,
                    },
                ))
                .await;
            let decision = if sent.is_ok() {
                decision_rx.await.ok()
            } else {
                None
            };
            match decision {
                Some(ExplicitResumeMemberDecision::Accept) => {
                    if let ExplicitResumeMemberCustody::Provisioned(provision) = custody
                        && let Err(error) = provision.commit()
                    {
                        let _ = command_tx
                            .send(RoutedMobCommand::internal(
                                MobCommand::ResumeLifecycleMemberUnproven {
                                    work,
                                    failure: ProvisionAttemptFailure::unproven(error),
                                    stage: UnprovenResumeCustodyStage::AfterAcceptance,
                                },
                            ))
                            .await;
                        return;
                    }
                    send_member_completion(
                        work,
                        ExplicitResumeMemberCompletion::Accepted,
                        &command_tx,
                    )
                    .await;
                }
                decision => {
                    let reason = match decision {
                        Some(ExplicitResumeMemberDecision::Rollback(reason)) => reason,
                        _ => ExplicitResumeCleanupReason::Superseded,
                    };
                    compensate_resume_member(work, custody, reason, &command_tx).await;
                }
            }
        }
        ExplicitResumeProvisionResult::NotProvisioned(error) => {
            send_member_completion(
                work,
                ExplicitResumeMemberCompletion::Failed(error),
                &command_tx,
            )
            .await;
        }
        ExplicitResumeProvisionResult::ProvisionFailed(failure) => {
            #[cfg(test)]
            if CLEANUP_HELD_TEST_SESSIONS
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&work.rebuild.bridge_session_id)
            {
                // Model a retained cleanup whose automatic retry already
                // failed: the actor holds the retry trigger.
                let (retry_tx, retry_rx) = oneshot::channel();
                let _ = command_tx
                    .send(RoutedMobCommand::internal(
                        MobCommand::ResumeLifecycleMemberCleanupHeld {
                            work,
                            retry_tx,
                            error: failure.error().to_string(),
                            attempts: 2,
                            automatic_retry: false,
                        },
                    ))
                    .await;
                let _ = retry_rx.await;
                return;
            }
            if failure.requires_further_cleanup() {
                if failure.retained_effects().is_some() {
                    let (error, _, retained) = failure.into_parts();
                    if let Some(retained) = retained
                        && retry_retained_provision_effects(
                            Arc::clone(&work),
                            retained,
                            provisioner,
                            &command_tx,
                        )
                        .await
                    {
                        send_member_completion(
                            work,
                            ExplicitResumeMemberCompletion::Failed(error),
                            &command_tx,
                        )
                        .await;
                    }
                } else if command_tx
                    .send(RoutedMobCommand::internal(
                        MobCommand::ResumeLifecycleMemberUnproven {
                            work,
                            failure,
                            stage: UnprovenResumeCustodyStage::BeforeAcceptance,
                        },
                    ))
                    .await
                    .is_err()
                {
                    tracing::error!("unproven provisioning effects outlived the resume actor");
                }
            } else {
                send_member_completion(
                    work,
                    ExplicitResumeMemberCompletion::Failed(failure.into_error()),
                    &command_tx,
                )
                .await;
            }
        }
        ExplicitResumeProvisionResult::FinalizationFailed { provision, error } => {
            compensate_resume_member(
                work,
                ExplicitResumeMemberCustody::Provisioned(provision),
                ExplicitResumeCleanupReason::Failed(error),
                &command_tx,
            )
            .await;
        }
    }
}

async fn compensate_resume_member(
    work: Arc<ExplicitResumeMemberWork>,
    custody: ExplicitResumeMemberCustody,
    reason: ExplicitResumeCleanupReason,
    command_tx: &mpsc::Sender<RoutedMobCommand>,
) {
    if let ExplicitResumeMemberCustody::Provisioned(provision) = custody
        && let Err(retained) = provision.rollback_retaining_custody().await
    {
        if let Err(error) =
            retry_retained_resume_cleanup(Arc::clone(&work), retained, command_tx).await
        {
            tracing::error!(%error, "resume compensation remains unproven");
            return;
        }
    }
    let completion = match reason {
        ExplicitResumeCleanupReason::Superseded => ExplicitResumeMemberCompletion::Superseded,
        ExplicitResumeCleanupReason::Failed(error) => ExplicitResumeMemberCompletion::Failed(error),
    };
    send_member_completion(work, completion, command_tx).await;
}

async fn retry_retained_resume_cleanup(
    work: Arc<ExplicitResumeMemberWork>,
    mut custody: RetainedProvisionCustody,
    command_tx: &mpsc::Sender<RoutedMobCommand>,
) -> Result<(), MobError> {
    let mut attempts = 1;
    loop {
        let (retry_tx, retry_rx) = oneshot::channel();
        let notice = MobCommand::ResumeLifecycleMemberCleanupHeld {
            work: Arc::clone(&work),
            retry_tx,
            error: custody.error().to_string(),
            attempts,
            automatic_retry: true,
        };
        if command_tx
            .send(RoutedMobCommand::internal(notice))
            .await
            .is_err()
            || retry_rx.await.is_err()
        {
            return Err(custody.abandon("actor stopped before exact resume cleanup was settled"));
        }
        match custody.retry().await {
            Ok(()) => return Ok(()),
            Err(retained) => {
                custody = retained;
                attempts = attempts.saturating_add(1);
            }
        }
    }
}

async fn retry_retained_provision_effects(
    work: Arc<ExplicitResumeMemberWork>,
    mut retained: RetainedProvisionEffects,
    provisioner: Arc<dyn MobProvisioner>,
    command_tx: &mpsc::Sender<RoutedMobCommand>,
) -> bool {
    let mut attempts = 0_u32;
    loop {
        attempts = attempts.saturating_add(1);
        let failure = match provisioner
            .retry_retained_provision_cleanup(
                retained,
                crate::runtime::provisioner::RollbackOrigin::RevivalOrRebuild,
            )
            .await
        {
            Ok(()) => return true,
            Err(failure) => failure,
        };
        let automatic_retry = !failure.is_unsupported();
        let error = failure.error().to_string();
        (retained, _) = failure.into_parts();
        let (retry_tx, retry_rx) = oneshot::channel();
        let notice = MobCommand::ResumeLifecycleMemberCleanupHeld {
            work: Arc::clone(&work),
            retry_tx,
            error,
            attempts,
            automatic_retry,
        };
        if command_tx
            .send(RoutedMobCommand::internal(notice))
            .await
            .is_err()
            || retry_rx.await.is_err()
        {
            tracing::error!(
                member_ref = ?retained.member_ref(),
                operation_anchor = ?retained.operation_anchor(),
                detail = retained.detail(),
                "actor stopped with retained provisioning effects; cleanup is not certified"
            );
            return false;
        }
    }
}

async fn send_member_completion(
    work: Arc<ExplicitResumeMemberWork>,
    completion: ExplicitResumeMemberCompletion,
    command_tx: &mpsc::Sender<RoutedMobCommand>,
) {
    if command_tx
        .send(RoutedMobCommand::internal(
            MobCommand::ResumeLifecycleMemberSettled { work, completion },
        ))
        .await
        .is_err()
    {
        tracing::warn!("resume owner settled after its actor stopped");
    }
}
