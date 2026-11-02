//! The resumed run of a durable session, on its live registration.
//!
//! A durable session that stopped inside a run keeps that run open. The
//! registration that opens the session again marks the run resumable on the
//! live authority, and the runtime loop resumes it ahead of any queued work.
//! This module holds the registration's resumed-run admission, the one host
//! hook asked before such a run is resumed.

use std::sync::Arc;

use super::{MeerkatMachine, RuntimeSessionRegistrationWitness};
use crate::resumed_run_admission::ResumedRunAdmission;
use crate::traits::RuntimeDriverError;

impl MeerkatMachine {
    /// The self-fence of this machine's store, when its writes hold a host
    /// store fence ([`crate::store::RuntimeStore::self_fence`]): a run that
    /// a store fence gap suspended resumes only once its window is open.
    pub(crate) fn store_self_fence(&self) -> Option<Arc<dyn meerkat_core::host_store::SelfFence>> {
        self.store.as_ref().and_then(|store| store.self_fence())
    }

    /// Whether `session_id`'s store holds a provisional tail of `run_id`:
    /// the run made a round commit, so it resumes from the cursor the tail
    /// carries. A run with none (a store fence gap suspended it before its
    /// first round commit) starts again from its bound inputs instead. A
    /// tail that cannot be read counts as held: the resume then reads it
    /// again, through the session layer, and fails as a resume does.
    pub(crate) async fn run_holds_provisional_tail(
        &self,
        session_id: &meerkat_core::types::SessionId,
        run_id: &meerkat_core::lifecycle::RunId,
    ) -> bool {
        let Some(store) = self.store.as_ref() else {
            return false;
        };
        let runtime_id = crate::identifiers::LogicalRuntimeId::for_session(session_id);
        match store.session_persistence_profile() {
            crate::store::RuntimeSessionPersistenceProfile::HeadCanonicalV1 => {
                match store
                    .load_head_canonical_provisional_tail(&runtime_id)
                    .await
                {
                    Ok(tail) => tail.is_some_and(|authority| authority.run_id() == run_id),
                    Err(_) => true,
                }
            }
            crate::store::RuntimeSessionPersistenceProfile::WholeBlobV1 => {
                match store.load_whole_blob_provisional_tail(&runtime_id).await {
                    Ok(tail) => tail.is_some_and(|tail| tail.authority().run_id() == run_id),
                    Err(_) => true,
                }
            }
        }
    }

    /// The run `session_id`'s durable tail left open, as its registration
    /// observes it before input recovery.
    ///
    /// Persistent input recovery marks the observed run resumable on the
    /// registration's live authority and keeps its bound inputs bound
    /// instead of requeueing them. A store failure is retried as a
    /// registration backoff. Evidence the observation cannot prove is not a
    /// resumable run: its inputs normalize as before, and the later
    /// durable-tail recovery cannot continue a run whose inputs went back to
    /// the queue.
    pub(super) async fn observe_resumable_run_for_registration(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Result<Option<crate::recovery::ResumableRunObservation>, RuntimeDriverError> {
        let Some(store) = self.store.as_ref() else {
            return Ok(None);
        };
        match crate::recovery::observe_resumable_run(store.as_ref(), session_id).await {
            Ok(observation) => Ok(observation),
            Err(crate::recovery::DurableTailRecoveryError::Store(error)) => {
                Err(RuntimeDriverError::RecoveryBackoff {
                    reason: format!(
                        "failed to observe the open run of session {session_id} during registration: {error}"
                    ),
                })
            }
            Err(error) => {
                tracing::warn!(
                    %session_id,
                    %error,
                    "the session's open run is not observable as resumable; its inputs normalize as before"
                );
                Ok(None)
            }
        }
    }

    /// The interrupted tool calls of `run_id`, the run `session_id`'s
    /// registration resumes, as the session's process-custody evidence
    /// records them.
    ///
    /// The attachment that serves the registration leaves a run with a
    /// provisional tail out of its settlement: the run keeps its inputs and
    /// resumes. So its calls owe the model no notice; the resumed round reads
    /// them here and answers each call whose process may have run before the
    /// restart as interrupted, with an outcome-uncertain result, instead of
    /// dispatching it again. They are read from the durable evidence, not
    /// handed over by the attachment, so a resumed round that the loop starts
    /// before that hand-off still finds them.
    ///
    /// The evidence is the one the attachment installed on the session's
    /// bindings when it settled the session's earlier-incarnation processes.
    /// The read tells it the run resumes
    /// ([`InterruptedToolEvidence::run_resumes`](meerkat_core::tool_process::InterruptedToolEvidence::run_resumes)),
    /// so it also settles what the run's execution left in this host process
    /// when that execution ended here (a stop, or a teardown of an earlier
    /// registration of the session): a process that ran is interrupted
    /// evidence too, and a started call is never run again. Only the run this
    /// registration resumes is settled: the session's current run, which
    /// `ResumeRun` made current from the resumable mark. Another run id
    /// settles nothing and fails the call.
    ///
    /// Empty when the host keeps no process custody. An evidence store that
    /// cannot be read or settled, a host that keeps custody but whose
    /// evidence the attachment did not install, or a run that is not the
    /// session's current run fails the call, so the round is not dispatched
    /// without it.
    pub async fn resumed_run_interrupted_tool_calls(
        &self,
        session_id: &meerkat_core::types::SessionId,
        run_id: &meerkat_core::lifecycle::RunId,
    ) -> Result<Vec<meerkat_core::tool_process::InterruptedToolCall>, RuntimeDriverError> {
        let Some(evidence) = self.bound_interrupted_tool_evidence(session_id).await else {
            if self.interrupted_tool_evidence_source().is_some() {
                return Err(RuntimeDriverError::Internal(format!(
                    "the tool-process custody evidence of session {session_id} is not installed, so the interrupted tool calls of run {run_id}, which resumes, cannot be read"
                )));
            }
            return Ok(Vec::new());
        };
        if self.current_run(session_id).await.as_ref() != Some(run_id) {
            return Err(RuntimeDriverError::Internal(format!(
                "run {run_id} is not the current run of session {session_id}, so it does not resume here: its tool-process custody evidence is not settled"
            )));
        }
        let calls = evidence.run_resumes(run_id).await.map_err(|error| {
            RuntimeDriverError::Internal(format!(
                "failed to read the interrupted tool calls of run {run_id} of session {session_id}, which resumes: {error}"
            ))
        })?;
        Ok(calls
            .into_iter()
            .filter(|call| &call.run_id == run_id)
            .collect())
    }

    /// Acknowledge the process-custody evidence of `calls`, which a resumed
    /// run's round answered as interrupted before the run ended: the round's
    /// results told the model, so the evidence owes it nothing more. Best
    /// effort: evidence left behind is settled at the next materialization,
    /// where the run has ended.
    pub async fn acknowledge_resumed_run_interrupted_tool_calls(
        &self,
        session_id: &meerkat_core::types::SessionId,
        calls: &[meerkat_core::tool_process::InterruptedToolCall],
    ) {
        if calls.is_empty() {
            return;
        }
        let Some(evidence) = self.bound_interrupted_tool_evidence(session_id).await else {
            tracing::warn!(
                %session_id,
                "the resumed run's interrupted-call evidence is not installed; it stays for the next materialization"
            );
            return;
        };
        let entries: Vec<uuid::Uuid> = calls.iter().map(|call| call.entry_id).collect();
        if let Err(error) = evidence.acknowledge(&entries).await {
            tracing::warn!(
                %session_id,
                %error,
                "the resumed run's interrupted-call evidence stays for the next materialization"
            );
        }
    }

    /// The run `session_id`'s current registration keeps resumable, if any.
    pub async fn resumable_run_id(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Option<meerkat_core::lifecycle::RunId> {
        let driver = {
            let sessions = self.sessions.read().await;
            Arc::clone(&sessions.get(session_id)?.driver)
        };
        driver.lock().await.resumable_run_id()
    }

    /// Release the run `session_id`'s registration marked resumable to
    /// ordinary requeue: its provisional tail is discarded first, by its
    /// exact authority, and then its bound inputs go back to their lanes,
    /// committed durably, and the loop wakes to serve them. A crash in
    /// between leaves bound inputs and no tail, which registration
    /// normalizes as an interrupted run's. A tail that cannot be discarded
    /// (another run's stands, or the exact discard lost its race) releases
    /// nothing and fails. A registration that no longer marks `run_id`
    /// releases nothing.
    ///
    /// A run whose tail records that it re-based is refused, and nothing is
    /// released: its inputs are already in the committed body, so running
    /// them again would duplicate them. Settle such a run instead.
    pub async fn release_resumable_run(
        &self,
        session_id: &meerkat_core::types::SessionId,
        run_id: &meerkat_core::lifecycle::RunId,
    ) -> Result<Vec<meerkat_core::lifecycle::InputId>, RuntimeDriverError> {
        let _gate = self
            .lock_current_durability_ready_session_mutation_gate(session_id)
            .await?;
        let (driver, wake_tx) = {
            let sessions = self.sessions.read().await;
            let Some(entry) = sessions.get(session_id) else {
                return Ok(Vec::new());
            };
            (Arc::clone(&entry.driver), entry.wake_sender())
        };
        let requeued = {
            let mut driver = driver.lock().await;
            if driver.resumable_run_id().as_ref() != Some(run_id) {
                return Ok(Vec::new());
            }
            if driver.run_tail_records_rebase(run_id).await? {
                return Err(RuntimeDriverError::Internal(format!(
                    "run {run_id} of session {session_id} is not released: its provisional tail records its re-base, so its inputs are already committed; settle it instead"
                )));
            }
            if !driver.discard_run_provisional_tail(run_id).await? {
                return Err(RuntimeDriverError::Internal(format!(
                    "run {run_id} of session {session_id} is not released: a provisional tail it does not own stands, or its exact discard lost its race"
                )));
            }
            driver.release_resumable_run(run_id).await?
        };
        if let Some(wake_tx) = wake_tx {
            let _ = wake_tx.try_send(());
        }
        Ok(requeued)
    }

    /// Require a registration-authorized cold reload of `session_id`'s
    /// persistent runtime: its shared durability gate refuses every later
    /// ordinary mutation, so nothing the live runtime realizes from here is
    /// committed, until registration cold-loads a fresh driver from durable
    /// state. Returns whether a registered persistent session was degraded.
    ///
    /// The session layer calls it when durable state it was about to change
    /// in step with the runtime (a failed run's provisional tail) could not
    /// be changed: the runtime must then not commit its half either.
    pub async fn require_session_durability_reload(
        &self,
        session_id: &meerkat_core::types::SessionId,
        operation: &'static str,
        reason: impl Into<String>,
    ) -> bool {
        let reason = reason.into();
        let sessions = self.sessions.read().await;
        sessions
            .get(session_id)
            .and_then(|entry| entry.durability_health.as_ref())
            .is_some_and(|health| health.mark_reload_required(operation, reason))
    }

    /// Bind `admission` to the exact current registration of its session.
    ///
    /// The runtime asks it before it resumes a run the session left open (see
    /// [`ResumedRunAdmission`]). A registration holds one admission for its
    /// lifetime: binding the same admission again is accepted, another one is
    /// refused. A replaced, reopened or unregistered registration starts
    /// without one. Bind it before the session's executor is attached: the
    /// runtime asks only once the executor is attached and the loop is about
    /// to resume the run.
    pub async fn bind_resumed_run_admission(
        &self,
        registration: &RuntimeSessionRegistrationWitness,
        admission: Arc<dyn ResumedRunAdmission>,
    ) -> Result<(), RuntimeDriverError> {
        let stale = || RuntimeDriverError::StaleAuthority {
            reason: "resumed run admission registration witness is not current".to_string(),
        };
        if !registration.belongs_to(self) {
            return Err(stale());
        }
        let _gate = self
            .lock_current_durability_ready_session_mutation_gate(registration.session_id())
            .await?;
        let sessions = self.sessions.read().await;
        let entry = sessions.get(registration.session_id()).ok_or_else(stale)?;
        if !registration.matches_entry(entry) || &entry.epoch_id != registration.epoch_id() {
            return Err(stale());
        }
        entry.resumed_run_admission.bind(admission)
    }

    /// Whether the exact current registration of its session already holds
    /// a resumed-run admission.
    ///
    /// A host that adopts a registration bound by an earlier composition (a
    /// restore in the same process) reads this instead of binding a second
    /// admission, which the registration would refuse.
    pub async fn resumed_run_admission_bound(
        &self,
        registration: &RuntimeSessionRegistrationWitness,
    ) -> Result<bool, RuntimeDriverError> {
        let stale = || RuntimeDriverError::StaleAuthority {
            reason: "resumed run admission registration witness is not current".to_string(),
        };
        if !registration.belongs_to(self) {
            return Err(stale());
        }
        let sessions = self.sessions.read().await;
        let entry = sessions.get(registration.session_id()).ok_or_else(stale)?;
        if !registration.matches_entry(entry) || &entry.epoch_id != registration.epoch_id() {
            return Err(stale());
        }
        Ok(entry.resumed_run_admission.current().is_some())
    }

    /// The resumed-run admission bound to `session_id`'s current
    /// registration, if any.
    pub(crate) async fn resumed_run_admission(
        &self,
        session_id: &meerkat_core::types::SessionId,
    ) -> Option<Arc<dyn ResumedRunAdmission>> {
        self.sessions
            .read()
            .await
            .get(session_id)
            .and_then(|entry| entry.resumed_run_admission.current())
    }
}

impl super::driver::DriverEntry {
    fn resumed_run_shell(&self) -> &crate::driver::EphemeralRuntimeDriver {
        match self {
            super::driver::DriverEntry::Ephemeral(driver) => driver,
            super::driver::DriverEntry::Persistent(driver) => driver.inner_ref(),
        }
    }

    fn resumed_run_shell_mut(&mut self) -> &mut crate::driver::EphemeralRuntimeDriver {
        match self {
            super::driver::DriverEntry::Ephemeral(driver) => driver,
            super::driver::DriverEntry::Persistent(driver) => driver.inner_mut(),
        }
    }

    /// Mark `run_id` resumable again after a reset or destroy that ended its
    /// mark failed before its sweep finished, so the run's bound inputs stay
    /// guarded: no other run may start over the run's provisional tail. The
    /// mark lives on the live authority only, so nothing durable changes.
    /// False, logged, when the machine refuses the mark; the caller returns
    /// its own error either way.
    pub(crate) fn restore_ended_resumable_run(
        &mut self,
        run_id: &meerkat_core::lifecycle::RunId,
    ) -> bool {
        let restored = self.resumed_run_shell_mut().mark_run_resumable(run_id);
        if !restored {
            tracing::error!(
                %run_id,
                "a failed reset or destroy could not mark the run it ended resumable again"
            );
        }
        restored
    }

    /// Whether `input_id` is still bound to the open `run_id`: staged for it,
    /// or applied by one of its committed rounds.
    pub(crate) fn input_bound_to_open_run(
        &self,
        input_id: &meerkat_core::lifecycle::InputId,
        run_id: &meerkat_core::lifecycle::RunId,
    ) -> bool {
        self.resumed_run_shell()
            .input_bound_to_open_run(input_id, run_id)
    }

    /// The session this runtime's machine is registered for, if any.
    pub(crate) fn session_id(&self) -> Option<meerkat_core::types::SessionId> {
        let authority = self.shared_dsl_authority();
        let authority = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        authority
            .state()
            .session_id
            .as_ref()
            .and_then(|session_id| meerkat_core::types::SessionId::parse(&session_id.0).ok())
    }

    /// Whether the machine would suspend `run_id` for this runtime's stop now:
    /// `SuspendRunForStop` previewed on a fork of the live authority, for a
    /// graceful requeue stop on a durable session, the run current, its turn
    /// not ended and no live-boundary join outstanding. The unregister saga
    /// asks the executor to suspend only then, so the loop's own application
    /// is refused only by a change in between; a run it would not suspend is
    /// hard-cancelled as before, with its terminal on the session's stream.
    pub(crate) fn suspension_admissible(&self, run_id: &meerkat_core::lifecycle::RunId) -> bool {
        let durable_session = matches!(self, super::driver::DriverEntry::Persistent(_));
        if !durable_session || !self.stop_keeps_queued_work() {
            return false;
        }
        let authority = self.shared_dsl_authority();
        super::MeerkatMachine::preview_dsl_input_on_shared_authority(
            &authority,
            crate::meerkat_machine::dsl::MeerkatMachineInput::SuspendRunForStop {
                run_id: crate::meerkat_machine::dsl::RunId::from_domain(run_id),
                durable_session,
                cause: crate::meerkat_machine::dsl::RunSuspensionCause::RequeueStop,
            },
            "SuspendRunForStop",
        )
        .is_ok()
    }

    /// Whether a stop was asked of the run marked resumable (the machine's
    /// stop intent names it): a cancel the run accepted before a store fence
    /// gap suspended it, a stop or an interrupt of it at the fence, or one of
    /// it while it was marked resumable (suspended by the gap, or left open
    /// by a restart). Such a run never resumes; the runtime settles it.
    pub(crate) fn resumable_run_stop_requested(&self) -> bool {
        let authority = self.shared_dsl_authority();
        let authority = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let state = authority.state();
        state.resumable_run_id.is_some() && state.run_stop_requested == state.resumable_run_id
    }

    /// The session's unfinished inputs its store keeps: every active input
    /// but an ephemeral one, which no store holds (an input whose row does
    /// not say counts as kept).
    pub(crate) fn store_kept_input_ids(&self) -> Vec<meerkat_core::lifecycle::InputId> {
        let driver = self.as_driver();
        driver
            .active_input_ids()
            .into_iter()
            .filter(|input_id| {
                driver.stored_input_state(input_id).is_none_or(|stored| {
                    stored.state.durability != Some(crate::input::InputDurability::Ephemeral)
                })
            })
            .collect()
    }

    /// Whether the registration is draining for an unregister: its runtime is
    /// stopping, whatever it keeps for the next start.
    pub(crate) fn registration_draining(&self) -> bool {
        let authority = self.shared_dsl_authority();
        let authority = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        authority.state().registration_phase
            == crate::meerkat_machine::dsl::RegistrationPhase::Draining
    }

    /// Whether this runtime's stop keeps its queued work for the next start:
    /// a graceful unregister that requeues, or the rollback of a
    /// materialization that never served, which keeps every input its
    /// registration recovered (either way, the inputs bound to a run marked
    /// resumable whose stop was asked end as stopped). Such a stop leaves a
    /// run the session left open to the next registration, which finds it
    /// open again; every other stop ends that run with the stop.
    pub(crate) fn stop_keeps_queued_work(&self) -> bool {
        let authority = self.shared_dsl_authority();
        let authority = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let state = authority.state();
        state.registration_phase == crate::meerkat_machine::dsl::RegistrationPhase::Draining
            && matches!(
                state.unregister_queued_inputs,
                crate::meerkat_machine::dsl::UnregisterQueuedInputsDisposition::Requeue
                    | crate::meerkat_machine::dsl::UnregisterQueuedInputsDisposition::Keep
            )
    }

    /// The inputs still bound to the open `run_id`, in admission order.
    pub(crate) fn resumed_run_bound_input_ids(
        &self,
        run_id: &meerkat_core::lifecycle::RunId,
    ) -> Vec<meerkat_core::lifecycle::InputId> {
        self.resumed_run_shell().resumed_run_bound_input_ids(run_id)
    }

    /// Repair the provisional tail of `run_id`, a marked run settled without
    /// resuming, once its inputs' abandonment has committed: the durable-tail
    /// recovery a registration runs, guarded to that run
    /// (`crate::recovery::recover_durable_tail_for_run`), commits the tail as
    /// an interrupted run (every row kept, open calls answered as
    /// outcome-unknown, the interrupted notice appended), and the inputs stay
    /// terminal with the settlement's reason. It runs under this driver's
    /// lock, on the store's current tail, as a registration's recovery does;
    /// the store's fences hold across processes. A crash between the
    /// settlement and the repair leaves the same state for the next
    /// registration's recovery.
    ///
    /// Returns whether the session must reload before it serves again:
    /// `true` only when a tail is left standing (another run's, one the
    /// recovery held, refused or would continue, or a failed recovery), which
    /// would refuse the next run's first round commit and is the next
    /// registration's to judge.
    ///
    /// `false` when the repair committed, as after a cancelled run's settle
    /// ([`Self::settle_cancelled_run_tail`]): the repair wrote a boundary this
    /// registration did not commit, and the session layer resynchronizes the
    /// actor seeded from the tail from that committed boundary, its
    /// checkpointer's receipt and base included, at its next step under the
    /// turn boundary. Also `false` when no tail stood (the committed document
    /// was already aligned). An ephemeral runtime keeps no tail.
    pub(crate) async fn repair_settled_run_tail(
        &self,
        session_id: &meerkat_core::types::SessionId,
        run_id: &meerkat_core::lifecycle::RunId,
    ) -> bool {
        let super::driver::DriverEntry::Persistent(driver) = self else {
            return false;
        };
        let outcome =
            crate::recovery::recover_durable_tail_for_run(driver.store_ref(), session_id, run_id)
                .await;
        let left = match &outcome {
            Ok(Some(crate::recovery::DurableTailRecoveryOutcome::AlreadyAligned { .. })) => {
                return false;
            }
            Ok(Some(crate::recovery::DurableTailRecoveryOutcome::Committed {
                boundary_sequence,
                ..
            })) => {
                tracing::info!(
                    %session_id,
                    %run_id,
                    boundary_sequence,
                    "repaired the settled run's provisional tail as an interrupted run"
                );
                return false;
            }
            Ok(None) => "another run's provisional tail stands".to_string(),
            Ok(Some(crate::recovery::DurableTailRecoveryOutcome::Held)) => {
                "the recovery held the tail".to_string()
            }
            Ok(Some(crate::recovery::DurableTailRecoveryOutcome::Refused)) => {
                "the recovery refused the tail".to_string()
            }
            Ok(Some(crate::recovery::DurableTailRecoveryOutcome::Continue { .. })) => {
                "the recovery would continue the run".to_string()
            }
            Err(error) => format!("the recovery failed: {error}"),
        };
        tracing::warn!(
            %session_id,
            %run_id,
            reason = %left,
            "the settled run's provisional tail was not repaired"
        );
        true
    }

    /// Close the provisional tail of `run_id`, a run whose cancellation the
    /// loop has just committed, as an interrupted run.
    ///
    /// A cancel (a user interrupt, a run stop, a refused suspension) drops
    /// the run future, so the run's last round commit stays in the store as
    /// its provisional tail, cursor included. Left standing, that tail
    /// refuses the session's next ordinary save and the next run's first
    /// commit until a registration's recovery settles it. Once the
    /// cancellation committed (no current run, the run's inputs abandoned),
    /// the durable-tail recovery a registration runs, guarded to that run
    /// ([`crate::recovery::recover_durable_tail_for_run`]), commits the tail
    /// as an interrupted run here instead: every row kept, open calls
    /// answered as outcome-unknown, the interrupted notice appended.
    ///
    /// This asks for no reload, as [`Self::repair_settled_run_tail`] asks
    /// none after a committed repair. The session layer kept the live actor,
    /// and its next step under the turn boundary resynchronizes that actor
    /// from the committed boundary, the checkpointer's receipt and base
    /// included. Unlike that repair, it asks none for a tail it leaves
    /// either: a tail the recovery holds, refuses or would continue, another
    /// run's tail, and a failed recovery are left as they stand, for the next
    /// registration's recovery, and logged. A suspended run never comes here: the loop realizes a
    /// suspension before its cancelled arm. An ephemeral runtime keeps no
    /// tail.
    pub(crate) async fn settle_cancelled_run_tail(
        &self,
        session_id: &meerkat_core::types::SessionId,
        run_id: &meerkat_core::lifecycle::RunId,
    ) {
        let super::driver::DriverEntry::Persistent(driver) = self else {
            return;
        };
        let outcome =
            crate::recovery::recover_durable_tail_for_run(driver.store_ref(), session_id, run_id)
                .await;
        let left = match &outcome {
            // No tail stood: the run left nothing to settle.
            Ok(Some(crate::recovery::DurableTailRecoveryOutcome::AlreadyAligned { .. })) => {
                return;
            }
            Ok(Some(crate::recovery::DurableTailRecoveryOutcome::Committed {
                boundary_sequence,
                ..
            })) => {
                tracing::info!(
                    %session_id,
                    %run_id,
                    boundary_sequence,
                    "closed the cancelled run's provisional tail as an interrupted run"
                );
                return;
            }
            Ok(None) => "another run's provisional tail stands".to_string(),
            Ok(Some(crate::recovery::DurableTailRecoveryOutcome::Held)) => {
                "the recovery held the tail".to_string()
            }
            Ok(Some(crate::recovery::DurableTailRecoveryOutcome::Refused)) => {
                "the recovery refused the tail".to_string()
            }
            Ok(Some(crate::recovery::DurableTailRecoveryOutcome::Continue { .. })) => {
                "the recovery would continue the run".to_string()
            }
            Err(error) => format!("the recovery failed: {error}"),
        };
        tracing::warn!(
            %session_id,
            %run_id,
            reason = %left,
            "the cancelled run's provisional tail was not settled; it stays for the next registration's recovery"
        );
    }

    /// Clear the requeue marks a graceful stop left on a resumed run's
    /// inputs as it starts, durably for a persistent runtime, before the run
    /// is dispatched.
    pub(crate) async fn clear_resumed_run_stop_marks(
        &mut self,
        input_ids: &[meerkat_core::lifecycle::InputId],
    ) -> Result<(), RuntimeDriverError> {
        match self {
            super::driver::DriverEntry::Ephemeral(driver) => {
                driver.clear_stop_requeue_marks(input_ids);
                Ok(())
            }
            super::driver::DriverEntry::Persistent(driver) => {
                driver.clear_resumed_run_stop_marks(input_ids).await
            }
        }
    }

    /// Whether a graceful stop that kept the open `run_id` has outlived its
    /// max age at `now`, for any of the run's bound inputs.
    pub(crate) fn resumable_run_stop_requeue_expired(
        &self,
        run_id: &meerkat_core::lifecycle::RunId,
        now: chrono::DateTime<chrono::Utc>,
    ) -> bool {
        self.resumed_run_shell()
            .resumable_run_stop_requeue_expired(run_id, now)
    }

    /// The payloads of a resumed run's bound inputs, from the ledger.
    pub(crate) fn hydrate_resumed_run_inputs(
        &self,
        run_id: &meerkat_core::lifecycle::RunId,
        input_ids: &[meerkat_core::lifecycle::InputId],
    ) -> Result<Vec<(meerkat_core::lifecycle::InputId, crate::input::Input)>, RuntimeDriverError>
    {
        self.resumed_run_shell()
            .hydrate_resumed_run_inputs(run_id, input_ids)
    }
}
