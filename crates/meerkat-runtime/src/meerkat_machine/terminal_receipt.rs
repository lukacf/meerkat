//! Public terminal-receipt read and wait for one admitted input.
//!
//! Both methods read only what the runtime already owns: the machine-owned
//! input lifecycle and the durable terminal-completion batch that the run (or
//! runtime termination) committed for the input - the same rows the exact
//! completion reader projects. Nothing here decides or records a lifecycle
//! fact.
//!
//! The completion registry is used purely as a wake signal. A waiter is
//! registered under the driver lock after a `Pending` read; finalization of a
//! terminal batch needs that same lock, and every resolver delivers only after
//! finalizing, so a wake cannot be lost between the read and the registration.
//! Whatever a wake carries (an outcome, a process-local attempt failure, an
//! error, a closed channel) is ignored: the receipt is re-read after every
//! wake, so a requeued input simply re-arms.

use super::*;
use crate::input_state::StoredInputState;
use crate::terminal_status::{
    InputTerminalReceiptRead, InputTerminalReceiptWait, InteractionSelector, Sourced,
    TerminalWitnessSource,
};

impl MeerkatMachine {
    /// Read the terminal receipt of one input: its finalized receipt (run
    /// scope, full recipient set and finalized outcome), a receipt-less
    /// terminal, or its current pending lifecycle facts.
    ///
    /// Selector resolution matches [`SessionServiceRuntimeExt::interaction_terminal_status`]:
    /// a registered session resolves keys through the machine-owned admission
    /// map and falls back to the durable store for rows archived out of
    /// memory; an unregistered session answers from the durable store without
    /// reviving the runtime. `Ok(None)` means the session exists but holds no
    /// such input (or no such key yet). An unregistered session on a store-less
    /// machine fails `NotReady`, and a never-admitted session on a persistent
    /// machine fails `NotFound`.
    pub async fn input_terminal_receipt(
        &self,
        session_id: &SessionId,
        selector: InteractionSelector,
    ) -> Result<Option<Sourced<InputTerminalReceiptRead>>, RuntimeDriverError> {
        let driver = {
            let sessions = self.sessions.read().await;
            sessions.get(session_id).map(|entry| entry.driver.clone())
        };
        let Some(driver) = driver else {
            let target = match &selector {
                InteractionSelector::InputId(input_id) => {
                    self.durable_session_input_witness_by_id(session_id, input_id)
                        .await?
                }
                InteractionSelector::IdempotencyKey(key) => {
                    self.durable_session_input_witness_by_idempotency_key(session_id, key)
                        .await?
                }
            };
            return self
                .durable_input_terminal_receipt_read(session_id, target)
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
                Some(input_id) => Self::live_input_terminal_receipt_read(&guard, input_id)?,
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
                self.durable_input_witness_by_id(session_id, input_id)
                    .await?
            }
            (None, InteractionSelector::IdempotencyKey(key)) => {
                self.durable_input_witness_by_idempotency_key_if_present(session_id, key)
                    .await?
            }
        };
        self.durable_input_terminal_receipt_read(session_id, target)
            .await
    }

    /// Wait until one admitted input's terminal receipt resolves.
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
    /// The wait is event-driven and unbounded; callers bound it. Machine
    /// transitions that never deliver a completion (supersession or
    /// coalescing by a later admission) are observed by the caller's next
    /// read. Dropping the future unregisters its waiter. An input id the
    /// session and its store do not know fails `ValidationFailed`.
    pub async fn wait_input_terminal_receipt(
        &self,
        session_id: &SessionId,
        input_id: &InputId,
    ) -> Result<InputTerminalReceiptWait, RuntimeDriverError> {
        loop {
            let entry = {
                let sessions = self.sessions.read().await;
                sessions
                    .get(session_id)
                    .map(|entry| (entry.driver.clone(), entry.completions.clone()))
            };
            let Some((driver, completions)) = entry else {
                let target = self
                    .durable_input_witness_by_id(session_id, input_id)
                    .await?;
                return match self
                    .durable_input_terminal_receipt_read(session_id, target)
                    .await?
                {
                    Some(read) if read.report.is_resolved() => {
                        Ok(InputTerminalReceiptWait::Resolved(read))
                    }
                    read => Ok(InputTerminalReceiptWait::Detached(read)),
                };
            };
            let wake = {
                let guard = driver.lock().await;
                match Self::live_input_terminal_receipt_read(&guard, input_id)? {
                    Some(report) if report.is_resolved() => {
                        return Ok(InputTerminalReceiptWait::Resolved(Sourced {
                            source: TerminalWitnessSource::LiveRuntime,
                            report,
                        }));
                    }
                    // Same lock order as every resolver: driver, then
                    // registry. Registering before the driver guard drops
                    // orders this waiter before any later finalization.
                    Some(_) => completions.lock().await.register(input_id.clone()),
                    None => {
                        drop(guard);
                        let target = self
                            .durable_input_witness_by_id(session_id, input_id)
                            .await?;
                        return match self
                            .durable_input_terminal_receipt_read(session_id, target)
                            .await?
                        {
                            Some(read) if read.report.is_resolved() => {
                                Ok(InputTerminalReceiptWait::Resolved(read))
                            }
                            // A pending row that is not held live has no
                            // registration to wait on.
                            Some(read) => Ok(InputTerminalReceiptWait::Detached(Some(read))),
                            None => Err(RuntimeDriverError::ValidationFailed {
                                reason: format!(
                                    "input {input_id} is unknown to runtime session {session_id}"
                                ),
                            }),
                        };
                    }
                }
            };
            // Hold no session resources while parked, so teardown can drop
            // the registry and close this waiter.
            drop(completions);
            drop(driver);
            // A wake carries no semantics; the loop re-reads the receipt.
            let _wake = wake.try_wait().await;
        }
    }

    fn live_input_terminal_receipt_read(
        driver: &DriverEntry,
        input_id: &InputId,
    ) -> Result<Option<InputTerminalReceiptRead>, RuntimeDriverError> {
        driver
            .exact_input_terminal_completion_rows(input_id)?
            .map(|rows| Self::classify_input_terminal_receipt(&rows, input_id))
            .transpose()
    }

    async fn durable_input_terminal_receipt_read(
        &self,
        session_id: &SessionId,
        target: Option<StoredInputState>,
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
        let rows = Self::load_durable_terminal_completion_rows(store.as_ref(), &runtime_id, target)
            .await?;
        Ok(Some(Sourced {
            source: TerminalWitnessSource::DurableStore,
            report: Self::classify_input_terminal_receipt(&rows, &input_id)?,
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
