//! Fixed connection admission through existing staged, actor and native owners.

use super::*;
use crate::governed_jsonl::{GovernedConnection, unsupported};
use meerkat_runtime::accept::{AcceptOutcome, InputReplayPolicy};
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
    /// Reports the native settlement to the owned admission task, which
    /// settles the host's original outside the native guards.
    settlement_tx: Option<tokio::sync::oneshot::Sender<NativeInputAdmissionSettlement>>,
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
        // Only report the settlement here: the host's original is published
        // by the owned admission task, outside these guards.
        if let Some(tx) = self.settlement_tx.take() {
            let _ = tx.send(settlement);
        }
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

/// The owned outcome of one governed admission: the native accept result
/// and, after an actual acceptance, a typed post-admission custody fault.
struct GovernedAdmission {
    accepted: Result<
        (
            AcceptOutcome,
            Option<meerkat_runtime::completion::CompletionHandle>,
        ),
        RuntimeDriverError,
    >,
    custody_fault: Option<RpcError>,
}

/// What the accept call reported for this input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AcceptReport {
    /// Exactly this input was accepted.
    Accepted,
    /// A definitive other outcome (deduplicated to an existing input,
    /// rejected).
    NotAccepted,
    /// The accept returned an error.
    Failed,
}

fn accept_report(
    accepted: &Result<
        (
            AcceptOutcome,
            Option<meerkat_runtime::completion::CompletionHandle>,
        ),
        RuntimeDriverError,
    >,
) -> AcceptReport {
    match accepted {
        Ok((AcceptOutcome::Accepted { .. }, _)) => AcceptReport::Accepted,
        Ok(_) => AcceptReport::NotAccepted,
        Err(_) => AcceptReport::Failed,
    }
}

/// Settle the host's original from the definitive native settlement and the
/// accept report, outside the native guards. Only an actual acceptance of
/// exactly this input promotes it; an admission that may have happened (an
/// uncertain settlement, or an error returned after the native owner
/// admitted) keeps the candidate as it is; anything else withdraws it, so a
/// duplicate never promotes or revives an original.
fn settle_original(
    original: Box<dyn crate::governed_jsonl::GovernedInputCustody>,
    settlement: NativeInputAdmissionSettlement,
    report: AcceptReport,
) -> Result<(), crate::governed_jsonl::GovernedCustodyFault> {
    match (settlement, report) {
        (NativeInputAdmissionSettlement::Admitted, AcceptReport::Accepted) => original.admit(),
        (NativeInputAdmissionSettlement::Uncertain, _)
        | (NativeInputAdmissionSettlement::Admitted, AcceptReport::Failed) => {
            original.retain();
            Ok(())
        }
        _ => {
            drop(original);
            Ok(())
        }
    }
}

impl SessionRuntime {
    /// Bind and admit one governed input in an owned task that survives the
    /// caller: once the native owner accepts the input, the host's original
    /// is settled from that settlement even if the caller is gone or the
    /// accept returns an error. A staged session's actor is materialized on
    /// the caller's runtime, not in that task (see
    /// [`Self::materialize_staged_actor_on`]).
    async fn admit_governed_input(
        self: &Arc<Self>,
        session_id: SessionId,
        prompt: ContentInput,
        context: Vec<ContentInput>,
        connection: Arc<GovernedConnection>,
        request_context: Option<RequestContext>,
    ) -> Result<GovernedAdmission, RpcError> {
        let cleanup = RuntimeCleanupTaskSpawner::acquire().map_err(runtime_driver_error_to_rpc)?;
        let actor_runtime = tokio::runtime::Handle::try_current().ok();
        let runtime = Arc::clone(self);
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        cleanup.clone().spawn_detached(async move {
            let registration = runtime.runtime_registration_lock(&session_id);
            let guard = registration.lock.clone().lock_owned().await;
            if result_tx.is_closed() {
                return;
            }
            let (restored_tx, restored_rx) = tokio::sync::oneshot::channel();
            let prepared = runtime
                .prepare_governed_input_owned(
                    session_id.clone(),
                    prompt,
                    context,
                    connection,
                    cleanup,
                    registration,
                    guard,
                    restored_tx,
                    actor_runtime,
                )
                .await;
            let (input, original, attachment, mut custody) = match prepared {
                Ok(prepared) => prepared,
                Err(error) => {
                    let _ = restored_rx.await;
                    let _ = result_tx.send(Err(error));
                    return;
                }
            };
            if request_context
                .as_ref()
                .is_some_and(RequestContext::cancel_already_requested)
            {
                drop(custody);
                drop(original);
                let _ = restored_rx.await;
                let _ = result_tx.send(Err(request_cancelled_before_admission()));
                return;
            }
            let (settlement_tx, settlement_rx) = tokio::sync::oneshot::channel();
            custody.settlement_tx = Some(settlement_tx);
            let accepted = runtime
                .runtime_adapter
                .accept_input_with_completion_for_attachment_and_replay_policy_with_custody(
                    &attachment,
                    input,
                    InputReplayPolicy::KeyOnly,
                    custody,
                )
                .await;
            // An unsettled custody was dropped without admission.
            let settlement = settlement_rx
                .await
                .unwrap_or(NativeInputAdmissionSettlement::NotAdmitted);
            // Promotion takes the host's publication writer, which can be held
            // across a durable grant-store write: settle on the blocking pool,
            // owning the custody until it is settled, never on this runtime.
            let report = accept_report(&accepted);
            let custody_fault = match original {
                Some(original) => {
                    match tokio::task::spawn_blocking(move || {
                        settle_original(original, settlement, report)
                    })
                    .await
                    {
                        Ok(Ok(())) => None,
                        Ok(Err(fault)) => Some(post_admission_custody_fault(&session_id, &fault)),
                        Err(_) => Some(post_admission_custody_fault(
                            &session_id,
                            &crate::governed_jsonl::GovernedCustodyFault(
                                "the custody settlement did not complete".into(),
                            ),
                        )),
                    }
                }
                None => None,
            };
            // A definitive native refusal restores the actual stage before the
            // error reaches the peer, so an immediate retry sees the same owner.
            let _ = restored_rx.await;
            let _ = result_tx.send(Ok(GovernedAdmission {
                accepted,
                custody_fault,
            }));
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
        actor_runtime: Option<tokio::runtime::Handle>,
    ) -> Result<
        (
            Input,
            Option<Box<dyn crate::governed_jsonl::GovernedInputCustody>>,
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
            settlement_tx: None,
        });
        if let Some(slot) = slot {
            if let Some(seed) = slot.deferred_prompt.clone() {
                prompt = merge_content_inputs(seed, prompt);
            }
            let mut merged = slot.deferred_injected_context.clone();
            merged.append(&mut context);
            context = merged;
            if self.runtime_actor_witness(&session_id).is_err() {
                // The host binds the session again on its actual current
                // registration before the build connects any configured
                // server: the create's binding does not bind a replacement.
                connection
                    .bind_promoting_session(&self.runtime_adapter, &session_id)
                    .await
                    .map_err(runtime_driver_error_to_rpc)?;
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
                let materialized = self
                    .materialize_staged_actor_on(actor_runtime, seed, request, admission)
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
        // Recovery attaches a recovered session's executor before admission;
        // a missing attachment here is a broken invariant, not an
        // unsupported scope.
        let attachment = self
            .runtime_adapter
            .current_executor_attachment_witness(&session_id)
            .await
            .ok_or_else(|| RpcError {
                code: error::INTERNAL_ERROR,
                message: format!(
                    "governed session {session_id} has no executor attachment at admission"
                ),
                data: None,
            })?;
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
        let (input, original) = connection
            .bind_input(&self.runtime_adapter, &session_id, input, pin)
            .await
            .map_err(runtime_driver_error_to_rpc)?;
        // A resumed session's parked MCP attachment intents were restored,
        // and the session's MCP boundary applied, before this admission
        // (`start_governed_turn`), outside the registration fence.
        Ok((input, original, attachment, custody))
    }

    /// Materialize a staged session's actor for a governed input on
    /// `actor_runtime`, the runtime of the caller that admits the input, and
    /// await the answer.
    ///
    /// The admission runs on the process-wide cleanup runtime, so that its
    /// custody survives the caller. The actor must not be created there: its
    /// session task is spawned on the runtime that runs the materialization,
    /// and every turn of the session would then run on the cleanup runtime's
    /// two workers. A materialization that ends without an answer (its
    /// runtime shut down, or it panicked) is reported as a failed one, by
    /// that kind only: its own report, a panic's payload included, goes to
    /// the host's log. Outside a Tokio runtime it runs in the calling task.
    async fn materialize_staged_actor_on(
        self: &Arc<Self>,
        actor_runtime: Option<tokio::runtime::Handle>,
        seed: Session,
        request: CreateSessionRequest,
        admission: ActiveCapacityGuard,
    ) -> Result<RunResult, meerkat::surface::SurfaceRuntimeMaterializeError> {
        let runtime = Arc::clone(self);
        let materialization = async move {
            let executor_runtime = Arc::clone(&runtime);
            meerkat::surface::materialize_session_with_reserved_admission_and_actor_slot(
                &runtime.service,
                &runtime.runtime_adapter,
                seed,
                request,
                admission,
                move |id, attachment, actor_slot| {
                    executor_runtime
                        .session_runtime_executor_for_actor_slot(id, attachment, actor_slot)
                },
            )
            .await
        };
        let Some(handle) = actor_runtime else {
            return materialization.await;
        };
        match handle.spawn(materialization).await {
            Ok(materialized) => materialized,
            Err(error) => {
                let ended = if error.is_panic() {
                    "panicked"
                } else {
                    "was cancelled: the host's runtime stopped"
                };
                tracing::error!(
                    %error,
                    "the staged session's actor materialization on the host's runtime {ended}"
                );
                Err(RuntimeDriverError::Internal(format!(
                    "the staged session's actor materialization on the host's runtime {ended}; \
                     inspect host logs"
                ))
                .into())
            }
        }
    }

    /// Recover a governed session this host holds neither a live
    /// registration nor a staged seed for (a durable session of a previous
    /// host), so a governed turn can be served on it. The owned no-turn
    /// materialization loads it under its registration fence, exact
    /// materialization claim and capacity, binds the trusted host's
    /// destination and agent on the actual new registration before the agent
    /// is constructed or anything is restored, and rolls the claim back on
    /// any failure. The recovered session's runtime executor is then attached
    /// through the ordinary resume attach, so the governed admission finds
    /// its executor attachment. A session staged here is left as it is (its
    /// first turn promotes it with its executor); a session registered here
    /// without an executor attachment (a recovery that materialized it but
    /// did not attach it) is attached the same way.
    /// The request's cancellation is forwarded into the recovery: a request
    /// already cancelled is refused before anything is claimed, and a cancel
    /// during it abandons the binding or construction in progress and is
    /// compensated like any other failure (the claim is rolled back and the
    /// session's MCP state entry removed). Work already started before the
    /// cancel arrived is compensated, not prevented.
    pub(crate) async fn recover_cold_governed_session(
        self: &Arc<Self>,
        session_id: &SessionId,
        connection: &Arc<GovernedConnection>,
        callback_route: Option<crate::callback_dispatcher::CallbackRoute>,
        request_context: Option<&RequestContext>,
    ) -> Result<(), RpcError> {
        if self.staged_sessions.contains(session_id).await {
            return Ok(());
        }
        if self.runtime_adapter.contains_session(session_id).await {
            return self.attach_governed_session_executor(session_id).await;
        }
        let cancelled = match request_context {
            Some(context) => {
                let (cancel_tx, cancelled) = tokio::sync::watch::channel(false);
                let cancel_tx = Arc::new(cancel_tx);
                let outcome = context
                    .install_cancel_action_or_cancelled(request_action(move || {
                        let cancel_tx = Arc::clone(&cancel_tx);
                        async move {
                            cancel_tx.send_replace(true);
                        }
                    }))
                    .await;
                if matches!(
                    outcome,
                    meerkat::surface::CancelActionInstallOutcome::AlreadyCancelled
                ) {
                    return Err(request_cancelled_before_admission());
                }
                Some(cancelled)
            }
            None => None,
        };
        // Process-local resources only: durable configuration, the session's
        // configured LLM selection included, is inherited through the
        // recovery owner. This host's default controller client is supplied
        // only when it presents exactly that persisted selection. Otherwise
        // the persisted selection is kept, not re-resolved: a different host
        // default never serves the session, and the recovered controller's
        // pin check refuses the mismatch (fail closed).
        let client = self.default_llm_client();
        let selection = client
            .as_ref()
            .and_then(|client| client.controller_model_selection())
            .ok_or_else(|| runtime_driver_error_to_rpc(unsupported()))?;
        let persisted = self.load_persisted_session(session_id).await?;
        let persisted_identity = persisted
            .as_ref()
            .and_then(|session| session.session_metadata())
            .map(|metadata| metadata.llm_identity());
        let host_presents_persisted_selection =
            persisted_identity.as_ref().is_none_or(|identity| {
                selection.model() == identity.model
                    && selection.provider() == identity.provider
                    && selection.self_hosted_server_id()
                        == identity.self_hosted_server_id.as_deref()
                    && selection.auth_binding() == identity.auth_binding.as_ref()
            });
        let mut resources = AgentBuildConfig::new(selection.model().to_string());
        if host_presents_persisted_selection {
            resources.llm_client_override = client;
        }
        // A configured server that the session's durable record attaches
        // live is restored at this turn's boundary, from its current
        // installation under its own fresh control (`start_governed_turn`).
        // Configured startup leaves that server to the restore: two adds of
        // one server on the session's one tool surface collide, and the
        // restored add's boundary is refused while the configured connection
        // is still pending.
        let restored: Vec<String> = persisted
            .as_ref()
            .and_then(|session| session.build_state())
            .map(|state| {
                state
                    .mcp_attachment_intents
                    .into_iter()
                    .filter(|intent| {
                        intent.disposition
                            == meerkat_core::McpAttachmentIntentDisposition::Requested
                    })
                    .map(|intent| intent.descriptor.server_name)
                    .collect()
            })
            .unwrap_or_default();
        resources.mcp_servers = connection
            .configured_mcp_servers()
            .into_iter()
            .filter(|server| !restored.contains(&server.name))
            .collect();
        if let Some(route) = callback_route.as_ref() {
            let dispatcher: Arc<dyn meerkat_core::AgentToolDispatcher> =
                Arc::new(self.callback_tool_dispatcher_for_route(route, Vec::new()));
            resources.external_tools = Some(dispatcher);
        }
        let binder = Arc::clone(connection);
        let bind: super::BeforeConstruction = Box::new(move |machine, session_id| {
            Box::pin(async move {
                binder
                    .bind_promoting_session(&machine, &session_id)
                    .await
                    .map_err(runtime_driver_error_to_rpc)
            })
        });
        self.create_or_resume_session_without_turn_bound(
            resources,
            Some(session_id.clone()),
            None,
            SurfaceSessionRecoveryOverrides::default(),
            Some(bind),
            cancelled,
        )
        .await?;
        if let Some(route) = callback_route {
            self.bind_session_callback_route(session_id.clone(), route);
        }
        self.attach_governed_session_executor(session_id).await
    }

    /// Attach the runtime executor of a governed session registered on this
    /// host without one, through the ordinary resume attach (outside any
    /// admission, which holds the registration fence and the actor's turn
    /// boundary). A session that already has its executor attachment is left
    /// as it is.
    async fn attach_governed_session_executor(
        self: &Arc<Self>,
        session_id: &SessionId,
    ) -> Result<(), RpcError> {
        if self
            .runtime_adapter
            .current_executor_attachment_witness(session_id)
            .await
            .is_some()
        {
            return Ok(());
        }
        self.ensure_runtime_executor(session_id).await
    }

    /// Restore a resumed governed session's parked MCP attachment intents
    /// under fresh native controls prepared for this connection's binding
    /// (set when the connection created or recovered the session), staging
    /// each restored attachment and queueing each refusal (a revoked grant's
    /// `denied`) as the session's lifecycle outcome.
    #[cfg(feature = "mcp")]
    async fn restore_governed_mcp_attachments(
        self: &Arc<Self>,
        session_id: &SessionId,
        connection: &Arc<GovernedConnection>,
    ) {
        let runtime = Arc::clone(self);
        let connection = Arc::clone(connection);
        let session = session_id.clone();
        let installations = connection.realm_mcp_installations();
        self.restore_pending_mcp_attachments(session_id, installations, move |server| {
            let runtime = Arc::clone(&runtime);
            let connection = Arc::clone(&connection);
            let session = session.clone();
            async move {
                crate::governed_jsonl::governed_mcp_attachment_control(
                    &connection,
                    &runtime,
                    &session,
                    server,
                )
                .await
            }
        })
        .await;
    }

    /// Serve one governed turn. Before the input is admitted, the session's
    /// MCP boundary is applied as the ordinary runtime turn applies it: a
    /// resumed session's parked attachment intents are restored, staged MCP
    /// operations (restored attachments and `mcp/add`, `mcp/remove`,
    /// `mcp/reload`) connect and become visible to this turn, and the queued
    /// lifecycle outcomes are emitted on `mcp_event_tx` as
    /// `tool_config_changed`. This runs before the admission takes the
    /// registration fence and the actor's turn boundary (B -> M), and holds
    /// neither: it takes only the session's MCP state and intent order.
    pub(crate) async fn start_governed_turn(
        self: &Arc<Self>,
        session_id: &SessionId,
        prompt: ContentInput,
        context: Vec<ContentInput>,
        connection: Arc<GovernedConnection>,
        mcp_event_tx: mpsc::Sender<EventEnvelope<AgentEvent>>,
        request_context: Option<RequestContext>,
    ) -> Result<RunResult, RpcError> {
        if request_context
            .as_ref()
            .is_some_and(RequestContext::cancel_already_requested)
        {
            return Err(request_cancelled_before_admission());
        }
        #[cfg(feature = "mcp")]
        let prompt = {
            self.restore_governed_mcp_attachments(session_id, &connection)
                .await;
            let mut mcp_text = String::new();
            self.apply_mcp_boundary(session_id, &mcp_event_tx, &mut mcp_text)
                .await?;
            if mcp_text.is_empty() {
                prompt
            } else {
                let mut blocks = prompt.into_blocks();
                blocks.insert(
                    0,
                    meerkat_core::types::ContentBlock::Text { text: mcp_text },
                );
                ContentInput::Blocks(blocks)
            }
        };
        #[cfg(not(feature = "mcp"))]
        drop(mcp_event_tx);
        let GovernedAdmission {
            accepted,
            custody_fault,
        } = self
            .admit_governed_input(
                session_id.clone(),
                prompt,
                context,
                connection,
                request_context.clone(),
            )
            .await?;
        let result = accepted;
        let (outcome, handle) = result.map_err(runtime_driver_error_to_rpc)?;
        // The input stands admitted; the host could not publish its original.
        if let Some(fault) = custody_fault {
            return Err(fault);
        }
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
        let waited_input_id = match &outcome {
            AcceptOutcome::Accepted { input_id, .. } => Some(input_id.clone()),
            AcceptOutcome::Deduplicated { existing_id, .. } => Some(existing_id.clone()),
            _ => None,
        };
        let outcome = match handle.try_wait().await {
            Ok(outcome) => outcome,
            Err(error) => {
                return Err(self
                    .runtime_completion_wait_error_to_rpc_error(
                        error,
                        session_id,
                        waited_input_id.as_ref(),
                    )
                    .await);
            }
        };
        completion_outcome_to_rpc_result(outcome, session_id)
    }
}

fn request_cancelled_before_admission() -> RpcError {
    RpcError {
        code: error::REQUEST_CANCELLED,
        message: "request cancelled before admission".into(),
        data: None,
    }
}

/// A typed post-admission owner fault: the input stands admitted, but the
/// host could not publish the original it names. Not a permission refusal.
fn post_admission_custody_fault(
    session_id: &SessionId,
    fault: &crate::governed_jsonl::GovernedCustodyFault,
) -> RpcError {
    RpcError {
        code: error::INTERNAL_ERROR,
        message: format!("the admitted input's original custody was not published: {fault}"),
        data: Some(serde_json::json!({
            "session_id": session_id.to_string(),
            "reason": "post_admission_custody_fault",
        })),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::governed_jsonl::{GovernedCustodyFault, GovernedInputCustody};
    use std::sync::Mutex;

    struct Recorder {
        log: Arc<Mutex<Vec<&'static str>>>,
        admit_fails: bool,
        promoted: bool,
    }

    impl GovernedInputCustody for Recorder {
        fn admit(mut self: Box<Self>) -> Result<(), GovernedCustodyFault> {
            self.log.lock().unwrap().push("admit");
            self.promoted = true;
            if self.admit_fails {
                return Err(GovernedCustodyFault("publication unavailable".into()));
            }
            Ok(())
        }
        fn retain(self: Box<Self>) {
            self.log.lock().unwrap().push("retain");
            std::mem::forget(self);
        }
    }

    impl Drop for Recorder {
        fn drop(&mut self) {
            if !self.promoted {
                self.log.lock().unwrap().push("withdraw");
            }
        }
    }

    fn settle(
        settlement: NativeInputAdmissionSettlement,
        report: AcceptReport,
        admit_fails: bool,
    ) -> (Vec<&'static str>, bool) {
        let log = Arc::new(Mutex::new(Vec::new()));
        let result = settle_original(
            Box::new(Recorder {
                log: Arc::clone(&log),
                admit_fails,
                promoted: false,
            }),
            settlement,
            report,
        );
        let calls = log.lock().unwrap().clone();
        (calls, result.is_err())
    }

    /// Only an actual acceptance of exactly this input promotes the original;
    /// a possible admission keeps it; anything else withdraws it.
    #[test]
    fn the_original_follows_the_definitive_native_settlement() {
        use AcceptReport::{Accepted, Failed, NotAccepted};
        use NativeInputAdmissionSettlement::{Admitted, NotAdmitted, Uncertain};
        assert_eq!(settle(Admitted, Accepted, false), (vec!["admit"], false));
        // A duplicate (an admitted replay of an existing input) never
        // promotes or revives an original.
        assert_eq!(
            settle(Admitted, NotAccepted, false),
            (vec!["withdraw"], false)
        );
        // An error returned after the native owner admitted keeps it.
        assert_eq!(settle(Admitted, Failed, false), (vec!["retain"], false));
        for report in [Accepted, NotAccepted, Failed] {
            assert_eq!(settle(Uncertain, report, false), (vec!["retain"], false));
            assert_eq!(
                settle(NotAdmitted, report, false),
                (vec!["withdraw"], false)
            );
        }
    }

    /// A promotion that cannot publish is a typed post-admission fault, not a
    /// refusal and not a silent success.
    #[test]
    fn a_failed_promotion_is_a_typed_post_admission_fault() {
        let (calls, failed) = settle(
            NativeInputAdmissionSettlement::Admitted,
            AcceptReport::Accepted,
            true,
        );
        assert!(failed);
        assert_eq!(calls[0], "admit");
        let fault = post_admission_custody_fault(
            &SessionId::new(),
            &GovernedCustodyFault("publication unavailable".into()),
        );
        assert_eq!(fault.code, error::INTERNAL_ERROR);
        assert_eq!(
            fault.data.as_ref().unwrap()["reason"],
            "post_admission_custody_fault"
        );
    }
}
