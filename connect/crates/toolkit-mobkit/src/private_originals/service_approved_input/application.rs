//! Application policy approves one actual waiting task. Native grants remain
//! the sole access owner. An explicitly selected existing operator may issue
//! one exact task parent; application policy cannot create an operator root.

use super::*;
use meerkat_toolkit::host_lifetime::{NativeHostLifetime, control::HostLifetimeError};
use std::collections::BTreeSet;
use crate::installed_host::operator_setup::{ServiceParentRuleEvidence, StandingServiceParentProposal};
use meerkat_toolkit::host_lifetime::policy::ServiceParentAdministration;
mod administration;
mod operation;
pub use administration::{ApplicationServiceParentAuthorizer, ServiceParentAdministrationReservation,
    ServiceParentAuthorization, ServiceParentAuthorizationProposal, ServiceParentAuthorizationRequest};
pub(crate) use administration::ServiceParentAuthorizationEvidence;
pub use operation::{ApplicationServiceOperation, ApplicationServiceOperationControl,
    ApplicationServicePreparedStatus, ServiceParentOperationObserver, ServiceParentOperationObservation,
    ServiceParentOperationOutcome, ServiceParentCustody, ServiceParentOperationReason};
pub(crate) use operation::{ServiceOperationError, ServiceOperationCurrent, ServiceOperationProgress};

/// Supplied only by trusted application composition, never by a tool argument.
/// `reserve` must authenticate the policy producer and evaluate this exact
/// retained source, fresh instruction, account/configuration and operation set.
pub trait ApplicationServicePolicy: Send + Sync {
    fn reserve(&self, request: &ApplicationServicePolicyRequest<'_>)
        -> Result<ApplicationServicePolicyDecision, HostLifetimeError>;
    /// Recovery is fresh authority to accept the retained complete candidate,
    /// not a replay of an expired task or legacy service-only Permit.
    fn reserve_tool_policy_recovery(&self, _request: &ApplicationToolPolicyRecoveryRequest<'_>)
        -> Result<ApplicationToolPolicyRecoveryDecision, HostLifetimeError>
    { Ok(ApplicationToolPolicyRecoveryDecision::RequiresHuman) }
}

/// An owned exclusion against invalidating this decision during native issue
/// and conditional source publication. It is not a native grant. Implementors
/// must not hold/reenter NativeHostLifetime phase/publication locks here.
/// Clones of its Arc may outlive a dropped caller while native issue settles.
pub trait ApplicationServicePolicyReservation: Send + Sync {
    fn owner(&self) -> &PrincipalRef;
    fn provenance(&self) -> &serde_json::Map<String, serde_json::Value>;
    fn check_current(&self) -> Result<(), HostLifetimeError>;
}

pub enum ApplicationServicePolicyDecision {
    RequiresHuman,
    Permit(Box<dyn ApplicationServicePolicyReservation>),
    PermitWithToolPolicy {
        reservation: Box<dyn ApplicationServicePolicyReservation>,
        approval: ServiceToolPolicyApproval,
    },
}

/// Borrowed from actual selected owners. No actor label or supplied lineage
/// can stand in for these inputs. Configuration must not be logged or copied
/// into provenance; provenance must be bounded, structured and nonsecret.
pub struct ApplicationServicePolicyRequest<'a> {
    pub owner: &'a PrincipalRef,
    pub deployment: &'a Id,
    pub source_policy_sha256: &'a str,
    pub route: &'a PrivateOriginalRoute,
    pub original: &'a PrivateOriginalRef,
    pub authenticated_event: &'a PrivateAuthenticatedEvent,
    pub task: &'a NativeOriginalTask,
    pub acquisition: &'a StoreKey,
    pub fresh_original: &'a EvidenceId,
    pub instruction: &'a str,
    pub configuration: &'a meerkat_core::McpServerConfig,
    pub permission: &'a PrivateServiceMcpPermission,
    pub attachment: &'a PrivateServiceMcpLiveAttachment,
    pub tool_policy: Option<&'a ServiceToolPolicyProposal>,
}

/// Fix the adapter to the exact live native host, not equivalent snapshots.
pub struct InstalledApplicationServicePolicy {
    lifetime: Arc<NativeHostLifetime>,
    owner: PrincipalRef,
    deployment: Id,
    source_policy_sha256: String,
    policy: Arc<dyn ApplicationServicePolicy>,
    parent_provisioning: Option<(PrincipalRef, ServiceParentAdministration, Arc<dyn ApplicationServiceParentAuthorizer>)>,
}
impl InstalledApplicationServicePolicy {
    pub fn new(lifetime: Arc<NativeHostLifetime>, policy: Arc<dyn ApplicationServicePolicy>)
        -> Result<Self, HostLifetimeError>
    {
        lifetime.validate_current_selection()?;
        let selected = lifetime.selected_policy();
        let native = selected.native_host.as_ref().ok_or(HostLifetimeError::InvalidSelection)?;
        Ok(Self { owner: native.grant_administrator.clone(), deployment: selected.deployment.clone(),
            source_policy_sha256: selected.source_policy_sha256.clone(), lifetime, policy, parent_provisioning: None })
    }
    /// Compose selected finite scope with the actual administrative producer.
    /// It may authorize this task from a current authenticated decision or an
    /// independently retained standing rule. Selection and this constructor
    /// authenticate neither case and do not approve a Configure UI. No
    /// operator is inferred when the installed selection omits one.
    pub fn with_task_parent_provisioning(lifetime: Arc<NativeHostLifetime>,
        policy: Arc<dyn ApplicationServicePolicy>, administrator: Arc<dyn ApplicationServiceParentAuthorizer>) -> Result<Self, HostLifetimeError> {
        let mut installed = Self::new(lifetime, policy)?;
        let native = installed.lifetime.selected_policy().native_host.as_ref()
            .ok_or(HostLifetimeError::InvalidSelection)?;
        let operator = native.remote_operator.clone().ok_or(HostLifetimeError::InvalidSelection)?;
        let rule = native.service_parent_administration.clone().ok_or(HostLifetimeError::InvalidSelection)?;
        if rule.routes().is_empty() || rule.routes().iter().any(|id| !native.routes.iter().any(|route| &route.id == id)) {
            return Err(HostLifetimeError::InvalidSelection);
        }
        installed.parent_provisioning = Some((operator, rule, administrator));
        Ok(installed)
    }
    pub fn belongs_to(&self, lifetime: &Arc<NativeHostLifetime>) -> bool {
        Arc::ptr_eq(&self.lifetime, lifetime)
    }
    fn check(&self, task: &InstalledServiceTask) -> Result<(), ServiceOperationError> {
        let lifetime = task.source.host_lifetime.get().and_then(Weak::upgrade)
            .ok_or(PrivateOriginalError::Unavailable)?;
        if !self.belongs_to(&lifetime) { return Err(PrivateOriginalError::Denied.into()); }
        self.lifetime.validate_current_selection()?;
        let selected = self.lifetime.selected_policy();
        if selected.deployment != self.deployment || selected.source_policy_sha256 != self.source_policy_sha256
            || selected.native_host.as_ref().is_none_or(|native| native.grant_administrator != self.owner)
            || !task.source.application_service_source(&task.parent, &task.task)?
        { return Err(PrivateOriginalError::Denied.into()); }
        match &self.parent_provisioning {
            None if task.source.route(&task.parent.route)?.application_parent.as_ref().is_none_or(|parent| parent.is_empty()) =>
                return Err(PrivateOriginalError::Denied.into()),
            Some((operator, rule, _)) => {
                let native = selected.native_host.as_ref().ok_or(PrivateOriginalError::Unavailable)?;
                if native.remote_operator.as_ref() != Some(operator)
                    || native.service_parent_administration.as_ref() != Some(rule)
                    || !rule.routes().contains(&task.parent.route)
                { return Err(PrivateOriginalError::Denied.into()); }
            }
            None => {}
        }
        Ok(())
    }
}

pub enum ApplicationServicePreparation {
    RequiresHuman,
    Prepared(PreparedApplicationServiceInstruction),
}

pub struct PreparedApplicationServiceInstruction {
    task: InstalledServiceTask,
    approved: ApplicationServiceDecision,
    reservation: Arc<dyn ApplicationServicePolicyReservation>,
    parent_provisioning: Option<(OperatorSetup, StandingServiceParentProposal)>,
    administrator: Option<Arc<dyn ServiceParentAdministrationReservation>>,
    tool_policy: Option<crate::installed_host::tool_policy::service_policy::PreparedServiceToolPolicy>,
    tool_policy_approval: Option<ServiceToolPolicyApproval>,
}
impl PreparedApplicationServiceInstruction {
    /// Consume this exact decision. Entered native custody and policy/login
    /// reservations remain with the issuance worker through settlement.
    pub async fn apply(self, current: impl Fn() -> bool + Send + Sync + 'static)
        -> Result<bool, PrivateOriginalError>
    {
        if self.parent_provisioning.is_some() { return Err(PrivateOriginalError::Denied); }
        self.task.current().await?;
        let login = self.task.console_login.clone();
        let reservation = login.as_ref().map(|login| login.reserve())
            .transpose().map_err(|_| PrivateOriginalError::Denied)?;
        // This owned reservation must follow an entered blocking worker even
        // when its caller is dropped. Only full source publication settles it.
        let retained_login = Arc::new(std::sync::Mutex::new(reservation));
        let worker_login = retained_login.clone();
        let result = self.apply_retained(Arc::new(move || {
            let _retained_login = &worker_login;
            login.as_ref().is_none_or(|login| login.try_check().is_ok()) && current()
        })).await;
        if result.is_ok() {
            if let Some(reservation) = retained_login.lock()
                .map_err(|_| PrivateOriginalError::Unavailable)?.take()
            { reservation.settled(); }
        }
        result
    }
    async fn apply_retained(self, current: Arc<dyn Fn() -> bool + Send + Sync>)
        -> Result<bool, PrivateOriginalError>
    {
        let Self { task, approved, reservation, parent_provisioning, administrator, tool_policy, tool_policy_approval } = self;
        if parent_provisioning.is_some() || administrator.is_some() { return Err(PrivateOriginalError::Denied); }
        task.current().await?;
        reservation.check_current().map_err(policy_error)?;
        if !current() { return Err(PrivateOriginalError::Denied); }
        let evidence = approved.decision.evidence.clone();
        let transaction = match (tool_policy, tool_policy_approval) {
            (None, None) => None,
            (Some(prepared), Some(approval)) => Some(prepared.begin(&approval, &|| {
                reservation.check_current()?;
                if current() { Ok(()) } else { Err(HostLifetimeError::Denied) }
            }).await.map_err(policy_error)?),
            _ => return Err(PrivateOriginalError::Denied),
        };
        let policy_custody = Arc::new(tokio::sync::Mutex::new(transaction));
        let worker_policy = policy_custody.clone();
        // The worker owns this same exclusion even if the outer future is
        // dropped after spawn_blocking. No host lock is acquired by predicate.
        let current = Arc::new(move || {
            let _actual_policy_custody = &worker_policy;
            reservation.check_current().map_err(policy_error)?;
            if current() { Ok(()) } else { Err(PrivateOriginalError::Denied) }
        });
        let observed = task.source.apply_application_service_access(approved, current.clone()).await?;
        match observed.state {
            PrivateServiceGrantState::GrantIssued => {}
            PrivateServiceGrantState::PendingReconciliation | PrivateServiceGrantState::GrantUnavailable =>
                return Err(PrivateOriginalError::Unavailable),
            PrivateServiceGrantState::ApprovalDenied => return Err(PrivateOriginalError::Denied),
        }
        task.current().await?;
        current()?;
        let mut held_policy = policy_custody.lock().await;
        let condition = match held_policy.as_mut() {
            Some(policy) => Some(policy.publish(&|| current().map_err(|error| match error {
                PrivateOriginalError::Unavailable => HostLifetimeError::Unavailable,
                _ => HostLifetimeError::Denied,
            })).await.map_err(policy_error)?),
            None => None,
        };
        task.retain_approved_with_policy(observed.grant, evidence, &|| current(), condition).await?;
        current()?;
        Ok(true)
    }
}

/// Historical provenance in the existing durable access decision. It cannot
/// authenticate another invocation or reconstruct a policy reservation.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::private_originals) struct ApplicationServiceEvidence {
    pub owner: PrincipalRef,
    pub deployment: Id,
    pub source_policy_sha256: String,
    pub original: PrivateOriginalRef,
    pub acquisition: StoreKey,
    pub instruction_sha256: String,
    pub provenance: serde_json::Map<String, serde_json::Value>,
    /// Omitted by existing version 4 configured-parent records. Version 5
    /// carries the exact administrative basis, never reusable authority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_parent: Option<ServiceParentRuleEvidence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_policy: Option<String>,
    /// Exact product software evidence, separately retained from policy
    /// provenance. Absent in old records; absence never permits reconstruction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installation_readback: Option<RetainedApplicationInstallation>,
}
impl ApplicationServiceEvidence {
    pub(in crate::private_originals) fn validate(&self, route: &PrivateOriginalRoute,
        decision: &PrivateServiceMcpAccessDecision, deployment: &str,
    ) -> Result<(), PrivateOriginalError> {
        if self.owner != route.grant_caller || self.deployment.as_str() != deployment
            || self.original.route != route.id
            || self.acquisition.owner != format!("toolkit/acquisition/{deployment}")
            || self.acquisition.realm != route.realm.as_str()
            || !valid_digest(&self.source_policy_sha256) || !valid_digest(&self.instruction_sha256)
            || self.tool_policy.as_ref().is_some_and(|value| !valid_digest(value))
            || self.provenance.is_empty() || canonical_json(&self.provenance).map_err(|_| PrivateOriginalError::Conflict)?.len() > 16 * 1024
            || decision.evidence.sha256 != digest(&canonical_json(self).map_err(|_| PrivateOriginalError::Conflict)?)
        { return Err(PrivateOriginalError::Conflict); }
        if let Some(selected) = &self.installation_readback { selected.validate(decision)?; }
        Ok(())
    }
    pub(in crate::private_originals) fn check_parent(&self, lifetime: &NativeHostLifetime,
        route: &PrivateOriginalRoute, parent: &[meerkat_authorization_contracts::grant::GrantLineageRef])
        -> Result<(), PrivateOriginalError> {
        self.check_parent_current(lifetime, route, parent).map_err(policy_error)
    }
    fn check_parent_current(&self, lifetime: &NativeHostLifetime,
        route: &PrivateOriginalRoute, parent: &[meerkat_authorization_contracts::grant::GrantLineageRef])
        -> Result<(), HostLifetimeError> {
        match &self.service_parent {
            Some(evidence) => {
                evidence.check_selected(lifetime, &route.id)?;
                evidence.check_parent(parent)
            }
            None if route.application_parent.as_deref() == Some(parent) => Ok(()),
            None => Err(HostLifetimeError::Denied),
        }
    }
}

/// Only this module constructs the decision after the actual policy owner has
/// reserved its exact task. A provisioned basis must consume the operator's
/// opaque exact parent before issuance; no bare lineage or deserializer exists.
pub(in crate::private_originals) struct ApplicationServiceDecision {
    source: Arc<PrivateOriginals>,
    lifetime: Arc<NativeHostLifetime>,
    decision: PrivateServiceMcpAccessDecision,
    selection: ServiceMcpSelection,
    evidence: ApplicationServiceEvidence,
    parent: Option<crate::installed_host::operator_setup::ReviewedServiceMcpParent>,
}
impl ApplicationServiceDecision {
    pub(in crate::private_originals) fn into_parts(self, source: &PrivateOriginals)
        -> Result<(PrivateServiceMcpAccessDecision, ServiceMcpSelection,
            Vec<meerkat_authorization_contracts::grant::GrantLineageRef>, Weak<NativeHostLifetime>, ApplicationServiceEvidence), ServiceOperationError>
    {
        if !std::ptr::eq(self.source.as_ref(), source)
            || source.host_lifetime.get().and_then(Weak::upgrade).is_none_or(|host| !Arc::ptr_eq(&host, &self.lifetime))
        { return Err(PrivateOriginalError::Denied.into()); }
        self.lifetime.validate_current_selection()?;
        let route = source.route(&self.decision.route)?;
        self.evidence.validate(route, &self.decision, &source.scope.owner)?;
        let parent = match (&self.evidence.service_parent, self.parent) {
            (None, None) => route.application_parent.clone().filter(|lineage| !lineage.is_empty())
                .ok_or(PrivateOriginalError::Unavailable)?,
            (Some(_), Some(parent)) => {
                let (decision, selection, lineage, lifetime) = parent.into_parts_operation(&source.owner)?;
                if decision != self.decision || selection != self.selection
                    || lifetime.upgrade().is_none_or(|host| !Arc::ptr_eq(&host, &self.lifetime))
                { return Err(PrivateOriginalError::Denied.into()); }
                lineage
            }
            _ => return Err(PrivateOriginalError::Denied.into()),
        };
        self.evidence.check_parent_current(&self.lifetime, route, &parent)?;
        Ok((self.decision, self.selection, parent, Arc::downgrade(&self.lifetime), self.evidence))
    }
}

impl InstalledServiceTask {
    pub async fn prepare_application_connector(self, policy: &InstalledApplicationServicePolicy,
        installation: &meerkat::ConnectorMcpInstallation, operations: BTreeSet<Id>, expires_in_ms: Option<u64>,
    ) -> Result<ApplicationServicePreparation, PrivateOriginalError> {
        self.prepare_application_connector_with_tool_policy(policy, installation, operations, expires_in_ms, None).await
    }
    pub async fn prepare_application_no_connector(self, policy: &InstalledApplicationServicePolicy,
        config: &meerkat_core::McpServerConfig, operations: BTreeSet<Id>, expires_in_ms: Option<u64>,
    ) -> Result<ApplicationServicePreparation, PrivateOriginalError> {
        self.prepare_application_no_connector_with_tool_policy(policy, config, operations, expires_in_ms, None).await
    }
    pub async fn prepare_application_connector_with_tool_policy(self, policy: &InstalledApplicationServicePolicy,
        installation: &meerkat::ConnectorMcpInstallation, operations: BTreeSet<Id>, expires_in_ms: Option<u64>,
        tool_policy: Option<ServiceToolPolicySelection>,
    ) -> Result<ApplicationServicePreparation, PrivateOriginalError> {
        if policy.parent_provisioning.is_some() { return Err(PrivateOriginalError::Denied); }
        self.prepare_application_service(policy, installation.config(), ServiceMcpSelection::connector(installation),
            operations, expires_in_ms, None, tool_policy).await.map_err(ServiceOperationError::source)
    }
    pub async fn prepare_application_no_connector_with_tool_policy(self, policy: &InstalledApplicationServicePolicy,
        config: &meerkat_core::McpServerConfig, operations: BTreeSet<Id>, expires_in_ms: Option<u64>,
        tool_policy: Option<ServiceToolPolicySelection>,
    ) -> Result<ApplicationServicePreparation, PrivateOriginalError> {
        if policy.parent_provisioning.is_some() { return Err(PrivateOriginalError::Denied); }
        let selection = ServiceMcpSelection::no_connector(config)?;
        self.prepare_application_service(policy, config, selection, operations, expires_in_ms, None, tool_policy)
            .await.map_err(ServiceOperationError::source)
    }
    async fn prepare_application_service(self, policy: &InstalledApplicationServicePolicy,
        config: &meerkat_core::McpServerConfig, selection: ServiceMcpSelection,
        operations: BTreeSet<Id>, expires_in_ms: Option<u64>,
        progress: Option<&ServiceOperationProgress>, tool_policy: Option<ServiceToolPolicySelection>,
    ) -> Result<ApplicationServicePreparation, ServiceOperationError> {
        let (parent, _) = self.current().await?;
        policy.check(&self)?;
        if self.installation_readback.as_ref().is_some_and(|selected|
            selected.configuration().ok().as_ref() != Some(config)) {
            return Err(PrivateOriginalError::Denied.into());
        }
        if tool_policy.is_some() && self.source.service_tool_policy.get()
            .is_none_or(|owner| !owner.same_participant(&policy.policy)) {
            return Err(PrivateOriginalError::Denied.into());
        }
        let tool_policy = self.prepare_tool_policy(tool_policy).await?;
        let route = self.source.route(&self.parent.route)?;
        let source_record = self.source.decode_record(&parent, &self.parent)?;
        let id = Id::new(self.fresh.as_str()).map_err(|_| PrivateOriginalError::Conflict)?;
        let operator = if policy.parent_provisioning.is_some() {
            // Explicit selected application operator only; do not use the
            // convenience method that falls back to the local OS operator.
            Some(self.source.application_operator_setup(policy.lifetime.clone())?)
        } else { None };
        let retained_expiry = match &operator {
            Some(operator) => operator.retained_service_parent_expiry(&self.parent, &self.acquisition,
                &id, &digest(self.instruction.as_bytes()), expires_in_ms, progress).await?,
            None => None,
        };
        let now = u64::try_from(self.store.now_ms().await.map_err(store_error)?).map_err(|_| PrivateOriginalError::Unavailable)?;
        let expiry = match retained_expiry {
            Some(expiry) if expiry <= self.before_ms => expiry,
            Some(_) => return Err(PrivateOriginalError::Denied.into()),
            None => expires_in_ms.map(|ttl| now.checked_add(ttl).ok_or(PrivateOriginalError::Denied))
                .transpose()?.unwrap_or(self.before_ms).min(self.before_ms),
        };
        let attachment = PrivateServiceMcpLiveAttachment { destination: self.parent_association.candidate().target.clone() };
        let permission = PrivateServiceMcpPermission { target: route.grant_target.clone(),
            connection: Id::new(config.name.clone()).map_err(|_| PrivateOriginalError::Denied)?,
            resource: Id::new(config.name.clone()).map_err(|_| PrivateOriginalError::Denied)?, operations,
            lifetime: RoleLifetime { scope: LifetimeScope::Task { task: id.clone() }, expires_at_ms: Some(expiry) } };
        if permission.operations.is_empty() || !permission.lifetime.active_at(now) { return Err(PrivateOriginalError::Denied.into()); }
        let (reservation, tool_policy_approval) = match policy.policy.reserve(&ApplicationServicePolicyRequest {
            owner: &policy.owner, deployment: &policy.deployment, source_policy_sha256: &policy.source_policy_sha256,
            route, original: &self.parent, authenticated_event: &source_record.event,
            task: &self.task, acquisition: &self.acquisition,
            fresh_original: &self.fresh, instruction: &self.instruction, configuration: config,
            permission: &permission, attachment: &attachment,
            tool_policy: tool_policy.as_ref().map(|prepared| &prepared.proposal),
        })? {
            ApplicationServicePolicyDecision::RequiresHuman => return Ok(ApplicationServicePreparation::RequiresHuman),
            ApplicationServicePolicyDecision::Permit(reservation) if tool_policy.is_none() => (reservation, None),
            ApplicationServicePolicyDecision::Permit(_) => return Err(PrivateOriginalError::Denied.into()),
            ApplicationServicePolicyDecision::PermitWithToolPolicy { reservation, approval } => {
                let proposal = &tool_policy.as_ref().ok_or(PrivateOriginalError::Denied)?.proposal;
                approval.check(proposal)?;
                (reservation, Some(approval))
            }
        };
        reservation.check_current()?;
        if reservation.owner() != &policy.owner { return Err(PrivateOriginalError::Denied.into()); }
        let evidence = ApplicationServiceEvidence { owner: policy.owner.clone(), deployment: policy.deployment.clone(),
            source_policy_sha256: policy.source_policy_sha256.clone(), original: self.parent.clone(),
            acquisition: self.acquisition.clone(), instruction_sha256: digest(self.instruction.as_bytes()),
            provenance: reservation.provenance().clone(),
            installation_readback: self.installation_readback.clone(),
            tool_policy: tool_policy.as_ref().map(|p| p.proposal.sha256()).transpose()?,
            service_parent: operator.as_ref().map(|operator|
                operator.service_parent_rule(&self.parent, &self.acquisition, &id)).transpose()? };
        let decision = PrivateServiceMcpAccessDecision {
            catalog: tool_policy.as_ref().and_then(|p| p.proposal.selection.catalog.as_ref())
                .map(|_| PrivateServiceMcpCatalog::OriginalSession),
            route: self.parent.route.clone(), decision: id,
            evidence: PrivateSourceEvidence { resource_id: self.fresh.clone(),
                revision: EvidenceId::new("application-service-policy-v1").map_err(|_| PrivateOriginalError::Conflict)?,
                sha256: digest(&canonical_json(&evidence).map_err(|_| PrivateOriginalError::Conflict)?) },
            choice: PrivateAccessChoice::Approve, permission, live_attachment: Some(attachment),
            connection_revision: Revision::new(digest(&canonical_json(config).map_err(|_| PrivateOriginalError::Conflict)?))
                .map_err(|_| PrivateOriginalError::Conflict)? };
        evidence.validate(route, &decision, &self.source.scope.owner)?;
        // Mechanical validation uses the same native restrictions as reviewed
        // access; current full-lineage checks still run at actual issue_child.
        selection.restrictions(route, &decision, now)?;
        self.current().await?;
        policy.check(&self)?;
        reservation.check_current()?;
        let parent_provisioning = operator.map(|operator| {
            let mut proposal = operator.prepare_standing_service_parent(self.parent.clone(), self.acquisition.clone(),
                digest(self.instruction.as_bytes()), expires_in_ms, decision.clone(), selection.clone())?;
            if let Some(prepared) = &tool_policy { proposal.set_tool_policy(prepared.proposal.clone())?; }
            Ok::<_, HostLifetimeError>((operator, proposal))
        }).transpose()?;
        let approved = ApplicationServiceDecision { source: self.source.clone(), lifetime: policy.lifetime.clone(),
            decision, selection, evidence, parent: None };
        Ok(ApplicationServicePreparation::Prepared(PreparedApplicationServiceInstruction {
            task: self, approved, reservation: Arc::from(reservation), parent_provisioning, administrator: None,
            tool_policy, tool_policy_approval,
        }))
    }
}

impl PrivateOriginals {
    /// Eligibility is the exact current compiled source mapping. Authentication
    /// comes from the retained source and access from the native task grant.
    pub(super) fn application_service_source(&self, reference: &PrivateOriginalRef, task: &NativeOriginalTask)
        -> Result<bool, PrivateOriginalError>
    {
        self.service_source_console_login(reference, task).map(|_| true)
    }

    /// Source authentication depends on the selected origin, independently of
    /// application policy. A provider's Human sender is not a Console login.
    pub(super) fn service_source_console_login(&self, reference: &PrivateOriginalRef, task: &NativeOriginalTask)
        -> Result<Option<crate::private_host::ConsoleLoginHandle>, PrivateOriginalError>
    {
        let host = self.host_lifetime.get().and_then(Weak::upgrade).ok_or(PrivateOriginalError::Unavailable)?;
        host.validate_current_selection().map_err(policy_error)?;
        let selected = host.selected_policy();
        let native = selected.native_host.as_ref().ok_or(PrivateOriginalError::Unavailable)?;
        let route = self.route(&reference.route)?;
        let compiled = native.routes.iter().find(|route| route.id == reference.route).ok_or(PrivateOriginalError::Denied)?;
        let (member, requester, actor) = compiled.source_binding(&selected).map_err(policy_error)?;
        if task.member.as_ref() != Some(member) || requester != &route.requester || actor != &route.ingress_actor
            || compiled.logical_executor != route.logical_executor || compiled.represented_subject != route.represented_subject
            || compiled.source_authority != route.source_authority || compiled.authentication_domain != route.authentication_domain
            || compiled.event_domain != route.event_domain || compiled.application_parent != route.application_parent
            || native.grant_administrator != route.grant_caller || native.realm != route.realm
        { return Err(PrivateOriginalError::Denied); }
        let retained_login = self.retained.lock().map_err(|_| PrivateOriginalError::Unavailable)?
            .iter().find(|held| &held.reference == reference)
            .and_then(|held| held.console_login.get()).cloned();
        // Never shed an actual retained login, including a stock operator's
        // optional Console-origin custody. Missing login is allowed only for
        // sources whose existing authentication does not require one.
        let retained_login = retained_login.map(|login| current_console_login(Some(login))).transpose()?;
        match &compiled.origin {
            CompiledNativeSourceOrigin::ProviderEvent { .. } => Ok(retained_login),
            CompiledNativeSourceOrigin::LocalConsole { .. } => {
                let operator = meerkat::governed_realm::stock::StockPrincipals::for_local_operator(&native.realm)
                    .map_err(|_| PrivateOriginalError::Unavailable)?.operator;
                if route.requester == operator {
                    if route.represented_subject.as_ref().is_some_and(|subject| subject != &operator) {
                        return Err(PrivateOriginalError::Denied);
                    }
                    return Ok(retained_login);
                }
                if route.requester.kind != meerkat_core::PrincipalKind::Human { return Ok(retained_login); }
                if route.represented_subject.as_ref() != Some(&route.requester) {
                    return Err(PrivateOriginalError::Denied);
                }
                current_console_login(retained_login).map(Some)
            }
        }
    }
}

fn current_console_login(login: Option<crate::private_host::ConsoleLoginHandle>)
    -> Result<crate::private_host::ConsoleLoginHandle, PrivateOriginalError>
{
    let login = login.ok_or(PrivateOriginalError::Unavailable)?;
    login.check().map_err(|_| PrivateOriginalError::Denied)?;
    Ok(login)
}

pub(super) fn policy_error(error: HostLifetimeError) -> PrivateOriginalError {
    match error {
        HostLifetimeError::Unavailable | HostLifetimeError::NotSettled => PrivateOriginalError::Unavailable,
        _ => PrivateOriginalError::Denied,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::FutureExt;
    struct NeverReserve;
    impl ApplicationServicePolicy for NeverReserve {
        fn reserve(&self, _: &ApplicationServicePolicyRequest<'_>)
            -> Result<ApplicationServicePolicyDecision, HostLifetimeError> {
            panic!("owner matching must not invoke application policy")
        }
    }
    #[test]
    fn selected_service_policy_requires_the_same_native_owner_not_matching_content() {
        let (_left_dir, left) = crate::installed_host::operator_setup::test_service_lifetime();
        let (_right_dir, right) = crate::installed_host::operator_setup::test_service_lifetime();
        let policy = InstalledApplicationServicePolicy::new(left.clone(), Arc::new(NeverReserve)).unwrap();
        assert!(policy.belongs_to(&left));
        assert!(!policy.belongs_to(&right));
    }

    // Actual native original admission and canonical task/source ownership.
    // These fixtures select no service parent. This proves source eligibility
    // independently of permission, not an authenticated Console policy grant.
    async fn source_eligibility(fixture: ContinuationFixture, kind: meerkat_core::PrincipalKind) {
        let checked = std::panic::AssertUnwindSafe(async {
            let admitted = fixture.submit().await;
            fixture.finish(&admitted).await.unwrap();
            let dispatch = fixture.probe.as_ref().unwrap().captured();
            let snapshot = super::super::admission_tests::acquisition(&fixture, &dispatch).await;
            let member = snapshot.request().host().continuation.native_original.as_ref().unwrap()
                .member.as_ref().unwrap().member.clone();
            let task = fixture.source.prepare_service_task(fixture.store.clone(), snapshot,
                "application-source-eligibility", &member).await.unwrap().unwrap();
            let route = fixture.source.route(&task.parent.route).unwrap();
            assert_eq!(route.requester.kind, kind);
            assert!(route.application_parent.is_none(), "source preparation needs no service permission");
            assert!(fixture.source.application_service_source(&task.parent, &task.task).unwrap());
            task.check_setup_current().await.unwrap();
            let mut wrong_member = task.clone();
            wrong_member.task.member = None;
            assert!(matches!(wrong_member.check_setup_current().await, Err(PrivateOriginalError::Denied)));
            let mut expired = task;
            expired.before_ms = 0;
            assert!(matches!(expired.check_setup_current().await, Err(PrivateOriginalError::Denied)));
        }).catch_unwind().await;
        let cleanup = fixture.shutdown().await;
        assert!(cleanup.is_ok(), "{cleanup:?}");
        if let Err(panic) = checked { std::panic::resume_unwind(panic); }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn native_stock_human_source_remains_eligible_without_a_new_operator_review() {
        source_eligibility(ContinuationFixture::for_service_input().await, meerkat_core::PrincipalKind::Human).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn native_service_source_prepares_the_same_canonical_task_without_access_permission() {
        source_eligibility(ContinuationFixture::for_continuation().await, meerkat_core::PrincipalKind::ServiceAccount).await;
    }

    async fn late_followup_currentness(error: HostLifetimeError, expected: PrivateOriginalError) {
        let fixture = ContinuationFixture::for_service_input().await;
        let checked = std::panic::AssertUnwindSafe(async {
            let admitted = fixture.submit().await;
            fixture.finish(&admitted).await.unwrap();
            let dispatch = fixture.probe.as_ref().unwrap().captured();
            let snapshot = super::super::admission_tests::acquisition(&fixture, &dispatch).await;
            let member = snapshot.request().host().continuation.native_original.as_ref().unwrap()
                .member.as_ref().unwrap().member.clone();
            let task = fixture.source.prepare_service_task(fixture.store.clone(), snapshot,
                "late-policy-followup", &member).await.unwrap().unwrap();
            task.check_setup_current().await.unwrap();
            let fresh = task.fresh.clone();
            let before = fixture.store.list_records(&fixture.scope, None, 256).await.unwrap();
            let observed = std::sync::atomic::AtomicBool::new(false);
            // This is a rejection control: unresolved grant coordinates are
            // never read or bound, because policy refuses before publication.
            // It does not claim a successful application-policy preparation.
            let grant = PrivateServiceGrantRef { route: task.parent.route.clone(),
                decision: Id::new(fresh.as_str()).unwrap(), decision_sha256: "0".repeat(64) };
            let evidence = PrivateSourceEvidence { resource_id: fresh, revision: EvidenceId::new("late-policy").unwrap(),
                sha256: "1".repeat(64) };
            let failure = policy_error(error);
            let result = task.retain_approved(grant, evidence, &|| {
                // retain_approved calls this only after reopening the actual
                // parent and acquisition through its awaited current() read.
                observed.store(true, std::sync::atomic::Ordering::SeqCst);
                Err(failure)
            }).await;
            assert!(matches!(result, Err(actual) if actual == expected));
            assert!(observed.load(std::sync::atomic::Ordering::SeqCst));
            assert_eq!(fixture.store.list_records(&fixture.scope, None, 256).await.unwrap(), before);
            assert_eq!(fixture.inputs().await, Ok(1));
        }).catch_unwind().await;
        let cleanup = fixture.shutdown().await;
        assert!(cleanup.is_ok(), "{cleanup:?}");
        if let Err(payload) = checked { std::panic::resume_unwind(payload); }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unavailable_after_followup_parent_read_stays_unavailable() {
        late_followup_currentness(HostLifetimeError::Unavailable, PrivateOriginalError::Unavailable).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unsettled_after_followup_parent_read_stays_unavailable() {
        late_followup_currentness(HostLifetimeError::NotSettled, PrivateOriginalError::Unavailable).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn denied_after_followup_parent_read_stays_denied() {
        late_followup_currentness(HostLifetimeError::WrongOwner, PrivateOriginalError::Denied).await;
    }

    #[tokio::test]
    async fn required_console_login_rejects_missing_and_expired_mechanical_custody() {
        use crate::private_host::{ConsoleLoginRefusal, ConsoleLoginSelection, ConsoleLogins};
        // This exercises the mandatory login guard, not a fabricated native
        // Human or the Console binder's authentication of one.
        assert!(matches!(current_console_login(None), Err(PrivateOriginalError::Unavailable)));
        let (_connection, logins) = ConsoleLogins::fixture_owner();
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
        let login = logins.fixture_enroll(1, ConsoleLoginSelection {
            issuer: "https://application-policy.fixture.invalid".into(), subject: "mechanical-login".into(),
            login_scope_id: "same-retained-source".into(), expires_at_ms: now + 250,
        }).unwrap();
        assert!(current_console_login(Some(login.clone())).unwrap().same_owner(&login));
        let closed = tokio::time::timeout(std::time::Duration::from_secs(2), login.closed()).await.unwrap();
        assert_eq!(closed, ConsoleLoginRefusal::Expired);
        assert!(matches!(current_console_login(Some(login)), Err(PrivateOriginalError::Denied)));
    }
}
