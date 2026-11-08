//! One schema4 preparation and installed runtime in the same gateway process.
//! Native held completion supplies the finished surfaces. The private channel
//! supplies final application publication; neither substitutes for the other.

use crate::{
    application_acquisition::ApplicationAcquisitionParameters,
    application_gateway::{InheritedOperatorTerminal, LocalSetupApprovals, LocalSetupReviews},
    application_setup::{
        PreparedApplicationSetup,
        compilation::{ApplicationSetupCompilation, ApplicationSetupCompilationInput},
    },
    resources::{InstalledResources, LocalRoundAttempts, LocalRoundOwner},
};
use async_trait::async_trait;
use meerkat_mcp::{McpAttemptGuardBounds, McpAttemptGuards};
use meerkat_mobkit::{
    UnifiedRuntime,
    identity_first::{
        DurableAgentSpec, HeldCompletion, HeldIdentityMember, RosterContext,
        contracts::RosterProjection,
    },
    mob_handle_runtime::MobBootstrapSpec,
    rpc_gateway::{
        GatewayBootstrapReady, GatewayFactoryConstruction, GatewayHostCloseReason,
        GatewayHostComposition, GatewayHostError, GatewayMcpComposition,
        PreparedApplicationRequest, PreparedGatewayApplication, run_persistent_with_host,
    },
    runtime::cross_mob_control::ControlListenAddr,
};
use meerkat_runtime::meerkat_machine::NativeGrantWorkConfiguration;
use meerkat_toolkit::host_lifetime::{
    control::{HostLifetimeError, SelectedHostArtifact},
    policy::PreparedLocalPolicy,
};
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::{oneshot, watch};
use toolkit_mobkit::{
    host_gateway::{PreparedGatewayRuntime, ToolkitGatewayHost},
    installed_host::{
        PreparedInstallationOwner, PreparedInstalledHost,
        acquisition::InstalledAcquisitionTools,
        operator_setup::OperatorOwnerStartup,
        preparation::{PreparationAuthorityIntent, PreparedControllerTarget},
        prepared_registration::PreparedMemberRegistration,
    },
    private_host::{PrivateHostConnection, PrivateHostStartup, PrivateHostStartupIntent},
};

pub(crate) mod local_round;
use local_round::LocalRoundCleanup;

/// Trusted fresh-application preparation input on the inherited channel. The owner
/// facts contain no surface references; native construction produces those.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApplicationPreparationPlan {
    pub schema: u32,
    pub deployment_root: PathBuf,
    pub owner: PreparedLocalPolicy,
    pub startup: OperatorOwnerStartup,
    #[serde(default)]
    pub authorities: PreparationAuthorityIntent,
    pub application: ApplicationSetupCompilationInput,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acquisition: Option<ApplicationAcquisitionParameters>,
    pub output: PathBuf,
    pub hold_timeout_ms: u64,
}

impl Serialize for ApplicationPreparationPlan {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut value = serializer.serialize_struct(
            "ApplicationPreparationPlan",
            8 + usize::from(self.acquisition.is_some()),
        )?;
        value.serialize_field("schema", &self.schema)?;
        value.serialize_field("deployment_root", &self.deployment_root)?;
        value.serialize_field("owner", &self.owner)?;
        value.serialize_field("startup", &self.startup)?;
        if self.application.helper.is_some() {
            value.serialize_field("authorities", &self.authorities)?;
        } else {
            #[derive(Serialize)]
            struct HelperlessAuthorities<'a> {
                #[serde(skip_serializing_if = "slice_is_empty")]
                controllers:
                    &'a [toolkit_mobkit::installed_host::preparation::PreparedControllerIntent],
            }
            fn slice_is_empty<T>(rows: &&[T]) -> bool {
                rows.is_empty()
            }
            // Reject incompatible typed input rather than dropping requested
            // authority when serializing the explicitly helperless wire.
            self.authorities
                .validate_without_helper()
                .map_err(serde::ser::Error::custom)?;
            value.serialize_field(
                "authorities",
                &HelperlessAuthorities {
                    controllers: &self.authorities.controllers,
                },
            )?;
        }
        value.serialize_field("application", &self.application)?;
        if let Some(acquisition) = &self.acquisition {
            value.serialize_field("acquisition", acquisition)?;
        }
        value.serialize_field("output", &self.output)?;
        value.serialize_field("hold_timeout_ms", &self.hold_timeout_ms)?;
        value.end()
    }
}

fn read_plan(bytes: &[u8]) -> Result<ApplicationPreparationPlan, String> {
    let wire: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
    let plan: ApplicationPreparationPlan =
        serde_json::from_value(wire.clone()).map_err(|error| error.to_string())?;
    if plan.application.helper.is_none()
        && wire
            .get("authorities")
            .and_then(serde_json::Value::as_object)
            .is_some_and(|fields| {
                [
                    "helper_mcp",
                    "helper_catalog",
                    "helper_report",
                    "agent_setup",
                    "shared_workflow_setup",
                ]
                .iter()
                .any(|field| fields.contains_key(*field))
            })
    {
        return Err("helperless preparation must omit helper-only authority fields".into());
    }
    Ok(plan)
}

struct Publication {
    closed: bool,
    admission_stopped: bool,
    installed: Option<Arc<ToolkitGatewayHost>>,
}

struct Driver {
    operation_review_policy:
        Option<Arc<dyn toolkit_mobkit::installed_host::InstalledOperationReviewPolicy>>,
    operation_review: Option<crate::application_consent::RetainedApplicationOperationReview>,
    plan: ApplicationPreparationPlan,
    plan_reference: SelectedHostArtifact,
    plan_bytes: Vec<u8>,
    compilation: Arc<ApplicationSetupCompilation>,
    registration: Arc<PreparedMemberRegistration>,
    runtime: PreparedGatewayRuntime,
    configuration: Mutex<Option<NativeGrantWorkConfiguration>>,
    prepared: Mutex<Option<PreparedInstalledHost>>,
    presenters: Option<LocalSetupApprovals>,
    selected_policy:
        Option<Arc<crate::application_acquisition::selected_policy::SelectedPolicyBinding>>,
    publication: Mutex<Publication>,
    closed: watch::Sender<bool>,
    selected: AtomicBool,
    completed: OnceLock<PreparedApplicationSetup>,
    deliver: Mutex<Option<oneshot::Sender<SelectedHostArtifact>>>,
    adopted: Mutex<Option<oneshot::Receiver<()>>>,
    local_round: Option<(Arc<LocalRoundAttempts>, McpAttemptGuards)>,
    local_round_cleanup: tokio::sync::Mutex<LocalRoundCleanup>,
    member_class_context: OnceLock<meerkat_mobkit::identity_first::PublicIdentityFirstContext>,
    workflow:
        Option<Arc<crate::application_acquisition::workflow::ApplicationWorkflowCommissioner>>,
    workflow_resources: Option<Arc<crate::installed_service_review::composer::LiveSharedSetup>>,
    hosted_mcp: Option<(
        toolkit_mobkit::private_host::HostedMcpAuthEndpoint,
        meerkat::McpRegisteredRedirectBase,
    )>,
    #[cfg(test)]
    late_adoption: Option<Arc<late_adoption_tests::Probe>>,
}

impl Driver {
    fn selected_operation_review(
        &self,
    ) -> Result<Option<Arc<meerkat_core::approval::review::BoundOperationReview>>, GatewayHostError>
    {
        self.current()?;
        if !self.selected.load(Ordering::Acquire) {
            return Err(GatewayHostError::Denied);
        }
        let review = self
            .operation_review
            .as_ref()
            .map(|selection| {
                selection
                    .current()
                    .map_err(|_| GatewayHostError::Unavailable)
            })
            .transpose()?;
        self.current()?;
        Ok(review)
    }

    fn current(&self) -> Result<(), GatewayHostError> {
        if self
            .publication
            .lock()
            .map_err(|_| GatewayHostError::Unavailable)?
            .closed
        {
            return Err(GatewayHostError::Denied);
        }
        Ok(())
    }
    fn installed(&self) -> Result<Arc<ToolkitGatewayHost>, GatewayHostError> {
        self.publication
            .lock()
            .map_err(|_| GatewayHostError::Unavailable)?
            .installed
            .clone()
            .ok_or(GatewayHostError::Unavailable)
    }
    fn retain_installed(&self, host: Arc<ToolkitGatewayHost>) -> Result<(), HostLifetimeError> {
        let mut publication = self
            .publication
            .lock()
            .map_err(|_| HostLifetimeError::Unavailable)?;
        if publication.closed || publication.installed.is_some() {
            return Err(HostLifetimeError::Denied);
        }
        // A held bootstrap may finish during the native drain window. Keep its
        // completion, but fence the adopted host before exposing its sources.
        if publication.admission_stopped {
            host.stop_admitting_work().map_err(|error| match error {
                GatewayHostError::Unavailable => HostLifetimeError::Unavailable,
                _ => HostLifetimeError::Denied,
            })?;
        }
        #[cfg(test)]
        if let Some(probe) = &self.late_adoption {
            probe
                .retained(&host, publication.admission_stopped)
                .map_err(|_| HostLifetimeError::Unavailable)?;
        }
        publication.installed = Some(host);
        // Wake the existing close observer to start observing this exact host.
        self.closed.send_replace(false);
        Ok(())
    }
    async fn closing(&self) {
        let mut closed = self.closed.subscribe();
        if *closed.borrow() {
            return;
        }
        while closed.changed().await.is_ok() {
            if *closed.borrow() {
                return;
            }
        }
    }
}

struct LiveRoster(Arc<ApplicationSetupCompilation>);
impl RosterProjection for LiveRoster {
    fn project(
        &self,
        context: &RosterContext,
        roster: Vec<DurableAgentSpec>,
    ) -> Result<Vec<DurableAgentSpec>, String> {
        if context
            .mob_definition
            .as_ref()
            .is_some_and(|definition| definition != self.0.definition())
        {
            return Err("prepared application definition changed".into());
        }
        self.0.project_roster(&roster)
    }
}

#[async_trait]
impl GatewayHostComposition for Driver {
    fn compose_agent_customizer(
        &self,
        customizer: Option<Arc<dyn meerkat_mobkit::identity_first::AgentCustomizer>>,
    ) -> Result<Option<Arc<dyn meerkat_mobkit::identity_first::AgentCustomizer>>, GatewayHostError>
    {
        self.current()?;
        if !self.selected.load(Ordering::Acquire) {
            return Err(GatewayHostError::Denied);
        }
        if customizer.is_none() && !self.plan.application.member_classes.is_empty() {
            return Err(GatewayHostError::AgentCustomizerRequired);
        }
        // Meta installs this same composed value for fresh, held and rebuilt
        // members. Ordinary applications may leave the SDK customizer absent.
        Ok(customizer.map(|inner| {
            let inner = self
                .compilation
                .prepared_tool_policy()
                .member_class_gateway_customizer(inner);
            match &self.workflow_resources {
                Some(resources) => resources.customizer(inner),
                None => inner,
            }
        }))
    }
    async fn prepared_application(
        &self,
        request: Option<&PreparedApplicationRequest>,
    ) -> Result<Option<PreparedGatewayApplication>, GatewayHostError> {
        self.current()?;
        let request = request.ok_or(GatewayHostError::Denied)?;
        if request.bundle != self.plan_bytes
            || request.declared_digest != self.plan_reference.sha256
            || self.selected.swap(true, Ordering::AcqRel)
        {
            return Err(GatewayHostError::Denied);
        }
        Ok(Some(
            PreparedGatewayApplication::new(
                self.plan_reference.sha256.clone(),
                self.compilation.definition().clone(),
            )
            .with_roster_projection(Arc::new(LiveRoster(self.compilation.clone()))),
        ))
    }
    fn native_grant_work_configuration(
        &self,
    ) -> Result<Option<NativeGrantWorkConfiguration>, GatewayHostError> {
        self.current()?;
        if !self.selected.load(Ordering::Acquire) {
            return Err(GatewayHostError::Denied);
        }
        self.configuration
            .lock()
            .map_err(|_| GatewayHostError::Unavailable)?
            .take()
            .map(Some)
            .ok_or(GatewayHostError::Denied)
    }
    fn compose_member_factory(
        &self,
        construction: &GatewayFactoryConstruction<'_>,
    ) -> Result<Option<GatewayMcpComposition>, GatewayHostError> {
        self.current()?;
        if !self.selected.load(Ordering::Acquire) {
            return Err(GatewayHostError::Denied);
        }
        self.runtime
            .capture_member_mcp_factory(construction)
            .map_err(|_| GatewayHostError::Unavailable)?;
        if let Some((endpoint, base)) = &self.hosted_mcp {
            endpoint
                .bind_factory(construction, base.clone())
                .map_err(|_| GatewayHostError::Unavailable)?;
        }
        let Some(helper) = self.compilation.helper() else {
            return Ok(None);
        };
        let realm = self
            .plan
            .application
            .realm
            .resolve()
            .map_err(|_| GatewayHostError::Denied)?
            .locator;
        let composition = match &self.selected_policy {
            Some(policy) => crate::installed_service_review::composer::gateway_selected_policy_composition_with_workflows(
                construction, helper.member.clone(), realm, policy.clone(),
                self.presenters.clone(), self.plan.deployment_root.clone(), self.runtime.service_tasks(),
                self.workflow_resources.clone()),
            None => crate::installed_service_review::composer::gateway_account_composition(
                construction, helper.member.clone(), realm,
                self.presenters.clone().ok_or(GatewayHostError::Unavailable)?, self.runtime.service_tasks()),
        }.map_err(|_| GatewayHostError::Unavailable)?;
        self.current()?;
        Ok(Some(
            composition.with_attempt_guards(
                self.local_round
                    .as_ref()
                    .ok_or(GatewayHostError::Unavailable)?
                    .1
                    .clone(),
            ),
        ))
    }
    fn operation_review(
        &self,
        _construction: &GatewayFactoryConstruction<'_>,
    ) -> Result<Option<Arc<meerkat_core::approval::review::BoundOperationReview>>, GatewayHostError>
    {
        // Meta offers the same construction to compose_member_factory first.
        // Its installation realm is not the prepared member realm. Selection
        // was bound to the actual application owner before either callback.
        // Meta installs this exact review's settlement before any session and
        // then retains the same Arc on its factory, for both init paths.
        self.selected_operation_review()
    }
    async fn prepare_bootstrap(
        &self,
        spec: MobBootstrapSpec,
    ) -> Result<MobBootstrapSpec, GatewayHostError> {
        self.current()?;
        let expected = self
            .plan
            .application
            .realm
            .resolve()
            .map_err(|_| GatewayHostError::Denied)?;
        if spec.definition != *self.compilation.definition()
            || spec
                .resolved_storage
                .as_ref()
                .and_then(|storage| storage.state_dir.as_ref())
                != Some(&expected.layout.state_root().to_path_buf())
        {
            return Err(GatewayHostError::Denied);
        }
        let spec = self
            .compilation
            .prepare_helper_bootstrap(spec)
            .map_err(|_| GatewayHostError::Denied)?;
        let spec = match &self.workflow_resources {
            Some(resources) => {
                resources
                    .capture_native(&spec)
                    .map_err(|_| GatewayHostError::Denied)?;
                spec.with_spawn_member_customizer(resources.clone())
            }
            None => spec,
        };
        self.runtime.prepare_bootstrap(spec)
    }
    async fn bind_runtime(&self, runtime: &mut UnifiedRuntime) -> Result<(), GatewayHostError> {
        self.current()?;
        self.runtime.bind_runtime(runtime)
    }
    async fn bind_identity_runtime(
        &self,
        runtime: &mut UnifiedRuntime,
    ) -> Result<(), GatewayHostError> {
        self.current()?;
        self.runtime.bind_identity_runtime(runtime)?;
        self.member_class_context
            .set(
                runtime
                    .identity_first_context()
                    .ok_or(GatewayHostError::Unavailable)?,
            )
            .map_err(|_| GatewayHostError::Denied)
    }
    fn identity_bootstrap_hold(&self) -> Option<Duration> {
        Some(Duration::from_millis(self.plan.hold_timeout_ms))
    }
    async fn complete_held_identity_members(
        &self,
        members: Vec<HeldIdentityMember>,
        _completion: HeldCompletion,
    ) -> Result<(), GatewayHostError> {
        self.current()?;
        self.compilation
            .validate_finished(
                members
                    .iter()
                    .map(|member| (&member.identity, &member.finished_surface)),
            )
            .map_err(|_| GatewayHostError::Denied)?;
        // Each report belongs to this exact finished native member. Configured
        // keys constrain these observations; they never stand in for a client.
        let observations = self
            .plan
            .authorities
            .controllers
            .iter()
            .map(|request| {
                let target = match &request.target {
                    PreparedControllerTarget::Helper => {
                        &self
                            .compilation
                            .helper()
                            .ok_or(GatewayHostError::Denied)?
                            .member
                    }
                    PreparedControllerTarget::OriginalRoute { route } => {
                        let route = self
                            .plan
                            .owner
                            .native_host
                            .routes
                            .iter()
                            .find(|row| &row.id == route)
                            .ok_or(GatewayHostError::Denied)?;
                        route
                            .source_binding_in(&self.plan.owner.channels)
                            .map_err(|_| GatewayHostError::Denied)?
                            .0
                    }
                };
                let identity = self
                    .compilation
                    .held_member_identity(target)
                    .map_err(|_| GatewayHostError::Denied)?;
                let held = members
                    .iter()
                    .find(|held| &held.identity == identity)
                    .ok_or(GatewayHostError::Denied)?;
                let report = held
                    .controller_route
                    .report()
                    .ok_or(GatewayHostError::Unavailable)?;
                Ok((request.target.clone(), report.key.clone()))
            })
            .collect::<Result<Vec<_>, GatewayHostError>>()?;
        let helper_session = if self.plan.authorities.helper_catalog.is_some() {
            Some(
                self.compilation
                    .held_helper_session(
                        members
                            .iter()
                            .map(|member| (&member.identity, &member.finished_surface)),
                    )
                    .map_err(|_| GatewayHostError::Unavailable)?,
            )
        } else {
            None
        };
        let authorities = self
            .plan
            .authorities
            .resolve_controller_routes(observations)
            .map_err(|error| match error {
                HostLifetimeError::Unavailable | HostLifetimeError::NotSettled => {
                    GatewayHostError::Unavailable
                }
                _ => GatewayHostError::Denied,
            })?;
        {
            // Linearize entry with the existing close owner, then release its
            // lock before native issuance/store work. Native retains and joins
            // this callback after close; issued effects are not rolled back.
            let publication = self
                .publication
                .lock()
                .map_err(|_| GatewayHostError::Unavailable)?;
            if publication.closed {
                return Err(GatewayHostError::Denied);
            }
        }
        let mut helper = self.compilation.helper().cloned();
        let selected = {
            let mut prepared = self
                .prepared
                .lock()
                .map_err(|_| GatewayHostError::Unavailable)?;
            let prepared = prepared.as_mut().ok_or(GatewayHostError::Denied)?;
            match helper.as_mut() {
                Some(helper) => prepared.finalize_authorities_with_helper_resources(
                    helper,
                    &authorities,
                    self.compilation.server(),
                    helper_session.as_ref().map(|session| {
                        (
                            &self.compilation.helper().expect("selected helper").member,
                            session,
                        )
                    }),
                ),
                None => prepared.finalize_authorities_without_helper(&authorities),
            }
            .map_err(|error| match error {
                HostLifetimeError::Unavailable | HostLifetimeError::NotSettled => {
                    GatewayHostError::Unavailable
                }
                _ => GatewayHostError::Denied,
            })?
        };
        match helper {
            Some(helper) => self.compilation.retain_authorities_with_helper_access(
                selected,
                helper,
                authorities.helper_mcp.as_ref(),
                authorities.helper_catalog.as_ref(),
            ),
            None => self.compilation.retain_owner(selected),
        }
        .map_err(|_| GatewayHostError::Denied)?;
        self.current()?;
        let (setup, manifest) = self
            .compilation
            .complete_bundle(&self.plan.output)
            .map_err(|_| GatewayHostError::Unavailable)?;
        self.completed
            .set(setup)
            .map_err(|_| GatewayHostError::Denied)?;
        let send = self
            .deliver
            .lock()
            .map_err(|_| GatewayHostError::Unavailable)?
            .take()
            .ok_or(GatewayHostError::Denied)?;
        let receive = self
            .adopted
            .lock()
            .map_err(|_| GatewayHostError::Unavailable)?
            .take()
            .ok_or(GatewayHostError::Denied)?;
        send.send(manifest)
            .map_err(|_| GatewayHostError::Unavailable)?;
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
    fn publish_bootstrap_ready(
        &self,
        publication: GatewayBootstrapReady<'_>,
    ) -> Result<(), GatewayHostError> {
        self.current()?;
        let host = self.installed()?;
        #[cfg(test)]
        if let Some(probe) = &self.late_adoption {
            probe.observe("before_ready", &host)?;
        }
        host.publish_bootstrap_ready(publication)?;
        #[cfg(test)]
        if let Some(probe) = &self.late_adoption {
            probe.observe("after_ready", &host)?;
        }
        Ok(())
    }
    fn host_app_routes(
        &self,
        viewer: meerkat_mobkit::http_console::ConsoleViewerResolver,
    ) -> Result<Option<axum::Router>, GatewayHostError> {
        self.current()?;
        self.installed()?.host_app_routes(viewer)
    }
    fn stop_admitting(&self, _reason: GatewayHostCloseReason) -> Result<(), GatewayHostError> {
        // The prepared runtime already owns sources before an installed host
        // exists. Fence it first, then serialize adoption with the same fence.
        // Actual close and its watch notification still belong to begin_close.
        let runtime_result = self.runtime.stop_admitting_work();
        let mut publication = self
            .publication
            .lock()
            .map_err(|_| GatewayHostError::Unavailable)?;
        publication.admission_stopped = true;
        let host_result = match &publication.installed {
            Some(host) => host.stop_admitting_work(),
            None => Ok(()),
        };
        let result = runtime_result.and(host_result);
        drop(publication);
        #[cfg(test)]
        if result.is_ok()
            && let Some(probe) = &self.late_adoption
        {
            probe.stopped(self)?;
        }
        result
    }
    fn begin_close(&self, reason: GatewayHostCloseReason) -> Result<(), GatewayHostError> {
        if let Some(resources) = &self.workflow_resources {
            resources.begin_close();
        }
        self.runtime.begin_custody_close();
        let host = {
            let mut publication = self
                .publication
                .lock()
                .map_err(|_| GatewayHostError::Unavailable)?;
            publication.closed = true;
            publication.installed.clone()
        };
        self.closed.send_replace(true);
        match host {
            Some(host) => host.begin_close(reason),
            None => Ok(()),
        }
    }
    async fn join_close(&self) -> Result<(), GatewayHostError> {
        self.runtime.begin_custody_close();
        let host = self
            .publication
            .lock()
            .map(|publication| publication.installed.clone())
            .map_err(|_| GatewayHostError::Unavailable);
        let host_close = async {
            match host {
                Ok(Some(host)) => host.join_close().await,
                Ok(None) => Ok(()),
                Err(error) => Err(error),
            }
        };
        // Meta calls this after native transport shutdown. A failed host lookup
        // cannot abandon either the selected source task or local guard owner.
        let (host_result, source_result, local_result) = tokio::join!(
            host_close,
            self.runtime.join_custody_close(),
            self.join_local_round()
        );
        if host_result.is_err() {
            close_join_refusal("installed_host");
        }
        if source_result.is_err() {
            close_join_refusal("source_custody");
        }
        if local_result.is_err() {
            close_join_refusal("local_round");
        }
        // Native transport and the one installed guard owner have now drained.
        // Join the actual shared Controllers even when another close failed.
        let shared_result = match &self.workflow_resources {
            Some(resources) => resources
                .join_after_native_close(
                    host_result.is_ok() && source_result.is_ok() && local_result.is_ok(),
                )
                .await
                .map_err(|_| GatewayHostError::Unavailable),
            None => Ok(()),
        };
        if shared_result.is_err() {
            close_join_refusal("shared_resources");
        }
        #[cfg(test)]
        if let Some(probe) = &self.late_adoption {
            probe.joined(
                self,
                host_result.is_ok()
                    && source_result.is_ok()
                    && local_result.is_ok()
                    && shared_result.is_ok(),
            )?;
        }
        host_result
            .and(source_result)
            .and(local_result)
            .and(shared_result)
    }
    async fn wait_closed(&self) {
        let mut changed = self.closed.subscribe();
        loop {
            if let Ok(host) = self.installed() {
                tokio::select! { _ = self.closing() => {}, _ = host.wait_closed() => {} }
                return;
            }
            let is_closed = *changed.borrow();
            if is_closed || changed.changed().await.is_err() {
                return;
            }
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
        if let Err(error) = self.private.finish().await {
            failures.push(error.to_string());
        }
        for task in self.presenters {
            match task.await {
                Ok(Ok(())) => (),
                Ok(Err(error)) => failures.push(error.to_string()),
                Err(error) => failures.push(error.to_string()),
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(failures.join("; "))
        }
    }
}

// Failure-only join phases; never format owner errors or dynamic data.
// Diagnostic I/O cannot change the original refusal or its cleanup path.
fn close_join_refusal(stage: &'static str) {
    use std::io::Write as _;
    let _ = writeln!(
        std::io::stderr().lock(),
        "{{\"event\":\"toolkit_application_close_join_refused\",\"stage\":\"{stage}\"}}"
    );
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
    let _ = writeln!(
        std::io::stderr().lock(),
        "{{\"event\":\"toolkit_application_adoption_refused\",\"stage\":\"{stage}\",\"cause\":\"{cause}\"}}"
    );
    error
}

async fn serve_private(
    mut startup: PrivateHostStartup,
    driver: Arc<Driver>,
    mut completed: oneshot::Receiver<SelectedHostArtifact>,
    adopted: oneshot::Sender<()>,
    terminal: Option<InheritedOperatorTerminal>,
    reviews: Option<LocalSetupReviews>,
) -> Result<RunningPrivate, String> {
    let manifest = tokio::select! { biased;
        _ = driver.closing() => return Err("native preparation closed before application completion".into()),
        error = startup.wait_lost() => return Err(error.to_string()),
        result = &mut completed => result.map_err(|_| "native preparation has no completed bundle")?,
    };
    #[cfg(test)]
    if let Some(probe) = &driver.late_adoption {
        probe
            .before_selection(&driver)
            .map_err(|error| error.to_string())?;
    }
    let startup = tokio::select! { biased;
        _ = driver.closing() => return Err("native preparation closed before final selection".into()),
        result = startup.application_prepared(manifest) => result.map_err(|error| error.to_string())?,
    };
    // After receiving Bootstrap, adoption and any failure cleanup stay joined.
    // No cancellation branch drops an entered final owner construction.
    let prepared = driver
        .prepared
        .lock()
        .map_err(|_| "prepared owner is unavailable")?
        .take()
        .ok_or("prepared owner was already consumed")?;
    let staged = Arc::new(Mutex::new(None));
    let factory_stage = staged.clone();
    let factory_driver = driver.clone();
    let policy = prepared.policy();
    let finish_driver = driver.clone();
    let (host, private, local_reviews, remote_reviews) =
        PrivateHostConnection::bootstrap_prepared_with_service_parent(
            startup,
            move |selection| async move {
                factory_driver.current().map_err(|_| {
                    adoption_refusal("driver.factory_current", HostLifetimeError::Denied)
                })?;
                let (owner, installed) = prepared
                    .adopt(selection.clone())
                    .map_err(|error| adoption_refusal("driver.prepared_adopt", error))?;
                let originals = installed.originals();
                *factory_stage.lock().map_err(|_| {
                    adoption_refusal("driver.stage_store_lock", HostLifetimeError::Unavailable)
                })? = Some((installed, selection));
                Ok((owner, originals))
            },
            move |owner, closed, logins, service_parent| async move {
                let (installed, selection) = staged
                    .lock()
                    .map_err(|_| {
                        adoption_refusal("driver.stage_take_lock", HostLifetimeError::Unavailable)
                    })?
                    .take()
                    .ok_or_else(|| {
                        adoption_refusal(
                            "driver.staged_installation",
                            HostLifetimeError::InvalidSelection,
                        )
                    })?;
                let native = owner
                    .selected_policy()
                    .native_host
                    .as_ref()
                    .ok_or_else(|| {
                        adoption_refusal(
                            "driver.native_policy",
                            HostLifetimeError::InvalidSelection,
                        )
                    })?;
                let completed = finish_driver.completed.get().ok_or_else(|| {
                    adoption_refusal(
                        "driver.completed_setup",
                        HostLifetimeError::InvalidSelection,
                    )
                })?;
                match &finish_driver.plan.acquisition {
                    Some(parameters) => {
                        let selected =
                            crate::application_acquisition::read_application_acquisition(
                                native,
                                |reference| {
                                    toolkit_mobkit::installed_host::read_bundle_artifact(
                                        &selection, reference,
                                    )
                                },
                            )?
                            .ok_or(HostLifetimeError::InvalidSelection)?;
                        if !selected.matches_preparation(parameters, completed)? {
                            return Err(HostLifetimeError::InvalidSelection);
                        }
                    }
                    None => {
                        let selected = crate::application_acquisition::read_application_only(
                            native,
                            |reference| {
                                toolkit_mobkit::installed_host::read_bundle_artifact(
                                    &selection, reference,
                                )
                            },
                        )?
                        .ok_or(HostLifetimeError::InvalidSelection)?;
                        if !selected.matches_preparation(completed)? {
                            return Err(HostLifetimeError::InvalidSelection);
                        }
                    }
                }
                let retained = finish_driver.runtime.acquisition();
                let (installed, acquisition) = match &finish_driver.workflow {
                    Some(workflow) => {
                        crate::application_acquisition::prepare_gateway_acquisition_with_workflow(
                            &selection,
                            native,
                            installed,
                            retained.ok_or(HostLifetimeError::InvalidSelection)?,
                            workflow.clone(),
                        )
                    }
                    None => {
                        crate::application_acquisition::prepare_gateway_acquisition_with_retained(
                            &selection, native, installed, retained,
                        )
                    }
                }
                .map_err(|error| adoption_refusal("driver.prepare_acquisition", error))?;
                if acquisition.is_some() != finish_driver.plan.acquisition.is_some() {
                    return Err(HostLifetimeError::InvalidSelection);
                }
                let class_policy = installed.prepared_tool_policy();
                let host = installed
                    .with_operation_review_policy(finish_driver.operation_review_policy.clone())
                    .compose_with_prepared_registration(
                        owner.clone(),
                        closed,
                        finish_driver.registration.clone(),
                        policy.clone(),
                    )
                    .map_err(|error| adoption_refusal("driver.compose_registration", error))?
                    .with_console_logins(logins)
                    .map_err(|error| adoption_refusal("driver.compose_console_logins", error))?
                    .with_member_class_policy(class_policy)
                    .map_err(|error| adoption_refusal("driver.compose_member_classes", error))?;
                let host = match acquisition {
                    Some(acquisition) => acquisition
                        .compose_prepared_factory(
                            host,
                            finish_driver.presenters.as_ref(),
                            finish_driver.selected_policy.as_ref(),
                            owner,
                            service_parent,
                        )
                        .map_err(|error| adoption_refusal("driver.compose_acquisition", error))?,
                    None => host,
                };
                let host = Arc::new(
                    finish_driver
                        .runtime
                        .adopt(host)
                        .await
                        .map_err(|error| adoption_refusal("driver.runtime_adopt", error))?,
                );
                host.bind_member_class_context(
                    finish_driver
                        .member_class_context
                        .get()
                        .cloned()
                        .ok_or(HostLifetimeError::Unavailable)?,
                )?;
                if let Some(workflow) = &finish_driver.workflow {
                    workflow.bind_runtime(
                        crate::application_acquisition::workflow::ApplicationWorkflowRuntime::new(
                            host.clone(),
                            finish_driver
                                .workflow_resources
                                .clone()
                                .ok_or(HostLifetimeError::Unavailable)?,
                            finish_driver
                                .presenters
                                .clone()
                                .ok_or(HostLifetimeError::Unavailable)?,
                            finish_driver
                                .selected_policy
                                .clone()
                                .ok_or(HostLifetimeError::Unavailable)?,
                            finish_driver.runtime.service_tasks(),
                            finish_driver
                                .plan
                                .acquisition
                                .as_ref()
                                .ok_or(HostLifetimeError::InvalidSelection)?
                                .output_audience
                                .clone(),
                            finish_driver.plan.application.realm.clone(),
                        ),
                    )?;
                }
                finish_driver
                    .retain_installed(host.clone())
                    .map_err(|error| adoption_refusal("driver.retain_installed", error))?;
                Ok(host)
            },
        )
        .await
        .map_err(|error| error.to_string())?;
    private
        .member_classes()
        .bind(&host)
        .map_err(|error| error.to_string())?;
    if let Some(workflow) = &driver.workflow {
        private
            .workflow_children()
            .bind(workflow.clone())
            .map_err(|error| error.to_string())?;
    }
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
    tasks.push(tokio::spawn(
        crate::application_remote::serve_remote_operator_with_service_parent(
            host.clone(),
            remote_reviews,
            driver.plan.deployment_root.clone(),
            private.closed_observer(),
            service_parent,
        ),
    ));
    match (terminal, reviews) {
        (Some(terminal), Some(reviews)) => tasks.push(tokio::spawn(
            crate::application_gateway::serve_local_operator_with_setup(
                host,
                local_reviews,
                terminal,
                driver.plan.deployment_root.clone(),
                private.closed_observer(),
                reviews,
            ),
        )),
        (None, Some(reviews)) => {
            drop(local_reviews);
            if driver
                .plan
                .acquisition
                .as_ref()
                .is_some_and(|selected| selected.console_setup.is_some())
            {
                tasks.push(tokio::spawn(
                    crate::application_gateway::serve_console_setup(
                        reviews,
                        private.closed_observer(),
                    ),
                ));
            } else {
                drop(reviews);
            }
        }
        (Some(terminal), None) => tasks.push(tokio::spawn(
            crate::application_gateway::serve_local_operator(
                host,
                local_reviews,
                terminal,
                driver.plan.deployment_root.clone(),
                private.closed_observer(),
            ),
        )),
        (None, None) => {
            drop(local_reviews);
        }
    }
    let running = RunningPrivate {
        private,
        presenters: tasks,
    };
    if adopted.send(()).is_err() {
        let cleanup = running.finish().await;
        return Err(match cleanup {
            Ok(()) => "native held completion was lost after final adoption".into(),
            Err(error) => {
                format!("native held completion was lost; private cleanup also failed: {error}")
            }
        });
    }
    Ok(running)
}

/// Actual executable entry for the private PrepareApplication request. The SDK
/// concurrently runs ordinary init and all live providers on this same child.
pub async fn run(
    startup: PrivateHostStartup,
    terminal: Option<InheritedOperatorTerminal>,
    control: Option<ControlListenAddr>,
) -> Result<(), String> {
    run_with_options(
        startup,
        terminal,
        control,
        ApplicationDriverOptions::default(),
    )
    .await
}

/// Trusted application entry for a real host-selected workflow launch owner.
/// Serialized setup configuration and ordinary Console input cannot select it.
/// The same driver still owns bootstrap, Console login, native runtime and
/// cleanup; the supplied owner only binds its actual workflow invocations.
pub async fn run_with_workflows(
    startup: PrivateHostStartup,
    terminal: Option<InheritedOperatorTerminal>,
    control: Option<ControlListenAddr>,
    workflows: Arc<dyn crate::application_acquisition::workflow::ApplicationWorkflowLaunchOwner>,
) -> Result<(), String> {
    run_with_options(
        startup,
        terminal,
        control,
        ApplicationDriverOptions {
            workflows: Some(workflows),
            ..ApplicationDriverOptions::default()
        },
    )
    .await
}

/// Trusted process composition, never application-plan or public RPC data.
#[derive(Default)]
pub struct ApplicationDriverOptions {
    /// Explicit process-owned review floor, applied by the final installed
    /// policy after ordinary authorization. No plan or Console tier selection.
    pub operation_review_policy:
        Option<Arc<dyn toolkit_mobkit::installed_host::InstalledOperationReviewPolicy>>,
    /// Trusted opt-in issuer/reviewer selection; absent preserves ordinary review.
    pub operation_review: Option<crate::application_consent::ApplicationOperationReview>,
    /// Explicit exact predecessor selection; never a fallback from failed successful close.
    pub abandoned_workflows: Option<crate::application_consent::AbandonedWorkflowRecovery>,
    pub workflows:
        Option<Arc<dyn crate::application_acquisition::workflow::ApplicationWorkflowLaunchOwner>>,
    pub hosted_mcp_redirect_base: Option<meerkat::McpRegisteredRedirectBase>,
    #[cfg(test)]
    late_adoption: Option<Arc<late_adoption_tests::Probe>>,
}

pub async fn run_with_options(
    mut startup: PrivateHostStartup,
    terminal: Option<InheritedOperatorTerminal>,
    control: Option<ControlListenAddr>,
    options: ApplicationDriverOptions,
) -> Result<(), String> {
    let ApplicationDriverOptions {
        operation_review_policy,
        workflows,
        hosted_mcp_redirect_base,
        operation_review,
        abandoned_workflows,
        #[cfg(test)]
        late_adoption,
    } = options;
    let hosted_mcp = hosted_mcp_redirect_base.map(|base| (startup.hosted_mcp_auth(), base));
    let PrivateHostStartupIntent::PrepareApplication(reference) = startup.intent().clone() else {
        return Err("schema4 driver requires its exact preparation request".into());
    };
    let bytes = crate::application_setup::read_regular_artifact(&reference.path)?;
    if crate::application_bootstrap::hash(&bytes) != reference.sha256 {
        return Err("preparation input changed".into());
    }
    let mut plan = read_plan(&bytes)?;
    if plan.schema != 1
        || plan.hold_timeout_ms == 0
        || plan.hold_timeout_ms > 900_000
        || !plan.deployment_root.is_absolute()
        || !plan.output.is_absolute()
    {
        return Err("invalid application preparation plan".into());
    }
    plan.owner.validate().map_err(|error| error.to_string())?;
    if plan.application.helper.is_some() != plan.acquisition.is_some() {
        return Err("helper and acquisition must be selected together".into());
    }
    if let Some(acquisition) = &plan.acquisition {
        acquisition.validate().map_err(|error| error.to_string())?;
        if workflows.is_some()
            && (acquisition.standing_policy.is_none()
                || (terminal.is_none() && acquisition.console_setup.is_none()))
        {
            return Err("shared workflows require an explicit application policy and actual review presenter".into());
        }
        let tools = plan
            .application
            .helper_tools
            .ok_or("helper tools are absent")?;
        if acquisition.standing_policy.is_some() && !tools.requires_software_review() {
            return Err(
                "standing acquisition policy requires the selected checked setup surface".into(),
            );
        }
        if tools.requires_software_review()
            && terminal.is_none()
            && acquisition.console_setup.is_none()
            && acquisition.standing_policy.is_none()
        {
            return Err(
                "checked helper installation requires the selected terminal or Console presenter"
                    .into(),
            );
        }
    } else {
        plan.authorities
            .validate_without_helper()
            .map_err(|error| error.to_string())?;
        if workflows.is_some() {
            return Err("shared workflows require a selected setup acquisition".into());
        }
    }
    if plan.owner.native_host.members != plan.application.members
        || plan.owner.native_host.member_classes != plan.application.member_classes
        || plan.owner.native_host.realm.as_str() != plan.application.realm.realm_id
        || (plan
            .application
            .helper
            .as_ref()
            .is_some_and(|helper| helper.controller_parent.is_empty())
            && !plan
                .authorities
                .controllers
                .iter()
                .any(|request| request.target == PreparedControllerTarget::Helper))
    {
        return Err("preparation owner and application selection differ or lack a helper controller request".into());
    }
    // Validate the unbound compiler input before opening any native owner.
    // Held construction keeps route authority unavailable until actual reports
    // resolve the retained requests and finalization precedes publication.
    let validation = plan.application.clone().validate()?;
    let workflow_realm = plan
        .application
        .realm
        .resolve()
        .map_err(|error| error.to_string())?
        .locator;
    // Open the canonical pair once. It issues no route-dependent authority
    // until the same runtime returns the selected members' actual held reports.
    let mut owner = PreparedInstallationOwner::open(
        plan.deployment_root.clone(),
        plan.owner.clone(),
        plan.startup.clone(),
    )
    .map_err(|error| error.to_string())?;
    // Bind before resources or native factories can be created. Reopening a
    // missing issuer is never an implicit fresh selection, and this exact
    // owner is retained across Driver, factory and settlement lifetimes.
    let operation_review = operation_review
        .map(|selection| selection.bind(&owner))
        .transpose()
        .map_err(|error| error.to_string())?;
    let bounds = McpAttemptGuardBounds::new(
        Duration::from_secs(60),
        Duration::from_secs(5),
        Duration::from_secs(5),
    )
    .map_err(|error| error.to_string())?;
    let (compilation, attempts) = if plan.application.helper.is_some() {
        let resources = InstalledResources::locate().map_err(|error| error.to_string())?;
        let (prepared_resources, mut local_owner) = resources
            .prepare_with_local_round_owner(validation.storage(), async {
                let _error = startup.wait_lost().await;
                Ok(())
            })
            .await
            .map_err(|error| error.to_string())?;
        let mut compilation = match validation.prepare(prepared_resources) {
            Ok(compilation) => compilation,
            Err(error) => {
                return Err(local_round::preparation_failed(error, &mut local_owner).await);
            }
        };
        let attempts = match compilation.retain_local_round_owner(local_owner) {
            Ok(attempts) => attempts,
            Err(failed) => {
                let (mut owner, error) = *failed;
                return Err(local_round::preparation_failed(error, &mut owner).await);
            }
        };
        (compilation, Some(attempts))
    } else {
        (validation.prepare_without_helper()?, None)
    };
    let compilation = Arc::new(compilation);
    let workflow_resources = workflows.as_ref().map(|_| {
        crate::installed_service_review::composer::LiveSharedSetup::new(
            workflow_realm,
            compilation.prepared_tool_policy(),
            compilation.definition().clone(),
            &plan.application.member_classes,
        )
    });
    let local_round = attempts.map(|attempts| {
        let factory: Arc<dyn meerkat_mcp::McpAttemptGuardFactory> = match &workflow_resources {
            Some(resources) => resources.guards(attempts.clone()),
            None => attempts.clone(),
        };
        (attempts, McpAttemptGuards::new(factory, bounds))
    });
    let prepared_driver = async {
        if abandoned_workflows.is_some() && workflow_resources.is_none() {
            return Err("abandoned recovery requires selected workflow resources".into());
        }
        if let Some(resources) = &workflow_resources {
            let issuer = operation_review.as_ref().map(|review| review.owner()).transpose()
                .map_err(|error| error.to_string())?;
            resources.bind_consent_owner(issuer).map_err(|error| error.to_string())?;
            resources.bind_abandoned_recovery(abandoned_workflows).map_err(|error| error.to_string())?;
        }
        let selected = compilation.project_policy(plan.owner.clone())?;
        if let (Some(previous), Some(compiled)) = (&plan.application.helper, compilation.helper()) {
            owner.project_helper_member(&previous.member, &compiled.member).map_err(|error| error.to_string())?;
            plan.authorities = plan.authorities.project_helper_member(&previous.member, &compiled.member);
        }
        if owner.selected() != &selected { return Err("prepared owner projection changed".into()); }
        plan.owner = selected;
        let selected_policy = plan.acquisition.as_ref().and_then(|row| row.standing_policy).map(|selection|
            crate::application_acquisition::selected_policy::SelectedPolicyBinding::new_with_member_classes(selection,
                plan.owner.native_host.grant_administrator.clone(), plan.owner.deployment.clone(), plan.owner.native_host.realm.clone(),
                plan.owner.native_host.members.iter().map(|row| row.member.member.clone()).collect(), plan.owner.native_host.member_classes.clone()));
        let recovery_participant = selected_policy.as_ref().map(|binding| binding.participant()).or_else(||
            (plan.application.helper_tools.is_some_and(|tools| tools.requires_software_review()) || !plan.application.member_classes.is_empty()).then(||
                crate::application_acquisition::selected_policy::SelectedPolicyBinding::human_review_participant_with_member_classes(
                    plan.owner.native_host.grant_administrator.clone(), plan.owner.deployment.clone(), plan.owner.native_host.realm.clone(),
                    plan.owner.native_host.members.iter().map(|row| row.member.member.clone()).collect(), plan.owner.native_host.member_classes.clone())));
        let prepared = match recovery_participant {
            Some(participant) => owner.prepare_with_application_service_policy(
                compilation.prepared_tool_policy(), participant).await,
            None => owner.prepare(compilation.prepared_tool_policy()).await,
        }.map_err(|error| error.to_string())?;
        let resources = compilation.clone();
        let live_resources = workflow_resources.clone();
        let registration = Arc::new(PreparedMemberRegistration::new(compilation.clone(), compilation.registry(),
            Arc::new(move |identity, destination| {
                resources.selected_destination(identity, destination)?;
                if let Some(live) = &live_resources { live.selected_destination(identity, destination)?; }
                Ok(())
            })));
        let runtime = match &plan.acquisition {
            Some(acquisition) => {
                let tools = InstalledAcquisitionTools::new(Duration::from_millis(acquisition.limits.admission_timeout_ms))
                    .map_err(|error| error.to_string())?;
                prepared.gateway_runtime(registration.clone(), tools, acquisition.output_audience.clone())
                    .map_err(|error| error.to_string())?
            }
            None => prepared.gateway_runtime_without_acquisition(registration.clone()),
        };
        let workflow = match workflows {
            Some(launch) => Some(Arc::new(crate::application_acquisition::workflow::ApplicationWorkflowCommissioner::new(
                launch, prepared.originals(), runtime.acquisition().ok_or("workflow acquisition absent")?.1))),
            None => None,
        };
        let mut configuration = prepared.take_configuration().map_err(|error| error.to_string())?;
        if let Some(resource) = compilation.resource_selection() {
            configuration.configured_mcp_attachment = Some(Arc::new(crate::application_setup::resource::SetupResourceProvider::new(
                resource, configuration.configured_mcp_attachment.take(),
            )));
        }
        if let Some(resources) = &workflow_resources {
            configuration.configured_mcp_attachment = Some(resources.configured(configuration.configured_mcp_attachment.take()));
        }
        let (presenters, reviews) = match &plan.acquisition {
            Some(acquisition) if retain_presenter_channel(acquisition.standing_policy.as_ref(),
                terminal.is_some(), acquisition.console_setup.is_some()) => {
                let (presenters, reviews) = crate::application_gateway::setup_presenter::channel(plan.deployment_root.clone());
                let presenters = if acquisition.console_setup.is_some() { presenters.with_pending_console() } else { presenters };
                (Some(presenters), Some(reviews))
            }
            _ => (None, None),
        };
        Ok::<_, String>((registration, runtime, configuration, prepared, presenters, selected_policy, reviews, workflow))
    }.await;
    let (
        registration,
        runtime,
        configuration,
        prepared,
        presenters,
        selected_policy,
        reviews,
        workflow,
    ) = match prepared_driver {
        Ok(prepared) => prepared,
        Err(error) => {
            // No native factory has received the installation yet. Closing it
            // recovers this exact unentered owner after any preparation refusal.
            let mut cleanup = LocalRoundCleanup::default();
            let joined = match &local_round {
                Some((attempts, guards)) => cleanup.join_until_settled(attempts, guards).await,
                None => Ok(()),
            };
            return Err(match joined {
                Ok(()) => error,
                Err(_) => format!("{error}; local preparation cleanup remains unavailable"),
            });
        }
    };
    let (deliver, completed) = oneshot::channel();
    let (adopted, receive) = oneshot::channel();
    let (closed, _) = watch::channel(false);
    let driver = Arc::new(Driver {
        operation_review_policy,
        operation_review,
        plan,
        plan_reference: reference,
        plan_bytes: bytes,
        compilation,
        registration,
        runtime,
        configuration: Mutex::new(Some(configuration)),
        prepared: Mutex::new(Some(prepared)),
        presenters,
        selected_policy,
        publication: Mutex::new(Publication {
            closed: false,
            admission_stopped: false,
            installed: None,
        }),
        closed,
        selected: AtomicBool::new(false),
        completed: OnceLock::new(),
        deliver: Mutex::new(Some(deliver)),
        adopted: Mutex::new(Some(receive)),
        local_round,
        local_round_cleanup: tokio::sync::Mutex::new(LocalRoundCleanup::default()),
        member_class_context: OnceLock::new(),
        workflow,
        workflow_resources,
        hosted_mcp,
        #[cfg(test)]
        late_adoption,
    });
    let actor_driver = driver.clone();
    let actor = tokio::spawn(async move {
        let result = serve_private(
            startup,
            actor_driver.clone(),
            completed,
            adopted,
            terminal,
            reviews,
        )
        .await;
        if result.is_err() {
            let _ = actor_driver.begin_close(GatewayHostCloseReason::HostDisconnected);
        }
        result
    });
    let native = run_persistent_with_host(control, driver.clone()).await;
    let closing = driver.begin_close(GatewayHostCloseReason::Shutdown);
    let private = match actor.await {
        Ok(Ok(running)) => running.finish().await,
        Ok(Err(error)) => Err(error),
        Err(error) => Err(error.to_string()),
    };
    let mut failures = Vec::new();
    if let Err(error) = native {
        failures.push(error.to_string());
    }
    if let Err(error) = closing {
        failures.push(error.to_string());
    }
    if let Err(error) = private {
        failures.push(error);
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

// Absence retains the legacy channel even for an inert helper without a UI.
// Only an explicitly selected headless policy composition omits the channel.
fn retain_presenter_channel(
    selected: Option<&crate::application_acquisition::selected_policy::StandingPolicySelection>,
    terminal: bool,
    console: bool,
) -> bool {
    selected.is_none() || terminal || console
}

#[cfg(test)]
mod standing_presentation_tests {
    use super::*;
    use crate::application_acquisition::selected_policy::{
        AccessSelection, InstallationSelection, StandingPolicySelection,
    };

    #[test]
    fn absent_selection_retains_legacy_channel_and_selected_headless_omits_it() {
        let selection = StandingPolicySelection {
            installation: InstallationSelection::CheckedSoftware,
            access: AccessSelection::ConfiguredParent,
            tool_policy: None,
        };
        for terminal in [false, true] {
            for console in [false, true] {
                assert!(retain_presenter_channel(None, terminal, console));
                assert_eq!(
                    retain_presenter_channel(Some(&selection), terminal, console),
                    terminal || console
                );
            }
        }
    }
}

#[cfg(test)]
mod helperless_tests;
#[cfg(test)]
mod late_adoption_tests;
