use super::*;
use crate::input_state::StoredInputState;
use crate::store::{RuntimeSessionAuthority, RuntimeSessionPersistenceProfile};

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

    async fn input_terminal_completion(
        &self,
        session_id: &SessionId,
        input_id: &InputId,
    ) -> Result<Option<crate::completion::CompletionOutcome>, RuntimeDriverError> {
        let driver = {
            let sessions = self.sessions.read().await;
            sessions.get(session_id).map(|entry| entry.driver.clone())
        };
        if let Some(driver) = driver {
            return driver
                .lock()
                .await
                .exact_input_terminal_completion_outcome(input_id);
        }

        let Some(store) = self.store.as_ref() else {
            return Err(RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            });
        };
        let runtime_id = Self::logical_runtime_id(session_id);
        let load = |error: crate::store::RuntimeStoreError| match error {
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
        };
        let mut target_rows = store
            .load_input_states_by_ids(&runtime_id, std::slice::from_ref(input_id))
            .await
            .map_err(load)?;
        let Some(target) = target_rows.pop().ok_or_else(|| {
            RuntimeDriverError::Internal(
                "exact terminal completion target read returned the wrong cardinality".to_string(),
            )
        })?
        else {
            let lifecycle = store
                .load_machine_lifecycle_record(&runtime_id)
                .await
                .map_err(load)?;
            return if lifecycle.is_some() {
                Ok(None)
            } else {
                Err(RuntimeDriverError::NotFound {
                    runtime_id: runtime_id.clone(),
                })
            };
        };
        let Some(target_completion) = target.state.terminal_completion.as_ref() else {
            return crate::input_state::input_terminal_completion_outcome(&[target], input_id)
                .map_err(|error| match error {
                    error @ crate::input_state::InputTerminalCompletionReadError::MigratedReceiptUnavailable => {
                        RuntimeDriverError::RecoveryRepairBlocked {
                            evidence_digest: None,
                            reason: error.to_string(),
                        }
                    }
                    crate::input_state::InputTerminalCompletionReadError::Corrupt(reason) => {
                        RuntimeDriverError::RecoveryCorruption { reason }
                    }
                });
        };
        let owner_input_id = target_completion.owner_input_id.clone();
        let owner = if owner_input_id == *input_id {
            target
        } else {
            let mut owner_rows = store
                .load_input_states_by_ids(&runtime_id, std::slice::from_ref(&owner_input_id))
                .await
                .map_err(load)?;
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
            .load_input_states_by_ids(&runtime_id, &recipient_ids)
            .await
            .map_err(load)?;
        if recipient_rows.len() != recipient_ids.len() {
            return Err(RuntimeDriverError::Internal(
                "exact terminal completion batch read returned the wrong cardinality".to_string(),
            ));
        }
        let witnesses = recipient_rows
            .into_iter()
            .zip(recipient_ids)
            .map(|(stored, recipient_id)| {
                stored.ok_or_else(|| RuntimeDriverError::RecoveryCorruption {
                    reason: format!(
                        "terminal completion durable batch lost recipient row {recipient_id}"
                    ),
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        crate::input_state::input_terminal_completion_outcome(&witnesses, input_id).map_err(
            |error| match error {
                error @ crate::input_state::InputTerminalCompletionReadError::MigratedReceiptUnavailable => {
                    RuntimeDriverError::RecoveryRepairBlocked {
                        evidence_digest: None,
                        reason: error.to_string(),
                    }
                }
                crate::input_state::InputTerminalCompletionReadError::Corrupt(reason) => {
                    RuntimeDriverError::RecoveryCorruption { reason }
                }
            },
        )
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

    async fn durable_input_state_by_idempotency_key(
        &self,
        session_id: &SessionId,
        idempotency_key: &str,
    ) -> Result<Option<StoredInputState>, RuntimeDriverError> {
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::DurableInputStateByIdempotencyKey {
                    session_id: session_id.clone(),
                    idempotency_key: idempotency_key.to_string(),
                },
            )
            .await
            .map_err(MeerkatMachine::driver_error_from_command_error)?
        {
            MeerkatMachineCommandResult::InputState(state) => Ok(state),
            other => Err(RuntimeDriverError::Internal(format!(
                "unexpected MeerkatMachineCommandResult for SessionServiceRuntimeExt::durable_input_state_by_idempotency_key: {other:?}"
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

    /// Install or join the process-owned convergence task for one exact
    /// driver incarnation. The returned start permit must be fired only after
    /// the caller has released M and every registration/member lease.
    pub(super) fn prepare_runless_terminal_publication_dispatch(
        &self,
        driver: &SharedDriver,
        completions: &SharedCompletionRegistry,
        mutation_gate: &Arc<Mutex<()>>,
        publication_handle: Arc<dyn meerkat_core::lifecycle::CoreExecutorPublicationHandle>,
    ) -> Result<
        (
            crate::tokio::sync::watch::Receiver<Option<Result<(), RuntimeDriverError>>>,
            Option<crate::tokio::sync::oneshot::Sender<()>>,
        ),
        RuntimeDriverError,
    > {
        let driver_key = Arc::as_ptr(driver) as usize;
        let mut pending = self
            .pending_runless_terminal_publications
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(existing) = pending.get_mut(&driver_key) {
            if !Arc::ptr_eq(&existing.driver, driver) {
                return Err(RuntimeDriverError::StaleAuthority {
                    reason: "runless terminal publication driver identity was reused while its prior dispatch remained pending"
                        .to_string(),
                });
            }
            if !Arc::ptr_eq(&existing.mutation_gate, mutation_gate) {
                return Err(RuntimeDriverError::StaleAuthority {
                    reason: "runless terminal publication driver changed its mutation gate while a prior dispatch remained pending"
                        .to_string(),
                });
            }
            existing.requested_generation = existing
                .requested_generation
                .checked_add(1)
                .ok_or_else(|| {
                    RuntimeDriverError::Internal(
                        "runless terminal publication request generation overflow".to_string(),
                    )
                })?;
            return Ok((existing.result_rx.clone(), None));
        }

        let cleanup_spawner = MachineCleanupTaskSpawner::acquire()?;
        let dispatch_id = uuid::Uuid::new_v4();
        let (result_tx, result_rx) = crate::tokio::sync::watch::channel(None);
        let (start_tx, start_rx) = crate::tokio::sync::oneshot::channel();
        pending.insert(
            driver_key,
            PendingRunlessTerminalPublicationDispatch {
                dispatch_id,
                driver: driver.clone(),
                mutation_gate: Arc::clone(mutation_gate),
                requested_generation: 1,
                result_rx: result_rx.clone(),
            },
        );
        drop(pending);

        let machine = self.clone();
        let driver = driver.clone();
        let completions = completions.clone();
        let mutation_gate = Arc::clone(mutation_gate);
        cleanup_spawner.spawn(async move {
            let result = if start_rx.await.is_err() {
                Err(RuntimeDriverError::Internal(
                    "runless terminal publication dispatch lost its post-M start permit"
                        .to_string(),
                ))
            } else {
                loop {
                    let requested_generation = {
                        let pending = machine
                            .pending_runless_terminal_publications
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        let Some(pending) = pending
                            .get(&driver_key)
                            .filter(|pending| pending.dispatch_id == dispatch_id)
                        else {
                            break Err(RuntimeDriverError::StaleAuthority {
                                reason: "runless terminal publication dispatch lost its exact process slot"
                                    .to_string(),
                            });
                        };
                        pending.requested_generation
                    };
                    let convergence = std::panic::AssertUnwindSafe(
                        crate::control_plane::converge_known_committed_runless_runtime_terminations_before(
                            &driver,
                            Some(&completions),
                            Some(publication_handle.as_ref()),
                            None,
                        ),
                    )
                    .catch_unwind()
                    .await;
                    let convergence = match convergence {
                        Ok(result) => result,
                        Err(payload) => Err(RuntimeDriverError::Internal(format!(
                            "runless terminal publication callback panicked: {}",
                            meerkat_core::panic_payload::panic_payload_detail(payload.as_ref())
                        ))),
                    };
                    if let Err(error) = convergence {
                        break Err(error);
                    }

                    // Every exact outbox commit for this driver holds this M.
                    // Reacquiring it closes the only gap between the durable
                    // scan and process-slot removal. A caller that committed a
                    // later carrier must first increment requested_generation,
                    // forcing this task to scan again before acknowledging it.
                    let gate_guard = Arc::clone(&mutation_gate).lock_owned().await;
                    let mut pending = machine
                        .pending_runless_terminal_publications
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    match pending.get(&driver_key) {
                        Some(current)
                            if current.dispatch_id == dispatch_id
                                && current.requested_generation == requested_generation =>
                        {
                            pending.remove(&driver_key);
                            result_tx.send_replace(Some(Ok(())));
                            drop(pending);
                            drop(gate_guard);
                            return;
                        }
                        Some(current) if current.dispatch_id == dispatch_id => {
                            drop(pending);
                            drop(gate_guard);
                        }
                        _ => {
                            drop(pending);
                            drop(gate_guard);
                            break Err(RuntimeDriverError::StaleAuthority {
                                reason: "runless terminal publication dispatch lost its exact process slot during final reconciliation"
                                    .to_string(),
                            });
                        }
                    }
                }
            };

            let gate_guard = Arc::clone(&mutation_gate).lock_owned().await;
            let mut pending = machine
                .pending_runless_terminal_publications
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if pending
                .get(&driver_key)
                .is_some_and(|pending| pending.dispatch_id == dispatch_id)
            {
                pending.remove(&driver_key);
            }
            result_tx.send_replace(Some(result));
            drop(pending);
            drop(gate_guard);
        });
        Ok((result_rx, Some(start_tx)))
    }

    pub(super) fn existing_runless_terminal_publication_dispatch(
        &self,
        driver: &SharedDriver,
    ) -> Option<crate::tokio::sync::watch::Receiver<Option<Result<(), RuntimeDriverError>>>> {
        let driver_key = Arc::as_ptr(driver) as usize;
        self.pending_runless_terminal_publications
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&driver_key)
            .filter(|pending| Arc::ptr_eq(&pending.driver, driver))
            .map(|pending| pending.result_rx.clone())
    }

    pub(super) fn runless_terminal_publication_dispatch_pending(
        &self,
        driver: &SharedDriver,
    ) -> bool {
        self.existing_runless_terminal_publication_dispatch(driver)
            .is_some()
    }

    pub(super) async fn await_runless_terminal_publication_dispatch(
        &self,
        runtime_id: &LogicalRuntimeId,
        mut result_rx: crate::tokio::sync::watch::Receiver<Option<Result<(), RuntimeDriverError>>>,
        deadline: Option<meerkat_core::time_compat::Instant>,
    ) -> Result<(), RuntimeDriverError> {
        loop {
            if let Some(result) = result_rx.borrow().clone() {
                return result;
            }
            let changed = match deadline {
                Some(deadline) => {
                    let remaining = deadline
                        .saturating_duration_since(meerkat_core::time_compat::Instant::now());
                    if remaining.is_zero() {
                        return Err(RuntimeDriverError::RuntimeTerminalPublicationInProgress {
                            runtime_id: runtime_id.clone(),
                        });
                    }
                    match crate::tokio::time::timeout(remaining, result_rx.changed()).await {
                        Ok(changed) => changed,
                        Err(_) => {
                            return Err(RuntimeDriverError::RuntimeTerminalPublicationInProgress {
                                runtime_id: runtime_id.clone(),
                            });
                        }
                    }
                }
                None => result_rx.changed().await,
            };
            if changed.is_err() {
                return Err(RuntimeDriverError::Internal(
                    "process-owned runless terminal publication ended without a result".to_string(),
                ));
            }
        }
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
            RuntimeControlPlaneError::RetirementInProgress { runtime_id, .. } => {
                RuntimeDriverError::RuntimeTerminalPublicationInProgress { runtime_id }
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
            bool,
            bool,
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
            entry.archive_recovered_registration,
            entry.archive_recovered_from_quiescent,
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
        self.prepare_session_archive_lease_owned(session_id).await
    }

    /// Process-owned archive preparation bounded only at the caller's
    /// acknowledgement edge. Injected RuntimeStore reads and recovery cannot
    /// be cancelled by deadline expiry, and no M is acquired by the caller
    /// while that arbitrary storage future remains pending.
    pub async fn prepare_session_archive_lease_before(
        &self,
        session_id: &SessionId,
        deadline: meerkat_core::time_compat::Instant,
    ) -> Result<Option<super::MachineSessionArchiveLease>, RuntimeControlPlaneError> {
        let runtime_id = LogicalRuntimeId::for_session(session_id);
        loop {
            if deadline <= meerkat_core::time_compat::Instant::now() {
                return Err(RuntimeControlPlaneError::RetirementInProgress {
                    runtime_id,
                    stage: "archive_lease_preparation".to_string(),
                });
            }
            let existing = self
                .pending_session_archive_lease_preparations
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(session_id)
                .map(|pending| pending.completion_rx.clone());
            if let Some(mut completion_rx) = existing {
                let remaining = deadline
                    .checked_duration_since(meerkat_core::time_compat::Instant::now())
                    .ok_or_else(|| RuntimeControlPlaneError::RetirementInProgress {
                        runtime_id: runtime_id.clone(),
                        stage: "archive_lease_preparation".to_string(),
                    })?;
                let completion = crate::tokio::time::timeout(remaining, async {
                    loop {
                        if let Some(result) = completion_rx.borrow().clone() {
                            break result;
                        }
                        completion_rx.changed().await.map_err(|error| {
                            format!("archive lease preparation leader dropped completion: {error}")
                        })?;
                    }
                })
                .await
                .map_err(|_| RuntimeControlPlaneError::RetirementInProgress {
                    runtime_id: runtime_id.clone(),
                    stage: "archive_lease_preparation".to_string(),
                })?
                .map_err(RuntimeControlPlaneError::Internal);
                completion?;
                // The leader's unique lease went to its live caller or was
                // safely dropped after caller timeout. Reissue only after the
                // exact slot has completed and been removed; repeated retries
                // never accumulate behind its registration transaction.
                continue;
            }

            let cleanup_spawner = MachineCleanupTaskSpawner::acquire()
                .map_err(|error| RuntimeControlPlaneError::Internal(error.to_string()))?;
            let preparation_id = uuid::Uuid::new_v4();
            let (completion_tx, completion_rx) = crate::tokio::sync::watch::channel(None);
            let (result_tx, mut result_rx) = crate::tokio::sync::oneshot::channel();
            {
                let mut pending = self
                    .pending_session_archive_lease_preparations
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if pending.contains_key(session_id) {
                    continue;
                }
                pending.insert(
                    session_id.clone(),
                    super::PendingSessionArchiveLeasePreparation {
                        preparation_id,
                        completion_rx,
                    },
                );
            }
            let machine = self.clone();
            let owned_session_id = session_id.clone();
            cleanup_spawner.spawn(async move {
                let result = std::panic::AssertUnwindSafe(
                    machine.prepare_session_archive_lease_owned(&owned_session_id),
                )
                .catch_unwind()
                .await
                .unwrap_or_else(|payload| {
                    Err(RuntimeControlPlaneError::Internal(format!(
                        "archive lease preparation panicked: {}",
                        meerkat_core::panic_payload::panic_payload_detail(payload.as_ref())
                    )))
                });
                completion_tx.send_replace(Some(
                    result
                        .as_ref()
                        .map(|_| ())
                        .map_err(std::string::ToString::to_string),
                ));
                {
                    let mut pending = machine
                        .pending_session_archive_lease_preparations
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if pending
                        .get(&owned_session_id)
                        .is_some_and(|pending| pending.preparation_id == preparation_id)
                    {
                        pending.remove(&owned_session_id);
                    }
                }
                // A timed-out caller loses only its acknowledgement. Dropping
                // the undelivered completed lease releases in-process guards;
                // durable runtime/outbox authority remains canonical.
                let _ = result_tx.send(result);
            });
            let remaining = deadline
                .checked_duration_since(meerkat_core::time_compat::Instant::now())
                .ok_or_else(|| RuntimeControlPlaneError::RetirementInProgress {
                    runtime_id: runtime_id.clone(),
                    stage: "archive_lease_preparation".to_string(),
                })?;
            return match crate::tokio::time::timeout(remaining, &mut result_rx).await {
                Ok(Ok(result)) => result,
                Ok(Err(error)) => Err(RuntimeControlPlaneError::Internal(format!(
                    "archive lease preparation task ended without an outcome: {error}"
                ))),
                Err(_) => Err(RuntimeControlPlaneError::RetirementInProgress {
                    runtime_id,
                    stage: "archive_lease_preparation".to_string(),
                }),
            };
        }
    }

    async fn prepare_session_archive_lease_owned(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<super::MachineSessionArchiveLease>, RuntimeControlPlaneError> {
        let runtime_id = LogicalRuntimeId::for_session(session_id);
        // Durable observation and recovery invoke the injected RuntimeStore,
        // so they must complete before T is acquired. The exact preparation
        // single-flight is already published for this session. After recovery
        // we acquire T, recheck the map, and either publish this exact candidate
        // or discard it in favor of the concurrent registration that won T.
        // No arbitrary store callback can therefore retain T indefinitely.
        let (registration_transaction_guard, entry_parts) = loop {
            let prepared_registration = match self.lookup_entry(&runtime_id).await {
                Ok(_) => None,
                Err(RuntimeControlPlaneError::NotFound(_)) => {
                    // A process restart can leave unfinished runtime realization
                    // without a live session entry. Runtime lifecycle residue
                    // requires recovery so archive can finish process cleanup. A
                    // store-issued session-boundary authority independently
                    // requires recovery so archive can establish the singular
                    // Retired lifecycle terminal even when the old process left a
                    // clean Idle row. This is a bounded authority-row read, never
                    // a Session body parse.
                    let Some(store) = self.store.as_ref() else {
                        return Ok(None);
                    };
                    let durable_lifecycle =
                        crate::store::load_machine_lifecycle(store.as_ref(), &runtime_id)
                            .await
                            .map_err(|error| {
                                RuntimeControlPlaneError::Internal(error.to_string())
                            })?;
                    let lifecycle_requires_archive = durable_lifecycle.as_ref().is_some_and(
                        super::session_management::machine_lifecycle_has_runtime_archive_residue,
                    );
                    let lifecycle_is_quiescent =
                        durable_lifecycle.as_ref().is_some_and(|lifecycle| {
                            matches!(
                                lifecycle.runtime_state(),
                                RuntimeState::Retired | RuntimeState::Destroyed
                            )
                        });
                    // Retired does not prove terminal publication converged. The
                    // durable outbox is committed before arbitrary publisher IO,
                    // and the caller may time out after archive releases its
                    // recovered registration. The store-owned pending-owner index
                    // is the canonical cold retry authority in that state.
                    let pending_terminal_authority = if lifecycle_is_quiescent {
                        !store
                            .load_pending_terminal_owner_ids_page(&runtime_id, None, 1)
                            .await
                            .map_err(|error| RuntimeControlPlaneError::Internal(error.to_string()))?
                            .is_empty()
                    } else {
                        false
                    };
                    let durable_session_authority = if lifecycle_is_quiescent {
                        None
                    } else {
                        match store.session_persistence_profile() {
                            RuntimeSessionPersistenceProfile::WholeBlobV1 => store
                                .load_whole_blob_store_authority(&runtime_id)
                                .await
                                .map(|authority| authority.map(RuntimeSessionAuthority::WholeBlob))
                                .map_err(|error| {
                                    RuntimeControlPlaneError::Internal(error.to_string())
                                })?,
                            RuntimeSessionPersistenceProfile::HeadCanonicalV1 => store
                                .load_session_boundary_authority(&runtime_id)
                                .await
                                .map_err(|error| {
                                    RuntimeControlPlaneError::Internal(error.to_string())
                                })?,
                        }
                    };
                    if let Some(authority) = durable_session_authority.as_ref()
                        && (authority.session_id() != session_id
                            || authority.profile() != store.session_persistence_profile())
                    {
                        return Err(RuntimeControlPlaneError::Internal(format!(
                            "runtime {runtime_id} returned mismatched session-boundary authority while archiving {session_id}"
                        )));
                    }
                    if !lifecycle_requires_archive
                        && durable_session_authority.is_none()
                        && !pending_terminal_authority
                    {
                        return Ok(None);
                    }
                    // Archive recovery must preserve the durable lifecycle
                    // authority exactly long enough to drain terminal outboxes
                    // and/or install the Retired terminal for the committed
                    // session body. The public RegisterSession command
                    // intentionally revives Stopped to Idle and clears that epoch
                    // tuple; doing so here would destroy the witness required for
                    // exact outbox adoption. Recover the entry mechanically,
                    // without applying the user-facing revival transition.
                    Some(
                        self.prepare_archive_session_registration(
                            session_id.clone(),
                            lifecycle_is_quiescent,
                            None,
                        )
                        .await
                        .map_err(|error| RuntimeControlPlaneError::Internal(error.to_string()))?,
                    )
                }
                Err(error) => return Err(error),
            };

            let registration_transaction_guard =
                self.lock_session_registration_transaction(session_id).await;
            match self.lookup_entry(&runtime_id).await {
                Ok(parts) => break (registration_transaction_guard, parts),
                Err(RuntimeControlPlaneError::NotFound(_)) => {
                    let Some(prepared_registration) = prepared_registration else {
                        // The optimistic live entry disappeared before T.
                        // Release T before any durable observation and retry.
                        drop(registration_transaction_guard);
                        continue;
                    };
                    self.commit_prepared_archive_session_registration_under_transaction(
                        prepared_registration,
                    )
                    .await
                    .map_err(|error| RuntimeControlPlaneError::Internal(error.to_string()))?;
                    let parts = self.lookup_entry(&runtime_id).await?;
                    break (registration_transaction_guard, parts);
                }
                Err(error) => return Err(error),
            }
        };
        let (resolved_session_id, driver, _, _) = entry_parts;
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
        let (
            driver,
            completions,
            wake_tx,
            publication_handle,
            recovered_registration_for_archive,
            recovered_from_quiescent_archive,
        ) = self
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
            recovered_from_quiescent_archive,
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
        let run_id = driver.lock().await.current_run_id().ok_or_else(|| {
            RuntimeDriverError::Internal(
                "service-turn terminal commit lease requires a machine-owned current_run_id"
                    .to_string(),
            )
        })?;
        Ok(super::MachineServiceTurnCommitLease {
            session_id: session_id.clone(),
            run_id,
            driver,
            _mutation_guard: mutation_guard,
        })
    }

    /// Commit a direct service-turn terminal through an already-held exact
    /// mutation lease. The lease remains live so the caller can publish any
    /// profile-specific downstream projection while retaining the same authority
    /// interval.
    pub async fn commit_service_turn_terminal_receipt_with_lease(
        &self,
        lease: &mut super::MachineServiceTurnCommitLease,
        session: meerkat_core::lifecycle::core_executor::BoundSessionCommit,
    ) -> Result<Option<crate::store::PreparedRuntimeSessionCommitResult>, RuntimeDriverError> {
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
            machine_commit_service_turn_terminal_receipt(&mut driver, session).await
        };
        match receipt_result {
            Ok(result) => Ok(result),
            Err(error) => Err(self
                .classify_session_driver_rejection(&lease.session_id, error)
                .await),
        }
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
        let _registration_transaction_guard =
            self.lock_session_registration_transaction(session_id).await;
        let _mutation_guard = self
            .lock_current_session_driver_gate(session_id, driver)
            .await
            .map_err(|error| RuntimeControlPlaneError::Internal(error.to_string()))?;
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
            if !entry.archive_recovered_registration {
                return Err(RuntimeControlPlaneError::Internal(format!(
                    "runtime {runtime_id} no longer carries its exact archive-recovered registration marker"
                )));
            }
            let raw_state = crate::meerkat_machine::dsl_authority::runtime_phase_from_authority(
                &entry
                    .dsl_authority
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            );
            if !matches!(raw_state, RuntimeState::Retired | RuntimeState::Destroyed) {
                return Err(RuntimeControlPlaneError::InvalidState { state: raw_state });
            }
            if entry.wake_sender().is_some() || entry.publication_handle().is_some() {
                return Err(RuntimeControlPlaneError::Internal(format!(
                    "archive-recovered quiescent runtime {runtime_id} acquired a live attachment before cleanup"
                )));
            }
            sessions.remove(session_id)
        };
        if let Some(entry) = removed.as_ref() {
            entry.post_commit_hooks.shutdown();
        }
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
            recovered_from_quiescent_archive: _,
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
        drop(_mutation_guard);
        #[cfg(feature = "live")]
        drop(_live_lifecycle_lease);
        #[cfg(not(feature = "live"))]
        let _ = _live_lifecycle_lease;
        drop(_registration_transaction_guard);
        self.remove_archive_recovered_registration_exact(&session_id, &runtime_id, &driver)
            .await
    }

    /// Realize Retire using a previously acquired archive lease without
    /// reacquiring the per-session mutation gate.
    pub async fn retire_session_with_archive_lease(
        &self,
        lease: super::MachineSessionArchiveLease,
    ) -> Result<RetireReport, RuntimeControlPlaneError> {
        self.realize_retire_with_archive_lease(lease, None, None, None)
            .await
    }

    /// Deadline-aware archive sibling. The absolute caller deadline is
    /// preserved through process-owned terminal publication.
    pub async fn retire_session_with_archive_lease_before(
        &self,
        lease: super::MachineSessionArchiveLease,
        deadline: meerkat_core::time_compat::Instant,
    ) -> Result<RetireReport, RuntimeControlPlaneError> {
        self.realize_retire_with_archive_lease(lease, None, Some(deadline), None)
            .await
    }

    pub async fn retire_session_with_archive_lease_and_post_commit_hook_before(
        &self,
        lease: super::MachineSessionArchiveLease,
        post_commit_hook: Arc<dyn super::MachineSessionArchivePostCommitHook>,
        deadline: meerkat_core::time_compat::Instant,
    ) -> Result<RetireReport, RuntimeControlPlaneError> {
        self.realize_retire_with_archive_lease(lease, None, Some(deadline), Some(post_commit_hook))
            .await
    }

    /// Archive sibling combining an owned stored-session publisher with a
    /// post-commit hook. A recovered quiescent registration has no attached
    /// executor, so its runless terminal publication needs the supplied
    /// handle while the hook still owes its terminalization under the exact
    /// retire commit. The lease-retained live publisher always wins when
    /// present.
    pub async fn retire_session_with_archive_lease_publication_handle_and_post_commit_hook_before(
        &self,
        lease: super::MachineSessionArchiveLease,
        publication_handle: Arc<dyn meerkat_core::lifecycle::CoreExecutorPublicationHandle>,
        post_commit_hook: Arc<dyn super::MachineSessionArchivePostCommitHook>,
        deadline: meerkat_core::time_compat::Instant,
    ) -> Result<RetireReport, RuntimeControlPlaneError> {
        self.realize_retire_with_archive_lease(
            lease,
            Some(publication_handle),
            Some(deadline),
            Some(post_commit_hook),
        )
        .await
    }

    /// Observe whether an archive lease owns a committed runless terminal
    /// carrier that must cross publication before the document verdict.
    ///
    /// This is a read-only observation under the lease's exact M. It never
    /// invokes a publication callback while authority is retained.
    pub async fn session_archive_lease_has_pending_terminals(
        &self,
        lease: &super::MachineSessionArchiveLease,
    ) -> Result<bool, RuntimeControlPlaneError> {
        crate::control_plane::has_committed_runless_recovery_carrier(&lease.driver)
            .await
            .map_err(|error| RuntimeControlPlaneError::Internal(error.to_string()))
    }

    /// Whether this exact archive lease's driver already has a process-owned
    /// publication dispatch. This observation never locks the driver or polls
    /// custom RuntimeStore IO, so archive can join it before scanning a carrier
    /// whose receipt CAS may currently retain the driver lock.
    pub fn session_archive_lease_has_pending_terminal_dispatch(
        &self,
        lease: &super::MachineSessionArchiveLease,
    ) -> bool {
        self.existing_runless_terminal_publication_dispatch(&lease.driver)
            .is_some()
    }

    /// Consume an archive lease and converge its already-committed terminal
    /// carrier through the process-owned single-flight after releasing M and
    /// every registration/live lease.
    ///
    /// The archive caller must restart its observation after this returns:
    /// publication deliberately releases the exact lease rather than carrying
    /// a pre-publication document verdict across arbitrary callback IO.
    pub async fn converge_session_archive_lease_terminals_before(
        &self,
        lease: super::MachineSessionArchiveLease,
        archive_publication_handle: Option<
            Arc<dyn meerkat_core::lifecycle::CoreExecutorPublicationHandle>,
        >,
        deadline: meerkat_core::time_compat::Instant,
    ) -> Result<bool, RuntimeControlPlaneError> {
        let super::MachineSessionArchiveLease {
            session_id,
            runtime_id,
            driver,
            completions,
            wake_tx: _,
            publication_handle,
            recovered_registration_for_archive,
            recovered_from_quiescent_archive,
            _registration_transaction_guard: registration_transaction_guard,
            _live_lifecycle_lease: live_lifecycle_lease,
            _mutation_guard: mutation_guard,
        } = lease;
        // Clone an existing exact slot before releasing M. Its owner cannot
        // remove the slot while this lease retains M because successful
        // compare-remove itself reacquires the same gate. This path therefore
        // never waits on the driver lock held across receipt persistence.
        let existing_dispatch = self.existing_runless_terminal_publication_dispatch(&driver);
        let (result_rx, start_tx) = match existing_dispatch {
            Some(result_rx) => (result_rx, None),
            None => {
                let publication_handle = publication_handle
                    .or(archive_publication_handle)
                    .ok_or_else(|| {
                        RuntimeControlPlaneError::Internal(format!(
                            "archive terminal carrier for {runtime_id} has no exact publication capability"
                        ))
                    })?;
                let mutation_gate_identity = self
                    .session_mutation_gate(&session_id)
                    .await
                    .ok_or_else(|| RuntimeControlPlaneError::NotFound(runtime_id.clone()))?;
                self.prepare_runless_terminal_publication_dispatch(
                    &driver,
                    &completions,
                    &mutation_gate_identity,
                    publication_handle,
                )
                .map_err(|error| RuntimeControlPlaneError::Internal(error.to_string()))?
            }
        };

        drop(mutation_guard);
        #[cfg(feature = "live")]
        drop(live_lifecycle_lease);
        #[cfg(not(feature = "live"))]
        let _ = live_lifecycle_lease;
        drop(registration_transaction_guard);
        if let Some(start_tx) = start_tx {
            let _ = start_tx.send(());
        }
        self.await_runless_terminal_publication_dispatch(&runtime_id, result_rx, Some(deadline))
            .await
            .map_err(|error| match error {
                RuntimeDriverError::RuntimeTerminalPublicationInProgress { .. } => {
                    RuntimeControlPlaneError::RetirementInProgress {
                        runtime_id: runtime_id.clone(),
                        stage: "terminal_publication".to_string(),
                    }
                }
                other => RuntimeControlPlaneError::Internal(other.to_string()),
            })?;
        if recovered_registration_for_archive {
            self.remove_archive_recovered_registration_exact(&session_id, &runtime_id, &driver)
                .await?;
        }
        Ok(recovered_from_quiescent_archive)
    }

    /// Archive-only sibling that supplies an owned, quiescent stored-session
    /// publisher when the restarted runtime has no attached executor. The
    /// lease-retained live publisher always wins when present.
    pub async fn retire_session_with_archive_lease_and_publication_handle(
        &self,
        lease: super::MachineSessionArchiveLease,
        publication_handle: Arc<dyn meerkat_core::lifecycle::CoreExecutorPublicationHandle>,
    ) -> Result<RetireReport, RuntimeControlPlaneError> {
        self.realize_retire_with_archive_lease(lease, Some(publication_handle), None, None)
            .await
    }

    /// Deadline-aware archive-only sibling for a provably stored-only
    /// publisher. The callback runs only after the archive lease releases M.
    pub async fn retire_session_with_archive_lease_and_publication_handle_before(
        &self,
        lease: super::MachineSessionArchiveLease,
        publication_handle: Arc<dyn meerkat_core::lifecycle::CoreExecutorPublicationHandle>,
        deadline: meerkat_core::time_compat::Instant,
    ) -> Result<RetireReport, RuntimeControlPlaneError> {
        self.realize_retire_with_archive_lease(
            lease,
            Some(publication_handle),
            Some(deadline),
            None,
        )
        .await
    }

    /// Retire while consuming the caller's single absolute acknowledgement
    /// deadline. The deadline is never extended or converted into a fresh
    /// relative budget inside the runtime.
    pub async fn retire_runtime_control_plane_before(
        &self,
        runtime_id: &LogicalRuntimeId,
        deadline: meerkat_core::time_compat::Instant,
    ) -> Result<RetireReport, RuntimeControlPlaneError> {
        self.retire_runtime_control_plane_inner(runtime_id, Some(deadline), None)
            .await
            .map_err(|error| match error {
                super::DirectMemberRetireError::Runtime(error) => error,
                super::DirectMemberRetireError::Stale { .. } => RuntimeControlPlaneError::Internal(
                    "ordinary retirement unexpectedly entered direct-member fencing".to_string(),
                ),
            })
    }

    pub async fn retire_runtime_control_plane(
        &self,
        runtime_id: &LogicalRuntimeId,
    ) -> Result<RetireReport, RuntimeControlPlaneError> {
        self.retire_runtime_control_plane_inner(runtime_id, None, None)
            .await
            .map_err(|error| match error {
                super::DirectMemberRetireError::Runtime(error) => error,
                super::DirectMemberRetireError::Stale { .. } => RuntimeControlPlaneError::Internal(
                    "ordinary retirement unexpectedly entered direct-member fencing".to_string(),
                ),
            })
    }

    /// Retire only while the exact V5 peer-only controller-member fence is
    /// still current. Validation and current-evidence capture occur under the
    /// stable member slot and the session registration transaction immediately
    /// before the canonical retire lease is captured.
    pub(crate) async fn retire_direct_member_runtime(
        &self,
        session_id: &SessionId,
        expected: &meerkat_contracts::wire::supervisor_bridge::BridgeDirectMemberFence,
    ) -> Result<RetireReport, super::DirectMemberRetireError> {
        if expected.member_session_id != session_id.to_string() {
            return Err(super::DirectMemberRetireError::Stale { current: None });
        }
        let runtime_id = MeerkatMachine::logical_runtime_id(session_id);
        self.retire_runtime_control_plane_inner(&runtime_id, None, Some(expected))
            .await
    }

    async fn retire_runtime_control_plane_inner(
        &self,
        runtime_id: &LogicalRuntimeId,
        deadline: Option<meerkat_core::time_compat::Instant>,
        expected_direct_member: Option<
            &meerkat_contracts::wire::supervisor_bridge::BridgeDirectMemberFence,
        >,
    ) -> Result<RetireReport, super::DirectMemberRetireError> {
        // Resolve only the transaction key optimistically. The authoritative
        // entry capture happens after the stable registration transaction is
        // held, so an old entry can never dispose a replacement's live state.
        let (session_id, _, _, _) = self.lookup_entry(runtime_id).await?;
        let direct_member_slot =
            expected_direct_member.map(|_| self.member_incarnation_slot(&session_id));
        let direct_member_slot_guard = match direct_member_slot.as_ref() {
            Some(slot) => Some(Arc::clone(&slot.gate).lock_owned().await),
            None => None,
        };
        let registration_transaction_guard = self
            .lock_session_registration_transaction(&session_id)
            .await;
        let (resolved_session_id, driver, _, _) = self.lookup_entry(runtime_id).await?;
        if resolved_session_id != session_id {
            return Err(RuntimeControlPlaneError::Internal(format!(
                "runtime {runtime_id} changed session identity from {session_id} to {resolved_session_id} during retirement"
            )).into());
        }
        if let (Some(expected), Some(slot)) = (expected_direct_member, direct_member_slot.as_ref())
        {
            let (runtime_epoch_id, session_mutation_gate) = {
                let sessions = self.sessions.read().await;
                let Some(entry) = sessions.get(&resolved_session_id) else {
                    return Err(super::DirectMemberRetireError::Stale { current: None });
                };
                (entry.epoch_id.clone(), Arc::clone(&entry.mutation_gate))
            };
            let current = {
                let state = slot
                    .state
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                match &*state {
                    MemberResidencyState::PeerOnly {
                        direct_member: Some(registration),
                    } if registration.runtime_epoch_id == runtime_epoch_id
                        && Arc::ptr_eq(
                            &registration.session_mutation_gate,
                            &session_mutation_gate,
                        ) =>
                    {
                        Some(registration.fence.clone())
                    }
                    MemberResidencyState::PeerOnly { .. }
                    | MemberResidencyState::VacantPlaced
                    | MemberResidencyState::Placed(_) => None,
                }
            };
            if current.as_ref() != Some(expected) {
                return Err(super::DirectMemberRetireError::Stale {
                    current: current.map(|fence| fence.evidence()),
                });
            }
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
        let (driver, completions, wake_tx, publication_handle, _, _) = self
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
            recovered_from_quiescent_archive: false,
            _registration_transaction_guard: registration_transaction_guard,
            _live_lifecycle_lease: live_lifecycle_lease,
            _mutation_guard: mutation_guard,
        };
        let result = self
            .realize_retire_with_archive_lease(lease, None, deadline, None)
            .await;
        drop(direct_member_slot_guard);
        result.map_err(super::DirectMemberRetireError::Runtime)
    }

    async fn realize_retire_with_archive_lease(
        &self,
        lease: super::MachineSessionArchiveLease,
        archive_publication_handle: Option<
            Arc<dyn meerkat_core::lifecycle::CoreExecutorPublicationHandle>,
        >,
        deadline: Option<meerkat_core::time_compat::Instant>,
        post_commit_hook: Option<Arc<dyn super::MachineSessionArchivePostCommitHook>>,
    ) -> Result<RetireReport, RuntimeControlPlaneError> {
        let super::MachineSessionArchiveLease {
            session_id,
            runtime_id,
            driver,
            completions,
            wake_tx,
            publication_handle,
            recovered_registration_for_archive,
            recovered_from_quiescent_archive: _,
            _registration_transaction_guard: registration_transaction_guard,
            _live_lifecycle_lease: live_lifecycle_lease,
            _mutation_guard: mutation_guard,
        } = lease;
        let retained_publication_handle = publication_handle.or(archive_publication_handle);
        let mutation_gate_identity = self
            .session_mutation_gate(&session_id)
            .await
            .ok_or_else(|| RuntimeControlPlaneError::NotFound(runtime_id.clone()))?;
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

        let mut abandoned_batch = None;
        if report.inputs_pending_drain > 0 {
            let woke_runtime = if let Some(ref tx) = wake_tx {
                tx.send(()).await.is_ok()
            } else {
                false
            };
            if !woke_runtime {
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
                    let candidate_owner_input_id = crate::meerkat_machine::driver::DriverEntry::commit_prepared_runless_interaction_terminal_outboxes(prepared);
                    (abandoned, completion_input_ids, candidate_owner_input_id)
                };
                report.inputs_abandoned += abandoned;
                report.inputs_pending_drain = 0;
                abandoned_batch = Some((completion_input_ids, candidate_owner_input_id, reason));
            }
        }

        let dispatch = match retained_publication_handle {
            Some(publication_handle) => {
                let (result_rx, start_tx) = self
                    .prepare_runless_terminal_publication_dispatch(
                        &driver,
                        &completions,
                        &mutation_gate_identity,
                        publication_handle,
                    )
                    .map_err(|error| RuntimeControlPlaneError::Internal(error.to_string()))?;
                Some((result_rx, start_tx))
            }
            None => None,
        };

        if let Some(reason) = commit_error {
            return Err(RuntimeControlPlaneError::Internal(reason));
        }
        if let Some(hook) = post_commit_hook {
            hook.after_runtime_retire_commit().await?;
        }

        drop(mutation_guard);
        #[cfg(feature = "live")]
        drop(live_lifecycle_lease);
        #[cfg(not(feature = "live"))]
        let _ = live_lifecycle_lease;
        drop(registration_transaction_guard);
        if recovered_registration_for_archive {
            self.remove_archive_recovered_registration_exact(&session_id, &runtime_id, &driver)
                .await?;
        }

        if let Some((result_rx, start_tx)) = dispatch {
            if let Some(start_tx) = start_tx {
                let _ = start_tx.send(());
            }
            self.await_runless_terminal_publication_dispatch(&runtime_id, result_rx, deadline)
                .await
                .map_err(|error| match error {
                    RuntimeDriverError::RuntimeTerminalPublicationInProgress { .. } => {
                        RuntimeControlPlaneError::RetirementInProgress {
                            runtime_id: runtime_id.clone(),
                            stage: "terminal_publication".to_string(),
                        }
                    }
                    other => RuntimeControlPlaneError::Internal(other.to_string()),
                })?;
        } else {
            crate::control_plane::converge_known_committed_runless_runtime_terminations_before(
                &driver,
                Some(&completions),
                None,
                deadline,
            )
            .await
            .map_err(|error| match error {
                RuntimeDriverError::RuntimeTerminalPublicationInProgress { .. } => {
                    RuntimeControlPlaneError::RetirementInProgress {
                        runtime_id: runtime_id.clone(),
                        stage: "terminal_publication".to_string(),
                    }
                }
                other => RuntimeControlPlaneError::Internal(other.to_string()),
            })?;
        }

        if let Some((completion_input_ids, candidate_owner_input_id, reason)) = abandoned_batch
            && candidate_owner_input_id.is_none()
        {
            crate::control_plane::publish_and_resolve_runless_runtime_termination_before(
                &driver,
                Some(&completions),
                None,
                &completion_input_ids,
                None,
                reason,
                deadline,
            )
            .await
            .map_err(|error| RuntimeControlPlaneError::Internal(error.to_string()))?;
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
    use crate::driver::ephemeral::EphemeralRuntimeDriver;
    use crate::input_state::InteractionTerminalOutboxPhase;
    use crate::meerkat_machine::driver::DriverEntry;
    use crate::traits::RuntimeDriver as _;
    use meerkat_core::lifecycle::core_executor::{
        CoreExecutorError, CoreInteractionTerminalPublicationReceipt,
    };
    use meerkat_core::types::ContentInput;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex as StdMutex};

    struct GatedDispatchPublisher {
        calls: AtomicUsize,
        batches: StdMutex<Vec<Vec<meerkat_core::event::AgentEvent>>>,
        first_entered: Arc<crate::tokio::sync::Notify>,
        release_first: Arc<crate::tokio::sync::Notify>,
        first_returning: Arc<crate::tokio::sync::Notify>,
    }

    #[async_trait::async_trait]
    impl meerkat_core::lifecycle::CoreExecutorPublicationHandle for GatedDispatchPublisher {
        async fn publish_interaction_terminals(
            &self,
            events: &[meerkat_core::event::AgentEvent],
        ) -> Result<Vec<CoreInteractionTerminalPublicationReceipt>, CoreExecutorError> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            if call == 1 {
                self.first_entered.notify_one();
                self.release_first.notified().await;
                self.first_returning.notify_one();
            }
            self.batches
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(events.to_vec());
            events
                .iter()
                .enumerate()
                .map(|(index, event)| {
                    CoreInteractionTerminalPublicationReceipt::try_new(
                        event,
                        call as u64 * 100 + index as u64,
                    )
                })
                .collect()
        }
    }

    struct PanicOnceDispatchPublisher {
        calls: AtomicUsize,
    }

    struct CountingDispatchPublisher {
        calls: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl meerkat_core::lifecycle::CoreExecutorPublicationHandle for CountingDispatchPublisher {
        async fn publish_interaction_terminals(
            &self,
            events: &[meerkat_core::event::AgentEvent],
        ) -> Result<Vec<CoreInteractionTerminalPublicationReceipt>, CoreExecutorError> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            events
                .iter()
                .enumerate()
                .map(|(index, event)| {
                    CoreInteractionTerminalPublicationReceipt::try_new(
                        event,
                        call as u64 * 100 + index as u64,
                    )
                })
                .collect()
        }
    }

    struct PublicationHandleExecutor {
        publication_handle: Arc<dyn meerkat_core::lifecycle::CoreExecutorPublicationHandle>,
    }

    struct NoPublicationHandleExecutor;

    #[async_trait::async_trait]
    impl meerkat_core::lifecycle::core_executor::CoreExecutor for NoPublicationHandleExecutor {
        async fn apply(
            &mut self,
            _run_id: RunId,
            _primitive: meerkat_core::lifecycle::run_primitive::RunPrimitive,
        ) -> Result<meerkat_core::lifecycle::core_executor::CoreApplyOutput, CoreExecutorError>
        {
            Err(CoreExecutorError::Internal(
                "stored-only publication fixture received work".to_string(),
            ))
        }

        async fn cancel_after_boundary(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }

        async fn stop_runtime_executor(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl meerkat_core::lifecycle::core_executor::CoreExecutor for PublicationHandleExecutor {
        fn publication_handle(
            &self,
        ) -> Option<Arc<dyn meerkat_core::lifecycle::CoreExecutorPublicationHandle>> {
            Some(Arc::clone(&self.publication_handle))
        }

        async fn apply(
            &mut self,
            _run_id: RunId,
            _primitive: meerkat_core::lifecycle::run_primitive::RunPrimitive,
        ) -> Result<meerkat_core::lifecycle::core_executor::CoreApplyOutput, CoreExecutorError>
        {
            Err(CoreExecutorError::Internal(
                "publication-handle recovery fixture received work".to_string(),
            ))
        }

        async fn cancel_after_boundary(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }

        async fn stop_runtime_executor(
            &mut self,
            _reason: String,
        ) -> Result<(), CoreExecutorError> {
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl meerkat_core::lifecycle::CoreExecutorPublicationHandle for PanicOnceDispatchPublisher {
        async fn publish_interaction_terminals(
            &self,
            events: &[meerkat_core::event::AgentEvent],
        ) -> Result<Vec<CoreInteractionTerminalPublicationReceipt>, CoreExecutorError> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            assert_ne!(call, 1, "synthetic public publication callback panic");
            events
                .iter()
                .enumerate()
                .map(|(index, event)| {
                    CoreInteractionTerminalPublicationReceipt::try_new(
                        event,
                        call as u64 * 100 + index as u64,
                    )
                })
                .collect()
        }
    }

    fn seed_dispatch_attached_authority(
        driver: &mut DriverEntry,
        session_id: &SessionId,
        runtime_id: &LogicalRuntimeId,
    ) {
        let epoch_id =
            crate::meerkat_machine::dsl::RuntimeEpochId::from("dispatch-test-epoch".to_string());
        let mut authority = crate::meerkat_machine::dsl_authority::new_registered_authority_id(
            crate::meerkat_machine::dsl::SessionId::from_domain(session_id),
            epoch_id.clone(),
        )
        .expect("register dispatch test authority");
        crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(
            &mut authority,
            crate::meerkat_machine::dsl::MeerkatMachineInput::PrepareBindings {
                agent_runtime_id: crate::meerkat_machine::dsl::AgentRuntimeId::from(
                    runtime_id.to_string(),
                ),
                fence_token: crate::meerkat_machine::dsl::FenceToken::from(31),
                generation: Some(crate::meerkat_machine::dsl::Generation::from(7)),
                runtime_epoch_id: Some(epoch_id),
                session_id: crate::meerkat_machine::dsl::SessionId::from_domain(session_id),
            },
        )
        .expect("attach dispatch test authority");
        *driver
            .shared_dsl_authority()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = authority;
        driver.set_control_projection(crate::RuntimeState::Attached, None, None);
    }

    async fn dispatch_test_driver(
        runtime_id: &LogicalRuntimeId,
        session_id: &SessionId,
        labels: &[&str],
    ) -> (SharedDriver, Vec<InputId>) {
        let mut driver = DriverEntry::Ephemeral(EphemeralRuntimeDriver::new(runtime_id.clone()));
        seed_dispatch_attached_authority(&mut driver, session_id, runtime_id);
        let mut input_ids = Vec::with_capacity(labels.len());
        for label in labels {
            let interaction_uuid = meerkat_core::time_compat::new_uuid_v7();
            let input = crate::mob_adapter::create_tracked_flow_step_input(
                label,
                ContentInput::Text((*label).to_string()),
                "dispatch-liveness-flow",
                None,
                &interaction_uuid.to_string(),
            )
            .expect("construct directed dispatch input");
            input_ids.push(input.id().clone());
            assert!(
                driver
                    .as_driver_mut()
                    .accept_input(input)
                    .await
                    .expect("accept directed dispatch input")
                    .is_accepted()
            );
        }
        {
            let authority_handle = driver.shared_dsl_authority();
            let mut authority = authority_handle
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(
                &mut *authority,
                crate::meerkat_machine::dsl::MeerkatMachineInput::Retire {
                    session_id: crate::meerkat_machine::dsl::SessionId::from_domain(session_id),
                },
            )
            .expect("retire dispatch test authority after input admission");
        }
        driver.set_control_projection(crate::RuntimeState::Retired, None, None);
        (Arc::new(crate::tokio::sync::Mutex::new(driver)), input_ids)
    }

    async fn commit_dispatch_outbox(
        driver: &SharedDriver,
        input_id: &InputId,
        reason: &str,
        abandon_active_inputs: bool,
    ) {
        let mut driver = driver.lock().await;
        let prepared = driver
            .prepare_runless_runtime_terminated_interaction_outboxes(
                std::slice::from_ref(input_id),
                reason.to_string(),
            )
            .expect("prepare exact dispatch outbox");
        if abandon_active_inputs {
            driver
                .abandon_pending_inputs(crate::input_state::InputAbandonReason::Retired)
                .await
                .expect("abandon dispatch inputs before committing terminal outbox");
        }
        assert_eq!(
            DriverEntry::commit_prepared_runless_interaction_terminal_outboxes(prepared),
            Some(input_id.clone())
        );
    }

    async fn seed_durable_attached_archive_authority(
        store: &crate::store::InMemoryRuntimeStore,
        session_id: &SessionId,
    ) {
        let runtime_id = LogicalRuntimeId::for_session(session_id);
        store
            .commit_machine_lifecycle(
                &runtime_id,
                crate::store::MachineLifecycleCommit::new_with_binding(
                    RuntimeState::Attached,
                    crate::store::MachineLifecycleBindingFacts::new(
                        Some(runtime_id.0.clone()),
                        Some(1),
                        Some(1),
                        Some("archive-fixture-epoch".to_string()),
                    ),
                    crate::store::SupervisorAuthoritySnapshot::UnboundNoReceipt,
                ),
                &[],
            )
            .await
            .expect("seed durable attached archive authority");
    }

    fn assert_dispatch_outboxes_published(driver: &DriverEntry, expected: usize) {
        let published = driver
            .as_driver()
            .stored_input_states_snapshot()
            .expect("snapshot dispatch test outboxes")
            .into_iter()
            .filter_map(|stored| stored.state.interaction_terminal_outbox)
            .filter(|outbox| {
                matches!(
                    outbox.phase,
                    InteractionTerminalOutboxPhase::Published { .. }
                )
            })
            .count();
        assert_eq!(published, expected);
    }

    #[tokio::test]
    async fn runless_publication_generation_covers_commit_before_slot_removal() {
        let machine = MeerkatMachine::ephemeral();
        let runtime_id = LogicalRuntimeId::new("runless-publication-high-water");
        let session_id = SessionId::new();
        let (driver, input_ids) =
            dispatch_test_driver(&runtime_id, &session_id, &["first", "second"]).await;
        let completions = Arc::new(crate::tokio::sync::Mutex::new(
            crate::completion::CompletionRegistry::new(),
        ));
        let mutation_gate = Arc::new(crate::tokio::sync::Mutex::new(()));
        let first_entered = Arc::new(crate::tokio::sync::Notify::new());
        let release_first = Arc::new(crate::tokio::sync::Notify::new());
        let first_returning = Arc::new(crate::tokio::sync::Notify::new());
        let publisher = Arc::new(GatedDispatchPublisher {
            calls: AtomicUsize::new(0),
            batches: StdMutex::new(Vec::new()),
            first_entered: Arc::clone(&first_entered),
            release_first: Arc::clone(&release_first),
            first_returning: Arc::clone(&first_returning),
        });

        let first_guard = Arc::clone(&mutation_gate).lock_owned().await;
        commit_dispatch_outbox(&driver, &input_ids[0], "first generation", true).await;
        let (first_result, start) = machine
            .prepare_runless_terminal_publication_dispatch(
                &driver,
                &completions,
                &mutation_gate,
                publisher.clone(),
            )
            .expect("install first exact dispatch");
        drop(first_guard);
        start
            .expect("new dispatch returns a post-M start permit")
            .send(())
            .expect("start exact dispatch");
        crate::tokio::time::timeout(std::time::Duration::from_secs(5), first_entered.notified())
            .await
            .expect("first callback enters after M is released");

        let second_guard = crate::tokio::time::timeout(
            std::time::Duration::from_secs(1),
            Arc::clone(&mutation_gate).lock_owned(),
        )
        .await
        .expect("wedged public callback must not retain M");
        let timed_out = machine
            .await_runless_terminal_publication_dispatch(
                &runtime_id,
                first_result,
                Some(
                    meerkat_core::time_compat::Instant::now()
                        + std::time::Duration::from_millis(10),
                ),
            )
            .await;
        assert!(matches!(
            timed_out,
            Err(RuntimeDriverError::RuntimeTerminalPublicationInProgress { .. })
        ));

        release_first.notify_one();
        crate::tokio::time::timeout(
            std::time::Duration::from_secs(1),
            first_returning.notified(),
        )
        .await
        .expect("first callback returns while final M reconciliation is fenced");
        commit_dispatch_outbox(&driver, &input_ids[1], "second generation", false).await;
        let (joined_result, joined_start) = machine
            .prepare_runless_terminal_publication_dispatch(
                &driver,
                &completions,
                &mutation_gate,
                publisher.clone(),
            )
            .expect("second commit joins exact dispatch and advances high-water");
        assert!(joined_start.is_none());
        assert_eq!(
            publisher.calls.load(Ordering::SeqCst),
            1,
            "join must not mint a second callback authority while the first is pending"
        );
        drop(second_guard);

        machine
            .await_runless_terminal_publication_dispatch(&runtime_id, joined_result, None)
            .await
            .expect("generation loop must publish the post-scan commit before success");
        assert_eq!(publisher.calls.load(Ordering::SeqCst), 2);
        {
            let driver = driver.lock().await;
            assert_dispatch_outboxes_published(&driver, 2);
        }
    }

    #[tokio::test]
    async fn runless_publication_panic_clears_slot_and_cold_authority_reissues() {
        let machine = MeerkatMachine::ephemeral();
        let runtime_id = LogicalRuntimeId::new("runless-publication-panic-retry");
        let session_id = SessionId::new();
        let (driver, input_ids) =
            dispatch_test_driver(&runtime_id, &session_id, &["panic-retry"]).await;
        let completions = Arc::new(crate::tokio::sync::Mutex::new(
            crate::completion::CompletionRegistry::new(),
        ));
        let mutation_gate = Arc::new(crate::tokio::sync::Mutex::new(()));
        let publisher = Arc::new(PanicOnceDispatchPublisher {
            calls: AtomicUsize::new(0),
        });

        let guard = Arc::clone(&mutation_gate).lock_owned().await;
        commit_dispatch_outbox(&driver, &input_ids[0], "panic remains durable", true).await;
        let (first_result, start) = machine
            .prepare_runless_terminal_publication_dispatch(
                &driver,
                &completions,
                &mutation_gate,
                publisher.clone(),
            )
            .expect("install panic dispatch");
        drop(guard);
        start
            .expect("first start permit")
            .send(())
            .expect("start panic dispatch");
        let first = machine
            .await_runless_terminal_publication_dispatch(&runtime_id, first_result, None)
            .await
            .expect_err("panic must surface as a typed internal failure");
        assert!(
            first.to_string().contains("callback panicked"),
            "unexpected first dispatch error: {first:?}"
        );
        {
            let driver = driver.lock().await;
            assert_dispatch_outboxes_published(&driver, 0);
        }

        let cold_machine = MeerkatMachine::ephemeral();
        let retry_guard = Arc::clone(&mutation_gate).lock_owned().await;
        let (retry_result, retry_start) = cold_machine
            .prepare_runless_terminal_publication_dispatch(
                &driver,
                &completions,
                &mutation_gate,
                publisher.clone(),
            )
            .expect("panic cleanup must remove the poisoned process slot");
        drop(retry_guard);
        retry_start
            .expect("cold authority must install its own process dispatch")
            .send(())
            .expect("start durable retry");
        cold_machine
            .await_runless_terminal_publication_dispatch(&runtime_id, retry_result, None)
            .await
            .expect("cold authority reissues the durable outbox after panic");
        assert_eq!(publisher.calls.load(Ordering::SeqCst), 2);
        {
            let driver = driver.lock().await;
            assert_dispatch_outboxes_published(&driver, 1);
        }

        let cleanup_probe_guard = Arc::clone(&mutation_gate).lock_owned().await;
        let (cleanup_probe, cleanup_probe_start) = machine
            .prepare_runless_terminal_publication_dispatch(
                &driver,
                &completions,
                &mutation_gate,
                publisher.clone(),
            )
            .expect("panicking process dispatch must remove its poisoned slot");
        drop(cleanup_probe_guard);
        cleanup_probe_start
            .expect("original machine must install a fresh slot after panic cleanup")
            .send(())
            .expect("start panic cleanup probe");
        machine
            .await_runless_terminal_publication_dispatch(&runtime_id, cleanup_probe, None)
            .await
            .expect("published durable authority makes the fresh probe a no-op");
        assert_eq!(
            publisher.calls.load(Ordering::SeqCst),
            2,
            "already-published durable authority must not duplicate the callback"
        );
    }

    #[tokio::test]
    async fn attached_archive_publication_releases_m_and_retries_join_exact_dispatch() {
        let machine = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();
        let runtime_id = LogicalRuntimeId::for_session(&session_id);
        let entered = Arc::new(crate::tokio::sync::Notify::new());
        let release = Arc::new(crate::tokio::sync::Notify::new());
        let returning = Arc::new(crate::tokio::sync::Notify::new());
        let publisher = Arc::new(GatedDispatchPublisher {
            calls: AtomicUsize::new(0),
            batches: StdMutex::new(Vec::new()),
            first_entered: Arc::clone(&entered),
            release_first: Arc::clone(&release),
            first_returning: Arc::clone(&returning),
        });
        let publisher_handle: Arc<dyn meerkat_core::lifecycle::CoreExecutorPublicationHandle> =
            publisher.clone();
        let mut prepared = machine
            .prepare_session_materialization(session_id.clone())
            .await
            .expect("prepare attached archive fixture");
        crate::begin_session_runtime_actor_materialization(prepared.bindings())
            .expect("claim attached archive actor construction")
            .commit()
            .expect("record attached archive actor materialization");
        let executor_publication = Arc::clone(&publisher_handle);
        let pending = match prepared
            .ensure_executor_attachment(move |_witness| {
                Box::new(PublicationHandleExecutor {
                    publication_handle: Arc::clone(&executor_publication),
                })
            })
            .await
            .expect("attach public archive publication capability")
        {
            super::super::EnsureRuntimeExecutorAttachment::Pending(pending) => pending,
            super::super::EnsureRuntimeExecutorAttachment::Existing(witness) => {
                panic!("fresh archive fixture reused {witness:?}")
            }
        };
        pending
            .commit()
            .await
            .expect("commit attached archive executor");

        let interaction_uuid = meerkat_core::time_compat::new_uuid_v7();
        let input = crate::mob_adapter::create_tracked_flow_step_input(
            "attached-archive-publication",
            ContentInput::Text("pending attached archive work".to_string()),
            "attached-archive-flow",
            None,
            &interaction_uuid.to_string(),
        )
        .expect("construct attached archive directed input");
        let input_id = input.id().clone();
        assert!(
            machine
                .accept_input_without_wake(&session_id, input)
                .await
                .expect("admit attached archive directed input")
                .is_accepted()
        );
        let driver = machine
            .sessions
            .read()
            .await
            .get(&session_id)
            .map(|entry| Arc::clone(&entry.driver))
            .expect("attached archive driver remains registered");
        commit_dispatch_outbox(&driver, &input_id, "attached archive", true).await;

        let first_lease = machine
            .prepare_session_archive_lease(&session_id)
            .await
            .expect("prepare first attached archive lease")
            .expect("attached archive lease must exist");
        let first_started = meerkat_core::time_compat::Instant::now();
        let first_error = machine
            .retire_session_with_archive_lease_before(
                first_lease,
                first_started + meerkat_core::time_compat::Duration::from_millis(150),
            )
            .await
            .expect_err("wedged attached publication must respect the caller deadline");
        assert!(
            matches!(
                first_error,
                RuntimeControlPlaneError::RetirementInProgress {
                    ref runtime_id,
                    ref stage,
                } if runtime_id == &LogicalRuntimeId::for_session(&session_id)
                    && stage == "terminal_publication"
            ),
            "unexpected first archive error: {first_error:?}"
        );
        assert!(
            first_started.elapsed() < meerkat_core::time_compat::Duration::from_secs(1),
            "archive caller must not wait for a wedged custom publisher"
        );
        assert_eq!(publisher.calls.load(Ordering::SeqCst), 1);

        let probe = crate::tokio::time::timeout(
            std::time::Duration::from_secs(1),
            machine.prepare_session_archive_lease(&session_id),
        )
        .await
        .expect("publication wait must release the machine mutation lease")
        .expect("archive lease probe should remain valid")
        .expect("runtime remains registered while publication is pending");
        drop(probe);

        let retry_lease = machine
            .prepare_session_archive_lease(&session_id)
            .await
            .expect("prepare retry attached archive lease")
            .expect("retry archive lease must exist");
        let retry_error = machine
            .retire_session_with_archive_lease_before(
                retry_lease,
                meerkat_core::time_compat::Instant::now()
                    + meerkat_core::time_compat::Duration::from_millis(150),
            )
            .await
            .expect_err("same-process retry must join the retained publication dispatch");
        assert!(matches!(
            retry_error,
            RuntimeControlPlaneError::RetirementInProgress { ref stage, .. }
                if stage == "terminal_publication"
        ));
        assert_eq!(
            publisher.calls.load(Ordering::SeqCst),
            1,
            "retry must not invoke the exact publication capability twice"
        );

        release.notify_waiters();
        let final_lease = machine
            .prepare_session_archive_lease(&session_id)
            .await
            .expect("prepare converging attached archive lease")
            .expect("converging archive lease must exist");
        machine
            .retire_session_with_archive_lease_before(
                final_lease,
                meerkat_core::time_compat::Instant::now()
                    + meerkat_core::time_compat::Duration::from_secs(2),
            )
            .await
            .expect("released attached publication must converge archive retirement");
        assert_eq!(
            publisher.calls.load(Ordering::SeqCst),
            1,
            "successful retry must preserve one exact callback"
        );
        let driver = driver.lock().await;
        assert_dispatch_outboxes_published(&driver, 1);
        assert_eq!(
            driver.runtime_id(),
            &runtime_id,
            "archive must retain the exact runtime identity"
        );
    }

    #[tokio::test]
    async fn stored_only_archive_publication_releases_m_and_retries_join_exact_dispatch() {
        let machine = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();
        let runtime_id = LogicalRuntimeId::for_session(&session_id);
        let entered = Arc::new(crate::tokio::sync::Notify::new());
        let release = Arc::new(crate::tokio::sync::Notify::new());
        let returning = Arc::new(crate::tokio::sync::Notify::new());
        let publisher = Arc::new(GatedDispatchPublisher {
            calls: AtomicUsize::new(0),
            batches: StdMutex::new(Vec::new()),
            first_entered: Arc::clone(&entered),
            release_first: Arc::clone(&release),
            first_returning: Arc::clone(&returning),
        });
        let publisher_handle: Arc<dyn meerkat_core::lifecycle::CoreExecutorPublicationHandle> =
            publisher.clone();

        let mut prepared = machine
            .prepare_session_materialization(session_id.clone())
            .await
            .expect("prepare stored-only archive fixture");
        crate::begin_session_runtime_actor_materialization(prepared.bindings())
            .expect("claim stored-only archive actor construction")
            .commit()
            .expect("record stored-only archive actor materialization");
        let pending = match prepared
            .ensure_executor_attachment(|_witness| Box::new(NoPublicationHandleExecutor))
            .await
            .expect("attach executor without a publication capability")
        {
            super::super::EnsureRuntimeExecutorAttachment::Pending(pending) => pending,
            super::super::EnsureRuntimeExecutorAttachment::Existing(witness) => {
                panic!("fresh stored-only archive fixture reused {witness:?}")
            }
        };
        pending
            .commit()
            .await
            .expect("commit stored-only archive executor");
        drop(prepared);

        let interaction_uuid = meerkat_core::time_compat::new_uuid_v7();
        let input = crate::mob_adapter::create_tracked_flow_step_input(
            "stored-only-archive-publication",
            ContentInput::Text("pending stored-only archive work".to_string()),
            "stored-only-archive-flow",
            None,
            &interaction_uuid.to_string(),
        )
        .expect("construct stored-only archive directed input");
        let input_id = input.id().clone();
        let driver = machine
            .sessions
            .read()
            .await
            .get(&session_id)
            .map(|entry| Arc::clone(&entry.driver))
            .expect("stored-only archive driver remains registered");
        assert!(
            driver
                .lock()
                .await
                .as_driver_mut()
                .accept_input(input)
                .await
                .expect("restore previously admitted stored-only archive input")
                .is_accepted()
        );
        {
            let mut driver = driver.lock().await;
            let authority_handle = driver.shared_dsl_authority();
            let mut authority = authority_handle
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(
                &mut *authority,
                crate::meerkat_machine::dsl::MeerkatMachineInput::Retire {
                    session_id: crate::meerkat_machine::dsl::SessionId::from_domain(&session_id),
                },
            )
            .expect("restore committed retired lifecycle for stored-only archive");
            drop(authority);
            driver.set_control_projection(crate::RuntimeState::Retired, None, None);
        }
        commit_dispatch_outbox(&driver, &input_id, "stored-only archive", true).await;

        let first_lease = machine
            .prepare_session_archive_lease(&session_id)
            .await
            .expect("prepare first stored-only archive lease")
            .expect("stored-only archive lease must exist");
        assert!(
            machine
                .session_archive_lease_has_pending_terminals(&first_lease)
                .await
                .expect("observe committed stored-only terminal carrier")
        );
        let first_started = meerkat_core::time_compat::Instant::now();
        let first_error = machine
            .converge_session_archive_lease_terminals_before(
                first_lease,
                Some(Arc::clone(&publisher_handle)),
                first_started + meerkat_core::time_compat::Duration::from_millis(150),
            )
            .await
            .expect_err("wedged stored-only publication must respect the caller deadline");
        assert!(
            matches!(
                first_error,
                RuntimeControlPlaneError::RetirementInProgress {
                    ref runtime_id,
                    ref stage,
                } if runtime_id == &LogicalRuntimeId::for_session(&session_id)
                    && stage == "terminal_publication"
            ),
            "unexpected first stored-only archive error: {first_error:?}"
        );
        assert!(
            first_started.elapsed() < meerkat_core::time_compat::Duration::from_secs(1),
            "stored-only archive caller must not wait for a wedged custom publisher"
        );
        assert_eq!(publisher.calls.load(Ordering::SeqCst), 1);

        let probe = crate::tokio::time::timeout(
            std::time::Duration::from_secs(1),
            machine.prepare_session_archive_lease(&session_id),
        )
        .await
        .expect("stored-only publication wait must release the machine mutation lease")
        .expect("stored-only archive lease probe should remain valid")
        .expect("stored-only runtime remains registered while publication is pending");
        drop(probe);

        let retry_lease = machine
            .prepare_session_archive_lease(&session_id)
            .await
            .expect("prepare retry stored-only archive lease")
            .expect("retry stored-only archive lease must exist");
        let retry_error = machine
            .converge_session_archive_lease_terminals_before(
                retry_lease,
                Some(Arc::clone(&publisher_handle)),
                meerkat_core::time_compat::Instant::now()
                    + meerkat_core::time_compat::Duration::from_millis(150),
            )
            .await
            .expect_err("same-process retry must join the retained stored-only dispatch");
        assert!(matches!(
            retry_error,
            RuntimeControlPlaneError::RetirementInProgress { ref stage, .. }
                if stage == "terminal_publication"
        ));
        assert_eq!(
            publisher.calls.load(Ordering::SeqCst),
            1,
            "stored-only retry must not invoke the exact publication capability twice"
        );

        release.notify_waiters();
        let final_lease = machine
            .prepare_session_archive_lease(&session_id)
            .await
            .expect("prepare converging stored-only archive lease")
            .expect("converging stored-only archive lease must exist");
        machine
            .converge_session_archive_lease_terminals_before(
                final_lease,
                Some(publisher_handle),
                meerkat_core::time_compat::Instant::now()
                    + meerkat_core::time_compat::Duration::from_secs(2),
            )
            .await
            .expect("released stored-only publication must converge");
        assert_eq!(
            publisher.calls.load(Ordering::SeqCst),
            1,
            "successful stored-only retry must preserve one exact callback"
        );
        let driver = driver.lock().await;
        assert_dispatch_outboxes_published(&driver, 1);
        assert_eq!(
            driver.runtime_id(),
            &runtime_id,
            "stored-only archive must retain the exact runtime identity"
        );
    }

    #[tokio::test]
    async fn archive_lease_preparation_deadline_does_not_cancel_wedged_store_read() {
        let store = Arc::new(crate::store::InMemoryRuntimeStore::new());
        let session_id = SessionId::new();
        seed_durable_attached_archive_authority(store.as_ref(), &session_id).await;

        let entered = Arc::new(crate::tokio::sync::Notify::new());
        let release = Arc::new(crate::tokio::sync::Notify::new());
        let baseline_load_calls = store.machine_lifecycle_load_calls();
        let expired = MeerkatMachine::persistent(
            store.clone(),
            Arc::new(meerkat_store::MemoryBlobStore::new()),
        );
        let expired_error = match expired
            .prepare_session_archive_lease_before(
                &session_id,
                meerkat_core::time_compat::Instant::now(),
            )
            .await
        {
            Ok(_) => panic!("an expired archive budget must fail before spawning preparation"),
            Err(error) => error,
        };
        assert!(matches!(
            expired_error,
            RuntimeControlPlaneError::RetirementInProgress { ref stage, .. }
                if stage == "archive_lease_preparation"
        ));
        assert_eq!(
            store.machine_lifecycle_load_calls(),
            baseline_load_calls,
            "zero-budget retry must not issue or queue mutable RuntimeStore work"
        );
        drop(expired);
        store.block_next_machine_lifecycle_load(Arc::clone(&entered), Arc::clone(&release));
        let restarted = Arc::new(MeerkatMachine::persistent(
            store.clone(),
            Arc::new(meerkat_store::MemoryBlobStore::new()),
        ));
        let first_machine = Arc::clone(&restarted);
        let first_session_id = session_id.clone();
        let first_prepare = crate::tokio::spawn(async move {
            first_machine
                .prepare_session_archive_lease_before(
                    &first_session_id,
                    meerkat_core::time_compat::Instant::now()
                        + meerkat_core::time_compat::Duration::from_millis(100),
                )
                .await
        });
        entered.notified().await;
        let registration_probe = crate::tokio::time::timeout(
            std::time::Duration::from_secs(1),
            restarted.lock_session_registration_transaction(&session_id),
        )
        .await
        .expect("wedged RuntimeStore preparation must not retain registration transaction T");
        drop(registration_probe);
        let mut followers = Vec::new();
        for _ in 0..4 {
            let follower_machine = Arc::clone(&restarted);
            let follower_session_id = session_id.clone();
            followers.push(crate::tokio::spawn(async move {
                follower_machine
                    .prepare_session_archive_lease_before(
                        &follower_session_id,
                        meerkat_core::time_compat::Instant::now()
                            + meerkat_core::time_compat::Duration::from_millis(100),
                    )
                    .await
            }));
        }
        let first_error = match first_prepare
            .await
            .expect("deadline-aware archive preparation task must join")
        {
            Ok(_) => panic!("wedged RuntimeStore read must return typed pending"),
            Err(error) => error,
        };
        assert!(matches!(
            first_error,
            RuntimeControlPlaneError::RetirementInProgress { ref stage, .. }
                if stage == "archive_lease_preparation"
        ));
        for follower in followers {
            let follower_error = match follower
                .await
                .expect("archive preparation follower task must join")
            {
                Ok(_) => panic!("followers must share the wedged leader deadline outcome"),
                Err(error) => error,
            };
            assert!(matches!(
                follower_error,
                RuntimeControlPlaneError::RetirementInProgress { ref stage, .. }
                    if stage == "archive_lease_preparation"
            ));
        }
        assert_eq!(
            store.machine_lifecycle_load_calls(),
            baseline_load_calls + 1,
            "bounded retries must join one exact process-owned RuntimeStore read"
        );

        release.notify_waiters();
        let lease = restarted
            .prepare_session_archive_lease_before(
                &session_id,
                meerkat_core::time_compat::Instant::now()
                    + meerkat_core::time_compat::Duration::from_secs(2),
            )
            .await
            .expect("retry must proceed after the process-owned store read completes")
            .expect("durable runtime authority must still yield an archive lease");
        drop(lease);
    }

    #[tokio::test]
    async fn archive_lease_preparation_panic_clears_singleflight_for_retry() {
        let store = Arc::new(crate::store::InMemoryRuntimeStore::new());
        let session_id = SessionId::new();
        seed_durable_attached_archive_authority(store.as_ref(), &session_id).await;

        let restarted = MeerkatMachine::persistent(
            store.clone(),
            Arc::new(meerkat_store::MemoryBlobStore::new()),
        );
        let baseline_load_calls = store.machine_lifecycle_load_calls();
        store.panic_next_machine_lifecycle_load();
        let panic_error = match restarted
            .prepare_session_archive_lease_before(
                &session_id,
                meerkat_core::time_compat::Instant::now()
                    + meerkat_core::time_compat::Duration::from_secs(2),
            )
            .await
        {
            Ok(_) => panic!("panicking RuntimeStore preparation must return a typed error"),
            Err(error) => error,
        };
        assert!(
            matches!(
                panic_error,
                RuntimeControlPlaneError::Internal(ref detail)
                    if detail.contains("archive lease preparation panicked")
            ),
            "unexpected archive preparation panic result: {panic_error:?}"
        );
        assert!(
            !restarted
                .pending_session_archive_lease_preparations
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .contains_key(&session_id),
            "panicking preparation must remove its exact single-flight slot"
        );
        assert_eq!(
            store.machine_lifecycle_load_calls(),
            baseline_load_calls + 1,
            "the panicking leader must issue exactly one lifecycle load"
        );

        let lease = restarted
            .prepare_session_archive_lease_before(
                &session_id,
                meerkat_core::time_compat::Instant::now()
                    + meerkat_core::time_compat::Duration::from_secs(2),
            )
            .await
            .expect("retry must reissue after panic cleanup")
            .expect("durable runtime authority must survive preparation panic");
        assert_eq!(
            store.machine_lifecycle_load_calls(),
            baseline_load_calls + 2,
            "retry must issue one fresh lifecycle load after exact slot cleanup"
        );
        drop(lease);
    }

    #[tokio::test]
    async fn archive_retry_joins_before_wedged_publication_receipt_cas() {
        let store = Arc::new(crate::store::InMemoryRuntimeStore::new());
        let machine = Arc::new(MeerkatMachine::persistent(
            store.clone(),
            Arc::new(meerkat_store::MemoryBlobStore::new()),
        ));
        let session_id = SessionId::new();
        machine
            .register_session(session_id.clone())
            .await
            .expect("register receipt-CAS archive fixture");
        let driver = machine
            .sessions
            .read()
            .await
            .get(&session_id)
            .map(|entry| Arc::clone(&entry.driver))
            .expect("receipt-CAS driver remains registered");
        let input = crate::mob_adapter::create_tracked_flow_step_input(
            "archive-receipt-cas",
            ContentInput::Text("terminal awaiting durable receipt CAS".to_string()),
            "archive-receipt-cas-flow",
            None,
            &meerkat_core::time_compat::new_uuid_v7().to_string(),
        )
        .expect("construct receipt-CAS directed input");
        let input_id = input.id().clone();
        {
            let mut driver = driver.lock().await;
            seed_dispatch_attached_authority(
                &mut driver,
                &session_id,
                &LogicalRuntimeId::for_session(&session_id),
            );
        }
        assert!(
            driver
                .lock()
                .await
                .as_driver_mut()
                .accept_input(input)
                .await
                .expect("admit receipt-CAS directed input")
                .is_accepted()
        );
        {
            let mut driver = driver.lock().await;
            let authority_handle = driver.shared_dsl_authority();
            let mut authority = authority_handle
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(
                &mut *authority,
                crate::meerkat_machine::dsl::MeerkatMachineInput::Retire {
                    session_id: crate::meerkat_machine::dsl::SessionId::from_domain(&session_id),
                },
            )
            .expect("restore retired receipt-CAS lifecycle");
            drop(authority);
            driver.set_control_projection(crate::RuntimeState::Retired, None, None);
        }
        commit_dispatch_outbox(&driver, &input_id, "archive receipt CAS", true).await;

        let entered = Arc::new(crate::tokio::sync::Notify::new());
        let release = Arc::new(crate::tokio::sync::Notify::new());
        store.block_next_input_state_batch_cas_before_mutation(
            Arc::clone(&entered),
            Arc::clone(&release),
        );
        let publisher = Arc::new(CountingDispatchPublisher {
            calls: AtomicUsize::new(0),
        });
        let first_lease = machine
            .prepare_session_archive_lease(&session_id)
            .await
            .expect("prepare first receipt-CAS archive lease")
            .expect("receipt-CAS archive lease must exist");
        let first_machine = Arc::clone(&machine);
        let first_publisher = publisher.clone();
        let first = crate::tokio::spawn(async move {
            first_machine
                .converge_session_archive_lease_terminals_before(
                    first_lease,
                    Some(first_publisher),
                    meerkat_core::time_compat::Instant::now()
                        + meerkat_core::time_compat::Duration::from_secs(5),
                )
                .await
        });
        entered.notified().await;

        let retry_lease = machine
            .prepare_session_archive_lease(&session_id)
            .await
            .expect("prepare retry while receipt CAS is wedged")
            .expect("receipt-CAS retry lease must exist");
        assert!(
            machine.session_archive_lease_has_pending_terminal_dispatch(&retry_lease),
            "retry must observe the exact process slot without locking the wedged driver"
        );
        let retry_error = machine
            .converge_session_archive_lease_terminals_before(
                retry_lease,
                None,
                meerkat_core::time_compat::Instant::now()
                    + meerkat_core::time_compat::Duration::from_millis(100),
            )
            .await
            .expect_err("retry acknowledgement must remain bounded while receipt CAS is wedged");
        assert!(matches!(
            retry_error,
            RuntimeControlPlaneError::RetirementInProgress { ref stage, .. }
                if stage == "terminal_publication"
        ));
        let probe = crate::tokio::time::timeout(
            std::time::Duration::from_secs(1),
            machine.prepare_session_archive_lease(&session_id),
        )
        .await
        .expect("wedged receipt CAS retry must not retain M")
        .expect("archive lease probe must remain valid")
        .expect("receipt-CAS runtime remains registered");
        drop(probe);

        release.notify_waiters();
        first
            .await
            .expect("receipt-CAS convergence task must join")
            .expect("released receipt CAS must converge");
        assert_eq!(publisher.calls.load(Ordering::SeqCst), 1);
        let stored = crate::store::RuntimeStore::load_input_state(
            store.as_ref(),
            &LogicalRuntimeId::for_session(&session_id),
            &input_id,
        )
        .await
        .expect("load durably published receipt-CAS input")
        .expect("receipt-CAS input must remain durably addressable");
        assert!(matches!(
            stored
                .state
                .interaction_terminal_outbox
                .map(|outbox| outbox.phase),
            Some(InteractionTerminalOutboxPhase::Published { .. })
        ));
    }

    #[tokio::test]
    async fn pending_stored_only_terminal_fences_successor_actor_materialization() {
        let machine = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();
        machine
            .register_session(session_id.clone())
            .await
            .expect("register cold stored-only runtime fixture");
        let driver = machine
            .sessions
            .read()
            .await
            .get(&session_id)
            .map(|entry| Arc::clone(&entry.driver))
            .expect("cold stored-only driver remains registered");
        let input = crate::mob_adapter::create_tracked_flow_step_input(
            "stored-only-successor-fence",
            ContentInput::Text("terminal pending before successor revival".to_string()),
            "stored-only-successor-flow",
            None,
            &meerkat_core::time_compat::new_uuid_v7().to_string(),
        )
        .expect("construct stored-only successor-fence input");
        let input_id = input.id().clone();
        {
            let mut driver = driver.lock().await;
            seed_dispatch_attached_authority(
                &mut driver,
                &session_id,
                &LogicalRuntimeId::for_session(&session_id),
            );
        }
        assert!(
            driver
                .lock()
                .await
                .as_driver_mut()
                .accept_input(input)
                .await
                .expect("admit stored-only successor-fence input")
                .is_accepted()
        );
        {
            let mut driver = driver.lock().await;
            let authority_handle = driver.shared_dsl_authority();
            let mut authority = authority_handle
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            crate::meerkat_machine::dsl::MeerkatMachineMutator::apply(
                &mut *authority,
                crate::meerkat_machine::dsl::MeerkatMachineInput::Retire {
                    session_id: crate::meerkat_machine::dsl::SessionId::from_domain(&session_id),
                },
            )
            .expect("restore retired stored-only successor-fence lifecycle");
            drop(authority);
            driver.set_control_projection(crate::RuntimeState::Retired, None, None);
        }
        commit_dispatch_outbox(&driver, &input_id, "stored-only successor fence", true).await;

        let error = machine
            .prepare_session_materialization(session_id.clone())
            .await
            .expect_err("pending predecessor terminal must fence actor B materialization");
        assert!(
            error
                .to_string()
                .to_ascii_lowercase()
                .contains("runtime terminal publication remains in progress"),
            "unexpected successor-fence error: {error:?}"
        );

        let publisher = Arc::new(CountingDispatchPublisher {
            calls: AtomicUsize::new(0),
        });
        let lease = machine
            .prepare_session_archive_lease(&session_id)
            .await
            .expect("prepare exact stored-only convergence lease")
            .expect("stored-only successor-fence lease remains present");
        machine
            .converge_session_archive_lease_terminals_before(
                lease,
                Some(publisher.clone()),
                meerkat_core::time_compat::Instant::now()
                    + meerkat_core::time_compat::Duration::from_secs(2),
            )
            .await
            .expect("predecessor terminal must converge before successor materialization");
        assert_eq!(publisher.calls.load(Ordering::SeqCst), 1);

        machine
            .unregister_session(&session_id)
            .await
            .expect("remove the retired predecessor only after exact receipt convergence");
        let mut prepared = machine
            .prepare_session_materialization(session_id)
            .await
            .expect("actor B materialization may proceed after exact receipt convergence");
        prepared
            .rollback_now()
            .await
            .expect("rollback successor-fence test materialization");
    }

    #[tokio::test]
    async fn attached_actor_recovery_replaces_machine_retained_publication_handle_exactly() {
        let machine = Arc::new(MeerkatMachine::ephemeral());
        let session_id = SessionId::new();
        let actor_a = Arc::new(CountingDispatchPublisher {
            calls: AtomicUsize::new(0),
        });
        let actor_b = Arc::new(CountingDispatchPublisher {
            calls: AtomicUsize::new(0),
        });
        let mut prepared = machine
            .prepare_session_materialization(session_id.clone())
            .await
            .expect("prepare actor A materialization");
        crate::begin_session_runtime_actor_materialization(prepared.bindings())
            .expect("claim actor A construction")
            .commit()
            .expect("record actor A materialization");
        let actor_a_handle: Arc<dyn meerkat_core::lifecycle::CoreExecutorPublicationHandle> =
            actor_a.clone();
        let actor_a_factory_handle = Arc::clone(&actor_a_handle);
        let pending = match prepared
            .ensure_executor_attachment(move |_witness| {
                Box::new(PublicationHandleExecutor {
                    publication_handle: Arc::clone(&actor_a_factory_handle),
                })
            })
            .await
            .expect("attach actor A executor")
        {
            super::super::EnsureRuntimeExecutorAttachment::Pending(pending) => pending,
            super::super::EnsureRuntimeExecutorAttachment::Existing(witness) => {
                panic!("fresh publication fixture reused {witness:?}")
            }
        };
        let attachment = pending.commit().await.expect("commit actor A attachment");

        let mut recovery = machine
            .prepare_attached_session_actor_recovery(&attachment)
            .await
            .expect("open actor B recovery under exact attachment M");
        crate::begin_session_runtime_actor_materialization(recovery.bindings())
            .expect("claim actor B construction")
            .commit()
            .expect("record actor B materialization");
        let actor_b_handle: Arc<dyn meerkat_core::lifecycle::CoreExecutorPublicationHandle> =
            actor_b.clone();
        recovery
            .replace_publication_handle_for_recovered_actor(actor_b_handle)
            .await
            .expect("replace retained publication capability before releasing M");
        recovery
            .commit_actor()
            .expect("commit actor B under the unchanged executor attachment");

        let retained = machine
            .sessions
            .read()
            .await
            .get(&session_id)
            .and_then(RuntimeSessionEntry::publication_handle)
            .expect("machine retains actor B publication handle");
        let event = meerkat_core::event::AgentEvent::InteractionComplete {
            interaction_id: meerkat_core::interaction::InteractionId(
                meerkat_core::time_compat::new_uuid_v7(),
            ),
            result: "actor B exact retained handle".to_string(),
            structured_output: None,
        };
        retained
            .publish_interaction_terminals(std::slice::from_ref(&event))
            .await
            .expect("machine-retained actor B capability publishes");
        assert_eq!(actor_a.calls.load(Ordering::SeqCst), 0);
        assert_eq!(actor_b.calls.load(Ordering::SeqCst), 1);

        actor_a_handle
            .publish_interaction_terminals(std::slice::from_ref(&event))
            .await
            .expect("detached actor A fixture remains a distinct capability");
        assert_eq!(actor_a.calls.load(Ordering::SeqCst), 1);
        assert_eq!(actor_b.calls.load(Ordering::SeqCst), 1);
    }

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
