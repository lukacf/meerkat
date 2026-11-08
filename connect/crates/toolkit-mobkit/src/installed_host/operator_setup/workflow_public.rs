//! One explicit registered public-request child of the selected G base root.
//! The target mapping is not a base fallback and never restores permission.
use meerkat_authorization::grants::{LocalGrantAuthority, ResolvedGrant};
use meerkat_authorization_contracts::{
    constraints::{
        ActionRef, DelegationDepth, DomainRestriction, ExactRestriction, ExecutionRestrictions,
        LifetimeBound, SetBound,
    },
    evidence::EvidenceId,
    grant::GrantLineageRef,
    work_association::{GrantTarget, WorkAuthorityBasis},
};
use meerkat_core::PrincipalRef;
use meerkat_toolkit::host_lifetime::control::HostLifetimeError;

pub(crate) fn target() -> GrantTarget {
    crate::installed_host::acquisition::ConfiguredHelperReportCapability::public_target()
}

pub(crate) fn selected(envelope: &ExecutionRestrictions) -> bool {
    matches!(envelope.actions.bound(), SetBound::Exact(actions) if actions.contains(&action()))
}

fn action() -> ActionRef {
    ActionRef {
        feature: "toolkit".into(),
        action: "setup-public-service-request".into(),
    }
}

pub(crate) fn child_id(base: &[GrantLineageRef]) -> Result<EvidenceId, HostLifetimeError> {
    if base.len() != 2 {
        return Err(HostLifetimeError::Denied);
    }
    // Bounds are excluded: changed intent must reconcile the same issued ID.
    let bytes = toolkit_package::canonical_json(&(
        "toolkit.workflow-public.v1",
        &base[0].grant_id,
        &base[1].grant_id,
        target(),
    ))
    .map_err(|_| HostLifetimeError::Unavailable)?;
    let id = EvidenceId::new(format!(
        "workflow-public/{}",
        toolkit_package::ContentDigest::of_bytes(&bytes)
    ))
    .map_err(|_| HostLifetimeError::Unavailable)?;
    if base.iter().any(|reference| reference.grant_id == id)
        || super::workflow_mcp::target_ids(base)?.contains(&id)
    {
        return Err(HostLifetimeError::Denied);
    }
    Ok(id)
}

pub(crate) fn validate_shape(
    base: &[GrantLineageRef],
    public: &[GrantLineageRef],
) -> Result<(), HostLifetimeError> {
    let id = child_id(base)?;
    if public.len() != 2
        || public[0] != base[0]
        || public[1].grant_id != id
        || public[1].root_authority != base[0].root_authority
    {
        return Err(HostLifetimeError::Denied);
    }
    Ok(())
}

pub(crate) fn bounds(
    application: &PrincipalRef,
    envelope: &ExecutionRestrictions,
) -> Result<ExecutionRestrictions, HostLifetimeError> {
    if !selected(envelope)
        || !envelope.actions.unresolved_facts().is_empty()
        || envelope.resource_domains != DomainRestriction::owned_by([application.clone()])
        || envelope.delegation_depth != DelegationDepth::remaining(0)
        || !envelope.lifetime.unresolved_facts().is_empty()
        || !matches!(envelope.lifetime.bound(), LifetimeBound::Window { not_before_ms, expires_at_ms }
            if not_before_ms < expires_at_ms && expires_at_ms < u64::MAX)
    {
        return Err(HostLifetimeError::Denied);
    }
    let mut selected = envelope.clone();
    selected.actions = ExactRestriction::exact([action()]);
    Ok(selected)
}

/// A canonical bound selection can only recover its exact saved shape. The
/// native reference lookup preserves identity even after revocation; it is
/// not permission and cannot select a target omitted before first admission.
pub(crate) fn select(
    grants: &LocalGrantAuthority,
    application: &PrincipalRef,
    executor: &PrincipalRef,
    represented: Option<&PrincipalRef>,
    base: &[GrantLineageRef],
    envelope: &ExecutionRestrictions,
    saved: Option<&WorkAuthorityBasis>,
    needs_mcp: bool,
) -> Result<Option<Vec<GrantLineageRef>>, HostLifetimeError> {
    if let Some(saved) = saved {
        let (saved_base, mcp) = super::workflow_mcp::split(saved)?;
        if saved_base != base || mcp.is_some() != needs_mcp {
            return Err(HostLifetimeError::Denied);
        }
        let public = super::workflow_mcp::public_lineage(saved)?;
        if let Some(public) = public {
            bounds(application, envelope)?;
            for reference in public {
                if grants
                    .issued_reference(&reference.grant_id)
                    .map_err(refusal)?
                    .as_ref()
                    != Some(reference)
                {
                    return Err(HostLifetimeError::Unavailable);
                }
            }
        }
        return Ok(public.map(<[GrantLineageRef]>::to_vec));
    }
    if !needs_mcp || !selected(envelope) {
        return Ok(None);
    }
    issue(grants, application, executor, represented, base, envelope).map(Some)
}

/// Fresh unbound selection only. An uncertain response reconciles the same
/// child; an already-bound source retains its saved reference without calling
/// this method, including after this public leaf is revoked.
pub(crate) fn issue(
    grants: &LocalGrantAuthority,
    application: &PrincipalRef,
    executor: &PrincipalRef,
    represented: Option<&PrincipalRef>,
    base: &[GrantLineageRef],
    envelope: &ExecutionRestrictions,
) -> Result<Vec<GrantLineageRef>, HostLifetimeError> {
    let id = child_id(base)?;
    let expected = bounds(application, envelope)?;
    let mut root_bounds = envelope.clone();
    root_bounds.delegation_depth = DelegationDepth::remaining(1);
    let root = grants
        .resolve_lineage(&base[..1], application, represented)
        .map_err(refusal)?;
    let original = grants
        .resolve_lineage(base, executor, represented)
        .map_err(refusal)?;
    if root.restrictions() != &root_bounds || original.restrictions() != envelope {
        return Err(HostLifetimeError::Denied);
    }
    root.check_current()
        .map_err(|_| HostLifetimeError::Unavailable)?;
    original
        .check_current()
        .map_err(|_| HostLifetimeError::Unavailable)?;
    let child = match grants.issued_reference(&id).map_err(refusal)? {
        Some(reference) => reference,
        None => grants
            .issue_child(
                &meerkat_core::AdmissionGuard::none(),
                application,
                &base[0],
                id,
                executor.clone(),
                expected,
            )
            .map_err(refusal)?,
    };
    let public = vec![base[0].clone(), child];
    resolve(
        grants,
        application,
        executor,
        represented,
        base,
        &public,
        envelope,
    )?;
    Ok(public)
}

pub(crate) fn resolve(
    grants: &LocalGrantAuthority,
    application: &PrincipalRef,
    executor: &PrincipalRef,
    represented: Option<&PrincipalRef>,
    base: &[GrantLineageRef],
    public: &[GrantLineageRef],
    envelope: &ExecutionRestrictions,
) -> Result<ResolvedGrant, HostLifetimeError> {
    validate_shape(base, public)?;
    let expected = bounds(application, envelope)?;
    let resolved = grants
        .resolve_lineage(public, executor, represented)
        .map_err(refusal)?;
    if resolved.restrictions() != &expected {
        return Err(HostLifetimeError::Denied);
    }
    resolved
        .check_current()
        .map_err(|_| HostLifetimeError::Unavailable)?;
    Ok(resolved)
}

fn refusal(error: meerkat_authorization::grants::GrantRefusal) -> HostLifetimeError {
    match error {
        meerkat_authorization::grants::GrantRefusal::Unavailable => HostLifetimeError::Unavailable,
        _ => HostLifetimeError::Denied,
    }
}
