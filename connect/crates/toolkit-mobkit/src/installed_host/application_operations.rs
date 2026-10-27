//! Toolkit operation vocabulary shared by issuance and invocation.
//!
//! These functions map actual typed facts. They neither look at grant bounds
//! to guess a target nor issue permission. The native grant owner conjoins the
//! exact resulting tuple with the separately reviewed grant at each boundary.

use meerkat_authorization_contracts::constraints::DomainRestriction;
use meerkat_authorization::policy::LocalOperationValues;
use meerkat_authorization_contracts::constraints::{
    ActionRef, AudienceRef, DelegationDepth, ExactRestriction, ExecutionRestrictions,
    LifetimeRestriction, ProcessorRef, ResourceDomain,
};
use meerkat_core::{ControllerRouteKey, PrincipalRef, SessionId};

pub(crate) enum ControllerAction {
    Inference,
    Compaction,
}
impl ControllerAction {
    fn name(&self) -> &'static str {
        match self {
            Self::Inference => "controller-infer",
            Self::Compaction => "context-compact",
        }
    }
}

fn tuple(
    authority: &PrincipalRef,
    executor: &PrincipalRef,
    audience: &AudienceRef,
    action: &str,
    namespace: String,
) -> LocalOperationValues {
    LocalOperationValues {
        action: ActionRef {
            feature: "toolkit".into(),
            action: action.into(),
        },
        resource_domain: ResourceDomain {
            authority: authority.clone(),
            namespace,
        },
        processor: ProcessorRef::Principal {
            principal: executor.clone(),
        },
        audience: audience.clone(),
    }
}

/// Native canonical selected route. Credential identity is one fact in this
/// coordinate, never proof of account permission. The exact reviewed grant
/// still has to cover this resource, action, processor and audience together.
pub(crate) fn controller(
    authority: &PrincipalRef,
    executor: &PrincipalRef,
    audience: &AudienceRef,
    route: &ControllerRouteKey,
    action: ControllerAction,
) -> LocalOperationValues {
    tuple(
        authority,
        executor,
        audience,
        action.name(),
        format!("toolkit-model-route/{route}"),
    )
}

pub(super) fn memory_read(
    authority: &PrincipalRef,
    executor: &PrincipalRef,
    audience: &AudienceRef,
    session: &SessionId,
) -> LocalOperationValues {
    tuple(
        authority,
        executor,
        audience,
        "memory-read",
        format!("toolkit-session-memory/{session}"),
    )
}

/// Search and Load expose this bound session's catalog. Loading changes its
/// visible tool surface only; it does not authorize any loaded tool to run.
pub(crate) fn catalog_read(
    authority: &PrincipalRef,
    executor: &PrincipalRef,
    audience: &AudienceRef,
    session: &SessionId,
) -> LocalOperationValues {
    tuple(
        authority,
        executor,
        audience,
        "catalog-read",
        format!("toolkit-session-catalog/{session}"),
    )
}

/// Explicit host selection for catalog reads by this helper's successors.
/// Only the selected application receives this one-level parent over its own
/// domains; the native binder narrows it to the observed helper session.
pub(crate) fn configured_catalog_parent(application: &PrincipalRef, executor: &PrincipalRef, audience: &AudienceRef)
    -> ExecutionRestrictions
{
    ExecutionRestrictions {
        actions: ExactRestriction::exact([ActionRef { feature: "toolkit".into(), action: "catalog-read".into() }]),
        resource_domains: DomainRestriction::owned_by([application.clone()]),
        processors: ExactRestriction::exact([ProcessorRef::Principal { principal: executor.clone() }]),
        audiences: ExactRestriction::exact([audience.clone()]),
        lifetime: LifetimeRestriction::unrestricted(),
        delegation_depth: DelegationDepth::remaining(1),
    }
}

/// Admin-issued capability for the exact setup action and selected destination.
pub(crate) fn configured_setup_parent(
    application: &PrincipalRef,
    executor: &PrincipalRef,
    audience: &AudienceRef,
) -> ExecutionRestrictions {
    ExecutionRestrictions {
        actions: ExactRestriction::exact([setup_request_action()]),
        // Sessions do not exist at configuration. The selected application may
        // delegate this action only to its own exact session resources.
        resource_domains: DomainRestriction::owned_by([application.clone()]),
        processors: ExactRestriction::exact([ProcessorRef::Principal { principal: executor.clone() }]),
        audiences: ExactRestriction::exact([audience.clone()]),
        lifetime: LifetimeRestriction::unrestricted(),
        delegation_depth: DelegationDepth::remaining(1),
    }
}

fn setup_request_action() -> ActionRef {
    ActionRef { feature: "toolkit".into(), action: "find-and-install".into() }
}

/// Request acquisition from this exact bound agent session. This operation
/// permits neither a provider call nor installation effects by itself. Native
/// helper admission, protected review and downstream grants remain separate.
pub(crate) fn find_and_install(
    authority: &PrincipalRef,
    executor: &PrincipalRef,
    audience: &AudienceRef,
    session: &SessionId,
) -> LocalOperationValues {
    tuple(
        authority,
        executor,
        audience,
        &setup_request_action().action,
        format!("toolkit-session-setup/{session}"),
    )
}

/// Historical session-coordinate fixture only. Production report/account
/// requests use the qualified canonical origin below. Existing fixture grants
/// do not qualify the new correlated-report path.
#[cfg(test)]
pub(crate) fn setup_report(
    authority: &PrincipalRef,
    executor: &PrincipalRef,
    audience: &AudienceRef,
    session: &SessionId,
) -> LocalOperationValues {
    tuple(
        authority,
        executor,
        audience,
        "setup-report",
        format!("toolkit-session-setup/{session}"),
    )
}

/// The existing resource authority owns this namespace. The full qualified
/// original is retained and validated by the source owner, not reconstructed
/// from a namespace. Hex encoding the canonical bytes is an exact coordinate;
/// it conveys neither source ownership nor current permission.
fn qualified_setup_operation(
    authority: &PrincipalRef,
    executor: &PrincipalRef,
    audience: &AudienceRef,
    origin: &crate::original_ingress::helper_originals::HelperSetupOrigin,
    action: &str,
) -> Result<LocalOperationValues, serde_json::Error> {
    let mut values = tuple(authority, executor, audience, action, String::new());
    values.resource_domain = qualified_setup_resource(authority, origin, action)?;
    Ok(values)
}

pub(crate) fn qualified_setup_resource(
    authority: &PrincipalRef,
    origin: &crate::original_ingress::helper_originals::HelperSetupOrigin,
    action: &str,
) -> Result<ResourceDomain, serde_json::Error> {
    let canonical = toolkit_package::canonical_json(origin)?;
    let coordinate: String = canonical.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(ResourceDomain { authority: authority.clone(), namespace: format!("toolkit-{action}-origin/v1/{coordinate}") })
}

/// Report scope can be reviewed before a lazy helper session exists. The
/// actual binding and dispatch still validate its native target separately.
/// This operation grants no installation, readiness or delivery permission.
pub(crate) fn setup_report_for_origin(
    authority: &PrincipalRef,
    executor: &PrincipalRef,
    audience: &AudienceRef,
    origin: &crate::original_ingress::helper_originals::HelperSetupOrigin,
) -> Result<LocalOperationValues, serde_json::Error> {
    qualified_setup_operation(authority, executor, audience, origin, "setup-report")
}

/// Request the protected account ceremony for the same designated original,
/// using a distinct action and resource. Later human/account/use authority is
/// checked by its existing owner and is never inferred from this coordinate.
pub(crate) fn setup_account_request_for_origin(
    authority: &PrincipalRef,
    executor: &PrincipalRef,
    audience: &AudienceRef,
    origin: &crate::original_ingress::helper_originals::HelperSetupOrigin,
) -> Result<LocalOperationValues, serde_json::Error> {
    qualified_setup_operation(authority, executor, audience, origin, "setup-account-request")
}

/// Request protected review of a public service for the same original. This
/// action supplies no account ceremony, service use or attachment authority.
pub(crate) fn setup_public_service_request_for_origin(
    authority: &PrincipalRef,
    executor: &PrincipalRef,
    audience: &AudienceRef,
    origin: &crate::original_ingress::helper_originals::HelperSetupOrigin,
) -> Result<LocalOperationValues, serde_json::Error> {
    qualified_setup_operation(authority, executor, audience, origin, "setup-public-service-request")
}

/// Exact registered server and raw operation supplied by the native live
/// dispatcher witness. Connection generation stays in that witness and is
/// revalidated by its owner at dispatch; it is not durable grant identity.
/// This operation tuple grants neither startup attachment nor account access.
pub(crate) fn mcp_operation(
    authority: &PrincipalRef,
    executor: &PrincipalRef,
    audience: &AudienceRef,
    server: &str,
    raw_operation: &str,
) -> LocalOperationValues {
    LocalOperationValues {
        action: ActionRef {
            feature: "mcp".into(),
            action: raw_operation.into(),
        },
        resource_domain: ResourceDomain {
            authority: authority.clone(),
            namespace: format!("mcp-server/{server}"),
        },
        processor: ProcessorRef::Principal { principal: executor.clone() },
        audience: audience.clone(),
    }
}

/// Companion to a raw operation on the same native installation domain.
/// The native grant policy requires an explicit connector approval throughout
/// the selected lineage; wildcard bounds and absent records cannot supply it.
pub(crate) fn mcp_connector_operation(
    authority: &PrincipalRef,
    executor: &PrincipalRef,
    audience: &AudienceRef,
    server: &str,
    connector: &meerkat_core::mcp_config::McpConnectorApproval,
) -> LocalOperationValues {
    let mut values = mcp_operation(authority, executor, audience, server, "");
    values.action = meerkat_authorization::mcp_installation::mcp_connector_action(server, connector);
    values
}

/// The explicitly reviewed live effect has a distinct action and exact
/// server/configuration/destination domain. Ordinary tool use cannot supply it.
pub(crate) fn mcp_live_attachment(
    authority: &PrincipalRef,
    executor: &PrincipalRef,
    audience: &AudienceRef,
    selection_digest: &str,
) -> LocalOperationValues {
    tuple(authority, executor, audience, "mcp-live-attach",
        format!("toolkit-mcp-live/{selection_digest}"))
}

/// Native MCP vocabulary shared by the explicit access decision and use.
/// The connector domain records approval; it is not another operation grant.
pub(crate) fn mcp_access_restrictions<'a>(
    authority: &PrincipalRef,
    executor: &PrincipalRef,
    audience: &AudienceRef,
    server: &str,
    raw_operations: impl IntoIterator<Item = &'a str>,
    connector: &meerkat_core::mcp_config::McpConnectorApproval,
) -> ExecutionRestrictions {
    mcp_access_restrictions_with_attachment(authority, executor, audience, server,
        raw_operations, connector, None)
}

pub(crate) fn mcp_access_restrictions_with_attachment<'a>(
    authority: &PrincipalRef,
    executor: &PrincipalRef,
    audience: &AudienceRef,
    server: &str,
    raw_operations: impl IntoIterator<Item = &'a str>,
    connector: &meerkat_core::mcp_config::McpConnectorApproval,
    attachment: Option<LocalOperationValues>,
) -> ExecutionRestrictions {
    mcp_access_restrictions_with_service_scope(authority, executor, audience, server,
        raw_operations, connector, attachment, None)
}

pub(crate) fn mcp_access_restrictions_with_service_scope<'a>(
    authority: &PrincipalRef, executor: &PrincipalRef, audience: &AudienceRef,
    server: &str, raw_operations: impl IntoIterator<Item = &'a str>,
    connector: &meerkat_core::mcp_config::McpConnectorApproval,
    attachment: Option<LocalOperationValues>, catalog: Option<LocalOperationValues>,
) -> ExecutionRestrictions {
    let operation = mcp_operation(authority, executor, audience, server, "");
    let actions = raw_operations.into_iter().map(|raw| ActionRef { feature: "mcp".into(), action: raw.into() });
    let approval = meerkat_authorization::mcp_installation::mcp_connector_action(server, connector);
    let connector_domain = meerkat_authorization::mcp_installation::mcp_connector_domain(authority, server, connector);
    ExecutionRestrictions {
        actions: ExactRestriction::exact(actions.chain(std::iter::once(approval))
            .chain(attachment.iter().map(|value| value.action.clone()))
            .chain(catalog.iter().map(|value| value.action.clone()))),
        resource_domains: DomainRestriction::exact([operation.resource_domain, connector_domain].into_iter()
            .chain(attachment.iter().map(|value| value.resource_domain.clone()))
            .chain(catalog.iter().map(|value| value.resource_domain.clone()))),
        processors: ExactRestriction::exact([operation.processor]),
        audiences: ExactRestriction::exact([operation.audience]),
        lifetime: LifetimeRestriction::unrestricted(),
        delegation_depth: DelegationDepth::remaining(0),
    }
}

/// A single correlated tuple cannot gain cross-product permissions. This is
/// the operator-to-application parent for one direct application-to-agent leaf;
/// the existing review/issuance owner decides whether to issue it at all.
pub(crate) fn parent_restrictions(values: LocalOperationValues) -> ExecutionRestrictions {
    ExecutionRestrictions {
        actions: ExactRestriction::exact([values.action]),
        resource_domains: DomainRestriction::exact([values.resource_domain]),
        processors: ExactRestriction::exact([values.processor]),
        audiences: ExactRestriction::exact([values.audience]),
        lifetime: LifetimeRestriction::unrestricted(),
        delegation_depth: DelegationDepth::remaining(1),
    }
}

#[cfg(test)]
#[path = "application_operations_owned_domains_tests.rs"]
mod owned_domains_tests;
