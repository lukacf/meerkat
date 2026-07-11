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

pub(crate) struct PreparedDestroy {
    pub(crate) report: DestroyReport,
    pub(crate) lifecycle: PreparedDestroyLifecycle,
}

impl DriverEntry {
    pub(crate) fn runtime_id(&self) -> &LogicalRuntimeId {
        match self {
            DriverEntry::Ephemeral(d) => d.runtime_id(),
            DriverEntry::Persistent(d) => d.runtime_id(),
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
    ) -> Result<(), RuntimeDriverError> {
        match self {
            DriverEntry::Ephemeral(_) => Ok(()),
            DriverEntry::Persistent(d) => {
                d.machine_commit_completed_boundary_snapshot(receipt, session_snapshot)
                    .await
            }
        }
    }

    pub(crate) async fn machine_realize_run_failed(
        &mut self,
        run_id: RunId,
        contributing_input_ids: Vec<InputId>,
        replay_plan: crate::driver::ephemeral::ReplayQueuedContributorsPlan,
        terminal_error: &str,
        runtime_apply_failure: Option<&CoreApplyFailureCause>,
        recoverable: bool,
    ) -> Result<(), RuntimeDriverError> {
        match self {
            DriverEntry::Ephemeral(d) => {
                let _ = (terminal_error, runtime_apply_failure, recoverable);
                d.machine_realize_run_failed(&run_id, &contributing_input_ids, &replay_plan)
            }
            DriverEntry::Persistent(d) => {
                d.machine_realize_run_failed(
                    &run_id,
                    &contributing_input_ids,
                    &replay_plan,
                    terminal_error,
                    runtime_apply_failure,
                    recoverable,
                )
                .await
            }
        }
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
    owner_session_id: Option<crate::meerkat_machine::dsl::SessionId>,
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
        Ok(Self {
            generated_plan:
                generated_kernel_command_capabilities::CommandPlanKind::AuthorizedRuntimeLoopRunCommit,
            run_id,
            consumed_input_ids,
            commit_input_id,
            receipt,
            owner_session_id: preview.owner_session_id,
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

    fn owner_session_id(&self) -> Option<&crate::meerkat_machine::dsl::SessionId> {
        self.owner_session_id.as_ref()
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

pub(crate) async fn machine_commit_service_turn_terminal_receipt(
    driver: &mut DriverEntry,
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
    let turn_needs_completion = {
        let authority = driver.shared_dsl_authority();
        let auth = authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        !matches!(
            auth.state().turn_phase,
            crate::meerkat_machine::dsl::TurnPhase::Completed
                | crate::meerkat_machine::dsl::TurnPhase::Failed
                | crate::meerkat_machine::dsl::TurnPhase::Cancelled
        )
    };
    let terminal_checkpoint = driver.rollback_snapshot();
    if turn_needs_completion && let Err(err) = machine_apply_turn_run_completed(driver, &run_id) {
        driver.restore_rollback_snapshot(terminal_checkpoint);
        return Err(err);
    }
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
                .publish_service_turn_terminal_lifecycle(rollback, projection.phase)
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

pub(crate) async fn machine_recover_persistent_driver(
    store: &dyn crate::store::RuntimeStore,
    runtime_id: &LogicalRuntimeId,
    driver: &mut crate::driver::ephemeral::EphemeralRuntimeDriver,
) -> Result<RecoveryReport, RuntimeDriverError> {
    let recovered_lifecycle = crate::store::load_machine_lifecycle(store, runtime_id)
        .await
        .map_err(|err| RuntimeDriverError::Internal(err.to_string()))?;
    let recovered_runtime_state = recovered_lifecycle
        .as_ref()
        .map(crate::store::MachineLifecycleSnapshot::runtime_state);
    if let Some(snapshot) = recovered_lifecycle {
        let session_id = driver.session_authority_id_for_recovery();
        let binding = snapshot.binding();
        let agent_runtime_id = binding
            .agent_runtime_id()
            .map(|value| LogicalRuntimeId::new(value.to_owned()));
        driver.recover_runtime_authority_from_binding_observation(
            session_id,
            snapshot.runtime_state(),
            agent_runtime_id.as_ref(),
            binding.fence_token(),
            binding
                .runtime_generation()
                .map(crate::meerkat_machine::dsl::Generation::from),
            binding
                .runtime_epoch_id()
                .map(crate::meerkat_machine::dsl::RuntimeEpochId::from),
            snapshot.supervisor_authority().clone(),
        )?;
    }

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

    for (input_id, _input) in recovered_payloads {
        let should_requeue =
            driver.input_phase(&input_id) == Some(crate::input_state::InputLifecycleState::Queued);
        if should_requeue && !driver.has_queued_input(&input_id) {
            return Err(RuntimeDriverError::Internal(format!(
                "persistent recover left queued input '{input_id}' out of the runtime queue projection"
            )));
        }
    }

    if let Some(runtime_state) = recovered_runtime_state
        && matches!(
            runtime_state,
            RuntimeState::Stopped | RuntimeState::Destroyed
        )
    {
        let active = driver.active_input_ids();
        if !active.is_empty() {
            return Err(RuntimeDriverError::Internal(format!(
                "store corruption: recovered runtime-state projection '{}' conflicts with {} active inputs",
                runtime_state,
                active.len()
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
        driver.contract_force_runtime_authority(
            RuntimeState::Running,
            Some(RunId::new()),
            Some(RuntimeState::Attached),
        );
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

pub(crate) async fn commit_runtime_loop_run(
    driver: &SharedDriver,
    run_id: RunId,
    consumed_input_ids: Vec<InputId>,
    receipt: meerkat_core::lifecycle::RunBoundaryReceiptDraft,
    session_snapshot: Option<Vec<u8>>,
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

    let terminal_checkpoint = driver.rollback_snapshot();
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
    if commit_authority.owner_session_id().is_none() {
        driver.restore_rollback_snapshot(terminal_checkpoint);
        return Err(RuntimeLoopRunCommitError::Rejected(
            RuntimeDriverError::Internal(
                "runtime-loop run commit authority carried no owner session".to_string(),
            ),
        ));
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
        .machine_commit_completed_boundary_snapshot(&receipt, session_snapshot.as_ref())
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

#[cfg(test)]
pub(crate) fn machine_resolve_pre_resolved_runtime_completion_result(
    run_id: Option<&RunId>,
    terminal: crate::meerkat_machine::dsl::RuntimeCompletionTerminalObservation,
    finalization: crate::meerkat_machine::dsl::RuntimeCompletionFinalizationObservation,
) -> Result<RuntimeCompletionResultAuthority, RuntimeDriverError> {
    let dsl_run_id = run_id.map(crate::meerkat_machine::dsl::RunId::from_domain);
    let mut authority = crate::meerkat_machine::dsl::MeerkatMachineAuthority::new();
    let session_id = SessionId::new();

    apply_runtime_completion_authority_preview(
        &mut authority,
        crate::meerkat_machine::dsl::MeerkatMachineInput::RecoverRuntimeAuthority {
            session_id: crate::meerkat_machine::dsl::SessionId::from_domain(&session_id),
            state: if dsl_run_id.is_some() {
                crate::meerkat_machine::dsl::RuntimeLifecycleObservedState::Running
            } else {
                crate::meerkat_machine::dsl::RuntimeLifecycleObservedState::Idle
            },
            agent_runtime_id: Some(crate::meerkat_machine::dsl::AgentRuntimeId::from(
                "pre-resolved-completion",
            )),
            fence_token: Some(crate::meerkat_machine::dsl::FenceToken::from(0)),
            runtime_generation: Some(crate::meerkat_machine::dsl::Generation::from(0)),
            runtime_epoch_id: None,
            current_run_id: dsl_run_id.clone(),
            pre_run_phase: dsl_run_id
                .as_ref()
                .map(|_| crate::meerkat_machine::dsl::PreRunPhase::Idle),
            silent_intent_overrides: std::collections::BTreeSet::new(),
        },
        "RecoverRuntimeAuthority",
    )?;

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
        failure.message().to_owned(),
        Some(failure),
        false,
        None,
    )
    .await
}

pub(crate) async fn fail_machine_run(
    driver: &SharedDriver,
    run_id: RunId,
    failure: super::MeerkatMachineRunFailure,
) -> Result<(), RuntimeLoopRunFailError> {
    fail_runtime_loop_run_inner(
        driver,
        run_id,
        failure.error,
        None,
        failure.machine_terminal_failure_observed,
        failure.source,
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
    let terminal_checkpoint = driver.rollback_snapshot();
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

async fn fail_runtime_loop_run_inner(
    driver: &SharedDriver,
    run_id: RunId,
    terminal_error: String,
    runtime_apply_failure: Option<CoreApplyFailureCause>,
    machine_terminal_failure_observed: bool,
    terminal_failure_source: Option<crate::meerkat_machine::dsl::RunFailureSourceKind>,
) -> Result<(), RuntimeLoopRunFailError> {
    let mut driver = driver.lock().await;
    let failed_run_id = run_id.clone();
    let staged_input_ids = machine_staged_contributors(&driver);
    machine_validate_run_failed(&driver, &staged_input_ids)
        .map_err(RuntimeLoopRunFailError::Rejected)?;
    let terminal_checkpoint = driver.rollback_snapshot();
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
        .machine_realize_run_failed(
            failed_run_id.clone(),
            staged_input_ids,
            replay_plan,
            &terminal_error,
            runtime_apply_failure.as_ref(),
            true,
        )
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
        driver.contract_force_runtime_authority(
            RuntimeState::Running,
            Some(run_id.clone()),
            Some(RuntimeState::Attached),
        );
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
