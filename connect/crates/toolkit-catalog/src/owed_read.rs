//! The driver's owed read by an offer's own qualified key: which of Core's two reads it issues
//! and what it feeds the offer owner.
//!
//! Core's live read (`MeerkatMachine::with_admitted_original`) takes the session's mutation gate
//! and then its driver lock, so it is ordered after any admission write in flight on the
//! session: its NotAdmitted is final for a lost dispatch of this process. Core's durable read
//! (`MeerkatMachine::durable_admitted_original`) takes no gate and can answer NotAdmitted while
//! such a write is in flight: its NotAdmitted counts only an ended process's dispatches. The
//! driver reads durably only while the live read cannot answer: its own admission write is in
//! flight (the dispatch's call returned the retryable refusal of a write past its commit outcome
//! deadline, and the session degraded until it reloads), or the session is not live on this
//! machine: then it never wakes the session to read (lazy loading). After a restart, until this
//! process dispatches the record (which loads the session), every recorded dispatch is an ended
//! process's, so the durable read finds what the live read would; on a session closed or
//! retired within the process it counts less (never a lost dispatch of this process). A durable
//! read that is not final owes a live read. A held claim on a live session with no write in
//! flight is therefore settled by the live read, never by a loop of durable reads.
//!
//! The input carries the record's revision from when the read started, so the owner refuses a
//! read whose record changed under it (a later lost answer included): the stale read is
//! discarded by the owner's revision check.

use crate::{
    ToolkitCoreObservation, ToolkitCoreRead, ToolkitOfferInput, ToolkitOriginalOfferMachineState,
};

/// What Core's live read answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LiveReadAnswer {
    /// The exact original's admission, with Core's receipt.
    Admitted { receipt_ref: String },
    /// The current registration has no admission under the original's qualified key.
    NotAdmitted,
    /// An admission under the key that is not the exact original's.
    Conflict,
    /// The registration, its gate or the live attachment cannot be shown current now.
    Unavailable,
}

/// What Core's durable read answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DurableReadAnswer {
    /// The exact original's admission row, with Core's receipt.
    Admitted { receipt_ref: String },
    /// The session's durable runtime holds no row under the original's qualified key.
    NotAdmitted,
    /// A row under the key that is not the exact original's admission.
    Conflict,
    /// The machine's store holds no durable runtime for the session.
    UnknownRuntime,
    /// The durable evidence cannot be read now.
    Unavailable,
}

/// The driver's port onto Core's two reads by an offer's own qualified key.
pub trait CoreReads {
    fn live_read(&mut self, offer_key: &str) -> LiveReadAnswer;
    fn durable_read(&mut self, offer_key: &str) -> DurableReadAnswer;
}

/// Whether Core's live read can answer for the record's session now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveReadState {
    /// The session is live on this machine and no admission write of this process holds its
    /// gate.
    Answers,
    /// An admission write of this process is in flight after its call returned: the session is
    /// degraded until it reloads, and the live read answers Unavailable meanwhile.
    WriteInFlight,
    /// The session is not live on this machine (after a restart, before it loads, or closed or
    /// retired within the process): the driver reads durably and never wakes it to read.
    NotLive,
}

/// Issue the read owed for one Offered record, from the owner state taken when the read starts,
/// and map Core's answer to the owner's read input. `None` when the answer is no owner input
/// (Unavailable, Conflict or an unknown runtime: never an absence): the read stays owed.
pub fn owed_read(
    reads: &mut dyn CoreReads,
    live: LiveReadState,
    offer: &ToolkitOriginalOfferMachineState,
    epoch: u64,
) -> Option<ToolkitOfferInput> {
    let (read, observation, receipt_ref) = match live {
        LiveReadState::Answers => match reads.live_read(&offer.offer_key) {
            LiveReadAnswer::Admitted { receipt_ref } => (
                ToolkitCoreRead::Live,
                ToolkitCoreObservation::Admitted,
                receipt_ref,
            ),
            LiveReadAnswer::NotAdmitted => (
                ToolkitCoreRead::Live,
                ToolkitCoreObservation::Absent,
                String::new(),
            ),
            LiveReadAnswer::Conflict | LiveReadAnswer::Unavailable => return None,
        },
        LiveReadState::WriteInFlight | LiveReadState::NotLive => {
            match reads.durable_read(&offer.offer_key) {
                DurableReadAnswer::Admitted { receipt_ref } => (
                    ToolkitCoreRead::Durable,
                    ToolkitCoreObservation::Admitted,
                    receipt_ref,
                ),
                DurableReadAnswer::NotAdmitted => (
                    ToolkitCoreRead::Durable,
                    ToolkitCoreObservation::Absent,
                    String::new(),
                ),
                DurableReadAnswer::Conflict
                | DurableReadAnswer::UnknownRuntime
                | DurableReadAnswer::Unavailable => return None,
            }
        }
    };
    Some(ToolkitOfferInput::ObserveRead {
        offer_key: offer.offer_key.clone(),
        expected_revision: offer.revision,
        input_digest: offer.input_digest.clone(),
        incarnation: offer.incarnation.clone(),
        epoch,
        observation,
        receipt_ref,
        read,
    })
}
