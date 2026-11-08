//! Workflow participants for an already authenticated installed application.
//! This uses ordinary native activation; it never repeats held finalization.
use super::*;
use crate::{
    application_gateway::LocalSetupApprovals, application_setup::ApplicationSetupRealm,
    installed_service_review::composer::LiveSharedSetup,
};
use meerkat_mobkit::{
    UnifiedRuntime,
    mob_handle_runtime::MobBootstrapSpec,
    rpc_gateway::{
        GatewayBootstrapReady, GatewayFactoryConstruction, GatewayHostCloseReason,
        GatewayHostComposition, GatewayHostError, GatewayMcpComposition,
        PreparedApplicationRequest, PreparedGatewayApplication,
    },
};
use std::sync::OnceLock;

/// The selected installed workflow participant. Reattachment checks exact
/// canonical, native, physical and approval-owner observations; the existing
/// source owner restores G's restriction and historical planning custody.
pub struct InstalledWorkflowComposition {
    pub(crate) commissioner: Arc<ApplicationWorkflowCommissioner>,
    pub(crate) resources: Arc<LiveSharedSetup>,
    output_audiences: BTreeSet<Id>,
    realm: ApplicationSetupRealm,
    policy: OnceLock<Arc<crate::application_acquisition::selected_policy::SelectedPolicyBinding>>,
    presenters: OnceLock<LocalSetupApprovals>,
    tasks: OnceLock<toolkit_mobkit::installed_host::acquisition::InstalledServiceTaskSource>,
    round: OnceLock<Arc<crate::application_acquisition::RestoredLocalRound>>,
}
impl InstalledWorkflowComposition {
    pub(crate) fn new(
        commissioner: Arc<ApplicationWorkflowCommissioner>,
        resources: Arc<LiveSharedSetup>,
        output_audiences: BTreeSet<Id>,
        realm: ApplicationSetupRealm,
    ) -> Arc<Self> {
        Arc::new(Self {
            commissioner,
            resources,
            output_audiences,
            realm,
            policy: OnceLock::new(),
            presenters: OnceLock::new(),
            tasks: OnceLock::new(),
            round: OnceLock::new(),
        })
    }
    pub(crate) fn bind_review(
        &self,
        policy: Arc<crate::application_acquisition::selected_policy::SelectedPolicyBinding>,
        presenters: LocalSetupApprovals,
    ) -> Result<(), HostLifetimeError> {
        presenters
            .check_open()
            .map_err(|_| HostLifetimeError::Unavailable)?;
        self.policy
            .set(policy)
            .map_err(|_| HostLifetimeError::Denied)?;
        self.presenters
            .set(presenters)
            .map_err(|_| HostLifetimeError::Denied)
    }
    pub(crate) fn capture_tasks(
        &self,
        tasks: toolkit_mobkit::installed_host::acquisition::InstalledServiceTaskSource,
    ) -> Result<(), HostLifetimeError> {
        self.resources.configure_restore(
            self.presenters
                .get()
                .cloned()
                .ok_or(HostLifetimeError::Unavailable)?,
            self.policy
                .get()
                .cloned()
                .ok_or(HostLifetimeError::Unavailable)?,
            tasks.clone(),
            self.realm.clone(),
        )?;
        self.tasks.set(tasks).map_err(|_| HostLifetimeError::Denied)
    }
    /// Bind the same private endpoint used by the authenticated installed host.
    pub fn bind_private(
        &self,
        endpoint: &toolkit_mobkit::private_host::WorkflowChildEndpoint,
    ) -> Result<(), HostLifetimeError> {
        endpoint.bind(self.commissioner.clone())
    }
    pub(crate) fn bind_round(
        &self,
        round: Arc<crate::application_acquisition::RestoredLocalRound>,
    ) -> Result<(), HostLifetimeError> {
        self.round.set(round).map_err(|_| HostLifetimeError::Denied)
    }
    /// Retain the exact host, source and native guard owners for activation and
    /// all-exit cleanup. Call after composing this acquisition's selected factory.
    pub fn compose(
        self: &Arc<Self>,
        host: Arc<ToolkitGatewayHost>,
    ) -> Result<Arc<dyn GatewayHostComposition>, HostLifetimeError> {
        self.compose_with_consent_owner(host, None, None)
    }
    pub(crate) fn compose_with_consent_owner(
        self: &Arc<Self>,
        host: Arc<ToolkitGatewayHost>,
        owner: Option<Arc<crate::application_consent::ApplicationConsentOwner>>,
        abandoned: Option<crate::application_consent::AbandonedWorkflowRecovery>,
    ) -> Result<Arc<dyn GatewayHostComposition>, HostLifetimeError> {
        let round = self
            .round
            .get()
            .cloned()
            .ok_or(HostLifetimeError::Unavailable)?;
        self.resources.bind_consent_owner(owner)?;
        self.resources.bind_abandoned_recovery(abandoned)?;
        self.resources
            .bind_restore_source(&host.workflow_source()?)?;
        #[cfg(test)]
        crate::application_installed::current_g_receiptful_capture::observe_workflow(
            &self.resources,
        )?;
        Ok(Arc::new(InstalledWorkflowHost {
            host,
            workflow: self.clone(),
            round,
        }))
    }
}

struct InstalledWorkflowHost {
    host: Arc<ToolkitGatewayHost>,
    workflow: Arc<InstalledWorkflowComposition>,
    round: Arc<crate::application_acquisition::RestoredLocalRound>,
}
#[async_trait]
impl GatewayHostComposition for InstalledWorkflowHost {
    async fn prepared_application(
        &self,
        request: Option<&PreparedApplicationRequest>,
    ) -> Result<Option<PreparedGatewayApplication>, GatewayHostError> {
        self.host.prepared_application(request).await
    }
    fn native_grant_work_configuration(
        &self,
    ) -> Result<
        Option<meerkat_runtime::meerkat_machine::NativeGrantWorkConfiguration>,
        GatewayHostError,
    > {
        self.host.native_grant_work_configuration()
    }
    fn compose_agent_customizer(
        &self,
        customizer: Option<Arc<dyn meerkat_mobkit::identity_first::AgentCustomizer>>,
    ) -> Result<Option<Arc<dyn meerkat_mobkit::identity_first::AgentCustomizer>>, GatewayHostError>
    {
        Ok(self
            .host
            .compose_agent_customizer(customizer)?
            .map(|inner| self.workflow.resources.customizer(inner)))
    }
    fn compose_member_factory(
        &self,
        construction: &GatewayFactoryConstruction<'_>,
    ) -> Result<Option<GatewayMcpComposition>, GatewayHostError> {
        self.host.compose_member_factory(construction)
    }
    fn operation_review(
        &self,
        construction: &GatewayFactoryConstruction<'_>,
    ) -> Result<Option<Arc<meerkat_core::approval::review::BoundOperationReview>>, GatewayHostError>
    {
        self.host.operation_review(construction)
    }
    fn operation_refusal_observer(
        &self,
        construction: &GatewayFactoryConstruction<'_>,
    ) -> Result<Option<Arc<dyn meerkat_core::OperationRefusalObserver>>, GatewayHostError> {
        self.host.operation_refusal_observer(construction)
    }
    async fn prepare_bootstrap(
        &self,
        spec: MobBootstrapSpec,
    ) -> Result<MobBootstrapSpec, GatewayHostError> {
        self.workflow
            .resources
            .capture_native(&spec)
            .map_err(|_| GatewayHostError::Denied)?;
        self.host
            .prepare_bootstrap(spec.with_spawn_member_customizer(self.workflow.resources.clone()))
            .await
    }
    async fn bind_runtime(&self, runtime: &mut UnifiedRuntime) -> Result<(), GatewayHostError> {
        self.host.bind_runtime(runtime).await
    }
    async fn bind_identity_runtime(
        &self,
        runtime: &mut UnifiedRuntime,
    ) -> Result<(), GatewayHostError> {
        self.host.bind_identity_runtime(runtime).await?;
        self.workflow
            .commissioner
            .bind_runtime(ApplicationWorkflowRuntime::new(
                self.host.clone(),
                self.workflow.resources.clone(),
                self.workflow
                    .presenters
                    .get()
                    .cloned()
                    .ok_or(GatewayHostError::Unavailable)?,
                self.workflow
                    .policy
                    .get()
                    .cloned()
                    .ok_or(GatewayHostError::Unavailable)?,
                self.workflow
                    .tasks
                    .get()
                    .cloned()
                    .ok_or(GatewayHostError::Unavailable)?,
                self.workflow.output_audiences.clone(),
                self.workflow.realm.clone(),
            ))
            .map_err(|error| match error {
                HostLifetimeError::Unavailable => GatewayHostError::Unavailable,
                _ => GatewayHostError::Denied,
            })
    }
    async fn identity_member_retired(
        &self,
        retired: &meerkat_mobkit::identity_first::RetiredIdentityMember,
    ) -> Result<(), GatewayHostError> {
        self.host.identity_member_retired(retired).await
    }
    fn host_app_routes(
        &self,
        viewer: meerkat_mobkit::http_console::ConsoleViewerResolver,
    ) -> Result<Option<axum::Router>, GatewayHostError> {
        self.host.host_app_routes(viewer)
    }
    fn host_app_routes_with_send_precheck(
        &self,
        viewer: meerkat_mobkit::http_console::ConsoleViewerResolver,
        precheck: meerkat_mobkit::http_console::ConsoleSendPrecheck,
    ) -> Result<Option<axum::Router>, GatewayHostError> {
        self.host
            .host_app_routes_with_send_precheck(viewer, precheck)
    }
    fn publish_bootstrap_ready(
        &self,
        publication: GatewayBootstrapReady<'_>,
    ) -> Result<(), GatewayHostError> {
        self.host.publish_bootstrap_ready(publication)
    }
    fn stop_admitting(&self, reason: GatewayHostCloseReason) -> Result<(), GatewayHostError> {
        self.host.stop_admitting(reason)
    }
    fn begin_close(&self, reason: GatewayHostCloseReason) -> Result<(), GatewayHostError> {
        self.workflow.resources.begin_close();
        self.host.begin_close(reason)
    }
    async fn join_close(&self) -> Result<(), GatewayHostError> {
        let (host, round) =
            tokio::join!(self.host.join_close(), self.round.join_after_native_close());
        let resources = self
            .workflow
            .resources
            .join_after_native_close(host.is_ok() && round.is_ok())
            .await
            .map_err(|_| GatewayHostError::SettlementIncomplete);
        // Native guard settlement precedes closing its physical shared channel.
        // Join every retained owner even when one reports a failure. A joined
        // transport is not a native quiescence or approval restoration receipt.
        host.and(round).and(resources)
    }
    async fn wait_closed(&self) {
        self.host.wait_closed().await
    }
}
