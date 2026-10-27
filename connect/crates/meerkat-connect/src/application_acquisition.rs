//! Trusted first-party acquisition selection from the activated application bundle.
//! This composes existing installed owners; it opens no runtime or WorkGraph store.
use std::{collections::BTreeSet, sync::Arc};
use meerkat_toolkit::{
    authority::Ceiling,
    host_lifetime::{control::{HostBootstrapSelection, HostLifetimeError}, policy::{CompiledArtifactReference, CompiledNativeHost}},
    setup::WorkAdmissionStepOptions,
};
use serde::{Deserialize, Serialize};
use toolkit_mobkit::{
    host_gateway::{PreparedSetupApplication, SetupBootstrapProjection, ToolkitGatewayHost},
    installed_host::{InstalledToolkitHost, read_bundle_artifact, acquisition::{InstalledNativeContinuation, InstalledSetupBootstrap, InstalledSetupLimits}},
};
use toolkit_package::Id;
use crate::application_setup::{ApplicationSetupDefinition, ApplicationSetupTools, PreparedApplicationSetup};

pub mod selected_policy;
use selected_policy::{SelectedPolicyBinding, StandingPolicySelection};

// This is only a configuration refusal. A present enrollment owner does not
// replace native member/operation authority or approve an installation.
fn require_setup_review(
    tools: ApplicationSetupTools,
    presenters: Option<&crate::application_gateway::LocalSetupApprovals>,
) -> Result<(), HostLifetimeError> {
    if tools.requires_software_review() {
        presenters.ok_or(HostLifetimeError::InvalidSelection)?
            .check_open().map_err(|_| HostLifetimeError::Unavailable)?;
    }
    Ok(())
}

/// The application compiler owns this selection and pins it as native_host.setup.
/// The resource preparer supplies only prepared_application_setup. Backend
/// generation belongs to the application's retained WorkGraph selection and
/// must be stable across restart until that owner declares replacement.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApplicationAcquisitionSelection {
    pub schema: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub console_setup: Option<crate::application_gateway::console_setup::ConsoleSetupSelection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_operator_recovery: Option<toolkit_mobkit::host_gateway::LocalOperatorRecoveryPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub standing_policy: Option<StandingPolicySelection>,
    pub prepared_application_setup: CompiledArtifactReference,
    pub backend_generation: Id,
    pub proposal_ceiling: Ceiling,
    pub output_audience: BTreeSet<Id>,
    pub limits: InstalledSetupLimits,
}
/// Configuration facts selected before a prepared artifact exists. The same
/// application compiler completes the artifact reference after native finish.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApplicationAcquisitionParameters {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub standing_policy: Option<StandingPolicySelection>,
    pub console_setup: Option<crate::application_gateway::console_setup::ConsoleSetupSelection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_operator_recovery: Option<toolkit_mobkit::host_gateway::LocalOperatorRecoveryPolicy>,
    pub backend_generation: Id,
    pub proposal_ceiling: Ceiling,
    pub output_audience: BTreeSet<Id>,
    pub limits: InstalledSetupLimits,
}
impl ApplicationAcquisitionParameters {
    pub fn complete(self, prepared_application_setup: CompiledArtifactReference) -> ApplicationAcquisitionSelection {
        ApplicationAcquisitionSelection { schema: 1, console_setup: self.console_setup, local_operator_recovery: self.local_operator_recovery, standing_policy: self.standing_policy,
            prepared_application_setup, backend_generation: self.backend_generation,
            proposal_ceiling: self.proposal_ceiling, output_audience: self.output_audience,
            limits: self.limits }
    }
    pub(crate) fn matches(&self, selected: &ApplicationAcquisitionSelection) -> Result<bool, HostLifetimeError> {
        let expected = self.clone().complete(selected.prepared_application_setup.clone());
        let encode = |value: &ApplicationAcquisitionSelection| toolkit_package::canonical_json(value)
            .map_err(|_| HostLifetimeError::InvalidSelection);
        Ok(encode(&expected)? == encode(selected)?)
    }
    pub(crate) fn validate(&self) -> Result<(), HostLifetimeError> {
        if self.output_audience.is_empty() || !self.output_audience.is_subset(&self.proposal_ceiling.output_audiences) {
            return Err(HostLifetimeError::InvalidSelection);
        }
        if let Some(console) = &self.console_setup { console.validate().map_err(|_| HostLifetimeError::InvalidSelection)?; }
        Ok(())
    }
}

impl ApplicationAcquisitionSelection {
    fn validate(&self) -> Result<(), HostLifetimeError> {
        self.prepared_application_setup.validate()?;
        if let Some(console) = &self.console_setup { console.validate().map_err(|_| HostLifetimeError::InvalidSelection)?; }
        if self.schema != 1 || self.output_audience.is_empty()
            || !self.output_audience.is_subset(&self.proposal_ceiling.output_audiences)
        { return Err(HostLifetimeError::InvalidSelection); }
        Ok(())
    }
}

/// Exact selected bytes and their existing typed projection. This is read-only
/// configuration, never a runtime, grant, installed owner or readiness receipt.
pub(crate) struct SelectedApplicationAcquisition {
    config: ApplicationAcquisitionSelection,
    prepared: PreparedApplicationSetup,
    prepared_bytes: Vec<u8>,
    helper: crate::setup::PreparedSetupHelper,
}

impl SelectedApplicationAcquisition {
    pub(crate) fn configuration(&self) -> &ApplicationAcquisitionSelection { &self.config }
    pub(crate) fn prepared(&self) -> &PreparedApplicationSetup { &self.prepared }

    /// The trusted issuer reads this exact configured helper even before a
    /// report capability exists. This projection neither fills nor publishes it.
    pub(crate) fn matches_preparation(&self, parameters: &ApplicationAcquisitionParameters,
        prepared: &PreparedApplicationSetup) -> Result<bool, HostLifetimeError> {
        Ok(parameters.matches(&self.config)? && toolkit_package::canonical_json(prepared)
            .map_err(|_| HostLifetimeError::InvalidSelection)? == self.prepared_bytes)
    }

    pub(crate) fn helper_selection(&self) -> &toolkit_mobkit::installed_host::acquisition::InstalledSetupSelection {
        &self.prepared.helper
    }
}

/// Both the installer and gateway walk the same typed artifact closure. The
/// installer retains every pinned read in its candidate; the gateway reads it
/// through the selected manifest. Referenced bytes are never regenerated.
pub(crate) fn read_application_acquisition(
    native: &CompiledNativeHost,
    mut read: impl FnMut(&CompiledArtifactReference) -> Result<Vec<u8>, HostLifetimeError>,
) -> Result<Option<SelectedApplicationAcquisition>, HostLifetimeError> {
    let Some(reference) = &native.setup else { return Ok(None); };
    let mut checked = |reference: &CompiledArtifactReference| {
        reference.validate()?;
        let bytes = read(reference)?;
        if crate::application_bootstrap::hash(&bytes) != reference.sha256 {
            return Err(HostLifetimeError::InvalidSelection);
        }
        Ok(bytes)
    };
    let config: ApplicationAcquisitionSelection = serde_json::from_slice(&checked(reference)?)
        .map_err(|_| HostLifetimeError::InvalidSelection)?;
    config.validate()?;
    // Keep the exact bytes for the SDK comparison at runtime, not a re-encoding.
    let prepared_bytes = checked(&config.prepared_application_setup)?;
    let prepared: PreparedApplicationSetup = serde_json::from_slice(&prepared_bytes)
        .map_err(|_| HostLifetimeError::InvalidSelection)?;
    if config.standing_policy.is_some() && !prepared.helper_tools.requires_software_review() {
        return Err(HostLifetimeError::InvalidSelection);
    }
    if prepared.realm.realm_id != native.realm.as_str()
        || prepared.native_members != native.members
        || prepared.application_tool_policy != native.application_tool_policy
    { return Err(HostLifetimeError::InvalidSelection); }
    let policy = meerkat_core::CompiledApplicationToolPolicy::parse_canonical_json(
        &checked(&prepared.application_tool_policy)?,
    ).map_err(|_| HostLifetimeError::InvalidSelection)?;
    let helper = prepared.helper_projection_for_policy(&policy)
        .map_err(|_| HostLifetimeError::InvalidSelection)?;
    Ok(Some(SelectedApplicationAcquisition { config, prepared, prepared_bytes, helper }))
}

/// Kept by the actual gateway factory until it composes the existing host. The
/// one-use installed bootstrap owns capability selection and driver lifetime.
pub struct PreparedGatewayAcquisition {
    continuation: Arc<InstalledNativeContinuation>,
    projection: SetupBootstrapProjection,
    application: PreparedSetupApplication,
    helper_member: meerkat_core::MobMemberBinding,
    helper_tools: ApplicationSetupTools,
    standing_policy: Option<StandingPolicySelection>,
    selected_policy: Option<Arc<SelectedPolicyBinding>>,
    acquisition_reference: CompiledArtifactReference,
    realm: meerkat_core::RealmLocator,
    console_setup: Option<crate::application_gateway::console_setup::ConsoleSetupSelection>,
    local_operator_recovery: Option<toolkit_mobkit::host_gateway::LocalOperatorRecoveryPolicy>,
    // Ordinary completed-bundle startup creates this from authenticated output.
    // A held driver already retains the original resource participant/provider.
    resource: Option<Arc<crate::application_setup::resource::SetupResourceSelection>>,
}
impl PreparedGatewayAcquisition {
    /// Final adoption for a factory already composed by the held driver. Its
    /// existing channel and weak task source are retained, not recreated.
    pub(crate) fn compose_prepared_factory(self, host: ToolkitGatewayHost,
        presenters: Option<&crate::application_gateway::LocalSetupApprovals>,
        selected: Option<&Arc<SelectedPolicyBinding>>,
        owner: Arc<meerkat_toolkit::host_lifetime::NativeHostLifetime>,
        service_parent: toolkit_mobkit::private_host::ServiceParentEndpoint,
    ) -> Result<ToolkitGatewayHost, HostLifetimeError> {
        match (self.standing_policy, selected) {
            (Some(selection), Some(binding)) => binding.bind_with_service_parent(owner, selection, &self.acquisition_reference, Some(service_parent))?,
            (None, None) => require_setup_review(self.helper_tools, presenters)?,
            _ => return Err(HostLifetimeError::InvalidSelection),
        }
        let console = if let Some(selection) = &self.console_setup {
            let panel = crate::application_gateway::console_setup::ConsoleSetup::new(
                selection.clone(), host.console_setup_binding()?,
            ).map_err(|_| HostLifetimeError::InvalidSelection)?;
            presenters.ok_or(HostLifetimeError::InvalidSelection)?.adopt_console(panel.clone())
                .map_err(|_| HostLifetimeError::InvalidSelection)?;
            Some(panel)
        } else { None };
        // The held driver installed the exact composer before construction and
        // retains this same enrollment owner through final adoption. Install
        // the prepared setup application before attaching its Console routes.
        let host = self.compose_resources(host)?;
        match console {
            Some(panel) => host.with_console_routes(Arc::new(move |viewer| panel.routes(viewer)
                .map_err(|_| HostLifetimeError::Unavailable))),
            None => Ok(host),
        }
    }
    /// A single selected review/presenter owner per native construction. The
    /// optional Console uses the same task/login owner as this private host.
    pub fn compose_with_presenters(self, host: ToolkitGatewayHost,
        local: Option<crate::application_gateway::LocalSetupApprovals>, artifact_parent: std::path::PathBuf,
    ) -> Result<(ToolkitGatewayHost, Option<crate::application_gateway::LocalSetupReviews>), HostLifetimeError> {
        let Some(selection) = self.console_setup.clone() else {
            return Ok((match local { Some(local) => self.compose_with_local_setup(host, local)?, None => self.compose(host)? }, None));
        };
        selection.validate().map_err(|_| HostLifetimeError::InvalidSelection)?;
        let panel = crate::application_gateway::console_setup::ConsoleSetup::new(selection, host.console_setup_binding()?)
            .map_err(|_| HostLifetimeError::InvalidSelection)?;
        let (approvals, reviews) = match local {
            Some(local) => (local, None),
            None => { let (approvals, reviews) = crate::application_gateway::setup_presenter::channel(artifact_parent); (approvals, Some(reviews)) },
        };
        let approvals = approvals.with_console(panel.clone());
        let host = self.compose_with_local_setup(host, approvals)?;
        let host = host.with_console_routes(Arc::new(move |viewer| panel.routes(viewer).map_err(|_| HostLifetimeError::Unavailable)))?;
        Ok((host, reviews))
    }

    /// Bundled executable selection. The pinned acquisition file selects the
    /// bounded mode; actual administrative authorization comes separately from
    /// its protected parent owner and cannot be selected by tool arguments.
    pub fn compose_selected(self, host: ToolkitGatewayHost,
        owner: Arc<meerkat_toolkit::host_lifetime::NativeHostLifetime>,
        local: Option<crate::application_gateway::LocalSetupApprovals>, artifact_parent: std::path::PathBuf,
    ) -> Result<(ToolkitGatewayHost, Option<crate::application_gateway::LocalSetupReviews>), HostLifetimeError> {
        self.compose_selected_with_service_parent(host, owner, local, artifact_parent, None)
    }

    /// The bundled gateway supplies its actual same-owner private endpoint.
    /// Serialized application selection cannot construct an admin producer.
    pub fn compose_selected_with_service_parent(self, host: ToolkitGatewayHost,
        owner: Arc<meerkat_toolkit::host_lifetime::NativeHostLifetime>,
        local: Option<crate::application_gateway::LocalSetupApprovals>, artifact_parent: std::path::PathBuf,
        service_parent: Option<toolkit_mobkit::private_host::ServiceParentEndpoint>,
    ) -> Result<(ToolkitGatewayHost, Option<crate::application_gateway::LocalSetupReviews>), HostLifetimeError> {
        let Some(selection) = self.standing_policy else {
            return self.compose_with_presenters(host, local, artifact_parent);
        };
        let native = owner.selected_policy().native_host.as_ref().ok_or(HostLifetimeError::InvalidSelection)?;
        let binding = self.selected_policy.clone().unwrap_or_else(|| SelectedPolicyBinding::new(selection,
            native.grant_administrator.clone(), owner.selected_policy().deployment.clone(), native.realm.clone(),
            native.members.iter().map(|row| row.member.member.clone()).collect()));
        binding.bind_with_service_parent(owner.clone(), selection, &self.acquisition_reference, service_parent)?;
        let (console, presenters, reviews) = if let Some(console) = self.console_setup.clone() {
            let panel = crate::application_gateway::console_setup::ConsoleSetup::new(console, host.console_setup_binding()?)
                .map_err(|_| HostLifetimeError::InvalidSelection)?;
            let (presenters, reviews) = match local {
                Some(local) => (local, None),
                None => { let (presenters, reviews) = crate::application_gateway::setup_presenter::channel(artifact_parent.clone());
                    (presenters, Some(reviews)) },
            };
            let presenters = presenters.with_console(panel.clone());
            (Some(panel), Some(presenters), reviews)
        } else { (None, local, None) };
        let member = self.helper_member.clone();
        let realm = self.realm.clone();
        let host = self.compose_resources(host)?;
        let host = match console {
            Some(panel) => host.with_console_routes(Arc::new(move |viewer| panel.routes(viewer)
                .map_err(|_| HostLifetimeError::Unavailable)))?,
            None => host,
        };
        let host = host.with_setup_member_factory_for_owner(&owner, Arc::new(move |construction, tasks| {
            crate::installed_service_review::composer::gateway_selected_policy_composition(construction,
                member.clone(), realm.clone(), binding.clone(), presenters.clone(), artifact_parent.clone(), tasks)
        }))?;
        Ok((host, reviews))
    }

    /// Explicit unattended application composition. Installation and service
    /// access have independent actual policy owners on this same host lifetime.
    /// No presenter, account request receiver or synthetic Console login is
    /// created. Missing human action is returned to the owning setup invocation;
    /// the caller must select its actual human surface to continue that action.
    /// A selected Console keeps its protected routes and status owner.
    pub fn compose_with_application_policies(self, host: ToolkitGatewayHost,
        installation: crate::approval::installation_policy::InstalledInstallationPolicy,
        service: Option<Arc<toolkit_mobkit::private_originals::InstalledApplicationServicePolicy>>,
        artifact_parent: std::path::PathBuf,
    ) -> Result<ToolkitGatewayHost, HostLifetimeError> {
        if !self.helper_tools.requires_software_review()
            || service.as_ref().is_some_and(|service| !service.belongs_to(installation.lifetime())) {
            return Err(HostLifetimeError::InvalidSelection);
        }
        let console = if let Some(selection) = &self.console_setup {
            Some(crate::application_gateway::console_setup::ConsoleSetup::new(
                selection.clone(), host.console_setup_binding()?,
            ).map_err(|_| HostLifetimeError::InvalidSelection)?)
        } else { None };
        let selected = self.helper_member.clone();
        let realm = self.realm.clone();
        let owner = installation.lifetime().clone();
        let host = self.compose_resources(host)?;
        let host = match console {
            Some(panel) => host.with_console_routes(Arc::new(move |viewer| panel.routes(viewer)
                .map_err(|_| HostLifetimeError::Unavailable)))?,
            None => host,
        };
        host.with_setup_member_factory_for_owner(&owner, Arc::new(move |construction, tasks| {
            crate::installed_service_review::composer::gateway_policy_composition(
                construction, selected.clone(), realm.clone(), installation.clone(),
                service.clone(), artifact_parent.clone(), tasks)
        }))
    }

    pub fn compose(self, host: ToolkitGatewayHost) -> Result<ToolkitGatewayHost, HostLifetimeError> {
        require_setup_review(self.helper_tools, None)?;
        self.compose_resources(host)
    }

    fn compose_resources(self, host: ToolkitGatewayHost) -> Result<ToolkitGatewayHost, HostLifetimeError> {
        let host = match self.local_operator_recovery {
            Some(policy) => host.with_local_operator_recovery(policy)?, None => host,
        };
        let host = host.with_native_setup_bootstrap(self.continuation, self.projection, self.application)?;
        match self.resource {
            Some(resource) => {
                let participant = resource.clone();
                host.with_configured_mcp_resource(
                    move |installed| Arc::new(crate::application_setup::resource::SetupResourceProvider::new(resource, installed)),
                    Arc::new(move |identity, destination| participant.selected_destination(identity, destination)),
                )
            }
            None => Ok(host),
        }
    }

    /// Join the already transferred operator terminal to the selected helper's
    /// native factory constructions. helper_projection verified the exact
    /// inline profile and configured setup server before this value was built;
    /// a no-MCP or conflicting profile cannot substitute for that selection.
    /// Native surface/policy validation still decides whether the complete
    /// constructed tool set is allowed. This method adds no tool permission.
    pub fn compose_with_local_setup(self, host: ToolkitGatewayHost,
        presenters: crate::application_gateway::LocalSetupApprovals,
    ) -> Result<ToolkitGatewayHost, HostLifetimeError> {
        require_setup_review(self.helper_tools, Some(&presenters))?;
        let selected = self.helper_member.clone();
        let realm = self.realm.clone();
        let host = self.compose_resources(host)?;
        host.with_setup_member_factory(Arc::new(move |construction, tasks| {
            crate::installed_service_review::composer::gateway_account_composition(
                construction, selected.clone(), realm.clone(), presenters.clone(), tasks)
        }))
    }
}

/// Ordinary installed startup retains the same selected service participant
/// across canonical member-policy recovery and final setup composition. The
/// candidate reader opens no owner; open revalidates actual current selection.
pub async fn open_gateway_acquisition(
    selection: meerkat_toolkit::host_lifetime::control::HostBootstrapSelection,
    startup: toolkit_mobkit::installed_host::NativeOwnerStartup,
) -> Result<(meerkat_toolkit::host_lifetime::NativeHostLifetime, InstalledToolkitHost,
    Option<PreparedGatewayAcquisition>), HostLifetimeError> {
    let (owner, installed, participant) = open_gateway_owner(selection.clone(), startup).await?;
    let native = owner.selected_policy().native_host.as_ref().ok_or(HostLifetimeError::InvalidSelection)?;
    let (installed, mut acquisition) = prepare_gateway_acquisition(&selection, native, installed)?;
    if let Some(acquisition) = &mut acquisition { acquisition.selected_policy = participant; }
    Ok((owner, installed, acquisition))
}

pub(crate) async fn open_gateway_owner(
    selection: HostBootstrapSelection, startup: toolkit_mobkit::installed_host::NativeOwnerStartup,
) -> Result<(meerkat_toolkit::host_lifetime::NativeHostLifetime, InstalledToolkitHost,
    Option<Arc<SelectedPolicyBinding>>), HostLifetimeError> {
    let selected = meerkat_toolkit::host_lifetime::policy::validate_candidate_selection(&selection)?.0;
    let native = selected.native_host.as_ref().ok_or(HostLifetimeError::InvalidSelection)?;
    let acquisition = read_application_acquisition(native, |reference| read_bundle_artifact(&selection, reference))?;
    let participant = acquisition.as_ref().and_then(|row| row.config.standing_policy).map(|choices| SelectedPolicyBinding::new(choices,
            native.grant_administrator.clone(), selected.deployment.clone(), native.realm.clone(),
            native.members.iter().map(|row| row.member.member.clone()).collect()));
    let recovery_participant = participant.as_ref().map(|binding| binding.participant()).or_else(||
        acquisition.as_ref().filter(|row| row.prepared.helper_tools.requires_software_review()).map(|_|
            SelectedPolicyBinding::human_review_participant(native.grant_administrator.clone(),
                selected.deployment.clone(), native.realm.clone(),
                native.members.iter().map(|row| row.member.member.clone()).collect())));
    let (owner, installed) = match recovery_participant {
        Some(recovery_participant) => InstalledToolkitHost::open_with_application_service_policy(
            selection.clone(), startup, None, recovery_participant).await?,
        None => InstalledToolkitHost::open(selection.clone(), startup).await?,
    };
    Ok((owner, installed, participant))
}

/// Called only after InstalledToolkitHost::open authenticated this exact bundle.
/// Absence keeps ordinary gateway behavior; a malformed selected bundle refuses.
/// Native durable-backend absence is handled later by the existing optional
/// bootstrap owner and is observable through acquisition status.
pub fn prepare_gateway_acquisition(
    selection: &HostBootstrapSelection,
    native: &CompiledNativeHost,
    installed: InstalledToolkitHost,
) -> Result<(InstalledToolkitHost, Option<PreparedGatewayAcquisition>), HostLifetimeError> {
    prepare_gateway_acquisition_with_retained(selection, native, installed, None)
}

pub(crate) fn prepare_gateway_acquisition_with_retained(
    selection: &HostBootstrapSelection, native: &CompiledNativeHost, installed: InstalledToolkitHost,
    retained: Option<(toolkit_mobkit::installed_host::acquisition::InstalledAcquisitionTools, Arc<InstalledNativeContinuation>)>,
) -> Result<(InstalledToolkitHost, Option<PreparedGatewayAcquisition>), HostLifetimeError> {
    prepare_gateway_acquisition_selected(selection, native, installed, retained, None)
}

pub(crate) fn prepare_gateway_acquisition_update(
    selection: &HostBootstrapSelection, native: &CompiledNativeHost, installed: InstalledToolkitHost,
    update: &crate::application_update::ValidatedReplacementStartup,
    participant: Option<Arc<SelectedPolicyBinding>>,
) -> Result<(InstalledToolkitHost, Option<PreparedGatewayAcquisition>), HostLifetimeError> {
    let (installed, mut acquisition) = prepare_gateway_acquisition_selected(selection, native, installed, None, Some(update))?;
    if let Some(acquisition) = &mut acquisition { acquisition.selected_policy = participant; }
    Ok((installed, acquisition))
}

fn prepare_gateway_acquisition_selected(
    selection: &HostBootstrapSelection, native: &CompiledNativeHost, installed: InstalledToolkitHost,
    retained: Option<(toolkit_mobkit::installed_host::acquisition::InstalledAcquisitionTools, Arc<InstalledNativeContinuation>)>,
    update: Option<&crate::application_update::ValidatedReplacementStartup>,
) -> Result<(InstalledToolkitHost, Option<PreparedGatewayAcquisition>), HostLifetimeError> {
    let Some(SelectedApplicationAcquisition { config, prepared, prepared_bytes, helper }) =
        read_application_acquisition(native, |reference| read_bundle_artifact(selection, reference))?
    else {
        if retained.is_some() || update.is_some() { return Err(HostLifetimeError::InvalidSelection); }
        return Ok((installed, None));
    };
    let resource = if retained.is_none() && prepared.schema == 4 {
        Some(Arc::new(crate::application_setup::resource::SetupResourceSelection::from_completed(&prepared)
            .map_err(|_| HostLifetimeError::InvalidSelection)?))
    } else { None };
    let storage = prepared.realm.resolve().map_err(|_| HostLifetimeError::InvalidSelection)?;
    let expected_state_root = storage.layout.state_root().to_path_buf();
    let realm = storage.locator.clone();
    let helper_member = helper.member().clone();
    let original_definition = match &prepared.definition_source {
        ApplicationSetupDefinition::Toml { text } => meerkat_mob::MobDefinition::from_toml(text)
            .map_err(|_| HostLifetimeError::InvalidSelection)?,
        ApplicationSetupDefinition::Native { definition } => definition.clone(),
    };
    let expected_definition = update.map(|row| row.update.definition().clone()).unwrap_or_else(|| prepared.definition.clone());
    let (carrier, bytes) = match update {
        Some(row) => (row.carrier.clone(), row.carrier_bytes.clone()),
        None => (config.prepared_application_setup, prepared_bytes),
    };
    let application = PreparedSetupApplication::from_verified_artifact(
        carrier, bytes, expected_definition.clone(),
        Arc::new(PreparedApplicationRoster::new(prepared.clone(), expected_definition.clone())),
    )?;
    let replacement = update.is_some();
    let projection: SetupBootstrapProjection = Arc::new(move |spec| {
        // Never replace an unrelated incoming definition or resurrect a frozen
        // roster. The early carrier installs PreparedApplicationRoster on the
        // native wrapper around the application's existing live provider.
        if (spec.definition != expected_definition && (replacement || spec.definition != original_definition))
            || spec.resolved_storage.as_ref().and_then(|summary| summary.state_dir.as_ref())
                != Some(&expected_state_root)
        { return Err(HostLifetimeError::InvalidSelection); }
        let spec = helper.compose(spec).map_err(|_| HostLifetimeError::InvalidSelection)?;
        if spec.definition != expected_definition { return Err(HostLifetimeError::InvalidSelection); }
        Ok(spec)
    });
    let installed = installed.with_service_installation_readback(Arc::new(
        crate::resources::installation_readback::ProductInstallationReadback { storage },
    ))?;
    let (continuation, tools) = match retained {
        Some((tools, continuation)) => (continuation, Some(tools)),
        None => (installed.native_continuation(config.output_audience)?, None),
    };
    let defaults = InstalledSetupBootstrap {
        helper: prepared.helper,
        proposal_ceiling: config.proposal_ceiling,
        continuation: continuation.clone(),
        backend_generation: config.backend_generation,
        references: None,
        limits: config.limits,
        work_step: WorkAdmissionStepOptions {
            worker: Id::new("toolkit-setup").map_err(|_| HostLifetimeError::InvalidSelection)?,
            lease_ms: 30_000, owner_timeout_ms: 10_000, settlement_budget_ms: 5_000,
        },
    };
    let installed = match tools {
        Some(tools) => installed.with_enrolled_bootstrap_acquisition(defaults, tools)?,
        None => installed.with_bootstrap_acquisition(defaults)?,
    };
    Ok((installed, Some(PreparedGatewayAcquisition { continuation, projection, application, helper_member, helper_tools: prepared.helper_tools, standing_policy: config.standing_policy, selected_policy: None, acquisition_reference: native.setup.clone().ok_or(HostLifetimeError::InvalidSelection)?, realm, console_setup: config.console_setup, local_operator_recovery: config.local_operator_recovery, resource })))
}

/// Native calls this with each actual provider result, including refresh and
/// reset. It reads no artifact and never substitutes a retained roster snapshot.
pub(crate) struct PreparedApplicationRoster {
    prepared: PreparedApplicationSetup, definition: meerkat_mob::MobDefinition,
}
impl PreparedApplicationRoster {
    pub(crate) fn new(prepared: PreparedApplicationSetup, definition: meerkat_mob::MobDefinition) -> Self {
        Self { prepared, definition }
    }
}
impl meerkat_mobkit::identity_first::contracts::RosterProjection for PreparedApplicationRoster {
    fn project(
        &self,
        context: &meerkat_mobkit::identity_first::RosterContext,
        roster: Vec<meerkat_mobkit::identity_first::DurableAgentSpec>,
    ) -> Result<Vec<meerkat_mobkit::identity_first::DurableAgentSpec>, String> {
        if context.mob_definition.as_ref().is_some_and(|definition| definition != &self.definition) {
            return Err("prepared application definition changed".into());
        }
        self.prepared.project_roster(&roster)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn id(value: &str) -> Id { Id::new(value).unwrap() }
    #[tokio::test]
    async fn checked_installation_requires_the_same_open_presenter_channel() {
        let directory = tempfile::tempdir().unwrap();
        for tools in [ApplicationSetupTools::Preparation, ApplicationSetupTools::InertAcquisition] {
            assert!(require_setup_review(tools, None).is_ok());
        }
        let selected = ApplicationSetupTools::CheckedInstallation;
        assert!(matches!(require_setup_review(selected, None), Err(HostLifetimeError::InvalidSelection)));
        let (presenters, reviews) = crate::application_gateway::setup_presenter::channel(directory.path().to_owned());
        assert!(require_setup_review(selected, Some(&presenters)).is_ok());
        let retained = presenters.clone();
        drop(reviews);
        assert!(matches!(require_setup_review(selected, Some(&retained)), Err(HostLifetimeError::Unavailable)));
        assert!(matches!(require_setup_review(selected, Some(&presenters)), Err(HostLifetimeError::Unavailable)));
        // This is the consumed composition guard with a real enrollment channel.
        // It does not claim a native gateway start or a granted install effect.
    }

    #[test]
    fn selected_output_audience_cannot_exceed_compiled_proposal_ceiling() {
        let mut selected = ApplicationAcquisitionSelection {
            schema: 1, console_setup: None, local_operator_recovery: None, standing_policy: None,
            prepared_application_setup: CompiledArtifactReference { name: "application-setup.json".into(), sha256: "a".repeat(64) },
            backend_generation: id("selected-backend"),
            proposal_ceiling: Ceiling { actions: BTreeSet::new(), services: BTreeSet::new(), frontends: BTreeSet::new(), output_audiences: [id("requester")].into() },
            output_audience: [id("requester")].into(), limits: InstalledSetupLimits::default(),
        };
        assert!(selected.validate().is_ok());
        selected.output_audience.insert(id("other"));
        assert!(selected.validate().is_err());
        selected.output_audience.clear();
        assert!(selected.validate().is_err());
    }

    #[test]
    fn absent_standing_policy_preserves_wire_and_selected_choices_match_exactly() {
        use selected_policy::{StandingPolicySelection, InstallationSelection, AccessSelection};
        let parameters = ApplicationAcquisitionParameters { standing_policy: None, console_setup: None, local_operator_recovery: None,
            backend_generation: id("selected-backend"),
            proposal_ceiling: Ceiling { actions: BTreeSet::new(), services: BTreeSet::new(),
                frontends: BTreeSet::new(), output_audiences: [id("requester")].into() },
            output_audience: [id("requester")].into(), limits: InstalledSetupLimits::default() };
        let reference = CompiledArtifactReference { name: "application-setup.json".into(), sha256: "a".repeat(64) };
        let selected = parameters.clone().complete(reference);
        for value in [serde_json::to_value(&parameters).unwrap(), serde_json::to_value(&selected).unwrap()] {
            assert!(value.get("standing_policy").is_none());
        }
        // The old documents omit the new field. Decoding and re-encoding them
        // preserves their exact canonical form, including the existing nulls.
        let parameter_bytes = toolkit_package::canonical_json(&parameters).unwrap();
        let decoded: ApplicationAcquisitionParameters = serde_json::from_slice(&parameter_bytes).unwrap();
        assert_eq!(toolkit_package::canonical_json(&decoded).unwrap(), parameter_bytes);
        let selected_bytes = toolkit_package::canonical_json(&selected).unwrap();
        let decoded: ApplicationAcquisitionSelection = serde_json::from_slice(&selected_bytes).unwrap();
        assert_eq!(toolkit_package::canonical_json(&decoded).unwrap(), selected_bytes);
        assert!(parameters.matches(&decoded).unwrap());

        let mut expected = parameters;
        expected.standing_policy = Some(StandingPolicySelection {
            installation: InstallationSelection::CheckedSoftware, access: AccessSelection::ConfiguredParent, tool_policy: None });
        let mut selected = expected.clone().complete(selected.prepared_application_setup);
        assert!(expected.matches(&selected).unwrap());
        for access in [AccessSelection::HumanReview, AccessSelection::ProvisionTaskParent] {
            selected.standing_policy.as_mut().unwrap().access = access;
            assert!(!expected.matches(&selected).unwrap());
        }
        let mut provisioning = expected.clone();
        provisioning.standing_policy.as_mut().unwrap().access = AccessSelection::ProvisionTaskParent;
        assert!(provisioning.matches(&selected).unwrap());
        let wire = serde_json::to_value(&provisioning).unwrap();
        assert_eq!(wire["standing_policy"]["access"], "provision_task_parent");
        assert!(wire["standing_policy"].get("service_parent_administration").is_none());
        let decoded: ApplicationAcquisitionParameters = serde_json::from_value(wire.clone()).unwrap();
        assert!(decoded.matches(&selected).unwrap());
        // An application selection cannot carry an administrative rule or actor.
        for (field, value) in [("service_parent_administration", serde_json::json!({"routes":["any"]})),
            ("remote_operator", serde_json::json!("operator"))] {
            let mut injected = wire.clone();
            injected["standing_policy"][field] = value;
            assert!(serde_json::from_value::<ApplicationAcquisitionParameters>(injected).is_err());
        }
        let mut unsupported = wire;
        unsupported["standing_policy"]["access"] = serde_json::json!("automatic_admin");
        assert!(serde_json::from_value::<ApplicationAcquisitionParameters>(unsupported).is_err());
        selected.standing_policy = None;
        assert!(!expected.matches(&selected).unwrap());
        let mut unknown = serde_json::to_value(&expected).unwrap();
        unknown["standing_policy"]["installation"] = serde_json::json!("agent_allow");
        assert!(serde_json::from_value::<ApplicationAcquisitionParameters>(unknown).is_err());
        let mut extra = serde_json::to_value(&expected).unwrap();
        extra["standing_policy"]["actor"] = serde_json::json!("untrusted");
        assert!(serde_json::from_value::<ApplicationAcquisitionParameters>(extra).is_err());
    }

    #[test]
    fn local_operator_recovery_is_explicit_pinned_and_preserves_absent_wire() {
        use toolkit_mobkit::host_gateway::LocalOperatorRecoveryPolicy;
        let mut parameters = ApplicationAcquisitionParameters { standing_policy: None, console_setup: None, local_operator_recovery: None,
            backend_generation: id("selected-backend"),
            proposal_ceiling: Ceiling { actions: BTreeSet::new(), services: BTreeSet::new(),
                frontends: BTreeSet::new(), output_audiences: [id("requester")].into() },
            output_audience: [id("requester")].into(), limits: InstalledSetupLimits::default() };
        let reference = CompiledArtifactReference { name: "application-setup.json".into(), sha256: "a".repeat(64) };
        let absent = parameters.clone().complete(reference.clone());
        let old_parameters = toolkit_package::canonical_json(&parameters).unwrap();
        let old_selected = toolkit_package::canonical_json(&absent).unwrap();
        assert!(serde_json::to_value(&parameters).unwrap().get("local_operator_recovery").is_none());
        assert!(serde_json::to_value(&absent).unwrap().get("local_operator_recovery").is_none());
        let decoded: ApplicationAcquisitionParameters = serde_json::from_slice(&old_parameters).unwrap();
        assert_eq!(toolkit_package::canonical_json(&decoded).unwrap(), old_parameters);
        let decoded: ApplicationAcquisitionSelection = serde_json::from_slice(&old_selected).unwrap();
        assert_eq!(toolkit_package::canonical_json(&decoded).unwrap(), old_selected);
        assert!(parameters.matches(&decoded).unwrap());
        parameters.local_operator_recovery = Some(LocalOperatorRecoveryPolicy::UnpublishedServiceApprovals);
        let selected = parameters.clone().complete(reference);
        assert!(parameters.matches(&selected).unwrap());
        assert!(!parameters.matches(&absent).unwrap());
        let value = serde_json::to_value(&selected).unwrap();
        assert_eq!(value["local_operator_recovery"], "unpublished_service_approvals");
        assert!(value.get("console_setup").is_none(), "local selection grants no Console policy");
        for unsupported in [serde_json::json!("automatic_startup"), serde_json::json!({"principal":"remote-user"})] {
            let mut injected = value.clone(); injected["local_operator_recovery"] = unsupported;
            assert!(serde_json::from_value::<ApplicationAcquisitionSelection>(injected).is_err());
        }
    }

}
