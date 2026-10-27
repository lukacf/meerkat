//! Concrete Toolkit ownership composed through MobKit's generic gateway hook.
//!
//! Public init fields never select a host, policy owner or registration hook.
//! The application's native registration hook owns destination selection and
//! surface verification. A lifecycle observation is never permission.

mod prepared_application;
mod console_setup;
mod local_recovery;
pub use local_recovery::{LocalRecoveryPageRead, LocalOperatorRecoveryPolicy, LocalServiceRecovery, LocalServiceRecoveryPage, LocalServiceRecoveryResult};
mod prepared_runtime;
pub use prepared_runtime::PreparedGatewayRuntime;
pub use console_setup::{ConsoleAccessPolicy, ConsoleRecoveryPolicy, ConsoleAccessSnapshot, ConsoleServiceRecoverySnapshot, ConsolePageRead, ConsoleSetupBinding, ConsoleStatusPage};
pub use prepared_application::PreparedSetupApplication;

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
        GatewayBootstrapReady, GatewayHostCloseReason, GatewayHostComposition, GatewayHostError,
        PreparedApplicationRequest, PreparedGatewayApplication, GatewayFactoryConstruction,
        GatewayMcpComposition,
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
pub type SetupBootstrapProjection = Arc<dyn Fn(MobBootstrapSpec) -> Result<MobBootstrapSpec, HostLifetimeError> + Send + Sync>;

/// Trusted application construction on the actual native factory offer. This
/// callback creates no factory, machine, policy registry or authorization owner.
/// It runs synchronously under this host's existing current-selection boundary.
pub type SetupMemberFactoryComposition = Arc<dyn Fn(&GatewayFactoryConstruction<'_>, crate::installed_host::acquisition::InstalledServiceTaskSource)
    -> Result<GatewayMcpComposition, HostLifetimeError> + Send + Sync>;

pub struct ToolkitGatewayHost {
    owner: Arc<NativeHostLifetime>,
    private_closed: PrivateHostClosed,
    machine: OnceLock<Arc<MeerkatMachine>>,
    // Outer None means the gateway already took this one-use configuration.
    // Inner None means the legacy constructor has no governed owners.
    grant_configuration: Mutex<Option<Option<NativeGrantWorkConfiguration>>>,
    member_registration: Option<Arc<dyn MemberRuntimeRegistrationHook>>,
    private_originals: Option<Arc<PrivateOriginals>>,
    console_logins: Option<crate::private_host::ConsoleLogins>,
    installed_tool_policy: Option<Arc<meerkat_core::ToolConsequencePolicyRegistry>>,
    acquisition: Arc<Mutex<crate::installed_host::acquisition::AcquisitionBootstrap>>,
    native_continuation: Option<Arc<crate::installed_host::acquisition::InstalledNativeContinuation>>,
    setup_projection: Option<SetupBootstrapProjection>,
    setup_application: Option<PreparedSetupApplication>,
    setup_member_factory: Option<SetupMemberFactoryComposition>,
    console_routes: Option<console_setup::ConsoleRouteComposition>,
    local_recovery: Option<LocalOperatorRecoveryPolicy>,
    // Process-local transport observation only, never an activation or grant.
    prepared_application_selected: OnceLock<()>,
    acquisition_recovery: crate::installed_host::acquisition::recovery::AcquisitionRecovery,
}

impl ToolkitGatewayHost {

    /// Join a trusted configured resource to the existing installed native
    /// owners before bootstrap takes them. The caller composes any existing
    /// attachment provider; the actual registration hook remains authoritative.
    pub fn with_configured_mcp_resource<F>(
        mut self,
        compose: F,
        selected: crate::installed_host::prepared_registration::SelectedDestinationParticipant,
    ) -> Result<Self, HostLifetimeError>
    where
        F: FnOnce(Option<Arc<dyn meerkat_runtime::meerkat_machine::ConfiguredMcpAttachmentProvider>>)
            -> Arc<dyn meerkat_runtime::meerkat_machine::ConfiguredMcpAttachmentProvider>,
    {
        self.owner.validate_current_selection()?;
        if self.private_closed.is_closed() || self.machine.get().is_some() {
            return Err(HostLifetimeError::Denied);
        }
        let registration = self.member_registration.clone().ok_or(HostLifetimeError::Unavailable)?;
        let registry = self.installed_tool_policy.clone().ok_or(HostLifetimeError::Unavailable)?;
        let configuration = self.grant_configuration.get_mut().map_err(|_| HostLifetimeError::Unavailable)?
            .as_mut().and_then(Option::as_mut).ok_or(HostLifetimeError::Unavailable)?;
        configuration.configured_mcp_attachment = Some(compose(configuration.configured_mcp_attachment.take()));
        self.member_registration = Some(Arc::new(
            crate::installed_host::prepared_registration::PreparedMemberRegistration::from_installed(
                registration, registry, selected,
            ),
        ));
        Ok(self)
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
        self.owner.validate_current_selection().map_err(|error| match error {
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
        let captured = prepared.source_owner().ok_or(HostLifetimeError::Unavailable)?;
        let current = self.private_originals.as_ref().ok_or(HostLifetimeError::Unavailable)?;
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
        let captured = prepared.source_owner().ok_or(HostLifetimeError::Unavailable)?;
        let current = self.private_originals.as_ref().ok_or(HostLifetimeError::Unavailable)?;
        if !Arc::ptr_eq(&captured, current) {
            return Err(HostLifetimeError::WrongOwner);
        }
        if self.private_closed.is_closed() { return Err(HostLifetimeError::Denied); }
        self.owner.validate_current_selection()?;
        let helpers = self.acquisition.lock().map_err(|_| HostLifetimeError::Unavailable)?
            .configured().map(|acquisition| acquisition.helper_selections()).unwrap_or_default();
        current.application_operator_setup(self.owner.clone())?.with_helper_selections(helpers)
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
        self.owner.validate_current_selection().map_err(|error| match error {
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
        &self, prepared: &crate::private_originals::LocalControllerSetup,
        parent: Vec<meerkat_authorization_contracts::grant::GrantLineageRef>,
    ) -> Result<crate::private_originals::PrivateSetupRequestSelection, PrivateOriginalError> {
        if self.private_closed.is_closed() { return Err(PrivateOriginalError::Denied); }
        self.owner.validate_current_selection().map_err(|_| PrivateOriginalError::Unavailable)?;
        self.private_originals.as_ref().ok_or(PrivateOriginalError::Unavailable)?
            .select_setup_request(prepared, parent).await
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
        let helpers = self.acquisition.lock().map_err(|_| HostLifetimeError::Unavailable)?
            .configured().map(|acquisition| acquisition.helper_selections()).unwrap_or_default();
        self.private_originals.as_ref().ok_or(HostLifetimeError::Unavailable)?
            .local_operator_setup(self.owner.clone())?.with_helper_selections(helpers)
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
        self.owner.validate_current_selection().map_err(|error| match error {
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
        self.owner.validate_current_selection().map_err(|error| match error {
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
        let mut host = Self::with_grant_configuration(
            owner, private_closed, None, Some(member_registration),
        );
        host.grant_configuration = Mutex::new(None);
        host.private_originals = Some(originals);
        host
    }

    /// Use this gateway's actual private connection login owner. Installed
    /// routes and native verified humans still select every Console original.
    pub fn with_console_logins(mut self, logins: crate::private_host::ConsoleLogins)
        -> Result<Self, HostLifetimeError>
    {
        if self.console_logins.is_some() || self.machine.get().is_some()
            || self.private_originals.is_none()
        { return Err(HostLifetimeError::InvalidSelection); }
        self.console_logins = Some(logins);
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

    /// Actual first-party setup composition, before `prepare_bootstrap`.
    /// The exact source must belong to this host; no independent result store
    /// or new delivery owner can be injected through this convenience seam.
    pub fn with_native_setup_bootstrap(mut self,
        continuation: Arc<crate::installed_host::acquisition::InstalledNativeContinuation>,
        projection: SetupBootstrapProjection,
        application: PreparedSetupApplication,
    ) -> Result<Self, HostLifetimeError> {
        if self.native_continuation.is_some() || self.machine.get().is_some()
            || !self.private_originals.as_ref().is_some_and(|source| continuation.shares_source(source))
        { return Err(HostLifetimeError::InvalidSelection); }
        self.native_continuation = Some(continuation);
        self.setup_projection = Some(projection);
        self.setup_application = Some(application);
        Ok(self)
    }

    /// Retain the selected application's private presenter composition before
    /// native factory construction. Ordinary hosts have no callback. This
    /// requires the already authenticated prepared setup application and cannot
    /// replace a callback or a running host's factory configuration.
    pub fn with_setup_member_factory(mut self, compose: SetupMemberFactoryComposition)
        -> Result<Self, HostLifetimeError>
    {
        if self.setup_application.is_none() || self.setup_member_factory.is_some()
            || self.machine.get().is_some() || self.private_closed.is_closed()
        { return Err(HostLifetimeError::InvalidSelection); }
        self.owner.validate_current_selection()?;
        self.setup_member_factory = Some(compose);
        Ok(self)
    }

    /// Select an application-policy factory on this exact installed lifetime.
    /// Matching serialized configuration cannot substitute another live owner.
    /// The ordinary factory slot/currentness checks remain the sole admission.
    pub fn with_setup_member_factory_for_owner(self, owner: &Arc<NativeHostLifetime>,
        compose: SetupMemberFactoryComposition,
    ) -> Result<Self, HostLifetimeError> {
        if !Arc::ptr_eq(owner, &self.owner) { return Err(HostLifetimeError::WrongOwner); }
        self.with_setup_member_factory(compose)
    }

    /// Bootstrap selection only, derived from the one retained owner state.
    /// Configured does not mean native helper execution or continuation ready.
    pub fn acquisition_bootstrap_status(
        &self,
    ) -> Result<crate::installed_host::acquisition::AcquisitionBootstrapStatus, GatewayHostError> {
        self.acquisition.lock().map(|selection| selection.status())
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
            grant_configuration: Mutex::new(Some(configuration)),
            member_registration,
            private_originals: None,
            console_logins: None,
            installed_tool_policy: None,
            acquisition: Arc::new(Mutex::new(Default::default())),
            native_continuation: None,
            setup_projection: None,
            setup_application: None,
            setup_member_factory: None,
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
    fn host_app_routes(&self, viewer: meerkat_mobkit::http_console::ConsoleViewerResolver)
        -> Result<Option<axum::Router>, GatewayHostError>
    {
        self.owner.with_current_setup(|| {
            if self.private_closed.is_closed() { return Err(HostLifetimeError::Denied); }
            self.console_routes.as_ref().map(|routes| routes(viewer)).transpose()
        }).map_err(host_error)
    }
    fn compose_member_factory(
        &self,
        construction: &GatewayFactoryConstruction<'_>,
    ) -> Result<Option<GatewayMcpComposition>, GatewayHostError> {
        let Some(compose) = &self.setup_member_factory else { return Ok(None); };
        self.owner.with_current_setup(|| {
            if self.private_closed.is_closed() || self.prepared_application_selected.get().is_none() {
                return Err(HostLifetimeError::Denied);
            }
            let native = self.owner.selected_policy().native_host.as_ref()
                .ok_or(HostLifetimeError::InvalidSelection)?;
            // The factory's installation realm is separate from the member
            // realm owned by this policy and checked at adapter construction.
            if native.members.is_empty() || native.members.iter().any(|member|
                meerkat_core::mob_realm_id(&member.member.mob_id).as_ref().ok() != Some(&native.realm)) {
                return Err(HostLifetimeError::WrongOwner);
            }
            let composition = compose(construction, crate::installed_host::acquisition::InstalledServiceTaskSource::new(&self.acquisition))?;
            if self.private_closed.is_closed() { return Err(HostLifetimeError::Denied); }
            Ok(Some(composition))
        }).map_err(host_error)
    }

    async fn prepared_application(
        &self,
        request: Option<&PreparedApplicationRequest>,
    ) -> Result<Option<PreparedGatewayApplication>, GatewayHostError> {
        let Some(application) = &self.setup_application else { return Ok(None); };
        self.owner.with_current_setup(|| {
            if self.private_closed.is_closed() { return Err(HostLifetimeError::Denied); }
            // Every init must carry this selected application. An earlier
            // successful selection cannot authenticate a later absent carrier.
            let request = request.ok_or(HostLifetimeError::Denied)?;
            let selected = application.select(request, self.owner.clone(), self.private_closed.clone())?;
            if self.private_closed.is_closed() { return Err(HostLifetimeError::Denied); }
            self.prepared_application_selected.get_or_init(|| ());
            Ok(Some(selected))
        }).map_err(host_error)
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
        }
        let registration = self
            .member_registration
            .as_ref()
            .ok_or(GatewayHostError::Unavailable)?;
        let spec = spec.with_member_runtime_registration_hook(registration.clone());
        // The same fully composed spec supplies both the actual selected
        // WorkGraph and its original provider-open census, before any inputs.
        // No setter, replacement service or late bind_runtime lookup is used.
        let acquisition = self.acquisition.lock()
            .map_err(|_| GatewayHostError::Unavailable)?
            .prepare(spec.workgraph_service(), spec.resolved_storage.as_ref())
            .map_err(host_error)?;
        self.owner.validate_current_selection().map_err(host_error)?;
        let spec = match acquisition {
            // The native registration owns the RegisteredToolBundleOperation
            // witness. Disabled acquisition has no driver or registration.
            Some(acquisition) => {
                let spec = if let Some(continuation) = &self.native_continuation {
                    let projection = self.setup_projection.as_ref().ok_or(GatewayHostError::Unavailable)?;
                    let spec = projection(spec).map_err(host_error)?;
                    // Core bca71's actual source constructor retains this
                    // exact adapter and namespace. Meta 2835 supplies the
                    // reviewed forwarding and final rebuild/rebind path. Governed
                    // Host completion, exact-content reconciliation and matched
                    // tuple execution remain separate qualification requirements.
                    let source = meerkat_mob_mcp::HostJobSource::new(
                        [continuation.namespace().to_owned()], continuation.clone(),
                    ).map_err(|_| GatewayHostError::Denied)?;
                    spec.with_host_job_source(source).map_err(|_| GatewayHostError::Unavailable)?
                } else { spec };
                spec.register_tool_bundle(crate::FIND_AND_INSTALL_BUNDLE, acquisition.tool())
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
        if self.acquisition_bootstrap_status()? == crate::installed_host::acquisition::AcquisitionBootstrapStatus::Configured {
            if let Some(continuation) = &self.native_continuation {
                continuation.bind_native(runtime.mob_runtime()).map_err(host_error)?;
            }
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
                access.join_authorization_publication(originals.authorization_publication())
                    .map_err(|_| GatewayHostError::Denied)?;
            }
            originals
                .bind_identity_runtime(&identities)
                .map_err(|error| match error {
                    PrivateOriginalError::Unavailable => GatewayHostError::Unavailable,
                    _ => GatewayHostError::Denied,
                })?;
            originals.start_custody_recovery(runtime.mob_runtime().continuation_source_announcer())
                .map_err(|_| GatewayHostError::Unavailable)?;
            if let Some(pins) = runtime.mob_runtime().controller_pin_source() {
                originals.bind_controller_pin_source(pins).map_err(|_| GatewayHostError::Denied)?;
            }
            if let Some(logins) = &self.console_logins {
                let ingress = originals.console_ingress(logins.clone(), runtime.access_controller().cloned())
                    .await.map_err(|error| match error {
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
        let acquisition = self.acquisition.lock()
            .map_err(|_| GatewayHostError::Unavailable)?.configured();
        // bind_runtime already retained the actual native service. Prepare the
        // task handle now, but enter no recovery work before ready publication.
        // A service Arc by itself is not binding or restore readiness.
        let recovery = acquisition.map(|owner| {
            self.acquisition_recovery.prepare_start().map(|runtime| (runtime, owner))
        }).transpose().map_err(host_error)?;
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
            self.acquisition_recovery.start(runtime, acquisition).map_err(host_error)?;
        }
        Ok(())
    }

    fn begin_close(&self, reason: GatewayHostCloseReason) -> Result<(), GatewayHostError> {
        self.acquisition_recovery.stop();
        if let Some(source) = &self.private_originals { source.stop_custody_recovery(); }
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
        // Stop new acquisition steps before native closure, including the
        // initialization-failure path. An entered step retains its own owner
        // and may settle; no task abort turns uncertainty into no effect.
        self.acquisition_recovery.stop();
        if let Some(source) = &self.private_originals { source.stop_custody_recovery(); }
        let closing = self.owner.begin_close_activation(HostCloseReason::Shutdown);
        let native_close = async {
            let closing = closing?;
            self.owner.join_close(&closing.handle).await
        };
        let recovery_close = async {
            // Stop above excludes new recovery work before this census,
            // even when initialization never started the recovery worker.
            let custody_close = async {
                match &self.private_originals {
                    Some(originals) => originals.join_custody_recovery().await
                        .map_err(|_| HostLifetimeError::NotSettled),
                    None => Ok(()),
                }
            };
            // These independently retained owners must all enter their joins,
            // including when another owner fails or does not settle promptly.
            let (recovered, custody) = tokio::join!(
                self.acquisition_recovery.join(), custody_close);
            recovered.and(custody)
        };
        let (recovery, native) = tokio::join!(
            tokio::time::timeout(std::time::Duration::from_secs(30), recovery_close),
            native_close,
        );
        let settled = native.map_err(host_error)?;
        // A timed-out join leaves the same task handle retained for a later
        // join_close; native closure was still allowed to complete above.
        recovery.map_err(|_| GatewayHostError::SettlementIncomplete)?
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
        if let Some(source) = &self.private_originals { source.stop_custody_recovery(); }
    }
}
