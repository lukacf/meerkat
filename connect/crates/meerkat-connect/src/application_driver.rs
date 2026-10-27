//! One schema4 preparation and installed runtime in the same gateway process.
//! Native held completion supplies the finished surfaces. The private channel
//! supplies final application publication; neither substitutes for the other.


use std::{path::PathBuf, sync::{Arc, Mutex, OnceLock, atomic::{AtomicBool, Ordering}}, time::Duration};
use async_trait::async_trait;
use meerkat_mobkit::{
    UnifiedRuntime, mob_handle_runtime::MobBootstrapSpec,
    identity_first::{DurableAgentSpec, HeldIdentityMember, HeldCompletion, RosterContext, contracts::RosterProjection},
    rpc_gateway::{GatewayBootstrapReady, GatewayFactoryConstruction, GatewayHostCloseReason,
        GatewayHostComposition, GatewayHostError, GatewayMcpComposition, PreparedApplicationRequest,
        PreparedGatewayApplication, run_persistent_with_host},
    runtime::cross_mob_control::ControlListenAddr,
};
use meerkat_runtime::meerkat_machine::NativeGrantWorkConfiguration;
use meerkat_mcp::{McpAttemptGuardBounds, McpAttemptGuards};
use meerkat_toolkit::host_lifetime::{
    control::{HostLifetimeError, SelectedHostArtifact}, policy::PreparedLocalPolicy,
};
use serde::{Deserialize, Serialize};
use tokio::sync::{oneshot, watch};
use toolkit_mobkit::{
    host_gateway::{PreparedGatewayRuntime, ToolkitGatewayHost},
    installed_host::{PreparedInstallationOwner, PreparedInstalledHost, prepared_registration::PreparedMemberRegistration, operator_setup::OperatorOwnerStartup,
        preparation::{PreparationAuthorityIntent, PreparedControllerTarget},
        acquisition::InstalledAcquisitionTools},
    private_host::{PrivateHostConnection, PrivateHostStartup, PrivateHostStartupIntent},
};
use crate::{application_acquisition::ApplicationAcquisitionParameters,
    application_gateway::{InheritedOperatorTerminal, LocalSetupApprovals, LocalSetupReviews},
    application_setup::{PreparedApplicationSetup, compilation::{ApplicationSetupCompilation,
        ApplicationSetupCompilationInput}}, resources::{InstalledResources,
        LocalRoundAttempts, LocalRoundOwner}};

pub(crate) mod local_round;
use local_round::LocalRoundCleanup;

/// Trusted fresh-application preparation input on the inherited channel. The owner
/// facts contain no surface references; native construction produces those.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApplicationPreparationPlan {
    pub schema: u32,
    pub deployment_root: PathBuf,
    pub owner: PreparedLocalPolicy,
    pub startup: OperatorOwnerStartup,
    #[serde(default)]
    pub authorities: PreparationAuthorityIntent,
    pub application: ApplicationSetupCompilationInput,
    pub acquisition: ApplicationAcquisitionParameters,
    pub output: PathBuf,
    pub hold_timeout_ms: u64,
}

struct Publication {
    closed: bool,
    installed: Option<Arc<ToolkitGatewayHost>>,
}

struct Driver {
    plan: ApplicationPreparationPlan,
    plan_reference: SelectedHostArtifact,
    plan_bytes: Vec<u8>,
    compilation: Arc<ApplicationSetupCompilation>,
    registration: Arc<PreparedMemberRegistration>,
    runtime: PreparedGatewayRuntime,
    configuration: Mutex<Option<NativeGrantWorkConfiguration>>,
    prepared: Mutex<Option<PreparedInstalledHost>>,
    presenters: Option<LocalSetupApprovals>,
    selected_policy: Option<Arc<crate::application_acquisition::selected_policy::SelectedPolicyBinding>>,
    publication: Mutex<Publication>,
    closed: watch::Sender<bool>,
    selected: AtomicBool,
    completed: OnceLock<PreparedApplicationSetup>,
    deliver: Mutex<Option<oneshot::Sender<SelectedHostArtifact>>>,
    adopted: Mutex<Option<oneshot::Receiver<()>>>,
    attempt_guards: McpAttemptGuards,
    local_round_attempts: Arc<LocalRoundAttempts>,
    local_round_cleanup: tokio::sync::Mutex<LocalRoundCleanup>,
}

impl Driver {
    fn current(&self) -> Result<(), GatewayHostError> {
        if self.publication.lock().map_err(|_| GatewayHostError::Unavailable)?.closed {
            return Err(GatewayHostError::Denied);
        }
        Ok(())
    }
    fn installed(&self) -> Result<Arc<ToolkitGatewayHost>, GatewayHostError> {
        self.publication.lock().map_err(|_| GatewayHostError::Unavailable)?
            .installed.clone().ok_or(GatewayHostError::Unavailable)
    }
    fn retain_installed(&self, host: Arc<ToolkitGatewayHost>) -> Result<(), HostLifetimeError> {
        let mut publication = self.publication.lock().map_err(|_| HostLifetimeError::Unavailable)?;
        if publication.closed || publication.installed.is_some() { return Err(HostLifetimeError::Denied); }
        publication.installed = Some(host);
        // Wake the existing close observer to start observing this exact host.
        self.closed.send_replace(false);
        Ok(())
    }
    async fn closing(&self) {
        let mut closed = self.closed.subscribe();
        if *closed.borrow() { return; }
        while closed.changed().await.is_ok() { if *closed.borrow() { return; } }
    }
}

struct LiveRoster(Arc<ApplicationSetupCompilation>);
impl RosterProjection for LiveRoster {
    fn project(&self, context: &RosterContext, roster: Vec<DurableAgentSpec>)
        -> Result<Vec<DurableAgentSpec>, String>
    {
        if context.mob_definition.as_ref().is_some_and(|definition| definition != self.0.definition()) {
            return Err("prepared application definition changed".into());
        }
        self.0.project_roster(&roster)
    }
}

#[async_trait]
impl GatewayHostComposition for Driver {
    async fn prepared_application(&self, request: Option<&PreparedApplicationRequest>)
        -> Result<Option<PreparedGatewayApplication>, GatewayHostError>
    {
        self.current()?;
        let request = request.ok_or(GatewayHostError::Denied)?;
        if request.bundle != self.plan_bytes || request.declared_digest != self.plan_reference.sha256
            || self.selected.swap(true, Ordering::AcqRel)
        { return Err(GatewayHostError::Denied); }
        Ok(Some(PreparedGatewayApplication::new(self.plan_reference.sha256.clone(), self.compilation.definition().clone())
            .with_roster_projection(Arc::new(LiveRoster(self.compilation.clone())))))
    }
    fn native_grant_work_configuration(&self) -> Result<Option<NativeGrantWorkConfiguration>, GatewayHostError> {
        self.current()?;
        if !self.selected.load(Ordering::Acquire) { return Err(GatewayHostError::Denied); }
        self.configuration.lock().map_err(|_| GatewayHostError::Unavailable)?
            .take().map(Some).ok_or(GatewayHostError::Denied)
    }
    fn compose_member_factory(&self, construction: &GatewayFactoryConstruction<'_>)
        -> Result<Option<GatewayMcpComposition>, GatewayHostError>
    {
        self.current()?;
        if !self.selected.load(Ordering::Acquire) { return Err(GatewayHostError::Denied); }
        let realm = self.plan.application.realm.resolve().map_err(|_| GatewayHostError::Denied)?.locator;
        let composition = match &self.selected_policy {
            Some(policy) => crate::installed_service_review::composer::gateway_selected_policy_composition(
                construction, self.compilation.helper().member.clone(), realm, policy.clone(),
                self.presenters.clone(), self.plan.deployment_root.clone(), self.runtime.service_tasks()),
            None => crate::installed_service_review::composer::gateway_account_composition(
                construction, self.compilation.helper().member.clone(), realm,
                self.presenters.clone().ok_or(GatewayHostError::Unavailable)?, self.runtime.service_tasks()),
        }.map_err(|_| GatewayHostError::Unavailable)?;
        self.current()?;
        Ok(Some(composition.with_attempt_guards(self.attempt_guards.clone())))
    }
    async fn prepare_bootstrap(&self, spec: MobBootstrapSpec) -> Result<MobBootstrapSpec, GatewayHostError> {
        self.current()?;
        let expected = self.plan.application.realm.resolve().map_err(|_| GatewayHostError::Denied)?;
        if spec.definition != *self.compilation.definition()
            || spec.resolved_storage.as_ref().and_then(|storage| storage.state_dir.as_ref())
                != Some(&expected.layout.state_root().to_path_buf())
        { return Err(GatewayHostError::Denied); }
        let spec = self.compilation.prepare_helper_bootstrap(spec)
            .map_err(|_| GatewayHostError::Denied)?;
        self.runtime.prepare_bootstrap(spec)
    }
    async fn bind_runtime(&self, runtime: &mut UnifiedRuntime) -> Result<(), GatewayHostError> {
        self.current()?;
        self.runtime.bind_runtime(runtime)
    }
    async fn bind_identity_runtime(&self, runtime: &mut UnifiedRuntime) -> Result<(), GatewayHostError> {
        self.current()?;
        self.runtime.bind_identity_runtime(runtime)
    }
    fn identity_bootstrap_hold(&self) -> Option<Duration> { Some(Duration::from_millis(self.plan.hold_timeout_ms)) }
    async fn complete_held_identity_members(&self, members: Vec<HeldIdentityMember>,
        _completion: HeldCompletion,
    ) -> Result<(), GatewayHostError> {
        self.current()?;
        self.compilation.validate_finished(members.iter().map(|member| (&member.identity, &member.finished_surface)))
            .map_err(|_| GatewayHostError::Denied)?;
        // Each report belongs to this exact finished native member. Configured
        // keys constrain these observations; they never stand in for a client.
        let observations = self.plan.authorities.controllers.iter().map(|request| {
            let target = match &request.target {
                PreparedControllerTarget::Helper => &self.compilation.helper().member,
                PreparedControllerTarget::OriginalRoute { route } => {
                    let route = self.plan.owner.native_host.routes.iter().find(|row| &row.id == route)
                        .ok_or(GatewayHostError::Denied)?;
                    route.source_binding_in(&self.plan.owner.channels)
                        .map_err(|_| GatewayHostError::Denied)?.0
                }
            };
            let identity = self.compilation.held_member_identity(target).map_err(|_| GatewayHostError::Denied)?;
            let held = members.iter().find(|held| &held.identity == identity)
                .ok_or(GatewayHostError::Denied)?;
            let report = held.controller_route.report().ok_or(GatewayHostError::Unavailable)?;
            Ok((request.target.clone(), report.key.clone()))
        }).collect::<Result<Vec<_>, GatewayHostError>>()?;
        let helper_session = if self.plan.authorities.helper_catalog.is_some() {
            Some(self.compilation.held_helper_session(members.iter().map(|member| (&member.identity, &member.finished_surface)))
                .map_err(|_| GatewayHostError::Unavailable)?)
        } else { None };
        let authorities = self.plan.authorities.resolve_controller_routes(observations)
            .map_err(|error| match error {
                HostLifetimeError::Unavailable | HostLifetimeError::NotSettled => GatewayHostError::Unavailable,
                _ => GatewayHostError::Denied,
            })?;
        {
            // Linearize entry with the existing close owner, then release its
            // lock before native issuance/store work. Native retains and joins
            // this callback after close; issued effects are not rolled back.
            let publication = self.publication.lock().map_err(|_| GatewayHostError::Unavailable)?;
            if publication.closed { return Err(GatewayHostError::Denied); }
        }
        let mut helper = self.compilation.helper().clone();
        let selected = {
            let mut prepared = self.prepared.lock().map_err(|_| GatewayHostError::Unavailable)?;
            prepared.as_mut().ok_or(GatewayHostError::Denied)?
                .finalize_authorities_with_helper_resources(&mut helper, &authorities, Some(self.compilation.server()),
                    helper_session.as_ref().map(|session| (&self.compilation.helper().member, session)))
                .map_err(|error| match error {
                    HostLifetimeError::Unavailable | HostLifetimeError::NotSettled => GatewayHostError::Unavailable,
                    _ => GatewayHostError::Denied,
                })?
        };
        self.compilation.retain_authorities_with_helper_access(selected, helper, authorities.helper_mcp.as_ref(), authorities.helper_catalog.as_ref()).map_err(|_| GatewayHostError::Denied)?;
        self.current()?;
        let (setup, manifest) = self.compilation.complete_bundle(&self.plan.output).map_err(|_| GatewayHostError::Unavailable)?;
        self.completed.set(setup).map_err(|_| GatewayHostError::Denied)?;
        let send = self.deliver.lock().map_err(|_| GatewayHostError::Unavailable)?.take().ok_or(GatewayHostError::Denied)?;
        let receive = self.adopted.lock().map_err(|_| GatewayHostError::Unavailable)?.take().ok_or(GatewayHostError::Denied)?;
        send.send(manifest).map_err(|_| GatewayHostError::Unavailable)?;
        // Native HeldCompletion owns the pass deadline and calls begin_close
        // before joining this callback on Abort. It retains close responsibility
        // through the pass's decision, including after this callback returns.
        // Observe that actual close fence, not aborted() as a substitute for it.
        // Cancelling this observation does not cancel an entered publication.
        tokio::select! { biased;
            _ = self.closing() => Err(GatewayHostError::Denied),
            result = receive => { result.map_err(|_| GatewayHostError::Unavailable)?; self.current() }
        }
    }
    fn publish_bootstrap_ready(&self, publication: GatewayBootstrapReady<'_>) -> Result<(), GatewayHostError> {
        self.current()?;
        self.installed()?.publish_bootstrap_ready(publication)
    }
    fn host_app_routes(&self, viewer: meerkat_mobkit::http_console::ConsoleViewerResolver)
        -> Result<Option<axum::Router>, GatewayHostError>
    {
        self.current()?;
        self.installed()?.host_app_routes(viewer)
    }
    fn begin_close(&self, reason: GatewayHostCloseReason) -> Result<(), GatewayHostError> {
        self.runtime.begin_custody_close();
        let host = {
            let mut publication = self.publication.lock().map_err(|_| GatewayHostError::Unavailable)?;
            publication.closed = true;
            publication.installed.clone()
        };
        self.closed.send_replace(true);
        match host { Some(host) => host.begin_close(reason), None => Ok(()) }
    }
    async fn join_close(&self) -> Result<(), GatewayHostError> {
        self.runtime.begin_custody_close();
        let host = self.publication.lock().map(|publication| publication.installed.clone())
            .map_err(|_| GatewayHostError::Unavailable);
        let host_close = async {
            match host { Ok(Some(host)) => host.join_close().await, Ok(None) => Ok(()), Err(error) => Err(error) }
        };
        // Meta calls this after native transport shutdown. A failed host lookup
        // cannot abandon either the selected source task or local guard owner.
        let (host_result, source_result, local_result) = tokio::join!(
            host_close, self.runtime.join_custody_close(), self.join_local_round());
        host_result.and(source_result).and(local_result)
    }
    async fn wait_closed(&self) {
        let mut changed = self.closed.subscribe();
        loop {
            if let Ok(host) = self.installed() {
                tokio::select! { _ = self.closing() => {}, _ = host.wait_closed() => {} }
                return;
            }
            let is_closed = *changed.borrow();
            if is_closed || changed.changed().await.is_err() { return; }
        }
    }
}

struct RunningPrivate {
    private: PrivateHostConnection,
    presenters: Vec<tokio::task::JoinHandle<Result<(), crate::ConnectError>>>,
}
impl RunningPrivate {
    async fn finish(self) -> Result<(), String> {
        let mut failures = Vec::new();
        if let Err(error) = self.private.finish().await { failures.push(error.to_string()); }
        for task in self.presenters {
            match task.await {
                Ok(Ok(())) => (), Ok(Err(error)) => failures.push(error.to_string()),
                Err(error) => failures.push(error.to_string()),
            }
        }
        if failures.is_empty() { Ok(()) } else { Err(failures.join("; ")) }
    }
}

// Failure-only local diagnostic. Fixed labels and unit variants contain no owner data.
// Diagnostic I/O cannot change the original refusal or its cleanup path.
fn adoption_refusal(stage: &'static str, error: HostLifetimeError) -> HostLifetimeError {
    use std::io::Write as _;
    let cause = match error {
        HostLifetimeError::InvalidSelection => "InvalidSelection",
        HostLifetimeError::Unavailable => "Unavailable",
        HostLifetimeError::Denied => "Denied",
        HostLifetimeError::WrongOwner => "WrongOwner",
        HostLifetimeError::NotSettled => "NotSettled",
    };
    let _ = writeln!(std::io::stderr().lock(),
        "{{\"event\":\"toolkit_application_adoption_refused\",\"stage\":\"{stage}\",\"cause\":\"{cause}\"}}");
    error
}

async fn serve_private(
    mut startup: PrivateHostStartup, driver: Arc<Driver>,
    mut completed: oneshot::Receiver<SelectedHostArtifact>, adopted: oneshot::Sender<()>,
    terminal: Option<InheritedOperatorTerminal>, reviews: Option<LocalSetupReviews>,
) -> Result<RunningPrivate, String> {
    let manifest = tokio::select! { biased;
        _ = driver.closing() => return Err("native preparation closed before application completion".into()),
        error = startup.wait_lost() => return Err(error.to_string()),
        result = &mut completed => result.map_err(|_| "native preparation has no completed bundle")?,
    };
    let startup = tokio::select! { biased;
        _ = driver.closing() => return Err("native preparation closed before final selection".into()),
        result = startup.application_prepared(manifest) => result.map_err(|error| error.to_string())?,
    };
    // After receiving Bootstrap, adoption and any failure cleanup stay joined.
    // No cancellation branch drops an entered final owner construction.
    let prepared = driver.prepared.lock().map_err(|_| "prepared owner is unavailable")?
        .take().ok_or("prepared owner was already consumed")?;
    let staged = Arc::new(Mutex::new(None));
    let factory_stage = staged.clone();
    let factory_driver = driver.clone();
    let policy = prepared.policy();
    let finish_driver = driver.clone();
    let (host, private, local_reviews, remote_reviews) = PrivateHostConnection::bootstrap_prepared_with_service_parent(
        startup,
        move |selection| async move {
            factory_driver.current().map_err(|_| adoption_refusal("driver.factory_current", HostLifetimeError::Denied))?;
            let (owner, installed) = prepared.adopt(selection.clone())
                .map_err(|error| adoption_refusal("driver.prepared_adopt", error))?;
            let originals = installed.originals();
            *factory_stage.lock().map_err(|_| adoption_refusal("driver.stage_store_lock", HostLifetimeError::Unavailable))? = Some((installed, selection));
            Ok((owner, originals))
        },
        move |owner, closed, logins, service_parent| async move {
            let (installed, selection) = staged.lock().map_err(|_| adoption_refusal("driver.stage_take_lock", HostLifetimeError::Unavailable))?
                .take().ok_or_else(|| adoption_refusal("driver.staged_installation", HostLifetimeError::InvalidSelection))?;
            let native = owner.selected_policy().native_host.as_ref()
                .ok_or_else(|| adoption_refusal("driver.native_policy", HostLifetimeError::InvalidSelection))?;
            let selected = crate::application_acquisition::read_application_acquisition(native,
                |reference| toolkit_mobkit::installed_host::read_bundle_artifact(&selection, reference))
                .map_err(|error| adoption_refusal("driver.read_acquisition", error))?
                .ok_or_else(|| adoption_refusal("driver.selected_acquisition", HostLifetimeError::InvalidSelection))?;
            let completed = finish_driver.completed.get()
                .ok_or_else(|| adoption_refusal("driver.completed_setup", HostLifetimeError::InvalidSelection))?;
            if !selected.matches_preparation(&finish_driver.plan.acquisition, completed)
                .map_err(|error| adoption_refusal("driver.compare_preparation", error))? {
                return Err(adoption_refusal("driver.preparation_matches", HostLifetimeError::InvalidSelection));
            }
            let (installed, acquisition) = crate::application_acquisition::prepare_gateway_acquisition_with_retained(
                &selection, native, installed,
                Some((finish_driver.runtime.tools(), finish_driver.runtime.continuation())),
            ).map_err(|error| adoption_refusal("driver.prepare_acquisition", error))?;
            let acquisition = acquisition
                .ok_or_else(|| adoption_refusal("driver.prepared_acquisition", HostLifetimeError::InvalidSelection))?;
            let host = installed.compose_with_prepared_registration(owner.clone(), closed,
                finish_driver.registration.clone(), policy)
                .map_err(|error| adoption_refusal("driver.compose_registration", error))?
                .with_console_logins(logins)
                .map_err(|error| adoption_refusal("driver.compose_console_logins", error))?;
            let host = acquisition.compose_prepared_factory(host, finish_driver.presenters.as_ref(),
                finish_driver.selected_policy.as_ref(), owner, service_parent)
                .map_err(|error| adoption_refusal("driver.compose_acquisition", error))?;
            let host = Arc::new(finish_driver.runtime.adopt(host).await
                .map_err(|error| adoption_refusal("driver.runtime_adopt", error))?);
            finish_driver.retain_installed(host.clone())
                .map_err(|error| adoption_refusal("driver.retain_installed", error))?;
            Ok(host)
        },
    ).await.map_err(|error| error.to_string())?;
    let service_parent = match private.service_parent_endpoint() {
        Ok(endpoint) => endpoint,
        Err(error) => {
            let cleanup = private.finish().await;
            return Err(match cleanup {
                Ok(()) => error.to_string(),
                Err(cleanup) => format!("{error}; private host cleanup also failed: {cleanup}"),
            });
        }
    };
    let mut tasks = Vec::new();
    tasks.push(tokio::spawn(crate::application_remote::serve_remote_operator_with_service_parent(
        host.clone(), remote_reviews, driver.plan.deployment_root.clone(), private.closed_observer(),
        service_parent,
    )));
    match (terminal, reviews) {
        (Some(terminal), Some(reviews)) => tasks.push(tokio::spawn(crate::application_gateway::serve_local_operator_with_setup(
            host, local_reviews, terminal, driver.plan.deployment_root.clone(), private.closed_observer(), reviews,
        ))),
        (None, Some(reviews)) => {
            drop(local_reviews);
            if driver.plan.acquisition.console_setup.is_some() {
                tasks.push(tokio::spawn(crate::application_gateway::serve_console_setup(reviews, private.closed_observer())));
            } else { drop(reviews); }
        }
        (_, None) => { drop(local_reviews); }
    }
    let running = RunningPrivate { private, presenters: tasks };
    if adopted.send(()).is_err() {
        let cleanup = running.finish().await;
        return Err(match cleanup { Ok(()) => "native held completion was lost after final adoption".into(),
            Err(error) => format!("native held completion was lost; private cleanup also failed: {error}") });
    }
    Ok(running)
}

/// Actual executable entry for the private PrepareApplication request. The SDK
/// concurrently runs ordinary init and all live providers on this same child.
pub async fn run(
    mut startup: PrivateHostStartup, terminal: Option<InheritedOperatorTerminal>,
    control: Option<ControlListenAddr>,
) -> Result<(), String> {
    let PrivateHostStartupIntent::PrepareApplication(reference) = startup.intent().clone() else {
        return Err("schema4 driver requires its exact preparation request".into());
    };
    let bytes = crate::application_setup::read_regular_artifact(&reference.path)?;
    if crate::application_bootstrap::hash(&bytes) != reference.sha256 { return Err("preparation input changed".into()); }
    let mut plan: ApplicationPreparationPlan = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    if plan.schema != 1 || plan.hold_timeout_ms == 0 || plan.hold_timeout_ms > 900_000
        || !plan.deployment_root.is_absolute() || !plan.output.is_absolute()
    { return Err("invalid application preparation plan".into()); }
    plan.owner.validate().map_err(|error| error.to_string())?;
    plan.acquisition.validate().map_err(|error| error.to_string())?;
    if plan.acquisition.standing_policy.is_some() && !plan.application.helper_tools.requires_software_review() {
        return Err("standing acquisition policy requires the selected checked setup surface".into());
    }
    if plan.application.helper_tools.requires_software_review()
        && terminal.is_none() && plan.acquisition.console_setup.is_none() && plan.acquisition.standing_policy.is_none()
    {
        return Err("checked helper installation requires the selected terminal or Console presenter".into());
    }
    if plan.owner.native_host.members != plan.application.members
        || plan.owner.native_host.realm.as_str() != plan.application.realm.realm_id
        || (plan.application.helper.controller_parent.is_empty()
            && !plan.authorities.controllers.iter().any(|request| request.target == PreparedControllerTarget::Helper))
    { return Err("preparation owner and application selection differ or lack a helper controller request".into()); }
    // Validate the unbound compiler input before opening any native owner.
    // Held construction keeps route authority unavailable until actual reports
    // resolve the retained requests and finalization precedes publication.
    let validation = plan.application.clone().validate()?;
    // Open the canonical pair once. It issues no route-dependent authority
    // until the same runtime returns the selected members' actual held reports.
    let mut owner = PreparedInstallationOwner::open(
        plan.deployment_root.clone(), plan.owner.clone(), plan.startup.clone())
        .map_err(|error| error.to_string())?;
    let bounds = McpAttemptGuardBounds::new(
        Duration::from_secs(60), Duration::from_secs(5), Duration::from_secs(5),
    ).map_err(|error| error.to_string())?;
    let resources = InstalledResources::locate().map_err(|error| error.to_string())?;
    let (prepared_resources, mut local_owner) = resources.prepare_with_local_round_owner(validation.storage(), async {
        let _error = startup.wait_lost().await; Ok(())
    }).await.map_err(|error| error.to_string())?;
    let mut compilation = match validation.prepare(prepared_resources) {
        Ok(compilation) => compilation,
        Err(error) => return Err(local_round::preparation_failed(error, &mut local_owner).await),
    };
    // Retain the actual fresh owner before compilation creates shared or weak
    // resource references. Reopening an installed descriptor cannot replace it.
    let attempts = match compilation.retain_local_round_owner(local_owner) {
        Ok(attempts) => attempts,
        Err((mut owner, error)) => return Err(local_round::preparation_failed(error, &mut owner).await),
    };
    let guards = McpAttemptGuards::new(attempts.clone(), bounds);
    let compilation = Arc::new(compilation);
    let prepared_driver = async {
        let selected = compilation.project_policy(plan.owner.clone())?;
        owner.project_helper_member(&plan.application.helper.member, &compilation.helper().member)
            .map_err(|error| error.to_string())?;
        if owner.selected() != &selected { return Err("prepared owner projection changed".into()); }
        plan.authorities = plan.authorities.project_helper_member(
            &plan.application.helper.member, &compilation.helper().member);
        plan.owner = selected;
        let selected_policy = plan.acquisition.standing_policy.map(|selection|
            crate::application_acquisition::selected_policy::SelectedPolicyBinding::new(selection,
                plan.owner.native_host.grant_administrator.clone(), plan.owner.deployment.clone(), plan.owner.native_host.realm.clone(),
                plan.owner.native_host.members.iter().map(|row| row.member.member.clone()).collect()));
        let recovery_participant = selected_policy.as_ref().map(|binding| binding.participant()).or_else(||
            plan.application.helper_tools.requires_software_review().then(||
                crate::application_acquisition::selected_policy::SelectedPolicyBinding::human_review_participant(
                    plan.owner.native_host.grant_administrator.clone(), plan.owner.deployment.clone(), plan.owner.native_host.realm.clone(),
                    plan.owner.native_host.members.iter().map(|row| row.member.member.clone()).collect())));
        let prepared = match recovery_participant {
            Some(participant) => owner.prepare_with_application_service_policy(
                compilation.prepared_tool_policy(), participant).await,
            None => owner.prepare(compilation.prepared_tool_policy()).await,
        }.map_err(|error| error.to_string())?;
        let resources = compilation.clone();
        let registration = Arc::new(PreparedMemberRegistration::new(compilation.clone(), compilation.registry(),
            Arc::new(move |identity, destination| resources.selected_destination(identity, destination))));
        let tools = InstalledAcquisitionTools::new(Duration::from_millis(plan.acquisition.limits.admission_timeout_ms))
            .map_err(|error| error.to_string())?;
        let runtime = prepared.gateway_runtime(registration.clone(), tools, plan.acquisition.output_audience.clone())
            .map_err(|error| error.to_string())?;
        let mut configuration = prepared.take_configuration().map_err(|error| error.to_string())?;
        configuration.configured_mcp_attachment = Some(Arc::new(crate::application_setup::resource::SetupResourceProvider::new(
            compilation.resource_selection(), configuration.configured_mcp_attachment.take(),
        )));
        let (presenters, reviews) = if retain_presenter_channel(plan.acquisition.standing_policy.as_ref(),
            terminal.is_some(), plan.acquisition.console_setup.is_some()) {
            let (presenters, reviews) = crate::application_gateway::setup_presenter::channel(plan.deployment_root.clone());
            let presenters = if plan.acquisition.console_setup.is_some() { presenters.with_pending_console() } else { presenters };
            (Some(presenters), Some(reviews))
        } else { (None, None) };
        Ok::<_, String>((registration, runtime, configuration, prepared, presenters, selected_policy, reviews))
    }.await;
    let (registration, runtime, configuration, prepared, presenters, selected_policy, reviews) = match prepared_driver {
        Ok(prepared) => prepared,
        Err(error) => {
            // No native factory has received the installation yet. Closing it
            // recovers this exact unentered owner after any preparation refusal.
            let mut cleanup = LocalRoundCleanup::default();
            let joined = cleanup.join_until_settled(&attempts, &guards).await;
            return Err(match joined {
                Ok(()) => error,
                Err(_) => format!("{error}; local preparation cleanup remains unavailable"),
            });
        }
    };
    let (deliver, completed) = oneshot::channel();
    let (adopted, receive) = oneshot::channel();
    let (closed, _) = watch::channel(false);
    let driver = Arc::new(Driver { plan, plan_reference: reference, plan_bytes: bytes,
        compilation, registration, runtime, configuration: Mutex::new(Some(configuration)),
        prepared: Mutex::new(Some(prepared)), presenters, selected_policy,
        publication: Mutex::new(Publication { closed: false, installed: None }), closed,
        selected: AtomicBool::new(false), completed: OnceLock::new(),
        deliver: Mutex::new(Some(deliver)), adopted: Mutex::new(Some(receive)),
        attempt_guards: guards, local_round_attempts: attempts,
        local_round_cleanup: tokio::sync::Mutex::new(LocalRoundCleanup::default()) });
    let actor_driver = driver.clone();
    let actor = tokio::spawn(async move {
        let result = serve_private(startup, actor_driver.clone(), completed, adopted, terminal, reviews).await;
        if result.is_err() { let _ = actor_driver.begin_close(GatewayHostCloseReason::HostDisconnected); }
        result
    });
    let native = run_persistent_with_host(control, driver.clone()).await;
    let closing = driver.begin_close(GatewayHostCloseReason::Shutdown);
    let private = match actor.await {
        Ok(Ok(running)) => running.finish().await,
        Ok(Err(error)) => Err(error), Err(error) => Err(error.to_string()),
    };
    let mut failures = Vec::new();
    if let Err(error) = native { failures.push(error.to_string()); }
    if let Err(error) = closing { failures.push(error.to_string()); }
    if let Err(error) = private { failures.push(error); }
    if failures.is_empty() { Ok(()) } else { Err(failures.join("; ")) }
}

// Absence retains the legacy channel even for an inert helper without a UI.
// Only an explicitly selected headless policy composition omits the channel.
fn retain_presenter_channel(
    selected: Option<&crate::application_acquisition::selected_policy::StandingPolicySelection>,
    terminal: bool, console: bool,
) -> bool { selected.is_none() || terminal || console }

#[cfg(test)]
mod standing_presentation_tests {
    use super::*;
    use crate::application_acquisition::selected_policy::{StandingPolicySelection, InstallationSelection, AccessSelection};

    #[test]
    fn absent_selection_retains_legacy_channel_and_selected_headless_omits_it() {
        let selection = StandingPolicySelection { installation: InstallationSelection::CheckedSoftware,
            access: AccessSelection::ConfiguredParent, tool_policy: None };
        for terminal in [false, true] {
            for console in [false, true] {
                assert!(retain_presenter_channel(None, terminal, console));
                assert_eq!(retain_presenter_channel(Some(&selection), terminal, console), terminal || console);
            }
        }
    }
}
