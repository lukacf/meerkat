//! Recall of one selected queued input
//! ([`MeerkatMachine::recall_queued_input`]).

use super::*;
use crate::input_state::{InputAbandonReason, StoredInputState};
use crate::queued_input_recall::QueuedInputRecall;
use meerkat_core::time_compat::Instant;

/// The reason recorded with the runtime termination of a recalled input.
const RECALL_REASON: &str = "input recalled before it started";

/// How long a committed recall waits for its terminal's publication and the
/// resolution of the input's completion waiter. The wait starts when the
/// abandon has committed: the time the call spent waiting for the session's
/// gate and its commit is not taken from it.
const RECALL_PUBLICATION_SETTLE: std::time::Duration = std::time::Duration::from_secs(5);

/// A registration replaced between the lookup and the gate is looked up
/// again this many times before the call reports the replacement.
const RECALL_REGISTRATION_PASSES: u32 = 3;

impl MeerkatMachine {
    /// Withdraw the input `input_id` of `session_id`, only if no run has
    /// taken it yet.
    ///
    /// The recall is session-level: it withdraws any queued input of the
    /// session, whoever submitted it, with everything the queue coalesced
    /// into it (an earlier input a later one took in is already
    /// `Coalesced`; recalling the later one withdraws both contents). The
    /// answer names only the recalled input.
    ///
    /// The machine decides (`AbandonQueuedInput`), under the session's
    /// mutation gate and driver, in one transition guarded on the input
    /// being `Queued`: the fact a run reads when it takes the input
    /// (`StageForRun`, the live-boundary join). So exactly one of "a run
    /// takes the input" and "the recall abandons it" commits, and concurrent
    /// recalls abandon it once.
    ///
    /// - [`QueuedInputRecall::Abandoned`]: the input was queued and is now
    ///   `Abandoned { reason: Cancelled }`, durably on a persistent machine;
    ///   its completion waiter is resolved and no run will take it.
    /// - [`QueuedInputRecall::NotQueued`]: a run holds the input. Nothing is
    ///   touched and the run is never interrupted; stop that run with
    ///   [`Self::stop_run`] if that is what the caller wants.
    /// - [`QueuedInputRecall::AlreadyTerminal`]: the input already ended.
    ///   Nothing is touched. A second recall of a recalled input answers
    ///   this, read from the live ledger or, once the row is archived and
    ///   after a restart, from the store.
    /// - `Ok(None)`: the session holds no such input (an input of another
    ///   session included), as the per-input reads answer.
    ///
    /// The call never loads, wakes or attaches a session, and it never
    /// waits for a run: it returns once the machine has answered and, for an
    /// abandoned input, its terminal is committed and published.
    ///
    /// # Errors
    /// For a session with no registration in this process: the durable row
    /// decides when it is terminal (`AlreadyTerminal`) or absent (`None`);
    /// a row that is not terminal is refused
    /// [`RuntimeDriverError::NotReady`] with
    /// [`crate::RuntimeState::Destroyed`], since the input's phase lives in a
    /// machine that is not loaded (recall it once the session serves again).
    /// A machine without a store refuses every such session that way, and a
    /// session never admitted to a persistent machine is
    /// [`RuntimeDriverError::NotFound`]. Otherwise: the refusal of a
    /// registration that needs a durability reload or whose unregister is
    /// owed or running, and the commit's own errors (a recall whose commit
    /// landed and whose publication then failed answers that error; the
    /// input is withdrawn, and the next recall answers `AlreadyTerminal`).
    pub async fn recall_queued_input(
        &self,
        session_id: &SessionId,
        input_id: &InputId,
    ) -> Result<Option<QueuedInputRecall>, RuntimeDriverError> {
        // Process-owned, like the exact-input cancellation: a caller that
        // drops this future after the commit cannot strand the publication
        // of the input's terminal.
        let machine = self.clone();
        let session_id = session_id.clone();
        let input_id = input_id.clone();
        let cleanup_spawner = MachineCleanupTaskSpawner::acquire()?;
        let completion = cleanup_spawner.spawn(async move {
            machine
                .recall_queued_input_owned(&session_id, &input_id)
                .await
        });
        completion.await.map_err(|error| {
            RuntimeDriverError::Internal(format!(
                "owned queued-input recall ended without a result: {error}"
            ))
        })?
    }

    async fn recall_queued_input_owned(
        &self,
        session_id: &SessionId,
        input_id: &InputId,
    ) -> Result<Option<QueuedInputRecall>, RuntimeDriverError> {
        enum Step {
            Answer(QueuedInputRecall),
            Publish {
                attempt_count: u32,
                candidate_owner_input_id: Option<InputId>,
            },
            Durable,
        }

        let mut pass: u32 = 0;
        loop {
            pass += 1;
            let driver = {
                let sessions = self.sessions.read().await;
                sessions.get(session_id).map(|entry| entry.driver.clone())
            };
            let Some(driver) = driver else {
                // No registration in this process: the durable row decides
                // what it can, and nothing is loaded for the rest.
                let stored = self
                    .durable_session_input_witness_by_id(session_id, input_id)
                    .await?;
                return Self::recall_answer_from_durable_row(input_id, stored, true);
            };
            // Bound before the driver gate so that it drops after it: a
            // committed recall is told once M is released, even if a later
            // step fails.
            let mut runless_terminal = self.runless_terminal_announcement();
            let authority = match self
                .lock_current_session_driver_gate(session_id, &driver)
                .await
            {
                Ok(authority) => authority,
                Err(
                    error @ (RuntimeDriverError::NotReady {
                        state: RuntimeState::Destroyed,
                    }
                    | RuntimeDriverError::Destroyed),
                ) => {
                    // The registration ended or was replaced while this call
                    // waited for its gate: resolve the session again.
                    if pass < RECALL_REGISTRATION_PASSES {
                        continue;
                    }
                    return Err(error);
                }
                Err(error) => return Err(error),
            };
            if let Some(refusal) = self
                .lifecycle_mutation_refused_by_unregister(session_id)
                .await
            {
                return Err(refusal);
            }
            let current = {
                let sessions = self.sessions.read().await;
                sessions
                    .get(session_id)
                    .filter(|entry| Arc::ptr_eq(&entry.driver, &driver))
                    .map(|entry| {
                        (
                            entry.completions.clone(),
                            entry.publication_handle(),
                            entry.mutation_gate.clone(),
                        )
                    })
            };
            let Some((completions, publication_handle, mutation_gate)) = current else {
                // Replaced between the gate and this read: resolve again.
                if pass < RECALL_REGISTRATION_PASSES {
                    continue;
                }
                return Err(RuntimeDriverError::NotReady {
                    state: RuntimeState::Destroyed,
                });
            };
            let step = {
                let mut driver_guard = driver.lock().await;
                // The machine answers first, on a fork of its authority, so
                // the input's terminal receipt is staged only for an input
                // the machine will abandon. The driver stays locked from
                // this answer to the apply below, which checks the same
                // guard again.
                match driver_guard
                    .preview_queued_input_abandon(input_id, InputAbandonReason::Cancelled)?
                {
                    driver::QueuedInputAbandon::Abandoned { attempt_count } => {
                        let prepared = driver_guard.prepare_sole_runless_runtime_termination(
                            input_id,
                            RECALL_REASON.to_string(),
                        )?;
                        match driver_guard
                            .abandon_queued_input(input_id, InputAbandonReason::Cancelled)
                            .await
                        {
                            Ok(true) => {}
                            Ok(false) => {
                                driver_guard.rollback_prepared_sole_runless_termination(prepared);
                                return Err(RuntimeDriverError::Internal(format!(
                                    "the machine refused to abandon queued input {input_id} \
                                     that it had just answered as queued"
                                )));
                            }
                            Err(error) => {
                                driver_guard.rollback_prepared_sole_runless_termination(prepared);
                                return Err(error);
                            }
                        }
                        runless_terminal.owe();
                        let candidate_owner_input_id =
                            driver::DriverEntry::commit_prepared_sole_runless_termination(prepared);
                        // A durable steer whose live-boundary work still waits
                        // for its boundary drops its preparation, so the next
                        // durable steer can take that boundary.
                        driver_guard
                            .driver_ingress()
                            .durable_steer_order()
                            .withdraw(input_id);
                        Step::Publish {
                            attempt_count,
                            candidate_owner_input_id,
                        }
                    }
                    driver::QueuedInputAbandon::NotQueued { phase } => {
                        Step::Answer(QueuedInputRecall::NotQueued {
                            input_id: input_id.clone(),
                            phase,
                            run_id: driver_guard.input_last_run_id(input_id),
                        })
                    }
                    driver::QueuedInputAbandon::AlreadyTerminal { .. } => {
                        let terminal = driver_guard
                            .as_driver()
                            .stored_input_state(input_id)
                            .and_then(|stored| stored.seed.terminal_outcome)
                            .ok_or_else(|| {
                                RuntimeDriverError::Internal(format!(
                                    "terminal input {input_id} has no terminal outcome in the \
                                     live ledger"
                                ))
                            })?;
                        Step::Answer(QueuedInputRecall::AlreadyTerminal {
                            input_id: input_id.clone(),
                            terminal,
                        })
                    }
                    driver::QueuedInputAbandon::Untracked => Step::Durable,
                }
            };
            match step {
                Step::Answer(answer) => return Ok(Some(answer)),
                Step::Durable => {
                    // Terminal rows leave the live machine once their durable
                    // obligations close; an attached session reads them from
                    // its store.
                    drop(authority);
                    let stored = self
                        .durable_input_witness_by_id(session_id, input_id)
                        .await?;
                    return Self::recall_answer_from_durable_row(input_id, stored, false);
                }
                Step::Publish {
                    attempt_count,
                    candidate_owner_input_id,
                } => {
                    // The abandon is committed. Its publication gets the
                    // whole settle bound from here, however long this call
                    // waited for the gate before.
                    let deadline = Instant::now() + RECALL_PUBLICATION_SETTLE;
                    let dispatch = match (
                        candidate_owner_input_id.as_ref(),
                        publication_handle.clone(),
                    ) {
                        (Some(_), Some(publication_handle)) => {
                            Some(self.prepare_runless_terminal_publication_dispatch(
                                &driver,
                                &completions,
                                &mutation_gate,
                                publication_handle,
                            )?)
                        }
                        _ => None,
                    };
                    drop(authority);
                    // The abandoned input's terminal is durable and readable:
                    // tell the host before publication, whose failure leaves
                    // it terminal all the same.
                    runless_terminal.release();
                    let publication = if let Some((result_rx, start_tx)) = dispatch {
                        if let Some(start_tx) = start_tx {
                            let _ = start_tx.send(());
                        }
                        self.await_runless_terminal_publication_dispatch(
                            &LogicalRuntimeId::for_session(session_id),
                            result_rx,
                            Some(deadline),
                        )
                        .await
                    } else {
                        crate::control_plane::publish_and_resolve_runless_runtime_termination_before(
                            &driver,
                            Some(&completions),
                            None,
                            std::slice::from_ref(input_id),
                            candidate_owner_input_id.as_ref(),
                            RECALL_REASON,
                            Some(deadline),
                        )
                        .await
                    };
                    if let Err(error) = publication {
                        if candidate_owner_input_id.is_none() {
                            // Nondirected completion has no durable outbox to
                            // authorize replay: its hand-off completed or
                            // returned the exact error.
                            return Err(error);
                        }
                        tracing::warn!(
                            %session_id,
                            %input_id,
                            error = %error,
                            "queued-input recall committed its terminal carrier; retrying canonical publication recovery"
                        );
                        crate::control_plane::converge_known_committed_runless_runtime_terminations_before(
                            &driver,
                            Some(&completions),
                            publication_handle.as_deref(),
                            Some(deadline),
                        )
                        .await?;
                    }
                    return Ok(Some(QueuedInputRecall::Abandoned {
                        input_id: input_id.clone(),
                        attempt_count,
                    }));
                }
            }
        }
    }

    /// The answer the durable row of an input gives when the live machine
    /// does not hold the input: a terminal row is `AlreadyTerminal`, no row
    /// is the unknown input. A row that is not terminal cannot be answered
    /// from the store: its phase is a machine's. With no registration
    /// (`unregistered`) that machine is not loaded, so the recall is refused
    /// until the session serves again; under a live registration it means
    /// another owner wrote the row, and the caller's view is stale.
    fn recall_answer_from_durable_row(
        input_id: &InputId,
        stored: Option<StoredInputState>,
        unregistered: bool,
    ) -> Result<Option<QueuedInputRecall>, RuntimeDriverError> {
        let Some(stored) = stored else {
            return Ok(None);
        };
        match stored.seed.terminal_outcome {
            Some(terminal) => Ok(Some(QueuedInputRecall::AlreadyTerminal {
                input_id: input_id.clone(),
                terminal,
            })),
            None if unregistered => Err(RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            }),
            None => Err(RuntimeDriverError::StaleAuthority {
                reason: format!(
                    "input {input_id} is not terminal in the store and is not held by the \
                     session's live runtime"
                ),
            }),
        }
    }
}
