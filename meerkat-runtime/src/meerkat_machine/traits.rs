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

    pub async fn retire_runtime_control_plane(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<RetireReport, RuntimeControlPlaneError> {
        tracing::info!(
            runtime_id = %runtime_id,
            "MeerkatMachine::retire_runtime_control_plane start"
        );
        let (session_id, driver, completions, wake_tx) = self.lookup_entry(runtime_id).await?;
        let gate = self.session_mutation_gate(&session_id).await;
        // Bounded acquisition (defense in depth for the stop-under-gate
        // deadlock class): the gate is only ever held for short critical
        // sections, so a long wait means another task is parked while
        // holding it — a bug in THAT task. Fail with a typed busy error so
        // callers (e.g. a single-task mob actor) fast-fail instead of
        // wedging every subsequent command behind an unbounded lock wait.
        let _gate_guard = match gate {
            Some(ref gate) => Some(
                crate::tokio::time::timeout(
                    std::time::Duration::from_secs(30),
                    gate.lock(),
                )
                .await
                .map_err(|_| {
                    RuntimeControlPlaneError::Internal(format!(
                        "retire for session {session_id} timed out acquiring the session                          mutation gate after 30s; the gate holder is likely deadlocked                          (stop-under-gate class) — retire can be retried"
                    ))
                })?,
            ),
            None => None,
        };

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
                drv.sync_control_projection_from_dsl_authority();
                return Err(RuntimeControlPlaneError::Internal(err.to_string()));
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

        if report.inputs_pending_drain > 0 {
            if let Some(ref tx) = wake_tx
                && tx.send(()).await.is_ok()
            {
                if let Some(reason) = commit_error {
                    return Err(RuntimeControlPlaneError::Internal(reason));
                }
                return Ok(report);
            }

            let mut drv = driver.lock().await;
            let abandoned = drv
                .abandon_pending_inputs(crate::input_state::InputAbandonReason::Retired)
                .await
                .map_err(|err| RuntimeControlPlaneError::Internal(err.to_string()))?;
            drop(drv);
            let result_class =
                crate::meerkat_machine::driver::machine_resolve_runtime_terminated_completion_result(
                    &driver,
                )
                .await
                .map_err(|err| RuntimeControlPlaneError::Internal(err.to_string()))?;
            let mut comp = completions.lock().await;
            comp.resolve_all_runtime_terminated("retired without runtime loop", result_class);
            report.inputs_abandoned += abandoned;
            report.inputs_pending_drain = 0;
        }
        if let Some(reason) = commit_error {
            return Err(RuntimeControlPlaneError::Internal(reason));
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
