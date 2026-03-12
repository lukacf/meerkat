use super::*;
use crate::MobBackendKind;
use crate::definition::ExternalBackendConfig;
use crate::event::MemberRef;
#[cfg(target_arch = "wasm32")]
use crate::tokio;
use async_trait::async_trait;
use meerkat_core::comms::TrustedPeerSpec;
use meerkat_core::event_injector::SubscribableInjector;
use meerkat_core::lifecycle::RunId as CoreRunId;
use meerkat_core::lifecycle::core_executor::{CoreApplyOutput, CoreExecutor, CoreExecutorError};
use meerkat_core::lifecycle::run_control::RunControlCommand;
use meerkat_core::lifecycle::run_primitive::{CoreRenderable, RunApplyBoundary, RunPrimitive};
use meerkat_core::service::{CreateSessionRequest, SessionError, StartTurnRequest};
use meerkat_core::types::SessionId;
#[allow(unused_imports)]
use meerkat_runtime::service_ext::SessionServiceRuntimeExt as _;
use meerkat_runtime::{
    Input, InputDurability, InputHeader, InputOrigin, InputVisibility, PromptInput,
    RuntimeDriverError, RuntimeSessionAdapter, RuntimeState,
};
use std::collections::{HashMap, VecDeque};
use tokio::sync::{Mutex, RwLock};

pub struct ProvisionMemberRequest {
    pub create_session: CreateSessionRequest,
    pub backend: MobBackendKind,
    pub peer_name: String,
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
pub trait MobProvisioner: Send + Sync {
    async fn provision_member(&self, req: ProvisionMemberRequest) -> Result<MemberRef, MobError>;
    async fn retire_member(&self, member_ref: &MemberRef) -> Result<(), MobError>;
    async fn reset_member(&self, member_ref: &MemberRef) -> Result<(), MobError>;
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

    /// Cancel all active checkpointer gates so in-flight saves complete but
    /// subsequent checkpoints are no-ops. Call during mob stop.
    async fn cancel_all_checkpointers(&self) {}

    /// Re-enable checkpointer gates after a prior cancel. Call during mob resume.
    async fn rearm_all_checkpointers(&self) {}
}

pub struct SubagentBackend {
    session_service: Arc<dyn MobSessionService>,
    runtime_adapter: Option<Arc<RuntimeSessionAdapter>>,
    runtime_sessions: RwLock<HashMap<SessionId, Arc<RuntimeSessionState>>>,
}

impl SubagentBackend {
    pub fn new(
        session_service: Arc<dyn MobSessionService>,
        runtime_adapter: Option<Arc<RuntimeSessionAdapter>>,
    ) -> Self {
        Self {
            session_service,
            runtime_adapter,
            runtime_sessions: RwLock::new(HashMap::new()),
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
            let executor = Box::new(MobSessionRuntimeExecutor::new(
                self.session_service.clone(),
                Arc::clone(adapter),
                session_id.clone(),
                existing.clone(),
            ));
            adapter
                .ensure_session_with_executor(session_id.clone(), executor)
                .await;
            return Some(existing);
        }
        let state = Arc::new(RuntimeSessionState {
            queued_turns: Mutex::new(VecDeque::new()),
        });
        let executor = Box::new(MobSessionRuntimeExecutor::new(
            self.session_service.clone(),
            Arc::clone(adapter),
            session_id.clone(),
            state.clone(),
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

    async fn execute_runtime_input(
        &self,
        session_id: &SessionId,
        input: Input,
        _event_tx: Option<
            tokio::sync::mpsc::Sender<meerkat_core::EventEnvelope<meerkat_core::AgentEvent>>,
        >,
    ) -> Result<(), MobError> {
        let adapter = self.runtime_adapter.as_ref().ok_or_else(|| {
            MobError::Internal(format!(
                "runtime-backed turn requested without runtime adapter: {session_id}"
            ))
        })?;
        let _ = self.runtime_session_state(session_id).await;
        let adapter_session_id = session_id.clone();

        let (_outcome, handle) = adapter
            .accept_input_with_completion(&adapter_session_id, input)
            .await
            .map_err(|err| MobError::Internal(err.to_string()))?;

        // Terminal dedup: input already processed — idempotent success
        let Some(handle) = handle else {
            return Ok(());
        };

        match handle.wait().await {
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
    queued_turns: Mutex<VecDeque<StartTurnRequest>>,
}

impl RuntimeSessionState {
    async fn clear_queued_turns(&self) {
        self.queued_turns.lock().await.clear();
    }
}

struct MobSessionRuntimeExecutor {
    session_service: Arc<dyn MobSessionService>,
    runtime_adapter: Arc<RuntimeSessionAdapter>,
    session_id: SessionId,
    state: Arc<RuntimeSessionState>,
}

impl MobSessionRuntimeExecutor {
    fn new(
        session_service: Arc<dyn MobSessionService>,
        runtime_adapter: Arc<RuntimeSessionAdapter>,
        session_id: SessionId,
        state: Arc<RuntimeSessionState>,
    ) -> Self {
        Self {
            session_service,
            runtime_adapter,
            session_id,
            state,
        }
    }
}

fn extract_prompt(primitive: &RunPrimitive) -> String {
    match primitive {
        RunPrimitive::StagedInput(staged) => staged
            .appends
            .iter()
            .filter_map(|append| match &append.content {
                CoreRenderable::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        RunPrimitive::ImmediateAppend(append) => match &append.content {
            CoreRenderable::Text { text } => text.clone(),
            _ => String::new(),
        },
        RunPrimitive::ImmediateContextAppend(append) => match &append.content {
            CoreRenderable::Text { text } => text.clone(),
            _ => String::new(),
        },
        _ => String::new(),
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
        let req = {
            let mut queued_turns = self.state.queued_turns.lock().await;
            queued_turns.pop_front().unwrap_or(StartTurnRequest {
                prompt: extract_prompt(&primitive),
                event_tx: None,
                host_mode: primitive.turn_metadata().is_some_and(|meta| meta.host_mode),
                skill_references: primitive
                    .turn_metadata()
                    .and_then(|meta| meta.skill_references.clone()),
                flow_tool_overlay: primitive
                    .turn_metadata()
                    .and_then(|meta| meta.flow_tool_overlay.clone()),
                additional_instructions: primitive
                    .turn_metadata()
                    .and_then(|meta| meta.additional_instructions.clone()),
            })
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
                primitive.contributing_input_ids().to_vec(),
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
                self.state.clear_queued_turns().await;
                self.runtime_adapter
                    .unregister_session(&self.session_id)
                    .await;
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
impl MobProvisioner for SubagentBackend {
    async fn provision_member(&self, req: ProvisionMemberRequest) -> Result<MemberRef, MobError> {
        tracing::debug!(
            backend = ?req.backend,
            peer_name = %req.peer_name,
            "SubagentBackend::provision_member start"
        );
        let created = self
            .session_service
            .create_session(req.create_session)
            .await?;
        if self.runtime_adapter.is_some() {
            let _ = self.runtime_session_state(&created.session_id).await;
        }
        tracing::debug!(
            session_id = %created.session_id,
            "SubagentBackend::provision_member created session"
        );
        Ok(MemberRef::from_session_id(created.session_id))
    }

    async fn retire_member(&self, member_ref: &MemberRef) -> Result<(), MobError> {
        let session_id = Self::require_session(member_ref, "retire")?;
        if let Some(adapter) = &self.runtime_adapter {
            let _ = self.runtime_session_state(&session_id).await;
            adapter
                .retire_runtime(&session_id)
                .await
                .map_err(|err| MobError::Internal(err.to_string()))?;
            adapter.unregister_session(&session_id).await;
            self.runtime_sessions.write().await.remove(&session_id);
        }
        self.session_service.archive(&session_id).await?;
        Ok(())
    }

    async fn interrupt_member(&self, member_ref: &MemberRef) -> Result<(), MobError> {
        let session_id = Self::require_session(member_ref, "interrupt")?;
        if let Some(adapter) = &self.runtime_adapter {
            let _ = self.runtime_session_state(&session_id).await;
            if adapter.interrupt_current_run(&session_id).await.is_ok() {
                return Ok(());
            }
        }
        self.session_service.interrupt(&session_id).await?;
        Ok(())
    }

    async fn reset_member(&self, member_ref: &MemberRef) -> Result<(), MobError> {
        let session_id = Self::require_session(member_ref, "reset")?;
        let adapter = self.runtime_adapter.as_ref().ok_or_else(|| {
            MobError::Internal(format!(
                "session-backed provisioner cannot reset member without runtime adapter: {member_ref:?}"
            ))
        })?;
        let runtime_state = self.runtime_session_state(&session_id).await;
        match adapter.reset_runtime(&session_id).await {
            Ok(_) => {}
            Err(RuntimeDriverError::NotReady {
                state: RuntimeState::Running,
            }) => {
                if adapter.interrupt_current_run(&session_id).await.is_err() {
                    let _ = self.session_service.interrupt(&session_id).await;
                }

                let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
                loop {
                    let runtime_state = adapter
                        .runtime_state(&session_id)
                        .await
                        .map_err(|err| MobError::Internal(err.to_string()))?;
                    if !matches!(runtime_state, RuntimeState::Running) {
                        break;
                    }
                    if tokio::time::Instant::now() >= deadline {
                        return Err(MobError::Internal(format!(
                            "timed out waiting for member runtime to stop before reset: {session_id}"
                        )));
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }

                adapter
                    .reset_runtime(&session_id)
                    .await
                    .map_err(|err| MobError::Internal(err.to_string()))?;
            }
            Err(err) => return Err(MobError::Internal(err.to_string())),
        }
        if let Some(state) = runtime_state {
            state.clear_queued_turns().await;
        }
        Ok(())
    }

    async fn start_turn(
        &self,
        member_ref: &MemberRef,
        req: StartTurnRequest,
    ) -> Result<(), MobError> {
        let session_id = Self::require_session(member_ref, "start turn")?;
        if self.runtime_adapter.is_some() {
            let turn_metadata = meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                host_mode: req.host_mode,
                skill_references: req.skill_references.clone(),
                flow_tool_overlay: req.flow_tool_overlay.clone(),
                additional_instructions: req.additional_instructions.clone(),
            };
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
                text: req.prompt,
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
                &req.prompt,
                &run_id.to_string(),
                0,
                Some(
                    meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                        host_mode: req.host_mode,
                        skill_references: req.skill_references.clone(),
                        flow_tool_overlay: req.flow_tool_overlay.clone(),
                        additional_instructions: req.additional_instructions.clone(),
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
        _member_ref: &MemberRef,
        fallback_name: &str,
        fallback_peer_id: &str,
    ) -> Result<TrustedPeerSpec, MobError> {
        Self::trusted_peer_spec(fallback_name, fallback_peer_id)
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
    subagent: SubagentBackend,
    external: Option<ExternalBackend>,
}

impl MultiBackendProvisioner {
    pub fn new(
        session_service: Arc<dyn MobSessionService>,
        runtime_adapter: Option<Arc<RuntimeSessionAdapter>>,
        external: Option<ExternalBackendConfig>,
    ) -> Self {
        let subagent = SubagentBackend::new(session_service.clone(), runtime_adapter);
        let external = external.map(|cfg| ExternalBackend::new(session_service, cfg));
        Self { subagent, external }
    }

    async fn external_member_ref(
        &self,
        create_session: CreateSessionRequest,
        peer_name: String,
    ) -> Result<MemberRef, MobError> {
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
        Ok(MemberRef::BackendPeer {
            peer_id,
            address,
            session_id: Some(created.session_id),
        })
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl MobProvisioner for MultiBackendProvisioner {
    async fn provision_member(&self, req: ProvisionMemberRequest) -> Result<MemberRef, MobError> {
        match req.backend {
            MobBackendKind::Subagent => {
                self.subagent
                    .provision_member(ProvisionMemberRequest {
                        create_session: req.create_session,
                        backend: MobBackendKind::Subagent,
                        peer_name: req.peer_name,
                    })
                    .await
            }
            MobBackendKind::External => {
                self.external_member_ref(req.create_session, req.peer_name)
                    .await
            }
        }
    }

    async fn retire_member(&self, member_ref: &MemberRef) -> Result<(), MobError> {
        self.subagent.retire_member(member_ref).await
    }

    async fn reset_member(&self, member_ref: &MemberRef) -> Result<(), MobError> {
        self.subagent.reset_member(member_ref).await
    }

    async fn interrupt_member(&self, member_ref: &MemberRef) -> Result<(), MobError> {
        self.subagent.interrupt_member(member_ref).await
    }

    async fn start_turn(
        &self,
        member_ref: &MemberRef,
        req: StartTurnRequest,
    ) -> Result<(), MobError> {
        self.subagent.start_turn(member_ref, req).await
    }

    async fn interaction_event_injector(
        &self,
        session_id: &SessionId,
    ) -> Option<Arc<dyn SubscribableInjector>> {
        self.subagent.interaction_event_injector(session_id).await
    }

    async fn is_member_active(&self, member_ref: &MemberRef) -> Result<Option<bool>, MobError> {
        self.subagent.is_member_active(member_ref).await
    }

    async fn comms_runtime(&self, member_ref: &MemberRef) -> Option<Arc<dyn CoreCommsRuntime>> {
        self.subagent.comms_runtime(member_ref).await
    }

    async fn trusted_peer_spec(
        &self,
        member_ref: &MemberRef,
        fallback_name: &str,
        fallback_peer_id: &str,
    ) -> Result<TrustedPeerSpec, MobError> {
        match member_ref {
            MemberRef::Session { .. } => {
                self.subagent
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
                        .subagent
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

    async fn cancel_all_checkpointers(&self) {
        self.subagent.cancel_all_checkpointers().await;
    }

    async fn rearm_all_checkpointers(&self) {
        self.subagent.rearm_all_checkpointers().await;
    }
}
