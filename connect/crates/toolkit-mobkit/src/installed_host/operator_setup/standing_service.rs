//! Standing administrative service actions in the existing operator decision
//! store. Version 3 has no human ApprovalRecord; version 2 and its keys are
//! unchanged. LocalGrantAuthority remains the sole native issuance owner.
use super::*;
use crate::private_originals::{PrivateOriginalError, PrivateOriginalRef,
    PrivateServiceMcpAccessDecision, ServiceMcpSelection};
use meerkat_toolkit::host_lifetime::policy::ServiceParentAdministration;
use crate::private_originals::{ServiceParentAuthorizationProposal, ServiceParentAuthorizationEvidence,
    ServiceOperationCurrent, ServiceOperationError, ServiceOperationProgress};

const STANDING_VERSION: u32 = 3;

/// Historical administrative attribution, never a grant or an operator token.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ServiceParentRuleEvidence {
    pub operator: meerkat_core::PrincipalRef,
    pub rule: ServiceParentAdministration,
    pub issuance_id: EvidenceId,
}
impl ServiceParentRuleEvidence {
    pub(crate) fn check_selected(&self, lifetime: &NativeHostLifetime, route: &Id)
        -> Result<(), HostLifetimeError>
    {
        lifetime.validate_current_selection()?;
        let native = lifetime.selected_policy().native_host.as_ref()
            .ok_or(HostLifetimeError::Unavailable)?;
        if native.remote_operator.as_ref() != Some(&self.operator)
            || native.service_parent_administration.as_ref() != Some(&self.rule)
            || !self.rule.routes().contains(route)
        { return Err(HostLifetimeError::Denied); }
        Ok(())
    }
    pub(crate) fn check_parent(&self, parent: &[GrantLineageRef]) -> Result<(), HostLifetimeError> {
        if parent.len() != 1 || parent[0].root_authority != self.operator
            || parent[0].grant_id != self.issuance_id
        { return Err(HostLifetimeError::Denied); }
        Ok(())
    }
}

/// A versioned body of the same operator record. The native outcome uses the
/// existing ParentIssuance vocabulary and the existing canonical store.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StandingServiceDecision {
    authorization: ServiceParentRuleEvidence,
    administrative_decision: Option<ServiceParentAuthorizationEvidence>,
    source_policy_sha256: String,
    original: PrivateOriginalRef,
    acquisition: StoreKey,
    instruction_sha256: String,
    expires_in_ms: Option<u64>,
    application: meerkat_core::PrincipalRef,
    represented_subject: Option<meerkat_core::PrincipalRef>,
    decision: PrivateServiceMcpAccessDecision,
    // Exact nonsecret connector correspondence, never the MCP configuration.
    connector_selection: serde_json::Value,
    requested: ExecutionRestrictions,
    issued: ParentIssuance,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tool_policy: Option<crate::private_originals::ServiceToolPolicyProposal>,
}

pub(crate) struct StandingServiceParentProposal {
    record: StandingServiceDecision,
    deployment: Id,
    selection: ServiceMcpSelection,
    source: std::sync::Weak<ToolkitOriginalOwner>,
    lifetime: std::sync::Weak<NativeHostLifetime>,
}
impl StandingServiceParentProposal {
    pub(crate) fn authorization_proposal(&self) -> Result<ServiceParentAuthorizationProposal, HostLifetimeError> {
        authorization_proposal(&self.record, &self.deployment)
    }
    pub(crate) fn set_tool_policy(&mut self, proposal: crate::private_originals::ServiceToolPolicyProposal)
        -> Result<(), HostLifetimeError> {
        if self.record.tool_policy.is_some() || self.record.administrative_decision.is_some() {
            return Err(HostLifetimeError::Denied);
        }
        self.record.tool_policy = Some(proposal);
        Ok(())
    }
    pub(crate) fn authorize(&mut self, evidence: ServiceParentAuthorizationEvidence) -> Result<(), HostLifetimeError> {
        evidence.validate(&self.authorization_proposal()?)?;
        if self.record.administrative_decision.is_some() { return Err(HostLifetimeError::Denied); }
        self.record.administrative_decision = Some(evidence);
        Ok(())
    }
}

impl OperatorSetup {
    pub(crate) fn service_parent_rule(&self, original: &PrivateOriginalRef,
        acquisition: &StoreKey, fresh: &Id) -> Result<ServiceParentRuleEvidence, HostLifetimeError>
    {
        let selected = self.lifetime.selected_policy();
        if self.native.remote_operator.as_ref() != Some(&self.operator)
            || !acquisition_key_matches(acquisition, self.native.realm.as_str(), selected.deployment.as_str())
        { return Err(HostLifetimeError::Denied); }
        let rule = self.native.service_parent_administration.clone().ok_or(HostLifetimeError::Denied)?;
        let evidence = ServiceParentRuleEvidence { operator: self.operator.clone(), rule,
            issuance_id: standing_issuance_id(self.native.realm.as_str(), selected.deployment.as_str(),
                original, acquisition, fresh)? };
        evidence.check_selected(&self.lifetime, &original.route)?;
        self.route(&original.route)?;
        Ok(evidence)
    }

    /// Read the existing intent before a retry computes a new relative expiry.
    /// This is observation only and neither refreshes nor reissues the action.
    pub(crate) async fn retained_service_parent_expiry(&self, original: &PrivateOriginalRef,
        acquisition: &StoreKey, fresh: &Id, instruction_sha256: &str, expires_in_ms: Option<u64>,
        progress: Option<&ServiceOperationProgress>,
    ) -> Result<Option<u64>, HostLifetimeError> {
        let authorization = self.service_parent_rule(original, acquisition, fresh)?;
        if let Some(progress) = progress { progress.parent_attempt(&authorization.issuance_id); }
        let key = self.standing_key(&authorization);
        let Some(stored) = self.originals.get(&key).await.map_err(|_| HostLifetimeError::Unavailable)?
            else { return Ok(None); };
        let record = decode_standing(&stored, &key)?;
        if let Some(progress) = progress {
            match &record.issued {
                ParentIssuance::Pending => progress.uncertain(),
                ParentIssuance::Issued { .. } => progress.parent_effect(),
                _ => {},
            }
        }
        if record.authorization != authorization || record.original != *original
            || record.acquisition != *acquisition || record.decision.decision != *fresh
            || record.instruction_sha256 != instruction_sha256 || record.expires_in_ms != expires_in_ms
            || record.source_policy_sha256 != self.lifetime.selected_policy().source_policy_sha256
            || record.application != self.native.grant_administrator
            || record.represented_subject != self.route(&original.route)?.represented_subject
        { return Err(HostLifetimeError::Denied); }
        Ok(Some(record.decision.permission.lifetime.expires_at_ms.ok_or(HostLifetimeError::InvalidSelection)?))
    }

    pub(crate) fn prepare_standing_service_parent(&self,
        original: PrivateOriginalRef, acquisition: StoreKey, instruction_sha256: String,
        expires_in_ms: Option<u64>, decision: PrivateServiceMcpAccessDecision, selection: ServiceMcpSelection,
    ) -> Result<StandingServiceParentProposal, HostLifetimeError> {
        let authorization = self.service_parent_rule(&original, &acquisition, &decision.decision)?;
        if decision.route != original.route || decision.live_attachment.is_none()
            || !matches!(&decision.permission.lifetime.scope,
                meerkat_toolkit::authority::LifetimeScope::Task { task } if task == &decision.decision)
            || decision.permission.lifetime.expires_at_ms.is_none()
        { return Err(HostLifetimeError::Denied); }
        let requested = self.service_mcp_parent_restrictions(&decision, &selection)?;
        let record = StandingServiceDecision { authorization, administrative_decision: None,
            source_policy_sha256: self.lifetime.selected_policy().source_policy_sha256.clone(),
            represented_subject: self.route(&decision.route)?.represented_subject.clone(),
            application: self.native.grant_administrator.clone(), original, acquisition, instruction_sha256,
            expires_in_ms, connector_selection: selection.reviewed_content().map_err(source_error)?,
            decision, requested, issued: ParentIssuance::Pending, tool_policy: None };
        Ok(StandingServiceParentProposal { record, selection, deployment: self.lifetime.selected_policy().deployment.clone(),
            source: Arc::downgrade(&self.source), lifetime: Arc::downgrade(&self.lifetime) })
    }

    pub(crate) async fn apply_standing_service_parent(&self, proposal: StandingServiceParentProposal,
        task: &crate::private_originals::InstalledServiceTask,
        current: ServiceOperationCurrent, progress: Arc<ServiceOperationProgress>,
    ) -> Result<ServiceMcpParentOutcome, ServiceOperationError> {
        if proposal.source.upgrade().is_none_or(|source| !Arc::ptr_eq(&source, &self.source))
            || proposal.lifetime.upgrade().is_none_or(|lifetime| !Arc::ptr_eq(&lifetime, &self.lifetime))
        { return Err(HostLifetimeError::WrongOwner.into()); }
        let StandingServiceParentProposal { record, selection, .. } = proposal;
        record.administrative_decision.as_ref().ok_or(HostLifetimeError::Denied)?
            .validate(&authorization_proposal(&record, &self.lifetime.selected_policy().deployment)?)?;
        record.authorization.check_selected(&self.lifetime, &record.decision.route)?;
        current()?;
        let key = self.standing_key(&record.authorization);
        if let Some(stored) = self.originals.get(&key).await.map_err(|_| HostLifetimeError::Unavailable)? {
            let existing = decode_standing(&stored, &key)?;
            if standing_body(&existing)? != standing_body(&record)? { return Err(HostLifetimeError::Denied.into()); }
            task.check_setup_current().await.map_err(source_error)?;
            return self.observe_standing_service(stored, existing, selection, current, progress).await;
        }
        task.check_setup_current().await.map_err(source_error)?;
        current()?;
        let expected = standing_body(&record)?;
        let admitted = self.originals.admit(AdmissionRequest { key: key.clone(), admission_key: key.id.clone(),
            data: encode_standing(&record)?, events: vec![], effects: vec![] }).await
            .map_err(|_| HostLifetimeError::Unavailable)?;
        let stored = admitted.record().clone();
        let mut record = decode_standing(&stored, &key)?;
        if standing_body(&record)? != expected { return Err(HostLifetimeError::Denied.into()); }
        task.check_setup_current().await.map_err(source_error)?;
        if !matches!(admitted, AdmissionOutcome::Created(_)) {
            return self.observe_standing_service(stored, record, selection, current, progress).await;
        }
        // This handle remains nested in the retained operation future, never
        // in a disposable result waiter. The actor can keep serving commands.
        let lifetime = self.lifetime.clone();
        let grants = self.source.grants();
        let operator = self.operator.clone();
        let issuance_id = record.authorization.issuance_id.clone();
        let application = record.application.clone();
        let represented = record.represented_subject.clone();
        let requested = record.requested.clone();
        let worker_current = current.clone();
        let worker_progress = progress.clone();
        let issued = tokio::task::spawn_blocking(move || {
            let issue = || -> Result<_, ServiceOperationError> {
                worker_current()?;
                worker_progress.entering();
                Ok(grants.issue_root(&operator, issuance_id, application, represented, requested))
            };
            lifetime.with_current_setup(|| Ok(issue())).map_err(ServiceOperationError::from)?
        }).await;
        let mut current_refusal = None;
        record.issued = match issued {
            Ok(Ok(Ok(parent))) => { progress.parent_effect(); ParentIssuance::Issued { lineage: vec![parent] } },
            Ok(Ok(Err(meerkat_authorization::grants::GrantRefusal::Denied))) => {
                progress.native_refused();
                ParentIssuance::NativeRefused
            },
            Ok(Err(error @ (ServiceOperationError::Authority(HostLifetimeError::Denied | HostLifetimeError::WrongOwner | HostLifetimeError::InvalidSelection)
                | ServiceOperationError::Source(PrivateOriginalError::Denied) | ServiceOperationError::Cancelled))) => {
                current_refusal = Some(error);
                ParentIssuance::Denied
            },
            Ok(Err(error)) => { progress.uncertain(); return Err(error); },
            _ => { progress.uncertain(); return Ok(ServiceMcpParentOutcome::PendingReconciliation); },
        };
        let stored = match self.originals.compare_and_swap(Transition { key,
            expected_revision: stored.revision, expected_digest: Some(stored.content_digest), claim: None,
            data: encode_standing(&record)?, events: vec![], effects: vec![] }).await {
            Ok(stored) => stored,
            Err(_) => { progress.uncertain(); return Ok(ServiceMcpParentOutcome::PendingReconciliation); },
        };
        if let Some(error) = current_refusal { return Err(error); }
        self.observe_standing_service(stored, record, selection, current, progress).await
    }

    /// Reopen the actual administrative decision and native issuance. This is
    /// readback of existing authority, never a human review or a new issuance.
    pub(crate) async fn retained_standing_service_parent(&self, original: &PrivateOriginalRef,
        acquisition: &StoreKey, instruction_sha256: &str, decision: &PrivateServiceMcpAccessDecision,
        expected_parent: &[GrantLineageRef],
    ) -> Result<(), HostLifetimeError> {
        self.lifetime.validate_current_selection()?;
        if expected_parent.len() != 1 { return Err(HostLifetimeError::Denied); }
        let key = StoreKey { realm: self.native.realm.to_string(),
            owner: self.lifetime.selected_policy().deployment.to_string(),
            id: expected_parent[0].grant_id.as_str().to_owned() };
        let stored = self.originals.get(&key).await.map_err(|_| HostLifetimeError::Unavailable)?
            .ok_or(HostLifetimeError::Unavailable)?;
        let record = decode_standing(&stored, &key)?;
        if record.original != *original || record.acquisition != *acquisition
            || record.instruction_sha256 != instruction_sha256 || record.decision != *decision
            || record.source_policy_sha256 != self.lifetime.selected_policy().source_policy_sha256
            || record.application != self.native.grant_administrator
            || record.represented_subject != self.route(&original.route)?.represented_subject
            || !matches!(&record.issued, ParentIssuance::Issued { lineage } if lineage == expected_parent)
        { return Err(HostLifetimeError::Denied); }
        self.check_standing_lineage(&record, expected_parent)?;
        self.lifetime.validate_current_selection()
    }

    fn standing_key(&self, authorization: &ServiceParentRuleEvidence) -> StoreKey {
        StoreKey { realm: self.native.realm.to_string(), owner: self.lifetime.selected_policy().deployment.to_string(),
            id: authorization.issuance_id.as_str().to_owned() }
    }

    async fn observe_standing_service(&self, stored: StoredRecord, mut record: StandingServiceDecision,
        selection: ServiceMcpSelection, current: ServiceOperationCurrent, progress: Arc<ServiceOperationProgress>,
    ) -> Result<ServiceMcpParentOutcome, ServiceOperationError> {
        record.authorization.check_selected(&self.lifetime, &record.decision.route)?;
        if matches!(record.issued, ParentIssuance::Pending) {
            // Same recovery as the existing original-setup decision: this is
            // identity lookup only. Missing/unknown never permits reissuance.
            let issued = self.lifetime.with_current_setup(||
                Ok(self.source.grants().issued_reference(&record.authorization.issuance_id)))?;
            let parent = match issued { Ok(Some(parent)) => parent,
                _ => { progress.uncertain(); return Ok(ServiceMcpParentOutcome::PendingReconciliation); } };
            let lineage = vec![parent];
            record.authorization.check_parent(&lineage)?;
            self.check_standing_lineage(&record, &lineage)?;
            record.issued = ParentIssuance::Issued { lineage };
            if self.originals.compare_and_swap(Transition { key: stored.key,
                expected_revision: stored.revision, expected_digest: Some(stored.content_digest), claim: None,
                data: encode_standing(&record)?, events: vec![], effects: vec![] }).await.is_err()
            { progress.uncertain(); return Ok(ServiceMcpParentOutcome::PendingReconciliation); }
            progress.resolved_pending_parent();
        }
        match &record.issued {
            ParentIssuance::Denied => Ok(ServiceMcpParentOutcome::Denied),
            ParentIssuance::NativeRefused => Ok(ServiceMcpParentOutcome::Denied),
            ParentIssuance::Pending => Ok(ServiceMcpParentOutcome::PendingReconciliation),
            ParentIssuance::Issued { lineage } => {
                progress.parent_effect();
                self.check_standing_lineage(&record, lineage)?;
                current()?;
                Ok(ServiceMcpParentOutcome::Approved(self.service_parent_result(
                    record.decision.clone(), selection, lineage.clone())))
            }
        }
    }

    fn check_standing_lineage(&self, record: &StandingServiceDecision, lineage: &[GrantLineageRef])
        -> Result<(), HostLifetimeError> {
        record.authorization.check_selected(&self.lifetime, &record.decision.route)?;
        record.authorization.check_parent(lineage)?;
        let resolved = self.source.grants().resolve_lineage(lineage, &record.application,
            record.represented_subject.as_ref()).map_err(|_| HostLifetimeError::Unavailable)?;
        if resolved.restrictions() != &record.requested { return Err(HostLifetimeError::Unavailable); }
        resolved.check_current().map_err(|_| HostLifetimeError::Unavailable)
    }
}

fn standing_body(record: &StandingServiceDecision) -> Result<Vec<u8>, HostLifetimeError> {
    let legacy = canonical_json(&(&record.authorization, &record.administrative_decision, &record.source_policy_sha256, &record.original,
        &record.acquisition, &record.instruction_sha256, record.expires_in_ms, &record.application,
        &record.represented_subject, &record.decision, &record.connector_selection, &record.requested))
        .map_err(|_| HostLifetimeError::InvalidSelection)?;
    match &record.tool_policy {
        None => Ok(legacy),
        Some(policy) => canonical_json(&(legacy, policy)).map_err(|_| HostLifetimeError::InvalidSelection),
    }
}
fn authorization_proposal(record: &StandingServiceDecision, deployment: &Id) -> Result<ServiceParentAuthorizationProposal, HostLifetimeError> {
    Ok(ServiceParentAuthorizationProposal {
        operator: record.authorization.operator.clone(), rule: record.authorization.rule.clone(),
        issuance_id: record.authorization.issuance_id.clone(),
        deployment: deployment.clone(),
        source_policy_sha256: record.source_policy_sha256.clone(), original: record.original.clone(),
        acquisition: record.acquisition.clone(), instruction_sha256: record.instruction_sha256.clone(),
        expires_in_ms: record.expires_in_ms, application: record.application.clone(),
        represented_subject: record.represented_subject.clone(), decision: record.decision.clone(),
        connector_selection: record.connector_selection.clone(), restrictions: record.requested.clone(),
        tool_policy: record.tool_policy.clone(),
    })
}
fn standing_issuance_id(realm: &str, owner: &str, original: &PrivateOriginalRef,
    acquisition: &StoreKey, fresh: &Id) -> Result<EvidenceId, HostLifetimeError> {
    let id = format!("operator-service-parent-{}", digest(&canonical_json(&(
        SCHEMA, realm, owner, original, acquisition, fresh,
    )).map_err(|_| HostLifetimeError::InvalidSelection)?));
    EvidenceId::new(id).map_err(|_| HostLifetimeError::InvalidSelection)
}
fn encode_standing(record: &StandingServiceDecision) -> Result<RecordData, HostLifetimeError> {
    Ok(RecordData { schema: SCHEMA.into(), version: STANDING_VERSION,
        body: serde_json::to_value(record).map_err(|_| HostLifetimeError::InvalidSelection)? })
}
fn acquisition_key_matches(acquisition: &StoreKey, realm: &str, deployment: &str) -> bool {
    let (Ok(realm), Ok(deployment), Ok(operation)) = (
        Id::new(realm), Id::new(deployment), Id::new(acquisition.id.clone()),
    ) else { return false; };
    meerkat_toolkit::setup::SetupScope::new(realm, deployment).key(&operation) == *acquisition
}
fn decode_standing(stored: &StoredRecord, key: &StoreKey) -> Result<StandingServiceDecision, HostLifetimeError> {
    if &stored.key != key || stored.data.schema != SCHEMA || stored.data.version != STANDING_VERSION
        || stored.data.digest().ok().as_deref() != Some(stored.content_digest.as_str())
    { return Err(HostLifetimeError::InvalidSelection); }
    let record: StandingServiceDecision = serde_json::from_value(stored.data.body.clone())
        .map_err(|_| HostLifetimeError::InvalidSelection)?;
    if canonical_json(&record).map_err(|_| HostLifetimeError::InvalidSelection)?
        != canonical_json(&stored.data.body).map_err(|_| HostLifetimeError::InvalidSelection)?
        || record.authorization.issuance_id.as_str() != key.id || record.original.route != record.decision.route
        || !acquisition_key_matches(&record.acquisition, &key.realm, &key.owner)
        || record.authorization.issuance_id != standing_issuance_id(&key.realm, &key.owner,
            &record.original, &record.acquisition, &record.decision.decision)?
        || !record.authorization.rule.routes().contains(&record.original.route)
        || record.decision.live_attachment.is_none()
        || record.decision.permission.lifetime.expires_at_ms.is_none()
        || !matches!(&record.decision.permission.lifetime.scope,
            meerkat_toolkit::authority::LifetimeScope::Task { task } if task == &record.decision.decision)
    { return Err(HostLifetimeError::InvalidSelection); }
    if let ParentIssuance::Issued { lineage } = &record.issued { record.authorization.check_parent(lineage)?; }
    record.administrative_decision.as_ref().ok_or(HostLifetimeError::InvalidSelection)?
        .validate(&authorization_proposal(&record, &Id::new(key.owner.clone())
            .map_err(|_| HostLifetimeError::InvalidSelection)?)?)?;
    Ok(record)
}
fn source_error(error: PrivateOriginalError) -> HostLifetimeError {
    match error { PrivateOriginalError::Unavailable => HostLifetimeError::Unavailable,
        _ => HostLifetimeError::Denied }
}
