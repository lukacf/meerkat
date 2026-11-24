//! Console schedules on `POST /console/rpc`: `meta/schedule/list`, `create`,
//! `update`, `pause`, `resume` and `delete`, each naming its agent as
//! `identity` (see `crate::console_schedules`).
//!
//! The list needs `agent.view` on the agent and every write `agent.send` and
//! `schedule.manage` (`console_rpc_access_requirements`): a schedule is a
//! deferred send, so a write takes what `meta/console/send` takes on this
//! route (a writable console, a valid public alias, `agent.send` under the
//! live access policy) for the caller as the console resolves it, signed in
//! or not, and `schedule.manage` lets a deployment keep some who message the
//! agent from changing its schedules. A create records that caller as the
//! Console caller the schedule fires as; an update or a resume by its
//! sender keeps it, and one by anyone else takes the schedule over only
//! when it says so (`take_over_sender`), recording the replaced sender in
//! the schedule's history. The handler judges what the access table cannot:
//! that the schedule is a console schedule of that agent, and that the
//! agent takes the Console's sends. An update and a resume are refused when
//! the schedule moved past the revision they name (`expected_revision`),
//! and write at the revision they read, so neither overwrites a change made
//! meanwhile. A pause, a delete and a create take no `expected_revision`
//! and no `take_over_sender`, and refuse them rather than ignore them. Each
//! write answers the schedule as it left it. Who a schedule sends as is
//! shown only to a caller who may send to its agent. The list and every
//! write but a create answer for an agent the console withholds from their
//! caller, because it is hidden from the caller or because the console does
//! not know it, as for an agent that does not exist
//! ([`acts_on_member_schedules`]).
//!
//! The agent a call names is an agent of the mob its request was served
//! for: the call finds it in that mob and judges it there, whatever agents
//! of that name the host's other mobs have. For a mob other than the host's
//! default mob it is the agent that mob's identity runtime knows; a name it
//! does not know is answered as a name no agent has. The fires reach the
//! agents of the host's default mob, so a create, an update and a resume
//! for an agent of another mob are refused (`schedule_host_unavailable`):
//! the schedule would never be sent.
//!
//! [`ConsoleSendDoor`] is the Console's own send path for the fires: the
//! Console's admission of the schedule's sender, then the very send
//! `/console/send` runs once it has admitted its caller
//! ([`send_as_admitted_console_caller`]).

use super::*;
use crate::console_schedules::{
    AttachedConsoleSchedules, ConsoleScheduleSender, ConsoleScheduledSend,
};
use meerkat::{
    CreateScheduleRequest, OccurrenceFilter, Schedule, ScheduleDomainError, ScheduleId,
    SchedulePhase, ScheduleRevision, ScheduleService, ScheduleStoreError, TriggerSpec,
    UpdateScheduleRequest,
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

/// The shortest interval a console schedule repeats at: a Console caller's
/// deferred message, not a pacing loop.
const MIN_CONSOLE_INTERVAL_SECONDS: u64 = 60;

/// Whether `method` writes a console schedule.
#[cfg(test)]
pub(super) fn is_write(method: &str) -> bool {
    SCHEDULE_WRITE_METHODS.contains(&method)
}

/// The method that defines a new console schedule: it names the agent the
/// schedule is recorded for, and no schedule that exists.
const SCHEDULE_CREATE_METHOD: &str = "meta/schedule/create";

/// Whether `method` reads or changes the console schedules of the agent it
/// names: the list, and every write but a create. The console's member
/// resolution answers such a method for an agent the console withholds from
/// its caller because it does not know it as for an agent that does not
/// exist (`console_rpc_resolve_members`): the list is empty, and a schedule
/// named by id is answered as not found.
pub(super) fn acts_on_member_schedules(method: &str) -> bool {
    method == SCHEDULE_LIST_METHOD
        || (method != SCHEDULE_CREATE_METHOD && SCHEDULE_WRITE_METHODS.contains(&method))
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

/// The mob a console schedule call is answered for, on its host.
pub(super) struct ScheduleMob<'a> {
    /// The host's default mob, as the host states it: a stored send that
    /// names no mob is its agent's.
    host_default_mob: &'a crate::member_key::DefaultMob,
    /// The wrapper of the mob the request resolved: the call names its
    /// agents.
    runtime: &'a MobRuntime,
}

impl<'a> ScheduleMob<'a> {
    /// The mob `runtime` wraps, on the host whose default mob is
    /// `host_default_mob`.
    pub(super) fn on_host(
        host_default_mob: &'a crate::member_key::DefaultMob,
        runtime: &'a MobRuntime,
    ) -> Self {
        Self {
            host_default_mob,
            runtime,
        }
    }
}

/// Answer one console schedule method for the mob `mob` names, on its host.
/// The host's default mob is the one the host states, never the mob of the
/// request's wrapper: a schedule of another mob's agent stores a send that
/// names that mob.
pub(super) async fn handle_for(
    mob: ScheduleMob<'_>,
    request: &JsonRpcRequest,
    identity_runtime: Option<&Arc<crate::identity_first::IdentityRuntime>>,
    aggregator: Option<&MetaConsoleAggregator>,
    principal: Option<&str>,
    access_view: Option<&AccessView>,
    governance: &ConsoleSendGovernance,
) -> Value {
    let response_id = request.id.clone().unwrap_or(Value::Null);
    let runtime = mob.runtime;
    let call = ScheduleCall {
        default_mob: mob.host_default_mob.clone(),
        mob_id: runtime.handle().mob_id().clone(),
        runtime,
        identity_runtime,
        aggregator,
        params: &request.params,
        principal,
        access_view,
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
    /// The host's default mob, as the host states it: a stored send that
    /// names no mob is its.
    default_mob: crate::member_key::DefaultMob,
    /// The mob whose agents the call names: the mob the request resolved.
    mob_id: meerkat_mob::MobId,
    /// The runtime whose mob the agents are members of: the list reads which
    /// schedules are aimed at an agent from it.
    runtime: &'a MobRuntime,
    /// That mob's identity runtime.
    identity_runtime: Option<&'a Arc<crate::identity_first::IdentityRuntime>>,
    /// The console's aggregator, built over the host's default mob. Its
    /// records are read by name, for a call for the default mob alone
    /// ([`known_target`]).
    aggregator: Option<&'a MetaConsoleAggregator>,
    params: &'a Value,
    principal: Option<&'a str>,
    /// The caller's access snapshot, when an access controller is wired.
    access_view: Option<&'a AccessView>,
    governance: &'a ConsoleSendGovernance,
}

impl ScheduleCall<'_> {
    /// Whether the caller may send to `identity`, and so may see who its
    /// schedules send as: a caller who may only view the agent does not.
    fn may_send_to(&self, identity: &str) -> bool {
        self.access_view
            .filter(|view| view.enforced())
            .is_none_or(|view| view.allows_agent(ACTION_AGENT_SEND, identity))
    }
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
    let shows_sender = call.may_send_to(identity);
    let all = schedules.service.list().await.map_err(schedule_error)?;
    let mut theirs = all
        .iter()
        .filter_map(|schedule| {
            let send = ConsoleScheduledSend::of_target(&schedule.target)?;
            (send.is_for(&call.default_mob, &call.mob_id, &target)
                || send.is_for(&call.default_mob, &call.mob_id, identity))
            .then_some((schedule, send))
        })
        .collect::<Vec<_>>();
    theirs.sort_by_key(|(schedule, _)| schedule.config.created_at_utc);
    let mut items = Vec::with_capacity(theirs.len());
    for (schedule, send) in theirs {
        items.push(schedule_item(&schedules.service, schedule, &send, shows_sender).await);
    }
    let others = other_schedules(call, &schedules.service, &all, &[&target, identity]).await;
    Ok(json!({ "identity": target, "schedules": items, "other_schedules": others }))
}

/// The schedules of `all` that are no Console schedules and whose target
/// names the agent `names` spell (its name as the console resolves it, and
/// as the caller wrote it), oldest first: the schedules an agent's schedule
/// tools or a host wrote for it. A Console caller reads them beside the
/// agent's Console schedules, and the Console's writes do not reach them.
///
/// A target names the agent as a member of the runtime's mob (a member
/// target, or the source of a fork helper), by an identity (the
/// `mob_member:` form for that mob, or the agent's name), or by a session
/// Core's entry of the agent records. An entry that cannot be read leaves
/// the agent's session schedules out.
async fn other_schedules(
    call: &ScheduleCall<'_>,
    service: &ScheduleService,
    all: &[Schedule],
    names: &[&str],
) -> Vec<Value> {
    let mob = call.runtime.handle();
    let mob_id = mob.mob_id().to_string();
    let roster_id = |name: &str| {
        crate::member_comms_id::mob_member_id_str(&crate::member_comms_id::runtime_alias_str(name))
            .into_owned()
    };
    let agents = names.iter().map(|name| roster_id(name)).collect::<Vec<_>>();
    let mut sessions = Vec::new();
    for agent in &agents {
        if let Ok(meerkat_mob::IdentityEntryRead::Entry(entry)) =
            mob.identity_entry(&meerkat_mob::AgentIdentity::from(agent.as_str()))
        {
            if let meerkat_mob::EntryStanding::Seated(seated)
            | meerkat_mob::EntryStanding::Retiring(seated) = &entry.standing
                && let Some(session_id) = seated.session_id.as_ref()
            {
                sessions.push(session_id.to_string());
            }
            if let Some(session_id) = entry.archived_session.as_ref() {
                sessions.push(session_id.to_string());
            }
        }
    }
    let names_agent =
        |named_mob: &str, member: &str| named_mob == mob_id && agents.contains(&roster_id(member));
    let mut theirs =
        all.iter()
            .filter(|schedule| ConsoleScheduledSend::of_target(&schedule.target).is_none())
            .filter_map(|schedule| {
                let (form, content) = match &schedule.target {
                    meerkat::TargetBinding::Mob(binding) => match binding.as_ref() {
                        meerkat::MobTargetBinding::Member {
                            mob_id: named_mob,
                            member_id,
                            action: meerkat::ScheduledMobAction::Send { content, .. },
                        } if names_agent(named_mob, member_id) => {
                            ("member", Some(content.text_content()))
                        }
                        meerkat::MobTargetBinding::ForkHelper {
                            mob_id: named_mob,
                            source_member_id,
                            prompt,
                            ..
                        } if names_agent(named_mob, source_member_id) => {
                            ("fork_helper", Some(prompt.clone()))
                        }
                        _ => return None,
                    },
                    meerkat::TargetBinding::Identity(binding) => {
                        let named = match meerkat::surface::parse_mob_member_schedule_identity(
                            binding.identity(),
                        ) {
                            Some(named) => names_agent(&named.mob_id, &named.member),
                            // A target that names no mob is the host's
                            // default mob's agent, in every mob's view.
                            None => {
                                call.default_mob.is(&call.mob_id)
                                    && agents.contains(&roster_id(binding.identity()))
                            }
                        };
                        if !named {
                            return None;
                        }
                        ("identity", prompt_text(binding.action()))
                    }
                    meerkat::TargetBinding::Session(session) => {
                        let (session_id, action) = match session.as_ref() {
                            meerkat::SessionTargetBinding::ExactSession { session_id, action }
                            | meerkat::SessionTargetBinding::ResumableSession {
                                session_id,
                                action,
                            } => (session_id, action),
                            meerkat::SessionTargetBinding::MaterializeOnDemandSession {
                                bound_session_id: Some(session_id),
                                action,
                                ..
                            } => (session_id, action),
                            meerkat::SessionTargetBinding::MaterializeOnDemandSession {
                                ..
                            } => {
                                return None;
                            }
                        };
                        if !sessions.contains(&session_id.to_string()) {
                            return None;
                        }
                        ("session", prompt_text(action))
                    }
                    meerkat::TargetBinding::HostRunnable(_) => return None,
                };
                Some((schedule, form, content))
            })
            .collect::<Vec<_>>();
    theirs.sort_by_key(|(schedule, _, _)| schedule.config.created_at_utc);
    let mut items = Vec::with_capacity(theirs.len());
    for (schedule, form, content) in theirs {
        items.push(json!({
            "schedule_id": schedule.schedule_id.to_string(),
            "identity": names.first(),
            "name": schedule.config.name,
            "target": form,
            "content": content,
            "trigger": schedule.trigger,
            "phase": schedule.phase,
            "revision": schedule.revision.0,
            "created_at_utc": schedule.config.created_at_utc,
            "updated_at_utc": schedule.config.updated_at_utc,
            "next_due_at_utc": next_due_at_utc(service, schedule).await,
        }));
    }
    items
}

/// The text a scheduled prompt sends, when the action is a prompt.
fn prompt_text(action: &meerkat::ScheduledSessionAction) -> Option<String> {
    match action {
        meerkat::ScheduledSessionAction::Prompt { prompt, .. } => Some(prompt.text_content()),
        _ => None,
    }
}

/// When the next occurrence of `schedule` is due, while it is active.
async fn next_due_at_utc(
    service: &ScheduleService,
    schedule: &Schedule,
) -> Option<chrono::DateTime<chrono::Utc>> {
    if schedule.phase != SchedulePhase::Active {
        return None;
    }
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
}

async fn create(
    call: &ScheduleCall<'_>,
    schedules: &AttachedConsoleSchedules,
    identity: &str,
) -> Result<Value, JsonRpcError> {
    refuse_unkept_preconditions(call.params, "meta/schedule/create")?;
    let sender = sender(call)?;
    let content = required_content(call.params)?;
    let trigger = trigger_param(call.params)?.ok_or_else(|| invalid("trigger is required"))?;
    let name = optional_string(call.params, "name")?;
    let target = sendable_target(call, schedules, identity).await?;
    refuse_archived(call, &target).await?;
    #[cfg(test)]
    schedules.wait_at_write_hold().await;
    let send = ConsoleScheduledSend::new(
        &call.default_mob,
        &call.mob_id,
        target,
        content,
        sender,
        now_epoch_seconds(),
    );
    caller_current(call)?;
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
    let schedule = paused_if_archived_since(call, schedules, &send.identity, schedule).await;
    Ok(schedule_item(&schedules.service, &schedule, &send, true).await)
}

/// Refuse a create or an update for an agent Core holds archived, as a send
/// to it is refused (`identity_archived`): an archive pauses the agent's
/// schedules, and nothing would pause one written afterwards. A pause, a
/// resume and a delete by name stay. An entry Core could not read refuses
/// too, retryable: it is never read as "not archived".
async fn refuse_archived(call: &ScheduleCall<'_>, identity: &str) -> Result<(), JsonRpcError> {
    match archived_refusal(call, identity).await {
        Ok(None) => Ok(()),
        Ok(Some(refusal)) | Err(refusal) => Err(crate::rpc::identity_json_rpc_error(&refusal)),
    }
}

/// The identity runtime's refusal of a schedule for `identity` while Core
/// holds it archived; `Ok(None)` for any other agent, and on a console with
/// no identity runtime, whose agents are never archived.
async fn archived_refusal(
    call: &ScheduleCall<'_>,
    identity: &str,
) -> Result<
    Option<crate::identity_first::IdentityRuntimeError>,
    crate::identity_first::IdentityRuntimeError,
> {
    let Some(identity_runtime) = call.identity_runtime else {
        return Ok(None);
    };
    let Ok(parsed) = crate::identity_first::AgentIdentity::parse(identity) else {
        return Ok(None);
    };
    identity_runtime.archived_refusal(&parsed).await
}

/// `schedule`, just created for `identity`, paused for its target when the
/// agent was archived since the create read it: an archive lists its
/// agent's schedules once, after it committed, so one written after that
/// listing is in none, and the archive committed before this read. The
/// pause is the schedule lifecycle's own for a target that takes no
/// deliveries, which the agent's unarchive ends.
async fn paused_if_archived_since(
    call: &ScheduleCall<'_>,
    schedules: &AttachedConsoleSchedules,
    identity: &str,
    schedule: Schedule,
) -> Schedule {
    if !matches!(archived_refusal(call, identity).await, Ok(Some(_))) {
        return schedule;
    }
    match schedules
        .service
        .pause_for_target(&schedule.schedule_id)
        .await
    {
        Ok((paused, _)) => paused,
        Err(error) => {
            tracing::warn!(
                target: "meta::console",
                %error,
                schedule_id = %schedule.schedule_id,
                "a console schedule created while its agent was archived could not be paused \
                 for it: it stays active until it is paused by name"
            );
            schedule
        }
    }
}

/// A change is defined again as its caller: by the schedule's sender, it
/// keeps the sender; by anyone else, it takes the schedule over, which the
/// caller must confirm (`take_over_sender`).
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
    let expected = expected_revision_param(call.params)?;
    let take_over = take_over_sender_param(call.params)?;
    let (read, current) = console_schedule(call, schedules, identity, &schedule_id).await?;
    unchanged_since(expected, &read)?;
    takeover_confirmed(&current.sender, &sender, take_over)?;
    let target = sendable_target(call, schedules, &current.identity).await?;
    refuse_archived(call, &target).await?;
    #[cfg(test)]
    schedules.wait_at_write_hold().await;
    caller_current(call)?;
    let send = current.redefined(
        target,
        content.unwrap_or_else(|| current.content.clone()),
        sender,
        now_epoch_seconds(),
    );
    let schedule = update_at(
        &schedules.service,
        &schedule_id,
        read.revision,
        UpdateScheduleRequest {
            name,
            trigger,
            target: Some(send.target().map_err(internal)?),
            ..UpdateScheduleRequest::default()
        },
    )
    .await?;
    Ok(schedule_item(&schedules.service, &schedule, &send, true).await)
}

async fn pause(
    call: &ScheduleCall<'_>,
    schedules: &AttachedConsoleSchedules,
    identity: &str,
) -> Result<Value, JsonRpcError> {
    refuse_unkept_preconditions(call.params, "meta/schedule/pause")?;
    let schedule_id = schedule_id_param(call.params)?;
    let (_, send) = console_schedule(call, schedules, identity, &schedule_id).await?;
    #[cfg(test)]
    schedules.wait_at_write_hold().await;
    caller_current(call)?;
    let schedule = schedules
        .service
        .pause(&schedule_id)
        .await
        .map_err(schedule_error)?;
    // Core pauses the schedule as it is then: a change made since the read
    // above is the schedule's, so the answer names it.
    Ok(schedule_item(
        &schedules.service,
        &schedule,
        &as_left(&schedule, send),
        true,
    )
    .await)
}

/// A resume is defined again as its caller, as an update is: the sender
/// resumes its own schedule, and anyone else takes it over only when it
/// says so.
async fn resume(
    call: &ScheduleCall<'_>,
    schedules: &AttachedConsoleSchedules,
    identity: &str,
) -> Result<Value, JsonRpcError> {
    let sender = sender(call)?;
    let schedule_id = schedule_id_param(call.params)?;
    let expected = expected_revision_param(call.params)?;
    let take_over = take_over_sender_param(call.params)?;
    let (paused, current) = console_schedule(call, schedules, identity, &schedule_id).await?;
    unchanged_since(expected, &paused)?;
    if paused.phase != SchedulePhase::Paused {
        return Err(invalid(format!("schedule {schedule_id} is not paused")));
    }
    takeover_confirmed(&current.sender, &sender, take_over)?;
    let target = sendable_target(call, schedules, &current.identity).await?;
    let send = current.redefined(target, current.content.clone(), sender, now_epoch_seconds());
    caller_current(call)?;
    // The sender is recorded while the schedule is still paused, so no fire
    // goes out as the earlier sender once it resumes, and at the revision the
    // resume read, so it never puts back a message changed meanwhile.
    update_at(
        &schedules.service,
        &schedule_id,
        paused.revision,
        UpdateScheduleRequest {
            target: Some(send.target().map_err(internal)?),
            ..UpdateScheduleRequest::default()
        },
    )
    .await?;
    #[cfg(test)]
    schedules.wait_at_resume_hold().await;
    let schedule = schedules
        .service
        .resume(&schedule_id)
        .await
        .map_err(schedule_error)?;
    // Core resumes the schedule as it is then: a change made between the
    // write above and the resume is the schedule's, so the answer names it.
    Ok(schedule_item(
        &schedules.service,
        &schedule,
        &as_left(&schedule, send),
        true,
    )
    .await)
}

async fn delete(
    call: &ScheduleCall<'_>,
    schedules: &AttachedConsoleSchedules,
    identity: &str,
) -> Result<Value, JsonRpcError> {
    refuse_unkept_preconditions(call.params, "meta/schedule/delete")?;
    let schedule_id = schedule_id_param(call.params)?;
    let (_, send) = console_schedule(call, schedules, identity, &schedule_id).await?;
    caller_current(call)?;
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

/// Refuse a write whose caller's login ended while it read: checked after
/// the write's last read, right before it writes.
fn caller_current(call: &ScheduleCall<'_>) -> Result<(), JsonRpcError> {
    call.governance
        .refuse_unless_caller_current()
        .map_err(ConsoleCallerRefusal::json_rpc_error)
}

/// The revision a change was made against (`expected_revision`, the
/// schedule's `revision` as its caller read it), when it names one.
fn expected_revision_param(params: &Value) -> Result<Option<ScheduleRevision>, JsonRpcError> {
    match params.get("expected_revision") {
        None | Some(Value::Null) => Ok(None),
        Some(revision) => revision
            .as_u64()
            .map(|revision| Some(ScheduleRevision(revision)))
            .ok_or_else(|| invalid("expected_revision must be a non-negative integer")),
    }
}

/// Whether a change confirms that it takes the schedule over from another
/// sender (`take_over_sender`).
fn take_over_sender_param(params: &Value) -> Result<bool, JsonRpcError> {
    match params.get("take_over_sender") {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(take_over)) => Ok(*take_over),
        Some(_) => Err(invalid("take_over_sender must be a boolean")),
    }
}

/// Refuse a precondition `method` would not keep: a pause and a delete act
/// on the schedule as it is (Core's pause and delete take no revision), and
/// neither they nor a create makes anyone the sender in another's place.
/// Named, `expected_revision` or `take_over_sender` would read as a
/// condition the call does not check, so it is refused, not ignored.
fn refuse_unkept_preconditions(params: &Value, method: &str) -> Result<(), JsonRpcError> {
    for param in ["expected_revision", "take_over_sender"] {
        if params.get(param).is_some_and(|value| !value.is_null()) {
            return Err(
                JsonRpcError::new(-32602, format!("{method} takes no {param}"))
                    .with_data(json!({ "kind": "unsupported_param", "param": param })),
            );
        }
    }
    Ok(())
}

/// The send `schedule` holds as a write left it, which is `send` unless a
/// change made meanwhile replaced it.
fn as_left(schedule: &Schedule, send: ConsoleScheduledSend) -> ConsoleScheduledSend {
    ConsoleScheduledSend::of_target(&schedule.target).unwrap_or(send)
}

/// Refuse a change made against a revision the schedule moved past: its
/// caller made it from a schedule that is no longer there, and writing it
/// would overwrite the change made meanwhile.
fn unchanged_since(
    expected: Option<ScheduleRevision>,
    read: &Schedule,
) -> Result<(), JsonRpcError> {
    match expected {
        Some(expected) if expected != read.revision => {
            Err(revision_conflict(expected, read.revision))
        }
        _ => Ok(()),
    }
}

/// Refuse a change that would make its caller the sender in another
/// caller's place unless it confirms the takeover: the schedule's later
/// sends would go out as the caller.
fn takeover_confirmed(
    current: &ConsoleScheduleSender,
    caller: &ConsoleScheduleSender,
    take_over: bool,
) -> Result<(), JsonRpcError> {
    if take_over || current.is_same_caller(caller) {
        return Ok(());
    }
    Err(JsonRpcError::new(
        -32009,
        "the console schedule sends as another caller; a change makes you its sender, so it \
         needs take_over_sender",
    )
    .with_data(json!({
        "kind": "sender_takeover_required",
        "sender": current.principal(),
    })))
}

/// The Console caller a create, update or resume records as the schedule's
/// sender: the caller as `/console/send` resolves it, with the principal the
/// access policy keys (a verified human, another authenticated principal, or
/// the open Console's shared account).
fn sender(call: &ScheduleCall<'_>) -> Result<ConsoleScheduleSender, JsonRpcError> {
    ConsoleScheduleSender::of_caller(
        &call.governance.scope,
        call.principal,
        call.governance
            .human
            .as_ref()
            .map(crate::runtime::AuthenticatedConsoleHuman::provider),
    )
    .ok_or_else(|| internal("the console could not resolve its caller"))
}

/// The agent a console schedule call names, as this console knows it.
struct ScheduleTarget {
    /// Its durable identity: what the schedule records.
    identity: String,
    /// Whether it takes human input (a console send).
    addressable: bool,
}

/// The agent `identity` names in the mob of the call, as the console's send
/// resolves it: the identity runtime's identity, else, for the host's
/// default mob, the console's identity record that names it by identity or
/// runtime member id. `None` when nothing does.
///
/// An agent is named by its mob and its name. The identity runtime is the
/// one of the call's mob. The console's records are read by name, and they
/// are the records of the console's own aggregator, which is built over the
/// host's default mob: they answer for a call for the default mob alone.
/// For another mob of the host only that mob's identity runtime answers,
/// and a name it does not know is no agent's.
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
    if call.default_mob.stored(&call.mob_id).is_some() {
        return Ok(None);
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
/// of the call's mob has the name, the agent takes no human input, its
/// sends are governed (a governed send needs its human present, and a fire
/// has none), or the agent is of a mob whose console schedules no fire
/// sends ([`refuse_a_mob_no_fire_reaches`]).
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
    refuse_a_mob_no_fire_reaches(call, &target.identity)?;
    Ok(target.identity)
}

/// Refuse a schedule for an agent of a mob whose console schedules no fire
/// sends. The Console's send path for the fires reaches the agents of the
/// host's default mob ([`ConsoleSendDoor`]): a schedule for an agent of
/// another mob would be stored, listed as active and never sent, so a
/// create, an update and a resume for one are refused, and say so. A list,
/// a pause and a delete stay. The refusal goes when the fires reach every
/// mob the host serves.
fn refuse_a_mob_no_fire_reaches(
    call: &ScheduleCall<'_>,
    identity: &str,
) -> Result<(), JsonRpcError> {
    match call.default_mob.stored(&call.mob_id) {
        None => Ok(()),
        Some(mob_id) => Err(schedule_fires_do_not_reach(mob_id, identity)),
    }
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
    // A schedule of an agent of the same name in another mob is another
    // agent's.
    if !send.is_for(&call.default_mob, &call.mob_id, identity) {
        let target = known_target(call, identity).await?;
        if target
            .is_none_or(|target| !send.is_for(&call.default_mob, &call.mob_id, &target.identity))
        {
            return Err(schedule_not_found());
        }
    }
    Ok((schedule, send))
}

/// One console schedule as the methods answer it. Who it sends as, and who
/// it sent as before each takeover, is shown only when `shows_sender`: to a
/// caller who may send to its agent (every write's caller does; a list
/// caller may only view the agent).
async fn schedule_item(
    service: &ScheduleService,
    schedule: &Schedule,
    send: &ConsoleScheduledSend,
    shows_sender: bool,
) -> Value {
    let next_due_at_utc = next_due_at_utc(service, schedule).await;
    let defined_at_utc = epoch_seconds_utc(send.defined_at);
    // Earlier senders are other callers' principals: shown with the sender,
    // to a caller who may send to the agent.
    let sender_history = if shows_sender {
        send.sender_history
            .iter()
            .map(|former| {
                json!({
                    "sender": former.sender.principal(),
                    "defined_at_utc": epoch_seconds_utc(former.defined_at),
                    "replaced_at_utc": epoch_seconds_utc(former.replaced_at),
                })
            })
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let mut item = json!({
        "schedule_id": schedule.schedule_id.to_string(),
        "identity": send.identity,
        "name": schedule.config.name,
        "content": send.content,
        "trigger": schedule.trigger,
        "phase": schedule.phase,
        "revision": schedule.revision.0,
        "sender": if shows_sender { send.sender.principal() } else { None },
        "sender_hidden": !shows_sender,
        "sender_history": sender_history,
        "defined_at_utc": defined_at_utc,
        "created_at_utc": schedule.config.created_at_utc,
        "updated_at_utc": schedule.config.updated_at_utc,
        "next_due_at_utc": next_due_at_utc,
    });
    // Whose pause it is, as the schedule's own lifecycle records it: said
    // only of a schedule paused because its agent takes no deliveries (its
    // agent is archived), never of an active schedule or of a pause
    // somebody asked for by name.
    if schedule.paused_for_target()
        && let Some(members) = item.as_object_mut()
    {
        members.insert("paused_for_target".to_string(), Value::Bool(true));
    }
    item
}

/// An instant held in whole seconds since the epoch, as UTC.
fn epoch_seconds_utc(seconds: u64) -> Option<chrono::DateTime<chrono::Utc>> {
    i64::try_from(seconds)
        .ok()
        .and_then(|seconds| chrono::DateTime::from_timestamp(seconds, 0))
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
    let trigger: TriggerSpec = match params.get("trigger") {
        None | Some(Value::Null) => return Ok(None),
        Some(trigger) => serde_json::from_value(trigger.clone())
            .map_err(|error| invalid(format!("invalid trigger: {error}")))?,
    };
    if let TriggerSpec::Interval(interval) = &trigger
        && interval.every_seconds < MIN_CONSOLE_INTERVAL_SECONDS
    {
        return Err(invalid(format!(
            "a console schedule repeats at most once every {MIN_CONSOLE_INTERVAL_SECONDS} \
             seconds"
        )));
    }
    Ok(Some(trigger))
}

fn schedule_id_param(params: &Value) -> Result<ScheduleId, JsonRpcError> {
    let raw = required_string(params, "schedule_id")?;
    ScheduleId::parse(&raw).map_err(|error| invalid(format!("invalid schedule_id: {error}")))
}

/// Write `request` to the schedule at `expected`, the revision the call
/// read. A schedule that moved on meanwhile is answered as a conflict and
/// left as it is: a write never overwrites a change it did not see.
pub(super) async fn update_at(
    service: &ScheduleService,
    schedule_id: &ScheduleId,
    expected: ScheduleRevision,
    request: UpdateScheduleRequest,
) -> Result<Schedule, JsonRpcError> {
    let request = UpdateScheduleRequest {
        expected_revision: Some(expected),
        ..request
    };
    match service.update(schedule_id, request).await {
        Ok(schedule) => Ok(schedule),
        // The service refuses a write at a stale revision as an invalid
        // schedule; one that moved on is the conflict.
        Err(error @ ScheduleDomainError::InvalidSchedule(_)) => {
            match service.get(schedule_id).await {
                Ok(now) if now.revision != expected => {
                    Err(revision_conflict(expected, now.revision))
                }
                _ => Err(schedule_error(error)),
            }
        }
        Err(error) => Err(schedule_error(error)),
    }
}

fn revision_conflict(expected: ScheduleRevision, actual: ScheduleRevision) -> JsonRpcError {
    JsonRpcError::new(
        -32009,
        "the console schedule changed meanwhile; read it again before changing it",
    )
    .with_data(json!({
        "kind": "schedule_revision_conflict",
        "expected_revision": expected.0,
        "actual_revision": actual.0,
    }))
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

/// The refusal of a schedule for `identity` of the mob `mob_id`, whose
/// console schedules no fire sends: the kind a schedule without a firing
/// host is refused with, since nothing would send this one either.
fn schedule_fires_do_not_reach(mob_id: &meerkat_mob::MobId, identity: &str) -> JsonRpcError {
    JsonRpcError::new(
        crate::rpc::CAPABILITY_UNAVAILABLE_CODE,
        format!(
            "this gateway sends the console schedules of its default mob's agents only, so a \
             console schedule for {identity} of mob {mob_id} would never be sent"
        ),
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
/// runtime's own non-owning back-edges (the mob's observer, never a public
/// owner). The runtime clears it when the console projection it holds is
/// replaced and at shutdown.
pub(crate) struct ConsoleSendDoor {
    pub(crate) decisions: RuntimeDecisionState,
    pub(crate) runtime: MobRuntime,
    pub(crate) aggregator: MetaConsoleAggregator,
    pub(crate) access: Option<AccessController>,
    pub(crate) identity_runtime: Option<Arc<crate::identity_first::IdentityRuntime>>,
    pub(crate) console_events: ConsoleEventStore,
    pub(crate) governed_ingress: crate::console_governed_ingress::ConsoleGovernedIngressSlot,
    pub(crate) session_fences: crate::auth::session_fence::ConsoleSessionFences,
}

#[async_trait::async_trait]
impl crate::console_schedules::ConsoleScheduleDoor for ConsoleSendDoor {
    /// As the Console's own configuration says.
    fn takes_a_persons_decision(&self) -> bool {
        crate::runtime::console_takes_a_persons_decision(&self.decisions, self.access.as_ref())
    }

    /// A fire is a `/console/send` by the schedule's sender, judged now. The
    /// Console first admits the sender as it admits a live request by it
    /// (`ConsoleScheduleSender::admitted_now`: the sign-in requirement, the
    /// trusted issuer, the provider policy and allowlist, the session
    /// fence), then sends exactly as `/console/send` does once its caller is
    /// admitted ([`send_as_admitted_console_caller`]), as the sender: the
    /// candidate-launch refusal, the stock send gate with `agent.send` for
    /// the sender under the live access policy, governance (a fire has no
    /// Console human present, so a governed target refuses it), and the
    /// identity-first send or the member lane, deduplicated under
    /// `delivery_key` in the sender's send scope, so a redelivered occurrence
    /// never sends twice.
    async fn send(&self, send: &ConsoleScheduledSend, delivery_key: &str) -> Result<(), String> {
        // This send path reaches the agents of its runtime's mob. A send to
        // an agent of another mob is never delivered to the agent of the
        // same name here. The door's runtime is the launch mob's wrapper
        // (the runtime installs the door from its default mob's entry), so
        // its mob is the host's default mob.
        let served = self.runtime.handle().mob_id().clone();
        let default_mob = crate::member_key::DefaultMob::new(served.clone());
        if send.mob(&default_mob) != served {
            return Err(format!(
                "the Console refused the scheduled send: its agent is a member of mob {}, \
                 which this Console send path does not reach",
                send.mob(&default_mob)
            ));
        }
        let sender = &send.sender;
        sender
            .admitted_now(&self.decisions, &self.session_fences, send.defined_at)
            .map_err(|refusal| format!("the Console refused the scheduled send: {refusal}"))?;
        // The sender's agent audience, as its live request would hold it:
        // a person owns the agents private to them.
        let private_owner_key = super::private_owner_key_of_send_scope(&sender.scope());
        let access_view = super::console_audience_view(
            self.access.as_ref(),
            sender.principal(),
            private_owner_key.as_deref(),
        );
        let governance = ConsoleSendGovernance {
            ingress: self.governed_ingress.current(),
            human: None,
            scope: sender.scope(),
            runtime_governed: self.runtime.native_work_governed(),
            access: self.access.clone(),
            verification: None,
            side_chat_owner: None,
            sender: Some(scheduled_sender(sender, &self.decisions)),
            served_mob: ServedMob::default(),
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
        let path = ConsoleSendPath {
            decisions: &self.decisions,
            runtime: Some(&self.runtime),
            aggregator: Some(&self.aggregator),
            access: self.access.as_ref(),
            identity_runtime: self.identity_runtime.as_ref(),
            console_events: Some(&self.console_events),
        };
        Box::pin(send_as_admitted_console_caller(
            &path,
            Some(&access_view),
            &governance,
            request,
        ))
        .await
        .map(|_accepted| ())
        .map_err(|error| scheduled_send_failure(&error))
    }
}

/// Who a fire sends as: the sender the schedule recorded, named as that
/// caller's live sends are named now. A person's fire on a console that
/// requires sign-in carries the person's qualified principal (its verified
/// `sub` in its issuer's trust domain) to the agent's run as its requester,
/// as the person's live send would; no other fire names one. A recorded
/// person whose issuer's tokens are now classified as shared accounts
/// (`shared_account_tokens`) is no person any more: a live token of theirs
/// names an account, so the fire names a token principal and no requester.
fn scheduled_sender(
    sender: &ConsoleScheduleSender,
    decisions: &RuntimeDecisionState,
) -> crate::console_aggregator::ConsoleSender {
    let kind = match sender {
        ConsoleScheduleSender::Anonymous => ConsoleActorKind::Anonymous,
        ConsoleScheduleSender::Human { .. } if decisions.trusted_oidc.shared_account_tokens => {
            ConsoleActorKind::TokenPrincipal
        }
        ConsoleScheduleSender::Human { .. } => ConsoleActorKind::Person,
        ConsoleScheduleSender::Principal { provider, .. }
            if provider == console_provider_name(&crate::AuthProvider::ServiceIdentity) =>
        {
            ConsoleActorKind::Service
        }
        ConsoleScheduleSender::Principal { .. } => ConsoleActorKind::TokenPrincipal,
    };
    let requester = match sender {
        ConsoleScheduleSender::Human {
            issuer, subject, ..
        } if kind == ConsoleActorKind::Person && console_sends_name_requesters(decisions) => {
            crate::runtime::console_human_principal(issuer, subject).ok()
        }
        ConsoleScheduleSender::Human { .. }
        | ConsoleScheduleSender::Anonymous
        | ConsoleScheduleSender::Principal { .. } => None,
    };
    crate::console_aggregator::ConsoleSender {
        kind,
        assurance: kind.assurance(),
        display_name: sender.principal().map(ToString::to_string),
        caller_scope: console_caller_scope(&sender.scope()),
        requester,
        // A schedule's fire is no human message: its turn carries no sender
        // label, although it may act for the schedule's person.
        model_label: None,
    }
}

/// The occurrence failure detail of a scheduled send the Console did not
/// accept. A send whose admission it could not observe (a timeout, a failed
/// probe) may still have been admitted: the detail says so, and a
/// redelivery under the same key would not send it twice.
fn scheduled_send_failure(error: &AdmittedConsoleSendError) -> String {
    let unobserved = matches!(
        error,
        AdmittedConsoleSendError::Send(
            ConsoleSendError::AdmissionTimeout { .. } | ConsoleSendError::ActorProbe(_)
        )
    );
    if unobserved {
        format!(
            "the Console could not observe whether the scheduled send was admitted, so it may \
             have been: {}",
            error.public_message()
        )
    } else {
        format!(
            "the Console refused the scheduled send: {}",
            error.public_message()
        )
    }
}
