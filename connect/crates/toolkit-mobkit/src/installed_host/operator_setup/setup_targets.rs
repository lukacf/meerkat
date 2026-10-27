//! Correlated target selection by the existing installed helper/operator owners.
//! Each target has one independent native root; no tuple dimensions are united.
use meerkat_authorization_contracts::constraints::DomainRestriction;
use super::*;
use crate::installed_host::acquisition::{ConfiguredHelperReportCapability, InstalledSetupSelection};
use crate::original_ingress::helper_originals::HelperSetupOrigin;
use meerkat_authorization_contracts::{
    constraints::{AudienceRef, DelegationDepth, ExactRestriction, ProcessorRef},
    work_association::{GrantTarget, MAX_ASSOCIATION_REFERENCES},
};
use meerkat_toolkit::host_lifetime::policy::CompiledAgentSetupTarget;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetupTargetRequest {
    pub target: Option<GrantTarget>,
    pub issuance_id: EvidenceId,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SetupTargetIntent {
    pub target: Option<GrantTarget>,
    pub requested: ExecutionRestrictions,
}

pub(crate) fn find_target() -> GrantTarget {
    GrantTarget::RegisteredToolBundleOperation {
        bundle: crate::FIND_AND_INSTALL_BUNDLE.into(),
        operation: crate::FIND_AND_INSTALL.into(),
    }
}

pub(crate) fn validate_targets<'a>(targets: impl IntoIterator<Item = &'a Option<GrantTarget>>)
    -> Result<(), HostLifetimeError>
{
    let targets = targets.into_iter().collect::<Vec<_>>();
    if targets.len() >= MAX_ASSOCIATION_REFERENCES
        || targets.windows(2).any(|pair| pair[0] >= pair[1])
        || targets.iter().any(|target| match target {
            None => false,
            Some(GrantTarget::McpServer { server }) => server.is_empty(),
            Some(target) => target != &ConfiguredHelperReportCapability::target()
                && target != &ConfiguredHelperReportCapability::account_target()
                && target != &ConfiguredHelperReportCapability::public_target(),
        })
    { return Err(HostLifetimeError::InvalidSelection); }
    Ok(())
}

pub(super) fn helper_intents(source: &ToolkitOriginalOwner, helper: &InstalledSetupSelection,
    application: &meerkat_core::PrincipalRef, audience: &AudienceRef,
    targets: impl IntoIterator<Item = Option<GrantTarget>>)
    -> Result<Vec<SetupTargetIntent>, HostLifetimeError>
{
    let targets = targets.into_iter().collect::<Vec<_>>();
    validate_targets(targets.iter())?;
    targets.into_iter().map(|target| {
        let requested = if target.is_none() && helper.catalog_capability.is_some() {
            let capability = helper.catalog_capability.as_ref().ok_or(HostLifetimeError::Denied)?;
            capability.validate(&helper.operation_basis)?;
            let expected = application_operations::configured_catalog_parent(application, &helper.logical_executor, audience);
            let current = source.grants().resolve_lineage(&capability.parent, application,
                helper.represented_subject.as_ref()).map_err(|_| HostLifetimeError::Denied)?;
            if current.restrictions() != &expected { return Err(HostLifetimeError::Denied); }
            current.check_current().map_err(|_| HostLifetimeError::Unavailable)?;
            // Only an independently requested/reviewed Original helper target
            // copies this catalog-only scope. Existing grants stay immutable.
            expected
        } else if target.as_ref() == Some(&ConfiguredHelperReportCapability::target()) {
            if helper.report_capability.is_none() { return Err(HostLifetimeError::Denied); }
            configured_report::report_parent(application, &helper.logical_executor, audience)
        } else if target.as_ref() == Some(&ConfiguredHelperReportCapability::account_target()) {
            if helper.report_capability.as_ref().and_then(|cap| cap.account_parent.as_ref()).is_none() {
                return Err(HostLifetimeError::Denied);
            }
            configured_report::account_parent(application, &helper.logical_executor, audience)
        } else if target.as_ref() == Some(&ConfiguredHelperReportCapability::public_target()) {
            if helper.report_capability.as_ref().and_then(|cap| cap.public_parent.as_ref()).is_none() {
                return Err(HostLifetimeError::Denied);
            }
            configured_report::public_parent(application, &helper.logical_executor, audience)
        } else {
            let lineage = helper.operation_basis.lineage_for(target.as_ref()).ok_or(HostLifetimeError::Denied)?;
            let current = source.grants().resolve_lineage(lineage, &helper.logical_executor,
                helper.represented_subject.as_ref()).map_err(|_| HostLifetimeError::Denied)?;
            current.check_current().map_err(|_| HostLifetimeError::Unavailable)?;
            let mut requested = current.restrictions().clone();
            // This is an explicit copy of the selected helper operation scope,
            // not an inference from an action label or a Cartesian union.
            if requested.processors != ExactRestriction::exact([ProcessorRef::Principal { principal: helper.logical_executor.clone() }])
                || requested.audiences != ExactRestriction::exact([audience.clone()])
            { return Err(HostLifetimeError::Denied); }
            requested.delegation_depth = DelegationDepth::remaining(1);
            requested
        };
        Ok(SetupTargetIntent { target, requested })
    }).collect()
}

/// Narrow only the qualified-origin resource for the three setup operations. Static MCP
/// and catalog scopes retain the exact independently reviewed restrictions.
pub(crate) fn child_bounds(application: &meerkat_core::PrincipalRef,
    origin: &HelperSetupOrigin, target: &Option<GrantTarget>, parent: &ExecutionRestrictions)
    -> Result<ExecutionRestrictions, HostLifetimeError>
{
    let mut requested = parent.clone();
    if target.as_ref() == Some(&ConfiguredHelperReportCapability::target())
        || target.as_ref() == Some(&ConfiguredHelperReportCapability::account_target())
        || target.as_ref() == Some(&ConfiguredHelperReportCapability::public_target())
    {
        // The exact processor/audience were selected by the actual helper
        // owner and retained in the reviewed intent, not reconstructed here.
        let action = if target.as_ref() == Some(&ConfiguredHelperReportCapability::target()) {
            "setup-report"
        } else if target.as_ref() == Some(&ConfiguredHelperReportCapability::account_target()) {
            "setup-account-request"
        } else { "setup-public-service-request" };
        let resource = application_operations::qualified_setup_resource(application, origin, action)
            .map_err(|_| HostLifetimeError::InvalidSelection)?;
        requested.resource_domains = DomainRestriction::exact([resource]);
    }
    requested.delegation_depth = DelegationDepth::remaining(0);
    Ok(requested)
}

pub(crate) fn validate_compiled(targets: &[CompiledAgentSetupTarget]) -> Result<(), HostLifetimeError> {
    validate_targets(targets.iter().map(|target| &target.target))?;
    if targets.iter().enumerate().any(|(index, target)| target.parent.validate().is_err()
        || target.restrictions.delegation_depth != DelegationDepth::remaining(1)
        || targets[..index].iter().any(|prior| prior.parent == target.parent))
    { return Err(HostLifetimeError::InvalidSelection); }
    Ok(())
}


/// Progress belongs to the same immutable operator decision. Pending means
/// entry may have occurred, so recovery observes the native owner only.
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum TargetIssuance {
    NotStarted,
    Pending,
    Refused,
    Issued { parent: GrantLineageRef },
}

pub(super) fn target_id(key: &StoreKey, target: &Option<GrantTarget>) -> Result<EvidenceId, HostLifetimeError> {
    let bytes = canonical_json(&("toolkit.original-setup-parent-target.v1", key, target))
        .map_err(|_| HostLifetimeError::InvalidSelection)?;
    EvidenceId::new(format!("operator-setup-target/{}", digest(&bytes)))
        .map_err(|_| HostLifetimeError::InvalidSelection)
}

pub(super) fn validate_progress(record: &OperatorDecision, key: &StoreKey) -> Result<(), HostLifetimeError> {
    let targets = record.setup_request.as_ref().map(|intent| intent.targets.as_slice()).unwrap_or(&[]);
    validate_targets(targets.iter().map(|target| &target.target))?;
    if targets.len() != record.target_issuance.len() { return Err(HostLifetimeError::InvalidSelection); }
    if !targets.is_empty() {
        let intent = record.setup_request.as_ref().ok_or(HostLifetimeError::InvalidSelection)?;
        let origin = intent.origin.as_ref().ok_or(HostLifetimeError::InvalidSelection)?;
        if intent.original.as_ref() != Some(&origin.original) || origin.realm.to_string() != key.realm
            || record.purpose != OperatorParentPurpose::SetupRequest
        { return Err(HostLifetimeError::InvalidSelection); }
    }
    for (target, progress) in targets.iter().zip(&record.target_issuance) {
        if target.requested.delegation_depth != DelegationDepth::remaining(1) {
            return Err(HostLifetimeError::InvalidSelection);
        }
        if let TargetIssuance::Issued { parent } = progress {
            if parent.validate().is_err() || parent.root_authority != record.operator
                || parent.grant_id != target_id(key, &target.target)?
            { return Err(HostLifetimeError::InvalidSelection); }
        }
    }
    Ok(())
}

pub(super) fn compiled_targets(record: &OperatorDecision) -> Result<Vec<CompiledAgentSetupTarget>, HostLifetimeError> {
    let targets = record.setup_request.as_ref().map(|intent| intent.targets.as_slice()).unwrap_or(&[]);
    if targets.len() != record.target_issuance.len() { return Err(HostLifetimeError::InvalidSelection); }
    targets.iter().zip(&record.target_issuance).map(|(target, progress)| {
        let TargetIssuance::Issued { parent } = progress else { return Err(HostLifetimeError::Unavailable); };
        Ok(CompiledAgentSetupTarget { target: target.target.clone(), parent: parent.clone(), restrictions: target.requested.clone() })
    }).collect()
}

impl OperatorSetup {
    pub(super) fn observe_target_progress(&self, record: &OperatorDecision)
        -> Result<Option<OperatorParentOutcome>, HostLifetimeError>
    {
        if record.target_issuance.iter().any(|state| matches!(state, TargetIssuance::Refused)) {
            return Ok(Some(OperatorParentOutcome::Unavailable));
        }
        if record.target_issuance.iter().any(|state| !matches!(state, TargetIssuance::Issued { .. })) {
            return Ok(Some(OperatorParentOutcome::PendingReconciliation));
        }
        for target in compiled_targets(record)? {
            let current = self.source.grants().resolve_lineage(std::slice::from_ref(&target.parent),
                &record.application, record.represented_subject.as_ref());
            if !current.is_ok_and(|grant| grant.restrictions() == &target.restrictions && grant.check_current().is_ok()) {
                return Ok(Some(OperatorParentOutcome::Unavailable));
            }
        }
        Ok(None)
    }

    pub(super) async fn complete_setup_targets(&self, mut saved: StoredRecord,
        mut record: OperatorDecision, presented: Arc<dyn Fn() -> bool + Send + Sync>)
        -> Result<OperatorParentOutcome, HostLifetimeError>
    {
        if !matches!(record.issued, ParentIssuance::Issued { .. }) { return self.observe(&record); }
        let targets = record.setup_request.as_ref().map(|intent| intent.targets.clone()).unwrap_or_default();
        for (index, target) in targets.iter().enumerate() {
            let id = target_id(&saved.key, &target.target)?;
            let enter = matches!(record.target_issuance[index], TargetIssuance::NotStarted);
            if enter {
                // Persist entry intent before touching the native writer. If
                // that CAS is unknown, no write is attempted on this path.
                record.target_issuance[index] = TargetIssuance::Pending;
                saved = match self.persist_target_progress(&saved, &record).await {
                    Ok(saved) => saved,
                    Err(_) => return Ok(OperatorParentOutcome::PendingReconciliation),
                };
            }
            let parent = match &record.target_issuance[index] {
                TargetIssuance::Issued { .. } => continue,
                TargetIssuance::Refused => return Ok(OperatorParentOutcome::Unavailable),
                TargetIssuance::NotStarted => return Err(HostLifetimeError::InvalidSelection),
                TargetIssuance::Pending => self.lifetime.with_current_setup(|| {
                    if enter {
                        if !presented() { return Ok(Err(meerkat_authorization::grants::GrantRefusal::Denied)); }
                        Ok(self.source.grants().issue_root(&record.operator, id.clone(),
                            record.application.clone(), record.represented_subject.clone(), target.requested.clone()).map(Some))
                    } else {
                        Ok(self.source.grants().issued_reference(&id))
                    }
                })?,
            };
            record.target_issuance[index] = match parent {
                Ok(Some(parent)) if parent.root_authority == record.operator && parent.grant_id == id => {
                    let current = self.source.grants().resolve_lineage(std::slice::from_ref(&parent),
                        &record.application, record.represented_subject.as_ref());
                    if !current.is_ok_and(|grant| grant.restrictions() == &target.requested && grant.check_current().is_ok()) {
                        return Ok(OperatorParentOutcome::Unavailable);
                    }
                    TargetIssuance::Issued { parent }
                }
                Err(meerkat_authorization::grants::GrantRefusal::Denied) if enter => TargetIssuance::Refused,
                Ok(Some(_)) => return Ok(OperatorParentOutcome::Unavailable),
                _ => return Ok(OperatorParentOutcome::PendingReconciliation),
            };
            saved = match self.persist_target_progress(&saved, &record).await {
                Ok(saved) => saved,
                Err(_) => return Ok(OperatorParentOutcome::PendingReconciliation),
            };
        }
        self.observe(&record)
    }

    async fn persist_target_progress(&self, saved: &StoredRecord, record: &OperatorDecision)
        -> Result<StoredRecord, HostLifetimeError>
    {
        self.originals.compare_and_swap(Transition { key: saved.key.clone(), expected_revision: saved.revision,
            expected_digest: Some(saved.content_digest.clone()), claim: None, data: encode(record)?,
            events: vec![], effects: vec![] }).await.map_err(|_| HostLifetimeError::Unavailable)
    }
}
