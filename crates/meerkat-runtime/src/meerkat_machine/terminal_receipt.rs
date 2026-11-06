//! Public terminal-receipt read and wait for one admitted input.
//!
//! Both methods read only what the runtime already owns: the machine-owned
//! input lifecycle and the durable terminal-completion batch that the run (or
//! runtime termination) committed for the input - the same rows the exact
//! completion reader projects. Nothing here decides or records a lifecycle
//! fact.
//!
//! The completion registry is used purely as a wake signal. A completion
//! waiter and a receipt-less terminal observer are registered under the driver
//! lock after a `Pending` read; every terminal transition needs that same
//! lock, and every waker delivers only after the transition committed, so a
//! wake cannot be lost between the read and the registration. Whatever a wake
//! carries (an outcome, a process-local attempt failure, an error, a closed
//! channel, a bare observer wake) is ignored: the receipt is re-read after
//! every wake, so a requeued input simply re-arms.

use super::*;
use crate::input_state::StoredInputState;
use crate::terminal_status::{
    InputTerminalReceiptObservation, InputTerminalReceiptRead, InputTerminalReceiptWait,
    InteractionSelector, Sourced, TerminalWitnessSource,
};

/// How a receipt read leaves the machine: through the payload reader's
/// scoped disclosure decision, or (for the body-free observation) to the
/// projection that drops its outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReceiptRelease {
    PayloadReader,
    Observation,
}

impl MeerkatMachine {
    /// Observe this session's input-state witnesses through its actual runtime
    /// owner. Registered sessions combine the live snapshot with durable rows
    /// no longer held in memory; unregistered sessions read the durable store
    /// without restoring a registration. Every durable row must decode, or the
    /// whole observation fails. No input is filtered by phase, run or workflow.
    ///
    /// This read does not close ingress or prove that work has settled. A
    /// caller requiring a complete closed cohort must separately establish
    /// that the selected registration is Draining or its teardown has joined,
    /// then check each input's canonical terminal receipt. Reused sessions
    /// also require the caller's exact start baseline to scope its workflow.
    /// Input ids are stable across the boundary: a requeue across a graceful
    /// stop, a replay of a failed run and a retry keep an input's id. A
    /// requeued input, or one bound to a run a graceful stop suspended, is not
    /// finalized at shutdown: it finalizes at the next start, or as
    /// `RequeueExpired` past the stop's max age. A coalesced or superseded
    /// input names the newer admitted input of the same session that carries
    /// its outcome.
    /// The returned source identifies the read path, not a close receipt.
    ///
    /// A known empty session returns an empty observation. An unregistered
    /// session on a store-less machine fails `NotReady`; a persistent session
    /// with neither lifecycle nor input records fails `NotFound`.
    ///
    /// Each row is released as the public row read releases it: a result
    /// payload (a run result or callback arguments) only to a reader the
    /// input's scoped destination permits, so once a scoped session is
    /// unregistered none comes out; every body-free lifecycle fact stays.
    pub async fn session_input_states(
        &self,
        session_id: &SessionId,
    ) -> Result<Sourced<Vec<StoredInputState>>, RuntimeDriverError> {
        let (source, rows) = self.session_input_witnesses(session_id).await?;
        let report = self.release_public_rows(session_id, rows).await;
        Ok(Sourced { source, report })
    }

    /// Wait until `session_id`'s live runtime has admitted an input for
    /// `idempotency_key` and return its id. Woken by the admission itself
    /// (the driver signals every accepted input); never re-reads on a timer.
    ///
    /// `Ok(None)` when the session has no live registration, now or once it
    /// is torn down while waiting: the caller reads durable evidence instead.
    /// Bound the call by the caller's own deadline.
    pub async fn wait_input_admitted_by_idempotency_key(
        &self,
        session_id: &SessionId,
        idempotency_key: &str,
    ) -> Result<Option<InputId>, RuntimeDriverError> {
        loop {
            let driver = {
                let sessions = self.sessions.read().await;
                sessions.get(session_id).map(|entry| entry.driver.clone())
            };
            let Some(driver) = driver else {
                return Ok(None);
            };
            let mut admissions = {
                let guard = driver.lock().await;
                if let Some(input_id) = guard
                    .as_driver()
                    .input_id_for_idempotency_key(idempotency_key)
                {
                    return Ok(Some(input_id));
                }
                // Subscribed under the driver: an admission after the check
                // above is a change on this receiver.
                guard.subscribe_admissions()
            };
            // Hold no session resources while parked, so teardown can drop
            // the driver and close the signal.
            drop(driver);
            if admissions.changed().await.is_err() {
                // The driver is gone; re-resolve the registration.
                continue;
            }
        }
    }

    /// Read the terminal receipt of one input: its finalized receipt (the
    /// scope, recipients and finalized outcome of the terminal batch the
    /// runtime committed it in), a receipt-less terminal, or its current
    /// pending lifecycle facts.
    ///
    /// A receipt proves which inputs were finalized together with one
    /// outcome; it does not prove that a run's result answers every input
    /// that run touched. See
    /// [`crate::terminal_status::InputTerminalReceiptScope::Run`].
    ///
    /// Selector resolution matches [`SessionServiceRuntimeExt::interaction_terminal_status`]:
    /// a registered session resolves keys through the machine-owned admission
    /// map and falls back to the durable store for rows archived out of
    /// memory; an unregistered session answers from the durable store without
    /// reviving the runtime. `Ok(None)` means the session exists but holds no
    /// such input (or no such key yet). An unregistered session on a store-less
    /// machine fails `NotReady`, and a never-admitted session on a persistent
    /// machine fails `NotFound`.
    ///
    /// A finalized receipt carries the batch's outcome, so a result payload
    /// is released only to a reader the input's scoped destination permits:
    /// otherwise the read fails `ScopedResultWithheld`. A reader without the
    /// current destination (for example after the session's unregister)
    /// proves the input's completion with
    /// [`Self::input_terminal_receipt_observation`] instead.
    pub async fn input_terminal_receipt(
        &self,
        session_id: &SessionId,
        selector: InteractionSelector,
    ) -> Result<Option<Sourced<InputTerminalReceiptRead>>, RuntimeDriverError> {
        crate::stack_relief::box_in_own_frame(|| {
            self.read_input_terminal_receipt(session_id, selector, ReceiptRelease::PayloadReader)
        })
        .await
    }

    /// Observe the terminal receipt of one input without its outcome: the
    /// same resolution, batch load and native batch validation as
    /// [`Self::input_terminal_receipt`], with a finalized receipt's outcome
    /// replaced by its body-free class
    /// ([`crate::terminal_status::InputTerminalReceiptSummary`]).
    ///
    /// It carries every identity of the receipt (the input, its terminal and
    /// attempts, the batch's scope, owner and recipients) and no run result,
    /// callback call or failure text, so it answers whatever the reader's
    /// destination: it never fails `ScopedResultWithheld`, and the payload
    /// reader keeps withholding. A lost owner, recipient or recipient set
    /// fails `RecoveryCorruption`, a store that cannot load the batch fails
    /// `RecoveryRepairBlocked`, and selector resolution, `Ok(None)`,
    /// `NotReady` and `NotFound` are as for the receipt. It is a read, not a
    /// close receipt: a caller proving a closed cohort still establishes
    /// that the registration is Draining or its teardown has joined.
    pub async fn input_terminal_receipt_observation(
        &self,
        session_id: &SessionId,
        selector: InteractionSelector,
    ) -> Result<Option<Sourced<InputTerminalReceiptObservation>>, RuntimeDriverError> {
        let read = crate::stack_relief::box_in_own_frame(|| {
            self.read_input_terminal_receipt(session_id, selector, ReceiptRelease::Observation)
        })
        .await?;
        Ok(read.map(|read| Sourced {
            source: read.source,
            report: crate::terminal_status::observe_input_terminal_receipt(read.report),
        }))
    }

    /// Observe the governed submission made under the submitter's raw key
    /// `raw_key` and authority `association` on `session_id`, by that key:
    /// Core qualifies the key under the association exactly as governed
    /// admission did, and reads the input admitted under it, live or from
    /// the store, as [`Self::input_terminal_receipt_observation`] reads it.
    ///
    /// The key, not an input id, names the submission: governed admission
    /// deduplicates a repeated submission onto the input first admitted under
    /// the key before writing any row, so the input found here is the
    /// canonical one and may differ from the one a caller submitted (a read by
    /// that id misses).
    ///
    /// The association is the caller's own; a governed submission's receipt
    /// names it by its digest
    /// ([`meerkat_authorization_contracts::work_association::InputAuthorityAssociation::digest`]:
    /// SHA-256 over its canonical V1 bytes, text `v1:<hex>`), which a caller
    /// checks before it reads, refusing an unknown version as unresolved.
    ///
    /// `Ok(None)` is a successful read that found no input under the key on
    /// this runtime's store. It is absence only for this read: whether it is
    /// final depends on the caller's own proof (the same store, its old
    /// writers fenced, no queued owner that may submit again). A missing
    /// session, a store not ready, a corrupt or repair-blocked row fail
    /// typed, as for [`Self::input_terminal_receipt_observation`]; none of
    /// them is absence.
    pub async fn governed_input_by_key(
        &self,
        session_id: &SessionId,
        raw_key: &str,
        association: &meerkat_authorization_contracts::work_association::InputAuthorityAssociation,
    ) -> Result<Option<Sourced<InputTerminalReceiptObservation>>, RuntimeDriverError> {
        let qualified = crate::input_authority::qualify_idempotency_key(association, raw_key)?;
        // RED skeleton: the read does not run yet.
        let _ = (session_id, qualified);
        Ok(None)
    }

    /// The shared receipt read of [`Self::input_terminal_receipt`] and
    /// [`Self::input_terminal_receipt_observation`]: resolution, batch load
    /// and validation, then `release`.
    async fn read_input_terminal_receipt(
        &self,
        session_id: &SessionId,
        selector: InteractionSelector,
        release: ReceiptRelease,
    ) -> Result<Option<Sourced<InputTerminalReceiptRead>>, RuntimeDriverError> {
        let driver = {
            let sessions = self.sessions.read().await;
            sessions.get(session_id).map(|entry| entry.driver.clone())
        };
        // Every store read below is built in its own frame: at opt-level 0
        // each awaited future reserved twice its size in this poll frame,
        // beneath every caller that waits on a delivery's terminal.
        let Some(driver) = driver else {
            let target = match &selector {
                InteractionSelector::InputId(input_id) => {
                    crate::stack_relief::box_in_own_frame(|| {
                        self.durable_session_input_witness_by_id(session_id, input_id)
                    })
                    .await?
                }
                InteractionSelector::IdempotencyKey(key) => {
                    crate::stack_relief::box_in_own_frame(|| {
                        self.durable_session_input_witness_by_idempotency_key(session_id, key)
                    })
                    .await?
                }
            };
            return crate::stack_relief::box_in_own_frame(|| {
                self.durable_input_terminal_receipt_read(session_id, target, release)
            })
            .await;
        };
        let (resolved_input_id, live) = {
            let guard = driver.lock().await;
            let input_id = match &selector {
                InteractionSelector::InputId(input_id) => Some(input_id.clone()),
                InteractionSelector::IdempotencyKey(key) => {
                    guard.as_driver().input_id_for_idempotency_key(key)
                }
            };
            let live = match &input_id {
                Some(input_id) => {
                    self.live_input_terminal_receipt_read(&guard, input_id, release)?
                }
                None => None,
            };
            (input_id, live)
        };
        if let Some(report) = live {
            return Ok(Some(Sourced {
                source: TerminalWitnessSource::LiveRuntime,
                report,
            }));
        }
        // Terminal rows leave the hot machine once their durable obligations
        // close; an attached session reads them from its store.
        let target = match (&resolved_input_id, &selector) {
            (Some(input_id), _) | (None, InteractionSelector::InputId(input_id)) => {
                crate::stack_relief::box_in_own_frame(|| {
                    self.durable_input_witness_by_id(session_id, input_id)
                })
                .await?
            }
            (None, InteractionSelector::IdempotencyKey(key)) => {
                crate::stack_relief::box_in_own_frame(|| {
                    self.durable_input_witness_by_idempotency_key_if_present(session_id, key)
                })
                .await?
            }
        };
        crate::stack_relief::box_in_own_frame(|| {
            self.durable_input_terminal_receipt_read(session_id, target, release)
        })
        .await
    }

    /// Wait until one input's terminal receipt resolves.
    ///
    /// The input is resolved exactly as [`Self::input_terminal_receipt`]
    /// resolves an [`InteractionSelector::InputId`], whether or not the
    /// session is registered: `Ok(None)` means the session exists but holds
    /// no such input, an unregistered session on a store-less machine fails
    /// `NotReady`, and a never-admitted session on a persistent machine fails
    /// `NotFound`.
    ///
    /// Returns `Resolved` with a finalized receipt or a receipt-less terminal,
    /// read live or (for rows archived after their durable obligations
    /// closed, or an unregistered session) from the durable store. An
    /// execution attempt that fails and is requeued by the machine does not
    /// resolve the wait: the runtime still owes the input a run. When the
    /// input is still pending but there is no live registration to wait on
    /// (session never registered, or unregistered while waiting) the wait
    /// returns `Detached` with the current durable read instead of polling.
    ///
    /// The wait is event-driven and unbounded; callers bound it. It wakes on
    /// the signals that already mark a pending input's terminal: a resolved
    /// completion waiter (batch finalization or runtime termination), a
    /// mechanically failed waiter (boot revival, a failed batch start), and
    /// the per-input receipt observer, woken by the admission that coalesces
    /// or supersedes the input and by a directed batch's receipt
    /// finalization. A wake carries nothing; the receipt
    /// is re-read. Dropping the future unregisters both registrations.
    ///
    /// `Resolved` means the input's receipt is finalized (or it reached a
    /// receipt-less terminal). It says nothing about interaction-terminal
    /// publication. Directed (peer-request) batches finalize their receipt
    /// before the batch's interaction terminals are published, and resolve
    /// their completion waiters only once publication succeeds. Finalizing
    /// the receipt wakes the receipt observers, so this wait resolves at
    /// finalization, exactly when [`Self::input_terminal_receipt`] starts
    /// reading the finalized receipt, even while a transient publication
    /// failure is still being retried. A caller that needs the published
    /// terminal waits on the input's completion instead.
    pub async fn wait_input_terminal_receipt(
        &self,
        session_id: &SessionId,
        input_id: &InputId,
    ) -> Result<Option<InputTerminalReceiptWait>, RuntimeDriverError> {
        loop {
            let entry = {
                let sessions = self.sessions.read().await;
                sessions
                    .get(session_id)
                    .map(|entry| (entry.driver.clone(), entry.completions.clone()))
            };
            // The store reads are built in their own frames, as in
            // `input_terminal_receipt`.
            let Some((driver, completions)) = entry else {
                let target = crate::stack_relief::box_in_own_frame(|| {
                    self.durable_session_input_witness_by_id(session_id, input_id)
                })
                .await?;
                return crate::stack_relief::box_in_own_frame(|| {
                    self.durable_input_terminal_receipt_wait(session_id, target)
                })
                .await;
            };
            let (completion, observer) = {
                let guard = driver.lock().await;
                match self.live_input_terminal_receipt_read(
                    &guard,
                    input_id,
                    ReceiptRelease::PayloadReader,
                )? {
                    Some(report) if report.is_resolved() => {
                        return Ok(Some(InputTerminalReceiptWait::Resolved(Sourced {
                            source: TerminalWitnessSource::LiveRuntime,
                            report,
                        })));
                    }
                    // Same lock order as every resolver: driver, then
                    // registry. Registering before the driver guard drops
                    // orders both registrations before any later terminal
                    // transition, so no wake can be lost.
                    Some(_) => {
                        let mut registry = completions.lock().await;
                        (
                            registry.register(input_id.clone()),
                            registry.observe_receipt_less_terminal(input_id.clone()),
                        )
                    }
                    None => {
                        drop(guard);
                        // Not held live: a row archived after its durable
                        // obligations closed, or no such input at all.
                        let target = crate::stack_relief::box_in_own_frame(|| {
                            self.durable_input_witness_by_id(session_id, input_id)
                        })
                        .await?;
                        return crate::stack_relief::box_in_own_frame(|| {
                            self.durable_input_terminal_receipt_wait(session_id, target)
                        })
                        .await;
                    }
                }
            };
            // Hold no session resources while parked, so teardown can drop
            // the registry and close both registrations.
            drop(completions);
            drop(driver);
            let completion = std::pin::pin!(completion.wait_terminal_wake());
            let observer = std::pin::pin!(observer.woken());
            // A wake carries no semantics; the loop re-reads the receipt.
            let _wake = futures::future::select(completion, observer).await;
        }
    }

    /// Settle a wait from the durable store: resolved, or `Detached` while
    /// the stored row is still pending. `None` for an input the store does
    /// not hold.
    async fn durable_input_terminal_receipt_wait(
        &self,
        session_id: &SessionId,
        target: Option<StoredInputState>,
    ) -> Result<Option<InputTerminalReceiptWait>, RuntimeDriverError> {
        Ok(self
            .durable_input_terminal_receipt_read(session_id, target, ReceiptRelease::PayloadReader)
            .await?
            .map(|read| {
                if read.report.is_resolved() {
                    InputTerminalReceiptWait::Resolved(read)
                } else {
                    InputTerminalReceiptWait::Detached(read)
                }
            }))
    }

    /// A finalized receipt carries the batch's outcome, so for the payload
    /// reader its result payload is released only to a reader the input's
    /// scoped destination permits (see
    /// [`super::scoped_context::result_disclosure_permitted`]). For the
    /// observation the read is returned unreleased, and its only caller
    /// drops the outcome before anything leaves the machine.
    fn live_input_terminal_receipt_read(
        &self,
        driver: &DriverEntry,
        input_id: &InputId,
        release: ReceiptRelease,
    ) -> Result<Option<InputTerminalReceiptRead>, RuntimeDriverError> {
        let Some(read) = driver
            .exact_input_terminal_completion_rows(input_id)?
            .map(|rows| Self::classify_input_terminal_receipt(&rows, input_id))
            .transpose()?
        else {
            return Ok(None);
        };
        if release == ReceiptRelease::Observation {
            return Ok(Some(read));
        }
        let posture =
            super::scoped_context::scoped_host_posture(&self.native_work_authorization_host);
        let permitted = match driver.as_driver().stored_input_state(input_id) {
            Some(row) => super::scoped_context::disclosure_permitted_with(
                Some(&driver.shared_dsl_authority()),
                &row.state,
                posture,
            ),
            None => posture == super::scoped_context::ScopedHostPosture::Ungoverned,
        };
        Self::withhold_undisclosed_receipt(read, permitted).map(Some)
    }

    /// Refuse a finalized receipt whose result payload this reader may not
    /// receive. The receipt, the input's terminal and its preserved result
    /// are unaffected; pending and receipt-less reads carry no payload.
    fn withhold_undisclosed_receipt(
        read: InputTerminalReceiptRead,
        permitted: bool,
    ) -> Result<InputTerminalReceiptRead, RuntimeDriverError> {
        match &read {
            InputTerminalReceiptRead::Finalized(receipt)
                if !permitted && receipt.outcome().carries_result_payload() =>
            {
                Err(RuntimeDriverError::ScopedResultWithheld {
                    input_id: receipt.input_id().clone(),
                })
            }
            _ => Ok(read),
        }
    }

    async fn durable_input_terminal_receipt_read(
        &self,
        session_id: &SessionId,
        target: Option<StoredInputState>,
        release: ReceiptRelease,
    ) -> Result<Option<Sourced<InputTerminalReceiptRead>>, RuntimeDriverError> {
        let Some(target) = target else {
            return Ok(None);
        };
        let Some(store) = self.store.as_ref() else {
            return Err(RuntimeDriverError::Internal(
                "durable input witness was read without a runtime store".to_string(),
            ));
        };
        let runtime_id = Self::logical_runtime_id(session_id);
        let input_id = target.state.input_id.clone();
        let row = target.state.clone();
        let rows = Self::load_durable_terminal_completion_rows(store.as_ref(), &runtime_id, target)
            .await?;
        let report = Self::classify_input_terminal_receipt(&rows, &input_id)?;
        if release == ReceiptRelease::Observation {
            return Ok(Some(Sourced {
                source: TerminalWitnessSource::DurableStore,
                report,
            }));
        }
        // Decided after the last awaited load, immediately before release.
        let permitted = self
            .stored_result_disclosure_permitted(session_id, &row)
            .await;
        Ok(Some(Sourced {
            source: TerminalWitnessSource::DurableStore,
            report: Self::withhold_undisclosed_receipt(report, permitted)?,
        }))
    }

    fn classify_input_terminal_receipt(
        rows: &[StoredInputState],
        input_id: &InputId,
    ) -> Result<InputTerminalReceiptRead, RuntimeDriverError> {
        crate::terminal_status::input_terminal_receipt_read(rows, input_id)
            .map_err(crate::input_state::InputTerminalCompletionReadError::into_driver_error)?
            .ok_or_else(|| RuntimeDriverError::RecoveryCorruption {
                reason: format!("terminal completion batch does not contain its target {input_id}"),
            })
    }

    /// Load the durable rows the exact completion projection needs for one
    /// stored target: the target alone when it carries no receipt, otherwise
    /// every recipient row of its batch in canonical order.
    pub(super) async fn load_durable_terminal_completion_rows(
        store: &dyn RuntimeStore,
        runtime_id: &LogicalRuntimeId,
        target: StoredInputState,
    ) -> Result<Vec<StoredInputState>, RuntimeDriverError> {
        let Some(target_completion) = target.state.terminal_completion.as_ref() else {
            return Ok(vec![target]);
        };
        let owner_input_id = target_completion.owner_input_id.clone();
        let owner = if owner_input_id == target.state.input_id {
            target
        } else {
            let mut owner_rows = store
                .load_input_states_by_ids(runtime_id, std::slice::from_ref(&owner_input_id))
                .await
                .map_err(|error| Self::terminal_completion_store_error(runtime_id, error))?;
            owner_rows
                .pop()
                .ok_or_else(|| {
                    RuntimeDriverError::Internal(
                        "exact terminal completion owner read returned the wrong cardinality"
                            .to_string(),
                    )
                })?
                .ok_or_else(|| RuntimeDriverError::RecoveryCorruption {
                    reason: "terminal completion target lost its canonical durable owner row"
                        .to_string(),
                })?
        };
        let recipient_ids = owner
            .state
            .terminal_completion
            .as_ref()
            .and_then(|completion| completion.completion_input_ids.clone())
            .ok_or_else(|| RuntimeDriverError::RecoveryCorruption {
                reason: "terminal completion durable owner lost its recipient set".to_string(),
            })?;
        let recipient_rows = store
            .load_input_states_by_ids(runtime_id, &recipient_ids)
            .await
            .map_err(|error| Self::terminal_completion_store_error(runtime_id, error))?;
        if recipient_rows.len() != recipient_ids.len() {
            return Err(RuntimeDriverError::Internal(
                "exact terminal completion batch read returned the wrong cardinality".to_string(),
            ));
        }
        recipient_rows
            .into_iter()
            .zip(recipient_ids)
            .map(|(stored, recipient_id)| {
                stored.ok_or_else(|| RuntimeDriverError::RecoveryCorruption {
                    reason: format!(
                        "terminal completion durable batch lost recipient row {recipient_id}"
                    ),
                })
            })
            .collect()
    }

    pub(super) fn terminal_completion_store_error(
        runtime_id: &LogicalRuntimeId,
        error: crate::store::RuntimeStoreError,
    ) -> RuntimeDriverError {
        match error {
            crate::store::RuntimeStoreError::Unsupported(reason) => {
                RuntimeDriverError::RecoveryRepairBlocked {
                    evidence_digest: None,
                    reason: format!(
                        "runtime store cannot load one exact terminal completion batch: {reason}"
                    ),
                }
            }
            error => RuntimeDriverError::Internal(format!(
                "exact terminal completion witness read failed for {runtime_id}: {error}"
            )),
        }
    }
}
