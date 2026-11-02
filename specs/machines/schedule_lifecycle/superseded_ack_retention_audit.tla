---- MODULE superseded_ack_retention_audit ----
\* Hand-written bounded audit of the retention of superseded_ack_ids in the
\* generated ScheduleLifecycleMachine model.
\*
\* A revise or delete supersedes every outstanding occurrence of the
\* schedule at its new revision, and each superseded occurrence acknowledges
\* back (ConfirmOccurrencesSuperseded { occurrence_id, superseding_revision })
\* into superseded_ack_ids. The set holds only the acknowledgements of the
\* latest supersession, the one at the current revision: Revise* and Delete*
\* (the only arms that bump the revision) drop every recorded id before the
\* new supersession's acknowledgements arrive, and
\* ConfirmOccurrencesSuperseded* record an acknowledgement only when it
\* answers the current revision. Any other acknowledgement (a late one of an
\* earlier supersession, whose id a later revise or delete dropped, or one of
\* a revision the schedule never reached) is accepted in every phase and
\* changes nothing.
\*
\* This audit keeps the generated model unchanged and drives one schedule
\* through revises from Active and Paused, deletes from Active and Paused, the
\* repeated delete, pause and resume, and acknowledgements of two occurrence
\* ids for every revision the bound reaches and one past it, in every phase,
\* before and after the supersession they answer. A history variable records
\* each acknowledgement that answered the current revision when it arrived.
\* Properties:
\*
\*   AuditAckSetAnswersTheLatestSupersession: every recorded id was
\*     acknowledged while the revision was the current one (no id of an
\*     earlier supersession survives a revise or delete, and no late
\*     acknowledgement is recorded).
\*   AuditLatestSupersessionAcksAreKept: every acknowledgement that answered
\*     the current revision is recorded (no arm drops the latest
\*     supersession's acknowledgements).
\*
\* Together they say the set is exactly the ids acknowledged at the current
\* revision, whatever came before.
\*
\* Reachability (the runner): three scenarios are checked as expected
\* violations of their negations: a revise dropped an earlier supersession's
\* acknowledgement while the latest one is recorded
\* (AuditWitnessReviseDropsEarlierAck); a late acknowledgement of a dropped
\* id was accepted and left the set without it
\* (AuditWitnessLateAckAccepted); a delete dropped an earlier supersession's
\* acknowledgement and recorded its own (AuditWitnessDeleteDropsEarlierAck).
\* With --mutants the runner seeds one defect at a time into a copy of the
\* generated model (each revise and delete arm keeping the old ids, the Active
\* and Paused acknowledgement arms recording late acknowledgements, the Paused
\* and Deleted ones dropping current acknowledgements) and requires TLC to
\* refuse it with the property it breaks.
\*
\* The TLC config is DERIVED from the generated ci.cfg on every run by
\* superseded_ack_retention_audit.sh:
\*   specs/machines/schedule_lifecycle/superseded_ack_retention_audit.sh 6 --mutants
EXTENDS model

CONSTANT AuditMaxSteps

AuditIds == {"occurrenceid_1", "occurrenceid_2"}
\* Every revision the bound reaches (one bump per step at most, from 1) and
\* one past it: an acknowledgement of a revision never reached.
AuditRevisions == 1..(AuditMaxSteps + 2)

VARIABLES
    audit_acked,          \* <<id, r>>: id acknowledged while the revision was r
    audit_late_accepted   \* a late acknowledgement of a dropped id was accepted

auditVars == << audit_acked, audit_late_accepted >>

AuditInit ==
    /\ Init
    /\ audit_acked = {}
    /\ audit_late_accepted = FALSE

\* --- environment -------------------------------------------------------
\* One fixed argument tuple per arm: no arm reads these values for the
\* acknowledgements, and the generated domains hold one value each.

AuditReviseActive ==
    /\ ReviseActive("trigger-0", "target-0", "Skip", "SkipIfRunning", "MarkMisfired", 1, 1, 0)
    /\ UNCHANGED auditVars

AuditRevisePaused ==
    /\ RevisePaused("trigger-0", "target-0", "Skip", "SkipIfRunning", "MarkMisfired", 1, 1, 0)
    /\ UNCHANGED auditVars

AuditDeleteActive ==
    /\ DeleteActive(0)
    /\ UNCHANGED auditVars

AuditDeletePaused ==
    /\ DeletePaused(0)
    /\ UNCHANGED auditVars

AuditDeleteDeleted ==
    /\ DeleteDeleted(0)
    /\ UNCHANGED auditVars

AuditPause ==
    /\ PauseActiveOrPaused(0)
    /\ UNCHANGED auditVars

AuditResume ==
    /\ ResumeActiveOrPaused(0)
    /\ UNCHANGED auditVars

\* An acknowledgement of `id` for revision `r`, in whichever phase the
\* schedule is: every ConfirmOccurrencesSuperseded arm accepts it.
AuditConfirm(id, r) ==
    /\ \/ ConfirmOccurrencesSupersededActive(id, r)
       \/ ConfirmOccurrencesSupersededPaused(id, r)
       \/ ConfirmOccurrencesSupersededDeleted(id, r)
    /\ audit_acked' = IF r = revision THEN audit_acked \cup {<<id, r>>} ELSE audit_acked
    /\ audit_late_accepted' =
           \/ audit_late_accepted
           \/ /\ r < revision
              /\ <<id, r>> \in audit_acked
              /\ id \notin superseded_ack_ids'

AuditNext ==
    \/ AuditReviseActive
    \/ AuditRevisePaused
    \/ AuditDeleteActive
    \/ AuditDeletePaused
    \/ AuditDeleteDeleted
    \/ AuditPause
    \/ AuditResume
    \/ \E id \in AuditIds, r \in AuditRevisions : AuditConfirm(id, r)

AuditSpec == AuditInit /\ [][AuditNext]_<< vars, auditVars >>

AuditStateConstraint == model_step_count <= AuditMaxSteps

\* --- properties --------------------------------------------------------

AuditAckSetAnswersTheLatestSupersession ==
    \A id \in superseded_ack_ids : <<id, revision>> \in audit_acked

AuditLatestSupersessionAcksAreKept ==
    \A ack \in audit_acked : ack[2] = revision => ack[1] \in superseded_ack_ids

\* --- witnesses (the runner requires each negation violated) -----------

AuditEarlierAckDropped ==
    /\ superseded_ack_ids # {}
    /\ \E early \in audit_acked : early[2] < revision /\ early[1] \notin superseded_ack_ids

AuditWitnessReviseDropsEarlierAck == phase # "Deleted" /\ AuditEarlierAckDropped
NotAuditWitnessReviseDropsEarlierAck == ~AuditWitnessReviseDropsEarlierAck

AuditWitnessLateAckAccepted == audit_late_accepted
NotAuditWitnessLateAckAccepted == ~AuditWitnessLateAckAccepted

AuditWitnessDeleteDropsEarlierAck == phase = "Deleted" /\ AuditEarlierAckDropped
NotAuditWitnessDeleteDropsEarlierAck == ~AuditWitnessDeleteDropsEarlierAck

=============================================================================
