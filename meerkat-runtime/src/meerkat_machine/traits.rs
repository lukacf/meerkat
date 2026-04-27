use super::*;
use crate::input_state::StoredInputState;

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl SessionServiceRuntimeExt for MeerkatMachine {
    fn runtime_mode(&self) -> RuntimeMode {
        self.mode
    }

    async fn accept_input(
        &self,
        session_id: &SessionId,
        input: Input,
    ) -> Result<AcceptOutcome, RuntimeDriverError> {
        let runtime_id = MeerkatMachine::logical_runtime_id(session_id);
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::Ingest { runtime_id, input },
            )
            .await
            .map_err(MeerkatMachine::driver_error_from_command_error)?
        {
            MeerkatMachineCommandResult::AcceptOutcome(outcome) => Ok(outcome),
            other => Err(RuntimeDriverError::Internal(format!(
                "unexpected MeerkatMachineCommandResult for SessionServiceRuntimeExt::accept_input: {other:?}"
            ))),
        }
    }

    async fn accept_input_with_completion(
        &self,
        session_id: &SessionId,
        input: Input,
    ) -> Result<(AcceptOutcome, Option<crate::completion::CompletionHandle>), RuntimeDriverError>
    {
        Box::pin(MeerkatMachine::accept_input_with_completion(
            self, session_id, input,
        ))
        .await
    }

    async fn runtime_state(
        &self,
        session_id: &SessionId,
    ) -> Result<RuntimeState, RuntimeDriverError> {
        let runtime_id = MeerkatMachine::logical_runtime_id(session_id);
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::RuntimeState { runtime_id },
            )
            .await
            .map_err(MeerkatMachine::driver_error_from_command_error)?
        {
            MeerkatMachineCommandResult::RuntimeState(state) => Ok(state),
            other => Err(RuntimeDriverError::Internal(format!(
                "unexpected MeerkatMachineCommandResult for SessionServiceRuntimeExt::runtime_state: {other:?}"
            ))),
        }
    }

    async fn retire_runtime(
        &self,
        session_id: &SessionId,
    ) -> Result<RetireReport, RuntimeDriverError> {
        let runtime_id = MeerkatMachine::logical_runtime_id(session_id);
        match self
            .execute_meerkat_machine_command(None, MeerkatMachineCommand::Retire { runtime_id })
            .await
            .map_err(MeerkatMachine::driver_error_from_command_error)?
        {
            MeerkatMachineCommandResult::RetireReport(report) => Ok(report),
            other => Err(RuntimeDriverError::Internal(format!(
                "unexpected MeerkatMachineCommandResult for SessionServiceRuntimeExt::retire_runtime: {other:?}"
            ))),
        }
    }

    async fn reset_runtime(
        &self,
        session_id: &SessionId,
    ) -> Result<ResetReport, RuntimeDriverError> {
        let runtime_id = MeerkatMachine::logical_runtime_id(session_id);
        match self
            .execute_meerkat_machine_command(None, MeerkatMachineCommand::Reset { runtime_id })
            .await
            .map_err(MeerkatMachine::driver_error_from_command_error)?
        {
            MeerkatMachineCommandResult::ResetReport(report) => Ok(report),
            other => Err(RuntimeDriverError::Internal(format!(
                "unexpected MeerkatMachineCommandResult for SessionServiceRuntimeExt::reset_runtime: {other:?}"
            ))),
        }
    }

    async fn input_state(
        &self,
        session_id: &SessionId,
        input_id: &InputId,
    ) -> Result<Option<StoredInputState>, RuntimeDriverError> {
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::InputState {
                    session_id: session_id.clone(),
                    input_id: input_id.clone(),
                },
            )
            .await
            .map_err(MeerkatMachine::driver_error_from_command_error)?
        {
            MeerkatMachineCommandResult::InputState(state) => Ok(state),
            other => Err(RuntimeDriverError::Internal(format!(
                "unexpected MeerkatMachineCommandResult for SessionServiceRuntimeExt::input_state: {other:?}"
            ))),
        }
    }

    async fn list_active_inputs(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<InputId>, RuntimeDriverError> {
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::ListActiveInputs {
                    session_id: session_id.clone(),
                },
            )
            .await
            .map_err(MeerkatMachine::driver_error_from_command_error)?
        {
            MeerkatMachineCommandResult::ActiveInputs(inputs) => Ok(inputs),
            other => Err(RuntimeDriverError::Internal(format!(
                "unexpected MeerkatMachineCommandResult for SessionServiceRuntimeExt::list_active_inputs: {other:?}"
            ))),
        }
    }

    async fn reconfigure_session_llm_identity(
        &self,
        session_id: &SessionId,
        request: SessionLlmReconfigureRequest,
    ) -> Result<SessionLlmReconfigureReport, RuntimeDriverError> {
        let command = self
            .prepare_reconfigure_session_llm_command(session_id, request)
            .await?;
        match self
            .execute_meerkat_machine_command(None, command)
            .await
            .map_err(MeerkatMachine::driver_error_from_command_error)?
        {
            MeerkatMachineCommandResult::LlmReconfigured(report) => Ok(report),
            other => Err(RuntimeDriverError::Internal(format!(
                "unexpected MeerkatMachineCommandResult for SessionServiceRuntimeExt::reconfigure_session_llm_identity: {other:?}"
            ))),
        }
    }

    async fn realtime_attachment_status(
        &self,
        session_id: &SessionId,
    ) -> Result<crate::meerkat_machine_types::RealtimeAttachmentStatus, RuntimeDriverError> {
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::RuntimeRealtimeAttachmentStatus {
                    session_id: session_id.clone(),
                },
            )
            .await
            .map_err(MeerkatMachine::driver_error_from_command_error)?
        {
            MeerkatMachineCommandResult::RealtimeAttachmentStatus(status) => Ok(status),
            other => Err(RuntimeDriverError::Internal(format!(
                "unexpected MeerkatMachineCommandResult for SessionServiceRuntimeExt::realtime_attachment_status: {other:?}"
            ))),
        }
    }

    /// Wave-c C-9c R4: fully-projected public channel status. Reads DSL
    /// state (attachment + reconnect-progress) and returns a
    /// ready-to-serialize `RealtimeChannelStatus` with real
    /// `attempt_count` / `next_retry_at` / `deadline_at` sourced from the
    /// overlay-projected fields, not hard-coded defaults.
    async fn realtime_channel_status(
        &self,
        session_id: &SessionId,
    ) -> Result<meerkat_contracts::RealtimeChannelStatus, RuntimeDriverError> {
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::RuntimeRealtimeChannelStatus {
                    session_id: session_id.clone(),
                },
            )
            .await
            .map_err(MeerkatMachine::driver_error_from_command_error)?
        {
            MeerkatMachineCommandResult::RealtimeChannelStatus(status) => Ok(status),
            other => Err(RuntimeDriverError::Internal(format!(
                "unexpected MeerkatMachineCommandResult for SessionServiceRuntimeExt::realtime_channel_status: {other:?}"
            ))),
        }
    }

    async fn configure_model_routing_baseline(
        &self,
        session_id: &SessionId,
        baseline_model: meerkat_core::lifecycle::run_primitive::ModelId,
        realtime_capable: bool,
    ) -> Result<(), RuntimeDriverError> {
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::ConfigureModelRoutingBaseline {
                    session_id: session_id.clone(),
                    baseline_model,
                    realtime_capable,
                },
            )
            .await
            .map_err(MeerkatMachine::driver_error_from_command_error)?
        {
            MeerkatMachineCommandResult::Unit => Ok(()),
            other => Err(RuntimeDriverError::Internal(format!(
                "unexpected MeerkatMachineCommandResult for SessionServiceRuntimeExt::configure_model_routing_baseline: {other:?}"
            ))),
        }
    }

    async fn session_model_routing_status(
        &self,
        session_id: &SessionId,
    ) -> Result<meerkat_core::image_generation::SessionModelRoutingStatus, RuntimeDriverError> {
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::SessionModelRoutingStatus {
                    session_id: session_id.clone(),
                },
            )
            .await
            .map_err(MeerkatMachine::driver_error_from_command_error)?
        {
            MeerkatMachineCommandResult::SessionModelRoutingStatus(status) => Ok(status),
            other => Err(RuntimeDriverError::Internal(format!(
                "unexpected MeerkatMachineCommandResult for SessionServiceRuntimeExt::session_model_routing_status: {other:?}"
            ))),
        }
    }

    async fn resolve_image_generation_plan(
        &self,
        session_id: &SessionId,
        operation_id: meerkat_core::image_generation::ImageOperationId,
        request: &meerkat_core::image_generation::GenerateImageRequest,
    ) -> Result<
        Result<
            meerkat_core::image_generation::ImageGenerationResolvedPlan,
            meerkat_core::image_generation::ImageOperationDenialReason,
        >,
        RuntimeDriverError,
    > {
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::ResolveImageGenerationPlan {
                    session_id: session_id.clone(),
                    operation_id,
                    request: Box::new(request.clone()),
                },
            )
            .await
            .map_err(MeerkatMachine::driver_error_from_command_error)?
        {
            MeerkatMachineCommandResult::ImageGenerationResolvedPlan(plan) => Ok(plan),
            other => Err(RuntimeDriverError::Internal(format!(
                "unexpected MeerkatMachineCommandResult for SessionServiceRuntimeExt::resolve_image_generation_plan: {other:?}"
            ))),
        }
    }

    async fn request_switch_turn(
        &self,
        session_id: &SessionId,
        request: crate::meerkat_machine_types::SwitchTurnRequest,
    ) -> Result<meerkat_core::image_generation::SwitchTurnControlResult, RuntimeDriverError> {
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::RequestSwitchTurn {
                    session_id: session_id.clone(),
                    request: Box::new(request),
                },
            )
            .await
            .map_err(MeerkatMachine::driver_error_from_command_error)?
        {
            MeerkatMachineCommandResult::SwitchTurnControlResult(result) => Ok(result),
            other => Err(RuntimeDriverError::Internal(format!(
                "unexpected MeerkatMachineCommandResult for SessionServiceRuntimeExt::request_switch_turn: {other:?}"
            ))),
        }
    }

    async fn admit_model_routing_assistant_turn(
        &self,
        session_id: &SessionId,
    ) -> Result<(), RuntimeDriverError> {
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::AdmitModelRoutingAssistantTurn {
                    session_id: session_id.clone(),
                },
            )
            .await
            .map_err(MeerkatMachine::driver_error_from_command_error)?
        {
            MeerkatMachineCommandResult::Unit => Ok(()),
            other => Err(RuntimeDriverError::Internal(format!(
                "unexpected MeerkatMachineCommandResult for SessionServiceRuntimeExt::admit_model_routing_assistant_turn: {other:?}"
            ))),
        }
    }

    async fn begin_image_operation(
        &self,
        session_id: &SessionId,
        request: crate::meerkat_machine_types::ImageOperationRoutingRequest,
    ) -> Result<crate::meerkat_machine_types::ImageOperationRoutingResult, RuntimeDriverError> {
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::BeginImageOperation {
                    session_id: session_id.clone(),
                    request: Box::new(request),
                },
            )
            .await
            .map_err(MeerkatMachine::driver_error_from_command_error)?
        {
            MeerkatMachineCommandResult::ImageOperationRoutingResult(result) => Ok(result),
            other => Err(RuntimeDriverError::Internal(format!(
                "unexpected MeerkatMachineCommandResult for SessionServiceRuntimeExt::begin_image_operation: {other:?}"
            ))),
        }
    }

    async fn activate_image_operation_override(
        &self,
        session_id: &SessionId,
        operation_id: meerkat_core::image_generation::ImageOperationId,
    ) -> Result<meerkat_core::image_generation::ImageOperationPhase, RuntimeDriverError> {
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::ActivateImageOperationOverride {
                    session_id: session_id.clone(),
                    operation_id,
                },
            )
            .await
            .map_err(MeerkatMachine::driver_error_from_command_error)?
        {
            MeerkatMachineCommandResult::ImageOperationPhase(phase) => Ok(phase),
            other => Err(RuntimeDriverError::Internal(format!(
                "unexpected MeerkatMachineCommandResult for SessionServiceRuntimeExt::activate_image_operation_override: {other:?}"
            ))),
        }
    }

    async fn complete_image_operation(
        &self,
        session_id: &SessionId,
        operation_id: meerkat_core::image_generation::ImageOperationId,
        terminal: meerkat_core::image_generation::ImageOperationTerminalClass,
    ) -> Result<meerkat_core::image_generation::ImageOperationPhase, RuntimeDriverError> {
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::CompleteImageOperation {
                    session_id: session_id.clone(),
                    operation_id,
                    terminal,
                },
            )
            .await
            .map_err(MeerkatMachine::driver_error_from_command_error)?
        {
            MeerkatMachineCommandResult::ImageOperationPhase(phase) => Ok(phase),
            other => Err(RuntimeDriverError::Internal(format!(
                "unexpected MeerkatMachineCommandResult for SessionServiceRuntimeExt::complete_image_operation: {other:?}"
            ))),
        }
    }

    async fn restore_image_operation_override(
        &self,
        session_id: &SessionId,
        operation_id: meerkat_core::image_generation::ImageOperationId,
    ) -> Result<meerkat_core::image_generation::ImageOperationPhase, RuntimeDriverError> {
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::RestoreImageOperationOverride {
                    session_id: session_id.clone(),
                    operation_id,
                },
            )
            .await
            .map_err(MeerkatMachine::driver_error_from_command_error)?
        {
            MeerkatMachineCommandResult::ImageOperationPhase(phase) => Ok(phase),
            other => Err(RuntimeDriverError::Internal(format!(
                "unexpected MeerkatMachineCommandResult for SessionServiceRuntimeExt::restore_image_operation_override: {other:?}"
            ))),
        }
    }
}

// ---------------------------------------------------------------------------
// RuntimeControlPlane implementation
// ---------------------------------------------------------------------------

impl MeerkatMachine {
    pub(super) fn logical_runtime_id(session_id: &SessionId) -> LogicalRuntimeId {
        LogicalRuntimeId::new(session_id.to_string())
    }

    pub(super) fn post_admission_signal_from_effects(
        effects: &[crate::meerkat_machine::dsl::MeerkatMachineEffect],
    ) -> crate::driver::ephemeral::PostAdmissionSignal {
        effects
            .iter()
            .find_map(|effect| match effect {
                crate::meerkat_machine::dsl::MeerkatMachineEffect::PostAdmissionSignal {
                    signal,
                } => Some(match signal {
                    crate::meerkat_machine::dsl::PostAdmissionSignalKind::WakeLoop => {
                        crate::driver::ephemeral::PostAdmissionSignal::WakeLoop
                    }
                    crate::meerkat_machine::dsl::PostAdmissionSignalKind::InterruptYielding => {
                        crate::driver::ephemeral::PostAdmissionSignal::InterruptYielding
                    }
                    crate::meerkat_machine::dsl::PostAdmissionSignalKind::RequestImmediateProcessing => {
                        crate::driver::ephemeral::PostAdmissionSignal::RequestImmediateProcessing
                    }
                }),
                _ => None,
            })
            .unwrap_or(crate::driver::ephemeral::PostAdmissionSignal::None)
    }

    pub(super) fn driver_error_from_command_error(
        err: MeerkatMachineCommandError,
    ) -> RuntimeDriverError {
        match err {
            MeerkatMachineCommandError::Driver(err) => err,
            MeerkatMachineCommandError::Control(err) => {
                Self::driver_error_from_control_plane_error(err)
            }
        }
    }

    pub(super) fn control_plane_error_from_command_error(
        err: MeerkatMachineCommandError,
    ) -> RuntimeControlPlaneError {
        match err {
            MeerkatMachineCommandError::Control(err) => err,
            MeerkatMachineCommandError::Driver(err) => {
                RuntimeControlPlaneError::Internal(err.to_string())
            }
        }
    }

    pub(super) fn driver_error_from_control_plane_error(
        err: RuntimeControlPlaneError,
    ) -> RuntimeDriverError {
        match err {
            RuntimeControlPlaneError::NotFound(_) => RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            },
            RuntimeControlPlaneError::InvalidState { state } => {
                RuntimeDriverError::NotReady { state }
            }
            RuntimeControlPlaneError::StoreError(message)
            | RuntimeControlPlaneError::Internal(message) => RuntimeDriverError::Internal(message),
        }
    }

    /// Resolve a LogicalRuntimeId to a SessionId for internal lookup.
    ///
    /// The adapter uses `LogicalRuntimeId::new(session_id.to_string())` when
    /// creating drivers, so runtime IDs are UUID strings that parse back to
    /// SessionId.
    pub(super) fn resolve_session_id(
        runtime_id: &LogicalRuntimeId,
    ) -> Result<SessionId, RuntimeControlPlaneError> {
        runtime_id
            .0
            .parse::<uuid::Uuid>()
            .map(SessionId)
            .map_err(|_| RuntimeControlPlaneError::NotFound(runtime_id.clone()))
    }

    pub(super) async fn existing_session_runtime_state(
        &self,
        session_id: &SessionId,
    ) -> Option<RuntimeState> {
        let sessions = self.sessions.read().await;
        let entry = sessions.get(session_id)?;
        // DSL is the single source of truth. The async runtime-loop stop
        // path fires `RuntimeExecutorExited` through `Weak<MeerkatMachine>`
        // so `lifecycle_phase` is Stopped by the time any observer reads it.
        let authority = entry
            .dsl_authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Some(dsl_authority::runtime_phase_from_authority(&authority))
    }

    /// Look up the session entry for a runtime ID, returning a control-plane error
    /// if not found.
    pub(super) async fn lookup_entry(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<
        (
            SessionId,
            SharedDriver,
            SharedCompletionRegistry,
            Option<mpsc::Sender<()>>,
        ),
        RuntimeControlPlaneError,
    > {
        let session_id = Self::resolve_session_id(runtime_id)?;
        let sessions = self.sessions.read().await;
        let entry = sessions
            .get(&session_id)
            .ok_or_else(|| RuntimeControlPlaneError::NotFound(runtime_id.clone()))?;
        Ok((
            session_id,
            entry.driver.clone(),
            entry.completions.clone(),
            entry.wake_sender(),
        ))
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl crate::traits::RuntimeControlPlane for MeerkatMachine {
    async fn ingest(
        &self,
        runtime_id: &LogicalRuntimeId,
        input: Input,
    ) -> Result<AcceptOutcome, RuntimeControlPlaneError> {
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::Ingest {
                    runtime_id: runtime_id.clone(),
                    input,
                },
            )
            .await
            .map_err(MeerkatMachine::control_plane_error_from_command_error)?
        {
            MeerkatMachineCommandResult::AcceptOutcome(outcome) => Ok(outcome),
            other => Err(RuntimeControlPlaneError::Internal(format!(
                "unexpected MeerkatMachineCommandResult for ingest: {other:?}"
            ))),
        }
    }

    async fn publish_event(
        &self,
        event: crate::runtime_event::RuntimeEventEnvelope,
    ) -> Result<(), RuntimeControlPlaneError> {
        match self
            .execute_meerkat_machine_command(None, MeerkatMachineCommand::PublishEvent { event })
            .await
            .map_err(MeerkatMachine::control_plane_error_from_command_error)?
        {
            MeerkatMachineCommandResult::Unit => Ok(()),
            other => Err(RuntimeControlPlaneError::Internal(format!(
                "unexpected MeerkatMachineCommandResult for publish_event: {other:?}"
            ))),
        }
    }

    async fn retire(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<RetireReport, RuntimeControlPlaneError> {
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::Retire {
                    runtime_id: runtime_id.clone(),
                },
            )
            .await
            .map_err(MeerkatMachine::control_plane_error_from_command_error)?
        {
            MeerkatMachineCommandResult::RetireReport(report) => Ok(report),
            other => Err(RuntimeControlPlaneError::Internal(format!(
                "unexpected MeerkatMachineCommandResult for retire: {other:?}"
            ))),
        }
    }

    async fn recycle(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<RecycleReport, RuntimeControlPlaneError> {
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::Recycle {
                    runtime_id: runtime_id.clone(),
                },
            )
            .await
            .map_err(MeerkatMachine::control_plane_error_from_command_error)?
        {
            MeerkatMachineCommandResult::RecycleReport(report) => Ok(report),
            other => Err(RuntimeControlPlaneError::Internal(format!(
                "unexpected MeerkatMachineCommandResult for recycle: {other:?}"
            ))),
        }
    }

    async fn reset(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<crate::traits::ResetReport, RuntimeControlPlaneError> {
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::Reset {
                    runtime_id: runtime_id.clone(),
                },
            )
            .await
            .map_err(MeerkatMachine::control_plane_error_from_command_error)?
        {
            MeerkatMachineCommandResult::ResetReport(report) => Ok(report),
            other => Err(RuntimeControlPlaneError::Internal(format!(
                "unexpected MeerkatMachineCommandResult for reset: {other:?}"
            ))),
        }
    }

    async fn recover(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<RecoveryReport, RuntimeControlPlaneError> {
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::Recover {
                    runtime_id: runtime_id.clone(),
                },
            )
            .await
            .map_err(MeerkatMachine::control_plane_error_from_command_error)?
        {
            MeerkatMachineCommandResult::RecoveryReport(report) => Ok(report),
            other => Err(RuntimeControlPlaneError::Internal(format!(
                "unexpected MeerkatMachineCommandResult for recover: {other:?}"
            ))),
        }
    }

    async fn destroy(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<DestroyReport, RuntimeControlPlaneError> {
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::Destroy {
                    runtime_id: runtime_id.clone(),
                },
            )
            .await
            .map_err(MeerkatMachine::control_plane_error_from_command_error)?
        {
            MeerkatMachineCommandResult::DestroyReport(report) => Ok(report),
            other => Err(RuntimeControlPlaneError::Internal(format!(
                "unexpected MeerkatMachineCommandResult for destroy: {other:?}"
            ))),
        }
    }

    async fn runtime_state(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<RuntimeState, RuntimeControlPlaneError> {
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::RuntimeState {
                    runtime_id: runtime_id.clone(),
                },
            )
            .await
            .map_err(MeerkatMachine::control_plane_error_from_command_error)?
        {
            MeerkatMachineCommandResult::RuntimeState(state) => Ok(state),
            other => Err(RuntimeControlPlaneError::Internal(format!(
                "unexpected MeerkatMachineCommandResult for runtime_state: {other:?}"
            ))),
        }
    }

    async fn load_boundary_receipt(
        &self,
        runtime_id: &LogicalRuntimeId,
        run_id: &RunId,
        sequence: u64,
    ) -> Result<Option<meerkat_core::lifecycle::RunBoundaryReceipt>, RuntimeControlPlaneError> {
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::LoadBoundaryReceipt {
                    runtime_id: runtime_id.clone(),
                    run_id: run_id.clone(),
                    sequence,
                },
            )
            .await
            .map_err(MeerkatMachine::control_plane_error_from_command_error)?
        {
            MeerkatMachineCommandResult::BoundaryReceipt(receipt) => Ok(receipt),
            other => Err(RuntimeControlPlaneError::Internal(format!(
                "unexpected MeerkatMachineCommandResult for load_boundary_receipt: {other:?}"
            ))),
        }
    }
}
