//! The existing configured setup owner supplies fresh children to one reviewed
//! service task. The canonical source retains the selection before first bind.
use super::*;
use crate::installed_host::acquisition::InstalledSetupSelection;
use meerkat_authorization_contracts::constraints::{
    AudienceRef, ExactRestriction, LifetimeRestriction, ProcessorRef,
};
use meerkat_authorization_contracts::work_association::{GrantTarget, TargetGrantLineage};
use meerkat_core::{MobMemberBinding, SessionId};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ServiceSetupHelper {
    pub member: MobMemberBinding,
    pub logical_executor: PrincipalRef,
    pub audience: AudienceRef,
    pub session_id: SessionId,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ServiceSetupSelection {
    pub configured: ConfiguredSetupSelection,
    pub helper: ServiceSetupHelper,
    pub expires_at_ms: u64,
}

impl ServiceSetupSelection {
    pub(crate) fn validate(&self) -> Result<(), PrivateOriginalError> {
        crate::installed_host::operator_setup::setup_targets::validate_compiled(
            &self.configured.capability.helper_targets,
        )
        .map_err(|_| PrivateOriginalError::Conflict)?;
        if self.expires_at_ms == 0
            || self.helper.session_id == self.configured.session_id
            || !self
                .configured
                .capability
                .helper_targets
                .iter()
                .any(|target| target.target.is_none())
            || self
                .configured
                .capability
                .helper_targets
                .iter()
                .any(|target| {
                    target.restrictions.processors
                        != ExactRestriction::exact([ProcessorRef::Principal {
                            principal: self.helper.logical_executor.clone(),
                        }])
                        || target.restrictions.audiences
                            != ExactRestriction::exact([self.helper.audience.clone()])
                })
        {
            return Err(PrivateOriginalError::Conflict);
        }
        Ok(())
    }

    pub(in crate::private_originals) fn binding_targets(
        &self,
    ) -> Result<Vec<GrantTarget>, PrivateOriginalError> {
        self.validate()?;
        let mut targets = vec![crate::installed_host::operator_setup::setup_targets::find_target()];
        targets.extend(
            self.configured
                .capability
                .helper_targets
                .iter()
                .filter_map(|target| target.target.clone()),
        );
        Ok(targets)
    }

    fn targets(&self) -> Vec<meerkat_toolkit::host_lifetime::policy::CompiledAgentSetupTarget> {
        // CatalogControl has one base. Its newly reviewed 2x2 catalog replaces
        // the configured helper catalog explicitly; no unused catalog is issued.
        self.configured
            .capability
            .helper_targets
            .iter()
            .filter(|target| target.target.is_some())
            .cloned()
            .collect()
    }
}

impl PrivateOriginals {
    pub(in crate::private_originals) fn select_service_setup(
        &self,
        parent: &PrivateOriginalRef,
        session: &SessionId,
        before_ms: u64,
        helper: &InstalledSetupSelection,
        helper_member: &CompiledNativeMember,
        helper_session: &SessionId,
    ) -> Result<Option<ServiceSetupSelection>, PrivateOriginalError> {
        let facts = self.configured_setup(parent)?;
        let Some(capability) = facts.selection.clone() else {
            return Ok(None);
        };
        // Older configured find/report selections do not select the catalog
        // permission needed by the warm path. Preserve their public review.
        if !capability
            .helper_targets
            .iter()
            .any(|target| target.target.is_none())
        {
            return Ok(None);
        }
        if !self.configured_parent_current(&facts, &capability.parent)? {
            return Ok(None);
        }
        if helper.member != helper_member.member
            || helper.logical_executor != helper_member.logical_owner
        {
            return Err(PrivateOriginalError::Denied);
        }
        crate::installed_host::operator_setup::setup_targets::validate_compiled(
            &capability.helper_targets,
        )
        .map_err(|_| PrivateOriginalError::Conflict)?;
        // The existing configured owner selects contributor targets for the
        // helper audience. Validate that exact contract before deciding whether
        // the Original and helper can share the optional warm metadata scope.
        if capability.helper_targets.iter().any(|target| {
            target.restrictions.processors
                != ExactRestriction::exact([ProcessorRef::Principal {
                    principal: helper.logical_executor.clone(),
                }])
                || target.restrictions.audiences
                    != ExactRestriction::exact([helper_member.audience.clone()])
        }) {
            return Err(PrivateOriginalError::Conflict);
        }
        if !supports_warm_metadata(
            &facts.route.logical_executor,
            &facts.member.audience,
            &helper_member.logical_owner,
            &helper_member.audience,
        ) {
            return Ok(None);
        }
        let selected = ServiceSetupSelection {
            configured: ConfiguredSetupSelection {
                capability,
                session_id: session.clone(),
            },
            helper: ServiceSetupHelper {
                member: helper.member.clone(),
                logical_executor: helper.logical_executor.clone(),
                audience: helper_member.audience.clone(),
                session_id: helper_session.clone(),
            },
            expires_at_ms: before_ms,
        };
        selected.validate()?;
        self.service_setup_facts(parent, &selected)?;
        Ok(Some(selected))
    }

    fn service_setup_facts(
        &self,
        reference: &PrivateOriginalRef,
        selected: &ServiceSetupSelection,
    ) -> Result<ConfiguredSetup, PrivateOriginalError> {
        selected.validate()?;
        let facts = self.configured_setup(reference)?;
        let native = facts
            .lifetime
            .selected_policy()
            .native_host
            .as_ref()
            .ok_or(PrivateOriginalError::Unavailable)?;
        let helper = native
            .members
            .iter()
            .find(|member| member.member == selected.helper.member)
            .ok_or(PrivateOriginalError::Denied)?;
        if facts.selection.as_ref() != Some(&selected.configured.capability)
            || selected.configured.capability.route != reference.route
            || helper.logical_owner != selected.helper.logical_executor
            || helper.audience != selected.helper.audience
            || helper.audience != facts.member.audience
            || helper.logical_owner == facts.route.logical_executor
        {
            return Err(PrivateOriginalError::Conflict);
        }
        Ok(facts)
    }

    /// A child is observed only after native resolution proves its exact
    /// parent, grantee and attenuation. The existing source is the durable
    /// owner; these observations never authorize input admission.
    fn service_setup_children(
        &self,
        reference: &PrivateOriginalRef,
        selected: &ServiceSetupSelection,
    ) -> Result<(ConfiguredSetup, Vec<ServiceSetupChild>), PrivateOriginalError> {
        let facts = self.service_setup_facts(reference, selected)?;
        let id = self.configured_child_id(reference)?;
        let origin = self.setup_origin_for(reference)?;
        let mut requested = facts.child_bounds(&selected.configured.session_id);
        requested.lifetime = LifetimeRestriction::window(0, selected.expires_at_ms);
        let parent_bounds = facts.parent_bounds();
        let mut children = vec![ServiceSetupChild::new(
            crate::installed_host::operator_setup::setup_targets::find_target(),
            selected.configured.capability.parent.clone(),
            id.clone(),
            parent_bounds,
            requested,
        )?];
        for target in selected.targets() {
            let child = super::super::setup_targets::child_id(&id, &target.target)
                .map_err(|_| PrivateOriginalError::Conflict)?;
            let mut requested = crate::installed_host::operator_setup::setup_targets::child_bounds(
                &facts.application,
                &origin,
                &target.target,
                &target.restrictions,
            )
            .map_err(|_| PrivateOriginalError::Conflict)?;
            requested.lifetime = LifetimeRestriction::window(0, selected.expires_at_ms);
            children.push(ServiceSetupChild::new(
                target.target.ok_or(PrivateOriginalError::Conflict)?,
                target.parent,
                child,
                target.restrictions,
                requested,
            )?);
        }
        Ok((facts, children))
    }

    /// The source writer spans issuance and each observation CAS. Every native
    /// call is synchronous and the login reservation survives observation
    /// writes. Drop/write loss remains uncertain, with the intent still stored.
    pub(in crate::private_originals) async fn issue_service_setup(
        &self,
        reference: &PrivateOriginalRef,
        selected: &ServiceSetupSelection,
        login: Option<&crate::private_host::ConsoleLoginHandle>,
    ) -> Result<(), PrivateOriginalError> {
        let _writer = self
            .owner
            .try_source_writer()
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        let mut stored = self
            .store
            .get(&self.key(reference)?)
            .await
            .map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        let mut record = self.decode_record(&stored, reference)?;
        if record.operation_cancelled
            || record.approved_task.as_ref().and_then(|task| task.setup()) != Some(selected)
        {
            return Err(PrivateOriginalError::Denied);
        }
        if record.final_association.is_some() {
            self.service_setup_targets(reference, selected, false)?;
            return Ok(());
        }
        if record.bound_service_input.is_some() || record.service_admission.is_some() {
            return Err(PrivateOriginalError::Unavailable);
        }
        let (facts, children) = self.service_setup_children(reference, selected)?;
        self.check_service_setup_observations(&record, &children)?;
        let registration = self
            .machine()?
            .current_session_registration_witness(&selected.helper.session_id)
            .await
            .ok_or(PrivateOriginalError::Unavailable)?;
        if registration.session_id() != &selected.helper.session_id {
            return Err(PrivateOriginalError::Conflict);
        }
        let reservation = login
            .map(|login| login.reserve())
            .transpose()
            .map_err(|_| PrivateOriginalError::Denied)?;
        let result = async {
            for child in &children {
                let observation = facts
                    .lifetime
                    .with_current_setup(|| {
                        let grants = self.owner.grants();
                        let parent = grants
                            .resolve_lineage(
                                std::slice::from_ref(&child.parent),
                                &facts.application,
                                facts.route.represented_subject.as_ref(),
                            )
                            .map_err(|_| HostLifetimeError::Denied)?;
                        if parent.restrictions() != &child.parent_bounds {
                            return Err(HostLifetimeError::Denied);
                        }
                        parent
                            .check_current()
                            .map_err(|_| HostLifetimeError::Unavailable)?;
                        let leaf = match grants
                            .issued_reference(&child.id)
                            .map_err(|_| HostLifetimeError::Unavailable)?
                        {
                            Some(leaf) => leaf,
                            None => grants
                                .issue_child(
                                    &facts.application,
                                    &child.parent,
                                    child.id.clone(),
                                    facts.route.logical_executor.clone(),
                                    child.requested.clone(),
                                )
                                .map_err(|_| HostLifetimeError::Unavailable)?,
                        };
                        child
                            .observe(&grants, &facts, leaf)
                            .map_err(|_| HostLifetimeError::Denied)
                    })
                    .map_err(|_| PrivateOriginalError::Unavailable)?;
                if let Some(prior) = record
                    .service_setup_children
                    .iter()
                    .find(|prior| prior.target == child.target)
                {
                    if prior != &observation {
                        return Err(PrivateOriginalError::Conflict);
                    }
                } else {
                    record.service_setup_children.push(observation);
                    stored = self
                        .retain_service_setup_observations(&stored, &record)
                        .await?;
                }
            }
            Ok(())
        }
        .await;
        if result.is_ok() {
            if let Some(reservation) = reservation {
                reservation.settled();
            }
        }
        result
    }

    async fn retain_service_setup_observations(
        &self,
        stored: &StoredRecord,
        record: &SourceRecord,
    ) -> Result<StoredRecord, PrivateOriginalError> {
        self.store
            .compare_and_swap(Transition {
                key: stored.key.clone(),
                expected_revision: stored.revision,
                expected_digest: Some(stored.content_digest.clone()),
                claim: None,
                data: encode(record.clone())?,
                events: vec![],
                effects: vec![],
            })
            .await
            .map_err(store_error)?;
        let saved = self
            .store
            .get(&stored.key)
            .await
            .map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        if self.decode_stored(&saved)? != *record {
            return Err(PrivateOriginalError::Conflict);
        }
        Ok(saved)
    }

    fn check_service_setup_observations(
        &self,
        record: &SourceRecord,
        children: &[ServiceSetupChild],
    ) -> Result<(), PrivateOriginalError> {
        for observation in &record.service_setup_children {
            let child = children
                .iter()
                .find(|child| child.target == observation.target)
                .ok_or(PrivateOriginalError::Conflict)?;
            if !child.matches(observation) {
                return Err(PrivateOriginalError::Conflict);
            }
        }
        Ok(())
    }

    /// Reconstruct this source's exact children. Historical structure is
    /// compared with the immutable binding; current entry resolves native use.
    pub(in crate::private_originals) fn service_setup_targets(
        &self,
        reference: &PrivateOriginalRef,
        selected: &ServiceSetupSelection,
        current: bool,
    ) -> Result<Vec<TargetGrantLineage>, PrivateOriginalError> {
        let (facts, children) = self.service_setup_children(reference, selected)?;
        children
            .iter()
            .map(|child| {
                let grants = self.owner.grants();
                let leaf = grants
                    .issued_reference(&child.id)
                    .map_err(|_| PrivateOriginalError::Unavailable)?
                    .ok_or(PrivateOriginalError::Unavailable)?;
                if leaf.root_authority != child.parent.root_authority {
                    return Err(PrivateOriginalError::Conflict);
                }
                if current {
                    child.observe(&grants, &facts, leaf.clone())?;
                }
                Ok(TargetGrantLineage {
                    target: child.target.clone(),
                    lineage: vec![child.parent.clone(), leaf],
                })
            })
            .collect()
    }

    /// Called with the existing source writer held. No new native issuance and
    /// no source admission occurs. Unknown children stay unresolved; proven
    /// children may be cleaned up independently. All reads precede revocation.
    pub(in crate::private_originals) async fn prepare_service_setup_cleanup(
        &self,
        reference: &PrivateOriginalRef,
        stored: &StoredRecord,
    ) -> Result<ServiceSetupCleanup, PrivateOriginalError> {
        let mut record = self.decode_record(stored, reference)?;
        let Some(selected) = record.approved_task.as_ref().and_then(|task| task.setup()) else {
            return Ok(ServiceSetupCleanup {
                issuer: None,
                children: vec![],
                unresolved: false,
            });
        };
        if !record.operation_cancelled
            || record.final_association.is_some()
            || record.bound_service_input.is_some()
            || record.service_admission.is_some()
        {
            return Err(PrivateOriginalError::Unavailable);
        }
        let (facts, children) = self.service_setup_children(reference, selected)?;
        self.check_service_setup_observations(&record, &children)?;
        let mut cleanup = ServiceSetupCleanup {
            issuer: Some(facts.application.clone()),
            children: vec![],
            unresolved: false,
        };
        let mut changed = false;
        for child in &children {
            let retained = record
                .service_setup_children
                .iter()
                .find(|prior| prior.target == child.target);
            let leaf = match self.owner.grants().issued_reference(&child.id) {
                Ok(Some(leaf)) => leaf,
                Ok(None) => {
                    cleanup.unresolved |= retained.is_some();
                    continue;
                }
                Err(_) => {
                    cleanup.unresolved = true;
                    continue;
                }
            };
            let observation = match retained {
                Some(prior) if prior.child == leaf => prior.clone(),
                Some(_) => {
                    cleanup.unresolved = true;
                    continue;
                }
                None => match child.observe(&self.owner.grants(), &facts, leaf) {
                    Ok(observation) => {
                        record.service_setup_children.push(observation.clone());
                        changed = true;
                        observation
                    }
                    Err(_) => {
                        cleanup.unresolved = true;
                        continue;
                    }
                },
            };
            cleanup.children.push(observation);
        }
        // A lost write answer cannot be promoted to successful observation or
        // cleanup. A later exact source/native read can reconcile it.
        if changed {
            self.retain_service_setup_observations(stored, &record)
                .await?;
        }
        Ok(cleanup)
    }
}

// This eligibility check applies only before selection. Captured selection
// validation never falls back to a legacy review after its scope changes.
fn supports_warm_metadata(
    original: &PrincipalRef,
    audience: &AudienceRef,
    helper: &PrincipalRef,
    helper_audience: &AudienceRef,
) -> bool {
    helper_audience == audience && helper != original
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::private_originals) struct ServiceSetupChildObservation {
    target: GrantTarget,
    parent: GrantLineageRef,
    child: GrantLineageRef,
    restrictions: ExecutionRestrictions,
}

impl ServiceSetupChildObservation {
    pub(in crate::private_originals) fn validate_set(
        record: &SourceRecord,
    ) -> Result<(), PrivateOriginalError> {
        let observations = &record.service_setup_children;
        if observations.is_empty() {
            return Ok(());
        }
        if record
            .approved_task
            .as_ref()
            .and_then(|task| task.setup())
            .is_none()
            || observations.len()
                >= meerkat_authorization_contracts::work_association::MAX_ASSOCIATION_REFERENCES
            || observations.iter().enumerate().any(|(index, item)| {
                item.child.root_authority != item.parent.root_authority
                    || observations[..index]
                        .iter()
                        .any(|prior| prior.target == item.target)
            })
        {
            return Err(PrivateOriginalError::Conflict);
        }
        Ok(())
    }
}

struct ServiceSetupChild {
    target: GrantTarget,
    parent: GrantLineageRef,
    id: EvidenceId,
    parent_bounds: ExecutionRestrictions,
    requested: ExecutionRestrictions,
    expected: ExecutionRestrictions,
}
impl ServiceSetupChild {
    fn new(
        target: GrantTarget,
        parent: GrantLineageRef,
        id: EvidenceId,
        parent_bounds: ExecutionRestrictions,
        requested: ExecutionRestrictions,
    ) -> Result<Self, PrivateOriginalError> {
        let expected = parent_bounds
            .for_child(&requested)
            .map_err(|_| PrivateOriginalError::Conflict)?;
        Ok(Self {
            target,
            parent,
            id,
            parent_bounds,
            requested,
            expected,
        })
    }
    fn matches(&self, observation: &ServiceSetupChildObservation) -> bool {
        observation.target == self.target
            && observation.parent == self.parent
            && observation.child.grant_id == self.id
            && observation.child.root_authority == self.parent.root_authority
            && observation.restrictions == self.expected
    }
    fn observe(
        &self,
        grants: &meerkat_authorization::grants::LocalGrantAuthority,
        facts: &ConfiguredSetup,
        leaf: GrantLineageRef,
    ) -> Result<ServiceSetupChildObservation, PrivateOriginalError> {
        if leaf.grant_id != self.id || leaf.root_authority != self.parent.root_authority {
            return Err(PrivateOriginalError::Conflict);
        }
        let lineage = vec![self.parent.clone(), leaf.clone()];
        let current = grants
            .resolve_lineage(
                &lineage,
                &facts.route.logical_executor,
                facts.route.represented_subject.as_ref(),
            )
            .map_err(|_| PrivateOriginalError::Denied)?;
        if current.restrictions() != &self.expected {
            return Err(PrivateOriginalError::Conflict);
        }
        current
            .check_current()
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        Ok(ServiceSetupChildObservation {
            target: self.target.clone(),
            parent: self.parent.clone(),
            child: leaf,
            restrictions: self.expected.clone(),
        })
    }
}

pub(in crate::private_originals) struct ServiceSetupCleanup {
    issuer: Option<PrincipalRef>,
    children: Vec<ServiceSetupChildObservation>,
    unresolved: bool,
}
impl ServiceSetupCleanup {
    /// Exact observed child references only. Shared configured parents are
    /// deliberately not candidates for this task's compensation.
    pub(in crate::private_originals) fn revoke(
        self,
        source: &PrivateOriginals,
    ) -> Result<(), PrivateOriginalError> {
        let mut unresolved = self.unresolved;
        if let Some(issuer) = self.issuer {
            let machine = source.machine()?;
            for child in self.children {
                match source
                    .owner
                    .grants()
                    .issued_reference(&child.child.grant_id)
                {
                    Ok(Some(current)) if current == child.child => {
                        let result = machine
                            .try_controller_grant_mutation()
                            .map_err(|_| PrivateOriginalError::Unavailable)
                            .and_then(|mut custody| {
                                source
                                    .owner
                                    .grants()
                                    .revoke(&issuer, &current, &mut custody)
                                    .map_err(|_| PrivateOriginalError::Unavailable)
                            });
                        unresolved |= result.is_err();
                    }
                    _ => unresolved = true,
                }
            }
        }
        if unresolved {
            Err(PrivateOriginalError::Unavailable)
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
#[path = "service_native_tests.rs"]
mod native_tests;
