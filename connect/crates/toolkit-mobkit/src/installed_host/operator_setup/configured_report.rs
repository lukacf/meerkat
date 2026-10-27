//! Trusted stopped-host configuration of one separate helper report parent.
use meerkat_authorization_contracts::constraints::DomainRestriction;
use super::*;
use crate::installed_host::acquisition::{ConfiguredHelperReportCapability, InstalledSetupSelection};
use meerkat_authorization::grants::GrantRefusal;
use meerkat_authorization_contracts::{
    constraints::{ActionRef, AudienceRef, DelegationDepth, ExactRestriction, LifetimeRestriction, ProcessorRef},
};

/// Retained by the trusted installer across uncertain replies. The selected
/// artifact supplies the helper destination; the request cannot override it.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HelperReportCapabilityRequest {
    pub member: meerkat_core::MobMemberBinding,
    pub issuance_id: EvidenceId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_issuance_id: Option<EvidenceId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_issuance_id: Option<EvidenceId>,
}

impl OperatorSetup {
    /// Restore the existing native pair inside the application's stopped-host
    /// transaction. The application reader supplies its exact selected helper
    /// artifact from this same candidate. No runtime or session is started.
    /// The receipt is input to the existing helper compilation and publication
    /// transaction; it never mutates or activates the selected configuration.
    pub fn configure_application_helper_report(
        selection: &HostBootstrapSelection,
        request: &HelperReportCapabilityRequest,
        select_helper: impl FnOnce(&CompiledNativeHost) -> Result<InstalledSetupSelection, HostLifetimeError>,
    ) -> Result<ConfiguredHelperReportCapability, HostLifetimeError> {
        NativeHostLifetime::with_candidate_installation(selection, |policy| {
            let native = policy.native_host.as_ref().ok_or(HostLifetimeError::Unavailable)?;
            let operator = native.remote_operator.as_ref().ok_or(HostLifetimeError::Unavailable)?;
            let _validated = validate_installed_artifacts(native, |reference| {
                read_bundle_artifact(selection, reference)
            })?;
            if !distinct_ids(request) {
                return Err(HostLifetimeError::InvalidSelection);
            }
            let helper = select_helper(native)?;
            let member = helper.selected_member(native)?;
            if helper.member != request.member
                || ConfiguredHelperReportCapability::validate_basis(&helper.operation_basis).is_err()
            { return Err(HostLifetimeError::Denied); }
            // Absence of report_capability is intentional for first issuance.
            // Existing controller and target-operation selections are unchanged.
            let expected = report_parent(&native.grant_administrator, &helper.logical_executor, &member.audience);
            let (source, originals) = open_owner_pair(selection, native, operator, OperatorOwnerStartup::Restore)?;
            let result = issue_or_recover(&source, &native.grant_administrator, operator, &helper, request, expected, &member.audience);
            drop(originals);
            drop(source);
            result
        })
    }
}

pub(crate) fn report_parent(application: &meerkat_core::PrincipalRef, executor: &meerkat_core::PrincipalRef, audience: &AudienceRef) -> ExecutionRestrictions {
    operation_parent(application, executor, audience, "setup-report")
}

pub(crate) fn account_parent(application: &meerkat_core::PrincipalRef, executor: &meerkat_core::PrincipalRef, audience: &AudienceRef) -> ExecutionRestrictions {
    operation_parent(application, executor, audience, "setup-account-request")
}

pub(crate) fn public_parent(application: &meerkat_core::PrincipalRef, executor: &meerkat_core::PrincipalRef, audience: &AudienceRef) -> ExecutionRestrictions {
    operation_parent(application, executor, audience, "setup-public-service-request")
}

fn distinct_ids(request: &HelperReportCapabilityRequest) -> bool {
    request.account_issuance_id.as_ref() != Some(&request.issuance_id)
        && request.public_issuance_id.as_ref() != Some(&request.issuance_id)
        && request.account_issuance_id.as_ref().is_none_or(|id| request.public_issuance_id.as_ref() != Some(id))
}

fn operation_parent(application: &meerkat_core::PrincipalRef, executor: &meerkat_core::PrincipalRef, audience: &AudienceRef, action: &str) -> ExecutionRestrictions {
    ExecutionRestrictions {
        actions: ExactRestriction::exact([ActionRef { feature: "toolkit".into(), action: action.into() }]),
        // The originating request does not exist during configuration. This
        // parent covers only the selected application's domains for this action
        // and destination. The child narrows to its qualified origin later.
        resource_domains: DomainRestriction::owned_by([application.clone()]),
        processors: ExactRestriction::exact([ProcessorRef::Principal { principal: executor.clone() }]),
        audiences: ExactRestriction::exact([audience.clone()]),
        lifetime: LifetimeRestriction::unrestricted(),
        delegation_depth: DelegationDepth::remaining(1),
    }
}

pub(crate) fn issue_or_recover(
    source: &ToolkitOriginalOwner, application: &meerkat_core::PrincipalRef, operator: &meerkat_core::PrincipalRef,
    helper: &InstalledSetupSelection, request: &HelperReportCapabilityRequest, expected: ExecutionRestrictions,
    audience: &AudienceRef,
) -> Result<ConfiguredHelperReportCapability, HostLifetimeError> {
    if !distinct_ids(request) {
        return Err(HostLifetimeError::InvalidSelection);
    }
    let grants = source.grants();
    let operation = || {
        let issue = |id: &EvidenceId, expected: &ExecutionRestrictions| {
        let parent = match grants.issued_reference(id)? {
            Some(parent) => parent,
            None => grants.issue_root(operator, id.clone(), application.clone(),
                helper.represented_subject.clone(), expected.clone())?,
        };
        if &parent.root_authority != operator { return Err(GrantRefusal::Denied); }
        // Controller-compatible resolution is required by the guarded native
        // child facade, including unrestricted lifetime on every ancestor.
        let resolved = grants.resolve_controller_lineage(std::slice::from_ref(&parent), application,
            helper.represented_subject.as_ref())?;
        if resolved.restrictions() != expected { return Err(GrantRefusal::Denied); }
        resolved.check_current().map_err(|_| GrantRefusal::Unavailable)?;
        Ok::<_, GrantRefusal>(parent)
        };
        let parent = issue(&request.issuance_id, &expected)?;
        let account_parent = request.account_issuance_id.as_ref().map(|id| {
            issue(id, &account_parent(application, &helper.logical_executor, audience))
                .map(|parent| vec![parent])
        }).transpose()?;
        let public_parent = request.public_issuance_id.as_ref().map(|id| {
            issue(id, &public_parent(application, &helper.logical_executor, audience))
                .map(|parent| vec![parent])
        }).transpose()?;
        Ok(ConfiguredHelperReportCapability { parent: vec![parent], account_parent, public_parent })
    };
    operation().map_err(|error| match error {
        GrantRefusal::Denied | GrantRefusal::Duplicate | GrantRefusal::ControllerInUse => HostLifetimeError::Denied,
        GrantRefusal::Unavailable => HostLifetimeError::Unavailable,
    })
}
