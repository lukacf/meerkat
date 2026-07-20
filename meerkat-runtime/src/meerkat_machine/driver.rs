//! Runtime driver entry point and lifecycle.

use std::sync::Arc;

use meerkat_core::lifecycle::{CoreApplyFailureCause, InputId, RunBoundaryReceipt, RunId};
use meerkat_core::types::SessionId;

use crate::accept::{AcceptOutcome, ResolvedAdmission};
use crate::driver::ephemeral::{EphemeralDriverRollbackSnapshot, EphemeralRuntimeDriver};
use crate::driver::persistent::PersistentRuntimeDriver;
use crate::identifiers::LogicalRuntimeId;
use crate::ingress_types::ContentShape;
use crate::input::Input;
use crate::input_state::{
    InputLifecycleState, InputState, InputStateHistoryEntry, InputStateSeed, InputTerminalOutcome,
    StoredInputState,
};
use crate::meerkat_machine::dsl::command_capabilities as generated_command_capabilities;
use crate::runtime_state::RuntimeState;
use crate::tokio::sync::Mutex;
use crate::traits::{
    DestroyReport, RecoveryReport, ResetReport, RetireReport, RuntimeDriver, RuntimeDriverError,
};
use chrono::Utc;
use meerkat_machine_kernels::generated::meerkat::command_capabilities as generated_kernel_command_capabilities;
use sha2::{Digest, Sha256};

/// Shared driver handle used by both the adapter and the RuntimeLoop.
pub(crate) type SharedDriver = Arc<Mutex<DriverEntry>>;

/// Proof that generated MeerkatMachine runtime-completion authority selected
/// the public completion result class.
///
/// The completion registry accepts this token rather than a bare generated enum
/// so handwritten code cannot directly fabricate waiter public result truth.
#[must_use = "runtime completion authority must be consumed by waiter resolution"]
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct RuntimeCompletionResultAuthority {
    generated_plan: generated_kernel_command_capabilities::CommandPlanKind,
    session_id: SessionId,
    agent_runtime_id: Option<crate::meerkat_machine::dsl::AgentRuntimeId>,
    fence_token: Option<crate::meerkat_machine::dsl::FenceToken>,
    runtime_generation: Option<crate::meerkat_machine::dsl::Generation>,
    runtime_epoch_id: Option<crate::meerkat_machine::dsl::RuntimeEpochId>,
    result_class: crate::meerkat_machine::dsl::RuntimeCompletionResultClass,
    cleanup_observation: crate::meerkat_machine::dsl::RuntimeCompletionObservedOutcome,
}

/// Attempted local closure of a generated runtime-completion result effect.
///
/// This is the runtime-facing phase view for the
/// `Authorized -> Attempted -> Realized | Failed | Abandoned` closure declared
/// by the generated `AuthorizedRuntimeCompletionResultClosure` command plan.
#[must_use = "attempted runtime completion closure must be realized, failed, or abandoned"]
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct RuntimeCompletionResultAttempt {
    authority: RuntimeCompletionResultAuthority,
}

/// Realized local closure of a generated runtime-completion result effect.
///
/// Completion waiter delivery and cleanup observations are minted only from
/// this consumed phase, so payload alignment failures cannot still surface a
/// public waiter result.
#[must_use = "realized runtime completion closure must mint a completion cleanup observation"]
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct RuntimeCompletionResultRealized {
    authority: RuntimeCompletionResultAuthority,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InteractionTerminalOwnerBinding {
    session_id: SessionId,
    agent_runtime_id: Option<String>,
    fence_token: Option<u64>,
    runtime_generation: Option<u64>,
    runtime_epoch_id: Option<String>,
}

impl InteractionTerminalOwnerBinding {
    fn from_outbox(outbox: &crate::input_state::InteractionTerminalOutbox) -> Self {
        Self {
            session_id: outbox.owner_session_id.clone(),
            agent_runtime_id: outbox.owner_agent_runtime_id.clone(),
            fence_token: outbox.owner_fence_token,
            runtime_generation: outbox.owner_runtime_generation,
            runtime_epoch_id: outbox.owner_runtime_epoch_id.clone(),
        }
    }

    fn write_to(&self, outbox: &mut crate::input_state::InteractionTerminalOutbox) {
        outbox.owner_session_id = self.session_id.clone();
        outbox.owner_agent_runtime_id = self.agent_runtime_id.clone();
        outbox.owner_fence_token = self.fence_token;
        outbox.owner_runtime_generation = self.runtime_generation;
        outbox.owner_runtime_epoch_id = self.runtime_epoch_id.clone();
    }
}

#[must_use = "authorized outbox adoption must be atomically realized"]
struct AuthorizedInteractionTerminalOutboxAdoption {
    generated_plan: generated_kernel_command_capabilities::CommandPlanKind,
    batch_key: crate::input_state::InteractionTerminalBatchKey,
    input_ids: Vec<InputId>,
    candidate_digest: String,
    previous: InteractionTerminalOwnerBinding,
    next: InteractionTerminalOwnerBinding,
}

pub(crate) enum InteractionTerminalRecoveryPhase {
    Candidate,
    Finalized {
        events: Vec<meerkat_core::event::AgentEvent>,
        finalization: crate::meerkat_machine::dsl::RuntimeCompletionFinalizationObservation,
        finalization_error: Option<meerkat_core::TurnErrorMetadata>,
    },
}

pub(crate) struct InteractionTerminalRecoveryBatch {
    pub(crate) batch_key: crate::input_state::InteractionTerminalBatchKey,
    pub(crate) input_ids: Vec<InputId>,
    pub(crate) interaction_ids: Vec<meerkat_core::interaction::InteractionId>,
    pub(crate) terminal: Option<meerkat_core::lifecycle::core_executor::CoreApplyTerminal>,
    pub(crate) terminal_observation:
        crate::meerkat_machine::dsl::RuntimeCompletionTerminalObservation,
    pub(crate) runtime_termination_reason: Option<String>,
    pub(crate) completion_error_metadata: Option<meerkat_core::TurnErrorMetadata>,
    pub(crate) phase: InteractionTerminalRecoveryPhase,
}

enum InteractionTerminalBatchScope<'a> {
    Run(&'a RunId),
    RuntimeTermination,
}

impl RuntimeCompletionResultAuthority {
    fn from_generated_effect(
        session_id: SessionId,
        agent_runtime_id: Option<crate::meerkat_machine::dsl::AgentRuntimeId>,
        fence_token: Option<crate::meerkat_machine::dsl::FenceToken>,
        runtime_generation: Option<crate::meerkat_machine::dsl::Generation>,
        runtime_epoch_id: Option<crate::meerkat_machine::dsl::RuntimeEpochId>,
        result_class: crate::meerkat_machine::dsl::RuntimeCompletionResultClass,
        cleanup_observation: crate::meerkat_machine::dsl::RuntimeCompletionObservedOutcome,
    ) -> Self {
        Self {
            generated_plan:
                generated_kernel_command_capabilities::CommandPlanKind::AuthorizedRuntimeCompletionResultClosure,
            session_id,
            agent_runtime_id,
            fence_token,
            runtime_generation,
            runtime_epoch_id,
            result_class,
            cleanup_observation,
        }
    }

    pub(crate) fn begin_surface_resolution(self) -> RuntimeCompletionResultAttempt {
        debug_assert_eq!(
            self.generated_plan,
            generated_kernel_command_capabilities::CommandPlanKind::AuthorizedRuntimeCompletionResultClosure
        );
        RuntimeCompletionResultAttempt { authority: self }
    }
}

impl RuntimeCompletionResultAttempt {
    pub(crate) fn class(&self) -> crate::meerkat_machine::dsl::RuntimeCompletionResultClass {
        self.authority.result_class
    }

    pub(crate) fn allows(
        &self,
        expected: crate::meerkat_machine::dsl::RuntimeCompletionResultClass,
    ) -> bool {
        self.authority.result_class == expected
    }

    pub(crate) fn realize(self) -> RuntimeCompletionResultRealized {
        RuntimeCompletionResultRealized {
            authority: self.authority,
        }
    }

    pub(crate) fn fail(self) {
        drop(self);
    }

    #[cfg(test)]
    pub(crate) fn abandon(self) {
        drop(self);
    }
}

impl RuntimeCompletionResultRealized {
    pub(crate) fn session_id(&self) -> &SessionId {
        &self.authority.session_id
    }

    pub(crate) fn agent_runtime_id(&self) -> Option<&crate::meerkat_machine::dsl::AgentRuntimeId> {
        self.authority.agent_runtime_id.as_ref()
    }

    pub(crate) fn fence_token(&self) -> Option<crate::meerkat_machine::dsl::FenceToken> {
        self.authority.fence_token
    }

    pub(crate) fn runtime_generation(&self) -> Option<crate::meerkat_machine::dsl::Generation> {
        self.authority.runtime_generation
    }

    pub(crate) fn runtime_epoch_id(&self) -> Option<&crate::meerkat_machine::dsl::RuntimeEpochId> {
        self.authority.runtime_epoch_id.as_ref()
    }

    pub(crate) fn cleanup_observation(
        &self,
    ) -> crate::meerkat_machine::dsl::RuntimeCompletionObservedOutcome {
        self.authority.cleanup_observation
    }
}

#[cfg(test)]
pub(crate) fn test_runtime_completion_authority(
    result_class: crate::meerkat_machine::dsl::RuntimeCompletionResultClass,
    cleanup_observation: crate::meerkat_machine::dsl::RuntimeCompletionObservedOutcome,
) -> RuntimeCompletionResultAuthority {
    RuntimeCompletionResultAuthority::from_generated_effect(
        SessionId::new(),
        Some(crate::meerkat_machine::dsl::AgentRuntimeId::from(
            "test-runtime",
        )),
        Some(crate::meerkat_machine::dsl::FenceToken::from(0)),
        Some(crate::meerkat_machine::dsl::Generation::from(0)),
        None,
        result_class,
        cleanup_observation,
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RuntimeLoopBatchSource {
    Queue,
    Steer,
}

impl From<generated_command_capabilities::RuntimeLoopBatchSource> for RuntimeLoopBatchSource {
    fn from(source: generated_command_capabilities::RuntimeLoopBatchSource) -> Self {
        match source {
            generated_command_capabilities::RuntimeLoopBatchSource::Queue => Self::Queue,
            generated_command_capabilities::RuntimeLoopBatchSource::Steer => Self::Steer,
        }
    }
}

impl From<RuntimeLoopBatchSource> for generated_command_capabilities::RuntimeLoopBatchSource {
    fn from(source: RuntimeLoopBatchSource) -> Self {
        match source {
            RuntimeLoopBatchSource::Queue => Self::Queue,
            RuntimeLoopBatchSource::Steer => Self::Steer,
        }
    }
}

#[must_use = "runtime loop batch authority must be consumed by stage authorization"]
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct AuthorizedRuntimeLoopBatch {
    input_ids: Vec<InputId>,
    source: RuntimeLoopBatchSource,
    generated: generated_command_capabilities::AuthorizedRuntimeLoopBatch,
}

impl AuthorizedRuntimeLoopBatch {
    fn from_generated_plan(
        plan: generated_command_capabilities::RuntimeLoopBatchPlan,
    ) -> Option<Self> {
        let (generated, raw_input_ids, generated_source) = plan.into_parts();
        let input_ids = raw_input_ids
            .into_iter()
            .map(|raw| raw.parse::<uuid::Uuid>().ok().map(InputId::from_uuid))
            .collect::<Option<Vec<_>>>()?;
        if input_ids.is_empty() {
            None
        } else {
            Some(Self {
                input_ids,
                source: generated_source.into(),
                generated,
            })
        }
    }

    pub(crate) fn input_ids(&self) -> &[InputId] {
        &self.input_ids
    }

    pub(crate) fn source(&self) -> RuntimeLoopBatchSource {
        self.source
    }
}

#[cfg(test)]
pub(crate) fn test_authorized_runtime_loop_batch(
    input_ids: Vec<InputId>,
) -> AuthorizedRuntimeLoopBatch {
    test_authorized_runtime_loop_batch_from_source(input_ids, RuntimeLoopBatchSource::Queue)
}

#[cfg(test)]
pub(crate) fn test_authorized_runtime_loop_batch_from_source(
    input_ids: Vec<InputId>,
    source: RuntimeLoopBatchSource,
) -> AuthorizedRuntimeLoopBatch {
    AuthorizedRuntimeLoopBatch {
        input_ids,
        source,
        generated:
            generated_command_capabilities::AuthorizedRuntimeLoopBatch::mint_from_generated_command_plan(),
    }
}

#[must_use = "stage-for-run authority must be consumed by machine_realize_stage_batch"]
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct AuthorizedStageForRun {
    input_ids: Vec<InputId>,
    run_id: RunId,
    source: RuntimeLoopBatchSource,
    generated: generated_command_capabilities::AuthorizedStageForRun,
}

impl AuthorizedStageForRun {
    fn from_generated_plan(
        run_id: RunId,
        plan: generated_command_capabilities::StageForRunPlan,
    ) -> Option<Self> {
        let (generated, raw_input_ids, generated_source) = plan.into_parts();
        let input_ids = raw_input_ids
            .into_iter()
            .map(|raw| raw.parse::<uuid::Uuid>().ok().map(InputId::from_uuid))
            .collect::<Option<Vec<_>>>()?;
        if input_ids.is_empty() {
            None
        } else {
            Some(Self {
                input_ids,
                run_id,
                source: generated_source.into(),
                generated,
            })
        }
    }

    pub(crate) fn into_parts(self) -> (Vec<InputId>, RunId, RuntimeLoopBatchSource) {
        let Self {
            input_ids,
            run_id,
            source,
            generated: _,
        } = self;
        (input_ids, run_id, source)
    }
}

#[cfg(test)]
pub(crate) fn test_authorized_stage_for_run(
    input_ids: Vec<InputId>,
    run_id: RunId,
) -> AuthorizedStageForRun {
    AuthorizedStageForRun {
        input_ids,
        run_id,
        source: RuntimeLoopBatchSource::Queue,
        generated:
            generated_command_capabilities::AuthorizedStageForRun::mint_from_generated_command_plan(
            ),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RuntimeLifecycleProjection {
    pub(crate) phase: RuntimeState,
    pub(crate) current_run_id: Option<RunId>,
    pub(crate) pre_run_phase: Option<RuntimeState>,
}

impl RuntimeLifecycleProjection {
    fn from_authority(authority: &crate::meerkat_machine::dsl::MeerkatMachineAuthority) -> Self {
        Self {
            phase: crate::meerkat_machine::dsl_authority::runtime_phase_from_authority(authority),
            current_run_id: crate::meerkat_machine::dsl_authority::current_run_id_from_authority(
                authority,
            ),
            pre_run_phase: crate::meerkat_machine::dsl_authority::pre_run_phase_from_authority(
                authority,
            ),
        }
    }
}

/// Read-only view over driver-local ingress state. The driver itself owns
/// the DSL and shell metadata; this view just forwards the read accessors
/// needed by callers that used to inspect the deleted `RuntimeIngressAuthority`.
pub(crate) struct IngressView<'a> {
    driver: &'a EphemeralRuntimeDriver,
}

impl IngressView<'_> {
    pub(crate) fn queue(&self) -> Vec<InputId> {
        self.driver.queue_lane()
    }

    pub(crate) fn steer_queue(&self) -> Vec<InputId> {
        self.driver.steer_lane()
    }

    pub(crate) fn admission_order(&self) -> Vec<InputId> {
        self.driver.admission_order()
    }

    pub(crate) fn handling_mode(
        &self,
        input_id: &InputId,
    ) -> Option<meerkat_core::types::HandlingMode> {
        self.driver.admitted_handling_mode(input_id)
    }

    /// #338: machine-owned per-input live-interrupt verdict, read from the
    /// admission `RuntimeInputSemantics`. Defaults to `false` when no admission
    /// semantics were recorded for the input.
    pub(crate) fn live_interrupt_required(&self, input_id: &InputId) -> bool {
        self.driver
            .admitted_runtime_semantics(input_id)
            .is_some_and(|semantics| semantics.live_interrupt_required)
    }

    pub(crate) fn runtime_semantics(
        &self,
        input_id: &InputId,
    ) -> Option<crate::ingress_types::RuntimeInputSemantics> {
        self.driver.admitted_runtime_semantics(input_id)
    }

    pub(crate) fn primitive_projection(
        &self,
        input_id: &InputId,
    ) -> Option<crate::ingress_types::RuntimeInputProjection> {
        self.driver.admitted_primitive_projection(input_id)
    }

    pub(crate) fn is_prompt(&self, input_id: &InputId) -> bool {
        self.driver.admitted_is_prompt(input_id)
    }

    pub(crate) fn content_shape(&self, input_id: &InputId) -> Option<ContentShape> {
        self.driver.admitted_content_shape(input_id)
    }

    pub(crate) fn request_id(&self, input_id: &InputId) -> Option<crate::ingress_types::RequestId> {
        self.driver.admitted_request_id(input_id)
    }

    pub(crate) fn reservation_key(
        &self,
        input_id: &InputId,
    ) -> Option<crate::ingress_types::ReservationKey> {
        self.driver.admitted_reservation_key(input_id)
    }

    #[allow(dead_code)]
    pub(crate) fn lifecycle_state(&self, input_id: &InputId) -> Option<InputLifecycleState> {
        self.driver.ingress_lifecycle(input_id)
    }
}

/// Per-session runtime driver entry.
pub(crate) enum DriverEntry {
    Ephemeral(EphemeralRuntimeDriver),
    Persistent(PersistentRuntimeDriver),
}

pub(crate) enum PreparedDestroyLifecycle {
    Ephemeral(EphemeralDriverRollbackSnapshot),
    Persistent(EphemeralDriverRollbackSnapshot),
}

enum DriverRollbackSnapshot {
    Ephemeral(EphemeralDriverRollbackSnapshot),
    Persistent(EphemeralDriverRollbackSnapshot),
}

#[must_use = "prepared runless terminal outboxes must be committed or rolled back"]
pub(crate) struct PreparedRunlessInteractionTerminalOutboxes {
    checkpoint: Option<DriverRollbackSnapshot>,
    candidate_owner_input_id: Option<InputId>,
}

pub(crate) struct PreparedDestroy {
    pub(crate) report: DestroyReport,
    pub(crate) lifecycle: PreparedDestroyLifecycle,
}

impl DriverEntry {
    fn shell_driver_mut(&mut self) -> &mut EphemeralRuntimeDriver {
        match self {
            DriverEntry::Ephemeral(driver) => driver,
            DriverEntry::Persistent(driver) => driver.inner_mut(),
        }
    }

    fn stage_interaction_terminal_outboxes(
        &mut self,
        outboxes: Vec<crate::input_state::InteractionTerminalOutbox>,
    ) -> Result<(), RuntimeDriverError> {
        let ledger = self.shell_driver_mut().ledger_mut();
        for outbox in outboxes {
            outbox.validate().map_err(RuntimeDriverError::Internal)?;
            let state = ledger.get_mut(&outbox.input_id).ok_or_else(|| {
                RuntimeDriverError::Internal(format!(
                    "directed terminal outbox input {} disappeared before commit",
                    outbox.input_id
                ))
            })?;
            match state.interaction_terminal_outbox.as_ref() {
                Some(existing)
                    if existing.interaction_id == outbox.interaction_id
                        && existing.batch_ordinal == outbox.batch_ordinal
                        && existing.batch_key == outbox.batch_key
                        && existing.candidate_owner_input_id == outbox.candidate_owner_input_id
                        && existing.candidate_digest == outbox.candidate_digest
                        && existing.completion_input_ids_digest
                            == outbox.completion_input_ids_digest =>
                {
                    // Exact boundary-commit retry: preserve any later
                    // finalized/publication facts already attached.
                }
                Some(_) => {
                    return Err(RuntimeDriverError::ValidationFailed {
                        reason: format!(
                            "directed terminal outbox identity/payload conflict for input {}",
                            outbox.input_id
                        ),
                    });
                }
                None => state.interaction_terminal_outbox = Some(outbox),
            }
        }
        Ok(())
    }

    /// Stage one runless runtime-termination batch into the same in-memory
    /// ledger snapshot that the caller will terminalize and persist. The
    /// canonical candidate-owner input is the durable group key; no synthetic
    /// run identity is minted for queued/staged inputs that never ran.
    pub(crate) fn prepare_runless_runtime_terminated_interaction_outboxes(
        &mut self,
        input_ids: &[InputId],
        reason: String,
    ) -> Result<PreparedRunlessInteractionTerminalOutboxes, RuntimeDriverError> {
        if let Some(candidate_owner_input_id) =
            self.existing_exact_runless_runtime_termination_batch(input_ids, &reason)?
        {
            return Ok(PreparedRunlessInteractionTerminalOutboxes {
                checkpoint: None,
                candidate_owner_input_id: Some(candidate_owner_input_id),
            });
        }
        let checkpoint = self.rollback_snapshot();
        let outboxes = authorized_staged_directed_terminal_outboxes(
            self,
            InteractionTerminalBatchScope::RuntimeTermination,
            input_ids,
            crate::input_state::InteractionTerminalCandidate::RuntimeTerminated { reason },
        )?;
        let candidate_owner_input_id = outboxes
            .first()
            .map(|outbox| outbox.candidate_owner_input_id.clone());
        if let Err(error) = self.stage_interaction_terminal_outboxes(outboxes) {
            self.restore_rollback_snapshot(checkpoint);
            return Err(error);
        }
        Ok(PreparedRunlessInteractionTerminalOutboxes {
            checkpoint: Some(checkpoint),
            candidate_owner_input_id,
        })
    }

    /// Whether any currently non-terminal input requires an Interaction
    /// terminal publication capability before it may be abandoned.
    ///
    /// Unregister uses this as a pre-Draining guard. Once generated Draining
    /// is visible, attaching a missing executor publisher is forbidden, so
    /// discovering this requirement only during terminalization would create
    /// an unrecoverable teardown trap.
    pub(crate) fn active_inputs_require_terminal_publication(
        &self,
    ) -> Result<bool, RuntimeDriverError> {
        for input_id in self.as_driver().active_input_ids() {
            let stored = self
                .as_driver()
                .stored_input_state(&input_id)
                .ok_or_else(|| {
                    RuntimeDriverError::Internal(format!(
                        "active input {input_id} disappeared while checking terminal publication capability"
                    ))
                })?;
            if let Some(input) = stored.state.persisted_input.as_ref()
                && let Some(interaction_id) = crate::input::validated_directed_interaction_id(input)
                    .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?
            {
                if input_id.0 != interaction_id.0 {
                    return Err(RuntimeDriverError::ValidationFailed {
                        reason: "directed terminal input/interaction identity mismatch".to_string(),
                    });
                }
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn existing_exact_runless_runtime_termination_batch(
        &self,
        input_ids: &[InputId],
        reason: &str,
    ) -> Result<Option<InputId>, RuntimeDriverError> {
        use crate::input_state::{
            InteractionTerminalBatchKey, InteractionTerminalCandidate,
            InteractionTerminalOutboxPhase, validate_interaction_terminal_outbox_batch_shape,
            validate_unpublished_interaction_terminal_outbox_batch,
        };

        let mut grouped = std::collections::HashMap::<
            InteractionTerminalBatchKey,
            Vec<crate::input_state::InteractionTerminalOutbox>,
        >::new();
        for outbox in self
            .as_driver()
            .stored_input_states_snapshot()?
            .into_iter()
            .filter_map(|stored| stored.state.interaction_terminal_outbox)
        {
            if matches!(
                outbox.batch_key,
                InteractionTerminalBatchKey::RuntimeTermination { .. }
            ) {
                grouped
                    .entry(outbox.batch_key.clone())
                    .or_default()
                    .push(outbox);
            }
        }

        let requested = input_ids.iter().collect::<std::collections::HashSet<_>>();
        let mut exact_owner = None;
        for mut outboxes in grouped.into_values() {
            outboxes.sort_by_key(|outbox| outbox.batch_ordinal);
            validate_interaction_terminal_outbox_batch_shape(&outboxes)
                .map_err(|reason| RuntimeDriverError::RecoveryCorruption { reason })?;
            let published = outboxes
                .iter()
                .filter(|outbox| {
                    matches!(
                        outbox.phase,
                        InteractionTerminalOutboxPhase::Published { .. }
                    )
                })
                .count();
            if published == outboxes.len() {
                continue;
            }
            if published != 0 {
                return Err(RuntimeDriverError::RecoveryCorruption {
                    reason: "pending runless terminal lookup found a partially published batch"
                        .to_string(),
                });
            }
            let persisted_completion_input_ids =
                validate_unpublished_interaction_terminal_outbox_batch(&outboxes)
                    .map_err(|reason| RuntimeDriverError::RecoveryCorruption { reason })?;
            let owner = &outboxes[0];
            let exact_reason = matches!(
                owner.candidate.as_ref(),
                Some(InteractionTerminalCandidate::RuntimeTerminated {
                    reason: existing_reason,
                }) if existing_reason == reason
            );
            let exact_recipients = persisted_completion_input_ids == input_ids;
            let overlaps_requested = persisted_completion_input_ids
                .iter()
                .any(|input_id| requested.contains(input_id));

            if exact_recipients && exact_reason {
                if exact_owner
                    .replace(owner.candidate_owner_input_id.clone())
                    .is_some()
                {
                    return Err(RuntimeDriverError::RecoveryCorruption {
                        reason: "multiple pending runless terminal batches matched one exact termination"
                            .to_string(),
                    });
                }
            } else if overlaps_requested {
                return Err(RuntimeDriverError::ValidationFailed {
                    reason: "pending runless terminal batch overlaps the requested termination without matching it exactly"
                        .to_string(),
                });
            }
        }
        Ok(exact_owner)
    }

    pub(crate) fn commit_prepared_runless_interaction_terminal_outboxes(
        prepared: PreparedRunlessInteractionTerminalOutboxes,
    ) -> Option<InputId> {
        let PreparedRunlessInteractionTerminalOutboxes {
            checkpoint: _,
            candidate_owner_input_id,
        } = prepared;
        candidate_owner_input_id
    }

    pub(crate) fn rollback_prepared_runless_interaction_terminal_outboxes(
        &mut self,
        prepared: PreparedRunlessInteractionTerminalOutboxes,
    ) {
        if let Some(checkpoint) = prepared.checkpoint {
            self.restore_rollback_snapshot(checkpoint);
        }
    }

    pub(crate) fn interaction_terminal_batch_for_owner(
        &self,
        candidate_owner_input_id: &InputId,
    ) -> Result<(Vec<InputId>, Vec<meerkat_core::interaction::InteractionId>), RuntimeDriverError>
    {
        let mut outboxes = self
            .as_driver()
            .stored_input_states_snapshot()?
            .into_iter()
            .filter_map(|stored| stored.state.interaction_terminal_outbox)
            .filter(|outbox| &outbox.candidate_owner_input_id == candidate_owner_input_id)
            .collect::<Vec<_>>();
        if outboxes.is_empty() || outboxes.len() > 256 {
            return Err(RuntimeDriverError::ValidationFailed {
                reason: format!(
                    "interaction terminal owner {candidate_owner_input_id} names invalid batch size {}",
                    outboxes.len()
                ),
            });
        }
        outboxes.sort_by_key(|outbox| outbox.batch_ordinal);
        let completion_input_ids =
            crate::input_state::validate_unpublished_interaction_terminal_outbox_batch(&outboxes)
                .map_err(|reason| RuntimeDriverError::RecoveryCorruption { reason })?;
        let mut interaction_ids = std::collections::BTreeSet::new();
        for outbox in &outboxes {
            self.validate_interaction_outbox_owner_binding(outbox)?;
            if outbox.candidate_owner_input_id != *candidate_owner_input_id
                || !matches!(
                    outbox.phase,
                    crate::input_state::InteractionTerminalOutboxPhase::Candidate
                        | crate::input_state::InteractionTerminalOutboxPhase::Finalized { .. }
                )
            {
                return Err(RuntimeDriverError::ValidationFailed {
                    reason: format!(
                        "interaction terminal owner {candidate_owner_input_id} does not name an unpublished runtime-termination batch"
                    ),
                });
            }
            if !interaction_ids.insert(outbox.interaction_id.0) {
                return Err(RuntimeDriverError::RecoveryCorruption {
                    reason: "interaction terminal batch repeats input/interaction identity"
                        .to_string(),
                });
            }
        }
        Ok((
            completion_input_ids,
            outboxes
                .iter()
                .map(|outbox| outbox.interaction_id)
                .collect(),
        ))
    }

    /// Rebind one exact unpublished terminal batch to the driver's current
    /// generated runtime owner before an immediate control-plane publication.
    /// The realization is exact-CAS fenced, so a concurrent higher owner wins
    /// without allowing this driver to overwrite it.
    pub(crate) async fn adopt_interaction_terminal_batch_for_owner(
        &mut self,
        candidate_owner_input_id: &InputId,
    ) -> Result<(), RuntimeDriverError> {
        let mut outboxes = self
            .as_driver()
            .stored_input_states_snapshot()?
            .into_iter()
            .filter_map(|stored| stored.state.interaction_terminal_outbox)
            .filter(|outbox| &outbox.candidate_owner_input_id == candidate_owner_input_id)
            .collect::<Vec<_>>();
        outboxes.sort_by_key(|outbox| outbox.batch_ordinal);
        crate::input_state::validate_unpublished_interaction_terminal_outbox_batch(&outboxes)
            .map_err(|reason| RuntimeDriverError::RecoveryCorruption { reason })?;
        let adoption = self.authorize_interaction_terminal_outbox_adoption(&outboxes)?;
        self.realize_interaction_terminal_outbox_adoption(adoption)
            .await
    }

    fn interaction_outbox_owner_binding_matches_current(
        &self,
        outbox: &crate::input_state::InteractionTerminalOutbox,
    ) -> bool {
        let authority = self.shared_dsl_authority();
        let authority = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let state = authority.state();
        let owner_session_id = outbox.owner_session_id.to_string();
        state.session_id.as_ref().map(|value| value.0.as_str()) == Some(owner_session_id.as_str())
            && state
                .active_runtime_id
                .as_ref()
                .map(|value| value.0.as_str())
                == outbox.owner_agent_runtime_id.as_deref()
            && state.active_fence_token.map(|value| value.0) == outbox.owner_fence_token
            && state.active_runtime_generation.map(|value| value.0)
                == outbox.owner_runtime_generation
            && state
                .active_runtime_epoch_id
                .as_ref()
                .map(|value| value.0.as_str())
                == outbox.owner_runtime_epoch_id.as_deref()
    }

    fn validate_interaction_outbox_owner_binding(
        &self,
        outbox: &crate::input_state::InteractionTerminalOutbox,
    ) -> Result<(), RuntimeDriverError> {
        if !self.interaction_outbox_owner_binding_matches_current(outbox) {
            return Err(RuntimeDriverError::ValidationFailed {
                reason: format!(
                    "interaction terminal outbox owner binding no longer matches runtime authority for input {}",
                    outbox.input_id
                ),
            });
        }
        Ok(())
    }

    fn authorize_interaction_terminal_outbox_adoption(
        &self,
        outboxes: &[crate::input_state::InteractionTerminalOutbox],
    ) -> Result<AuthorizedInteractionTerminalOutboxAdoption, RuntimeDriverError> {
        let first = outboxes.first().ok_or_else(|| {
            RuntimeDriverError::Internal(
                "cannot authorize an empty interaction terminal outbox adoption".to_string(),
            )
        })?;
        let previous = InteractionTerminalOwnerBinding::from_outbox(first);
        if outboxes.iter().any(|outbox| {
            outbox.batch_key != first.batch_key
                || outbox.candidate_digest != first.candidate_digest
                || InteractionTerminalOwnerBinding::from_outbox(outbox) != previous
        }) {
            return Err(RuntimeDriverError::ValidationFailed {
                reason: "interaction terminal recovery batch has split owner lineage or candidate digest"
                    .to_string(),
            });
        }
        let previous_agent_runtime_id = previous.agent_runtime_id.as_ref().ok_or_else(|| {
            RuntimeDriverError::ValidationFailed {
                reason: "interaction terminal recovery prior runtime id is missing".to_string(),
            }
        })?;
        let previous_fence_token =
            previous
                .fence_token
                .ok_or_else(|| RuntimeDriverError::ValidationFailed {
                    reason: "interaction terminal recovery prior fence is missing".to_string(),
                })?;
        let previous_runtime_generation =
            previous
                .runtime_generation
                .ok_or_else(|| RuntimeDriverError::ValidationFailed {
                    reason: "interaction terminal recovery prior generation is missing".to_string(),
                })?;
        let batch_key = serde_json::to_string(&first.batch_key).map_err(|error| {
            RuntimeDriverError::Internal(format!(
                "interaction terminal batch key failed to encode: {error}"
            ))
        })?;
        let input = crate::meerkat_machine::dsl::MeerkatMachineInput::AuthorizeInteractionTerminalOutboxAdoption {
            batch_key: batch_key.clone(),
            candidate_digest: first.candidate_digest.clone(),
            session_id: crate::meerkat_machine::dsl::SessionId::from_domain(&previous.session_id),
            previous_agent_runtime_id: crate::meerkat_machine::dsl::AgentRuntimeId::from(previous_agent_runtime_id.clone()),
            previous_fence_token: crate::meerkat_machine::dsl::FenceToken::from(previous_fence_token),
            previous_runtime_generation: crate::meerkat_machine::dsl::Generation::from(previous_runtime_generation),
            previous_runtime_epoch_id: previous.runtime_epoch_id.clone().map(crate::meerkat_machine::dsl::RuntimeEpochId::from),
        };
        let authority = self.shared_dsl_authority();
        let state = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .state()
            .clone();
        let mut preview =
            crate::meerkat_machine::dsl::MeerkatMachineAuthority::recover_from_state(state)
                .map_err(|error| {
                    RuntimeDriverError::Internal(crate::meerkat_machine::dsl_authority::map_error(
                        error,
                        "AuthorizeInteractionTerminalOutboxAdoption",
                    ))
                })?;
        let effects =
            crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(&mut preview, input)
                .map(|transition| transition.into_effects())
                .map_err(|error| RuntimeDriverError::ValidationFailed {
                    reason: crate::meerkat_machine::dsl_authority::map_error(
                        error,
                        "AuthorizeInteractionTerminalOutboxAdoption",
                    ),
                })?;
        let mut next = None;
        for effect in effects {
            let crate::meerkat_machine::dsl::MeerkatMachineEffect::InteractionTerminalOutboxAdoptionAuthorized {
                batch_key: emitted_batch_key,
                candidate_digest,
                session_id,
                previous_agent_runtime_id: emitted_previous_runtime_id,
                previous_fence_token: emitted_previous_fence,
                previous_runtime_generation: emitted_previous_generation,
                previous_runtime_epoch_id: emitted_previous_epoch,
                next_agent_runtime_id,
                next_fence_token,
                next_runtime_generation,
                next_runtime_epoch_id,
            } = effect
            else {
                continue;
            };
            if emitted_batch_key != batch_key
                || candidate_digest != first.candidate_digest
                || session_id.0 != previous.session_id.to_string()
                || emitted_previous_runtime_id.0 != *previous_agent_runtime_id
                || emitted_previous_fence.0 != previous_fence_token
                || emitted_previous_generation.0 != previous_runtime_generation
                || emitted_previous_epoch
                    .as_ref()
                    .map(|value| value.0.as_str())
                    != previous.runtime_epoch_id.as_deref()
            {
                return Err(RuntimeDriverError::Internal(
                    "generated interaction terminal adoption effect changed prior witnesses"
                        .to_string(),
                ));
            }
            let emitted_next = InteractionTerminalOwnerBinding {
                session_id: previous.session_id.clone(),
                agent_runtime_id: Some(next_agent_runtime_id.0),
                fence_token: Some(next_fence_token.0),
                runtime_generation: Some(next_runtime_generation.0),
                runtime_epoch_id: next_runtime_epoch_id.map(|value| value.0),
            };
            if next.replace(emitted_next).is_some() {
                return Err(RuntimeDriverError::Internal(
                    "generated interaction terminal adoption emitted multiple authorities"
                        .to_string(),
                ));
            }
        }
        let next = next.ok_or_else(|| {
            RuntimeDriverError::Internal(
                "generated interaction terminal adoption emitted no authority".to_string(),
            )
        })?;
        Ok(AuthorizedInteractionTerminalOutboxAdoption {
            generated_plan: generated_kernel_command_capabilities::CommandPlanKind::AuthorizedInteractionTerminalOutboxAdoption,
            batch_key: first.batch_key.clone(),
            input_ids: outboxes
                .iter()
                .map(|outbox| outbox.input_id.clone())
                .collect(),
            candidate_digest: first.candidate_digest.clone(),
            previous,
            next,
        })
    }

    async fn realize_interaction_terminal_outbox_adoption(
        &mut self,
        adoption: AuthorizedInteractionTerminalOutboxAdoption,
    ) -> Result<(), RuntimeDriverError> {
        debug_assert_eq!(
            adoption.generated_plan,
            generated_kernel_command_capabilities::CommandPlanKind::AuthorizedInteractionTerminalOutboxAdoption,
        );
        let expected = adoption
            .input_ids
            .iter()
            .map(|input_id| {
                self.as_driver().stored_input_state(input_id).ok_or_else(|| {
                    RuntimeDriverError::Internal(format!(
                        "interaction terminal adoption input {input_id} disappeared before CAS expectation"
                    ))
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let replacements = expected
            .iter()
            .cloned()
            .map(|mut replacement| {
                let input_id = replacement.state.input_id.clone();
                let outbox = replacement
                    .state
                    .interaction_terminal_outbox
                    .as_mut()
                    .ok_or_else(|| {
                        RuntimeDriverError::Internal(format!(
                            "interaction terminal adoption input {input_id} lost its outbox"
                        ))
                    })?;
                if outbox.batch_key != adoption.batch_key
                    || outbox.candidate_digest != adoption.candidate_digest
                    || InteractionTerminalOwnerBinding::from_outbox(outbox) != adoption.previous
                {
                    return Err(RuntimeDriverError::ValidationFailed {
                        reason: format!(
                            "interaction terminal adoption authority became stale for input {input_id}"
                        ),
                    });
                }
                adoption.next.write_to(outbox);
                outbox.validate().map_err(RuntimeDriverError::Internal)?;
                crate::input_state::InputStatePersistenceRecord::from_machine_snapshot(replacement)
                    .map_err(RuntimeDriverError::Internal)
            })
            .collect::<Result<Vec<_>, _>>()?;
        self.commit_interaction_terminal_outbox_replacements(
            &expected,
            replacements,
            "interaction terminal outbox adoption lost the exact durable CAS",
        )
        .await
    }

    fn interaction_terminal_outbox_phase_rank(
        outbox: &crate::input_state::InteractionTerminalOutbox,
    ) -> u8 {
        match &outbox.phase {
            crate::input_state::InteractionTerminalOutboxPhase::Candidate => 0,
            crate::input_state::InteractionTerminalOutboxPhase::Finalized { .. } => 1,
            crate::input_state::InteractionTerminalOutboxPhase::Published { .. } => 2,
        }
    }

    fn interaction_terminal_outbox_identity_matches(
        left: &crate::input_state::InteractionTerminalOutbox,
        right: &crate::input_state::InteractionTerminalOutbox,
    ) -> bool {
        left.interaction_id == right.interaction_id
            && left.input_id == right.input_id
            && left.batch_ordinal == right.batch_ordinal
            && left.batch_key == right.batch_key
            && InteractionTerminalOwnerBinding::from_outbox(left)
                == InteractionTerminalOwnerBinding::from_outbox(right)
            && left.candidate_owner_input_id == right.candidate_owner_input_id
            && left.candidate_digest == right.candidate_digest
            && left.completion_input_ids_digest == right.completion_input_ids_digest
    }

    fn interaction_terminal_outbox_is_exact_phase_successor(
        shell: &crate::input_state::InteractionTerminalOutbox,
        durable: &crate::input_state::InteractionTerminalOutbox,
    ) -> Result<bool, RuntimeDriverError> {
        use crate::input_state::InteractionTerminalOutboxPhase;

        let valid_transition = match (&shell.phase, &durable.phase) {
            (
                InteractionTerminalOutboxPhase::Candidate,
                InteractionTerminalOutboxPhase::Finalized { .. },
            ) => true,
            (
                InteractionTerminalOutboxPhase::Finalized {
                    finalization_failed: shell_failed,
                    finalized_payload_digest,
                    ..
                },
                InteractionTerminalOutboxPhase::Published {
                    finalization_failed: durable_failed,
                    publication,
                },
            ) => {
                shell_failed == durable_failed
                    && finalized_payload_digest == &publication.payload_digest
            }
            _ => false,
        };
        if !valid_transition {
            return Ok(false);
        }

        // Reconstruct the only image the corresponding store-first operation
        // is allowed to write. Candidate -> Finalized changes only the phase;
        // Finalized -> Published additionally compacts the owner payload and
        // completion recipients. Exact JSON equality then rejects every other
        // field mutation, including a forged receipt/class or torn compaction.
        let mut expected = shell.clone();
        if matches!(
            &durable.phase,
            InteractionTerminalOutboxPhase::Published { .. }
        ) {
            expected.candidate = None;
            expected.completion_input_ids = None;
        }
        expected.phase = durable.phase.clone();
        Ok(serde_json::to_vec(&expected)
            .map_err(|error| RuntimeDriverError::Internal(error.to_string()))?
            == serde_json::to_vec(durable)
                .map_err(|error| RuntimeDriverError::Internal(error.to_string()))?)
    }

    fn validate_finalized_interaction_terminal_batch(
        outboxes: &[crate::input_state::InteractionTerminalOutbox],
    ) -> Result<(bool, Vec<meerkat_core::event::AgentEvent>), String> {
        use crate::input_state::InteractionTerminalOutboxPhase;

        let owner = outboxes
            .first()
            .ok_or_else(|| "finalized interaction terminal batch is empty".to_string())?;
        let InteractionTerminalOutboxPhase::Finalized {
            finalization_failed,
            finalized_event: Some(owner_event),
            ..
        } = &owner.phase
        else {
            return Err("finalized interaction terminal owner lost its event".to_string());
        };
        let mut events = Vec::with_capacity(outboxes.len());
        for outbox in outboxes {
            let InteractionTerminalOutboxPhase::Finalized {
                finalization_failed: row_failed,
                finalized_payload_digest,
                ..
            } = &outbox.phase
            else {
                return Err("interaction terminal batch has mixed finalization phases".to_string());
            };
            if row_failed != finalization_failed {
                return Err("interaction terminal finalization class split".to_string());
            }
            let event = crate::input_state::interaction_terminal_event_for_id(
                owner_event,
                outbox.interaction_id,
            )
            .ok_or_else(|| {
                "interaction terminal finalized owner event was not terminal".to_string()
            })?;
            let digest = crate::input_state::interaction_terminal_payload_digest(&event)?;
            if &digest != finalized_payload_digest {
                return Err(format!(
                    "interaction terminal finalized digest mismatch for input {}",
                    outbox.input_id
                ));
            }
            events.push(event);
        }
        Ok((*finalization_failed, events))
    }

    /// Reconcile only a durable, uniform one-phase advance that committed
    /// before its caller lost the store acknowledgement. The store is the
    /// canonical side of a store-first terminal transition, but it may hydrate
    /// the shell only when every non-outbox byte and the exact prior owner
    /// witness still match. A machine-owner replacement may then use the
    /// ordinary generated adoption CAS; owner-only adoption acknowledgement
    /// loss is likewise left to that idempotent retry below.
    async fn reconcile_durable_interaction_terminal_phase(
        &mut self,
    ) -> Result<(), RuntimeDriverError> {
        let durable = match self {
            DriverEntry::Ephemeral(_) => return Ok(()),
            DriverEntry::Persistent(driver) => {
                driver.durable_input_states_for_terminal_recovery().await?
            }
        };
        let shell = self.as_driver().stored_input_states_snapshot()?;
        let durable_by_id = durable
            .into_iter()
            .map(|stored| (stored.state.input_id.clone(), stored))
            .collect::<std::collections::HashMap<_, _>>();
        let shell_outbox_ids = shell
            .iter()
            .filter(|stored| stored.state.interaction_terminal_outbox.is_some())
            .map(|stored| stored.state.input_id.clone())
            .collect::<std::collections::HashSet<_>>();
        for (input_id, stored) in &durable_by_id {
            if stored.state.interaction_terminal_outbox.is_some()
                && !shell_outbox_ids.contains(input_id)
            {
                return Err(RuntimeDriverError::RecoveryCorruption {
                    reason: format!(
                        "durable interaction terminal carrier {input_id} is absent from the live shell"
                    ),
                });
            }
        }

        let mut prospective = shell
            .iter()
            .cloned()
            .map(|stored| (stored.state.input_id.clone(), stored))
            .collect::<std::collections::HashMap<_, _>>();
        let mut replacements = Vec::new();
        for shell_stored in &shell {
            let Some(shell_outbox) = shell_stored.state.interaction_terminal_outbox.as_ref() else {
                continue;
            };
            let input_id = &shell_stored.state.input_id;
            let durable_stored = durable_by_id.get(input_id).ok_or_else(|| {
                RuntimeDriverError::RecoveryCorruption {
                    reason: format!(
                        "live interaction terminal carrier {input_id} is absent from durable state"
                    ),
                }
            })?;
            let durable_outbox = durable_stored
                .state
                .interaction_terminal_outbox
                .as_ref()
                .ok_or_else(|| RuntimeDriverError::RecoveryCorruption {
                    reason: format!(
                        "live interaction terminal carrier {input_id} lost its durable outbox"
                    ),
                })?;
            shell_outbox
                .validate()
                .map_err(|reason| RuntimeDriverError::RecoveryCorruption { reason })?;
            durable_outbox
                .validate()
                .map_err(|reason| RuntimeDriverError::RecoveryCorruption { reason })?;

            let shell_json = serde_json::to_vec(shell_stored)
                .map_err(|error| RuntimeDriverError::Internal(error.to_string()))?;
            let durable_json = serde_json::to_vec(durable_stored)
                .map_err(|error| RuntimeDriverError::Internal(error.to_string()))?;
            if shell_json == durable_json {
                continue;
            }

            let mut shell_without_outbox = shell_stored.clone();
            shell_without_outbox.state.interaction_terminal_outbox = None;
            let mut durable_without_outbox = durable_stored.clone();
            durable_without_outbox.state.interaction_terminal_outbox = None;
            if serde_json::to_vec(&shell_without_outbox)
                .map_err(|error| RuntimeDriverError::Internal(error.to_string()))?
                != serde_json::to_vec(&durable_without_outbox)
                    .map_err(|error| RuntimeDriverError::Internal(error.to_string()))?
            {
                return Err(RuntimeDriverError::StaleAuthority {
                    reason: format!(
                        "interaction terminal recovery found unrelated durable divergence for input {input_id}"
                    ),
                });
            }

            let shell_rank = Self::interaction_terminal_outbox_phase_rank(shell_outbox);
            let durable_rank = Self::interaction_terminal_outbox_phase_rank(durable_outbox);
            if shell_rank == durable_rank {
                let mut owner_normalized = durable_outbox.clone();
                InteractionTerminalOwnerBinding::from_outbox(shell_outbox)
                    .write_to(&mut owner_normalized);
                let owner_only_difference = serde_json::to_vec(shell_outbox)
                    .map_err(|error| RuntimeDriverError::Internal(error.to_string()))?
                    == serde_json::to_vec(&owner_normalized)
                        .map_err(|error| RuntimeDriverError::Internal(error.to_string()))?;
                if owner_only_difference
                    && !self.interaction_outbox_owner_binding_matches_current(shell_outbox)
                    && self.interaction_outbox_owner_binding_matches_current(durable_outbox)
                {
                    // Adoption committed durably and lost its acknowledgement.
                    // Keep the old shell expectation so the exact idempotent
                    // adoption CAS can acknowledge and hydrate it below.
                    continue;
                }
                return Err(RuntimeDriverError::StaleAuthority {
                    reason: format!(
                        "interaction terminal recovery found same-phase noncanonical divergence for input {input_id}"
                    ),
                });
            }

            if durable_rank != shell_rank + 1
                || !Self::interaction_terminal_outbox_identity_matches(shell_outbox, durable_outbox)
                || !Self::interaction_terminal_outbox_is_exact_phase_successor(
                    shell_outbox,
                    durable_outbox,
                )?
            {
                return Err(RuntimeDriverError::StaleAuthority {
                    reason: format!(
                        "interaction terminal recovery rejected a noncanonical durable phase successor for input {input_id}"
                    ),
                });
            }
            prospective.insert(input_id.clone(), durable_stored.clone());
            replacements.push(durable_stored.clone());
        }

        // Validate the complete prospective image before publishing any row
        // into the shell. An atomic backend should never expose a mixed batch;
        // treating one as corruption prevents row-order-dependent hydration.
        let mut grouped = std::collections::HashMap::<
            crate::input_state::InteractionTerminalBatchKey,
            Vec<crate::input_state::InteractionTerminalOutbox>,
        >::new();
        for outbox in prospective
            .into_values()
            .filter_map(|stored| stored.state.interaction_terminal_outbox)
        {
            grouped
                .entry(outbox.batch_key.clone())
                .or_default()
                .push(outbox);
        }
        for mut outboxes in grouped.into_values() {
            outboxes.sort_by_key(|outbox| outbox.batch_ordinal);
            crate::input_state::validate_interaction_terminal_outbox_batch_shape(&outboxes)
                .map_err(|reason| RuntimeDriverError::RecoveryCorruption { reason })?;
            let phase_rank = Self::interaction_terminal_outbox_phase_rank(&outboxes[0]);
            if outboxes
                .iter()
                .any(|outbox| Self::interaction_terminal_outbox_phase_rank(outbox) != phase_rank)
            {
                return Err(RuntimeDriverError::RecoveryCorruption {
                    reason: "durable interaction terminal reconciliation found a mixed-phase batch"
                        .to_string(),
                });
            }
            if phase_rank != 2 {
                crate::input_state::validate_unpublished_interaction_terminal_outbox_batch(
                    &outboxes,
                )
                .map_err(|reason| RuntimeDriverError::RecoveryCorruption { reason })?;
            }
            if phase_rank == 1 {
                Self::validate_finalized_interaction_terminal_batch(&outboxes)
                    .map_err(|reason| RuntimeDriverError::RecoveryCorruption { reason })?;
            }
        }

        let ledger = self.shell_driver_mut().ledger_mut();
        for replacement in replacements {
            ledger.recover(replacement.state);
        }
        Ok(())
    }

    pub(crate) async fn interaction_terminal_recovery_batches(
        &mut self,
    ) -> Result<Vec<InteractionTerminalRecoveryBatch>, RuntimeDriverError> {
        use crate::input_state::InteractionTerminalOutboxPhase;
        struct ValidatedRecoveryBatch {
            adoption: AuthorizedInteractionTerminalOutboxAdoption,
            batch: InteractionTerminalRecoveryBatch,
        }

        self.reconcile_durable_interaction_terminal_phase().await?;

        let mut grouped = std::collections::HashMap::<
            crate::input_state::InteractionTerminalBatchKey,
            Vec<crate::input_state::InteractionTerminalOutbox>,
        >::new();
        for stored in self.as_driver().stored_input_states_snapshot()? {
            if let Some(outbox) = stored.state.interaction_terminal_outbox {
                outbox
                    .validate()
                    .map_err(|reason| RuntimeDriverError::RecoveryCorruption { reason })?;
                grouped
                    .entry(outbox.batch_key.clone())
                    .or_default()
                    .push(outbox);
            }
        }

        // Validate the complete durable recovery image before mutating any
        // owner binding. Runtime-completion correlation is single-valued, so
        // two unpublished run-scoped batches for different runs are durable
        // corruption, not an iteration-order choice. Runtime-termination
        // batches are runless and remain independent.
        let mut recovered_run_id: Option<RunId> = None;
        let mut validated_batches = Vec::new();
        for (batch_key, mut outboxes) in grouped {
            outboxes.sort_by_key(|outbox| outbox.batch_ordinal);
            crate::input_state::validate_interaction_terminal_outbox_batch_shape(&outboxes)
                .map_err(|reason| RuntimeDriverError::RecoveryCorruption { reason })?;
            let published = outboxes
                .iter()
                .filter(|outbox| {
                    matches!(
                        outbox.phase,
                        InteractionTerminalOutboxPhase::Published { .. }
                    )
                })
                .count();
            if published == outboxes.len() {
                continue;
            }
            if published != 0 {
                return Err(RuntimeDriverError::ValidationFailed {
                    reason: "interaction terminal recovery found a partially published batch"
                        .to_string(),
                });
            }
            let completion_input_ids =
                crate::input_state::validate_unpublished_interaction_terminal_outbox_batch(
                    &outboxes,
                )
                .map_err(|reason| RuntimeDriverError::RecoveryCorruption { reason })?;
            let first = outboxes.first().ok_or_else(|| {
                RuntimeDriverError::Internal(
                    "interaction terminal recovery produced an empty batch".to_string(),
                )
            })?;
            let candidate_owner_input_id = first.candidate_owner_input_id.clone();
            if outboxes.iter().any(|outbox| {
                outbox.candidate_owner_input_id != candidate_owner_input_id
                    || outbox.candidate_digest != first.candidate_digest
                    || outbox.batch_key != batch_key
            }) {
                return Err(RuntimeDriverError::ValidationFailed {
                    reason: "interaction terminal recovery found split batch identity".to_string(),
                });
            }
            let owner_rows: Vec<_> = outboxes
                .iter()
                .filter(|outbox| outbox.input_id == candidate_owner_input_id)
                .collect();
            if owner_rows.len() != 1 {
                return Err(RuntimeDriverError::ValidationFailed {
                    reason: "interaction terminal recovery batch has no unique candidate owner"
                        .to_string(),
                });
            }
            let owner = owner_rows[0];
            let candidate =
                owner
                    .candidate
                    .as_ref()
                    .ok_or_else(|| RuntimeDriverError::ValidationFailed {
                        reason: "interaction terminal recovery candidate owner lost its payload"
                            .to_string(),
                    })?;
            let adoption = self.authorize_interaction_terminal_outbox_adoption(&outboxes)?;
            if let Some(run_id) = batch_key.run_id() {
                if let Some(recovered) = recovered_run_id.as_ref() {
                    if recovered != run_id {
                        return Err(RuntimeDriverError::RecoveryCorruption {
                            reason: format!(
                                "interaction terminal recovery found unpublished batches for distinct runs {recovered} and {run_id}"
                            ),
                        });
                    }
                } else {
                    recovered_run_id = Some(run_id.clone());
                }
            }

            let input_ids = completion_input_ids;
            let interaction_ids: Vec<_> = outboxes
                .iter()
                .map(|outbox| outbox.interaction_id)
                .collect();
            let phase = if outboxes
                .iter()
                .all(|outbox| matches!(outbox.phase, InteractionTerminalOutboxPhase::Candidate))
            {
                InteractionTerminalRecoveryPhase::Candidate
            } else if outboxes.iter().all(|outbox| {
                matches!(
                    outbox.phase,
                    InteractionTerminalOutboxPhase::Finalized { .. }
                )
            }) {
                let (finalization_failed, events) =
                    Self::validate_finalized_interaction_terminal_batch(&outboxes)
                        .map_err(|reason| RuntimeDriverError::RecoveryCorruption { reason })?;
                let finalization_error = if finalization_failed {
                    let detail = match events.first() {
                        Some(meerkat_core::event::AgentEvent::InteractionFailed {
                            reason:
                                meerkat_core::event::InteractionFailureReason::FinalizationFailed {
                                    detail,
                                }
                                | meerkat_core::event::InteractionFailureReason::Abandoned { detail },
                            ..
                        }) => detail.clone(),
                        _ => {
                            return Err(RuntimeDriverError::ValidationFailed {
                                reason: "failed terminal recovery carried no typed failure detail"
                                    .to_string(),
                            });
                        }
                    };
                    Some(meerkat_core::TurnErrorMetadata::runtime_apply_failure(
                        detail,
                    ))
                } else {
                    None
                };
                InteractionTerminalRecoveryPhase::Finalized {
                    events,
                    finalization: if finalization_failed {
                        crate::meerkat_machine::dsl::RuntimeCompletionFinalizationObservation::Failed
                    } else {
                        crate::meerkat_machine::dsl::RuntimeCompletionFinalizationObservation::Succeeded
                    },
                    finalization_error,
                }
            } else {
                return Err(RuntimeDriverError::ValidationFailed {
                    reason: "interaction terminal recovery found a candidate/finalized split batch"
                        .to_string(),
                });
            };
            validated_batches.push(ValidatedRecoveryBatch {
                adoption,
                batch: InteractionTerminalRecoveryBatch {
                    batch_key,
                    input_ids,
                    interaction_ids,
                    terminal: candidate.core_apply_terminal(),
                    terminal_observation: candidate.terminal_observation(),
                    runtime_termination_reason: match candidate {
                        crate::input_state::InteractionTerminalCandidate::RuntimeTerminated {
                            reason,
                        } => Some(reason.clone()),
                        _ => None,
                    },
                    completion_error_metadata: candidate.completion_error_metadata(),
                    phase,
                },
            });
        }

        if let Some(run_id) = recovered_run_id.as_ref() {
            machine_validate_runtime_completion_result_correlation_recovery(self, run_id)?;
        }

        let mut batches = Vec::with_capacity(validated_batches.len());
        for validated in validated_batches {
            self.realize_interaction_terminal_outbox_adoption(validated.adoption)
                .await?;
            batches.push(validated.batch);
        }
        Ok(batches)
    }

    pub(crate) async fn committed_session_snapshot_for_terminal_recovery(
        &self,
    ) -> Result<Option<Vec<u8>>, RuntimeDriverError> {
        match self {
            DriverEntry::Ephemeral(_) => Ok(None),
            DriverEntry::Persistent(driver) => {
                driver
                    .committed_session_snapshot_for_terminal_recovery()
                    .await
            }
        }
    }

    async fn commit_interaction_terminal_outbox_replacements(
        &mut self,
        expected: &[crate::input_state::StoredInputState],
        replacements: Vec<crate::input_state::InputStatePersistenceRecord>,
        stale_reason: &'static str,
    ) -> Result<(), RuntimeDriverError> {
        let outcome = match self {
            DriverEntry::Ephemeral(_) => crate::store::InputStateBatchCasOutcome::Swapped,
            DriverEntry::Persistent(driver) => {
                driver
                    .compare_and_swap_interaction_terminal_outbox_replacements(
                        expected,
                        &replacements,
                    )
                    .await?
            }
        };
        if outcome == crate::store::InputStateBatchCasOutcome::Stale {
            return Err(RuntimeDriverError::StaleAuthority {
                reason: stale_reason.to_string(),
            });
        }

        // The durable image is committed (or was already byte-identical) now.
        // Publish it into the shell without another await. If the caller was
        // cancelled while a store implementation completed the CAS after its
        // future was dropped, the idempotent retry reaches this same point.
        let ledger = self.shell_driver_mut().ledger_mut();
        for replacement in replacements {
            ledger.recover(replacement.into_stored().state);
        }
        Ok(())
    }

    pub(crate) async fn finalize_interaction_terminal_outboxes(
        &mut self,
        candidate_owner_input_id: &InputId,
        events: &[meerkat_core::event::AgentEvent],
        finalization: crate::meerkat_machine::dsl::RuntimeCompletionFinalizationObservation,
    ) -> Result<(), RuntimeDriverError> {
        let mut by_id = std::collections::BTreeMap::new();
        for event in events {
            let interaction_id = crate::input_state::interaction_terminal_event_id(event)
                .ok_or_else(|| RuntimeDriverError::ValidationFailed {
                    reason: "runtime completion authority emitted a non-interaction terminal"
                        .to_string(),
                })?;
            if by_id.insert(interaction_id.0, event).is_some() {
                return Err(RuntimeDriverError::ValidationFailed {
                    reason: format!(
                        "runtime completion authority emitted duplicate interaction terminal {interaction_id}"
                    ),
                });
            }
        }

        let expected: Vec<_> = self
            .as_driver()
            .stored_input_states_snapshot()?
            .into_iter()
            .filter(|stored| {
                stored
                    .state
                    .interaction_terminal_outbox
                    .as_ref()
                    .is_some_and(|outbox| {
                        &outbox.candidate_owner_input_id == candidate_owner_input_id
                    })
            })
            .collect();
        let outboxes: Vec<_> = expected
            .iter()
            .filter_map(|stored| stored.state.interaction_terminal_outbox.clone())
            .collect();
        let expected_ids: std::collections::BTreeSet<_> = outboxes
            .iter()
            .map(|outbox| outbox.interaction_id.0)
            .collect();
        let actual_ids: std::collections::BTreeSet<_> = by_id.keys().copied().collect();
        if expected_ids != actual_ids {
            return Err(RuntimeDriverError::ValidationFailed {
                reason: format!(
                    "generated completion batch did not exactly match directed outboxes: expected={expected_ids:?}, actual={actual_ids:?}"
                ),
            });
        }
        let candidate_owner_input_id = outboxes
            .first()
            .map(|outbox| outbox.candidate_owner_input_id.clone())
            .ok_or_else(|| RuntimeDriverError::ValidationFailed {
                reason:
                    "generated completion emitted directed terminals without committed outboxes"
                        .to_string(),
            })?;
        let candidate_owner_count = outboxes
            .iter()
            .filter(|outbox| outbox.input_id == candidate_owner_input_id)
            .count();
        if candidate_owner_count != 1 {
            return Err(RuntimeDriverError::ValidationFailed {
                reason: format!(
                    "directed terminal outbox batch has {candidate_owner_count} rows for shared candidate owner {candidate_owner_input_id}"
                ),
            });
        }
        let candidate_owner = outboxes
            .iter()
            .find(|outbox| outbox.input_id == candidate_owner_input_id)
            .ok_or_else(|| {
                RuntimeDriverError::Internal(
                    "directed terminal outbox shared candidate owner disappeared".to_string(),
                )
            })?;
        let candidate = candidate_owner.candidate.as_ref().ok_or_else(|| {
            RuntimeDriverError::ValidationFailed {
                reason: "directed terminal outbox shared candidate owner carried no candidate"
                    .to_string(),
            }
        })?;
        let candidate_digest = candidate_owner.candidate_digest.as_str();
        let finalization_failed = finalization
            == crate::meerkat_machine::dsl::RuntimeCompletionFinalizationObservation::Failed;
        for outbox in &outboxes {
            self.validate_interaction_outbox_owner_binding(outbox)?;
            if outbox.candidate_owner_input_id != candidate_owner_input_id
                || outbox.candidate_digest != candidate_digest
            {
                return Err(RuntimeDriverError::ValidationFailed {
                    reason: format!(
                        "directed terminal outbox batch shared-candidate reference mismatch for input {}",
                        outbox.input_id
                    ),
                });
            }
            let event = by_id.get(&outbox.interaction_id.0).ok_or_else(|| {
                RuntimeDriverError::Internal(
                    "exact directed terminal set changed during validation".to_string(),
                )
            })?;
            if !crate::input_state::interaction_terminal_candidate_matches_event(
                candidate,
                outbox.interaction_id,
                event,
                finalization_failed,
            ) {
                return Err(RuntimeDriverError::ValidationFailed {
                    reason: format!(
                        "generated terminal payload did not match committed candidate for interaction {}",
                        outbox.interaction_id
                    ),
                });
            }
        }

        let replacements = expected
            .iter()
            .cloned()
            .map(|mut replacement| {
                let replacement_input_id = replacement.state.input_id.clone();
                let live = replacement
                    .state
                    .interaction_terminal_outbox
                    .as_mut()
                    .ok_or_else(|| {
                        RuntimeDriverError::Internal(format!(
                            "interaction terminal outbox input {replacement_input_id} lost its retry carrier"
                        ))
                    })?;
                let event = (*by_id.get(&live.interaction_id.0).ok_or_else(|| {
                    RuntimeDriverError::Internal(
                        "exact directed terminal set changed during finalization".to_string(),
                    )
                })?)
                .clone();
                let digest = crate::input_state::interaction_terminal_payload_digest(&event)
                    .map_err(RuntimeDriverError::Internal)?;
                let owns_candidate = live.input_id == live.candidate_owner_input_id;
                match &live.phase {
                    crate::input_state::InteractionTerminalOutboxPhase::Candidate => {
                        live.phase =
                            crate::input_state::InteractionTerminalOutboxPhase::Finalized {
                                finalization_failed,
                                finalized_event: owns_candidate.then_some(event),
                                finalized_payload_digest: digest,
                            };
                    }
                    crate::input_state::InteractionTerminalOutboxPhase::Finalized {
                        finalization_failed: existing_failed,
                        finalized_event: Some(existing_event),
                        finalized_payload_digest: existing_digest,
                    } if owns_candidate
                        && crate::input_state::interaction_terminal_payload_digest(
                            existing_event,
                        )
                        .is_ok_and(|existing_event_digest| existing_event_digest == digest)
                        && existing_digest == &digest
                        && *existing_failed == finalization_failed => {}
                    crate::input_state::InteractionTerminalOutboxPhase::Finalized {
                        finalization_failed: existing_failed,
                        finalized_event: None,
                        finalized_payload_digest: existing_digest,
                    } if !owns_candidate
                        && existing_digest == &digest
                        && *existing_failed == finalization_failed => {}
                    _ => {
                        return Err(RuntimeDriverError::ValidationFailed {
                            reason: format!(
                                "interaction terminal finalization conflict for input {}",
                                live.input_id
                            ),
                        });
                    }
                }
                live.validate().map_err(RuntimeDriverError::Internal)?;
                crate::input_state::InputStatePersistenceRecord::from_machine_snapshot(replacement)
                    .map_err(RuntimeDriverError::Internal)
            })
            .collect::<Result<Vec<_>, _>>()?;
        self.commit_interaction_terminal_outbox_replacements(
            &expected,
            replacements,
            "interaction terminal finalization lost the exact durable CAS",
        )
        .await
    }

    pub(crate) async fn mark_interaction_terminal_outboxes_published(
        &mut self,
        candidate_owner_input_id: &InputId,
        receipts: &[meerkat_core::lifecycle::core_executor::CoreInteractionTerminalPublicationReceipt],
    ) -> Result<(), RuntimeDriverError> {
        let mut by_id = std::collections::BTreeMap::new();
        for receipt in receipts {
            if by_id.insert(receipt.interaction_id().0, receipt).is_some() {
                return Err(RuntimeDriverError::ValidationFailed {
                    reason: "duplicate interaction terminal publication receipt".to_string(),
                });
            }
        }
        let expected: Vec<_> = self
            .as_driver()
            .stored_input_states_snapshot()?
            .into_iter()
            .filter(|stored| {
                stored
                    .state
                    .interaction_terminal_outbox
                    .as_ref()
                    .is_some_and(|outbox| {
                        &outbox.candidate_owner_input_id == candidate_owner_input_id
                    })
            })
            .collect();
        let outboxes: Vec<_> = expected
            .iter()
            .filter_map(|stored| stored.state.interaction_terminal_outbox.clone())
            .collect();
        let expected_ids: std::collections::BTreeSet<_> = outboxes
            .iter()
            .map(|outbox| outbox.interaction_id.0)
            .collect();
        if expected_ids != by_id.keys().copied().collect() {
            return Err(RuntimeDriverError::ValidationFailed {
                reason: "publication receipts did not exactly match finalized outbox batch"
                    .to_string(),
            });
        }
        for outbox in &outboxes {
            self.validate_interaction_outbox_owner_binding(outbox)?;
        }

        let replacements = expected
            .iter()
            .cloned()
            .map(|mut replacement| {
                let replacement_input_id = replacement.state.input_id.clone();
                let live = replacement
                    .state
                    .interaction_terminal_outbox
                    .as_mut()
                    .ok_or_else(|| {
                        RuntimeDriverError::Internal(format!(
                            "interaction terminal outbox input {replacement_input_id} disappeared before publish receipt"
                        ))
                    })?;
                let receipt = by_id.get(&live.interaction_id.0).ok_or_else(|| {
                    RuntimeDriverError::Internal(
                        "exact publication receipt set changed during persistence".to_string(),
                    )
                })?;
                let publication = crate::input_state::InteractionTerminalPublication {
                    terminal_seq: receipt.terminal_seq(),
                    payload_digest: receipt.payload_digest().to_string(),
                };
                match &live.phase {
                    crate::input_state::InteractionTerminalOutboxPhase::Finalized {
                        finalization_failed,
                        finalized_payload_digest,
                        ..
                    } if finalized_payload_digest == &publication.payload_digest => {
                        let finalization_failed = *finalization_failed;
                        live.candidate = None;
                        live.completion_input_ids = None;
                        live.phase =
                            crate::input_state::InteractionTerminalOutboxPhase::Published {
                                finalization_failed,
                                publication,
                            };
                    }
                    crate::input_state::InteractionTerminalOutboxPhase::Published {
                        publication: existing,
                        ..
                    } if existing.terminal_seq == publication.terminal_seq
                        && existing.payload_digest == publication.payload_digest => {}
                    _ => {
                        return Err(RuntimeDriverError::ValidationFailed {
                            reason: format!(
                                "interaction terminal publication receipt conflict for input {}",
                                live.input_id
                            ),
                        });
                    }
                }
                live.validate().map_err(RuntimeDriverError::Internal)?;
                crate::input_state::InputStatePersistenceRecord::from_machine_snapshot(replacement)
                    .map_err(RuntimeDriverError::Internal)
            })
            .collect::<Result<Vec<_>, _>>()?;
        self.commit_interaction_terminal_outbox_replacements(
            &expected,
            replacements,
            "interaction terminal publication receipt lost the exact durable CAS",
        )
        .await
    }

    pub(crate) fn runtime_id(&self) -> &LogicalRuntimeId {
        match self {
            DriverEntry::Ephemeral(d) => d.runtime_id(),
            DriverEntry::Persistent(d) => d.runtime_id(),
        }
    }

    pub(crate) async fn load_pending_compaction_projections(
        &self,
    ) -> Result<Vec<meerkat_core::CompactionProjectionIntent>, RuntimeDriverError> {
        match self {
            DriverEntry::Ephemeral(_) => Ok(Vec::new()),
            DriverEntry::Persistent(driver) => driver.load_pending_compaction_projections().await,
        }
    }

    pub(crate) async fn mark_compaction_projection_finalized(
        &self,
        projection: &meerkat_core::CompactionProjectionId,
    ) -> Result<(), RuntimeDriverError> {
        match self {
            DriverEntry::Ephemeral(_) => Err(RuntimeDriverError::Internal(
                "ephemeral runtime cannot finalize a durable compaction outbox".to_string(),
            )),
            DriverEntry::Persistent(driver) => {
                driver
                    .mark_compaction_projection_finalized(projection)
                    .await
            }
        }
    }

    pub(crate) async fn load_compaction_checkpoint_snapshot(
        &self,
    ) -> Result<Option<Vec<u8>>, RuntimeDriverError> {
        match self {
            DriverEntry::Ephemeral(_) => Ok(None),
            DriverEntry::Persistent(driver) => driver.load_compaction_checkpoint_snapshot().await,
        }
    }

    pub(crate) fn as_driver(&self) -> &dyn RuntimeDriver {
        match self {
            DriverEntry::Ephemeral(d) => d,
            DriverEntry::Persistent(d) => d,
        }
    }

    /// Machine-owned per-run boundary counter — the single producer of the
    /// run-boundary receipt sequence (dogma K10).
    pub(crate) fn run_boundary_sequence(&self, run_id: &RunId) -> u64 {
        match self {
            DriverEntry::Ephemeral(d) => d.run_boundary_sequence(run_id),
            DriverEntry::Persistent(d) => d.inner_ref().run_boundary_sequence(run_id),
        }
    }

    pub(crate) fn as_driver_mut(&mut self) -> &mut dyn RuntimeDriver {
        match self {
            DriverEntry::Ephemeral(d) => d,
            DriverEntry::Persistent(d) => d,
        }
    }

    pub(crate) fn resolve_admission_with_active_turn_boundary(
        &self,
        input: &Input,
        active_turn_boundary_available: bool,
    ) -> Result<ResolvedAdmission, RuntimeDriverError> {
        match self {
            DriverEntry::Ephemeral(d) => {
                d.resolve_admission_with_active_turn_boundary(input, active_turn_boundary_available)
            }
            DriverEntry::Persistent(d) => {
                d.resolve_admission_with_active_turn_boundary(input, active_turn_boundary_available)
            }
        }
    }

    pub(crate) fn resolve_admission_without_wake_with_active_turn_boundary(
        &self,
        input: &Input,
        active_turn_boundary_available: bool,
    ) -> Result<ResolvedAdmission, RuntimeDriverError> {
        match self {
            DriverEntry::Ephemeral(d) => d
                .resolve_admission_without_wake_with_active_turn_boundary(
                    input,
                    active_turn_boundary_available,
                ),
            DriverEntry::Persistent(d) => d
                .resolve_admission_without_wake_with_active_turn_boundary(
                    input,
                    active_turn_boundary_available,
                ),
        }
    }

    pub(crate) async fn accept_resolved_input(
        &mut self,
        input: Input,
        resolved: ResolvedAdmission,
    ) -> Result<AcceptOutcome, RuntimeDriverError> {
        match self {
            DriverEntry::Ephemeral(d) => d.accept_resolved_input(input, resolved).await,
            DriverEntry::Persistent(d) => d.accept_resolved_input(input, resolved).await,
        }
    }

    pub(crate) async fn preview_accept_resolved_input(
        &self,
        input: Input,
        resolved: &ResolvedAdmission,
    ) -> Result<AcceptOutcome, RuntimeDriverError> {
        match self {
            DriverEntry::Ephemeral(d) => d.preview_accept_resolved_input(input, resolved).await,
            DriverEntry::Persistent(d) => d.preview_accept_resolved_input(input, resolved).await,
        }
    }

    pub(crate) fn input_phase(&self, input_id: &InputId) -> Option<InputLifecycleState> {
        self.as_driver().input_phase(input_id)
    }

    pub(crate) fn input_last_run_id(&self, input_id: &InputId) -> Option<RunId> {
        self.as_driver().input_last_run_id(input_id)
    }

    pub(crate) fn input_last_boundary_sequence(&self, input_id: &InputId) -> Option<u64> {
        self.as_driver().input_last_boundary_sequence(input_id)
    }

    #[cfg(test)]
    pub(crate) fn input_runtime_boundary(
        &self,
        input_id: &InputId,
    ) -> Option<crate::meerkat_machine::dsl::RecoveredRunApplyBoundary> {
        match self {
            DriverEntry::Ephemeral(d) => d.input_runtime_boundary(input_id),
            DriverEntry::Persistent(d) => d.inner_ref().input_runtime_boundary(input_id),
        }
    }

    #[cfg(test)]
    pub(crate) fn input_runtime_execution_kind(
        &self,
        input_id: &InputId,
    ) -> Option<crate::meerkat_machine::dsl::RecoveredRuntimeExecutionKind> {
        match self {
            DriverEntry::Ephemeral(d) => d.input_runtime_execution_kind(input_id),
            DriverEntry::Persistent(d) => d.inner_ref().input_runtime_execution_kind(input_id),
        }
    }

    #[cfg(test)]
    pub(crate) fn input_peer_response_terminal_apply_intent(
        &self,
        input_id: &InputId,
    ) -> Option<crate::meerkat_machine::dsl::RecoveredPeerResponseTerminalApplyIntent> {
        match self {
            DriverEntry::Ephemeral(d) => d.input_peer_response_terminal_apply_intent(input_id),
            DriverEntry::Persistent(d) => d
                .inner_ref()
                .input_peer_response_terminal_apply_intent(input_id),
        }
    }

    #[cfg(test)]
    pub(crate) fn input_is_prompt_for_batch(&self, input_id: &InputId) -> Option<bool> {
        match self {
            DriverEntry::Ephemeral(d) => d.input_is_prompt_for_batch(input_id),
            DriverEntry::Persistent(d) => d.inner_ref().input_is_prompt_for_batch(input_id),
        }
    }

    pub(crate) fn input_terminal_outcome(
        &self,
        input_id: &InputId,
    ) -> Option<crate::input_state::InputTerminalOutcome> {
        match self {
            DriverEntry::Ephemeral(d) => d.input_terminal_outcome(input_id),
            DriverEntry::Persistent(d) => d.inner_ref().input_terminal_outcome(input_id),
        }
    }

    pub(crate) fn input_is_terminal_by_authority(
        &self,
        input_id: &InputId,
    ) -> Result<bool, RuntimeDriverError> {
        match self {
            DriverEntry::Ephemeral(d) => d.input_is_terminal_by_authority(input_id),
            DriverEntry::Persistent(d) => d.inner_ref().input_is_terminal_by_authority(input_id),
        }
    }

    pub(crate) fn silent_comms_intents(&self) -> Vec<String> {
        match self {
            DriverEntry::Ephemeral(d) => d.silent_comms_intents(),
            DriverEntry::Persistent(d) => d.silent_comms_intents(),
        }
    }

    pub(crate) fn runtime_lifecycle_facts(
        &self,
    ) -> Result<crate::meerkat_machine::RuntimeLifecycleFacts, RuntimeDriverError> {
        let state = self.runtime_state();
        crate::meerkat_machine::classify_runtime_lifecycle_state(state).map_err(|reason| {
            RuntimeDriverError::Internal(format!(
                "generated runtime lifecycle classification failed for {state}: {reason}"
            ))
        })
    }

    /// Whether this session is quiescent for detached-wake purposes.
    ///
    /// A session is quiescent when it is idle/attached (not running) AND has
    /// no non-terminal inputs in its ledger. Queued-only inputs intentionally
    /// block quiescence — `accept_input_without_wake` stages work without
    /// waking, so detached-wake must not race with pending queue processing.
    pub(crate) fn is_quiescent_for_detached_wake(&self) -> bool {
        match self.runtime_lifecycle_facts() {
            Ok(facts) => facts.can_prepare_run() && self.as_driver().active_input_ids().is_empty(),
            Err(error) => {
                tracing::error!(
                    error = %error,
                    "failed closed while classifying runtime quiescence"
                );
                false
            }
        }
    }

    pub(crate) fn runtime_loop_queue_admission(
        &self,
        current_run_bound: bool,
    ) -> Result<crate::meerkat_machine::RuntimeLoopQueueAdmissionPlan, RuntimeDriverError> {
        let state = self.runtime_state();
        crate::meerkat_machine::classify_runtime_loop_queue_admission(state, current_run_bound)
            .map_err(|reason| {
                RuntimeDriverError::Internal(format!(
                    "generated runtime-loop queue admission failed for {state}: {reason}"
                ))
            })
    }

    /// Inspect the current typed post-admission signal without draining it.
    pub(crate) fn post_admission_signal(&self) -> crate::driver::ephemeral::PostAdmissionSignal {
        match self {
            DriverEntry::Ephemeral(d) => d.post_admission_signal(),
            DriverEntry::Persistent(d) => d.post_admission_signal(),
        }
    }

    pub(crate) fn shared_dsl_authority(
        &self,
    ) -> crate::driver::ephemeral::SharedIngressDslAuthority {
        match self {
            DriverEntry::Ephemeral(d) => d.shared_dsl_authority(),
            DriverEntry::Persistent(d) => d.inner_ref().shared_dsl_authority(),
        }
    }

    pub(crate) fn absorb_post_admission_effects(
        &mut self,
        effects: &[crate::meerkat_machine::dsl::MeerkatMachineEffect],
    ) {
        match self {
            DriverEntry::Ephemeral(d) => d.absorb_post_admission_effects(effects),
            DriverEntry::Persistent(d) => d.absorb_post_admission_effects(effects),
        }
    }

    /// Check and clear the wake flag (backward-compat wrapper).
    pub(crate) fn take_wake_requested(&mut self) -> bool {
        match self {
            DriverEntry::Ephemeral(d) => d.take_wake_requested(),
            DriverEntry::Persistent(d) => d.take_wake_requested(),
        }
    }

    pub(crate) fn dequeue_batch_exact(
        &mut self,
        batch: &AuthorizedRuntimeLoopBatch,
    ) -> Result<Vec<(InputId, crate::input::Input)>, RuntimeDriverError> {
        match self {
            DriverEntry::Ephemeral(d) => d.dequeue_batch_exact(batch),
            DriverEntry::Persistent(d) => d.dequeue_batch_exact(batch),
        }
    }

    /// Get access to the driver's ingress state (queue lanes, admission
    /// metadata) through the concrete driver shell. This is a thin passthrough
    /// facade — the driver itself owns the DSL and the shell metadata maps.
    pub(crate) fn driver_ingress(&self) -> IngressView<'_> {
        match self {
            DriverEntry::Ephemeral(d) => IngressView { driver: d },
            DriverEntry::Persistent(d) => IngressView {
                driver: d.inner_ref(),
            },
        }
    }

    pub(crate) fn runtime_state(&self) -> crate::runtime_state::RuntimeState {
        let authority = self.shared_dsl_authority();
        let authority = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        crate::meerkat_machine::dsl_authority::runtime_phase_from_authority(&authority)
    }

    pub(crate) fn current_run_id(&self) -> Option<RunId> {
        let authority = self.shared_dsl_authority();
        let authority = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        crate::meerkat_machine::dsl_authority::current_run_id_from_authority(&authority)
    }

    pub(crate) fn pre_run_phase(&self) -> Option<crate::runtime_state::RuntimeState> {
        let authority = self.shared_dsl_authority();
        let authority = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        crate::meerkat_machine::dsl_authority::pre_run_phase_from_authority(&authority)
    }

    pub(crate) fn set_control_projection(
        &mut self,
        next_phase: crate::runtime_state::RuntimeState,
        current_run_id: Option<RunId>,
        pre_run_phase: Option<crate::runtime_state::RuntimeState>,
    ) {
        match self {
            DriverEntry::Ephemeral(d) => {
                d.set_control_projection(next_phase, current_run_id, pre_run_phase);
            }
            DriverEntry::Persistent(d) => {
                d.set_control_projection(next_phase, current_run_id, pre_run_phase);
            }
        }
    }

    pub(crate) fn sync_control_projection_from_dsl_authority(&mut self) {
        match self {
            DriverEntry::Ephemeral(d) => d.sync_control_projection_from_dsl_authority(),
            DriverEntry::Persistent(d) => d.sync_control_projection_from_dsl_authority(),
        }
    }

    pub(crate) async fn persist_current_machine_lifecycle(
        &mut self,
        context: &str,
    ) -> Result<(), RuntimeDriverError> {
        match self {
            DriverEntry::Ephemeral(_) => Ok(()),
            DriverEntry::Persistent(d) => d.persist_current_machine_lifecycle(context).await,
        }
    }

    pub(crate) async fn commit_unregister_finalization(
        &mut self,
        context: &str,
        retired_ops_epoch: &meerkat_core::RuntimeEpochId,
        authority: super::DeleteOpsFinalizationAuthority,
    ) -> Result<(), RuntimeDriverError> {
        match self {
            DriverEntry::Ephemeral(_) => Ok(()),
            DriverEntry::Persistent(driver) => {
                driver
                    .commit_unregister_finalization(context, retired_ops_epoch, authority)
                    .await
            }
        }
    }

    pub(crate) async fn persist_completed_unregister_machine_lifecycle(
        &mut self,
        context: &str,
        authority: super::RetainOpsFinalizationAuthority,
    ) -> Result<(), RuntimeDriverError> {
        match self {
            DriverEntry::Ephemeral(_) => Ok(()),
            DriverEntry::Persistent(driver) => {
                driver
                    .persist_completed_unregister_machine_lifecycle(context, authority)
                    .await
            }
        }
    }

    pub(crate) async fn persist_current_machine_lifecycle_with_supervisor_authority(
        &mut self,
        context: &str,
        supervisor_authority: crate::store::SupervisorAuthoritySnapshot,
    ) -> Result<(), RuntimeDriverError> {
        match self {
            DriverEntry::Ephemeral(_) => Ok(()),
            DriverEntry::Persistent(d) => {
                d.persist_current_machine_lifecycle_with_supervisor_authority(
                    context,
                    supervisor_authority,
                )
                .await
            }
        }
    }

    pub(crate) fn control_projection_handle(
        &self,
    ) -> Arc<std::sync::RwLock<crate::driver::ephemeral::RuntimeControlProjection>> {
        match self {
            DriverEntry::Ephemeral(d) => d.control_handle(),
            DriverEntry::Persistent(d) => d.inner_ref().control_handle(),
        }
    }

    pub(crate) fn ledger(&self) -> &crate::input_ledger::InputLedger {
        match self {
            DriverEntry::Ephemeral(d) => d.ledger(),
            DriverEntry::Persistent(d) => d.inner_ref().ledger(),
        }
    }

    fn rollback_snapshot(&self) -> DriverRollbackSnapshot {
        match self {
            DriverEntry::Ephemeral(d) => DriverRollbackSnapshot::Ephemeral(d.rollback_snapshot()),
            DriverEntry::Persistent(d) => DriverRollbackSnapshot::Persistent(d.rollback_snapshot()),
        }
    }

    fn restore_rollback_snapshot(&mut self, checkpoint: DriverRollbackSnapshot) {
        match (self, checkpoint) {
            (DriverEntry::Ephemeral(d), DriverRollbackSnapshot::Ephemeral(checkpoint)) => {
                d.restore_rollback_snapshot(checkpoint);
            }
            (DriverEntry::Persistent(d), DriverRollbackSnapshot::Persistent(checkpoint)) => {
                d.restore_rollback_snapshot(checkpoint);
            }
            _ => {}
        }
    }

    pub(crate) fn has_queued_input_outside(&self, excluded: &[InputId]) -> bool {
        match self {
            DriverEntry::Ephemeral(d) => d.has_queued_input_outside(excluded),
            DriverEntry::Persistent(d) => d.has_queued_input_outside(excluded),
        }
    }

    pub(crate) fn defer_queued_inputs_behind_backlog(
        &mut self,
        input_ids: &[InputId],
    ) -> Result<(), RuntimeDriverError> {
        match self {
            DriverEntry::Ephemeral(d) => d.defer_queued_inputs_behind_backlog(input_ids),
            DriverEntry::Persistent(d) => d.defer_queued_inputs_behind_backlog(input_ids),
        }
    }

    pub(crate) fn machine_realize_boundary_applied_in_memory(
        &mut self,
        run_id: &RunId,
        receipt: &meerkat_core::lifecycle::RunBoundaryReceipt,
    ) -> Result<(), RuntimeDriverError> {
        match self {
            DriverEntry::Ephemeral(d) => d.machine_realize_boundary_applied(run_id, receipt),
            DriverEntry::Persistent(d) => {
                d.machine_realize_boundary_applied_in_memory(run_id, receipt)
            }
        }
    }

    pub(crate) fn machine_realize_run_completed_in_memory(
        &mut self,
        run_id: &RunId,
        consumed_input_ids: &[InputId],
    ) -> Result<(), RuntimeDriverError> {
        match self {
            DriverEntry::Ephemeral(d) => {
                d.machine_realize_run_completed(run_id, consumed_input_ids)
            }
            DriverEntry::Persistent(d) => {
                d.machine_realize_run_completed_in_memory(run_id, consumed_input_ids)
            }
        }
    }

    pub(crate) async fn machine_commit_completed_boundary_snapshot(
        &mut self,
        receipt: &meerkat_core::lifecycle::RunBoundaryReceipt,
        session_snapshot: Option<&Vec<u8>>,
        owner_session_id: &SessionId,
    ) -> Result<(), RuntimeDriverError> {
        match self {
            DriverEntry::Ephemeral(_) => Ok(()),
            DriverEntry::Persistent(d) => {
                d.machine_commit_completed_boundary_snapshot(
                    receipt,
                    session_snapshot,
                    owner_session_id,
                )
                .await
            }
        }
    }

    pub(crate) async fn machine_realize_run_failed(
        &mut self,
        realization: MachineRunFailureRealization,
    ) -> Result<(), RuntimeDriverError> {
        let MachineRunFailureRealization {
            run_id,
            contributing_input_ids,
            replay_plan,
            terminal_error,
            runtime_apply_failure,
            recoverable,
            applied_commit,
        } = realization;
        let checkpoint = self.rollback_snapshot();

        if let Err(error) = self.shell_driver_mut().machine_realize_run_failed(
            &run_id,
            &contributing_input_ids,
            &replay_plan,
            recoverable,
        ) {
            self.restore_rollback_snapshot(checkpoint);
            return Err(error);
        }

        // A recoverable failed run normally requeues its contributors, but the
        // generated rollback transition can terminalize the exact inputs whose
        // stage-attempt budget was exhausted. Only that newly-Abandoned directed
        // subset owns an Interaction terminal. Requeued directed inputs retain
        // their existing waiters and receive no outbox row.
        if recoverable {
            let terminal_directed_input_ids =
                match max_attempts_exhausted_directed_contributors(self, &contributing_input_ids) {
                    Ok(input_ids) => input_ids,
                    Err(error) => {
                        self.restore_rollback_snapshot(checkpoint);
                        return Err(error);
                    }
                };
            if !terminal_directed_input_ids.is_empty() {
                let outboxes = match authorized_staged_directed_terminal_outboxes(
                    self,
                    InteractionTerminalBatchScope::Run(&run_id),
                    &terminal_directed_input_ids,
                    crate::input_state::InteractionTerminalCandidate::MachineTerminalFailure {
                        error: meerkat_core::TurnErrorMetadata::runtime_apply_failure(
                            terminal_error.clone(),
                        ),
                    },
                ) {
                    Ok(outboxes) => outboxes,
                    Err(error) => {
                        self.restore_rollback_snapshot(checkpoint);
                        return Err(error);
                    }
                };
                if let Err(error) = self.stage_interaction_terminal_outboxes(outboxes) {
                    self.restore_rollback_snapshot(checkpoint);
                    return Err(error);
                }
            }
        }

        let persist_result = match self {
            DriverEntry::Ephemeral(_) => Ok(()),
            DriverEntry::Persistent(driver) => {
                driver
                    .persist_machine_realized_run_failed(MachineRunFailureRealization {
                        run_id,
                        contributing_input_ids,
                        replay_plan,
                        terminal_error,
                        runtime_apply_failure,
                        recoverable,
                        applied_commit,
                    })
                    .await
            }
        };
        if let Err(error) = persist_result {
            self.restore_rollback_snapshot(checkpoint);
            return Err(error);
        }
        Ok(())
    }

    fn machine_realize_terminal_failure_applied_in_memory(
        &mut self,
        run_id: &RunId,
        contributing_input_ids: &[InputId],
    ) -> Result<(), RuntimeDriverError> {
        match self {
            DriverEntry::Ephemeral(driver) => {
                driver.machine_realize_terminal_failure_applied(run_id, contributing_input_ids)?;
            }
            DriverEntry::Persistent(driver) => {
                driver.machine_realize_terminal_failure_applied(run_id, contributing_input_ids)?;
            }
        }
        Ok(())
    }

    pub(crate) async fn machine_realize_run_cancelled(
        &mut self,
        run_id: RunId,
        contributing_input_ids: Vec<InputId>,
    ) -> Result<(), RuntimeDriverError> {
        match self {
            DriverEntry::Ephemeral(d) => {
                d.machine_realize_run_cancelled(&run_id, &contributing_input_ids)
            }
            DriverEntry::Persistent(d) => {
                d.machine_realize_run_cancelled(&run_id, &contributing_input_ids)
                    .await
            }
        }
    }

    pub(crate) async fn machine_realize_live_boundary_context_injected(
        &mut self,
        run_id: &RunId,
        input_ids: &[InputId],
        session_snapshot: Option<Vec<u8>>,
    ) -> Result<(), RuntimeDriverError> {
        let stage_authority = machine_authorize_stage_for_run(
            self,
            run_id,
            input_ids,
            RuntimeLoopBatchSource::Steer,
        )
        .ok_or_else(|| {
            RuntimeDriverError::Internal(format!(
                "generated machine did not authorize live-boundary StageForRun for run {run_id:?} and inputs {input_ids:?}"
            ))
        })?;
        match self {
            DriverEntry::Ephemeral(d) => {
                let _ = session_snapshot;
                d.machine_realize_live_boundary_context_injected(run_id, input_ids, stage_authority)
                    .map(|_| ())
            }
            DriverEntry::Persistent(d) => {
                d.machine_realize_live_boundary_context_injected(
                    run_id,
                    input_ids,
                    stage_authority,
                    session_snapshot,
                )
                .await
            }
        }
    }

    /// Stage a batch of inputs atomically in a single `StageDrainSnapshot`.
    pub(crate) fn machine_realize_authorized_stage_batch(
        &mut self,
        authority: AuthorizedStageForRun,
    ) -> Result<(), crate::traits::RuntimeDriverError> {
        match self {
            DriverEntry::Ephemeral(d) => d.machine_realize_authorized_stage_batch(authority),
            DriverEntry::Persistent(d) => d.machine_realize_authorized_stage_batch(authority),
        }
    }

    /// Roll back staged inputs after a failed staging attempt.
    pub(crate) fn rollback_staged(
        &mut self,
        input_ids: &[InputId],
    ) -> Result<(), crate::traits::RuntimeDriverError> {
        match self {
            DriverEntry::Ephemeral(d) => d.rollback_staged(input_ids),
            DriverEntry::Persistent(d) => d.rollback_staged(input_ids),
        }
    }

    pub(crate) async fn abandon_pending_inputs(
        &mut self,
        reason: crate::input_state::InputAbandonReason,
    ) -> Result<usize, RuntimeDriverError> {
        match self {
            DriverEntry::Ephemeral(d) => d.abandon_pending_inputs(reason),
            DriverEntry::Persistent(d) => d.abandon_pending_inputs(reason).await,
        }
    }

    pub(crate) async fn abandon_queued_input(
        &mut self,
        input_id: &InputId,
        reason: crate::input_state::InputAbandonReason,
    ) -> Result<bool, RuntimeDriverError> {
        match self {
            DriverEntry::Ephemeral(driver) => driver.abandon_queued_input(input_id, reason),
            DriverEntry::Persistent(driver) => driver.abandon_queued_input(input_id, reason).await,
        }
    }

    pub(crate) fn prepare_destroy_lifecycle(
        &mut self,
    ) -> Result<PreparedDestroy, RuntimeDriverError> {
        match self {
            DriverEntry::Ephemeral(d) => {
                let checkpoint = d.rollback_snapshot();
                let abandoned = d.destroy_cleanup()?;
                Ok(PreparedDestroy {
                    report: DestroyReport {
                        inputs_abandoned: abandoned,
                    },
                    lifecycle: PreparedDestroyLifecycle::Ephemeral(checkpoint),
                })
            }
            DriverEntry::Persistent(d) => {
                let (checkpoint, report) = d.prepare_destroy_lifecycle()?;
                Ok(PreparedDestroy {
                    report,
                    lifecycle: PreparedDestroyLifecycle::Persistent(checkpoint),
                })
            }
        }
    }

    pub(crate) async fn commit_prepared_destroy_lifecycle(
        &mut self,
        lifecycle: PreparedDestroyLifecycle,
    ) -> Result<(), RuntimeDriverError> {
        match (self, lifecycle) {
            (DriverEntry::Ephemeral(_), PreparedDestroyLifecycle::Ephemeral(_)) => Ok(()),
            (DriverEntry::Persistent(d), PreparedDestroyLifecycle::Persistent(checkpoint)) => {
                d.commit_prepared_destroy_lifecycle(checkpoint).await
            }
            _ => Err(RuntimeDriverError::Internal(
                "destroy lifecycle prepared for a different driver kind".to_string(),
            )),
        }
    }

    pub(crate) fn rollback_prepared_destroy_lifecycle(
        &mut self,
        lifecycle: PreparedDestroyLifecycle,
    ) {
        match (self, lifecycle) {
            (DriverEntry::Ephemeral(d), PreparedDestroyLifecycle::Ephemeral(checkpoint)) => {
                d.restore_rollback_snapshot(checkpoint);
            }
            (DriverEntry::Persistent(d), PreparedDestroyLifecycle::Persistent(checkpoint)) => {
                d.rollback_prepared_destroy_lifecycle(checkpoint);
            }
            _ => {}
        }
    }
}

fn authorized_directed_terminal_outboxes(
    driver: &DriverEntry,
    authority: &AuthorizedRuntimeLoopRunCommit,
    directed_interaction_ids: &[meerkat_core::interaction::InteractionId],
    terminal: Option<&meerkat_core::lifecycle::core_executor::CoreApplyTerminal>,
) -> Result<Vec<crate::input_state::InteractionTerminalOutbox>, RuntimeDriverError> {
    use crate::input_state::{
        InteractionTerminalCandidate, InteractionTerminalOutbox,
        interaction_terminal_payload_digest,
    };

    let mut directed_interaction_ids = directed_interaction_ids.to_vec();
    directed_interaction_ids.sort_by_key(|interaction_id| interaction_id.0);
    let mut declared = std::collections::BTreeSet::new();
    if directed_interaction_ids.len() > 256 {
        return Err(RuntimeDriverError::ValidationFailed {
            reason: "directed interaction terminal batch exceeds the 256-input pending bound"
                .to_string(),
        });
    }
    for interaction_id in &directed_interaction_ids {
        if !declared.insert(interaction_id.0) {
            return Err(RuntimeDriverError::ValidationFailed {
                reason: format!(
                    "duplicate directed interaction id {interaction_id} in runtime turn metadata"
                ),
            });
        }
    }

    let consumed: std::collections::BTreeSet<_> = authority
        .consumed_input_ids()
        .iter()
        .map(|input_id| input_id.0)
        .collect();
    let mut persisted_directed = std::collections::BTreeSet::new();
    for input_id in authority.consumed_input_ids() {
        let stored = driver
            .as_driver()
            .stored_input_state(input_id)
            .ok_or_else(|| RuntimeDriverError::Internal(format!(
                "consumed input {input_id} has no persisted shell for directed terminal validation"
            )))?;
        if let Some(input) = stored.state.persisted_input.as_ref()
            && let Some(interaction_id) = crate::input::validated_directed_interaction_id(input)
                .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?
        {
            persisted_directed.insert(interaction_id.0);
        }
    }
    if declared != persisted_directed {
        return Err(RuntimeDriverError::ValidationFailed {
            reason: format!(
                "directed interaction metadata did not exactly match persisted tracked inputs: declared={declared:?}, persisted={persisted_directed:?}"
            ),
        });
    }
    if !declared.is_subset(&consumed) {
        return Err(RuntimeDriverError::ValidationFailed {
            reason: "directed interaction metadata named a non-contributing input".to_string(),
        });
    }
    if declared.is_empty() {
        return Ok(Vec::new());
    }

    let completion_input_ids = authority.consumed_input_ids().to_vec();
    if completion_input_ids.is_empty() || completion_input_ids.len() > 256 {
        return Err(RuntimeDriverError::ValidationFailed {
            reason: "runtime completion recipient batch has invalid size".to_string(),
        });
    }
    if completion_input_ids
        .iter()
        .collect::<std::collections::HashSet<_>>()
        .len()
        != completion_input_ids.len()
    {
        return Err(RuntimeDriverError::ValidationFailed {
            reason: "runtime completion recipient batch contains duplicate inputs".to_string(),
        });
    }
    let completion_input_ids_digest = interaction_terminal_payload_digest(&completion_input_ids)
        .map_err(RuntimeDriverError::Internal)?;

    let owner_session_id = authority.owner_session_id().clone();
    let candidate = match terminal {
        Some(meerkat_core::lifecycle::core_executor::CoreApplyTerminal::RunResult(result)) => {
            InteractionTerminalCandidate::RunResult {
                result: result.clone(),
            }
        }
        Some(meerkat_core::lifecycle::core_executor::CoreApplyTerminal::CallbackPending {
            tool_name,
            args,
        }) => InteractionTerminalCandidate::CallbackPending {
            tool_name: tool_name.clone(),
            args: args.clone(),
        },
        Some(
            meerkat_core::lifecycle::core_executor::CoreApplyTerminal::MachineTerminalFailure {
                error,
            },
        ) => InteractionTerminalCandidate::MachineTerminalFailure {
            error: error.clone(),
        },
        Some(meerkat_core::lifecycle::core_executor::CoreApplyTerminal::NoPendingBoundary)
        | None => InteractionTerminalCandidate::CompletedWithoutResult,
    };
    let candidate_digest =
        interaction_terminal_payload_digest(&candidate).map_err(RuntimeDriverError::Internal)?;

    let candidate_owner_input_id = InputId::from_uuid(
        directed_interaction_ids
            .first()
            .ok_or_else(|| RuntimeDriverError::Internal("directed batch lost owner".into()))?
            .0,
    );
    directed_interaction_ids
        .iter()
        .enumerate()
        .map(|(ordinal, interaction_id)| {
            let input_id = InputId::from_uuid(interaction_id.0);
            let owns_candidate = input_id == candidate_owner_input_id;
            let outbox = InteractionTerminalOutbox {
                interaction_id: *interaction_id,
                input_id,
                batch_ordinal: ordinal as u16,
                batch_key: crate::input_state::InteractionTerminalBatchKey::Run {
                    run_id: authority.run_id().clone(),
                },
                owner_session_id: owner_session_id.clone(),
                owner_agent_runtime_id: authority
                    .owner_agent_runtime_id()
                    .map(|value| value.0.clone()),
                owner_fence_token: authority.owner_fence_token().map(|value| value.0),
                owner_runtime_generation: authority.owner_runtime_generation().map(|value| value.0),
                owner_runtime_epoch_id: authority
                    .owner_runtime_epoch_id()
                    .map(|value| value.0.clone()),
                candidate_owner_input_id: candidate_owner_input_id.clone(),
                candidate: owns_candidate.then(|| candidate.clone()),
                candidate_digest: candidate_digest.clone(),
                completion_input_ids: owns_candidate.then(|| completion_input_ids.clone()),
                completion_input_ids_digest: completion_input_ids_digest.clone(),
                phase: crate::input_state::InteractionTerminalOutboxPhase::Candidate,
            };
            outbox.validate().map_err(RuntimeDriverError::Internal)?;
            Ok(outbox)
        })
        .collect()
}

fn authorized_staged_directed_terminal_outboxes(
    driver: &DriverEntry,
    scope: InteractionTerminalBatchScope<'_>,
    input_ids: &[InputId],
    candidate: crate::input_state::InteractionTerminalCandidate,
) -> Result<Vec<crate::input_state::InteractionTerminalOutbox>, RuntimeDriverError> {
    use crate::input_state::{InteractionTerminalOutbox, interaction_terminal_payload_digest};
    let mut directed = Vec::new();
    for input_id in input_ids {
        let stored = driver
            .as_driver()
            .stored_input_state(input_id)
            .ok_or_else(|| {
                RuntimeDriverError::Internal(format!(
                    "staged input {input_id} disappeared before terminal outbox authorization"
                ))
            })?;
        if let Some(input) = stored.state.persisted_input.as_ref()
            && let Some(interaction_id) = crate::input::validated_directed_interaction_id(input)
                .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?
        {
            if input_id.0 != interaction_id.0 {
                return Err(RuntimeDriverError::ValidationFailed {
                    reason: "directed terminal input/interaction identity mismatch".to_string(),
                });
            }
            directed.push((input_id.clone(), interaction_id));
        }
    }
    if directed.len() > 256 {
        return Err(RuntimeDriverError::ValidationFailed {
            reason: "directed interaction terminal batch exceeds the 256-input pending bound"
                .to_string(),
        });
    }
    if directed.is_empty() {
        return Ok(Vec::new());
    }
    if input_ids.len() > 256 {
        return Err(RuntimeDriverError::ValidationFailed {
            reason: "staged completion recipient batch exceeds the 256-input pending bound"
                .to_string(),
        });
    }
    if input_ids
        .iter()
        .collect::<std::collections::HashSet<_>>()
        .len()
        != input_ids.len()
    {
        return Err(RuntimeDriverError::ValidationFailed {
            reason: "staged completion recipient batch contains duplicate inputs".to_string(),
        });
    }
    let completion_input_ids = input_ids.to_vec();
    let completion_input_ids_digest = interaction_terminal_payload_digest(&completion_input_ids)
        .map_err(RuntimeDriverError::Internal)?;
    directed.sort_by_key(|(input_id, _)| input_id.0);
    let binding = {
        let authority = driver.shared_dsl_authority();
        let authority = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let state = authority.state();
        (
            state.session_id.clone(),
            state.active_runtime_id.clone(),
            state.active_fence_token,
            state.active_runtime_generation,
            state.active_runtime_epoch_id.clone(),
        )
    };
    let owner_session = binding.0.ok_or_else(|| {
        RuntimeDriverError::Internal(
            "directed terminal outbox authority carried no owner session".to_string(),
        )
    })?;
    let owner_session_id = SessionId::parse(&owner_session.0).map_err(|error| {
        RuntimeDriverError::Internal(format!(
            "directed terminal outbox owner session was not a UUID: {error}"
        ))
    })?;
    let candidate_digest =
        interaction_terminal_payload_digest(&candidate).map_err(RuntimeDriverError::Internal)?;
    let candidate_owner_input_id = directed
        .first()
        .map(|(input_id, _)| input_id.clone())
        .ok_or_else(|| RuntimeDriverError::Internal("directed batch lost owner".into()))?;
    let batch_key = match scope {
        InteractionTerminalBatchScope::Run(run_id) => {
            crate::input_state::InteractionTerminalBatchKey::Run {
                run_id: run_id.clone(),
            }
        }
        InteractionTerminalBatchScope::RuntimeTermination => {
            crate::input_state::InteractionTerminalBatchKey::RuntimeTermination {
                candidate_owner_input_id: candidate_owner_input_id.clone(),
            }
        }
    };
    directed
        .into_iter()
        .enumerate()
        .map(|(ordinal, (input_id, interaction_id))| {
            let owns_candidate = input_id == candidate_owner_input_id;
            let outbox = InteractionTerminalOutbox {
                interaction_id,
                input_id,
                batch_ordinal: ordinal as u16,
                batch_key: batch_key.clone(),
                owner_session_id: owner_session_id.clone(),
                owner_agent_runtime_id: binding.1.as_ref().map(|value| value.0.clone()),
                owner_fence_token: binding.2.map(|value| value.0),
                owner_runtime_generation: binding.3.map(|value| value.0),
                owner_runtime_epoch_id: binding.4.as_ref().map(|value| value.0.clone()),
                candidate_owner_input_id: candidate_owner_input_id.clone(),
                candidate: owns_candidate.then(|| candidate.clone()),
                candidate_digest: candidate_digest.clone(),
                completion_input_ids: owns_candidate.then(|| completion_input_ids.clone()),
                completion_input_ids_digest: completion_input_ids_digest.clone(),
                phase: crate::input_state::InteractionTerminalOutboxPhase::Candidate,
            };
            outbox.validate().map_err(RuntimeDriverError::Internal)?;
            Ok(outbox)
        })
        .collect()
}

/// Identify the exact contributors terminalized by the generated
/// `ResolveStagedRollback -> Abandon(MaxAttemptsExhausted)` transition.
///
/// The caller supplies the pre-transition staged contributor set. Reading the
/// generated-backed stored state after rollback makes this an observation of
/// the transition result, not a prediction from an attempt counter. Only
/// directed terminal rows are returned; surviving queued contributors are
/// deliberately absent so their completion waiters remain registered.
fn max_attempts_exhausted_directed_contributors(
    driver: &DriverEntry,
    contributing_input_ids: &[InputId],
) -> Result<Vec<InputId>, RuntimeDriverError> {
    let mut terminal_directed = Vec::new();
    for input_id in contributing_input_ids {
        let stored = driver
            .as_driver()
            .stored_input_state(input_id)
            .ok_or_else(|| {
                RuntimeDriverError::Internal(format!(
                    "failed-run contributor {input_id} disappeared after rollback realization"
                ))
            })?;
        if !matches!(
            (&stored.seed.phase, &stored.seed.terminal_outcome),
            (
                crate::input_state::InputLifecycleState::Abandoned,
                Some(crate::input_state::InputTerminalOutcome::Abandoned {
                    reason: crate::input_state::InputAbandonReason::MaxAttemptsExhausted { .. }
                })
            )
        ) {
            continue;
        }
        let input = stored.state.persisted_input.as_ref().ok_or_else(|| {
            RuntimeDriverError::RecoveryCorruption {
                reason: format!(
                    "max-attempts-abandoned contributor {input_id} lost its admitted input payload"
                ),
            }
        })?;
        let Some(interaction_id) = crate::input::validated_directed_interaction_id(input)
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?
        else {
            continue;
        };
        if input_id.0 != interaction_id.0 {
            return Err(RuntimeDriverError::ValidationFailed {
                reason: "abandoned directed terminal input/interaction identity mismatch"
                    .to_string(),
            });
        }
        terminal_directed.push(input_id.clone());
    }
    terminal_directed.sort_by_key(|input_id| input_id.0);
    Ok(terminal_directed)
}

/// Shared completion registry (accessed by adapter for registration and loop for resolution).
pub(crate) type SharedCompletionRegistry = Arc<Mutex<crate::completion::CompletionRegistry>>;

pub(crate) fn machine_begin_run(
    driver: &mut DriverEntry,
    run_id: RunId,
) -> Result<(), crate::runtime_state::RuntimeStateTransitionError> {
    let from = driver.runtime_state();
    if from == RuntimeState::Running && driver.current_run_id().as_ref() == Some(&run_id) {
        return Ok(());
    }

    // DSL is authoritative for `lifecycle_phase` + `current_run_id`
    // post-#32 W6-J (dogma #1 split). Fire the typed `Prepare { session_id,
    // run_id }` DSL input; the `PrepareIdle` / `PrepareAttached` transitions
    // flip `lifecycle_phase` to Running and set `current_run_id` atomically.
    // The runtime-loop path reaches this code directly (via
    // `prepare_runtime_loop_batch_start`); previously the Attached→Running
    // hinge was shell-only via `set_control_projection`, leaving DSL's
    // `current_run_id` unbound / mismatched and the `CommitRunningTo*`
    // guards on the return path rejecting the Commit input. Both sides now
    // go through DSL uniformly. Shell `control_projection` remains only as
    // mechanical projection/event plumbing; DriverEntry reads the DSL
    // authority directly.
    let projection = {
        let authority = driver.shared_dsl_authority();
        let mut auth = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Idempotent fast-path: if DSL is already at Running with a
        // matching run_id, the caller (e.g. `dispatch_ingress::Prepare`
        // command handler) has already staged Prepare; nothing to do.
        let already_prepared = auth.state().lifecycle_phase
            == crate::meerkat_machine::dsl::MeerkatPhase::Running
            && auth.state().current_run_id.as_ref().map(|id| id.0.as_str())
                == Some(run_id.to_string().as_str());
        let is_retired_drain =
            auth.state().lifecycle_phase == crate::meerkat_machine::dsl::MeerkatPhase::Retired;
        if !already_prepared {
            let apply_result = if is_retired_drain {
                auth.apply_signal(
                    crate::meerkat_machine::dsl::MeerkatMachineSignal::DrainQueuedRun {
                        run_id: crate::meerkat_machine::dsl::RunId::from_domain(&run_id),
                    },
                )
                .map(|_| ())
            } else {
                let Some(dsl_session_id) = auth.state().session_id.clone() else {
                    return Err(crate::runtime_state::RuntimeStateTransitionError {
                        from,
                        to: RuntimeState::Running,
                    });
                };
                crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(
                    &mut *auth,
                    crate::meerkat_machine::dsl::MeerkatMachineInput::Prepare {
                        session_id: dsl_session_id,
                        run_id: crate::meerkat_machine::dsl::RunId::from_domain(&run_id),
                    },
                )
                .map(|_| ())
            };
            if apply_result.is_err() {
                return Err(crate::runtime_state::RuntimeStateTransitionError {
                    from,
                    to: RuntimeState::Running,
                });
            }
        }
        RuntimeLifecycleProjection::from_authority(&auth)
    };

    driver.set_control_projection(
        projection.phase,
        projection.current_run_id,
        projection.pre_run_phase,
    );
    Ok(())
}

/// Disposition the runtime-loop apply produced. `Failed` is only used after the
/// turn-state authority has recorded a typed failed terminal cause; `Rollback`
/// is non-semantic cleanup for prepare/stage failures before turn failure
/// exists.
#[derive(Clone, Copy)]
pub(crate) enum RunReturnDisposition<'a> {
    Commit { input_id: &'a InputId },
    Failed,
    Cancelled,
    Rollback,
}

#[derive(Debug)]
pub(crate) enum RuntimeLoopRunCommitError {
    Rejected(RuntimeDriverError),
    BoundaryCommit(RuntimeDriverError),
    PostBoundaryValidation(RuntimeDriverError),
    TerminalSnapshot(RuntimeDriverError),
}

pub(crate) struct MachineTerminalAppliedDraft {
    pub(crate) receipt: meerkat_core::lifecycle::RunBoundaryReceiptDraft,
    pub(crate) session_snapshot: Vec<u8>,
}

pub(crate) struct MachineTerminalAppliedCommit {
    pub(crate) receipt: meerkat_core::lifecycle::RunBoundaryReceipt,
    pub(crate) session_snapshot: Vec<u8>,
    pub(crate) owner_session_id: SessionId,
}

/// Complete mechanical payload for realizing a machine-authorized failed run.
///
/// The runtime-loop owner assembles this only after the generated authority has
/// classified the failure and validated any failed-but-applied commit. Driver
/// variants then consume the same payload without reconstructing semantic
/// failure facts at the persistence boundary.
pub(crate) struct MachineRunFailureRealization {
    pub(crate) run_id: RunId,
    pub(crate) contributing_input_ids: Vec<InputId>,
    pub(crate) replay_plan: crate::driver::ephemeral::ReplayQueuedContributorsPlan,
    pub(crate) terminal_error: String,
    pub(crate) runtime_apply_failure: Option<CoreApplyFailureCause>,
    pub(crate) recoverable: bool,
    pub(crate) applied_commit: Option<MachineTerminalAppliedCommit>,
}

impl std::fmt::Display for RuntimeLoopRunCommitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rejected(err)
            | Self::BoundaryCommit(err)
            | Self::PostBoundaryValidation(err)
            | Self::TerminalSnapshot(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for RuntimeLoopRunCommitError {}

#[must_use = "runtime-loop run commit authority must be consumed by commit realization"]
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct AuthorizedRuntimeLoopRunCommit {
    generated_plan: generated_kernel_command_capabilities::CommandPlanKind,
    run_id: RunId,
    consumed_input_ids: Vec<InputId>,
    commit_input_id: InputId,
    receipt: meerkat_core::lifecycle::RunBoundaryReceipt,
    owner_session_id: SessionId,
    owner_agent_runtime_id: Option<crate::meerkat_machine::dsl::AgentRuntimeId>,
    owner_fence_token: Option<crate::meerkat_machine::dsl::FenceToken>,
    owner_runtime_generation: Option<crate::meerkat_machine::dsl::Generation>,
    owner_runtime_epoch_id: Option<crate::meerkat_machine::dsl::RuntimeEpochId>,
    commit_outcome: AuthorizedRuntimeLoopRunCommitOutcome,
    effect_closure_obligations: Vec<RuntimeLoopRunCommitEffectObligation>,
    return_projection: RuntimeLifecycleProjection,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct AuthorizedRuntimeLoopRunCommitOutcome {
    run_id: RunId,
    outcome: crate::meerkat_machine::dsl::TurnTerminalOutcome,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct RuntimeLoopRunCommitEffectObligation {
    run_id: RunId,
    effect: RuntimeLoopRunCommitEffect,
    closure_policy: &'static str,
    lifecycle: &'static [&'static str],
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RuntimeLoopRunCommitEffect {
    Completed,
    Failed,
    Cancelled,
}

impl AuthorizedRuntimeLoopRunCommit {
    fn authorize(
        driver: &DriverEntry,
        run_id: RunId,
        consumed_input_ids: Vec<InputId>,
        receipt: meerkat_core::lifecycle::RunBoundaryReceiptDraft,
    ) -> Result<Self, RuntimeDriverError> {
        // Dogma K10: mint the final receipt from the machine-owned per-run
        // boundary counter — the executor's draft carries no sequence, so the
        // durable receipt can never diverge from `RecordBoundarySeq`'s value.
        let receipt = receipt.into_sequenced(driver.run_boundary_sequence(&run_id));
        machine_validate_run_commit_receipt(driver, &run_id, &consumed_input_ids, &receipt)?;
        let commit_input_id = consumed_input_ids.first().cloned().ok_or_else(|| {
            RuntimeDriverError::Internal(
                "runtime-loop run commit authority requires at least one terminal input"
                    .to_string(),
            )
        })?;
        let preview =
            preview_authorized_runtime_loop_run_commit(driver, &run_id, &commit_input_id)?;
        let owner_session_id = preview
            .owner_session_id
            .as_ref()
            .ok_or_else(|| RuntimeDriverError::ValidationFailed {
                reason: "runtime-loop run commit authority carried no owner session".to_string(),
            })
            .and_then(|session_id| {
                SessionId::parse(&session_id.0).map_err(|error| {
                    RuntimeDriverError::ValidationFailed {
                        reason: format!(
                            "runtime-loop run commit owner session was invalid: {error}"
                        ),
                    }
                })
            })?;
        Ok(Self {
            generated_plan:
                generated_kernel_command_capabilities::CommandPlanKind::AuthorizedRuntimeLoopRunCommit,
            run_id,
            consumed_input_ids,
            commit_input_id,
            receipt,
            owner_session_id,
            owner_agent_runtime_id: preview.owner_agent_runtime_id,
            owner_fence_token: preview.owner_fence_token,
            owner_runtime_generation: preview.owner_runtime_generation,
            owner_runtime_epoch_id: preview.owner_runtime_epoch_id,
            commit_outcome: preview.commit_outcome,
            effect_closure_obligations: preview.effect_closure_obligations,
            return_projection: preview.return_projection,
        })
    }

    fn run_id(&self) -> &RunId {
        &self.run_id
    }

    fn consumed_input_ids(&self) -> &[InputId] {
        &self.consumed_input_ids
    }

    fn commit_input_id(&self) -> &InputId {
        &self.commit_input_id
    }

    fn receipt(&self) -> &meerkat_core::lifecycle::RunBoundaryReceipt {
        &self.receipt
    }

    fn commit_outcome(&self) -> &AuthorizedRuntimeLoopRunCommitOutcome {
        &self.commit_outcome
    }

    fn return_projection(&self) -> &RuntimeLifecycleProjection {
        &self.return_projection
    }

    fn effect_closure_obligations(&self) -> &[RuntimeLoopRunCommitEffectObligation] {
        &self.effect_closure_obligations
    }

    fn generated_plan(&self) -> generated_kernel_command_capabilities::CommandPlanKind {
        self.generated_plan
    }

    fn owner_session_id(&self) -> &SessionId {
        &self.owner_session_id
    }

    fn owner_agent_runtime_id(&self) -> Option<&crate::meerkat_machine::dsl::AgentRuntimeId> {
        self.owner_agent_runtime_id.as_ref()
    }

    fn owner_fence_token(&self) -> Option<crate::meerkat_machine::dsl::FenceToken> {
        self.owner_fence_token
    }

    fn owner_runtime_generation(&self) -> Option<crate::meerkat_machine::dsl::Generation> {
        self.owner_runtime_generation
    }

    fn owner_runtime_epoch_id(&self) -> Option<&crate::meerkat_machine::dsl::RuntimeEpochId> {
        self.owner_runtime_epoch_id.as_ref()
    }

    fn into_receipt(self) -> meerkat_core::lifecycle::RunBoundaryReceipt {
        self.receipt
    }
}

impl AuthorizedRuntimeLoopRunCommitOutcome {
    fn run_id(&self) -> &RunId {
        &self.run_id
    }

    fn outcome(&self) -> crate::meerkat_machine::dsl::TurnTerminalOutcome {
        self.outcome
    }
}

struct RuntimeLoopRunCommitPreview {
    owner_session_id: Option<crate::meerkat_machine::dsl::SessionId>,
    owner_agent_runtime_id: Option<crate::meerkat_machine::dsl::AgentRuntimeId>,
    owner_fence_token: Option<crate::meerkat_machine::dsl::FenceToken>,
    owner_runtime_generation: Option<crate::meerkat_machine::dsl::Generation>,
    owner_runtime_epoch_id: Option<crate::meerkat_machine::dsl::RuntimeEpochId>,
    commit_outcome: AuthorizedRuntimeLoopRunCommitOutcome,
    effect_closure_obligations: Vec<RuntimeLoopRunCommitEffectObligation>,
    return_projection: RuntimeLifecycleProjection,
}

fn preview_authorized_runtime_loop_run_commit(
    driver: &DriverEntry,
    run_id: &RunId,
    commit_input_id: &InputId,
) -> Result<RuntimeLoopRunCommitPreview, RuntimeDriverError> {
    let authority = driver.shared_dsl_authority();
    let state = {
        let authority = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        authority.state().clone()
    };
    let owner_session_id = state.session_id.clone();
    let owner_agent_runtime_id = state.active_runtime_id.clone();
    let owner_fence_token = state.active_fence_token;
    let owner_runtime_generation = state.active_runtime_generation;
    let owner_runtime_epoch_id = state.active_runtime_epoch_id.clone();
    let mut preview = crate::meerkat_machine::dsl::MeerkatMachineAuthority::recover_from_state(
        state,
    )
    .map_err(|err| {
        RuntimeDriverError::Internal(crate::meerkat_machine::dsl_authority::map_error(
            err,
            "AuthorizedRuntimeLoopRunCommit",
        ))
    })?;
    let dsl_run_id = crate::meerkat_machine::dsl::RunId::from_domain(run_id);
    crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(
        &mut preview,
        crate::meerkat_machine::dsl::MeerkatMachineInput::RunCompleted {
            run_id: dsl_run_id.clone(),
        },
    )
    .map_err(|err| RuntimeDriverError::ValidationFailed {
        reason: crate::meerkat_machine::dsl_authority::map_error(err, "RunCompleted"),
    })?;
    if preview.state().runtime_completion_result_run_id.as_ref() != Some(&dsl_run_id) {
        return Err(RuntimeDriverError::Internal(format!(
            "RunCompleted did not bind runtime completion result to run {run_id}"
        )));
    }
    let commit_outcome = match preview.state().terminal_outcome {
        Some(crate::meerkat_machine::dsl::TurnTerminalOutcome::Completed) => {
            AuthorizedRuntimeLoopRunCommitOutcome {
                run_id: run_id.clone(),
                outcome: crate::meerkat_machine::dsl::TurnTerminalOutcome::Completed,
            }
        }
        other => {
            return Err(RuntimeDriverError::Internal(format!(
                "RunCompleted produced unexpected terminal outcome {other:?} for run {run_id}"
            )));
        }
    };
    let effect_closure_obligations =
        RuntimeLoopRunCommitEffectObligation::for_outcome(run_id, commit_outcome.outcome());

    crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(
        &mut preview,
        crate::meerkat_machine::dsl::MeerkatMachineInput::Commit {
            input_id: crate::meerkat_machine::dsl::InputId::from_domain(commit_input_id),
            run_id: dsl_run_id,
        },
    )
    .map_err(|err| RuntimeDriverError::ValidationFailed {
        reason: crate::meerkat_machine::dsl_authority::map_error(err, "Commit"),
    })?;

    Ok(RuntimeLoopRunCommitPreview {
        owner_session_id,
        owner_agent_runtime_id,
        owner_fence_token,
        owner_runtime_generation,
        owner_runtime_epoch_id,
        commit_outcome,
        effect_closure_obligations,
        return_projection: RuntimeLifecycleProjection::from_authority(&preview),
    })
}

impl RuntimeLoopRunCommitEffectObligation {
    const LIFECYCLE: &'static [&'static str] = &[
        "Authorized",
        "Attempted",
        "Realized",
        "Failed",
        "Cancelled",
        "Abandoned",
    ];

    fn for_outcome(
        run_id: &RunId,
        outcome: crate::meerkat_machine::dsl::TurnTerminalOutcome,
    ) -> Vec<Self> {
        let effect = match outcome {
            crate::meerkat_machine::dsl::TurnTerminalOutcome::Completed => {
                RuntimeLoopRunCommitEffect::Completed
            }
            crate::meerkat_machine::dsl::TurnTerminalOutcome::Cancelled => {
                RuntimeLoopRunCommitEffect::Cancelled
            }
            _ => RuntimeLoopRunCommitEffect::Failed,
        };
        vec![Self {
            run_id: run_id.clone(),
            effect,
            closure_policy: "RuntimeLoopRunCommitEffect",
            lifecycle: Self::LIFECYCLE,
        }]
    }

    fn is_satisfied_by(&self, run_id: &RunId, effect: RuntimeLoopRunCommitEffect) -> bool {
        self.run_id == *run_id
            && self.effect == effect
            && self.closure_policy == "RuntimeLoopRunCommitEffect"
            && self.lifecycle == Self::LIFECYCLE
    }
}

#[derive(Debug)]
pub(crate) enum RuntimeLoopRunFailError {
    Rejected(RuntimeDriverError),
    TerminalSnapshot(RuntimeDriverError),
}

impl std::fmt::Display for RuntimeLoopRunFailError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rejected(err) | Self::TerminalSnapshot(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for RuntimeLoopRunFailError {}

pub(crate) fn machine_apply_run_return_projection(
    driver: &mut DriverEntry,
    run_id: &RunId,
    disposition: RunReturnDisposition<'_>,
) -> Result<RuntimeLifecycleProjection, crate::runtime_state::RuntimeStateTransitionError> {
    let current_phase = driver.runtime_state();
    if !matches!(current_phase, RuntimeState::Running) {
        return Ok(RuntimeLifecycleProjection {
            phase: driver.runtime_state(),
            current_run_id: driver.current_run_id(),
            pre_run_phase: driver.pre_run_phase(),
        });
    }

    // DSL is authoritative for `lifecycle_phase` post-#32 W6-J (dogma #1
    // split). Fire the typed `Commit {input_id, run_id}` or `Fail {run_id}`
    // DSL input; the DSL's `CommitRunningTo{Idle,Attached,Retired}` /
    // `FailRunningTo{Idle,Attached,Retired}` transitions dispatch on
    // `pre_run_phase` (set by `Prepare` during `machine_begin_run`) and
    // flip `lifecycle_phase` accordingly. The runtime-loop path owns this
    // DSL transition uniformly; dispatch-ingress no longer snapshots or
    // pre-stages the return input.
    let publish_control_immediately = matches!(driver, DriverEntry::Ephemeral(_))
        || matches!(disposition, RunReturnDisposition::Rollback);
    let authority = driver.shared_dsl_authority();
    let projection = {
        let mut auth = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let input = match disposition {
            RunReturnDisposition::Commit { input_id } => {
                crate::meerkat_machine::dsl::MeerkatMachineInput::Commit {
                    input_id: crate::meerkat_machine::dsl::InputId::from_domain(input_id),
                    run_id: crate::meerkat_machine::dsl::RunId::from_domain(run_id),
                }
            }
            RunReturnDisposition::Failed => {
                crate::meerkat_machine::dsl::MeerkatMachineInput::Fail {
                    run_id: crate::meerkat_machine::dsl::RunId::from_domain(run_id),
                }
            }
            RunReturnDisposition::Cancelled => {
                crate::meerkat_machine::dsl::MeerkatMachineInput::CancelRun {
                    run_id: crate::meerkat_machine::dsl::RunId::from_domain(run_id),
                }
            }
            RunReturnDisposition::Rollback => {
                crate::meerkat_machine::dsl::MeerkatMachineInput::RollbackRun {
                    run_id: crate::meerkat_machine::dsl::RunId::from_domain(run_id),
                }
            }
        };
        if crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(&mut *auth, input).is_err() {
            return Err(crate::runtime_state::RuntimeStateTransitionError {
                from: current_phase,
                to: RuntimeState::Running,
            });
        }
        RuntimeLifecycleProjection::from_authority(&auth)
    };

    // Persistent terminal paths publish the user-visible control projection
    // only after the durable receipt succeeds. Rollback is non-terminal
    // cleanup, and ephemeral drivers have no durable receipt to await.
    if publish_control_immediately {
        driver.set_control_projection(
            projection.phase,
            projection.current_run_id.clone(),
            projection.pre_run_phase,
        );
    }
    Ok(projection)
}

fn service_turn_terminal_is_coherent(
    turn_phase: crate::meerkat_machine::dsl::TurnPhase,
    terminal_outcome: Option<crate::meerkat_machine::dsl::TurnTerminalOutcome>,
    terminal_cause_kind: Option<crate::meerkat_machine::dsl::TurnTerminalCauseKind>,
) -> bool {
    use crate::meerkat_machine::dsl::{TurnPhase, TurnTerminalCauseKind, TurnTerminalOutcome};

    match (turn_phase, terminal_outcome, terminal_cause_kind) {
        (TurnPhase::Completed, Some(TurnTerminalOutcome::Completed), None) => true,
        (TurnPhase::Failed, Some(outcome), Some(cause))
            if cause != TurnTerminalCauseKind::Unknown =>
        {
            outcome
                == match cause {
                    TurnTerminalCauseKind::BudgetExhausted => TurnTerminalOutcome::BudgetExhausted,
                    TurnTerminalCauseKind::TimeBudgetExceeded => {
                        TurnTerminalOutcome::TimeBudgetExceeded
                    }
                    TurnTerminalCauseKind::StructuredOutputValidationFailed => {
                        TurnTerminalOutcome::StructuredOutputValidationFailed
                    }
                    _ => TurnTerminalOutcome::Failed,
                }
        }
        _ => false,
    }
}

pub(crate) async fn machine_commit_service_turn_terminal_receipt(
    driver: &mut DriverEntry,
    session_snapshot: Vec<u8>,
) -> Result<(), RuntimeDriverError> {
    let current_phase = driver.runtime_state();
    if current_phase != RuntimeState::Running {
        return Err(RuntimeDriverError::Internal(format!(
            "service-turn terminal receipt requires a running machine-owned lifecycle; found {current_phase:?}"
        )));
    }
    let active_inputs = driver.as_driver().active_input_ids();
    if !active_inputs.is_empty() {
        return Err(RuntimeDriverError::Internal(format!(
            "direct service turn returned while runtime inputs are still active: {}",
            active_inputs
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    let Some(run_id) = driver.current_run_id() else {
        return Err(RuntimeDriverError::Internal(
            "service-turn terminal receipt requires a machine-owned current_run_id".to_string(),
        ));
    };
    let (
        owner_session_id,
        generated_run_id,
        turn_phase,
        turn_terminal_run_id,
        terminal_outcome,
        terminal_cause_kind,
        primitive_kind,
    ) = {
        let authority = driver.shared_dsl_authority();
        let auth = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (
            auth.state().session_id.clone(),
            auth.state().current_run_id.clone(),
            auth.state().turn_phase,
            auth.state().turn_terminal_run_id.clone(),
            auth.state().terminal_outcome,
            auth.state().terminal_cause_kind,
            auth.state().primitive_kind,
        )
    };
    if generated_run_id.as_ref() != Some(&crate::meerkat_machine::dsl::RunId::from_domain(&run_id))
    {
        return Err(RuntimeDriverError::ValidationFailed {
            reason: "service-turn terminal snapshot did not match the generated current run"
                .to_string(),
        });
    }
    if turn_terminal_run_id.as_ref()
        != Some(&crate::meerkat_machine::dsl::RunId::from_domain(&run_id))
    {
        return Err(RuntimeDriverError::ValidationFailed {
            reason: "service-turn commit lacked an exact generated terminal-run witness"
                .to_string(),
        });
    }
    if !service_turn_terminal_is_coherent(turn_phase, terminal_outcome, terminal_cause_kind) {
        return Err(RuntimeDriverError::ValidationFailed {
            reason: format!(
                "service-turn commit requires an exact coherent generated Completed or Failed terminal; phase={turn_phase:?}, outcome={terminal_outcome:?}, cause={terminal_cause_kind:?}"
            ),
        });
    }
    let owner_session_id = owner_session_id
        .ok_or_else(|| RuntimeDriverError::ValidationFailed {
            reason: "service-turn terminal snapshot lacked generated session ownership".to_string(),
        })
        .and_then(|session_id| {
            SessionId::parse(&session_id.0).map_err(|error| RuntimeDriverError::ValidationFailed {
                reason: format!("generated service-turn session identity was invalid: {error}"),
            })
        })?;
    let session =
        serde_json::from_slice::<meerkat_core::Session>(&session_snapshot).map_err(|error| {
            RuntimeDriverError::ValidationFailed {
                reason: format!("service-turn terminal snapshot was not a Session: {error}"),
            }
        })?;
    if session.id() != &owner_session_id {
        return Err(RuntimeDriverError::ValidationFailed {
            reason: format!(
                "service-turn terminal session owner mismatch: generated {owner_session_id}, snapshot {}",
                session.id()
            ),
        });
    }
    let encoded_messages = serde_json::to_vec(session.messages()).map_err(|error| {
        RuntimeDriverError::ValidationFailed {
            reason: format!(
                "service-turn terminal messages could not be encoded for receipt digest: {error}"
            ),
        }
    })?;
    let boundary = match primitive_kind {
        Some(crate::meerkat_machine::dsl::TurnPrimitiveKind::ConversationTurn) => {
            meerkat_core::lifecycle::run_primitive::RunApplyBoundary::RunStart
        }
        Some(
            crate::meerkat_machine::dsl::TurnPrimitiveKind::ImmediateAppend
            | crate::meerkat_machine::dsl::TurnPrimitiveKind::ImmediateContextAppend,
        ) => meerkat_core::lifecycle::run_primitive::RunApplyBoundary::Immediate,
        other => {
            return Err(RuntimeDriverError::ValidationFailed {
                reason: format!(
                    "service-turn terminal snapshot had no supported generated primitive kind: {other:?}"
                ),
            });
        }
    };
    let receipt = RunBoundaryReceipt {
        run_id: run_id.clone(),
        boundary,
        contributing_input_ids: Vec::new(),
        conversation_digest: Some(format!("{:x}", Sha256::digest(encoded_messages))),
        message_count: session.messages().len(),
        sequence: driver.run_boundary_sequence(&run_id),
    };
    let terminal_checkpoint = driver.rollback_snapshot();
    let authority = driver.shared_dsl_authority();
    let service_turn_commit_result = {
        let mut auth = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(
            &mut *auth,
            crate::meerkat_machine::dsl::MeerkatMachineInput::ServiceTurnCommitted {
                run_id: crate::meerkat_machine::dsl::RunId::from_domain(&run_id),
            },
        )
        .map_err(|_| {
            RuntimeDriverError::Internal(
                crate::runtime_state::RuntimeStateTransitionError {
                    from: current_phase,
                    to: RuntimeState::Running,
                }
                .to_string(),
            )
        })?;
        Ok::<RuntimeLifecycleProjection, RuntimeDriverError>(
            RuntimeLifecycleProjection::from_authority(&auth),
        )
    };
    let projection = match service_turn_commit_result {
        Ok(projection) => projection,
        Err(err) => {
            driver.restore_rollback_snapshot(terminal_checkpoint);
            return Err(err);
        }
    };
    match (driver, terminal_checkpoint) {
        (DriverEntry::Persistent(driver), DriverRollbackSnapshot::Persistent(rollback)) => {
            driver
                .publish_service_turn_terminal(
                    rollback,
                    projection.phase,
                    session_snapshot,
                    receipt,
                    owner_session_id,
                )
                .await?;
        }
        (DriverEntry::Ephemeral(driver), DriverRollbackSnapshot::Ephemeral(_)) => {
            driver.set_control_projection(
                projection.phase,
                projection.current_run_id,
                projection.pre_run_phase,
            );
        }
        (driver, checkpoint) => {
            driver.restore_rollback_snapshot(checkpoint);
            return Err(RuntimeDriverError::Internal(
                "service-turn terminal receipt rollback snapshot/driver kind mismatch".to_string(),
            ));
        }
    }
    Ok(())
}

fn machine_apply_turn_run_completed(
    driver: &mut DriverEntry,
    run_id: &RunId,
) -> Result<(), RuntimeDriverError> {
    let authority = driver.shared_dsl_authority();
    let mut auth = authority
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if auth.state().lifecycle_phase != crate::meerkat_machine::dsl::MeerkatPhase::Running
        || auth.state().current_run_id.as_ref().map(|id| id.0.as_str())
            != Some(run_id.to_string().as_str())
    {
        return Ok(());
    }
    crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(
        &mut *auth,
        crate::meerkat_machine::dsl::MeerkatMachineInput::RunCompleted {
            run_id: crate::meerkat_machine::dsl::RunId::from_domain(run_id),
        },
    )
    .map(|_| ())
    .map_err(|err| {
        RuntimeDriverError::Internal(format!(
            "failed to apply runtime turn completion for run {run_id}: {err}"
        ))
    })
}

fn machine_apply_turn_run_failed(
    driver: &mut DriverEntry,
    run_id: &RunId,
    terminal_error: &str,
    runtime_apply_failure: Option<&CoreApplyFailureCause>,
    machine_terminal_failure_observed: bool,
    terminal_failure_source: Option<crate::meerkat_machine::dsl::RunFailureSourceKind>,
) -> Result<Vec<crate::meerkat_machine::dsl::MeerkatMachineEffect>, RuntimeDriverError> {
    let authority = driver.shared_dsl_authority();
    let mut auth = authority
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if auth.state().lifecycle_phase != crate::meerkat_machine::dsl::MeerkatPhase::Running
        || auth.state().current_run_id.as_ref().map(|id| id.0.as_str())
            != Some(run_id.to_string().as_str())
    {
        return Err(RuntimeDriverError::Internal(format!(
            "generated RunFailed authority absent for run {run_id}: lifecycle={:?}, current_run_id={:?}",
            auth.state().lifecycle_phase,
            auth.state().current_run_id
        )));
    }
    crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(
        &mut *auth,
        crate::meerkat_machine::dsl::MeerkatMachineInput::RunFailed {
            run_id: crate::meerkat_machine::dsl::RunId::from_domain(run_id),
            runtime_apply_failure_cause: runtime_apply_failure
                .map(crate::meerkat_machine::dsl::RuntimeApplyFailureCause::from),
            runtime_apply_failure_message: runtime_apply_failure
                .map(|failure| failure.message().to_owned()),
            machine_terminal_failure_observed,
            terminal_failure_source,
            error: terminal_error.to_owned(),
        },
    )
    .map(|transition| transition.into_effects())
    .map_err(|err| {
        RuntimeDriverError::Internal(format!(
            "failed to apply runtime turn failure for run {run_id}: {err}"
        ))
    })
}

fn machine_apply_turn_run_cancelled(
    driver: &mut DriverEntry,
    run_id: &RunId,
) -> Result<(), RuntimeDriverError> {
    let authority = driver.shared_dsl_authority();
    let mut auth = authority
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if auth.state().lifecycle_phase != crate::meerkat_machine::dsl::MeerkatPhase::Running
        || auth.state().current_run_id.as_ref().map(|id| id.0.as_str())
            != Some(run_id.to_string().as_str())
    {
        return Err(RuntimeDriverError::Internal(format!(
            "generated RunCancelled authority absent for run {run_id}: lifecycle={:?}, current_run_id={:?}",
            auth.state().lifecycle_phase,
            auth.state().current_run_id
        )));
    }
    crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(
        &mut *auth,
        crate::meerkat_machine::dsl::MeerkatMachineInput::RunCancelled {
            run_id: crate::meerkat_machine::dsl::RunId::from_domain(run_id),
        },
    )
    .map(|_| ())
    .map_err(|err| {
        RuntimeDriverError::Internal(format!(
            "failed to apply runtime turn cancellation for run {run_id}: {err}"
        ))
    })
}

#[cfg(test)]
pub(crate) fn machine_input_boundary(
    driver: &DriverEntry,
    work_id: &InputId,
) -> Option<crate::meerkat_machine::dsl::RecoveredRunApplyBoundary> {
    driver.input_runtime_boundary(work_id)
}

#[cfg(test)]
pub(crate) fn machine_input_execution_kind(
    driver: &DriverEntry,
    work_id: &InputId,
) -> Option<crate::meerkat_machine::dsl::RecoveredRuntimeExecutionKind> {
    driver.input_runtime_execution_kind(work_id)
}

#[cfg(test)]
pub(crate) fn machine_input_peer_response_terminal_apply_intent(
    driver: &DriverEntry,
    work_id: &InputId,
) -> Option<crate::meerkat_machine::dsl::RecoveredPeerResponseTerminalApplyIntent> {
    driver.input_peer_response_terminal_apply_intent(work_id)
}

#[cfg(test)]
pub(crate) fn machine_batch_execution_kind(
    driver: &DriverEntry,
    work_ids: &[InputId],
) -> Option<crate::meerkat_machine::dsl::RecoveredRuntimeExecutionKind> {
    let mut semantics = work_ids
        .iter()
        .map(|id| machine_input_execution_kind(driver, id))
        .collect::<Option<Vec<_>>>()?
        .into_iter();
    let first = semantics.next()?;

    if semantics.all(|execution_kind| execution_kind == first) {
        Some(first)
    } else {
        None
    }
}

pub(crate) fn machine_batch_runtime_semantics(
    driver: &DriverEntry,
    work_ids: &[InputId],
) -> Option<Vec<crate::ingress_types::RuntimeInputSemantics>> {
    work_ids
        .iter()
        .map(|id| driver.driver_ingress().runtime_semantics(id))
        .collect()
}

pub(crate) fn machine_batch_primitive_projections(
    driver: &DriverEntry,
    inputs: &[(InputId, Input)],
) -> Option<Vec<crate::ingress_types::RuntimeInputProjection>> {
    let ingress = driver.driver_ingress();
    inputs
        .iter()
        .map(|(id, input)| {
            if matches!(
                input,
                Input::Peer(crate::input::PeerInput {
                    convention: Some(crate::input::PeerConvention::ResponseTerminal { .. }),
                    ..
                })
            ) {
                // ResponseTerminal projections are derived from the input itself,
                // not from the admitted ingress projection.
                Some(crate::input::runtime_input_projection_for_machine_batch(
                    input,
                ))
            } else {
                // Fail closed: the admitted primitive projection is co-recorded
                // with runtime semantics (`record_admission_metadata`), so a
                // missing projection here is a real gap, not a defaultable
                // absence. Returning `None` rejects the whole batch — mirroring
                // `machine_batch_runtime_semantics` — rather than silently
                // feeding a default projection into run-primitive construction.
                ingress.primitive_projection(id)
            }
        })
        .collect()
}

pub(crate) fn machine_authorize_runtime_loop_batch(
    driver: &DriverEntry,
) -> Option<AuthorizedRuntimeLoopBatch> {
    let authority = driver.shared_dsl_authority();
    let plan = {
        let authority = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        generated_command_capabilities::AuthorizedRuntimeLoopBatch::authorize_runtime_loop_batch_from_state(
            authority.state(),
        )
    }?;
    AuthorizedRuntimeLoopBatch::from_generated_plan(plan)
}

pub(crate) fn machine_authorize_stage_for_run(
    driver: &DriverEntry,
    run_id: &RunId,
    input_ids: &[InputId],
    source: RuntimeLoopBatchSource,
) -> Option<AuthorizedStageForRun> {
    let raw_input_ids = input_ids
        .iter()
        .map(std::string::ToString::to_string)
        .collect::<Vec<_>>();
    let dsl_run_id = crate::meerkat_machine::dsl::RunId(run_id.to_string());
    let authority = driver.shared_dsl_authority();
    let plan = {
        let authority = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        generated_command_capabilities::AuthorizedStageForRun::authorize_stage_for_run_from_state(
            authority.state(),
            &raw_input_ids,
            &dsl_run_id,
            source.into(),
        )
    }?;
    AuthorizedStageForRun::from_generated_plan(run_id.clone(), plan)
}

pub(crate) fn machine_validate_boundary_applied(
    driver: &DriverEntry,
    contributing_work_ids: &[InputId],
) -> Result<(), RuntimeDriverError> {
    if contributing_work_ids.is_empty() {
        return Err(RuntimeDriverError::Internal(
            "boundary applied requires at least one contributor".to_string(),
        ));
    }

    for work_id in contributing_work_ids {
        let lifecycle = driver.as_driver().input_phase(work_id);
        if lifecycle != Some(InputLifecycleState::Staged) {
            return Err(RuntimeDriverError::Internal(format!(
                "boundary applied requires staged contributors, but {work_id:?} is {lifecycle:?}"
            )));
        }
    }

    Ok(())
}

pub(crate) fn machine_validate_run_completed(
    driver: &DriverEntry,
    contributing_work_ids: &[InputId],
) -> Result<(), RuntimeDriverError> {
    if contributing_work_ids.is_empty() {
        return Err(RuntimeDriverError::Internal(
            "run completed requires at least one contributor".to_string(),
        ));
    }

    for work_id in contributing_work_ids {
        let lifecycle = driver.as_driver().input_phase(work_id);
        if lifecycle != Some(InputLifecycleState::AppliedPendingConsumption) {
            return Err(RuntimeDriverError::Internal(format!(
                "run completed requires contributors pending consumption, but {work_id:?} is {lifecycle:?}"
            )));
        }
    }

    Ok(())
}

pub(crate) fn machine_validate_run_commit_receipt(
    driver: &DriverEntry,
    run_id: &RunId,
    consumed_input_ids: &[InputId],
    receipt: &meerkat_core::lifecycle::RunBoundaryReceipt,
) -> Result<(), RuntimeDriverError> {
    if &receipt.run_id != run_id {
        return Err(RuntimeDriverError::Internal(format!(
            "run commit receipt run_id {:?} does not match active run {:?}",
            receipt.run_id, run_id
        )));
    }

    if consumed_input_ids != receipt.contributing_input_ids.as_slice() {
        return Err(RuntimeDriverError::Internal(format!(
            "run commit consumed inputs {:?} do not exactly match receipt contributors {:?}",
            consumed_input_ids, receipt.contributing_input_ids
        )));
    }

    machine_validate_boundary_applied(driver, &receipt.contributing_input_ids)
}

pub(crate) fn machine_staged_contributors(driver: &DriverEntry) -> Vec<InputId> {
    driver
        .as_driver()
        .active_input_ids()
        .into_iter()
        .filter(|work_id| {
            driver.as_driver().input_phase(work_id) == Some(InputLifecycleState::Staged)
        })
        .collect()
}

/// Pure observation of the exact validated directed interactions among the
/// current generated Staged contributor set.
///
/// Runtime-loop failure paths use this before realizing `RunFailed` so they can
/// distinguish a genuinely non-directed batch from directed contributors whose
/// rollback may either requeue them or terminalize them at max attempts. The
/// caller must already own the normal B -> M -> driver read order; this helper
/// acquires no locks and mutates no authority.
pub(crate) fn machine_staged_directed_interaction_ids(
    driver: &DriverEntry,
) -> Result<Vec<meerkat_core::interaction::InteractionId>, RuntimeDriverError> {
    let staged_input_ids = machine_staged_contributors(driver);
    let mut directed = Vec::new();
    for input_id in staged_input_ids {
        let stored = driver
            .as_driver()
            .stored_input_state(&input_id)
            .ok_or_else(|| {
                RuntimeDriverError::Internal(format!(
                    "staged contributor {input_id} disappeared during directed-interaction observation"
                ))
            })?;
        let input = stored.state.persisted_input.as_ref().ok_or_else(|| {
            RuntimeDriverError::Internal(format!(
                "staged contributor {input_id} lost its admitted input payload"
            ))
        })?;
        let Some(interaction_id) = crate::input::validated_directed_interaction_id(input)
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?
        else {
            continue;
        };
        if input_id.0 != interaction_id.0 {
            return Err(RuntimeDriverError::ValidationFailed {
                reason: "staged directed input/interaction identity mismatch".to_string(),
            });
        }
        directed.push(interaction_id);
    }
    directed.sort_by_key(|interaction_id| interaction_id.0);
    if directed.len() > 256 {
        return Err(RuntimeDriverError::ValidationFailed {
            reason: "staged directed interaction batch exceeds the 256-input pending bound"
                .to_string(),
        });
    }
    if directed
        .iter()
        .collect::<std::collections::HashSet<_>>()
        .len()
        != directed.len()
    {
        return Err(RuntimeDriverError::ValidationFailed {
            reason: "staged directed interaction batch contains duplicate interactions".to_string(),
        });
    }
    Ok(directed)
}

pub(crate) fn machine_validate_run_failed(
    driver: &DriverEntry,
    contributing_work_ids: &[InputId],
) -> Result<(), RuntimeDriverError> {
    if contributing_work_ids.is_empty() {
        return Err(RuntimeDriverError::Internal(
            "run failed requires at least one staged contributor".to_string(),
        ));
    }

    for work_id in contributing_work_ids {
        let lifecycle = driver.as_driver().input_phase(work_id);
        if lifecycle != Some(InputLifecycleState::Staged) {
            return Err(RuntimeDriverError::Internal(format!(
                "run failed requires staged contributors, but {work_id:?} is {lifecycle:?}"
            )));
        }
    }

    Ok(())
}

pub(crate) fn machine_validate_run_cancelled(
    driver: &DriverEntry,
    contributing_work_ids: &[InputId],
) -> Result<(), RuntimeDriverError> {
    if contributing_work_ids.is_empty() {
        return Err(RuntimeDriverError::Internal(
            "run cancelled requires at least one staged contributor".to_string(),
        ));
    }

    for work_id in contributing_work_ids {
        let lifecycle = driver.as_driver().input_phase(work_id);
        if lifecycle != Some(InputLifecycleState::Staged) {
            return Err(RuntimeDriverError::Internal(format!(
                "run cancelled requires staged contributors, but {work_id:?} is {lifecycle:?}"
            )));
        }
    }

    Ok(())
}

pub(crate) async fn machine_normalize_recovered_input_state(
    store: &dyn crate::store::RuntimeStore,
    runtime_id: &LogicalRuntimeId,
    mut bundle: StoredInputState,
) -> Result<
    (
        StoredInputState,
        Option<crate::meerkat_machine::dsl::RecoveredInputNormalizationReasonKind>,
    ),
    RuntimeDriverError,
> {
    let applied_boundary_committed = if matches!(
        bundle.seed.phase,
        InputLifecycleState::Applied | InputLifecycleState::AppliedPendingConsumption
    ) {
        Some(
            match (
                bundle.seed.last_run_id.clone(),
                bundle.seed.last_boundary_sequence,
            ) {
                (Some(run_id), Some(sequence)) => {
                    load_boundary_receipt_for_runtime(store, runtime_id, &run_id, sequence)
                        .await
                        .map_err(|e| RuntimeDriverError::Internal(e.to_string()))?
                        .is_some()
                }
                _ => false,
            },
        )
    } else {
        None
    };

    let delta =
        machine_apply_recovered_input_normalization(&mut bundle, applied_boundary_committed)?;

    Ok((bundle, delta.admission_sequence_recovery))
}

pub(super) async fn load_boundary_receipt_for_runtime(
    store: &dyn crate::store::RuntimeStore,
    runtime_id: &LogicalRuntimeId,
    run_id: &RunId,
    sequence: u64,
) -> Result<Option<RunBoundaryReceipt>, crate::store::RuntimeStoreError> {
    store
        .load_boundary_receipt(runtime_id, run_id, sequence)
        .await
}

async fn load_input_states_for_runtime(
    store: &dyn crate::store::RuntimeStore,
    runtime_id: &LogicalRuntimeId,
) -> Result<Vec<(LogicalRuntimeId, StoredInputState)>, crate::store::RuntimeStoreError> {
    store.load_input_states(runtime_id).await.map(|states| {
        states
            .into_iter()
            .map(|state| (runtime_id.clone(), state))
            .collect()
    })
}

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct MachineRecoveryDelta {
    pub recovered: usize,
    pub abandoned: usize,
    pub requeued: usize,
    pub admission_sequence_recovery:
        Option<crate::meerkat_machine::dsl::RecoveredInputNormalizationReasonKind>,
}

/// Mechanical, total, identity-preserving projection of the durable runtime
/// `InputLifecycleState` seed phase onto the generated DSL's
/// `RecoveredInputObservedPhase` input type. This is a boundary type-translation
/// only — NOT an authority step: every runtime variant maps 1:1 onto its
/// same-named DSL variant and no phase is collapsed, inferred, or normalized
/// here. All recovery normalization authority lives in the generated
/// `NormalizeRecoveredInputLifecycle` transition, which receives this raw
/// observed phase and decides the normalized lifecycle. The `match` is
/// exhaustive over `InputLifecycleState`, so a new runtime phase fails closed at
/// compile time rather than silently defaulting. Mirrors the reverse
/// DSL→runtime translation in `lifecycle_from_normalized_phase`.
fn recovered_observed_phase(
    phase: InputLifecycleState,
) -> crate::meerkat_machine::dsl::RecoveredInputObservedPhase {
    match phase {
        InputLifecycleState::Accepted => {
            crate::meerkat_machine::dsl::RecoveredInputObservedPhase::Accepted
        }
        InputLifecycleState::Queued => {
            crate::meerkat_machine::dsl::RecoveredInputObservedPhase::Queued
        }
        InputLifecycleState::Staged => {
            crate::meerkat_machine::dsl::RecoveredInputObservedPhase::Staged
        }
        InputLifecycleState::Applied => {
            crate::meerkat_machine::dsl::RecoveredInputObservedPhase::Applied
        }
        InputLifecycleState::AppliedPendingConsumption => {
            crate::meerkat_machine::dsl::RecoveredInputObservedPhase::AppliedPendingConsumption
        }
        InputLifecycleState::Consumed => {
            crate::meerkat_machine::dsl::RecoveredInputObservedPhase::Consumed
        }
        InputLifecycleState::Superseded => {
            crate::meerkat_machine::dsl::RecoveredInputObservedPhase::Superseded
        }
        InputLifecycleState::Coalesced => {
            crate::meerkat_machine::dsl::RecoveredInputObservedPhase::Coalesced
        }
        InputLifecycleState::Abandoned => {
            crate::meerkat_machine::dsl::RecoveredInputObservedPhase::Abandoned
        }
    }
}

fn lifecycle_from_normalized_phase(
    phase: crate::meerkat_machine::dsl::InputPhase,
) -> InputLifecycleState {
    match phase {
        crate::meerkat_machine::dsl::InputPhase::Queued => InputLifecycleState::Queued,
        crate::meerkat_machine::dsl::InputPhase::Staged => InputLifecycleState::Staged,
        crate::meerkat_machine::dsl::InputPhase::Applied => InputLifecycleState::Applied,
        crate::meerkat_machine::dsl::InputPhase::AppliedPendingConsumption => {
            InputLifecycleState::AppliedPendingConsumption
        }
        crate::meerkat_machine::dsl::InputPhase::Consumed => InputLifecycleState::Consumed,
        crate::meerkat_machine::dsl::InputPhase::Superseded => InputLifecycleState::Superseded,
        crate::meerkat_machine::dsl::InputPhase::Coalesced => InputLifecycleState::Coalesced,
        crate::meerkat_machine::dsl::InputPhase::Abandoned => InputLifecycleState::Abandoned,
    }
}

fn terminal_from_normalized_kind(
    kind: Option<crate::meerkat_machine::dsl::InputTerminalKind>,
) -> Result<Option<InputTerminalOutcome>, RuntimeDriverError> {
    match kind {
        None => Ok(None),
        Some(crate::meerkat_machine::dsl::InputTerminalKind::Consumed) => {
            Ok(Some(InputTerminalOutcome::Consumed))
        }
        Some(other) => Err(RuntimeDriverError::Internal(format!(
            "NormalizeRecoveredInputLifecycle emitted terminal kind {other:?} without required payload"
        ))),
    }
}

fn recovery_normalization_reason(
    reason: crate::meerkat_machine::dsl::RecoveredInputNormalizationReasonKind,
) -> &'static str {
    match reason {
        crate::meerkat_machine::dsl::RecoveredInputNormalizationReasonKind::QueueAccepted => {
            "recovery: QueueAccepted"
        }
        crate::meerkat_machine::dsl::RecoveredInputNormalizationReasonKind::RollbackStaged => {
            "recovery: RollbackStaged"
        }
        crate::meerkat_machine::dsl::RecoveredInputNormalizationReasonKind::BoundaryReceiptCommitted => {
            "recovery: boundary receipt already committed"
        }
        crate::meerkat_machine::dsl::RecoveredInputNormalizationReasonKind::MissingBoundaryReceipt => {
            "recovery: missing boundary receipt"
        }
    }
}

pub(crate) fn machine_apply_recovered_input_normalization(
    bundle: &mut StoredInputState,
    applied_boundary_committed: Option<bool>,
) -> Result<MachineRecoveryDelta, RuntimeDriverError> {
    let mut delta = MachineRecoveryDelta::default();
    let StoredInputState { state, seed } = bundle;

    if crate::meerkat_machine::input_seed_behavioral_terminality_via_authority(
        &state.input_id,
        seed,
    )
    .map_err(RuntimeDriverError::Internal)?
    {
        return Ok(delta);
    }

    let input_id = state.input_id.to_string();
    let mut authority = crate::meerkat_machine::dsl::MeerkatMachineAuthority::new();
    let transition = crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(
        &mut authority,
        crate::meerkat_machine::dsl::MeerkatMachineInput::NormalizeRecoveredInputLifecycle {
            input_id: input_id.clone(),
            phase: recovered_observed_phase(seed.phase),
            applied_boundary_committed,
        },
    )
    .map_err(|err| {
        RuntimeDriverError::Internal(format!(
            "NormalizeRecoveredInputLifecycle rejected recovered input '{input_id}': {err:?}"
        ))
    })?;

    let Some((
        effect_input_id,
        normalized_phase,
        terminal_kind,
        recovered,
        abandoned,
        requeued,
        history_reason,
    )) =
        transition.into_effects().into_iter().find_map(|effect| {
            match effect {
        crate::meerkat_machine::dsl::MeerkatMachineEffect::RecoveredInputLifecycleNormalized {
            input_id,
            phase,
            terminal_kind,
            recovered,
            abandoned,
            requeued,
            history_reason,
        } => Some((
            input_id,
            phase,
            terminal_kind,
            recovered,
            abandoned,
            requeued,
            history_reason,
        )),
        _ => None,
    }
        })
    else {
        return Err(RuntimeDriverError::Internal(format!(
            "NormalizeRecoveredInputLifecycle emitted no normalized lifecycle effect for '{input_id}'"
        )));
    };

    if effect_input_id != input_id {
        return Err(RuntimeDriverError::Internal(format!(
            "NormalizeRecoveredInputLifecycle returned input id '{effect_input_id}' for '{input_id}'"
        )));
    }

    let next_phase = lifecycle_from_normalized_phase(normalized_phase);
    let next_terminal = terminal_from_normalized_kind(terminal_kind)?;
    crate::meerkat_machine::input_phase_behavioral_terminality_via_authority(
        &state.input_id,
        next_phase,
        next_terminal.clone(),
    )
    .map_err(RuntimeDriverError::Internal)?;

    let from = seed.phase;
    if history_reason.is_none()
        && (next_phase != seed.phase || next_terminal != seed.terminal_outcome)
    {
        return Err(RuntimeDriverError::Internal(format!(
            "NormalizeRecoveredInputLifecycle changed '{input_id}' without a history reason"
        )));
    }

    if next_phase == InputLifecycleState::Queued && seed.admission_sequence.is_none() {
        delta.admission_sequence_recovery = history_reason;
    }

    if let Some(reason) = history_reason {
        let now = Utc::now();
        state.history.push(InputStateHistoryEntry {
            timestamp: now,
            from,
            to: next_phase,
            reason: Some(recovery_normalization_reason(reason).into()),
        });
        state.updated_at = now;
    }

    seed.phase = next_phase;
    if next_terminal.is_some() {
        seed.recovery_lane = None;
    }
    seed.terminal_outcome = next_terminal;

    if recovered {
        delta.recovered += 1;
    }
    if abandoned {
        delta.abandoned += 1;
    }
    if requeued {
        delta.requeued += 1;
    }

    Ok(delta)
}

pub(crate) fn machine_classify_recovered_input_durability(
    state: &InputState,
) -> Result<crate::meerkat_machine::dsl::RecoveredInputRecoveryDisposition, RuntimeDriverError> {
    let input_id = state.input_id.to_string();
    let mut authority = crate::meerkat_machine::dsl::MeerkatMachineAuthority::new();
    let transition = crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(
        &mut authority,
        crate::meerkat_machine::dsl::MeerkatMachineInput::ClassifyRecoveredInputDurability {
            input_id: input_id.clone(),
            durability: crate::meerkat_machine::dsl::InputDurabilityKind::from(state.durability),
        },
    )
    .map_err(|err| {
        RuntimeDriverError::Internal(format!(
            "ClassifyRecoveredInputDurability rejected recovered input '{input_id}': {err:?}"
        ))
    })?;

    let Some((effect_input_id, disposition)) =
        transition.into_effects().into_iter().find_map(|effect| {
            match effect {
            crate::meerkat_machine::dsl::MeerkatMachineEffect::RecoveredInputDurabilityClassified {
                input_id,
                disposition,
            } => Some((input_id, disposition)),
            _ => None,
        }
        })
    else {
        return Err(RuntimeDriverError::Internal(format!(
            "ClassifyRecoveredInputDurability emitted no retention effect for '{input_id}'"
        )));
    };

    if effect_input_id != input_id {
        return Err(RuntimeDriverError::Internal(format!(
            "ClassifyRecoveredInputDurability returned input id '{effect_input_id}' for '{input_id}'"
        )));
    }

    Ok(disposition)
}

pub(crate) struct RecoveredIngressEntry {
    pub runtime_semantics: crate::ingress_types::RuntimeInputSemantics,
}

pub(crate) fn machine_build_recovered_ingress_entry(
    state: &InputState,
    seed: &InputStateSeed,
) -> Option<RecoveredIngressEntry> {
    state.persisted_input.as_ref()?;
    let runtime_semantics = state.runtime_semantics?;
    seed.recovery_lane?;

    Some(RecoveredIngressEntry { runtime_semantics })
}

fn missing_recovered_ingress_entry_reason(state: &InputState, seed: &InputStateSeed) -> String {
    if state.persisted_input.is_none() {
        return format!(
            "store corruption: recovered input '{}' has no persisted input; cannot derive admitted-input content shape",
            state.input_id
        );
    }
    if state.runtime_semantics.is_none() {
        return format!(
            "store corruption: recovered input '{}' missing runtime execution semantics stamp; cannot recover without runtime-stamped execution kind",
            state.input_id
        );
    }
    if seed.recovery_lane.is_none() {
        return format!(
            "store corruption: recovered input '{}' missing generated recovery lane witness; cannot recover without machine-owned lane metadata",
            state.input_id
        );
    }
    format!(
        "store corruption: recovered input '{}' is missing required admitted-input metadata",
        state.input_id
    )
}

pub(crate) fn machine_recover_ephemeral_driver(
    driver: &mut crate::driver::ephemeral::EphemeralRuntimeDriver,
) -> Result<RecoveryReport, RuntimeDriverError> {
    let mut recovered = 0;
    let mut abandoned = 0;
    let mut requeued = 0;

    // Normalize every active input. Build a bundle from the live driver
    // (ledger + DSL) so the normalization can read/rewrite the seed, then
    // push the normalized bundle back through `admit_recovered_to_ingress`
    // so recovery facts re-enter via typed DSL input.
    let active_ids: Vec<InputId> = driver.active_input_ids();

    let mut normalized: Vec<(InputId, StoredInputState, MachineRecoveryDelta)> =
        Vec::with_capacity(active_ids.len());
    for input_id in &active_ids {
        let Some(mut bundle) = driver.stored_input_state(input_id) else {
            continue;
        };
        let delta = machine_apply_recovered_input_normalization(&mut bundle, None)?;
        recovered += delta.recovered;
        abandoned += delta.abandoned;
        requeued += delta.requeued;
        normalized.push((input_id.clone(), bundle, delta));
    }

    // Replay recovered lifecycle facts through the driver's DSL authority.
    // No rebuilt authority — the DSL is the only owner of recovered phase,
    // run/boundary associations, typed terminal metadata, attempt count, and
    // lane membership.
    let mut recovered_entries: Vec<(
        InputId,
        RecoveredIngressEntry,
        InputState,
        InputStateSeed,
        Option<crate::meerkat_machine::dsl::RecoveredInputNormalizationReasonKind>,
    )> = Vec::with_capacity(normalized.len());
    for (input_id, bundle, delta) in normalized {
        let Some(entry) = machine_build_recovered_ingress_entry(&bundle.state, &bundle.seed) else {
            return Err(RuntimeDriverError::Internal(
                missing_recovered_ingress_entry_reason(&bundle.state, &bundle.seed),
            ));
        };
        recovered_entries.push((
            input_id,
            entry,
            bundle.state,
            bundle.seed,
            delta.admission_sequence_recovery,
        ));
    }

    for (input_id, entry, state, seed, admission_sequence_recovery) in recovered_entries {
        driver.admit_recovered_to_ingress(
            input_id.clone(),
            entry.runtime_semantics,
            &state,
            &seed,
            None,
            None,
            admission_sequence_recovery,
        )?;
        // Persist the normalized shell back into the ledger only after
        // generated recovered-admission authority accepts the witness.
        if let Some(ledger_slot) = driver.ledger_mut().get_mut(&input_id) {
            *ledger_slot = state.clone();
        }
    }

    driver.rebuild_queue_projections_after_recovery();

    Ok(RecoveryReport {
        inputs_recovered: recovered,
        inputs_abandoned: abandoned,
        inputs_requeued: requeued,
        details: Vec::new(),
    })
}

pub(crate) struct ReconciledRuntimeAuthority {
    pub(crate) authority: crate::meerkat_machine::dsl::MeerkatMachineAuthority,
    pub(crate) unregister_progress: Option<crate::store::MachineUnregisterProgressSnapshot>,
}

fn runtime_authority_reconcile_decision(
    observation_kind: crate::meerkat_machine::dsl::RuntimeAuthorityObservationKind,
    observation: Option<&crate::store::DecodedMachineLifecycleObservation>,
) -> Result<crate::meerkat_machine::dsl::RuntimeAuthorityReconcileDecision, RuntimeDriverError> {
    let state = observation
        .and_then(crate::store::DecodedMachineLifecycleObservation::runtime_state)
        .map(crate::meerkat_machine::dsl_authority::observed_runtime_lifecycle_state);
    let binding = observation.map(crate::store::DecodedMachineLifecycleObservation::binding);
    let run = observation.map(crate::store::DecodedMachineLifecycleObservation::run);
    let pre_run_phase = run
        .and_then(crate::store::MachineLifecycleRunFacts::pre_run_phase)
        .map(|phase| match phase {
            crate::store::MachineLifecyclePreRunPhase::Idle => {
                crate::meerkat_machine::dsl::PreRunPhase::Idle
            }
            crate::store::MachineLifecyclePreRunPhase::Attached => {
                crate::meerkat_machine::dsl::PreRunPhase::Attached
            }
            crate::store::MachineLifecyclePreRunPhase::Retired => {
                crate::meerkat_machine::dsl::PreRunPhase::Retired
            }
        });
    let mut authority = crate::meerkat_machine::dsl::MeerkatMachineAuthority::new();
    let transition = crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(
        &mut authority,
        crate::meerkat_machine::dsl::MeerkatMachineInput::ClassifyRuntimeAuthorityReconciliation {
            observation_kind,
            state,
            agent_runtime_id: binding
                .and_then(crate::store::MachineLifecycleBindingFacts::agent_runtime_id)
                .map(crate::meerkat_machine::dsl::AgentRuntimeId::from),
            fence_token: binding
                .and_then(crate::store::MachineLifecycleBindingFacts::fence_token)
                .map(crate::meerkat_machine::dsl::FenceToken::from),
            runtime_generation: binding
                .and_then(crate::store::MachineLifecycleBindingFacts::runtime_generation)
                .map(crate::meerkat_machine::dsl::Generation::from),
            runtime_epoch_id: binding
                .and_then(crate::store::MachineLifecycleBindingFacts::runtime_epoch_id)
                .map(crate::meerkat_machine::dsl::RuntimeEpochId::from),
            current_run_id: run
                .and_then(crate::store::MachineLifecycleRunFacts::current_run_id)
                .map(crate::meerkat_machine::dsl::RunId::from_domain),
            pre_run_phase,
            malformed_reclaim_safe: false,
        },
    )
    .map_err(|error| {
        RuntimeDriverError::Internal(format!(
            "generated runtime-authority classifier rejected a typed observation: {error}"
        ))
    })?;
    transition
        .into_effects()
        .into_iter()
        .find_map(|effect| match effect {
            crate::meerkat_machine::dsl::MeerkatMachineEffect::RuntimeAuthorityReconciliationClassified { decision } => Some(decision),
            _ => None,
        })
        .ok_or_else(|| RuntimeDriverError::Internal(
            "generated runtime-authority classifier emitted no decision".to_string(),
        ))
}

pub(crate) fn runtime_authority_reconcile_decision_for_observation(
    observed: &crate::store::MachineLifecycleObservation,
) -> Result<crate::meerkat_machine::dsl::RuntimeAuthorityReconcileDecision, RuntimeDriverError> {
    let (kind, decoded) = match observed {
        crate::store::MachineLifecycleObservation::Missing => (
            crate::meerkat_machine::dsl::RuntimeAuthorityObservationKind::Missing,
            None,
        ),
        crate::store::MachineLifecycleObservation::Decoded { record, .. } => (
            crate::meerkat_machine::dsl::RuntimeAuthorityObservationKind::Decoded,
            Some(record),
        ),
        crate::store::MachineLifecycleObservation::Unsupported { .. } => (
            crate::meerkat_machine::dsl::RuntimeAuthorityObservationKind::Unsupported,
            None,
        ),
        crate::store::MachineLifecycleObservation::Malformed { .. } => (
            crate::meerkat_machine::dsl::RuntimeAuthorityObservationKind::Malformed,
            None,
        ),
    };
    runtime_authority_reconcile_decision(kind, decoded)
}

pub(crate) fn registered_runtime_authority_from_converged_record(
    session_id: &SessionId,
    record: &crate::store::DecodedMachineLifecycleObservation,
) -> Result<ReconciledRuntimeAuthority, RuntimeDriverError> {
    registered_runtime_authority_from_converged_record_id(
        &crate::meerkat_machine::dsl::SessionId::from_domain(session_id),
        record,
    )
}

fn registered_runtime_authority_from_converged_record_id(
    session_id: &crate::meerkat_machine::dsl::SessionId,
    record: &crate::store::DecodedMachineLifecycleObservation,
) -> Result<ReconciledRuntimeAuthority, RuntimeDriverError> {
    let mut authority =
        crate::meerkat_machine::dsl_authority::new_registered_authority_id(session_id.clone())
            .map_err(|error| {
                RuntimeDriverError::Internal(crate::meerkat_machine::dsl_authority::map_error(
                    error,
                    "fresh cold runtime registration",
                ))
            })?;
    crate::meerkat_machine::dsl_authority::recover_supervisor_authority_snapshot(
        &mut authority,
        record.supervisor_authority().clone(),
    )
    .map_err(|error| RuntimeDriverError::RecoveryCorruption {
        reason: crate::meerkat_machine::dsl_authority::map_error(
            error,
            "independent supervisor custody recovery",
        ),
    })?;
    Ok(ReconciledRuntimeAuthority {
        authority,
        unregister_progress: record.unregister_progress().cloned(),
    })
}

/// Level-triggered cold convergence for the one runtime-lifecycle row.
///
/// The observed phase is never adopted. Every non-fixed decoded shape is
/// replaced with a clean unbound Idle projection using the exact raw row
/// version, conflicts restart from observation, and only then is a fresh
/// Initialize/RegisterSession authority constructed. Supervisor and unregister
/// custody are recovered independently from the converged row.
pub(crate) async fn reconcile_runtime_authority_for_cold_recovery(
    store: &dyn crate::store::RuntimeStore,
    runtime_id: &LogicalRuntimeId,
    session_id: &SessionId,
) -> Result<ReconciledRuntimeAuthority, RuntimeDriverError> {
    reconcile_runtime_authority_for_cold_recovery_id(
        store,
        runtime_id,
        &crate::meerkat_machine::dsl::SessionId::from_domain(session_id),
    )
    .await
}

async fn reconcile_runtime_authority_for_cold_recovery_id(
    store: &dyn crate::store::RuntimeStore,
    runtime_id: &LogicalRuntimeId,
    session_id: &crate::meerkat_machine::dsl::SessionId,
) -> Result<ReconciledRuntimeAuthority, RuntimeDriverError> {
    loop {
        let observed = match store.observe_machine_lifecycle(runtime_id).await {
            Ok(observed) => observed,
            Err(crate::store::RuntimeStoreError::Unsupported(reason)) => {
                let decision = runtime_authority_reconcile_decision(
                    crate::meerkat_machine::dsl::RuntimeAuthorityObservationKind::Unsupported,
                    None,
                )?;
                debug_assert_eq!(
                    decision,
                    crate::meerkat_machine::dsl::RuntimeAuthorityReconcileDecision::RepairBlocked
                );
                return Err(RuntimeDriverError::RecoveryRepairBlocked {
                    evidence_digest: None,
                    reason: format!("runtime lifecycle observation is unsupported: {reason}"),
                });
            }
            Err(error) => {
                let decision = runtime_authority_reconcile_decision(
                    crate::meerkat_machine::dsl::RuntimeAuthorityObservationKind::Unavailable,
                    None,
                )?;
                debug_assert_eq!(
                    decision,
                    crate::meerkat_machine::dsl::RuntimeAuthorityReconcileDecision::Backoff
                );
                return Err(RuntimeDriverError::RecoveryBackoff {
                    reason: error.to_string(),
                });
            }
        };
        match runtime_authority_reconcile_decision_for_observation(&observed)? {
            crate::meerkat_machine::dsl::RuntimeAuthorityReconcileDecision::Converged => {
                let crate::store::MachineLifecycleObservation::Decoded { record, .. } = observed
                else {
                    return Err(RuntimeDriverError::Internal(
                        "runtime-authority classifier converged a non-decoded row".to_string(),
                    ));
                };
                return registered_runtime_authority_from_converged_record_id(session_id, &record);
            }
            crate::meerkat_machine::dsl::RuntimeAuthorityReconcileDecision::NormalizeOrReplace => {
                let expected = match observed.version() {
                    Some(version) => {
                        crate::store::MachineLifecycleExpectedVersion::Version(version.clone())
                    }
                    None => crate::store::MachineLifecycleExpectedVersion::Missing,
                };
                let replacement =
                    crate::store::MachineLifecycleCommit::new_with_binding_and_unregister_progress(
                        RuntimeState::Idle,
                        crate::store::MachineLifecycleBindingFacts::default(),
                        crate::store::SupervisorAuthoritySnapshot::UnboundNoReceipt,
                        None,
                    );
                match store
                    .compare_and_swap_machine_lifecycle(runtime_id, expected, replacement)
                    .await
                {
                    Ok(crate::store::MachineLifecycleCasOutcome::Applied { .. }) => continue,
                    Ok(crate::store::MachineLifecycleCasOutcome::Conflict { .. }) => {
                        // A concurrent writer won the target-local CAS. The
                        // caller owns scheduling: return a typed requeue rather
                        // than spinning inside one recovery invocation.
                        return Err(RuntimeDriverError::RecoveryBackoff {
                            reason: "runtime lifecycle CAS conflicted; re-observe on the next reconciliation pass".to_string(),
                        });
                    }
                    Err(crate::store::RuntimeStoreError::MachineLifecycleRepairBlocked {
                        evidence_digest,
                        detail,
                    }) => {
                        return Err(RuntimeDriverError::RecoveryRepairBlocked {
                            evidence_digest,
                            reason: detail,
                        });
                    }
                    Err(error) => {
                        return Err(RuntimeDriverError::RecoveryBackoff {
                            reason: error.to_string(),
                        });
                    }
                }
            }
            crate::meerkat_machine::dsl::RuntimeAuthorityReconcileDecision::RepairBlocked
            | crate::meerkat_machine::dsl::RuntimeAuthorityReconcileDecision::Quarantine => {
                return Err(RuntimeDriverError::RecoveryRepairBlocked {
                    evidence_digest: observed.evidence_digest().map(ToOwned::to_owned),
                    reason: "runtime lifecycle evidence cannot be normalized safely".to_string(),
                });
            }
            crate::meerkat_machine::dsl::RuntimeAuthorityReconcileDecision::Backoff => {
                return Err(RuntimeDriverError::RecoveryBackoff {
                    reason: "runtime lifecycle observation is temporarily unavailable".to_string(),
                });
            }
        }
    }
}

pub(crate) async fn machine_recover_persistent_driver(
    store: &dyn crate::store::RuntimeStore,
    runtime_id: &LogicalRuntimeId,
    driver: &mut crate::driver::ephemeral::EphemeralRuntimeDriver,
) -> Result<RecoveryReport, RuntimeDriverError> {
    let session_id = driver.session_authority_id_for_recovery();
    let reconciled =
        reconcile_runtime_authority_for_cold_recovery_id(store, runtime_id, &session_id).await?;
    let recovered_unregister_progress = reconciled.unregister_progress;
    driver.replace_runtime_authority(reconciled.authority);

    machine_recover_persistent_inputs(
        store,
        runtime_id,
        driver,
        recovered_unregister_progress.as_ref(),
    )
    .await
}

/// Recover durable input work after the caller has already converged and
/// installed fresh runtime lifecycle authority.
///
/// Cold registration uses this path so the lifecycle row has exactly one
/// observe/classify/CAS owner. Direct `PersistentRuntimeDriver::recover`
/// retains the wrapper above for compatibility with callers that have not
/// established runtime authority yet.
pub(crate) async fn machine_recover_persistent_inputs(
    store: &dyn crate::store::RuntimeStore,
    runtime_id: &LogicalRuntimeId,
    driver: &mut crate::driver::ephemeral::EphemeralRuntimeDriver,
    recovered_unregister_progress: Option<&crate::store::MachineUnregisterProgressSnapshot>,
) -> Result<RecoveryReport, RuntimeDriverError> {
    let mut recovered_payloads = Vec::new();

    for (_stored_runtime_id, bundle) in load_input_states_for_runtime(store, runtime_id)
        .await
        .map_err(|e| RuntimeDriverError::Internal(e.to_string()))?
    {
        let (bundle, admission_sequence_recovery) =
            machine_normalize_recovered_input_state(store, runtime_id, bundle).await?;

        if matches!(
            machine_classify_recovered_input_durability(&bundle.state)?,
            crate::meerkat_machine::dsl::RecoveredInputRecoveryDisposition::Discard
        ) {
            continue;
        }

        if driver.input_state(&bundle.state.input_id).is_none() {
            if crate::meerkat_machine::input_seed_behavioral_terminality_via_authority(
                &bundle.state.input_id,
                &bundle.seed,
            )
            .map_err(RuntimeDriverError::Internal)?
            {
                driver.recover_terminal_input_lifecycle(
                    &bundle.state.input_id,
                    &bundle.seed,
                    bundle.state.idempotency_key.as_ref(),
                )?;
                let inserted = driver.ledger_mut().recover(bundle.state.clone());
                if !inserted {
                    continue;
                }
                continue;
            }

            let Some(entry) = machine_build_recovered_ingress_entry(&bundle.state, &bundle.seed)
            else {
                return Err(RuntimeDriverError::Internal(
                    missing_recovered_ingress_entry_reason(&bundle.state, &bundle.seed),
                ));
            };

            driver.admit_recovered_to_ingress(
                bundle.state.input_id.clone(),
                entry.runtime_semantics,
                &bundle.state,
                &bundle.seed,
                None,
                None,
                admission_sequence_recovery,
            )?;

            let inserted = driver.ledger_mut().recover(bundle.state.clone());
            if !inserted {
                continue;
            }

            if let Some(input) = bundle.state.persisted_input.clone() {
                recovered_payloads.push((bundle.state.input_id.clone(), input));
            }
        }
    }

    let report = machine_recover_ephemeral_driver(driver)?;

    // Generic input recovery may rebuild the ordinary runtime phase from
    // stored ingress. The durable unregister prefix is later lifecycle
    // authority, so replay it after that reconstruction and before the
    // PersistentRuntimeDriver persists the recovered image.
    if let Some(progress) = recovered_unregister_progress {
        let session_id = driver.session_authority_id_for_recovery();
        let authority = driver.shared_dsl_authority();
        {
            let mut authority = authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            super::session_management::replay_durable_unregister_progress_id(
                &mut authority,
                &session_id,
                Some(progress),
            )?;
        }
        driver.sync_control_projection_from_dsl_authority();
    }

    for (input_id, _input) in recovered_payloads {
        let should_requeue =
            driver.input_phase(&input_id) == Some(crate::input_state::InputLifecycleState::Queued);
        if should_requeue && !driver.has_queued_input(&input_id) {
            return Err(RuntimeDriverError::Internal(format!(
                "persistent recover left queued input '{input_id}' out of the runtime queue projection"
            )));
        }
    }

    Ok(report)
}

pub(crate) fn machine_build_replay_plan(
    driver: &DriverEntry,
    contributing_work_ids: &[InputId],
    notice_kind: &'static str,
) -> crate::driver::ephemeral::ReplayQueuedContributorsPlan {
    let mut queue_work_ids = Vec::new();
    let mut steer_work_ids = Vec::new();
    for work_id in contributing_work_ids {
        match driver.driver_ingress().handling_mode(work_id) {
            Some(meerkat_core::types::HandlingMode::Steer) => steer_work_ids.push(work_id.clone()),
            _ => queue_work_ids.push(work_id.clone()),
        }
    }
    crate::driver::ephemeral::ReplayQueuedContributorsPlan {
        queue_work_ids,
        steer_work_ids,
        notice_kind,
    }
}

pub(crate) async fn machine_stop_runtime(
    driver: &mut DriverEntry,
) -> Result<(), RuntimeDriverError> {
    match driver {
        DriverEntry::Ephemeral(d) => {
            d.apply_runtime_executor_exited_authority()?;
            d.sync_control_projection_from_dsl_authority();
            d.finalize_stop_runtime()?;
            Ok(())
        }
        DriverEntry::Persistent(d) => d.finalize_runtime_executor_exit().await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queued_seed() -> InputStateSeed {
        let mut seed = InputStateSeed::new_accepted();
        seed.phase = InputLifecycleState::Queued;
        seed.admission_sequence = Some(1_000_000_000_000);
        seed.recovery_lane = Some(meerkat_core::types::HandlingMode::Queue);
        seed
    }

    fn queue_policy(
        wake_mode: crate::policy::WakeMode,
        drain_policy: crate::policy::DrainPolicy,
    ) -> crate::policy::PolicyDecision {
        crate::policy::PolicyDecision {
            apply_mode: crate::policy::ApplyMode::StageRunStart,
            wake_mode,
            queue_mode: crate::policy::QueueMode::Fifo,
            consume_point: crate::policy::ConsumePoint::OnRunComplete,
            drain_policy,
            routing_disposition: crate::policy::RoutingDisposition::Queue,
            record_transcript: true,
            emit_operator_content: true,
            policy_version: crate::policy_table::generated_default_policy_version(),
        }
    }

    fn generated_runtime_semantics(input: &Input) -> crate::ingress_types::RuntimeInputSemantics {
        crate::ingress_types::RuntimeInputSemantics::try_from_generated_admission(input, true)
            .expect("generated admission semantics")
    }

    fn authorized_batch_input_ids(driver: &DriverEntry) -> Vec<InputId> {
        machine_authorize_runtime_loop_batch(driver)
            .expect("generated runtime-loop batch authority")
            .input_ids()
            .to_vec()
    }

    #[test]
    fn machine_batch_execution_kind_requires_admitted_semantics() {
        let driver = DriverEntry::Ephemeral(EphemeralRuntimeDriver::new(
            crate::identifiers::LogicalRuntimeId::new("test"),
        ));
        let unstamped_input = InputId::new();

        assert_eq!(
            machine_batch_execution_kind(&driver, &[unstamped_input]),
            None,
            "missing runtime semantics must not locally default to ContentTurn"
        );
    }

    #[test]
    fn machine_input_boundary_requires_admitted_semantics() {
        let driver = DriverEntry::Ephemeral(EphemeralRuntimeDriver::new(
            crate::identifiers::LogicalRuntimeId::new("boundary-test"),
        ));
        let unstamped_input = InputId::new();

        assert_eq!(
            machine_input_boundary(&driver, &unstamped_input),
            None,
            "missing runtime semantics must not locally default to RunStart"
        );
    }

    #[tokio::test]
    async fn batch_selection_drains_queue_next_turn_after_prior_run_even_without_wake() {
        let mut driver = EphemeralRuntimeDriver::new(crate::identifiers::LogicalRuntimeId::new(
            "queue-next-turn-no-wake",
        ));
        driver
            .contract_begin_run_authority(RunId::new())
            .expect("test run must start through generated authority");
        let input = Input::Prompt(crate::input::PromptInput::new(
            "queued behind an active turn",
            None,
        ));
        let input_id = input.id().clone();
        let outcome = crate::traits::RuntimeDriver::accept_input(&mut driver, input)
            .await
            .expect("accept queued input behind active run");
        match outcome {
            AcceptOutcome::Accepted { policy, .. } => {
                assert_eq!(policy.wake_mode, crate::policy::WakeMode::None);
                assert_eq!(
                    policy.drain_policy,
                    crate::policy::DrainPolicy::QueueNextTurn
                );
            }
            other => panic!("expected accepted queued input, got {other:?}"),
        }

        let selected = authorized_batch_input_ids(&DriverEntry::Ephemeral(driver));

        assert_eq!(
            selected,
            vec![input_id],
            "QueueNextTurn is generated drain authority; WakeMode::None only suppresses the immediate wake"
        );
    }

    #[test]
    fn recovered_ingress_entry_requires_persisted_input_payload() {
        let mut state = InputState::new_accepted(InputId::new());
        state.policy = Some(crate::input_state::PolicySnapshot {
            version: crate::identifiers::PolicyVersion(1),
            decision: queue_policy(
                crate::policy::WakeMode::WakeIfIdle,
                crate::policy::DrainPolicy::QueueNextTurn,
            ),
        });

        assert!(
            machine_build_recovered_ingress_entry(&state, &queued_seed()).is_none(),
            "recovery must not infer Prompt/ContentTurn when persisted input payload is missing"
        );
    }

    #[test]
    fn recovered_ingress_admission_rejects_mismatched_runtime_semantics() {
        let mut driver = EphemeralRuntimeDriver::new(crate::identifiers::LogicalRuntimeId::new(
            "recovered-admission-mismatch",
        ));
        let input = Input::Prompt(crate::input::PromptInput::new("queued prompt", None));
        let input_id = input.id().clone();
        let mut state = InputState::new_accepted(input_id.clone());
        state.persisted_input = Some(input.clone());
        let seed = queued_seed();
        assert!(driver.ledger_mut().recover(state.clone()));
        let mut runtime_semantics = generated_runtime_semantics(&input);
        runtime_semantics.execution_kind =
            meerkat_core::lifecycle::RuntimeExecutionKind::ResumePending;

        let err = driver
            .admit_recovered_to_ingress(
                input_id.clone(),
                runtime_semantics,
                &state,
                &seed,
                None,
                None,
                None,
            )
            .expect_err("lower-level recovered admission must reject contradictory stamps");

        assert!(
            err.to_string()
                .contains("generated recovered-admission authority"),
            "unexpected recovery error: {err}"
        );
        assert!(
            driver.admitted_runtime_semantics(&input_id).is_none(),
            "failed recovered admission must not record mismatched runtime semantics"
        );
    }

    #[test]
    fn prompt_batch_selection_drives_incompatible_prefix_before_prompt() {
        let mut driver = EphemeralRuntimeDriver::new(crate::identifiers::LogicalRuntimeId::new(
            "mixed-prefix-test",
        ));
        let resume_input = Input::Continuation(
            crate::input::ContinuationInput::detached_background_op_completed(),
        );
        let prompt_input = Input::Prompt(crate::input::PromptInput {
            injected_context: Vec::new(),
            header: crate::input::InputHeader {
                id: InputId::new(),
                timestamp: chrono::Utc::now(),
                source: crate::input::InputOrigin::Operator,
                durability: crate::input::InputDurability::Durable,
                visibility: crate::input::InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            content: "drive the queue".into(),
            typed_turn_appends: Vec::new(),
            turn_metadata: None,
        });
        let resume_id = resume_input.id().clone();
        let prompt_id = prompt_input.id().clone();
        let mut resume_state = InputState::new_accepted(resume_id.clone());
        resume_state.persisted_input = Some(resume_input.clone());
        let mut prompt_state = InputState::new_accepted(prompt_id.clone());
        prompt_state.persisted_input = Some(prompt_input.clone());
        let seed = queued_seed();
        assert!(driver.ledger_mut().recover(resume_state.clone()));
        assert!(driver.ledger_mut().recover(prompt_state.clone()));

        driver
            .admit_recovered_to_ingress(
                resume_id.clone(),
                crate::ingress_types::RuntimeInputSemantics {
                    boundary:
                        meerkat_core::lifecycle::run_primitive::RunApplyBoundary::RunCheckpoint,
                    execution_kind: meerkat_core::lifecycle::RuntimeExecutionKind::ResumePending,
                    execution_handling_mode: None,
                    peer_response_terminal_apply_intent: None,
                    live_interrupt_required: false,
                },
                &resume_state,
                &seed,
                None,
                None,
                None,
            )
            .expect("recover queued resume input");
        driver
            .admit_recovered_to_ingress(
                prompt_id.clone(),
                crate::ingress_types::RuntimeInputSemantics {
                    boundary: meerkat_core::lifecycle::run_primitive::RunApplyBoundary::RunStart,
                    execution_kind: meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn,
                    execution_handling_mode: None,
                    peer_response_terminal_apply_intent: None,
                    live_interrupt_required: false,
                },
                &prompt_state,
                &seed,
                None,
                None,
                None,
            )
            .expect("recover queued prompt input");
        driver.rebuild_queue_projections_after_recovery();

        let entry = DriverEntry::Ephemeral(driver);
        let selected = authorized_batch_input_ids(&entry);

        assert_eq!(
            selected,
            vec![resume_id],
            "a later prompt may drive the queue, but selection must preserve the staged queue prefix when an older input has a different execution kind"
        );
    }

    #[test]
    fn batch_selection_surfaces_unstamped_no_wake_prefix_before_prompt() {
        let mut driver = EphemeralRuntimeDriver::new(crate::identifiers::LogicalRuntimeId::new(
            "missing-prefix-before-prompt-selection",
        ));
        let prefix_input = Input::Continuation(
            crate::input::ContinuationInput::detached_background_op_completed(),
        );
        let prompt_input = Input::Prompt(crate::input::PromptInput::new("drive the queue", None));
        let prefix_id = prefix_input.id().clone();
        let prompt_id = prompt_input.id().clone();
        let mut prefix_state = InputState::new_accepted(prefix_id.clone());
        prefix_state.persisted_input = Some(prefix_input.clone());
        let mut prompt_state = InputState::new_accepted(prompt_id.clone());
        prompt_state.persisted_input = Some(prompt_input.clone());
        let seed = queued_seed();
        assert!(driver.ledger_mut().recover(prefix_state.clone()));
        assert!(driver.ledger_mut().recover(prompt_state.clone()));

        driver
            .admit_recovered_to_ingress(
                prefix_id.clone(),
                crate::ingress_types::RuntimeInputSemantics {
                    boundary:
                        meerkat_core::lifecycle::run_primitive::RunApplyBoundary::RunCheckpoint,
                    execution_kind: meerkat_core::lifecycle::RuntimeExecutionKind::ResumePending,
                    execution_handling_mode: None,
                    peer_response_terminal_apply_intent: None,
                    live_interrupt_required: false,
                },
                &prefix_state,
                &seed,
                None,
                None,
                None,
            )
            .expect("recover queued prefix input");
        driver
            .admit_recovered_to_ingress(
                prompt_id,
                crate::ingress_types::RuntimeInputSemantics {
                    boundary: meerkat_core::lifecycle::run_primitive::RunApplyBoundary::RunStart,
                    execution_kind: meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn,
                    execution_handling_mode: None,
                    peer_response_terminal_apply_intent: None,
                    live_interrupt_required: false,
                },
                &prompt_state,
                &seed,
                None,
                None,
                None,
            )
            .expect("recover queued prompt input");
        driver.rebuild_queue_projections_after_recovery();
        driver.clear_admitted_runtime_semantics_for_test(&prefix_id);

        let entry = DriverEntry::Ephemeral(driver);
        let selected = authorized_batch_input_ids(&entry);

        assert_eq!(
            selected,
            vec![prefix_id],
            "an unstamped no-wake prefix entry must be selected before the later prompt so runtime-loop failure handling can consume the queue prefix"
        );
        assert!(
            machine_batch_runtime_semantics(&entry, &selected).is_none(),
            "selected unstamped prefix must flow into the runtime-loop metadata conflict path"
        );
    }

    #[test]
    fn batch_selection_non_prompt_driver_stops_before_following_prompt() {
        let mut driver = EphemeralRuntimeDriver::new(crate::identifiers::LogicalRuntimeId::new(
            "non-prompt-driver-before-prompt-selection",
        ));
        let event_input = Input::ExternalEvent(crate::input::ExternalEventInput {
            objective_id: None,
            header: crate::input::InputHeader {
                id: InputId::new(),
                timestamp: chrono::Utc::now(),
                source: crate::input::InputOrigin::External {
                    source_name: "scheduler".into(),
                },
                durability: crate::input::InputDurability::Durable,
                visibility: crate::input::InputVisibility::default(),
                idempotency_key: None,
                supersession_key: None,
                correlation_id: None,
            },
            event_type: "scheduler_tick".into(),
            payload: serde_json::json!({"body": "tick"}),
            blocks: None,
            handling_mode: meerkat_core::types::HandlingMode::Queue,
            render_metadata: None,
        });
        let prompt_input = Input::Prompt(crate::input::PromptInput::new(
            "operator prompt must wait",
            None,
        ));
        let event_id = event_input.id().clone();
        let prompt_id = prompt_input.id().clone();
        let mut event_state = InputState::new_accepted(event_id.clone());
        event_state.persisted_input = Some(event_input.clone());
        let mut prompt_state = InputState::new_accepted(prompt_id.clone());
        prompt_state.persisted_input = Some(prompt_input.clone());
        let seed = queued_seed();
        assert!(driver.ledger_mut().recover(event_state.clone()));
        assert!(driver.ledger_mut().recover(prompt_state.clone()));

        driver
            .admit_recovered_to_ingress(
                event_id.clone(),
                crate::ingress_types::RuntimeInputSemantics {
                    boundary: meerkat_core::lifecycle::run_primitive::RunApplyBoundary::RunStart,
                    execution_kind: meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn,
                    execution_handling_mode: None,
                    peer_response_terminal_apply_intent: None,
                    live_interrupt_required: false,
                },
                &event_state,
                &seed,
                None,
                None,
                None,
            )
            .expect("recover queued event input");
        driver
            .admit_recovered_to_ingress(
                prompt_id,
                crate::ingress_types::RuntimeInputSemantics {
                    boundary: meerkat_core::lifecycle::run_primitive::RunApplyBoundary::RunStart,
                    execution_kind: meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn,
                    execution_handling_mode: None,
                    peer_response_terminal_apply_intent: None,
                    live_interrupt_required: false,
                },
                &prompt_state,
                &seed,
                None,
                None,
                None,
            )
            .expect("recover queued prompt input");
        driver.rebuild_queue_projections_after_recovery();

        let entry = DriverEntry::Ephemeral(driver);
        let selected = authorized_batch_input_ids(&entry);

        assert_eq!(
            selected,
            vec![event_id],
            "a non-prompt-driven batch must not absorb a following prompt into the same run"
        );
    }

    #[test]
    fn batch_selection_surfaces_queued_input_missing_runtime_semantics() {
        let mut driver = EphemeralRuntimeDriver::new(crate::identifiers::LogicalRuntimeId::new(
            "missing-runtime-semantics-selection",
        ));
        let input = Input::Prompt(crate::input::PromptInput::new(
            "unstamped queued prompt",
            None,
        ));
        let input_id = input.id().clone();
        let mut state = InputState::new_accepted(input_id.clone());
        state.persisted_input = Some(input.clone());
        let seed = queued_seed();
        assert!(driver.ledger_mut().recover(state.clone()));
        driver
            .admit_recovered_to_ingress(
                input_id.clone(),
                crate::ingress_types::RuntimeInputSemantics {
                    boundary: meerkat_core::lifecycle::run_primitive::RunApplyBoundary::RunStart,
                    execution_kind: meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn,
                    execution_handling_mode: None,
                    peer_response_terminal_apply_intent: None,
                    live_interrupt_required: false,
                },
                &state,
                &seed,
                None,
                None,
                None,
            )
            .expect("recover queued prompt input");
        driver.rebuild_queue_projections_after_recovery();
        driver.clear_admitted_runtime_semantics_for_test(&input_id);

        let selected = authorized_batch_input_ids(&DriverEntry::Ephemeral(driver));

        assert_eq!(
            selected,
            vec![input_id],
            "missing runtime semantics must be selected so the runtime loop records a typed failure instead of treating the queue as empty"
        );
    }

    #[test]
    fn batch_selection_surfaces_steered_input_missing_runtime_semantics() {
        let mut driver = EphemeralRuntimeDriver::new(crate::identifiers::LogicalRuntimeId::new(
            "missing-steer-runtime-semantics-selection",
        ));
        let input = Input::Continuation(
            crate::input::ContinuationInput::detached_background_op_completed(),
        );
        let input_id = input.id().clone();
        let mut state = InputState::new_accepted(input_id.clone());
        state.persisted_input = Some(input.clone());
        let mut seed = queued_seed();
        seed.recovery_lane = Some(meerkat_core::types::HandlingMode::Steer);

        assert!(driver.ledger_mut().recover(state.clone()));
        driver
            .admit_recovered_to_ingress(
                input_id.clone(),
                crate::ingress_types::RuntimeInputSemantics {
                    boundary:
                        meerkat_core::lifecycle::run_primitive::RunApplyBoundary::RunCheckpoint,
                    execution_kind: meerkat_core::lifecycle::RuntimeExecutionKind::ResumePending,
                    execution_handling_mode: None,
                    peer_response_terminal_apply_intent: None,
                    live_interrupt_required: false,
                },
                &state,
                &seed,
                None,
                None,
                None,
            )
            .expect("recover steered continuation input");
        driver.rebuild_queue_projections_after_recovery();
        driver.clear_admitted_runtime_semantics_for_test(&input_id);

        let selected = authorized_batch_input_ids(&DriverEntry::Ephemeral(driver));

        assert_eq!(
            selected,
            vec![input_id],
            "missing runtime semantics in the steer lane must be selected so the runtime loop records a typed failure instead of treating the lane as empty"
        );
    }
}

pub(crate) fn machine_prepare_destroy(
    driver: &mut DriverEntry,
) -> Result<PreparedDestroy, RuntimeDriverError> {
    driver.prepare_destroy_lifecycle()
}

pub(crate) async fn machine_commit_prepared_destroy(
    driver: &mut DriverEntry,
    lifecycle: PreparedDestroyLifecycle,
) -> Result<(), RuntimeDriverError> {
    Box::pin(driver.commit_prepared_destroy_lifecycle(lifecycle)).await?;
    driver.sync_control_projection_from_dsl_authority();
    Ok(())
}

pub(crate) async fn machine_retire(
    driver: &mut DriverEntry,
) -> Result<RetireReport, RuntimeDriverError> {
    match driver {
        DriverEntry::Ephemeral(d) => {
            d.sync_control_projection_from_dsl_authority();
            Ok(d.finalize_retire())
        }
        DriverEntry::Persistent(d) => d.realize_retire_lifecycle().await,
    }
}

pub(crate) async fn machine_reset(
    driver: &mut DriverEntry,
) -> Result<ResetReport, RuntimeDriverError> {
    match driver {
        DriverEntry::Ephemeral(d) => {
            let report = d.reset_cleanup()?;
            d.sync_control_projection_from_dsl_authority();
            Ok(report)
        }
        DriverEntry::Persistent(d) => d.realize_reset_lifecycle().await,
    }
}

pub(crate) fn machine_prepare_bindings_projection(driver: &mut DriverEntry) {
    driver.sync_control_projection_from_dsl_authority();
}

pub(crate) async fn machine_recycle_preserving_work(
    driver: &mut DriverEntry,
) -> Result<usize, RuntimeDriverError> {
    match driver {
        DriverEntry::Ephemeral(driver) => {
            let transferred = driver.recycle_preserving_work()?;
            driver.sync_control_projection_from_dsl_authority();
            Ok(transferred)
        }
        DriverEntry::Persistent(driver) => driver.recycle_preserving_work().await,
    }
}

pub(crate) async fn prepare_runtime_loop_batch_start(
    driver: &SharedDriver,
    run_id: RunId,
    batch: AuthorizedRuntimeLoopBatch,
) -> Result<(), RuntimeDriverError> {
    let mut driver = driver.lock().await;
    let staged_ids = batch.input_ids().to_vec();
    let stage_source = batch.source();
    machine_begin_run(&mut driver, run_id.clone()).map_err(|err| {
        RuntimeDriverError::Internal(format!("failed to start runtime run: {err}"))
    })?;

    let stage_result = machine_authorize_stage_for_run(&driver, &run_id, &staged_ids, stage_source)
        .ok_or_else(|| {
            RuntimeDriverError::Internal(format!(
                "generated machine did not authorize StageForRun for run {run_id:?} and inputs {staged_ids:?}"
            ))
        })
        .and_then(|stage_authority| {
            driver.machine_realize_authorized_stage_batch(stage_authority)
        });

    if let Err(err) = stage_result {
        let _ = driver.rollback_staged(&staged_ids);
        if let Err(rollback_err) = machine_apply_run_return_projection(
            &mut driver,
            &run_id,
            RunReturnDisposition::Rollback,
        ) {
            return Err(RuntimeDriverError::Internal(format!(
                "failed to roll back runtime run after batch staging failure: {rollback_err}; staging failure: {err}"
            )));
        }
        return Err(RuntimeDriverError::Internal(format!(
            "failed to stage accepted input batch: {err}"
        )));
    }

    Ok(())
}

fn validate_completed_run_session_witness(
    owner_session_id: &SessionId,
    receipt: &RunBoundaryReceipt,
    session_snapshot: Option<&[u8]>,
) -> Result<(), RuntimeDriverError> {
    let Some(session_snapshot) = session_snapshot else {
        return Ok(());
    };
    let session =
        serde_json::from_slice::<meerkat_core::Session>(session_snapshot).map_err(|error| {
            RuntimeDriverError::ValidationFailed {
                reason: format!("completed-run session snapshot was not a Session: {error}"),
            }
        })?;
    if session.id() != owner_session_id {
        return Err(RuntimeDriverError::ValidationFailed {
            reason: format!(
                "completed-run session owner mismatch: generated {owner_session_id}, snapshot {}",
                session.id()
            ),
        });
    }
    if receipt.message_count != session.messages().len() {
        return Err(RuntimeDriverError::ValidationFailed {
            reason: format!(
                "completed-run receipt message count {} did not match session message count {}",
                receipt.message_count,
                session.messages().len()
            ),
        });
    }
    let encoded_messages = serde_json::to_vec(session.messages()).map_err(|error| {
        RuntimeDriverError::ValidationFailed {
            reason: format!(
                "completed-run session messages could not be encoded for digest validation: {error}"
            ),
        }
    })?;
    let expected_digest = format!("{:x}", Sha256::digest(encoded_messages));
    match receipt.conversation_digest.as_deref() {
        Some(actual_digest) if actual_digest == expected_digest => Ok(()),
        Some(actual_digest) => Err(RuntimeDriverError::ValidationFailed {
            reason: format!(
                "completed-run receipt digest {actual_digest} did not match session digest {expected_digest}"
            ),
        }),
        None => Err(RuntimeDriverError::ValidationFailed {
            reason: "completed-run receipt omitted the required SHA-256 session digest".to_string(),
        }),
    }
}

pub(crate) async fn commit_runtime_loop_run(
    driver: &SharedDriver,
    run_id: RunId,
    consumed_input_ids: Vec<InputId>,
    receipt: meerkat_core::lifecycle::RunBoundaryReceiptDraft,
    session_snapshot: Option<Vec<u8>>,
    directed_interaction_ids: Vec<meerkat_core::interaction::InteractionId>,
    terminal: Option<&meerkat_core::lifecycle::core_executor::CoreApplyTerminal>,
) -> Result<(), RuntimeLoopRunCommitError> {
    let mut driver = driver.lock().await;
    let commit_authority =
        AuthorizedRuntimeLoopRunCommit::authorize(&driver, run_id, consumed_input_ids, receipt)
            .map_err(RuntimeLoopRunCommitError::Rejected)?;
    debug_assert_eq!(
        commit_authority.generated_plan(),
        generated_kernel_command_capabilities::CommandPlanKind::AuthorizedRuntimeLoopRunCommit
    );
    let completed_run_id = commit_authority.run_id().clone();
    let receipt = commit_authority.receipt().clone();
    let consumed_input_ids = commit_authority.consumed_input_ids().to_vec();
    let commit_input_id = commit_authority.commit_input_id().clone();
    validate_completed_run_session_witness(
        commit_authority.owner_session_id(),
        &receipt,
        session_snapshot.as_deref(),
    )
    .map_err(RuntimeLoopRunCommitError::Rejected)?;
    let interaction_outboxes = authorized_directed_terminal_outboxes(
        &driver,
        &commit_authority,
        &directed_interaction_ids,
        terminal,
    )
    .map_err(RuntimeLoopRunCommitError::Rejected)?;

    let terminal_checkpoint = driver.rollback_snapshot();
    if let Err(err) = driver.stage_interaction_terminal_outboxes(interaction_outboxes) {
        driver.restore_rollback_snapshot(terminal_checkpoint);
        return Err(RuntimeLoopRunCommitError::Rejected(err));
    }
    if let Err(err) = driver.machine_realize_boundary_applied_in_memory(&completed_run_id, &receipt)
    {
        driver.restore_rollback_snapshot(terminal_checkpoint);
        return Err(RuntimeLoopRunCommitError::BoundaryCommit(
            RuntimeDriverError::Internal(format!("runtime boundary realization failed: {err}")),
        ));
    }

    if let Err(err) = machine_validate_run_completed(&driver, &consumed_input_ids) {
        driver.restore_rollback_snapshot(terminal_checkpoint);
        return Err(RuntimeLoopRunCommitError::PostBoundaryValidation(
            RuntimeDriverError::Internal(format!(
                "runtime completion validation failed after boundary commit: {err}"
            )),
        ));
    }
    if let Err(err) = machine_apply_turn_run_completed(&mut driver, &completed_run_id) {
        driver.restore_rollback_snapshot(terminal_checkpoint);
        return Err(RuntimeLoopRunCommitError::Rejected(err));
    }
    let _owner_runtime_binding = (
        commit_authority.owner_agent_runtime_id(),
        commit_authority.owner_fence_token(),
        commit_authority.owner_runtime_generation(),
        commit_authority.owner_runtime_epoch_id(),
    );
    if commit_authority.commit_outcome().run_id() != &completed_run_id
        || commit_authority.commit_outcome().outcome()
            != crate::meerkat_machine::dsl::TurnTerminalOutcome::Completed
    {
        driver.restore_rollback_snapshot(terminal_checkpoint);
        return Err(RuntimeLoopRunCommitError::Rejected(
            RuntimeDriverError::Internal(
                "runtime-loop run commit authority carried mismatched commit outcome".to_string(),
            ),
        ));
    }
    if !commit_authority
        .effect_closure_obligations()
        .iter()
        .any(|obligation| {
            obligation.is_satisfied_by(&completed_run_id, RuntimeLoopRunCommitEffect::Completed)
        })
    {
        driver.restore_rollback_snapshot(terminal_checkpoint);
        return Err(RuntimeLoopRunCommitError::Rejected(
            RuntimeDriverError::Internal(
                "runtime-loop run commit authority carried no completed-run effect closure obligation"
                    .to_string(),
            ),
        ));
    }
    let return_projection = match machine_apply_run_return_projection(
        &mut driver,
        &completed_run_id,
        RunReturnDisposition::Commit {
            input_id: &commit_input_id,
        },
    ) {
        Ok(projection) => projection,
        Err(err) => {
            driver.restore_rollback_snapshot(terminal_checkpoint);
            return Err(RuntimeLoopRunCommitError::Rejected(
                RuntimeDriverError::Internal(format!(
                    "failed to apply runtime return projection after completion: {err}"
                )),
            ));
        }
    };
    if &return_projection != commit_authority.return_projection() {
        driver.restore_rollback_snapshot(terminal_checkpoint);
        return Err(RuntimeLoopRunCommitError::Rejected(
            RuntimeDriverError::Internal(format!(
                "runtime-loop run commit projection {:?} did not match generated authority {:?}",
                return_projection,
                commit_authority.return_projection()
            )),
        ));
    }
    if let Err(err) =
        driver.machine_realize_run_completed_in_memory(&completed_run_id, &consumed_input_ids)
    {
        driver.restore_rollback_snapshot(terminal_checkpoint);
        return Err(RuntimeLoopRunCommitError::Rejected(
            RuntimeDriverError::Internal(format!(
                "failed to realize runtime completion snapshot: {err}"
            )),
        ));
    }
    if let Err(err) = driver
        .machine_commit_completed_boundary_snapshot(
            &receipt,
            session_snapshot.as_ref(),
            commit_authority.owner_session_id(),
        )
        .await
    {
        driver.restore_rollback_snapshot(terminal_checkpoint);
        return Err(RuntimeLoopRunCommitError::TerminalSnapshot(
            RuntimeDriverError::Internal(format!(
                "failed to persist runtime completed-boundary snapshot: {err}"
            )),
        ));
    }
    if matches!(&*driver, DriverEntry::Persistent(_)) {
        driver.set_control_projection(
            return_projection.phase,
            return_projection.current_run_id,
            return_projection.pre_run_phase,
        );
    }

    let _receipt = commit_authority.into_receipt();
    Ok(())
}

pub(crate) fn machine_resolve_runtime_completion_result(
    driver: &DriverEntry,
    run_id: Option<&RunId>,
    terminal: crate::meerkat_machine::dsl::RuntimeCompletionTerminalObservation,
    finalization: crate::meerkat_machine::dsl::RuntimeCompletionFinalizationObservation,
) -> Result<RuntimeCompletionResultAuthority, RuntimeDriverError> {
    let dsl_run_id = run_id.map(crate::meerkat_machine::dsl::RunId::from_domain);
    let input = crate::meerkat_machine::dsl::MeerkatMachineInput::ResolveRuntimeCompletionResult {
        run_id: dsl_run_id.clone(),
        terminal,
        finalization,
    };
    let authority = driver.shared_dsl_authority();
    let state = {
        let authority = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        authority.state().clone()
    };
    let mut preview = crate::meerkat_machine::dsl::MeerkatMachineAuthority::recover_from_state(
        state,
    )
    .map_err(|err| {
        RuntimeDriverError::Internal(crate::meerkat_machine::dsl_authority::map_error(
            err,
            "ResolveRuntimeCompletionResult",
        ))
    })?;
    let effects = crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(&mut preview, input)
        .map(|transition| transition.into_effects())
        .map_err(|err| RuntimeDriverError::ValidationFailed {
            reason: crate::meerkat_machine::dsl_authority::map_error(
                err,
                "ResolveRuntimeCompletionResult",
            ),
        })?;
    runtime_completion_result_authority_from_effects(dsl_run_id.as_ref(), &effects)
}

fn apply_runtime_completion_result_correlation_recovery(
    authority: &mut crate::meerkat_machine::dsl::MeerkatMachineAuthority,
    run_id: &RunId,
) -> Result<(), RuntimeDriverError> {
    crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(
        authority,
        crate::meerkat_machine::dsl::MeerkatMachineInput::
            RecoverRuntimeCompletionResultCorrelation {
                run_id: crate::meerkat_machine::dsl::RunId::from_domain(run_id),
            },
    )
    .map(|_| ())
    .map_err(|error| RuntimeDriverError::ValidationFailed {
        reason: crate::meerkat_machine::dsl_authority::map_error(
            error,
            "RecoverRuntimeCompletionResultCorrelation",
        ),
    })
}

fn machine_validate_runtime_completion_result_correlation_recovery(
    driver: &DriverEntry,
    run_id: &RunId,
) -> Result<(), RuntimeDriverError> {
    let authority = driver.shared_dsl_authority();
    let state = authority
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .state()
        .clone();
    let mut preview = crate::meerkat_machine::dsl::MeerkatMachineAuthority::recover_from_state(
        state,
    )
    .map_err(|error| {
        RuntimeDriverError::Internal(crate::meerkat_machine::dsl_authority::map_error(
            error,
            "RecoverRuntimeCompletionResultCorrelation preview",
        ))
    })?;
    apply_runtime_completion_result_correlation_recovery(&mut preview, run_id)
}

/// Restore the single generated completion-result correlation from an exact
/// durable unpublished interaction-terminal batch after whole-image preview
/// validation and owner adoption have succeeded.
pub(crate) fn machine_recover_runtime_completion_result_correlation(
    driver: &DriverEntry,
    run_id: &RunId,
) -> Result<(), RuntimeDriverError> {
    let authority = driver.shared_dsl_authority();
    let mut authority = authority
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    apply_runtime_completion_result_correlation_recovery(&mut authority, run_id)
}

#[cfg(test)]
pub(crate) fn machine_resolve_pre_resolved_runtime_completion_result(
    run_id: Option<&RunId>,
    terminal: crate::meerkat_machine::dsl::RuntimeCompletionTerminalObservation,
    finalization: crate::meerkat_machine::dsl::RuntimeCompletionFinalizationObservation,
) -> Result<RuntimeCompletionResultAuthority, RuntimeDriverError> {
    let dsl_run_id = run_id.map(crate::meerkat_machine::dsl::RunId::from_domain);
    let session_id = SessionId::new();
    let mut authority = crate::meerkat_machine::dsl_authority::new_registered_authority(
        &session_id,
    )
    .map_err(|error| RuntimeDriverError::ValidationFailed {
        reason: crate::meerkat_machine::dsl_authority::map_error(
            error,
            "fresh pre-resolved completion authority",
        ),
    })?;

    if let Some(run_id) = dsl_run_id.clone() {
        apply_runtime_completion_authority_preview(
            &mut authority,
            crate::meerkat_machine::dsl::MeerkatMachineInput::StartConversationRun {
                run_id,
                primitive_kind: crate::meerkat_machine::dsl::TurnPrimitiveKind::ConversationTurn,
                admitted_content_shape: crate::meerkat_machine::dsl::ContentShape::Conversation,
                vision_enabled: false,
                image_tool_results_enabled: false,
                max_extraction_retries: 0,
            },
            "StartConversationRun",
        )?;
    }

    if finalization
        == crate::meerkat_machine::dsl::RuntimeCompletionFinalizationObservation::Succeeded
        && let Some(run_id) = dsl_run_id.clone()
    {
        apply_runtime_completion_authority_preview(
            &mut authority,
            crate::meerkat_machine::dsl::MeerkatMachineInput::RunCompleted { run_id },
            "RunCompleted",
        )?;
    }

    let effects = apply_runtime_completion_authority_preview(
        &mut authority,
        crate::meerkat_machine::dsl::MeerkatMachineInput::ResolveRuntimeCompletionResult {
            run_id: dsl_run_id.clone(),
            terminal,
            finalization,
        },
        "ResolveRuntimeCompletionResult",
    )?;

    runtime_completion_result_authority_from_effects(dsl_run_id.as_ref(), &effects)
}

#[cfg(test)]
fn apply_runtime_completion_authority_preview(
    authority: &mut crate::meerkat_machine::dsl::MeerkatMachineAuthority,
    input: crate::meerkat_machine::dsl::MeerkatMachineInput,
    context: &'static str,
) -> Result<Vec<crate::meerkat_machine::dsl::MeerkatMachineEffect>, RuntimeDriverError> {
    crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(authority, input)
        .map(|transition| transition.into_effects())
        .map_err(|err| RuntimeDriverError::ValidationFailed {
            reason: crate::meerkat_machine::dsl_authority::map_error(err, context),
        })
}

fn runtime_completion_result_authority_from_effects(
    expected_run_id: Option<&crate::meerkat_machine::dsl::RunId>,
    effects: &[crate::meerkat_machine::dsl::MeerkatMachineEffect],
) -> Result<RuntimeCompletionResultAuthority, RuntimeDriverError> {
    let mut resolved = None;
    for effect in effects {
        let crate::meerkat_machine::dsl::MeerkatMachineEffect::RuntimeCompletionResultResolved {
            session_id,
            agent_runtime_id,
            fence_token,
            runtime_generation,
            runtime_epoch_id,
            run_id,
            result_class,
            cleanup_outcome,
        } = effect
        else {
            continue;
        };
        if run_id.as_ref() != expected_run_id {
            continue;
        }
        let session_id = SessionId::parse(&session_id.0).map_err(|err| {
            RuntimeDriverError::Internal(format!(
                "generated runtime completion authority emitted invalid session id '{}': {err}",
                session_id.0
            ))
        })?;
        if resolved
            .replace(RuntimeCompletionResultAuthority::from_generated_effect(
                session_id,
                agent_runtime_id.clone(),
                *fence_token,
                *runtime_generation,
                runtime_epoch_id.clone(),
                *result_class,
                *cleanup_outcome,
            ))
            .is_some()
        {
            return Err(RuntimeDriverError::Internal(
                "generated runtime completion authority emitted multiple public results"
                    .to_string(),
            ));
        }
    }

    resolved.ok_or_else(|| {
        RuntimeDriverError::Internal(format!(
            "ResolveRuntimeCompletionResult emitted no RuntimeCompletionResultResolved effect for run {expected_run_id:?}"
        ))
    })
}

pub(crate) async fn machine_resolve_runtime_completed_without_result(
    driver: &SharedDriver,
    run_id: &RunId,
) -> Result<RuntimeCompletionResultAuthority, RuntimeDriverError> {
    let driver = driver.lock().await;
    machine_resolve_runtime_completion_result(
        &driver,
        Some(run_id),
        crate::meerkat_machine::dsl::RuntimeCompletionTerminalObservation::NoResult,
        crate::meerkat_machine::dsl::RuntimeCompletionFinalizationObservation::Succeeded,
    )
}

pub(crate) async fn machine_resolve_runtime_terminated_completion_result(
    driver: &SharedDriver,
) -> Result<RuntimeCompletionResultAuthority, RuntimeDriverError> {
    let driver = driver.lock().await;
    machine_resolve_runtime_completion_result(
        &driver,
        None,
        crate::meerkat_machine::dsl::RuntimeCompletionTerminalObservation::RuntimeTerminated,
        crate::meerkat_machine::dsl::RuntimeCompletionFinalizationObservation::Succeeded,
    )
}

pub(crate) async fn fail_runtime_loop_run(
    driver: &SharedDriver,
    run_id: RunId,
    failure: CoreApplyFailureCause,
) -> Result<(), RuntimeLoopRunFailError> {
    fail_runtime_loop_run_inner(
        driver,
        run_id,
        RuntimeLoopRunFailureContext {
            terminal_error: failure.message().to_owned(),
            runtime_apply_failure: Some(failure),
            machine_terminal_failure_observed: false,
            machine_terminal_error: None,
            terminal_failure_source: None,
            applied_terminal: None,
        },
    )
    .await
}

#[cfg(test)]
pub(crate) async fn fail_machine_run(
    driver: &SharedDriver,
    run_id: RunId,
    failure: super::MeerkatMachineRunFailure,
) -> Result<(), RuntimeLoopRunFailError> {
    fail_runtime_loop_run_inner(
        driver,
        run_id,
        RuntimeLoopRunFailureContext {
            terminal_error: failure.error,
            runtime_apply_failure: None,
            machine_terminal_failure_observed: failure.machine_terminal_failure_observed,
            machine_terminal_error: failure.machine_terminal_error,
            terminal_failure_source: failure.source,
            applied_terminal: None,
        },
    )
    .await
}

pub(crate) async fn commit_machine_terminal_run(
    driver: &SharedDriver,
    run_id: RunId,
    error: meerkat_core::TurnErrorMetadata,
    applied: MachineTerminalAppliedDraft,
) -> Result<(), RuntimeLoopRunFailError> {
    let failure = super::MeerkatMachineRunFailure::from_machine_terminal_failure(error);
    fail_runtime_loop_run_inner(
        driver,
        run_id,
        RuntimeLoopRunFailureContext {
            terminal_error: failure.error,
            runtime_apply_failure: None,
            machine_terminal_failure_observed: failure.machine_terminal_failure_observed,
            machine_terminal_error: failure.machine_terminal_error,
            terminal_failure_source: failure.source,
            applied_terminal: Some(applied),
        },
    )
    .await
}

pub(crate) async fn cancel_runtime_loop_run(
    driver: &SharedDriver,
    run_id: RunId,
) -> Result<(), RuntimeLoopRunFailError> {
    let mut driver = driver.lock().await;
    let cancelled_run_id = run_id.clone();
    let staged_input_ids = machine_staged_contributors(&driver);
    machine_validate_run_cancelled(&driver, &staged_input_ids)
        .map_err(RuntimeLoopRunFailError::Rejected)?;
    let interaction_outboxes = authorized_staged_directed_terminal_outboxes(
        &driver,
        InteractionTerminalBatchScope::Run(&cancelled_run_id),
        &staged_input_ids,
        crate::input_state::InteractionTerminalCandidate::Cancelled,
    )
    .map_err(RuntimeLoopRunFailError::Rejected)?;
    let terminal_checkpoint = driver.rollback_snapshot();
    if let Err(error) = driver.stage_interaction_terminal_outboxes(interaction_outboxes) {
        driver.restore_rollback_snapshot(terminal_checkpoint);
        return Err(RuntimeLoopRunFailError::Rejected(error));
    }
    if let Err(err) = machine_apply_turn_run_cancelled(&mut driver, &cancelled_run_id) {
        driver.restore_rollback_snapshot(terminal_checkpoint);
        return Err(RuntimeLoopRunFailError::Rejected(err));
    }
    if let Err(err) = machine_apply_run_return_projection(
        &mut driver,
        &cancelled_run_id,
        RunReturnDisposition::Cancelled,
    ) {
        driver.restore_rollback_snapshot(terminal_checkpoint);
        return Err(RuntimeLoopRunFailError::Rejected(
            RuntimeDriverError::Internal(format!(
                "failed to apply runtime return projection after cancellation: {err}"
            )),
        ));
    }
    if let Err(run_err) = driver
        .machine_realize_run_cancelled(cancelled_run_id.clone(), staged_input_ids)
        .await
    {
        driver.restore_rollback_snapshot(terminal_checkpoint);
        return Err(RuntimeLoopRunFailError::TerminalSnapshot(
            RuntimeDriverError::Internal(format!(
                "failed to record run-cancelled event: {run_err}"
            )),
        ));
    }
    if matches!(&*driver, DriverEntry::Persistent(_)) {
        driver.sync_control_projection_from_dsl_authority();
    }
    Ok(())
}

fn machine_terminal_carrier_validation_failed(reason: impl Into<String>) -> RuntimeDriverError {
    RuntimeDriverError::ValidationFailed {
        reason: reason.into(),
    }
}

/// Validate every executor-provided failed-but-applied witness against the
/// generated outer-machine state before any live or durable mutation occurs.
///
/// A compliant executor reaches the failed turn through its shared
/// `TurnStateHandle`, so the outer machine already owns the exact terminal
/// outcome/cause and session identity. An executor that bypasses that authority
/// cannot mint a `MachineTerminalFailure` carrier after the fact.
fn validate_machine_terminal_applied_commit(
    driver: &DriverEntry,
    failed_run_id: &RunId,
    staged_input_ids: &[InputId],
    error: &meerkat_core::TurnErrorMetadata,
    applied: MachineTerminalAppliedDraft,
) -> Result<MachineTerminalAppliedCommit, RuntimeDriverError> {
    if !error.terminal {
        return Err(machine_terminal_carrier_validation_failed(
            "machine-terminal metadata was not marked terminal",
        ));
    }
    let outcome = error.outcome.ok_or_else(|| {
        machine_terminal_carrier_validation_failed(
            "machine-terminal metadata omitted its terminal outcome",
        )
    })?;
    if !error.kind.is_specific_failure_cause() {
        return Err(machine_terminal_carrier_validation_failed(
            "machine-terminal metadata used a nonspecific failure cause",
        ));
    }

    let (
        owner_session_id,
        generated_run_id,
        generated_turn_phase,
        generated_terminal_run_id,
        generated_terminal_outcome,
        generated_terminal_cause_kind,
    ) = {
        let authority = driver.shared_dsl_authority();
        let authority = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let state = authority.state();
        (
            state.session_id.clone(),
            state.current_run_id.clone(),
            state.turn_phase,
            state.turn_terminal_run_id.clone(),
            state.terminal_outcome,
            state.terminal_cause_kind,
        )
    };
    if generated_run_id.as_ref()
        != Some(&crate::meerkat_machine::dsl::RunId::from_domain(
            failed_run_id,
        ))
    {
        return Err(machine_terminal_carrier_validation_failed(format!(
            "machine-terminal carrier run {failed_run_id} did not match generated current run"
        )));
    }
    if generated_turn_phase != crate::meerkat_machine::dsl::TurnPhase::Failed {
        return Err(machine_terminal_carrier_validation_failed(
            "machine-terminal carrier lacked a generated failed turn",
        ));
    }
    if generated_terminal_run_id.as_ref()
        != Some(&crate::meerkat_machine::dsl::RunId::from_domain(
            failed_run_id,
        ))
    {
        return Err(machine_terminal_carrier_validation_failed(
            "machine-terminal carrier did not match the generated terminal run witness",
        ));
    }
    let expected_outcome = crate::meerkat_machine::dsl::TurnTerminalOutcome::from(outcome);
    if generated_terminal_outcome != Some(expected_outcome) {
        return Err(machine_terminal_carrier_validation_failed(format!(
            "machine-terminal outcome {outcome:?} did not match generated terminal outcome {generated_terminal_outcome:?}"
        )));
    }
    let expected_cause = crate::meerkat_machine::dsl::TurnTerminalCauseKind::from(error.kind);
    if generated_terminal_cause_kind != Some(expected_cause) {
        return Err(machine_terminal_carrier_validation_failed(format!(
            "machine-terminal cause {:?} did not match generated terminal cause {generated_terminal_cause_kind:?}",
            error.kind
        )));
    }

    let owner_session_id = owner_session_id
        .ok_or_else(|| {
            machine_terminal_carrier_validation_failed(
                "machine-terminal carrier lacked generated session ownership",
            )
        })
        .and_then(|session_id| {
            SessionId::parse(&session_id.0).map_err(|parse_error| {
                machine_terminal_carrier_validation_failed(format!(
                    "generated machine session identity was invalid: {parse_error}"
                ))
            })
        })?;
    let session = serde_json::from_slice::<meerkat_core::Session>(&applied.session_snapshot)
        .map_err(|error| {
            machine_terminal_carrier_validation_failed(format!(
                "machine-terminal session snapshot was not a Session: {error}"
            ))
        })?;
    if session.id() != &owner_session_id {
        return Err(machine_terminal_carrier_validation_failed(format!(
            "machine-terminal session owner mismatch: generated {owner_session_id}, snapshot {}",
            session.id()
        )));
    }

    let receipt = applied
        .receipt
        .into_sequenced(driver.run_boundary_sequence(failed_run_id));
    machine_validate_run_commit_receipt(driver, failed_run_id, staged_input_ids, &receipt)?;
    if receipt.message_count != session.messages().len() {
        return Err(machine_terminal_carrier_validation_failed(format!(
            "machine-terminal receipt message count {} did not match session message count {}",
            receipt.message_count,
            session.messages().len()
        )));
    }
    let encoded_messages = serde_json::to_vec(session.messages()).map_err(|error| {
        machine_terminal_carrier_validation_failed(format!(
            "machine-terminal session messages could not be encoded for digest validation: {error}"
        ))
    })?;
    let expected_digest = format!("{:x}", Sha256::digest(encoded_messages));
    match receipt.conversation_digest.as_deref() {
        Some(actual_digest) if actual_digest == expected_digest => {}
        Some(actual_digest) => {
            return Err(machine_terminal_carrier_validation_failed(format!(
                "machine-terminal receipt digest {actual_digest} did not match session digest {expected_digest}"
            )));
        }
        None => {
            return Err(machine_terminal_carrier_validation_failed(
                "machine-terminal receipt omitted the required SHA-256 session digest",
            ));
        }
    }

    Ok(MachineTerminalAppliedCommit {
        receipt,
        session_snapshot: applied.session_snapshot,
        owner_session_id,
    })
}

struct RuntimeLoopRunFailureContext {
    terminal_error: String,
    runtime_apply_failure: Option<CoreApplyFailureCause>,
    machine_terminal_failure_observed: bool,
    machine_terminal_error: Option<meerkat_core::TurnErrorMetadata>,
    terminal_failure_source: Option<crate::meerkat_machine::dsl::RunFailureSourceKind>,
    applied_terminal: Option<MachineTerminalAppliedDraft>,
}

async fn fail_runtime_loop_run_inner(
    driver: &SharedDriver,
    run_id: RunId,
    failure: RuntimeLoopRunFailureContext,
) -> Result<(), RuntimeLoopRunFailError> {
    let RuntimeLoopRunFailureContext {
        terminal_error,
        runtime_apply_failure,
        machine_terminal_failure_observed,
        machine_terminal_error,
        terminal_failure_source,
        applied_terminal,
    } = failure;
    let mut driver = driver.lock().await;
    let failed_run_id = run_id.clone();
    let staged_input_ids = machine_staged_contributors(&driver);
    machine_validate_run_failed(&driver, &staged_input_ids)
        .map_err(RuntimeLoopRunFailError::Rejected)?;
    if machine_terminal_failure_observed != machine_terminal_error.is_some() {
        return Err(RuntimeLoopRunFailError::Rejected(
            RuntimeDriverError::ValidationFailed {
                reason: "machine-terminal failure observation and typed completion error disagreed"
                    .to_string(),
            },
        ));
    }
    let prepared_applied_commit = match (machine_terminal_error.as_ref(), applied_terminal) {
        (Some(error), Some(applied)) => Some(
            validate_machine_terminal_applied_commit(
                &driver,
                &failed_run_id,
                &staged_input_ids,
                error,
                applied,
            )
            .map_err(RuntimeLoopRunFailError::Rejected)?,
        ),
        (Some(_), None) => {
            return Err(RuntimeLoopRunFailError::Rejected(
                machine_terminal_carrier_validation_failed(
                    "machine-terminal failure must use the failed-but-applied output carrier",
                ),
            ));
        }
        (None, Some(_)) => {
            return Err(RuntimeLoopRunFailError::Rejected(
                machine_terminal_carrier_validation_failed(
                    "failed-but-applied commit lacked machine-terminal metadata",
                ),
            ));
        }
        (None, None) => None,
    };
    let terminal_checkpoint = driver.rollback_snapshot();
    let mut applied_commit = None;
    if let Some(error) = machine_terminal_error.as_ref() {
        let interaction_outboxes = authorized_staged_directed_terminal_outboxes(
            &driver,
            InteractionTerminalBatchScope::Run(&failed_run_id),
            &staged_input_ids,
            crate::input_state::InteractionTerminalCandidate::MachineTerminalFailure {
                error: error.clone(),
            },
        )
        .map_err(RuntimeLoopRunFailError::Rejected)?;
        if let Err(error) = driver.stage_interaction_terminal_outboxes(interaction_outboxes) {
            driver.restore_rollback_snapshot(terminal_checkpoint);
            return Err(RuntimeLoopRunFailError::Rejected(error));
        }
        if let Err(error) = driver
            .machine_realize_terminal_failure_applied_in_memory(&failed_run_id, &staged_input_ids)
        {
            driver.restore_rollback_snapshot(terminal_checkpoint);
            return Err(RuntimeLoopRunFailError::Rejected(error));
        }
        if let Some(commit) = prepared_applied_commit {
            if commit.receipt.sequence != driver.run_boundary_sequence(&failed_run_id) {
                driver.restore_rollback_snapshot(terminal_checkpoint);
                return Err(RuntimeLoopRunFailError::Rejected(
                    RuntimeDriverError::ValidationFailed {
                        reason: "machine-terminal boundary sequence changed during commit"
                            .to_string(),
                    },
                ));
            }
            applied_commit = Some(commit);
        }
    }
    let run_failed_effects = match machine_apply_turn_run_failed(
        &mut driver,
        &failed_run_id,
        &terminal_error,
        runtime_apply_failure.as_ref(),
        machine_terminal_failure_observed,
        terminal_failure_source,
    ) {
        Ok(effects) => effects,
        Err(err) => {
            driver.restore_rollback_snapshot(terminal_checkpoint);
            return Err(RuntimeLoopRunFailError::Rejected(err));
        }
    };
    let recoverable = !machine_terminal_failure_observed;
    let replay_plan = machine_build_replay_plan(&driver, &staged_input_ids, "RunFailed");
    if let Err(err) = machine_apply_run_return_projection(
        &mut driver,
        &failed_run_id,
        RunReturnDisposition::Failed,
    ) {
        driver.restore_rollback_snapshot(terminal_checkpoint);
        return Err(RuntimeLoopRunFailError::Rejected(
            RuntimeDriverError::Internal(format!(
                "failed to apply runtime return projection after failure: {err}"
            )),
        ));
    }
    if let Err(run_err) = driver
        .machine_realize_run_failed(MachineRunFailureRealization {
            run_id: failed_run_id.clone(),
            contributing_input_ids: staged_input_ids,
            replay_plan,
            terminal_error,
            runtime_apply_failure,
            recoverable,
            applied_commit,
        })
        .await
    {
        driver.restore_rollback_snapshot(terminal_checkpoint);
        return Err(RuntimeLoopRunFailError::TerminalSnapshot(
            RuntimeDriverError::Internal(format!("failed to record run-failed event: {run_err}")),
        ));
    }
    driver.absorb_post_admission_effects(&run_failed_effects);
    if matches!(&*driver, DriverEntry::Persistent(_)) {
        driver.sync_control_projection_from_dsl_authority();
    }
    Ok(())
}

#[cfg(test)]
mod run_failed_cause_tests {
    use super::*;

    fn running_driver(run_id: &RunId) -> DriverEntry {
        let mut driver = crate::driver::ephemeral::EphemeralRuntimeDriver::new(
            LogicalRuntimeId::new("run-failed-cause-test"),
        );
        driver
            .contract_begin_run_authority(run_id.clone())
            .expect("test run must start through generated authority");
        DriverEntry::Ephemeral(driver)
    }

    fn assert_runtime_completion_authority(
        authority: RuntimeCompletionResultAuthority,
        expected_class: crate::meerkat_machine::dsl::RuntimeCompletionResultClass,
        expected_cleanup: crate::meerkat_machine::dsl::RuntimeCompletionObservedOutcome,
    ) {
        let attempt = authority.begin_surface_resolution();
        assert_eq!(attempt.class(), expected_class);
        let realized = attempt.realize();
        assert_eq!(realized.cleanup_observation(), expected_cleanup);
    }

    #[test]
    fn service_turn_terminal_validation_rejects_outcome_cause_mismatches() {
        use crate::meerkat_machine::dsl::{
            TurnPhase, TurnTerminalCauseKind as Cause, TurnTerminalOutcome as Outcome,
        };

        let coherent = [
            (TurnPhase::Completed, Some(Outcome::Completed), None),
            (
                TurnPhase::Failed,
                Some(Outcome::BudgetExhausted),
                Some(Cause::BudgetExhausted),
            ),
            (
                TurnPhase::Failed,
                Some(Outcome::TimeBudgetExceeded),
                Some(Cause::TimeBudgetExceeded),
            ),
            (
                TurnPhase::Failed,
                Some(Outcome::StructuredOutputValidationFailed),
                Some(Cause::StructuredOutputValidationFailed),
            ),
            (
                TurnPhase::Failed,
                Some(Outcome::Failed),
                Some(Cause::LlmFailure),
            ),
        ];
        for (phase, outcome, cause) in coherent {
            assert!(
                service_turn_terminal_is_coherent(phase, outcome, cause),
                "coherent terminal was rejected: phase={phase:?}, outcome={outcome:?}, cause={cause:?}"
            );
        }

        let mismatched = [
            (
                TurnPhase::Completed,
                Some(Outcome::Completed),
                Some(Cause::FatalFailure),
            ),
            (TurnPhase::Completed, Some(Outcome::Failed), None),
            (
                TurnPhase::Failed,
                Some(Outcome::BudgetExhausted),
                Some(Cause::TimeBudgetExceeded),
            ),
            (
                TurnPhase::Failed,
                Some(Outcome::Failed),
                Some(Cause::BudgetExhausted),
            ),
            (
                TurnPhase::Failed,
                Some(Outcome::StructuredOutputValidationFailed),
                Some(Cause::LlmFailure),
            ),
            (TurnPhase::Failed, None, Some(Cause::LlmFailure)),
            (
                TurnPhase::Failed,
                Some(Outcome::Failed),
                Some(Cause::Unknown),
            ),
        ];
        for (phase, outcome, cause) in mismatched {
            assert!(
                !service_turn_terminal_is_coherent(phase, outcome, cause),
                "mismatched terminal was accepted: phase={phase:?}, outcome={outcome:?}, cause={cause:?}"
            );
        }
    }

    #[test]
    fn runtime_completion_result_authority_classifies_success_result() {
        let run_id = RunId::new();
        let mut driver = running_driver(&run_id);
        machine_apply_turn_run_completed(&mut driver, &run_id)
            .expect("completion fact should be machine-owned");

        let class = machine_resolve_runtime_completion_result(
            &driver,
            Some(&run_id),
            crate::meerkat_machine::dsl::RuntimeCompletionTerminalObservation::RunResult,
            crate::meerkat_machine::dsl::RuntimeCompletionFinalizationObservation::Succeeded,
        )
        .expect("generated completion result authority should resolve");

        assert_runtime_completion_authority(
            class,
            crate::meerkat_machine::dsl::RuntimeCompletionResultClass::Completed,
            crate::meerkat_machine::dsl::RuntimeCompletionObservedOutcome::Completed,
        );
    }

    #[test]
    fn runtime_completion_result_authority_classifies_finalization_failure_with_result() {
        let run_id = RunId::new();
        let mut driver = running_driver(&run_id);
        machine_apply_turn_run_completed(&mut driver, &run_id)
            .expect("completion fact should be machine-owned");

        let class = machine_resolve_runtime_completion_result(
            &driver,
            Some(&run_id),
            crate::meerkat_machine::dsl::RuntimeCompletionTerminalObservation::RunResult,
            crate::meerkat_machine::dsl::RuntimeCompletionFinalizationObservation::Failed,
        )
        .expect("generated completion result authority should resolve");

        assert_runtime_completion_authority(
            class,
            crate::meerkat_machine::dsl::RuntimeCompletionResultClass::CompletedWithFinalizationFailure,
            crate::meerkat_machine::dsl::RuntimeCompletionObservedOutcome::FinalizationFailed,
        );
    }

    #[test]
    fn recovered_observed_phase_is_a_total_identity_projection_not_an_authority_step() {
        // `recovered_observed_phase` must remain a mechanical 1:1 boundary
        // type-translation from the durable runtime seed phase onto the DSL's
        // `RecoveredInputObservedPhase` — it must not collapse, infer, or
        // normalize any phase (that authority lives in
        // `NormalizeRecoveredInputLifecycle`). Every runtime phase variant maps
        // to its same-named observed-phase variant.
        use crate::meerkat_machine::dsl::RecoveredInputObservedPhase as Observed;
        let cases = [
            (InputLifecycleState::Accepted, Observed::Accepted),
            (InputLifecycleState::Queued, Observed::Queued),
            (InputLifecycleState::Staged, Observed::Staged),
            (InputLifecycleState::Applied, Observed::Applied),
            (
                InputLifecycleState::AppliedPendingConsumption,
                Observed::AppliedPendingConsumption,
            ),
            (InputLifecycleState::Consumed, Observed::Consumed),
            (InputLifecycleState::Superseded, Observed::Superseded),
            (InputLifecycleState::Coalesced, Observed::Coalesced),
            (InputLifecycleState::Abandoned, Observed::Abandoned),
        ];
        for (phase, expected) in cases {
            assert_eq!(
                recovered_observed_phase(phase),
                expected,
                "recovered_observed_phase must project {phase:?} onto its same-named observed phase without reclassification",
            );
        }
    }

    #[test]
    fn runtime_completion_result_authority_classifies_finalization_failure_without_result() {
        let run_id = RunId::new();
        let mut driver = running_driver(&run_id);
        machine_apply_turn_run_completed(&mut driver, &run_id)
            .expect("completion fact should be machine-owned");

        let class = machine_resolve_runtime_completion_result(
            &driver,
            Some(&run_id),
            crate::meerkat_machine::dsl::RuntimeCompletionTerminalObservation::NoResult,
            crate::meerkat_machine::dsl::RuntimeCompletionFinalizationObservation::Failed,
        )
        .expect("generated completion result authority should resolve");

        assert_runtime_completion_authority(
            class,
            crate::meerkat_machine::dsl::RuntimeCompletionResultClass::AbandonedWithError,
            crate::meerkat_machine::dsl::RuntimeCompletionObservedOutcome::RuntimeApplyFailed,
        );
    }

    #[test]
    fn runtime_completion_result_authority_classifies_machine_terminal_commit_failure() {
        let run_id = RunId::new();
        let driver = running_driver(&run_id);

        let class = machine_resolve_runtime_completion_result(
            &driver,
            Some(&run_id),
            crate::meerkat_machine::dsl::RuntimeCompletionTerminalObservation::MachineTerminal,
            crate::meerkat_machine::dsl::RuntimeCompletionFinalizationObservation::Failed,
        )
        .expect("failed machine-terminal commit should retain generated completion authority");

        assert_runtime_completion_authority(
            class,
            crate::meerkat_machine::dsl::RuntimeCompletionResultClass::AbandonedWithError,
            crate::meerkat_machine::dsl::RuntimeCompletionObservedOutcome::RuntimeApplyFailed,
        );
    }

    #[test]
    fn runtime_completion_result_authority_classifies_machine_cancellation() {
        let run_id = RunId::new();
        let mut driver = running_driver(&run_id);
        machine_apply_turn_run_cancelled(&mut driver, &run_id)
            .expect("cancellation fact should be machine-owned");

        let class = machine_resolve_runtime_completion_result(
            &driver,
            Some(&run_id),
            crate::meerkat_machine::dsl::RuntimeCompletionTerminalObservation::MachineTerminal,
            crate::meerkat_machine::dsl::RuntimeCompletionFinalizationObservation::Succeeded,
        )
        .expect("generated completion result authority should resolve");

        assert_runtime_completion_authority(
            class,
            crate::meerkat_machine::dsl::RuntimeCompletionResultClass::Cancelled,
            crate::meerkat_machine::dsl::RuntimeCompletionObservedOutcome::Cancelled,
        );
    }

    #[test]
    fn runtime_completion_result_authority_classifies_runtime_terminated_without_run() {
        let run_id = RunId::new();
        let driver = running_driver(&run_id);

        let class = machine_resolve_runtime_completion_result(
            &driver,
            None,
            crate::meerkat_machine::dsl::RuntimeCompletionTerminalObservation::RuntimeTerminated,
            crate::meerkat_machine::dsl::RuntimeCompletionFinalizationObservation::Succeeded,
        )
        .expect("generated completion result authority should resolve");

        assert_runtime_completion_authority(
            class,
            crate::meerkat_machine::dsl::RuntimeCompletionResultClass::RuntimeTerminated,
            crate::meerkat_machine::dsl::RuntimeCompletionObservedOutcome::RuntimeTerminated,
        );
    }

    #[test]
    fn runtime_completion_result_authority_rejects_unowned_run_id() {
        let run_id = RunId::new();
        let other_run_id = RunId::new();
        let mut driver = running_driver(&run_id);
        machine_apply_turn_run_completed(&mut driver, &run_id)
            .expect("completion fact should be machine-owned");

        let err = machine_resolve_runtime_completion_result(
            &driver,
            Some(&other_run_id),
            crate::meerkat_machine::dsl::RuntimeCompletionTerminalObservation::RunResult,
            crate::meerkat_machine::dsl::RuntimeCompletionFinalizationObservation::Succeeded,
        )
        .expect_err("generated completion result authority should reject mismatched run");

        assert!(format!("{err}").contains("ResolveRuntimeCompletionResult"));
    }

    #[test]
    fn run_failed_without_runtime_apply_cause_uses_fatal_terminal_cause() {
        let run_id = RunId::new();
        let mut driver = running_driver(&run_id);

        machine_apply_turn_run_failed(&mut driver, &run_id, "legacy failure", None, false, None)
            .expect("legacy run failure should apply");

        let authority = driver.shared_dsl_authority();
        let auth = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(
            auth.state().terminal_cause_kind,
            Some(crate::meerkat_machine::dsl::TurnTerminalCauseKind::FatalFailure)
        );
        assert_eq!(
            auth.state().terminal_outcome,
            Some(crate::meerkat_machine::dsl::TurnTerminalOutcome::Failed)
        );
        assert_eq!(auth.state().last_runtime_apply_failure_cause, None);
        assert_eq!(auth.state().last_runtime_apply_failure_message, None);
    }

    #[test]
    fn direct_run_failure_display_message_does_not_classify_terminal_cause() {
        let run_id = RunId::new();
        let mut driver = running_driver(&run_id);

        machine_apply_turn_run_failed(
            &mut driver,
            &run_id,
            "runtime apply failure: display-only text",
            None,
            false,
            None,
        )
        .expect("legacy run failure should apply");

        let authority = driver.shared_dsl_authority();
        let auth = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(
            auth.state().terminal_cause_kind,
            Some(crate::meerkat_machine::dsl::TurnTerminalCauseKind::FatalFailure)
        );
        assert_eq!(
            auth.state().terminal_outcome,
            Some(crate::meerkat_machine::dsl::TurnTerminalOutcome::Failed)
        );
        assert_eq!(auth.state().last_runtime_apply_failure_cause, None);
        assert_eq!(auth.state().last_runtime_apply_failure_message, None);
    }

    #[test]
    fn run_failed_with_runtime_apply_cause_uses_runtime_apply_terminal_cause() {
        let run_id = RunId::new();
        let mut driver = running_driver(&run_id);
        let failure = CoreApplyFailureCause::runtime_turn("runtime apply failed");

        machine_apply_turn_run_failed(
            &mut driver,
            &run_id,
            failure.message(),
            Some(&failure),
            false,
            None,
        )
        .expect("runtime apply failure should apply");

        let authority = driver.shared_dsl_authority();
        let auth = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(
            auth.state().terminal_cause_kind,
            Some(crate::meerkat_machine::dsl::TurnTerminalCauseKind::RuntimeApplyFailure)
        );
        assert_eq!(
            auth.state().terminal_outcome,
            Some(crate::meerkat_machine::dsl::TurnTerminalOutcome::Failed)
        );
        assert_eq!(
            auth.state().last_runtime_apply_failure_cause,
            Some(crate::meerkat_machine::dsl::RuntimeApplyFailureCause::RuntimeTurn)
        );
        assert_eq!(
            auth.state().last_runtime_apply_failure_message.as_deref(),
            Some("runtime apply failed")
        );
        drop(auth);

        let class = machine_resolve_runtime_completion_result(
            &driver,
            Some(&run_id),
            crate::meerkat_machine::dsl::RuntimeCompletionTerminalObservation::MachineTerminal,
            crate::meerkat_machine::dsl::RuntimeCompletionFinalizationObservation::Succeeded,
        )
        .expect("generated completion result authority should resolve runtime apply failure");
        assert_runtime_completion_authority(
            class,
            crate::meerkat_machine::dsl::RuntimeCompletionResultClass::AbandonedWithError,
            crate::meerkat_machine::dsl::RuntimeCompletionObservedOutcome::RuntimeApplyFailed,
        );
    }

    #[test]
    fn run_failed_rejects_machine_terminal_observation_without_generated_terminal_state() {
        let run_id = RunId::new();
        let mut driver = running_driver(&run_id);

        let err = machine_apply_turn_run_failed(
            &mut driver,
            &run_id,
            "caller supplied terminal failure",
            None,
            true,
            None,
        )
        .expect_err("machine terminal handoff without generated terminal state must fail closed");

        assert!(
            err.to_string().contains("guard rejected transition"),
            "unexpected machine terminal handoff error: {err}"
        );
        let authority = driver.shared_dsl_authority();
        let auth = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(auth.state().terminal_outcome, None);
        assert_eq!(auth.state().terminal_cause_kind, None);
    }

    #[test]
    fn run_failed_preserves_existing_generated_terminal_state_for_machine_terminal_observation() {
        let run_id = RunId::new();
        let mut driver = running_driver(&run_id);

        {
            let authority = driver.shared_dsl_authority();
            let mut auth = authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(
                &mut *auth,
                crate::meerkat_machine::dsl::MeerkatMachineInput::BudgetExhausted {
                    run_id: crate::meerkat_machine::dsl::RunId::from_domain(&run_id),
                },
            )
            .expect("budget terminal should apply");
        }

        machine_apply_turn_run_failed(
            &mut driver,
            &run_id,
            "machine-observed terminal failure",
            None,
            true,
            None,
        )
        .expect("machine terminal handoff should preserve generated terminal state");

        let authority = driver.shared_dsl_authority();
        let auth = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(
            auth.state().terminal_outcome,
            Some(crate::meerkat_machine::dsl::TurnTerminalOutcome::BudgetExhausted)
        );
        assert_eq!(
            auth.state().terminal_cause_kind,
            Some(crate::meerkat_machine::dsl::TurnTerminalCauseKind::BudgetExhausted)
        );
        assert_eq!(auth.state().last_runtime_apply_failure_cause, None);
        assert_eq!(auth.state().last_runtime_apply_failure_message, None);
    }

    #[test]
    fn run_failed_rejects_when_generated_authority_is_absent() {
        let run_id = RunId::new();
        let other_run_id = RunId::new();
        let mut driver = running_driver(&other_run_id);

        let err =
            machine_apply_turn_run_failed(&mut driver, &run_id, "stale failure", None, false, None)
                .expect_err("stale run failure must not be treated as generated authority");

        assert!(
            err.to_string()
                .contains("generated RunFailed authority absent"),
            "unexpected stale RunFailed error: {err}"
        );
        let authority = driver.shared_dsl_authority();
        let auth = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(auth.state().terminal_outcome, None);
        assert_eq!(auth.state().terminal_cause_kind, None);
    }

    #[test]
    fn run_cancelled_rejects_when_generated_authority_is_absent() {
        let run_id = RunId::new();
        let other_run_id = RunId::new();
        let mut driver = running_driver(&other_run_id);

        let err = machine_apply_turn_run_cancelled(&mut driver, &run_id)
            .expect_err("stale cancellation must not be treated as generated authority");

        assert!(
            err.to_string()
                .contains("generated RunCancelled authority absent"),
            "unexpected stale RunCancelled error: {err}"
        );
        let authority = driver.shared_dsl_authority();
        let auth = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(auth.state().terminal_outcome, None);
        assert_eq!(auth.state().terminal_cause_kind, None);
    }

    #[test]
    fn run_completed_preserves_existing_machine_terminal_outcome() {
        let run_id = RunId::new();
        let mut driver = running_driver(&run_id);

        {
            let authority = driver.shared_dsl_authority();
            let mut auth = authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(
                &mut *auth,
                crate::meerkat_machine::dsl::MeerkatMachineInput::BudgetExhausted {
                    run_id: crate::meerkat_machine::dsl::RunId::from_domain(&run_id),
                },
            )
            .expect("budget terminal should apply");
        }

        machine_apply_turn_run_completed(&mut driver, &run_id)
            .expect("durable commit completion should not overwrite terminal evidence");

        let authority = driver.shared_dsl_authority();
        let auth = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(
            auth.state().turn_phase,
            crate::meerkat_machine::dsl::TurnPhase::Failed
        );
        assert_eq!(
            auth.state().terminal_outcome,
            Some(crate::meerkat_machine::dsl::TurnTerminalOutcome::BudgetExhausted)
        );
        assert_eq!(
            auth.state().terminal_cause_kind,
            Some(crate::meerkat_machine::dsl::TurnTerminalCauseKind::BudgetExhausted)
        );
    }
}

#[cfg(test)]
mod recovery_tests {
    use super::*;
    use crate::policy::{
        ApplyMode, ConsumePoint, DrainPolicy, QueueMode, RoutingDisposition, WakeMode,
    };

    fn policy(apply_mode: ApplyMode) -> crate::policy::PolicyDecision {
        crate::policy::PolicyDecision {
            apply_mode,
            wake_mode: WakeMode::WakeIfIdle,
            queue_mode: QueueMode::Fifo,
            consume_point: ConsumePoint::OnRunComplete,
            drain_policy: DrainPolicy::QueueNextTurn,
            routing_disposition: RoutingDisposition::Queue,
            record_transcript: true,
            emit_operator_content: true,
            policy_version: crate::policy_table::generated_default_policy_version(),
        }
    }

    fn generated_runtime_semantics(input: &Input) -> crate::ingress_types::RuntimeInputSemantics {
        crate::ingress_types::RuntimeInputSemantics::try_from_generated_admission(input, true)
            .expect("generated admission semantics")
    }

    fn generated_runtime_semantics_by_kind(
        kind: crate::identifiers::InputKind,
    ) -> crate::ingress_types::RuntimeInputSemantics {
        crate::policy_table::generated_admission_projection_for_kind(
            crate::identifiers::KindId::new(kind),
            true,
        )
        .expect("generated admission semantics")
        .runtime_semantics
    }

    fn state_with_runtime_semantics(
        input: Input,
        decision: crate::policy::PolicyDecision,
        runtime_semantics: crate::ingress_types::RuntimeInputSemantics,
    ) -> crate::input_state::InputState {
        let mut state = crate::input_state::InputState::new_accepted(input.id().clone());
        state.persisted_input = Some(input);
        state.policy = Some(crate::input_state::PolicySnapshot {
            version: crate::policy_table::generated_default_policy_version(),
            decision,
        });
        state.runtime_semantics = Some(runtime_semantics);
        state
    }

    fn queued_seed() -> InputStateSeed {
        let mut seed = InputStateSeed::new_accepted();
        seed.phase = InputLifecycleState::Queued;
        seed.recovery_lane = Some(meerkat_core::types::HandlingMode::Queue);
        seed
    }

    fn queued_seed_with_admission_sequence(sequence: u64) -> InputStateSeed {
        let mut seed = queued_seed();
        seed.admission_sequence = Some(sequence);
        seed
    }

    fn persistable(
        bundle: crate::input_state::StoredInputState,
    ) -> crate::input_state::InputStatePersistenceRecord {
        crate::input_state::InputStatePersistenceRecord::from_machine_snapshot(bundle)
            .expect("test input-state seed should pass generated persistence authority")
    }

    fn serialized_input_states(
        mut states: Vec<crate::input_state::StoredInputState>,
    ) -> Vec<serde_json::Value> {
        states.sort_by_key(|stored| stored.state.input_id.to_string());
        states
            .into_iter()
            .map(|stored| serde_json::to_value(stored).expect("serialize stored input state"))
            .collect()
    }

    async fn seed_terminal_recovery_outbox(
        persistent: &mut PersistentRuntimeDriver,
        session_id: &SessionId,
        runtime_id: &LogicalRuntimeId,
        batch_key: crate::input_state::InteractionTerminalBatchKey,
        candidate: crate::input_state::InteractionTerminalCandidate,
    ) {
        use crate::input_state::{InteractionTerminalOutbox, InteractionTerminalOutboxPhase};

        let input = Input::Prompt(crate::input::PromptInput::new(
            format!("terminal recovery batch {batch_key:?}"),
            None,
        ));
        let input_id = input.id().clone();
        assert!(
            persistent
                .accept_input(input)
                .await
                .expect("accept terminal recovery input")
                .is_accepted(),
            "seed unpublished terminal recovery input"
        );
        let completion_input_ids = vec![input_id.clone()];
        let candidate_digest = crate::input_state::interaction_terminal_payload_digest(&candidate)
            .expect("digest terminal candidate");
        let completion_input_ids_digest =
            crate::input_state::interaction_terminal_payload_digest(&completion_input_ids)
                .expect("digest completion recipients");
        persistent
            .inner_mut()
            .ledger_mut()
            .get_mut(&input_id)
            .expect("accepted terminal recovery input")
            .interaction_terminal_outbox = Some(InteractionTerminalOutbox {
            interaction_id: meerkat_core::interaction::InteractionId(input_id.0),
            input_id: input_id.clone(),
            batch_ordinal: 0,
            batch_key,
            owner_session_id: session_id.clone(),
            owner_agent_runtime_id: Some(runtime_id.0.clone()),
            owner_fence_token: Some(1),
            owner_runtime_generation: Some(1),
            owner_runtime_epoch_id: Some("previous-epoch".to_string()),
            candidate_owner_input_id: input_id.clone(),
            candidate: Some(candidate),
            candidate_digest,
            completion_input_ids: Some(completion_input_ids),
            completion_input_ids_digest,
            phase: InteractionTerminalOutboxPhase::Candidate,
        });
    }

    async fn candidate_terminal_recovery_driver(
        runtime_name: &str,
        run_id: RunId,
    ) -> (
        LogicalRuntimeId,
        Arc<crate::store::InMemoryRuntimeStore>,
        DriverEntry,
        Vec<serde_json::Value>,
    ) {
        use crate::input_state::{InteractionTerminalBatchKey, InteractionTerminalCandidate};
        use crate::store::RuntimeStore;

        let runtime_id = LogicalRuntimeId::new(runtime_name);
        let session_id = SessionId::new();
        let store = Arc::new(crate::store::InMemoryRuntimeStore::new());
        let store_trait: Arc<dyn RuntimeStore> = store.clone();
        let blob_store: Arc<dyn meerkat_core::BlobStore> =
            Arc::new(meerkat_store::MemoryBlobStore::new());
        let mut persistent =
            PersistentRuntimeDriver::new(runtime_id.clone(), store_trait, blob_store);
        persistent
            .inner_mut()
            .install_registered_authority_for_test(
                crate::meerkat_machine::dsl::SessionId::from_domain(&session_id),
                Some(&runtime_id),
                Some(2),
                Some(crate::meerkat_machine::dsl::Generation::from(2)),
                Some(crate::meerkat_machine::dsl::RuntimeEpochId::from(
                    "current-epoch".to_string(),
                )),
                crate::store::SupervisorAuthoritySnapshot::UnboundNoReceipt,
            )
            .expect("seed current runtime attachment authority");
        seed_terminal_recovery_outbox(
            &mut persistent,
            &session_id,
            &runtime_id,
            InteractionTerminalBatchKey::Run { run_id },
            InteractionTerminalCandidate::CompletedWithoutResult,
        )
        .await;

        let driver = DriverEntry::Persistent(persistent);
        let shell_before = driver
            .as_driver()
            .stored_input_states_snapshot()
            .expect("snapshot seeded terminal batch");
        for stored in &shell_before {
            store
                .persist_input_state(&runtime_id, &persistable(stored.clone()))
                .await
                .expect("persist seeded terminal batch");
        }
        (
            runtime_id,
            store,
            driver,
            serialized_input_states(shell_before),
        )
    }

    fn completed_terminal_events(
        batch: &InteractionTerminalRecoveryBatch,
    ) -> Vec<meerkat_core::event::AgentEvent> {
        batch
            .interaction_ids
            .iter()
            .map(
                |interaction_id| meerkat_core::event::AgentEvent::InteractionComplete {
                    interaction_id: *interaction_id,
                    result: String::new(),
                    structured_output: None,
                },
            )
            .collect()
    }

    #[tokio::test]
    async fn terminal_recovery_cancelled_before_cas_leaves_both_images_unchanged() {
        use crate::store::RuntimeStore;

        let (runtime_id, store, driver, before) =
            candidate_terminal_recovery_driver("terminal-recovery-cancel-before-cas", RunId::new())
                .await;
        let entered = Arc::new(crate::tokio::sync::Notify::new());
        let release = Arc::new(crate::tokio::sync::Notify::new());
        store.block_next_input_state_batch_cas_before_mutation(
            Arc::clone(&entered),
            Arc::clone(&release),
        );
        let driver = Arc::new(crate::tokio::sync::Mutex::new(driver));
        let task = crate::tokio::spawn({
            let driver = Arc::clone(&driver);
            async move {
                driver
                    .lock()
                    .await
                    .interaction_terminal_recovery_batches()
                    .await
            }
        });
        entered.notified().await;
        task.abort();
        let join_error = match task.await {
            Ok(_) => panic!("blocked recovery completed instead of being cancelled"),
            Err(error) => error,
        };
        assert!(
            join_error.is_cancelled(),
            "test cancellation must stop recovery before the durable CAS"
        );
        release.notify_waiters();

        let shell_after_cancel = serialized_input_states(
            driver
                .lock()
                .await
                .as_driver()
                .stored_input_states_snapshot()
                .expect("snapshot shell after cancellation"),
        );
        let durable_after_cancel = serialized_input_states(
            store
                .load_input_states(&runtime_id)
                .await
                .expect("load durable image after cancellation"),
        );
        assert_eq!(shell_after_cancel, before);
        assert_eq!(durable_after_cancel, before);

        let batches = driver
            .lock()
            .await
            .interaction_terminal_recovery_batches()
            .await
            .expect("retry after pre-CAS cancellation must succeed");
        assert_eq!(batches.len(), 1);
    }

    #[tokio::test]
    async fn terminal_recovery_cancelled_after_store_commit_retries_idempotently() {
        use crate::store::RuntimeStore;

        let (runtime_id, store, driver, before) =
            candidate_terminal_recovery_driver("terminal-recovery-cancel-after-cas", RunId::new())
                .await;
        let entered = Arc::new(crate::tokio::sync::Notify::new());
        let release = Arc::new(crate::tokio::sync::Notify::new());
        store.block_next_input_state_batch_cas_after_commit(
            Arc::clone(&entered),
            Arc::clone(&release),
        );
        let driver = Arc::new(crate::tokio::sync::Mutex::new(driver));
        let task = crate::tokio::spawn({
            let driver = Arc::clone(&driver);
            async move {
                driver
                    .lock()
                    .await
                    .interaction_terminal_recovery_batches()
                    .await
            }
        });
        entered.notified().await;
        task.abort();
        let join_error = match task.await {
            Ok(_) => panic!("blocked recovery completed instead of being cancelled"),
            Err(error) => error,
        };
        assert!(
            join_error.is_cancelled(),
            "test cancellation must drop the store acknowledgement"
        );
        release.notify_waiters();

        let shell_after_cancel = serialized_input_states(
            driver
                .lock()
                .await
                .as_driver()
                .stored_input_states_snapshot()
                .expect("snapshot shell after lost acknowledgement"),
        );
        let durable_after_cancel = serialized_input_states(
            store
                .load_input_states(&runtime_id)
                .await
                .expect("load committed durable image"),
        );
        assert_eq!(
            shell_after_cancel, before,
            "store-first recovery must not publish shell state before CAS acknowledgement"
        );
        assert_ne!(
            durable_after_cancel, before,
            "the injected boundary must observe a committed durable replacement"
        );

        let batches = driver
            .lock()
            .await
            .interaction_terminal_recovery_batches()
            .await
            .expect("byte-identical CAS retry must acknowledge and publish the shell");
        assert_eq!(batches.len(), 1);
        let shell_after_retry = serialized_input_states(
            driver
                .lock()
                .await
                .as_driver()
                .stored_input_states_snapshot()
                .expect("snapshot shell after idempotent retry"),
        );
        let durable_after_retry = serialized_input_states(
            store
                .load_input_states(&runtime_id)
                .await
                .expect("load durable image after idempotent retry"),
        );
        assert_eq!(shell_after_retry, durable_after_retry);
        assert_eq!(durable_after_retry, durable_after_cancel);
    }

    #[tokio::test]
    async fn terminal_finalization_cancelled_after_commit_reconciles_then_adopts_new_owner() {
        use crate::input_state::InteractionTerminalOutboxPhase;
        use crate::store::RuntimeStore;

        let (runtime_id, store, driver, _) = candidate_terminal_recovery_driver(
            "terminal-finalization-cancel-after-commit",
            RunId::new(),
        )
        .await;
        let driver = Arc::new(crate::tokio::sync::Mutex::new(driver));
        let candidate_batch = {
            let mut driver = driver.lock().await;
            let mut batches = driver
                .interaction_terminal_recovery_batches()
                .await
                .expect("initial recovery must adopt the candidate owner");
            assert_eq!(batches.len(), 1);
            batches.remove(0)
        };
        let candidate_owner_input_id = candidate_batch.input_ids[0].clone();
        let events = completed_terminal_events(&candidate_batch);
        let entered = Arc::new(crate::tokio::sync::Notify::new());
        let release = Arc::new(crate::tokio::sync::Notify::new());
        store.block_next_input_state_batch_cas_after_commit(
            Arc::clone(&entered),
            Arc::clone(&release),
        );
        let task = crate::tokio::spawn({
            let driver = Arc::clone(&driver);
            let candidate_owner_input_id = candidate_owner_input_id.clone();
            let events = events.clone();
            async move {
                driver
                    .lock()
                    .await
                    .finalize_interaction_terminal_outboxes(
                        &candidate_owner_input_id,
                        &events,
                        crate::meerkat_machine::dsl::RuntimeCompletionFinalizationObservation::Succeeded,
                    )
                    .await
            }
        });
        entered.notified().await;
        task.abort();
        let join_error = match task.await {
            Ok(_) => panic!("blocked terminal finalization completed instead of being cancelled"),
            Err(error) => error,
        };
        assert!(join_error.is_cancelled());
        release.notify_waiters();

        let shell_phase = {
            let driver = driver.lock().await;
            let stored = driver
                .as_driver()
                .stored_input_states_snapshot()
                .expect("snapshot shell after lost finalization acknowledgement");
            let outbox = stored[0]
                .state
                .interaction_terminal_outbox
                .as_ref()
                .expect("shell terminal outbox");
            outbox.phase.clone()
        };
        assert!(matches!(
            shell_phase,
            InteractionTerminalOutboxPhase::Candidate
        ));
        let durable = store
            .load_input_states(&runtime_id)
            .await
            .expect("load durable finalized outbox");
        assert!(matches!(
            &durable[0]
                .state
                .interaction_terminal_outbox
                .as_ref()
                .expect("durable terminal outbox")
                .phase,
            InteractionTerminalOutboxPhase::Finalized { .. }
        ));

        // Replace machine owner B with C after B's Finalized CAS committed.
        // Recovery must hydrate B's exact committed phase, then use ordinary
        // exact adoption to bind the unpublished batch to C.
        {
            let authority = driver.lock().await.shared_dsl_authority();
            let mut authority = authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut rotated = authority.state().clone();
            rotated.active_fence_token = Some(crate::meerkat_machine::dsl::FenceToken::from(3));
            rotated.active_runtime_generation =
                Some(crate::meerkat_machine::dsl::Generation::from(3));
            rotated.active_runtime_epoch_id = Some(
                crate::meerkat_machine::dsl::RuntimeEpochId::from("replacement-epoch".to_string()),
            );
            *authority =
                crate::meerkat_machine::dsl::MeerkatMachineAuthority::recover_from_state(rotated)
                    .expect("rotate terminal recovery owner without losing input authority");
        }

        let recovered = driver
            .lock()
            .await
            .interaction_terminal_recovery_batches()
            .await
            .expect("lost finalization acknowledgement must reconcile and adopt");
        assert_eq!(recovered.len(), 1);
        assert!(matches!(
            &recovered[0].phase,
            InteractionTerminalRecoveryPhase::Finalized { .. }
        ));
        let shell = driver
            .lock()
            .await
            .as_driver()
            .stored_input_states_snapshot()
            .expect("snapshot reconciled finalized shell");
        assert_eq!(
            shell[0]
                .state
                .interaction_terminal_outbox
                .as_ref()
                .expect("reconciled terminal outbox")
                .owner_fence_token,
            Some(3)
        );
    }

    #[tokio::test]
    async fn terminal_publication_cancelled_after_commit_reconciles_published_image() {
        use crate::input_state::InteractionTerminalOutboxPhase;
        use crate::store::RuntimeStore;

        let (runtime_id, store, driver, _) = candidate_terminal_recovery_driver(
            "terminal-publication-cancel-after-commit",
            RunId::new(),
        )
        .await;
        let driver = Arc::new(crate::tokio::sync::Mutex::new(driver));
        let candidate_batch = {
            let mut driver = driver.lock().await;
            let mut batches = driver
                .interaction_terminal_recovery_batches()
                .await
                .expect("initial recovery must adopt the candidate owner");
            assert_eq!(batches.len(), 1);
            batches.remove(0)
        };
        let candidate_owner_input_id = candidate_batch.input_ids[0].clone();
        let events = completed_terminal_events(&candidate_batch);
        driver
            .lock()
            .await
            .finalize_interaction_terminal_outboxes(
                &candidate_owner_input_id,
                &events,
                crate::meerkat_machine::dsl::RuntimeCompletionFinalizationObservation::Succeeded,
            )
            .await
            .expect("finalize terminal outbox before publication");
        let finalized = driver
            .lock()
            .await
            .interaction_terminal_recovery_batches()
            .await
            .expect("load finalized recovery batch");
        let InteractionTerminalRecoveryPhase::Finalized {
            events: finalized_events,
            ..
        } = &finalized[0].phase
        else {
            panic!("terminal batch must be finalized before publication");
        };
        let receipts = finalized_events
            .iter()
            .enumerate()
            .map(|(index, event)| {
                meerkat_core::lifecycle::core_executor::CoreInteractionTerminalPublicationReceipt::try_new(
                    event,
                    u64::try_from(index).expect("receipt index") + 1,
                )
                .expect("build exact terminal publication receipt")
            })
            .collect::<Vec<_>>();

        let entered = Arc::new(crate::tokio::sync::Notify::new());
        let release = Arc::new(crate::tokio::sync::Notify::new());
        store.block_next_input_state_batch_cas_after_commit(
            Arc::clone(&entered),
            Arc::clone(&release),
        );
        let task = crate::tokio::spawn({
            let driver = Arc::clone(&driver);
            let candidate_owner_input_id = candidate_owner_input_id.clone();
            let receipts = receipts.clone();
            async move {
                driver
                    .lock()
                    .await
                    .mark_interaction_terminal_outboxes_published(
                        &candidate_owner_input_id,
                        &receipts,
                    )
                    .await
            }
        });
        entered.notified().await;
        task.abort();
        let join_error = match task.await {
            Ok(_) => panic!("blocked terminal publication completed instead of being cancelled"),
            Err(error) => error,
        };
        assert!(join_error.is_cancelled());
        release.notify_waiters();

        let shell = driver
            .lock()
            .await
            .as_driver()
            .stored_input_states_snapshot()
            .expect("snapshot shell after lost publication acknowledgement");
        assert!(matches!(
            &shell[0]
                .state
                .interaction_terminal_outbox
                .as_ref()
                .expect("shell terminal outbox")
                .phase,
            InteractionTerminalOutboxPhase::Finalized { .. }
        ));
        let durable_after_cancel = store
            .load_input_states(&runtime_id)
            .await
            .expect("load durable published outbox");
        assert!(matches!(
            &durable_after_cancel[0]
                .state
                .interaction_terminal_outbox
                .as_ref()
                .expect("durable terminal outbox")
                .phase,
            InteractionTerminalOutboxPhase::Published { .. }
        ));

        let recovered = driver
            .lock()
            .await
            .interaction_terminal_recovery_batches()
            .await
            .expect("lost publication acknowledgement must reconcile");
        assert!(
            recovered.is_empty(),
            "published batches are already drained"
        );
        let shell_after_retry = serialized_input_states(
            driver
                .lock()
                .await
                .as_driver()
                .stored_input_states_snapshot()
                .expect("snapshot shell after publication reconciliation"),
        );
        assert_eq!(
            shell_after_retry,
            serialized_input_states(durable_after_cancel)
        );
    }

    #[tokio::test]
    async fn split_run_terminal_recovery_rejects_before_any_owner_mutation() {
        use crate::input_state::{InteractionTerminalBatchKey, InteractionTerminalCandidate};
        use crate::store::RuntimeStore;

        let runtime_id = LogicalRuntimeId::new("split-run-terminal-recovery");
        let session_id = SessionId::new();
        let store = Arc::new(crate::store::InMemoryRuntimeStore::new());
        let store_trait: Arc<dyn RuntimeStore> = store.clone();
        let blob_store: Arc<dyn meerkat_core::BlobStore> =
            Arc::new(meerkat_store::MemoryBlobStore::new());
        let mut persistent =
            PersistentRuntimeDriver::new(runtime_id.clone(), store_trait, blob_store);
        persistent
            .inner_mut()
            .install_registered_authority_for_test(
                crate::meerkat_machine::dsl::SessionId::from_domain(&session_id),
                Some(&runtime_id),
                Some(2),
                Some(crate::meerkat_machine::dsl::Generation::from(2)),
                Some(crate::meerkat_machine::dsl::RuntimeEpochId::from(
                    "current-epoch".to_string(),
                )),
                crate::store::SupervisorAuthoritySnapshot::UnboundNoReceipt,
            )
            .expect("seed current runtime attachment authority");

        for run_id in [RunId::new(), RunId::new()] {
            seed_terminal_recovery_outbox(
                &mut persistent,
                &session_id,
                &runtime_id,
                InteractionTerminalBatchKey::Run { run_id },
                InteractionTerminalCandidate::CompletedWithoutResult,
            )
            .await;
        }

        let mut driver = DriverEntry::Persistent(persistent);
        let shell_before = driver
            .as_driver()
            .stored_input_states_snapshot()
            .expect("snapshot seeded terminal batches");
        for stored in &shell_before {
            store
                .persist_input_state(&runtime_id, &persistable(stored.clone()))
                .await
                .expect("persist seeded terminal batch");
        }
        let shell_before = serialized_input_states(shell_before);
        let durable_before = serialized_input_states(
            store
                .load_input_states(&runtime_id)
                .await
                .expect("load durable terminal batches"),
        );

        let error = match driver.interaction_terminal_recovery_batches().await {
            Ok(_) => panic!("distinct unpublished run batches must fail closed"),
            Err(error) => error,
        };
        assert!(
            matches!(
                error,
                RuntimeDriverError::RecoveryCorruption { ref reason }
                    if reason.contains("distinct runs")
            ),
            "unexpected split-run recovery error: {error}"
        );

        let shell_after = serialized_input_states(
            driver
                .as_driver()
                .stored_input_states_snapshot()
                .expect("snapshot terminal batches after rejection"),
        );
        let durable_after = serialized_input_states(
            store
                .load_input_states(&runtime_id)
                .await
                .expect("load durable terminal batches after rejection"),
        );
        assert_eq!(
            shell_after, shell_before,
            "split-run rejection must not adopt either shell owner binding"
        );
        assert_eq!(
            durable_after, durable_before,
            "split-run rejection must not persist either owner adoption"
        );
    }

    #[tokio::test]
    async fn conflicting_machine_run_correlation_rejects_before_any_owner_mutation() {
        use crate::input_state::{InteractionTerminalBatchKey, InteractionTerminalCandidate};
        use crate::store::RuntimeStore;

        let runtime_id = LogicalRuntimeId::new("conflicting-run-terminal-recovery");
        let session_id = SessionId::new();
        let store = Arc::new(crate::store::InMemoryRuntimeStore::new());
        let store_trait: Arc<dyn RuntimeStore> = store.clone();
        let blob_store: Arc<dyn meerkat_core::BlobStore> =
            Arc::new(meerkat_store::MemoryBlobStore::new());
        let mut persistent =
            PersistentRuntimeDriver::new(runtime_id.clone(), store_trait, blob_store);
        persistent
            .inner_mut()
            .install_registered_authority_for_test(
                crate::meerkat_machine::dsl::SessionId::from_domain(&session_id),
                Some(&runtime_id),
                Some(2),
                Some(crate::meerkat_machine::dsl::Generation::from(2)),
                Some(crate::meerkat_machine::dsl::RuntimeEpochId::from(
                    "current-epoch".to_string(),
                )),
                crate::store::SupervisorAuthoritySnapshot::UnboundNoReceipt,
            )
            .expect("seed current runtime attachment authority");
        let durable_run_id = RunId::new();
        seed_terminal_recovery_outbox(
            &mut persistent,
            &session_id,
            &runtime_id,
            InteractionTerminalBatchKey::Run {
                run_id: durable_run_id,
            },
            InteractionTerminalCandidate::CompletedWithoutResult,
        )
        .await;

        let machine_run_id = RunId::new();
        let mut driver = DriverEntry::Persistent(persistent);
        machine_recover_runtime_completion_result_correlation(&driver, &machine_run_id)
            .expect("seed a different machine-owned run correlation");
        let shell_before = driver
            .as_driver()
            .stored_input_states_snapshot()
            .expect("snapshot seeded terminal batch");
        for stored in &shell_before {
            store
                .persist_input_state(&runtime_id, &persistable(stored.clone()))
                .await
                .expect("persist seeded terminal batch");
        }
        let shell_before = serialized_input_states(shell_before);
        let durable_before = serialized_input_states(
            store
                .load_input_states(&runtime_id)
                .await
                .expect("load durable terminal batch"),
        );

        let error = match driver.interaction_terminal_recovery_batches().await {
            Ok(_) => panic!("a different machine-owned run correlation must fail closed"),
            Err(error) => error,
        };
        assert!(
            matches!(error, RuntimeDriverError::ValidationFailed { .. }),
            "unexpected correlation recovery error: {error}"
        );
        assert_eq!(
            serialized_input_states(
                driver
                    .as_driver()
                    .stored_input_states_snapshot()
                    .expect("snapshot terminal batch after rejection"),
            ),
            shell_before,
            "correlation rejection must precede shell owner adoption"
        );
        assert_eq!(
            serialized_input_states(
                store
                    .load_input_states(&runtime_id)
                    .await
                    .expect("load durable terminal batch after rejection"),
            ),
            durable_before,
            "correlation rejection must precede durable owner adoption"
        );
    }

    #[test]
    fn recovered_durability_retention_is_generated() {
        let mut state = crate::input_state::InputState::new_accepted(InputId::new());

        state.durability = Some(crate::input::InputDurability::Ephemeral);
        assert_eq!(
            machine_classify_recovered_input_durability(&state)
                .expect("generated durability classification should accept ephemeral witness"),
            crate::meerkat_machine::dsl::RecoveredInputRecoveryDisposition::Discard
        );

        state.durability = Some(crate::input::InputDurability::Durable);
        assert_eq!(
            machine_classify_recovered_input_durability(&state)
                .expect("generated durability classification should accept durable witness"),
            crate::meerkat_machine::dsl::RecoveredInputRecoveryDisposition::Retain
        );

        state.durability = None;
        assert_eq!(
            machine_classify_recovered_input_durability(&state)
                .expect("generated durability classification should accept missing witness"),
            crate::meerkat_machine::dsl::RecoveredInputRecoveryDisposition::Retain
        );
    }

    fn recovered_admission_rejection(state: crate::input_state::InputState) -> String {
        let seed = queued_seed();
        let entry = machine_build_recovered_ingress_entry(&state, &seed)
            .expect("test state should carry the recovered admission witness fields");
        let mut driver = crate::driver::ephemeral::EphemeralRuntimeDriver::new(
            LogicalRuntimeId::new("recovered-admission-authority-test"),
        );
        let input_id = state.input_id.clone();
        let err = driver
            .admit_recovered_to_ingress(
                input_id.clone(),
                entry.runtime_semantics,
                &state,
                &seed,
                None,
                None,
                None,
            )
            .expect_err("generated recovered-admission authority must reject this witness");
        assert!(
            driver.admitted_runtime_semantics(&input_id).is_none(),
            "rejected recovered admission must not record runtime semantics"
        );
        err.to_string()
    }

    #[test]
    fn recovered_ingress_entry_requires_persisted_input_for_content_shape() {
        let mut state = crate::input_state::InputState::new_accepted(InputId::new());
        state.policy = Some(crate::input_state::PolicySnapshot {
            version: crate::policy_table::generated_default_policy_version(),
            decision: policy(ApplyMode::StageRunStart),
        });

        assert!(
            machine_build_recovered_ingress_entry(&state, &queued_seed()).is_none(),
            "recovery must not synthesize an unknown admitted-input content shape"
        );
    }

    #[test]
    fn recovered_ingress_entry_requires_runtime_semantics_stamp() {
        let input = Input::Prompt(crate::input::PromptInput::new("queued prompt", None));
        let mut state = crate::input_state::InputState::new_accepted(input.id().clone());
        state.persisted_input = Some(input);
        state.policy = Some(crate::input_state::PolicySnapshot {
            version: crate::policy_table::generated_default_policy_version(),
            decision: policy(ApplyMode::StageRunStart),
        });

        assert!(
            machine_build_recovered_ingress_entry(&state, &queued_seed()).is_none(),
            "recovery must not derive execution kind from payload/policy when the durable runtime semantics stamp is missing"
        );
    }

    #[test]
    fn recovered_ingress_entry_requires_generated_recovery_lane() {
        let input = Input::Prompt(crate::input::PromptInput::new("queued prompt", None));
        let mut state = crate::input_state::InputState::new_accepted(input.id().clone());
        state.runtime_semantics = Some(generated_runtime_semantics(&input));
        state.persisted_input = Some(input);
        let mut seed = queued_seed();
        seed.recovery_lane = None;

        assert!(
            machine_build_recovered_ingress_entry(&state, &seed).is_none(),
            "recovery must not derive lane metadata from policy or handling-mode caches"
        );
    }

    #[test]
    fn recovered_admission_authority_rejects_prompt_stamped_as_resume_pending() {
        let input = Input::Prompt(crate::input::PromptInput::new("queued prompt", None));
        let decision = policy(ApplyMode::StageRunStart);
        let mut runtime_semantics = generated_runtime_semantics(&input);
        runtime_semantics.execution_kind =
            meerkat_core::lifecycle::RuntimeExecutionKind::ResumePending;
        let state = state_with_runtime_semantics(input, decision, runtime_semantics);

        assert!(
            recovered_admission_rejection(state)
                .contains("generated recovered-admission authority"),
            "recovery must reject a prompt row whose durable stamp says ResumePending"
        );
    }

    #[test]
    fn recovered_admission_authority_rejects_continuation_stamped_as_content_turn() {
        let input = Input::Continuation(
            crate::input::ContinuationInput::detached_background_op_completed(),
        );
        let decision = policy(ApplyMode::StageRunBoundary);
        let mut runtime_semantics = generated_runtime_semantics(&input);
        runtime_semantics.execution_kind =
            meerkat_core::lifecycle::RuntimeExecutionKind::ContentTurn;
        let state = state_with_runtime_semantics(input, decision, runtime_semantics);

        assert!(
            recovered_admission_rejection(state)
                .contains("generated recovered-admission authority"),
            "recovery must reject a continuation row whose durable stamp says ContentTurn"
        );
    }

    #[test]
    fn recovered_admission_authority_rejects_immediate_boundary_on_queue_lane() {
        let input = Input::Prompt(crate::input::PromptInput::new("queued prompt", None));
        let decision = policy(ApplyMode::StageRunStart);
        let mut runtime_semantics = generated_runtime_semantics(&input);
        runtime_semantics.boundary =
            meerkat_core::lifecycle::run_primitive::RunApplyBoundary::Immediate;
        let state = state_with_runtime_semantics(input, decision, runtime_semantics);

        assert!(
            recovered_admission_rejection(state)
                .contains("generated recovered-admission authority"),
            "recovery must reject an immediate runtime boundary without a generated steer lane"
        );
    }

    #[test]
    fn recovered_admission_authority_rejects_terminal_intent_mismatch() {
        let input = crate::input::peer_response_terminal_input(
            meerkat_core::comms::PeerId::new(),
            Some(meerkat_core::comms::PeerName::new("reviewer").expect("peer name")),
            meerkat_core::PeerCorrelationId::new(),
            meerkat_contracts::PeerResponseTerminalStatusWire::Completed,
            serde_json::json!({"status": "complete"}),
        );
        let decision = policy(ApplyMode::StageRunStart);
        let mut runtime_semantics = generated_runtime_semantics(&input);
        runtime_semantics.peer_response_terminal_apply_intent = None;
        let state = state_with_runtime_semantics(input, decision, runtime_semantics);

        assert!(
            recovered_admission_rejection(state)
                .contains("generated recovered-admission authority"),
            "recovery must reject a terminal peer-response stamp with missing terminal apply intent"
        );
    }

    #[tokio::test]
    async fn persistent_recovery_rejects_state_without_runtime_semantics_stamp() {
        use crate::store::RuntimeStore;

        let runtime_id = LogicalRuntimeId::new("missing-runtime-semantics-stamp");
        let input = Input::Prompt(crate::input::PromptInput::new("queued prompt", None));
        let input_id = input.id().clone();
        let mut state = crate::input_state::InputState::new_accepted(input_id.clone());
        state.persisted_input = Some(input);
        state.policy = Some(crate::input_state::PolicySnapshot {
            version: crate::policy_table::generated_default_policy_version(),
            decision: policy(ApplyMode::StageRunStart),
        });
        let mut seed = InputStateSeed::new_accepted();
        seed.phase = InputLifecycleState::Queued;
        let bundle = crate::input_state::StoredInputState { state, seed };
        let store = crate::store::memory::InMemoryRuntimeStore::new();
        store
            .persist_input_state(&runtime_id, &persistable(bundle))
            .await
            .expect("persist corrupt recovered input state");

        let mut driver = crate::driver::ephemeral::EphemeralRuntimeDriver::new(runtime_id.clone());
        let err = machine_recover_persistent_driver(&store, &runtime_id, &mut driver)
            .await
            .expect_err("unstamped recovered input must not recover through local classification");

        assert!(
            err.to_string()
                .contains("missing runtime execution semantics stamp"),
            "unexpected recovery error: {err}"
        );
        assert!(
            driver.input_state(&input_id).is_none(),
            "failed recovery must not leave a ledger-only input row"
        );
    }

    #[tokio::test]
    async fn persistent_recovery_accepts_state_without_policy_snapshot_when_lane_witness_exists() {
        use crate::store::RuntimeStore;

        let runtime_id = LogicalRuntimeId::new("policyless-with-generated-lane");
        let input = Input::Prompt(crate::input::PromptInput::new("queued prompt", None));
        let input_id = input.id().clone();
        let mut state = crate::input_state::InputState::new_accepted(input_id.clone());
        state.runtime_semantics = Some(generated_runtime_semantics(&input));
        state.persisted_input = Some(input);
        let seed = queued_seed_with_admission_sequence(10);
        let bundle = crate::input_state::StoredInputState { state, seed };
        let store = crate::store::memory::InMemoryRuntimeStore::new();
        store
            .persist_input_state(&runtime_id, &persistable(bundle))
            .await
            .expect("persist corrupt recovered input state");

        let mut driver = crate::driver::ephemeral::EphemeralRuntimeDriver::new(runtime_id.clone());
        machine_recover_persistent_driver(&store, &runtime_id, &mut driver)
            .await
            .expect("policy-less recovered input should recover through generated lane witness");

        assert!(
            driver.input_state(&input_id).is_some(),
            "successful recovery should restore the input row"
        );
        assert_eq!(driver.queue_lane(), vec![input_id]);
    }

    #[tokio::test]
    async fn persistent_recovery_normalizes_cold_running_lifecycle_to_idle() {
        use crate::store::RuntimeStore;

        let session_id = SessionId::new();
        let runtime_id = LogicalRuntimeId::for_session(&session_id);
        let store = crate::store::memory::InMemoryRuntimeStore::new();
        store
            .commit_machine_lifecycle(
                &runtime_id,
                crate::store::MachineLifecycleCommit::new_with_binding(
                    RuntimeState::Running,
                    crate::store::MachineLifecycleBindingFacts::new(None, None, None, None),
                    crate::store::SupervisorAuthoritySnapshot::UnboundNoReceipt,
                ),
                &[],
            )
            .await
            .expect("persist cold Running lifecycle fixture");

        let mut driver = crate::driver::ephemeral::EphemeralRuntimeDriver::new(runtime_id.clone());
        machine_recover_persistent_driver(&store, &runtime_id, &mut driver)
            .await
            .expect("cold Running lifecycle should normalize through generated authority");

        assert_eq!(driver.phase(), RuntimeState::Idle);
        assert!(driver.current_run_id().is_none());
        assert!(driver.pre_run_phase().is_none());
    }

    #[tokio::test]
    async fn cold_runtime_driver_totalizes_decoded_torn_observation_product() {
        use crate::store::{MachineLifecycleObservation, RuntimeStore};

        let session_id = SessionId::new();
        let runtime_id = LogicalRuntimeId::for_session(&session_id);
        let run_id = RunId::new();
        let states = [
            None,
            Some(RuntimeState::Initializing),
            Some(RuntimeState::Idle),
            Some(RuntimeState::Attached),
            Some(RuntimeState::Running),
            Some(RuntimeState::Retired),
            Some(RuntimeState::Stopped),
            Some(RuntimeState::Destroyed),
        ];
        let pre_run_phases = [
            None,
            Some(crate::store::MachineLifecyclePreRunPhase::Idle),
            Some(crate::store::MachineLifecyclePreRunPhase::Attached),
            Some(crate::store::MachineLifecyclePreRunPhase::Retired),
        ];

        let mut classified = 0usize;
        for state in states {
            for binding_bits in 0u8..16 {
                for run_present in [false, true] {
                    for pre_run_phase in pre_run_phases {
                        let raw = serde_json::to_vec(&serde_json::json!({
                            "record_version": crate::store::MACHINE_LIFECYCLE_STORE_RECORD_VERSION,
                            "runtime_state": state,
                            "binding": {
                                "agent_runtime_id": (binding_bits & 1 != 0).then(|| runtime_id.0.clone()),
                                "fence_token": (binding_bits & 2 != 0).then_some(41_u64),
                                "runtime_generation": (binding_bits & 4 != 0).then_some(7_u64),
                                "runtime_epoch_id": (binding_bits & 8 != 0).then_some("dead-process-epoch"),
                            },
                            "current_run_id": run_present.then_some(&run_id),
                            "pre_run_phase": pre_run_phase,
                            "supervisor_authority": { "kind": "unbound_no_receipt" },
                            "unregister_progress": null,
                        }))
                        .expect("serialize torn runtime observation");
                        let store = crate::store::InMemoryRuntimeStore::new();
                        store.seed_machine_lifecycle_raw(&runtime_id, raw).await;

                        let reconciled = reconcile_runtime_authority_for_cold_recovery(
                            &store,
                            &runtime_id,
                            &session_id,
                        )
                        .await
                        .expect("every decoded torn tuple must converge through a typed decision");
                        assert_eq!(
                            reconciled.authority.state().lifecycle_phase,
                            crate::meerkat_machine::dsl::MeerkatPhase::Idle
                        );
                        let MachineLifecycleObservation::Decoded { record, .. } =
                            store.observe_machine_lifecycle(&runtime_id).await.unwrap()
                        else {
                            panic!("reconciled lifecycle row must decode");
                        };
                        assert_eq!(record.runtime_state(), Some(RuntimeState::Idle));
                        assert_eq!(
                            record.binding(),
                            &crate::store::MachineLifecycleBindingFacts::default()
                        );
                        assert_eq!(
                            record.run(),
                            &crate::store::MachineLifecycleRunFacts::default()
                        );
                        classified += 1;
                    }
                }
            }
        }
        assert_eq!(classified, 1_024);
    }

    #[tokio::test]
    async fn cold_runtime_driver_blocks_unsafe_bytes_and_requeues_transport_or_conflict() {
        use crate::store::{MachineLifecycleObservation, RuntimeStore};

        let session_id = SessionId::new();
        let runtime_id = LogicalRuntimeId::for_session(&session_id);
        for raw in [
            br"not-json".to_vec(),
            br#"{"record_version":999,"future":"opaque"}"#.to_vec(),
        ] {
            let store = crate::store::InMemoryRuntimeStore::new();
            store
                .seed_machine_lifecycle_raw(&runtime_id, raw.clone())
                .await;
            let error = match reconcile_runtime_authority_for_cold_recovery(
                &store,
                &runtime_id,
                &session_id,
            )
            .await
            {
                Ok(_) => panic!("malformed and unsupported rows must fail closed"),
                Err(error) => error,
            };
            assert!(matches!(
                error,
                RuntimeDriverError::RecoveryRepairBlocked { .. }
            ));
            assert_eq!(
                store
                    .load_machine_lifecycle_record(&runtime_id)
                    .await
                    .unwrap(),
                Some(raw),
                "blocked evidence must remain byte-identical"
            );
        }

        let unavailable = crate::store::InMemoryRuntimeStore::new();
        unavailable.fail_next_machine_lifecycle_observation();
        assert!(matches!(
            reconcile_runtime_authority_for_cold_recovery(&unavailable, &runtime_id, &session_id,)
                .await,
            Err(RuntimeDriverError::RecoveryBackoff { .. })
        ));

        let conflict = crate::store::InMemoryRuntimeStore::new();
        conflict
            .commit_machine_lifecycle(
                &runtime_id,
                crate::store::MachineLifecycleCommit::new_with_binding(
                    RuntimeState::Running,
                    crate::store::MachineLifecycleBindingFacts::default(),
                    crate::store::SupervisorAuthoritySnapshot::UnboundNoReceipt,
                ),
                &[],
            )
            .await
            .unwrap();
        conflict.conflict_next_machine_lifecycle_cas();
        assert!(matches!(
            reconcile_runtime_authority_for_cold_recovery(&conflict, &runtime_id, &session_id,)
                .await,
            Err(RuntimeDriverError::RecoveryBackoff { .. })
        ));
        assert!(matches!(
            conflict.observe_machine_lifecycle(&runtime_id).await.unwrap(),
            MachineLifecycleObservation::Decoded { ref record, .. }
                if record.runtime_state() == Some(RuntimeState::Running)
        ));

        reconcile_runtime_authority_for_cold_recovery(&conflict, &runtime_id, &session_id)
            .await
            .expect("requeued conflict must converge on its next pass");
        let fixed_version = conflict
            .observe_machine_lifecycle(&runtime_id)
            .await
            .unwrap()
            .version()
            .cloned()
            .expect("fixed lifecycle row version");
        reconcile_runtime_authority_for_cold_recovery(&conflict, &runtime_id, &session_id)
            .await
            .expect("fixed point must remain converged");
        assert_eq!(
            conflict
                .observe_machine_lifecycle(&runtime_id)
                .await
                .unwrap()
                .version(),
            Some(&fixed_version),
            "a converged second pass must not rewrite the lifecycle row"
        );
    }

    #[tokio::test]
    async fn persistent_recovery_rejects_state_without_persisted_input_content_shape() {
        use crate::store::RuntimeStore;

        let runtime_id = LogicalRuntimeId::new("missing-persisted-input-content-shape");
        let input_id = InputId::new();
        let store = crate::store::memory::InMemoryRuntimeStore::new();
        let bundle = crate::input_state::StoredInputState::new_accepted(input_id.clone());
        store
            .persist_input_state(&runtime_id, &persistable(bundle))
            .await
            .expect("persist corrupt recovered input state");

        let mut driver = crate::driver::ephemeral::EphemeralRuntimeDriver::new(runtime_id.clone());
        let err = machine_recover_persistent_driver(&store, &runtime_id, &mut driver)
            .await
            .expect_err("missing persisted input must not recover as a ledger-only row");

        assert!(
            err.to_string()
                .contains("cannot derive admitted-input content shape"),
            "unexpected recovery error: {err}"
        );
        assert!(
            driver.input_state(&input_id).is_none(),
            "failed recovery must not leave a ledger-only input row"
        );
    }

    #[tokio::test]
    async fn persistent_recovery_rejects_queued_state_without_admission_sequence() {
        use crate::store::RuntimeStore;

        let runtime_id = LogicalRuntimeId::new("missing-admission-sequence");
        let input = Input::Prompt(crate::input::PromptInput::new("queued prompt", None));
        let input_id = input.id().clone();
        let decision = policy(ApplyMode::StageRunStart);
        let runtime_semantics = generated_runtime_semantics(&input);
        let state = state_with_runtime_semantics(input, decision, runtime_semantics);
        let bundle = crate::input_state::StoredInputState {
            state,
            seed: queued_seed(),
        };
        let store = crate::store::memory::InMemoryRuntimeStore::new();
        store
            .persist_input_state(&runtime_id, &persistable(bundle))
            .await
            .expect("persist corrupt recovered input state");

        let mut driver = crate::driver::ephemeral::EphemeralRuntimeDriver::new(runtime_id.clone());
        let err = machine_recover_persistent_driver(&store, &runtime_id, &mut driver)
            .await
            .expect_err("queued recovery must fail closed without generated order witness");

        assert!(
            err.to_string().contains("RecoverInputLifecycle"),
            "unexpected recovery error: {err}"
        );
        assert!(
            driver.input_state(&input_id).is_none(),
            "failed recovery must not leave a ledger-only input row"
        );
    }

    #[tokio::test]
    async fn persistent_recovery_restores_queue_order_from_admission_sequence() {
        use crate::store::RuntimeStore;

        let runtime_id = LogicalRuntimeId::new("restore-admission-sequence");
        let first_input = Input::Prompt(crate::input::PromptInput::new("first", None));
        let second_input = Input::Prompt(crate::input::PromptInput::new("second", None));
        let first_id = first_input.id().clone();
        let second_id = second_input.id().clone();
        let first_decision = policy(ApplyMode::StageRunStart);
        let second_decision = policy(ApplyMode::StageRunStart);
        let first_state = state_with_runtime_semantics(
            first_input,
            first_decision.clone(),
            generated_runtime_semantics_by_kind(crate::identifiers::InputKind::Prompt),
        );
        let second_state = state_with_runtime_semantics(
            second_input,
            second_decision.clone(),
            generated_runtime_semantics_by_kind(crate::identifiers::InputKind::Prompt),
        );
        let first_bundle = crate::input_state::StoredInputState {
            state: first_state,
            seed: queued_seed_with_admission_sequence(10),
        };
        let second_bundle = crate::input_state::StoredInputState {
            state: second_state,
            seed: queued_seed_with_admission_sequence(20),
        };
        let store = crate::store::memory::InMemoryRuntimeStore::new();
        store
            .persist_input_state(&runtime_id, &persistable(second_bundle))
            .await
            .expect("persist later input first");
        store
            .persist_input_state(&runtime_id, &persistable(first_bundle))
            .await
            .expect("persist earlier input second");

        let mut driver = crate::driver::ephemeral::EphemeralRuntimeDriver::new(runtime_id.clone());
        machine_recover_persistent_driver(&store, &runtime_id, &mut driver)
            .await
            .expect("recover queued inputs");

        assert_eq!(
            driver.queue_lane(),
            vec![first_id.clone(), second_id.clone()],
            "queue projection must follow the machine-owned recovered admission sequence, not store iteration order"
        );
        assert_eq!(
            driver.admission_order(),
            vec![first_id.clone(), second_id.clone()],
            "public admission projection must follow the machine-owned recovered admission sequence, not store iteration order"
        );
        assert_eq!(driver.input_admission_sequence(&first_id), Some(10));
        assert_eq!(driver.input_admission_sequence(&second_id), Some(20));
    }

    #[tokio::test]
    async fn ephemeral_recovery_rejects_state_without_persisted_input_content_shape() {
        let runtime_id = LogicalRuntimeId::new("ephemeral-missing-persisted-content-shape");
        let mut driver = crate::driver::ephemeral::EphemeralRuntimeDriver::new(runtime_id);
        let input = Input::Prompt(crate::input::PromptInput::new("queued prompt", None));
        let input_id = input.id().clone();

        driver.accept_input(input).await.expect("accept input");
        driver
            .ledger_mut()
            .get_mut(&input_id)
            .expect("accepted input ledger state")
            .persisted_input = None;

        let err = machine_recover_ephemeral_driver(&mut driver)
            .expect_err("missing persisted input must not be silently skipped");

        assert!(
            err.to_string()
                .contains("cannot derive admitted-input content shape"),
            "unexpected recovery error: {err}"
        );
    }
}
