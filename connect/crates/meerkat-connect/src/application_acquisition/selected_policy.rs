//! Explicit provisioning choices from the trusted application's selected setup.
//! Native authority and the real administrative producer remain separate.
use crate::approval::installation_policy::{
    ApplicationInstallationPolicy, InstallationPolicyDecision, InstallationPolicyRequest,
    InstallationPolicyReservation, InstalledInstallationPolicy,
};
use meerkat_core::{
    ApprovalDecision, ApprovalStatus, CompiledApplicationToolPolicy, PolicyDigest, PrincipalRef,
    RealmId,
};
use meerkat_toolkit::host_lifetime::{
    NativeHostLifetime,
    control::HostLifetimeError,
    policy::{CompiledArtifactReference, CompiledMemberClass},
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::{
    collections::BTreeSet,
    sync::{Arc, OnceLock},
};
use toolkit_mobkit::installed_host::dynamic_members::{
    MemberClassAdmission, MemberClassAdmissionPhase,
};
use toolkit_mobkit::private_originals::{
    ApplicationServicePolicy, ApplicationServicePolicyDecision, ApplicationServicePolicyRequest,
    ApplicationServicePolicyReservation, ApplicationToolPolicyRecoveryDecision,
    ApplicationToolPolicyRecoveryRequest, InstalledApplicationServicePolicy, MemberPolicyLifetime,
    ServiceToolPolicyApproval, ServiceToolPolicyProposal,
};
use toolkit_package::Id;

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
pub enum InstallationSelection {
    HumanReview,
    CheckedSoftware,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessSelection {
    HumanReview,
    ConfiguredParent,
    ProvisionTaskParent,
}

/// Same participant before preparation, during canonical recovery and after
/// adoption. It has no early lifetime or grant and cannot authorize live work
/// before bind. Recovery rechecks explicit standing scope or the canonical
/// owner's exact retained broader human decision.
pub struct SelectedPolicyBinding {
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
    classes: Vec<CompiledMemberClass>,
    installed: OnceLock<InstalledFacts>,
}
struct InstalledFacts {
    source_policy_sha256: String,
    reference: CompiledArtifactReference,
}
struct PolicyFacts {
    scope: Arc<PolicyScope>,
}
impl SelectedPolicyBinding {
    /// Retain an explicit trusted application choice from the actual pending
    /// owner. This participant stays unavailable until bind authenticates the
    /// same deployment, realm, administrator, members and final setup artifact.
    pub fn from_preparation(
        owner: &toolkit_mobkit::installed_host::preparation::PreparedInstallationOwner,
        selection: StandingPolicySelection,
    ) -> Arc<Self> {
        let selected = owner.selected();
        let native = &selected.native_host;
        Self::new(
            selection,
            native.grant_administrator.clone(),
            selected.deployment.clone(),
            native.realm.clone(),
            native
                .members
                .iter()
                .map(|row| row.member.member.clone())
                .collect(),
        )
    }
    pub(crate) fn new(
        selection: StandingPolicySelection,
        owner: PrincipalRef,
        deployment: Id,
        realm: RealmId,
        members: BTreeSet<String>,
    ) -> Arc<Self> {
        Self::new_with_member_classes(selection, owner, deployment, realm, members, Vec::new())
    }
    pub(crate) fn new_with_member_classes(
        selection: StandingPolicySelection,
        owner: PrincipalRef,
        deployment: Id,
        realm: RealmId,
        members: BTreeSet<String>,
        classes: Vec<CompiledMemberClass>,
    ) -> Arc<Self> {
        Arc::new(Self {
            facts: Arc::new(PolicyFacts {
                scope: Arc::new(PolicyScope {
                    selection: Some(selection),
                    owner,
                    deployment,
                    realm,
                    members,
                    classes,
                    installed: OnceLock::new(),
                }),
            }),
            bound: OnceLock::new(),
        })
    }
    /// Default checked setup retains only recovery of a real broader human
    /// decision. This is not a standing selection or an installed grant owner.
    #[cfg(test)]
    pub(crate) fn human_review_participant(
        owner: PrincipalRef,
        deployment: Id,
        realm: RealmId,
        members: BTreeSet<String>,
    ) -> Arc<dyn ApplicationServicePolicy> {
        Self::human_review_participant_with_member_classes(
            owner,
            deployment,
            realm,
            members,
            Vec::new(),
        )
    }
    pub(crate) fn human_review_participant_with_member_classes(
        owner: PrincipalRef,
        deployment: Id,
        realm: RealmId,
        members: BTreeSet<String>,
        classes: Vec<CompiledMemberClass>,
    ) -> Arc<dyn ApplicationServicePolicy> {
        Arc::new(PolicyFacts {
            scope: Arc::new(PolicyScope {
                selection: None,
                owner,
                deployment,
                realm,
                members,
                classes,
                installed: OnceLock::new(),
            }),
        })
    }
    pub fn participant(&self) -> Arc<dyn ApplicationServicePolicy> {
        self.facts.clone()
    }
    pub(crate) fn owner(&self) -> &PrincipalRef {
        &self.facts.scope.owner
    }
    #[cfg(test)]
    pub(crate) fn bind(
        &self,
        lifetime: Arc<NativeHostLifetime>,
        selection: StandingPolicySelection,
        reference: &CompiledArtifactReference,
    ) -> Result<(), HostLifetimeError> {
        self.bind_with_service_parent(lifetime, selection, reference, None)
    }
    pub fn bind_with_service_parent(
        &self,
        lifetime: Arc<NativeHostLifetime>,
        selection: StandingPolicySelection,
        reference: &CompiledArtifactReference,
        service_parent: Option<toolkit_mobkit::private_host::ServiceParentEndpoint>,
    ) -> Result<(), HostLifetimeError> {
        lifetime.validate_current_selection()?;
        if service_parent
            .as_ref()
            .is_some_and(|endpoint| !endpoint.belongs_to(&lifetime))
        {
            return Err(HostLifetimeError::WrongOwner);
        }
        let selected = lifetime.selected_policy();
        let native = selected
            .native_host
            .as_ref()
            .ok_or(HostLifetimeError::InvalidSelection)?;
        let scope = &self.facts.scope;
        let members = native
            .members
            .iter()
            .map(|row| row.member.member.clone())
            .collect::<BTreeSet<_>>();
        if Some(selection) != scope.selection
            || selected.deployment != scope.deployment
            || native.grant_administrator != scope.owner
            || native.realm != scope.realm
            || members != scope.members
            || native.member_classes != scope.classes
            || native.setup.as_ref() != Some(reference)
            || self.bound.get().is_some()
        {
            return Err(HostLifetimeError::WrongOwner);
        }
        scope
            .installed
            .set(InstalledFacts {
                source_policy_sha256: selected.source_policy_sha256.clone(),
                reference: reference.clone(),
            })
            .map_err(|_| HostLifetimeError::WrongOwner)?;
        let installation = match selection.installation {
            InstallationSelection::HumanReview => None,
            InstallationSelection::CheckedSoftware => Some(InstalledInstallationPolicy::new(
                lifetime.clone(),
                self.facts.clone(),
            )?),
        };
        let service = match selection.access {
            AccessSelection::HumanReview => None,
            AccessSelection::ConfiguredParent => Some(Arc::new(
                InstalledApplicationServicePolicy::new(lifetime.clone(), self.participant())?,
            )),
            AccessSelection::ProvisionTaskParent => Some(Arc::new(
                InstalledApplicationServicePolicy::with_task_parent_provisioning(
                    lifetime.clone(),
                    self.participant(),
                    Arc::new(
                        service_parent
                            .clone()
                            .ok_or(HostLifetimeError::Unavailable)?,
                    ),
                )?,
            )),
        };
        self.bound
            .set(BoundPolicies {
                lifetime,
                installation,
                service,
                service_parent,
            })
            .map_err(|_| HostLifetimeError::WrongOwner)
    }
    fn bound(&self) -> Result<&BoundPolicies, HostLifetimeError> {
        self.bound.get().ok_or(HostLifetimeError::Unavailable)
    }
    pub(crate) fn installation(
        &self,
    ) -> Result<Option<InstalledInstallationPolicy>, HostLifetimeError> {
        let bound = self.bound()?;
        bound.lifetime.validate_current_selection()?;
        bound.lifetime.validate_native_entry(None)?;
        Ok(bound.installation.clone())
    }
    pub(crate) fn service_parent(
        &self,
    ) -> Result<Option<toolkit_mobkit::private_host::ServiceParentEndpoint>, HostLifetimeError>
    {
        let bound = self.bound()?;
        bound.lifetime.validate_current_selection()?;
        bound.lifetime.validate_native_entry(None)?;
        if self
            .facts
            .scope
            .selection
            .is_some_and(|selection| selection.access == AccessSelection::ProvisionTaskParent)
        {
            Ok(Some(
                bound
                    .service_parent
                    .clone()
                    .ok_or(HostLifetimeError::Unavailable)?,
            ))
        } else {
            Ok(None)
        }
    }
    pub(crate) fn service(
        &self,
    ) -> Result<Option<Arc<InstalledApplicationServicePolicy>>, HostLifetimeError> {
        let bound = self.bound()?;
        bound.lifetime.validate_current_selection()?;
        bound.lifetime.validate_native_entry(None)?;
        Ok(bound.service.clone())
    }
}
impl PolicyScope {
    fn check(
        &self,
        owner: &PrincipalRef,
        deployment: &Id,
        source: &str,
    ) -> Result<(), HostLifetimeError> {
        let installed = self.installed.get().ok_or(HostLifetimeError::Unavailable)?;
        if owner != &self.owner
            || deployment != &self.deployment
            || source != installed.source_policy_sha256
        {
            return Err(HostLifetimeError::WrongOwner);
        }
        Ok(())
    }
    fn proposal(
        &self,
        proposal: &ServiceToolPolicyProposal,
        canonical: &[u8],
        admissions: &[MemberClassAdmission],
        historical: bool,
    ) -> Result<(), HostLifetimeError> {
        if proposal.lifetime != MemberPolicyLifetime::UntilSuperseded
            || !self.members.contains(&proposal.member)
            || canonical != proposal.candidate_canonical_json.as_bytes()
        {
            return Err(HostLifetimeError::Denied);
        }
        let compiled = CompiledApplicationToolPolicy::parse_canonical_json(canonical)
            .map_err(|_| HostLifetimeError::InvalidSelection)?;
        if compiled.provider_id != proposal.provider_id
            || compiled.policy_id != proposal.policy_id
            || compiled.revision != proposal.candidate.revision
            || PolicyDigest::from_canonical_bytes(canonical) != proposal.candidate.digest
        {
            return Err(HostLifetimeError::WrongOwner);
        }
        let members = compiled
            .members
            .iter()
            .map(|row| row.member_identity.clone())
            .collect::<BTreeSet<_>>();
        if !self.members.is_subset(&members) {
            return Err(HostLifetimeError::WrongOwner);
        }
        if !historical
            && admissions.iter().any(|admission| {
                matches!(
                    &admission.phase,
                    MemberClassAdmissionPhase::Prepared { .. }
                        | MemberClassAdmissionPhase::Active { .. }
                ) && !members.contains(&admission.instance.member.member.member)
            })
        {
            return Err(HostLifetimeError::WrongOwner);
        }
        let mut matched = BTreeSet::new();
        for row in compiled
            .members
            .iter()
            .filter(|row| !self.members.contains(&row.member_identity))
        {
            let mut candidates = admissions
                .iter()
                .filter(|admission| admission.instance.member.member.member == row.member_identity);
            let admission = candidates.next().ok_or(HostLifetimeError::WrongOwner)?;
            if candidates.next().is_some()
                || !matched.insert(admission.id.clone())
                || !self.classes.contains(&admission.class)
                || admission.instance.member.setup_capability.is_some()
                || admission
                    .class
                    .materialize(&self.realm, &admission.instance)?
                    != *row
            {
                return Err(HostLifetimeError::WrongOwner);
            }
            // This is preservation evidence from the canonical owner, never
            // permission to add or operate an instance. Fresh proposals only
            // contain prepared/active rows. Recovery checks exact saved bytes,
            // so retirement or a new reservation can leave that exact row in
            // the old decision while the current provider omits it. The
            // canonical owner validates that narrowing and any pending
            // reinstatement; this reservation does not authorize member use.
            match &admission.phase {
                MemberClassAdmissionPhase::Prepared { .. }
                | MemberClassAdmissionPhase::Active { .. } => {}
                MemberClassAdmissionPhase::Reserved | MemberClassAdmissionPhase::Retired
                    if historical => {}
                _ => return Err(HostLifetimeError::Denied),
            }
        }
        Ok(())
    }
    fn reserve(self: &Arc<Self>, domain: &str) -> SelectedReservation {
        let mut provenance = Map::from_iter([
            ("kind".into(), json!("selected_application_setup_policy")),
            ("domain".into(), json!(domain)),
            ("selection".into(), json!(self.selection)),
        ]);
        if let Some(installed) = self.installed.get() {
            provenance.insert("setup".into(), json!(installed.reference));
        }
        SelectedReservation {
            facts: self.clone(),
            provenance,
        }
    }
}
impl ApplicationInstallationPolicy for PolicyFacts {
    fn reserve(
        &self,
        request: &InstallationPolicyRequest<'_>,
    ) -> Result<InstallationPolicyDecision, HostLifetimeError> {
        self.scope.check(
            request.application,
            request.deployment,
            request.source_policy_sha256,
        )?;
        if request.realm != &self.scope.realm
            || !crate::service_install::INSTALL_TOOLS.contains(&request.call.name)
        {
            return Err(HostLifetimeError::WrongOwner);
        }
        match self.scope.selection.map(|selection| selection.installation) {
            Some(InstallationSelection::CheckedSoftware) => Ok(InstallationPolicyDecision::Permit(
                Box::new(self.scope.reserve("installation")),
            )),
            _ => Ok(InstallationPolicyDecision::RequiresHuman),
        }
    }
}
impl ApplicationServicePolicy for PolicyFacts {
    fn reserve_member_policy(
        &self,
        request: &toolkit_mobkit::installed_host::dynamic_members::ApplicationMemberPolicyRequest<
            '_,
        >,
    ) -> Result<ApplicationToolPolicyRecoveryDecision, HostLifetimeError> {
        use toolkit_mobkit::installed_host::dynamic_members::MemberClassAdmissionPhase;
        let admission = request.admission;
        // The authenticated package selected these finite class declarations.
        // Installation/access review preferences confer no class authority.
        if request.owner != &self.scope.owner
            || request.realm != &self.scope.realm
            || request.deployment != self.scope.deployment.as_str()
            || !self.scope.classes.contains(&admission.class)
            || self
                .scope
                .members
                .contains(&admission.instance.member.member.member)
            || admission.instance.member.setup_capability.is_some()
        {
            return Err(HostLifetimeError::WrongOwner);
        }
        let expected = admission
            .class
            .materialize(request.realm, &admission.instance)?;
        let canonical =
            CompiledApplicationToolPolicy::parse_canonical_json(request.canonical_policy)
                .map_err(|_| HostLifetimeError::InvalidSelection)?;
        let actual = canonical
            .members
            .iter()
            .find(|row| row.member_identity == expected.member_identity);
        match &admission.phase {
            MemberClassAdmissionPhase::Reserved | MemberClassAdmissionPhase::Retired
                if actual.is_none() => {}
            MemberClassAdmissionPhase::Prepared { .. }
            | MemberClassAdmissionPhase::Active { .. }
                if actual == Some(&expected) => {}
            _ => return Err(HostLifetimeError::Denied),
        }
        Ok(ApplicationToolPolicyRecoveryDecision::Permit(Box::new(
            self.scope.reserve("member_class"),
        )))
    }
    fn reserve(
        &self,
        request: &ApplicationServicePolicyRequest<'_>,
    ) -> Result<ApplicationServicePolicyDecision, HostLifetimeError> {
        let Some(selection) = self.scope.selection else {
            return Ok(ApplicationServicePolicyDecision::RequiresHuman);
        };
        self.scope.check(
            request.owner,
            request.deployment,
            request.source_policy_sha256,
        )?;
        if selection.access == AccessSelection::HumanReview {
            return Ok(ApplicationServicePolicyDecision::RequiresHuman);
        }
        if let Some(proposal) = request.tool_policy {
            if selection.tool_policy.is_none() {
                return Ok(ApplicationServicePolicyDecision::RequiresHuman);
            }
            self.scope.proposal(
                proposal,
                proposal.candidate_canonical_json.as_bytes(),
                request.member_admissions,
                false,
            )?;
            return Ok(ApplicationServicePolicyDecision::PermitWithToolPolicy {
                reservation: Box::new(self.scope.reserve("service_and_member_tool_policy")),
                approval: Box::new(ServiceToolPolicyApproval::until_superseded(proposal)?),
            });
        }
        Ok(ApplicationServicePolicyDecision::Permit(Box::new(
            self.scope.reserve("service_access"),
        )))
    }
    fn reserve_tool_policy_recovery(
        &self,
        request: &ApplicationToolPolicyRecoveryRequest<'_>,
    ) -> Result<ApplicationToolPolicyRecoveryDecision, HostLifetimeError> {
        let decision = request.decision;
        if decision.owner != self.scope.owner
            || decision.realm != self.scope.realm.as_str()
            || decision.deployment != self.scope.deployment.as_str()
        {
            return Err(HostLifetimeError::WrongOwner);
        }
        let standing = self
            .scope
            .selection
            .and_then(|selection| selection.tool_policy)
            == Some(MemberToolPolicySelection::UntilSuperseded);
        if !standing && !exact_human_policy_decision(decision)? {
            return Ok(ApplicationToolPolicyRecoveryDecision::RequiresHuman);
        }
        self.scope.proposal(
            &decision.proposal,
            request.canonical_json,
            request.member_admissions,
            true,
        )?;
        Ok(ApplicationToolPolicyRecoveryDecision::Permit(Box::new(
            self.scope.reserve("member_tool_policy_recovery"),
        )))
    }
}
fn exact_human_policy_decision(
    decision: &toolkit_mobkit::private_originals::ActiveMemberToolPolicyDecision,
) -> Result<bool, HostLifetimeError> {
    let Some(record) = &decision.human_review else {
        return Ok(false);
    };
    if record.status != ApprovalStatus::Approved
        || !record
            .decision
            .as_ref()
            .is_some_and(|answer| answer.decision == ApprovalDecision::Approve)
        || decision.approval_id != decision.proposal.sha256()?
    {
        return Ok(false);
    }
    let expected = serde_json::to_value(&decision.proposal)
        .map_err(|_| HostLifetimeError::InvalidSelection)?;
    // The canonical owner authenticated this exact operator record before
    // retaining it. A historical service-only review has no member-policy body.
    // Its old task/login deadline is not the UntilSuperseded member lifetime.
    Ok(record
        .proposed_action
        .body
        .as_ref()
        .and_then(|body| body.get("access_request"))
        .and_then(|body| body.get("member_tool_policy"))
        .and_then(|body| body.get("proposal"))
        == Some(&expected))
}
/// The trusted finite setup choice is immutable for this owner. The reservation
/// retains that exclusion; native owner, slot and currentness checks stay outside
/// it. Recovery never manufactures a final lifetime or a task grant.
struct SelectedReservation {
    facts: Arc<PolicyScope>,
    provenance: Map<String, Value>,
}
impl InstallationPolicyReservation for SelectedReservation {
    fn owner(&self) -> &PrincipalRef {
        &self.facts.owner
    }
    fn provenance(&self) -> &Map<String, Value> {
        &self.provenance
    }
    fn check_current(&self) -> Result<(), HostLifetimeError> {
        Ok(())
    }
}
impl ApplicationServicePolicyReservation for SelectedReservation {
    fn owner(&self) -> &PrincipalRef {
        &self.facts.owner
    }
    fn provenance(&self) -> &Map<String, Value> {
        &self.provenance
    }
    fn check_current(&self) -> Result<(), HostLifetimeError> {
        Ok(())
    }
}

#[cfg(test)]
mod member_class_tests;
