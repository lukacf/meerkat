//! Console schedules on `POST /console/rpc`: `meta/schedule/list`, `create`,
//! `update`, `pause`, `resume` and `delete`, each naming its agent as
//! `identity` (see `crate::console_schedules`).
//!
//! The list needs `agent.view` on the agent and every write `agent.send`
//! (`console_rpc_access_requirements`): a schedule is a deferred send, so a
//! write takes what `meta/console/send` takes on this route (a writable
//! console, a valid public alias, `agent.send` under the live access
//! policy) for the caller as the console resolves it, signed in or not. A
//! create, update or resume records that caller as the Console caller the
//! schedule fires as. The handler judges what the access table cannot: that
//! the schedule is a console schedule of that agent, and that the agent
//! takes the Console's sends.
//!
//! [`ConsoleSendDoor`] is the Console's own send path for the fires: the
//! stock send gate, governance and the identity-first or member send, as
//! `/console/send` runs them, for the schedule's sender.

use super::*;
use crate::console_schedules::{
    AttachedConsoleSchedules, ConsoleScheduleSender, ConsoleScheduledSend,
};
use meerkat::{
    CreateScheduleRequest, OccurrenceFilter, Schedule, ScheduleDomainError, ScheduleId,
    SchedulePhase, ScheduleService, ScheduleStoreError, TriggerSpec, UpdateScheduleRequest,
};

/// The read of the family.
pub(super) const SCHEDULE_LIST_METHOD: &str = "meta/schedule/list";

/// The writes of the family, each a deferred send's definition or its end.
pub(super) const SCHEDULE_WRITE_METHODS: &[&str] = &[
    "meta/schedule/create",
    "meta/schedule/update",
    "meta/schedule/pause",
    "meta/schedule/resume",
    "meta/schedule/delete",
];

/// The `origin` of a console schedule's send: the Console deduplicates it
/// under the occurrence's delivery key in its sender's scope.
const CONSOLE_SCHEDULE_SEND_ORIGIN: &str = "console:schedule";

/// Whether `method` writes a console schedule.
#[cfg(test)]
pub(super) fn is_write(method: &str) -> bool {
    SCHEDULE_WRITE_METHODS.contains(&method)
}

/// The console schedule methods this console offers the caller: the list
/// where the runtime's schedule host attached its service, and the writes
/// where the console is also writable and a firing host drains the store.
/// Whether the caller may view or send to a given agent is judged per call.
pub(super) fn advertised_methods(runtime: &MobRuntime, can_mutate: bool) -> Vec<&'static str> {
    let Some(schedules) = runtime.console_schedules().attached() else {
        return Vec::new();
    };
    let mut methods = vec![SCHEDULE_LIST_METHOD];
    if can_mutate && schedules.firing.is_bound() {
        methods.extend_from_slice(SCHEDULE_WRITE_METHODS);
    }
    methods
}

/// Answer one console schedule method.
pub(super) async fn handle(
    request: &JsonRpcRequest,
    runtime: &MobRuntime,
    identity_runtime: Option<&Arc<crate::identity_first::IdentityRuntime>>,
    aggregator: Option<&MetaConsoleAggregator>,
    principal: Option<&str>,
    governance: &ConsoleSendGovernance,
) -> Value {
    let response_id = request.id.clone().unwrap_or(Value::Null);
    let call = ScheduleCall {
        identity_runtime,
        aggregator,
        params: &request.params,
        principal,
        governance,
    };
    let answered = match runtime.console_schedules().attached() {
        Some(schedules) => Box::pin(answer(&request.method, &call, &schedules)).await,
        None => Err(JsonRpcError::new(-32601, "Method not found")),
    };
    match answered {
        Ok(result) => response_value(response_id, Some(result), None),
        Err(error) => response_value(response_id, None, Some(error)),
    }
}

/// One console schedule call: its params and caller, and what the console
/// resolves its agent with.
struct ScheduleCall<'a> {
    identity_runtime: Option<&'a Arc<crate::identity_first::IdentityRuntime>>,
    aggregator: Option<&'a MetaConsoleAggregator>,
    params: &'a Value,
    principal: Option<&'a str>,
    governance: &'a ConsoleSendGovernance,
}

async fn answer(
    method: &str,
    call: &ScheduleCall<'_>,
    schedules: &AttachedConsoleSchedules,
) -> Result<Value, JsonRpcError> {
    let identity = required_string(call.params, "identity")?;
    crate::member_comms_id::validate_public_member_alias("identity", &identity).map_err(invalid)?;
    match method {
        "meta/schedule/list" => list(call, schedules, &identity).await,
        "meta/schedule/create" => create(call, schedules, &identity).await,
        "meta/schedule/update" => update(call, schedules, &identity).await,
        "meta/schedule/pause" => pause(call, schedules, &identity).await,
        "meta/schedule/resume" => resume(call, schedules, &identity).await,
        "meta/schedule/delete" => delete(call, schedules, &identity).await,
        _ => Err(JsonRpcError::new(-32601, "Method not found")),
    }
}

async fn list(
    call: &ScheduleCall<'_>,
    schedules: &AttachedConsoleSchedules,
    identity: &str,
) -> Result<Value, JsonRpcError> {
    let target = known_target(call, identity)
        .await?
        .map_or_else(|| identity.to_string(), |target| target.identity);
    let all = schedules.service.list().await.map_err(schedule_error)?;
    let mut theirs = all
        .iter()
        .filter_map(|schedule| {
            let send = ConsoleScheduledSend::of_target(&schedule.target)?;
            (send.identity == target || send.identity == identity).then_some((schedule, send))
        })
        .collect::<Vec<_>>();
    theirs.sort_by_key(|(schedule, _)| schedule.config.created_at_utc);
    let mut items = Vec::with_capacity(theirs.len());
    for (schedule, send) in theirs {
        items.push(schedule_item(&schedules.service, schedule, &send).await);
    }
    Ok(json!({ "identity": target, "schedules": items }))
}

async fn create(
    call: &ScheduleCall<'_>,
    schedules: &AttachedConsoleSchedules,
    identity: &str,
) -> Result<Value, JsonRpcError> {
    let sender = sender(call)?;
    let content = required_content(call.params)?;
    let trigger = trigger_param(call.params)?.ok_or_else(|| invalid("trigger is required"))?;
    let name = optional_string(call.params, "name")?;
    let target = sendable_target(call, schedules, identity).await?;
    let send = ConsoleScheduledSend::new(target, content, sender, now_epoch_seconds());
    let schedule = schedules
        .service
        .create(CreateScheduleRequest {
            name,
            description: None,
            trigger,
            target: send.target().map_err(internal)?,
            misfire_policy: meerkat::MisfirePolicy::default(),
            overlap_policy: meerkat::OverlapPolicy::default(),
            missing_target_policy: meerkat::MissingTargetPolicy::default(),
            labels: BTreeMap::new(),
            planning_horizon_days: None,
            planning_horizon_occurrences: None,
        })
        .await
        .map_err(schedule_error)?;
    Ok(schedule_item(&schedules.service, &schedule, &send).await)
}

/// A changed schedule is the changer's send: the update records the caller
/// as its sender, whatever it changes.
async fn update(
    call: &ScheduleCall<'_>,
    schedules: &AttachedConsoleSchedules,
    identity: &str,
) -> Result<Value, JsonRpcError> {
    let sender = sender(call)?;
    let schedule_id = schedule_id_param(call.params)?;
    let content = optional_content(call.params)?;
    let trigger = trigger_param(call.params)?;
    let name = optional_string(call.params, "name")?;
    let (_, current) = console_schedule(call, schedules, identity, &schedule_id).await?;
    let target = sendable_target(call, schedules, &current.identity).await?;
    let send = ConsoleScheduledSend::new(
        target,
        content.unwrap_or(current.content),
        sender,
        now_epoch_seconds(),
    );
    let schedule = schedules
        .service
        .update(
            &schedule_id,
            UpdateScheduleRequest {
                name,
                trigger,
                target: Some(send.target().map_err(internal)?),
                ..UpdateScheduleRequest::default()
            },
        )
        .await
        .map_err(schedule_error)?;
    Ok(schedule_item(&schedules.service, &schedule, &send).await)
}

async fn pause(
    call: &ScheduleCall<'_>,
    schedules: &AttachedConsoleSchedules,
    identity: &str,
) -> Result<Value, JsonRpcError> {
    let schedule_id = schedule_id_param(call.params)?;
    let (_, send) = console_schedule(call, schedules, identity, &schedule_id).await?;
    let schedule = schedules
        .service
        .pause(&schedule_id)
        .await
        .map_err(schedule_error)?;
    Ok(schedule_item(&schedules.service, &schedule, &send).await)
}

/// A resumed schedule is the resumer's send: the resume records the caller
/// as its sender before it fires again.
async fn resume(
    call: &ScheduleCall<'_>,
    schedules: &AttachedConsoleSchedules,
    identity: &str,
) -> Result<Value, JsonRpcError> {
    let sender = sender(call)?;
    let schedule_id = schedule_id_param(call.params)?;
    let (paused, current) = console_schedule(call, schedules, identity, &schedule_id).await?;
    if paused.phase != SchedulePhase::Paused {
        return Err(invalid(format!("schedule {schedule_id} is not paused")));
    }
    let target = sendable_target(call, schedules, &current.identity).await?;
    let send = ConsoleScheduledSend::new(target, current.content, sender, now_epoch_seconds());
    // The sender is recorded while the schedule is still paused, so no fire
    // goes out as the earlier sender once it resumes.
    schedules
        .service
        .update(
            &schedule_id,
            UpdateScheduleRequest {
                target: Some(send.target().map_err(internal)?),
                ..UpdateScheduleRequest::default()
            },
        )
        .await
        .map_err(schedule_error)?;
    let schedule = schedules
        .service
        .resume(&schedule_id)
        .await
        .map_err(schedule_error)?;
    Ok(schedule_item(&schedules.service, &schedule, &send).await)
}

async fn delete(
    call: &ScheduleCall<'_>,
    schedules: &AttachedConsoleSchedules,
    identity: &str,
) -> Result<Value, JsonRpcError> {
    let schedule_id = schedule_id_param(call.params)?;
    let (_, send) = console_schedule(call, schedules, identity, &schedule_id).await?;
    schedules
        .service
        .delete(&schedule_id)
        .await
        .map_err(schedule_error)?;
    Ok(json!({
        "identity": send.identity,
        "schedule_id": schedule_id.to_string(),
        "deleted": true,
    }))
}

/// The Console caller a create, update or resume records as the schedule's
/// sender: the caller as `/console/send` resolves it, with the principal the
/// access policy keys (a verified human, another authenticated principal, or
/// the open Console's shared account).
fn sender(call: &ScheduleCall<'_>) -> Result<ConsoleScheduleSender, JsonRpcError> {
    ConsoleScheduleSender::of_caller(&call.governance.scope, call.principal)
        .ok_or_else(|| internal("the console could not resolve its caller"))
}

/// The agent a console schedule call names, as this console knows it.
struct ScheduleTarget {
    /// Its durable identity: what the schedule records.
    identity: String,
    /// Whether it takes human input (a console send).
    addressable: bool,
}

/// The agent `identity` names, as the console's send resolves it: the
/// identity runtime's identity, else the console's identity record that
/// names it by identity or runtime member id. `None` when nothing does.
async fn known_target(
    call: &ScheduleCall<'_>,
    identity: &str,
) -> Result<Option<ScheduleTarget>, JsonRpcError> {
    if let Some(identity_runtime) = call.identity_runtime
        && let Ok(parsed) = crate::identity_first::AgentIdentity::parse(identity)
        && let Ok(status) = identity_runtime.status(&parsed).await
    {
        return Ok(Some(ScheduleTarget {
            identity: identity.to_string(),
            addressable: status.addressability
                == crate::identity_first::AgentAddressability::Addressable,
        }));
    }
    let Some(aggregator) = call.aggregator else {
        return Ok(None);
    };
    let records = Box::pin(aggregator.identity_records_now())
        .await
        .map_err(|error| internal(format!("the console's agents could not be read: {error}")))?;
    Ok(records
        .into_iter()
        .find(|record| record.identity == identity || record.runtime_member_id == identity)
        .map(|record| ScheduleTarget {
            identity: record.identity,
            addressable: record.addressable,
        }))
}

/// The agent a schedule may be defined for, refused typed when the
/// schedule could never be sent: no firing host drains the store, no agent
/// has the name, the agent takes no human input, or its sends are governed
/// (a governed send needs its human present, and a fire has none).
async fn sendable_target(
    call: &ScheduleCall<'_>,
    schedules: &AttachedConsoleSchedules,
    identity: &str,
) -> Result<String, JsonRpcError> {
    if !schedules.firing.is_bound() {
        return Err(schedule_host_unavailable());
    }
    let Some(target) = known_target(call, identity).await? else {
        return Err(unknown_identity(identity));
    };
    if !target.addressable {
        return Err(not_addressable(&target.identity));
    }
    let governed = match crate::identity_first::AgentIdentity::parse(&target.identity) {
        Ok(parsed) => call.governance.governed(&parsed),
        // A member outside identity-first admission takes no governed send,
        // and no send at all once Console sends can be governed.
        Err(_) => call.governance.runtime_governed || call.governance.ingress.is_some(),
    };
    if governed {
        return Err(governed_target(&target.identity));
    }
    Ok(target.identity)
}

/// The console schedule `schedule_id` of the agent `identity` names, with
/// its send. A schedule that does not exist, is not a console schedule, was
/// deleted or is another agent's is answered alike: not found.
async fn console_schedule(
    call: &ScheduleCall<'_>,
    schedules: &AttachedConsoleSchedules,
    identity: &str,
    schedule_id: &ScheduleId,
) -> Result<(Schedule, ConsoleScheduledSend), JsonRpcError> {
    let schedule = schedules
        .service
        .get(schedule_id)
        .await
        .map_err(schedule_error)?;
    let Some(send) = ConsoleScheduledSend::of_target(&schedule.target) else {
        return Err(schedule_not_found());
    };
    if schedule.phase == SchedulePhase::Deleted {
        return Err(schedule_not_found());
    }
    if send.identity != identity {
        let target = known_target(call, identity).await?;
        if target.is_none_or(|target| target.identity != send.identity) {
            return Err(schedule_not_found());
        }
    }
    Ok((schedule, send))
}

/// One console schedule as the methods answer it.
async fn schedule_item(
    service: &ScheduleService,
    schedule: &Schedule,
    send: &ConsoleScheduledSend,
) -> Value {
    let next_due_at_utc = if schedule.phase == SchedulePhase::Active {
        service
            .list_occurrences_matching(
                &schedule.schedule_id,
                OccurrenceFilter {
                    include_terminal: false,
                    limit: Some(1),
                    ..OccurrenceFilter::default()
                },
            )
            .await
            .ok()
            .and_then(|occurrences| occurrences.into_iter().next())
            .map(|occurrence| occurrence.due_at_utc)
    } else {
        None
    };
    let defined_at_utc = i64::try_from(send.defined_at)
        .ok()
        .and_then(|seconds| chrono::DateTime::from_timestamp(seconds, 0));
    json!({
        "schedule_id": schedule.schedule_id.to_string(),
        "identity": send.identity,
        "name": schedule.config.name,
        "content": send.content,
        "trigger": schedule.trigger,
        "phase": schedule.phase,
        "revision": schedule.revision.0,
        "sender": send.sender.principal(),
        "defined_at_utc": defined_at_utc,
        "created_at_utc": schedule.config.created_at_utc,
        "updated_at_utc": schedule.config.updated_at_utc,
        "next_due_at_utc": next_due_at_utc,
    })
}

fn now_epoch_seconds() -> u64 {
    u64::try_from(chrono::Utc::now().timestamp()).unwrap_or_default()
}

fn required_string(params: &Value, key: &str) -> Result<String, JsonRpcError> {
    params
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| invalid(format!("{key} must be a non-empty string")))
}

fn optional_string(params: &Value, key: &str) -> Result<Option<String>, JsonRpcError> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.trim().to_string())),
        Some(_) => Err(invalid(format!("{key} must be a string"))),
    }
}

/// The message, kept as written; refused when it is blank.
fn required_content(params: &Value) -> Result<String, JsonRpcError> {
    optional_content(params)?.ok_or_else(|| invalid("content must be a non-empty string"))
}

fn optional_content(params: &Value) -> Result<Option<String>, JsonRpcError> {
    match params.get("content") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(content)) if !content.trim().is_empty() => Ok(Some(content.clone())),
        Some(_) => Err(invalid("content must be a non-empty string")),
    }
}

fn trigger_param(params: &Value) -> Result<Option<TriggerSpec>, JsonRpcError> {
    match params.get("trigger") {
        None | Some(Value::Null) => Ok(None),
        Some(trigger) => serde_json::from_value(trigger.clone())
            .map(Some)
            .map_err(|error| invalid(format!("invalid trigger: {error}"))),
    }
}

fn schedule_id_param(params: &Value) -> Result<ScheduleId, JsonRpcError> {
    let raw = required_string(params, "schedule_id")?;
    ScheduleId::parse(&raw).map_err(|error| invalid(format!("invalid schedule_id: {error}")))
}

fn invalid(message: impl Into<String>) -> JsonRpcError {
    JsonRpcError::new(-32602, message)
}

fn internal(message: impl Into<String>) -> JsonRpcError {
    let message = message.into();
    tracing::warn!(
        target: "meta::console",
        error = %message,
        "console schedule internal error"
    );
    JsonRpcError::new(-32000, message.clone())
        .with_data(json!({ "error": "internal_error", "detail": message }))
}

fn schedule_not_found() -> JsonRpcError {
    JsonRpcError::new(-32001, "console schedule not found")
        .with_data(json!({ "kind": "schedule_not_found" }))
}

fn unknown_identity(identity: &str) -> JsonRpcError {
    JsonRpcError::new(-32001, format!("unknown identity: {identity}"))
        .with_data(json!({ "kind": "unknown_identity" }))
}

fn not_addressable(identity: &str) -> JsonRpcError {
    JsonRpcError::new(
        -32002,
        format!("{identity} takes no console sends, so no console schedule"),
    )
    .with_data(json!({ "kind": "not_addressable" }))
}

fn governed_target(identity: &str) -> JsonRpcError {
    JsonRpcError::new(
        crate::rpc::CAPABILITY_UNAVAILABLE_CODE,
        format!(
            "{identity} admits only governed input, which needs its Console human present; a \
             console schedule cannot be sent to it"
        ),
    )
    .with_data(json!({ "kind": "governed_target" }))
}

fn schedule_host_unavailable() -> JsonRpcError {
    JsonRpcError::new(
        crate::rpc::CAPABILITY_UNAVAILABLE_CODE,
        "no schedule firing host drains this gateway's schedule store, so a console schedule \
         would never be sent",
    )
    .with_data(json!({ "kind": "schedule_host_unavailable" }))
}

fn schedule_error(error: ScheduleDomainError) -> JsonRpcError {
    match &error {
        ScheduleDomainError::Store(ScheduleStoreError::ScheduleNotFound { .. }) => {
            schedule_not_found()
        }
        ScheduleDomainError::InvalidSchedule(_)
        | ScheduleDomainError::InvalidTrigger(_)
        | ScheduleDomainError::InvalidCron(_) => invalid(error.to_string()),
        ScheduleDomainError::DriverStopped
        | ScheduleDomainError::Store(ScheduleStoreError::UnsupportedBackend { .. }) => {
            schedule_host_unavailable()
        }
        _ => internal(error.to_string()),
    }
}

/// The Console's own send path for console schedule fires
/// (`crate::console_schedules::ConsoleScheduleDoor`): what `/console/send`
/// sends through, as the console router built last composed it, holding the
/// runtime's own non-owning back-edges.
pub(crate) struct ConsoleSendDoor {
    pub(crate) decisions: RuntimeDecisionState,
    pub(crate) runtime: MobRuntime,
    pub(crate) aggregator: MetaConsoleAggregator,
    pub(crate) access: Option<AccessController>,
    pub(crate) identity_runtime: Option<Arc<crate::identity_first::IdentityRuntime>>,
    pub(crate) console_events: ConsoleEventStore,
    pub(crate) governed_ingress: crate::console_governed_ingress::ConsoleGovernedIngressSlot,
    pub(crate) operator_resolver:
        Option<Arc<crate::memory::coordinator::ConsolePrincipalOperatorResolver>>,
    pub(crate) session_fences: crate::auth::session_fence::ConsoleSessionFences,
}

#[async_trait::async_trait]
impl crate::console_schedules::ConsoleScheduleDoor for ConsoleSendDoor {
    /// A fire is a `/console/send` by the schedule's sender, judged now: a
    /// verified human's Console sessions were not revoked since it defined
    /// the schedule, the stock send gate passes (a writable console, a valid
    /// public alias, and `agent.send` for the sender's principal under the
    /// live access policy), and the send goes through governance (a governed
    /// target is refused: a fire has no human present) and then the
    /// identity-first send or the member lane, deduplicated under
    /// `delivery_key` in the sender's send scope, as a live send by the
    /// sender would be, so a redelivered occurrence never sends twice.
    async fn send(&self, send: &ConsoleScheduledSend, delivery_key: &str) -> Result<(), String> {
        let sender = &send.sender;
        if let ConsoleScheduleSender::Human { principal, .. } = sender
            && self
                .session_fences
                .not_before(principal)
                .is_some_and(|not_before| not_before > send.defined_at)
        {
            return Err(format!(
                "the Console sessions of {principal} were revoked after the schedule was defined"
            ));
        }
        let access_view = self
            .access
            .as_ref()
            .map(|controller| controller.view_for_subject(sender.principal()));
        console_send_gate(
            &self.decisions,
            Some(&self.runtime),
            Some(&self.aggregator),
            self.access.as_ref(),
            access_view.as_ref(),
            &send.identity,
        )
        .await
        .map_err(|refusal| {
            format!(
                "the Console refused the scheduled send: {}",
                refusal.message()
            )
        })?;
        if let Some(resolver) = self.operator_resolver.as_ref()
            && let Some(subject) = access_view.as_ref().and_then(AccessView::subject)
        {
            resolver.note_interaction(send.identity.as_str(), subject);
        }
        let governance = ConsoleSendGovernance {
            ingress: self.governed_ingress.current(),
            human: None,
            scope: sender.scope(),
            runtime_governed: self.runtime.native_work_governed(),
            access: self.access.clone(),
        };
        let request = ConsoleSendRequest {
            identity: send.identity.clone(),
            content: Value::String(send.content.clone()),
            origin: CONSOLE_SCHEDULE_SEND_ORIGIN.to_string(),
            idempotency_key: delivery_key.to_string(),
            handling_mode: None,
            origin_kind: Some(crate::console_aggregator::ConsoleTurnOrigin::Scheduler),
            skill_refs: Vec::new(),
        };
        let sent = match self.identity_runtime.as_ref() {
            Some(identity_runtime) => {
                Box::pin(console_send_governed_with_identity_first_fallback(
                    &self.aggregator,
                    Arc::clone(identity_runtime),
                    Some(&self.console_events),
                    request,
                    &governance,
                ))
                .await
            }
            None => match governance.refuse_ungoverned_delivery(&send.identity) {
                Ok(()) => Box::pin(self.aggregator.send_scoped(request, &governance.scope)).await,
                Err(error) => Err(error),
            },
        };
        sent.map(|_accepted| ()).map_err(|error| {
            format!(
                "the Console refused the scheduled send: {}",
                console_send_public_message(&error)
            )
        })
    }
}
