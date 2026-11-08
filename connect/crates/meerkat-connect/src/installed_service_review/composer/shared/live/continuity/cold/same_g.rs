//! Acceptance observations inside the actual retained R5 claim owner.
//! These hooks run only in the explicit same-G child. They create no claim,
//! completion, consent transition or physical disposition of their own.
use super::*;
use crate::application_installed::canonical_g_recovery as fixture;
fn require(value: bool) -> Result<(), HostLifetimeError> {
    if value {
        Ok(())
    } else {
        Err(HostLifetimeError::NotSettled)
    }
}
// Core's public unregistered read withholds result bodies and body-derived
// digests. Compare every remaining field, retaining both original raw reports.
// This is only a predecessor identity check. Actual terminality comes from the
// mandatory R5 result and complete native receipt census below.
fn body_free_rows(
    rows: &Value,
) -> Result<std::collections::BTreeMap<String, Value>, HostLifetimeError> {
    let mut result = std::collections::BTreeMap::new();
    for raw in rows.as_array().ok_or_else(invalid)? {
        let mut row = raw.clone();
        let id = row["input_id"].as_str().ok_or_else(invalid)?.to_owned();
        for field in ["terminal_completion", "interaction_terminal_outbox"] {
            if let Some(object) = row.get_mut(field).and_then(Value::as_object_mut) {
                // Exact released names of Core withhold_result_payload fields.
                object.remove("candidate");
                object.remove("candidate_digest");
                if field == "terminal_completion" {
                    object.remove("outcome");
                }
                if let Some(phase) = object.get_mut("phase").and_then(Value::as_object_mut) {
                    if field == "terminal_completion" {
                        phase.remove("receipt_digest");
                    } else {
                        phase.remove("finalized_event");
                        phase.remove("finalized_payload_digest");
                        if let Some(publication) =
                            phase.get_mut("publication").and_then(Value::as_object_mut)
                        {
                            publication.remove("payload_digest");
                        }
                    }
                }
            }
        }
        if result.insert(id, row).is_some() {
            return Err(invalid());
        }
    }
    Ok(result)
}
pub(super) async fn before_native(
    live: &LiveSharedSetup,
    native: &NativeContext,
    retained: &RetainedAssociation,
    claim: &HostingClaim,
    physical: &crate::resources::ColdPhysicalNoEntry,
    local: &crate::approval::ClosedLocalApprovalsArtifact,
) -> Result<(), HostLifetimeError> {
    let Some(old) = fixture::predecessor() else {
        return Ok(());
    };
    let document = &retained.document;
    let machine = native.machine()?;
    let owner = live
        .consent_owner
        .get()
        .and_then(Option::as_ref)
        .ok_or_else(unavailable)?;
    require(
        json!(retained.selection) == old["association"]
            && json!(document) == old["association_document"]
            && json!(document.session) == old["session_id"]
            && json!(owner.selection()) == old["issuer"]
            && json!(owner.identity()) == old["identity"]
            && json!(physical.selection()) == old["physical"]
            && json!(local.artifact()) == old["presenter_closed"],
    )?;
    require(
        machine
            .current_session_registration_witness(&document.session)
            .await
            .is_none()
            && claim.session_id() == &document.session
            && claim.domain_ref().as_ref()
                == machine
                    .hosting_authority()
                    .capability()
                    .domain_ref()
                    .as_ref()
            && claim.completed_owner_loss().is_none(),
    )?;
    // Reacquire through the exact captured service, retaining both pairs across
    // the observation. Forwarding wrappers are not compared by Arc identity.
    let reacquired = native
        .service
        .acquire_runtime_adapter(None)
        .map_err(|_| unavailable())?
        .ok_or_else(unavailable)?;
    require(Arc::ptr_eq(machine, &reacquired))?;
    let input = machine
        .session_input_states(&document.session)
        .await
        .map_err(|_| unavailable())?;
    require(
        input.source == TerminalWitnessSource::DurableStore
            && body_free_rows(&json!(input.report))? == body_free_rows(&old["native_inputs"])?,
    )?;
    let rows = owner
        .approvals()
        .list(meerkat_core::approval::ApprovalListFilter::default())
        .map_err(|_| unavailable())?;
    require(json!(rows) == old["approvals"])?;
    local.check().map_err(|_| unavailable())?;
    physical.recheck().map_err(|_| unavailable())?;
    // Missing presenter/legacy physical evidence remains a refusal; no JSON
    // field in this output is accepted in place of these production objects.
    fixture::record(
        "before-native",
        &json!({"association":retained.selection,"session":document.session,
        "claim_domain":claim.domain_ref(),"claim_session":claim.session_id(),"claim_completed_owner_loss":null,
        "registration_absent":true,"same_canonical_machine":true,"input_source":"DurableStore",
        "inputs":input.report,"historical_live_inputs":old["native_inputs"],
        "comparison":"all fields except documented Core public-read result payload and payload digests","issuer_rows":rows,"physical":physical.census(),"presenter":local.artifact()}),
    )
}
pub(super) async fn repeat_native(
    live: &LiveSharedSetup,
    state: &mut ColdState,
    native: &NativeContext,
    retained: &RetainedAssociation,
    association: &crate::application_consent::ConsentAssociation,
    first: &ColdPredecessorSettlement,
    observations: &[Value],
    records: &[meerkat_core::ApprovalRecord],
    deadline: Instant,
) -> Result<(), HostLifetimeError> {
    if fixture::predecessor().is_none() {
        return Ok(());
    }
    require(state.native.len() == 1 && !state.adoption_claimed && state.result.is_none())?;
    let Some(PredecessorOwnerLoss::Completed(proof)) = &first.owner_loss else {
        return Err(HostLifetimeError::NotSettled);
    };
    let claim = state.claim.as_ref().ok_or_else(unavailable)?.clone();
    require(claim.completed_owner_loss().as_ref() == Some(proof))?;
    let repeated = state
        .observe_native(native, &retained.document.session, deadline)
        .await?;
    let second = repeated
        .as_ref()
        .as_ref()
        .map_err(|_| HostLifetimeError::NotSettled)?;
    require(
        second.inputs.is_empty()
            && second.session_id == first.session_id
            && second.durable_rows_closed == first.durable_rows_closed
            && second.migrated_without_receipt.is_empty()
            && second.terminals_owed.is_empty()
            && matches!(&second.owner_loss, Some(PredecessorOwnerLoss::NotOwed(same)) if same == proof)
            && state
                .claim
                .as_ref()
                .and_then(HostingClaim::completed_owner_loss)
                .as_ref()
                == Some(proof),
    )?;
    let (ids, census) = cold_native_census(native.machine()?, &retained.document, second).await?;
    require(census == observations && !ids.is_empty())?;
    let owner = live
        .consent_owner
        .get()
        .and_then(Option::as_ref)
        .ok_or_else(unavailable)?;
    let whole = owner
        .observe_abandoned_after_native(association, &claim, proof, &ids)
        .map_err(consent_error)?;
    require(whole.records == records && whole.records.len() == 1)?;
    state
        .physical
        .as_ref()
        .ok_or_else(unavailable)?
        .recheck()
        .map_err(|_| unavailable())?;
    fixture::record(
        "native-repeat",
        &json!({"association":retained.selection,"session":first.session_id,
        "first_owner_loss":"completed","first_convergence_count":first.inputs.len(),
        "repeat_owner_loss":"not_owed","repeat_convergence_count":second.inputs.len(),
        "durable_rows_closed":second.durable_rows_closed,"same_completed_proof":true,
        "registration_absent":true,"complete_census":census,"consent_records":whole.records,
        "physical":state.physical.as_ref().ok_or_else(unavailable)?.census()}),
    )
}
pub(super) fn one_shot(
    selected: &SelectedHostArtifact,
    aggregate: &AbandonedNoEntryDisposition,
) -> Result<(), HostLifetimeError> {
    if fixture::predecessor().is_none() {
        return Ok(());
    }
    aggregate.check(selected)?;
    let path = selected
        .path
        .parent()
        .ok_or_else(invalid)?
        .join(format!("abandoned-adoption-{}.json", selected.sha256));
    let before = std::fs::read(&path).map_err(|_| unavailable())?;
    require(
        claim_adoption(selected, aggregate).is_err()
            && std::fs::read(&path).map_err(|_| unavailable())? == before,
    )?;
    fixture::record(
        "one-shot",
        &json!({"association":selected,"aggregate":aggregate.artifact,
        "adoption_fence":SelectedHostArtifact {path,sha256:digest(&before)},"second_claim_refused":true}),
    )
}
impl LiveSharedSetup {
    /// Observe the actual post-publication owner after native Ready or work.
    /// The original claim and physical exclusion remain in ColdState.
    pub(crate) async fn inspect_recovered_canonical_g(&self) -> Result<Value, HostLifetimeError> {
        let old = fixture::predecessor().ok_or_else(invalid)?;
        let session: SessionId =
            serde_json::from_value(old["session_id"].clone()).map_err(|_| invalid())?;
        let selected: SelectedHostArtifact =
            serde_json::from_value(old["association"].clone()).map_err(|_| invalid())?;
        let old_doc: ResourceAssociation =
            serde_json::from_value(old["association_document"].clone()).map_err(|_| invalid())?;
        let mut entries = self
            .snapshot()?
            .into_iter()
            .filter(|entry| entry.admitted.instance().member.member.member == "planning-lead");
        let entry = entries.next().ok_or_else(unavailable)?;
        require(entries.next().is_none())?;
        let current = self.current(&entry)?;
        let retained = entry.custody.association.get().ok_or_else(unavailable)?;
        retained.check_open(
            &current,
            &self.realm,
            &entry.custody.prepared.artifact_directory,
        )?;
        self.check_consent_provenance(&retained.document)?;
        let document = &retained.document;
        require(
            active_session(&current)? == session
                && document.session == session
                && document.predecessor.as_ref() == Some(&selected)
                && document.closed.is_none()
                && document.consent == old_doc.consent
                && document.baseline == old_doc.baseline
                && document.resources.artifact_directory != old_doc.resources.artifact_directory
                && document.approval_source != old_doc.approval_source
                && document.physical.is_some()
                && document.physical != old_doc.physical
                && entry.context.physical_selection() == document.physical
                && document.projected_peer.as_ref() == Some(entry.resource.server())
                && document.projected_peer != old_doc.projected_peer
                && current.workflow_resource.as_ref() == Some(&retained.selection),
        )?;
        let aggregate = document
            .abandoned_predecessor
            .as_ref()
            .ok_or_else(unavailable)?;
        let aggregate_raw = read_selected(aggregate)?;
        let aggregate_value: Value =
            serde_json::from_slice(&aggregate_raw).map_err(|_| invalid())?;
        require(
            aggregate_value["association"] == old["association"]
                && aggregate_value["session"] == old["session_id"],
        )?;
        let recovery = self
            .cold_recoveries
            .lock()
            .map_err(|_| unavailable())?
            .get(entry.admitted.admission_id())
            .cloned()
            .ok_or_else(unavailable)?;
        let state = recovery.state.lock().await;
        require(state.adoption_claimed && state.result == Some(Ok(())) && state.native.len() == 2)?;
        let claim = state.claim.as_ref().ok_or_else(unavailable)?;
        require(claim.session_id() == &session && claim.completed_owner_loss().is_some())?;
        state
            .physical
            .as_ref()
            .ok_or_else(unavailable)?
            .recheck()
            .map_err(|_| unavailable())?;
        let native = self.native_context()?;
        let machine = native.machine()?;
        let registration = machine
            .current_session_registration_witness(&session)
            .await
            .ok_or_else(unavailable)?;
        require(!registration.has_ended())?;
        let observed = machine
            .session_input_states(&session)
            .await
            .map_err(|_| unavailable())?;
        require(observed.source == TerminalWitnessSource::LiveRuntime)?;
        let owner = self
            .consent_owner
            .get()
            .and_then(Option::as_ref)
            .ok_or_else(unavailable)?;
        let rows = owner
            .approvals()
            .list(meerkat_core::approval::ApprovalListFilter::default())
            .map_err(|_| unavailable())?;
        Ok(
            json!({"session":session,"admission":current,"association":retained.selection,
            "document":document,"aggregate":aggregate,"aggregate_value":aggregate_value,
            "native_source":"LiveRuntime","inputs":observed.report,"registration":format!("{registration:?}"),
            "same_claim_completed":true,"native_observations":state.native.len(),
            "old_physical":state.physical.as_ref().ok_or_else(unavailable)?.census(),"issuer_rows":rows}),
        )
    }
}
