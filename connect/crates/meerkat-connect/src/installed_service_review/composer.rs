//! Bind the application's private review owner at native router construction.

use std::sync::{
    Arc,
    mpsc::{Receiver, SyncSender, sync_channel},
};

use meerkat::{
    AgentFactory, HostAuthService, McpAdapterComposeRefusal, McpAdapterComposer, McpRouterAdapter,
};
use meerkat_core::{AgentToolDispatcher, MobMemberBinding, RealmLocator, SessionId};

use super::{InstalledServiceReview, compose_with_auth, refused};
use crate::{ConnectError, approval::LocalApprovals, connector_auth::ConnectorAuth};

/// Configure the application's existing factory before it prepares members.
/// The application supplies its exact selected helper, realm, native account
/// facade and fresh private approval owner. This explicitly selects both the
/// factory's MCP composer and call-context provider slots. It must be composed
/// with any other application requirements before preparing an agent.
///
/// Native construction supplies the actual session and adapter. For the exact
/// selected helper, it binds the existing approval owner and publishes the
/// existing checked dispatcher. Unrelated members and non-member sessions keep
/// their adapter. The selected member under a different role refuses.
/// This function does not select a profile, add grants or authorize startup.
///
/// The receiver transfers the same review to the application's private owner.
/// Receive it after the native preparation has succeeded, and retain it with
/// that actor. Receiving it is not native finish/publication or readiness. On
/// any later preparation/activation failure, retire the review and await the
/// native owner's abort. During shutdown, join entered review/login work and
/// close the native actor through its existing owner. The receiver and factory
/// must remain alive until the preparation has either succeeded or failed.
///
/// Native calls this hook once for every router construction, including a
/// same-session rebuild. `LocalApprovals::bind_session` is already single-use;
/// a replacement construction therefore needs a fresh application-supplied
/// private review owner. This function never rebinds or retires a prior actor.
pub fn configure_installed_service_review(
    factory: AgentFactory,
    selected: MobMemberBinding,
    facade: HostAuthService,
    realm: RealmLocator,
    approvals: Arc<LocalApprovals>,
) -> Result<(AgentFactory, Receiver<InstalledServiceReview>), ConnectError> {
    let (composer, reviews) = SelectedReviewComposer::new(selected, facade, realm, approvals)?;
    let factory = factory
        .mcp_call_context_provider(composer.auth.context_provider.clone())
        .with_mcp_adapter_composer(composer);
    Ok((factory, reviews))
}

struct SelectedReviewComposer {
    selected: MobMemberBinding,
    realm: RealmLocator,
    auth: Arc<ConnectorAuth>,
    approvals: Arc<LocalApprovals>,
    reviews: SyncSender<InstalledServiceReview>,
}

impl SelectedReviewComposer {
    fn new(
        selected: MobMemberBinding,
        facade: HostAuthService,
        realm: RealmLocator,
        approvals: Arc<LocalApprovals>,
    ) -> Result<(Arc<Self>, Receiver<InstalledServiceReview>), ConnectError> {
        selected
            .comms_name()
            .map_err(|_| refused("The selected setup member is invalid"))?;
        let auth = ConnectorAuth::from_facade(facade, realm.realm.clone())?;
        // This is a transfer of one existing owner, not an effect queue.
        // try_send below never blocks native preparation on a private caller.
        let (sender, receiver) = sync_channel(1);
        Ok((
            Arc::new(Self {
                selected,
                realm,
                auth,
                approvals,
                reviews: sender,
            }),
            receiver,
        ))
    }

    fn is_selected(&self, member: Option<&MobMemberBinding>) -> Result<bool, McpAdapterComposeRefusal> {
        selected_member(&self.selected, member)
    }
}

impl McpAdapterComposer for SelectedReviewComposer {
    fn compose(
        &self,
        adapter: &Arc<McpRouterAdapter>,
        session_id: &SessionId,
        member: Option<&MobMemberBinding>,
    ) -> Result<Arc<dyn AgentToolDispatcher>, McpAdapterComposeRefusal> {
        if !self.is_selected(member)? {
            return Ok(adapter.clone());
        }
        self.approvals
            .bind_session(session_id)
            .map_err(|_| composition_refused())?;
        let review = compose_with_auth(
            adapter.clone(),
            self.auth.clone(),
            self.realm.clone(),
            session_id.clone(),
            self.approvals.clone(),
        )
        .map_err(|_| composition_refused())?;
        let dispatcher = review.checked_dispatcher();
        // Failed transfer drops the review, retiring its existing review and
        // checked-install owners. Native construction still owns and shuts
        // down the actual adapter on the returned refusal.
        self.reviews
            .try_send(review)
            .map_err(|_| composition_refused())?;
        Ok(dispatcher)
    }
}

/// Explicit terminal composition using the existing selected factory slots.
/// Create this handle from the terminal that will run
/// `serve_local_operator_with_setup`. Each selected construction enrolls a fresh
/// same-session review and its actual account request receiver together. The
/// application's existing native policy compiler must allow the exact final
/// tool identity and setup-account-request operation; this adds no grants.
/// Like the legacy constructor, this explicitly selects both factory slots.
/// The gateway uses the borrowed native factory offer below instead.
#[cfg(all(unix, feature = "console"))]
pub fn configure_installed_account_requests(
    factory: AgentFactory,
    selected: MobMemberBinding,
    facade: HostAuthService,
    realm: RealmLocator,
    presenters: crate::application_gateway::LocalSetupApprovals,
) -> Result<AgentFactory, ConnectError> {
    let composer = terminal_composer(selected, facade, realm, presenters)?;
    Ok(factory.mcp_call_context_provider(composer.auth.context_provider.clone())
        .with_mcp_adapter_composer(composer))
}

// The native factory offer names the installation's shared-store realm.
// Setup and grants belong to the selected member's canonical mob realm.
#[cfg(all(unix, feature = "console"))]
fn require_gateway_member_realm(
    selected: &MobMemberBinding,
    realm: &RealmLocator,
) -> Result<(), meerkat_toolkit::host_lifetime::control::HostLifetimeError> {
    if meerkat_core::mob_realm_id(&selected.mob_id).as_ref().ok() != Some(&realm.realm) {
        return Err(meerkat_toolkit::host_lifetime::control::HostLifetimeError::WrongOwner);
    }
    Ok(())
}

/// Called by the current installed host for each actual native factory offer.
/// The facade comes only from that offered factory and machine. The native
/// gateway owns installing the returned slots before cloning/building. Every
/// selected adapter construction then enrolls fresh private review custody.
#[cfg(all(unix, feature = "console"))]
pub(crate) fn gateway_account_composition(
    construction: &meerkat_mobkit::rpc_gateway::GatewayFactoryConstruction<'_>,
    selected: MobMemberBinding,
    realm: RealmLocator,
    presenters: crate::application_gateway::LocalSetupApprovals,
    tasks: toolkit_mobkit::installed_host::acquisition::InstalledServiceTaskSource,
) -> Result<meerkat_mobkit::rpc_gateway::GatewayMcpComposition,
    meerkat_toolkit::host_lifetime::control::HostLifetimeError>
{
    use meerkat_toolkit::host_lifetime::control::HostLifetimeError;
    require_gateway_member_realm(&selected, &realm)?;
    let facade = construction.host_auth_service().map_err(|_| HostLifetimeError::Unavailable)?;
    let realm_installation = if presenters.has_console() {
        Some(construction.realm_mcp_installation().map_err(|_| HostLifetimeError::Unavailable)?)
    } else { None };
    let mut composer = terminal_composer_with_tasks(selected, facade, realm, presenters, Some(tasks))
        .map_err(|_| HostLifetimeError::Unavailable)?;
    Arc::get_mut(&mut composer).ok_or(HostLifetimeError::Unavailable)?.realm_installation =
        realm_installation;
    let provider = composer.auth.context_provider.clone();
    Ok(meerkat_mobkit::rpc_gateway::GatewayMcpComposition::new(composer)
        .with_call_context_provider(provider))
}

#[cfg(all(unix, feature = "console"))]
pub(crate) fn terminal_composer(
    selected: MobMemberBinding,
    facade: HostAuthService,
    realm: RealmLocator,
    presenters: crate::application_gateway::LocalSetupApprovals,
) -> Result<Arc<TerminalReviewComposer>, ConnectError> {
    terminal_composer_with_tasks(selected, facade, realm, presenters, None)
}

#[cfg(all(unix, feature = "console"))]
fn terminal_composer_with_tasks(
    selected: MobMemberBinding, facade: HostAuthService, realm: RealmLocator,
    presenters: crate::application_gateway::LocalSetupApprovals,
    tasks: Option<toolkit_mobkit::installed_host::acquisition::InstalledServiceTaskSource>,
) -> Result<Arc<TerminalReviewComposer>, ConnectError> {
    selected.comms_name().map_err(|_| refused("The selected setup member is invalid"))?;
    let auth = ConnectorAuth::from_facade(facade, realm.realm.clone())?;
    Ok(Arc::new(TerminalReviewComposer { selected, realm, auth, presenters, tasks, realm_installation: None }))
}

#[cfg(all(unix, feature = "console"))]
pub(crate) struct TerminalReviewComposer {
    selected: MobMemberBinding,
    realm: RealmLocator,
    auth: Arc<ConnectorAuth>,
    presenters: crate::application_gateway::LocalSetupApprovals,
    tasks: Option<toolkit_mobkit::installed_host::acquisition::InstalledServiceTaskSource>,
    realm_installation: Option<meerkat_mobkit::rpc_gateway::GatewayRealmMcpInstallation>,
}

#[cfg(all(unix, feature = "console"))]
impl McpAdapterComposer for TerminalReviewComposer {
    fn compose(&self, adapter: &Arc<McpRouterAdapter>, session_id: &SessionId,
        member: Option<&MobMemberBinding>) -> Result<Arc<dyn AgentToolDispatcher>, McpAdapterComposeRefusal> {
        if !selected_member(&self.selected, member)? { return Ok(adapter.clone()); }
        self.presenters.create_installed_with_tasks(self.tasks.clone(), |approvals| {
            approvals.bind_session(session_id)?;
            let mut review = compose_with_auth(adapter.clone(), self.auth.clone(), self.realm.clone(),
                session_id.clone(), approvals)?;
            review.realm_installation = self.realm_installation.clone();
            Ok(review)
        }).map_err(|_| composition_refused())
    }
}

/// Consume the actual offered native factory, preserving its connector owner
/// and call-context provider. The application policies were bound to this exact
/// installed host before entering the existing factory callback.
#[cfg(all(unix, feature = "console"))]
pub(crate) fn gateway_policy_composition(
    construction: &meerkat_mobkit::rpc_gateway::GatewayFactoryConstruction<'_>,
    selected: MobMemberBinding,
    realm: RealmLocator,
    installation: crate::approval::installation_policy::InstalledInstallationPolicy,
    service: Option<Arc<toolkit_mobkit::private_originals::InstalledApplicationServicePolicy>>,
    artifact_parent: std::path::PathBuf,
    tasks: toolkit_mobkit::installed_host::acquisition::InstalledServiceTaskSource,
) -> Result<meerkat_mobkit::rpc_gateway::GatewayMcpComposition,
    meerkat_toolkit::host_lifetime::control::HostLifetimeError>
{
    use meerkat_toolkit::host_lifetime::control::HostLifetimeError;
    require_gateway_member_realm(&selected, &realm)?;
    if service.as_ref().is_some_and(|service| !service.belongs_to(installation.lifetime())) {
        return Err(HostLifetimeError::WrongOwner);
    }
    selected.comms_name().map_err(|_| HostLifetimeError::InvalidSelection)?;
    let facade = construction.host_auth_service().map_err(|_| HostLifetimeError::Unavailable)?;
    let auth = ConnectorAuth::from_facade(facade, realm.realm.clone())
        .map_err(|_| HostLifetimeError::Unavailable)?;
    let provider = auth.context_provider.clone();
    let composer = Arc::new(PolicyReviewComposer {
        selected, realm, auth, installation, service, artifact_parent, tasks,
    });
    Ok(meerkat_mobkit::rpc_gateway::GatewayMcpComposition::new(composer)
        .with_call_context_provider(provider))
}

#[cfg(all(unix, feature = "console"))]
struct PolicyReviewComposer {
    selected: MobMemberBinding,
    realm: RealmLocator,
    auth: Arc<ConnectorAuth>,
    installation: crate::approval::installation_policy::InstalledInstallationPolicy,
    service: Option<Arc<toolkit_mobkit::private_originals::InstalledApplicationServicePolicy>>,
    artifact_parent: std::path::PathBuf,
    tasks: toolkit_mobkit::installed_host::acquisition::InstalledServiceTaskSource,
}

#[cfg(all(unix, feature = "console"))]
impl McpAdapterComposer for PolicyReviewComposer {
    fn compose(&self, adapter: &Arc<McpRouterAdapter>, session_id: &SessionId,
        member: Option<&MobMemberBinding>) -> Result<Arc<dyn AgentToolDispatcher>, McpAdapterComposeRefusal> {
        if !selected_member(&self.selected, member)? { return Ok(adapter.clone()); }
        // Each actual native router receives a fresh same-session decision
        // owner. No presenter is constructed and then abandoned to retire it.
        let approvals = Arc::new(LocalApprovals::create_for_installation_policy(
            &self.artifact_parent, self.installation.clone()).map_err(|_| composition_refused())?
            .with_service_tasks(Some(self.tasks.clone())));
        approvals.bind_session(session_id).map_err(|_| composition_refused())?;
        match &self.service {
            Some(service) => super::account_request::compose_application_policy_requests(
                adapter.clone(), self.auth.clone(), self.realm.clone(), session_id.clone(),
                approvals, self.tasks.clone(), service.clone()).map_err(|_| composition_refused()),
            None => {
                // Software authority does not require an access parent. This
                // selected composition adds no account/public request surface.
                let checked = crate::service_install::CheckedInstalls::with_software_review(adapter.clone(), approvals);
                checked.bind_realm(&self.realm).map_err(|_| composition_refused())?;
                Ok(checked)
            }
        }
    }
}

/// Early factory composition keeps the actual adapter/auth facade and one
/// selection reference. Only final same-owner adoption makes policy usable.
#[cfg(all(unix, feature = "console"))]
pub(crate) fn gateway_selected_policy_composition(
    construction: &meerkat_mobkit::rpc_gateway::GatewayFactoryConstruction<'_>,
    selected: MobMemberBinding, realm: RealmLocator,
    policy: Arc<crate::application_acquisition::selected_policy::SelectedPolicyBinding>,
    presenters: Option<crate::application_gateway::LocalSetupApprovals>,
    artifact_parent: std::path::PathBuf,
    tasks: toolkit_mobkit::installed_host::acquisition::InstalledServiceTaskSource,
) -> Result<meerkat_mobkit::rpc_gateway::GatewayMcpComposition,
    meerkat_toolkit::host_lifetime::control::HostLifetimeError> {
    use meerkat_toolkit::host_lifetime::control::HostLifetimeError;
    require_gateway_member_realm(&selected, &realm)?;
    let facade = construction.host_auth_service().map_err(|_| HostLifetimeError::Unavailable)?;
    let realm_installation = if presenters.as_ref().is_some_and(|owner| owner.has_console()) {
        Some(construction.realm_mcp_installation().map_err(|_| HostLifetimeError::Unavailable)?)
    } else { None };
    let mut composer = selected_policy_composer(selected, facade, realm, policy, presenters, artifact_parent, tasks)
        .map_err(|_| HostLifetimeError::Unavailable)?;
    Arc::get_mut(&mut composer).ok_or(HostLifetimeError::Unavailable)?.realm_installation =
        realm_installation;
    let provider = composer.auth.context_provider.clone();
    Ok(meerkat_mobkit::rpc_gateway::GatewayMcpComposition::new(composer).with_call_context_provider(provider))
}

#[cfg(all(unix, feature = "console"))]
fn selected_policy_composer(selected: MobMemberBinding, facade: HostAuthService, realm: RealmLocator,
    policy: Arc<crate::application_acquisition::selected_policy::SelectedPolicyBinding>,
    presenters: Option<crate::application_gateway::LocalSetupApprovals>, artifact_parent: std::path::PathBuf,
    tasks: toolkit_mobkit::installed_host::acquisition::InstalledServiceTaskSource,
) -> Result<Arc<SelectedPolicyComposer>, ConnectError> {
    selected.comms_name().map_err(|_| refused("The selected setup member is invalid"))?;
    let auth = ConnectorAuth::from_facade(facade, realm.realm.clone())?;
    Ok(Arc::new(SelectedPolicyComposer { selected, realm, auth, policy, presenters, artifact_parent, tasks, realm_installation: None }))
}
#[cfg(all(unix, feature = "console"))]
struct SelectedPolicyComposer {
    selected: MobMemberBinding, realm: RealmLocator, auth: Arc<ConnectorAuth>,
    policy: Arc<crate::application_acquisition::selected_policy::SelectedPolicyBinding>,
    presenters: Option<crate::application_gateway::LocalSetupApprovals>, artifact_parent: std::path::PathBuf,
    tasks: toolkit_mobkit::installed_host::acquisition::InstalledServiceTaskSource,
    realm_installation: Option<meerkat_mobkit::rpc_gateway::GatewayRealmMcpInstallation>,
}
#[cfg(all(unix, feature = "console"))]
impl McpAdapterComposer for SelectedPolicyComposer {
    fn compose(&self, adapter: &Arc<McpRouterAdapter>, session: &SessionId,
        member: Option<&MobMemberBinding>) -> Result<Arc<dyn AgentToolDispatcher>, McpAdapterComposeRefusal> {
        if !selected_member(&self.selected, member)? { return Ok(adapter.clone()); }
        if let Some(presenters) = &self.presenters {
            return presenters.create_installed_selected(Some(self.tasks.clone()), Some(self.policy.clone()), |approvals| {
                approvals.bind_session(session)?;
                let mut review = compose_with_auth(adapter.clone(), self.auth.clone(), self.realm.clone(), session.clone(), approvals)?;
                review.realm_installation = self.realm_installation.clone();
                Ok(review)
            }).map_err(|_| composition_refused());
        }
        // No unused presenter or account queue in the headless composition.
        let (approvals, _) = LocalApprovals::create_for_selected_policy(&self.artifact_parent, self.policy.clone(), None)
            .map_err(|_| composition_refused())?;
        let approvals = Arc::new(approvals.with_service_tasks(Some(self.tasks.clone())));
        approvals.bind_session(session).map_err(|_| composition_refused())?;
        super::account_request::compose_selected_policy_requests(adapter.clone(), self.auth.clone(),
            self.realm.clone(), session.clone(), approvals, self.tasks.clone(), self.policy.clone())
            .map_err(|_| composition_refused())
    }
}

fn selected_member(selected: &MobMemberBinding, member: Option<&MobMemberBinding>)
    -> Result<bool, McpAdapterComposeRefusal> {
    let Some(member) = member else { return Ok(false); };
    if member == selected { return Ok(true); }
    if member.mob_id == selected.mob_id && member.member == selected.member {
        return Err(composition_refused());
    }
    Ok(false)
}

fn composition_refused() -> McpAdapterComposeRefusal {
    // Factory failures may reach a public error sink. Do not include private
    // presenter, account, storage or ceremony details in this error.
    McpAdapterComposeRefusal::new("The selected setup review could not be composed")
}

#[cfg(test)]
mod tests;
