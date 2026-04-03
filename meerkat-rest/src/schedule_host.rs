use crate::{
    AppState, RestRuntimeExecutorContext, RestSessionRuntimeExecutor,
    session_metadata_marks_archived,
};
use async_trait::async_trait;
use chrono::Duration as ChronoDuration;
use meerkat::{
    AgentBuildConfig, DeliveryDispatch, DeliveryReceipt, DeliveryReceiptStage, DeliveryTerminal,
    MobTargetBinding, Occurrence, OccurrenceFailureClass, OccurrencePhase, ScheduleDomainError,
    ScheduleDriver, ScheduleDriverConfig, ScheduleStoreKind, ScheduleTargetDelivery,
    ScheduleTargetProbe, ScheduledSessionAction, SessionMaterializationSpec, SessionService,
    SessionTargetBinding, TargetBinding, TargetProbeOutcome, surface::prepare_surface_session,
};
#[cfg(feature = "mob")]
use meerkat::{
    ForkContextSpec, HelperOptionsSpec, ScheduledMobBackendKind, ScheduledMobRuntimeMode,
};
use meerkat_contracts::SkillsParams;
use meerkat_core::service::{
    CreateSessionRequest as SvcCreateSessionRequest, DeferredPromptPolicy, InitialTurnPolicy,
};
use meerkat_core::skills::SkillRef;
#[cfg(feature = "mob")]
use meerkat_core::types::HandlingMode;
use meerkat_core::{ContentInput, OutputSchema, SessionId};
#[cfg(feature = "mob")]
use meerkat_mob::{
    FlowId, ForkContext, HelperOptions, MeerkatId, MobBackendKind, MobId, MobRunStatus, RunId,
};
#[cfg(feature = "mob")]
use meerkat_mob_mcp::MobMcpState;
use meerkat_runtime::completion::{CompletionHandle, CompletionOutcome};
use meerkat_runtime::{
    AcceptOutcome, CorrelationId, IdempotencyKey, Input, InputDurability, InputHeader, InputOrigin,
    InputVisibility, PromptInput,
};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, oneshot};
#[cfg(feature = "mob")]
use tokio::time::sleep;

pub struct ScheduleHostHandle {
    shutdown_tx: Option<oneshot::Sender<()>>,
    join: tokio::task::JoinHandle<()>,
}

impl ScheduleHostHandle {
    async fn shutdown(mut self) {
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }
        let _ = self.join.await;
    }
}

#[derive(Default)]
pub struct ScheduleHostState {
    inner: Mutex<Option<ScheduleHostHandle>>,
}

#[derive(Clone)]
struct RestScheduleContext {
    runtime: RestRuntimeExecutorContext,
    schedule_service: meerkat::ScheduleService,
    #[cfg(feature = "mob")]
    mob_state: Arc<MobMcpState>,
}

struct RestScheduleTargetAdapter {
    context: RestScheduleContext,
}

struct ResolvedScheduledSession {
    session_id: SessionId,
    materialized_session_id: Option<SessionId>,
    allow_system_prompt_override: bool,
}

struct AcceptedScheduledInput {
    correlation_id: String,
    handle: Option<CompletionHandle>,
}

impl RestScheduleContext {
    fn from_state(state: &AppState) -> Self {
        Self {
            runtime: state.runtime_executor_context(),
            schedule_service: state.schedule_service.clone(),
            #[cfg(feature = "mob")]
            mob_state: Arc::clone(&state.mob_state),
        }
    }

    async fn ensure_runtime_session_registered(
        &self,
        session_id: &SessionId,
    ) -> Result<(), ScheduleDomainError> {
        let persisted = self
            .runtime
            .session_service
            .load_persisted(session_id)
            .await
            .ok()
            .flatten();
        if persisted
            .as_ref()
            .is_some_and(session_metadata_marks_archived)
        {
            return Err(ScheduleDomainError::DeliveryFailed {
                occurrence_id: meerkat::OccurrenceId::new(),
                reason: format!("session not found: {session_id}"),
            });
        }

        let session_exists = if persisted.is_some() {
            true
        } else {
            self.runtime.session_service.read(session_id).await.is_ok()
        };
        if !session_exists {
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

        if let Ok(summaries) = self.runtime.session_service.list(Default::default()).await
            && let Some(summary) = summaries
                .into_iter()
                .find(|summary| summary.session_id == *session_id)
        {
            return Ok(if summary.is_active {
                TargetProbeOutcome::Busy {
                    detail: Some(format!("session still running: {session_id}")),
                }
            } else {
                TargetProbeOutcome::Ready
            });
        }

        let persisted = self
            .runtime
            .session_service
            .load_persisted(session_id)
            .await
            .map_err(|error| ScheduleDomainError::ProbeFailed(error.to_string()))?;
        match persisted {
            Some(session) if !session_metadata_marks_archived(&session) => {
                Ok(TargetProbeOutcome::Ready)
            }
            _ => Ok(TargetProbeOutcome::Missing {
                detail: Some(format!("session not found: {session_id}")),
            }),
        }
    }

    async fn materialize_scheduled_session(
        &self,
        create: &SessionMaterializationSpec,
        prompt_system_prompt: Option<&str>,
    ) -> Result<SessionId, ScheduleDomainError> {
        let prepared = prepare_surface_session(&self.runtime.runtime_adapter)
            .await
            .map_err(ScheduleDomainError::Internal)?;
        let pre_session = prepared.session;
        let ops_lifecycle = prepared.ops_lifecycle;

        let mut build_config = AgentBuildConfig::new(create.model.clone());
        build_config.provider = create.provider;
        build_config.max_tokens = create.max_tokens;
        build_config.system_prompt = prompt_system_prompt
            .map(str::to_owned)
            .or_else(|| create.system_prompt.clone());
        if let Some(output_schema_json) = create.output_schema_json.clone() {
            build_config.output_schema = Some(
                OutputSchema::from_json_value(output_schema_json)
                    .map_err(|error| ScheduleDomainError::InvalidSchedule(error.to_string()))?,
            );
        }
        build_config.structured_output_retries = create.structured_output_retries;
        build_config.provider_params = create.provider_params.clone();
        build_config.comms_name = create.comms_name.clone();
        build_config.peer_meta = create.peer_meta.clone();
        build_config.preload_skills = (!create.preload_skills.is_empty()).then(|| {
            create
                .preload_skills
                .iter()
                .cloned()
                .map(Into::into)
                .collect()
        });
        build_config.additional_instructions = (!create.additional_instructions.is_empty())
            .then(|| create.additional_instructions.clone());
        build_config.realm_id = create
            .realm_id
            .clone()
            .or_else(|| Some(self.runtime.realm_id.clone()));
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
        build_config.ops_lifecycle_override = Some(ops_lifecycle);
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
                let meerkat_id = MeerkatId::from(member_id.as_str());
                Ok(if handle.get_member(&meerkat_id).await.is_some() {
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
                let meerkat_id = MeerkatId::from(member_id.as_str());
                Ok(if handle.get_member(&meerkat_id).await.is_some() {
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
                let source = MeerkatId::from(source_member_id.as_str());
                if handle.get_member(&source).await.is_none() {
                    return Ok(TargetProbeOutcome::Missing {
                        detail: Some(format!("mob source member not found: {source_member_id}")),
                    });
                }
                let target = MeerkatId::from(member_id.as_str());
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

    #[cfg(not(feature = "mob"))]
    async fn probe_mob_binding(
        &self,
        _binding: &MobTargetBinding,
    ) -> Result<TargetProbeOutcome, ScheduleDomainError> {
        Ok(TargetProbeOutcome::Missing {
            detail: Some("scheduled mob targets are not yet available on the REST host".into()),
        })
    }

    async fn resolve_session(
        &self,
        occurrence: &Occurrence,
        binding: &SessionTargetBinding,
    ) -> Result<ResolvedScheduledSession, DeliveryDispatch> {
        match binding {
            SessionTargetBinding::ExactSession { session_id, .. }
            | SessionTargetBinding::ResumableSession { session_id, .. } => {
                Ok(ResolvedScheduledSession {
                    session_id: session_id.clone(),
                    materialized_session_id: None,
                    allow_system_prompt_override: false,
                })
            }
            SessionTargetBinding::MaterializeOnDemandSession {
                bound_session_id: Some(session_id),
                ..
            } => Ok(ResolvedScheduledSession {
                session_id: session_id.clone(),
                materialized_session_id: Some(session_id.clone()),
                allow_system_prompt_override: false,
            }),
            SessionTargetBinding::MaterializeOnDemandSession {
                create,
                action,
                bound_session_id: None,
            } => {
                let prompt_system_prompt = match action {
                    ScheduledSessionAction::Prompt { system_prompt, .. } => {
                        system_prompt.as_deref()
                    }
                    ScheduledSessionAction::Event { .. } => None,
                };
                match self
                    .context
                    .materialize_scheduled_session(create, prompt_system_prompt)
                    .await
                {
                    Ok(session_id) => {
                        if let Err(error) = self
                            .context
                            .schedule_service
                            .bind_materialized_session_for_occurrence(occurrence, &session_id)
                            .await
                        {
                            return Err(immediate_delivery_failure(
                                occurrence,
                                error.to_string(),
                                OccurrenceFailureClass::InternalError,
                                None,
                                Some(session_id),
                            ));
                        }
                        Ok(ResolvedScheduledSession {
                            session_id: session_id.clone(),
                            materialized_session_id: Some(session_id),
                            allow_system_prompt_override: true,
                        })
                    }
                    Err(error) => Err(immediate_delivery_failure(
                        occurrence,
                        error.to_string(),
                        OccurrenceFailureClass::TargetMaterializationFailed,
                        None,
                        None,
                    )),
                }
            }
        }
    }

    #[cfg(feature = "mob")]
    async fn deliver_mob_binding(
        &self,
        occurrence: &Occurrence,
        binding: &MobTargetBinding,
    ) -> Result<DeliveryDispatch, ScheduleDomainError> {
        let mob_state = Arc::clone(&self.context.mob_state);
        let mob_id = MobId::from(mob_binding_mob_id(binding));

        match binding {
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
                    MeerkatId::from(member_id.as_str()),
                    content.clone(),
                    HandlingMode::Queue,
                    render_metadata.clone(),
                )
                .await
            {
                Ok(receipt) => Ok(immediate_completed_dispatch(
                    occurrence,
                    Some(receipt.session_id.to_string()),
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
            } => match mob_state
                .mob_run_flow(&mob_id, FlowId::from(flow_id.as_str()), params.clone())
                .await
            {
                Ok(run_id) => {
                    let mut receipt = DeliveryReceipt::new(
                        occurrence.occurrence_id.clone(),
                        occurrence.attempt_count,
                        DeliveryReceiptStage::DispatchAccepted,
                    );
                    receipt.correlation_id = Some(run_id.to_string());
                    Ok(DeliveryDispatch {
                        receipt,
                        correlation_id: Some(run_id.to_string()),
                        materialized_session_id: None,
                        completion: mob_flow_completion_future(mob_state, mob_id, run_id),
                    })
                }
                Err(error) => Ok(immediate_delivery_failure(
                    occurrence,
                    error.to_string(),
                    OccurrenceFailureClass::MobRejected,
                    None,
                    None,
                )),
            },
            MobTargetBinding::SpawnHelper {
                member_id,
                prompt,
                options,
                ..
            } => {
                let meerkat_id = MeerkatId::from(member_id.as_str());
                let helper_options = helper_options_from_spec(options)?;
                let prompt = prompt.clone();
                Ok(async_mob_completion_dispatch(
                    occurrence,
                    Some(member_id.clone()),
                    Box::pin(async move {
                        match mob_state
                            .mob_spawn_helper(&mob_id, meerkat_id, prompt, helper_options)
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
                let source_member_id = MeerkatId::from(source_member_id.as_str());
                let meerkat_id = MeerkatId::from(member_id.as_str());
                let helper_options = helper_options_from_spec(options)?;
                let fork_context = fork_context_from_spec(fork_context);
                let prompt = prompt.clone();
                Ok(async_mob_completion_dispatch(
                    occurrence,
                    Some(member_id.clone()),
                    Box::pin(async move {
                        match mob_state
                            .mob_fork_helper(
                                &mob_id,
                                &source_member_id,
                                meerkat_id,
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
        }
    }

    #[cfg(not(feature = "mob"))]
    async fn deliver_mob_binding(
        &self,
        occurrence: &Occurrence,
        _binding: &MobTargetBinding,
    ) -> Result<DeliveryDispatch, ScheduleDomainError> {
        Ok(immediate_delivery_failure(
            occurrence,
            "scheduled mob targets are not yet available on the REST host".to_string(),
            OccurrenceFailureClass::MobRejected,
            None,
            None,
        ))
    }
}

#[async_trait]
impl ScheduleTargetProbe for RestScheduleTargetAdapter {
    async fn probe_target(
        &self,
        occurrence: &Occurrence,
    ) -> Result<TargetProbeOutcome, ScheduleDomainError> {
        match &occurrence.target_snapshot {
            TargetBinding::Session(binding) => self.context.session_target_probe(binding).await,
            TargetBinding::Mob(binding) => self.probe_mob_binding(binding).await,
        }
    }
}

#[async_trait]
impl ScheduleTargetDelivery for RestScheduleTargetAdapter {
    async fn deliver_occurrence(
        &self,
        occurrence: &Occurrence,
    ) -> Result<DeliveryDispatch, ScheduleDomainError> {
        match &occurrence.target_snapshot {
            TargetBinding::Session(binding) => {
                let resolved = match self.resolve_session(occurrence, binding).await {
                    Ok(resolved) => resolved,
                    Err(dispatch) => return Ok(dispatch),
                };

                match binding.action() {
                    ScheduledSessionAction::Prompt {
                        prompt,
                        system_prompt,
                        render_metadata,
                        skill_references,
                        additional_instructions,
                    } => {
                        if system_prompt.is_some() && !resolved.allow_system_prompt_override {
                            return Ok(immediate_delivery_failure(
                                occurrence,
                                "scheduled system_prompt override is only supported when materializing a new session"
                                    .to_string(),
                                OccurrenceFailureClass::RuntimeRejected,
                                None,
                                resolved.materialized_session_id,
                            ));
                        }
                        deliver_scheduled_prompt(
                            &self.context,
                            &resolved.session_id,
                            occurrence,
                            ScheduledPromptDispatch {
                                prompt: prompt.clone(),
                                render_metadata: render_metadata.clone(),
                                skill_references: skill_references.clone(),
                                additional_instructions: additional_instructions.clone(),
                                materialized_session_id: resolved.materialized_session_id,
                            },
                        )
                        .await
                    }
                    ScheduledSessionAction::Event {
                        event_type,
                        payload,
                        render_metadata,
                    } => {
                        deliver_scheduled_event(
                            &self.context,
                            &resolved.session_id,
                            occurrence,
                            event_type.clone(),
                            payload.clone(),
                            render_metadata.clone(),
                            resolved.materialized_session_id,
                        )
                        .await
                    }
                }
            }
            TargetBinding::Mob(binding) => self.deliver_mob_binding(occurrence, binding).await,
        }
    }
}

impl AppState {
    pub async fn ensure_schedule_host_started(&self) -> Result<(), ScheduleDomainError> {
        let kind = self.schedule_service.store().kind();
        if matches!(kind, ScheduleStoreKind::Disabled | ScheduleStoreKind::Jsonl) {
            return Ok(());
        }

        let mut slot = self.schedule_host.inner.lock().await;
        if slot.is_some() {
            return Ok(());
        }

        let context = RestScheduleContext::from_state(self);
        let adapter = Arc::new(RestScheduleTargetAdapter::new(context));
        let driver = Arc::new(ScheduleDriver::new(
            self.schedule_service.clone(),
            self.schedule_service.store(),
            adapter.clone(),
            adapter,
            self.schedule_owner_id(),
            ScheduleDriverConfig {
                claim_limit: 32,
                lease_duration: ChronoDuration::seconds(60),
            },
        ));
        let poll_interval = if cfg!(test) {
            Duration::from_millis(50)
        } else {
            Duration::from_millis(250)
        };
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
        let join = tokio::spawn(async move {
            let mut interval = tokio::time::interval(poll_interval);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => break,
                    _ = interval.tick() => {
                        let _ = driver.tick_once().await;
                    }
                }
            }
        });

        *slot = Some(ScheduleHostHandle {
            shutdown_tx: Some(shutdown_tx),
            join,
        });
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
        format!("rest-scheduler:{}:{instance}", self.realm_id)
    }
}

struct ScheduledPromptDispatch {
    prompt: ContentInput,
    render_metadata: Option<meerkat_core::types::RenderMetadata>,
    skill_references: Vec<String>,
    additional_instructions: Vec<String>,
    materialized_session_id: Option<SessionId>,
}

async fn deliver_scheduled_prompt(
    context: &RestScheduleContext,
    session_id: &SessionId,
    occurrence: &Occurrence,
    dispatch: ScheduledPromptDispatch,
) -> Result<DeliveryDispatch, ScheduleDomainError> {
    match accept_scheduled_prompt_with_completion(
        context,
        session_id,
        occurrence,
        dispatch.prompt,
        dispatch.render_metadata,
        dispatch.skill_references,
        dispatch.additional_instructions,
    )
    .await
    {
        Ok(accepted) => Ok(build_dispatch_from_accepted(
            occurrence,
            accepted,
            dispatch.materialized_session_id,
        )),
        Err(error) => Ok(immediate_delivery_failure(
            occurrence,
            error.to_string(),
            OccurrenceFailureClass::RuntimeRejected,
            None,
            dispatch.materialized_session_id,
        )),
    }
}

async fn deliver_scheduled_event(
    context: &RestScheduleContext,
    session_id: &SessionId,
    occurrence: &Occurrence,
    event_type: String,
    payload: serde_json::Value,
    render_metadata: Option<meerkat_core::types::RenderMetadata>,
    materialized_session_id: Option<SessionId>,
) -> Result<DeliveryDispatch, ScheduleDomainError> {
    match accept_scheduled_event_with_completion(
        context,
        session_id,
        occurrence,
        event_type,
        payload,
        render_metadata,
    )
    .await
    {
        Ok(accepted) => Ok(build_dispatch_from_accepted(
            occurrence,
            accepted,
            materialized_session_id,
        )),
        Err(error) => Ok(immediate_delivery_failure(
            occurrence,
            error.to_string(),
            OccurrenceFailureClass::RuntimeRejected,
            None,
            materialized_session_id,
        )),
    }
}

async fn accept_scheduled_prompt_with_completion(
    context: &RestScheduleContext,
    session_id: &SessionId,
    occurrence: &Occurrence,
    prompt: ContentInput,
    render_metadata: Option<meerkat_core::types::RenderMetadata>,
    skill_references: Vec<String>,
    additional_instructions: Vec<String>,
) -> Result<AcceptedScheduledInput, ScheduleDomainError> {
    context
        .ensure_runtime_session_registered(session_id)
        .await?;
    maybe_spawn_comms_drain(context, session_id).await;
    let skill_keys = canonical_skill_keys(context, skill_references).await?;

    let turn_metadata = Some(
        meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
            handling_mode: None,
            keep_alive: None,
            skill_references: skill_keys,
            flow_tool_overlay: None,
            additional_instructions: (!additional_instructions.is_empty())
                .then_some(additional_instructions),
            model: None,
            provider: None,
            provider_params: None,
            render_metadata,
        },
    );
    let mut prompt_input = PromptInput::from_content_input(prompt, turn_metadata);
    prompt_input.header.source = InputOrigin::System;
    prompt_input.header.idempotency_key = Some(IdempotencyKey::new(
        schedule_attempt_idempotency_key(occurrence),
    ));
    prompt_input.header.correlation_id = Some(CorrelationId::from_uuid(occurrence.occurrence_id.0));

    let (outcome, handle) = context
        .runtime
        .runtime_adapter
        .accept_input_with_completion(session_id, Input::Prompt(prompt_input))
        .await
        .map_err(|error| ScheduleDomainError::Internal(error.to_string()))?;
    Ok(accepted_from_outcome(outcome, handle))
}

async fn accept_scheduled_event_with_completion(
    context: &RestScheduleContext,
    session_id: &SessionId,
    occurrence: &Occurrence,
    event_type: String,
    payload: serde_json::Value,
    render_metadata: Option<meerkat_core::types::RenderMetadata>,
) -> Result<AcceptedScheduledInput, ScheduleDomainError> {
    context
        .ensure_runtime_session_registered(session_id)
        .await?;
    maybe_spawn_comms_drain(context, session_id).await;

    let input = Input::ExternalEvent(meerkat_runtime::ExternalEventInput {
        header: InputHeader {
            id: meerkat_core::lifecycle::InputId::new(),
            timestamp: chrono::Utc::now(),
            source: InputOrigin::External {
                source_name: format!("schedule:{}", occurrence.schedule_id),
            },
            durability: InputDurability::Durable,
            visibility: InputVisibility::default(),
            idempotency_key: Some(IdempotencyKey::new(schedule_attempt_idempotency_key(
                occurrence,
            ))),
            supersession_key: None,
            correlation_id: Some(CorrelationId::from_uuid(occurrence.occurrence_id.0)),
        },
        event_type,
        payload,
        blocks: None,
        handling_mode: meerkat_core::types::HandlingMode::Queue,
        render_metadata,
    });

    let (outcome, handle) = context
        .runtime
        .runtime_adapter
        .accept_input_with_completion(session_id, input)
        .await
        .map_err(|error| ScheduleDomainError::Internal(error.to_string()))?;
    Ok(accepted_from_outcome(outcome, handle))
}

#[cfg(feature = "comms")]
async fn maybe_spawn_comms_drain(context: &RestScheduleContext, session_id: &SessionId) {
    let keep_alive = context
        .runtime
        .session_service
        .load_persisted(session_id)
        .await
        .ok()
        .flatten()
        .and_then(|session| {
            session
                .session_metadata()
                .map(|metadata| metadata.keep_alive)
        })
        .unwrap_or(false);
    let comms_rt = context
        .runtime
        .session_service
        .comms_runtime(session_id)
        .await;
    context
        .runtime
        .runtime_adapter
        .maybe_spawn_comms_drain(session_id, keep_alive, comms_rt)
        .await;
}

#[cfg(not(feature = "comms"))]
async fn maybe_spawn_comms_drain(_context: &RestScheduleContext, _session_id: &SessionId) {}

async fn canonical_skill_keys(
    context: &RestScheduleContext,
    skill_references: Vec<String>,
) -> Result<Option<Vec<meerkat_core::skills::SkillKey>>, ScheduleDomainError> {
    if skill_references.is_empty() {
        return Ok(None);
    }

    let snapshot = context
        .runtime
        .config_runtime
        .get()
        .await
        .map_err(|error| ScheduleDomainError::InvalidSchedule(error.to_string()))?;
    let registry = snapshot
        .config
        .skills
        .build_source_identity_registry()
        .map_err(|error| ScheduleDomainError::InvalidSchedule(error.to_string()))?;

    SkillsParams {
        preload_skills: None,
        skill_refs: Some(
            skill_references
                .into_iter()
                .map(SkillRef::Legacy)
                .collect::<Vec<_>>(),
        ),
        skill_references: None,
    }
    .canonical_skill_keys_with_registry(&registry)
    .map_err(|error| ScheduleDomainError::InvalidSchedule(error.to_string()))
}

fn accepted_from_outcome(
    outcome: AcceptOutcome,
    handle: Option<CompletionHandle>,
) -> AcceptedScheduledInput {
    match outcome {
        AcceptOutcome::Accepted { input_id, .. } => AcceptedScheduledInput {
            correlation_id: input_id.to_string(),
            handle,
        },
        AcceptOutcome::Deduplicated { existing_id, .. } => AcceptedScheduledInput {
            correlation_id: existing_id.to_string(),
            handle,
        },
        AcceptOutcome::Rejected { reason } => AcceptedScheduledInput {
            correlation_id: "rejected".into(),
            handle: Some(CompletionHandle::already_resolved(
                CompletionOutcome::Abandoned(reason),
            )),
        },
        _ => AcceptedScheduledInput {
            correlation_id: "rejected".into(),
            handle: Some(CompletionHandle::already_resolved(
                CompletionOutcome::Abandoned("unsupported runtime accept outcome".into()),
            )),
        },
    }
}

fn build_dispatch_from_accepted(
    occurrence: &Occurrence,
    accepted: AcceptedScheduledInput,
    materialized_session_id: Option<SessionId>,
) -> DeliveryDispatch {
    let mut receipt = DeliveryReceipt::new(
        occurrence.occurrence_id.clone(),
        occurrence.attempt_count,
        DeliveryReceiptStage::DispatchAccepted,
    );
    receipt.correlation_id = Some(accepted.correlation_id.clone());
    receipt.materialized_session_id = materialized_session_id.clone();

    let occurrence_id = occurrence.occurrence_id.clone();
    let attempt_count = occurrence.attempt_count;
    let correlation_id = accepted.correlation_id.clone();
    let completion_materialized_session_id = materialized_session_id.clone();
    let completed_receipt_materialized_session_id = materialized_session_id.clone();
    let completion = match accepted.handle {
        Some(handle) => scheduled_completion_future(handle, completion_materialized_session_id),
        None => Box::pin(async move {
            Ok(DeliveryTerminal::completed(Some({
                let mut receipt = DeliveryReceipt::new(
                    occurrence_id,
                    attempt_count,
                    DeliveryReceiptStage::Completed,
                );
                receipt.correlation_id = Some(correlation_id);
                receipt.materialized_session_id = completed_receipt_materialized_session_id;
                receipt
            })))
        }),
    };

    DeliveryDispatch {
        receipt,
        correlation_id: Some(accepted.correlation_id),
        materialized_session_id,
        completion,
    }
}

#[cfg(feature = "mob")]
fn immediate_completed_dispatch(
    occurrence: &Occurrence,
    correlation_id: Option<String>,
) -> DeliveryDispatch {
    let mut receipt = DeliveryReceipt::new(
        occurrence.occurrence_id.clone(),
        occurrence.attempt_count,
        DeliveryReceiptStage::DispatchAccepted,
    );
    receipt.correlation_id = correlation_id.clone();
    DeliveryDispatch {
        receipt,
        correlation_id,
        materialized_session_id: None,
        completion: Box::pin(async { Ok(DeliveryTerminal::completed(None)) }),
    }
}

#[cfg(feature = "mob")]
fn async_mob_completion_dispatch(
    occurrence: &Occurrence,
    correlation_id: Option<String>,
    completion: meerkat::DeliveryCompletion,
) -> DeliveryDispatch {
    let mut receipt = DeliveryReceipt::new(
        occurrence.occurrence_id.clone(),
        occurrence.attempt_count,
        DeliveryReceiptStage::DispatchAccepted,
    );
    receipt.correlation_id = correlation_id.clone();
    DeliveryDispatch {
        receipt,
        correlation_id,
        materialized_session_id: None,
        completion,
    }
}

fn scheduled_completion_future(
    handle: CompletionHandle,
    materialized_session_id: Option<SessionId>,
) -> meerkat::DeliveryCompletion {
    Box::pin(async move {
        Ok(match handle.wait().await {
            CompletionOutcome::Completed(_) | CompletionOutcome::CompletedWithoutResult => {
                DeliveryTerminal {
                    phase: OccurrencePhase::Completed,
                    receipt: None,
                    detail: None,
                    failure_class: None,
                    materialized_session_id,
                }
            }
            CompletionOutcome::CallbackPending { tool_name, .. } => DeliveryTerminal {
                phase: OccurrencePhase::DeliveryFailed,
                receipt: None,
                detail: Some(format!(
                    "scheduled delivery is waiting for unsupported external tool results: {tool_name}"
                )),
                failure_class: Some(OccurrenceFailureClass::RuntimeRejected),
                materialized_session_id,
            },
            CompletionOutcome::Abandoned(reason) => DeliveryTerminal {
                phase: OccurrencePhase::DeliveryFailed,
                receipt: None,
                detail: Some(reason),
                failure_class: Some(OccurrenceFailureClass::RuntimeRejected),
                materialized_session_id,
            },
            CompletionOutcome::RuntimeTerminated(reason) => DeliveryTerminal {
                phase: OccurrencePhase::DeliveryFailed,
                receipt: None,
                detail: Some(reason),
                failure_class: Some(OccurrenceFailureClass::TransportError),
                materialized_session_id,
            },
        })
    })
}

fn immediate_delivery_failure(
    occurrence: &Occurrence,
    detail: String,
    failure_class: OccurrenceFailureClass,
    correlation_id: Option<String>,
    materialized_session_id: Option<SessionId>,
) -> DeliveryDispatch {
    let mut receipt = DeliveryReceipt::new(
        occurrence.occurrence_id.clone(),
        occurrence.attempt_count,
        DeliveryReceiptStage::DispatchStarted,
    );
    receipt.correlation_id = correlation_id.clone();
    receipt.materialized_session_id = materialized_session_id.clone();

    DeliveryDispatch {
        receipt,
        correlation_id,
        materialized_session_id: materialized_session_id.clone(),
        completion: Box::pin(async move {
            Ok(DeliveryTerminal {
                phase: OccurrencePhase::DeliveryFailed,
                receipt: None,
                detail: Some(detail),
                failure_class: Some(failure_class),
                materialized_session_id,
            })
        }),
    }
}

fn schedule_attempt_idempotency_key(occurrence: &Occurrence) -> String {
    format!(
        "schedule:{}:occurrence:{}:attempt:{}",
        occurrence.schedule_id, occurrence.occurrence_id, occurrence.attempt_count
    )
}

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
    options.profile_name = spec.profile_name.clone().map(Into::into);
    options.runtime_mode = spec.runtime_mode.map(|mode| match mode {
        ScheduledMobRuntimeMode::AutonomousHost => meerkat_mob::MobRuntimeMode::AutonomousHost,
        ScheduledMobRuntimeMode::TurnDriven => meerkat_mob::MobRuntimeMode::TurnDriven,
    });
    options.backend = spec.backend.map(|backend| match backend {
        ScheduledMobBackendKind::Session => MobBackendKind::Session,
        ScheduledMobBackendKind::External => MobBackendKind::External,
    });
    options.tool_access_policy = spec
        .tool_access_policy
        .as_ref()
        .map(|value| serde_json::from_value(value.clone()))
        .transpose()
        .map_err(|error| ScheduleDomainError::InvalidSchedule(error.to_string()))?;
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
