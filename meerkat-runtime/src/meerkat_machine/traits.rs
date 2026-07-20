use super::*;
use crate::input_state::StoredInputState;

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl SessionServiceRuntimeExt for MeerkatMachine {
    async fn accept_input(
        &self,
        session_id: &SessionId,
        input: Input,
    ) -> Result<AcceptOutcome, RuntimeDriverError> {
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::AcceptWithCompletion {
                    session_id: session_id.clone(),
                    input,
                    register_completion: false,
                    member_residency: MemberResidencyExpectation::Unfenced,
                    expected_attachment: None,
                },
            )
            .await
            .map_err(MeerkatMachine::driver_error_from_command_error)?
        {
            MeerkatMachineCommandResult::AcceptWithCompletion {
                outcome,
                handle: _,
                admission_signal: _,
            } => Ok(outcome),
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
        tracing::debug!(
            session_id = %session_id,
            input_id = %input.id(),
            "SessionServiceRuntimeExt::accept_input_with_completion entered"
        );
        self.accept_input_with_completion_boxed(session_id, input)
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

    async fn input_state_by_idempotency_key(
        &self,
        session_id: &SessionId,
        idempotency_key: &str,
    ) -> Result<Option<StoredInputState>, RuntimeDriverError> {
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::InputStateByIdempotencyKey {
                    session_id: session_id.clone(),
                    idempotency_key: idempotency_key.to_string(),
                },
            )
            .await
            .map_err(MeerkatMachine::driver_error_from_command_error)?
        {
            MeerkatMachineCommandResult::InputState(state) => Ok(state),
            other => Err(RuntimeDriverError::Internal(format!(
                "unexpected MeerkatMachineCommandResult for SessionServiceRuntimeExt::input_state_by_idempotency_key: {other:?}"
            ))),
        }
    }

    async fn interaction_terminal_status(
        &self,
        session_id: &SessionId,
        selector: crate::terminal_status::InteractionSelector,
    ) -> Result<
        Option<crate::terminal_status::Sourced<crate::terminal_status::InteractionTerminalReport>>,
        RuntimeDriverError,
    > {
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::InteractionTerminalStatus {
                    session_id: session_id.clone(),
                    selector,
                },
            )
            .await
            .map_err(MeerkatMachine::driver_error_from_command_error)?
        {
            MeerkatMachineCommandResult::InteractionTerminalStatus(report) => Ok(report),
            other => Err(RuntimeDriverError::Internal(format!(
                "unexpected MeerkatMachineCommandResult for SessionServiceRuntimeExt::interaction_terminal_status: {other:?}"
            ))),
        }
    }

    async fn run_terminal_status(
        &self,
        session_id: &SessionId,
        run_id: &meerkat_core::lifecycle::RunId,
    ) -> Result<
        crate::terminal_status::Sourced<crate::terminal_status::RunTerminalReport>,
        RuntimeDriverError,
    > {
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::RunTerminalStatus {
                    session_id: session_id.clone(),
                    run_id: run_id.clone(),
                },
            )
            .await
            .map_err(MeerkatMachine::driver_error_from_command_error)?
        {
            MeerkatMachineCommandResult::RunTerminalStatus(report) => Ok(report),
            other => Err(RuntimeDriverError::Internal(format!(
                "unexpected MeerkatMachineCommandResult for SessionServiceRuntimeExt::run_terminal_status: {other:?}"
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
        let host = self.llm_reconfigure_host()?;
        let _turn_finalization_guard = host.acquire_turn_finalization_boundary(session_id).await?;
        self.reconfigure_session_llm_identity_under_turn_finalization_boundary(session_id, request)
            .await
    }

    async fn resolved_session_llm_capabilities(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<SessionLlmCapabilitySurface>, RuntimeDriverError> {
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::ResolvedSessionLlmCapabilities {
                    session_id: session_id.clone(),
                },
            )
            .await
            .map_err(MeerkatMachine::driver_error_from_command_error)?
        {
            MeerkatMachineCommandResult::ResolvedSessionLlmCapabilities(capabilities) => {
                Ok(capabilities)
            }
            other => Err(RuntimeDriverError::Internal(format!(
                "unexpected MeerkatMachineCommandResult for SessionServiceRuntimeExt::resolved_session_llm_capabilities: {other:?}"
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

    async fn request_switch_turn(
        &self,
        session_id: &SessionId,
        request: crate::meerkat_machine_types::SwitchTurnRequest,
    ) -> Result<meerkat_core::image_generation::SwitchTurnControlResult, RuntimeDriverError> {
        // UntilChanged performs a live LLM reconfigure inside the generated
        // switch transaction. Enclose the complete routing + live mutation +
        // persistence sequence in the same stable service boundary as direct
        // reconfigure; the nested host methods deliberately acquire recovery
        // only while the machine mutation gate is held.
        let _turn_finalization_guard = if matches!(
            &request.intent.duration,
            meerkat_core::image_generation::SwitchTurnDuration::UntilChanged
        ) {
            Some(
                self.llm_reconfigure_host()?
                    .acquire_turn_finalization_boundary(session_id)
                    .await?,
            )
        } else {
            None
        };
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

    async fn deny_image_operation_plan(
        &self,
        session_id: &SessionId,
        operation_id: meerkat_core::image_generation::ImageOperationId,
        reason: meerkat_core::image_generation::ImageOperationDenialReason,
    ) -> Result<meerkat_core::image_generation::ImageOperationPhase, RuntimeDriverError> {
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::DenyImageOperationPlan {
                    session_id: session_id.clone(),
                    operation_id,
                    reason,
                },
            )
            .await
            .map_err(MeerkatMachine::driver_error_from_command_error)?
        {
            MeerkatMachineCommandResult::ImageOperationPhase(phase) => Ok(phase),
            other => Err(RuntimeDriverError::Internal(format!(
                "unexpected MeerkatMachineCommandResult for SessionServiceRuntimeExt::deny_image_operation_plan: {other:?}"
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

    async fn classify_image_operation_terminal(
        &self,
        session_id: &SessionId,
        operation_id: meerkat_core::image_generation::ImageOperationId,
        observation: meerkat_core::image_generation::ImageProviderTerminalObservation,
        provider_text: meerkat_core::image_generation::ProviderTextDisposition,
    ) -> Result<meerkat_core::image_generation::ImageOperationTerminalClass, RuntimeDriverError>
    {
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::ClassifyImageOperationTerminal {
                    session_id: session_id.clone(),
                    operation_id,
                    observation,
                    provider_text,
                },
            )
            .await
            .map_err(MeerkatMachine::driver_error_from_command_error)?
        {
            MeerkatMachineCommandResult::ImageOperationTerminalClass(terminal) => Ok(terminal),
            other => Err(RuntimeDriverError::Internal(format!(
                "unexpected MeerkatMachineCommandResult for SessionServiceRuntimeExt::classify_image_operation_terminal: {other:?}"
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
    pub(crate) fn logical_runtime_id(session_id: &SessionId) -> LogicalRuntimeId {
        LogicalRuntimeId::for_session(session_id)
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
            RuntimeControlPlaneError::NotFound(runtime_id) => {
                RuntimeDriverError::NotFound { runtime_id }
            }
            RuntimeControlPlaneError::InvalidState { state } => {
                RuntimeDriverError::NotReady { state }
            }
            RuntimeControlPlaneError::StoreError(message)
            | RuntimeControlPlaneError::Internal(message) => RuntimeDriverError::Internal(message),
        }
    }

    /// Resolve a LogicalRuntimeId to a registered SessionId for internal lookup.
    pub(super) async fn resolve_session_id(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<SessionId, RuntimeControlPlaneError> {
        let sessions = self.sessions.read().await;
        sessions
            .iter()
            .find_map(|(session_id, entry)| {
                (&entry.runtime_id == runtime_id).then(|| session_id.clone())
            })
            .ok_or_else(|| RuntimeControlPlaneError::NotFound(runtime_id.clone()))
    }

    pub(super) async fn existing_session_runtime_state(
        &self,
        session_id: &SessionId,
    ) -> Option<RuntimeState> {
        let sessions = self.sessions.read().await;
        let entry = sessions.get(session_id)?;
        // DSL remains the transition authority for live, non-terminal states.
        // Persistent drivers use the published control projection as the
        // visibility barrier when DSL has crossed a run-return or terminal
        // lifecycle boundary before the durable commit has published it.
        let control = entry.control_snapshot();
        let authority = entry
            .dsl_authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let dsl_phase = dsl_authority::runtime_phase_from_authority(&authority);
        let dsl_pre_run_phase = dsl_authority::pre_run_phase_from_authority(&authority);
        // The visible-phase arbitration verdict is machine-owned: mirror the
        // generated `selected_raw_phase` (the chosen phase without the
        // visibility rewrite). The classifier is total over the pure
        // observations, so a failure is structurally unreachable; if it ever
        // arises we fail closed to the most-terminal phase rather than re-derive
        // a disposition in the shell.
        match crate::meerkat_machine::resolve_visible_runtime_phase(
            dsl_phase,
            dsl_pre_run_phase,
            control.phase,
            control.pre_run_phase,
            self.has_runtime_persistence(),
        ) {
            Ok(plan) => Some(plan.selected_raw_phase),
            Err(reason) => {
                tracing::error!(%session_id, %reason, "MeerkatMachine visible runtime phase resolution failed; failing closed to Destroyed");
                Some(RuntimeState::Destroyed)
            }
        }
    }

    pub(super) async fn existing_session_visible_runtime_state(
        &self,
        session_id: &SessionId,
    ) -> Option<RuntimeState> {
        let sessions = self.sessions.read().await;
        let entry = sessions.get(session_id)?;
        let control = entry.control_snapshot();
        let authority = entry
            .dsl_authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let dsl_phase = dsl_authority::runtime_phase_from_authority(&authority);
        let dsl_pre_run_phase = dsl_authority::pre_run_phase_from_authority(&authority);
        // Mirror the machine-owned `visible_phase` verdict (the externally-
        // visible phase after the Running+pre_run(Retired)->Retired rewrite).
        // The classifier is total; a failure is structurally unreachable and
        // fails closed to the most-terminal phase rather than re-deriving in the
        // shell.
        match crate::meerkat_machine::resolve_visible_runtime_phase(
            dsl_phase,
            dsl_pre_run_phase,
            control.phase,
            control.pre_run_phase,
            self.has_runtime_persistence(),
        ) {
            Ok(plan) => Some(plan.visible_phase),
            Err(reason) => {
                tracing::error!(%session_id, %reason, "MeerkatMachine visible runtime phase resolution failed; failing closed to Destroyed");
                Some(RuntimeState::Destroyed)
            }
        }
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
        let sessions = self.sessions.read().await;
        let (session_id, entry) = sessions
            .iter()
            .find(|(_, entry)| &entry.runtime_id == runtime_id)
            .ok_or_else(|| RuntimeControlPlaneError::NotFound(runtime_id.clone()))?;
        Ok((
            session_id.clone(),
            entry.driver.clone(),
            entry.completions.clone(),
            entry.wake_sender(),
        ))
    }

    /// Re-capture every archive/retire handle only after the exact current
    /// session mutation gate is held. A pending executor attachment can become
    /// attached while a lifecycle command waits for M; in particular, its wake
    /// sender must not remain the pre-M `None` snapshot.
    async fn capture_archive_lease_entry_under_mutation_guard(
        &self,
        runtime_id: &LogicalRuntimeId,
        session_id: &SessionId,
        expected_driver: &SharedDriver,
        _mutation_guard: &crate::tokio::sync::OwnedMutexGuard<()>,
    ) -> Result<
        (
            SharedDriver,
            SharedCompletionRegistry,
            Option<mpsc::Sender<()>>,
            Option<Arc<dyn meerkat_core::lifecycle::CoreExecutorPublicationHandle>>,
        ),
        RuntimeControlPlaneError,
    > {
        let sessions = self.sessions.read().await;
        let entry = sessions.get(session_id).ok_or_else(|| {
            RuntimeControlPlaneError::Internal(format!(
                "runtime {runtime_id} disappeared while its archive/retire mutation gate was held"
            ))
        })?;
        if &entry.runtime_id != runtime_id || !Arc::ptr_eq(&entry.driver, expected_driver) {
            return Err(RuntimeControlPlaneError::Internal(format!(
                "runtime {runtime_id} changed authority while its archive/retire mutation gate was held"
            )));
        }
        Ok((
            Arc::clone(&entry.driver),
            Arc::clone(&entry.completions),
            entry.wake_sender(),
            entry.publication_handle(),
        ))
    }

    /// Fail a lifecycle operation before it attempts the live/mutation gates
    /// when this session is already inside exact unregister convergence.
    ///
    /// Callers own the stable registration transaction. A recovered Draining
    /// retry anchor may have no process-local coordinator after restart; in
    /// that case transfer retry to the process-lifetime cleanup executor and
    /// still return immediately. Archive owns the outer turn-finalization
    /// boundary here, so waiting for unregister would invert that boundary
    /// with the unregister worker's post-stop callback.
    async fn reject_unregister_overlap_under_registration_transaction(
        &self,
        session_id: &SessionId,
    ) -> Result<(), RuntimeControlPlaneError> {
        let (blocked, coordinator_present, pending_finalization, runtime_state) = {
            let sessions = self.sessions.read().await;
            let entry = sessions.get(session_id).ok_or_else(|| {
                RuntimeControlPlaneError::NotFound(LogicalRuntimeId::for_session(session_id))
            })?;
            let registration_phase = entry
                .dsl_authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .state()
                .registration_phase;
            let coordinator_present = entry.unregister_coordinator.is_some();
            let pending_finalization = entry.pending_unregister_finalization.is_some();
            (
                coordinator_present
                    || pending_finalization
                    || registration_phase
                        == crate::meerkat_machine::dsl::RegistrationPhase::Draining,
                coordinator_present,
                pending_finalization,
                entry.control_snapshot().phase,
            )
        };
        if !blocked {
            return Ok(());
        }
        if !coordinator_present && !pending_finalization {
            let cleanup_spawner = super::MachineCleanupTaskSpawner::acquire()
                .map_err(|error| RuntimeControlPlaneError::Internal(error.to_string()))?;
            let machine = self.clone();
            let retry_session_id = session_id.clone();
            drop(cleanup_spawner.spawn(async move {
                if let Err(error) = machine.try_unregister_session(&retry_session_id).await {
                    tracing::warn!(
                        session_id = %retry_session_id,
                        %error,
                        "cold unregister retry started by lifecycle overlap failed"
                    );
                }
            }));
        }
        if pending_finalization {
            return Err(RuntimeControlPlaneError::Internal(
                RuntimeDriverError::UnregisterFinalizationOutcomeUnknown {
                    reason: format!(
                        "session {session_id} retains an ambiguous unregister finalization; retry unregister before applying any other lifecycle mutation"
                    ),
                }
                .to_string(),
            ));
        }
        Err(RuntimeControlPlaneError::InvalidState {
            state: runtime_state,
        })
    }

    /// Acquire the current session mutation authority for an archive before
    /// the session layer takes its recovery/checkpointer gates.
    pub async fn prepare_session_archive_lease(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<super::MachineSessionArchiveLease>, RuntimeControlPlaneError> {
        let runtime_id = LogicalRuntimeId::for_session(session_id);
        // Archive's outer turn-finalization boundary is already held. Take
        // the stable absent-entry transaction before observing the map or
        // durable lifecycle so a delayed capture can neither dispose nor
        // retire a same-SessionId replacement.
        let registration_transaction_guard =
            self.lock_session_registration_transaction(session_id).await;
        let mut recovered_registration_for_archive = false;
        let (resolved_session_id, driver, _, _) = match self.lookup_entry(&runtime_id).await {
            Ok(parts) => parts,
            Err(RuntimeControlPlaneError::NotFound(_)) => {
                // A process restart can leave unfinished runtime realization
                // without a live session entry. Recover only that lifecycle
                // residue before deciding archive has no runtime half. A
                // checkpoint snapshot is session content authority and is
                // intentionally not consulted here; a clean unbound Idle row
                // likewise belongs to the dead process and is already absent.
                let Some(store) = self.store.as_ref() else {
                    return Ok(None);
                };
                let durable_lifecycle =
                    crate::store::load_machine_lifecycle(store.as_ref(), &runtime_id)
                        .await
                        .map_err(|error| RuntimeControlPlaneError::Internal(error.to_string()))?;
                if !durable_lifecycle.as_ref().is_some_and(
                    super::session_management::machine_lifecycle_has_runtime_archive_residue,
                ) {
                    return Ok(None);
                }
                // Archive recovery must preserve the durable lifecycle
                // authority exactly long enough to drain any terminal
                // outboxes owned by its last placement. The public
                // RegisterSession command intentionally revives Stopped
                // to Idle and clears that epoch tuple; doing so here would
                // destroy the witness required for exact outbox adoption.
                // Recover the entry mechanically, without applying the
                // user-facing revival transition.
                recovered_registration_for_archive = self
                    .register_session_inner_under_registration_transaction(session_id.clone(), None)
                    .await
                    .map_err(|error| RuntimeControlPlaneError::Internal(error.to_string()))?
                    .inserted();
                self.lookup_entry(&runtime_id).await?
            }
            Err(error) => return Err(error),
        };
        if &resolved_session_id != session_id {
            return Err(RuntimeControlPlaneError::Internal(format!(
                "runtime {runtime_id} resolved to unexpected session {resolved_session_id} while archiving {session_id}"
            )));
        }
        self.reject_unregister_overlap_under_registration_transaction(&resolved_session_id)
            .await?;
        #[cfg(test)]
        self.run_control_command_after_logical_lookup_test_hook(
            ControlCommandLookupTestKind::Retire,
            &resolved_session_id,
        )
        .await;
        #[cfg(feature = "live")]
        let live_lifecycle_lease = Some(
            self.acquire_member_live_disposal_lease(&resolved_session_id)
                .await
                .map_err(|error| RuntimeControlPlaneError::Internal(error.to_string()))?,
        );
        #[cfg(not(feature = "live"))]
        let live_lifecycle_lease = None;
        let mutation_guard = self
            .lock_current_session_driver_gate(&resolved_session_id, &driver)
            .await
            .map_err(|error| RuntimeControlPlaneError::Internal(error.to_string()))?;
        let (driver, completions, wake_tx, publication_handle) = self
            .capture_archive_lease_entry_under_mutation_guard(
                &runtime_id,
                &resolved_session_id,
                &driver,
                &mutation_guard,
            )
            .await?;
        Ok(Some(super::MachineSessionArchiveLease {
            session_id: resolved_session_id,
            runtime_id,
            driver,
            completions,
            wake_tx,
            publication_handle,
            recovered_registration_for_archive,
            _registration_transaction_guard: registration_transaction_guard,
            _live_lifecycle_lease: live_lifecycle_lease,
            _mutation_guard: mutation_guard,
        }))
    }

    /// Capture the exact runtime entry before a direct SessionService turn.
    /// The identity carries no lock; the session layer separately owns its
    /// stable turn-finalization boundary while the actor executes.
    pub async fn capture_service_turn_identity(
        &self,
        session_id: &SessionId,
    ) -> Result<super::MachineServiceTurnIdentity, RuntimeDriverError> {
        let driver = {
            let sessions = self.sessions.read().await;
            let entry = sessions
                .get(session_id)
                .ok_or(RuntimeDriverError::NotReady {
                    state: RuntimeState::Destroyed,
                })?;
            if !entry.generated_service_turn_binding_open(session_id) {
                return Err(RuntimeDriverError::NotReady {
                    state: RuntimeState::Destroyed,
                });
            }
            Arc::clone(&entry.driver)
        };
        Ok(super::MachineServiceTurnIdentity {
            session_id: session_id.clone(),
            driver,
        })
    }

    /// Acquire exact mutation authority for the terminal commit of a direct
    /// SessionService turn.
    ///
    /// Callers must not hold the session recovery gate while awaiting this
    /// lease. Once acquired, they may take recovery and commit/checkpoint in
    /// the global machine-mutation -> recovery order.
    pub async fn prepare_service_turn_commit_lease(
        &self,
        turn_identity: &super::MachineServiceTurnIdentity,
    ) -> Result<super::MachineServiceTurnCommitLease, RuntimeDriverError> {
        let session_id = &turn_identity.session_id;
        let driver = Arc::clone(&turn_identity.driver);
        let mutation_guard = self
            .lock_current_session_driver_gate(session_id, &driver)
            .await?;
        let registration_open = {
            let sessions = self.sessions.read().await;
            sessions.get(session_id).is_some_and(|entry| {
                Arc::ptr_eq(&entry.driver, &driver)
                    && entry.generated_service_turn_binding_open(session_id)
            })
        };
        if !registration_open {
            return Err(RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            });
        }
        Ok(super::MachineServiceTurnCommitLease {
            session_id: session_id.clone(),
            driver,
            _mutation_guard: mutation_guard,
        })
    }

    /// Commit a direct service-turn terminal through an already-held exact
    /// mutation lease. The lease remains live so the caller can checkpoint the
    /// committed snapshot while retaining the same authority interval.
    pub async fn commit_service_turn_terminal_receipt_with_lease(
        &self,
        lease: &mut super::MachineServiceTurnCommitLease,
        session_snapshot: Vec<u8>,
    ) -> Result<(), RuntimeDriverError> {
        let still_current = {
            let sessions = self.sessions.read().await;
            sessions.get(&lease.session_id).is_some_and(|entry| {
                Arc::ptr_eq(&entry.driver, &lease.driver)
                    && entry.generated_service_turn_binding_open(&lease.session_id)
            })
        };
        if !still_current {
            return Err(RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            });
        }
        let receipt_result = {
            let mut driver = lease.driver.lock().await;
            machine_commit_service_turn_terminal_receipt(&mut driver, session_snapshot).await
        };
        if let Err(error) = receipt_result {
            return Err(self
                .classify_session_driver_rejection(&lease.session_id, error)
                .await);
        }
        Ok(())
    }

    /// Compare-and-remove the reconstructable in-memory registration inserted
    /// by archive preparation itself.
    ///
    /// This is intentionally not the generated `UnregisterSession` path: the
    /// durable runtime is already `Retired` or `Destroyed`, and archive cleanup
    /// must not rewrite that terminal truth. The exact runtime and driver
    /// identity prevent cleanup from removing a registration that predated the
    /// archive or replaced its recovered incarnation.
    async fn remove_archive_recovered_registration_exact(
        &self,
        session_id: &SessionId,
        runtime_id: &LogicalRuntimeId,
        driver: &SharedDriver,
    ) -> Result<(), RuntimeControlPlaneError> {
        let state = driver.lock().await.runtime_state();
        if !matches!(state, RuntimeState::Retired | RuntimeState::Destroyed) {
            return Err(RuntimeControlPlaneError::InvalidState { state });
        }

        let removed = {
            let mut sessions = self.sessions.write().await;
            let Some(entry) = sessions.get(session_id) else {
                // Another terminal cleanup already removed the reconstructable
                // entry. Durable truth remains untouched, so this is converged.
                return Ok(());
            };
            if &entry.runtime_id != runtime_id || !Arc::ptr_eq(&entry.driver, driver) {
                return Err(RuntimeControlPlaneError::Internal(format!(
                    "archive-recovered runtime {runtime_id} was replaced before quiescent cleanup"
                )));
            }
            if entry.wake_sender().is_some() || entry.publication_handle().is_some() {
                return Err(RuntimeControlPlaneError::Internal(format!(
                    "archive-recovered quiescent runtime {runtime_id} acquired a live attachment before cleanup"
                )));
            }
            sessions.remove(session_id)
        };
        drop(removed);
        Ok(())
    }

    /// Release a quiescent archive lease and discard only the reconstructable
    /// in-memory registration inserted by archive preparation itself.
    pub async fn release_quiescent_session_archive_lease(
        &self,
        lease: super::MachineSessionArchiveLease,
    ) -> Result<(), RuntimeControlPlaneError> {
        let super::MachineSessionArchiveLease {
            session_id,
            runtime_id,
            driver,
            completions: _,
            wake_tx,
            publication_handle,
            recovered_registration_for_archive,
            _registration_transaction_guard,
            _live_lifecycle_lease,
            _mutation_guard,
        } = lease;

        if !recovered_registration_for_archive {
            return Ok(());
        }

        if wake_tx.is_some() || publication_handle.is_some() {
            return Err(RuntimeControlPlaneError::Internal(format!(
                "archive-recovered quiescent runtime {runtime_id} acquired a live attachment before cleanup"
            )));
        }
        self.remove_archive_recovered_registration_exact(&session_id, &runtime_id, &driver)
            .await
    }

    /// Realize Retire using a previously acquired archive lease without
    /// reacquiring the per-session mutation gate.
    pub async fn retire_session_with_archive_lease(
        &self,
        lease: super::MachineSessionArchiveLease,
    ) -> Result<RetireReport, RuntimeControlPlaneError> {
        self.realize_retire_with_archive_lease(lease, None).await
    }

    /// Drain durable runless terminals while an archive lease still owns the
    /// session mutation gate. Archive calls this before its document verdict,
    /// so a prior crash after runtime terminalization cannot be hidden behind
    /// an `AlreadyArchived` document result on retry.
    pub async fn drain_session_archive_lease_terminals(
        &self,
        lease: &super::MachineSessionArchiveLease,
        archive_publication_handle: Option<
            &dyn meerkat_core::lifecycle::CoreExecutorPublicationHandle,
        >,
    ) -> Result<(), RuntimeControlPlaneError> {
        let publication_handle = lease
            .publication_handle
            .as_deref()
            .or(archive_publication_handle);
        crate::control_plane::drain_recovered_runless_runtime_terminations(
            &lease.driver,
            Some(&lease.completions),
            publication_handle,
        )
        .await
        .map_err(|error| RuntimeControlPlaneError::Internal(error.to_string()))
    }

    /// Archive-only sibling that supplies a borrowed, quiescent stored-session
    /// publisher when the restarted runtime has no attached executor. The
    /// lease-retained live publisher always wins when present.
    pub async fn retire_session_with_archive_lease_and_publication_handle(
        &self,
        lease: super::MachineSessionArchiveLease,
        publication_handle: &dyn meerkat_core::lifecycle::CoreExecutorPublicationHandle,
    ) -> Result<RetireReport, RuntimeControlPlaneError> {
        self.realize_retire_with_archive_lease(lease, Some(publication_handle))
            .await
    }

    pub async fn retire_runtime_control_plane(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<RetireReport, RuntimeControlPlaneError> {
        // Resolve only the transaction key optimistically. The authoritative
        // entry capture happens after the stable registration transaction is
        // held, so an old entry can never dispose a replacement's live state.
        let (session_id, _, _, _) = self.lookup_entry(runtime_id).await?;
        let registration_transaction_guard = self
            .lock_session_registration_transaction(&session_id)
            .await;
        let (resolved_session_id, driver, _, _) = self.lookup_entry(runtime_id).await?;
        if resolved_session_id != session_id {
            return Err(RuntimeControlPlaneError::Internal(format!(
                "runtime {runtime_id} changed session identity from {session_id} to {resolved_session_id} during retirement"
            )));
        }
        self.reject_unregister_overlap_under_registration_transaction(&resolved_session_id)
            .await?;
        #[cfg(test)]
        self.run_control_command_after_logical_lookup_test_hook(
            ControlCommandLookupTestKind::Retire,
            &resolved_session_id,
        )
        .await;
        #[cfg(feature = "live")]
        let live_lifecycle_lease = Some(
            self.acquire_member_live_disposal_lease(&session_id)
                .await
                .map_err(|error| RuntimeControlPlaneError::Internal(error.to_string()))?,
        );
        #[cfg(not(feature = "live"))]
        let live_lifecycle_lease = None;
        let mutation_guard = self
            .lock_current_session_driver_gate(&session_id, &driver)
            .await
            .map_err(|error| RuntimeControlPlaneError::Internal(error.to_string()))?;
        let (driver, completions, wake_tx, publication_handle) = self
            .capture_archive_lease_entry_under_mutation_guard(
                runtime_id,
                &resolved_session_id,
                &driver,
                &mutation_guard,
            )
            .await?;
        let lease = super::MachineSessionArchiveLease {
            session_id: resolved_session_id,
            runtime_id: runtime_id.clone(),
            driver,
            completions,
            wake_tx,
            publication_handle,
            recovered_registration_for_archive: false,
            _registration_transaction_guard: registration_transaction_guard,
            _live_lifecycle_lease: live_lifecycle_lease,
            _mutation_guard: mutation_guard,
        };
        self.realize_retire_with_archive_lease(lease, None).await
    }

    async fn realize_retire_with_archive_lease(
        &self,
        lease: super::MachineSessionArchiveLease,
        archive_publication_handle: Option<
            &dyn meerkat_core::lifecycle::CoreExecutorPublicationHandle,
        >,
    ) -> Result<RetireReport, RuntimeControlPlaneError> {
        let super::MachineSessionArchiveLease {
            session_id,
            runtime_id,
            driver,
            completions,
            wake_tx,
            publication_handle,
            recovered_registration_for_archive,
            _registration_transaction_guard,
            _live_lifecycle_lease,
            _mutation_guard,
        } = lease;
        let retained_publication_handle = publication_handle;
        let publication_handle = retained_publication_handle
            .as_deref()
            .or(archive_publication_handle);
        tracing::info!(
            runtime_id = %runtime_id,
            "MeerkatMachine::retire_runtime_control_plane start"
        );

        let staged_dsl = self
            .stage_session_dsl_transition(
                &session_id,
                crate::meerkat_machine::dsl::MeerkatMachineInput::Retire {
                    session_id: crate::meerkat_machine::dsl::SessionId::from_domain(&session_id),
                },
                "Retire",
            )
            .await
            .map_err(RuntimeControlPlaneError::Internal)?;

        let mut drv = driver.lock().await;
        let mut report = match Box::pin(machine_retire(&mut drv)).await {
            Ok(report) => report,
            Err(err) => {
                drop(drv);
                let restored = self
                    .restore_session_dsl_state_if_current(
                        &session_id,
                        staged_dsl.committed_snapshot.clone(),
                        staged_dsl.previous_snapshot.clone(),
                    )
                    .await;
                driver
                    .lock()
                    .await
                    .sync_control_projection_from_dsl_authority();
                let detail = if restored {
                    err.to_string()
                } else {
                    format!(
                        "{err}; archive retire realization failed to restore the staged runtime authority"
                    )
                };
                return Err(RuntimeControlPlaneError::Internal(detail));
            }
        };
        drop(drv);

        let mut commit_error = None;
        if let Err(reason) = self
            .commit_session_dsl_transition_preserving_committed_state(
                &session_id,
                staged_dsl,
                "Retire",
            )
            .await
        {
            driver
                .lock()
                .await
                .sync_control_projection_from_dsl_authority();
            commit_error = Some(reason);
        }

        crate::control_plane::drain_recovered_runless_runtime_terminations(
            &driver,
            Some(&completions),
            publication_handle,
        )
        .await
        .map_err(|error| RuntimeControlPlaneError::Internal(error.to_string()))?;

        if report.inputs_pending_drain > 0 {
            if let Some(ref tx) = wake_tx
                && tx.send(()).await.is_ok()
            {
                if let Some(reason) = commit_error {
                    return Err(RuntimeControlPlaneError::Internal(reason));
                }
                return Ok(report);
            }

            let reason = "retired without runtime loop";
            let (abandoned, completion_input_ids, candidate_owner_input_id) = {
                let mut drv = driver.lock().await;
                let completion_input_ids = drv.as_driver().active_input_ids();
                let prepared = drv
                    .prepare_runless_runtime_terminated_interaction_outboxes(
                        &completion_input_ids,
                        reason.to_string(),
                    )
                    .map_err(|error| RuntimeControlPlaneError::Internal(error.to_string()))?;
                let abandoned = match drv
                    .abandon_pending_inputs(crate::input_state::InputAbandonReason::Retired)
                    .await
                {
                    Ok(abandoned) => abandoned,
                    Err(error) => {
                        drv.rollback_prepared_runless_interaction_terminal_outboxes(prepared);
                        return Err(RuntimeControlPlaneError::Internal(error.to_string()));
                    }
                };
                let candidate_owner_input_id =
                    crate::meerkat_machine::driver::DriverEntry::commit_prepared_runless_interaction_terminal_outboxes(prepared);
                (abandoned, completion_input_ids, candidate_owner_input_id)
            };
            crate::control_plane::publish_and_resolve_runless_runtime_termination(
                &driver,
                Some(&completions),
                publication_handle,
                &completion_input_ids,
                candidate_owner_input_id.as_ref(),
                reason,
            )
            .await
            .map_err(|error| RuntimeControlPlaneError::Internal(error.to_string()))?;
            report.inputs_abandoned += abandoned;
            report.inputs_pending_drain = 0;
        }
        if let Some(reason) = commit_error {
            return Err(RuntimeControlPlaneError::Internal(reason));
        }
        if recovered_registration_for_archive {
            self.remove_archive_recovered_registration_exact(&session_id, &runtime_id, &driver)
                .await?;
        }
        Ok(report)
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
        self.retire_runtime_control_plane(runtime_id).await
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

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;

    /// Row #45 gate: control-plane not-found must map to the dedicated
    /// `RuntimeDriverError::NotFound` carrying the runtime id, NOT to
    /// `NotReady { state: Destroyed }` (which conflates never-existed/absent
    /// with a torn-down lifecycle).
    #[test]
    fn control_plane_not_found_maps_to_driver_not_found() {
        let runtime_id = LogicalRuntimeId("missing-runtime".to_string());
        let mapped = MeerkatMachine::driver_error_from_control_plane_error(
            RuntimeControlPlaneError::NotFound(runtime_id.clone()),
        );

        match mapped {
            RuntimeDriverError::NotFound {
                runtime_id: mapped_id,
            } => assert_eq!(mapped_id, runtime_id),
            other => panic!(
                "expected RuntimeDriverError::NotFound, got {other:?} (must not collapse absence into NotReady/Destroyed)"
            ),
        }
    }

    /// Guard the negative half explicitly: the not-found mapping must never
    /// surface as `NotReady { state: Destroyed }`.
    #[test]
    fn control_plane_not_found_is_not_destroyed_not_ready() {
        let mapped = MeerkatMachine::driver_error_from_control_plane_error(
            RuntimeControlPlaneError::NotFound(LogicalRuntimeId("missing-runtime".to_string())),
        );

        assert!(
            !matches!(
                mapped,
                RuntimeDriverError::NotReady {
                    state: RuntimeState::Destroyed
                }
            ),
            "not-found must not be laundered into NotReady{{Destroyed}}"
        );
    }
}
