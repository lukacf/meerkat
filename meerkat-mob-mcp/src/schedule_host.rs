use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use meerkat::surface::{
    SurfaceScheduleMobHost, async_completion_dispatch, immediate_completed_dispatch,
    immediate_delivery_failure,
};
use meerkat::{
    DeliveryCompletion, DeliveryDispatch, DeliveryTerminal, ForkContextSpec, HelperOptionsSpec,
    MobTargetBinding, Occurrence, OccurrenceFailureClass, ScheduleDomainError,
    ScheduleSpawnTooling, ScheduledMobAction, ScheduledMobBackendKind, ScheduledMobRuntimeMode,
    TargetProbeOutcome,
};
use meerkat_core::types::{ContentInput, HandlingMode, RenderMetadata};
use meerkat_mob::{
    AgentIdentity, FlowId, ForkContext, HelperOptions, MobBackendKind, MobError, MobId,
    MobRunStatus, RunId,
};

use crate::MobMcpState;

#[cfg(target_arch = "wasm32")]
use crate::tokio::time::sleep;
#[cfg(not(target_arch = "wasm32"))]
use tokio::time::sleep;

#[async_trait]
pub(crate) trait ScheduleMobRuntime: Send + Sync {
    async fn member_exists(
        &self,
        mob_id: &MobId,
        identity: &AgentIdentity,
    ) -> Result<bool, MobError>;

    async fn flow_exists(&self, mob_id: &MobId, flow_id: &FlowId) -> Result<bool, MobError>;

    async fn member_send(
        &self,
        mob_id: &MobId,
        identity: AgentIdentity,
        content: ContentInput,
        render_metadata: Option<RenderMetadata>,
    ) -> Result<String, MobError>;

    async fn run_flow(
        &self,
        mob_id: &MobId,
        flow_id: FlowId,
        params: serde_json::Value,
    ) -> Result<RunId, MobError>;

    async fn flow_status(
        &self,
        mob_id: &MobId,
        run_id: RunId,
    ) -> Result<Option<MobRunStatus>, MobError>;

    async fn spawn_helper(
        &self,
        mob_id: &MobId,
        identity: AgentIdentity,
        prompt: String,
        options: HelperOptions,
    ) -> Result<(), MobError>;

    async fn fork_helper(
        &self,
        mob_id: &MobId,
        source_identity: &AgentIdentity,
        identity: AgentIdentity,
        prompt: String,
        fork_context: ForkContext,
        options: HelperOptions,
    ) -> Result<(), MobError>;
}

#[async_trait]
impl ScheduleMobRuntime for MobMcpState {
    async fn member_exists(
        &self,
        mob_id: &MobId,
        identity: &AgentIdentity,
    ) -> Result<bool, MobError> {
        Ok(self
            .handle_for(mob_id)
            .await?
            .get_member(identity)
            .await
            .is_some())
    }

    async fn flow_exists(&self, mob_id: &MobId, flow_id: &FlowId) -> Result<bool, MobError> {
        Ok(self
            .handle_for(mob_id)
            .await?
            .list_flows()
            .into_iter()
            .any(|candidate| candidate == *flow_id))
    }

    async fn member_send(
        &self,
        mob_id: &MobId,
        identity: AgentIdentity,
        content: ContentInput,
        render_metadata: Option<RenderMetadata>,
    ) -> Result<String, MobError> {
        self.mob_member_send(
            mob_id,
            identity,
            content,
            HandlingMode::Queue,
            render_metadata,
        )
        .await
        .map(|receipt| receipt.identity.to_string())
    }

    async fn run_flow(
        &self,
        mob_id: &MobId,
        flow_id: FlowId,
        params: serde_json::Value,
    ) -> Result<RunId, MobError> {
        self.mob_run_flow(mob_id, flow_id, params).await
    }

    async fn flow_status(
        &self,
        mob_id: &MobId,
        run_id: RunId,
    ) -> Result<Option<MobRunStatus>, MobError> {
        Ok(self
            .mob_flow_status(mob_id, run_id)
            .await?
            .map(|run| run.status))
    }

    async fn spawn_helper(
        &self,
        mob_id: &MobId,
        identity: AgentIdentity,
        prompt: String,
        options: HelperOptions,
    ) -> Result<(), MobError> {
        self.mob_spawn_helper(mob_id, identity, prompt, options)
            .await
            .map(|_| ())
    }

    async fn fork_helper(
        &self,
        mob_id: &MobId,
        source_identity: &AgentIdentity,
        identity: AgentIdentity,
        prompt: String,
        fork_context: ForkContext,
        options: HelperOptions,
    ) -> Result<(), MobError> {
        self.mob_fork_helper(
            mob_id,
            source_identity,
            identity,
            prompt,
            fork_context,
            options,
        )
        .await
        .map(|_| ())
    }
}

/// Reusable schedule-to-mob delivery adapter for surfaces backed by `MobMcpState`.
///
/// Scheduled mob targets remain identity-first: member bindings are resolved as
/// `(mob_id, AgentIdentity)` by the mob runtime at probe/delivery time.
pub struct MobMcpScheduleHost {
    runtime: Arc<dyn ScheduleMobRuntime>,
}

impl MobMcpScheduleHost {
    pub fn new(state: Arc<MobMcpState>) -> Self {
        Self { runtime: state }
    }

    #[cfg(test)]
    pub(crate) fn from_runtime(runtime: Arc<dyn ScheduleMobRuntime>) -> Self {
        Self { runtime }
    }
}

#[async_trait]
impl SurfaceScheduleMobHost for MobMcpScheduleHost {
    async fn probe_mob_target(
        &self,
        binding: &MobTargetBinding,
    ) -> Result<TargetProbeOutcome, ScheduleDomainError> {
        let mob_id = MobId::from(mob_binding_mob_id(binding));

        match binding {
            MobTargetBinding::Member { member_id, .. } => {
                let identity = AgentIdentity::from(member_id.as_str());
                match self.runtime.member_exists(&mob_id, &identity).await {
                    Ok(true) => Ok(TargetProbeOutcome::Ready),
                    Ok(false) => Ok(TargetProbeOutcome::Missing {
                        detail: Some(format!("mob member not found: {member_id}")),
                    }),
                    Err(error) => mob_probe_error_outcome(error),
                }
            }
            MobTargetBinding::Flow { flow_id, .. } => {
                let flow_id = FlowId::from(flow_id.as_str());
                match self.runtime.flow_exists(&mob_id, &flow_id).await {
                    Ok(true) => Ok(TargetProbeOutcome::Ready),
                    Ok(false) => Ok(TargetProbeOutcome::Missing {
                        detail: Some(format!("mob flow not found: {flow_id}")),
                    }),
                    Err(error) => mob_probe_error_outcome(error),
                }
            }
            MobTargetBinding::SpawnHelper { member_id, .. } => {
                let identity = AgentIdentity::from(member_id.as_str());
                match self.runtime.member_exists(&mob_id, &identity).await {
                    Ok(true) => Ok(TargetProbeOutcome::Busy {
                        detail: Some(format!("mob member already exists: {member_id}")),
                    }),
                    Ok(false) => Ok(TargetProbeOutcome::Ready),
                    Err(error) => mob_probe_error_outcome(error),
                }
            }
            MobTargetBinding::ForkHelper {
                source_member_id,
                member_id,
                ..
            } => {
                let source = AgentIdentity::from(source_member_id.as_str());
                match self.runtime.member_exists(&mob_id, &source).await {
                    Ok(true) => {}
                    Ok(false) => {
                        return Ok(TargetProbeOutcome::Missing {
                            detail: Some(format!(
                                "mob source member not found: {source_member_id}"
                            )),
                        });
                    }
                    Err(error) => return mob_probe_error_outcome(error),
                }

                let target = AgentIdentity::from(member_id.as_str());
                match self.runtime.member_exists(&mob_id, &target).await {
                    Ok(true) => Ok(TargetProbeOutcome::Busy {
                        detail: Some(format!("mob member already exists: {member_id}")),
                    }),
                    Ok(false) => Ok(TargetProbeOutcome::Ready),
                    Err(error) => mob_probe_error_outcome(error),
                }
            }
        }
    }

    async fn deliver_mob_target(
        &self,
        occurrence: &Occurrence,
        binding: &MobTargetBinding,
    ) -> Result<DeliveryDispatch, ScheduleDomainError> {
        let mob_id = MobId::from(mob_binding_mob_id(binding));

        match binding {
            MobTargetBinding::Member {
                member_id,
                action:
                    ScheduledMobAction::Send {
                        content,
                        render_metadata,
                    },
                ..
            } => match self
                .runtime
                .member_send(
                    &mob_id,
                    AgentIdentity::from(member_id.as_str()),
                    content.clone(),
                    render_metadata.clone(),
                )
                .await
            {
                Ok(correlation_id) => Ok(immediate_completed_dispatch(
                    occurrence,
                    Some(correlation_id),
                )),
                Err(error) => Ok(mob_delivery_failed_dispatch(occurrence, error)),
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
                let flow_id = FlowId::from(flow_id.as_str());
                match self.runtime.run_flow(&mob_id, flow_id, params).await {
                    Ok(run_id) => Ok(async_completion_dispatch(
                        occurrence,
                        Some(run_id.to_string()),
                        mob_flow_completion_future(Arc::clone(&self.runtime), mob_id, run_id),
                    )),
                    Err(error) => Ok(mob_delivery_failed_dispatch(occurrence, error)),
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
                let runtime = Arc::clone(&self.runtime);
                Ok(async_completion_dispatch(
                    occurrence,
                    Some(member_id.clone()),
                    Box::pin(async move {
                        match runtime
                            .spawn_helper(&mob_id, identity, prompt, helper_options)
                            .await
                        {
                            Ok(()) => Ok(DeliveryTerminal::completed(None)),
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
                let runtime = Arc::clone(&self.runtime);
                Ok(async_completion_dispatch(
                    occurrence,
                    Some(member_id.clone()),
                    Box::pin(async move {
                        match runtime
                            .fork_helper(
                                &mob_id,
                                &source_identity,
                                identity,
                                prompt,
                                fork_context,
                                helper_options,
                            )
                            .await
                        {
                            Ok(()) => Ok(DeliveryTerminal::completed(None)),
                            Err(error) => Ok(mob_delivery_failed_terminal(error)),
                        }
                    }),
                ))
            }
        }
    }
}

fn mob_binding_mob_id(binding: &MobTargetBinding) -> &str {
    match binding {
        MobTargetBinding::Member { mob_id, .. }
        | MobTargetBinding::Flow { mob_id, .. }
        | MobTargetBinding::SpawnHelper { mob_id, .. }
        | MobTargetBinding::ForkHelper { mob_id, .. } => mob_id,
    }
}

fn helper_options_from_spec(
    spec: &HelperOptionsSpec,
) -> Result<HelperOptions, ScheduleDomainError> {
    let mut options = HelperOptions::default();
    options.role_name = spec.role_name.clone().map(Into::into);
    if let Some(tooling) = &spec.tooling {
        match tooling {
            ScheduleSpawnTooling::InheritParent { .. } | ScheduleSpawnTooling::Minimal
                if spec.resolved_spawn_snapshot.is_none() =>
            {
                return Err(ScheduleDomainError::InvalidSchedule(
                    "scheduled mob helper tooling requires a pre-resolved spawn snapshot"
                        .to_string(),
                ));
            }
            ScheduleSpawnTooling::Profile {
                name,
                allow_overlay,
                deny_overlay,
            } => {
                if options.role_name.is_none() {
                    options.role_name = Some(name.clone().into());
                }
                if spec.resolved_spawn_snapshot.is_none()
                    && (allow_overlay.is_some() || deny_overlay.is_some())
                {
                    return Err(ScheduleDomainError::InvalidSchedule(
                        "scheduled mob helper profile tooling overlays require a pre-resolved spawn snapshot"
                            .to_string(),
                    ));
                }
            }
            _ => {}
        }
    }
    options.runtime_mode = spec.runtime_mode.map(|mode| match mode {
        ScheduledMobRuntimeMode::AutonomousHost => meerkat_mob::MobRuntimeMode::AutonomousHost,
        ScheduledMobRuntimeMode::TurnDriven => meerkat_mob::MobRuntimeMode::TurnDriven,
    });
    options.backend = spec.backend.map(|backend| match backend {
        ScheduledMobBackendKind::Session => MobBackendKind::Session,
        ScheduledMobBackendKind::External => MobBackendKind::External,
    });
    options.tool_access_policy = spec.tool_access_policy.clone();
    if let Some(snapshot) = &spec.resolved_spawn_snapshot {
        options.inherited_tool_filter = Some(meerkat_core::WitnessedToolFilter::new(
            snapshot.tool_filter.clone(),
            snapshot.tool_filter_witnesses.clone(),
        ));
        options.model_override = Some(snapshot.model.clone());
        options.provider_params_override = snapshot.provider_params.clone();
    }
    Ok(options)
}

fn fork_context_from_spec(spec: &ForkContextSpec) -> ForkContext {
    match spec {
        ForkContextSpec::FullHistory => ForkContext::FullHistory,
        ForkContextSpec::LastMessages { count } => ForkContext::LastMessages { count: *count },
    }
}

fn mob_delivery_failed_dispatch(occurrence: &Occurrence, error: MobError) -> DeliveryDispatch {
    let failure_class = mob_error_failure_class(&error);
    immediate_delivery_failure(occurrence, error.to_string(), failure_class, None, None)
}

fn mob_delivery_failed_terminal(error: MobError) -> DeliveryTerminal {
    let failure_class = mob_error_failure_class(&error);
    DeliveryTerminal::delivery_failed(error.to_string(), failure_class)
}

fn mob_probe_error_outcome(error: MobError) -> Result<TargetProbeOutcome, ScheduleDomainError> {
    if mob_error_is_missing_target(&error) {
        Ok(TargetProbeOutcome::Missing {
            detail: Some(error.to_string()),
        })
    } else {
        Err(ScheduleDomainError::ProbeFailed(error.to_string()))
    }
}

fn mob_error_is_missing_target(error: &MobError) -> bool {
    matches!(
        error,
        MobError::ProfileNotFound(_)
            | MobError::MemberNotFound(_)
            | MobError::FlowNotFound(_)
            | MobError::RunNotFound(_)
            | MobError::WorkNotFound(_)
    ) || matches!(error, MobError::Internal(detail) if detail.starts_with("mob not found:"))
}

fn mob_error_failure_class(error: &MobError) -> OccurrenceFailureClass {
    match error {
        MobError::ProfileNotFound(_)
        | MobError::MemberNotFound(_)
        | MobError::FlowNotFound(_)
        | MobError::RunNotFound(_)
        | MobError::WorkNotFound(_) => OccurrenceFailureClass::TargetMissing,
        MobError::MemberAlreadyExists(_) => OccurrenceFailureClass::TargetBusy,
        MobError::StorageError(_)
        | MobError::SessionError(_)
        | MobError::CommsError(_)
        | MobError::MemberRestoreFailed { .. }
        | MobError::KickoffWaitTimedOut { .. }
        | MobError::ReadyWaitTimedOut { .. }
        | MobError::FlowTurnTimedOut => OccurrenceFailureClass::TransportError,
        MobError::Internal(_) => OccurrenceFailureClass::InternalError,
        MobError::CallbackPending { .. } => OccurrenceFailureClass::RuntimeRejected,
        _ => OccurrenceFailureClass::MobRejected,
    }
}

fn mob_flow_completion_future(
    runtime: Arc<dyn ScheduleMobRuntime>,
    mob_id: MobId,
    run_id: RunId,
) -> DeliveryCompletion {
    Box::pin(async move {
        loop {
            match runtime.flow_status(&mob_id, run_id.clone()).await {
                Ok(Some(MobRunStatus::Completed)) => {
                    return Ok(DeliveryTerminal::completed(None));
                }
                Ok(Some(status)) if status.is_terminal() => {
                    return Ok(DeliveryTerminal::delivery_failed(
                        format!("mob flow terminated as {status:?}"),
                        OccurrenceFailureClass::MobRejected,
                    ));
                }
                Ok(Some(_)) => sleep(Duration::from_millis(100)).await,
                Ok(None) => {
                    return Ok(DeliveryTerminal::delivery_failed(
                        format!("mob flow run disappeared: {run_id}"),
                        OccurrenceFailureClass::TargetMissing,
                    ));
                }
                Err(error) => return Ok(mob_delivery_failed_terminal(error)),
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::BTreeSet;
    use std::sync::Mutex;

    use meerkat::{
        CreateScheduleRequest, IntervalTriggerSpec, MisfirePolicy, MissingTargetPolicy,
        OccurrenceOrdinal, OverlapPolicy, ResolvedSpawnSnapshot, Schedule, TargetBinding,
        TriggerSpec,
    };
    use meerkat_core::tool_scope::ToolFilter;
    use serde_json::value::RawValue;

    #[derive(Debug, Default)]
    struct RecordingMobRuntime {
        members: Mutex<BTreeSet<String>>,
        flows: Mutex<BTreeSet<String>>,
        sent_members: Mutex<Vec<(String, String, String)>>,
        run_flows: Mutex<Vec<(String, String, serde_json::Value)>>,
        spawn_helpers: Mutex<Vec<(String, String, HelperOptions)>>,
        member_exists_error: Mutex<Option<String>>,
        flow_run_missing: Mutex<bool>,
        next_run_id: Mutex<Option<RunId>>,
    }

    impl RecordingMobRuntime {
        fn with_member(&self, member_id: &str) {
            self.members
                .lock()
                .expect("members lock")
                .insert(member_id.to_string());
        }

        fn with_flow(&self, flow_id: &str) {
            self.flows
                .lock()
                .expect("flows lock")
                .insert(flow_id.to_string());
        }
    }

    #[async_trait]
    impl ScheduleMobRuntime for RecordingMobRuntime {
        async fn member_exists(
            &self,
            _mob_id: &MobId,
            identity: &AgentIdentity,
        ) -> Result<bool, MobError> {
            if let Some(error) = self
                .member_exists_error
                .lock()
                .expect("member exists error lock")
                .clone()
            {
                return Err(MobError::Internal(error));
            }
            Ok(self
                .members
                .lock()
                .expect("members lock")
                .contains(identity.as_str()))
        }

        async fn flow_exists(&self, _mob_id: &MobId, flow_id: &FlowId) -> Result<bool, MobError> {
            Ok(self
                .flows
                .lock()
                .expect("flows lock")
                .contains(flow_id.as_str()))
        }

        async fn member_send(
            &self,
            mob_id: &MobId,
            identity: AgentIdentity,
            content: ContentInput,
            _render_metadata: Option<RenderMetadata>,
        ) -> Result<String, MobError> {
            let content = match content {
                ContentInput::Text(text) => text,
                other => format!("{other:?}"),
            };
            self.sent_members.lock().expect("sent lock").push((
                mob_id.to_string(),
                identity.to_string(),
                content,
            ));
            Ok(identity.to_string())
        }

        async fn run_flow(
            &self,
            mob_id: &MobId,
            flow_id: FlowId,
            params: serde_json::Value,
        ) -> Result<RunId, MobError> {
            self.run_flows.lock().expect("run lock").push((
                mob_id.to_string(),
                flow_id.to_string(),
                params,
            ));
            Ok(self
                .next_run_id
                .lock()
                .expect("run id lock")
                .clone()
                .unwrap_or_default())
        }

        async fn flow_status(
            &self,
            _mob_id: &MobId,
            _run_id: RunId,
        ) -> Result<Option<MobRunStatus>, MobError> {
            if *self.flow_run_missing.lock().expect("flow run missing lock") {
                return Ok(None);
            }
            Ok(Some(MobRunStatus::Completed))
        }

        async fn spawn_helper(
            &self,
            mob_id: &MobId,
            identity: AgentIdentity,
            _prompt: String,
            options: HelperOptions,
        ) -> Result<(), MobError> {
            self.spawn_helpers.lock().expect("spawn lock").push((
                mob_id.to_string(),
                identity.to_string(),
                options,
            ));
            Ok(())
        }

        async fn fork_helper(
            &self,
            _mob_id: &MobId,
            _source_identity: &AgentIdentity,
            _identity: AgentIdentity,
            _prompt: String,
            _fork_context: ForkContext,
            _options: HelperOptions,
        ) -> Result<(), MobError> {
            Ok(())
        }
    }

    fn raw_json(value: &str) -> Box<RawValue> {
        RawValue::from_string(value.to_string()).expect("valid raw json")
    }

    fn sample_occurrence(binding: MobTargetBinding) -> Occurrence {
        let schedule = Schedule::new(CreateScheduleRequest {
            name: Some("mob-schedule-host-test".to_string()),
            description: None,
            trigger: TriggerSpec::Interval(IntervalTriggerSpec {
                start_at_utc: chrono::Utc::now(),
                every_seconds: 60,
                end_at_utc: None,
            }),
            target: TargetBinding::Mob(Box::new(binding)),
            misfire_policy: MisfirePolicy::Skip,
            overlap_policy: OverlapPolicy::SkipIfRunning,
            missing_target_policy: MissingTargetPolicy::Skip,
            labels: Default::default(),
            planning_horizon_days: None,
            planning_horizon_occurrences: None,
        });
        let mut occurrence =
            Occurrence::planned_from_schedule(&schedule, OccurrenceOrdinal(0), chrono::Utc::now());
        occurrence.attempt_count = 1;
        occurrence
    }

    #[tokio::test]
    async fn member_send_target_resolves_member_id_as_agent_identity() {
        let runtime = Arc::new(RecordingMobRuntime::default());
        runtime.with_member("deploy-monitor");
        let host = MobMcpScheduleHost::from_runtime(runtime.clone());
        let binding = MobTargetBinding::Member {
            mob_id: "ops".to_string(),
            member_id: "deploy-monitor".to_string(),
            action: ScheduledMobAction::Send {
                content: ContentInput::Text("Check deploy state.".to_string()),
                render_metadata: None,
            },
        };
        let occurrence = sample_occurrence(binding.clone());

        let dispatch = host
            .deliver_mob_target(&occurrence, &binding)
            .await
            .expect("delivery dispatch");

        assert_eq!(dispatch.correlation_id.as_deref(), Some("deploy-monitor"));
        assert_eq!(
            runtime.sent_members.lock().expect("sent lock").as_slice(),
            &[(
                "ops".to_string(),
                "deploy-monitor".to_string(),
                "Check deploy state.".to_string()
            )]
        );
    }

    #[tokio::test]
    async fn flow_target_starts_mob_flow() {
        let runtime = Arc::new(RecordingMobRuntime::default());
        runtime.with_flow("release-check");
        let run_id = RunId::new();
        *runtime.next_run_id.lock().expect("run id lock") = Some(run_id.clone());
        let host = MobMcpScheduleHost::from_runtime(runtime.clone());
        let binding = MobTargetBinding::Flow {
            mob_id: "ops".to_string(),
            flow_id: "release-check".to_string(),
            params: raw_json(r#"{"sha":"abc123"}"#),
        };
        let occurrence = sample_occurrence(binding.clone());

        let dispatch = host
            .deliver_mob_target(&occurrence, &binding)
            .await
            .expect("delivery dispatch");

        let run_id_string = run_id.to_string();
        assert_eq!(
            dispatch.correlation_id.as_deref(),
            Some(run_id_string.as_str())
        );
        assert_eq!(
            runtime.run_flows.lock().expect("run lock").as_slice(),
            &[(
                "ops".to_string(),
                "release-check".to_string(),
                serde_json::json!({"sha": "abc123"})
            )]
        );
    }

    #[tokio::test]
    async fn missing_member_probe_reports_missing() {
        let runtime = Arc::new(RecordingMobRuntime::default());
        let host = MobMcpScheduleHost::from_runtime(runtime);
        let binding = MobTargetBinding::Member {
            mob_id: "ops".to_string(),
            member_id: "deploy-monitor".to_string(),
            action: ScheduledMobAction::Send {
                content: ContentInput::Text("Check deploy state.".to_string()),
                render_metadata: None,
            },
        };

        let outcome = host
            .probe_mob_target(&binding)
            .await
            .expect("probe should succeed");

        let TargetProbeOutcome::Missing { detail } = outcome else {
            panic!("expected missing member probe, got {outcome:?}");
        };
        assert_eq!(
            detail.as_deref(),
            Some("mob member not found: deploy-monitor")
        );
    }

    #[tokio::test]
    async fn helper_target_reports_busy_when_identity_exists() {
        let runtime = Arc::new(RecordingMobRuntime::default());
        runtime.with_member("deploy-monitor");
        let host = MobMcpScheduleHost::from_runtime(runtime);
        let binding = MobTargetBinding::SpawnHelper {
            mob_id: "ops".to_string(),
            member_id: "deploy-monitor".to_string(),
            prompt: "Check deploy state.".to_string(),
            options: HelperOptionsSpec::default(),
        };

        let outcome = host
            .probe_mob_target(&binding)
            .await
            .expect("probe should succeed");

        let TargetProbeOutcome::Busy { detail } = outcome else {
            panic!("expected busy helper probe, got {outcome:?}");
        };
        assert_eq!(
            detail.as_deref(),
            Some("mob member already exists: deploy-monitor")
        );
    }

    #[tokio::test]
    async fn probe_runtime_error_is_reported_as_probe_failure() {
        let runtime = Arc::new(RecordingMobRuntime::default());
        *runtime
            .member_exists_error
            .lock()
            .expect("member exists error lock") = Some("storage unavailable".to_string());
        let host = MobMcpScheduleHost::from_runtime(runtime);
        let binding = MobTargetBinding::Member {
            mob_id: "ops".to_string(),
            member_id: "deploy-monitor".to_string(),
            action: ScheduledMobAction::Send {
                content: ContentInput::Text("Check deploy state.".to_string()),
                render_metadata: None,
            },
        };

        let error = host
            .probe_mob_target(&binding)
            .await
            .expect_err("probe error should not be downgraded to missing");

        assert!(
            matches!(error, ScheduleDomainError::ProbeFailed(detail) if detail == "internal error: storage unavailable")
        );
    }

    #[tokio::test]
    async fn helper_delivery_preserves_resolved_spawn_snapshot() {
        let runtime = Arc::new(RecordingMobRuntime::default());
        let host = MobMcpScheduleHost::from_runtime(runtime.clone());
        let filter = ToolFilter::Allow(["send", "read_file"].into_iter().collect());
        let filter_witnesses: std::collections::BTreeMap<
            String,
            meerkat_core::ToolVisibilityWitness,
        > = [
            (
                "send".to_string(),
                meerkat_core::ToolVisibilityWitness {
                    stable_owner_key: Some("test-owner:send".to_string()),
                    last_seen_provenance: None,
                },
            ),
            (
                "read_file".to_string(),
                meerkat_core::ToolVisibilityWitness {
                    stable_owner_key: Some("test-owner:read_file".to_string()),
                    last_seen_provenance: None,
                },
            ),
        ]
        .into_iter()
        .collect();
        let binding = MobTargetBinding::SpawnHelper {
            mob_id: "ops".to_string(),
            member_id: "deploy-monitor".to_string(),
            prompt: "Check deploy state.".to_string(),
            options: HelperOptionsSpec {
                tooling: Some(ScheduleSpawnTooling::Profile {
                    name: "delegate".to_string(),
                    allow_overlay: None,
                    deny_overlay: None,
                }),
                resolved_spawn_snapshot: Some(ResolvedSpawnSnapshot {
                    tool_filter: filter.clone(),
                    tool_filter_witnesses: filter_witnesses.clone(),
                    model: "claude-snapshot".to_string(),
                    provider_params: Some(serde_json::json!({"thinking_budget": 1024})),
                }),
                ..HelperOptionsSpec::default()
            },
        };
        let occurrence = sample_occurrence(binding.clone());

        let dispatch = host
            .deliver_mob_target(&occurrence, &binding)
            .await
            .expect("delivery dispatch");
        let terminal = (dispatch.completion).await.expect("completion");
        assert_eq!(terminal.phase, meerkat::OccurrencePhase::Completed);

        let helpers = runtime.spawn_helpers.lock().expect("spawn lock");
        assert_eq!(helpers.len(), 1);
        let (_, identity, options) = &helpers[0];
        assert_eq!(identity, "deploy-monitor");
        assert_eq!(
            options.role_name.as_ref().map(|name| name.as_str()),
            Some("delegate")
        );
        assert_eq!(
            options.inherited_tool_filter.as_ref(),
            Some(&meerkat_core::WitnessedToolFilter::new(
                filter,
                filter_witnesses
            ))
        );
        assert_eq!(options.model_override.as_deref(), Some("claude-snapshot"));
        assert_eq!(
            options.provider_params_override,
            Some(serde_json::json!({"thinking_budget": 1024}))
        );
        assert!(
            options.override_profile.is_none(),
            "schedule snapshot should not fabricate a replacement role profile"
        );
    }

    #[tokio::test]
    async fn flow_completion_reports_missing_when_run_disappears() {
        let runtime = Arc::new(RecordingMobRuntime::default());
        *runtime
            .flow_run_missing
            .lock()
            .expect("flow run missing lock") = true;
        let run_id = RunId::new();
        *runtime.next_run_id.lock().expect("run id lock") = Some(run_id.clone());
        let host = MobMcpScheduleHost::from_runtime(runtime);
        let binding = MobTargetBinding::Flow {
            mob_id: "ops".to_string(),
            flow_id: "release-check".to_string(),
            params: raw_json(r#"{}"#),
        };
        let occurrence = sample_occurrence(binding.clone());

        let dispatch = host
            .deliver_mob_target(&occurrence, &binding)
            .await
            .expect("delivery dispatch");
        let terminal = (dispatch.completion).await.expect("completion");

        assert_eq!(terminal.phase, meerkat::OccurrencePhase::DeliveryFailed);
        assert_eq!(
            terminal.failure_class,
            Some(OccurrenceFailureClass::TargetMissing)
        );
        assert_eq!(
            terminal.detail.as_deref(),
            Some(format!("mob flow run disappeared: {run_id}").as_str())
        );
    }

    #[test]
    fn mob_error_failure_class_maps_common_target_failures() {
        assert_eq!(
            mob_error_failure_class(&MobError::MemberNotFound(AgentIdentity::from("missing"))),
            OccurrenceFailureClass::TargetMissing
        );
        assert_eq!(
            mob_error_failure_class(&MobError::MemberAlreadyExists(AgentIdentity::from("busy"))),
            OccurrenceFailureClass::TargetBusy
        );
        assert_eq!(
            mob_error_failure_class(&MobError::Internal("boom".to_string())),
            OccurrenceFailureClass::InternalError
        );
    }
}
