//! One native-registered request into the existing installed account owner.
//! Only the retained private presenter receives login material.

use super::*;
use async_trait::async_trait;
use meerkat_core::{
    ApprovalId, DynamicToolComposite, RegisteredToolBundleDispatcher, ResolvedToolExecutionPlan,
    ReviewedEntryTicket, ToolCallView, ToolCatalogCapabilities, ToolDef, ToolDispatchContext,
    ToolError, ToolMutationClass, ToolResult, ops::ToolDispatchOutcome,
};
use meerkat_toolkit::setup::{SETUP_ACCOUNT_REQUEST, SETUP_ACCOUNTS_BUNDLE, SETUP_PUBLIC_SERVICE_REQUEST};
use serde::Deserialize;
use toolkit_mobkit::private_originals::ServiceToolPolicySelection;
use tokio::sync::{mpsc, oneshot};

mod present;
pub(crate) mod public;
pub(crate) mod credentials;
pub(crate) mod console;
mod application;
pub(crate) use application::compose_application_policy_requests;
#[cfg(all(unix, feature = "console"))]
pub(crate) use application::compose_selected_policy_requests;
#[cfg(test)]
mod tests;

/// Closed proposal data. Native principals, route, realm, original binding,
/// credential values and a connection revision cannot be supplied here.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Proposal {
    job_id: String,
    connector: Descriptor,
    operations: BTreeMap<String, Operation>,
    agent: String,
    // A missing lifetime must not silently become until-revoked access.
    #[serde(deserialize_with = "explicit_lifetime")]
    expires_in_ms: Option<u64>,
    #[serde(default)]
    tool_policy: Option<ServiceToolPolicySelection>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Descriptor {
    slot_id: String,
    issuer: String,
    client: String,
    resource: String,
    scopes: std::collections::BTreeSet<String>,
    strategy_id: String,
    #[serde(deserialize_with = "explicit_account")]
    account_selection: meerkat_auth_core::connector_oauth::AccountSelection,
    #[serde(default, deserialize_with = "profile_name")]
    client_auth_profile: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Operation {
    required_scopes: std::collections::BTreeSet<String>,
    destination_origin: String,
}
impl From<Operation> for McpConnectorOperation {
    fn from(operation: Operation) -> Self {
        Self {
            required_scopes: operation.required_scopes,
            destination_origin: operation.destination_origin,
            read_only: false,
        }
    }
}
fn profile_name<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    String::deserialize(d).map(Some)
}

fn explicit_account<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<meerkat_auth_core::connector_oauth::AccountSelection, D::Error> {
    meerkat_auth_core::connector_oauth::AccountSelection::deserialize(d)
}

fn explicit_lifetime<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
    Option::<u64>::deserialize(d)
}

impl Descriptor {
    fn into_native(self, realm: meerkat_core::RealmId) -> Result<ConnectorOAuthTarget, ToolError> {
        let account =
            meerkat_core::CredentialAccountId::parse(&self.slot_id).map_err(|_| invalid())?;
        Ok(ConnectorOAuthTarget {
            slot: meerkat_core::CredentialAccountRef { realm, account },
            issuer: self.issuer,
            client: self.client,
            resource: self.resource,
            scopes: self.scopes,
            strategy_id: self.strategy_id,
            account: self.account_selection,
            client_auth_profile: self.client_auth_profile,
        })
    }
}

/// The human reviews this exact requested destination and lifetime. It is not
/// a stock grant, application ServicePermission or permission to attach.
#[derive(Clone, Serialize)]
pub struct InstalledAccountSelection {
    pub agent: String,
    pub expires_in_ms: Option<u64>,
    pub operations: BTreeMap<String, McpConnectorOperation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_policy: Option<ServiceToolPolicySelection>,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum AccountState {
    Reviewed,
    Denied,
    Unavailable,
}

#[derive(Serialize)]
struct AccountReply {
    state: AccountState,
    login_completed: bool,
    access_reviewed: bool,
    grant_issued: bool,
    ready: bool,
    fresh_instruction_retained: bool,
}

impl AccountReply {
    fn new(state: AccountState, login_completed: bool, access_reviewed: bool) -> Self {
        Self {
            state,
            login_completed,
            access_reviewed,
            grant_issued: false,
            ready: false,
            fresh_instruction_retained: false,
        }
    }
}

/// Process-only request carried from this native registration to its private
/// presenter. The native ticket retains the actual borrowed dispatch binding;
/// no call, plan or original authority is reconstructed from these fields.
pub(crate) struct InstalledAccountRequest {
    job_id: String,
    target: ConnectorOAuthTarget,
    software: InstalledSelection,
    selection: InstalledAccountSelection,
    task: Option<toolkit_mobkit::private_originals::InstalledServiceTask>,
    entry: ReviewedEntryTicket,
    current: CurrentRequest,
    original: meerkat_core::retained_work::RetainedWorkIdentity,
    response: oneshot::Sender<AccountReply>,
}

/// Retains exactly the review transferred by one native adapter construction.
/// It is neither a session registry nor a new authentication state machine.
pub(crate) struct InstalledAccountRequests {
    review: InstalledServiceReview,
    requests: mpsc::Receiver<InstalledAccountRequest>,
    public_requests: mpsc::Receiver<public::InstalledPublicRequest>,
    credential_requests: mpsc::Receiver<credentials::InstalledCredentialRequest>,
    // Historical native identity and exact private decision, not live grants.
    // Multiple integrations can finish in one helper session. The subsequent
    // application connection/permission mapping is a separate owner join.
    reviewed: Vec<(
        meerkat_core::retained_work::RetainedWorkIdentity,
        ReviewedInstalledAccount<InstalledAccountSelection>,
    )>,
}

impl InstalledAccountRequest {
    /// The actual borrowed helper request owns this response channel.
    pub(crate) async fn closed(&mut self) { self.response.closed().await; }

    pub(crate) fn console_task(&self) -> Option<&toolkit_mobkit::private_originals::InstalledServiceTask> {
        self.task.as_ref().filter(|task| task.console_login().is_some())
    }
}

impl InstalledAccountRequests {
    pub(crate) fn new(review: InstalledServiceReview) -> (Arc<dyn AgentToolDispatcher>, Self) {
        Self::with_tasks(review, None)
    }
    pub(crate) fn with_tasks(review: InstalledServiceReview,
        tasks: Option<toolkit_mobkit::installed_host::acquisition::InstalledServiceTaskSource>,
    ) -> (Arc<dyn AgentToolDispatcher>, Self) {
        Self::with_policy(review, tasks, None)
    }
    #[cfg(all(unix, feature = "console"))]
    pub(crate) fn with_selected_policy(review: InstalledServiceReview,
        tasks: Option<toolkit_mobkit::installed_host::acquisition::InstalledServiceTaskSource>,
        selected: Arc<crate::application_acquisition::selected_policy::SelectedPolicyBinding>,
    ) -> (Arc<dyn AgentToolDispatcher>, Self) {
        let application = application::ApplicationRequestOwner::selected(&review, selected);
        Self::with_policy(review, tasks, Some(application))
    }
    fn with_policy(review: InstalledServiceReview,
        tasks: Option<toolkit_mobkit::installed_host::acquisition::InstalledServiceTaskSource>,
        application: Option<Arc<application::ApplicationRequestOwner>>,
    ) -> (Arc<dyn AgentToolDispatcher>, Self) {
        let (send, requests) = mpsc::channel(1);
        let (public_send, public_requests) = mpsc::channel(1);
        let (credential_send, credential_requests) = mpsc::channel(1);
        let account: Arc<dyn AgentToolDispatcher> = Arc::new(RequestTool {
            checked: review.checked.clone(),
            realm: review.realm.clone(),
            session: review.session.clone(),
            requests: Some(send),
            public_requests: Some(public_send),
            credential_requests: Some(credential_send),
            tasks,
            application,
        });
        let registered: Arc<dyn AgentToolDispatcher> = Arc::new(
            RegisteredToolBundleDispatcher::new(SETUP_ACCOUNTS_BUNDLE, account),
        );
        let dispatcher = Arc::new(DynamicToolComposite::new(vec![
            review.checked_dispatcher(),
            registered,
        ]));
        (
            dispatcher,
            Self {
                review,
                requests,
                public_requests,
                credential_requests,
                reviewed: Vec::new(),
            },
        )
    }

    pub(crate) async fn next(&mut self) -> Option<InstalledAccountRequest> {
        self.requests.recv().await
    }

    pub(crate) async fn next_request(&mut self) -> Option<InstalledRequest> {
        tokio::select! { biased;
            request = self.requests.recv() => request.map(InstalledRequest::Account),
            request = self.public_requests.recv() => request.map(InstalledRequest::Public),
            request = self.credential_requests.recv() => request.map(InstalledRequest::Credentials),
        }
    }
}

pub(crate) enum InstalledRequest {
    Account(InstalledAccountRequest),
    Public(public::InstalledPublicRequest),
    Credentials(credentials::InstalledCredentialRequest),
}
impl InstalledRequest {
    pub(crate) fn console_task(&self) -> Option<&toolkit_mobkit::private_originals::InstalledServiceTask> {
        match self {
            Self::Account(request) => request.console_task(),
            Self::Public(request) => request.console_task(),
            Self::Credentials(request) => request.console_task(),
        }
    }
    pub(crate) async fn closed(&mut self) {
        match self {
            Self::Account(request) => request.closed().await,
            Self::Public(request) => request.closed().await,
            Self::Credentials(request) => request.closed().await,
        }
    }
}

/// An in-call rendezvous with the original awaited dispatcher. A response is
/// only a current check point, never a reusable permit or native entry ticket.
pub(super) struct CurrentRequest(mpsc::Sender<oneshot::Sender<bool>>);
impl CurrentRequest {
    pub(super) async fn check(&self) -> bool {
        let (send, receive) = oneshot::channel();
        if self.0.send(send).await.is_err() {
            return false;
        }
        receive.await.unwrap_or(false)
    }
}

/// Request-local retirement, not a grant or a replacement for native checks.
/// It remains live through post-decision native work and its final predicate.
pub(super) struct ReviewControl {
    cancelled: tokio::sync::watch::Sender<bool>,
    created: tokio::sync::watch::Receiver<Option<ApprovalId>>,
}
impl ReviewControl {
    fn retire(&self) { self.cancelled.send_replace(true); }
    async fn approval(&self) -> Result<ApprovalId, ConnectError> {
        let mut created = self.created.clone();
        loop {
            if let Some(id) = created.borrow_and_update().clone() { return Ok(id); }
            created.changed().await.map_err(|_| ConnectError::ReviewCancelled)?;
        }
    }
}

#[derive(Clone)]
pub(super) struct ReviewCancellation {
    cancelled: tokio::sync::watch::Receiver<bool>,
    created: tokio::sync::watch::Sender<Option<ApprovalId>>,
}
impl ReviewCancellation {
    fn channel() -> (ReviewControl, Self) {
        let (cancelled, receive) = tokio::sync::watch::channel(false);
        let (created, approval) = tokio::sync::watch::channel(None);
        (ReviewControl { cancelled, created: approval }, Self { cancelled: receive, created })
    }
    pub(super) fn approval_created(&self, id: &ApprovalId) {
        self.created.send_replace(Some(id.clone()));
    }
    pub(super) fn check(&self) -> Result<(), ConnectError> {
        if *self.cancelled.borrow() || self.cancelled.has_changed().is_err() {
            Err(ConnectError::ReviewCancelled)
        } else { Ok(()) }
    }
    pub(super) async fn cancelled(&self) {
        let mut receive = self.cancelled.clone();
        while !*receive.borrow_and_update() {
            if receive.changed().await.is_err() { break; }
        }
    }
}

/// Report the local failure without exposing provider bodies, callback data or
/// account identifiers. Native tool feedback remains the existing unavailable
/// result. Only an already typed cleanup failure gets a cleanup label; all
/// other errors remain generic rather than inferring meaning from display text.
pub(crate) fn report_failure(error: &ConnectError) {
    let label = match error {
        ConnectError::Cleanup { .. } => "account request unavailable; cleanup failed",
        ConnectError::ReviewCancelled => "account request cancelled",
        _ => "account request unavailable",
    };
    eprintln!("{label}");
}

fn joined_review<T>(result: Result<T, ConnectError>) -> Result<(), ConnectError> {
    match result {
        Ok(_) | Err(ConnectError::ReviewCancelled) => Ok(()),
        Err(error) => Err(error),
    }
}

fn consumer_agent_schema() -> Value {
    json!({"type":"string", "description":"Exact native member identifier of the captured waiting task, not a display name or role label. If this tool returns a validated consumer mismatch, use only its exact returned selector with the same job, raw operations and requested lifetime."})
}

// The caller invokes this only after preparing the captured task and checking
// the borrowed call again. A mismatch discloses no unrelated member or grant.
fn require_captured_consumer(name: &str, proposed: &str,
    task: &toolkit_mobkit::private_originals::InstalledServiceTask) -> Result<(), ToolError> {
    let captured = task.consumer_member().ok_or_else(|| ToolError::unavailable(name,
        meerkat_core::ToolUnavailableReason::TemporarilyUnavailable))?;
    validate_captured_consumer(name, proposed, captured)
}

fn validate_captured_consumer(name: &str, proposed: &str, captured: &str) -> Result<(), ToolError> {
    if proposed != captured {
        let exact = serde_json::to_string(captured).map_err(|_| unavailable())?;
        return Err(ToolError::InvalidArguments { name: name.into(), reason: format!(
            "agent must be the captured waiting-task member {exact}, not a display name or role label. Correct only agent and resubmit the same job, raw operations and requested lifetime. This request has not entered access review.") });
    }
    Ok(())
}

struct RequestTool {
    checked: Arc<CheckedInstalls>,
    realm: RealmLocator,
    session: SessionId,
    requests: Option<mpsc::Sender<InstalledAccountRequest>>,
    public_requests: Option<mpsc::Sender<public::InstalledPublicRequest>>,
    credential_requests: Option<mpsc::Sender<credentials::InstalledCredentialRequest>>,
    tasks: Option<toolkit_mobkit::installed_host::acquisition::InstalledServiceTaskSource>,
    application: Option<Arc<application::ApplicationRequestOwner>>,
}

impl RequestTool {
    async fn request(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        plan: Option<&ResolvedToolExecutionPlan>,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        if call.name == SETUP_PUBLIC_SERVICE_REQUEST {
            return public::dispatch(self, call, context, plan).await;
        }
        if call.name != SETUP_ACCOUNT_REQUEST {
            return Err(ToolError::NotFound {
                name: call.name.into(),
            });
        }
        if context.origin_session_id() != Some(&self.session)
            || context.work_authorization().is_none()
            || context.retained_work().is_none()
        {
            return Err(ToolError::access_denied(call.name));
        }
        if credentials::is_proposal(call.args.get())? {
            return credentials::dispatch(self, call, context, plan).await;
        }
        let refreshed = context.check_tool_authorization(call, plan)?;
        let mut current = refreshed.unwrap_or_else(|| context.clone());
        let proposal = parse(call.args.get())?;
        let target = proposal.connector.into_native(self.realm.realm.clone())?;
        if target.account.is_unverified() {
            return Err(invalid());
        }
        let selected = self
            .checked
            .connector_selection(&self.realm, &self.session, &proposal.job_id)
            .map_err(|_| unavailable())?;
        let software = selected.installed.clone();
        let operations: BTreeMap<_, McpConnectorOperation> = proposal
            .operations
            .into_iter()
            .map(|(name, operation)| (name, operation.into()))
            .collect();
        let login =
            InstallationLogin::prepare(selected, operations.clone()).map_err(|_| unavailable())?;
        login.check_target(&target).map_err(|_| unavailable())?;
        let original = current
            .retained_work()
            .cloned()
            .ok_or_else(|| ToolError::access_denied(call.name))?;
        // A task is offered only by the existing installed acquisition owner.
        // Unsupported requester/route authority leaves account-only setup valid.
        let task = match &self.tasks {
            Some(tasks) => tasks.prepare_captured_consumer(call, &current, plan).await,
            None => Ok(None),
        };
        // Revalidate this actual call after every preparation outcome, before
        // exposing a refusal or sending a proposal to the private presenter.
        if let Some(refreshed) = current.check_tool_authorization(call, plan)? { current = refreshed; }
        let task = task.map_err(|error| match error {
            toolkit_mobkit::private_originals::PrivateOriginalError::Unavailable => unavailable(),
            toolkit_mobkit::private_originals::PrivateOriginalError::Conflict
            | toolkit_mobkit::private_originals::PrivateOriginalError::Denied => ToolError::access_denied(call.name),
        })?;
        if let Some(task) = &task { require_captured_consumer(call.name, &proposal.agent, task)?; }
        if task.is_none() && proposal.tool_policy.is_some() { return Err(ToolError::access_denied(call.name)); }
        let selection = InstalledAccountSelection {
            agent: proposal.agent, expires_in_ms: proposal.expires_in_ms,
            operations, tool_policy: proposal.tool_policy,
        };
        if let Some(application) = &self.application {
            if let Some(result) = application::connector(self, application.clone(), call, &current, plan,
                task.as_ref().ok_or_else(|| ToolError::access_denied(call.name))?, login, &target,
                &software, &selection).await? { return Ok(result); }
            if let Some(refreshed) = current.check_tool_authorization(call, plan)? { current = refreshed; }
        }
        let entry = current.reviewed_entry_ticket(call, plan)?;
        let (checks, mut requests) = mpsc::channel::<oneshot::Sender<bool>>(1);
        let (response, receive) = oneshot::channel();
        self.requests
            .as_ref().ok_or_else(unavailable)?
            .try_send(InstalledAccountRequest {
                job_id: proposal.job_id,
                target,
                software,
                selection,
                task,
                entry,
                current: CurrentRequest(checks),
                original,
                response,
            })
            .map_err(|_| unavailable())?;
        tokio::pin!(receive);
        let response = loop {
            tokio::select! { biased;
                request = requests.recv() => {
                    let Some(request) = request else { break receive.await; };
                    match current.check_tool_authorization(call, plan) {
                        Ok(refreshed) => {
                            if let Some(refreshed) = refreshed { current = refreshed; }
                            let _ = request.send(true);
                        }
                        Err(error) => { let _ = request.send(false); return Err(error); }
                    }
                }
                response = receive.as_mut() => break response,
            }
        };
        // Success and error remain private until this same native call passes
        // its final check. Every refresh retains the native-owned binding.
        current.check_tool_authorization(call, plan)?;
        let reply = response.map_err(|_| unavailable())?;
        serde_json::to_string(&reply)
            .map(|body| ToolResult::new(call.id.into(), body, false).into())
            .map_err(|_| unavailable())
    }
}

#[async_trait]
impl AgentToolDispatcher for RequestTool {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        vec![Arc::new(installed_account_request_definition()),
            Arc::new(public::installed_public_service_request_definition())].into()
    }
    fn tool_catalog_capabilities(&self) -> ToolCatalogCapabilities {
        ToolCatalogCapabilities {
            exact_catalog: true,
            may_require_catalog_control_plane: false,
        }
    }
    fn review_entry_support(&self, name: &str) -> meerkat_core::approval::review::ReviewEntrySupport {
        use meerkat_core::approval::review::ReviewEntrySupport;
        if matches!(name, SETUP_ACCOUNT_REQUEST | SETUP_PUBLIC_SERVICE_REQUEST) {
            ReviewEntrySupport::ConsumesAtEntry
        } else {
            ReviewEntrySupport::Unsupported
        }
    }
    fn tool_mutation_class(&self, name: &str) -> ToolMutationClass {
        if matches!(name, SETUP_ACCOUNT_REQUEST | SETUP_PUBLIC_SERVICE_REQUEST) {
            ToolMutationClass::Mutating
        } else {
            ToolMutationClass::Unknown
        }
    }
    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        Err(ToolError::access_denied(call.name))
    }
    async fn dispatch_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        self.request(call, context, None).await
    }
    async fn dispatch_resolved_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        plan: &ResolvedToolExecutionPlan,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        self.request(call, context, Some(plan)).await
    }
}

fn parse(raw: &str) -> Result<Proposal, ToolError> {
    if raw.len() > 32 * 1024 {
        return Err(invalid());
    }
    let proposal: Proposal = serde_json::from_str(raw).map_err(|_| invalid())?;
    let bounded = |s: &str| !s.is_empty() && s.len() <= 2048 && !s.chars().any(char::is_control);
    if !bounded(&proposal.job_id)
        || !bounded(&proposal.agent)
        || proposal.expires_in_ms == Some(0)
        || proposal.connector.account_selection.is_unverified()
        || proposal
            .connector
            .account_selection
            .known()
            .is_some_and(|s| !bounded(s))
        || proposal.operations.is_empty()
        || proposal.operations.len() > 128
        || proposal.operations.iter().any(|(name, op)| {
            !bounded(name)
                || !bounded(&op.destination_origin)
                || op.required_scopes.len() > 128
                || op.required_scopes.iter().any(|scope| !bounded(scope))
        })
        || [
            &proposal.connector.slot_id,
            &proposal.connector.issuer,
            &proposal.connector.client,
            &proposal.connector.resource,
            &proposal.connector.strategy_id,
        ]
        .iter()
        .any(|s| !bounded(s))
        || proposal.connector.scopes.len() > 128
        || proposal.connector.scopes.iter().any(|s| !bounded(s))
        || proposal
            .connector
            .client_auth_profile
            .as_ref()
            .is_some_and(|s| !bounded(s))
    {
        return Err(invalid());
    }
    Ok(proposal)
}

fn invalid() -> ToolError {
    ToolError::InvalidArguments { name: SETUP_ACCOUNT_REQUEST.into(), reason: "Provide one exact checked job, nonsecret connector descriptor, explicit operations, proposed agent and explicit lifetime. Authority, callbacks and credentials are not accepted.".into() }
}
fn unavailable() -> ToolError {
    ToolError::unavailable(
        SETUP_ACCOUNT_REQUEST,
        meerkat_core::ToolUnavailableReason::TemporarilyUnavailable,
    )
}

/// Exact final surface identity for the application's existing native compiler.
/// Exposure remains subject to its explicitly selected policy and native grants.
pub fn installed_account_request_definition() -> ToolDef {
    // The execution decoders retain their closed OAuth/credential alternatives.
    // Expose their object envelope without a root combinator to model providers.
    let mut schema = oauth_proposal_schema();
    let credential = credentials::proposal_schema();
    schema["required"] = json!(["job_id", "operations", "agent", "expires_in_ms"]);
    schema["properties"]["authentication"] = credential["properties"]["authentication"].clone();
    let oauth_operations = schema["properties"]["operations"].take();
    schema["properties"]["operations"] = json!({"anyOf":[
        oauth_operations, credential["properties"]["operations"]
    ]});
    ToolDef::new(
        SETUP_ACCOUNT_REQUEST,
        "Request account access for this exact checked software job and waiting task. For native connector OAuth, propose the nonsecret connector descriptor and operations. For a package with declared secret slots, use authentication.kind=declared_secret_slots and exact raw operation names; the current protected Console human reviews saving configuration/credentials plus task access and enters values separately. That Console path requires explicit tool_policy with a non-null catalog consequence and coverage of every selected exposed tool name; consequences are proposals for the same protected review. Never send credential values or slot names. The selected host may use application policy only for the existing native-connector branch. A requires_action result is not success. Authorized access may retain a fresh instruction and task-scoped grant; native attachment, input admission, provider authentication and readiness remain separate.",
        schema,
    )
}

fn oauth_proposal_schema() -> Value {
    json!({"type":"object","additionalProperties":false,"required":["job_id","connector","operations","agent","expires_in_ms"],
            "properties":{"job_id":{"type":"string"},"agent":consumer_agent_schema(),"expires_in_ms":{"type":["integer","null"],"minimum":1},
                "tool_policy":tool_policy_schema(),
                "connector":{"type":"object","additionalProperties":false,"required":["slot_id","issuer","client","resource","scopes","strategy_id","account_selection"],
                    "properties":{"slot_id":{"type":"string"},"issuer":{"type":"string"},"client":{"type":"string"},"resource":{"type":"string"},
                        "scopes":{"type":"array","items":{"type":"string"},"uniqueItems":true},"strategy_id":{"type":"string"},
                        "account_selection":{"type":["string","null"]},"client_auth_profile":{"type":"string"}}},
                "operations":{"type":"object","minProperties":1,"maxProperties":128,"additionalProperties":{"type":"object","additionalProperties":false,
                    "required":["required_scopes","destination_origin"],"properties":{"required_scopes":{"type":"array","items":{"type":"string"},"uniqueItems":true},"destination_origin":{"type":"string"}}}}}})
}

/// The captured-Original Console path needs both parts of the proposed
/// successor: native task catalog scope and ordinary exposed-tool invocation.
/// This validates a proposal only; the existing owner still constructs and
/// reviews the whole successor and issues any native grant.
fn require_console_tool_policy(name: &str, config: &meerkat_core::McpServerConfig,
    operations: &std::collections::BTreeSet<String>, selection: Option<&ServiceToolPolicySelection>,
) -> Result<(), ToolError> {
    let required = operations.iter().map(|raw|
        config.tool_names.get(raw).unwrap_or(raw).as_str()).collect::<std::collections::BTreeSet<_>>();
    let covered = selection.into_iter().flat_map(|policy| policy.tools.iter())
        .map(|tool| tool.name.as_str()).collect::<std::collections::BTreeSet<_>>();
    let missing = required.difference(&covered).copied().collect::<Vec<_>>();
    if selection.is_some_and(|policy| policy.catalog.is_some()) && missing.is_empty() { return Ok(()); }
    let names = serde_json::to_string(&required).map_err(|_| unavailable())?;
    Err(ToolError::InvalidArguments { name: name.into(), reason: format!(
        "This captured-original Console setup requires explicit tool_policy: select a catalog consequence (r0-r3, not null) and tools with consequences covering these exact exposed names: {names}. Raw operations and exposed tool names are separate; do not use declaration IDs. Keep the same checked job, consumer and requested lifetime. Existing member policy cannot supply the new task's catalog grant. The same protected review covers the whole resulting member policy until superseded and catalog metadata for the Original conversation, not only this integration. This request has not entered access review."
    ) })
}

/// Optional proposal only. The canonical owner supplies the member, complete
/// candidate, version and task/session scopes before the same approval.
fn tool_policy_schema() -> Value {
    json!({"type":"object","additionalProperties":false,"required":["tools","catalog"],
        "description":"Propose exact observed model-facing tool names for the waiting original agent. The same approval covers the complete resulting member policy, including preserved permissions, until superseded. Native service and catalog grants remain task-scoped. Catalog access lets the waiting original agent find tools available in the original conversation; it is not limited to this plugin. The protected Console public-service and declared-secret-slot paths require an explicit non-null catalog consequence and tools covering every selected operation's exact exposed name from checked tool_names, or the raw name when unmapped, even if member policy already permits it. Trusted application delegation and other lower-level paths retain their own selection contract.",
        "properties":{"tools":{"type":"array","maxItems":128,"items":{"type":"object","additionalProperties":false,
            "required":["name","consequence"],"properties":{"name":{"type":"string","minLength":1,"maxLength":512},
                "consequence":{"enum":["r0","r1","r2","r3"]}}}},"catalog":{"enum":[null,"r0","r1","r2","r3"]}}})
}
