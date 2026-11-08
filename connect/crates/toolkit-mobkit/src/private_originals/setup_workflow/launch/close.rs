//! Consume only the recorded final-departure obligation. This source owner
//! fences G and observes its exact input; it does not cancel an ambient run.
use super::*;
use meerkat_runtime::{
    SessionServiceRuntimeExt,
    input_state::verify_exact_replay,
    terminal_status::{InputTerminalReceiptObservation, InteractionSelector},
};
use toolkit_store::{EffectClaimKind, StoredEffect};

const SCHEMA: &str = "toolkit.setup-workflow-close";
const CLOSE_BOUND: std::time::Duration = std::time::Duration::from_secs(30);

/// Historical close evidence, not result delivery, installation retirement or
/// interaction-terminal publication. A pending observation permits no replay.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum WorkflowCloseObservation {
    SourceFencedAndNotAdmitted,
    SourceFencedAndFinalized {
        session_id: String,
        input_id: String,
    },
    Pending,
}

impl InstalledWorkflowPlanning {
    /// The existing host close/reconciliation caller uses this only after
    /// durable final departure. Passive expiry does not manufacture departure.
    /// Dropping this future leaves the same effect claim and source custody for
    /// reconciliation. It never submits G or infers nonadmission from absence.
    pub(crate) async fn observe_close(
        &self,
        reference: &SetupWorkflowRef,
    ) -> Result<WorkflowCloseObservation, PrivateOriginalError> {
        let source = self
            .source
            .upgrade()
            .ok_or(PrivateOriginalError::Unavailable)?;
        let snapshot = source
            .workflow_store()?
            .load_setup_workflow(reference)
            .await
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        if snapshot.terms() != &self.terms
            || !snapshot.sealed()
            || snapshot.active_waiters() != 0
            || self.terms.planning_source != source.key(&self.submission.original)?
        {
            return Err(PrivateOriginalError::Denied);
        }
        let key = reference.close_effect();
        let effect = source
            .store
            .get_effect(&key)
            .await
            .map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        check_intent(&effect, reference)?;
        if effect.status == EffectStatus::Succeeded {
            let Some(EffectOutcome::Succeeded { receipt }) = effect.outcome else {
                return Err(PrivateOriginalError::Conflict);
            };
            let observation = read_receipt(receipt)?;
            return if observation != WorkflowCloseObservation::Pending {
                Ok(observation)
            } else {
                Err(PrivateOriginalError::Conflict)
            };
        }
        let claim = if effect.status == EffectStatus::Pending && effect.attempt == 0 {
            match source
                .store
                .claim_effect(
                    &key,
                    snapshot.revision(),
                    "installed-workflow-close",
                    60_000,
                )
                .await
            {
                Ok(EffectClaimOutcome::Claimed(claim)) => claim,
                Ok(EffectClaimOutcome::NeedsReconciliation(_))
                | Err(StoreError::Claimed | StoreError::Conflict | StoreError::EffectState) => {
                    return Ok(WorkflowCloseObservation::Pending);
                }
                Err(error) => return Err(store_error(error)),
            }
        } else {
            match source
                .store
                .claim_reconciliation(&key, "installed-workflow-close-observe", 60_000)
                .await
            {
                Ok(claim) => claim,
                Err(StoreError::Claimed) => return Ok(WorkflowCloseObservation::Pending),
                Err(error) => return Err(store_error(error)),
            }
        };
        let lease = claim.lease.ok_or(PrivateOriginalError::Unavailable)?;
        // This bounds this read/repair attempt, not the lifetime of G's run.
        // Pending cancellation and admitted native custody remain owned when
        // an await is dropped. A future caller must reconcile this same effect.
        let observation =
            tokio::time::timeout(CLOSE_BOUND, self.close_observation(&source, reference))
                .await
                .unwrap_or(Ok(WorkflowCloseObservation::Pending))?;
        let receipt = RecordData {
            schema: SCHEMA.into(),
            version: 1,
            body: serde_json::to_value(&observation).map_err(|_| PrivateOriginalError::Conflict)?,
        };
        let pending = observation == WorkflowCloseObservation::Pending;
        match lease.kind {
            EffectClaimKind::Dispatch => {
                source
                    .store
                    .acknowledge_effect(
                        &lease,
                        if pending {
                            EffectOutcome::Uncertain { evidence: receipt }
                        } else {
                            EffectOutcome::Succeeded { receipt }
                        },
                    )
                    .await
                    .map_err(store_error)?;
            }
            EffectClaimKind::Reconcile => {
                source
                    .store
                    .reconcile_effect(
                        &lease,
                        if pending {
                            Reconciliation::Pending { evidence: receipt }
                        } else {
                            Reconciliation::Applied { receipt }
                        },
                    )
                    .await
                    .map_err(store_error)?;
            }
        }
        Ok(observation)
    }

    pub(super) async fn close_observation(
        &self,
        source: &Arc<PrivateOriginals>,
        reference: &SetupWorkflowRef,
    ) -> Result<WorkflowCloseObservation, PrivateOriginalError> {
        // Canonical final departure is already committed. Close every local
        // interest under the same lock as LaunchFence::enter, then release it
        // before source persistence or native reads. No new entry is created.
        let entries = {
            let mut interests = self
                .interests
                .lock()
                .map_err(|_| PrivateOriginalError::Unavailable)?;
            let mut entries = Vec::new();
            for interest in interests.iter_mut() {
                if interest.membership.workflow() != reference {
                    return Err(PrivateOriginalError::Conflict);
                }
                interest.closed = true;
                for entry in &interest.entries {
                    entry.close().map_err(custody_error)?;
                    entries.push(entry.clone());
                }
            }
            entries
        };
        let record = source
            .validate_cancellation_source(&self.submission.original, None, None)
            .await
            .map_err(custody_error)?;
        let decoded = source.decode_stored(&record)?;
        let original = meerkat_authorization_contracts::work_association::OriginalWorkRef {
            authority: decoded.source.source_authority,
            work: decoded.event.original,
        };
        // Use the exact G source owner, not task-group or installation cleanup.
        // P and each waiter keep their own source/grants. This fences future
        // G operations but does not interrupt a model request already entered.
        source
            .owner
            .record_operation_cancelled(
                &decoded.source.realm,
                &original,
                |binding, prior| async move {
                    source
                        .validate_cancellation_source(
                            &self.submission.original,
                            binding.as_ref(),
                            prior.as_deref(),
                        )
                        .await
                },
                |record| source.persist_source_cancellation(record),
            )
            .await
            .map_err(custody_error)?;
        // This is actual admission-decision settlement, not runtime terminal.
        crate::original_ingress::ToolkitOriginalOwner::drain_shared_entries(entries)
            .await
            .map_err(custody_error)?;

        let launch = source
            .store
            .get_effect(&reference.launch_effect())
            .await
            .map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        if launch.status == EffectStatus::Failed {
            let Some(EffectOutcome::Failed { evidence }) = launch.outcome else {
                return Err(PrivateOriginalError::Conflict);
            };
            if evidence.schema != "toolkit.setup-workflow-launch" || evidence.version != 1 {
                return Err(PrivateOriginalError::Conflict);
            }
            return match serde_json::from_value::<WorkflowLaunchObservation>(evidence.body)
                .map_err(|_| PrivateOriginalError::Conflict)?
            {
                WorkflowLaunchObservation::NotAdmitted => {
                    Ok(WorkflowCloseObservation::SourceFencedAndNotAdmitted)
                }
                _ => Err(PrivateOriginalError::Conflict),
            };
        }
        let held = source
            .retained
            .lock()
            .map_err(|_| PrivateOriginalError::Unavailable)?
            .iter()
            .find(|held| held.reference == self.submission.original)
            .cloned();
        let Some(held) = held else {
            return Ok(WorkflowCloseObservation::Pending);
        };
        let Some(workflow) = held.setup_workflow.get() else {
            return Ok(WorkflowCloseObservation::Pending);
        };
        if workflow.reference != *reference || !Arc::ptr_eq(&workflow.planning, &held.original) {
            return Err(PrivateOriginalError::Conflict);
        }
        let pending = {
            let inputs = held
                .unresolved
                .lock()
                .map_err(|_| PrivateOriginalError::Unavailable)?;
            if inputs.len() > 1 {
                return Err(PrivateOriginalError::Conflict);
            }
            inputs.first().cloned()
        };
        let (session, input, selected) = if let Some(pending) = pending {
            let (session, input) = pending.workflow_input(workflow)?;
            (session, input, None)
        } else {
            // A restored G has no in-process binder. Its cancelled canonical
            // source selects a historical read, never another native action.
            let Some(row) = source
                .store
                .get(&source.key(&self.submission.original)?)
                .await
                .map_err(store_error)?
            else {
                return Ok(WorkflowCloseObservation::Pending);
            };
            let record = source.decode_record(&row, &self.submission.original)?;
            if record.planning_source_evidence.is_none() || record.final_association.is_none() {
                return Ok(WorkflowCloseObservation::Pending);
            }
            self.check_restored_close_source(source, &held, workflow, &record)?;
            let Some(bound) = &record.bound_service_input else {
                return Ok(WorkflowCloseObservation::Pending);
            };
            (workflow.session.clone(), bound.decode()?, Some(record))
        };
        if session != workflow.session {
            return Err(PrivateOriginalError::Conflict);
        }
        let machine = source.machine()?;
        let observation = async {
            let Ok(Some(stored)) = machine.input_state(&session, input.id()).await else {
                return Ok(WorkflowCloseObservation::Pending);
            };
            if stored.state.input_id != *input.id()
                || verify_exact_replay(&stored.state, &input).is_err()
            {
                return Err(PrivateOriginalError::Conflict);
            }
            // Historical close needs native finalization, never the result payload.
            // The body-free reader preserves this fact after the scoped destination
            // retires. As before, every finalized class qualifies (including failure
            // and CallbackPending); this is not successful result delivery or a
            // closed-cohort proof. Pending and receipt-less/coalesced inputs do not.
            let Ok(Some(terminal)) = machine
                .input_terminal_receipt_observation(
                    &session,
                    InteractionSelector::InputId(input.id().clone()),
                )
                .await
            else {
                return Ok(WorkflowCloseObservation::Pending);
            };
            if terminal.report.input_id() != input.id() {
                return Err(PrivateOriginalError::Conflict);
            }
            match terminal.report {
                InputTerminalReceiptObservation::Finalized(_) => {
                    Ok(WorkflowCloseObservation::SourceFencedAndFinalized {
                        session_id: session.to_string(),
                        input_id: input.id().to_string(),
                    })
                }
                _ => Ok(WorkflowCloseObservation::Pending),
            }
        }
        .await;
        if let Some(selected) = selected {
            // Reobserve this immutable read selection even if native failed.
            // Cancellation is required, but no current operation grant is:
            // revoked/expired permission cannot erase historical completion.
            let Some(row) = source
                .store
                .get(&source.key(&self.submission.original)?)
                .await
                .map_err(store_error)?
            else {
                return Ok(WorkflowCloseObservation::Pending);
            };
            let current = source.decode_record(&row, &self.submission.original)?;
            self.check_restored_close_source(source, &held, workflow, &current)?;
            if current.source != selected.source
                || current.event != selected.event
                || current.final_association != selected.final_association
                || current.bound_service_input != selected.bound_service_input
                || current.planning_source_evidence != selected.planning_source_evidence
                || current.late_workflow_helper != selected.late_workflow_helper
            {
                return Err(PrivateOriginalError::Conflict);
            }
        }
        observation
    }

    fn check_restored_close_source(
        &self,
        source: &Arc<PrivateOriginals>,
        held: &Arc<HeldPrivateOriginal>,
        workflow: &HeldSetupWorkflow,
        record: &SourceRecord,
    ) -> Result<(), PrivateOriginalError> {
        use crate::private_originals::planning_source_evidence::PlanningSourceEvidence;
        source.service_input_host_current()?;
        if !record.operation_cancelled
            || record.planning_source_evidence
                != Some(PlanningSourceEvidence::Lead {
                    parent: self.parent_reference.clone(),
                })
            || record.final_association.as_ref() != Some(held.original.association())
            || held.reference != self.submission.original
            || workflow.member != self.terms.lead
            || workflow.session.to_string()
                != held
                    .original
                    .association()
                    .candidate()
                    .target
                    .context
                    .as_str()
            || !Arc::ptr_eq(&workflow.planning, &held.original)
            || !workflow
                .source
                .upgrade()
                .is_some_and(|owner| Arc::ptr_eq(&owner, source))
            || !self
                .source
                .upgrade()
                .is_some_and(|owner| Arc::ptr_eq(&owner, source))
            || !workflow.accepted.load(Ordering::Acquire)
            || record
                .bound_service_input
                .as_ref()
                .map(|bound| bound.decode())
                .transpose()?
                .is_some_and(|input| {
                    input.header().authority_association.as_ref()
                        != Some(held.original.association())
                })
        {
            return Err(PrivateOriginalError::Conflict);
        }
        let retained = source
            .retained
            .lock()
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        if !retained.iter().any(|current| Arc::ptr_eq(current, held)) {
            return Err(PrivateOriginalError::Conflict);
        }
        let parent = retained
            .iter()
            .find(|current| current.reference == self.parent_reference)
            .ok_or(PrivateOriginalError::Unavailable)?;
        if !Arc::ptr_eq(&parent.original, &self.parent)
            || !parent
                .workflow_plans
                .lock()
                .map_err(|_| PrivateOriginalError::Unavailable)?
                .iter()
                .any(|plan| std::ptr::eq(self, plan.as_ref()))
        {
            return Err(PrivateOriginalError::Conflict);
        }
        Ok(())
    }
}

fn check_intent(
    effect: &StoredEffect,
    reference: &SetupWorkflowRef,
) -> Result<(), PrivateOriginalError> {
    if effect.key != reference.close_effect()
        || effect.intent.id != "close"
        || effect.intent.kind != "setup_workflow_close"
        || effect.intent.request.schema != "meerkat_toolkit.setup_workflow.close"
        || effect.intent.request.version != 1
        || effect.intent.request.body
            != serde_json::json!({"workflow":reference,"payload":reference})
    {
        return Err(PrivateOriginalError::Conflict);
    }
    Ok(())
}

fn read_receipt(receipt: RecordData) -> Result<WorkflowCloseObservation, PrivateOriginalError> {
    if receipt.schema != SCHEMA || receipt.version != 1 {
        return Err(PrivateOriginalError::Conflict);
    }
    serde_json::from_value(receipt.body).map_err(|_| PrivateOriginalError::Conflict)
}

impl PrivateOriginals {
    /// The outer cancellation operation has already fenced and persisted the
    /// actual source set, including approved-task descendants. No source lock
    /// is retained across these reads. Conditional join commits make its census
    /// complete even when an earlier join reply/interest publication was lost.
    pub(in crate::private_originals) async fn depart_cancelled_workflows(
        &self,
        cancelled: &[Arc<StoredRecord>],
    ) -> Result<PrivateOriginalCancellation, PrivateOriginalError> {
        let selected = {
            let retained = self
                .retained
                .lock()
                .map_err(|_| PrivateOriginalError::Unavailable)?;
            let mut selected: Vec<(toolkit_store::StoreKey, Arc<InstalledWorkflowPlanning>)> =
                Vec::new();
            for row in cancelled {
                let decoded = self.decode_stored(row)?;
                if !decoded.operation_cancelled {
                    return Err(PrivateOriginalError::Conflict);
                }
                for held in retained
                    .iter()
                    .filter(|held| self.key(&held.reference).ok().as_ref() == Some(&row.key))
                {
                    let choices = held
                        .workflow_selections
                        .lock()
                        .map_err(|_| PrivateOriginalError::Unavailable)?;
                    for choice in choices.iter() {
                        if let Some(planning) = &choice.selection
                            && !selected
                                .iter()
                                .any(|(key, prior)| key == &row.key && Arc::ptr_eq(prior, planning))
                        {
                            selected.push((row.key.clone(), planning.clone()));
                        }
                    }
                }
            }
            selected
        };
        let mut outcome = PrivateOriginalCancellation::OperationsCancelled;
        for (source_key, planning) in selected {
            let store = self.workflow_store()?;
            let reference = store
                .workflow_ref(&planning.terms)
                .map_err(|_| PrivateOriginalError::Conflict)?;
            let snapshot = match store.load_setup_workflow(&reference).await {
                Ok(snapshot) => snapshot,
                Err(meerkat_toolkit::setup::SetupError::Store(StoreError::NotFound)) => continue,
                Err(_) => return Err(PrivateOriginalError::Unavailable),
            };
            for membership in snapshot.waiters_for_source(&source_key) {
                let departed = planning.depart_waiter(&membership).await?;
                if departed.sealed() && departed.active_waiters() == 0 {
                    // Source cancellation and this departure are durable. A
                    // missing receipt or failed close read/ack is cleanup still
                    // pending, not failure of the completed cancellation. The
                    // same close effect remains for the joined recovery owner.
                    if !matches!(
                        planning.observe_close(&reference).await,
                        Ok(WorkflowCloseObservation::SourceFencedAndNotAdmitted
                            | WorkflowCloseObservation::SourceFencedAndFinalized { .. })
                    ) {
                        outcome = PrivateOriginalCancellation::OperationsCancelledCleanupPending;
                    }
                }
            }
        }
        Ok(outcome)
    }
}

/// Private canonical source metadata for one existing recovery-worker page.
/// Rows and cursor are observations, not reconstructed workflow authority.
pub(crate) struct WorkflowCloseRecoveryPage {
    pub(crate) sources: Vec<StoredRecord>,
    pub(crate) next_after_id: Option<String>,
}

impl PrivateOriginals {
    pub(crate) async fn workflow_close_recovery_page(
        &self,
        after: Option<&str>,
        limit: u32,
    ) -> Result<WorkflowCloseRecoveryPage, PrivateOriginalError> {
        if !(1..=128).contains(&limit) {
            return Err(PrivateOriginalError::Denied);
        }
        self.service_input_host_current()?;
        let sources = self
            .store
            .list_records(&self.scope, after, limit)
            .await
            .map_err(store_error)?;
        self.service_input_host_current()?;
        let next_after_id = if sources.len() == limit as usize {
            sources.last().map(|row| row.key.id.clone())
        } else {
            None
        };
        Ok(WorkflowCloseRecoveryPage {
            sources,
            next_after_id,
        })
    }

    /// Reconcile only an already cancelled canonical source. Settled P cleanup
    /// uses the same cancellation owner before workflow departure. Missing live
    /// planning handles are never rebuilt from stored labels, and this path never
    /// launches G or interprets source cancellation as native completion.
    pub(crate) async fn reconcile_cancelled_workflow_source(
        &self,
        observed: &StoredRecord,
    ) -> Result<PrivateOriginalCancellation, PrivateOriginalError> {
        if observed.key.realm != self.scope.realm || observed.key.owner != self.scope.owner {
            return Err(PrivateOriginalError::Denied);
        }
        self.service_input_host_current()?;
        if observed.data.schema != SOURCE_SCHEMA {
            return Ok(PrivateOriginalCancellation::OperationsCancelled);
        }
        let current = self
            .store
            .get(&observed.key)
            .await
            .map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        self.service_input_host_current()?;
        let current = Arc::new(current);
        let record = self.decode_stored(&current)?;
        if !record.operation_cancelled {
            return Ok(PrivateOriginalCancellation::OperationsCancelled);
        }
        if !record.shared_preparations.is_empty() {
            let reference = PrivateOriginalRef {
                route: record.event.route.clone(),
                original: record.event.original.clone(),
                event_sha256: digest(
                    &canonical_json(&record.event).map_err(|_| PrivateOriginalError::Conflict)?,
                ),
            };
            // This revalidates the canonical group and retains its actual
            // Cancelled owner before resolving physical evidence. Unavailable
            // leaves the existing worker sweep delayed, never acknowledged.
            let cancelled = self
                .record_cancelled_source_set(&reference, None, None, None)
                .await?;
            return self.depart_cancelled_workflows(&cancelled).await;
        }
        self.depart_cancelled_workflows(&[current]).await
    }
}

/// Cleanup of one terminal acquisition. This never acknowledges its Original
/// continuation, waiter context delivery, installation or native run cleanup.
pub(crate) enum WorkflowTerminalRecovery {
    NotSelected,
    Departed,
    ContinuationPending,
    CleanupPending,
}

impl PrivateOriginals {
    /// Called only by the existing joined host recovery owner. Page metadata is
    /// not a terminality witness. Select through the existing retained workflow
    /// choice before reopening the canonical result and immutable request.
    pub(crate) async fn reconcile_terminal_workflow_acquisition(
        &self,
        candidate: &meerkat_toolkit::setup::ContinuationRecoveryCandidate,
    ) -> Result<WorkflowTerminalRecovery, PrivateOriginalError> {
        self.reconcile_terminal_workflow_candidate(candidate, self.store.as_ref())
            .await
    }

    #[cfg(test)]
    pub(super) async fn terminal_workflow_with_read_probe(
        &self,
        candidate: &meerkat_toolkit::setup::ContinuationRecoveryCandidate,
        store: &dyn toolkit_store::ToolkitStore,
    ) -> Result<WorkflowTerminalRecovery, PrivateOriginalError> {
        self.reconcile_terminal_workflow_candidate(candidate, store)
            .await
    }

    async fn reconcile_terminal_workflow_candidate(
        &self,
        candidate: &meerkat_toolkit::setup::ContinuationRecoveryCandidate,
        read_store: &dyn toolkit_store::ToolkitStore,
    ) -> Result<WorkflowTerminalRecovery, PrivateOriginalError> {
        self.service_input_host_current()?;
        let selected = {
            let retained = self
                .retained
                .lock()
                .map_err(|_| PrivateOriginalError::Unavailable)?;
            match retained
                .iter()
                .find(|held| held.original.association() == candidate.association())
            {
                None => false,
                Some(held) => {
                    let choices = held
                        .workflow_selections
                        .lock()
                        .map_err(|_| PrivateOriginalError::Unavailable)?;
                    choices.iter().any(|choice| {
                        choice.correlation == *candidate.correlation() && choice.selection.is_some()
                    })
                }
            }
        };
        // Absence is only a live-owner selection result, not proof that a
        // durable obligation is closed. Ordinary historical rows need no
        // canonical terminal/task reads and do not cause unavailable backoff.
        if !selected {
            return Ok(WorkflowTerminalRecovery::NotSelected);
        }
        let operation = candidate.operation();
        let store = SetupStore::new(
            read_store,
            SetupScope::new(
                Id::new(self.scope.realm.clone()).map_err(|_| PrivateOriginalError::Conflict)?,
                Id::new(self.scope.owner.clone()).map_err(|_| PrivateOriginalError::Conflict)?,
            ),
        )
        .map_err(|_| PrivateOriginalError::Unavailable)?;
        let result = store
            .committed_continuation(operation)
            .await
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        self.service_input_host_current()?;
        let Some(result) = result else {
            return Ok(WorkflowTerminalRecovery::NotSelected);
        };
        let Some(task) = result.continuation().native_original.as_ref() else {
            return Ok(WorkflowTerminalRecovery::NotSelected);
        };
        // Reuse the source owner's exact native-task read port. No expired
        // setup token is reused, and no request content leaves this owner.
        let acquisition = store
            .service_task_snapshot(operation, task)
            .await
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        self.service_input_host_current()?;
        if acquisition.state().terminal_revision == 0
            || acquisition.state().terminal_revision != result.terminal_revision()
            || acquisition.request().original_work() != result.original_work()
            || acquisition.request().evidence().association() != candidate.association()
            || acquisition.request().host().correlation != *candidate.correlation()
        {
            return Err(PrivateOriginalError::Conflict);
        }
        let (original, planning) = {
            let retained = self
                .retained
                .lock()
                .map_err(|_| PrivateOriginalError::Unavailable)?;
            let held = retained
                .iter()
                .find(|held| {
                    held.original.association() == acquisition.request().evidence().association()
                })
                .ok_or(PrivateOriginalError::Unavailable)?;
            let choices = held
                .workflow_selections
                .lock()
                .map_err(|_| PrivateOriginalError::Unavailable)?;
            let planning = choices
                .iter()
                .find(|choice| choice.correlation == acquisition.request().host().correlation)
                .and_then(|choice| choice.selection.clone());
            (held.clone(), planning)
        };
        let Some(planning) = planning else {
            return Ok(WorkflowTerminalRecovery::NotSelected);
        };
        let owner = planning
            .source
            .upgrade()
            .ok_or(PrivateOriginalError::Unavailable)?;
        if !std::ptr::eq(self, owner.as_ref()) {
            return Err(PrivateOriginalError::Denied);
        }
        let reference = store
            .workflow_ref(planning.terms())
            .map_err(|_| PrivateOriginalError::Conflict)?;
        let workflow = match store.load_setup_workflow(&reference).await {
            Ok(workflow) => workflow,
            Err(meerkat_toolkit::setup::SetupError::Store(StoreError::NotFound)) => {
                return Ok(WorkflowTerminalRecovery::NotSelected);
            }
            Err(_) => return Err(PrivateOriginalError::Unavailable),
        };
        self.service_input_host_current()?;
        let source_key = self.key(&original.reference)?;
        let expected = store
            .workflow_waiter(planning.terms(), &acquisition, source_key.clone())
            .map_err(|_| PrivateOriginalError::Conflict)?;
        // A can own several independent acquisitions. Terminality of one must
        // not retire every waiter for the same Original, including late B/C.
        let Some(membership) = workflow
            .waiters_for_source(&source_key)
            .into_iter()
            .find(|waiter| waiter.acquisition() == acquisition.key())
        else {
            return Ok(WorkflowTerminalRecovery::NotSelected);
        };
        if membership != expected {
            return Err(PrivateOriginalError::Conflict);
        }
        let attached = {
            let retained = self
                .retained
                .lock()
                .map_err(|_| PrivateOriginalError::Unavailable)?;
            retained
                .iter()
                .find(|held| held.reference == planning.submission.original)
                .and_then(|held| held.setup_workflow.get().cloned())
        };
        // Serialize against retain_waiter's phase read and relationship insert.
        // Retire only exact A membership calls under the common publication.
        // Entered custody/receipts stay owned; an interrupted departure cannot
        // reopen these calls. A's source and its legitimate result stay live.
        self.owner
            .retire_workflow_membership(original.original.association(), || {
                let Some(attached) = attached else {
                    return Ok(Vec::new());
                };
                if attached.reference != reference {
                    return Err(OriginalWriterError::Conflict);
                }
                let waiters = attached
                    .waiters
                    .lock()
                    .map_err(|_| OriginalWriterError::Unavailable)?;
                Ok(waiters
                    .iter()
                    .filter(|waiter| waiter.membership == membership)
                    .map(|waiter| waiter.authority.clone())
                    .collect())
            })
            .await
            .map_err(custody_error)?;
        self.service_input_host_current()?;
        // Terminal setup retires its call authority immediately. An admitted
        // approved-service continuation still owns cohort membership until its
        // exact native input finalizes; admission alone is not task completion.
        if !workflow
            .departed(&membership)
            .map_err(|_| PrivateOriginalError::Conflict)?
            && matches!(
                result.outcome(),
                meerkat_toolkit::continuation::SetupTerminalOutcome::ApprovedServiceAdmitted { .. }
            )
        {
            let finalized = self
                .approved_service_continuation_finalized(&acquisition, &result)
                .await?;
            self.service_input_host_current()?;
            if !finalized {
                return Ok(WorkflowTerminalRecovery::ContinuationPending);
            }
        }
        let departed = planning.depart_waiter(&membership).await?;
        if departed.sealed() && departed.active_waiters() == 0 {
            self.service_input_host_current()?;
            if !matches!(
                planning.observe_close(&reference).await,
                Ok(WorkflowCloseObservation::SourceFencedAndNotAdmitted
                    | WorkflowCloseObservation::SourceFencedAndFinalized { .. })
            ) {
                return Ok(WorkflowTerminalRecovery::CleanupPending);
            }
        }
        Ok(WorkflowTerminalRecovery::Departed)
    }
}
