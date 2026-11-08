//! Pure host projection plus a read of the already selected WorkGraph.
use super::*;
use meerkat::{WorkGraphError, WorkGraphMachine};
use meerkat_authorization_contracts::constraints::{AudienceRef, ExecutionRestrictions};
use meerkat_core::{CompiledMemberToolAction, McpServerConfig, PrincipalRef};
use meerkat_toolkit::host_lifetime::policy::{
    CompiledMemberClassPurpose, CompiledNativeHost, CompiledNativeSourceOrigin,
    CompiledNativeSourceRoute,
};

pub(super) struct Selection {
    pub route: CompiledNativeSourceRoute,
    pub requester: PrincipalRef,
    pub profile: String,
    pub operations: BTreeSet<String>,
    pub envelope: ExecutionRestrictions,
}

pub(super) fn select(
    native: &CompiledNativeHost,
    request: &WorkflowCommissionRequest,
    now: u64,
) -> Result<Selection, HostLifetimeError> {
    let class = native
        .member_classes
        .iter()
        .find(|class| class.id == request.instance.class)
        .ok_or(HostLifetimeError::Denied)?;
    class.materialize(&native.realm, &request.instance)?;
    let parent = native
        .routes
        .iter()
        .find(|route| route.id == request.parent.original.route)
        .ok_or(HostLifetimeError::Denied)?;
    let CompiledNativeSourceOrigin::LocalConsole {
        requester,
        ingress_actor,
        member,
    } = &parent.origin
    else {
        return Err(HostLifetimeError::Denied);
    };
    if class.purpose != CompiledMemberClassPurpose::SharedWorkflow
        || !class.callbacks.is_empty()
        || request.instance.member.setup_capability.is_some()
        || request.instance.member.member.mob_id != member.mob_id
        || meerkat_mobkit::member_comms_id::roster_member_id_for_identity(request.identity.as_str())
            .as_str()
            != request.instance.member.member.member
        || request.instance.member.audience
            != (AudienceRef::Principal {
                principal: requester.clone(),
            })
        || request.event.route == request.parent.original.route
        || request.event.original == request.parent.original.original
        || request.event.contributors.as_slice() != [request.parent.original.original.clone()]
        || request.event.content_sha256
            != ContentDigest::of_bytes(request.content.as_bytes()).to_string()
        || request.not_before_ms > now
        || now >= request.deadline_ms
        || request.not_before_ms >= request.expires_at_ms
        || request.expires_at_ms == u64::MAX
        || request.deadline_ms > request.expires_at_ms
        || request.max_waiters == 0
        || !(1..=60_000).contains(&request.hold_timeout_ms)
        || request.controller_issuance_id == request.planning_root_issuance_id
        || request.controller_issuance_id == request.planning_leaf_issuance_id
        || request.planning_root_issuance_id == request.planning_leaf_issuance_id
        || class
            .grants
            .iter()
            .any(|grant| grant.action != CompiledMemberToolAction::Invoke)
    {
        return Err(HostLifetimeError::Denied);
    }
    let has = |name: &str| class.grants.iter().any(|grant| grant.tool_name == name);
    if !has("tool_catalog_search")
        || !has("tool_catalog_load")
        || !has("toolkit_request_public_service")
    {
        return Err(HostLifetimeError::Denied);
    }
    let operations = class
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
        .map(|grant| grant.tool_name.clone())
        .collect::<BTreeSet<_>>();
    if operations.is_empty() {
        return Err(HostLifetimeError::Denied);
    }
    let route = CompiledNativeSourceRoute {
        id: request.event.route.clone(),
        origin: CompiledNativeSourceOrigin::LocalConsole {
            member: request.instance.member.member.clone(),
            requester: requester.clone(),
            ingress_actor: ingress_actor.clone(),
        },
        application_parent: None,
        controller_parent: None,
        logical_executor: request.instance.member.logical_owner.clone(),
        represented_subject: parent.represented_subject.clone(),
        source_authority: parent.source_authority.clone(),
        authentication_domain: parent.authentication_domain.clone(),
        event_domain: parent.event_domain.clone(),
    };
    let envelope = toolkit_mobkit::installed_host::operator_setup::workflow_operation_envelope(
        native,
        &request.instance,
        request.not_before_ms,
        request.expires_at_ms,
    )?;
    Ok(Selection {
        route,
        requester: requester.clone(),
        profile: class.profile.clone(),
        operations,
        envelope,
    })
}

pub(super) fn matching_peer(server: &McpServerConfig, operations: &BTreeSet<String>) -> bool {
    server.name == "toolkit-setup"
        && server.connector.is_none()
        && server.access.as_ref().is_some_and(|access| {
            access
                .allowed_operations
                .iter()
                .cloned()
                .collect::<BTreeSet<_>>()
                == *operations
        })
}

pub(super) async fn check_parent_work(
    backend: &SetupWorkBackend,
    request: &WorkflowCommissionRequest,
) -> Result<(), HostLifetimeError> {
    let parent = &request.parent_work;
    if parent.realm_id != backend.pin().realm().as_str()
        || &parent.namespace != backend.pin().namespace()
        || request.cycle_label.is_empty()
        || request.cycle_label.len() > 128
        || request.cycle_label.trim() != request.cycle_label
        || request.cycle_label.chars().any(char::is_control)
    {
        return Err(HostLifetimeError::Denied);
    }
    let row = backend
        .service()
        .get(
            Some(parent.realm_id.clone()),
            Some(parent.namespace.clone()),
            parent.item_id.clone(),
        )
        .await
        .map_err(|error| match error {
            WorkGraphError::NotFound { .. } => HostLifetimeError::Denied,
            _ => HostLifetimeError::Unavailable,
        })?;
    if row.realm_id != parent.realm_id
        || row.namespace != parent.namespace
        || row.id != parent.item_id
        || !row.labels.contains(&request.cycle_label)
        || row.owner.as_ref().map(|owner| &owner.key) != Some(&request.parent_owner)
        || WorkGraphMachine::classify_terminality(&row)
            .map_err(|_| HostLifetimeError::Unavailable)?
    {
        return Err(HostLifetimeError::Denied);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
