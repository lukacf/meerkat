//! The authenticated application's access decision to native issuance bridge.
//!
//! This is the application-host path, not stock rkat realm administration.
//! The private parent reports an actual service/account access decision, never
//! a package-install approval. Its real installed route supplies native principals
//! and audience. The row below is immutable decision/binding evidence; native
//! LocalGrantAuthority remains the sole current grant owner.

use meerkat_authorization_contracts::constraints::DomainRestriction;
use super::*;
use meerkat_authorization::grants::GrantRefusal;
use meerkat_authorization_contracts::{
    constraints::{
        ActionRef, DelegationDepth, ExactRestriction, ExecutionRestrictions, LifetimeRestriction,
        ProcessorRef,
    },
    grant::GrantLineageRef,
    work_association::OriginalWorkRef,
};
use meerkat_toolkit::authority::{LifetimeScope, ServicePermission};
use meerkat_toolkit::host_lifetime::{NativeHostLifetime, control::HostLifetimeError};
use toolkit_package::Revision;
use toolkit_store::AdmissionOutcome;

const SCHEMA: &str = "toolkit.authenticated-service-access-decision";

pub(crate) mod console;
pub(crate) mod recovery;
mod mcp;
pub(crate) mod catalog;
mod batch;
pub use mcp::{PrivateServiceMcpAccessDecision, PrivateServiceMcpPermission, PrivateServiceMcpLiveAttachment, PrivateServiceMcpCatalog, ServiceMcpSelection};
pub(crate) use mcp::RetainedServiceMcpAccess;
use mcp::StoredServiceMcpSelection;
use super::service_approved_input::application::{ApplicationServiceEvidence, ApplicationServiceDecision,
    ServiceOperationCurrent, ServiceOperationError, ServiceOperationProgress};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrivateAccessChoice {
    Approve,
    Deny,
}

/// Output of the existing authenticated application access UI/policy owner.
/// This command is unavailable on public RPC. It carries no native principal,
/// grant, lineage, runtime identity or preconstructed ExecutionRestrictions.
/// A package/config approval must not be translated into this decision.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrivateServiceAccessDecision {
    pub route: Id,
    pub decision: Id,
    pub evidence: PrivateSourceEvidence,
    pub choice: PrivateAccessChoice,
    pub permission: ServicePermission,
    pub connection_revision: Revision,
}

/// Historical decision coordinates. Possession of this value grants nothing.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrivateServiceGrantRef {
    pub route: Id,
    pub decision: Id,
    pub decision_sha256: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrivateServiceGrantState {
    ApprovalDenied,
    GrantIssued,
    GrantUnavailable,
    PendingReconciliation,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrivateServiceGrantObservation {
    pub grant: PrivateServiceGrantRef,
    pub state: PrivateServiceGrantState,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
enum IssuanceObservation {
    PendingNativeIssuance,
    ApprovalDenied,
    NativeRefused,
    Issued { lineage: Vec<GrantLineageRef> },
}

// Keep a typed application currentness failure distinct from native issuance.
// This is a worker result only; the existing durable attempt remains the owner.
enum ServiceIssueError {
    Native(GrantRefusal),
    Current(ServiceOperationError),
}
impl From<GrantRefusal> for ServiceIssueError {
    fn from(error: GrantRefusal) -> Self { Self::Native(error) }
}
fn observe_progress(record: &AccessRecord, progress: Option<&ServiceOperationProgress>) {
    if let Some(progress) = progress {
        match record.issuance {
            IssuanceObservation::Issued { .. } => progress.effect(),
            IssuanceObservation::PendingNativeIssuance => progress.uncertain(),
            IssuanceObservation::NativeRefused => progress.native_refused(),
            IssuanceObservation::ApprovalDenied => {},
        }
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum AccessDecision {
    Legacy(PrivateServiceAccessDecision),
    Mcp(PrivateServiceMcpAccessDecision),
    Catalog(catalog::ServiceCatalogDecision),
}
impl AccessDecision {
    pub(crate) fn route(&self) -> &Id { match self { Self::Legacy(d) => &d.route, Self::Mcp(d) => &d.route, Self::Catalog(d) => &d.route } }
    fn id(&self) -> &Id { match self { Self::Legacy(d) => &d.decision, Self::Mcp(d) => &d.decision, Self::Catalog(d) => &d.decision } }
    fn choice(&self) -> PrivateAccessChoice { match self { Self::Legacy(d) => d.choice, Self::Mcp(d) => d.choice, Self::Catalog(_) => PrivateAccessChoice::Approve } }
    fn lifetime(&self) -> &meerkat_toolkit::authority::RoleLifetime {
        match self { Self::Legacy(d) => &d.permission.lifetime, Self::Mcp(d) => &d.permission.lifetime, Self::Catalog(d) => &d.lifetime }
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AccessRecord {
    pub(super) decision: AccessDecision,
    /// Selected before issuance, never replaced by a later policy on replay.
    caller: PrincipalRef,
    pub(super) parent: Vec<GrantLineageRef>,
    issuance: IssuanceObservation,
    // Version 2 has no native connector correspondence. Preserve its bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) mcp: Option<StoredServiceMcpSelection>,
    /// Version 4 records the actual application policy decision beside the
    /// exact native issuance attempt. It is evidence, never reusable authority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    application_policy: Option<ApplicationServiceEvidence>,
}

/// Legacy version 2 access vocabulary. Its retained account/resource and
/// connection revision encoding is preserved for historical replay. It does
/// not establish native connector correspondence. Version 3 uses the same
/// decision validation with native operation and connector restrictions.
fn restrictions(
    route: &PrivateOriginalRoute,
    decision: &PrivateServiceAccessDecision,
    now_ms: u64,
) -> Result<ExecutionRestrictions, PrivateOriginalError> {
    let permission = &decision.permission;
    if permission.target != route.grant_target
        || !permission.lifetime.active_at(now_ms)
        || !valid_digest(&decision.evidence.sha256)
    {
        return Err(PrivateOriginalError::Denied);
    }
    match &permission.lifetime.scope {
        LifetimeScope::Member { member } if member != &route.grant_target => {
            return Err(PrivateOriginalError::Denied);
        }
        _ => {}
    }
    let account = canonical_json(&(
        &permission.connection,
        &decision.connection_revision,
        &permission.resource,
    ))
    .map_err(|_| PrivateOriginalError::Denied)?;
    Ok(ExecutionRestrictions {
        actions: ExactRestriction::exact([ActionRef {
            feature: "mcp".into(),
            action: permission.operation.to_string(),
        }]),
        resource_domains: DomainRestriction::exact([ResourceDomain {
            authority: route.grant_caller.clone(),
            namespace: format!(
                "toolkit-mcp-account/{}",
                String::from_utf8(account).map_err(|_| PrivateOriginalError::Denied)?
            ),
        }]),
        processors: ExactRestriction::exact([ProcessorRef::Principal {
            principal: route.logical_executor.clone(),
        }]),
        audiences: ExactRestriction::exact([route.grant_audience.clone()]),
        lifetime: match permission.lifetime.expires_at_ms {
            Some(expiry) => LifetimeRestriction::window(0, expiry),
            None => LifetimeRestriction::unrestricted(),
        },
        // The app issues directly to the agent. Task access is already this
        // original-specific leaf; no agent-to-agent child is added later.
        delegation_depth: DelegationDepth::remaining(0),
    })
}

impl PrivateOriginals {
    pub(super) fn service_mcp_reference(decision: &PrivateServiceMcpAccessDecision,
        selection: &ServiceMcpSelection) -> Result<PrivateServiceGrantRef, PrivateOriginalError>
    {
        Ok(PrivateServiceGrantRef { route: decision.route.clone(), decision: decision.decision.clone(),
            decision_sha256: decision_digest(&AccessDecision::Mcp(decision.clone()), Some(&selection.0))? })
    }

    /// Only an access intent bound to this exact approved parent belongs to
    /// its compensation. Another approval for the same task may share the
    /// decision ID; that earlier approval's child must remain untouched.
    pub(super) async fn service_compensation_child(&self, reference: &PrivateServiceGrantRef,
        parent: &GrantLineageRef) -> Result<Option<(PrincipalRef, GrantLineageRef)>, PrivateOriginalError>
    {
        let key = self.access_key(reference)?;
        let child_id = EvidenceId::new(format!("access-{}", key.id)).map_err(|_| PrivateOriginalError::Conflict)?;
        let Some(stored) = self.store.get(&key).await.map_err(store_error)? else {
            // Issuance requires this intent first. A missing intent alongside
            // native issuance is inconsistent, never proof that cleanup ended.
            return match self.owner.grants().issued_reference(&child_id) {
                Ok(None) => Ok(None),
                _ => Err(PrivateOriginalError::Unavailable),
            };
        };
        let decoded: AccessRecord = serde_json::from_value(stored.data.body.clone())
            .map_err(|_| PrivateOriginalError::Conflict)?;
        let actual_ref = PrivateServiceGrantRef { route: decoded.decision.route().clone(),
            decision: decoded.decision.id().clone(),
            decision_sha256: decision_digest(&decoded.decision, decoded.mcp.as_ref())? };
        let record = self.decode_access(&stored, &actual_ref)?;
        if record.parent.as_slice() != [parent.clone()] { return Ok(None); }
        if &actual_ref != reference || record.application_policy.is_some()
            || record.caller != self.route(&reference.route)?.grant_caller {
            return Err(PrivateOriginalError::Conflict);
        }
        let leaf = self.owner.grants().issued_reference(&child_id)
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        match (&record.issuance, &leaf) {
            (IssuanceObservation::PendingNativeIssuance, _) => {},
            (IssuanceObservation::Issued { lineage }, Some(leaf)) if lineage.last() == Some(leaf) => {},
            (IssuanceObservation::NativeRefused | IssuanceObservation::ApprovalDenied, None) => {},
            _ => return Err(PrivateOriginalError::Unavailable),
        }
        Ok(leaf.map(|leaf| (record.caller, leaf)))
    }

    pub(super) async fn require_service_grant_issued(&self, reference: &PrivateServiceGrantRef)
        -> Result<(), PrivateOriginalError>
    {
        let stored = self.store.get(&self.access_key(reference)?).await.map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        let record = self.decode_access(&stored, reference)?;
        if self.access_observation(reference.clone(), &record)?.state != PrivateServiceGrantState::GrantIssued {
            return Err(PrivateOriginalError::Unavailable);
        }
        Ok(())
    }

    pub(super) fn access_key(
        &self,
        reference: &PrivateServiceGrantRef,
    ) -> Result<StoreKey, PrivateOriginalError> {
        self.route(&reference.route)?;
        Ok(StoreKey {
            realm: self.scope.realm.clone(),
            owner: self.scope.owner.clone(),
            id: digest(
                &canonical_json(&(SCHEMA, &reference.route, &reference.decision))
                    .map_err(|_| PrivateOriginalError::Denied)?,
            ),
        })
    }

    pub(super) fn decode_access(
        &self,
        stored: &StoredRecord,
        reference: &PrivateServiceGrantRef,
    ) -> Result<AccessRecord, PrivateOriginalError> {
        if stored.key != self.access_key(reference)?
            || stored.data.schema != SCHEMA
            || !matches!(stored.data.version, 2 | 3 | 4 | 5 | 6)
            || stored.data.digest().ok().as_deref() != Some(stored.content_digest.as_str())
        {
            return Err(PrivateOriginalError::Conflict);
        }
        let record: AccessRecord = serde_json::from_value(stored.data.body.clone())
            .map_err(|_| PrivateOriginalError::Conflict)?;
        if !matches!((&record.decision, &record.mcp, &record.application_policy, stored.data.version),
                (AccessDecision::Legacy(_), None, None, 2) | (AccessDecision::Mcp(_), Some(_), None, 3)
                | (AccessDecision::Mcp(_), Some(_), Some(_), 4 | 5)
                | (AccessDecision::Catalog(_), None, None, 6))
            || (record.application_policy.as_ref().is_some_and(|evidence| evidence.service_parent.is_some())
                != (stored.data.version == 5))
            || canonical_json(&record).map_err(|_| PrivateOriginalError::Conflict)?
            != canonical_json(&stored.data.body).map_err(|_| PrivateOriginalError::Conflict)?
            || record.caller.validate_qualified().is_err()
            || record.parent.is_empty()
            || record.parent.len()
                >= meerkat_authorization_contracts::work_association::MAX_ASSOCIATION_REFERENCES
            || record.decision.route() != &reference.route
            || record.decision.id() != &reference.decision
            || decision_digest(&record.decision, record.mcp.as_ref())?
                != reference.decision_sha256
        {
            return Err(PrivateOriginalError::Conflict);
        }
        if let (Some(mcp), AccessDecision::Mcp(decision)) = (&record.mcp, &record.decision) {
            mcp.validate(self.route(&reference.route)?, decision)?;
            if let Some(evidence) = &record.application_policy {
                evidence.validate(self.route(&reference.route)?, decision, &self.scope.owner)?;
            }
        }
        if let IssuanceObservation::Issued { lineage } = &record.issuance {
            if record.decision.choice() != PrivateAccessChoice::Approve
                || record.parent.is_empty()
                || lineage.len() != record.parent.len() + 1
                || lineage[..record.parent.len()] != record.parent
                || lineage.last().is_none_or(|leaf| {
                    leaf.grant_id.as_str() != format!("access-{}", stored.key.id)
                })
            {
                return Err(PrivateOriginalError::Conflict);
            }
        }
        Ok(record)
    }

    /// Persist the exact decision before native issuance. Same-key retries
    /// observe saved evidence and current native state; they never issue again.
    /// If the process loses the native result before saving its lineage, this
    /// decision remains pending reconciliation. No native journal is parsed and
    /// no second grant is silently minted for an uncertain first issuance.
    pub(crate) async fn apply_service_access_decision(
        &self,
        decision: PrivateServiceAccessDecision,
    ) -> Result<PrivateServiceGrantObservation, PrivateOriginalError> {
        self.apply_service_access(AccessDecision::Legacy(decision), None, None, None, None, Arc::new(|| Ok(()))).await
    }

    pub(crate) async fn apply_mcp_service_access_decision(
        &self,
        approved: crate::installed_host::operator_setup::ReviewedServiceMcpParent,
    ) -> Result<PrivateServiceGrantObservation, PrivateOriginalError> {
        self.apply_mcp_service_access_decision_current(approved, Arc::new(|| true)).await
    }

    pub(crate) async fn apply_mcp_service_access_decision_current(
        &self, approved: crate::installed_host::operator_setup::ReviewedServiceMcpParent,
        presented: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Result<PrivateServiceGrantObservation, PrivateOriginalError> {
        let (decision, selection, parent, lifetime) = approved.into_parts(&self.owner)?;
        self.apply_service_access(AccessDecision::Mcp(decision), Some(selection.0), Some(parent), Some(lifetime), None,
            Arc::new(move || if presented() { Ok(()) } else { Err(PrivateOriginalError::Denied) })).await
    }

    pub(in crate::private_originals) async fn apply_application_service_access(
        &self, approved: ApplicationServiceDecision,
        current: Arc<dyn Fn() -> Result<(), PrivateOriginalError> + Send + Sync>,
    ) -> Result<PrivateServiceGrantObservation, PrivateOriginalError> {
        self.apply_application_service_access_owned(approved, Arc::new(move || current().map_err(Into::into)), None)
            .await.map_err(ServiceOperationError::source)
    }

    pub(in crate::private_originals) async fn apply_application_service_access_owned(
        &self, approved: ApplicationServiceDecision, current: ServiceOperationCurrent,
        progress: Option<Arc<ServiceOperationProgress>>,
    ) -> Result<PrivateServiceGrantObservation, ServiceOperationError> {
        let (decision, selection, parent, lifetime, evidence) = approved.into_parts(self)?;
        current()?;
        self.apply_service_access_owned(AccessDecision::Mcp(decision), Some(selection.0), Some(parent),
            Some(lifetime), Some(evidence), current, progress).await
    }

    async fn apply_service_access(
        &self, decision: AccessDecision, mcp: Option<StoredServiceMcpSelection>,
        reviewed_parent: Option<Vec<GrantLineageRef>>, mcp_lifetime: Option<std::sync::Weak<NativeHostLifetime>>,
        application_policy: Option<ApplicationServiceEvidence>,
        presented: Arc<dyn Fn() -> Result<(), PrivateOriginalError> + Send + Sync>,
    ) -> Result<PrivateServiceGrantObservation, PrivateOriginalError> {
        self.apply_service_access_owned(decision, mcp, reviewed_parent, mcp_lifetime,
            application_policy, Arc::new(move || presented().map_err(Into::into)), None)
            .await.map_err(ServiceOperationError::source)
    }

    async fn apply_service_access_owned(
        &self,
        decision: AccessDecision,
        mcp: Option<StoredServiceMcpSelection>,
        reviewed_parent: Option<Vec<GrantLineageRef>>,
        mcp_lifetime: Option<std::sync::Weak<NativeHostLifetime>>,
        application_policy: Option<ApplicationServiceEvidence>,
        presented: ServiceOperationCurrent,
        progress: Option<Arc<ServiceOperationProgress>>,
    ) -> Result<PrivateServiceGrantObservation, ServiceOperationError> {
        let route = self.route(decision.route())?;
        if let Some(evidence) = &application_policy {
            let AccessDecision::Mcp(ref proposal) = decision else { return Err(PrivateOriginalError::Denied.into()); };
            evidence.validate(route, proposal, &self.scope.owner)?;
            if evidence.service_parent.is_some() {
                let lifetime = mcp_lifetime.as_ref().and_then(Weak::upgrade).ok_or(PrivateOriginalError::Unavailable)?;
                evidence.check_parent(&lifetime, route, reviewed_parent.as_deref().ok_or(PrivateOriginalError::Denied)?)?;
            } else if reviewed_parent.as_ref() != route.application_parent.as_ref() {
                return Err(PrivateOriginalError::Denied.into());
            }
        }
        match (&decision, &mcp, &reviewed_parent, &mcp_lifetime) {
            (AccessDecision::Legacy(_), None, None, None) => {}
            (AccessDecision::Mcp(decision), Some(mcp), Some(_), Some(_)) => mcp.validate(route, decision)?,
            (AccessDecision::Catalog(decision), None, Some(_), Some(_)) => { decision.restrictions(route, 0)?; }
            _ => return Err(PrivateOriginalError::Denied.into()),
        }
        if matches!(&decision.lifetime().scope, LifetimeScope::Deployment { deployment }
            if deployment.as_str() != self.scope.owner)
        {
            return Err(PrivateOriginalError::Denied.into());
        }
        let reference = PrivateServiceGrantRef {
            route: decision.route().clone(),
            decision: decision.id().clone(),
            decision_sha256: decision_digest(&decision, mcp.as_ref())?,
        };
        let key = self.access_key(&reference)?;
        // Exact saved decisions remain observable after their proposed access
        // expires. Observation re-resolves issued grants but never reissues.
        if let Some(stored) = self.store.get(&key).await.map_err(store_error)? {
            let record = self.decode_access(&stored, &reference)?;
            if reviewed_parent.as_ref().is_some_and(|parent| parent != &record.parent) {
                return Err(PrivateOriginalError::Conflict.into());
            }
            if record.application_policy != application_policy { return Err(PrivateOriginalError::Conflict.into()); }
            observe_progress(&record, progress.as_deref());
            if progress.is_some() { presented()?; }
            return self.access_observation(reference, &record).map_err(Into::into);
        }
        let now_ms = self
            .owner
            .clock()
            .now()
            .map_err(|_| PrivateOriginalError::Unavailable)?
            .unix_ms;
        let bounds = access_restrictions(route, &decision, mcp.as_ref(), now_ms)?;
        let parent = reviewed_parent.or_else(|| route.application_parent.clone())
            .ok_or(PrivateOriginalError::Unavailable)?;
        if parent.is_empty() {
            return Err(PrivateOriginalError::Unavailable.into());
        }
        if progress.is_some() { presented()?; }
        let initial = AccessRecord {
            caller: route.grant_caller.clone(),
            parent,
            issuance: match decision.choice() {
                PrivateAccessChoice::Approve => IssuanceObservation::PendingNativeIssuance,
                PrivateAccessChoice::Deny => IssuanceObservation::ApprovalDenied,
            },
            decision,
            mcp,
            application_policy,
        };
        let admitted = self
            .store
            .admit(AdmissionRequest {
                key: key.clone(),
                admission_key: key.id.clone(),
                data: encode_access(&initial)?,
                events: vec![],
                effects: vec![],
            })
            .await
            .map_err(store_error)?;
        let stored = admitted.record();
        let mut record = self.decode_access(stored, &reference)?;
        if ((initial.mcp.is_some() || matches!(initial.decision, AccessDecision::Catalog(_))) && record.parent != initial.parent)
            || record.application_policy != initial.application_policy {
            return Err(PrivateOriginalError::Conflict.into());
        }
        if !matches!(&admitted, AdmissionOutcome::Created(_))
            || record.decision.choice() == PrivateAccessChoice::Deny
        {
            observe_progress(&record, progress.as_deref());
            if progress.is_some() { presented()?; }
            return self.access_observation(reference, &record).map_err(Into::into);
        }
        let grants = self.owner.grants();
        let caller = record.caller.clone();
        let parent = record.parent.clone();
        let grantee = route.logical_executor.clone();
        let represented = route.represented_subject.clone();
        let mcp_check = match (&record.mcp, &record.decision) {
            (Some(selection), AccessDecision::Mcp(decision)) => Some((selection.clone(), route.clone(), decision.clone())),
            _ => None,
        };
        let grant_id = EvidenceId::new(format!("access-{}", key.id))
            .map_err(|_| PrivateOriginalError::Denied)?;
        // The private command owner retains this task through reply or channel
        // close. Native durable issuance may block, so it uses its own worker.
        let worker_progress = progress.clone();
        let owned_operation = progress.is_some();
        let issued = tokio::task::spawn_blocking(move || {
            let issue = || -> Result<GrantLineageRef, ServiceIssueError> {
                // Re-resolve the complete selected lineage inside the actual
                // host-open mutation. Native issuance repeats currentness,
                // grantee/caller, depth and attenuation checks.
                let resolved = grants.resolve_lineage(&parent, &caller, represented.as_ref())?;
                resolved.check_current().map_err(|_| GrantRefusal::Unavailable)?;
                if let Some((selection, route, decision)) = &mcp_check {
                    selection.check_grant(route, decision, &resolved, now_ms)?;
                }
                let leaf = parent.last().ok_or(GrantRefusal::Denied)?;
                // Recheck the exact accepted action in this worker after the
                // store and scheduler waits, immediately before native entry.
                presented().map_err(ServiceIssueError::Current)?;
                if let Some(progress) = &worker_progress { progress.entering(); }
                grants.issue_child(&caller, leaf, grant_id, grantee, bounds).map_err(ServiceIssueError::Native)
            };
            match mcp_lifetime {
                Some(lifetime) => {
                    let lifetime = lifetime.upgrade().ok_or(GrantRefusal::Unavailable)?;
                    // No fence spans store awaits. Close either wins here, or
                    // waits for this entered synchronous native mutation.
                    lifetime.with_current_setup(|| Ok(issue()))
                        .map_err(|error| if owned_operation { ServiceIssueError::Current(error.into()) }
                            else { ServiceIssueError::Native(match error {
                                HostLifetimeError::Unavailable | HostLifetimeError::NotSettled => GrantRefusal::Unavailable,
                                _ => GrantRefusal::Denied,
                            }) })?
                }
                None => issue(), // Unchanged legacy access path.
            }
        })
        .await;
        let mut current_refusal = None;
        record.issuance = match issued {
            Ok(Ok(reference)) => {
                if let Some(progress) = &progress { progress.effect(); }
                let mut lineage = record.parent.clone();
                lineage.push(reference);
                IssuanceObservation::Issued { lineage }
            }
            Ok(Err(ServiceIssueError::Native(GrantRefusal::Denied))) => {
                if let Some(progress) = &progress { progress.native_refused(); }
                IssuanceObservation::NativeRefused
            },
            Ok(Err(ServiceIssueError::Current(error @ (ServiceOperationError::Source(PrivateOriginalError::Denied)
                | ServiceOperationError::Authority(HostLifetimeError::Denied | HostLifetimeError::WrongOwner | HostLifetimeError::InvalidSelection)
                | ServiceOperationError::Cancelled)))) => {
                current_refusal = Some(error);
                IssuanceObservation::NativeRefused
            }
            // The exact attempt is already Pending. Unknown currentness is
            // not a definitive native refusal and never permits reissuance.
            Ok(Err(ServiceIssueError::Current(error))) => {
                if let Some(progress) = &progress { progress.uncertain(); }
                return Err(error);
            },
            _ => {
                if let Some(progress) = &progress { progress.uncertain(); }
                return Ok(PrivateServiceGrantObservation {
                    grant: reference,
                    state: PrivateServiceGrantState::PendingReconciliation,
                });
            }
        };
        if self
            .store
            .compare_and_swap(Transition {
                key,
                expected_revision: stored.revision,
                expected_digest: Some(stored.content_digest.clone()),
                claim: None,
                data: encode_access(&record)?,
                events: vec![],
                effects: vec![],
            })
            .await
            .is_err()
        {
            // Even an uncertain store response is not grounds to issue again.
            // An exact retry reads the saved row and may recover its lineage.
            if let Some(progress) = &progress { progress.uncertain(); }
            return Ok(PrivateServiceGrantObservation {
                grant: reference,
                state: PrivateServiceGrantState::PendingReconciliation,
            });
        }
        if let Some(error) = current_refusal { return Err(error); }
        self.access_observation(reference, &record).map_err(Into::into)
    }

    fn access_observation(
        &self,
        reference: PrivateServiceGrantRef,
        record: &AccessRecord,
    ) -> Result<PrivateServiceGrantObservation, PrivateOriginalError> {
        let route = self.route(&reference.route)?;
        let state = match &record.issuance {
            IssuanceObservation::PendingNativeIssuance => {
                PrivateServiceGrantState::PendingReconciliation
            }
            IssuanceObservation::ApprovalDenied => PrivateServiceGrantState::ApprovalDenied,
            IssuanceObservation::NativeRefused => PrivateServiceGrantState::GrantUnavailable,
            IssuanceObservation::Issued { lineage } => match self.owner.grants().resolve_lineage(
                lineage,
                &route.logical_executor,
                route.represented_subject.as_ref(),
            ) {
                Ok(resolved) if resolved.check_current().is_ok() => {
                    let now_ms = self
                        .owner
                        .clock()
                        .now()
                        .map_err(|_| PrivateOriginalError::Unavailable)?
                        .unix_ms;
                    let expected = (|| {
                        if route.grant_caller != record.caller
                            || (matches!(&record.decision, AccessDecision::Legacy(_))
                                && route.application_parent.as_ref() != Some(&record.parent))
                        {
                            return Err(PrivateOriginalError::Unavailable);
                        }
                        if let Some(evidence) = &record.application_policy {
                            let lifetime = self.host_lifetime.get().and_then(Weak::upgrade)
                                .ok_or(PrivateOriginalError::Unavailable)?;
                            lifetime.validate_current_selection().map_err(|_| PrivateOriginalError::Unavailable)?;
                            let selected = lifetime.selected_policy();
                            evidence.check_parent(&lifetime, route, &record.parent)?;
                            if evidence.deployment != selected.deployment
                                || evidence.source_policy_sha256 != selected.source_policy_sha256
                                || selected.native_host.as_ref().is_none_or(|native| native.grant_administrator != evidence.owner)
                            { return Err(PrivateOriginalError::Unavailable); }
                        }
                        if let (Some(selection), AccessDecision::Mcp(decision)) = (&record.mcp, &record.decision) {
                            selection.check_grant(route, decision, &resolved, now_ms)
                                .map_err(|_| PrivateOriginalError::Unavailable)?;
                        }
                        let parent = self
                            .owner
                            .grants()
                            .resolve_lineage(
                                &record.parent,
                                &record.caller,
                                route.represented_subject.as_ref(),
                            )
                            .map_err(|_| PrivateOriginalError::Unavailable)?;
                        parent
                            .check_current()
                            .map_err(|_| PrivateOriginalError::Unavailable)?;
                        parent
                            .restrictions()
                            .for_child(&access_restrictions(route, &record.decision, record.mcp.as_ref(), now_ms)?)
                            .map_err(|_| PrivateOriginalError::Unavailable)
                    })();
                    match expected {
                        Ok(expected) if &expected == resolved.restrictions() => {
                            PrivateServiceGrantState::GrantIssued
                        }
                        _ => PrivateServiceGrantState::GrantUnavailable,
                    }
                }
                _ => PrivateServiceGrantState::GrantUnavailable,
            },
        };
        Ok(PrivateServiceGrantObservation {
            grant: reference,
            state,
        })
    }

    /// Retain the exact app-selected permission and immutable source binding.
    /// Task uniqueness comes from the retained task decision; standing access
    /// may be shared without acquiring task-revoke authority. This source-only
    /// step is not native admission or proof of the pending native operation fence.
    pub(crate) async fn bind_access_source(
        &self,
        source: &PrivateOriginalRef,
        grant: &PrivateServiceGrantRef,
        association: meerkat_authorization_contracts::work_association::InputAuthorityAssociation,
        custody: meerkat_runtime::meerkat_machine::AuthenticatedOriginal,
    ) -> Result<Arc<RetainedToolkitOriginal>, PrivateOriginalError> {
        let stored = self
            .store
            .get(&self.access_key(grant)?)
            .await
            .map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        let decision = self.decode_access(&stored, grant)?;
        let original = &association.candidate().original_work;
        if source.route != grant.route
            || source.original != original.work
            || matches!(&decision.decision.lifetime().scope, LifetimeScope::Task { task } if task.as_str() != original.work.as_str())
        {
            return Err(PrivateOriginalError::Denied);
        }
        let lineage = self.service_grant_lineage(grant, original).await?;
        if association.candidate().authority_basis != (meerkat_authorization_contracts::work_association::WorkAuthorityBasis::GrantLineage { lineage }) {
            return Err(PrivateOriginalError::Conflict);
        }
        let route = self.route(&source.route)?;
        let task_issuer = matches!(
            &decision.decision.lifetime().scope,
            LifetimeScope::Task { .. }
        )
        .then_some(&decision.caller);
        let retained = self
            .owner
            .bind_issued_access_async(
                association,
                custody,
                &route.requester,
                &route.ingress_actor,
                task_issuer,
                |association| async move {
                    self.persist_or_observe_binding(source, &association, true)
                        .await
                },
            )
            .await
            .map_err(|error| match error {
                OriginalWriterError::OperationCancelled => PrivateOriginalError::Denied,
                OriginalWriterError::Conflict => PrivateOriginalError::Conflict,
                _ => PrivateOriginalError::Unavailable,
            })?;
        let mcp = match (decision.decision, decision.mcp) {
            (AccessDecision::Mcp(decision), Some(selection)) => Some(RetainedServiceMcpAccess { decision, selection }),
            (AccessDecision::Legacy(_), None) => None,
            _ => return Err(PrivateOriginalError::Conflict),
        };
        self.hold_binding_with_mcp(source.clone(), retained.clone(), mcp.into_iter().collect()).await?;
        Ok(retained)
    }

    /// Historical B decision join only. No current permission, fresh issuance
    /// or task-revoke issuer is recovered from this recorded lineage.
    pub(super) async fn historical_task_access(
        &self, source: &PrivateOriginalRef, grant: &PrivateServiceGrantRef,
        association: &meerkat_authorization_contracts::work_association::InputAuthorityAssociation,
    ) -> Result<RetainedServiceMcpAccess, PrivateOriginalError> {
        let stored = self.store.get(&self.access_key(grant)?).await.map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        let record = self.decode_access(&stored, grant)?;
        let claims = association.candidate();
        if source.route != grant.route || source.original != claims.original_work.work
            || !matches!(&record.decision.lifetime().scope,
                LifetimeScope::Task { task } if task.as_str() == source.original.as_str())
        { return Err(PrivateOriginalError::Conflict); }
        let IssuanceObservation::Issued { lineage } = record.issuance else {
            return Err(PrivateOriginalError::Unavailable);
        };
        if claims.authority_basis != (meerkat_authorization_contracts::work_association::WorkAuthorityBasis::GrantLineage { lineage }) {
            return Err(PrivateOriginalError::Conflict);
        }
        match (record.decision, record.mcp) {
            (AccessDecision::Mcp(decision), Some(selection)) => Ok(RetainedServiceMcpAccess { decision, selection }),
            _ => Err(PrivateOriginalError::Unavailable),
        }
    }

    /// Current owner-internal binding input. Persisted lineage is re-resolved
    /// against native state before use; decoded evidence never authorizes work.
    /// No original is needed to issue a persistent member role. This later source
    /// binding step selects its actual original independently. A task role
    /// cannot be used as another original's role, even when expiry is absent.
    pub(crate) async fn service_grant_lineage(
        &self,
        reference: &PrivateServiceGrantRef,
        original: &OriginalWorkRef,
    ) -> Result<Vec<GrantLineageRef>, PrivateOriginalError> {
        let stored = self
            .store
            .get(&self.access_key(reference)?)
            .await
            .map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        let record = self.decode_access(&stored, reference)?;
        let route = self.route(&reference.route)?;
        if original.authority != route.source_authority
            || matches!(&record.decision.lifetime().scope, LifetimeScope::Task { task }
                if task.as_str() != original.work.as_str())
        {
            return Err(PrivateOriginalError::Denied);
        }
        if self.access_observation(reference.clone(), &record)?.state
            != PrivateServiceGrantState::GrantIssued
        {
            return Err(PrivateOriginalError::Unavailable);
        }
        match record.issuance {
            IssuanceObservation::Issued { lineage } => Ok(lineage),
            _ => Err(PrivateOriginalError::Unavailable),
        }
    }
}

fn decision_digest(
    decision: &AccessDecision,
    mcp: Option<&StoredServiceMcpSelection>,
) -> Result<String, PrivateOriginalError> {
    // Existing generic decisions retain exactly their version 2 digest.
    let bytes = match mcp {
        None => canonical_json(decision),
        Some(selection) => canonical_json(&(decision, selection)),
    }.map_err(|_| PrivateOriginalError::Conflict)?;
    Ok(digest(&bytes))
}

fn access_restrictions(
    route: &PrivateOriginalRoute,
    decision: &AccessDecision,
    mcp: Option<&StoredServiceMcpSelection>,
    now_ms: u64,
) -> Result<ExecutionRestrictions, PrivateOriginalError> {
    match (decision, mcp) {
        (AccessDecision::Legacy(decision), None) => restrictions(route, decision, now_ms),
        (AccessDecision::Mcp(decision), Some(selection)) => selection.restrictions(route, decision, now_ms),
        (AccessDecision::Catalog(decision), None) => decision.restrictions(route, now_ms),
        _ => Err(PrivateOriginalError::Denied),
    }
}

fn encode_access(record: &AccessRecord) -> Result<RecordData, PrivateOriginalError> {
    Ok(RecordData {
        schema: SCHEMA.into(),
        version: if matches!(record.decision, AccessDecision::Catalog(_)) { 6 }
            else if record.application_policy.as_ref().is_some_and(|evidence| evidence.service_parent.is_some()) { 5 }
            else if record.application_policy.is_some() { 4 } else if record.mcp.is_some() { 3 } else { 2 },
        body: serde_json::to_value(record).map_err(|_| PrivateOriginalError::Denied)?,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use meerkat_authorization_contracts::constraints::{OperationRestrictionValues, SetBound};
    use meerkat_toolkit::authority::RoleLifetime;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicU64, Ordering};

    pub(crate) fn fixture_access_key(
        adapter: &PrivateOriginals,
        reference: &PrivateServiceGrantRef,
    ) -> Result<StoreKey, PrivateOriginalError> {
        adapter.access_key(reference)
    }

    struct Clock(AtomicU64);
    impl meerkat_authorization::clock::LocalAuthorizationClock for Clock {
        fn now(
            &self,
        ) -> Result<
            meerkat_authorization::clock::LocalAuthorizationTime,
            meerkat_authorization::clock::LocalClockError,
        > {
            Ok(meerkat_authorization::clock::LocalAuthorizationTime {
                unix_ms: self.0.load(Ordering::SeqCst),
                monotonic: meerkat_core::time_compat::Instant::now(),
            })
        }
    }

    pub(crate) fn decision() -> PrivateServiceAccessDecision {
        PrivateServiceAccessDecision {
            route: Id::new("route").unwrap(),
            decision: Id::new("approve-account-read").unwrap(),
            evidence: PrivateSourceEvidence {
                resource_id: EvidenceId::new("access-panel-decision").unwrap(),
                revision: EvidenceId::new("1").unwrap(),
                sha256: "a".repeat(64),
            },
            choice: PrivateAccessChoice::Approve,
            permission: ServicePermission {
                target: Id::new("agent").unwrap(),
                connection: Id::new("gmail-primary").unwrap(),
                operation: Id::new("read-messages").unwrap(),
                resource: Id::new("account-one").unwrap(),
                lifetime: RoleLifetime {
                    scope: LifetimeScope::Member {
                        member: Id::new("agent").unwrap(),
                    },
                    expires_at_ms: None,
                },
            },
            connection_revision: Revision::new("account-binding-1").unwrap(),
        }
    }
    fn directory() -> tempfile::TempDir {
        tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap()
    }
    fn app() -> PrincipalRef {
        PrincipalRef::in_domain(
            meerkat_core::PrincipalKind::ServiceAccount,
            "application",
            meerkat_core::TrustDomainId::new("toolkit-original-test").unwrap(),
        )
        .unwrap()
    }
    pub(crate) async fn adapter(
        owner: Arc<ToolkitOriginalOwner>,
        path: &std::path::Path,
    ) -> Arc<PrivateOriginals> {
        let mut adapter =
            crate::original_ingress::tests::private_originals(owner.clone(), path).await;
        let route = &mut Arc::get_mut(&mut adapter).unwrap().routes.get_mut().unwrap()[0];
        let parent = owner
            .grants()
            .issue_root(
                &route.grant_caller,
                EvidenceId::new("application-parent").unwrap(),
                app(),
                route.represented_subject.clone(),
                ExecutionRestrictions {
                    delegation_depth: DelegationDepth::remaining(1),
                    ..ExecutionRestrictions::unrestricted()
                },
            )
            .unwrap();
        route.grant_caller = app();
        route.application_parent = Some(vec![parent]);
        adapter
    }
    pub(crate) async fn restored_adapter(
        owner: Arc<ToolkitOriginalOwner>,
        path: &std::path::Path,
        parent: Vec<GrantLineageRef>,
    ) -> Arc<PrivateOriginals> {
        let mut adapter = crate::original_ingress::tests::private_originals(owner, path).await;
        let route = &mut Arc::get_mut(&mut adapter).unwrap().routes.get_mut().unwrap()[0];
        route.grant_caller = app();
        route.application_parent = Some(parent);
        adapter
    }
    fn original(adapter: &PrivateOriginals, work: &str) -> OriginalWorkRef {
        OriginalWorkRef {
            authority: adapter.routes.get().unwrap()[0].source_authority.clone(),
            work: EvidenceId::new(work).unwrap(),
        }
    }
    fn first<T: Ord>(restriction: &ExactRestriction<T>) -> &T {
        match restriction.bound() {
            SetBound::Exact(values) if values.len() == 1 => values.first().unwrap(),
            _ => panic!("expected one approved bound"),
        }
    }

    #[tokio::test]
    async fn approved_access_issues_native_bounds_and_replays_after_restart() {
        let directory = directory();
        let path = directory.path().join("grants");
        let owner = crate::original_ingress::tests::initialized(&path);
        let adapter = adapter(owner.clone(), &directory.path().join("originals")).await;
        let request = decision();
        let observed = adapter
            .apply_service_access_decision(request.clone())
            .await
            .unwrap();
        assert_eq!(observed.state, PrivateServiceGrantState::GrantIssued);
        let lineage = adapter
            .service_grant_lineage(&observed.grant, &original(&adapter, "work"))
            .await
            .unwrap();
        let resolved = owner
            .grants()
            .resolve_lineage(&lineage, &adapter.routes.get().unwrap()[0].logical_executor, None)
            .unwrap();
        let bounds = resolved.restrictions();
        let now_ms = owner.clock().now().unwrap().unix_ms;
        let values = OperationRestrictionValues {
            action: first(&bounds.actions),
            resource_domain: match bounds.resource_domains.bound() {
                meerkat_authorization_contracts::constraints::DomainBound::Exact(domains) => domains.first().unwrap(),
                _ => panic!("the selected service child must name an exact domain"),
            },
            processor: first(&bounds.processors),
            audience: first(&bounds.audiences),
            now_ms,
        };
        assert!(bounds.check_bounds(values).is_ok());
        let other_action = ActionRef {
            feature: "mcp".into(),
            action: "send-message".into(),
        };
        assert!(
            bounds
                .check_bounds(OperationRestrictionValues {
                    action: &other_action,
                    ..values
                })
                .is_err()
        );
        let mut other_resource = values.resource_domain.clone();
        other_resource.namespace = "other-account".into();
        assert!(
            bounds
                .check_bounds(OperationRestrictionValues {
                    resource_domain: &other_resource,
                    ..values
                })
                .is_err()
        );
        drop(resolved);
        let native_bytes = std::fs::read(&path).unwrap();
        assert_eq!(
            adapter
                .apply_service_access_decision(request.clone())
                .await
                .unwrap()
                .state,
            PrivateServiceGrantState::GrantIssued
        );
        assert_eq!(std::fs::read(&path).unwrap(), native_bytes);
        let parent = adapter.routes.get().unwrap()[0].application_parent.clone().unwrap();
        drop(adapter);
        drop(owner);
        let owner = Arc::new(
            ToolkitOriginalOwner::restore(
                Arc::new(meerkat_authorization::grants::FileGrantOwnerStore::new(
                    path.clone(),
                )),
                Arc::new(meerkat_authorization::clock::HostAuthorizationClock),
                // Same actual source domain retained by the existing fixture.
                ResourceDomain {
                    authority: PrincipalRef::in_domain(
                        meerkat_core::PrincipalKind::ServiceAccount,
                        "source",
                        meerkat_core::TrustDomainId::new("toolkit-original-test").unwrap(),
                    )
                    .unwrap(),
                    namespace: "originals".into(),
                },
            )
            .unwrap(),
        );
        let adapter =
            restored_adapter(owner.clone(), &directory.path().join("originals"), parent).await;
        let replay = adapter
            .apply_service_access_decision(request.clone())
            .await
            .unwrap();
        assert_eq!(replay.state, PrivateServiceGrantState::GrantIssued);
        assert!(
            adapter
                .service_grant_lineage(&replay.grant, &original(&adapter, "work"))
                .await
                .unwrap()
                == lineage
        );
        let mut changed = request;
        changed.permission.resource = Id::new("account-two").unwrap();
        assert!(matches!(
            adapter.apply_service_access_decision(changed).await,
            Err(PrivateOriginalError::Conflict)
        ));
        assert_eq!(std::fs::read(&path).unwrap(), native_bytes);
    }

    #[tokio::test]
    async fn expired_issued_and_denied_decisions_remain_observable_without_new_issuance() {
        for choice in [PrivateAccessChoice::Approve, PrivateAccessChoice::Deny] {
            let directory = directory();
            let path = directory.path().join("grants");
            let clock = Arc::new(Clock(AtomicU64::new(100)));
            let owner =
                crate::original_ingress::tests::initialized_with_clock(&path, clock.clone());
            let store_path = directory.path().join("originals");
            let adapter = adapter(owner, &store_path).await;
            let mut request = decision();
            request.choice = choice;
            request.permission.lifetime.expires_at_ms = Some(150);
            let before_expiry = adapter
                .apply_service_access_decision(request.clone())
                .await
                .unwrap();
            assert_eq!(
                before_expiry.state,
                match choice {
                    PrivateAccessChoice::Approve => PrivateServiceGrantState::GrantIssued,
                    PrivateAccessChoice::Deny => PrivateServiceGrantState::ApprovalDenied,
                }
            );
            let native_bytes = std::fs::read(&path).unwrap();
            clock.0.store(200, Ordering::SeqCst);
            let replay = adapter
                .apply_service_access_decision(request.clone())
                .await
                .unwrap();
            assert!(replay.grant == before_expiry.grant);
            assert_eq!(
                replay.state,
                match choice {
                    PrivateAccessChoice::Approve => PrivateServiceGrantState::GrantUnavailable,
                    PrivateAccessChoice::Deny => PrivateServiceGrantState::ApprovalDenied,
                }
            );
            assert!(matches!(
                adapter
                    .service_grant_lineage(&replay.grant, &original(&adapter, "work"))
                    .await,
                Err(PrivateOriginalError::Unavailable)
            ));
            // A distinct expired approval has no prior decision to observe.
            request.decision = Id::new("new-expired-approval").unwrap();
            request.choice = PrivateAccessChoice::Approve;
            assert!(matches!(
                adapter.apply_service_access_decision(request).await,
                Err(PrivateOriginalError::Denied)
            ));
            assert_eq!(std::fs::read(&path).unwrap(), native_bytes);
            let db = rusqlite::Connection::open(&store_path).unwrap();
            let rows: u64 = db
                .query_row("SELECT count(*) FROM toolkit_records", [], |row| row.get(0))
                .unwrap();
            assert_eq!(rows, 1, "expired new approval wrote another decision");
        }
    }

    #[tokio::test]
    async fn denied_access_and_wrong_native_administrator_never_issue() {
        let directory = directory();
        let path = directory.path().join("grants");
        let owner = crate::original_ingress::tests::initialized(&path);
        let mut adapter = adapter(owner.clone(), &directory.path().join("originals")).await;
        let before = std::fs::read(&path).unwrap();
        let mut denied = decision();
        denied.choice = PrivateAccessChoice::Deny;
        assert_eq!(
            adapter
                .apply_service_access_decision(denied)
                .await
                .unwrap()
                .state,
            PrivateServiceGrantState::ApprovalDenied
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
        let wrong_caller = adapter.routes.get().unwrap()[0].requester.clone();
        Arc::get_mut(&mut adapter).unwrap().routes.get_mut().unwrap()[0].grant_caller = wrong_caller;
        let mut approved = decision();
        approved.decision = Id::new("different-decision").unwrap();
        assert_eq!(
            adapter
                .apply_service_access_decision(approved)
                .await
                .unwrap()
                .state,
            PrivateServiceGrantState::GrantUnavailable
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    #[tokio::test]
    async fn actual_native_issue_without_saved_receipt_stays_pending_without_duplicate() {
        let directory = directory();
        let path = directory.path().join("grants");
        let clock = Arc::new(Clock(AtomicU64::new(100)));
        let owner = crate::original_ingress::tests::initialized_with_clock(&path, clock.clone());
        let adapter = adapter(owner.clone(), &directory.path().join("originals")).await;
        let mut request = decision();
        request.permission.lifetime.expires_at_ms = Some(150);
        let reference = PrivateServiceGrantRef {
            route: request.route.clone(),
            decision: request.decision.clone(),
            decision_sha256: digest(&canonical_json(&request).unwrap()),
        };
        let key = adapter.access_key(&reference).unwrap();
        let pending = AccessRecord {
            caller: adapter.routes.get().unwrap()[0].grant_caller.clone(),
            parent: adapter.routes.get().unwrap()[0].application_parent.clone().unwrap(),
            decision: AccessDecision::Legacy(request.clone()),
            issuance: IssuanceObservation::PendingNativeIssuance,
            mcp: None,
            application_policy: None,
        };
        adapter
            .store
            .admit(AdmissionRequest {
                key: key.clone(),
                admission_key: key.id.clone(),
                data: encode_access(&pending).unwrap(),
                events: vec![],
                effects: vec![],
            })
            .await
            .unwrap();
        let route = &adapter.routes.get().unwrap()[0];
        let now_ms = owner.clock().now().unwrap().unix_ms;
        owner
            .grants()
            .issue_child(
                &route.grant_caller,
                route.application_parent.as_ref().unwrap().last().unwrap(),
                EvidenceId::new(format!("access-{}", key.id)).unwrap(),
                route.logical_executor.clone(),
                restrictions(route, &request, now_ms).unwrap(),
            )
            .unwrap();
        // This is the real crash boundary after native issuance and before the
        // Toolkit receipt CAS, not a fake successful result from another owner.
        let before = std::fs::read(&path).unwrap();
        assert_eq!(
            adapter
                .apply_service_access_decision(request.clone())
                .await
                .unwrap()
                .state,
            PrivateServiceGrantState::PendingReconciliation
        );
        clock.0.store(200, Ordering::SeqCst);
        assert_eq!(
            adapter
                .apply_service_access_decision(request)
                .await
                .unwrap()
                .state,
            PrivateServiceGrantState::PendingReconciliation
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert!(matches!(
            adapter
                .service_grant_lineage(&reference, &original(&adapter, "work"))
                .await,
            Err(PrivateOriginalError::Unavailable)
        ));
    }

    #[tokio::test]
    async fn task_access_cannot_be_bound_to_sibling_original() {
        let directory = directory();
        let owner = crate::original_ingress::tests::initialized(&directory.path().join("grants"));
        let adapter = adapter(owner, &directory.path().join("originals")).await;
        let mut request = decision();
        request.permission.lifetime.scope = LifetimeScope::Task {
            task: Id::new("original-a").unwrap(),
        };
        let observed = adapter
            .apply_service_access_decision(request)
            .await
            .unwrap();
        assert!(
            adapter
                .service_grant_lineage(&observed.grant, &original(&adapter, "original-a"))
                .await
                .is_ok()
        );
        assert!(matches!(
            adapter
                .service_grant_lineage(&observed.grant, &original(&adapter, "original-b"))
                .await,
            Err(PrivateOriginalError::Denied)
        ));
    }
    #[tokio::test]
    async fn saved_parent_is_not_substituted_after_policy_change() {
        let directory = directory();
        let path = directory.path().join("grants");
        let owner = crate::original_ingress::tests::initialized(&path);
        let mut adapter = adapter(owner.clone(), &directory.path().join("originals")).await;
        let request = decision();
        let observed = adapter
            .apply_service_access_decision(request.clone())
            .await
            .unwrap();
        let old_parent = adapter.routes.get().unwrap()[0].application_parent.clone().unwrap();
        let operator = crate::original_ingress::tests::principal("admin");
        let replacement = owner
            .grants()
            .issue_root(
                &operator,
                EvidenceId::new("replacement-parent").unwrap(),
                app(),
                None,
                ExecutionRestrictions::unrestricted(),
            )
            .unwrap();
        Arc::get_mut(&mut adapter).unwrap().routes.get_mut().unwrap()[0].application_parent = Some(vec![replacement]);
        let before = std::fs::read(&path).unwrap();
        assert_eq!(
            adapter
                .apply_service_access_decision(request.clone())
                .await
                .unwrap()
                .state,
            PrivateServiceGrantState::GrantUnavailable
        );
        let stored = adapter
            .store
            .get(&adapter.access_key(&observed.grant).unwrap())
            .await
            .unwrap()
            .unwrap();
        assert!(
            adapter
                .decode_access(&stored, &observed.grant)
                .unwrap()
                .parent
                == old_parent
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
        Arc::get_mut(&mut adapter).unwrap().routes.get_mut().unwrap()[0].application_parent = None;
        let mut new = request;
        new.decision = Id::new("no-selected-parent").unwrap();
        assert!(matches!(
            adapter.apply_service_access_decision(new).await,
            Err(PrivateOriginalError::Unavailable)
        ));
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    #[tokio::test]
    async fn direct_task_leaves_bind_without_extra_issuance_and_revoke_only_one_sibling() {
        use crate::original_ingress::tests::{claims, machine, source};
        use meerkat_authorization_contracts::work_association::{
            InputAuthorityAssociation, WorkAuthorityBasis,
        };
        let directory = directory();
        let path = directory.path().join("grants");
        let owner = crate::original_ingress::tests::initialized(&path);
        let adapter = adapter(owner.clone(), &directory.path().join("originals")).await;
        let mut held = Vec::new();
        let mut lineages = Vec::new();
        for label in ["original-a", "original-b"] {
            let mut request = decision();
            request.decision = Id::new(label).unwrap();
            request.permission.lifetime.scope = LifetimeScope::Task {
                task: Id::new(label).unwrap(),
            };
            let access = adapter
                .apply_service_access_decision(request)
                .await
                .unwrap();
            assert_eq!(access.state, PrivateServiceGrantState::GrantIssued);
            let original = original(&adapter, label);
            let lineage = adapter
                .service_grant_lineage(&access.grant, &original)
                .await
                .unwrap();
            assert_eq!(
                lineage.len(),
                2,
                "operator->app root plus direct app->agent task leaf"
            );
            let mut candidate = claims(label, &lineage[0]).candidate().clone();
            candidate.authority_basis = WorkAuthorityBasis::GrantLineage {
                lineage: lineage.clone(),
            };
            let association = InputAuthorityAssociation::new(candidate).unwrap();
            let event = adapter
                .retain_event(crate::original_ingress::tests::private_event(label))
                .await
                .unwrap();
            let before = std::fs::read(&path).unwrap();
            let original = adapter
                .bind_access_source(
                    &event,
                    &access.grant,
                    association.clone(),
                    source(&association),
                )
                .await
                .unwrap();
            assert_eq!(
                std::fs::read(&path).unwrap(),
                before,
                "source binding must not issue another grant"
            );
            assert!(original.association() == &association);
            assert!(adapter.binding_evidence(&event).await.unwrap().as_ref() == Some(&association));
            held.push(original);
            lineages.push(lineage);
        }
        let native = machine(&owner);
        let mut custody = native.try_controller_grant_mutation().unwrap();
        owner
            .cancel_original(&held[0], &app(), &mut custody)
            .unwrap();
        assert!(
            owner
                .grants()
                .resolve_lineage(&lineages[0], &adapter.routes.get().unwrap()[0].logical_executor, None)
                .is_err()
        );
        assert!(
            owner
                .grants()
                .resolve_lineage(&lineages[1], &adapter.routes.get().unwrap()[0].logical_executor, None)
                .is_ok()
        );
        assert!(
            owner
                .grants()
                .resolve_lineage(
                    adapter.routes.get().unwrap()[0].application_parent.as_ref().unwrap(),
                    &app(),
                    None
                )
                .is_ok()
        );
        // The exact operator parent still owns both task permissions.
        owner
            .grants()
            .revoke(
                &crate::original_ingress::tests::principal("admin"),
                &adapter.routes.get().unwrap()[0].application_parent.as_ref().unwrap()[0],
                &mut custody,
            )
            .unwrap();
        assert!(
            owner
                .grants()
                .resolve_lineage(&lineages[1], &adapter.routes.get().unwrap()[0].logical_executor, None)
                .is_err()
        );
        // This checks native permission revocation only. No run was admitted,
        // so it establishes neither active work cancellation nor settlement.
    }
}
