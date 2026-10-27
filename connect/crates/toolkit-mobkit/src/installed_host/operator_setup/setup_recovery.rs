//! Recovery of an approved original through the existing decision/source owners.
use super::*;
use crate::private_originals::{
    LocalControllerSetup, PrivateOriginalError, PrivateOriginalRef, PrivateSetupRequestSelection,
};
use meerkat_authorization_contracts::work_association::OriginalWorkRef;

impl OperatorSetup {
    /// Own the complete review and source selection for this prepared original.
    /// The callback only presents the owner's request and returns its actual
    /// ApprovalRecord. It cannot select a parent, principal, original or session.
    /// No source mutation custody or native identity lease spans the human wait.
    /// Retrying this operation recovers an approved decision before presenting.
    /// A denied review may be explicitly requested again; an approved decision
    /// is never replaced by a new review, including after unknown settlement.
    pub async fn review_original_setup_request<F, Fut>(
        &self,
        prepared: LocalControllerSetup,
        request: ApprovalRequest,
        review: F,
    ) -> Result<OperatorParentOutcome, HostLifetimeError>
    where
        F: FnOnce(ApprovalRequest) -> Fut,
        Fut: std::future::Future<Output = Result<ApprovalRecord, HostLifetimeError>>,
    {
        let proposal = self.prepare_original_setup_parent_review(&prepared, request)?;
        if let Some(outcome) = self
            .finish_original_setup(&prepared, &proposal, None)
            .await?
        {
            return Ok(outcome);
        }
        let approval = review(proposal.request().clone()).await?;
        self.finish_original_setup(&prepared, &proposal, Some(approval))
            .await?
            .ok_or(HostLifetimeError::Unavailable)
    }

    async fn finish_original_setup(
        &self,
        prepared: &LocalControllerSetup,
        proposal: &OperatorParentProposal,
        approval: Option<ApprovalRecord>,
    ) -> Result<Option<OperatorParentOutcome>, HostLifetimeError> {
        self.check_parent_proposal_owner(proposal)?;
        let source = prepared
            .source_owner()
            .ok_or(HostLifetimeError::Unavailable)?;
        if !prepared.matches_grant_owner(&self.source) {
            return Err(HostLifetimeError::WrongOwner);
        }
        let route = self.route(&prepared.original().route)?;
        let original = OriginalWorkRef {
            authority: route.source_authority.clone(),
            work: prepared.original().original.clone(),
        };
        self.source
            .with_unbound_source_selection(&self.native.realm, &original, || async {
                let selected_source = source
                    .check_setup_review_source(prepared.original())
                    .await
                    .map_err(source_error)?;
                let outcome =
                    if let Some(saved) = self.retained_setup_decision(prepared.original()).await? {
                        let record = decode(&saved, &saved.key)?;
                        self.check_retained_setup(proposal, &saved.key, &record)?;
                        // A simultaneous review cannot overwrite the decision that won.
                        if approval
                            .as_ref()
                            .is_some_and(|decision| decision != &record.approval)
                        {
                            return Err(HostLifetimeError::Denied);
                        }
                        self.observe_setup_decision(saved, record).await?
                    } else if selected_source.is_some() {
                        return Err(HostLifetimeError::Unavailable);
                    } else if let Some(approval) = approval {
                        self.apply_parent_decision(proposal.clone(), approval)
                            .await?
                    } else {
                        return Ok(None);
                    };
                let result = match outcome {
                    OperatorParentOutcome::Selected {
                        route,
                        purpose: OperatorParentPurpose::SetupRequest,
                        lineage,
                    } if route == prepared.original().route => {
                        let selection = PrivateSetupRequestSelection {
                            original: prepared.original().clone(),
                            session_id: prepared.session_id().clone(),
                            helper_targets: {
                                let saved = self.retained_setup_decision(prepared.original()).await?
                                    .ok_or(HostLifetimeError::Unavailable)?;
                                setup_targets::compiled_targets(&decode(&saved, &saved.key)?)?
                            },
                            parent: lineage,
                        };
                        match source.select_setup_request_locked(selection).await {
                            Ok(selection) => {
                                OperatorParentOutcome::SetupRequestSelected { selection }
                            }
                            // The canonical approved decision remains the retry
                            // coordinate even when the source write/read is unknown.
                            Err(PrivateOriginalError::Unavailable) => {
                                OperatorParentOutcome::PendingReconciliation
                            }
                            Err(_) => OperatorParentOutcome::Unavailable,
                        }
                    }
                    OperatorParentOutcome::Denied
                    | OperatorParentOutcome::Unavailable
                    | OperatorParentOutcome::PendingReconciliation => outcome,
                    _ => return Err(HostLifetimeError::Denied),
                };
                Ok(Some(result))
            })
            .await
            .map_err(|error| match error {
                crate::original_ingress::OriginalWriterError::Conflict
                | crate::original_ingress::OriginalWriterError::OperationCancelled => {
                    HostLifetimeError::Denied
                }
                _ => HostLifetimeError::Unavailable,
            })?
    }

    fn check_retained_setup(
        &self,
        proposal: &OperatorParentProposal,
        key: &StoreKey,
        record: &OperatorDecision,
    ) -> Result<(), HostLifetimeError> {
        let route = self.route(&proposal.route)?;
        // Only the new attempt deadline differs on retry. The actual prior
        // approved record remains authoritative, including protected context.
        let mut request = proposal.request.clone();
        request.expires_at = record.approval.expires_at;
        if !decision_key_matches(&self.lifetime, &self.native, key, record)?
            || record.route != proposal.route
            || record.purpose != OperatorParentPurpose::SetupRequest
            || record.setup_request != proposal.setup_request
            || record.operator != self.operator
            || record.application != self.native.grant_administrator
            || record.represented_subject != route.represented_subject
            || record.requested != proposal.restrictions
            || !approved(record)
            || !same_request(&request, &record.approval)
        {
            return Err(HostLifetimeError::Denied);
        }
        Ok(())
    }

    /// Read the exact canonical decision for this original. Unrelated realm
    /// records and enumeration availability cannot affect this lookup. The
    /// unreleased approval-ID setup format is not migrated or selected here.
    pub(super) async fn retained_setup_decision(
        &self,
        original: &PrivateOriginalRef,
    ) -> Result<Option<StoredRecord>, HostLifetimeError> {
        let key = StoreKey {
            realm: self.native.realm.to_string(),
            owner: self.lifetime.selected_policy().deployment.to_string(),
            id: original_decision_id(&self.native, original)?,
        };
        let Some(saved) = self
            .originals
            .get(&key)
            .await
            .map_err(|_| HostLifetimeError::Unavailable)?
        else {
            return Ok(None);
        };
        let record = decode(&saved, &key)?;
        if !decision_key_matches(&self.lifetime, &self.native, &key, &record)?
            || !approved(&record)
            || record.purpose != OperatorParentPurpose::SetupRequest
            || record
                .setup_request
                .as_ref()
                .and_then(|intent| intent.original.as_ref())
                != Some(original)
        {
            return Err(HostLifetimeError::Denied);
        }
        Ok(Some(saved))
    }

    pub(super) fn parent_decision_key(
        &self,
        record: &OperatorDecision,
    ) -> Result<StoreKey, HostLifetimeError> {
        Ok(StoreKey {
            realm: self.native.realm.to_string(),
            owner: self.lifetime.selected_policy().deployment.to_string(),
            id: decision_id(&self.native, record)?,
        })
    }

    /// Pending is recovered only by observing the actual native issuance owner.
    /// No reference means unknown, not permission to reissue. This also covers
    /// native success followed by failure of the decision's Issued-record CAS.
    pub(super) async fn observe_setup_decision(
        &self,
        saved: StoredRecord,
        mut record: OperatorDecision,
    ) -> Result<OperatorParentOutcome, HostLifetimeError> {
        if !matches!(record.issued, ParentIssuance::Pending) {
            return self.complete_setup_targets(saved, record, Arc::new(|| true)).await;
        }
        if !approved(&record)
            || !decision_key_matches(&self.lifetime, &self.native, &saved.key, &record)?
        {
            return Err(HostLifetimeError::Denied);
        }
        let parent = self.lifetime.with_current_setup(|| {
            let id = EvidenceId::new(saved.key.id.clone())
                .map_err(|_| HostLifetimeError::InvalidSelection)?;
            Ok(self.source.grants().issued_reference(&id))
        })?;
        let parent = match parent {
            Ok(Some(parent)) => parent,
            _ => return Ok(OperatorParentOutcome::PendingReconciliation),
        };
        if parent.root_authority != record.operator || parent.grant_id.as_str() != saved.key.id {
            return Ok(OperatorParentOutcome::Unavailable);
        }
        record.issued = ParentIssuance::Issued {
            lineage: vec![parent],
        };
        let current = self.source.grants().resolve_lineage(
            match &record.issued { ParentIssuance::Issued { lineage } => lineage, _ => unreachable!() },
            &record.application, record.represented_subject.as_ref());
        if !current.is_ok_and(|grant| grant.restrictions() == &record.requested && grant.check_current().is_ok()) {
            return Ok(OperatorParentOutcome::Unavailable);
        }
        match self
            .originals
            .compare_and_swap(Transition {
                key: saved.key,
                expected_revision: saved.revision,
                expected_digest: Some(saved.content_digest),
                claim: None,
                data: encode(&record)?,
                events: vec![],
                effects: vec![],
            })
            .await
        {
            Ok(saved) => self.complete_setup_targets(saved, record, Arc::new(|| true)).await,
            Err(_) => Ok(OperatorParentOutcome::PendingReconciliation),
        }
    }
}

fn approved(record: &OperatorDecision) -> bool {
    record.approval.status == ApprovalStatus::Approved
        && record
            .approval
            .decision
            .as_ref()
            .is_some_and(|decision| decision.decision == ApprovalDecision::Approve)
}
fn original_coordinate(
    native: &CompiledNativeHost,
    original: &PrivateOriginalRef,
) -> Result<String, HostLifetimeError> {
    let route = native
        .routes
        .iter()
        .find(|route| route.id == original.route)
        .ok_or(HostLifetimeError::Unavailable)?;
    crate::private_originals::source_key_id(
        &native.realm,
        &native.ingress_domain,
        &route.source_authority,
        &original.original,
    )
    .map_err(|_| HostLifetimeError::InvalidSelection)
}
fn original_decision_id(
    native: &CompiledNativeHost,
    original: &PrivateOriginalRef,
) -> Result<String, HostLifetimeError> {
    Ok(format!(
        "operator-original-setup-{}",
        digest(
            &canonical_json(&(SCHEMA, original_coordinate(native, original)?))
                .map_err(|_| HostLifetimeError::InvalidSelection)?
        )
    ))
}
fn approval_decision_id(record: &OperatorDecision) -> Result<String, HostLifetimeError> {
    Ok(format!(
        "operator-parent-{}",
        digest(
            &canonical_json(&(SCHEMA, &record.route, &record.approval.approval_id))
                .map_err(|_| HostLifetimeError::InvalidSelection)?
        )
    ))
}
fn decision_id(
    native: &CompiledNativeHost,
    record: &OperatorDecision,
) -> Result<String, HostLifetimeError> {
    if let Some(member) = &record.batch_member {
        return Ok(format!("operator-batch-parent-{}", digest(
            &canonical_json(&("toolkit.public-service-batch-parent.v1", &record.route,
                &record.approval.approval_id, member))
                .map_err(|_| HostLifetimeError::InvalidSelection)?)));
    }
    if approved(record) && record.purpose == OperatorParentPurpose::SetupRequest {
        if let Some(original) = record
            .setup_request
            .as_ref()
            .and_then(|intent| intent.original.as_ref())
        {
            return original_decision_id(native, original);
        }
    }
    // Controller, generic, tuple-only setup and denied decisions keep their
    // established approval-ID format. Only approved original setup is stable.
    approval_decision_id(record)
}
pub(super) fn decision_key_matches(
    lifetime: &NativeHostLifetime,
    native: &CompiledNativeHost,
    key: &StoreKey,
    record: &OperatorDecision,
) -> Result<bool, HostLifetimeError> {
    Ok(key.realm == native.realm.to_string()
        && key.owner == lifetime.selected_policy().deployment.to_string()
        && key.id == decision_id(native, record)?)
}
fn source_error(error: PrivateOriginalError) -> HostLifetimeError {
    match error {
        PrivateOriginalError::Unavailable => HostLifetimeError::Unavailable,
        _ => HostLifetimeError::Denied,
    }
}
