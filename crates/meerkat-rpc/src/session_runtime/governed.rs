//! Fixed connection admission through existing staged, actor and native owners.

use super::*;
use crate::governed_jsonl::{GovernedConnection, unsupported};
use meerkat_runtime::accept::{AcceptOutcome, InputReplayPolicy};
use meerkat_runtime::identifiers::LogicalRuntimeId;
use meerkat_runtime::input::{Input, PromptInput};
use meerkat_runtime::input_admission_custody::{
    NativeInputAdmissionCompletion, NativeInputAdmissionCustody, NativeInputAdmissionSettlement,
};
use meerkat_runtime::{RuntimeCleanupTaskSpawner, RuntimeExecutorAttachmentWitness};

// This owner contains the real Promoting claim and actual capacity. Its default
// Drop restores only before a native mutation attempt. The native participant
// consumes it at the admission cut, independently of the RPC waiter lifetime.
struct GovernedAdmissionCustody {
    runtime: Arc<SessionRuntime>,
    session_id: SessionId,
    input_id: InputId,
    promotion: Option<PendingPromotionCleanup>,
    admission: Option<RuntimePreAdmission>,
    registration: Option<(
        RuntimeRegistrationLockLease,
        tokio::sync::OwnedMutexGuard<()>,
    )>,
    cleanup: RuntimeCleanupTaskSpawner,
    settled: bool,
    restore_permitted: bool,
    restored_tx: Option<tokio::sync::oneshot::Sender<()>>,
}

impl GovernedAdmissionCustody {
    fn restore(&mut self) {
        if !self.restore_permitted {
            if let Some(promotion) = self.promotion.as_mut() {
                promotion.retain_unresolved();
            }
            drop(self.registration.take());
            if let Some(tx) = self.restored_tx.take() {
                let _ = tx.send(());
            }
            return;
        }
        let promotion = self.promotion.take();
        let admission = self.admission.take();
        let registration = self.registration.take();
        let restored_tx = self.restored_tx.take();
        self.cleanup.spawn_detached(async move {
            if let Some(mut promotion) = promotion {
                promotion.restore_now().await;
            }
            drop(admission);
            drop(registration);
            if let Some(tx) = restored_tx {
                let _ = tx.send(());
            }
        });
    }
}

impl Drop for GovernedAdmissionCustody {
    fn drop(&mut self) {
        if !self.settled {
            self.restore();
        }
    }
}

impl NativeInputAdmissionCustody for GovernedAdmissionCustody {
    fn settle(
        mut self: Box<Self>,
        settlement: NativeInputAdmissionSettlement,
    ) -> Option<NativeInputAdmissionCompletion> {
        self.settled = true;
        if matches!(settlement, NativeInputAdmissionSettlement::NotAdmitted) {
            self.restore();
            return None;
        }
        let unresolved = matches!(settlement, NativeInputAdmissionSettlement::Uncertain);
        if let Some(promotion) = self.promotion.as_mut() {
            if unresolved {
                promotion.retain_unresolved();
            } else {
                self.admission = promotion
                    .take_staged_capacity_admission()
                    .map(RuntimePreAdmission::fresh);
                promotion.mark_materialized();
            }
        }
        // The registration guard serializes this exact session and every Input
        // here is freshly minted by the process-only producer, never a wire ID.
        self.runtime
            .runtime_pre_admissions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(self.session_id.clone())
            .or_default()
            .push(RuntimePreAdmissionEntry {
                input_id: self.input_id.clone(),
                admission: RpcRuntimePreAdmission {
                    admission: self.admission.take(),
                    promotion: self.promotion.take(),
                    unresolved,
                },
            });
        drop(self.registration.take());
        if let Some(tx) = self.restored_tx.take() {
            let _ = tx.send(());
        }
        if unresolved {
            return None;
        }
        let runtime = Arc::clone(&self.runtime);
        let session_id = self.session_id.clone();
        let input_id = self.input_id.clone();
        Some(Box::new(move |observation| {
            Box::pin(async move {
                match observation {
                    Ok(observation) => match runtime
                        .cleanup_runtime_after_completion_outcome(&session_id, observation)
                        .await
                    {
                        Ok(authority) => {
                            if authority == Some(true) {
                                runtime.restore_or_release_runtime_pre_admission(
                                    &session_id,
                                    &input_id,
                                );
                            }
                            Ok(())
                        }
                        Err(failure) => {
                            if failure.releases_pre_admission {
                                runtime.restore_or_release_runtime_pre_admission(
                                    &session_id,
                                    &input_id,
                                );
                            }
                            Err(failure.error)
                        }
                    },
                    Err(error) => {
                        runtime
                            .release_runtime_pre_admission_after_wait_failure_authority(
                                &session_id,
                                &input_id,
                                &error,
                            )
                            .await;
                        Ok(())
                    }
                }
            })
        }))
    }
}

struct PreparedGovernedInput {
    input: Input,
    attachment: RuntimeExecutorAttachmentWitness,
    custody: Box<GovernedAdmissionCustody>,
    restored_rx: tokio::sync::oneshot::Receiver<()>,
}

impl SessionRuntime {
    async fn prepare_governed_input(
        self: &Arc<Self>,
        session_id: SessionId,
        prompt: ContentInput,
        context: Vec<ContentInput>,
        connection: Arc<GovernedConnection>,
    ) -> Result<PreparedGovernedInput, RpcError> {
        let cleanup = RuntimeCleanupTaskSpawner::acquire().map_err(runtime_driver_error_to_rpc)?;
        let runtime = Arc::clone(self);
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        // Only fixed setup runs on the owner task. Input admission remains in
        // the caller until the existing native credential-custody handoff.
        cleanup.clone().spawn_detached(async move {
            let registration = runtime.runtime_registration_lock(&session_id);
            let guard = registration.lock.clone().lock_owned().await;
            if result_tx.is_closed() {
                return;
            }
            let (restored_tx, restored_rx) = tokio::sync::oneshot::channel();
            let result = runtime
                .prepare_governed_input_owned(
                    session_id,
                    prompt,
                    context,
                    connection,
                    cleanup,
                    registration,
                    guard,
                    restored_tx,
                )
                .await;
            let result = match result {
                Ok((input, attachment, custody)) => Ok(PreparedGovernedInput {
                    input,
                    attachment,
                    custody,
                    restored_rx,
                }),
                Err(error) => {
                    let _ = restored_rx.await;
                    Err(error)
                }
            };
            let _ = result_tx.send(result);
        });
        result_rx
            .await
            .map_err(|_| runtime_driver_error_to_rpc(unsupported()))?
    }

    #[allow(clippy::too_many_arguments)]
    async fn prepare_governed_input_owned(
        self: &Arc<Self>,
        session_id: SessionId,
        mut prompt: ContentInput,
        mut context: Vec<ContentInput>,
        connection: Arc<GovernedConnection>,
        cleanup: RuntimeCleanupTaskSpawner,
        registration: RuntimeRegistrationLockLease,
        guard: tokio::sync::OwnedMutexGuard<()>,
        restored_tx: tokio::sync::oneshot::Sender<()>,
    ) -> Result<
        (
            Input,
            RuntimeExecutorAttachmentWitness,
            Box<GovernedAdmissionCustody>,
        ),
        RpcError,
    > {
        if !self.runtime_adapter.contains_session(&session_id).await {
            return Err(runtime_driver_error_to_rpc(unsupported()));
        }
        let slot = self
            .staged_sessions
            .begin_promotion(&session_id)
            .await
            .map_err(|_| RpcError {
                code: error::SESSION_BUSY,
                message: "session already has an owned deferred admission".into(),
                data: None,
            })?;
        let mut custody = Box::new(GovernedAdmissionCustody {
            runtime: Arc::clone(self),
            session_id: session_id.clone(),
            input_id: InputId::new(),
            promotion: slot.as_ref().map(|slot| {
                PendingPromotionCleanup::new(
                    Arc::clone(&self.staged_sessions),
                    Arc::clone(&self.staged_capacity_admissions),
                    &session_id,
                    slot,
                    self.take_staged_capacity_admission(&session_id),
                )
            }),
            admission: None,
            registration: Some((registration, guard)),
            cleanup,
            settled: false,
            restore_permitted: true,
            restored_tx: Some(restored_tx),
        });
        if let Some(slot) = slot {
            if let Some(seed) = slot.deferred_prompt.clone() {
                prompt = merge_content_inputs(seed, prompt);
            }
            let mut merged = slot.deferred_injected_context.clone();
            merged.append(&mut context);
            context = merged;
            if self.runtime_actor_witness(&session_id).is_err() {
                // A fixed create prepared the exact native bindings but no
                // actor. Preserve that seed and build intent in the real
                // service/actor-slot materialization transaction.
                let mut resources = (*slot.build_config).clone();
                let seed = resources
                    .resume_session
                    .clone()
                    .ok_or_else(|| runtime_driver_error_to_rpc(unsupported()))?;
                resources.llm_client_override = self.default_llm_client();
                let request = CreateSessionRequest {
                    model: resources.model.clone(),
                    prompt: ContentInput::Text(String::new()),
                    injected_context: Vec::new(),
                    system_prompt: resources.system_prompt.clone(),
                    max_tokens: resources.max_tokens,
                    event_tx: None,
                    initial_turn: InitialTurnPolicy::Defer,
                    deferred_prompt_policy: DeferredPromptPolicy::Discard,
                    build: Some(resources.to_session_build_options()),
                    labels: slot.labels.clone(),
                };
                let admission = custody
                    .promotion
                    .as_mut()
                    .and_then(PendingPromotionCleanup::take_staged_capacity_admission)
                    .ok_or_else(|| runtime_driver_error_to_rpc(unsupported()))?;
                custody.restore_permitted = false;
                let runtime = Arc::clone(self);
                let materialized =
                    meerkat::surface::materialize_session_with_reserved_admission_and_actor_slot(
                        &self.service,
                        &self.runtime_adapter,
                        seed,
                        request,
                        admission,
                        move |id, attachment, actor_slot| {
                            runtime
                                .session_runtime_executor_for_actor_slot(id, attachment, actor_slot)
                        },
                    )
                    .await;
                if let Err(error) = materialized {
                    // The materialization owner performs its own rollback. A
                    // failed/unknown transaction cannot be treated as an
                    // unmaterialized seed suitable for a second build.
                    if let Some(promotion) = custody.promotion.as_mut() {
                        promotion.retain_unresolved();
                    }
                    custody.settled = true;
                    return Err(match error {
                        meerkat::surface::SurfaceRuntimeMaterializeError::Session(error) => {
                            session_error_to_rpc(error)
                        }
                        _ => runtime_driver_error_to_rpc(unsupported()),
                    });
                }
            }
        }
        let actor = self.runtime_actor_witness(&session_id)?;
        let actor_lease = self
            .service
            .acquire_live_session_actor_turn_boundary_lease_exact(&actor)
            .await
            .map_err(session_error_to_rpc)?
            .ok_or_else(|| runtime_driver_error_to_rpc(unsupported()))?;
        // The service reserves against the exact retained actor, with no cold
        // recovery fallback. No boundary guard crosses native credential I/O.
        let admission = self
            .service
            .reserve_runtime_turn_admission_for_actor(&actor_lease)
            .await
            .map_err(session_error_to_rpc)?;
        if let Some(promotion) = custody.promotion.as_mut() {
            promotion.replace_staged_capacity_admission(admission);
        } else {
            custody.admission = Some(RuntimePreAdmission::fresh(admission));
        }
        custody.restore_permitted = true;
        let attachment = self
            .runtime_adapter
            .current_executor_attachment_witness(&session_id)
            .await
            .ok_or_else(|| runtime_driver_error_to_rpc(unsupported()))?;
        let pin = self
            .service
            .pin_controller_client_for_actor(&actor_lease)
            .await
            .map_err(session_error_to_rpc)?;
        drop(actor_lease);
        let input = Input::Prompt(
            PromptInput::from_content_input(prompt, None).with_injected_context(context),
        );
        custody.input_id = input.id().clone();
        let input = connection
            .bind(&LogicalRuntimeId::for_session(&session_id), input, pin)
            .map_err(runtime_driver_error_to_rpc)?;
        Ok((input, attachment, custody))
    }

    pub(crate) async fn start_governed_turn(
        self: &Arc<Self>,
        session_id: &SessionId,
        prompt: ContentInput,
        context: Vec<ContentInput>,
        connection: Arc<GovernedConnection>,
        request_context: Option<RequestContext>,
    ) -> Result<RunResult, RpcError> {
        let cancelled = || RpcError {
            code: error::REQUEST_CANCELLED,
            message: "request cancelled before admission".into(),
            data: None,
        };
        if request_context
            .as_ref()
            .is_some_and(RequestContext::cancel_already_requested)
        {
            return Err(cancelled());
        }
        let PreparedGovernedInput {
            input,
            attachment,
            custody,
            restored_rx,
        } = self
            .prepare_governed_input(session_id.clone(), prompt, context, connection)
            .await?;
        if request_context
            .as_ref()
            .is_some_and(RequestContext::cancel_already_requested)
        {
            drop(custody);
            let _ = restored_rx.await;
            return Err(cancelled());
        }
        let result = self
            .runtime_adapter
            .accept_input_with_completion_for_attachment_and_replay_policy_with_custody(
                &attachment,
                input,
                InputReplayPolicy::KeyOnly,
                custody,
            )
            .await;
        // A definitive native refusal restores the actual stage before the
        // error reaches the peer, so an immediate retry sees the same owner.
        let _ = restored_rx.await;
        let (outcome, handle) = result.map_err(runtime_driver_error_to_rpc)?;
        if let AcceptOutcome::Accepted { input_id, .. } = &outcome
            && let Some(context) = request_context.as_ref()
        {
            let adapter = Arc::clone(&self.runtime_adapter);
            let session = session_id.clone();
            let input = input_id.clone();
            let _ = context
                .install_cancel_action_or_cancelled(request_action(move || {
                    let adapter = Arc::clone(&adapter);
                    let session = session.clone();
                    let input = input.clone();
                    async move {
                        let _ = adapter
                            .cancel_input_if_present(
                                &session,
                                &input,
                                "governed RPC request cancelled",
                            )
                            .await;
                    }
                }))
                .await;
        }
        let Some(handle) = handle else {
            return Err(runtime_driver_error_to_rpc(unsupported()));
        };
        let outcome = match handle.try_wait().await {
            Ok(outcome) => outcome,
            Err(error) => {
                return Err(self
                    .runtime_completion_wait_error_to_rpc_error(error, session_id)
                    .await);
            }
        };
        completion_outcome_to_rpc_result(outcome, session_id)
    }
}
