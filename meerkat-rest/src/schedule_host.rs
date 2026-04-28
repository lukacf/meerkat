use std::sync::Arc;
#[cfg(feature = "mob")]
use std::time::Duration;

use async_trait::async_trait;
#[cfg(feature = "mob")]
use meerkat::DeliveryTerminal;
use meerkat::surface::{
    ScheduledPromptDispatch, SharedScheduleTargetAdapter, SurfaceScheduleMobHost,
    SurfaceScheduleSessionHost, immediate_delivery_failure, schedule_host_supported,
    spawn_schedule_host,
};
#[cfg(feature = "mob")]
use meerkat::surface::{async_completion_dispatch, immediate_completed_dispatch};
use meerkat::{
    AgentBuildConfig, DeliveryDispatch, MobTargetBinding, Occurrence, OccurrenceFailureClass,
    ScheduleDomainError, SessionMaterializationSpec, SessionService, SessionTargetBinding,
    TargetProbeOutcome,
};
#[cfg(feature = "mob")]
use meerkat::{
    ForkContextSpec, HelperOptionsSpec, ScheduledMobBackendKind, ScheduledMobRuntimeMode,
};
use meerkat_contracts::SkillsParams;
use meerkat_core::service::{
    CreateSessionRequest as SvcCreateSessionRequest, DeferredPromptPolicy, InitialTurnPolicy,
};
#[cfg(feature = "mob")]
use meerkat_core::types::HandlingMode;
use meerkat_core::{ContentInput, SessionId};
#[cfg(feature = "mob")]
use meerkat_mob::{
    AgentIdentity, FlowId, ForkContext, HelperOptions, MobBackendKind, MobId, MobRunStatus, RunId,
};
#[cfg(feature = "mob")]
use meerkat_mob_mcp::MobMcpState;
use meerkat_runtime::{
    CorrelationId, IdempotencyKey, Input, InputDurability, InputHeader, InputOrigin,
    InputVisibility, PromptInput,
};
use tokio::sync::Mutex;
#[cfg(feature = "mob")]
use tokio::time::sleep;

use crate::{
    AppState, RestRuntimeExecutorContext, RestSessionRuntimeExecutor,
    session_metadata_marks_archived,
};

fn materialized_preload_skills(
    preload_skills: &[meerkat_core::skills::SkillKey],
) -> Option<Vec<meerkat_core::skills::SkillKey>> {
    (!preload_skills.is_empty()).then(|| preload_skills.to_vec())
}

#[derive(Default)]
pub struct ScheduleHostState {
    inner: Mutex<Option<meerkat::surface::ScheduleHostHandle>>,
}

#[derive(Clone)]
struct RestScheduleContext {
    runtime: RestRuntimeExecutorContext,
    #[cfg(feature = "mob")]
    mob_state: Arc<MobMcpState>,
}

impl RestScheduleContext {
    fn from_state(state: &AppState) -> Self {
        Self {
            runtime: state.runtime_executor_context(),
            #[cfg(feature = "mob")]
            mob_state: Arc::clone(&state.mob_state),
        }
    }

    async fn ensure_runtime_session_registered(
        &self,
        session_id: &SessionId,
    ) -> Result<(), ScheduleDomainError> {
        // `service.read()` is the single authoritative liveness+presence
        // check post-wave-c. Archived and genuinely-missing sessions both
        // fail to read — both map to "session not found".
        if self.runtime.session_service.read(session_id).await.is_err() {
            return Err(ScheduleDomainError::InvalidSchedule(format!(
                "session not found: {session_id}"
            )));
        }

        let executor = Box::new(RestSessionRuntimeExecutor::new(
            self.runtime.clone(),
            session_id.clone(),
        ));
        self.runtime
            .runtime_adapter
            .ensure_session_with_executor(session_id.clone(), executor)
            .await;
        Ok(())
    }

    async fn session_target_probe(
        &self,
        binding: &SessionTargetBinding,
    ) -> Result<TargetProbeOutcome, ScheduleDomainError> {
        let Some(session_id) = binding.resolved_session_id() else {
            return Ok(TargetProbeOutcome::Ready);
        };

        // `service.read()` is the single authoritative liveness+presence
        // check post-wave-c. Archived sessions and genuinely-missing
        // sessions both fail to read; the scheduler treats both as
        // `Missing`.
        match self.runtime.session_service.read(session_id).await {
            Ok(view) if view.state.is_active => Ok(TargetProbeOutcome::Busy {
                detail: Some(format!("session still running: {session_id}")),
            }),
            Ok(_) => Ok(TargetProbeOutcome::Ready),
            Err(_) => Ok(TargetProbeOutcome::Missing {
                detail: Some(format!("session not found: {session_id}")),
            }),
        }
    }

    async fn materialize_scheduled_session(
        &self,
        create: &SessionMaterializationSpec,
        prompt_system_prompt: Option<&str>,
    ) -> Result<SessionId, ScheduleDomainError> {
        let prepared = meerkat::surface::prepare_surface_session(&self.runtime.runtime_adapter)
            .await
            .map_err(ScheduleDomainError::Internal)?;
        let pre_session = prepared.session;
        let runtime_bindings = prepared.bindings;

        let mut build_config = AgentBuildConfig::new(create.model.clone());
        build_config.provider = create.provider;
        build_config.max_tokens = create.max_tokens;
        build_config.system_prompt = prompt_system_prompt
            .map(str::to_owned)
            .or_else(|| create.system_prompt.clone());
        build_config.output_schema = create.output_schema.clone();
        build_config.structured_output_retries = create.structured_output_retries;
        build_config.provider_params = create.provider_params.clone();
        build_config.comms_name = create.comms_name.clone();
        build_config.peer_meta = create.peer_meta.clone();
        build_config.preload_skills = materialized_preload_skills(&create.preload_skills);
        build_config.additional_instructions = (!create.additional_instructions.is_empty())
            .then(|| create.additional_instructions.clone());
        build_config.realm_id = create
            .realm_id
            .clone()
            .or_else(|| Some(self.runtime.realm.to_string()));
        build_config.instance_id = create
            .instance_id
            .clone()
            .or_else(|| self.runtime.instance_id.clone());
        build_config.backend = create
            .backend
            .clone()
            .or_else(|| Some(self.runtime.backend.clone()));
        build_config.keep_alive = create.keep_alive;
        build_config.app_context = create.app_context.clone();
        build_config.resume_session = Some(pre_session);
        build_config.runtime_build_mode =
            meerkat_core::RuntimeBuildMode::SessionOwned(runtime_bindings);
        build_config.config_generation = self
            .runtime
            .config_runtime
            .get()
            .await
            .ok()
            .map(|snapshot| snapshot.generation)
            .or(create.config_generation);
        if build_config.llm_client_override.is_none()
            && let Some(default_llm_client) = self.runtime.llm_client_override.clone()
        {
            build_config.llm_client_override = Some(default_llm_client);
        }

        let create_req = SvcCreateSessionRequest {
            model: build_config.model.clone(),
            prompt: ContentInput::Text(String::new()),
            render_metadata: None,
            system_prompt: build_config.system_prompt.clone(),
            max_tokens: build_config.max_tokens,
            event_tx: None,
            skill_references: None,
            initial_turn: InitialTurnPolicy::Defer,
            deferred_prompt_policy: DeferredPromptPolicy::Discard,
            build: Some(build_config.to_session_build_options()),
            labels: Some(create.labels.clone()),
        };
        let result = self
            .runtime
            .session_service
            .create_session(create_req)
            .await
            .map_err(|error| ScheduleDomainError::Internal(error.to_string()))?;
        Ok(result.session_id)
    }
}

struct RestScheduleTargetAdapter {
    context: RestScheduleContext,
}

impl RestScheduleTargetAdapter {
    fn new(context: RestScheduleContext) -> Self {
        Self { context }
    }

    #[cfg(feature = "mob")]
    async fn probe_mob_binding(
        &self,
        binding: &MobTargetBinding,
    ) -> Result<TargetProbeOutcome, ScheduleDomainError> {
        let mob_id = MobId::from(mob_binding_mob_id(binding));
        let handle = match self.context.mob_state.handle_for(&mob_id).await {
            Ok(handle) => handle,
            Err(error) => {
                return Ok(TargetProbeOutcome::Missing {
                    detail: Some(error.to_string()),
                });
            }
        };

        match binding {
            MobTargetBinding::Member { member_id, .. } => {
                let identity = AgentIdentity::from(member_id.as_str());
                Ok(if handle.get_member(&identity).await.is_some() {
                    TargetProbeOutcome::Ready
                } else {
                    TargetProbeOutcome::Missing {
                        detail: Some(format!("mob member not found: {member_id}")),
                    }
                })
            }
            MobTargetBinding::Flow { flow_id, .. } => {
                let expected = FlowId::from(flow_id.as_str());
                Ok(
                    if handle.list_flows().into_iter().any(|flow| flow == expected) {
                        TargetProbeOutcome::Ready
                    } else {
                        TargetProbeOutcome::Missing {
                            detail: Some(format!("mob flow not found: {flow_id}")),
                        }
                    },
                )
            }
            MobTargetBinding::SpawnHelper { member_id, .. } => {
                let identity = AgentIdentity::from(member_id.as_str());
                Ok(if handle.get_member(&identity).await.is_some() {
                    TargetProbeOutcome::Busy {
                        detail: Some(format!("mob member already exists: {member_id}")),
                    }
                } else {
                    TargetProbeOutcome::Ready
                })
            }
            MobTargetBinding::ForkHelper {
                source_member_id,
                member_id,
                ..
            } => {
                let source = AgentIdentity::from(source_member_id.as_str());
                if handle.get_member(&source).await.is_none() {
                    return Ok(TargetProbeOutcome::Missing {
                        detail: Some(format!("mob source member not found: {source_member_id}")),
                    });
                }
                let target = AgentIdentity::from(member_id.as_str());
                Ok(if handle.get_member(&target).await.is_some() {
                    TargetProbeOutcome::Busy {
                        detail: Some(format!("mob member already exists: {member_id}")),
                    }
                } else {
                    TargetProbeOutcome::Ready
                })
            }
        }
    }
}

fn scheduled_materialization_preload_skills(
    create: &SessionMaterializationSpec,
) -> Option<Vec<meerkat_core::skills::SkillKey>> {
    (!create.preload_skills.is_empty()).then(|| create.preload_skills.clone())
}

#[async_trait]
impl SurfaceScheduleSessionHost for RestScheduleTargetAdapter {
    async fn probe_session_target(
        &self,
        binding: &SessionTargetBinding,
    ) -> Result<TargetProbeOutcome, ScheduleDomainError> {
        self.context.session_target_probe(binding).await
    }

    async fn materialize_session(
        &self,
        create: &SessionMaterializationSpec,
        prompt_system_prompt: Option<&str>,
    ) -> Result<SessionId, ScheduleDomainError> {
        self.context
            .materialize_scheduled_session(create, prompt_system_prompt)
            .await
    }

    async fn deliver_prompt(
        &self,
        session_id: &SessionId,
        occurrence: &Occurrence,
        dispatch: ScheduledPromptDispatch,
    ) -> Result<DeliveryDispatch, ScheduleDomainError> {
        deliver_scheduled_prompt(&self.context, session_id, occurrence, dispatch).await
    }

    async fn deliver_event(
        &self,
        session_id: &SessionId,
        occurrence: &Occurrence,
        event_type: String,
        payload: serde_json::Value,
        render_metadata: Option<meerkat_core::types::RenderMetadata>,
        materialized_session_id: Option<SessionId>,
    ) -> Result<DeliveryDispatch, ScheduleDomainError> {
        deliver_scheduled_event(
            &self.context,
            session_id,
            occurrence,
            event_type,
            payload,
            render_metadata,
            materialized_session_id,
        )
        .await
    }
}

#[async_trait]
impl SurfaceScheduleMobHost for RestScheduleTargetAdapter {
    async fn probe_mob_target(
        &self,
        binding: &MobTargetBinding,
    ) -> Result<TargetProbeOutcome, ScheduleDomainError> {
        #[cfg(feature = "mob")]
        {
            self.probe_mob_binding(binding).await
        }

        #[cfg(not(feature = "mob"))]
        {
            let _ = binding;
            Ok(TargetProbeOutcome::Missing {
                detail: Some("scheduled mob targets are not yet available on the REST host".into()),
            })
        }
    }

    async fn deliver_mob_target(
        &self,
        occurrence: &Occurrence,
        binding: &MobTargetBinding,
    ) -> Result<DeliveryDispatch, ScheduleDomainError> {
        #[cfg(feature = "mob")]
        {
            let mob_state = Arc::clone(&self.context.mob_state);
            let mob_id = MobId::from(mob_binding_mob_id(binding));
            return match binding {
                MobTargetBinding::Member {
                    member_id,
                    action:
                        meerkat::ScheduledMobAction::Send {
                            content,
                            render_metadata,
                        },
                    ..
                } => match mob_state
                    .mob_member_send(
                        &mob_id,
                        AgentIdentity::from(member_id.as_str()),
                        content.clone(),
                        HandlingMode::Queue,
                        render_metadata.clone(),
                    )
                    .await
                {
                    Ok(receipt) => Ok(immediate_completed_dispatch(
                        occurrence,
                        Some(receipt.identity.to_string()),
                    )),
                    Err(error) => Ok(immediate_delivery_failure(
                        occurrence,
                        error.to_string(),
                        OccurrenceFailureClass::MobRejected,
                        None,
                        None,
                    )),
                },
                MobTargetBinding::Flow {
                    flow_id, params, ..
                } => {
                    let params: serde_json::Value =
                        serde_json::from_str(params.get()).map_err(|error| {
                            ScheduleDomainError::InvalidSchedule(format!(
                                "invalid mob flow params: {error}"
                            ))
                        })?;
                    match mob_state
                        .mob_run_flow(&mob_id, FlowId::from(flow_id.as_str()), params)
                        .await
                    {
                        Ok(run_id) => Ok(async_completion_dispatch(
                            occurrence,
                            Some(run_id.to_string()),
                            mob_flow_completion_future(mob_state, mob_id, run_id),
                        )),
                        Err(error) => Ok(immediate_delivery_failure(
                            occurrence,
                            error.to_string(),
                            OccurrenceFailureClass::MobRejected,
                            None,
                            None,
                        )),
                    }
                }
                MobTargetBinding::SpawnHelper {
                    member_id,
                    prompt,
                    options,
                    ..
                } => {
                    let identity = AgentIdentity::from(member_id.as_str());
                    let helper_options = helper_options_from_spec(options)?;
                    let prompt = prompt.clone();
                    Ok(async_completion_dispatch(
                        occurrence,
                        Some(member_id.clone()),
                        Box::pin(async move {
                            match mob_state
                                .mob_spawn_helper(&mob_id, identity, prompt, helper_options)
                                .await
                            {
                                Ok(_) => Ok(DeliveryTerminal::completed(None)),
                                Err(error) => Ok(mob_delivery_failed_terminal(error)),
                            }
                        }),
                    ))
                }
                MobTargetBinding::ForkHelper {
                    source_member_id,
                    member_id,
                    prompt,
                    fork_context,
                    options,
                    ..
                } => {
                    let source_identity = AgentIdentity::from(source_member_id.as_str());
                    let identity = AgentIdentity::from(member_id.as_str());
                    let helper_options = helper_options_from_spec(options)?;
                    let fork_context = fork_context_from_spec(fork_context);
                    let prompt = prompt.clone();
                    Ok(async_completion_dispatch(
                        occurrence,
                        Some(member_id.clone()),
                        Box::pin(async move {
                            match mob_state
                                .mob_fork_helper(
                                    &mob_id,
                                    &source_identity,
                                    identity,
                                    prompt,
                                    fork_context,
                                    helper_options,
                                )
                                .await
                            {
                                Ok(_) => Ok(DeliveryTerminal::completed(None)),
                                Err(error) => Ok(mob_delivery_failed_terminal(error)),
                            }
                        }),
                    ))
                }
            };
        }

        #[cfg(not(feature = "mob"))]
        {
            let _ = binding;
            Ok(immediate_delivery_failure(
                occurrence,
                "scheduled mob targets are not yet available on the REST host".to_string(),
                OccurrenceFailureClass::MobRejected,
                None,
                None,
            ))
        }
    }
}

impl AppState {
    pub async fn ensure_schedule_host_started(&self) -> Result<(), ScheduleDomainError> {
        if !schedule_host_supported(self.schedule_service.store().kind()) {
            return Ok(());
        }

        let mut slot = self.schedule_host.inner.lock().await;
        if slot.is_some() {
            return Ok(());
        }

        let surface_adapter = Arc::new(RestScheduleTargetAdapter::new(
            RestScheduleContext::from_state(self),
        ));
        let session_host: Arc<dyn SurfaceScheduleSessionHost> = surface_adapter.clone();
        let mob_host: Arc<dyn SurfaceScheduleMobHost> = surface_adapter;
        let shared_adapter = Arc::new(SharedScheduleTargetAdapter::new(
            self.schedule_service.clone(),
            session_host,
            mob_host,
        ));
        *slot = Some(spawn_schedule_host(
            self.schedule_service.clone(),
            shared_adapter,
            self.schedule_owner_id(),
        ));
        Ok(())
    }

    pub async fn shutdown_schedule_host(&self) {
        let mut slot = self.schedule_host.inner.lock().await;
        let Some(handle) = slot.take() else {
            return;
        };
        drop(slot);
        handle.shutdown().await;
    }

    fn schedule_owner_id(&self) -> String {
        let instance = self.instance_id.as_deref().unwrap_or("rest");
        format!("rest-scheduler:{}:{instance}", self.realm)
    }
}

async fn deliver_scheduled_prompt(
    _context: &RestScheduleContext,
    _session_id: &SessionId,
    _occurrence: &Occurrence,
    _dispatch: ScheduledPromptDispatch,
) -> Result<DeliveryDispatch, ScheduleDomainError> {
    Err(ScheduleDomainError::Internal(
        "rest deliver_prompt no longer reinterprets runtime terminal classes into schedule-local failure classes; the schedule surface must consume the runtime's typed CompletionOutcome directly".to_string(),
    ))
}

async fn deliver_scheduled_event(
    _context: &RestScheduleContext,
    _session_id: &SessionId,
    _occurrence: &Occurrence,
    _event_type: String,
    _payload: serde_json::Value,
    _render_metadata: Option<meerkat_core::types::RenderMetadata>,
    _materialized_session_id: Option<SessionId>,
) -> Result<DeliveryDispatch, ScheduleDomainError> {
    Err(ScheduleDomainError::Internal(
        "rest deliver_event no longer reinterprets runtime terminal classes into schedule-local failure classes; the schedule surface must consume the runtime's typed CompletionOutcome directly".to_string(),
    ))
}

#[cfg(feature = "comms")]
async fn update_peer_ingress_context(context: &RestScheduleContext, session_id: &SessionId) {
    // Post-wave-c the raw `load_persisted` escape hatch is gone.
    // `SessionInfo` (returned by `service.read()`) does not surface
    // `keep_alive`, so we default to `false` — matches the pre-retype
    // `.unwrap_or(false)` fallback. Sessions that need comms-driven
    // keep-alive configure it through the canonical
    // `SessionBuildOptions.keep_alive` on create.
    let keep_alive = false;
    let comms_rt = context
        .runtime
        .session_service
        .comms_runtime(session_id)
        .await;
    context
        .runtime
        .runtime_adapter
        .update_peer_ingress_context(session_id, keep_alive, comms_rt)
        .await;
}

#[cfg(not(feature = "comms"))]
async fn update_peer_ingress_context(_context: &RestScheduleContext, _session_id: &SessionId) {}

#[cfg(feature = "mob")]
fn mob_binding_mob_id(binding: &MobTargetBinding) -> &str {
    match binding {
        MobTargetBinding::Member { mob_id, .. }
        | MobTargetBinding::Flow { mob_id, .. }
        | MobTargetBinding::SpawnHelper { mob_id, .. }
        | MobTargetBinding::ForkHelper { mob_id, .. } => mob_id,
    }
}

#[cfg(feature = "mob")]
fn helper_options_from_spec(
    spec: &HelperOptionsSpec,
) -> Result<HelperOptions, ScheduleDomainError> {
    let mut options = HelperOptions::default();
    options.role_name = spec.role_name.clone().map(Into::into);
    options.runtime_mode = spec.runtime_mode.map(|mode| match mode {
        ScheduledMobRuntimeMode::AutonomousHost => meerkat_mob::MobRuntimeMode::AutonomousHost,
        ScheduledMobRuntimeMode::TurnDriven => meerkat_mob::MobRuntimeMode::TurnDriven,
    });
    options.backend = spec.backend.map(|backend| match backend {
        ScheduledMobBackendKind::Session => MobBackendKind::Session,
        ScheduledMobBackendKind::External => MobBackendKind::External,
    });
    options.tool_access_policy = spec.tool_access_policy.clone();
    Ok(options)
}

#[cfg(feature = "mob")]
fn fork_context_from_spec(spec: &ForkContextSpec) -> ForkContext {
    match spec {
        ForkContextSpec::FullHistory => ForkContext::FullHistory,
        ForkContextSpec::LastMessages { count } => ForkContext::LastMessages { count: *count },
    }
}

#[cfg(feature = "mob")]
fn mob_delivery_failed_terminal(error: meerkat_mob::MobError) -> DeliveryTerminal {
    DeliveryTerminal::delivery_failed(error.to_string(), OccurrenceFailureClass::MobRejected)
}

#[cfg(feature = "mob")]
fn mob_flow_completion_future(
    mob_state: Arc<MobMcpState>,
    mob_id: MobId,
    run_id: RunId,
) -> meerkat::DeliveryCompletion {
    Box::pin(async move {
        loop {
            match mob_state.mob_flow_status(&mob_id, run_id.clone()).await {
                Ok(Some(run)) if run.status == MobRunStatus::Completed => {
                    return Ok(DeliveryTerminal::completed(None));
                }
                Ok(Some(run)) if run.status.is_terminal() => {
                    return Ok(DeliveryTerminal::delivery_failed(
                        format!("mob flow terminated as {:?}", run.status),
                        OccurrenceFailureClass::MobRejected,
                    ));
                }
                Ok(Some(_) | None) => sleep(Duration::from_millis(100)).await,
                Err(error) => return Ok(mob_delivery_failed_terminal(error)),
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat_core::skills::{SkillKey, SkillName, SourceUuid};

    fn fixture_skill_key(name: &str) -> SkillKey {
        let skill_name = SkillName::parse(name).expect("fixture skill name should be valid");
        SkillKey::new(SourceUuid::builtin(), skill_name)
    }

    #[test]
    fn materialized_preload_skills_preserves_typed_skill_keys() {
        let key = fixture_skill_key("email");

        assert_eq!(
            materialized_preload_skills(std::slice::from_ref(&key)),
            Some(vec![key])
        );
    }

    #[test]
    fn materialized_preload_skills_leaves_empty_preload_unset() {
        let preload_skills: Vec<SkillKey> = Vec::new();

        assert_eq!(materialized_preload_skills(&preload_skills), None);
    }
}
