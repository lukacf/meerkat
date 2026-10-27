//! Explicit admin requests on the same stopped application's native owner.
//! Returned references are compiler inputs, not a declaration of currentness.
use super::*;
use crate::installed_host::acquisition::InstalledSetupSelection;
use crate::installed_host::operator_setup::{AgentSetupCapabilityRequest, HelperReportCapabilityRequest, setup_targets};
use meerkat_authorization::grants::GrantRefusal;
use meerkat_authorization_contracts::{evidence::EvidenceId, grant::GrantLineageRef};
use meerkat_core::ControllerRouteKey;
use serde::{Deserialize, Serialize};

/// Requests are supplied and retained by the trusted application's explicit
/// configure/install transaction. An absent request grants nothing. Reusing an
/// issuance ID requires the exact same native parent and complete restrictions.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparationAuthorities {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub helper_mcp: Option<PreparedHelperMcpRequest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub helper_catalog: Option<PreparedHelperCatalogRequest>,
    #[serde(default)]
    pub controllers: Vec<PreparedControllerRequest>,
    #[serde(default)]
    pub agent_setup: Vec<AgentSetupCapabilityRequest>,
    pub helper_report: Option<HelperReportCapabilityRequest>,
}

/// Retained application requests before actual controller routes are available.
/// An explicit legacy route key constrains the eventual observation; it is not
/// evidence that a native member has been constructed or pinned that client.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparationAuthorityIntent {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub helper_mcp: Option<PreparedHelperMcpRequest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub helper_catalog: Option<PreparedHelperCatalogRequest>,
    #[serde(default)]
    pub controllers: Vec<PreparedControllerIntent>,
    #[serde(default)]
    pub agent_setup: Vec<AgentSetupCapabilityRequest>,
    pub helper_report: Option<HelperReportCapabilityRequest>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedControllerIntent {
    pub target: PreparedControllerTarget,
    pub issuance_id: EvidenceId,
    #[serde(default, with = "optional_controller_key", skip_serializing_if = "Option::is_none")]
    pub controller_route: Option<ControllerRouteKey>,
}

/// Fully resolved input to the existing native issuer. Direct trusted callers
/// with already selected canonical keys retain this request type unchanged.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedControllerRequest {
    pub target: PreparedControllerTarget,
    pub issuance_id: EvidenceId,
    /// Native canonical key, explicitly selected by the trusted admin. It is
    /// never inferred from model configuration or fabricated client facts.
    #[serde(with = "controller_key")]
    pub controller_route: ControllerRouteKey,
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PreparedControllerTarget {
    OriginalRoute { route: toolkit_package::Id },
    Helper,
}

mod controller_key {
    use super::*;
    pub fn serialize<S: serde::Serializer>(key: &ControllerRouteKey, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(key.as_str())
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<ControllerRouteKey, D::Error> {
        let value = String::deserialize(deserializer)?;
        ControllerRouteKey::parse(&value).ok_or_else(|| serde::de::Error::custom("invalid native controller route key"))
    }
}

mod optional_controller_key {
    use super::*;
    pub fn serialize<S: serde::Serializer>(key: &Option<ControllerRouteKey>, serializer: S) -> Result<S::Ok, S::Error> {
        key.as_ref().map(ControllerRouteKey::as_str).serialize(serializer)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<ControllerRouteKey>, D::Error> {
        Option::<String>::deserialize(deserializer)?.map(|value|
            ControllerRouteKey::parse(&value).ok_or_else(|| serde::de::Error::custom("invalid native controller route key"))
        ).transpose()
    }
}

/// Explicit trusted-administrator request for the compiler's bundled setup
/// server. The retained request contains no server, operation, actor or bounds;
/// the actual packaged-resource compiler supplies that finite selection.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedHelperMcpRequest {
    pub issuance_id: EvidenceId,
}

/// Explicit catalog-read request. Exact-session behavior remains the default;
/// successor access must be selected by the trusted host administrator.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedHelperCatalogRequest {
    pub issuance_id: EvidenceId,
    #[serde(default, skip_serializing_if = "PreparedHelperCatalogScope::is_exact_session")]
    pub scope: PreparedHelperCatalogScope,
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreparedHelperCatalogScope {
    #[default]
    ExactSession,
    CurrentHelperSuccessors,
}

impl PreparedHelperCatalogScope {
    fn is_exact_session(&self) -> bool { *self == Self::ExactSession }
}

impl From<PreparationAuthorities> for PreparationAuthorityIntent {
    fn from(requests: PreparationAuthorities) -> Self {
        Self {
            helper_mcp: requests.helper_mcp,
            helper_catalog: requests.helper_catalog,
            controllers: requests.controllers.into_iter().map(|request| PreparedControllerIntent {
                target: request.target, issuance_id: request.issuance_id,
                controller_route: Some(request.controller_route),
            }).collect(),
            agent_setup: requests.agent_setup, helper_report: requests.helper_report,
        }
    }
}

impl PreparationAuthorityIntent {
    /// Validate the complete supplied inventory before yielding any issuance
    /// input. Target/key pairs are trusted caller observations, not native
    /// provenance: the held caller must first join each exact member, finished
    /// surface and actual report. This conversion neither observes nor grants.
    pub fn resolve_controller_routes(&self,
        routes: impl IntoIterator<Item = (PreparedControllerTarget, ControllerRouteKey)>,
    ) -> Result<PreparationAuthorities, HostLifetimeError> {
        let mut targets = std::collections::BTreeSet::new();
        for request in &self.controllers {
            if !targets.insert(request.target.clone()) { return Err(HostLifetimeError::InvalidSelection); }
        }
        let mut observed = std::collections::BTreeMap::new();
        for (target, key) in routes {
            if !targets.contains(&target) || observed.insert(target, key).is_some() {
                return Err(HostLifetimeError::InvalidSelection);
            }
        }
        let controllers = self.controllers.iter().map(|request| {
            let key = observed.get(&request.target).ok_or(HostLifetimeError::Unavailable)?;
            if request.controller_route.as_ref().is_some_and(|expected| expected != key) {
                return Err(HostLifetimeError::Denied);
            }
            Ok(PreparedControllerRequest { target: request.target.clone(),
                issuance_id: request.issuance_id.clone(), controller_route: key.clone() })
        }).collect::<Result<Vec<_>, HostLifetimeError>>()?;
        let resolved = PreparationAuthorities { controllers, helper_mcp: self.helper_mcp.clone(),
            helper_catalog: self.helper_catalog.clone(),
            agent_setup: self.agent_setup.clone(), helper_report: self.helper_report.clone() };
        resolved.validate_inventory()?;
        Ok(resolved)
    }

    /// Preserve the existing trusted known-key configuration path. An omitted
    /// key is unavailable here, never inferred or treated as an empty grant.
    /// Success does not claim a native controller report or live currentness.
    pub fn resolve_known_routes(&self) -> Result<PreparationAuthorities, HostLifetimeError> {
        let routes = self.controllers.iter().map(|request| {
            Ok((request.target.clone(), request.controller_route.clone().ok_or(HostLifetimeError::Unavailable)?))
        }).collect::<Result<Vec<_>, HostLifetimeError>>()?;
        self.resolve_controller_routes(routes)
    }

    /// The same trusted compiler role projection as the resolved request path.
    /// It changes no controller target, issuance ID or capability selection.
    pub fn project_helper_member(&self, previous: &meerkat_core::MobMemberBinding,
        compiled: &meerkat_core::MobMemberBinding) -> Self
    {
        let mut selected = self.clone();
        project_helper_requests(&mut selected.helper_report, &mut selected.agent_setup, previous, compiled);
        selected
    }
}

impl PreparationAuthorities {
    // Shared with intent resolution; the existing issuer remains the owner of
    // request inventory semantics, including every target and issuance ID.
    fn validate_inventory(&self) -> Result<(), HostLifetimeError> {
        let mut targets = std::collections::BTreeSet::new();
        let mut ids = std::collections::BTreeSet::new();
        if let Some(request) = &self.helper_mcp { ids.insert(request.issuance_id.clone()); }
        if self.helper_catalog.as_ref().is_some_and(|request| !ids.insert(request.issuance_id.clone())) {
            return Err(HostLifetimeError::InvalidSelection);
        }
        for request in &self.controllers {
            if !targets.insert(request.target.clone()) || !ids.insert(request.issuance_id.clone()) {
                return Err(HostLifetimeError::InvalidSelection);
            }
        }
        let mut members = std::collections::BTreeSet::new();
        for request in &self.agent_setup {
            setup_targets::validate_targets(request.helper_targets.iter().map(|target| &target.target))?;
            if !members.insert(request.member.member.clone()) || !ids.insert(request.issuance_id.clone())
                || request.helper_targets.iter().any(|target| !ids.insert(target.issuance_id.clone())) {
                return Err(HostLifetimeError::InvalidSelection);
            }
        }
        if self.helper_report.as_ref().is_some_and(|request| !ids.insert(request.issuance_id.clone())
            || request.account_issuance_id.as_ref().is_some_and(|id| !ids.insert(id.clone()))
            || request.public_issuance_id.as_ref().is_some_and(|id| !ids.insert(id.clone()))) {
            return Err(HostLifetimeError::InvalidSelection);
        }
        Ok(())
    }

    /// Apply only the actual compiler's helper member projection. This method
    /// selects no identity and changes no retained route, target or issuance ID.
    pub fn project_helper_member(&self, previous: &meerkat_core::MobMemberBinding,
        compiled: &meerkat_core::MobMemberBinding) -> Self
    {
        let mut selected = self.clone();
        project_helper_requests(&mut selected.helper_report, &mut selected.agent_setup, previous, compiled);
        selected
    }
}

fn project_helper_requests(report: &mut Option<HelperReportCapabilityRequest>,
    setup: &mut [AgentSetupCapabilityRequest], previous: &meerkat_core::MobMemberBinding,
    compiled: &meerkat_core::MobMemberBinding)
{
    if let Some(request) = report {
        if &request.member == previous { request.member = compiled.clone(); }
    }
    for request in setup {
        if &request.member == previous { request.member = compiled.clone(); }
    }
}

impl PreparedInstallationOwner {
    /// Current trusted known-key callers resolve completely before opening or
    /// initializing the canonical pair. Deferred held callers must instead use
    /// their actual report join; this method supplies no controller evidence.
    pub fn open_with_known_authorities(deployment_root: PathBuf, selected: PreparedLocalPolicy,
        startup: operator_setup::OperatorOwnerStartup, intent: &PreparationAuthorityIntent,
    ) -> Result<(Self, PreparationAuthorities), HostLifetimeError> {
        let requests = intent.resolve_known_routes()?;
        let owner = Self::open(deployment_root, selected, startup)?;
        Ok((owner, requests))
    }

    /// Complete explicit parent selections on the same retained owner. Held
    /// construction may already exist, but publication is still withheld. A
    /// retry uses the same caller-retained IDs and never replaces a parent.
    pub fn configure_authorities(&mut self, helper: &mut InstalledSetupSelection,
        requests: &PreparationAuthorities) -> Result<(), HostLifetimeError>
    {
        self.configure_authorities_with_helper_server(helper, requests, None)
    }

    /// The trusted resource compiler supplies its actual selected server. An
    /// explicit request is required; configuration alone never issues access.
    /// Server config/generation custody remains with native registration.
    pub fn configure_authorities_with_helper_server(&mut self, helper: &mut InstalledSetupSelection,
        requests: &PreparationAuthorities, server: Option<&meerkat_core::McpServerConfig>,
    ) -> Result<(), HostLifetimeError> {
        self.configure_authorities_with_helper_resources(helper, requests, server, None)
    }

    /// Native preparation supplies the actual captured member/session pair.
    /// This observation selects no authority; only the explicit admin request
    /// may issue the exact catalog base. Missing observation remains unavailable.
    pub fn configure_authorities_with_helper_resources(&mut self, helper: &mut InstalledSetupSelection,
        requests: &PreparationAuthorities, server: Option<&meerkat_core::McpServerConfig>,
        session: Option<(&meerkat_core::MobMemberBinding, &meerkat_core::SessionId)>,
    ) -> Result<(), HostLifetimeError> {
        self.selected.validate()?;
        let member = self.selected.native_host.members.iter().find(|row| row.member == helper.member)
            .ok_or(HostLifetimeError::InvalidSelection)?;
        if [&helper.ingress_actor, &helper.logical_executor].into_iter().any(|value| value.validate_qualified().is_err())
            || helper.represented_subject.as_ref().is_some_and(|value| value.validate_qualified().is_err())
        { return Err(HostLifetimeError::InvalidSelection); }
        let helper_audience = member.audience.clone();
        let any = !requests.controllers.is_empty() || !requests.agent_setup.is_empty()
            || requests.helper_report.is_some() || requests.helper_mcp.is_some() || requests.helper_catalog.is_some();
        if !any { return Ok(()); }
        let operator = self.selected.native_host.remote_operator.clone().ok_or(HostLifetimeError::Denied)?;
        requests.validate_inventory()?;
        // Preflight both separately selected scopes before entering any issuer.
        // A missing session or changed current base cannot leave new sibling grants.
        let helper_catalog = super::helper_catalog::PreparedHelperCatalog::select(
            &self.source, &self.selected.native_host.grant_administrator, &operator,
            helper, &helper_audience, requests.helper_catalog.as_ref(), session,
        )?;
        // Validate the entire new resource scope and existing selection before
        // entering any issuer in this pass. This does not infer MCP authority.
        let helper_mcp = super::helper_mcp::PreparedHelperMcp::select(
            &self.source, &self.selected.native_host.grant_administrator, &operator,
            helper, &helper_audience, requests.helper_mcp.as_ref(), server,
            requests.helper_report.as_ref().map(|request| 1 + usize::from(request.account_issuance_id.is_some())
                + usize::from(request.public_issuance_id.is_some())).unwrap_or_else(||
                    helper.report_capability.as_ref().map(|report| 1 + usize::from(report.account_parent.is_some())
                        + usize::from(report.public_parent.is_some())).unwrap_or(0)),
        )?;
        // Reject changed selections before any new issuance. Recovery of an
        // existing selection must name its exact retained native issue ID.
        let existing = |parent: Option<&Vec<GrantLineageRef>>, id: &EvidenceId| {
            if parent.is_some_and(|refs| refs.len() != 1 || &refs[0].grant_id != id || refs[0].root_authority != operator) {
                Err(HostLifetimeError::Denied)
            } else { Ok(()) }
        };
        for request in &requests.controllers {
            match &request.target {
                PreparedControllerTarget::Helper => existing((!helper.controller_parent.is_empty()).then_some(&helper.controller_parent), &request.issuance_id)?,
                PreparedControllerTarget::OriginalRoute { route } => {
                    let route = self.selected.native_host.routes.iter().find(|row| &row.id == route).ok_or(HostLifetimeError::Denied)?;
                    existing(route.controller_parent.as_ref(), &request.issuance_id)?;
                }
            }
        }
        for request in &requests.agent_setup {
            let route = self.selected.native_host.routes.iter().find(|row| row.id == request.route).ok_or(HostLifetimeError::Denied)?;
            if route.source_binding_in(&self.selected.channels)?.0 != &request.member { return Err(HostLifetimeError::Denied); }
            let member = self.selected.native_host.members.iter().find(|row| row.member == request.member).ok_or(HostLifetimeError::Denied)?;
            if member.setup_capability.as_ref().is_some_and(|selected| selected.route != request.route
                || selected.parent.grant_id != request.issuance_id || selected.parent.root_authority != operator
                || selected.helper_targets.len() != request.helper_targets.len()
                || selected.helper_targets.iter().zip(&request.helper_targets).any(|(old, new)|
                    old.target != new.target || old.parent.grant_id != new.issuance_id || old.parent.root_authority != operator))
            { return Err(HostLifetimeError::Denied); }
        }
        if let Some(request) = &requests.helper_report {
            if request.member != helper.member
                || crate::installed_host::acquisition::ConfiguredHelperReportCapability::validate_basis(&helper.operation_basis).is_err()
            { return Err(HostLifetimeError::Denied); }
            existing(helper.report_capability.as_ref().map(|selected| &selected.parent), &request.issuance_id)?;
            match (helper.report_capability.as_ref().and_then(|selected| selected.account_parent.as_ref()),
                &request.account_issuance_id) {
                (Some(parent), Some(id)) => existing(Some(parent), id)?,
                (Some(_), None) => return Err(HostLifetimeError::Denied),
                _ => {},
            }
            match (helper.report_capability.as_ref().and_then(|selected| selected.public_parent.as_ref()),
                &request.public_issuance_id) {
                (Some(parent), Some(id)) => existing(Some(parent), id)?,
                (Some(_), None) => return Err(HostLifetimeError::Denied),
                _ => {},
            }
        }
        for request in &requests.controllers {
            let (executor, subject, audience, selected) = match &request.target {
                PreparedControllerTarget::Helper => (helper.logical_executor.clone(), helper.represented_subject.clone(),
                    helper_audience.clone(), (!helper.controller_parent.is_empty()).then(|| helper.controller_parent.clone())),
                PreparedControllerTarget::OriginalRoute { route } => {
                    let route = self.selected.native_host.routes.iter().find(|row| &row.id == route)
                        .ok_or(HostLifetimeError::Denied)?;
                    let target = route.source_binding_in(&self.selected.channels)?.0;
                    let member = self.selected.native_host.members.iter().find(|row| &row.member == target)
                        .ok_or(HostLifetimeError::Denied)?;
                    (route.logical_executor.clone(), route.represented_subject.clone(), member.audience.clone(), route.controller_parent.clone())
                }
            };
            let expected = application_operations::parent_restrictions(application_operations::controller(
                &self.selected.native_host.grant_administrator, &executor, &audience,
                &request.controller_route, application_operations::ControllerAction::Inference));
            let parent = controller_parent(&self.source, &self.selected.native_host.grant_administrator,
                &operator, subject.as_ref(), request, expected)?;
            if selected.as_ref().is_some_and(|old| old != &parent) { return Err(HostLifetimeError::Denied); }
            match &request.target {
                PreparedControllerTarget::Helper => helper.controller_parent = parent,
                PreparedControllerTarget::OriginalRoute { route } => {
                    self.selected.native_host.routes.iter_mut().find(|row| &row.id == route)
                        .ok_or(HostLifetimeError::Denied)?.controller_parent = Some(parent);
                }
            }
        }
        if let Some(request) = &requests.helper_report {
            if request.member != helper.member
                || crate::installed_host::acquisition::ConfiguredHelperReportCapability::validate_basis(&helper.operation_basis).is_err()
            { return Err(HostLifetimeError::Denied); }
            let expected = operator_setup::configured_report::report_parent(&self.selected.native_host.grant_administrator, &helper.logical_executor, &helper_audience);
            let capability = operator_setup::configured_report::issue_or_recover(&self.source,
                &self.selected.native_host.grant_administrator, &operator, helper, request, expected, &helper_audience)?;
            if helper.report_capability.as_ref().is_some_and(|old| old != &capability) { return Err(HostLifetimeError::Denied); }
            helper.report_capability = Some(capability);
        }
        if let Some(selected) = helper_mcp {
            selected.issue_or_recover(&self.source, &operator, helper)?;
        }
        if let Some(selected) = helper_catalog {
            selected.issue_or_recover(&self.source, &operator, helper)?;
        }
        for request in &requests.agent_setup {
            let capability = operator_setup::configured_setup::configure_prepared(
                &self.selected.native_host, &self.selected.channels, &self.source, &operator, request, Some(helper))?;
            let member = self.selected.native_host.members.iter_mut().find(|row| row.member == request.member)
                .ok_or(HostLifetimeError::Denied)?;
            if member.setup_capability.as_ref().is_some_and(|old| old != &capability) { return Err(HostLifetimeError::Denied); }
            member.setup_capability = Some(capability);
        }
        self.selected.validate()
    }
}

fn controller_parent(source: &ToolkitOriginalOwner, application: &meerkat_core::PrincipalRef,
    operator: &meerkat_core::PrincipalRef, subject: Option<&meerkat_core::PrincipalRef>,
    request: &PreparedControllerRequest, expected: ExecutionRestrictions,
) -> Result<Vec<GrantLineageRef>, HostLifetimeError> {
    let grants = source.grants();
    let issue = || {
        let parent = match grants.issued_reference(&request.issuance_id)? {
            Some(parent) => parent,
            None => grants.issue_root(operator, request.issuance_id.clone(), application.clone(), subject.cloned(), expected.clone())?,
        };
        if &parent.root_authority != operator { return Err(GrantRefusal::Denied); }
        let resolved = grants.resolve_controller_lineage(std::slice::from_ref(&parent), application, subject)?;
        if resolved.restrictions() != &expected { return Err(GrantRefusal::Denied); }
        resolved.check_current().map_err(|_| GrantRefusal::Unavailable)?;
        Ok(vec![parent])
    };
    issue().map_err(|error| match error {
        GrantRefusal::Denied | GrantRefusal::Duplicate | GrantRefusal::ControllerInUse => HostLifetimeError::Denied,
        GrantRefusal::Unavailable => HostLifetimeError::Unavailable,
    })
}
