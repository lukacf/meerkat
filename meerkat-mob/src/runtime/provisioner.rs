use super::*;
use crate::MobBackendKind;
use crate::definition::ExternalBackendConfig;
use crate::event::MemberRef;
use crate::runtime::handle::MemberSpawnReceipt;
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use async_trait::async_trait;
use meerkat_core::comms::TrustedPeerSpec;
use meerkat_core::event_injector::SubscribableInjector;
use meerkat_core::lifecycle::core_executor::{CoreApplyOutput, CoreExecutor, CoreExecutorError};
use meerkat_core::lifecycle::run_control::RunControlCommand;
use meerkat_core::lifecycle::run_primitive::{CoreRenderable, RunApplyBoundary, RunPrimitive};
use meerkat_core::lifecycle::{InputId, RunId as CoreRunId};
use meerkat_core::ops::OperationId;
use meerkat_core::ops_lifecycle::{OperationStatus, OpsLifecycleRegistry};
use meerkat_core::service::{CreateSessionRequest, SessionError, StartTurnRequest};
use meerkat_core::types::SessionId;
#[allow(unused_imports)]
use meerkat_runtime::service_ext::SessionServiceRuntimeExt as _;
use meerkat_runtime::{
    Input, InputDurability, InputHeader, InputOrigin, InputVisibility, PromptInput,
    RuntimeSessionAdapter,
};
use std::collections::HashMap;
use tokio::sync::{Mutex, RwLock};

type TurnEventTx = tokio::sync::mpsc::Sender<meerkat_core::EventEnvelope<meerkat_core::AgentEvent>>;

pub struct ProvisionMemberRequest {
    pub create_session: CreateSessionRequest,
    pub backend: MobBackendKind,
    pub peer_name: String,
    pub owner_session_id: Option<SessionId>,
    pub ops_registry: Option<Arc<dyn OpsLifecycleRegistry>>,
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait MobProvisioner: Send + Sync {
    async fn provision_member(
        &self,
        req: ProvisionMemberRequest,
    ) -> Result<MemberSpawnReceipt, MobError>;
    async fn abort_member_provision(
        &self,
        member_ref: &MemberRef,
        operation_id: &OperationId,
        reason: &str,
    ) -> Result<(), MobError>;
    async fn retire_member(&self, member_ref: &MemberRef) -> Result<(), MobError>;
    async fn interrupt_member(&self, member_ref: &MemberRef) -> Result<(), MobError>;
    async fn start_turn(
        &self,
        member_ref: &MemberRef,
        req: StartTurnRequest,
    ) -> Result<(), MobError>;
    async fn start_flow_step(
        &self,
        member_ref: &MemberRef,
        run_id: &crate::RunId,
        step_id: &crate::StepId,
        req: StartTurnRequest,
    ) -> Result<(), MobError> {
        let _ = (run_id, step_id);
        self.start_turn(member_ref, req).await
    }
    async fn interaction_event_injector(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<dyn SubscribableInjector>>;
    async fn is_member_active(&self, member_ref: &MemberRef) -> Result<Option<bool>, MobError>;
    async fn comms_runtime(&self, member_ref: &MemberRef) -> Option<Arc<dyn CoreCommsRuntime>>;
    async fn trusted_peer_spec(
        &self,
        member_ref: &MemberRef,
        fallback_name: &str,
        fallback_peer_id: &str,
    ) -> Result<TrustedPeerSpec, MobError>;
    /// Resolve the live canonical mob-child lifecycle operation for an
    /// existing member bridge.
    async fn active_operation_id_for_member(&self, member_ref: &MemberRef) -> Option<OperationId>;
    async fn bind_member_owner_context(
        &self,
        member_ref: &MemberRef,
        owner_session_id: SessionId,
        ops_registry: Arc<dyn OpsLifecycleRegistry>,
    ) -> Result<(), MobError>;

    /// Cancel all active checkpointer gates so in-flight saves complete but
    /// subsequent checkpoints are no-ops. Call during mob stop.
    async fn cancel_all_checkpointers(&self) {}

    /// Re-enable checkpointer gates after a prior cancel. Call during mob resume.
    async fn rearm_all_checkpointers(&self) {}
}

pub struct SessionBackend {
    session_service: Arc<dyn MobSessionService>,
    runtime_adapter: Option<Arc<RuntimeSessionAdapter>>,
    ops_adapter: Arc<super::ops_adapter::MobOpsAdapter>,
    // Capability index for runtime bridge sidecars keyed by registered runtime
    // session identity. This map is never lifecycle truth; canonical
    // registration/attachment truth stays in RuntimeSessionAdapter.
    runtime_sessions: Arc<RwLock<HashMap<SessionId, Arc<RuntimeSessionState>>>>,
}

impl SessionBackend {
    pub fn new(
        session_service: Arc<dyn MobSessionService>,
        runtime_adapter: Option<Arc<RuntimeSessionAdapter>>,
    ) -> Self {
        Self {
            session_service,
            runtime_adapter,
            ops_adapter: Arc::new(super::ops_adapter::MobOpsAdapter::new()),
            runtime_sessions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    fn require_session(
        member_ref: &MemberRef,
        operation: &'static str,
    ) -> Result<SessionId, MobError> {
        member_ref.session_id().cloned().ok_or_else(|| {
            MobError::Internal(format!(
                "session-backed provisioner cannot {operation} member without session bridge: {member_ref:?}"
            ))
        })
    }

    fn trusted_peer_spec(
        fallback_name: &str,
        fallback_peer_id: &str,
    ) -> Result<TrustedPeerSpec, MobError> {
        TrustedPeerSpec::new(
            fallback_name,
            fallback_peer_id,
            format!("inproc://{fallback_name}"),
        )
        .map_err(|error| MobError::WiringError(format!("invalid peer spec: {error}")))
    }

    async fn runtime_session_state(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<RuntimeSessionState>> {
        let adapter = self.runtime_adapter.as_ref()?;
        if let Some(existing) = self.runtime_sessions.read().await.get(session_id).cloned() {
            if adapter.contains_session(session_id).await {
                return Some(existing);
            }
            existing.clear_queued_turns().await;
            let state = Arc::new(RuntimeSessionState {
                queued_turns: Mutex::new(RuntimeSessionQueue::default()),
            });
            let executor = Box::new(MobSessionRuntimeExecutor::new(
                self.session_service.clone(),
                Arc::clone(adapter),
                session_id.clone(),
                state.clone(),
                Arc::clone(&self.runtime_sessions),
            ));
            // Runtime session registrations are capability bindings. If the
            // adapter lost this session, drop any queued bridge context from
            // the stale binding before reattaching with a fresh sidecar.
            adapter
                .ensure_session_with_executor(session_id.clone(), executor)
                .await;
            self.runtime_sessions
                .write()
                .await
                .insert(session_id.clone(), state.clone());
            return Some(state);
        }
        let state = Arc::new(RuntimeSessionState {
            queued_turns: Mutex::new(RuntimeSessionQueue::default()),
        });
        let executor = Box::new(MobSessionRuntimeExecutor::new(
            self.session_service.clone(),
            Arc::clone(adapter),
            session_id.clone(),
            state.clone(),
            Arc::clone(&self.runtime_sessions),
        ));
        adapter
            .ensure_session_with_executor(session_id.clone(), executor)
            .await;
        self.runtime_sessions
            .write()
            .await
            .insert(session_id.clone(), state.clone());
        Some(state)
    }

    async fn remove_runtime_session_state(&self, session_id: &SessionId) {
        let removed = self.runtime_sessions.write().await.remove(session_id);
        if let Some(state) = removed {
            state.clear_queued_turns().await;
        }
    }

    async fn execute_runtime_input(
        &self,
        session_id: &SessionId,
        input: Input,
        event_tx: Option<TurnEventTx>,
    ) -> Result<(), MobError> {
        let adapter = self.runtime_adapter.as_ref().ok_or_else(|| {
            MobError::Internal(format!(
                "runtime-backed turn requested without runtime adapter: {session_id}"
            ))
        })?;
        let state = self.runtime_session_state(session_id).await;
        let adapter_session_id = session_id.clone();
        let requested_input_id = input.id().clone();
        let mut context_input_id = requested_input_id.clone();

        // Queue only owner bridge context that cannot be reconstructed from
        // runtime primitives. Bind context by canonical input identity (not by
        // FIFO order) so runtime-owned contributing IDs remain the sole source
        // of semantic ordering.
        let queued_context = if let Some(ref state) = state {
            state
                .enqueue_turn_context(requested_input_id.clone(), event_tx)
                .await
        } else {
            false
        };

        let (outcome, handle) = match adapter
            .accept_input_with_completion(&adapter_session_id, input)
            .await
        {
            Ok(result) => result,
            Err(err) => {
                if let Some(state) = state.as_ref()
                    && queued_context
                {
                    let _ = state.discard_turn_context(&requested_input_id).await;
                }
                return Err(MobError::Internal(err.to_string()));
            }
        };

        if let Some(state) = state.as_ref()
            && queued_context
        {
            let canonical_input_id = match &outcome {
                meerkat_runtime::AcceptOutcome::Accepted { input_id, .. } => Some(input_id),
                // For in-flight dedup, runtime primitives are keyed by the existing
                // canonical input id, not the newly attempted one.
                meerkat_runtime::AcceptOutcome::Deduplicated { existing_id, .. } => {
                    Some(existing_id)
                }
                _ => None,
            };
            if let Some(input_id) = canonical_input_id
                && input_id != &requested_input_id
            {
                let rekeyed = state
                    .rekey_turn_context(&requested_input_id, input_id.clone())
                    .await;
                if rekeyed {
                    context_input_id = input_id.clone();
                }
            }
        }

        // Terminal dedup: input already processed — idempotent success
        let Some(handle) = handle else {
            if let Some(state) = state.as_ref()
                && queued_context
            {
                let _ = state.discard_turn_context(&context_input_id).await;
            }
            return Ok(());
        };

        let completion = handle.wait().await;
        if let Some(state) = state.as_ref()
            && queued_context
        {
            let _ = state.discard_turn_context(&context_input_id).await;
        }

        match completion {
            meerkat_runtime::completion::CompletionOutcome::Completed(_) => Ok(()),
            meerkat_runtime::completion::CompletionOutcome::CompletedWithoutResult => Ok(()),
            meerkat_runtime::completion::CompletionOutcome::Abandoned(reason) => {
                Err(MobError::Internal(format!("turn abandoned: {reason}")))
            }
            meerkat_runtime::completion::CompletionOutcome::RuntimeTerminated(reason) => {
                Err(MobError::Internal(format!("runtime terminated: {reason}")))
            }
        }
    }
}

struct RuntimeSessionState {
    // Transport-only owner context keyed by canonical runtime input identity.
    // Never used as lifecycle/ordering truth; ordering is runtime-owned via
    // contributing_input_ids and input lifecycle state.
    queued_turns: Mutex<RuntimeSessionQueue>,
}

#[derive(Default)]
struct RuntimeSessionQueue {
    // Event delivery transport handles for runtime-backed turn dispatch.
    entries: HashMap<InputId, QueuedTurnContext>,
}

struct QueuedTurnContext {
    event_tx: TurnEventTx,
}

impl RuntimeSessionState {
    async fn enqueue_turn_context(&self, input_id: InputId, event_tx: Option<TurnEventTx>) -> bool {
        let Some(event_tx) = event_tx else {
            return false;
        };
        let mut queued_turns = self.queued_turns.lock().await;
        queued_turns
            .entries
            .insert(input_id, QueuedTurnContext { event_tx });
        true
    }

    async fn take_turn_context_for_inputs(
        &self,
        contributing_input_ids: &[InputId],
    ) -> Option<QueuedTurnContext> {
        let mut queued_turns = self.queued_turns.lock().await;
        let mut selected: Option<QueuedTurnContext> = None;
        for input_id in contributing_input_ids {
            if let Some(context) = queued_turns.entries.remove(input_id) {
                // A runtime primitive may contribute multiple input IDs
                // (staged/apply-boundary cases). Drain every matching key so the
                // bridge map never accumulates stale side entries; prefer the
                // most-recent matching contributor in canonical order.
                selected = Some(context);
            }
        }
        selected
    }

    async fn rekey_turn_context(&self, from_input_id: &InputId, to_input_id: InputId) -> bool {
        if from_input_id == &to_input_id {
            return true;
        }
        let mut queued_turns = self.queued_turns.lock().await;
        if queued_turns.entries.contains_key(&to_input_id) {
            // Keep the canonical destination binding and drop the source alias.
            queued_turns.entries.remove(from_input_id);
            return true;
        }
        let Some(context) = queued_turns.entries.remove(from_input_id) else {
            return false;
        };
        queued_turns.entries.insert(to_input_id, context);
        true
    }

    async fn discard_turn_context(&self, input_id: &InputId) -> bool {
        let mut queued_turns = self.queued_turns.lock().await;
        queued_turns.entries.remove(input_id).is_some()
    }

    async fn clear_queued_turns(&self) {
        self.queued_turns.lock().await.entries.clear();
    }
}

struct MobSessionRuntimeExecutor {
    session_service: Arc<dyn MobSessionService>,
    runtime_adapter: Arc<RuntimeSessionAdapter>,
    session_id: SessionId,
    state: Arc<RuntimeSessionState>,
    runtime_sessions: Arc<RwLock<HashMap<SessionId, Arc<RuntimeSessionState>>>>,
}

impl MobSessionRuntimeExecutor {
    fn new(
        session_service: Arc<dyn MobSessionService>,
        runtime_adapter: Arc<RuntimeSessionAdapter>,
        session_id: SessionId,
        state: Arc<RuntimeSessionState>,
        runtime_sessions: Arc<RwLock<HashMap<SessionId, Arc<RuntimeSessionState>>>>,
    ) -> Self {
        Self {
            session_service,
            runtime_adapter,
            session_id,
            state,
            runtime_sessions,
        }
    }
}

fn extract_prompt_input(primitive: &RunPrimitive) -> ContentInput {
    match primitive {
        RunPrimitive::StagedInput(staged) => {
            let mut all_blocks = Vec::new();
            for append in &staged.appends {
                match &append.content {
                    CoreRenderable::Text { text } => {
                        all_blocks
                            .push(meerkat_core::types::ContentBlock::Text { text: text.clone() });
                    }
                    CoreRenderable::Blocks { blocks } => {
                        all_blocks.extend(blocks.iter().cloned());
                    }
                    _ => {}
                }
            }
            if all_blocks.len() == 1 {
                if let meerkat_core::types::ContentBlock::Text { text } = &all_blocks[0] {
                    ContentInput::Text(text.clone())
                } else {
                    ContentInput::Blocks(all_blocks)
                }
            } else if all_blocks.is_empty() {
                ContentInput::Text(String::new())
            } else {
                ContentInput::Blocks(all_blocks)
            }
        }
        RunPrimitive::ImmediateAppend(append) => match &append.content {
            CoreRenderable::Text { text } => ContentInput::Text(text.clone()),
            CoreRenderable::Blocks { blocks } => ContentInput::Blocks(blocks.clone()),
            _ => ContentInput::Text(String::new()),
        },
        RunPrimitive::ImmediateContextAppend(append) => match &append.content {
            CoreRenderable::Text { text } => ContentInput::Text(text.clone()),
            CoreRenderable::Blocks { blocks } => ContentInput::Blocks(blocks.clone()),
            _ => ContentInput::Text(String::new()),
        },
        _ => ContentInput::Text(String::new()),
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl CoreExecutor for MobSessionRuntimeExecutor {
    async fn apply(
        &mut self,
        run_id: CoreRunId,
        primitive: RunPrimitive,
    ) -> Result<CoreApplyOutput, CoreExecutorError> {
        let contributing_input_ids = primitive.contributing_input_ids().to_vec();
        let queued_context = self
            .state
            .take_turn_context_for_inputs(&contributing_input_ids)
            .await;
        let req = StartTurnRequest {
            prompt: extract_prompt_input(&primitive),
            system_prompt: None,
            render_metadata: primitive
                .turn_metadata()
                .and_then(|meta| meta.render_metadata.clone()),
            handling_mode: primitive
                .turn_metadata()
                .and_then(|meta| meta.handling_mode)
                .unwrap_or(meerkat_core::types::HandlingMode::Queue),
            event_tx: queued_context.map(|context| context.event_tx),
            skill_references: primitive
                .turn_metadata()
                .and_then(|meta| meta.skill_references.clone()),
            flow_tool_overlay: primitive
                .turn_metadata()
                .and_then(|meta| meta.flow_tool_overlay.clone()),
            additional_instructions: primitive
                .turn_metadata()
                .and_then(|meta| meta.additional_instructions.clone()),
        };

        self.session_service
            .apply_runtime_turn(
                &self.session_id,
                run_id,
                req,
                match &primitive {
                    RunPrimitive::StagedInput(staged) => staged.boundary,
                    _ => RunApplyBoundary::Immediate,
                },
                contributing_input_ids,
            )
            .await
            .map_err(|err| CoreExecutorError::ApplyFailed {
                reason: err.to_string(),
            })
    }

    async fn control(&mut self, command: RunControlCommand) -> Result<(), CoreExecutorError> {
        match command {
            RunControlCommand::CancelCurrentRun { .. } => self
                .session_service
                .interrupt(&self.session_id)
                .await
                .map_err(|err| CoreExecutorError::ControlFailed {
                    reason: err.to_string(),
                }),
            RunControlCommand::StopRuntimeExecutor { .. } => {
                let discard_result = self
                    .session_service
                    .discard_live_session(&self.session_id)
                    .await;
                self.runtime_adapter
                    .unregister_session(&self.session_id)
                    .await;
                let removed = {
                    let mut runtime_sessions = self.runtime_sessions.write().await;
                    let should_remove = runtime_sessions
                        .get(&self.session_id)
                        .is_some_and(|state| Arc::ptr_eq(state, &self.state));
                    if should_remove {
                        runtime_sessions.remove(&self.session_id)
                    } else {
                        None
                    }
                };
                if let Some(state) = removed {
                    state.clear_queued_turns().await;
                } else {
                    self.state.clear_queued_turns().await;
                }
                match discard_result {
                    Ok(()) | Err(SessionError::NotFound { .. }) => Ok(()),
                    Err(err) => Err(CoreExecutorError::ControlFailed {
                        reason: err.to_string(),
                    }),
                }
            }
            _ => Ok(()),
        }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl MobProvisioner for SessionBackend {
    async fn provision_member(
        &self,
        req: ProvisionMemberRequest,
    ) -> Result<MemberSpawnReceipt, MobError> {
        tracing::debug!(
            backend = ?req.backend,
            peer_name = %req.peer_name,
            "SessionBackend::provision_member start"
        );
        let created = self
            .session_service
            .create_session(req.create_session)
            .await?;
        if self.runtime_adapter.is_some() {
            let _ = self.runtime_session_state(&created.session_id).await;
        }
        if let (Some(owner_session_id), Some(registry)) = (req.owner_session_id, req.ops_registry) {
            self.ops_adapter.bind_session_registry(
                created.session_id.clone(),
                owner_session_id,
                registry,
            );
        }
        let operation_id = self
            .ops_adapter
            .mark_member_provisioned(&created.session_id, &req.peer_name)
            .await?;
        tracing::debug!(
            session_id = %created.session_id,
            "SessionBackend::provision_member created session"
        );
        Ok(MemberSpawnReceipt {
            member_ref: MemberRef::from_session_id(created.session_id),
            operation_id,
        })
    }

    async fn abort_member_provision(
        &self,
        member_ref: &MemberRef,
        operation_id: &OperationId,
        reason: &str,
    ) -> Result<(), MobError> {
        let session_id = Self::require_session(member_ref, "abort provision for")?;
        match self.ops_adapter.operation_status(&session_id, operation_id) {
            Some(OperationStatus::Provisioning) => {
                if let Some(adapter) = &self.runtime_adapter {
                    if adapter.contains_session(&session_id).await {
                        adapter.unregister_session(&session_id).await;
                    }
                    self.remove_runtime_session_state(&session_id).await;
                }
                match self.session_service.archive(&session_id).await {
                    Ok(()) | Err(SessionError::NotFound { .. }) => {}
                    Err(error) => return Err(error.into()),
                }
                self.ops_adapter
                    .abort_member_provision(&session_id, operation_id, Some(reason.to_string()))
                    .await
            }
            Some(OperationStatus::Running) | Some(OperationStatus::Retiring) => {
                self.retire_member(member_ref).await
            }
            Some(
                OperationStatus::Completed
                | OperationStatus::Failed
                | OperationStatus::Aborted
                | OperationStatus::Cancelled
                | OperationStatus::Retired
                | OperationStatus::Terminated,
            ) => {
                if let Some(adapter) = &self.runtime_adapter {
                    if adapter.contains_session(&session_id).await {
                        adapter.unregister_session(&session_id).await;
                    }
                    self.remove_runtime_session_state(&session_id).await;
                }
                match self.session_service.archive(&session_id).await {
                    Ok(()) | Err(SessionError::NotFound { .. }) => {}
                    Err(error) => return Err(error.into()),
                }
                Ok(())
            }
            Some(OperationStatus::Absent) | None => {
                if let Some(adapter) = &self.runtime_adapter {
                    if adapter.contains_session(&session_id).await {
                        adapter.unregister_session(&session_id).await;
                    }
                    self.remove_runtime_session_state(&session_id).await;
                }
                match self.session_service.archive(&session_id).await {
                    Ok(()) | Err(SessionError::NotFound { .. }) => {}
                    Err(error) => return Err(error.into()),
                }
                Ok(())
            }
        }
    }

    async fn retire_member(&self, member_ref: &MemberRef) -> Result<(), MobError> {
        let session_id = Self::require_session(member_ref, "retire")?;
        if let Some(adapter) = &self.runtime_adapter {
            if adapter.contains_session(&session_id).await {
                adapter
                    .retire_runtime(&session_id)
                    .await
                    .map_err(|err| MobError::Internal(err.to_string()))?;
                adapter.unregister_session(&session_id).await;
            }
            self.remove_runtime_session_state(&session_id).await;
        }
        self.session_service.archive(&session_id).await?;
        self.ops_adapter.mark_member_retired(member_ref).await?;
        Ok(())
    }

    async fn interrupt_member(&self, member_ref: &MemberRef) -> Result<(), MobError> {
        let session_id = Self::require_session(member_ref, "interrupt")?;
        if let Some(adapter) = &self.runtime_adapter {
            if adapter.contains_session(&session_id).await {
                return match adapter.interrupt_current_run(&session_id).await {
                    Ok(()) => Ok(()),
                    Err(error) => {
                        // Preserve bridge sidecar alignment when registration
                        // changed between contains_session and interrupt.
                        if !adapter.contains_session(&session_id).await {
                            self.remove_runtime_session_state(&session_id).await;
                        }
                        Err(MobError::Internal(format!(
                            "runtime-backed interrupt must resolve through RuntimeSessionAdapter for '{session_id}': {error}"
                        )))
                    }
                };
            }

            // Runtime-backed members must be interrupted through runtime
            // adapter registration truth, not direct session-service fallback.
            self.remove_runtime_session_state(&session_id).await;
            return Err(MobError::Internal(format!(
                "runtime-backed interrupt requested for unregistered runtime session '{session_id}'"
            )));
        }
        self.session_service.interrupt(&session_id).await?;
        Ok(())
    }

    async fn start_turn(
        &self,
        member_ref: &MemberRef,
        req: StartTurnRequest,
    ) -> Result<(), MobError> {
        let session_id = Self::require_session(member_ref, "start turn")?;
        if self.runtime_adapter.is_some() {
            self.ops_adapter
                .report_member_progress(member_ref, "turn dispatched")
                .await?;
            let turn_metadata = meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                handling_mode: Some(req.handling_mode),
                keep_alive: None,
                skill_references: req.skill_references.clone(),
                flow_tool_overlay: req.flow_tool_overlay.clone(),
                additional_instructions: req.additional_instructions.clone(),
                render_metadata: req.render_metadata.clone(),
                ..Default::default()
            };
            let prompt = req.prompt.clone();
            let input = Input::Prompt(PromptInput {
                header: InputHeader {
                    id: meerkat_core::InputId::new(),
                    timestamp: chrono::Utc::now(),
                    source: InputOrigin::Operator,
                    durability: InputDurability::Durable,
                    visibility: InputVisibility::default(),
                    idempotency_key: None,
                    supersession_key: None,
                    correlation_id: None,
                },
                text: prompt.text_content(),
                blocks: if prompt.has_images() {
                    Some(prompt.into_blocks())
                } else {
                    None
                },
                turn_metadata: Some(turn_metadata),
            });
            return self
                .execute_runtime_input(&session_id, input, req.event_tx)
                .await;
        }

        self.session_service
            .start_turn(&session_id, req)
            .await
            .map(|_| ())
            .map_err(Into::into)
    }

    async fn start_flow_step(
        &self,
        member_ref: &MemberRef,
        run_id: &RunId,
        step_id: &crate::StepId,
        req: StartTurnRequest,
    ) -> Result<(), MobError> {
        let session_id = Self::require_session(member_ref, "start flow step")?;
        if self.runtime_adapter.is_some() {
            let input = meerkat_runtime::mob_adapter::create_flow_step_input(
                step_id.as_str(),
                req.prompt.clone(),
                &run_id.to_string(),
                0,
                Some(
                    meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                        handling_mode: Some(req.handling_mode),
                        keep_alive: None,
                        skill_references: req.skill_references.clone(),
                        flow_tool_overlay: req.flow_tool_overlay.clone(),
                        additional_instructions: req.additional_instructions.clone(),
                        render_metadata: req.render_metadata.clone(),
                        ..Default::default()
                    },
                ),
            );
            return self
                .execute_runtime_input(&session_id, input, req.event_tx)
                .await;
        }

        self.start_turn(member_ref, req).await
    }

    async fn interaction_event_injector(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<dyn SubscribableInjector>> {
        self.session_service
            .interaction_event_injector(session_id)
            .await
    }

    async fn is_member_active(&self, member_ref: &MemberRef) -> Result<Option<bool>, MobError> {
        let session_id = match member_ref.session_id() {
            Some(id) => id.clone(),
            None => return Ok(None),
        };
        match self.session_service.read(&session_id).await {
            Ok(view) => Ok(Some(view.state.is_active)),
            Err(meerkat_core::service::SessionError::NotFound { .. }) => Ok(Some(false)),
            Err(error) => Err(error.into()),
        }
    }

    async fn comms_runtime(&self, member_ref: &MemberRef) -> Option<Arc<dyn CoreCommsRuntime>> {
        let session_id = member_ref.session_id()?;
        self.session_service.comms_runtime(session_id).await
    }

    async fn trusted_peer_spec(
        &self,
        member_ref: &MemberRef,
        fallback_name: &str,
        fallback_peer_id: &str,
    ) -> Result<TrustedPeerSpec, MobError> {
        let trusted_peer = Self::trusted_peer_spec(fallback_name, fallback_peer_id)?;
        self.ops_adapter
            .mark_member_peer_ready(member_ref, fallback_name, trusted_peer.clone())
            .await?;
        Ok(trusted_peer)
    }

    async fn active_operation_id_for_member(&self, member_ref: &MemberRef) -> Option<OperationId> {
        let session_id = member_ref.session_id()?;
        self.ops_adapter
            .active_operation_id_for_session(session_id)
            .await
    }

    async fn bind_member_owner_context(
        &self,
        member_ref: &MemberRef,
        owner_session_id: SessionId,
        ops_registry: Arc<dyn OpsLifecycleRegistry>,
    ) -> Result<(), MobError> {
        let Some(session_id) = member_ref.session_id().cloned() else {
            return Err(MobError::Internal(
                "member has no session bridge for canonical ops binding".into(),
            ));
        };
        self.ops_adapter
            .bind_session_registry(session_id, owner_session_id, ops_registry);
        Ok(())
    }

    async fn cancel_all_checkpointers(&self) {
        self.session_service.cancel_all_checkpointers().await;
    }

    async fn rearm_all_checkpointers(&self) {
        self.session_service.rearm_all_checkpointers().await;
    }
}

pub struct ExternalBackend {
    session_service: Arc<dyn MobSessionService>,
    address_base: String,
}

impl ExternalBackend {
    pub fn new(session_service: Arc<dyn MobSessionService>, config: ExternalBackendConfig) -> Self {
        Self {
            session_service,
            address_base: config.address_base.trim_end_matches('/').to_string(),
        }
    }
}

fn is_valid_peer_name_component(component: &str) -> bool {
    if component.is_empty() {
        return false;
    }
    let mut chars = component.chars();
    let first = chars.next().unwrap_or(' ');
    if !first.is_ascii_alphabetic() && first != '_' {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn is_valid_external_peer_name(peer_name: &str) -> bool {
    let mut parts = peer_name.split('/');
    let Some(mob_id) = parts.next() else {
        return false;
    };
    let Some(profile) = parts.next() else {
        return false;
    };
    let Some(meerkat_id) = parts.next() else {
        return false;
    };
    if parts.next().is_some() {
        return false;
    }
    [mob_id, profile, meerkat_id]
        .iter()
        .all(|part| is_valid_peer_name_component(part))
}

pub struct MultiBackendProvisioner {
    session: SessionBackend,
    external: Option<ExternalBackend>,
}

impl MultiBackendProvisioner {
    pub fn new(
        session_service: Arc<dyn MobSessionService>,
        runtime_adapter: Option<Arc<RuntimeSessionAdapter>>,
        external: Option<ExternalBackendConfig>,
    ) -> Self {
        let session = SessionBackend::new(session_service.clone(), runtime_adapter);
        let external = external.map(|cfg| ExternalBackend::new(session_service, cfg));
        Self { session, external }
    }

    async fn external_member_ref(
        &self,
        create_session: CreateSessionRequest,
        peer_name: String,
        owner_session_id: Option<SessionId>,
        ops_registry: Option<Arc<dyn OpsLifecycleRegistry>>,
    ) -> Result<MemberSpawnReceipt, MobError> {
        if !is_valid_external_peer_name(&peer_name) {
            return Err(MobError::WiringError(format!(
                "invalid external peer name '{peer_name}': expected '<mob>/<profile>/<meerkat>' using identifier-safe segments"
            )));
        }
        tracing::debug!(
            peer_name = %peer_name,
            "ExternalBackend::external_member_ref start"
        );
        let external = self
            .external
            .as_ref()
            .ok_or_else(|| MobError::WiringError("external backend is not configured".into()))?;
        let created = external
            .session_service
            .create_session(create_session)
            .await?;
        if let (Some(owner_session_id), Some(registry)) = (owner_session_id, ops_registry) {
            self.session.ops_adapter.bind_session_registry(
                created.session_id.clone(),
                owner_session_id,
                registry,
            );
        }
        let operation_id = self
            .session
            .ops_adapter
            .mark_member_provisioned(&created.session_id, &peer_name)
            .await?;
        tracing::debug!(
            session_id = %created.session_id,
            "ExternalBackend::external_member_ref created session"
        );
        let comms = external
            .session_service
            .comms_runtime(&created.session_id)
            .await
            .ok_or_else(|| {
                MobError::WiringError(format!(
                    "external backend missing comms runtime for '{peer_name}'"
                ))
            })?;
        let peer_id = comms.public_key().ok_or_else(|| {
            MobError::WiringError(format!(
                "external backend missing public key for '{peer_name}'"
            ))
        })?;
        let address = format!("{}/{}", external.address_base, peer_name);
        tracing::debug!(
            peer_id = %peer_id,
            address = %address,
            "ExternalBackend::external_member_ref success"
        );
        Ok(MemberSpawnReceipt {
            member_ref: MemberRef::BackendPeer {
                peer_id,
                address,
                session_id: Some(created.session_id),
            },
            operation_id,
        })
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl MobProvisioner for MultiBackendProvisioner {
    async fn provision_member(
        &self,
        req: ProvisionMemberRequest,
    ) -> Result<MemberSpawnReceipt, MobError> {
        match req.backend {
            MobBackendKind::Session => {
                self.session
                    .provision_member(ProvisionMemberRequest {
                        create_session: req.create_session,
                        backend: MobBackendKind::Session,
                        peer_name: req.peer_name,
                        owner_session_id: req.owner_session_id,
                        ops_registry: req.ops_registry,
                    })
                    .await
            }
            MobBackendKind::External => {
                self.external_member_ref(
                    req.create_session,
                    req.peer_name,
                    req.owner_session_id,
                    req.ops_registry,
                )
                .await
            }
        }
    }

    async fn abort_member_provision(
        &self,
        member_ref: &MemberRef,
        operation_id: &OperationId,
        reason: &str,
    ) -> Result<(), MobError> {
        self.session
            .abort_member_provision(member_ref, operation_id, reason)
            .await
    }

    async fn retire_member(&self, member_ref: &MemberRef) -> Result<(), MobError> {
        self.session.retire_member(member_ref).await
    }

    async fn interrupt_member(&self, member_ref: &MemberRef) -> Result<(), MobError> {
        self.session.interrupt_member(member_ref).await
    }

    async fn start_turn(
        &self,
        member_ref: &MemberRef,
        req: StartTurnRequest,
    ) -> Result<(), MobError> {
        self.session.start_turn(member_ref, req).await
    }

    async fn interaction_event_injector(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<dyn SubscribableInjector>> {
        self.session.interaction_event_injector(session_id).await
    }

    async fn is_member_active(&self, member_ref: &MemberRef) -> Result<Option<bool>, MobError> {
        self.session.is_member_active(member_ref).await
    }

    async fn comms_runtime(&self, member_ref: &MemberRef) -> Option<Arc<dyn CoreCommsRuntime>> {
        self.session.comms_runtime(member_ref).await
    }

    async fn trusted_peer_spec(
        &self,
        member_ref: &MemberRef,
        fallback_name: &str,
        fallback_peer_id: &str,
    ) -> Result<TrustedPeerSpec, MobError> {
        match member_ref {
            MemberRef::Session { .. } => {
                self.session
                    .trusted_peer_spec(member_ref, fallback_name, fallback_peer_id)
                    .await
            }
            MemberRef::BackendPeer {
                peer_id,
                address,
                session_id,
            } => {
                if let Some(session_id) = session_id {
                    // External members still keep a local session bridge; use a sendable
                    // transport address for trust wiring while preserving backend identity
                    // in MemberRef::BackendPeer.address.
                    return self
                        .session
                        .trusted_peer_spec(
                            &MemberRef::Session {
                                session_id: session_id.clone(),
                            },
                            fallback_name,
                            peer_id,
                        )
                        .await;
                }
                TrustedPeerSpec::new(fallback_name, peer_id.clone(), address.clone())
                    .map_err(|error| MobError::WiringError(format!("invalid peer spec: {error}")))
            }
        }
    }

    async fn active_operation_id_for_member(&self, member_ref: &MemberRef) -> Option<OperationId> {
        let session_id = member_ref.session_id()?;
        self.session
            .ops_adapter
            .active_operation_id_for_session(session_id)
            .await
    }

    async fn bind_member_owner_context(
        &self,
        member_ref: &MemberRef,
        owner_session_id: SessionId,
        ops_registry: Arc<dyn OpsLifecycleRegistry>,
    ) -> Result<(), MobError> {
        self.session
            .bind_member_owner_context(member_ref, owner_session_id, ops_registry)
            .await
    }

    async fn cancel_all_checkpointers(&self) {
        self.session.cancel_all_checkpointers().await;
    }

    async fn rearm_all_checkpointers(&self) {
        self.session.rearm_all_checkpointers().await;
    }
}
