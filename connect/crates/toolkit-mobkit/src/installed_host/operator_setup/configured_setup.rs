//! Explicit trusted configuration, separate from per-original human review.
use super::*;
use meerkat_authorization::grants::GrantRefusal;
use meerkat_toolkit::host_lifetime::policy::{CompiledAgentSetupCapability, CompiledLocalPolicy};

/// Supplied by the authenticated configuration owner and retained verbatim
/// across an uncertain response. A new ID is a new admin issuance request.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentSetupCapabilityRequest {
    pub member: meerkat_core::MobMemberBinding,
    pub route: Id,
    pub issuance_id: EvidenceId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub helper: Option<meerkat_core::MobMemberBinding>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub helper_targets: Vec<super::SetupTargetRequest>,
}

impl OperatorSetup {
    /// The actual operator of this existing owner, never a model or app RPC,
    /// may configure acquisition and explicitly named helper targets for one
    /// member. This is not an approval record; helper grants, protected
    /// installation/account review and controller authority remain separate.
    pub fn configure_agent_setup(
        &self,
        request: &AgentSetupCapabilityRequest,
    ) -> Result<CompiledAgentSetupCapability, HostLifetimeError> {
        self.lifetime.with_current_setup(|| {
            configure(
                self.lifetime.selected_policy(),
                &self.source,
                &self.operator,
                request,
                match request.helper.as_ref() {
                    None => self.helper_selection.as_ref(),
                    Some(member) => Some(self.helper_selection.iter().chain(self.additional_helpers.iter())
                        .find(|helper| &helper.member == member).ok_or(HostLifetimeError::Denied)?),
                },
            )
        })
    }

    /// Existing trusted installer's stopped-host transaction. The selected
    /// native owner is restored, never initialized or replaced. No session,
    /// runtime or activation is started. The exact parent is native authority;
    /// the returned value only lets the existing compiler select its reference.
    pub fn configure_application_agent_setup(
        selection: &HostBootstrapSelection,
        request: &AgentSetupCapabilityRequest,
    ) -> Result<CompiledAgentSetupCapability, HostLifetimeError> {
        Self::configure_application_agent_setup_with_helper(selection, request, |_| Ok(None))
    }

    pub fn configure_application_agent_setup_with_helper(
        selection: &HostBootstrapSelection,
        request: &AgentSetupCapabilityRequest,
        select_helper: impl FnOnce(&CompiledNativeHost) -> Result<Option<crate::installed_host::acquisition::InstalledSetupSelection>, HostLifetimeError>,
    ) -> Result<CompiledAgentSetupCapability, HostLifetimeError> {
        NativeHostLifetime::with_candidate_installation(selection, |policy| {
            let native = policy
                .native_host
                .as_ref()
                .ok_or(HostLifetimeError::Unavailable)?;
            let operator = native
                .remote_operator
                .as_ref()
                .ok_or(HostLifetimeError::Unavailable)?;
            let _validated = validate_installed_artifacts(native, |reference| {
                read_bundle_artifact(selection, reference)
            })?;
            let helper = select_helper(native)?;
            if let Some(helper) = &helper { helper.selected_member(native)?; }
            let (source, originals) =
                open_owner_pair(selection, native, operator, OperatorOwnerStartup::Restore)?;
            let result = configure(policy, &source, operator, request, helper.as_ref());
            drop(originals);
            drop(source);
            result
        })
    }
}

fn configure(
    policy: &CompiledLocalPolicy,
    source: &ToolkitOriginalOwner,
    operator: &meerkat_core::PrincipalRef,
    request: &AgentSetupCapabilityRequest,
    helper: Option<&crate::installed_host::acquisition::InstalledSetupSelection>,
) -> Result<CompiledAgentSetupCapability, HostLifetimeError> {
    let native = policy
        .native_host
        .as_ref()
        .ok_or(HostLifetimeError::Unavailable)?;
    configure_prepared(&native.preparation(), &policy.channels, source, operator, request, helper)
}

pub(crate) fn configure_prepared(
    native: &meerkat_toolkit::host_lifetime::policy::PreparedNativeHost,
    channels: &[meerkat_toolkit::host_lifetime::policy::CompiledChannelBinding],
    source: &ToolkitOriginalOwner, operator: &meerkat_core::PrincipalRef,
    request: &AgentSetupCapabilityRequest,
    helper: Option<&crate::installed_host::acquisition::InstalledSetupSelection>,
) -> Result<CompiledAgentSetupCapability, HostLifetimeError> {
    let route = native
        .routes
        .iter()
        .find(|route| route.id == request.route)
        .ok_or(HostLifetimeError::Denied)?;
    if request.helper.as_ref().is_some_and(|selected| {
        helper.is_none_or(|helper| &helper.member != selected)
    }) {
        return Err(HostLifetimeError::Denied);
    }
    if route.source_binding_in(channels)?.0 != &request.member {
        return Err(HostLifetimeError::Denied);
    }
    let member = native
        .members
        .iter()
        .find(|member| member.member == request.member)
        .ok_or(HostLifetimeError::Denied)?;
    super::setup_targets::validate_targets(request.helper_targets.iter().map(|target| &target.target))?;
    let mut ids = std::collections::BTreeSet::from([request.issuance_id.clone()]);
    if request.helper_targets.iter().any(|target| !ids.insert(target.issuance_id.clone())) {
        return Err(HostLifetimeError::InvalidSelection);
    }
    let intents = if request.helper_targets.is_empty() { vec![] } else {
        let helper = helper.ok_or(HostLifetimeError::Denied)?;
        let member = native.members.iter().find(|row| row.member == helper.member)
            .ok_or(HostLifetimeError::Denied)?;
        super::setup_targets::helper_intents(source, helper, &native.grant_administrator, &member.audience,
            request.helper_targets.iter().map(|target| target.target.clone()))?
    };
    let expected =
        application_operations::configured_setup_parent(&native.grant_administrator, &route.logical_executor, &member.audience);
    let grants = source.grants();
    let operation = || {
        // A retained identity never implies usability. Reconcile only the same
        // current root, application, represented subject and exact restrictions.
        let parent = match grants.issued_reference(&request.issuance_id)? {
            Some(parent) => parent,
            None => grants.issue_root(
                operator,
                request.issuance_id.clone(),
                native.grant_administrator.clone(),
                route.represented_subject.clone(),
                expected.clone(),
            )?,
        };
        if &parent.root_authority != operator {
            return Err(GrantRefusal::Denied);
        }
        let resolved = grants.resolve_lineage(
            std::slice::from_ref(&parent),
            &native.grant_administrator,
            route.represented_subject.as_ref(),
        )?;
        if resolved.restrictions() != &expected {
            return Err(GrantRefusal::Denied);
        }
        resolved
            .check_current()
            .map_err(|_| GrantRefusal::Unavailable)?;
        let mut helper_targets = vec![];
        for (target, intent) in request.helper_targets.iter().zip(&intents) {
            let reference = match grants.issued_reference(&target.issuance_id)? {
                Some(reference) => reference,
                None => grants.issue_root(operator, target.issuance_id.clone(), native.grant_administrator.clone(),
                    route.represented_subject.clone(), intent.requested.clone())?,
            };
            if &reference.root_authority != operator { return Err(GrantRefusal::Denied); }
            let current = grants.resolve_lineage(std::slice::from_ref(&reference), &native.grant_administrator,
                route.represented_subject.as_ref())?;
            if current.restrictions() != &intent.requested { return Err(GrantRefusal::Denied); }
            current.check_current().map_err(|_| GrantRefusal::Unavailable)?;
            helper_targets.push(meerkat_toolkit::host_lifetime::policy::CompiledAgentSetupTarget {
                target: target.target.clone(), parent: reference, restrictions: intent.requested.clone(),
            });
        }
        Ok(CompiledAgentSetupCapability { route: request.route.clone(), parent,
            helper: request.helper.clone(), helper_targets })
    };
    operation().map_err(|error| match error {
        GrantRefusal::Denied | GrantRefusal::Duplicate | GrantRefusal::ControllerInUse => {
            HostLifetimeError::Denied
        }
        GrantRefusal::Unavailable => HostLifetimeError::Unavailable,
    })
}
