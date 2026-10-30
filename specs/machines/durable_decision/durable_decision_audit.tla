---- MODULE durable_decision_audit ----
\* Hand-written bounded audit of the generated DurableDecisionMachine model.
\*
\* One durable decision holds one operation until an attested party decides
\* it, it is withdrawn, or its deadline passes; the operation's owner then
\* executes the held request at most once, under an execution permit. The
\* generated model is one decision record; this audit keeps it unchanged and
\* drives it with every input in every phase, through wrappers that read
\* each step's input and record what the step did.
\*
\* The environment. One binding is held: the requester "alpha" asks to run
\* operation "alpha" with request digest "alpha", whose subject digest is
\* "alpha", for the approver id "beta"; "beta" is also the named reviewer of
\* a review-stage hold. A hold may name no reviewer, the reviewer, or the
\* requester as its own reviewer (refused), under either deadline (1 or 2)
\* and either notice route; a later hold that differs from the sealed one in
\* any of them is a conflict, an equal one a replay. A human decision is
\* valid or has exactly one defect (no evidence, a stale digest, another
\* approver id, an ineligible decider, the requester as decider). A valid
\* one carries no presentation or one shown at 0, 1 or 2 (0 is no reading),
\* and its ingress observed it at any time in 0..2, before, at or after each
\* presentation; one with a defect carries a presentation shown at 1,
\* observed at 1. A complete presentation of the held subject to the
\* approver was shown at 0, 1 or 2, any other at 1, whatever clock reading
\* records it; the first presentation is the earliest reading. A review
\* verdict is the
\* reviewer's or another party's, with or without evidence, for the held or
\* a stale digest. Every input carries a clock reading in 0..2 and may
\* arrive in any phase: the machine reads no clock, so readings may repeat
\* or go back. The owner commits the held request under an outstanding
\* permit at most once (AuditCommit: its commit refuses a permit at or after
\* the permit's deadline, so the commit is made before it); after its commit
\* it never asks for, releases or refuses a permit again, and only records
\* the execution. Any other party's input, a deadline observation and a
\* replayed settlement may still arrive.
\*
\* Properties. Each step property is checked on every step by its wrapper,
\* which records the property in audit_broken when the step breaks it; the
\* invariant of the same name requires that it never was:
\*
\*   AuditTerminalsLatch: an ended decision (Rejected, Expired, Withdrawn,
\*     Executed, ExecutionRefused) never moves again; only its owed notice
\*     settles.
\*   AuditApprovalIsAttested: an approval comes only from an open decision,
\*     before its deadline, by an attested decision bound to the held
\*     subject digest: a human acting for the sealed approver id, eligible,
\*     not the requester, shown a complete presentation (carried or
\*     recorded); or the sealed reviewer, shown one; with its evidence and
\*     decision time recorded (I-C7).
\*   AuditRejectionIsAttested: the same for a rejection, which also ends it.
\*   AuditExecutedOnlyFromApprovedWithHeldBinding: an execution is recorded
\*     only on an approved decision with an outstanding permit, for the held
\*     operation and request, and it clears the permit (I-C4).
\*   AuditExpiryOnlyAtTheDeadline: an open or approved decision expires only
\*     on a reading at or after its deadline, which it records.
\*   AuditApprovedExpiresOnlyWithoutPermit: an approved decision expires only
\*     with no permit outstanding, or by the release that clears it.
\*   AuditWithdrawOnlyOpenByRequesterOrHost: only an open decision is
\*     withdrawn, before its deadline, by its requester or the host.
\*   AuditSealedFieldsNeverChange: once held, the binding (decision id,
\*     deadline, subject, operation, request, requester, approver id,
\*     reviewer) and the notice route never change.
\*   AuditNoticeOwedOnceAtTheEnd: the notice of a routable requester becomes
\*     owed exactly when the decision ends, never earlier, and only its
\*     receipt settles it.
\*   AuditNoPermitAtOrAfterDeadline: a permit is issued only by an
\*     authorization of the held binding on an approved decision before its
\*     deadline.
\*   AuditReleaseClearsPermit: a release of an outstanding permit clears it.
\*   AuditDecisionFollowsPresentation: a human decision is decided only when
\*     its ingress observed it at or after the approver's first presentation
\*     (the earliest the audit saw recorded, or the one the decision
\*     carries, a time of 0 being none), and is refused for the order only
\*     when observed before it.
\* and two state invariants:
\*   AuditCommittedIsSettleable: once the owner committed, the decision is
\*     Executed, or Approved with the permit still outstanding, so the commit
\*     is always settled as executed: no expiry, refusal or lost permit
\*     orphans it.
\*   AuditFirstPresentationIsTheEarliest: the machine's first presentation
\*     time is the earliest approver presentation the audit saw, recorded or
\*     carried, in whatever order the presentations arrived. The audit reads
\*     a recorded one from the input itself (a complete presentation of the
\*     held subject, at a reading other than 0, to the approver id of an
\*     open human-stage decision before its deadline), never from the arm
\*     that fired.
\*
\* Reachability (the runner): every wrapper fires in the bounded space,
\* checked from TLC's coverage, and fifteen scenarios are checked as
\* expected violations of their negations: a human approval executed and its
\* notice delivered; an execution retried after a release; an approval
\* closed by the owner's permanent refusal; a review escalated and then
\* approved by the human; a stale digest refused and then an approval; a
\* decision at the deadline that records the expiry; a decision refused
\* after a withdrawal; a self-approval refused and then another approver's;
\* a replayed settlement after the commit; a replayed and a conflicting
\* hold; a permit outstanding across the deadline settled as executed; a
\* release after the deadline that records the expiry; an authorization at
\* the deadline refused; a decision observed before the first presentation
\* refused and then an approval; and an approval observed in the same
\* millisecond as the first presentation. With --mutants the runner seeds one
\* defect per property into a copy of the generated model and requires TLC
\* to refuse each with that property.
\*
\* The executor's obligations. The machine records that a permit is
\* outstanding, not who holds it or whether a commit landed under it. The
\* owner wrappers below encode what the operation's owner must keep, and the
\* audit assumes it: one executor per decision (one owner drives the permit
\* and AuditCommit); never a release or a refusal after the commit (only the
\* execution is recorded); and settlement first after a restart (the owner
\* reads its own commit record before it releases). AuditCommittedIsSettleable
\* holds only under them: a release after the commit orphans it (hold,
\* approve, authorize, commit, release). DecisionExecutionPermit and the
\* settlement calls of DurableDecisionService state the same obligations.
\*
\* The TLC config is DERIVED from the generated ci.cfg on every run by
\* durable_decision_audit.sh:
\*   specs/machines/durable_decision/durable_decision_audit.sh 6 --mutants
EXTENDS model

CONSTANT AuditMaxSteps

\* The held binding's values, and the other value of each key type.
AuditHeld == "alpha"
AuditOther == "beta"
AuditRequester == "alpha"
AuditApprover == "beta"
AuditReviewer == "beta"
AuditEvidence == Some("alpha")
AuditTimes == 0..2
\* The readings a presentation is shown at: a decision observed at a time in
\* AuditTimes falls before, at or after each. A time of 0 is no reading.
AuditShownTimes == 1..2

VARIABLES
    audit_committed,        \* the owner committed the held request under a permit
    audit_broken,           \* the step properties some step broke
    audit_seen,             \* the scenario marks the witnesses read
    audit_first_presented   \* the earliest approver presentation time accepted

auditVars == << audit_committed, audit_broken, audit_seen, audit_first_presented >>

AuditInit ==
    /\ Init
    /\ audit_committed = FALSE
    /\ audit_broken = {}
    /\ audit_seen = {}
    /\ audit_first_presented = None

\* --- inputs -----------------------------------------------------------

AuditHoldInputs ==
    { [decision_id |-> AuditHeld, now_ms |-> n, deadline_at_ms |-> d,
       subject_digest |-> AuditHeld, operation_id |-> AuditHeld,
       request_digest |-> AuditHeld, requester |-> AuditRequester,
       approver_id |-> AuditApprover, reviewer |-> r, notice_routable |-> b] :
      n \in AuditTimes, d \in 1..2,
      r \in {None, Some(AuditReviewer), Some(AuditRequester)}, b \in BOOLEAN }

\* A complete presentation of the held subject to the approver is shown at
\* 0 (no reading) or at a reading; any other presentation records no time,
\* so it is shown at 1.
AuditPresentInputs ==
    UNION { { [audience |-> a, digest |-> g, complete |-> c, presented_at_ms |-> p,
               now_ms |-> n] :
              p \in IF a = AuditApprover /\ g = AuditHeld /\ c THEN AuditTimes ELSE {1},
              n \in AuditTimes } :
            <<a, g, c>> \in {AuditRequester, AuditApprover} \X {AuditHeld, AuditOther}
                             \X BOOLEAN }

AuditValidDecision ==
    [approver_id |-> AuditApprover, decider |-> AuditApprover,
     digest |-> AuditHeld, evidence_digest |-> AuditEvidence, eligible |-> TRUE]

\* A valid human decision, and the same with exactly one defect.
AuditDecideShapes ==
    { AuditValidDecision,
      [AuditValidDecision EXCEPT !.evidence_digest = None],
      [AuditValidDecision EXCEPT !.digest = AuditOther],
      [AuditValidDecision EXCEPT !.approver_id = AuditRequester],
      [AuditValidDecision EXCEPT !.eligible = FALSE],
      [AuditValidDecision EXCEPT !.decider = AuditRequester] }

\* The valid decision carries no presentation, one at 0 (no reading), or
\* one shown at either reading, and was observed at any time: before, at or
\* after each presentation. A decision with a defect is refused whatever
\* its times, so it carries a presentation it was observed at.
AuditDecideTimes(s) ==
    IF s = AuditValidDecision
    THEN { [carried |-> c, observed |-> o] :
           c \in {None} \cup {Some(t) : t \in AuditTimes}, o \in AuditTimes }
    ELSE { [carried |-> Some(1), observed |-> 1] }

AuditDecideInputs ==
    UNION { { [approve |-> a, approver_id |-> s.approver_id, decider |-> s.decider,
               digest |-> s.digest, evidence_digest |-> s.evidence_digest,
               eligible |-> s.eligible, carried_presentation_at_ms |-> t.carried,
               observed_at_ms |-> t.observed, now_ms |-> n] :
              a \in BOOLEAN, t \in AuditDecideTimes(s), n \in AuditTimes } :
            s \in AuditDecideShapes }

\* The reviewer's verdict, and the same by another party, without evidence,
\* or for a stale digest.
AuditReviewShapes ==
    LET valid == [reviewer |-> AuditReviewer, digest |-> AuditHeld,
                  evidence_digest |-> AuditEvidence]
    IN { valid,
         [valid EXCEPT !.reviewer = AuditRequester],
         [valid EXCEPT !.evidence_digest = None],
         [valid EXCEPT !.digest = AuditOther] }

AuditReviewInputs ==
    { [verdict |-> v, reviewer |-> s.reviewer, digest |-> s.digest,
       evidence_digest |-> s.evidence_digest, now_ms |-> n] :
      v \in DurableDecisionReviewVerdictValues, s \in AuditReviewShapes,
      n \in AuditTimes }

AuditWithdrawInputs ==
    { [withdrawer |-> w, by_host |-> h, now_ms |-> n] :
      w \in {AuditRequester, AuditApprover}, h \in BOOLEAN, n \in AuditTimes }

AuditClockInputs == { [now_ms |-> n] : n \in AuditTimes }

AuditExecutionInputs ==
    { [operation_id |-> AuditHeld, request_digest |-> g, now_ms |-> n] :
      g \in {AuditHeld, AuditOther}, n \in AuditTimes }

AuditNoticeInputs == { [delivered |-> b] : b \in BOOLEAN }

\* One application of a generated arm to an input record, per input.
HoldArm(Arm(_, _, _, _, _, _, _, _, _, _), in) ==
    Arm(in.decision_id, in.now_ms, in.deadline_at_ms, in.subject_digest,
        in.operation_id, in.request_digest, in.requester, in.approver_id,
        in.reviewer, in.notice_routable)
PresentArm(Arm(_, _, _, _, _), in) ==
    Arm(in.audience, in.digest, in.complete, in.presented_at_ms, in.now_ms)
DecideArm(Arm(_, _, _, _, _, _, _, _, _), in) ==
    Arm(in.approve, in.approver_id, in.decider, in.digest, in.evidence_digest,
        in.eligible, in.carried_presentation_at_ms, in.observed_at_ms, in.now_ms)
ReviewArm(Arm(_, _, _, _, _), in) ==
    Arm(in.verdict, in.reviewer, in.digest, in.evidence_digest, in.now_ms)
WithdrawArm(Arm(_, _, _), in) == Arm(in.withdrawer, in.by_host, in.now_ms)
ExpireArm(Arm(_), in) == Arm(in.now_ms)
AuthorizeExecutionArm(Arm(_, _, _), in) == Arm(in.operation_id, in.request_digest, in.now_ms)
ReleaseExecutionArm(Arm(_), in) == Arm(in.now_ms)
RecordExecutedArm(Arm(_, _, _), in) == Arm(in.operation_id, in.request_digest, in.now_ms)
CloseRefusedArm(Arm(_), in) == Arm(in.now_ms)
SettleNoticeArm(Arm(_), in) == Arm(in.delivered)

\* --- step properties --------------------------------------------------
\* Each reads the step's input record, the state before (unprimed) and the
\* state after (primed). The input record of a kind has only its own fields,
\* so each property reads a field only after checking the kind.

AuditEnded(p) == p \in {"Rejected", "Expired", "Withdrawn", "Executed", "ExecutionRefused"}

AuditLatchKept ==
    AuditEnded(phase) =>
        /\ phase' = phase
        /\ stage' = stage
        /\ first_presented_to_approver_at_ms' = first_presented_to_approver_at_ms
        /\ presented_to_reviewer' = presented_to_reviewer
        /\ evidence_digest' = evidence_digest
        /\ decided_at_ms' = decided_at_ms
        /\ ended_at_ms' = ended_at_ms
        /\ permit_issued' = permit_issued
        /\ executed_request_digest' = executed_request_digest
        /\ \/ notice' = notice
           \/ notice = "Owed" /\ notice' \in {"Delivered", "Failed"}

\* The attestation an approval and a rejection share.
AuditDecisionAttested(kind, in) ==
    /\ kind \in {"Decide", "Review"}
    /\ phase = "Open"
    /\ in.now_ms < deadline_at_ms
    /\ in.evidence_digest # None
    /\ in.digest = subject_digest
    /\ evidence_digest' = in.evidence_digest
    /\ decided_at_ms' = Some(in.now_ms)
    /\ IF kind = "Decide"
       THEN (/\ stage = "Human"
             /\ in.approver_id = approver_id
             /\ in.eligible
             /\ in.decider # requester
             /\ (in.carried_presentation_at_ms # None
                 \/ first_presented_to_approver_at_ms # None))
       ELSE (/\ stage = "Review"
             /\ reviewer = Some(in.reviewer)
             /\ presented_to_reviewer)

AuditApprovalAttested(kind, in) ==
    (phase # "Approved" /\ phase' = "Approved") =>
        /\ AuditDecisionAttested(kind, in)
        /\ IF kind = "Decide" THEN in.approve ELSE in.verdict = "Allow"

AuditRejectionAttested(kind, in) ==
    (phase # "Rejected" /\ phase' = "Rejected") =>
        /\ AuditDecisionAttested(kind, in)
        /\ IF kind = "Decide" THEN ~in.approve ELSE in.verdict = "Deny"
        /\ ended_at_ms' = Some(in.now_ms)

AuditExecutionBound(kind, in) ==
    (phase # "Executed" /\ phase' = "Executed") =>
        /\ kind = "RecordExecuted"
        /\ phase = "Approved"
        /\ permit_issued
        /\ in.operation_id = operation_id
        /\ in.request_digest = request_digest
        /\ executed_request_digest' = Some(request_digest)
        /\ ~permit_issued'

AuditExpiryAtDeadline(kind, in) ==
    (phase # "Expired" /\ phase' = "Expired") =>
        /\ kind # "SettleNotice"
        /\ phase \in {"Open", "Approved"}
        /\ deadline_at_ms <= in.now_ms
        /\ ended_at_ms' = Some(in.now_ms)

AuditApprovedExpiryWithoutPermit(kind) ==
    (phase = "Approved" /\ phase' = "Expired")
        => (~permit_issued \/ kind = "ReleaseExecution")

AuditWithdrawal(kind, in) ==
    (phase # "Withdrawn" /\ phase' = "Withdrawn") =>
        /\ kind = "Withdraw"
        /\ phase = "Open"
        /\ in.now_ms < deadline_at_ms
        /\ (in.by_host \/ in.withdrawer = requester)

AuditSealedKept ==
    phase # "Unheld" =>
        /\ decision_id' = decision_id
        /\ deadline_at_ms' = deadline_at_ms
        /\ subject_digest' = subject_digest
        /\ operation_id' = operation_id
        /\ request_digest' = request_digest
        /\ requester' = requester
        /\ approver_id' = approver_id
        /\ reviewer' = reviewer
        /\ (notice' = "NotRequired") = (notice = "NotRequired")

AuditNoticeKept(kind) ==
    /\ (~AuditEnded(phase) /\ AuditEnded(phase'))
           => notice' = (IF notice = "NotDue" THEN "Owed" ELSE notice)
    /\ (phase # "Unheld" /\ ~AuditEnded(phase')) => notice' = notice
    /\ (AuditEnded(phase) /\ notice' # notice) => (kind = "SettleNotice" /\ notice = "Owed")

AuditPermitBeforeDeadline(arm, in) ==
    /\ (~permit_issued /\ permit_issued') => arm = "AuthorizeExecution"
    /\ arm = "AuthorizeExecution" =>
           /\ phase = "Approved"
           /\ in.now_ms < deadline_at_ms
           /\ in.operation_id = operation_id
           /\ in.request_digest = request_digest
           /\ permit_issued'

AuditReleaseClears(kind) ==
    (kind = "ReleaseExecution" /\ phase = "Approved" /\ permit_issued) => ~permit_issued'

\* The time of a present optional time. It never names the option's value
\* field: TLC compares record fields in the order their names were first
\* seen, and the model's None must keep comparing its tag first.
AuditTimeOf(time) == CHOOSE t \in AuditTimes : Some(t) = time

\* The earlier of two optional presentation times.
AuditEarliest(a, b) ==
    IF a = None THEN b
    ELSE IF b = None THEN a
    ELSE IF AuditTimeOf(b) < AuditTimeOf(a) THEN b ELSE a

\* The presentation a decision carries, when its time is a reading (0 is
\* none). Read only for a Decide input.
AuditTimedCarried(in) ==
    IF in.carried_presentation_at_ms # None
       /\ AuditTimeOf(in.carried_presentation_at_ms) > 0
    THEN in.carried_presentation_at_ms
    ELSE None

\* The approver's first presentation as a human decision sees it: the
\* earliest the audit saw recorded, or the one the decision carries. Read
\* only for a Decide input.
AuditFirstShown(in) == AuditEarliest(audit_first_presented, AuditTimedCarried(in))

\* A presentation the approver's first presentation time comes from, read
\* from the input itself and never from the arm that fired: on an open
\* human-stage decision before its deadline, a complete presentation of
\* the held subject to the approver id, shown at a reading other than 0.
AuditApproverPresentation(kind, in) ==
    /\ kind = "Present"
    /\ phase = "Open"
    /\ stage = "Human"
    /\ in.now_ms < deadline_at_ms
    /\ in.audience = approver_id
    /\ in.digest = subject_digest
    /\ in.complete
    /\ in.presented_at_ms > 0

AuditPresentationOrderKept(kind, arm, in) ==
    /\ (kind = "Decide" /\ phase = "Open" /\ phase' \in {"Approved", "Rejected"})
           => /\ AuditFirstShown(in) # None
              /\ AuditTimeOf(AuditFirstShown(in)) <= in.observed_at_ms
    /\ arm = "DecideRefusedDecidedBeforePresentation"
           => /\ AuditFirstShown(in) # None
              /\ in.observed_at_ms < AuditTimeOf(AuditFirstShown(in))

\* The earliest approver presentation after the step: a recorded one, or
\* the one a decision that the step decided carried.
AuditFirstPresentedAfter(kind, in) ==
    IF AuditApproverPresentation(kind, in)
    THEN AuditEarliest(audit_first_presented, Some(in.presented_at_ms))
    ELSE IF kind = "Decide" /\ phase = "Open" /\ phase' \in {"Approved", "Rejected"}
         THEN AuditFirstShown(in)
         ELSE audit_first_presented

AuditBreaks(kind, arm, in) ==
    (IF AuditLatchKept THEN {} ELSE {"TerminalsLatch"})
    \cup (IF AuditApprovalAttested(kind, in) THEN {} ELSE {"ApprovalIsAttested"})
    \cup (IF AuditRejectionAttested(kind, in) THEN {} ELSE {"RejectionIsAttested"})
    \cup (IF AuditExecutionBound(kind, in) THEN {}
          ELSE {"ExecutedOnlyFromApprovedWithHeldBinding"})
    \cup (IF AuditExpiryAtDeadline(kind, in) THEN {} ELSE {"ExpiryOnlyAtTheDeadline"})
    \cup (IF AuditApprovedExpiryWithoutPermit(kind) THEN {}
          ELSE {"ApprovedExpiresOnlyWithoutPermit"})
    \cup (IF AuditWithdrawal(kind, in) THEN {} ELSE {"WithdrawOnlyOpenByRequesterOrHost"})
    \cup (IF AuditSealedKept THEN {} ELSE {"SealedFieldsNeverChange"})
    \cup (IF AuditNoticeKept(kind) THEN {} ELSE {"NoticeOwedOnceAtTheEnd"})
    \cup (IF AuditPermitBeforeDeadline(arm, in) THEN {} ELSE {"NoPermitAtOrAfterDeadline"})
    \cup (IF AuditReleaseClears(kind) THEN {} ELSE {"ReleaseClearsPermit"})
    \cup (IF AuditPresentationOrderKept(kind, arm, in) THEN {}
          ELSE {"DecisionFollowsPresentation"})

\* The scenario marks a step leaves for the witnesses.
AuditMarks(arm, in) ==
    (IF arm = "ReleaseExecution" THEN {"Released"} ELSE {})
    \cup (IF arm \in {"PresentRefusedSubjectChanged", "DecideRefusedSubjectChanged",
                      "ReviewRefusedSubjectChanged"}
          THEN {"StaleRefused"} ELSE {})
    \cup (IF arm = "DecideAtDeadline" THEN {"DecideExpired"} ELSE {})
    \cup (IF arm = "DecideRefusedNotOpen" /\ phase = "Withdrawn"
          THEN {"DecideRefusedWithdrawn"} ELSE {})
    \cup (IF arm = "DecideRefusedSelfApproval" THEN {"SelfApprovalRefused"} ELSE {})
    \cup (IF arm = "RecordExecutedReplayed" /\ audit_committed THEN {"RecordReplayed"} ELSE {})
    \cup (IF arm \in {"HoldReplayed", "HoldConflict"} THEN {arm} ELSE {})
    \cup (IF arm = "ExpireApprovedSettlementOwed" THEN {"ExpiryHeldOff"} ELSE {})
    \cup (IF arm = "ReleaseExecutionAtDeadline" THEN {"LateRelease"} ELSE {})
    \cup (IF arm \in {"AuthorizeExecutionAtDeadline", "AuthorizeExecutionRefusedSettlementOwed"}
          THEN {"LateAuthorizeRefused"} ELSE {})
    \cup (IF arm = "DecideRefusedDecidedBeforePresentation"
          THEN {"DecidedBeforePresentationRefused"} ELSE {})
    \cup (IF arm = "DecideApproved" /\ AuditFirstShown(in) # None
              /\ in.observed_at_ms = AuditTimeOf(AuditFirstShown(in))
          THEN {"ApprovedAtThePresentation"} ELSE {})

\* Every wrapper ends with this step: the step properties it broke, the
\* marks it left and the earliest approver presentation accepted.
AuditStep(kind, arm, in) ==
    /\ audit_broken' = audit_broken \cup AuditBreaks(kind, arm, in)
    /\ audit_seen' = audit_seen \cup AuditMarks(arm, in)
    /\ audit_first_presented' = AuditFirstPresentedAfter(kind, in)
    /\ UNCHANGED audit_committed

\* --- the owner's commit -----------------------------------------------

\* The owner commits the held request under the outstanding permit, once.
AuditCommit ==
    /\ ~audit_committed
    /\ phase = "Approved"
    /\ permit_issued
    /\ audit_committed' = TRUE
    /\ UNCHANGED << vars, audit_broken, audit_seen, audit_first_presented >>

\* --- one wrapper per generated arm (a per-phase arm's phases share one) -
\* The owner never asks for, releases or refuses a permit after its commit.

AuditHoldHuman(in) ==
    /\ HoldArm(HoldHuman, in)
    /\ AuditStep("Hold", "HoldHuman", in)

AuditHoldReview(in) ==
    /\ HoldArm(HoldReview, in)
    /\ AuditStep("Hold", "HoldReview", in)

AuditHoldRefusedWindow(in) ==
    /\ HoldArm(HoldRefusedWindow, in)
    /\ AuditStep("Hold", "HoldRefusedWindow", in)

AuditHoldRefusedSelfReview(in) ==
    /\ HoldArm(HoldRefusedSelfReview, in)
    /\ AuditStep("Hold", "HoldRefusedSelfReview", in)

AuditHoldReplayed(in) ==
    /\ \/ HoldArm(HoldReplayedOpen, in)
       \/ HoldArm(HoldReplayedApproved, in)
       \/ HoldArm(HoldReplayedRejected, in)
       \/ HoldArm(HoldReplayedExpired, in)
       \/ HoldArm(HoldReplayedWithdrawn, in)
       \/ HoldArm(HoldReplayedExecuted, in)
       \/ HoldArm(HoldReplayedExecutionRefused, in)
    /\ AuditStep("Hold", "HoldReplayed", in)

AuditHoldConflict(in) ==
    /\ \/ HoldArm(HoldConflictOpen, in)
       \/ HoldArm(HoldConflictApproved, in)
       \/ HoldArm(HoldConflictRejected, in)
       \/ HoldArm(HoldConflictExpired, in)
       \/ HoldArm(HoldConflictWithdrawn, in)
       \/ HoldArm(HoldConflictExecuted, in)
       \/ HoldArm(HoldConflictExecutionRefused, in)
    /\ AuditStep("Hold", "HoldConflict", in)

AuditPresentAtDeadline(in) ==
    /\ PresentArm(PresentAtDeadline, in)
    /\ AuditStep("Present", "PresentAtDeadline", in)

AuditPresentRefusedSubjectChanged(in) ==
    /\ PresentArm(PresentRefusedSubjectChanged, in)
    /\ AuditStep("Present", "PresentRefusedSubjectChanged", in)

AuditPresentRefusedIncomplete(in) ==
    /\ PresentArm(PresentRefusedIncomplete, in)
    /\ AuditStep("Present", "PresentRefusedIncomplete", in)

AuditPresentToApprover(in) ==
    /\ PresentArm(PresentToApprover, in)
    /\ AuditStep("Present", "PresentToApprover", in)

AuditPresentRefusedUntimed(in) ==
    /\ PresentArm(PresentRefusedUntimed, in)
    /\ AuditStep("Present", "PresentRefusedUntimed", in)

AuditPresentRefusedWrongApproverAudience(in) ==
    /\ PresentArm(PresentRefusedWrongApproverAudience, in)
    /\ AuditStep("Present", "PresentRefusedWrongApproverAudience", in)

AuditPresentToReviewer(in) ==
    /\ PresentArm(PresentToReviewer, in)
    /\ AuditStep("Present", "PresentToReviewer", in)

AuditPresentRefusedWrongReviewerAudience(in) ==
    /\ PresentArm(PresentRefusedWrongReviewerAudience, in)
    /\ AuditStep("Present", "PresentRefusedWrongReviewerAudience", in)

AuditPresentRefusedNotOpen(in) ==
    /\ \/ PresentArm(PresentRefusedNotOpenApproved, in)
       \/ PresentArm(PresentRefusedNotOpenRejected, in)
       \/ PresentArm(PresentRefusedNotOpenExpired, in)
       \/ PresentArm(PresentRefusedNotOpenWithdrawn, in)
       \/ PresentArm(PresentRefusedNotOpenExecuted, in)
       \/ PresentArm(PresentRefusedNotOpenExecutionRefused, in)
    /\ AuditStep("Present", "PresentRefusedNotOpen", in)

AuditDecideAtDeadline(in) ==
    /\ DecideArm(DecideAtDeadline, in)
    /\ AuditStep("Decide", "DecideAtDeadline", in)

AuditDecideRefusedReviewStage(in) ==
    /\ DecideArm(DecideRefusedReviewStage, in)
    /\ AuditStep("Decide", "DecideRefusedReviewStage", in)

AuditDecideRefusedEvidenceRequired(in) ==
    /\ DecideArm(DecideRefusedEvidenceRequired, in)
    /\ AuditStep("Decide", "DecideRefusedEvidenceRequired", in)

AuditDecideRefusedSubjectChanged(in) ==
    /\ DecideArm(DecideRefusedSubjectChanged, in)
    /\ AuditStep("Decide", "DecideRefusedSubjectChanged", in)

AuditDecideRefusedNotEligible(in) ==
    /\ DecideArm(DecideRefusedNotEligible, in)
    /\ AuditStep("Decide", "DecideRefusedNotEligible", in)

AuditDecideRefusedSelfApproval(in) ==
    /\ DecideArm(DecideRefusedSelfApproval, in)
    /\ AuditStep("Decide", "DecideRefusedSelfApproval", in)

AuditDecideRefusedPresentationIncomplete(in) ==
    /\ DecideArm(DecideRefusedPresentationIncomplete, in)
    /\ AuditStep("Decide", "DecideRefusedPresentationIncomplete", in)

AuditDecideRefusedDecidedBeforePresentation(in) ==
    /\ DecideArm(DecideRefusedDecidedBeforePresentation, in)
    /\ AuditStep("Decide", "DecideRefusedDecidedBeforePresentation", in)

AuditDecideApproved(in) ==
    /\ DecideArm(DecideApproved, in)
    /\ AuditStep("Decide", "DecideApproved", in)

AuditDecideRejected(in) ==
    /\ DecideArm(DecideRejected, in)
    /\ AuditStep("Decide", "DecideRejected", in)

AuditDecideRefusedNotOpen(in) ==
    /\ \/ DecideArm(DecideRefusedNotOpenApproved, in)
       \/ DecideArm(DecideRefusedNotOpenRejected, in)
       \/ DecideArm(DecideRefusedNotOpenExpired, in)
       \/ DecideArm(DecideRefusedNotOpenWithdrawn, in)
       \/ DecideArm(DecideRefusedNotOpenExecuted, in)
       \/ DecideArm(DecideRefusedNotOpenExecutionRefused, in)
    /\ AuditStep("Decide", "DecideRefusedNotOpen", in)

AuditReviewAtDeadline(in) ==
    /\ ReviewArm(ReviewAtDeadline, in)
    /\ AuditStep("Review", "ReviewAtDeadline", in)

AuditReviewRefusedHumanStage(in) ==
    /\ ReviewArm(ReviewRefusedHumanStage, in)
    /\ AuditStep("Review", "ReviewRefusedHumanStage", in)

AuditReviewRefusedWrongReviewer(in) ==
    /\ ReviewArm(ReviewRefusedWrongReviewer, in)
    /\ AuditStep("Review", "ReviewRefusedWrongReviewer", in)

AuditReviewRefusedEvidenceRequired(in) ==
    /\ ReviewArm(ReviewRefusedEvidenceRequired, in)
    /\ AuditStep("Review", "ReviewRefusedEvidenceRequired", in)

AuditReviewRefusedSubjectChanged(in) ==
    /\ ReviewArm(ReviewRefusedSubjectChanged, in)
    /\ AuditStep("Review", "ReviewRefusedSubjectChanged", in)

AuditReviewRefusedPresentationIncomplete(in) ==
    /\ ReviewArm(ReviewRefusedPresentationIncomplete, in)
    /\ AuditStep("Review", "ReviewRefusedPresentationIncomplete", in)

AuditReviewAllowed(in) ==
    /\ ReviewArm(ReviewAllowed, in)
    /\ AuditStep("Review", "ReviewAllowed", in)

AuditReviewDenied(in) ==
    /\ ReviewArm(ReviewDenied, in)
    /\ AuditStep("Review", "ReviewDenied", in)

AuditReviewEscalated(in) ==
    /\ ReviewArm(ReviewEscalated, in)
    /\ AuditStep("Review", "ReviewEscalated", in)

AuditReviewRefusedNotOpen(in) ==
    /\ \/ ReviewArm(ReviewRefusedNotOpenApproved, in)
       \/ ReviewArm(ReviewRefusedNotOpenRejected, in)
       \/ ReviewArm(ReviewRefusedNotOpenExpired, in)
       \/ ReviewArm(ReviewRefusedNotOpenWithdrawn, in)
       \/ ReviewArm(ReviewRefusedNotOpenExecuted, in)
       \/ ReviewArm(ReviewRefusedNotOpenExecutionRefused, in)
    /\ AuditStep("Review", "ReviewRefusedNotOpen", in)

AuditWithdrawAtDeadline(in) ==
    /\ WithdrawArm(WithdrawAtDeadline, in)
    /\ AuditStep("Withdraw", "WithdrawAtDeadline", in)

AuditWithdrawByHost(in) ==
    /\ WithdrawArm(WithdrawByHost, in)
    /\ AuditStep("Withdraw", "WithdrawByHost", in)

AuditWithdrawByRequester(in) ==
    /\ WithdrawArm(WithdrawByRequester, in)
    /\ AuditStep("Withdraw", "WithdrawByRequester", in)

AuditWithdrawRefusedNotRequester(in) ==
    /\ WithdrawArm(WithdrawRefusedNotRequester, in)
    /\ AuditStep("Withdraw", "WithdrawRefusedNotRequester", in)

AuditWithdrawRefusedNotOpen(in) ==
    /\ \/ WithdrawArm(WithdrawRefusedNotOpenApproved, in)
       \/ WithdrawArm(WithdrawRefusedNotOpenRejected, in)
       \/ WithdrawArm(WithdrawRefusedNotOpenExpired, in)
       \/ WithdrawArm(WithdrawRefusedNotOpenWithdrawn, in)
       \/ WithdrawArm(WithdrawRefusedNotOpenExecuted, in)
       \/ WithdrawArm(WithdrawRefusedNotOpenExecutionRefused, in)
    /\ AuditStep("Withdraw", "WithdrawRefusedNotOpen", in)

AuditExpireOpenDue(in) ==
    /\ ExpireArm(ExpireOpenDue, in)
    /\ AuditStep("Expire", "ExpireOpenDue", in)

AuditExpireOpenNotDue(in) ==
    /\ ExpireArm(ExpireOpenNotDue, in)
    /\ AuditStep("Expire", "ExpireOpenNotDue", in)

AuditExpireApprovedDue(in) ==
    /\ ExpireArm(ExpireApprovedDue, in)
    /\ AuditStep("Expire", "ExpireApprovedDue", in)

AuditExpireApprovedSettlementOwed(in) ==
    /\ ExpireArm(ExpireApprovedSettlementOwed, in)
    /\ AuditStep("Expire", "ExpireApprovedSettlementOwed", in)

AuditExpireApprovedNotDue(in) ==
    /\ ExpireArm(ExpireApprovedNotDue, in)
    /\ AuditStep("Expire", "ExpireApprovedNotDue", in)

AuditExpireSettled(in) ==
    /\ \/ ExpireArm(ExpireSettledRejected, in)
       \/ ExpireArm(ExpireSettledExpired, in)
       \/ ExpireArm(ExpireSettledWithdrawn, in)
       \/ ExpireArm(ExpireSettledExecuted, in)
       \/ ExpireArm(ExpireSettledExecutionRefused, in)
    /\ AuditStep("Expire", "ExpireSettled", in)

AuditAuthorizeExecution(in) ==
    /\ ~audit_committed
    /\ AuthorizeExecutionArm(AuthorizeExecution, in)
    /\ AuditStep("AuthorizeExecution", "AuthorizeExecution", in)

AuditAuthorizeExecutionAtDeadline(in) ==
    /\ ~audit_committed
    /\ AuthorizeExecutionArm(AuthorizeExecutionAtDeadline, in)
    /\ AuditStep("AuthorizeExecution", "AuthorizeExecutionAtDeadline", in)

AuditAuthorizeExecutionRefusedSettlementOwed(in) ==
    /\ ~audit_committed
    /\ AuthorizeExecutionArm(AuthorizeExecutionRefusedSettlementOwed, in)
    /\ AuditStep("AuthorizeExecution", "AuthorizeExecutionRefusedSettlementOwed", in)

AuditAuthorizeExecutionRefusedBinding(in) ==
    /\ ~audit_committed
    /\ AuthorizeExecutionArm(AuthorizeExecutionRefusedBinding, in)
    /\ AuditStep("AuthorizeExecution", "AuthorizeExecutionRefusedBinding", in)

AuditAuthorizeExecutionRefusedNotApproved(in) ==
    /\ ~audit_committed
    /\ \/ AuthorizeExecutionArm(AuthorizeExecutionRefusedNotApprovedOpen, in)
       \/ AuthorizeExecutionArm(AuthorizeExecutionRefusedNotApprovedRejected, in)
       \/ AuthorizeExecutionArm(AuthorizeExecutionRefusedNotApprovedExpired, in)
       \/ AuthorizeExecutionArm(AuthorizeExecutionRefusedNotApprovedWithdrawn, in)
       \/ AuthorizeExecutionArm(AuthorizeExecutionRefusedNotApprovedExecuted, in)
       \/ AuthorizeExecutionArm(AuthorizeExecutionRefusedNotApprovedExecutionRefused, in)
    /\ AuditStep("AuthorizeExecution", "AuthorizeExecutionRefusedNotApproved", in)

AuditReleaseExecution(in) ==
    /\ ~audit_committed
    /\ ReleaseExecutionArm(ReleaseExecution, in)
    /\ AuditStep("ReleaseExecution", "ReleaseExecution", in)

AuditReleaseExecutionAtDeadline(in) ==
    /\ ~audit_committed
    /\ ReleaseExecutionArm(ReleaseExecutionAtDeadline, in)
    /\ AuditStep("ReleaseExecution", "ReleaseExecutionAtDeadline", in)

AuditReleaseExecutionRefusedNotAuthorized(in) ==
    /\ ~audit_committed
    /\ ReleaseExecutionArm(ReleaseExecutionRefusedNotAuthorized, in)
    /\ AuditStep("ReleaseExecution", "ReleaseExecutionRefusedNotAuthorized", in)

AuditReleaseExecutionRefusedNotApproved(in) ==
    /\ ~audit_committed
    /\ \/ ReleaseExecutionArm(ReleaseExecutionRefusedNotApprovedOpen, in)
       \/ ReleaseExecutionArm(ReleaseExecutionRefusedNotApprovedRejected, in)
       \/ ReleaseExecutionArm(ReleaseExecutionRefusedNotApprovedExpired, in)
       \/ ReleaseExecutionArm(ReleaseExecutionRefusedNotApprovedWithdrawn, in)
       \/ ReleaseExecutionArm(ReleaseExecutionRefusedNotApprovedExecuted, in)
       \/ ReleaseExecutionArm(ReleaseExecutionRefusedNotApprovedExecutionRefused, in)
    /\ AuditStep("ReleaseExecution", "ReleaseExecutionRefusedNotApproved", in)

AuditRecordExecuted(in) ==
    /\ RecordExecutedArm(RecordExecuted, in)
    /\ AuditStep("RecordExecuted", "RecordExecuted", in)

AuditRecordExecutedRefusedNotAuthorized(in) ==
    /\ RecordExecutedArm(RecordExecutedRefusedNotAuthorized, in)
    /\ AuditStep("RecordExecuted", "RecordExecutedRefusedNotAuthorized", in)

AuditRecordExecutedRefusedBinding(in) ==
    /\ RecordExecutedArm(RecordExecutedRefusedBinding, in)
    /\ AuditStep("RecordExecuted", "RecordExecutedRefusedBinding", in)

AuditRecordExecutedReplayed(in) ==
    /\ RecordExecutedArm(RecordExecutedReplayed, in)
    /\ AuditStep("RecordExecuted", "RecordExecutedReplayed", in)

AuditRecordExecutedReplayRefusedBinding(in) ==
    /\ RecordExecutedArm(RecordExecutedReplayRefusedBinding, in)
    /\ AuditStep("RecordExecuted", "RecordExecutedReplayRefusedBinding", in)

AuditRecordExecutedRefusedNotApproved(in) ==
    /\ \/ RecordExecutedArm(RecordExecutedRefusedNotApprovedOpen, in)
       \/ RecordExecutedArm(RecordExecutedRefusedNotApprovedRejected, in)
       \/ RecordExecutedArm(RecordExecutedRefusedNotApprovedExpired, in)
       \/ RecordExecutedArm(RecordExecutedRefusedNotApprovedWithdrawn, in)
       \/ RecordExecutedArm(RecordExecutedRefusedNotApprovedExecutionRefused, in)
    /\ AuditStep("RecordExecuted", "RecordExecutedRefusedNotApproved", in)

AuditCloseRefused(in) ==
    /\ ~audit_committed
    /\ CloseRefusedArm(CloseRefused, in)
    /\ AuditStep("CloseRefused", "CloseRefused", in)

AuditCloseRefusedWithPermit(in) ==
    /\ ~audit_committed
    /\ CloseRefusedArm(CloseRefusedWithPermit, in)
    /\ AuditStep("CloseRefused", "CloseRefusedWithPermit", in)

AuditCloseRefusedAtDeadline(in) ==
    /\ ~audit_committed
    /\ CloseRefusedArm(CloseRefusedAtDeadline, in)
    /\ AuditStep("CloseRefused", "CloseRefusedAtDeadline", in)

AuditCloseRefusedNotApproved(in) ==
    /\ ~audit_committed
    /\ \/ CloseRefusedArm(CloseRefusedNotApprovedOpen, in)
       \/ CloseRefusedArm(CloseRefusedNotApprovedRejected, in)
       \/ CloseRefusedArm(CloseRefusedNotApprovedExpired, in)
       \/ CloseRefusedArm(CloseRefusedNotApprovedWithdrawn, in)
       \/ CloseRefusedArm(CloseRefusedNotApprovedExecuted, in)
       \/ CloseRefusedArm(CloseRefusedNotApprovedExecutionRefused, in)
    /\ AuditStep("CloseRefused", "CloseRefusedNotApproved", in)

AuditSettleNoticeDelivered(in) ==
    /\ \/ SettleNoticeArm(SettleNoticeDeliveredRejected, in)
       \/ SettleNoticeArm(SettleNoticeDeliveredExpired, in)
       \/ SettleNoticeArm(SettleNoticeDeliveredWithdrawn, in)
       \/ SettleNoticeArm(SettleNoticeDeliveredExecuted, in)
       \/ SettleNoticeArm(SettleNoticeDeliveredExecutionRefused, in)
    /\ AuditStep("SettleNotice", "SettleNoticeDelivered", in)

AuditSettleNoticeFailed(in) ==
    /\ \/ SettleNoticeArm(SettleNoticeFailedRejected, in)
       \/ SettleNoticeArm(SettleNoticeFailedExpired, in)
       \/ SettleNoticeArm(SettleNoticeFailedWithdrawn, in)
       \/ SettleNoticeArm(SettleNoticeFailedExecuted, in)
       \/ SettleNoticeArm(SettleNoticeFailedExecutionRefused, in)
    /\ AuditStep("SettleNotice", "SettleNoticeFailed", in)

AuditSettleNoticeReplayed(in) ==
    /\ \/ SettleNoticeArm(SettleNoticeReplayedRejected, in)
       \/ SettleNoticeArm(SettleNoticeReplayedExpired, in)
       \/ SettleNoticeArm(SettleNoticeReplayedWithdrawn, in)
       \/ SettleNoticeArm(SettleNoticeReplayedExecuted, in)
       \/ SettleNoticeArm(SettleNoticeReplayedExecutionRefused, in)
    /\ AuditStep("SettleNotice", "SettleNoticeReplayed", in)

AuditSettleNoticeRefused(in) ==
    /\ \/ SettleNoticeArm(SettleNoticeRefusedRejected, in)
       \/ SettleNoticeArm(SettleNoticeRefusedExpired, in)
       \/ SettleNoticeArm(SettleNoticeRefusedWithdrawn, in)
       \/ SettleNoticeArm(SettleNoticeRefusedExecuted, in)
       \/ SettleNoticeArm(SettleNoticeRefusedExecutionRefused, in)
    /\ AuditStep("SettleNotice", "SettleNoticeRefused", in)

AuditSettleNoticeRefusedNotEnded(in) ==
    /\ \/ SettleNoticeArm(SettleNoticeRefusedNotEndedOpen, in)
       \/ SettleNoticeArm(SettleNoticeRefusedNotEndedApproved, in)
    /\ AuditStep("SettleNotice", "SettleNoticeRefusedNotEnded", in)

AuditNext ==
    \/ \E in \in AuditHoldInputs : AuditHoldHuman(in)
    \/ \E in \in AuditHoldInputs : AuditHoldReview(in)
    \/ \E in \in AuditHoldInputs : AuditHoldRefusedWindow(in)
    \/ \E in \in AuditHoldInputs : AuditHoldRefusedSelfReview(in)
    \/ \E in \in AuditHoldInputs : AuditHoldReplayed(in)
    \/ \E in \in AuditHoldInputs : AuditHoldConflict(in)
    \/ \E in \in AuditPresentInputs : AuditPresentAtDeadline(in)
    \/ \E in \in AuditPresentInputs : AuditPresentRefusedSubjectChanged(in)
    \/ \E in \in AuditPresentInputs : AuditPresentRefusedIncomplete(in)
    \/ \E in \in AuditPresentInputs : AuditPresentToApprover(in)
    \/ \E in \in AuditPresentInputs : AuditPresentRefusedUntimed(in)
    \/ \E in \in AuditPresentInputs : AuditPresentRefusedWrongApproverAudience(in)
    \/ \E in \in AuditPresentInputs : AuditPresentToReviewer(in)
    \/ \E in \in AuditPresentInputs : AuditPresentRefusedWrongReviewerAudience(in)
    \/ \E in \in AuditPresentInputs : AuditPresentRefusedNotOpen(in)
    \/ \E in \in AuditDecideInputs : AuditDecideAtDeadline(in)
    \/ \E in \in AuditDecideInputs : AuditDecideRefusedReviewStage(in)
    \/ \E in \in AuditDecideInputs : AuditDecideRefusedEvidenceRequired(in)
    \/ \E in \in AuditDecideInputs : AuditDecideRefusedSubjectChanged(in)
    \/ \E in \in AuditDecideInputs : AuditDecideRefusedNotEligible(in)
    \/ \E in \in AuditDecideInputs : AuditDecideRefusedSelfApproval(in)
    \/ \E in \in AuditDecideInputs : AuditDecideRefusedPresentationIncomplete(in)
    \/ \E in \in AuditDecideInputs : AuditDecideRefusedDecidedBeforePresentation(in)
    \/ \E in \in AuditDecideInputs : AuditDecideApproved(in)
    \/ \E in \in AuditDecideInputs : AuditDecideRejected(in)
    \/ \E in \in AuditDecideInputs : AuditDecideRefusedNotOpen(in)
    \/ \E in \in AuditReviewInputs : AuditReviewAtDeadline(in)
    \/ \E in \in AuditReviewInputs : AuditReviewRefusedHumanStage(in)
    \/ \E in \in AuditReviewInputs : AuditReviewRefusedWrongReviewer(in)
    \/ \E in \in AuditReviewInputs : AuditReviewRefusedEvidenceRequired(in)
    \/ \E in \in AuditReviewInputs : AuditReviewRefusedSubjectChanged(in)
    \/ \E in \in AuditReviewInputs : AuditReviewRefusedPresentationIncomplete(in)
    \/ \E in \in AuditReviewInputs : AuditReviewAllowed(in)
    \/ \E in \in AuditReviewInputs : AuditReviewDenied(in)
    \/ \E in \in AuditReviewInputs : AuditReviewEscalated(in)
    \/ \E in \in AuditReviewInputs : AuditReviewRefusedNotOpen(in)
    \/ \E in \in AuditWithdrawInputs : AuditWithdrawAtDeadline(in)
    \/ \E in \in AuditWithdrawInputs : AuditWithdrawByHost(in)
    \/ \E in \in AuditWithdrawInputs : AuditWithdrawByRequester(in)
    \/ \E in \in AuditWithdrawInputs : AuditWithdrawRefusedNotRequester(in)
    \/ \E in \in AuditWithdrawInputs : AuditWithdrawRefusedNotOpen(in)
    \/ \E in \in AuditClockInputs : AuditExpireOpenDue(in)
    \/ \E in \in AuditClockInputs : AuditExpireOpenNotDue(in)
    \/ \E in \in AuditClockInputs : AuditExpireApprovedDue(in)
    \/ \E in \in AuditClockInputs : AuditExpireApprovedSettlementOwed(in)
    \/ \E in \in AuditClockInputs : AuditExpireApprovedNotDue(in)
    \/ \E in \in AuditClockInputs : AuditExpireSettled(in)
    \/ \E in \in AuditExecutionInputs : AuditAuthorizeExecution(in)
    \/ \E in \in AuditExecutionInputs : AuditAuthorizeExecutionAtDeadline(in)
    \/ \E in \in AuditExecutionInputs : AuditAuthorizeExecutionRefusedSettlementOwed(in)
    \/ \E in \in AuditExecutionInputs : AuditAuthorizeExecutionRefusedBinding(in)
    \/ \E in \in AuditExecutionInputs : AuditAuthorizeExecutionRefusedNotApproved(in)
    \/ \E in \in AuditClockInputs : AuditReleaseExecution(in)
    \/ \E in \in AuditClockInputs : AuditReleaseExecutionAtDeadline(in)
    \/ \E in \in AuditClockInputs : AuditReleaseExecutionRefusedNotAuthorized(in)
    \/ \E in \in AuditClockInputs : AuditReleaseExecutionRefusedNotApproved(in)
    \/ \E in \in AuditExecutionInputs : AuditRecordExecuted(in)
    \/ \E in \in AuditExecutionInputs : AuditRecordExecutedRefusedNotAuthorized(in)
    \/ \E in \in AuditExecutionInputs : AuditRecordExecutedRefusedBinding(in)
    \/ \E in \in AuditExecutionInputs : AuditRecordExecutedReplayed(in)
    \/ \E in \in AuditExecutionInputs : AuditRecordExecutedReplayRefusedBinding(in)
    \/ \E in \in AuditExecutionInputs : AuditRecordExecutedRefusedNotApproved(in)
    \/ \E in \in AuditClockInputs : AuditCloseRefused(in)
    \/ \E in \in AuditClockInputs : AuditCloseRefusedWithPermit(in)
    \/ \E in \in AuditClockInputs : AuditCloseRefusedAtDeadline(in)
    \/ \E in \in AuditClockInputs : AuditCloseRefusedNotApproved(in)
    \/ \E in \in AuditNoticeInputs : AuditSettleNoticeDelivered(in)
    \/ \E in \in AuditNoticeInputs : AuditSettleNoticeFailed(in)
    \/ \E in \in AuditNoticeInputs : AuditSettleNoticeReplayed(in)
    \/ \E in \in AuditNoticeInputs : AuditSettleNoticeRefused(in)
    \/ \E in \in AuditNoticeInputs : AuditSettleNoticeRefusedNotEnded(in)
    \/ AuditCommit

AuditSpec == AuditInit /\ [][AuditNext]_<<vars, auditVars>>

AuditStateConstraint == model_step_count <= AuditMaxSteps

\* --- properties ---------------------------------------------------------

AuditTerminalsLatch == "TerminalsLatch" \notin audit_broken
AuditApprovalIsAttested == "ApprovalIsAttested" \notin audit_broken
AuditRejectionIsAttested == "RejectionIsAttested" \notin audit_broken
AuditExecutedOnlyFromApprovedWithHeldBinding ==
    "ExecutedOnlyFromApprovedWithHeldBinding" \notin audit_broken
AuditExpiryOnlyAtTheDeadline == "ExpiryOnlyAtTheDeadline" \notin audit_broken
AuditApprovedExpiresOnlyWithoutPermit == "ApprovedExpiresOnlyWithoutPermit" \notin audit_broken
AuditWithdrawOnlyOpenByRequesterOrHost == "WithdrawOnlyOpenByRequesterOrHost" \notin audit_broken
AuditSealedFieldsNeverChange == "SealedFieldsNeverChange" \notin audit_broken
AuditNoticeOwedOnceAtTheEnd == "NoticeOwedOnceAtTheEnd" \notin audit_broken
AuditNoPermitAtOrAfterDeadline == "NoPermitAtOrAfterDeadline" \notin audit_broken
AuditReleaseClearsPermit == "ReleaseClearsPermit" \notin audit_broken
AuditDecisionFollowsPresentation == "DecisionFollowsPresentation" \notin audit_broken

AuditCommittedIsSettleable ==
    audit_committed => (phase = "Executed" \/ (phase = "Approved" /\ permit_issued))

AuditFirstPresentationIsTheEarliest ==
    first_presented_to_approver_at_ms = audit_first_presented

\* --- reachability goals, each checked as an expected violation of its
\* negation ---------------------------------------------------------------

\* A human approval, authorized, committed and recorded, and its notice
\* delivered.
AuditWitnessHumanApprovalExecutedAndNoticed ==
    audit_committed /\ phase = "Executed" /\ notice = "Delivered" /\ reviewer = None
\* A permit released (no commit), authorized again, then committed and
\* recorded.
AuditWitnessRetriedAfterRelease ==
    audit_committed /\ phase = "Executed" /\ "Released" \in audit_seen
\* An approval the owner refused permanently.
AuditWitnessExecutionRefused == phase = "ExecutionRefused"
\* A review escalated to the human, who approved.
AuditWitnessEscalatedThenHumanApproved ==
    phase = "Approved" /\ reviewer # None /\ stage = "Human"
\* A stale digest refused, then an approval.
AuditWitnessStaleDigestThenApproved == phase = "Approved" /\ "StaleRefused" \in audit_seen
\* A decision at the deadline records the expiry.
AuditWitnessDecideAtDeadlineExpires == phase = "Expired" /\ "DecideExpired" \in audit_seen
\* A decision refused after the withdrawal.
AuditWitnessWithdrawnThenDecideRefused == "DecideRefusedWithdrawn" \in audit_seen
\* The requester's own decision refused, then the approver's.
AuditWitnessSelfApprovalThenAnotherApproves ==
    phase = "Approved" /\ "SelfApprovalRefused" \in audit_seen
\* After the commit and its record, a replayed record changes nothing.
AuditWitnessCommitThenRecordReplayed == "RecordReplayed" \in audit_seen
\* A replayed hold and a conflicting one, both refused to change anything.
AuditWitnessHoldReplayedAndConflicting == {"HoldReplayed", "HoldConflict"} \subseteq audit_seen
\* A deadline observation held off by the outstanding permit, then the
\* commit settled as executed.
AuditWitnessPermitAcrossDeadlineExecuted ==
    audit_committed /\ phase = "Executed" /\ "ExpiryHeldOff" \in audit_seen
\* A release at or after the deadline records the expiry.
AuditWitnessReleaseAfterDeadlineExpires == phase = "Expired" /\ "LateRelease" \in audit_seen
\* An authorization at or after the deadline refused.
AuditWitnessAuthorizeAtDeadlineRefused == "LateAuthorizeRefused" \in audit_seen
\* A decision observed before the first presentation refused, then an
\* approval.
AuditWitnessDecidedBeforePresentationRefused ==
    phase = "Approved" /\ "DecidedBeforePresentationRefused" \in audit_seen
\* An approval observed in the same millisecond as the first presentation.
AuditWitnessApprovedAtThePresentation ==
    phase = "Approved" /\ "ApprovedAtThePresentation" \in audit_seen

NotAuditWitnessHumanApprovalExecutedAndNoticed == ~AuditWitnessHumanApprovalExecutedAndNoticed
NotAuditWitnessRetriedAfterRelease == ~AuditWitnessRetriedAfterRelease
NotAuditWitnessExecutionRefused == ~AuditWitnessExecutionRefused
NotAuditWitnessEscalatedThenHumanApproved == ~AuditWitnessEscalatedThenHumanApproved
NotAuditWitnessStaleDigestThenApproved == ~AuditWitnessStaleDigestThenApproved
NotAuditWitnessDecideAtDeadlineExpires == ~AuditWitnessDecideAtDeadlineExpires
NotAuditWitnessWithdrawnThenDecideRefused == ~AuditWitnessWithdrawnThenDecideRefused
NotAuditWitnessSelfApprovalThenAnotherApproves == ~AuditWitnessSelfApprovalThenAnotherApproves
NotAuditWitnessCommitThenRecordReplayed == ~AuditWitnessCommitThenRecordReplayed
NotAuditWitnessHoldReplayedAndConflicting == ~AuditWitnessHoldReplayedAndConflicting
NotAuditWitnessPermitAcrossDeadlineExecuted == ~AuditWitnessPermitAcrossDeadlineExecuted
NotAuditWitnessReleaseAfterDeadlineExpires == ~AuditWitnessReleaseAfterDeadlineExpires
NotAuditWitnessAuthorizeAtDeadlineRefused == ~AuditWitnessAuthorizeAtDeadlineRefused
NotAuditWitnessDecidedBeforePresentationRefused ==
    ~AuditWitnessDecidedBeforePresentationRefused
NotAuditWitnessApprovedAtThePresentation == ~AuditWitnessApprovedAtThePresentation
====
