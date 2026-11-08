//! Fresh late-helper configuration through the existing authenticated operator.
//! This is a trusted host selection, not an agent argument or an approval locator.
use super::workflow_mcp;
use super::*;
use crate::installed_host::dynamic_members::AdmittedMemberClass;
use crate::private_originals::{
    InstalledLateWorkflowHelper, InstalledLateWorkflowWaiter, PrivateOriginals,
};
use meerkat_authorization::grants::GrantRefusal;
use meerkat_authorization_contracts::constraints::{
    ActionRef, DelegationDepth, DomainRestriction, ExactRestriction, LifetimeBound, ProcessorRef,
    SetBound,
};
use meerkat_core::{McpServerConfig, ToolSourceKind};

/// Exact IDs and the independently selected G envelope survive an uncertain
/// response. Per-caller A/B effect reviews are separate requests, not its source.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowHelperAuthorityRequest {
    pub parent_issuance_id: EvidenceId,
    pub leaf_issuance_id: EvidenceId,
    pub envelope: ExecutionRestrictions,
}

/// Immutable pre-admission intent. These fields select expected grants; only
/// the native grant owner supplies their current lineage and authority.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowParentAuthoritySelection {
    pub commission_sha256: String,
    pub instance: meerkat_toolkit::host_lifetime::policy::CompiledMemberClassInstance,
    pub authority: WorkflowHelperAuthorityRequest,
}

/// One finite envelope derived from the installed SharedWorkflow class. It is
/// usable by G only after G's actual materialization and independent issuance.
/// P may hold the same operation ceiling as a named contributor before then.
pub fn workflow_operation_envelope(
    native: &CompiledNativeHost,
    instance: &meerkat_toolkit::host_lifetime::policy::CompiledMemberClassInstance,
    not_before_ms: u64,
    expires_at_ms: u64,
) -> Result<ExecutionRestrictions, HostLifetimeError> {
    use meerkat_toolkit::host_lifetime::policy::CompiledMemberClassPurpose;
    let class = native
        .member_classes
        .iter()
        .find(|class| class.id == instance.class)
        .ok_or(HostLifetimeError::Denied)?;
    class.materialize(&native.realm, instance)?;
    if class.purpose != CompiledMemberClassPurpose::SharedWorkflow
        || !class.callbacks.is_empty()
        || instance.member.setup_capability.is_some()
        || not_before_ms >= expires_at_ms
        || expires_at_ms == u64::MAX
        || class
            .grants
            .iter()
            .any(|grant| grant.action != meerkat_core::CompiledMemberToolAction::Invoke)
        || [
            "tool_catalog_search",
            "tool_catalog_load",
            "toolkit_request_public_service",
        ]
        .iter()
        .any(|name| !class.grants.iter().any(|grant| grant.tool_name == *name))
    {
        return Err(HostLifetimeError::Denied);
    }
    let mut actions = class
        .grants
        .iter()
        .filter(|grant| {
            ![
                "tool_catalog_search",
                "tool_catalog_load",
                "toolkit_request_public_service",
            ]
            .contains(&grant.tool_name.as_str())
        })
        .map(|grant| ActionRef {
            feature: "mcp".into(),
            action: grant.tool_name.clone(),
        })
        .collect::<Vec<_>>();
    if actions.is_empty() {
        return Err(HostLifetimeError::Denied);
    }
    actions.extend([
        ActionRef {
            feature: "toolkit".into(),
            action: "catalog-read".into(),
        },
        ActionRef {
            feature: "toolkit".into(),
            action: "setup-public-service-request".into(),
        },
        meerkat_authorization::mcp_installation::mcp_connector_action(
            "toolkit-setup",
            &meerkat_core::mcp_config::McpConnectorApproval::NoConnector,
        ),
    ]);
    Ok(ExecutionRestrictions {
        actions: ExactRestriction::exact(actions),
        resource_domains: DomainRestriction::owned_by([native.grant_administrator.clone()]),
        processors: ExactRestriction::exact([ProcessorRef::Principal {
            principal: instance.member.logical_owner.clone(),
        }]),
        audiences: ExactRestriction::exact([instance.member.audience.clone()]),
        lifetime: meerkat_authorization_contracts::constraints::LifetimeRestriction::window(
            not_before_ms,
            expires_at_ms,
        ),
        delegation_depth: DelegationDepth::remaining(0),
    })
}

impl OperatorSetup {
    /// Prepare this exact P before its first admission. Persist immutable intent
    /// before issuance so a lost reply can reconcile only the same native IDs.
    /// This neither creates G nor grants any G surface to P.
    pub async fn configure_workflow_parent(
        &self,
        originals: &Arc<PrivateOriginals>,
        parent: &crate::private_originals::PrivateOriginalRef,
        selection: &WorkflowParentAuthoritySelection,
    ) -> Result<(), HostLifetimeError> {
        originals.workflow_helper_owner(&self.source, &self.originals, &self.lifetime)?;
        let LifetimeBound::Window {
            not_before_ms,
            expires_at_ms,
        } = selection.authority.envelope.lifetime.bound()
        else {
            return Err(HostLifetimeError::Denied);
        };
        let expected = workflow_operation_envelope(
            &self.native,
            &selection.instance,
            not_before_ms,
            expires_at_ms,
        )?;
        if expected != selection.authority.envelope
            || selection.authority.parent_issuance_id == selection.authority.leaf_issuance_id
        {
            return Err(HostLifetimeError::Denied);
        }
        originals
            .select_workflow_parent_authority(parent, selection.clone())
            .await
            .map_err(private_error)?;
        let route = self
            .native
            .routes
            .iter()
            .find(|route| route.id == parent.route)
            .ok_or(HostLifetimeError::Denied)?;
        let mut root_bounds = expected.clone();
        root_bounds.delegation_depth = DelegationDepth::remaining(1);
        // Same source writer as first binding/cancellation; no asynchronous gap
        // can select a different original while the actual grant effects enter.
        originals
            .with_workflow_parent_selection(parent, selection, || {
                self.lifetime.with_current_setup(|| {
                    let grants = self.source.grants();
                    let root = issue_root(
                        &grants,
                        &self.operator,
                        &self.native.grant_administrator,
                        route.represented_subject.as_ref(),
                        &selection.authority.parent_issuance_id,
                        &root_bounds,
                    )?;
                    let leaf = match grants
                        .issued_reference(&selection.authority.leaf_issuance_id)
                        .map_err(refusal)?
                    {
                        Some(leaf) => leaf,
                        None => grants
                            .issue_child(
                                &self.native.grant_administrator,
                                &root,
                                selection.authority.leaf_issuance_id.clone(),
                                route.logical_executor.clone(),
                                expected.clone(),
                            )
                            .map_err(refusal)?,
                    };
                    let resolved = grants
                        .resolve_lineage(
                            &[root, leaf],
                            &route.logical_executor,
                            route.represented_subject.as_ref(),
                        )
                        .map_err(refusal)?;
                    if resolved.restrictions() != &expected {
                        return Err(HostLifetimeError::Denied);
                    }
                    resolved
                        .check_current()
                        .map_err(|_| HostLifetimeError::Unavailable)
                })
            })
            .await
            .map_err(private_error)
    }

    /// Select only an actually completed late member of this installed owner.
    /// The real resource participant supplies its exact no-account server.
    /// No static member or surface is constructed from these observations.
    pub async fn configure_workflow_helper(
        &self,
        originals: &Arc<PrivateOriginals>,
        admitted: Arc<AdmittedMemberClass>,
        server: &McpServerConfig,
        request: &WorkflowHelperAuthorityRequest,
    ) -> Result<InstalledLateWorkflowHelper, HostLifetimeError> {
        originals.workflow_helper_owner(&self.source, &self.originals, &self.lifetime)?;
        let (admission, _) = originals.completed_workflow_class(&admitted)?;
        let member = admission.selected_member()?;
        let route = admission
            .route
            .as_ref()
            .ok_or(HostLifetimeError::Unavailable)?;
        validate_envelope(
            &self.native.grant_administrator,
            &member,
            route,
            admission.native_surface.as_deref(),
            server,
            request,
        )?;
        // Catalog-only selections retain their original single-lineage shape.
        // A selected MCP action needs an explicit connector record as well.
        let needs_mcp = matches!(request.envelope.actions.bound(), SetBound::Exact(actions)
            if actions.iter().any(|action| action.feature == "mcp"));
        if needs_mcp {
            workflow_mcp::target_bounds(
                &self.native.grant_administrator,
                &request.envelope,
                server,
            )?;
        }
        let mut root_bounds = request.envelope.clone();
        root_bounds.delegation_depth = DelegationDepth::remaining(1);
        let grants = self.source.grants();
        let (lineage, mcp_lineage, public_lineage) = originals.with_workflow_helper_grants(&admission, server, |saved| {
            let admitted = &admitted;
            let grants = &grants;
            let root_bounds = &root_bounds;
            async move {
                admitted.owner.with_current_member(admitted.admission_id(), admitted.class(), admitted.instance(), || {
                    self.lifetime.with_current_setup(|| {
                        // A bound legacy G cannot acquire a target on recommission.
                        // All saved references must exist before any new issue.
                        if let Some(saved) = &saved {
                            let (base, target) = workflow_mcp::split(saved)?;
                            if target.is_some() != needs_mcp || base.len() != 2
                                || base[0].grant_id != request.parent_issuance_id
                                || base[1].grant_id != request.leaf_issuance_id
                            { return Err(HostLifetimeError::Denied); }
                            for lineage in saved.grant_lineages() {
                                for reference in lineage {
                                    if grants.issued_reference(&reference.grant_id).map_err(refusal)?.as_ref() != Some(reference) {
                                        return Err(HostLifetimeError::Unavailable);
                                    }
                                }
                            }
                        }
                        originals.completed_workflow_class(admitted)?;
                        let root = issue_root(grants, &self.operator, &self.native.grant_administrator,
                            route.represented_subject.as_ref(), &request.parent_issuance_id, root_bounds)?;
                        let leaf = match grants.issued_reference(&request.leaf_issuance_id).map_err(refusal)? {
                            Some(leaf) => leaf,
                            None => grants.issue_child(&self.native.grant_administrator, &root,
                                request.leaf_issuance_id.clone(), route.logical_executor.clone(), request.envelope.clone()).map_err(refusal)?,
                        };
                        let lineage = vec![root, leaf];
                        let resolved = grants.resolve_lineage(&lineage, &route.logical_executor,
                            route.represented_subject.as_ref()).map_err(refusal)?;
                        if resolved.restrictions() != &request.envelope { return Err(HostLifetimeError::Denied); }
                        resolved.check_current().map_err(|_| HostLifetimeError::Unavailable)?;
                        let target = if needs_mcp {
                            let target_ids = workflow_mcp::target_ids(&lineage)?;
                            if route.controller_parent.as_ref().is_some_and(|controller| controller.iter()
                                .any(|grant| target_ids.contains(&grant.grant_id)))
                            { return Err(HostLifetimeError::Denied); }
                            Some(issue_mcp_target(grants, &self.operator, &self.native.grant_administrator,
                                &route.logical_executor, route.represented_subject.as_ref(), &lineage, &request.envelope, server)?)
                        } else { None };
                        if saved.is_none() && needs_mcp && super::workflow_public::selected(&request.envelope) {
                            let id = super::workflow_public::child_id(&lineage)?;
                            if route.controller_parent.as_ref().is_some_and(|controller|
                                controller.iter().any(|grant| grant.grant_id == id))
                            { return Err(HostLifetimeError::Denied); }
                        }
                        let public = super::workflow_public::select(grants, &self.native.grant_administrator,
                            &route.logical_executor, route.represented_subject.as_ref(), &lineage,
                            &request.envelope, saved.as_ref(), needs_mcp)?;
                        let basis = match &target {
                            Some(target) => workflow_mcp::basis_with_public(lineage.clone(), target.clone(), public.clone()),
                            None => meerkat_authorization_contracts::work_association::WorkAuthorityBasis::GrantLineage {
                                lineage: lineage.clone(),
                            },
                        };
                        if saved.as_ref().is_some_and(|saved| saved != &basis) { return Err(HostLifetimeError::Denied); }
                        Ok((lineage, target, public))
                    })
                }).await
            }
        }).await?;
        let selected = InstalledLateWorkflowHelper {
            source: Arc::downgrade(originals),
            admitted,
            current_resource: admission.workflow_resource.clone(),
            admission,
            server: server.clone(),
            envelope: request.envelope.clone(),
            lineage,
            mcp_lineage,
            public_lineage,
        };
        selected.check(originals)?;
        Ok(selected)
    }

    /// Fresh, separately selected A roots. The current helper receipt supplies
    /// destination/surface facts only; it does not authorize A's operation set.
    pub async fn configure_late_workflow_setup(
        &self,
        originals: &Arc<PrivateOriginals>,
        helper: &InstalledLateWorkflowHelper,
        request: &SharedWorkflowSetupRequest,
    ) -> Result<InstalledLateWorkflowWaiter, HostLifetimeError> {
        originals.workflow_helper_owner(&self.source, &self.originals, &self.lifetime)?;
        helper.check(originals)?;
        let capability = helper
            .admitted
            .owner
            .with_current_member(
                helper.admitted.admission_id(),
                helper.admitted.class(),
                helper.admitted.instance(),
                || {
                    self.lifetime.with_current_setup(|| {
                        helper.check(originals)?;
                        let selected = self.lifetime.selected_policy();
                        let native = selected
                            .native_host
                            .as_ref()
                            .ok_or(HostLifetimeError::Unavailable)?;
                        configured_setup::configure_late_prepared(
                            &native.preparation(),
                            &selected.channels,
                            &self.source,
                            &self.operator,
                            request,
                            helper,
                        )
                    })
                },
            )
            .await?;
        Ok(InstalledLateWorkflowWaiter {
            helper: helper.clone(),
            capability,
        })
    }
}

fn validate_envelope(
    application: &meerkat_core::PrincipalRef,
    member: &meerkat_toolkit::host_lifetime::policy::PreparedNativeMember,
    route: &meerkat_toolkit::host_lifetime::policy::CompiledNativeSourceRoute,
    surface: Option<&[meerkat_core::ToolIdentity]>,
    server: &McpServerConfig,
    request: &WorkflowHelperAuthorityRequest,
) -> Result<(), HostLifetimeError> {
    let access = server.access.as_ref().ok_or(HostLifetimeError::Denied)?;
    if request.parent_issuance_id == request.leaf_issuance_id
        || server.name.is_empty()
        || server.connector.is_some()
        || access.allowed_operations.is_empty()
        || access
            .allowed_operations
            .iter()
            .any(|name| name.trim().is_empty() || name.trim() != name)
        || surface.is_none_or(|surface| {
            !surface.iter().any(|tool| {
                tool.provenance.as_ref().is_some_and(|provenance| {
                    provenance.kind == ToolSourceKind::Mcp
                        && provenance.source_id.as_str() == server.name
                })
            })
        })
    {
        return Err(HostLifetimeError::Denied);
    }
    let mcp = application_operations::mcp_access_restrictions(
        application,
        &route.logical_executor,
        &member.audience,
        &server.name,
        access.allowed_operations.iter().map(String::as_str),
        &meerkat_core::mcp_config::McpConnectorApproval::NoConnector,
    );
    let SetBound::Exact(mcp_actions) = mcp.actions.bound() else {
        return Err(HostLifetimeError::Denied);
    };
    let allowed = mcp_actions
        .iter()
        .cloned()
        .chain([
            ActionRef {
                feature: "toolkit".into(),
                action: "catalog-read".into(),
            },
            ActionRef {
                feature: "toolkit".into(),
                action: "setup-public-service-request".into(),
            },
        ])
        .collect::<std::collections::BTreeSet<_>>();
    let bounds = &request.envelope;
    let SetBound::Exact(actions) = bounds.actions.bound() else {
        return Err(HostLifetimeError::Denied);
    };
    if actions.is_empty()
        || !actions.is_subset(&allowed)
        || !bounds.actions.unresolved_facts().is_empty()
        || bounds.resource_domains != DomainRestriction::owned_by([application.clone()])
        || bounds.processors
            != ExactRestriction::exact([ProcessorRef::Principal {
                principal: route.logical_executor.clone(),
            }])
        || bounds.audiences != ExactRestriction::exact([member.audience.clone()])
        || bounds.delegation_depth != DelegationDepth::remaining(0)
        || !bounds.lifetime.unresolved_facts().is_empty()
        || !matches!(bounds.lifetime.bound(), LifetimeBound::Window { not_before_ms, expires_at_ms }
            if not_before_ms < expires_at_ms && expires_at_ms < u64::MAX)
    {
        return Err(HostLifetimeError::Denied);
    }
    Ok(())
}

// Actual target authority is independently issued under the same selected
// operator. Owner-scoped base bounds cannot record a connector approval.
fn issue_mcp_target(
    grants: &meerkat_authorization::grants::LocalGrantAuthority,
    operator: &meerkat_core::PrincipalRef,
    application: &meerkat_core::PrincipalRef,
    executor: &meerkat_core::PrincipalRef,
    represented: Option<&meerkat_core::PrincipalRef>,
    base: &[GrantLineageRef],
    envelope: &ExecutionRestrictions,
    server: &McpServerConfig,
) -> Result<Vec<GrantLineageRef>, HostLifetimeError> {
    let ids = workflow_mcp::target_ids(base)?;
    let expected = workflow_mcp::target_bounds(application, envelope, server)?;
    let mut root_bounds = expected.clone();
    root_bounds.delegation_depth = DelegationDepth::remaining(1);
    let root = issue_root(
        grants,
        operator,
        application,
        represented,
        &ids[0],
        &root_bounds,
    )?;
    let leaf = match grants.issued_reference(&ids[1]).map_err(refusal)? {
        Some(leaf) => leaf,
        None => grants
            .issue_child(
                application,
                &root,
                ids[1].clone(),
                executor.clone(),
                expected,
            )
            .map_err(refusal)?,
    };
    let target = vec![root, leaf];
    workflow_mcp::resolve_target(
        grants,
        application,
        executor,
        represented,
        base,
        &target,
        envelope,
        server,
    )?;
    Ok(target)
}

#[cfg(test)]
pub(crate) fn issue_mcp_target_for_policy_test(
    grants: &meerkat_authorization::grants::LocalGrantAuthority,
    operator: &meerkat_core::PrincipalRef,
    application: &meerkat_core::PrincipalRef,
    executor: &meerkat_core::PrincipalRef,
    base: &[GrantLineageRef],
    envelope: &ExecutionRestrictions,
    server: &McpServerConfig,
) -> Vec<GrantLineageRef> {
    issue_mcp_target(
        grants,
        operator,
        application,
        executor,
        None,
        base,
        envelope,
        server,
    )
    .unwrap()
}

// Same exact native issuance/reconciliation as configured_setup. Partial
// issuance remains named by the caller-retained IDs; failure never substitutes
// new IDs or reports that no native effect occurred.
fn issue_root(
    grants: &meerkat_authorization::grants::LocalGrantAuthority,
    operator: &meerkat_core::PrincipalRef,
    grantee: &meerkat_core::PrincipalRef,
    represented: Option<&meerkat_core::PrincipalRef>,
    id: &EvidenceId,
    expected: &ExecutionRestrictions,
) -> Result<GrantLineageRef, HostLifetimeError> {
    let root = match grants.issued_reference(id).map_err(refusal)? {
        Some(root) => root,
        None => grants
            .issue_root(
                operator,
                id.clone(),
                grantee.clone(),
                represented.cloned(),
                expected.clone(),
            )
            .map_err(refusal)?,
    };
    if &root.root_authority != operator {
        return Err(HostLifetimeError::Denied);
    }
    let resolved = grants
        .resolve_lineage(std::slice::from_ref(&root), grantee, represented)
        .map_err(refusal)?;
    if resolved.restrictions() != expected {
        return Err(HostLifetimeError::Denied);
    }
    resolved
        .check_current()
        .map_err(|_| HostLifetimeError::Unavailable)?;
    Ok(root)
}
fn private_error(error: crate::private_originals::PrivateOriginalError) -> HostLifetimeError {
    match error {
        crate::private_originals::PrivateOriginalError::Unavailable => {
            HostLifetimeError::Unavailable
        }
        _ => HostLifetimeError::Denied,
    }
}
fn refusal(error: GrantRefusal) -> HostLifetimeError {
    match error {
        GrantRefusal::Unavailable => HostLifetimeError::Unavailable,
        _ => HostLifetimeError::Denied,
    }
}

#[cfg(test)]
mod tests;
