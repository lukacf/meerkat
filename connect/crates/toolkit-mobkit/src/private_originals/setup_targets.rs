//! Existing original-source projection of independently issued target children.
use super::*;
use crate::installed_host::operator_setup::setup_targets as targets;
use crate::original_ingress::helper_originals::HelperSetupOrigin;
use meerkat_authorization::grants::GrantRefusal;
use meerkat_authorization_contracts::{
    grant::GrantLineageRef,
    work_association::{GrantTarget, TargetGrantLineage, WorkAuthorityBasis},
};
use meerkat_toolkit::host_lifetime::{control::HostLifetimeError, policy::CompiledAgentSetupTarget};

impl PrivateOriginals {
    pub(super) fn setup_origin_for(&self, reference: &PrivateOriginalRef) -> Result<HelperSetupOrigin, PrivateOriginalError> {
        let route = self.route(&reference.route)?;
        Ok(HelperSetupOrigin { realm: route.realm.clone(), source_owner: self.owner.ingress_domain().clone(),
            original_authority: route.source_authority.clone(), original: reference.clone() })
    }
}

pub(super) fn child_id(find: &EvidenceId, target: &Option<GrantTarget>) -> Result<EvidenceId, HostLifetimeError> {
    let bytes = canonical_json(&("toolkit.original-setup-target.v1", find, target))
        .map_err(|_| HostLifetimeError::InvalidSelection)?;
    EvidenceId::new(format!("original-setup-target/{}", digest(&bytes)))
        .map_err(|_| HostLifetimeError::InvalidSelection)
}

pub(super) fn issue_targets(owner: &ToolkitOriginalOwner, application: &PrincipalRef,
    executor: &PrincipalRef, subject: Option<&PrincipalRef>, origin: &HelperSetupOrigin,
    find_id: &EvidenceId, selected: &[CompiledAgentSetupTarget])
    -> Result<Vec<(Option<GrantTarget>, Vec<GrantLineageRef>)>, HostLifetimeError>
{
    targets::validate_compiled(selected)?;
    selected.iter().map(|selected| {
        let grants = owner.grants();
        let requested = targets::child_bounds(application, origin, &selected.target, &selected.restrictions)?;
        let id = child_id(find_id, &selected.target)?;
        let issue = || {
            let parent = grants.resolve_lineage(std::slice::from_ref(&selected.parent), application, subject)?;
            if parent.restrictions() != &selected.restrictions { return Err(GrantRefusal::Denied); }
            let expected = parent.restrictions().for_child(&requested).map_err(|_| GrantRefusal::Denied)?;
            parent.check_current().map_err(|_| GrantRefusal::Unavailable)?;
            let leaf = match grants.issued_reference(&id)? {
                Some(leaf) => leaf,
                None => grants.issue_child(application, &selected.parent, id, executor.clone(), requested)?,
            };
            let lineage = vec![selected.parent.clone(), leaf];
            let child = grants.resolve_lineage(&lineage, executor, subject)?;
            if child.restrictions() != &expected { return Err(GrantRefusal::Denied); }
            child.check_current().map_err(|_| GrantRefusal::Unavailable)?;
            Ok(lineage)
        };
        issue().map(|lineage| (selected.target.clone(), lineage)).map_err(|error| match error {
            GrantRefusal::Denied | GrantRefusal::Duplicate | GrantRefusal::ControllerInUse => HostLifetimeError::Denied,
            GrantRefusal::Unavailable => HostLifetimeError::Unavailable,
        })
    }).collect()
}

pub(super) fn project(find: Vec<GrantLineageRef>, selected: Vec<(Option<GrantTarget>, Vec<GrantLineageRef>)>)
    -> Result<WorkAuthorityBasis, HostLifetimeError>
{
    if selected.is_empty() { return Ok(WorkAuthorityBasis::GrantLineage { lineage: find }); }
    targets::validate_targets(selected.iter().map(|(target, _)| target))?;
    let mut base = None;
    let mut targets = vec![TargetGrantLineage { target: targets::find_target(), lineage: find }];
    for (target, lineage) in selected {
        if let Some(target) = target { targets.push(TargetGrantLineage { target, lineage }); }
        else { base = Some(lineage); }
    }
    targets.sort_by(|left, right| left.target.cmp(&right.target));
    Ok(WorkAuthorityBasis::TargetedGrantLineages { base, targets })
}

pub(super) fn find_lineage(basis: &WorkAuthorityBasis) -> Option<&[GrantLineageRef]> {
    match basis {
        WorkAuthorityBasis::GrantLineage { lineage } => Some(lineage),
        WorkAuthorityBasis::TargetedGrantLineages { .. } => basis.lineage_for(Some(&targets::find_target())),
        _ => None,
    }
}

/// Historical reconstruction compares the complete retained selection without
/// resolving unrelated target currentness. Native operation entry does that.
pub(super) fn check_structure(find_parent: &[GrantLineageRef], find_id: &EvidenceId,
    selected: &[CompiledAgentSetupTarget], basis: &WorkAuthorityBasis) -> Result<(), HostLifetimeError>
{
    targets::validate_compiled(selected)?;
    let find = find_lineage(basis).ok_or(HostLifetimeError::Denied)?;
    let check = |parent: &[GrantLineageRef], id: &EvidenceId, lineage: &[GrantLineageRef]| {
        if lineage.len() != parent.len() + 1 || !lineage.starts_with(parent)
            || lineage.last().is_none_or(|leaf| &leaf.grant_id != id
                || parent.last().is_none_or(|root| leaf.root_authority != root.root_authority))
        { Err(HostLifetimeError::Denied) } else { Ok(()) }
    };
    check(find_parent, find_id, find)?;
    let mut targets = vec![];
    for target in selected {
        let lineage = basis.lineage_for(target.target.as_ref()).ok_or(HostLifetimeError::Denied)?;
        check(std::slice::from_ref(&target.parent), &child_id(find_id, &target.target)?, lineage)?;
        targets.push((target.target.clone(), lineage.to_vec()));
    }
    if project(find.to_vec(), targets)? != *basis { return Err(HostLifetimeError::Denied); }
    Ok(())
}
