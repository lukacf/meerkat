//! Canonical selected-interest recovery inside the existing joined host worker.
//! No new source input, Find grant, member selection or context transfer is made.
use super::*;
use crate::private_originals::{
    cold_service_admission::writer_error, planning_source_evidence::PlanningSourceEvidence,
};

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum WorkflowInterestRecovery {
    NotSelected,
    Restored { memberships: usize },
    Stopped,
}

#[cfg(test)]
tokio::task_local! {
    // Same-crate control only: hold a recovery step at its workflow lead's
    // wake selection, after the step's canonical reads.
    pub(in crate::private_originals) static LEAD_WAKE_SELECTION:
        Arc<(tokio::sync::Notify, tokio::sync::Notify)>;
}

impl PrivateOriginals {
    /// Consume one row from the existing canonical source page. The row and
    /// cursor are disposable observations; every selected source is reread.
    /// The worker retains this whole future and checks stop between its awaits.
    pub(crate) async fn recover_workflow_interest_source(
        self: &Arc<Self>,
        observed: &toolkit_store::StoredRecord,
        enter: impl Fn() -> bool + Send + Sync,
    ) -> Result<WorkflowInterestRecovery, PrivateOriginalError> {
        if !enter() {
            return Ok(WorkflowInterestRecovery::Stopped);
        }
        if observed.key.scope() != self.scope {
            return Err(PrivateOriginalError::Denied);
        }
        self.service_input_host_current()?;
        if observed.data.schema != SOURCE_SCHEMA {
            return Ok(WorkflowInterestRecovery::NotSelected);
        }
        // One publication observation covers this step's canonical reads. A
        // workflow lead's wake selected from them carries it: a cancellation
        // or any other owner change completed after these reads refuses
        // before the wake. No fresh observation starts after stale evidence
        // was read.
        let (_, stamp) = self
            .owner
            .publication()
            .observe(|| ())
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        let row = self
            .store
            .get(&observed.key)
            .await
            .map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        if !enter() {
            return Ok(WorkflowInterestRecovery::Stopped);
        }
        self.service_input_host_current()?;
        let record = self.decode_stored(&row)?;
        // Recreate only this owner's historical reference using the same event
        // projection as retain_event, then validate it against the canonical row.
        let original = PrivateOriginalRef {
            route: record.event.route.clone(),
            original: record.event.original.clone(),
            event_sha256: digest(
                &canonical_json(&record.event).map_err(|_| PrivateOriginalError::Conflict)?,
            ),
        };
        if let Some(PlanningSourceEvidence::Lead { parent }) = &record.planning_source_evidence {
            let Some(provenance) = &record.late_workflow_helper else {
                return Ok(WorkflowInterestRecovery::NotSelected);
            };
            self.decode_record(&row, &original)?;
            if record.operation_cancelled {
                return Err(PrivateOriginalError::Denied);
            }
            let binding = provenance
                .admission
                .workflow
                .as_ref()
                .ok_or(PrivateOriginalError::Conflict)?;
            if binding.original != original || &binding.parent != parent {
                return Err(PrivateOriginalError::Conflict);
            }
            // The same capture-independent checks as for a waiter's lead,
            // before the wake: the lead's own record, then the workflow that
            // names it as planning source.
            let association = Self::selected_lead_before_wake(&record)?;
            let snapshot = self
                .workflow_store()?
                .load_setup_workflow(&binding.workflow)
                .await
                .map_err(|_| PrivateOriginalError::Unavailable)?;
            if !enter() {
                return Ok(WorkflowInterestRecovery::Stopped);
            }
            if snapshot.terms().planning_source != row.key {
                return Err(PrivateOriginalError::Conflict);
            }
            self.wake_workflow_lead(association, &stamp).await?;
            let member = self
                .owner
                .captured_workflow_member(&binding.workflow, association)
                .map_err(writer_error)?;
            if member != provenance.admission.instance.member.member {
                return Err(PrivateOriginalError::Conflict);
            }
            if self.held_workflow_source(&original, association)? {
                self.restore_workflow_parent_source(parent).await?;
            } else {
                self.restore_workflow_lead_source(&binding.workflow, &original, &member)
                    .await?;
            }
            if !enter() {
                return Ok(WorkflowInterestRecovery::Stopped);
            }
            // A fully settled Lead still needs its restrictive attachment and
            // exact plan for a genuinely new B. No departed interest is revived.
            self.restore_workflow_planning_source(&binding.workflow, &original, &member)
                .await?;
            return Ok(WorkflowInterestRecovery::Restored { memberships: 0 });
        }
        let (workflow, planning) = match &record.planning_source_evidence {
            Some(
                PlanningSourceEvidence::Waiter { workflow, planning }
                | PlanningSourceEvidence::EffectWaiter { workflow, planning },
            ) => (workflow, planning),
            _ => return Ok(WorkflowInterestRecovery::NotSelected),
        };
        self.decode_record(&row, &original)?;
        if record.operation_cancelled {
            return Err(PrivateOriginalError::Denied);
        }
        if record.final_association.is_none() || record.bound_service_input.is_none() {
            return Err(PrivateOriginalError::Unavailable);
        }
        let snapshot = self
            .workflow_store()?
            .load_setup_workflow(workflow)
            .await
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        if !enter() {
            return Ok(WorkflowInterestRecovery::Stopped);
        }
        let now = self
            .owner
            .clock()
            .now()
            .map_err(|_| PrivateOriginalError::Unavailable)?
            .unix_ms;
        let mut memberships = Vec::new();
        for membership in snapshot.waiters_for_source(&row.key) {
            if !snapshot
                .departed(&membership)
                .map_err(|_| PrivateOriginalError::Conflict)?
                && now < membership.deadline_ms()
                && now < snapshot.terms().deadline_ms
            {
                memberships.push(membership);
            }
        }
        // Neither absent membership nor a completed departure can cause source
        // promotion or recreate a local interest. Expiry is not new permission.
        if memberships.is_empty() {
            return Ok(WorkflowInterestRecovery::NotSelected);
        }
        let g_row = self
            .store
            .get(&self.key(planning)?)
            .await
            .map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        if !enter() {
            return Ok(WorkflowInterestRecovery::Stopped);
        }
        let g = self.decode_record(&g_row, planning)?;
        // Every capture-independent canonical fact of the selected lead is
        // checked before its wake: its Lead evidence, cancellation, retained
        // input and association, and the workflow that names it as planning
        // source. A cancelled or mismatched lead is refused, never woken.
        let association = Self::selected_lead_before_wake(&g)?;
        if snapshot.terms().planning_source != g_row.key {
            return Err(PrivateOriginalError::Conflict);
        }
        self.wake_workflow_lead(association, &stamp).await?;
        let member = self
            .owner
            .captured_workflow_member(workflow, association)
            .map_err(writer_error)?;
        if snapshot.terms().lead != member || snapshot.terms().planning_source != g_row.key {
            return Err(PrivateOriginalError::Conflict);
        }
        let Some(PlanningSourceEvidence::Lead { parent }) = &g.planning_source_evidence else {
            return Err(PrivateOriginalError::Unavailable);
        };
        // A is always re-observed through its actual current native owner. The
        // promotion owner suppresses publication only after exact native replay.
        self.restore_workflow_waiter_source(workflow, planning, &member, &original)
            .await?;
        if !enter() {
            return Ok(WorkflowInterestRecovery::Stopped);
        }
        if self.held_workflow_source(planning, association)? {
            self.restore_workflow_parent_source(parent).await?;
        } else {
            // The restricted G restore includes actual P observation first.
            self.restore_workflow_lead_source(workflow, planning, &member)
                .await?;
        }
        if !enter() {
            return Ok(WorkflowInterestRecovery::Stopped);
        }
        // The skip above only avoids the incomplete-controller preparation on
        // an already held G. Every successful step below still observes actual
        // G admission/controller/actor and checks the retained client Arc. A
        // stale client refuses; actor-replacement recovery is not implemented.
        for membership in &memberships {
            if !enter() {
                return Ok(WorkflowInterestRecovery::Stopped);
            }
            self.restore_workflow_interest(workflow, planning, &member, &original, membership)
                .await?;
        }
        Ok(WorkflowInterestRecovery::Restored {
            memberships: memberships.len(),
        })
    }

    /// A restarted workflow lead stays Dormant, with no capture in this
    /// process, until its first registration. Restoring a selected interest
    /// is an operation on it, so wake exactly the lead's session first (no
    /// input, no turn) before reading its capture. A refused wake leaves the
    /// interest unavailable for a later round, as before; a cancelled,
    /// departed or unselected source never reaches this point.
    ///
    /// `stamp` is the step's publication observation, taken before its
    /// canonical reads: a change completed after those reads refuses before
    /// the wake. The caller holds no source writer or publication guard.
    async fn wake_workflow_lead(
        &self,
        association: &InputAuthorityAssociation,
        stamp: &meerkat_authorization::publication::LocalPublicationStamp,
    ) -> Result<(), PrivateOriginalError> {
        let session =
            meerkat_core::SessionId::parse(association.candidate().target.context.as_str())
                .map_err(|_| PrivateOriginalError::Conflict)?;
        let pins = self
            .controller_pin_source
            .get()
            .and_then(Weak::upgrade)
            .ok_or(PrivateOriginalError::Unavailable)?;
        let machine = self.machine()?;
        #[cfg(test)]
        if let Ok(hold) = LEAD_WAKE_SELECTION.try_with(Arc::clone) {
            hold.0.notify_one();
            hold.1.notified().await;
        }
        Self::wake_original_session_owner(&machine, &*pins, &session, stamp).await
    }

    /// The capture-independent canonical facts of an exact selected workflow
    /// lead, checked before anything wakes it: its Lead evidence, its
    /// cancellation, and its retained input with that input's association.
    /// The lead's capture exists only once it registers, so the capture and
    /// every check that needs it still follow the wake.
    fn selected_lead_before_wake(
        lead: &crate::private_originals::SourceRecord,
    ) -> Result<&InputAuthorityAssociation, PrivateOriginalError> {
        let Some(PlanningSourceEvidence::Lead { .. }) = &lead.planning_source_evidence else {
            return Err(PrivateOriginalError::Unavailable);
        };
        if lead.operation_cancelled {
            return Err(PrivateOriginalError::Denied);
        }
        let input = lead
            .bound_service_input
            .as_ref()
            .ok_or(PrivateOriginalError::Unavailable)?
            .decode()?;
        let association = lead
            .final_association
            .as_ref()
            .ok_or(PrivateOriginalError::Unavailable)?;
        if input.header().authority_association.as_ref() != Some(association) {
            return Err(PrivateOriginalError::Conflict);
        }
        Ok(association)
    }

    fn held_workflow_source(
        &self,
        reference: &PrivateOriginalRef,
        association: &InputAuthorityAssociation,
    ) -> Result<bool, PrivateOriginalError> {
        let original = self
            .retained
            .lock()
            .map_err(|_| PrivateOriginalError::Unavailable)?
            .iter()
            .find(|held| &held.reference == reference)
            .map(|held| held.original.clone());
        let Some(original) = original else {
            return Ok(false);
        };
        if original.association() != association {
            return Err(PrivateOriginalError::Conflict);
        }
        self.owner
            .check_bound_source(&original)
            .map_err(writer_error)?;
        self.owner
            .has_admitted_source(association)
            .map_err(writer_error)
    }
}
