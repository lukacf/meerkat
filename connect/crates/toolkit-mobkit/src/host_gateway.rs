//! Concrete Toolkit ownership composed through MobKit's generic gateway hook.
//!
//! Public init fields never select a host, policy owner or registration hook.
//! The application's native registration hook owns destination selection and
//! surface verification. A lifecycle observation is never permission.

mod console_setup;
mod local_recovery;
mod prepared_application;
pub use local_recovery::{
    LocalOperatorRecoveryPolicy, LocalRecoveryPageRead, LocalServiceRecovery,
    LocalServiceRecoveryPage, LocalServiceRecoveryResult,
};
mod collector;
mod member_classes;
mod prepared_runtime;
#[cfg(test)]
pub(crate) use collector::tests::configuration as collector_test_configuration;
pub use collector::{
    CollectorOccurrence, CollectorOpenError, CollectorOpenRefusal, CollectorReadError,
};
mod workflow_classes;
pub use console_setup::{
    ConsoleAccessPolicy, ConsoleAccessSnapshot, ConsolePageRead, ConsoleRecoveryPolicy,
    ConsoleServiceRecoverySnapshot, ConsoleSetupBinding, ConsoleStatusPage, ObservedConsoleConsent,
};
pub use member_classes::{
    MemberClassAddError, MemberClassAddOutcome, MemberClassAddStart, MemberClassCleanupObservation,
    MemberClassResourceCleanup, MemberClassSettlement,
};
pub use prepared_application::PreparedSetupApplication;
pub use prepared_runtime::PreparedGatewayRuntime;
pub use workflow_classes::CompletedWorkflowMemberClass;

use crate::{
    private_host::PrivateHostClosed,
    private_originals::{
        PrivateOriginalCancellation, PrivateOriginalError, PrivateOriginalRef, PrivateOriginals,
        PrivateServiceAccessDecision, PrivateServiceGrantObservation,
    },
};
use async_trait::async_trait;
use meerkat_mob::MemberRuntimeRegistrationHook;
use meerkat_mobkit::{
    UnifiedRuntime,
    mob_handle_runtime::MobBootstrapSpec,
    rpc_gateway::{
        GatewayBootstrapReady, GatewayFactoryConstruction, GatewayHostCloseReason,
        GatewayHostComposition, GatewayHostError, GatewayMcpComposition,
        PreparedApplicationRequest, PreparedGatewayApplication,
    },
};
use meerkat_runtime::{MeerkatMachine, meerkat_machine::NativeGrantWorkConfiguration};
use meerkat_toolkit::host_lifetime::{
    NativeHostLifetime,
    control::{HostCloseReason, HostCloseState, HostLifetimeError, PrivateHostLifetimeControl},
};
use std::sync::{Arc, Mutex, OnceLock};

/// Pure pre-build projection of an already authenticated setup profile.
/// The application supplies the existing prepared helper; no resolver, backend
/// constructor or source/grant authority is hidden in this closure.
pub type SetupBootstrapProjection =
    Arc<dyn Fn(MobBootstrapSpec) -> Result<MobBootstrapSpec, HostLifetimeError> + Send + Sync>;

/// Trusted application construction on the actual native factory offer. This
/// callback creates no factory, machine, policy registry or authorization owner.
/// It runs synchronously under this host's existing current-selection boundary.
pub type SetupMemberFactoryComposition = Arc<
    dyn Fn(
            &GatewayFactoryConstruction<'_>,
            crate::installed_host::acquisition::InstalledServiceTaskSource,
        ) -> Result<GatewayMcpComposition, HostLifetimeError>
        + Send
        + Sync,
>;

/// Retained process-selected review, checked only through its actual host.
/// It supplies no human authentication, grant or machine authority. Every call
/// runs synchronously under the host's current-selection/close fence: it must
/// not await, detach work or reenter that native lifetime, just like the initial
/// selector. It may recheck its own retained issuer and return the same review.
pub type SetupOperationReview = Arc<
    dyn Fn() -> Result<Arc<meerkat_core::approval::review::BoundOperationReview>, HostLifetimeError>
        + Send
        + Sync,
>;

pub struct ToolkitGatewayHost {
    owner: Arc<NativeHostLifetime>,
    private_closed: PrivateHostClosed,
    machine: OnceLock<Arc<MeerkatMachine>>,
    #[cfg(feature = "test-support")]
    runtime_observer: Option<Arc<Mutex<Option<meerkat_mobkit::mob_handle_runtime::MobRuntime>>>>,
    // Outer None means the gateway already took this one-use configuration.
    // Inner None means the legacy constructor has no governed owners.
    grant_configuration: Mutex<Option<Option<NativeGrantWorkConfiguration>>>,
    member_registration: Option<Arc<dyn MemberRuntimeRegistrationHook>>,
    private_originals: Option<Arc<PrivateOriginals>>,
    console_logins: Option<crate::private_host::ConsoleLogins>,
    hosted_mcp: Option<(
        crate::private_host::HostedMcpAuthEndpoint,
        meerkat::McpRegisteredRedirectBase,
    )>,
    installed_tool_policy: Option<Arc<meerkat_core::ToolConsequencePolicyRegistry>>,
    member_class_policy: Option<crate::installed_host::tool_policy::PreparedInstalledToolPolicy>,
    member_class_context: OnceLock<meerkat_mobkit::identity_first::PublicIdentityFirstContext>,
    member_class_operations: member_classes::MemberClassOperations,
    collector_adapters: collector::CollectorAdapters,
    acquisition: Arc<Mutex<crate::installed_host::acquisition::AcquisitionBootstrap>>,
    native_continuation:
        Option<Arc<crate::installed_host::acquisition::InstalledNativeContinuation>>,
    setup_projection: Option<SetupBootstrapProjection>,
    setup_application: Option<PreparedSetupApplication>,
    setup_member_factory: Option<SetupMemberFactoryComposition>,
    operation_review: Option<SetupOperationReview>,
    operation_review_offered: OnceLock<()>,
    console_routes: Option<console_setup::ConsoleRouteComposition>,
    local_recovery: Option<LocalOperatorRecoveryPolicy>,
    // Process-local transport observation only, never an activation or grant.
    prepared_application_selected: OnceLock<()>,
    acquisition_recovery: crate::installed_host::acquisition::recovery::AcquisitionRecovery,
}

impl ToolkitGatewayHost {
    /// Observe the actual bound runtime in explicit native process controls.
    /// This fills no production owner slot and selects no replacement handle.
    #[cfg(feature = "test-support")]
    pub fn with_runtime_observer_for_test(
        mut self,
        observer: Arc<Mutex<Option<meerkat_mobkit::mob_handle_runtime::MobRuntime>>>,
    ) -> Result<Self, HostLifetimeError> {
        if self.machine.get().is_some()
            || self.runtime_observer.is_some()
            || observer
                .lock()
                .map_err(|_| HostLifetimeError::Unavailable)?
                .is_some()
        {
            return Err(HostLifetimeError::InvalidSelection);
        }
        self.runtime_observer = Some(observer);
        Ok(self)
    }

    /// Select the trusted application's review using this host's actual native
    /// lifetime. Runs once before native construction; no wire document selects
    /// it. The callback must not reenter the lifetime or detach work. It may
    /// inspect `selected_policy()` under this existing current-selection fence.
    pub fn with_operation_review(
        mut self,
        select: impl FnOnce(&NativeHostLifetime) -> Result<SetupOperationReview, HostLifetimeError>,
    ) -> Result<Self, HostLifetimeError> {
        if self.operation_review.is_some()
            || self.machine.get().is_some()
            || self.operation_review_offered.get().is_some()
        {
            return Err(HostLifetimeError::WrongOwner);
        }
        let selected = self.owner.with_current_setup(|| {
            if self.private_closed.is_closed() {
                return Err(HostLifetimeError::Denied);
            }
            let selected = select(&self.owner)?;
            if self.private_closed.is_closed() {
                return Err(HostLifetimeError::Denied);
            }
            Ok(selected)
        })?;
        self.operation_review = Some(selected);
        Ok(self)
    }

    /// The exact selection used by the native factory hook. This observation
    /// grants no operation or human eligibility. The first offer freezes the
    /// selection, including an ordinary None, before native factory use.
    pub fn selected_operation_review(
        &self,
    ) -> Result<Option<Arc<meerkat_core::approval::review::BoundOperationReview>>, GatewayHostError>
    {
        self.operation_review_offered.get_or_init(|| ());
        self.owner
            .with_current_setup(|| {
                if self.private_closed.is_closed()
                    || ((self.setup_member_factory.is_some() || self.setup_application.is_some())
                        && self.prepared_application_selected.get().is_none())
                {
                    return Err(HostLifetimeError::Denied);
                }
                let review = self
                    .operation_review
                    .as_ref()
                    .map(|current| current())
                    .transpose()?;
                if self.private_closed.is_closed() {
                    return Err(HostLifetimeError::Denied);
                }
                Ok(review)
            })
            .map_err(host_error)
    }

    /// Reserve a compiled planning member before `prepare_bootstrap`.
    /// A validated reservation selects this exact host lifetime in the source
    /// owner and composes its retained registration hook. Invalid reservations
    /// select neither. Actual machine binding remains in normal bootstrap.
    /// Fresh members require real held completion. An ordinary resumed member
    /// may restore historical source custody from exact retained P/G evidence,
    /// but stays restricted pending workflow attachment. Neither path permits
    /// appending to an already active host.
    /// A later host-currentness or bootstrap failure retains the restriction;
    /// it does not transfer the source to a different host or imply activation.
    pub async fn reserve_workflow_member(
        &mut self,
        terms: &meerkat_toolkit::setup::SetupWorkflowTerms,
        original: &PrivateOriginalRef,
        spec: &meerkat_mobkit::identity_first::DurableAgentSpec,
    ) -> Result<Arc<crate::private_originals::InstalledWorkflowMember>, HostLifetimeError> {
        self.owner.validate_current_selection()?;
        if self.private_closed.is_closed() || self.machine.get().is_some() {
            return Err(HostLifetimeError::Denied);
        }
        let originals = self
            .private_originals
            .as_ref()
            .ok_or(HostLifetimeError::Unavailable)?;
        let registration = self
            .member_registration
            .clone()
            .ok_or(HostLifetimeError::Unavailable)?;
        let registry = self
            .installed_tool_policy
            .clone()
            .ok_or(HostLifetimeError::Unavailable)?;
        let member = originals
            .reserve_workflow_member_for_host(&self.owner, terms, original, spec)
            .await
            .map_err(|error| match error {
                PrivateOriginalError::Unavailable => HostLifetimeError::Unavailable,
                _ => HostLifetimeError::Denied,
            })?;
        // The source reservation completes synchronously after its writer is
        // acquired. Retain its participant before any later refusal, with no
        // intervening await that could abandon an unregistered selection.
        self.member_registration = Some(Arc::new(
            crate::installed_host::prepared_registration::PreparedMemberRegistration::from_installed(
                registration, registry, member.registration_participant(),
            ),
        ));
        self.owner.validate_current_selection()?;
        if self.private_closed.is_closed() {
            return Err(HostLifetimeError::Denied);
        }
        Ok(member)
    }
    /// Restore an already committed inline disclosure from the host's exact
    /// private G/P WorkGraph service. The expected audiences must equal the
    /// original recorded selection. This does not recreate Find or a native
    /// input, start recovery, or resolve unknown historical delivery. An absent
    /// record remains Pending; a reconstructed committed origin cannot dispatch.
    pub async fn restore_committed_workflow_context(
        &self,
        planning: &Arc<crate::private_originals::InstalledWorkflowPlanning>,
        membership: &meerkat_toolkit::setup::SetupWorkflowWaiter,
        recipient: meerkat_workgraph::WorkGraphService,
        backend: &meerkat_toolkit::setup::SetupWorkBackend,
        output_audiences: std::collections::BTreeSet<toolkit_package::Id>,
    ) -> Result<crate::private_originals::WorkflowContextObservation, PrivateOriginalError> {
        self.owner
            .validate_current_selection()
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        if self.private_closed.is_closed() {
            return Err(PrivateOriginalError::Denied);
        }
        let source = self
            .private_originals
            .as_ref()
            .ok_or(PrivateOriginalError::Unavailable)?;
        source.check_context_restore_host(&self.owner)?;
        let acquisition = self
            .acquisition
            .lock()
            .map_err(|_| PrivateOriginalError::Unavailable)?
            .configured()
            .ok_or(PrivateOriginalError::Unavailable)?;
        // Only the control published after the normal Ready/start boundary is
        // usable. An open default control cannot bypass that native lifecycle.
        let control = self
            .acquisition_recovery
            .published_transfer_control(&acquisition, source, backend)
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        planning
            .restore_committed_context(
                source,
                membership,
                recipient,
                backend,
                output_audiences,
                &control,
            )
            .await
    }

    pub(crate) fn bind_member_mcp_presenter(
        &self,
        presenter: Arc<dyn crate::installed_host::member_mcp_consumer::MemberMcpPresenter>,
    ) -> Result<(), HostLifetimeError> {
        self.member_mcp_originals()?
            .member_mcp_presenter
            .set(presenter)
            .map_err(|_| HostLifetimeError::WrongOwner)
    }
    pub(crate) fn member_mcp_originals(&self) -> Result<Arc<PrivateOriginals>, HostLifetimeError> {
        self.owner.validate_current_selection()?;
        self.private_originals
            .clone()
            .ok_or(HostLifetimeError::Unavailable)
    }
    pub(crate) fn member_mcp_operator(
        &self,
    ) -> Result<crate::installed_host::operator_setup::OperatorSetup, HostLifetimeError> {
        self.member_mcp_originals()?
            .application_operator_setup(self.owner.clone())
    }
    pub(crate) fn member_mcp_closed(&self) -> PrivateHostClosed {
        self.private_closed.clone()
    }
    pub(crate) fn bind_member_mcp_application(
        &self,
        participant: Arc<dyn crate::installed_host::member_mcp_consumer::MemberMcpApplication>,
    ) -> Result<(), HostLifetimeError> {
        self.owner.validate_current_selection()?;
        self.private_originals
            .as_ref()
            .ok_or(HostLifetimeError::Unavailable)?
            .bind_member_mcp_application(participant)
    }
    pub(crate) fn has_private_owner(&self, owner: &Arc<dyn PrivateHostLifetimeControl>) -> bool {
        let actual: Arc<dyn PrivateHostLifetimeControl> = self.owner.clone();
        Arc::ptr_eq(owner, &actual)
    }

    pub fn with_member_class_policy(
        mut self,
        policy: crate::installed_host::tool_policy::PreparedInstalledToolPolicy,
    ) -> Result<Self, HostLifetimeError> {
        if self.member_class_policy.is_some()
            || self.machine.get().is_some()
            || !self
                .installed_tool_policy
                .as_ref()
                .is_some_and(|registry| Arc::ptr_eq(registry, &policy.registry()))
        {
            return Err(HostLifetimeError::WrongOwner);
        }
        self.owner.validate_current_selection()?;
        self.member_class_policy = Some(policy);
        Ok(self)
    }

    pub(crate) fn member_class_policy(
        &self,
    ) -> Result<crate::installed_host::tool_policy::PreparedInstalledToolPolicy, HostLifetimeError>
    {
        self.owner.validate_current_selection()?;
        if self.private_closed.is_closed() {
            return Err(HostLifetimeError::Denied);
        }
        self.member_class_policy
            .clone()
            .ok_or(HostLifetimeError::Unavailable)
    }

    /// Retain the existing native context, including its roster and customizer.
    /// Fresh preparation adopts this exact context after its held completion.
    pub fn bind_member_class_context(
        &self,
        context: meerkat_mobkit::identity_first::PublicIdentityFirstContext,
    ) -> Result<(), HostLifetimeError> {
        let bound = self
            .private_originals
            .as_ref()
            .ok_or(HostLifetimeError::Unavailable)?
            .bound_identity_runtime()
            .map_err(|_| HostLifetimeError::Unavailable)?;
        if !bound
            .owner()
            .handle()
            .actor_task_watch()
            .observes(&context.owner().handle())
        {
            return Err(HostLifetimeError::WrongOwner);
        }
        self.member_class_context
            .set(context)
            .map_err(|_| HostLifetimeError::WrongOwner)
    }

    // Only fresh private Add requests use this entry, before policy reservation.
    // Bootstrap binding and entered finalizers keep their retained context.
    pub(crate) fn admit_member_class_context(
        &self,
    ) -> Result<meerkat_mobkit::identity_first::PublicIdentityFirstContext, HostLifetimeError> {
        self.member_class_policy()?;
        self.private_originals
            .as_ref()
            .ok_or(HostLifetimeError::Unavailable)?
            .with_new_work_admission(|| ())
            .map_err(|_| HostLifetimeError::Unavailable)?;
        self.member_class_context
            .get()
            .cloned()
            .ok_or(HostLifetimeError::Unavailable)
    }

    /// Join a trusted configured resource to the existing installed native
    /// owners before bootstrap takes them. The caller composes any existing
    /// attachment provider; the actual registration hook remains authoritative.
    pub fn with_configured_mcp_resource<F>(
        mut self,
        compose: F,
        selected: crate::installed_host::prepared_registration::SelectedDestinationParticipant,
    ) -> Result<Self, HostLifetimeError>
    where
        F: FnOnce(
            Option<Arc<dyn meerkat_runtime::meerkat_machine::ConfiguredMcpAttachmentProvider>>,
        )
            -> Arc<dyn meerkat_runtime::meerkat_machine::ConfiguredMcpAttachmentProvider>,
    {
        self.owner.validate_current_selection()?;
        if self.private_closed.is_closed() || self.machine.get().is_some() {
            return Err(HostLifetimeError::Denied);
        }
        let registration = self
            .member_registration
            .clone()
            .ok_or(HostLifetimeError::Unavailable)?;
        let registry = self
            .installed_tool_policy
            .clone()
            .ok_or(HostLifetimeError::Unavailable)?;
        let configuration = self
            .grant_configuration
            .get_mut()
            .map_err(|_| HostLifetimeError::Unavailable)?
            .as_mut()
            .and_then(Option::as_mut)
            .ok_or(HostLifetimeError::Unavailable)?;
        configuration.configured_mcp_attachment =
            Some(compose(configuration.configured_mcp_attachment.take()));
        self.member_registration = Some(Arc::new(
            crate::installed_host::prepared_registration::PreparedMemberRegistration::from_installed(
                registration, registry, selected,
            ),
        ));
        Ok(self)
    }

    /// Fence new source inputs, member additions and recovery without closing
    /// the selected owner or cancelling entered callbacks. Full close retains
    /// its existing resource, approval and task-join responsibilities.
    pub fn stop_admitting_work(&self) -> Result<(), GatewayHostError> {
        self.collector_adapters.stop();
        let source = self
            .private_originals
            .as_ref()
            .map(|source| source.stop_admitting_work())
            .transpose();
        self.acquisition_recovery.stop();
        source
            .map(|_| ())
            .map_err(|_| GatewayHostError::Unavailable)
    }

    /// Submit an authenticated original through this gateway's existing native
    /// runtime. This admits the Rust call through the same lifetime boundary as
    /// the private pipe; the native result alone reports input admission.
    pub async fn submit_authenticated_original(
        &self,
        submission: crate::private_originals::PrivateOriginalSubmit,
    ) -> Result<crate::private_originals::PrivateOriginalSubmission, PrivateOriginalError> {
        if self.private_closed.is_closed() {
            return Err(PrivateOriginalError::Denied);
        }
        self.owner
            .validate_current_selection()
            .map_err(|error| match error {
                HostLifetimeError::InvalidSelection
                | HostLifetimeError::Denied
                | HostLifetimeError::WrongOwner => PrivateOriginalError::Denied,
                HostLifetimeError::Unavailable | HostLifetimeError::NotSettled => {
                    PrivateOriginalError::Unavailable
                }
            })?;
        self.private_originals
            .as_ref()
            .ok_or(PrivateOriginalError::Unavailable)?
            .submit_authenticated_original(submission)
            .await
    }

    /// Join the per-call native preparation to this exact existing source
    /// owner before entering local operator review. Equal route or principal
    /// values on another installed gateway cannot substitute for this owner.
    pub fn local_operator_setup_for(
        &self,
        prepared: &crate::private_originals::LocalControllerSetup,
    ) -> Result<crate::installed_host::operator_setup::OperatorSetup, HostLifetimeError> {
        let captured = prepared
            .source_owner()
            .ok_or(HostLifetimeError::Unavailable)?;
        let current = self
            .private_originals
            .as_ref()
            .ok_or(HostLifetimeError::Unavailable)?;
        if !Arc::ptr_eq(&captured, current) {
            return Err(HostLifetimeError::WrongOwner);
        }
        self.local_operator_setup()
    }

    /// Remote protected review uses the same captured source owner, with its
    /// operator selected only by the trusted compiled application bootstrap.
    pub fn application_operator_setup_for(
        &self,
        prepared: &crate::private_originals::LocalControllerSetup,
    ) -> Result<crate::installed_host::operator_setup::OperatorSetup, HostLifetimeError> {
        let captured = prepared
            .source_owner()
            .ok_or(HostLifetimeError::Unavailable)?;
        let current = self
            .private_originals
            .as_ref()
            .ok_or(HostLifetimeError::Unavailable)?;
        if !Arc::ptr_eq(&captured, current) {
            return Err(HostLifetimeError::WrongOwner);
        }
        if self.private_closed.is_closed() {
            return Err(HostLifetimeError::Denied);
        }
        self.owner.validate_current_selection()?;
        let helpers = self
            .acquisition
            .lock()
            .map_err(|_| HostLifetimeError::Unavailable)?
            .configured()
            .map(|acquisition| acquisition.helper_selections())
            .unwrap_or_default();
        current
            .application_operator_setup(self.owner.clone())?
            .with_helper_selections(helpers)
    }

    /// Prepare the existing local presenter from this member's actual native
    /// controller client. The native binder deliberately returns before input
    /// submission, releasing the identity lease before any human interaction.
    /// The returned value is process-local and grants no controller authority.
    pub async fn prepare_local_controller_setup(
        &self,
        submission: crate::private_originals::PrivateOriginalSubmit,
    ) -> Result<crate::private_originals::LocalControllerSetup, PrivateOriginalError> {
        if self.private_closed.is_closed() {
            return Err(PrivateOriginalError::Denied);
        }
        self.owner
            .validate_current_selection()
            .map_err(|error| match error {
                HostLifetimeError::InvalidSelection
                | HostLifetimeError::Denied
                | HostLifetimeError::WrongOwner => PrivateOriginalError::Denied,
                HostLifetimeError::Unavailable | HostLifetimeError::NotSettled => {
                    PrivateOriginalError::Unavailable
                }
            })?;
        self.private_originals
            .as_ref()
            .ok_or(PrivateOriginalError::Unavailable)?
            .prepare_local_controller_setup(submission)
            .await
    }

    /// Commit a distinct reviewed setup choice to its exact pending original.
    /// The current controller policy is unchanged and no native input enters.
    /// A changed session, cancelled source or existing final binding refuses.
    pub async fn select_original_setup_request(
        &self,
        prepared: &crate::private_originals::LocalControllerSetup,
        parent: Vec<meerkat_authorization_contracts::grant::GrantLineageRef>,
    ) -> Result<crate::private_originals::PrivateSetupRequestSelection, PrivateOriginalError> {
        if self.private_closed.is_closed() {
            return Err(PrivateOriginalError::Denied);
        }
        self.owner
            .validate_current_selection()
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        self.private_originals
            .as_ref()
            .ok_or(PrivateOriginalError::Unavailable)?
            .select_setup_request(prepared, parent)
            .await
    }

    /// Same-process OS-operator setup over this installed gateway's existing
    /// stores and native source owner. No private or public wire method can
    /// construct this view, and it cannot reopen or replace the grant owner.
    pub fn local_operator_setup(
        &self,
    ) -> Result<crate::installed_host::operator_setup::OperatorSetup, HostLifetimeError> {
        if self.private_closed.is_closed() {
            return Err(HostLifetimeError::Denied);
        }
        self.owner.validate_current_selection()?;
        let helpers = self
            .acquisition
            .lock()
            .map_err(|_| HostLifetimeError::Unavailable)?
            .configured()
            .map(|acquisition| acquisition.helper_selections())
            .unwrap_or_default();
        self.private_originals
            .as_ref()
            .ok_or(HostLifetimeError::Unavailable)?
            .local_operator_setup(self.owner.clone())?
            .with_helper_selections(helpers)
    }

    /// Apply an authenticated application's exact access decision through the
    /// same current activation owner used by the inherited private channel.
    /// This check admits the call before its asynchronous store/native work;
    /// closing later does not turn already-admitted issuance into a refusal.
    /// The source adapter's unfenced mutator is not part of the public Rust API.
    pub async fn apply_service_access_decision(
        &self,
        decision: PrivateServiceAccessDecision,
    ) -> Result<PrivateServiceGrantObservation, PrivateOriginalError> {
        if self.private_closed.is_closed() {
            return Err(PrivateOriginalError::Denied);
        }
        self.owner
            .validate_current_selection()
            .map_err(|error| match error {
                HostLifetimeError::InvalidSelection
                | HostLifetimeError::Denied
                | HostLifetimeError::WrongOwner => PrivateOriginalError::Denied,
                HostLifetimeError::Unavailable | HostLifetimeError::NotSettled => {
                    PrivateOriginalError::Unavailable
                }
            })?;
        self.private_originals
            .as_ref()
            .ok_or(PrivateOriginalError::Unavailable)?
            .apply_service_access_decision(decision)
            .await
    }

    /// Apply the trusted application's reviewed MCP selection through the same
    /// access owner. This Rust-only selection is not deserializable or available
    /// on the private wire. The caller must retain this future through settlement.
    /// It selects no original, grants no continuation and does not attest readiness.
    pub async fn apply_mcp_service_access_decision(
        &self,
        approved: crate::installed_host::operator_setup::ReviewedServiceMcpParent,
    ) -> Result<PrivateServiceGrantObservation, PrivateOriginalError> {
        if self.private_closed.is_closed() {
            return Err(PrivateOriginalError::Denied);
        }
        self.owner
            .validate_current_selection()
            .map_err(|error| match error {
                HostLifetimeError::InvalidSelection
                | HostLifetimeError::Denied
                | HostLifetimeError::WrongOwner => PrivateOriginalError::Denied,
                HostLifetimeError::Unavailable | HostLifetimeError::NotSettled => {
                    PrivateOriginalError::Unavailable
                }
            })?;
        self.private_originals
            .as_ref()
            .ok_or(PrivateOriginalError::Unavailable)?
            .apply_mcp_service_access_decision(approved)
            .await
    }

    /// Durably cancel only this authenticated original's operations through
    /// the same activation fence as the private channel. No standing grant,
    /// sibling, controller or runtime run is revoked or stopped. If the caller
    /// drops this future during persistence, the source owner remains Pending
    /// until exact reconciliation, never Active or falsely completed.
    pub async fn cancel_original_operation(
        &self,
        original: &PrivateOriginalRef,
    ) -> Result<PrivateOriginalCancellation, PrivateOriginalError> {
        if self.private_closed.is_closed() {
            return Err(PrivateOriginalError::Denied);
        }
        self.owner
            .validate_current_selection()
            .map_err(|error| match error {
                HostLifetimeError::InvalidSelection
                | HostLifetimeError::Denied
                | HostLifetimeError::WrongOwner => PrivateOriginalError::Denied,
                HostLifetimeError::Unavailable | HostLifetimeError::NotSettled => {
                    PrivateOriginalError::Unavailable
                }
            })?;
        self.private_originals
            .as_ref()
            .ok_or(PrivateOriginalError::Unavailable)?
            .cancel_original_operation(original)
            .await
    }

    /// Retain the legacy constructor without manufacturing native owners.
    /// It has no registration owner and bootstrap reports typed unavailable.
    /// A governed launch uses `with_governed_owners`; restore failures must
    /// never select this constructor as a fallback.
    pub fn new(owner: Arc<NativeHostLifetime>, private_closed: PrivateHostClosed) -> Self {
        Self::with_grant_configuration(owner, private_closed, None, None)
    }

    /// Consume the configuration composed by the actual Toolkit original,
    /// grant and policy owners. The gateway takes it exactly once through E1.
    /// The application's actual native registration hook selects the exact
    /// destination and verifies its current surface, including dynamic tools.
    /// It must agree with the original-source and operation-policy owners.
    /// This constructor never restores again, issues grants or selects policy.
    pub fn with_governed_owners(
        owner: Arc<NativeHostLifetime>,
        private_closed: PrivateHostClosed,
        configuration: NativeGrantWorkConfiguration,
        member_registration: Arc<dyn MemberRuntimeRegistrationHook>,
    ) -> Self {
        Self::with_grant_configuration(
            owner,
            private_closed,
            Some(configuration),
            Some(member_registration),
        )
    }

    /// Shared Python/TypeScript executable composition. Both private source
    /// commands and E1 use the same original/grant owner, and the adapter is
    /// given only the actual native machine built from that configuration.
    pub fn with_private_originals<P>(
        owner: Arc<NativeHostLifetime>,
        private_closed: PrivateHostClosed,
        originals: Arc<PrivateOriginals>,
        policy: Arc<P>,
        member_registration: Arc<dyn MemberRuntimeRegistrationHook>,
    ) -> Self
    where
        P: meerkat_runtime::meerkat_machine::ScopedMandateOwner
            + meerkat_authorization::grant_policy::AdmittedWorkPolicyOwner
            + meerkat_authorization::grant_policy::OperationPolicyOwner
            + meerkat_runtime::meerkat_machine::RetainedInvocationOwner
            + 'static,
    {
        let configuration = originals.configuration(policy);
        let mut host = Self::with_grant_configuration(
            owner,
            private_closed,
            Some(configuration),
            Some(member_registration),
        );
        host.private_originals = Some(originals);
        host
    }

    /// The same preparation already supplied the native configuration. This
    /// final host must not construct or offer a second native grant owner.
    pub(crate) fn with_prepared_private_originals(
        owner: Arc<NativeHostLifetime>,
        private_closed: PrivateHostClosed,
        originals: Arc<PrivateOriginals>,
        member_registration: Arc<dyn MemberRuntimeRegistrationHook>,
    ) -> Self {
        let mut host =
            Self::with_grant_configuration(owner, private_closed, None, Some(member_registration));
        host.grant_configuration = Mutex::new(None);
        host.private_originals = Some(originals);
        host
    }

    /// Use this gateway's actual private connection login owner. Installed
    /// routes and native verified humans still select every Console original.
    pub fn with_console_logins(
        mut self,
        logins: crate::private_host::ConsoleLogins,
    ) -> Result<Self, HostLifetimeError> {
        if self.console_logins.is_some()
            || self.machine.get().is_some()
            || self.private_originals.is_none()
        {
            return Err(HostLifetimeError::InvalidSelection);
        }
        self.console_logins = Some(logins);
        Ok(self)
    }

    /// Bind this private connection's hosted auth endpoint at the real factory
    /// offer. The trusted launcher selects the registered HTTPS base; this does
    /// not mount a route or create another login, account or configuration owner.
    pub fn with_hosted_mcp_auth(
        mut self,
        endpoint: crate::private_host::HostedMcpAuthEndpoint,
        redirect_base: meerkat::McpRegisteredRedirectBase,
    ) -> Result<Self, HostLifetimeError> {
        if self.hosted_mcp.is_some()
            || self.machine.get().is_some()
            || self.private_originals.is_none()
            || self.private_closed.is_closed()
        {
            return Err(HostLifetimeError::InvalidSelection);
        }
        self.hosted_mcp = Some((endpoint, redirect_base));
        Ok(self)
    }

    /// Exact native registry built from the selected canonical application
    /// policy. Member Provider bindings still belong to native spawn material;
    /// registry installation alone never changes Unmanaged into permission.
    pub(crate) fn with_installed_tool_policy(
        mut self,
        registry: Arc<meerkat_core::ToolConsequencePolicyRegistry>,
    ) -> Self {
        self.installed_tool_policy = Some(registry);
        self
    }

    pub(crate) fn with_installed_acquisition(
        mut self,
        acquisition: crate::installed_host::acquisition::AcquisitionBootstrap,
    ) -> Self {
        self.acquisition = Arc::new(Mutex::new(acquisition));
        self
    }

    /// Only InstalledToolkitHost transfers this already validated owner during
    /// construction. Public setup attachment remains a separate one-use path.
    pub(crate) fn with_installed_prepared_application(
        mut self,
        application: Option<PreparedSetupApplication>,
    ) -> Self {
        self.setup_application = application;
        self
    }

    /// Actual first-party setup composition, before `prepare_bootstrap`.
    /// The exact source must belong to this host; no independent result store
    /// or new delivery owner can be injected through this convenience seam.
    pub fn with_native_setup_bootstrap(
        mut self,
        continuation: Arc<crate::installed_host::acquisition::InstalledNativeContinuation>,
        projection: SetupBootstrapProjection,
        application: PreparedSetupApplication,
    ) -> Result<Self, HostLifetimeError> {
        if self.native_continuation.is_some()
            || self.setup_application.is_some()
            || self.machine.get().is_some()
            || !self
                .private_originals
                .as_ref()
                .is_some_and(|source| continuation.shares_source(source))
        {
            return Err(HostLifetimeError::InvalidSelection);
        }
        self.native_continuation = Some(continuation);
        self.setup_projection = Some(projection);
        self.setup_application = Some(application);
        Ok(self)
    }

    /// Retain the selected application's private presenter composition before
    /// native factory construction. Ordinary hosts have no callback. This
    /// requires the already authenticated prepared setup application and cannot
    /// replace a callback or a running host's factory configuration.
    pub fn with_setup_member_factory(
        mut self,
        compose: SetupMemberFactoryComposition,
    ) -> Result<Self, HostLifetimeError> {
        if self.setup_application.is_none()
            || self.setup_member_factory.is_some()
            || self.machine.get().is_some()
            || self.private_closed.is_closed()
        {
            return Err(HostLifetimeError::InvalidSelection);
        }
        self.owner.validate_current_selection()?;
        self.setup_member_factory = Some(compose);
        Ok(self)
    }

    /// Select an application-policy factory on this exact installed lifetime.
    /// Matching serialized configuration cannot substitute another live owner.
    /// The ordinary factory slot/currentness checks remain the sole admission.
    pub fn with_setup_member_factory_for_owner(
        self,
        owner: &Arc<NativeHostLifetime>,
        compose: SetupMemberFactoryComposition,
    ) -> Result<Self, HostLifetimeError> {
        if !Arc::ptr_eq(owner, &self.owner) {
            return Err(HostLifetimeError::WrongOwner);
        }
        self.with_setup_member_factory(compose)
    }

    /// Bootstrap selection only, derived from the one retained owner state.
    /// Configured does not mean native helper execution or continuation ready.
    pub fn acquisition_bootstrap_status(
        &self,
    ) -> Result<crate::installed_host::acquisition::AcquisitionBootstrapStatus, GatewayHostError>
    {
        self.acquisition
            .lock()
            .map(|selection| selection.status())
            .map_err(|_| GatewayHostError::Unavailable)
    }

    /// Publish this bound in-process runtime's actual bootstrap completion once
    /// and arm this host's existing acquisition recovery owner. This is not an
    /// SDK Ready publication, input admission, or a recovery/drain receipt.
    ///
    /// The caller first completes the runtime's supported identity bootstrap.
    /// Machine, identity owner and native mob incarnation must still be this
    /// host's bound construction before its one-use completion is taken.
    /// Publication and the short synchronous task spawn run under the existing
    /// host and runtime close gates. No work is awaited and neither gate is
    /// reentered; the existing host close/join retains the spawned task.
    pub fn publish_runtime_bootstrap_completed(
        &self,
        runtime: &UnifiedRuntime,
    ) -> Result<(), GatewayHostError> {
        self.owner
            .validate_current_selection()
            .map_err(host_error)?;
        let expected = self.machine.get().ok_or(GatewayHostError::Unavailable)?;
        let machine = runtime
            .mob_runtime()
            .session_service()
            .ok_or(GatewayHostError::Unavailable)?
            .acquire_runtime_adapter(None)
            .map_err(|_| GatewayHostError::Unavailable)?
            .ok_or(GatewayHostError::Unavailable)?;
        if !Arc::ptr_eq(expected, &machine) {
            return Err(GatewayHostError::Denied);
        }
        let originals = self
            .private_originals
            .as_ref()
            .ok_or(GatewayHostError::Unavailable)?;
        let bound = originals
            .bound_identity_runtime()
            .map_err(|_| GatewayHostError::Unavailable)?;
        let actual = runtime
            .identity_runtime()
            .ok_or(GatewayHostError::Unavailable)?;
        if !std::ptr::eq(&*bound, &*actual)
            || bound.owner().handle().incarnation() != runtime.mob_runtime().handle().incarnation()
        {
            return Err(GatewayHostError::Denied);
        }
        let acquisition = self
            .acquisition
            .lock()
            .map_err(|_| GatewayHostError::Unavailable)?
            .configured();
        let mut recovery = acquisition
            .map(|owner| {
                self.acquisition_recovery
                    .prepare_start()
                    .map(|handle| (handle, owner))
            })
            .transpose()
            .map_err(host_error)?;
        let mut completion = Some(
            runtime
                .take_bootstrap_completion()
                .map_err(|_| GatewayHostError::Unavailable)?,
        );
        self.owner
            .publish_gateway_ready(&mut || {
                let completion = completion.take().ok_or(HostLifetimeError::Denied)?;
                completion
                    .publish(|| match recovery.take() {
                        Some((handle, owner)) => self.acquisition_recovery.start(handle, owner),
                        None => Ok(()),
                    })
                    .map_err(|_| HostLifetimeError::Unavailable)?
            })
            .map_err(host_error)
    }

    /// Test-only observation of this host's actual configured acquisition owner.
    #[cfg(feature = "test-support")]
    pub fn observe_acquisition_recovery(
        &self,
    ) -> Result<crate::installed_host::acquisition::AcquisitionRecoveryObservation, GatewayHostError>
    {
        let owner = self
            .acquisition
            .lock()
            .map_err(|_| GatewayHostError::Unavailable)?
            .configured()
            .ok_or(GatewayHostError::Unavailable)?;
        self.acquisition_recovery
            .observe(&owner)
            .map_err(|_| GatewayHostError::Unavailable)
    }

    fn with_grant_configuration(
        owner: Arc<NativeHostLifetime>,
        private_closed: PrivateHostClosed,
        configuration: Option<NativeGrantWorkConfiguration>,
        member_registration: Option<Arc<dyn MemberRuntimeRegistrationHook>>,
    ) -> Self {
        Self {
            owner,
            private_closed,
            machine: OnceLock::new(),
            #[cfg(feature = "test-support")]
            runtime_observer: None,
            grant_configuration: Mutex::new(Some(configuration)),
            member_registration,
            private_originals: None,
            console_logins: None,
            hosted_mcp: None,
            installed_tool_policy: None,
            member_class_policy: None,
            member_class_context: OnceLock::new(),
            member_class_operations: Default::default(),
            collector_adapters: Default::default(),
            acquisition: Arc::new(Mutex::new(Default::default())),
            native_continuation: None,
            setup_projection: None,
            setup_application: None,
            setup_member_factory: None,
            operation_review: None,
            operation_review_offered: OnceLock::new(),
            console_routes: None,
            local_recovery: None,
            prepared_application_selected: OnceLock::new(),
            acquisition_recovery: Default::default(),
        }
    }
}

fn host_error(error: HostLifetimeError) -> GatewayHostError {
    match error {
        HostLifetimeError::InvalidSelection
        | HostLifetimeError::Denied
        | HostLifetimeError::WrongOwner => GatewayHostError::Denied,
        HostLifetimeError::Unavailable => GatewayHostError::Unavailable,
        HostLifetimeError::NotSettled => GatewayHostError::SettlementIncomplete,
    }
}

#[async_trait]
impl GatewayHostComposition for ToolkitGatewayHost {
    fn operation_review(
        &self,
        _construction: &GatewayFactoryConstruction<'_>,
    ) -> Result<Option<Arc<meerkat_core::approval::review::BoundOperationReview>>, GatewayHostError>
    {
        self.selected_operation_review()
    }

    fn stop_admitting(&self, _reason: GatewayHostCloseReason) -> Result<(), GatewayHostError> {
        self.stop_admitting_work()
    }

    fn compose_agent_customizer(
        &self,
        customizer: Option<Arc<dyn meerkat_mobkit::identity_first::AgentCustomizer>>,
    ) -> Result<Option<Arc<dyn meerkat_mobkit::identity_first::AgentCustomizer>>, GatewayHostError>
    {
        let classes_declared = self
            .owner
            .current_setup_policy()
            .map_err(host_error)?
            .native_host
            .as_ref()
            .is_some_and(|native| !native.member_classes.is_empty());
        let composed = self
            .owner
            .with_current_setup(|| {
                if self.private_closed.is_closed() {
                    return Err(HostLifetimeError::Denied);
                }
                Ok(match (&self.member_class_policy, customizer) {
                    (Some(policy), Some(inner)) => {
                        Some(policy.member_class_gateway_customizer(inner))
                    }
                    (_, unchanged) => unchanged,
                })
            })
            .map_err(host_error)?;
        if classes_declared && composed.is_none() {
            return Err(GatewayHostError::AgentCustomizerRequired);
        }
        Ok(composed)
    }
    fn host_app_routes(
        &self,
        viewer: meerkat_mobkit::http_console::ConsoleViewerResolver,
    ) -> Result<Option<axum::Router>, GatewayHostError> {
        self.owner
            .with_current_setup(|| {
                if self.private_closed.is_closed() {
                    return Err(HostLifetimeError::Denied);
                }
                self.console_routes
                    .as_ref()
                    .map(|routes| routes(viewer))
                    .transpose()
            })
            .map_err(host_error)
    }
    fn compose_member_factory(
        &self,
        construction: &GatewayFactoryConstruction<'_>,
    ) -> Result<Option<GatewayMcpComposition>, GatewayHostError> {
        if self.setup_member_factory.is_none()
            && self.hosted_mcp.is_none()
            && self.member_class_policy.is_none()
        {
            return Ok(None);
        }
        if self.setup_member_factory.is_some() || self.member_class_policy.is_some() {
            self.private_originals
                .as_ref()
                .ok_or(GatewayHostError::Unavailable)?
                .capture_member_mcp_factory(construction)
                .map_err(|_| GatewayHostError::Unavailable)?;
        }
        self.owner
            .with_current_setup(|| {
                if self.private_closed.is_closed()
                    || ((self.setup_member_factory.is_some() || self.setup_application.is_some())
                        && self.prepared_application_selected.get().is_none())
                {
                    return Err(HostLifetimeError::Denied);
                }
                if self.setup_member_factory.is_some() {
                    let native = self
                        .owner
                        .selected_policy()
                        .native_host
                        .as_ref()
                        .ok_or(HostLifetimeError::InvalidSelection)?;
                    // The factory's installation realm is separate from the member
                    // realm owned by this policy and checked at adapter construction.
                    if native.members.is_empty()
                        || native.members.iter().any(|member| {
                            meerkat_core::mob_realm_id(&member.member.mob_id)
                                .as_ref()
                                .ok()
                                != Some(&native.realm)
                        })
                    {
                        return Err(HostLifetimeError::WrongOwner);
                    }
                }
                if let Some((endpoint, base)) = &self.hosted_mcp {
                    endpoint.bind_factory(construction, base.clone())?;
                }
                let composition = self
                    .setup_member_factory
                    .as_ref()
                    .map(|compose| {
                        compose(
                            construction,
                            crate::installed_host::acquisition::InstalledServiceTaskSource::new(
                                &self.acquisition,
                            ),
                        )
                    })
                    .transpose()?;
                if self.private_closed.is_closed() {
                    return Err(HostLifetimeError::Denied);
                }
                Ok(composition)
            })
            .map_err(host_error)
    }

    async fn prepared_application(
        &self,
        request: Option<&PreparedApplicationRequest>,
    ) -> Result<Option<PreparedGatewayApplication>, GatewayHostError> {
        let Some(application) = &self.setup_application else {
            return Ok(None);
        };
        self.owner
            .with_current_setup(|| {
                if self.private_closed.is_closed() {
                    return Err(HostLifetimeError::Denied);
                }
                // Every init must carry this selected application. An earlier
                // successful selection cannot authenticate a later absent carrier.
                let request = request.ok_or(HostLifetimeError::Denied)?;
                let selected =
                    application.select(request, self.owner.clone(), self.private_closed.clone())?;
                if self.private_closed.is_closed() {
                    return Err(HostLifetimeError::Denied);
                }
                self.prepared_application_selected.get_or_init(|| ());
                Ok(Some(selected))
            })
            .map_err(host_error)
    }

    fn native_grant_work_configuration(
        &self,
    ) -> Result<Option<NativeGrantWorkConfiguration>, GatewayHostError> {
        // The early native hook authenticates this init before storage. This
        // retained selection is only a later composition defense, never a
        // replacement for authenticating the carrier on every init.
        self.require_prepared_application().map_err(host_error)?;
        let configuration = self
            .grant_configuration
            .lock()
            .map_err(|_| GatewayHostError::Unavailable)?
            .take()
            .ok_or(GatewayHostError::Denied)?;
        if self.member_registration.is_none() {
            return Err(GatewayHostError::Unavailable);
        }
        configuration.map(Some).ok_or(GatewayHostError::Unavailable)
    }

    async fn prepare_bootstrap(
        &self,
        spec: MobBootstrapSpec,
    ) -> Result<MobBootstrapSpec, GatewayHostError> {
        self.require_prepared_application().map_err(host_error)?;
        self.owner
            .validate_current_selection()
            .map_err(host_error)?;
        let service_machine = spec
            .session_service
            .acquire_runtime_adapter(spec.runtime_adapter.clone())
            .map_err(|_| GatewayHostError::Unavailable)?
            .ok_or(GatewayHostError::Unavailable)?;
        if spec
            .runtime_adapter
            .as_ref()
            .is_some_and(|machine| !Arc::ptr_eq(machine, &service_machine))
        {
            return Err(GatewayHostError::Denied);
        }
        self.machine
            .set(service_machine.clone())
            .map_err(|_| GatewayHostError::Denied)?;
        if let Some(originals) = &self.private_originals {
            originals
                .bind_host_lifetime(&self.owner)
                .map_err(|_| GatewayHostError::Denied)?;
            originals
                .bind_machine(service_machine.clone())
                .map_err(|_| GatewayHostError::Denied)?;
            if self.member_class_policy.is_some() {
                originals
                    .reconcile_observed_member_mcp(self.owner.clone())
                    .await
                    .map_err(|_| GatewayHostError::Unavailable)?;
            }
            originals
                .install_peer_receiver(&service_machine)
                .map_err(|_| GatewayHostError::Denied)?;
        }
        let registration = self
            .member_registration
            .as_ref()
            .ok_or(GatewayHostError::Unavailable)?;
        let spec = spec.with_member_runtime_registration_hook(registration.clone());
        // The same fully composed spec supplies both the actual selected
        // WorkGraph and its original provider-open census, before any inputs.
        // No setter, replacement service or late bind_runtime lookup is used.
        let acquisition = self
            .acquisition
            .lock()
            .map_err(|_| GatewayHostError::Unavailable)?
            .prepare(spec.workgraph_service(), spec.resolved_storage.as_ref())
            .map_err(host_error)?;
        self.owner
            .validate_current_selection()
            .map_err(host_error)?;
        let spec = match acquisition {
            // The native registration owns the RegisteredToolBundleOperation
            // witness. Disabled acquisition has no driver or registration.
            Some(acquisition) => {
                let spec = if let Some(continuation) = &self.native_continuation {
                    let projection = self
                        .setup_projection
                        .as_ref()
                        .ok_or(GatewayHostError::Unavailable)?;
                    let spec = projection(spec).map_err(host_error)?;
                    // Core bca71's actual source constructor retains this
                    // exact adapter and namespace. Meta 2835 supplies the
                    // reviewed forwarding and final rebuild/rebind path. Governed
                    // Host completion, exact-content reconciliation and matched
                    // tuple execution remain separate qualification requirements.
                    let source = meerkat_mob_mcp::HostJobSource::new(
                        [continuation.namespace().to_owned()],
                        continuation.clone(),
                    )
                    .map_err(|_| GatewayHostError::Denied)?;
                    spec.with_host_job_source(source)
                        .map_err(|_| GatewayHostError::Unavailable)?
                } else {
                    spec
                };
                // Find admits a local setup request without using a service
                // account. State that at this exact host registration; later
                // setup operations retain their separate account declarations.
                let find = Arc::new(
                    meerkat_core::RegisteredToolBundleDispatcher::new(
                        crate::FIND_AND_INSTALL_BUNDLE,
                        acquisition.tool(),
                    )
                    .with_account(meerkat_core::OperationAccount::DeclaredNone),
                );
                spec.register_tool_bundle(crate::FIND_AND_INSTALL_BUNDLE, find)
                    .register_tool_bundle(
                        meerkat_toolkit::setup::SETUP_RESULTS_BUNDLE,
                        acquisition.result_tool(),
                    )
            }
            None => spec,
        };
        Ok(match &self.installed_tool_policy {
            Some(registry) => spec.with_tool_consequence_policy_registry(registry.clone()),
            None => spec,
        })
    }

    async fn bind_runtime(&self, runtime: &mut UnifiedRuntime) -> Result<(), GatewayHostError> {
        self.owner
            .validate_current_selection()
            .map_err(host_error)?;
        let expected = self.machine.get().ok_or(GatewayHostError::Unavailable)?;
        let actual = runtime
            .mob_runtime()
            .session_service()
            .ok_or(GatewayHostError::Unavailable)?
            .acquire_runtime_adapter(Some(expected.clone()))
            .map_err(|_| GatewayHostError::Unavailable)?
            .ok_or(GatewayHostError::Unavailable)?;
        if !Arc::ptr_eq(expected, &actual) {
            return Err(GatewayHostError::Denied);
        }
        if self.acquisition_bootstrap_status()?
            == crate::installed_host::acquisition::AcquisitionBootstrapStatus::Configured
            && let Some(continuation) = &self.native_continuation
        {
            continuation
                .bind_native(runtime.mob_runtime())
                .map_err(host_error)?;
        }
        #[cfg(feature = "test-support")]
        if let Some(observer) = &self.runtime_observer {
            let mut slot = observer.lock().map_err(|_| GatewayHostError::Unavailable)?;
            if slot.is_some() {
                return Err(GatewayHostError::Denied);
            }
            *slot = Some(runtime.mob_runtime().observer());
        }
        Ok(())
    }

    async fn bind_identity_runtime(
        &self,
        runtime: &mut UnifiedRuntime,
    ) -> Result<(), GatewayHostError> {
        self.owner
            .validate_current_selection()
            .map_err(host_error)?;
        if let Some(originals) = &self.private_originals {
            let identities = runtime
                .identity_runtime()
                .ok_or(GatewayHostError::Unavailable)?;
            if let Some(access) = runtime.access_controller() {
                access
                    .join_authorization_publication(originals.authorization_publication())
                    .map_err(|_| GatewayHostError::Denied)?;
            }
            originals
                .bind_identity_runtime(&identities)
                .map_err(|error| match error {
                    PrivateOriginalError::Unavailable => GatewayHostError::Unavailable,
                    _ => GatewayHostError::Denied,
                })?;
            if self.member_class_policy.is_some() {
                self.bind_member_class_context(
                    runtime
                        .identity_first_context()
                        .ok_or(GatewayHostError::Unavailable)?,
                )
                .map_err(host_error)?;
            }
            originals
                .start_custody_recovery(runtime.mob_runtime().continuation_source_announcer())
                .map_err(|_| GatewayHostError::Unavailable)?;
            if let Some(pins) = runtime.mob_runtime().controller_pin_source() {
                originals
                    .bind_controller_pin_source(pins)
                    .map_err(|_| GatewayHostError::Denied)?;
            }
            if let Some(logins) = &self.console_logins {
                let ingress = originals
                    .console_ingress(logins.clone(), runtime.access_controller().cloned())
                    .await
                    .map_err(|error| match error {
                        PrivateOriginalError::Unavailable => GatewayHostError::Unavailable,
                        _ => GatewayHostError::Denied,
                    })?;
                runtime.set_console_governed_ingress(ingress);
            }
        }
        Ok(())
    }

    fn publish_bootstrap_ready(
        &self,
        publication: GatewayBootstrapReady<'_>,
    ) -> Result<(), GatewayHostError> {
        if let Some(originals) = &self.private_originals {
            // A gateway without an identity context skips the identity hook.
            // Its source owner must not be presented as ready in that case.
            originals
                .bound_identity_runtime()
                .map_err(|_| GatewayHostError::Unavailable)?;
        }
        let acquisition = self
            .acquisition
            .lock()
            .map_err(|_| GatewayHostError::Unavailable)?
            .configured();
        // bind_runtime already retained the actual native service. Prepare the
        // task handle now, but enter no recovery work before ready publication.
        // A service Arc by itself is not binding or restore readiness.
        let recovery = acquisition
            .map(|owner| {
                self.acquisition_recovery
                    .prepare_start()
                    .map(|runtime| (runtime, owner))
            })
            .transpose()
            .map_err(host_error)?;
        let mut publication = Some(publication);
        self.owner
            .publish_gateway_ready(&mut || {
                let publication = publication.take().ok_or(HostLifetimeError::Denied)?;
                publication.publish();
                Ok(())
            })
            .map_err(host_error)?;
        // Start outside publication custody. The existing native gateway
        // selected this boundary after restore. The task uses the existing
        // canonical recovery and native submission owners for each step.
        if let Some((runtime, acquisition)) = recovery {
            self.acquisition_recovery
                .start(runtime, acquisition)
                .map_err(host_error)?;
        }
        Ok(())
    }

    fn begin_close(&self, reason: GatewayHostCloseReason) -> Result<(), GatewayHostError> {
        // The test observation owns no public mob token. Drop its remaining
        // composition references before the normal owner-close/join boundary.
        #[cfg(feature = "test-support")]
        if let Some(observer) = &self.runtime_observer {
            let observed = observer
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take();
            drop(observed);
        }
        self.collector_adapters.begin_close();
        self.member_class_operations.stop();
        self.acquisition_recovery.stop();
        if let Some(source) = &self.private_originals {
            source.stop_custody_recovery();
        }
        match reason {
            GatewayHostCloseReason::HostDisconnected | GatewayHostCloseReason::CallerCancelled => {
                self.owner.host_lost()
            }
            GatewayHostCloseReason::Shutdown | GatewayHostCloseReason::InitializationFailed => {
                self.owner.begin_close_activation(HostCloseReason::Shutdown)
            }
        }
        .map(|_| ())
        .map_err(host_error)
    }

    async fn join_close(&self) -> Result<(), GatewayHostError> {
        self.collector_adapters.begin_close();
        self.member_class_operations.stop();
        // Stop new acquisition steps before native closure, including the
        // initialization-failure path. An entered step retains its own owner
        // and may settle; no task abort turns uncertainty into no effect.
        self.acquisition_recovery.stop();
        if let Some(source) = &self.private_originals {
            source.stop_custody_recovery();
        }
        let closing = self.owner.begin_close_activation(HostCloseReason::Shutdown);
        let native_close = async {
            let closing = closing?;
            self.owner.join_close(&closing.handle).await
        };
        let recovery_close = async {
            // Stop above excludes new transfer registration before this census,
            // even when initialization never started the recovery worker.
            let custody_close = async {
                match &self.private_originals {
                    Some(originals) => originals
                        .join_custody_recovery()
                        .await
                        .map_err(|_| HostLifetimeError::NotSettled),
                    None => Ok(()),
                }
            };
            let transfer_close = async {
                match &self.private_originals {
                    Some(originals) => originals
                        .join_workflow_context_transfers(false)
                        .await
                        .map_err(|_| HostLifetimeError::NotSettled),
                    None => Ok(()),
                }
            };
            // These independently retained owners must all enter their joins,
            // including when another owner fails or does not settle promptly.
            let (recovered, custody, transfers) = tokio::join!(
                self.acquisition_recovery.join(),
                custody_close,
                transfer_close
            );
            recovered.and(custody).and(transfers)
        };
        let (recovery, native, members, collectors) = tokio::join!(
            tokio::time::timeout(std::time::Duration::from_secs(30), recovery_close),
            native_close,
            tokio::time::timeout(
                std::time::Duration::from_secs(30),
                self.member_class_operations.join()
            ),
            tokio::time::timeout(
                std::time::Duration::from_secs(30),
                self.collector_adapters.join()
            ),
        );
        // Drain body-free observations only after these native/retained joins
        // have returned, outside every synchronous policy/publication callback.
        // A failed join is still failed; this snapshot proves no settlement or
        // complete callback census. No buffer lock is held during output.
        if let Some(source) = &self.private_originals {
            let observations = source.review_observations().drain();
            observations.write_to(&mut std::io::stderr().lock());
        }
        // Every collector join above runs even if another owner fails. A
        // timeout leaves the actual adapter/report retained for the next join.
        collectors.map_err(|_| GatewayHostError::SettlementIncomplete)??;
        // A deadline ends observation only. The JoinSet retains any native
        // settlement or entered finalizer for another exact host join.
        members
            .map_err(|_| GatewayHostError::SettlementIncomplete)?
            .map_err(host_error)?;
        let settled = native.map_err(host_error)?;
        // A timed-out join leaves the same task handle retained for a later
        // join_close; native closure was still allowed to complete above.
        recovery
            .map_err(|_| GatewayHostError::SettlementIncomplete)?
            .map_err(host_error)?;
        if settled.state != HostCloseState::Settled {
            return Err(GatewayHostError::SettlementIncomplete);
        }
        Ok(())
    }

    async fn wait_closed(&self) {
        tokio::select! {
            _ = self.owner.wait_closed() => {},
            _ = self.private_closed.wait_closed() => {},
        }
        self.acquisition_recovery.stop();
        self.collector_adapters.begin_close();
        if let Some(source) = &self.private_originals {
            source.stop_custody_recovery();
        }
    }
}
