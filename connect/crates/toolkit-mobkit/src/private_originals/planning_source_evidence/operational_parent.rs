//! Selected P intent and actual native grant observations share the source owner.
use super::*;
use crate::installed_host::operator_setup::{
    WorkflowParentAuthoritySelection, workflow_operation_envelope,
};
use meerkat_authorization_contracts::constraints::{DelegationDepth, LifetimeBound};
use meerkat_toolkit::host_lifetime::control::HostLifetimeError;

impl PrivateOriginals {
    pub(crate) async fn select_workflow_parent_authority(
        &self,
        reference: &PrivateOriginalRef,
        selection: WorkflowParentAuthoritySelection,
    ) -> Result<(), PrivateOriginalError> {
        self.select_planning_source_evidence(
            reference,
            PlanningSourceEvidence::OperationalParent { selection },
            None,
        )
        .await
    }

    pub(crate) async fn with_workflow_parent_selection<T>(
        &self,
        reference: &PrivateOriginalRef,
        selection: &WorkflowParentAuthoritySelection,
        action: impl FnOnce() -> Result<T, HostLifetimeError>,
    ) -> Result<T, PrivateOriginalError> {
        let _writer = self.owner.source_writer().await;
        let row = self
            .store
            .get(&self.key(reference)?)
            .await
            .map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        let record = self.decode_record(&row, reference)?;
        self.check_planning_evidence_source(&record, reference)?;
        if record.operation_cancelled {
            return Err(PrivateOriginalError::Denied);
        }
        if record.planning_source_evidence.as_ref()
            != Some(&PlanningSourceEvidence::OperationalParent {
                selection: selection.clone(),
            })
        {
            return Err(PrivateOriginalError::Conflict);
        }
        action().map_err(|error| match error {
            HostLifetimeError::Unavailable | HostLifetimeError::NotSettled => {
                PrivateOriginalError::Unavailable
            }
            _ => PrivateOriginalError::Denied,
        })
    }

    /// Commissioning cannot reinterpret a prepared P as another workflow or G.
    /// Legacy warm sources retain their existing contract and native ceilings.
    pub async fn check_workflow_parent_commission(
        &self,
        reference: &PrivateOriginalRef,
        commission_sha256: &str,
    ) -> Result<(), PrivateOriginalError> {
        let row = self
            .store
            .get(&self.key(reference)?)
            .await
            .map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        let record = self.decode_record(&row, reference)?;
        self.check_planning_evidence_source(&record, reference)?;
        if record.operation_cancelled {
            return Err(PrivateOriginalError::Denied);
        }
        if let Some(PlanningSourceEvidence::OperationalParent { selection }) =
            &record.planning_source_evidence
        {
            if selection.commission_sha256 != commission_sha256 {
                return Err(PrivateOriginalError::Conflict);
            }
            self.workflow_parent_basis(&record, reference)?;
        }
        Ok(())
    }

    pub(in crate::private_originals) fn workflow_parent_basis(
        &self,
        record: &SourceRecord,
        reference: &PrivateOriginalRef,
    ) -> Result<Option<WorkAuthorityBasis>, PrivateOriginalError> {
        let Some(PlanningSourceEvidence::OperationalParent { selection }) =
            &record.planning_source_evidence
        else {
            return Ok(None);
        };
        self.check_planning_evidence_source(record, reference)?;
        if record.operation_cancelled {
            return Err(PrivateOriginalError::Denied);
        }
        let lifetime = self
            .host_lifetime
            .get()
            .and_then(Weak::upgrade)
            .ok_or(PrivateOriginalError::Unavailable)?;
        let native = lifetime
            .selected_policy()
            .native_host
            .as_ref()
            .ok_or(PrivateOriginalError::Unavailable)?;
        let LifetimeBound::Window {
            not_before_ms,
            expires_at_ms,
        } = selection.authority.envelope.lifetime.bound()
        else {
            return Err(PrivateOriginalError::Conflict);
        };
        let expected =
            workflow_operation_envelope(native, &selection.instance, not_before_ms, expires_at_ms)
                .map_err(|_| PrivateOriginalError::Denied)?;
        if expected != selection.authority.envelope {
            return Err(PrivateOriginalError::Conflict);
        }
        let grants = self.owner.grants();
        let reference_for = |id| {
            grants
                .issued_reference(id)
                .map_err(|error| writer_error(OriginalWriterError::Grant(error)))?
                .ok_or(PrivateOriginalError::Unavailable)
        };
        let root = reference_for(&selection.authority.parent_issuance_id)?;
        let leaf = reference_for(&selection.authority.leaf_issuance_id)?;
        let mut root_bounds = expected.clone();
        root_bounds.delegation_depth = DelegationDepth::remaining(1);
        let root_observed = grants
            .resolve_lineage(
                std::slice::from_ref(&root),
                &native.grant_administrator,
                record.source.represented_subject.as_ref(),
            )
            .map_err(|error| writer_error(OriginalWriterError::Grant(error)))?;
        if root_observed.restrictions() != &root_bounds {
            return Err(PrivateOriginalError::Conflict);
        }
        let lineage = vec![root, leaf];
        let observed = grants
            .resolve_lineage(
                &lineage,
                &record.source.logical_executor,
                record.source.represented_subject.as_ref(),
            )
            .map_err(|error| writer_error(OriginalWriterError::Grant(error)))?;
        if observed.restrictions() != &expected {
            return Err(PrivateOriginalError::Conflict);
        }
        root_observed
            .check_current()
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        observed
            .check_current()
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        if record
            .final_association
            .as_ref()
            .is_some_and(|association| {
                association.candidate().authority_basis
                    != (WorkAuthorityBasis::GrantLineage {
                        lineage: lineage.clone(),
                    })
            })
        {
            return Err(PrivateOriginalError::Conflict);
        }
        Ok(Some(WorkAuthorityBasis::GrantLineage { lineage }))
    }
}
