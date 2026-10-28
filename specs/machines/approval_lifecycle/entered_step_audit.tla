---- MODULE entered_step_audit ----
\* Hand-written bounded audit of the entered state of a reviewed effect in
\* the generated ApprovalLifecycleMachine model.
\*
\* A governed tool dispatch whose operation needs a model review (R2) spends
\* its retained allow once at its effect's entry (ConsumeReviewForEntry,
\* Allowed -> Used); a fresh attempt of a consented candidate spends its
\* authenticated human consent there (ConsumeConsentForEntry, which also
\* unlinks the attempt). Either spend opens the attempt's entered state: its
\* Used status with an entered phase (Open) and no admitted step in flight.
\* A later step of the entered effect (a governed helper graph admission, a
\* helper child grant issuance) begins only with AdmitEnteredStep against
\* that attempt while its effect is Open, and ends with FinishEnteredStep;
\* neither spends. The admitting dispatch's end hands the attempt back
\* (ReleaseReview): with no admitted step in flight it is released
\* (ReleaseReviewUsed); otherwise the effect closes (ReleaseReviewUsedClosing),
\* no new step is admitted (AdmitEnteredStepRejectedClosing), and the last
\* admitted step's finish releases it (FinishEnteredStepReleases). So the
\* attempt stays Used until the later of the dispatch end and the end of the
\* last step admitted before it.
\*
\* This audit keeps the generated model unchanged and drives two dispatches,
\* each with its own attempt: a model-reviewed one ("alpha") and a consented
\* one ("beta", under the consent approval "alpha"; approval ids are their
\* own namespace). Attempt ids are never reused (they are fresh UUIDs), so
\* each attempt is issued once, and its dispatch ends once; nothing spends
\* after its dispatch ended (core closes every clone of the entry there).
\* Explored: the review's verdicts, its unavailability and its retirement;
\* the consent's decision and withdrawal; each spend and every refused second
\* spend; step admissions and finishes requested at any time, so every
\* AdmitEnteredStep and FinishEnteredStep arm is decided by the owner alone;
\* the dispatch end with zero, one or more steps in flight; a repeated
\* hand-back; a retirement requested for an entered attempt; and a consent
\* withdrawal after the spend. Not driven, and untouched by the entered
\* state: ReleaseReviewRejectedConsentLinked and ReleaseReviewRejectedPending
\* (core hands back neither a linked consent attempt, which stays with its
\* owner, nor a pending one at a dispatch end). Core settles the hand-written queue of non-waiting inputs into
\* the owner before deciding an admission, so the owner's input order is the
\* order explored here. History variables record each attempt's successful
\* spends, its dispatch end, the steps admitted and finished for it, and its
\* release. Properties:
\*
\*   AuditOneSpendPerAttempt: no attempt is spent twice.
\*   AuditStepOnlyAfterSpend: no step is admitted for an attempt that was
\*     never spent (Pending, Allowed, Denied, Escalated, Unavailable,
\*     Retired, consent-linked, unknown).
\*   AuditNoStepAfterDispatchEnd: no step is admitted after the admitting
\*     dispatch ended, whether the effect then closed or was released.
\*   AuditFinishesNeverExceedAdmissions: the owner never finishes more steps
\*     than it admitted.
\*   AuditReleaseWaitsForSteps: while any admitted step has not finished,
\*     its attempt stays registered as Used.
\*   AuditEnteredStateMatchesHistory: the attempt is registered as Used
\*     exactly while it was spent once and its dispatch is live or a step is
\*     in flight; it is released exactly at the dispatch end with no step in
\*     flight, or at the last finish after the end; and while entered, the
\*     owner's step count is the steps in flight and its phase is Closing
\*     exactly after the dispatch end.
\*   AuditStepsNeverSpendOrSettle (action property): admitting a step, or
\*     finishing one without releasing the attempt, changes no attempt,
\*     consent or approval state.
\*
\* Reachability (the runner): every new arm and every arm the entered state
\* changed fires in the bounded space, checked from TLC's coverage of the
\* audit's per-arm actions, and three scenarios are checked as expected
\* violations of their negations: the dispatch cancelled while an admitted
\* step is in flight keeps the attempt Used, refuses a new step and releases
\* it at that step's finish (AuditWitnessCancelWhileStepInFlight); a step
\* refused after the attempt's release (AuditWitnessRefusedAfterRelease);
\* and the helper graph admission plus two child steps under one consent
\* spend (AuditWitnessGraphAndTwoChildrenAfterConsent). With --mutants the
\* runner seeds one defect per invariant and property into a copy of the
\* generated model and requires TLC to refuse each with that property.
\*
\* The TLC config is DERIVED from the generated ci.cfg on every run by
\* entered_step_audit.sh:
\*   specs/machines/approval_lifecycle/entered_step_audit.sh 16 --mutants
EXTENDS model

CONSTANT AuditMaxSteps

AuditReview == "alpha"
AuditConsent == "beta"
AuditApproval == "alpha"
AuditIds == {AuditReview, AuditConsent}
\* One retirement reason stands for all: no guard here reads it.
AuditReason == "Abandoned"

VARIABLES
    audit_issued,                 \* the attempt was issued (never reused)
    audit_spends,                 \* successful spends of the attempt
    audit_ended,                  \* its admitting dispatch ended (one hand-back)
    audit_admitted,               \* steps admitted for it
    audit_finished,               \* admitted steps that finished
    audit_released,               \* the owner released it after its spend
    audit_step_unspent,           \* a step was admitted for an unspent attempt
    audit_step_after_end,         \* a step was admitted after its dispatch ended
    audit_end_in_flight,          \* its dispatch ended with a step in flight
    audit_refused_closing,        \* a step was refused while its effect closed
    audit_refused_after_release   \* a step was refused after its release

auditVars == << audit_issued, audit_spends, audit_ended, audit_admitted,
                audit_finished, audit_released, audit_step_unspent,
                audit_step_after_end, audit_end_in_flight,
                audit_refused_closing, audit_refused_after_release >>

AuditInit ==
    /\ Init
    /\ audit_issued = [id \in AuditIds |-> FALSE]
    /\ audit_spends = [id \in AuditIds |-> 0]
    /\ audit_ended = [id \in AuditIds |-> FALSE]
    /\ audit_admitted = [id \in AuditIds |-> 0]
    /\ audit_finished = [id \in AuditIds |-> 0]
    /\ audit_released = [id \in AuditIds |-> FALSE]
    /\ audit_step_unspent = FALSE
    /\ audit_step_after_end = FALSE
    /\ audit_end_in_flight = [id \in AuditIds |-> FALSE]
    /\ audit_refused_closing = [id \in AuditIds |-> FALSE]
    /\ audit_refused_after_release = [id \in AuditIds |-> FALSE]

InFlight(id) == audit_admitted[id] - audit_finished[id]

RegisteredUsed(id) == id \in review_ids /\ review_statuses[id] = "Used"

\* --- environment: issuance, settlement, spends ------------------------

IssueReview ==
    /\ ~audit_issued[AuditReview]
    /\ BeginReviewPending(AuditReview)
    /\ audit_issued' = [audit_issued EXCEPT ![AuditReview] = TRUE]
    /\ UNCHANGED << audit_spends, audit_ended, audit_admitted, audit_finished,
                    audit_released, audit_step_unspent, audit_step_after_end,
                    audit_end_in_flight, audit_refused_closing,
                    audit_refused_after_release >>

IssueConsent ==
    /\ ~audit_issued[AuditConsent]
    /\ CreateConsentPending(AuditApproval, AuditConsent, TRUE, TRUE, FALSE)
    /\ audit_issued' = [audit_issued EXCEPT ![AuditConsent] = TRUE]
    /\ UNCHANGED << audit_spends, audit_ended, audit_admitted, audit_finished,
                    audit_released, audit_step_unspent, audit_step_after_end,
                    audit_end_in_flight, audit_refused_closing,
                    audit_refused_after_release >>

ReviewSettles ==
    /\ \/ \E verdict \in ReviewVerdictValues :
              \/ RecordReviewVerdictAllowed(AuditReview, verdict)
              \/ RecordReviewVerdictDenied(AuditReview, verdict)
              \/ RecordReviewVerdictEscalated(AuditReview, verdict)
       \/ RecordReviewUnavailable(AuditReview)
       \/ RetireReview(AuditReview, AuditReason)
    /\ UNCHANGED auditVars

\* A retirement requested for a settled attempt, an entered one included, is
\* refused and changes nothing.
AuditRetireReviewRejectedSettled(id) ==
    /\ RetireReviewRejectedSettled(id, AuditReason)
    /\ UNCHANGED auditVars

ConsentSettles ==
    /\ \/ DecideConsentApprove(AuditApproval, AuditConsent, "Approve", TRUE)
       \/ DecideConsentDeny(AuditApproval, AuditConsent, "Deny", TRUE)
       \/ WithdrawConsentPending(AuditApproval, AuditConsent, AuditReason)
       \/ WithdrawConsentSettled(AuditApproval, AuditConsent, AuditReason)
    /\ UNCHANGED auditVars

\* A withdrawal after the consent spend, which unlinked the attempt, is
\* refused and changes nothing.
AuditWithdrawConsentRejectedMismatch ==
    /\ WithdrawConsentRejectedMismatch(AuditApproval, AuditConsent, AuditReason)
    /\ UNCHANGED auditVars

\* The one spend at the effect's entry, only while its dispatch is live.
AuditConsumeReviewForEntry ==
    /\ ~audit_ended[AuditReview]
    /\ ConsumeReviewForEntry(AuditReview)
    /\ audit_spends' = [audit_spends EXCEPT ![AuditReview] = @ + 1]
    /\ UNCHANGED << audit_issued, audit_ended, audit_admitted, audit_finished,
                    audit_released, audit_step_unspent, audit_step_after_end,
                    audit_end_in_flight, audit_refused_closing,
                    audit_refused_after_release >>

AuditConsumeConsentForEntry ==
    /\ ~audit_ended[AuditConsent]
    /\ ConsumeConsentForEntry(AuditApproval, AuditConsent)
    /\ audit_spends' = [audit_spends EXCEPT ![AuditConsent] = @ + 1]
    /\ UNCHANGED << audit_issued, audit_ended, audit_admitted, audit_finished,
                    audit_released, audit_step_unspent, audit_step_after_end,
                    audit_end_in_flight, audit_refused_closing,
                    audit_refused_after_release >>

\* A second spend while entered, refused by the owner.
AuditConsumeReviewRejectedUsed ==
    /\ ~audit_ended[AuditReview]
    /\ ConsumeReviewRejectedUsed(AuditReview)
    /\ UNCHANGED auditVars

AuditConsumeConsentRejectedAlreadyConsumed ==
    /\ ~audit_ended[AuditConsent]
    /\ ConsumeConsentRejectedAlreadyConsumed(AuditApproval, AuditConsent)
    /\ UNCHANGED auditVars

\* Every other refused spend while the dispatch is live.
SpendRefused ==
    /\ \/ /\ ~audit_ended[AuditReview]
          /\ \/ ConsumeReviewRejectedMissing(AuditReview)
             \/ ConsumeReviewRejectedConsentLinked(AuditReview)
             \/ ConsumeReviewRejectedRetired(AuditReview)
             \/ ConsumeReviewRejectedNotSatisfied(AuditReview)
       \/ /\ ~audit_ended[AuditConsent]
          /\ \/ ConsumeConsentRejectedMissing(AuditApproval, AuditConsent)
             \/ ConsumeConsentRejectedMismatch(AuditApproval, AuditConsent)
             \/ ConsumeConsentRejectedNotSatisfied(AuditApproval, AuditConsent)
    /\ UNCHANGED auditVars

\* --- later steps of the entered effect ---------------------------------

\* A later step admitted (requested at any time; the owner decides).
AuditAdmitEnteredStep(id) ==
    /\ AdmitEnteredStep(id)
    /\ audit_admitted' = [audit_admitted EXCEPT ![id] = @ + 1]
    /\ audit_step_unspent' = (audit_step_unspent \/ audit_spends[id] # 1)
    /\ audit_step_after_end' = (audit_step_after_end \/ audit_ended[id])
    /\ UNCHANGED << audit_issued, audit_spends, audit_ended, audit_finished,
                    audit_released, audit_end_in_flight, audit_refused_closing,
                    audit_refused_after_release >>

AuditAdmitEnteredStepRejectedMissing(id) ==
    /\ AdmitEnteredStepRejectedMissing(id)
    /\ audit_refused_after_release' =
           [audit_refused_after_release EXCEPT ![id] = @ \/ audit_released[id]]
    /\ UNCHANGED << audit_issued, audit_spends, audit_ended, audit_admitted,
                    audit_finished, audit_released, audit_step_unspent,
                    audit_step_after_end, audit_end_in_flight,
                    audit_refused_closing >>

AuditAdmitEnteredStepRejectedConsentLinked(id) ==
    /\ AdmitEnteredStepRejectedConsentLinked(id)
    /\ UNCHANGED auditVars

AuditAdmitEnteredStepRejectedRetired(id) ==
    /\ AdmitEnteredStepRejectedRetired(id)
    /\ UNCHANGED auditVars

AuditAdmitEnteredStepRejectedNotEntered(id) ==
    /\ AdmitEnteredStepRejectedNotEntered(id)
    /\ UNCHANGED auditVars

AuditAdmitEnteredStepRejectedClosing(id) ==
    /\ AdmitEnteredStepRejectedClosing(id)
    /\ audit_refused_closing' = [audit_refused_closing EXCEPT ![id] = TRUE]
    /\ UNCHANGED << audit_issued, audit_spends, audit_ended, audit_admitted,
                    audit_finished, audit_released, audit_step_unspent,
                    audit_step_after_end, audit_end_in_flight,
                    audit_refused_after_release >>

\* One step finished (requested at any time; the owner decides).
AuditFinishEnteredStepOpen(id) ==
    /\ FinishEnteredStepOpen(id)
    /\ audit_finished' = [audit_finished EXCEPT ![id] = @ + 1]
    /\ UNCHANGED << audit_issued, audit_spends, audit_ended, audit_admitted,
                    audit_released, audit_step_unspent, audit_step_after_end,
                    audit_end_in_flight, audit_refused_closing,
                    audit_refused_after_release >>

AuditFinishEnteredStepClosing(id) ==
    /\ FinishEnteredStepClosing(id)
    /\ audit_finished' = [audit_finished EXCEPT ![id] = @ + 1]
    /\ UNCHANGED << audit_issued, audit_spends, audit_ended, audit_admitted,
                    audit_released, audit_step_unspent, audit_step_after_end,
                    audit_end_in_flight, audit_refused_closing,
                    audit_refused_after_release >>

AuditFinishEnteredStepReleases(id) ==
    /\ FinishEnteredStepReleases(id)
    /\ audit_finished' = [audit_finished EXCEPT ![id] = @ + 1]
    /\ audit_released' = [audit_released EXCEPT ![id] = TRUE]
    /\ UNCHANGED << audit_issued, audit_spends, audit_ended, audit_admitted,
                    audit_step_unspent, audit_step_after_end,
                    audit_end_in_flight, audit_refused_closing,
                    audit_refused_after_release >>

AuditFinishEnteredStepRejectedMissing(id) ==
    /\ FinishEnteredStepRejectedMissing(id)
    /\ UNCHANGED auditVars

AuditFinishEnteredStepRejectedNotInFlight(id) ==
    /\ FinishEnteredStepRejectedNotInFlight(id)
    /\ UNCHANGED auditVars

\* --- the admitting dispatch's end: one hand-back per attempt -----------

AuditReleaseReviewUsed(id) ==
    /\ ~audit_ended[id]
    /\ ReleaseReviewUsed(id)
    /\ audit_ended' = [audit_ended EXCEPT ![id] = TRUE]
    /\ audit_released' = [audit_released EXCEPT ![id] = TRUE]
    /\ UNCHANGED << audit_issued, audit_spends, audit_admitted, audit_finished,
                    audit_step_unspent, audit_step_after_end,
                    audit_end_in_flight, audit_refused_closing,
                    audit_refused_after_release >>

AuditReleaseReviewUsedClosing(id) ==
    /\ ~audit_ended[id]
    /\ ReleaseReviewUsedClosing(id)
    /\ audit_ended' = [audit_ended EXCEPT ![id] = TRUE]
    /\ audit_end_in_flight' = [audit_end_in_flight EXCEPT ![id] = TRUE]
    /\ UNCHANGED << audit_issued, audit_spends, audit_admitted, audit_finished,
                    audit_released, audit_step_unspent, audit_step_after_end,
                    audit_refused_closing, audit_refused_after_release >>

\* The end of a dispatch whose attempt was never spent (core retires an
\* unspent allow first when the dispatch was cancelled).
DispatchEndsUnspent(id) ==
    /\ ~audit_ended[id]
    /\ \/ ReleaseReviewAllowed(id)
       \/ ReleaseReviewDenied(id)
       \/ ReleaseReviewEscalated(id)
       \/ ReleaseReviewUnavailable(id)
       \/ ReleaseReviewRetired(id)
    /\ audit_ended' = [audit_ended EXCEPT ![id] = TRUE]
    /\ UNCHANGED << audit_issued, audit_spends, audit_admitted, audit_finished,
                    audit_released, audit_step_unspent, audit_step_after_end,
                    audit_end_in_flight, audit_refused_closing,
                    audit_refused_after_release >>

\* A repeated hand-back after the dispatch end, refused while the effect
\* closes and after the release (core hands an attempt back once).
AuditReleaseReviewRejectedClosing(id) ==
    /\ audit_ended[id]
    /\ ReleaseReviewRejectedClosing(id)
    /\ UNCHANGED auditVars

AuditReleaseReviewRejectedMissing(id) ==
    /\ audit_ended[id]
    /\ ReleaseReviewRejectedMissing(id)
    /\ UNCHANGED auditVars

AuditNext ==
    \/ IssueReview
    \/ IssueConsent
    \/ ReviewSettles
    \/ \E id \in AuditIds : AuditRetireReviewRejectedSettled(id)
    \/ ConsentSettles
    \/ AuditWithdrawConsentRejectedMismatch
    \/ AuditConsumeReviewForEntry
    \/ AuditConsumeConsentForEntry
    \/ AuditConsumeReviewRejectedUsed
    \/ AuditConsumeConsentRejectedAlreadyConsumed
    \/ SpendRefused
    \/ \E id \in AuditIds : AuditAdmitEnteredStep(id)
    \/ \E id \in AuditIds : AuditAdmitEnteredStepRejectedMissing(id)
    \/ \E id \in AuditIds : AuditAdmitEnteredStepRejectedConsentLinked(id)
    \/ \E id \in AuditIds : AuditAdmitEnteredStepRejectedRetired(id)
    \/ \E id \in AuditIds : AuditAdmitEnteredStepRejectedNotEntered(id)
    \/ \E id \in AuditIds : AuditAdmitEnteredStepRejectedClosing(id)
    \/ \E id \in AuditIds : AuditFinishEnteredStepOpen(id)
    \/ \E id \in AuditIds : AuditFinishEnteredStepClosing(id)
    \/ \E id \in AuditIds : AuditFinishEnteredStepReleases(id)
    \/ \E id \in AuditIds : AuditFinishEnteredStepRejectedMissing(id)
    \/ \E id \in AuditIds : AuditFinishEnteredStepRejectedNotInFlight(id)
    \/ \E id \in AuditIds : AuditReleaseReviewUsed(id)
    \/ \E id \in AuditIds : AuditReleaseReviewUsedClosing(id)
    \/ \E id \in AuditIds : DispatchEndsUnspent(id)
    \/ \E id \in AuditIds : AuditReleaseReviewRejectedClosing(id)
    \/ \E id \in AuditIds : AuditReleaseReviewRejectedMissing(id)

AuditSpec == AuditInit /\ [][AuditNext]_<<vars, auditVars>>

AuditStateConstraint == model_step_count <= AuditMaxSteps

AuditOneSpendPerAttempt == \A id \in AuditIds : audit_spends[id] <= 1

AuditStepOnlyAfterSpend == ~audit_step_unspent

AuditNoStepAfterDispatchEnd == ~audit_step_after_end

AuditFinishesNeverExceedAdmissions ==
    \A id \in AuditIds : audit_finished[id] <= audit_admitted[id]

AuditReleaseWaitsForSteps ==
    \A id \in AuditIds : InFlight(id) > 0 => RegisteredUsed(id)

AuditEnteredStateMatchesHistory ==
    \A id \in AuditIds :
        /\ RegisteredUsed(id)
               <=> (audit_spends[id] = 1 /\ (~audit_ended[id] \/ InFlight(id) > 0))
        /\ audit_released[id]
               <=> (audit_spends[id] = 1 /\ audit_ended[id] /\ InFlight(id) = 0)
        /\ id \in DOMAIN review_entered_steps
               => review_entered_steps[id] = InFlight(id)
        /\ id \in DOMAIN review_entered_phase
               => ((review_entered_phase[id] = "Closing") <=> audit_ended[id])

AuditStepsNeverSpendOrSettle ==
    [][(audit_admitted' # audit_admitted
          \/ (audit_finished' # audit_finished /\ audit_released' = audit_released))
         => UNCHANGED << review_ids, review_statuses, review_retirements,
                         review_consent_approval, approval_consent_review,
                         approval_consent_consumed, approval_statuses >>
      ]_<<vars, auditVars>>

\* Reachability goals, each checked as an expected violation of its negation.
\* The dispatch ended while an admitted step was in flight: a new step was
\* refused while the effect closed, and the attempt was released (by that
\* step's finish: AuditEnteredStateMatchesHistory).
AuditWitnessCancelWhileStepInFlight ==
    \E id \in AuditIds :
        audit_end_in_flight[id] /\ audit_refused_closing[id] /\ audit_released[id]
\* A step refused after its attempt was released.
AuditWitnessRefusedAfterRelease ==
    \E id \in AuditIds : audit_refused_after_release[id]
\* The helper graph admission and two child steps under one consent spend.
AuditWitnessGraphAndTwoChildrenAfterConsent ==
    audit_admitted[AuditConsent] >= 3 /\ audit_spends[AuditConsent] = 1

NotAuditWitnessCancelWhileStepInFlight == ~AuditWitnessCancelWhileStepInFlight
NotAuditWitnessRefusedAfterRelease == ~AuditWitnessRefusedAfterRelease
NotAuditWitnessGraphAndTwoChildrenAfterConsent == ~AuditWitnessGraphAndTwoChildrenAfterConsent
====
