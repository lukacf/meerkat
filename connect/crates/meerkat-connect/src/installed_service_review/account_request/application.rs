//! Direct application-policy consumption on the same native setup router.
//! Human-required results pause setup; this path never begins authentication.

use super::*;
use std::{collections::BTreeSet, sync::atomic::{AtomicBool, Ordering}};
use toolkit_mobkit::private_originals::{ApplicationServicePreparation, InstalledApplicationServicePolicy,
    InstalledServiceTask, PreparedApplicationServiceInstruction, PrivateOriginalError};

pub(super) struct ApplicationRequestOwner {
    policy: ServicePolicySource,
    auth: Arc<ConnectorAuth>,
    approvals: Arc<LocalApprovals>,
    checked: Arc<CheckedInstalls>,
}
enum ServicePolicySource {
    Installed(Arc<InstalledApplicationServicePolicy>),
    #[cfg(all(unix, feature = "console"))]
    Selected(Arc<crate::application_acquisition::selected_policy::SelectedPolicyBinding>),
}
impl ServicePolicySource {
    #[cfg(all(unix, feature = "console"))]
    fn operation_endpoint(&self, name: &str) -> Result<Option<toolkit_mobkit::private_host::ServiceParentEndpoint>, ToolError> {
        match self {
            Self::Installed(_) => Ok(None),
            Self::Selected(selected) => selected.service_parent().map_err(|error|
                crate::approval::installation_policy::tool_refusal(name, error)),
        }
    }

    fn resolve(&self, name: &str) -> Result<Option<Arc<InstalledApplicationServicePolicy>>, ToolError> {
        match self {
            Self::Installed(policy) => Ok(Some(policy.clone())),
            #[cfg(all(unix, feature = "console"))]
            Self::Selected(selected) => selected.service().map_err(|error|
                crate::approval::installation_policy::tool_refusal(name, error)),
        }
    }
}
impl ApplicationRequestOwner {
    #[cfg(all(unix, feature = "console"))]
    pub(super) fn selected(review: &InstalledServiceReview,
        selected: Arc<crate::application_acquisition::selected_policy::SelectedPolicyBinding>) -> Arc<Self> {
        Arc::new(Self { policy: ServicePolicySource::Selected(selected), auth: review.auth.clone(),
            approvals: review.approvals.clone(), checked: review.checked.clone() })
    }
}
impl Drop for ApplicationRequestOwner {
    fn drop(&mut self) {
        self.approvals.retire();
        self.checked.retire();
    }
}

/// Called only by the selected factory composition after the same actual host
/// owner was checked for both independent installation and access policies.
/// This owns no unused review, login attempt, presenter or request queue.
pub(crate) fn compose_application_policy_requests(
    adapter: Arc<meerkat::McpRouterAdapter>, auth: Arc<ConnectorAuth>, realm: RealmLocator,
    session: SessionId, approvals: Arc<LocalApprovals>,
    tasks: toolkit_mobkit::installed_host::acquisition::InstalledServiceTaskSource,
    policy: Arc<InstalledApplicationServicePolicy>,
) -> Result<Arc<dyn AgentToolDispatcher>, ConnectError> {
    approvals.check_session(&session)?;
    if realm.realm != auth.realm { return Err(refused("The application account owner belongs to another realm")); }
    let checked = CheckedInstalls::with_software_review(adapter, approvals.clone());
    checked.bind_realm(&realm).map_err(|_| refused("The selected application setup router could not be bound"))?;
    let application = Arc::new(ApplicationRequestOwner { policy: ServicePolicySource::Installed(policy), auth, approvals, checked: checked.clone() });
    let tools: Arc<dyn AgentToolDispatcher> = Arc::new(RequestTool { checked: checked.clone(), realm, session,
        requests: None, public_requests: None, credential_requests: None, tasks: Some(tasks), application: Some(application) });
    let registered: Arc<dyn AgentToolDispatcher> = Arc::new(RegisteredToolBundleDispatcher::new(SETUP_ACCOUNTS_BUNDLE, tools));
    Ok(Arc::new(DynamicToolComposite::new(vec![checked, registered])))
}

#[cfg(all(unix, feature = "console"))]
pub(crate) fn compose_selected_policy_requests(
    adapter: Arc<meerkat::McpRouterAdapter>, auth: Arc<ConnectorAuth>, realm: RealmLocator,
    session: SessionId, approvals: Arc<LocalApprovals>,
    tasks: toolkit_mobkit::installed_host::acquisition::InstalledServiceTaskSource,
    selected: Arc<crate::application_acquisition::selected_policy::SelectedPolicyBinding>,
) -> Result<Arc<dyn AgentToolDispatcher>, ConnectError> {
    approvals.check_session(&session)?;
    if realm.realm != auth.realm { return Err(refused("The selected setup router belongs to another realm")); }
    let checked = CheckedInstalls::with_software_review(adapter, approvals.clone());
    checked.bind_realm(&realm).map_err(|_| refused("The selected setup router could not be bound"))?;
    let application = Arc::new(ApplicationRequestOwner { policy: ServicePolicySource::Selected(selected),
        auth, approvals, checked: checked.clone() });
    let tools: Arc<dyn AgentToolDispatcher> = Arc::new(RequestTool { checked: checked.clone(), realm, session,
        requests: None, public_requests: None, credential_requests: None, tasks: Some(tasks), application: Some(application) });
    let registered: Arc<dyn AgentToolDispatcher> = Arc::new(RegisteredToolBundleDispatcher::new(SETUP_ACCOUNTS_BUNDLE, tools));
    Ok(Arc::new(DynamicToolComposite::new(vec![checked, registered])))
}

fn failure(name: &str, error: PrivateOriginalError) -> ToolError {
    match error {
        PrivateOriginalError::Unavailable => ToolError::unavailable(name, meerkat_core::ToolUnavailableReason::TemporarilyUnavailable),
        PrivateOriginalError::Denied | PrivateOriginalError::Conflict => ToolError::access_denied(name),
    }
}
fn reply(call: ToolCallView<'_>, body: Value) -> Result<ToolDispatchOutcome, ToolError> {
    serde_json::to_string(&body).map(|body| ToolResult::new(call.id.into(), body, false).into()).map_err(|_| unavailable())
}
fn requires_action(call: ToolCallView<'_>, action: &str) -> Result<ToolDispatchOutcome, ToolError> {
    reply(call, json!({"state":"requires_action","action":action,"grant_issued":false,
        "fresh_instruction_retained":false,"ready":false}))
}
// None transfers only an actual human-needed outcome to the same presenter.
// Denied/unavailable/native refusal never takes this branch.
fn human_action(call: ToolCallView<'_>, action: &str, presenter: bool)
    -> Result<Option<ToolDispatchOutcome>, ToolError> {
    if presenter { Ok(None) } else { requires_action(call, action).map(Some) }
}
fn ids(operations: impl IntoIterator<Item = String>, name: &str) -> Result<BTreeSet<toolkit_package::Id>, ToolError> {
    operations.into_iter().map(toolkit_package::Id::new).collect::<Result<_, _>>()
        .map_err(|_| ToolError::access_denied(name))
}

pub(super) async fn public(owner: &RequestTool, application: Arc<ApplicationRequestOwner>,
    call: ToolCallView<'_>, context: &ToolDispatchContext, plan: Option<&ResolvedToolExecutionPlan>,
    task: &InstalledServiceTask, selected: &crate::service_install::PublicServiceSelection,
    config: &meerkat_core::McpServerConfig, operations: &BTreeSet<String>, expires_in_ms: Option<u64>,
    tool_policy: Option<ServiceToolPolicySelection>,
) -> Result<Option<ToolDispatchOutcome>, ToolError> {
    application.approvals.check_session(&owner.session).map_err(|_| unavailable())?;
    if selected.public_configuration(operations).ok().as_ref() != Some(config) { return Err(unavailable()); }
    let human = owner.public_requests.as_ref().is_some_and(|sender| !sender.is_closed()) && task.console_login().is_some();
    let Some(policy) = application.policy.resolve(call.name)? else {
        return human_action(call, "human_service_access_review", human);
    };
    let prepared = prepare_public(&application, task, policy, config,
        ids(operations.iter().cloned(), call.name)?, expires_in_ms, tool_policy, call.name).await?;
    context.check_tool_authorization(call, plan)?;
    let Some(prepared) = prepared else {
        return human_action(call, "human_service_access_review", human);
    };
    let (selected, config, operations) = (selected.clone(), config.clone(), operations.clone());
    let session = owner.session.clone();
    enter_and_apply(call, context, plan, prepared, move || {
        application.approvals.check_session(&session).is_ok()
            && selected.public_configuration(&operations).ok().as_ref() == Some(&config)
    }).await.map(Some)
}

pub(super) async fn connector(owner: &RequestTool, application: Arc<ApplicationRequestOwner>,
    call: ToolCallView<'_>, context: &ToolDispatchContext, plan: Option<&ResolvedToolExecutionPlan>,
    task: &InstalledServiceTask, login: InstallationLogin, target: &ConnectorOAuthTarget,
    software: &InstalledSelection, selection: &InstalledAccountSelection,
) -> Result<Option<ToolDispatchOutcome>, ToolError> {
    application.approvals.check_session(&owner.session).map_err(|_| unavailable())?;
    login.check_target(target).map_err(|_| ToolError::access_denied(call.name))?;
    let human = owner.requests.as_ref().is_some_and(|sender| !sender.is_closed());
    let Some(policy) = application.policy.resolve(call.name)? else {
        return human_action(call, "human_service_access_review", human);
    };
    let account = application.auth.facade.connector_status(&target.slot).await;
    context.check_tool_authorization(call, plan)?;
    task.check_setup_current().await.map_err(|e| failure(call.name, e))?;
    application.approvals.check_session(&owner.session).map_err(|_| unavailable())?;
    login.check_current().map_err(|_| unavailable())?;
    let account = account.map_err(|_| unavailable())?;
    if matches!(account.phase, meerkat::ConnectorAuthPhase::AuthorizationRequired | meerkat::ConnectorAuthPhase::ReauthRequired) {
        return human_action(call, "connector_authentication", human);
    }
    let joined = login.join_authorized(&target, &account).map_err(|_| ToolError::access_denied(call.name))?;
    let config = reviewed_configuration(&joined).map_err(|_| ToolError::access_denied(call.name))?;
    let installation = ConnectorMcpInstallation::from_config(&config).ok_or_else(unavailable)?
        .map_err(|_| ToolError::access_denied(call.name))?;
    let document = account_review_body(software, target, &account, &installation, selection)
        .map_err(|_| ToolError::access_denied(call.name))?;
    let task = task.clone().with_installation_readback_source(&config, document)
        .map_err(|error| failure(call.name, error))?;
    let prepared = prepare_connector(&application, &task, policy, installation,
        ids(selection.operations.keys().cloned(), call.name)?, selection.expires_in_ms,
        selection.tool_policy.clone(), call.name).await?;
    context.check_tool_authorization(call, plan)?;
    let Some(prepared) = prepared else {
        return human_action(call, "human_service_access_review", human);
    };
    // Re-observe the same native account after policy evaluation. Live attach
    // and operation use continue to enforce the native account and lineage.
    let latest = application.auth.facade.connector_status(&target.slot).await.map_err(|_| unavailable())?;
    context.check_tool_authorization(call, plan)?;
    if authorized_account(&target, &latest).map_err(|_| ToolError::access_denied(call.name))?
        != authorized_account(&target, &account).map_err(|_| ToolError::access_denied(call.name))?
        || reviewed_configuration(&login.join_authorized(&target, &latest).map_err(|_| unavailable())?).ok().as_ref() != Some(&config)
    { return Err(ToolError::access_denied(call.name)); }
    let session = owner.session.clone();
    enter_and_apply(call, context, plan, prepared, move || {
        application.approvals.check_session(&session).is_ok() && login.check_current().is_ok()
    }).await.map(Some)
}

enum PreparedAction {
    Configured(PreparedApplicationServiceInstruction),
    #[cfg(all(unix, feature = "console"))]
    Owned(OwnedOperation),
}
#[cfg(all(unix, feature = "console"))]
struct OwnedOperation(toolkit_mobkit::private_originals::ApplicationServiceOperationControl);
#[cfg(all(unix, feature = "console"))]
impl Drop for OwnedOperation { fn drop(&mut self) { self.0.begin_retire(); } }

async fn prepare_public(application: &ApplicationRequestOwner, task: &InstalledServiceTask,
    policy: Arc<InstalledApplicationServicePolicy>, config: &meerkat_core::McpServerConfig,
    operations: BTreeSet<toolkit_package::Id>, expires: Option<u64>,
    tool_policy: Option<ServiceToolPolicySelection>, name: &str,
) -> Result<Option<PreparedAction>, ToolError> {
    #[cfg(all(unix, feature = "console"))]
    if let Some(endpoint) = application.policy.operation_endpoint(name)? {
        let operation = task.clone().begin_application_no_connector_with_tool_policy(policy, config.clone(), operations, expires, tool_policy)
            .map_err(|error| failure(name, error))?;
        return prepare_owned(endpoint, operation, name).await;
    }
    match task.clone().prepare_application_no_connector_with_tool_policy(&policy, config, operations, expires, tool_policy).await
        .map_err(|error| failure(name, error))? {
        ApplicationServicePreparation::RequiresHuman => Ok(None),
        ApplicationServicePreparation::Prepared(prepared) => Ok(Some(PreparedAction::Configured(prepared))),
    }
}
async fn prepare_connector(application: &ApplicationRequestOwner, task: &InstalledServiceTask,
    policy: Arc<InstalledApplicationServicePolicy>, installation: ConnectorMcpInstallation,
    operations: BTreeSet<toolkit_package::Id>, expires: Option<u64>,
    tool_policy: Option<ServiceToolPolicySelection>, name: &str,
) -> Result<Option<PreparedAction>, ToolError> {
    #[cfg(all(unix, feature = "console"))]
    if let Some(endpoint) = application.policy.operation_endpoint(name)? {
        let operation = task.clone().begin_application_connector_with_tool_policy(policy, installation, operations, expires, tool_policy)
            .map_err(|error| failure(name, error))?;
        return prepare_owned(endpoint, operation, name).await;
    }
    match task.clone().prepare_application_connector_with_tool_policy(&policy, &installation, operations, expires, tool_policy).await
        .map_err(|error| failure(name, error))? {
        ApplicationServicePreparation::RequiresHuman => Ok(None),
        ApplicationServicePreparation::Prepared(prepared) => Ok(Some(PreparedAction::Configured(prepared))),
    }
}
#[cfg(all(unix, feature = "console"))]
async fn prepare_owned(endpoint: toolkit_mobkit::private_host::ServiceParentEndpoint,
    operation: toolkit_mobkit::private_originals::ApplicationServiceOperation, name: &str,
) -> Result<Option<PreparedAction>, ToolError> {
    use toolkit_mobkit::private_originals::ApplicationServicePreparedStatus;
    let control = match endpoint.adopt(operation) {
        Ok(control) => OwnedOperation(control),
        Err(mut operation) => { operation.retire().await; return Err(unavailable()); }
    };
    match control.0.prepared().await {
        Ok(ApplicationServicePreparedStatus::PreparedForEntry) => Ok(Some(PreparedAction::Owned(control))),
        Ok(ApplicationServicePreparedStatus::RequiresHuman) => { control.0.retire().await; Ok(None) }
        Err(error) => { control.0.retire().await; Err(failure(name, error)) }
    }
}

struct InvocationOpen(Arc<AtomicBool>);
impl Drop for InvocationOpen {
    fn drop(&mut self) { self.0.store(false, Ordering::SeqCst); }
}

async fn enter_and_apply(call: ToolCallView<'_>, context: &ToolDispatchContext,
    plan: Option<&ResolvedToolExecutionPlan>, prepared: PreparedAction,
    current: impl Fn() -> bool + Send + Sync + 'static,
) -> Result<ToolDispatchOutcome, ToolError> {
    context.check_tool_authorization(call, plan)?;
    let ticket = context.reviewed_entry_ticket(call, plan)?;
    let invocation = InvocationOpen(Arc::new(AtomicBool::new(true)));
    let open = invocation.0.clone();
    if !current() { return Err(ToolError::access_denied(call.name)); }
    // Keep the direct future under its native caller. The callback retains
    // actual entry custody in the existing blocking issuance worker if this
    // future is dropped. It never locks/reenters native phase publication.
    // Cancellation does not roll back an already-entered grant or source write.
    let custody = Arc::new(std::sync::Mutex::new(ticket.enter().map_err(|_| ToolError::access_denied(call.name))?));
    let current = move || { let _custody = &custody; open.load(Ordering::SeqCst) && current() };
    let retained = match prepared {
        PreparedAction::Configured(prepared) => prepared.apply(current).await.map_err(|error| failure(call.name, error))?,
        #[cfg(all(unix, feature = "console"))]
        PreparedAction::Owned(operation) => {
            use toolkit_mobkit::private_originals::{ServiceParentOperationOutcome, ServiceParentOperationReason};
            // The actor already owns this exact operation. Only this original
            // native caller can supply its entered custody and currentness.
            operation.0.enter(current).map_err(|error| failure(call.name, error))?;
            let observed = operation.0.result().await;
            match observed.outcome {
                Some(ServiceParentOperationOutcome::Succeeded) => observed.source.is_some(),
                Some(ServiceParentOperationOutcome::Uncertain) => return Err(unavailable()),
                _ => return Err(match observed.reason {
                    Some(ServiceParentOperationReason::Unavailable | ServiceParentOperationReason::NotSettled) => unavailable(),
                    _ => ToolError::access_denied(call.name),
                }),
            }
        }
    };
    context.check_tool_authorization(call, plan)?;
    reply(call, json!({"state":"authorized","grant_issued":retained,
        "fresh_instruction_retained":retained,"ready":false}))
}
