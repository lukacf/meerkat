use super::*;
use crate::input_state::StoredInputState;
use crate::terminal_status::{
    self, InteractionSelector, Sourced, TerminalWitnessSource, interaction_report,
};
use meerkat_core::ToolName;

#[path = "../user_interrupt.rs"]
mod user_interrupt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SessionBindingPreparation {
    /// The runtime binding itself is the semantic event: apply
    /// `PrepareBindings` to MeerkatMachine and publish any routed seam signals.
    AuthoritativeRuntimeBinding,
    /// Only create the session-local handle bundle. A separate owner will
    /// route the authoritative binding later.
    LocalSessionResources,
}

struct RuntimeCompactionCommitCoordinator {
    session_id: SessionId,
    runtime_binding: std::sync::Mutex<
        Option<(
            crate::meerkat_machine::dsl::AgentRuntimeId,
            Option<crate::meerkat_machine::dsl::FenceToken>,
            Option<crate::meerkat_machine::dsl::Generation>,
        )>,
    >,
    runtime_epoch_id: crate::meerkat_machine::dsl::RuntimeEpochId,
    allow_late_binding: bool,
    dsl_authority: Arc<crate::handles::HandleDslAuthority>,
    store: Option<Arc<dyn crate::store::RuntimeStore>>,
}

struct RuntimeStickyModelFallbackCommitCoordinator {
    session_id: SessionId,
    store: Option<Arc<dyn crate::store::RuntimeStore>>,
}

struct RuntimeStickyModelFallbackCommitOperation {
    result_rx: crate::tokio::sync::watch::Receiver<
        Option<Result<(), meerkat_core::handles::StickyModelFallbackCommitError>>,
    >,
}

#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
impl meerkat_core::handles::StickyModelFallbackCommitOperation
    for RuntimeStickyModelFallbackCommitOperation
{
    async fn wait(&self) -> Result<(), meerkat_core::handles::StickyModelFallbackCommitError> {
        let mut result_rx = self.result_rx.clone();
        loop {
            if let Some(result) = result_rx.borrow().clone() {
                return result;
            }
            if result_rx.changed().await.is_err() {
                return Err(meerkat_core::handles::StickyModelFallbackCommitError::SupervisorLost);
            }
        }
    }
}

impl meerkat_core::handles::StickyModelFallbackCommitCoordinator
    for RuntimeStickyModelFallbackCommitCoordinator
{
    fn begin(
        &self,
        machine_commit: Box<dyn meerkat_core::handles::StickyModelFallbackMachineCommit>,
        control_delta: meerkat_core::handles::StickyModelFallbackControlDelta,
    ) -> Result<
        Arc<dyn meerkat_core::handles::StickyModelFallbackCommitOperation>,
        meerkat_core::handles::StickyModelFallbackCommitError,
    > {
        let Some(store) = self.store.clone() else {
            // Ephemeral runtimes have no recovery boundary to split. Consume
            // the generated one-shot commit synchronously and return the same
            // retained-result operation shape used by durable runtimes.
            let result = machine_commit
                .commit()
                .map_err(meerkat_core::handles::StickyModelFallbackCommitError::MachineRejected);
            let (_result_tx, result_rx) = crate::tokio::sync::watch::channel(Some(result));
            return Ok(Arc::new(RuntimeStickyModelFallbackCommitOperation {
                result_rx,
            }));
        };
        let session_id = self.session_id.clone();
        let (result_tx, result_rx) = crate::tokio::sync::watch::channel(None);
        crate::tokio::spawn(async move {
            let result =
                run_sticky_model_fallback_commit(session_id, store, machine_commit, control_delta)
                    .await;
            let _ = result_tx.send(Some(result));
        });
        Ok(Arc::new(RuntimeStickyModelFallbackCommitOperation {
            result_rx,
        }))
    }
}

async fn run_sticky_model_fallback_commit(
    session_id: SessionId,
    store: Arc<dyn crate::store::RuntimeStore>,
    machine_commit: Box<dyn meerkat_core::handles::StickyModelFallbackMachineCommit>,
    control_delta: meerkat_core::handles::StickyModelFallbackControlDelta,
) -> Result<(), meerkat_core::handles::StickyModelFallbackCommitError> {
    use meerkat_core::handles::StickyModelFallbackCommitError as CommitError;

    let runtime_id = crate::identifiers::LogicalRuntimeId::for_session(&session_id);
    let previous_snapshot = store
        .load_session_snapshot(&runtime_id)
        .await
        .map_err(|error| CommitError::Store(error.to_string()))?
        .ok_or_else(|| CommitError::SnapshotMissing {
            session_id: session_id.clone(),
        })?;
    let mut target_session: meerkat_core::Session = serde_json::from_slice(&previous_snapshot)
        .map_err(|error| CommitError::SnapshotInvalid(error.to_string()))?;
    if target_session.id() != &session_id {
        return Err(CommitError::SessionMismatch {
            expected: session_id,
            actual: target_session.id().clone(),
        });
    }
    control_delta
        .validate_and_apply(&mut target_session)
        .map_err(CommitError::InvalidControlDelta)?;
    let target_snapshot = serde_json::to_vec(&target_session)
        .map_err(|error| CommitError::SnapshotInvalid(error.to_string()))?;

    let cas_result = store
        .replace_session_snapshot_if_current(
            &runtime_id,
            &previous_snapshot,
            target_snapshot.clone(),
        )
        .await;
    if matches!(&cas_result, Ok(false)) {
        return Err(CommitError::SnapshotConflict);
    }
    if let Err(cas_error) = cas_result {
        let observed = store
            .load_session_snapshot(&runtime_id)
            .await
            .map_err(|read_error| {
                CommitError::SnapshotOutcomeUnknown(format!(
                    "compare-and-swap failed with '{cas_error}' and reconciliation read failed with '{read_error}'"
                ))
            })?;
        match observed {
            Some(observed) if observed == target_snapshot => {}
            Some(observed) if observed == previous_snapshot => {
                return Err(CommitError::Store(cas_error.to_string()));
            }
            _ => {
                return Err(CommitError::SnapshotOutcomeUnknown(cas_error.to_string()));
            }
        }
    }

    if let Err(machine_error) = machine_commit.commit() {
        let rollback_result = store
            .replace_session_snapshot_if_current(
                &runtime_id,
                &target_snapshot,
                previous_snapshot.clone(),
            )
            .await;
        if matches!(rollback_result, Ok(true)) {
            return Err(CommitError::MachineRejected(machine_error));
        }
        let observed = store.load_session_snapshot(&runtime_id).await;
        match observed {
            Ok(Some(observed)) if observed == previous_snapshot => {
                Err(CommitError::MachineRejected(machine_error))
            }
            Ok(Some(observed)) if observed == target_snapshot => {
                Err(CommitError::CompensationFailed(format!(
                    "{machine_error}; durable target snapshot remains committed"
                )))
            }
            Ok(Some(_)) => Err(CommitError::CompensationFailed(format!(
                "{machine_error}; a competing durable snapshot replaced the target during compensation"
            ))),
            Ok(None) => Err(CommitError::CompensationFailed(format!(
                "{machine_error}; durable snapshot disappeared during compensation"
            ))),
            Err(read_error) => Err(CommitError::CompensationFailed(format!(
                "{machine_error}; compensation result could not be reconciled: {read_error}"
            ))),
        }
    } else {
        Ok(())
    }
}

impl meerkat_core::memory::CompactionCommitCoordinator for RuntimeCompactionCommitCoordinator {
    fn authorize_projection(
        &self,
        projection: &meerkat_core::memory::CompactionProjectionId,
    ) -> Result<(), meerkat_core::memory::CompactionCommitCoordinationError> {
        if projection.session_id() != &self.session_id {
            return Err(
                meerkat_core::memory::CompactionCommitCoordinationError::SessionMismatch {
                    expected: self.session_id.clone(),
                    actual: projection.session_id().clone(),
                },
            );
        }
        let store = self.store.as_ref().ok_or_else(|| {
            meerkat_core::memory::CompactionCommitCoordinationError::Rejected(
                "runtime binding has no durable RuntimeStore".to_string(),
            )
        })?;
        if !store.supports_compaction_projection_outbox() {
            return Err(
                meerkat_core::memory::CompactionCommitCoordinationError::Rejected(
                    "runtime store does not support atomic compaction projection outbox"
                        .to_string(),
                ),
            );
        }
        let current = self
            .dsl_authority
            .current_runtime_binding(
                &crate::meerkat_machine::dsl::SessionId::from_domain(&self.session_id),
                &self.runtime_epoch_id,
                "RuntimeCompactionCommitCoordinator::authorize_projection",
            )
            .map_err(meerkat_core::memory::CompactionCommitCoordinationError::Rejected)?;
        let mut expected = self
            .runtime_binding
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match expected.as_ref() {
            Some(expected) if expected == &current => {}
            Some(expected) => {
                return Err(
                    meerkat_core::memory::CompactionCommitCoordinationError::Rejected(format!(
                        "runtime binding rotated (expected {expected:?}, current {current:?})"
                    )),
                );
            }
            None if self.allow_late_binding => {
                // Mob/local-resource construction precedes the routed
                // RequestRuntimeBinding transition. The first resultful
                // authorization latches the exact generated binding; later
                // fence/generation rotation then fails closed.
                *expected = Some(current);
            }
            None => {
                return Err(
                    meerkat_core::memory::CompactionCommitCoordinationError::Rejected(
                        "session resources do not carry an authoritative runtime binding"
                            .to_string(),
                    ),
                );
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod compaction_coordinator_tests {
    use super::*;
    use meerkat_core::memory::CompactionCommitCoordinator;

    fn projection(session_id: &SessionId) -> meerkat_core::CompactionProjectionId {
        serde_json::from_value(serde_json::json!({
            "session_id": session_id,
            "parent_revision": "parent",
            "revision": "revision",
            "commit_fingerprint": "sha256:coordinator-persisted-fixture",
        }))
        .expect("persisted compaction projection fixture")
    }

    #[test]
    fn local_binding_rejects_before_mob_bind_accepts_after_and_rejects_stale_epoch() {
        let session_id = SessionId::new();
        let dsl_session_id = crate::meerkat_machine::dsl::SessionId::from_domain(&session_id);
        let epoch_id = meerkat_core::RuntimeEpochId::new();
        let dsl_epoch_id = crate::meerkat_machine::dsl::RuntimeEpochId::from_domain(&epoch_id);
        let authority = Arc::new(std::sync::Mutex::new(
            crate::meerkat_machine::dsl::MeerkatMachineAuthority::new(),
        ));
        let teardown_gate = crate::handles::HandleTeardownGate::open();
        let handle = Arc::new(
            crate::handles::HandleDslAuthority::from_shared_with_teardown_gate(
                Arc::clone(&authority),
                Arc::clone(&teardown_gate),
            ),
        );
        let coordinator = RuntimeCompactionCommitCoordinator {
            session_id: session_id.clone(),
            runtime_binding: std::sync::Mutex::new(None),
            runtime_epoch_id: dsl_epoch_id.clone(),
            allow_late_binding: true,
            dsl_authority: Arc::clone(&handle),
            store: Some(Arc::new(crate::store::memory::InMemoryRuntimeStore::new())),
        };
        let projection = projection(&session_id);

        assert!(coordinator.authorize_projection(&projection).is_err());
        handle
            .apply_signal(
                crate::meerkat_machine::dsl::MeerkatMachineSignal::Initialize,
                "compaction_coordinator_test::initialize",
            )
            .expect("initialize machine");
        handle
            .apply_input(
                crate::meerkat_machine::dsl::MeerkatMachineInput::RegisterSession {
                    session_id: dsl_session_id.clone(),
                },
                "compaction_coordinator_test::register",
            )
            .expect("register session");
        handle
            .apply_input(
                crate::meerkat_machine::dsl::MeerkatMachineInput::PrepareBindings {
                    agent_runtime_id: crate::meerkat_machine::dsl::AgentRuntimeId::from(
                        "mob-runtime",
                    ),
                    fence_token: crate::meerkat_machine::dsl::FenceToken::from(7),
                    generation: Some(crate::meerkat_machine::dsl::Generation::from(3)),
                    runtime_epoch_id: Some(dsl_epoch_id),
                    session_id: dsl_session_id,
                },
                "compaction_coordinator_test::bind",
            )
            .expect("mob binding");
        coordinator
            .authorize_projection(&projection)
            .expect("late generated mob binding must be accepted and latched");

        {
            let mut guard = authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut rotated = guard.state().clone();
            rotated.active_fence_token = Some(crate::meerkat_machine::dsl::FenceToken::from(8));
            *guard =
                crate::meerkat_machine::dsl::MeerkatMachineAuthority::recover_from_state(rotated)
                    .expect("same-epoch rotated binding state");
        }
        assert!(
            coordinator.authorize_projection(&projection).is_err(),
            "a same-gate fence rotation must not be accepted by the latched coordinator"
        );

        // Restore the originally latched facts to distinguish epoch teardown
        // rejection from the binding-rotation assertion above.
        {
            let mut guard = authority
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut restored = guard.state().clone();
            restored.active_fence_token = Some(crate::meerkat_machine::dsl::FenceToken::from(7));
            *guard =
                crate::meerkat_machine::dsl::MeerkatMachineAuthority::recover_from_state(restored)
                    .expect("restored binding state");
        }
        coordinator
            .authorize_projection(&projection)
            .expect("restored exact binding must match the latch");

        teardown_gate.close();
        assert!(
            coordinator.authorize_projection(&projection).is_err(),
            "a coordinator from the torn-down epoch must fail closed"
        );
    }
}

fn visibility_authorities_for_names(
    names: &std::collections::BTreeSet<ToolName>,
    witnesses: &std::collections::BTreeMap<ToolName, meerkat_core::ToolVisibilityWitness>,
) -> std::collections::BTreeMap<ToolName, crate::meerkat_machine::dsl::ToolVisibilityWitness> {
    names
        .iter()
        .filter_map(|name| {
            witnesses.get(name).map(|witness| {
                (
                    name.clone(),
                    crate::meerkat_machine::dsl::ToolVisibilityWitness::from(witness),
                )
            })
        })
        .collect()
}

impl MeerkatMachine {
    async fn dispatch_user_interrupt(
        &self,
        session_id: &SessionId,
        reason: String,
    ) -> Result<(), RuntimeDriverError> {
        let gate = self.session_mutation_gate(session_id).await;
        let _gate_guard = match gate {
            Some(g) => match Arc::clone(&g).try_lock_owned() {
                Ok(guard) => Some(guard),
                Err(_) if self.generated_stop_deferred(session_id).await => None,
                Err(_) => Some(g.lock_owned().await),
            },
            None => None,
        };

        let staged_interrupt = match self
            .stage_session_runtime_internal_dsl_transition(
                session_id,
                crate::meerkat_machine_types::MeerkatMachineFieldlessRuntimeInternalInput::InterruptCurrentRun,
            )
            .await
        {
            Ok(state) => state,
            Err(_) => {
                // The generated machine rejected `InterruptCurrentRun` for the
                // current phase. Surface the terminal `Destroyed` truth as its
                // own typed variant (DestroyedShapeInvariant) so callers that
                // distinguish a destroyed binding from a merely not-ready one
                // still observe it; every other rejected phase is `NotReady`.
                let state = self
                    .existing_session_runtime_state(session_id)
                    .await
                    .unwrap_or(RuntimeState::Destroyed);
                if state == RuntimeState::Destroyed {
                    return Err(RuntimeDriverError::Destroyed);
                }
                return Err(RuntimeDriverError::NotReady { state });
            }
        };

        if let Err(err) = self
            .apply_user_interrupt_live_cancel(session_id, reason)
            .await
        {
            self.restore_session_dsl_state_if_current(
                session_id,
                staged_interrupt.committed_snapshot,
                staged_interrupt.previous_snapshot,
            )
            .await;
            return Err(err);
        }

        Ok(())
    }

    /// Classify a generated-machine rejection of a session lifecycle input.
    ///
    /// The machine already made the legality decision (stage-first shape, same
    /// as `dispatch_user_interrupt`); this only maps the rejection onto the
    /// typed wire error: a `Destroyed` binding surfaces as the terminal
    /// [`RuntimeDriverError::Destroyed`], every other rejection keeps its
    /// reason as `ValidationFailed`. Reading the runtime state here is a
    /// post-verdict projection read for classification, never a guard.
    pub(super) async fn classify_session_dsl_rejection(
        &self,
        session_id: &SessionId,
        reason: String,
    ) -> RuntimeDriverError {
        if matches!(
            self.existing_session_runtime_state(session_id).await,
            Some(RuntimeState::Destroyed)
        ) {
            return RuntimeDriverError::Destroyed;
        }
        RuntimeDriverError::ValidationFailed { reason }
    }

    /// Same stage-first classification for lifecycle errors that already carry
    /// a typed [`RuntimeDriverError`]: a rejection observed on a `Destroyed`
    /// binding is surfaced as the terminal `Destroyed` truth; everything else
    /// propagates unchanged.
    pub(super) async fn classify_session_driver_rejection(
        &self,
        session_id: &SessionId,
        err: RuntimeDriverError,
    ) -> RuntimeDriverError {
        if matches!(
            self.existing_session_runtime_state(session_id).await,
            Some(RuntimeState::Destroyed)
        ) {
            return RuntimeDriverError::Destroyed;
        }
        err
    }

    async fn generated_stop_deferred(&self, session_id: &SessionId) -> bool {
        let Ok(authority) = self.session_dsl_authority(session_id).await else {
            return false;
        };
        authority
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .state()
            .runtime_stop_deferred
    }

    pub(super) async fn prepare_session_runtime_bindings(
        &self,
        session_id: SessionId,
        preparation: SessionBindingPreparation,
    ) -> Result<MeerkatMachineCommandResult, RuntimeDriverError> {
        tracing::debug!(
            %session_id,
            ?preparation,
            "MeerkatMachine::prepare_session_runtime_bindings start"
        );
        tracing::debug!(
            %session_id,
            ?preparation,
            "MeerkatMachine::prepare_session_runtime_bindings registering session"
        );
        #[cfg(target_arch = "wasm32")]
        let inserted_by_call = if self.store.is_none() {
            {
                tracing::debug!(%session_id, "MeerkatMachine::prepare_session_runtime_bindings attempting storeless existing check lock");
                let mut sessions = self.sessions.try_write().map_err(|_| {
                    tracing::warn!(
                        %session_id,
                        "storeless session map busy while checking existing registration"
                    );
                    RuntimeDriverError::Internal(format!(
                        "storeless session map busy while registering {session_id}"
                    ))
                })?;
                tracing::debug!(%session_id, "MeerkatMachine::prepare_session_runtime_bindings locked storeless existing check");
                if let Some(existing) = sessions.get_mut(&session_id) {
                    tracing::debug!(
                        %session_id,
                        "MeerkatMachine::prepare_session_runtime_bindings found existing session"
                    );
                    if existing.clear_dead_attachment() {
                        existing.stage_generated_executor_exit_observation().map_err(|reason| {
                            RuntimeDriverError::Internal(format!(
                                "generated MeerkatMachine rejected executor-exit observation: {reason}"
                            ))
                        })?;
                    }
                    false
                } else {
                    drop(sessions);
                    self.register_storeless_session_inner_sync_build_step(session_id.clone())?
                }
            }
        } else {
            Box::pin(self.register_session_inner(session_id.clone())).await?
        };
        #[cfg(not(target_arch = "wasm32"))]
        let inserted_by_call = Box::pin(self.register_session_inner(session_id.clone())).await?;
        let registration_gate_guard = self.lock_registration_gate(&session_id).await?;
        tracing::debug!(
            %session_id,
            inserted_by_call,
            ?preparation,
            "MeerkatMachine::prepare_session_runtime_bindings registered session"
        );
        let (
            driver_handle,
            epoch_id,
            ops_lifecycle,
            cursor_state,
            tool_visibility_owner,
            dsl_authority_shared,
            handle_teardown_gate,
        ) = {
            let sessions = self.sessions.read().await;
            let entry = sessions
                .get(&session_id)
                .ok_or(RuntimeDriverError::Internal(format!(
                    "session {session_id} missing after register_session_inner"
                )))?;
            (
                Arc::clone(&entry.driver),
                entry.epoch_id.clone(),
                Arc::clone(&entry.ops_lifecycle),
                Arc::clone(&entry.cursor_state),
                Arc::clone(&entry.tool_visibility_owner),
                Arc::clone(&entry.dsl_authority),
                Arc::clone(&entry.handle_teardown_gate),
            )
        };
        let dsl_session_id = crate::meerkat_machine::dsl::SessionId::from_domain(&session_id);
        let terminal_supervisor_cleanup_bindings = matches!(
            self.existing_session_runtime_state(&session_id).await,
            Some(RuntimeState::Destroyed)
        ) && self
            .has_terminal_supervisor_cleanup_authority(&session_id)
            .await;
        // Stage RegisterSession unconditionally: the generated machine owns
        // the idempotence verdict (`RegisterSessionIdempotent` no-ops a
        // same-binding re-registration), the revival verdict
        // (`RegisterSessionResumesStopped` re-admits a stopped session to
        // Idle), and the Destroyed rejection (RegisterSession is not declared
        // from Destroyed). No shell probe of the authority state precedes the
        // staging.
        let registration_input = if terminal_supervisor_cleanup_bindings {
            crate::meerkat_machine::dsl::MeerkatMachineInput::PrepareTerminalSupervisorCleanupBindings {
                session_id: dsl_session_id,
            }
        } else {
            crate::meerkat_machine::dsl::MeerkatMachineInput::RegisterSession {
                session_id: dsl_session_id,
            }
        };
        match self
            .stage_session_dsl_transition(
                &session_id,
                registration_input,
                if terminal_supervisor_cleanup_bindings {
                    "PrepareTerminalSupervisorCleanupBindings"
                } else {
                    "RegisterSession"
                },
            )
            .await
        {
            Ok(staged) => {
                if staged.revived_stopped_session() {
                    // Machine-emitted revival: refresh the durable lifecycle
                    // record so cross-process readers never observe a stale
                    // `Stopped` snapshot for a revived session.
                    if let Err(err) = driver_handle
                        .lock()
                        .await
                        .persist_current_machine_lifecycle("resume")
                        .await
                    {
                        drop(registration_gate_guard);
                        return Err(if inserted_by_call {
                            self.compensate_inserted_session_error(
                                &session_id,
                                &epoch_id,
                                err,
                                "failed lifecycle persistence after session revival",
                            )
                            .await
                        } else {
                            err
                        });
                    }
                }
            }
            Err(reason) => {
                let err = self
                    .classify_session_dsl_rejection(&session_id, reason)
                    .await;
                drop(registration_gate_guard);
                return Err(if inserted_by_call {
                    self.compensate_inserted_session_error(
                        &session_id,
                        &epoch_id,
                        err,
                        "generated session registration rejection",
                    )
                    .await
                } else {
                    err
                });
            }
        }
        tracing::debug!(
            %session_id,
            ?preparation,
            "MeerkatMachine::prepare_session_runtime_bindings prepared generated registration"
        );
        let compaction_runtime_epoch_id =
            crate::meerkat_machine::dsl::RuntimeEpochId::from_domain(&epoch_id);
        let allow_late_compaction_binding = !terminal_supervisor_cleanup_bindings
            && preparation == SessionBindingPreparation::LocalSessionResources;
        let compaction_runtime_binding = if terminal_supervisor_cleanup_bindings {
            tracing::debug!(
                %session_id,
                "preserving Destroyed lifecycle while installing terminal supervisor cleanup handles"
            );
            None
        } else if preparation == SessionBindingPreparation::AuthoritativeRuntimeBinding {
            let runtime_id = {
                tracing::debug!(
                    %session_id,
                    ?preparation,
                    "MeerkatMachine::prepare_session_runtime_bindings locking driver for runtime id"
                );
                let driver = driver_handle.lock().await;
                driver.runtime_id().clone()
            };
            tracing::debug!(
                %session_id,
                ?preparation,
                "MeerkatMachine::prepare_session_runtime_bindings locked driver for runtime id"
            );
            let agent_runtime_id =
                crate::meerkat_machine::dsl::AgentRuntimeId::from_domain(&runtime_id);
            let fence_token = crate::meerkat_machine::dsl::FenceToken::from(0);
            let dsl_input = crate::meerkat_machine::dsl::MeerkatMachineInput::PrepareBindings {
                agent_runtime_id: agent_runtime_id.clone(),
                fence_token,
                generation: None,
                runtime_epoch_id: Some(compaction_runtime_epoch_id.clone()),
                session_id: crate::meerkat_machine::dsl::SessionId::from_domain(&session_id),
            };
            let staged = match self
                .stage_session_dsl_transition(&session_id, dsl_input, "PrepareBindings")
                .await
            {
                Ok(staged) => staged,
                Err(reason) => {
                    let err = RuntimeDriverError::ValidationFailed { reason };
                    drop(registration_gate_guard);
                    return Err(if inserted_by_call {
                        self.compensate_inserted_session_error(
                            &session_id,
                            &epoch_id,
                            err,
                            "binding preparation rejection",
                        )
                        .await
                    } else {
                        err
                    });
                }
            };
            {
                tracing::debug!(
                    %session_id,
                    ?preparation,
                    "MeerkatMachine::prepare_session_runtime_bindings locking driver for authoritative projection"
                );
                let mut driver = driver_handle.lock().await;
                machine_prepare_bindings_projection(&mut driver);
            }
            tracing::debug!(
                %session_id,
                ?preparation,
                "MeerkatMachine::prepare_session_runtime_bindings applied authoritative projection"
            );
            if let Err(reason) = self
                .commit_session_dsl_transition(&session_id, staged, "PrepareBindings")
                .await
            {
                driver_handle
                    .lock()
                    .await
                    .sync_control_projection_from_dsl_authority();
                let err = RuntimeDriverError::Internal(reason);
                drop(registration_gate_guard);
                return Err(if inserted_by_call {
                    self.compensate_inserted_session_error(
                        &session_id,
                        &epoch_id,
                        err,
                        "binding preparation commit failure",
                    )
                    .await
                } else {
                    err
                });
            }
            Some((agent_runtime_id, Some(fence_token), None))
        } else {
            {
                tracing::debug!(
                    %session_id,
                    ?preparation,
                    "MeerkatMachine::prepare_session_runtime_bindings locking driver for local projection"
                );
                let mut driver = driver_handle.lock().await;
                machine_prepare_bindings_projection(&mut driver);
            }
            tracing::debug!(
                %session_id,
                ?preparation,
                "MeerkatMachine::prepare_session_runtime_bindings applied local projection"
            );
            None
        };
        // Share ONE HandleDslAuthority across all 5 handles so their
        // transitions land on the session's real DSL state (same Arc
        // as RuntimeSessionEntry.dsl_authority). Phase 5F/1-5 callsites
        // rely on this — parallel private authorities would silently
        // diverge from the session DSL.
        let shared_handle_authority = Arc::new(
            crate::handles::HandleDslAuthority::from_shared_with_teardown_gate(
                dsl_authority_shared,
                handle_teardown_gate,
            ),
        );
        let auth_lease = self.generated_auth_lease_handle();
        let runtime_authority = match preparation {
            SessionBindingPreparation::AuthoritativeRuntimeBinding => {
                crate::session_runtime_bindings_authority()
            }
            SessionBindingPreparation::LocalSessionResources => {
                crate::local_session_runtime_bindings_authority()
            }
        };
        let peer_comms_install = crate::handles::RuntimePeerCommsHandle::generated_install_factory(
            Arc::clone(&shared_handle_authority),
        )
        .map_err(RuntimeDriverError::Internal)?;

        tracing::debug!(
            %session_id,
            ?preparation,
            "MeerkatMachine::prepare_session_runtime_bindings assembling bindings"
        );
        Ok(MeerkatMachineCommandResult::Bindings(
            meerkat_core::SessionRuntimeBindings::__from_runtime_authority(
                session_id.clone(),
                epoch_id,
                ops_lifecycle as Arc<dyn meerkat_core::OpsLifecycleRegistry>,
                cursor_state,
                generated_tool_visibility_owner(Arc::clone(&tool_visibility_owner)
                    as Arc<dyn meerkat_core::ToolVisibilityOwner>)
                .map_err(RuntimeDriverError::Internal)?,
                Arc::new(crate::handles::RuntimeTurnStateHandle::new(Arc::clone(
                    &shared_handle_authority,
                ))),
                Arc::new(crate::handles::RuntimeCommsDrainHandle::new(Arc::clone(
                    &shared_handle_authority,
                ))),
                Arc::new(crate::handles::RuntimeExternalToolSurfaceHandle::new(
                    Arc::clone(&shared_handle_authority),
                )),
                peer_comms_install,
                Arc::new(crate::handles::RuntimeSessionAdmissionHandle::new(
                    Arc::clone(&shared_handle_authority),
                )),
                Arc::new(
                    crate::handles::RuntimeModelRoutingHandle::new_with_visibility_owner(
                        Arc::clone(&shared_handle_authority),
                        Arc::clone(&tool_visibility_owner),
                    ),
                ),
                Arc::new(RuntimeStickyModelFallbackCommitCoordinator {
                    session_id: session_id.clone(),
                    store: self.store.clone(),
                }),
                auth_lease,
                Arc::new(crate::handles::RuntimeMcpServerLifecycleHandle::new(
                    Arc::clone(&shared_handle_authority),
                )),
                Arc::new(crate::handles::RuntimePeerInteractionHandle::new(
                    Arc::clone(&shared_handle_authority),
                )),
                Arc::new(crate::handles::RuntimeSessionContextHandle::new(
                    Arc::clone(&shared_handle_authority),
                )),
                self.session_claim_handle(),
                Arc::new(crate::handles::RuntimeInteractionStreamHandle::new(
                    Arc::clone(&shared_handle_authority),
                )),
                Arc::new(RuntimeCompactionCommitCoordinator {
                    session_id: session_id.clone(),
                    runtime_binding: std::sync::Mutex::new(compaction_runtime_binding),
                    runtime_epoch_id: compaction_runtime_epoch_id,
                    allow_late_binding: allow_late_compaction_binding,
                    dsl_authority: Arc::clone(&shared_handle_authority),
                    store: self.store.clone(),
                }),
                runtime_authority,
            ),
        ))
    }

    /// Durable input-state witnesses for an UNREGISTERED session.
    ///
    /// Persistent machines answer from the RuntimeStore rows committed
    /// atomically at every machine lifecycle boundary (rows are never
    /// deleted, so terminal facts persist). A session with no lifecycle
    /// record AND no input rows was never admitted and fails typed
    /// `NotFound` — never an empty success. Ephemeral machines keep the
    /// existing `NotReady` class: with no store there is no durable witness
    /// to answer from.
    pub(super) async fn durable_session_input_witnesses(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<StoredInputState>, RuntimeDriverError> {
        let Some(store) = self.store.as_ref() else {
            return Err(RuntimeDriverError::NotReady {
                state: RuntimeState::Destroyed,
            });
        };
        let runtime_id = Self::logical_runtime_id(session_id);
        let witnesses = store.load_input_states(&runtime_id).await.map_err(|err| {
            RuntimeDriverError::Internal(format!(
                "terminal-status witness read failed for {runtime_id}: {err}"
            ))
        })?;
        if witnesses.is_empty() {
            let lifecycle = store
                .load_machine_lifecycle_record(&runtime_id)
                .await
                .map_err(|err| {
                    RuntimeDriverError::Internal(format!(
                        "terminal-status lifecycle read failed for {runtime_id}: {err}"
                    ))
                })?;
            if lifecycle.is_none() {
                return Err(RuntimeDriverError::NotFound { runtime_id });
            }
        }
        Ok(witnesses)
    }

    /// Input-state witnesses for a session: the live DSL-backed snapshot when
    /// the session is registered, the durable store rows otherwise. Both
    /// sides feed the same pure evaluators in [`crate::terminal_status`].
    pub(super) async fn session_input_witnesses(
        &self,
        session_id: &SessionId,
    ) -> Result<(TerminalWitnessSource, Vec<StoredInputState>), RuntimeDriverError> {
        let driver = {
            let sessions = self.sessions.read().await;
            sessions.get(session_id).map(|entry| entry.driver.clone())
        };
        if let Some(driver) = driver {
            let driver = driver.lock().await;
            return Ok((
                TerminalWitnessSource::LiveRuntime,
                driver.as_driver().stored_input_states_snapshot()?,
            ));
        }
        Ok((
            TerminalWitnessSource::DurableStore,
            self.durable_session_input_witnesses(session_id).await?,
        ))
    }

    pub(super) async fn execute_meerkat_machine_session_command(
        &self,
        command: MeerkatMachineCommand,
    ) -> Result<MeerkatMachineCommandResult, RuntimeDriverError> {
        match command {
            MeerkatMachineCommand::RegisterSession { session_id } => {
                let sid = session_id.clone();
                self.register_session_inner(session_id).await?;
                let _registration_gate_guard = self.lock_registration_gate(&sid).await?;
                // Stage-first: the generated machine owns the legality verdict.
                // RegisterSession is not declared from Destroyed (it is a
                // resurrection input the DestroyedShapeInvariant forbids), so a
                // resident OR cold-recovered Destroyed binding is rejected by
                // the machine and classified as the terminal `Destroyed` truth
                // — never silently skipped, never preflighted in the shell. A
                // same-binding re-registration is the machine-owned
                // `RegisterSessionIdempotent` no-op.
                if let Err(reason) = self
                    .stage_session_dsl_input(
                        &sid,
                        crate::meerkat_machine::dsl::MeerkatMachineInput::RegisterSession {
                            session_id: crate::meerkat_machine::dsl::SessionId::from_domain(&sid),
                        },
                        "RegisterSession",
                    )
                    .await
                {
                    return Err(self.classify_session_dsl_rejection(&sid, reason).await);
                }
                Ok(MeerkatMachineCommandResult::Unit)
            }
            MeerkatMachineCommand::UnregisterSession { session_id } => {
                if !self.sessions.read().await.contains_key(&session_id) {
                    return Err(RuntimeDriverError::NotReady {
                        state: RuntimeState::Destroyed,
                    });
                }
                self.unregister_session_inner(&session_id).await?;
                Ok(MeerkatMachineCommandResult::Unit)
            }
            MeerkatMachineCommand::SetSilentIntents {
                session_id,
                intents,
            } => {
                let gate = self.session_mutation_gate(&session_id).await;
                let _gate_guard = match gate {
                    Some(ref g) => Some(g.lock().await),
                    None => None,
                };

                // Stage-first: SetSilentIntents is not declared from Destroyed,
                // so the machine rejects it there and the rejection is
                // classified as the terminal `Destroyed` truth.
                if let Err(reason) = self
                    .stage_session_dsl_input(
                        &session_id,
                        crate::meerkat_machine::dsl::MeerkatMachineInput::SetSilentIntents {
                            session_id: crate::meerkat_machine::dsl::SessionId::from_domain(
                                &session_id,
                            ),
                            intents: intents.into_iter().collect(),
                        },
                        "SetSilentIntents",
                    )
                    .await
                {
                    return Err(self
                        .classify_session_dsl_rejection(&session_id, reason)
                        .await);
                }
                Ok(MeerkatMachineCommandResult::Unit)
            }
            MeerkatMachineCommand::CancelAfterBoundary { session_id } => {
                // Stage-first: `cancel_after_boundary_inner` stages the
                // CancelAfterBoundary DSL input; the machine rejects it on a
                // Destroyed binding and the inner classification surfaces the
                // terminal `Destroyed` truth.
                self.cancel_after_boundary_inner(&session_id).await?;
                Ok(MeerkatMachineCommandResult::Unit)
            }
            MeerkatMachineCommand::StopRuntimeExecutor { session_id, reason } => {
                // Stage-first: `stop_runtime_executor_inner` stages the
                // StopRuntimeExecutor DSL input; the machine rejects it on a
                // Destroyed binding and the inner classification surfaces the
                // terminal `Destroyed` truth.
                self.stop_runtime_executor_inner(&session_id, reason)
                    .await?;
                Ok(MeerkatMachineCommandResult::Unit)
            }
            MeerkatMachineCommand::CommitServiceTurnTerminalReceipt { session_id } => {
                // Direct SessionService turns share the runtime turn-state
                // handle, but their durable commit occurs inside
                // `SessionService::start_turn`, not the runtime loop. After
                // that call returns successfully, close the run binding here
                // through the same machine-owned lifecycle authority.
                let gate = self.session_mutation_gate(&session_id).await;
                let _gate_guard = match gate {
                    Some(ref g) => Some(g.lock().await),
                    None => None,
                };
                let driver = {
                    let sessions = self.sessions.read().await;
                    sessions
                        .get(&session_id)
                        .ok_or(RuntimeDriverError::NotReady {
                            state: RuntimeState::Destroyed,
                        })?
                        .driver
                        .clone()
                };
                let receipt_result = {
                    let mut driver = driver.lock().await;
                    machine_commit_service_turn_terminal_receipt(&mut driver).await
                };
                // The driver-level receipt requires a Running machine-owned
                // lifecycle (it rejects every other phase, including
                // Destroyed); classify a rejection observed on a Destroyed
                // binding as the terminal `Destroyed` truth.
                if let Err(err) = receipt_result {
                    return Err(self
                        .classify_session_driver_rejection(&session_id, err)
                        .await);
                }
                Ok(MeerkatMachineCommandResult::Unit)
            }
            MeerkatMachineCommand::ContainsSession { session_id } => {
                Ok(MeerkatMachineCommandResult::Bool(
                    self.sessions.read().await.contains_key(&session_id),
                ))
            }
            MeerkatMachineCommand::SessionHasExecutor { session_id } => {
                let sessions = self.sessions.read().await;
                Ok(MeerkatMachineCommandResult::Bool(
                    sessions
                        .get(&session_id)
                        .map(RuntimeSessionEntry::generated_executor_registration_active)
                        .unwrap_or(false),
                ))
            }
            MeerkatMachineCommand::SessionHasComms { session_id } => {
                let engaged = self
                    .drain_authority_state(&session_id)
                    .await
                    .is_some_and(|state| {
                        state.peer_owner_kind
                            != crate::meerkat_machine::dsl::PeerIngressOwnerKind::Unattached
                    });
                Ok(MeerkatMachineCommandResult::Bool(engaged))
            }
            MeerkatMachineCommand::OpsLifecycleRegistry { session_id } => {
                let sessions = self.sessions.read().await;
                Ok(MeerkatMachineCommandResult::OpsLifecycleRegistry(
                    sessions
                        .get(&session_id)
                        .map(|e| Arc::clone(&e.ops_lifecycle)),
                ))
            }
            MeerkatMachineCommand::PrepareBindings { session_id } => {
                Box::pin(self.prepare_session_runtime_bindings(
                    session_id,
                    SessionBindingPreparation::AuthoritativeRuntimeBinding,
                ))
                .await
            }
            MeerkatMachineCommand::PrepareLocalSessionBindings { session_id } => {
                Box::pin(self.prepare_session_runtime_bindings(
                    session_id,
                    SessionBindingPreparation::LocalSessionResources,
                ))
                .await
            }
            MeerkatMachineCommand::InputState {
                session_id,
                input_id,
            } => {
                let driver = {
                    let sessions = self.sessions.read().await;
                    sessions.get(&session_id).map(|entry| entry.driver.clone())
                };
                match driver {
                    Some(driver) => {
                        let driver = driver.lock().await;
                        Ok(MeerkatMachineCommandResult::InputState(
                            driver.as_driver().stored_input_state(&input_id),
                        ))
                    }
                    // Restart-first-class fallback: an unregistered session on
                    // a persistent machine answers from the durable
                    // RuntimeStore witnesses without reviving the runtime.
                    None => {
                        let witnesses = self.durable_session_input_witnesses(&session_id).await?;
                        Ok(MeerkatMachineCommandResult::InputState(
                            witnesses
                                .into_iter()
                                .find(|stored| stored.state.input_id == input_id),
                        ))
                    }
                }
            }
            MeerkatMachineCommand::InputStateByIdempotencyKey {
                session_id,
                idempotency_key,
            } => {
                let driver = {
                    let sessions = self.sessions.read().await;
                    sessions.get(&session_id).map(|entry| entry.driver.clone())
                };
                match driver {
                    Some(driver) => {
                        let driver = driver.lock().await;
                        let driver = driver.as_driver();
                        Ok(MeerkatMachineCommandResult::InputState(
                            driver
                                .input_id_for_idempotency_key(&idempotency_key)
                                .and_then(|input_id| driver.stored_input_state(&input_id)),
                        ))
                    }
                    // Restart-first-class fallback: the persisted shell key is
                    // the exact fact recovery re-enters as the machine-owned
                    // idempotency binding, so the durable witness and the live
                    // admission map cannot diverge.
                    None => {
                        let witnesses = self.durable_session_input_witnesses(&session_id).await?;
                        Ok(MeerkatMachineCommandResult::InputState(
                            terminal_status::find_by_idempotency_key(&witnesses, &idempotency_key)
                                .cloned(),
                        ))
                    }
                }
            }
            MeerkatMachineCommand::InteractionTerminalStatus {
                session_id,
                selector,
            } => {
                let driver = {
                    let sessions = self.sessions.read().await;
                    sessions.get(&session_id).map(|entry| entry.driver.clone())
                };
                let sourced = match driver {
                    Some(driver) => {
                        let driver = driver.lock().await;
                        let driver = driver.as_driver();
                        let bundle = match &selector {
                            InteractionSelector::InputId(input_id) => {
                                driver.stored_input_state(input_id)
                            }
                            // Live path: the machine-owned admission map is
                            // the authority for key -> input resolution.
                            InteractionSelector::IdempotencyKey(key) => driver
                                .input_id_for_idempotency_key(key)
                                .and_then(|input_id| driver.stored_input_state(&input_id)),
                        };
                        bundle.map(|bundle| Sourced {
                            source: TerminalWitnessSource::LiveRuntime,
                            report: interaction_report(&bundle),
                        })
                    }
                    None => {
                        let witnesses = self.durable_session_input_witnesses(&session_id).await?;
                        let bundle = match &selector {
                            InteractionSelector::InputId(input_id) => witnesses
                                .iter()
                                .find(|stored| &stored.state.input_id == input_id),
                            InteractionSelector::IdempotencyKey(key) => {
                                terminal_status::find_by_idempotency_key(&witnesses, key)
                            }
                        };
                        bundle.map(|bundle| Sourced {
                            source: TerminalWitnessSource::DurableStore,
                            report: interaction_report(bundle),
                        })
                    }
                };
                Ok(MeerkatMachineCommandResult::InteractionTerminalStatus(
                    sourced,
                ))
            }
            MeerkatMachineCommand::RunTerminalStatus { session_id, run_id } => {
                let (source, witnesses) = self.session_input_witnesses(&session_id).await?;
                Ok(MeerkatMachineCommandResult::RunTerminalStatus(Sourced {
                    source,
                    report: terminal_status::evaluate_run(&run_id, &witnesses),
                }))
            }
            MeerkatMachineCommand::ListActiveInputs { session_id } => {
                let driver = {
                    let sessions = self.sessions.read().await;
                    let entry = sessions
                        .get(&session_id)
                        .ok_or(RuntimeDriverError::NotReady {
                            state: RuntimeState::Destroyed,
                        })?;
                    entry.driver.clone()
                };
                let driver = driver.lock().await;
                Ok(MeerkatMachineCommandResult::ActiveInputs(
                    driver.as_driver().active_input_ids(),
                ))
            }
            MeerkatMachineCommand::ReconfigureSessionLlmIdentity {
                session_id,
                previous_identity,
                previous_visibility_state,
                previous_capability_surface,
                previous_capability_surface_status,
                view_image_tool_available,
                previous_view_image_visible,
                next_view_image_visible,
                previous_active_visibility_revision,
                previous_staged_visibility_revision,
                target_identity,
                target_capability_surface,
                next_visibility_state,
                next_capability_base_filter,
                next_active_visibility_revision,
                tool_visibility_delta,
            } => {
                let gate = self.session_mutation_gate(&session_id).await;
                let _gate_guard = match gate {
                    Some(ref g) => Some(g.lock().await),
                    None => None,
                };

                use crate::meerkat_machine::dsl as mm_dsl;
                let dsl_previous_identity =
                    mm_dsl::SessionLlmIdentity::from_domain(previous_identity.as_ref());
                let dsl_previous_visibility_state = mm_dsl::SessionToolVisibilityState::from_domain(
                    previous_visibility_state.as_ref(),
                );
                let dsl_previous_capability_surface = previous_capability_surface
                    .as_ref()
                    .map(mm_dsl::SessionLlmCapabilitySurface::from_domain);
                let dsl_previous_capability_surface_status =
                    mm_dsl::SessionLlmCapabilitySurfaceStatus::from_domain(
                        &previous_capability_surface_status,
                    );
                let dsl_previous_capability_base_filter = mm_dsl::ToolFilter::from_domain(
                    &previous_visibility_state.capability_base_filter,
                );
                let dsl_target_identity =
                    mm_dsl::SessionLlmIdentity::from_domain(target_identity.as_ref());
                let dsl_target_capability_surface =
                    mm_dsl::SessionLlmCapabilitySurface::from_domain(&target_capability_surface);
                let dsl_next_visibility_state =
                    mm_dsl::SessionToolVisibilityState::from_domain(next_visibility_state.as_ref());
                let dsl_next_capability_base_filter =
                    mm_dsl::ToolFilter::from_domain(&next_capability_base_filter);
                let dsl_tool_visibility_delta =
                    mm_dsl::SessionToolVisibilityDelta::from_domain(tool_visibility_delta.as_ref());

                let staged_dsl_input = self
                    .stage_session_dsl_transition(
                        &session_id,
                        crate::meerkat_machine::dsl::MeerkatMachineInput::ReconfigureSessionLlmIdentity {
                            previous_identity: dsl_previous_identity,
                            previous_visibility_state: dsl_previous_visibility_state,
                            previous_capability_surface: dsl_previous_capability_surface,
                            previous_capability_surface_status:
                                dsl_previous_capability_surface_status,
                            previous_capability_base_filter: dsl_previous_capability_base_filter,
                            view_image_tool_available,
                            previous_view_image_visible,
                            next_view_image_visible,
                            previous_active_visibility_revision,
                            previous_staged_visibility_revision,
                            target_identity: dsl_target_identity,
                            target_capability_surface: dsl_target_capability_surface,
                            next_visibility_state: dsl_next_visibility_state,
                            next_capability_base_filter: dsl_next_capability_base_filter,
                            next_active_visibility_revision,
                            tool_visibility_delta: dsl_tool_visibility_delta,
                        },
                        "ReconfigureSessionLlmIdentity",
                    )
                    .await
                    .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
                let authority_plan =
                    Self::session_llm_reconfigure_authority_plan(&staged_dsl_input.effects)?;
                let report = match self
                    .reconfigure_session_llm_identity_inner(
                        &session_id,
                        *previous_identity,
                        *previous_visibility_state,
                        *target_identity,
                        *next_visibility_state,
                        authority_plan,
                    )
                    .await
                {
                    Ok(report) => report,
                    Err(err) => {
                        self.restore_session_dsl_state(
                            &session_id,
                            staged_dsl_input.previous_snapshot,
                        )
                        .await;
                        if err.clear_generated_llm_state {
                            self.stage_session_dsl_input(
                                &session_id,
                                crate::meerkat_machine::dsl::MeerkatMachineInput::ClearSessionLlmState,
                                "ClearSessionLlmState",
                            )
                            .await
                            .map_err(|reason| RuntimeDriverError::ValidationFailed { reason })?;
                        }
                        return Err(err.error);
                    }
                };
                Ok(MeerkatMachineCommandResult::LlmReconfigured(report))
            }
            MeerkatMachineCommand::StagePersistentFilter {
                session_id,
                filter,
                witnesses,
            } => {
                if !self.sessions.read().await.contains_key(&session_id) {
                    return Err(RuntimeDriverError::NotReady {
                        state: RuntimeState::Destroyed,
                    });
                }

                let gate = self.session_mutation_gate(&session_id).await;
                let _gate_guard = match gate {
                    Some(ref g) => Some(g.lock().await),
                    None => None,
                };

                let owner = {
                    let sessions = self.sessions.read().await;
                    Arc::clone(
                        &sessions
                            .get(&session_id)
                            .ok_or(RuntimeDriverError::NotReady {
                                state: RuntimeState::Destroyed,
                            })?
                            .tool_visibility_owner,
                    )
                };
                // Delegate to the owner — the `MachineToolVisibilityOwner`
                // trait impl fires the `StageVisibilityFilter` DSL input
                // internally (dogma round 4, wave 2b #12: DSL owns the
                // `next_staged_visibility_revision` monotonic). The DSL
                // input's `update {}` increments and stamps the revision
                // under the authority lock; the owner reads the minted
                // value back and projects it onto its own state.
                // Stage-first: the owner fires the StageVisibilityFilter DSL
                // input, which is not declared from Destroyed — classify a
                // rejection on a Destroyed binding as the terminal truth.
                let revision = match owner.stage_persistent_filter(filter, witnesses) {
                    Ok(revision) => revision,
                    Err(err) => {
                        return Err(self
                            .classify_session_driver_rejection(
                                &session_id,
                                RuntimeDriverError::Internal(err.to_string()),
                            )
                            .await);
                    }
                };
                Ok(MeerkatMachineCommandResult::VisibilityRevision(revision))
            }
            MeerkatMachineCommand::RequestDeferredTools {
                session_id,
                authorities,
            } => {
                if !self.sessions.read().await.contains_key(&session_id) {
                    return Err(RuntimeDriverError::NotReady {
                        state: RuntimeState::Destroyed,
                    });
                }

                let gate = self.session_mutation_gate(&session_id).await;
                let _gate_guard = match gate {
                    Some(ref g) => Some(g.lock().await),
                    None => None,
                };

                let owner = {
                    let sessions = self.sessions.read().await;
                    Arc::clone(
                        &sessions
                            .get(&session_id)
                            .ok_or(RuntimeDriverError::NotReady {
                                state: RuntimeState::Destroyed,
                            })?
                            .tool_visibility_owner,
                    )
                };
                // Delegate to the owner: `request_deferred_tools` applies one
                // generated authority-bearing batch input and then mirrors the
                // accepted machine state into the owner projection. Stage-first:
                // the input is not declared from Destroyed — classify a
                // rejection on a Destroyed binding as the terminal truth.
                let revision = match owner.request_deferred_tools(authorities) {
                    Ok(revision) => revision,
                    Err(err) => {
                        return Err(self
                            .classify_session_driver_rejection(
                                &session_id,
                                RuntimeDriverError::Internal(err.to_string()),
                            )
                            .await);
                    }
                };
                Ok(MeerkatMachineCommandResult::VisibilityRevision(revision))
            }
            MeerkatMachineCommand::PublishCommittedVisibleSet {
                session_id,
                visibility_state,
            } => {
                // Guard: session must exist — publishing to an unknown session
                // has no target.
                let sessions = self.sessions.read().await;
                if !sessions.contains_key(&session_id) {
                    return Err(RuntimeDriverError::NotReady {
                        state: RuntimeState::Destroyed,
                    });
                }
                drop(sessions);

                let gate = self.session_mutation_gate(&session_id).await;
                let _gate_guard = match gate {
                    Some(ref g) => Some(g.lock().await),
                    None => None,
                };

                let owner = {
                    let sessions = self.sessions.read().await;
                    Arc::clone(
                        &sessions
                            .get(&session_id)
                            .ok_or(RuntimeDriverError::NotReady {
                                state: RuntimeState::Destroyed,
                            })?
                            .tool_visibility_owner,
                    )
                };

                // DSL-first: fire the canonical typed `PublishCommittedVisibleSet`
                // input. The per-phase transitions at `dsl::PublishCommittedVisibleSet*`
                // own the `VisibleSurfacesMatchAppliedStateInvariant`:
                //
                //   * `active_not_behind_staged`
                //   * `equal_revision_requires_equal_active_and_staged_input`
                //   * `active_requested_subset_of_staged_requested`
                //
                // Guard rejections surface as `RuntimeDriverError::ValidationFailed`
                // via `stage_session_dsl_input`, so the hand-written shell
                // pre-checks that previously duplicated these invariants have
                // been deleted — the DSL guard is the single source of truth.
                let previous_dsl_state = match self
                    .stage_session_dsl_input(
                        &session_id,
                        crate::meerkat_machine::dsl::MeerkatMachineInput::PublishCommittedVisibleSet {
                            active_filter: crate::meerkat_machine::dsl::ToolFilter::from(
                                &visibility_state.active_filter,
                            ),
                            staged_filter: crate::meerkat_machine::dsl::ToolFilter::from(
                                &visibility_state.staged_filter,
                            ),
                            active_requested_deferred_names: visibility_state
                                .active_requested_deferred_names
                                .clone(),
                            staged_requested_deferred_names: visibility_state
                                .staged_requested_deferred_names
                                .clone(),
                            active_deferred_authorities: visibility_authorities_for_names(
                                &visibility_state.active_requested_deferred_names,
                                &visibility_state.requested_witnesses,
                            ),
                            staged_deferred_authorities: visibility_authorities_for_names(
                                &visibility_state.staged_requested_deferred_names,
                                &visibility_state.requested_witnesses,
                            ),
                            active_visibility_revision: visibility_state.active_revision,
                            staged_visibility_revision: visibility_state.staged_revision,
                        },
                        "PublishCommittedVisibleSet",
                    )
                    .await
                {
                    Ok(previous) => previous,
                    Err(reason) => {
                        // Stage-first: PublishCommittedVisibleSet is declared
                        // per non-Destroyed phase only — classify a rejection
                        // on a Destroyed binding as the terminal truth.
                        return Err(self
                            .classify_session_dsl_rejection(&session_id, reason)
                            .await);
                    }
                };

                if let Err(err) = owner.replace_visibility_state(*visibility_state.clone()) {
                    self.restore_session_dsl_state(&session_id, previous_dsl_state)
                        .await;
                    return Err(RuntimeDriverError::Internal(err.to_string()));
                }

                Ok(MeerkatMachineCommandResult::VisibilityPublished(
                    *visibility_state,
                ))
            }
            _ => unreachable!("non-session command routed to session handler"),
        }
    }

    /// Arc-requiring session dispatch: handles commands that spawn runtime-owned
    /// background tasks.
    pub(super) async fn execute_meerkat_machine_ensure_session_command(
        self: &Arc<Self>,
        command: MeerkatMachineCommand,
    ) -> Result<MeerkatMachineCommandResult, RuntimeDriverError> {
        match command {
            MeerkatMachineCommand::EnsureSessionWithExecutor {
                session_id,
                executor,
            } => {
                // Stage-first: `inner` stages the generated executor
                // registration claim; the machine rejects it on a Destroyed
                // binding and the inner classification surfaces the terminal
                // `Destroyed` truth. `inner` creates the session entry (if
                // new), holds the per-session mutation gate across the
                // generated registration claim and shell publication, attaches
                // the executor, and spawns the runtime loop.
                self.ensure_session_with_executor_inner(session_id, executor)
                    .await?;
                Ok(MeerkatMachineCommandResult::Unit)
            }
            _ => unreachable!("non-ensure-session command routed to arc session handler"),
        }
    }
}
