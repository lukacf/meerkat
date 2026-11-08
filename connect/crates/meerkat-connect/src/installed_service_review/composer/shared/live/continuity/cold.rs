//! Contract-bound R5 consumer draft. The selected ce43/Meta52 tuple does not
//! implement the called method or completed_owner_loss observer. Integrate only
//! after exact upstream source and both erased-service forwarders are reviewed.
//! Successful closed restoration remains a separate unchanged path.
use super::*;
use meerkat_core::{session_hosting::HostingClaim, time_compat::Instant};
use meerkat_runtime::{
    ColdPredecessorSettlement, PredecessorOwnerLoss,
    completion::CompletionOutcome,
    terminal_status::{
        InputTerminalReceipt, InputTerminalReceiptRead, InputTerminalReceiptScope,
        InputTerminalReceiptSummary,
    },
};
use meerkat_session::ColdPredecessorSettleError;
use serde_json::{Value, json};

pub(crate) struct ColdRecovery {
    expected: MemberClassAdmission,
    identity: meerkat_mobkit::identity_first::AgentIdentity,
    attempt: tokio::sync::Mutex<Option<tokio::task::JoinHandle<Result<(), HostLifetimeError>>>>,
    state: tokio::sync::Mutex<ColdState>,
}
#[derive(Default)]
struct ColdState {
    // Neither field is owned by RetireNativeContext. Native calls borrow a
    // clone of this same lineage; a callback error never replaces the claim.
    claim: Option<HostingClaim>,
    physical: Option<Arc<crate::resources::ColdPhysicalNoEntry>>,
    native: Vec<Arc<Result<ColdPredecessorSettlement, ColdPredecessorSettleError>>>,
    // The durable one-shot adoption fence exists. This is not evidence that
    // a physical child entered or completed its preparation.
    adoption_claimed: bool,
    result: Option<Result<(), HostLifetimeError>>,
}
impl ColdState {
    // This is the sole R5 call boundary. Keep the actual typed result/error and
    // the original claim even when a disposable observation returns refusal.
    async fn observe_native(
        &mut self,
        native: &NativeContext,
        session: &SessionId,
        deadline: Instant,
    ) -> Result<Arc<Result<ColdPredecessorSettlement, ColdPredecessorSettleError>>, HostLifetimeError>
    {
        if self.native.len() >= 32 {
            return Err(HostLifetimeError::Denied);
        }
        let claim = self.claim.as_ref().ok_or(HostLifetimeError::NotSettled)?;
        let observed = Arc::new(
            native
                .service
                .settle_cold_predecessor_with_mob_lifecycle_authority_before(
                    session, claim, deadline,
                )
                .await,
        );
        self.native.push(observed.clone());
        Ok(observed)
    }
}
async fn join_native_attempt(
    slot: &mut Option<tokio::task::JoinHandle<Result<(), HostLifetimeError>>>,
) -> Result<(), HostLifetimeError> {
    let result = match slot.as_mut().ok_or_else(unavailable)?.await {
        Ok(result) => result,
        Err(_) => Err(HostLifetimeError::NotSettled),
    };
    slot.take();
    result
}

impl ColdRecovery {
    async fn observe(
        self: &Arc<Self>,
        live: Arc<LiveSharedSetup>,
        admitted: Arc<AdmittedMemberClass>,
    ) -> Result<(), HostLifetimeError> {
        let mut task = self.attempt.lock().await;
        if task.is_none() {
            if live.closed.load(Ordering::Acquire) {
                return Err(HostLifetimeError::Denied);
            }
            let retained = self.clone();
            *task = Some(tokio::spawn(async move {
                live.cold_attempt(admitted, retained).await
            }));
        }
        // Observer cancellation leaves the handle in its retained owner. A
        // following observation joins it; only a later explicit invocation can
        // begin another native observation after the prior result was returned.
        join_native_attempt(&mut task).await
    }
    async fn join(&self) -> Result<(), HostLifetimeError> {
        let mut task = self.attempt.lock().await;
        if task.is_some() {
            return join_native_attempt(&mut task).await;
        }
        self.state
            .lock()
            .await
            .result
            .unwrap_or(Err(HostLifetimeError::NotSettled))
    }
}

impl LiveSharedSetup {
    pub(crate) fn bind_abandoned_recovery(
        &self,
        selected: Option<crate::application_consent::AbandonedWorkflowRecovery>,
    ) -> Result<(), HostLifetimeError> {
        if selected
            .as_ref()
            .is_some_and(|_| self.consent_owner.get().and_then(Option::as_ref).is_none())
        {
            return Err(HostLifetimeError::NotSettled);
        }
        // Only trusted intent is frozen here. Prepared Driver has not bound
        // its canonical service policy owner yet, so capacity is checked at
        // restore_abandoned with the actual admission, never guessed as zero.
        let native = self.native.lock().map_err(|_| unavailable())?;
        if self.closed.load(Ordering::Acquire) || !matches!(&*native, NativeCapture::Uncaptured) {
            return Err(HostLifetimeError::Denied);
        }
        self.abandoned
            .set(selected)
            .map_err(|_| HostLifetimeError::Denied)
    }

    pub(super) async fn restore_abandoned(
        self: &Arc<Self>,
        admitted: Arc<AdmittedMemberClass>,
        current: MemberClassAdmission,
        identity: meerkat_mobkit::identity_first::AgentIdentity,
    ) -> Result<(), HostLifetimeError> {
        let recovery = {
            let mut held = self.cold_recoveries.lock().map_err(|_| unavailable())?;
            if self.closed.load(Ordering::Acquire) {
                return Err(HostLifetimeError::Denied);
            }
            if let Some(recovery) = held.get(admitted.admission_id()) {
                if recovery.expected != current || recovery.identity.as_str() != identity.as_str() {
                    return Err(HostLifetimeError::WrongOwner);
                }
                recovery.clone()
            } else {
                if held.len() >= self.policy.member_class_registration_capacity()? {
                    return Err(HostLifetimeError::Denied);
                }
                let recovery = Arc::new(ColdRecovery {
                    expected: current,
                    identity,
                    attempt: tokio::sync::Mutex::new(None),
                    state: tokio::sync::Mutex::new(ColdState::default()),
                });
                held.insert(admitted.admission_id().clone(), recovery.clone());
                recovery
            }
        };
        recovery.observe(self.clone(), admitted).await
    }

    pub(in super::super) async fn join_cold_recoveries(&self) -> Result<(), HostLifetimeError> {
        let selected: Vec<_> = self
            .cold_recoveries
            .lock()
            .map_err(|_| unavailable())?
            .values()
            .cloned()
            .collect();
        let mut failure = None;
        for recovery in selected {
            if let Err(error) = recovery.join().await {
                failure = Some(error);
            }
        }
        // Keep claims and the full native history in their owners. This does
        // not clear them on an error or attest native/physical release.
        failure.map_or(Ok(()), Err)
    }

    async fn cold_attempt(
        self: Arc<Self>,
        admitted: Arc<AdmittedMemberClass>,
        recovery: Arc<ColdRecovery>,
    ) -> Result<(), HostLifetimeError> {
        let mut state = recovery.state.lock().await;
        if let Some(result) = state.result {
            return result;
        }
        if state.adoption_claimed {
            return Err(HostLifetimeError::NotSettled);
        }
        let expected = &recovery.expected;
        self.cold_current(&admitted, expected)?;
        let retained = RetainedAssociation::selected(expected, &self.realm)?;
        let document = &retained.document;
        if document.closed.is_some() || active_session(expected)? != document.session {
            return Err(HostLifetimeError::InvalidSelection);
        }
        let selection = self
            .abandoned
            .get()
            .and_then(Option::as_ref)
            .ok_or(HostLifetimeError::NotSettled)?;
        if !selection.selects(&retained.selection) {
            return Err(HostLifetimeError::WrongOwner);
        }
        // Ordinary invocation custody is a different owner from persistent
        // scoped consent. A missing closed-presenter record is Unknown, even
        // when R5 and the scoped issuer could otherwise settle their work.
        let local = crate::approval::ClosedLocalApprovalsArtifact::read_for_association(
            &retained.selection,
            &document.approval_source,
            &document.session,
        )
        .map_err(|_| HostLifetimeError::NotSettled)?;
        let old_peer = document
            .projected_peer
            .as_ref()
            .ok_or(HostLifetimeError::NotSettled)?;
        let old_physical = document
            .physical
            .as_ref()
            .ok_or(HostLifetimeError::NotSettled)?;
        self.check_consent_provenance(document)?;
        let owner = self
            .consent_owner
            .get()
            .and_then(Option::as_ref)
            .ok_or(HostLifetimeError::NotSettled)?;
        let native = self.native_context()?;
        let machine = native.machine()?;
        if machine
            .current_session_registration_witness(&document.session)
            .await
            .is_some()
        {
            return Err(HostLifetimeError::NotSettled);
        }
        if state.claim.is_none() {
            state.claim = Some(
                machine
                    .grant_session_hosting(&document.session)
                    .map_err(|_| HostLifetimeError::NotSettled)?,
            );
        }
        let claim = state.claim.as_ref().ok_or_else(unavailable)?.clone();
        let association =
            RetainedAssociation::bind_consent(expected, &self.realm, owner, machine, &claim)?;
        if state.physical.is_none() {
            state.physical = Some(Arc::new(
                old_physical
                    .seal_no_entry()
                    .map_err(|_| HostLifetimeError::NotSettled)?,
            ));
        }
        let physical = state.physical.as_ref().ok_or_else(unavailable)?.clone();
        physical
            .recheck()
            .map_err(|_| HostLifetimeError::NotSettled)?;
        if physical.selection() != old_physical {
            return Err(HostLifetimeError::WrongOwner);
        }
        physical_member_scope(
            physical.census(),
            &expected.instance.member.member,
            &document.session,
        )?;
        if !machine.has_consent_owner_loss_settlement() {
            return Err(HostLifetimeError::NotSettled);
        }
        let deadline = Instant::now()
            .checked_add(selection.native_budget())
            .ok_or_else(invalid)?;
        // The actual retained erased service owns this operation. No new
        // service, host-authored registration or reservation completion exists.
        #[cfg(test)]
        same_g::before_native(
            self.as_ref(),
            &native,
            &retained,
            &claim,
            physical.as_ref(),
            &local,
        )
        .await?;
        #[cfg(test)]
        historical_g::before_native(
            self.as_ref(),
            &native,
            &retained,
            &claim,
            physical.as_ref(),
            &local,
        )
        .await?;
        #[cfg(test)]
        current_g::before_native(
            self.as_ref(),
            &native,
            &retained,
            &claim,
            physical.as_ref(),
            &local,
        )
        .await?;
        let observed = state
            .observe_native(&native, &document.session, deadline)
            .await?;
        let result = observed
            .as_ref()
            .as_ref()
            .map_err(|_| HostLifetimeError::NotSettled)?;
        let proof = match &result.owner_loss {
            Some(PredecessorOwnerLoss::Completed(proof) | PredecessorOwnerLoss::NotOwed(proof)) => {
                proof
            }
            _ => return Err(HostLifetimeError::NotSettled),
        };
        if result.session_id != document.session
            || !result.terminals_owed.is_empty()
            || !result.migrated_without_receipt.is_empty()
            || claim.completed_owner_loss().as_ref() != Some(proof)
        {
            return Err(HostLifetimeError::NotSettled);
        }
        let (ids, observations) = cold_native_census(machine, document, result).await?;
        let consent = owner
            .observe_abandoned_after_native(&association, &claim, proof, &ids)
            .map_err(consent_error)?;
        if consent.association != association {
            return Err(HostLifetimeError::WrongOwner);
        }
        self.cold_current(&admitted, expected)?;
        self.check_consent_provenance(document)?;
        physical
            .recheck()
            .map_err(|_| HostLifetimeError::NotSettled)?;
        #[cfg(test)]
        same_g::repeat_native(
            self.as_ref(),
            &mut state,
            &native,
            &retained,
            &association,
            result,
            &observations,
            &consent.records,
            deadline,
        )
        .await?;
        #[cfg(test)]
        historical_g::repeat_native(
            self.as_ref(),
            &mut state,
            &native,
            &retained,
            &association,
            result,
            &observations,
            &consent.records,
            deadline,
        )
        .await?;
        #[cfg(test)]
        current_g::repeat_native(
            self.as_ref(),
            &mut state,
            &native,
            &retained,
            &association,
            result,
            &observations,
            &consent.records,
            deadline,
        )
        .await?;
        let aggregate = retained_abandoned_aggregate(
            document,
            &retained.selection,
            result,
            proof,
            &observations,
            &consent.records,
            physical.census(),
            &local,
        )?;
        // Persist before the first source mutation/physical preparation. A
        // cross-process retry cannot restart an entered or uncertain attempt.
        claim_adoption(&retained.selection, &aggregate)?;
        state.adoption_claimed = true;
        #[cfg(test)]
        same_g::one_shot(&retained.selection, &aggregate)?;
        #[cfg(test)]
        historical_g::one_shot(&retained.selection, &aggregate)?;
        #[cfg(test)]
        current_g::one_shot(&retained.selection, &aggregate)?;
        let result = self
            .publish_abandoned(
                admitted,
                expected,
                &recovery.identity,
                &retained,
                &claim,
                result,
                physical,
                local,
                aggregate,
                old_peer.clone(),
            )
            .await;
        state.result = Some(result);
        result
    }

    pub(in super::super) fn retain_closed_presenter(
        &self,
        entry: &LiveEntry,
    ) -> Result<(), HostLifetimeError> {
        let Some(retained) = entry.custody.association.get() else {
            return Ok(());
        };
        read_selected(&retained.selection)?;
        let document = &retained.document;
        let Some(closed) = entry
            .approvals
            .closed_observation(&document.session)
            .map_err(|_| unavailable())?
        else {
            return Ok(());
        };
        let (source, session) = closed.source();
        if source != document.approval_source || session != &document.session {
            return Err(HostLifetimeError::WrongOwner);
        }
        closed
            .retain_for_association(&retained.selection)
            .map_err(|_| unavailable())?;
        Ok(())
    }

    fn cold_current(
        &self,
        admitted: &Arc<AdmittedMemberClass>,
        expected: &MemberClassAdmission,
    ) -> Result<(), HostLifetimeError> {
        if self.closed.load(Ordering::Acquire)
            || self.policy.current_member_class(admitted)? != *expected
        {
            return Err(HostLifetimeError::Denied);
        }
        Ok(())
    }
}

fn physical_member_scope(
    census: &Value,
    member: &meerkat_core::MobMemberBinding,
    session: &SessionId,
) -> Result<(), HostLifetimeError> {
    let members = census
        .get("members")
        .and_then(Value::as_array)
        .ok_or_else(invalid)?;
    let expected = serde_json::to_value(member).map_err(|_| invalid())?;
    let session = session.to_string();
    if members.is_empty()
        || members.iter().any(|row| {
            row.get("member") != Some(&expected)
                || row.get("session").and_then(Value::as_str) != Some(session.as_str())
        })
    {
        return Err(HostLifetimeError::WrongOwner);
    }
    Ok(())
}

async fn cold_native_census(
    machine: &meerkat_runtime::MeerkatMachine,
    document: &ResourceAssociation,
    result: &ColdPredecessorSettlement,
) -> Result<(HashSet<InputId>, Vec<Value>), HostLifetimeError> {
    cold_session_census(machine, &document.session, &document.baseline, result).await
}

// Same production census, with identity/baseline arguments selected by the
// checked association above. The native fixture also uses this exact reader.
async fn cold_session_census(
    machine: &meerkat_runtime::MeerkatMachine,
    session: &SessionId,
    baseline: &HashSet<InputId>,
    result: &ColdPredecessorSettlement,
) -> Result<(HashSet<InputId>, Vec<Value>), HostLifetimeError> {
    if machine
        .current_session_registration_witness(session)
        .await
        .is_some()
    {
        return Err(HostLifetimeError::NotSettled);
    }
    let observed = machine
        .session_input_states(session)
        .await
        .map_err(|_| unavailable())?;
    if observed.source != TerminalWitnessSource::DurableStore
        || u64::try_from(observed.report.len()).ok() != Some(result.durable_rows_closed)
    {
        return Err(HostLifetimeError::NotSettled);
    }
    let count = observed.report.len();
    let ids: HashSet<_> = observed
        .report
        .iter()
        .map(|row| row.state.input_id.clone())
        .collect();
    if ids.len() != count || !baseline.is_subset(&ids) {
        return Err(invalid());
    }
    let mut reports = Vec::new();
    for input in ordered_input_ids(&ids) {
        let observed = machine
            .input_terminal_receipt_observation(
                session,
                InteractionSelector::InputId(input.clone()),
            )
            .await
            .map_err(|_| unavailable())?
            .ok_or_else(unavailable)?;
        if observed.source != TerminalWitnessSource::DurableStore
            || observed.report.input_id() != input
        {
            return Err(HostLifetimeError::WrongOwner);
        }
        let value = match &observed.report {
            InputTerminalReceiptObservation::Finalized(receipt)
                if receipt
                    .recipient_input_ids()
                    .iter()
                    .all(|id| ids.contains(id)) =>
            {
                let outcome_unknown =
                    unknown_receipt_attribution(machine, session, receipt).await?;
                json!({"input_id":input,"kind":"finalized","terminal":receipt.terminal(),
                        "attempt_count":receipt.attempt_count(),"scope":retained_receipt_scope(receipt.scope())?,"run_id":receipt.run_id(),
                        "owner_input_id":receipt.owner_input_id(),"recipients":receipt.recipient_input_ids(),
                        "completion_class":format!("{:?}",receipt.class()),"outcome_unknown":outcome_unknown})
            }
            // R5's complete closing census is authoritative for native
            // receipt-less transitions; migrated rows were refused above.
            InputTerminalReceiptObservation::TerminalWithoutReceipt {
                terminal,
                last_run_id,
                attempt_count,
                ..
            } => {
                json!({"input_id":input,"kind":"native_terminal_without_receipt","terminal":terminal,
                    "last_run_id":last_run_id,"attempt_count":attempt_count})
            }
            _ => return Err(HostLifetimeError::NotSettled),
        };
        reports.push(value);
    }
    let mut returned = HashSet::new();
    for input in &result.inputs {
        if !ids.contains(&input.input_id)
            || !returned.insert(input.input_id.clone())
            || input.receipt.input_id() != &input.input_id
            || input.interaction_id.is_some() != input.published_terminal_seq.is_some()
            || input
                .receipt
                .recipient_input_ids()
                .iter()
                .any(|id| !ids.contains(id))
        {
            return Err(invalid());
        }
    }
    if machine
        .current_session_registration_witness(session)
        .await
        .is_some()
    {
        return Err(HostLifetimeError::NotSettled);
    }
    // result.inputs is convergence work, never the complete historical census.
    // A repeat with inputs=[] and durable_rows_closed>0 remains valid.
    Ok((ids, reports))
}

// This private artifact projection records the actual typed native scope.
// A runless batch does not imply OutcomeUnknown: that outcome's historical
// run remains separately attributed below. Future native scopes must refuse.
pub(super) fn retained_receipt_scope(
    scope: &InputTerminalReceiptScope,
) -> Result<Value, HostLifetimeError> {
    match scope {
        InputTerminalReceiptScope::Run { run_id } => Ok(json!({"type":"run","run_id":run_id})),
        InputTerminalReceiptScope::RuntimeTermination => Ok(json!({"type":"runtime_termination"})),
        _ => Err(invalid()),
    }
}

// OutcomeUnknown is body-free but its historical run lives inside the
// outcome, not the runless receipt scope. Only that class uses the payload
// reader; successful results/callbacks remain undisclosed summary reads.
async fn unknown_receipt_attribution(
    machine: &meerkat_runtime::MeerkatMachine,
    session: &SessionId,
    summary: &InputTerminalReceiptSummary,
) -> Result<Option<Value>, HostLifetimeError> {
    if summary.class() != CompletionClass::OutcomeUnknown {
        return Ok(None);
    }
    let actual = machine
        .input_terminal_receipt(
            session,
            InteractionSelector::InputId(summary.input_id().clone()),
        )
        .await
        .map_err(|_| unavailable())?
        .ok_or_else(unavailable)?;
    if actual.source != TerminalWitnessSource::DurableStore {
        return Err(HostLifetimeError::NotSettled);
    }
    let InputTerminalReceiptRead::Finalized(receipt) = actual.report else {
        return Err(HostLifetimeError::NotSettled);
    };
    if receipt.input_id() != summary.input_id()
        || receipt.terminal() != summary.terminal()
        || receipt.attempt_count() != summary.attempt_count()
        || receipt.scope() != summary.scope()
        || receipt.run_id() != summary.run_id()
        || receipt.owner_input_id() != summary.owner_input_id()
        || receipt.recipient_input_ids() != summary.recipient_input_ids()
        || receipt.outcome().class() != summary.class()
    {
        return Err(HostLifetimeError::WrongOwner);
    }
    unknown_outcome(&receipt)
}
fn unknown_outcome(receipt: &InputTerminalReceipt) -> Result<Option<Value>, HostLifetimeError> {
    match receipt.outcome() {
        CompletionOutcome::OutcomeUnknown { run_id } => {
            if receipt.scope() != &InputTerminalReceiptScope::RuntimeTermination
                || receipt.run_id().is_some()
            {
                return Err(invalid());
            }
            Ok(Some(json!({"type":"outcome_unknown","run_id":run_id})))
        }
        _ if receipt.outcome().class() == CompletionClass::OutcomeUnknown => Err(invalid()),
        _ => Ok(None),
    }
}

fn write_private(
    directory: &Path,
    name: &str,
    raw: &[u8],
) -> Result<SelectedHostArtifact, HostLifetimeError> {
    use std::{io::Write, os::unix::fs::OpenOptionsExt};
    let path = directory.join(name);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .map_err(|_| unavailable())?;
    file.write_all(raw)
        .and_then(|_| file.sync_all())
        .map_err(|_| unavailable())?;
    std::fs::File::open(directory)
        .and_then(|file| file.sync_all())
        .map_err(|_| unavailable())?;
    let selected = SelectedHostArtifact {
        path,
        sha256: digest(raw),
    };
    if read_selected(&selected)? != raw {
        return Err(invalid());
    }
    Ok(selected)
}

// A separate disposition type, constructible only after this module's full
// typed native/consent/physical join. It is never a successful ClosedObservation
// and cannot be passed to the old restore_shared_setup path.
struct AbandonedNoEntryDisposition {
    association: SelectedHostArtifact,
    artifact: SelectedHostArtifact,
}
impl AbandonedNoEntryDisposition {
    fn check(&self, selected: &SelectedHostArtifact) -> Result<(), HostLifetimeError> {
        if &self.association != selected {
            return Err(HostLifetimeError::WrongOwner);
        }
        read_selected(selected)?;
        let value: Value =
            serde_json::from_slice(&read_selected(&self.artifact)?).map_err(|_| invalid())?;
        if value["association"] != serde_json::to_value(selected).map_err(|_| invalid())?
            || value["scope"] != "abandoned_native_settled_consent_withdrawn_physical_not_entered"
        {
            return Err(invalid());
        }
        Ok(())
    }
}

fn retained_abandoned_aggregate(
    document: &ResourceAssociation,
    selected: &SelectedHostArtifact,
    native: &ColdPredecessorSettlement,
    proof: &meerkat_core::session_hosting::HostingOwnerLossProof,
    observations: &[Value],
    consent: &[meerkat_core::ApprovalRecord],
    physical: &Value,
    local: &crate::approval::ClosedLocalApprovalsArtifact,
) -> Result<AbandonedNoEntryDisposition, HostLifetimeError> {
    use meerkat_runtime::SettledBy;
    local.check().map_err(|_| HostLifetimeError::NotSettled)?;
    let work: Vec<_> = native.inputs.iter().map(|input| {
        let settled_by = match &input.settled_by {
            SettledBy::TailConsumed => "tail_consumed", SettledBy::Ended => "ended",
            SettledBy::PendingTerminalConverged => "pending_terminal_converged", SettledBy::OutcomeUnknown => "outcome_unknown",
            _ => return Err(invalid()),
        };
        Ok(json!({"input_id":input.input_id,"interaction_id":input.interaction_id,
            "settled_by":settled_by,"published_terminal_seq":input.published_terminal_seq,
            "receipt":{"terminal":input.receipt.terminal(),"scope":retained_receipt_scope(input.receipt.scope())?,"run_id":input.receipt.run_id(),
                "owner_input_id":input.receipt.owner_input_id(),"recipients":input.receipt.recipient_input_ids(),
                "completion_class":format!("{:?}",input.receipt.outcome().class()),
                "outcome_unknown":unknown_outcome(&input.receipt)?}}))
    }).collect::<Result<_,_>>()?;
    let owner_loss = match &native.owner_loss {
        Some(PredecessorOwnerLoss::Completed(_)) => "completed",
        Some(PredecessorOwnerLoss::NotOwed(_)) => "not_owed",
        _ => return Err(HostLifetimeError::NotSettled),
    };
    let raw = toolkit_package::canonical_json(&json!({"schema_version":1,
        "scope":"abandoned_native_settled_consent_withdrawn_physical_not_entered",
        "association":selected,"session":document.session,"issuer":document.consent,
        "native":{"owner_loss":owner_loss,"domain":proof.domain(),"session":proof.session_id(),
            "acquisition":format!("{:?}",proof.acquisition()),"tail":native.tail.as_ref().map(|tail|
                json!({"run_id":tail.run_id,"disposition":format!("{:?}",tail.disposition),"boundary_sequence":tail.boundary_sequence})),
            "convergence_work":work,"durable_rows_closed":native.durable_rows_closed,
            "migrated_without_receipt":native.migrated_without_receipt,"terminals_owed":native.terminals_owed,
            "census":observations},"consent_records":consent,"physical":physical,"local_presenter":local.artifact(),
        "qualification":"input terminality and physical nonentry, not successful execution or an operation retry"})).map_err(|_| invalid())?;
    if raw.len() > 8 * 1024 * 1024 {
        return Err(invalid());
    }
    let artifact = write_private(
        &document.resources.artifact_directory,
        &format!("abandoned-workflow-{}.json", uuid::Uuid::new_v4()),
        &raw,
    )?;
    Ok(AbandonedNoEntryDisposition {
        association: selected.clone(),
        artifact,
    })
}
fn claim_adoption(
    selected: &SelectedHostArtifact,
    aggregate: &AbandonedNoEntryDisposition,
) -> Result<(), HostLifetimeError> {
    aggregate.check(selected)?;
    let directory = selected.path.parent().ok_or_else(invalid)?;
    let raw = toolkit_package::canonical_json(
        &json!({"schema_version":1,"scope":"one_shot_abandoned_workflow_adoption",
        "association":selected,"aggregate":aggregate.artifact}),
    )
    .map_err(|_| invalid())?;
    write_private(
        directory,
        &format!("abandoned-adoption-{}.json", selected.sha256),
        &raw,
    )?;
    Ok(())
}

impl LiveSharedSetup {
    async fn publish_abandoned(
        &self,
        admitted: Arc<AdmittedMemberClass>,
        expected: &MemberClassAdmission,
        identity: &meerkat_mobkit::identity_first::AgentIdentity,
        retained: &RetainedAssociation,
        claim: &HostingClaim,
        native: &ColdPredecessorSettlement,
        physical: Arc<crate::resources::ColdPhysicalNoEntry>,
        local: crate::approval::ClosedLocalApprovalsArtifact,
        aggregate: AbandonedNoEntryDisposition,
        old_peer: McpServerConfig,
    ) -> Result<(), HostLifetimeError> {
        let inputs = self.restore_inputs.get().ok_or_else(unavailable)?;
        let document = &retained.document;
        aggregate.check(&retained.selection)?;
        local.check().map_err(|_| HostLifetimeError::NotSettled)?;
        let captured = self.native_context()?;
        let machine = captured.machine()?;
        let owner = self
            .consent_owner
            .get()
            .and_then(Option::as_ref)
            .ok_or(HostLifetimeError::NotSettled)?;
        let proof = claim
            .completed_owner_loss()
            .ok_or(HostLifetimeError::NotSettled)?;
        let association =
            RetainedAssociation::bind_consent(expected, &self.realm, owner, machine, claim)?;
        let (ids, _) = cold_native_census(machine, document, native).await?;
        owner
            .observe_abandoned_after_native(&association, claim, &proof, &ids)
            .map_err(consent_error)?;
        self.cold_current(&admitted, expected)?;
        physical
            .recheck()
            .map_err(|_| HostLifetimeError::NotSettled)?;
        let source = self
            .restore_source
            .get()
            .and_then(std::sync::Weak::upgrade)
            .ok_or_else(unavailable)?;
        source
            .restore_workflow_class_source(admitted.clone(), identity)
            .await?;
        let storage = inputs.realm.resolve().map_err(|_| unavailable())?;
        let resources =
            crate::resources::InstalledResources::locate().map_err(|_| unavailable())?;
        let cancelled = self.closing.notified();
        tokio::pin!(cancelled);
        cancelled.as_mut().enable();
        let prepared = resources
            .prepare_with_shared_setup(&storage, 1, async {
                if !self.closed.load(Ordering::Acquire) {
                    cancelled.await;
                }
                Ok(())
            })
            .await
            .map_err(|_| HostLifetimeError::NotSettled)?;
        let mut new_approvals = None;
        let publication = async {
            self.cold_current(&admitted, expected)?;
            physical
                .recheck()
                .map_err(|_| HostLifetimeError::NotSettled)?;
            let (ids, _) = cold_native_census(machine, document, native).await?;
            owner
                .observe_abandoned_after_native(&association, claim, &proof, &ids)
                .map_err(consent_error)?;
            let approvals = inputs
                .presenters
                .create_shared(inputs.tasks.clone(), inputs.policy.clone())
                .map_err(|_| unavailable())?;
            new_approvals = Some(approvals.clone());
            approvals
                .bind_session(&document.session)
                .map_err(|_| unavailable())?;
            let mut open = document.clone();
            open.resources = prepared_resources(&prepared.artifact_directory, &self.realm)?;
            open.approval_source = approvals
                .source_for_session(&document.session)
                .map_err(|_| unavailable())?
                .to_owned();
            open.physical = prepared.owner.physical_selection();
            if open.physical.is_none() || prepared.connections.len() != 1 {
                return Err(HostLifetimeError::NotSettled);
            }
            open.projected_peer = Some(prepared.connections[0].connection.server().clone());
            open.predecessor = Some(retained.selection.clone());
            open.abandoned_predecessor = Some(aggregate.artifact.clone());
            open.closed = None;
            self.check_consent_provenance(&open)?;
            let open = open.retain()?;
            let observation = self
                .policy
                .observe_member_class(&expected.instance.member.member)
                .await?;
            observation.authorization?;
            if observation.admission != *expected {
                return Err(HostLifetimeError::Denied);
            }
            self.cold_current(&admitted, expected)?;
            aggregate.check(&retained.selection)?;
            local.check().map_err(|_| HostLifetimeError::NotSettled)?;
            let (ids, _) = cold_native_census(machine, document, native).await?;
            owner
                .observe_abandoned_after_native(&association, claim, &proof, &ids)
                .map_err(consent_error)?;
            physical
                .recheck()
                .map_err(|_| HostLifetimeError::NotSettled)?;
            self.policy
                .retain_workflow_member_resource(
                    &admitted,
                    expected,
                    &observation.checkpoint,
                    open.selection.clone(),
                )
                .await?;
            Ok((approvals, open))
        }
        .await;
        match publication {
            Ok((approvals, open)) => match self.retain_selected(
                admitted,
                prepared,
                approvals.clone(),
                Some(document.session.clone()),
                Some(old_peer),
                Some(open),
            ) {
                Ok(()) => Ok(()),
                Err((owner, error)) => {
                    approvals.retire();
                    owner
                        .close()
                        .await
                        .map_err(|_| HostLifetimeError::NotSettled)?;
                    Err(error)
                }
            },
            Err(error) => {
                if let Some(approvals) = new_approvals {
                    approvals.retire();
                }
                prepared
                    .owner
                    .close()
                    .await
                    .map_err(|_| HostLifetimeError::NotSettled)?;
                Err(error)
            }
        }
    }
}

#[cfg(test)]
mod same_g;
#[cfg(test)]
mod tests;

#[cfg(test)]
mod current_g;
#[cfg(test)]
mod historical_g;
