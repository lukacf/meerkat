//! Shutdown's runtime teardown, off the actor loop.
//!
//! Each member session is unregistered through its own exact-registration
//! runtime authority: admitting the unregister takes the session's
//! registration transaction, and the runtime's unregister coordinator then
//! owns the teardown. One slow session (a held registration transaction, a
//! drain waiting on a turn) must not freeze the actor or the other sessions,
//! so every session is admitted and awaited concurrently, in an actor-owned
//! task, bounded by the Shutdown's budget: the caller's deadline, or the
//! member lifecycle hang guard without one. The parked Shutdown re-enters
//! with every session's typed outcome; meanwhile the actor keeps serving
//! status and observation commands.
//!
//! A bound member whose runtime the teardown unregistered has its identity
//! reconcile lease released there, once the member's session actor has
//! exited and so released the session's hosting claim, so the next
//! incarnation takes the identity at once instead of waiting the lease out.

use super::*;

/// A Shutdown parked while its runtime teardown runs off the actor loop.
pub(in crate::runtime) struct PendingShutdownTeardown {
    ticket: u64,
    /// The result of the Shutdown steps before teardown.
    prior: Result<(), MobError>,
    reply_tx: super::LifecycleReplyTx,
    /// Shutdowns that arrived while this one was parked; they receive its
    /// result.
    pub(super) joined: Vec<super::LifecycleReplyTx>,
    /// The member each session belongs to, for the report.
    members: HashMap<SessionId, AgentIdentity>,
}

/// How one session's runtime unregister settled within the Shutdown's budget.
pub(in crate::runtime) enum ShutdownUnregisterOutcome {
    /// The session's runtime binding is gone.
    Unregistered,
    /// The unregister is not complete: `stage` names where it stands. The
    /// runtime coordinator, when one was admitted, keeps running on its own
    /// and its observer is retained; `settlement` follows the exact
    /// registration for the report.
    #[cfg(feature = "runtime-adapter")]
    Pending {
        stage: &'static str,
        observer: Option<meerkat_runtime::RuntimeSessionUnregisterObserver>,
        settlement: crate::runtime::MemberUnregisterSettlement,
    },
    /// The unregister failed or its exact registration changed.
    Failed(String),
}

/// A Shutdown with no runtime teardown to park: its result so far and its
/// reply channels, handed back so the caller finishes it inline.
pub(super) struct InlineShutdownTail {
    pub(super) prior: Result<(), MobError>,
    pub(super) reply_tx: super::LifecycleReplyTx,
    pub(super) joined: Vec<super::LifecycleReplyTx>,
}

impl MobActor {
    /// Run Shutdown's runtime teardown off the actor loop and park the
    /// Shutdown until it reports. Returns the reply and its joiners back when
    /// there is no runtime to tear down, so the caller finishes inline.
    pub(super) fn park_shutdown_teardown(
        &mut self,
        prior: Result<(), MobError>,
        reply_tx: super::LifecycleReplyTx,
        joined: Vec<super::LifecycleReplyTx>,
    ) -> Result<(), Box<InlineShutdownTail>> {
        #[cfg(feature = "runtime-adapter")]
        if let Some(adapter) = self.runtime_adapter.clone() {
            let (members, bound, targets) = match self.shutdown_teardown_targets() {
                Ok(targets) => targets,
                Err(error) => {
                    return Err(Box::new(InlineShutdownTail {
                        prior: prior.and(Err(error)),
                        reply_tx,
                        joined,
                    }));
                }
            };
            let ticket = self.next_autonomous_stop_ticket;
            self.next_autonomous_stop_ticket = ticket.wrapping_add(1);
            let deadline = Instant::now() + self.shutdown_wait_budget();
            // The Shutdown's choice applies to every unregister it begins for
            // a member it keeps: a bound member's session. Its default
            // resolves per session: a session that commits each round keeps
            // its running turn and eligible queued inputs for the next start
            // (`member_sessions_commit_each_round`; every session this
            // teardown unregisters lives on the mob's own service and
            // runtime). A run that commits no rounds (a live bridge run) is
            // still cancelled by the runtime stop. A retiring member's session
            // is not expected to resume, so its queued inputs end stopped, as
            // its retirement would end them. An unregister an earlier Shutdown
            // left with its coordinator keeps the choice it began with; this
            // one's applies only if that coordinator ends without
            // unregistering and the unregister is begun again.
            let requested = self
                .shutdown_queued_inputs
                .unregister_queued_inputs(self.member_sessions_commit_each_round());
            let targets = targets
                .into_iter()
                .map(|(session_id, retained)| {
                    let queued_inputs = if bound.contains_key(&session_id) {
                        requested
                    } else {
                        meerkat_runtime::UnregisterQueuedInputs::Stop
                    };
                    (session_id, retained, queued_inputs)
                })
                .collect::<Vec<_>>();
            tracing::info!(
                mob_id = %self.definition.id,
                sessions = targets.len(),
                budget_ms = deadline.saturating_duration_since(Instant::now()).as_millis() as u64,
                "shutdown runtime teardown started off the actor loop"
            );
            #[cfg(test)]
            self.lifecycle_observations
                .shutdown_teardown_parked
                .send_replace(true);
            self.pending_shutdown_teardown = Some(PendingShutdownTeardown {
                ticket,
                prior,
                reply_tx,
                joined,
                members,
            });
            let command_tx = self.command_tx.clone();
            let mob_id = self.definition.id.clone();
            let ops_adapter = Arc::clone(&self.session_ops_adapter);
            let lease_release = Arc::new(ShutdownLeaseRelease {
                store: Arc::clone(&self.identity),
                session_service: Arc::clone(&self.session_service),
                hosting: adapter.hosting_authority(),
                mob_id: mob_id.clone(),
                holder_id: self.identity_reconcile_holder_id.clone(),
                incarnation_id: self.identity_reconcile_incarnation_id.clone(),
                members: bound,
            });
            self.actor_io_tasks.spawn(async move {
                let settled = futures::future::join_all(targets.into_iter().map(
                    |(session_id, retained, queued_inputs)| {
                        let adapter = Arc::clone(&adapter);
                        let ops_adapter = Arc::clone(&ops_adapter);
                        let lease_release = Arc::clone(&lease_release);
                        async move {
                            let outcome = teardown_session(
                                &adapter,
                                &ops_adapter,
                                &session_id,
                                retained,
                                deadline,
                                queued_inputs,
                            )
                            .await;
                            // Shutdown is done with the member for good
                            // (#1500): a `MobStop` an earlier Stop staged must
                            // not hold a later registration of the session.
                            // Dropped after the teardown, so a successor the
                            // teardown's own discard publishes registers as
                            // held as before.
                            adapter.unstage_registration_run_start_hold(
                                &session_id,
                                meerkat_runtime::RunStartHoldReason::MobStop,
                            );
                            let lease =
                                if matches!(outcome, ShutdownUnregisterOutcome::Unregistered) {
                                    lease_release.release(&session_id, deadline).await
                                } else {
                                    None
                                };
                            (session_id, outcome, lease)
                        }
                    },
                ))
                .await;
                let mut outcomes = Vec::with_capacity(settled.len());
                let mut leases = Vec::new();
                for (session_id, outcome, lease) in settled {
                    outcomes.push((session_id, outcome));
                    leases.extend(lease);
                }
                lease_release.log_summary(&leases);
                if command_tx
                    .send(RoutedMobCommand::internal(
                        MobCommand::ShutdownTeardownResolved { ticket, outcomes },
                    ))
                    .await
                    .is_err()
                {
                    tracing::warn!(%mob_id, ticket, "shutdown teardown settled after the actor stopped");
                }
            });
            return Ok(());
        }
        Err(Box::new(InlineShutdownTail {
            prior,
            reply_tx,
            joined,
        }))
    }

    /// The sessions Shutdown unregisters: bound members' sessions, the
    /// sessions of Retiring members (their binding moved to the machine's
    /// retire-pending map, including stuck and interrupted retirements), and
    /// any unregister a previous attempt left with its coordinator. Returns
    /// every member by session, the bound members by session, and the
    /// targets.
    #[cfg(feature = "runtime-adapter")]
    #[allow(clippy::type_complexity)]
    fn shutdown_teardown_targets(
        &mut self,
    ) -> Result<
        (
            HashMap<SessionId, AgentIdentity>,
            HashMap<SessionId, AgentIdentity>,
            Vec<(
                SessionId,
                Option<meerkat_runtime::RuntimeSessionUnregisterObserver>,
            )>,
        ),
        MobError,
    > {
        let state = self.dsl_authority.state();
        let parse = |session_id: &mob_dsl::SessionId| {
            SessionId::parse(&session_id.0).map_err(|error| {
                MobError::Internal(format!(
                    "shutdown found invalid machine-owned session binding '{}': {error}",
                    session_id.0
                ))
            })
        };
        let mut members: HashMap<SessionId, AgentIdentity> = HashMap::new();
        for (identity, session_id) in &state.member_session_bindings {
            if !state.member_placement.contains_key(identity) {
                members.insert(parse(session_id)?, AgentIdentity::from(identity.0.as_str()));
            }
        }
        let bound = members.clone();
        for (identity, runtime_id) in &state.identity_to_runtime {
            if state.member_placement.contains_key(identity) {
                continue;
            }
            if let Some(session_id) = state.runtime_retire_pending_sessions.get(runtime_id) {
                members
                    .entry(parse(session_id)?)
                    .or_insert_with(|| AgentIdentity::from(identity.0.as_str()));
            }
        }
        let mut retained = std::mem::take(&mut self.shutdown_runtime_unregister_observers);
        let mut targets = members
            .keys()
            .map(|session_id| (session_id.clone(), retained.remove(session_id)))
            .collect::<Vec<_>>();
        targets.extend(
            retained
                .into_iter()
                .map(|(session_id, observer)| (session_id, Some(observer))),
        );
        Ok((members, bound, targets))
    }

    /// Re-entry for a Shutdown parked on its runtime teardown.
    pub(super) async fn resolve_shutdown_teardown(
        &mut self,
        ticket: u64,
        outcomes: Vec<(SessionId, ShutdownUnregisterOutcome)>,
        command_rx: &mut crate::tokio::sync::mpsc::Receiver<
            super::super::scope_gate::RoutedMobCommand,
        >,
    ) -> ActorLoopControl {
        let Some(pending) = self
            .pending_shutdown_teardown
            .take_if(|pending| pending.ticket == ticket)
        else {
            tracing::warn!(
                mob_id = %self.definition.id,
                ticket,
                "ignoring stale shutdown teardown resolution"
            );
            return ActorLoopControl::ProceedBoundary;
        };
        let PendingShutdownTeardown {
            prior,
            reply_tx,
            joined,
            members,
            ..
        } = pending;
        let mut failures = Vec::new();
        for (session_id, outcome) in outcomes {
            let member = members.get(&session_id);
            match outcome {
                ShutdownUnregisterOutcome::Unregistered => {
                    self.record_shutdown_unregistered(member);
                }
                #[cfg(feature = "runtime-adapter")]
                ShutdownUnregisterOutcome::Pending {
                    stage,
                    observer,
                    settlement,
                } => {
                    self.record_shutdown_unregister_pending(member, stage, settlement);
                    if let Some(observer) = observer {
                        self.shutdown_runtime_unregister_observers
                            .insert(session_id, observer);
                    }
                }
                ShutdownUnregisterOutcome::Failed(failure) => failures.push(failure),
            }
        }
        let teardown = if failures.is_empty() {
            Ok(())
        } else {
            let error = MobError::Internal(failures.join("; "));
            tracing::warn!(error = %error, "shutdown session binding teardown failed");
            Err(error)
        };
        tracing::info!(
            mob_id = %self.definition.id,
            "mob shutdown step completed: session_runtime_bindings_torn_down"
        );
        Box::pin(self.finish_shutdown(prior.and(teardown), reply_tx, joined, command_rx)).await
    }
}

/// Releases a bound member's identity reconcile lease once Shutdown's
/// teardown unregistered the member's runtime.
///
/// Only a lease this incarnation holds is released, and only once this
/// process no longer hosts the member's session: the member's session actor
/// holds the session's hosting claim until it exits, and the runtime
/// unregister does not wait for that exit. A successor given the identity
/// earlier would find the session still hosted here.
///
/// The release is a compare-and-swap on the exact claim this incarnation
/// holds: a lease another incarnation holds, or one renewed since it was
/// read, stays. A Retiring member keeps its lease, and its retirement resumes
/// under the next incarnation once the lease expires. So does a member whose
/// unregister is still running at the deadline, a member whose session actor
/// outlives the deadline, and any lease a store error leaves.
#[cfg(feature = "runtime-adapter")]
struct ShutdownLeaseRelease {
    store: Arc<dyn crate::store::MobIdentityStore>,
    session_service: Arc<dyn MobSessionService>,
    hosting: meerkat_runtime::SessionHostingAuthority,
    mob_id: crate::ids::MobId,
    holder_id: String,
    incarnation_id: String,
    /// The bound members, by session.
    members: HashMap<SessionId, AgentIdentity>,
}

/// What a member's finished teardown did with the member's identity lease.
#[cfg(feature = "runtime-adapter")]
enum ShutdownLeaseOutcome {
    /// Released for the next incarnation.
    Released,
    /// Kept until it expires, and why.
    Kept(&'static str),
}

#[cfg(feature = "runtime-adapter")]
impl ShutdownLeaseRelease {
    /// Release the lease of `session_id`'s bound member, within `deadline`.
    /// None when the session is not a bound member's, or the member holds no
    /// lease of this incarnation.
    async fn release(
        &self,
        session_id: &SessionId,
        deadline: Instant,
    ) -> Option<(AgentIdentity, ShutdownLeaseOutcome)> {
        let member = self.members.get(session_id)?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        let outcome =
            match tokio::time::timeout(remaining, self.release_once_unhosted(session_id, member))
                .await
            {
                Ok(outcome) => outcome?,
                Err(_) => ShutdownLeaseOutcome::Kept("deadline"),
            };
        Some((member.clone(), outcome))
    }

    async fn release_once_unhosted(
        &self,
        session_id: &SessionId,
        member: &AgentIdentity,
    ) -> Option<ShutdownLeaseOutcome> {
        let active = match self
            .store
            .observe_identity_lease(&self.mob_id, member)
            .await
        {
            Ok(crate::identity::IdentityStoredObservation::Valid(record)) => record.active,
            Ok(_) => None,
            Err(error) => {
                tracing::warn!(
                    mob_id = %self.mob_id,
                    agent_identity = %member,
                    error = %error,
                    "shutdown could not read the member's identity lease; it expires on its own"
                );
                return Some(ShutdownLeaseOutcome::Kept("store_error"));
            }
        };
        let claim = active.filter(|claim| {
            claim.holder_id == self.holder_id && claim.incarnation_id == self.incarnation_id
        })?;
        // The session actor holds the session's hosting claim until it exits.
        self.session_service
            .await_removed_actor_exit(session_id)
            .await;
        if self.hosting.capability().is_cross_process()
            && self.hosting.serving(session_id)
                != meerkat_runtime::SessionServing::NotHeldInThisProcess
        {
            return Some(ShutdownLeaseOutcome::Kept("still_hosted"));
        }
        Some(
            match self
                .store
                .release_identity_lease(&self.mob_id, member, &claim)
                .await
            {
                Ok(true) => ShutdownLeaseOutcome::Released,
                Ok(false) => ShutdownLeaseOutcome::Kept("changed"),
                Err(error) => {
                    tracing::warn!(
                        mob_id = %self.mob_id,
                        agent_identity = %member,
                        error = %error,
                        "shutdown could not release the member's identity lease; it expires on its own"
                    );
                    ShutdownLeaseOutcome::Kept("store_error")
                }
            },
        )
    }

    /// One line per Shutdown: whose leases were released, and whose were kept
    /// and why, so an operator can tell why a successor waited for one.
    fn log_summary(&self, leases: &[(AgentIdentity, ShutdownLeaseOutcome)]) {
        if leases.is_empty() {
            return;
        }
        let mut released = Vec::new();
        let mut kept = Vec::new();
        for (member, outcome) in leases {
            match outcome {
                ShutdownLeaseOutcome::Released => released.push(member.to_string()),
                ShutdownLeaseOutcome::Kept(reason) => kept.push(format!("{member} ({reason})")),
            }
        }
        tracing::info!(
            mob_id = %self.mob_id,
            ?released,
            ?kept,
            "shutdown identity lease release"
        );
    }
}

/// Unregister one session through its exact-registration runtime authority,
/// within `deadline`, with `queued_inputs` deciding the session's queued
/// inputs when this call begins the unregister (a settlement that begins it
/// later carries the same choice). Never forces an unregister the runtime
/// refuses or has not completed: that is reported `Pending` with the stage
/// it stands at, and with a settlement that follows the exact registration
/// afterwards. A
/// durability-degraded registration goes through the runtime's
/// reload-required discard first, as retire and destroy do, and the cold
/// successor that discard returns as published is the registration
/// unregistered. Shutdown owns the registration until it returns, so its own
/// discard and unregister are not gated on the registration's use.
#[cfg(feature = "runtime-adapter")]
pub(in crate::runtime) async fn teardown_session(
    adapter: &Arc<meerkat_runtime::MeerkatMachine>,
    ops_adapter: &Arc<super::super::ops_adapter::MobOpsAdapter>,
    session_id: &SessionId,
    retained: Option<meerkat_runtime::RuntimeSessionUnregisterObserver>,
    deadline: Instant,
    queued_inputs: meerkat_runtime::UnregisterQueuedInputs,
) -> ShutdownUnregisterOutcome {
    use super::super::provisioner::{DegradedUnregisterStep, MemberSessionDisposalArc};

    let remaining = || deadline.saturating_duration_since(Instant::now());
    let pending =
        |stage: &'static str,
         registration: meerkat_runtime::RuntimeSessionRegistrationWitness,
         published_use: Option<meerkat_runtime::RuntimeSessionRegistrationUse>,
         observer: Option<meerkat_runtime::RuntimeSessionUnregisterObserver>| {
            pending_unregister(
                adapter,
                ops_adapter,
                stage,
                registration,
                published_use,
                observer,
                queued_inputs,
            )
        };
    // The use of a successor the teardown's own discard published, as the
    // discard returned it.
    let mut published_use = None;
    let mut observer = match retained {
        Some(observer) => observer,
        None => {
            let Some(registration) = adapter
                .current_session_registration_witness(session_id)
                .await
            else {
                return ShutdownUnregisterOutcome::Unregistered;
            };
            // A degraded registration's exited runtime loop can hold the
            // session's mutation fence, which only the reload-required
            // discard releases: the exact unregister below would wait on it.
            let registration = match MemberSessionDisposalArc::discard_degraded_registration_first(
                adapter,
                Some(ops_adapter),
                &registration,
                None,
                deadline,
            )
            .await
            {
                DegradedUnregisterStep::Unchanged => registration,
                DegradedUnregisterStep::Replaced {
                    successor,
                    successor_use,
                } => {
                    published_use = Some(successor_use);
                    successor
                }
                DegradedUnregisterStep::TimedOut => {
                    return pending("durability_reload_discard", registration, None, None).await;
                }
                DegradedUnregisterStep::HandedOver => {
                    return ShutdownUnregisterOutcome::Failed(format!(
                        "runtime session {session_id} was handed to another user during mob teardown"
                    ));
                }
                DegradedUnregisterStep::Failed(failure) => {
                    return ShutdownUnregisterOutcome::Failed(format!(
                        "failed to unregister runtime session {session_id} during mob teardown: {failure}"
                    ));
                }
            };
            // Exact-current unregister runs the two-phase drain internally;
            // the witness keeps a same-SessionId replacement out of reach.
            // Admission takes the session's registration transaction, so it
            // is bounded too: a held transaction is reported, not waited on.
            let admitted = crate::tokio::time::timeout(
                remaining(),
                adapter.observe_unregister_session_registration_with_queued_inputs_if_current(
                    &registration,
                    queued_inputs,
                ),
            )
            .await;
            match admitted {
                Err(_) => {
                    return pending(
                        "registration_transaction_admission",
                        registration,
                        published_use,
                        None,
                    )
                    .await;
                }
                // Refused until the registration's durability is repaired:
                // the settlement runs the discard and admits it again.
                Ok(Err(error)) if crate::runtime::unregister_refused_until_repair(&error) => {
                    return pending(
                        "durability_reload_discard",
                        registration,
                        published_use,
                        None,
                    )
                    .await;
                }
                Ok(Err(error)) => {
                    return ShutdownUnregisterOutcome::Failed(format!(
                        "failed to unregister runtime session {session_id} during mob teardown: {error}"
                    ));
                }
                Ok(Ok(meerkat_runtime::RuntimeSessionUnregisterAdmission::Completed)) => {
                    return verify_unregistered(adapter, session_id, &registration).await;
                }
                Ok(Ok(meerkat_runtime::RuntimeSessionUnregisterAdmission::NotCurrent)) => {
                    return if adapter
                        .current_session_registration_witness(session_id)
                        .await
                        .is_some()
                    {
                        ShutdownUnregisterOutcome::Failed(format!(
                            "runtime session {session_id} changed before exact mob shutdown teardown admission"
                        ))
                    } else {
                        ShutdownUnregisterOutcome::Unregistered
                    };
                }
                Ok(Ok(meerkat_runtime::RuntimeSessionUnregisterAdmission::Pending(observer))) => {
                    observer
                }
            }
        }
    };
    let registration = observer.registration().clone();
    let waited = crate::tokio::time::timeout(remaining(), observer.wait_for_result()).await;
    match waited {
        Err(_) => {
            pending(
                "shutdown_runtime_unregister",
                registration,
                published_use,
                Some(observer),
            )
            .await
        }
        // The coordinator ended without unregistering and released the
        // registration for the discard: the settlement runs it and admits the
        // unregister again.
        Ok(Err(error)) if crate::runtime::unregister_refused_until_repair(&error) => {
            pending(
                "durability_reload_discard",
                registration,
                published_use,
                None,
            )
            .await
        }
        Ok(Err(error)) => ShutdownUnregisterOutcome::Failed(format!(
            "failed to unregister runtime session {session_id} during mob teardown: {error}"
        )),
        Ok(Ok(())) => verify_unregistered(adapter, session_id, &registration).await,
    }
}

/// A `Pending` outcome whose settlement follows `registration` after
/// Shutdown returns. The settlement admits the unregister, or runs the
/// reload-required discard, only while the registration's use is still the
/// one it has here, as Shutdown hands it back: `published_use` for a
/// successor the teardown's own discard published, otherwise read now. An
/// unregister the settlement begins carries the Shutdown's `queued_inputs`.
#[cfg(feature = "runtime-adapter")]
async fn pending_unregister(
    adapter: &Arc<meerkat_runtime::MeerkatMachine>,
    ops_adapter: &Arc<super::super::ops_adapter::MobOpsAdapter>,
    stage: &'static str,
    registration: meerkat_runtime::RuntimeSessionRegistrationWitness,
    published_use: Option<meerkat_runtime::RuntimeSessionRegistrationUse>,
    observer: Option<meerkat_runtime::RuntimeSessionUnregisterObserver>,
    queued_inputs: meerkat_runtime::UnregisterQueuedInputs,
) -> ShutdownUnregisterOutcome {
    let registration_use = match published_use {
        Some(published) => Some(published),
        None => adapter.session_registration_use(&registration).await,
    };
    ShutdownUnregisterOutcome::Pending {
        stage,
        settlement: crate::runtime::MemberUnregisterSettlement::new(
            registration,
            registration_use,
            observer.clone(),
            adapter,
            Some(ops_adapter),
        )
        .with_queued_inputs(queued_inputs),
        observer,
    }
}

#[cfg(feature = "runtime-adapter")]
async fn verify_unregistered(
    adapter: &Arc<meerkat_runtime::MeerkatMachine>,
    session_id: &SessionId,
    registration: &meerkat_runtime::RuntimeSessionRegistrationWitness,
) -> ShutdownUnregisterOutcome {
    match adapter
        .current_session_registration_witness(session_id)
        .await
    {
        None => ShutdownUnregisterOutcome::Unregistered,
        Some(current) if &current == registration => ShutdownUnregisterOutcome::Failed(format!(
            "runtime unregister coordinator for session {session_id} completed while its exact registration remained current"
        )),
        Some(_) => ShutdownUnregisterOutcome::Failed(format!(
            "runtime session {session_id} was replaced during exact mob shutdown teardown"
        )),
    }
}
