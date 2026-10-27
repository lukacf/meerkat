//! Read the existing durable operator decision. This never reissues a grant.
use super::*;
use crate::private_originals::PrivateSourceEvidence;

impl OperatorSetup {
    pub(crate) async fn retained_service_review(
        &self, route: &Id, evidence: &PrivateSourceEvidence,
        expected_parent: &[GrantLineageRef],
    ) -> Result<ApprovalRecord, HostLifetimeError> {
        Ok(self.retained_service_review_record(route, evidence, expected_parent).await?.approval)
    }

    pub(crate) async fn retained_service_installation_review(
        &self, route: &Id, evidence: &PrivateSourceEvidence, expected_parent: &[GrantLineageRef],
    ) -> Result<crate::private_originals::ServiceInstallationReadbackSource, HostLifetimeError> {
        let record = self.retained_service_review_record(route, evidence, expected_parent).await?;
        if record.batch_member.is_none() {
            return Ok(crate::private_originals::ServiceInstallationReadbackSource::SettledReview(record.approval));
        }
        let body = service_batch::member_body(&record)?.ok_or(HostLifetimeError::InvalidSelection)?;
        let document = body.get("access_request").cloned().ok_or(HostLifetimeError::InvalidSelection)?;
        Ok(crate::private_originals::ServiceInstallationReadbackSource::ReviewedBatchMember(
            crate::private_originals::ReviewedBatchInstallation::new(document)))
    }

    async fn retained_service_review_record(
        &self, route: &Id, evidence: &PrivateSourceEvidence, expected_parent: &[GrantLineageRef],
    ) -> Result<OperatorDecision, HostLifetimeError> {
        self.lifetime.validate_current_selection()?;
        if evidence.revision.as_str() != "settled-review-v1" || expected_parent.len() != 1 {
            return Err(HostLifetimeError::Denied);
        }
        // The issuer already names its row with the grant id. Do not derive
        // a new authority or search other operator decisions by description.
        let key = StoreKey {
            realm: self.native.realm.to_string(),
            owner: self.lifetime.selected_policy().deployment.to_string(),
            id: expected_parent[0].grant_id.as_str().to_owned(),
        };
        let stored = self.originals.get(&key).await
            .map_err(|_| HostLifetimeError::Unavailable)?
            .ok_or(HostLifetimeError::Unavailable)?;
        let record = decode(&stored, &key)?;
        let selected = self.route(route)?;
        let expected_key = self.parent_decision_key(&record)?;
        if key != expected_key || record.route != *route
            || record.purpose != OperatorParentPurpose::Operations
            || record.operator != self.operator
            || record.application != self.native.grant_administrator
            || record.represented_subject != selected.represented_subject
            || record.approval.approval_id.to_string() != evidence.resource_id.as_str()
            || digest(&canonical_json(&record.approval).map_err(|_| HostLifetimeError::InvalidSelection)?) != evidence.sha256
            || record.approval.status != ApprovalStatus::Approved
            || !record.approval.decision.as_ref().is_some_and(|d| d.decision == ApprovalDecision::Approve)
            || !matches!(&record.issued, ParentIssuance::Issued { lineage } if lineage == expected_parent)
        { return Err(HostLifetimeError::Denied); }
        if !matches!(self.observe(&record)?, OperatorParentOutcome::Selected {
            route: ref selected_route, purpose: OperatorParentPurpose::Operations, ref lineage,
        } if selected_route == route && lineage == expected_parent) {
            return Err(HostLifetimeError::Unavailable);
        }
        self.lifetime.validate_current_selection()?;
        Ok(record)
    }
}
