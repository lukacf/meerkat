//! Exact MCP target of an independently selected workflow envelope.
//! All references and currentness remain owned by the native grant authority.
use meerkat_authorization::grants::{
    GrantRefusal, LineageResolutionRefusal, LocalGrantAuthority, ResolvedGrant,
};
use meerkat_authorization::publication::PublicationError;
use meerkat_authorization_contracts::{
    constraints::{
        DelegationDepth, DomainRestriction, ExactRestriction, ExecutionRestrictions, SetBound,
    },
    evidence::EvidenceId,
    grant::GrantLineageRef,
    work_association::{GrantTarget, TargetGrantLineage, WorkAuthorityBasis},
};
use meerkat_core::{McpServerConfig, PrincipalRef};
use meerkat_toolkit::host_lifetime::control::HostLifetimeError;

pub(crate) const SERVER: &str = "toolkit-setup";

pub(crate) fn target_ids(base: &[GrantLineageRef]) -> Result<[EvidenceId; 2], HostLifetimeError> {
    if base.len() != 2 {
        return Err(HostLifetimeError::Denied);
    }
    let make = |role| {
        // Bounds are deliberately absent: changed intent must check the same
        // already-issued IDs and refuse, never manufacture fresh authority.
        let bytes = toolkit_package::canonical_json(&(
            "toolkit.workflow-mcp.v1",
            &base[0].grant_id,
            &base[1].grant_id,
            SERVER,
            role,
        ))
        .map_err(|_| HostLifetimeError::Unavailable)?;
        EvidenceId::new(format!(
            "workflow-mcp/{}",
            toolkit_package::ContentDigest::of_bytes(&bytes)
        ))
        .map_err(|_| HostLifetimeError::Unavailable)
    };
    let ids = [make("root")?, make("leaf")?];
    if ids[0] == ids[1]
        || ids
            .iter()
            .any(|id| base.iter().any(|grant| &grant.grant_id == id))
    {
        return Err(HostLifetimeError::Denied);
    }
    Ok(ids)
}

#[cfg(test)]
pub(crate) fn basis(
    base: Vec<GrantLineageRef>,
    target: Vec<GrantLineageRef>,
) -> WorkAuthorityBasis {
    basis_with_public(base, target, None)
}

pub(crate) fn basis_with_public(
    base: Vec<GrantLineageRef>,
    target: Vec<GrantLineageRef>,
    public: Option<Vec<GrantLineageRef>>,
) -> WorkAuthorityBasis {
    let mut targets = vec![TargetGrantLineage {
        target: GrantTarget::McpServer {
            server: SERVER.into(),
        },
        lineage: target,
    }];
    if let Some(lineage) = public {
        targets.push(TargetGrantLineage {
            target: super::workflow_public::target(),
            lineage,
        });
    }
    targets.sort_by(|a, b| a.target.cmp(&b.target));
    WorkAuthorityBasis::TargetedGrantLineages {
        base: Some(base),
        targets,
    }
}

pub(crate) fn split(
    basis: &WorkAuthorityBasis,
) -> Result<(&[GrantLineageRef], Option<&[GrantLineageRef]>), HostLifetimeError> {
    match basis {
        WorkAuthorityBasis::GrantLineage { lineage } if lineage.len() >= 2 => Ok((lineage, None)),
        WorkAuthorityBasis::TargetedGrantLineages {
            base: Some(base),
            targets,
        } if base.len() == 2 && (1..=2).contains(&targets.len()) => {
            if targets
                .windows(2)
                .any(|rows| rows[0].target >= rows[1].target)
            {
                return Err(HostLifetimeError::Denied);
            }
            let mcp = targets
                .iter()
                .find(|row| {
                    row.target
                        == (GrantTarget::McpServer {
                            server: SERVER.into(),
                        })
                })
                .ok_or(HostLifetimeError::Denied)?;
            let ids = target_ids(base)?;
            if mcp.lineage.len() != 2
                || mcp
                    .lineage
                    .iter()
                    .zip(&ids)
                    .any(|(grant, id)| &grant.grant_id != id)
            {
                return Err(HostLifetimeError::Denied);
            }
            for row in targets.iter().filter(|row| row.target != mcp.target) {
                if row.target != super::workflow_public::target() {
                    return Err(HostLifetimeError::Denied);
                }
                super::workflow_public::validate_shape(base, &row.lineage)?;
            }
            Ok((base, Some(&mcp.lineage)))
        }
        _ => Err(HostLifetimeError::Denied),
    }
}

pub(crate) fn public_lineage(
    basis: &WorkAuthorityBasis,
) -> Result<Option<&[GrantLineageRef]>, HostLifetimeError> {
    split(basis)?;
    Ok(match basis {
        WorkAuthorityBasis::TargetedGrantLineages { .. } => {
            basis.lineage_for(Some(&super::workflow_public::target()))
        }
        _ => None,
    })
}

/// Historical G custody keeps every target reference, but only the existing
/// base/MCP observations gate general planning restoration. Public revocation
/// is checked by the native policy when that exact registered target is used.
pub(crate) fn planning_lineages(
    basis: &WorkAuthorityBasis,
) -> Result<Vec<&[GrantLineageRef]>, HostLifetimeError> {
    let (base, mcp) = split(basis)?;
    Ok(std::iter::once(base).chain(mcp).collect())
}

pub(crate) fn target_bounds(
    application: &PrincipalRef,
    envelope: &ExecutionRestrictions,
    server: &McpServerConfig,
) -> Result<ExecutionRestrictions, HostLifetimeError> {
    let access = server.access.as_ref().ok_or(HostLifetimeError::Denied)?;
    let SetBound::Exact(actions) = envelope.actions.bound() else {
        return Err(HostLifetimeError::Denied);
    };
    let approval = meerkat_authorization::mcp_installation::mcp_connector_action(
        SERVER,
        &meerkat_core::McpConnectorApproval::NoConnector,
    );
    let selected = actions
        .iter()
        .filter(|action| action.feature == "mcp")
        .cloned()
        .collect::<Vec<_>>();
    if server.name != SERVER
        || server.connector.is_some()
        || selected.is_empty()
        || !actions.contains(&approval)
        || !envelope.actions.unresolved_facts().is_empty()
        || selected
            .iter()
            .any(|action| !access.allowed_operations.contains(&action.action))
        || envelope.resource_domains != DomainRestriction::owned_by([application.clone()])
        || envelope.delegation_depth != DelegationDepth::remaining(0)
    {
        return Err(HostLifetimeError::Denied);
    }
    let mut target = envelope.clone();
    target.actions = ExactRestriction::exact(selected.into_iter().chain(std::iter::once(approval)));
    target.resource_domains = DomainRestriction::exact([
        meerkat_authorization_contracts::constraints::ResourceDomain {
            authority: application.clone(),
            namespace: format!("mcp-server/{SERVER}"),
        },
        meerkat_authorization::mcp_installation::mcp_connector_domain(
            application,
            SERVER,
            &meerkat_core::McpConnectorApproval::NoConnector,
        ),
    ]);
    Ok(target)
}

pub(crate) fn resolve_target(
    grants: &LocalGrantAuthority,
    application: &PrincipalRef,
    executor: &PrincipalRef,
    represented: Option<&PrincipalRef>,
    base: &[GrantLineageRef],
    target: &[GrantLineageRef],
    envelope: &ExecutionRestrictions,
    server: &McpServerConfig,
) -> Result<ResolvedGrant, HostLifetimeError> {
    resolve_target_observed(
        grants,
        application,
        executor,
        represented,
        base,
        target,
        envelope,
        server,
    )
    .map_err(|error| match error {
        LineageResolutionRefusal::Changed
        | LineageResolutionRefusal::Refused(GrantRefusal::Unavailable) => {
            HostLifetimeError::Unavailable
        }
        _ => HostLifetimeError::Denied,
    })
}

// Preserve native Changed inside the existing enclosing child observation.
// Standalone callers use the wrapper above and do not retry effects.
pub(crate) fn resolve_target_observed(
    grants: &LocalGrantAuthority,
    application: &PrincipalRef,
    executor: &PrincipalRef,
    represented: Option<&PrincipalRef>,
    base: &[GrantLineageRef],
    target: &[GrantLineageRef],
    envelope: &ExecutionRestrictions,
    server: &McpServerConfig,
) -> Result<ResolvedGrant, LineageResolutionRefusal> {
    let ids = target_ids(base).map_err(selected_refusal)?;
    if target.len() != 2
        || target
            .iter()
            .zip(&ids)
            .any(|(grant, id)| &grant.grant_id != id)
        || target[0].root_authority != base[0].root_authority
    {
        return Err(LineageResolutionRefusal::Refused(GrantRefusal::Denied));
    }
    let expected = target_bounds(application, envelope, server).map_err(selected_refusal)?;
    let mut root_bounds = expected.clone();
    root_bounds.delegation_depth = DelegationDepth::remaining(1);
    let root = grants.resolve_lineage_observed(&target[..1], application, represented)?;
    let resolved = grants.resolve_lineage_observed(target, executor, represented)?;
    if root.restrictions() != &root_bounds || resolved.restrictions() != &expected {
        return Err(LineageResolutionRefusal::Refused(GrantRefusal::Denied));
    }
    let action = meerkat_authorization::mcp_installation::mcp_connector_action(
        SERVER,
        &meerkat_core::McpConnectorApproval::NoConnector,
    );
    let installation = meerkat_authorization_contracts::constraints::ResourceDomain {
        authority: application.clone(),
        namespace: format!("mcp-server/{SERVER}"),
    };
    if meerkat_authorization::mcp_installation::lineage_connector_bound(
        resolved.lineage_restrictions(),
        resolved.restrictions(),
        &action,
        &installation,
    ) != meerkat_authorization::mcp_installation::McpConnectorBound::Approved
    {
        return Err(LineageResolutionRefusal::Refused(GrantRefusal::Denied));
    }
    root.check_current().map_err(publication_refusal)?;
    resolved.check_current().map_err(publication_refusal)?;
    Ok(resolved)
}
fn publication_refusal(error: PublicationError) -> LineageResolutionRefusal {
    match error {
        PublicationError::Changed => LineageResolutionRefusal::Changed,
        PublicationError::Unavailable => {
            LineageResolutionRefusal::Refused(GrantRefusal::Unavailable)
        }
    }
}
fn selected_refusal(error: HostLifetimeError) -> LineageResolutionRefusal {
    LineageResolutionRefusal::Refused(match error {
        HostLifetimeError::Unavailable => GrantRefusal::Unavailable,
        _ => GrantRefusal::Denied,
    })
}
