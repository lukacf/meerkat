//! Durable append-only model-routing control history carried on [`Session`].
//!
//! [`Session`]: crate::Session
//!
//! # What this is
//!
//! A committed, ordered, append-only handoff log. A run that asks for a
//! permanent model change cannot perform that change itself: the request and
//! its later resolution are recorded here, on the authoritative session
//! document, so a *different* owner (the runtime pre-admission seam) can act on
//! it after the originating run has durably committed.
//!
//! # What this is NOT
//!
//! This history is **not** routing authority. Nothing reads it to decide which
//! model a provider call uses; the canonical baseline stays owned by the
//! generated `MeerkatMachine` model-routing state. This module owns exactly one
//! thing: the durable *representation* of the handoff, plus the fail-closed
//! well-formedness rules that make an incoherent log unrepresentable.
//!
//! Claim, conflict adjudication, application, and terminality are decided by
//! generated machine authority. The derivations exposed here answer only "what
//! does the committed log literally say", which is why they are named for
//! records rather than for lifecycle status.
//!
//! Archive terminality is generated status: the `MeerkatMachine` handoff
//! lifecycle decides it through `ArchiveUnresolvedModelRoutingHandoff`. The
//! [`SessionModelRoutingControlRecord::ModelRoutingIntentAbandoned`] record
//! here is the durable *mechanical log* of that decision and nothing more.
//! It exists because generated state is in-memory: without a committed record,
//! a request left unresolved by an archived session would still read as
//! `awaiting_decision` after a restart and would be realized by whichever
//! owner revived the document. Appending it is not a caller-held
//! terminalization capability — no authorization token exists, and the only
//! production writer is the single session-archive chokepoint, which appends
//! exactly what generated authority already archived.
//!
//! # Vocabulary
//!
//! The *intent* vocabulary is not new: a committed record carries the existing
//! [`SwitchTurnRequestId`] and [`SwitchTurnIntent`], so the durable log and the
//! live switch-turn control surface cannot drift into two dialects for one
//! fact. The only nouns minted here are the durable record, its history, its
//! record-level disposition, and the append outcome/error.
//!
//! These nouns must never be conflated with input-lifecycle vocabulary: a
//! routing intent survives across runs and is carried by the session document,
//! whereas input lifecycle state is per-input runtime state that is explicitly
//! uncarryable mid-run.
//!
//! # Durable-compatibility decision record
//!
//! Carrying a new durable fact on `Session` has three shapes. They were
//! compared before choosing, because the wrong choice strands released state.
//!
//! * **(a) additive typed field under the current envelope version — CHOSEN.**
//!   The field is `#[serde(default)]` and skipped when empty, so a document
//!   that owes no handoff is byte-identical to one written before this existed.
//!   No released `Session` is rewritten, no importer is needed, and
//!   `SESSION_VERSION` stays 3.
//! * **(b) an exact v3 to v4 importer.** Rejected. Doctrine currently sanctions
//!   exactly one historical importer (the frozen released-0.8.10 lane), and a
//!   second one would have to re-derive every `HeadCanonical` head token to
//!   restamp the version. That is a whole-corpus rewrite bought for a property
//!   (a) already provides.
//! * **(c) move the handoff to a versioned `RuntimeStore` domain.** Rejected on
//!   authority grounds, not convenience: an intent is only actionable when read
//!   from the committed authoritative session, so putting the source anywhere
//!   else makes the session document a mirror of it — two owners for one fact.
//!
//! ## How each representation actually fails closed
//!
//! The mechanism differs per carrier, and saying "`deny_unknown_fields`
//! everywhere" would be wrong:
//!
//! * **Session envelope and WholeBlob** — the decode shape is
//!   `deny_unknown_fields`, so an old binary handed a document that carries an
//!   owed handoff refuses it outright rather than silently dropping it.
//! * **`SessionHead` row** — `SessionHead` is NOT `deny_unknown_fields`, so an
//!   old binary would strip the field silently. What actually protects it is
//!   the head CAS binding: the log participates in the digest-addressed head
//!   preimage, so a stripped or tampered head no longer re-derives its stored
//!   token and reads fail closed as `Corrupted`. That binding is therefore
//!   load-bearing, not decorative.
//! * **History decode** — deserialization routes through [`Self::from_records`],
//!   so duplicate, orphaned, resurrected, or scoped records fail closed in
//!   every embedding carrier, including `SessionHead`.
//! * **Durable save transition** — the committed log is append-only across
//!   saves, enforced by one successor-coherence plus prefix-extension guard in
//!   every whole-session and head-canonical write path. Prefix alone is not
//!   enough: `Requested, Realized, Requested` still extends the first two bytes
//!   conceptually, but resurrects a settled request.
//!
//! # Deliberately deferred (named, not silent)
//!
//! * The claim/pending lifecycle (a request that generated authority has
//!   claimed but not yet realized) is NOT represented here. It is machine
//!   state, not durable document state, and lands with the generated
//!   model-routing lifecycle in Phase 1.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize};

use crate::image_generation::{
    SwitchTurnDenialReason, SwitchTurnDuration, SwitchTurnIntent, SwitchTurnOrigin,
    SwitchTurnRequestId,
};
use crate::lifecycle::identifiers::RunId;

/// One durable record in the append-only model-routing control history.
///
/// Every variant names the exact request identity, the exact originating run,
/// and the full typed intent. Terminal variants repeat the intent rather than
/// referencing it, so a record is self-describing when read in isolation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "record", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum SessionModelRoutingControlRecord {
    /// A run committed a request for a later routing change.
    ///
    /// This is a durable source and outbox entry, never active routing.
    ModelRoutingIntentRequested {
        request_id: SwitchTurnRequestId,
        originating_run_id: RunId,
        intent: SwitchTurnIntent,
    },
    /// The routing change was applied; the resolved identity is recorded.
    ///
    /// `applied_identity` is the identity that was actually installed, resolved
    /// fresh at realization. It is deliberately not constrained to equal
    /// `intent.target_model`: catalog aliasing and provider-side resolution can
    /// legitimately land on a differently-spelled model, and recording what was
    /// really installed is more useful than recording what was asked for. Both
    /// facts are kept so the divergence stays visible.
    ModelRoutingIntentRealized {
        request_id: SwitchTurnRequestId,
        originating_run_id: RunId,
        intent: SwitchTurnIntent,
        applied_identity: Box<crate::SessionLlmIdentity>,
    },
    /// The routing change was refused at its decision point.
    ModelRoutingIntentDenied {
        request_id: SwitchTurnRequestId,
        originating_run_id: RunId,
        intent: SwitchTurnIntent,
        reason: SwitchTurnDenialReason,
    },
    /// The session reached lifecycle terminality before the request resolved.
    ///
    /// Deliberately not spelled as a denial: nothing refused the switch, and
    /// no switch-turn decision point was ever reached. The session's life
    /// ended while the request was still owed, so it can never be realized.
    ///
    /// This is the durable projection of the generated `Archived` handoff
    /// phase. It is only ever appended by the session-archive chokepoint,
    /// after generated authority has archived that exact request, and only
    /// once the archive terminal is durably committed.
    ModelRoutingIntentAbandoned {
        request_id: SwitchTurnRequestId,
        originating_run_id: RunId,
        intent: SwitchTurnIntent,
    },
}

impl SessionModelRoutingControlRecord {
    /// Build the canonical request record a committed `brain_swap` stages.
    ///
    /// The intent is validated here so an unrepresentable durable handoff — one
    /// that is not an until-changed, model-origin switch — cannot be committed
    /// in the first place.
    pub fn request(
        request_id: SwitchTurnRequestId,
        originating_run_id: RunId,
        intent: SwitchTurnIntent,
    ) -> Result<Self, ModelRoutingControlAppendError> {
        if let Some(refusal) = durable_handoff_intent_refusal(request_id, &intent) {
            return Err(refusal);
        }
        Ok(Self::ModelRoutingIntentRequested {
            request_id,
            originating_run_id,
            intent,
        })
    }

    #[must_use]
    pub const fn request_id(&self) -> &SwitchTurnRequestId {
        match self {
            Self::ModelRoutingIntentRequested { request_id, .. }
            | Self::ModelRoutingIntentRealized { request_id, .. }
            | Self::ModelRoutingIntentDenied { request_id, .. }
            | Self::ModelRoutingIntentAbandoned { request_id, .. } => request_id,
        }
    }

    #[must_use]
    pub const fn originating_run_id(&self) -> &RunId {
        match self {
            Self::ModelRoutingIntentRequested {
                originating_run_id, ..
            }
            | Self::ModelRoutingIntentRealized {
                originating_run_id, ..
            }
            | Self::ModelRoutingIntentDenied {
                originating_run_id, ..
            }
            | Self::ModelRoutingIntentAbandoned {
                originating_run_id, ..
            } => originating_run_id,
        }
    }

    #[must_use]
    pub const fn intent(&self) -> &SwitchTurnIntent {
        match self {
            Self::ModelRoutingIntentRequested { intent, .. }
            | Self::ModelRoutingIntentRealized { intent, .. }
            | Self::ModelRoutingIntentDenied { intent, .. }
            | Self::ModelRoutingIntentAbandoned { intent, .. } => intent,
        }
    }

    /// The literal disposition this record expresses.
    #[must_use]
    pub const fn disposition(&self) -> ModelRoutingIntentRecordDisposition {
        match self {
            Self::ModelRoutingIntentRequested { .. } => {
                ModelRoutingIntentRecordDisposition::Requested
            }
            Self::ModelRoutingIntentRealized { .. } => {
                ModelRoutingIntentRecordDisposition::Realized
            }
            Self::ModelRoutingIntentDenied { .. } => ModelRoutingIntentRecordDisposition::Denied,
            Self::ModelRoutingIntentAbandoned { .. } => {
                ModelRoutingIntentRecordDisposition::Abandoned
            }
        }
    }
}

/// Whether an intent is expressible as a durable cross-run handoff.
///
/// Only the until-changed, model-origin switch is: a finite scoped override
/// belongs to the run that requested it and must never outlive it, and a
/// user/system-policy origin has a live control surface of its own.
///
/// This answers SHAPE only. A record additionally needs a target that actually
/// names something — see [`durable_handoff_intent_refusal`], which is what the
/// commit paths call.
#[must_use]
pub fn intent_is_durable_handoff(intent: &SwitchTurnIntent) -> bool {
    matches!(intent.duration, SwitchTurnDuration::UntilChanged)
        && matches!(intent.origin, SwitchTurnOrigin::Model { .. })
}

/// The reason an intent cannot be committed as a durable handoff, if any.
///
/// One function so the append path and the decode path cannot disagree about
/// what "committable" means. An empty target is refused here rather than left
/// to a later layer: a request naming no model is a request nothing can ever
/// realize, and the generated record constructors already refuse it — so
/// admitting one durably would mint state that can never be carried into
/// machine authority.
fn durable_handoff_intent_refusal(
    request_id: SwitchTurnRequestId,
    intent: &SwitchTurnIntent,
) -> Option<ModelRoutingControlAppendError> {
    if !intent_is_durable_handoff(intent) {
        return Some(ModelRoutingControlAppendError::UnsupportedIntent { request_id });
    }
    if intent.target_model.as_str().is_empty() {
        return Some(ModelRoutingControlAppendError::EmptyTargetModel { request_id });
    }
    None
}

/// The literal disposition of the newest record for one request identity.
///
/// This is representation derivation, not lifecycle status: it reports what the
/// committed log says, and deliberately does not decide whether a request is
/// actionable. Actionability additionally requires an exact committed boundary
/// receipt for the originating run and generated machine authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ModelRoutingIntentRecordDisposition {
    /// A request is recorded and no terminal record followed it.
    Requested,
    /// The request was realized.
    Realized,
    /// The request was denied at its decision point.
    Denied,
    /// The session reached lifecycle terminality before the request resolved,
    /// so the request can never be realized.
    Abandoned,
}

impl ModelRoutingIntentRecordDisposition {
    /// Whether this disposition ends the request's recorded life.
    ///
    /// `Requested` is the one waiting disposition; everything else is terminal.
    /// Waiting and terminal are distinct variants rather than a boolean field so
    /// no caller can invent another "maybe done" reading.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        !matches!(self, Self::Requested)
    }

    /// Whether this disposition is still awaiting a decision.
    #[must_use]
    pub const fn is_awaiting_decision(self) -> bool {
        matches!(self, Self::Requested)
    }
}

/// Outcome of appending one record to the committed history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ModelRoutingControlAppendOutcome {
    /// The record extended the log.
    Appended,
    /// An exactly equal record was already committed; the log is unchanged.
    AlreadyRecorded,
}

/// Fail-closed refusals for an incoherent append or persisted log.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum ModelRoutingControlAppendError {
    /// The request identity is already bound to a different originating run or
    /// a different typed intent.
    #[error(
        "model-routing intent {request_id:?} is already committed with a different originating run or target"
    )]
    ConflictingIntent { request_id: SwitchTurnRequestId },
    /// The request identity already reached a terminal disposition.
    #[error("model-routing intent {request_id:?} already terminalized as {disposition:?}")]
    AfterTerminal {
        request_id: SwitchTurnRequestId,
        disposition: ModelRoutingIntentRecordDisposition,
    },
    /// A terminal record was offered for a request that was never committed.
    #[error("model-routing intent {request_id:?} has no committed request to terminalize")]
    UnknownRequest { request_id: SwitchTurnRequestId },
    /// The intent cannot be expressed as a durable cross-run handoff.
    #[error("model-routing intent {request_id:?} is not an until-changed model-origin switch")]
    UnsupportedIntent { request_id: SwitchTurnRequestId },
    /// The intent names no target model.
    ///
    /// A request that names nothing can never be realized, and the generated
    /// record constructors refuse it, so committing one would create durable
    /// state that machine authority can never accept.
    #[error("model-routing intent {request_id:?} names no target model")]
    EmptyTargetModel { request_id: SwitchTurnRequestId },
    /// A persisted log carried the same record twice.
    ///
    /// Silently collapsing it would make decode disagree with the bytes, so an
    /// incoherent persisted log fails closed instead of being normalized.
    #[error("persisted model-routing log carries a duplicate record for intent {request_id:?}")]
    DuplicateRecord { request_id: SwitchTurnRequestId },
}

/// Ordered, append-only committed model-routing control history.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(transparent)]
pub struct SessionModelRoutingControlHistory {
    records: Vec<SessionModelRoutingControlRecord>,
}

impl<'de> Deserialize<'de> for SessionModelRoutingControlHistory {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let records = Vec::<SessionModelRoutingControlRecord>::deserialize(deserializer)?;
        Self::from_records(records).map_err(serde::de::Error::custom)
    }
}

impl SessionModelRoutingControlHistory {
    /// An empty history.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            records: Vec::new(),
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Every committed record, oldest first.
    #[must_use]
    pub fn records(&self) -> &[SessionModelRoutingControlRecord] {
        &self.records
    }

    /// Rebuild a history from persisted records, revalidating coherence.
    ///
    /// Deserialization and every store-side materialization route through this,
    /// so a hand-edited or corrupted log cannot enter memory as authority.
    ///
    /// Strict on purpose: a duplicate in a persisted vector is incoherent input
    /// and is refused. Collapsing it would make the decoded value disagree with
    /// the bytes it came from, which is the fail-OPEN twin of everything else
    /// this module does.
    pub fn from_records(
        records: Vec<SessionModelRoutingControlRecord>,
    ) -> Result<Self, ModelRoutingControlAppendError> {
        validate_record_sequence(&records)?;
        Ok(Self { records })
    }

    pub(crate) fn validate_coherent(&self) -> Result<(), ModelRoutingControlAppendError> {
        validate_record_sequence(&self.records)
    }

    /// Whether this log is a prefix extension of `predecessor`.
    ///
    /// The durable save boundary demands this: a successor may only add
    /// records, never drop, reorder, or rewrite a committed one.
    #[must_use]
    pub fn extends(&self, predecessor: &Self) -> bool {
        self.records.len() >= predecessor.records.len()
            && self.records[..predecessor.records.len()] == predecessor.records[..]
    }

    /// The newest record committed for `request_id`, if any.
    #[must_use]
    pub fn latest_record_for(
        &self,
        request_id: &SwitchTurnRequestId,
    ) -> Option<&SessionModelRoutingControlRecord> {
        self.records
            .iter()
            .rev()
            .find(|record| record.request_id() == request_id)
    }

    /// The literal disposition of `request_id` in the committed log.
    #[must_use]
    pub fn disposition_of(
        &self,
        request_id: &SwitchTurnRequestId,
    ) -> Option<ModelRoutingIntentRecordDisposition> {
        self.latest_record_for(request_id)
            .map(SessionModelRoutingControlRecord::disposition)
    }

    /// Every committed request that has not yet reached a terminal record.
    ///
    /// These are candidates only. A candidate becomes actionable exclusively
    /// when generated authority admits it and an exact committed boundary
    /// receipt proves its originating run committed successfully.
    pub fn awaiting_decision(
        &self,
    ) -> impl Iterator<Item = &SessionModelRoutingControlRecord> + '_ {
        self.records.iter().filter(|record| {
            record.disposition().is_awaiting_decision()
                && self
                    .latest_record_for(record.request_id())
                    .is_some_and(|latest| latest.disposition().is_awaiting_decision())
        })
    }

    /// Every committed request whose newest record is a terminal.
    ///
    /// The mirror of [`Self::awaiting_decision`], and it exists for the crash
    /// window that seam cannot see: a terminal that persisted before generated
    /// authority recorded it leaves a request that owes no work durably, yet is
    /// still `Imported`/`Claimed` in the machine. Such a request is invisible
    /// to `awaiting_decision` — it is settled — so without this it would stay
    /// half-finished in generated state for the life of the runtime.
    ///
    /// Yields the newest record per request, so the caller reads the exact
    /// terminal facts (run, intent, installed identity or refusal reason)
    /// rather than inferring them.
    pub fn settled_terminals(
        &self,
    ) -> impl Iterator<Item = &SessionModelRoutingControlRecord> + '_ {
        self.records.iter().filter(|record| {
            record.disposition().is_terminal()
                && self
                    .latest_record_for(record.request_id())
                    .is_some_and(|latest| std::ptr::eq(latest, *record))
        })
    }

    /// Build a history WITHOUT revalidating the sequence.
    ///
    /// Test-support only, and deliberately named for it. Some refusals can only
    /// be proved against a log the ordinary paths would never produce — a
    /// terminal bound to a different originating run, say, which `append`
    /// refuses as a conflicting intent. Without a way to stage that, the
    /// downstream refusal under test would be unreachable and its test would
    /// pass for the wrong reason.
    #[cfg(any(test, feature = "test-support"))]
    #[must_use]
    pub fn from_records_unchecked_for_tests(
        records: Vec<SessionModelRoutingControlRecord>,
    ) -> Self {
        Self { records }
    }

    /// Append one record, enforcing the log's well-formedness rules.
    ///
    /// Exactly-equal duplicates are idempotent; anything that would make the
    /// committed log ambiguous is refused with a typed error.
    pub fn append(
        &mut self,
        record: SessionModelRoutingControlRecord,
    ) -> Result<ModelRoutingControlAppendOutcome, ModelRoutingControlAppendError> {
        let outcome =
            classify_record_transition(self.latest_record_for(record.request_id()), &record)?;
        if matches!(outcome, ModelRoutingControlAppendOutcome::Appended) {
            self.records.push(record);
        }
        Ok(outcome)
    }
}

fn classify_record_transition(
    previous: Option<&SessionModelRoutingControlRecord>,
    record: &SessionModelRoutingControlRecord,
) -> Result<ModelRoutingControlAppendOutcome, ModelRoutingControlAppendError> {
    let request_id = *record.request_id();
    if let Some(refusal) = durable_handoff_intent_refusal(request_id, record.intent()) {
        return Err(refusal);
    }
    let Some(previous) = previous else {
        if record.disposition().is_terminal() {
            return Err(ModelRoutingControlAppendError::UnknownRequest { request_id });
        }
        return Ok(ModelRoutingControlAppendOutcome::Appended);
    };
    if previous == record {
        return Ok(ModelRoutingControlAppendOutcome::AlreadyRecorded);
    }
    if previous.originating_run_id() != record.originating_run_id()
        || previous.intent() != record.intent()
    {
        return Err(ModelRoutingControlAppendError::ConflictingIntent { request_id });
    }
    let disposition = previous.disposition();
    if disposition.is_terminal() {
        return Err(ModelRoutingControlAppendError::AfterTerminal {
            request_id,
            disposition,
        });
    }
    if record.disposition().is_awaiting_decision() {
        return Err(ModelRoutingControlAppendError::ConflictingIntent { request_id });
    }
    Ok(ModelRoutingControlAppendOutcome::Appended)
}

fn validate_record_sequence(
    records: &[SessionModelRoutingControlRecord],
) -> Result<(), ModelRoutingControlAppendError> {
    let mut latest = BTreeMap::<SwitchTurnRequestId, &SessionModelRoutingControlRecord>::new();
    for record in records {
        let request_id = *record.request_id();
        match classify_record_transition(latest.get(&request_id).copied(), record)? {
            ModelRoutingControlAppendOutcome::Appended => {
                latest.insert(request_id, record);
            }
            ModelRoutingControlAppendOutcome::AlreadyRecorded => {
                return Err(ModelRoutingControlAppendError::DuplicateRecord { request_id });
            }
        }
    }
    Ok(())
}
