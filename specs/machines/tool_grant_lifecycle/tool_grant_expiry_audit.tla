---- MODULE tool_grant_expiry_audit ----
\* Hand-written bounded audit of scoped tool grant expiry over the generated
\* ToolGrantLifecycleMachine model.
\*
\* One grant is issued with a fixed deadline. The generated machine variables
\* ARE its durable row: the grant service holds no state, and every
\* adjudication (a model-request boundary, a gated dispatch, a list or audit
\* read, a revoke, a deadline timer fire) rebuilds the machine from the row
\* and applies one generated input at the host clock it reads. A crash
\* therefore loses only process-local state: the session's projection of the
\* gated name (visible, visible_at) and the armed deadline timer.
\*
\* Explored, at every interleaving: issuing the grant from any process;
\* the host clock advancing one millisecond at a time and rolling back once
\* (the clock is host-trusted, not rollback-resistant); the served session's
\* deadline timer firing early or due (Expire); the boundary applying
\* AdmitUse and showing the name only on admission; a gated dispatch
\* applying its own fresh AdmitUse right before the hand-off, so the
\* admission time is the hand-off time; an operator revoke from any
\* process; a read from any process settling a due grant (Expire) without
\* loading the session; and the serving process crashing and restarting,
\* the restored gate starting closed.
\*
\* Properties:
\*
\*   AuditNoAdmitAtOrAfterDeadline: no gated call is handed on at or after
\*     the deadline.
\*   AuditTerminalNeverAdmits: no gated call is handed on once the grant
\*     expired or was revoked.
\*   AuditTerminalLatched (action property): an expired or revoked grant
\*     never changes again, across crash, restart and clock rollback.
\*   AuditExpiryNotEarly: an expired grant was observed at or after its
\*     deadline.
\*   AuditRevokedBeforeDeadline: a revoked grant ended before its deadline.
\*   AuditVisibleOnlyWhileLive: while the gated name is visible, the
\*     boundary that showed it ran before the deadline.
\*
\* AuditMutant selects a seeded shell defect (tool_grant_expiry_audit.sh
\* seeds the machine defects into a copy of the generated model instead):
\*   "dispatch_reuses_boundary_verdict": the dispatch admits a call because
\*     the boundary showed the name, without a fresh AdmitUse.
\*   "restart_reactivates_terminal": a restart reloads an ended grant as
\*     live.
\*   "revoked_admits": the dispatch hands on a call the revoked grant
\*     refused.
\*   "boundary_trusts_durable_phase": the boundary shows the name because
\*     the stored row is still Active, without applying AdmitUse at the
\*     current clock.
EXTENDS model

CONSTANTS AuditMaxSteps, AuditDeadline, AuditMaxLifetime, AuditClockMax, AuditGrantId,
          AuditMutant

ASSUME AuditDeadline \in 1..AuditClockMax
ASSUME AuditMaxLifetime >= AuditDeadline
ASSUME AuditMutant \in {"none", "dispatch_reuses_boundary_verdict",
                        "restart_reactivates_terminal", "revoked_admits",
                        "boundary_trusts_durable_phase"}

VARIABLES
    \* The host wall clock and its one bounded rollback.
    clock, rollback_used,
    \* Whether the serving process is up.
    up,
    \* Process-local: the session shows the gated name, the clock reading at
    \* the boundary that showed it, and whether the deadline timer is armed.
    visible, visible_at, timer,
    \* History: the clock readings at which gated calls were handed on,
    \* whether a call was refused at a reading before the deadline, the last
    \* dispatch outcome, which driver observed the expiry, whether the
    \* deadline passed while the process was down, whether a call was handed
    \* on after the grant ended, and whether the clock rolled back after it
    \* ended.
    admissions, refused_before_deadline, last_dispatch, expired_by, down_at_deadline,
    admitted_after_end, rolled_back_after_end

audit_vars == << clock, rollback_used, up, visible, visible_at, timer, admissions,
                 refused_before_deadline, last_dispatch, expired_by, down_at_deadline,
                 admitted_after_end, rolled_back_after_end >>

all_vars == << vars, audit_vars >>

\* The grant ended. Expired and Revoked are terminal.
Ended == phase = "Expired" \/ phase = "Revoked"

AuditInit ==
    /\ Init
    /\ clock = 0
    /\ rollback_used = FALSE
    /\ up = TRUE
    /\ visible = FALSE
    /\ visible_at = 0
    /\ timer = FALSE
    /\ admissions = {}
    /\ refused_before_deadline = FALSE
    /\ last_dispatch = "none"
    /\ expired_by = "none"
    /\ down_at_deadline = FALSE
    /\ admitted_after_end = FALSE
    /\ rolled_back_after_end = FALSE

\* An operator issues the grant from any process; a past or empty window is
\* refused. A served session's deadline timer is armed by the grant.
AuditIssue ==
    /\ \/ Issue(AuditGrantId, clock, AuditDeadline, AuditMaxLifetime)
       \/ IssueRefusedWindow(AuditGrantId, clock, AuditDeadline, AuditMaxLifetime)
    /\ timer' = (up /\ phase' = "Active")
    /\ UNCHANGED << clock, rollback_used, up, visible, visible_at, admissions,
                    refused_before_deadline, last_dispatch, expired_by, down_at_deadline,
                    admitted_after_end, rolled_back_after_end >>

\* The host clock advances one millisecond.
AuditTick ==
    /\ clock < AuditClockMax
    /\ clock' = clock + 1
    /\ down_at_deadline' = (down_at_deadline \/ (~up /\ clock' = AuditDeadline))
    /\ UNCHANGED vars
    /\ UNCHANGED << rollback_used, up, visible, visible_at, timer, admissions,
                    refused_before_deadline, last_dispatch, expired_by, admitted_after_end,
                    rolled_back_after_end >>

\* One bounded rollback of the host clock.
AuditRollback ==
    /\ ~rollback_used
    /\ clock > 0
    /\ clock' = clock - 1
    /\ rollback_used' = TRUE
    /\ rolled_back_after_end' = (rolled_back_after_end \/ Ended)
    /\ UNCHANGED vars
    /\ UNCHANGED << up, visible, visible_at, timer, admissions, refused_before_deadline,
                    last_dispatch, expired_by, down_at_deadline, admitted_after_end >>

\* The served session's deadline timer fires, early or due, and applies
\* Expire at the current clock. An early fire changes nothing and re-arms.
\* The timer never wakes or loads anything.
AuditTimerFire ==
    /\ up
    /\ timer
    /\ \/ /\ ExpireDue(clock)
          /\ expired_by' = "timer"
          /\ timer' = FALSE
       \/ /\ ExpireNotDue(clock)
          /\ timer' = TRUE
          /\ UNCHANGED expired_by
       \/ /\ (ExpireSettledExpired(clock) \/ ExpireSettledRevoked(clock))
          /\ timer' = FALSE
          /\ UNCHANGED expired_by
    /\ UNCHANGED << clock, rollback_used, up, visible, visible_at, admissions,
                    refused_before_deadline, last_dispatch, down_at_deadline,
                    admitted_after_end, rolled_back_after_end >>

\* The model-request boundary: the service applies AdmitUse at the current
\* clock, and the session shows the gated name only on UseAdmitted. Every
\* boundary arms the timer while the grant is live.
AuditBoundary ==
    /\ up
    /\ IF AuditMutant = "boundary_trusts_durable_phase"
       THEN /\ visible' = (phase = "Active")
            /\ visible_at' = clock
            /\ UNCHANGED vars
            /\ UNCHANGED expired_by
       ELSE \/ /\ AdmitLive(clock)
               /\ visible' = TRUE
               /\ visible_at' = clock
               /\ UNCHANGED expired_by
            \/ /\ AdmitDue(clock)
               /\ visible' = FALSE
               /\ visible_at' = 0
               /\ expired_by' = "boundary"
            \/ /\ (AdmitEndedExpired(clock) \/ AdmitEndedRevoked(clock))
               /\ visible' = FALSE
               /\ visible_at' = 0
               /\ UNCHANGED expired_by
            \/ /\ phase = "Unissued"
               /\ visible' = FALSE
               /\ visible_at' = 0
               /\ UNCHANGED vars
               /\ UNCHANGED expired_by
    /\ timer' = (phase' = "Active")
    /\ UNCHANGED << clock, rollback_used, up, admissions, refused_before_deadline,
                    last_dispatch, down_at_deadline, admitted_after_end,
                    rolled_back_after_end >>

\* The gated call is handed on at the current clock.
Admit ==
    /\ admissions' = admissions \cup {clock}
    /\ last_dispatch' = "admitted"
    /\ admitted_after_end' = (admitted_after_end \/ Ended)
    /\ UNCHANGED refused_before_deadline

\* The gated call is refused at the current clock.
Refuse ==
    /\ refused_before_deadline' = (refused_before_deadline \/ clock < AuditDeadline)
    /\ last_dispatch' = "refused"
    /\ UNCHANGED << admissions, admitted_after_end >>

\* A gated call: the dispatcher applies its own AdmitUse at the current clock
\* after every other await of its entry, and hands the call on only on
\* UseAdmitted. Nothing awaits between the comparison and the hand-off.
AuditDispatch ==
    /\ up
    /\ IF AuditMutant = "dispatch_reuses_boundary_verdict"
       THEN /\ visible
            /\ UNCHANGED vars
            /\ Admit
            /\ UNCHANGED expired_by
       ELSE \/ /\ AdmitLive(clock)
               /\ Admit
               /\ UNCHANGED expired_by
            \/ /\ AdmitDue(clock)
               /\ Refuse
               /\ expired_by' = "dispatch"
            \/ /\ AdmitEndedExpired(clock)
               /\ Refuse
               /\ UNCHANGED expired_by
            \/ /\ AdmitEndedRevoked(clock)
               /\ IF AuditMutant = "revoked_admits" THEN Admit ELSE Refuse
               /\ UNCHANGED expired_by
    /\ UNCHANGED << clock, rollback_used, up, visible, visible_at, timer, down_at_deadline,
                    rolled_back_after_end >>

\* An operator revokes the grant from any process. A revoke at or after the
\* deadline records the expiry instead.
AuditRevoke ==
    /\ \/ /\ RevokeActive(clock)
          /\ UNCHANGED expired_by
       \/ /\ RevokeAfterDeadline(clock)
          /\ expired_by' = "revoke"
       \/ /\ (RevokeSettledExpired(clock) \/ RevokeSettledRevoked(clock))
          /\ UNCHANGED expired_by
    /\ UNCHANGED << clock, rollback_used, up, visible, visible_at, timer, admissions,
                    refused_before_deadline, last_dispatch, down_at_deadline,
                    admitted_after_end, rolled_back_after_end >>

\* A list or audit read from any process settles a due grant with Expire. It
\* touches only the grant row: it neither loads nor wakes the session.
AuditRead ==
    /\ \/ /\ ExpireDue(clock)
          /\ expired_by' = "read"
       \/ /\ (ExpireNotDue(clock) \/ ExpireSettledExpired(clock) \/ ExpireSettledRevoked(clock))
          /\ UNCHANGED expired_by
    /\ UNCHANGED << clock, rollback_used, up, visible, visible_at, timer, admissions,
                    refused_before_deadline, last_dispatch, down_at_deadline,
                    admitted_after_end, rolled_back_after_end >>

\* The serving process dies. The durable row survives; the projection and
\* the armed timer are process-local and are lost.
AuditCrash ==
    /\ up
    /\ up' = FALSE
    /\ visible' = FALSE
    /\ visible_at' = 0
    /\ timer' = FALSE
    /\ UNCHANGED vars
    /\ UNCHANGED << clock, rollback_used, admissions, refused_before_deadline,
                    last_dispatch, expired_by, down_at_deadline, admitted_after_end,
                    rolled_back_after_end >>

\* A new process serves the session. Every build clears the open names it
\* restores, so the gate restores closed; nothing settles the row until the
\* first boundary, dispatch, read or timer fire.
AuditRestart ==
    /\ ~up
    /\ up' = TRUE
    /\ visible' = FALSE
    /\ visible_at' = 0
    /\ timer' = FALSE
    /\ IF AuditMutant = "restart_reactivates_terminal" /\ Ended
       THEN /\ phase' = "Active"
            /\ ended_at_ms' = None
            /\ end_reason' = None
            /\ UNCHANGED << model_step_count, grant_id, issued_at_ms, expires_at_ms >>
       ELSE UNCHANGED vars
    /\ UNCHANGED << clock, rollback_used, admissions, refused_before_deadline,
                    last_dispatch, expired_by, down_at_deadline, admitted_after_end,
                    rolled_back_after_end >>

AuditNext ==
    \/ AuditIssue
    \/ AuditTick
    \/ AuditRollback
    \/ AuditTimerFire
    \/ AuditBoundary
    \/ AuditDispatch
    \/ AuditRevoke
    \/ AuditRead
    \/ AuditCrash
    \/ AuditRestart

AuditSpec == AuditInit /\ [][AuditNext]_all_vars

AuditStateConstraint == model_step_count <= AuditMaxSteps

\* --- Properties -------------------------------------------------------------

AuditNoAdmitAtOrAfterDeadline == \A t \in admissions : t < AuditDeadline

AuditTerminalNeverAdmits == ~admitted_after_end

AuditTerminalLatched ==
    [][Ended => /\ phase' = phase
               /\ ended_at_ms' = ended_at_ms
               /\ end_reason' = end_reason]_all_vars

AuditExpiryNotEarly ==
    phase = "Expired" => (ended_at_ms # None /\ ended_at_ms.value >= expires_at_ms)

AuditRevokedBeforeDeadline ==
    phase = "Revoked" => (ended_at_ms # None /\ ended_at_ms.value < expires_at_ms)

AuditVisibleOnlyWhileLive == visible => visible_at < AuditDeadline

\* --- Reachability witnesses: each negation must be violated ----------------

\* The deadline timer observed the expiry.
NotAuditWitnessExpiredByTimer == ~(expired_by = "timer")

\* A boundary before the deadline showed the name, and the racing call
\* adjudicated at the deadline expired the grant and was refused.
NotAuditWitnessExpiredAtDispatch ==
    ~(expired_by = "dispatch" /\ last_dispatch = "refused" /\ visible)

\* The deadline passed while the process was down, and a read settled it.
NotAuditWitnessExpiredWhileDownSettledOnRead ==
    ~(expired_by = "read" /\ down_at_deadline)

\* A call adjudicated one millisecond before the deadline was handed on.
NotAuditWitnessAdmittedOneTickBeforeDeadline ==
    ~((AuditDeadline - 1) \in admissions)

\* After the expiry, the clock rolled back before the deadline, and a call
\* adjudicated there was still refused.
NotAuditWitnessRollbackAfterExpiryStillRefused ==
    ~(rolled_back_after_end /\ phase = "Expired" /\ refused_before_deadline)

\* A revoked grant refused a call.
NotAuditWitnessRevokedThenRefused ==
    ~(phase = "Revoked" /\ last_dispatch = "refused")

\* A revoke at the deadline recorded expiry, not revocation.
NotAuditWitnessRevokeAfterDeadlineRecordsExpiry == ~(expired_by = "revoke")

====
