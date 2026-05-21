use super::*;

#[cfg(feature = "live")]
fn dsl_live_channel_status_from_observation(
    status: &meerkat_core::live_adapter::LiveAdapterStatus,
) -> (
    crate::meerkat_machine::dsl::LiveChannelPublicStatus,
    Option<crate::meerkat_machine::dsl::LiveChannelDegradationReason>,
    Option<String>,
) {
    use crate::meerkat_machine::dsl::{
        LiveChannelDegradationReason as DslReason, LiveChannelPublicStatus as DslStatus,
    };
    use meerkat_core::live_adapter::LiveAdapterStatus;

    match status {
        LiveAdapterStatus::Idle => (DslStatus::Idle, None, None),
        LiveAdapterStatus::Opening => (DslStatus::Opening, None, None),
        LiveAdapterStatus::Ready => (DslStatus::Ready, None, None),
        LiveAdapterStatus::Closing => (DslStatus::Closing, None, None),
        LiveAdapterStatus::Closed => (DslStatus::Closed, None, None),
        LiveAdapterStatus::Degraded { reason } => {
            let (reason, detail) = dsl_live_channel_degradation_reason(reason);
            (DslStatus::Degraded, Some(reason), detail)
        }
        other => (
            DslStatus::Degraded,
            Some(DslReason::Unknown),
            Some(format!("{other:?}")),
        ),
    }
}

#[cfg(feature = "live")]
fn dsl_live_channel_degradation_reason(
    reason: &meerkat_core::live_adapter::LiveDegradationReason,
) -> (
    crate::meerkat_machine::dsl::LiveChannelDegradationReason,
    Option<String>,
) {
    use crate::meerkat_machine::dsl::LiveChannelDegradationReason as DslReason;
    use meerkat_core::live_adapter::LiveDegradationReason;

    match reason {
        LiveDegradationReason::RateLimited => (DslReason::RateLimited, None),
        LiveDegradationReason::ProviderThrottled => (DslReason::ProviderThrottled, None),
        LiveDegradationReason::NetworkUnstable => (DslReason::NetworkUnstable, None),
        LiveDegradationReason::Other { detail } => {
            (DslReason::Other, Some(detail.clone().into_owned()))
        }
        other => (DslReason::Unknown, Some(format!("{other:?}"))),
    }
}

impl MeerkatMachine {
    #[cfg(feature = "live")]
    pub async fn resolve_live_refresh_queued_result(
        &self,
        session_id: &SessionId,
        acceptance: &meerkat_live::LiveRefreshQueueAcceptance,
    ) -> Result<LiveRefreshResultAuthority, RuntimeDriverError> {
        let channel_id = acceptance.channel_id().to_string();
        let (_, effects) = self
            .apply_session_dsl_input(
                session_id,
                crate::meerkat_machine::dsl::MeerkatMachineInput::RecordLiveRefreshQueued {
                    channel_id: channel_id.clone(),
                    queue_acceptance_sequence: acceptance.acceptance_sequence(),
                },
                "RecordLiveRefreshQueued",
            )
            .await
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;

        effects
            .as_slice()
            .iter()
            .find_map(|effect| match effect {
                crate::meerkat_machine::dsl::MeerkatMachineEffect::LiveRefreshResultResolved {
                    channel_id: effect_channel_id,
                    status,
                    refresh_enqueued,
                    sequence,
                    queue_acceptance_sequence,
                } if *effect_channel_id == channel_id => Some(LiveRefreshResultAuthority {
                    status: *status,
                    refresh_enqueued: *refresh_enqueued,
                    sequence: *sequence,
                    queue_acceptance_sequence: *queue_acceptance_sequence,
                }),
                _ => None,
            })
            .ok_or_else(|| {
                RuntimeDriverError::Internal(format!(
                    "RecordLiveRefreshQueued for channel '{channel_id}' emitted no LiveRefreshResultResolved effect"
                ))
            })
    }

    #[cfg(feature = "live")]
    pub async fn resolve_live_close_result(
        &self,
        session_id: &SessionId,
        observation: &meerkat_live::LiveChannelCloseObservation,
    ) -> Result<LiveCloseResultAuthority, RuntimeDriverError> {
        let channel_id = observation.channel_id().to_string();
        let (_, effects) = self
            .apply_session_dsl_input(
                session_id,
                crate::meerkat_machine::dsl::MeerkatMachineInput::RecordLiveCloseClosed {
                    channel_id: channel_id.clone(),
                    close_observation_sequence: observation.close_sequence(),
                },
                "RecordLiveCloseClosed",
            )
            .await
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;

        effects
            .as_slice()
            .iter()
            .find_map(|effect| match effect {
                crate::meerkat_machine::dsl::MeerkatMachineEffect::LiveCloseResultResolved {
                    channel_id: effect_channel_id,
                    status,
                    closed,
                    sequence,
                    close_observation_sequence,
                } if *effect_channel_id == channel_id
                    && *close_observation_sequence == observation.close_sequence() =>
                {
                    Some(LiveCloseResultAuthority {
                        status: *status,
                        closed: *closed,
                        sequence: *sequence,
                        close_observation_sequence: *close_observation_sequence,
                    })
                }
                _ => None,
            })
            .ok_or_else(|| {
                RuntimeDriverError::Internal(format!(
                    "RecordLiveCloseClosed for channel '{channel_id}' emitted no LiveCloseResultResolved effect"
                ))
            })
    }

    #[cfg(feature = "live")]
    pub async fn resolve_live_channel_status_result(
        &self,
        session_id: &SessionId,
        observation: &meerkat_live::LiveChannelStatusObservation,
    ) -> Result<LiveChannelStatusAuthority, RuntimeDriverError> {
        let channel_id = observation.channel_id().to_string();
        let (status, degradation_reason, degradation_detail) =
            dsl_live_channel_status_from_observation(observation.status());
        let (_, effects) = self
            .apply_session_dsl_input(
                session_id,
                crate::meerkat_machine::dsl::MeerkatMachineInput::RecordLiveChannelStatus {
                    channel_id: channel_id.clone(),
                    status,
                    status_observation_sequence: observation.observation_sequence(),
                    degradation_reason,
                    degradation_detail: degradation_detail.clone(),
                },
                "RecordLiveChannelStatus",
            )
            .await
            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;

        effects
            .as_slice()
            .iter()
            .find_map(|effect| match effect {
                crate::meerkat_machine::dsl::MeerkatMachineEffect::LiveChannelStatusResolved {
                    channel_id: effect_channel_id,
                    status,
                    sequence,
                    status_observation_sequence,
                    degradation_reason,
                    degradation_detail,
                } if *effect_channel_id == channel_id
                    && *status_observation_sequence == observation.observation_sequence() =>
                {
                    Some(LiveChannelStatusAuthority {
                        status: *status,
                        sequence: *sequence,
                        status_observation_sequence: *status_observation_sequence,
                        degradation_reason: *degradation_reason,
                        degradation_detail: degradation_detail.clone(),
                    })
                }
                _ => None,
            })
            .ok_or_else(|| {
                RuntimeDriverError::Internal(format!(
                    "RecordLiveChannelStatus for channel '{channel_id}' emitted no LiveChannelStatusResolved effect"
                ))
            })
    }

    pub(super) async fn cancel_after_boundary_inner(
        &self,
        session_id: &SessionId,
    ) -> Result<(), RuntimeDriverError> {
        let staged = match self
            .stage_session_dsl_transition(
                session_id,
                crate::meerkat_machine::dsl::MeerkatMachineInput::CancelAfterBoundary {
                    reason: "boundary cancel".to_string(),
                },
                "CancelAfterBoundary",
            )
            .await
        {
            Ok(staged) => staged,
            Err(_) => {
                return Err(RuntimeDriverError::NotReady {
                    state: self
                        .existing_session_runtime_state(session_id)
                        .await
                        .unwrap_or(RuntimeState::Destroyed),
                });
            }
        };
        let projected_effect =
            crate::effect::runtime_effect_projection_from_dsl_effects(&staged.effects)
                .map_err(RuntimeDriverError::Internal)?;

        let (effect_tx, boundary_handle) = {
            let sessions = self.sessions.read().await;
            let entry = sessions
                .get(session_id)
                .ok_or(RuntimeDriverError::NotReady {
                    state: RuntimeState::Destroyed,
                })?;
            (entry.effect_sender(), entry.boundary_handle())
        };

        if let Err(err) = self
            .dispatch_cancel_after_boundary_runtime_effect(
                session_id,
                effect_tx,
                boundary_handle,
                projected_effect,
                "CancelAfterBoundary",
            )
            .await
        {
            self.restore_session_dsl_state(session_id, staged.previous_snapshot)
                .await;
            return Err(err);
        }

        Ok(())
    }

    /// Stop the attached runtime executor through the out-of-band control
    /// channel. When no loop is attached yet, a stop command is applied directly
    /// against the driver so queued work is still terminated consistently.
    pub async fn stop_runtime_executor(
        &self,
        session_id: &SessionId,
        reason: impl Into<String>,
    ) -> Result<(), RuntimeDriverError> {
        self.execute_meerkat_machine_command(
            None,
            MeerkatMachineCommand::StopRuntimeExecutor {
                session_id: session_id.clone(),
                reason: reason.into(),
            },
        )
        .await
        .map_err(MeerkatMachine::driver_error_from_command_error)
        .map(|_| ())
    }

    pub(super) async fn stop_runtime_executor_inner(
        &self,
        session_id: &SessionId,
        reason: String,
    ) -> Result<(), RuntimeDriverError> {
        let (driver, effect_tx, effect) = {
            let Some(_gate_guard) = self.lock_current_session_mutation_gate(session_id).await
            else {
                return Err(RuntimeDriverError::NotReady {
                    state: RuntimeState::Destroyed,
                });
            };
            let staged = self
                .stage_session_dsl_transition(
                    session_id,
                    crate::meerkat_machine::dsl::MeerkatMachineInput::StopRuntimeExecutor {
                        reason,
                    },
                    "StopRuntimeExecutor",
                )
                .await
                .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
            let projected_effect =
                crate::effect::runtime_effect_projection_from_dsl_effects(&staged.effects)
                    .map_err(RuntimeDriverError::Internal)?;

            let sessions = self.sessions.read().await;
            let entry = sessions
                .get(session_id)
                .ok_or(RuntimeDriverError::NotReady {
                    state: RuntimeState::Destroyed,
                })?;
            (
                entry.driver.clone(),
                entry.effect_sender(),
                projected_effect.into_effect(),
            )
        };

        if let Some(effect_tx) = effect_tx
            && effect_tx.send(effect).await.is_ok()
        {
            let stopped = tokio::time::timeout(std::time::Duration::from_millis(200), async {
                loop {
                    let state = {
                        let sessions = self.sessions.read().await;
                        let entry =
                            sessions
                                .get(session_id)
                                .ok_or(RuntimeDriverError::NotReady {
                                    state: RuntimeState::Destroyed,
                                })?;
                        if !Arc::ptr_eq(&entry.driver, &driver) {
                            return Err(RuntimeDriverError::NotReady {
                                state: RuntimeState::Destroyed,
                            });
                        }
                        entry.control_snapshot().phase
                    };
                    match state {
                        RuntimeState::Stopped => return Ok(()),
                        RuntimeState::Destroyed => {
                            return Err(RuntimeDriverError::NotReady {
                                state: RuntimeState::Destroyed,
                            });
                        }
                        _ => tokio::time::sleep(std::time::Duration::from_millis(10)).await,
                    }
                }
            })
            .await;
            match stopped {
                Ok(result) => result?,
                Err(_) => {
                    let authority = self
                        .session_dsl_authority(session_id)
                        .await
                        .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
                    let generated_stop_deferred = authority
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .state()
                        .runtime_stop_deferred;
                    if generated_stop_deferred {
                        return Ok(());
                    }
                    return Err(RuntimeDriverError::ValidationFailed {
                        reason: "StopRuntimeExecutor effect was accepted but generated authority did not reach stopped"
                            .to_string(),
                    });
                }
            }

            let _gate_guard = self
                .lock_current_session_driver_gate(session_id, &driver)
                .await?;
            let final_state = {
                let sessions = self.sessions.read().await;
                sessions
                    .get(session_id)
                    .ok_or(RuntimeDriverError::NotReady {
                        state: RuntimeState::Destroyed,
                    })?
                    .control_snapshot()
                    .phase
            };
            if !matches!(final_state, RuntimeState::Stopped) {
                return Err(RuntimeDriverError::ValidationFailed {
                    reason: format!(
                        "StopRuntimeExecutor effect completed without generated stopped authority: {final_state}"
                    ),
                });
            }

            return Ok(());
        }

        let (driver, _gate_guard) = self
            .current_session_driver_with_authority(session_id)
            .await?;
        let completions = {
            let sessions = self.sessions.read().await;
            sessions
                .get(session_id)
                .ok_or(RuntimeDriverError::NotReady {
                    state: RuntimeState::Destroyed,
                })?
                .completions
                .clone()
        };
        crate::control_plane::terminalize_async_stop(&driver, Some(&completions)).await?;

        // No live effect sender was available for this stop path. Scrub any
        // dead attachment capabilities that may still be published.
        self.clear_dead_runtime_attachment(session_id).await;
        Ok(())
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
                        failure: MeerkatMachineRunFailure::new(
                            meerkat_core::TurnTerminalCauseKind::FatalFailure,
                            err.to_string(),
                        ),
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
            Err(err) => Err(RuntimeBindingsError::PrepareFailed(
                session_id,
                err.to_string(),
            )),
        }
    }

    /// Prepare factory-consumable session runtime resources without emitting
    /// cross-machine binding signals.
    ///
    /// Mob provisioning uses this to pre-create the session-owned handle bundle
    /// before `MobMachine::Spawn` has committed the member runtime id. The
    /// authoritative mob binding is routed later through
    /// `RequestRuntimeBinding -> PrepareBindings`, which emits the typed
    /// `RuntimeBound` signal with the mob-owned `AgentRuntimeId` and fence.
    pub async fn prepare_local_session_bindings(
        &self,
        session_id: SessionId,
    ) -> Result<meerkat_core::SessionRuntimeBindings, RuntimeBindingsError> {
        match self
            .execute_meerkat_machine_command(
                None,
                MeerkatMachineCommand::PrepareLocalSessionBindings {
                    session_id: session_id.clone(),
                },
            )
            .await
        {
            Ok(MeerkatMachineCommandResult::Bindings(bindings)) => Ok(bindings),
            Ok(_) => {
                tracing::error!(
                    "prepare_local_session_bindings: unexpected command result variant"
                );
                Err(RuntimeBindingsError::SessionNotFound(session_id))
            }
            Err(_) => Err(RuntimeBindingsError::SessionNotFound(session_id)),
        }
    }
}
