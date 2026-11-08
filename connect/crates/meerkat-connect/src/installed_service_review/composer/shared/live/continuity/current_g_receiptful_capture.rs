//! Current-behavior test observation of the actual published G at its terminal hook.
//! No synthetic input, authority, consent transition or replacement preparation.
use super::*;
use meerkat_core::{
    ApprovalRecord, ApprovalStatus,
    approval::{ApprovalConsentCustody, ApprovalListFilter},
};
use meerkat_runtime::{MeerkatMachine, input_state::InputLifecycleState};
use serde_json::{Value, json};
fn check(ok: bool, why: &str) -> Result<(), String> {
    if ok { Ok(()) } else { Err(why.into()) }
}
fn error(e: impl std::fmt::Display) -> String {
    e.to_string()
}
impl LiveSharedSetup {
    pub(crate) fn current_g_review_floor(
        &self,
        issuer: &Arc<crate::application_consent::ApplicationConsentOwner>,
        context: toolkit_mobkit::installed_host::InstalledOperationReviewContext<'_>,
    ) -> Result<(SessionId, Arc<MeerkatMachine>), meerkat_core::OperationAuthorizationError> {
        use meerkat_core::OperationAuthorizationError;
        let unavailable = || OperationAuthorizationError::Unavailable;
        crate::application_consent::fixture_review_floor::same_application(issuer, &context)?;
        let selected = self
            .consent_owner
            .get()
            .and_then(Option::as_ref)
            .ok_or_else(unavailable)?;
        if !Arc::ptr_eq(selected, issuer) {
            return Err(unavailable());
        }
        let meerkat_core::ToolOperationTarget::McpServerOperation {
            server,
            raw_operation,
            ..
        } = context.target()
        else {
            return Err(unavailable());
        };
        if raw_operation != "setup_state" {
            return Err(unavailable());
        }
        let claims = context.association().candidate();
        let mut entries = self
            .snapshot()
            .map_err(|_| unavailable())?
            .into_iter()
            .filter(|entry| &entry.resource.server().name == server);
        let entry = entries.next().ok_or_else(unavailable)?;
        if entries.next().is_some()
            || entry.admitted.instance().member.member.member != "planning-lead"
        {
            return Err(unavailable());
        }
        let current = self.current(&entry).map_err(|_| unavailable())?;
        let session = active_session(&current).map_err(|_| unavailable())?;
        let retained = entry.custody.association.get().ok_or_else(unavailable)?;
        retained
            .check_open(
                &current,
                &self.realm,
                &entry.custody.prepared.artifact_directory,
            )
            .map_err(|_| unavailable())?;
        self.check_consent_provenance(&retained.document)
            .map_err(|_| unavailable())?;
        let member = &entry.admitted.instance().member;
        if claims.target.context.as_str() != session.to_string()
            || claims.target.logical_owner != member.logical_owner
            || claims.target.context_generation != member.context_generation
            || claims.target.audience != member.audience
            || retained.document.projected_peer.as_ref() != Some(entry.resource.server())
            || current.workflow_resource.as_ref() != Some(&retained.selection)
        {
            return Err(unavailable());
        }
        let native = self.native_context().map_err(|_| unavailable())?;
        let machine = native.machine().map_err(|_| unavailable())?.clone();
        Ok((session, machine))
    }

    /// Only the explicit child calls this after the exact native terminal hook enters.
    /// Its retained sender keeps the actual commit paused through process loss.
    /// Retires this presenter's eligibility, without closing native/physical work.
    pub(crate) async fn capture_held_current_g(
        &self,
        issuer: &Arc<crate::application_consent::ApplicationConsentOwner>,
        machine: &Arc<MeerkatMachine>,
        approval: &ApprovalRecord,
    ) -> Result<Value, String> {
        let selected = self
            .consent_owner
            .get()
            .and_then(Option::as_ref)
            .ok_or("G issuer absent")?;
        check(
            Arc::ptr_eq(selected, issuer),
            "resource and review issuer owners differ",
        )?;
        let context = self.native_context().map_err(error)?;
        check(
            Arc::ptr_eq(context.machine().map_err(error)?, machine),
            "actual runtime and G machine differ",
        )?;
        let consent = approval.consent.as_ref().ok_or("G consent absent")?;
        let scope = consent.scope.as_ref().ok_or("G scope absent")?;
        check(
            approval.status == ApprovalStatus::Pending
                && consent.routable()
                && consent.custody == Some(ApprovalConsentCustody::Parked)
                && consent.withdrawal.is_none()
                && consent.consumed_at.is_none()
                && consent.deliveries.is_empty(),
            "one real Pending/Parked G candidate required",
        )?;
        let origin = consent
            .origin
            .as_ref()
            .ok_or("retained native origin absent")?;
        let mut entries = self
            .snapshot()
            .map_err(error)?
            .into_iter()
            .filter(|entry| entry.admitted.instance().member.member.member == "planning-lead");
        let entry = entries.next().ok_or("actual G entry absent")?;
        check(entries.next().is_none(), "ambiguous G entry")?;
        let before = self.current(&entry).map_err(error)?;
        let session = active_session(&before).map_err(error)?;
        check(
            session == scope.session_id,
            "approval belongs to another session",
        )?;
        let retained = entry
            .custody
            .association
            .get()
            .ok_or("canonical G association absent")?;
        retained
            .check_open(
                &before,
                &self.realm,
                &entry.custody.prepared.artifact_directory,
            )
            .map_err(error)?;
        self.check_consent_provenance(&retained.document)
            .map_err(error)?;
        let document = &retained.document;
        check(
            document.session == session
                && document.closed.is_none()
                && before.workflow_resource.as_ref() == Some(&retained.selection),
            "current open canonical association required",
        )?;
        let provenance = document
            .consent
            .as_ref()
            .ok_or("selected issuer provenance absent")?;
        check(
            provenance.domain() == Some(&scope.domain)
                && provenance.session() == &session
                && provenance.issuer() == issuer.selection()
                && provenance.identity() == issuer.identity(),
            "canonical provenance differs from the actual issued row",
        )?;
        let physical = document
            .physical
            .as_ref()
            .ok_or("physical provenance absent")?;
        check(
            entry.context.physical_selection().as_ref() == Some(physical)
                && document.projected_peer.as_ref() == Some(entry.resource.server()),
            "actual selected G peer differs",
        )?;
        let bytes = read_selected(&retained.selection).map_err(error)?;
        check(
            serde_json::from_slice::<Value>(&bytes).map_err(error)?
                == serde_json::to_value(document).map_err(error)?,
            "canonical bytes differ from the retained owner document",
        )?;
        let registration = machine
            .current_session_registration_witness(&session)
            .await
            .ok_or("G registration absent")?;
        let observed = machine
            .session_input_states(&session)
            .await
            .map_err(error)?;
        check(
            observed.source == TerminalWitnessSource::LiveRuntime && !registration.has_ended(),
            "live native G witness required",
        )?;
        check(
            observed.report.len() == 2,
            "exact G planning plus actual context inputs required",
        )?;
        let contributors = origin.retained_work.contributors();
        check(
            contributors.len() == 1,
            "one actual G context contributor required",
        )?;
        let origin_input = observed
            .report
            .iter()
            .find(|row| row.state.input_id == contributors[0].input_id)
            .ok_or("consent contributor is not a G input")?;
        check(
            origin_input.seed.last_run_id.as_ref() == Some(origin.retained_work.run_id()),
            "actual G run differs",
        )?;
        check(
            matches!(
                origin_input.seed.phase,
                InputLifecycleState::Applied | InputLifecycleState::AppliedPendingConsumption
            ) && origin_input.seed.terminal_outcome.is_none(),
            "the exact live G contributor must remain applied and unconsumed at the real hook",
        )?;
        for row in observed
            .report
            .iter()
            .filter(|row| row.state.input_id != contributors[0].input_id)
        {
            check(
                row.seed.phase == InputLifecycleState::Consumed,
                "the separate G planning input must be consumed",
            )?;
            let receipt = machine
                .input_terminal_receipt_observation(
                    &session,
                    InteractionSelector::InputId(row.state.input_id.clone()),
                )
                .await
                .map_err(error)?
                .ok_or("prior G planning receipt absent")?;
            check(
                matches!(&receipt.report, InputTerminalReceiptObservation::Finalized(summary)
                if summary.input_id() == &row.state.input_id),
                "prior G planning input must have a real finalized receipt",
            )?;
        }
        let realm = &self
            .restore_inputs
            .get()
            .ok_or("actual selected G realm absent")?
            .realm;
        check(
            realm.resolve().map_err(error)?.locator == self.realm,
            "selected G realm differs",
        )?;
        let records = issuer
            .approvals()
            .list(ApprovalListFilter::default())
            .map_err(error)?;
        check(
            records.len() == 1 && records.first() == Some(approval),
            "whole issuer census differs",
        )?;
        check(
            matches!(
                crate::application_consent::ApplicationConsentOwner::reopen(
                    issuer.selection().clone(),
                    issuer.identity()
                ),
                Err(crate::application_consent::ConsentOwnerError::OwnerHeld)
            ),
            "actual issuer exclusion absent",
        )?;
        // This exact owner supplies ordinary invocation closure. Scoped consent
        // remains Pending; empty JSON or its withdrawal cannot replace this token.
        entry.approvals.retire_checked().map_err(error)?;
        let closed = entry
            .approvals
            .closed_observation(&session)
            .map_err(error)?
            .ok_or("ordinary presenter has unresolved custody")?;
        let (source, selected_session) = closed.source();
        check(
            source == document.approval_source && selected_session == &session,
            "wrong presenter source",
        )?;
        let local = closed
            .retain_for_association(&retained.selection)
            .map_err(error)?;
        local.check().map_err(error)?;
        let again = machine
            .session_input_states(&session)
            .await
            .map_err(error)?;
        check(
            self.current(&entry).map_err(error)? == before
                && again.source == TerminalWitnessSource::LiveRuntime
                && machine
                    .current_session_registration_witness(&session)
                    .await
                    .as_ref()
                    == Some(&registration)
                && serde_json::to_value(&again.report).map_err(error)?
                    == serde_json::to_value(&observed.report).map_err(error)?
                && issuer
                    .approvals()
                    .list(ApprovalListFilter::default())
                    .map_err(error)?
                    == records
                && read_selected(&retained.selection).map_err(error)? == bytes,
            "G capture changed during observation",
        )?;
        Ok(
            json!({"schema":1,"phase":"current_g_at_terminal_commit_hook","session_id":session,
            "origin_input_id":contributors[0].input_id,"origin_run_id":origin.retained_work.run_id(),
            "admission":before,"association":retained.selection,"association_document":document,
            "issuer":issuer.selection(),"identity":issuer.identity(),"selected_approval":approval,"approvals":records,
            "presenter_closed":local.artifact(),"physical":physical,"native_input_source":"LiveRuntime",
            "native_inputs":observed.report,"realm":realm,"projected_peer":entry.resource.server(),"registration":format!("{registration:?}"),"issuer_exclusion_observed":true,
            "native_settlement":null,"physical_disposition":null,"new_preparation":null}),
        )
    }
}
