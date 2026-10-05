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
    /// and its observer is retained.
    Pending {
        stage: &'static str,
        #[cfg(feature = "runtime-adapter")]
        observer: Option<meerkat_runtime::RuntimeSessionUnregisterObserver>,
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
            let (members, targets) = match self.shutdown_teardown_targets() {
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
            self.actor_io_tasks.spawn(async move {
                let outcomes = futures::future::join_all(targets.into_iter().map(
                    |(session_id, retained)| {
                        let adapter = Arc::clone(&adapter);
                        async move {
                            let outcome =
                                teardown_session(&adapter, &session_id, retained, deadline).await;
                            (session_id, outcome)
                        }
                    },
                ))
                .await;
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
    /// any unregister a previous attempt left with its coordinator.
    #[cfg(feature = "runtime-adapter")]
    #[allow(clippy::type_complexity)]
    fn shutdown_teardown_targets(
        &mut self,
    ) -> Result<
        (
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
        Ok((members, targets))
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
                ShutdownUnregisterOutcome::Pending {
                    stage,
                    #[cfg(feature = "runtime-adapter")]
                    observer,
                } => {
                    self.record_shutdown_unregister_pending(member, stage);
                    #[cfg(feature = "runtime-adapter")]
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

/// Unregister one session through its exact-registration runtime authority,
/// within `deadline`. Never forces an unregister the runtime refuses or has
/// not completed: that is reported `Pending` with the stage it stands at.
#[cfg(feature = "runtime-adapter")]
async fn teardown_session(
    adapter: &Arc<meerkat_runtime::MeerkatMachine>,
    session_id: &SessionId,
    retained: Option<meerkat_runtime::RuntimeSessionUnregisterObserver>,
    deadline: Instant,
) -> ShutdownUnregisterOutcome {
    let remaining = || deadline.saturating_duration_since(Instant::now());
    let mut observer = match retained {
        Some(observer) => observer,
        None => {
            let Some(registration) = adapter
                .current_session_registration_witness(session_id)
                .await
            else {
                return ShutdownUnregisterOutcome::Unregistered;
            };
            // Exact-current unregister runs the two-phase drain internally;
            // the witness keeps a same-SessionId replacement out of reach.
            // Admission takes the session's registration transaction, so it
            // is bounded too: a held transaction is reported, not waited on.
            let admitted = crate::tokio::time::timeout(
                remaining(),
                adapter.observe_unregister_session_registration_if_current(&registration),
            )
            .await;
            match admitted {
                Err(_) => {
                    return ShutdownUnregisterOutcome::Pending {
                        stage: "registration_transaction_admission",
                        observer: None,
                    };
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
        Err(_) => ShutdownUnregisterOutcome::Pending {
            stage: "shutdown_runtime_unregister",
            observer: Some(observer),
        },
        Ok(Err(error)) => ShutdownUnregisterOutcome::Failed(format!(
            "failed to unregister runtime session {session_id} during mob teardown: {error}"
        )),
        Ok(Ok(())) => verify_unregistered(adapter, session_id, &registration).await,
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
