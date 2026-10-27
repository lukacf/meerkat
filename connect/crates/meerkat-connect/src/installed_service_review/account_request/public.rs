//! Public-service review on the existing installed helper and waiting task.
//! No account is invented and no standalone realm finalizer is composed here.

use super::*;
use crate::service_install::{ConnectorSelectionError, PublicServiceSelection};
use crate::approval::ApprovalPresenter;
use crate::stdio::PromptBridge;
use std::{collections::BTreeSet, future::Future, sync::atomic::{AtomicBool, Ordering}, time::Duration};
use meerkat_core::McpServerConfig;
use meerkat_toolkit::setup::SETUP_PUBLIC_SERVICE_REQUEST;
use toolkit_mobkit::private_originals::InstalledServiceTask;
mod batch;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PublicSelection {
    operations: BTreeSet<String>,
    agent: String,
    #[serde(deserialize_with = "explicit_lifetime")]
    expires_in_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tool_policy: Option<ServiceToolPolicySelection>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PublicProposal {
    job_id: String,
    operations: Vec<String>,
    agent: String,
    #[serde(deserialize_with = "explicit_lifetime")]
    expires_in_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tool_policy: Option<ServiceToolPolicySelection>,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum PublicState { Reviewed, Denied }

#[derive(Serialize)]
struct PublicReply {
    state: PublicState,
    access_reviewed: bool,
    grant_issued: bool,
    fresh_instruction_retained: bool,
    ready: bool,
}

/// Custody of this actual native invocation, separate from account sign-in.
/// The selected task and login cannot be decoded from model arguments.
pub(crate) enum InstalledPublicRequest {
    Single(InstalledPublicSingleRequest),
    Batch(batch::InstalledPublicBatchRequest),
}
pub(crate) struct InstalledPublicSingleRequest {
    selected: PublicServiceSelection,
    config: McpServerConfig,
    selection: PublicSelection,
    task: Option<InstalledServiceTask>,
    entry: ReviewedEntryTicket,
    current: CurrentRequest,
    response: oneshot::Sender<PublicReply>,
}
impl InstalledPublicRequest {
    pub(crate) fn console_task(&self) -> Option<&InstalledServiceTask> {
        let task = match self { Self::Single(value) => value.task.as_ref(), Self::Batch(value) => value.task.as_ref() };
        task.filter(|task| task.console_login().is_some())
    }
    pub(crate) async fn closed(&mut self) {
        match self { Self::Single(value) => value.response.closed().await, Self::Batch(value) => value.response.closed().await }
    }
}
impl InstalledPublicSingleRequest {
    fn console_task(&self) -> Option<&InstalledServiceTask> {
        self.task.as_ref().filter(|task| task.console_login().is_some())
    }
}

fn unavailable() -> ToolError {
    ToolError::unavailable(SETUP_PUBLIC_SERVICE_REQUEST,
        meerkat_core::ToolUnavailableReason::TemporarilyUnavailable)
}

/// Shared singleton/batch selection feedback, after the caller's native check.
/// This only projects the existing owner's refusal; it performs no setup work.
pub(crate) fn select_public_job(
    checked: &Arc<CheckedInstalls>, realm: &RealmLocator, session: &SessionId, job_id: &str,
) -> Result<PublicServiceSelection, ToolError> {
    checked.public_selection(realm, session, job_id).map_err(|error| {
        let job = json!(job_id);
        let reason = match error {
            ConnectorSelectionError::Unconfirmed => format!(
                "Job {job} has no current reconciled selection in this helper session. If this is an existing job from this setup session, inspect it with setup_job using the same job_id and call setup_reconcile when reconciliation is available. A succeeded job alone does not confirm its retained receipt. Do not reinstall to remedy this refusal. Unknown, foreign or stale jobs remain refused. This request has not entered access review."),
            ConnectorSelectionError::MetadataRequired { family, profile } => {
                let help = json!({"family": family, "profile": profile});
                format!(
                    "Job {job} needs current library operation metadata. Call setup_help with {help} after the current setup_check; help read before that check is not retained for this selection. Then retry the public request with the same job, exact raw operation names, intended consumer agent and requested lifetime. Do not reinstall. This request has not entered access review.")
            }
        };
        ToolError::InvalidArguments { name: SETUP_PUBLIC_SERVICE_REQUEST.into(), reason }
    })
}

fn invalid() -> ToolError {
    ToolError::InvalidArguments { name: SETUP_PUBLIC_SERVICE_REQUEST.into(),
        reason: "Provide one exact reconciled public job, raw operations, intended consumer agent and explicit lifetime. Commands, credentials, accounts and authority are not accepted.".into() }
}
fn parse(raw: &str) -> Result<PublicProposal, ToolError> {
    if raw.len() > 32 * 1024 { return Err(invalid()); }
    serde_json::from_str::<crate::forge::UniqueJson>(raw).map_err(|_| invalid())?;
    let value: Value = serde_json::from_str(raw).map_err(|_| invalid())?;
    if value.get("expires_in_ms").is_none() { return Err(invalid()); }
    let proposal: PublicProposal = serde_json::from_value(value).map_err(|_| invalid())?;
    let bounded = |text: &str| !text.is_empty() && text.len() <= 2048 && !text.chars().any(char::is_control);
    if !bounded(&proposal.job_id) || !bounded(&proposal.agent) || proposal.expires_in_ms == Some(0)
        || proposal.operations.is_empty() || proposal.operations.len() > 128
        || proposal.operations.iter().any(|name| name.is_empty() || name.len() > 128
            || !name.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte)))
        || proposal.operations.iter().collect::<BTreeSet<_>>().len() != proposal.operations.len()
    { return Err(invalid()); }
    Ok(proposal)
}

pub(super) async fn dispatch(owner: &RequestTool, call: ToolCallView<'_>,
    context: &ToolDispatchContext, plan: Option<&ResolvedToolExecutionPlan>,
) -> Result<ToolDispatchOutcome, ToolError> {
    if serde_json::from_str::<Value>(call.args.get()).ok().is_some_and(|value| value.get("services").is_some()) {
        return batch::dispatch(owner, call, context, plan).await;
    }
    if context.origin_session_id() != Some(&owner.session)
        || context.work_authorization().is_none() || context.retained_work().is_none()
    { return Err(ToolError::access_denied(call.name)); }
    let mut current = context.check_tool_authorization(call, plan)?.unwrap_or_else(|| context.clone());
    let proposal = parse(call.args.get())?;
    let selected = select_public_job(&owner.checked, &owner.realm, &owner.session, &proposal.job_id)?;
    let selection = PublicSelection { operations: proposal.operations.into_iter().collect(),
        agent: proposal.agent, expires_in_ms: proposal.expires_in_ms, tool_policy: proposal.tool_policy };
    let permitted = selected.public_operations().map_err(|_| unavailable())?;
    if !selection.operations.is_subset(&permitted) {
        let names = serde_json::to_string(&permitted).map_err(|_| unavailable())?;
        return Err(ToolError::InvalidArguments { name: call.name.into(), reason: format!(
            "operations must contain exact raw MCP tool names from the current checked selection. Use operation_tools values, not declaration IDs from its keys. Permitted raw names: {names}. Select only the operations needed for this task, keeping the same job, agent and requested lifetime. This request has not entered access review.") });
    }
    let config = selected.public_configuration(&selection.operations).map_err(|_| unavailable())?;
    let prepared = match &owner.tasks {
        Some(tasks) => tasks.prepare_public_service(call, &current, plan).await,
        None => Ok(None),
    };
    if let Some(refreshed) = current.check_tool_authorization(call, plan)? { current = refreshed; }
    let task = prepared.map_err(|error| match error {
        toolkit_mobkit::private_originals::PrivateOriginalError::Unavailable =>
            ToolError::unavailable(call.name, meerkat_core::ToolUnavailableReason::TemporarilyUnavailable),
        toolkit_mobkit::private_originals::PrivateOriginalError::Denied
        | toolkit_mobkit::private_originals::PrivateOriginalError::Conflict => ToolError::access_denied(call.name),
    })?
        .ok_or_else(|| ToolError::access_denied(call.name))?;
    require_captured_consumer(call.name, &selection.agent, &task)?;
    if let Some(application) = &owner.application {
        // Bind the already-checked product selection, independently of the
        // application policy decision. No human ApprovalRecord is fabricated.
        let application_task = task.clone().with_installation_readback_source(&config,
            public_review_body(&selected, &config, &selection).map_err(|_| unavailable())?)
            .map_err(|_| unavailable())?;
        if let Some(result) = super::application::public(owner, application.clone(), call, &current, plan,
            &application_task, &selected, &config, &selection.operations, selection.expires_in_ms, selection.tool_policy.clone()).await? { return Ok(result); }
        if let Some(refreshed) = current.check_tool_authorization(call, plan)? { current = refreshed; }
    }
    // This public entry serves the integrated Console. A local installed host
    // without that retained human uses its standalone Connect setup surface.
    if task.console_login().is_none() || selected.public_configuration(&selection.operations).ok().as_ref() != Some(&config) {
        return Err(ToolError::access_denied(call.name));
    }
    require_console_tool_policy(call.name, &config, &selection.operations, selection.tool_policy.as_ref())?;
    let entry = current.reviewed_entry_ticket(call, plan)?;
    let (checks, mut requests) = mpsc::channel::<oneshot::Sender<bool>>(1);
    let (response, receive) = oneshot::channel();
    owner.public_requests.as_ref().ok_or_else(unavailable)?.try_send(InstalledPublicRequest::Single(InstalledPublicSingleRequest { selected, config, selection,
        task: Some(task), entry, current: CurrentRequest(checks), response })).map_err(|_| unavailable())?;
    tokio::pin!(receive);
    let response = loop {
        tokio::select! { biased;
            request = requests.recv() => {
                let Some(request) = request else { break receive.await; };
                match current.check_tool_authorization(call, plan) {
                    Ok(refreshed) => { if let Some(refreshed) = refreshed { current = refreshed; } let _ = request.send(true); }
                    Err(error) => { let _ = request.send(false); return Err(error); }
                }
            }
            response = receive.as_mut() => break response,
        }
    };
    current.check_tool_authorization(call, plan)?;
    let reply = response.map_err(|_| unavailable())?;
    serde_json::to_string(&reply).map(|body| ToolResult::new(call.id.into(), body, false).into())
        .map_err(|_| unavailable())
}

pub fn installed_public_service_request_definition() -> ToolDef {
    ToolDef::new(SETUP_PUBLIC_SERVICE_REQUEST,
        "Request public service access for the exact waiting task. The selected host uses its application policy or protected Console review. A requires_action result names needed human access review and is not success. Reconcile the installed package or retained remote HTTP export job first. Requires explicit authentication kind none and exact raw names from current library publisher metadata, the selected package canonical manifest, or the retained remote recipe declaration; absence of a native connector is insufficient. Use operation_tools values as raw names, not its declaration-ID keys. For one service supply job_id and raw operations. In an integrated Console, for 2-8 public services needed by this one acquisition, supply services as an array of exact {job_id, operations} members instead. The services form is Console-only in this version. The set must have distinct MCP server targets; protected Console reviews the complete set, and one continuation enters only after every attachment is usable. When the host has selected a current configured setup capability, this same review also shows fresh task-specific setup authority and the exact metadata scope for both agents and sessions. That optional warm path lets the continued task request another public integration through the same live helper; each later service set needs a new access review and fresh grants, including any earlier connection selected again. Without that capability, this continuation cannot request another integration. This does not enable authenticated-service carry-forward or helper reset. Supply intended consumer agent and explicit expires_in_ms (null leaves the task deadline as the limit). If trusted application delegation does not handle the request, protected Console review requires explicit tool_policy with a non-null catalog consequence and coverage of every selected exposed tool name. These are proposals for the same protected review, not authority. Authorized access retains a new instruction and a task-scoped live attachment grant through the existing original-task owner. It never saves realm configuration, creates an account, invokes the service or establishes readiness. A separate public-request capability is required.",
        { let mut singleton = json!({"type":"object","additionalProperties":false,"required":["agent","expires_in_ms"],
            "properties":{"job_id":{"type":"string"},"agent":consumer_agent_schema(),
                "expires_in_ms":{"type":["integer","null"],"minimum":1},
                "tool_policy":tool_policy_schema(),
                "operations":{"type":"array","description":"Exact raw MCP tool names: operation_tools values, not declaration-ID keys. Select only the needed subset.","minItems":1,"maxItems":128,"uniqueItems":true,
                    "items":{"type":"string","minLength":1,"maxLength":128}}}});
          let services = json!({"type":"array","description":"Console-only: for a multi-service acquisition, supply this instead of top-level job_id and operations. The strict parser requires exactly one form and a complete tool_policy.","minItems":2,"maxItems":8,
              "items":{"type":"object","additionalProperties":false,"required":["job_id","operations"],
                  "properties":{"job_id":{"type":"string"},"operations":singleton["properties"]["operations"]}}});
          singleton["properties"]["services"] = services;
          singleton })
}

impl InstalledAccountRequests {
    /// Same enrollment, prompt transport and native entry custody as account
    /// requests, with no account state or callback work on this branch.
    pub(crate) async fn present_public_console<F: Future<Output = ()>>(
        &mut self, request: InstalledPublicRequest,
        presenter: &mut Option<ApprovalPresenter>, bridge: PromptBridge,
        refresh: &console::Refresh<'_>, decision: &console::CaptureDecision<'_>, closed: F,
    ) -> Result<bool, ConnectError> {
        match request {
            InstalledPublicRequest::Single(request) => self.present_public_single_console(request, presenter, bridge, refresh, decision, closed).await,
            InstalledPublicRequest::Batch(request) => self.present_public_batch_console(request, presenter, bridge, refresh, decision, closed).await,
        }
    }

    async fn present_public_single_console<F: Future<Output = ()>>(
        &mut self, mut request: InstalledPublicSingleRequest,
        presenter: &mut Option<ApprovalPresenter>, bridge: PromptBridge,
        refresh: &console::Refresh<'_>, decision: &console::CaptureDecision<'_>, closed: F,
    ) -> Result<bool, ConnectError> {
        let owner_closed = AtomicBool::new(false);
        let result = async {
            if request.response.is_closed() { return Ok(true); }
            let login = request.console_task().and_then(|task| task.console_login()).cloned()
                .ok_or_else(|| refused("The public service request has no retained Console login"))?;
            let reservation = login.reserve().map_err(|_| refused("The Console login refused public service entry"))?;
            let checked_refresh = || -> futures_util::future::BoxFuture<'static, Result<(), ConnectError>> {
                let login = login.clone();
                let current = login.check().map_err(|_| refused("The Console login is unavailable"));
                let refreshed = current.is_ok().then(|| refresh());
                Box::pin(async move {
                    current?;
                    refreshed.expect("a current login selected refresh").await?;
                    login.check().map_err(|_| refused("The Console login is unavailable"))?;
                    Ok(())
                })
            };
            let refresh: &console::Refresh<'_> = &checked_refresh;
            let result = async {
                let closing = async {
                    tokio::select! { biased;
                        _ = closed => { owner_closed.store(true, Ordering::SeqCst); },
                        _ = login.closed() => {},
                    }
                };
                tokio::pin!(closing);
                if !console::checkpoint(&request.current, &mut request.response, &bridge, refresh, closing.as_mut()).await? {
                    return Ok(request.response.is_closed());
                }
                if request.selected.public_configuration(&request.selection.operations).ok().as_ref() != Some(&request.config) {
                    return Err(refused("The selected public installation changed"));
                }
                let custody = request.entry.enter().map_err(|_| refused("The public service request cannot enter"))?;
                let (cancel_review, cancellation) = ReviewCancellation::channel();
                let task = request.task.take().ok_or_else(|| refused("The waiting task is unavailable"))?;
                let reviewed = console::drive_review_cancellable(
                    self.review.review_public_with_task(&request.selected, &request.config, &request.selection,
                        task, &request.current, refresh, decision, cancellation),
                    &self.review.approvals, presenter, &bridge, refresh, &mut request.response,
                    closing.as_mut(), Some(&login), Some(cancel_review), Some(&owner_closed),
                ).await?;
                let Some(reply) = reviewed else { return Ok(request.response.is_closed()); };
                if !console::checkpoint(&request.current, &mut request.response, &bridge, refresh, closing.as_mut()).await? {
                    return Ok(request.response.is_closed());
                }
                if request.selected.public_configuration(&request.selection.operations).ok().as_ref() != Some(&request.config) {
                    return Err(refused("The public installation changed after review"));
                }
                let _ = request.response.send(reply);
                drop(custody);
                Ok(true)
            }.await;
            if result.is_ok() { reservation.settled(); }
            result
        }.await;
        if owner_closed.load(Ordering::SeqCst) || presenter.is_none() {
            let retired = self.review.retire();
            drop(presenter.take());
            return match result {
                Ok(_) => retired.map(|()| false),
                Err(error) => present::join_cleanup(Err(error), retired).map(|()| false),
            };
        }
        // Losing one request or login does not close the host enrollment.
        result.map(|_| true)
    }
}

impl InstalledServiceReview {
    async fn review_public_with_task(
        &self, selected: &PublicServiceSelection, config: &McpServerConfig,
        selection: &PublicSelection, task: InstalledServiceTask, current: &CurrentRequest,
        refresh: &console::Refresh<'_>, capture_decision: &console::CaptureDecision<'_>,
        cancellation: ReviewCancellation,
    ) -> Result<PublicReply, ConnectError> {
        let check = || -> Result<(), ConnectError> {
            self.approvals.check_session(&self.session)?;
            cancellation.check()?;
            if selected.public_configuration(&selection.operations).ok().as_ref() != Some(config) {
                return Err(refused("The reviewed public installation changed"));
            }
            Ok(())
        };
        check()?;
        refresh().await?;
        if !current.check().await { return Err(refused("The public service request is no longer current")); }
        check()?;
        let request = public_review_request(selected, config, selection)?;
        let operations = selection.operations.iter().cloned().map(toolkit_package::Id::new)
            .collect::<Result<BTreeSet<_>, _>>().map_err(|_| refused("The public operations are invalid"))?;
        let task = task.prepare_public_instruction(config, operations, selection.expires_in_ms, request, selection.tool_policy.clone())
            .await.map_err(|_| refused("The waiting task cannot be offered for public service approval"))?;
        check()?;
        let record = self.approvals.review_access_cancellable(task.request().clone(),
            cancellation.cancelled(), |id| cancellation.approval_created(id)).await?;
        let decision_current = capture_decision()?;
        check()?;
        if record.status == ApprovalStatus::Denied {
            return Ok(PublicReply { state: PublicState::Denied, access_reviewed: true,
                grant_issued: false, fresh_instruction_retained: false, ready: false });
        }
        if record.status != ApprovalStatus::Approved { return Err(refused("The public review has no matching decision")); }
        if !current.check().await { return Err(refused("The public service request is no longer current")); }
        refresh().await?;
        check()?;
        // Native publication retains this predicate beyond the borrowed review.
        // Clones retain the same checked-install owner and exact evidence; each
        // callback still resolves its current selection instead of trusting it.
        let publication_selection = selected.clone();
        let publication_config = config.clone();
        let publication_operations = selection.operations.clone();
        let publication_approvals = self.approvals.clone();
        let publication_session = self.session.clone();
        let publication_cancellation = cancellation.clone();
        let retained = task.approve(record, move || {
            publication_approvals.check_session(&publication_session).is_ok()
                && publication_cancellation.check().is_ok()
                && publication_selection.public_configuration(&publication_operations).ok().as_ref()
                    == Some(&publication_config)
                && decision_current()
        }).await.map_err(|_| refused("The approved public instruction could not be retained"))?;
        check()?;
        Ok(PublicReply { state: PublicState::Reviewed, access_reviewed: true,
            grant_issued: retained, fresh_instruction_retained: retained, ready: false })
    }
}

fn public_review_body(selected: &PublicServiceSelection, config: &McpServerConfig,
    selection: &PublicSelection,
) -> Result<Value, ConnectError> {
    if selected.public_configuration(&selection.operations).ok().as_ref() != Some(config) {
        return Err(refused("The exact public package selection is unavailable"));
    }
    let mut body = selected.review_source();
    let fields = json!({"phase":"installed_public_service_access",
        "realm":selected.realm().realm.as_str(),"realm_state_root":selected.realm().state_root,"session":selected.session(),
        "authentication":{"kind":"none"},"operation_basis":selected.operation_basis(),
        "installation":config,"application_selection":selection,
        "notice":"The selected source declares that this service requires no sign-in. This is a researched declaration, not proof of anonymous execution or an observed account. Approval authorizes the reviewed fresh instruction and task-scoped server policy. The existing source and native owners must still verify, attach and observe readiness. It does not promise continuing permission for one fixed endpoint or account. Realm configuration is not changed."});
    body.as_object_mut().expect("source projection is an object").extend(fields.as_object().unwrap().clone());
    Ok(body)
}

fn public_review_request(selected: &PublicServiceSelection, config: &McpServerConfig,
    selection: &PublicSelection,
) -> Result<ApprovalRequest, ConnectError> {
    let body = public_review_body(selected, config, selection)?;
    require_console_tool_policy(SETUP_PUBLIC_SERVICE_REQUEST, config, &selection.operations, selection.tool_policy.as_ref())
        .map_err(|error| refused(error.to_string()))?;
    Ok(ApprovalRequest {
        requester: ApprovalPrincipalId::new("connect-installed-setup-agent").map_err(|error| refused(error.to_string()))?,
        owner: ApprovalOwnerRef::Session { session_id: selected.session().clone() },
        resource: ApprovalResourceRef { kind: ApprovalResourceKind::Runtime,
            id: ApprovalResourceId::new("selected-installed-public-service") },
        proposed_action: ApprovalProposedAction { kind: ApprovalActionKind::Other,
            summary: "Review public service access for this task".into(), body: Some(body.clone()) },
        risk: ApprovalRisk::Medium, request_body: Some(body),
        allowed_decisions: [ApprovalDecision::Approve, ApprovalDecision::Deny].into(),
        expires_at: Some((SystemTime::now() + Duration::from_secs(120)).into()),
        metadata: Default::default(), request_provenance: None,
    })
}

#[cfg(test)]
mod tests;
