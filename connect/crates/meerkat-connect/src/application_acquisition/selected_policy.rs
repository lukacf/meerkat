//! Explicit provisioning choices from the trusted application's selected setup.
//! Native authority and the real administrative producer remain separate.
use std::{collections::BTreeSet, sync::{Arc, OnceLock}};
use meerkat_core::{ApprovalDecision, ApprovalStatus, CompiledApplicationToolPolicy, PolicyDigest, PrincipalRef, RealmId};
use meerkat_toolkit::host_lifetime::{NativeHostLifetime, control::HostLifetimeError,
    policy::CompiledArtifactReference};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use toolkit_package::Id;
use crate::approval::installation_policy::{ApplicationInstallationPolicy,
    InstallationPolicyDecision, InstallationPolicyRequest, InstallationPolicyReservation,
    InstalledInstallationPolicy};
use toolkit_mobkit::private_originals::{ApplicationServicePolicy, ApplicationServicePolicyDecision,
    ApplicationServicePolicyRequest, ApplicationServicePolicyReservation, InstalledApplicationServicePolicy,
    ApplicationToolPolicyRecoveryRequest, ApplicationToolPolicyRecoveryDecision,
    ServiceToolPolicyApproval, ServiceToolPolicyProposal, MemberPolicyLifetime};

/// Trusted unattended choices, pinned by native_host.setup. Omission grants no
/// automatic persistent scope; an actual broader human decision stays separate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StandingPolicySelection {
    pub installation: InstallationSelection,
    pub access: AccessSelection,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_policy: Option<MemberToolPolicySelection>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemberToolPolicySelection {
    /// Authorize each exact complete successor for this configured member set.
    /// Preserved grants are part of that decision. Ordinary Invoke persists
    /// beyond task expiry; service and catalog lineage keeps its task deadline.
    UntilSuperseded,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallationSelection { HumanReview, CheckedSoftware }
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessSelection { HumanReview, ConfiguredParent, ProvisionTaskParent }

/// Same participant before preparation, during canonical recovery and after
/// adoption. It has no early lifetime or grant and cannot authorize live work
/// before bind. Recovery rechecks explicit standing scope or the canonical
/// owner's exact retained broader human decision.
pub(crate) struct SelectedPolicyBinding {
    facts: Arc<PolicyFacts>,
    bound: OnceLock<BoundPolicies>,
}
struct BoundPolicies {
    lifetime: Arc<NativeHostLifetime>,
    installation: Option<InstalledInstallationPolicy>,
    service: Option<Arc<InstalledApplicationServicePolicy>>,
    service_parent: Option<toolkit_mobkit::private_host::ServiceParentEndpoint>,
}
struct PolicyScope {
    selection: Option<StandingPolicySelection>,
    owner: PrincipalRef,
    deployment: Id,
    realm: RealmId,
    members: BTreeSet<String>,
    installed: OnceLock<InstalledFacts>,
}
struct InstalledFacts { source_policy_sha256: String, reference: CompiledArtifactReference }
struct PolicyFacts { scope: Arc<PolicyScope> }
impl SelectedPolicyBinding {
    pub(crate) fn new(selection: StandingPolicySelection, owner: PrincipalRef,
        deployment: Id, realm: RealmId, members: BTreeSet<String>) -> Arc<Self> {
        Arc::new(Self { facts: Arc::new(PolicyFacts { scope: Arc::new(PolicyScope {
            selection: Some(selection), owner, deployment, realm, members, installed: OnceLock::new(),
        }) }), bound: OnceLock::new() })
    }
    /// Default checked setup retains only recovery of a real broader human
    /// decision. This is not a standing selection or an installed grant owner.
    pub(crate) fn human_review_participant(owner: PrincipalRef, deployment: Id,
        realm: RealmId, members: BTreeSet<String>) -> Arc<dyn ApplicationServicePolicy> {
        Arc::new(PolicyFacts { scope: Arc::new(PolicyScope { selection: None,
            owner, deployment, realm, members, installed: OnceLock::new() }) })
    }
    pub(crate) fn participant(&self) -> Arc<dyn ApplicationServicePolicy> { self.facts.clone() }
    pub(crate) fn owner(&self) -> &PrincipalRef { &self.facts.scope.owner }
    pub(crate) fn bind(&self, lifetime: Arc<NativeHostLifetime>,
        selection: StandingPolicySelection, reference: &CompiledArtifactReference)
        -> Result<(), HostLifetimeError> {
        self.bind_with_service_parent(lifetime, selection, reference, None)
    }
    pub(crate) fn bind_with_service_parent(&self, lifetime: Arc<NativeHostLifetime>,
        selection: StandingPolicySelection, reference: &CompiledArtifactReference,
        service_parent: Option<toolkit_mobkit::private_host::ServiceParentEndpoint>,
    ) -> Result<(), HostLifetimeError> {
        lifetime.validate_current_selection()?;
        if service_parent.as_ref().is_some_and(|endpoint| !endpoint.belongs_to(&lifetime)) {
            return Err(HostLifetimeError::WrongOwner);
        }
        let selected = lifetime.selected_policy();
        let native = selected.native_host.as_ref().ok_or(HostLifetimeError::InvalidSelection)?;
        let scope = &self.facts.scope;
        let members = native.members.iter().map(|row| row.member.member.clone()).collect::<BTreeSet<_>>();
        if Some(selection) != scope.selection || selected.deployment != scope.deployment
            || native.grant_administrator != scope.owner || native.realm != scope.realm
            || members != scope.members || native.setup.as_ref() != Some(reference)
            || self.bound.get().is_some()
        { return Err(HostLifetimeError::WrongOwner); }
        scope.installed.set(InstalledFacts { source_policy_sha256: selected.source_policy_sha256.clone(),
            reference: reference.clone() }).map_err(|_| HostLifetimeError::WrongOwner)?;
        let installation = match selection.installation {
            InstallationSelection::HumanReview => None,
            InstallationSelection::CheckedSoftware => Some(InstalledInstallationPolicy::new(
                lifetime.clone(), self.facts.clone())?),
        };
        let service = match selection.access {
            AccessSelection::HumanReview => None,
            AccessSelection::ConfiguredParent => Some(Arc::new(InstalledApplicationServicePolicy::new(
                lifetime.clone(), self.participant())?)),
            AccessSelection::ProvisionTaskParent => Some(Arc::new(
                InstalledApplicationServicePolicy::with_task_parent_provisioning(
                    lifetime.clone(), self.participant(), Arc::new(service_parent.clone()
                        .ok_or(HostLifetimeError::Unavailable)?))?)),
        };
        self.bound.set(BoundPolicies { lifetime, installation, service, service_parent })
            .map_err(|_| HostLifetimeError::WrongOwner)
    }
    fn bound(&self) -> Result<&BoundPolicies, HostLifetimeError> {
        self.bound.get().ok_or(HostLifetimeError::Unavailable)
    }
    pub(crate) fn installation(&self) -> Result<Option<InstalledInstallationPolicy>, HostLifetimeError> {
        let bound = self.bound()?;
        bound.lifetime.validate_current_selection()?;
        bound.lifetime.validate_native_entry(None)?;
        Ok(bound.installation.clone())
    }
    pub(crate) fn service_parent(&self) -> Result<Option<toolkit_mobkit::private_host::ServiceParentEndpoint>, HostLifetimeError> {
        let bound = self.bound()?;
        bound.lifetime.validate_current_selection()?;
        bound.lifetime.validate_native_entry(None)?;
        if self.facts.scope.selection.is_some_and(|selection| selection.access == AccessSelection::ProvisionTaskParent) {
            Ok(Some(bound.service_parent.clone().ok_or(HostLifetimeError::Unavailable)?))
        } else { Ok(None) }
    }
    pub(crate) fn service(&self) -> Result<Option<Arc<InstalledApplicationServicePolicy>>, HostLifetimeError> {
        let bound = self.bound()?;
        bound.lifetime.validate_current_selection()?;
        bound.lifetime.validate_native_entry(None)?;
        Ok(bound.service.clone())
    }
}
impl PolicyScope {
    fn check(&self, owner: &PrincipalRef, deployment: &Id, source: &str) -> Result<(), HostLifetimeError> {
        let installed = self.installed.get().ok_or(HostLifetimeError::Unavailable)?;
        if owner != &self.owner || deployment != &self.deployment || source != installed.source_policy_sha256 {
            return Err(HostLifetimeError::WrongOwner);
        }
        Ok(())
    }
    fn proposal(&self, proposal: &ServiceToolPolicyProposal, canonical: &[u8]) -> Result<(), HostLifetimeError> {
        if proposal.lifetime != MemberPolicyLifetime::UntilSuperseded
            || !self.members.contains(&proposal.member)
            || canonical != proposal.candidate_canonical_json.as_bytes()
        { return Err(HostLifetimeError::Denied); }
        let compiled = CompiledApplicationToolPolicy::parse_canonical_json(canonical)
            .map_err(|_| HostLifetimeError::InvalidSelection)?;
        if compiled.provider_id != proposal.provider_id || compiled.policy_id != proposal.policy_id
            || compiled.revision != proposal.candidate.revision
            || PolicyDigest::from_canonical_bytes(canonical) != proposal.candidate.digest
            || compiled.members.iter().map(|row| row.member_identity.clone()).collect::<BTreeSet<_>>() != self.members
        { return Err(HostLifetimeError::WrongOwner); }
        Ok(())
    }
    fn reserve(self: &Arc<Self>, domain: &str) -> SelectedReservation {
        let mut provenance = Map::from_iter([
            ("kind".into(), json!("selected_application_setup_policy")),
            ("domain".into(), json!(domain)), ("selection".into(), json!(self.selection)),
        ]);
        if let Some(installed) = self.installed.get() { provenance.insert("setup".into(), json!(installed.reference)); }
        SelectedReservation { facts: self.clone(), provenance }
    }
}
impl ApplicationInstallationPolicy for PolicyFacts {
    fn reserve(&self, request: &InstallationPolicyRequest<'_>) -> Result<InstallationPolicyDecision, HostLifetimeError> {
        self.scope.check(request.application, request.deployment, request.source_policy_sha256)?;
        if request.realm != &self.scope.realm || !crate::service_install::INSTALL_TOOLS.contains(&request.call.name) {
            return Err(HostLifetimeError::WrongOwner);
        }
        match self.scope.selection.map(|selection| selection.installation) {
            Some(InstallationSelection::CheckedSoftware) => Ok(InstallationPolicyDecision::Permit(Box::new(self.scope.reserve("installation")))),
            _ => Ok(InstallationPolicyDecision::RequiresHuman),
        }
    }
}
impl ApplicationServicePolicy for PolicyFacts {
    fn reserve(&self, request: &ApplicationServicePolicyRequest<'_>) -> Result<ApplicationServicePolicyDecision, HostLifetimeError> {
        let Some(selection) = self.scope.selection else { return Ok(ApplicationServicePolicyDecision::RequiresHuman); };
        self.scope.check(request.owner, request.deployment, request.source_policy_sha256)?;
        if selection.access == AccessSelection::HumanReview { return Ok(ApplicationServicePolicyDecision::RequiresHuman); }
        if let Some(proposal) = request.tool_policy {
            if selection.tool_policy.is_none() { return Ok(ApplicationServicePolicyDecision::RequiresHuman); }
            self.scope.proposal(proposal, proposal.candidate_canonical_json.as_bytes())?;
            return Ok(ApplicationServicePolicyDecision::PermitWithToolPolicy {
                reservation: Box::new(self.scope.reserve("service_and_member_tool_policy")),
                approval: ServiceToolPolicyApproval::until_superseded(proposal)?,
            });
        }
        Ok(ApplicationServicePolicyDecision::Permit(Box::new(self.scope.reserve("service_access"))))
    }
    fn reserve_tool_policy_recovery(&self, request: &ApplicationToolPolicyRecoveryRequest<'_>)
        -> Result<ApplicationToolPolicyRecoveryDecision, HostLifetimeError> {
        let decision = request.decision;
        if decision.owner != self.scope.owner || decision.realm != self.scope.realm.as_str()
            || decision.deployment != self.scope.deployment.as_str()
        { return Err(HostLifetimeError::WrongOwner); }
        let standing = self.scope.selection.and_then(|selection| selection.tool_policy)
            == Some(MemberToolPolicySelection::UntilSuperseded);
        if !standing && !exact_human_policy_decision(decision)? {
            return Ok(ApplicationToolPolicyRecoveryDecision::RequiresHuman);
        }
        self.scope.proposal(&decision.proposal, request.canonical_json)?;
        Ok(ApplicationToolPolicyRecoveryDecision::Permit(Box::new(self.scope.reserve("member_tool_policy_recovery"))))
    }
}
fn exact_human_policy_decision(decision: &toolkit_mobkit::private_originals::ActiveMemberToolPolicyDecision)
    -> Result<bool, HostLifetimeError> {
    let Some(record) = &decision.human_review else { return Ok(false); };
    if record.status != ApprovalStatus::Approved
        || !record.decision.as_ref().is_some_and(|answer| answer.decision == ApprovalDecision::Approve)
        || decision.approval_id != decision.proposal.sha256()?
    { return Ok(false); }
    let expected = serde_json::to_value(&decision.proposal).map_err(|_| HostLifetimeError::InvalidSelection)?;
    // The canonical owner authenticated this exact operator record before
    // retaining it. A historical service-only review has no member-policy body.
    // Its old task/login deadline is not the UntilSuperseded member lifetime.
    Ok(record.proposed_action.body.as_ref()
        .and_then(|body| body.get("access_request"))
        .and_then(|body| body.get("member_tool_policy"))
        .and_then(|body| body.get("proposal")) == Some(&expected))
}
/// The trusted finite setup choice is immutable for this owner. The reservation
/// retains that exclusion; native owner, slot and currentness checks stay outside
/// it. Recovery never manufactures a final lifetime or a task grant.
struct SelectedReservation { facts: Arc<PolicyScope>, provenance: Map<String, Value> }
impl InstallationPolicyReservation for SelectedReservation {
    fn owner(&self) -> &PrincipalRef { &self.facts.owner }
    fn provenance(&self) -> &Map<String, Value> { &self.provenance }
    fn check_current(&self) -> Result<(), HostLifetimeError> { Ok(()) }
}
impl ApplicationServicePolicyReservation for SelectedReservation {
    fn owner(&self) -> &PrincipalRef { &self.facts.owner }
    fn provenance(&self) -> &Map<String, Value> { &self.provenance }
    fn check_current(&self) -> Result<(), HostLifetimeError> { Ok(()) }
}
