use super::*;

impl MeerkatMachine {
    pub(super) async fn interrupt_current_run_inner(
        &self,
        session_id: &SessionId,
    ) -> Result<(), RuntimeDriverError> {
        let control_tx = {
            let sessions = self.sessions.read().await;
            let entry = sessions
                .get(session_id)
                .ok_or(RuntimeDriverError::NotReady {
                    state: RuntimeState::Destroyed,
                })?;
            entry.control_sender()
        };

        let Some(control_tx) = control_tx else {
            let state = self
                .existing_session_runtime_state(session_id)
                .await
                .unwrap_or(RuntimeState::Destroyed);
            return Err(RuntimeDriverError::NotReady { state });
        };
        control_tx
            .send(RunControlCommand::CancelCurrentRun {
                reason: "mob interrupt".to_string(),
            })
            .await
            .map_err(|err| RuntimeDriverError::Internal(format!("failed to send interrupt: {err}")))
    }

    pub(super) async fn cancel_after_boundary_inner(
        &self,
        session_id: &SessionId,
    ) -> Result<(), RuntimeDriverError> {
        let control_tx = {
            let sessions = self.sessions.read().await;
            let entry = sessions
                .get(session_id)
                .ok_or(RuntimeDriverError::NotReady {
                    state: RuntimeState::Destroyed,
                })?;
            entry.control_sender()
        };

        let Some(control_tx) = control_tx else {
            let state = self
                .existing_session_runtime_state(session_id)
                .await
                .unwrap_or(RuntimeState::Destroyed);
            return Err(RuntimeDriverError::NotReady { state });
        };
        control_tx
            .send(RunControlCommand::CancelAfterBoundary {
                reason: "boundary cancel".to_string(),
            })
            .await
            .map_err(|err| {
                RuntimeDriverError::Internal(format!("failed to send cancel_after_boundary: {err}"))
            })
    }

    /// Stop the attached runtime executor through the out-of-band control
    /// channel. When no loop is attached yet, a stop command is applied directly
    /// against the driver so queued work is still terminated consistently.
    pub async fn stop_runtime_executor(
        &self,
        session_id: &SessionId,
        command: RunControlCommand,
    ) -> Result<(), RuntimeDriverError> {
        self.execute_meerkat_machine_command(
            None,
            MeerkatMachineCommand::StopRuntimeExecutor {
                session_id: session_id.clone(),
                command,
            },
        )
        .await
        .map_err(MeerkatMachine::driver_error_from_command_error)
        .map(|_| ())
    }

    pub(super) async fn stop_runtime_executor_inner(
        &self,
        session_id: &SessionId,
        command: RunControlCommand,
    ) -> Result<(), RuntimeDriverError> {
        let (driver, completions, control_tx) = {
            let sessions = self.sessions.read().await;
            let entry = sessions
                .get(session_id)
                .ok_or(RuntimeDriverError::NotReady {
                    state: RuntimeState::Destroyed,
                })?;
            (
                entry.driver.clone(),
                entry.completions.clone(),
                entry.control_sender(),
            )
        };

        let state_before_stop = self
            .existing_session_runtime_state(session_id)
            .await
            .unwrap_or(RuntimeState::Destroyed);

        if let Some(control_tx) = control_tx
            && control_tx.send(command.clone()).await.is_ok()
        {
            if matches!(
                (state_before_stop, &command),
                (
                    RuntimeState::Attached,
                    RunControlCommand::StopRuntimeExecutor { .. }
                )
            ) {
                let _ = tokio::time::timeout(std::time::Duration::from_millis(200), async {
                    loop {
                        match self.existing_session_runtime_state(session_id).await {
                            Some(RuntimeState::Stopped | RuntimeState::Destroyed) => break,
                            Some(RuntimeState::Attached) => {
                                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                            }
                            Some(_) | None => break,
                        }
                    }
                })
                .await;
            }

            return Ok(());
        }

        if matches!(command, RunControlCommand::StopRuntimeExecutor { .. }) {
            let mut driver = driver.lock().await;
            machine_stop_runtime(&mut driver).await?;
            drop(driver);
            self.notify_runtime_executor_exited(session_id).await;
            let mut completions = completions.lock().await;
            completions.resolve_all_terminated("runtime stopped");
            drop(completions);

            // No live control sender was available for this stop path. Scrub any
            // dead attachment capabilities that may still be published.
            let mut sessions = self.sessions.write().await;
            if let Some(entry) = sessions.get_mut(session_id) {
                entry.clear_dead_attachment();
            }
            Ok(())
        } else {
            Err(RuntimeDriverError::Internal(
                "failed to send stop: runtime loop is unavailable".into(),
            ))
        }
    }

    /// Accept an input and execute it synchronously through the runtime driver.
    ///
    /// Used by surfaces that need the request/response shape while still
    /// preserving v9 input lifecycle semantics.
    pub async fn accept_input_and_run<T, F, Fut>(
        &self,
        session_id: &SessionId,
        input: Input,
        op: F,
    ) -> Result<T, RuntimeDriverError>
    where
        F: FnOnce(RunId, meerkat_core::lifecycle::run_primitive::RunPrimitive) -> Fut,
        Fut: Future<Output = Result<(T, CoreApplyOutput), RuntimeDriverError>>,
    {
        let MeerkatMachineRunPrepared {
            input_id,
            run_id,
            primitive,
        } = match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::Prepare {
                    session_id: session_id.clone(),
                    input,
                },
            )
            .await
            .map_err(MeerkatMachine::driver_error_from_command_error)?
        {
            MeerkatMachineCommandResult::Prepared(prepared) => prepared,
            other => {
                return Err(RuntimeDriverError::Internal(format!(
                    "unexpected command result preparing Meerkat run: {other:?}"
                )));
            }
        };

        match op(run_id.clone(), primitive).await {
            Ok((result, output)) => {
                self.execute_meerkat_machine_command(
                    None,
                    MeerkatMachineCommand::Commit {
                        session_id: session_id.clone(),
                        input_id,
                        run_id,
                        output,
                    },
                )
                .await
                .map_err(MeerkatMachine::driver_error_from_command_error)?;
                Ok(result)
            }
            Err(err) => {
                self.execute_meerkat_machine_command(
                    None,
                    MeerkatMachineCommand::Fail {
                        session_id: session_id.clone(),
                        run_id,
                        error: err.to_string(),
                    },
                )
                .await
                .map_err(MeerkatMachine::driver_error_from_command_error)?;
                Err(err)
            }
        }
    }

    /// Accept an input and return a completion handle that resolves when the
    /// input reaches a terminal state (Consumed or Abandoned).
    ///
    /// Returns `(AcceptOutcome, Option<CompletionHandle>)`:
    /// - `(Accepted, Some(handle))` — await handle for result
    /// - `(Accepted, None)` — input reached a terminal state during admission
    /// - `(Deduplicated, Some(handle))` — joined in-flight waiter
    /// - `(Deduplicated, None)` — input already terminal; no waiter needed
    /// - `(Rejected, _)` — returned as `Err(ValidationFailed)`
    pub async fn accept_input_with_completion(
        &self,
        session_id: &SessionId,
        input: Input,
    ) -> Result<(AcceptOutcome, Option<crate::completion::CompletionHandle>), RuntimeDriverError>
    {
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::AcceptWithCompletion {
                    session_id: session_id.clone(),
                    input,
                },
            )
            .await
            .map_err(MeerkatMachine::driver_error_from_command_error)?
        {
            MeerkatMachineCommandResult::AcceptWithCompletion {
                outcome,
                handle,
                admission_signal: _,
            } => Ok((outcome, handle)),
            other => Err(RuntimeDriverError::Internal(format!(
                "unexpected command result for accept_input_with_completion: {other:?}"
            ))),
        }
    }

    /// Accept an input but intentionally do not wake the runtime loop.
    ///
    /// This is reserved for explicitly queued-only surface contracts that
    /// stage work for the next turn boundary instead of waking an idle session
    /// immediately.
    pub async fn accept_input_without_wake(
        &self,
        session_id: &SessionId,
        input: Input,
    ) -> Result<AcceptOutcome, RuntimeDriverError> {
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::AcceptWithoutWake {
                    session_id: session_id.clone(),
                    input,
                },
            )
            .await
            .map_err(MeerkatMachine::driver_error_from_command_error)?
        {
            MeerkatMachineCommandResult::AcceptOutcome(outcome) => Ok(outcome),
            other => Err(RuntimeDriverError::Internal(format!(
                "unexpected command result for accept_input_without_wake: {other:?}"
            ))),
        }
    }

    /// Get the shared ops lifecycle registry for a session/runtime instance.
    pub async fn ops_lifecycle_registry(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<crate::ops_lifecycle::RuntimeOpsLifecycleRegistry>> {
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::OpsLifecycleRegistry {
                    session_id: session_id.clone(),
                },
            )
            .await
        {
            Ok(MeerkatMachineCommandResult::OpsLifecycleRegistry(registry)) => registry,
            Ok(_) => {
                tracing::error!("ops_lifecycle_registry: unexpected command result variant");
                None
            }
            Err(_) => None,
        }
    }

    /// Prepare canonical runtime bindings for a session.
    ///
    /// This is the single canonical helper that replaces the hand-rolled
    /// `register_session()` + `ops_lifecycle_registry()` + manual threading
    /// dance. All runtime-backed surfaces should call this instead.
    ///
    /// The method is idempotent: if the session is already registered, it
    /// returns bindings from the existing entry. The epoch_id is stable
    /// across repeated calls for the same session.
    pub async fn prepare_bindings(
        &self,
        session_id: SessionId,
    ) -> Result<meerkat_core::SessionRuntimeBindings, RuntimeBindingsError> {
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::PrepareBindings {
                    session_id: session_id.clone(),
                },
            )
            .await
        {
            Ok(MeerkatMachineCommandResult::Bindings(bindings)) => Ok(bindings),
            Ok(_) => {
                tracing::error!("prepare_bindings: unexpected command result variant");
                Err(RuntimeBindingsError::SessionNotFound(session_id))
            }
            Err(_) => Err(RuntimeBindingsError::SessionNotFound(session_id)),
        }
    }
}
