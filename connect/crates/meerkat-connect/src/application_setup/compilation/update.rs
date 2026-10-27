//! Read-only preconstruction validation for the application's existing update
//! transaction. No replacement surface artifacts or completed candidate are
//! required. The trusted caller retains the exact package/source closure and
//! lease; this module does not establish authorization or currentness.
use super::{ApplicationSetupCompilationInput, ValidatedApplicationCompilation};
use crate::{
    application_acquisition::{ApplicationAcquisitionSelection, SelectedApplicationAcquisition, read_application_acquisition},
    application_bootstrap::{InstallerProgress, emit, hash, read_json},
    application_setup::{PreparedApplicationSetup, read_application_input},
};
use meerkat_core::CompiledApplicationToolPolicy;
use meerkat_mob::MobDefinition;
use meerkat_mobkit::{identity_first::DurableAgentSpec, member_tool_policy::providers_from_canonical_payloads};
use meerkat_toolkit::host_lifetime::{
    control::{HostBootstrapSelection, SelectedHostArtifact},
    policy::{PreparedLocalPolicy, validate_candidate_selection},
};
use serde::{Deserialize, Serialize};
use toolkit_mobkit::installed_host::{read_bundle_artifact, validate_installed_artifacts};

/// Byte selection within the existing application-owned update journal. The
/// update ID correlates that journal; it is neither authority nor a retry key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApplicationUpdateSelection {
    pub update_id: String,
    pub predecessor: HostBootstrapSelection,
    pub compilation_input: SelectedHostArtifact,
    pub application_tool_policy: SelectedHostArtifact,
}

/// Immutable validated inputs for a later unheld consumer. There is no public
/// constructor or serialization, and no native construction/finish token.
/// Recheck the real owner's currentness before any subsequent effect. Native
/// provider acceptance and tracked reload/refresh outcomes remain separate.
pub struct ValidatedApplicationUpdate {
    selection: ApplicationUpdateSelection,
    compilation: ValidatedApplicationCompilation,
    original: SelectedApplicationAcquisition,
    owner: PreparedLocalPolicy,
    definition: MobDefinition,
    roster: Vec<DurableAgentSpec>,
    policy: CompiledApplicationToolPolicy,
}

fn read_pin(pin: &SelectedHostArtifact) -> Result<Vec<u8>, String> {
    let bytes = read_application_input(&pin.path)?;
    if hash(&bytes) != pin.sha256 { return Err("selected update artifact changed".into()); }
    Ok(bytes)
}

impl ApplicationUpdateSelection {
    /// Validate the original completed bundle and candidate inputs only. This
    /// first in-place slice retains the same member/authority inventory, realm
    /// roots, helper resource, complete helper roster row and provider/policy
    /// slot. It cannot add members, replace native grant references, reprepare
    /// setup, adopt a candidate or move current. Tool-policy grants may change.
    pub fn validate(self) -> Result<ValidatedApplicationUpdate, String> {
        if self.update_id.is_empty() { return Err("update ID is empty".into()); }
        let input: ApplicationSetupCompilationInput = serde_json::from_slice(&read_pin(&self.compilation_input)?)
            .map_err(|error| error.to_string())?;
        let compilation = input.validate()?;
        let policy_bytes = read_pin(&self.application_tool_policy)?;
        if policy_bytes != compilation.policy_bytes {
            return Err("selected update policy differs from its exact compiler input".into());
        }
        let policy = CompiledApplicationToolPolicy::parse_canonical_json(&policy_bytes)
            .map_err(|error| error.to_string())?;

        // This authenticates selected bytes, not the live current pointer. The
        // caller's original update transaction must supply the predecessor.
        let (previous, _) = validate_candidate_selection(&self.predecessor)
            .map_err(|error| error.to_string())?;
        let native = previous.native_host.as_ref().ok_or("predecessor has no native selection")?;
        validate_installed_artifacts(native, |reference| read_bundle_artifact(&self.predecessor, reference))
            .map_err(|error| error.to_string())?;
        let original = read_application_acquisition(native, |reference| read_bundle_artifact(&self.predecessor, reference))
            .map_err(|error| error.to_string())?.ok_or("predecessor has no retained setup selection")?;
        let prepared = original.prepared();
        let owner = prepared.native_owner.as_ref().ok_or("update requires the original schema4 owner selection")?;
        if !owner.matches(&previous) {
            return Err("predecessor setup differs from its exact owner and source routes".into());
        }
        let old_policy_bytes = read_bundle_artifact(&self.predecessor, &native.application_tool_policy)
            .map_err(|error| error.to_string())?;
        let old_policy = CompiledApplicationToolPolicy::parse_canonical_json(&old_policy_bytes)
            .map_err(|error| error.to_string())?;
        if policy.provider_id != old_policy.provider_id || policy.policy_id != old_policy.policy_id {
            return Err("update requires the original provider and policy slot".into());
        }
        // Reuse the actual provider's predecessor revision/digest fence in a
        // disposable, worker-free instance. This is not acceptance by the live
        // provider, whose current snapshot may already be newer. No construction
        // or member is pinned to this revision; bound calls use live snapshots.
        providers_from_canonical_payloads([old_policy_bytes.as_slice(), policy_bytes.as_slice()])
            .map_err(|error| error.to_string())?;

        let input = &compilation.input;
        let encode = |value: &super::super::ApplicationSetupRealm| toolkit_package::canonical_json(value)
            .map_err(|error| error.to_string());
        if encode(&input.realm)? != encode(&prepared.realm)?
            || input.helper.member != prepared.source_helper_member
            || input.helper_profile != prepared.helper_profile
            || toolkit_package::canonical_json(&input.helper_tools).map_err(|error| error.to_string())?
                != toolkit_package::canonical_json(&prepared.helper_tools).map_err(|error| error.to_string())?
        {
            return Err("update changes the original realm or setup resource selection".into());
        }
        // Reuse the exact retained preparer result. No new resource owner or
        // policy registry is constructed, and absent grants are never filled.
        let projection = prepared.helper_projection()?;
        let mut helper = input.helper.clone();
        helper.member = projection.member().clone();
        if helper != prepared.helper {
            return Err("update changes the original helper authority or request selection".into());
        }
        let mut members = input.members.clone();
        members[compilation.helper_index].member = projection.member().clone();
        if members.len() != owner.native_host.members.len()
            || members.iter().any(|member| !owner.native_host.members.contains(member))
        {
            return Err("update changes the original member destination or authority selection".into());
        }
        let roster = prepared.project_roster(&input.roster)?;
        let mut definition = compilation.definition.clone();
        projection.compose_definition(&mut definition).map_err(|error| error.to_string())?;
        let owner = owner.clone();
        Ok(ValidatedApplicationUpdate { selection: self, compilation, original, owner,
            definition, roster, policy })
    }
}

impl ValidatedApplicationUpdate {
    pub fn selection(&self) -> &ApplicationUpdateSelection { &self.selection }
    pub fn compilation_input(&self) -> &ApplicationSetupCompilationInput { &self.compilation.input }
    /// Definition and initial roster with the original helper resource projected
    /// once. Every later actual roster read still needs original_setup().project_roster.
    pub fn definition(&self) -> &MobDefinition { &self.definition }
    pub fn roster(&self) -> &[DurableAgentSpec] { &self.roster }
    pub fn original_owner(&self) -> &PreparedLocalPolicy { &self.owner }
    pub fn original_setup(&self) -> &PreparedApplicationSetup { self.original.prepared() }
    pub fn original_acquisition(&self) -> &ApplicationAcquisitionSelection { self.original.configuration() }
    pub fn tool_policy(&self) -> &CompiledApplicationToolPolicy { &self.policy }
    pub fn tool_policy_bytes(&self) -> &[u8] { &self.compilation.policy_bytes }
}

fn receipt(args: &[String], progress: &mut InstallerProgress) -> Result<serde_json::Value, String> {
    let [_, flag, path] = args else {
        return Err("expected --validate-application-update --selection FILE".into());
    };
    if flag != "--selection" { return Err("invalid update validation arguments".into()); }
    progress.phase = "validation";
    let selected: ApplicationUpdateSelection = read_json(path)?;
    let validated = selected.validate()?;
    progress.phase = "receipt";
    // An exact echo for the trusted installer caller, not an authorization,
    // currentness, provider acceptance, publication or Ready receipt.
    Ok(serde_json::json!({"kind":"application_update_validated","selection":validated.selection()}))
}

pub(crate) fn command(args: &[String], progress: &mut InstallerProgress) -> Result<(), String> {
    emit(&receipt(args, progress)?)
}

#[cfg(test)]
mod tests;
