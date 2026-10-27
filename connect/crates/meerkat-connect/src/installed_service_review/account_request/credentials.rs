//! Protected credential setup on the actual installed Console task.
//! Values are accepted only by the protected transport, never tool arguments.

use super::*;
use std::collections::BTreeSet;
use crate::service_install::{InstalledConnectorSelection, ServiceCredentialSlot};
use meerkat_core::McpServerConfig;

pub(crate) mod transport;
pub(crate) mod outcome;
#[cfg(all(unix, feature = "console"))]
pub(crate) mod administration;
#[cfg(all(unix, feature = "console"))]
mod driver;
use outcome::CredentialReply;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CredentialProposal {
    job_id: String,
    authentication: Authentication,
    operations: Vec<String>,
    agent: String,
    #[serde(deserialize_with = "explicit_lifetime")]
    expires_in_ms: Option<u64>,
    #[serde(default)]
    tool_policy: Option<ServiceToolPolicySelection>,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Authentication {
    DeclaredSecretSlots,
}

#[derive(Serialize)]
struct CredentialSelection {
    operations: BTreeSet<String>,
    agent: String,
    expires_in_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_policy: Option<ServiceToolPolicySelection>,
}

fn parse(raw: &str) -> Result<CredentialProposal, ToolError> {
    if raw.len() > 32 * 1024 { return Err(invalid()); }
    serde_json::from_str::<crate::forge::UniqueJson>(raw).map_err(|_| invalid())?;
    let value: Value = serde_json::from_str(raw).map_err(|_| invalid())?;
    if value.get("expires_in_ms").is_none() { return Err(invalid()); }
    let proposal: CredentialProposal = serde_json::from_value(value).map_err(|_| invalid())?;
    let bounded = |value: &str| !value.is_empty() && value.len() <= 2048
        && !value.chars().any(char::is_control);
    if !bounded(&proposal.job_id) || !bounded(&proposal.agent)
        || proposal.expires_in_ms == Some(0) || proposal.operations.is_empty()
        || proposal.operations.len() > 128
        || proposal.operations.iter().any(|name| name.is_empty() || name.len() > 128
            || !name.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte)))
        || proposal.operations.iter().collect::<BTreeSet<_>>().len() != proposal.operations.len()
    { return Err(invalid()); }
    Ok(proposal)
}

fn invalid() -> ToolError {
    ToolError::InvalidArguments {
        name: SETUP_ACCOUNT_REQUEST.into(),
        reason: "Provide one exact reconciled credentialed job, declared_secret_slots authentication, raw operations, intended consumer agent and explicit lifetime. Commands, realm selections, slot names and credential values are not accepted.".into(),
    }
}

fn checked_configuration(selected: &InstalledConnectorSelection, selection: &CredentialSelection)
    -> Result<(McpServerConfig, Vec<ServiceCredentialSlot>), ConnectError>
{
    let config = selected.secret_configuration(&selection.operations)
        .map_err(|_| refused("The checked credential configuration is unavailable"))?;
    let slots = selected.secret_slots()
        .map_err(|_| refused("The checked credential declarations are unavailable"))?;
    let names = slots.iter().map(ServiceCredentialSlot::environment_variable).collect::<BTreeSet<_>>();
    if slots.is_empty() || slots.len() > 256 || names.len() != slots.len() {
        return Err(refused("The checked credential declarations are unavailable"));
    }
    Ok((config, slots))
}

fn credential_review_request(selected: &InstalledConnectorSelection, config: &McpServerConfig,
    selection: &CredentialSelection,
) -> Result<ApprovalRequest, ConnectError> {
    let (checked, slots) = checked_configuration(selected, selection)?;
    if checked != *config {
        return Err(refused("The checked credential configuration changed"));
    }
    require_console_tool_policy(SETUP_ACCOUNT_REQUEST, config, &selection.operations, selection.tool_policy.as_ref())
        .map_err(|error| refused(error.to_string()))?;
    let software = &selected.installed;
    let body = json!({
        "phase": "installed_credential_service_access",
        "realm": software.realm.realm.as_str(),
        "realm_state_root": software.realm.state_root,
        "session": software.session,
        "software": {"job_id": software.job_id, "family": software.family,
            "profile": software.profile, "server_name": software.server_name,
            "validation_id": software.validation_id,
            "receipt_sha256": software.receipt_sha256,
            "configuration_sha256": software.configuration_sha256,
            "mcp_path": software.mcp_path, "receipt_path": software.receipt_path,
            "document_sha256": software.mcp_document_sha256,
            "recovery_request_sha256": software.recovery_request_sha256},
        "credential_slots": slots,
        "authentication": {"kind": "declared_secret_slots"},
        "operation_basis": selected.operation_basis(),
        "installation": config,
        "application_selection": selection,
        "notice": "Approval covers saving this exact connector configuration in the host's defining realm, storing the declared credentials through its native secret owner, and then retaining the reviewed fresh instruction and task-scoped access. Saved configuration or credentials can remain if a later step fails or setup is cancelled. Credential values are entered only in the protected form and are not part of this record. Saving credentials does not verify a provider account. Native attachment and observed task completion remain separate."
    });
    Ok(ApprovalRequest {
        requester: ApprovalPrincipalId::new("connect-installed-setup-agent")
            .map_err(|_| refused("The installed requester is unavailable"))?,
        owner: ApprovalOwnerRef::Session { session_id: software.session.clone() },
        resource: ApprovalResourceRef { kind: ApprovalResourceKind::Runtime,
            id: ApprovalResourceId::new("selected-installed-credential-service") },
        proposed_action: ApprovalProposedAction { kind: ApprovalActionKind::Other,
            summary: "Save this service's configuration and credentials, then enable the reviewed task".into(),
            body: Some(body.clone()) },
        risk: ApprovalRisk::High,
        request_body: Some(body),
        allowed_decisions: [ApprovalDecision::Approve, ApprovalDecision::Deny].into(),
        expires_at: Some((SystemTime::now() + std::time::Duration::from_secs(120)).into()),
        metadata: Default::default(), request_provenance: None,
    })
}

pub(super) fn proposal_schema() -> Value {
    json!({"type":"object","additionalProperties":false,
        "required":["job_id","authentication","operations","agent","expires_in_ms"],
        "properties":{
            "job_id":{"type":"string"},
            "authentication":{"type":"object","additionalProperties":false,
                "required":["kind"],"properties":{"kind":{"const":"declared_secret_slots"}}},
            "operations":{"type":"array","minItems":1,"maxItems":128,"uniqueItems":true,
                "items":{"type":"string","minLength":1,"maxLength":128}},
            "agent":consumer_agent_schema(),
            "expires_in_ms":{"type":["integer","null"],"minimum":1},
            "tool_policy":tool_policy_schema(),
        }})
}

/// This request comes only from the native borrowed setup invocation. Neither
/// the protected body nor tool arguments can select its task or native entry.
pub(crate) struct InstalledCredentialRequest {
    selected: InstalledConnectorSelection,
    config: McpServerConfig,
    slots: Vec<ServiceCredentialSlot>,
    selection: CredentialSelection,
    task: Option<toolkit_mobkit::private_originals::InstalledServiceTask>,
    entry: ReviewedEntryTicket,
    current: CurrentRequest,
    response: oneshot::Sender<CredentialReply>,
}
impl InstalledCredentialRequest {
    pub(crate) fn console_task(&self) -> Option<&toolkit_mobkit::private_originals::InstalledServiceTask> {
        self.task.as_ref().filter(|task| task.console_login().is_some())
    }
    pub(crate) fn credential_delivery(&self) -> Result<(transport::CredentialDelivery,
        oneshot::Receiver<transport::CredentialValues>), ConnectError> {
        transport::CredentialDelivery::channel(self.slots.clone())
    }
    pub(crate) async fn closed(&mut self) { self.response.closed().await; }
}

/// Select a closed alternative without accepting either alternative's fields.
/// The full parser below still rejects mixed, duplicate and unknown fields.
pub(super) fn is_proposal(raw: &str) -> Result<bool, ToolError> {
    if raw.len() > 32 * 1024 { return Err(invalid()); }
    let value: Value = serde_json::from_str(raw).map_err(|_| invalid())?;
    Ok(value.get("authentication").is_some())
}

pub(super) async fn dispatch(owner: &RequestTool, call: ToolCallView<'_>,
    context: &ToolDispatchContext, plan: Option<&ResolvedToolExecutionPlan>,
) -> Result<ToolDispatchOutcome, ToolError> {
    if context.origin_session_id() != Some(&owner.session)
        || context.work_authorization().is_none() || context.retained_work().is_none()
    { return Err(ToolError::access_denied(call.name)); }
    let mut current = context.check_tool_authorization(call, plan)?.unwrap_or_else(|| context.clone());
    let proposal = parse(call.args.get())?;
    let selected = owner.checked.connector_selection(&owner.realm, &owner.session, &proposal.job_id)
        .map_err(|_| unavailable())?;
    let selection = CredentialSelection { operations: proposal.operations.into_iter().collect(),
        agent: proposal.agent, expires_in_ms: proposal.expires_in_ms, tool_policy: proposal.tool_policy };
    let permitted = selected.secret_operations().map_err(|_| unavailable())?;
    if !selection.operations.is_subset(&permitted) {
        let names = serde_json::to_string(&permitted).map_err(|_| unavailable())?;
        return Err(ToolError::InvalidArguments { name: call.name.into(), reason: format!(
            "operations must contain exact raw MCP tool names from the current checked selection. Use operation_tools values, not declaration IDs from its keys. Permitted raw names: {names}. Select only the operations needed for this task, keeping the same job, agent and requested lifetime. This request has not entered access review."
        ) });
    }
    let (config, slots) = checked_configuration(&selected, &selection).map_err(|_| unavailable())?;
    let prepared = match &owner.tasks {
        Some(tasks) => tasks.prepare_captured_consumer(call, &current, plan).await,
        None => Ok(None),
    };
    if let Some(refreshed) = current.check_tool_authorization(call, plan)? { current = refreshed; }
    let task = prepared.map_err(|error| match error {
        toolkit_mobkit::private_originals::PrivateOriginalError::Unavailable => unavailable(),
        toolkit_mobkit::private_originals::PrivateOriginalError::Denied
        | toolkit_mobkit::private_originals::PrivateOriginalError::Conflict => ToolError::access_denied(call.name),
    })?.ok_or_else(|| ToolError::access_denied(call.name))?;
    require_captured_consumer(call.name, &selection.agent, &task)?;
    // This alternative has no application-policy or terminal fallback. The
    // existing task grant permits the request, never realm administration.
    if task.console_login().is_none()
        || checked_configuration(&selected, &selection).ok().as_ref().map(|value| &value.0) != Some(&config)
    { return Err(ToolError::access_denied(call.name)); }
    require_console_tool_policy(call.name, &config, &selection.operations, selection.tool_policy.as_ref())?;
    let entry = current.reviewed_entry_ticket(call, plan)?;
    let (checks, mut requests) = mpsc::channel::<oneshot::Sender<bool>>(1);
    let (response, receive) = oneshot::channel();
    owner.credential_requests.as_ref().ok_or_else(unavailable)?.try_send(InstalledCredentialRequest {
        selected, config, slots, selection, task: Some(task), entry,
        current: CurrentRequest(checks), response,
    }).map_err(|_| unavailable())?;
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

#[cfg(test)]
mod tests;
