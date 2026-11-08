//! Existing original-source projection of independently issued target children.
use super::*;
use crate::installed_host::operator_setup::setup_targets as targets;
use crate::original_ingress::helper_originals::HelperSetupOrigin;
use meerkat_authorization::{
    grants::{GrantRefusal, LineageResolutionRefusal, LocalGrantAuthority},
    publication::{LocalAuthorizationPublication, PublicationError},
};
use meerkat_authorization_contracts::constraints::ExecutionRestrictions;
use meerkat_authorization_contracts::{
    grant::GrantLineageRef,
    work_association::{GrantTarget, TargetGrantLineage, WorkAuthorityBasis},
};
use meerkat_core::authorization::{OWNER_CHANGE_FREE_REBUILDS, OWNER_CHANGE_WAIT_BOUND};
use meerkat_toolkit::host_lifetime::{
    control::HostLifetimeError, policy::CompiledAgentSetupTarget,
};
use std::time::{Duration, Instant};

impl PrivateOriginals {
    pub(super) fn setup_origin_for(
        &self,
        reference: &PrivateOriginalRef,
    ) -> Result<HelperSetupOrigin, PrivateOriginalError> {
        let route = &self.route(&reference.route)?;
        Ok(HelperSetupOrigin {
            realm: route.realm.clone(),
            source_owner: self.owner.ingress_domain().clone(),
            original_authority: route.source_authority.clone(),
            original: reference.clone(),
        })
    }
}

pub(super) fn child_id(
    find: &EvidenceId,
    target: &Option<GrantTarget>,
) -> Result<EvidenceId, HostLifetimeError> {
    let bytes = canonical_json(&("toolkit.original-setup-target.v1", find, target))
        .map_err(|_| HostLifetimeError::InvalidSelection)?;
    EvidenceId::new(format!("original-setup-target/{}", digest(&bytes)))
        .map_err(|_| HostLifetimeError::InvalidSelection)
}

/// A selected target and the grant lineage it is issued under.
type TargetLineage = (Option<GrantTarget>, Vec<GrantLineageRef>);

pub(super) fn issue_targets(
    owner: &ToolkitOriginalOwner,
    application: &PrincipalRef,
    executor: &PrincipalRef,
    subject: Option<&PrincipalRef>,
    origin: &HelperSetupOrigin,
    find_id: &EvidenceId,
    selected: &[CompiledAgentSetupTarget],
) -> Result<Vec<TargetLineage>, HostLifetimeError> {
    issue_selected_targets(
        owner,
        application,
        executor,
        subject,
        origin,
        find_id,
        selected,
        false,
    )
}

pub(super) fn issue_workflow_targets(
    owner: &ToolkitOriginalOwner,
    application: &PrincipalRef,
    executor: &PrincipalRef,
    subject: Option<&PrincipalRef>,
    origin: &HelperSetupOrigin,
    find_id: &EvidenceId,
    selected: &[CompiledAgentSetupTarget],
) -> Result<Vec<TargetLineage>, HostLifetimeError> {
    issue_selected_targets(
        owner,
        application,
        executor,
        subject,
        origin,
        find_id,
        selected,
        true,
    )
}

fn issue_selected_targets(
    owner: &ToolkitOriginalOwner,
    application: &PrincipalRef,
    executor: &PrincipalRef,
    subject: Option<&PrincipalRef>,
    origin: &HelperSetupOrigin,
    find_id: &EvidenceId,
    selected: &[CompiledAgentSetupTarget],
    shared: bool,
) -> Result<Vec<TargetLineage>, HostLifetimeError> {
    if shared {
        targets::validate_workflow_compiled(selected)?;
    } else {
        targets::validate_compiled(selected)?;
    }
    let publication = owner.publication();
    let mut resolution = ChildResolutionBudget::default();
    selected
        .iter()
        .map(|selected| {
            let grants = owner.grants();
            let mut requested = targets::child_bounds(
                application,
                origin,
                &selected.target,
                &selected.restrictions,
            )?;
            if shared {
                requested.delegation_depth =
                    meerkat_authorization_contracts::constraints::DelegationDepth::remaining(1);
            }
            let id = child_id(find_id, &selected.target)?;
            let issue = || {
                let parent = grants.resolve_lineage(
                    std::slice::from_ref(&selected.parent),
                    application,
                    subject,
                )?;
                if parent.restrictions() != &selected.restrictions {
                    return Err(GrantRefusal::Denied);
                }
                let expected = parent
                    .restrictions()
                    .for_child(&requested)
                    .map_err(|_| GrantRefusal::Denied)?;
                parent
                    .check_current()
                    .map_err(|_| GrantRefusal::Unavailable)?;
                let leaf = match grants.issued_reference(&id)? {
                    Some(leaf) => leaf,
                    None => grants.issue_child(
                        application,
                        &selected.parent,
                        id,
                        executor.clone(),
                        requested,
                    )?,
                };
                let lineage = vec![selected.parent.clone(), leaf];
                // Issuance above is never replayed. Only a disposable child read
                // is rebuilt after this actual owner's publication changes.
                resolution
                    .resolve(
                        &grants,
                        &publication,
                        &lineage,
                        executor,
                        subject,
                        &expected,
                    )
                    .inspect_err(|error| match error {
                        LineageResolutionRefusal::Changed => crate::aa1_failure(
                            "binding.target.child_resolve",
                            "LineageResolutionRefusal::Changed",
                        ),
                        LineageResolutionRefusal::Refused(error) => {
                            first_input::binding_grant_failure(
                                "binding.target.child_resolve",
                                error,
                            )
                        }
                    })
                    .map_err(|error| match error {
                        LineageResolutionRefusal::Changed => GrantRefusal::Unavailable,
                        LineageResolutionRefusal::Refused(error) => error,
                    })?;
                Ok(lineage)
            };
            issue()
                .map(|lineage| (selected.target.clone(), lineage))
                .map_err(|error| match error {
                    GrantRefusal::Denied
                    | GrantRefusal::Duplicate
                    | GrantRefusal::ControllerInUse => HostLifetimeError::Denied,
                    GrantRefusal::Unavailable => HostLifetimeError::Unavailable,
                })
        })
        .collect()
}

/// One bounded observation budget for this selected target batch. The caller
/// already runs on the existing blocking writer with its login and host entry
/// reservations. Neither reservation owns the publication writer, and native
/// issuance has returned before this read. No mutation is retried here.
#[derive(Default)]
struct ChildResolutionBudget {
    deadline: Option<Instant>,
    rebuilds: u8,
}

impl ChildResolutionBudget {
    fn resolve(
        &mut self,
        grants: &LocalGrantAuthority,
        publication: &LocalAuthorizationPublication,
        lineage: &[GrantLineageRef],
        executor: &PrincipalRef,
        subject: Option<&PrincipalRef>,
        expected: &ExecutionRestrictions,
    ) -> Result<(), LineageResolutionRefusal> {
        let mut rebuilding = false;
        loop {
            if rebuilding
                && self
                    .deadline
                    .is_some_and(|deadline| Instant::now() >= deadline)
            {
                return Err(LineageResolutionRefusal::Changed);
            }
            let observed = grants
                .resolve_lineage_observed(lineage, executor, subject)
                .and_then(|child| {
                    if child.restrictions() != expected {
                        crate::aa1_failure("binding.target.child_bounds", "GrantRefusal::Denied");
                        return Err(LineageResolutionRefusal::Refused(GrantRefusal::Denied));
                    }
                    child.check_current().map_err(|error| {
                        first_input::binding_publication_failure(
                            "binding.target.child_current",
                            &error,
                        );
                        match error {
                            PublicationError::Changed => LineageResolutionRefusal::Changed,
                            PublicationError::Unavailable => {
                                LineageResolutionRefusal::Refused(GrantRefusal::Unavailable)
                            }
                        }
                    })
                });
            match observed {
                Ok(()) => {
                    if rebuilding
                        && self
                            .deadline
                            .is_some_and(|deadline| Instant::now() >= deadline)
                    {
                        return Err(LineageResolutionRefusal::Changed);
                    }
                    return Ok(());
                }
                Err(error @ LineageResolutionRefusal::Refused(_)) => return Err(error),
                Err(LineageResolutionRefusal::Changed) => {}
            }
            let deadline = *self
                .deadline
                .get_or_insert_with(|| Instant::now() + OWNER_CHANGE_WAIT_BOUND);
            if self.rebuilds >= OWNER_CHANGE_FREE_REBUILDS {
                return Err(LineageResolutionRefusal::Changed);
            }
            let mut backoff = Duration::from_millis(1);
            loop {
                let now = Instant::now();
                if now >= deadline {
                    return Err(LineageResolutionRefusal::Changed);
                }
                // None is not permission: a poisoned publication also returns
                // None, and the next fresh read must reject it as Refused.
                if publication.change_in_progress().is_none() {
                    break;
                }
                std::thread::sleep(backoff.min(deadline.saturating_duration_since(now)));
                backoff = backoff.saturating_mul(2).min(Duration::from_millis(64));
            }
            self.rebuilds += 1;
            rebuilding = true;
        }
    }
}

pub(super) fn project(
    find: Vec<GrantLineageRef>,
    selected: Vec<(Option<GrantTarget>, Vec<GrantLineageRef>)>,
) -> Result<WorkAuthorityBasis, HostLifetimeError> {
    if selected.is_empty() {
        return Ok(WorkAuthorityBasis::GrantLineage { lineage: find });
    }
    targets::validate_targets(selected.iter().map(|(target, _)| target)).inspect_err(|error| {
        first_input::binding_host_failure("binding.target.projection", error)
    })?;
    let mut base = None;
    let mut targets = vec![TargetGrantLineage {
        target: targets::find_target(),
        lineage: find,
    }];
    for (target, lineage) in selected {
        if let Some(target) = target {
            targets.push(TargetGrantLineage { target, lineage });
        } else {
            base = Some(lineage);
        }
    }
    targets.sort_by(|left, right| left.target.cmp(&right.target));
    Ok(WorkAuthorityBasis::TargetedGrantLineages { base, targets })
}

pub(super) fn find_lineage(basis: &WorkAuthorityBasis) -> Option<&[GrantLineageRef]> {
    match basis {
        WorkAuthorityBasis::GrantLineage { lineage } => Some(lineage),
        WorkAuthorityBasis::TargetedGrantLineages { .. } => {
            basis.lineage_for(Some(&targets::find_target()))
        }
        _ => None,
    }
}

/// Historical reconstruction compares the complete retained selection without
/// resolving unrelated target currentness. Native operation entry does that.
pub(super) fn check_structure(
    find_parent: &[GrantLineageRef],
    find_id: &EvidenceId,
    selected: &[CompiledAgentSetupTarget],
    basis: &WorkAuthorityBasis,
) -> Result<(), HostLifetimeError> {
    targets::validate_compiled(selected)?;
    check_selected_structure(find_parent, find_id, selected, basis)
}

pub(super) fn check_workflow_structure(
    find_parent: &[GrantLineageRef],
    find_id: &EvidenceId,
    selected: &[CompiledAgentSetupTarget],
    basis: &WorkAuthorityBasis,
) -> Result<(), HostLifetimeError> {
    targets::validate_workflow_compiled(selected)?;
    check_selected_structure(find_parent, find_id, selected, basis)
}

fn check_selected_structure(
    find_parent: &[GrantLineageRef],
    find_id: &EvidenceId,
    selected: &[CompiledAgentSetupTarget],
    basis: &WorkAuthorityBasis,
) -> Result<(), HostLifetimeError> {
    let find = find_lineage(basis).ok_or(HostLifetimeError::Denied)?;
    let check = |parent: &[GrantLineageRef], id: &EvidenceId, lineage: &[GrantLineageRef]| {
        if lineage.len() != parent.len() + 1
            || !lineage.starts_with(parent)
            || lineage.last().is_none_or(|leaf| {
                &leaf.grant_id != id
                    || parent
                        .last()
                        .is_none_or(|root| leaf.root_authority != root.root_authority)
            })
        {
            Err(HostLifetimeError::Denied)
        } else {
            Ok(())
        }
    };
    check(find_parent, find_id, find)?;
    let mut targets = vec![];
    for target in selected {
        let lineage = basis
            .lineage_for(target.target.as_ref())
            .ok_or(HostLifetimeError::Denied)?;
        check(
            std::slice::from_ref(&target.parent),
            &child_id(find_id, &target.target)?,
            lineage,
        )?;
        targets.push((target.target.clone(), lineage.to_vec()));
    }
    if project(find.to_vec(), targets)? != *basis {
        return Err(HostLifetimeError::Denied);
    }
    Ok(())
}

#[cfg(test)]
#[path = "setup_targets/lineage_tests.rs"]
mod lineage_tests;
