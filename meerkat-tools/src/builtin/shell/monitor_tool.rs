//! High-trust agent-authored durable script monitor tool.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use meerkat_core::{
    DetachedToolExecutionPolicy, IdempotencyScope, ToolCallView, ToolDef, ToolProvenance,
    ToolSourceKind,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

use super::{
    JobId, JobManager, MonitorOutputProtocol, MonitorProtocolLimits, MonitorStartOptions,
    ShellConfig,
};
use crate::builtin::{BuiltinTool, BuiltinToolError, ToolOutput};

const DETACHED_SUBMISSION_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
pub struct MonitorStartTool {
    config: ShellConfig,
    job_manager: Arc<JobManager>,
}

impl MonitorStartTool {
    pub fn new(config: ShellConfig, job_manager: Arc<JobManager>) -> Self {
        Self {
            config,
            job_manager,
        }
    }

    async fn call_with_tool_call_id(
        &self,
        args: Value,
        tool_call_id: Option<&str>,
    ) -> Result<ToolOutput, BuiltinToolError> {
        let input: MonitorStartInput = serde_json::from_value(args)
            .map_err(|error| BuiltinToolError::invalid_args(error.to_string()))?;
        let timeout_secs = input
            .timeout_secs
            .unwrap_or(self.config.default_timeout_secs);
        if timeout_secs == 0 {
            return Err(BuiltinToolError::invalid_args(
                "timeout_secs must be greater than zero",
            ));
        }
        self.config
            .check_allowlist(&input.command)
            .map_err(|error| BuiltinToolError::execution_failed(error.to_string()))?;
        let working_dir = match input.working_dir.as_deref() {
            Some(path) => Some(
                self.config
                    .validate_working_dir_async(Path::new(path))
                    .await
                    .map_err(|error| BuiltinToolError::execution_failed(error.to_string()))?,
            ),
            None => None,
        };
        let limits = MonitorProtocolLimits {
            max_line_bytes: input
                .max_line_bytes
                .unwrap_or_else(|| MonitorProtocolLimits::default().max_line_bytes),
            max_notifications_per_window: input
                .max_notifications_per_window
                .unwrap_or_else(|| MonitorProtocolLimits::default().max_notifications_per_window),
            notification_window_ms: input
                .notification_window_ms
                .unwrap_or_else(|| MonitorProtocolLimits::default().notification_window_ms),
            max_retained_diagnostic_bytes: input
                .max_retained_diagnostic_bytes
                .unwrap_or_else(|| MonitorProtocolLimits::default().max_retained_diagnostic_bytes),
        };
        let options = MonitorStartOptions {
            protocol: input.protocol,
            restart_class: input.restart_class.into_job_restart_class(),
            limits,
            delivery: input.delivery.into_job_delivery_kind(),
        };
        let job_id = match tool_call_id {
            Some(tool_call_id) => {
                self.job_manager
                    .spawn_monitor_for_call(
                        &input.command,
                        working_dir.as_deref(),
                        timeout_secs,
                        tool_call_id,
                        options,
                    )
                    .await
            }
            None => {
                let nonce = meerkat_core::time_compat::new_uuid_v7().to_string();
                self.job_manager
                    .spawn_monitor_for_call(
                        &input.command,
                        working_dir.as_deref(),
                        timeout_secs,
                        &nonce,
                        options,
                    )
                    .await
            }
        }
        .map_err(|error| BuiltinToolError::execution_failed(error.to_string()))?;
        Ok(ToolOutput::Json(serde_json::json!({
            "job_id": job_id.to_string(),
            "status": "running",
            "restart_class": input.restart_class.as_str(),
            "output_protocol": input.protocol,
            "delivery": input.delivery.as_str(),
            "message": format!("Durable monitor submitted with ID: {job_id}")
        })))
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum MonitorRestartDeclaration {
    Replayable,
    CheckpointResumable,
    #[default]
    NonResumable,
}

impl MonitorRestartDeclaration {
    const fn into_job_restart_class(self) -> meerkat_jobs::RestartClass {
        match self {
            Self::Replayable => meerkat_jobs::RestartClass::Replayable,
            Self::CheckpointResumable => meerkat_jobs::RestartClass::CheckpointResumable,
            Self::NonResumable => meerkat_jobs::RestartClass::NonResumable,
        }
    }

    const fn into_core_restart_class(self) -> meerkat_core::RestartClass {
        match self {
            Self::Replayable => meerkat_core::RestartClass::Replayable,
            Self::CheckpointResumable => meerkat_core::RestartClass::CheckpointResumable,
            Self::NonResumable => meerkat_core::RestartClass::NonResumable,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Replayable => "replayable",
            Self::CheckpointResumable => "checkpoint_resumable",
            Self::NonResumable => "non_resumable",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum MonitorDeliveryDeclaration {
    Record,
    #[default]
    Notification,
    EventSteer,
    EventQueue,
}

impl MonitorDeliveryDeclaration {
    const fn into_job_delivery_kind(self) -> meerkat_jobs::JobDeliveryKind {
        match self {
            Self::Record => meerkat_jobs::JobDeliveryKind::Record,
            Self::Notification => meerkat_jobs::JobDeliveryKind::Notification,
            Self::EventSteer => meerkat_jobs::JobDeliveryKind::Event {
                handling_mode: meerkat_core::HandlingMode::Steer,
            },
            Self::EventQueue => meerkat_jobs::JobDeliveryKind::Event {
                handling_mode: meerkat_core::HandlingMode::Queue,
            },
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Record => "record",
            Self::Notification => "notification",
            Self::EventSteer => "event_steer",
            Self::EventQueue => "event_queue",
        }
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
struct MonitorStartInput {
    command: String,
    #[serde(default)]
    working_dir: Option<String>,
    #[serde(default)]
    timeout_secs: Option<u64>,
    #[serde(default)]
    protocol: MonitorOutputProtocol,
    #[serde(default)]
    restart_class: MonitorRestartDeclaration,
    #[serde(default)]
    delivery: MonitorDeliveryDeclaration,
    #[serde(default)]
    max_line_bytes: Option<usize>,
    #[serde(default)]
    max_notifications_per_window: Option<usize>,
    #[serde(default)]
    notification_window_ms: Option<u64>,
    #[serde(default)]
    max_retained_diagnostic_bytes: Option<usize>,
}

#[async_trait]
impl BuiltinTool for MonitorStartTool {
    fn name(&self) -> &'static str {
        "monitor_start"
    }

    fn def(&self) -> ToolDef {
        ToolDef {
            name: "monitor_start".into(),
            description: "Start a high-trust durable script monitor. Framed JSONL emits typed notify/checkpoint/progress/complete events; notifications do not complete the job.".into(),
            input_schema: crate::schema::schema_for::<MonitorStartInput>(),
            provenance: Some(ToolProvenance {
                kind: ToolSourceKind::Shell,
                source_id: "monitor_start".into(),
            }),
        }
    }

    fn default_enabled(&self) -> bool {
        false
    }

    fn execution_contract(&self) -> meerkat_core::ToolExecutionContract {
        if !self.job_manager.exports_canonical_async_ops() {
            return meerkat_core::ToolExecutionContract::default();
        }
        let Ok(runner) = meerkat_core::RunnerIdentity::new("meerkat.monitor_script", "v1") else {
            return meerkat_core::ToolExecutionContract::default();
        };
        let Ok(policy) = DetachedToolExecutionPolicy::new(
            runner,
            meerkat_core::RestartClass::NonResumable,
            IdempotencyScope::ToolCall,
            DETACHED_SUBMISSION_TIMEOUT,
        ) else {
            return meerkat_core::ToolExecutionContract::default();
        };
        let Ok(contract) = meerkat_core::ToolExecutionContract::new(
            BTreeSet::from([meerkat_core::ToolExecutionMode::Detached]),
            meerkat_core::ToolExecutionMode::Detached,
            None,
            Some(policy),
        ) else {
            return meerkat_core::ToolExecutionContract::default();
        };
        contract
            .with_detached_restart_classes(BTreeSet::from([
                meerkat_core::RestartClass::Replayable,
                meerkat_core::RestartClass::CheckpointResumable,
                meerkat_core::RestartClass::NonResumable,
            ]))
            .unwrap_or_default()
    }

    fn resolve_execution_plan(
        &self,
        call: ToolCallView<'_>,
        resolution_context: &meerkat_core::ToolExecutionResolutionContext,
    ) -> Result<meerkat_core::ResolvedToolExecutionPlan, meerkat_core::ToolExecutionResolutionError>
    {
        let input: MonitorStartInput = serde_json::from_str(call.args.get()).map_err(|error| {
            meerkat_core::ToolExecutionResolutionError::InvalidArguments {
                tool_name: call.name.to_string(),
                reason: error.to_string(),
            }
        })?;
        self.execution_contract()
            .resolve_detached_with_restart_class(
                input.restart_class.into_core_restart_class(),
                resolution_context.deadlines().clone(),
            )
            .map_err(Into::into)
    }

    async fn call(&self, args: Value) -> Result<ToolOutput, BuiltinToolError> {
        self.call_with_tool_call_id(args, None).await
    }

    async fn call_with_context(
        &self,
        call: ToolCallView<'_>,
        args: Value,
        _context: &meerkat_core::ToolDispatchContext,
    ) -> Result<ToolOutput, BuiltinToolError> {
        self.call_with_tool_call_id(args, Some(call.id)).await
    }

    fn async_ops_for_output(&self, output: &ToolOutput) -> Vec<meerkat_core::ops::AsyncOpRef> {
        match output {
            ToolOutput::Json(value) => value
                .get("job_id")
                .and_then(Value::as_str)
                .map(JobId::from_string)
                .and_then(|job_id| self.job_manager.canonical_operation_for_job(&job_id))
                .map(meerkat_core::ops::AsyncOpRef::detached)
                .into_iter()
                .collect(),
            ToolOutput::Blocks(_) | ToolOutput::JsonWithEffects { .. } => Vec::new(),
        }
    }
}
