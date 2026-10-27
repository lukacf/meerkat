//! Shared executable composition from the selected application bundle.
//!
//! The inherited parent supplies selectors and authenticated source events.
//! This adapter opens native stores and uses native policy/registration APIs;
//! public RPC cannot supply owners. Tool grants, installed-surface evidence,
//! source authentication and account policy remain distinct conjuncts.

pub mod acquisition;
pub(crate) mod application_operations;
#[cfg(test)]
#[path = "installed_host/application_operations_tests.rs"]
mod application_operations_tests;
#[cfg(test)]
pub(crate) mod controller_first_input_fixture;
pub mod operator_setup;
pub mod prepared_registration;
pub mod preparation;
pub use preparation::{PreparedInstallationOwner, PreparedInstalledHost};
mod preparation_policy;
pub use preparation_policy::PreparedApplicationPolicy;
pub mod tool_policy;

use crate::{
    host_gateway::ToolkitGatewayHost,
    original_ingress::ToolkitOriginalOwner,
    private_host::PrivateHostClosed,
    private_originals::{PrivateOriginalRoute, PrivateOriginals},
};
use async_trait::async_trait;
use meerkat_authorization::{
    clock::HostAuthorizationClock,
    grant_policy::{
        AdmittedWorkPolicyOwner, ControllerAdmissionAllowance, OperationPolicyOwner,
        WorkOwnerAllowance,
    },
    grants::{FileGrantOwnerStore, LocalGrantConfiguration},
    policy::{LocalPolicyAllowance, LocalPolicyPurpose},
};
use meerkat_authorization_contracts::{
    constraints::{ExecutionRestrictions, ResourceDomain},
    work_association::{InputAuthorityAssociation, WorkAuthorityBasis},
};
use meerkat_core::{
    OperationAuthorizationError, OperationRefusalKind, OperationRefused, ToolIdentity,
    authorization::{
        AuthorizationOperation, ModelAuthorizationUse, OperationReviewTier,
        PreparedAuthorizationBinding, SourceAuthorizationTarget, SourceAuthorizationUse,
    },
};
use meerkat_mob::{
    MemberDestinationSelection, MemberRuntimeBuildIdentity, MemberRuntimeRegistrationHook, MobError,
};
use tool_policy::{PreparedInstalledToolPolicy, ValidatedInstalledToolPolicy};
use meerkat_runtime::{
    input_authority::{NativeAdmissionError, NativeIngressContext},
    meerkat_machine::{RetainedInvocationOwner, ScopedInvocation, ScopedMandateOwner},
};
use meerkat_toolkit::host_lifetime::{
    NativeHostLifetime,
    control::{HostBootstrapSelection, HostLifetimeError},
    policy::{CompiledArtifactReference, CompiledNativeHost, CompiledNativeMember},
};
use sha2::{Digest, Sha256};
use std::{
    fs,
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::Arc,
};
use toolkit_package::canonical_json;
use toolkit_store::{SqliteStore, StoreScope};

/// Ordinary applications restore the operator-provisioned owner pair. The
/// legacy Initialize choice is explicitly refused; operator setup is a separate
/// OS-authenticated entry, never a restore-error fallback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeOwnerStartup {
    Restore,
    Initialize,
}

pub struct InstalledToolkitHost {
    originals: Arc<PrivateOriginals>,
    store: Arc<SqliteStore>,
    source_scope: StoreScope,
    acquisition: acquisition::AcquisitionBootstrap,
    native: CompiledNativeHost,
    tool_policy: PreparedInstalledToolPolicy,
    surfaces: Vec<Vec<ToolIdentity>>,
    update_associations: Option<Arc<dyn InstalledSurfaceAssociations>>,
}

/// Fixed process-source custody for actual native prepare and visibility
/// observations. Implementations must never treat these as finish or adoption.
pub trait InstalledSurfaceAssociations: Send + Sync {
    fn prepared(&self, identity: &MemberRuntimeBuildIdentity, surface: &meerkat_core::PreparedToolSurface) -> Result<(), HostLifetimeError>;
    fn visible(&self, identity: &MemberRuntimeBuildIdentity, surface: &[meerkat_core::ToolDef]) -> Result<(), HostLifetimeError>;
}

/// Read-only result of the actual executable artifact validators. It carries
/// installed surface/policy construction, never an invocation grant or an owner.
pub struct ValidatedInstalledArtifacts {
    tool_policy: ValidatedInstalledToolPolicy,
    surfaces: Vec<Vec<ToolIdentity>>,
}

/// Shared by immutable bundle preparation, first-use provisioning and ordinary
/// restore. The reader supplies exact bytes; this same validator checks their
/// identity, canonical native policy/member content and initial tool surfaces.
/// No grant/source store, policy evaluation worker or publication is opened here.
pub fn validate_installed_artifacts(
    native: &CompiledNativeHost,
    mut read: impl FnMut(&CompiledArtifactReference) -> Result<Vec<u8>, HostLifetimeError>,
) -> Result<ValidatedInstalledArtifacts, HostLifetimeError> {
    let mut checked = |reference: &CompiledArtifactReference| {
        reference.validate()?;
        let bytes = read(reference)?;
        if digest(&bytes) != reference.sha256 {
            return Err(HostLifetimeError::InvalidSelection);
        }
        Ok(bytes)
    };
    if let Some(setup) = &native.setup { let _ = checked(setup)?; }
    let tool_bytes = checked(&native.application_tool_policy)?;
    let tool_policy = ValidatedInstalledToolPolicy::parse(&tool_bytes)?;
    let mut surfaces = Vec::new();
    for member in &native.members {
        if !tool_policy.contains_member(&member.member) {
            return Err(HostLifetimeError::InvalidSelection);
        }
        let bytes = checked(&member.initial_surface)?;
        let surface: Vec<ToolIdentity> =
            serde_json::from_slice(&bytes).map_err(|_| HostLifetimeError::InvalidSelection)?;
        if canonical_json(&surface).map_err(|_| HostLifetimeError::InvalidSelection)? != bytes
            || surface.iter().enumerate().any(|(index, entry)| {
                surface[..index]
                    .iter()
                    .any(|prior| prior.name == entry.name)
            })
        {
            return Err(HostLifetimeError::InvalidSelection);
        }
        surfaces.push(surface);
    }
    Ok(ValidatedInstalledArtifacts {
        tool_policy,
        surfaces,
    })
}

/// Validate an application-owned candidate bundle without selecting, rewriting
/// or opening it. Ordinary runtime bootstrap still requires exact current.
pub fn validate_application_candidate(
    selection: &HostBootstrapSelection,
) -> Result<(), HostLifetimeError> {
    let (policy, _) =
        meerkat_toolkit::host_lifetime::policy::validate_candidate_selection(selection)?;
    let native = policy
        .native_host
        .as_ref()
        .ok_or(HostLifetimeError::Unavailable)?;
    let _validated = validate_installed_artifacts(native, |reference| {
        read_bundle_artifact(selection, reference)
    })?;
    Ok(())
}

impl InstalledToolkitHost {
    /// Enroll the existing installed product's read-only package verifier.
    /// Public/private command documents cannot select or replace this owner.
    pub fn with_service_installation_readback(
        self, owner: Arc<dyn crate::private_originals::ServiceInstallationReadback>,
    ) -> Result<Self, HostLifetimeError> {
        self.originals.bind_service_installation_readback(owner)
            .map_err(|_| HostLifetimeError::InvalidSelection)?;
        Ok(self)
    }

    /// Called inside the existing private bootstrap factory.
    /// All artifacts are checked before restoring the existing native owner pair.
    pub async fn open(
        selection: HostBootstrapSelection,
        startup: NativeOwnerStartup,
    ) -> Result<(NativeHostLifetime, Self), HostLifetimeError> {
        Self::open_selected(selection, startup, None, None, #[cfg(test)] None).await
    }

    /// Adopt the policy used by this process's completed native preparations.
    /// The selected bundle is authenticated and every artifact is validated
    /// before native stores open. Only an exact canonical policy match retains
    /// this existing registry; no policy worker or replacement registry starts.
    /// This does not finish native preparations or activate their members.
    pub async fn open_with_prepared_tool_policy(
        selection: HostBootstrapSelection,
        startup: NativeOwnerStartup,
        prepared_tool_policy: PreparedInstalledToolPolicy,
    ) -> Result<(NativeHostLifetime, Self), HostLifetimeError> {
        Self::open_selected(selection, startup, Some(prepared_tool_policy), None, #[cfg(test)] None).await
    }

    pub async fn open_with_application_service_policy(
        selection: HostBootstrapSelection, startup: NativeOwnerStartup,
        prepared_tool_policy: Option<PreparedInstalledToolPolicy>,
        participant: Arc<dyn crate::private_originals::ApplicationServicePolicy>,
    ) -> Result<(NativeHostLifetime, Self), HostLifetimeError> {
        Self::open_selected(selection, startup, prepared_tool_policy, Some(participant), #[cfg(test)] None).await
    }

    /// Test fixtures select a storage observation wrapper before the first
    /// source adapter claims its canonical store. No live binding is replaced.
    #[cfg(test)]
    async fn open_with_source_store(
        selection: HostBootstrapSelection,
        startup: NativeOwnerStartup,
        participant: Option<Arc<dyn crate::private_originals::ApplicationServicePolicy>>,
        prepare_source: controller_first_input_fixture::PrepareOriginalSource,
    ) -> Result<(NativeHostLifetime, Self), HostLifetimeError> {
        Self::open_selected(selection, startup, None, participant, Some(prepare_source)).await
    }

    async fn open_selected(
        selection: HostBootstrapSelection,
        startup: NativeOwnerStartup,
        prepared_tool_policy: Option<PreparedInstalledToolPolicy>,
        participant: Option<Arc<dyn crate::private_originals::ApplicationServicePolicy>>,
        #[cfg(test)] prepare_source: Option<controller_first_input_fixture::PrepareOriginalSource>,
    ) -> Result<(NativeHostLifetime, Self), HostLifetimeError> {
        if startup != NativeOwnerStartup::Restore {
            // Ordinary application bootstrap cannot initialize an operator root.
            return Err(HostLifetimeError::Denied);
        }
        let mut owner = NativeHostLifetime::bootstrap(selection.clone())?;
        let native = owner
            .selected_policy()
            .native_host
            .clone()
            .ok_or(HostLifetimeError::Unavailable)?;
        let ValidatedInstalledArtifacts {
            tool_policy,
            surfaces,
        } = validate_installed_artifacts(&native, |reference| {
            read_bundle_artifact(&selection, reference)
        })?;
        let tool_policy = match prepared_tool_policy {
            Some(prepared) => tool_policy.adopt(prepared)?,
            None => tool_policy.prepare()?,
        };
        let directory = state_directory(&selection, &native, startup)?;
        // Use the same intact canonical pair contract as stopped preparation.
        // Its native lease is acquired before the published source store opens.
        let (source, store) = operator_setup::open_owner_pair_in(
            &directory, &native.ingress_domain, native.remote_operator.as_ref(),
            operator_setup::OperatorOwnerStartup::Restore, true,
        )?;
        let store = Arc::new(store);
        let routes = native
            .routes
            .iter()
            .map(|route| {
                let (target, requester, actor) = route.source_binding(owner.selected_policy())?;
                let member = native
                    .members
                    .iter()
                    .find(|member| &member.member == target)
                    .ok_or(HostLifetimeError::InvalidSelection)?;
                Ok(PrivateOriginalRoute {
                    id: route.id.clone(),
                    grant_caller: native.grant_administrator.clone(),
                    application_parent: route.application_parent.clone(),
                    controller_parent: route.controller_parent.clone(),
                    grant_target: toolkit_package::Id::new(target.member.clone())
                        .map_err(|_| HostLifetimeError::InvalidSelection)?,
                    grant_audience: member.audience.clone(),
                    realm: native.realm.clone(),
                    requester: requester.clone(),
                    ingress_actor: actor.clone(),
                    logical_executor: route.logical_executor.clone(),
                    represented_subject: route.represented_subject.clone(),
                    source_authority: route.source_authority.clone(),
                    authentication_domain: route.authentication_domain.clone(),
                    event_domain: route.event_domain.clone(),
                })
            })
            .collect::<Result<Vec<_>, HostLifetimeError>>()?;
        let source_scope = StoreScope {
            realm: native.realm.to_string(),
            owner: owner.selected_policy().deployment.to_string(),
        };
        let source_store: Arc<dyn toolkit_store::ToolkitStore> = store.clone();
        #[cfg(test)]
        let source_store = match prepare_source {
            Some(prepare_source) => prepare_source(source_store),
            None => source_store,
        };
        let originals = Arc::new(
            PrivateOriginals::new(source_scope.clone(), source_store, source.clone(), routes)
                .await
                .map_err(|_| HostLifetimeError::InvalidSelection)?,
        );
        originals.select_installed_pair_recovery()?;
        let service_policy = tool_policy.bind_service_owner(store.clone(), source_scope.clone(),
            native.grant_administrator.clone(), participant).await?;
        originals.bind_service_tool_policy(service_policy)?;
        owner.bind_native_authorization_publication(source.publication())?;
        owner.validate_current_selection()?;
        Ok((
            owner,
            Self {
                originals,
                store,
                source_scope,
                acquisition: acquisition::AcquisitionBootstrap::default(),
                native,
                tool_policy,
                surfaces,
                update_associations: None,
            },
        ))
    }

    /// Selected only by a trusted replacement startup after complete input
    /// preflight. The original owner, resources, routes, destinations and native
    /// mandate remain intact; this process has one immutable source selection.
    pub fn with_application_update(
        mut self, prior_policy: &[u8], candidate_policy: &[u8],
        associations: Arc<dyn InstalledSurfaceAssociations>,
    ) -> Result<Self, HostLifetimeError> {
        if self.update_associations.is_some() { return Err(HostLifetimeError::InvalidSelection); }
        self.tool_policy = self.tool_policy.successor(prior_policy, candidate_policy)?;
        self.update_associations = Some(associations);
        Ok(self)
    }

    pub fn originals(&self) -> Arc<PrivateOriginals> {
        self.originals.clone()
    }

    /// Explicit same-host opt-in through the existing trusted Rust factory.
    /// Console composition supplies its concrete owners; public init/tool JSON
    /// cannot select them. Native helper/operation cutover still gates dispatch.
    pub fn with_acquisition(
        self,
        owners: acquisition::InstalledAcquisitionOwners,
    ) -> Result<Self, HostLifetimeError> {
        self.with_acquisition_helpers(owners, vec![])
    }

    /// Additional native helper inventory for the same acquisition owner. The
    /// current route capability selects a member; this supplies no route map.
    pub fn with_acquisition_helpers(
        mut self,
        owners: acquisition::InstalledAcquisitionOwners,
        additional_helpers: Vec<acquisition::InstalledSetupSelection>,
    ) -> Result<Self, HostLifetimeError> {
        if !self.acquisition.is_unconfigured() {
            return Err(HostLifetimeError::InvalidSelection);
        }
        let acquisition = Arc::new(acquisition::InstalledAcquisition::new(
            self.store.clone(),
            self.source_scope.clone(),
            self.originals.clone(),
            &self.native,
            owners,
            additional_helpers,
        )?);
        acquisition.bind_tool_driver()?;
        self.acquisition = acquisition::AcquisitionBootstrap::selected(acquisition);
        Ok(self)
    }

    /// Standard installed composition: derive admission and input preparation
    /// from this exact source/store/policy. Only the existing continuation owner,
    /// selected native helper, proposal bounds and WorkGraph backend are supplied.
    pub fn with_default_acquisition(
        self,
        defaults: acquisition::InstalledSetupDefaults,
    ) -> Result<Self, HostLifetimeError> {
        self.with_default_acquisition_helpers(defaults, vec![])
    }

    /// Derive the ordinary owner with additional exact native helpers. Omission
    /// through with_default_acquisition retains the existing singleton path.
    pub fn with_default_acquisition_helpers(
        mut self,
        defaults: acquisition::InstalledSetupDefaults,
        additional_helpers: Vec<acquisition::InstalledSetupSelection>,
    ) -> Result<Self, HostLifetimeError> {
        if !self.acquisition.is_unconfigured() { return Err(HostLifetimeError::InvalidSelection); }
        let acquisition = Arc::new(acquisition::InstalledAcquisition::from_defaults(
            self.store.clone(), self.source_scope.clone(), self.originals.clone(), &self.native, defaults, additional_helpers,
        )?);
        acquisition.bind_tool_driver()?;
        self.acquisition = acquisition::AcquisitionBootstrap::selected(acquisition);
        Ok(self)
    }

    /// Build the Host result source over the SAME installed canonical store.
    /// The caller must install its native source hook before first input and
    /// pass the resulting exact service back during runtime binding.
    pub fn native_continuation(
        &self,
        audience: std::collections::BTreeSet<toolkit_package::Id>,
    ) -> Result<Arc<acquisition::InstalledNativeContinuation>, HostLifetimeError> {
        acquisition::InstalledNativeContinuation::new(self.store.clone(), self.source_scope.clone(),
            self.originals.clone(), toolkit_package::Id::new("toolkit.setup")
                .map_err(|_| HostLifetimeError::InvalidSelection)?, audience)
    }

    /// Opt in before the native spec exists. The same application retains the
    /// WorkGraph backend generation and original-task owner; public init and
    /// model arguments cannot supply them. The actual selected service and its
    /// unchanged declaration are consumed once in prepare_bootstrap, before
    /// restored inputs. Unsupported durability disables only this capability.
    pub fn with_bootstrap_acquisition(
        self,
        defaults: acquisition::InstalledSetupBootstrap,
    ) -> Result<Self, HostLifetimeError> {
        self.with_bootstrap_acquisition_helpers(defaults, vec![])
    }

    /// Live SDK opt-in to more than one selected helper before native bootstrap.
    /// Each route still captures its one helper before immutable admission.
    pub fn with_bootstrap_acquisition_helpers(
        self,
        defaults: acquisition::InstalledSetupBootstrap,
        additional_helpers: Vec<acquisition::InstalledSetupSelection>,
    ) -> Result<Self, HostLifetimeError> {
        let tools = acquisition::InstalledAcquisitionTools::new(
            std::time::Duration::from_millis(defaults.limits.admission_timeout_ms),
        )?;
        self.with_enrolled_bootstrap_acquisition_helpers(defaults, tools, additional_helpers)
    }

    /// Adopt the dispatcher objects already used by this application's native
    /// preparation. Stores still come only from this authenticated final host.
    /// The same leaves are bound during the existing bootstrap service selection.
    pub fn with_enrolled_bootstrap_acquisition(
        self,
        defaults: acquisition::InstalledSetupBootstrap,
        tools: acquisition::InstalledAcquisitionTools,
    ) -> Result<Self, HostLifetimeError> {
        self.with_enrolled_bootstrap_acquisition_helpers(defaults, tools, vec![])
    }

    /// Use the same already prepared leaves and private helper inventory. No
    /// additional runtime, native adapter or authority is created here.
    pub fn with_enrolled_bootstrap_acquisition_helpers(
        mut self,
        defaults: acquisition::InstalledSetupBootstrap,
        tools: acquisition::InstalledAcquisitionTools,
        additional_helpers: Vec<acquisition::InstalledSetupSelection>,
    ) -> Result<Self, HostLifetimeError> {
        if !self.acquisition.is_unconfigured() {
            return Err(HostLifetimeError::InvalidSelection);
        }
        self.acquisition = acquisition::AcquisitionBootstrap::deferred_with_tools(
            self.store.clone(), self.source_scope.clone(), self.originals.clone(),
            self.native.clone(), defaults, tools, additional_helpers,
        )?;
        Ok(self)
    }


    pub fn compose(
        mut self,
        owner: Arc<NativeHostLifetime>,
        closed: PrivateHostClosed,
    ) -> ToolkitGatewayHost {
        let registration = self.final_registration(owner.clone());
        let policy = Arc::new(self.final_policy(owner.clone()));
        self.compose_selected(owner, closed, registration, policy)
    }

    /// Bind the already retained native registration participant to this exact
    /// authenticated installed owner. The driver has finished every held
    /// construction and validated its actual selection before entering here.
    /// The same prepared policy must have been adopted when opening this host.
    pub fn compose_with_prepared_registration(
        mut self,
        owner: Arc<NativeHostLifetime>,
        closed: PrivateHostClosed,
        registration: Arc<prepared_registration::PreparedMemberRegistration>,
        policy: Arc<PreparedApplicationPolicy>,
    ) -> Result<ToolkitGatewayHost, HostLifetimeError> {
        owner.validate_current_selection()?;
        if owner.selected_policy().native_host.as_ref() != Some(&self.native)
            || owner.selected_policy().deployment.as_str() != self.source_scope.owner.as_str()
        {
            return Err(HostLifetimeError::InvalidSelection);
        }
        registration.adopt(self.final_registration(owner.clone()), &self.tool_policy.registry())?;
        policy.adopt(self.final_policy(owner.clone()))?;
        Ok(ToolkitGatewayHost::with_prepared_private_originals(
            owner, closed, self.originals, registration,
        ).with_installed_tool_policy(self.tool_policy.registry())
            .with_installed_acquisition(self.acquisition))
    }

    fn final_registration(
        &mut self,
        owner: Arc<NativeHostLifetime>,
    ) -> Arc<dyn MemberRuntimeRegistrationHook> {
        Arc::new(InstalledMemberRegistration {
            owner,
            native: self.native.clone(),
            surfaces: std::mem::take(&mut self.surfaces),
            tool_policy: self.tool_policy.clone(),
            update_associations: self.update_associations.clone(),
        })
    }

    fn final_policy(&self, owner: Arc<NativeHostLifetime>) -> InstalledApplicationPolicy {
        InstalledApplicationPolicy {
            originals: self.originals.clone(),
            lifetime: owner.clone(),
            native: self.native.clone(),
            selected: owner.selected_policy().clone(),
        }
    }

    fn compose_selected<P>(
        self,
        owner: Arc<NativeHostLifetime>,
        closed: PrivateHostClosed,
        registration: Arc<dyn MemberRuntimeRegistrationHook>,
        policy: Arc<P>,
    ) -> ToolkitGatewayHost
    where P: ScopedMandateOwner + AdmittedWorkPolicyOwner + OperationPolicyOwner
        + RetainedInvocationOwner + 'static,
    {
        ToolkitGatewayHost::with_private_originals(
            owner,
            closed,
            self.originals,
            policy,
            registration,
        )
        .with_installed_tool_policy(self.tool_policy.registry())
        .with_installed_acquisition(self.acquisition)
    }
}

/// Read exact manifest-pinned bytes selected by the authenticated application
/// bundle. This is an artifact consistency read, not activation or permission.
pub fn read_bundle_artifact(
    selection: &HostBootstrapSelection,
    reference: &CompiledArtifactReference,
) -> Result<Vec<u8>, HostLifetimeError> {
    reference.validate()?;
    let bundle = selection
        .policy_manifest
        .path
        .parent()
        .ok_or(HostLifetimeError::InvalidSelection)?;
    let manifest_bytes =
        fs::read(&selection.policy_manifest.path).map_err(|_| HostLifetimeError::Unavailable)?;
    if digest(&manifest_bytes) != selection.policy_manifest.sha256 {
        return Err(HostLifetimeError::InvalidSelection);
    }
    let manifest: serde_json::Value =
        serde_json::from_slice(&manifest_bytes).map_err(|_| HostLifetimeError::InvalidSelection)?;
    let rows = manifest
        .get("artifacts")
        .and_then(|value| value.as_array())
        .ok_or(HostLifetimeError::InvalidSelection)?;
    let matches: Vec<_> = rows
        .iter()
        .filter(|row| {
            row.get("path").and_then(|value| value.as_str()) == Some(reference.name.as_str())
        })
        .collect();
    if matches.len() != 1
        || matches[0].get("sha256").and_then(|value| value.as_str())
            != Some(reference.sha256.as_str())
    {
        return Err(HostLifetimeError::InvalidSelection);
    }
    let path = bundle.join(&reference.name);
    if path.is_symlink() || !path.is_file() {
        return Err(HostLifetimeError::InvalidSelection);
    }
    let bytes = fs::read(path).map_err(|_| HostLifetimeError::Unavailable)?;
    if digest(&bytes) != reference.sha256 {
        return Err(HostLifetimeError::InvalidSelection);
    }
    Ok(bytes)
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn private_directory(path: &Path, create: bool) -> Result<(), HostLifetimeError> {
    if create {
        match fs::DirBuilder::new().mode(0o700).create(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return Err(HostLifetimeError::Unavailable),
        }
    }
    let metadata = fs::symlink_metadata(path).map_err(|_| HostLifetimeError::Unavailable)?;
    if !metadata.is_dir() || metadata.permissions().mode() & 0o077 != 0 {
        return Err(HostLifetimeError::InvalidSelection);
    }
    Ok(())
}
fn state_directory(
    selection: &HostBootstrapSelection,
    native: &CompiledNativeHost,
    startup: NativeOwnerStartup,
) -> Result<PathBuf, HostLifetimeError> {
    state_directory_in(&selection.deployment_root, &native.realm, startup)
}

fn state_directory_in(
    deployment_root: &Path,
    realm: &meerkat_core::connection::RealmId,
    startup: NativeOwnerStartup,
) -> Result<PathBuf, HostLifetimeError> {
    let root = deployment_root
        .canonicalize()
        .map_err(|_| HostLifetimeError::Unavailable)?;
    let data = root
        .join("shared/data")
        .canonicalize()
        .map_err(|_| HostLifetimeError::Unavailable)?;
    if !data.starts_with(&root) {
        return Err(HostLifetimeError::InvalidSelection);
    }
    let parent = data.join("toolkit-native");
    let create = startup == NativeOwnerStartup::Initialize;
    private_directory(&parent, create)?;
    let directory = parent.join(digest(realm.to_string().as_bytes()));
    private_directory(&directory, create)?;
    Ok(directory)
}

struct InstalledMemberRegistration {
    owner: Arc<NativeHostLifetime>,
    native: CompiledNativeHost,
    surfaces: Vec<Vec<ToolIdentity>>,
    tool_policy: PreparedInstalledToolPolicy,
    update_associations: Option<Arc<dyn InstalledSurfaceAssociations>>,
}
impl InstalledMemberRegistration {
    fn member(
        &self,
        identity: &MemberRuntimeBuildIdentity,
    ) -> Result<(usize, &CompiledNativeMember), MobError> {
        self.owner.validate_current_selection().map_err(|_| {
            MobError::WiringError("selected Toolkit activation is unavailable".into())
        })?;
        if identity.realm_id() != &self.native.realm {
            return Err(MobError::WiringError("member realm is not selected".into()));
        }
        self.native
            .members
            .iter()
            .enumerate()
            .find(|(_, row)| &row.member == identity.member())
            .ok_or_else(|| {
                MobError::WiringError("member has no installed destination policy".into())
            })
    }
    fn destination(
        &self,
        identity: &MemberRuntimeBuildIdentity,
    ) -> Result<MemberDestinationSelection, MobError> {
        let (_, member) = self.member(identity)?;
        Ok(MemberDestinationSelection {
            logical_owner: member.logical_owner.clone(),
            context: identity.session_document().clone(),
            context_generation: member.context_generation,
            audience: member.audience.clone(),
        })
    }
}
#[async_trait]
impl MemberRuntimeRegistrationHook for InstalledMemberRegistration {
    async fn select_destination(
        &self,
        identity: &MemberRuntimeBuildIdentity,
    ) -> Result<MemberDestinationSelection, MobError> {
        self.destination(identity)
    }
    async fn expected_destination(
        &self,
        identity: &MemberRuntimeBuildIdentity,
    ) -> Result<MemberDestinationSelection, MobError> {
        self.destination(identity)
    }
    async fn prepare_member_surface(
        &self,
        identity: &MemberRuntimeBuildIdentity,
        surface: &meerkat_core::PreparedToolSurface,
    ) -> Result<meerkat_core::ToolSurfaceSelection, MobError> {
        let (index, member) = self.member(identity)?;
        match &self.update_associations {
            Some(associations) => associations.prepared(identity, surface),
            None => tool_policy::validate_installed_surface(surface, &self.surfaces[index]),
        }.map_err(|_| MobError::WiringError(
            "prepared tool has no selected installed-artifact association".into(),
        ))?;
        self.tool_policy.select(&member.member, surface).map_err(|_| {
            MobError::WiringError("member has no selected prepared application tool policy".into())
        })
    }
    async fn verify_member_surface(
        &self,
        identity: &MemberRuntimeBuildIdentity,
        surface: &[meerkat_core::ToolDef],
    ) -> Result<(), MobError> {
        let (index, _) = self.member(identity)?;
        if let Some(associations) = &self.update_associations {
            return associations.visible(identity, surface).map_err(|_| MobError::WiringError(
                "visible tool has no selected update preparation association".into(),
            ));
        }
        let installed = &self.surfaces[index];
        if surface
            .iter()
            .any(|tool| !installed.contains(&tool.identity()))
        {
            return Err(MobError::WiringError(
                "visible tool has no selected installed-artifact association".into(),
            ));
        }
        // This is an initial-surface check. It does not grant invocation or
        // demand that lazy/catalog tools already be visible. Native attachment
        // and operation owners govern later publication and invocation.
        Ok(())
    }
}

struct InstalledApplicationPolicy {
    originals: Arc<PrivateOriginals>,
    lifetime: Arc<NativeHostLifetime>,
    native: CompiledNativeHost,
    selected: meerkat_toolkit::host_lifetime::policy::CompiledLocalPolicy,
}
// Fixed local failure labels only; no association, operation or authority data.
fn installed_operation_unavailable(stage: &'static str, error: &OperationAuthorizationError) {
    if matches!(error, OperationAuthorizationError::Unavailable) {
        use std::io::Write as _;
        let _ = writeln!(std::io::stderr().lock(),
            "{{\"event\":\"toolkit_installed_operation_unavailable\",\"stage\":\"{stage}\",\"cause\":\"Unavailable\"}}");
    }
}

impl InstalledApplicationPolicy {
    /// Helper effects use their one canonical setup deadline, never a review TTL.
    fn allowance_deadline(
        &self, association: &InputAuthorityAssociation,
        purpose: LocalPolicyPurpose, now_ms: u64,
    ) -> Result<u64, OperationAuthorizationError> {
        if purpose == LocalPolicyPurpose::Operation {
            if let Some(deadline) = self.originals.helper_operation_deadline(association)
                .map_err(mcp_control_error)?
            {
                if now_ms >= deadline {
                    return Err(OperationRefused::new(OperationRefusalKind::Denied).into());
                }
                return Ok(deadline);
            }
        }
        // Ordinary work and already-admitted controllers retain their existing
        // policy lifetime. Native grant/restriction minima still apply to all.
        Ok(now_ms.saturating_add(60_000))
    }

    /// The actual activation/component machine is read locally under the
    /// enclosing native publication. BeginClose uses that same publication.
    /// This method performs no file reads and retains no permission snapshot.
    fn current_invocation(
        &self,
        association: &InputAuthorityAssociation,
    ) -> Result<(), OperationAuthorizationError> {
        self.member(association)?;
        if let Some(helper) = self.originals.helper_policy_source(association)
            .map_err(|_| OperationAuthorizationError::Unavailable)?
        {
            self.validate_source_entry(None)?;
            // The canonical helper records the complete original contributor
            // set. Each source's installed component policy remains a conjunct;
            // native separately rechecks its current source/grant authority.
            for contributor in &helper.contributors {
                self.member(contributor)?;
                let component = self.source_component(contributor)?;
                self.validate_source_entry(component)?;
            }
            return Ok(());
        }
        self.validate_source_entry(self.source_component(association)?)
    }

    fn source_component(&self, association: &InputAuthorityAssociation)
        -> Result<Option<&toolkit_package::Id>, OperationAuthorizationError>
    {
        if self.originals.helper_policy_source(association)
            .map_err(|_| OperationAuthorizationError::Unavailable)?.is_some()
        { return Ok(None); }
        let route_id = self.originals.route_for_binding(association)
            .map_err(|_| OperationAuthorizationError::Unavailable)?;
        let route = self.native.routes.iter().find(|route| route.id == route_id)
            .ok_or(OperationAuthorizationError::Unavailable)?;
        match &route.origin {
            meerkat_toolkit::host_lifetime::policy::CompiledNativeSourceOrigin::LocalConsole { .. } => Ok(None),
            meerkat_toolkit::host_lifetime::policy::CompiledNativeSourceOrigin::ProviderEvent {
                channel_id, sender_id, target_identity,
            } => Ok(Some(&self.selected.channels.iter().find(|channel| {
                &channel.channel_id == channel_id && &channel.sender_id == sender_id
                    && &channel.target_identity == target_identity
            }).ok_or(OperationAuthorizationError::Unavailable)?.component)),
        }
    }

    fn validate_source_entry(&self, component: Option<&toolkit_package::Id>)
        -> Result<(), OperationAuthorizationError>
    {
        self.lifetime.validate_native_entry(component).map_err(|error| match error {
            HostLifetimeError::Denied | HostLifetimeError::WrongOwner | HostLifetimeError::InvalidSelection => {
                OperationRefused::new(OperationRefusalKind::Denied).into()
            }
            _ => OperationAuthorizationError::Unavailable,
        })
    }

    fn model_values(
        &self,
        association: &InputAuthorityAssociation,
        facts: &meerkat_core::authorization::ModelAuthorizationFacts,
        purpose: LocalPolicyPurpose,
    ) -> Result<meerkat_authorization::policy::LocalOperationValues, OperationAuthorizationError>
    {
        let claims = association.candidate();
        let selected = claims
            .controller_model
            .as_ref()
            .ok_or(OperationAuthorizationError::Unavailable)?;
        if !selected.matches_model_facts(facts) {
            return Err(OperationRefused::new(OperationRefusalKind::Denied).into());
        }
        if !facts.hosted_capabilities.is_empty() || facts.live_channel.is_some() {
            return Err(OperationAuthorizationError::Unavailable);
        }
        let action = match (purpose, facts.usage) {
            (
                LocalPolicyPurpose::Controller,
                ModelAuthorizationUse::ControllerInference | ModelAuthorizationUse::Compaction,
            ) => application_operations::ControllerAction::Inference,
            (LocalPolicyPurpose::Operation, ModelAuthorizationUse::Compaction) => {
                application_operations::ControllerAction::Compaction
            }
            _ => return Err(OperationAuthorizationError::Unavailable),
        };
        if facts.endpoint.is_empty() || facts.wire_model.is_empty() {
            return Err(OperationAuthorizationError::Unavailable);
        }
        let route = facts
            .controller_route_key()
            .ok_or(OperationAuthorizationError::Unavailable)?;
        Ok(application_operations::controller(
            &self.native.grant_administrator,
            &claims.logical_executor,
            &claims.target.audience,
            &route,
            action,
        ))
    }

    fn member(
        &self,
        association: &InputAuthorityAssociation,
    ) -> Result<&CompiledNativeMember, OperationAuthorizationError> {
        if let Some(policy) = self.originals.service_tool_policy.get() {
            policy.check_current().map_err(|_| OperationAuthorizationError::Unavailable)?;
        }
        let claims = association.candidate();
        if claims.ingress_namespace.realm != self.native.realm
            || claims.ingress_namespace.ingress != self.native.ingress_domain
        {
            return Err(OperationRefused::new(OperationRefusalKind::Denied).into());
        }
        if let Some(source) = self.originals.helper_policy_source(association)
            .map_err(|_| OperationAuthorizationError::Unavailable)?
        {
            let selected = source.selection;
            let member = self.native.members.iter().find(|member| **member == selected.member)
                .ok_or(OperationAuthorizationError::Unavailable)?;
            if selected.grant_caller != self.native.grant_administrator
                || selected.helper.member != member.member
                || claims.original_work.authority != self.native.ingress_domain.authority
            { return Err(OperationRefused::new(OperationRefusalKind::Denied).into()); }
            // The helper source verifies the exact independently selected basis,
            // including native per-target lineages. This historical mapping
            // grants nothing; native resolves that basis for the real operation.
            return Ok(member);
        }
        if !(matches!(&claims.authority_basis, WorkAuthorityBasis::GrantLineage { lineage } if !lineage.is_empty())
            || matches!(&claims.authority_basis, WorkAuthorityBasis::TargetedGrantLineages { .. }))
        { return Err(OperationRefused::new(OperationRefusalKind::Denied).into()); }
        // Targeted grants include configured setup and controller-only work.
        // Exact retained source custody is still required below; native checks
        // only the lineage selected by the actual operation at its entry.
        // The application owns this immutable source, not the wire claims.
        // Its native source adapter has independently retained the exact row;
        // the runtime and native grants still own acceptance/current permission.
        let route_id = self
            .originals
            .route_for_binding(association)
            .map_err(|_| OperationAuthorizationError::Unavailable)?;
        let route = self
            .native
            .routes
            .iter()
            .find(|route| route.id == route_id)
            .ok_or(OperationAuthorizationError::Unavailable)?;
        let (member_binding, requester, actor) = route
            .source_binding(&self.selected)
            .map_err(|_| OperationAuthorizationError::Unavailable)?;
        let member = self
            .native
            .members
            .iter()
            .find(|member| &member.member == member_binding)
            .ok_or(OperationAuthorizationError::Unavailable)?;
        if &claims.requester == requester
            && &claims.ingress_actor == actor
            && claims.represented_subject == route.represented_subject
            && claims.logical_executor == route.logical_executor
            && claims.original_work.authority == route.source_authority
            && claims.target.logical_owner == member.logical_owner
            && claims.target.context_generation == member.context_generation
            && claims.target.audience == member.audience
        {
            return Ok(member);
        }
        Err(OperationRefused::new(OperationRefusalKind::Denied).into())
    }
}
impl ScopedMandateOwner for InstalledApplicationPolicy {
    fn authorize_invocation(
        &self,
        ingress: &NativeIngressContext,
        association: &InputAuthorityAssociation,
    ) -> Result<(), NativeAdmissionError> {
        let claims = association.candidate();
        if ingress.requester() != &claims.requester
            || ingress.ingress_actor() != &claims.ingress_actor
            || ingress.realm() != &claims.ingress_namespace.realm
        {
            return Err(NativeAdmissionError::Refused(OperationRefused::new(
                OperationRefusalKind::Denied,
            )));
        }
        self.current_invocation(association)
            .map_err(|error| match error {
                OperationAuthorizationError::Refused(refusal) => {
                    NativeAdmissionError::Refused(refusal)
                }
                _ => NativeAdmissionError::Readiness(
                    meerkat_core::authorization::ControllerReadinessFailure::PolicyUnavailable,
                ),
            })
    }
}
impl RetainedInvocationOwner for InstalledApplicationPolicy {
    fn reauthorize_retained(
        &self,
        association: &InputAuthorityAssociation,
    ) -> Result<(), NativeAdmissionError> {
        // Native has verified the retained input/contributor ledger. Reuse the
        // same current source, member and host lifetime policy as fresh entry;
        // no fresh transport authentication is invented for a completion.
        self.current_invocation(association).map_err(|error| match error {
            OperationAuthorizationError::Refused(refusal) => NativeAdmissionError::Refused(refusal),
            _ => NativeAdmissionError::Readiness(
                meerkat_core::authorization::ControllerReadinessFailure::PolicyUnavailable,
            ),
        })
    }
}
impl AdmittedWorkPolicyOwner for InstalledApplicationPolicy {
    fn authorize_mcp_attachment(
        &self,
        control: &meerkat_core::mcp_attachment_control::McpAttachmentControlRequest,
        binding: &PreparedAuthorizationBinding,
        now_ms: u64,
    ) -> Result<WorkOwnerAllowance, OperationAuthorizationError> {
        let current = self.originals.current_service_mcp_control(control, binding, now_ms, false)
            .map_err(mcp_control_error)?;
        self.member(&current.parent)?;
        self.current_invocation(&current.parent)?;
        Ok(WorkOwnerAllowance {
            restrictions: current.restrictions,
            expires_at_ms: current.expires_at_ms,
        })
    }

    fn authorize_admitted_work(
        &self,
        association: &InputAuthorityAssociation,
        _: &PreparedAuthorizationBinding,
        purpose: LocalPolicyPurpose,
        now_ms: u64,
    ) -> Result<WorkOwnerAllowance, OperationAuthorizationError> {
        self.member(association).map_err(|error| {
            installed_operation_unavailable("work.member", &error);
            error
        })?;
        if purpose == LocalPolicyPurpose::Operation {
            self.current_invocation(association).map_err(|error| {
                installed_operation_unavailable("work.current_invocation", &error);
                error
            })?;
        }
        // Closing stops new ingress and ordinary effects. Already-admitted
        // controller authority remains until native runtime shutdown settles
        // its custody; activation close is not last-controller revocation.
        Ok(WorkOwnerAllowance {
            restrictions: ExecutionRestrictions::unrestricted(),
            expires_at_ms: self.allowance_deadline(association, purpose, now_ms)?,
        })
    }
}
impl OperationPolicyOwner for InstalledApplicationPolicy {
    fn authorize_mcp_attachment(
        &self,
        control: &meerkat_core::mcp_attachment_control::McpAttachmentControlRequest,
        binding: &PreparedAuthorizationBinding,
        now_ms: u64,
    ) -> Result<LocalPolicyAllowance, OperationAuthorizationError> {
        let current = self.originals.current_service_mcp_control(control, binding, now_ms, true)
            .map_err(mcp_control_error)?;
        self.member(&current.parent)?;
        self.current_invocation(&current.parent)?;
        Ok(LocalPolicyAllowance {
            operation_values: vec![current.values],
            restrictions: current.restrictions,
            expires_at_ms: current.expires_at_ms,
            review_tier: OperationReviewTier::R1,
        })
    }

    fn authorize_controller_admission(
        &self,
        association: &InputAuthorityAssociation,
        facts: &meerkat_core::ControllerModelFacts,
        _: u64,
    ) -> Result<ControllerAdmissionAllowance, OperationAuthorizationError> {
        self.member(association)?;
        let claims = association.candidate();
        if claims.controller_model.as_ref() != Some(facts.selection()) {
            return Err(OperationRefused::new(OperationRefusalKind::Denied).into());
        }
        // The same invocation owner separately fences new admission. Keeping
        // this route valid during close preserves already-admitted controllers.
        if facts.endpoint().is_empty() || facts.wire_model().is_empty() {
            return Err(OperationAuthorizationError::Unavailable);
        }
        let values = application_operations::controller(
            &self.native.grant_administrator,
            &claims.logical_executor,
            &claims.target.audience,
            &facts.route_key(),
            application_operations::ControllerAction::Inference,
        );
        Ok(ControllerAdmissionAllowance {
            operation_values: vec![values],
            restrictions: ExecutionRestrictions::unrestricted(),
        })
    }

    fn authorize_operation(
        &self,
        association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        purpose: LocalPolicyPurpose,
        now_ms: u64,
    ) -> Result<LocalPolicyAllowance, OperationAuthorizationError> {
        self.member(association).map_err(|error| {
            installed_operation_unavailable("operation.member", &error);
            error
        })?;
        if purpose == LocalPolicyPurpose::Operation {
            self.current_invocation(association).map_err(|error| {
                installed_operation_unavailable("operation.current_invocation", &error);
                error
            })?;
        }
        let claims = association.candidate();
        let mut connector_value = None;
        let values = match (&binding.facts().operation, purpose) {
            (AuthorizationOperation::Model(facts), purpose) => {
                self.model_values(association, facts, purpose)?
            }
            (AuthorizationOperation::Source(facts), LocalPolicyPurpose::Operation)
                if facts.usage == SourceAuthorizationUse::Read =>
            {
                let SourceAuthorizationTarget::Memory(scope) = &facts.target else {
                    return Err(OperationAuthorizationError::Unavailable);
                };
                let session = meerkat_core::SessionId::parse(claims.target.context.as_str())
                    .map_err(|_| OperationAuthorizationError::Unavailable)?;
                // A different memory owner within the same session is not the
                // canonical session owner. Compare the complete typed scope.
                if scope != &meerkat_core::memory::MemorySearchScope::for_session(session.clone()) {
                    return Err(OperationRefused::new(OperationRefusalKind::Denied).into());
                }
                application_operations::memory_read(
                    &self.native.grant_administrator,
                    &claims.logical_executor,
                    &claims.target.audience,
                    &session,
                )
            }
            (AuthorizationOperation::Tool(facts), LocalPolicyPurpose::Operation) => {
                let meerkat_core::authorization::ToolAuthorizationTarget::Dispatcher(plan) =
                    &facts.target
                else {
                    return Err(OperationAuthorizationError::Unavailable);
                };
                // The current native execution owner supplies this fact and
                // revalidates it at the leaf. Names and arguments carry no
                // ownership. Composite witnesses without a target add no scope.
                // Account facts remain in the native target for review and leaf
                // revalidation; they grant no operation rights to this R1 mapping.
                let mut targets = plan
                    .owner_witnesses()
                    .iter()
                    .filter_map(|witness| witness.operation_target());
                match (targets.next(), targets.next()) {
                    (
                        Some(meerkat_core::ToolOperationTarget::CatalogControl {
                            operation:
                                meerkat_core::CatalogOperation::Search
                                | meerkat_core::CatalogOperation::Load,
                        }),
                        None,
                    ) => {
                        let session =
                            meerkat_core::SessionId::parse(claims.target.context.as_str())
                                .map_err(|_| OperationAuthorizationError::Unavailable)?;
                        application_operations::catalog_read(
                            &self.native.grant_administrator,
                            &claims.logical_executor,
                            &claims.target.audience,
                            &session,
                        )
                    }
                    (
                        Some(meerkat_core::ToolOperationTarget::RegisteredToolBundleOperation {
                            bundle,
                            operation,
                            account: _,
                        }),
                        None,
                    ) if bundle == crate::FIND_AND_INSTALL_BUNDLE
                        && operation == crate::FIND_AND_INSTALL =>
                    {
                        let session =
                            meerkat_core::SessionId::parse(claims.target.context.as_str())
                                .map_err(|_| OperationAuthorizationError::Unavailable)?;
                        application_operations::find_and_install(
                            &self.native.grant_administrator,
                            &claims.logical_executor,
                            &claims.target.audience,
                            &session,
                        )
                    }
                    (
                        Some(meerkat_core::ToolOperationTarget::RegisteredToolBundleOperation {
                            bundle,
                            operation,
                            account: _,
                        }),
                        None,
                    ) if bundle == meerkat_toolkit::setup::SETUP_RESULTS_BUNDLE
                        && operation == meerkat_toolkit::setup::SETUP_REPORT =>
                    {
                        // The registered owner witness identifies the operation;
                        // only the actual selected helper source may report it.
                        let Some(helper) = self
                            .originals
                            .helper_policy_source(association)
                            .map_err(|_| OperationAuthorizationError::Unavailable)?
                        else {
                            return Err(OperationRefused::new(OperationRefusalKind::Denied).into());
                        };
                        let _session =
                            meerkat_core::SessionId::parse(claims.target.context.as_str())
                                .map_err(|_| OperationAuthorizationError::Unavailable)?;
                        application_operations::setup_report_for_origin(
                            &self.native.grant_administrator,
                            &claims.logical_executor,
                            &claims.target.audience,
                            &helper.origin,
                        ).map_err(|_| OperationAuthorizationError::Unavailable)?
                    }
                    (
                        Some(meerkat_core::ToolOperationTarget::RegisteredToolBundleOperation {
                            bundle, operation, account: _,
                        }),
                        None,
                    ) if bundle == meerkat_toolkit::setup::SETUP_ACCOUNTS_BUNDLE
                        && operation == meerkat_toolkit::setup::SETUP_ACCOUNT_REQUEST =>
                    {
                        // Same canonical helper source, separate native operation.
                        // Neither the request name nor human review grants access.
                        let Some(helper) = self.originals.helper_policy_source(association)
                            .map_err(|_| OperationAuthorizationError::Unavailable)? else {
                            return Err(OperationRefused::new(OperationRefusalKind::Denied).into());
                        };
                        let _session = meerkat_core::SessionId::parse(claims.target.context.as_str())
                            .map_err(|_| OperationAuthorizationError::Unavailable)?;
                        application_operations::setup_account_request_for_origin(
                            &self.native.grant_administrator,
                            &claims.logical_executor,
                            &claims.target.audience,
                            &helper.origin,
                        ).map_err(|_| OperationAuthorizationError::Unavailable)?
                    }
                    (
                        Some(meerkat_core::ToolOperationTarget::RegisteredToolBundleOperation {
                            bundle, operation, account: _,
                        }),
                        None,
                    ) if bundle == meerkat_toolkit::setup::SETUP_ACCOUNTS_BUNDLE
                        && operation == meerkat_toolkit::setup::SETUP_PUBLIC_SERVICE_REQUEST =>
                    {
                        // Same canonical helper source, separate native operation.
                        // Neither the request name nor human review grants access.
                        let Some(helper) = self.originals.helper_policy_source(association)
                            .map_err(|_| OperationAuthorizationError::Unavailable)? else {
                            return Err(OperationRefused::new(OperationRefusalKind::Denied).into());
                        };
                        let _session = meerkat_core::SessionId::parse(claims.target.context.as_str())
                            .map_err(|_| OperationAuthorizationError::Unavailable)?;
                        application_operations::setup_public_service_request_for_origin(
                            &self.native.grant_administrator,
                            &claims.logical_executor,
                            &claims.target.audience,
                            &helper.origin,
                        ).map_err(|_| OperationAuthorizationError::Unavailable)?
                    }
                    (
                        Some(meerkat_core::ToolOperationTarget::McpServerOperation {
                            server, raw_operation, connector, ..
                        }),
                        None,
                    ) => {
                        if !matches!(connector, meerkat_core::mcp_config::McpConnectorApproval::NoConnector) {
                            // This exact source selected the retained access decision.
                            // A current server name or physical generation cannot
                            // invent its Toolkit connection/Revision correspondence.
                            self.originals.check_service_mcp_operation(
                                association, server, raw_operation, connector, now_ms,
                            ).map_err(|error| match error {
                                crate::private_originals::PrivateOriginalError::Denied
                                | crate::private_originals::PrivateOriginalError::Conflict =>
                                    OperationAuthorizationError::from(OperationRefused::new(OperationRefusalKind::Denied)),
                                _ => OperationAuthorizationError::Unavailable,
                            })?;
                        }
                        connector_value = Some(application_operations::mcp_connector_operation(
                            &self.native.grant_administrator, &claims.logical_executor,
                            &claims.target.audience, server, connector,
                        ));
                        application_operations::mcp_operation(
                            &self.native.grant_administrator, &claims.logical_executor,
                            &claims.target.audience, server, raw_operation,
                        )
                    },
                    // Missing, multiple and future operation kinds do not
                    // inherit catalog exposure or downstream invocation access.
                    _ => return Err(OperationAuthorizationError::Unavailable),
                }
            }
            _ => return Err(OperationAuthorizationError::Unavailable),
        };
        let allowance = LocalPolicyAllowance {
            operation_values: std::iter::once(values).chain(connector_value).collect(),
            restrictions: ExecutionRestrictions::unrestricted(),
            expires_at_ms: self.allowance_deadline(association, purpose, now_ms)?,
            review_tier: OperationReviewTier::R1,
        };
        Ok(allowance)
    }
}

fn mcp_control_error(error: crate::private_originals::PrivateOriginalError) -> OperationAuthorizationError {
    match error {
        crate::private_originals::PrivateOriginalError::Denied
        | crate::private_originals::PrivateOriginalError::Conflict =>
            OperationRefused::new(OperationRefusalKind::Denied).into(),
        crate::private_originals::PrivateOriginalError::Unavailable => OperationAuthorizationError::Unavailable,
    }
}

#[cfg(test)]
mod tests {
    mod gateway_identity;
    mod identity_binding;

    use super::*;
    use meerkat_authorization_contracts::{constraints::AudienceRef, evidence::EvidenceId};
    use meerkat_mobkit::member_tool_policy::providers_from_canonical_payloads;
    use meerkat_core::{
        CompiledApplicationToolPolicy, CompiledMemberToolGrants, CompiledPolicySourceProvenance, PolicyDigest, PolicyId,
        PolicyProviderId, PolicyRevision,
        auth::{PrincipalKind, PrincipalRef, TrustDomainId},
    };
    use meerkat_toolkit::host_lifetime::{
        control::SelectedHostArtifact, policy::CompiledLocalPolicy,
    };

    pub(super) fn principal(value: &str) -> PrincipalRef {
        PrincipalRef::in_domain(
            PrincipalKind::ServiceAccount,
            value,
            TrustDomainId::new("factory-test").unwrap(),
        )
        .unwrap()
    }
    fn artifact(bundle: &Path, name: &str, bytes: &[u8]) -> CompiledArtifactReference {
        fs::write(bundle.join(name), bytes).unwrap();
        CompiledArtifactReference {
            name: name.into(),
            sha256: digest(bytes),
        }
    }
    pub(super) fn fixture() -> (tempfile::TempDir, HostBootstrapSelection) {
        let directory = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let root = directory.path().canonicalize().unwrap();
        let bundle = root.join("shared/policy/bundle.1");
        fs::create_dir_all(&bundle).unwrap();
        for name in ["data", "control", "activations"] {
            fs::create_dir_all(root.join("shared").join(name)).unwrap();
        }
        let source = artifact(&bundle, "policy.json", br#"{"schema":1}"#);
        let tool = CompiledApplicationToolPolicy::new(
            PolicyProviderId::new("fixture").unwrap(),
            PolicyId::new("fixture").unwrap(),
            PolicyRevision(1),
            CompiledPolicySourceProvenance {
                source_id: "fixture-source".into(),
                source_digest: PolicyDigest::from_canonical_bytes(b"fixture"),
            },
            vec![CompiledMemberToolGrants {
                member_identity: "agent".into(),
                grants: vec![],
            }],
        )
        .unwrap();
        let tool = artifact(
            &bundle,
            "application-tool-policy.json",
            &tool.canonical_json().unwrap(),
        );
        let surface = artifact(&bundle, "agent-surface.json", b"[]");
        let member = meerkat_core::MobMemberBinding {
            mob_id: "factory".into(),
            role: "identity".into(),
            member: "agent".into(),
        };
        let local = CompiledLocalPolicy {
            schema: 7,
            deployment: toolkit_package::Id::new("deployment").unwrap(),
            source_policy_sha256: source.sha256.clone(),
            components: vec![],
            channels: vec![],
            native_host: Some(CompiledNativeHost { setup: None, service_parent_administration: None,
                remote_operator: None,
                realm: meerkat_core::mob_realm_id("factory").unwrap(),
                grant_administrator: principal("application"),
                ingress_domain: ResourceDomain {
                    authority: principal("source"),
                    namespace: "events".into(),
                },
                application_tool_policy: tool.clone(),
                members: vec![CompiledNativeMember {
                    member: member.clone(),
                    logical_owner: principal("agent"),
                    context_generation: 1,
                    audience: AudienceRef::Principal {
                        principal: principal("operator"),
                    },
                    initial_surface: surface.clone(),
                    setup_capability: None,
                }],
                routes: vec![meerkat_toolkit::host_lifetime::policy::CompiledNativeSourceRoute {
                    id: toolkit_package::Id::new("console").unwrap(),
                    application_parent: None,
                    controller_parent: None,
                    origin: meerkat_toolkit::host_lifetime::policy::CompiledNativeSourceOrigin::LocalConsole {
                        member, requester: principal("operator"), ingress_actor: principal("console"),
                    },
                    logical_executor: principal("agent"), represented_subject: None, source_authority: principal("console"),
                    authentication_domain: ResourceDomain { authority: principal("console"), namespace: "login".into() },
                    event_domain: ResourceDomain { authority: principal("console"), namespace: "input".into() },
                }],
            }),
        };
        let local = artifact(
            &bundle,
            "toolkit-local-policy.json",
            &canonical_json(&local).unwrap(),
        );
        let artifacts: Vec<_> = [source, tool, surface, local.clone()]
            .into_iter()
            .map(|r| serde_json::json!({"path":r.name,"sha256":r.sha256}))
            .collect();
        let manifest = canonical_json(&serde_json::json!({"artifacts": artifacts})).unwrap();
        let manifest = artifact(&bundle, "manifest.json", &manifest);
        let activation = canonical_json(&serde_json::json!({"activation_id":"1","policy_bundle":{
            "path":"shared/policy/bundle.1", "policy_digest":format!("hc-policy-v1:sha256:{}",manifest.sha256)
        }})).unwrap();
        let activation_path = root.join("shared/activations/1.json");
        fs::write(&activation_path, &activation).unwrap();
        std::os::unix::fs::symlink("shared/activations/1.json", root.join("current")).unwrap();
        let selection = HostBootstrapSelection {
            deployment_root: root,
            activation: SelectedHostArtifact {
                path: activation_path,
                sha256: digest(&activation),
            },
            policy_manifest: SelectedHostArtifact {
                path: bundle.join("manifest.json"),
                sha256: manifest.sha256,
            },
            local_policy: SelectedHostArtifact {
                path: bundle.join("toolkit-local-policy.json"),
                sha256: local.sha256,
            },
        };
        (directory, selection)
    }

    // Fixture setup uses real native stores and issuance. It does not stand in
    // for OS authentication of the separate production operator entry.
    pub(super) fn provision_fixture(selection: &mut HostBootstrapSelection) {
        let mut local: CompiledLocalPolicy =
            serde_json::from_slice(&fs::read(&selection.local_policy.path).unwrap()).unwrap();
        let native = local.native_host.as_mut().unwrap();
        let directory = state_directory(selection, native, NativeOwnerStartup::Initialize).unwrap();
        let source = ToolkitOriginalOwner::initialize(
            LocalGrantConfiguration {
                root: principal("admin"),
                namespace: EvidenceId::new("native-grants").unwrap(),
                generation: 1,
            },
            Arc::new(FileGrantOwnerStore::new(directory.join("grants.jsonl"))),
            Arc::new(HostAuthorizationClock),
            native.ingress_domain.clone(),
        )
        .unwrap();
        let _originals = SqliteStore::open(directory.join("originals.sqlite3")).unwrap();
        let parent = source
            .grants()
            .issue_root(
                &principal("admin"),
                EvidenceId::new("application-parent").unwrap(),
                native.grant_administrator.clone(),
                None,
                ExecutionRestrictions {
                    delegation_depth:
                        meerkat_authorization_contracts::constraints::DelegationDepth::remaining(1),
                    ..ExecutionRestrictions::unrestricted()
                },
            )
            .unwrap();
        native.routes[0].application_parent = Some(vec![parent]);
        replace_local(selection, &local);
    }

    pub(super) fn replace_local(
        selection: &mut HostBootstrapSelection,
        local: &CompiledLocalPolicy,
    ) {
        let bytes = canonical_json(local).unwrap();
        fs::write(&selection.local_policy.path, &bytes).unwrap();
        selection.local_policy.sha256 = digest(&bytes);
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(&selection.policy_manifest.path).unwrap()).unwrap();
        for entry in manifest["artifacts"].as_array_mut().unwrap() {
            if entry["path"] == "toolkit-local-policy.json" {
                entry["sha256"] = selection.local_policy.sha256.clone().into();
            }
        }
        let bytes = canonical_json(&manifest).unwrap();
        fs::write(&selection.policy_manifest.path, &bytes).unwrap();
        selection.policy_manifest.sha256 = digest(&bytes);
        let mut activation: serde_json::Value =
            serde_json::from_slice(&fs::read(&selection.activation.path).unwrap()).unwrap();
        activation["policy_bundle"]["policy_digest"] =
            format!("hc-policy-v1:sha256:{}", selection.policy_manifest.sha256).into();
        let bytes = canonical_json(&activation).unwrap();
        fs::write(&selection.activation.path, &bytes).unwrap();
        selection.activation.sha256 = digest(&bytes);
    }

    #[test]
    fn schema_six_missing_controller_selection_does_not_copy_operation_parent() {
        let (_directory, mut selection) = fixture();
        provision_fixture(&mut selection);
        let local: CompiledLocalPolicy =
            serde_json::from_slice(&fs::read(&selection.local_policy.path).unwrap()).unwrap();
        let operation_parent = local.native_host.as_ref().unwrap().routes[0]
            .application_parent
            .clone()
            .unwrap();
        let mut value = serde_json::to_value(&local).unwrap();
        value["native_host"]["routes"][0]
            .as_object_mut()
            .unwrap()
            .remove("controller_parent");
        let decoded: CompiledLocalPolicy = serde_json::from_value(value).unwrap();
        let route = &decoded.native_host.as_ref().unwrap().routes[0];
        assert_eq!(
            route.application_parent.as_deref(),
            Some(operation_parent.as_slice())
        );
        assert!(route.controller_parent.is_none());
        replace_local(&mut selection, &decoded);
        let owner = NativeHostLifetime::bootstrap(selection.clone()).unwrap();
        let selected = &owner.selected_policy().native_host.as_ref().unwrap().routes[0];
        assert_eq!(
            selected.application_parent.as_deref(),
            Some(operation_parent.as_slice())
        );
        assert!(selected.controller_parent.is_none());
        drop(owner);

        // Old compiled documents are explicitly versioned out, not silently
        // reinterpreted as controller selection. No native owner is opened.
        let mut old = decoded.clone();
        old.schema = 5;
        replace_local(&mut selection, &old);
        assert!(matches!(
            NativeHostLifetime::bootstrap(selection.clone()),
            Err(HostLifetimeError::InvalidSelection)
        ));
        let mut empty = decoded;
        empty.native_host.as_mut().unwrap().routes[0].controller_parent = Some(vec![]);
        replace_local(&mut selection, &empty);
        assert!(matches!(
            NativeHostLifetime::bootstrap(selection),
            Err(HostLifetimeError::InvalidSelection)
        ));
    }

    /// Laptop-only owner fixture. Ordinary portable runs report this ignored;
    /// explicit selection requires a path and never substitutes sample bytes.
    #[test]
    #[ignore = "requires an explicitly selected external PairedHost policy artifact"]
    fn external_paired_host_policy_uses_actual_native_provider_parser() {
        let path = std::env::var_os("MEERKAT_TOOLKIT_APPLICATION_POLICY_FIXTURE")
            .expect("set MEERKAT_TOOLKIT_APPLICATION_POLICY_FIXTURE for this explicit check");
        let bytes = fs::read(path).expect("external application policy fixture is unreadable");
        assert_eq!(
            digest(&bytes),
            "96ed1fcf9c4b82e1841cf3804ecfb4f9dbc36f87bb51f432b624643c0f6f4bd6",
            "external application policy fixture differs from the selected owner artifact"
        );
        // Do not print policy names or contents into build/coordination output.
        let providers = providers_from_canonical_payloads([bytes]);
        assert!(
            providers.is_ok(),
            "actual native provider parser rejected the artifact"
        );
        assert_eq!(providers.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn direct_rust_access_is_fenced_by_owner_close_or_selection_replacement() {
        use crate::private_originals::{PrivateOriginalError, PrivateServiceGrantState};
        use meerkat_toolkit::host_lifetime::control::{
            HostCloseReason, PrivateHostLifetimeControl,
        };
        for replace_selection in [false, true] {
            let (_directory, mut selection) = fixture();
            provision_fixture(&mut selection);
            let (owner, installed) =
                InstalledToolkitHost::open(selection.clone(), NativeOwnerStartup::Restore)
                    .await
                    .unwrap();
            let directory =
                state_directory(&selection, &installed.native, NativeOwnerStartup::Restore)
                    .unwrap();
            let owner = Arc::new(owner);
            let (signal, closed) = tokio::sync::watch::channel(false);
            let host = installed.compose(owner.clone(), PrivateHostClosed(closed));
            let mut decision = crate::private_originals::service_grants::tests::decision();
            decision.route = toolkit_package::Id::new("console").unwrap();
            assert_eq!(
                host.apply_service_access_decision(decision.clone())
                    .await
                    .unwrap()
                    .state,
                PrivateServiceGrantState::GrantIssued
            );
            let grant_bytes = fs::read(directory.join("grants.jsonl")).unwrap();
            if replace_selection {
                fs::copy(
                    &selection.activation.path,
                    selection.deployment_root.join("shared/activations/2.json"),
                )
                .unwrap();
                fs::remove_file(selection.deployment_root.join("current")).unwrap();
                std::os::unix::fs::symlink(
                    "shared/activations/2.json",
                    selection.deployment_root.join("current"),
                )
                .unwrap();
            } else {
                owner
                    .begin_close_activation(HostCloseReason::OperatorDisable)
                    .unwrap();
            }
            // The connection signal remains open. This exercises the actual
            // retained owner, without sending a private close command.
            assert!(!*signal.borrow());
            decision.decision = toolkit_package::Id::new("after-owner-change").unwrap();
            assert!(matches!(
                host.apply_service_access_decision(decision).await,
                Err(PrivateOriginalError::Denied)
            ));
            assert_eq!(
                fs::read(directory.join("grants.jsonl")).unwrap(),
                grant_bytes
            );
            let db = rusqlite::Connection::open(directory.join("originals.sqlite3")).unwrap();
            let rows: u64 = db
                .query_row("SELECT count(*) FROM toolkit_records", [], |row| row.get(0))
                .unwrap();
            assert_eq!(
                rows, 1,
                "closed/replaced host admitted a new access decision"
            );
        }
    }

    #[tokio::test]
    async fn direct_rust_original_cancellation_is_fenced_and_does_not_revoke_standing_access() {
        use crate::private_originals::{PrivateOriginalCancellation, PrivateOriginalError};
        use meerkat_toolkit::host_lifetime::control::{
            HostCloseReason, PrivateHostLifetimeControl,
        };
        use toolkit_store::ToolkitStore;
        for replace_selection in [false, true] {
            let (_directory, mut selection) = fixture();
            provision_fixture(&mut selection);
            let (owner, installed) =
                InstalledToolkitHost::open(selection.clone(), NativeOwnerStartup::Restore)
                    .await
                    .unwrap();
            let directory =
                state_directory(&selection, &installed.native, NativeOwnerStartup::Restore)
                    .unwrap();
            let originals = installed.originals();
            let db = installed.store.clone();
            let mut event = crate::original_ingress::tests::private_event("cancel-before-close");
            event.route = toolkit_package::Id::new("console").unwrap();
            let original = originals.retain_event(event).await.unwrap();
            let owner = Arc::new(owner);
            let (signal, closed) = tokio::sync::watch::channel(false);
            let host = installed.compose(owner.clone(), PrivateHostClosed(closed));
            let grants_before = fs::read(directory.join("grants.jsonl")).unwrap();
            assert!(
                host.cancel_original_operation(&original).await.unwrap()
                    == PrivateOriginalCancellation::OperationsCancelled
            );
            let scope = StoreScope {
                realm: owner
                    .selected_policy()
                    .native_host
                    .as_ref()
                    .unwrap()
                    .realm
                    .to_string(),
                owner: owner.selected_policy().deployment.to_string(),
            };
            let before = db.list_records(&scope, None, 10).await.unwrap();
            assert_eq!(before.len(), 1);
            assert_eq!(
                before[0].data.body["operation_cancelled"],
                serde_json::Value::Bool(true)
            );
            if replace_selection {
                fs::copy(
                    &selection.activation.path,
                    selection.deployment_root.join("shared/activations/2.json"),
                )
                .unwrap();
                fs::remove_file(selection.deployment_root.join("current")).unwrap();
                std::os::unix::fs::symlink(
                    "shared/activations/2.json",
                    selection.deployment_root.join("current"),
                )
                .unwrap();
            } else {
                owner
                    .begin_close_activation(HostCloseReason::OperatorDisable)
                    .unwrap();
            }
            assert!(!*signal.borrow());
            assert!(matches!(
                host.cancel_original_operation(&original).await,
                Err(PrivateOriginalError::Denied)
            ));
            assert!(db.list_records(&scope, None, 10).await.unwrap() == before);
            assert_eq!(
                fs::read(directory.join("grants.jsonl")).unwrap(),
                grants_before
            );
        }
    }

    #[tokio::test]
    async fn native_factory_restores_only_explicitly_initialized_stores() {
        let (_directory, mut selection) = fixture();
        assert!(matches!(
            InstalledToolkitHost::open(selection.clone(), NativeOwnerStartup::Restore).await,
            Err(HostLifetimeError::Unavailable)
        ));
        assert!(
            !selection
                .deployment_root
                .join("shared/data/toolkit-native")
                .exists()
        );
        provision_fixture(&mut selection);
        let (owner, installed) =
            InstalledToolkitHost::open(selection.clone(), NativeOwnerStartup::Restore)
                .await
                .unwrap();
        let native = owner.selected_policy().native_host.as_ref().unwrap();
        let directory = state_directory(&selection, native, NativeOwnerStartup::Restore).unwrap();
        assert_eq!(
            fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let original = fs::read(directory.join("grants.jsonl")).unwrap();
        let evidence = crate::private_originals::PrivateSourceEvidence {
            resource_id: EvidenceId::new("console-input").unwrap(),
            revision: EvidenceId::new("1").unwrap(),
            sha256: "b".repeat(64),
        };
        let event = crate::private_originals::PrivateAuthenticatedEvent {
            route: toolkit_package::Id::new("console").unwrap(),
            original: EvidenceId::new("original").unwrap(),
            occurrence: EvidenceId::new("occurrence").unwrap(),
            original_authentication: evidence.clone(),
            root_event: evidence,
            contributors: vec![],
            content_sha256: "c".repeat(64),
        };
        let reference = installed
            .originals
            .retain_event(event.clone())
            .await
            .unwrap();
        drop(installed);
        drop(owner);
        let (owner, installed) =
            InstalledToolkitHost::open(selection.clone(), NativeOwnerStartup::Restore)
                .await
                .unwrap();
        assert_eq!(fs::read(directory.join("grants.jsonl")).unwrap(), original);
        assert!(installed.originals.event(&reference).await.unwrap() == event);
        assert!(
            installed
                .originals
                .binding_evidence(&reference)
                .await
                .unwrap()
                .is_none()
        );
        assert!(matches!(
            installed
                .originals
                .revoke_original_operation_grant(&reference),
            Err(crate::private_originals::PrivateOriginalError::Unavailable)
        ));
        drop(installed);
        drop(owner);
        assert!(matches!(
            InstalledToolkitHost::open(selection.clone(), NativeOwnerStartup::Initialize).await,
            Err(HostLifetimeError::Denied)
        ));
        assert_eq!(fs::read(directory.join("grants.jsonl")).unwrap(), original);
        fs::write(directory.join("grants.jsonl"), b"corrupt-owner\n").unwrap();
        assert!(matches!(
            InstalledToolkitHost::open(selection, NativeOwnerStartup::Restore).await,
            Err(HostLifetimeError::Unavailable)
        ));
        assert_eq!(
            fs::read(directory.join("grants.jsonl")).unwrap(),
            b"corrupt-owner\n"
        );
    }

    #[tokio::test]
    async fn restore_keeps_native_owner_when_application_identity_changes() {
        let (_directory, mut selection) = fixture();
        provision_fixture(&mut selection);
        let (owner, installed) =
            InstalledToolkitHost::open(selection.clone(), NativeOwnerStartup::Restore)
                .await
                .unwrap();
        let directory =
            state_directory(&selection, &installed.native, NativeOwnerStartup::Restore).unwrap();
        let original = fs::read(directory.join("grants.jsonl")).unwrap();
        drop(installed);
        drop(owner);

        // A subsequent compiled bundle need not carry old initialization
        // metadata. Its administrative caller is a separate application fact,
        // not a claim that this is the restored native root.
        let mut local: CompiledLocalPolicy =
            serde_json::from_slice(&fs::read(&selection.local_policy.path).unwrap()).unwrap();
        let native = local.native_host.as_mut().unwrap();
        native.grant_administrator = principal("replacement-administrator");
        let local_bytes = canonical_json(&local).unwrap();
        fs::write(&selection.local_policy.path, &local_bytes).unwrap();
        selection.local_policy.sha256 = digest(&local_bytes);
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(&selection.policy_manifest.path).unwrap()).unwrap();
        for entry in manifest["artifacts"].as_array_mut().unwrap() {
            if entry["path"] == "toolkit-local-policy.json" {
                entry["sha256"] = selection.local_policy.sha256.clone().into();
            }
        }
        let manifest_bytes = canonical_json(&manifest).unwrap();
        fs::write(&selection.policy_manifest.path, &manifest_bytes).unwrap();
        selection.policy_manifest.sha256 = digest(&manifest_bytes);
        let mut activation: serde_json::Value =
            serde_json::from_slice(&fs::read(&selection.activation.path).unwrap()).unwrap();
        activation["policy_bundle"]["policy_digest"] =
            format!("hc-policy-v1:sha256:{}", selection.policy_manifest.sha256).into();
        let activation_bytes = canonical_json(&activation).unwrap();
        fs::write(&selection.activation.path, &activation_bytes).unwrap();
        selection.activation.sha256 = digest(&activation_bytes);

        let (owner, installed) =
            InstalledToolkitHost::open(selection.clone(), NativeOwnerStartup::Restore)
                .await
                .unwrap();
        assert_eq!(
            installed.native.grant_administrator,
            principal("replacement-administrator")
        );
        assert_eq!(fs::read(directory.join("grants.jsonl")).unwrap(), original);
        drop(installed);
        drop(owner);
        assert!(matches!(
            InstalledToolkitHost::open(selection, NativeOwnerStartup::Initialize).await,
            Err(HostLifetimeError::Denied)
        ));
        assert_eq!(fs::read(directory.join("grants.jsonl")).unwrap(), original);
    }

    #[tokio::test]
    async fn native_policy_back_reference_does_not_retain_machine_or_store() {
        let (_directory, mut selection) = fixture();
        provision_fixture(&mut selection);
        let (owner, installed) =
            InstalledToolkitHost::open(selection.clone(), NativeOwnerStartup::Restore)
                .await
                .unwrap();
        let owner = Arc::new(owner);
        let policy = Arc::new(InstalledApplicationPolicy {
            originals: installed.originals.clone(),
            lifetime: owner.clone(),
            native: installed.native.clone(),
            selected: owner.selected_policy().clone(),
        });
        // The actual native authorization attachment retains this policy, which
        // itself retains the private source adapter. Exercise the real cycle.
        let machine = Arc::new(
            meerkat_runtime::MeerkatMachine::ephemeral()
                .with_local_grant_authorization(
                    installed
                        .originals
                        .configuration(policy),
                )
                .unwrap(),
        );
        installed.originals.bind_machine(machine.clone()).unwrap();
        assert!(Arc::ptr_eq(
            &installed.originals.machine().unwrap(),
            &machine
        ));
        let weak = Arc::downgrade(&machine);
        drop(machine);
        assert!(weak.upgrade().is_none());
        assert!(matches!(
            installed.originals.machine(),
            Err(crate::private_originals::PrivateOriginalError::Unavailable)
        ));
        drop(installed);
        drop(owner);
        // Native restoration reacquires its actual store lease after all host
        // owners are dropped. No journal inspection or force-close is used.
        assert!(
            InstalledToolkitHost::open(selection, NativeOwnerStartup::Restore)
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn native_factory_refuses_legacy_source_format_before_exposing_adapter() {
        use toolkit_store::{AdmissionRequest, RecordData, StoreKey, ToolkitStore};
        let (_directory, mut selection) = fixture();
        provision_fixture(&mut selection);
        let owner = NativeHostLifetime::bootstrap(selection.clone()).unwrap();
        let native = owner.selected_policy().native_host.as_ref().unwrap();
        let directory = state_directory(&selection, native, NativeOwnerStartup::Restore).unwrap();
        let scope = StoreScope {
            realm: native.realm.to_string(),
            owner: owner.selected_policy().deployment.to_string(),
        };
        drop(owner);
        let store = SqliteStore::open_existing(directory.join("originals.sqlite3")).unwrap();
        let key = StoreKey {
            realm: scope.realm.clone(),
            owner: scope.owner.clone(),
            id: "legacy-source".into(),
        };
        store
            .admit(AdmissionRequest {
                admission_key: key.id.clone(),
                key,
                data: RecordData {
                    schema: "toolkit.authenticated-source-event".into(),
                    version: 2,
                    body: serde_json::json!({
                        "event": crate::original_ingress::tests::private_event("legacy"),
                        "final_association": null,
                        "operation_cancelled": true,
                    }),
                },
                events: vec![],
                effects: vec![],
            })
            .await
            .unwrap();
        let before = store.list_records(&scope, None, 10).await.unwrap();
        drop(store);
        assert!(matches!(
            InstalledToolkitHost::open(selection, NativeOwnerStartup::Restore).await,
            Err(HostLifetimeError::InvalidSelection)
        ));
        let store = SqliteStore::open_existing(directory.join("originals.sqlite3")).unwrap();
        assert!(store.list_records(&scope, None, 10).await.unwrap() == before);
    }

    pub(super) const INVALID_EXECUTABLE_CASES: &[&str] = &[
        "tool_malformed",
        "tool_noncanonical",
        "tool_missing_member",
        "surface_malformed",
        "surface_noncanonical",
        "surface_duplicate_name",
    ];

    // Preserve a completely consistent envelope and correct artifact digests.
    // These controls test semantic artifact validation, not a hash mismatch.
    pub(super) fn invalid_executable_artifact(selection: &mut HostBootstrapSelection, case: &str) {
        let bundle = selection
            .policy_manifest
            .path
            .parent()
            .unwrap()
            .to_path_buf();
        let mut local: CompiledLocalPolicy =
            serde_json::from_slice(&fs::read(&selection.local_policy.path).unwrap()).unwrap();
        let native = local.native_host.as_mut().unwrap();
        let reference = if case.starts_with("tool_") {
            &mut native.application_tool_policy
        } else {
            &mut native.members[0].initial_surface
        };
        let original = fs::read(bundle.join(&reference.name)).unwrap();
        let bytes = match case {
            "tool_malformed" => b"not-json".to_vec(),
            "tool_noncanonical" => serde_json::to_vec_pretty(
                &serde_json::from_slice::<serde_json::Value>(&original).unwrap(),
            )
            .unwrap(),
            "tool_missing_member" => {
                let policy: CompiledApplicationToolPolicy =
                    serde_json::from_slice(&original).unwrap();
                CompiledApplicationToolPolicy::new(
                    policy.provider_id,
                    policy.policy_id,
                    policy.revision,
                    policy.source,
                    vec![meerkat_core::CompiledMemberToolGrants {
                        member_identity: "different-member".into(),
                        grants: vec![],
                    }],
                )
                .unwrap()
                .canonical_json()
                .unwrap()
            }
            "surface_malformed" => b"[{}]".to_vec(),
            "surface_noncanonical" => b"[\n]\n".to_vec(),
            "surface_duplicate_name" => {
                let tool =
                    meerkat_core::ToolDef::new("same-tool", "fixture", serde_json::json!({}))
                        .identity();
                canonical_json(&vec![tool.clone(), tool]).unwrap()
            }
            _ => panic!("unknown executable artifact control"),
        };
        reference.sha256 = digest(&bytes);
        fs::write(bundle.join(&reference.name), &bytes).unwrap();
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(&selection.policy_manifest.path).unwrap()).unwrap();
        for row in manifest["artifacts"].as_array_mut().unwrap() {
            if row["path"] == reference.name {
                row["sha256"] = reference.sha256.clone().into();
            }
        }
        fs::write(
            &selection.policy_manifest.path,
            canonical_json(&manifest).unwrap(),
        )
        .unwrap();
        replace_local(selection, &local);
    }

    #[test]
    fn shared_executable_validator_accepts_actual_fixture_and_refuses_correct_hash_invalid_content()
    {
        let (_directory, selection) = fixture();
        let selected = NativeHostLifetime::bootstrap(selection.clone()).unwrap();
        let native = selected.selected_policy().native_host.as_ref().unwrap();
        assert!(
            validate_installed_artifacts(native, |reference| read_bundle_artifact(
                &selection, reference
            ))
            .is_ok()
        );
        drop(selected);
        for case in INVALID_EXECUTABLE_CASES {
            let (_directory, mut selection) = fixture();
            invalid_executable_artifact(&mut selection, case);
            let selected = NativeHostLifetime::bootstrap(selection.clone()).unwrap();
            let native = selected.selected_policy().native_host.as_ref().unwrap();
            assert!(
                matches!(
                    validate_installed_artifacts(native, |reference| read_bundle_artifact(
                        &selection, reference
                    )),
                    Err(HostLifetimeError::InvalidSelection)
                ),
                "{case}"
            );
            assert!(
                !selection
                    .deployment_root
                    .join("shared/data/toolkit-native")
                    .exists()
            );
        }
    }

    #[tokio::test]
    async fn native_factory_rejects_changed_artifact_without_creating_owner() {
        let (_directory, mut selection) = fixture();
        let bundle = selection.policy_manifest.path.parent().unwrap();
        fs::write(bundle.join("agent-surface.json"), b"[{}]").unwrap();
        assert!(matches!(
            InstalledToolkitHost::open(selection.clone(), NativeOwnerStartup::Restore).await,
            Err(HostLifetimeError::InvalidSelection)
        ));
        assert!(
            !selection
                .deployment_root
                .join("shared/data/toolkit-native")
                .exists()
        );
    }
    #[tokio::test]
    async fn ordinary_application_cannot_initialize_operator_owner() {
        let (_directory, selection) = fixture();
        assert!(matches!(
            InstalledToolkitHost::open(selection.clone(), NativeOwnerStartup::Initialize).await,
            Err(HostLifetimeError::Denied)
        ));
        assert!(
            !selection
                .deployment_root
                .join("shared/data/toolkit-native")
                .exists()
        );
    }
}
