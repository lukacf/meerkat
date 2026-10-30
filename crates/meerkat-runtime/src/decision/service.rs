//! [`DurableDecisionService`]: the owner of durable decisions over a
//! [`DecisionStore`].
//!
//! The service holds no decision state. Each decision is one stored row: its
//! immutable descriptor, the state of its generated `DurableDecisionMachine`
//! and the records the service keeps beside it (presentations, the outcome
//! and its evidence, an execution refusal, the notice receipt). Every call
//! reads the row it needs, reads the host clock once (after its reads),
//! rebuilds the machine from the row (the machine's invariants refuse a torn
//! row) and applies one input. It decides only from the effects of that
//! input, and writes a changed row back with a compare-and-swap on the row's
//! revision. On a conflict it reloads the row and applies the input once
//! more; a second conflict is [`DecisionError::Conflict`].
//!
//! A row that cannot be decoded, or that is torn (its machine state, its
//! descriptor and its evidence do not agree), is unreadable for that
//! decision only: it is listed as unreadable and nothing executes it.

use std::fmt;
use std::sync::Arc;

use meerkat_core::approval::consent::ConsentEligibility;
use meerkat_core::approval::decision::{
    ApproverId, AttestedDecision, AttributedReview, ConsentProvenance, DECISION_PAGE_MAX,
    DECISION_PRESENTATIONS_MAX, DecisionContractError, DecisionDescriptor, DecisionHoldRequest,
    DecisionId, DecisionListFilter, DecisionMachineState, DecisionNotice, DecisionOperationId,
    DecisionOutcome, DecisionPage, DecisionPhase, DecisionPrincipalKey, DecisionReference,
    DecisionReviewVerdict, DecisionRow, DecisionStage, DecisionStore, DecisionTier,
    PresentationAudience, PresentationRecord, Sha256Digest, StoredDecision, UnreadableDecision,
};
use meerkat_core::approval::{ApprovalConsentDeliveryState, ApprovalStoreError};
use serde::{Deserialize, Serialize};

use super::dsl::{
    DecisionDigest, DecisionParty, DurableDecisionEffect, DurableDecisionInput,
    DurableDecisionMachineAuthority, DurableDecisionMachineMutator, DurableDecisionMachineState,
    DurableDecisionNotice, DurableDecisionPhase, DurableDecisionRefusal,
    DurableDecisionReviewVerdict, DurableDecisionStage,
};
use super::permit::DecisionExecutionPermit;
use super::{DecisionClock, DecisionWriteMode, SystemDecisionClock};

/// Why a decision refused an input. Each is the machine's typed refusal;
/// the input changed nothing, except [`Self::Expired`], which a touch at or
/// after the deadline answers with the expiry it recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum DecisionRefusal {
    /// The hold's deadline is not after the hold's clock reading.
    InvalidWindow,
    /// The hold names the requester as its own reviewer.
    SelfReview,
    /// The decision is held with another binding.
    HoldConflict,
    /// The decision is no longer open: the first valid decision won.
    NotOpen,
    /// The deadline passed; the touch recorded the expiry.
    Expired,
    /// The presented digest is not the held subject digest.
    SubjectChanged,
    /// The presentation did not cover the whole held subject, or none
    /// reached the deciding party.
    PresentationIncomplete,
    /// The presentation was shown to a party that does not decide now.
    WrongAudience,
    /// A human decision while the review stage is open.
    ReviewStage,
    /// A review verdict while the human stage is open.
    HumanStage,
    /// The decision carries no attested evidence.
    EvidenceRequired,
    /// The decider does not act for the approver id, or is not currently
    /// eligible.
    NotEligible,
    /// The requester tried to decide its own request.
    SelfApproval,
    /// The verdict is not the named reviewer's.
    WrongReviewer,
    /// Only the requester or the host may withdraw.
    NotRequester,
    /// The decision is not approved.
    NotApproved,
    /// The execution names another operation or request.
    BindingMismatch,
    /// No execution permit is outstanding.
    NotAuthorized,
    /// The deadline passed while a permit is outstanding: the permit must be
    /// settled (executed, released or refused) first.
    SettlementOwed,
    /// No notice is owed, or the receipt contradicts the settled one.
    NotOwed,
}

impl DecisionRefusal {
    /// The refusal's stable token (`subject_changed`, ...).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvalidWindow => "invalid_window",
            Self::SelfReview => "self_review",
            Self::HoldConflict => "hold_conflict",
            Self::NotOpen => "not_open",
            Self::Expired => "expired",
            Self::SubjectChanged => "subject_changed",
            Self::PresentationIncomplete => "presentation_incomplete",
            Self::WrongAudience => "wrong_audience",
            Self::ReviewStage => "review_stage",
            Self::HumanStage => "human_stage",
            Self::EvidenceRequired => "evidence_required",
            Self::NotEligible => "not_eligible",
            Self::SelfApproval => "self_approval",
            Self::WrongReviewer => "wrong_reviewer",
            Self::NotRequester => "not_requester",
            Self::NotApproved => "not_approved",
            Self::BindingMismatch => "binding_mismatch",
            Self::NotAuthorized => "not_authorized",
            Self::SettlementOwed => "settlement_owed",
            Self::NotOwed => "not_owed",
        }
    }
}

impl From<DurableDecisionRefusal> for DecisionRefusal {
    fn from(refusal: DurableDecisionRefusal) -> Self {
        match refusal {
            DurableDecisionRefusal::InvalidWindow => Self::InvalidWindow,
            DurableDecisionRefusal::SelfReview => Self::SelfReview,
            DurableDecisionRefusal::HoldConflict => Self::HoldConflict,
            DurableDecisionRefusal::NotOpen => Self::NotOpen,
            DurableDecisionRefusal::Expired => Self::Expired,
            DurableDecisionRefusal::SubjectChanged => Self::SubjectChanged,
            DurableDecisionRefusal::PresentationIncomplete => Self::PresentationIncomplete,
            DurableDecisionRefusal::WrongAudience => Self::WrongAudience,
            DurableDecisionRefusal::ReviewStage => Self::ReviewStage,
            DurableDecisionRefusal::HumanStage => Self::HumanStage,
            DurableDecisionRefusal::EvidenceRequired => Self::EvidenceRequired,
            DurableDecisionRefusal::NotEligible => Self::NotEligible,
            DurableDecisionRefusal::SelfApproval => Self::SelfApproval,
            DurableDecisionRefusal::WrongReviewer => Self::WrongReviewer,
            DurableDecisionRefusal::NotRequester => Self::NotRequester,
            DurableDecisionRefusal::NotApproved => Self::NotApproved,
            DurableDecisionRefusal::BindingMismatch => Self::BindingMismatch,
            DurableDecisionRefusal::NotAuthorized => Self::NotAuthorized,
            DurableDecisionRefusal::SettlementOwed => Self::SettlementOwed,
            DurableDecisionRefusal::NotOwed => Self::NotOwed,
        }
    }
}

/// A durable decision call failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum DecisionError {
    /// No decision has this id.
    #[error("decision {0} not found")]
    NotFound(DecisionId),
    /// The decision's row cannot be read: undecodable, or torn. Nothing
    /// executes it.
    #[error("decision {decision_id} is unreadable: {reason}")]
    Unreadable { decision_id: String, reason: String },
    /// The decision refused the input.
    #[error("decision {decision_id} refused the input: {}", .refusal.as_str())]
    Refused {
        decision_id: DecisionId,
        refusal: DecisionRefusal,
    },
    /// Another owner changed the decision twice while this call applied its
    /// input.
    #[error("decision {0} changed concurrently")]
    Conflict(DecisionId),
    /// The decision's write may have committed.
    #[error("the outcome of the write of decision {0} is unknown")]
    CommitUnknown(DecisionId),
    /// The owner serves a candidate launch, which writes no decision.
    #[error("a candidate launch writes no decision")]
    CandidateLaunch,
    /// The decision store cannot serve the call; nothing was written.
    #[error("the decision store is unavailable: {0}")]
    Unavailable(String),
    /// A value of the call is not valid.
    #[error("invalid decision request: {0}")]
    Invalid(#[from] DecisionContractError),
    /// A notice receipt must record an admission or a terminal failure.
    #[error("a notice receipt must be admitted or failed")]
    NotAReceipt,
    /// The host clock gave no time.
    #[error("the decision clock is unavailable")]
    Clock,
    /// The generated machine refused to resolve the input.
    #[error("decision machine failure: {0}")]
    Machine(String),
}

/// Who withdraws a decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecisionWithdrawer {
    /// The decision's requester.
    Requester(DecisionPrincipalKey),
    /// The host.
    Host,
}

/// One page of decisions, each projected at the clock reading of the read
/// (a decision past its deadline reads as expired; the projection is not
/// recorded), and the rows of the page this binary cannot read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DecisionListing {
    pub decisions: Vec<StoredDecision>,
    pub unreadable: Vec<UnreadableDecision>,
    /// The id to read the next page after, when there may be more rows.
    pub next_after: Option<String>,
}

#[derive(Clone)]
enum Backing {
    Store(Arc<dyn DecisionStore>),
    Unavailable(String),
}

/// The owner of durable decisions over one [`DecisionStore`].
///
/// Opened once per persistence, beside its approval issuer, and shared by
/// every surface of that persistence. Cloning shares the store.
#[derive(Clone)]
pub struct DurableDecisionService {
    backing: Backing,
    clock: Arc<dyn DecisionClock>,
    mode: DecisionWriteMode,
}

impl fmt::Debug for DurableDecisionService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DurableDecisionService")
            .field("available", &matches!(self.backing, Backing::Store(_)))
            .field("persistent", &self.is_persistent())
            .field("mode", &self.mode)
            .finish_non_exhaustive()
    }
}

/// The machine's party key of an approver id.
fn approver_party(approver_id: &ApproverId) -> DecisionParty {
    DecisionParty(format!("approver:{approver_id}"))
}

/// The machine's party key of a principal.
fn principal_party(key: &DecisionPrincipalKey) -> DecisionParty {
    DecisionParty(key.as_str().to_owned())
}

/// The machine's party key of the host, as a withdrawer.
fn host_party() -> DecisionParty {
    DecisionParty("host:".to_owned())
}

fn machine_digest(digest: &Sha256Digest) -> DecisionDigest {
    DecisionDigest(digest.as_str().to_owned())
}

fn audience_party(audience: &PresentationAudience) -> DecisionParty {
    match audience {
        PresentationAudience::Approver { approver_id } => approver_party(approver_id),
        PresentationAudience::Reviewer { reviewer } => principal_party(reviewer),
    }
}

/// The generated machine's state of a stored state.
fn machine_state(state: &DecisionMachineState) -> DurableDecisionMachineState {
    DurableDecisionMachineState {
        lifecycle_phase: match state.phase {
            DecisionPhase::Open => DurableDecisionPhase::Open,
            DecisionPhase::Approved => DurableDecisionPhase::Approved,
            DecisionPhase::Rejected => DurableDecisionPhase::Rejected,
            DecisionPhase::Expired => DurableDecisionPhase::Expired,
            DecisionPhase::Withdrawn => DurableDecisionPhase::Withdrawn,
            DecisionPhase::Executed => DurableDecisionPhase::Executed,
            DecisionPhase::ExecutionRefused => DurableDecisionPhase::ExecutionRefused,
        },
        decision_id: state.decision_id.clone(),
        deadline_at_ms: state.deadline_at_ms,
        subject_digest: DecisionDigest(state.subject_digest.clone()),
        operation_id: state.operation_id.clone(),
        request_digest: DecisionDigest(state.request_digest.clone()),
        requester: DecisionParty(state.requester.clone()),
        approver_id: DecisionParty(state.approver_id.clone()),
        reviewer: state.reviewer.clone().map(DecisionParty),
        stage: match state.stage {
            DecisionStage::Human => DurableDecisionStage::Human,
            DecisionStage::Review => DurableDecisionStage::Review,
        },
        presented_to_approver: state.presented_to_approver,
        presented_to_reviewer: state.presented_to_reviewer,
        evidence_digest: state.evidence_digest.clone(),
        decided_at_ms: state.decided_at_ms,
        ended_at_ms: state.ended_at_ms,
        permit_issued: state.permit_issued,
        executed_request_digest: state.executed_request_digest.clone().map(DecisionDigest),
        notice: match state.notice {
            DecisionNotice::NotRequired => DurableDecisionNotice::NotRequired,
            DecisionNotice::NotDue => DurableDecisionNotice::NotDue,
            DecisionNotice::Owed => DurableDecisionNotice::Owed,
            DecisionNotice::Delivered => DurableDecisionNotice::Delivered,
            DecisionNotice::Failed => DurableDecisionNotice::Failed,
        },
    }
}

/// The stored state of a generated machine state. A held decision is never
/// unheld.
fn stored_state(
    state: &DurableDecisionMachineState,
) -> Result<DecisionMachineState, DecisionError> {
    let phase = match state.lifecycle_phase {
        DurableDecisionPhase::Unheld => {
            return Err(DecisionError::Machine(
                "an unheld decision has no state to store".to_owned(),
            ));
        }
        DurableDecisionPhase::Open => DecisionPhase::Open,
        DurableDecisionPhase::Approved => DecisionPhase::Approved,
        DurableDecisionPhase::Rejected => DecisionPhase::Rejected,
        DurableDecisionPhase::Expired => DecisionPhase::Expired,
        DurableDecisionPhase::Withdrawn => DecisionPhase::Withdrawn,
        DurableDecisionPhase::Executed => DecisionPhase::Executed,
        DurableDecisionPhase::ExecutionRefused => DecisionPhase::ExecutionRefused,
    };
    Ok(DecisionMachineState {
        phase,
        decision_id: state.decision_id.clone(),
        deadline_at_ms: state.deadline_at_ms,
        subject_digest: state.subject_digest.0.clone(),
        operation_id: state.operation_id.clone(),
        request_digest: state.request_digest.0.clone(),
        requester: state.requester.0.clone(),
        approver_id: state.approver_id.0.clone(),
        reviewer: state.reviewer.as_ref().map(|reviewer| reviewer.0.clone()),
        stage: match state.stage {
            DurableDecisionStage::Human => DecisionStage::Human,
            DurableDecisionStage::Review => DecisionStage::Review,
        },
        presented_to_approver: state.presented_to_approver,
        presented_to_reviewer: state.presented_to_reviewer,
        evidence_digest: state.evidence_digest.clone(),
        decided_at_ms: state.decided_at_ms,
        ended_at_ms: state.ended_at_ms,
        permit_issued: state.permit_issued,
        executed_request_digest: state
            .executed_request_digest
            .as_ref()
            .map(|digest| digest.0.clone()),
        notice: match state.notice {
            DurableDecisionNotice::NotRequired => DecisionNotice::NotRequired,
            DurableDecisionNotice::NotDue => DecisionNotice::NotDue,
            DurableDecisionNotice::Owed => DecisionNotice::Owed,
            DurableDecisionNotice::Delivered => DecisionNotice::Delivered,
            DurableDecisionNotice::Failed => DecisionNotice::Failed,
        },
    })
}

/// A row read and checked, with its machine rebuilt.
struct LoadedDecision {
    row: StoredDecision,
    authority: DurableDecisionMachineAuthority,
}

/// Check `row`, stored under `stored_under`, and rebuild its machine. A row
/// whose descriptor, machine state and evidence do not agree is torn.
fn check_row(stored_under: &str, row: StoredDecision) -> Result<LoadedDecision, DecisionError> {
    let torn = |reason: String| DecisionError::Unreadable {
        decision_id: stored_under.to_owned(),
        reason,
    };
    if row.decision_id.as_str() != stored_under {
        return Err(torn(format!(
            "the row names the decision {}",
            row.decision_id
        )));
    }
    let descriptor = &row.descriptor;
    if descriptor.decision_id() != row.decision_id {
        return Err(torn(
            "the descriptor's kind and subject key derive another id".to_owned(),
        ));
    }
    if descriptor.rendering.subject_digest() != descriptor.subject_digest {
        return Err(torn(
            "the subject digest is not the rendering's digest".to_owned(),
        ));
    }
    let state = &row.state;
    let reviewer = descriptor.reviewer.as_ref().map(principal_party);
    let binding = [
        (state.decision_id == row.decision_id.as_str(), "decision id"),
        (
            state.deadline_at_ms == descriptor.deadline_at_ms,
            "deadline",
        ),
        (
            state.subject_digest == descriptor.subject_digest.as_str(),
            "subject digest",
        ),
        (
            state.operation_id == descriptor.operation_id.as_str(),
            "operation id",
        ),
        (
            state.request_digest == descriptor.request_digest.as_str(),
            "request digest",
        ),
        (
            state.requester == descriptor.requester.as_str(),
            "requester",
        ),
        (
            state.approver_id == approver_party(&descriptor.approver_id).0,
            "approver id",
        ),
        (
            state.reviewer.as_deref() == reviewer.as_ref().map(|party| party.0.as_str()),
            "reviewer",
        ),
    ];
    if let Some((_, field)) = binding.iter().find(|(agrees, _)| !agrees) {
        return Err(torn(format!(
            "the machine state's {field} is not the descriptor's"
        )));
    }
    match (&state.evidence_digest, &row.outcome) {
        (None, None) => {}
        (Some(evidence_digest), Some(outcome)) => {
            let computed = outcome
                .evidence
                .digest()
                .map_err(|error| torn(error.to_string()))?;
            if computed.as_str() != evidence_digest || outcome.tier != outcome.evidence.tier() {
                return Err(torn(
                    "the stored evidence is not the evidence the decision binds".to_owned(),
                ));
            }
        }
        _ => {
            return Err(torn(
                "the outcome and the machine's evidence digest disagree".to_owned(),
            ));
        }
    }
    let authority = DurableDecisionMachineAuthority::recover_from_state(machine_state(state))
        .map_err(|error| torn(error.to_string()))?;
    Ok(LoadedDecision { row, authority })
}

/// Apply `input` to a fork of the loaded machine: the effects, and the
/// stored state the machine left.
fn transition(
    loaded: &LoadedDecision,
    input: DurableDecisionInput,
) -> Result<(Vec<DurableDecisionEffect>, DecisionMachineState), DecisionError> {
    let mut authority = loaded.authority.fork();
    let effects = DurableDecisionMachineMutator::apply(&mut authority, input)
        .map_err(|error| {
            DecisionError::Machine(format!("decision {}: {error}", loaded.row.decision_id))
        })?
        .into_effects();
    Ok((effects, stored_state(authority.state())?))
}

/// The machine's refusal among `effects`, if any.
fn refusal_of(effects: &[DurableDecisionEffect]) -> Option<DecisionRefusal> {
    effects.iter().find_map(|effect| match effect {
        DurableDecisionEffect::Refused { reason, .. } => Some(DecisionRefusal::from(*reason)),
        _ => None,
    })
}

/// The row after `loaded` with the machine state `state` and the record
/// changes `change`, at the next revision; `None` when nothing changed.
fn successor(
    loaded: &LoadedDecision,
    state: DecisionMachineState,
    change: impl FnOnce(&mut StoredDecision),
) -> Option<StoredDecision> {
    let mut next = loaded.row.clone();
    next.state = state;
    change(&mut next);
    if next == loaded.row {
        return None;
    }
    next.revision = loaded.row.revision.saturating_add(1);
    Some(next)
}

/// Keep `presentation` as the latest one of its audience.
fn keep_presentation(row: &mut StoredDecision, presentation: PresentationRecord) {
    row.presentations
        .retain(|kept| kept.audience != presentation.audience);
    row.presentations.push(presentation);
    let excess = row
        .presentations
        .len()
        .saturating_sub(DECISION_PRESENTATIONS_MAX);
    if excess > 0 {
        row.presentations = row.presentations.split_off(excess);
    }
}

/// How a call answers once its row is settled.
enum Answer {
    /// With the decision as it stands.
    Row,
    /// With the machine's refusal (after recording what the input changed).
    Refused(DecisionRefusal),
}

/// One input applied to a loaded row: the row to write, and the answer.
struct Step {
    next: Option<StoredDecision>,
    answer: Answer,
}

impl Step {
    /// The step of `effects` and the row `next` they leave.
    fn of(effects: &[DurableDecisionEffect], next: Option<StoredDecision>) -> Self {
        Self {
            next,
            answer: refusal_of(effects).map_or(Answer::Row, Answer::Refused),
        }
    }
}

fn read_error(error: ApprovalStoreError) -> DecisionError {
    DecisionError::Unavailable(error.to_string())
}

fn write_error(decision_id: &DecisionId, error: ApprovalStoreError) -> DecisionError {
    match error {
        ApprovalStoreError::NotCommitted(reason) => DecisionError::Unavailable(reason),
        ApprovalStoreError::Contended => {
            DecisionError::Unavailable("the decision store is held by another owner".to_owned())
        }
        ApprovalStoreError::Conflict => DecisionError::Conflict(decision_id.clone()),
        ApprovalStoreError::CommitUnknown(_) => DecisionError::CommitUnknown(decision_id.clone()),
    }
}

fn unreadable(row: UnreadableDecision) -> DecisionError {
    DecisionError::Unreadable {
        decision_id: row.decision_id,
        reason: row.reason,
    }
}

/// The hold input of `descriptor`.
fn hold_input(
    decision_id: &DecisionId,
    descriptor: &DecisionDescriptor,
    now_ms: u64,
    notice_routable: bool,
) -> DurableDecisionInput {
    DurableDecisionInput::Hold {
        decision_id: decision_id.as_str().to_owned(),
        now_ms,
        deadline_at_ms: descriptor.deadline_at_ms,
        subject_digest: machine_digest(&descriptor.subject_digest),
        operation_id: descriptor.operation_id.as_str().to_owned(),
        request_digest: machine_digest(&descriptor.request_digest),
        requester: principal_party(&descriptor.requester),
        approver_id: approver_party(&descriptor.approver_id),
        reviewer: descriptor.reviewer.as_ref().map(principal_party),
        notice_routable,
    }
}

/// The permit of an approved row: its held binding, its deadline and the
/// consent it decided.
fn permit_of(row: &StoredDecision) -> Result<DecisionExecutionPermit, DecisionError> {
    let (Some(outcome), Some(decided_at_ms)) = (&row.outcome, row.state.decided_at_ms) else {
        return Err(DecisionError::Machine(format!(
            "decision {} was authorized without an outcome",
            row.decision_id
        )));
    };
    Ok(DecisionExecutionPermit::new(
        row.decision_id.clone(),
        row.descriptor.operation_id.clone(),
        row.descriptor.request_digest.clone(),
        row.descriptor.deadline_at_ms,
        ConsentProvenance::Decided {
            decision_id: row.decision_id.clone(),
            subject_digest: row.descriptor.subject_digest.clone(),
            renderer: row.descriptor.renderer.clone(),
            tier: outcome.tier,
            approver_id: row.descriptor.approver_id.clone(),
            decided_at_ms,
            evidence: outcome.evidence.clone(),
        },
    ))
}

impl DurableDecisionService {
    /// The owner of the decisions in `store`, on the host's wall clock.
    /// Opening reads nothing.
    #[must_use]
    pub fn new(store: Arc<dyn DecisionStore>) -> Self {
        Self::with_clock(store, Arc::new(SystemDecisionClock))
    }

    /// The owner of the decisions in `store`, on `clock`.
    #[must_use]
    pub fn with_clock(store: Arc<dyn DecisionStore>, clock: Arc<dyn DecisionClock>) -> Self {
        Self {
            backing: Backing::Store(store),
            clock,
            mode: DecisionWriteMode::Admitted,
        }
    }

    /// An owner that refuses every call as [`DecisionError::Unavailable`]
    /// with `reason`: the owner of a persistence with no durable decision
    /// store.
    #[must_use]
    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self {
            backing: Backing::Unavailable(reason.into()),
            clock: Arc::new(SystemDecisionClock),
            mode: DecisionWriteMode::Admitted,
        }
    }

    /// This owner narrowed to a candidate launch: it reads, and refuses
    /// every write as [`DecisionError::CandidateLaunch`]. There is no way
    /// back: the narrowed owner cannot be widened.
    #[must_use]
    pub fn for_candidate_launch(&self) -> Self {
        Self {
            mode: DecisionWriteMode::CandidateLaunch,
            ..self.clone()
        }
    }

    #[must_use]
    pub fn write_mode(&self) -> DecisionWriteMode {
        self.mode
    }

    /// Whether decisions survive a restart.
    #[must_use]
    pub fn is_persistent(&self) -> bool {
        match &self.backing {
            Backing::Store(store) => store.is_persistent(),
            Backing::Unavailable(_) => false,
        }
    }

    fn store(&self) -> Result<&Arc<dyn DecisionStore>, DecisionError> {
        match &self.backing {
            Backing::Store(store) => Ok(store),
            Backing::Unavailable(reason) => Err(DecisionError::Unavailable(reason.clone())),
        }
    }

    fn require_writes(&self) -> Result<(), DecisionError> {
        match self.mode {
            DecisionWriteMode::Admitted => Ok(()),
            DecisionWriteMode::CandidateLaunch => Err(DecisionError::CandidateLaunch),
        }
    }

    fn now_ms(&self) -> Result<u64, DecisionError> {
        self.clock.now_ms().map_err(|_| DecisionError::Clock)
    }

    fn load(&self, decision_id: &DecisionId) -> Result<LoadedDecision, DecisionError> {
        match self.store()?.get(decision_id).map_err(read_error)? {
            None => Err(DecisionError::NotFound(decision_id.clone())),
            Some(DecisionRow::Unreadable(row)) => Err(unreadable(row)),
            Some(DecisionRow::Readable(row)) => check_row(decision_id.as_str(), *row),
        }
    }

    /// Load the row, apply `step` at the clock reading after the read, and
    /// write the row it leaves with a compare-and-swap on the revision. On a
    /// conflict reload once and apply `step` again. Answers with the row as
    /// it stands after the call, and the step's answer.
    fn commit(
        &self,
        decision_id: &DecisionId,
        mut step: impl FnMut(&LoadedDecision, u64) -> Result<Step, DecisionError>,
    ) -> Result<(StoredDecision, Answer), DecisionError> {
        self.require_writes()?;
        let store = self.store()?;
        let mut reloaded = false;
        loop {
            let loaded = self.load(decision_id)?;
            let now_ms = self.now_ms()?;
            let Step { next, answer } = step(&loaded, now_ms)?;
            let Some(next) = next else {
                return Ok((loaded.row, answer));
            };
            match store.replace_if_revision(loaded.row.revision, &next) {
                Ok(()) => return Ok((next, answer)),
                Err(ApprovalStoreError::Conflict) if !reloaded => reloaded = true,
                Err(error) => return Err(write_error(decision_id, error)),
            }
        }
    }

    /// [`Self::commit`], answering with the row, or with the machine's
    /// refusal.
    fn commit_row(
        &self,
        decision_id: &DecisionId,
        step: impl FnMut(&LoadedDecision, u64) -> Result<Step, DecisionError>,
    ) -> Result<StoredDecision, DecisionError> {
        match self.commit(decision_id, step)? {
            (row, Answer::Row) => Ok(row),
            (_, Answer::Refused(refusal)) => Err(DecisionError::Refused {
                decision_id: decision_id.clone(),
                refusal,
            }),
        }
    }

    /// Hold one operation for a decision, or answer the decision that
    /// already holds exactly this binding.
    ///
    /// The decision's id derives from the kind and subject key; a hold of
    /// the same subject with another binding (or another deadline, approver,
    /// reviewer or notice route) is refused as
    /// [`DecisionRefusal::HoldConflict`]. The subject digest is computed
    /// from the rendering. The first hold creates the store's file; nothing
    /// before it does. A requester that can be told the decision's end
    /// (`notice_routable`) is owed one notice when the decision ends.
    ///
    /// # Errors
    /// [`DecisionError::Refused`] with `InvalidWindow`, `SelfReview` or
    /// `HoldConflict`; [`DecisionError::Invalid`] for a subject digest other
    /// than the rendering's; [`DecisionError::CandidateLaunch`]; store, clock
    /// and unreadable-row failures.
    pub fn hold(
        &self,
        request: DecisionHoldRequest,
        notice_routable: bool,
    ) -> Result<StoredDecision, DecisionError> {
        self.require_writes()?;
        let store = self.store()?;
        let decision_id = DecisionId::derive(&request.kind, &request.subject_key);
        let mut raced = false;
        loop {
            let existing = store.get(&decision_id).map_err(read_error)?;
            let now_ms = self.now_ms()?;
            let descriptor = DecisionDescriptor::seal(request.clone(), now_ms)?;
            let input = hold_input(&decision_id, &descriptor, now_ms, notice_routable);
            match existing {
                Some(DecisionRow::Unreadable(row)) => return Err(unreadable(row)),
                Some(DecisionRow::Readable(row)) => {
                    let loaded = check_row(decision_id.as_str(), *row)?;
                    let (effects, _) = transition(&loaded, input)?;
                    return match refusal_of(&effects) {
                        Some(refusal) => Err(DecisionError::Refused {
                            decision_id,
                            refusal,
                        }),
                        None => Ok(loaded.row),
                    };
                }
                None => {
                    let mut authority = DurableDecisionMachineAuthority::new();
                    let effects = DurableDecisionMachineMutator::apply(&mut authority, input)
                        .map_err(|error| {
                            DecisionError::Machine(format!("decision {decision_id}: {error}"))
                        })?
                        .into_effects();
                    if let Some(refusal) = refusal_of(&effects) {
                        return Err(DecisionError::Refused {
                            decision_id,
                            refusal,
                        });
                    }
                    let row = StoredDecision {
                        decision_id: decision_id.clone(),
                        descriptor,
                        state: stored_state(authority.state())?,
                        presentations: Vec::new(),
                        outcome: None,
                        execution_refusal: None,
                        notice_receipt: None,
                        revision: 1,
                    };
                    match store.insert_if_absent(&row) {
                        Ok(()) => return Ok(row),
                        // Another owner held it first: answer this hold as a
                        // replay of, or a conflict with, that row.
                        Err(ApprovalStoreError::Conflict) if !raced => raced = true,
                        Err(error) => return Err(write_error(&decision_id, error)),
                    }
                }
            }
        }
    }

    /// Record a presentation of the decision's rendering. Only a complete
    /// presentation of the held subject to the party that decides now (the
    /// approver id in the human stage, the reviewer in the review stage)
    /// counts; it is kept as that audience's latest.
    ///
    /// # Errors
    /// [`DecisionError::Refused`] with `SubjectChanged`,
    /// `PresentationIncomplete`, `WrongAudience`, `NotOpen` or `Expired` (a
    /// presentation at or after the deadline records the expiry);
    /// [`DecisionError::CandidateLaunch`]; store, clock and unreadable-row
    /// failures.
    pub fn present(
        &self,
        decision_id: &DecisionId,
        presentation: &PresentationRecord,
    ) -> Result<StoredDecision, DecisionError> {
        self.commit_row(decision_id, |loaded, now_ms| {
            let input = DurableDecisionInput::Present {
                audience: audience_party(&presentation.audience),
                digest: machine_digest(&presentation.subject_digest),
                complete: presentation.shows_every_part_of(&loaded.row.descriptor.rendering),
                now_ms,
            };
            let (effects, state) = transition(loaded, input)?;
            let presented = effects
                .iter()
                .any(|effect| matches!(effect, DurableDecisionEffect::Presented { .. }));
            let next = successor(loaded, state, |row| {
                if presented {
                    keep_presentation(row, presentation.clone());
                }
            });
            Ok(Step::of(&effects, next))
        })
    }

    /// Decide an open decision in its human stage with one attested human
    /// decision. The decider's eligibility to act for the approver id is
    /// checked when the decision commits; the first valid decision wins.
    ///
    /// # Errors
    /// [`DecisionError::Refused`] with `EvidenceRequired`, `SubjectChanged`,
    /// `NotEligible`, `SelfApproval`, `PresentationIncomplete`, `ReviewStage`,
    /// `NotOpen` or `Expired`; [`DecisionError::CandidateLaunch`]; store,
    /// clock and unreadable-row failures.
    pub fn decide(
        &self,
        decision_id: &DecisionId,
        decision: &AttestedDecision,
        eligibility: &dyn ConsentEligibility,
    ) -> Result<StoredDecision, DecisionError> {
        let evidence = decision.evidence();
        let evidence_digest = evidence.digest()?;
        self.commit_row(decision_id, |loaded, now_ms| {
            let descriptor = &loaded.row.descriptor;
            let carried = decision.presentation().filter(|presentation| {
                matches!(
                    &presentation.audience,
                    PresentationAudience::Approver { approver_id }
                        if *approver_id == descriptor.approver_id
                ) && presentation.subject_digest == descriptor.subject_digest
                    && presentation.shows_every_part_of(&descriptor.rendering)
            });
            let input = DurableDecisionInput::Decide {
                approve: decision.approves(),
                approver_id: approver_party(decision.approver_id()),
                decider: principal_party(decision.decider()),
                digest: machine_digest(decision.presented_digest()),
                evidence_digest: Some(evidence_digest.as_str().to_owned()),
                eligible: eligibility.check_current().is_ok(),
                carries_presentation: carried.is_some(),
                now_ms,
            };
            let (effects, state) = transition(loaded, input)?;
            let decided = effects.iter().any(|effect| {
                matches!(
                    effect,
                    DurableDecisionEffect::Approved { .. } | DurableDecisionEffect::Rejected { .. }
                )
            });
            let next = successor(loaded, state, |row| {
                if decided {
                    row.outcome = Some(DecisionOutcome {
                        tier: DecisionTier::Human,
                        evidence: evidence.clone(),
                    });
                    if let Some(presentation) = carried {
                        keep_presentation(row, presentation.clone());
                    }
                }
            });
            Ok(Step::of(&effects, next))
        })
    }

    /// Decide an open decision in its review stage with its reviewer's
    /// attributed verdict: allow approves, deny rejects, and escalate hands
    /// the decision to the human approver, still open.
    ///
    /// # Errors
    /// [`DecisionError::Refused`] with `WrongReviewer`, `EvidenceRequired`,
    /// `SubjectChanged`, `PresentationIncomplete`, `HumanStage`, `NotOpen`
    /// or `Expired`; [`DecisionError::CandidateLaunch`]; store, clock and
    /// unreadable-row failures.
    pub fn review(
        &self,
        decision_id: &DecisionId,
        review: &AttributedReview,
    ) -> Result<StoredDecision, DecisionError> {
        let evidence = review.evidence();
        let evidence_digest = evidence.digest()?;
        self.commit_row(decision_id, |loaded, now_ms| {
            let input = DurableDecisionInput::Review {
                verdict: match review.verdict() {
                    DecisionReviewVerdict::Allow => DurableDecisionReviewVerdict::Allow,
                    DecisionReviewVerdict::Deny => DurableDecisionReviewVerdict::Deny,
                    DecisionReviewVerdict::Escalate => DurableDecisionReviewVerdict::Escalate,
                },
                reviewer: principal_party(review.reviewer_key()),
                digest: machine_digest(review.presented_digest()),
                evidence_digest: Some(evidence_digest.as_str().to_owned()),
                now_ms,
            };
            let (effects, state) = transition(loaded, input)?;
            let decided = effects.iter().any(|effect| {
                matches!(
                    effect,
                    DurableDecisionEffect::Approved { .. } | DurableDecisionEffect::Rejected { .. }
                )
            });
            let next = successor(loaded, state, |row| {
                if decided {
                    row.outcome = Some(DecisionOutcome {
                        tier: DecisionTier::Review,
                        evidence: evidence.clone(),
                    });
                }
            });
            Ok(Step::of(&effects, next))
        })
    }

    /// Withdraw an open decision, by its requester or the host.
    ///
    /// # Errors
    /// [`DecisionError::Refused`] with `NotRequester`, `NotOpen` or
    /// `Expired`; [`DecisionError::CandidateLaunch`]; store, clock and
    /// unreadable-row failures.
    pub fn withdraw(
        &self,
        decision_id: &DecisionId,
        withdrawer: &DecisionWithdrawer,
    ) -> Result<StoredDecision, DecisionError> {
        self.commit_row(decision_id, |loaded, now_ms| {
            let input = DurableDecisionInput::Withdraw {
                withdrawer: match withdrawer {
                    DecisionWithdrawer::Requester(requester) => principal_party(requester),
                    DecisionWithdrawer::Host => host_party(),
                },
                by_host: matches!(withdrawer, DecisionWithdrawer::Host),
                now_ms,
            };
            let (effects, state) = transition(loaded, input)?;
            Ok(Step::of(&effects, successor(loaded, state, |_| {})))
        })
    }

    /// Read one decision, settled at the clock reading of the read: a
    /// decision past its deadline (an approved one with no permit
    /// outstanding included) reads as expired. An admitted owner records the
    /// expiry; a candidate answers it without recording it.
    ///
    /// # Errors
    /// [`DecisionError::NotFound`], [`DecisionError::Unreadable`], store and
    /// clock failures.
    pub fn get(&self, decision_id: &DecisionId) -> Result<StoredDecision, DecisionError> {
        let observe = |loaded: &LoadedDecision, now_ms: u64| -> Result<Step, DecisionError> {
            let (effects, state) = transition(loaded, DurableDecisionInput::Expire { now_ms })?;
            Ok(Step::of(&effects, successor(loaded, state, |_| {})))
        };
        match self.mode {
            DecisionWriteMode::Admitted => self.commit_row(decision_id, observe),
            DecisionWriteMode::CandidateLaunch => {
                let loaded = self.load(decision_id)?;
                let now_ms = self.now_ms()?;
                let revision = loaded.row.revision;
                let mut row = observe(&loaded, now_ms)?.next.unwrap_or(loaded.row);
                row.revision = revision;
                Ok(row)
            }
        }
    }

    /// One page of the decisions that match `filter`, after `after`.
    ///
    /// # Errors
    /// Store and clock failures.
    pub fn list(
        &self,
        filter: &DecisionListFilter,
        after: Option<&str>,
        limit: usize,
    ) -> Result<DecisionListing, DecisionError> {
        let page = self
            .store()?
            .list(filter, after, limit.min(DECISION_PAGE_MAX))
            .map_err(read_error)?;
        self.project(page)
    }

    /// One page of the decisions that owe work, after `after`: approved
    /// decisions to execute (or settle), and ended ones owing their notice.
    /// The page is the store's index page; nothing else is read.
    ///
    /// # Errors
    /// Store and clock failures.
    pub fn owed(
        &self,
        after: Option<&str>,
        limit: usize,
    ) -> Result<DecisionListing, DecisionError> {
        let page = self
            .store()?
            .owed(after, limit.min(DECISION_PAGE_MAX))
            .map_err(read_error)?;
        self.project(page)
    }

    /// One page of the decisions due an expiry observation at the clock
    /// reading of the read, after `after`, as stored: a deadline driver
    /// [`Self::get`]s each to record its expiry.
    ///
    /// # Errors
    /// Store and clock failures.
    pub fn due(&self, after: Option<&str>, limit: usize) -> Result<DecisionListing, DecisionError> {
        let store = self.store()?;
        let now_ms = self.now_ms()?;
        let page = store
            .due(now_ms, after, limit.min(DECISION_PAGE_MAX))
            .map_err(read_error)?;
        let mut listing = DecisionListing {
            next_after: page.next_after,
            ..DecisionListing::default()
        };
        for row in page.rows {
            match row {
                DecisionRow::Unreadable(row) => listing.unreadable.push(row),
                DecisionRow::Readable(row) => {
                    let stored_under = row.decision_id.as_str().to_owned();
                    match check_row(&stored_under, *row) {
                        Ok(loaded) => listing.decisions.push(loaded.row),
                        Err(error) => listing.unreadable.push(UnreadableDecision {
                            decision_id: stored_under,
                            reason: error.to_string(),
                        }),
                    }
                }
            }
        }
        Ok(listing)
    }

    /// How many decisions match `filter`.
    ///
    /// # Errors
    /// Store failures.
    pub fn count(&self, filter: &DecisionListFilter) -> Result<u64, DecisionError> {
        self.store()?.count(filter).map_err(read_error)
    }

    /// Each row of `page`, projected at the clock reading after the read.
    fn project(&self, page: DecisionPage) -> Result<DecisionListing, DecisionError> {
        let now_ms = self.now_ms()?;
        let mut listing = DecisionListing {
            next_after: page.next_after,
            ..DecisionListing::default()
        };
        for row in page.rows {
            let row = match row {
                DecisionRow::Unreadable(row) => {
                    listing.unreadable.push(row);
                    continue;
                }
                DecisionRow::Readable(row) => row,
            };
            let stored_under = row.decision_id.as_str().to_owned();
            let projected = check_row(&stored_under, *row).and_then(|loaded| {
                let (_, state) = transition(&loaded, DurableDecisionInput::Expire { now_ms })?;
                let mut row = loaded.row;
                row.state = state;
                Ok(row)
            });
            match projected {
                Ok(row) => listing.decisions.push(row),
                Err(error) => listing.unreadable.push(UnreadableDecision {
                    decision_id: stored_under,
                    reason: error.to_string(),
                }),
            }
        }
        Ok(listing)
    }

    /// Authorize the execution of an approved decision's held request: the
    /// binding must be the held one, and the clock before the deadline. The
    /// decision records the permit as outstanding, and from then on waits
    /// for its settlement instead of expiring. Authorizing again before the
    /// deadline (an owner retrying after a crash) gives the same permit.
    ///
    /// # Errors
    /// [`DecisionError::Refused`] with `BindingMismatch`, `NotApproved`,
    /// `SettlementOwed` (at or after the deadline, with a permit
    /// outstanding) or `Expired` (at or after the deadline, with none: the
    /// expiry is recorded); [`DecisionError::CandidateLaunch`]; store, clock
    /// and unreadable-row failures.
    pub fn authorize_execution(
        &self,
        decision_id: &DecisionId,
        operation_id: &DecisionOperationId,
        request_digest: &Sha256Digest,
    ) -> Result<DecisionExecutionPermit, DecisionError> {
        let row = self.commit_row(decision_id, |loaded, now_ms| {
            let input = DurableDecisionInput::AuthorizeExecution {
                operation_id: operation_id.as_str().to_owned(),
                request_digest: machine_digest(request_digest),
                now_ms,
            };
            let (effects, state) = transition(loaded, input)?;
            Ok(Step::of(&effects, successor(loaded, state, |_| {})))
        })?;
        permit_of(&row)
    }

    /// Settle an outstanding permit under which no commit was made (a
    /// retryable refusal, or a crash before the commit): the decision stays
    /// approved, or, at or after its deadline, records its expiry.
    ///
    /// # Errors
    /// [`DecisionError::Refused`] with `NotAuthorized` or `NotApproved`;
    /// [`DecisionError::CandidateLaunch`]; store, clock and unreadable-row
    /// failures.
    pub fn release_execution(
        &self,
        decision_id: &DecisionId,
    ) -> Result<StoredDecision, DecisionError> {
        self.commit_row(decision_id, |loaded, now_ms| {
            let (effects, state) =
                transition(loaded, DurableDecisionInput::ReleaseExecution { now_ms })?;
            Ok(Step::of(&effects, successor(loaded, state, |_| {})))
        })
    }

    /// Settle a commit of the held request under the outstanding permit:
    /// the decision is executed. Accepted at any time, the deadline
    /// included; the same settlement again changes nothing, so an owner's
    /// normal path and its settlement after a crash are one call.
    ///
    /// # Errors
    /// [`DecisionError::Refused`] with `BindingMismatch`, `NotAuthorized` or
    /// `NotApproved`; [`DecisionError::CandidateLaunch`]; store, clock and
    /// unreadable-row failures.
    pub fn record_executed(
        &self,
        decision_id: &DecisionId,
        operation_id: &DecisionOperationId,
        request_digest: &Sha256Digest,
    ) -> Result<StoredDecision, DecisionError> {
        self.commit_row(decision_id, |loaded, now_ms| {
            let input = DurableDecisionInput::RecordExecuted {
                operation_id: operation_id.as_str().to_owned(),
                request_digest: machine_digest(request_digest),
                now_ms,
            };
            let (effects, state) = transition(loaded, input)?;
            Ok(Step::of(&effects, successor(loaded, state, |_| {})))
        })
    }

    /// Close an approved decision on the owner's typed permanent refusal of
    /// the held request: the decision ends refused and owes its notice. At
    /// or after the deadline with no permit outstanding, the approval has
    /// lapsed: the expiry is recorded instead.
    ///
    /// # Errors
    /// [`DecisionError::Refused`] with `NotApproved` or `Expired`;
    /// [`DecisionError::CandidateLaunch`]; store, clock and unreadable-row
    /// failures.
    pub fn close_refused(
        &self,
        decision_id: &DecisionId,
        refusal: &DecisionReference,
    ) -> Result<StoredDecision, DecisionError> {
        self.commit_row(decision_id, |loaded, now_ms| {
            let (effects, state) =
                transition(loaded, DurableDecisionInput::CloseRefused { now_ms })?;
            let refused = effects
                .iter()
                .any(|effect| matches!(effect, DurableDecisionEffect::ExecutionRefused { .. }));
            let next = successor(loaded, state, |row| {
                if refused {
                    row.execution_refusal = Some(refusal.clone());
                }
            });
            Ok(Step::of(&effects, next))
        })
    }

    /// Settle the owed requester notice of an ended decision with its
    /// delivery receipt: admitted, or failed terminally. The same receipt
    /// again changes nothing.
    ///
    /// # Errors
    /// [`DecisionError::NotAReceipt`] for an owed state;
    /// [`DecisionError::Refused`] with `NotOwed`;
    /// [`DecisionError::CandidateLaunch`]; store, clock and unreadable-row
    /// failures.
    pub fn settle_notice(
        &self,
        decision_id: &DecisionId,
        receipt: &ApprovalConsentDeliveryState,
    ) -> Result<StoredDecision, DecisionError> {
        let delivered = match receipt {
            ApprovalConsentDeliveryState::Admitted { .. } => true,
            ApprovalConsentDeliveryState::Failed { .. } => false,
            ApprovalConsentDeliveryState::Owed => return Err(DecisionError::NotAReceipt),
        };
        self.commit_row(decision_id, |loaded, _now_ms| {
            let (effects, state) =
                transition(loaded, DurableDecisionInput::SettleNotice { delivered })?;
            let settled = state.notice != loaded.row.state.notice;
            let next = successor(loaded, state, |row| {
                if settled {
                    row.notice_receipt = Some(receipt.clone());
                }
            });
            Ok(Step::of(&effects, next))
        })
    }
}
