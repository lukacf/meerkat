//! A durable original offer, its effect intent and its dispatch journal, not a native
//! admission mirror.
//!
//! The exact first input and all selection/attribution bindings live in the
//! immutable record referenced by input_ref and binding_digest. Core owns the
//! native key and outcome. Only its verified immutable receipt is retained.
//! CoreObserved is terminal receipt-backed observation, never a local decision.
//!
//! Every fact that decides one of its transitions is owner state: the preparing
//! process's epoch (only an ended process's preparation is withdrawn at restart),
//! each dispatch started before its call into Meta (a peer delivery's before its
//! protected section) with that process's epoch, a lost answer recorded against its
//! dispatch, the dispatch and kind a terminal receipt settles, the final-absence
//! evidence with the dispatches it covers, and the close's unresolved record with the
//! closing process's epoch (no dispatch of that process follows it; the next process's
//! restart dispatches the record again). A dispatch answered without native submission,
//! or refused while another dispatch is open or unresolved, leaves the journal: it
//! cannot have admitted. An open dispatch of an ended process stays: its outcome is
//! unknown.
#![allow(clippy::cmp_owned)]

use super::original_types::{
    ToolkitCoreObservation, ToolkitDispatchAnswer, ToolkitReceiptKind, ToolkitWithdrawalCause,
};
use meerkat_machine_dsl::machine;

// The DSL's map read (`get_cloned(key).get("value")`); every read is guarded by its key.
use meerkat_machine_schema::catalog::dsl::OptionValueExt;

machine! {
    machine ToolkitOriginalOfferMachine {
        version: 2,
        rust: "toolkit_catalog" / "original_offer",
        state {
            offer_phase: ToolkitOfferPhase,
            offer_key: String,
            source_key: String,
            source_revision: u64,
            incarnation: String,
            input_ref: String,
            input_digest: String,
            binding_digest: String,
            preparation_id: String,
            preparation_receipt: String,
            preparation_epoch: u64,
            dispatch_count: u64,
            dispatch_epochs: Map<u64, u64>,
            unresolved_dispatches: Set<u64>,
            last_dispatch_epoch: u64,
            core_receipt_ref: String,
            receipt_kind: Enum<ToolkitReceiptKind>,
            receipt_dispatch: u64,
            receipt_epoch: u64,
            absence_through: u64,
            close_epoch: u64,
            withdrawal_cause: Enum<ToolkitWithdrawalCause>,
            revision: u64,
        }
        init(Absent) {
            offer_key = "", source_key = "", source_revision = 0, incarnation = "",
            input_ref = "", input_digest = "", binding_digest = "",
            preparation_id = "", preparation_receipt = "", preparation_epoch = 0,
            dispatch_count = 0, dispatch_epochs = EmptyMap, unresolved_dispatches = EmptySet,
            last_dispatch_epoch = 0, core_receipt_ref = "", receipt_kind = ToolkitReceiptKind::Unsettled,
            receipt_dispatch = 0, receipt_epoch = 0, absence_through = 0, close_epoch = 0,
            withdrawal_cause = ToolkitWithdrawalCause::NotWithdrawn, revision = 0,
        }
        terminal [Withdrawn, CoreObserved]
        phase ToolkitOfferPhase { Absent, Prepared, Offered, Withdrawn, CoreObserved }
        input ToolkitOfferInput {
            Prepare {
                offer_key: String, source_key: String, source_revision: u64,
                incarnation: String, input_ref: String, input_digest: String,
                binding_digest: String, preparation_id: String, preparation_epoch: u64,
            },
            SealPreparation {
                offer_key: String, expected_revision: u64, preparation_id: String,
                input_digest: String, preparation_receipt: String,
            },
            Offer {
                offer_key: String, expected_revision: u64, source_key: String,
                source_revision: u64, current_incarnation: String,
                source_open: bool, source_ready: bool, component_open: bool, authority_current: bool,
            },
            WithdrawPrepared { offer_key: String, expected_revision: u64, epoch: u64 },
            StartDispatch { offer_key: String, expected_revision: u64, epoch: u64, dispatch_id: u64 },
            ObserveDispatch {
                offer_key: String, expected_revision: u64, input_digest: String, incarnation: String,
                dispatch_id: u64, answer: Enum<ToolkitDispatchAnswer>, receipt_ref: String,
            },
            ObserveRead {
                offer_key: String, expected_revision: u64, input_digest: String, incarnation: String,
                epoch: u64, observation: Enum<ToolkitCoreObservation>, receipt_ref: String,
            },
            CloseUnresolved { offer_key: String, expected_revision: u64, close_id: String, epoch: u64 },
        }
        effect ToolkitOfferEffect {
            PrepareInput { offer_key: String, preparation_id: String },
            SendIntent { offer_key: String, input_ref: String, input_digest: String },
            PublishOffer { offer_key: String, revision: u64 },
            DispatchSettled { offer_key: String, dispatch_id: u64, answer: Enum<ToolkitDispatchAnswer>, receipt_ref: String },
            CoreObserved { offer_key: String, observation: Enum<ToolkitCoreObservation>, receipt_ref: String },
        }
        disposition PrepareInput => local seam NoOwnerRealization,
        disposition SendIntent => local seam NoOwnerRealization,
        disposition PublishOffer => local seam SurfaceResultAlignment,
        disposition DispatchSettled => local seam SurfaceResultAlignment,
        disposition CoreObserved => local seam SurfaceResultAlignment,

        invariant absent_offer_has_no_uncommitted_facts {
            self.offer_phase != Phase::Absent ||
                (self.offer_key == "" && self.source_key == "" && self.source_revision == 0 && self.incarnation == ""
                && self.input_ref == "" && self.input_digest == "" && self.binding_digest == ""
                && self.preparation_id == "" && self.preparation_receipt == "" && self.preparation_epoch == 0
                && self.core_receipt_ref == "" && self.revision == 0)
        }
        invariant offer_identity_is_complete {
            self.offer_phase == Phase::Absent ||
                (self.offer_key != "" && self.source_key != "" && self.source_revision > 0
                && self.incarnation != "" && self.input_ref != "" && self.input_digest != ""
                && self.binding_digest != "" && self.preparation_id != "" && self.preparation_epoch > 0
                && self.revision > 0)
        }
        invariant send_requires_sealed_preparation {
            (self.offer_phase != Phase::Offered && self.offer_phase != Phase::CoreObserved) || self.preparation_receipt != ""
        }
        invariant core_receipt_only_in_observed_terminal {
            self.core_receipt_ref == "" || self.offer_phase == Phase::CoreObserved
        }
        invariant observed_terminal_requires_core_receipt {
            self.offer_phase != Phase::CoreObserved || self.core_receipt_ref != ""
        }
        invariant receipt_kind_follows_the_phase {
            (self.offer_phase == Phase::CoreObserved) == (self.receipt_kind != ToolkitReceiptKind::Unsettled)
        }
        invariant dispatches_follow_the_send_intent {
            self.offer_phase == Phase::Offered || self.offer_phase == Phase::CoreObserved ||
                (self.dispatch_count == 0 && self.dispatch_epochs.len() == 0
                && self.unresolved_dispatches.len() == 0 && self.last_dispatch_epoch == 0
                && self.absence_through == 0 && self.close_epoch == 0)
        }
        invariant recorded_dispatches_are_numbered_and_dated {
            for_all(dispatch in self.dispatch_epochs.keys(),
                dispatch >= 1 && dispatch <= self.dispatch_count
                && self.dispatch_epochs.get_cloned(dispatch).get("value") >= self.preparation_epoch
                && self.dispatch_epochs.get_cloned(dispatch).get("value") <= self.last_dispatch_epoch)
        }
        invariant unresolved_dispatches_are_recorded {
            for_all(dispatch in self.unresolved_dispatches, self.dispatch_epochs.contains_key(dispatch))
        }
        invariant last_dispatch_epoch_follows_the_dispatches {
            (self.dispatch_count == 0) == (self.last_dispatch_epoch == 0)
                && (self.dispatch_count == 0 || self.last_dispatch_epoch >= self.preparation_epoch)
        }
        invariant receipt_names_its_dispatch {
            ((self.receipt_kind == ToolkitReceiptKind::Unsettled || self.receipt_kind == ToolkitReceiptKind::Found)
                == (self.receipt_dispatch == 0))
            && ((self.receipt_dispatch == 0) == (self.receipt_epoch == 0))
            && (self.receipt_dispatch == 0
                || (self.receipt_dispatch <= self.dispatch_count && self.receipt_epoch >= self.preparation_epoch
                    && self.receipt_epoch <= self.last_dispatch_epoch))
        }
        invariant absence_evidence_within_the_dispatches {
            self.absence_through <= self.dispatch_count
        }
        invariant close_epoch_follows_the_preparation {
            self.close_epoch == 0 || self.close_epoch >= self.preparation_epoch
        }
        invariant withdrawal_cause_follows_the_phase {
            (self.offer_phase == Phase::Withdrawn) == (self.withdrawal_cause != ToolkitWithdrawalCause::NotWithdrawn)
        }

        transition PrepareOriginalOffer {
            on input Prepare {
                offer_key, source_key, source_revision, incarnation, input_ref,
                input_digest, binding_digest, preparation_id, preparation_epoch
            }
            guard {
                self.offer_phase == Phase::Absent && offer_key != "" && source_key != "" && source_revision > 0
                && incarnation != "" && input_ref != "" && input_digest != "" && binding_digest != "" && preparation_id != ""
                && preparation_epoch > 0
            }
            update {
                self.offer_key = offer_key; self.source_key = source_key; self.source_revision = source_revision;
                self.incarnation = incarnation; self.input_ref = input_ref; self.input_digest = input_digest;
                self.binding_digest = binding_digest; self.preparation_id = preparation_id;
                self.preparation_epoch = preparation_epoch; self.revision += 1;
            }
            to Prepared
            emit PrepareInput { offer_key: self.offer_key, preparation_id: self.preparation_id }
            emit PublishOffer { offer_key: self.offer_key, revision: self.revision }
        }
        transition SealOriginalPreparation {
            on input SealPreparation { offer_key, expected_revision, preparation_id, input_digest, preparation_receipt }
            guard {
                self.offer_phase == Phase::Prepared && self.preparation_receipt == "" && offer_key == self.offer_key
                && expected_revision == self.revision && preparation_id == self.preparation_id
                && input_digest == self.input_digest && preparation_receipt != "" && self.revision < u64::MAX - 2
            }
            update { self.preparation_receipt = preparation_receipt; self.revision += 1; }
            to Prepared
            emit PublishOffer { offer_key: self.offer_key, revision: self.revision }
        }
        transition RecordOriginalSendIntent {
            on input Offer {
                offer_key, expected_revision, source_key, source_revision, current_incarnation,
                source_open, source_ready, component_open, authority_current
            }
            guard {
                self.offer_phase == Phase::Prepared && self.preparation_receipt != ""
                && offer_key == self.offer_key && expected_revision == self.revision
                && source_key == self.source_key && source_revision == self.source_revision
                && current_incarnation == self.incarnation && source_open && source_ready && component_open
                && authority_current && self.revision < u64::MAX - 1
            }
            update { self.revision += 1; }
            to Offered
            emit SendIntent { offer_key: self.offer_key, input_ref: self.input_ref, input_digest: self.input_digest }
            emit PublishOffer { offer_key: self.offer_key, revision: self.revision }
        }
        // The same observation, failing a check: the unsent preparation is withdrawn (the
        // send ends refused, typed by its cause), and the cause is recorded: a closed
        // admission first, then a closed or changed source, then a denied authority, else a
        // replaced session. A denied authority and a closed source are verdicts on the
        // delivery; the others let the same delivery be prepared again.
        transition RefuseOriginalSendIntent {
            on input Offer {
                offer_key, expected_revision, source_key, source_revision, current_incarnation,
                source_open, source_ready, component_open, authority_current
            }
            guard {
                self.offer_phase == Phase::Prepared && self.preparation_receipt != ""
                && offer_key == self.offer_key && expected_revision == self.revision
                && source_key == self.source_key
                && !(source_revision == self.source_revision && current_incarnation == self.incarnation
                    && source_open && source_ready && component_open && authority_current)
                && self.revision < u64::MAX
            }
            update {
                self.withdrawal_cause = if !component_open { ToolkitWithdrawalCause::AdmissionClosed }
                    else { if !(source_open && source_ready && source_revision == self.source_revision)
                           { ToolkitWithdrawalCause::SourceClosed }
                           else { if !authority_current { ToolkitWithdrawalCause::PolicyDenied }
                                  else { ToolkitWithdrawalCause::PreparationStale } } };
                self.revision += 1;
            }
            to Withdrawn
            emit PublishOffer { offer_key: self.offer_key, revision: self.revision }
        }
        // The restart's withdrawal of an ended process's preparation (never Offered): not a
        // verdict, the same delivery may be prepared again.
        transition WithdrawUnsentPreparation {
            on input WithdrawPrepared { offer_key, expected_revision, epoch }
            guard {
                self.offer_phase == Phase::Prepared && offer_key == self.offer_key
                && expected_revision == self.revision && self.preparation_epoch < epoch
                && self.revision < u64::MAX
            }
            update { self.withdrawal_cause = ToolkitWithdrawalCause::ProcessEnded; self.revision += 1; }
            to Withdrawn
            emit PublishOffer { offer_key: self.offer_key, revision: self.revision }
        }
        // Before each call into Meta (a peer delivery: before its protected section). The
        // closing process starts none after its close's unresolved record; a later process
        // (its restart's re-drive, a same-key replay, Core's re-offer) does.
        transition RecordDispatchStart {
            on input StartDispatch { offer_key, expected_revision, epoch, dispatch_id }
            guard {
                self.offer_phase == Phase::Offered && self.core_receipt_ref == "" && self.close_epoch < epoch
                && offer_key == self.offer_key && expected_revision == self.revision
                && epoch >= self.preparation_epoch && epoch >= self.last_dispatch_epoch
                && self.dispatch_count < u64::MAX && dispatch_id == self.dispatch_count + 1
                && self.revision < u64::MAX
            }
            update {
                self.dispatch_count += 1;
                self.dispatch_epochs.insert(dispatch_id, epoch);
                self.last_dispatch_epoch = epoch;
                self.revision += 1;
            }
            to Offered
            emit PublishOffer { offer_key: self.offer_key, revision: self.revision }
        }
        // A dispatch's answer settles the record: an admission, or a refusal (Core's, or
        // the entry fence's typed refusal) when every other recorded dispatch is answered:
        // none in flight, none of an ended process (ambiguous), none unresolved.
        transition RetainCoreReceipt {
            on input ObserveDispatch { offer_key, expected_revision, input_digest, incarnation, dispatch_id, answer, receipt_ref }
            guard {
                self.offer_phase == Phase::Offered && self.core_receipt_ref == ""
                && offer_key == self.offer_key && expected_revision == self.revision
                && input_digest == self.input_digest && incarnation == self.incarnation
                && self.dispatch_epochs.contains_key(dispatch_id) && !self.unresolved_dispatches.contains(dispatch_id)
                && receipt_ref != "" && self.revision < u64::MAX
                && (answer == ToolkitDispatchAnswer::Admitted
                    || ((answer == ToolkitDispatchAnswer::CoreRefused || answer == ToolkitDispatchAnswer::EntryRefused)
                        && self.dispatch_epochs.len() == 1 && self.unresolved_dispatches.len() == 0))
            }
            update {
                self.core_receipt_ref = receipt_ref;
                self.receipt_kind = if answer == ToolkitDispatchAnswer::Admitted { ToolkitReceiptKind::Admitted }
                    else { if answer == ToolkitDispatchAnswer::CoreRefused { ToolkitReceiptKind::CoreRefused }
                           else { ToolkitReceiptKind::EntryRefused } };
                self.receipt_dispatch = dispatch_id;
                self.receipt_epoch = self.dispatch_epochs.get_cloned(dispatch_id).get("value");
                self.revision += 1;
            }
            to CoreObserved
            emit DispatchSettled { offer_key: self.offer_key, dispatch_id: dispatch_id, answer: answer, receipt_ref: self.core_receipt_ref }
            emit PublishOffer { offer_key: self.offer_key, revision: self.revision }
        }
        // A dispatch that ended without native submission, or a refusal while another
        // dispatch is open or unresolved: it cannot have admitted, so it leaves the journal.
        transition RecordDispatchAnswer {
            on input ObserveDispatch { offer_key, expected_revision, input_digest, incarnation, dispatch_id, answer, receipt_ref }
            guard {
                self.offer_phase == Phase::Offered && self.core_receipt_ref == ""
                && offer_key == self.offer_key && expected_revision == self.revision
                && input_digest == self.input_digest && incarnation == self.incarnation
                && self.dispatch_epochs.contains_key(dispatch_id) && !self.unresolved_dispatches.contains(dispatch_id)
                && self.revision < u64::MAX
                && ((answer == ToolkitDispatchAnswer::NotSubmitted && receipt_ref == "")
                    || ((answer == ToolkitDispatchAnswer::CoreRefused || answer == ToolkitDispatchAnswer::EntryRefused)
                        && !(self.dispatch_epochs.len() == 1 && self.unresolved_dispatches.len() == 0)))
            }
            update { self.dispatch_epochs.remove(dispatch_id); self.revision += 1; }
            to Offered
            emit DispatchSettled { offer_key: self.offer_key, dispatch_id: dispatch_id, answer: answer, receipt_ref: receipt_ref }
            emit PublishOffer { offer_key: self.offer_key, revision: self.revision }
        }
        // A lost answer (AdmissionUnresolved), recorded against its dispatch.
        transition PreserveUnresolvedOffer {
            on input ObserveDispatch { offer_key, expected_revision, input_digest, incarnation, dispatch_id, answer, receipt_ref }
            guard {
                self.offer_phase == Phase::Offered && self.core_receipt_ref == ""
                && offer_key == self.offer_key && expected_revision == self.revision
                && input_digest == self.input_digest && incarnation == self.incarnation
                && self.dispatch_epochs.contains_key(dispatch_id) && !self.unresolved_dispatches.contains(dispatch_id)
                && answer == ToolkitDispatchAnswer::Unresolved && receipt_ref == "" && self.revision < u64::MAX
            }
            update { self.unresolved_dispatches.insert(dispatch_id); self.revision += 1; }
            to Offered
            emit DispatchSettled { offer_key: self.offer_key, dispatch_id: dispatch_id, answer: answer, receipt_ref: receipt_ref }
            emit PublishOffer { offer_key: self.offer_key, revision: self.revision }
        }
        // A read by the offer's own key that found Core's admission.
        transition RetainFoundReceipt {
            on input ObserveRead { offer_key, expected_revision, input_digest, incarnation, epoch, observation, receipt_ref }
            guard {
                self.offer_phase == Phase::Offered && self.core_receipt_ref == ""
                && offer_key == self.offer_key && expected_revision == self.revision
                && input_digest == self.input_digest && incarnation == self.incarnation
                && observation == ToolkitCoreObservation::Admitted && receipt_ref != "" && self.revision < u64::MAX
            }
            update {
                self.core_receipt_ref = receipt_ref;
                self.receipt_kind = ToolkitReceiptKind::Found;
                self.revision += 1;
            }
            to CoreObserved
            emit CoreObserved { offer_key: self.offer_key, observation: observation, receipt_ref: self.core_receipt_ref }
            emit PublishOffer { offer_key: self.offer_key, revision: self.revision }
        }
        // A successful Absent read, final for the dispatches it covers: every open or
        // unresolved dispatch was made by an ended process, and this process has dispatched
        // the record (no re-drive or re-offer owed after a restart). It covers dispatches
        // 1..dispatch_count; a consumer checks that none was started after it.
        transition RecordFinalAbsence {
            on input ObserveRead { offer_key, expected_revision, input_digest, incarnation, epoch, observation, receipt_ref }
            guard {
                self.offer_phase == Phase::Offered && self.core_receipt_ref == ""
                && offer_key == self.offer_key && expected_revision == self.revision
                && input_digest == self.input_digest && incarnation == self.incarnation
                && observation == ToolkitCoreObservation::Absent && receipt_ref == ""
                && epoch > 0 && self.last_dispatch_epoch == epoch && self.absence_through < self.dispatch_count
                && for_all(dispatch in self.dispatch_epochs.keys(),
                    self.dispatch_epochs.get_cloned(dispatch).get("value") < epoch)
                && self.revision < u64::MAX
            }
            update { self.absence_through = self.dispatch_count; self.revision += 1; }
            to Offered
            emit CoreObserved { offer_key: self.offer_key, observation: observation, receipt_ref: receipt_ref }
            emit PublishOffer { offer_key: self.offer_key, revision: self.revision }
        }
        // The close's join found the record Offered: durably unresolved, with the closing
        // process's epoch; no dispatch of that process follows. A later process's close may
        // record it again.
        transition RecordCloseUnresolved {
            on input CloseUnresolved { offer_key, expected_revision, close_id, epoch }
            guard {
                self.offer_phase == Phase::Offered && self.core_receipt_ref == ""
                && offer_key == self.offer_key && expected_revision == self.revision && close_id != ""
                && epoch > 0 && self.close_epoch < epoch && epoch >= self.preparation_epoch
                && epoch >= self.last_dispatch_epoch && self.revision < u64::MAX
            }
            update { self.close_epoch = epoch; self.revision += 1; }
            to Offered
            emit PublishOffer { offer_key: self.offer_key, revision: self.revision }
        }
    }
}
