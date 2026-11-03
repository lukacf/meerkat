//! Canonical lifecycle authority for ONE durable decision.
//!
//! A durable decision holds one operation until an attested party approves
//! it, rejects it, withdraws it, or its deadline passes, and then lets the
//! operation's owner execute the held request exactly once. This machine owns
//! the decision's own lifecycle: the hold and its sealed binding, the
//! presentation of the held subject, the attested decision (a human
//! approver's, or a named reviewer's verdict), the execution permit and the
//! executed fact, expiry, withdrawal, and the one notice an ended decision
//! owes its requester.
//!
//! # Scope discipline
//!
//! Record-scoped: the store persists one machine state per decision, next to
//! the decision's immutable descriptor (its request, rendering, evidence and
//! presentation records). The machine never interprets the descriptor; its
//! owner service does, and passes the machine the digests and the
//! comparisons it needs as typed inputs.
//!
//! The machine deliberately does NOT own:
//!
//! - **The operation.** The operation's owner commits the held request under
//!   a permit this machine authorized and then records the commit here. The
//!   machine owns only the legality of the permit and of the executed fact.
//! - **Eligibility.** Whether a decider may act for the sealed approver id is
//!   the host's current answer, passed in as `eligible` at each decision.
//! - **Time.** It reads no clock. Every timed input carries the host's
//!   `now_ms` observation, and the guards compare it with the decision's
//!   sealed `deadline_at_ms`.
//! - **Notice content and delivery.** The machine owes one requester notice
//!   per ended decision whose requester is routable, and records its receipt.
//!
//! # Deadline and permit
//!
//! An open decision expires at its deadline, observed by `Expire` or by any
//! touch at or after it. An approved decision that was not executed expires
//! at its deadline too, unless an execution permit is outstanding: a permit
//! is authorized only before the deadline, and while one is outstanding the
//! operation's owner may already have committed the held request, so the
//! decision waits for the owner's settlement. `RecordExecuted` settles a
//! commit at any time; `ReleaseExecution` says no commit was made and, at or
//! after the deadline, records the expiry. No permit is authorized at or
//! after the deadline, so nothing new runs after it.
//!
//! # Presentation order
//!
//! A human decision carries the time its trusted ingress observed the
//! decision event (`observed_at_ms`), and the machine records when a
//! complete presentation first reached the approver: the earliest
//! presentation time, from a recorded presentation (`presented_at_ms`) or
//! the one a decision event carried. A human decision observed before that
//! first presentation is refused (`DecidedBeforePresentation`); one observed
//! in the same millisecond is accepted. With no presentation at all, the
//! decision stays `PresentationIncomplete`. A presentation time of 0 is no
//! reading: a presentation to the approver at 0, recorded or carried by a
//! decision, is refused `PresentationIncomplete`, so it never becomes the
//! first presentation. The order is this machine's fact: neither the owner
//! service nor a host compares the times again. It is separate from the
//! deadline, which the owner's clock (`now_ms`) decides.
//!
//! # Latching
//!
//! `Rejected`, `Expired`, `Withdrawn`, `Executed` and `ExecutionRefused` are
//! terminal. Once a decision reaches one, no input moves it again; only its
//! owed notice settles. A restart or a clock rollback can never revive it.
//!
//! # Totality discipline
//!
//! Every input has an explicit transition in every held phase, and every
//! refusal is a typed `Refused` effect that leaves the state byte-identical.
//! Against an `Unheld` record the generated kernel refuses (`GuardRejected`:
//! no arm admits them) every input other than `Hold`, and a `Hold` that
//! names no decision or no operation (unless its window is already closed or
//! it names its requester as its reviewer: the typed `InvalidWindow` and
//! `SelfReview` refusals). The owner service creates a row only through a
//! `Hold` whose ids it validated as non-empty, so it never sends them.

use super::OptionValueExt;

/// Canonical data declarations shared by every host instantiation of the
/// machine. These are closed vocabularies and opaque keys; they carry no
/// authority.
pub mod types {
    /// An opaque party key: a requester, a decider, an approver id, a
    /// reviewer identity or a presentation audience. The machine only
    /// compares keys for equality.
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
    pub struct DecisionParty(pub String);

    impl<T: Into<String>> From<T> for DecisionParty {
        fn from(value: T) -> Self {
            Self(value.into())
        }
    }

    /// An opaque content digest: the held subject digest, a presented
    /// digest, or the held request digest. The machine only compares digests
    /// for equality.
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
    pub struct DecisionDigest(pub String);

    impl<T: Into<String>> From<T> for DecisionDigest {
        fn from(value: T) -> Self {
            Self(value.into())
        }
    }

    /// Who decides an open decision now.
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub enum DurableDecisionStage {
        /// An authenticated human acting for the sealed approver id.
        #[default]
        Human,
        /// The sealed reviewer, through its verdict.
        Review,
    }

    /// Where the requester notice of a decision stands.
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub enum DurableDecisionNotice {
        /// The requester is not routable: no notice is ever owed.
        #[default]
        NotRequired,
        /// The requester is routable and the decision has not ended.
        NotDue,
        /// The decision ended and its notice is owed.
        Owed,
        /// The notice was delivered.
        Delivered,
        /// The notice failed terminally.
        Failed,
    }

    /// A reviewer's verdict on an open decision in its review stage.
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub enum DurableDecisionReviewVerdict {
        /// Approve the held operation.
        #[default]
        Allow,
        /// Reject it.
        Deny,
        /// Hand the decision to the human approver.
        Escalate,
    }

    /// Why an input was refused. Every refusal leaves the decision unchanged,
    /// except `Expired`, which a touch at or after the deadline emits with
    /// the expiry it recorded.
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub enum DurableDecisionRefusal {
        /// The hold's deadline is not after the hold's clock reading.
        #[default]
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
        /// The presentation did not cover the whole held subject, none reached
        /// the deciding party, or it was shown at 0 (no reading of when).
        PresentationIncomplete,
        /// The presentation was shown to a party that does not decide now.
        WrongAudience,
        /// A human decision while the review stage is open.
        ReviewStage,
        /// A review verdict while the human stage is open.
        HumanStage,
        /// The decision carries no attested evidence.
        EvidenceRequired,
        /// The decider does not act for the sealed approver id, or is not
        /// currently eligible.
        NotEligible,
        /// The requester tried to decide its own request.
        SelfApproval,
        /// The verdict is not the sealed reviewer's.
        WrongReviewer,
        /// Only the requester or the host may withdraw.
        NotRequester,
        /// The decision is not approved.
        NotApproved,
        /// The execution names another operation or request.
        BindingMismatch,
        /// No execution permit is outstanding.
        NotAuthorized,
        /// The deadline passed while a permit is outstanding: the owner must
        /// settle the permit before anything else.
        SettlementOwed,
        /// No notice is owed, or this receipt contradicts the settled one.
        NotOwed,
        /// The human decision was observed before the decision's first
        /// presentation to the approver.
        DecidedBeforePresentation,
    }
}

#[macro_export]
macro_rules! durable_decision_catalog_machine_dsl {
    ($rust_crate:literal, $rust_module:literal) => {
        pub use $crate::catalog::dsl::durable_decision::types::{
            DecisionDigest, DecisionParty, DurableDecisionNotice, DurableDecisionRefusal,
            DurableDecisionReviewVerdict, DurableDecisionStage,
        };

        meerkat_machine_dsl::machine! {
            machine DurableDecisionMachine {
                version: 1,
                rust: $rust_crate / $rust_module,

                state {
                    lifecycle_phase: DurableDecisionPhase,
                    // Service-derived decision identity. Empty only while
                    // Unheld.
                    decision_id: String,
                    // Sealed absolute deadline. Never moves once held.
                    deadline_at_ms: u64,
                    // Sealed digest of the one canonical rendering of the
                    // held request: the only digest a decision binds to.
                    subject_digest: DecisionDigest,
                    // Sealed binding of the held operation: its owner's
                    // operation id and request digest.
                    operation_id: String,
                    request_digest: DecisionDigest,
                    // Sealed parties: the requester (never a decider), the
                    // approver id a human decides for, and the reviewer of a
                    // review-stage decision.
                    requester: DecisionParty,
                    approver_id: DecisionParty,
                    reviewer: Option<DecisionParty>,
                    // Who decides now. A review escalation hands the
                    // decision to the human stage.
                    stage: Enum<DurableDecisionStage>,
                    // When a complete presentation of the held subject first
                    // reached the approver: the earliest presentation time,
                    // recorded or carried by a decision event. A human
                    // decision observed before it is refused.
                    first_presented_to_approver_at_ms: Option<u64>,
                    // Whether a complete presentation of the held subject
                    // reached the reviewer.
                    presented_to_reviewer: bool,
                    // Digest of the attested evidence of the decision.
                    evidence_digest: Option<String>,
                    // Host times the decision was made and ended.
                    decided_at_ms: Option<u64>,
                    ended_at_ms: Option<u64>,
                    // An execution permit is outstanding: the owner may have
                    // committed the held request and must settle it.
                    permit_issued: bool,
                    // The request digest the owner executed, set exactly when
                    // the decision is Executed.
                    executed_request_digest: Option<DecisionDigest>,
                    // The requester notice the decision owes.
                    notice: Enum<DurableDecisionNotice>,
                }

                init(Unheld) {
                    decision_id = "",
                    deadline_at_ms = 0,
                    subject_digest = "",
                    operation_id = "",
                    request_digest = "",
                    requester = "",
                    approver_id = "",
                    reviewer = None,
                    stage = DurableDecisionStage::Human,
                    first_presented_to_approver_at_ms = None,
                    presented_to_reviewer = false,
                    evidence_digest = None,
                    decided_at_ms = None,
                    ended_at_ms = None,
                    permit_issued = false,
                    executed_request_digest = None,
                    notice = DurableDecisionNotice::NotRequired,
                }

                terminal [Rejected, Expired, Withdrawn, Executed, ExecutionRefused]

                phase DurableDecisionPhase {
                    // No decision is held on this record yet.
                    Unheld,
                    // Held and not yet decided, withdrawn or expired.
                    Open,
                    // Approved with attested evidence; not yet executed.
                    Approved,
                    // Rejected with attested evidence. Terminal.
                    Rejected,
                    // Observed at or after its deadline undecided, or
                    // approved and never executed. Terminal.
                    Expired,
                    // Withdrawn by its requester or the host. Terminal.
                    Withdrawn,
                    // The held request was executed. Terminal.
                    Executed,
                    // The owner refused the held request permanently.
                    // Terminal.
                    ExecutionRefused,
                }

                input DurableDecisionInput {
                    // Hold one operation for a decision.
                    Hold {
                        decision_id: String,
                        now_ms: u64,
                        deadline_at_ms: u64,
                        subject_digest: DecisionDigest,
                        operation_id: String,
                        request_digest: DecisionDigest,
                        requester: DecisionParty,
                        approver_id: DecisionParty,
                        reviewer: Option<DecisionParty>,
                        notice_routable: bool,
                    },
                    // A presentation of `digest`, complete or not, was shown
                    // to `audience` at `presented_at_ms`, the presentation
                    // record's own time.
                    Present {
                        audience: DecisionParty,
                        digest: DecisionDigest,
                        complete: bool,
                        presented_at_ms: u64,
                        now_ms: u64,
                    },
                    // An authenticated human decides for `approver_id`. The
                    // decision event's trusted ingress observed it at
                    // `observed_at_ms`; it may carry the complete
                    // presentation to the approver it was shown at
                    // `carried_presentation_at_ms`.
                    Decide {
                        approve: bool,
                        approver_id: DecisionParty,
                        decider: DecisionParty,
                        digest: DecisionDigest,
                        evidence_digest: Option<String>,
                        eligible: bool,
                        carried_presentation_at_ms: Option<u64>,
                        observed_at_ms: u64,
                        now_ms: u64,
                    },
                    // The reviewer's attributed verdict.
                    Review {
                        verdict: Enum<DurableDecisionReviewVerdict>,
                        reviewer: DecisionParty,
                        digest: DecisionDigest,
                        evidence_digest: Option<String>,
                        now_ms: u64,
                    },
                    // The requester, or the host, withdraws the request.
                    Withdraw { withdrawer: DecisionParty, by_host: bool, now_ms: u64 },
                    // A deadline driver or a read observed the clock.
                    Expire { now_ms: u64 },
                    // The owner asks for a permit to execute the held request.
                    AuthorizeExecution {
                        operation_id: String,
                        request_digest: DecisionDigest,
                        now_ms: u64,
                    },
                    // The owner made no commit under the outstanding permit.
                    ReleaseExecution { now_ms: u64 },
                    // The owner committed the held request.
                    RecordExecuted {
                        operation_id: String,
                        request_digest: DecisionDigest,
                        now_ms: u64,
                    },
                    // The owner refused the held request permanently.
                    CloseRefused { now_ms: u64 },
                    // The requester notice's delivery receipt.
                    SettleNotice { delivered: bool },
                }

                effect DurableDecisionEffect {
                    Held { decision_id: String, deadline_at_ms: u64 },
                    HoldReplayed { decision_id: String },
                    Presented { decision_id: String, audience: DecisionParty },
                    Escalated { decision_id: String },
                    Approved { decision_id: String, decided_at_ms: u64 },
                    Rejected { decision_id: String, decided_at_ms: u64 },
                    Expired { decision_id: String, deadline_at_ms: u64, observed_at_ms: u64 },
                    Withdrawn { decision_id: String, observed_at_ms: u64 },
                    ExecutionAuthorized {
                        decision_id: String,
                        operation_id: String,
                        request_digest: DecisionDigest,
                        deadline_at_ms: u64,
                    },
                    ExecutionReleased { decision_id: String },
                    Executed {
                        decision_id: String,
                        operation_id: String,
                        request_digest: DecisionDigest,
                        executed_at_ms: u64,
                    },
                    ExecutionReplayed { decision_id: String },
                    ExecutionRefused { decision_id: String, closed_at_ms: u64 },
                    NoticeSettled { decision_id: String, delivered: bool },
                    Refused { decision_id: String, reason: Enum<DurableDecisionRefusal> },
                }

                // The notice an ending owes: a routable requester's notice
                // becomes owed; a notice that is not required stays so.
                helper owed_if_due(notice: Enum<DurableDecisionNotice>) -> Enum<DurableDecisionNotice> {
                    if notice == DurableDecisionNotice::NotDue {
                        DurableDecisionNotice::Owed
                    } else {
                        notice
                    }
                }

                // The first presentation time once another presentation, if
                // any, was shown at `presented`: the earlier of the two, so
                // the first presentation is the earliest one in whatever
                // order the presentations arrive.
                helper earliest_presentation(first: Option<u64>, presented: Option<u64>) -> Option<u64> {
                    if presented != None
                        && (first == None || presented.get("value") < first.get("value"))
                    {
                        presented
                    } else {
                        first
                    }
                }

                // An unheld record carries no decision facts at all.
                invariant unheld_is_empty {
                    self.lifecycle_phase != Phase::Unheld
                        || (
                            self.decision_id == ""
                            && self.operation_id == ""
                            && self.deadline_at_ms == 0
                            && self.reviewer == None
                            && self.stage == DurableDecisionStage::Human
                            && self.first_presented_to_approver_at_ms == None
                            && !self.presented_to_reviewer
                            && self.evidence_digest == None
                            && self.decided_at_ms == None
                            && self.ended_at_ms == None
                            && !self.permit_issued
                            && self.executed_request_digest == None
                            && self.notice == DurableDecisionNotice::NotRequired
                        )
                }

                // Every held record names its decision and operation and has
                // a deadline after the epoch.
                invariant held_records_are_named {
                    self.lifecycle_phase == Phase::Unheld
                        || (
                            self.decision_id != ""
                            && self.operation_id != ""
                            && self.deadline_at_ms > 0
                        )
                }

                // A review stage names a reviewer other than the requester.
                invariant review_stage_names_another_reviewer {
                    self.stage != DurableDecisionStage::Review
                        || (
                            self.reviewer != None
                            && self.reviewer.get("value") != self.requester
                        )
                }

                // Only a named reviewer is ever presented to as one.
                invariant reviewer_presentation_needs_a_reviewer {
                    !self.presented_to_reviewer || self.reviewer != None
                }

                // The approver's first presentation time is a reading: a
                // time of 0 is never recorded.
                invariant first_presentation_is_a_reading {
                    self.first_presented_to_approver_at_ms != Some(0)
                }

                // An open decision has no outcome, no permit and owes no
                // notice yet.
                invariant open_has_no_outcome {
                    self.lifecycle_phase != Phase::Open
                        || (
                            self.evidence_digest == None
                            && self.decided_at_ms == None
                            && self.ended_at_ms == None
                            && !self.permit_issued
                            && self.executed_request_digest == None
                            && (
                                self.notice == DurableDecisionNotice::NotRequired
                                || self.notice == DurableDecisionNotice::NotDue
                            )
                        )
                }

                // An approved decision has not ended and owes no notice yet.
                invariant approved_has_not_ended {
                    self.lifecycle_phase != Phase::Approved
                        || (
                            self.ended_at_ms == None
                            && self.executed_request_digest == None
                            && (
                                self.notice == DurableDecisionNotice::NotRequired
                                || self.notice == DurableDecisionNotice::NotDue
                            )
                        )
                }

                // Evidence and the decision time are recorded together.
                invariant evidence_and_decision_time_agree {
                    (self.evidence_digest == None) == (self.decided_at_ms == None)
                }

                // Every decided decision is attested and was decided before
                // its deadline.
                invariant decided_decisions_are_attested {
                    (
                        self.lifecycle_phase != Phase::Approved
                        && self.lifecycle_phase != Phase::Rejected
                        && self.lifecycle_phase != Phase::Executed
                        && self.lifecycle_phase != Phase::ExecutionRefused
                    )
                    || (
                        self.evidence_digest != None
                        && self.decided_at_ms != None
                        && self.decided_at_ms.get("value") < self.deadline_at_ms
                    )
                }

                // An execution permit is outstanding only on an approved
                // decision.
                invariant permit_is_outstanding_only_while_approved {
                    self.lifecycle_phase == Phase::Approved || !self.permit_issued
                }

                // An executed decision names the held request (I-C4).
                invariant executed_names_its_held_request {
                    self.lifecycle_phase != Phase::Executed
                        || (
                            self.executed_request_digest != None
                            && self.executed_request_digest.get("value") == self.request_digest
                        )
                }

                // Only an executed decision records an execution.
                invariant only_executed_records_an_execution {
                    self.lifecycle_phase == Phase::Executed
                        || self.executed_request_digest == None
                }

                // Every ended decision records when it ended.
                invariant ended_decisions_record_their_end {
                    (
                        self.lifecycle_phase != Phase::Rejected
                        && self.lifecycle_phase != Phase::Expired
                        && self.lifecycle_phase != Phase::Withdrawn
                        && self.lifecycle_phase != Phase::Executed
                        && self.lifecycle_phase != Phase::ExecutionRefused
                    )
                    || self.ended_at_ms != None
                }

                // Expiry is observed at or after the deadline, never before,
                // and an expired decision was either never decided or was
                // approved before its deadline.
                invariant expiry_never_precedes_its_deadline {
                    self.lifecycle_phase != Phase::Expired
                        || (
                            self.ended_at_ms != None
                            && self.ended_at_ms.get("value") >= self.deadline_at_ms
                            && (
                                self.decided_at_ms == None
                                || self.decided_at_ms.get("value") < self.deadline_at_ms
                            )
                        )
                }

                // A withdrawal ends an undecided decision before its
                // deadline; a withdraw at or after it records expiry instead.
                invariant withdrawal_precedes_the_deadline {
                    self.lifecycle_phase != Phase::Withdrawn
                        || (
                            self.ended_at_ms != None
                            && self.ended_at_ms.get("value") < self.deadline_at_ms
                            && self.evidence_digest == None
                        )
                }

                // An ended decision of a routable requester owes, or has
                // settled, its notice: an ended decision is never NotDue.
                invariant ended_decisions_owe_their_notice {
                    (
                        self.lifecycle_phase != Phase::Rejected
                        && self.lifecycle_phase != Phase::Expired
                        && self.lifecycle_phase != Phase::Withdrawn
                        && self.lifecycle_phase != Phase::Executed
                        && self.lifecycle_phase != Phase::ExecutionRefused
                    )
                    || self.notice != DurableDecisionNotice::NotDue
                }

                // Lifecycle facts: the owner service persists the row in one
                // conditional write and realizes the fact.
                disposition Held => local seam OwnerRealizationOnly,
                disposition Presented => local seam OwnerRealizationOnly,
                disposition Escalated => local seam OwnerRealizationOnly,
                disposition Approved => local seam OwnerRealizationOnly,
                disposition Rejected => local seam OwnerRealizationOnly,
                disposition Expired => local seam OwnerRealizationOnly,
                disposition Withdrawn => local seam OwnerRealizationOnly,
                disposition ExecutionAuthorized => local seam OwnerRealizationOnly,
                disposition ExecutionReleased => local seam OwnerRealizationOnly,
                disposition Executed => local seam OwnerRealizationOnly,
                disposition ExecutionRefused => local seam OwnerRealizationOnly,
                disposition NoticeSettled => local seam OwnerRealizationOnly,
                // Verdicts: the calling surface mirrors them as its typed
                // result.
                disposition HoldReplayed => local seam SurfaceResultAlignment,
                disposition ExecutionReplayed => local seam SurfaceResultAlignment,
                disposition Refused => local seam SurfaceResultAlignment,

                // ----------------------------------------------------------
                // Hold
                // ----------------------------------------------------------

                transition HoldHuman {
                    on input Hold {
                        decision_id,
                        now_ms,
                        deadline_at_ms,
                        subject_digest,
                        operation_id,
                        request_digest,
                        requester,
                        approver_id,
                        reviewer,
                        notice_routable
                    }
                    guard "unheld" { self.lifecycle_phase == Phase::Unheld }
                    guard "named_decision" { decision_id != "" }
                    guard "named_operation" { operation_id != "" }
                    guard "window_is_open" { now_ms < deadline_at_ms }
                    guard "no_reviewer" { reviewer == None }
                    update {
                        self.decision_id = decision_id;
                        self.deadline_at_ms = deadline_at_ms;
                        self.subject_digest = subject_digest;
                        self.operation_id = operation_id;
                        self.request_digest = request_digest;
                        self.requester = requester;
                        self.approver_id = approver_id;
                        self.reviewer = reviewer;
                        self.stage = DurableDecisionStage::Human;
                        if notice_routable {
                            self.notice = DurableDecisionNotice::NotDue;
                        }
                    }
                    to Open
                    emit Held { decision_id: self.decision_id, deadline_at_ms: self.deadline_at_ms }
                }

                transition HoldReview {
                    on input Hold {
                        decision_id,
                        now_ms,
                        deadline_at_ms,
                        subject_digest,
                        operation_id,
                        request_digest,
                        requester,
                        approver_id,
                        reviewer,
                        notice_routable
                    }
                    guard "unheld" { self.lifecycle_phase == Phase::Unheld }
                    guard "named_decision" { decision_id != "" }
                    guard "named_operation" { operation_id != "" }
                    guard "window_is_open" { now_ms < deadline_at_ms }
                    guard "names_a_reviewer" { reviewer != None }
                    guard "reviewer_is_not_requester" { reviewer != Some(requester) }
                    update {
                        self.decision_id = decision_id;
                        self.deadline_at_ms = deadline_at_ms;
                        self.subject_digest = subject_digest;
                        self.operation_id = operation_id;
                        self.request_digest = request_digest;
                        self.requester = requester;
                        self.approver_id = approver_id;
                        self.reviewer = reviewer;
                        self.stage = DurableDecisionStage::Review;
                        if notice_routable {
                            self.notice = DurableDecisionNotice::NotDue;
                        }
                    }
                    to Open
                    emit Held { decision_id: self.decision_id, deadline_at_ms: self.deadline_at_ms }
                }

                transition HoldRefusedWindow {
                    on input Hold {
                        decision_id,
                        now_ms,
                        deadline_at_ms,
                        subject_digest,
                        operation_id,
                        request_digest,
                        requester,
                        approver_id,
                        reviewer,
                        notice_routable
                    }
                    guard "unheld" { self.lifecycle_phase == Phase::Unheld }
                    guard "window_is_closed" { deadline_at_ms <= now_ms }
                    update {}
                    to Unheld
                    emit Refused { decision_id: decision_id, reason: DurableDecisionRefusal::InvalidWindow }
                }

                transition HoldRefusedSelfReview {
                    on input Hold {
                        decision_id,
                        now_ms,
                        deadline_at_ms,
                        subject_digest,
                        operation_id,
                        request_digest,
                        requester,
                        approver_id,
                        reviewer,
                        notice_routable
                    }
                    guard "unheld" { self.lifecycle_phase == Phase::Unheld }
                    guard "window_is_open" { now_ms < deadline_at_ms }
                    guard "requester_reviews" { reviewer == Some(requester) }
                    update {}
                    to Unheld
                    emit Refused { decision_id: decision_id, reason: DurableDecisionRefusal::SelfReview }
                }

                // The same hold again: every sealed field is equal. The
                // clock reading of a replay is not compared.
                transition HoldReplayed {
                    per_phase [Open, Approved, Rejected, Expired, Withdrawn, Executed, ExecutionRefused]
                    on input Hold {
                        decision_id,
                        now_ms,
                        deadline_at_ms,
                        subject_digest,
                        operation_id,
                        request_digest,
                        requester,
                        approver_id,
                        reviewer,
                        notice_routable
                    }
                    guard "same_decision" { decision_id == self.decision_id }
                    guard "same_deadline" { deadline_at_ms == self.deadline_at_ms }
                    guard "same_subject" { subject_digest == self.subject_digest }
                    guard "same_operation" { operation_id == self.operation_id }
                    guard "same_request" { request_digest == self.request_digest }
                    guard "same_requester" { requester == self.requester }
                    guard "same_approver" { approver_id == self.approver_id }
                    guard "same_reviewer" { reviewer == self.reviewer }
                    guard "same_notice_route" {
                        notice_routable == (self.notice != DurableDecisionNotice::NotRequired)
                    }
                    update {}
                    to Open
                    emit HoldReplayed { decision_id: self.decision_id }
                }

                // A hold that names this decision with another binding.
                transition HoldConflict {
                    per_phase [Open, Approved, Rejected, Expired, Withdrawn, Executed, ExecutionRefused]
                    on input Hold {
                        decision_id,
                        now_ms,
                        deadline_at_ms,
                        subject_digest,
                        operation_id,
                        request_digest,
                        requester,
                        approver_id,
                        reviewer,
                        notice_routable
                    }
                    guard "a_sealed_field_differs" {
                        decision_id != self.decision_id
                        || deadline_at_ms != self.deadline_at_ms
                        || subject_digest != self.subject_digest
                        || operation_id != self.operation_id
                        || request_digest != self.request_digest
                        || requester != self.requester
                        || approver_id != self.approver_id
                        || reviewer != self.reviewer
                        || notice_routable != (self.notice != DurableDecisionNotice::NotRequired)
                    }
                    update {}
                    to Open
                    emit Refused { decision_id: self.decision_id, reason: DurableDecisionRefusal::HoldConflict }
                }

                // ----------------------------------------------------------
                // Present
                // ----------------------------------------------------------

                transition PresentAtDeadline {
                    on input Present { audience, digest, complete, presented_at_ms, now_ms }
                    guard "open" { self.lifecycle_phase == Phase::Open }
                    guard "deadline_reached" { self.deadline_at_ms <= now_ms }
                    update {
                        self.ended_at_ms = Some(now_ms);
                        self.notice = owed_if_due(self.notice);
                    }
                    to Expired
                    emit Expired {
                        decision_id: self.decision_id,
                        deadline_at_ms: self.deadline_at_ms,
                        observed_at_ms: now_ms
                    }
                    emit Refused { decision_id: self.decision_id, reason: DurableDecisionRefusal::Expired }
                }

                transition PresentRefusedSubjectChanged {
                    on input Present { audience, digest, complete, presented_at_ms, now_ms }
                    guard "open" { self.lifecycle_phase == Phase::Open }
                    guard "before_deadline" { now_ms < self.deadline_at_ms }
                    guard "subject_differs" { digest != self.subject_digest }
                    update {}
                    to Open
                    emit Refused {
                        decision_id: self.decision_id,
                        reason: DurableDecisionRefusal::SubjectChanged
                    }
                }

                transition PresentRefusedIncomplete {
                    on input Present { audience, digest, complete, presented_at_ms, now_ms }
                    guard "open" { self.lifecycle_phase == Phase::Open }
                    guard "before_deadline" { now_ms < self.deadline_at_ms }
                    guard "subject_matches" { digest == self.subject_digest }
                    guard "incomplete" { !complete }
                    update {}
                    to Open
                    emit Refused {
                        decision_id: self.decision_id,
                        reason: DurableDecisionRefusal::PresentationIncomplete
                    }
                }

                transition PresentToApprover {
                    on input Present { audience, digest, complete, presented_at_ms, now_ms }
                    guard "open" { self.lifecycle_phase == Phase::Open }
                    guard "before_deadline" { now_ms < self.deadline_at_ms }
                    guard "subject_matches" { digest == self.subject_digest }
                    guard "complete" { complete }
                    guard "human_stage" { self.stage == DurableDecisionStage::Human }
                    guard "approver_audience" { audience == self.approver_id }
                    guard "timed" { presented_at_ms != 0 }
                    update {
                        self.first_presented_to_approver_at_ms = earliest_presentation(
                            self.first_presented_to_approver_at_ms,
                            Some(presented_at_ms)
                        );
                    }
                    to Open
                    emit Presented { decision_id: self.decision_id, audience: audience }
                }

                // A presentation to the approver at 0 is no reading of when
                // it was shown: it never becomes the first presentation.
                transition PresentRefusedUntimed {
                    on input Present { audience, digest, complete, presented_at_ms, now_ms }
                    guard "open" { self.lifecycle_phase == Phase::Open }
                    guard "before_deadline" { now_ms < self.deadline_at_ms }
                    guard "subject_matches" { digest == self.subject_digest }
                    guard "complete" { complete }
                    guard "human_stage" { self.stage == DurableDecisionStage::Human }
                    guard "approver_audience" { audience == self.approver_id }
                    guard "untimed" { presented_at_ms == 0 }
                    update {}
                    to Open
                    emit Refused {
                        decision_id: self.decision_id,
                        reason: DurableDecisionRefusal::PresentationIncomplete
                    }
                }

                transition PresentRefusedWrongApproverAudience {
                    on input Present { audience, digest, complete, presented_at_ms, now_ms }
                    guard "open" { self.lifecycle_phase == Phase::Open }
                    guard "before_deadline" { now_ms < self.deadline_at_ms }
                    guard "subject_matches" { digest == self.subject_digest }
                    guard "complete" { complete }
                    guard "human_stage" { self.stage == DurableDecisionStage::Human }
                    guard "other_audience" { audience != self.approver_id }
                    update {}
                    to Open
                    emit Refused {
                        decision_id: self.decision_id,
                        reason: DurableDecisionRefusal::WrongAudience
                    }
                }

                transition PresentToReviewer {
                    on input Present { audience, digest, complete, presented_at_ms, now_ms }
                    guard "open" { self.lifecycle_phase == Phase::Open }
                    guard "before_deadline" { now_ms < self.deadline_at_ms }
                    guard "subject_matches" { digest == self.subject_digest }
                    guard "complete" { complete }
                    guard "review_stage" { self.stage == DurableDecisionStage::Review }
                    guard "reviewer_audience" { self.reviewer == Some(audience) }
                    update {
                        self.presented_to_reviewer = true;
                    }
                    to Open
                    emit Presented { decision_id: self.decision_id, audience: audience }
                }

                transition PresentRefusedWrongReviewerAudience {
                    on input Present { audience, digest, complete, presented_at_ms, now_ms }
                    guard "open" { self.lifecycle_phase == Phase::Open }
                    guard "before_deadline" { now_ms < self.deadline_at_ms }
                    guard "subject_matches" { digest == self.subject_digest }
                    guard "complete" { complete }
                    guard "review_stage" { self.stage == DurableDecisionStage::Review }
                    guard "other_audience" { self.reviewer != Some(audience) }
                    update {}
                    to Open
                    emit Refused {
                        decision_id: self.decision_id,
                        reason: DurableDecisionRefusal::WrongAudience
                    }
                }

                transition PresentRefusedNotOpen {
                    per_phase [Approved, Rejected, Expired, Withdrawn, Executed, ExecutionRefused]
                    on input Present { audience, digest, complete, presented_at_ms, now_ms }
                    update {}
                    to Approved
                    emit Refused { decision_id: self.decision_id, reason: DurableDecisionRefusal::NotOpen }
                }

                // ----------------------------------------------------------
                // Decide (the human stage)
                // ----------------------------------------------------------

                transition DecideAtDeadline {
                    on input Decide {
                        approve,
                        approver_id,
                        decider,
                        digest,
                        evidence_digest,
                        eligible,
                        carried_presentation_at_ms,
                        observed_at_ms,
                        now_ms
                    }
                    guard "open" { self.lifecycle_phase == Phase::Open }
                    guard "deadline_reached" { self.deadline_at_ms <= now_ms }
                    update {
                        self.ended_at_ms = Some(now_ms);
                        self.notice = owed_if_due(self.notice);
                    }
                    to Expired
                    emit Expired {
                        decision_id: self.decision_id,
                        deadline_at_ms: self.deadline_at_ms,
                        observed_at_ms: now_ms
                    }
                    emit Refused { decision_id: self.decision_id, reason: DurableDecisionRefusal::Expired }
                }

                transition DecideRefusedReviewStage {
                    on input Decide {
                        approve,
                        approver_id,
                        decider,
                        digest,
                        evidence_digest,
                        eligible,
                        carried_presentation_at_ms,
                        observed_at_ms,
                        now_ms
                    }
                    guard "open" { self.lifecycle_phase == Phase::Open }
                    guard "before_deadline" { now_ms < self.deadline_at_ms }
                    guard "review_stage" { self.stage == DurableDecisionStage::Review }
                    update {}
                    to Open
                    emit Refused {
                        decision_id: self.decision_id,
                        reason: DurableDecisionRefusal::ReviewStage
                    }
                }

                transition DecideRefusedEvidenceRequired {
                    on input Decide {
                        approve,
                        approver_id,
                        decider,
                        digest,
                        evidence_digest,
                        eligible,
                        carried_presentation_at_ms,
                        observed_at_ms,
                        now_ms
                    }
                    guard "open" { self.lifecycle_phase == Phase::Open }
                    guard "before_deadline" { now_ms < self.deadline_at_ms }
                    guard "human_stage" { self.stage == DurableDecisionStage::Human }
                    guard "no_evidence" { evidence_digest == None }
                    update {}
                    to Open
                    emit Refused {
                        decision_id: self.decision_id,
                        reason: DurableDecisionRefusal::EvidenceRequired
                    }
                }

                transition DecideRefusedSubjectChanged {
                    on input Decide {
                        approve,
                        approver_id,
                        decider,
                        digest,
                        evidence_digest,
                        eligible,
                        carried_presentation_at_ms,
                        observed_at_ms,
                        now_ms
                    }
                    guard "open" { self.lifecycle_phase == Phase::Open }
                    guard "before_deadline" { now_ms < self.deadline_at_ms }
                    guard "human_stage" { self.stage == DurableDecisionStage::Human }
                    guard "evidence_present" { evidence_digest != None }
                    guard "subject_differs" { digest != self.subject_digest }
                    update {}
                    to Open
                    emit Refused {
                        decision_id: self.decision_id,
                        reason: DurableDecisionRefusal::SubjectChanged
                    }
                }

                transition DecideRefusedNotEligible {
                    on input Decide {
                        approve,
                        approver_id,
                        decider,
                        digest,
                        evidence_digest,
                        eligible,
                        carried_presentation_at_ms,
                        observed_at_ms,
                        now_ms
                    }
                    guard "open" { self.lifecycle_phase == Phase::Open }
                    guard "before_deadline" { now_ms < self.deadline_at_ms }
                    guard "human_stage" { self.stage == DurableDecisionStage::Human }
                    guard "evidence_present" { evidence_digest != None }
                    guard "subject_matches" { digest == self.subject_digest }
                    guard "approver_or_eligibility_fails" {
                        approver_id != self.approver_id || !eligible
                    }
                    update {}
                    to Open
                    emit Refused {
                        decision_id: self.decision_id,
                        reason: DurableDecisionRefusal::NotEligible
                    }
                }

                transition DecideRefusedSelfApproval {
                    on input Decide {
                        approve,
                        approver_id,
                        decider,
                        digest,
                        evidence_digest,
                        eligible,
                        carried_presentation_at_ms,
                        observed_at_ms,
                        now_ms
                    }
                    guard "open" { self.lifecycle_phase == Phase::Open }
                    guard "before_deadline" { now_ms < self.deadline_at_ms }
                    guard "human_stage" { self.stage == DurableDecisionStage::Human }
                    guard "evidence_present" { evidence_digest != None }
                    guard "subject_matches" { digest == self.subject_digest }
                    guard "sealed_approver" { approver_id == self.approver_id }
                    guard "eligible" { eligible }
                    guard "decider_is_requester" { decider == self.requester }
                    update {}
                    to Open
                    emit Refused {
                        decision_id: self.decision_id,
                        reason: DurableDecisionRefusal::SelfApproval
                    }
                }

                transition DecideRefusedPresentationIncomplete {
                    on input Decide {
                        approve,
                        approver_id,
                        decider,
                        digest,
                        evidence_digest,
                        eligible,
                        carried_presentation_at_ms,
                        observed_at_ms,
                        now_ms
                    }
                    guard "open" { self.lifecycle_phase == Phase::Open }
                    guard "before_deadline" { now_ms < self.deadline_at_ms }
                    guard "human_stage" { self.stage == DurableDecisionStage::Human }
                    guard "evidence_present" { evidence_digest != None }
                    guard "subject_matches" { digest == self.subject_digest }
                    guard "sealed_approver" { approver_id == self.approver_id }
                    guard "eligible" { eligible }
                    guard "decider_is_not_requester" { decider != self.requester }
                    // No presentation reached the approver, recorded or
                    // carried; or the decision event carries one at 0, which
                    // is no reading of when it was shown.
                    guard "no_timed_presentation" {
                        (
                            carried_presentation_at_ms == None
                            && self.first_presented_to_approver_at_ms == None
                        )
                        || carried_presentation_at_ms == Some(0)
                    }
                    update {}
                    to Open
                    emit Refused {
                        decision_id: self.decision_id,
                        reason: DurableDecisionRefusal::PresentationIncomplete
                    }
                }

                // The decision was observed before the approver's first
                // presentation: before the recorded one, and before the one
                // the decision event carries, whichever exist.
                transition DecideRefusedDecidedBeforePresentation {
                    on input Decide {
                        approve,
                        approver_id,
                        decider,
                        digest,
                        evidence_digest,
                        eligible,
                        carried_presentation_at_ms,
                        observed_at_ms,
                        now_ms
                    }
                    guard "open" { self.lifecycle_phase == Phase::Open }
                    guard "before_deadline" { now_ms < self.deadline_at_ms }
                    guard "human_stage" { self.stage == DurableDecisionStage::Human }
                    guard "evidence_present" { evidence_digest != None }
                    guard "subject_matches" { digest == self.subject_digest }
                    guard "sealed_approver" { approver_id == self.approver_id }
                    guard "eligible" { eligible }
                    guard "decider_is_not_requester" { decider != self.requester }
                    guard "carried_presentation_timed" { carried_presentation_at_ms != Some(0) }
                    guard "presented_or_carried" {
                        carried_presentation_at_ms != None
                        || self.first_presented_to_approver_at_ms != None
                    }
                    guard "observed_before_the_recorded_presentation" {
                        self.first_presented_to_approver_at_ms == None
                        || observed_at_ms < self.first_presented_to_approver_at_ms.get("value")
                    }
                    guard "observed_before_the_carried_presentation" {
                        carried_presentation_at_ms == None
                        || observed_at_ms < carried_presentation_at_ms.get("value")
                    }
                    update {}
                    to Open
                    emit Refused {
                        decision_id: self.decision_id,
                        reason: DurableDecisionRefusal::DecidedBeforePresentation
                    }
                }

                transition DecideApproved {
                    on input Decide {
                        approve,
                        approver_id,
                        decider,
                        digest,
                        evidence_digest,
                        eligible,
                        carried_presentation_at_ms,
                        observed_at_ms,
                        now_ms
                    }
                    guard "open" { self.lifecycle_phase == Phase::Open }
                    guard "before_deadline" { now_ms < self.deadline_at_ms }
                    guard "human_stage" { self.stage == DurableDecisionStage::Human }
                    guard "evidence_present" { evidence_digest != None }
                    guard "subject_matches" { digest == self.subject_digest }
                    guard "sealed_approver" { approver_id == self.approver_id }
                    guard "eligible" { eligible }
                    guard "decider_is_not_requester" { decider != self.requester }
                    guard "carried_presentation_timed" { carried_presentation_at_ms != Some(0) }
                    // A presentation, recorded or carried, at or before the
                    // observed decision: it was observed at or after its
                    // first presentation, the same millisecond included.
                    guard "observed_at_or_after_a_presentation" {
                        (
                            self.first_presented_to_approver_at_ms != None
                            && self.first_presented_to_approver_at_ms.get("value") <= observed_at_ms
                        )
                        || (
                            carried_presentation_at_ms != None
                            && carried_presentation_at_ms.get("value") <= observed_at_ms
                        )
                    }
                    guard "approves" { approve }
                    update {
                        self.evidence_digest = evidence_digest;
                        self.decided_at_ms = Some(now_ms);
                        self.first_presented_to_approver_at_ms = earliest_presentation(
                            self.first_presented_to_approver_at_ms,
                            carried_presentation_at_ms
                        );
                    }
                    to Approved
                    emit Approved { decision_id: self.decision_id, decided_at_ms: now_ms }
                }

                transition DecideRejected {
                    on input Decide {
                        approve,
                        approver_id,
                        decider,
                        digest,
                        evidence_digest,
                        eligible,
                        carried_presentation_at_ms,
                        observed_at_ms,
                        now_ms
                    }
                    guard "open" { self.lifecycle_phase == Phase::Open }
                    guard "before_deadline" { now_ms < self.deadline_at_ms }
                    guard "human_stage" { self.stage == DurableDecisionStage::Human }
                    guard "evidence_present" { evidence_digest != None }
                    guard "subject_matches" { digest == self.subject_digest }
                    guard "sealed_approver" { approver_id == self.approver_id }
                    guard "eligible" { eligible }
                    guard "decider_is_not_requester" { decider != self.requester }
                    guard "carried_presentation_timed" { carried_presentation_at_ms != Some(0) }
                    guard "observed_at_or_after_a_presentation" {
                        (
                            self.first_presented_to_approver_at_ms != None
                            && self.first_presented_to_approver_at_ms.get("value") <= observed_at_ms
                        )
                        || (
                            carried_presentation_at_ms != None
                            && carried_presentation_at_ms.get("value") <= observed_at_ms
                        )
                    }
                    guard "rejects" { !approve }
                    update {
                        self.evidence_digest = evidence_digest;
                        self.decided_at_ms = Some(now_ms);
                        self.ended_at_ms = Some(now_ms);
                        self.first_presented_to_approver_at_ms = earliest_presentation(
                            self.first_presented_to_approver_at_ms,
                            carried_presentation_at_ms
                        );
                        self.notice = owed_if_due(self.notice);
                    }
                    to Rejected
                    emit Rejected { decision_id: self.decision_id, decided_at_ms: now_ms }
                }

                transition DecideRefusedNotOpen {
                    per_phase [Approved, Rejected, Expired, Withdrawn, Executed, ExecutionRefused]
                    on input Decide {
                        approve,
                        approver_id,
                        decider,
                        digest,
                        evidence_digest,
                        eligible,
                        carried_presentation_at_ms,
                        observed_at_ms,
                        now_ms
                    }
                    update {}
                    to Approved
                    emit Refused { decision_id: self.decision_id, reason: DurableDecisionRefusal::NotOpen }
                }

                // ----------------------------------------------------------
                // Review (the review stage)
                // ----------------------------------------------------------

                transition ReviewAtDeadline {
                    on input Review { verdict, reviewer, digest, evidence_digest, now_ms }
                    guard "open" { self.lifecycle_phase == Phase::Open }
                    guard "deadline_reached" { self.deadline_at_ms <= now_ms }
                    update {
                        self.ended_at_ms = Some(now_ms);
                        self.notice = owed_if_due(self.notice);
                    }
                    to Expired
                    emit Expired {
                        decision_id: self.decision_id,
                        deadline_at_ms: self.deadline_at_ms,
                        observed_at_ms: now_ms
                    }
                    emit Refused { decision_id: self.decision_id, reason: DurableDecisionRefusal::Expired }
                }

                transition ReviewRefusedHumanStage {
                    on input Review { verdict, reviewer, digest, evidence_digest, now_ms }
                    guard "open" { self.lifecycle_phase == Phase::Open }
                    guard "before_deadline" { now_ms < self.deadline_at_ms }
                    guard "human_stage" { self.stage == DurableDecisionStage::Human }
                    update {}
                    to Open
                    emit Refused {
                        decision_id: self.decision_id,
                        reason: DurableDecisionRefusal::HumanStage
                    }
                }

                transition ReviewRefusedWrongReviewer {
                    on input Review { verdict, reviewer, digest, evidence_digest, now_ms }
                    guard "open" { self.lifecycle_phase == Phase::Open }
                    guard "before_deadline" { now_ms < self.deadline_at_ms }
                    guard "review_stage" { self.stage == DurableDecisionStage::Review }
                    guard "other_reviewer" { self.reviewer != Some(reviewer) }
                    update {}
                    to Open
                    emit Refused {
                        decision_id: self.decision_id,
                        reason: DurableDecisionRefusal::WrongReviewer
                    }
                }

                transition ReviewRefusedEvidenceRequired {
                    on input Review { verdict, reviewer, digest, evidence_digest, now_ms }
                    guard "open" { self.lifecycle_phase == Phase::Open }
                    guard "before_deadline" { now_ms < self.deadline_at_ms }
                    guard "review_stage" { self.stage == DurableDecisionStage::Review }
                    guard "sealed_reviewer" { self.reviewer == Some(reviewer) }
                    guard "no_evidence" { evidence_digest == None }
                    update {}
                    to Open
                    emit Refused {
                        decision_id: self.decision_id,
                        reason: DurableDecisionRefusal::EvidenceRequired
                    }
                }

                transition ReviewRefusedSubjectChanged {
                    on input Review { verdict, reviewer, digest, evidence_digest, now_ms }
                    guard "open" { self.lifecycle_phase == Phase::Open }
                    guard "before_deadline" { now_ms < self.deadline_at_ms }
                    guard "review_stage" { self.stage == DurableDecisionStage::Review }
                    guard "sealed_reviewer" { self.reviewer == Some(reviewer) }
                    guard "evidence_present" { evidence_digest != None }
                    guard "subject_differs" { digest != self.subject_digest }
                    update {}
                    to Open
                    emit Refused {
                        decision_id: self.decision_id,
                        reason: DurableDecisionRefusal::SubjectChanged
                    }
                }

                transition ReviewRefusedPresentationIncomplete {
                    on input Review { verdict, reviewer, digest, evidence_digest, now_ms }
                    guard "open" { self.lifecycle_phase == Phase::Open }
                    guard "before_deadline" { now_ms < self.deadline_at_ms }
                    guard "review_stage" { self.stage == DurableDecisionStage::Review }
                    guard "sealed_reviewer" { self.reviewer == Some(reviewer) }
                    guard "evidence_present" { evidence_digest != None }
                    guard "subject_matches" { digest == self.subject_digest }
                    guard "not_presented_to_reviewer" { !self.presented_to_reviewer }
                    update {}
                    to Open
                    emit Refused {
                        decision_id: self.decision_id,
                        reason: DurableDecisionRefusal::PresentationIncomplete
                    }
                }

                transition ReviewAllowed {
                    on input Review { verdict, reviewer, digest, evidence_digest, now_ms }
                    guard "open" { self.lifecycle_phase == Phase::Open }
                    guard "before_deadline" { now_ms < self.deadline_at_ms }
                    guard "review_stage" { self.stage == DurableDecisionStage::Review }
                    guard "sealed_reviewer" { self.reviewer == Some(reviewer) }
                    guard "evidence_present" { evidence_digest != None }
                    guard "subject_matches" { digest == self.subject_digest }
                    guard "presented_to_reviewer" { self.presented_to_reviewer }
                    guard "allows" { verdict == DurableDecisionReviewVerdict::Allow }
                    update {
                        self.evidence_digest = evidence_digest;
                        self.decided_at_ms = Some(now_ms);
                    }
                    to Approved
                    emit Approved { decision_id: self.decision_id, decided_at_ms: now_ms }
                }

                transition ReviewDenied {
                    on input Review { verdict, reviewer, digest, evidence_digest, now_ms }
                    guard "open" { self.lifecycle_phase == Phase::Open }
                    guard "before_deadline" { now_ms < self.deadline_at_ms }
                    guard "review_stage" { self.stage == DurableDecisionStage::Review }
                    guard "sealed_reviewer" { self.reviewer == Some(reviewer) }
                    guard "evidence_present" { evidence_digest != None }
                    guard "subject_matches" { digest == self.subject_digest }
                    guard "presented_to_reviewer" { self.presented_to_reviewer }
                    guard "denies" { verdict == DurableDecisionReviewVerdict::Deny }
                    update {
                        self.evidence_digest = evidence_digest;
                        self.decided_at_ms = Some(now_ms);
                        self.ended_at_ms = Some(now_ms);
                        self.notice = owed_if_due(self.notice);
                    }
                    to Rejected
                    emit Rejected { decision_id: self.decision_id, decided_at_ms: now_ms }
                }

                // An escalation hands the open decision to the human
                // approver; it records no outcome.
                transition ReviewEscalated {
                    on input Review { verdict, reviewer, digest, evidence_digest, now_ms }
                    guard "open" { self.lifecycle_phase == Phase::Open }
                    guard "before_deadline" { now_ms < self.deadline_at_ms }
                    guard "review_stage" { self.stage == DurableDecisionStage::Review }
                    guard "sealed_reviewer" { self.reviewer == Some(reviewer) }
                    guard "evidence_present" { evidence_digest != None }
                    guard "subject_matches" { digest == self.subject_digest }
                    guard "presented_to_reviewer" { self.presented_to_reviewer }
                    guard "escalates" { verdict == DurableDecisionReviewVerdict::Escalate }
                    update {
                        self.stage = DurableDecisionStage::Human;
                    }
                    to Open
                    emit Escalated { decision_id: self.decision_id }
                }

                transition ReviewRefusedNotOpen {
                    per_phase [Approved, Rejected, Expired, Withdrawn, Executed, ExecutionRefused]
                    on input Review { verdict, reviewer, digest, evidence_digest, now_ms }
                    update {}
                    to Approved
                    emit Refused { decision_id: self.decision_id, reason: DurableDecisionRefusal::NotOpen }
                }

                // ----------------------------------------------------------
                // Withdraw
                // ----------------------------------------------------------

                transition WithdrawAtDeadline {
                    on input Withdraw { withdrawer, by_host, now_ms }
                    guard "open" { self.lifecycle_phase == Phase::Open }
                    guard "deadline_reached" { self.deadline_at_ms <= now_ms }
                    update {
                        self.ended_at_ms = Some(now_ms);
                        self.notice = owed_if_due(self.notice);
                    }
                    to Expired
                    emit Expired {
                        decision_id: self.decision_id,
                        deadline_at_ms: self.deadline_at_ms,
                        observed_at_ms: now_ms
                    }
                    emit Refused { decision_id: self.decision_id, reason: DurableDecisionRefusal::Expired }
                }

                transition WithdrawByHost {
                    on input Withdraw { withdrawer, by_host, now_ms }
                    guard "open" { self.lifecycle_phase == Phase::Open }
                    guard "before_deadline" { now_ms < self.deadline_at_ms }
                    guard "host_withdraws" { by_host }
                    update {
                        self.ended_at_ms = Some(now_ms);
                        self.notice = owed_if_due(self.notice);
                    }
                    to Withdrawn
                    emit Withdrawn { decision_id: self.decision_id, observed_at_ms: now_ms }
                }

                transition WithdrawByRequester {
                    on input Withdraw { withdrawer, by_host, now_ms }
                    guard "open" { self.lifecycle_phase == Phase::Open }
                    guard "before_deadline" { now_ms < self.deadline_at_ms }
                    guard "not_by_host" { !by_host }
                    guard "withdrawer_is_requester" { withdrawer == self.requester }
                    update {
                        self.ended_at_ms = Some(now_ms);
                        self.notice = owed_if_due(self.notice);
                    }
                    to Withdrawn
                    emit Withdrawn { decision_id: self.decision_id, observed_at_ms: now_ms }
                }

                transition WithdrawRefusedNotRequester {
                    on input Withdraw { withdrawer, by_host, now_ms }
                    guard "open" { self.lifecycle_phase == Phase::Open }
                    guard "before_deadline" { now_ms < self.deadline_at_ms }
                    guard "not_by_host" { !by_host }
                    guard "withdrawer_is_not_requester" { withdrawer != self.requester }
                    update {}
                    to Open
                    emit Refused {
                        decision_id: self.decision_id,
                        reason: DurableDecisionRefusal::NotRequester
                    }
                }

                transition WithdrawRefusedNotOpen {
                    per_phase [Approved, Rejected, Expired, Withdrawn, Executed, ExecutionRefused]
                    on input Withdraw { withdrawer, by_host, now_ms }
                    update {}
                    to Approved
                    emit Refused { decision_id: self.decision_id, reason: DurableDecisionRefusal::NotOpen }
                }

                // ----------------------------------------------------------
                // Expire
                // ----------------------------------------------------------

                transition ExpireOpenDue {
                    on input Expire { now_ms }
                    guard "open" { self.lifecycle_phase == Phase::Open }
                    guard "deadline_reached" { self.deadline_at_ms <= now_ms }
                    update {
                        self.ended_at_ms = Some(now_ms);
                        self.notice = owed_if_due(self.notice);
                    }
                    to Expired
                    emit Expired {
                        decision_id: self.decision_id,
                        deadline_at_ms: self.deadline_at_ms,
                        observed_at_ms: now_ms
                    }
                }

                // An early fire: nothing changes and nothing is emitted.
                transition ExpireOpenNotDue {
                    on input Expire { now_ms }
                    guard "open" { self.lifecycle_phase == Phase::Open }
                    guard "before_deadline" { now_ms < self.deadline_at_ms }
                    update {}
                    to Open
                }

                // An approved decision with no outstanding permit expires at
                // its deadline unexecuted.
                transition ExpireApprovedDue {
                    on input Expire { now_ms }
                    guard "approved" { self.lifecycle_phase == Phase::Approved }
                    guard "deadline_reached" { self.deadline_at_ms <= now_ms }
                    guard "no_permit" { !self.permit_issued }
                    update {
                        self.ended_at_ms = Some(now_ms);
                        self.notice = owed_if_due(self.notice);
                    }
                    to Expired
                    emit Expired {
                        decision_id: self.decision_id,
                        deadline_at_ms: self.deadline_at_ms,
                        observed_at_ms: now_ms
                    }
                }

                // While a permit is outstanding the owner may have committed
                // the held request: the decision waits for its settlement.
                transition ExpireApprovedSettlementOwed {
                    on input Expire { now_ms }
                    guard "approved" { self.lifecycle_phase == Phase::Approved }
                    guard "deadline_reached" { self.deadline_at_ms <= now_ms }
                    guard "permit_outstanding" { self.permit_issued }
                    update {}
                    to Approved
                }

                transition ExpireApprovedNotDue {
                    on input Expire { now_ms }
                    guard "approved" { self.lifecycle_phase == Phase::Approved }
                    guard "before_deadline" { now_ms < self.deadline_at_ms }
                    update {}
                    to Approved
                }

                transition ExpireSettled {
                    per_phase [Rejected, Expired, Withdrawn, Executed, ExecutionRefused]
                    on input Expire { now_ms }
                    update {}
                    to Rejected
                }

                // ----------------------------------------------------------
                // AuthorizeExecution
                // ----------------------------------------------------------

                // The only source of an execution permit: an approved
                // decision, the held binding, before the deadline.
                transition AuthorizeExecution {
                    on input AuthorizeExecution { operation_id, request_digest, now_ms }
                    guard "approved" { self.lifecycle_phase == Phase::Approved }
                    guard "same_operation" { operation_id == self.operation_id }
                    guard "same_request" { request_digest == self.request_digest }
                    guard "before_deadline" { now_ms < self.deadline_at_ms }
                    update {
                        self.permit_issued = true;
                    }
                    to Approved
                    emit ExecutionAuthorized {
                        decision_id: self.decision_id,
                        operation_id: self.operation_id,
                        request_digest: self.request_digest,
                        deadline_at_ms: self.deadline_at_ms
                    }
                }

                transition AuthorizeExecutionAtDeadline {
                    on input AuthorizeExecution { operation_id, request_digest, now_ms }
                    guard "approved" { self.lifecycle_phase == Phase::Approved }
                    guard "same_operation" { operation_id == self.operation_id }
                    guard "same_request" { request_digest == self.request_digest }
                    guard "deadline_reached" { self.deadline_at_ms <= now_ms }
                    guard "no_permit" { !self.permit_issued }
                    update {
                        self.ended_at_ms = Some(now_ms);
                        self.notice = owed_if_due(self.notice);
                    }
                    to Expired
                    emit Expired {
                        decision_id: self.decision_id,
                        deadline_at_ms: self.deadline_at_ms,
                        observed_at_ms: now_ms
                    }
                    emit Refused { decision_id: self.decision_id, reason: DurableDecisionRefusal::Expired }
                }

                transition AuthorizeExecutionRefusedSettlementOwed {
                    on input AuthorizeExecution { operation_id, request_digest, now_ms }
                    guard "approved" { self.lifecycle_phase == Phase::Approved }
                    guard "same_operation" { operation_id == self.operation_id }
                    guard "same_request" { request_digest == self.request_digest }
                    guard "deadline_reached" { self.deadline_at_ms <= now_ms }
                    guard "permit_outstanding" { self.permit_issued }
                    update {}
                    to Approved
                    emit Refused {
                        decision_id: self.decision_id,
                        reason: DurableDecisionRefusal::SettlementOwed
                    }
                }

                transition AuthorizeExecutionRefusedBinding {
                    on input AuthorizeExecution { operation_id, request_digest, now_ms }
                    guard "approved" { self.lifecycle_phase == Phase::Approved }
                    guard "binding_differs" {
                        operation_id != self.operation_id || request_digest != self.request_digest
                    }
                    update {}
                    to Approved
                    emit Refused {
                        decision_id: self.decision_id,
                        reason: DurableDecisionRefusal::BindingMismatch
                    }
                }

                transition AuthorizeExecutionRefusedNotApproved {
                    per_phase [Open, Rejected, Expired, Withdrawn, Executed, ExecutionRefused]
                    on input AuthorizeExecution { operation_id, request_digest, now_ms }
                    update {}
                    to Open
                    emit Refused {
                        decision_id: self.decision_id,
                        reason: DurableDecisionRefusal::NotApproved
                    }
                }

                // ----------------------------------------------------------
                // ReleaseExecution
                // ----------------------------------------------------------

                // No commit was made under the outstanding permit (a
                // retryable refusal, or a crash before the commit): the
                // decision stays approved and owed.
                transition ReleaseExecution {
                    on input ReleaseExecution { now_ms }
                    guard "approved" { self.lifecycle_phase == Phase::Approved }
                    guard "permit_outstanding" { self.permit_issued }
                    guard "before_deadline" { now_ms < self.deadline_at_ms }
                    update {
                        self.permit_issued = false;
                    }
                    to Approved
                    emit ExecutionReleased { decision_id: self.decision_id }
                }

                // A release at or after the deadline records the expiry the
                // outstanding permit held off.
                transition ReleaseExecutionAtDeadline {
                    on input ReleaseExecution { now_ms }
                    guard "approved" { self.lifecycle_phase == Phase::Approved }
                    guard "permit_outstanding" { self.permit_issued }
                    guard "deadline_reached" { self.deadline_at_ms <= now_ms }
                    update {
                        self.permit_issued = false;
                        self.ended_at_ms = Some(now_ms);
                        self.notice = owed_if_due(self.notice);
                    }
                    to Expired
                    emit ExecutionReleased { decision_id: self.decision_id }
                    emit Expired {
                        decision_id: self.decision_id,
                        deadline_at_ms: self.deadline_at_ms,
                        observed_at_ms: now_ms
                    }
                }

                transition ReleaseExecutionRefusedNotAuthorized {
                    on input ReleaseExecution { now_ms }
                    guard "approved" { self.lifecycle_phase == Phase::Approved }
                    guard "no_permit" { !self.permit_issued }
                    update {}
                    to Approved
                    emit Refused {
                        decision_id: self.decision_id,
                        reason: DurableDecisionRefusal::NotAuthorized
                    }
                }

                transition ReleaseExecutionRefusedNotApproved {
                    per_phase [Open, Rejected, Expired, Withdrawn, Executed, ExecutionRefused]
                    on input ReleaseExecution { now_ms }
                    update {}
                    to Open
                    emit Refused {
                        decision_id: self.decision_id,
                        reason: DurableDecisionRefusal::NotApproved
                    }
                }

                // ----------------------------------------------------------
                // RecordExecuted
                // ----------------------------------------------------------

                // The owner committed the held request under an outstanding
                // permit. Accepted at any time: it settles a commit already
                // made, so the deadline is not compared.
                transition RecordExecuted {
                    on input RecordExecuted { operation_id, request_digest, now_ms }
                    guard "approved" { self.lifecycle_phase == Phase::Approved }
                    guard "same_operation" { operation_id == self.operation_id }
                    guard "same_request" { request_digest == self.request_digest }
                    guard "permit_outstanding" { self.permit_issued }
                    update {
                        self.permit_issued = false;
                        self.executed_request_digest = Some(request_digest);
                        self.ended_at_ms = Some(now_ms);
                        self.notice = owed_if_due(self.notice);
                    }
                    to Executed
                    emit Executed {
                        decision_id: self.decision_id,
                        operation_id: self.operation_id,
                        request_digest: self.request_digest,
                        executed_at_ms: now_ms
                    }
                }

                transition RecordExecutedRefusedNotAuthorized {
                    on input RecordExecuted { operation_id, request_digest, now_ms }
                    guard "approved" { self.lifecycle_phase == Phase::Approved }
                    guard "same_operation" { operation_id == self.operation_id }
                    guard "same_request" { request_digest == self.request_digest }
                    guard "no_permit" { !self.permit_issued }
                    update {}
                    to Approved
                    emit Refused {
                        decision_id: self.decision_id,
                        reason: DurableDecisionRefusal::NotAuthorized
                    }
                }

                transition RecordExecutedRefusedBinding {
                    on input RecordExecuted { operation_id, request_digest, now_ms }
                    guard "approved" { self.lifecycle_phase == Phase::Approved }
                    guard "binding_differs" {
                        operation_id != self.operation_id || request_digest != self.request_digest
                    }
                    update {}
                    to Approved
                    emit Refused {
                        decision_id: self.decision_id,
                        reason: DurableDecisionRefusal::BindingMismatch
                    }
                }

                // The same settlement again changes nothing: the executor's
                // normal path and a settlement after a crash are one input.
                transition RecordExecutedReplayed {
                    on input RecordExecuted { operation_id, request_digest, now_ms }
                    guard "executed" { self.lifecycle_phase == Phase::Executed }
                    guard "same_operation" { operation_id == self.operation_id }
                    guard "same_request" { request_digest == self.request_digest }
                    update {}
                    to Executed
                    emit ExecutionReplayed { decision_id: self.decision_id }
                }

                transition RecordExecutedReplayRefusedBinding {
                    on input RecordExecuted { operation_id, request_digest, now_ms }
                    guard "executed" { self.lifecycle_phase == Phase::Executed }
                    guard "binding_differs" {
                        operation_id != self.operation_id || request_digest != self.request_digest
                    }
                    update {}
                    to Executed
                    emit Refused {
                        decision_id: self.decision_id,
                        reason: DurableDecisionRefusal::BindingMismatch
                    }
                }

                transition RecordExecutedRefusedNotApproved {
                    per_phase [Open, Rejected, Expired, Withdrawn, ExecutionRefused]
                    on input RecordExecuted { operation_id, request_digest, now_ms }
                    update {}
                    to Open
                    emit Refused {
                        decision_id: self.decision_id,
                        reason: DurableDecisionRefusal::NotApproved
                    }
                }

                // ----------------------------------------------------------
                // CloseRefused
                // ----------------------------------------------------------

                // The owner refused the held request permanently before the
                // deadline: the decision closes and owes its notice.
                transition CloseRefused {
                    on input CloseRefused { now_ms }
                    guard "approved" { self.lifecycle_phase == Phase::Approved }
                    guard "before_deadline" { now_ms < self.deadline_at_ms }
                    update {
                        self.permit_issued = false;
                        self.ended_at_ms = Some(now_ms);
                        self.notice = owed_if_due(self.notice);
                    }
                    to ExecutionRefused
                    emit ExecutionRefused { decision_id: self.decision_id, closed_at_ms: now_ms }
                }

                // At or after the deadline a permanent refusal settles the
                // outstanding permit: no commit was made.
                transition CloseRefusedWithPermit {
                    on input CloseRefused { now_ms }
                    guard "approved" { self.lifecycle_phase == Phase::Approved }
                    guard "deadline_reached" { self.deadline_at_ms <= now_ms }
                    guard "permit_outstanding" { self.permit_issued }
                    update {
                        self.permit_issued = false;
                        self.ended_at_ms = Some(now_ms);
                        self.notice = owed_if_due(self.notice);
                    }
                    to ExecutionRefused
                    emit ExecutionRefused { decision_id: self.decision_id, closed_at_ms: now_ms }
                }

                // With no permit outstanding the approval had already lapsed:
                // the refusal records the expiry instead.
                transition CloseRefusedAtDeadline {
                    on input CloseRefused { now_ms }
                    guard "approved" { self.lifecycle_phase == Phase::Approved }
                    guard "deadline_reached" { self.deadline_at_ms <= now_ms }
                    guard "no_permit" { !self.permit_issued }
                    update {
                        self.ended_at_ms = Some(now_ms);
                        self.notice = owed_if_due(self.notice);
                    }
                    to Expired
                    emit Expired {
                        decision_id: self.decision_id,
                        deadline_at_ms: self.deadline_at_ms,
                        observed_at_ms: now_ms
                    }
                    emit Refused { decision_id: self.decision_id, reason: DurableDecisionRefusal::Expired }
                }

                transition CloseRefusedNotApproved {
                    per_phase [Open, Rejected, Expired, Withdrawn, Executed, ExecutionRefused]
                    on input CloseRefused { now_ms }
                    update {}
                    to Open
                    emit Refused {
                        decision_id: self.decision_id,
                        reason: DurableDecisionRefusal::NotApproved
                    }
                }

                // ----------------------------------------------------------
                // SettleNotice
                // ----------------------------------------------------------

                transition SettleNoticeDelivered {
                    per_phase [Rejected, Expired, Withdrawn, Executed, ExecutionRefused]
                    on input SettleNotice { delivered }
                    guard "notice_owed" { self.notice == DurableDecisionNotice::Owed }
                    guard "delivered" { delivered }
                    update {
                        self.notice = DurableDecisionNotice::Delivered;
                    }
                    to Rejected
                    emit NoticeSettled { decision_id: self.decision_id, delivered: delivered }
                }

                transition SettleNoticeFailed {
                    per_phase [Rejected, Expired, Withdrawn, Executed, ExecutionRefused]
                    on input SettleNotice { delivered }
                    guard "notice_owed" { self.notice == DurableDecisionNotice::Owed }
                    guard "failed" { !delivered }
                    update {
                        self.notice = DurableDecisionNotice::Failed;
                    }
                    to Rejected
                    emit NoticeSettled { decision_id: self.decision_id, delivered: delivered }
                }

                // The same receipt again changes nothing.
                transition SettleNoticeReplayed {
                    per_phase [Rejected, Expired, Withdrawn, Executed, ExecutionRefused]
                    on input SettleNotice { delivered }
                    guard "same_receipt" {
                        (self.notice == DurableDecisionNotice::Delivered && delivered)
                        || (self.notice == DurableDecisionNotice::Failed && !delivered)
                    }
                    update {}
                    to Rejected
                    emit NoticeSettled { decision_id: self.decision_id, delivered: delivered }
                }

                // No notice is owed, or the receipt contradicts the settled
                // one.
                transition SettleNoticeRefused {
                    per_phase [Rejected, Expired, Withdrawn, Executed, ExecutionRefused]
                    on input SettleNotice { delivered }
                    guard "not_owed" {
                        self.notice == DurableDecisionNotice::NotRequired
                        || (self.notice == DurableDecisionNotice::Delivered && !delivered)
                        || (self.notice == DurableDecisionNotice::Failed && delivered)
                    }
                    update {}
                    to Rejected
                    emit Refused { decision_id: self.decision_id, reason: DurableDecisionRefusal::NotOwed }
                }

                transition SettleNoticeRefusedNotEnded {
                    per_phase [Open, Approved]
                    on input SettleNotice { delivered }
                    update {}
                    to Open
                    emit Refused { decision_id: self.decision_id, reason: DurableDecisionRefusal::NotOwed }
                }
            }
        }
    };
}

crate::durable_decision_catalog_machine_dsl!("self", "catalog::dsl::durable_decision");
