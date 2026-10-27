//! Exact compiled application facts. The host compiler resolves household
//! mapping once; this consumer performs exact binding comparisons only.
use super::control::{HostBootstrapSelection, HostLifetimeError, SelectedHostArtifact};
use crate::authority::ChannelSourceProvenance;
use meerkat_authorization_contracts::work_association::InputAuthorityAssociation;
use meerkat_authorization_contracts::{
    constraints::{AudienceRef, ResourceDomain},
    grant::GrantLineageRef,
};
use meerkat_core::auth::PrincipalRef;
use meerkat_core::connection::{MobMemberBinding, RealmId};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use toolkit_package::{Id, canonical_json};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompiledLocalPolicy {
    pub schema: u32,
    pub deployment: Id,
    /// Exact hash of the existing compiled policy.json in the same bundle.
    pub source_policy_sha256: String,
    pub components: Vec<Id>,
    pub channels: Vec<CompiledChannelBinding>,
    /// Optional native executable composition. Absence supplies no native owner.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_host: Option<CompiledNativeHost>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompiledChannelBinding {
    pub component: Id,
    pub channel_id: String,
    pub service_subject: String,
    pub provider_account: String,
    pub sender_id: String,
    pub requester: PrincipalRef,
    pub ingress_actor: PrincipalRef,
    pub target_identity: String,
    pub member: MobMemberBinding,
    pub realm: RealmId,
    pub required_surfaces: Vec<String>,
}

/// Existing bundle artifact selected by relative name and content digest.
/// The bundle manifest must contain these exact bytes as well.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompiledArtifactReference {
    pub name: String,
    pub sha256: String,
}

/// Selected maximum scope for administrative service-parent requests. Actual
/// authorization comes from an independent current administrator/rule owner;
/// deserializing this field grants nothing. The scope covers finite service
/// actions on named routes, not an account allowlist or a native grant.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ServiceParentAdministration {
    ApplicationPolicyTasks { routes: BTreeSet<Id> },
}
impl ServiceParentAdministration {
    pub fn routes(&self) -> &BTreeSet<Id> {
        match self { Self::ApplicationPolicyTasks { routes } => routes }
    }
}

/// Missing native host facts compiled by the same application policy owner.
/// This selects owner identity and source/registration inputs, never a grant.
/// Account/operation policy remains with its existing owner.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompiledNativeHost {
    pub realm: RealmId,
    /// Authenticated application principal. Native child issuance requires it
    /// to be the selected parent grantee; it never acts as the operator root.
    pub grant_administrator: PrincipalRef,
    /// Exact operator selected by the trusted application installer. Null
    /// disables remote review. Review/answer JSON never supplies this caller.
    /// Native root issuance still requires equality with the restored root.
    pub remote_operator: Option<PrincipalRef>,
    /// Separate selected administrative scope. An actual current authorizer is
    /// also required; absence never falls back to stock OS or application policy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_parent_administration: Option<ServiceParentAdministration>,
    pub ingress_domain: ResourceDomain,
    pub application_tool_policy: CompiledArtifactReference,
    /// Optional application-owned acquisition configuration in this same
    /// authenticated bundle. Resource preparation cannot issue its backend
    /// generation. Absence preserves existing hosts and canonical bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup: Option<CompiledArtifactReference>,
    pub members: Vec<CompiledNativeMember>,
    pub routes: Vec<CompiledNativeSourceRoute>,
}

/// A named member's explicit acquisition capability. This native parent is
/// separate from controller, service and helper selections. Its currentness
/// and exact bounds are resolved by the native owner at each first binding.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompiledAgentSetupCapability {
    pub route: Id,
    pub parent: GrantLineageRef,
    /// Optional exact host-selected helper for this route. Absence retains the
    /// application's legacy singleton helper; this does not select a grant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub helper: Option<MobMemberBinding>,
    /// Explicit correlated helper operations, each with an independent parent.
    /// These are compiler-selected references, not permission or live grants.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub helper_targets: Vec<CompiledAgentSetupTarget>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompiledAgentSetupTarget {
    /// None is the existing catalog base. Registered operations and MCP
    /// servers use their exact native target, never a fallback base.
    pub target: Option<meerkat_authorization_contracts::work_association::GrantTarget>,
    pub parent: GrantLineageRef,
    pub restrictions: meerkat_authorization_contracts::constraints::ExecutionRestrictions,
}

/// The host selects audience and generation. The native registration supplies
/// its own session document and runtime; neither can be serialized here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompiledNativeMember {
    pub member: MobMemberBinding,
    pub logical_owner: PrincipalRef,
    pub context_generation: u64,
    pub audience: AudienceRef,
    /// Existing installed-artifact evidence, distinct from Invoke grants.
    /// The referenced canonical array contains the approved ToolIdentity values
    /// for this member's initial visible surface. Lazy catalog entries need not
    /// be visible. A provenance annotation alone is never the association owner.
    pub initial_surface: CompiledArtifactReference,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup_capability: Option<CompiledAgentSetupCapability>,
}

/// Application-selected facts available before native member construction.
/// Artifact references are intentionally absent. This input is neither a
/// published activation nor permission to use a native owner.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedLocalPolicy {
    pub deployment: Id,
    pub components: Vec<Id>,
    pub channels: Vec<CompiledChannelBinding>,
    pub native_host: PreparedNativeHost,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedNativeHost {
    pub realm: RealmId,
    pub grant_administrator: PrincipalRef,
    pub remote_operator: Option<PrincipalRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_parent_administration: Option<ServiceParentAdministration>,
    pub ingress_domain: ResourceDomain,
    pub members: Vec<PreparedNativeMember>,
    pub routes: Vec<CompiledNativeSourceRoute>,
}

/// The native registration supplies the real session and finished surface.
/// These remaining fields retain the application's exact destination choice.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedNativeMember {
    pub member: MobMemberBinding,
    pub logical_owner: PrincipalRef,
    pub context_generation: u64,
    pub audience: AudienceRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup_capability: Option<CompiledAgentSetupCapability>,
}

/// Exact installed provider-event or local-console route. The application
/// authenticates each event before using this route on its inherited pipe.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompiledNativeSourceRoute {
    pub id: Id,
    pub origin: CompiledNativeSourceOrigin,
    /// Exact operator-issued parent from this host's native owner, selected by
    /// the installed policy owner. None leaves access setup unavailable. This
    /// is historical selection; native resolution supplies current authority.
    pub application_parent: Option<Vec<GrantLineageRef>>,
    /// Independently selected operator-issued controller parent for this route.
    /// None supplies no controller access. It never falls back to the ordinary
    /// application parent; native full-lineage resolution remains mandatory.
    pub controller_parent: Option<Vec<GrantLineageRef>>,
    pub logical_executor: PrincipalRef,
    pub represented_subject: Option<PrincipalRef>,
    pub source_authority: PrincipalRef,
    pub authentication_domain: ResourceDomain,
    pub event_domain: ResourceDomain,
}

/// Each source owner supplies only facts it actually owns. A local console
/// login is not represented as a provider account or a provider sender.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CompiledNativeSourceOrigin {
    ProviderEvent {
        channel_id: String,
        sender_id: String,
        target_identity: String,
    },
    LocalConsole {
        member: MobMemberBinding,
        requester: PrincipalRef,
        ingress_actor: PrincipalRef,
    },
}

impl CompiledNativeSourceRoute {
    /// Exact installed source mapping. These are application-selected facts,
    /// not authentication of an event or permission to invoke an operation.
    pub fn source_binding<'a>(
        &'a self,
        policy: &'a CompiledLocalPolicy,
    ) -> Result<(&'a MobMemberBinding, &'a PrincipalRef, &'a PrincipalRef), HostLifetimeError> {
        self.source_binding_in(&policy.channels)
    }

    /// The same exact source mapping before and after artifact completion.
    /// Supplying these compiled facts does not authenticate an event.
    pub fn source_binding_in<'a>(
        &'a self,
        channels: &'a [CompiledChannelBinding],
    ) -> Result<(&'a MobMemberBinding, &'a PrincipalRef, &'a PrincipalRef), HostLifetimeError> {
        match &self.origin {
            CompiledNativeSourceOrigin::ProviderEvent {
                channel_id,
                sender_id,
                target_identity,
            } => {
                let mut rows = channels.iter().filter(|row| {
                    &row.channel_id == channel_id
                        && &row.sender_id == sender_id
                        && &row.target_identity == target_identity
                });
                let row = rows.next().ok_or(HostLifetimeError::InvalidSelection)?;
                if rows.next().is_some() {
                    return Err(HostLifetimeError::InvalidSelection);
                }
                Ok((&row.member, &row.requester, &row.ingress_actor))
            }
            CompiledNativeSourceOrigin::LocalConsole {
                member,
                requester,
                ingress_actor,
            } => Ok((member, requester, ingress_actor)),
        }
    }
}

impl CompiledLocalPolicy {
    /// Canonical compiler output using the same validation consumed at bootstrap.
    /// This does not publish an activation or authorize any operation.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, HostLifetimeError> {
        self.validate()?;
        canonical_json(self).map_err(|_| HostLifetimeError::InvalidSelection)
    }

    pub(crate) fn validate(&self) -> Result<(), HostLifetimeError> {
        if self.schema != 7
            || !valid_sha256(&self.source_policy_sha256)
            || self.components.windows(2).any(|p| p[0] >= p[1])
        {
            return Err(HostLifetimeError::InvalidSelection);
        }
        validate_channels(&self.components, &self.channels)?;
        if let Some(native) = &self.native_host {
            native.validate(self)?;
        }
        Ok(())
    }

    pub(crate) fn binding<'a>(
        &'a self,
        source: &ChannelSourceProvenance,
        association: &InputAuthorityAssociation,
        target: &str,
    ) -> Result<&'a CompiledChannelBinding, HostLifetimeError> {
        self.channels
            .iter()
            .find(|r| {
                r.channel_id == source.channel_id
                    && r.component.as_str() == source.extension_id
                    && r.service_subject == source.service_subject
                    && r.provider_account == source.provider_account
                    && r.sender_id == source.sender_id
                    && r.requester.id.as_str() == source.requester
                    && r.requester == association.candidate().requester
                    && r.ingress_actor == association.candidate().ingress_actor
                    && r.realm == association.candidate().ingress_namespace.realm
                    && self.native_host.as_ref().is_some_and(|native| {
                        native.members.iter().any(|member| {
                            member.member == r.member
                                && member.logical_owner
                                    == association.candidate().target.logical_owner
                                && member.context_generation
                                    == association.candidate().target.context_generation
                                && member.audience == association.candidate().target.audience
                        })
                    })
                    && r.target_identity == target
            })
            .ok_or(HostLifetimeError::Denied)
    }
}

fn validate_channels(
    components: &[Id],
    channels: &[CompiledChannelBinding],
) -> Result<(), HostLifetimeError> {
    if components.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(HostLifetimeError::InvalidSelection);
    }
        let mut unique = BTreeSet::new();
        for row in channels {
            if !components.contains(&row.component)
                || [
                    &row.channel_id,
                    &row.service_subject,
                    &row.provider_account,
                    &row.sender_id,
                    &row.target_identity,
                ]
                .iter()
                .any(|v| v.is_empty())
                || row.member.comms_name().is_err()
                || meerkat_core::mob_realm_id(&row.member.mob_id).as_ref().ok() != Some(&row.realm)
                || row.requester.validate_qualified().is_err()
                || row.ingress_actor.validate_qualified().is_err()
                || row.required_surfaces.is_empty()
                || row.required_surfaces.windows(2).any(|p| p[0] >= p[1])
                || row.required_surfaces.iter().any(|s| !valid_surface(s))
                || !row
                    .required_surfaces
                    .contains(&format!("channel:{}", row.channel_id))
                || !unique.insert((
                    row.channel_id.clone(),
                    row.sender_id.clone(),
                    row.target_identity.clone(),
                ))
            {
                return Err(HostLifetimeError::InvalidSelection);
            }
        }
    Ok(())
}

pub(crate) fn read_selected(selected: &SelectedHostArtifact) -> Result<Vec<u8>, HostLifetimeError> {
    if !valid_sha256(&selected.sha256) || selected.path.is_symlink() || !selected.path.is_file() {
        return Err(HostLifetimeError::InvalidSelection);
    }
    let bytes = std::fs::read(&selected.path).map_err(|_| HostLifetimeError::Unavailable)?;
    if hex::encode(Sha256::digest(&bytes)) != selected.sha256 {
        return Err(HostLifetimeError::InvalidSelection);
    }
    Ok(bytes)
}

pub(crate) fn validate_selection(
    selection: &HostBootstrapSelection,
) -> Result<(CompiledLocalPolicy, String), HostLifetimeError> {
    let root = selection
        .deployment_root
        .canonicalize()
        .map_err(|_| HostLifetimeError::Unavailable)?;
    let activation_path = selection
        .activation
        .path
        .canonicalize()
        .map_err(|_| HostLifetimeError::Unavailable)?;
    if root.join("current").canonicalize()
        .map_err(|_| HostLifetimeError::Unavailable)? != activation_path
    {
        return Err(HostLifetimeError::Denied);
    }
    validate_candidate_selection(selection)
}

/// Read-only candidate envelope validation for the application's installer.
/// The application's deployment transaction selects these exact candidate bytes.
/// This does not make the candidate current or permit runtime admission.
/// Application-specific manifest fields are retained and never rewritten here.
pub fn validate_candidate_selection(
    selection: &HostBootstrapSelection,
) -> Result<(CompiledLocalPolicy, String), HostLifetimeError> {
    let root = selection.deployment_root.canonicalize()
        .map_err(|_| HostLifetimeError::Unavailable)?;
    let activation_path = selection.activation.path.canonicalize()
        .map_err(|_| HostLifetimeError::Unavailable)?;
    if !activation_path.starts_with(&root) {
        return Err(HostLifetimeError::Denied);
    }
    let activation: serde_json::Value =
        serde_json::from_slice(&read_selected(&selection.activation)?)
            .map_err(|_| HostLifetimeError::InvalidSelection)?;
    let bundle = activation
        .get("policy_bundle")
        .ok_or(HostLifetimeError::InvalidSelection)?;
    let relative = bundle
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or(HostLifetimeError::InvalidSelection)?;
    let bundle_path = root
        .join(relative)
        .canonicalize()
        .map_err(|_| HostLifetimeError::Unavailable)?;
    if !bundle_path.starts_with(&root)
        || selection.policy_manifest.path != bundle_path.join("manifest.json")
        || selection.local_policy.path != bundle_path.join("toolkit-local-policy.json")
    {
        return Err(HostLifetimeError::InvalidSelection);
    }
    let revision = format!("hc-policy-v1:sha256:{}", selection.policy_manifest.sha256);
    if bundle.get("policy_digest").and_then(|v| v.as_str()) != Some(revision.as_str()) {
        return Err(HostLifetimeError::InvalidSelection);
    }
    let manifest: serde_json::Value =
        serde_json::from_slice(&read_selected(&selection.policy_manifest)?)
            .map_err(|_| HostLifetimeError::InvalidSelection)?;
    let artifacts = manifest
        .get("artifacts")
        .and_then(|v| v.as_array())
        .ok_or(HostLifetimeError::InvalidSelection)?;
    let bytes = read_selected(&selection.local_policy)?;
    let policy: CompiledLocalPolicy =
        serde_json::from_slice(&bytes).map_err(|_| HostLifetimeError::InvalidSelection)?;
    policy.validate()?;
    if canonical_json(&policy).map_err(|_| HostLifetimeError::InvalidSelection)? != bytes {
        return Err(HostLifetimeError::InvalidSelection);
    }
    for (name, expected) in [
        ("toolkit-local-policy.json", &selection.local_policy.sha256),
        ("policy.json", &policy.source_policy_sha256),
    ] {
        if artifacts
            .iter()
            .filter(|a| a.get("path").and_then(|v| v.as_str()) == Some(name))
            .count()
            != 1
            || !artifacts.iter().any(|a| {
                a.get("path").and_then(|v| v.as_str()) == Some(name)
                    && a.get("sha256").and_then(|v| v.as_str()) == Some(expected)
            })
        {
            return Err(HostLifetimeError::InvalidSelection);
        }
        read_selected(&SelectedHostArtifact {
            path: bundle_path.join(name),
            sha256: expected.clone(),
        })?;
    }
    Ok((policy, revision))
}

pub(crate) fn valid_surface(value: &str) -> bool {
    !value.contains('*')
        && ["cap:", "route:", "channel:", "runnable:"]
            .iter()
            .any(|p| value.starts_with(p) && value.len() > p.len())
}
fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|v| v.is_ascii_digit() || (b'a'..=b'f').contains(&v))
}

impl CompiledArtifactReference {
    pub fn validate(&self) -> Result<(), HostLifetimeError> {
        let mut components = std::path::Path::new(&self.name).components();
        if !matches!(components.next(), Some(std::path::Component::Normal(_)))
            || components.next().is_some()
            || !valid_sha256(&self.sha256)
        {
            return Err(HostLifetimeError::InvalidSelection);
        }
        Ok(())
    }
}

impl CompiledNativeHost {
    fn validate(&self, policy: &CompiledLocalPolicy) -> Result<(), HostLifetimeError> {
        self.preparation().validate(&policy.channels)?;
        self.application_tool_policy.validate()?;
        if let Some(setup) = &self.setup { setup.validate()?; }
        for member in &self.members { member.initial_surface.validate()?; }
        Ok(())
    }

    pub fn preparation(&self) -> PreparedNativeHost {
        PreparedNativeHost {
            realm: self.realm.clone(),
            grant_administrator: self.grant_administrator.clone(),
            remote_operator: self.remote_operator.clone(),
            service_parent_administration: self.service_parent_administration.clone(),
            ingress_domain: self.ingress_domain.clone(),
            members: self.members.iter().map(|member| PreparedNativeMember {
                member: member.member.clone(),
                logical_owner: member.logical_owner.clone(),
                context_generation: member.context_generation,
                audience: member.audience.clone(),
                setup_capability: member.setup_capability.clone(),
            }).collect(),
            routes: self.routes.clone(),
        }
    }
}

impl PreparedLocalPolicy {
    pub fn validate(&self) -> Result<(), HostLifetimeError> {
        validate_channels(&self.components, &self.channels)?;
        self.native_host.validate(&self.channels)
    }

    /// Match the exact early selection to a fully validated final policy.
    /// This does not validate an activation, finish a member or adopt an owner.
    pub fn matches(&self, policy: &CompiledLocalPolicy) -> bool {
        policy.deployment == self.deployment
            && policy.components == self.components
            && policy.channels == self.channels
            && policy.native_host.as_ref().is_some_and(|native| {
                native.preparation() == self.native_host
            })
    }
}

impl PreparedNativeHost {
    fn validate(&self, channels: &[CompiledChannelBinding]) -> Result<(), HostLifetimeError> {
        if self.members.is_empty()
            || self.grant_administrator.validate_qualified().is_err()
            || self.remote_operator.as_ref().is_some_and(|operator| {
                operator.validate_qualified().is_err() || operator == &self.grant_administrator
            })
            || self.ingress_domain.authority.validate_qualified().is_err()
            || self.ingress_domain.namespace.is_empty()
        {
            return Err(HostLifetimeError::InvalidSelection);
        }
        if let Some(rule) = &self.service_parent_administration {
            if self.remote_operator.is_none() || rule.routes().is_empty()
                || rule.routes().iter().any(|id| !self.routes.iter().any(|route| &route.id == id))
            { return Err(HostLifetimeError::InvalidSelection); }
        }
        let mut members = BTreeSet::new();
        for member in &self.members {
            if member.member.comms_name().is_err()
                || meerkat_core::mob_realm_id(&member.member.mob_id)
                    .as_ref()
                    .ok()
                    != Some(&self.realm)
                || !members.insert(member.member.member.clone())
                || member.logical_owner.validate_qualified().is_err()
                || member.context_generation == 0
            {
                return Err(HostLifetimeError::InvalidSelection);
            }
            let audience_owner = match &member.audience {
                AudienceRef::Principal { principal } => principal,
                AudienceRef::Destination {
                    authority,
                    destination_id,
                } => {
                    if destination_id.is_empty() {
                        return Err(HostLifetimeError::InvalidSelection);
                    }
                    authority
                }
            };
            audience_owner
                .validate_qualified()
                .map_err(|_| HostLifetimeError::InvalidSelection)?;
        }
        for member in &self.members {
            if let Some(capability) = &member.setup_capability {
                let route = self.routes.iter().find(|route| route.id == capability.route)
                    .ok_or(HostLifetimeError::InvalidSelection)?;
                if capability.parent.validate().is_err()
                    || capability.helper_targets.len() >= meerkat_authorization_contracts::work_association::MAX_ASSOCIATION_REFERENCES
                    || capability.helper_targets.windows(2).any(|pair| pair[0].target >= pair[1].target)
                    || capability.helper_targets.iter().any(|target| target.parent.validate().is_err()
                        || target.parent == capability.parent)
                    || capability.helper_targets.iter().enumerate().any(|(index, target)|
                        capability.helper_targets[..index].iter().any(|prior| prior.parent == target.parent))
                    || route.source_binding_in(channels)?.0 != &member.member {
                    return Err(HostLifetimeError::InvalidSelection);
                }
            }
        }
        let mut routes = BTreeSet::new();
        for route in &self.routes {
            let (member, requester, actor) = route.source_binding_in(channels)?;
            if !routes.insert(route.id.clone())
                || [&route.application_parent, &route.controller_parent]
                    .into_iter()
                    .flatten()
                    .any(|parent| {
                        parent.is_empty()
                            || parent.len() >= meerkat_authorization_contracts::work_association::MAX_ASSOCIATION_REFERENCES
                    })
                || route
                    .represented_subject
                    .as_ref()
                    .is_some_and(|principal| principal.validate_qualified().is_err())
                || [
                    &route.logical_executor,
                    &route.source_authority,
                    requester,
                    actor,
                    &route.authentication_domain.authority,
                    &route.event_domain.authority,
                ]
                .iter()
                .any(|principal| principal.validate_qualified().is_err())
                || route.authentication_domain.namespace.is_empty()
                || route.event_domain.namespace.is_empty()
                || !self.members.iter().any(|m| &m.member == member)
            {
                return Err(HostLifetimeError::InvalidSelection);
            }
        }
        Ok(())
    }
}
