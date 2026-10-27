//! Compile the selected helper's upper bound from the application's selected artifact.
//! Actual visible subsets and current native grants remain native-owned.
use super::*;
use meerkat_core::{CompiledApplicationToolPolicy, CompiledMemberToolAction, ToolIdentity};
use meerkat_toolkit::setup::SETUP_REPORT;
use toolkit_mobkit::installed_host::acquisition::setup_report_definition;

pub(super) const OUTPUT_NAME: &str = "helper-initial-surface.json";

pub(super) fn require_source_selection(
    selected: &SelectedHostArtifact,
    reference: &CompiledArtifactReference,
) -> Result<(), String> {
    reference.validate().map_err(|error| error.to_string())?;
    if selected.sha256 != reference.sha256
        || selected.path.file_name().and_then(|name| name.to_str()) != Some(reference.name.as_str())
    {
        return Err("helper surface source does not match its selected artifact reference".into());
    }
    Ok(())
}

pub(super) fn require_exclusive_output_name(
    members: &[CompiledNativeMember],
    selected: usize,
) -> Result<(), String> {
    if members
        .iter()
        .enumerate()
        .any(|(index, member)| index != selected && member.initial_surface.name == OUTPUT_NAME)
    {
        return Err("helper surface output name belongs to another member".into());
    }
    Ok(())
}

pub(super) fn compile(selected: &SelectedHostArtifact) -> Result<Vec<u8>, String> {
    let bytes = read_regular_artifact(&selected.path)?;
    if hash(&bytes) != selected.sha256 {
        return Err("selected helper surface artifact changed".into());
    }
    let mut identities: Vec<ToolIdentity> =
        serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    if toolkit_package::canonical_json(&identities).map_err(|error| error.to_string())? != bytes
        || identities.iter().enumerate().any(|(index, identity)| {
            identities[..index]
                .iter()
                .any(|prior| prior.name == identity.name)
        })
    {
        return Err("helper surface is not canonical or contains duplicate tools".into());
    }
    // Use the same definition as the actual registered result dispatcher.
    // Identity is full native name/provenance, not an independently copied name.
    let report = setup_report_definition().identity();
    if let Some(existing) = identities
        .iter()
        .find(|identity| identity.name == report.name)
    {
        if existing != &report {
            return Err("helper surface has conflicting setup-report provenance".into());
        }
    } else {
        identities.push(report);
    }
    toolkit_package::canonical_json(&identities).map_err(|error| error.to_string())
}

pub(super) fn reference(bytes: &[u8]) -> CompiledArtifactReference {
    CompiledArtifactReference {
        name: OUTPUT_NAME.into(),
        sha256: hash(bytes),
    }
}

pub(super) fn require_report_policy(
    policy: &ApplicationToolPolicyInput,
    member: &MobMemberBinding,
) -> Result<(), String> {
    require_report_grants(&policy.members, member)
}

pub(super) fn require_compiled_report_policy(
    bytes: &[u8],
    member: &MobMemberBinding,
) -> Result<(), String> {
    let policy = CompiledApplicationToolPolicy::parse_canonical_json(bytes)
        .map_err(|error| error.to_string())?;
    require_report_grants(&policy.members, member)
}

fn require_report_grants(
    members: &[meerkat_core::CompiledMemberToolGrants],
    selected: &MobMemberBinding,
) -> Result<(), String> {
    let mut matching = members
        .iter()
        .filter(|member| member.member_identity == selected.member);
    let member = matching
        .next()
        .ok_or("helper has no explicit tool policy")?;
    if matching.next().is_some()
        || member
            .grants
            .iter()
            .filter(|grant| {
                grant.tool_name == SETUP_REPORT && grant.action == CompiledMemberToolAction::Invoke
            })
            .count()
            != 1
    {
        return Err("helper policy must explicitly select setup_report invocation".into());
    }
    // The application-selected consequence and native operation basis are not
    // rewritten. The member tool allowance is not a native operation grant.
    Ok(())
}
