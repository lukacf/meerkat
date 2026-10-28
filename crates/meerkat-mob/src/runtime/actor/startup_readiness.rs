//! The startup readiness pass of a resumed mob.
//!
//! The members a crash-path start materialized (its boot set) leave the
//! builder bound and Waking: their readiness and topology settle before
//! MobMachine holds them Live (Live means ready). The topology settled in the builder;
//! this pass runs their readiness with the readiness of every other member
//! materialized in this process, then applies each boot member's
//! `ResolveMemberRevivalSucceeded` under its open attempt. A boot member
//! the start left to be recovered right after it gets no readiness run
//! here. One whose stopped registration the start took over has no
//! registration left: its attempt closes as NotCurrent, so MobMachine holds
//! it Dormant, and its recovery wakes it through the ordinary wake (the
//! host's build preparer, readiness, topology) to Live. One whose
//! registration needs a durability reload is settled Live, because its
//! session is still materialized and the warm reload that recovers it
//! starts from a Live member (the drift arm), as an operator reload of a
//! degraded Live member does; the reload then runs its readiness.
//!
//! A member's readiness failure is that member's outcome, never the
//! mob's: a boot member's attempt is abandoned with the typed
//! readiness failure, which records it Broken; a Live member whose comms
//! runtime is gone is woken again through the drift arm; any other Live
//! member failure records that member Broken. The mob keeps serving every
//! other member. An explicit resume's post-commit readiness pass resolves
//! its members' failures the same way, and the resume does not answer them.

use super::*;

impl MobActor {
    /// Run the startup readiness of every member materialized in this
    /// process and settle the start's boot set (see the module docs).
    /// Errors only for failures that are not one member's: a lifecycle that
    /// fences readiness, or a refused machine input.
    ///
    /// It runs in `prepare_actor_run`, before the command loop exists: every
    /// member's bounded readiness step runs concurrently, so the pass is
    /// bounded by one `RESUME_MEMBER_READINESS_BOUND` rather than by the
    /// member count, and commands sent meanwhile queue until the loop starts.
    /// It fires no synthetic kickoff turn. Once a local autonomous member's
    /// drain and injector were observed, its exact runtime and fence are
    /// published through MobMachine's startup readiness transition: durable
    /// membership alone is not readiness.
    pub(super) async fn ensure_startup_member_readiness(&mut self) -> Result<(), MobError> {
        let Some(mut targets) = self.collect_member_readiness_targets(false).await? else {
            // No readiness runs while a lifecycle cleanup owns the mob: the
            // boot set is settled as every other member is, without one.
            let _ = self.settle_held_member_readiness(Vec::new()).await;
            return Ok(());
        };
        // The boot set is Waking, so the Live-only target set leaves it out:
        // its readiness is this start's to run.
        targets.extend(self.held_member_readiness_targets().await);
        #[cfg(feature = "runtime-adapter")]
        let targets = self
            .break_members_whose_runtime_ended_outside_the_mob(targets)
            .await;
        let expected = member_readiness_expectations(&targets);
        let readiness = self.spawn_member_readiness_tasks(targets, None);
        let outcomes =
            collect_member_readiness_outcomes(readiness, expected, &self.definition.id).await;
        // A boot member's failure is its own (it is Broken); the start goes on.
        let (outcomes, _) = self.settle_held_member_readiness(outcomes).await;
        let mut settled = Vec::with_capacity(outcomes.len());
        for outcome in outcomes {
            if let Err(error) = &outcome.result {
                self.resolve_live_member_readiness_failure(&outcome.entry, error, "at startup")
                    .await;
                continue;
            }
            settled.push(outcome);
        }
        // Every outcome left is a success: the kept-kickoff re-arm and the
        // autonomous startup marks run as for any readiness pass.
        self.apply_member_readiness_outcomes(settled).await
    }

    /// The readiness targets of the members held Waking for a readiness pass
    /// (`startup_boot_attempts`: a start's boot set, or the members an
    /// explicit resume rebuilt). The Live-only target set leaves them out.
    ///
    /// A held member this start left to be recovered right after it (its
    /// stopped registration taken over, or its registration needing a
    /// durability reload) is left out as the Live-only target set leaves it
    /// out: it has no registration to make ready now, and its recovery runs
    /// its readiness on the registration it wakes or reloads
    /// (`recover_startup_members`). The settlement returns a taken-over
    /// member to Dormant and settles a reload-required one Live (see the
    /// module docs).
    pub(super) async fn held_member_readiness_targets(&self) -> Vec<MemberReadinessTarget> {
        let roster = self.roster.read().await;
        let state = self.dsl_authority.state();
        self.startup_boot_attempts
            .keys()
            .filter(|identity| !self.recovered_after_start(identity))
            .filter_map(|identity| roster.get(identity))
            .map(|entry| MemberReadinessTarget {
                entry: entry.clone(),
                stage: member_readiness_stage(entry),
                drain_required: member_requires_comms_drain(state, entry),
            })
            .collect()
    }

    /// Settle every member held Waking for this readiness pass from its
    /// outcome: Live once its readiness succeeded (Live means ready), Broken
    /// with the readiness failure otherwise (member-scoped; the pass goes
    /// on). A held member the pass recorded Broken before its readiness (its
    /// runtime ended outside the mob) has its attempt closed with that
    /// failure. A held member the pass ran no readiness for (none runs while
    /// a lifecycle cleanup owns the mob, the pass was not dispatched, or the
    /// start left the member to be recovered right after it) is settled Live
    /// as every other member is then, except one whose stopped registration
    /// the start took over: it goes back to Dormant, and its recovery wakes
    /// it. Returns the outcomes left to apply (every other member's, and each
    /// held member's success) and the first held member's readiness failure,
    /// which a caller may report.
    pub(super) async fn settle_held_member_readiness(
        &mut self,
        outcomes: Vec<MemberReadinessOutcome>,
    ) -> (Vec<MemberReadinessOutcome>, Option<MobError>) {
        let held = std::mem::take(&mut self.startup_boot_attempts);
        self.settle_held_readiness(held, outcomes).await
    }

    /// [`Self::settle_held_member_readiness`] for one held member alone: an
    /// explicit resume's rebuilt member, whose own readiness ran right after
    /// its acceptance. Every other held member stays held.
    pub(super) async fn settle_held_member_readiness_of(
        &mut self,
        identity: &AgentIdentity,
        outcomes: Vec<MemberReadinessOutcome>,
    ) -> (Vec<MemberReadinessOutcome>, Option<MobError>) {
        let held: BTreeMap<AgentIdentity, u64> = self
            .startup_boot_attempts
            .remove(identity)
            .map(|attempt| (identity.clone(), attempt))
            .into_iter()
            .collect();
        self.settle_held_readiness(held, outcomes).await
    }

    async fn settle_held_readiness(
        &mut self,
        mut held: BTreeMap<AgentIdentity, u64>,
        outcomes: Vec<MemberReadinessOutcome>,
    ) -> (Vec<MemberReadinessOutcome>, Option<MobError>) {
        let mut rest = Vec::with_capacity(outcomes.len());
        let mut first_failure = None;
        for outcome in outcomes {
            match (held.remove(&outcome.entry.agent_identity), outcome.result) {
                (Some(attempt), Ok(())) => {
                    self.settle_startup_boot_member_live(&outcome.entry, attempt)
                        .await;
                    rest.push(MemberReadinessOutcome {
                        result: Ok(()),
                        ..outcome
                    });
                }
                (Some(attempt), Err(error)) => {
                    self.abandon_startup_boot_member(&outcome.entry, attempt, &error)
                        .await;
                    first_failure.get_or_insert(error);
                }
                (None, result) => rest.push(MemberReadinessOutcome { result, ..outcome }),
            }
        }
        for (identity, attempt) in held {
            let entry = self.roster.read().await.get(&identity).cloned();
            // A member that left the roster closed its attempt as it left.
            let Some(entry) = entry else {
                continue;
            };
            let broken = self
                .dsl_authority
                .state()
                .member_restore_failures
                .get(&mob_dsl::AgentIdentity::from_domain(&identity))
                .cloned();
            match broken {
                Some(reason) => {
                    let error = MobError::Internal(reason.to_string());
                    self.abandon_startup_boot_member(&entry, attempt, &error)
                        .await;
                }
                None if self.taken_over_at_start(&identity) => {
                    self.return_taken_over_boot_member_to_dormant(&entry, attempt);
                }
                None => self.settle_startup_boot_member_live(&entry, attempt).await,
            }
        }
        (rest, first_failure)
    }

    /// Whether this start took over `agent_identity`'s stopped runtime
    /// registration (`StartupMemberRecoveryCause::StoppedResidue`): the
    /// member has no registration left until its recovery wakes it.
    fn taken_over_at_start(&self, agent_identity: &AgentIdentity) -> bool {
        #[cfg(feature = "runtime-adapter")]
        {
            self.startup_member_recoveries
                .get(agent_identity)
                .is_some_and(|recovery| {
                    recovery.cause == super::StartupMemberRecoveryCause::StoppedResidue
                })
        }
        #[cfg(not(feature = "runtime-adapter"))]
        {
            let _ = agent_identity;
            false
        }
    }

    /// A boot member whose stopped registration this start took over: its
    /// attempt closes as NotCurrent, so MobMachine holds it Dormant (its
    /// boot obligation kept) instead of Live with no registration and no
    /// readiness. Its recovery, dispatched right after the start, wakes it
    /// from Dormant (`WakeMember`, the host's build preparer, readiness and
    /// topology), and that wake settles it Live once it is ready and reports
    /// its build. A stale attempt is the machine's no-op.
    fn return_taken_over_boot_member_to_dormant(&mut self, entry: &RosterEntry, attempt: u64) {
        let verdict = self.apply_member_wake_feedback(
            &entry.agent_identity,
            attempt,
            mob_dsl::MobMachineSignal::AbandonMemberWake {
                agent_identity: mob_dsl::AgentIdentity::from_domain(&entry.agent_identity),
                attempt,
                kind: mob_dsl::MemberWakeAbandonKind::NotCurrent,
                reason: format!(
                    "the start took over the stopped runtime registration of member '{}'; its recovery wakes it",
                    entry.agent_identity
                ),
            },
            "startup_taken_over_member_dormant",
        );
        if verdict != mob_dsl::MemberWakeFeedbackVerdict::Applied {
            tracing::info!(
                mob_id = %self.definition.id,
                agent_identity = %entry.agent_identity,
                attempt,
                "a taken-over boot member's attempt closed before the start returned it to Dormant"
            );
        }
    }

    /// A boot member's readiness settled: MobMachine holds it Live under the
    /// attempt its plan opened (Live means ready). A stale attempt (the
    /// member's attempt closed meanwhile) is the machine's no-op.
    async fn settle_startup_boot_member_live(&mut self, entry: &RosterEntry, attempt: u64) {
        let identity = mob_dsl::AgentIdentity::from_domain(&entry.agent_identity);
        match self.apply_dsl_signal_collect_transition(
            mob_dsl::MobMachineSignal::ResolveMemberRevivalSucceeded {
                agent_identity: identity.clone(),
                attempt,
            },
            "startup_boot_member_ready",
        ) {
            Ok(transition)
                if super::super::boot_plan::wake_feedback_applied(
                    transition.effects(),
                    &identity,
                    attempt,
                ) =>
            {
                self.report_startup_boot_member_built(entry);
            }
            Ok(_) => {
                tracing::info!(
                    mob_id = %self.definition.id,
                    agent_identity = %entry.agent_identity,
                    attempt,
                    "a boot member's attempt closed before its startup readiness settled it"
                );
            }
            Err(error) => {
                // A refused readiness never leaves the member Waking under an
                // attempt nothing closes (every later delivery would wait on
                // it): the attempt is abandoned, member-scoped, with the
                // refusal as the reason.
                tracing::error!(
                    mob_id = %self.definition.id,
                    agent_identity = %entry.agent_identity,
                    %error,
                    "MobMachine refused the startup readiness of a boot member; its attempt is abandoned"
                );
                self.abandon_startup_boot_member(entry, attempt, &error)
                    .await;
            }
        }
    }

    /// Tell the host's build preparer that a boot member was built, off the
    /// actor (the visible tool record of the member's build).
    fn report_startup_boot_member_built(&mut self, entry: &RosterEntry) {
        #[cfg(feature = "runtime-adapter")]
        {
            let Some(preparer) = self.member_residency.build_preparer.clone() else {
                return;
            };
            let Some(session_id) = entry.member_ref.bridge_session_id().cloned() else {
                return;
            };
            let session_service = Arc::clone(&self.session_service);
            let identity = entry.agent_identity.clone();
            self.actor_io_tasks.spawn(async move {
                crate::runtime::member_build_preparer::report_member_built(
                    Some(preparer),
                    session_service,
                    identity,
                    session_id,
                )
                .await;
            });
        }
        #[cfg(not(feature = "runtime-adapter"))]
        let _ = entry;
    }

    /// A boot member's readiness failed: its attempt is abandoned with the
    /// typed readiness failure, which records it Broken (member-scoped).
    async fn abandon_startup_boot_member(
        &mut self,
        entry: &RosterEntry,
        attempt: u64,
        error: &MobError,
    ) {
        let identity = mob_dsl::AgentIdentity::from_domain(&entry.agent_identity);
        let reason = format!(
            "member '{}' was restored at this start but its readiness failed: {error}",
            entry.agent_identity
        );
        tracing::warn!(
            mob_id = %self.definition.id,
            agent_identity = %entry.agent_identity,
            %error,
            "boot member readiness failed; the member is Broken and the mob keeps running"
        );
        match self.apply_dsl_signal_collect_transition(
            mob_dsl::MobMachineSignal::AbandonMemberWake {
                agent_identity: identity.clone(),
                attempt,
                kind: mob_dsl::MemberWakeAbandonKind::ReadinessFailed,
                reason: reason.clone(),
            },
            "startup_boot_member_readiness_failed",
        ) {
            Ok(transition)
                if super::super::boot_plan::wake_feedback_applied(
                    transition.effects(),
                    &identity,
                    attempt,
                ) =>
            {
                self.record_startup_member_broken(entry, reason).await;
            }
            Ok(_) => {}
            Err(error) => {
                tracing::error!(
                    mob_id = %self.definition.id,
                    agent_identity = %entry.agent_identity,
                    %error,
                    "MobMachine refused to abandon a boot member's failed readiness"
                );
            }
        }
    }

    /// The Live members' readiness failures of an explicit resume's
    /// post-commit readiness pass, resolved as the startup pass resolves
    /// them: each is that member's outcome, never the lift's. The pass ran
    /// while the mob served, so a failure is resolved only while it still
    /// describes the member: the roster holds the incarnation it observed,
    /// and MobMachine holds the member materialized under the epoch it held
    /// when the pass began (`target_epochs`), neither Broken nor retiring.
    /// The failure of a member retired, respawned, released (and perhaps
    /// woken again) or recorded Broken while its readiness ran is dropped:
    /// that path owns the member's outcome. Returns the outcomes left to
    /// apply (the successes).
    pub(super) async fn resolve_live_member_readiness_failures(
        &mut self,
        outcomes: Vec<MemberReadinessOutcome>,
        target_epochs: &BTreeMap<mob_dsl::AgentIdentity, Option<mob_dsl::RuntimeEpochId>>,
    ) -> Vec<MemberReadinessOutcome> {
        let mut rest = Vec::with_capacity(outcomes.len());
        for outcome in outcomes {
            if outcome.result.is_ok() {
                rest.push(outcome);
                continue;
            }
            if !self
                .live_readiness_target_is_current(&outcome.entry, target_epochs)
                .await
            {
                tracing::info!(
                    mob_id = %self.definition.id,
                    agent_identity = %outcome.entry.agent_identity,
                    "a member changed while its readiness ran; its readiness failure is dropped"
                );
                continue;
            }
            if let Err(error) = &outcome.result {
                self.resolve_live_member_readiness_failure(
                    &outcome.entry,
                    error,
                    "at the explicit resume",
                )
                .await;
            }
        }
        rest
    }

    /// Whether the member a readiness target observed is still that Live
    /// target: the same incarnation in the roster, materialized under the
    /// epoch it held when the pass began, neither Broken nor retiring.
    async fn live_readiness_target_is_current(
        &self,
        observed: &RosterEntry,
        target_epochs: &BTreeMap<mob_dsl::AgentIdentity, Option<mob_dsl::RuntimeEpochId>>,
    ) -> bool {
        let same_incarnation = self
            .roster
            .read()
            .await
            .get(&observed.agent_identity)
            .is_some_and(|entry| {
                entry.agent_runtime_id == observed.agent_runtime_id
                    && entry.fence_token == observed.fence_token
            });
        let state = self.dsl_authority.state();
        let identity = mob_dsl::AgentIdentity::from_domain(&observed.agent_identity);
        same_incarnation
            && target_epochs.get(&identity) == state.member_materialization_epoch.get(&identity)
            && !state.member_restore_failures.contains_key(&identity)
            && !super::super::boot_plan::domain_member_is_unmaterialized(
                state,
                &observed.agent_identity,
            )
            && !state.runtime_retire_pending_sessions.contains_key(
                &mob_dsl::AgentRuntimeId::from_domain(&observed.agent_runtime_id),
            )
    }

    /// A Live member's readiness failed, observed by the readiness pass
    /// `pass` names (its diagnostic says where). A member whose comms runtime
    /// is gone is not materialized any more: the drift arm makes it Waking and
    /// the wake path materializes it again. Any other failure records the
    /// member Broken. Neither stops the mob.
    async fn resolve_live_member_readiness_failure(
        &mut self,
        entry: &RosterEntry,
        error: &MobError,
        pass: &'static str,
    ) {
        let identity = mob_dsl::AgentIdentity::from_domain(&entry.agent_identity);
        let lost_materialization = matches!(
            error,
            MobError::MissingMemberCapability {
                capability: crate::error::MobMemberCapability::OutboundCommsRuntime,
                ..
            }
        );
        tracing::warn!(
            mob_id = %self.definition.id,
            agent_identity = %entry.agent_identity,
            %error,
            lost_materialization,
            pass,
            "member readiness failed; the outcome is the member's, the mob keeps running"
        );
        if lost_materialization {
            let transition = match self.apply_dsl_signal_collect_transition(
                mob_dsl::MobMachineSignal::ClassifyMemberLiveMaterialization {
                    agent_identity: identity.clone(),
                    observation:
                        mob_dsl::MemberLiveMaterializationObservationKind::DurableSnapshotPresent,
                    reason: format!("the member's comms runtime is missing {pass}: {error}"),
                },
                "startup_member_materialization_lost",
            ) {
                Ok(transition) => transition,
                Err(classify_error) => {
                    tracing::error!(
                        mob_id = %self.definition.id,
                        agent_identity = %entry.agent_identity,
                        error = %classify_error,
                        "MobMachine refused the startup drift observation of a member"
                    );
                    return;
                }
            };
            // The drift arm opens a wake attempt and hands nothing off
            // itself: the member's materialization is queued here, as the
            // wake path queues its own drift, so the member does not stay
            // Waking under an attempt nothing realizes.
            let authorized = transition.effects().iter().find_map(|effect| match effect {
                mob_dsl::MobMachineEffect::MemberLiveMaterializationClassified {
                    agent_identity,
                    verdict: mob_dsl::MemberRevivalVerdictKind::ReviveAuthorized,
                    attempt: Some(attempt),
                    ..
                } if agent_identity == &identity => Some(*attempt),
                _ => None,
            });
            if let Some(attempt) = authorized
                && !self
                    .member_wakes
                    .pending
                    .iter()
                    .any(|pending| pending.attempt == attempt)
            {
                self.member_wakes.pending.push_back(
                    super::member_wake::PendingMemberMaterialization {
                        identity: entry.agent_identity.clone(),
                        cause: mob_dsl::MemberWakeCause::Boot,
                        attempt,
                    },
                );
            }
            return;
        }
        let reason = format!(
            "member '{}' readiness failed {pass}: {error}",
            entry.agent_identity
        );
        if let Err(recover_error) = self.apply_dsl_signal(
            mob_dsl::MobMachineSignal::RecoverMemberRestoreFailure {
                agent_identity: identity,
                reason: reason.clone(),
            },
            "startup_member_readiness_failed",
        ) {
            tracing::error!(
                mob_id = %self.definition.id,
                agent_identity = %entry.agent_identity,
                error = %recover_error,
                "MobMachine refused the startup readiness failure of a member"
            );
            return;
        }
        self.record_startup_member_broken(entry, reason).await;
    }

    /// The handle diagnostic of a member the startup readiness recorded
    /// Broken, with its projected comms identity cleared, as every other
    /// restore failure leaves it.
    async fn record_startup_member_broken(&mut self, entry: &RosterEntry, reason: String) {
        {
            let mut roster = self.roster.write().await;
            let mut snapshot = roster.snapshot();
            let _ = snapshot.set_comms_identity(&entry.agent_identity, None, None);
            *roster = super::super::roster_authority::RosterAuthority::from_roster(snapshot);
        }
        self.record_member_restore_failure(
            &entry.agent_identity,
            super::super::handle::RestoreFailureDiagnostic {
                bridge_session_id: entry.member_ref.bridge_session_id().cloned(),
                reason,
                hold: None,
                registration_refusal: None,
            },
        )
        .await;
        self.publish_machine_state_projection();
    }
}
