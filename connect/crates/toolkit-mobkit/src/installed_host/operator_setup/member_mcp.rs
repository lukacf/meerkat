//! New protected access to a current own configured server. Installation and
//! account facts select the review; only Core's grant owner issues permission.
use super::*;
use crate::installed_host::dynamic_members::AdmittedMemberClass;
use crate::installed_host::member_mcp_consumer::{
    AcceptedMemberMcpRequirement, MemberMcpAdmissionSelection, MemberMcpRequirementContract,
};
use crate::private_originals::{PrivateOriginals, PrivateSourceEvidence, ServiceMcpSelection};
use meerkat::{ConnectorAuthPhase, ConnectorAuthStatus, HostAuthService};
use meerkat_authorization_contracts::constraints::{AudienceRef, LifetimeRestriction};
use meerkat_core::{CredentialAccountRef, McpServerConfig, PrincipalRef};
use meerkat_mobkit::rpc_gateway::{GatewayFactoryConstruction, GatewayRealmMcpInstallation};
use meerkat_toolkit::host_lifetime::policy::{CompiledMemberClass, CompiledMemberClassInstance};
use std::{collections::BTreeSet, sync::Weak};

pub(crate) const SCHEMA: &str = "toolkit.operator-member-mcp";
const MAX_OPERATIONS: usize = 256;
pub(crate) const MAX_BINDINGS: usize = 32;

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum MemberMcpError {
    #[error("member access is denied")]
    Denied,
    #[error("member access owner is unavailable")]
    Unavailable,
    #[error("the selected installed definition changed")]
    InstallationChanged,
    #[error("the selected account needs authorization")]
    ReauthRequired,
    #[error("the selected account identity changed")]
    PrincipalChanged,
    #[error("member access was cancelled")]
    Cancelled,
    #[error("the exact entered grant outcome or cleanup needs reconciliation")]
    PendingReconciliation,
}
impl From<HostLifetimeError> for MemberMcpError {
    fn from(value: HostLifetimeError) -> Self {
        match value {
            HostLifetimeError::Unavailable | HostLifetimeError::NotSettled => Self::Unavailable,
            _ => Self::Denied,
        }
    }
}

/// Intent only. No principal, grant, configuration, account or reservation can
/// be supplied in these fields. The operator reviews the owner-selected facts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemberMcpIntent {
    pub server: String,
    pub operations: BTreeSet<String>,
    pub lifetime: LifetimeRestriction,
}
impl MemberMcpIntent {
    pub(crate) fn check(&self) -> Result<(), MemberMcpError> {
        if self.server.is_empty()
            || self.server.len() > 512
            || self.server.trim() != self.server
            || self.server.chars().any(char::is_control)
            || self.operations.is_empty()
            || self.operations.len() > MAX_OPERATIONS
            || self.operations.iter().any(|name| {
                name.is_empty()
                    || name.len() > 512
                    || name.trim() != name
                    || name.chars().any(char::is_control)
            })
        {
            return Err(MemberMcpError::Denied);
        }
        Ok(())
    }
}

/// Review provenance only. Both origins require the actual protected operator
/// approval. An accepted requirement additionally resolves through the bound
/// application admission owner; its digest never supplies permission.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemberMcpOrigin {
    OperatorSelf,
    AcceptedApplicationRequirement { source: PrivateSourceEvidence },
}

/// An opaque locator in this installed owner's existing store, not a bearer
/// grant. It is usable for cancellation before approval and for exact recovery.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MemberMcpBindingRef(EvidenceId);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MemberMcpTarget {
    pub(crate) id: Id,
    pub(crate) class: CompiledMemberClass,
    pub(crate) instance: CompiledMemberClassInstance,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Account {
    slot: CredentialAccountRef,
    issuer: String,
    strategy: String,
    subject: String,
}

/// No raw configuration, environment values or credentials enter the record.
/// Explicit None means this selected configuration declares no connector.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Installation {
    server: String,
    config_sha256: String,
    connector_approval: String,
    account: Option<Account>,
    operations: BTreeSet<String>,
}
impl Installation {
    fn capture(
        credential_realm: &meerkat_core::RealmId,
        config: &McpServerConfig,
        status: Option<&ConnectorAuthStatus>,
        operations: BTreeSet<String>,
    ) -> Result<Self, MemberMcpError> {
        let access = config.access.as_ref().ok_or(MemberMcpError::Denied)?;
        if config.name.trim().is_empty()
            || operations.is_empty()
            || operations.len() > MAX_OPERATIONS
            || operations.iter().any(|name| {
                name.is_empty()
                    || name.len() > 512
                    || name.trim() != name
                    || name.chars().any(char::is_control)
                    || !access.allowed_operations.contains(name)
            })
        {
            return Err(MemberMcpError::Denied);
        }
        let account = match &config.connector {
            None => {
                ServiceMcpSelection::no_connector(config).map_err(|_| MemberMcpError::Denied)?;
                if status.is_some() {
                    return Err(MemberMcpError::Denied);
                }
                None
            }
            Some(binding) => {
                let installed = meerkat::ConnectorMcpInstallation::from_config(config)
                    .ok_or(MemberMcpError::Denied)?
                    .map_err(|_| MemberMcpError::Denied)?;
                let _selection = ServiceMcpSelection::connector(&installed);
                if operations
                    .iter()
                    .any(|name| !binding.operations.contains_key(name))
                {
                    return Err(MemberMcpError::Denied);
                }
                let account = Account {
                    slot: CredentialAccountRef {
                        realm: meerkat_core::RealmId::parse(&binding.realm_id)
                            .map_err(|_| MemberMcpError::Denied)?,
                        account: meerkat_core::CredentialAccountId::parse(&binding.slot_id)
                            .map_err(|_| MemberMcpError::Denied)?,
                    },
                    issuer: binding.issuer.clone(),
                    strategy: binding.strategy_id.clone(),
                    subject: binding.account.clone(),
                };
                if &account.slot.realm != credential_realm {
                    return Err(MemberMcpError::Denied);
                }
                let status = status.ok_or(MemberMcpError::ReauthRequired)?;
                if status.slot != account.slot {
                    return Err(MemberMcpError::PrincipalChanged);
                }
                if status.phase != ConnectorAuthPhase::Authorized {
                    return Err(MemberMcpError::ReauthRequired);
                }
                let verified = status
                    .verified_account
                    .as_ref()
                    .ok_or(MemberMcpError::ReauthRequired)?;
                if verified.issuer != account.issuer
                    || verified.strategy_id != account.strategy
                    || verified.subject != account.subject
                {
                    return Err(MemberMcpError::PrincipalChanged);
                }
                Some(account)
            }
        };
        Ok(Self {
            server: config.name.clone(),
            config_sha256: digest(
                &toolkit_package::canonical_json(config).map_err(|_| MemberMcpError::Denied)?,
            ),
            connector_approval: config.connector_approval().token().to_owned(),
            account,
            operations,
        })
    }
    fn approval(&self) -> meerkat_core::mcp_config::McpConnectorApproval {
        match &self.account {
            None => meerkat_core::mcp_config::McpConnectorApproval::NoConnector,
            Some(_) => meerkat_core::mcp_config::McpConnectorApproval::Connector(
                self.connector_approval.clone(),
            ),
        }
    }
    fn config<'a>(
        &self,
        configs: &'a [McpServerConfig],
    ) -> Result<&'a McpServerConfig, MemberMcpError> {
        let selected = unique(configs, &self.server)?;
        if digest(&toolkit_package::canonical_json(selected).map_err(|_| MemberMcpError::Denied)?)
            != self.config_sha256
            || selected.connector_approval().token() != self.connector_approval
        {
            return Err(MemberMcpError::InstallationChanged);
        }
        Ok(selected)
    }
}
fn unique<'a>(
    configs: &'a [McpServerConfig],
    name: &str,
) -> Result<&'a McpServerConfig, MemberMcpError> {
    let mut selected = configs.iter().filter(|config| config.name == name);
    let config = selected.next().ok_or(MemberMcpError::InstallationChanged)?;
    if selected.next().is_some() {
        return Err(MemberMcpError::InstallationChanged);
    }
    Ok(config)
}

/// Process-local handles captured only from the actual native factory offer.
/// No authority or application desired state is stored here.
pub(crate) struct MemberMcpFactory {
    installation: GatewayRealmMcpInstallation,
    auth: HostAuthService,
}
impl MemberMcpFactory {
    pub(crate) fn capture(
        construction: &GatewayFactoryConstruction<'_>,
    ) -> Result<Self, MemberMcpError> {
        Ok(Self {
            installation: construction
                .realm_mcp_installation()
                .map_err(|_| MemberMcpError::Unavailable)?,
            auth: construction
                .host_auth_service()
                .map_err(|_| MemberMcpError::Unavailable)?,
        })
    }
}

/// Only the actual installed factory can supply these native owner handles.
/// It is not constructible from a server name, account label or wire config.
pub struct MemberMcpSetupHost {
    installation: GatewayRealmMcpInstallation,
    auth: HostAuthService,
    source: Weak<ToolkitOriginalOwner>,
    lifetime: Weak<NativeHostLifetime>,
    store: Weak<dyn ToolkitStore>,
}
impl MemberMcpSetupHost {
    pub(crate) fn check_owner(
        &self,
        source: &Arc<ToolkitOriginalOwner>,
        store: &Arc<dyn ToolkitStore>,
    ) -> Result<(), MemberMcpError> {
        self.lifetime
            .upgrade()
            .ok_or(MemberMcpError::Unavailable)?
            .validate_current_selection()?;
        if !self
            .source
            .upgrade()
            .is_some_and(|value| Arc::ptr_eq(&value, source))
            || !self
                .store
                .upgrade()
                .is_some_and(|value| Arc::ptr_eq(&value, store))
        {
            return Err(MemberMcpError::Denied);
        }
        Ok(())
    }
    async fn select(
        &self,
        server: &str,
        operations: BTreeSet<String>,
    ) -> Result<Installation, MemberMcpError> {
        let lifetime = self.lifetime.upgrade().ok_or(MemberMcpError::Unavailable)?;
        lifetime.validate_current_selection()?;
        let (configs, _) = self
            .installation
            .servers()
            .await
            .map_err(|_| MemberMcpError::Unavailable)?;
        let config = unique(&configs, server)?;
        let status = if let Some(binding) = &config.connector {
            let slot = CredentialAccountRef {
                realm: meerkat_core::RealmId::parse(&binding.realm_id)
                    .map_err(|_| MemberMcpError::Denied)?,
                account: meerkat_core::CredentialAccountId::parse(&binding.slot_id)
                    .map_err(|_| MemberMcpError::Denied)?,
            };
            if &slot.realm != self.installation.realm() {
                return Err(MemberMcpError::Denied);
            }
            Some(
                self.auth
                    .connector_status(&slot)
                    .await
                    .map_err(|_| MemberMcpError::Unavailable)?,
            )
        } else {
            None
        };
        let selected = Installation::capture(
            self.installation.realm(),
            config,
            status.as_ref(),
            operations,
        )?;
        // Account I/O is not a lock over the config owner. Re-read the exact
        // server, without treating another server's edit as this one's change.
        let (current, _) = self
            .installation
            .servers()
            .await
            .map_err(|_| MemberMcpError::Unavailable)?;
        selected.config(&current)?;
        lifetime.validate_current_selection()?;
        Ok(selected)
    }
    async fn recheck(&self, selected: &Installation) -> Result<(), MemberMcpError> {
        if self
            .select(&selected.server, selected.operations.clone())
            .await?
            != *selected
        {
            return Err(MemberMcpError::InstallationChanged);
        }
        Ok(())
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Intent {
    binding: MemberMcpBindingRef,
    target: MemberMcpTarget,
    installation: Installation,
    origin: MemberMcpOrigin,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    accepted_requirement: Option<AcceptedMemberMcpRequirement>,
    operator: PrincipalRef,
    executor: PrincipalRef,
    audience: AudienceRef,
    represented_subject: Option<PrincipalRef>,
    restrictions: ExecutionRestrictions,
}
impl Intent {
    fn body(&self) -> serde_json::Value {
        let mut body = serde_json::json!({
            "phase":"member_configured_mcp_access", "binding":self.binding,
            "admission":self.target, "installation":self.installation,
            "origin":self.origin, "operator":self.operator, "executor":self.executor,
            "audience":self.audience, "represented_subject":self.represented_subject,
            "restrictions":self.restrictions,
            "notice":"New access to this exact current own configuration. This does not install software, infer package provenance, broaden the member class, or prove a successful service call. Native grants remain current authority."
        });
        // Preserve the existing OperatorSelf approval body byte-for-byte.
        if let Some(requirement) = &self.accepted_requirement {
            body["accepted_requirement"] = serde_json::json!(requirement);
            if requirement.contract == MemberMcpRequirementContract::ObservedRequirement {
                body["requirement_contract"] = serde_json::json!("observed_requirement");
                body["requirement_notice"] = serde_json::json!(
                    "Observed requirement: the remote owner is read before and after issuance. Drift withholds the binding and requires exact grant revocation; this is not a synchronous current-owner check."
                );
            }
        }
        body
    }
    fn service(&self) -> MemberMcpIntent {
        MemberMcpIntent {
            server: self.installation.server.clone(),
            operations: self.installation.operations.clone(),
            lifetime: self.restrictions.lifetime.clone(),
        }
    }
    fn check_requirement_shape(&self) -> Result<(), MemberMcpError> {
        match (&self.origin, &self.accepted_requirement) {
            (MemberMcpOrigin::OperatorSelf, None) => Ok(()),
            (MemberMcpOrigin::AcceptedApplicationRequirement { source }, Some(value)) => {
                value.check(
                    &MemberMcpAdmissionSelection::from_target(&self.target),
                    source,
                )?;
                if !value.services.contains(&self.service()) {
                    return Err(MemberMcpError::Denied);
                }
                Ok(())
            }
            _ => Err(MemberMcpError::Denied),
        }
    }
    fn check_requirement_current(
        &self,
        originals: &PrivateOriginals,
    ) -> Result<(), MemberMcpError> {
        self.check_requirement_shape()?;
        if let Some(value) = &self.accepted_requirement {
            value.check_owner(
                originals
                    .member_mcp_application
                    .get()
                    .ok_or(MemberMcpError::Unavailable)?
                    .as_ref(),
            )?;
        }
        Ok(())
    }
    async fn reobserve_requirement(
        &self,
        originals: &PrivateOriginals,
    ) -> Result<(), MemberMcpError> {
        self.check_requirement_shape()?;
        let current =
            observe_requirement(originals, &self.target, &self.origin, &self.service()).await?;
        if current != self.accepted_requirement {
            return Err(MemberMcpError::Denied);
        }
        self.check_requirement_current(originals)
    }
}

async fn observe_requirement(
    originals: &PrivateOriginals,
    target: &MemberMcpTarget,
    origin: &MemberMcpOrigin,
    service: &MemberMcpIntent,
) -> Result<Option<AcceptedMemberMcpRequirement>, MemberMcpError> {
    let MemberMcpOrigin::AcceptedApplicationRequirement { source } = origin else {
        return Ok(None);
    };
    let participant = originals
        .member_mcp_application
        .get()
        .ok_or(MemberMcpError::Unavailable)?;
    let selection = MemberMcpAdmissionSelection::from_target(target);
    let value = participant
        .accepted_requirement(selection.clone(), source.clone())
        .await?;
    value.check(&selection, source)?;
    if !value.services.contains(service) {
        return Err(MemberMcpError::Denied);
    }
    value.check_owner(participant.as_ref())?;
    Ok(Some(value))
}

/// The existing protected presenter retains this exact request. Its response
/// supplies only the matching ApprovalRecord, never a decoded permission body.
#[derive(Clone)]
pub struct MemberMcpReview {
    intent: Intent,
    request: ApprovalRequest,
    source: Weak<ToolkitOriginalOwner>,
    lifetime: Weak<NativeHostLifetime>,
}
impl MemberMcpReview {
    pub(crate) fn accepted_requirement(&self) -> Option<&AcceptedMemberMcpRequirement> {
        self.intent.accepted_requirement.as_ref()
    }
    pub fn binding(&self) -> &MemberMcpBindingRef {
        &self.intent.binding
    }
    pub fn request(&self) -> &ApprovalRequest {
        &self.request
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Record {
    intent: Intent,
    request: ApprovalRequest,
    approval: Option<ApprovalRecord>,
    issued: ParentIssuance,
    cancelled: bool,
    /// Only an observed native revoke (or observed nonissuance under the same
    /// writer) settles cleanup. Uncertainty retains this field as None.
    cleanup: Option<Cleanup>,
    /// Only observed requirements need this post-observation publication fence.
    /// Legacy current-owner records retain their existing decoding and behavior.
    #[serde(default, skip_serializing_if = "is_false")]
    observed_requirement_published: bool,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Cleanup {
    NotIssued,
    Revoked,
}
fn is_false(value: &bool) -> bool {
    !value
}
impl Record {
    fn observed_requirement(&self) -> bool {
        self.intent
            .accepted_requirement
            .as_ref()
            .is_some_and(|value| {
                value.contract == MemberMcpRequirementContract::ObservedRequirement
            })
    }
    pub(crate) fn observed_cleanup_binding(&self) -> Option<&MemberMcpBindingRef> {
        (self.observed_requirement()
            && (self.cancelled || !self.observed_requirement_published)
            && self.cleanup.is_none())
        .then_some(&self.intent.binding)
    }
    fn check_published(&self) -> Result<(), MemberMcpError> {
        if self.observed_requirement() && !self.observed_requirement_published {
            return Err(MemberMcpError::PendingReconciliation);
        }
        Ok(())
    }
    pub(crate) fn accepted_requirement(&self) -> Option<&AcceptedMemberMcpRequirement> {
        self.intent.accepted_requirement.as_ref()
    }
    pub(crate) fn target(&self) -> &MemberMcpTarget {
        &self.intent.target
    }
    pub(crate) fn validate_binding(
        &self,
        binding: &MemberMcpBindingRef,
        source: &ToolkitOriginalOwner,
    ) -> Result<(), MemberMcpError> {
        self.validate_retained_binding(binding, source)?;
        if self.cancelled {
            return Err(MemberMcpError::Cancelled);
        }
        Ok(())
    }
    pub(crate) fn validate_retained_binding(
        &self,
        binding: &MemberMcpBindingRef,
        source: &ToolkitOriginalOwner,
    ) -> Result<(), MemberMcpError> {
        self.intent.check_requirement_shape()?;
        if &self.intent.binding != binding
            || self.request.proposed_action.body != Some(self.intent.body())
            || self.intent.operator.validate_qualified().is_err()
            || self.intent.executor != self.intent.target.instance.member.logical_owner
            || self.intent.audience != self.intent.target.instance.member.audience
            || self.intent.represented_subject.is_some()
        {
            return Err(MemberMcpError::Denied);
        }
        let expected = restrictions(
            source,
            &self.intent.target,
            &self.intent.installation,
            self.intent.restrictions.lifetime.clone(),
        );
        if expected != self.intent.restrictions {
            return Err(MemberMcpError::Denied);
        }
        Ok(())
    }
    /// Identity of an actually approved/issued decision, even after revoke.
    /// This does not resolve permission and is only used for immutable input
    /// source binding. Native checks the selected target on every operation.
    pub(crate) fn retained_lineage(
        &self,
        source: &ToolkitOriginalOwner,
    ) -> Result<Vec<GrantLineageRef>, MemberMcpError> {
        self.validate_retained_binding(&self.intent.binding, source)?;
        self.check_published()?;
        let approval = self.approval.as_ref().ok_or(MemberMcpError::Denied)?;
        if !same_request(&self.request, approval) || !approved(approval) {
            return Err(MemberMcpError::Denied);
        }
        let ParentIssuance::Issued { lineage } = &self.issued else {
            return Err(MemberMcpError::PendingReconciliation);
        };
        if lineage.len() != 1
            || lineage[0].root_authority != self.intent.operator
            || source
                .grants()
                .issued_reference(&self.intent.binding.0)
                .map_err(|_| MemberMcpError::Unavailable)?
                .as_ref()
                != lineage.first()
        {
            return Err(MemberMcpError::Denied);
        }
        Ok(lineage.clone())
    }
    pub(crate) fn lineage(
        &self,
        source: &ToolkitOriginalOwner,
    ) -> Result<Vec<GrantLineageRef>, MemberMcpError> {
        self.check_published()?;
        self.issued_lineage(source)
    }
    fn issued_lineage(
        &self,
        source: &ToolkitOriginalOwner,
    ) -> Result<Vec<GrantLineageRef>, MemberMcpError> {
        if self.cancelled {
            return Err(MemberMcpError::Cancelled);
        }
        let approval = self.approval.as_ref().ok_or(MemberMcpError::Denied)?;
        if !same_request(&self.request, approval) || !approved(approval) {
            return Err(MemberMcpError::Denied);
        }
        let ParentIssuance::Issued { lineage } = &self.issued else {
            return Err(MemberMcpError::PendingReconciliation);
        };
        if lineage.len() != 1 || lineage[0].root_authority != self.intent.operator {
            return Err(MemberMcpError::Denied);
        }
        let grant = source
            .grants()
            .resolve_lineage(
                lineage,
                &self.intent.executor,
                self.intent.represented_subject.as_ref(),
            )
            .map_err(|error| match error {
                meerkat_authorization::grants::GrantRefusal::Denied => MemberMcpError::Denied,
                _ => MemberMcpError::Unavailable,
            })?;
        if grant.restrictions() != &self.intent.restrictions {
            return Err(MemberMcpError::Denied);
        }
        grant
            .check_current()
            .map_err(|_| MemberMcpError::Unavailable)?;
        Ok(lineage.clone())
    }
    pub(crate) async fn recheck(&self, host: &MemberMcpSetupHost) -> Result<(), MemberMcpError> {
        host.recheck(&self.intent.installation).await
    }
    pub(crate) fn server(&self) -> &str {
        &self.intent.installation.server
    }
    pub(crate) fn executor(&self) -> &PrincipalRef {
        &self.intent.executor
    }
    pub(crate) fn represented_subject(&self) -> Option<&PrincipalRef> {
        self.intent.represented_subject.as_ref()
    }
}
fn restrictions(
    source: &ToolkitOriginalOwner,
    target: &MemberMcpTarget,
    installation: &Installation,
    lifetime: LifetimeRestriction,
) -> ExecutionRestrictions {
    let mut restrictions = crate::installed_host::application_operations::mcp_access_restrictions(
        &source.ingress_domain().authority,
        &target.instance.member.logical_owner,
        &target.instance.member.audience,
        &installation.server,
        installation.operations.iter().map(String::as_str),
        &installation.approval(),
    );
    restrictions.lifetime = lifetime;
    restrictions
}

impl OperatorSetup {
    pub fn member_mcp_setup_host(
        &self,
        construction: &GatewayFactoryConstruction<'_>,
    ) -> Result<MemberMcpSetupHost, MemberMcpError> {
        self.member_mcp_setup_host_from_factory(&MemberMcpFactory::capture(construction)?)
    }
    pub(crate) fn member_mcp_setup_host_from_factory(
        &self,
        factory: &MemberMcpFactory,
    ) -> Result<MemberMcpSetupHost, MemberMcpError> {
        self.member_mcp_current()?;
        // The factory owns the installation's credential/config realm. The
        // canonical member realm is checked separately by admission and native
        // registration; it is not the factory's credential namespace.
        Ok(MemberMcpSetupHost {
            installation: factory.installation.clone(),
            auth: factory.auth.clone(),
            source: Arc::downgrade(&self.source),
            lifetime: Arc::downgrade(&self.lifetime),
            store: Arc::downgrade(&self.originals),
        })
    }
    /// Prepare against a real Reserved admission, before a native session is
    /// allocated. The caller retains binding() even if a later reply is lost.
    pub async fn prepare_member_mcp_review(
        &self,
        originals: &PrivateOriginals,
        host: &MemberMcpSetupHost,
        admitted: &AdmittedMemberClass,
        origin: MemberMcpOrigin,
        intent: MemberMcpIntent,
        request: ApprovalRequest,
    ) -> Result<MemberMcpReview, MemberMcpError> {
        self.check_member_mcp_inputs(originals, host)?;
        let target = originals.member_mcp_target(admitted).await?;
        let installation = host.select(&intent.server, intent.operations).await?;
        self.retain_member_mcp_review(
            originals,
            target,
            origin,
            installation,
            intent.lifetime,
            request,
        )
        .await
    }
    /// Recover this exact retained review after a lost reply or owner restart.
    /// The original approval is still required; this does not mint permission.
    pub async fn resume_member_mcp_review(
        &self,
        originals: &PrivateOriginals,
        binding: &MemberMcpBindingRef,
    ) -> Result<MemberMcpReview, MemberMcpError> {
        self.check_member_mcp_owner(originals)?;
        let (_, record) = self.read_member_mcp(binding).await?;
        record.validate_binding(binding, &self.source)?;
        originals
            .member_mcp_check_target(&record.intent.target)
            .await?;
        record.intent.reobserve_requirement(originals).await?;
        Ok(MemberMcpReview {
            intent: record.intent,
            request: record.request,
            source: Arc::downgrade(&self.source),
            lifetime: Arc::downgrade(&self.lifetime),
        })
    }
    async fn retain_member_mcp_review(
        &self,
        originals: &PrivateOriginals,
        target: MemberMcpTarget,
        origin: MemberMcpOrigin,
        installation: Installation,
        lifetime: LifetimeRestriction,
        mut request: ApprovalRequest,
    ) -> Result<MemberMcpReview, MemberMcpError> {
        self.check_member_mcp_owner(originals)?;
        let accepted_requirement = observe_requirement(
            originals,
            &target,
            &origin,
            &MemberMcpIntent {
                server: installation.server.clone(),
                operations: installation.operations.clone(),
                lifetime: lifetime.clone(),
            },
        )
        .await?;
        let binding = new_binding()?;
        let intent = Intent {
            binding,
            restrictions: restrictions(&self.source, &target, &installation, lifetime),
            executor: target.instance.member.logical_owner.clone(),
            audience: target.instance.member.audience.clone(),
            represented_subject: None,
            operator: self.operator.clone(),
            target,
            installation,
            origin,
            accepted_requirement,
        };
        request.proposed_action.summary =
            "Authorize this member's exact configured-service operations".into();
        request.proposed_action.body = Some(intent.body());
        let review = MemberMcpReview {
            intent,
            request,
            source: Arc::downgrade(&self.source),
            lifetime: Arc::downgrade(&self.lifetime),
        };
        let record = Record {
            intent: review.intent.clone(),
            request: review.request.clone(),
            approval: None,
            issued: ParentIssuance::Pending,
            cancelled: false,
            cleanup: None,
            observed_requirement_published: false,
        };
        let key = key(&self.member_mcp_scope(), review.binding())?;
        originals
            .member_mcp_check_target(&review.intent.target)
            .await?;
        review.intent.check_requirement_current(originals)?;
        let saved = self
            .originals
            .admit(AdmissionRequest {
                key: key.clone(),
                admission_key: key.id,
                data: encode(&record)?,
                events: vec![],
                effects: vec![],
            })
            .await
            .map_err(|_| MemberMcpError::PendingReconciliation)?;
        let actual = decode(saved.record())?;
        if actual.intent != record.intent || actual.request != record.request {
            return Err(MemberMcpError::Denied);
        }
        Ok(review)
    }
    /// Keep the returned future through settlement. Cancellation is explicit by
    /// the retained binding id; an absent reply never permits a fresh issuance.
    pub async fn finalize_member_mcp_review<F: Fn() -> bool + Sync>(
        &self,
        originals: &PrivateOriginals,
        host: &MemberMcpSetupHost,
        review: &MemberMcpReview,
        approval: ApprovalRecord,
        presented: &F,
    ) -> Result<MemberMcpBindingRef, MemberMcpError> {
        self.check_member_mcp_inputs(originals, host)?;
        self.check_member_review(review)?;
        review.intent.reobserve_requirement(originals).await?;
        host.recheck(&review.intent.installation).await?;
        self.approve_and_issue_member_mcp(originals, review, approval, presented)
            .await?;
        // Cross-owner I/O cannot make an issued grant unissued. Drift after
        // native entry withholds the binding and leaves exact cleanup owed.
        if host.recheck(&review.intent.installation).await.is_err()
            || review
                .intent
                .reobserve_requirement(originals)
                .await
                .is_err()
            || originals
                .member_mcp_check_target(&review.intent.target)
                .await
                .is_err()
            || !presented()
        {
            self.revoke_member_mcp_binding(originals, review.binding())
                .await?;
            return Err(MemberMcpError::Denied);
        }
        self.settle_member_mcp_review(originals, review).await
    }
    async fn settle_member_mcp_review(
        &self,
        originals: &PrivateOriginals,
        review: &MemberMcpReview,
    ) -> Result<MemberMcpBindingRef, MemberMcpError> {
        let finished = self.finish_member_mcp_review(originals, review).await;
        if finished.is_err() && review.intent.accepted_requirement.is_some() {
            // The final awaited read can observe source drift too. Preserve
            // exact cleanup custody for the already-issued native reference.
            self.revoke_member_mcp_binding(originals, review.binding())
                .await?;
        }
        finished
    }
    async fn finish_member_mcp_review(
        &self,
        originals: &PrivateOriginals,
        review: &MemberMcpReview,
    ) -> Result<MemberMcpBindingRef, MemberMcpError> {
        self.check_member_mcp_owner(originals)?;
        if review
            .intent
            .accepted_requirement
            .as_ref()
            .is_some_and(|value| {
                value.contract == MemberMcpRequirementContract::ObservedRequirement
            })
        {
            review.intent.reobserve_requirement(originals).await?;
        }
        let _writer = self.source.source_writer().await;
        let (saved, mut record) = self.read_member_mcp(review.binding()).await?;
        if record.intent != review.intent || record.request != review.request {
            return Err(MemberMcpError::Denied);
        }
        if record.cancelled {
            return Err(if record.cleanup.is_none() {
                MemberMcpError::PendingReconciliation
            } else {
                MemberMcpError::Cancelled
            });
        }
        // Revoke shares this writer. The post-I/O success read cannot escape
        // over a cancellation committed while configuration was being checked.
        originals
            .member_mcp_policy()?
            .with_current_member(
                &record.intent.target.id,
                &record.intent.target.class,
                &record.intent.target.instance,
                || {
                    self.lifetime.with_current_setup(|| {
                        record
                            .intent
                            .check_requirement_current(originals)
                            .map_err(|error| {
                                if error == MemberMcpError::Unavailable {
                                    HostLifetimeError::Unavailable
                                } else {
                                    HostLifetimeError::Denied
                                }
                            })?;
                        record
                            .issued_lineage(&self.source)
                            .map_err(|error| match error {
                                MemberMcpError::Denied => HostLifetimeError::Denied,
                                _ => HostLifetimeError::NotSettled,
                            })?;
                        Ok(())
                    })
                },
            )
            .await
            .map_err(|error| match error {
                HostLifetimeError::NotSettled => MemberMcpError::PendingReconciliation,
                other => other.into(),
            })?;
        if record.observed_requirement() && !record.observed_requirement_published {
            record.observed_requirement_published = true;
            self.save_member_mcp(&saved, &record).await?;
        }
        Ok(review.binding().clone())
    }
    async fn approve_and_issue_member_mcp<F: Fn() -> bool + Sync>(
        &self,
        originals: &PrivateOriginals,
        review: &MemberMcpReview,
        approval: ApprovalRecord,
        presented: &F,
    ) -> Result<(), MemberMcpError> {
        self.check_member_mcp_owner(originals)?;
        self.check_member_review(review)?;
        if !same_request(&review.request, &approval) {
            return Err(MemberMcpError::Denied);
        }
        if review
            .intent
            .accepted_requirement
            .as_ref()
            .is_some_and(|value| {
                value.contract == MemberMcpRequirementContract::ObservedRequirement
            })
        {
            review.intent.reobserve_requirement(originals).await?;
        }
        let _writer = self.source.source_writer().await;
        let (mut saved, mut record) = self.read_member_mcp(review.binding()).await?;
        if record.intent != review.intent || record.request != review.request {
            return Err(MemberMcpError::Denied);
        }
        if record.cancelled {
            return Err(MemberMcpError::Cancelled);
        }
        if let Some(existing) = &record.approval {
            if existing != &approval {
                return Err(MemberMcpError::Denied);
            }
        } else {
            record.approval = Some(approval.clone());
            if !approved(&approval) {
                record.issued = ParentIssuance::Denied;
            }
            saved = self.save_member_mcp(&saved, &record).await?;
        }
        if !approved(&approval)
            || matches!(
                record.issued,
                ParentIssuance::Denied | ParentIssuance::NativeRefused
            )
        {
            return Err(MemberMcpError::Denied);
        }
        let native_id =
            EvidenceId::new(saved.key.id.clone()).map_err(|_| MemberMcpError::Unavailable)?;
        let policy = originals.member_mcp_policy()?;
        let reference = policy
            .with_current_member(
                &record.intent.target.id,
                &record.intent.target.class,
                &record.intent.target.instance,
                || {
                    self.lifetime.with_current_setup(|| {
                        if !presented() {
                            return Err(HostLifetimeError::Denied);
                        }
                        record
                            .intent
                            .check_requirement_current(originals)
                            .map_err(|error| {
                                if error == MemberMcpError::Unavailable {
                                    HostLifetimeError::Unavailable
                                } else {
                                    HostLifetimeError::Denied
                                }
                            })?;
                        let grants = self.source.grants();
                        let reference = match grants.issued_reference(&native_id) {
                            Ok(Some(reference)) => Ok(reference),
                            Ok(None) if matches!(record.issued, ParentIssuance::Pending) => {
                                if expired(&approval) {
                                    return Err(HostLifetimeError::Denied);
                                }
                                grants.issue_root(
                                    &meerkat_core::AdmissionGuard::none(),
                                    &self.operator,
                                    native_id,
                                    record.intent.executor.clone(),
                                    record.intent.represented_subject.clone(),
                                    record.intent.restrictions.clone(),
                                )
                            }
                            _ => return Err(HostLifetimeError::NotSettled),
                        };
                        Ok(reference)
                    })
                },
            )
            .await
            .map_err(|error| {
                if matches!(error, HostLifetimeError::NotSettled) {
                    MemberMcpError::PendingReconciliation
                } else {
                    error.into()
                }
            })?;
        let reference = match reference {
            Ok(reference) => reference,
            Err(meerkat_authorization::grants::GrantRefusal::Denied) => {
                record.issued = ParentIssuance::NativeRefused;
                self.save_member_mcp(&saved, &record).await?;
                return Err(MemberMcpError::Denied);
            }
            Err(_) => return Err(MemberMcpError::PendingReconciliation),
        };
        let lineage = vec![reference];
        if let ParentIssuance::Issued { lineage: prior } = &record.issued
            && prior != &lineage
        {
            return Err(MemberMcpError::Denied);
        }
        record.issued = ParentIssuance::Issued { lineage };
        self.save_member_mcp(&saved, &record).await?;
        record.issued_lineage(&self.source)?;
        Ok(())
    }
    /// Cancel this exact review/binding and revoke only its observed native
    /// grant. This never changes shared credentials or another member's record.
    pub async fn revoke_member_mcp_binding(
        &self,
        originals: &PrivateOriginals,
        binding: &MemberMcpBindingRef,
    ) -> Result<(), MemberMcpError> {
        // Cleanup narrows an exact retained grant and must survive activation
        // closure. Keep the same durable source/store pair; new issuance still
        // requires check_member_mcp_owner and with_current_setup.
        originals.member_mcp_owner(&self.source, &self.originals)?;
        let _writer = self.source.source_writer().await;
        let (mut saved, mut record) = self.read_member_mcp(binding).await?;
        if !record.cancelled {
            record.cancelled = true;
            saved = self.save_member_mcp(&saved, &record).await?;
        }
        if record.cleanup.is_some() {
            return Ok(());
        }
        let native_id =
            EvidenceId::new(saved.key.id.clone()).map_err(|_| MemberMcpError::Unavailable)?;
        let grants = self.source.grants();
        match grants
            .issued_reference(&native_id)
            .map_err(|_| MemberMcpError::PendingReconciliation)?
        {
            None => {
                if matches!(record.issued, ParentIssuance::Issued { .. }) {
                    return Err(MemberMcpError::PendingReconciliation);
                }
                record.cleanup = Some(Cleanup::NotIssued);
            }
            Some(reference) => {
                let lineage = vec![reference];
                if lineage[0].root_authority != self.operator {
                    return Err(MemberMcpError::Denied);
                }
                match &record.issued {
                    ParentIssuance::Issued { lineage: prior } if prior != &lineage => {
                        return Err(MemberMcpError::Denied);
                    }
                    ParentIssuance::Issued { .. } => {}
                    _ => {
                        let grant = grants
                            .resolve_lineage(
                                &lineage,
                                &record.intent.executor,
                                record.intent.represented_subject.as_ref(),
                            )
                            .map_err(|_| MemberMcpError::PendingReconciliation)?;
                        if grant.restrictions() != &record.intent.restrictions {
                            return Err(MemberMcpError::Denied);
                        }
                        record.issued = ParentIssuance::Issued {
                            lineage: lineage.clone(),
                        };
                        saved = self.save_member_mcp(&saved, &record).await?;
                    }
                }
                let machine = originals
                    .machine()
                    .map_err(|_| MemberMcpError::PendingReconciliation)?;
                let mut custody = machine
                    .try_controller_grant_mutation()
                    .map_err(|_| MemberMcpError::PendingReconciliation)?;
                grants
                    .revoke(
                        &meerkat_core::AdmissionGuard::none(),
                        &self.operator,
                        &lineage[0],
                        &mut custody,
                    )
                    .map_err(|_| MemberMcpError::PendingReconciliation)?;
                record.cleanup = Some(Cleanup::Revoked);
            }
        }
        self.save_member_mcp(&saved, &record).await?;
        Ok(())
    }
    fn member_mcp_current(&self) -> Result<(), MemberMcpError> {
        self.lifetime.validate_current_selection()?;
        if !self
            .source
            .has_installed_pair_recovery()
            .map_err(|_| MemberMcpError::Unavailable)?
        {
            return Err(MemberMcpError::Unavailable);
        }
        self.originals
            .capabilities()
            .require_durable_contract()
            .map_err(|_| MemberMcpError::Unavailable)
    }
    fn check_member_mcp_owner(&self, originals: &PrivateOriginals) -> Result<(), MemberMcpError> {
        self.member_mcp_current()?;
        originals.member_mcp_owner(&self.source, &self.originals)
    }
    fn check_member_mcp_inputs(
        &self,
        originals: &PrivateOriginals,
        host: &MemberMcpSetupHost,
    ) -> Result<(), MemberMcpError> {
        self.check_member_mcp_owner(originals)?;
        host.check_owner(&self.source, &self.originals)
    }
    fn check_member_review(&self, review: &MemberMcpReview) -> Result<(), MemberMcpError> {
        if !review
            .source
            .upgrade()
            .is_some_and(|owner| Arc::ptr_eq(&owner, &self.source))
            || !review
                .lifetime
                .upgrade()
                .is_some_and(|owner| Arc::ptr_eq(&owner, &self.lifetime))
        {
            return Err(MemberMcpError::Denied);
        }
        Ok(())
    }
    fn member_mcp_scope(&self) -> toolkit_store::StoreScope {
        toolkit_store::StoreScope {
            realm: self.native.realm.to_string(),
            owner: self.lifetime.selected_policy().deployment.to_string(),
        }
    }
    async fn read_member_mcp(
        &self,
        binding: &MemberMcpBindingRef,
    ) -> Result<(StoredRecord, Record), MemberMcpError> {
        let saved = self
            .originals
            .get(&key(&self.member_mcp_scope(), binding)?)
            .await
            .map_err(|_| MemberMcpError::Unavailable)?
            .ok_or(MemberMcpError::Denied)?;
        let record = decode(&saved)?;
        if record.intent.operator != self.operator || &record.intent.binding != binding {
            return Err(MemberMcpError::Denied);
        }
        Ok((saved, record))
    }
    async fn save_member_mcp(
        &self,
        saved: &StoredRecord,
        record: &Record,
    ) -> Result<StoredRecord, MemberMcpError> {
        self.originals
            .compare_and_swap(Transition {
                key: saved.key.clone(),
                expected_revision: saved.revision,
                expected_digest: Some(saved.content_digest.clone()),
                claim: None,
                data: encode(record)?,
                events: vec![],
                effects: vec![],
            })
            .await
            .map_err(|_| MemberMcpError::PendingReconciliation)
    }
}
pub(crate) fn key(
    scope: &toolkit_store::StoreScope,
    binding: &MemberMcpBindingRef,
) -> Result<StoreKey, MemberMcpError> {
    if !binding
        .0
        .as_str()
        .strip_prefix("member-mcp-")
        .is_some_and(|suffix| suffix.len() == 64 && suffix.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return Err(MemberMcpError::Denied);
    }
    Ok(StoreKey {
        realm: scope.realm.clone(),
        owner: scope.owner.clone(),
        id: binding.0.as_str().to_owned(),
    })
}
pub(crate) fn decode(saved: &StoredRecord) -> Result<Record, MemberMcpError> {
    if saved.data.schema != SCHEMA
        || saved.data.version != 1
        || saved.data.digest().ok().as_deref() != Some(saved.content_digest.as_str())
    {
        return Err(MemberMcpError::Unavailable);
    }
    let record: Record =
        serde_json::from_value(saved.data.body.clone()).map_err(|_| MemberMcpError::Unavailable)?;
    if key(&saved.key.scope(), &record.intent.binding)? != saved.key {
        return Err(MemberMcpError::Denied);
    }
    Ok(record)
}
fn encode(record: &Record) -> Result<RecordData, MemberMcpError> {
    Ok(RecordData {
        schema: SCHEMA.into(),
        version: 1,
        body: serde_json::to_value(record).map_err(|_| MemberMcpError::Unavailable)?,
    })
}
fn approved(record: &ApprovalRecord) -> bool {
    record.status == ApprovalStatus::Approved
        && record
            .decision
            .as_ref()
            .is_some_and(|decision| decision.decision == ApprovalDecision::Approve)
}
fn expired(record: &ApprovalRecord) -> bool {
    record
        .expires_at
        .is_some_and(|expiry| std::time::SystemTime::from(expiry) <= std::time::SystemTime::now())
}
fn new_binding() -> Result<MemberMcpBindingRef, MemberMcpError> {
    use std::io::Read;
    let mut bytes = [0u8; 32];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut bytes))
        .map_err(|_| MemberMcpError::Unavailable)?;
    EvidenceId::new(format!("member-mcp-{}", digest(&bytes)))
        .map(MemberMcpBindingRef)
        .map_err(|_| MemberMcpError::Unavailable)
}

#[cfg(test)]
mod tests;
