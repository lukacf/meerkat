use crate::{
    CreateScheduleRequest, Occurrence, OccurrenceFilter, OccurrenceId, OccurrencePhase,
    ScheduleDomainError, ScheduleId, ScheduleService, ScheduleStoreError, ScheduledSessionAction,
    SessionTargetBinding, TargetBinding, TriggerSpec, UpdateScheduleRequest,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use meerkat_core::error::ToolError;
use meerkat_core::types::SessionId;
use meerkat_core::types::{ToolCallView, ToolDef, ToolProvenance, ToolResult, ToolSourceKind};
use meerkat_core::{AgentToolDispatcher, ToolDispatchOutcome, ToolUnavailableReason};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::sync::Arc;

pub const INVALID_ARGUMENTS: i32 = -32602;
const INTERNAL_ERROR: i32 = -32000;
pub const NOT_FOUND: i32 = -32004;
pub const CAPABILITY_UNAVAILABLE: i32 = -32001;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScheduleToolError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl ScheduleToolError {
    fn invalid_arguments(message: impl Into<String>) -> Self {
        Self {
            code: INVALID_ARGUMENTS,
            message: message.into(),
            data: None,
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self {
            code: INTERNAL_ERROR,
            message: message.into(),
            data: None,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct EmptyArgs {}

#[derive(Debug, Deserialize)]
struct ScheduleIdArgs {
    schedule_id: String,
}

/// Arguments of `meerkat_schedule_occurrences`. Only `schedule_id` is
/// required; with nothing else set the tool returns every occurrence in full,
/// exactly as before the bounded options existed.
#[derive(Debug, Deserialize)]
struct OccurrencesArgs {
    schedule_id: String,
    #[serde(default)]
    occurrence_id: Option<String>,
    #[serde(default)]
    due_after_utc: Option<String>,
    #[serde(default)]
    due_before_utc: Option<String>,
    #[serde(default)]
    phase: Option<OccurrencePhase>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    view: OccurrencesView,
}

impl OccurrencesArgs {
    fn is_unbounded(&self) -> bool {
        self.occurrence_id.is_none()
            && self.due_after_utc.is_none()
            && self.due_before_utc.is_none()
            && self.phase.is_none()
            && self.limit.is_none()
            && self.cursor.is_none()
            && self.view == OccurrencesView::Full
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum OccurrencesView {
    #[default]
    Full,
    Summary,
}

/// The largest page `meerkat_schedule_occurrences` returns in one call.
const MAX_OCCURRENCE_PAGE: usize = 500;
const OCCURRENCE_CURSOR_PREFIX: &str = "occ1.";

#[derive(Debug, Deserialize)]
struct UpdateScheduleArgs {
    schedule_id: String,
    #[serde(flatten)]
    update: UpdateScheduleRequest,
}

/// The names of the schedule tools [`schedule_tools_list`] defines.
pub fn schedule_tool_names() -> &'static [&'static str] {
    &[
        "meerkat_schedule_create",
        "meerkat_schedule_get",
        "meerkat_schedule_list",
        "meerkat_schedule_update",
        "meerkat_schedule_pause",
        "meerkat_schedule_resume",
        "meerkat_schedule_delete",
        "meerkat_schedule_occurrences",
    ]
}

pub fn schedule_tools_list() -> Vec<Value> {
    vec![
        tool_descriptor(
            "meerkat_schedule_create",
            concat!(
                "Create a realm-scoped schedule: trigger, target, and delivery policies. ",
                "Use this for reminders, recurring automation, follow-ups, monitors, ",
                "and wakeups. For authoring guidance, load the schedule-workflow ",
                "companion skill when available.",
            ),
            create_schedule_schema(),
        ),
        tool_descriptor(
            "meerkat_schedule_get",
            "Fetch a single schedule by schedule_id. Returns the full schedule object including its current phase (active/paused/deleted), trigger, target, policies, revision number, and timestamps. Use this to inspect a schedule after creation or to confirm an update took effect.",
            schedule_id_schema("The schedule_id to fetch."),
        ),
        tool_descriptor(
            "meerkat_schedule_list",
            "List the schedules in the active realm. Returns an array of schedule objects for active and paused schedules; deleted schedules are not listed (their occurrence history stays readable with meerkat_schedule_occurrences). Use this to discover existing schedules before creating duplicates.",
            empty_schema(),
        ),
        tool_descriptor(
            "meerkat_schedule_update",
            "Update a live schedule by schedule_id. Only the fields you provide are changed; omitted fields keep their current values. The trigger and target must use the same JSON shapes as meerkat_schedule_create. Updating the trigger replans future occurrences. The schedule revision increments on each successful update.",
            update_schedule_schema(),
        ),
        tool_descriptor(
            "meerkat_schedule_pause",
            "Pause an active schedule. While paused, no new occurrences will fire. Already in-flight occurrences continue to completion. Use meerkat_schedule_resume to re-activate. Returns the updated schedule with phase=paused.",
            schedule_id_schema("The schedule_id to pause."),
        ),
        tool_descriptor(
            "meerkat_schedule_resume",
            "Resume a paused schedule, returning it to active phase. Future occurrences will be planned and fired according to the trigger. Returns the updated schedule with phase=active.",
            schedule_id_schema("The schedule_id to resume."),
        ),
        tool_descriptor(
            "meerkat_schedule_delete",
            "Permanently delete a schedule. The schedule transitions to phase=deleted and no further occurrences will fire. Historical occurrence records are preserved and can still be queried with meerkat_schedule_occurrences. This is irreversible -- a deleted schedule cannot be resumed.",
            schedule_id_schema("The schedule_id to delete."),
        ),
        tool_descriptor(
            "meerkat_schedule_occurrences",
            concat!(
                "List occurrences (planned and historical) for a schedule. Each occurrence has a phase: ",
                "pending (not yet due), claimed (picked up by driver), dispatching (delivery in progress), ",
                "awaiting_completion, completed, skipped, misfired, superseded, or delivery_failed. Use this ",
                "to verify a schedule is firing correctly, diagnose misfires, or check delivery history. ",
                "With only schedule_id it returns every occurrence in full, which can be large for a ",
                "long-running schedule. To read less: occurrence_id returns that one occurrence with all ",
                "of its delivery receipts (not found if it belongs to another schedule); due_after_utc / ",
                "due_before_utc (inclusive, RFC 3339 with any offset) and phase filter the rows; limit pages ",
                "them in due order and the reply's page.next_cursor continues after the last row; ",
                "view=summary drops the trigger, target action and machine state. A receipt with stage ",
                "completed records that delivery finished, not the outcome of the work the target did; a ",
                "missing or null materialized_session_id means the runtime session is unknown.",
            ),
            occurrences_schema(),
        ),
    ]
}

pub struct ScheduleToolDispatcher {
    service: ScheduleService,
    tool_defs: Arc<[Arc<ToolDef>]>,
}

impl ScheduleToolDispatcher {
    pub fn new(service: ScheduleService) -> Self {
        let tool_defs: Arc<[Arc<ToolDef>]> = schedule_tools_list()
            .into_iter()
            .map(|tool| {
                Arc::new(ToolDef {
                    audience: Default::default(),
                    name: tool["name"].as_str().unwrap_or_default().into(),
                    description: tool["description"].as_str().unwrap_or_default().to_string(),
                    input_schema: tool["inputSchema"].clone(),
                    provenance: Some(ToolProvenance {
                        kind: ToolSourceKind::Schedule,
                        source_id: "schedule".into(),
                        artifact: None,
                    }),
                })
            })
            .collect::<Vec<_>>()
            .into();
        Self { service, tool_defs }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl AgentToolDispatcher for ScheduleToolDispatcher {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        Arc::clone(&self.tool_defs)
    }

    fn tool_catalog_capabilities(&self) -> meerkat_core::ToolCatalogCapabilities {
        meerkat_core::ToolCatalogCapabilities {
            exact_catalog: true,
            may_require_catalog_control_plane: false,
        }
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        if !self.tool_defs.iter().any(|tool| tool.name == call.name) {
            return Err(ToolError::not_found(call.name));
        }

        let arguments: Value = serde_json::from_str(call.args.get()).map_err(|error| {
            ToolError::invalid_arguments(
                call.name,
                format!("invalid schedule tool-call arguments JSON: {error}"),
            )
        })?;
        let result = run_schedule_tools_call(&self.service, call.name, &arguments)
            .await
            .map_err(|failure| map_schedule_tool_dispatch_error(call.name, failure))?;

        Ok(ToolResult::new(call.id.to_string(), result.to_string(), false).into())
    }
}

/// Resolves the agent-facing `current_session` shortcut into a persisted target.
pub trait CurrentSessionScheduleTargetResolver: Send + Sync {
    fn resolve_current_session_target(
        &self,
        current_session_id: &SessionId,
        action: ScheduledSessionAction,
    ) -> TargetBinding;
}

#[derive(Debug, Default)]
pub struct DefaultCurrentSessionScheduleTargetResolver;

impl CurrentSessionScheduleTargetResolver for DefaultCurrentSessionScheduleTargetResolver {
    fn resolve_current_session_target(
        &self,
        current_session_id: &SessionId,
        action: ScheduledSessionAction,
    ) -> TargetBinding {
        TargetBinding::session(SessionTargetBinding::ResumableSession {
            session_id: current_session_id.clone(),
            action,
        })
    }
}

/// Session-scoped adapter for agent-facing schedule tools.
///
/// The durable schedule model intentionally stores concrete durable targets.
/// This adapter is only an authoring convenience: it lets an agent say
/// `current_session`, then asks the host resolver which target should be
/// persisted. The default resolver preserves the historical
/// `resumable_session(session_id)` behavior.
pub struct CurrentSessionScheduleToolDispatcher {
    inner: Arc<dyn AgentToolDispatcher>,
    current_session_id: SessionId,
    target_resolver: Arc<dyn CurrentSessionScheduleTargetResolver>,
    /// The CREATING session's effective tool access policy.
    ///
    /// Scheduled spawn/fork-helper targets fire later under host authority,
    /// where MobMachine spawn admission cannot see the creator — without
    /// creation-time containment a call-level-gated agent could schedule a
    /// helper that escapes its own policy. When set, helper targets created
    /// or updated through this dispatcher inherit exactly this policy
    /// (absent/`inherit` specs are stamped; a diverging explicit spec is
    /// denied).
    creator_tool_access_policy: Option<meerkat_core::ops::ToolAccessPolicy>,
}

impl CurrentSessionScheduleToolDispatcher {
    pub fn new(inner: Arc<dyn AgentToolDispatcher>, current_session_id: SessionId) -> Self {
        Self::new_with_resolver(
            inner,
            current_session_id,
            Arc::new(DefaultCurrentSessionScheduleTargetResolver),
        )
    }

    pub fn new_with_resolver(
        inner: Arc<dyn AgentToolDispatcher>,
        current_session_id: SessionId,
        target_resolver: Arc<dyn CurrentSessionScheduleTargetResolver>,
    ) -> Self {
        Self {
            inner,
            current_session_id,
            target_resolver,
            creator_tool_access_policy: None,
        }
    }

    /// Carry the creating session's effective tool access policy so scheduled
    /// helper targets stay contained by it (see the field docs).
    #[must_use]
    pub fn with_creator_tool_access_policy(
        mut self,
        policy: Option<meerkat_core::ops::ToolAccessPolicy>,
    ) -> Self {
        self.creator_tool_access_policy = policy;
        self
    }

    fn rewrite_call_arguments(
        &self,
        call: &ToolCallView<'_>,
    ) -> Result<Box<serde_json::value::RawValue>, ToolError> {
        let args: Value = serde_json::from_str(call.args.get()).map_err(|error| {
            ToolError::invalid_arguments(
                call.name,
                format!("invalid schedule tool-call arguments JSON: {error}"),
            )
        })?;
        let rewritten = rewrite_current_session_target(
            args,
            &self.current_session_id,
            self.target_resolver.as_ref(),
        )
        .map_err(|error| ToolError::invalid_arguments(call.name, error))?;
        let rewritten = enforce_creator_tool_access_policy(
            rewritten,
            self.creator_tool_access_policy.as_ref(),
            call.name,
        )?;
        serde_json::value::RawValue::from_string(rewritten.to_string()).map_err(|error| {
            ToolError::invalid_arguments(
                call.name,
                format!("failed to encode rewritten schedule arguments: {error}"),
            )
        })
    }
}

/// Enforce the creating session's tool access policy on scheduled mob
/// spawn/fork-helper targets.
///
/// An ungated creator (`None`) passes through unchanged. For a gated creator,
/// a helper target's `options.tool_access_policy` must resolve to EXACTLY the
/// creator's policy: absent or `inherit` specs are stamped with it, an equal
/// explicit spec passes, and any diverging explicit spec is denied — the
/// scheduled spawn fires later under host authority, so creation is the only
/// seam where the creator's containment can be applied.
fn enforce_creator_tool_access_policy(
    mut args: Value,
    creator: Option<&meerkat_core::ops::ToolAccessPolicy>,
    tool_name: &str,
) -> Result<Value, ToolError> {
    let Some(creator) = creator else {
        return Ok(args);
    };
    let Some(target) = args
        .as_object_mut()
        .and_then(|object| object.get_mut("target"))
        .and_then(Value::as_object_mut)
    else {
        return Ok(args);
    };
    if target.get("target_kind").and_then(Value::as_str) != Some("mob") {
        return Ok(args);
    }
    let is_helper_target = matches!(
        target.get("type").and_then(Value::as_str),
        Some("spawn_helper" | "fork_helper")
    );
    if !is_helper_target {
        return Ok(args);
    }
    if matches!(creator, meerkat_core::ops::ToolAccessPolicy::Inherit) {
        // A creator policy of `Inherit` at this seam is a wiring fault (the
        // spawn chain resolves Inherit before the session is built); refusing
        // helper targets outright is the only fail-closed option.
        return Err(ToolError::access_denied(tool_name));
    }
    let creator_value = serde_json::to_value(creator).map_err(|error| {
        ToolError::execution_failed(format!(
            "tool '{tool_name}' failed to encode the creator tool access policy: {error}"
        ))
    })?;
    let options = target
        .entry("options")
        .or_insert_with(|| Value::Object(Map::new()));
    let Some(options) = options.as_object_mut() else {
        // Malformed options are the schedule service's typed parse error to
        // surface; do not mask it here.
        return Ok(args);
    };
    match options.get("tool_access_policy") {
        None | Some(Value::Null) => {
            options.insert("tool_access_policy".to_string(), creator_value);
        }
        Some(existing) => {
            let requested: meerkat_core::ops::ToolAccessPolicy =
                serde_json::from_value(existing.clone()).map_err(|error| {
                    ToolError::invalid_arguments(
                        tool_name,
                        format!("invalid scheduled helper tool_access_policy: {error}"),
                    )
                })?;
            match requested {
                meerkat_core::ops::ToolAccessPolicy::Inherit => {
                    options.insert("tool_access_policy".to_string(), creator_value);
                }
                explicit if &explicit == creator => {}
                _ => {
                    return Err(ToolError::access_denied(tool_name));
                }
            }
        }
    }
    Ok(args)
}

fn map_schedule_plan_rewrite_error(
    tool_name: &str,
    error: ToolError,
) -> meerkat_core::ToolExecutionResolutionError {
    match error {
        ToolError::NotFound { .. } => meerkat_core::ToolExecutionResolutionError::NotFound {
            tool_name: tool_name.to_string(),
        },
        ToolError::Unavailable { reason, .. } => {
            meerkat_core::ToolExecutionResolutionError::Unavailable {
                tool_name: tool_name.to_string(),
                reason,
            }
        }
        ToolError::AccessDenied { .. } => {
            meerkat_core::ToolExecutionResolutionError::AccessDenied {
                tool_name: tool_name.to_string(),
            }
        }
        ToolError::InvalidArguments { reason, .. } => {
            meerkat_core::ToolExecutionResolutionError::InvalidArguments {
                tool_name: tool_name.to_string(),
                reason,
            }
        }
        other => meerkat_core::ToolExecutionResolutionError::InvalidArguments {
            tool_name: tool_name.to_string(),
            reason: format!("schedule call rewrite failed before dispatch: {other}"),
        },
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl AgentToolDispatcher for CurrentSessionScheduleToolDispatcher {
    async fn resolve_tool_application(
        &self,
        source_tool: &str,
        request: &meerkat_core::ToolApplicationRequest,
        invocation: &serde_json::Value,
        context: &meerkat_core::ToolDispatchContext,
    ) -> Result<meerkat_core::tool_application::ToolApplicationResolution, ToolError> {
        let catalog = self.tool_catalog();
        let entry = catalog
            .iter()
            .find(|entry| entry.tool.name == source_tool)
            .ok_or_else(|| ToolError::not_found(source_tool))?;
        if let Some(reason) = entry.callability.unavailable_reason() {
            return Err(ToolError::unavailable(source_tool, reason));
        }
        self.inner
            .resolve_tool_application(source_tool, request, invocation, context)
            .await
    }

    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        self.tool_catalog()
            .iter()
            .filter(|entry| entry.callability.is_callable())
            .map(|entry| Arc::clone(&entry.tool))
            .collect::<Vec<_>>()
            .into()
    }

    fn tool_catalog_capabilities(&self) -> meerkat_core::ToolCatalogCapabilities {
        self.inner.tool_catalog_capabilities()
    }

    fn tool_catalog(&self) -> Arc<[meerkat_core::ToolCatalogEntry]> {
        self.inner
            .tool_catalog()
            .iter()
            .map(|entry| {
                let mut rewritten = entry.clone();
                rewritten.tool = Arc::new(current_session_tool_def(entry.tool.as_ref()));
                rewritten
            })
            .collect::<Vec<_>>()
            .into()
    }

    fn pending_catalog_sources(&self) -> Arc<[String]> {
        self.inner.pending_catalog_sources()
    }

    fn execution_binding_fingerprint(
        &self,
        tool_name: &str,
    ) -> Result<
        meerkat_core::EphemeralToolBindingFingerprint,
        meerkat_core::ToolExecutionResolutionError,
    > {
        let catalog = self.tool_catalog();
        let entry = catalog
            .iter()
            .find(|entry| entry.tool.name == tool_name)
            .ok_or_else(|| meerkat_core::ToolExecutionResolutionError::NotFound {
                tool_name: tool_name.to_string(),
            })?;
        Ok(
            meerkat_core::ephemeral_tool_catalog_binding_fingerprint(entry)
                .with_live_authority(0, 0)
                .with_dependency(&self.inner.execution_binding_fingerprint(tool_name)?),
        )
    }

    fn resolve_execution_plan(
        &self,
        call: ToolCallView<'_>,
        dispatch_context: &meerkat_core::ToolDispatchContext,
        resolution_context: &meerkat_core::ToolExecutionResolutionContext,
    ) -> Result<meerkat_core::ResolvedToolExecutionPlan, meerkat_core::ToolExecutionResolutionError>
    {
        let catalog = self.tool_catalog();
        let entry = catalog
            .iter()
            .find(|entry| entry.tool.name == call.name)
            .ok_or_else(|| meerkat_core::ToolExecutionResolutionError::NotFound {
                tool_name: call.name.to_string(),
            })?;
        if let Some(reason) = entry.callability.unavailable_reason() {
            return Err(meerkat_core::ToolExecutionResolutionError::Unavailable {
                tool_name: call.name.to_string(),
                reason,
            });
        }

        if call.name != "meerkat_schedule_create" && call.name != "meerkat_schedule_update" {
            return self
                .inner
                .resolve_execution_plan(call, dispatch_context, resolution_context);
        }

        let rewritten_raw = self
            .rewrite_call_arguments(&call)
            .map_err(|error| map_schedule_plan_rewrite_error(call.name, error))?;
        self.inner.resolve_execution_plan(
            ToolCallView {
                id: call.id,
                name: call.name,
                args: &rewritten_raw,
            },
            dispatch_context,
            resolution_context,
        )
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        self.dispatch_with_context(call, &meerkat_core::ToolDispatchContext::default())
            .await
    }

    async fn dispatch_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &meerkat_core::ToolDispatchContext,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        let catalog = self.tool_catalog();
        let entry = catalog
            .iter()
            .find(|entry| entry.tool.name == call.name)
            .ok_or_else(|| ToolError::not_found(call.name))?;
        if let Some(reason) = entry.callability.unavailable_reason() {
            return Err(ToolError::unavailable(call.name, reason));
        }

        if call.name != "meerkat_schedule_create" && call.name != "meerkat_schedule_update" {
            return self.inner.dispatch_with_context(call, context).await;
        }

        let rewritten_raw = self.rewrite_call_arguments(&call)?;
        self.inner
            .dispatch_with_context(
                ToolCallView {
                    id: call.id,
                    name: call.name,
                    args: &rewritten_raw,
                },
                context,
            )
            .await
    }

    async fn dispatch_resolved_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &meerkat_core::ToolDispatchContext,
        plan: &meerkat_core::ResolvedToolExecutionPlan,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        let catalog = self.tool_catalog();
        let entry = catalog
            .iter()
            .find(|entry| entry.tool.name == call.name)
            .ok_or_else(|| ToolError::not_found(call.name))?;
        if let Some(reason) = entry.callability.unavailable_reason() {
            return Err(ToolError::unavailable(call.name, reason));
        }

        if call.name != "meerkat_schedule_create" && call.name != "meerkat_schedule_update" {
            return self
                .inner
                .dispatch_resolved_with_context(call, context, plan)
                .await;
        }

        let rewritten_raw = self.rewrite_call_arguments(&call)?;
        self.inner
            .dispatch_resolved_with_context(
                ToolCallView {
                    id: call.id,
                    name: call.name,
                    args: &rewritten_raw,
                },
                context,
                plan,
            )
            .await
    }
}

pub async fn handle_schedule_tools_call(
    service: &ScheduleService,
    name: &str,
    arguments: &Value,
) -> Result<Value, ScheduleToolError> {
    run_schedule_tools_call(service, name, arguments)
        .await
        .map_err(ScheduleToolFailure::into_wire_error)
}

/// Why a schedule tool call failed. The service's error stays typed until a
/// caller renders it, so the in-process tool dispatchers can report a stopped
/// schedule host as an unavailable tool while the wire keeps its error code.
pub(crate) enum ScheduleToolFailure {
    /// The call's arguments, or the encoding of its result.
    Call(ScheduleToolError),
    /// The schedule service refused or failed the operation.
    Service(ScheduleDomainError),
}

impl From<ScheduleToolError> for ScheduleToolFailure {
    fn from(error: ScheduleToolError) -> Self {
        Self::Call(error)
    }
}

impl From<ScheduleDomainError> for ScheduleToolFailure {
    fn from(error: ScheduleDomainError) -> Self {
        Self::Service(error)
    }
}

impl ScheduleToolFailure {
    /// The agent-facing error when the surface's schedule host stopped and
    /// no host judges identity targets: a host attached later judges them
    /// again, so the tool is unavailable for now rather than failed.
    pub(crate) fn unavailable_tool_error(&self, name: &str) -> Option<ToolError> {
        matches!(self, Self::Service(ScheduleDomainError::DriverStopped))
            .then(|| ToolError::unavailable(name, ToolUnavailableReason::TemporarilyUnavailable))
    }

    /// The wire error for this failure.
    pub(crate) fn into_wire_error(self) -> ScheduleToolError {
        match self {
            Self::Call(error) => error,
            Self::Service(error) => map_schedule_error(error),
        }
    }
}

pub(crate) async fn run_schedule_tools_call(
    service: &ScheduleService,
    name: &str,
    arguments: &Value,
) -> Result<Value, ScheduleToolFailure> {
    match name {
        "meerkat_schedule_create" => {
            let request: CreateScheduleRequest = parse_args(name, arguments)?;
            let schedule = service.create(request).await?;
            Ok(encode(name, schedule)?)
        }
        "meerkat_schedule_get" => {
            let args: ScheduleIdArgs = parse_args(name, arguments)?;
            let schedule_id = parse_schedule_id(&args.schedule_id)?;
            let schedule = service.get(&schedule_id).await?;
            Ok(encode(name, schedule)?)
        }
        "meerkat_schedule_list" => {
            let _: EmptyArgs = parse_args(name, arguments)?;
            let schedules = service.list().await?;
            Ok(encode(name, json!({ "schedules": schedules }))?)
        }
        "meerkat_schedule_update" => {
            let args: UpdateScheduleArgs = parse_args(name, arguments)?;
            let schedule_id = parse_schedule_id(&args.schedule_id)?;
            let schedule = service.update(&schedule_id, args.update).await?;
            Ok(encode(name, schedule)?)
        }
        "meerkat_schedule_pause" => {
            let args: ScheduleIdArgs = parse_args(name, arguments)?;
            let schedule_id = parse_schedule_id(&args.schedule_id)?;
            let schedule = service.pause(&schedule_id).await?;
            Ok(encode(name, schedule)?)
        }
        "meerkat_schedule_resume" => {
            let args: ScheduleIdArgs = parse_args(name, arguments)?;
            let schedule_id = parse_schedule_id(&args.schedule_id)?;
            let schedule = service.resume(&schedule_id).await?;
            Ok(encode(name, schedule)?)
        }
        "meerkat_schedule_delete" => {
            let args: ScheduleIdArgs = parse_args(name, arguments)?;
            let schedule_id = parse_schedule_id(&args.schedule_id)?;
            let schedule = service.delete(&schedule_id).await?;
            Ok(encode(name, schedule)?)
        }
        "meerkat_schedule_occurrences" => {
            let args: OccurrencesArgs = parse_args(name, arguments)?;
            let schedule_id = parse_schedule_id(&args.schedule_id)?;
            if args.is_unbounded() {
                let occurrences = service.list_occurrences(&schedule_id).await?;
                return Ok(encode_occurrences(occurrences)?);
            }
            bounded_occurrences(service, &schedule_id, &args).await
        }
        other => Err(ScheduleToolError::invalid_arguments(format!(
            "unknown schedule tool: {other}"
        ))
        .into()),
    }
}

fn parse_args<T>(name: &str, arguments: &Value) -> Result<T, ScheduleToolError>
where
    T: DeserializeOwned,
{
    serde_json::from_value(arguments.clone()).map_err(|error| {
        ScheduleToolError::invalid_arguments(format!("invalid arguments for {name}: {error}"))
    })
}

fn parse_schedule_id(raw: &str) -> Result<ScheduleId, ScheduleToolError> {
    ScheduleId::parse(raw).map_err(|error| {
        ScheduleToolError::invalid_arguments(format!("invalid schedule_id: {error}"))
    })
}

fn encode(name: &str, value: impl Serialize) -> Result<Value, ScheduleToolError> {
    serde_json::to_value(value).map_err(|error| {
        ScheduleToolError::internal(format!("failed to encode {name} result: {error}"))
    })
}

fn encode_occurrences(occurrences: Vec<Occurrence>) -> Result<Value, ScheduleToolError> {
    encode(
        "meerkat_schedule_occurrences",
        json!({ "occurrences": occurrences }),
    )
}

/// `meerkat_schedule_occurrences` with any bounded option set. Phase and due
/// bounds go to the store as an [`OccurrenceFilter`], which applies them in
/// its query; the cursor and page cut are applied here on the store's
/// `(due_at, schedule_revision, occurrence_ordinal)` order.
async fn bounded_occurrences(
    service: &ScheduleService,
    schedule_id: &ScheduleId,
    args: &OccurrencesArgs,
) -> Result<Value, ScheduleToolFailure> {
    if let Some(raw) = &args.occurrence_id {
        if args.due_after_utc.is_some()
            || args.due_before_utc.is_some()
            || args.phase.is_some()
            || args.limit.is_some()
            || args.cursor.is_some()
        {
            return Err(ScheduleToolError::invalid_arguments(
                "occurrence_id reads one occurrence; it cannot be combined with \
                 due_after_utc, due_before_utc, phase, limit or cursor",
            )
            .into());
        }
        let occurrence_id = OccurrenceId::parse(raw).map_err(|error| {
            ScheduleToolError::invalid_arguments(format!("invalid occurrence_id: {error}"))
        })?;
        let Some(occurrence) = service
            .get_schedule_occurrence(schedule_id, &occurrence_id)
            .await?
        else {
            return Err(ScheduleToolError {
                code: NOT_FOUND,
                message: "occurrence not found for this schedule".into(),
                data: None,
            }
            .into());
        };
        let receipts = service.list_occurrence_receipts(&occurrence_id).await?;
        let rendered = render_occurrence(&occurrence, args.view)?;
        return Ok(encode(
            "meerkat_schedule_occurrences",
            json!({ "occurrences": [rendered], "receipts": receipts }),
        )?);
    }

    let due_after = parse_due_bound("due_after_utc", args.due_after_utc.as_deref())?;
    let due_before = parse_due_bound("due_before_utc", args.due_before_utc.as_deref())?;
    if let (Some(after), Some(before)) = (due_after, due_before)
        && after > before
    {
        return Err(ScheduleToolError::invalid_arguments(
            "due_after_utc must not be later than due_before_utc",
        )
        .into());
    }
    let limit = match args.limit {
        Some(0) => {
            return Err(ScheduleToolError::invalid_arguments("limit must be at least 1").into());
        }
        Some(limit) if limit > MAX_OCCURRENCE_PAGE => {
            return Err(ScheduleToolError::invalid_arguments(format!(
                "limit must be at most {MAX_OCCURRENCE_PAGE}"
            ))
            .into());
        }
        other => other,
    };
    let after_key = match args.cursor.as_deref() {
        Some(_) if limit.is_none() => {
            return Err(ScheduleToolError::invalid_arguments("cursor requires limit").into());
        }
        Some(raw) => Some(OccurrencePageKey::decode(raw)?),
        None => None,
    };

    // A cursor narrows the store read to rows due at or after its row; rows at
    // that same instant up to and including the cursor row are dropped below,
    // so the store must not cut the page before that happens.
    let cursor_due = after_key
        .as_ref()
        .and_then(|key| DateTime::<Utc>::from_timestamp_millis(key.due_at_ms));
    let store_due_after = match (due_after, cursor_due) {
        (Some(after), Some(cursor)) => Some(after.max(cursor)),
        (after, cursor) => after.or(cursor),
    };
    let store_limit = match (limit, &after_key) {
        (Some(limit), None) => Some(limit + 1),
        _ => None,
    };
    let mut rows = service
        .list_occurrences_matching(
            schedule_id,
            OccurrenceFilter {
                schedule_id: None,
                phase: args.phase,
                include_terminal: true,
                due_after_utc: store_due_after,
                due_before_utc: due_before,
                limit: store_limit,
            },
        )
        .await?;
    rows.sort_by_key(OccurrencePageKey::of);
    if let Some(after) = &after_key {
        rows.retain(|occurrence| OccurrencePageKey::of(occurrence) > *after);
    }

    let mut page = None;
    if let Some(limit) = limit {
        let has_more = rows.len() > limit;
        rows.truncate(limit);
        let next_cursor = if has_more {
            rows.last().map(|last| OccurrencePageKey::of(last).encode())
        } else {
            None
        };
        page = Some(json!({
            "returned": rows.len(),
            "has_more": has_more,
            "next_cursor": next_cursor,
        }));
    }
    let rendered = rows
        .iter()
        .map(|occurrence| render_occurrence(occurrence, args.view))
        .collect::<Result<Vec<_>, _>>()?;
    let mut reply = json!({ "occurrences": rendered });
    if let (Some(page), Some(object)) = (page, reply.as_object_mut()) {
        object.insert("page".into(), page);
    }
    Ok(reply)
}

fn parse_due_bound(
    field: &str,
    raw: Option<&str>,
) -> Result<Option<DateTime<Utc>>, ScheduleToolError> {
    raw.map(|value| {
        DateTime::parse_from_rfc3339(value)
            .map(|parsed| parsed.with_timezone(&Utc))
            .map_err(|error| {
                ScheduleToolError::invalid_arguments(format!(
                    "{field} must be an RFC 3339 timestamp: {error}"
                ))
            })
    })
    .transpose()
}

/// The order the store lists one schedule's occurrences in, and the position
/// a page cursor names. A revision bump re-plans with new ordinals, so
/// `(due_at, revision, ordinal)` stays a strict, stable order across it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct OccurrencePageKey {
    due_at_ms: i64,
    schedule_revision: u64,
    occurrence_ordinal: u64,
}

impl OccurrencePageKey {
    fn of(occurrence: &Occurrence) -> Self {
        Self {
            due_at_ms: occurrence.due_at_utc.timestamp_millis(),
            schedule_revision: occurrence.schedule_revision.0,
            occurrence_ordinal: occurrence.occurrence_ordinal.0,
        }
    }

    fn encode(&self) -> String {
        format!(
            "{OCCURRENCE_CURSOR_PREFIX}{}.{}.{}",
            self.due_at_ms, self.schedule_revision, self.occurrence_ordinal
        )
    }

    fn decode(raw: &str) -> Result<Self, ScheduleToolError> {
        let invalid = || ScheduleToolError::invalid_arguments("invalid cursor");
        let body = raw
            .strip_prefix(OCCURRENCE_CURSOR_PREFIX)
            .ok_or_else(invalid)?;
        let mut parts = body.split('.');
        let (Some(due), Some(revision), Some(ordinal), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(invalid());
        };
        Ok(Self {
            due_at_ms: due.parse().map_err(|_| invalid())?,
            schedule_revision: revision.parse().map_err(|_| invalid())?,
            occurrence_ordinal: ordinal.parse().map_err(|_| invalid())?,
        })
    }
}

fn render_occurrence(
    occurrence: &Occurrence,
    view: OccurrencesView,
) -> Result<Value, ScheduleToolError> {
    match view {
        OccurrencesView::Full => encode("meerkat_schedule_occurrences", occurrence),
        OccurrencesView::Summary => occurrence_summary(occurrence),
    }
}

/// A compact occurrence: identity, timing, delivery state and the target's
/// kind, without the trigger, the target's action payload or machine state.
fn occurrence_summary(occurrence: &Occurrence) -> Result<Value, ScheduleToolError> {
    let target = encode("meerkat_schedule_occurrences", &occurrence.target_snapshot)?;
    let target_kind: Map<String, Value> = target
        .as_object()
        .map(|object| {
            object
                .iter()
                .filter(|(_, value)| value.is_string())
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect()
        })
        .unwrap_or_default();
    let (due_at_local, timezone) =
        local_due_at(&occurrence.trigger_snapshot, occurrence.due_at_utc).unzip();
    let last_receipt = occurrence.last_receipt.as_ref().map(|receipt| {
        json!({
            "receipt_id": receipt.receipt_id,
            "occurrence_id": receipt.occurrence_id,
            "attempt": receipt.attempt,
            "stage": receipt.stage,
            "recorded_at_utc": receipt.recorded_at_utc,
            "materialized_session_id": receipt.materialized_session_id,
        })
    });
    encode(
        "meerkat_schedule_occurrences",
        json!({
            "occurrence_id": occurrence.occurrence_id,
            "schedule_id": occurrence.schedule_id,
            "schedule_revision": occurrence.schedule_revision,
            "occurrence_ordinal": occurrence.occurrence_ordinal,
            "phase": occurrence.phase,
            "due_at_utc": occurrence.due_at_utc,
            "due_at_local": due_at_local,
            "timezone": timezone,
            "attempt_count": occurrence.attempt_count,
            "created_at_utc": occurrence.created_at_utc,
            "claimed_at_utc": occurrence.claimed_at_utc,
            "dispatched_at_utc": occurrence.dispatched_at_utc,
            "completed_at_utc": occurrence.completed_at_utc,
            "superseded_by_revision": occurrence.superseded_by_revision,
            "last_receipt": last_receipt,
            "runtime_outcome": occurrence.runtime_outcome,
            "failure_class": occurrence.failure_class,
            "failure_detail": occurrence.failure_detail,
            "target": target_kind,
        }),
    )
}

/// The wall-clock time a calendar trigger meant for `due_at_utc`, in the
/// trigger's own timezone, with that timezone's name. Other triggers are
/// defined in UTC and have no local time.
fn local_due_at(trigger: &TriggerSpec, due_at_utc: DateTime<Utc>) -> Option<(String, String)> {
    let TriggerSpec::Calendar(calendar) = trigger else {
        return None;
    };
    let timezone = calendar.timezone.parse::<chrono_tz::Tz>().ok()?;
    Some((
        due_at_utc.with_timezone(&timezone).to_rfc3339(),
        calendar.timezone.clone(),
    ))
}

fn map_schedule_error(error: ScheduleDomainError) -> ScheduleToolError {
    match error {
        ScheduleDomainError::Store(ScheduleStoreError::ScheduleNotFound { .. }) => {
            ScheduleToolError {
                code: NOT_FOUND,
                message: "schedule not found".into(),
                data: None,
            }
        }
        // No schedule capability here: the backend lacks it, or the schedule
        // host that judged identity targets has stopped.
        ScheduleDomainError::Store(ScheduleStoreError::UnsupportedBackend { .. })
        | ScheduleDomainError::DriverStopped => ScheduleToolError {
            code: CAPABILITY_UNAVAILABLE,
            message: error.to_string(),
            data: None,
        },
        ScheduleDomainError::InvalidSchedule(_)
        | ScheduleDomainError::InvalidTrigger(_)
        | ScheduleDomainError::InvalidCron(_) => ScheduleToolError {
            code: INVALID_ARGUMENTS,
            message: error.to_string(),
            data: None,
        },
        other => ScheduleToolError {
            code: INTERNAL_ERROR,
            message: other.to_string(),
            data: None,
        },
    }
}

fn map_schedule_tool_dispatch_error(name: &str, failure: ScheduleToolFailure) -> ToolError {
    if let Some(unavailable) = failure.unavailable_tool_error(name) {
        return unavailable;
    }
    let error = failure.into_wire_error();
    if error.code == INVALID_ARGUMENTS {
        return ToolError::invalid_arguments(name, error.message);
    }
    ToolError::ExecutionFailed {
        message: format!("{name}: {}", error.message),
    }
}

fn tool_descriptor(name: &'static str, description: &'static str, input_schema: Value) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": input_schema,
    })
}

fn current_session_tool_def(tool: &ToolDef) -> ToolDef {
    let mut rewritten = tool.clone();
    if rewritten.name == "meerkat_schedule_create" || rewritten.name == "meerkat_schedule_update" {
        rewritten.description.push_str(
            "\n\nAgent-facing shortcut: session targets may also use type=\"current_session\". \
             The tool host resolves it to a durable target before persistence. Hosts with stable \
             agent identities may persist identity targets instead of raw session ids.",
        );
        add_current_session_to_schema(&mut rewritten.input_schema);
    }
    rewritten
}

fn add_current_session_to_schema(schema: &mut Value) {
    let Some(target_schema) = schema
        .get_mut("properties")
        .and_then(Value::as_object_mut)
        .and_then(|properties| properties.get_mut("target"))
    else {
        return;
    };
    let Some(one_of) = target_schema.get_mut("oneOf").and_then(Value::as_array_mut) else {
        return;
    };
    let Some(session_schema) = one_of.first_mut().and_then(Value::as_object_mut) else {
        return;
    };
    let Some(type_values) = session_schema
        .get_mut("properties")
        .and_then(Value::as_object_mut)
        .and_then(|properties| properties.get_mut("type"))
        .and_then(Value::as_object_mut)
        .and_then(|type_schema| type_schema.get_mut("enum"))
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    if !type_values
        .iter()
        .any(|value| value.as_str() == Some("current_session"))
    {
        type_values.push(Value::String("current_session".into()));
    }
}

fn rewrite_current_session_target(
    mut args: Value,
    current_session_id: &SessionId,
    resolver: &dyn CurrentSessionScheduleTargetResolver,
) -> Result<Value, String> {
    let Some(target) = args
        .as_object_mut()
        .and_then(|object| object.get_mut("target"))
        .and_then(Value::as_object_mut)
    else {
        return Ok(args);
    };

    rewrite_current_session_target_object(target, current_session_id, resolver)?;
    Ok(args)
}

/// Tool-host-only target kinds. The `current_session` pseudo-variant is a
/// shortcut the host injects into the tool schema; it is not part of the
/// persisted `TargetBinding`/`SessionTargetBinding` domain. Deserializing the
/// raw target into this closed type replaces the
/// `target["target_kind"] == "session" && target["type"] == "current_session"`
/// string oracle with a typed parse-at-boundary classification.
#[derive(Deserialize)]
#[serde(tag = "target_kind", rename_all = "snake_case")]
enum HostTargetProbe {
    Session(HostSessionTargetProbe),
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum HostSessionTargetProbe {
    CurrentSession,
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct HostCurrentSessionTarget {
    action: ScheduledSessionAction,
}

impl HostTargetProbe {
    fn is_current_session(&self) -> bool {
        matches!(self, Self::Session(HostSessionTargetProbe::CurrentSession))
    }
}

fn rewrite_current_session_target_object(
    target: &mut Map<String, Value>,
    session_id: &SessionId,
    resolver: &dyn CurrentSessionScheduleTargetResolver,
) -> Result<(), String> {
    let is_current_session =
        serde_json::from_value::<HostTargetProbe>(Value::Object(target.clone()))
            .map(|probe| probe.is_current_session())
            .unwrap_or(false);

    if !is_current_session {
        return Ok(());
    }

    let current = serde_json::from_value::<HostCurrentSessionTarget>(Value::Object(target.clone()))
        .map_err(|error| format!("invalid current_session target: {error}"))?;
    let resolved = resolver.resolve_current_session_target(session_id, current.action);
    let resolved_value = serde_json::to_value(resolved)
        .map_err(|error| format!("failed to encode resolved current_session target: {error}"))?;
    let resolved_object = resolved_value
        .as_object()
        .ok_or_else(|| "resolved current_session target was not an object".to_string())?;
    *target = resolved_object.clone();
    Ok(())
}

fn empty_schema() -> Value {
    json!({
        "type": "object",
        "properties": {},
        "additionalProperties": false,
    })
}

fn occurrences_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "schedule_id": {
                "type": "string",
                "description": "The schedule_id whose occurrences should be listed.",
            },
            "occurrence_id": {
                "type": "string",
                "description": "Read only this occurrence, with all of its delivery receipts. Not found when it belongs to a different schedule. Cannot be combined with the filters or paging.",
            },
            "due_after_utc": {
                "type": "string",
                "description": "Only occurrences due at or after this instant (RFC 3339, any offset; e.g. 2026-03-29T00:00:00+01:00).",
            },
            "due_before_utc": {
                "type": "string",
                "description": "Only occurrences due at or before this instant (RFC 3339, any offset). Inclusive: to cover one local day, pass its last second.",
            },
            "phase": {
                "type": "string",
                "enum": [
                    "pending", "claimed", "dispatching", "awaiting_completion", "completed",
                    "skipped", "misfired", "superseded", "delivery_failed"
                ],
                "description": "Only occurrences in this phase.",
            },
            "limit": {
                "type": "integer",
                "description": "Return at most this many occurrences (1 to 500) in due order. The reply then carries page.returned (rows in this page), page.has_more and page.next_cursor.",
            },
            "cursor": {
                "type": "string",
                "description": "page.next_cursor from the previous reply, to continue after its last row. Requires limit; pass the same filters again.",
            },
            "view": {
                "type": "string",
                "enum": ["full", "summary"],
                "description": "full (default) returns whole occurrence objects; summary returns ids, phase, due time in UTC and in the trigger's timezone, attempts, the last receipt, the runtime outcome and the target's kind, without the trigger, the target action or machine state.",
            },
        },
        "required": ["schedule_id"],
        "additionalProperties": false,
    })
}

fn schedule_id_schema(description: &'static str) -> Value {
    json!({
        "type": "object",
        "properties": {
            "schedule_id": {
                "type": "string",
                "description": description,
            }
        },
        "required": ["schedule_id"],
        "additionalProperties": false,
    })
}

fn create_schedule_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "name": { "type": "string" },
            "description": { "type": "string" },
            "trigger": trigger_spec_schema(),
            "target": target_binding_schema(),
            "misfire_policy": misfire_policy_schema(),
            "overlap_policy": overlap_policy_schema(),
            "missing_target_policy": missing_target_policy_schema(),
            "labels": {
                "type": "object",
                "additionalProperties": { "type": "string" }
            },
            "planning_horizon_days": {
                "type": "integer",
                "minimum": 1
            },
            "planning_horizon_occurrences": {
                "type": "integer",
                "minimum": 1
            }
        },
        "required": ["trigger", "target", "misfire_policy", "overlap_policy", "missing_target_policy"],
        "additionalProperties": false,
    })
}

/// Update schema. Every replaceable field carries exactly the shape
/// `meerkat_schedule_create` advertises for it, built from the same schema
/// functions, so this tool stays self-contained. Tool visibility is decided
/// per tool name (`ToolScope` filters, `--allow-tool`/`--block-tool`, per-turn
/// allow/deny lists, member tool policies, deferred catalog loads), so the
/// update tool can be visible while the create tool is not; a pointer to the
/// create schema would then leave the model guessing the trigger and target
/// field names.
fn update_schedule_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "schedule_id": {
                "type": "string",
                "description": "The persisted schedule_id to update."
            },
            "name": { "type": "string" },
            "description": { "type": "string" },
            "trigger": trigger_spec_schema(),
            "target": target_binding_schema(),
            "misfire_policy": misfire_policy_schema(),
            "overlap_policy": overlap_policy_schema(),
            "missing_target_policy": missing_target_policy_schema(),
            "labels": {
                "type": "object",
                "additionalProperties": { "type": "string" }
            },
            "planning_horizon_days": {
                "type": "integer",
                "minimum": 1
            },
            "planning_horizon_occurrences": {
                "type": "integer",
                "minimum": 1
            }
        },
        "required": ["schedule_id"],
        "additionalProperties": false,
    })
}

fn date_time_schema(description: &'static str) -> Value {
    json!({
        "type": "string",
        "format": "date-time",
        "description": description,
    })
}

fn trigger_spec_schema() -> Value {
    json!({
        "description": "When the schedule fires. Uses internally tagged JSON with a \"type\" field. Three types: \"once\" fires at a single UTC timestamp. \"interval\" fires repeatedly every N seconds from a start time (optional end_at_utc). \"calendar\" fires on cron-style fields in a named IANA timezone. Examples: {\"type\":\"once\",\"due_at_utc\":\"2026-04-10T14:00:00Z\"} | {\"type\":\"interval\",\"start_at_utc\":\"2026-04-10T14:00:00Z\",\"every_seconds\":60} | {\"type\":\"calendar\",\"timezone\":\"UTC\",\"minute\":{\"kind\":\"values\",\"values\":[0]},\"hour\":{\"kind\":\"values\",\"values\":[9,17]}}.",
        "oneOf": [
            {
                "type": "object",
                "properties": {
                    "type": { "const": "once" },
                    "due_at_utc": date_time_schema("Deliver once at this UTC timestamp.")
                },
                "required": ["type", "due_at_utc"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "properties": {
                    "type": { "const": "interval" },
                    "start_at_utc": date_time_schema("First due time in UTC."),
                    "every_seconds": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Repeat cadence in seconds."
                    },
                    "end_at_utc": date_time_schema("Optional final due time in UTC.")
                },
                "required": ["type", "start_at_utc", "every_seconds"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "properties": {
                    "type": { "const": "calendar" },
                    "timezone": {
                        "type": "string",
                        "description": "IANA timezone such as Europe/Stockholm or UTC."
                    },
                    "minute": calendar_field_schema("Minute values."),
                    "hour": calendar_field_schema("Hour values, 0-23."),
                    "day_of_month": calendar_field_schema("Day-of-month values, 1-31."),
                    "month": calendar_field_schema("Month values, 1-12."),
                    "day_of_week": calendar_field_schema("Weekday values using the scheduler's cron-style domain: 0-6 and weekday names such as MON or FRI."),
                    "year": calendar_field_schema("Optional year filter.")
                },
                "required": ["type", "timezone"],
                "additionalProperties": false
            }
        ]
    })
}

fn calendar_field_schema(description: &'static str) -> Value {
    json!({
        "description": description,
        "oneOf": [
            {
                "type": "object",
                "properties": { "kind": { "const": "any" } },
                "required": ["kind"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "properties": {
                    "kind": { "const": "values" },
                    "values": {
                        "type": "array",
                        "items": { "type": "integer", "minimum": 0 }
                    }
                },
                "required": ["kind", "values"],
                "additionalProperties": false
            }
        ]
    })
}

fn target_binding_schema() -> Value {
    json!({
        "description": "Where the schedule delivers. Uses target_kind to select session, identity, mob, or host_runnable. Host-runnable targets invoke an already registered host runnable by its published name, with optional opaque JSON params. The host owns runnable availability and execution; this target does not create a runnable or grant permission. Session targets: exact_session (deliver to a known session_id; fails if session is gone), resumable_session (deliver to a session_id that may be idle; runtime resumes it -- best for stable long-lived sessions), materialize_on_demand_session (create a new session on first fire using a \"create\" spec, then reuse it -- use when no session exists yet). Identity targets: resumable_identity (deliver to the current materialized session for a stable agent identity; host resolves at fire time). The identity string must be a form a schedule host on this surface can resolve, such as the mob-member form minted by the host; free-form strings are refused at create. Mob-member identity targets accept only prompt actions without session-only overrides (no system_prompt, skill_refs, or additional_instructions). Mob targets: member, flow, spawn_helper, fork_helper (deliver to a mob member or flow). Examples: schedule for yourself {\"target_kind\":\"session\",\"type\":\"current_session\",\"action\":{\"type\":\"prompt\",\"prompt\":\"Check in\"}} | mob member {\"target_kind\":\"identity\",\"type\":\"resumable_identity\",\"identity\":\"mob_member:{\\\"schema\\\":\\\"meerkat.schedule.mob_member_identity.v2\\\",\\\"mob_id\\\":\\\"ops\\\",\\\"member\\\":\\\"watcher\\\"}\",\"action\":{\"type\":\"prompt\",\"prompt\":\"Check in\"}} | {\"target_kind\":\"session\",\"type\":\"materialize_on_demand_session\",\"action\":{\"type\":\"prompt\",\"prompt\":\"Run report\"},\"create\":{\"model\":\"claude-sonnet-4-6\"}}.",
        "oneOf": [
            {
                "type": "object",
                "properties": {
                    "target_kind": { "const": "session" },
                    "type": {
                        "enum": ["exact_session", "resumable_session", "materialize_on_demand_session"]
                    },
                    "session_id": {
                        "type": "string",
                        "description": "Required for exact_session and resumable_session."
                    },
                    "action": scheduled_session_action_schema(),
                    "create": session_materialization_spec_schema(),
                    "bound_session_id": {
                        "type": "string",
                        "description": "Only used by persisted materialize_on_demand_session schedules after the first materialization."
                    }
                },
                "required": ["target_kind", "type", "action"],
                "allOf": [
                    {
                        "if": { "properties": { "type": { "const": "materialize_on_demand_session" } } },
                        "then": { "required": ["create"] }
                    },
                    {
                        "if": {
                            "properties": {
                                "type": { "enum": ["exact_session", "resumable_session"] }
                            }
                        },
                        "then": { "required": ["session_id"] }
                    }
                ],
                "additionalProperties": false
            },
            {
                "type": "object",
                "properties": {
                    "target_kind": { "const": "identity" },
                    "type": { "const": "resumable_identity" },
                    "identity": {
                        "type": "string",
                        "description": "Stable agent identity resolved by the schedule host at occurrence delivery time. Must be a host-recognized form (for example mob_member:{...} as minted by the host); arbitrary strings are refused at create because no host could ever deliver them."
                    },
                    "action": scheduled_session_action_schema()
                },
                "required": ["target_kind", "type", "identity", "action"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "properties": {
                    "target_kind": { "const": "mob" },
                    "type": { "enum": ["member", "flow", "spawn_helper", "fork_helper"] },
                    "mob_id": { "type": "string" },
                    "member_id": { "type": "string" },
                    "source_member_id": { "type": "string" },
                    "flow_id": { "type": "string" },
                    "params": {
                        "description": "Raw JSON value for mob flow parameters."
                    },
                    "prompt": { "type": "string" },
                    "action": scheduled_mob_action_schema(),
                    "fork_context": fork_context_schema(),
                    "options": helper_options_schema()
                },
                "required": ["target_kind", "type", "mob_id"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "properties": {
                    "target_kind": { "const": "host_runnable" },
                    "runnable": {
                        "type": "string",
                        "minLength": 1,
                        "description": "Published name of a host-registered runnable; empty or whitespace-only names are invalid."
                    },
                    "params": {
                        "description": "Optional opaque JSON payload passed to the runnable. Omit when no payload is needed."
                    }
                },
                "required": ["target_kind", "runnable"],
                "additionalProperties": false
            }
        ]
    })
}

fn scheduled_session_action_schema() -> Value {
    json!({
        "oneOf": [
            {
                "type": "object",
                "properties": {
                    "type": { "const": "prompt" },
                    "prompt": content_input_schema(),
                    "system_prompt": {
                        "type": "string",
                        "description": "Appended as an ordinary System message at this scheduled turn boundary."
                    },
                    "render_metadata": { "type": "object" },
                    "skill_refs": {
                        "type": "array",
                        "items": { "type": "object" },
                        "description": "Structured skill references."
                    },
                    "additional_instructions": {
                        "type": "array",
                        "items": { "type": "string" }
                    }
                },
                "required": ["type", "prompt"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "properties": {
                    "type": { "const": "event" },
                    "event_type": { "type": "string" },
                    "payload": {},
                    "render_metadata": { "type": "object" }
                },
                "required": ["type", "event_type", "payload"],
                "additionalProperties": false
            }
        ]
    })
}

fn skill_key_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "source_uuid": { "type": "string" },
            "skill_name": { "type": "string" }
        },
        "required": ["source_uuid", "skill_name"],
        "additionalProperties": false
    })
}

fn session_materialization_spec_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "model": { "type": "string" },
            "system_prompt": { "type": "string" },
            "max_tokens": { "type": "integer", "minimum": 1 },
            "provider": { "enum": ["anthropic", "openai", "gemini", "other"] },
            "output_schema": { "type": "object" },
            "structured_output_retries": { "type": "integer", "minimum": 0 },
            "provider_params": { "type": "object" },
            "comms_name": { "type": "string" },
            "peer_meta": { "type": "object" },
            "labels": {
                "type": "object",
                "additionalProperties": { "type": "string" }
            },
            "preload_skills": {
                "type": "array",
                "items": skill_key_schema()
            },
            "additional_instructions": {
                "type": "array",
                "items": { "type": "string" }
            },
            "realm_id": { "type": "string" },
            "instance_id": { "type": "string" },
            "backend": { "type": "string" },
            "config_generation": { "type": "integer", "minimum": 0 },
            "keep_alive": { "type": "boolean" },
            "app_context": { "type": "object" }
        },
        "required": ["model"],
        "additionalProperties": false
    })
}

fn scheduled_mob_action_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "type": { "const": "send" },
            "content": content_input_schema(),
            "render_metadata": { "type": "object" }
        },
        "required": ["type", "content"],
        "additionalProperties": false
    })
}

fn helper_options_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "role_name": { "type": "string" },
            "runtime_mode": { "enum": ["autonomous_host", "turn_driven"] },
            "backend": { "enum": ["session", "external"] },
            "tool_access_policy": { "type": "object" }
        },
        "additionalProperties": false
    })
}

fn fork_context_schema() -> Value {
    json!({
        "oneOf": [
            {
                "type": "object",
                "properties": { "type": { "const": "full_history" } },
                "required": ["type"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "properties": {
                    "type": { "const": "last_messages" },
                    "count": { "type": "integer", "minimum": 1 }
                },
                "required": ["type", "count"],
                "additionalProperties": false
            }
        ]
    })
}

fn content_input_schema() -> Value {
    json!({
        "description": "Either a plain text string or a list of multimodal content blocks.",
        "oneOf": [
            { "type": "string" },
            {
                "type": "array",
                "items": {
                    "oneOf": [
                        {
                            "type": "object",
                            "properties": {
                                "type": { "const": "text" },
                                "text": { "type": "string" }
                            },
                            "required": ["type", "text"],
                            "additionalProperties": false
                        },
                        {
                            "type": "object",
                            "properties": {
                                "type": { "const": "image" },
                                "media_type": { "type": "string" },
                                "source": { "enum": ["inline", "blob"] },
                                "data": { "type": "string" },
                                "blob_id": { "type": "string" }
                            },
                            "required": ["type", "media_type", "source"],
                            "additionalProperties": true
                        },
                        {
                            "type": "object",
                            "properties": {
                                "type": { "const": "video" },
                                "media_type": { "type": "string" },
                                "duration_ms": { "type": "integer", "minimum": 0 },
                                "source": { "const": "inline" },
                                "data": { "type": "string" }
                            },
                            "required": ["type", "media_type", "duration_ms", "source", "data"],
                            "additionalProperties": false
                        }
                    ]
                }
            }
        ]
    })
}

fn misfire_policy_schema() -> Value {
    json!({
        "description": "What happens when the scheduler misses an occurrence's due time (e.g., downtime or lag). {\"type\":\"skip\"} (recommended): discard overdue occurrences after a ~30s grace window. {\"type\":\"catch_up_within\",\"window_seconds\":N}: deliver the occurrence if it is overdue by less than N seconds, otherwise misfire. Use skip unless you specifically need late delivery.",
        "oneOf": [
            {
                "type": "object",
                "properties": { "type": { "const": "skip" } },
                "required": ["type"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "properties": {
                    "type": { "const": "catch_up_within" },
                    "window_seconds": { "type": "integer", "minimum": 1 }
                },
                "required": ["type", "window_seconds"],
                "additionalProperties": false
            }
        ]
    })
}

fn overlap_policy_schema() -> Value {
    json!({
        "type": "string",
        "enum": ["allow_concurrent", "skip_if_running"],
        "description": "What happens when a new occurrence fires while a previous one is still running. \"skip_if_running\" (recommended): skip the new occurrence to prevent pile-up. \"allow_concurrent\": deliver both, allowing parallel execution. Use skip_if_running unless the target is designed for concurrent prompts."
    })
}

fn missing_target_policy_schema() -> Value {
    json!({
        "type": "string",
        "enum": ["skip", "mark_misfired"],
        "description": "What happens when the target session or mob does not exist at fire time. \"mark_misfired\" (recommended): record the occurrence as misfired so failures are visible. \"skip\": silently skip the occurrence. Use mark_misfired unless you expect transient target absence and do not want noise."
    })
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::{
        IdentityTargetBinding, IntervalTriggerSpec, MemoryScheduleStore, MisfirePolicy,
        MissingTargetPolicy, OverlapPolicy, ScheduledSessionAction, SessionTargetBinding,
        TargetBinding, TriggerSpec,
    };
    use chrono::{Duration, Utc};
    use meerkat_core::{AgentToolDispatcher, ToolError};
    use meerkat_core::{ContentInput, SessionId};
    use serde_json::value::RawValue;
    use std::collections::{BTreeMap, BTreeSet};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    #[test]
    fn tool_names_match_the_tool_definitions() {
        let defined: Vec<String> = schedule_tools_list()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(defined, schedule_tool_names());
    }

    struct IdentityCurrentSessionResolver;

    struct HybridScheduleDispatcher {
        catalog: Arc<[meerkat_core::ToolCatalogEntry]>,
    }

    struct LiveScheduleDispatcher {
        callable: AtomicBool,
        saw_context_image: AtomicBool,
        rewritten_target_type: Mutex<Option<String>>,
        tool: Arc<ToolDef>,
    }

    impl HybridScheduleDispatcher {
        fn new() -> Self {
            let detached = meerkat_core::DetachedToolExecutionPolicy::new(
                meerkat_core::RunnerIdentity::new("test.schedule", "v1")
                    .expect("runner declaration"),
                meerkat_core::RestartClass::NonResumable,
                meerkat_core::IdempotencyScope::ToolCall,
                std::time::Duration::from_secs(10),
            )
            .expect("detached policy");
            let contract = meerkat_core::ToolExecutionContract::new(
                BTreeSet::from([
                    meerkat_core::ToolExecutionMode::Fast,
                    meerkat_core::ToolExecutionMode::Detached,
                ]),
                meerkat_core::ToolExecutionMode::Fast,
                None,
                Some(detached),
            )
            .expect("hybrid execution contract");
            let tool = Arc::new(ToolDef {
                audience: Default::default(),
                name: "meerkat_schedule_create".into(),
                description: "Create a schedule.".to_string(),
                input_schema: create_schedule_schema(),
                provenance: Some(ToolProvenance {
                    kind: ToolSourceKind::Schedule,
                    source_id: "schedule".into(),
                    artifact: None,
                }),
            });
            Self {
                catalog: vec![
                    meerkat_core::ToolCatalogEntry::session_inline(tool, true)
                        .with_execution_contract(contract),
                ]
                .into(),
            }
        }
    }

    impl LiveScheduleDispatcher {
        fn new() -> Self {
            Self {
                callable: AtomicBool::new(true),
                saw_context_image: AtomicBool::new(false),
                rewritten_target_type: Mutex::new(None),
                tool: Arc::new(ToolDef {
                    audience: Default::default(),
                    name: "meerkat_schedule_create".into(),
                    description: "Create a schedule.".to_string(),
                    input_schema: create_schedule_schema(),
                    provenance: Some(ToolProvenance {
                        kind: ToolSourceKind::Schedule,
                        source_id: "schedule".into(),
                        artifact: None,
                    }),
                }),
            }
        }

        fn entry(&self) -> meerkat_core::ToolCatalogEntry {
            meerkat_core::ToolCatalogEntry::session_inline_with_callability(
                Arc::clone(&self.tool),
                if self.callable.load(Ordering::SeqCst) {
                    meerkat_core::ToolCallability::callable()
                } else {
                    meerkat_core::ToolCallability::unavailable(
                        meerkat_core::ToolUnavailableReason::TemporarilyUnavailable,
                    )
                },
            )
        }
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl AgentToolDispatcher for HybridScheduleDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            self.catalog
                .iter()
                .map(|entry| Arc::clone(&entry.tool))
                .collect::<Vec<_>>()
                .into()
        }

        fn tool_catalog_capabilities(&self) -> meerkat_core::ToolCatalogCapabilities {
            meerkat_core::ToolCatalogCapabilities {
                exact_catalog: true,
                may_require_catalog_control_plane: false,
            }
        }

        fn tool_catalog(&self) -> Arc<[meerkat_core::ToolCatalogEntry]> {
            Arc::clone(&self.catalog)
        }

        fn resolve_execution_plan(
            &self,
            call: ToolCallView<'_>,
            _dispatch_context: &meerkat_core::ToolDispatchContext,
            resolution_context: &meerkat_core::ToolExecutionResolutionContext,
        ) -> Result<
            meerkat_core::ResolvedToolExecutionPlan,
            meerkat_core::ToolExecutionResolutionError,
        > {
            let args: Value = serde_json::from_str(call.args.get()).map_err(|error| {
                meerkat_core::ToolExecutionResolutionError::InvalidArguments {
                    tool_name: call.name.to_string(),
                    reason: error.to_string(),
                }
            })?;
            let mode = if args["target"]["type"] == "resumable_session" {
                meerkat_core::ToolExecutionMode::Detached
            } else {
                meerkat_core::ToolExecutionMode::Fast
            };
            self.catalog[0]
                .execution
                .resolve(mode, resolution_context.deadlines().clone())
                .map_err(Into::into)
        }

        async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
            Err(ToolError::not_found(call.name))
        }

        async fn dispatch_resolved_with_context(
            &self,
            call: ToolCallView<'_>,
            _context: &meerkat_core::ToolDispatchContext,
            plan: &meerkat_core::ResolvedToolExecutionPlan,
        ) -> Result<ToolDispatchOutcome, ToolError> {
            let args: Value = serde_json::from_str(call.args.get())
                .map_err(|error| ToolError::invalid_arguments(call.name, error.to_string()))?;
            if plan.mode() != meerkat_core::ToolExecutionMode::Detached
                || args["target"]["type"] != "resumable_session"
            {
                return Err(ToolError::execution_failed(
                    "rewritten detached schedule plan was not preserved",
                ));
            }
            Ok(ToolResult::new(call.id.to_string(), "ok".to_string(), false).into())
        }
    }

    #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
    impl AgentToolDispatcher for LiveScheduleDispatcher {
        fn tools(&self) -> Arc<[Arc<ToolDef>]> {
            if self.callable.load(Ordering::SeqCst) {
                vec![Arc::clone(&self.tool)].into()
            } else {
                Arc::from([])
            }
        }

        fn tool_catalog_capabilities(&self) -> meerkat_core::ToolCatalogCapabilities {
            meerkat_core::ToolCatalogCapabilities {
                exact_catalog: true,
                may_require_catalog_control_plane: true,
            }
        }

        fn tool_catalog(&self) -> Arc<[meerkat_core::ToolCatalogEntry]> {
            vec![self.entry()].into()
        }

        async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
            self.dispatch_with_context(call, &meerkat_core::ToolDispatchContext::default())
                .await
        }

        async fn dispatch_with_context(
            &self,
            call: ToolCallView<'_>,
            context: &meerkat_core::ToolDispatchContext,
        ) -> Result<ToolDispatchOutcome, ToolError> {
            let arguments: Value = serde_json::from_str(call.args.get())
                .map_err(|error| ToolError::invalid_arguments(call.name, error.to_string()))?;
            *self
                .rewritten_target_type
                .lock()
                .expect("rewritten target recorder lock") =
                arguments["target"]["type"].as_str().map(str::to_string);
            self.saw_context_image.store(
                context
                    .current_turn()
                    .and_then(|turn| turn.image_ref(0))
                    .and_then(|image_ref| context.current_turn_image(image_ref))
                    .is_some(),
                Ordering::SeqCst,
            );
            Ok(ToolResult::new(call.id.to_string(), "{}".to_string(), false).into())
        }
    }

    impl CurrentSessionScheduleTargetResolver for IdentityCurrentSessionResolver {
        fn resolve_current_session_target(
            &self,
            _current_session_id: &SessionId,
            action: ScheduledSessionAction,
        ) -> TargetBinding {
            TargetBinding::identity(IdentityTargetBinding::resumable("domain:security", action))
        }
    }

    fn schedule_request() -> CreateScheduleRequest {
        CreateScheduleRequest {
            name: Some("heartbeat".into()),
            description: Some("tool surface schedule".into()),
            trigger: TriggerSpec::Interval(IntervalTriggerSpec {
                start_at_utc: Utc::now() + Duration::minutes(1),
                every_seconds: 60,
                end_at_utc: None,
            }),
            target: TargetBinding::session(SessionTargetBinding::ExactSession {
                session_id: SessionId::new(),
                action: ScheduledSessionAction::Prompt {
                    prompt: ContentInput::from("tool surface"),
                    system_prompt: None,
                    render_metadata: None,
                    skill_refs: Vec::new(),
                    additional_instructions: Vec::new(),
                },
            }),
            misfire_policy: MisfirePolicy::Skip,
            overlap_policy: OverlapPolicy::SkipIfRunning,
            missing_target_policy: MissingTargetPolicy::MarkMisfired,
            labels: BTreeMap::new(),
            planning_horizon_days: Some(1),
            planning_horizon_occurrences: Some(2),
        }
    }

    #[tokio::test]
    async fn schedule_tools_create_and_list_round_trip() -> Result<(), String> {
        let service = ScheduleService::new(Arc::new(MemoryScheduleStore::default()));
        let request =
            serde_json::to_value(schedule_request()).map_err(|error| error.to_string())?;
        let created = handle_schedule_tools_call(&service, "meerkat_schedule_create", &request)
            .await
            .map_err(|error| format!("{error:?}"))?;
        let schedule_id = created["schedule_id"]
            .as_str()
            .ok_or_else(|| "create should return schedule_id".to_string())?;

        let listed = handle_schedule_tools_call(&service, "meerkat_schedule_list", &json!({}))
            .await
            .map_err(|error| format!("{error:?}"))?;
        assert_eq!(
            listed["schedules"][0]["schedule_id"].as_str(),
            Some(schedule_id)
        );

        let occurrences = handle_schedule_tools_call(
            &service,
            "meerkat_schedule_occurrences",
            &json!({ "schedule_id": schedule_id }),
        )
        .await
        .map_err(|error| format!("{error:?}"))?;
        assert!(
            occurrences["occurrences"]
                .as_array()
                .map(|rows| !rows.is_empty())
                .unwrap_or(false),
            "planning should persist occurrences"
        );
        Ok(())
    }

    fn tool_call<'a>(
        id: &'a str,
        name: &'a str,
        args: &'a RawValue,
    ) -> meerkat_core::ToolCallView<'a> {
        meerkat_core::ToolCallView { id, name, args }
    }

    #[tokio::test]
    async fn schedule_tool_dispatcher_tools_match_tool_list() {
        let service = ScheduleService::new(Arc::new(MemoryScheduleStore::default()));
        let dispatcher = ScheduleToolDispatcher::new(service);

        let actual: Vec<String> = dispatcher
            .tools()
            .iter()
            .map(|tool| tool.name.to_string())
            .collect();
        let expected: Vec<String> = schedule_tools_list()
            .into_iter()
            .map(|value| value["name"].as_str().expect("tool name").to_string())
            .collect();

        assert_eq!(actual, expected);
    }

    #[test]
    fn schedule_trigger_schema_matches_internal_tagged_deserializer_shape() {
        let tools = schedule_tools_list();
        let create_schema = &tools
            .iter()
            .find(|tool| tool["name"] == "meerkat_schedule_create")
            .expect("create tool schema must exist")["inputSchema"];
        let trigger_schema = &create_schema["properties"]["trigger"];
        let variants = trigger_schema["oneOf"]
            .as_array()
            .expect("trigger schema variants must be an array");

        assert_eq!(
            trigger_schema["description"].as_str(),
            Some(
                "When the schedule fires. Uses internally tagged JSON with a \"type\" field. Three types: \"once\" fires at a single UTC timestamp. \"interval\" fires repeatedly every N seconds from a start time (optional end_at_utc). \"calendar\" fires on cron-style fields in a named IANA timezone. Examples: {\"type\":\"once\",\"due_at_utc\":\"2026-04-10T14:00:00Z\"} | {\"type\":\"interval\",\"start_at_utc\":\"2026-04-10T14:00:00Z\",\"every_seconds\":60} | {\"type\":\"calendar\",\"timezone\":\"UTC\",\"minute\":{\"kind\":\"values\",\"values\":[0]},\"hour\":{\"kind\":\"values\",\"values\":[9,17]}}."
            )
        );
        assert_eq!(variants[0]["properties"]["type"]["const"], json!("once"));
        assert_eq!(
            variants[1]["properties"]["type"]["const"],
            json!("interval")
        );
        assert_eq!(
            variants[2]["properties"]["type"]["const"],
            json!("calendar")
        );
    }

    #[test]
    fn host_runnable_target_is_advertised_by_create_update_and_session_wrapper() {
        let service = ScheduleService::new(Arc::new(MemoryScheduleStore::default()));
        let native = Arc::new(ScheduleToolDispatcher::new(service));
        let scoped = CurrentSessionScheduleToolDispatcher::new(native.clone(), SessionId::new());
        for tools in [native.tools(), scoped.tools()] {
            for name in ["meerkat_schedule_create", "meerkat_schedule_update"] {
                let tool = tools
                    .iter()
                    .find(|tool| tool.name == name)
                    .expect("schedule tool");
                let variants = tool.input_schema["properties"]["target"]["oneOf"]
                    .as_array()
                    .expect("target variants");
                let host = variants
                    .iter()
                    .find(|variant| {
                        variant["properties"]["target_kind"]["const"] == "host_runnable"
                    })
                    .expect("existing host target must be discoverable without another tool");
                assert_eq!(host["required"], json!(["target_kind", "runnable"]));
                assert_eq!(host["properties"]["runnable"]["type"], "string");
                assert!(host["properties"].get("params").is_some());
                assert!(
                    host["properties"]["params"].get("type").is_none(),
                    "host params must not be narrowed to object values"
                );
                assert_eq!(host["additionalProperties"], false);
                for existing in ["session", "identity", "mob"] {
                    assert!(variants.iter().any(|variant| {
                        variant["properties"]["target_kind"]["const"] == existing
                    }));
                }
            }
        }
    }

    #[tokio::test]
    async fn host_runnable_target_round_trips_through_create_update_dispatch() -> Result<(), String>
    {
        let service = ScheduleService::new(Arc::new(MemoryScheduleStore::default()));
        let dispatcher = ScheduleToolDispatcher::new(service.clone());
        for params in [
            None,
            Some(json!({"nested": [1, true]})),
            Some(json!(["a", 3])),
            Some(json!(7)),
        ] {
            let mut target =
                json!({"target_kind": "host_runnable", "runnable": "published-report"});
            if let Some(params) = params {
                target["params"] = params;
            }
            let mut request =
                serde_json::to_value(schedule_request()).map_err(|e| e.to_string())?;
            request["target"] = target.clone();
            let raw = RawValue::from_string(request.to_string()).map_err(|e| e.to_string())?;
            let created = dispatcher
                .dispatch(tool_call(
                    "host-create",
                    "meerkat_schedule_create",
                    raw.as_ref(),
                ))
                .await
                .map_err(|e| format!("{e:?}"))?;
            let created: Value =
                serde_json::from_str(&created.result.text_content()).map_err(|e| e.to_string())?;
            assert_eq!(created["target"], target);
            let schedule_id = created["schedule_id"]
                .as_str()
                .ok_or("missing schedule id")?;
            target["runnable"] = json!("published-updated-report");
            let raw = RawValue::from_string(
                json!({"schedule_id": schedule_id, "target": target}).to_string(),
            )
            .map_err(|e| e.to_string())?;
            let updated = dispatcher
                .dispatch(tool_call(
                    "host-update",
                    "meerkat_schedule_update",
                    raw.as_ref(),
                ))
                .await
                .map_err(|e| format!("{e:?}"))?;
            let updated: Value =
                serde_json::from_str(&updated.result.text_content()).map_err(|e| e.to_string())?;
            assert_eq!(updated["target"], target);
            let stored = handle_schedule_tools_call(
                &service,
                "meerkat_schedule_get",
                &json!({"schedule_id": schedule_id}),
            )
            .await
            .map_err(|e| format!("{e:?}"))?;
            assert_eq!(stored["target"], target);
        }
        Ok(())
    }

    #[tokio::test]
    async fn host_runnable_target_still_refuses_missing_or_blank_names() {
        let service = ScheduleService::new(Arc::new(MemoryScheduleStore::default()));
        for target in [
            json!({"target_kind": "host_runnable"}),
            json!({"target_kind": "host_runnable", "runnable": ""}),
            json!({"target_kind": "host_runnable", "runnable": "   "}),
        ] {
            let mut request = serde_json::to_value(schedule_request()).expect("valid request");
            request["target"] = target;
            assert!(
                handle_schedule_tools_call(&service, "meerkat_schedule_create", &request)
                    .await
                    .is_err()
            );
        }
        let listed = handle_schedule_tools_call(&service, "meerkat_schedule_list", &json!({}))
            .await
            .expect("list");
        assert_eq!(listed["schedules"], json!([]));
    }

    /// Tool visibility is decided per tool name, so `meerkat_schedule_update`
    /// can be visible while `meerkat_schedule_create` is filtered out (for
    /// example `--block-tool meerkat_schedule_create`, a per-turn allow list,
    /// or a member tool policy that allows edits but not creation). Update
    /// must then still carry every replaceable shape itself: each field it
    /// shares with create is the identical schema, no field defers its shape
    /// to the create tool, and the session-scoped wrapper advertises the
    /// `current_session` shortcut on update's target exactly as on create's.
    #[tokio::test]
    async fn update_schema_is_self_contained_when_create_is_not_visible() {
        let tools = schedule_tools_list();
        let schema_of = |name: &str| {
            tools
                .iter()
                .find(|tool| tool["name"] == name)
                .expect("schedule tool schema must exist")["inputSchema"]
                .clone()
        };
        let create = schema_of("meerkat_schedule_create");
        let update = schema_of("meerkat_schedule_update");
        let create_properties = create["properties"]
            .as_object()
            .expect("create schema properties");
        let update_properties = update["properties"]
            .as_object()
            .expect("update schema properties");

        for (field, create_shape) in create_properties {
            assert_eq!(
                update_properties.get(field),
                Some(create_shape),
                "update.{field} must carry the create shape itself, not point at it"
            );
        }
        assert_eq!(update["required"], json!(["schedule_id"]));
        assert!(
            !update.to_string().contains("meerkat_schedule_create"),
            "no update field may defer its shape to the create tool"
        );

        // The replacement from the review scenario (repeat every 15 minutes)
        // resolves from the update schema alone.
        let interval = update["properties"]["trigger"]["oneOf"]
            .as_array()
            .expect("update trigger variants")
            .iter()
            .find(|variant| variant["properties"]["type"]["const"] == "interval")
            .expect("update trigger advertises the interval variant");
        assert_eq!(
            interval["required"],
            json!(["type", "start_at_utc", "every_seconds"])
        );

        let service = ScheduleService::new(Arc::new(MemoryScheduleStore::default()));
        let dispatcher = CurrentSessionScheduleToolDispatcher::new(
            Arc::new(ScheduleToolDispatcher::new(service)),
            SessionId::new(),
        );
        let session_tools = dispatcher.tools();
        let session_target_types = |name: &str| {
            session_tools
                .iter()
                .find(|tool| tool.name == name)
                .expect("session-scoped schedule tool")
                .input_schema["properties"]["target"]["oneOf"][0]["properties"]["type"]["enum"]
                .clone()
        };
        let update_target_types = session_target_types("meerkat_schedule_update");
        assert_eq!(
            update_target_types,
            session_target_types("meerkat_schedule_create")
        );
        assert!(
            update_target_types
                .as_array()
                .expect("update target type enum")
                .iter()
                .any(|value| value.as_str() == Some("current_session")),
            "session-scoped wrapper should advertise current_session on update"
        );
    }

    #[tokio::test]
    async fn schedule_tool_dispatcher_delegates_to_schedule_handler() -> Result<(), String> {
        let service = ScheduleService::new(Arc::new(MemoryScheduleStore::default()));
        let dispatcher = ScheduleToolDispatcher::new(service.clone());
        let args = serde_json::to_string(&schedule_request()).map_err(|error| error.to_string())?;
        let raw = RawValue::from_string(args).map_err(|error| error.to_string())?;
        let call = tool_call("sched-1", "meerkat_schedule_create", raw.as_ref());

        let outcome = dispatcher
            .dispatch(call)
            .await
            .map_err(|error| format!("{error:?}"))?;
        let created_value: Value = serde_json::from_str(&outcome.result.text_content())
            .map_err(|error| error.to_string())?;
        assert_eq!(created_value["name"].as_str(), Some("heartbeat"));
        assert!(created_value["schedule_id"].as_str().is_some());

        let listed = handle_schedule_tools_call(&service, "meerkat_schedule_list", &json!({}))
            .await
            .map_err(|error| format!("{error:?}"))?;
        assert_eq!(listed["schedules"].as_array().map(Vec::len), Some(1));
        Ok(())
    }

    #[tokio::test]
    async fn current_session_schedule_dispatcher_rewrites_create_target() -> Result<(), String> {
        let service = ScheduleService::new(Arc::new(MemoryScheduleStore::default()));
        let current_session_id = SessionId::new();
        let dispatcher = CurrentSessionScheduleToolDispatcher::new(
            Arc::new(ScheduleToolDispatcher::new(service.clone())),
            current_session_id.clone(),
        );
        let tools = dispatcher.tools();
        let create_tool = tools
            .iter()
            .find(|tool| tool.name == "meerkat_schedule_create")
            .expect("create tool");
        let target_types = &create_tool.input_schema["properties"]["target"]["oneOf"][0]["properties"]
            ["type"]["enum"];
        assert!(
            target_types
                .as_array()
                .expect("target type enum")
                .iter()
                .any(|value| value.as_str() == Some("current_session")),
            "session-scoped wrapper should advertise current_session"
        );

        let request = json!({
            "name": "self-followup",
            "trigger": {
                "type": "interval",
                "start_at_utc": (Utc::now() + Duration::minutes(1)).to_rfc3339(),
                "every_seconds": 60
            },
            "target": {
                "target_kind": "session",
                "type": "current_session",
                "action": {
                    "type": "prompt",
                    "prompt": "check this session"
                }
            },
            "misfire_policy": { "type": "skip" },
            "overlap_policy": "skip_if_running",
            "missing_target_policy": "mark_misfired",
            "planning_horizon_occurrences": 1
        });
        let raw = RawValue::from_string(request.to_string()).map_err(|error| error.to_string())?;
        let outcome = dispatcher
            .dispatch(tool_call(
                "sched-current-create",
                "meerkat_schedule_create",
                raw.as_ref(),
            ))
            .await
            .map_err(|error| format!("{error:?}"))?;

        let created: Value = serde_json::from_str(&outcome.result.text_content())
            .map_err(|error| error.to_string())?;
        assert_eq!(
            created["target"]["type"].as_str(),
            Some("resumable_session")
        );
        assert_eq!(
            created["target"]["session_id"].as_str(),
            Some(current_session_id.to_string().as_str())
        );

        let listed = handle_schedule_tools_call(&service, "meerkat_schedule_list", &json!({}))
            .await
            .map_err(|error| format!("{error:?}"))?;
        assert_eq!(
            listed["schedules"][0]["target"]["type"].as_str(),
            Some("resumable_session")
        );
        Ok(())
    }

    #[tokio::test]
    async fn current_session_schedule_dispatcher_rewrites_update_target() -> Result<(), String> {
        let service = ScheduleService::new(Arc::new(MemoryScheduleStore::default()));
        let current_session_id = SessionId::new();
        let dispatcher = CurrentSessionScheduleToolDispatcher::new(
            Arc::new(ScheduleToolDispatcher::new(service)),
            current_session_id.clone(),
        );

        let create_raw = RawValue::from_string(
            serde_json::to_string(&schedule_request()).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
        let created = dispatcher
            .dispatch(tool_call(
                "sched-current-update-create",
                "meerkat_schedule_create",
                create_raw.as_ref(),
            ))
            .await
            .map_err(|error| format!("{error:?}"))?;
        let created: Value = serde_json::from_str(&created.result.text_content())
            .map_err(|error| error.to_string())?;
        let schedule_id = created["schedule_id"]
            .as_str()
            .ok_or_else(|| "missing schedule_id".to_string())?;

        let update = json!({
            "schedule_id": schedule_id,
            "target": {
                "target_kind": "session",
                "type": "current_session",
                "action": {
                    "type": "prompt",
                    "prompt": "updated self followup"
                }
            }
        });
        let update_raw =
            RawValue::from_string(update.to_string()).map_err(|error| error.to_string())?;
        let updated = dispatcher
            .dispatch(tool_call(
                "sched-current-update",
                "meerkat_schedule_update",
                update_raw.as_ref(),
            ))
            .await
            .map_err(|error| format!("{error:?}"))?;
        let updated: Value = serde_json::from_str(&updated.result.text_content())
            .map_err(|error| error.to_string())?;

        assert_eq!(
            updated["target"]["type"].as_str(),
            Some("resumable_session")
        );
        assert_eq!(
            updated["target"]["session_id"].as_str(),
            Some(current_session_id.to_string().as_str())
        );
        Ok(())
    }

    #[tokio::test]
    async fn current_session_schedule_dispatcher_uses_host_resolver_target() -> Result<(), String> {
        let service = ScheduleService::new(Arc::new(MemoryScheduleStore::default()));
        let current_session_id = SessionId::new();
        let dispatcher = CurrentSessionScheduleToolDispatcher::new_with_resolver(
            Arc::new(ScheduleToolDispatcher::new(service.clone())),
            current_session_id,
            Arc::new(IdentityCurrentSessionResolver),
        );

        let request = json!({
            "name": "identity-followup",
            "trigger": {
                "type": "interval",
                "start_at_utc": (Utc::now() + Duration::minutes(1)).to_rfc3339(),
                "every_seconds": 60
            },
            "target": {
                "target_kind": "session",
                "type": "current_session",
                "action": {
                    "type": "prompt",
                    "prompt": "check this identity"
                }
            },
            "misfire_policy": { "type": "skip" },
            "overlap_policy": "skip_if_running",
            "missing_target_policy": "mark_misfired",
            "planning_horizon_occurrences": 1
        });
        let raw = RawValue::from_string(request.to_string()).map_err(|error| error.to_string())?;
        let outcome = dispatcher
            .dispatch(tool_call(
                "sched-current-identity-create",
                "meerkat_schedule_create",
                raw.as_ref(),
            ))
            .await
            .map_err(|error| format!("{error:?}"))?;

        let created: Value = serde_json::from_str(&outcome.result.text_content())
            .map_err(|error| error.to_string())?;
        assert_eq!(created["target"]["target_kind"].as_str(), Some("identity"));
        assert_eq!(
            created["target"]["type"].as_str(),
            Some("resumable_identity")
        );
        assert_eq!(
            created["target"]["identity"].as_str(),
            Some("domain:security")
        );

        let listed = handle_schedule_tools_call(&service, "meerkat_schedule_list", &json!({}))
            .await
            .map_err(|error| format!("{error:?}"))?;
        assert_eq!(
            listed["schedules"][0]["target"]["target_kind"].as_str(),
            Some("identity")
        );
        Ok(())
    }

    #[tokio::test]
    async fn current_session_schedule_dispatcher_preserves_contract_and_forwards_rewritten_hybrid_call()
     {
        let dispatcher = CurrentSessionScheduleToolDispatcher::new(
            Arc::new(HybridScheduleDispatcher::new()),
            SessionId::new(),
        );
        let request = json!({
            "target": {
                "target_kind": "session",
                "type": "current_session",
                "action": {
                    "type": "prompt",
                    "prompt": "continue"
                }
            }
        });
        let raw = RawValue::from_string(request.to_string()).expect("raw schedule arguments");
        let resolution = meerkat_core::ToolExecutionResolutionContext::new(
            meerkat_core::ToolDeadlineChain::new(vec![
                meerkat_core::ToolDeadlineContributor::finite(
                    meerkat_core::ToolDeadlineOwner::CoreToolDispatch,
                    std::time::Duration::from_secs(600),
                ),
            ])
            .expect("deadline chain"),
        );

        let plan = dispatcher
            .resolve_execution_plan(
                tool_call("schedule-hybrid", "meerkat_schedule_create", raw.as_ref()),
                &meerkat_core::ToolDispatchContext::default(),
                &resolution,
            )
            .expect("rewritten hybrid call should resolve through the inner owner");

        assert_eq!(plan.mode(), meerkat_core::ToolExecutionMode::Detached);
        let catalog = dispatcher.tool_catalog();
        assert!(
            dispatcher.tool_catalog_capabilities().exact_catalog,
            "the wrapper must preserve exact catalog authority"
        );
        assert_eq!(
            catalog[0].execution.default_mode(),
            meerkat_core::ToolExecutionMode::Fast,
            "rewriting the provider definition must not replace internal execution metadata"
        );
        assert!(
            catalog[0]
                .execution
                .supported_modes()
                .contains(&meerkat_core::ToolExecutionMode::Detached)
        );
        let provider_def = serde_json::to_value(catalog[0].tool.as_ref())
            .expect("provider-facing tool definition serializes");
        assert!(
            provider_def.get("execution").is_none(),
            "internal execution metadata must not leak into ToolDef serialization"
        );
        dispatcher
            .dispatch_resolved_with_context(
                tool_call("schedule-hybrid", "meerkat_schedule_create", raw.as_ref()),
                &meerkat_core::ToolDispatchContext::default(),
                &plan,
            )
            .await
            .expect("resolved dispatch must apply the identical target rewrite");
    }

    #[test]
    fn current_session_schedule_tools_follow_live_exact_catalog_callability() {
        let inner = Arc::new(LiveScheduleDispatcher::new());
        let inner_dispatcher: Arc<dyn AgentToolDispatcher> = inner.clone();
        let dispatcher =
            CurrentSessionScheduleToolDispatcher::new(inner_dispatcher, SessionId::new());

        assert_eq!(
            dispatcher
                .tools()
                .iter()
                .filter(|tool| tool.name == "meerkat_schedule_create")
                .count(),
            1,
            "the live callable exact-catalog entry should be provider-visible"
        );

        inner.callable.store(false, Ordering::SeqCst);

        assert!(
            dispatcher
                .tools()
                .iter()
                .all(|tool| tool.name != "meerkat_schedule_create"),
            "tools() must not retain the construction-time callable snapshot"
        );
        let catalog = dispatcher.tool_catalog();
        let entry = catalog
            .iter()
            .find(|entry| entry.tool.name == "meerkat_schedule_create")
            .expect("exact catalog keeps the temporarily unavailable identity");
        assert_eq!(
            entry.callability.unavailable_reason(),
            Some(meerkat_core::ToolUnavailableReason::TemporarilyUnavailable)
        );
    }

    #[tokio::test]
    async fn current_session_schedule_dispatch_preserves_context_after_argument_rewrite() {
        let inner = Arc::new(LiveScheduleDispatcher::new());
        let inner_dispatcher: Arc<dyn AgentToolDispatcher> = inner.clone();
        let dispatcher =
            CurrentSessionScheduleToolDispatcher::new(inner_dispatcher, SessionId::new());
        let request = json!({
            "target": {
                "target_kind": "session",
                "type": "current_session",
                "action": {
                    "type": "prompt",
                    "prompt": "continue"
                }
            }
        });
        let raw = RawValue::from_string(request.to_string()).expect("raw schedule arguments");
        let context = meerkat_core::ToolDispatchContext::from_current_turn_input(
            &ContentInput::Blocks(vec![meerkat_core::ContentBlock::Image {
                media_type: "image/png".into(),
                data: "abc".into(),
            }]),
        );

        dispatcher
            .dispatch_with_context(
                tool_call("schedule-context", "meerkat_schedule_create", raw.as_ref()),
                &context,
            )
            .await
            .expect("rewritten schedule call should dispatch");

        assert!(
            inner.saw_context_image.load(Ordering::SeqCst),
            "the wrapper must forward the caller's original dispatch context"
        );
        assert_eq!(
            inner
                .rewritten_target_type
                .lock()
                .expect("rewritten target recorder lock")
                .as_deref(),
            Some("resumable_session"),
            "the same forwarded call must carry the rewritten durable target"
        );
    }

    #[tokio::test]
    async fn schedule_tool_dispatcher_unknown_tool_is_not_found() {
        let service = ScheduleService::new(Arc::new(MemoryScheduleStore::default()));
        let dispatcher = ScheduleToolDispatcher::new(service);
        let raw = RawValue::from_string("{}".to_string()).expect("raw args");
        let err = dispatcher
            .dispatch(tool_call("sched-2", "unknown_schedule_tool", raw.as_ref()))
            .await
            .expect_err("unknown tool should fail");

        assert!(matches!(err, ToolError::NotFound { .. }));
    }

    #[tokio::test]
    async fn schedule_tool_dispatcher_maps_unsupported_backend_to_execution_failed() {
        let service = ScheduleService::new(Arc::new(crate::DisabledScheduleStore));
        let dispatcher = ScheduleToolDispatcher::new(service);
        let raw = RawValue::from_string(
            serde_json::to_string(&schedule_request()).expect("schedule request json"),
        )
        .expect("raw args");
        let err = dispatcher
            .dispatch(tool_call(
                "sched-3",
                "meerkat_schedule_create",
                raw.as_ref(),
            ))
            .await
            .expect_err("unsupported backend should fail");

        assert!(matches!(err, ToolError::ExecutionFailed { .. }));
    }

    /// Once every attached identity probe is gone, an identity create reaches
    /// agents as a temporarily unavailable tool through both schedule tool
    /// dispatchers, and the wire as `CAPABILITY_UNAVAILABLE`.
    #[tokio::test]
    async fn schedule_tools_report_a_stopped_identity_judge_as_unavailable() {
        struct VouchingProbe;

        #[async_trait]
        impl crate::ScheduleIdentityTargetProbe for VouchingProbe {
            async fn probe_identity_target(
                &self,
                _binding: &IdentityTargetBinding,
            ) -> Result<crate::IdentityTargetDeliverability, ScheduleDomainError> {
                Ok(crate::IdentityTargetDeliverability::Deliverable)
            }
        }

        let service = ScheduleService::new(Arc::new(MemoryScheduleStore::default()));
        let probe: Arc<dyn crate::ScheduleIdentityTargetProbe> = Arc::new(VouchingProbe);
        service.attach_identity_target_probe(&probe);
        drop(probe);
        let mut request = schedule_request();
        request.target = TargetBinding::identity(IdentityTargetBinding::resumable(
            "domain:ops",
            ScheduledSessionAction::Prompt {
                prompt: ContentInput::from("tool surface"),
                system_prompt: None,
                render_metadata: None,
                skill_refs: Vec::new(),
                additional_instructions: Vec::new(),
            },
        ));
        let arguments = serde_json::to_value(&request).expect("schedule request json");
        let raw = RawValue::from_string(arguments.to_string()).expect("raw args");

        let dispatched = ScheduleToolDispatcher::new(service.clone())
            .dispatch(tool_call(
                "sched-4",
                "meerkat_schedule_create",
                raw.as_ref(),
            ))
            .await
            .expect_err("no live probe judges the identity target");
        assert!(
            matches!(
                dispatched,
                ToolError::Unavailable {
                    reason: ToolUnavailableReason::TemporarilyUnavailable,
                    ..
                }
            ),
            "expected a temporarily unavailable tool, found {dispatched:?}"
        );
        let surfaced = crate::ScheduleToolSurface::new(service.clone())
            .dispatch(tool_call(
                "sched-5",
                "meerkat_schedule_create",
                raw.as_ref(),
            ))
            .await
            .expect_err("no live probe judges the identity target");
        assert!(
            matches!(surfaced, ToolError::Unavailable { .. }),
            "expected an unavailable tool, found {surfaced:?}"
        );
        let wire = handle_schedule_tools_call(&service, "meerkat_schedule_create", &arguments)
            .await
            .expect_err("no live probe judges the identity target");
        assert_eq!(wire.code, CAPABILITY_UNAVAILABLE);
    }

    #[test]
    #[allow(clippy::panic)]
    fn schedule_tools_have_schedule_provenance() {
        let service = ScheduleService::new(Arc::new(crate::DisabledScheduleStore));
        let dispatcher = ScheduleToolDispatcher::new(service);
        let tools = dispatcher.tools();
        assert!(
            !tools.is_empty(),
            "schedule should expose at least one tool"
        );
        for tool in tools.iter() {
            let prov = tool
                .provenance
                .as_ref()
                .unwrap_or_else(|| panic!("schedule tool '{}' is missing provenance", tool.name));
            assert_eq!(
                prov.kind,
                meerkat_core::types::ToolSourceKind::Schedule,
                "schedule tool '{}' should have Schedule provenance",
                tool.name
            );
            assert_eq!(prov.source_id, "schedule");
        }
    }

    fn allow_list_policy(names: &[&str]) -> meerkat_core::ops::ToolAccessPolicy {
        meerkat_core::ops::ToolAccessPolicy::AllowList(
            names
                .iter()
                .copied()
                .collect::<meerkat_core::types::ToolNameSet>(),
        )
    }

    fn spawn_helper_create_args(policy_json: Option<Value>) -> Value {
        let mut options = serde_json::Map::new();
        if let Some(policy) = policy_json {
            options.insert("tool_access_policy".to_string(), policy);
        }
        json!({
            "name": "gated-helper",
            "trigger": {
                "type": "interval",
                "start_at_utc": (Utc::now() + Duration::minutes(1)).to_rfc3339(),
                "every_seconds": 60
            },
            "target": {
                "target_kind": "mob",
                "type": "spawn_helper",
                "mob_id": "ops",
                "member_id": "helper-1",
                "prompt": "do scheduled work",
                "options": Value::Object(options)
            },
            "misfire_policy": { "type": "skip" },
            "overlap_policy": "skip_if_running",
            "missing_target_policy": "mark_misfired",
            "planning_horizon_occurrences": 1
        })
    }

    #[test]
    fn creator_policy_passthrough_when_ungated() {
        let args = spawn_helper_create_args(None);
        let out = enforce_creator_tool_access_policy(args.clone(), None, "meerkat_schedule_create")
            .expect("ungated creator passes through");
        assert_eq!(out, args, "ungated creator must not alter the request");
    }

    #[test]
    fn creator_policy_stamps_absent_helper_policy() {
        let creator = allow_list_policy(&["send_message"]);
        let out = enforce_creator_tool_access_policy(
            spawn_helper_create_args(None),
            Some(&creator),
            "meerkat_schedule_create",
        )
        .expect("absent helper policy is stamped");
        let stamped: meerkat_core::ops::ToolAccessPolicy =
            serde_json::from_value(out["target"]["options"]["tool_access_policy"].clone())
                .expect("stamped policy parses");
        assert_eq!(stamped, creator);
    }

    #[test]
    fn creator_policy_stamps_inherit_helper_policy() {
        let creator = allow_list_policy(&["send_message"]);
        let out = enforce_creator_tool_access_policy(
            spawn_helper_create_args(Some(json!({ "type": "inherit" }))),
            Some(&creator),
            "meerkat_schedule_create",
        )
        .expect("inherit helper policy is stamped");
        let stamped: meerkat_core::ops::ToolAccessPolicy =
            serde_json::from_value(out["target"]["options"]["tool_access_policy"].clone())
                .expect("stamped policy parses");
        assert_eq!(stamped, creator);
    }

    #[test]
    fn creator_policy_allows_matching_explicit_helper_policy() {
        let creator = allow_list_policy(&["peers", "send_message"]);
        // Same set, different order: comparison is typed set equality, not
        // JSON text equality.
        let out = enforce_creator_tool_access_policy(
            spawn_helper_create_args(Some(json!({
                "type": "allow_list",
                "value": ["send_message", "peers"]
            }))),
            Some(&creator),
            "meerkat_schedule_create",
        )
        .expect("matching explicit policy passes");
        let kept: meerkat_core::ops::ToolAccessPolicy =
            serde_json::from_value(out["target"]["options"]["tool_access_policy"].clone())
                .expect("kept policy parses");
        assert_eq!(kept, creator);
    }

    #[test]
    fn creator_policy_denies_diverging_explicit_helper_policy() {
        let creator = allow_list_policy(&["send_message"]);
        let err = enforce_creator_tool_access_policy(
            spawn_helper_create_args(Some(json!({
                "type": "allow_list",
                "value": ["bash"]
            }))),
            Some(&creator),
            "meerkat_schedule_create",
        )
        .expect_err("a scheduled helper must not exceed the creator's policy");
        assert_eq!(err, ToolError::access_denied("meerkat_schedule_create"));
    }

    #[test]
    fn creator_policy_ignores_non_helper_targets() {
        let creator = allow_list_policy(&["send_message"]);
        let args = json!({
            "target": {
                "target_kind": "session",
                "type": "exact_session",
                "session_id": "01920000-0000-7000-8000-000000000001",
                "action": { "type": "prompt", "prompt": "hello" }
            }
        });
        let out = enforce_creator_tool_access_policy(
            args.clone(),
            Some(&creator),
            "meerkat_schedule_create",
        )
        .expect("non-helper targets pass through");
        assert_eq!(out, args);
    }

    #[test]
    fn creator_policy_inherit_creator_fails_closed() {
        let err = enforce_creator_tool_access_policy(
            spawn_helper_create_args(None),
            Some(&meerkat_core::ops::ToolAccessPolicy::Inherit),
            "meerkat_schedule_create",
        )
        .expect_err("an unresolved creator Inherit is a wiring fault");
        assert_eq!(err, ToolError::access_denied("meerkat_schedule_create"));
    }

    #[tokio::test]
    async fn gated_creator_helper_schedule_persists_stamped_policy() {
        let service = ScheduleService::new(Arc::new(MemoryScheduleStore::default()));
        let creator = allow_list_policy(&["send_message"]);
        let dispatcher = CurrentSessionScheduleToolDispatcher::new(
            Arc::new(ScheduleToolDispatcher::new(service.clone())),
            SessionId::new(),
        )
        .with_creator_tool_access_policy(Some(creator.clone()));

        let request = spawn_helper_create_args(None);
        let raw = RawValue::from_string(request.to_string()).expect("raw args");
        let outcome = dispatcher
            .dispatch(tool_call(
                "sched-gated-create",
                "meerkat_schedule_create",
                raw.as_ref(),
            ))
            .await
            .expect("gated helper schedule create succeeds");
        let created: Value =
            serde_json::from_str(&outcome.result.text_content()).expect("created schedule JSON");
        let persisted: meerkat_core::ops::ToolAccessPolicy =
            serde_json::from_value(created["target"]["options"]["tool_access_policy"].clone())
                .expect("persisted policy parses");
        assert_eq!(
            persisted, creator,
            "the stored schedule must carry the creator's policy"
        );
    }

    // --- bounded occurrence reads -------------------------------------------

    fn interval_trigger(every_seconds: u64) -> TriggerSpec {
        TriggerSpec::Interval(IntervalTriggerSpec {
            start_at_utc: Utc::now() + Duration::minutes(1),
            every_seconds,
            end_at_utc: None,
        })
    }

    fn bounded_read_request(trigger: TriggerSpec, occurrences: u32) -> CreateScheduleRequest {
        CreateScheduleRequest {
            trigger,
            planning_horizon_days: Some(2),
            planning_horizon_occurrences: Some(occurrences),
            ..schedule_request()
        }
    }

    async fn create_for_bounded_reads(
        service: &ScheduleService,
        occurrences: u32,
    ) -> Result<String, String> {
        let schedule = service
            .create(bounded_read_request(interval_trigger(60), occurrences))
            .await
            .map_err(|error| error.to_string())?;
        Ok(schedule.schedule_id.to_string())
    }

    async fn read_occurrences(
        service: &ScheduleService,
        args: Value,
    ) -> Result<Value, ScheduleToolError> {
        handle_schedule_tools_call(service, "meerkat_schedule_occurrences", &args).await
    }

    fn rows_of(reply: &Value) -> Result<Vec<Value>, String> {
        reply["occurrences"]
            .as_array()
            .cloned()
            .ok_or_else(|| format!("reply has no occurrences array: {reply}"))
    }

    fn ids_of(rows: &[Value]) -> Vec<String> {
        rows.iter()
            .filter_map(|row| row["occurrence_id"].as_str().map(str::to_string))
            .collect()
    }

    fn utc(text: &str) -> Result<DateTime<Utc>, String> {
        DateTime::parse_from_rfc3339(text)
            .map(|parsed| parsed.with_timezone(&Utc))
            .map_err(|error| format!("{text}: {error}"))
    }

    fn row_key(row: &Value) -> Result<OccurrencePageKey, String> {
        let due = row["due_at_utc"]
            .as_str()
            .ok_or_else(|| format!("row has no due_at_utc: {row}"))?;
        Ok(OccurrencePageKey {
            due_at_ms: utc(due)?.timestamp_millis(),
            schedule_revision: row["schedule_revision"]
                .as_u64()
                .ok_or_else(|| format!("row has no schedule_revision: {row}"))?,
            occurrence_ordinal: row["occurrence_ordinal"]
                .as_u64()
                .ok_or_else(|| format!("row has no occurrence_ordinal: {row}"))?,
        })
    }

    fn stockholm_morning_and_afternoon() -> TriggerSpec {
        TriggerSpec::Calendar(crate::CalendarTriggerSpec {
            timezone: "Europe/Stockholm".into(),
            minute: crate::CalendarFieldSpec::Values(vec![15]),
            hour: crate::CalendarFieldSpec::Values(vec![6, 16]),
            day_of_month: crate::CalendarFieldSpec::Any,
            month: crate::CalendarFieldSpec::Any,
            day_of_week: crate::CalendarFieldSpec::Any,
            year: None,
        })
    }

    #[tokio::test]
    async fn occurrences_without_bounded_options_keep_the_full_unpaged_reply() -> Result<(), String>
    {
        let service = ScheduleService::new(Arc::new(MemoryScheduleStore::default()));
        let schedule_id = create_for_bounded_reads(&service, 4).await?;

        let reply = read_occurrences(&service, json!({ "schedule_id": schedule_id }))
            .await
            .map_err(|error| format!("{error:?}"))?;

        let object = reply.as_object().ok_or("reply should be an object")?;
        assert_eq!(
            object.len(),
            1,
            "no page or receipts keys by default: {reply}"
        );
        let rows = rows_of(&reply)?;
        assert_eq!(rows.len(), 4);
        assert!(
            rows.iter()
                .all(|row| row.get("machine_state").is_some()
                    && row.get("target_snapshot").is_some()),
            "the default view stays the full occurrence object"
        );
        Ok(())
    }

    #[tokio::test]
    async fn summary_view_drops_the_payload_and_keeps_delivery_state() -> Result<(), String> {
        let service = ScheduleService::new(Arc::new(MemoryScheduleStore::default()));
        let schedule_id = create_for_bounded_reads(&service, 2).await?;

        let reply = read_occurrences(
            &service,
            json!({ "schedule_id": schedule_id, "view": "summary" }),
        )
        .await
        .map_err(|error| format!("{error:?}"))?;

        assert!(reply.get("page").is_none(), "no limit, no page: {reply}");
        let rows = rows_of(&reply)?;
        assert_eq!(rows.len(), 2);
        for row in &rows {
            for dropped in ["machine_state", "trigger_snapshot", "target_snapshot"] {
                assert!(row.get(dropped).is_none(), "summary keeps {dropped}: {row}");
            }
            assert_eq!(row["phase"], "pending");
            assert!(
                row["last_receipt"].is_null(),
                "a pending occurrence has no receipt"
            );
            assert!(
                row["due_at_local"].is_null(),
                "an interval trigger has no local time"
            );
            assert_eq!(row["target"]["target_kind"], "session");
            assert_eq!(row["target"]["type"], "exact_session");
            assert!(
                row["target"].get("action").is_none(),
                "the action payload is dropped"
            );
        }
        Ok(())
    }

    #[test]
    fn calendar_occurrences_map_to_local_time_across_the_dst_change() -> Result<(), String> {
        // Europe/Stockholm leaves summer time on 2026-10-25 at 03:00 local.
        let trigger = stockholm_morning_and_afternoon();
        let planned = crate::occurrences_for_horizon(
            &trigger,
            Some(utc("2026-10-24T12:00:00Z")?),
            utc("2026-11-01T00:00:00Z")?,
            64,
        )
        .map_err(|error| error.to_string())?;
        assert_eq!(
            planned.iter().take(4).copied().collect::<Vec<_>>(),
            vec![
                utc("2026-10-24T14:15:00Z")?,
                utc("2026-10-25T05:15:00Z")?,
                utc("2026-10-25T15:15:00Z")?,
                utc("2026-10-26T05:15:00Z")?,
            ],
            "16:15 is 14:15Z in summer time and 15:15Z after the change"
        );
        assert!(planned.contains(&utc("2026-10-31T15:15:00Z")?));

        let local = |at: DateTime<Utc>| local_due_at(&trigger, at);
        assert_eq!(
            local(utc("2026-10-24T14:15:00Z")?),
            Some((
                "2026-10-24T16:15:00+02:00".to_string(),
                "Europe/Stockholm".to_string()
            ))
        );
        assert_eq!(
            local(utc("2026-10-25T05:15:00Z")?).map(|(at, _)| at),
            Some("2026-10-25T06:15:00+01:00".to_string())
        );
        assert_eq!(
            local(utc("2026-10-25T15:15:00Z")?).map(|(at, _)| at),
            Some("2026-10-25T16:15:00+01:00".to_string())
        );
        assert_eq!(
            local(utc("2026-10-31T15:15:00Z")?).map(|(at, _)| at),
            Some("2026-10-31T16:15:00+01:00".to_string())
        );
        assert_eq!(
            local_due_at(&interval_trigger(60), utc("2026-10-25T15:15:00Z")?),
            None
        );
        Ok(())
    }

    #[test]
    fn a_local_day_range_selects_exactly_that_days_occurrences() -> Result<(), String> {
        // The 25-hour local day 2026-10-25 in Europe/Stockholm, as a caller
        // would pass it: first and last second, each with its own offset.
        let after = parse_due_bound("due_after_utc", Some("2026-10-25T00:00:00+02:00"))
            .map_err(|error| error.message)?
            .ok_or("due_after_utc should parse")?;
        let before = parse_due_bound("due_before_utc", Some("2026-10-25T23:59:59+01:00"))
            .map_err(|error| error.message)?
            .ok_or("due_before_utc should parse")?;
        assert_eq!(after, utc("2026-10-24T22:00:00Z")?);
        assert_eq!(before, utc("2026-10-25T22:59:59Z")?);

        let planned = crate::occurrences_for_horizon(
            &stockholm_morning_and_afternoon(),
            Some(utc("2026-10-24T12:00:00Z")?),
            utc("2026-11-01T00:00:00Z")?,
            64,
        )
        .map_err(|error| error.to_string())?;
        // The store's due bounds are inclusive on both ends.
        let day: Vec<_> = planned
            .into_iter()
            .filter(|due| *due >= after && *due <= before)
            .collect();
        assert_eq!(
            day,
            vec![utc("2026-10-25T05:15:00Z")?, utc("2026-10-25T15:15:00Z")?],
            "the day before (14:15Z) and the next week (10-31) stay out"
        );
        Ok(())
    }

    #[tokio::test]
    async fn due_bounds_are_inclusive_and_accept_any_offset() -> Result<(), String> {
        let service = ScheduleService::new(Arc::new(MemoryScheduleStore::default()));
        let schedule_id = create_for_bounded_reads(&service, 6).await?;
        let all = rows_of(
            &read_occurrences(
                &service,
                json!({ "schedule_id": schedule_id, "view": "summary" }),
            )
            .await
            .map_err(|error| format!("{error:?}"))?,
        )?;
        let second = all.get(1).ok_or("expected six planned occurrences")?;
        let third = all.get(2).ok_or("expected six planned occurrences")?;
        let plus_one = chrono::FixedOffset::east_opt(3600).ok_or("offset")?;
        let bound = |row: &Value| -> Result<String, String> {
            let due = row["due_at_utc"].as_str().ok_or("due_at_utc")?;
            Ok(utc(due)?.with_timezone(&plus_one).to_rfc3339())
        };

        let reply = read_occurrences(
            &service,
            json!({
                "schedule_id": schedule_id,
                "view": "summary",
                "due_after_utc": bound(second)?,
                "due_before_utc": bound(third)?,
            }),
        )
        .await
        .map_err(|error| format!("{error:?}"))?;

        assert_eq!(
            ids_of(&rows_of(&reply)?),
            ids_of(&[second.clone(), third.clone()]),
            "both bounds are inclusive and their offset is honored"
        );
        Ok(())
    }

    #[tokio::test]
    async fn an_exact_occurrence_read_never_crosses_schedules() -> Result<(), String> {
        let service = ScheduleService::new(Arc::new(MemoryScheduleStore::default()));
        let mine = create_for_bounded_reads(&service, 2).await?;
        let other = create_for_bounded_reads(&service, 2).await?;
        let my_rows = rows_of(
            &read_occurrences(&service, json!({ "schedule_id": mine }))
                .await
                .map_err(|error| format!("{error:?}"))?,
        )?;
        let occurrence_id = ids_of(&my_rows)
            .into_iter()
            .next()
            .ok_or("expected a planned occurrence")?;

        let exact = read_occurrences(
            &service,
            json!({ "schedule_id": mine, "occurrence_id": occurrence_id, "view": "summary" }),
        )
        .await
        .map_err(|error| format!("{error:?}"))?;
        assert_eq!(ids_of(&rows_of(&exact)?), vec![occurrence_id.clone()]);
        let receipts = exact["receipts"]
            .as_array()
            .ok_or("an exact read carries the receipts array")?;
        assert!(
            receipts
                .iter()
                .all(|receipt| receipt["occurrence_id"] == occurrence_id.as_str()),
            "only this occurrence's receipts"
        );

        // The same id under another schedule is not found, exactly like an
        // id that exists nowhere.
        for (schedule_id, candidate) in [
            (other, occurrence_id.clone()),
            (mine.clone(), crate::OccurrenceId::new().to_string()),
        ] {
            match read_occurrences(
                &service,
                json!({ "schedule_id": schedule_id, "occurrence_id": candidate }),
            )
            .await
            {
                Err(error) => assert_eq!(error.code, NOT_FOUND, "{error:?}"),
                Ok(reply) => return Err(format!("expected not found, got {reply}")),
            }
        }

        match read_occurrences(
            &service,
            json!({ "schedule_id": mine, "occurrence_id": occurrence_id, "limit": 1 }),
        )
        .await
        {
            Err(error) => assert_eq!(error.code, INVALID_ARGUMENTS, "{error:?}"),
            Ok(reply) => return Err(format!("occurrence_id with limit was accepted: {reply}")),
        }
        Ok(())
    }

    #[tokio::test]
    async fn pages_cover_every_row_once_in_due_order() -> Result<(), String> {
        let service = ScheduleService::new(Arc::new(MemoryScheduleStore::default()));
        let schedule_id = create_for_bounded_reads(&service, 6).await?;
        let everything = ids_of(&rows_of(
            &read_occurrences(&service, json!({ "schedule_id": schedule_id }))
                .await
                .map_err(|error| format!("{error:?}"))?,
        )?);
        assert_eq!(everything.len(), 6);

        let mut seen = Vec::new();
        let mut pages = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..10 {
            let mut args = json!({ "schedule_id": schedule_id, "limit": 4, "view": "summary" });
            if let Some(cursor) = &cursor {
                args["cursor"] = json!(cursor);
            }
            let reply = read_occurrences(&service, args)
                .await
                .map_err(|error| format!("{error:?}"))?;
            let rows = rows_of(&reply)?;
            let page = &reply["page"];
            assert_eq!(page["returned"].as_u64(), Some(rows.len() as u64));
            let has_more = page["has_more"].as_bool().ok_or("page.has_more")?;
            pages.push((rows.len(), has_more));
            seen.extend(ids_of(&rows));
            if !has_more {
                assert!(page["next_cursor"].is_null(), "the last page has no cursor");
                break;
            }
            cursor = Some(
                page["next_cursor"]
                    .as_str()
                    .ok_or("has_more without next_cursor")?
                    .to_string(),
            );
        }
        assert_eq!(pages, vec![(4, true), (2, false)]);
        assert_eq!(seen, everything, "every row exactly once, in due order");
        Ok(())
    }

    #[tokio::test]
    async fn a_revision_supersede_between_pages_neither_repeats_nor_reorders() -> Result<(), String>
    {
        let service = ScheduleService::new(Arc::new(MemoryScheduleStore::default()));
        let schedule_id = create_for_bounded_reads(&service, 6).await?;
        let first = read_occurrences(
            &service,
            json!({ "schedule_id": schedule_id, "limit": 2, "view": "summary" }),
        )
        .await
        .map_err(|error| format!("{error:?}"))?;
        let first_rows = rows_of(&first)?;
        let first_ids = ids_of(&first_rows);
        let boundary = row_key(first_rows.last().ok_or("first page is empty")?)?;
        let cursor = first["page"]["next_cursor"]
            .as_str()
            .ok_or("first page should continue")?
            .to_string();

        // Re-plan between the pages: the revision bumps and the old pending
        // rows are superseded by new ones.
        let parsed_id = parse_schedule_id(&schedule_id).map_err(|error| error.message)?;
        service
            .update(
                &parsed_id,
                UpdateScheduleRequest {
                    trigger: Some(interval_trigger(120)),
                    ..UpdateScheduleRequest::default()
                },
            )
            .await
            .map_err(|error| error.to_string())?;

        let second = read_occurrences(
            &service,
            json!({
                "schedule_id": schedule_id,
                "limit": 50,
                "cursor": cursor,
                "view": "summary",
            }),
        )
        .await
        .map_err(|error| format!("{error:?}"))?;
        let second_rows = rows_of(&second)?;
        let mut previous = boundary;
        for row in &second_rows {
            let id = row["occurrence_id"].as_str().ok_or("occurrence_id")?;
            assert!(
                !first_ids.iter().any(|seen| seen == id),
                "row {id} repeated after the cursor"
            );
            let key = row_key(row)?;
            assert!(
                key > previous,
                "rows after the cursor stay in key order: {row}"
            );
            previous = key;
        }
        Ok(())
    }

    #[test]
    fn page_cursor_round_trips_and_orders_by_due_revision_ordinal() {
        let key = OccurrencePageKey {
            due_at_ms: 1_792_941_300_000,
            schedule_revision: 3,
            occurrence_ordinal: 78,
        };
        assert_eq!(OccurrencePageKey::decode(&key.encode()).ok(), Some(key));
        let superseding = OccurrencePageKey {
            schedule_revision: 4,
            occurrence_ordinal: 79,
            ..key
        };
        assert!(
            superseding > key,
            "a later revision at the same instant sorts after"
        );
        let later = OccurrencePageKey {
            due_at_ms: key.due_at_ms + 1,
            schedule_revision: 1,
            occurrence_ordinal: 1,
        };
        assert!(later > superseding, "due time decides first");
        for bad in [
            "",
            "occ1.",
            "occ1.1.2",
            "occ1.1.2.3.4",
            "occ1.a.2.3",
            "x.1.2.3",
        ] {
            assert!(OccurrencePageKey::decode(bad).is_err(), "{bad:?} decoded");
        }
    }

    #[tokio::test]
    async fn malformed_bounded_options_are_refused() -> Result<(), String> {
        let service = ScheduleService::new(Arc::new(MemoryScheduleStore::default()));
        let schedule_id = create_for_bounded_reads(&service, 2).await?;
        let cases = [
            json!({ "limit": 0 }),
            json!({ "limit": MAX_OCCURRENCE_PAGE + 1 }),
            json!({ "cursor": "occ1.1.1.1" }),
            json!({ "limit": 2, "cursor": "not-a-cursor" }),
            json!({ "due_after_utc": "yesterday" }),
            json!({
                "due_after_utc": "2026-10-26T00:00:00Z",
                "due_before_utc": "2026-10-25T00:00:00Z",
            }),
            json!({ "occurrence_id": "not-a-uuid" }),
            json!({ "view": "everything" }),
        ];
        for mut case in cases {
            case["schedule_id"] = json!(schedule_id);
            match read_occurrences(&service, case.clone()).await {
                Err(error) => assert_eq!(error.code, INVALID_ARGUMENTS, "{case}: {error:?}"),
                Ok(reply) => return Err(format!("{case} was accepted: {reply}")),
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn schedule_list_describes_and_omits_deleted_schedules() -> Result<(), String> {
        let list_tool = schedule_tools_list()
            .into_iter()
            .find(|tool| tool["name"] == "meerkat_schedule_list")
            .ok_or("list tool")?;
        let description = list_tool["description"].as_str().ok_or("description")?;
        assert!(description.contains("deleted schedules are not listed"));

        let service = ScheduleService::new(Arc::new(MemoryScheduleStore::default()));
        let kept = create_for_bounded_reads(&service, 1).await?;
        let deleted = create_for_bounded_reads(&service, 1).await?;
        handle_schedule_tools_call(
            &service,
            "meerkat_schedule_delete",
            &json!({ "schedule_id": deleted }),
        )
        .await
        .map_err(|error| format!("{error:?}"))?;

        let listed = handle_schedule_tools_call(&service, "meerkat_schedule_list", &json!({}))
            .await
            .map_err(|error| format!("{error:?}"))?;
        let ids: Vec<&str> = listed["schedules"]
            .as_array()
            .ok_or("schedules array")?
            .iter()
            .filter_map(|schedule| schedule["schedule_id"].as_str())
            .collect();
        assert!(ids.contains(&kept.as_str()));
        assert!(!ids.contains(&deleted.as_str()));
        Ok(())
    }
}
